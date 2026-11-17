//! Model-page presets for the existing HTTP adapter. These functions perform no I/O.
use crate::plugins::{HTTP, Source, validate_endpoint};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Preset {
    GptTranscribe,
    Compatible,
}
impl Preset {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::GptTranscribe => "GPT Transcribe",
            Self::Compatible => "OpenAI-compatible",
        }
    }
    pub(crate) fn base_url(self) -> &'static str {
        match self {
            Self::GptTranscribe => "https://api.openai.com/v1",
            Self::Compatible => "",
        }
    }
    pub(crate) fn model(self) -> &'static str {
        match self {
            Self::GptTranscribe => "gpt-4o-transcribe",
            Self::Compatible => "",
        }
    }
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::GptTranscribe => "选中后将音频发送给 OpenAI；使用独立 API 密钥及服务计费。",
            Self::Compatible => "选中后将音频发送到你配置的服务；本地服务器可不填密钥。",
        }
    }
}

/// Accept a versioned API base or an already complete standard transcription URL.
/// Retain a legacy custom endpoint unchanged when editing its existing value.
/// Query parameters are rejected here: secrets must be kept in Keychain.
pub(crate) fn transcription_endpoint(input: &str, old: Option<&str>) -> Result<String, String> {
    let input = input.trim();
    let mut url = validate_endpoint(input)?;
    if old == Some(input) {
        return Ok(input.into());
    }
    if url.query().is_some() {
        return Err("基础 URL 不能包含查询参数；API 密钥请填入密钥栏。".into());
    }
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with("/audio/transcriptions") {
        path.to_owned()
    } else {
        format!("{path}/audio/transcriptions")
    };
    url.set_path(&path);
    Ok(url.into())
}

pub(crate) fn editable_base(endpoint: &str) -> String {
    let Ok(mut url) = validate_endpoint(endpoint) else {
        return endpoint.into();
    };
    if url.query().is_none()
        && let Some(base) = url
            .path()
            .trim_end_matches('/')
            .strip_suffix("/audio/transcriptions")
    {
        let base = base.to_owned();
        url.set_path(&base);
        return url.as_str().trim_end_matches('/').into();
    }
    endpoint.into()
}

/// Display only destination origin; never echo a path/query supplied by a service.
pub(crate) fn destination(source: &Source) -> String {
    source
        .config
        .get("endpoint")
        .and_then(|s| validate_endpoint(s).ok())
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_else(|| "地址需要修正".into())
}
pub(crate) fn is_http(source: &Source) -> bool {
    source.plugin_id == HTTP
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bases_and_complete_urls_normalize_once_and_keep_legacy_endpoints() {
        for (base, expected) in [
            (
                "https://api.openai.com/v1",
                "https://api.openai.com/v1/audio/transcriptions",
            ),
            (
                "https://api.groq.com/openai/v1/",
                "https://api.groq.com/openai/v1/audio/transcriptions",
            ),
            (
                "http://127.0.0.1:8080",
                "http://127.0.0.1:8080/audio/transcriptions",
            ),
            (
                "http://[::1]:8080/v1/audio/transcriptions/",
                "http://[::1]:8080/v1/audio/transcriptions",
            ),
        ] {
            assert_eq!(transcription_endpoint(base, None).unwrap(), expected);
            assert_eq!(transcription_endpoint(expected, None).unwrap(), expected);
            assert_eq!(
                transcription_endpoint(&editable_base(expected), None).unwrap(),
                expected
            );
        }
        let legacy = "https://example.invalid/custom/transcribe?route=one";
        assert_eq!(
            transcription_endpoint(legacy, Some(legacy)).unwrap(),
            legacy
        );
        assert_eq!(editable_base(legacy), legacy);
        for bad in [
            "http://remote.invalid/v1",
            "https://u:p@example.invalid/v1",
            "https://example.invalid/v1?key=synthetic",
            "https://example.invalid/#key",
            "file:///tmp/x",
        ] {
            assert!(transcription_endpoint(bad, None).is_err());
        }
    }
}
