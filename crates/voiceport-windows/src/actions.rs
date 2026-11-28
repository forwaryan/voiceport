//! UI action state independent of Win32.
use crate::{
    Controller, Job, Outcome, Phase, Recording, Refused,
    capture::{
        self,
        owner::{CaptureOwner, Factory, Status},
    },
    executor::Executor,
};
#[derive(Clone, Copy)]
#[repr(u32)]
pub enum Action {
    Start = 1,
    Stop = 2,
    Cancel = 3,
    Copy = 4,
    Retry = 5,
    Exit = 6,
    Poll = 7,
    RecoverCapture = 9,
    ToggleCapture = 10,
}
impl Action {
    pub(crate) fn queued(self, bits: u32) -> bool {
        bits & (1 << self as u32) != 0
    }
}

type StartCapture = Box<dyn Fn(&mut CaptureOwner) -> capture::Result<u64> + Send + Sync>;

struct Capture {
    owner: CaptureOwner,
    start: StartCapture,
    generation: Option<u64>,
    stop_requested: bool,
    start_error: Option<capture::Error>,
}
pub trait ManualClipboard {
    fn copy(&mut self, text: &str) -> Result<(), String>;
}
pub struct Actions {
    controller: Controller,
    result: Option<String>,
    pub status: String,
    closing: bool,
    executor: Option<Executor>,
    capture: Option<Capture>,
}
impl Default for Actions {
    fn default() -> Self {
        Self {
            controller: Controller::default(),
            result: None,
            status: "Recording unavailable; automatic paste disabled".into(),
            closing: false,
            executor: None,
            capture: None,
        }
    }
}
impl Actions {
    pub fn with_executor(executor: Executor) -> Self {
        Self {
            executor: Some(executor),
            ..Self::default()
        }
    }
    pub fn with_capture<F: Factory>(
        executor: Executor,
        factory: impl Fn() -> F + Send + Sync + 'static,
    ) -> Self {
        Self {
            capture: Some(Capture {
                owner: CaptureOwner::default(),
                start: Box::new(move |owner| owner.start(factory())),
                generation: None,
                stop_requested: false,
                start_error: None,
            }),
            status: "Ready to record; results are copied manually".into(),
            ..Self::with_executor(executor)
        }
    }
    fn capture_status(&self) -> Status {
        self.capture
            .as_ref()
            .map_or(Status::Idle, |capture| capture.owner.poll())
    }
    fn capture_idle(&self) -> bool {
        matches!(self.capture_status(), Status::Idle | Status::Stopped)
    }
    pub fn can_start(&self) -> bool {
        !self.closing
            && self
                .capture
                .as_ref()
                .is_some_and(|capture| capture.start_error.is_none())
            && self.capture_idle()
            && matches!(self.controller.phase(), Phase::Idle | Phase::Ready)
            && self.executor.as_ref().is_some_and(Executor::idle)
    }
    pub fn can_stop(&self) -> bool {
        !self.closing
            && self.capture.as_ref().is_some_and(|capture| {
                capture.generation.is_some()
                    && !capture.stop_requested
                    && matches!(capture.owner.poll(), Status::Starting | Status::Running)
            })
    }
    pub fn can_recover_capture(&self) -> bool {
        self.capture.as_ref().is_some_and(|capture| {
            (!self.closing && capture.start_error == Some(capture::Error::Start))
                || capture.owner.poll() == Status::RecoveryRequired
        })
    }
    fn start_capture(&mut self) {
        if !self.can_start() {
            return;
        }
        let capture = self.capture.as_mut().unwrap();
        match (capture.start)(&mut capture.owner) {
            Ok(generation) => {
                capture.generation = Some(generation);
                capture.stop_requested = false;
                self.result = None;
                self.status = "Starting microphone".into();
            }
            Err(error) => {
                capture.start_error = (error != capture::Error::Busy).then_some(error);
                self.status = if error == capture::Error::Start {
                    "Capture could not start; recover capture to try again".into()
                } else {
                    error.to_string()
                };
            }
        }
    }
    fn stop_capture(&mut self) {
        if !self.can_stop() {
            return;
        }
        let capture = self.capture.as_mut().unwrap();
        capture.stop_requested = true;
        capture.owner.stop();
        self.status = "Finishing recording".into();
    }
    fn recover_capture(&mut self) {
        if !self.can_recover_capture() {
            return;
        }
        let capture = self.capture.as_mut().unwrap();
        if capture.start_error == Some(capture::Error::Start) {
            if CaptureOwner::recover_infrastructure().is_ok() {
                capture.start_error = None;
                self.status = "Ready to try recording again".into();
            }
        } else if capture.owner.recover().is_ok() {
            self.status = "Recovering microphone release".into();
        }
    }
    fn drain_capture(&mut self, snapshot: Status) {
        let Some(capture) = &mut self.capture else {
            return;
        };
        match snapshot {
            Status::RecoveryRequired => {
                self.status = "Microphone release failed; select Recover capture".into()
            }
            Status::Interrupted | Status::Unavailable => {
                self.status = "Microphone release could not be confirmed".into()
            }
            _ => {}
        }
        let Some(generation) = capture.generation else {
            return;
        };
        match snapshot {
            Status::Running if !capture.stop_requested => self.status = "Recording".into(),
            Status::Stopping | Status::Joining => self.status = "Finishing recording".into(),
            Status::Ready => {
                let Some(answer) = capture.owner.take_result(generation) else {
                    return;
                };
                capture.generation = None;
                match answer {
                    Ok(recording) => match self.admit(recording) {
                        Ok(job) => self.enqueue(job),
                        Err(error) => {
                            self.status = format!("Recording could not be submitted ({error:?})")
                        }
                    },
                    Err(error) => self.status = error.to_string(),
                }
            }
            _ => {}
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
            && self.capture_idle()
            && self.controller.phase() == Phase::Failed
            && self.executor.as_ref().is_some_and(Executor::idle)
    }

    pub fn admit_finalized(&mut self, r: Recording) -> Result<Job, Refused> {
        if !self.closing && !self.capture_idle() {
            return Err(Refused::Busy);
        }
        self.admit(r)
    }
    fn admit(&mut self, r: Recording) -> Result<Job, Refused> {
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
    /// Invalidation precedes result consumption, even if a timer or Stop was queued.
    pub fn dispatch(&mut self, bits: u32, sink: &mut impl ManualClipboard) -> bool {
        if Action::Exit.queued(bits) {
            return self.act(Action::Exit, sink);
        }
        if !self.closing && Action::Cancel.queued(bits) {
            return self.act(Action::Cancel, sink);
        }
        if Action::RecoverCapture.queued(bits) {
            self.act(Action::RecoverCapture, sink);
        }
        if self.closing {
            return self.act(Action::Poll, sink);
        }
        if let Some(action) = [Action::Stop, Action::ToggleCapture, Action::Start]
            .into_iter()
            .find(|action| action.queued(bits))
        {
            self.act(action, sink);
        }
        self.drain();
        for action in [Action::Copy, Action::Retry, Action::Poll] {
            if action.queued(bits) {
                self.act(action, sink);
            }
        }
        false
    }
    pub fn can_copy(&self) -> bool {
        !self.closing && self.controller.phase() == Phase::Ready && self.result.is_some()
    }
    fn invalidate_capture(&mut self) {
        if let Some(capture) = &mut self.capture {
            capture.generation = None;
            if self.closing {
                capture.owner.exit();
            } else {
                capture.owner.cancel();
            }
        }
    }
    fn poll(&mut self) -> bool {
        self.drain();
        let capture_status = self.capture_status();
        self.drain_capture(capture_status);
        let capture_idle = matches!(capture_status, Status::Idle | Status::Stopped);
        if self.controller.phase() == Phase::Stopping || (self.closing && !capture_idle) {
            if capture_idle && self.controller.poll_stopped() {
                self.status = if self.capture.is_some() {
                    "Cancelled; microphone and service stopped"
                } else {
                    "Cancelled; service stopped"
                }
                .into();
            } else if !matches!(
                capture_status,
                Status::RecoveryRequired | Status::Interrupted | Status::Unavailable
            ) {
                self.status = if self.capture.is_some() {
                    "Waiting for microphone and service to stop"
                } else {
                    "Cancelled; waiting for service stop"
                }
                .into();
            }
        }
        self.closing && capture_idle && self.controller.phase() == Phase::Closed
    }
    /// True means confirmed idle shutdown; false keeps the message pump alive.
    pub fn act(&mut self, a: Action, sink: &mut impl ManualClipboard) -> bool {
        match a {
            Action::Start => self.start_capture(),
            Action::Stop => self.stop_capture(),
            Action::ToggleCapture => {
                if self.can_stop() {
                    self.stop_capture();
                } else {
                    self.start_capture();
                }
            }
            Action::RecoverCapture => self.recover_capture(),
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
            Action::Cancel | Action::Exit => {
                self.closing |= matches!(a, Action::Exit);
                self.result = None;
                self.invalidate_capture();
                if self.closing {
                    self.controller.shutdown();
                } else {
                    self.controller.cancel();
                }
                return self.poll();
            }
            Action::Poll => return self.poll(),
        }
        false
    }
}
