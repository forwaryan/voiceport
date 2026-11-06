use super::*;
use serde_json::json;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, oneshot};
use tokio::time::timeout;

struct Fixture {
    _directory: tempfile::TempDir,
    audio: PathBuf,
    auth: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let audio = directory.path().join("private-user-filename.m4a");
        let auth = directory.path().join("synthetic-auth.json");
        std::fs::write(&audio, [0, 255, 1, 13, 10]).unwrap();
        let fixture = Self {
            _directory: directory,
            audio,
            auth,
        };
        fixture.credentials("synthetic-token", "synthetic-account");
        fixture
    }
    fn credentials(&self, token: &str, account: &str) {
        std::fs::write(&self.auth, serde_json::to_vec(&json!({"auth_mode":"chatgpt","tokens":{"access_token":token,"account_id":account}})).unwrap()).unwrap();
    }
    fn client(&self, endpoint: &str) -> TranscriptionClient {
        TranscriptionClient::configured(
            self.auth.clone(),
            endpoint,
            Duration::from_secs(3),
            Duration::from_secs(2),
        )
        .unwrap()
    }
}
struct Request {
    headers: String,
    body: Vec<u8>,
}
async fn listener() -> (TcpListener, String) {
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let endpoint = format!(
        "http://{}/backend-api/transcribe",
        listener.local_addr().unwrap()
    );
    (listener, endpoint)
}
async fn request(stream: &mut TcpStream) -> Request {
    timeout(Duration::from_secs(3), async {
        let mut bytes = Vec::new();
        let (headers, end, length) = loop {
            let mut buffer = [0; 4096];
            let count = stream.read(&mut buffer).await.unwrap();
            assert!(count > 0, "request ended early");
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(index) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                let headers = String::from_utf8(bytes[..index].to_vec()).unwrap();
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .unwrap()
                    .parse()
                    .unwrap();
                break (headers, index + 4, length);
            }
            assert!(bytes.len() < 16_384);
        };
        while bytes.len() - end < length {
            let mut buffer = [0; 4096];
            let count = stream.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
        }
        Request {
            headers,
            body: bytes[end..end + length].to_vec(),
        }
    })
    .await
    .unwrap()
}
async fn respond(stream: &mut TcpStream, status: u16, extra: &str, body: &[u8]) {
    let headers = format!(
        "HTTP/1.1 {status} test\r\nContent-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(body).await.unwrap();
}
async fn result_for(status: u16, body: &[u8]) -> Result<String> {
    let fixture = Fixture::new();
    let (listener, endpoint) = listener().await;
    let body = body.to_vec();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        respond(&mut stream, status, "", &body).await;
        assert!(
            timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "request was automatically retried"
        );
    });
    let result = fixture
        .client(&endpoint)
        .transcribe(&fixture.audio, None, &CancellationToken::new())
        .await;
    server.await.unwrap();
    result
}

#[tokio::test]
async fn real_reqwest_sends_binary_multipart_safe_name_and_required_headers() {
    let fixture = Fixture::new();
    let (listener, endpoint) = listener().await;
    let transcript = "保留 requestID\r\n原文\t\u{1b}";
    let response = serde_json::to_vec(&json!({"text":transcript})).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = request(&mut stream).await;
        let headers = request.headers.to_ascii_lowercase();
        assert!(headers.starts_with("post /backend-api/transcribe http/1.1"));
        for header in [
            "authorization: bearer synthetic-token",
            "chatgpt-account-id: synthetic-account",
            "originator: codex_dictation",
            "user-agent: voiceport-rust/0.1",
        ] {
            assert!(headers.contains(header));
        }
        assert!(!headers.contains("cookie:"));
        assert!(!headers.contains("private-user-filename"));
        assert!(request.body.windows(5).any(|v| v == [0, 255, 1, 13, 10]));
        let body = String::from_utf8_lossy(&request.body);
        assert!(body.contains("filename=\"dictation.m4a\""));
        assert!(body.contains("Content-Type: audio/mp4"));
        assert!(body.contains("name=\"language\"\r\n\r\nzh-CN\r\n"));
        assert!(!body.contains("private-user-filename"));
        let boundary = headers
            .lines()
            .find(|v| v.starts_with("content-type:"))
            .unwrap()
            .split("boundary=")
            .nth(1)
            .unwrap();
        // Boundary case is significant: extract the original header's value.
        let original = request
            .headers
            .lines()
            .find(|v| v.to_ascii_lowercase().starts_with("content-type:"))
            .unwrap()
            .split("boundary=")
            .nth(1)
            .unwrap();
        assert!(!boundary.is_empty());
        assert!(
            request
                .body
                .ends_with(format!("--{original}--\r\n").as_bytes())
        );
        respond(&mut stream, 200, "", &response).await;
    });
    assert_eq!(
        fixture
            .client(&endpoint)
            .transcribe(&fixture.audio, Some(" zh-CN "), &CancellationToken::new())
            .await
            .unwrap(),
        transcript
    );
    server.await.unwrap();
}

