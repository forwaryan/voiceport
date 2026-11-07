use std::{
    path::Path,
    sync::{
        Arc, Weak,
        atomic::{AtomicUsize, Ordering},
    },
};
use voiceport_windows::{
    Controller, Outcome, Recording,
    storage::{Storage, validate_local_path},
};
#[test]
fn bounded_local_path_policy_and_inert_constructor() {
    for p in ["C:\\Users\\test\\AppData\\Local", "D:\\owned"] {
        validate_local_path(Path::new(p)).unwrap();
    }
    for p in [
        "relative",
        "C:relative",
        "\\\\server\\share",
        "\\\\?\\C:\\owned",
        "C:\\a\\..\\b",
        "C:\\a:stream",
        "C:\\a.\\b",
        "C:\\a ",
        "C:\\NUL",
        "C:\\con.txt",
        "C:\\lpt1",
        "C:\\COM¹.txt",
        "C:\\LPT²",
        "C:\\CONIN$",
        "C:\\x\\",
        "C:\\a/b",
    ] {
        assert!(validate_local_path(Path::new(p)).is_err(), "{p}");
    }
    assert!(validate_local_path(Path::new(&format!("C:\\{}", "x".repeat(180)))).is_err());
    let owned = tempfile::tempdir().unwrap();
    let storage = Storage;
    assert_eq!(std::fs::read_dir(owned.path()).unwrap().count(), 0);
    #[cfg(not(windows))]
    assert!(storage.open_or_create().is_err());
    let _ = storage;
    assert_eq!(std::fs::read_dir(owned.path()).unwrap().count(), 0);
}
struct Guard(Arc<AtomicUsize>);
impl Drop for Guard {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn recording(d: &Path, n: &str, c: &Arc<AtomicUsize>) -> (Recording, Weak<Guard>) {
    let path = d.join(n);
    let guard = Arc::new(Guard(c.clone()));
    let weak = Arc::downgrade(&guard);
    (
        Recording::caller_owned_with_guard(
            std::fs::File::create_new(&path).unwrap(),
            path,
            Some("fr".into()),
            guard,
        ),
        weak,
    )
}
#[test]
fn lifetime_guard_survives_failure_retry_and_cancel() {
    let d = tempfile::tempdir().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let mut c = Controller::default();
    let (r, weak) = recording(d.path(), "one.wav", &count);
    let j = c.start(r).unwrap();
    assert_eq!(c.complete(j, Err(())), Outcome::Failed);
    assert!(weak.upgrade().is_some());
    let j = c.retry().unwrap();
    assert_eq!(j.language(), Some("fr"));
    c.cancel();
    assert!(!c.poll_stopped());
    assert!(weak.upgrade().is_some());
    drop(j);
    assert!(c.poll_stopped());
    assert!(weak.upgrade().is_none());
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(d.path().join("one.wav").exists());
}
#[test]
fn lifetime_guard_remains_through_actual_worker_tls_and_abnormal_owner_drop() {
    use std::{
        cell::RefCell,
        sync::{Mutex, mpsc},
        time::{Duration, Instant},
    };
    use voiceport_windows::{
        Job,
        actions::{Actions, ManualClipboard},
        executor::{Executor, Service, ServiceFailure},
    };
    struct Hold(mpsc::SyncSender<()>, mpsc::Receiver<()>);
    impl Drop for Hold {
        fn drop(&mut self) {
            self.0.send(()).unwrap();
            self.1.recv_timeout(Duration::from_secs(5)).unwrap();
        }
    }
    thread_local! {static TAIL:RefCell<Option<Hold>>=const{RefCell::new(None)};}
    struct Held(Mutex<Option<Hold>>);
    impl Service for Held {
        fn run(&self, _: &Job) -> Result<String, ServiceFailure> {
            TAIL.with(|x| *x.borrow_mut() = self.0.lock().unwrap().take());
            Ok("synthetic".into())
        }
    }
    struct Sink;
    impl ManualClipboard for Sink {
        fn copy(&mut self, _: &str) -> Result<(), String> {
            panic!("no copy expected")
        }
    }
    let d = tempfile::tempdir().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let (tx, rx) = mpsc::sync_channel(1);
    let (release, wait) = mpsc::sync_channel(1);
    let (r, weak) = recording(d.path(), "tls.wav", &count);
    let mut ui = Actions::with_executor(Executor::new(Arc::new(Held(Mutex::new(Some(Hold(
        tx, wait,
    )))))));
    ui.submit_finalized(r).unwrap();
    rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut sink = Sink;
    assert!(!ui.act(voiceport_windows::actions::Action::Poll, &mut sink));
    drop(ui);
    assert!(weak.upgrade().is_some());
    assert_eq!(count.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    // Arc's strong count reaches zero before Guard::drop finishes. A failed
    // upgrade alone is not the destructor-completion witness checked below.
    while weak.upgrade().is_some() || count.load(Ordering::SeqCst) == 0 {
        assert!(
            Instant::now() < end,
            "guard disposal after worker TLS release did not complete; drops={}",
            count.load(Ordering::SeqCst)
        );
        std::thread::yield_now();
    }
    assert_eq!(count.load(Ordering::SeqCst), 1);
}
#[cfg(windows)]
#[test]
#[ignore = "requires separately authorized actual Windows NTFS runtime; not run by cross check"]
fn native_private_root_file_sharing_and_directory_guard() {
    use std::io::Read;
    use voiceport_windows::storage::Root;
    let d = tempfile::tempdir().unwrap();
    let root = Root::under_owned_parent(d.path()).unwrap();
    let mut p = root.create().unwrap();
    p.append(&[-32768, 0, 32767]).unwrap();
    let r = p.finalize(Some("zh".into())).unwrap();
    let path = r.path().to_path_buf();
    drop(root);
    assert!(
        std::fs::rename(
            d.path().join("VoicePort-recordings-v1"),
            d.path().join("renamed")
        )
        .is_err()
    );
    assert!(std::fs::OpenOptions::new().write(true).open(&path).is_err());
    let mut bytes = vec![];
    std::fs::File::open(&path)
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    assert_eq!(&bytes[..4], b"RIFF");
    assert_eq!(&bytes[44..], &[0, 128, 0, 0, 255, 127]);
    let mut c = Controller::default();
    let j = c.start(r).unwrap();
    assert_eq!(c.complete(j, Err(())), Outcome::Failed);
    let j = c.retry().unwrap();
    assert_eq!(j.language(), Some("zh"));
    c.cancel();
    assert!(!c.poll_stopped());
    drop(j);
    assert!(c.poll_stopped());
    std::fs::rename(
        d.path().join("VoicePort-recordings-v1"),
        d.path().join("renamed"),
    )
    .unwrap();
}

#[test]
fn failure_messages_never_include_paths_or_upstream_details() {
    use voiceport_windows::storage::Error;
    for e in [
        Error::Unsupported,
        Error::Unavailable,
        Error::UnsafeObject,
        Error::Collision,
        Error::InvalidRecording,
    ] {
        let text = e.to_string();
        assert!(text.len() < 80 && !text.contains(['\\', '/', ':']));
    }
}
#[test]
fn owned_guard_released_on_success_or_idle_owner_drop_without_deletion() {
    for success in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let count = Arc::new(AtomicUsize::new(0));
        let (r, weak) = recording(d.path(), "keep.wav", &count);
        let mut c = Controller::default();
        let j = c.start(r).unwrap();
        if success {
            assert_eq!(
                c.complete(j, Ok("synthetic".into())),
                Outcome::Result {
                    text: "synthetic".into(),
                    automatic: false
                }
            );
        } else {
            assert_eq!(c.complete(j, Err(())), Outcome::Failed);
            assert!(weak.upgrade().is_some());
            drop(c);
        }
        assert!(weak.upgrade().is_none());
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(d.path().join("keep.wav").exists());
    }
}
#[cfg(windows)]
#[test]
#[ignore = "native NTFS security/share integration requires actual Windows; compile-only here"]
fn native_rejects_inherited_root_tampered_marker_hardlinks_and_partial_wav() {
    use voiceport_windows::{storage::Root, wav::MAX_SAMPLES};
    // Never seize/repair an arbitrary existing directory or create its marker.
    let unsafe_parent = tempfile::tempdir().unwrap();
    let unsafe_root = unsafe_parent.path().join("VoicePort-recordings-v1");
    std::fs::create_dir(&unsafe_root).unwrap();
    assert!(Root::under_owned_parent(unsafe_parent.path()).is_err());
    assert!(!unsafe_root.join(".voiceport-owner-v1").exists());

    let d = tempfile::tempdir().unwrap();
    let root = Root::under_owned_parent(d.path()).unwrap();
    assert!(root.create().unwrap().finalize(None).is_err());
    let mut p = root.create().unwrap();
    assert!(p.append(&vec![0; MAX_SAMPLES + 1]).is_err());
    assert!(p.append(&[1]).is_err());
    assert!(p.finalize(None).is_err());
    drop(root);
    // Exact owner ACL plus malformed marker must not be adopted.
    let marker = d.path().join("VoicePort-recordings-v1/.voiceport-owner-v1");
    let original = std::fs::read(&marker).unwrap();
    std::fs::write(&marker, b"wrong-owner-marker").unwrap();
    assert!(Root::under_owned_parent(d.path()).is_err());
    std::fs::write(&marker, original).unwrap();
    // Positive existing private root: no marker rewrite or directory scan needed.
    drop(Root::under_owned_parent(d.path()).unwrap());
    std::fs::hard_link(&marker, d.path().join("own-marker-alias")).unwrap();
    assert!(Root::under_owned_parent(d.path()).is_err());
}
