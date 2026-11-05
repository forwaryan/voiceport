use crate::audio::read_capped;
use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE};
use serde_json::Value;
use std::ffi::OsStr;
use std::fmt;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAXIMUM_AUTH_BYTES: u64 = 1_000_000;

/// Opaque credentials. Neither Debug nor Display exposes either header value.
#[derive(Clone)]
pub struct CodexCredentials {
    pub(crate) access_token: String,
    pub(crate) account_id: String,
    expires_at: Option<f64>,
}
impl fmt::Debug for CodexCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("CodexCredentials(<redacted>)")
    }
}
impl fmt::Display for CodexCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

pub fn default_path_from(codex_home: Option<&OsStr>, home: &Path) -> PathBuf {
    codex_home
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
        .join("auth.json")
}

pub fn default_path() -> Result<PathBuf> {
    let codex_home = std::env::var_os("CODEX_HOME");
    if let Some(value) = codex_home.as_deref().filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(value).join("auth.json"));
    }
    let home = dirs::home_dir().ok_or(Error::SignInRequired)?;
    Ok(default_path_from(None, &home))
}

pub(crate) fn now_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

impl CodexCredentials {
    pub fn load(file: &Path, now: f64) -> Result<Self> {
        let info = std::fs::metadata(file).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::SignInRequired
            } else {
                Error::InvalidCredentials
            }
        })?;
        if !info.is_file() || info.len() > MAXIMUM_AUTH_BYTES {
            return Err(Error::InvalidCredentials);
        }
        let file = File::open(file).map_err(|_| Error::InvalidCredentials)?;
        let info = file.metadata().map_err(|_| Error::InvalidCredentials)?;
        if !info.is_file() || info.len() > MAXIMUM_AUTH_BYTES {
            return Err(Error::InvalidCredentials);
        }
        let bytes =
            read_capped(&file, MAXIMUM_AUTH_BYTES).map_err(|_| Error::InvalidCredentials)?;
        let after = file.metadata().map_err(|_| Error::InvalidCredentials)?;
        if after.len() > MAXIMUM_AUTH_BYTES || after.len() != bytes.len() as u64 {
            return Err(Error::InvalidCredentials);
        }
        Self::decode(&bytes, now)
    }

    pub fn decode(data: &[u8], now: f64) -> Result<Self> {
        if data.len() as u64 > MAXIMUM_AUTH_BYTES || !now.is_finite() {
            return Err(Error::InvalidCredentials);
        }
        let object: Value = serde_json::from_slice(data).map_err(|_| Error::InvalidCredentials)?;
        if object
            .get("auth_mode")
            .is_some_and(|mode| mode.as_str() != Some("chatgpt"))
        {
            return Err(Error::InvalidCredentials);
        }
        let tokens = object.get("tokens").ok_or(Error::InvalidCredentials)?;
        let access_token = tokens
            .get("access_token")
            .and_then(Value::as_str)
            .filter(|v| valid_header(v))
            .ok_or(Error::InvalidCredentials)?
            .to_owned();
        let account_id = tokens
            .get("account_id")
            .and_then(Value::as_str)
            .filter(|v| valid_header(v))
            .ok_or(Error::InvalidCredentials)?
            .to_owned();
        let expires_at = expiration(&access_token);
        if expires_at.is_some_and(|expiry| expiry - now <= 30.0) {
            return Err(Error::ExpiredCredentials);
        }
        Ok(Self {
            access_token,
            account_id,
            expires_at,
        })
    }

    pub fn expires_at(&self) -> Option<f64> {
        self.expires_at
    }
}
fn valid_header(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|v| (0x21..=0x7e).contains(&v))
}
fn expiration(token: &str) -> Option<f64> {
    let parts: Vec<_> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let mut payload = parts[1].to_owned();
    while !payload.len().is_multiple_of(4) {
        payload.push('=');
    }
    let bytes = URL_SAFE.decode(payload).ok()?;
    let object: Value = serde_json::from_slice(&bytes).ok()?;
    object
        .get("exp")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn auth(token: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({"auth_mode":"chatgpt","tokens":{"access_token":token,"account_id":"synthetic-account"}})).unwrap()
    }

    #[test]
    fn chatgpt_and_legacy_tokens_accepted() {
        let value = CodexCredentials::decode(&auth("synthetic-token"), 2_000.0).unwrap();
        assert_eq!(value.account_id, "synthetic-account");
        assert_eq!(value.access_token, "synthetic-token");
        assert!(
            CodexCredentials::decode(
                br#"{"tokens":{"access_token":"old","account_id":"account"}}"#,
                2_000.0
            )
            .is_ok()
        );
    }
    #[test]
    fn rejects_api_key_modes_missing_types_and_malformed_json() {
        for body in [
            "not-json",
            "[]",
            "{}",
            r#"{"OPENAI_API_KEY":"synthetic-key"}"#,
            r#"{"auth_mode":"apikey","tokens":{"access_token":"token","account_id":"account"}}"#,
            r#"{"auth_mode":null,"tokens":{"access_token":"token","account_id":"account"}}"#,
            r#"{"tokens":{"access_token":42,"account_id":"account"}}"#,
        ] {
            assert_eq!(
                CodexCredentials::decode(body.as_bytes(), 0.0).unwrap_err(),
                Error::InvalidCredentials
            );
        }
    }
    #[test]
    fn rejects_header_injection_in_both_fields() {
        for value in [
            "",
            " ",
            "private\r\nInjected: value",
            "non-ascii-令牌",
            "has space",
            "\u{7f}",
        ] {
            assert_eq!(
                CodexCredentials::decode(&auth(value), 0.0).unwrap_err(),
                Error::InvalidCredentials
            );
            let body =
                serde_json::to_vec(&json!({"tokens":{"access_token":"valid","account_id":value}}))
                    .unwrap();
            assert_eq!(
                CodexCredentials::decode(&body, 0.0).unwrap_err(),
                Error::InvalidCredentials
            );
        }
    }
    #[test]
    fn jwt_expiration_has_thirty_second_margin_and_opaque_fallback() {
        for expiry in [1_000.0, 2_000.0, 2_030.0, 2_030.1, 5_000.0] {
            let token = format!(
                "header.{}.signature",
                URL_SAFE.encode(serde_json::to_vec(&json!({"exp":expiry})).unwrap())
            );
            let result = CodexCredentials::decode(&auth(&token), 2_000.0);
            if expiry <= 2_030.0 {
                assert_eq!(result.unwrap_err(), Error::ExpiredCredentials);
            } else {
                assert_eq!(result.unwrap().expires_at(), Some(expiry));
            }
        }
        for token in ["opaque", "bad.bad.bad", "header.e30.signature"] {
            assert_eq!(
                CodexCredentials::decode(&auth(token), 0.0)
                    .unwrap()
                    .expires_at(),
                None
            );
        }
    }
    #[test]
    fn default_paths_are_injected_without_consulting_real_login() {
        let home = Path::new("synthetic-home");
        assert_eq!(default_path_from(None, home), home.join(".codex/auth.json"));
        assert_eq!(
            default_path_from(Some(OsStr::new("")), home),
            home.join(".codex/auth.json")
        );
        assert_eq!(
            default_path_from(Some(OsStr::new("synthetic-codex")), home),
            Path::new("synthetic-codex/auth.json")
        );
    }
    #[test]
    fn file_load_limits_missing_files_and_descriptions_are_redacted() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("auth.json");
        assert_eq!(
            CodexCredentials::load(&file, 0.0).unwrap_err(),
            Error::SignInRequired
        );
        std::fs::write(&file, auth("synthetic-private-token")).unwrap();
        let credentials = CodexCredentials::load(&file, 0.0).unwrap();
        assert_eq!(format!("{credentials:?}"), "CodexCredentials(<redacted>)");
        assert_eq!(format!("{credentials}"), "CodexCredentials(<redacted>)");
        File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_len(MAXIMUM_AUTH_BYTES + 1)
            .unwrap();
        assert_eq!(
            CodexCredentials::load(&file, 0.0).unwrap_err(),
            Error::InvalidCredentials
        );
    }
}
