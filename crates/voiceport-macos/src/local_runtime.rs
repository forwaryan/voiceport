//! Worker-owned native recognizer. No auth, proxy, network, Node or shell calls.
use crate::{
    local_audio::{Incoming, MAX_SAMPLES, Packet, RATE, Resampler, Transcript, quiet_split},
    local_models::{Model, ModelKind},
};
use std::{
    ffi::{CStr, CString, c_char, c_void},
    path::Path,
    ptr::NonNull,
    sync::mpsc::RecvTimeoutError,
    time::Duration,
};
use voiceport_core::CancellationToken;

#[path = "local_runtime/app_engine.rs"]
mod app_engine;
#[cfg(all(test, debug_assertions, target_arch = "aarch64"))]
pub(crate) use app_engine::check_app_engine;
#[cfg(all(debug_assertions, target_arch = "aarch64"))]
pub(crate) use app_engine::fixture_catalog;
pub(crate) use app_engine::stage_app_engine;

fn runtime_staging_directory() -> Result<tempfile::TempDir, String> {
    use std::{fs::Permissions, os::unix::fs::PermissionsExt};
    tempfile::Builder::new()
        .prefix("voiceport-runtime-")
        .permissions(Permissions::from_mode(0o700))
        .tempdir_in("/private/tmp")
        .map_err(|error| error.to_string())
}

#[cfg(target_arch = "aarch64")]
#[path = "local_runtime/verified.rs"]
mod verified;
#[cfg(target_arch = "aarch64")]
pub(crate) use verified::stage_verified_runtime;

#[cfg(debug_assertions)]
#[path = "local_runtime/performance.rs"]
mod performance;
#[cfg(debug_assertions)]
pub(crate) use performance::performance_file;

unsafe extern "C" {
    fn vp_local_open(
        directory: *const c_char,
        encoder: *const c_char,
        decoder: *const c_char,
        joiner: *const c_char,
        tokens: *const c_char,
        language: *const c_char,
        version: *const c_char,
        mode: i32,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn vp_local_close(runtime: *mut c_void);
    fn vp_local_error(runtime: *mut c_void) -> *const c_char;
    fn vp_local_feed(runtime: *mut c_void, samples: *const f32, count: i32) -> i32;
    fn vp_local_ready(runtime: *mut c_void) -> i32;
    fn vp_local_decode(runtime: *mut c_void) -> i32;
    fn vp_local_endpoint(runtime: *mut c_void) -> i32;
    fn vp_local_reset(runtime: *mut c_void) -> i32;
    fn vp_local_finish(runtime: *mut c_void) -> i32;
    fn vp_local_result(runtime: *mut c_void) -> *mut c_char;
    fn vp_local_offline(runtime: *mut c_void, samples: *const f32, count: i32) -> *mut c_char;
    fn vp_local_free(text: *mut c_char);
}
struct Native(NonNull<c_void>);
impl Native {
    fn open(model: &Model, language: &str) -> Result<Self, String> {
        model.validate()?;
        if !model.kind.supports_language(language) {
            return Err("本地转写语言不受支持。".into());
        }
        let string = |path: &Path| {
            CString::new(path.as_os_str().as_encoded_bytes())
                .map_err(|_| "本地模型路径包含无效字符。".to_string())
        };
        let directory = string(&model.runtime)?;
        let encoder = string(&model.encoder)?;
        let decoder = string(&model.decoder)?;
        let joiner = model.joiner.as_deref().map(string).transpose()?;
        let tokens = string(&model.tokens)?;
        let language = CString::new(if model.kind == ModelKind::Nemotron && language == "no" {
            "nb-NO"
        } else {
            language
        })
        .unwrap();
        let version = CString::new(crate::local_models::runtime_version(&model.runtime)?).unwrap();
        let mut error = [0 as c_char; 1024];
        // SAFETY: The exact versioned C header defines all native configuration layouts.
        // Inputs remain alive until creation returns; ownership of handles stays here.
        let pointer = unsafe {
            vp_local_open(
                directory.as_ptr(),
                encoder.as_ptr(),
                decoder.as_ptr(),
                joiner.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
                tokens.as_ptr(),
                language.as_ptr(),
                version.as_ptr(),
                model.kind.native_mode(),
                error.as_mut_ptr(),
                error.len(),
            )
        };
        NonNull::new(pointer).map(Self).ok_or_else(|| {
            format!(
                "无法启动本地识别：{}",
                unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy()
            )
        })
    }
    fn json(pointer: *mut c_char) -> Result<String, String> {
        if pointer.is_null() {
            return Err("本地识别未返回有效结果。".into());
        }
        // SAFETY: The bridge returns a strdup allocation and exposes its matching free.
        let result = unsafe { CStr::from_ptr(pointer) }.to_bytes();
        let result = serde_json::from_slice::<serde_json::Value>(result)
            .map_err(|_| "本地识别结果格式无效。".to_string())
            .and_then(|value| {
                value["text"]
                    .as_str()
                    .map(|s| s.trim().to_owned())
                    .ok_or_else(|| "本地识别结果缺少文字。".into())
            });
        unsafe {
            vp_local_free(pointer);
        }
        result
    }
    fn status(&self, status: i32) -> Result<i32, String> {
        if status < 0 {
            let error =
                unsafe { CStr::from_ptr(vp_local_error(self.0.as_ptr())) }.to_string_lossy();
            Err(format!("本地模型运行失败：{error}"))
        } else {
            Ok(status)
        }
    }
    fn feed(&self, samples: &[f32]) -> Result<(), String> {
        self.status(unsafe {
            vp_local_feed(self.0.as_ptr(), samples.as_ptr(), samples.len() as i32)
        })?;
        Ok(())
    }
    fn decode_ready(&self, cancel: &CancellationToken) -> Result<(), String> {
        while self.status(unsafe { vp_local_ready(self.0.as_ptr()) })? != 0 {
            check_cancel(cancel)?;
            self.status(unsafe { vp_local_decode(self.0.as_ptr()) })?;
        }
        Ok(())
    }
    fn result(&self) -> Result<String, String> {
        let pointer = unsafe { vp_local_result(self.0.as_ptr()) };
        if pointer.is_null() {
            self.status(-1)?;
        }
        Self::json(pointer)
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            vp_local_close(self.0.as_ptr());
        }
    }
}
fn check_cancel(cancel: &CancellationToken) -> Result<(), String> {
    if cancel.is_cancelled() {
        Err("本地转写已取消。".into())
    } else {
        Ok(())
    }
}
#[derive(Debug)]
pub(crate) enum FileError {
    NoSpeech,
    NoText,
    Failed(String),
}

