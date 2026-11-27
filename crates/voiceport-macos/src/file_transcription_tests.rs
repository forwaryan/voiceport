use super::*;
use crate::{
    controller::Controller,
    local_models::{CHATGPT, Model, ModelKind},
    plugins::{Source, Store},
};
use std::{
    collections::BTreeMap,
    ffi::CString,
    fs,
    os::unix::ffi::OsStrExt,
    time::{Duration, Instant},
};

fn wav(directory: &Path) -> std::path::PathBuf {
    let path = directory.join("selected.wav");
    fs::write(&path, crate::plugin_runtime::sample()).unwrap();
    path
}

fn http_catalog(endpoint: &str) -> Catalog {
    let store = Store::default();
    let plugin = store.plugin(crate::plugins::HTTP).unwrap();
    let source = Source {
        id: "remote-source".into(),
        name: "Synthetic HTTP".into(),
        enabled: true,
        plugin_id: plugin.id,
        plugin_version: plugin.version,
        config: BTreeMap::from([
            ("endpoint".into(), endpoint.into()),
            ("model".into(), "synthetic".into()),
        ]),
        credential_refs: BTreeMap::new(),
    };
    let session = crate::plugin_runtime::Snapshot::configured(&store, &source).unwrap();
    let mut catalog = Catalog::pending(CHATGPT);
    catalog.entries.push(crate::local_models::Entry {
        id: source.id,
        title: source.name,
        detail: String::new(),
        source: Ok(SessionSource::Plugin(Box::new(session))),
    });
    catalog
}

