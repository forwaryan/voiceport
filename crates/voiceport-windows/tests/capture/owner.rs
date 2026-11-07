use super::*;
use crate::{
    capture::{Backend, Budget, Device, FRAME_SAMPLES, Format, Frame, Output, PCM, Read},
    wav::{MAX_SAMPLES, Wav},
};
use std::{
    cell::RefCell, fs::File, marker::PhantomData, path::PathBuf, rc::Rc, sync::atomic::AtomicUsize,
    time::Instant,
};
const LIMIT: Duration = Duration::from_secs(8);
type Gate = (&'static str, mpsc::SyncSender<()>, mpsc::Receiver<()>);
struct Trace {
    calls: Mutex<Vec<(&'static str, thread::ThreadId)>>,
    fail: Mutex<Option<&'static str>>,
    panic: Mutex<Option<&'static str>>,
    gate: Mutex<Option<Gate>>,
    returned: AtomicBool,
    drops: AtomicUsize,
    budget: Budget,
}
impl Trace {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(vec![]),
            fail: Mutex::new(None),
            panic: Mutex::new(None),
            gate: Mutex::new(None),
            returned: AtomicBool::new(true),
            drops: AtomicUsize::new(0),
            budget: Budget::default(),
        })
    }
    fn call(&self, name: &'static str) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((name, thread::current().id()));
        let gate = {
            let mut g = self.gate.lock().unwrap();
            if g.as_ref().is_some_and(|g| g.0 == name) {
                g.take()
            } else {
                None
            }
        };
        if let Some((_, tx, rx)) = gate {
            tx.send(()).unwrap();
            rx.recv_timeout(LIMIT).unwrap();
        }
        let panic = *self.panic.lock().unwrap() == Some(name);
        assert!(!panic, "owner synthetic panic {name}");
        let fail = *self.fail.lock().unwrap();
        if fail == Some(name) || (fail == Some("open+reset") && ["open", "reset"].contains(&name)) {
            Err(Error::Read)
        } else {
            Ok(())
        }
    }
    fn count(&self, name: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|x| x.0 == name)
            .count()
    }
    fn gate(&self, name: &'static str) -> (mpsc::Receiver<()>, mpsc::SyncSender<()>) {
        let (tx, ack) = mpsc::sync_channel(1);
        let (release, rx) = mpsc::sync_channel(1);
        *self.gate.lock().unwrap() = Some((name, tx, rx));
        (ack, release)
    }
}
struct Tail {
    t: Arc<Trace>,
    name: &'static str,
}
impl Drop for Tail {
    fn drop(&mut self) {
        self.t.call(self.name).unwrap();
    }
}
thread_local! { static TLS: RefCell<Option<Tail>> = const { RefCell::new(None) }; }
struct Guard(Arc<Trace>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.call("guard").unwrap();
        self.0.drops.fetch_add(1, Ordering::SeqCst);
    }
}
struct Driver {
    t: Arc<Trace>,
    id: u64,
    seq: u64,
    left: usize,
    packet: usize,
    _tail: Tail,
    _not_send: PhantomData<Rc<()>>,
}
impl Backend for Driver {
    fn open(&mut self, id: u64, _: Format) -> Result<Device> {
        self.t.call("open")?;
        self.id = id;
        Ok(Device {
            id: 17,
            format: PCM,
        })
    }
    fn prepare(&mut self) -> Result<()> {
        self.t.call("prepare")
    }
    fn start(&mut self) -> Result<()> {
        self.t.call("start")
    }
    fn stop(&mut self) -> Result<()> {
        self.t.call("stop")
    }
    fn reset(&mut self) -> Result<()> {
        self.t.call("reset")
    }
    fn unprepare(&mut self) -> Result<()> {
        self.t.call("unprepare")
    }
    fn close(&mut self) -> Result<()> {
        self.t.call("close")
    }
    fn read(&mut self, drain: bool, f: &mut Frame) -> Result<Read> {
        self.t.call(if drain { "drain" } else { "read" })?;
        if drain && !self.t.returned.load(Ordering::Acquire) {
            return Ok(Read::Idle);
        }
        if self.left == 0 {
            return Ok(if drain { Read::End } else { Read::Idle });
        }
        let n = self.left.min(self.packet);
        self.left -= n;
        self.seq += 1;
        f.session = self.id;
        f.device = 17;
        f.sequence = self.seq;
        f.format = PCM;
        f.bytes = n * 2;
        f.done = true;
        f.samples[..n].fill(23);
        // Synthetic packets are already available. Acknowledge actual data with
        // a wake token so bounded owner polls need not each wait for a timer.
        // Idle, End and errors provide no wake or evidence of release/join.
        thread::current().unpark();
        Ok(Read::Data)
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        self.t.call("backend-body").unwrap();
    }
}
struct Sink {
    t: Arc<Trace>,
    wav: Option<Wav<File>>,
    path: PathBuf,
    guard: Option<Arc<Guard>>,
    _tail: Tail,
}
impl Output for Sink {
    fn append(&mut self, s: &[i16]) -> Result<()> {
        self.t.call("append")?;
        self.wav
            .as_mut()
            .unwrap()
            .append(s)
            .map_err(|_| Error::Write)
    }
    fn finish(mut self, l: Option<String>) -> Result<Recording> {
        self.t.call("finish")?;
        let f = self
            .wav
            .take()
            .unwrap()
            .finish()
            .map_err(|_| Error::Finalize)?;
        Ok(Recording::caller_owned_with_guard(
            f,
            self.path.clone(),
            l,
            self.guard.take().unwrap(),
        ))
    }
}
impl Drop for Sink {
    fn drop(&mut self) {
        self.t.call("output-body").unwrap();
    }
}
struct Make {
    t: Arc<Trace>,
    n: usize,
    packet: usize,
    path: PathBuf,
    tls: bool,
}
impl Factory for Make {
    type Session = Session<Driver, Sink>;
    fn create(self) -> Result<Self::Session> {
        self.t.call("factory")?;
        if self.tls {
            TLS.with(|s| {
                *s.borrow_mut() = Some(Tail {
                    t: self.t.clone(),
                    name: "tls",
                })
            });
        }
        std::fs::create_dir_all(self.path.parent().unwrap()).unwrap();
        let output = Sink {
            t: self.t.clone(),
            wav: Some(Wav::new(File::create_new(&self.path).unwrap()).unwrap()),
            path: self.path,
            guard: Some(Arc::new(Guard(self.t.clone()))),
            _tail: Tail {
                t: self.t.clone(),
                name: "output-field",
            },
        };
        Session::with_backend(
            Driver {
                t: self.t.clone(),
                id: 0,
                seq: 0,
                left: self.n,
                packet: self.packet,
                _tail: Tail {
                    t: self.t.clone(),
                    name: "backend-field",
                },
                _not_send: PhantomData,
            },
            output,
            Some("zh".into()),
            &self.t.budget,
        )
    }
}
static FILES: AtomicUsize = AtomicUsize::new(0);
fn make(t: &Arc<Trace>, n: usize) -> Make {
    let root = std::env::var_os("VP_OWNER_FILES")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("voiceport-owner-tests-{}", std::process::id()))
        });
    Make {
        t: t.clone(),
        n,
        packet: FRAME_SAMPLES,
        path: root.join(format!(
            "owned-{}.wav",
            FILES.fetch_add(1, Ordering::SeqCst)
        )),
        tls: false,
    }
}
struct Harness(Arc<Domain>);
impl Harness {
    fn new() -> Self {
        Self(Arc::new(Domain::default()))
    }
    fn start(&self, h: &mut CaptureOwner, f: Make) -> u64 {
        h.start_in(&self.0, f, Arc::new(OsSpawner), Arc::new(OsSpawner))
            .unwrap()
    }
    fn released(&self) {
        wait(|| !self.0.busy.load(Ordering::Acquire));
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        let old = std::mem::replace(
            &mut *self.0.infrastructure.lock().unwrap(),
            Infrastructure::Lost,
        );
        if let Infrastructure::Live(r) = old {
            drop(r.sender);
            r.handle.join().unwrap();
        }
    }
}
fn wait(f: impl FnMut() -> bool) {
    wait_diagnostic(f, || "owner transition".into());
}
fn wait_diagnostic(mut f: impl FnMut() -> bool, diagnostic: impl Fn() -> String) {
    let start = Instant::now();
    while !f() {
        assert!(
            start.elapsed() < LIMIT,
            "acknowledged owner transition did not finish after {:?}: {}",
            start.elapsed(),
            diagnostic()
        );
        thread::park_timeout(Duration::from_millis(1));
    }
}
fn ready(h: &CaptureOwner) {
    wait_diagnostic(
        || h.poll() == Status::Ready,
        || {
            format!(
                "waiting=Ready generation={:?} status={:?}",
                h.generation(),
                h.poll()
            )
        },
    );
}
fn wait_case(harness: &Harness, h: &CaptureOwner, t: &Trace, n: usize, expected: Status) {
    let start = Instant::now();
    let diagnostic = || {
        let shared = h.active.as_ref().unwrap();
        let calls = t.calls.try_lock().ok();
        let count = |name| {
            calls
                .as_ref()
                .map(|calls| calls.iter().filter(|call| call.0 == name).count())
        };
        format!(
            "n={n} waiting={expected:?} generation={} status={:?} busy={} release={} claimed={} invalid={} commands={} drain={:?} append={:?} close={:?} finish={:?} drops={} tail={:?}",
            shared.generation,
            h.poll(),
            harness.0.busy.load(Ordering::Acquire),
            shared.release.load(Ordering::Acquire),
            shared.claimed.load(Ordering::Acquire),
            shared.invalid.load(Ordering::Acquire),
            shared.commands.load(Ordering::Acquire),
            count("drain"),
            count("append"),
            count("close"),
            count("finish"),
            t.drops.load(Ordering::SeqCst),
            calls.as_ref().map(|calls| calls
                .iter()
                .rev()
                .take(6)
                .map(|call| call.0)
                .collect::<Vec<_>>()),
        )
    };
    eprintln!("owner case begin: {}", diagnostic());
    wait_diagnostic(
        || {
            h.poll() == expected
                && (expected != Status::Stopped || !harness.0.busy.load(Ordering::Acquire))
        },
        diagnostic,
    );
    eprintln!(
        "owner case complete after {:?}: {}",
        start.elapsed(),
        diagnostic()
    );
}
fn take(h: &mut CaptureOwner, id: u64) -> Result<Recording> {
    ready(h);
    h.take_result(id).unwrap()
}
fn assert_affinity(t: &Trace) {
    let ui = thread::current().id();
    let calls = t.calls.lock().unwrap();
    let owner = calls[0].1;
    assert_ne!(owner, ui);
    assert!(
        calls
            .iter()
            .filter(|x| x.0 != "guard")
            .all(|x| x.1 == owner)
    );
}

