//! Main-thread-owned native capture with a single synchronous audio tap.
//! Native callbacks send only generation-tagged values; none assumes a UI thread.
use crate::storage::{self, Recording};
use block2::RcBlock;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Bool};
use objc2::{AllocAnyThread, MainThreadMarker};
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_avf_audio::{
    AVAudioBuffer, AVAudioCommonFormat, AVAudioConverter, AVAudioConverterInputStatus,
    AVAudioConverterOutputStatus, AVAudioFile, AVAudioFormat, AVAudioPCMBuffer,
    AVEncoderAudioQualityKey, AVEncoderBitRateKey, AVFormatIDKey, AVNumberOfChannelsKey,
    AVSampleRateKey,
};
use objc2_foundation::{NSDictionary, NSError, NSLocale, NSNumber, NSString, NSURL};
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask,
    SFSpeechRecognizer, SFSpeechRecognizerAuthorizationStatus,
};
use std::marker::PhantomData;
use std::path::Path;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[path = "audio_diagnostics.rs"]
pub(crate) mod diagnostics;
use diagnostics::{CaptureDiagnostics, CaptureObservation, InputSignal, StageTimer};
#[path = "audio_devices.rs"]
pub(crate) mod devices;
use devices::{OpenInput, Route};
#[path = "audio_level.rs"]
pub(crate) mod level;
#[path = "audio_recovery_pcm.rs"]
mod recovery_pcm;

pub const MAXIMUM_SECONDS: u64 = 180;
const OUTPUT_RATE: f64 = 24_000.0;
const MAXIMUM_OUTPUT_FRAMES: u64 = 24_000 * MAXIMUM_SECONDS;
const MAXIMUM_TAP_FRAMES: u32 = 1_048_576;
const AAC: u32 = u32::from_be_bytes(*b"aac ");

type EventSender = Sender<(u64, AudioEvent)>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioEvent {
    Permissions {
        microphone: bool,
        speech: bool,
    },
    Partial(String),
    Status(String),
    Failed(String),
    /// The tap has stopped accepting samples. The UI must call stop() on its
    /// main thread to stop hardware and finalize the AAC file before accepting.
    LimitReached,
}

/// Construction is inert: no files, permission queries, engine or Speech object.
/// This type is deliberately !Send/!Sync. start/stop/cancel/finish belong to UI.
pub struct Recorder {
    input: Option<OpenInput>,
    route: Option<Route>,
    recovered_input: bool,
    sink: Option<Arc<Sink>>,
    speech: Option<Speech>,
    recording: Option<Arc<Recording>>,
    events: Option<Arc<Events>>,
    capture_diagnostics: Option<CaptureDiagnostics>,
    finalized: bool,
    failure: Option<String>,
    _main_thread: PhantomData<Rc<()>>,
}

impl Default for Recorder {
    fn default() -> Self {
        Self::new()
    }
}

impl Recorder {
    pub fn new() -> Self {
        Self {
            input: None,
            route: None,
            recovered_input: false,
            sink: None,
            speech: None,
            recording: None,
            events: None,
            capture_diagnostics: None,
            finalized: false,
            failure: None,
            _main_thread: PhantomData,
        }
    }

