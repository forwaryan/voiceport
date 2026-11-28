use std::{
    fs::File,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
        mpsc::{Receiver, SyncSender, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};
use voiceport_windows::{
    Job, Recording,
    actions::{Action, Actions, ManualClipboard},
    capture::{
        CleanupStatus, Error, Phase, Result,
        owner::{CaptureSession, Factory},
    },
    executor::{Executor, Service, ServiceFailure},
    wav::Wav,
};

const LIMIT: Duration = Duration::from_secs(5);

struct Gate(SyncSender<()>, Receiver<()>);
impl Gate {
    fn new() -> (Self, Receiver<()>, SyncSender<()>) {
        let (entered, observe) = sync_channel(1);
        let (release, wait) = sync_channel(1);
        (Self(entered, wait), observe, release)
    }
    fn hold(self) {
        self.0.send(()).unwrap();
        self.1.recv_timeout(LIMIT).unwrap();
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Normal,
    CannotOpen,
    ReleaseError,
}

struct Fixture {
    root: PathBuf,
    mode: Mode,
    start_gate: Mutex<Option<Gate>>,
    release_gate: Mutex<Option<Gate>>,
    starts: AtomicUsize,
    stops: AtomicUsize,
    recoveries: AtomicUsize,
    guards_dropped: Arc<AtomicUsize>,
}
impl Fixture {
    fn new(root: PathBuf, mode: Mode) -> Arc<Self> {
        Arc::new(Self {
            root,
            mode,
            start_gate: Mutex::default(),
            release_gate: Mutex::default(),
            starts: AtomicUsize::new(0),
            stops: AtomicUsize::new(0),
            recoveries: AtomicUsize::new(0),
            guards_dropped: Arc::default(),
        })
    }
}
struct Synthetic(Arc<Fixture>);
impl Factory for Synthetic {
    type Session = Session;
    fn create(self) -> Result<Session> {
        let index = self.0.starts.fetch_add(1, Relaxed);
        let path = self.0.root.join(format!("capture-{index}.wav"));
        Ok(Session {
            fixture: self.0,
            path,
            phase: Phase::Prepared,
            error: None,
            cleanup: CleanupStatus::Progress,
        })
    }
}

struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Relaxed);
    }
}

struct Session {
    fixture: Arc<Fixture>,
    path: PathBuf,
    phase: Phase,
    error: Option<Error>,
    cleanup: CleanupStatus,
}
impl Session {
    fn release(&mut self) -> Phase {
        self.cleanup = CleanupStatus::Released;
        self.phase = Phase::Released;
        self.phase
    }
}
impl CaptureSession for Session {
    fn begin(&mut self) -> Result<()> {
        let gate = self.fixture.start_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.hold();
        }
        if matches!(self.fixture.mode, Mode::CannotOpen) {
            self.error = Some(Error::Open);
            self.release();
            return Err(Error::Open);
        }
        self.phase = Phase::Running;
        Ok(())
    }
    fn stop(&mut self) -> Result<Phase> {
        self.fixture.stops.fetch_add(1, Relaxed);
        if matches!(self.fixture.mode, Mode::ReleaseError) {
            self.phase = Phase::Stopping;
            self.cleanup = CleanupStatus::RecoveryRequired;
            self.error = Some(Error::Reset);
            return Err(Error::Reset);
        }
        Ok(self.release())
    }
    fn cancel(&mut self) -> Result<Phase> {
        Ok(self.release())
    }
    fn poll(&mut self) -> Result<Phase> {
        if self.cleanup == CleanupStatus::RecoveryRequired {
            self.fixture.recoveries.fetch_add(1, Relaxed);
            self.release();
        }
        Ok(self.phase)
    }
    fn phase(&self) -> Phase {
        self.phase
    }
    fn error(&self) -> Option<Error> {
        self.error
    }
    fn cleanup_status(&self) -> CleanupStatus {
        self.cleanup
    }
    fn take_recording(&mut self) -> Option<Recording> {
        let mut wav = Wav::new(File::create_new(&self.path).unwrap()).unwrap();
        wav.append(&[23]).unwrap();
        Some(Recording::caller_owned_with_guard(
            wav.finish().unwrap(),
            self.path.clone(),
            Some("zh".into()),
            Arc::new(Guard(self.fixture.guards_dropped.clone())),
        ))
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        let gate = self.fixture.release_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.hold();
        }
    }
}

