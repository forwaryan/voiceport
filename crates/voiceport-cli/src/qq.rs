//! Offline installation diagnostics using the version-matched embedded doctor.
//! No App IPC, QQ launch, account access or runtime/bootstrap modes are exposed.
#[cfg(any(target_os = "macos", test))]
use serde_json::Value;
use voiceport_control::Response;

pub fn doctor(app: Option<&str>) -> Result<Response, Response> {
    if app.is_some_and(|path| !std::path::Path::new(path).is_absolute()) {
        return Err(Response::error(
            "invalid_arguments",
            "QQ 安装路径必须是绝对路径。用 qq doctor 自动查找，或 qq doctor --qq /Applications/QQ.app。",
        ));
    }
    #[cfg(target_os = "macos")]
    {
        run_doctor(app).map(Response::ok)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err(Response::error(
            "unsupported",
            "QQ 安装诊断目前仅支持 macOS；其他平台的真实 QQ 连接尚未验证。",
        ))
    }
}

#[cfg(target_os = "macos")]
fn run_doctor(app: Option<&str>) -> Result<Value, Response> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const LIMIT: u64 = 65_536;
    // Isolated Python ignores cwd/PYTHON* imports. The script is compiled into
    // the CLI, so neither a checkout nor a mutable adjacent helper is required.
    // Give the module a non-main name: none of its other CLI modes may run.
    let script = format!(
        "__file__ = '/voiceport-embedded-qq-doctor.py'\n__name__ = 'voiceport_qq_doctor'\n{}\nprint(json.dumps(doctor(Path(sys.argv[1]) if len(sys.argv) == 2 else None), ensure_ascii=False))\n",
        include_str!("../../../scripts/qq_bridge/diagnose.py")
    );
    let mut command = Command::new("/usr/bin/python3");
    command.args(["-I", "-B", "-c", &script]);
    if let Some(app) = app {
        command.arg(app);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| {
            Response::error(
                "diagnostic_unavailable",
                "无法运行 QQ 安装诊断。此命令需要可用的 /usr/bin/python3；未启动 QQ 或测试转写。",
            )
        })?;
    let deadline = Instant::now() + Duration::from_secs(45);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => {
                let timed_out = result.is_ok();
                let _ = child.kill();
                let _ = child.wait();
                return Err(Response::error(
                    if timed_out {
                        "diagnostic_timeout"
                    } else {
                        "diagnostic_failed"
                    },
                    "QQ 安装诊断未完成；未启动 QQ 或测试转写。请检查安装后重新执行诊断。",
                ));
            }
        }
    };
    let mut bytes = Vec::new();
    let read = child.stdout.take().ok_or(()).and_then(|stdout| {
        stdout
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| ())
    });
    if !status.success() || read.is_err() || bytes.len() > LIMIT as usize {
        return Err(diagnostic_failed());
    }
    parse_report(&bytes)
}

#[cfg(any(target_os = "macos", test))]
fn diagnostic_failed() -> Response {
    Response::error(
        "diagnostic_failed",
        "无法确认 QQ 安装检查结果；未启动 QQ 或测试转写。请检查安装后重新执行诊断。",
    )
}

#[cfg(any(target_os = "macos", test))]
fn parse_report(bytes: &[u8]) -> Result<Value, Response> {
    let report: Value = serde_json::from_slice(bytes).map_err(|_| diagnostic_failed())?;
    if report.get("schema_version") != Some(&Value::from(1))
        || report.get("diagnostic_only") != Some(&Value::Bool(true))
        || report.get("connection_status") != Some(&Value::from("not_tested"))
        || report.get("transcription_verified") != Some(&Value::Bool(false))
        || report.get("automatic_setup_supported") != Some(&Value::Bool(false))
        || !report.get("checks").is_some_and(Value::is_array)
        || !report.get("blockers").is_some_and(Value::is_array)
    {
        return Err(diagnostic_failed());
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_runtime_success_claims_and_raw_helper_errors() {
        let mut report = serde_json::json!({
            "schema_version":1,"diagnostic_only":true,"connection_status":"not_tested",
            "transcription_verified":false,"automatic_setup_supported":false,
            "checks":[],"blockers":[]
        });
        assert!(parse_report(&serde_json::to_vec(&report).unwrap()).is_ok());
        report["transcription_verified"] = Value::Bool(true);
        assert!(parse_report(&serde_json::to_vec(&report).unwrap()).is_err());
        let error = parse_report(b"PRIVATE_TRACEBACK_TOKEN").unwrap_err();
        assert!(
            !serde_json::to_string(&error)
                .unwrap()
                .contains("PRIVATE_TRACEBACK_TOKEN")
        );
    }
}
