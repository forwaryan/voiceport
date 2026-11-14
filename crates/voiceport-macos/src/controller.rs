//! Main-thread orchestration; cleanup confirmation gates recording release and admission.
use crate::{
    audio::{AudioEvent, Recorder},
    delivery::Delivery,
    lifecycle::{Lifecycle, Phase},
    local_audio::Incoming,
    local_models::{CHATGPT, Catalog, SessionSource},
    plugin_process::{CLEANUP_WARNING, Custody, Failure},
    storage::Recording,
};
use std::sync::Mutex;
use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender, SyncSender, TryRecvError, TrySendError},
    },
    time::Instant,
};
type PartialSlot = Arc<Mutex<Option<(u64, String)>>>;
use voiceport_core::{CancellationToken, TranscriptionClient};

struct Job {
    generation: u64,
    recording: Option<Arc<Recording>>,
    sample: bool,
    language: String,
    proxy: Option<String>,
    cancel: CancellationToken,
    source: SessionSource,
    incoming: Option<Incoming>,
    partial: PartialSlot,
    custody: Custody,
}
struct Completion {
    generation: u64,
    result: Result<String, Failure>,
}
struct ActiveJob {
    generation: u64,
    cancel: CancellationToken,
    custody: Custody,
    // Established before dispatch; Job drop/panic/send failure cannot release it.
    _recording: Option<Arc<Recording>>,
}
struct CleanupBlock {
    generation: u64,
    custody: Custody,
    cancelled: bool,
    original_error: String,
    observation: Option<Receiver<bool>>,
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
                while let Ok(mut job) = incoming.recv() {
                    let generation = job.generation;
                    // Unwind on the worker only. Controller already holds custody
                    // and its recording lease before Job can drop or send can fail.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        match &job.source {
                            SessionSource::Plugin(plugin) => {
                                return if job.sample {
                                    plugin.execute(
                                        crate::plugin_runtime::sample(),
                                        &job.language,
                                        &job.cancel,
                                        &job.custody,
                                    )
                                } else {
                                    plugin.transcribe(
                                        job.recording.as_ref().ok_or("录音已不可用。")?.path(),
                                        &job.language,
                                        &job.cancel,
                                        &job.custody,
                                    )
                                };
                            }
                            SessionSource::Local(model) => {
                                let partial = |text| {
                                    if let Ok(mut slot) = job.partial.lock() {
                                        *slot = Some((generation, text));
                                    }
                                };
                                return match job.incoming.take() {
                                    Some(incoming) => crate::local_runtime::transcribe_live(
                                        model,
                                        &job.language,
                                        incoming,
                                        &job.cancel,
                                        partial,
                                    ),
                                    None => crate::local_runtime::transcribe_file(
                                        model,
                                        &job.language,
                                        job.recording.as_ref().ok_or("录音已不可用。")?.path(),
                                        &job.cancel,
                                        partial,
                                    ),
                                }
                                .map_err(Failure::from);
                            }
                            SessionSource::ChatGpt => {}
                        }
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .map_err(|_| "无法启动转写任务。".to_string())?;
                        runtime
                            .block_on(async {
                                let auth = voiceport_core::auth::default_path()
                                    .map_err(|e| e.to_string())?;
                                let client =
                                    TranscriptionClient::new_with_proxy(auth, job.proxy.as_deref())
                                        .map_err(|e| e.to_string())?;
                                client
                                    .transcribe(
                                        job.recording.as_ref().ok_or("录音已不可用。")?.path(),
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
                            .map_err(Failure::from)
                    }))
                    .unwrap_or_else(|_| Err("转写任务异常结束，请手动重试。".into()));
                    let result = if job.custody.unresolved() {
                        let mut failure = result
                            .err()
                            .unwrap_or_else(|| job.custody.failure_message().into());
                        if failure.cleanup.is_none() {
                            failure.message = job.custody.failure_message();
                            failure.cleanup = Some(job.custody.clone());
                        }
                        Err(failure)
                    } else {
                        result
                    };
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
    pub source_id: String,
    pub catalog: Catalog,
    discovery: Option<Receiver<Catalog>>,
    recorded_source: SessionSource,
    partial: PartialSlot,
    recorder: Recorder,
    delivery: Delivery,
    events_tx: Sender<(u64, AudioEvent)>,
    events: Receiver<(u64, AudioEvent)>,
    worker: Option<Worker>,
    active: Option<ActiveJob>,
    cleanup: Option<CleanupBlock>,
    recording: Option<Arc<Recording>>,
    recorded_language: String,
    started: Option<Instant>,
    allow_delivery: bool,
    delivery_settle: Option<Phase>,
    stop_failure: Option<String>,
    testing: bool,
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
            source_id: CHATGPT.into(),
            catalog: Catalog::pending(CHATGPT),
            discovery: None,
            recorded_source: SessionSource::ChatGpt,
            partial: Arc::new(Mutex::new(None)),
            recorder: Recorder::new(),
            delivery: Delivery::new(),
            events_tx,
            events,
            worker: None,
            active: None,
            cleanup: None,
            recording: None,
            recorded_language: String::new(),
            started: None,
            allow_delivery: false,
            delivery_settle: None,
            stop_failure: None,
            testing: false,
        }
    }
    pub fn refresh_catalog(&mut self) {
        if self.busy() || self.discovery.is_some() {
            return;
        }
        let saved = self.source_id.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("voiceport-model-discovery".into())
            .spawn(move || {
                let _ = sender.send(Catalog::system(&saved));
            }) {
            Ok(_) => self.discovery = Some(receiver),
            Err(_) => self.message = "无法检查本地模型，请再次刷新。".into(),
        }
    }
    pub fn uses_system_speech(&self) -> bool {
        self.catalog
            .snapshot(&self.source_id)
            .is_ok_and(|s| s.uses_system_speech())
    }
    pub fn discovering(&self) -> bool {
        self.discovery.is_some()
    }
    pub fn seconds(&self) -> u64 {
        self.started.map(|s| s.elapsed().as_secs()).unwrap_or(0)
    }
    pub fn busy(&self) -> bool {
        !self.state.can_start() || self.delivery.has_pending()
    }
    pub(crate) fn input_failure_stage(&self) -> Option<&'static str> {
        self.delivery.failure_stage()
    }
    pub(crate) fn input_method(&self) -> Option<&'static str> {
        self.delivery.method()
    }
    pub(crate) fn reject_start(&mut self, message: &str) {
        if self.busy() {
            return;
        }
        self.state.phase = Phase::Failed;
        self.state.retry_available = false;
        self.message = format!("{message} 本轮未开始录音，已有文字保留在结果区。");
    }
    pub fn toggle(&mut self) {
        if self.state.phase == Phase::Recording {
            self.stop();
        } else if self.state.can_start() {
            self.start();
        }
    }
    pub fn start(&mut self) {
        self.start_for_application(None);
    }
    pub(crate) fn start_in_application(
        &mut self,
        target: &crate::delivery::model::ApplicationTarget,
    ) {
        self.start_for_application(Some(target));
    }
    fn start_for_application(
        &mut self,
        target: Option<&crate::delivery::model::ApplicationTarget>,
    ) {
        if !self.state.can_start() {
            return;
        }
        let source = match self
            .catalog
            .snapshot(&self.source_id)
            .and_then(SessionSource::pin)
        {
            Ok(source) => source,
            Err(error) => {
                self.message = error;
                self.state.phase = Phase::Failed;
                self.state.retry_available = false;
                return;
            }
        };
        if let Some((generation, use_speech)) =
            self.prepare_start_in_application(source, self.language.clone(), self.automatic, target)
        {
            Recorder::request_permissions_for_source(
                generation,
                self.events_tx.clone(),
                use_speech,
            );
        }
    }
    /// Prepared on a background thread; never changes GUI source/language/preferences.
    pub(crate) fn start_pinned(
        &mut self,
        source: SessionSource,
        language: String,
        automatic: bool,
    ) {
        if let Some((generation, use_speech)) = self.prepare_start(source, language, automatic) {
            Recorder::request_permissions_for_source(
                generation,
                self.events_tx.clone(),
                use_speech,
            );
        }
    }
    fn prepare_start(
        &mut self,
        source: SessionSource,
        language: String,
        automatic: bool,
    ) -> Option<(u64, bool)> {
        self.prepare_start_in_application(source, language, automatic, None)
    }
    fn prepare_start_in_application(
        &mut self,
        source: SessionSource,
        language: String,
        automatic: bool,
        target: Option<&crate::delivery::model::ApplicationTarget>,
    ) -> Option<(u64, bool)> {
        if self.busy() {
            return None;
        }
        self.delivery.clear();
        if self.delivery.has_pending() {
            self.wait_for_delivery();
            self.message = "正在确认上一轮输入，请稍后重新开始。".into();
            return None;
        }
        if automatic && let Err(message) = self.delivery.begin_in_application(target) {
            self.delivery_status = message.clone();
            if self.delivery.blocks_recording() {
                self.reject_start(&message);
                return None;
            }
        }
        self.recorder.cancel();
        self.recording = None;
        let generation = self.state.begin()?;
        self.transcript.clear();
        self.testing = false;
        self.recorded_language = language;
        self.recorded_source = source;
        self.allow_delivery = automatic;
        let use_speech = self.recorded_source.uses_system_speech();
        self.message = if use_speech {
            "正在检查麦克风和本机语音识别权限…"
        } else {
            "正在检查麦克风权限；此来源无需系统语音识别权限…"
        }
        .into();
        Some((generation, use_speech))
    }
    /// Explicit synthetic sample only; no recorder, permissions or delivery target.
    pub fn test_source(&mut self) {
        if self.busy() || self.discovering() {
            return;
        }
        let result = self
            .catalog
            .snapshot(&self.source_id)
            .and_then(|s| match s {
                SessionSource::Plugin(_) => s.pin(),
                _ => Err("内置样例测试适用于自定义服务或外部适配器；请选择一个配置实例。".into()),
            });
        let source = match result {
            Ok(s) => s,
            Err(e) => {
                self.message = e;
                return;
            }
        };
        self.test_pinned(source, self.language.clone());
    }
    pub(crate) fn test_pinned(&mut self, source: SessionSource, language: String) {
        if self.busy() {
            return;
        }
        self.delivery.clear();
        self.recording = None;
        self.transcript.clear();
        let Some(generation) = self.state.begin() else {
            return;
        };
        self.state.permissions(generation, true);
        self.state.transcribe();
        self.testing = true;
        self.allow_delivery = false;
        self.recorded_source = source;
        self.recorded_language = language;
        self.submit();
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
                if self.recorded_source.is_local() && self.active.is_some() {
                    self.message = "正在本地处理剩余音频并完成最终转写…".into();
                } else {
                    self.submit();
                }
            }
            Err(message) => {
                // This recording cannot be retried. Release capture/controller
                // references; an active local Job keeps its lease until completion.
                self.recorder.cancel();
                self.recording = None;
                self.state.phase = Phase::Failed;
                if let Some(active) = &self.active {
                    active.cancel.cancel();
                    self.stop_failure = Some(message.clone());
                    self.state.phase = Phase::Stopping;
                }
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
        self.submit_input(None);
    }
    fn submit_input(&mut self, incoming: Option<Incoming>) {
        let result = (|| {
            if self.worker.is_none() {
                self.worker = Some(Worker::new()?);
            }
            let cancel = CancellationToken::new();
            let recording = self.recording.clone();
            if recording.is_none() && !self.testing {
                return Err("录音已不可用。".to_string());
            }
            let custody = Custody::default();
            // These local clones survive send and are installed synchronously in
            // Controller. No worker/channel path can become the sole custodian.
            let retained_recording = recording.clone();
            let job = Job {
                generation: self.state.generation(),
                recording,
                custody: custody.clone(),
                sample: self.testing,
                language: self.recorded_language.clone(),
                proxy: if matches!(self.recorded_source, SessionSource::ChatGpt) {
                    crate::proxy::https_proxy()
                } else {
                    None
                },
                cancel: cancel.clone(),
                source: self.recorded_source.clone(),
                incoming,
                partial: self.partial.clone(),
            };
            self.active = Some(ActiveJob {
                generation: self.state.generation(),
                cancel,
                custody,
                _recording: retained_recording,
            });
            match self.worker.as_ref().unwrap().jobs.try_send(job) {
                Ok(()) => {}
                Err(TrySendError::Disconnected(_)) => {
                    self.active = None; // Rejected Job never reached a worker.
                    self.worker = None;
                    return Err("转写线程已退出，请手动重试。".to_string());
                }
                Err(TrySendError::Full(_)) => {
                    self.active = None;
                    return Err("转写线程仍忙，请稍后手动重试。".to_string());
                }
            }
            Ok::<(), String>(())
        })();
        match result {
            Ok(()) => {
                self.message = if self.recorded_source.is_local() {
                    "正在载入本地模型并处理音频…"
                } else if matches!(self.recorded_source, SessionSource::Plugin(_)) {
                    if self.testing {
                        "正在发送内置一秒静音样例；未启用麦克风，可取消。"
                    } else {
                        "正在使用所选插件完成停止后转写…"
                    }
                } else {
                    "正在使用 ChatGPT 完成最终转写…"
                }
                .into()
            }
            Err(message) => {
                self.recorder.stop().ok();
                self.state.phase = Phase::Failed;
                self.state.retry_available = self.recording.is_some();
                self.message = message;
                self.delivery.retain_preview();
                self.wait_for_delivery();
            }
        }
    }
    fn wait_for_delivery(&mut self) {
        if self.delivery.has_pending()
            && matches!(self.state.phase, Phase::Idle | Phase::Ready | Phase::Failed)
        {
            self.delivery_settle = Some(self.state.phase);
            self.state.phase = Phase::Delivering;
        }
    }
    fn block_cleanup(&mut self, original_error: String) {
        if self.cleanup.is_none() {
            let Some(active) = &self.active else {
                return;
            };
            self.cleanup = Some(CleanupBlock {
                generation: active.generation,
                custody: active.custody.clone(),
                cancelled: self.state.phase == Phase::Stopping,
                original_error,
                observation: None,
            });
        }
        self.state.phase = Phase::Stopping;
        self.state.retry_available = false;
        self.allow_delivery = false;
        self.delivery_settle = None;
        self.delivery.clear();
        self.started = None;
        self.recorder.cancel();
        self.show_cleanup_warning();
    }
    fn show_cleanup_warning(&mut self) {
        if let Some(block) = &self.cleanup {
            self.message = format!("{}{CLEANUP_WARNING}", block.original_error);
        }
    }
    fn request_cleanup_observation(&mut self) {
        let Some(block) = &mut self.cleanup else {
            return;
        };
        if block.observation.is_some() {
            return;
        }
        let Some(active) = &self.active else {
            return;
        };
        if active.generation != block.generation || !active.custody.same_owner(&block.custody) {
            return;
        }
        let custody = block.custody.clone();
        let (sender, receiver) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("voiceport-cleanup-check".into())
            .spawn(move || {
                let _ = sender.send(custody.observe_once());
            }) {
            Ok(_) => {
                block.observation = Some(receiver);
                self.message = format!(
                    "{}正在检查一次；清理尚未确认，录音仍被保留。",
                    block.original_error
                );
            }
            Err(_) => self.show_cleanup_warning(),
        }
    }
    fn poll_cleanup_observation(&mut self) {
        let result = self
            .cleanup
            .as_ref()
            .and_then(|block| block.observation.as_ref())
            .map(|receiver| receiver.try_recv());
        match result {
            Some(Ok(true))
                if self.active.as_ref().is_some_and(|active| {
                    !active.custody.unresolved()
                        && self.cleanup.as_ref().is_some_and(|block| {
                            active.generation == block.generation
                                && active.custody.same_owner(&block.custody)
                        })
                }) =>
            {
                let block = self.cleanup.take().expect("matching cleanup observation");
                self.active = None;
                if block.cancelled {
                    self.recording = None;
                    if let Some(error) = self.stop_failure.take() {
                        self.state.phase = Phase::Failed;
                        self.message = error;
                    } else {
                        self.state.phase = Phase::Idle;
                        self.message =
                            format!("{}插件清理已确认；听写已取消。", block.original_error);
                    }
                    self.state.retry_available = false;
                } else {
                    self.state.phase = Phase::Failed;
                    self.state.retry_available = self.recording.is_some();
                    self.message = format!("{}插件清理已确认；未重试转写。", block.original_error);
                }
            }
            Some(Ok(_)) | Some(Err(TryRecvError::Disconnected)) => {
                if let Some(block) = &mut self.cleanup {
                    block.observation = None;
                }
                self.show_cleanup_warning();
            }
            Some(Err(TryRecvError::Empty)) | None => {}
        }
    }
    pub fn cancel(&mut self) {
        if self.cleanup.is_some() {
            // This existing, enabled native action is an explicit observation
            // request. Neither polling nor repeated Quit flags call it for us.
            self.request_cleanup_observation();
            return;
        }
        self.stop_failure = None;
        self.delivery_settle = None;
        let worker_active = self.active.is_some();
        self.state.cancel(worker_active);
        self.delivery.clear();
        self.started = None;
        self.recorder.cancel();
        if let Some(active) = &self.active {
            active.cancel.cancel();
            self.message = "正在取消并释放录音…".into();
        } else {
            self.recorder.cancel();
            self.recording = None;
            self.message = "听写已取消。".into();
        }
        self.wait_for_delivery();
    }
    pub fn poll(&mut self) {
        self.poll_cleanup_observation();
        if let Some(discovery) = &self.discovery {
            match discovery.try_recv() {
                Ok(catalog) => {
                    self.catalog = catalog;
                    self.discovery = None;
                }
                Err(TryRecvError::Disconnected) => {
                    self.discovery = None;
                    self.message = "本地模型发现线程已退出，请刷新模型。".into();
                }
                Err(TryRecvError::Empty) => {}
            }
        }
        let partial = self.partial.lock().ok().and_then(|mut slot| slot.take());
        if let Some((generation, text)) = partial
            && self.state.accepts(generation)
            && self.state.phase == Phase::Recording
        {
            self.transcript = text;
            self.delivery_status = self.delivery.preview(&self.transcript);
        }
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
                    let (local, incoming) = if self.recorded_source.is_local() {
                        let (input, incoming) = crate::local_audio::channel();
                        (Some(input), Some(incoming))
                    } else {
                        (None, None)
                    };
                    match self.recorder.start(
                        speech && self.recorded_source.uses_system_speech(),
                        &self.recorded_language,
                        generation,
                        self.events_tx.clone(),
                        local,
                    ) {
                        Ok(preview) => {
                            self.started = Some(Instant::now());
                            if let Some(incoming) = incoming {
                                self.recording = self.recorder.lease();
                                self.submit_input(Some(incoming));
                            }
                            if self.state.phase == Phase::Recording {
                                self.message =
                                    if self.delivery.method() == Some("wechat_menu_paste") {
                                        "微信兼容听写中；停止后会复制最终文字并请求菜单粘贴。"
                                    } else {
                                        "正在录音；再次按快捷键停止。"
                                    }
                                    .into();
                            }
                            self.preview_status = if self.recorded_source.streaming() {
                                "Paraformer 本地流式识别（模型在后台载入）"
                            } else if self.recorded_source.is_local() {
                                "Whisper Tiny 本地离线识别；停止后出字"
                            } else if preview {
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
                AudioEvent::Failed(message)
                    if self.state.phase == Phase::Recording && self.recorded_source.is_local() =>
                {
                    self.stop();
                    self.message = message;
                }
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
                if self.state.phase == Phase::Recording {
                    self.stop();
                }
                if self.cleanup.is_some()
                    || self
                        .active
                        .as_ref()
                        .is_some_and(|active| active.custody.unresolved())
                {
                    if let Some(active) = &self.active {
                        active.cancel.cancel();
                        let error = active.custody.failure_message();
                        self.block_cleanup(error);
                    }
                } else if let Some(active) = self.active.take() {
                    active.cancel.cancel();
                    self.allow_delivery = false;
                    self.delivery.retain_preview();
                    self.wait_for_delivery();
                    let stopping = self.state.phase == Phase::Stopping;
                    if self.state.worker_disconnected(self.recording.is_some()) {
                        if stopping {
                            self.recorder.cancel();
                            self.recording = None;
                            if let Some(error) = self.stop_failure.take() {
                                self.state.phase = Phase::Failed;
                                self.message = error;
                            } else {
                                self.message = "听写已取消。".into();
                            }
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
            if self.state.phase == Phase::Recording {
                // A local worker can fail while loading or consuming live audio.
                // Finalize the valid AAC lease before offering the same-source retry.
                self.stop();
            }
            if self.cleanup.is_some()
                || self
                    .active
                    .as_ref()
                    .is_some_and(|active| active.custody.unresolved())
                || completion
                    .result
                    .as_ref()
                    .is_err_and(|error| error.cleanup.is_some())
            {
                let error = completion
                    .result
                    .err()
                    .map(|error| error.message)
                    .unwrap_or_else(|| self.active.as_ref().unwrap().custody.failure_message());
                self.block_cleanup(error);
                return;
            }
            self.active = None;
            if self.state.phase == Phase::Stopping {
                self.recorder.cancel();
                self.recording = None;
                if let Some(error) = self.stop_failure.take() {
                    self.state.phase = Phase::Failed;
                    self.message = error;
                } else {
                    self.state.phase = Phase::Idle;
                    self.message = "听写已取消。".into();
                }
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
                        self.message = if self.testing { if self.transcript.is_empty() { "样例接口已响应：没有识别到语音（内置样例为一秒静音）；未验证识别质量。" } else { "样例接口已响应，返回文字见面板；静音样例不能证明识别质量。" } } else if self.transcript.is_empty() { "没有识别到语音。" } else { "最终转写完成。" }.into();
                        if self.allow_delivery
                            && !self.transcript.is_empty()
                            && self.delivery.method() == Some("wechat_menu_paste")
                        {
                            self.message = self.delivery_status.clone();
                        }
                        self.recorder.finish();
                        self.recording = None;
                    }
                    Err(error) => {
                        self.message = error.message;
                        self.delivery.retain_preview();
                        self.wait_for_delivery();
                    }
                }
            }
        }
        self.wait_for_delivery();
        if let Some(status) = self.delivery.poll() {
            self.delivery_status = status;
            if self.delivery.method() == Some("wechat_menu_paste") {
                self.message = self.delivery_status.clone();
            }
        }
        if self.state.phase == Phase::Delivering && !self.delivery.has_pending() {
            self.state.phase = self.delivery_settle.take().unwrap_or(Phase::Ready);
        }
    }
}

// Compiled into tests/stop_failure.rs, whose custom harness runs on the OS
// main thread. Ordinary libtest threads cannot exercise native Recorder.stop.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) mod stop_failure_regression {
    use super::*;
    use crate::audio::stop_failure_fixture::{self, Input};
    use crate::storage::{NAMESPACE, Storage};
    use objc2::MainThreadMarker;
    use std::{
        fs::File, os::fd::AsRawFd, path::PathBuf, sync::Weak, thread::JoinHandle, time::Duration,
    };

    const STOP_ERROR: &str = "无法写入完整录音，请重新录音。";
    const WRITE_ERROR: &str = "录音写入失败，请重新录音：录音文件已关闭。";
    const PREVIEW: &str = "synthetic local preview";

    struct Fixture {
        controller: Controller,
        recording: Weak<Recording>,
        audio: PathBuf,
        lease: File,
        _temp: tempfile::TempDir,
    }

    impl Fixture {
        fn new(input: Input) -> Self {
            assert!(MainThreadMarker::new().is_some());
            let temp = tempfile::tempdir().unwrap();
            let store = Storage::at(temp.path().canonicalize().unwrap().join(NAMESPACE));
            let mut controller = Controller::new();
            controller.automatic = false;
            let generation = controller.state.begin().unwrap();
            assert!(controller.state.permissions(generation, true));
            let (recorder, recording, audio) = stop_failure_fixture::recorder(
                &store,
                generation,
                controller.events_tx.clone(),
                input,
            );
            controller.recorder = recorder;
            controller.transcript = PREVIEW.into();
            controller.preview_status = "synthetic preview status".into();
            controller.started = Some(Instant::now());
            let lease = File::open(audio.parent().unwrap().join("lease")).unwrap();
            let fixture = Self {
                controller,
                recording,
                audio,
                lease,
                _temp: temp,
            };
            assert!(fixture.lease_active());
            assert!(fixture.recording.upgrade().is_some());
            fixture
        }

        // An independent descriptor probes the actual kernel flock, including
        // after the lease pathname has been unlinked. No cleanup clock or delete
        // operation can make the resource assertions pass.
        fn lease_active(&self) -> bool {
            let fd = self.lease.as_raw_fd();
            if unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                assert_eq!(unsafe { libc::flock(fd, libc::LOCK_UN) }, 0);
                false
            } else {
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::EWOULDBLOCK)
                );
                true
            }
        }

        fn released(&self, case: &str, stage: &str) -> bool {
            let c = &self.controller;
            let recorder_audio = stop_failure_fixture::owns_recording(&c.recorder);
            let controller_audio = c.recording.is_some();
            let live_audio = self.recording.upgrade().is_some();
            let audio_exists = self.audio.exists();
            let session_exists = self.audio.parent().unwrap().exists();
            let lease_active = self.lease_active();
            println!(
                "{}",
                serde_json::json!({
                    "case": case, "stage": stage, "os_main_thread": MainThreadMarker::new().is_some(),
                    "phase": format!("{:?}", c.state.phase), "retry": c.state.retry_available,
                    "active_job": c.active.is_some(), "recorder_audio": recorder_audio,
                    "controller_audio": controller_audio, "live_audio": live_audio,
                    "audio_exists": audio_exists, "session_exists": session_exists,
                    "lease_active": lease_active, "message": c.message,
                })
            );
            !recorder_audio
                && !controller_audio
                && !live_audio
                && !audio_exists
                && !session_exists
                && !lease_active
        }
    }

    fn local_failure(case: &str) {
        let mut fixture = Fixture::new(if case == "zero" {
            Input::Empty
        } else {
            Input::Damaged
        });
        if case != "zero" {
            // Inspect and restore the single real Sink event, without fabricating
            // a failure or changing its generation or its order relative to Stop.
            let event = fixture.controller.events.try_recv().unwrap();
            assert_eq!(
                event,
                (
                    fixture.controller.state.generation(),
                    AudioEvent::Failed(WRITE_ERROR.into())
                )
            );
            println!("{case}: real Sink.consume event={event:?}");
            assert!(fixture.controller.events.try_recv().is_err());
            fixture.controller.events_tx.send(event).unwrap();
        }
        if case == "poll-first" {
            fixture.controller.poll();
            assert_eq!(fixture.controller.message, WRITE_ERROR);
        }
        fixture.controller.stop();
        let immediate = fixture.released(case, "immediate");
        fixture.controller.poll();
        let after_poll = fixture.released(case, "after-poll");
        let c = &fixture.controller;
        assert_eq!(c.state.phase, Phase::Failed);
        assert!(!c.state.retry_available);
        assert!(c.active.is_none() && c.worker.is_none());
        assert!(c.started.is_none());
        assert_eq!(c.transcript, PREVIEW);
        assert_eq!(c.preview_status, "synthetic preview status");
        assert_eq!(
            c.message,
            if case == "poll-first" {
                WRITE_ERROR
            } else {
                STOP_ERROR
            }
        );
        assert!(
            immediate && after_poll,
            "a non-retryable local Stop failure must release recording references, session files and the real flock before returning and after poll"
        );
    }

    struct WorkerControl {
        started: Receiver<u64>,
        reply: Sender<Result<String, String>>,
        published: Receiver<()>,
        outgoing: Sender<Completion>,
        thread: JoinHandle<()>,
    }

    // A controlled service substitute receives actual production Job values
    // from submit(). A real thread owns each Arc and publishes completion only
    // after dropping that Job, matching Worker's resource ordering. No auth or
    // network client is constructed and no join is used to claim UI completion.
    fn controlled_worker(c: &mut Controller, count: usize) -> WorkerControl {
        let (jobs, incoming) = mpsc::sync_channel::<Job>(1);
        let (outgoing, results) = mpsc::channel();
        let (started_tx, started) = mpsc::channel();
        let (reply, responses) = mpsc::channel::<Result<String, String>>();
        let (published_tx, published) = mpsc::channel();
        let completion = outgoing.clone();
        let thread = std::thread::spawn(move || {
            for _ in 0..count {
                let job = incoming.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(
                    job.recording
                        .as_ref()
                        .expect("synthetic job must own a recording")
                        .path()
                        .exists()
                );
                let generation = job.generation;
                started_tx.send(generation).unwrap();
                let result = responses.recv_timeout(Duration::from_secs(5)).unwrap();
                drop(job);
                completion
                    .send(Completion {
                        generation,
                        result: result.map_err(Failure::from),
                    })
                    .unwrap();
                published_tx.send(()).unwrap();
            }
        });
        c.worker = Some(Worker { jobs, results });
        WorkerControl {
            started,
            reply,
            published,
            outgoing,
            thread,
        }
    }

    fn retry_preserves_recording() {
        let mut fixture = Fixture::new(Input::Valid);
        let control = controlled_worker(&mut fixture.controller, 2);
        fixture.controller.stop();
        let first = control
            .started
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_eq!(fixture.controller.state.phase, Phase::Transcribing);
        control
            .reply
            .send(Err("synthetic transcription failure".into()))
            .unwrap();
        control
            .published
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        fixture.controller.poll();
        assert_eq!(fixture.controller.state.phase, Phase::Failed);
        assert!(fixture.controller.state.retry_available);
        assert_eq!(
            fixture.controller.message,
            "synthetic transcription failure"
        );
        assert!(
            fixture.recording.upgrade().is_some()
                && fixture.audio.exists()
                && fixture.lease_active()
        );
        assert!(stop_failure_fixture::owns_recording(
            &fixture.controller.recorder
        ));
        assert!(fixture.controller.recording.is_some());
        fixture.controller.retry();
        let current = control
            .started
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_ne!(current, first);
        for _ in 0..2 {
            control
                .outgoing
                .send(Completion {
                    generation: first,
                    result: Ok("stale result".into()),
                })
                .unwrap();
            fixture.controller.poll();
            assert_eq!(
                fixture.controller.active.as_ref().unwrap().generation,
                current
            );
            assert_eq!(fixture.controller.state.phase, Phase::Transcribing);
            assert_eq!(fixture.controller.transcript, PREVIEW);
            assert!(fixture.recording.upgrade().is_some() && fixture.lease_active());
        }
        control
            .reply
            .send(Ok("synthetic final transcription".into()))
            .unwrap();
        control
            .published
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        fixture.controller.poll();
        assert_eq!(fixture.controller.state.phase, Phase::Ready);
        assert!(!fixture.controller.state.retry_available);
        assert_eq!(
            fixture.controller.transcript,
            "synthetic final transcription"
        );
        assert!(fixture.released("retry", "matching-success"));
        control.thread.join().unwrap();
    }

    fn cancellation_waits_for_recording_release() {
        let mut fixture = Fixture::new(Input::Valid);
        let control = controlled_worker(&mut fixture.controller, 1);
        fixture.controller.stop();
        let generation = control
            .started
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let token = fixture.controller.active.as_ref().unwrap().cancel.clone();
        fixture.controller.cancel();
        fixture.controller.cancel();
        fixture.controller.poll();
        assert!(token.is_cancelled());
        assert_eq!(fixture.controller.state.phase, Phase::Stopping);
        assert!(
            fixture.recording.upgrade().is_some()
                && fixture.audio.exists()
                && fixture.lease_active()
        );
        control
            .outgoing
            .send(Completion {
                generation: generation - 1,
                result: Ok("stale".into()),
            })
            .unwrap();
        fixture.controller.poll();
        assert_eq!(fixture.controller.state.phase, Phase::Stopping);
        assert!(fixture.controller.active.is_some() && fixture.lease_active());
        control.reply.send(Ok("cancelled result".into())).unwrap();
        control
            .published
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        fixture.controller.poll();
        assert_eq!(fixture.controller.state.phase, Phase::Idle);
        assert!(!fixture.controller.state.retry_available);
        assert_eq!(fixture.controller.transcript, PREVIEW);
        assert!(fixture.released("cancel", "matching-completion"));
        control.thread.join().unwrap();
    }

    fn local_stop_failure_waits_for_worker() {
        let mut fixture = Fixture::new(Input::Damaged);
        let c = &mut fixture.controller;
        c.recorded_source = SessionSource::Local(crate::local_models::Model {
            kind: crate::local_models::ModelKind::Paraformer,
            directory: "/synthetic/model".into(),
            runtime: "/synthetic/runtime".into(),
            encoder: "/synthetic/encoder".into(),
            decoder: "/synthetic/decoder".into(),
            tokens: "/synthetic/tokens".into(),
        });
        c.recording = c.recorder.lease();
        let control = controlled_worker(c, 1);
        let (_input, incoming) = crate::local_audio::channel();
        c.submit_input(Some(incoming));
        let generation = control
            .started
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        let token = c.active.as_ref().unwrap().cancel.clone();
        assert_eq!(c.state.phase, Phase::Recording);
        c.stop();
        assert_eq!(c.message, STOP_ERROR);
        assert!(token.is_cancelled());
        assert_eq!(c.state.phase, Phase::Stopping);
        assert!(!c.state.retry_available && !c.state.can_start());
        assert!(
            fixture.recording.upgrade().is_some()
                && fixture.audio.exists()
                && fixture.lease_active()
        );
        // Drain the real queued Sink failure and a stale completion. Neither
        // may settle this active local Job or release its file/lock early.
        control
            .outgoing
            .send(Completion {
                generation: generation - 1,
                result: Ok("stale local result".into()),
            })
            .unwrap();
        fixture.controller.poll();
        assert_eq!(fixture.controller.state.phase, Phase::Stopping);
        assert!(fixture.controller.active.is_some() && fixture.lease_active());
        assert_eq!(fixture.controller.message, STOP_ERROR);
        control
            .reply
            .send(Ok("cancelled local result".into()))
            .unwrap();
        control
            .published
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        fixture.controller.poll();
        let c = &fixture.controller;
        assert_eq!(c.state.phase, Phase::Failed);
        assert!(!c.state.retry_available && c.active.is_none());
        assert_eq!(c.message, STOP_ERROR);
        assert_eq!(c.transcript, PREVIEW);
        assert_eq!(c.preview_status, "synthetic preview status");
        assert!(fixture.released("local-stop-failure", "matching-completion"));
        control.thread.join().unwrap();
    }

    fn cli_start_stop_uses_pinned_policy_without_devices() {
        let temp = tempfile::tempdir().unwrap();
        let storage = Storage::at(temp.path().canonicalize().unwrap().join(NAMESPACE));
        let mut c = Controller::new();
        c.language = "en".into();
        c.automatic = true;
        let source = SessionSource::Local(crate::local_models::Model {
            kind: crate::local_models::ModelKind::Whisper,
            directory: "/synthetic/model".into(),
            runtime: "/synthetic/runtime".into(),
            encoder: "/synthetic/encoder".into(),
            decoder: "/synthetic/decoder".into(),
            tokens: "/synthetic/tokens".into(),
        });
        // Same production preparation used by start_pinned; substitute only the
        // permission/device boundary, never invoke an OS permission request.
        let (generation, speech) = c.prepare_start(source.clone(), "zh".into(), false).unwrap();
        assert!(!speech && !c.allow_delivery);
        assert_eq!(c.source_id, CHATGPT);
        assert_eq!(c.language, "en");
        assert!(c.automatic);
        assert_eq!(c.recorded_source, source);
        assert_eq!(c.recorded_language, "zh");
        assert!(
            c.prepare_start(SessionSource::ChatGpt, "en".into(), false)
                .is_none()
        );
        assert_eq!(c.recorded_source, source);
        assert!(c.state.permissions(generation, true));
        let (recorder, recording, audio) =
            stop_failure_fixture::recorder(&storage, generation, c.events_tx.clone(), Input::Valid);
        c.recorder = recorder;
        let worker = controlled_worker(&mut c, 1);
        c.stop();
        assert_eq!(
            worker.started.recv_timeout(Duration::from_secs(5)).unwrap(),
            generation
        );
        assert_eq!(c.state.phase, Phase::Transcribing);
        assert!(!c.allow_delivery);
        assert_eq!(c.recorded_source, source);
        worker
            .reply
            .send(Ok("synthetic CLI result".into()))
            .unwrap();
        worker
            .published
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        c.poll();
        assert_eq!(c.state.phase, Phase::Ready);
        assert_eq!(c.transcript, "synthetic CLI result");
        assert!(recording.upgrade().is_none() && !audio.exists());
        assert_eq!(c.source_id, CHATGPT);
        assert_eq!(c.language, "en");
        worker.thread.join().unwrap();
    }
    pub(crate) fn run(case: &str, _main_thread: MainThreadMarker) {
        assert!(MainThreadMarker::new().is_some());
        match case {
            "zero" | "stop-first" | "poll-first" => local_failure(case),
            "retry" => retry_preserves_recording(),
            "cancel" => cancellation_waits_for_recording_release(),
            "local-stop-failure" => local_stop_failure_waits_for_worker(),
            "cli-session" => cli_start_stop_uses_pinned_policy_without_devices(),
            _ => panic!("unknown stop_failure case: {case}"),
        }
        println!("stop_failure: {case} passed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests use inert device/delivery constructors and synthetic leased storage.
    // None requests permissions, starts capture/inference, or contacts a service.
    // The separate OS-main-thread stop_failure target exercises real isolated
    // recordings and Job Arcs with a controlled transcription worker.
    fn transcribing() -> Controller {
        let mut c = Controller::new();
        let generation = c.state.begin().unwrap();
        c.state.permissions(generation, true);
        c.state.transcribe();
        c.active = Some(ActiveJob {
            generation,
            cancel: CancellationToken::new(),
            custody: Custody::default(),
            _recording: None,
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

    fn local_source() -> SessionSource {
        SessionSource::Local(crate::local_models::Model {
            kind: crate::local_models::ModelKind::Paraformer,
            directory: "/synthetic/model".into(),
            runtime: "/synthetic/runtime".into(),
            encoder: "/synthetic/encoder".into(),
            decoder: "/synthetic/decoder".into(),
            tokens: "/synthetic/tokens".into(),
        })
    }
    #[test]
    fn retry_snapshots_original_local_source_and_language_without_cloud_dispatch() {
        let temp = tempfile::tempdir().unwrap();
        let recording = Arc::new(
            crate::storage::Storage::at(
                temp.path()
                    .canonicalize()
                    .unwrap()
                    .join(crate::storage::NAMESPACE),
            )
            .create_recording()
            .unwrap(),
        );
        let mut c = transcribing();
        let old = c.state.generation();
        c.active = None;
        c.state.complete(old, false, true);
        c.recording = Some(recording.clone());
        c.recorded_source = local_source();
        c.recorded_language = "zh".into();
        c.source_id = CHATGPT.into();
        c.language = "en".into();
        let (jobs, incoming) = mpsc::sync_channel(1);
        let (_outgoing, results) = mpsc::channel();
        c.worker = Some(Worker { jobs, results });
        c.retry();
        let job = incoming.try_recv().unwrap();
        assert_eq!(job.source, local_source());
        assert_eq!(job.language, "zh");
        assert!(job.proxy.is_none() && job.incoming.is_none());
        assert_ne!(job.generation, old);
        assert!(!c.allow_delivery);
        c.cancel();
        assert!(job.cancel.is_cancelled());
        assert_eq!(c.state.phase, Phase::Stopping);
        assert!(!c.state.can_start());
        drop(job);
        drop(c);
        drop(recording);
    }
    fn configured_source() -> crate::plugins::Source {
        crate::plugins::Source {
            id: "source-synthetic".into(),
            name: "Synthetic HTTP".into(),
            plugin_id: crate::plugins::HTTP.into(),
            plugin_version: "1".into(),
            enabled: true,
            config: std::collections::BTreeMap::from([
                ("endpoint".into(), "http://127.0.0.1:1/transcribe".into()),
                ("model".into(), "pinned-model".into()),
            ]),
            credential_refs: std::collections::BTreeMap::new(),
        }
    }
    #[test]
    fn plugin_sample_has_no_mic_target_or_recording_and_cancel_waits_for_release() {
        let mut c = Controller::new();
        let mut store = crate::plugins::Store::default();
        store.sources.push(configured_source());
        c.catalog.add_configured(&store);
        c.source_id = store.sources[0].id.clone();
        assert!(!c.uses_system_speech());
        let (jobs, incoming) = mpsc::sync_channel(1);
        let (outgoing, results) = mpsc::channel();
        c.worker = Some(Worker { jobs, results });
        c.test_source();
        let job = incoming.try_recv().unwrap();
        assert!(job.sample && job.recording.is_none() && job.proxy.is_none());
        assert!(c.events.try_recv().is_err());
        assert!(!c.allow_delivery);
        assert!(c.busy());
        c.source_id = CHATGPT.into();
        c.language = "zh".into();
        assert!(matches!(job.source, SessionSource::Plugin(_)));
        assert_eq!(job.language, "");
        let generation = job.generation;
        c.cancel();
        assert!(job.cancel.is_cancelled());
        assert_eq!(c.state.phase, Phase::Stopping);
        drop(job);
        outgoing
            .send(Completion {
                generation,
                result: Ok("late".into()),
            })
            .unwrap();
        c.poll();
        assert!(!c.busy());
        assert!(c.transcript.is_empty());
        assert!(!c.state.retry_available);
    }
    #[test]
    fn plugin_retry_binds_original_endpoint_version_language_and_no_new_target() {
        let temp = tempfile::tempdir().unwrap();
        let recording = Arc::new(
            crate::storage::Storage::at(
                temp.path()
                    .canonicalize()
                    .unwrap()
                    .join(crate::storage::NAMESPACE),
            )
            .create_recording()
            .unwrap(),
        );
        let mut c = transcribing();
        let mut store = crate::plugins::Store::default();
        store.sources.push(configured_source());
        let pinned = crate::plugin_runtime::Snapshot::configured(&store, &store.sources[0])
            .unwrap()
            .pin()
            .unwrap();
        c.recorded_source = SessionSource::Plugin(Box::new(pinned.clone()));
        c.recorded_language = "en".into();
        c.active = None;
        let old = c.state.generation();
        c.state.complete(old, false, true);
        c.recording = Some(recording);
        c.source_id = CHATGPT.into();
        c.language = "zh".into();
        store.sources[0]
            .config
            .insert("endpoint".into(), "https://different.example/new".into());
        c.catalog.add_configured(&store);
        let (jobs, incoming) = mpsc::sync_channel(1);
        let (_outgoing, results) = mpsc::channel();
        c.worker = Some(Worker { jobs, results });
        c.retry();
        let job = incoming.try_recv().unwrap();
        assert_eq!(job.source, SessionSource::Plugin(Box::new(pinned)));
        assert_eq!(job.language, "en");
        assert!(!job.sample && job.proxy.is_none());
        assert!(!c.allow_delivery);
        assert_ne!(job.generation, old);
        assert!(c.busy());
    }
    #[test]
    fn missing_saved_source_fails_before_permissions_delivery_or_cloud_worker() {
        let mut c = Controller::new();
        c.source_id = crate::local_models::PARAFORMER.into();
        c.catalog = Catalog::pending(&c.source_id);
        c.start();
        assert_eq!(c.state.phase, Phase::Failed);
        assert_eq!(c.state.generation(), 0);
        assert!(c.worker.is_none() && c.active.is_none());
        assert!(c.events.try_recv().is_err());
        assert_eq!(c.source_id, crate::local_models::PARAFORMER);
        assert!(c.message.contains("不可用"));
    }
    #[test]
    fn local_partials_are_coalesced_and_never_cross_generations_or_stop() {
        let mut c = Controller::new();
        c.automatic = false;
        let old = c.state.begin().unwrap();
        c.state.permissions(old, true);
        c.state.cancel(false);
        let current = c.state.begin().unwrap();
        c.state.permissions(current, true);
        *c.partial.lock().unwrap() = Some((old, "stale".into()));
        c.poll();
        assert!(c.transcript.is_empty());
        *c.partial.lock().unwrap() = Some((current, "current".into()));
        c.poll();
        assert_eq!(c.transcript, "current");
        c.state.transcribe();
        *c.partial.lock().unwrap() = Some((current, "late".into()));
        c.poll();
        assert_eq!(c.transcript, "current");
        assert!(c.partial.lock().unwrap().is_none());
    }
    #[test]
    fn local_cancel_during_loading_waits_for_matching_release_and_allows_restart() {
        let mut c = Controller::new();
        let generation = c.state.begin().unwrap();
        c.state.permissions(generation, true);
        c.recorded_source = local_source();
        let cancel = CancellationToken::new();
        c.active = Some(ActiveJob {
            generation,
            cancel: cancel.clone(),
            custody: Custody::default(),
            _recording: None,
        });
        let (jobs, _incoming) = mpsc::sync_channel(1);
        let (outgoing, results) = mpsc::channel();
        c.worker = Some(Worker { jobs, results });
        c.cancel();
        assert!(cancel.is_cancelled());
        assert_eq!(c.state.phase, Phase::Stopping);
        assert!(c.state.begin().is_none());
        outgoing
            .send(Completion {
                generation,
                result: Err("cancelled".into()),
            })
            .unwrap();
        c.poll();
        assert_eq!(c.state.phase, Phase::Idle);
        assert!(c.active.is_none());
        let next = c.state.begin().unwrap();
        assert_ne!(next, generation);
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

    #[test]
    fn rejected_start_keeps_previous_text_but_cannot_look_like_completed_recording() {
        let mut c = Controller::new();
        c.state.phase = Phase::Ready;
        c.state.retry_available = true;
        c.transcript = "previous synthetic result".into();
        let generation = c.state.generation();
        c.reject_start("目标应用发生变化。");
        assert_eq!(c.state.phase, Phase::Failed);
        assert!(!c.state.retry_available);
        assert_eq!(c.state.generation(), generation);
        assert_eq!(c.transcript, "previous synthetic result");
        assert!(c.message.contains("本轮未开始录音"));
        assert!(c.active.is_none());
        assert!(c.recording.is_none());
    }

    // Real Controller/Job/Custody and a real synthetic Recording/flock. Only the
    // transcription service is substituted: it installs a fresh owned child in
    // the pre-dispatch custody and models an already-unconfirmed cleanup. Native
    // termination/budget semantics are tested separately in plugin_process.
    struct CleanupRecording {
        path: std::path::PathBuf,
        recording: std::sync::Weak<Recording>,
        lease: std::fs::File,
        _temp: tempfile::TempDir,
    }
    impl CleanupRecording {
        fn install(c: &mut Controller) -> Self {
            let temp = tempfile::tempdir().unwrap();
            let recording = Arc::new(
                crate::storage::Storage::at(
                    temp.path()
                        .canonicalize()
                        .unwrap()
                        .join(crate::storage::NAMESPACE),
                )
                .create_recording()
                .unwrap(),
            );
            let path = recording.path().to_owned();
            let lease = std::fs::File::open(path.parent().unwrap().join("lease")).unwrap();
            let weak = Arc::downgrade(&recording);
            c.recording = Some(recording);
            Self {
                path,
                recording: weak,
                lease,
                _temp: temp,
            }
        }
        fn held(&self) -> bool {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(self.lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                assert_eq!(
                    unsafe { libc::flock(self.lease.as_raw_fd(), libc::LOCK_UN) },
                    0
                );
                false
            } else {
                assert_eq!(
                    std::io::Error::last_os_error().raw_os_error(),
                    Some(libc::EWOULDBLOCK)
                );
                true
            }
        }
        fn assert_retained(&self) {
            assert!(self.recording.upgrade().is_some() && self.path.exists() && self.held());
        }
        fn assert_released(&self) {
            assert!(self.recording.upgrade().is_none(), "recording still owned");
            assert!(!self.path.exists(), "recording file still exists");
            // A concurrent test's fork can inherit the lease until exec closes
            // CLOEXEC descriptors. Ownership and unlink must be immediate; only
            // this process-wide descriptor inheritance gets a bounded wait.
            let deadline = Instant::now() + std::time::Duration::from_secs(2);
            while self.held() {
                assert!(Instant::now() < deadline, "recording lease still held");
                std::thread::yield_now();
            }
        }
    }
    fn dispatch_cleanup_control(
        c: &mut Controller,
        mode: &'static str,
    ) -> (
        crate::plugin_process::cleanup_fixture::PendingChild,
        Sender<()>,
        std::thread::JoinHandle<()>,
    ) {
        use std::time::Duration;
        let (jobs, incoming) = mpsc::sync_channel::<Job>(1);
        let (outgoing, results) = mpsc::channel();
        let (ready_tx, ready) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            let job = incoming.recv_timeout(Duration::from_secs(2)).unwrap();
            let child = crate::plugin_process::cleanup_fixture::PendingChild::in_custody(
                job.custody.clone(),
            );
            ready_tx
                .send(child)
                .unwrap_or_else(|_| panic!("fixture receiver disappeared"));
            gate.recv_timeout(Duration::from_secs(2)).unwrap();
            if mode == "panic" {
                panic!("deliberate worker unwind with a retained cleanup owner");
            }
            let generation = job.generation;
            let failure = || Failure {
                message: "插件转写超时。".into(),
                cleanup: Some(job.custody.clone()),
            };
            if mode == "stale" {
                outgoing
                    .send(Completion {
                        generation: generation + 1,
                        result: Err(failure()),
                    })
                    .unwrap();
            }
            let result = if mode == "success" {
                Ok("must never be delivered".into())
            } else {
                Err(failure())
            };
            drop(job);
            let sent = outgoing.send(Completion { generation, result });
            if mode == "send-failure" {
                assert!(sent.is_err());
            } else {
                assert!(sent.is_ok());
            }
        });
        c.active = None;
        c.recorded_source = local_source(); // No proxy, auth, model load or network.
        c.worker = Some(Worker { jobs, results });
        c.submit(); // The real dispatch establishes custody and lease first.
        let child = ready.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            c.active
                .as_ref()
                .unwrap()
                .custody
                .same_owner(&child.custody)
        );
        (child, release, thread)
    }
    fn assert_cleanup_blocks(c: &mut Controller, recording: &CleanupRecording) {
        let generation = c.state.generation();
        assert_eq!(c.state.phase, Phase::Stopping); // Also blocks normal App termination.
        assert!(c.busy() && !c.state.can_start() && !c.state.retry_available);
        assert!(c.active.is_some() && c.cleanup.is_some());
        assert!(!c.allow_delivery && !c.delivery.has_pending());
        assert!(c.message.contains("超时") && c.message.contains("无法确认"));
        assert!(c.message.contains("再次点取消") && !c.message.contains("听写已取消"));
        c.start();
        c.retry();
        c.toggle();
        c.test_source();
        assert_eq!(c.state.generation(), generation);
        assert_eq!(c.state.phase, Phase::Stopping);
        assert!(c.transcript.is_empty());
        recording.assert_retained();
    }
    fn await_single_cleanup_check(c: &mut Controller) {
        // Poll only transports ONE explicitly requested async observation. It
        // neither queries native state nor starts another observation itself.
        let deadline = Instant::now() + std::time::Duration::from_secs(2);
        while c
            .cleanup
            .as_ref()
            .is_some_and(|block| block.observation.is_some())
        {
            assert!(
                Instant::now() < deadline,
                "single observation thread did not return"
            );
            c.poll();
            std::thread::yield_now();
        }
    }
    #[test]
    fn unresolved_cleanup_must_not_reopen_admission() {
        let mut c = transcribing();
        let recording = CleanupRecording::install(&mut c);
        let (child, release, thread) = dispatch_cleanup_control(&mut c, "error");
        c.cancel();
        release.send(()).unwrap();
        thread.join().unwrap();
        c.poll();
        assert_cleanup_blocks(&mut c, &recording);
        c.poll(); // Disconnected completion channel must retain the same owner.
        assert_cleanup_blocks(&mut c, &recording);
        child.while_locked(|| {
            for _ in 0..8 {
                c.cancel();
            }
            assert!(c.cleanup.as_ref().unwrap().observation.is_some());
            await_single_cleanup_check(&mut c);
        });
        assert_eq!(child.observations(), 1); // No queue, native wait or hidden retry.
        assert_cleanup_blocks(&mut c, &recording);
        c.poll();
        c.poll();
        assert_eq!(child.observations(), 1);
        child.terminate(); // Fixture owner alone signals its fresh child.
        c.cancel();
        await_single_cleanup_check(&mut c);
        assert_eq!(child.observations(), 2);
        assert!(c.cleanup.is_none() && c.active.is_none());
        assert_eq!(c.state.phase, Phase::Idle);
        assert!(c.message.contains("超时") && c.message.contains("清理已确认"));
        assert!(c.transcript.is_empty());
        recording.assert_released();
    }
    #[test]
    fn unresolved_normal_error_retains_recording_until_explicit_confirmation() {
        let mut c = transcribing();
        let recording = CleanupRecording::install(&mut c);
        let (child, release, thread) = dispatch_cleanup_control(&mut c, "stale");
        release.send(()).unwrap();
        thread.join().unwrap();
        c.poll(); // Stale typed completion cannot discard the existing owner.
        assert!(c.active.as_ref().unwrap().custody.unresolved());
        recording.assert_retained();
        c.poll();
        assert_cleanup_blocks(&mut c, &recording);
        child.terminate();
        c.poll(); // Terminal OS state alone does not request reconciliation.
        assert_eq!(child.observations(), 0);
        assert_cleanup_blocks(&mut c, &recording);
        c.cancel();
        await_single_cleanup_check(&mut c);
        assert_eq!(c.state.phase, Phase::Failed);
        assert!(c.state.retry_available && c.state.can_start());
        assert!(c.message.contains("超时") && c.transcript.is_empty());
        recording.assert_retained(); // Original ordinary-error retention policy.
        c.cancel();
        recording.assert_released();
    }
    #[test]
    fn worker_unwind_and_send_failure_cannot_discard_cleanup_or_recording() {
        for mode in ["panic", "send-failure"] {
            let mut c = transcribing();
            let recording = CleanupRecording::install(&mut c);
            let (child, release, thread) = dispatch_cleanup_control(&mut c, mode);
            c.cancel();
            if mode == "send-failure" {
                c.worker = None;
            }
            release.send(()).unwrap();
            let joined = thread.join();
            assert_eq!(joined.is_err(), mode == "panic");
            if mode == "send-failure" {
                c.worker = Some(disconnected_worker());
            }
            c.poll();
            assert_cleanup_blocks(&mut c, &recording);
            child.terminate();
            c.cancel();
            await_single_cleanup_check(&mut c);
            assert_eq!(c.state.phase, Phase::Idle);
            recording.assert_released();
        }
    }
    #[test]
    fn unconfirmed_success_and_duplicate_completion_never_deliver_or_release() {
        let mut c = transcribing();
        let recording = CleanupRecording::install(&mut c);
        let (child, release, thread) = dispatch_cleanup_control(&mut c, "success");
        release.send(()).unwrap();
        thread.join().unwrap();
        c.poll();
        assert_cleanup_blocks(&mut c, &recording);
        let generation = c.active.as_ref().unwrap().generation;
        let (jobs, _incoming) = mpsc::sync_channel(1);
        let (outgoing, results) = mpsc::channel();
        outgoing
            .send(Completion {
                generation,
                result: Ok("duplicate must not deliver".into()),
            })
            .unwrap();
        c.worker = Some(Worker { jobs, results });
        c.poll();
        assert_cleanup_blocks(&mut c, &recording);
        child.terminate();
        c.cancel();
        await_single_cleanup_check(&mut c);
        assert_eq!(c.state.phase, Phase::Failed);
        assert!(c.transcript.is_empty());
        c.cancel();
        recording.assert_released();
    }
    #[test]
    fn cleanup_observation_result_cannot_release_a_different_owner_or_generation() {
        let mut c = transcribing();
        let recording = CleanupRecording::install(&mut c);
        let (child, release, thread) = dispatch_cleanup_control(&mut c, "error");
        release.send(()).unwrap();
        thread.join().unwrap();
        c.poll();
        let original = c.active.as_ref().unwrap().custody.clone();
        // Inject a stale observation receipt; even true cannot clear another
        // owner, an unresolved owner, or a mismatched generation.
        for mismatch in ["pending", "owner", "generation"] {
            let (sender, receiver) = mpsc::sync_channel(1);
            c.cleanup.as_mut().unwrap().observation = Some(receiver);
            match mismatch {
                "owner" => c.active.as_mut().unwrap().custody = Custody::default(),
                "generation" => {
                    c.active.as_mut().unwrap().generation += 1;
                }
                _ => {}
            }
            sender.send(true).unwrap();
            c.poll();
            assert!(c.cleanup.is_some() && c.active.is_some() && c.busy());
            c.active.as_mut().unwrap().custody = original.clone();
            c.active.as_mut().unwrap().generation = c.cleanup.as_ref().unwrap().generation;
            recording.assert_retained();
        }
        child.terminate();
        assert!(child.custody.observe_once()); // Explicit terminal evidence for this owner.
        let (sender, receiver) = mpsc::sync_channel(1);
        c.cleanup.as_mut().unwrap().observation = Some(receiver);
        c.active.as_mut().unwrap().generation += 1;
        sender.send(true).unwrap();
        c.poll();
        assert!(c.cleanup.is_some() && c.active.is_some() && c.busy());
        recording.assert_retained();
        c.active.as_mut().unwrap().generation = c.cleanup.as_ref().unwrap().generation;
        c.cancel();
        await_single_cleanup_check(&mut c);
        c.cancel();
        recording.assert_released();
    }
}
