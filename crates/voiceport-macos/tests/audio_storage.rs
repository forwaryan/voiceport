#![cfg(target_os = "macos")]
//! Isolated storage tests. No microphone, permission request, Speech recognizer,
//! network, user cache, or running app recording is accessed.
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use voiceport_macos::audio::Recorder;
use voiceport_macos::storage::{NAMESPACE, RETENTION, Storage};

fn isolated() -> (tempfile::TempDir, Storage) {
    let temp = tempfile::tempdir().unwrap();
    // macOS /var and /tmp are symlinks, deliberately unsupported by this store.
    let root = temp.path().canonicalize().unwrap().join(NAMESPACE);
    (temp, Storage::at(root))
}
fn created_time() -> SystemTime {
    UNIX_EPOCH
        + Duration::from_secs(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
}
fn crash_fixture(store: &Storage, created: SystemTime) -> PathBuf {
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_fixture_child", "--nocapture"])
        .env("VOICEPORT_ISOLATED_STORAGE_FIXTURE", store.path())
        .env(
            "VOICEPORT_ISOLATED_STORAGE_CREATED",
            created
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs()
                .to_string(),
        )
        .status()
        .unwrap();
    assert!(status.success());
    fs::read_dir(store.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .unwrap()
}

#[test]
fn crash_fixture_child() {
    let Some(root) = std::env::var_os("VOICEPORT_ISOLATED_STORAGE_FIXTURE") else {
        return;
    };
    let created: u64 = std::env::var("VOICEPORT_ISOLATED_STORAGE_CREATED")
        .unwrap()
        .parse()
        .unwrap();
    let recording = Storage::at(root)
        .create_recording_at(UNIX_EPOCH + Duration::from_secs(created))
        .unwrap();
    // Simulate process disappearance: no Rust Drop cleanup, kernel releases flock
    // when this dedicated child test process exits.
    std::mem::forget(recording);
}

#[test]
fn constructors_and_missing_root_cleanup_are_inert() {
    let (_temp, store) = isolated();
    let recorder = Recorder::new();
    assert!(!store.path().exists());
    drop(recorder);
    let report = store.cleanup_expired_at(SystemTime::now());
    assert!(report.root_unavailable);
    assert!(!store.path().exists());
}

#[test]
fn private_modes_and_last_arc_keep_the_worker_recording_alive() {
    let (_temp, store) = isolated();
    let original = Arc::new(store.create_recording().unwrap());
    let session = original.path().parent().unwrap().to_path_buf();
    let audio = original.path().to_path_buf();
    assert_eq!(fs::metadata(store.path()).unwrap().mode() & 0o7777, 0o700);
    assert_eq!(fs::metadata(&session).unwrap().mode() & 0o7777, 0o700);
    for name in ["dictation.m4a", "session.json", "lease"] {
        let metadata = fs::metadata(session.join(name)).unwrap();
        assert_eq!(metadata.mode() & 0o7777, 0o600);
        assert_eq!(metadata.uid(), unsafe { libc::geteuid() });
        assert_eq!(metadata.nlink(), 1);
    }
    fs::write(&audio, b"synthetic worker payload").unwrap();
    let worker = original.clone();
    drop(original);
    assert!(audio.exists());
    let later = SystemTime::now() + RETENTION + Duration::from_secs(10);
    let report = store.cleanup_expired_at(later);
    assert_eq!(report.active, 1);
    assert_eq!(report.removed, 0);
    std::thread::spawn(move || {
        assert_eq!(
            fs::read(worker.path()).unwrap(),
            b"synthetic worker payload"
        );
        drop(worker);
    })
    .join()
    .unwrap();
    assert!(!session.exists());
    assert!(store.path().join("owner.json").exists());
}

#[test]
fn expired_crashed_session_is_removed_only_strictly_after_24_hours() {
    let (_temp, store) = isolated();
    let created = created_time();
    let session = crash_fixture(&store, created);
    let boundary = store.cleanup_expired_at(created + RETENTION);
    assert_eq!(boundary.retained, 1);
    assert_eq!(boundary.removed, 0);
    assert!(session.exists());
    let expired = store.cleanup_expired_at(created + RETENTION + Duration::from_secs(1));
    assert_eq!(expired.removed, 1);
    assert!(!session.exists());
}

#[test]
fn unknown_session_content_prevents_drop_and_launch_deletion() {
    let (_temp, store) = isolated();
    let recording = store.create_recording().unwrap();
    let session = recording.path().parent().unwrap().to_path_buf();
    fs::write(session.join("keep-user-data"), b"unknown").unwrap();
    drop(recording);
    assert_eq!(
        fs::read(session.join("keep-user-data")).unwrap(),
        b"unknown"
    );
    let report = store.cleanup_expired_at(SystemTime::now() + RETENTION + Duration::from_secs(5));
    assert_eq!(report.retained, 1);
    assert_eq!(report.removed, 0);
    assert!(session.join("dictation.m4a").exists());
}

#[test]
fn unknown_root_entries_and_legacy_namespace_are_retained() {
    let (temp, store) = isolated();
    let recording = store.create_recording().unwrap();
    drop(recording);
    fs::write(store.path().join("unrecognized"), b"keep").unwrap();
    let legacy = temp.path().join("io.voiceport.recordings-v1");
    fs::create_dir(&legacy).unwrap();
    fs::write(legacy.join("dictation.m4a"), b"running Swift app").unwrap();
    let report = store.cleanup_expired_at(SystemTime::now() + RETENTION + Duration::from_secs(5));
    assert_eq!(report.retained, 1);
    assert_eq!(
        fs::read(legacy.join("dictation.m4a")).unwrap(),
        b"running Swift app"
    );
    assert_eq!(
        fs::read(store.path().join("unrecognized")).unwrap(),
        b"keep"
    );
}

#[test]
fn symlink_root_and_symlink_ancestor_are_never_followed() {
    let (temp, store) = isolated();
    let base = temp.path().canonicalize().unwrap();
    let target = base.join("target");
    fs::create_dir(&target).unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o700)).unwrap();
    symlink(&target, store.path()).unwrap();
    assert!(store.create_recording().is_err());
    assert!(store.cleanup_expired_at(SystemTime::now()).root_unavailable);
    let ancestor = base.join("ancestor");
    symlink(&target, &ancestor).unwrap();
    assert!(
        Storage::at(ancestor.join(NAMESPACE))
            .create_recording()
            .is_err()
    );
    assert_eq!(fs::read_dir(target).unwrap().count(), 0);
}

