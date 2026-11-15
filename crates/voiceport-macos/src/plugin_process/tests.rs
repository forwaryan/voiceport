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
fn run(script: &str) -> Result<String, Failure> {
    execute(
        &manifest(script),
        &BTreeMap::new(),
        &crate::plugin_runtime::sample(),
        "en",
        &CancellationToken::new(),
        Duration::from_secs(3),
    )
}
/// Opt-in live QQ verification. Inputs must be owned synthetic speech; never
/// starts QQ, logs in, records a microphone, or runs in the normal test suite.
#[test]
#[ignore = "requires explicit QQ bridge, synthetic WAV, and upload authorization"]
fn qq_smartptt_live_owned_process() {
    use std::os::unix::fs::OpenOptionsExt;
    let input = |name: &str| std::path::PathBuf::from(std::env::var(name).expect(name));
    let manifest = read_manifest(&input("VOICEPORT_QQ_PROBE_MANIFEST")).unwrap();
    assert_eq!(manifest.id, "qq.smartptt");
    manifest.validate_program().unwrap();
    let config: BTreeMap<String, String> =
        serde_json::from_slice(&std::fs::read(input("VOICEPORT_QQ_PROBE_CONFIG")).unwrap())
            .unwrap();
    let wav = std::fs::read(input("VOICEPORT_QQ_PROBE_WAV")).unwrap();
    let result_path = input("VOICEPORT_QQ_PROBE_RESULT");
    let mut attempt = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(result_path.with_extension("attempt.json"))
        .unwrap();
    attempt
        .write_all(b"{\"operation\":\"live_qq_host_execute_owned\",\"synthetic_audio\":true}\n")
        .unwrap();
    attempt.sync_all().unwrap();
    let custody = Custody::default();
    let started = Instant::now();
    let result = execute_owned(
        &manifest,
        &config,
        &wav,
        "zh",
        &CancellationToken::new(),
        Duration::from_secs(110),
        &custody,
    );
    let report = serde_json::json!({
        "elapsed_ms": started.elapsed().as_millis(),
        "custody_unresolved": custody.unresolved(),
        "text": result.as_ref().ok(),
        "error": result.as_ref().err().map(ToString::to_string),
    });
    let mut output = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(result_path)
        .unwrap();
    output
        .write_all(&serde_json::to_vec_pretty(&report).unwrap())
        .unwrap();
    output.sync_all().unwrap();
    let text = result.unwrap();
    assert!(!text.trim().is_empty());
    if let Ok(expected) = std::env::var("VOICEPORT_QQ_PROBE_EXPECTED") {
        assert!(text.contains(&expected), "unexpected native recognition");
    }
    assert!(
        !custody.unresolved(),
        "adapter process group must be released"
    );
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
        assert!(!error.message.contains("synthetic-secret"));
    }
}
#[test]
fn qq_errors_explain_recovery_without_exposing_raw_codes() {
    let cases = [
        ("bridge_unavailable", "未运行或连接地址已失效"),
        ("bridge_disconnected", "连接已中断"),
        ("qq_bridge_stopped", "连接已中断"),
        ("not_ready", "尚未就绪"),
        ("qq_not_ready", "尚未就绪"),
        ("unauthorized", "配对密钥"),
        ("busy", "上一项任务"),
        ("qq_busy", "上一项任务"),
        ("qq_cleanup_failed", "清理"),
        ("encoding_failed", "编码依赖"),
        ("timeout", "确认桥接任务已结束"),
        ("qq_smartptt_timeout", "确认桥接任务已结束"),
        ("qq_smartptt_error", "未能完成转写"),
        ("qq_smartptt_call_failed", "未能完成转写"),
        ("transcription_failed", "未能完成转写"),
    ];
    for (code, expected) in cases {
        let bytes = format!("{{\"protocol_version\":1,\"event\":\"error\",\"code\":\"{code}\"}}\n");
        let message = parse(bytes.as_bytes(), &BTreeMap::new(), "qq.smartptt").unwrap_err();
        assert!(message.contains(expected), "{code}: {message}");
        let other = parse(bytes.as_bytes(), &BTreeMap::new(), "example.synthetic").unwrap_err();
        assert_eq!(other, "插件报告转写失败；请检查服务或适配器配置。");
    }
    for code in [
        "synthetic-secret",
        "bridge_unavailable:synthetic-secret",
        "unauthorized=synthetic-secret",
    ] {
        let bytes = format!("{{\"protocol_version\":1,\"event\":\"error\",\"code\":\"{code}\"}}\n");
        let message = parse(bytes.as_bytes(), &BTreeMap::new(), "qq.smartptt").unwrap_err();
        assert_eq!(message, "插件报告转写失败；请检查服务或适配器配置。");
        assert!(!message.contains("synthetic-secret"));
    }
}

