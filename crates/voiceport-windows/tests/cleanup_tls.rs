#[path = "support/file_handle.rs"]
mod file_handle;

use file_handle::{Handle, is_open};
#[cfg(not(windows))]
use std::cell::RefCell;
use std::{
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
fn recording(root: &Path, name: &str) -> (Recording, Handle) {
    let path = root.join(name);
    let file = File::create_new(&path).unwrap();
    let handle = file_handle::handle(&file);
    (
        Recording::caller_owned(file, path, Some("en".into())),
        handle,
    )
}
fn until(stage: &str, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {stage}");
        thread::yield_now();
    }
}
#[cfg(not(windows))]
struct Tail {
    work: Option<Box<dyn FnOnce() + Send>>,
    entered: mpsc::SyncSender<()>,
    release: mpsc::Receiver<()>,
}
#[cfg(not(windows))]
impl Drop for Tail {
    fn drop(&mut self) {
        self.entered.send(()).unwrap();
        self.release.recv_timeout(Duration::from_secs(15)).unwrap();
        self.work.take().unwrap()();
    }
}
#[cfg(not(windows))]
thread_local! { static CLEANUP_TAIL:RefCell<Option<Tail>>=const{RefCell::new(None)}; }
struct GatedCleanup(Mutex<Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>>);
impl Spawner for GatedCleanup {
    fn spawn(&self, work: Box<dyn FnOnce() + Send>) -> io::Result<JoinHandle<()>> {
        let (entered, release) = self.0.lock().unwrap().take().unwrap();
        // The actual OS-thread handle covers the supplied loop in both cases.
        // The Windows TLS loader lock would prevent the loop from joining a worker.
        OsSpawner.spawn(Box::new(move || {
            #[cfg(windows)]
            {
                entered.send(()).unwrap();
                release.recv_timeout(Duration::from_secs(15)).unwrap();
                work();
            }
            #[cfg(not(windows))]
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
        let (r, handle) = recording(&root, "discard.wav");
        ui.submit_finalized(r).unwrap();
        until("discarded cleanup initial failure", || {
            ui.act(Action::Poll, &mut sink);
            ui.can_retry()
        });
        assert!(is_open(handle));
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        for _ in 0..8 {
            ui.act(Action::Retry, &mut sink);
            until("discarded cleanup retry failure", || {
                ui.act(Action::Poll, &mut sink);
                ui.can_retry()
            });
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
        assert!(ui.act(Action::Exit, &mut sink));
        assert!(!is_open(handle));
        return;
    }
    if mode == "control" {
        let mut ui = Actions::with_executor(Executor::new(Arc::new(Good(calls.clone()))));
        let (r, handle) = recording(&root, "control.wav");
        ui.submit_finalized(r).unwrap();
        until("default cleanup result", || {
            assert!(!ui.act(Action::Poll, &mut sink));
            ui.can_copy()
        });
        assert!(!is_open(handle));
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
    let cleanup = Arc::new(GatedCleanup(Mutex::new(Some((tx, gate)))));
    let mut ui = Actions::with_executor(Executor::with_spawners(
        Arc::new(Good(calls.clone())),
        Arc::new(OsSpawner),
        cleanup,
    ));
    let (r, handle) = recording(&root, "held.wav");
    ui.submit_finalized(r).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(is_open(handle));
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
    assert!(is_open(handle));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(sink.0.is_empty());
    release.send(()).unwrap();
    if mode == "poll" {
        until("held cleanup service entry", || {
            calls.load(Ordering::SeqCst) == 1
        });
        owner.as_mut().unwrap().act(Action::Cancel, &mut sink);
    }
    if let Some(a) = owner.as_mut() {
        until("held cleanup owner cancellation or exit", || {
            let closed = a.act(Action::Poll, &mut sink);
            if mode == "exit" {
                closed
            } else {
                a.status == "Cancelled; service stopped"
            }
        });
    }
    until("held recording file disposal", || !is_open(handle));
    let service_calls = calls.load(Ordering::SeqCst);
    assert_eq!(service_calls, if mode == "poll" { 1 } else { 0 });
    assert!(next.act(Action::Exit, &mut sink));
    let under_tls = !cfg!(windows);
    println!(
        "{{\"case\":\"cleanup-TLS-{mode}\",\"actual_OS_handle_covers_supplied_loop\":true,\"cleanup_loop_runs_in_owned_TLS_destructor\":{under_tls},\"destructor_entered_and_held\":{under_tls},\"cleanup_gate_entered_and_held\":true,\"worker_calls_before_release\":0,\"UI_panicked_before_release\":{panicked},\"panic_text\":\"{panic_text}\",\"File_retained_until_release\":true,\"service_calls_after_release\":{service_calls},\"recording_closed_after_release\":true}}"
    );
    assert!(!panicked);
    let mut healthy = Actions::with_executor(Executor::new(Arc::new(Good(calls.clone()))));
    healthy
        .submit_finalized(recording(&root, "healthy.wav").0)
        .unwrap();
    // File disposal precedes the orphan's final Permit drop. Controller admission
    // can therefore succeed while enqueue fails; recover through explicit Retry.
    until("healthy result after cleanup permit release", || {
        healthy.act(Action::Poll, &mut sink);
        if healthy.can_retry() {
            assert_eq!(calls.load(Ordering::SeqCst), service_calls);
            healthy.act(Action::Retry, &mut sink);
        }
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
