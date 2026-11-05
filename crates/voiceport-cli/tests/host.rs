use std::{
    io::Write,
    process::{Command, Stdio},
};
#[test]
fn real_host_is_lazy_and_shuts_down_with_input_open() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("auth-must-not-be-read");
    let mut child = Command::new(env!("CARGO_BIN_EXE_voiceport-rs"))
        .args(["host", "--auth"])
        .arg(&missing)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(b"{\"version\":1,\"action\":\"status\",\"request_id\":\"q\"}\n{\"version\":1,\"action\":\"shutdown\",\"request_id\":\"end\"}\n").unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            panic!("shutdown waited for stdin EOF");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let result = child.wait_with_output().unwrap();
    assert!(result.status.success());
    let events: Vec<serde_json::Value> = result
        .stdout
        .split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_slice(l).unwrap())
        .collect();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["event"], "ready");
    assert_eq!(events[1]["event"], "status");
    assert_eq!(events[2]["event"], "shutdown");
    assert!(!missing.exists());
}
