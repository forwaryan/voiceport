use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    time::Duration,
};
use voiceport_windows::{
    Recording,
    capture::*,
    wav::{MAX_SAMPLES, Wav},
};
#[derive(Default)]
struct Trace {
    calls: Mutex<Vec<&'static str>>,
    fail: Mutex<Option<&'static str>>,
    release: AtomicBool,
    returned: AtomicBool,
    total: AtomicUsize,
    bad: Mutex<Option<&'static str>>,
    block_drop: Mutex<Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>>,
}
impl Trace {
    fn call(&self, s: &'static str) -> Result<()> {
        self.calls.lock().unwrap().push(s);
        if *self.fail.lock().unwrap() == Some(s) {
            Err(Error::Read)
        } else {
            Ok(())
        }
    }
}
struct Fake {
    t: Arc<Trace>,
    id: u64,
    seq: u64,
    left: usize,
    reset: bool,
}
impl Fake {
    fn new(t: &Arc<Trace>, n: usize) -> Self {
        Self {
            t: t.clone(),
            id: 0,
            seq: 0,
            left: n,
            reset: false,
        }
    }
}
impl Backend for Fake {
    fn open(&mut self, id: u64, f: Format) -> Result<Device> {
        self.t.call("open")?;
        assert_eq!(f, PCM);
        self.id = id;
        Ok(Device {
            id: 7,
            format: if *self.t.bad.lock().unwrap() == Some("format") {
                Format { rate: 24000, ..PCM }
            } else {
                PCM
            },
        })
    }
    fn prepare(&mut self) -> Result<()> {
        self.t.call("prepare")
    }
    fn start(&mut self) -> Result<()> {
        self.t.call("start")
    }
    fn read(&mut self, draining: bool, f: &mut Frame) -> Result<Read> {
        self.t.call("read")?;
        if self.id == 0 {
            return Ok(Read::End);
        }
        if draining && !self.t.returned.load(Ordering::Acquire) {
            return Ok(Read::Idle);
        }
        if self.left == 0 {
            return Ok(if draining { Read::End } else { Read::Idle });
        }
        let n = self.left.min(FRAME_SAMPLES);
        self.left -= n;
        self.seq += 1;
        f.session = self.id;
        f.device = 7;
        f.sequence = self.seq;
        f.format = PCM;
        f.bytes = n * 2;
        f.done = true;
        for i in 0..n {
            f.samples[i] = (self.t.total.fetch_add(1, Ordering::SeqCst) as i16).wrapping_mul(31);
        }
        match *self.t.bad.lock().unwrap() {
            Some("foreign") => f.session += 1,
            Some("device") => f.device = 8,
            Some("duplicate") => f.sequence = 0,
            Some("odd") => f.bytes = 3,
            Some("large") => f.bytes = FRAME_SAMPLES * 2 + 2,
            Some("zero") => f.bytes = 0,
            Some("notdone") => f.done = false,
            _ => {}
        }
        Ok(Read::Data)
    }
    fn stop(&mut self) -> Result<()> {
        self.t.call("stop")
    }
    fn reset(&mut self) -> Result<()> {
        self.t.call("reset")?;
        if !self.t.release.load(Ordering::Acquire) {
            return Err(Error::Reset);
        }
        self.reset = true;
        Ok(())
    }
    fn unprepare(&mut self) -> Result<()> {
        assert!(self.reset);
        self.t.call("unprepare")
    }
    fn close(&mut self) -> Result<()> {
        self.t.call("close")
    }
}
impl Drop for Fake {
    fn drop(&mut self) {
        if let Some((entered, release)) = self.t.block_drop.lock().unwrap().take() {
            entered.send(()).unwrap();
            release.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        self.t.calls.lock().unwrap().push("backend-drop-returning");
    }
}
struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
struct Sink {
    writer: Wav<File>,
    path: PathBuf,
    t: Arc<Trace>,
    guard: Arc<Guard>,
}
impl Output for Sink {
    fn append(&mut self, s: &[i16]) -> Result<()> {
        self.t.call("write").map_err(|_| Error::Write)?;
        self.writer.append(s).map_err(|_| Error::Write)
    }
    fn finish(self, l: Option<String>) -> Result<Recording> {
        self.t.call("finish").map_err(|_| Error::Finalize)?;
        let f = self.writer.finish().map_err(|_| Error::Finalize)?;
        Ok(Recording::caller_owned_with_guard(
            f, self.path, l, self.guard,
        ))
    }
}
fn output(d: &Path, n: &str, t: &Arc<Trace>, guard: Arc<Guard>) -> Sink {
    let path = d.join(n);
    Sink {
        writer: Wav::new(File::create_new(&path).unwrap()).unwrap(),
        path,
        t: t.clone(),
        guard,
    }
}
fn trace() -> Arc<Trace> {
    let t = Arc::new(Trace::default());
    t.release.store(true, Ordering::Release);
    t.returned.store(true, Ordering::Release);
    t
}
fn setup(d: &Path, n: usize) -> (Session<Fake, Sink>, Arc<Trace>, Budget, Arc<AtomicUsize>) {
    let t = trace();
    let budget = Budget::default();
    let count = Arc::new(AtomicUsize::new(0));
    let guard = Arc::new(Guard(count.clone()));
    let s = Session::with_backend(
        Fake::new(&t, n),
        output(d, "owned.wav", &t, guard),
        Some("zh".into()),
        &budget,
    )
    .unwrap();
    (s, t, budget, count)
}
fn release(s: &mut Session<Fake, Sink>) {
    for _ in 0..MAX_DRAIN_PACKETS {
        let _ = s.poll();
        if s.phase() == Phase::Released {
            return;
        }
    }
    panic!("synthetic cleanup never acknowledged");
}
#[test]
fn inert_and_nonwindows_native_unsupported() {
    let native = NativeCapture;
    let d = tempfile::tempdir().unwrap();
    assert_eq!(std::fs::read_dir(d.path()).unwrap().count(), 0);
    #[cfg(not(windows))]
    assert!(matches!(
        native.prepare(&voiceport_windows::storage::Root, None),
        Err(Error::Unsupported)
    ));
    let _ = native;
    let (mut s, t, b, c) = setup(d.path(), 3);
    assert!(t.calls.lock().unwrap().is_empty());
    assert!(b.occupied());
    s.cancel().unwrap();
    assert_eq!(c.load(Ordering::SeqCst), 1);
    assert!(!b.occupied());
    assert!(s.take_recording().is_none());
}
#[test]
fn success_exact_samples_language_identity_and_one_result() {
    let d = tempfile::tempdir().unwrap();
    let (mut s, t, b, c) = setup(d.path(), FRAME_SAMPLES + 3);
    s.begin().unwrap();
    s.poll().unwrap();
    s.stop().unwrap();
    release(&mut s);
    assert!(!b.occupied());
    let r = s.take_recording().unwrap();
    assert_eq!(r.language(), Some("zh"));
    assert_eq!(r.path(), d.path().join("owned.wav"));
    let bytes = std::fs::read(r.path()).unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(bytes.len(), 44 + (FRAME_SAMPLES + 3) * 2);
    for (i, p) in bytes[44..].chunks_exact(2).enumerate() {
        assert_eq!(
            i16::from_le_bytes([p[0], p[1]]),
            (i as i16).wrapping_mul(31)
        );
    }
    assert!(s.take_recording().is_none());
    let calls = t.calls.lock().unwrap();
    assert!(
        calls.iter().position(|x| *x == "close")
            < calls.iter().position(|x| *x == "backend-drop-returning")
    );
    assert!(
        calls.iter().position(|x| *x == "backend-drop-returning")
            < calls.iter().position(|x| *x == "finish")
    );
    drop(calls);
    assert_eq!(c.load(Ordering::SeqCst), 0);
    drop(r);
    assert_eq!(c.load(Ordering::SeqCst), 1);
    assert!(d.path().join("owned.wav").exists());
    assert_eq!(s.poll(), Ok(Phase::Released));
}
#[test]
fn all_stages_fail_without_admitting_partial_and_cleanup_can_recover() {
    for stage in [
        "open",
        "prepare",
        "start",
        "read",
        "write",
        "stop",
        "reset",
        "unprepare",
        "close",
        "finish",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (mut s, t, b, c) = setup(d.path(), 3);
        *t.fail.lock().unwrap() = Some(stage);
        let begun = s.begin();
        if ["open", "prepare", "start"].contains(&stage) {
            assert!(begun.is_err());
        } else {
            begun.unwrap();
            let _ = s.poll();
            let _ = s.stop();
        }
        if ["reset", "unprepare", "close"].contains(&stage) {
            assert_eq!(s.phase(), Phase::Stopping);
            assert!(b.occupied());
            assert_eq!(c.load(Ordering::SeqCst), 0);
        }
        *t.fail.lock().unwrap() = None;
        release(&mut s);
        assert!(s.take_recording().is_none(), "{stage}");
        assert!(s.error().is_some(), "{stage}");
        assert!(!b.occupied());
        assert_eq!(c.load(Ordering::SeqCst), 1);
        assert!(d.path().join("owned.wav").exists());
    }
}
#[test]
fn empty_maximum_and_excess_never_truncate_to_success() {
    for n in [0, MAX_SAMPLES, MAX_SAMPLES + 1] {
        let d = tempfile::tempdir().unwrap();
        let (mut s, _, _, _) = setup(d.path(), n);
        s.begin().unwrap();
        let _ = s.stop();
        release(&mut s);
        if n == MAX_SAMPLES {
            assert_eq!(
                std::fs::metadata(s.take_recording().unwrap().path())
                    .unwrap()
                    .len(),
                5_760_044
            );
        } else {
            assert!(s.take_recording().is_none());
            assert_eq!(
                s.error(),
                Some(if n == 0 {
                    Error::Empty
                } else {
                    Error::Overflow
                })
            );
        }
    }
}
#[test]
fn foreign_duplicate_invalid_frames_and_format_fail_closed() {
    for bad in [
        "foreign",
        "device",
        "duplicate",
        "odd",
        "large",
        "zero",
        "notdone",
        "format",
    ] {
        let d = tempfile::tempdir().unwrap();
        let (mut s, t, _, _) = setup(d.path(), 3);
        *t.bad.lock().unwrap() = Some(bad);
        let _ = s.begin();
        let _ = s.poll();
        release(&mut s);
        assert!(s.take_recording().is_none(), "{bad}");
        assert!(s.error().is_some());
        assert!(!t.calls.lock().unwrap().contains(&"finish"));
    }
}
#[test]
fn cancel_queued_late_data_and_session_identity_cannot_restore_success() {
    let d = tempfile::tempdir().unwrap();
    let (mut s, t, b, c) = setup(d.path(), FRAME_SAMPLES + 1);
    s.begin().unwrap();
    s.poll().unwrap();
    s.cancel().unwrap();
    release(&mut s);
    assert!(s.take_recording().is_none());
    let writes = t
        .calls
        .lock()
        .unwrap()
        .iter()
        .filter(|x| **x == "write")
        .count();
    assert_eq!(writes, 1);
    assert_eq!(c.load(Ordering::SeqCst), 1);
    let id = s.session_id().unwrap();
    let guard = Arc::new(Guard(Arc::new(AtomicUsize::new(0))));
    let mut next = Session::with_backend(
        Fake::new(&t, 1),
        output(d.path(), "next.wav", &t, guard),
        None,
        &b,
    )
    .unwrap();
    assert!(next.session_id().unwrap() > id);
    next.begin().unwrap();
    next.stop().unwrap();
    release(&mut next);
    assert!(next.take_recording().is_some());
    assert!(s.take_recording().is_none());
    assert!(s.begin().is_err());
}
#[test]
fn uncertain_release_blocks_replacement_until_actual_ack_then_explicit_recovery() {
    let d = tempfile::tempdir().unwrap();
    let (mut s, t, b, c) = setup(d.path(), 3);
    s.begin().unwrap();
    t.release.store(false, Ordering::Release);
    assert_eq!(s.cancel(), Err(Error::Reset));
    assert!(b.occupied());
    assert_eq!(c.load(Ordering::SeqCst), 0);
    let other = Arc::new(Guard(Arc::new(AtomicUsize::new(0))));
    assert!(matches!(
        Session::with_backend(
            Fake::new(&t, 1),
            output(d.path(), "busy.wav", &t, other),
            None,
            &b
        ),
        Err(Error::Busy)
    ));
    for _ in 0..8 {
        assert_eq!(s.poll(), Err(Error::Reset));
    }
    assert!(b.occupied());
    t.release.store(true, Ordering::Release);
    release(&mut s);
    assert!(!b.occupied());
    assert!(s.take_recording().is_none());
    assert_eq!(c.load(Ordering::SeqCst), 1);
    let guard = Arc::new(Guard(Arc::new(AtomicUsize::new(0))));
    let mut next = Session::with_backend(
        Fake::new(&t, 1),
        output(d.path(), "recovery.wav", &t, guard),
        None,
        &b,
    )
    .unwrap();
    drop(s);
    assert!(b.occupied(), "old released owner must not clear new permit");
    next.begin().unwrap();
    next.stop().unwrap();
    release(&mut next);
    assert!(next.take_recording().is_some());
}
#[test]
fn backend_destructor_body_must_finish_before_output_or_budget_release() {
    let d = tempfile::tempdir().unwrap();
    let (mut s, t, b, c) = setup(d.path(), 1);
    let (entered, ack) = mpsc::sync_channel(1);
    let (release_gate, wait) = mpsc::sync_channel(1);
    *t.block_drop.lock().unwrap() = Some((entered, wait));
    s.begin().unwrap();
    let (done_tx, done_rx) = mpsc::sync_channel(1);
    let h = std::thread::spawn(move || {
        let _ = s.stop();
        done_tx.send(s).unwrap();
    });
    ack.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(b.occupied());
    assert_eq!(c.load(Ordering::SeqCst), 0);
    assert!(!t.calls.lock().unwrap().contains(&"finish"));
    assert!(matches!(done_rx.try_recv(), Err(mpsc::TryRecvError::Empty)));
    release_gate.send(()).unwrap();
    let mut s = done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    h.join().unwrap();
    assert!(!b.occupied());
    drop(s.take_recording().unwrap());
    assert_eq!(c.load(Ordering::SeqCst), 1);
}
#[test]
fn unknown_drop_retains_one_owned_budget_and_never_frees_output() {
    let d = tempfile::tempdir().unwrap();
    let (mut s, t, b, c) = setup(d.path(), 1);
    s.begin().unwrap();
    t.release.store(false, Ordering::Release);
    drop(s);
    assert!(b.occupied());
    assert_eq!(c.load(Ordering::SeqCst), 0);
    assert!(!t.calls.lock().unwrap().contains(&"backend-drop-returning"));
    assert!(d.path().join("owned.wav").exists()); // deliberate bounded test-owner retention, no deletion
}

#[test]
fn actual_partial_pcm_write_is_failed_and_never_finalized() {
    use std::io::{self, Seek, SeekFrom, Write};
    struct ShortFile {
        file: File,
        left: usize,
    }
    impl Write for ShortFile {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            if self.left == 0 {
                return Err(io::Error::other("owned synthetic fault"));
            }
            let n = b.len().min(self.left);
            let written = self.file.write(&b[..n])?;
            self.left -= written;
            Ok(written)
        }
        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }
    impl Seek for ShortFile {
        fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
            self.file.seek(p)
        }
    }
    struct Partial {
        wav: Wav<ShortFile>,
        finished: Arc<AtomicBool>,
    }
    impl Output for Partial {
        fn append(&mut self, s: &[i16]) -> Result<()> {
            self.wav.append(s).map_err(|_| Error::Write)
        }
        fn finish(self, _: Option<String>) -> Result<Recording> {
            self.finished.store(true, Ordering::Release);
            Err(Error::Finalize)
        }
    }
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("partial.wav");
    let f = File::create_new(&path).unwrap();
    let finish = Arc::new(AtomicBool::new(false));
    let t = trace();
    let b = Budget::default();
    let mut s = Session::with_backend(
        Fake::new(&t, 3),
        Partial {
            wav: Wav::new(ShortFile { file: f, left: 45 }).unwrap(),
            finished: finish.clone(),
        },
        None,
        &b,
    )
    .unwrap();
    s.begin().unwrap();
    assert_eq!(s.poll(), Err(Error::Write));
    assert_eq!(s.phase(), Phase::Released);
    assert!(s.take_recording().is_none());
    assert!(!finish.load(Ordering::Acquire));
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 45);
    assert!(!b.occupied());
    assert!(t.calls.lock().unwrap().contains(&"backend-drop-returning"));
}

