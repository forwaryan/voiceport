use super::*;
use crate::plugins::*;
fn manifest(script: &str) -> Manifest {
    Manifest {
        schema_version: 1,
        protocol_version: 1,
        id: "example.synthetic".into(),
        version: "1.0".into(),
        name: "Synthetic fixture".into(),
        origin: "Local development fixture; not ASR".into(),
        adapter: Adapter::Jsonl,
        capabilities: Capabilities {
            batch: true,
            streaming: false,
            system_preview: false,
        },
        fields: vec![],
        program: Some(Program {
            executable: "/usr/bin/python3".into(),
            args: vec!["-I".into(), "-c".into(), script.into()],
        }),
    }
}
fn run(script: &str) -> Result<String, String> {
    execute(
        &manifest(script),
        &BTreeMap::new(),
        &crate::plugin_runtime::sample(),
        "en",
        &CancellationToken::new(),
        Duration::from_secs(3),
    )
}
#[test]
fn jsonl_audio_config_and_environment_are_real() {
    let script = r#"import json,sys,os,base64,struct
r=json.loads(sys.stdin.readline(8000001))
assert r['protocol_version']==1 and r['plugin_id']=='example.synthetic'
assert r['config']=={'api_key':'synthetic-only'} and r['language']=='en'
w=base64.b64decode(r['audio']['base64'],validate=True)
assert w[:4]==b'RIFF' and struct.unpack('<I',w[24:28])[0]==16000
assert not any(k in os.environ for k in ['HOME','CODEX_HOME','OPENAI_API_KEY','HTTPS_PROXY','ORCA_TERMINAL_HANDLE'])
print(json.dumps({'protocol_version':1,'event':'final','text':'synthetic transport only'}))"#;
    assert_eq!(
        execute(
            &manifest(script),
            &BTreeMap::from([("api_key".into(), "synthetic-only".into())]),
            &crate::plugin_runtime::sample(),
            "en",
            &CancellationToken::new(),
            Duration::from_secs(3)
        )
        .unwrap(),
        "synthetic transport only"
    );
}
#[test]
fn errors_and_output_limits_do_not_leak_stderr() {
    for script in [
        "import sys;sys.stdin.read();sys.stderr.write('synthetic-secret');sys.exit(2)",
        "import sys;sys.stdin.read();print('bad json')",
        "import sys;sys.stdin.read();print('{\"protocol_version\":2,\"event\":\"final\",\"text\":\"x\"}')",
        "import sys;sys.stdin.read();print('{\"protocol_version\":1,\"event\":\"error\",\"code\":\"failed\"}')",
        "import sys;sys.stdin.read();print('x'*1000001)",
        "import sys;sys.stdin.read();sys.stderr.write('x'*65537)",
        "import sys;sys.stdin.read();print('{}\\n{}')",
    ] {
        let error = run(script).unwrap_err();
        assert!(!error.contains("synthetic-secret"));
    }
}
#[test]
fn cancellation_and_timeout_reap_owned_process_group() {
    // Initialization deliberately exceeds both deadlines. Reaping must not
    // depend on the interpreter having written a readiness/PID file.
    for cancel_case in [false, true] {
        let m = manifest("import sys,time;time.sleep(0.7);sys.stdin.read();time.sleep(10)");
        let cancel = CancellationToken::new();
        let token = cancel.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        let watcher = std::thread::spawn(move || {
            rx.recv_timeout(Duration::from_secs(2)).unwrap();
            if cancel_case {
                std::thread::sleep(Duration::from_millis(300));
                token.cancel();
            }
        });
        let mut pid = None;
        let now = Instant::now();
        let result = execute_observed(
            &m,
            &BTreeMap::new(),
            &crate::plugin_runtime::sample(),
            "",
            &cancel,
            Duration::from_millis(500),
            |spawned| {
                pid = Some(spawned as i32);
                tx.send(()).unwrap();
            },
        );
        watcher.join().unwrap();
        let error = result.unwrap_err();
        assert!(
            error.contains(if cancel_case { "取消" } else { "超时" }),
            "{error}"
        );
        assert!(now.elapsed() < Duration::from_secs(2));
        let pid = pid.expect("adapter was actually spawned");
        for target in [pid, -pid] {
            assert_eq!(unsafe { libc::kill(target, 0) }, -1, "PID/group {target}");
            assert_eq!(
                std::io::Error::last_os_error().raw_os_error(),
                Some(libc::ESRCH)
            );
        }
    }
}
#[test]
fn import_validation_is_nonexecuting_and_rejects_version_schema_duplicates_paths() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("must-not-exist");
    let mut m = manifest("import sys;open(sys.argv[1],'w').write('executed')");
    m.program
        .as_mut()
        .unwrap()
        .args
        .push(marker.to_str().unwrap().into());
    let file = dir.path().join("manifest.json");
    std::fs::write(&file, serde_json::to_vec(&m).unwrap()).unwrap();
    let loaded = read_manifest(&file).unwrap();
    let mut store = Store::default();
    store.import(loaded.clone()).unwrap();
    assert!(!marker.exists());
    assert!(store.import(loaded).is_err());
    m.protocol_version = 2;
    assert!(m.validate_external().is_err());
    m.protocol_version = 1;
    m.capabilities.streaming = true;
    assert!(m.validate_external().is_err());
    m.capabilities.streaming = false;
    m.program.as_mut().unwrap().executable = "/missing/plugin".into();
    assert!(m.validate_program().is_err());
    m.program.as_mut().unwrap().executable = "relative".into();
    assert!(m.validate_external().is_err());
    let mut json = serde_json::to_value(manifest("pass")).unwrap();
    json["fields"] = serde_json::json!([{"id":"x","label":"X","kind":"number","required":false}]);
    std::fs::write(&file, serde_json::to_vec(&json).unwrap()).unwrap();
    assert!(read_manifest(&file).is_err());
}
