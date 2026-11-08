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

unsafe extern "C" {
    fn vp_local_open(
        directory: *const c_char,
        encoder: *const c_char,
        decoder: *const c_char,
        tokens: *const c_char,
        language: *const c_char,
        online: i32,
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
        if !matches!(language, "" | "zh" | "en") {
            return Err("本地转写语言不受支持。".into());
        }
        let string = |path: &Path| {
            CString::new(path.as_os_str().as_encoded_bytes())
                .map_err(|_| "本地模型路径包含无效字符。".to_string())
        };
        let directory = string(&model.runtime)?;
        let encoder = string(&model.encoder)?;
        let decoder = string(&model.decoder)?;
        let tokens = string(&model.tokens)?;
        let language = CString::new(language).unwrap();
        let mut error = [0 as c_char; 1024];
        // SAFETY: The exact versioned C header defines all native configuration layouts.
        // Inputs remain alive until creation returns; ownership of handles stays here.
        let pointer = unsafe {
            vp_local_open(
                directory.as_ptr(),
                encoder.as_ptr(),
                decoder.as_ptr(),
                tokens.as_ptr(),
                language.as_ptr(),
                i32::from(model.kind == ModelKind::Paraformer),
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
            ModelKind::Paraformer => {
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
            ModelKind::Whisper => {
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
    ) -> Result<String, String> {
        let tail = self.resampler.finish()?;
        self.accept(&tail, cancel, partial)?;
        if !self.heard_audio {
            return Err("未检测到可转写的声音，请重新录音。".into());
        }
        match self.kind {
            ModelKind::Paraformer => {
                let padding = vec![0.0; RATE * 8 / 10];
                // Flush the final model context before InputFinished. Padding is never
                // endpoint-committed separately, so the last segment is appended once.
                self.native.feed(&padding)?;
                self.native
                    .status(unsafe { vp_local_finish(self.native.0.as_ptr()) })?;
                self.native.decode_ready(cancel)?;
                self.transcript.commit(&self.native.result()?);
            }
            ModelKind::Whisper => {
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
            Err("本地模型未识别出文字，可重试或重新录音。".into())
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
    session.finish(cancel, &mut partial)
}
pub fn transcribe_file(
    model: &Model,
    language: &str,
    path: &Path,
    cancel: &CancellationToken,
    mut partial: impl FnMut(String),
) -> Result<String, String> {
    let mut session = Session::new(model, language, cancel)?;
    crate::audio::read_pcm(path, |packet| session.push(packet, cancel, &mut partial))?;
    session.finish(cancel, &mut partial)
}

#[cfg(test)]
mod tests {
    use super::*;
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
