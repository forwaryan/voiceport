//! Identity of the separately packaged macOS test executables.
use std::path::PathBuf;

pub const APP_NAME: &str = "VoicePort Test";

#[cfg(target_os = "macos")]
pub fn active() -> bool {
    use std::sync::LazyLock;
    // The executable basename survives moving or renaming the outer .app.
    // Do not infer the profile from argv[0], HOME, or an inherited flag.
    static ACTIVE: LazyLock<bool> = LazyLock::new(|| {
        std::env::current_exe().is_ok_and(|path| {
            let path = path.canonicalize().unwrap_or(path);
            path.file_name()
                .is_some_and(|name| name == "voiceport-test-app" || name == "voiceport-test")
        })
    });
    *ACTIVE
}

#[cfg(not(target_os = "macos"))]
pub fn active() -> bool {
    false
}

pub fn application_name() -> &'static str {
    if active() { APP_NAME } else { "VoicePort" }
}

pub fn support_directory() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join("Library/Application Support").join(APP_NAME))
}