#[tokio::test]
async fn same_real_client_has_no_cookie_persistence_or_implicit_language() {
    let fixture = Fixture::new();
    let (listener, endpoint) = listener().await;
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request(&mut stream).await;
            assert!(!request.headers.to_ascii_lowercase().contains("cookie:"));
            assert!(!String::from_utf8_lossy(&request.body).contains("name=\"language\""));
            respond(
                &mut stream,
                200,
                "Set-Cookie: private-cookie=synthetic; Path=/\r\n",
                br#"{"text":"ok"}"#,
            )
            .await;
        }
    });
    let client = fixture.client(&endpoint);
    for _ in 0..2 {
        assert_eq!(
            client
                .transcribe(&fixture.audio, None, &CancellationToken::new())
                .await
                .unwrap(),
            "ok"
        );
    }
    server.await.unwrap();
}

#[tokio::test]
async fn credentials_are_reloaded_before_every_request_without_refresh() {
    let fixture = Fixture::new();
    let (listener, endpoint) = listener().await;
    let server = tokio::spawn(async move {
        for credential in ["synthetic-token", "changed-token"] {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = request(&mut stream).await;
            assert!(request.headers.contains(&format!("Bearer {credential}")));
            respond(&mut stream, 200, "", br#"{"text":"ok"}"#).await;
        }
        assert!(
            timeout(Duration::from_millis(150), listener.accept())
                .await
                .is_err()
        );
    });
    let client = fixture.client(&endpoint);
    client
        .transcribe(&fixture.audio, None, &CancellationToken::new())
        .await
        .unwrap();
    fixture.credentials("changed-token", "changed-account");
    client
        .transcribe(&fixture.audio, None, &CancellationToken::new())
        .await
        .unwrap();
    std::fs::write(&fixture.auth, b"malformed private login").unwrap();
    assert_eq!(
        client
            .transcribe(&fixture.audio, None, &CancellationToken::new())
            .await
            .unwrap_err(),
        Error::InvalidCredentials
    );
    server.await.unwrap();
}

#[tokio::test]
async fn http_errors_are_structured_redacted_and_not_retried() {
    for status in [301, 302, 307, 308, 401, 403, 429, 500, 503] {
        let error = result_for(status, b"private-secret-response-token")
            .await
            .unwrap_err();
        assert_eq!(error, Error::Service(status));
        assert!(!format!("{error} {error:?}").contains("private-secret"));
    }
}

#[tokio::test]
async fn reqwest_redirect_policy_does_not_contact_destination_or_forward_credentials() {
    let fixture = Fixture::new();
    let (origin, endpoint) = listener().await;
    let (destination, location) = listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = origin.accept().await.unwrap();
        request(&mut stream).await;
        respond(
            &mut stream,
            302,
            &format!("Location: {location}\r\n"),
            b"private body",
        )
        .await;
    });
    assert_eq!(
        fixture
            .client(&endpoint)
            .transcribe(&fixture.audio, None, &CancellationToken::new())
            .await
            .unwrap_err(),
        Error::Service(302)
    );
    assert!(
        timeout(Duration::from_millis(150), destination.accept())
            .await
            .is_err()
    );
    server.await.unwrap();
}

#[tokio::test]
async fn empty_malformed_and_empty_text_responses_follow_baseline() {
    for body in [
        b"".as_slice(),
        b"not-json-private",
        b"{}",
        br#"{"text":42}"#,
    ] {
        assert_eq!(
            result_for(200, body).await.unwrap_err(),
            Error::InvalidResponse
        );
    }
    assert_eq!(
        result_for(200, br#"{"text":" \r\n\t"}"#).await.unwrap_err(),
        Error::EmptyTranscript
    );
}

#[tokio::test]
async fn network_disconnect_is_redacted_and_does_not_repeat_post() {
    let fixture = Fixture::new();
    let (listener, endpoint) = listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        drop(stream);
        assert!(
            timeout(Duration::from_millis(150), listener.accept())
                .await
                .is_err()
        );
    });
    assert_eq!(
        fixture
            .client(&endpoint)
            .transcribe(&fixture.audio, None, &CancellationToken::new())
            .await
            .unwrap_err(),
        Error::Network
    );
    server.await.unwrap();
}

