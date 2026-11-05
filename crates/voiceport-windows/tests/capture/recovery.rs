use super::*;

#[derive(Default)]
struct ReleaseFault {
    at: Mutex<Option<&'static str>>,
    calls: Mutex<Vec<&'static str>>,
}
impl ReleaseFault {
    fn call(&self, stage: &'static str) -> Result<()> {
        self.calls.lock().unwrap().push(stage);
        if *self.at.lock().unwrap() == Some(stage) {
            Err(Error::Read)
        } else {
            Ok(())
        }
    }
    fn count(&self, stage: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|&&s| s == stage)
            .count()
    }
}
struct RecoveryBackend {
    fake: Fake,
    fault: Arc<ReleaseFault>,
    read_error: Option<Error>,
}
impl Backend for RecoveryBackend {
    fn open(&mut self, id: u64, f: Format) -> Result<Device> {
        self.fake.open(id, f)
    }
    fn prepare(&mut self) -> Result<()> {
        self.fake.prepare()
    }
    fn start(&mut self) -> Result<()> {
        self.fake.start()
    }
    fn read(&mut self, draining: bool, frame: &mut Frame) -> Result<Read> {
        if !draining && let Some(e) = self.read_error.take() {
            return Err(e);
        }
        self.fake.read(draining, frame)
    }
    fn stop(&mut self) -> Result<()> {
        self.fake.stop()
    }
    fn reset(&mut self) -> Result<()> {
        self.fault.call("reset")?;
        self.fake.reset()
    }
    fn unprepare(&mut self) -> Result<()> {
        self.fault.call("unprepare")?;
        self.fake.unprepare()
    }
    fn close(&mut self) -> Result<()> {
        self.fault.call("close")?;
        self.fake.close()
    }
}
fn session(
    d: &Path,
    n: usize,
    t: &Arc<Trace>,
    b: &Budget,
    c: &Arc<AtomicUsize>,
    f: &Arc<ReleaseFault>,
) -> Session<RecoveryBackend, Sink> {
    Session::with_backend(
        RecoveryBackend {
            fake: Fake::new(t, n),
            fault: f.clone(),
            read_error: None,
        },
        output(d, "recovery.wav", t, Arc::new(Guard(c.clone()))),
        None,
        b,
    )
    .unwrap()
}
fn stage_error(stage: &str) -> Error {
    match stage {
        "reset" => Error::Reset,
        "unprepare" => Error::Unprepare,
        "close" => Error::Close,
        _ => unreachable!(),
    }
}
fn explicit(s: &mut Session<RecoveryBackend, Sink>, entry: &str) -> Result<Phase> {
    match entry {
        "stop" => s.stop(),
        "cancel" => s.cancel(),
        "poll" => s.poll(),
        _ => unreachable!(),
    }
}

#[test]
fn returned_release_error_retains_owner_on_drop_even_after_fault_removed() {
    for stage in ["reset", "unprepare", "close"] {
        for entry in ["stop", "cancel", "poll"] {
            for transient in [false, true] {
                let d = tempfile::tempdir().unwrap();
                let t = trace();
                let b = Budget::default();
                let c = Arc::new(AtomicUsize::new(0));
                let f = Arc::new(ReleaseFault::default());
                let mut s = session(d.path(), 1, &t, &b, &c, &f);
                s.begin().unwrap();
                *f.at.lock().unwrap() = Some(stage);
                if entry == "poll" {
                    *t.fail.lock().unwrap() = Some("read");
                }
                assert_eq!(explicit(&mut s, entry), Err(stage_error(stage)));
                assert_eq!(s.phase(), Phase::Stopping);
                assert!(b.occupied());
                assert_eq!(f.count(stage), 1);
                assert!(s.take_recording().is_none());
                if transient {
                    *f.at.lock().unwrap() = None;
                }
                let calls = t.calls.lock().unwrap().clone();
                let releases = f.calls.lock().unwrap().clone();
                drop(s);
                assert_eq!(
                    *t.calls.lock().unwrap(),
                    calls,
                    "{stage}/{entry}/{transient}"
                );
                assert_eq!(*f.calls.lock().unwrap(), releases);
                assert!(b.occupied());
                assert_eq!(c.load(Ordering::SeqCst), 0);
            }
        }
    }
}