    /// Read-only status; never creates a recorder engine or requests access.
    pub fn permission_status() -> (&'static str, &'static str) {
        let microphone = match unsafe { AVMediaTypeAudio } {
            Some(media) => match unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) }
            {
                AVAuthorizationStatus::Authorized => "authorized",
                AVAuthorizationStatus::Denied => "denied",
                AVAuthorizationStatus::Restricted => "restricted",
                AVAuthorizationStatus::NotDetermined => "notDetermined",
                _ => "unknown",
            },
            None => "unavailable",
        };
        let speech = match unsafe { SFSpeechRecognizer::authorizationStatus() } {
            SFSpeechRecognizerAuthorizationStatus::Authorized => "authorized",
            SFSpeechRecognizerAuthorizationStatus::Denied => "denied",
            SFSpeechRecognizerAuthorizationStatus::Restricted => "restricted",
            SFSpeechRecognizerAuthorizationStatus::NotDetermined => "notDetermined",
            _ => "unknown",
        };
        (microphone, speech)
    }

    /// Explicit permission request. Both native completion blocks may run on
    /// arbitrary queues. Exactly one Permissions value is sent, never UI calls.
    /// A denied Speech permission still permits ordinary microphone recording.
    pub fn request_permissions(generation: u64, sender: EventSender) {
        Self::request_permissions_for_source(generation, sender, true);
    }
    pub(crate) fn request_speech_preview(sender: EventSender) {
        request_speech_permission(
            Arc::new(PermissionReply {
                generation: 0,
                sender,
                use_speech: true,
                sent: AtomicBool::new(false),
            }),
            Self::permission_status().0 == "authorized",
        );
    }

    pub fn request_permissions_for_source(generation: u64, sender: EventSender, use_speech: bool) {
        let reply = Arc::new(PermissionReply {
            generation,
            sender,
            use_speech,
            sent: AtomicBool::new(false),
        });
        let Some(media_type) = (unsafe { AVMediaTypeAudio }) else {
            reply.send(false, false);
            return;
        };
        let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
        if status == AVAuthorizationStatus::Authorized {
            request_speech_permission(reply, true);
        } else if status == AVAuthorizationStatus::NotDetermined {
            let callback = RcBlock::new(move |allowed: Bool| {
                if allowed.as_bool() {
                    request_speech_permission(reply.clone(), true)
                } else {
                    reply.send(false, speech_authorized())
                }
            });
            unsafe {
                AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &callback)
            };
        } else {
            reply.send(false, speech_authorized())
        }
    }

    pub fn start(
        &mut self,
        speech_authorized: bool,
        language: &str,
        generation: u64,
        sender: EventSender,
        local: Option<crate::local_audio::Input>,
    ) -> Result<bool, String> {
        self.start_with_input(speech_authorized, language, generation, sender, local, None)
    }

    pub(crate) fn start_with_input(
        &mut self,
        speech_authorized: bool,
        language: &str,
        generation: u64,
        sender: EventSender,
        local: Option<crate::local_audio::Input>,
        preferred_uid: Option<&str>,
    ) -> Result<bool, String> {
        main_thread()
            .inspect_err(|_| self.capture_diagnostics = Some(CaptureDiagnostics::default()))?;
        self.cancel();
        self.capture_diagnostics = Some(CaptureDiagnostics::default());
        let capture_started = StageTimer::start(Instant::now);
        let media_type = unsafe { AVMediaTypeAudio }.ok_or("系统音频接口不可用。")?;
        if unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) }
            != AVAuthorizationStatus::Authorized
        {
            return Err("麦克风权限未获允许，无法录音。".into());
        }
        let events = Arc::new(Events {
            generation,
            sender,
            active: AtomicBool::new(true),
        });
        self.events = Some(events.clone());
        let result = (|| {
            let (route, input) = OpenInput::select(preferred_uid, |format| {
                validate_format(format).map_err(str::to_owned)
            })?;
            let format = input.format.clone();
            let recording = Arc::new(
                storage::default_storage()
                    .and_then(|s| s.create_recording())
                    .map_err(|e| e.to_string())?,
            );
            self.recording = Some(recording.clone());
            let writer = AacWriter::new(recording.path(), &format)?;
            recording.secure_audio_file().map_err(|e| e.to_string())?;
            let local_mode = local.is_some();
            let speech = if local_mode {
                None
            } else if speech_authorized {
                match Speech::prepare(language) {
                    Ok(speech) => Some(speech),
                    Err(error) => {
                        events.send(AudioEvent::Status(format!(
                            "{error} 将仅录音并在结束后转写。"
                        )));
                        None
                    }
                }
            } else {
                events.send(AudioEvent::Status(
                    "语音识别权限未获允许，将仅录音并在结束后转写。".into(),
                ));
                None
            };
            let live = speech.is_some();
            let speech_active = Arc::new(AtomicBool::new(live));
            let request = speech.as_ref().map(|s| s.request.clone());
            let sink = Arc::new(Sink {
                state: Mutex::new(SinkState {
                    format: format.clone(),
                    maximum_frames: (unsafe { format.sampleRate() } * MAXIMUM_SECONDS as f64)
                        .floor() as u64,
                    frames: 0,
                    accepting: true,
                    valid: true,
                    diagnostics: CaptureObservation::new(capture_started),
                    writer: Some(writer),
                    request,
                    local,
                    local_error: None,
                }),
                events: events.clone(),
                speech_active: speech_active.clone(),
            });
            self.sink = Some(sink.clone());
            self.speech = speech;
            if let Some(speech) = self.speech.as_mut() {
                speech.start(events.clone(), speech_active);
            }
            input.attach(move |buffer| sink.consume(buffer));
            self.input = Some(input);
            self.route = Some(route);
            self.recovered_input = false;
            Ok(live)
        })();
        if result.is_err() {
            self.cancel();
            self.mark_capture_unknown();
        } else {
            self.capture_diagnostics = None;
        }
        result
    }

    pub(crate) fn route(&self) -> Option<&Route> {
        self.route.as_ref()
    }

    /// Replace only the engine and tap. Never restart a transcription job, reset
    /// the recording clock, or open a second writer. Normalize a replacement
    /// tap before it reaches audio consumers that require a fixed format.
    pub(crate) fn recover_input(&mut self, preferred_uid: Option<&str>) -> Result<(), String> {
        let (Some(input), Some(route)) = (self.input.as_mut(), self.route.as_ref()) else {
            return Ok(());
        };
        if input.healthy(route, false) {
            return Ok(());
        }
        self.route = None;
        self.input.take(); // Drain and revoke old tap access before opening another one.
        if std::mem::replace(&mut self.recovered_input, true) {
            return Err("麦克风再次断开，已停止本轮录音；请检查设备后重新开始。".into());
        }
        let Some(sink) = self.sink.clone() else {
            return Err("录音输入已关闭，请重新开始。".into());
        };
        let format = sink
            .state
            .lock()
            .map_err(|_| "录音写入锁失效。")?
            .format
            .clone();
        let (route, (input, bridge)) =
            OpenInput::open_prepared(preferred_uid, true, |new_format| {
                recovery_pcm::FormatBridge::new(new_format, &format)
            })
            .map_err(|error| format!("麦克风已断开，无法继续本轮录音：{error} 请重新开始。"))?;
        if let Some(bridge) = bridge {
            let bridge = Mutex::new(bridge);
            input.attach(move |buffer| {
                let result = bridge
                    .lock()
                    .map_err(|_| "备用麦克风转换锁失效。".to_owned())
                    .and_then(|mut bridge| {
                        sink.observe_replacement(buffer);
                        bridge.convert(buffer, |output| sink.consume_pcm(output, false))
                    });
                if let Err(error) = result {
                    sink.fail_route_conversion(error);
                }
            });
        } else {
            input.attach(move |buffer| sink.consume(buffer));
        }
        self.input = Some(input);
        self.route = Some(route);
        Ok(())
    }

    /// The settled capture snapshot survives stop, cancellation, and release
    /// until the next start attempt. The caller owns the session association.
    #[allow(dead_code)] // Read by the controller when the receipt is integrated.
    pub(crate) fn capture_diagnostics(&self) -> Option<CaptureDiagnostics> {
        self.capture_diagnostics
    }

    pub(crate) fn live_level(&self) -> Option<u8> {
        // The main run loop never waits for the tap or the encoder.
        let state = self.sink.as_ref()?.state.try_lock().ok()?;
        if !state.accepting {
            return None;
        }
        state.diagnostics.live_level(Instant::now())
    }

    fn mark_capture_unknown(&mut self) {
        self.capture_diagnostics
            .get_or_insert_with(CaptureDiagnostics::default)
            .signal = InputSignal::Unknown;
    }

    /// Revoke callback delivery, stop/remove the tap, drain conversion, close the
    /// AAC writer, and validate the finalized file before publishing its Arc.
    /// A failed writer can never produce an uploadable recording.
    pub(crate) fn lease(&self) -> Option<Arc<Recording>> {
        self.recording.clone()
    }

    pub fn stop(&mut self) -> Result<Arc<Recording>, String> {
        main_thread()?;
        let interrupted = self
            .input
            .as_mut()
            .zip(self.route.as_ref())
            .is_some_and(|(input, route)| !input.healthy(route, true));
        self.stop_native();
        if interrupted {
            self.failure = Some("麦克风在结束录音前已断开；未转写本轮音频，请重新开始。".into());
            self.route = None;
            self.mark_capture_unknown();
        }
        if let Some(error) = &self.failure {
            return Err(error.clone());
        }
        let recording = self
            .recording
            .as_ref()
            .ok_or_else(|| "当前没有录音。".to_owned())?;
        if !self.finalized {
            if let Err(error) = recording
                .validate_finalized()
                .map_err(|e| e.to_string())
                .and_then(|_| validate_aac(recording.path()))
            {
                self.failure = Some(error.clone());
                self.mark_capture_unknown();
                return Err(error);
            }
            self.finalized = true;
        }
        Ok(recording.clone())
    }

    /// Stops and releases this recorder's lease reference. A worker's Arc keeps
    /// its recording alive until that worker finishes reading it.
    pub fn cancel(&mut self) {
        self.stop_native();
        self.route = None;
        self.recording = None;
        self.finalized = false;
        self.failure = None;
    }

    /// Call after successful network completion. For a retryable network failure,
    /// retain the Recorder and call stop() again to obtain the same finalized Arc.
    pub fn finish(&mut self) {
        self.cancel();
    }

    fn stop_native(&mut self) {
        if let Some(events) = self.events.take() {
            events.active.store(false, Ordering::Release);
        }
        self.input.take();
        if let Some(sink) = self.sink.take() {
            let (result, diagnostics) = sink.finish();
            self.capture_diagnostics = Some(diagnostics);
            if let Err(error) = result {
                self.failure = Some(error);
            }
        }
        if let Some(mut speech) = self.speech.take() {
            speech.cancel();
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn main_thread() -> Result<(), String> {
    MainThreadMarker::new()
        .map(|_| ())
        .ok_or_else(|| "录音操作必须在主线程执行。".into())
}
fn native_error(error: Retained<NSError>) -> String {
    error.localizedDescription().to_string()
}
fn speech_authorized() -> bool {
    unsafe {
        SFSpeechRecognizer::authorizationStatus()
            == SFSpeechRecognizerAuthorizationStatus::Authorized
    }
}
struct PermissionReply {
    generation: u64,
    sender: EventSender,
    sent: AtomicBool,
    use_speech: bool,
}
impl PermissionReply {
    fn send(&self, microphone: bool, speech: bool) {
        if !self.sent.swap(true, Ordering::AcqRel) {
            let _ = self.sender.send((
                self.generation,
                AudioEvent::Permissions { microphone, speech },
            ));
        }
    }
}
fn request_speech_permission(reply: Arc<PermissionReply>, microphone: bool) {
    if !reply.use_speech {
        reply.send(microphone, false);
        return;
    }
    let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
    if status == SFSpeechRecognizerAuthorizationStatus::NotDetermined {
        let callback = RcBlock::new(move |status| {
            reply.send(
                microphone,
                status == SFSpeechRecognizerAuthorizationStatus::Authorized,
            )
        });
        unsafe { SFSpeechRecognizer::requestAuthorization(&callback) };
    } else {
        reply.send(
            microphone,
            status == SFSpeechRecognizerAuthorizationStatus::Authorized,
        )
    }
}
struct Events {
    generation: u64,
    sender: EventSender,
    active: AtomicBool,
}
impl Events {
    fn send(&self, event: AudioEvent) {
        if self.active.load(Ordering::Acquire) {
            let _ = self.sender.send((self.generation, event));
        }
    }
}

struct Speech {
    recognizer: Retained<SFSpeechRecognizer>,
    request: Retained<SFSpeechAudioBufferRecognitionRequest>,
    task: Option<Retained<SFSpeechRecognitionTask>>,
}
impl Speech {
    fn prepare(language: &str) -> Result<Self, String> {
        let recognizer = if language.trim().is_empty() {
            unsafe { SFSpeechRecognizer::init(SFSpeechRecognizer::alloc()) }
        } else {
            let supported = unsafe { SFSpeechRecognizer::supportedLocales() }.allObjects();
            let supported: Vec<_> = supported
                .iter()
                .map(|l| l.localeIdentifier().to_string())
                .collect();
            let preferred: Vec<_> = NSLocale::preferredLanguages()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let locale = resolve_locale(language, &supported, &preferred)
                .ok_or("系统不支持所选识别语言。")?;
            let locale =
                NSLocale::initWithLocaleIdentifier(NSLocale::alloc(), &NSString::from_str(&locale));
            unsafe { SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale) }
        }
        .ok_or("系统不支持所选识别语言。")?;
        unsafe {
            if !recognizer.isAvailable() {
                return Err("系统语音识别当前不可用。".into());
            }
            if !recognizer.supportsOnDeviceRecognition() {
                return Err("所选语言的本机识别不可用，可能尚未安装语言模型。".into());
            }
            let request = SFSpeechAudioBufferRecognitionRequest::new();
            // Set before creating a task. Never permit a server Speech fallback.
            request.setRequiresOnDeviceRecognition(true);
            request.setShouldReportPartialResults(true);
            Ok(Self {
                recognizer,
                request,
                task: None,
            })
        }
    }

    fn start(&mut self, events: Arc<Events>, active: Arc<AtomicBool>) {
        let callback = RcBlock::new(
            move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
                if !active.load(Ordering::Acquire) {
                    return;
                }
                // Snapshot native values here; no result/request object crosses into UI.
                if let Some(result) = unsafe { result.as_ref() } {
                    let text = unsafe { result.bestTranscription().formattedString() }.to_string();
                    if !text.is_empty() {
                        events.send(AudioEvent::Partial(text));
                    }
                    if unsafe { result.isFinal() } {
                        active.store(false, Ordering::Release);
                    }
                }
                if let Some(error) = unsafe { error.as_ref() }
                    && active.swap(false, Ordering::AcqRel)
                {
                    events.send(AudioEvent::Status(speech_failure(
                        &error.domain().to_string(),
                        error.code(),
                        &error.localizedDescription().to_string(),
                    )));
                }
            },
        );
        self.task = Some(unsafe {
            self.recognizer
                .recognitionTaskWithRequest_resultHandler(&self.request, &callback)
        });
    }
    fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            unsafe { task.cancel() };
        }
    }
}

