//! UI action state independent of Win32. No native capability is implied.
use crate::{Controller, Job, Outcome, Phase, Recording, Refused};
#[derive(Clone, Copy)]
pub enum Action {
    Start,
    Stop,
    Cancel,
    Copy,
    Retry,
    Exit,
    Poll,
}
pub trait ManualClipboard {
    fn copy(&mut self, text: &str) -> Result<(), String>;
}
pub struct Actions {
    controller: Controller,
    result: Option<String>,
    pub status: String,
    closing: bool,
    executor: Option<crate::executor::Executor>,
}
impl Default for Actions {
    fn default() -> Self {
        Self {
            controller: Controller::default(),
            result: None,
            status: "Recording/storage unavailable; automatic paste disabled".into(),
            closing: false,
            executor: None,
        }
    }
}
impl Actions {
    pub fn with_executor(executor: crate::executor::Executor) -> Self {
        Self {
            executor: Some(executor),
            ..Self::default()
        }
    }
    pub fn submit_finalized(&mut self, r: Recording) -> Result<(), Refused> {
        if self.executor.as_ref().is_none_or(|e| !e.idle()) {
            return Err(Refused::Busy);
        }
        let job = self.admit_finalized(r)?;
        self.enqueue(job);
        Ok(())
    }
    fn enqueue(&mut self, job: Job) {
        if let Some(executor) = &mut self.executor {
            if let Err(job) = executor.submit(job) {
                self.complete(job, Err(()));
            }
        } else {
            self.complete(job, Err(()));
        }
    }
    fn drain(&mut self) {
        if let Some(done) = self.executor.as_mut().and_then(|e| e.poll()) {
            self.complete(done.0, done.1.map_err(|_| ()));
        }
    }
    pub fn can_retry(&self) -> bool {
        !self.closing
            && self.controller.phase() == Phase::Failed
            && self.executor.as_ref().is_some_and(|e| e.idle())
    }

    pub fn admit_finalized(&mut self, r: Recording) -> Result<Job, Refused> {
        let job = self.controller.start(r)?;
        self.result = None;
        self.status = "Transcribing".into();
        Ok(job)
    }
    pub fn complete(&mut self, job: Job, result: Result<String, ()>) {
        match self.controller.complete(job, result) {
            Outcome::Result { text, .. } => {
                if text.len() <= 1_000_000 {
                    self.result = Some(text);
                    self.status = "Result available for manual copy".into();
                } else {
                    self.result = None;
                    self.status = "Result too large".into();
                }
            }
            Outcome::Failed => {
                self.result = None;
                self.status = "Transcription failed; recording retained".into();
            }
            Outcome::Ignored => {}
        }
    }
    /// Exit/cancel take priority over coalesced copy or start notifications.
    pub fn dispatch(&mut self, bits: u32, sink: &mut impl ManualClipboard) -> bool {
        if bits & (1 << 6) != 0 {
            return self.act(Action::Exit, sink);
        }
        if self.closing {
            return self.act(Action::Poll, sink);
        }
        if bits & (1 << 3) != 0 {
            self.act(Action::Cancel, sink);
            return false;
        }
        self.drain();
        for (id, action) in [
            (1, Action::Start),
            (2, Action::Stop),
            (4, Action::Copy),
            (5, Action::Retry),
            (7, Action::Poll),
        ] {
            if bits & (1 << id) != 0 && self.act(action, sink) {
                return true;
            }
        }
        false
    }
    pub fn can_copy(&self) -> bool {
        !self.closing && self.controller.phase() == Phase::Ready && self.result.is_some()
    }
    /// True means confirmed idle shutdown; false keeps the message pump alive.
    pub fn act(&mut self, a: Action, sink: &mut impl ManualClipboard) -> bool {
        match a {
            Action::Start | Action::Stop => {
                self.status = "Native recording and secure storage are not implemented".into()
            }
            Action::Retry => {
                if self.can_retry()
                    && let Ok(job) = self.controller.retry()
                {
                    self.result = None;
                    self.status = "Retrying".into();
                    self.enqueue(job);
                }
            }
            Action::Copy => {
                if self.can_copy() {
                    self.status = match sink.copy(self.result.as_deref().unwrap()) {
                        Ok(()) => "Copied manually".into(),
                        Err(_) => "Clipboard unavailable; result retained".into(),
                    };
                }
            }
            Action::Cancel => {
                self.result = None;
                self.controller.cancel();
                let stopped = self.controller.poll_stopped();
                self.status = if stopped {
                    "Cancelled; service stopped"
                } else {
                    "Cancelled; waiting for service stop"
                }
                .into();
            }
            Action::Exit => {
                self.closing = true;
                self.result = None;
                self.controller.shutdown();
                self.drain();
                return self.controller.poll_stopped();
            }
            Action::Poll => {
                self.drain();
                let was_stopping = self.controller.phase() == Phase::Stopping;
                let stopped = self.controller.poll_stopped();
                if self.closing {
                    return stopped;
                }
                if was_stopping && stopped {
                    self.status = "Cancelled; service stopped".into();
                }
            }
        }
        false
    }
}
