//! Native Rust macOS application components.
#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod app_update;
#[cfg(target_os = "macos")]
mod cloud_models;
#[cfg(target_os = "macos")]
mod controller;
#[cfg(target_os = "macos")]
mod dictation_dictionary;
#[cfg(target_os = "macos")]
mod dictation_workflows;
#[cfg(target_os = "macos")]
mod hotkey;
#[cfg(target_os = "macos")]
mod input_target;
pub mod lifecycle;
pub mod local_audio;
pub mod local_models;
#[cfg(target_os = "macos")]
mod local_runtime;
#[cfg(target_os = "macos")]
pub mod model_library;
#[cfg(target_os = "macos")]
mod plugin_credentials;
#[cfg(target_os = "macos")]
mod plugin_process;
#[cfg(target_os = "macos")]
mod plugin_runtime;
#[cfg(target_os = "macos")]
pub mod plugins;
#[cfg(target_os = "macos")]
mod proxy;
#[cfg(target_os = "macos")]
mod shortcut;
#[cfg(target_os = "macos")]
mod shortcut_recorder;
#[cfg(target_os = "macos")]
mod source_probe;
#[cfg(target_os = "macos")]
mod source_service;
#[cfg(target_os = "macos")]
mod source_tests;
#[cfg(target_os = "macos")]
mod status_icon;
#[cfg(target_os = "macos")]
mod status_overlay;
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub use app::first_run_field_fixture;
#[cfg(target_os = "macos")]
#[doc(hidden)]
pub use app::isolated_shortcut_regression;
#[cfg(target_os = "macos")]
pub use app::run;
#[cfg(target_os = "macos")]
pub mod audio;
#[cfg(target_os = "macos")]
pub mod delivery;
#[cfg(target_os = "macos")]
pub mod storage;
