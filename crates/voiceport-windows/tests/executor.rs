use std::{
    io,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use voiceport_windows::{
    Job, Recording, Refused,
    actions::{Action, Actions, ManualClipboard},
    executor::{
        CoreService, Executor, OsSpawner, RuntimeFactory, Service, ServiceFailure, Spawner,
    },
};
struct Sink(Vec<String>);
impl ManualClipboard for Sink {
    fn copy(&mut self, s: &str) -> Result<(), String> {
        self.0.push(s.into());
        Ok(())
    }
}
struct Held {
    entered: mpsc::SyncSender<(std::path::PathBuf, Option<String>)>,
    release: Mutex<mpsc::Receiver<Result<String, ()>>>,
}
impl Service for Held {
    fn run(&self, j: &Job) -> Result<String, ServiceFailure> {
        self.entered
            .send((j.path().into(), j.language().map(str::to_owned)))
            .unwrap();
        self.release
            .lock()
            .unwrap()
            .recv()
            .unwrap()
            .map_err(|_| ServiceFailure)
    }
}
fn rec(d: &std::path::Path, n: &str) -> Recording {
    let path = d.join(n);
    Recording::caller_owned(
        std::fs::File::create_new(&path).unwrap(),
        path,
        Some("zh".into()),
    )
}
fn pump(ui: &mut Actions, s: &mut Sink, done: impl Fn(&Actions) -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !done(ui) {
        assert!(Instant::now() < end, "worker never completed");
        ui.act(Action::Poll, s);
        thread::yield_now();
    }
}
struct FailSpawn;
impl Spawner for FailSpawn {
    fn spawn(&self, _: Box<dyn FnOnce() + Send>) -> io::Result<thread::JoinHandle<()>> {
        Err(io::Error::other("synthetic spawn"))
    }
}
struct Silent;
impl Spawner for Silent {
    fn spawn(&self, _: Box<dyn FnOnce() + Send>) -> io::Result<thread::JoinHandle<()>> {
        thread::Builder::new().spawn(|| {})
    }
}
struct Answer;
impl Service for Answer {
    fn run(&self, _: &Job) -> Result<String, ServiceFailure> {
        Ok("real result".into())
    }
}
struct Panic;
impl Service for Panic {
    fn run(&self, _: &Job) -> Result<String, ServiceFailure> {
        panic!("synthetic private panic")
    }
}
struct RuntimeFail;
impl RuntimeFactory for RuntimeFail {
    fn build(&self) -> io::Result<tokio::runtime::Runtime> {
        Err(io::Error::other("synthetic runtime"))
    }
}
// Single test intentionally serializes scenarios against the production
// process-wide worker permit. Channels hold real threads past cancellation.
#[test]
fn production_executor_lifecycle_matrix() {
    let d = tempfile::tempdir().unwrap();
    let mut sink = Sink(vec![]);
    // Cleanup initialization failure must not start a worker or retain permit.
    let mut failed_cleanup = Actions::with_executor(Executor::with_spawners(
        Arc::new(Answer),
        Arc::new(OsSpawner),
        Arc::new(FailSpawn),
    ));
    failed_cleanup
        .submit_finalized(rec(d.path(), "cleanup-init.wav"))
        .unwrap();
    assert!(failed_cleanup.can_retry());
    failed_cleanup.act(Action::Retry, &mut sink);
    assert!(failed_cleanup.can_retry());
    assert!(failed_cleanup.act(Action::Exit, &mut sink));
    drop(failed_cleanup);
    for cancel in [false, true] {
        let (entered, rx) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        let service = Arc::new(Held {
            entered,
            release: Mutex::new(wait),
        });
        let mut ui = Actions::with_executor(Executor::new(service));
        let name = if cancel { "cancel.wav" } else { "retry.wav" };
        ui.submit_finalized(rec(d.path(), name)).unwrap();
        let captured = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(captured.1.as_deref(), Some("zh"));
        for n in 0..16 {
            let start = Instant::now();
            assert!(!ui.act(Action::Poll, &mut sink));
            assert!(start.elapsed() < Duration::from_secs(1));
            assert!(matches!(
                ui.submit_finalized(rec(d.path(), &format!("busy-{cancel}-{n}.wav"))),
                Err(Refused::Busy)
            ));
        }
        if cancel {
            ui.act(Action::Cancel, &mut sink);
            for _ in 0..8 {
                assert!(!ui.act(Action::Poll, &mut sink));
            }
            release.send(Ok("late".into())).unwrap();
            pump(&mut ui, &mut sink, |u| {
                u.status == "Cancelled; service stopped"
            });
            assert!(!ui.can_copy());
            assert!(sink.0.is_empty());
        } else {
            release.send(Err(())).unwrap();
            pump(&mut ui, &mut sink, |u| u.can_retry());
            ui.act(Action::Retry, &mut sink);
            assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), captured);
            release.send(Ok("manual\nraw".into())).unwrap();
            pump(&mut ui, &mut sink, |u| u.can_copy());
            ui.act(Action::Copy, &mut sink);
            assert_eq!(sink.0, ["manual\nraw"]);
            let status = ui.status.clone();
            for _ in 0..8 {
                ui.act(Action::Poll, &mut sink);
            }
            assert_eq!(ui.status, status);
            sink.0.clear();
        }
        assert!(ui.act(Action::Exit, &mut sink));
    }
    // Completion waits in the real executor until the UI polls: cancellation or
    // exit wins before draining, even with Copy in that same dispatch batch.
    for control in [3, 6] {
        let (entered, rx) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        let mut ui = Actions::with_executor(Executor::new(Arc::new(Held {
            entered,
            release: Mutex::new(wait),
        })));
        ui.submit_finalized(rec(d.path(), &format!("queued-{control}.wav")))
            .unwrap();
        rx.recv_timeout(Duration::from_secs(5)).unwrap();
        release.send(Ok("queued".into())).unwrap();
        thread::sleep(Duration::from_millis(20));
        ui.dispatch((1 << control) | (1 << 4) | (1 << 7), &mut sink);
        assert!(sink.0.is_empty());
        assert!(!ui.can_copy());
        let end = Instant::now() + Duration::from_secs(5);
        while !ui.act(Action::Exit, &mut sink) {
            assert!(Instant::now() < end);
            thread::yield_now();
        }
    }
    for (n, executor) in [
        Executor::with_spawner(Arc::new(Answer), Arc::new(FailSpawn)),
        Executor::with_spawner(Arc::new(Answer), Arc::new(Silent)),
        Executor::with_spawner(Arc::new(Answer), Arc::new(TailPanic)),
        Executor::new(Arc::new(Panic)),
        Executor::new(Arc::new(CoreService {
            runtime: Arc::new(RuntimeFail),
            auth: Some(d.path().join("absent-auth")),
        })),
    ]
    .into_iter()
    .enumerate()
    {
        let mut ui = Actions::with_executor(executor);
        ui.submit_finalized(rec(d.path(), &format!("failure-{n}.wav")))
            .unwrap();
        pump(&mut ui, &mut sink, |u| u.can_retry());
        assert!(!ui.status.contains("synthetic"));
        assert!(ui.act(Action::Exit, &mut sink));
    }
    // Deterministic before-start gate: cancelled work never calls service.
    let (release, wait) = mpsc::sync_channel(1);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut ui = Actions::with_executor(Executor::with_spawner(
        Arc::new(Count(calls.clone())),
        Arc::new(Before(Mutex::new(Some(wait)))),
    ));
    ui.submit_finalized(rec(d.path(), "before-start.wav"))
        .unwrap();
    ui.act(Action::Cancel, &mut sink);
    release.send(()).unwrap();
    pump(&mut ui, &mut sink, |u| {
        u.status == "Cancelled; service stopped"
    });
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 0);
    assert!(ui.act(Action::Exit, &mut sink));
    // The completion is definitely populated while worker teardown is held.
    let (done, ready) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    let mut ui = Actions::with_executor(Executor::with_spawner(
        Arc::new(Answer),
        Arc::new(Tail(Mutex::new(Some((done, wait))))),
    ));
    ui.submit_finalized(rec(d.path(), "queued-tail.wav"))
        .unwrap();
    ready.recv_timeout(Duration::from_secs(5)).unwrap();
    for _ in 0..8 {
        assert!(!ui.act(Action::Poll, &mut sink));
        assert!(!ui.can_copy());
    }
    assert!(!ui.dispatch((1 << 6) | (1 << 4) | (1 << 7), &mut sink));
    assert!(sink.0.is_empty());
    release.send(()).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while !ui.act(Action::Poll, &mut sink) {
        assert!(Instant::now() < end);
        thread::yield_now();
    }
    // Abnormal owner drop does not join an uncooperative worker. Its process
    // owner holds the permit/ticket until actual finish, then explicit retry
    // can reclaim it; no second worker may start meanwhile.
    let (entered, rx) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    let mut old = Actions::with_executor(Executor::new(Arc::new(Held {
        entered,
        release: Mutex::new(wait),
    })));
    old.submit_finalized(rec(d.path(), "orphan.wav")).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let t = Instant::now();
    drop(old);
    assert!(t.elapsed() < Duration::from_secs(1));
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut ui = Actions::with_executor(Executor::new(Arc::new(Count(count.clone()))));
    ui.submit_finalized(rec(d.path(), "after-orphan.wav"))
        .unwrap();
    assert!(ui.can_retry());
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 0);
    release.send(Ok("ignored orphan".into())).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while !ui.can_copy() {
        assert!(Instant::now() < end);
        if ui.can_retry() {
            ui.act(Action::Retry, &mut sink);
        }
        ui.act(Action::Poll, &mut sink);
        thread::yield_now();
    }
    assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert!(ui.act(Action::Exit, &mut sink));
    for n in 0..16 {
        let mut ui = Actions::with_executor(Executor::new(Arc::new(Answer)));
        ui.submit_finalized(rec(d.path(), &format!("repeat-{n}.wav")))
            .unwrap();
        pump(&mut ui, &mut sink, |u| u.can_copy());
        assert!(ui.act(Action::Exit, &mut sink));
    }
    let mut ui = Actions::with_executor(Executor::new(Arc::new(Huge)));
    ui.submit_finalized(rec(d.path(), "oversize.wav")).unwrap();
    pump(&mut ui, &mut sink, |u| u.can_retry());
    assert!(!ui.can_copy());
    assert!(ui.act(Action::Exit, &mut sink));
    // Default production core backend uses actual runtime/client with owned
    // missing auth: fails offline and manual retry goes through same worker.
    let mut ui = Actions::with_executor(Executor::new(Arc::new(CoreService {
        runtime: Arc::new(WorkingRuntime),
        auth: Some(d.path().join("no-auth")),
    })));
    ui.submit_finalized(rec(d.path(), "core.wav")).unwrap();
    pump(&mut ui, &mut sink, |u| u.can_retry());
    ui.act(Action::Retry, &mut sink);
    pump(&mut ui, &mut sink, |u| u.can_retry());
    assert!(ui.act(Action::Exit, &mut sink));
}
struct WorkingRuntime;
impl RuntimeFactory for WorkingRuntime {
    fn build(&self) -> io::Result<tokio::runtime::Runtime> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
    }
}