#[test]
fn repeated_explicit_stop_cancel_and_poll_recover_without_admitting_prefix() {
    for stage in ["reset", "unprepare", "close"] {
        for entry in ["stop", "cancel", "poll"] {
            let d = tempfile::tempdir().unwrap();
            let t = trace();
            let b = Budget::default();
            let c = Arc::new(AtomicUsize::new(0));
            let f = Arc::new(ReleaseFault::default());
            let mut s = session(d.path(), 1, &t, &b, &c, &f);
            s.begin().unwrap();
            s.poll().unwrap(); // a valid written prefix must never be finalized after a release error
            *f.at.lock().unwrap() = Some(stage);
            assert_eq!(s.stop(), Err(stage_error(stage)));
            for _ in 0..2 {
                assert_eq!(explicit(&mut s, entry), Err(stage_error(stage)));
                assert!(b.occupied());
                assert_eq!(c.load(Ordering::SeqCst), 0);
            }
            assert_eq!(f.count(stage), 3);
            *f.at.lock().unwrap() = None;
            assert_eq!(explicit(&mut s, entry), Err(stage_error(stage)));
            assert_eq!(f.count(stage), 4);
            assert_eq!(s.phase(), Phase::Released);
            assert_eq!(s.error(), Some(stage_error(stage)));
            assert!(s.take_recording().is_none());
            assert!(!t.calls.lock().unwrap().contains(&"finish"));
            assert!(!b.occupied());
            assert_eq!(c.load(Ordering::SeqCst), 1);
            drop(s);
            assert_eq!(c.load(Ordering::SeqCst), 1);
        }
    }
}

#[test]
fn explicit_partial_recovery_tracks_each_new_uncertain_release_stage() {
    for (first, next) in [("reset", "unprepare"), ("unprepare", "close")] {
        for recover_next in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let t = trace();
            let b = Budget::default();
            let c = Arc::new(AtomicUsize::new(0));
            let f = Arc::new(ReleaseFault::default());
            let mut s = session(d.path(), 1, &t, &b, &c, &f);
            s.begin().unwrap();
            *f.at.lock().unwrap() = Some(first);
            assert_eq!(s.stop(), Err(stage_error(first)));
            *f.at.lock().unwrap() = Some(next);
            assert_eq!(s.cancel(), Err(stage_error(next)));
            assert_eq!(s.error(), Some(stage_error(first)));
            assert_eq!(f.count(first), 2);
            assert_eq!(f.count(next), 1);
            *f.at.lock().unwrap() = None;
            if recover_next {
                assert_eq!(s.poll(), Err(stage_error(first)));
                assert_eq!(s.phase(), Phase::Released);
                assert!(!b.occupied());
                assert_eq!(c.load(Ordering::SeqCst), 1);
            }
            let releases = f.calls.lock().unwrap().clone();
            drop(s);
            assert_eq!(*f.calls.lock().unwrap(), releases);
            assert_eq!(b.occupied(), !recover_next);
            assert_eq!(c.load(Ordering::SeqCst), usize::from(recover_next));
        }
    }
    // A successful explicit reset followed by a returned-data quota authorizes ordinary finite Drop progress.
    let d = tempfile::tempdir().unwrap();
    let t = trace();
    let b = Budget::default();
    let c = Arc::new(AtomicUsize::new(0));
    let f = Arc::new(ReleaseFault::default());
    let mut s = session(d.path(), FRAME_SAMPLES * 8 + 1, &t, &b, &c, &f);
    s.begin().unwrap();
    *f.at.lock().unwrap() = Some("reset");
    assert_eq!(s.stop(), Err(Error::Reset));
    *f.at.lock().unwrap() = None;
    assert_eq!(s.cancel(), Ok(Phase::Stopping));
    assert_eq!(f.count("reset"), 2);
    assert_eq!(
        t.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|&&x| x == "read")
            .count(),
        4
    );
    drop(s);
    assert_eq!(f.count("reset"), 2);
    assert_eq!(f.count("close"), 1);
    assert!(!b.occupied());
    assert_eq!(c.load(Ordering::SeqCst), 1);
    assert!(!t.calls.lock().unwrap().contains(&"finish"));
}

