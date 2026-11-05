use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output};

const AUTH: &str = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"CLI_PRIVATE_TOKEN","account_id":"CLI_PRIVATE_ACCOUNT"}}"#;
fn run(home: &Path, arguments: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_voiceport-rs"))
        .args(arguments)
        .env("CODEX_HOME", home)
        .output()
        .unwrap()
}
fn text(output: &[u8]) -> String {
    String::from_utf8(output.to_vec()).unwrap()
}
fn assert_redacted(output: &Output) {
    let all = format!("{}{}", text(&output.stdout), text(&output.stderr));
    for secret in [
        "CLI_PRIVATE_TOKEN",
        "CLI_PRIVATE_ACCOUNT",
        "RAW_PRIVATE_JSON",
    ] {
        assert!(!all.contains(secret));
    }
}
#[test]
fn help_aliases_are_offline_and_leave_synthetic_home_absent() {
    let scope = tempfile::tempdir().unwrap();
    let home = scope.path().join("absent-codex-home");
    for argument in [None, Some("help"), Some("--help"), Some("-h")] {
        let arguments: Vec<_> = argument.map(OsStr::new).into_iter().collect();
        let output = run(&home, &arguments);
        assert_eq!(output.status.code(), Some(0));
        assert!(text(&output.stdout).contains("voiceport-rs transcribe AUDIO"));
        assert!(output.stderr.is_empty());
    }
    assert!(!home.exists());
}
#[test]
fn demo_is_explicitly_offline_and_never_requires_login() {
    let scope = tempfile::tempdir().unwrap();
    let home = scope.path().join("absent-codex-home");
    let output = run(&home, &[OsStr::new("demo")]);
    assert_eq!(output.status.code(), Some(0));
    assert!(text(&output.stdout).contains("离线模拟，未调用云端"));
    assert!(output.stderr.is_empty() && !home.exists());
}
#[test]
fn doctor_only_reports_existence_even_for_malformed_synthetic_auth() {
    let scope = tempfile::tempdir().unwrap();
    let file = scope.path().join("synthetic-auth.json");
    std::fs::write(&file, "RAW_PRIVATE_JSON: not valid JSON").unwrap();
    for path in [file.as_path(), &scope.path().join("missing.json")] {
        let output = run(
            scope.path(),
            &[OsStr::new("doctor"), OsStr::new("--auth"), path.as_os_str()],
        );
        assert_eq!(output.status.code(), Some(0));
        assert!(text(&output.stdout).contains(if path.exists() {
            "存在（未读取、未检查有效性）"
        } else {
            "未找到"
        }));
        assert!(output.stderr.is_empty());
        assert!(!text(&output.stdout).contains(&path.to_string_lossy().to_string()));
        assert_redacted(&output);
    }
}
#[test]
fn invalid_grammar_is_nonzero_without_echoing_arguments() {
    let scope = tempfile::tempdir().unwrap();
    for args in [
        vec!["unknown"],
        vec!["demo", "extra"],
        vec!["help", "extra"],
        vec!["doctor", "--language", "zh"],
        vec!["doctor", "--auth"],
        vec!["transcribe"],
        vec!["transcribe", "--auth", "file"],
        vec!["transcribe", "file", "--language"],
        vec!["transcribe", "file", "--auth", "a", "--auth", "b"],
        vec!["transcribe", "file", "--language", "zh", "--language", "en"],
        vec!["transcribe", "file", "--endpoint", "http://127.0.0.1:1"],
        vec!["transcribe", "file", "--auth", ""],
    ] {
        let args: Vec<_> = args.iter().map(OsStr::new).collect();
        let output = run(scope.path(), &args);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
        assert!(text(&output.stderr).contains("参数无效"));
        assert_redacted(&output);
    }
}
#[test]
fn runtime_input_failures_are_stderr_only_redacted_and_never_upload() {
    let scope = tempfile::tempdir().unwrap();
    let auth = scope.path().join("auth.json");
    let audio = scope.path().join("fixture.m4a");
    let missing = scope.path().join("missing-audio.m4a");
    std::fs::write(&auth, AUTH).unwrap();
    std::fs::write(&audio, [0, 255, 1]).unwrap();
    let cases = [
        (auth.clone(), missing.clone(), None, "非空"),
        (
            scope.path().join("missing-auth.json"),
            audio.clone(),
            None,
            "未找到",
        ),
        (
            auth.clone(),
            audio.clone(),
            Some("invalid-language"),
            "语言",
        ),
    ];
    for (auth, audio, language, marker) in cases {
        let mut arguments = vec![
            OsStr::new("transcribe"),
            audio.as_os_str(),
            OsStr::new("--auth"),
            auth.as_os_str(),
        ];
        if let Some(language) = language {
            arguments.extend([OsStr::new("--language"), OsStr::new(language)]);
        }
        let output = run(scope.path(), &arguments);
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty() && text(&output.stderr).contains(marker));
        assert_redacted(&output);
    }
    for body in [
        "RAW_PRIVATE_JSON",
        r#"{"auth_mode":"apikey","OPENAI_API_KEY":"CLI_PRIVATE_TOKEN"}"#,
        r#"{"tokens":{"access_token":"header.eyJleHAiOjB9.signature","account_id":"CLI_PRIVATE_ACCOUNT"}}"#,
    ] {
        std::fs::write(&auth, body).unwrap();
        let output = run(
            scope.path(),
            &[
                OsStr::new("transcribe"),
                audio.as_os_str(),
                OsStr::new("--auth"),
                auth.as_os_str(),
            ],
        );
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty() && !output.stderr.is_empty());
        assert_redacted(&output);
    }
}
#[test]
fn default_codex_home_uses_only_the_process_injected_fixture() {
    let scope = tempfile::tempdir().unwrap();
    std::fs::write(scope.path().join("auth.json"), AUTH).unwrap();
    let doctor = run(scope.path(), &[OsStr::new("doctor")]);
    assert_eq!(doctor.status.code(), Some(0));
    assert!(text(&doctor.stdout).contains("存在（未读取"));
    let output = run(
        scope.path(),
        &[
            OsStr::new("transcribe"),
            scope.path().join("missing-audio.m4a").as_os_str(),
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(text(&output.stderr).contains("非空"));
    assert_redacted(&output);
}