#[test]
fn inert_construction_and_factory_error_are_bounded() {
    let harness = Harness::new();
    let mut h = CaptureOwner::default();
    let t = Trace::new();
    let f = make(&t, 1);
    assert_eq!(h.poll(), Status::Idle);
    assert!(matches!(
        *harness.0.infrastructure.lock().unwrap(),
        Infrastructure::Cold
    ));
    assert!(t.calls.lock().unwrap().is_empty());
    assert!(!f.path.exists());
    *t.fail.lock().unwrap() = Some("factory");
    let id = harness.start(&mut h, f);
    assert!(matches!(take(&mut h, id), Err(Error::Read)));
    assert!(h.take_result(id).is_none());
    harness.released();
    assert_eq!(t.count("open"), 0);
    assert_affinity(&t);
}
#[test]
fn exact_success_stop_empty_error_max_and_single_generation_result() {
    let harness = Harness::new();
    let mut h = CaptureOwner::default();
    for n in [0, 1, FRAME_SAMPLES * 5 + 1, MAX_SAMPLES, MAX_SAMPLES + 1] {
        let t = Trace::new();
        let id = harness.start(&mut h, make(&t, n));
        h.stop();
        wait_case(&harness, &h, &t, n, Status::Ready);
        assert!(h.take_result(id + 1).is_none());
        let answer = h.take_result(id).unwrap();
        assert!(h.take_result(id).is_none());
        if n == 0 {
            assert!(matches!(answer, Err(Error::Empty)))
        } else if n > MAX_SAMPLES {
            assert!(matches!(answer, Err(Error::Overflow)))
        } else {
            let r = answer.unwrap();
            assert_eq!(r.language(), Some("zh"));
            assert_eq!(
                std::fs::metadata(r.path()).unwrap().len(),
                44 + n as u64 * 2
            );
            assert_eq!(t.drops.load(Ordering::SeqCst), 0);
            drop(r);
        }
        wait_case(&harness, &h, &t, n, Status::Stopped);
        assert_eq!(t.drops.load(Ordering::SeqCst), 1);
        assert_affinity(&t);
    }
}
#[test]
fn held_startup_running_io_drain_finish_fields_and_tls_do_not_block_controls() {
    for at in [
        "factory",
        "open",
        "prepare",
        "start",
        "read",
        "append",
        "stop",
        "reset",
        "drain",
        "unprepare",
        "close",
        "finish",
        "backend-field",
        "output-field",
        "tls",
    ] {
        let harness = Harness::new();
        let mut h = CaptureOwner::default();
        let t = Trace::new();
        let (ack, release) = t.gate(at);
        let mut f = make(&t, FRAME_SAMPLES * 5 + 1);
        f.tls = true;
        let id = harness.start(&mut h, f);
        if !["factory", "open", "prepare", "start", "read", "append"].contains(&at) {
            h.stop();
        }
        ack.recv_timeout(LIMIT).unwrap();
        for _ in 0..16 {
            h.stop();
            let _ = h.poll();
            assert!(h.take_result(id).is_none());
        }
        h.cancel();
        assert!(h.take_result(id).is_none());
        assert!(harness.0.busy.load(Ordering::Acquire));
        let mut other = CaptureOwner::default();
        assert_eq!(
            other.start_in(
                &harness.0,
                make(&t, 1),
                Arc::new(OsSpawner),
                Arc::new(OsSpawner)
            ),
            Err(Error::Busy)
        );
        release.send(()).unwrap();
        harness.released();
        assert_eq!(h.poll(), Status::Stopped);
        assert!(h.take_result(id).is_none());
        assert_eq!(t.drops.load(Ordering::SeqCst), 1);
        assert_affinity(&t);
        let next = Trace::new();
        let nid = harness.start(&mut other, make(&next, 1));
        other.stop();
        drop(take(&mut other, nid));
        harness.released();
    }
}
#[test]
fn drop_exit_and_late_publication_dispose_actual_guards_off_caller() {
    for action in ["drop", "exit", "cancel"] {
        for at in ["factory", "read", "drain", "finish", "tls", "ready"] {
            let harness = Harness::new();
            let mut h = CaptureOwner::default();
            let t = Trace::new();
            let mut f = make(&t, 1);
            f.tls = true;
            let gate = if at != "ready" {
                Some(t.gate(at))
            } else {
                None
            };
            let id = harness.start(&mut h, f);
            if ["drain", "finish", "tls", "ready"].contains(&at) {
                h.stop();
            }
            if let Some((ack, _)) = &gate {
                ack.recv_timeout(LIMIT).unwrap();
            } else {
                ready(&h);
            }
            if action == "drop" {
                drop(h);
            } else if action == "exit" {
                h.exit();
                assert_eq!(
                    h.start_in(
                        &harness.0,
                        make(&t, 1),
                        Arc::new(OsSpawner),
                        Arc::new(OsSpawner)
                    ),
                    Err(Error::Closed)
                );
                assert!(h.take_result(id).is_none());
            } else {
                h.cancel();
                assert!(h.take_result(id).is_none());
            }
            if let Some((_, release)) = gate {
                release.send(()).unwrap();
            }
            harness.released();
            assert_eq!(t.drops.load(Ordering::SeqCst), 1);
            let ui = thread::current().id();
            assert!(t.calls.lock().unwrap().iter().all(|x| x.1 != ui));
        }
    }
}
#[test]
fn unclaimed_recording_disposal_is_covered_before_replacement() {
    let harness = Harness::new();
    let mut h = CaptureOwner::default();
    let t = Trace::new();
    let id = harness.start(&mut h, make(&t, 1));
    h.stop();
    ready(&h);
    let (ack, release) = t.gate("guard");
    h.cancel();
    ack.recv_timeout(LIMIT).unwrap();
    assert!(harness.0.busy.load(Ordering::Acquire));
    assert!(h.take_result(id).is_none());
    assert_eq!(t.drops.load(Ordering::SeqCst), 0);
    drop(h);
    release.send(()).unwrap();
    harness.released();
    assert_eq!(t.drops.load(Ordering::SeqCst), 1);
    assert_ne!(
        t.calls
            .lock()
            .unwrap()
            .iter()
            .find(|x| x.0 == "guard")
            .unwrap()
            .1,
        thread::current().id()
    );
}
#[test]
fn release_errors_park_until_new_explicit_recovery_and_preserve_first_error() {
    for at in ["reset", "unprepare", "close"] {
        let harness = Harness::new();
        let mut h = CaptureOwner::default();
        let t = Trace::new();
        *t.fail.lock().unwrap() = Some(at);
        let id = harness.start(&mut h, make(&t, 1));
        h.stop();
        wait(|| h.poll() == Status::RecoveryRequired);
        let calls = t.count(at);
        assert_eq!(calls, 1);
        *t.fail.lock().unwrap() = None;
        for _ in 0..16 {
            h.stop();
            h.cancel();
            assert!(h.take_result(id).is_none());
            assert_eq!(h.poll(), Status::RecoveryRequired);
        }
        // A real acknowledged unrelated channel round-trip ensures background
        // scheduling has run; snapshot/Drop must still not retry this stage.
        let (tx, rx) = mpsc::sync_channel(1);
        thread::spawn(move || tx.send(()).unwrap()).join().unwrap();
        rx.recv().unwrap();
        assert_eq!(t.count(at), calls);
        h.recover().unwrap();
        assert!(h.recover().is_err());
        harness.released();
        assert_eq!(t.count(at), 2);
        assert_eq!(t.count("finish"), 0);
        assert_eq!(t.drops.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn hidden_begin_release_failure_and_query_preserve_ordinary_progress() {
    let harness = Harness::new();
    let mut h = CaptureOwner::default();
    let t = Trace::new();
    *t.fail.lock().unwrap() = Some("open");
    // Arrange reset failure after open has actually entered, rather than infer
    // the cleanup stage from begin's returned first Open error.
    let (ack, release) = t.gate("reset");
    let id = harness.start(&mut h, make(&t, 1));
    ack.recv_timeout(LIMIT).unwrap();
    *t.fail.lock().unwrap() = Some("reset");
    release.send(()).unwrap();
    wait(|| h.poll() == Status::RecoveryRequired);
    assert_eq!(t.count("reset"), 1);
    *t.fail.lock().unwrap() = None;
    h.recover().unwrap();
    assert!(matches!(take(&mut h, id), Err(Error::Open)));
    harness.released();
    assert_eq!(t.count("finish"), 0);
    let t = Trace::new();
    t.returned.store(false, Ordering::Release);
    let id = harness.start(&mut h, make(&t, 1));
    h.stop();
    wait(|| t.count("drain") >= 2);
    assert_eq!(h.poll(), Status::Stopping);
    assert!(h.recover().is_err());
    t.returned.store(true, Ordering::Release);
    drop(take(&mut h, id));
    harness.released();
}
#[test]
fn recovered_stage_can_park_again_without_consuming_old_authorization() {
    let harness = Harness::new();
    let mut h = CaptureOwner::default();
    let t = Trace::new();
    *t.fail.lock().unwrap() = Some("reset");
    let id = harness.start(&mut h, make(&t, 1));
    h.stop();
    wait(|| h.poll() == Status::RecoveryRequired);
    let (ack, release) = t.gate("reset");
    *t.fail.lock().unwrap() = None;
    h.recover().unwrap();
    ack.recv_timeout(LIMIT).unwrap();
    assert!(h.recover().is_err());
    *t.fail.lock().unwrap() = Some("close");
    release.send(()).unwrap();
    wait(|| h.poll() == Status::RecoveryRequired);
    assert_eq!(t.count("close"), 1);
    *t.fail.lock().unwrap() = None;
    assert!(h.take_result(id).is_none());
    h.recover().unwrap();
    assert!(matches!(take(&mut h, id), Err(Error::Reset)));
    harness.released();
    assert_eq!(t.count("close"), 2);
    assert_eq!(t.count("finish"), 0);
}
#[test]
fn permanent_method_finish_field_and_factory_panics_retain_admission() {
    for at in [
        "factory",
        "open",
        "prepare",
        "start",
        "read",
        "append",
        "stop",
        "reset",
        "drain",
        "unprepare",
        "close",
        "finish",
        "backend-field",
        "output-field",
    ] {
        let harness = Harness::new();
        let mut h = CaptureOwner::default();
        let t = Trace::new();
        *t.panic.lock().unwrap() = Some(at);
        let id = harness.start(&mut h, make(&t, 1));
        if !["factory", "open", "prepare", "start", "read", "append"].contains(&at) {
            h.stop();
        }
        wait(|| h.poll() == Status::Interrupted);
        *t.panic.lock().unwrap() = None;
        assert!(harness.0.busy.load(Ordering::Acquire));
        h.cancel();
        h.stop();
        assert!(h.recover().is_err());
        assert!(h.take_result(id).is_none());
        let before = t.calls.lock().unwrap().len();
        drop(h);
        assert_eq!(t.calls.lock().unwrap().len(), before);
        let mut other = CaptureOwner::default();
        assert_eq!(
            other.start_in(
                &harness.0,
                make(&t, 1),
                Arc::new(OsSpawner),
                Arc::new(OsSpawner)
            ),
            Err(Error::Busy)
        );
    }
}
struct Fail;
impl Spawner for Fail {
    fn spawn(&self, _: Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>> {
        Err(std::io::Error::other("owned no-start failure"))
    }
}
struct Lose;
impl Spawner for Lose {
    fn spawn(&self, work: Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>> {
        thread::Builder::new().spawn(move || drop(work))
    }
}
struct PanicSpawn;
impl Spawner for PanicSpawn {
    fn spawn(&self, _: Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>> {
        panic!("owned unknown spawn")
    }
}
struct PanicAfter;
impl Spawner for PanicAfter {
    fn spawn(&self, work: Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>> {
        thread::Builder::new().spawn(move || {
            work();
            panic!("owned uncaught worker panic")
        })
    }
}
#[test]
fn spawn_initialization_ack_loss_and_uncaught_panic_are_honest() {
    let harness = Harness::new();
    let mut h = CaptureOwner::default();
    let t = Trace::new();
    assert_eq!(
        h.start_in(&harness.0, make(&t, 1), Arc::new(Fail), Arc::new(OsSpawner)),
        Err(Error::Start)
    );
    assert!(!harness.0.busy.load(Ordering::Acquire));
    assert!(t.calls.lock().unwrap().is_empty());
    assert_eq!(
        h.start_in(
            &harness.0,
            make(&t, 1),
            Arc::new(OsSpawner),
            Arc::new(OsSpawner)
        ),
        Err(Error::Start)
    );
    harness.0.recover_infrastructure().unwrap();
    let id = h
        .start_in(&harness.0, make(&t, 1), Arc::new(OsSpawner), Arc::new(Fail))
        .unwrap();
    assert!(matches!(take(&mut h, id), Err(Error::Start)));
    harness.released();
    assert!(t.calls.lock().unwrap().is_empty());
    for mode in 0..4 {
        let harness = Harness::new();
        let mut h = CaptureOwner::default();
        let t = Trace::new();
        let (sup, worker): (Arc<dyn Spawner>, Arc<dyn Spawner>) = match mode {
            0 => (Arc::new(Lose), Arc::new(OsSpawner)),
            1 => (Arc::new(OsSpawner), Arc::new(Lose)),
            2 => (Arc::new(OsSpawner), Arc::new(PanicSpawn)),
            _ => (Arc::new(OsSpawner), Arc::new(PanicAfter)),
        };
        let id = h.start_in(&harness.0, make(&t, 1), sup, worker).unwrap();
        h.stop();
        wait(|| matches!(h.poll(), Status::Unavailable | Status::Interrupted));
        assert!(h.take_result(id).is_none());
        assert!(harness.0.busy.load(Ordering::Acquire));
        assert_eq!(harness.0.recover_infrastructure(), Err(Error::Busy));
    }
}

#[test]
fn read_only_cleanup_query_never_drives_or_clears_hidden_uncertainty() {
    thread::spawn(|| {
        let t = Trace::new();
        *t.fail.lock().unwrap() = Some("open+reset");
        let mut s = make(&t, FRAME_SAMPLES * 5).create().unwrap();
        assert_eq!(s.cleanup_status(), CleanupStatus::Progress);
        assert_eq!(s.begin(), Err(Error::Open));
        assert_eq!(s.error(), Some(Error::Open));
        assert_eq!(s.cleanup_status(), CleanupStatus::RecoveryRequired);
        let count = t.calls.lock().unwrap().len();
        for _ in 0..3 {
            assert_eq!(s.cleanup_status(), CleanupStatus::RecoveryRequired)
        }
        assert_eq!(t.calls.lock().unwrap().len(), count);
        *t.fail.lock().unwrap() = None;
        let _ = s.poll();
        assert_eq!(s.cleanup_status(), CleanupStatus::Progress);
        assert_eq!(s.error(), Some(Error::Open));
        let _ = s.poll();
        assert_eq!(s.cleanup_status(), CleanupStatus::Released);
        assert!(s.take_recording().is_none());
        drop(s);
        let t = Trace::new();
        let mut s = make(&t, 1).create().unwrap();
        s.begin().unwrap();
        *t.panic.lock().unwrap() = Some("reset");
        assert!(catch_unwind(AssertUnwindSafe(|| s.cancel())).is_err());
        let count = t.calls.lock().unwrap().len();
        assert_eq!(s.error(), None);
        assert_eq!(s.cleanup_status(), CleanupStatus::Interrupted);
        assert_eq!(s.error(), None);
        *t.panic.lock().unwrap() = None;
        assert_eq!(s.poll(), Err(Error::Cleanup));
        assert_eq!(s.cleanup_status(), CleanupStatus::Interrupted);
        assert_eq!(t.calls.lock().unwrap().len(), count);
        drop(s);
        assert!(t.budget.occupied());
    })
    .join()
    .unwrap();
}
#[test]
fn drop_while_parked_never_retries_unknown_release() {
    let harness = Harness::new();
    let mut h = CaptureOwner::default();
    let t = Trace::new();
    *t.fail.lock().unwrap() = Some("reset");
    harness.start(&mut h, make(&t, 1));
    h.stop();
    wait(|| h.poll() == Status::RecoveryRequired);
    let retained = h.active.as_ref().unwrap().clone();
    let before = retained.recovery_parks.load(Ordering::Acquire);
    *t.fail.lock().unwrap() = None;
    drop(h);
    wait(|| retained.recovery_parks.load(Ordering::Acquire) >= before + 2);
    assert_eq!(t.count("reset"), 1);
    assert!(harness.0.busy.load(Ordering::Acquire));
    assert_eq!(t.drops.load(Ordering::SeqCst), 0);
    // Test-only retained-control restoration supplies a NEW explicit command so
    // the OS thread is lawfully released and joined at test teardown. Production
    // Drop exposes no replacement/rebind or automatic unknown-release recovery.
    let mut recovery = CaptureOwner {
        active: Some(retained),
        closed: false,
    };
    recovery.recover().unwrap();
    harness.released();
    assert_eq!(t.count("reset"), 2);
    assert_eq!(t.drops.load(Ordering::SeqCst), 1);
}
struct GatedSpawn {
    entered: mpsc::SyncSender<()>,
    release: Mutex<Option<mpsc::Receiver<()>>>,
}
impl Spawner for GatedSpawn {
    fn spawn(&self, work: Box<dyn FnOnce() + Send>) -> std::io::Result<JoinHandle<()>> {
        let tx = self.entered.clone();
        let rx = self.release.lock().unwrap().take().unwrap();
        thread::Builder::new().spawn(move || {
            tx.send(()).unwrap();
            rx.recv_timeout(LIMIT).unwrap();
            work();
        })
    }
}
#[test]
fn cancel_before_factory_entry_skips_creation_and_owner_tls_join_still_matters() {
    let harness = Harness::new();
    let mut h = CaptureOwner::default();
    let t = Trace::new();
    let (tx, ack) = mpsc::sync_channel(1);
    let (release, rx) = mpsc::sync_channel(1);
    let id = h
        .start_in(
            &harness.0,
            make(&t, 1),
            Arc::new(OsSpawner),
            Arc::new(GatedSpawn {
                entered: tx,
                release: Mutex::new(Some(rx)),
            }),
        )
        .unwrap();
    ack.recv_timeout(LIMIT).unwrap();
    h.cancel();
    assert!(h.take_result(id).is_none());
    assert!(t.calls.lock().unwrap().is_empty());
    release.send(()).unwrap();
    harness.released();
    assert!(t.calls.lock().unwrap().is_empty());
}
#[test]
fn proven_no_start_and_unknown_supervisor_spawn_remain_distinct() {
    let harness = Harness::new();
    let mut h = CaptureOwner::default();
    let t = Trace::new();
    assert_eq!(
        h.start_in(
            &harness.0,
            make(&t, 1),
            Arc::new(PanicSpawn),
            Arc::new(OsSpawner)
        ),
        Err(Error::Cleanup)
    );
    assert!(harness.0.busy.load(Ordering::Acquire));
    assert_eq!(harness.0.recover_infrastructure(), Err(Error::Busy));
    assert!(t.calls.lock().unwrap().is_empty());
}

#[test]
fn normal_first_errors_and_io_failure_never_publish_failed_prefix() {
    for at in [
        "open", "prepare", "start", "read", "append", "stop", "finish",
    ] {
        let harness = Harness::new();
        let mut h = CaptureOwner::default();
        let t = Trace::new();
        *t.fail.lock().unwrap() = Some(at);
        let id = harness.start(&mut h, make(&t, 1));
        if ["stop", "finish"].contains(&at) {
            h.stop();
        }
        let answer = take(&mut h, id);
        let expected = match at {
            "open" => Error::Open,
            "prepare" => Error::Prepare,
            "start" => Error::Start,
            "append" => Error::Write,
            "stop" => Error::Stop,
            "finish" => Error::Finalize,
            _ => Error::Read,
        };
        assert!(matches!(answer,Err(e) if e==expected));
        harness.released();
        assert_eq!(t.drops.load(Ordering::SeqCst), 1);
        assert_eq!(t.count("finish"), usize::from(at == "finish"));
        assert_affinity(&t);
    }
}

#[test]
fn infrastructure_and_result_lock_contention_never_wait_on_caller() {
    // Hold process initialization from an acknowledged OS thread. Start must
    // refuse immediately without blocking behind OnceLock initialization.
    let (tx, ack) = mpsc::sync_channel(1);
    let (release, rx) = mpsc::sync_channel(1);
    let gate = thread::spawn(move || {
        let _lock = DOMAIN.lock().unwrap();
        tx.send(()).unwrap();
        rx.recv_timeout(LIMIT).unwrap();
    });
    ack.recv_timeout(LIMIT).unwrap();
    let mut h = CaptureOwner::default();
    let t = Trace::new();
    assert_eq!(h.start(make(&t, 1)), Err(Error::Busy));
    assert_eq!(CaptureOwner::recover_infrastructure(), Err(Error::Busy));
    release.send(()).unwrap();
    gate.join().unwrap();
    assert!(t.calls.lock().unwrap().is_empty());
    let harness = Harness::new();
    let id = harness.start(&mut h, make(&t, 1));
    h.stop();
    ready(&h);
    let s = h.active.as_ref().unwrap().clone();
    let (tx, ack) = mpsc::sync_channel(1);
    let (release, rx) = mpsc::sync_channel(1);
    let gate = thread::spawn(move || {
        let _lock = s.answer.lock().unwrap();
        tx.send(()).unwrap();
        rx.recv_timeout(LIMIT).unwrap();
    });
    ack.recv_timeout(LIMIT).unwrap();
    assert!(h.take_result(id).is_none());
    h.cancel();
    assert!(h.take_result(id).is_none());
    release.send(()).unwrap();
    gate.join().unwrap();
    harness.released();
    assert_eq!(t.drops.load(Ordering::SeqCst), 1);
}
#[test]
fn unclaimed_guard_disposal_panic_retains_process_capacity() {
    let harness = Harness::new();
    let mut h = CaptureOwner::default();
    let t = Trace::new();
    let id = harness.start(&mut h, make(&t, 1));
    h.stop();
    ready(&h);
    *t.panic.lock().unwrap() = Some("guard");
    h.cancel();
    wait(|| h.poll() == Status::Interrupted);
    assert!(harness.0.busy.load(Ordering::Acquire));
    assert!(h.take_result(id).is_none());
    assert!(h.recover().is_err());
}
