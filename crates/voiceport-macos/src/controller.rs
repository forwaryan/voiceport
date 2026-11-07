//! Main-thread orchestration; worker completion follows release of its recording lease.
use crate::{
    audio::{AudioEvent, Recorder},
    delivery::Delivery,
    lifecycle::{Lifecycle, Phase},
    storage::Recording,
};
use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender, SyncSender, TryRecvError, TrySendError},
    },
    time::Instant,
};
use voiceport_core::{CancellationToken, TranscriptionClient};

struct Job {
    generation: u64,
    recording: Arc<Recording>,
    language: String,
    proxy: Option<String>,
    cancel: CancellationToken,
}
struct Completion {
    generation: u64,
    result: Result<String, String>,
}
struct ActiveJob {
    generation: u64,
    cancel: CancellationToken,
}
struct Worker {
    jobs: SyncSender<Job>,
    results: Receiver<Completion>,
}
impl Worker {
    fn new() -> Result<Self, String> {
        let (jobs, incoming) = mpsc::sync_channel::<Job>(1);
        let (outgoing, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("voiceport-transcription".into())
            .spawn(move || {
                while let Ok(job) = incoming.recv() {
                    let generation = job.generation;
                    // Unwind on the worker only. Resources, runtime and file clone are all
                    // gone before completion is published; the UI never joins this thread.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|_| "无法启动转写任务。".to_string())?;
                        runtime.block_on(async {
                            let auth =
                                voiceport_core::auth::default_path().map_err(|e| e.to_string())?;
                            let client =
                                TranscriptionClient::new_with_proxy(auth, job.proxy.as_deref())
                                    .map_err(|e| e.to_string())?;
                            client
                                .transcribe(
                                    job.recording.path(),
                                    if job.language.is_empty() {
                                        None
                                    } else {
                                        Some(&job.language)
                                    },
                                    &job.cancel,
                                )
                                .await
                                .map_err(|e| e.to_string())
                        })
                    }))
                    .unwrap_or_else(|_| Err("转写任务异常结束，请手动重试。".into()));
                    drop(job);
                    if outgoing.send(Completion { generation, result }).is_err() {
                        break;
                    }
                }
            })
            .map_err(|_| "无法启动转写工作线程。".to_string())?;
        Ok(Self { jobs, results })
    }
}