#[tokio::test]
async fn reqwest_total_timeout_bounds_waiting_for_headers() {
    let fixture = Fixture::new();
    let (listener, endpoint) = listener().await;
    let (release, hold) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        let _ = hold.await;
    });
    let client = TranscriptionClient::configured(
        fixture.auth.clone(),
        &endpoint,
        Duration::from_millis(60),
        Duration::from_secs(2),
    )
    .unwrap();
    assert_eq!(
        client
            .transcribe(&fixture.audio, None, &CancellationToken::new())
            .await
            .unwrap_err(),
        Error::TimedOut
    );
    let _ = release.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn reqwest_read_timeout_bounds_a_stalled_response_body() {
    let fixture = Fixture::new();
    let (listener, endpoint) = listener().await;
    let (release, hold) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{")
            .await
            .unwrap();
        let _ = hold.await;
    });
    let client = TranscriptionClient::configured(
        fixture.auth.clone(),
        &endpoint,
        Duration::from_secs(2),
        Duration::from_millis(60),
    )
    .unwrap();
    assert_eq!(
        client
            .transcribe(&fixture.audio, None, &CancellationToken::new())
            .await
            .unwrap_err(),
        Error::TimedOut
    );
    let _ = release.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn precancelled_operation_does_not_read_credentials_or_contact_server() {
    let fixture = Fixture::new();
    std::fs::remove_file(&fixture.auth).unwrap();
    let (listener, endpoint) = listener().await;
    let cancellation = CancellationToken::new();
    cancellation.cancel();
    assert_eq!(
        fixture
            .client(&endpoint)
            .transcribe(Path::new("missing-audio.m4a"), None, &cancellation)
            .await
            .unwrap_err(),
        Error::Cancelled
    );
    assert!(
        timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cancellation_during_real_network_wait_drops_request_without_retry() {
    let fixture = Fixture::new();
    let (listener, endpoint) = listener().await;
    let (arrived, arrival) = oneshot::channel();
    let (release, hold) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        arrived.send(()).unwrap();
        let _ = hold.await;
        assert!(
            timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
    });
    let token = CancellationToken::new();
    let client = fixture.client(&endpoint);
    let operation = client.transcribe(&fixture.audio, None, &token);
    tokio::pin!(operation);
    tokio::select! { result = &mut operation => panic!("completed early: {result:?}"), result = arrival => result.unwrap() }
    token.cancel();
    assert_eq!(
        timeout(Duration::from_millis(250), operation)
            .await
            .unwrap()
            .unwrap_err(),
        Error::Cancelled
    );
    let _ = release.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn cancellation_after_actual_response_chunk_is_read_is_prompt() {
    let fixture = Fixture::new();
    let (listener, endpoint) = listener().await;
    let (release, hold) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n{\"text\":\"partial").await.unwrap();
        let _ = hold.await;
    });
    let observer = Arc::new(Notify::new());
    let mut client = fixture.client(&endpoint);
    // Observer only: it cannot replace reqwest, alter response bytes or bypass configuration.
    client.chunk_read = Some(observer.clone());
    let token = CancellationToken::new();
    let operation = client.transcribe(&fixture.audio, None, &token);
    tokio::pin!(operation);
    tokio::select! { result = &mut operation => panic!("completed early: {result:?}"), _ = observer.notified() => () }
    token.cancel();
    assert_eq!(
        timeout(Duration::from_millis(250), operation)
            .await
            .unwrap()
            .unwrap_err(),
        Error::Cancelled
    );
    let _ = release.send(());
    server.await.unwrap();
}

#[tokio::test]
async fn declared_response_limit_is_enforced_before_reading_large_body() {
    let fixture = Fixture::new();
    let (listener, endpoint) = listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    MAXIMUM_RESPONSE_BYTES + 1
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    assert_eq!(
        fixture
            .client(&endpoint)
            .transcribe(&fixture.audio, None, &CancellationToken::new())
            .await
            .unwrap_err(),
        Error::InvalidResponse
    );
    server.await.unwrap();
}

#[tokio::test]
async fn chunked_valid_json_cannot_bypass_response_limit() {
    let fixture = Fixture::new();
    let (listener, endpoint) = listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request(&mut stream).await;
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let body =
            serde_json::to_vec(&json!({"text":"a".repeat(MAXIMUM_RESPONSE_BYTES + 1)})).unwrap();
        for bytes in body.chunks(8192) {
            if stream
                .write_all(format!("{:x}\r\n", bytes.len()).as_bytes())
                .await
                .is_err()
            {
                return;
            }
            if stream.write_all(bytes).await.is_err() {
                return;
            }
            if stream.write_all(b"\r\n").await.is_err() {
                return;
            }
        }
        let _ = stream.write_all(b"0\r\n\r\n").await;
    });
    assert_eq!(
        fixture
            .client(&endpoint)
            .transcribe(&fixture.audio, None, &CancellationToken::new())
            .await
            .unwrap_err(),
        Error::InvalidResponse
    );
    server.await.unwrap();
}

