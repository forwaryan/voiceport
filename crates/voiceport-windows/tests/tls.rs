#[path = "support/file_handle.rs"]
mod file_handle;

use std::{
    cell::RefCell,
    fs::File,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use voiceport_windows::{
    Job, Recording,
    actions::{Action, Actions, ManualClipboard},
    executor::{Executor, Service, ServiceFailure},
};
struct Sink(Vec<String>);
impl ManualClipboard for Sink {
    fn copy(&mut self, s: &str) -> Result<(), String> {
        self.0.push(s.into());
        Ok(())
    }
}
struct Hold {
    entered: mpsc::SyncSender<()>,
    release: mpsc::Receiver<()>,
}
impl Drop for Hold {
    fn drop(&mut self) {
        self.entered.send(()).unwrap();
        self.release.recv_timeout(Duration::from_secs(15)).unwrap();
    }
}
thread_local! {static TAIL:RefCell<Option<Hold>>=const{RefCell::new(None)};}
struct TlsService(Mutex<Option<Hold>>);
impl Service for TlsService {
    fn run(&self, _: &Job) -> Result<String, ServiceFailure> {
        let hold = self.0.lock().unwrap().take().unwrap();
        TAIL.with(|v| *v.borrow_mut() = Some(hold));
        Ok("current TLS result".into())
    }
}
struct Answer;
impl Service for Answer {
    fn run(&self, _: &Job) -> Result<String, ServiceFailure> {
        Ok("positive control".into())
    }
}
fn rec(root: &std::path::Path, name: &str) -> (Recording, file_handle::Handle) {
    let path = root.join(name);
    let file = File::create_new(&path).unwrap();
    let handle = file_handle::handle(&file);
    (
        Recording::caller_owned(file, path, Some("en".into())),
        handle,
    )
}
fn pump(a: &mut Actions, s: &mut Sink) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !a.can_copy() {
        assert!(Instant::now() < deadline);
        a.act(Action::Poll, s);
        thread::yield_now();
    }
}
#[test]
fn acknowledged_tls_teardown_is_nonblocking() {
    let owned = tempfile::tempdir().unwrap();
    let root = owned.path().to_path_buf();
    let mut failures = 0;
    // Actual OsSpawner positive control,not a parallel executor model.
    let mut normal = Actions::with_executor(Executor::new(Arc::new(Answer)));
    let mut sink = Sink(vec![]);
    normal
        .submit_finalized(rec(&root, "positive.wav").0)
        .unwrap();
    pump(&mut normal, &mut sink);
    normal.act(Action::Copy, &mut sink);
    assert_eq!(sink.0, ["positive control"]);
    assert!(normal.act(Action::Exit, &mut sink));
    drop(normal);
    for kind in [
        "ordinary-poll",
        "cancelled-poll",
        "closing-poll",
        "orphan-reap",
    ] {
        let (ready, probe_ready) = mpsc::sync_channel(1);
        let (start_probe, probe_input) = mpsc::sync_channel::<Option<(Actions, Sink)>>(1);
        let (started, observed) = mpsc::sync_channel(1);
        let (done, result) = mpsc::sync_channel(1);
        let root2 = root.clone();
        // A Windows TLS destructor holds the loader lock and blocks thread startup.
        // Start the probe before creating the service worker, then gate its action.
        let owner = thread::spawn(move || {
            ready.send(()).unwrap();
            let input = probe_input.recv().unwrap();
            if kind == "orphan-reap" {
                let mut next = Actions::with_executor(Executor::new(Arc::new(Answer)));
                let mut s = Sink(vec![]);
                started.send(()).unwrap();
                let t = Instant::now();
                next.submit_finalized(rec(&root2, "orphan-next.wav").0)
                    .unwrap();
                done.send(t.elapsed()).unwrap();
                let deadline = Instant::now() + Duration::from_secs(5);
                while !next.can_copy() {
                    assert!(Instant::now() < deadline);
                    if next.can_retry() {
                        next.act(Action::Retry, &mut s);
                    }
                    next.act(Action::Poll, &mut s);
                    thread::yield_now();
                }
                next.act(Action::Copy, &mut s);
                assert_eq!(s.0, ["positive control"]);
                assert!(next.act(Action::Exit, &mut s));
            } else {
                let (mut ui, mut sink) = input.unwrap();
                started.send(()).unwrap();
                let t = Instant::now();
                assert!(!ui.act(Action::Poll, &mut sink));
                done.send(t.elapsed()).unwrap();
                if kind == "ordinary-poll" {
                    pump(&mut ui, &mut sink);
                    ui.act(Action::Copy, &mut sink);
                    assert_eq!(sink.0, ["current TLS result"]);
                } else if kind == "closing-poll" {
                    let until = Instant::now() + Duration::from_secs(5);
                    while !ui.act(Action::Poll, &mut sink) {
                        assert!(Instant::now() < until);
                        thread::yield_now();
                    }
                    assert!(sink.0.is_empty());
                } else {
                    let until = Instant::now() + Duration::from_secs(5);
                    while ui.status != "Cancelled; service stopped" {
                        assert!(Instant::now() < until);
                        ui.act(Action::Poll, &mut sink);
                        thread::yield_now();
                    }
                    assert!(!ui.can_copy());
                    assert!(sink.0.is_empty());
                }
                assert!(ui.act(Action::Exit, &mut sink));
            }
        });
        probe_ready.recv_timeout(Duration::from_secs(5)).unwrap();

        let (entered, at_tail) = mpsc::sync_channel(1);
        let (release, held) = mpsc::sync_channel(1);
        let mut ui = Actions::with_executor(Executor::new(Arc::new(TlsService(Mutex::new(Some(
            Hold {
                entered,
                release: held,
            },
        ))))));
        let (recording, held_handle) = rec(&root, &format!("{kind}.wav"));
        ui.submit_finalized(recording).unwrap();
        // Only exercise the action after the real TLS destructor acknowledges entry.
        at_tail.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(file_handle::is_open(held_handle));
        let mut sink = Sink(vec![]);
        if kind == "cancelled-poll" {
            assert!(!ui.act(Action::Cancel, &mut sink));
        }
        if kind == "closing-poll" {
            assert!(!ui.act(Action::Exit, &mut sink));
        }
        let input = if kind == "orphan-reap" {
            let t = Instant::now();
            drop(ui);
            assert!(t.elapsed() < Duration::from_secs(1));
            None
        } else {
            Some((ui, sink))
        };
        start_probe.send(input).unwrap();
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        let response = result.recv_timeout(Duration::from_secs(1));
        let responsive = response.is_ok();
        if !responsive {
            failures += 1;
        }
        assert!(file_handle::is_open(held_handle));
        release.send(()).unwrap();
        let elapsed =
            response.unwrap_or_else(|_| result.recv_timeout(Duration::from_secs(5)).unwrap());
        owner.join().unwrap();
        println!(
            "{{\"case\":\"{kind}\",\"TLS_destructor_acknowledged_and_held\":true,\"uses_actual_OsSpawner\":true,\"expected\":\"nonblocking while TLS cleanup held\",\"responsive_before_release\":{responsive},\"elapsed_ms\":{},\"returns_after_explicit_release\":true}}",
            elapsed.as_millis()
        );
    }
    println!(
        "{{\"success_oracle_passed\":{},\"blocking_cases\":{failures},\"positive_current_result_and_cleanup_controls_passed\":true}}",
        failures == 0
    );
    assert_eq!(failures, 0);
}
