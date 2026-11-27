//! Main-thread orchestration; cleanup confirmation gates recording release and admission.
use crate::{
    audio::{
        AudioEvent, Recorder,
        diagnostics::{CaptureDiagnostics, InputSignal, StageTimer, StageTimings},
        worker::{self as audio, Capture, Reply as CaptureReply},
    },
    delivery::Delivery,
    dictation_output::OutputPipeline,
    dictation_workflows::Workflows,
    lifecycle::{Lifecycle, Phase},
    local_audio::Incoming,
    local_models::{CHATGPT, Catalog, SessionSource},
    plugin_process::{CLEANUP_WARNING, Custody, Failure},
    storage::Recording,
};
#[path = "controller/dictation_result.rs"]
mod dictation_result;
use dictation_result::{GuiResultContext, RetainedGuiResult};
#[cfg(test)]
#[path = "controller/dictation_workflow_tests.rs"]
mod dictation_workflow_tests;
#[path = "controller/latest_output.rs"]
mod latest_output;
#[path = "controller/recent_dictations.rs"]
pub(crate) mod recent_dictations;
use latest_output::LatestOutput;

use std::sync::Mutex;
use std::{
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender, SyncSender, TryRecvError, TrySendError},
    },
    time::{Duration, Instant},
};
type PartialSlot = Arc<Mutex<Option<(u64, String)>>>;
use voiceport_control::{
    CaptureSignal, CaptureSummary, DictationBlockReason as BlockReason, DictationTimings,
    GuiDictationDiagnostics, GuiDictationReceipt, TranscriptionState,
};
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
    elapsed: Option<Duration>,
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
                    let started = StageTimer::start(Instant::now);
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
                    let elapsed = started.finish(Instant::now);
                    drop(job);
                    if outgoing
                        .send(Completion {
                            generation,
                            elapsed,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })
            .map_err(|_| "无法启动转写工作线程。".to_string())?;
        Ok(Self { jobs, results })
    }
}

#[derive(Default)]
struct GuiDiagnostics {
    capture_pending: Option<u64>,
    capture: Option<CaptureSummary>,
    preparation: Option<StageTimer>,
    delivery: Option<StageTimer>,
    timings: StageTimings,
}

impl GuiDiagnostics {
    fn projection(&self, receipt: &GuiDictationReceipt) -> GuiDictationDiagnostics {
        fn millis(duration: Option<Duration>) -> Option<u64> {
            duration.and_then(|duration| u64::try_from(duration.as_millis()).ok())
        }
        GuiDictationDiagnostics {
            session: receipt.session,
            generation: receipt.generation,
            capture: self.capture,
            timings: DictationTimings {
                capture_first_frame_ms: millis(self.timings.capture_first_frame),
                preparation_ms: millis(self.timings.preparation),
                transcription_ms: millis(self.timings.transcription),
                delivery_ms: millis(self.timings.delivery),
            },
        }
    }

    fn finish_preparation(&mut self) {
        if let Some(started) = self.preparation.take() {
            self.timings.preparation = started.finish(Instant::now);
        }
    }

    fn finish_delivery_if_settled(&mut self, delivery: &Delivery) {
        if !delivery.has_pending()
            && let Some(started) = self.delivery.take()
        {
            self.timings.delivery = started.finish(Instant::now);
        }
    }

    fn abandon_stages(&mut self) {
        self.preparation = None;
        self.delivery = None;
    }
}

/// Origin of the display buffer, not a new transcript store or delivery receipt.
#[derive(Clone, Copy)]
pub(crate) enum VisibleTranscript {
    Preview(u64),
    Final(u64),
}

pub struct Controller {
    pub state: Lifecycle,
    pub transcript: String,
    pub(crate) visible_transcript: Option<VisibleTranscript>,
    pub(crate) raw_transcript: Option<String>,
    pub(crate) workflows: Workflows,
    recorded_output: OutputPipeline,
    pub message: String,
    pub preview_status: String,
    pub delivery_status: String,
    pub automatic: bool,
    pub language: String,
    pub source_id: String,
    pub catalog: Catalog,
    pub source_tests: crate::source_tests::SourceTests,
    pub(crate) file_jobs: crate::file_transcription::Jobs,
    discovery: Option<Receiver<Catalog>>,
    recorded_source: SessionSource,
    partial: PartialSlot,
    #[cfg(test)]
    recorder: Recorder,
    audio_lane: audio::Lane,
    capture: Option<Capture>,
    capture_incoming: Option<Incoming>,
    capture_route: Option<String>,
    capture_recovered: bool,
    capture_preview_status: Option<String>,
    capture_stop: Option<Instant>,
    input_check: Option<(u64, Instant)>,
    deferred_completion: Option<Completion>,
    pub(crate) microphone_uid: Option<String>,
    recorded_microphone_uid: Option<String>,
    microphone_test: Option<crate::audio::level::Job>,
    microphone_test_status: String,
    last_microphone_test: Option<String>,
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
    gui_receipt: Option<GuiDictationReceipt>,
    gui_result_context: GuiResultContext,
    retained_gui_result: Option<RetainedGuiResult>,
    recent_dictations: recent_dictations::RecentDictations,
    latest_output: LatestOutput,
    gui_owns_delivery: bool,
    overlay_origin_pid: Option<i32>,
    gui_diagnostics: GuiDiagnostics,
    stop_failure: Option<String>,
    testing: bool,
}
#[cfg(debug_assertions)]
pub(crate) struct AudioFixtureAsr {
    jobs: Receiver<Job>,
    results: Sender<Completion>,
}

#[cfg(debug_assertions)]
impl AudioFixtureAsr {
    pub fn assert_no_job(&self) {
        assert!(matches!(self.jobs.try_recv(), Err(TryRecvError::Empty)));
    }