fn speech_failure(domain: &str, code: isize, description: &str) -> String {
    if domain == "kLSRErrorDomain" && code == 201 {
        return "本机实时预览不可用：系统 Siri 或听写未启用（kLSRErrorDomain/201）。请检查系统设置中的 Siri/键盘听写；继续录音，停止后完成最终转写。".into();
    }
    let description: String = description.chars().take(160).collect();
    format!("本机实时预览失败（{domain}/{code}）：{description}。继续录音，停止后完成最终转写。")
}

fn locale_key(value: &str) -> String {
    value.trim().replace('_', "-").to_ascii_lowercase()
}
fn resolve_locale(language: &str, supported: &[String], preferred: &[String]) -> Option<String> {
    let wanted = locale_key(language);
    let mut candidates = supported.to_vec();
    candidates.sort();
    if let Some(exact) = candidates.iter().find(|l| locale_key(l) == wanted) {
        return Some(exact.clone());
    }
    if wanted.is_empty() || wanted.contains('-') {
        return None;
    }
    candidates.retain(|l| locale_key(l).split('-').next() == Some(wanted.as_str()));
    for preference in preferred
        .iter()
        .map(String::as_str)
        .chain(["zh-CN", "en-US"])
    {
        if let Some(exact) = candidates
            .iter()
            .find(|l| locale_key(l) == locale_key(preference))
        {
            return Some(exact.clone());
        }
    }
    candidates.into_iter().next()
}

struct Sink {
    state: Mutex<SinkState>,
    events: Arc<Events>,
    speech_active: Arc<AtomicBool>,
}
struct SinkState {
    format: Retained<AVAudioFormat>,
    maximum_frames: u64,
    frames: u64,
    accepting: bool,
    valid: bool,
    diagnostics: CaptureObservation,
    writer: Option<AacWriter>,
    request: Option<Retained<SFSpeechAudioBufferRecognitionRequest>>,
    local: Option<crate::local_audio::Input>,
    local_error: Option<String>,
}
// SAFETY: The immutable format and the writer/request are created before tap
// publication. Every subsequent access, including append, endAudio, conversion,
// close and release, is serialized by Sink::state. PCM buffers are borrowed only
// during consume. No engine, recognizer or recognition task enters this state.
unsafe impl Send for SinkState {}

