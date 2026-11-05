//! Portable, read-only Codex file login and bounded transcription client.
//! Production uploads always use the fixed ChatGPT endpoint. No recorder,
//! clipboard, login refresh, persistent HTTP credentials or automatic retry.

pub mod audio;
pub mod auth;
mod client;
mod error;

pub use client::TranscriptionClient;
pub use error::{Error, Result};
pub use tokio_util::sync::CancellationToken;

pub mod protocol;
pub mod session;
