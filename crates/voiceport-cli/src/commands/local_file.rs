use super::invalid;
use voiceport_control::Response;

pub(super) async fn execute(args: &[String]) -> Result<Response, Response> {
    let (path, options) = args.split_first().ok_or_else(invalid)?;
    if !is_value(path) {
        return Err(invalid());
    }
    let mut model = None;
    let mut language = None;
    for pair in options.chunks(2) {
        match pair {
            [option, value] if is_value(value) => match option.as_str() {
                "--model" if model.is_none() => model = Some(value.clone()),
                "--language" if language.is_none() => language = Some(value.clone()),
                _ => return Err(invalid()),
            },
            _ => return Err(invalid()),
        }
    }
    let model = model.ok_or_else(invalid)?;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let path = std::path::PathBuf::from(path);
        let language = language.unwrap_or_else(|| "auto".into());
        run_cancellable(move |cancel| {
            let text = voiceport_macos::transcribe_local_file(&path, &model, &language, &cancel)?;
            Ok(Response::ok(serde_json::json!({
                "text": text,
                "source": model,
                "language": language,
                "destination": {"kind": "on_device"},
                "delivery": "none",
            })))
        })
        .await
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (model, language);
        Err(Response::error(
            "unsupported",
            "Local file transcription requires an Apple Silicon macOS CLI.",
        ))
    }
}

fn is_value(value: &str) -> bool {
    !value.is_empty() && !value.starts_with("--")
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
async fn run_cancellable(
    operation: impl FnOnce(voiceport_core::CancellationToken) -> Result<Response, Response>
    + Send
    + 'static,
) -> Result<Response, Response> {
    let cancel = voiceport_core::CancellationToken::new();
    let worker_cancel = cancel.clone();
    let mut worker = tokio::task::spawn_blocking(move || operation(worker_cancel));
    tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => {
            cancel.cancel();
            // Keep the CLI alive until the native call and its private file are released.
            let _ = worker.await;
            if signal.is_ok() {
                Err(Response::error("cancelled", "文件转写已取消。"))
            } else {
                Err(Response::error("transcription_error", "无法处理本地文件转写的中断信号。"))
            }
        }
        result = &mut worker => result.map_err(|_| {
            Response::error("transcription_error", "本地文件转写未能完成。")
        })?,
    }
}
