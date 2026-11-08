use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
};
fn serve(
    response: Vec<u8>,
    delay: Duration,
) -> (String, mpsc::Receiver<Vec<u8>>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!(
        "http://{}/full/transcriptions",
        listener.local_addr().unwrap()
    );
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut bytes = vec![];
        let mut chunk = [0; 8192];
        let header_end = loop {
            let n = stream.read(&mut chunk).unwrap();
            assert_ne!(n, 0);
            bytes.extend_from_slice(&chunk[..n]);
            if let Some(i) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let headers = String::from_utf8_lossy(&bytes[..header_end]).to_lowercase();
        let length = headers
            .lines()
            .find_map(|l| l.strip_prefix("content-length: "))
            .unwrap()
            .parse::<usize>()
            .unwrap();
        while bytes.len() < header_end + length {
            let n = stream.read(&mut chunk).unwrap();
            assert_ne!(n, 0);
            bytes.extend_from_slice(&chunk[..n]);
        }
        tx.send(bytes).unwrap();
        thread::sleep(delay);
        let _ = stream.write_all(&response);
        drop(stream);
        listener.set_nonblocking(true).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_millis(100);
        while std::time::Instant::now() < deadline {
            assert!(
                listener.accept().is_err(),
                "implicit extra request attempted"
            );
            thread::sleep(Duration::from_millis(5));
        }
    });
    (endpoint, rx, handle)
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
    let (endpoint, rx, handle) = serve(
        response("200 OK", br#"{"text":" fixture "}"#),
        Duration::ZERO,
    );
    let sample = sample();
    let result = http(
        &config(endpoint),
        sample.clone(),
        "zh",
        &CancellationToken::new(),
        Duration::from_secs(2),
        Duration::from_secs(1),
    )
    .await
    .unwrap();
    assert_eq!(result, "fixture");
    let request = rx.recv().unwrap();
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
    handle.join().unwrap();
}
#[tokio::test]
async fn no_key_and_auto_language_are_omitted_and_empty_text_is_valid() {
    let (endpoint, rx, handle) = serve(response("200 OK", br#"{"text":""}"#), Duration::ZERO);
    let mut cfg = config(endpoint);
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
    let request = String::from_utf8_lossy(&rx.recv().unwrap()).to_string();
    assert!(!request.contains("authorization:"));
    assert!(!request.contains("name=\"language\""));
    handle.join().unwrap();
}
#[tokio::test]
async fn malformed_oversize_failure_and_redirect_are_bounded() {
    for bytes in [response("200 OK",b"[]"),response("200 OK",br#"{"text":7}"#),response("200 OK",b"bad-json"),response("401 Unauthorized",b"synthetic-independent-key"),b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/never\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",MAX_RESPONSE+1).into_bytes(),response("200 OK",&vec![b'x';MAX_RESPONSE+1])]{let (endpoint,rx,handle)=serve(bytes,Duration::ZERO);let error=http(&config(endpoint),sample(),"",&CancellationToken::new(),Duration::from_secs(2),Duration::from_secs(1)).await.unwrap_err();assert!(!error.contains("synthetic-independent-key"));rx.recv().unwrap();handle.join().unwrap();}
}
#[tokio::test]
async fn redirect_status_is_not_followed_and_undeclared_response_size_is_bounded() {
    for redirect in [true, false] {
        let bytes = if redirect {
            b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/never\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec()
        } else {
            let mut body = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
            body.extend(vec![b'x'; MAX_RESPONSE + 1]);
            body
        };
        let (endpoint, rx, handle) = serve(bytes, Duration::ZERO);
        let error = http(
            &config(endpoint),
            sample(),
            "",
            &CancellationToken::new(),
            Duration::from_secs(2),
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert!(
            error.contains(if redirect {
                "HTTP 302"
            } else {
                "超过大小限制"
            }),
            "{error}"
        );
        rx.recv().unwrap();
        handle.join().unwrap();
    }
}
#[tokio::test]
async fn timeout_and_cancel_drop_network_operation() {
    for cancel_case in [false, true] {
        let (endpoint, rx, handle) = serve(
            response("200 OK", br#"{"text":"late"}"#),
            Duration::from_millis(150),
        );
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let canceler = thread::spawn(move || {
            if cancel_case {
                thread::sleep(Duration::from_millis(30));
                token.cancel();
            }
        });
        let now = std::time::Instant::now();
        let error = http(
            &config(endpoint),
            sample(),
            "",
            &cancel,
            Duration::from_millis(80),
            Duration::from_millis(60),
        )
        .await
        .unwrap_err();
        assert!(now.elapsed() < Duration::from_secs(1));
        if cancel_case {
            assert!(error.contains("取消"));
        }
        rx.recv().unwrap();
        canceler.join().unwrap();
        handle.join().unwrap();
    }
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
    let (endpoint, rx, handle) = serve(
        response("200 OK", br#"{"text":"reference transport fixture"}"#),
        Duration::ZERO,
    );
    let text = crate::plugin_process::execute(
        &manifest,
        &config(endpoint),
        &sample(),
        "en",
        &CancellationToken::new(),
        Duration::from_secs(5),
    )
    .unwrap();
    assert_eq!(text, "reference transport fixture");
    let request = rx.recv().unwrap();
    let text = String::from_utf8_lossy(&request);
    assert!(text.contains("Bearer synthetic-independent-key"));
    assert!(text.contains("fixture-model"));
    assert!(!text.to_lowercase().contains("chatgpt-account-id"));
    assert!(!text.to_lowercase().contains("originator:"));
    assert!(request.windows(sample().len()).any(|w| w == sample()));
    handle.join().unwrap();
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
