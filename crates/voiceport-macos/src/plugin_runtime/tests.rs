use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
};
const FIXTURE_LIMIT: Duration = Duration::from_secs(5);
struct Server {
    endpoint: String,
    requests: mpsc::Receiver<Vec<u8>>,
    received: Arc<AtomicBool>,
    disconnected: Arc<AtomicBool>,
    written: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Server {
    fn request(&self) -> Vec<u8> {
        self.requests
            .recv_timeout(FIXTURE_LIMIT)
            .expect("complete fixture request")
    }
    async fn finish(self) {
        // Keep the current-thread runtime driving connection shutdown while the
        // server completes writes and the no-extra-request observation window.
        while !self.handle.as_ref().unwrap().is_finished() {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        self.finish_blocking();
    }
    fn finish_blocking(mut self) {
        self.handle.take().unwrap().join().unwrap();
    }
    async fn assert_disconnected(&self) {
        let deadline = Instant::now() + Duration::from_secs(1);
        while !self.disconnected.load(Ordering::SeqCst) && Instant::now() < deadline {
            // Let hyper's connection task observe the dropped request future.
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        assert!(
            self.disconnected.load(Ordering::SeqCst),
            "HTTP future left its socket open"
        );
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            // Every socket operation is nonblocking and every loop polls stop.
            let result = handle.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}
fn serve(response: Vec<u8>, delay: Duration) -> Server {
    // Small buffers deliberately exercise backpressure and stop-aware cleanup.
    serve_with_send_buffer(response, delay, 4096)
}
fn serve_with_send_buffer(response: Vec<u8>, delay: Duration, send_buffer: libc::c_int) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = format!(
        "http://{}/full/transcriptions",
        listener.local_addr().unwrap()
    );
    let (tx, requests) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let received = Arc::new(AtomicBool::new(false));
    let disconnected = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicUsize::new(0));
    let written_thread = written.clone();
    let (stop_thread, received_thread, disconnected_thread) =
        (stop.clone(), received.clone(), disconnected.clone());
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + FIXTURE_LIMIT;
        let active = || !stop_thread.load(Ordering::SeqCst) && Instant::now() < deadline;
        let mut stream = loop {
            if !active() {
                return;
            }
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2))
                }
                Err(e) => panic!("fixture accept: {e}"),
            }
        };
        stream.set_nonblocking(true).unwrap();
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    std::os::fd::AsRawFd::as_raw_fd(&stream),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&send_buffer as *const libc::c_int).cast(),
                    std::mem::size_of_val(&send_buffer) as libc::socklen_t,
                )
            },
            0
        );
        let mut bytes = vec![];
        let mut chunk = [0; 8192];
        let mut expected = None;
        loop {
            if !active() {
                return;
            }
            match stream.read(&mut chunk) {
                Ok(0) => return,
                Ok(n) => bytes.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2));
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => return,
                Err(e) => panic!("fixture request: {e}"),
            }
            assert!(bytes.len() <= 8_000_000, "fixture request too large");
            if expected.is_none()
                && let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n")
            {
                let end = i + 4;
                let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                let length = headers
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length: "))
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                assert!(length <= 8_000_000 - end);
                expected = Some(end + length);
            }
            if expected.is_some_and(|len| bytes.len() >= len) {
                break;
            }
        }
        received_thread.store(true, Ordering::SeqCst);
        if tx.send(bytes).is_err() {
            return;
        }
        let respond_at = Instant::now() + delay;
        while active() && Instant::now() < respond_at {
            match stream.read(&mut chunk) {
                Ok(0) => {
                    disconnected_thread.store(true, Ordering::SeqCst);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => {
                    disconnected_thread.store(true, Ordering::SeqCst);
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2))
                }
                other => panic!("unexpected fixture data while awaiting response: {other:?}"),
            }
        }
        let mut written = 0;
        while active() && !disconnected_thread.load(Ordering::SeqCst) && written < response.len() {
            match stream.write(&response[written..]) {
                Ok(0) => break,
                Ok(n) => {
                    written += n;
                    written_thread.store(written, Ordering::SeqCst);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(2))
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ) =>
                {
                    break;
                }
                Err(e) => panic!("fixture response: {e}"),
            }
        }
        drop(stream);
        let observation_end = Instant::now() + Duration::from_millis(100);
        while active() && Instant::now() < observation_end {
            assert!(
                matches!(listener.accept(), Err(e) if e.kind() == std::io::ErrorKind::WouldBlock),
                "implicit extra request attempted"
            );
            thread::sleep(Duration::from_millis(2));
        }
    });
    Server {
        endpoint,
        requests,
        received,
        disconnected,
        written,
        stop,
        handle: Some(handle),
    }
}
fn config(endpoint: String) -> BTreeMap<String, String> {
    BTreeMap::from([
        ("endpoint".into(), endpoint),
        ("model".into(), "fixture-model".into()),
        ("api_key".into(), "synthetic-independent-key".into()),
    ])
}
fn response(status: &str, body: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    bytes.extend(body);
    bytes
}
#[tokio::test]
async fn multipart_bytes_language_bearer_and_no_chatgpt_auth() {
    let server = serve(
        response("200 OK", br#"{"text":" fixture "}"#),
        Duration::ZERO,
    );
    let sample = sample();
    let result = http(
        &config(server.endpoint.clone()),
        sample.clone(),
        "zh",
        &CancellationToken::new(),
        Duration::from_secs(2),
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    assert_eq!(result, "fixture");
    let request = server.request();
    let as_text = String::from_utf8_lossy(&request);
    assert!(as_text.starts_with("POST /full/transcriptions HTTP/1.1"));
    assert!(as_text.contains("authorization: Bearer synthetic-independent-key"));
    for missing in [
        "chatgpt-account-id",
        "originator",
        "Codex",
        "api.openai.com",
    ] {
        assert!(!as_text.contains(missing));
    }
    for value in [
        "name=\"model\"\r\n\r\nfixture-model",
        "name=\"language\"\r\n\r\nzh",
        "name=\"response_format\"\r\n\r\njson",
        "filename=\"dictation.wav\"",
        "Content-Type: audio/wav",
    ] {
        assert!(as_text.contains(value), "{value}");
    }
    assert!(request.windows(sample.len()).any(|w| w == sample));
    assert_eq!(&sample[24..28], &16000u32.to_le_bytes());
    assert_eq!(&sample[22..24], &1u16.to_le_bytes());
    assert_eq!(&sample[34..36], &16u16.to_le_bytes());
    server.finish().await;
}
#[tokio::test]
async fn no_key_and_auto_language_are_omitted_and_empty_text_is_valid() {
    let server = serve(response("200 OK", br#"{"text":""}"#), Duration::ZERO);
    let mut cfg = config(server.endpoint.clone());
    cfg.remove("api_key");
    assert_eq!(
        http(
            &cfg,
            sample(),
            "",
            &CancellationToken::new(),
            Duration::from_secs(2),
            Duration::from_secs(1)
        )
        .await
        .unwrap(),
        ""
    );
    let request = String::from_utf8_lossy(&server.request()).to_string();
    assert!(!request.contains("authorization:"));
    assert!(!request.contains("name=\"language\""));
    server.finish().await;
}
#[tokio::test]
async fn malformed_oversize_failure_and_redirect_are_bounded() {
    for bytes in [
        response("200 OK", b"[]"),
        response("200 OK", br#"{"text":7}"#),
        response("200 OK", b"bad-json"),
        response("401 Unauthorized", b"synthetic-independent-key"),
        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/never\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", MAX_RESPONSE + 1).into_bytes(),
        response("200 OK", &vec![b'x'; MAX_RESPONSE + 1]),
    ] {
        let server = serve(bytes, Duration::ZERO);
        let error = http(&config(server.endpoint.clone()), sample(), "", &CancellationToken::new(), Duration::from_secs(2), Duration::from_secs(1)).await.unwrap_err();
        assert!(!error.contains("synthetic-independent-key"));
        server.request();
        server.finish().await;
    }
}
#[tokio::test]
async fn redirect_status_is_not_followed() {
    let server = serve(
        b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/never\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
        Duration::ZERO,
    );
    let error = http(
        &config(server.endpoint.clone()),
        sample(),
        "",
        &CancellationToken::new(),
        Duration::from_secs(2),
        Duration::from_secs(1),
    )
    .await
    .unwrap_err();
    assert!(error.contains("HTTP 302"), "{error}");
    server.request();
    server.finish().await;
}
#[tokio::test]
async fn undeclared_response_size_accepts_exact_limit_and_rejects_one_more_byte() {
    assert_eq!(MAX_RESPONSE, 1_000_000);
    for size in [MAX_RESPONSE, MAX_RESPONSE + 1] {
        // Both bodies are valid JSON, so success, parse failure, or timeout
        // cannot stand in for the streaming size-limit branch.
        let prefix = br#"{"text":""#;
        let suffix = br#""}"#;
        let text = "x".repeat(size - prefix.len() - suffix.len());
        let mut bytes = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        let headers = bytes.len();
        bytes.extend(prefix);
        bytes.extend(text.as_bytes());
        bytes.extend(suffix);
        assert_eq!(bytes.len() - headers, size);
        // This test measures the response-size contract. Give its writer room
        // for the full response instead of forcing 4KiB TCP backpressure to
        // race the client's short timeout. Other fixtures retain that pressure.
        let server = serve_with_send_buffer(bytes, Duration::ZERO, 2_000_000);
        let written = server.written.clone();
        let result = http(
            &config(server.endpoint.clone()),
            sample(),
            "",
            &CancellationToken::new(),
            // Test-only safety deadlines exceed the server's bounded 5s life.
            // Timeout/cancellation expectations have their own test below.
            Duration::from_secs(30),
            Duration::from_secs(10),
        )
        .await;
        server.request();
        server.finish().await;
        let sent = written.load(Ordering::SeqCst);
        assert_eq!(sent, headers + size, "fixture did not send the boundary");
        if size == MAX_RESPONSE {
            assert_eq!(result.unwrap(), text);
        } else {
            assert_eq!(result.unwrap_err(), "转写响应超过大小限制。");
        }
        eprintln!("undeclared response: {size} body bytes, {sent} wire bytes, joined fixture");
    }
}
#[tokio::test]
async fn timeout_and_cancel_drop_network_operation() {
    for cancel_case in [false, true] {
        let server = serve(
            response("200 OK", br#"{"text":"late"}"#),
            Duration::from_secs(3),
        );
        let cancel = CancellationToken::new();
        let cfg = config(server.endpoint.clone());
        let now = Instant::now();
        let cancellation = async {
            if cancel_case {
                // Exercise an in-flight request, regardless of client setup speed.
                while !server.received.load(Ordering::SeqCst) {
                    assert!(
                        now.elapsed() < Duration::from_secs(1),
                        "request never reached fixture"
                    );
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
                cancel.cancel();
            }
        };
        let (result, ()) = tokio::join!(
            http(
                &cfg,
                sample(),
                "",
                &cancel,
                Duration::from_millis(500),
                Duration::from_millis(200)
            ),
            cancellation,
        );
        let error = result.unwrap_err();
        assert!(now.elapsed() < Duration::from_secs(1));
        assert!(
            error.contains(if cancel_case { "取消" } else { "超时" }),
            "{error}"
        );
        server.request();
        server.assert_disconnected().await;
        server.finish().await;
    }
}
#[tokio::test]
async fn fixture_cleanup_handles_no_request_partial_upload_and_assertion_failure() {
    let now = Instant::now();
    let server = serve(response("200 OK", br#"{"text":"late"}"#), Duration::ZERO);
    let stopped = server.stop.clone();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(
        http(
            &config(server.endpoint.clone()),
            sample(),
            "",
            &cancel,
            Duration::from_millis(80),
            Duration::from_millis(60)
        )
        .await
        .unwrap_err()
        .contains("取消")
    );
    drop(server);
    // The joined worker has released all owned resources. Ephemeral ports may
    // already belong to another parallel fixture, so do not reconnect to them.
    assert_eq!(Arc::strong_count(&stopped), 1);
    assert!(now.elapsed() < Duration::from_secs(1));

    let server = serve(vec![], Duration::ZERO);
    let endpoint = reqwest::Url::parse(&server.endpoint).unwrap();
    let address = ("127.0.0.1", endpoint.port().unwrap());
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    stream
        .write_all(b"POST / HTTP/1.1\r\nContent-Length: 32000\r\n\r\nx")
        .unwrap();
    drop(stream);
    let stopped = server.stop.clone();
    server.finish().await; // Early EOF is a legitimate cancellation, not a fixture panic.
    assert_eq!(Arc::strong_count(&stopped), 1);

    let server = serve(vec![], Duration::ZERO);
    let stopped = server.stop.clone();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _owned = server;
        panic!("deliberate test assertion failure to exercise RAII cleanup");
    }));
    assert!(failure.is_err());
    assert_eq!(Arc::strong_count(&stopped), 1);

    // Fail an assertion while the small-buffer writer has a non-reading peer.
    // Drop must stop and join it even though the response cannot be completed.
    let server = serve(vec![b'x'; 8_000_000], Duration::ZERO);
    let endpoint = reqwest::Url::parse(&server.endpoint).unwrap();
    let address = format!("127.0.0.1:{}", endpoint.port().unwrap())
        .parse()
        .unwrap();
    let mut peer = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(1)).unwrap();
    peer.set_write_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    peer.write_all(b"POST / HTTP/1.1\r\nContent-Length: 0\r\n\r\n")
        .unwrap();
    server.request();
    let started = Instant::now();
    while server.written.load(Ordering::SeqCst) == 0 {
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "writer never started"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(server.written.load(Ordering::SeqCst) < 8_000_000);
    assert!(!server.handle.as_ref().unwrap().is_finished());
    let stopped = server.stop.clone();
    let written = server.written.clone();
    let started = Instant::now();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _owned = server;
        panic!("deliberate assertion failure during a backpressured response");
    }));
    assert!(failure.is_err());
    assert!(stopped.load(Ordering::SeqCst));
    assert_eq!(Arc::strong_count(&stopped), 1);
    assert!(written.load(Ordering::SeqCst) < 8_000_000);
    assert!(started.elapsed() < Duration::from_secs(1));
    eprintln!(
        "backpressured assertion cleanup: {} wire bytes, joined in {:?}",
        written.load(Ordering::SeqCst),
        started.elapsed()
    );
    drop(peer);
}
#[test]
fn documented_reference_manifest_generates_loads_and_transcribes_controlled_http() {
    let dir = tempfile::tempdir().unwrap();
    let generated = dir.path().join("reference.json");
    let examples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/plugins");
    let result = std::process::Command::new("/usr/bin/python3")
        .arg("-I")
        .arg(examples.join("prepare_manifest.py"))
        .arg("--output")
        .arg(&generated)
        .env_clear()
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let manifest = crate::plugins::read_manifest(&generated).unwrap();
    let mut store = Store::default();
    store.import(manifest.clone()).unwrap();
    let server = serve(
        response("200 OK", br#"{"text":"reference transport fixture"}"#),
        Duration::ZERO,
    );
    let text = crate::plugin_process::execute(
        &manifest,
        &config(server.endpoint.clone()),
        &sample(),
        "en",
        &CancellationToken::new(),
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(text, "reference transport fixture");
    let request = server.request();
    let text = String::from_utf8_lossy(&request);
    assert!(text.contains("Bearer synthetic-independent-key"));
    assert!(text.contains("fixture-model"));
    assert!(!text.to_lowercase().contains("chatgpt-account-id"));
    assert!(!text.to_lowercase().contains("originator:"));
    assert!(request.windows(sample().len()).any(|w| w == sample()));
    server.finish_blocking();
}
#[test]
fn wav_conversion_is_bounded_and_preserves_pcm() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("synthetic.wav");
    let original = wav(&[0., 0.5, -0.5, 1., -1.]).unwrap();
    std::fs::write(&file, &original).unwrap();
    let converted = convert(&file, &CancellationToken::new()).unwrap();
    assert_eq!(&converted[..44], &original[..44]);
    assert_eq!(&converted[44..], &original[44..]);
    assert!(wav(&[]).is_err());
    assert!(wav(&[f32::NAN]).is_err());
    assert!(wav(&vec![0.; MAX_SAMPLES + 1]).is_err());
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(convert(&file, &cancel).is_err());
}
