use serde_json::{Value, json};
use std::process::{Command, Stdio};

fn binaries() -> [&'static str; 2] {
    [
        env!("CARGO_BIN_EXE_voiceport"),
        env!("CARGO_BIN_EXE_voiceport-rs"),
    ]
}

#[test]
fn offline_help_and_invalid_arguments_never_connect() {
    for bin in binaries() {
        for (group, usage) in [
            ("app", "app open files"),
            ("preferences", "preferences set language"),
            ("models", "models refresh"),
            ("microphone", "microphone test-start"),
            ("dictionary", "dictionary set"),
            ("scenes", "scenes apply"),
            ("history", "history clear"),
            ("dictation", "dictation paste-last"),
            ("file", "file export"),
        ] {
            let output = Command::new(bin)
                .args([
                    "--json",
                    "--socket",
                    "intentionally-invalid",
                    group,
                    "--help",
                ])
                .output()
                .unwrap();
            assert!(output.status.success(), "{group}: {:?}", output.status);
            let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(reply["data"]["text"].as_str().unwrap().contains(usage));
        }
        for args in [
            vec!["app", "update", "--help"],
            vec!["help", "app", "update", "check"],
            vec!["app", "update", "status", "-h"],
        ] {
            let output = Command::new(bin)
                .args(["--json", "--socket", "intentionally-invalid"])
                .args(args)
                .output()
                .unwrap();
            assert!(output.status.success());
            let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
            let help = reply["data"]["text"].as_str().unwrap();
            assert!(help.contains("app update check"));
            assert!(help.contains("app update status"));
        }
        for args in [
            vec!["app", "update", "install"],
            vec!["app", "update", "check", "--confirm"],
            vec!["app", "open", "arbitrary-url"],
            vec!["file", "plan", "relative.wav", "--source", "chatgpt"],
            vec!["history", "list", "--limit", "0"],
            vec!["dictation", "copy-last", "--confirm"],
            vec!["dictation", "paste-last"],
            vec![
                "dictation",
                "paste-last",
                "--target-pid",
                "0",
                "--target-bundle",
                "synthetic.editor",
            ],
            vec!["microphone", "select", "--device", ""],
            vec![
                "file",
                "export",
                "id",
                "--output",
                "/synthetic.txt",
                "--format",
                "html",
            ],
            vec!["preferences", "set", "recording-mode", "arbitrary"],
        ] {
            let output = Command::new(bin)
                .args(["--json", "--socket", "intentionally-invalid"])
                .args(&args)
                .output()
                .unwrap();
            let reply: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(output.status.code(), Some(2), "{args:?}: {reply}");
            assert_eq!(reply["error"]["code"], "invalid_arguments");
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn real_binaries_and_transport_carry_product_requests_and_mock_receipts() {
    use std::{
        io::Write,
        time::{Duration, Instant},
    };
    use voiceport_control::{Response, transport::Server};

    struct Example {
        args: &'static [&'static str],
        wire: Value,
        stdin: Option<&'static str>,
    }
    fn example(args: &'static [&'static str], command: &str, request: Value) -> Example {
        Example {
            args,
            wire: json!({"command": command, "request": request}),
            stdin: None,
        }
    }
    let examples = [
        Example {
            args: &["app", "capabilities"],
            wire: json!({"command":"capabilities"}),
            stdin: None,
        },
        Example {
            args: &["app", "open", "source-tests"],
            wire: json!({"command":"open", "page":"source_tests"}),
            stdin: None,
        },
        Example {
            args: &["app", "open", "quick-panel"],
            wire: json!({"command":"open", "page":"quick_panel"}),
            stdin: None,
        },
        Example {
            args: &["app", "open", "files"],
            wire: json!({"command":"open", "page":"file"}),
            stdin: None,
        },
        Example {
            args: &["app", "open", "file"],
            wire: json!({"command":"open", "page":"file"}),
            stdin: None,
        },
        Example {
            args: &["file", "open"],
            wire: json!({"command":"open", "page":"file"}),
            stdin: None,
        },
        Example {
            args: &["sources", "destination", "fixture"],
            wire: json!({"command":"sources_destination", "id":"fixture"}),
            stdin: None,
        },
        Example {
            args: &["models", "list"],
            wire: json!({"command":"models_list"}),
            stdin: None,
        },
        Example {
            args: &["models", "refresh"],
            wire: json!({"command":"models_refresh"}),
            stdin: None,
        },
        example(
            &["models", "details", "test-model"],
            "models",
            json!({"action":"details","id":"test-model"}),
        ),
        example(
            &["models", "download", "test-model"],
            "models",
            json!({"action":"plan","mutation":{"action":"download","id":"test-model"}}),
        ),
        example(
            &["models", "update", "test-model"],
            "models",
            json!({"action":"plan","mutation":{"action":"update","id":"test-model"}}),
        ),
        example(
            &["models", "import", "test-model", "/synthetic fixture"],
            "models",
            json!({"action":"plan","mutation":{"action":"import","id":"test-model","directory":"/synthetic fixture"}}),
        ),
        example(
            &["models", "remove", "test-model"],
            "models",
            json!({"action":"plan","mutation":{"action":"remove","id":"test-model"}}),
        ),
        example(
            &["models", "install-engine"],
            "models",
            json!({"action":"plan","mutation":{"action":"install_engine"}}),
        ),
        example(
            &["models", "start", "fixture-op", "--confirm"],
            "models",
            json!({"action":"start","operation":"fixture-op","confirm":true}),
        ),
        example(
            &["models", "status", "fixture-op"],
            "models",
            json!({"action":"status","operation":"fixture-op"}),
        ),
        example(
            &["models", "cancel", "fixture-op"],
            "models",
            json!({"action":"cancel","operation":"fixture-op"}),
        ),
        Example {
            args: &["dictation", "copy-last"],
            wire: json!({"command":"copy_last_dictation"}),
            stdin: None,
        },
        Example {
            args: &[
                "dictation",
                "paste-last",
                "--target-bundle",
                "synthetic.editor",
                "--target-pid",
                "123",
            ],
            wire: json!({"command":"paste_last_dictation", "target_pid":123, "target_bundle":"synthetic.editor"}),
            stdin: None,
        },
        Example {
            args: &["dictation", "gui-result", "8", "--original"],
            wire: json!({"command":"gui_result", "generation":8, "original":true}),
            stdin: None,
        },
        example(
            &["preferences", "get"],
            "preferences",
            json!({"action":"get"}),
        ),
        example(
            &["preferences", "set", "language", "auto"],
            "preferences",
            json!({"action":"language", "language":""}),
        ),
        example(
            &["preferences", "set", "automatic-input", "off"],
            "preferences",
            json!({"action":"automatic_input", "enabled":false}),
        ),
        example(
            &["preferences", "set", "status-overlay", "on"],
            "preferences",
            json!({"action":"status_overlay", "enabled":true}),
        ),
        example(
            &["preferences", "set", "recording-mode", "hold"],
            "preferences",
            json!({"action":"recording_mode", "mode":"hold"}),
        ),
        example(
            &["microphone", "list"],
            "microphone",
            json!({"action":"list"}),
        ),
        example(
            &["microphone", "status"],
            "microphone",
            json!({"action":"status"}),
        ),
        example(
            &["microphone", "select", "--system"],
            "microphone",
            json!({"action":"select", "uid":null}),
        ),
        example(
            &["microphone", "select", "--device", "fixture"],
            "microphone",
            json!({"action":"select", "uid":"fixture"}),
        ),
        example(
            &["microphone", "test-start", "--confirm"],
            "microphone",
            json!({"action":"test_start", "confirm":true}),
        ),
        example(
            &["microphone", "test-stop", "fixture"],
            "microphone",
            json!({"action":"test_stop", "operation":"fixture"}),
        ),
        example(
            &["dictionary", "show"],
            "dictionary",
            json!({"action":"show"}),
        ),
        Example {
            stdin: Some("原 → 合成\n"),
            ..example(
                &["dictionary", "set", "on", "--stdin"],
                "dictionary",
                json!({"action":"set", "enabled":true, "rules":"原 → 合成\n"}),
            )
        },
        Example {
            stdin: Some("合成"),
            ..example(
                &["dictionary", "preview", "--stdin"],
                "dictionary",
                json!({"action":"preview", "text":"合成"}),
            )
        },
        example(&["scenes", "list"], "scenes", json!({"action":"list"})),
        example(
            &[
                "scenes",
                "add",
                "lab",
                "--source",
                "chatgpt",
                "--language",
                "zh",
                "--dictionary",
                "on",
            ],
            "scenes",
            json!({"action":"add", "name":"lab", "source":"chatgpt", "language":"zh", "dictionary_enabled":true}),
        ),
        example(
            &["scenes", "remove", "lab", "--confirm"],
            "scenes",
            json!({"action":"remove", "name":"lab", "confirm":true}),
        ),
        example(
            &["scenes", "apply", "lab"],
            "scenes",
            json!({"action":"apply", "name":"lab"}),
        ),
        example(
            &["history", "list", "--limit", "2"],
            "history",
            json!({"action":"list", "limit":2}),
        ),
        example(
            &["history", "show", "fixture-entry"],
            "history",
            json!({"action":"show", "id":"fixture-entry"}),
        ),
        example(
            &["history", "clear"],
            "history",
            json!({"action":"clear", "confirm":false}),
        ),
        example(
            &["history", "clear", "--confirm"],
            "history",
            json!({"action":"clear", "confirm":true}),
        ),
        example(
            &["history", "settings"],
            "history",
            json!({"action":"settings", "enabled":null, "confirm":false}),
        ),
        example(
            &["history", "settings", "on"],
            "history",
            json!({"action":"settings", "enabled":true, "confirm":false}),
        ),
        example(
            &["history", "settings", "off", "--confirm"],
            "history",
            json!({"action":"settings", "enabled":false, "confirm":true}),
        ),
        example(
            &["file", "sources", "--language", "zh"],
            "file",
            json!({"action":"sources", "language":"zh"}),
        ),
        example(
            &[
                "file",
                "plan",
                "/synthetic input.wav",
                "--source",
                "fixture",
            ],
            "file",
            json!({"action":"plan", "path":"/synthetic input.wav", "source":"fixture", "language":""}),
        ),
        example(
            &["file", "start", "fixture-op"],
            "file",
            json!({"action":"start", "operation":"fixture-op", "confirm":false}),
        ),
        example(
            &["file", "start", "fixture-op", "--confirm"],
            "file",
            json!({"action":"start", "operation":"fixture-op", "confirm":true}),
        ),
        example(
            &["file", "status", "fixture-op"],
            "file",
            json!({"action":"status", "operation":"fixture-op"}),
        ),
        example(
            &["file", "result", "fixture-op"],
            "file",
            json!({"action":"result", "operation":"fixture-op"}),
        ),
        example(
            &["file", "cancel", "fixture-op"],
            "file",
            json!({"action":"cancel", "operation":"fixture-op"}),
        ),
        example(
            &[
                "file",
                "export",
                "fixture-op",
                "--output",
                "/synthetic result.md",
                "--format",
                "markdown",
            ],
            "file",
            json!({"action":"export", "operation":"fixture-op", "path":"/synthetic result.md", "format":"markdown"}),
        ),
    ];
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let socket = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("private/control.sock");
    let server = Server::bind(&socket).unwrap();
    for bin in binaries() {
        for example in &examples {
            let mut child = Command::new(bin)
                .args(["--json", "--socket", socket.to_str().unwrap()])
                .args(example.args)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            if let Some(text) = example.stdin {
                child
                    .stdin
                    .take()
                    .unwrap()
                    .write_all(text.as_bytes())
                    .unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(3);
            let pending = loop {
                if let Some(request) = server.next() {
                    break request;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let output = child.wait_with_output().unwrap();
                    panic!(
                        "No IPC request for {:?}: {}",
                        example.args,
                        String::from_utf8_lossy(&output.stdout)
                    );
                }
                std::thread::sleep(Duration::from_millis(2));
            };
            assert_eq!(
                serde_json::to_value(&pending.request.action).unwrap(),
                example.wire,
                "{:?}",
                example.args
            );
            let response = match example.args {
                ["history", "clear"] | ["file", "start", _] => {
                    Response::error("confirmation_required", "synthetic confirmation")
                }
                ["file", "cancel", _] => Response::error("cancelled", "synthetic cancellation"),
                _ => Response::ok(json!({"fixture":true})),
            };
            let expected = serde_json::to_value(&response).unwrap();
            let code = i32::from(response.exit_code());
            pending.respond(response);
            let output = child.wait_with_output().unwrap();
            assert_eq!(output.status.code(), Some(code));
            assert!(output.stderr.is_empty());
            assert_eq!(
                serde_json::from_slice::<Value>(&output.stdout).unwrap(),
                expected
            );
        }
    }
}