#[test]
fn linked_audio_is_retained_and_external_content_is_untouched() {
    for hard_link in [false, true] {
        let (temp, store) = isolated();
        let created = created_time();
        let session = crash_fixture(&store, created);
        let target = temp.path().join("external-data");
        fs::write(&target, b"must survive").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        fs::remove_file(session.join("dictation.m4a")).unwrap();
        if hard_link {
            fs::hard_link(&target, session.join("dictation.m4a")).unwrap()
        } else {
            symlink(&target, session.join("dictation.m4a")).unwrap()
        }
        let report = store.cleanup_expired_at(created + RETENTION + Duration::from_secs(5));
        assert_eq!(report.removed, 0);
        assert_eq!(report.retained, 1);
        assert_eq!(fs::read(target).unwrap(), b"must survive");
    }
}

fn edit_marker(session: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let path = session.join("session.json");
    let mut marker = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    edit(&mut marker);
    fs::write(path, serde_json::to_vec(&marker).unwrap()).unwrap();
}

#[test]
fn unrecognized_markers_and_future_creation_times_are_retained() {
    for alteration in 0..4 {
        let (_temp, store) = isolated();
        let created = created_time();
        let session = crash_fixture(&store, created);
        let now = created + RETENTION + Duration::from_secs(5);
        edit_marker(&session, |marker| match alteration {
            0 => marker["unknown"] = serde_json::json!(true),
            1 => marker["owner_uid"] = serde_json::json!(u32::MAX),
            2 => {
                marker["created_at"] = serde_json::json!(
                    (now + Duration::from_secs(60))
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs()
                )
            }
            _ => marker["root_id"] = serde_json::json!(uuid::Uuid::new_v4().to_string()),
        });
        let report = store.cleanup_expired_at(now);
        assert_eq!(report.retained, 1, "alteration {alteration}");
        assert_eq!(report.removed, 0);
        assert!(session.exists());
    }
}

#[test]
fn insecure_file_mode_and_future_file_timestamp_prevent_cleanup() {
    for future in [false, true] {
        let (_temp, store) = isolated();
        let created = created_time();
        let session = crash_fixture(&store, created);
        let now = created + RETENTION + Duration::from_secs(5);
        let audio = session.join("dictation.m4a");
        if future {
            fs::File::open(&audio)
                .unwrap()
                .set_modified(now + Duration::from_secs(5))
                .unwrap()
        } else {
            fs::set_permissions(&audio, fs::Permissions::from_mode(0o644)).unwrap()
        }
        let report = store.cleanup_expired_at(now);
        assert_eq!(report.retained, 1);
        assert_eq!(report.removed, 0);
        assert!(audio.exists());
    }
}

#[test]
fn replacing_audio_with_another_regular_file_prevents_owner_drop_deletion() {
    let (_temp, store) = isolated();
    let recording = store.create_recording().unwrap();
    let audio = recording.path().to_path_buf();
    let session = audio.parent().unwrap().to_path_buf();
    fs::remove_file(&audio).unwrap();
    fs::write(&audio, b"unknown replacement").unwrap();
    fs::set_permissions(&audio, fs::Permissions::from_mode(0o600)).unwrap();
    drop(recording);
    assert_eq!(fs::read(audio).unwrap(), b"unknown replacement");
    assert!(session.join("lease").exists());
}

#[test]
fn replacing_root_does_not_redirect_a_live_recording_drop() {
    let (temp, store) = isolated();
    let recording = store.create_recording().unwrap();
    let original_session_name = recording
        .path()
        .parent()
        .unwrap()
        .file_name()
        .unwrap()
        .to_owned();
    let moved = temp.path().canonicalize().unwrap().join("moved-root");
    fs::rename(store.path(), &moved).unwrap();
    fs::create_dir(store.path()).unwrap();
    let replacement = store.path().join(&original_session_name);
    fs::create_dir(&replacement).unwrap();
    fs::write(replacement.join("dictation.m4a"), b"replacement data").unwrap();
    drop(recording);
    assert_eq!(
        fs::read(replacement.join("dictation.m4a")).unwrap(),
        b"replacement data"
    );
    assert!(
        moved
            .join(original_session_name)
            .join("dictation.m4a")
            .exists()
    );
}
