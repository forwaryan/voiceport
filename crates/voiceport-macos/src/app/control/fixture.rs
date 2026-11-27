//! Real CLI and production socket/Delegate routing in a fixture-owned AppKit process.
mod audio_start;
mod dictionary;
mod update;
pub use audio_start::{isolated_audio_start_regression, isolated_performance_baseline};
pub use dictionary::isolated_dictionary_exchange_regression;

use super::*;
use crate::{
    controller::recent_dictations::fixture::SyntheticDictations,
    local_models::Entry,
    plugin_runtime::{Snapshot, sample},
};
use objc2::AnyThread;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

const SOURCE: &str = "fixture-file-http";
const HISTORY_TEXT: &str = "Synthetic GUI selection 聽寫";
const FILE_TEXT: &str = "Synthetic file result";
const LIMIT: Duration = Duration::from_secs(8);

struct Defaults {
    name: Retained<NSString>,
    store: Retained<NSUserDefaults>,
}
impl Defaults {
    fn new() -> Self {
        let name = NSString::from_str(&format!(
            "voiceport.control.fixture.{}",
            uuid::Uuid::new_v4()
        ));
        let store =
            NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&name)).unwrap();
        Self { name, store }
    }
}
impl Drop for Defaults {
    fn drop(&mut self) {
        self.store.removePersistentDomainForName(&self.name);
    }
}

struct Fixture<'a> {
    cli: &'a Path,
    socket: PathBuf,
    owner: Retained<Delegate>,
}
impl Fixture<'_> {
    fn spawn(&self, words: &[&str]) -> Child {
        Command::new(self.cli)
            .arg("--json")
            .arg("--socket")
            .arg(&self.socket)
            .args(words)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    fn call(&self, words: &[&str], expected: i32) -> Value {
        let mut child = self.spawn(words);
        // A bounded control response can still exceed a child pipe's capacity.
        let mut stdout = child.stdout.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let stdout_reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let stderr_reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).unwrap();
            bytes
        });
        let deadline = Instant::now() + LIMIT;
        while child.try_wait().unwrap().is_none() {
            if self.owner.ivars().timer.get().is_some() {
                audio_start::pump();
            } else {
                self.owner.with_controller(|c| c.file_jobs.poll());
                self.owner.poll_control();
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("CLI did not return for {words:?}");
            }
            thread::sleep(Duration::from_millis(2));
        }
        let status = child.wait().unwrap();
        let stdout = stdout_reader.join().unwrap();
        let stderr = stderr_reader.join().unwrap();
        assert_eq!(
            status.code(),
            Some(expected),
            "{words:?}: {}",
            String::from_utf8_lossy(&stdout)
        );
        assert!(stderr.is_empty(), "{words:?}: {stderr:?}");
        serde_json::from_slice(&stdout).unwrap()
    }

    fn finish_gui(&self, rig: &mut SyntheticDictations, text: &str) -> usize {
        let mut slot = self.owner.ivars().controller.borrow_mut();
        let c = slot.as_mut().unwrap();
        std::mem::swap(c, &mut rig.controller);
        let (generation, receiver) = rig.begin(true, true);
        rig.finish(generation, text);
        std::mem::swap(c, &mut rig.controller);
        receiver.borrow().writes
    }

    fn plan(&self, input: &Path) -> String {
        let reply = self.call(
            &[
                "file",
                "plan",
                input.to_str().unwrap(),
                "--source",
                SOURCE,
                "--language",
                "en",
            ],
            0,
        );
        let data = &reply["data"];
        assert_eq!(data["state"], "planned");
        assert_eq!(data["target"]["destination"]["kind"], "http");
        assert_eq!(data["target"]["destination"]["loopback"], true);
        assert_eq!(data["delivery"], "none");
        data["operation"].as_str().unwrap().to_owned()
    }

    fn lose_start_reply(&self, operation: &str) {
        let mut child = self.spawn(&["file", "start", operation, "--confirm"]);
        let mut c = self.owner.ivars().control.borrow_mut();
        let deadline = Instant::now() + Duration::from_secs(3);
        let pending = loop {
            if let Some(pending) = c.server.as_ref().and_then(Server::next) {
                break pending;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("confirmed start did not reach the socket");
            }
            thread::sleep(Duration::from_millis(2));
        };
        // The request reached the App. Disconnect the caller before dispatch so
        // it cannot see the reply even though the operation gets accepted.
        child.kill().unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.stdout.is_empty());
        self.owner.dispatch_control(&mut c, pending);
    }

    fn wait_for(&self, job: &str, expected: &str) -> Value {
        let deadline = Instant::now() + LIMIT;
        loop {
            let reply = self.call(&["file", "status", job], 0);
            let data = &reply["data"];
            assert_eq!(data["delivery"], "none");
            if data["state"] == expected {
                return reply;
            }
            assert!(
                matches!(data["state"].as_str(), Some("working" | "cancelling")),
                "{data}"
            );
            assert!(Instant::now() < deadline, "{data}");
        }
    }
}