#[tokio::test]
async fn explicit_https_proxy_uses_connect_without_leaking_credentials() {
    let fixture = Fixture::new();
    let (proxy, _) = listener().await;
    let proxy_url = format!("http://{}", proxy.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut stream, _) = timeout(Duration::from_secs(3), proxy.accept())
            .await
            .unwrap()
            .unwrap();
        let mut headers = Vec::new();
        while !headers.windows(4).any(|v| v == b"\r\n\r\n") {
            let mut block = [0; 1024];
            let count = timeout(Duration::from_secs(3), stream.read(&mut block))
                .await
                .unwrap()
                .unwrap();
            assert!(count > 0 && headers.len() < 16_384);
            headers.extend_from_slice(&block[..count]);
        }
        let headers = String::from_utf8(headers).unwrap().to_lowercase();
        assert!(headers.starts_with("connect chatgpt.com:443 http/1.1"));
        assert!(!headers.contains("synthetic-token"));
        assert!(!headers.contains("synthetic-account"));
        assert!(!headers.contains("authorization:"));
        respond(&mut stream, 502, "", b"").await;
    });
    let client = TranscriptionClient::configured_with_proxy(
        fixture.auth.clone(),
        ENDPOINT,
        Duration::from_secs(3),
        Duration::from_secs(2),
        Some(&proxy_url),
    )
    .unwrap();
    assert_eq!(
        client
            .transcribe(&fixture.audio, None, &CancellationToken::new())
            .await
            .unwrap_err(),
        Error::Network
    );
    server.await.unwrap();
}

#[test]
fn invalid_or_credential_bearing_proxy_is_rejected_without_disclosing_it() {
    let fixture = Fixture::new();
    for proxy in [
        "not-a-url",
        "file:///private",
        "http://user:private-password@localhost:7897",
        "http://localhost/path",
        "http://localhost?private",
        "http://localhost#private",
    ] {
        let result = TranscriptionClient::configured_with_proxy(
            fixture.auth.clone(),
            ENDPOINT,
            Duration::from_secs(3),
            Duration::from_secs(2),
            Some(proxy),
        );
        assert!(matches!(result, Err(Error::Network)));
    }
}

#[tokio::test]
async fn production_builder_requires_https_and_fixed_endpoint() {
    let fixture = Fixture::new();
    let client = TranscriptionClient::new(fixture.auth.clone()).unwrap();
    assert_eq!(client.endpoint.as_str(), ENDPOINT);
    let (listener, http_endpoint) = listener().await;
    assert!(
        client
            .client
            .get(http_endpoint)
            .send()
            .await
            .unwrap_err()
            .is_builder()
    );
    assert!(
        timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn invalid_audio_and_language_do_not_reach_http_stack() {
    let fixture = Fixture::new();
    let (listener, endpoint) = listener().await;
    let client = fixture.client(&endpoint);
    assert_eq!(
        client
            .transcribe(Path::new("missing.m4a"), None, &CancellationToken::new())
            .await
            .unwrap_err(),
        Error::InvalidAudio
    );
    assert_eq!(
        client
            .transcribe(
                &fixture.audio,
                Some("zh\r\nprivate injection"),
                &CancellationToken::new()
            )
            .await
            .unwrap_err(),
        Error::InvalidLanguage
    );
    assert!(
        timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
}

#[test]
fn actual_request_header_builder_marks_both_credentials_sensitive() {
    let credentials = CodexCredentials::decode(br#"{"tokens":{"access_token":"synthetic-private-token","account_id":"synthetic-private-account"}}"#, 0.0).unwrap();
    let headers = credential_headers(&credentials).unwrap();
    assert!(headers[AUTHORIZATION].is_sensitive());
    assert!(headers["chatgpt-account-id"].is_sensitive());
    assert!(!format!("{headers:?}").contains("synthetic-private"));
    for error in [
        Error::SignInRequired,
        Error::InvalidCredentials,
        Error::ExpiredCredentials,
        Error::Network,
        Error::TimedOut,
        Error::Service(500),
        Error::InvalidResponse,
        Error::Cancelled,
    ] {
        assert!(!format!("{error} {error:?}").contains("synthetic-private"));
        assert!(std::error::Error::source(&error).is_none());
    }
}

#[tokio::test]
async fn top_level_sequence_cannot_impersonate_a_transcription_object() {
    assert_eq!(
        result_for(200, br#"["not an object"]"#).await.unwrap_err(),
        Error::InvalidResponse
    );
}