impl From<String> for FileError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

impl FileError {
    fn into_message(self) -> String {
        match self {
            Self::NoSpeech => "未检测到可转写的声音，请重新录音。".into(),
            Self::NoText => "本地模型未识别出文字，可重试或重新录音。".into(),
            Self::Failed(error) => error,
        }
    }
}

struct Session {
    native: Native,
    kind: ModelKind,
    resampler: Resampler,
    transcript: Transcript,
    offline: Vec<f32>,
    heard_audio: bool,
}
impl Session {
    fn new(model: &Model, language: &str, cancel: &CancellationToken) -> Result<Self, String> {
        check_cancel(cancel)?;
        let native = Native::open(model, language)?;
        check_cancel(cancel)?;
        Ok(Self {
            native,
            kind: model.kind,
            resampler: Resampler::default(),
            transcript: Transcript::default(),
            offline: vec![],
            heard_audio: false,
        })
    }
    fn push(
        &mut self,
        packet: Packet,
        cancel: &CancellationToken,
        partial: &mut impl FnMut(String),
    ) -> Result<(), String> {
        check_cancel(cancel)?;
        let samples = self.resampler.push(packet)?;
        self.accept(&samples, cancel, partial)
    }
    fn accept(
        &mut self,
        samples: &[f32],
        cancel: &CancellationToken,
        partial: &mut impl FnMut(String),
    ) -> Result<(), String> {
        check_cancel(cancel)?;
        self.heard_audio |= samples.iter().any(|s| s.abs() > 0.0001);
        match self.kind {
            ModelKind::Paraformer | ModelKind::ParakeetUnified | ModelKind::Nemotron => {
                if !samples.is_empty() {
                    self.native.feed(samples)?;
                    self.native.decode_ready(cancel)?;
                    let text = self.native.result()?;
                    if self
                        .native
                        .status(unsafe { vp_local_endpoint(self.native.0.as_ptr()) })?
                        != 0
                    {
                        self.transcript.commit(&text);
                        self.native
                            .status(unsafe { vp_local_reset(self.native.0.as_ptr()) })?;
                        partial(self.transcript.current(""));
                    } else {
                        partial(self.transcript.current(&text));
                    }
                }
            }
            ModelKind::Whisper
            | ModelKind::WhisperTurbo
            | ModelKind::SenseVoice
            | ModelKind::ParakeetTdtV3
            | ModelKind::ParakeetTdtV2 => {
                if self.offline.len() + samples.len() > MAX_SAMPLES {
                    return Err("本地音频超过 180 秒。".into());
                }
                self.offline.extend_from_slice(samples);
            }
        }
        Ok(())
    }
    fn finish(
        mut self,
        cancel: &CancellationToken,
        partial: &mut impl FnMut(String),
    ) -> Result<String, FileError> {
        let tail = self.resampler.finish()?;
        self.accept(&tail, cancel, partial)?;
        if !self.heard_audio {
            return Err(FileError::NoSpeech);
        }
        match self.kind {
            ModelKind::Paraformer | ModelKind::ParakeetUnified | ModelKind::Nemotron => {
                let padding = vec![0.0; RATE * 8 / 10];
                // Flush the final model context before InputFinished. Padding is never
                // endpoint-committed separately, so the last segment is appended once.
                self.native.feed(&padding)?;
                self.native
                    .status(unsafe { vp_local_finish(self.native.0.as_ptr()) })?;
                self.native.decode_ready(cancel)?;
                self.transcript.commit(&self.native.result()?);
            }
            ModelKind::Whisper
            | ModelKind::WhisperTurbo
            | ModelKind::SenseVoice
            | ModelKind::ParakeetTdtV3
            | ModelKind::ParakeetTdtV2 => {
                let mut offset = 0;
                while offset < self.offline.len() {
                    check_cancel(cancel)?;
                    let count = quiet_split(&self.offline[offset..]);
                    let samples = &self.offline[offset..offset + count];
                    if samples.iter().any(|s| s.abs() > 0.0001) {
                        let text = Native::json(unsafe {
                            vp_local_offline(
                                self.native.0.as_ptr(),
                                samples.as_ptr(),
                                samples.len() as i32,
                            )
                        })?;
                        self.transcript.commit(&text);
                    }
                    offset += count;
                }
            }
        }
        check_cancel(cancel)?;
        let result = self.transcript.current("");
        if result.trim().is_empty() {
            Err(FileError::NoText)
        } else {
            Ok(result)
        }
    }
}