impl Sink {
    fn fail_route_conversion(&self, error: String) {
        if let Ok(mut state) = self.state.lock() {
            if !state.accepting {
                return;
            }
            state.accepting = false;
            state.valid = false;
            state.diagnostics.unavailable();
        }
        self.events.send(AudioEvent::Failed(format!(
            "备用麦克风不可用，请重新录音：{error}"
        )));
    }
    fn observe_replacement(&self, buffer: &AVAudioPCMBuffer) {
        if let Ok(mut state) = self.state.lock()
            && state.accepting
        {
            state.diagnostics.callback();
            let (original, replacement) =
                unsafe { (state.format.sampleRate(), buffer.format().sampleRate()) };
            let remaining = state.maximum_frames.saturating_sub(state.frames);
            let raw_limit = (remaining as f64 * replacement / original).floor() as usize;
            state.observe_pcm(
                buffer,
                (unsafe { buffer.frameLength() } as usize).min(raw_limit),
            );
        }
    }
    fn consume(&self, buffer: &AVAudioPCMBuffer) {
        self.consume_pcm(buffer, true);
    }
    fn consume_pcm(&self, buffer: &AVAudioPCMBuffer, observe: bool) {
        let event = {
            let Ok(mut state) = self.state.lock() else {
                self.events
                    .send(AudioEvent::Failed("录音写入锁失效，请重新录音。".into()));
                return;
            };
            if !state.accepting {
                return;
            }
            if observe {
                state.diagnostics.callback();
            }
            match state.consume(buffer, self.speech_active.load(Ordering::Acquire), observe) {
                Ok(true) => Some(AudioEvent::LimitReached),
                Ok(false) => state.local_error.take().map(AudioEvent::Failed),
                Err(error) => {
                    state.valid = false;
                    state.accepting = false;
                    state.diagnostics.unavailable();
                    Some(AudioEvent::Failed(format!(
                        "录音写入失败，请重新录音：{error}"
                    )))
                }
            }
        };
        if let Some(event) = event {
            self.events.send(event);
        }
    }
    fn finish(&self) -> (Result<(), String>, CaptureDiagnostics) {
        self.speech_active.store(false, Ordering::Release);
        let Ok(mut state) = self.state.lock() else {
            return (
                Err("录音写入锁失效。".into()),
                CaptureDiagnostics::default(),
            );
        };
        state.accepting = false;
        state.end_speech();
        if let Some(local) = state.local.take() {
            local.finish();
        }
        let result = state
            .writer
            .take()
            .map_or(Ok(()), |mut writer| writer.finish());
        if result.is_err() {
            state.valid = false;
            state.diagnostics.unavailable();
        }
        let result = if !state.valid || state.frames == 0 {
            Err("无法写入完整录音，请重新录音。".into())
        } else {
            result
        };
        (result, state.diagnostics.snapshot())
    }
}
impl SinkState {
    fn observe_pcm(&mut self, buffer: &AVAudioPCMBuffer, count: usize) {
        if count == 0 {
            return;
        }
        match pcm_samples(buffer, MAXIMUM_TAP_FRAMES as usize) {
            Ok(pcm) => self.diagnostics.observe(
                count.min(pcm.frames),
                pcm.channels,
                pcm.sample,
                Instant::now,
            ),
            Err(_) => self.diagnostics.unavailable(),
        }
    }
    fn consume(
        &mut self,
        buffer: &AVAudioPCMBuffer,
        speech_active: bool,
        observe: bool,
    ) -> Result<bool, String> {
        unsafe {
            if buffer.format() != self.format
                || buffer.frameLength() > MAXIMUM_TAP_FRAMES
                || buffer.frameLength() > buffer.frameCapacity()
            {
                return Err("麦克风音频格式不可用。".into());
            }
            let count =
                (buffer.frameLength() as u64).min(self.maximum_frames.saturating_sub(self.frames));
            if count == 0 {
                return Ok(false);
            }
            // Observe only the frames accepted by the current recording limit,
            // before any mixing. Missing observations cannot alter writer/ASR input.
            if observe {
                self.observe_pcm(buffer, count as usize);
            }
            let copy;
            let input = if count < buffer.frameLength() as u64 {
                copy = prefix(buffer, count as u32)?;
                &copy
            } else {
                buffer
            };
            let packet = self.local.as_ref().map(|_| copy_pcm(input)).transpose()?;
            self.writer
                .as_mut()
                .ok_or("录音写入器已关闭。")?
                .write(input)?;
            if let (Some(local), Some(packet)) = (&self.local, packet)
                && let Err(error) = local.send(packet)
            {
                self.local_error = Some(error);
                // Keep the valid AAC writer available for an explicit local retry.
                self.local.take();
            }
            if !speech_active {
                self.end_speech();
            }
            if let Some(request) = &self.request {
                request.appendAudioPCMBuffer(input);
            }
            self.frames += count;
            if self.frames == self.maximum_frames {
                self.accepting = false;
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn end_speech(&mut self) {
        if let Some(request) = self.request.take() {
            unsafe { request.endAudio() };
        }
    }
}

fn validate_format(format: &AVAudioFormat) -> Result<(), &'static str> {
    unsafe {
        let rate = format.sampleRate();
        let channels = format.channelCount();
        let description = format.streamDescription();
        if !rate.is_finite()
            || !(8_000.0..=192_000.0).contains(&rate)
            || !(1..=8).contains(&channels)
            || description.as_ref().mFormatID != u32::from_be_bytes(*b"lpcm")
            || description.as_ref().mBytesPerFrame == 0
        {
            return Err("麦克风音频格式不可用。");
        }
    }
    Ok(())
}

/// Copy only the last permitted prefix, preserving arbitrary native PCM layouts.
fn prefix(input: &AVAudioPCMBuffer, frames: u32) -> Result<Retained<AVAudioPCMBuffer>, String> {
    unsafe {
        let format = input.format();
        let output = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
            AVAudioPCMBuffer::alloc(),
            &format,
            frames,
        )
        .ok_or("无法分配音频缓冲区。")?;
        output.setFrameLength(frames);
        let source = input.audioBufferList();
        let mut destination = output.mutableAudioBufferList();
        let source = source.as_ref();
        let destination = destination.as_mut();
        let bytes = (frames as usize)
            .checked_mul(format.streamDescription().as_ref().mBytesPerFrame as usize)
            .ok_or("音频缓冲区过大。")?;
        let buffers = source.mNumberBuffers as usize;
        if buffers == 0 || buffers > 8 || buffers != destination.mNumberBuffers as usize {
            return Err("音频缓冲区无效。".into());
        }
        let source = std::slice::from_raw_parts(source.mBuffers.as_ptr(), buffers);
        let destination =
            std::slice::from_raw_parts_mut(destination.mBuffers.as_mut_ptr(), buffers);
        for (from, to) in source.iter().zip(destination) {
            if from.mData.is_null()
                || to.mData.is_null()
                || bytes > from.mDataByteSize as usize
                || bytes > to.mDataByteSize as usize
            {
                return Err("音频缓冲区无效。".into());
            }
            std::ptr::copy_nonoverlapping(from.mData.cast::<u8>(), to.mData.cast::<u8>(), bytes);
        }
        Ok(output)
    }
}

/// CoreAudio can drop unlabeled channels when converting directly to mono.
/// An unlabeled stereo format still has the standard left/right convention.
fn needs_explicit_downmix(format: &AVAudioFormat) -> bool {
    const USE_DESCRIPTIONS: u32 = 0;
    const DISCRETE_IN_ORDER: u32 = 147 << 16;
    const UNKNOWN_LAYOUT: u32 = 0xffff << 16;
    const UNKNOWN_LABEL: u32 = u32::MAX;
    const DISCRETE_LABEL: u32 = 400;

    unsafe {
        let channels = format.channelCount() as usize;
        if channels < 2 {
            return false;
        }
        let Some(layout) = format.channelLayout() else {
            return channels > 2;
        };
        let tag = layout.layoutTag();
        if tag == USE_DESCRIPTIONS {
            let raw = layout.layout().as_ptr();
            if (*raw).mNumberChannelDescriptions as usize != channels {
                return false;
            }
            // The native layout contains a variable-length description array.
            let descriptions = std::slice::from_raw_parts(
                std::ptr::addr_of!((*raw).mChannelDescriptions[0]),
                channels,
            );
            return descriptions.iter().all(|description| {
                let label = description.mChannelLabel;
                label == UNKNOWN_LABEL || label == DISCRETE_LABEL || label >> 16 == 1
            });
        }
        matches!(tag & 0xffff_0000, DISCRETE_IN_ORDER | UNKNOWN_LAYOUT)
    }
}

fn mix_to_mono(
    input: &AVAudioPCMBuffer,
    format: &AVAudioFormat,
) -> Result<Retained<AVAudioPCMBuffer>, String> {
    let (channels, frames) = pcm_frames(input, MAXIMUM_TAP_FRAMES as usize)?;
    let length = frames.len() as u32;
    let output = unsafe {
        AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(AVAudioPCMBuffer::alloc(), format, length)
    }
    .ok_or("无法分配音频缓冲区。")?;
    unsafe {
        output.setFrameLength(length);
        let samples =
            std::slice::from_raw_parts_mut((*output.floatChannelData()).as_ptr(), length as usize);
        for (sample, frame) in samples.iter_mut().zip(frames) {
            // Without speaker roles, give each channel equal weight and keep headroom.
            let sum = frame[..channels]
                .iter()
                .map(|value| f64::from(*value))
                .sum::<f64>();
            *sample = (sum / channels as f64) as f32;
            if !sample.is_finite() {
                return Err("PCM 混音产生无效数值。".into());
            }
        }
    }
    Ok(output)
}

/// Owned exclusively by the sink after construction. Conversion drains before
/// releasing the sole AVAudioFile retain, which closes and finalizes its header.
struct AacWriter {
    file: Option<Retained<AVAudioFile>>,
    converter: Option<Retained<AVAudioConverter>>,
    mix_format: Option<Retained<AVAudioFormat>>,
    output_format: Retained<AVAudioFormat>,
    frames: u64,
}
impl AacWriter {
    fn new(path: &Path, input_format: &AVAudioFormat) -> Result<Self, String> {
        validate_format(input_format)?;
        let url = file_url(path)?;
        unsafe {
            let values = [
                NSNumber::numberWithUnsignedInt(AAC),
                NSNumber::numberWithDouble(OUTPUT_RATE),
                NSNumber::numberWithUnsignedInt(1),
                NSNumber::numberWithUnsignedInt(64_000),
                NSNumber::numberWithUnsignedInt(96),
            ];
            let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                &[
                    AVFormatIDKey.ok_or("系统音频接口不可用。")?,
                    AVSampleRateKey.ok_or("系统音频接口不可用。")?,
                    AVNumberOfChannelsKey.ok_or("系统音频接口不可用。")?,
                    AVEncoderBitRateKey.ok_or("系统音频接口不可用。")?,
                    AVEncoderAudioQualityKey.ok_or("系统音频接口不可用。")?,
                ],
                &[&values[0], &values[1], &values[2], &values[3], &values[4]],
            );
            let file = AVAudioFile::initForWriting_settings_commonFormat_interleaved_error(
                AVAudioFile::alloc(),
                &url,
                &settings,
                AVAudioCommonFormat::PCMFormatFloat32,
                false,
            )
            .map_err(native_error)?;
            let output_format = file.processingFormat();
            let mix_format = if needs_explicit_downmix(input_format) {
                Some(
                    AVAudioFormat::initStandardFormatWithSampleRate_channels(
                        AVAudioFormat::alloc(),
                        input_format.sampleRate(),
                        1,
                    )
                    .ok_or("无法创建单声道录音格式。")?,
                )
            } else {
                None
            };
            let converter = AVAudioConverter::initFromFormat_toFormat(
                AVAudioConverter::alloc(),
                mix_format.as_deref().unwrap_or(input_format),
                &output_format,
            )
            .ok_or("无法转换麦克风音频格式。")?;
            converter.setDownmix(true);
            Ok(Self {
                file: Some(file),
                converter: Some(converter),
                mix_format,
                output_format,
                frames: 0,
            })
        }
    }
    fn write(&mut self, input: &AVAudioPCMBuffer) -> Result<(), String> {
        let mixed;
        let input = if let Some(format) = &self.mix_format {
            mixed = mix_to_mono(input, format)?;
            &mixed
        } else {
            input
        };
        let capacity = unsafe {
            ((input.frameLength() as f64 * OUTPUT_RATE / input.format().sampleRate()).ceil() as u32)
                .checked_add(256)
        }
        .ok_or("音频缓冲区过大。")?;
        self.convert(Some(input), capacity)
    }
    fn finish(&mut self) -> Result<(), String> {
        let result = if self.converter.is_some() {
            self.convert(None, 4096)
        } else {
            Ok(())
        };
        self.converter = None;
        // AVAudioFile.close is macOS 15+. Releasing the sole owned reference also
        // closes synchronously and supports the app's older deployment target.
        self.file = None;
        result
    }
    fn convert(&mut self, input: Option<&AVAudioPCMBuffer>, capacity: u32) -> Result<(), String> {
        let converter = self.converter.as_ref().ok_or("音频转换器已关闭。")?;
        let file = self.file.as_ref().ok_or("录音文件已关闭。")?;
        let output = unsafe {
            AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                AVAudioPCMBuffer::alloc(),
                &self.output_format,
                capacity,
            )
        }
        .ok_or("无法分配音频缓冲区。")?;
        // The API's block alias requires a 'static capture. Retain its input for
        // the synchronous conversion; PCM data is accessed only before return.
        let input = input.map(|buffer| unsafe {
            Retained::retain((buffer as *const AVAudioPCMBuffer).cast_mut())
                .expect("nonnull borrowed PCM buffer")
        });
        let supplied = AtomicBool::new(false);
        let feeder = RcBlock::new(
            move |_packets: u32,
                  mut status: NonNull<AVAudioConverterInputStatus>|
                  -> *mut AVAudioBuffer {
                if let Some(input) = &input {
                    if !supplied.swap(true, Ordering::AcqRel) {
                        unsafe { *status.as_mut() = AVAudioConverterInputStatus::HaveData };
                        return Retained::as_ptr(input).cast_mut().cast();
                    }
                    unsafe { *status.as_mut() = AVAudioConverterInputStatus::NoDataNow };
                } else {
                    unsafe { *status.as_mut() = AVAudioConverterInputStatus::EndOfStream };
                }
                std::ptr::null_mut()
            },
        );
        for _ in 0..16 {
            let mut error = None;
            let status = unsafe {
                converter.convertToBuffer_error_withInputFromBlock(
                    &output,
                    Some(&mut error),
                    RcBlock::as_ptr(&feeder),
                )
            };
            if let Some(error) = error {
                return Err(native_error(error));
            }
            if status == AVAudioConverterOutputStatus::Error {
                return Err("音频转换失败。".into());
            }
            unsafe {
                output.setFrameLength(
                    output
                        .frameLength()
                        .min(MAXIMUM_OUTPUT_FRAMES.saturating_sub(self.frames) as u32),
                );
                if output.frameLength() > 0 {
                    file.writeFromBuffer_error(&output).map_err(native_error)?;
                    self.frames += output.frameLength() as u64;
                }
            }
            if status == AVAudioConverterOutputStatus::InputRanDry
                || status == AVAudioConverterOutputStatus::EndOfStream
            {
                return Ok(());
            }
        }
        Err("音频转换未能完成。".into())
    }
}
impl Drop for AacWriter {
    fn drop(&mut self) {
        // Error paths must also close, but must not mask a prior writer failure.
        self.converter = None;
        self.file = None;
    }
}
fn file_url(path: &Path) -> Result<Retained<NSURL>, String> {
    let path = path.to_str().ok_or("录音路径无效。")?;
    Ok(NSURL::fileURLWithPath(&NSString::from_str(path)))
}
/// Copy native PCM while its framework-owned pointers are valid. Mixing and
/// resampling happen later on the inference worker.
fn copy_pcm(buffer: &AVAudioPCMBuffer) -> Result<crate::local_audio::Packet, String> {
    use crate::local_audio::{MAX_PACKET_FRAMES, Packet};
    let (channels, frames) = pcm_frames(buffer, MAX_PACKET_FRAMES)?;
    let mut samples = Vec::with_capacity(frames.len() * channels);
    for frame in frames {
        samples.extend_from_slice(&frame[..channels]);
    }
    let packet = Packet {
        rate: unsafe { buffer.format().sampleRate() },
        channels,
        samples,
    };
    packet.validate()?;
    Ok(packet)
}

