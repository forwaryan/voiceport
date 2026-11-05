use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind};

#[derive(Default)]
struct Fault {
    at: Mutex<Option<&'static str>>,
    backend_body_returning: AtomicBool,
    tail_returned: AtomicBool,
}
impl Fault {
    fn check(&self, at: &'static str) {
        if *self.at.lock().unwrap() == Some(at) {
            panic!("synthetic cleanup fault");
        }
    }
}
struct Tail {
    fault: Arc<Fault>,
    gate: Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>,
}
impl Drop for Tail {
    fn drop(&mut self) {
        if let Some((entered, release)) = self.gate.take() {
            entered.send(()).unwrap();
            release.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        self.fault.tail_returned.store(true, Ordering::Release);
    }
}
struct FaultBackend {
    fake: Fake,
    fault: Arc<Fault>,
    _tail: Tail,
}
impl Backend for FaultBackend {
    fn open(&mut self, id: u64, f: Format) -> Result<Device> {
        self.fault.check("open");
        self.fake.open(id, f)
    }
    fn prepare(&mut self) -> Result<()> {
        self.fault.check("prepare");
        self.fake.prepare()
    }
    fn start(&mut self) -> Result<()> {
        self.fault.check("start");
        self.fake.start()
    }
    fn read(&mut self, draining: bool, frame: &mut Frame) -> Result<Read> {
        self.fault.check("read");
        self.fake.read(draining, frame)
    }
    fn stop(&mut self) -> Result<()> {
        self.fault.check("stop");
        self.fake.stop()
    }
    fn reset(&mut self) -> Result<()> {
        self.fault.check("reset");
        self.fake.reset()
    }
    fn unprepare(&mut self) -> Result<()> {
        self.fault.check("unprepare");
        self.fake.unprepare()
    }
    fn close(&mut self) -> Result<()> {
        self.fault.check("close");
        self.fake.close()
    }
}
impl Drop for FaultBackend {
    fn drop(&mut self) {
        self.fault.check("backend-drop");
        self.fault
            .backend_body_returning
            .store(true, Ordering::Release);
    }
}
struct FaultOutput {
    inner: Option<Sink>,
    fault: Arc<Fault>,
}
impl Output for FaultOutput {
    fn append(&mut self, samples: &[i16]) -> Result<()> {
        self.fault.check("append");
        self.inner.as_mut().unwrap().append(samples)
    }
    fn finish(mut self, language: Option<String>) -> Result<Recording> {
        self.fault.check("finish");
        self.inner.take().unwrap().finish(language)
    }
}
impl Drop for FaultOutput {
    fn drop(&mut self) {
        self.fault.check("output-drop");
    }
}
fn fault_session(
    d: &Path,
    f: &Arc<Fault>,
    t: &Arc<Trace>,
    b: &Budget,
    c: &Arc<AtomicUsize>,
) -> Session<FaultBackend, FaultOutput> {
    Session::with_backend(
        FaultBackend {
            fake: Fake::new(t, 1),
            fault: f.clone(),
            _tail: Tail {
                fault: f.clone(),
                gate: None,
            },
        },
        FaultOutput {
            inner: Some(output(d, "fault.wav", t, Arc::new(Guard(c.clone())))),
            fault: f.clone(),
        },
        Some("de".into()),
        b,
    )
    .unwrap()
}
#[test]
fn destructor_and_finish_panics_never_confirm_explicit_or_drop_release() {
    for entry in ["stop", "cancel", "poll", "drop"] {
        for fault in ["backend-drop", "output-drop", "finish"] {
            if fault == "finish" && ["cancel", "drop"].contains(&entry) {
                continue;
            }
            let d = tempfile::tempdir().unwrap();
            let f = Arc::new(Fault::default());
            let t = trace();
            let b = Budget::default();
            let count = Arc::new(AtomicUsize::new(0));
            let mut s = fault_session(d.path(), &f, &t, &b, &count);
            s.begin().unwrap();
            if entry == "poll" {
                t.returned.store(false, Ordering::Release);
                assert_eq!(s.stop(), Ok(Phase::Stopping));
                t.returned.store(true, Ordering::Release);
            }
            *f.at.lock().unwrap() = Some(fault);
            if entry == "drop" {
                assert!(catch_unwind(AssertUnwindSafe(|| drop(s))).is_err());
            } else {
                assert!(
                    catch_unwind(AssertUnwindSafe(|| {
                        match entry {
                            "stop" => s.stop(),
                            "cancel" => s.cancel(),
                            _ => s.poll(),
                        }
                    }))
                    .is_err()
                );
                assert!(b.occupied());
                assert!(s.take_recording().is_none());
                let calls = t.calls.lock().unwrap().len();
                for _ in 0..3 {
                    assert_eq!(s.poll(), Err(Error::Cleanup));
                }
                assert_eq!(s.cancel(), Err(Error::Cleanup));
                assert_eq!(s.stop(), Err(Error::Cleanup));
                assert_eq!(
                    t.calls.lock().unwrap().len(),
                    calls,
                    "no retry after caught unwind"
                );
                drop(s);
            }
            assert!(
                b.occupied(),
                "{entry}/{fault}: destructor RETURN was not confirmed"
            );
            if fault == "backend-drop" {
                assert_eq!(count.load(Ordering::SeqCst), 0);
            }
        }
    }
}
#[test]
fn caught_operation_panic_is_latched_before_any_later_poll_or_drop() {
    for stage in [
        "open",
        "prepare",
        "start",
        "read",
        "append",
        "stop",
        "reset",
        "unprepare",
        "close",
    ] {
        let d = tempfile::tempdir().unwrap();
        let f = Arc::new(Fault::default());
        let t = trace();
        let b = Budget::default();
        let count = Arc::new(AtomicUsize::new(0));
        let mut s = fault_session(d.path(), &f, &t, &b, &count);
        if !["open", "prepare", "start"].contains(&stage) {
            s.begin().unwrap();
        }
        *f.at.lock().unwrap() = Some(stage);
        assert!(
            catch_unwind(AssertUnwindSafe(|| match stage {
                "open" | "prepare" | "start" => s.begin().map(|()| Phase::Running),
                "read" | "append" => s.poll(),
                _ => s.stop(),
            }))
            .is_err()
        );
        *f.at.lock().unwrap() = None; // even removing the injected fault is NOT proof
        let calls = t.calls.lock().unwrap().len();
        assert_eq!(s.poll(), Err(Error::Cleanup));
        assert_eq!(s.cancel(), Err(Error::Cleanup));
        assert!(s.take_recording().is_none());
        drop(s);
        assert_eq!(t.calls.lock().unwrap().len(), calls);
        assert!(b.occupied());
        assert_eq!(count.load(Ordering::SeqCst), 0);
    }
}
#[test]
fn destructor_field_gate_not_body_flag_controls_output_and_admission() {
    let d = tempfile::tempdir().unwrap();
    let f = Arc::new(Fault::default());
    let t = trace();
    let b = Budget::default();
    let count = Arc::new(AtomicUsize::new(0));
    let (tx, entered) = mpsc::sync_channel(1);
    let (release, rx) = mpsc::sync_channel(1);
    let mut s = Session::with_backend(
        FaultBackend {
            fake: Fake::new(&t, 1),
            fault: f.clone(),
            _tail: Tail {
                fault: f.clone(),
                gate: Some((tx, rx)),
            },
        },
        output(
            d.path(),
            "field-gate.wav",
            &t,
            Arc::new(Guard(count.clone())),
        ),
        None,
        &b,
    )
    .unwrap();
    s.begin().unwrap();
    let h = std::thread::spawn(move || drop(s));
    entered.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(f.backend_body_returning.load(Ordering::Acquire));
    assert!(t.calls.lock().unwrap().contains(&"backend-drop-returning"));
    assert!(!f.tail_returned.load(Ordering::Acquire));
    assert!(b.occupied());
    assert_eq!(count.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    h.join().unwrap();
    assert!(f.tail_returned.load(Ordering::Acquire));
    assert!(!b.occupied());
    assert_eq!(count.load(Ordering::SeqCst), 1);
}
#[test]
fn ordinary_drop_drains_all_bounded_returned_packets_then_later_admission() {
    for n in [
        0,
        FRAME_SAMPLES * 3,
        FRAME_SAMPLES * 4,
        FRAME_SAMPLES * 4 + 1,
        MAX_SAMPLES,
        MAX_SAMPLES + FRAME_SAMPLES,
        usize::MAX,
    ] {
        let d = tempfile::tempdir().unwrap();
        let (mut s, t, b, c) = setup(d.path(), n);
        s.begin().unwrap();
        drop(s);
        assert!(!b.occupied(), "{n}");
        assert_eq!(c.load(Ordering::SeqCst), 1);
        let calls = t.calls.lock().unwrap();
        assert_eq!(calls.iter().filter(|&&x| x == "close").count(), 1);
        assert!(calls.iter().filter(|&&x| x == "read").count() <= MAX_DRAIN_PACKETS + 1);
        drop(calls);
        let mut next = Session::with_backend(
            Fake::new(&t, 1),
            output(
                d.path(),
                "next.wav",
                &t,
                Arc::new(Guard(Arc::new(AtomicUsize::new(0)))),
            ),
            None,
            &b,
        )
        .unwrap();
        next.begin().unwrap();
        next.stop().unwrap();
        release(&mut next);
        assert!(next.take_recording().is_some());
    }
}
#[test]
fn unknown_cleanup_drop_never_spins_or_falsely_releases() {
    for stage in ["idle", "reset", "unprepare", "close"] {
        let d = tempfile::tempdir().unwrap();
        let (mut s, t, b, c) = setup(d.path(), FRAME_SAMPLES * 6);
        s.begin().unwrap();
        if stage == "idle" {
            t.returned.store(false, Ordering::Release);
        } else {
            *t.fail.lock().unwrap() = Some(stage);
        }
        drop(s);
        assert!(b.occupied());
        assert_eq!(c.load(Ordering::SeqCst), 0);
        let calls = t.calls.lock().unwrap();
        if stage == "idle" {
            assert_eq!(calls.iter().filter(|&&x| x == "read").count(), 1);
        } else {
            assert_eq!(calls.iter().filter(|&&x| x == stage).count(), 1);
        }
        assert!(!calls.contains(&"backend-drop-returning"));
    }
}
