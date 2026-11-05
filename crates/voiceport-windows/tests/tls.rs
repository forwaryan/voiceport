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
static FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);
#[cfg(unix)]
unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32) -> i32;
}
fn descriptor_open(fd: i32) -> bool {
    #[cfg(unix)]
    {
        unsafe { fcntl(fd, 1) >= 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = fd;
        true
    }
}
fn rec(root: &std::path::Path, name: &str) -> Recording {
    let path = root.join(name);
    let file = File::create_new(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        FD.store(file.as_raw_fd(), std::sync::atomic::Ordering::SeqCst);
    }
    Recording {
        file,
        path,
        language: Some("en".into()),
    }
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
    normal.submit_finalized(rec(&root, "positive.wav")).unwrap();
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
        let (entered, at_tail) = mpsc::sync_channel(1);
        let (release, held) = mpsc::sync_channel(1);
        let mut ui = Actions::with_executor(Executor::new(Arc::new(TlsService(Mutex::new(Some(
            Hold {
                entered,
                release: held,
            },
        ))))));
        ui.submit_finalized(rec(&root, &format!("{kind}.wav")))
            .unwrap();
        at_tail.recv_timeout(Duration::from_secs(5)).unwrap();
        let held_fd = FD.load(std::sync::atomic::Ordering::SeqCst);
        assert!(descriptor_open(held_fd));
        let mut sink = Sink(vec![]);
        // Acknowledgement proves genuine OS thread-local destruction has begun after
        // the supplied service and work closure returned;the destructor is still held.
        if kind == "cancelled-poll" {
            assert!(!ui.act(Action::Cancel, &mut sink));
        }
        if kind == "closing-poll" {
            assert!(!ui.act(Action::Exit, &mut sink));
        }
        let (started, observed) = mpsc::sync_channel(1);
        let (done, result) = mpsc::sync_channel(1);
        let root2 = root.clone();
        let mode = kind.to_owned();
        let owner = if kind == "orphan-reap" {
            let t = Instant::now();
            drop(ui);
            assert!(t.elapsed() < Duration::from_secs(1));
            thread::spawn(move || {
                let mut next = Actions::with_executor(Executor::new(Arc::new(Answer)));
                let mut s = Sink(vec![]);
                started.send(()).unwrap();
                let t = Instant::now();
                next.submit_finalized(rec(&root2, "orphan-next.wav"))
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
            })
        } else {
            thread::spawn(move || {
                started.send(()).unwrap();
                let t = Instant::now();
                assert!(!ui.act(Action::Poll, &mut sink));
                done.send(t.elapsed()).unwrap();
                if mode == "ordinary-poll" {
                    pump(&mut ui, &mut sink);
                    ui.act(Action::Copy, &mut sink);
                    assert_eq!(sink.0, ["current TLS result"]);
                } else if mode == "closing-poll" {
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
            })
        };
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        let response = result.recv_timeout(Duration::from_secs(1));
        let responsive = response.is_ok();
        if !responsive {
            failures += 1;
        }
        assert!(descriptor_open(held_fd));
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