fn pcm_frames(
    buffer: &AVAudioPCMBuffer,
    maximum_frames: usize,
) -> Result<(usize, impl ExactSizeIterator<Item = [f32; 8]> + '_), String> {
    let PcmSamples {
        channels,
        frames,
        sample,
    } = pcm_samples(buffer, maximum_frames)?;
    let frames = (0..frames).map(move |frame| {
        let mut samples = [0.; 8];
        for (channel, value) in samples[..channels].iter_mut().enumerate() {
            *value = sample(frame, channel) as f32;
        }
        samples
    });
    Ok((channels, frames))
}

struct PcmSamples<Sample> {
    channels: usize,
    frames: usize,
    sample: Sample,
}

/// Borrow native samples only for the lifetime of the callback's PCM buffer.
/// Full precision lets the observer distinguish digital zero from subnormal PCM.
fn pcm_samples(
    buffer: &AVAudioPCMBuffer,
    maximum_frames: usize,
) -> Result<PcmSamples<impl Fn(usize, usize) -> f64 + '_>, &'static str> {
    unsafe {
        let format = buffer.format();
        validate_format(&format)?;
        let frames = buffer.frameLength() as usize;
        let channels = format.channelCount() as usize;
        if frames == 0 || frames > maximum_frames || buffer.frameLength() > buffer.frameCapacity() {
            return Err("本地 PCM 缓冲区大小无效。");
        }
        let common = format.commonFormat();
        let width = match common {
            AVAudioCommonFormat::PCMFormatFloat32 | AVAudioCommonFormat::PCMFormatInt32 => 4,
            AVAudioCommonFormat::PCMFormatFloat64 => 8,
            AVAudioCommonFormat::PCMFormatInt16 => 2,
            _ => return Err("本地转写不支持麦克风的 PCM 格式。"),
        };
        let interleaved = format.isInterleaved();
        let list = buffer.audioBufferList().as_ref();
        let count = if interleaved { 1 } else { channels };
        if list.mNumberBuffers as usize != count {
            return Err("本地 PCM 通道布局无效。");
        }
        let buffers = std::slice::from_raw_parts(list.mBuffers.as_ptr(), count);
        for native in buffers {
            let native_channels = if interleaved { channels } else { 1 };
            if native.mNumberChannels as usize != native_channels
                || native.mData.is_null()
                || (native.mDataByteSize as usize) < frames * native_channels * width
            {
                return Err("本地 PCM 缓冲区不完整。");
            }
        }
        let sample = move |frame: usize, channel: usize| {
            assert!(frame < frames && channel < channels);
            let (native, index) = if interleaved {
                (&buffers[0], frame * channels + channel)
            } else {
                (&buffers[channel], frame)
            };
            let pointer = native.mData.cast::<u8>().add(index * width);
            match common {
                AVAudioCommonFormat::PCMFormatFloat32 => {
                    f64::from(pointer.cast::<f32>().read_unaligned())
                }
                AVAudioCommonFormat::PCMFormatFloat64 => pointer.cast::<f64>().read_unaligned(),
                AVAudioCommonFormat::PCMFormatInt16 => {
                    f64::from(pointer.cast::<i16>().read_unaligned()) / 32768.0
                }
                AVAudioCommonFormat::PCMFormatInt32 => {
                    f64::from(pointer.cast::<i32>().read_unaligned()) / 2147483648.0
                }
                _ => unreachable!(),
            }
        };
        Ok(PcmSamples {
            channels,
            frames,
            sample,
        })
    }
}

/// Used only by worker-owned local retries and explicit public-fixture tests.
/// Reads at most one small native buffer at a time, under the caller's lease.
pub(crate) fn read_pcm(
    path: &Path,
    mut consume: impl FnMut(crate::local_audio::Packet) -> Result<(), String>,
) -> Result<(), String> {
    autoreleasepool(|_| unsafe {
        let url = file_url(path)?;
        let file = AVAudioFile::initForReading_commonFormat_interleaved_error(
            AVAudioFile::alloc(),
            &url,
            AVAudioCommonFormat::PCMFormatFloat32,
            false,
        )
        .map_err(native_error)?;
        let format = file.processingFormat();
        validate_format(&format)?;
        let length = file.length();
        if length <= 0 || length as f64 > format.sampleRate() * MAXIMUM_SECONDS as f64 {
            return Err("本地转写录音为空或超过 180 秒。".into());
        }
        let buffer = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
            AVAudioPCMBuffer::alloc(),
            &format,
            4096,
        )
        .ok_or("无法分配本地转写音频缓冲区。")?;
        let mut read = 0;
        while read < length {
            let frames = (length - read).min(4096) as u32;
            file.readIntoBuffer_frameCount_error(&buffer, frames)
                .map_err(|e| format!("无法读取本地录音：{}", native_error(e)))?;
            if buffer.frameLength() == 0 {
                break;
            }
            read += buffer.frameLength() as i64;
            if read > length {
                return Err("本地录音长度发生变化。".into());
            }
            consume(copy_pcm(&buffer)?)?;
        }
        if read != length {
            return Err("本地录音未能完整读取。".into());
        }
        Ok(())
    })
}

fn validate_aac(path: &Path) -> Result<(), String> {
    let url = file_url(path)?;
    unsafe {
        let file =
            AVAudioFile::initForReading_error(AVAudioFile::alloc(), &url).map_err(native_error)?;
        let format = file.fileFormat();
        let length = file.length();
        if format.streamDescription().as_ref().mFormatID != AAC
            || format.sampleRate() != OUTPUT_RATE
            || format.channelCount() != 1
            || length <= 0
            || length as u64 > MAXIMUM_OUTPUT_FRAMES
        {
            return Err("录音文件未能完整编码。".into());
        }
        // Decode both ends after close: a missing/truncated AAC header or tail
        // cannot be accepted merely because a nonempty file exists.
        let buffer = AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
            AVAudioPCMBuffer::alloc(),
            &file.processingFormat(),
            1024,
        )
        .ok_or("无法验证录音文件。")?;
        file.readIntoBuffer_error(&buffer).map_err(native_error)?;
        if buffer.frameLength() == 0 {
            return Err("录音文件为空。".into());
        }
        file.setFramePosition((length - 1024).max(0));
        file.readIntoBuffer_error(&buffer).map_err(native_error)?;
        if buffer.frameLength() == 0 {
            return Err("录音文件不完整。".into());
        }
    }
    Ok(())
}

/// Private native fixture used only by the OS-main-thread regression target.
/// It installs no engine/tap/Speech object and never queries permissions.
#[cfg(test)]
#[allow(dead_code)] // Called by the separately compiled main-thread test target.
pub(crate) mod stop_failure_fixture {
    use super::*;
    use std::{path::PathBuf, sync::Weak};

    pub(crate) enum Input {
        Empty,
        Damaged,
        Valid,
    }

