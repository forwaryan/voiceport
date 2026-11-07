//! Session identity and UI-independent state transitions. No platform I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Permissions,
    Recording,
    Transcribing,
    Delivering,
    Stopping,
    Ready,
    Failed,
}

#[derive(Debug)]
pub struct Lifecycle {
    pub phase: Phase,
    generation: u64,
    pub retry_available: bool,
}
impl Default for Lifecycle {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            generation: 0,
            retry_available: false,
        }
    }
}
impl Lifecycle {
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn accepts(&self, generation: u64) -> bool {
        self.generation == generation
    }
    pub fn can_start(&self) -> bool {
        matches!(self.phase, Phase::Idle | Phase::Ready | Phase::Failed)
    }
    pub fn begin(&mut self) -> Option<u64> {
        if !self.can_start() {
            return None;
        }
        self.generation = self.generation.checked_add(1)?;
        self.phase = Phase::Permissions;
        self.retry_available = false;
        Some(self.generation)
    }
    pub fn retry(&mut self) -> Option<u64> {
        if self.phase != Phase::Failed || !self.retry_available {
            return None;
        }
        self.generation = self.generation.checked_add(1)?;
        self.phase = Phase::Transcribing;
        Some(self.generation)
    }
    pub fn cancel(&mut self, worker_active: bool) {
        // Exhausting identities disables future starts, without accepting old callbacks.
        self.generation = self.generation.saturating_add(1);
        self.phase = if worker_active {
            Phase::Stopping
        } else {
            Phase::Idle
        };
        self.retry_available = false;
    }
    pub fn permissions(&mut self, generation: u64, microphone: bool) -> bool {
        if !self.accepts(generation) || self.phase != Phase::Permissions {
            return false;
        }
        self.phase = if microphone {
            Phase::Recording
        } else {
            Phase::Failed
        };
        microphone
    }
    pub fn transcribe(&mut self) -> bool {
        if self.phase != Phase::Recording {
            return false;
        }
        self.phase = Phase::Transcribing;
        true
    }
    /// The worker owns no further job after its result sender disconnects.
    /// Retry keeps the existing recording but always receives a fresh identity.
    pub fn worker_disconnected(&mut self, has_recording: bool) -> bool {
        match self.phase {
            Phase::Stopping => {
                self.phase = Phase::Idle;
                self.retry_available = false;
            }
            Phase::Transcribing => {
                self.phase = Phase::Failed;
                self.retry_available = has_recording;
            }
            _ => return false,
        }
        true
    }
    pub fn complete(&mut self, generation: u64, success: bool, has_recording: bool) -> bool {
        if !self.accepts(generation) || self.phase != Phase::Transcribing {
            return false;
        }
        self.phase = if success { Phase::Ready } else { Phase::Failed };
        self.retry_available = !success && has_recording;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancelled_permission_cannot_start_a_new_recording() {
        let mut state = Lifecycle::default();
        let old = state.begin().unwrap();
        state.cancel(false);
        let current = state.begin().unwrap();
        assert!(!state.permissions(old, true));
        assert_eq!(state.phase, Phase::Permissions);
        assert!(state.permissions(current, true));
    }
    #[test]
    fn cancellation_holds_busy_until_worker_releases_audio() {
        let mut state = Lifecycle::default();
        let id = state.begin().unwrap();
        assert!(state.permissions(id, true));
        assert!(state.transcribe());
        state.cancel(true);
        assert!(state.begin().is_none());
        assert!(!state.complete(id, true, true));
        assert_eq!(state.phase, Phase::Stopping);
    }
    #[test]
    fn retry_has_a_fresh_identity_and_rejects_old_completion() {
        let mut state = Lifecycle::default();
        let id = state.begin().unwrap();
        state.permissions(id, true);
        state.transcribe();
        assert!(state.complete(id, false, true));
        let retry = state.retry().unwrap();
        assert_ne!(id, retry);
        assert!(!state.complete(id, true, true));
        assert!(state.complete(retry, true, false));
        assert!(!state.retry_available);
    }
    #[test]
    fn denial_and_missing_audio_are_not_retryable() {
        let mut state = Lifecycle::default();
        let id = state.begin().unwrap();
        assert!(!state.permissions(id, false));
        assert!(state.retry().is_none());
        let id = state.begin().unwrap();
        state.permissions(id, true);
        state.transcribe();
        assert!(state.complete(id, false, false));
        assert!(state.retry().is_none());
    }
    #[test]
    fn disconnected_worker_unblocks_cancel_and_preserves_retry_identity() {
        let mut state = Lifecycle::default();
        let original = state.begin().unwrap();
        state.permissions(original, true);
        state.transcribe();
        assert!(state.worker_disconnected(true));
        assert_eq!(state.phase, Phase::Failed);
        let retry = state.retry().unwrap();
        assert_ne!(original, retry);
        assert!(!state.complete(original, true, true));
        state.cancel(true);
        assert!(state.worker_disconnected(true));
        assert_eq!(state.phase, Phase::Idle);
        assert!(!state.retry_available);
        assert!(state.begin().is_some());
    }

    #[test]
    fn disconnect_without_recording_is_not_retryable_and_does_not_change_idle() {
        let mut state = Lifecycle::default();
        assert!(!state.worker_disconnected(true));
        let generation = state.begin().unwrap();
        state.permissions(generation, true);
        state.transcribe();
        assert!(state.worker_disconnected(false));
        assert!(state.retry().is_none());
    }

    #[test]
    fn exhausted_generation_does_not_reuse_an_identity() {
        let mut state = Lifecycle {
            generation: u64::MAX,
            ..Lifecycle::default()
        };
        assert!(state.begin().is_none());
        assert_eq!(state.phase, Phase::Idle);
    }
}