    pub fn complete(&self, generation: u64) -> bool {
        let job = match self.jobs.try_recv() {
            Ok(job) => job,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => panic!("fixture ASR worker disconnected"),
        };
        assert_eq!(job.generation, generation);
        assert!(!job.sample && job.incoming.is_none());
        job.recording
            .as_ref()
            .unwrap()
            .validate_finalized()
            .unwrap();
        self.results
            .send(Completion {
                generation,
                elapsed: None,
                result: Ok("Synthetic capture result".into()),
            })
            .unwrap();
        true
    }
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
            visible_transcript: None,
            raw_transcript: None,
            workflows: Workflows::default(),
            recorded_output: OutputPipeline::default(),
            message: "聚焦输入框，按快捷键开始听写。".into(),
            preview_status: "本机预览尚未启动".into(),
            delivery_status: String::new(),
            automatic: true,
            language: String::new(),
            source_id: CHATGPT.into(),
            catalog: Catalog::pending(CHATGPT),
            source_tests: crate::source_tests::SourceTests::new(),
            file_jobs: crate::file_transcription::Jobs::default(),
            discovery: None,
            recorded_source: SessionSource::ChatGpt,
            partial: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            recorder: Recorder::new(),
            audio_lane: audio::Lane::default(),
            capture: None,
            capture_incoming: None,
            capture_route: None,
            capture_recovered: false,
            capture_preview_status: None,
            capture_stop: None,
            input_check: None,
            deferred_completion: None,
            microphone_uid: None,
            recorded_microphone_uid: None,
            microphone_test: None,
            microphone_test_status: String::new(),
            last_microphone_test: None,
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
            gui_receipt: None,
            gui_result_context: GuiResultContext::default(),
            retained_gui_result: None,
            recent_dictations: recent_dictations::RecentDictations::default(),
            latest_output: LatestOutput::default(),
            gui_owns_delivery: false,
            overlay_origin_pid: None,
            gui_diagnostics: GuiDiagnostics::default(),
            stop_failure: None,
            testing: false,
        }
    }
    pub(crate) fn set_stop_shortcut(
        &mut self,
        shortcut: Option<crate::delivery::model::StopShortcut>,
    ) {
        self.delivery.set_stop_shortcut(shortcut);
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
    pub(crate) fn start_prepared_file(
        &mut self,
        input: crate::file_transcription::Input,
        plan: crate::file_transcription::Plan,
        accept_upload: bool,
    ) -> Result<crate::file_transcription::Status, crate::file_transcription::Error> {
        if self.busy() {
            return Err(crate::file_transcription::BUSY);
        }
        plan.verify_source(&self.catalog)?;
        self.file_jobs.start(input, plan, accept_upload)
    }
    pub fn busy(&self) -> bool {
        self.file_jobs.busy()
            || !self.state.can_start()
            || self.delivery.has_pending()
            || self.latest_output.has_pending()
            || self.source_tests.busy()
            || self.microphone_test.is_some()
    }
    /// Test concrete source IDs without changing preferences, transcript or lifecycle.
    pub fn start_source_tests(&mut self, ids: &[String]) -> Result<(), String> {
        self.start_source_tests_with_language(ids, self.language.clone())
    }
    pub(crate) fn start_source_tests_with_language(
        &mut self,
        ids: &[String],
        language: String,
    ) -> Result<(), String> {
        if !voiceport_core::language::supported(&language) {
            return Err("Unsupported source test language.".into());
        }
        if self.busy() || self.discovering() {
            return Err("请等待当前任务完成后再测试来源。".into());
        }
        let entries = ids
            .iter()
            .map(|id| {
                self.catalog
                    .entries
                    .iter()
                    .find(|entry| &entry.id == id)
                    .cloned()
                    .ok_or_else(|| "来源列表已变化，请重新选择测试目标。".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.source_tests.start(entries, language)?;
        self.message = self.source_tests.summary();
        Ok(())
    }
    pub(crate) fn input_failure_stage(&self) -> Option<&'static str> {
        self.delivery.failure_stage()
    }
    pub(crate) fn input_method(&self) -> Option<&'static str> {
        self.delivery.method()
    }
    fn foreground_pid() -> Option<i32> {
        objc2_app_kit::NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .map(|app| app.processIdentifier())
    }

    pub(crate) fn capture_level(&self) -> Option<u8> {
        if let Some(capture) = &self.capture {
            return capture.level();
        }
        #[cfg(test)]
        if let Some(level) = self.recorder.live_level() {
            return Some(level);
        }
        None
    }

    pub(crate) fn overlay_snapshot(&self) -> crate::status_overlay::Snapshot {
        let attempt = self
            .gui_receipt
            .as_ref()
            .filter(|_| !self.gui_owns_delivery || !self.delivery.original_focus_changed());
        crate::status_overlay::Snapshot::project(
            &self.state,
            attempt.map(|receipt| (receipt.generation, receipt.transcription)),
            self.gui_owns_delivery && self.delivery.has_pending(),
            self.overlay_origin_pid,
            self.capture_stage(),
        )
    }

    pub(crate) fn capture_stage(&self) -> Option<crate::status_overlay::CaptureStage> {
        use crate::status_overlay::CaptureStage;
        self.capture.as_ref()?;
        match self.state.phase {
            Phase::Permissions => Some(CaptureStage::Opening),
            Phase::Recording => Some(
                match (self.capture_route.is_some(), self.capture_recovered) {
                    (true, false) => CaptureStage::Recording,
                    (false, false) => CaptureStage::Recovering,
                    (true, true) => CaptureStage::Resumed,
                    (false, true) => CaptureStage::Disconnected,
                },
            ),
            Phase::Transcribing if self.finalizing_capture() => Some(CaptureStage::Finalizing),
            _ => None,
        }
    }

    /// The native slot can outlive its UI operation. Read it on each refresh so
    /// the waiting text disappears on release without starting another capture.
    pub(crate) fn audio_release_notice(&self) -> Option<&'static str> {
        (self.capture.is_none() && !self.microphone_test_running() && self.audio_input_busy())
            .then_some(crate::status_overlay::WAITING_FOR_AUDIO_RELEASE)
    }

    /// Read the latest GUI session, even after another CLI operation takes over.
    pub(crate) fn gui_dictation_receipt(&self) -> Option<GuiDictationReceipt> {
        let mut receipt = self.gui_receipt.clone()?;
        if self.gui_owns_delivery {
            receipt.delivery = self.delivery.receipt();
            receipt.method = self.delivery.method().map(str::to_owned);
            receipt.failure_stage = self.delivery.failure_stage().map(str::to_owned);
        }
        Some(receipt)
    }
    pub(crate) fn gui_dictation_diagnostics(&self) -> Option<GuiDictationDiagnostics> {
        self.gui_receipt
            .as_ref()
            .map(|receipt| self.gui_diagnostics.projection(receipt))
    }
    pub(crate) fn gui_result_context(&self) -> &GuiResultContext {
        &self.gui_result_context
    }
    pub(crate) fn active_app_rule(&self) -> Option<&str> {
        let receipt = self.gui_receipt.as_ref()?;
        if receipt.generation == self.state.generation()
            && matches!(
                self.state.phase,
                Phase::Permissions | Phase::Recording | Phase::Transcribing | Phase::Delivering
            )
        {
            self.gui_result_context.app_rule.as_deref()
        } else {
            None
        }
    }
    pub(crate) fn retained_gui_result(&self) -> Option<&RetainedGuiResult> {
        self.retained_gui_result.as_ref().filter(|result| {
            matches!(self.visible_transcript, Some(VisibleTranscript::Final(generation))
                if generation == result.receipt.generation)
        })
    }
    fn seal_gui_capture(&mut self, capture: Option<CaptureDiagnostics>) {
        if let Some(generation) = self.gui_diagnostics.capture_pending
            && let Some(capture) = capture
        {
            self.gui_diagnostics.capture_pending = None;
            self.gui_diagnostics.timings.capture_first_frame = capture.timings.capture_first_frame;
            self.gui_diagnostics.capture = Some(CaptureSummary {
                generation,
                signal: match capture.signal {
                    InputSignal::NoCallbacks => CaptureSignal::NoCallbacks,
                    InputSignal::NoFrames => CaptureSignal::NoFrames,
                    InputSignal::DigitalSilence => CaptureSignal::DigitalSilence,
                    InputSignal::LowEnergy => CaptureSignal::LowEnergy,
                    InputSignal::Signal => CaptureSignal::Signal,
                    InputSignal::Unknown => CaptureSignal::Unknown,
                },
                callbacks: capture.callbacks,
                observed_frames: capture.observed_frames,
            });
        }
    }
    fn cancel_capture(&mut self) {
        if self.capture.take().is_some() {
            self.seal_gui_capture(Some(CaptureDiagnostics::default()));
        }
        self.capture_incoming = None;
        self.capture_route = None;
        self.capture_preview_status = None;
        self.capture_stop = None;
        self.input_check = None;
        // Keep a deferred ASR completion until its active job is settled.
        #[cfg(test)]
        {
            self.recorder.cancel();
            self.seal_gui_capture(self.recorder.capture_diagnostics());
        }
    }
    #[cfg(test)]
    fn stop_fixture_capture(&mut self) -> Result<Arc<Recording>, String> {
        let result = self.recorder.stop();
        self.seal_gui_capture(self.recorder.capture_diagnostics());
        result
    }
    #[cfg(debug_assertions)]
    pub(crate) fn install_audio_fixture(
        &mut self,
        fixture: crate::audio::devices::fixture::Backend,
    ) -> AudioFixtureAsr {
        self.audio_lane.fixture = Some(fixture);
        let (sender, jobs) = mpsc::sync_channel(1);
        let (results, receiver) = mpsc::channel();
        self.worker = Some(Worker {
            jobs: sender,
            results: receiver,
        });
        AudioFixtureAsr { jobs, results }
    }
    pub(crate) fn opening_microphone(&self) -> bool {
        self.state.phase == Phase::Permissions && self.capture.is_some()
    }
    pub(crate) fn finalizing_capture(&self) -> bool {
        self.capture_stop.is_some()
    }
    pub(crate) fn audio_input_busy(&self) -> bool {
        self.audio_lane.busy()
    }
    pub(crate) fn read_microphones(&self) -> Result<audio::Inventory, String> {
        self.audio_lane.inventory()
    }
    pub(crate) fn input_device_snapshot(
        &self,
    ) -> Result<audio::Inventory<audio::InputSnapshot>, String> {
        self.audio_lane.input_snapshot()
    }
    fn detach_gui_delivery(&mut self) {
        if self.gui_owns_delivery {
            self.gui_receipt = self.gui_dictation_receipt();
            self.gui_owns_delivery = false;
        }
    }
    fn begin_gui_receipt(&mut self, generation: u64, automatic: bool) {
        let context = std::mem::take(&mut self.gui_result_context);
        if let Some(receipt) = self.gui_dictation_receipt()
            && matches!(self.visible_transcript, Some(VisibleTranscript::Final(visible))
                if visible == receipt.generation)
        {
            self.retained_gui_result = Some(RetainedGuiResult {
                diagnostics: self.gui_diagnostics.projection(&receipt),
                receipt,
                context,
            });
        }
        self.gui_diagnostics = GuiDiagnostics::default();
        self.gui_receipt = Some(GuiDictationReceipt {
            session: generation,
            generation,
            automatic,
            transcription: TranscriptionState::Pending,
            delivery: Default::default(),
            method: None,
            failure_stage: None,
            block_reason: (!automatic).then_some(BlockReason::PanelOnly),
        });
        self.gui_owns_delivery = false;
    }
    fn current_gui_receipt(&mut self) -> Option<&mut GuiDictationReceipt> {
        self.gui_receipt
            .as_mut()
            .filter(|receipt| receipt.generation == self.state.generation())
    }
    fn report_gui_transcription(
        &mut self,
        transcription: TranscriptionState,
        reason: Option<BlockReason>,
    ) {
        if let Some(receipt) = self.current_gui_receipt()
            && receipt.transcription == TranscriptionState::Pending
        {
            receipt.transcription = transcription;
            if reason.is_some() {
                receipt.block_reason = reason;
            }
        }
    }
    pub(crate) fn reject_start(&mut self, message: &str) {
        self.reject_gui_start(message, BlockReason::StartRejected, self.automatic);
    }
    fn reject_gui_start(&mut self, message: &str, reason: BlockReason, automatic: bool) {
        if self.busy() {
            return;
        }
        if let Some(generation) = self.state.begin() {
            self.begin_gui_receipt(generation, automatic);
            self.report_gui_transcription(TranscriptionState::NotStarted, Some(reason));
        }
        self.mark_start_rejected(message);
    }
    fn mark_start_rejected(&mut self, message: &str) {
        self.state.phase = Phase::Failed;
        self.state.retry_available = false;
        self.message = format!("{message} 本轮未开始录音，已有文字保留在结果区。");
    }
    /// Delivery feedback belongs only to the current automatic session.
    pub(crate) fn delivery_failure_stage(&self) -> Option<&'static str> {
        (self.allow_delivery && !self.testing)
            .then(|| self.delivery.failure_stage())
            .flatten()
    }
    pub fn toggle(&mut self) {
        if self.state.phase == Phase::Recording || self.opening_microphone() {
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
        if self.busy() {
            return;
        }
        if self.audio_input_busy() {
            self.message = audio::AUDIO_BUSY.into();
            return;
        }
        let source = match self
            .catalog
            .snapshot(&self.source_id)
            .and_then(SessionSource::pin)
        {
            Ok(source) => source,
            Err(error) => {
                self.reject_gui_start(&error, BlockReason::SourceUnavailable, self.automatic);
                return;
            }
        };
        let origin = target.map(|target| target.pid);
        #[cfg(debug_assertions)]
        let fixture = self.audio_lane.fixture.is_some();
        #[cfg(not(debug_assertions))]
        let fixture = false;
        let origin = if fixture {
            origin
        } else {
            origin.or_else(Self::foreground_pid)
        };
        if let Some((generation, use_speech)) = self.prepare_start_in_application(
            source,
            self.language.clone(),
            self.automatic,
            target,
            true,
        ) {
            self.overlay_origin_pid = origin;
            #[cfg(debug_assertions)]
            if self.audio_lane.fixture.is_some() {
                let _ = self.events_tx.send((
                    generation,
                    AudioEvent::Permissions {
                        microphone: self.capture_permission_status().0 == "authorized",
                        speech: false,
                    },
                ));
                return;
            }
            Recorder::request_permissions_for_source(
                generation,
                self.events_tx.clone(),
                use_speech,
            );
        }
    }
    /// Start a background-pinned source without changing GUI preferences.
    /// Only an accepted start returns a generation that a CLI operation may own.
    pub(crate) fn start_pinned(
        &mut self,
        source: SessionSource,
        language: String,
        automatic: bool,
    ) -> Option<u64> {
        let permissions = self.capture_permission_status();
        self.start_pinned_with_permissions(source, language, automatic, || permissions)
    }
    pub(crate) fn accessibility_trusted(&self) -> bool {
        #[cfg(debug_assertions)]
        if self.audio_lane.fixture.is_some() {
            return false;
        }
        Delivery::trusted()
    }

    pub(crate) fn capture_permission_status(&self) -> (&'static str, &'static str) {
        #[cfg(debug_assertions)]
        if let Some(fixture) = &self.audio_lane.fixture {
            return (
                if fixture.authorized() {
                    "authorized"
                } else {
                    "denied"
                },
                "denied",
            );
        }
        Recorder::permission_status()
    }

    pub(crate) fn start_pinned_with_permissions(
        &mut self,
        source: SessionSource,
        language: String,
        automatic: bool,
        permissions: impl FnOnce() -> (&'static str, &'static str),
    ) -> Option<u64> {
        let (generation, use_speech) = self.prepare_start(source, language, automatic)?;
        // CLI operations consume existing grants only. Speech is optional;
        // an undecided grant must not open an authorization dialog.
        let (microphone, speech) = permissions();
        let _ = self.events_tx.send((
            generation,
            AudioEvent::Permissions {
                microphone: microphone == "authorized",
                speech: use_speech && speech == "authorized",
            },
        ));
        Some(generation)
    }
    fn prepare_start(
        &mut self,
        source: SessionSource,
        language: String,
        automatic: bool,
    ) -> Option<(u64, bool)> {
        self.prepare_start_in_application(source, language, automatic, None, false)
    }
    fn prepare_start_in_application(
        &mut self,
        source: SessionSource,
        language: String,
        automatic: bool,
        target: Option<&crate::delivery::model::ApplicationTarget>,
        gui: bool,
    ) -> Option<(u64, bool)> {
        let mut original_application = None;
        let start = self.prepare_start_with(source, language, automatic, gui, |delivery| {
            delivery
                .begin_in_application_reporting(target, gui.then_some(&mut original_application))
        });
        if let Some(application) = original_application {
            self.gui_result_context.target = Some(application);
        }
        start
    }
    fn prepare_start_with(
        &mut self,
        source: SessionSource,
        language: String,
        automatic: bool,
        gui: bool,
        capture_target: impl FnOnce(&mut Delivery) -> Result<(), String>,
    ) -> Option<(u64, bool)> {
        if self.busy() {
            return None;
        }
        if self.audio_input_busy() {
            self.message = audio::AUDIO_BUSY.into();
            return None;
        }
        if !voiceport_core::language::supported(&language)
            || matches!(&source, SessionSource::Local(model) if !model.kind.supports_language(&language))
        {
            let message = "所选模型不支持此语言，请更改语言或模型。";
            if gui {
                self.reject_gui_start(message, BlockReason::UnsupportedLanguage, automatic);
            } else {
                self.message = message.into();
            }
            return None;
        }
        self.detach_gui_delivery();
        self.gui_diagnostics.abandon_stages();
        self.delivery.clear();
        if self.delivery.has_pending() {
            self.wait_for_delivery();
            self.message = "正在确认上一轮输入，请稍后重新开始。".into();
            return None;
        }
        let generation = self.state.begin()?;
        if gui {
            self.begin_gui_receipt(generation, automatic);
            self.gui_owns_delivery = automatic;
        }
        if automatic {
            if gui {
                self.gui_diagnostics.preparation = Some(StageTimer::start(Instant::now));
            }
            let target = capture_target(&mut self.delivery);
            if !self.delivery.preparing_input() {
                self.gui_diagnostics.finish_preparation();
            }
            if let Err(message) = target {
                self.delivery_status = message.clone();
                if self.delivery.blocks_recording() {
                    self.report_gui_transcription(TranscriptionState::NotStarted, None);
                    self.mark_start_rejected(&message);
                    return None;
                }
            }
        }
        if gui {
            self.gui_result_context =
                GuiResultContext::at_start(&source, &language, &self.catalog, &self.source_id);
            self.begin_recent_dictation(generation);
        } else {
            self.discard_recent_dictation_candidate();
        }
        self.cancel_capture();
        self.recording = None;
        self.recorded_output = if gui {
            self.workflows.output_for_session(
                self.workflows.dictionary_enabled,
                self.workflows.output_rules,
            )
        } else {
            OutputPipeline::default()
        };
        self.testing = false;
        self.recorded_language = language;
        self.recorded_source = source;
        self.recorded_microphone_uid = self.microphone_uid.clone();
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
    /// Legacy selected-plugin fixture; GUI tests now use independent source IDs.
    #[cfg(test)]
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
        let _ = self.test_pinned(source, self.language.clone());
    }
    pub(crate) fn test_pinned(&mut self, source: SessionSource, language: String) -> Option<u64> {
        if self.busy() {
            return None;
        }
        self.detach_gui_delivery();
        self.gui_diagnostics.abandon_stages();
        self.delivery.clear();
        self.recording = None;
        self.latest_output.clear_notice();
        self.transcript.clear();
        self.visible_transcript = None;
        self.retained_gui_result = None;
        self.raw_transcript = None;
        self.recorded_output = OutputPipeline::default();
        let generation = self.state.begin()?;
        self.discard_recent_dictation_candidate();
        self.state.permissions(generation, true);
        self.state.transcribe();
        self.testing = true;
        self.allow_delivery = false;
        self.recorded_source = source;
        self.recorded_language = language;
        self.submit();
        Some(generation)
    }
    pub fn stop(&mut self) {
        if self.opening_microphone() {
            self.cancel();
            return;
        }
        if self.state.phase != Phase::Recording || self.capture_stop.is_some() {
            return;
        }
        self.started = None;
        if self.recorded_output.is_identity() {
            self.delivery_status = self.delivery.finish_preview();
        } else if self.allow_delivery && self.delivery.failure_stage().is_none() {
            self.delivery_status = "文字处理完成后，再输入最终文字。".into();
        }
        if let Some(capture) = &self.capture {
            match capture.stop() {
                Ok(()) => {
                    self.capture_stop = Some(Instant::now() + audio::WAIT);
                    self.state.transcribe();
                    self.message = "正在停止麦克风并完成录音…".into();
                }
                Err(error) => self.fail_capture(error),
            }
            return;
        }
        #[cfg(test)]
        let recording = self.stop_fixture_capture();
        #[cfg(not(test))]
        let recording = Err("当前没有录音。".into());
        self.finish_capture(recording);
    }
    fn finish_capture(&mut self, recording: Result<Arc<Recording>, String>) {
        match recording {
            Ok(recording) => {
                self.recording = Some(recording);
                self.state.transcribe();
                if self.recorded_source.is_local() && self.active.is_some() {
                    self.message = "正在本地处理剩余音频并完成最终转写…".into();
                } else {
                    self.submit();
                }
            }
            Err(message) => self.fail_capture(message),
        }
    }
    fn fail_capture(&mut self, message: String) {
        // A local job keeps its lease until completion, but neither it nor a
        // partial AAC from an interrupted capture may be submitted for delivery.
        self.started = None;
        self.cancel_capture();
        self.recording = None;
        self.state.phase = Phase::Failed;
        if let Some(active) = &self.active {
            active.cancel.cancel();
            self.stop_failure = Some(message.clone());
            self.state.phase = Phase::Stopping;
        }
        self.state.retry_available = false;
        self.message = message;
        self.report_gui_transcription(TranscriptionState::Failed, Some(BlockReason::CaptureFailed));
        self.gui_diagnostics.abandon_stages();
        self.delivery.retain_preview();
        self.wait_for_delivery();
    }
    pub(crate) fn microphone_route(&self) -> Option<String> {
        self.capture_route.clone()
    }
    pub(crate) fn start_microphone_test(&mut self) {
        if self.busy() {
            return;
        }
        match crate::audio::level::Job::start(&self.audio_lane, self.microphone_uid.clone()) {
            Ok(test) => {
                self.microphone_test_status = "正在打开麦克风…".into();
                self.last_microphone_test = Some(test.id.clone());
                self.microphone_test = Some(test);
                self.poll_microphone_test();
            }
            Err(error) => self.microphone_test_status = error,
        }
    }
    pub(crate) fn microphone_test_running(&self) -> bool {
        self.microphone_test.is_some()
    }
    pub(crate) fn microphone_test_measuring(&self) -> bool {
        self.microphone_test
            .as_ref()
            .is_some_and(crate::audio::level::Job::measuring)
    }
    pub(crate) fn microphone_test_id(&self) -> Option<&str> {
        self.microphone_test.as_ref().map(|test| test.id.as_str())
    }
    pub(crate) fn microphone_test_status(&self) -> &str {
        &self.microphone_test_status
    }
    pub(crate) fn microphone_test_display_is_stale(&self, now: Instant) -> bool {
        self.microphone_test
            .as_ref()
            .is_some_and(|test| test.display_is_stale(now))
    }
    pub(crate) fn microphone_test_detail(&self, operation: Option<&str>) -> Option<&str> {
        operation
            .filter(|id| Some(*id) == self.last_microphone_test.as_deref())
            .map(|_| self.microphone_test_status.as_str())
    }
    pub(crate) fn stop_microphone_test(&mut self) {
        if self.microphone_test.take().is_some() {
            self.microphone_test_status = "电平测试已停止。".into();
        }
    }
    fn poll_microphone_test(&mut self) {
        if let Some(test) = self.microphone_test.as_mut() {
            match test.status(Instant::now()) {
                Ok(Some(status)) => self.microphone_test_status = status,
                Ok(None) => {}
                Err(message) => {
                    self.stop_microphone_test();
                    self.microphone_test_status = message;
                }
            }
        }
    }
    pub(crate) fn has_retained_recording(&self) -> bool {
        self.recording.is_some()
    }
    pub(crate) fn can_retry_recording(&self) -> bool {
        !self.busy()
            && self.active.is_none()
            && self.cleanup.is_none()
            && !self.testing
            && self.recording.is_some()
            && self.state.phase == Phase::Failed
            && self.state.retry_available
    }
    pub fn retry(&mut self) {
        if !self.can_retry_recording() {
            return;
        }
        let gui = self.current_gui_receipt().is_some();
        let previous = self.state.generation();
        let Some(generation) = self.state.retry() else {
            return;
        };
        self.detach_gui_delivery();
        if gui && let Some(receipt) = self.gui_receipt.as_mut() {
            receipt.generation = generation;
            receipt.transcription = TranscriptionState::Pending;
            receipt.block_reason = Some(BlockReason::RetryPanelOnly);
            self.overlay_origin_pid = Self::foreground_pid();
            self.gui_diagnostics.timings = StageTimings {
                capture_first_frame: self.gui_diagnostics.timings.capture_first_frame,
                ..StageTimings::default()
            };
            self.retry_recent_dictation(previous, generation);
        }
        self.gui_diagnostics.abandon_stages();
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
                if self.state.phase == Phase::Recording {
                    self.fail_capture(message);
                    return;
                }
                self.state.phase = Phase::Failed;
                self.state.retry_available = self.recording.is_some();
                self.message = message;
                self.report_gui_transcription(
                    TranscriptionState::Failed,
                    Some(BlockReason::TranscriptionFailed),
                );
                self.gui_diagnostics.abandon_stages();
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
        self.report_gui_transcription(
            TranscriptionState::Failed,
            Some(BlockReason::TranscriptionFailed),
        );
        self.state.phase = Phase::Stopping;
        self.state.retry_available = false;
        self.allow_delivery = false;
        self.delivery_settle = None;
        self.gui_diagnostics.abandon_stages();
        self.delivery.clear();
        self.started = None;
        self.cancel_capture();
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
        if self.cancel_repaste() {
            return;
        }
        if self.file_jobs.busy() {
            return;
        }
        self.stop_microphone_test();
        if self.source_tests.busy() {
            self.source_tests.cancel();
            self.message = self.source_tests.summary();
            return;
        }
        if self.cleanup.is_some() {
            // This existing, enabled native action is an explicit observation
            // request. Neither polling nor repeated Quit flags call it for us.
            self.request_cleanup_observation();
            return;
        }
        let delivery_pending = self.delivery.has_pending();
        let gui_pending = self.current_gui_receipt().is_some_and(|receipt| {
            if receipt.transcription == TranscriptionState::Pending {
                receipt.transcription = TranscriptionState::Cancelled;
                receipt.block_reason = Some(BlockReason::Cancelled);
                true
            } else {
                if delivery_pending {
                    receipt.block_reason = Some(BlockReason::Cancelled);
                }
                false
            }
        });
        if !gui_pending && !delivery_pending {
            self.detach_gui_delivery();
        }
        self.stop_failure = None;
        self.delivery_settle = None;
        let worker_active = self.active.is_some();
        self.state.cancel(worker_active);
        self.discard_recent_dictation_candidate();
        self.gui_diagnostics.abandon_stages();
        self.delivery.clear();
        self.observe_recent_dictation();
        self.started = None;
        self.cancel_capture();
        if let Some(active) = &self.active {
            active.cancel.cancel();
            self.message = "正在取消并释放录音…".into();
        } else {
            self.recording = None;
            self.message = "听写已取消。".into();
        }
        self.wait_for_delivery();
    }
    pub(crate) fn cancel_released_shortcut(&mut self) {
        self.cancel();
        self.message = "准备期间已松开快捷键，未开始录音。".into();
    }

    fn update_dictation_partial(&mut self, generation: u64, text: String) {
        self.transcript = text;
        self.visible_transcript = Some(VisibleTranscript::Preview(generation));
        // An append-only target cannot undo raw partial text after correction.
        if self.recorded_output.is_identity() {
            self.delivery_status = self.delivery.preview(&self.transcript);
        }
    }

    fn apply_dictation_final(&mut self, text: String) {
        let corrected = self.recorded_output.process(&text).into_final();
        if corrected == text {
            self.transcript = text;
            self.raw_transcript = None;
        } else {
            self.transcript = corrected.into_owned();
            self.raw_transcript = Some(text);
        }
    }

    #[cfg(test)]
    pub fn poll(&mut self) {
        self.poll_with_start_gate(|_| true);
    }

    pub(crate) fn poll_with_start_gate(&mut self, start_allowed: impl FnMut(u64) -> bool) {
        self.poll_inner(start_allowed, |c, speech, generation, local| {
            c.capture = Some(Capture::begin(
                &c.audio_lane,
                audio::Start {
                    generation,
                    events: c.events_tx.clone(),
                    speech: speech && c.recorded_source.uses_system_speech(),
                    language: c.recorded_language.clone(),
                    preferred_uid: c.recorded_microphone_uid.clone(),
                    local,
                },
            )?);
            Ok(None)
        });
    }

    pub(crate) fn poll_with_capture(
        &mut self,
        start_allowed: impl FnMut(u64) -> bool,
        mut start_capture: impl FnMut(
            &mut Self,
            bool,
            u64,
            Option<crate::local_audio::Input>,
        ) -> Result<bool, String>,
    ) {
        self.poll_inner(start_allowed, |c, speech, generation, local| {
            start_capture(c, speech, generation, local).map(Some)
        });
    }

    fn fail_start(&mut self, message: String) {
        self.state.phase = Phase::Failed;
        self.message = message;
        self.report_gui_transcription(
            TranscriptionState::NotStarted,
            Some(BlockReason::CaptureFailed),
        );
        self.seal_gui_capture(Some(CaptureDiagnostics::default()));
        self.cancel_capture();
        self.gui_diagnostics.abandon_stages();
        self.delivery.clear();
        self.wait_for_delivery();
    }

    fn capture_started(
        &mut self,
        generation: u64,
        preview: bool,
        lease: Option<Arc<Recording>>,
        incoming: Option<Incoming>,
    ) {
        if !self.state.permissions(generation, true) {
            return;
        }
        self.latest_output.clear_notice();
        self.transcript.clear();
        self.visible_transcript = None;
        self.retained_gui_result = None;
        self.raw_transcript = None;
        self.started = Some(Instant::now());
        if let Some(incoming) = incoming {
            self.recording = lease;
            self.submit_input(Some(incoming));
        }
        if self.state.phase == Phase::Recording {
            self.message = if self.delivery.uses_clipboard() {
                "兼容听写中；停止后会复制最终文字并粘贴到原输入位置。"
            } else {
                "正在录音；结束后转写。"
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

    fn poll_capture(&mut self, start_allowed: &mut impl FnMut(u64) -> bool) {
        let Some(capture) = &self.capture else {
            return;
        };
        let generation = capture.generation;
        if !self.state.accepts(generation) {
            self.cancel_capture();
            return;
        }
        let opening = self.opening_microphone();
        let now = Instant::now();
        if opening {
            if !start_allowed(generation) {
                self.cancel_released_shortcut();
                return;
            }
            if self.recorded_microphone_uid != self.microphone_uid {
                self.fail_start("准备期间麦克风偏好已变化，请重新开始。".into());
                return;
            }
            if now >= capture.deadline {
                self.fail_start(audio::START_TIMEOUT.into());
                return;
            }
        } else if let Some(deadline) = self.capture_stop {
            if now >= deadline {
                self.fail_capture(audio::STOP_TIMEOUT.into());
                return;
            }
        } else if let Some(check) = capture.checking() {
            match self.input_check {
                Some((current, deadline)) if current == check => {
                    if now >= deadline {
                        self.fail_capture(audio::RECOVERY_TIMEOUT.into());
                        return;
                    }
                }
                _ => self.input_check = Some((check, now + audio::WAIT)),
            }
        } else {
            self.input_check = None;
        }
        let reply = match capture.replies.try_recv() {
            Ok(reply) => reply,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                CaptureReply::Failed("音频线程已退出，请重新开始。".into())
            }
        };
        match reply {
            CaptureReply::Prepared if opening => {
                if let Err(message) = capture.commit() {
                    self.fail_start(message);
                }
            }
            CaptureReply::Started {
                preview,
                lease,
                route,
            } if opening => {
                self.capture_route = Some(route);
                self.capture_recovered = false;
                let incoming = self.capture_incoming.take();
                self.capture_started(generation, preview, Some(lease), incoming);
                if let Some(status) = self.capture_preview_status.take() {
                    self.preview_status = status;
                }
            }
            CaptureReply::Recovering if self.state.phase == Phase::Recording => {
                self.capture_route = None;
                self.message = "输入设备已断开，正在尝试恢复麦克风…".into();
            }
            CaptureReply::Route(route) if self.state.phase == Phase::Recording => {
                self.capture_route = Some(route);
                self.capture_recovered = true;
                self.message = "麦克风已恢复，正在继续录音。".into();
            }
            CaptureReply::Stopped {
                result,
                diagnostics,
            } if self.capture_stop.take().is_some() => {
                let recording = result.and_then(|()| capture.take_finalized());
                self.capture = None;
                self.input_check = None;
                self.seal_gui_capture(diagnostics);
                self.finish_capture(recording);
            }
            CaptureReply::Failed(message) if opening => self.fail_start(message),
            CaptureReply::Failed(message) => self.fail_capture(message),
            _ => {}
        }
    }

    fn poll_transcription_worker(&mut self) -> Option<Completion> {
        match self.worker.as_ref().map(|worker| worker.results.try_recv()) {
            Some(Ok(completion)) => Some(completion),
            Some(Err(TryRecvError::Disconnected)) => {
                if self.state.phase == Phase::Recording {
                    self.stop();
                    if self.capture_stop.is_some() {
                        return None;
                    }
                }
                self.worker = None;
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
                    self.gui_diagnostics.abandon_stages();
                    self.delivery.retain_preview();
                    self.wait_for_delivery();
                    let stopping = self.state.phase == Phase::Stopping;
                    if self.state.worker_disconnected(self.recording.is_some()) {
                        if stopping {
                            self.cancel_capture();
                            self.recording = None;
                            if let Some(error) = self.stop_failure.take() {
                                self.state.phase = Phase::Failed;
                                self.message = error;
                            } else {
                                self.message = "听写已取消。".into();
                            }
                        } else {
                            self.report_gui_transcription(
                                TranscriptionState::Failed,
                                Some(BlockReason::TranscriptionFailed),
                            );
                            self.message = "转写工作线程已退出，录音保留供手动重试。".into();
                        }
                    }
                }
                None
            }
            Some(Err(TryRecvError::Empty)) | None => None,
        }
    }

    fn poll_inner(
        &mut self,
        mut start_allowed: impl FnMut(u64) -> bool,
        mut start_capture: impl FnMut(
            &mut Self,
            bool,
            u64,
            Option<crate::local_audio::Input>,
        ) -> Result<Option<bool>, String>,
    ) {
        self.file_jobs.poll();
        self.poll_microphone_test();
        let was_testing = self.source_tests.busy();
        self.source_tests.poll();
        if was_testing {
            self.message = self.source_tests.summary();
        }
        self.poll_cleanup_observation();
        if let Some(discovery) = &self.discovery {
            match discovery.try_recv() {
                Ok(catalog) => {
                    self.source_tests.retain_catalog(&catalog.entries);
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
        self.poll_capture(&mut start_allowed);
        let partial = self.partial.lock().ok().and_then(|mut slot| slot.take());
        if let Some((generation, text)) = partial
            && self.state.accepts(generation)
            && self.state.phase == Phase::Recording
        {
            self.update_dictation_partial(generation, text);
        }
        while let Ok((generation, event)) = self.events.try_recv() {
            if !self.state.accepts(generation) {
                continue;
            }
            match event {
                AudioEvent::Permissions { microphone, speech } => {
                    // Duplicate callbacks must not cancel an already-started
                    // session or change its delivery target.
                    if self.state.phase != Phase::Permissions || self.capture.is_some() {
                        continue;
                    }
                    // A synchronous permission reply can arrive during the same tick
                    // that began preparations, while Carbon key-up is still pending.
                    if !start_allowed(generation) {
                        self.cancel_released_shortcut();
                        continue;
                    }
                    if !microphone {
                        self.state.permissions(generation, false);
                        self.message = "请在系统设置中允许 VoicePort 使用麦克风。".into();
                        self.report_gui_transcription(
                            TranscriptionState::NotStarted,
                            Some(BlockReason::MicrophoneDenied),
                        );
                        self.gui_diagnostics.abandon_stages();
                        self.delivery.clear();
                        continue;
                    }
                    let (local, incoming) = if self.recorded_source.is_local() {
                        let (input, incoming) = crate::local_audio::channel();
                        (Some(input), Some(incoming))
                    } else {
                        (None, None)
                    };
                    if self.current_gui_receipt().is_some() {
                        self.gui_diagnostics.capture_pending = Some(generation);
                    }
                    match start_capture(self, speech, generation, local) {
                        Ok(None) => {
                            self.capture_incoming = incoming;
                            self.message = "正在打开麦克风…".into();
                        }
                        // The synchronous fixtures own isolated synthetic recording leases.
                        Ok(Some(preview)) => {
                            #[cfg(test)]
                            let lease = self.recorder.lease();
                            #[cfg(not(test))]
                            let lease = None;
                            self.capture_started(generation, preview, lease, incoming);
                        }
                        Err(message) => self.fail_start(message),
                    }
                }
                AudioEvent::Partial(text) if self.state.phase == Phase::Recording => {
                    self.update_dictation_partial(generation, text);
                }
                AudioEvent::Status(text) if self.opening_microphone() => {
                    self.capture_preview_status = Some(text);
                }
                AudioEvent::Status(text) if self.state.phase == Phase::Recording => {
                    self.preview_status = text
                }
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
                    self.cancel_capture();
                    self.report_gui_transcription(
                        TranscriptionState::Failed,
                        Some(BlockReason::CaptureFailed),
                    );
                    self.gui_diagnostics.abandon_stages();
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
        let completion = if self.capture_stop.is_some() {
            None
        } else if self.deferred_completion.is_some() {
            self.deferred_completion.take()
        } else {
            self.poll_transcription_worker()
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
                if self.capture_stop.is_some() {
                    self.deferred_completion = Some(completion);
                    return;
                }
            }
            if self.state.phase == Phase::Transcribing && self.current_gui_receipt().is_some() {
                self.gui_diagnostics.timings.transcription = completion.elapsed;
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
                self.cancel_capture();
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
                        self.apply_dictation_final(text);
                        let has_text = !self.transcript.trim().is_empty();
                        if has_text {
                            self.report_gui_transcription(TranscriptionState::Completed, None);
                        } else {
                            self.report_gui_transcription(
                                TranscriptionState::Empty,
                                Some(BlockReason::EmptyTranscript),
                            );
                        }
                        if self.gui_owns_delivery
                            && self.allow_delivery
                            && has_text
                            && self.delivery.failure_stage().is_none()
                            && (self.delivery.method().is_some() || self.delivery.preparing_input())
                        {
                            self.gui_diagnostics.delivery = Some(StageTimer::start(Instant::now));
                        }
                        if !has_text {
                            self.gui_diagnostics.preparation = None;
                        }
                        self.visible_transcript =
                            has_text.then_some(VisibleTranscript::Final(completion.generation));
                        self.delivery_status =
                            self.delivery.deliver(&self.transcript, self.allow_delivery);
                        self.gui_diagnostics
                            .finish_delivery_if_settled(&self.delivery);
                        self.message = match (self.testing, has_text) {
                            (true, false) => "样例接口已响应：没有识别到语音（内置样例为一秒静音）；未验证识别质量。",
                            (true, true) => "样例接口已响应，返回文字见面板；静音样例不能证明识别质量。",
                            (false, false) => "没有识别到语音。",
                            (false, true) => "最终转写完成。",
                        }
                        .into();
                        if self.allow_delivery && has_text && self.delivery.uses_clipboard() {
                            self.message = self.delivery_status.clone();
                        }
                        self.cancel_capture();
                        self.recording = None;
                    }
                    Err(error) => {
                        self.message = error.message;
                        self.report_gui_transcription(
                            TranscriptionState::Failed,
                            Some(BlockReason::TranscriptionFailed),
                        );
                        self.gui_diagnostics.abandon_stages();
                        self.delivery.retain_preview();
                        self.wait_for_delivery();
                    }
                }
            }
        }
        self.wait_for_delivery();
        let status = self
            .delivery
            .poll_observing_preparation(|| self.gui_diagnostics.finish_preparation());
        self.gui_diagnostics
            .finish_delivery_if_settled(&self.delivery);
        if let Some(status) = status {
            self.delivery_status = status;
            if self.delivery.uses_clipboard() {
                self.message = self.delivery_status.clone();
            }
        }
        if self.state.phase == Phase::Delivering && !self.delivery.has_pending() {
            self.state.phase = self.delivery_settle.take().unwrap_or(Phase::Ready);
        }
        self.observe_recent_dictation();
        self.poll_repaste();
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
                let timer = StageTimer::start(Instant::now);
                started_tx.send(generation).unwrap();
                let result = responses.recv_timeout(Duration::from_secs(5)).unwrap();
                let elapsed = timer.finish(Instant::now);
                drop(job);
                completion
                    .send(Completion {
                        elapsed,
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
        let original = fixture.controller.state.generation();
        fixture.controller.begin_gui_receipt(original, false);
        fixture.controller.gui_diagnostics.capture_pending = Some(original);
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
        let failed = fixture.controller.gui_dictation_diagnostics().unwrap();
        assert_eq!(failed.capture.unwrap().generation, original);
        assert!(failed.timings.transcription_ms.is_some());
        fixture.controller.retry();
        let current = control
            .started
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        assert_ne!(current, first);
        let retried = fixture.controller.gui_dictation_diagnostics().unwrap();
        assert_eq!(retried.session, original);
        assert_eq!(retried.generation, current);
        assert_eq!(retried.capture, failed.capture);
        assert_eq!(
            retried.timings.capture_first_frame_ms,
            failed.timings.capture_first_frame_ms
        );
        assert_eq!(retried.timings.transcription_ms, None);
        assert_eq!(retried.timings.delivery_ms, None);
        for _ in 0..2 {
            control
                .outgoing
                .send(Completion {
                    elapsed: None,
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
        let original = fixture.controller.state.generation();
        fixture.controller.begin_gui_receipt(original, false);
        fixture.controller.gui_diagnostics.capture_pending = Some(original);
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
                elapsed: None,
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
        let cancelled = fixture.controller.gui_dictation_diagnostics().unwrap();
        assert_eq!(cancelled.generation, original);
        assert_eq!(cancelled.capture.unwrap().signal, CaptureSignal::Signal);
        assert_eq!(cancelled.timings.transcription_ms, None);
        assert_eq!(cancelled.timings.delivery_ms, None);
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
            joiner: None,
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
                elapsed: None,
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
            joiner: None,
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
    fn gui_delivery_without_devices() {
        use crate::delivery::model::{
            Failure as InputFailure, ReplacementFailure, Snapshot, Target, TextRange, byte_range,
        };
        use crate::status_overlay::{CaptureStage, Stage};
        use std::{cell::RefCell, rc::Rc};
        use voiceport_control::DeliveryState;

        struct ReceiverState {
            snapshot: Snapshot,
            focused: bool,
            write_error: bool,
            writes: usize,
        }
        struct Receiver(Rc<RefCell<ReceiverState>>);
        impl Target for Receiver {
            fn read(&mut self) -> Result<Snapshot, InputFailure> {
                let receiver = self.0.borrow();
                if !receiver.focused {
                    return Err(InputFailure::FocusElementChanged);
                }
                Ok(receiver.snapshot.clone())
            }
            fn replace(
                &mut self,
                range: TextRange,
                text: &str,
                expected: &Snapshot,
            ) -> Result<(), ReplacementFailure> {
                assert_eq!(
                    &self.read().map_err(ReplacementFailure::BeforeWrite)?,
                    expected
                );
                let mut receiver = self.0.borrow_mut();
                let bytes = byte_range(&receiver.snapshot.text, range).unwrap();
                receiver.snapshot.text.replace_range(bytes, text);
                receiver.snapshot.selection = TextRange {
                    location: range.location + text.encode_utf16().count(),
                    length: 0,
                };
                receiver.writes += 1;
                if receiver.write_error {
                    return Err(ReplacementFailure::Uncertain(InputFailure::Write));
                }
                Ok(())
            }
        }
        fn receiver() -> Rc<RefCell<ReceiverState>> {
            Rc::new(RefCell::new(ReceiverState {
                snapshot: Snapshot {
                    text: "A|B".into(),
                    selection: TextRange {
                        location: 1,
                        length: 1,
                    },
                },
                focused: true,
                write_error: false,
                writes: 0,
            }))
        }
        fn start(c: &mut Controller) -> (u64, Rc<RefCell<ReceiverState>>) {
            let receiver = receiver();
            let (generation, _) = c
                .prepare_start_with(
                    SessionSource::ChatGpt,
                    "zh".into(),
                    true,
                    true,
                    |delivery| delivery.begin_fixture(Box::new(Receiver(receiver.clone()))),
                )
                .unwrap();
            assert_eq!(c.overlay_snapshot().stage, Some(Stage::Preparing));
            c.capture_started(generation, false, None, None);
            assert_eq!(
                c.overlay_snapshot().stage,
                Some(Stage::Capture(CaptureStage::Recording))
            );
            assert!(c.state.transcribe());
            assert_eq!(c.overlay_snapshot().stage, Some(Stage::Transcribing));
            c.active = Some(ActiveJob {
                generation,
                cancel: CancellationToken::new(),
                custody: Custody::default(),
                _recording: None,
            });
            (generation, receiver)
        }
        fn finish(c: &mut Controller, outgoing: &Sender<Completion>, generation: u64, text: &str) {
            outgoing
                .send(Completion {
                    elapsed: None,
                    generation,
                    result: Ok(text.into()),
                })
                .unwrap();
            c.poll();
        }

        let mut c = Controller::new();
        let (outgoing, results) = mpsc::channel();
        let (jobs, _incoming) = mpsc::sync_channel(1);
        c.worker = Some(Worker { jobs, results });
        assert!(c.gui_dictation_receipt().is_none());
        let mut preceding = 0;
        for (case, result, state, stage, target_text, writes) in [
            (
                "in-place",
                "测试",
                DeliveryState::Confirmed,
                None,
                "A测试B",
                1,
            ),
            (
                "focus-change",
                "测试",
                DeliveryState::NotSent,
                Some("focus"),
                "A|B",
                0,
            ),
            (
                "user-edit",
                "测试",
                DeliveryState::NotSent,
                Some("ownership"),
                "A用户B",
                0,
            ),
            ("empty", "", DeliveryState::NotSent, None, "A|B", 0),
            ("whitespace", " \n", DeliveryState::NotSent, None, "A|B", 0),
            (
                "unknown-write",
                "测试",
                DeliveryState::Uncertain,
                Some("write"),
                "A测试B",
                1,
            ),
        ] {
            let (generation, receiver) = start(&mut c);
            let pending = c.gui_dictation_receipt().unwrap();
            assert_eq!(
                (pending.session, pending.generation),
                (generation, generation)
            );
            assert_eq!(pending.transcription, TranscriptionState::Pending);
            assert_eq!(pending.delivery.state, DeliveryState::NotSent);
            assert_eq!(pending.method.as_deref(), Some("accessibility"));
            assert_eq!(pending.failure_stage, None);

            c.events_tx
                .send((preceding, AudioEvent::Partial("旧轮".into())))
                .unwrap();
            finish(&mut c, &outgoing, preceding, "迟到结果");
            assert_eq!(c.gui_dictation_receipt().unwrap(), pending, "{case}");
            assert_eq!(c.overlay_snapshot().stage, Some(Stage::Transcribing));
            assert!(c.transcript.is_empty());
            assert_eq!(receiver.borrow().writes, 0);

            match case {
                "focus-change" => receiver.borrow_mut().focused = false,
                "user-edit" => receiver.borrow_mut().snapshot.text = "A用户B".into(),
                "unknown-write" => receiver.borrow_mut().write_error = true,
                _ => {}
            }
            finish(&mut c, &outgoing, generation, result);
            assert_eq!(c.state.phase, Phase::Ready, "{case}");
            assert_eq!(c.overlay_snapshot().stage, None);
            let receipt = c.gui_dictation_receipt().unwrap();
            assert_eq!(receipt.delivery.state, state, "{case}");
            assert_eq!(receipt.failure_stage.as_deref(), stage, "{case}");
            let empty = result.trim().is_empty();
            assert_eq!(
                receipt.transcription,
                if empty {
                    TranscriptionState::Empty
                } else {
                    TranscriptionState::Completed
                }
            );
            assert_eq!(
                receipt.block_reason,
                empty.then_some(BlockReason::EmptyTranscript)
            );
            assert_eq!(
                receipt.delivery.submitted_operations,
                usize::from(state == DeliveryState::Confirmed)
            );
            assert_eq!(
                receipt.delivery.confirmed_operations,
                usize::from(state == DeliveryState::Confirmed)
            );
            c.events_tx
                .send((generation, AudioEvent::Partial("迟到预览".into())))
                .unwrap();
            finish(&mut c, &outgoing, generation, "重复结果");
            assert_eq!(c.gui_dictation_receipt().unwrap(), receipt, "{case}");
            assert_eq!(&c.transcript, result);
            let receiver = receiver.borrow();
            assert_eq!(receiver.snapshot.text, target_text, "{case}");
            assert_eq!(receiver.writes, writes, "{case}");
            if writes > 0 {
                assert_eq!(
                    receiver.snapshot.selection,
                    TextRange {
                        location: 3,
                        length: 0
                    }
                );
            }
            let json = serde_json::to_value(&receipt).unwrap();
            assert_eq!(
                serde_json::from_value::<GuiDictationReceipt>(json.clone()).unwrap(),
                receipt
            );
            for private in ["A|B", "A测试B", "测试", "A用户B", "重复结果", "迟到预览"]
            {
                assert!(!json.to_string().contains(private));
            }
            preceding = generation;
            println!(
                "gui-delivery: {case} text={target_text:?} selection={:?} writes={writes} receipt={json}",
                receiver.snapshot.selection
            );
        }

        // Starting a panel-only GUI session or a CLI operation cannot borrow a
        // previous automatic route, failure, or ambiguous-write outcome.
        let automatic = c.gui_dictation_receipt().unwrap();
        let (cli, _) = c
            .prepare_start(SessionSource::ChatGpt, "zh".into(), false)
            .unwrap();
        assert!(cli > automatic.generation);
        assert_eq!(c.overlay_snapshot().stage, None);
        assert_eq!(c.gui_dictation_receipt().unwrap(), automatic);
        c.cancel();
        assert_eq!(c.gui_dictation_receipt().unwrap(), automatic);
        let original_source = c.gui_result_context().source.clone().unwrap();
        c.gui_result_context.target = Some("原目标夹具".into());
        c.language = "en".into();
        c.reject_start("目标应用发生变化。");
        let retained = c
            .retained_gui_result()
            .expect("previous final remains visible");
        assert_eq!(retained.receipt, automatic);
        assert_eq!(retained.diagnostics.generation, automatic.generation);
        assert_eq!(
            retained.context.source.as_deref(),
            Some(original_source.as_str())
        );
        assert_eq!(retained.context.target.as_deref(), Some("原目标夹具"));
        assert_eq!(c.gui_result_context().target, None);
        assert_eq!(c.transcript, "测试");
        let (panel, _) = c
            .prepare_start_with(SessionSource::ChatGpt, "zh".into(), false, true, |_| {
                panic!("panel-only GUI must not capture a target");
            })
            .unwrap();
        let receipt = c.gui_dictation_receipt().unwrap();
        assert_eq!(receipt.generation, panel);
        assert_eq!(receipt.method, None);
        assert_eq!(receipt.failure_stage, None);
        assert_eq!(receipt.delivery, Default::default());
        assert_eq!(receipt.block_reason, Some(BlockReason::PanelOnly));
        assert_eq!(c.overlay_snapshot().stage, Some(Stage::Preparing));
        c.cancel();
        assert_eq!(c.overlay_snapshot().stage, None);
        let cancelled = c.gui_dictation_receipt().unwrap();
        assert_eq!(cancelled.generation, panel);
        assert_eq!(c.retained_gui_result().unwrap().receipt, automatic);
        assert_eq!(cancelled.transcription, TranscriptionState::Cancelled);
        assert_eq!(cancelled.block_reason, Some(BlockReason::Cancelled));
        finish(&mut c, &outgoing, panel, "晚于取消");
        assert_eq!(c.gui_dictation_receipt().unwrap(), cancelled);

        c.workflows.dictionary_enabled = true;
        c.workflows.rules = crate::dictation_dictionary::parse_rules("北经 → 北京").unwrap();
        c.workflows.output_rules.trim_edges = true;
        c.workflows.output_rules.normalize_newlines = true;
        let (generation, corrected_receiver) = start(&mut c);
        assert!(c.retained_gui_result().is_none());
        c.update_dictation_partial(c.state.generation(), "北经".into());
        assert_eq!(c.transcript, "北经");
        assert_eq!(corrected_receiver.borrow().writes, 0);
        let original = "\r\n  北经🙂\r\n\r\n第二行 \r\n";
        finish(&mut c, &outgoing, generation, original);
        finish(&mut c, &outgoing, generation, "重复结果");
        assert_eq!(c.transcript, "北京🙂\n\n第二行");
        assert_eq!(c.raw_transcript.as_deref(), Some(original));
        let corrected = corrected_receiver.borrow();
        assert_eq!(corrected.snapshot.text, "A北京🙂  第二行B");
        assert_eq!(corrected.snapshot.selection.location, 10);
        assert_eq!(corrected.writes, 1);
        assert_eq!(
            c.gui_dictation_receipt().unwrap().delivery.state,
            DeliveryState::Confirmed
        );
        println!(
            "gui-delivery: dictionary and output preview writes=0, processed final writes=1, original retained; panel keeps LF, target receives spaces"
        );

        c.workflows.dictionary_enabled = false;
        let (generation, empty_receiver) = start(&mut c);
        c.update_dictation_partial(generation, "仅面板预览".into());
        assert_eq!(c.transcript, "仅面板预览");
        assert_eq!(empty_receiver.borrow().writes, 0);
        finish(&mut c, &outgoing, generation, " \t\n");
        assert!(c.transcript.is_empty());
        assert!(c.visible_transcript.is_none());
        assert_eq!(
            c.gui_dictation_receipt().unwrap().transcription,
            TranscriptionState::Empty
        );
        assert_eq!(empty_receiver.borrow().snapshot.text, "A|B");
        assert_eq!(empty_receiver.borrow().writes, 0);

        // Real synthetic PCM/AAC and production Stop/Job dispatch, with an
        // isolated receiver and an ASR stand-in that returns a successful empty
        // result. Leave the receiver open to detect any second request.
        let mut fixture = Fixture::new(Input::Valid);
        let c = &mut fixture.controller;
        let generation = c.state.generation();
        c.begin_gui_receipt(generation, true);
        c.gui_diagnostics.capture_pending = Some(generation);
        c.allow_delivery = true;
        c.gui_owns_delivery = true;
        let receiver = receiver();
        c.delivery
            .begin_fixture(Box::new(Receiver(receiver.clone())))
            .unwrap();
        assert!(c.gui_dictation_diagnostics().unwrap().capture.is_none());
        let (jobs, incoming) = mpsc::sync_channel(1);
        let (outgoing, results) = mpsc::channel();
        c.worker = Some(Worker { jobs, results });
        let recording = c.overlay_snapshot();
        assert_eq!(
            recording.stage,
            Some(Stage::Capture(CaptureStage::Recording))
        );
        // Synthetic PCM went through the actual capture sink, but scheduling can
        // delay this observation past the meter's freshness window.
        assert!(c.capture_level().is_none_or(|level| level == 3));
        c.stop();
        assert_eq!(c.overlay_snapshot().stage, Some(Stage::Transcribing));
        let job = incoming
            .try_recv()
            .expect("one ASR request for audible input");
        assert_eq!(job.generation, generation);
        assert!(job.recording.as_ref().unwrap().path().exists());
        let elapsed = StageTimer::start(Instant::now).finish(Instant::now);
        drop(job);
        outgoing
            .send(Completion {
                generation,
                elapsed,
                result: Ok(String::new()),
            })
            .unwrap();
        c.poll();
        let receipt = c.gui_dictation_receipt().unwrap();
        let diagnostics = c.gui_dictation_diagnostics().unwrap();
        let capture = diagnostics.capture.unwrap();
        assert_eq!(capture.generation, generation);
        assert_eq!(capture.signal, CaptureSignal::Signal);
        assert_eq!(capture.callbacks, Some(3));
        assert_eq!(capture.observed_frames, Some(14_400));
        assert!(diagnostics.timings.capture_first_frame_ms.is_some());
        assert!(diagnostics.timings.transcription_ms.is_some());
        assert_eq!(diagnostics.timings.preparation_ms, None);
        assert_eq!(diagnostics.timings.delivery_ms, None);
        assert_eq!(receipt.transcription, TranscriptionState::Empty);
        assert_eq!(receipt.delivery.state, DeliveryState::NotSent);
        assert!(!c.can_retry_recording());
        c.retry();
        finish(c, &outgoing, generation, "重复结果");
        assert!(matches!(incoming.try_recv(), Err(TryRecvError::Empty)));
        assert_eq!(c.gui_dictation_diagnostics().unwrap(), diagnostics);
        assert_eq!(c.gui_dictation_receipt().unwrap(), receipt);
        assert_eq!(receiver.borrow().snapshot.text, "A|B");
        assert_eq!(receiver.borrow().writes, 0);
        println!(
            "gui-delivery: audible empty ASR requests=1 writes=0 diagnostics={}",
            serde_json::to_string(&diagnostics).unwrap()
        );
    }

    pub(crate) fn run(case: &str, _main_thread: MainThreadMarker) {
        assert!(MainThreadMarker::new().is_some());
        match case {
            "zero" | "stop-first" | "poll-first" => local_failure(case),
            "retry" => retry_preserves_recording(),
            "cancel" => cancellation_waits_for_recording_release(),
            "local-stop-failure" => local_stop_failure_waits_for_worker(),
            "cli-session" => cli_start_stop_uses_pinned_policy_without_devices(),
            "gui-delivery" => gui_delivery_without_devices(),
            _ => panic!("unknown stop_failure case: {case}"),
        }
        println!("stop_failure: {case} passed");
    }
}

#[cfg(test)]
pub(crate) mod tests {
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

    #[test]
    fn cli_permission_policy_never_requests_undecided_grants() {
        for microphone in ["authorized", "notDetermined", "denied", "restricted"] {
            for speech in ["authorized", "notDetermined", "denied", "restricted"] {
                for use_speech in [false, true] {
                    let mut c = Controller::new();
                    let source = if use_speech {
                        SessionSource::ChatGpt
                    } else {
                        local_source()
                    };
                    let accepted =
                        c.start_pinned_with_permissions(source, "zh".into(), false, || {
                            (microphone, speech)
                        });
                    assert_eq!(accepted, Some(c.state.generation()));
                    let (generation, event) = c.events.try_recv().unwrap();
                    assert_eq!(generation, c.state.generation());
                    assert_eq!(
                        event,
                        AudioEvent::Permissions {
                            microphone: microphone == "authorized",
                            speech: use_speech && speech == "authorized",
                        }
                    );
                    assert!(c.events.try_recv().is_err());
                    assert_eq!(c.state.phase, Phase::Permissions);
                    assert!(c.worker.is_none() && c.recording.is_none());
                    assert!(!c.allow_delivery);
                }
            }
        }
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
            joiner: None,
        })
    }
    #[test]
    fn unsupported_model_language_is_rejected_before_recording_preparation() {
        let mut c = Controller::new();
        let source = local_source(); // Fixed bilingual Paraformer fixture; no model/device I/O.
        assert!(
            c.prepare_start(source.clone(), "ja".into(), false)
                .is_none()
        );
        assert_eq!(c.state.phase, Phase::Idle);
        assert!(matches!(c.recorded_source, SessionSource::ChatGpt));
        assert!(c.recording.is_none());
        assert!(c.message.contains("不支持此语言"));
        let (_, speech) = c.prepare_start(source.clone(), "zh".into(), false).unwrap();
        assert!(!speech);
        assert_eq!(c.recorded_source, source);
        assert_eq!(c.recorded_language, "zh");
    }
    pub(crate) fn check_control_start_rejection(
        mut start: impl FnMut(&mut Controller, SessionSource, u64),
    ) {
        for phase in [Phase::Ready, Phase::Failed, Phase::Idle] {
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
            let mut c = Controller::new();
            let generation = c.state.begin().unwrap();
            c.state.phase = phase;
            c.state.retry_available = phase == Phase::Failed;
            c.transcript = "synthetic previous transcript".into();
            c.recorded_source = SessionSource::ChatGpt;
            c.recorded_language = "en".into();
            c.allow_delivery = true;
            if phase == Phase::Failed {
                c.recording = Some(recording.clone());
            }
            start(&mut c, local_source(), generation);
            assert_eq!(c.state.generation(), generation);
            assert_eq!(c.state.phase, phase);
            assert_eq!(c.transcript, "synthetic previous transcript");
            assert_eq!(c.recorded_source, SessionSource::ChatGpt);
            assert_eq!(c.recorded_language, "en");
            assert!(c.allow_delivery);
            assert_eq!(c.state.retry_available, phase == Phase::Failed);
            if phase == Phase::Failed {
                assert!(Arc::ptr_eq(c.recording.as_ref().unwrap(), &recording));
                assert!(c.can_retry_recording());
            } else {
                assert!(c.recording.is_none());
            }
            assert!(c.events.try_recv().is_err());
            assert!(c.worker.is_none() && c.active.is_none());
            assert!(c.message.contains("不支持此语言"));
        }
    }
    #[test]
    fn retry_snapshots_original_local_source_and_language_without_cloud_dispatch() {
        check_control_retry(|c, _| c.retry());
    }
    pub(crate) fn check_control_retry(retry: impl FnOnce(&mut Controller, u64)) {
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
        retry(&mut c, old);
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
    fn independent_source_probes_preserve_dictation_and_hold_admission_until_exit() {
        use std::time::Duration;
        let mut c = Controller::new();
        let entry = crate::local_models::Entry {
            id: "synthetic-local-probe".into(),
            title: "Synthetic local probe".into(),
            detail: "Never opens a model".into(),
            source: Ok(local_source()),
        };
        c.catalog.entries.push(entry.clone());
        c.source_id = CHATGPT.into();
        c.language = "en".into();
        c.automatic = false;
        c.transcript = "existing dictation".into();
        c.preview_status = "existing preview".into();
        c.delivery_status = "existing delivery".into();
        // A failed dictation can have a retained recording available to Retry.
        let generation = c.state.begin().unwrap();
        c.state.permissions(generation, true);
        c.state.transcribe();
        c.state.complete(generation, false, true);
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
        c.recording = Some(recording.clone());
        let (started_tx, started) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let released = Mutex::new(released);
        c.source_tests = crate::source_tests::SourceTests::with_executor(
            Duration::from_secs(5),
            move |source, language, cancel, _custody| {
                started_tx
                    .send((source, language.to_owned(), cancel.clone()))
                    .unwrap();
                released
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap();
                Ok(())
            },
        );
        c.start_source_tests(std::slice::from_ref(&entry.id))
            .unwrap();
        let (source, language, cancel) = started.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(source, local_source());
        assert_eq!(language, "en");
        assert!(c.busy());
        assert!(
            c.start_source_tests(std::slice::from_ref(&entry.id))
                .is_err()
        );
        c.start();
        c.retry();
        assert_eq!(c.state.phase, Phase::Failed);
        assert_eq!(c.state.generation(), generation);
        assert!(c.state.retry_available);
        assert!(c.active.is_none() && c.worker.is_none());
        assert!(c.events.try_recv().is_err());
        c.cancel();
        c.poll();
        assert!(cancel.is_cancelled());
        assert!(
            c.busy(),
            "cancel must keep admission until the worker exits"
        );
        assert_eq!(c.source_id, CHATGPT);
        assert_eq!(c.language, "en");
        assert!(!c.automatic);
        assert_eq!(c.transcript, "existing dictation");
        assert_eq!(c.preview_status, "existing preview");
        assert_eq!(c.delivery_status, "existing delivery");
        assert!(Arc::ptr_eq(c.recording.as_ref().unwrap(), &recording));
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while c.busy() && Instant::now() < deadline {
            c.poll();
            std::thread::yield_now();
        }
        assert!(!c.busy());
        assert_eq!(
            c.source_tests.row(&entry).unwrap().status,
            crate::source_tests::Status::Cancelled
        );
        assert_eq!(c.state.generation(), generation);
        assert_eq!(c.state.phase, Phase::Failed);
        assert!(c.state.retry_available);
        assert_eq!(c.transcript, "existing dictation");
        assert!(Arc::ptr_eq(c.recording.as_ref().unwrap(), &recording));
    }

    #[test]
    fn independent_source_probe_selection_is_atomic_and_respects_dictation_admission() {
        let mut c = Controller::new();
        assert!(
            c.start_source_tests(&[CHATGPT.into(), "missing-source".into()])
                .is_err()
        );
        assert!(!c.source_tests.busy());
        let generation = c.state.begin().unwrap();
        assert!(c.start_source_tests(&[CHATGPT.into()]).is_err());
        assert_eq!(c.state.generation(), generation);
        assert!(!c.source_tests.busy());
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
                elapsed: None,
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
        let receipt = c.gui_dictation_receipt().unwrap();
        assert_eq!(receipt.generation, c.state.generation());
        assert_eq!(receipt.transcription, TranscriptionState::NotStarted);
        assert_eq!(receipt.block_reason, Some(BlockReason::SourceUnavailable));
        assert_eq!(receipt.delivery, Default::default());
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
                elapsed: None,
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
                elapsed: None,
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
                elapsed: None,
                generation: job_generation - 1,
                result: Ok("stale".into()),
            })
            .unwrap();
        outgoing
            .send(Completion {
                elapsed: None,
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
    fn late_permission_after_shortcut_release_cannot_capture_or_transcribe() {
        let mut c = Controller::new();
        let (generation, _) = c
            .prepare_start_with(SessionSource::ChatGpt, String::new(), false, true, |_| {
                panic!("panel-only start must not capture a target")
            })
            .unwrap();
        c.poll_with_capture(
            |_| panic!("permission has not arrived"),
            |_, _, _, _| panic!("capture without permission"),
        );
        assert_eq!(c.state.phase, Phase::Permissions);
        for _ in 0..2 {
            c.events_tx
                .send((
                    generation,
                    AudioEvent::Permissions {
                        microphone: true,
                        speech: false,
                    },
                ))
                .unwrap();
        }
        let samples = std::cell::Cell::new(0);
        c.poll_with_capture(
            |reply_generation| {
                assert_eq!(reply_generation, generation);
                samples.set(samples.get() + 1);
                false
            },
            |_, _, _, _| panic!("released shortcut reached capture"),
        );
        assert_eq!(samples.get(), 1);
        assert_eq!(c.state.phase, Phase::Idle);
        assert!(c.active.is_none() && c.worker.is_none() && c.recording.is_none());
        let receipt = c.gui_dictation_receipt().unwrap();
        assert_eq!(receipt.transcription, TranscriptionState::Cancelled);
        assert_eq!(receipt.block_reason, Some(BlockReason::Cancelled));
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
        assert_eq!(c.state.generation(), generation + 1);
        let receipt = c.gui_dictation_receipt().unwrap();
        assert_eq!(receipt.session, generation + 1);
        assert_eq!(receipt.transcription, TranscriptionState::NotStarted);
        assert_eq!(receipt.block_reason, Some(BlockReason::StartRejected));
        assert_eq!(receipt.delivery, Default::default());
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
                        elapsed: None,
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
            let sent = outgoing.send(Completion {
                elapsed: None,
                generation,
                result,
            });
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
                elapsed: None,
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