    pub(crate) fn recorder(
        store: &storage::Storage,
        generation: u64,
        sender: EventSender,
        input: Input,
    ) -> (Recorder, Weak<Recording>, PathBuf) {
        main_thread().unwrap();
        let recording = Arc::new(store.create_recording().unwrap());
        let path = recording.path().to_path_buf();
        let weak = Arc::downgrade(&recording);
        let (format, buffer) = tests::synthetic_buffer(48_000.0, 1, 4800);
        let writer = AacWriter::new(recording.path(), &format).unwrap();
        recording.secure_audio_file().unwrap();
        let events = Arc::new(Events {
            generation,
            sender,
            active: AtomicBool::new(true),
        });
        let sink = Arc::new(Sink {
            state: Mutex::new(SinkState {
                format,
                maximum_frames: 48_000 * MAXIMUM_SECONDS,
                frames: 0,
                accepting: true,
                valid: true,
                diagnostics: CaptureObservation::new(StageTimer::start(Instant::now)),
                writer: Some(writer),
                request: None,
                local: None,
                local_error: None,
            }),
            events: events.clone(),
            speech_active: Arc::new(AtomicBool::new(false)),
        });
        if !matches!(input, Input::Empty) {
            for _ in 0..3 {
                sink.consume(&buffer);
            }
            let state = sink.state.lock().unwrap();
            assert!(state.valid);
            assert_eq!(state.frames, 14_400);
        }
        if matches!(input, Input::Damaged) {
            // Same unavailable-AVAudioFile injection as the native writer unit
            // test. The actual consume method emits the generation-tagged event.
            sink.state.lock().unwrap().writer.as_mut().unwrap().file = None;
            sink.consume(&buffer);
            let state = sink.state.lock().unwrap();
            assert!(!state.valid && !state.accepting);
            assert_eq!(state.frames, 14_400);
        }
        let mut recorder = Recorder::new();
        recorder.recording = Some(recording);
        recorder.sink = Some(sink);
        recorder.events = Some(events);
        (recorder, weak, path)
    }

    pub(crate) fn owns_recording(recorder: &Recorder) -> bool {
        recorder.recording.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_pcm_copy_checks_native_layout_and_nonfinite_samples_without_devices() {
        for interleaved in [false, true] {
            let format = unsafe {
                AVAudioFormat::initWithCommonFormat_sampleRate_channels_interleaved(
                    AVAudioFormat::alloc(),
                    AVAudioCommonFormat::PCMFormatInt16,
                    44_100.5,
                    2,
                    interleaved,
                )
                .unwrap()
            };
            let buffer = unsafe {
                AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                    AVAudioPCMBuffer::alloc(),
                    &format,
                    3,
                )
                .unwrap()
            };
            unsafe {
                buffer.setFrameLength(3);
                let mut list = buffer.mutableAudioBufferList();
                let list = list.as_mut();
                let buffers = std::slice::from_raw_parts_mut(
                    list.mBuffers.as_mut_ptr(),
                    list.mNumberBuffers as usize,
                );
                for native in buffers {
                    let samples = std::slice::from_raw_parts_mut(
                        native.mData.cast::<i16>(),
                        3 * native.mNumberChannels as usize,
                    );
                    samples.fill(16384);
                }
            }
            let packet = copy_pcm(&buffer).unwrap();
            assert_eq!(packet.rate, 44_100.5);
            assert_eq!(packet.channels, 2);
            assert_eq!(packet.samples, vec![0.5; 6]);
        }
        let (_, buffer) = synthetic_buffer(48000., 1, 8);
        unsafe {
            (*buffer.floatChannelData()).as_ptr().write(f32::NAN);
        }
        assert!(copy_pcm(&buffer).is_err());
        let (_, large) =
            synthetic_buffer(48000., 1, crate::local_audio::MAX_PACKET_FRAMES as u32 + 1);
        assert!(copy_pcm(&large).is_err());
    }
    #[test]
    fn local_permission_policy_replies_without_querying_system_speech() {
        let (sender, receiver) = std::sync::mpsc::channel();
        request_speech_permission(
            Arc::new(PermissionReply {
                generation: 78,
                sender,
                use_speech: false,
                sent: AtomicBool::new(false),
            }),
            true,
        );
        assert_eq!(
            receiver.try_recv().unwrap(),
            (
                78,
                AudioEvent::Permissions {
                    microphone: true,
                    speech: false
                }
            )
        );
    }
    #[test]
    fn locale_resolution_never_substitutes_explicit_region() {
        let supported = ["en_GB", "en_US", "zh_CN", "zh_TW"].map(str::to_owned);
        assert_eq!(
            resolve_locale("en", &supported, &["en-GB".into()]),
            Some("en_GB".into())
        );
        assert_eq!(resolve_locale("en", &supported, &[]), Some("en_US".into()));
        assert_eq!(
            resolve_locale("ZH-cn", &supported, &[]),
            Some("zh_CN".into())
        );
        assert_eq!(resolve_locale("en-AU", &supported, &[]), None);
        assert_eq!(resolve_locale("ja", &supported, &[]), None);
    }

    pub(super) fn synthetic_buffer(
        rate: f64,
        channels: u32,
        frames: u32,
    ) -> (Retained<AVAudioFormat>, Retained<AVAudioPCMBuffer>) {
        let format = unsafe {
            AVAudioFormat::initStandardFormatWithSampleRate_channels(
                AVAudioFormat::alloc(),
                rate,
                channels,
            )
            .unwrap()
        };
        let input = unsafe {
            AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                AVAudioPCMBuffer::alloc(),
                &format,
                frames,
            )
            .unwrap()
        };
        unsafe {
            input.setFrameLength(frames);
            let data = input.floatChannelData();
            for channel in 0..channels as usize {
                let samples =
                    std::slice::from_raw_parts_mut((*data.add(channel)).as_ptr(), frames as usize);
                for (frame, sample) in samples.iter_mut().enumerate() {
                    *sample =
                        (frame as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 0.1;
                }
            }
        }
        (format, input)
    }

    fn diagnostic_sink(
        recording: &Recording,
        format: &Retained<AVAudioFormat>,
        generation: u64,
    ) -> (Arc<Sink>, std::sync::mpsc::Receiver<(u64, AudioEvent)>) {
        let writer = AacWriter::new(recording.path(), format).unwrap();
        recording.secure_audio_file().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let sink = Arc::new(Sink {
            state: Mutex::new(SinkState {
                format: format.clone(),
                maximum_frames: 48000,
                frames: 0,
                accepting: true,
                valid: true,
                diagnostics: CaptureObservation::new(StageTimer::start(Instant::now)),
                writer: Some(writer),
                request: None,
                local: None,
                local_error: None,
            }),
            events: Arc::new(Events {
                generation,
                sender,
                active: AtomicBool::new(true),
            }),
            speech_active: Arc::new(AtomicBool::new(false)),
        });
        (sink, receiver)
    }

    #[test]
    fn capture_distinguishes_no_callbacks_empty_buffers_and_pre_mix_signal() {
        let temp = tempfile::tempdir().unwrap();
        let store =
            storage::Storage::at(temp.path().canonicalize().unwrap().join(storage::NAMESPACE));
        let (format, buffer) = synthetic_buffer(48_000., 2, 4800);
        for (case, amplitudes, frames, expected) in [
            ("no callback", [0., 0.], None, InputSignal::NoCallbacks),
            ("empty", [0., 0.], Some(0), InputSignal::NoFrames),
            (
                "silence",
                [0., -0.],
                Some(4800),
                InputSignal::DigitalSilence,
            ),
            (
                "quiet",
                [0.0001, 0.0001],
                Some(4800),
                InputSignal::LowEnergy,
            ),
            ("one side", [0., 0.05], Some(4800), InputSignal::Signal),
            (
                "opposite phase",
                [0.05, -0.05],
                Some(4800),
                InputSignal::Signal,
            ),
        ] {
            let recording = store.create_recording().unwrap();
            let (sink, events) = diagnostic_sink(&recording, &format, 82);
            if let Some(frames) = frames {
                unsafe {
                    buffer.setFrameLength(frames);
                    for (channel, amplitude) in amplitudes.into_iter().enumerate() {
                        let data = (*buffer.floatChannelData().add(channel)).as_ptr();
                        for frame in 0..frames as usize {
                            let tone =
                                (frame as f32 * 440. * std::f32::consts::TAU / 48_000.).sin();
                            data.add(frame).write(amplitude * tone);
                        }
                    }
                }
                sink.consume(&buffer);
            }
            let (result, diagnostics) = sink.finish();
            let frames_observed = frames.unwrap_or(0);
            assert_eq!(result.is_ok(), frames_observed > 0, "{case}");
            assert_eq!(diagnostics.signal, expected, "{case}");
            assert_eq!(
                diagnostics.callbacks,
                Some(u64::from(frames.is_some())),
                "{case}"
            );
            assert_eq!(
                diagnostics.observed_frames,
                Some(frames_observed.into()),
                "{case}"
            );
            assert_eq!(
                diagnostics.timings.capture_first_frame.is_some(),
                frames_observed > 0,
                "{case}"
            );
            assert_eq!(diagnostics.timings.preparation, None);
            assert_eq!(diagnostics.timings.transcription, None);
            assert_eq!(diagnostics.timings.delivery, None);
            assert!(events.try_recv().is_err(), "{case}");
        }
    }