#[derive(Default)]
struct FakeService {
    calls: AtomicUsize,
    fail_next: AtomicBool,
    gate: Mutex<Option<Gate>>,
}
impl Service for FakeService {
    fn run(&self, job: &Job) -> std::result::Result<String, ServiceFailure> {
        self.calls.fetch_add(1, Relaxed);
        assert_eq!(job.language(), Some("zh"));
        assert_eq!(&std::fs::read(job.path()).unwrap()[..4], b"RIFF");
        if self.fail_next.swap(false, Relaxed) {
            return Err(ServiceFailure);
        }
        let gate = self.gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.hold();
        }
        Ok("synthetic transcript".into())
    }
}

#[derive(Default)]
struct Clipboard(Vec<String>);
impl ManualClipboard for Clipboard {
    fn copy(&mut self, text: &str) -> std::result::Result<(), String> {
        self.0.push(text.into());
        Ok(())
    }
}
fn dispatch(ui: &mut Actions, sink: &mut Clipboard, actions: &[Action]) -> bool {
    ui.dispatch(
        actions
            .iter()
            .fold(0, |bits, action| bits | (1 << *action as u32)),
        sink,
    )
}
fn wait(mut operation: impl FnMut() -> bool) {
    let deadline = Instant::now() + LIMIT;
    while !operation() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for UI transition"
        );
        thread::sleep(Duration::from_millis(2));
    }
}
fn ui(fixture: &Arc<Fixture>, service: &Arc<FakeService>) -> Actions {
    let fixture = fixture.clone();
    Actions::with_capture(Executor::new(service.clone()), move || {
        Synthetic(fixture.clone())
    })
}

