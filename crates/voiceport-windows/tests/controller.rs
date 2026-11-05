use std::{fs::File, io::Cursor};
use voiceport_windows::{
    Controller, Outcome, Phase, Recording, Refused,
    wav::{MAX_SAMPLES, Wav},
};
fn rec(dir: &std::path::Path, name: &str) -> Recording {
    let path = dir.join(name);
    let file = File::create_new(&path).unwrap();
    Recording {
        file,
        path,
        language: Some("zh".into()),
    }
}
#[test]
fn failed_retry_preserves_file_language_and_manual_result() {
    let d = tempfile::tempdir().unwrap();
    let mut c = Controller::default();
    let j = c.start(rec(d.path(), "one.wav")).unwrap();
    let path = j.path().to_owned();
    assert_eq!(c.complete(j, Err(())), Outcome::Failed);
    assert!(c.retains_recording());
    let j = c.retry().unwrap();
    assert_eq!(j.path(), path);
    assert_eq!(j.language(), Some("zh"));
    assert_eq!(
        c.complete(j, Ok("raw\ntext".into())),
        Outcome::Result {
            text: "raw\ntext".into(),
            automatic: false
        }
    );
    assert!(path.exists());
    assert!(!c.retains_recording());
    assert!(matches!(c.retry(), Err(Refused::NotRetryable)));
}
#[test]
fn cancellation_invalidates_then_blocks_until_worker_dropped() {
    let d = tempfile::tempdir().unwrap();
    let mut c = Controller::default();
    let j = c.start(rec(d.path(), "old.wav")).unwrap();
    c.cancel();
    assert!(j.cancelled());
    assert!(!c.poll_stopped());
    assert!(c.retains_recording());
    assert!(matches!(
        c.start(rec(d.path(), "blocked.wav")),
        Err(Refused::Busy)
    ));
    assert_eq!(c.complete(j, Ok("late".into())), Outcome::Ignored);
    assert!(c.poll_stopped());
    assert_eq!(c.phase(), Phase::Idle);
    let next = c.start(rec(d.path(), "next.wav")).unwrap();
    assert_eq!(
        c.complete(next, Ok("new".into())),
        Outcome::Result {
            text: "new".into(),
            automatic: false
        }
    );
    assert!(d.path().join("old.wav").exists());
}
#[test]
fn shutdown_unknown_stop_retains_lease_and_refuses_new_work() {
    let d = tempfile::tempdir().unwrap();
    let mut c = Controller::default();
    let j = c.start(rec(d.path(), "one.wav")).unwrap();
    assert!(!c.shutdown());
    assert!(j.cancelled());
    assert!(c.retains_recording());
    assert!(matches!(
        c.start(rec(d.path(), "two.wav")),
        Err(Refused::Closed)
    ));
    drop(j);
    assert!(c.poll_stopped());
    assert_eq!(c.phase(), Phase::Closed);
}
#[test]
fn foreign_controller_ticket_cannot_complete_same_counter() {
    let d = tempfile::tempdir().unwrap();
    let mut a = Controller::default();
    let mut b = Controller::default();
    let first = a.start(rec(d.path(), "a.wav")).unwrap();
    let second = b.start(rec(d.path(), "b.wav")).unwrap();
    assert_eq!(b.complete(first, Ok("foreign".into())), Outcome::Ignored);
    assert_eq!(b.phase(), Phase::Working);
    assert_eq!(
        b.complete(second, Ok("own".into())),
        Outcome::Result {
            text: "own".into(),
            automatic: false
        }
    );
}
#[test]
fn wav_exact_samples_duration_and_nonempty_destination() {
    assert!(Wav::new(Cursor::new(vec![1; 50])).is_err());
    assert!(Wav::new(Cursor::new(Vec::new())).unwrap().finish().is_err());
    let mut w = Wav::new(Cursor::new(Vec::new())).unwrap();
    w.append(&[-32768, 0, 32767]).unwrap();
    let b = w.finish().unwrap().into_inner();
    assert_eq!(&b[..12], b"RIFF\x2a\0\0\0WAVE");
    assert_eq!(&b[44..], &[0, 128, 0, 0, 255, 127]);
    let mut w = Wav::new(Cursor::new(Vec::new())).unwrap();
    w.append(&vec![0; MAX_SAMPLES]).unwrap();
    assert!(w.append(&[0]).is_err());
    assert_eq!(w.finish().unwrap().into_inner().len(), 44 + MAX_SAMPLES * 2);
}
#[cfg(feature = "service")]
#[tokio::test]
async fn actual_shared_client_missing_auth_and_precancel_are_offline() {
    let d = tempfile::tempdir().unwrap();
    let mut c = Controller::default();
    let j = c.start(rec(d.path(), "file.wav")).unwrap();
    assert_eq!(
        j.transcribe(d.path().join("missing-auth")).await,
        Err(voiceport_core::Error::SignInRequired)
    );
    c.cancel();
    assert_eq!(
        j.transcribe(d.path().join("missing-auth")).await,
        Err(voiceport_core::Error::Cancelled)
    );
    assert_eq!(c.complete(j, Err(())), Outcome::Ignored);
    assert!(c.poll_stopped());
}
#[test]
fn interrupted_pcm_write_cannot_finalize_as_success() {
    struct Broken(std::io::Cursor<Vec<u8>>);
    impl std::io::Seek for Broken {
        fn seek(&mut self, p: std::io::SeekFrom) -> std::io::Result<u64> {
            self.0.seek(p)
        }
    }
    impl std::io::Write for Broken {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            if self.0.position() >= 44 {
                return Err(std::io::Error::other("synthetic device write failure"));
            }
            self.0.write(b)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut w = Wav::new(Broken(Cursor::new(Vec::new()))).unwrap();
    assert!(w.append(&[1, 2]).is_err());
    assert!(w.finish().is_err());
}
