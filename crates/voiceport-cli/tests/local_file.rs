use serde_json::Value;
use std::{path::Path, process::Command};

const MODEL: &str = "orca:whisper-tiny";

fn invoke(binary: &str, root: &Path, args: &[&str], expect: i32) -> Value {
    invoke_with_models(binary, root, &root.join("models"), args, expect)
}

fn invoke_with_models(
    binary: &str,
    root: &Path,
    models: &Path,
    args: &[&str],
    expect: i32,
) -> Value {
    let output = Command::new(binary)
        .current_dir(root)
        .args(["--json"])
        .args(args)
        .env("HOME", root.join("absent-home"))
        .env("CODEX_HOME", root.join("absent-auth"))
        .env("VOICEPORT_MODELS_PATH", models)
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

#[test]
fn local_batch_checks_arguments_and_shared_setup_before_any_file() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let native = cfg!(all(target_os = "macos", target_arch = "aarch64"));
    for binary in [
        env!("CARGO_BIN_EXE_voiceport"),
        env!("CARGO_BIN_EXE_voiceport-rs"),
    ] {
        let help = invoke(binary, root, &["transcribe-local", "--help"], 0);
        assert!(
            help["data"]["text"]
                .as_str()
                .unwrap()
                .contains("data.results")
        );
        for (options, exit, code) in [
            (vec![], 2, "invalid_arguments"),
            (vec!["--model", MODEL, "--language"], 2, "invalid_arguments"),
            (
                vec!["--model", MODEL, "--language", "en", "--language", "en"],
                2,
                "invalid_arguments",
            ),
            (
                vec!["--model", MODEL, "--socket", "/unused.sock"],
                2,
                "invalid_arguments",
            ),
            (
                vec!["--model", MODEL, "--accept-upload"],
                2,
                "invalid_arguments",
            ),
            (vec!["--model", MODEL], 1, "source_unavailable"),
            (vec!["--model", "chatgpt"], 6, "not_found"),
            (
                vec!["--model", MODEL, "--language", "bogus"],
                1,
                "unsupported_language",
            ),
        ] {
            let (exit, code) = if !native && exit != 2 {
                (7, "unsupported")
            } else {
                (exit, code)
            };
            let mut args = vec!["transcribe-local", "absent.wav", "absent.mp3"];
            args.extend(options);
            let response = invoke(binary, root, &args, exit);
            assert_eq!(response["error"]["code"], code);
            assert!(response["data"].is_null());
        }
        let args = [
            "transcribe-local",
            "--model",
            MODEL,
            "--",
            "first.wav",
            "--named.wav",
        ];
        let (exit, code) = if native {
            (2, "invalid_config")
        } else {
            (7, "unsupported")
        };
        let response =
            invoke_with_models(binary, root, Path::new("relative-models-path"), &args, exit);
        assert_eq!(response["error"]["code"], code);
        assert!(response["data"].is_null());

        for count in [0, 16, 17] {
            let mut args = vec!["transcribe-local", "--model", "chatgpt", "--"];
            args.extend(std::iter::repeat_n("absent.wav", count));
            let (exit, code) = match (count, native) {
                (16, true) => (6, "not_found"),
                (16, false) => (7, "unsupported"),
                _ => (2, "invalid_arguments"),
            };
            let response = invoke(binary, root, &args, exit);
            assert_eq!(response["error"]["code"], code);
            assert!(response["data"].is_null());
        }
    }
    assert!(!root.join("models").exists());
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[test]
fn local_batch_preflights_engine_integrity_without_changing_single_file_precedence() {
    use std::fs;
    use voiceport_macos::model_library::MODELS;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let definition = MODELS.iter().find(|model| model.id == MODEL).unwrap();
    let installed_model = root.join("models").join(definition.directory);
    fs::create_dir_all(&installed_model).unwrap();
    for asset in definition.assets {
        fs::File::create(installed_model.join(asset.name))
            .unwrap()
            .set_len(asset.bytes)
            .unwrap();
    }
    let runtime = root.join("models/runtime-2.0.1");
    fs::create_dir(&runtime).unwrap();
    fs::write(runtime.join("package.json"), br#"{"version":"2.0.1"}"#).unwrap();
    for name in ["libonnxruntime.dylib", "libsherpa-onnx-c-api.dylib"] {
        // Pass the catalogue's arm64 header check but not the pinned release check.
        fs::write(runtime.join(name), [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 1]).unwrap();
    }

    let binary = env!("CARGO_BIN_EXE_voiceport");
    let single = invoke(
        binary,
        root,
        &["transcribe-local", "absent.wav", "--model", MODEL],
        1,
    );
    assert_eq!(single["error"]["code"], "invalid_file");
    assert!(single["data"].is_null());
    let batch = invoke(
        binary,
        root,
        &[
            "transcribe-local",
            "absent.wav",
            "later.wav",
            "--model",
            MODEL,
        ],
        1,
    );
    assert_eq!(batch["error"]["code"], "source_unavailable");
    assert!(
        batch["error"]["message"]
            .as_str()
            .unwrap()
            .contains("验证或暂存")
    );
    assert!(batch["data"].is_null());
}
