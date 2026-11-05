//! Incremental Windows adapter. No native capture, storage cleanup or automatic
//! delivery is enabled. Direct core integration avoids a second host parser.
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
};
use tokio_util::sync::CancellationToken;
#[cfg(windows)]
pub mod hotkey;
pub mod wav;

/// Already finalized recording supplied by a platform owner. The caller must
/// establish native ACL/ownership/sharing before admission. This crate does not
/// infer ownership from a path. Native storage finalization retains its handle
/// guards here. Fields are private so that a finalized native path/File pair
/// cannot be replaced while retaining an unrelated guard.
pub struct Recording {
    _file: File,
    path: PathBuf,
    language: Option<String>,
    _guards: Option<Arc<dyn Send + Sync>>,
}
impl Recording {
    /// Caller establishes path/File ownership. This is NOT a native storage lease.
    pub fn caller_owned(file: File, path: PathBuf, language: Option<String>) -> Self {
        Self {
            _file: file,
            path,
            language,
            _guards: None,
        }
    }
    /// Retain caller-supplied ownership through every Job/controller reference.
    /// This adds lifetime only; it does not validate ACLs, sharing or path identity.
    pub fn caller_owned_with_guard(
        file: File,
        path: PathBuf,
        language: Option<String>,
        guards: Arc<dyn Send + Sync>,
    ) -> Self {
        Self {
            _file: file,
            path,
            language,
            _guards: Some(guards),
        }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }
}
pub mod storage;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Working,
    Failed,
    Ready,
    Stopping,
    Closed,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    Busy,
    Closed,
    Exhausted,
    NotRetryable,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Result { text: String, automatic: bool },
    Failed,
    Ignored,
}
/// Non-cloneable work ticket owns a recording reference until the service
/// future and ticket are dropped. An uncooperative worker blocks replacement.
pub struct Job {
    id: u64,
    recording: Arc<Recording>,
    cancellation: CancellationToken,
    _alive: Arc<()>,
    manual: bool,
}
impl Job {
    pub fn path(&self) -> &Path {
        &self.recording.path
    }
    pub fn language(&self) -> Option<&str> {
        self.recording.language.as_deref()
    }
    pub fn cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }
    #[cfg(feature = "service")]
    pub async fn transcribe(&self, auth: PathBuf) -> voiceport_core::Result<String> {
        // Only this explicit call constructs the shared client; never on start,
        // help, smoke or controller construction. Core owns auth/network policy.
        voiceport_core::TranscriptionClient::new(auth)?
            .transcribe(self.path(), self.language(), &self.cancellation)
            .await
    }
}
pub struct Controller {
    phase: Phase,
    generation: u64,
    current: Option<u64>,
    recording: Option<Arc<Recording>>,
    worker: Weak<()>,
    cancel: Option<CancellationToken>,
    closing: bool,
}
impl Default for Controller {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            generation: 0,
            current: None,
            recording: None,
            worker: Weak::new(),
            cancel: None,
            closing: false,
        }
    }
}
impl Controller {
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn retains_recording(&self) -> bool {
        self.recording.is_some()
    }
    pub fn start(&mut self, recording: Recording) -> Result<Job, Refused> {
        if self.closing {
            return Err(Refused::Closed);
        }
        if self.worker.upgrade().is_some() || self.phase == Phase::Stopping {
            return Err(Refused::Busy);
        }
        self.generation.checked_add(1).ok_or(Refused::Exhausted)?;
        self.recording = Some(Arc::new(recording));
        self.issue(false)
    }
    fn issue(&mut self, manual: bool) -> Result<Job, Refused> {
        self.generation = self.generation.checked_add(1).ok_or(Refused::Exhausted)?;
        let alive = Arc::new(());
        let cancellation = CancellationToken::new();
        self.worker = Arc::downgrade(&alive);
        self.cancel = Some(cancellation.clone());
        self.current = Some(self.generation);
        self.phase = Phase::Working;
        Ok(Job {
            id: self.generation,
            recording: self.recording.as_ref().unwrap().clone(),
            cancellation,
            _alive: alive,
            manual,
        })
    }
    pub fn retry(&mut self) -> Result<Job, Refused> {
        if self.closing {
            return Err(Refused::Closed);
        }
        if self.phase != Phase::Failed || self.worker.upgrade().is_some() {
            return Err(Refused::NotRetryable);
        }
        self.issue(true)
    }
    /// Consuming the unique ticket proves no borrowed service future can still
    /// use it. Old completions cannot supply text or release a new recording.
    pub fn complete(&mut self, job: Job, result: Result<String, ()>) -> Outcome {
        let valid = self.worker.ptr_eq(&Arc::downgrade(&job._alive))
            && self.current == Some(job.id)
            && self.phase == Phase::Working
            && !self.closing;
        let manual = job.manual;
        drop(job);
        if !valid {
            return Outcome::Ignored;
        }
        self.current = None;
        self.cancel = None;
        match result {
            Ok(text) => {
                self.phase = Phase::Ready;
                self.recording = None;
                Outcome::Result {
                    text,
                    automatic: automatic_delivery_supported() && !manual,
                }
            }
            Err(()) => {
                self.phase = Phase::Failed;
                Outcome::Failed
            }
        }
    }
    pub fn cancel(&mut self) {
        self.current = None; // Invalidate before signalling cancellation.
        if let Some(c) = self.cancel.take() {
            c.cancel();
        }
        self.phase = Phase::Stopping;
    }
    /// Unknown stop retains the lease and blocks new work; no timer can turn it
    /// into a false stop confirmation. No file deletion is implemented here.
    pub fn poll_stopped(&mut self) -> bool {
        if self.worker.upgrade().is_some() {
            return false;
        }
        if self.phase == Phase::Stopping {
            self.recording = None;
            self.phase = if self.closing {
                Phase::Closed
            } else {
                Phase::Idle
            };
        }
        true
    }
    pub fn shutdown(&mut self) -> bool {
        self.closing = true;
        self.cancel();
        self.poll_stopped()
    }
}
impl Drop for Controller {
    fn drop(&mut self) {
        self.current = None;
        if let Some(c) = self.cancel.take() {
            c.cancel();
        }
    }
}
/// Windows UIA/privilege/clipboard preservation is not implemented. No positive
/// synthetic focus evidence can turn on input injection in this adapter.
pub fn automatic_delivery_supported() -> bool {
    false
}
pub mod actions;
#[cfg(windows)]
pub mod shell;

pub mod executor;
