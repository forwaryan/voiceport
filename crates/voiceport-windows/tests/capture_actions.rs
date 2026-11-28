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
        Backend, Budget, CleanupStatus, Device, Error, FRAME_SAMPLES, Format, Frame, Output, PCM,
        Phase, Read, Result,
        owner::{CaptureSession, Factory},
    },
    executor::{Executor, Service, ServiceFailure},
    wav::{MAX_SAMPLES, Wav},
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
    wav_bytes: AtomicUsize,
    fail_next: AtomicBool,
    gate: Mutex<Option<Gate>>,
}
impl Service for FakeService {
    fn run(&self, job: &Job) -> std::result::Result<String, ServiceFailure> {
        self.calls.fetch_add(1, Relaxed);
        assert_eq!(job.language(), Some("zh"));
        let wav = std::fs::read(job.path()).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(wav[40..44].try_into().unwrap()) as usize,
            wav.len() - 44
        );
        self.wav_bytes.store(wav.len(), Relaxed);
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

    buffered_limit_matrix(root.path(), &mut sink);
}

// Exercise the production Session as well as the owner/Actions/executor. The
// backend models a completed fixed buffer, not elapsed real recording time.
struct BufferedCapture {
    path: PathBuf,
    at_limit: bool,
    samples: usize,
    reset_gate: Mutex<Option<Gate>>,
    stops: AtomicUsize,
}
struct BufferedBackend {
    capture: Arc<BufferedCapture>,
    id: u64,
    sequence: u64,
    remaining: usize,
    reset: bool,
}
impl Backend for BufferedBackend {
    fn open(&mut self, id: u64, format: Format) -> Result<Device> {
        self.id = id;
        Ok(Device { id: 1, format })
    }
    fn prepare(&mut self) -> Result<()> {
        Ok(())
    }
    fn start(&mut self) -> Result<()> {
        Ok(())
    }
    fn read(&mut self, draining: bool, frame: &mut Frame) -> Result<Read> {
        if !draining {
            return Ok(if self.capture.at_limit {
                Read::LimitReached
            } else {
                Read::Idle
            });
        }
        assert!(self.reset);
        let n = self.remaining.min(FRAME_SAMPLES);
        if n == 0 {
            return Ok(Read::End);
        }
        self.sequence += 1;
        self.remaining -= n;
        frame.session = self.id;
        frame.device = 1;
        frame.sequence = self.sequence;
        frame.format = PCM;
        frame.bytes = n * 2;
        frame.done = true;
        frame.samples[..n].fill(23);
        thread::current().unpark();
        Ok(Read::Data)
    }
    fn stop(&mut self) -> Result<()> {
        assert_eq!(self.capture.stops.fetch_add(1, Relaxed), 0);
        Ok(())
    }
    fn reset(&mut self) -> Result<()> {
        assert_eq!(self.capture.stops.load(Relaxed), 1);
        let gate = self.capture.reset_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            gate.hold();
        }
        self.reset = true;
        Ok(())
    }
    fn unprepare(&mut self) -> Result<()> {
        Ok(())
    }
    fn close(&mut self) -> Result<()> {
        Ok(())
    }
}
struct BufferedOutput(Wav<File>, PathBuf);
impl Output for BufferedOutput {
    fn append(&mut self, samples: &[i16]) -> Result<()> {
        self.0.append(samples).map_err(|_| Error::Write)
    }
    fn finish(self, language: Option<String>) -> Result<Recording> {
        Ok(Recording::caller_owned(
            self.0.finish().map_err(|_| Error::Finalize)?,
            self.1,
            language,
        ))
    }
}
struct BufferedFactory(Arc<BufferedCapture>);
impl Factory for BufferedFactory {
    type Session = voiceport_windows::capture::Session<BufferedBackend, BufferedOutput>;
    fn create(self) -> Result<Self::Session> {
        let capture = self.0;
        let path = capture.path.clone();
        let file = File::create_new(&path).map_err(|_| Error::Write)?;
        let output = BufferedOutput(Wav::new(file).map_err(|_| Error::Write)?, path);
        let remaining = capture.samples;
        Self::Session::with_backend(
            BufferedBackend {
                capture,
                id: 0,
                sequence: 0,
                remaining,
                reset: false,
            },
            output,
            Some("zh".into()),
            &Budget::default(),
        )
    }
}

fn buffered_limit_matrix(root: &std::path::Path, sink: &mut Clipboard) {
    for (name, at_limit, samples, cancel, error) in [
        ("automatic", true, MAX_SAMPLES, false, None),
        ("manual", false, 17, false, None),
        (
            "overflow",
            true,
            MAX_SAMPLES + 1,
            false,
            Some(Error::Overflow),
        ),
        (
            "short-after-limit",
            true,
            MAX_SAMPLES - 1,
            false,
            Some(Error::InvalidEvent),
        ),
        ("cancel", true, MAX_SAMPLES, true, None),
    ] {
        let (gate, resetting, release) = Gate::new();
        let capture = Arc::new(BufferedCapture {
            path: root.join(format!("buffered-{name}.wav")),
            at_limit,
            samples,
            reset_gate: Mutex::new(Some(gate)),
            stops: AtomicUsize::new(0),
        });
        let service = Arc::new(FakeService::default());
        let factory = capture.clone();
        let mut actions = Actions::with_capture(Executor::new(service.clone()), move || {
            BufferedFactory(factory.clone())
        });
        wait(|| {
            dispatch(&mut actions, sink, &[Action::Start]);
            actions.status == "Starting microphone"
        });
        if !at_limit {
            assert!(actions.can_stop());
            dispatch(&mut actions, sink, &[Action::Stop]);
        }
        // Automatic cases must reach reset without an Action::Stop or any UI poll.
        resetting.recv_timeout(LIMIT).unwrap();
        assert_eq!(capture.stops.load(Relaxed), 1);
        assert!(!actions.can_start());
        assert_eq!(service.calls.load(Relaxed), 0);
        if cancel {
            dispatch(&mut actions, sink, &[Action::Cancel, Action::Poll]);
            assert!(!actions.can_start());
        }
        release.send(()).unwrap();
        wait(|| {
            dispatch(&mut actions, sink, &[Action::Poll]);
            actions.can_start()
        });
        for _ in 0..3 {
            dispatch(&mut actions, sink, &[Action::Poll]);
        }
        if let Some(error) = error {
            assert_eq!(actions.status, error.to_string(), "{name}");
        }
        let delivered = !cancel && error.is_none();
        assert_eq!(actions.can_copy(), delivered, "{name}");
        assert_eq!(
            service.calls.load(Relaxed),
            usize::from(delivered),
            "{name}"
        );
        assert_eq!(
            service.wav_bytes.load(Relaxed),
            if delivered { 44 + 2 * samples } else { 0 },
            "{name}"
        );
        assert!(dispatch(&mut actions, sink, &[Action::Exit]));
    }
}
