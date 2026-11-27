//! Native failure regression on the OS main thread, without capture or permissions.
//! Include the production modules so their cfg(test) fixtures can reach private
//! state; all Stop/poll, native writer and storage methods remain the real ones.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/audio.rs"]
mod audio;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/controller.rs"]
mod controller;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/dictation_dictionary.rs"]
mod dictation_dictionary;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/dictation_output.rs"]
mod dictation_output;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/dictation_workflows.rs"]
mod dictation_workflows;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/local_audio.rs"]
mod local_audio;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/local_models.rs"]
mod local_models;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src/local_runtime.rs"]
mod local_runtime;
// Use the source directory as the module root so each plugin module's nested
// test file resolves normally, rather than resolving both as src/tests.rs.
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../src"]
mod plugin_dependencies {
    pub(crate) mod delivery;
    pub(crate) mod file_transcription;
    pub(crate) mod model_library;
    pub(crate) mod plugin_credentials;
    pub(crate) mod plugin_process;
    pub(crate) mod plugin_runtime;
    pub(crate) mod source_probe;
    pub(crate) mod source_tests;
}
#[cfg(target_os = "macos")]
use plugin_dependencies::{
    delivery, file_transcription, model_library, plugin_credentials, plugin_process,
    plugin_runtime, source_probe, source_tests,
};
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)] // Its libtest-only imports are inert in this harness.
#[path = "../src/proxy.rs"]
mod proxy;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/status_overlay.rs"]
mod status_overlay;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/app/status_overlay.rs"]
mod status_overlay_panel;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/storage.rs"]
mod storage;
#[cfg(target_os = "macos")]
use voiceport_macos::{lifecycle, plugins};

#[cfg(target_os = "macos")]
fn main() {
    let main_thread = objc2::MainThreadMarker::new()
        .expect("stop_failure must execute on the actual OS main thread");
    println!("stop_failure: os_main_thread=true");
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args
        .first()
        .is_some_and(|flag| flag == "--status-overlay-focus")
    {
        let [_, consent, boot_id, session_id] = args.as_slice() else {
            eprintln!(
                "status-overlay-focus: refused before AppKit (isolated VM authorization required)"
            );
            std::process::exit(78);
        };
        if consent != "--authorized-isolated-vm" {
            eprintln!(
                "status-overlay-focus: refused before AppKit (isolated VM authorization required)"
            );
            std::process::exit(78);
        }
        objc2::rc::autoreleasepool(|_| {
            status_overlay_panel::fixture::run(main_thread, boot_id, session_id)
        });
    }
    let cases: Vec<&str> = match args.as_slice() {
        [] => vec![
            "zero",
            "stop-first",
            "poll-first",
            "retry",
            "cancel",
            "local-stop-failure",
            "cli-session",
            "gui-delivery",
        ],
        [flag, case] if flag == "--case" => vec![case],
        _ => panic!(
            "expected no arguments or --case zero|stop-first|poll-first|retry|cancel|local-stop-failure|cli-session|gui-delivery"
        ),
    };
    for case in cases {
        objc2::rc::autoreleasepool(|_| {
            controller::stop_failure_regression::run(case, main_thread);
        });
    }
    println!("stop_failure: all selected cases passed; 0 failed");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("stop_failure: macOS-only native regression; not exercised on this platform");
}