fn wait(jobs: &mut Jobs) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while jobs.busy() {
        jobs.poll();
        assert!(Instant::now() < deadline, "job did not settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn unexpected(_: &Plan, _: &Path, _: &CancellationToken, _: &Custody) -> Result<String, Error> {
    panic!("adapter dispatcher must remain unreachable");
}

#[cfg(target_arch = "aarch64")]
#[test]
fn standalone_file_fixture_validates_then_disposes_its_snapshot_on_success_and_cancel() {
    let folder = tempfile::tempdir().unwrap();
    let selected = wav(folder.path());
    let mut staged = None;
    let cancel = CancellationToken::new();
    let text = transcribe_local_input(&selected, &cancel, |audio| {
        assert_ne!(audio, selected);
        assert_eq!(fs::read(audio).unwrap(), crate::plugin_runtime::sample());
        staged = Some(audio.to_owned());
        Ok(" fixture transcript \n".into())
    });
    assert_eq!(text.unwrap(), "fixture transcript");
    assert!(!staged.take().unwrap().exists());
    let result = transcribe_local_input(&selected, &cancel, |audio| {
        staged = Some(audio.to_owned());
        cancel.cancel();
        Err(TRANSCRIPTION)
    });
    assert_eq!(result, Err(CANCELLED));
    assert!(!staged.unwrap().exists());
    assert!(selected.is_file());
}

#[cfg(target_arch = "aarch64")]
#[test]
fn standalone_file_fixture_rejects_missing_and_malformed_audio_before_recognition() {
    let folder = tempfile::tempdir().unwrap();
    let selected = folder.path().join("input.wav");
    let cancel = CancellationToken::new();
    let unexpected = |_: &Path| panic!("invalid audio must not reach recognition");
    assert_eq!(
        transcribe_local_input(&selected, &cancel, unexpected),
        Err(INVALID_FILE)
    );
    fs::write(&selected, b"RIFF\x10\0\0\0WAVEbroken").unwrap();
    assert_eq!(
        transcribe_local_input(&selected, &cancel, unexpected),
        Err(INVALID_AUDIO)
    );
    cancel.cancel();
    assert_eq!(
        transcribe_local_input(&selected, &cancel, unexpected),
        Err(CANCELLED)
    );
}

#[test]
fn missing_source_confirmation_target_and_config_change_do_not_dispatch() {
    let folder = tempfile::tempdir().unwrap();
    let input = wav(folder.path());
    let mut c = Controller::new();
    c.file_jobs.use_test_transcriber(unexpected);
    c.catalog = http_catalog("https://example.invalid/api/audio?synthetic-secret=1");
    assert_eq!(
        Plan::resolve(&c.catalog, "missing", "en").err(),
        Some(UNAVAILABLE)
    );
    let mut nonbatch = c.catalog.entries[1].source.clone().unwrap();
    if let SessionSource::Plugin(session) = &mut nonbatch {
        session.manifest.capabilities.batch = false;
    }
    c.catalog.entries.push(crate::local_models::Entry {
        id: "streaming-only".into(),
        title: "Streaming Only".into(),
        detail: String::new(),
        source: Ok(nonbatch),
    });
    assert_eq!(
        Plan::resolve(&c.catalog, "streaming-only", "en")
            .err()
            .unwrap()
            .code,
        "unsupported_source"
    );
    assert!(
        !sources(&c.catalog, "en")
            .iter()
            .any(|target| target.source == "streaming-only")
    );
    let plan = Plan::resolve(&c.catalog, "remote-source", "auto").unwrap();
    let response = c
        .start_prepared_file(Input::open(&input).unwrap(), plan, false)
        .unwrap_err();
    assert_eq!(response.code, "confirmation_required");
    let encoded = serde_json::to_value(&response).unwrap();
    assert_eq!(encoded["target"]["source"], "remote-source");
    assert_eq!(encoded["target"]["language"], "");
    assert_eq!(encoded["target"]["destination"]["kind"], "http");
    assert_eq!(
        encoded["target"]["destination"]["origin"],
        "https://example.invalid"
    );
    assert!(!encoded.to_string().contains("synthetic-secret"));
    assert_eq!(c.file_jobs.generation, 0);
    assert!(!c.file_jobs.busy());
    let plan = Plan::resolve(&c.catalog, "remote-source", "en").unwrap();
    c.catalog = http_catalog("https://example.invalid/different-private-path");
    assert_eq!(
        c.start_prepared_file(Input::open(&input).unwrap(), plan, true)
            .unwrap_err()
            .code,
        "source_changed"
    );
    assert_eq!(c.file_jobs.generation, 0);
    assert_eq!(c.source_id, CHATGPT);
    assert_eq!(fs::read(input).unwrap(), crate::plugin_runtime::sample());
}

#[test]
fn validates_file_type_size_codec_duration_and_source_mutation_before_adapter() {
    let folder = tempfile::tempdir().unwrap();
    let path = wav(folder.path());
    let dir = folder.path().join("directory.wav");
    let link = folder.path().join("link.wav");
    let fifo = folder.path().join("stream.wav");
    fs::create_dir(&dir).unwrap();
    std::os::unix::fs::symlink(&path, &link).unwrap();
    let fifo_path = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
    for rejected in [
        dir.as_path(),
        link.as_path(),
        fifo.as_path(),
        Path::new("selected.wav"),
    ] {
        assert_eq!(Input::open(rejected).err(), Some(INVALID_FILE));
    }
    assert_eq!(
        Input::open(&folder.path().join("audio.ogg")).err(),
        Some(INVALID_AUDIO)
    );
    let sparse = folder.path().join("oversized.wav");
    File::create(&sparse)
        .unwrap()
        .set_len(MAXIMUM_AUDIO_BYTES + 1)
        .unwrap();
    assert_eq!(Input::open(&sparse).err(), Some(TOO_LARGE));
    let pending = Input::open(&path).unwrap();
    let invalid = b"Not a WAV file!";
    fs::write(&path, invalid).unwrap();
    let mut jobs = Jobs::default();
    jobs.use_test_transcriber(unexpected);
    let start = |jobs: &mut Jobs, input: Input| {
        jobs.start(
            input,
            Plan::resolve(&Catalog::pending(CHATGPT), CHATGPT, "").unwrap(),
            true,
        )
        .unwrap()
    };
    let changed = start(&mut jobs, pending);
    wait(&mut jobs);
    assert_eq!(
        jobs.status(&changed.operation).unwrap().error,
        Some(CHANGED)
    );
    let mislabeled = start(&mut jobs, Input::open(&path).unwrap());
    wait(&mut jobs);
    assert_eq!(
        jobs.status(&mislabeled.operation).unwrap().error,
        Some(INVALID_AUDIO)
    );
    assert_eq!(fs::read(&path).unwrap(), invalid);
    let header_only = b"RIFF\xff\xff\xff\x7fWAVEgarbage";
    fs::write(&path, header_only).unwrap();
    let bad_codec = start(&mut jobs, Input::open(&path).unwrap());
    wait(&mut jobs);
    assert_eq!(
        jobs.status(&bad_codec.operation).unwrap().error,
        Some(INVALID_AUDIO)
    );
    assert_eq!(fs::read(&path).unwrap(), header_only);
    let mut too_long = crate::plugin_runtime::sample();
    let audio_bytes = (181 * 16_000 * 2) as u32;
    too_long.resize(44 + audio_bytes as usize, 0);
    too_long[4..8].copy_from_slice(&(36 + audio_bytes).to_le_bytes());
    too_long[40..44].copy_from_slice(&audio_bytes.to_le_bytes());
    fs::write(&path, &too_long).unwrap();
    let long = start(&mut jobs, Input::open(&path).unwrap());
    wait(&mut jobs);
    assert_eq!(
        jobs.status(&long.operation).unwrap().error,
        Some(INVALID_AUDIO)
    );
    assert_eq!(fs::read(&path).unwrap(), too_long);
}

#[test]
fn live_and_file_are_exclusive_cancellation_discards_late_result_and_ids_do_not_alias() {
    use std::sync::{Mutex, OnceLock};
    type Channels = (mpsc::Sender<()>, mpsc::Receiver<()>);
    static GATE: OnceLock<Mutex<Option<Channels>>> = OnceLock::new();
    fn slow(_: &Plan, _: &Path, cancel: &CancellationToken, _: &Custody) -> Result<String, Error> {
        let (entered, released) = GATE.get().unwrap().lock().unwrap().take().unwrap();
        entered.send(()).unwrap();
        released.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(cancel.is_cancelled());
        // A decoder that doesn't honor the cancellation still returns a late result.
        Ok("stale late completion".into())
    }
    fn success(_: &Plan, _: &Path, _: &CancellationToken, _: &Custody) -> Result<String, Error> {
        Ok("fresh result".into())
    }
    let (entered, wait_for_entry) = mpsc::channel();
    let (release_worker, released) = mpsc::channel();
    GATE.get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap()
        .replace((entered, released));
    let folder = tempfile::tempdir().unwrap();
    let path = wav(folder.path());
    let mut c = Controller::new();
    c.file_jobs.use_test_transcriber(slow);
    let start = |c: &mut Controller| {
        let plan = Plan::resolve(&c.catalog, CHATGPT, "en").unwrap();
        c.start_prepared_file(Input::open(&path).unwrap(), plan, true)
    };
    c.state.phase = crate::lifecycle::Phase::Recording;
    assert_eq!(start(&mut c).unwrap_err(), BUSY);
    c.state.phase = crate::lifecycle::Phase::Idle;
    let first = start(&mut c).unwrap();
    wait_for_entry.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(c.busy());
    assert!(
        c.start_pinned_with_permissions(SessionSource::ChatGpt, "en".into(), true, || panic!(
            "capture must not be attempted"
        ))
        .is_none()
    );
    assert_eq!(start(&mut c).unwrap_err(), BUSY);
    assert_eq!(
        c.file_jobs.cancel("no-other-operation").unwrap_err(),
        MISSING
    );
    assert_eq!(
        c.file_jobs.cancel(&first.operation).unwrap().state,
        State::Cancelling
    );
    assert_eq!(
        c.file_jobs.cancel(&first.operation).unwrap().state,
        State::Cancelling
    );
    assert_eq!(c.file_jobs.result(&first.operation).unwrap().1, None);
    assert_eq!(start(&mut c).unwrap_err(), BUSY);
    release_worker.send(()).unwrap();
    wait(&mut c.file_jobs);
    assert_eq!(
        c.file_jobs.status(&first.operation).unwrap().state,
        State::Cancelled
    );
    assert_eq!(c.file_jobs.result(&first.operation).unwrap().1, None);
    c.file_jobs.use_test_transcriber(success);
    let second = start(&mut c).unwrap();
    assert_ne!(first.operation, second.operation);
    assert_eq!(first.generation + 1, second.generation);
    assert_eq!(
        c.file_jobs.cancel(&first.operation).unwrap().state,
        State::Cancelled
    );
    wait(&mut c.file_jobs);
    assert_eq!(
        c.file_jobs.result(&second.operation).unwrap().1,
        Some("fresh result")
    );
    assert_eq!(c.file_jobs.result(&first.operation).unwrap().1, None);
    c.file_jobs.keep_for_page(&second.operation);
    for _ in 0..=RECEIPTS {
        start(&mut c).unwrap();
        wait(&mut c.file_jobs);
    }
    assert_eq!(c.file_jobs.completed.len(), RECEIPTS);
    assert_eq!(c.file_jobs.status(&first.operation).unwrap_err(), MISSING);
    assert_eq!(
        c.file_jobs.result(&second.operation).unwrap().1,
        Some("fresh result")
    );
    assert_eq!(c.state.generation(), 0);
    assert_eq!(c.input_method(), None);
}

#[test]
fn upload_wav_keeps_audio_but_strips_original_container_chunks() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("source.wav");
    let marker = b"private-metadata";
    let mut original = crate::plugin_runtime::sample();
    original.extend_from_slice(b"JUNK");
    original.extend_from_slice(&(marker.len() as u32).to_le_bytes());
    original.extend_from_slice(marker);
    let riff_length = (original.len() - 8) as u32;
    original[4..8].copy_from_slice(&riff_length.to_le_bytes());
    fs::write(&path, &original).unwrap();
    let upload = stage_upload_wav(&path, &CancellationToken::new()).unwrap();
    let upload_bytes = fs::read(upload).unwrap();
    assert_eq!(upload_bytes, crate::plugin_runtime::sample());
    assert_eq!(fs::read(path).unwrap(), original);
}

#[test]
fn local_language_capability_matches_the_real_model_and_has_no_upload_gate() {
    let mut catalog = Catalog::pending(CHATGPT);
    catalog.entries.push(crate::local_models::Entry {
        id: "local".into(),
        title: "Local".into(),
        detail: String::new(),
        source: Ok(SessionSource::Local(Model {
            kind: ModelKind::ParakeetUnified,
            directory: "/synthetic".into(),
            runtime: "/synthetic".into(),
            encoder: "/synthetic".into(),
            decoder: "/synthetic".into(),
            joiner: None,
            tokens: "/synthetic".into(),
        })),
    });
    assert_eq!(
        Plan::resolve(&catalog, "local", "zh").err().unwrap().code,
        "unsupported_language"
    );
    assert!(
        !Plan::resolve(&catalog, "local", "en")
            .unwrap()
            .target
            .destination
            .requires_confirmation()
    );
}
