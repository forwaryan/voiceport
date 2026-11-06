use crate::audio;
use crate::auth::{self, CodexCredentials};
use crate::{CancellationToken, Error, Result};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue, USER_AGENT};
use reqwest::{Client, Url, multipart};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const ENDPOINT: &str = "https://chatgpt.com/backend-api/transcribe";
pub const MAXIMUM_RESPONSE_BYTES: usize = 1_000_000;
const USER_AGENT_VALUE: &str = "VoicePort-Rust/0.1";

/// Reuses connections, never credentials. Every call loads its auth file again.
/// No endpoint override is exposed by the library constructor or production CLI.
pub struct TranscriptionClient {
    client: Client,
    auth_file: PathBuf,
    endpoint: Url,
    #[cfg(test)]
    chunk_read: Option<std::sync::Arc<tokio::sync::Notify>>,
}

impl TranscriptionClient {
    pub fn new(auth_file: PathBuf) -> Result<Self> {
        let proxy = std::env::var("VOICEPORT_HTTPS_PROXY").ok();
        Self::configured_with_proxy(
            auth_file,
            ENDPOINT,
            Duration::from_secs(110),
            Duration::from_secs(90),
            proxy.as_deref(),
        )
    }

    #[cfg(test)]
    fn configured(
        auth_file: PathBuf,
        endpoint: &str,
        timeout: Duration,
        read_timeout: Duration,
    ) -> Result<Self> {
        Self::configured_with_proxy(auth_file, endpoint, timeout, read_timeout, None)
    }

    fn configured_with_proxy(
        auth_file: PathBuf,
        endpoint: &str,
        timeout: Duration,
        read_timeout: Duration,
        proxy: Option<&str>,
    ) -> Result<Self> {
        let endpoint = Url::parse(endpoint).map_err(|_| Error::Network)?;
        let mut builder = Client::builder()
            .https_only(endpoint.scheme() == "https")
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy();
        if let Some(proxy) = proxy {
            let url = Url::parse(proxy).map_err(|_| Error::Network)?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || !matches!(url.path(), "" | "/")
            {
                return Err(Error::Network);
            }
            builder = builder.proxy(reqwest::Proxy::https(url).map_err(|_| Error::Network)?);
        }
        let client = builder
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(read_timeout)
            .timeout(timeout)
            .build()
            .map_err(|_| Error::Network)?;
        Ok(Self {
            client,
            auth_file,
            endpoint,
            #[cfg(test)]
            chunk_read: None,
        })
    }

    pub async fn transcribe(
        &self,
        file: &Path,
        language: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<String> {
        check_cancelled(cancel)?;
        let credentials = CodexCredentials::load(&self.auth_file, auth::now_seconds())?;
        let language = audio::normalize_language(language)?;
        let audio = audio::load(file)?;
        check_cancelled(cancel)?;
        let headers = credential_headers(&credentials)?;
        let part = multipart::Part::bytes(audio.bytes)
            .file_name(format!("dictation.{}", audio.extension))
            .mime_str(audio.mime)
            .map_err(|_| Error::InvalidAudio)?;
        let mut form = multipart::Form::new().part("file", part);
        if let Some(language) = language {
            form = form.text("language", language);
        }
        let request = self
            .client
            .post(self.endpoint.clone())
            .headers(headers)
            .multipart(form)
            .build()
            .map_err(|_| Error::Network)?;
        let mut response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            result = self.client.execute(request) => result.map_err(map_transport)?,
        };
        check_cancelled(cancel)?;
        if response.status().as_u16() != 200 {
            return Err(Error::Service(response.status().as_u16()));
        }
        if response
            .content_length()
            .is_some_and(|size| size > MAXIMUM_RESPONSE_BYTES as u64)
        {
            return Err(Error::InvalidResponse);
        }
        let mut body = Vec::new();
        loop {
            let chunk = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(Error::Cancelled),
                result = response.chunk() => result.map_err(map_transport)?,
            };
            let Some(chunk) = chunk else {
                break;
            };
            if chunk.len() > MAXIMUM_RESPONSE_BYTES - body.len() {
                return Err(Error::InvalidResponse);
            }
            body.extend_from_slice(&chunk);
            #[cfg(test)]
            if let Some(observer) = &self.chunk_read {
                observer.notify_one();
            }
        }
        check_cancelled(cancel)?;
        decode_response(&body)
    }
}

fn credential_headers(credentials: &CodexCredentials) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    let mut authorization = HeaderValue::from_str(&format!("Bearer {}", credentials.access_token))
        .map_err(|_| Error::InvalidCredentials)?;
    authorization.set_sensitive(true);
    let mut account =
        HeaderValue::from_str(&credentials.account_id).map_err(|_| Error::InvalidCredentials)?;
    account.set_sensitive(true);
    headers.insert(AUTHORIZATION, authorization);
    headers.insert("chatgpt-account-id", account);
    headers.insert("originator", HeaderValue::from_static("codex_dictation"));
    headers.insert(USER_AGENT, HeaderValue::from_static(USER_AGENT_VALUE));
    Ok(headers)
}

fn check_cancelled(cancel: &CancellationToken) -> Result<()> {
    if cancel.is_cancelled() {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}
fn map_transport(error: reqwest::Error) -> Error {
    if error.is_timeout() {
        Error::TimedOut
    } else {
        Error::Network
    }
}
fn decode_response(body: &[u8]) -> Result<String> {
    // Serde also supports a struct's positional sequence representation. The
    // HTTP contract is a JSON object, so reject that representation explicitly;
    // the typed decoder below still checks syntax, field type and duplicates.
    if !body.trim_ascii_start().starts_with(b"{") {
        return Err(Error::InvalidResponse);
    }
    #[derive(Deserialize)]
    struct Response {
        text: String,
    }
    let response: Response = serde_json::from_slice(body).map_err(|_| Error::InvalidResponse)?;
    if response.text.trim().is_empty() {
        return Err(Error::EmptyTranscript);
    }
    Ok(response.text)
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
