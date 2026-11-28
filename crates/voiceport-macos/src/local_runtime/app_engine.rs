//! Loader preflight and per-recording engine snapshot for automatic app rules.
use super::runtime_staging_directory;
use crate::local_models::{
    Model, model_file, runtime_libraries, runtime_version, validate_runtime_for,
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
    fn dlopen_preflight(path: *const c_char) -> bool;
}

fn preflight(directory: &Path, libraries: [&str; 2]) -> Result<(), String> {
    for name in libraries {
        let path = CString::new(directory.join(name).as_os_str().as_encoded_bytes())
            .map_err(|_| "识别引擎路径包含无效字符。")?;
        // dyld checks the current image without opening a handle or running initializers.
        if !unsafe { dlopen_preflight(path.as_ptr()) } {
            let error = unsafe { libc::dlerror() };
            let detail = if error.is_null() {
                "动态加载预检未通过".into()
            } else {
                unsafe { CStr::from_ptr(error) }.to_string_lossy()
            };
            return Err(format!("识别引擎 {name} 无法加载：{detail}"));
        }
    }
    Ok(())
}

#[cfg(all(test, debug_assertions, target_arch = "aarch64"))]
pub(crate) fn check_app_engine(directory: &Path) -> Result<(), String> {
    preflight(directory, runtime_libraries(&runtime_version(directory)?)?)
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
    preflight(stage.path(), libraries)?;
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
pub(crate) fn fixture_catalog(root: &Path) -> (crate::local_models::Catalog, std::path::PathBuf) {
    use crate::local_models::{Catalog, LEGACY_RUNTIME_VERSION, PARAFORMER};
    use std::process::Command;

    let runtime = root.join("runtime");
    fs::create_dir(&runtime).unwrap();
    fs::write(runtime.join("package.json"), br#"{"version":"1.12.37"}"#).unwrap();
    let [ort, sherpa] = runtime_libraries(LEGACY_RUNTIME_VERSION).unwrap();
    let ort_path = runtime.join(ort);
    let code = root.join("engine_fixture.c");
    let compile = |name, body: &str, dependency: Option<&Path>| {
        fs::write(&code, body).unwrap();
        let output = Command::new("/usr/bin/clang")
            .args([
                "-dynamiclib",
                "-arch",
                "arm64",
                "-install_name",
                &format!("@loader_path/{name}"),
            ])
            .arg(&code)
            .args(dependency)
            .arg("-o")
            .arg(runtime.join(name))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    compile(ort, "int vp_fixture_ort(void) { return 1; }", None);
    compile(
        sherpa,
        "extern int vp_fixture_ort(void); int vp_fixture_sherpa(void) { return vp_fixture_ort(); }",
        Some(&ort_path),
    );
    let model = root.join("paraformer-bilingual-zh-en");
    fs::create_dir(&model).unwrap();
    for name in ["encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt"] {
        fs::write(model.join(name), [1]).unwrap();
    }
    let catalog = Catalog::discover(root, &runtime, PARAFORMER, "aarch64");
    assert!(catalog.snapshot(PARAFORMER).is_ok());
    (catalog, ort_path)
}
