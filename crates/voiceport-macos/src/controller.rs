//! Main-thread orchestration; worker completion follows release of its recording lease.
use crate::{
    audio::{AudioEvent, Recorder},
    delivery::Delivery,
    lifecycle::{Lifecycle, Phase},
    local_audio::Incoming,
    local_models::{CHATGPT, Catalog, SessionSource},
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
                while let Ok(mut job) = incoming.recv() {
                    let generation = job.generation;
                    // Unwind on the worker only. Resources, runtime and file clone are all
                    // gone before completion is published; the UI never joins this thread.
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        match &job.source {
                            SessionSource::Plugin(plugin) => {
                                return if job.sample {
                                    plugin.execute(
                                        crate::plugin_runtime::sample(),
                                        &job.language,
                                        &job.cancel,
                                    )
                                } else {
                                    plugin.transcribe(
                                        job.recording.as_ref().ok_or("录音已不可用。")?.path(),
                                        &job.language,
                                        &job.cancel,
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
                                };
                            }
                            SessionSource::ChatGpt => {}
                        }
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
        self.testing = false;
        self.recorded_language = self.language.clone();
        self.recorded_source = source;
        self.allow_delivery = self.automatic;
        let use_speech = self.recorded_source.uses_system_speech();
        self.message = if use_speech {
            "正在检查麦克风和本机语音识别权限…"
        } else {
            "正在检查麦克风权限；此来源无需系统语音识别权限…"
        }
        .into();
        Recorder::request_permissions_for_source(generation, self.events_tx.clone(), use_speech);
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
        self.recorded_language = self.language.clone();
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
            let job = Job {
                generation: self.state.generation(),
                recording,
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
        if self.delivery.has_pending() && matches!(self.state.phase, Phase::Ready | Phase::Failed) {
            self.delivery_settle = Some(self.state.phase);
            self.state.phase = Phase::Delivering;
        }
    }
    pub fn cancel(&mut self) {
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
    }
    pub fn poll(&mut self) {
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
                                self.message = "正在录音；再次按快捷键停止。".into();
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

    // Tests use inert device/delivery constructors and synthetic leased storage.
    // None requests permissions, starts capture/inference, or contacts a service.
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
}
