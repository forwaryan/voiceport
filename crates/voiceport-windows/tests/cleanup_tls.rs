#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::{
    cell::RefCell,
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
use voiceport_windows::{
    Job, Recording,
    actions::{Action, Actions, ManualClipboard},
    executor::{Executor, OsSpawner, Service, ServiceFailure, Spawner},
};
#[cfg(unix)]
unsafe extern "C" {
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}
fn open(fd: i32) -> bool {
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
struct Sink(Vec<String>);
impl ManualClipboard for Sink {
    fn copy(&mut self, s: &str) -> Result<(), String> {
        self.0.push(s.into());
        Ok(())
    }
}
struct Good(Arc<AtomicUsize>);
impl Service for Good {
    fn run(&self, _: &Job) -> Result<String, ServiceFailure> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok("cleanup TLS positive".into())
    }
}
fn recording(root: &Path, name: &str) -> (Recording, i32) {
    let path = root.join(name);
    let file = File::create_new(&path).unwrap();
    #[cfg(unix)]
    let fd = file.as_raw_fd();
    #[cfg(not(unix))]
    let fd = -1;
    (Recording::caller_owned(file, path, Some("en".into())), fd)
}
fn until(mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < end);
        thread::yield_now();
    }
}
struct Tail {
    work: Option<Box<dyn FnOnce() + Send>>,
    entered: mpsc::SyncSender<()>,
    release: mpsc::Receiver<()>,
}
impl Drop for Tail {
    fn drop(&mut self) {
        self.entered.send(()).unwrap();
        self.release.recv_timeout(Duration::from_secs(15)).unwrap();
        self.work.take().unwrap()();
    }
}
thread_local! { static CLEANUP_TAIL:RefCell<Option<Tail>>=const{RefCell::new(None)}; }
struct CleanupTls(Mutex<Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>>);
impl Spawner for CleanupTls {
    fn spawn(&self, work: Box<dyn FnOnce() + Send>) -> io::Result<JoinHandle<()>> {
        let (entered, release) = self.0.lock().unwrap().take().unwrap();
        // The returned actual OS-thread handle covers the supplied loop, including
        // its execution during thread-local destruction. No detached helper is used.
        // On Err OsSpawner drops the unstarted closure and all supplied work.
        OsSpawner.spawn(Box::new(move || {
            CLEANUP_TAIL.with(|slot| {
                *slot.borrow_mut() = Some(Tail {
                    work: Some(work),
                    entered,
                    release,
                })
            });
        }))
    }
}
struct DiscardCleanup;
impl Spawner for DiscardCleanup {
    fn spawn(&self, work: Box<dyn FnOnce() + Send>) -> io::Result<JoinHandle<()>> {
        OsSpawner.spawn(Box::new(move || drop(work)))
    }
}
fn run_case(root: PathBuf, mode: String) {
    let mode = &mode;
    let calls = Arc::new(AtomicUsize::new(0));
    let mut sink = Sink(vec![]);
    if mode == "discard" {
        // Inject infrastructure loss before the supplied cleanup loop starts;
        // this is not the lawful TLS-spawner case above.
        let mut ui = Actions::with_executor(Executor::with_spawners(
            Arc::new(Good(calls.clone())),
            Arc::new(OsSpawner),
            Arc::new(DiscardCleanup),
        ));
        let (r, fd) = recording(&root, "discard.wav");
        ui.submit_finalized(r).unwrap();
        until(|| {
            ui.act(Action::Poll, &mut sink);
            ui.can_retry()
        });
        assert!(open(fd));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        for _ in 0..8 {
            ui.act(Action::Retry, &mut sink);
            until(|| {
                ui.act(Action::Poll, &mut sink);
                ui.can_retry()
            });
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
        assert!(ui.act(Action::Exit, &mut sink));
        assert!(!open(fd));
        return;
    }
    if mode == "control" {
        let mut ui = Actions::with_executor(Executor::new(Arc::new(Good(calls.clone()))));
        let (r, fd) = recording(&root, "control.wav");
        ui.submit_finalized(r).unwrap();
        until(|| {
            assert!(!ui.act(Action::Poll, &mut sink));
            ui.can_copy()
        });
        assert!(!open(fd));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        ui.act(Action::Copy, &mut sink);
        assert_eq!(sink.0, ["cleanup TLS positive"]);
        assert!(ui.act(Action::Exit, &mut sink));
        println!(
            "{{\"case\":\"actual-default-cleanup-positive\",\"result_and_manual_copy\":true}}"
        );
        return;
    }
    let (tx, entered) = mpsc::sync_channel(1);
    let (release, gate) = mpsc::sync_channel(1);
    let cleanup = Arc::new(CleanupTls(Mutex::new(Some((tx, gate)))));
    let mut ui = Actions::with_executor(Executor::with_spawners(
        Arc::new(Good(calls.clone())),
        Arc::new(OsSpawner),
        cleanup,
    ));
    let (r, fd) = recording(&root, "held.wav");
    ui.submit_finalized(r).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(open(fd));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    let mut owner = Some(ui);
    let mut next = Actions::with_executor(Executor::new(Arc::new(Good(calls.clone()))));
    let observed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if mode == "orphan" {
            drop(owner.take());
            next.submit_finalized(recording(&root, "next.wav").0)
                .unwrap();
        } else if mode == "exit" {
            owner.as_mut().unwrap().act(Action::Exit, &mut sink);
        } else {
            owner.as_mut().unwrap().act(Action::Poll, &mut sink);
        }
    }));
    let panicked = observed.is_err();
    let panic_text = observed
        .err()
        .map(|p| {
            if let Some(s) = p.downcast_ref::<&str>() {
                (*s).to_owned()
            } else if let Some(s) = p.downcast_ref::<String>() {
                s.clone()
            } else {
                "other panic".into()
            }
        })
        .unwrap_or_default();
    assert!(open(fd));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(sink.0.is_empty());
    release.send(()).unwrap();
    if mode == "poll" {
        until(|| calls.load(Ordering::SeqCst) == 1);
        owner.as_mut().unwrap().act(Action::Cancel, &mut sink);
    }
    if let Some(a) = owner.as_mut() {
        until(|| {
            let closed = a.act(Action::Poll, &mut sink);
            if mode == "exit" {
                closed
            } else {
                a.status == "Cancelled; service stopped"
            }
        });
    }
    until(|| !open(fd));
    let service_calls = calls.load(Ordering::SeqCst);
    assert_eq!(service_calls, if mode == "poll" { 1 } else { 0 });
    assert!(next.act(Action::Exit, &mut sink));
    println!(
        "{{\"case\":\"cleanup-TLS-{mode}\",\"actual_OS_handle_covers_supplied_loop\":true,\"cleanup_loop_runs_in_owned_TLS_destructor\":true,\"destructor_entered_and_held\":true,\"worker_calls_before_release\":0,\"UI_panicked_before_release\":{panicked},\"panic_text\":\"{panic_text}\",\"File_retained_until_release\":true,\"service_calls_after_release\":{service_calls},\"recording_closed_after_release\":true}}"
    );
    assert!(!panicked);
    let mut healthy = Actions::with_executor(Executor::new(Arc::new(Good(calls.clone()))));
    healthy
        .submit_finalized(recording(&root, "healthy.wav").0)
        .unwrap();
    until(|| {
        healthy.act(Action::Poll, &mut sink);
        healthy.can_copy()
    });
    assert_eq!(calls.load(Ordering::SeqCst), service_calls + 1);
    assert!(healthy.act(Action::Exit, &mut sink));
}

#[test]
fn cleanup_tls_lifecycle() {
    if let Ok(mode) = std::env::var("VP013_CLEANUP_TEST_MODE") {
        run_case(
            PathBuf::from(std::env::var_os("VP013_CLEANUP_TEST_ROOT").unwrap()),
            mode,
        );
        return;
    }
    let owned = tempfile::tempdir().unwrap();
    for mode in ["control", "poll", "exit", "orphan", "discard"] {
        let root = owned.path().join(mode);
        std::fs::create_dir(&root).unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "cleanup_tls_lifecycle",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("VP013_CLEANUP_TEST_MODE", mode)
            .env("VP013_CLEANUP_TEST_ROOT", &root)
            .output()
            .unwrap();
        println!("{}", String::from_utf8_lossy(&out.stdout));
        eprintln!("{}", String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success(), "{mode} failed");
    }
}
