use super::*;

#[test]
fn public_metadata_works_without_gh_or_credentials() {
    let home = tempfile::tempdir().unwrap();
    let metadata = br#"{"tag_name":"v0.3.14"}"#;
    let (bytes, source) = latest_with(home.path(), Ok(metadata.to_vec()), |_| {
        panic!("a successful public request does not discover or execute gh")
    })
    .unwrap();
    assert_eq!(bytes, metadata);
    assert!(matches!(source, Source::Public));
}

#[test]
fn restricted_or_rate_limited_requests_find_home_gh_with_minimal_path() {
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join(".local/bin/gh");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, concat!(
        "#!/bin/sh\n",
        "[ \"$PATH\" = /usr/bin:/bin ] && [ \"$PWD\" = / ] && ",
        "[ \"$GH_PROMPT_DISABLED\" = 1 ] && [ \"$HOME/.local/bin/gh\" = \"$0\" ] && ",
        "[ \"$*\" = 'api --hostname github.com --method GET repos/forwaryan/voiceport/releases/latest --header Accept: application/vnd.github+json' ] || exit 4\n",
        "printf '%s' '{\"tag_name\":\"v0.3.14\"}'\n",
    )).unwrap();
    let find_cli =
        |home: &Path| executable([home.join("missing/bin/gh"), home.join(".local/bin/gh")]);
    assert!(find_cli(home.path()).is_none());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    for error in [PublicError::Restricted, PublicError::RateLimited] {
        let (bytes, source) = latest_with(home.path(), Err(error), find_cli).unwrap();
        assert_eq!(bytes, br#"{"tag_name":"v0.3.14"}"#);
        assert!(matches!(source, Source::Cli(gh) if gh == path));
    }
    for (exit_code, expected) in [(4, "尚未登录 GitHub"), (1, "GitHub 登录状态")] {
        fs::write(&path, format!("#!/bin/sh\nexit {exit_code}\n")).unwrap();
        let error = latest_with(home.path(), Err(PublicError::Restricted), find_cli).unwrap_err();
        assert!(error.contains(expected));
    }
}

#[test]
fn distinct_public_and_missing_cli_failures_do_not_report_a_version() {
    for (failure, message) in [
        (PublicError::Restricted, "未找到 GitHub CLI"),
        (PublicError::RateLimited, "限制了公开查询"),
        (PublicError::Connection, "无法连接 GitHub"),
        (PublicError::Response, "异常的发布响应"),
    ] {
        let error = latest_with(Path::new("/nonexistent"), Err(failure), |_| None).unwrap_err();
        assert!(error.contains(message), "{error}");
    }
}
