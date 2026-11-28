use std::process::{Command, Stdio};
fn json(args: &[&str], exit: i32) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_voiceport"))
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(exit),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "JSON failures must be structured and must not echo secrets"
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["schema_version"], 1);
    response
}
#[test]
fn bundled_guides_equal_repository_single_source_offline() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let guides = [
        ("core", "skills/voiceport/SKILL.md"),
        ("cli", "skills/voiceport/references/cli.md"),
        ("workflows", "skills/voiceport/references/workflows.md"),
        ("plugins", "skills/voiceport-plugins/SKILL.md"),
        (
            "plugin-examples",
            "skills/voiceport-plugins/references/examples.md",
        ),
        ("plugin-protocol", "docs/TRANSCRIPTION_PLUGINS.md"),
        (
            "plugin-template",
            "skills/voiceport-plugins/templates/json_http_adapter.py",
        ),
    ];
    for (name, file) in guides {
        let response = json(&["--json", "skills", "get", name], 0);
        assert_eq!(
            response["data"]["text"].as_str().unwrap(),
            std::fs::read_to_string(root.join(file)).unwrap()
        );
    }
    assert_eq!(
        json(&["--json", "--version"], 0)["data"]["control_schema"],
        1
    );
    assert_eq!(
        json(&["--json", "skills", "list"], 0)["data"]["skills"],
        serde_json::json!(guides.map(|(name, _)| name))
    );
    assert_eq!(
        json(&["--json", "skills", "get"], 0)["data"]["text"],
        std::fs::read_to_string(root.join("skills/voiceport/SKILL.md")).unwrap()
    );
}
#[test]
fn invalid_automation_never_starts_app_or_records() {
    for args in [
        vec!["--json", "dictation", "start"],
        vec!["--json", "status", "--unknown"],
        vec!["--json", "sources", "add", "id"],
        vec!["--json", "plugins", "import"],
        vec!["--json", "--json", "status"],
        vec!["--json", "--socket"],
        vec!["--json", "skills", "get", "unknown"],
        vec!["--json", "skills", "get", "plugins", "extra"],
    ] {
        assert_eq!(json(&args, 2)["error"]["code"], "invalid_arguments");
    }
    assert_eq!(
        json(
            &["--json", "sources", "secret", "id", "api_key", "--stdin"],
            1
        )["error"]["code"],
        "missing_secret"
    );
}
#[test]
fn contextual_help_is_offline_for_both_entrypoints() {
    for args in [
        vec!["help", "sources"],
        vec!["sources", "--help"],
        vec!["dictation", "start", "-h"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_voiceport"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains("voiceport"));
        assert!(output.stderr.is_empty());
    }
    for binary in [
        env!("CARGO_BIN_EXE_voiceport"),
        env!("CARGO_BIN_EXE_voiceport-rs"),
    ] {
        for path in [
            vec!["sources", "--help"],
            vec!["help", "dictation"],
            vec!["dictation", "retry", "--help"],
            vec!["dictation", "fragment", "--help"],
            vec!["dictation", "transcribe-fragment", "--help"],
            vec!["qq", "doctor", "-h"],
            vec!["sources", "probe", "--help"],
            vec!["plugins", "import", "--help"],
        ] {
            let output = Command::new(binary)
                .arg("--json")
                .arg("--socket")
                .arg("relative-unused-socket")
                .args(path)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            assert!(output.stderr.is_empty());
            let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            assert!(
                result["data"]["text"]
                    .as_str()
                    .unwrap()
                    .contains("voiceport")
            );
        }
    }
}
#[test]
fn probe_and_dictation_invalid_options_fail_before_ipc() {
    for args in [
        vec!["sources", "probe"],
        vec!["sources", "probe", "--source"],
        vec!["sources", "probe", "--source", ""],
        vec!["sources", "probe", "--all", "--source", "chatgpt"],
        vec!["sources", "probe", "chatgpt", "--source", "other"],
        vec!["sources", "probe", "--all", "chatgpt"],
        vec![
            "sources",
            "probe",
            "--all",
            "--accept-upload",
            "--accept-upload",
        ],
        vec!["sources", "probe", "chatgpt", "--language"],
        vec!["sources", "probe", "chatgpt", "--language", "xx"],
        vec![
            "sources",
            "probe",
            "chatgpt",
            "--language",
            "en",
            "--language",
            "zh",
        ],
        vec!["dictation", "retry"],
        vec!["dictation", "retry", ""],
        vec!["dictation", "retry", "id", "--source", "other"],
        vec!["dictation", "fragment", "id", "--accept-upload"],
        vec!["dictation", "transcribe-fragment", "id", "--accept-upload"],
        vec!["dictation", "transcribe-fragment", "id", "--confirm", ""],
        vec![
            "dictation",
            "transcribe-fragment",
            "id",
            "--confirm",
            "--accept-upload",
        ],
        vec!["dictation", "start", "--language", "en"],
        vec![
            "dictation",
            "start",
            "--source",
            "chatgpt",
            "--source",
            "other",
        ],
        vec![
            "dictation",
            "start",
            "--source",
            "chatgpt",
            "--language",
            "xx",
        ],
        vec!["sources", "unknown", "--help"],
    ] {
        let mut full = vec!["--json", "--socket", "relative-unused-socket"];
        full.extend(args);
        assert_eq!(json(&full, 2)["error"]["code"], "invalid_arguments");
    }
}
#[cfg(target_os = "macos")]
#[test]
fn parsed_options_reach_only_the_explicit_synthetic_server() {
    use std::time::{Duration, Instant};
    use voiceport_control::{
        Action, DeliveryReceipt, DeliveryState, GuiDictationReceipt, Response, TranscriptionState,
        transport::Server,
    };
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let socket = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("private/control.sock");
    let server = Server::bind(&socket).unwrap();
    for (args, kind) in [
        (vec!["status"], "legacy-status"),
        (vec!["app", "status"], "gui-status"),
        (
            vec![
                "status",
                "--expect-bundle",
                "/synthetic/VoicePort.app",
                "--expect-pid",
                "19",
            ],
            "readiness",
        ),
        (
            vec![
                "sources",
                "probe",
                "--language",
                "ja",
                "--all",
                "--accept-upload",
            ],
            "batch",
        ),
        (
            vec!["sources", "probe", "chatgpt", "--language", "auto"],
            "single",
        ),
        (vec!["sources", "probe", "--source", "-synthetic"], "hyphen"),
        (vec!["sources", "probe", "--source", "--all"], "literal-all"),
        (
            vec!["sources", "probe", "--source", "--json"],
            "literal-json",
        ),
        (vec!["sources", "probe-result", "probe-id"], "result"),
        (vec!["sources", "probe-cancel", "probe-id"], "cancel"),
        (vec!["dictation", "retry", "failed-id"], "retry"),
        (vec!["dictation", "fragment", "failed-id"], "fragment"),
        (
            vec![
                "dictation",
                "transcribe-fragment",
                "failed-id",
                "--confirm",
                "offer-token",
                "--accept-upload",
            ],
            "transcribe-fragment",
        ),
        (
            vec![
                "dictation",
                "start",
                "--language",
                "fr",
                "--source",
                "chatgpt",
            ],
            "dictation",
        ),
        (
            vec![
                "sources",
                "add",
                "synthetic",
                "--plugin",
                "lab.adapter",
                "--name",
                "--json",
            ],
            "name",
        ),
        (
            vec![
                "sources",
                "add",
                "synthetic",
                "--plugin",
                "lab.adapter",
                "--name",
                "--socket",
            ],
            "name-socket",
        ),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_voiceport"))
            .args(["--json", "--socket", socket.to_str().unwrap()])
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let pending = loop {
            if let Some(request) = server.next() {
                break request;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("No request for {kind}");
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        match (&pending.request.action, kind) {
            (Action::Status, "legacy-status" | "gui-status" | "readiness") => {}
            (
                Action::SourcesProbe {
                    source,
                    language,
                    accept_upload,
                },
                "batch",
            ) => {
                assert!(source.is_none());
                assert_eq!(language, "ja");
                assert!(*accept_upload);
            }
            (
                Action::SourcesProbe {
                    source,
                    language,
                    accept_upload,
                },
                "single",
            ) => {
                assert_eq!(source.as_deref(), Some("chatgpt"));
                assert!(language.is_empty());
                assert!(!accept_upload);
            }
            (Action::SourcesProbe { source, .. }, "hyphen") => {
                assert_eq!(source.as_deref(), Some("-synthetic"))
            }
            (Action::SourcesProbe { source, .. }, "literal-all") => {
                assert_eq!(source.as_deref(), Some("--all"))
            }
            (Action::SourcesProbe { source, .. }, "literal-json") => {
                assert_eq!(source.as_deref(), Some("--json"))
            }
            (Action::SourcesProbeResult { operation }, "result")
            | (Action::SourcesProbeCancel { operation }, "cancel") => {
                assert_eq!(operation, "probe-id")
            }
            (Action::DictationRetry { operation }, "retry") => {
                assert_eq!(operation, "failed-id");
            }
            (Action::DictationFragment { operation }, "fragment") => {
                assert_eq!(operation, "failed-id");
            }
            (
                Action::DictationTranscribeFragment {
                    operation,
                    confirmation,
                    accept_upload,
                },
                "transcribe-fragment",
            ) => {
                assert_eq!(operation, "failed-id");
                assert_eq!(confirmation, "offer-token");
                assert!(accept_upload);
            }
            (Action::DictationStart { source, language }, "dictation") => {
                assert_eq!(source, "chatgpt");
                assert_eq!(language, "fr");
            }
            (Action::SourcesSave { name, .. }, "name") => {
                assert_eq!(name.as_deref(), Some("--json"))
            }
            (Action::SourcesSave { name, .. }, "name-socket") => {
                assert_eq!(name.as_deref(), Some("--socket"))
            }
            _ => panic!("Wrong request for {kind}"),
        }
        let data = match kind {
            "legacy-status" => serde_json::json!({"input": {"method": "clipboard_final"}}),
            "gui-status" => serde_json::json!({"input": {
                "method": "clipboard_final",
                "gui_receipt": GuiDictationReceipt {
                    session: 5,
                    generation: 5,
                    automatic: true,
                    transcription: TranscriptionState::Completed,
                    delivery: DeliveryReceipt {
                        state: DeliveryState::Submitted,
                        submitted_operations: 1,
                        confirmed_operations: 0,
                        stopped: false,
                    },
                    method: Some("clipboard_final".into()),
                    failure_stage: None,
                    block_reason: None,
                },
            }}),
            "readiness" => serde_json::json!({
                "bundle_path": "/synthetic/VoicePort.app", "pid": 19,
                "permissions": {"microphone": "authorized", "speech": "denied", "accessibility": true},
                "input": {"automatic": true}, "hotkey": {"registration": "conflict"},
            }),
            _ => serde_json::json!({"synthetic": true}),
        };
        pending.respond(Response::ok(data.clone()));
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty());
        let mut result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["schema_version"], 1);
        if kind == "readiness" {
            let readiness = result["data"]
                .as_object_mut()
                .unwrap()
                .remove("readiness")
                .unwrap();
            assert_eq!(readiness["instance"], "matched");
            assert_eq!(readiness["microphone"], "granted");
            assert_eq!(readiness["hotkey"], "conflict");
            assert_eq!(
                readiness["next_steps"],
                serde_json::json!(["change_shortcut"])
            );
            assert!(server.next().is_none());
        }
        assert_eq!(result["data"], data);
    }
}
#[cfg(target_os = "macos")]
#[test]
fn explicit_missing_instance_returns_actionable_json_without_fallback() {
    // Unix socket paths must stay short even under the verifier's nested TMPDIR.
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let socket = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("private/control.sock");
    let response = json(
        &["--json", "--socket", socket.to_str().unwrap(), "status"],
        3,
    );
    assert_eq!(response["error"]["code"], "app_not_running");
    let projected = json(
        &[
            "--json",
            "--socket",
            socket.to_str().unwrap(),
            "app",
            "status",
            "--expect-bundle",
            "/synthetic/VoicePort.app",
        ],
        3,
    );
    assert_eq!(projected["error"]["code"], "app_not_running");
    assert_eq!(projected["data"]["readiness"]["instance"], "not_running");
    assert_eq!(projected["data"]["readiness"]["microphone"], "unknown");
    for options in [
        vec!["--expect-bundle", "relative.app"],
        vec!["--expect-pid", "19"],
        vec![
            "--expect-bundle",
            "/synthetic/VoicePort.app",
            "--expect-pid",
            "0",
        ],
    ] {
        let mut args = vec!["--json", "--socket", socket.to_str().unwrap(), "status"];
        args.extend(options);
        assert_eq!(json(&args, 2)["error"]["code"], "invalid_arguments");
    }
    assert!(!socket.exists());
    assert!(!socket.parent().unwrap().exists());
}
#[cfg(target_os = "macos")]
#[test]
fn multiple_synthetic_secrets_travel_only_over_private_stdin_and_ipc() {
    use std::{
        io::Write,
        time::{Duration, Instant},
    };
    use voiceport_control::{Action, Response, transport::Server};
    // Unix socket paths must stay short even under the verifier's nested TMPDIR.
    let temp = tempfile::tempdir_in("/tmp").unwrap();
    let socket = temp
        .path()
        .canonicalize()
        .unwrap()
        .join("private/control.sock");
    let server = Server::bind(&socket).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_voiceport"))
        .args([
            "--json",
            "--socket",
            socket.to_str().unwrap(),
            "sources",
            "add",
            "synthetic",
            "--plugin",
            "lab.adapter",
            "--name",
            "Synthetic",
            "--secrets-stdin",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"first":"SYNTHETIC_STDIN_FIRST","second":"SYNTHETIC_STDIN_SECOND"}"#)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    let pending = loop {
        if let Some(p) = server.next() {
            break p;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("synthetic request did not arrive");
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    match &pending.request.action {
        Action::SourcesSave {
            secrets, config, ..
        } => {
            assert_eq!(secrets.len(), 2);
            assert_eq!(secrets["first"], "SYNTHETIC_STDIN_FIRST");
            assert_eq!(secrets["second"], "SYNTHETIC_STDIN_SECOND");
            assert!(config.is_empty());
        }
        _ => panic!("wrong command"),
    }
    pending.respond(Response::ok(serde_json::json!({"source":"synthetic"})));
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("SYNTHETIC_STDIN"));
}
#[test]
fn both_entrypoints_preserve_legacy_demo_and_help() {
    for binary in [
        env!("CARGO_BIN_EXE_voiceport"),
        env!("CARGO_BIN_EXE_voiceport-rs"),
    ] {
        for args in [["demo"], ["--help"]] {
            let output = Command::new(binary).args(args).output().unwrap();
            assert!(output.status.success());
            assert!(!output.stdout.is_empty());
            assert!(output.stderr.is_empty());
        }
    }
}
