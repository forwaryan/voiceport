use std::process::{Command, Output};

fn run(binary: &str, args: &[&str]) -> Output {
    Command::new(binary).args(args).output().unwrap()
}

#[test]
fn invalid_qq_commands_fail_before_diagnostics_or_ipc() {
    for binary in [
        env!("CARGO_BIN_EXE_voiceport"),
        env!("CARGO_BIN_EXE_voiceport-rs"),
    ] {
        for args in [
            vec!["--json", "qq", "doctor", "--qq", "relative/QQ.app"],
            vec!["--json", "qq", "doctor", "--accept-upload"],
            vec!["--json", "qq", "connect"],
            vec!["--json", "--socket", "/absent/control.sock", "qq", "doctor"],
            vec!["--json", "qq", "doctor", "--qq"],
        ] {
            let output = run(binary, &args);
            assert_eq!(output.status.code(), Some(2), "{output:?}");
            let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(result["error"]["code"], "invalid_arguments");
            assert!(output.stderr.is_empty());
        }
    }
}

#[test]
fn embedded_doctor_runs_without_checkout_or_app_and_never_reports_connection() {
    let scope = tempfile::tempdir().unwrap();
    let absent = scope.path().join("missing QQ.app");
    let absent = absent.to_str().unwrap();
    for binary in [
        env!("CARGO_BIN_EXE_voiceport"),
        env!("CARGO_BIN_EXE_voiceport-rs"),
    ] {
        let output = Command::new(binary)
            .args(["--json", "qq", "doctor", "--qq", absent])
            .current_dir(scope.path())
            .env("VOICEPORT_CONTROL_SOCKET", "invalid-unused-socket")
            .env("PYTHONPATH", "/invalid-unused-import-directory")
            .output()
            .unwrap();
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        #[cfg(target_os = "macos")]
        {
            assert_eq!(output.status.code(), Some(0), "{output:?}");
            assert_eq!(result["status"], "ok");
            let report = &result["data"];
            assert_eq!(report["diagnostic_only"], true);
            assert_eq!(report["connection_status"], "not_tested");
            assert_eq!(report["transcription_verified"], false);
            assert_eq!(report["automatic_setup_supported"], false);
            assert!(!report["blockers"].as_array().unwrap().is_empty());
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(output.status.code(), Some(7), "{output:?}");
            assert_eq!(result["error"]["code"], "unsupported");
        }
        assert!(output.stderr.is_empty(), "{output:?}");
        assert!(!std::path::Path::new(absent).exists());
    }
}
