//! Test-only executable: synthetic service, same production host/parser/writer.
#[path = "../src/host.rs"]
mod host;
use std::{sync::Arc, time::Duration};
use voiceport_core::Error;
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let service: host::Service = Arc::new(|job| {
        Box::pin(async move {
            match job.audio_path.to_str().unwrap_or("") {
                "slow" => {
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    Ok("must never escape".into())
                }
                "fail" if !job.is_retry => Err(Error::Network),
                "fail" if job.language.as_deref() == Some("zh") => {
                    Ok("retried same path and language".into())
                }
                "cancel" => Err(Error::Cancelled),
                "large" => Ok("x".repeat(2_000_000)),
                "pressure" => Ok("x".repeat(900_000)),
                _ => Ok("原文\n\r\t\0\"保留".into()),
            }
        })
    });
    let result = host::run(
        std::io::stdin(),
        std::io::stdout(),
        service,
        Duration::from_millis(150),
    )
    .await;
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}