pub struct Controller {
    pub state: Lifecycle,
    pub transcript: String,
    pub message: String,
    pub preview_status: String,
    pub delivery_status: String,
    pub automatic: bool,
    pub language: String,
    recorder: Recorder,
    delivery: Delivery,
    events_tx: Sender<(u64, AudioEvent)>,
    events: Receiver<(u64, AudioEvent)>,
    worker: Option<Worker>,
    active: Option<ActiveJob>,
    recording: Option<Arc<Recording>>,
    recorded_language: String,
    started: Option<Instant>,
    allow_delivery: bool,
    delivery_settle: Option<Phase>,
}
impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}
impl Controller {
    pub fn new() -> Self {
        let (events_tx, events) = mpsc::channel();
        Self {
            state: Lifecycle::default(),
            transcript: String::new(),
            message: "聚焦输入框，按快捷键开始听写。".into(),
            preview_status: "本机预览尚未启动".into(),
            delivery_status: String::new(),
            automatic: true,
            language: String::new(),
            recorder: Recorder::new(),
            delivery: Delivery::new(),
            events_tx,
            events,
            worker: None,
            active: None,
            recording: None,
            recorded_language: String::new(),
            started: None,
            allow_delivery: false,
            delivery_settle: None,
        }
    }
    pub fn seconds(&self) -> u64 {
        self.started.map(|s| s.elapsed().as_secs()).unwrap_or(0)
    }
    pub fn busy(&self) -> bool {
        !self.state.can_start()
    }
    pub fn toggle(&mut self) {
        if self.state.phase == Phase::Recording {
            self.stop();
        } else if self.state.can_start() {
            self.start();
        }
    }
    pub fn start(&mut self) {
        if !self.state.can_start() {
            return;
        }
        self.delivery.clear();
        if self.automatic
            && let Err(message) = self.delivery.begin()
        {
            self.delivery_status = message.clone();
            if self.delivery.blocks_recording() {
                self.message = message;
                return;
            }
        }
        self.recorder.cancel();
        self.recording = None;
        let Some(generation) = self.state.begin() else {
            return;
        };
        self.transcript.clear();
        self.recorded_language = self.language.clone();
        self.allow_delivery = self.automatic;
        self.message = "正在检查麦克风和本机语音识别权限…".into();
        Recorder::request_permissions(generation, self.events_tx.clone());
    }
    pub fn stop(&mut self) {
        if self.state.phase != Phase::Recording {
            return;
        }
        self.started = None;
        self.delivery_status = self.delivery.finish_preview();
        match self.recorder.stop() {
            Ok(recording) => {
                self.recording = Some(recording);
                self.state.transcribe();
                self.submit();
            }
            Err(message) => {
                self.state.phase = Phase::Failed;
                self.state.retry_available = false;
                self.message = message;
                self.delivery.retain_preview();
                self.wait_for_delivery();
            }
        }
    }
    pub fn retry(&mut self) {
        if self.recording.is_none() || self.state.retry().is_none() {
            return;
        }
        self.delivery.clear();
        self.allow_delivery = false;
        self.submit();
    }
    fn submit(&mut self) {
        let result = (|| {
            if self.worker.is_none() {
                self.worker = Some(Worker::new()?);
            }
            let cancel = CancellationToken::new();
            let recording = self
                .recording
                .as_ref()
                .ok_or_else(|| "录音已不可用。".to_string())?
                .clone();
            let job = Job {
                generation: self.state.generation(),
                recording,
                language: self.recorded_language.clone(),
                proxy: crate::proxy::https_proxy(),
                cancel: cancel.clone(),
            };
            match self.worker.as_ref().unwrap().jobs.try_send(job) {
                Ok(()) => {}
                Err(TrySendError::Disconnected(_)) => {
                    self.worker = None;
                    return Err("转写线程已退出，请手动重试。".to_string());
                }
                Err(TrySendError::Full(_)) => {
                    return Err("转写线程仍忙，请稍后手动重试。".to_string());
                }
            }
            self.active = Some(ActiveJob {
                generation: self.state.generation(),
                cancel,
            });
            Ok::<(), String>(())
        })();
        match result {
            Ok(()) => self.message = "正在使用 ChatGPT 完成最终转写…".into(),
            Err(message) => {
                self.state.phase = Phase::Failed;
                self.state.retry_available = self.recording.is_some();
                self.message = message;
                self.delivery.retain_preview();
                self.wait_for_delivery();
            }
        }
    }
    fn wait_for_delivery(&mut self) {
        if self.delivery.has_pending() && matches!(self.state.phase, Phase::Ready | Phase::Failed) {
            self.delivery_settle = Some(self.state.phase);
            self.state.phase = Phase::Delivering;
        }
    }
    pub fn cancel(&mut self) {
        self.delivery_settle = None;
        let worker_active = self.active.is_some();
        self.state.cancel(worker_active);
        self.delivery.clear();
        self.started = None;
        if let Some(active) = &self.active {
            active.cancel.cancel();
            self.message = "正在取消并释放录音…".into();
        } else {
            self.recorder.cancel();
            self.recording = None;
            self.message = "听写已取消。".into();
        }
    }
    pub fn poll(&mut self) {
        while let Ok((generation, event)) = self.events.try_recv() {
            if !self.state.accepts(generation) {
                continue;
            }
            match event {
                AudioEvent::Permissions { microphone, speech } => {
                    // Duplicate callbacks must not cancel an already-started
                    // session or change its delivery target.
                    if self.state.phase != Phase::Permissions {
                        continue;
                    }
                    if !self.state.permissions(generation, microphone) {
                        if !microphone {
                            self.message = "请在系统设置中允许 VoicePort 使用麦克风。".into();
                            self.delivery.clear();
                        }
                        continue;
                    }
                    match self.recorder.start(
                        speech,
                        &self.recorded_language,
                        generation,
                        self.events_tx.clone(),
                    ) {
                        Ok(preview) => {
                            self.started = Some(Instant::now());
                            self.message = "正在录音；再次按快捷键停止。".into();
                            self.preview_status = if preview {
                                "设备端语音预览已启用"
                            } else {
                                "本机预览不可用，停止后批量转写"
                            }
                            .into();
                        }
                        Err(message) => {
                            self.state.phase = Phase::Failed;
                            self.message = message;
                            self.recorder.cancel();
                            self.delivery.clear();
                        }
                    }
                }
                AudioEvent::Partial(text) if self.state.phase == Phase::Recording => {
                    self.transcript = text;
                    self.delivery_status = self.delivery.preview(&self.transcript);
                }
                AudioEvent::Status(text) => self.preview_status = text,
                AudioEvent::Failed(message) if self.state.phase == Phase::Recording => {
                    self.message = message;
                    self.state.phase = Phase::Failed;
                    self.started = None;
                    self.recorder.cancel();
                    self.delivery.retain_preview();
                    self.wait_for_delivery();
                }
                AudioEvent::LimitReached if self.state.phase == Phase::Recording => self.stop(),
                _ => {}
            }
        }
        if self.state.phase == Phase::Recording && self.seconds() >= 180 {
            self.stop();
        }
        let completion = match self.worker.as_ref().map(|worker| worker.results.try_recv()) {
            Some(Ok(completion)) => Some(completion),
            Some(Err(TryRecvError::Disconnected)) => {
                self.worker = None;
                if let Some(active) = self.active.take() {
                    active.cancel.cancel();
                    self.allow_delivery = false;
                    self.delivery.retain_preview();
                    self.wait_for_delivery();
                    let stopping = self.state.phase == Phase::Stopping;
                    if self.state.worker_disconnected(self.recording.is_some()) {
                        if stopping {
                            self.recorder.cancel();
                            self.recording = None;
                            self.message = "听写已取消。".into();
                        } else {
                            self.message = "转写工作线程已退出，录音保留供手动重试。".into();
                        }
                    }
                }
                None
            }
            Some(Err(TryRecvError::Empty)) | None => None,
        };
        if let Some(completion) = completion {
            // A stale completion cannot release a newer job's cancellation
            // token or recording lease, including during shutdown.
            if self
                .active
                .as_ref()
                .is_none_or(|active| active.generation != completion.generation)
            {
                return;
            }
            self.active = None;
            if self.state.phase == Phase::Stopping {
                self.recorder.cancel();
                self.recording = None;
                self.state.phase = Phase::Idle;
                self.message = "听写已取消。".into();
            } else if self.state.complete(
                completion.generation,
                completion.result.is_ok(),
                self.recording.is_some(),
            ) {
                match completion.result {
                    Ok(text) => {
                        self.transcript = text;
                        self.delivery_status =
                            self.delivery.deliver(&self.transcript, self.allow_delivery);
                        self.message = "最终转写完成。".into();
                        self.recorder.finish();
                        self.recording = None;
                    }
                    Err(message) => {
                        self.message = message;
                        self.delivery.retain_preview();
                        self.wait_for_delivery();
                    }
                }
            }
        }
        self.wait_for_delivery();
        if let Some(status) = self.delivery.poll() {
            self.delivery_status = status;
        }
        if self.state.phase == Phase::Delivering && !self.delivery.has_pending() {
            self.state.phase = self.delivery_settle.take().unwrap_or(Phase::Ready);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Recorder/Delivery constructors are inert. No test calls start, requests
    // permissions, creates a recording, starts a worker, or contacts a service.
    fn transcribing() -> Controller {
        let mut c = Controller::new();
        let generation = c.state.begin().unwrap();
        c.state.permissions(generation, true);
        c.state.transcribe();
        c.active = Some(ActiveJob {
            generation,
            cancel: CancellationToken::new(),
        });
        c.allow_delivery = true;
        c
    }

    fn disconnected_worker() -> Worker {
        let (jobs, incoming) = mpsc::sync_channel(1);
        let (outgoing, results) = mpsc::channel();
        drop(incoming);
        drop(outgoing);
        Worker { jobs, results }
    }

    #[test]
    fn disconnected_worker_exits_transcribing_and_is_recreated_on_next_submission() {
        let mut c = transcribing();
        let token = c.active.as_ref().unwrap().cancel.clone();
        c.worker = Some(disconnected_worker());
        c.poll();
        assert_eq!(c.state.phase, Phase::Failed);
        assert!(c.worker.is_none());
        assert!(c.active.is_none());
        assert!(!c.allow_delivery);
        assert!(!c.state.retry_available); // no synthetic recording is provided
        assert!(token.is_cancelled());
    }

    #[test]
    fn disconnected_worker_releases_stopping_without_waiting_forever() {
        let mut c = transcribing();
        c.worker = Some(disconnected_worker());
        c.cancel();
        assert_eq!(c.state.phase, Phase::Stopping);
        c.poll();
        assert_eq!(c.state.phase, Phase::Idle);
        assert!(c.worker.is_none() && c.active.is_none());
        assert!(!c.state.retry_available);
    }

    #[test]
    fn empty_result_channel_keeps_current_job_and_state() {
        let mut c = transcribing();
        let (jobs, _incoming) = mpsc::sync_channel(1);
        let (_outgoing, results) = mpsc::channel();
        c.worker = Some(Worker { jobs, results });
        c.poll();
        assert_eq!(c.state.phase, Phase::Transcribing);
        assert!(c.worker.is_some() && c.active.is_some());
    }

    #[test]
    fn stale_completion_cannot_release_current_job_or_replace_transcript() {
        let mut c = transcribing();
        let current = c.state.generation();
        let (jobs, _incoming) = mpsc::sync_channel(1);
        let (outgoing, results) = mpsc::channel();
        outgoing
            .send(Completion {
                generation: current - 1,
                result: Ok("stale".into()),
            })
            .unwrap();
        c.worker = Some(Worker { jobs, results });
        c.poll();
        assert_eq!(c.active.as_ref().unwrap().generation, current);
        assert_eq!(c.state.phase, Phase::Transcribing);
        assert!(c.transcript.is_empty());
    }

    #[test]
    fn cancelled_completion_only_releases_matching_job() {
        let mut c = transcribing();
        let job_generation = c.state.generation();
        let (jobs, _incoming) = mpsc::sync_channel(1);
        let (outgoing, results) = mpsc::channel();
        outgoing
            .send(Completion {
                generation: job_generation - 1,
                result: Ok("stale".into()),
            })
            .unwrap();
        outgoing
            .send(Completion {
                generation: job_generation,
                result: Ok("cancelled".into()),
            })
            .unwrap();
        c.worker = Some(Worker { jobs, results });
        c.cancel();
        c.cancel(); // repeated cancellation does not lose the active job identity
        c.poll();
        assert_eq!(c.state.phase, Phase::Stopping);
        assert!(c.active.is_some());
        c.poll();
        assert_eq!(c.state.phase, Phase::Idle);
        assert!(c.active.is_none());
        assert!(c.transcript.is_empty());
    }

    #[test]
    fn cancelled_and_duplicate_permission_callbacks_never_start_or_cancel_a_session() {
        let mut c = Controller::new();
        let old = c.state.begin().unwrap();
        c.state.cancel(false);
        let current = c.state.begin().unwrap();
        c.events_tx
            .send((
                old,
                AudioEvent::Permissions {
                    microphone: true,
                    speech: true,
                },
            ))
            .unwrap();
        c.poll();
        assert_eq!(c.state.phase, Phase::Permissions);
        c.state.permissions(current, true); // pure transition, no recorder start
        c.message = "current recording".into();
        c.events_tx
            .send((
                current,
                AudioEvent::Permissions {
                    microphone: false,
                    speech: false,
                },
            ))
            .unwrap();
        c.poll();
        assert_eq!(c.state.phase, Phase::Recording);
        assert_eq!(c.message, "current recording");
        assert_eq!(c.delivery.failure_stage(), None);
    }
}