struct Count(Arc<std::sync::atomic::AtomicUsize>);
impl Service for Count {
    fn run(&self, _: &Job) -> Result<String, ServiceFailure> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok("counted".into())
    }
}
struct Huge;
impl Service for Huge {
    fn run(&self, _: &Job) -> Result<String, ServiceFailure> {
        Ok("x".repeat(1_000_001))
    }
}
struct Before(Mutex<Option<mpsc::Receiver<()>>>);
impl Spawner for Before {
    fn spawn(&self, work: Box<dyn FnOnce() + Send>) -> io::Result<thread::JoinHandle<()>> {
        let wait = self.0.lock().unwrap().take().unwrap();
        thread::Builder::new().spawn(move || {
            wait.recv().unwrap();
            work();
        })
    }
}
type TailChannels = (mpsc::SyncSender<()>, mpsc::Receiver<()>);
struct Tail(Mutex<Option<TailChannels>>);
impl Spawner for Tail {
    fn spawn(&self, work: Box<dyn FnOnce() + Send>) -> io::Result<thread::JoinHandle<()>> {
        let (done, wait) = self.0.lock().unwrap().take().unwrap();
        thread::Builder::new().spawn(move || {
            work();
            done.send(()).unwrap();
            wait.recv().unwrap();
        })
    }
}

struct TailPanic;
impl Spawner for TailPanic {
    fn spawn(&self, work: Box<dyn FnOnce() + Send>) -> io::Result<thread::JoinHandle<()>> {
        thread::Builder::new().spawn(move || {
            work();
            panic!("synthetic worker tail panic");
        })
    }
}
