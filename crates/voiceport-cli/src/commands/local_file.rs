use super::invalid;
use std::path::PathBuf;
use voiceport_control::Response;

const MAX_FILES: usize = 16;

pub(super) async fn execute(args: &[String]) -> Result<Response, Response> {
    let (mut model, mut language) = (None, None);
    let mut paths = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let option = match arg.as_str() {
            "--model" if model.is_none() => &mut model,
            "--language" if language.is_none() => &mut language,
            "--" => {
                paths.extend(args.map(PathBuf::from));
                break;
            }
            path if is_value(path) => {
                paths.push(PathBuf::from(path));
                continue;
            }
            _ => return Err(invalid()),
        };
        *option = Some(
            args.next()
                .filter(|value| is_value(value))
                .ok_or_else(invalid)?
                .clone(),
        );
    }
    if !(1..=MAX_FILES).contains(&paths.len())
        || paths.iter().any(|path| path.as_os_str().is_empty())
    {
        return Err(invalid());
    }
    let model = model.ok_or_else(invalid)?;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        let language = language.unwrap_or_else(|| "auto".into());
        run_cancellable(move |cancel| {
            let mut data = serde_json::json!({
                "source": model,
                "language": language,
                "destination": {"kind": "on_device"},
                "delivery": "none",
            });
            if let [path] = paths.as_slice() {
                let text =
                    voiceport_macos::transcribe_local_file(path, &model, &language, &cancel)?;
                data["text"] = text.into();
                return Ok(Response::ok(data));
            }
            let results =
                voiceport_macos::transcribe_local_batch(&paths, &model, &language, &cancel)?;
            batch_response(data, &paths, results)
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
fn batch_response(
    mut data: serde_json::Value,
    paths: &[PathBuf],
    results: Vec<Result<String, voiceport_control::Failure>>,
) -> Result<Response, Response> {
    let failed = results.iter().any(Result::is_err);
    data["results"] = paths
        .iter()
        .zip(results)
        .map(|(path, result)| match result {
            Ok(text) => serde_json::json!({"path": path, "text": text}),
            Err(error) => serde_json::json!({"path": path, "error": error}),
        })
        .collect();
    if failed {
        let mut response = Response::error("batch_failed", "一个或多个文件未能完成转写。");
        response.data = data;
        Err(response)
    } else {
        Ok(Response::ok(data))
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
async fn run_cancellable(
    operation: impl FnOnce(voiceport_core::CancellationToken) -> Result<Response, Response>
    + Send
    + 'static,
) -> Result<Response, Response> {
    use tokio::signal::unix::{SignalKind, signal};
    // Install the listener before the blocking worker can prepare or open files.
    let mut interrupt = signal(SignalKind::interrupt())
        .map_err(|_| Response::error("transcription_error", "无法处理本地文件转写的中断信号。"))?;
    let cancel = voiceport_core::CancellationToken::new();
    let worker_cancel = cancel.clone();
    let mut worker = tokio::task::spawn_blocking(move || operation(worker_cancel));
    tokio::select! {
        biased;
        _ = interrupt.recv() => {
            cancel.cancel();
            // Keep the CLI alive until the native call and its private files are released.
            let _ = worker.await;
            Err(Response::error("cancelled", "文件转写已取消。"))
        }
        result = &mut worker => result.map_err(|_| {
            Response::error("transcription_error", "本地文件转写未能完成。")
        })?,
    }
}

#[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn batch_reports_both_success_and_failure_in_one_response() {
        let paths = ["first.wav", "bad.wav", "first.wav"].map(PathBuf::from);
        let base = json!({"delivery": "none"});
        let mixed = batch_response(
            base.clone(),
            &paths,
            vec![
                Ok("one".into()),
                Err(voiceport_control::Failure {
                    code: "invalid_audio".into(),
                    message: "bad fixture".into(),
                }),
                Ok("one".into()),
            ],
        )
        .unwrap_err();
        assert_eq!(mixed.exit_code(), 1);
        assert_eq!(mixed.error.unwrap().code, "batch_failed");
        assert_eq!(
            mixed.data,
            json!({"delivery": "none", "results": [
                {"path": "first.wav", "text": "one"},
                {"path": "bad.wav", "error": {"code": "invalid_audio", "message": "bad fixture"}},
                {"path": "first.wav", "text": "one"},
            ]})
        );
        let success = batch_response(base, &paths, vec![Ok("ok".into()); paths.len()]).unwrap();
        assert_eq!(success.exit_code(), 0);
        assert_eq!(
            success.data["results"].as_array().unwrap().len(),
            paths.len()
        );
    }

    #[tokio::test]
    async fn sigint_waits_for_the_worker_and_discards_its_output() {
        use std::{process::Command, time::Duration};
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (cleaned_tx, cleaned_rx) = std::sync::mpsc::channel();
        let operation = run_cancellable(move |cancel| {
            started_tx.send(()).unwrap();
            tokio::runtime::Handle::current().block_on(async {
                tokio::time::timeout(Duration::from_secs(5), cancel.cancelled())
                    .await
                    .unwrap();
            });
            std::thread::sleep(Duration::from_millis(20)); // stand in for native cleanup
            cleaned_tx.send(()).unwrap();
            Ok(Response::ok(json!({"text": "must be discarded"})))
        });
        let interrupt = async {
            started_rx.await.unwrap();
            assert!(
                Command::new("/bin/kill")
                    .args(["-INT", &std::process::id().to_string()])
                    .status()
                    .unwrap()
                    .success()
            );
        };
        let (result, ()) = tokio::join!(operation, interrupt);
        let response = result.unwrap_err();
        assert_eq!(response.exit_code(), 130);
        assert!(response.data.is_null());
        cleaned_rx.try_recv().unwrap();
    }
}