fn http_request(connection: &mut TcpStream) -> Vec<u8> {
    connection.set_nonblocking(false).unwrap();
    connection.set_read_timeout(Some(LIMIT)).unwrap();
    let mut reader = BufReader::new(connection);
    let mut headers = String::new();
    loop {
        let offset = headers.len();
        assert_ne!(reader.read_line(&mut headers).unwrap(), 0);
        if headers[offset..] == *"\r\n" {
            break;
        }
    }
    let length = headers
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap();
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    let mut request = headers.into_bytes();
    request.extend(body);
    request
}

fn loopback(listener: TcpListener, requests: mpsc::Sender<Vec<u8>>, finish: Receiver<()>) {
    listener.set_nonblocking(true).unwrap();
    for index in 0..4 {
        let deadline = Instant::now() + LIMIT;
        let mut connection = loop {
            match listener.accept() {
                Ok((connection, _)) => break connection,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "missing loopback request {index}"
                    );
                    thread::sleep(Duration::from_millis(2));
                }
                Err(e) => panic!("loopback accept: {e}"),
            }
        };
        requests.send(http_request(&mut connection)).unwrap();
        if index == 3 {
            finish.recv_timeout(LIMIT).unwrap();
        } else {
            let body = format!(r#"{{"text":"{FILE_TEXT}"}}"#);
            write!(
                connection,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        }
    }
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
}

fn install_source(c: &mut Controller, endpoint: &str) {
    let source = Source {
        id: SOURCE.into(),
        name: "Synthetic loopback".into(),
        plugin_id: crate::plugins::HTTP.into(),
        plugin_version: "1".into(),
        enabled: true,
        config: BTreeMap::from([
            ("endpoint".into(), endpoint.into()),
            ("model".into(), "fixture-file".into()),
        ]),
        credential_refs: BTreeMap::new(),
    };
    let snapshot = Snapshot::configured(&Store::default(), &source).unwrap();
    c.catalog.entries.retain(|entry| entry.id != SOURCE);
    c.catalog.entries.push(Entry {
        id: source.id,
        title: source.name,
        detail: String::new(),
        source: Ok(SessionSource::Plugin(Box::new(snapshot))),
    });
}

fn synthetic_audio(folder: &Path) -> [PathBuf; 3] {
    let wav = folder.join("private-fixture.wav");
    fs::write(&wav, sample()).unwrap();
    let m4a = folder.join("private-fixture.m4a");
    assert!(
        Command::new("/usr/bin/afconvert")
            .args(["-f", "m4af", "-d", "aac"])
            .arg(&wav)
            .arg(&m4a)
            .status()
            .unwrap()
            .success()
    );
    // MPEG-1 Layer III, 128 kb/s, 44.1 kHz. Zeroed side information
    // declares no main data and decodes to silence.
    let mut frame = [0u8; 417];
    frame[..4].copy_from_slice(&[0xff, 0xfb, 0x90, 0x00]);
    let mp3 = folder.join("private-fixture.mp3");
    fs::write(&mp3, frame.repeat(40)).unwrap();
    [wav, m4a, mp3]
}