pub fn transcribe_live(
    model: &Model,
    language: &str,
    incoming: Incoming,
    cancel: &CancellationToken,
    mut partial: impl FnMut(String),
) -> Result<String, String> {
    let mut session = Session::new(model, language, cancel)?;
    loop {
        check_cancel(cancel)?;
        incoming.check()?;
        match incoming.receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(packet) => session.push(packet, cancel, &mut partial)?,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                incoming.completed()?;
                break;
            }
        }
    }
    session
        .finish(cancel, &mut partial)
        .map_err(FileError::into_message)
}
pub fn transcribe_file(
    model: &Model,
    language: &str,
    path: &Path,
    cancel: &CancellationToken,
    partial: impl FnMut(String),
) -> Result<String, String> {
    transcribe_file_classified(model, language, path, cancel, partial)
        .map_err(FileError::into_message)
}

pub(crate) fn transcribe_file_classified(
    model: &Model,
    language: &str,
    path: &Path,
    cancel: &CancellationToken,
    mut partial: impl FnMut(String),
) -> Result<String, FileError> {
    let mut session = Session::new(model, language, cancel)?;
    crate::audio::read_pcm(path, |packet| session.push(packet, cancel, &mut partial))?;
    session.finish(cancel, &mut partial)
}

/// Test actual runtime initialization and one decode using generated silence.
/// No model output is retained and no microphone or recording file is opened.
pub(crate) fn probe(
    model: &Model,
    language: &str,
    cancel: &CancellationToken,
) -> Result<(), String> {
    check_cancel(cancel)?;
    let native = Native::open(model, language)?;
    check_cancel(cancel)?;
    let silence = vec![0.0; RATE];
    match model.kind {
        ModelKind::Paraformer | ModelKind::ParakeetUnified | ModelKind::Nemotron => {
            native.feed(&silence)?;
            native.status(unsafe { vp_local_finish(native.0.as_ptr()) })?;
            native.decode_ready(cancel)?;
            native.result()?;
        }
        ModelKind::Whisper
        | ModelKind::WhisperTurbo
        | ModelKind::SenseVoice
        | ModelKind::ParakeetTdtV3
        | ModelKind::ParakeetTdtV2 => {
            Native::json(unsafe {
                vp_local_offline(native.0.as_ptr(), silence.as_ptr(), silence.len() as i32)
            })?;
        }
    }
    check_cancel(cancel)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_bridge_rejects_unknown_modes_before_loading_libraries() {
        let mut error = [0 as c_char; 256];
        let pointer = unsafe {
            vp_local_open(
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                99,
                error.as_mut_ptr(),
                error.len(),
            )
        };
        assert!(pointer.is_null());
        assert!(
            unsafe { CStr::from_ptr(error.as_ptr()) }
                .to_str()
                .unwrap()
                .contains("Unsupported local model mode: 99")
        );
    }
    #[test]
    fn cancelled_probe_does_not_open_even_missing_model_files() {
        let model = Model {
            kind: ModelKind::Whisper,
            directory: "/nonexistent-synthetic-model".into(),
            runtime: "/nonexistent-synthetic-runtime".into(),
            encoder: "/nonexistent-synthetic-encoder".into(),
            decoder: "/nonexistent-synthetic-decoder".into(),
            joiner: None,
            tokens: "/nonexistent-synthetic-tokens".into(),
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(probe(&model, "", &cancel), Err("本地转写已取消。".into()));
    }
    #[test]
    #[ignore = "explicit installed-model synthetic silence test; no microphone, auth or network"]
    fn installed_models_decode_synthetic_probe() {
        let own_root = std::env::var_os("VOICEPORT_TEST_MODELS_ROOT").map(std::path::PathBuf::from);
        let root = own_root.clone().unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var_os("HOME").unwrap())
                .join("Library/Application Support/orca/speech-models")
        });
        assert!(root.is_absolute());
        let runtime = own_root
            .map(|root| root.join(format!("runtime-{}", crate::local_models::RUNTIME_VERSION)))
            .unwrap_or_else(|| crate::local_models::RUNTIME_DIRECTORY.into());
        let ids = std::env::var("VOICEPORT_TEST_MODEL")
            .map(|id| vec![id])
            .unwrap_or_else(|_| {
                vec![
                    crate::local_models::PARAFORMER.into(),
                    crate::local_models::WHISPER.into(),
                ]
            });
        for id in ids {
            let catalog = crate::local_models::Catalog::discover(
                &root,
                &runtime,
                &id,
                std::env::consts::ARCH,
            );
            let crate::local_models::SessionSource::Local(model) = catalog.snapshot(&id).unwrap()
            else {
                panic!("expected installed model");
            };
            for language in ["", "zh", "en"]
                .into_iter()
                .filter(|language| model.kind.supports_language(language))
            {
                let start = std::time::Instant::now();
                probe(&model, language, &CancellationToken::new()).unwrap();
                println!(
                    "synthetic silence {id} language={language:?}: {} ms",
                    start.elapsed().as_millis()
                );
            }
        }
    }
    #[test]
    #[ignore = "explicit new-model ASR using hash-pinned public WAV and dedicated model root; no microphone or network"]
    fn expanded_model_transcribes_pinned_public_fixture() {
        use std::io::Read;
        let root = std::path::PathBuf::from(
            std::env::var_os("VOICEPORT_TEST_MODELS_ROOT").expect("dedicated model root"),
        );
        let fixture = std::path::PathBuf::from(
            std::env::var_os("VOICEPORT_TEST_PUBLIC_WAV").expect("pinned public WAV path"),
        );
        assert!(root.is_absolute() && fixture.is_absolute());
        let id = std::env::var("VOICEPORT_TEST_MODEL").expect("explicit expanded model ID");
        assert!(matches!(
            id.as_str(),
            crate::local_models::PARAFORMER
                | crate::local_models::WHISPER
                | crate::local_models::SENSEVOICE
                | crate::local_models::WHISPER_TURBO
                | crate::local_models::PARAKEET_TDT_V3
                | crate::local_models::PARAKEET_TDT_V2
                | crate::local_models::PARAKEET_UNIFIED
                | crate::local_models::NEMOTRON
        ));
        let mut bytes = vec![];
        std::fs::File::open(&fixture)
            .unwrap()
            .take(1_000_001)
            .read_to_end(&mut bytes)
            .unwrap();
        assert_eq!(bytes.len(), 212044);
        // Official whisper-tiny revision 65176e2deb88badc814a94058666cadccc29b61c,
        // test_wavs/0.wav. Existing production fixture tests independently expect "yellow".
        let digest: String = ring::digest::digest(&ring::digest::SHA256, &bytes)
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(
            digest,
            "6bc58a4efdf20daac252b6b1502632601a71efe0308f6757dc1eda34891a7e4f"
        );
        assert_eq!(&bytes[..12], b"RIFF\x44\x3c\x03\x00WAVE");
        let runtime = root.join(format!("runtime-{}", crate::local_models::RUNTIME_VERSION));
        let catalog =
            crate::local_models::Catalog::discover(&root, &runtime, &id, std::env::consts::ARCH);
        let crate::local_models::SessionSource::Local(model) = catalog.snapshot(&id).unwrap()
        else {
            panic!("local model")
        };
        let mut partials = vec![];
        let started = std::time::Instant::now();
        let text = transcribe_file(&model, "en", &fixture, &CancellationToken::new(), |text| {
            partials.push(text)
        })
        .unwrap();
        assert!(!text.trim().is_empty());
        assert!(
            text.to_lowercase().contains("yellow"),
            "unexpected public fixture ASR: {text}"
        );
        if model.kind.streaming() {
            assert!(
                partials.iter().any(|text| !text.trim().is_empty()),
                "streaming preview missing"
            );
        } else {
            assert!(
                partials.is_empty(),
                "offline models must not claim streaming previews"
            );
        }
        let (input, incoming) = crate::local_audio::channel();
        crate::audio::read_pcm(&fixture, |packet| input.send(packet)).unwrap();
        input.finish();
        let live =
            transcribe_live(&model, "en", incoming, &CancellationToken::new(), |_| {}).unwrap();
        assert_eq!(live, text, "queued capture and file retry route must agree");
        println!(
            "PUBLIC FIXTURE ASR {id}: {text:?}; file+queued-capture {:?}",
            started.elapsed()
        );
    }
    #[test]
    #[ignore = "explicit public model fixture inference, no microphone or auth"]
    fn installed_models_transcribe_public_fixture_through_production_path() {
        let catalog = crate::local_models::Catalog::system(crate::local_models::CHATGPT);
        let home = std::env::var_os("HOME").unwrap();
        let fixture = std::path::PathBuf::from(home)
            .join("Library/Application Support/orca/speech-models/whisper-tiny/test_wavs/0.wav");
        for id in [
            crate::local_models::PARAFORMER,
            crate::local_models::WHISPER,
        ] {
            let crate::local_models::SessionSource::Local(model) = catalog.snapshot(id).unwrap()
            else {
                panic!("missing installed model")
            };
            let mut partials = vec![];
            let text = transcribe_file(&model, "en", &fixture, &CancellationToken::new(), |text| {
                partials.push(text)
            })
            .unwrap();
            println!(
                "DEVELOPMENT SELF-CHECK {id}: final={text:?}; partial_count={}",
                partials.len()
            );
            assert!(
                text.to_lowercase().contains("yellow"),
                "unexpected fixture transcription: {text}"
            );
            if id == crate::local_models::PARAFORMER {
                assert!(partials.iter().any(|p| !p.trim().is_empty()));
            } else {
                assert!(partials.is_empty());
            }
            let (input, incoming) = crate::local_audio::channel();
            // Stopping before model creation must still drain the complete queued tail.
            crate::audio::read_pcm(&fixture, |packet| input.send(packet)).unwrap();
            input.finish();
            let live =
                transcribe_live(&model, "en", incoming, &CancellationToken::new(), |_| {}).unwrap();
            assert_eq!(live, text);
            if model.kind == ModelKind::Paraformer {
                let (input, incoming) = crate::local_audio::channel();
                crate::audio::read_pcm(&fixture, |packet| input.send(packet)).unwrap();
                for _ in 0..6 {
                    input
                        .send(Packet {
                            rate: 16000.,
                            channels: 1,
                            samples: vec![0.; 8000],
                        })
                        .unwrap();
                }
                crate::audio::read_pcm(&fixture, |packet| input.send(packet)).unwrap();
                input.finish();
                let mut accumulated = vec![];
                let repeated =
                    transcribe_live(&model, "en", incoming, &CancellationToken::new(), |text| {
                        accumulated.push(text)
                    })
                    .unwrap();
                assert_eq!(
                    repeated.to_lowercase().matches("yellow").count(),
                    2,
                    "endpoint segments duplicated or lost: {repeated}"
                );
                assert!(
                    accumulated
                        .iter()
                        .any(|text| text.to_lowercase().matches("yellow").count() == 2)
                );
                println!("DEVELOPMENT SELF-CHECK endpoint accumulation: {repeated:?}");
            }
            let fixture8k = fixture.with_file_name("8k.wav");
            let result8k =
                transcribe_file(&model, "en", &fixture8k, &CancellationToken::new(), |_| {})
                    .unwrap();
            println!(
                "DEVELOPMENT SELF-CHECK {id}: 8k_final={result8k:?}; live stop-flush equals file result"
            );
            assert!(result8k.to_lowercase().contains("thought"));
            let (input, incoming) = crate::local_audio::channel();
            input
                .send(Packet {
                    rate: 16000.,
                    channels: 1,
                    samples: vec![0.; 1600],
                })
                .unwrap();
            input.finish();
            assert!(
                transcribe_live(&model, "", incoming, &CancellationToken::new(), |_| {})
                    .unwrap_err()
                    .contains("声音")
            );
        }
    }
}