    #[test]
    fn capture_ignores_signal_beyond_the_recording_limit() {
        let temp = tempfile::tempdir().unwrap();
        let recording =
            storage::Storage::at(temp.path().canonicalize().unwrap().join(storage::NAMESPACE))
                .create_recording()
                .unwrap();
        let (format, buffer) = synthetic_buffer(48_000., 1, 4800);
        let (sink, events) = diagnostic_sink(&recording, &format, 82);
        sink.state.lock().unwrap().maximum_frames = 2400;
        unsafe {
            let samples = (*buffer.floatChannelData()).as_ptr();
            samples.write_bytes(0, 2400);
        }
        sink.consume(&buffer);
        let (result, diagnostics) = sink.finish();
        result.unwrap();
        assert_eq!(diagnostics.signal, InputSignal::DigitalSilence);
        assert_eq!(diagnostics.observed_frames, Some(2400));
        assert_eq!(
            events.try_iter().collect::<Vec<_>>(),
            [(82, AudioEvent::LimitReached)]
        );
    }

    #[test]
    fn replacement_diagnostics_are_raw_single_callback_and_limited_by_remaining_time() {
        let dir = tempfile::tempdir().unwrap();
        let store =
            storage::Storage::at(dir.path().canonicalize().unwrap().join(storage::NAMESPACE));
        let recording = store.create_recording().unwrap();
        let (original, normalized) = synthetic_buffer(48_000., 1, 480);
        let (_, incoming) = synthetic_buffer(16_000., 2, 480);
        unsafe {
            let samples = (*normalized.floatChannelData()).as_ptr();
            std::slice::from_raw_parts_mut(samples, 480).fill(0.0);
        }
        let (sink, _) = diagnostic_sink(&recording, &original, 85);
        sink.state.lock().unwrap().maximum_frames = 480;
        sink.observe_replacement(&incoming);
        sink.consume_pcm(&normalized, false);
        let (result, diagnostics) = sink.finish();
        result.unwrap();
        assert_eq!(diagnostics.signal, InputSignal::Signal);
        assert_eq!(diagnostics.callbacks, Some(1));
        assert_eq!(diagnostics.observed_frames, Some(160));
    }

    #[test]
    fn float64_observation_retains_nonzero_below_float32_precision() {
        let format = unsafe {
            AVAudioFormat::initWithCommonFormat_sampleRate_channels_interleaved(
                AVAudioFormat::alloc(),
                AVAudioCommonFormat::PCMFormatFloat64,
                48_000.,
                1,
                true,
            )
            .unwrap()
        };
        let buffer = unsafe {
            AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(AVAudioPCMBuffer::alloc(), &format, 1)
                .unwrap()
        };
        unsafe {
            buffer.setFrameLength(1);
            let buffer_list = buffer.mutableAudioBufferList();
            let native = &buffer_list.as_ref().mBuffers[0];
            native.mData.cast::<f64>().write_unaligned(1e-100);
        }
        assert_eq!(copy_pcm(&buffer).unwrap().samples, [0.0]);
        let pcm = pcm_samples(&buffer, 1).unwrap();
        let mut diagnostics = CaptureObservation::new(StageTimer::start(Instant::now));
        diagnostics.callback();
        diagnostics.observe(pcm.frames, pcm.channels, pcm.sample, Instant::now);
        assert_eq!(diagnostics.snapshot().signal, InputSignal::LowEnergy);
    }

    #[test]
    fn stopped_or_cancelled_capture_keeps_only_its_own_snapshot() {
        let temp = tempfile::tempdir().unwrap();
        let store =
            storage::Storage::at(temp.path().canonicalize().unwrap().join(storage::NAMESPACE));
        let (format, good) = synthetic_buffer(48_000., 2, 4800);
        let (_, wrong_format) = synthetic_buffer(48_000., 1, 4800);
        let mut first = Recorder::new();
        let recording = Arc::new(store.create_recording().unwrap());
        let (old, old_events) = diagnostic_sink(&recording, &format, 82);
        first.recording = Some(recording);
        first.sink = Some(old.clone());
        first.events = Some(old.events.clone());
        assert_eq!(first.capture_diagnostics(), None);
        old.consume(&good);
        first.cancel();
        assert!(first.lease().is_none());
        let cancelled = first.capture_diagnostics().unwrap();
        assert_eq!(cancelled.signal, InputSignal::Signal);
        assert_eq!(cancelled.callbacks, Some(1));
        first.cancel();

        let mut second = Recorder::new();
        let recording = Arc::new(store.create_recording().unwrap());
        let (current, events) = diagnostic_sink(&recording, &format, 83);
        second.recording = Some(recording);
        second.sink = Some(current.clone());
        second.events = Some(current.events.clone());
        old.consume(&wrong_format);
        current.consume(&wrong_format);
        assert!(matches!(events.try_recv(), Ok((83, AudioEvent::Failed(_)))));
        assert!(old_events.try_recv().is_err());
        second.stop_native();
        assert!(second.failure.is_some());
        let failed = second.capture_diagnostics().unwrap();
        assert_eq!(failed.signal, InputSignal::Unknown);
        assert_eq!(failed.callbacks, Some(1));
        assert_eq!(failed.observed_frames, Some(0));
        assert_eq!(failed.timings.capture_first_frame, None);
        second.cancel();
        assert_eq!(second.capture_diagnostics(), Some(failed));
        assert_eq!(first.capture_diagnostics(), Some(cancelled));
        assert_eq!(old.finish().1, cancelled);
    }