pub fn isolated_control_regression(mtm: MainThreadMarker, cli: &Path) {
    let app = NSApplication::sharedApplication(mtm);
    let folder = tempfile::tempdir_in("/tmp").unwrap();
    let folder = folder.path().canonicalize().unwrap();
    let socket = folder.join("ipc/control.sock");
    let owner = Delegate::new(mtm);
    let defaults = Defaults::new();
    assert!(!super::super::recent_dictations::preference(
        &defaults.store
    ));
    assert!(
        owner
            .ivars()
            .recent_dictations_fixture_defaults
            .set(defaults.store.clone())
            .is_ok()
    );
    let mut rig = SyntheticDictations::new();
    owner
        .ivars()
        .controller
        .replace(Some(std::mem::take(&mut rig.controller)));
    owner.ivars().control.borrow_mut().server = Some(Server::bind(&socket).unwrap());
    let fixture = Fixture { cli, socket, owner };
    let call = |words: &[&str], exit| fixture.call(words, exit);
    assert_eq!(
        call(&["history", "list"], 0)["data"],
        json!({"enabled":false,"entries":[]})
    );
    assert_eq!(fixture.finish_gui(&mut rig, "disabled"), 1);
    assert_eq!(call(&["history", "list"], 0)["data"]["entries"], json!([]));
    assert_eq!(
        call(&["history", "settings", "on"], 0)["data"]["enabled"],
        true
    );
    assert!(super::super::recent_dictations::preference(&defaults.store));
    fixture.finish_gui(&mut rig, "expired");
    let oldest = call(&["history", "list"], 0)["data"]["entries"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for n in 0..5 {
        fixture.finish_gui(&mut rig, &format!("bounded entry {n}"));
    }
    assert_eq!(
        call(&["history", "show", &oldest], 6)["error"]["code"],
        "not_found"
    );
    fixture.finish_gui(&mut rig, &"x".repeat(17 * 1024));
    let metadata = call(&["history", "list", "--limit", "1"], 0);
    let oversized = &metadata["data"]["entries"][0];
    assert_eq!(oversized["textAvailable"], false);
    assert_eq!(
        call(&["history", "show", oversized["id"].as_str().unwrap()], 1)["error"]["code"],
        "text_unavailable"
    );
    assert_eq!(fixture.finish_gui(&mut rig, HISTORY_TEXT), 1);
    let history = call(&["history", "list"], 0);
    assert_eq!(history["data"]["entries"].as_array().unwrap().len(), 5);
    assert!(!history.to_string().contains(HISTORY_TEXT));
    let id = history["data"]["entries"][0]["id"].as_str().unwrap();
    assert_eq!(
        call(&["history", "show", id], 0)["data"]["text"],
        HISTORY_TEXT
    );
    assert_eq!(
        call(&["history", "clear"], 5)["error"]["code"],
        "confirmation_required"
    );
    assert_eq!(
        call(&["history", "settings", "off"], 5)["error"]["code"],
        "confirmation_required"
    );

    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let endpoint = format!(
        "http://{}/full/transcriptions",
        listener.local_addr().unwrap()
    );
    let (requests, received) = mpsc::channel();
    let (finish, finished) = mpsc::channel();
    fixture
        .owner
        .with_controller(|c| install_source(c, &endpoint));
    let dictation_state = || {
        let slot = fixture.owner.ivars().controller.borrow();
        let c = slot.as_ref().unwrap();
        json!({
            "generation":c.state.generation(), "text":c.transcript, "method":c.input_method(),
            "gui_receipt":c.gui_dictation_receipt(), "source":c.source_id, "language":c.language,
        })
    };
    let before = dictation_state();
    let generation = before["generation"].as_u64().unwrap();
    assert_eq!(
        call(&["dictation", "gui-result", &generation.to_string()], 0)["data"]["text"],
        HISTORY_TEXT
    );
    assert_eq!(
        call(
            &["dictation", "gui-result", &(generation - 1).to_string()],
            6
        )["error"]["code"],
        "not_found"
    );
    assert_eq!(
        call(&["file", "sources", "--language", "en"], 0)["data"]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["source"] == SOURCE)
            .unwrap()["destination"]["origin"],
        endpoint.trim_end_matches("/full/transcriptions")
    );
    let audio = synthetic_audio(&folder);
    let original = audio.each_ref().map(|path| fs::read(path).unwrap());
    let invalidated = fixture.plan(&audio[0]);
    assert_eq!(
        call(
            &[
                "file",
                "plan",
                folder.join("missing.wav").to_str().unwrap(),
                "--source",
                SOURCE
            ],
            1,
        )["error"]["code"],
        "invalid_file"
    );
    assert_eq!(
        call(&["file", "start", &invalidated, "--confirm"], 6)["error"]["code"],
        "not_found"
    );
    let stale = fixture.plan(&audio[0]);
    fixture
        .owner
        .with_controller(|c| install_source(c, &format!("{endpoint}/changed")));
    assert_eq!(
        call(&["file", "start", &stale, "--confirm"], 1)["error"]["code"],
        "source_changed"
    );
    assert_eq!(
        call(&["file", "status", &stale], 1)["data"]["state"],
        "failed"
    );
    fixture
        .owner
        .with_controller(|c| install_source(c, &endpoint));
    let proposal = fixture.plan(&audio[0]);
    assert_eq!(
        call(&["file", "start", &proposal], 5)["data"]["state"],
        "planned"
    );
    listener.set_nonblocking(true).unwrap();
    assert!(matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock));
    let server = thread::spawn(move || loopback(listener, requests, finished));
    fixture.lose_start_reply(&proposal);
    let canonical = fixture.wait_for(&proposal, "ready")["data"]["operation"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(canonical, proposal);
    assert_eq!(
        call(&["file", "start", &proposal, "--confirm"], 0)["data"]["operation"],
        canonical
    );
    for path in &audio[1..] {
        let proposal = fixture.plan(path);
        assert_eq!(
            call(&["file", "start", &proposal, "--confirm"], 0)["data"]["state"],
            "working"
        );
        let ready = fixture.wait_for(&proposal, "ready");
        let job = ready["data"]["operation"].as_str().unwrap();
        assert_eq!(call(&["file", "result", job], 0)["data"]["text"], FILE_TEXT);
    }
    let result = call(&["file", "result", &canonical], 0);
    assert_eq!(result["data"]["text"], FILE_TEXT);
    assert_eq!(result["data"]["delivery"], "none");
    let output = folder.join("result.md");
    let export = [
        "file",
        "export",
        &canonical,
        "--output",
        output.to_str().unwrap(),
        "--format",
        "markdown",
    ];
    assert_eq!(call(&export, 0)["data"]["exported"], true);
    let markdown = fs::read_to_string(&output).unwrap();
    assert!(markdown.contains(FILE_TEXT));
    assert_eq!(call(&export, 1)["status"], "error");
    assert_eq!(fs::read_to_string(output).unwrap(), markdown);
    let cancel = fixture.plan(&audio[0]);
    assert_eq!(
        call(&["file", "start", &cancel, "--confirm"], 0)["data"]["state"],
        "working"
    );
    for _ in 0..4 {
        let request = received.recv_timeout(LIMIT).unwrap();
        let request = String::from_utf8_lossy(&request);
        assert!(request.starts_with("POST /full/transcriptions HTTP/1.1"));
        assert!(request.contains("fixture-file"));
        assert!(request.contains("name=\"language\"\r\n\r\nen"));
        assert!(request.contains("filename=\"dictation.wav\""));
        assert!(!request.contains("private-fixture"));
    }
    assert_eq!(call(&["app", "quit"], 4)["error"]["code"], "busy");
    assert_eq!(
        call(&["file", "cancel", &cancel], 0)["data"]["state"],
        "cancelling"
    );
    fixture.wait_for(&cancel, "cancelled");
    assert_eq!(
        call(&["file", "result", &cancel], 130)["data"]["state"],
        "cancelled"
    );
    finish.send(()).unwrap();
    server.join().unwrap();
    assert_eq!(
        audio.each_ref().map(|path| fs::read(path).unwrap()),
        original
    );
    assert_eq!(call(&["history", "list"], 0), history);
    assert_eq!(
        call(&["history", "show", id], 0)["data"]["text"],
        HISTORY_TEXT
    );
    assert_eq!(dictation_state(), before);
    update::verify(&fixture, &folder);
    assert_eq!(call(&["history", "list"], 0), history);
    assert_eq!(call(&["file", "result", &canonical], 0), result);
    assert_eq!(dictation_state(), before);
    assert_eq!(
        call(&["history", "clear", "--confirm"], 0)["data"]["removed"],
        5
    );
    assert_eq!(
        call(&["history", "show", id], 6)["error"]["code"],
        "not_found"
    );
    fixture.finish_gui(&mut rig, "disable erases");
    assert_eq!(
        call(&["history", "settings", "off", "--confirm"], 0)["data"]["enabled"],
        false
    );
    assert!(!super::super::recent_dictations::preference(
        &defaults.store
    ));
    assert_eq!(call(&["history", "list"], 0)["data"]["entries"], json!([]));
    call(&["history", "settings", "on"], 0);
    fixture.finish_gui(&mut rig, "shutdown erases");
    assert_eq!(call(&["app", "quit"], 0)["data"]["quitting"], true);
    assert_eq!(
        call(&["app", "update", "check"], 4)["data"]["state"],
        "quitting"
    );
    assert_eq!(call(&["history", "list"], 4)["error"]["code"], "busy");
    assert_eq!(
        fixture
            .owner
            .should_terminate(sel!(applicationShouldTerminate:), &app),
        NSApplicationTerminateReply::TerminateNow
    );
    fixture.owner.with_controller(|c| {
        assert!(!c.recent_dictations_enabled());
        assert!(c.list_recent_dictations(5).is_empty());
    });
    assert!(super::super::recent_dictations::preference(&defaults.store));
    assert!(!fixture.socket.exists());
    println!(
        "PASS real CLI -> same-UID socket -> Delegate/Controller: history default/consent/metadata/explicit text/eviction/clear/disable/quit; production loopback WAV/M4A/MP3, source change, pre-consent silence, lost start reply, export no-clobber, cancel release, GUI/history/input unchanged"
    );
}
