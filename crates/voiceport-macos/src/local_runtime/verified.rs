use crate::local_models::{LEGACY_RUNTIME_VERSION, RUNTIME_VERSION, model_file, runtime_version};
use ring::digest::{Context, SHA256};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::Path,
};
use tempfile::TempDir;

struct Library {
    name: &'static str,
    bytes: u64,
    sha256: &'static str,
}

// Exact regular members of the official sherpa-onnx-darwin-arm64 npm archives.
// See SOURCE.md for the archive pins and linked dependency scope.
const CURRENT: [Library; 2] = [
    Library {
        name: "libonnxruntime.dylib",
        bytes: 29_006_384,
        sha256: "b0613d0ae53199a83b05fa48e169211498e9d40d54beaa372068ebe5ec5b0929",
    },
    Library {
        name: "libsherpa-onnx-c-api.dylib",
        bytes: 4_237_128,
        sha256: "08874710fd7e215a0d6724d43f05626e59807642b63db7766fe7577737592b2c",
    },
];
const LEGACY: [Library; 2] = [
    Library {
        name: "libonnxruntime.1.23.2.dylib",
        bytes: 35_138_784,
        sha256: "d306d2bc768540766c7ed8a1e0ff05d2870c77a934ebeee4a7bafa1b732ef299",
    },
    Library {
        name: "libsherpa-onnx-c-api.dylib",
        bytes: 4_054_024,
        sha256: "78f3df545285af08c34d087b9066bc7b7cfdfa4a0c693674b83be4cda349a0f8",
    },
];

pub(crate) fn stage_verified_runtime(directory: &Path) -> Result<TempDir, String> {
    let version = runtime_version(directory)?;
    let libraries = match version.as_str() {
        RUNTIME_VERSION => &CURRENT,
        LEGACY_RUNTIME_VERSION => &LEGACY,
        _ => return Err("识别引擎版本没有受信的发行记录。".into()),
    };
    // Use the system sticky directory and owner-only mode, independent of the
    // model directory and callers' TMPDIR. dyld opens only these copies.
    let stage = super::runtime_staging_directory()?;
    for library in libraries {
        copy_verified(directory, stage.path(), library)
            .map_err(|error| format!("{}: {error}", library.name))?;
    }
    fs::write(
        stage.path().join("package.json"),
        serde_json::json!({"version": version}).to_string(),
    )
    .map_err(|error| error.to_string())?;
    Ok(stage)
}

fn copy_verified(directory: &Path, stage: &Path, library: &Library) -> io::Result<()> {
    let source = model_file(&directory.join(library.name))?;
    let mismatch = || io::Error::other("library does not match the pinned release");
    if source.metadata()?.len() != library.bytes {
        return Err(mismatch());
    }
    // Copy and hash the same stream from one open descriptor. The private copy
    // is the one passed to dyld, even if the installation changes during use.
    let mut input = source.take(library.bytes + 1);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(stage.join(library.name))?;
    let mut hash = Context::new(&SHA256);
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0;
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        hash.update(&buffer[..count]);
        bytes += count as u64;
    }
    let digest = hash.finish();
    if bytes != library.bytes || crate::model_library::hex(digest.as_ref()) != library.sha256 {
        return Err(mismatch());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_same_length_content_and_keeps_the_verified_copy() {
        let input = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let library = Library {
            name: "fixture",
            bytes: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        };
        let source = input.path().join(library.name);
        let staged = output.path().join(library.name);
        fs::write(&source, b"abc").unwrap();
        copy_verified(input.path(), output.path(), &library).unwrap();
        fs::write(&source, b"abd").unwrap();
        assert_eq!(fs::read(&staged).unwrap(), b"abc");
        fs::remove_file(&staged).unwrap();
        let error = copy_verified(input.path(), output.path(), &library).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not match the pinned release")
        );
    }
}
