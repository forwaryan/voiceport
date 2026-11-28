//! Native runtime compatibility and per-recording engine snapshot for automatic app rules.
use super::runtime_staging_directory;
use crate::local_models::{
    Model, ModelKind, model_file, runtime_libraries, runtime_version, validate_runtime_for,
};
use std::{
    ffi::{CStr, CString, c_char},
    fs::{self, Metadata, OpenOptions},
    io::{self, Read},
    os::unix::fs::MetadataExt,
    path::Path,
};
use tempfile::TempDir;

unsafe extern "C" {
    fn vp_local_check_engine(
        directory: *const c_char,
        version: *const c_char,
        mode: i32,
        error: *mut c_char,
        capacity: usize,
    ) -> bool;
}

fn check_engine(directory: &Path, version: &str, kind: ModelKind) -> Result<(), String> {
    let directory = CString::new(directory.as_os_str().as_encoded_bytes())
        .map_err(|_| "识别引擎路径包含无效字符。")?;
    // runtime_libraries already accepted this package version.
    let version = CString::new(version).unwrap();
    let mut error = [0 as c_char; 1024];
    // SAFETY: The bridge uses the versioned C ABI and closes its handles before returning.
    if unsafe {
        vp_local_check_engine(
            directory.as_ptr(),
            version.as_ptr(),
            kind.native_mode(),
            error.as_mut_ptr(),
            error.len(),
        )
    } {
        Ok(())
    } else {
        let detail = unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy();
        Err(format!("识别引擎无法加载：{detail}"))
    }
}

pub(crate) fn stage_app_engine(model: &mut Model) -> Result<TempDir, String> {
    let version = runtime_version(&model.runtime)?;
    let libraries = runtime_libraries(&version)?;
    let stage = runtime_staging_directory()?;
    for name in libraries {
        copy_library(&model.runtime.join(name), &stage.path().join(name))
            .map_err(|error| format!("无法暂存识别引擎 {name}：{error}"))?;
    }
    if runtime_version(&model.runtime)? != version {
        return Err("识别引擎版本在准备期间发生变化，请重试。".into());
    }
    fs::write(
        stage.path().join("package.json"),
        serde_json::json!({"version": version}).to_string(),
    )
    .map_err(|error| error.to_string())?;
    validate_runtime_for(stage.path(), std::env::consts::ARCH, model.kind)?;
    // Use the checked copies for the worker and any retry, even if the installation changes.
    check_engine(stage.path(), &version, model.kind)?;
    model.runtime = stage.path().to_owned();
    Ok(stage)
}

fn copy_library(source: &Path, target: &Path) -> io::Result<()> {
    let input = model_file(source)?;
    let original = input.metadata()?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    let copied = io::copy(&mut (&input).take(original.len()), &mut output)?;
    let identity = |metadata: &Metadata| {
        (
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
    };
    let current = fs::symlink_metadata(source)?;
    if copied != original.len() || identity(&current) != identity(&original) {
        return Err(io::Error::other("识别引擎文件在准备期间发生变化"));
    }
    Ok(())
}

#[cfg(all(debug_assertions, target_arch = "aarch64"))]
fn compile_fixture(library: &Path, mut clang: std::process::Command) {
    let pending = library.with_extension("building");
    let output = clang
        .args([
            "-std=c++17",
            "-dynamiclib",
            "-arch",
            "arm64",
            "-install_name",
        ])
        .arg(format!(
            "@loader_path/{}",
            library.file_name().unwrap().to_str().unwrap()
        ))
        .arg("-o")
        .arg(&pending)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::rename(pending, library).unwrap();
}

#[cfg(all(debug_assertions, target_arch = "aarch64"))]
pub(crate) fn install_fixture_sherpa(
    root: &Path,
    ort: &Path,
    sherpa: &Path,
    abi: &str,
    omit_symbol: bool,
) {
    let mut clang = std::process::Command::new("/usr/bin/clang++");
    clang
        .arg(root.join("engine_fixture.cc"))
        .arg(ort)
        .arg(format!(r#"-DFIXTURE_ABI="{abi}""#));
    if omit_symbol {
        clang.arg("-DFIXTURE_OMIT_OFFLINE_DECODE");
    }
    compile_fixture(sherpa, clang);
}

#[cfg(all(debug_assertions, target_arch = "aarch64"))]
pub(crate) fn fixture_catalog(
    root: &Path,
) -> (crate::local_models::Catalog, [std::path::PathBuf; 2]) {
    use crate::local_models::{Catalog, LEGACY_RUNTIME_VERSION, PARAFORMER};

    let runtime = root.join("runtime");
    fs::create_dir(&runtime).unwrap();
    fs::write(runtime.join("package.json"), br#"{"version":"1.12.37"}"#).unwrap();
    let [ort, sherpa] = runtime_libraries(LEGACY_RUNTIME_VERSION)
        .unwrap()
        .map(|library| runtime.join(library));
    let ort_source = root.join("ort_fixture.cc");
    fs::write(
        &ort_source,
        r#"extern "C" int vp_fixture_ort() { return 1; }"#,
    )
    .unwrap();
    let mut clang = std::process::Command::new("/usr/bin/clang++");
    clang.arg(ort_source);
    compile_fixture(&ort, clang);
    fs::write(root.join("c-api.h"), include_bytes!("c-api.h")).unwrap();
    fs::write(
        root.join("engine_fixture.cc"),
        include_bytes!("engine_fixture.cc"),
    )
    .unwrap();
    install_fixture_sherpa(root, &ort, &sherpa, LEGACY_RUNTIME_VERSION, false);
    let model = root.join("paraformer-bilingual-zh-en");
    fs::create_dir(&model).unwrap();
    for name in ["encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt"] {
        fs::write(model.join(name), [1]).unwrap();
    }
    let catalog = Catalog::discover(root, &runtime, PARAFORMER, "aarch64");
    assert!(catalog.snapshot(PARAFORMER).is_ok());
    (catalog, [ort, sherpa])
}
