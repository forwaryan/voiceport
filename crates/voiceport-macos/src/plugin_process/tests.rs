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
fn group_members(pgid: i32) -> Result<Vec<i32>, String> {
    if pgid <= 1 {
        return Err("expected an owned process group".into());
    }
    let mut members = [0i32; 64];
    let count = unsafe {
        // libproc maps list errors to zero, also the valid empty-list result.
        *libc::__error() = 0;
        libc::proc_listpgrppids(
            pgid,
            members.as_mut_ptr().cast(),
            std::mem::size_of_val(&members) as i32,
        )
    };
    let errno = std::io::Error::last_os_error().raw_os_error();
    if count < 0 || (count == 0 && errno != Some(0)) {
        return Err(format!("group {pgid}: list failed: {errno:?}"));
    }
    if count as usize >= members.len() {
        return Err(format!("group {pgid}: member list may be truncated"));
    }
    Ok(members[..count as usize].to_vec())
}

fn group_has_no_live_members(pgid: i32) -> Result<(), String> {
    // kill(-pgid, 0) reports EPERM for a macOS group containing only zombies.
    // Reaping our direct child cannot reap descendants owned by another parent.
    let members = group_members(pgid)?;
    let mut verified_members = Vec::new();
    for &pid in &members {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = std::mem::size_of_val(&info) as i32;
        let count = unsafe {
            // Nonzero arg explicitly includes zombies in PROC_PIDTBSDINFO.
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                1,
                info.as_mut_ptr().cast(),
                size,
            )
        };
        let errno = std::io::Error::last_os_error().raw_os_error();
        if count == 0 && errno == Some(libc::ESRCH) {
            continue; // A listed member was reaped before its state query.
        }
        if count != size {
            return Err(format!(
                "group {pgid}: member {pid} state failed: {errno:?}"
            ));
        }
        let info = unsafe { info.assume_init() };
        if info.pbi_pid != pid as u32 || info.pbi_pgid != pgid as u32 {
            return Err(format!("group {pgid}: member {pid} identity changed"));
        }
        if info.pbi_status != libc::SZOMB {
            return Err(format!(
                "group {pgid}: live member {pid}, state={}",
                info.pbi_status
            ));
        }
        verified_members.push(pid);
        eprintln!(
            "group {pgid}: terminated member {pid}, state={}",
            info.pbi_status
        );
    }
    // Every remaining member needs zombie-state evidence, including a PID
    // listed again after its state query returned ESRCH.
    if group_members(pgid)?
        .iter()
        .any(|pid| !verified_members.contains(pid))
    {
        return Err(format!(
            "group {pgid}: membership changed during observation"
        ));
    }
    Ok(())
}

struct ReapFixture(Child);
impl ReapFixture {
    fn spawn(pgid: i32) -> Self {
        Self(
            Command::new("/bin/sleep")
                .arg("10")
                .env_clear()
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(pgid)
                .spawn()
                .unwrap(),
        )
    }
}
impl Drop for ReapFixture {
    fn drop(&mut self) {
        let pid = self.0.id();
        let _ = self.0.kill();
        let status = self.0.wait();
        eprintln!("owned group fixture cleanup: pid={pid} wait={status:?}");
    }
}

#[test]
fn group_fixture_reaps_children_during_assertion_unwind() {
    let mut pids = vec![];
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let leader = ReapFixture::spawn(0);
        pids.push(leader.0.id() as i32);
        let member = ReapFixture::spawn(pids[0]);
        pids.push(member.0.id() as i32);
        panic!("deliberate assertion unwind with owned group members");
    }));
    assert!(failure.is_err());
    assert_eq!(pids.len(), 2);
    for &pid in &pids {
        let result = unsafe { libc::kill(pid, 0) };
        let errno = std::io::Error::last_os_error().raw_os_error();
        assert_eq!((result, errno), (-1, Some(libc::ESRCH)));
    }
    group_has_no_live_members(pids[0]).unwrap();
}

#[test]
fn group_liveness_distinguishes_live_members_from_unreaped_zombies() {
    let mut leader = ReapFixture::spawn(0);
    let pgid = leader.0.id() as i32;
    let mut member = ReapFixture::spawn(pgid);
    let member_pid = member.0.id() as i32;
    assert!(group_has_no_live_members(pgid).is_err());
    leader.0.kill().unwrap();
    leader.0.wait().unwrap();
    assert!(group_has_no_live_members(pgid).is_err());
    member.0.kill().unwrap();
    let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
    assert_eq!(
        unsafe {
            libc::waitid(
                libc::P_PID,
                member_pid as u32,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        },
        0
    );
    assert_eq!(unsafe { info.si_pid() }, member_pid);
    let positive = unsafe { libc::kill(pgid, 0) };
    let positive_errno = std::io::Error::last_os_error().raw_os_error();
    let group = unsafe { libc::kill(-pgid, 0) };
    let group_errno = std::io::Error::last_os_error().raw_os_error();
    eprintln!(
        "owned zombie control: leader={pgid} member={member_pid} positive=({positive},{positive_errno:?}) group=({group},{group_errno:?})"
    );
    assert_eq!((positive, positive_errno), (-1, Some(libc::ESRCH)));
    assert_eq!(group_members(pgid).unwrap(), vec![member_pid]);
    group_has_no_live_members(pgid).expect("a zombie-only owned group has no live members");
    member.0.wait().unwrap();
    group_has_no_live_members(pgid).unwrap();
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
        let result = unsafe { libc::kill(pid, 0) };
        let errno = std::io::Error::last_os_error().raw_os_error();
        assert_eq!(
            (result, errno),
            (-1, Some(libc::ESRCH)),
            "cancel={cancel_case} direct child {pid} was not reaped"
        );
        group_has_no_live_members(pid)
            .unwrap_or_else(|error| panic!("cancel={cancel_case} pid={pid}: {error}"));
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