    #[test]
    fn sink_trims_limit_once_and_rejects_late_buffers_after_finalization() {
        let temp = tempfile::tempdir().unwrap();
        let recording =
            storage::Storage::at(temp.path().canonicalize().unwrap().join(storage::NAMESPACE))
                .create_recording()
                .unwrap();
        let (format, input) = synthetic_buffer(48_000.0, 2, 4800);
        let writer = AacWriter::new(recording.path(), &format).unwrap();
        recording.secure_audio_file().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let (local, incoming) = crate::local_audio::channel();
        let sink = Sink {
            state: Mutex::new(SinkState {
                format,
                maximum_frames: 5001,
                frames: 0,
                accepting: true,
                valid: true,
                diagnostics: CaptureObservation::new(StageTimer::start(Instant::now)),
                writer: Some(writer),
                request: None,
                local: Some(local),
                local_error: None,
            }),
            events: Arc::new(Events {
                generation: 42,
                sender,
                active: AtomicBool::new(true),
            }),
            speech_active: Arc::new(AtomicBool::new(false)),
        };
        sink.consume(&input);
        sink.consume(&input); // The final buffer is copied down to exactly 201 frames.
        sink.consume(&input);
        assert_eq!(sink.state.lock().unwrap().frames, 5001);
        assert_eq!(
            receiver.try_iter().collect::<Vec<_>>(),
            [(42, AudioEvent::LimitReached)]
        );
        let (result, diagnostics) = sink.finish();
        result.unwrap();
        assert_eq!(diagnostics.signal, InputSignal::Signal);
        assert_eq!(diagnostics.callbacks, Some(2));
        assert_eq!(diagnostics.observed_frames, Some(5001));
        let expected = copy_pcm(&input).unwrap();
        let packets: Vec<_> = incoming.receiver.try_iter().collect();
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0].samples, expected.samples);
        assert_eq!(
            packets[1].samples,
            expected.samples[..201 * expected.channels]
        );
        assert_eq!(
            packets
                .iter()
                .map(|p| p.samples.len() / p.channels)
                .collect::<Vec<_>>(),
            [4800, 201]
        );
        incoming.completed().unwrap();
        recording.validate_finalized().unwrap();
        validate_aac(recording.path()).unwrap();
        let bytes = std::fs::read(recording.path()).unwrap();
        sink.consume(&input);
        assert_eq!(std::fs::read(recording.path()).unwrap(), bytes);
        assert!(receiver.try_recv().is_err());
        assert_eq!(sink.finish().1, diagnostics);
    }

    #[test]
    fn local_overflow_fails_explicitly_but_preserves_valid_aac_for_retry() {
        let temp = tempfile::tempdir().unwrap();
        let recording =
            storage::Storage::at(temp.path().canonicalize().unwrap().join(storage::NAMESPACE))
                .create_recording()
                .unwrap();
        let (format, buffer) = synthetic_buffer(48000., 1, 4800);
        let writer = AacWriter::new(recording.path(), &format).unwrap();
        recording.secure_audio_file().unwrap();
        let (local, incoming) = crate::local_audio::channel();
        for _ in 0..128 {
            local
                .send(crate::local_audio::Packet {
                    rate: 48000.,
                    channels: 1,
                    samples: vec![0.; 1],
                })
                .unwrap();
        }
        let (sender, events) = std::sync::mpsc::channel();
        let sink = Sink {
            state: Mutex::new(SinkState {
                format,
                maximum_frames: 48000,
                frames: 0,
                accepting: true,
                valid: true,
                diagnostics: CaptureObservation::new(StageTimer::start(Instant::now)),
                writer: Some(writer),
                request: None,
                local: Some(local),
                local_error: None,
            }),
            events: Arc::new(Events {
                generation: 123,
                sender,
                active: AtomicBool::new(true),
            }),
            speech_active: Arc::new(AtomicBool::new(false)),
        };
        sink.consume(&buffer);
        assert!(
            matches!(events.try_recv(), Ok((123, AudioEvent::Failed(text))) if text.contains("队列"))
        );
        assert!(incoming.check().is_err());
        assert!(sink.state.lock().unwrap().valid);
        let (result, diagnostics) = sink.finish();
        result.unwrap();
        assert_eq!(diagnostics.signal, InputSignal::Signal);
        recording.validate_finalized().unwrap();
        validate_aac(recording.path()).unwrap();
        let mut frames = 0;
        read_pcm(recording.path(), |packet| {
            frames += packet.samples.len();
            Ok(())
        })
        .unwrap();
        assert!(frames > 0);
    }
    #[test]
    fn damaged_writer_emits_generation_tagged_failure_and_cannot_finalize() {
        let temp = tempfile::tempdir().unwrap();
        let recording =
            storage::Storage::at(temp.path().canonicalize().unwrap().join(storage::NAMESPACE))
                .create_recording()
                .unwrap();
        let (format, input) = synthetic_buffer(48_000.0, 1, 4800);
        let mut writer = AacWriter::new(recording.path(), &format).unwrap();
        recording.secure_audio_file().unwrap();
        writer.file = None; // Inject an unavailable native writer, no device required.
        let (sender, receiver) = std::sync::mpsc::channel();
        let sink = Sink {
            state: Mutex::new(SinkState {
                format,
                maximum_frames: 48_000,
                frames: 0,
                accepting: true,
                valid: true,
                diagnostics: CaptureObservation::new(StageTimer::start(Instant::now)),
                writer: Some(writer),
                request: None,
                local: None,
                local_error: None,
            }),
            events: Arc::new(Events {
                generation: 99,
                sender,
                active: AtomicBool::new(true),
            }),
            speech_active: Arc::new(AtomicBool::new(false)),
        };
        sink.consume(&input);
        assert!(matches!(
            receiver.try_recv(),
            Ok((99, AudioEvent::Failed(_)))
        ));
        let (result, diagnostics) = sink.finish();
        assert!(result.is_err());
        assert_eq!(diagnostics.signal, InputSignal::Unknown);
        assert!(!sink.state.lock().unwrap().valid);
        sink.consume(&input);
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn encoder_sample_limit_remains_at_most_180_seconds() {
        let temp = tempfile::tempdir().unwrap();
        let recording =
            storage::Storage::at(temp.path().canonicalize().unwrap().join(storage::NAMESPACE))
                .create_recording()
                .unwrap();
        let (format, input) = synthetic_buffer(24_000.0, 1, 24_000);
        let mut writer = AacWriter::new(recording.path(), &format).unwrap();
        recording.secure_audio_file().unwrap();
        // Supply extra samples to exercise the encoder's independent output cap.
        for _ in 0..182 {
            writer.write(&input).unwrap();
        }
        writer.finish().unwrap();
        assert_eq!(writer.frames, MAXIMUM_OUTPUT_FRAMES);
        recording.validate_finalized().unwrap();
        validate_aac(recording.path()).unwrap();
        let file = unsafe {
            AVAudioFile::initForReading_error(
                AVAudioFile::alloc(),
                &file_url(recording.path()).unwrap(),
            )
            .unwrap()
        };
        assert!(unsafe { file.length() } as u64 <= MAXIMUM_OUTPUT_FRAMES);
    }

    #[test]
    fn unlabeled_microphone_channels_survive_aac_encoding() {
        use objc2_avf_audio::AVAudioChannelLayout;

        // CoreAudio's layout header followed by three channel descriptions.
        // The built-in input reports three unknown labels and zero coordinates.
        let mut descriptions = [0_u32; 3 + 3 * 5];
        descriptions[2] = 3;
        for description in descriptions[3..].chunks_exact_mut(5) {
            description[0] = u32::MAX; // kAudioChannelLabel_Unknown
        }
        let layouts = unsafe {
            [
                AVAudioChannelLayout::initWithLayout(
                    AVAudioChannelLayout::alloc(),
                    NonNull::new(descriptions.as_mut_ptr().cast()).unwrap(),
                ),
                AVAudioChannelLayout::initWithLayoutTag(
                    AVAudioChannelLayout::alloc(),
                    (147 << 16) | 3, // kAudioChannelLayoutTag_DiscreteInOrder
                )
                .unwrap(),
            ]
        };
        for layout in layouts {
            for (common_format, interleaved) in [
                (AVAudioCommonFormat::PCMFormatFloat32, false),
                (AVAudioCommonFormat::PCMFormatInt16, true),
            ] {
                let format = unsafe {
                    AVAudioFormat::initWithCommonFormat_sampleRate_interleaved_channelLayout(
                        AVAudioFormat::alloc(),
                        common_format,
                        48_000.,
                        interleaved,
                        &layout,
                    )
                };
                for active_channel in [None, Some(0), Some(1), Some(2)] {
                    let temp = tempfile::tempdir().unwrap();
                    let path = temp.path().join("synthetic.m4a");
                    let mut writer = AacWriter::new(&path, &format).unwrap();
                    let input = unsafe {
                        AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                            AVAudioPCMBuffer::alloc(),
                            &format,
                            1024,
                        )
                    }
                    .unwrap();
                    for offset in (0..24_000u32).step_by(1024) {
                        let frames = (24_000 - offset).min(1024);
                        unsafe {
                            input.setFrameLength(frames);
                            for frame in 0..frames as usize {
                                let tone = (f64::from(offset + frame as u32)
                                    * std::f64::consts::TAU
                                    * 440.
                                    / 48_000.)
                                    .sin() as f32
                                    * 0.15;
                                for channel in 0..3 {
                                    let value = if active_channel == Some(channel) {
                                        tone
                                    } else {
                                        0.
                                    };
                                    if interleaved {
                                        (*input.int16ChannelData())
                                            .as_ptr()
                                            .add(frame * 3 + channel)
                                            .write((value * 32768.) as i16);
                                    } else {
                                        (*input.floatChannelData().add(channel))
                                            .as_ptr()
                                            .add(frame)
                                            .write(value);
                                    }
                                }
                            }
                        }
                        writer.write(&input).unwrap();
                    }
                    writer.finish().unwrap();
                    validate_aac(&path).unwrap();
                    let mut decoded = Vec::new();
                    read_pcm(&path, |packet| {
                        assert_eq!((packet.rate, packet.channels), (24_000., 1));
                        decoded.extend(packet.samples);
                        Ok(())
                    })
                    .unwrap();
                    assert!((11_900..=12_100).contains(&decoded.len()));
                    let peak = decoded.into_iter().map(f32::abs).fold(0., f32::max);
                    let tag = unsafe { layout.layoutTag() };
                    if active_channel.is_some() {
                        assert!(
                            peak > 0.03,
                            "layout {tag}, {active_channel:?}, interleaved={interleaved}, peak={peak}"
                        );
                    } else {
                        assert!(peak < 0.00001, "silence must stay silent, peak={peak}");
                    }
                }
            }
        }
    }

    #[test]
    fn native_pcm_converts_to_finalized_aac_without_hardware() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join(storage::NAMESPACE);
        let recording = storage::Storage::at(root).create_recording().unwrap();
        let format = unsafe {
            AVAudioFormat::initStandardFormatWithSampleRate_channels(
                AVAudioFormat::alloc(),
                48_000.0,
                2,
            )
            .unwrap()
        };
        let input = unsafe {
            AVAudioPCMBuffer::initWithPCMFormat_frameCapacity(
                AVAudioPCMBuffer::alloc(),
                &format,
                4800,
            )
            .unwrap()
        };
        unsafe {
            input.setFrameLength(4800);
            let channels = input.floatChannelData();
            for channel in 0..2 {
                let samples =
                    std::slice::from_raw_parts_mut((*channels.add(channel)).as_ptr(), 4800);
                for (frame, sample) in samples.iter_mut().enumerate() {
                    *sample =
                        ((frame as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin()) * 0.1;
                }
            }
        }
        let mut writer = AacWriter::new(recording.path(), &format).unwrap();
        recording.secure_audio_file().unwrap();
        for _ in 0..10 {
            writer.write(&input).unwrap();
        }
        writer.finish().unwrap();
        recording.validate_finalized().unwrap();
        validate_aac(recording.path()).unwrap();
        let file = unsafe {
            AVAudioFile::initForReading_error(
                AVAudioFile::alloc(),
                &file_url(recording.path()).unwrap(),
            )
            .unwrap()
        };
        let length = unsafe { file.length() };
        assert!(
            (23_000..=25_000).contains(&length),
            "unexpected AAC length: {length}"
        );
    }
}