#[test]
fn qq_missing_bridge_error_survives_real_owned_plugin_execution() {
    let script = r#"import json,sys
r=json.loads(sys.stdin.readline())
assert r['plugin_id']=='qq.smartptt'
sys.stderr.write('synthetic-private-stderr')
print(json.dumps({'protocol_version':1,'event':'error','code':'bridge_unavailable'}))"#;
    let mut plugin = manifest(script);
    plugin.id = "qq.smartptt".into();
    let custody = Custody::default();
    let error = execute_owned(
        &plugin,
        &BTreeMap::new(),
        &crate::plugin_runtime::sample(),
        "zh",
        &CancellationToken::new(),
        Duration::from_secs(3),
        &custody,
    )
    .unwrap_err();
    assert!(error.message.contains("普通 QQ 不会自动提供桥接"));
    assert!(!error.message.contains("synthetic-private"));
    assert!(error.cleanup.is_none());
    assert!(!custody.unresolved());
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
            error
                .message
                .contains(if cancel_case { "取消" } else { "超时" }),
            "{error}"
        );
        assert!(now.elapsed() < Duration::from_secs(2));
        assert!(
            error.cleanup.is_none(),
            "cleanup was not confirmed: {error}"
        );
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

// Cleanup confirmation controls: also extracted byte-for-byte for standalone
// validation without the package build slot.
struct ScriptedTermination {
    observations: std::collections::VecDeque<Result<bool, CleanupIssue>>,
    signal_result: Result<(), CleanupIssue>,
    reap_result: Result<bool, CleanupIssue>,
    signals: usize,
    observations_taken: usize,
}
impl ScriptedTermination {
    fn new(observations: impl IntoIterator<Item = Result<bool, CleanupIssue>>) -> Self {
        Self {
            observations: observations.into_iter().collect(),
            signal_result: Ok(()),
            reap_result: Ok(true),
            signals: 0,
            observations_taken: 0,
        }
    }
}
impl Termination for ScriptedTermination {
    fn observe(&mut self) -> Result<bool, CleanupIssue> {
        self.observations_taken += 1;
        self.observations
            .pop_front()
            .expect("no implicit observation after trace ends")
    }
    fn signal(&mut self) -> Result<(), CleanupIssue> {
        self.signals += 1;
        self.signal_result
    }
    fn reap(&mut self) -> Result<bool, CleanupIssue> {
        self.reap_result
    }
}
#[test]
fn cleanup_confirms_pending_termination_before_returning() {
    let mut trace = ScriptedTermination::new([Ok(false), Ok(false), Ok(true)]);
    confirm_termination(&mut trace, CONFIRMATION_BUDGET).unwrap();
    assert_eq!((trace.signals, trace.observations_taken), (1, 3));
}
#[test]
fn cleanup_budget_exhaustion_does_not_claim_stopped() {
    let mut trace = ScriptedTermination::new([Ok(false), Ok(false)]);
    assert_eq!(
        confirm_termination(&mut trace, Duration::ZERO),
        Err(CleanupIssue::Pending)
    );
    assert_eq!((trace.signals, trace.observations_taken), (1, 2));
    let mut persistent = ScriptedTermination::new(std::iter::repeat_n(Ok(false), 256));
    let started = Instant::now();
    assert_eq!(
        confirm_termination(&mut persistent, CONFIRMATION_BUDGET),
        Err(CleanupIssue::Pending)
    );
    assert!(started.elapsed() >= CONFIRMATION_BUDGET);
    assert_eq!(persistent.signals, 1);
    assert!(persistent.observations_taken > 1 && persistent.observations_taken < 256);
}
#[test]
fn cleanup_never_retries_through_uncertainty_signal_or_reap_errors() {
    for issue in [
        CleanupIssue::List,
        CleanupIssue::Truncated,
        CleanupIssue::State,
        CleanupIssue::Identity,
        CleanupIssue::Membership,
    ] {
        let mut trace = ScriptedTermination::new([Err(issue)]);
        assert_eq!(
            confirm_termination(&mut trace, CONFIRMATION_BUDGET),
            Err(issue)
        );
        assert_eq!((trace.signals, trace.observations_taken), (1, 1));
    }
    let mut trace = ScriptedTermination::new([Ok(false)]);
    trace.signal_result = Err(CleanupIssue::Signal);
    assert_eq!(
        confirm_termination(&mut trace, CONFIRMATION_BUDGET),
        Err(CleanupIssue::Signal)
    );
    assert_eq!((trace.signals, trace.observations_taken), (1, 1));
    let mut trace = ScriptedTermination::new([Ok(false)]);
    trace.reap_result = Err(CleanupIssue::Reap);
    assert_eq!(
        confirm_termination(&mut trace, CONFIRMATION_BUDGET),
        Err(CleanupIssue::Reap)
    );
    assert_eq!((trace.signals, trace.observations_taken), (1, 1));
}
#[test]
fn cleanup_already_terminal_group_needs_reaping_and_no_signal() {
    let mut trace = ScriptedTermination::new([Ok(true), Ok(true)]);
    confirm_termination(&mut trace, Duration::ZERO).unwrap();
    assert_eq!(trace.signals, 0);
    let mut trace = ScriptedTermination::new([Ok(true), Ok(true)]);
    trace.reap_result = Ok(false);
    assert_eq!(
        confirm_termination(&mut trace, Duration::ZERO),
        Err(CleanupIssue::Pending)
    );
    assert_eq!(trace.signals, 0);
}
struct ScriptedGroup {
    lists: std::collections::VecDeque<Result<Vec<i32>, CleanupIssue>>,
    states: std::collections::VecDeque<Result<Option<MemberState>, CleanupIssue>>,
}
impl GroupProbe for ScriptedGroup {
    fn members(&mut self, _: i32) -> Result<Vec<i32>, CleanupIssue> {
        self.lists.pop_front().unwrap()
    }
    fn state(&mut self, _: i32) -> Result<Option<MemberState>, CleanupIssue> {
        self.states.pop_front().unwrap()
    }
}
fn member(status: u32) -> MemberState {
    MemberState {
        pid: 42,
        pgid: 41,
        start: (123, 456),
        status,
    }
}
#[test]
fn cleanup_group_requires_matching_zombie_evidence_and_preserves_r1() {
    for state in [
        Some(member(libc::SZOMB)),
        None,
        Some(member(2)),
        Some(member(4)),
    ] {
        let expected = match state {
            None => Err(CleanupIssue::Membership),
            Some(info) => Ok(info.status == libc::SZOMB),
        };
        let mut group = ScriptedGroup {
            lists: [Ok(vec![42]), Ok(vec![42])].into(),
            states: [Ok(state)].into(),
        };
        assert_eq!(
            observe_group(41, &mut GroupHistory::default(), &mut group),
            expected
        );
    }
    let mut empty = ScriptedGroup {
        lists: [Ok(vec![]), Ok(vec![])].into(),
        states: [].into(),
    };
    assert_eq!(
        observe_group(41, &mut GroupHistory::default(), &mut empty),
        Ok(true)
    );
    let mut gone = ScriptedGroup {
        lists: [Ok(vec![42]), Ok(vec![])].into(),
        states: [Ok(None)].into(),
    };
    assert_eq!(
        observe_group(41, &mut GroupHistory::default(), &mut gone),
        Ok(true)
    );
}
#[test]
fn cleanup_group_rejects_identity_change_and_unverified_final_members() {
    let mut changed = member(libc::SZOMB);
    changed.start.0 += 1;
    let mut group = ScriptedGroup {
        lists: [Ok(vec![42]), Ok(vec![42]), Ok(vec![42])].into(),
        states: [Ok(Some(member(2))), Ok(Some(changed))].into(),
    };
    let mut history = GroupHistory::default();
    assert_eq!(observe_group(41, &mut history, &mut group), Ok(false));
    assert_eq!(
        observe_group(41, &mut history, &mut group),
        Err(CleanupIssue::Identity)
    );
    let mut group = ScriptedGroup {
        lists: [Ok(vec![42]), Ok(vec![42, 43])].into(),
        states: [Ok(Some(member(libc::SZOMB)))].into(),
    };
    assert_eq!(
        observe_group(41, &mut GroupHistory::default(), &mut group),
        Err(CleanupIssue::Membership)
    );
    for (pid, pgid) in [(43, 41), (42, 99)] {
        let mut changed = member(libc::SZOMB);
        changed.pid = pid;
        changed.pgid = pgid;
        let mut group = ScriptedGroup {
            lists: [Ok(vec![42])].into(),
            states: [Ok(Some(changed))].into(),
        };
        assert_eq!(
            observe_group(41, &mut GroupHistory::default(), &mut group),
            Err(CleanupIssue::Identity)
        );
    }
}
#[test]
fn cleanup_group_rejects_reappeared_esrch_across_confirmation_passes() {
    let mut group = ScriptedGroup {
        lists: [Ok(vec![42]), Ok(vec![]), Ok(vec![42])].into(),
        states: [Ok(None)].into(),
    };
    let mut history = GroupHistory::default();
    assert_eq!(observe_group(41, &mut history, &mut group), Ok(true));
    assert_eq!(
        observe_group(41, &mut history, &mut group),
        Err(CleanupIssue::Membership)
    );
}
#[test]
fn cleanup_native_owned_group_is_terminal_at_return() {
    let custody = Custody::default();
    let leader = Command::new("/bin/sleep")
        .arg("10")
        .env_clear()
        .process_group(0)
        .spawn()
        .unwrap();
    let pgid = leader.id() as i32;
    custody.install(leader);
    let _guard = CleanupGuard(custody.clone());
    let _first = ReapFixture::spawn(pgid);
    let _second = ReapFixture::spawn(pgid);
    custody.finish_initial().unwrap();
    assert!(!custody.unresolved());
    let result = unsafe { libc::kill(pgid, 0) };
    let errno = std::io::Error::last_os_error().raw_os_error();
    assert_eq!((result, errno), (-1, Some(libc::ESRCH)));
    group_has_no_live_members(pgid).unwrap(); // Immediate independent oracle.
}
#[test]
fn cleanup_guard_confirms_owned_child_during_unwind() {
    let custody = Custody::default();
    let child = Command::new("/bin/sleep")
        .arg("10")
        .env_clear()
        .process_group(0)
        .spawn()
        .unwrap();
    let pid = child.id() as i32;
    custody.install(child);
    let result = std::panic::catch_unwind(|| {
        let _guard = CleanupGuard(custody.clone());
        panic!("deliberate cleanup guard unwind");
    });
    assert!(result.is_err());
    assert!(!custody.unresolved());
    let result = unsafe { libc::kill(pid, 0) };
    let errno = std::io::Error::last_os_error().raw_os_error();
    assert_eq!((result, errno), (-1, Some(libc::ESRCH)));
    group_has_no_live_members(pid).unwrap();
}
#[test]
fn cleanup_observation_does_not_signal_or_renew_initial_cleanup() {
    let child = cleanup_fixture::PendingChild::new();
    assert!(!child.custody.observe_once());
    assert!(child.custody.unresolved());
    assert_eq!(child.custody.finish_initial(), Err(CleanupIssue::Pending));
    assert!(!child.custody.observe_once()); // Still alive: no hidden signal.
    child.terminate(); // Only the fresh fixture's explicit owner signals it.
    assert!(child.custody.observe_once());
    assert!(!child.custody.unresolved());
}