#[cfg(windows)]
#[test]
#[ignore = "requires explicit authorized Windows device and private NTFS runtime; NEVER run by cross checks"]
fn native_explicit_prepare_start_cancel_and_confirmed_release() {
    use voiceport_windows::storage::Root;
    let d = tempfile::tempdir().unwrap();
    let root = Root::under_owned_parent(d.path()).unwrap();
    let mut s = NativeCapture.prepare(&root, Some("en".into())).unwrap();
    assert_eq!(s.phase(), Phase::Prepared);
    // ONLY a future separately authorized Windows test may activate the device.
    s.begin().unwrap();
    assert_eq!(s.phase(), Phase::Running);
    let _ = s.cancel();
    for _ in 0..MAX_DRAIN_PACKETS {
        if s.phase() == Phase::Released {
            break;
        }
        let _ = s.poll();
    }
    assert_eq!(s.phase(), Phase::Released);
    assert!(s.take_recording().is_none());
    let mut next = NativeCapture.prepare(&root, None).unwrap();
    next.cancel().unwrap();
    assert_eq!(next.phase(), Phase::Released);
}

#[test]
fn reset_ack_without_returned_buffers_cannot_release_or_publish() {
    let d = tempfile::tempdir().unwrap();
    let (mut s, t, b, c) = setup(d.path(), 3);
    s.begin().unwrap();
    t.returned.store(false, Ordering::Release);
    assert_eq!(s.stop(), Ok(Phase::Stopping));
    for _ in 0..8 {
        assert_eq!(s.poll(), Ok(Phase::Stopping));
    }
    assert!(s.take_recording().is_none());
    assert!(b.occupied());
    assert_eq!(c.load(Ordering::SeqCst), 0);
    assert!(!t.calls.lock().unwrap().contains(&"unprepare"));
    t.returned.store(true, Ordering::Release);
    release(&mut s);
    let r = s.take_recording().unwrap();
    assert!(!b.occupied());
    drop(r);
    assert_eq!(c.load(Ordering::SeqCst), 1);
}

#[path = "capture/cleanup.rs"]
mod cleanup;

#[path = "capture/recovery.rs"]
mod recovery;
