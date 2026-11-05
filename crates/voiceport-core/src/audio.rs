use crate::{Error, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;

pub const MAXIMUM_AUDIO_BYTES: u64 = 20_000_000;

pub(crate) struct Audio {
    pub bytes: Vec<u8>,
    pub extension: String,
    pub mime: &'static str,
}

pub(crate) fn load(path: &Path) -> Result<Audio> {
    let extension = path
        .extension()
        .and_then(|v| v.to_str())
        .ok_or(Error::InvalidAudio)?
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "wav" => "audio/wav",
        "m4a" | "mp4" => "audio/mp4",
        "mp3" => "audio/mpeg",
        "webm" => "audio/webm",
        "ogg" => "audio/ogg",
        "flac" => "audio/flac",
        _ => return Err(Error::InvalidAudio),
    };
    // Reject nonregular input before open, then verify the opened handle again.
    // Like the Swift baseline, this does not promise atomic filesystem identity
    // under hostile same-user replacement between these observations.
    let info = std::fs::symlink_metadata(path).map_err(|_| Error::InvalidAudio)?;
    if !info.is_file() || info.len() == 0 {
        return Err(Error::InvalidAudio);
    }
    if info.len() > MAXIMUM_AUDIO_BYTES {
        return Err(Error::AudioTooLarge);
    }
    let file = File::open(path).map_err(|_| Error::InvalidAudio)?;
    let info = file.metadata().map_err(|_| Error::InvalidAudio)?;
    if !info.is_file() || info.len() == 0 {
        return Err(Error::InvalidAudio);
    }
    if info.len() > MAXIMUM_AUDIO_BYTES {
        return Err(Error::AudioTooLarge);
    }
    // Actual reads are capped at exactly 20,000,000 bytes, not just preflight
    // metadata. Re-stat the same open file to reject growth without an extra byte.
    let bytes = read_capped(&file, MAXIMUM_AUDIO_BYTES).map_err(|_| Error::InvalidAudio)?;
    let after = file.metadata().map_err(|_| Error::InvalidAudio)?;
    if after.len() > MAXIMUM_AUDIO_BYTES {
        return Err(Error::AudioTooLarge);
    }
    if bytes.is_empty() || after.len() != bytes.len() as u64 {
        return Err(Error::InvalidAudio);
    }
    Ok(Audio {
        bytes,
        extension,
        mime,
    })
}

pub(crate) fn read_capped(reader: impl Read, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(limit).read_to_end(&mut bytes)?;
    Ok(bytes)
}

pub(crate) fn normalize_language(language: Option<&str>) -> Result<Option<String>> {
    let Some(value) = language.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let mut parts = value.split('-');
    let first = parts.next().unwrap_or_default();
    if !(2..=3).contains(&first.len()) || !first.bytes().all(|v| v.is_ascii_alphabetic()) {
        return Err(Error::InvalidLanguage);
    }
    if let Some(second) = parts.next()
        && (!(2..=8).contains(&second.len()) || !second.bytes().all(|v| v.is_ascii_alphanumeric()))
    {
        return Err(Error::InvalidLanguage);
    }
    if parts.next().is_some() {
        return Err(Error::InvalidLanguage);
    }
    Ok(Some(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io;

    #[test]
    fn accepts_binary_and_supported_extensions_without_exposing_original_name() {
        let temp = tempfile::tempdir().unwrap();
        for extension in ["M4A", "mp4", "wav", "mp3", "webm", "ogg", "flac"] {
            let path = temp.path().join(format!("private-name.{extension}"));
            std::fs::write(&path, [0, 255, 13, 10, 1]).unwrap();
            let audio = load(&path).unwrap();
            assert_eq!(audio.bytes, [0, 255, 13, 10, 1]);
            assert_eq!(audio.extension, extension.to_ascii_lowercase());
        }
    }

    #[test]
    fn rejects_empty_missing_directory_unsupported_and_oversized() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("audio.m4a");
        assert!(matches!(load(&file), Err(Error::InvalidAudio)));
        std::fs::write(&file, []).unwrap();
        assert!(matches!(load(&file), Err(Error::InvalidAudio)));
        std::fs::create_dir(temp.path().join("dir.m4a")).unwrap();
        assert!(matches!(
            load(&temp.path().join("dir.m4a")),
            Err(Error::InvalidAudio)
        ));
        let text = temp.path().join("file.txt");
        std::fs::write(&text, [1]).unwrap();
        assert!(matches!(load(&text), Err(Error::InvalidAudio)));
        File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_len(MAXIMUM_AUDIO_BYTES + 1)
            .unwrap();
        assert!(matches!(load(&file), Err(Error::AudioTooLarge)));
    }

    #[test]
    fn reader_cannot_read_more_than_limit_even_if_source_keeps_growing() {
        struct Growing<'a>(&'a Cell<u64>);
        impl Read for Growing<'_> {
            fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
                buffer.fill(1);
                self.0.set(self.0.get() + buffer.len() as u64);
                Ok(buffer.len())
            }
        }
        let consumed = Cell::new(0);
        assert_eq!(
            read_capped(Growing(&consumed), MAXIMUM_AUDIO_BYTES)
                .unwrap()
                .len(),
            MAXIMUM_AUDIO_BYTES as usize
        );
        assert_eq!(consumed.get(), MAXIMUM_AUDIO_BYTES);
    }

    #[test]
    fn languages_follow_baseline_and_reject_multipart_injection() {
        for value in [None, Some(""), Some(" \n ")] {
            assert_eq!(normalize_language(value).unwrap(), None);
        }
        for value in ["zh", "en", "zh-CN", "eng-12345678"] {
            assert_eq!(
                normalize_language(Some(value)).unwrap().as_deref(),
                Some(value)
            );
        }
        for value in [
            "z",
            "english",
            "zh-",
            "zh-CN-extra",
            "zh\r\nInjected: x",
            "中文",
        ] {
            assert_eq!(normalize_language(Some(value)), Err(Error::InvalidLanguage));
        }
    }
}
