//! Batch adapters run only on a worker. HTTP has no access to ChatGPT auth.
use crate::{
    local_audio::{MAX_SAMPLES, Resampler},
    plugin_credentials::{Keychain, Secrets},
    plugins::{Adapter, Manifest, Source, Store, validate_endpoint},
};
use std::{collections::BTreeMap, path::Path, time::Duration};
use voiceport_core::CancellationToken;
pub const MAX_RESPONSE: usize = 1_000_000;
pub const MAX_WAV: usize = 44 + 180 * 16000 * 2;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub manifest: Manifest,
    pub source: Source,
    pub secrets: Option<Secrets>,
}
impl Snapshot {
    pub fn configured(store: &Store, source: &Source) -> Result<Self, String> {
        let manifest = store.validate_source(source)?;
        Ok(Self {
            manifest,
            source: source.clone(),
            secrets: None,
        })
    }
    pub fn pin(mut self) -> Result<Self, String> {
        self.secrets = Some(Secrets::pin(&self.source.credential_refs, &Keychain)?);
        Ok(self)
    }
    pub fn transcribe(
        &self,
        path: &Path,
        language: &str,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        let wav = convert(path, cancel)?;
        self.execute(wav, language, cancel)
    }
    pub fn execute(
        &self,
        wav: Vec<u8>,
        language: &str,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        check(cancel)?;
        let secrets = self.secrets.as_ref().ok_or("来源凭据尚未固定。")?;
        secrets.check(&Keychain)?;
        if wav.len() > MAX_WAV {
            return Err("音频超过 180 秒限制。".into());
        }
        let mut config = self.source.config.clone();
        config.extend(secrets.values());
        let result = match self.manifest.adapter {
            Adapter::Http => {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| "无法启动 HTTP 转写任务。")?;
                runtime.block_on(http(
                    &config,
                    wav,
                    language,
                    cancel,
                    Duration::from_secs(110),
                    Duration::from_secs(30),
                ))
            }
            Adapter::Jsonl => crate::plugin_process::execute(
                &self.manifest,
                &config,
                &wav,
                language,
                cancel,
                Duration::from_secs(110),
            ),
            _ => Err("此来源的插件不支持通用批量派发。".into()),
        };
        result
            .map(|text| secrets.redact(&text))
            .map_err(|error| secrets.redact(&error))
    }
}
pub fn check(cancel: &CancellationToken) -> Result<(), String> {
    if cancel.is_cancelled() {
        Err("转写已取消。".into())
    } else {
        Ok(())
    }
}
pub fn convert(path: &Path, cancel: &CancellationToken) -> Result<Vec<u8>, String> {
    let mut resampler = Resampler::default();
    let mut samples = vec![];
    crate::audio::read_pcm(path, |packet| {
        check(cancel)?;
        let out = resampler.push(packet)?;
        if samples.len() + out.len() > MAX_SAMPLES {
            return Err("音频超过 180 秒。".into());
        }
        samples.extend(out);
        Ok(())
    })?;
    samples.extend(resampler.finish()?);
    check(cancel)?;
    wav(&samples)
}
pub fn wav(samples: &[f32]) -> Result<Vec<u8>, String> {
    if samples.is_empty() || samples.len() > MAX_SAMPLES || samples.iter().any(|s| !s.is_finite()) {
        return Err("音频为空、超过 180 秒或含无效采样。".into());
    }
    let count = (samples.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(count as usize + 44);
    bytes.extend(b"RIFF");
    bytes.extend((count + 36).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend(16000u32.to_le_bytes());
    bytes.extend(32000u32.to_le_bytes());
    bytes.extend(2u16.to_le_bytes());
    bytes.extend(16u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(count.to_le_bytes());
    for sample in samples {
        bytes.extend(
            ((sample.clamp(-1., 1.) * 32768.)
                .round()
                .clamp(-32768., 32767.) as i16)
                .to_le_bytes(),
        );
    }
    Ok(bytes)
}
pub fn sample() -> Vec<u8> {
    wav(&vec![0.; 16000]).expect("fixed silent sample")
}
pub async fn http(
    config: &BTreeMap<String, String>,
    wav: Vec<u8>,
    language: &str,
    cancel: &CancellationToken,
    timeout: Duration,
    read_timeout: Duration,
) -> Result<String, String> {
    check(cancel)?;
    if wav.len() > MAX_WAV {
        return Err("音频超过大小限制。".into());
    }
    let endpoint = validate_endpoint(config.get("endpoint").ok_or("未配置转写地址。")?)?;
    let model = config
        .get("model")
        .filter(|s| !s.trim().is_empty())
        .ok_or("未配置模型标识。")?;
    let language = match language {
        "" => None,
        "zh" | "en" => Some(language.to_owned()),
        _ => return Err("语言代码无效。".into()),
    };
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .read_timeout(read_timeout)
        .build()
        .map_err(|_| "无法建立转写连接。")?;
    let part = reqwest::multipart::Part::bytes(wav)
        .file_name("dictation.wav")
        .mime_str("audio/wav")
        .map_err(|_| "无法创建音频请求。")?;
    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("model", model.clone())
        .text("response_format", "json");
    if let Some(language) = language {
        form = form.text("language", language);
    }
    let mut request = client.post(endpoint).multipart(form);
    if let Some(key) = config.get("api_key").filter(|s| !s.is_empty()) {
        let mut header = reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))
            .map_err(|_| "来源密钥格式无效。")?;
        header.set_sensitive(true);
        request = request.header(reqwest::header::AUTHORIZATION, header);
    }
    let operation = async {
        let mut response = request
            .send()
            .await
            .map_err(|_| "转写服务连接失败或超时。".to_string())?;
        if !response.status().is_success() {
            return Err(format!(
                "转写服务返回 HTTP {}；未显示服务错误正文。",
                response.status().as_u16()
            ));
        }
        if response
            .content_length()
            .is_some_and(|s| s > MAX_RESPONSE as u64)
        {
            return Err("转写响应超过大小限制。".into());
        }
        let mut body = vec![];
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "读取转写响应失败或超时。")?
        {
            if chunk.len() > MAX_RESPONSE - body.len() {
                return Err("转写响应超过大小限制。".into());
            }
            body.extend_from_slice(&chunk);
        }
        parse_response(&body)
    };
    tokio::select! {biased; _=cancel.cancelled()=>Err("转写已取消。".into()),result=operation=>result}
}
fn parse_response(bytes: &[u8]) -> Result<String, String> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "转写响应不是合法 JSON 对象。")?;
    value
        .as_object()
        .and_then(|o| o.get("text"))
        .and_then(|s| s.as_str())
        .map(|s| s.trim().to_owned())
        .ok_or_else(|| "转写响应缺少字符串 text。".into())
}
#[cfg(test)]
mod tests;
