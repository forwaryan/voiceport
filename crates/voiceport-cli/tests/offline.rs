use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output};

const AUTH: &str = r#"{"auth_mode":"chatgpt","tokens":{"access_token":"CLI_PRIVATE_TOKEN","account_id":"CLI_PRIVATE_ACCOUNT"}}"#;
fn run_with_paths(home: &Path, codex_home: Option<&OsStr>, arguments: &[&OsStr]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_voiceport-rs"));
    command
        .args(arguments)
        .env("HOME", home)
        .env_remove("CODEX_HOME");
    if let Some(codex_home) = codex_home {
        command.env("CODEX_HOME", codex_home);
    }
    command.output().unwrap()
}
fn run(home: &Path, arguments: &[&OsStr]) -> Output {
    run_with_paths(home, Some(home.as_os_str()), arguments)
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
        "CLI_PRIVATE_PATH",
        "CLI_PRIVATE_ARGUMENT",
    ] {
        assert!(!all.contains(secret), "output exposed {secret}: {all}");
    }
}
fn assert_paths_redacted(output: &Output, paths: &[&Path]) {
    let all = format!("{}{}", text(&output.stdout), text(&output.stderr));
    for path in paths {
        assert!(!all.contains(path.to_string_lossy().as_ref()), "{all}");
    }
    assert_redacted(output);
}
const REGULAR: &str = "普通文件（未读取、未验证登录有效性）";
const MISSING: &str = "未找到（请先通过 codex login 准备 ChatGPT 文件登录）";
const NON_REGULAR: &str = "不是普通文件（未读取、未验证登录有效性）";
const CHECK_FAILED: &str = "检查失败（无法确认文件状态，未读取或验证凭据）";
fn assert_diagnosis(output: &Output, status: &str) {
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        text(&output.stdout).lines().next(),
        Some(format!("Codex 登录文件：{status}").as_str()),
        "{output:?}"
    );
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(text(&output.stdout).contains("文件存在不等于有效登录"));
    assert!(text(&output.stdout).contains("不申请或验证系统权限"));
    assert_redacted(output);
}
fn assert_capabilities(output: &Output) {
    let stdout = text(&output.stdout);
    assert!(!stdout.contains("Rust 系统录音和投递尚未接入"));
    assert!(stdout.contains("CLI 仅处理已有音频文件，不录音、不自动投递文字。"));
    assert!(stdout.contains("macOS app 已实现录音、实时预览和受保护输入；真实用户流程仍待验证。"));
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
        assert!(text(&output.stdout).contains("普通文件、未找到、非普通文件或检查失败"));
        assert!(text(&output.stdout).contains("不申请或验证系统权限"));
        assert_capabilities(&output);
        assert_paths_redacted(&output, &[&home]);
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
fn doctor_classifies_empty_malformed_and_synthetic_credentials_only_by_metadata() {
    let scope = tempfile::tempdir().unwrap();
    let file = scope.path().join("CLI_PRIVATE_PATH-auth.json");
    let mut previous = None;
    for body in [
        b"".as_slice(),
        b"RAW_PRIVATE_JSON: not valid JSON",
        &[0xff, 0, 0xfe],
        AUTH.as_bytes(),
    ] {
        std::fs::write(&file, body).unwrap();
        let output = run(
            scope.path(),
            &[OsStr::new("doctor"), OsStr::new("--auth"), file.as_os_str()],
        );
        assert_diagnosis(&output, REGULAR);
        assert_capabilities(&output);
        assert_paths_redacted(&output, &[scope.path(), &file]);
        assert_eq!(std::fs::read(&file).unwrap(), body);
        if let Some(previous) = previous {
            assert_eq!(output.stdout, previous);
        }
        previous = Some(output.stdout);
    }
}
#[test]
fn doctor_describes_cli_and_mac_app_capabilities_separately() {
    let scope = tempfile::tempdir().unwrap();
    let output = run(scope.path(), &[OsStr::new("doctor")]);
    assert_capabilities(&output);
    assert_diagnosis(&output, MISSING);
    assert_paths_redacted(&output, &[scope.path()]);
}
#[test]
fn doctor_reports_missing_without_creating_files() {
    let scope = tempfile::tempdir().unwrap();
    let file = scope.path().join("CLI_PRIVATE_PATH-missing/auth.json");
    let output = run(
        scope.path(),
        &[OsStr::new("doctor"), OsStr::new("--auth"), file.as_os_str()],
    );
    assert_diagnosis(&output, MISSING);
    assert_paths_redacted(&output, &[scope.path(), &file]);
    assert!(!file.parent().unwrap().exists());
}
#[test]
fn doctor_rejects_directories_as_non_regular() {
    let scope = tempfile::tempdir().unwrap();
    let directory = scope.path().join("CLI_PRIVATE_PATH-directory.json");
    std::fs::create_dir(&directory).unwrap();
    let output = run(
        scope.path(),
        &[
            OsStr::new("doctor"),
            OsStr::new("--auth"),
            directory.as_os_str(),
        ],
    );
    assert_diagnosis(&output, NON_REGULAR);
    assert_paths_redacted(&output, &[scope.path(), &directory]);
    assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 0);
}
#[cfg(unix)]
#[test]
fn doctor_reports_metadata_failure_without_misclassifying_missing() {
    let scope = tempfile::tempdir().unwrap();
    let file = scope.path().join("CLI_PRIVATE_PATH-loop");
    std::os::unix::fs::symlink(file.file_name().unwrap(), &file).unwrap();
    let error = std::fs::metadata(&file).unwrap_err();
    assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
    let output = run(
        scope.path(),
        &[OsStr::new("doctor"), OsStr::new("--auth"), file.as_os_str()],
    );
    assert_diagnosis(&output, CHECK_FAILED);
    assert_paths_redacted(&output, &[scope.path(), &file]);
    assert!(!text(&output.stdout).contains(&error.to_string()));
    assert!(std::fs::symlink_metadata(&file).unwrap().is_symlink());
}
#[cfg(unix)]
#[test]
fn doctor_follows_symlinks_for_regular_and_missing_targets() {
    let scope = tempfile::tempdir().unwrap();
    let target = scope.path().join("CLI_PRIVATE_PATH-target");
    let link = scope.path().join("CLI_PRIVATE_PATH-link");
    std::fs::write(&target, "RAW_PRIVATE_JSON").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();
    for status in [REGULAR, MISSING] {
        let output = run(
            scope.path(),
            &[OsStr::new("doctor"), OsStr::new("--auth"), link.as_os_str()],
        );
        assert_diagnosis(&output, status);
        assert_paths_redacted(&output, &[scope.path(), &target, &link]);
        if target.exists() {
            std::fs::remove_file(&target).unwrap();
        }
    }
}
#[test]
fn doctor_explicit_auth_wins_without_falling_back_to_other_locations() {
    let scope = tempfile::tempdir().unwrap();
    let home = scope.path().join("CLI_PRIVATE_PATH-home");
    let codex = scope.path().join("CLI_PRIVATE_PATH-codex");
    let explicit = scope.path().join("CLI_PRIVATE_PATH-explicit.json");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::create_dir_all(codex.join("auth.json")).unwrap();
    std::fs::write(home.join(".codex/auth.json"), AUTH).unwrap();
    for status in [MISSING, REGULAR] {
        let output = run_with_paths(
            &home,
            Some(codex.as_os_str()),
            &[
                OsStr::new("doctor"),
                OsStr::new("--auth"),
                explicit.as_os_str(),
            ],
        );
        assert_diagnosis(&output, status);
        assert_paths_redacted(&output, &[scope.path(), &home, &codex, &explicit]);
        std::fs::write(&explicit, "RAW_PRIVATE_JSON").unwrap();
    }
}
#[test]
fn doctor_nonempty_codex_home_wins_without_falling_back_to_home() {
    let scope = tempfile::tempdir().unwrap();
    let home = scope.path().join("CLI_PRIVATE_PATH-home");
    let codex = scope.path().join("CLI_PRIVATE_PATH-codex");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    std::fs::create_dir_all(codex.join("auth.json")).unwrap();
    std::fs::write(home.join(".codex/auth.json"), AUTH).unwrap();
    for status in [NON_REGULAR, MISSING] {
        let output = run_with_paths(&home, Some(codex.as_os_str()), &[OsStr::new("doctor")]);
        assert_diagnosis(&output, status);
        assert_paths_redacted(&output, &[scope.path(), &home, &codex]);
        if codex.join("auth.json").exists() {
            std::fs::remove_dir(codex.join("auth.json")).unwrap();
        }
    }
}
// dirs::home_dir honors HOME on Unix; Windows uses its native known-folder API.
#[cfg(unix)]
#[test]
fn doctor_unset_or_empty_codex_home_falls_back_to_synthetic_home() {
    let scope = tempfile::tempdir().unwrap();
    let home = scope.path().join("CLI_PRIVATE_PATH-home");
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    let file = home.join(".codex/auth.json");
    std::fs::write(&file, "RAW_PRIVATE_JSON").unwrap();
    for status in [REGULAR, MISSING] {
        for codex_home in [None, Some(OsStr::new(""))] {
            let output = run_with_paths(&home, codex_home, &[OsStr::new("doctor")]);
            assert_diagnosis(&output, status);
            assert_paths_redacted(&output, &[scope.path(), &home, &file]);
        }
        if file.exists() {
            std::fs::remove_file(&file).unwrap();
        }
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
        vec!["doctor", "--auth", ""],
        vec!["doctor", "--auth", "CLI_PRIVATE_PATH", "--auth", "other"],
        vec!["doctor", "CLI_PRIVATE_ARGUMENT"],
        vec!["CLI_PRIVATE_ARGUMENT"],
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
    assert_diagnosis(&doctor, REGULAR);
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
