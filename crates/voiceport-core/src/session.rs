//! Process-local generations, independent of caller request identifiers.
use crate::{CancellationToken, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
static GENERATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
pub struct Job {
    pub generation: u64,
    pub request_id: String,
    pub audio_path: PathBuf,
    pub language: Option<String>,
    pub is_retry: bool,
    pub cancellation: CancellationToken,
}
impl Job {
    pub fn session_id(&self) -> String {
        format!("s{}", self.generation)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Transcribing,
    Ready,
    Failed,
}
impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Transcribing => "transcribing",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }
}
#[derive(Default)]
pub struct Sessions {
    current: Option<Job>,
    phase: Option<Phase>,
    stopped: bool,
}
impl Sessions {
    pub fn phase(&self) -> Phase {
        self.phase.unwrap_or(Phase::Idle)
    }
    pub fn current(&self) -> Option<&Job> {
        self.current.as_ref()
    }
    pub fn start(
        &mut self,
        request_id: String,
        audio_path: PathBuf,
        language: Option<String>,
        is_retry: bool,
    ) -> Option<Job> {
        if self.stopped {
            return None;
        }
        let next = GENERATION
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .ok()?
            .checked_add(1)?;
        self.invalidate();
        let job = Job {
            generation: next,
            request_id,
            audio_path,
            language,
            is_retry,
            cancellation: CancellationToken::new(),
        };
        self.current = Some(job.clone());
        self.phase = Some(Phase::Transcribing);
        Some(job)
    }
    pub fn retry(&mut self, id: &str, request_id: String) -> Option<Job> {
        let job = self.current.as_ref()?;
        if self.phase() != Phase::Failed || job.session_id() != id {
            return None;
        }
        self.start(
            request_id,
            job.audio_path.clone(),
            job.language.clone(),
            true,
        )
    }
    pub fn complete(&mut self, generation: u64, result: &Result<String>) -> bool {
        if self.stopped
            || self.phase() != Phase::Transcribing
            || self
                .current
                .as_ref()
                .is_none_or(|j| j.generation != generation)
        {
            return false;
        }
        self.phase = Some(if result.is_ok() {
            Phase::Ready
        } else {
            Phase::Failed
        });
        // Only failures retain a retry path. No result/history is stored.
        if result.is_ok() {
            let j = self.current.as_mut().unwrap();
            j.audio_path = PathBuf::new();
            j.language = None;
        }
        true
    }
    pub fn cancel(&mut self, id: &str) -> bool {
        if self.current.as_ref().is_none_or(|j| j.session_id() != id) {
            return false;
        }
        self.invalidate();
        true
    }
    fn invalidate(&mut self) {
        let old = self.current.take();
        self.phase = Some(Phase::Idle);
        if let Some(old) = old {
            old.cancellation.cancel();
        }
    }
    pub fn shutdown(&mut self) {
        self.stopped = true;
        self.invalidate();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;
    #[test]
    fn invalidation_rejects_late_success_error_and_service_cancellation() {
        for result in [Ok("old".into()), Err(Error::Network), Err(Error::Cancelled)] {
            let mut s = Sessions::default();
            let a = s.start("same".into(), "a".into(), None, false).unwrap();
            let b = s.start("same".into(), "b".into(), None, false).unwrap();
            assert!(a.cancellation.is_cancelled());
            assert_ne!(a.generation, b.generation);
            assert!(!s.complete(a.generation, &result));
            assert_eq!(s.phase(), Phase::Transcribing);
            assert!(s.cancel(&b.session_id()));
            assert!(!s.complete(b.generation, &result));
            s.shutdown();
            assert!(s.start("x".into(), "x".into(), None, false).is_none());
        }
    }
    #[test]
    fn only_failed_current_session_retries_same_input_with_new_generation() {
        let mut s = Sessions::default();
        let a = s
            .start("a".into(), "audio".into(), Some("zh".into()), false)
            .unwrap();
        assert!(s.retry(&a.session_id(), "r".into()).is_none());
        assert!(s.complete(a.generation, &Err(Error::Cancelled)));
        let b = s.retry(&a.session_id(), "r".into()).unwrap();
        assert!(b.is_retry);
        assert_ne!(a.generation, b.generation);
        assert_eq!(b.audio_path, a.audio_path);
        assert_eq!(b.language, a.language);
        assert!(!s.cancel(&a.session_id()));
        assert!(s.complete(b.generation, &Ok("text".into())));
        assert!(s.retry(&b.session_id(), "r2".into()).is_none());
    }
}

#[cfg(test)]
mod process_identity_test {
    use super::*;
    #[test]
    fn multiple_engines_in_one_process_never_reuse_generation() {
        let mut a = Sessions::default();
        let mut b = Sessions::default();
        let first = a.start("x".into(), "x".into(), None, false).unwrap();
        let second = b.start("x".into(), "x".into(), None, false).unwrap();
        assert_ne!(first.generation, second.generation);
    }
}