// One matrix exercises the real process-wide owner and executor without having
// independent test functions race for those intentional single-owner permits.
#[test]
fn tray_and_hotkey_share_capture_lifecycle() {
    let root = tempfile::tempdir().unwrap();
    let fixture = Fixture::new(root.path().to_owned(), Mode::Normal);
    let service = Arc::new(FakeService::default());
    let mut sink = Clipboard::default();
    let mut unsupported = Actions::with_executor(Executor::new(service.clone()));
    assert!(!unsupported.can_start());
    assert!(!unsupported.can_stop());
    dispatch(
        &mut unsupported,
        &mut sink,
        &[Action::ToggleCapture, Action::Poll],
    );
    assert_eq!(service.calls.load(Relaxed), 0);

    let mut actions = ui(&fixture, &service);
    assert_eq!(fixture.starts.load(Relaxed), 0);
    assert!(actions.can_start());
    assert!(!actions.can_stop());
    let (gate, starting, begin) = Gate::new();
    *fixture.start_gate.lock().unwrap() = Some(gate);
    let (gate, releasing, release_capture) = Gate::new();
    *fixture.release_gate.lock().unwrap() = Some(gate);
    let (gate, transcribing, finish_service) = Gate::new();
    *service.gate.lock().unwrap() = Some(gate);
    dispatch(&mut actions, &mut sink, &[Action::Start]);
    starting.recv_timeout(LIMIT).unwrap();
    let mut competing = ui(&fixture, &service);
    dispatch(&mut competing, &mut sink, &[Action::Start]);
    assert_eq!(competing.status, Error::Busy.to_string());
    assert!(!competing.can_stop());
    assert!(dispatch(&mut competing, &mut sink, &[Action::Exit]));
    assert!(actions.can_stop());
    dispatch(&mut actions, &mut sink, &[Action::Start]);
    dispatch(&mut actions, &mut sink, &[Action::ToggleCapture]);
    dispatch(
        &mut actions,
        &mut sink,
        &[Action::Stop, Action::ToggleCapture],
    );
    assert!(!actions.can_stop());
    begin.send(()).unwrap();
    releasing.recv_timeout(LIMIT).unwrap();
    dispatch(&mut actions, &mut sink, &[Action::Start, Action::Poll]);
    assert_eq!(fixture.starts.load(Relaxed), 1);
    assert_eq!(fixture.stops.load(Relaxed), 1);
    assert_eq!(service.calls.load(Relaxed), 0);
    assert!(!actions.can_start());
    release_capture.send(()).unwrap();
    wait(|| {
        dispatch(&mut actions, &mut sink, &[Action::Poll]);
        transcribing.try_recv().is_ok()
    });
    dispatch(
        &mut actions,
        &mut sink,
        &[Action::ToggleCapture, Action::Poll],
    );
    assert!(!actions.can_start());
    assert_eq!(fixture.guards_dropped.load(Relaxed), 0);
    finish_service.send(()).unwrap();
    wait(|| {
        dispatch(&mut actions, &mut sink, &[Action::Poll]);
        actions.can_copy() && actions.can_start()
    });
    assert_eq!(fixture.guards_dropped.load(Relaxed), 1);
    assert!(sink.0.is_empty());
    dispatch(&mut actions, &mut sink, &[Action::Copy]);
    assert_eq!(sink.0, ["synthetic transcript"]);

    // Cancel while a finalized recording is still on the owner thread. The
    // simultaneous timer and copy must never consume the late recording.
    let (gate, releasing, release_capture) = Gate::new();
    *fixture.release_gate.lock().unwrap() = Some(gate);
    wait(|| {
        dispatch(&mut actions, &mut sink, &[Action::ToggleCapture]);
        actions.can_stop()
    });
    assert!(!actions.can_copy());
    dispatch(&mut actions, &mut sink, &[Action::Stop]);
    releasing.recv_timeout(LIMIT).unwrap();
    dispatch(
        &mut actions,
        &mut sink,
        &[Action::Cancel, Action::Copy, Action::Start, Action::Poll],
    );
    assert!(!actions.can_start());
    release_capture.send(()).unwrap();
    wait(|| {
        dispatch(&mut actions, &mut sink, &[Action::Poll]);
        actions.can_start()
    });
    assert_eq!(fixture.guards_dropped.load(Relaxed), 2);
    assert_eq!(service.calls.load(Relaxed), 1);
    assert_eq!(sink.0.len(), 1);

    // A failed transcription keeps its capture guard for explicit retry.
    service.fail_next.store(true, Relaxed);
    wait(|| {
        dispatch(&mut actions, &mut sink, &[Action::Start]);
        actions.can_stop()
    });
    dispatch(&mut actions, &mut sink, &[Action::Stop]);
    wait(|| {
        dispatch(&mut actions, &mut sink, &[Action::Poll]);
        actions.can_retry()
    });
    assert!(!actions.can_start());
    assert_eq!(fixture.guards_dropped.load(Relaxed), 2);
    let (gate, retrying, finish_retry) = Gate::new();
    *service.gate.lock().unwrap() = Some(gate);
    dispatch(&mut actions, &mut sink, &[Action::Retry]);
    retrying.recv_timeout(LIMIT).unwrap();
    assert!(!dispatch(
        &mut actions,
        &mut sink,
        &[Action::Exit, Action::Poll]
    ));
    assert_eq!(fixture.guards_dropped.load(Relaxed), 2);
    finish_retry.send(()).unwrap();
    wait(|| dispatch(&mut actions, &mut sink, &[Action::Poll]));
    assert_eq!(fixture.guards_dropped.load(Relaxed), 3);
    assert_eq!(service.calls.load(Relaxed), 3);
    assert!(!actions.can_copy());

    let failing = Fixture::new(root.path().to_owned(), Mode::CannotOpen);
    let mut actions = ui(&failing, &service);
    wait(|| {
        dispatch(&mut actions, &mut sink, &[Action::Start]);
        failing.starts.load(Relaxed) == 1
    });
    wait(|| {
        dispatch(&mut actions, &mut sink, &[Action::Poll]);
        actions.can_start()
    });
    assert_eq!(actions.status, Error::Open.to_string());
    assert_eq!(service.calls.load(Relaxed), 3);
    assert!(dispatch(&mut actions, &mut sink, &[Action::Exit]));

    let recovery = Fixture::new(root.path().to_owned(), Mode::ReleaseError);
    let mut actions = ui(&recovery, &service);
    wait(|| {
        dispatch(&mut actions, &mut sink, &[Action::Start]);
        recovery.starts.load(Relaxed) == 1
    });
    dispatch(&mut actions, &mut sink, &[Action::Stop]);
    wait(|| {
        dispatch(&mut actions, &mut sink, &[Action::Poll]);
        actions.can_recover_capture()
    });
    assert!(!dispatch(
        &mut actions,
        &mut sink,
        &[Action::Exit, Action::RecoverCapture, Action::Poll]
    ));
    for _ in 0..3 {
        assert!(!dispatch(
            &mut actions,
            &mut sink,
            &[Action::Start, Action::Stop, Action::Poll]
        ));
    }
    assert_eq!(recovery.recoveries.load(Relaxed), 0);
    dispatch(&mut actions, &mut sink, &[Action::RecoverCapture]);
    wait(|| dispatch(&mut actions, &mut sink, &[Action::Poll]));
    assert_eq!(recovery.recoveries.load(Relaxed), 1);
    assert_eq!(service.calls.load(Relaxed), 3);
}
