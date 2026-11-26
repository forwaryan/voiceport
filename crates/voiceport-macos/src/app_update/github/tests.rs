use super::*;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Instant;

fn wait_for(mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready() {
        assert!(Instant::now() < deadline, "fixture did not make progress");
        thread::sleep(Duration::from_millis(5));
    }
}

fn send_bytes(connection: &mut TcpStream, bytes: &[u8], chunked: bool) {
    if chunked {
        write!(connection, "{:x}\r\n", bytes.len()).unwrap();
    }
    connection.write_all(bytes).unwrap();
    if chunked {
        connection.write_all(b"\r\n").unwrap();
    }
}

#[test]
fn slow_loopback_body_reports_actual_bytes_and_framing() {
    const TOTAL: usize = 4096;
    for (chunked, truncated) in [(false, true), (false, false), (true, false)] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/synthetic.zip", listener.local_addr().unwrap());
        let progress = DownloadProgress::default();
        assert_eq!(progress.snapshot().percent(), None);
        let client_progress = progress.clone();
        let client = thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let client = reqwest::Client::builder()
                        .no_proxy()
                        .timeout(Duration::from_secs(8))
                        .build()
                        .unwrap();
                    let response = client.get(url).send().await.unwrap();
                    assert_eq!(response.status(), StatusCode::OK);
                    // The final response may be shorter than the earlier metadata.
                    read_body(response, (TOTAL * 2) as u64, Some(&client_progress)).await
                })
        });

        let mut accepted = None;
        wait_for(|| {
            accepted = match listener.accept() {
                Ok((stream, _)) => Some(stream),
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => None,
                Err(error) => panic!("loopback accept: {error}"),
            };
            accepted.is_some()
        });
        let mut connection = accepted.unwrap();
        connection.set_nonblocking(false).unwrap();
        connection
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut request = BufReader::new(&mut connection);
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(request.read_line(&mut line).unwrap(), 0);
            if line == "\r\n" {
                break;
            }
        }
        let framing = if chunked {
            "Transfer-Encoding: chunked".into()
        } else {
            format!("Content-Length: {TOTAL}")
        };
        write!(
            connection,
            "HTTP/1.1 200 OK\r\n{framing}\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        send_bytes(&mut connection, &[1; 512], chunked);
        wait_for(|| progress.snapshot().received == 512);
        let first = progress.snapshot();
        assert_eq!(first.percent(), (!chunked).then_some(12));
        assert_eq!(first.rate(Instant::now()), None);

        thread::sleep(Duration::from_millis(550));
        send_bytes(&mut connection, &[2; 256], chunked);
        wait_for(|| progress.snapshot().received == 768);
        let second = progress.snapshot();
        let slow = second.rate(Instant::now()).unwrap();
        assert_eq!(second.percent(), (!chunked).then_some(18));
        if truncated {
            drop(connection);
            assert!(matches!(
                client.join().unwrap(),
                Err(PublicError::Connection)
            ));
            assert!(!progress.snapshot().verifying);
            println!(
                "loopback truncated: 512 → 768 B, 12 → 18%, {slow} B/s, failed before verification"
            );
            continue;
        }

        thread::sleep(Duration::from_millis(550));
        send_bytes(&mut connection, &[3; 2048], chunked);
        wait_for(|| progress.snapshot().received == 2816);
        let third = progress.snapshot();
        let fast = third.rate(Instant::now()).unwrap();
        assert!(fast > slow, "{fast} should exceed {slow}");
        assert_eq!(third.percent(), (!chunked).then_some(68));
        send_bytes(&mut connection, &[4; TOTAL - 2816], chunked);
        if chunked {
            connection.write_all(b"0\r\n\r\n").unwrap();
        }
        let bytes = client.join().unwrap().unwrap();
        assert_eq!(bytes.len(), TOTAL);
        assert_eq!(progress.snapshot().received, TOTAL as u64);
        assert_eq!(progress.snapshot().percent(), (!chunked).then_some(99));
        progress.verifying();
        assert_eq!(progress.snapshot().percent(), (!chunked).then_some(100));
        assert_eq!(progress.snapshot().rate(Instant::now()), None);
        println!(
            "loopback {}: 512 → 768 → 2816 → {TOTAL} B, {:?} → {:?} → {:?} → {:?}%, {slow} → {fast} B/s, complete before verification",
            if chunked {
                "chunked"
            } else {
                "known length after retry"
            },
            first.percent(),
            second.percent(),
            third.percent(),
            progress.snapshot().percent(),
        );
    }
}

#[test]
fn gh_reports_stdout_with_asset_size_and_requires_a_successful_exit() {
    let home = tempfile::tempdir().unwrap();
    let gh = home.path().join("fixture-gh");
    fs::write(&gh, concat!(
        "#!/bin/sh\n",
        "[ \"$*\" = 'api --hostname github.com --method GET repos/forwaryan/voiceport/releases/assets/23 --header Accept: application/octet-stream' ] || exit 4\n",
        "printf 12\n",
        "i=0\n",
        "while [ ! -f \"$HOME/continue\" ]; do\n",
        "  [ \"$i\" -lt 250 ] || exit 2\n",
        "  i=$((i + 1))\n",
        "  /bin/sleep .02\n",
        "done\n",
        "printf 345678\n",
        "exit 1\n",
    )).unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
    let update = Update {
        version: "0.3.14".into(),
        tag: "v0.3.14".into(),
        asset_name: "VoicePort-macos-arm64.zip".into(),
        id: 23,
        digest: [0; 32],
        bytes: 8,
        source: Source::Cli(gh),
    };
    let progress = DownloadProgress::default();
    let result = thread::scope(|scope| {
        let worker = scope.spawn(|| download(home.path(), &update, &progress));
        wait_for(|| progress.snapshot().received > 0);
        let first = progress.snapshot();
        assert_eq!(first.received, 2);
        assert_eq!(first.percent(), Some(25));
        assert_eq!(first.rate(Instant::now()), None);
        thread::sleep(Duration::from_millis(550));
        fs::write(home.path().join("continue"), []).unwrap();
        worker.join().unwrap()
    });
    assert!(result.is_err());
    let last = progress.snapshot();
    assert_eq!(last.received, 8);
    assert_eq!(last.percent(), Some(99));
    assert!(last.rate(Instant::now()).is_some_and(|rate| rate > 0.0));
    assert!(!last.verifying);
    println!("synthetic gh: 2 → 8 B, 25 → 99%, nonzero exit blocked verification");
}

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
