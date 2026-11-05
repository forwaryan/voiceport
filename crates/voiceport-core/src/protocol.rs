use serde::{Deserialize, Serialize};
use std::io::{self, BufRead};
pub const INPUT_LIMIT: usize = 16_384;
pub const OUTPUT_LIMIT: usize = 2_000_000;
pub const REQUEST_LIMIT: usize = 1024;
#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Status {
        version: u8,
        request_id: String,
    },
    Transcribe {
        version: u8,
        request_id: String,
        audio_path: String,
        language: Option<String>,
    },
    Retry {
        version: u8,
        request_id: String,
        session_id: String,
    },
    Cancel {
        version: u8,
        request_id: String,
        session_id: String,
    },
    Shutdown {
        version: u8,
        request_id: String,
    },
}
impl Command {
    pub fn id(&self) -> &str {
        match self {
            Self::Status { request_id, .. }
            | Self::Transcribe { request_id, .. }
            | Self::Retry { request_id, .. }
            | Self::Cancel { request_id, .. }
            | Self::Shutdown { request_id, .. } => request_id,
        }
    }
    fn version(&self) -> u8 {
        match self {
            Self::Status { version, .. }
            | Self::Transcribe { version, .. }
            | Self::Retry { version, .. }
            | Self::Cancel { version, .. }
            | Self::Shutdown { version, .. } => *version,
        }
    }
}
pub fn parse(bytes: &[u8]) -> Option<Command> {
    let command: Command = serde_json::from_slice(bytes).ok()?;
    let id = command.id();
    if command.version() != 1
        || id.is_empty()
        || id.len() > 64
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b))
    {
        return None;
    }
    if let Command::Transcribe { audio_path, .. } = &command
        && (audio_path.is_empty() || audio_path.contains('\0'))
    {
        return None;
    }
    Some(command)
}
/// Bounded accumulation; oversize input is drained through LF without growing.
/// EOF with any unterminated bytes produces one error, then EOF.
pub fn frame(reader: &mut impl BufRead) -> io::Result<Option<Option<Command>>> {
    let mut bytes = Vec::new();
    let mut oversized = false;
    let mut any = false;
    loop {
        let buf = reader.fill_buf()?;
        if buf.is_empty() {
            return Ok(if any { Some(None) } else { None });
        }
        any = true;
        let position = buf.iter().position(|b| *b == b'\n');
        let count = position.map_or(buf.len(), |i| i + 1);
        let content = if position.is_some() { count - 1 } else { count };
        if !oversized {
            if bytes.len() + content > INPUT_LIMIT {
                oversized = true;
                bytes.clear();
            } else {
                bytes.extend_from_slice(&buf[..content]);
            }
        }
        reader.consume(count);
        if position.is_some() {
            return Ok(Some(if oversized { None } else { parse(&bytes) }));
        }
    }
}
#[derive(Serialize)]
pub struct Event {
    pub version: u8,
    pub event: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_retry: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}
impl Event {
    pub fn new(
        event: &'static str,
        request_id: Option<String>,
        session_id: Option<String>,
        state: &'static str,
    ) -> Self {
        Self {
            version: 1,
            event,
            request_id,
            session_id,
            state,
            is_retry: None,
            code: None,
            text: None,
        }
    }
    pub fn error(id: Option<String>, code: &str, state: &'static str) -> Self {
        let mut e = Self::new("error", id, None, state);
        e.code = Some(code.into());
        e
    }
    pub fn encode(&self) -> Option<Vec<u8>> {
        struct Capped(Vec<u8>);
        impl io::Write for Capped {
            fn write(&mut self, data: &[u8]) -> io::Result<usize> {
                if data.len() > OUTPUT_LIMIT - self.0.len() {
                    return Err(io::Error::other("frame limit"));
                }
                self.0.extend_from_slice(data);
                Ok(data.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let mut capped = Capped(Vec::new());
        serde_json::to_writer(&mut capped, self).ok()?;
        let mut bytes = capped.0;
        if bytes.len() > OUTPUT_LIMIT {
            return None;
        }
        bytes.push(b'\n');
        Some(bytes)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_frames_and_recovery() {
        for value in [
            r#"{"version":1,"action":"status","request_id":"a","token":"secret"}"#,
            r#"{"version":1,"action":"status","request_id":"a","request_id":"b"}"#,
            r#"{"version":2,"action":"status","request_id":"a"}"#,
            r#"{"version":1,"action":"status","request_id":"bad id"}"#,
        ] {
            assert!(parse(value.as_bytes()).is_none());
        }
        let mut bytes = vec![b'x'; INPUT_LIMIT + 100];
        bytes.extend_from_slice(
            b"\n{\"version\":1,\"action\":\"status\",\"request_id\":\"ok\"}\r\nunterminated",
        );
        let mut reader = io::Cursor::new(bytes);
        assert!(frame(&mut reader).unwrap().unwrap().is_none());
        assert!(frame(&mut reader).unwrap().unwrap().is_some());
        assert!(frame(&mut reader).unwrap().unwrap().is_none());
        assert!(frame(&mut reader).unwrap().is_none());
    }
}
