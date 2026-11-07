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
    AVAudioConverterOutputStatus, AVAudioEngine, AVAudioFile, AVAudioFormat, AVAudioPCMBuffer,
    AVAudioTime, AVEncoderAudioQualityKey, AVEncoderBitRateKey, AVFormatIDKey,
    AVNumberOfChannelsKey, AVSampleRateKey,
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

pub const MAXIMUM_SECONDS: u64 = 180;
const OUTPUT_RATE: f64 = 24_000.0;
const MAXIMUM_OUTPUT_FRAMES: u64 = 24_000 * MAXIMUM_SECONDS;
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
    engine: Option<Retained<AVAudioEngine>>,
    sink: Option<Arc<Sink>>,
    speech: Option<Speech>,
    recording: Option<Arc<Recording>>,
    events: Option<Arc<Events>>,
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
            engine: None,
            sink: None,
            speech: None,
            recording: None,
            events: None,
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
        let reply = Arc::new(PermissionReply {
            generation,
            sender,
            sent: AtomicBool::new(false),
        });
        let Some(media_type) = (unsafe { AVMediaTypeAudio }) else {
            reply.send(false, false);
            return;
        };
        let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media_type) };
        if status == AVAuthorizationStatus::Authorized {
            request_speech_permission(reply);
        } else if status == AVAuthorizationStatus::NotDetermined {
            let callback = RcBlock::new(move |allowed: Bool| {
                if allowed.as_bool() {
                    request_speech_permission(reply.clone())
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
    ) -> Result<bool, String> {
        main_thread()?;
        self.cancel();
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
            let recording = Arc::new(
                storage::default_storage()
                    .and_then(|s| s.create_recording())
                    .map_err(|e| e.to_string())?,
            );
            self.recording = Some(recording.clone());
            let engine = unsafe { AVAudioEngine::new() };
            let input = unsafe { engine.inputNode() };
            let format = unsafe { input.outputFormatForBus(0) };
            validate_format(&format)?;
            let writer = AacWriter::new(recording.path(), &format)?;
            recording.secure_audio_file().map_err(|e| e.to_string())?;
            let speech = if speech_authorized {
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
                    writer: Some(writer),
                    request,
                }),
                events: events.clone(),
                speech_active: speech_active.clone(),
            });
            self.sink = Some(sink.clone());
            self.speech = speech;
            if let Some(speech) = self.speech.as_mut() {
                speech.start(events.clone(), speech_active);
            }
            let tap = RcBlock::new(
                move |buffer: NonNull<AVAudioPCMBuffer>, _time: NonNull<AVAudioTime>| {
                    // The framework owns buffer for this callback. consume is synchronous;
                    // neither this reference nor PCM pointers escape the callback.
                    // Bound temporary Objective-C objects even on an audio thread
                    // without a surrounding application autorelease pool.
                    autoreleasepool(|_| sink.consume(unsafe { buffer.as_ref() }));
                },
            );
            self.engine = Some(engine);
            unsafe {
                input.installTapOnBus_bufferSize_format_block(
                    0,
                    1024,
                    Some(&format),
                    RcBlock::as_ptr(&tap),
                );
                let engine = self.engine.as_ref().expect("engine stored before tap");
                engine.prepare();
                engine.startAndReturnError().map_err(native_error)?;
            }
            Ok(live)
        })();
        if result.is_err() {
            self.cancel();
        }
        result
    }

    /// Revoke callback delivery, stop/remove the tap, drain conversion, close the
    /// AAC writer, and validate the finalized file before publishing its Arc.
    /// A failed writer can never produce an uploadable recording.
    pub fn stop(&mut self) -> Result<Arc<Recording>, String> {
        main_thread()?;
        self.stop_native();
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
        if let Some(engine) = self.engine.take() {
            unsafe {
                engine.stop();
                engine.inputNode().removeTapOnBus(0);
            }
        }
        if let Some(sink) = self.sink.take()
            && let Err(error) = sink.finish()
        {
            self.failure = Some(error);
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
fn request_speech_permission(reply: Arc<PermissionReply>) {
    let status = unsafe { SFSpeechRecognizer::authorizationStatus() };
    if status == SFSpeechRecognizerAuthorizationStatus::NotDetermined {
        let callback = RcBlock::new(move |status| {
            reply.send(
                true,
                status == SFSpeechRecognizerAuthorizationStatus::Authorized,
            )
        });
        unsafe { SFSpeechRecognizer::requestAuthorization(&callback) };
    } else {
        reply.send(
            true,
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
                    events.send(AudioEvent::Status(format!(
                        "本机语音识别失败（错误码 {}），将继续录音并在结束后转写。",
                        error.code()
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
    writer: Option<AacWriter>,
    request: Option<Retained<SFSpeechAudioBufferRecognitionRequest>>,
}
// SAFETY: The immutable format and the writer/request are created before tap
// publication. Every subsequent access, including append, endAudio, conversion,
// close and release, is serialized by Sink::state. PCM buffers are borrowed only
// during consume. No engine, recognizer or recognition task enters this state.
unsafe impl Send for SinkState {}

impl Sink {
    fn consume(&self, buffer: &AVAudioPCMBuffer) {
        let event = {
            let Ok(mut state) = self.state.lock() else {
                self.events
                    .send(AudioEvent::Failed("录音写入锁失效，请重新录音。".into()));
                return;
            };
            if !state.accepting {
                return;
            }
            match state.consume(buffer, self.speech_active.load(Ordering::Acquire)) {
                Ok(true) => Some(AudioEvent::LimitReached),
                Ok(false) => None,
                Err(error) => {
                    state.valid = false;
                    state.accepting = false;
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
    fn finish(&self) -> Result<(), String> {
        self.speech_active.store(false, Ordering::Release);
        let mut state = self
            .state
            .lock()
            .map_err(|_| "录音写入锁失效。".to_owned())?;
        state.accepting = false;
        state.end_speech();
        let result = state
            .writer
            .take()
            .map_or(Ok(()), |mut writer| writer.finish());
        if result.is_err() {
            state.valid = false;
        }
        if !state.valid || state.frames == 0 {
            return Err("无法写入完整录音，请重新录音。".into());
        }
        result
    }
}
impl SinkState {
    fn consume(&mut self, buffer: &AVAudioPCMBuffer, speech_active: bool) -> Result<bool, String> {
        unsafe {
            if buffer.format() != self.format
                || buffer.frameLength() > 1_048_576
                || buffer.frameLength() > buffer.frameCapacity()
            {
                return Err("麦克风音频格式不可用。".into());
            }
            let count =
                (buffer.frameLength() as u64).min(self.maximum_frames.saturating_sub(self.frames));
            if count == 0 {
                return Ok(false);
            }
            let copy;
            let input = if count < buffer.frameLength() as u64 {
                copy = prefix(buffer, count as u32)?;
                &copy
            } else {
                buffer
            };
            self.writer
                .as_mut()
                .ok_or("录音写入器已关闭。")?
                .write(input)?;
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

fn validate_format(format: &AVAudioFormat) -> Result<(), String> {
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
            return Err("麦克风音频格式不可用。".into());
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

/// Owned exclusively by the sink after construction. Conversion drains before
/// releasing the sole AVAudioFile retain, which closes and finalizes its header.
struct AacWriter {
    file: Option<Retained<AVAudioFile>>,
    converter: Option<Retained<AVAudioConverter>>,
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
            let converter = AVAudioConverter::initFromFormat_toFormat(
                AVAudioConverter::alloc(),
                input_format,
                &output_format,
            )
            .ok_or("无法转换麦克风音频格式。")?;
            converter.setDownmix(true);
            Ok(Self {
                file: Some(file),
                converter: Some(converter),
                output_format,
                frames: 0,
            })
        }
    }
    fn write(&mut self, input: &AVAudioPCMBuffer) -> Result<(), String> {
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

#[cfg(test)]
mod tests {
    use super::*;

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

    fn synthetic_buffer(
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
        let sink = Sink {
            state: Mutex::new(SinkState {
                format,
                maximum_frames: 5001,
                frames: 0,
                accepting: true,
                valid: true,
                writer: Some(writer),
                request: None,
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
        sink.finish().unwrap();
        recording.validate_finalized().unwrap();
        validate_aac(recording.path()).unwrap();
        let bytes = std::fs::read(recording.path()).unwrap();
        sink.consume(&input);
        assert_eq!(std::fs::read(recording.path()).unwrap(), bytes);
        assert!(receiver.try_recv().is_err());
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
                writer: Some(writer),
                request: None,
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
        assert!(sink.finish().is_err());
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
