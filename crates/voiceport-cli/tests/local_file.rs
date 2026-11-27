use serde_json::Value;
use std::{path::Path, process::Command};

const MODEL: &str = "orca:whisper-tiny";

fn invoke(binary: &str, root: &Path, args: &[&str], expect: i32) -> Value {
    let output = Command::new(binary)
        .current_dir(root)
        .args(["--json"])
        .args(args)
        .env("HOME", root.join("absent-home"))
        .env("CODEX_HOME", root.join("absent-auth"))
        .env("VOICEPORT_MODELS_PATH", root.join("models"))
        .env("VOICEPORT_CONTROL_SOCKET", "unused-relative-socket")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(expect), "{output:?}");
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["schema_version"], 1);
    assert!(!root.join("absent-home").exists());
    assert!(!root.join("absent-auth").exists());
    response
}

#[test]
fn local_file_has_offline_help_and_never_accepts_app_or_cloud_options() {
    let temp = tempfile::tempdir().unwrap();
    for binary in [
        env!("CARGO_BIN_EXE_voiceport"),
        env!("CARGO_BIN_EXE_voiceport-rs"),
    ] {
        let help = invoke(binary, temp.path(), &["transcribe-local", "--help"], 0);
        let text = help["data"]["text"].as_str().unwrap();
        assert!(text.contains(MODEL));
        assert!(text.contains("WAV, M4A and MP3"));
        let help = invoke(binary, temp.path(), &["skills", "get", "cli"], 0);
        assert!(
            help["data"]["text"]
                .as_str()
                .unwrap()
                .contains("transcribe-local")
        );
        for args in [
            vec!["transcribe-local", "input.wav"],
            vec![
                "transcribe-local",
                "input.wav",
                "--model",
                MODEL,
                "--model",
                MODEL,
            ],
            vec![
                "--socket",
                "/invalid.sock",
                "transcribe-local",
                "input.wav",
                "--model",
                MODEL,
            ],
            vec![
                "transcribe-local",
                "input.wav",
                "--model",
                MODEL,
                "--accept-upload",
            ],
            vec![
                "transcribe-local",
                "input.wav",
                "--model",
                MODEL,
                "--auth",
                "unused",
            ],
        ] {
            let response = invoke(binary, temp.path(), &args, 2);
            assert_eq!(response["error"]["code"], "invalid_arguments");
            assert!(response["data"].is_null());
        }
    }
}

#[test]
fn local_file_reports_the_platform_or_explicit_model_error_before_ipc() {
    let temp = tempfile::tempdir().unwrap();
    let binary = env!("CARGO_BIN_EXE_voiceport");
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        for (model, exit, code) in [
            ("chatgpt", 6, "not_found"),
            (MODEL, 1, "source_unavailable"),
        ] {
            let response = invoke(
                binary,
                temp.path(),
                &["transcribe-local", "input.wav", "--model", model],
                exit,
            );
            assert_eq!(response["error"]["code"], code);
            if exit == 1 {
                assert!(
                    response["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("安装或修复")
                );
            }
        }
        let response = invoke(
            binary,
            temp.path(),
            &[
                "transcribe-local",
                "input.wav",
                "--model",
                "orca:paraformer-bilingual-zh-en",
                "--language",
                "ja",
            ],
            1,
        );
        assert_eq!(response["error"]["code"], "unsupported_language");
        assert!(!temp.path().join("models").exists());
    } else {
        let response = invoke(
            binary,
            temp.path(),
            &["transcribe-local", "input.wav", "--model", MODEL],
            7,
        );
        assert_eq!(response["error"]["code"], "unsupported");
    }
}