#[test]
fn earlier_open_read_or_output_error_preserves_later_release_uncertainty() {
    for first in ["open", "read", "write"] {
        for stage in ["reset", "unprepare", "close"] {
            for recover in [false, true] {
                let d = tempfile::tempdir().unwrap();
                let t = trace();
                let b = Budget::default();
                let c = Arc::new(AtomicUsize::new(0));
                let f = Arc::new(ReleaseFault::default());
                let mut s = session(d.path(), 1, &t, &b, &c, &f);
                *t.fail.lock().unwrap() = Some(first);
                *f.at.lock().unwrap() = Some(stage);
                let expected = match first {
                    "open" => Error::Open,
                    "read" => Error::Read,
                    _ => Error::Write,
                };
                if first == "open" {
                    assert_eq!(s.begin(), Err(Error::Open));
                } else {
                    s.begin().unwrap();
                    assert_eq!(s.poll(), Err(stage_error(stage)));
                }
                assert_eq!(s.error(), Some(expected));
                assert_eq!(f.count(stage), 1);
                *t.fail.lock().unwrap() = None;
                *f.at.lock().unwrap() = None;
                if recover {
                    assert_eq!(s.stop(), Err(expected));
                    assert_eq!(s.phase(), Phase::Released);
                    assert_eq!(f.count(stage), 2);
                    assert_eq!(s.error(), Some(expected));
                }
                assert!(s.take_recording().is_none());
                let calls = f.calls.lock().unwrap().clone();
                drop(s);
                assert_eq!(*f.calls.lock().unwrap(), calls);
                assert_eq!(b.occupied(), !recover);
                assert_eq!(c.load(Ordering::SeqCst), usize::from(recover));
                assert!(!t.calls.lock().unwrap().contains(&"finish"));
            }
        }
    }
}

#[test]
fn arbitrary_first_error_is_not_release_uncertainty_and_idle_does_not_spin() {
    for first in [Error::Reset, Error::Unprepare, Error::Close] {
        let d = tempfile::tempdir().unwrap();
        let t = trace();
        let b = Budget::default();
        let c = Arc::new(AtomicUsize::new(0));
        let f = Arc::new(ReleaseFault::default());
        let mut s = Session::with_backend(
            RecoveryBackend {
                fake: Fake::new(&t, FRAME_SAMPLES * 8 + 1),
                fault: f.clone(),
                read_error: Some(first),
            },
            output(d.path(), "arbitrary.wav", &t, Arc::new(Guard(c.clone()))),
            None,
            &b,
        )
        .unwrap();
        s.begin().unwrap();
        assert_eq!(s.poll(), Ok(Phase::Stopping));
        assert_eq!(s.error(), Some(first));
        drop(s);
        assert!(!b.occupied());
        assert_eq!(c.load(Ordering::SeqCst), 1);
        assert_eq!(f.count("close"), 1);
        assert!(!t.calls.lock().unwrap().contains(&"finish"));
    }
    let d = tempfile::tempdir().unwrap();
    let t = trace();
    let b = Budget::default();
    let c = Arc::new(AtomicUsize::new(0));
    let f = Arc::new(ReleaseFault::default());
    let mut s = session(d.path(), 1, &t, &b, &c, &f);
    s.begin().unwrap();
    *f.at.lock().unwrap() = Some("reset");
    assert_eq!(s.stop(), Err(Error::Reset));
    *f.at.lock().unwrap() = None;
    t.returned.store(false, Ordering::Release);
    assert_eq!(s.poll(), Ok(Phase::Stopping));
    assert_eq!(f.count("reset"), 2);
    let reads = t
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|&&x| x == "read")
        .count();
    drop(s);
    assert_eq!(f.count("reset"), 2);
    assert_eq!(f.count("unprepare"), 0);
    assert_eq!(
        t.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|&&x| x == "read")
            .count(),
        reads + 1
    );
    assert!(b.occupied());
    assert_eq!(c.load(Ordering::SeqCst), 0);
}

#[test]
fn stop_error_does_not_prevent_finite_returned_data_drop_cleanup() {
    let d = tempfile::tempdir().unwrap();
    let t = trace();
    let b = Budget::default();
    let c = Arc::new(AtomicUsize::new(0));
    let f = Arc::new(ReleaseFault::default());
    let mut s = session(d.path(), FRAME_SAMPLES * 8 + 1, &t, &b, &c, &f);
    s.begin().unwrap();
    *t.fail.lock().unwrap() = Some("stop");
    assert_eq!(s.stop(), Ok(Phase::Stopping));
    assert_eq!(s.error(), Some(Error::Stop));
    drop(s);
    assert_eq!(f.count("reset"), 1);
    assert_eq!(f.count("unprepare"), 1);
    assert_eq!(f.count("close"), 1);
    assert!(!b.occupied());
    assert_eq!(c.load(Ordering::SeqCst), 1);
    assert!(!t.calls.lock().unwrap().contains(&"finish"));
}
