//! Private native storage foundation. No constructor IO or automatic cleanup.
use std::{fmt, path::Path};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    Unavailable,
    UnsafeObject,
    Collision,
    InvalidRecording,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Unsupported => "Private storage unsupported",
            Self::Unavailable => "Private storage unavailable",
            Self::UnsafeObject => "Private storage ownership could not be verified",
            Self::Collision => "Private storage name collision",
            Self::InvalidRecording => "Recording could not be finalized",
        })
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
/// Only drive-qualified local paths, bounded depth, no namespace/ADS/dot aliases.
pub fn validate_local_path(path: &Path) -> Result<()> {
    let s = path.to_str().ok_or(Error::Unsupported)?;
    let bytes = s.as_bytes();
    if bytes.len() < 3
        || bytes.len() > 180
        || !bytes[0].is_ascii_alphabetic()
        || &bytes[1..3] != b":\\"
    {
        return Err(Error::Unsupported);
    }
    let parts: Vec<_> = s[3..].split('\\').collect();
    if parts.len() > 24
        || parts.iter().any(|p| {
            p.is_empty()
                || *p == "."
                || *p == ".."
                || p.ends_with([' ', '.'])
                || reserved(p)
                || p.chars().any(|c| c < ' ' || "<>:\"/|?*".contains(c))
        })
    {
        return Err(Error::Unsupported);
    }
    Ok(())
}
#[derive(Default)]
pub struct Storage;
#[cfg(not(windows))]
impl Storage {
    pub fn open_or_create(&self) -> Result<Root> {
        Err(Error::Unsupported)
    }
}
#[cfg(not(windows))]
pub struct Root;
#[cfg(windows)]
mod native;
#[cfg(windows)]
pub use native::{Pending, Root};

fn reserved(s: &str) -> bool {
    let head = s.split('.').next().unwrap_or_default().to_ascii_uppercase();
    matches!(
        head.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
    ) || ["COM¹", "COM²", "COM³", "LPT¹", "LPT²", "LPT³"].contains(&head.as_str())
        || (head.len() == 4
            && (head.starts_with("COM") || head.starts_with("LPT"))
            && matches!(head.as_bytes()[3], b'1'..=b'9'))
}
