//! Native Rust macOS application components.
#[cfg(target_os = "macos")]
mod app;
#[cfg(target_os = "macos")]
mod controller;
#[cfg(target_os = "macos")]
mod hotkey;
pub mod lifecycle;
#[cfg(target_os = "macos")]
mod proxy;
#[cfg(target_os = "macos")]
pub use app::run;
#[cfg(target_os = "macos")]
pub mod audio;
#[cfg(target_os = "macos")]
pub mod delivery;
#[cfg(target_os = "macos")]
pub mod storage;
