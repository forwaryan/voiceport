//! One owned process group, nonblocking pipes, no reader threads, bounded JSONL v1.
use crate::{
    plugin_runtime::{MAX_RESPONSE, check},
    plugins::Manifest,
};
use base64::Engine;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use voiceport_core::CancellationToken;
const MAX_INPUT: usize = 8_000_000;
const MAX_STDERR: usize = 65_536;
const CONFIRMATION_BUDGET: Duration = Duration::from_millis(100);
pub(crate) const CLEANUP_WARNING: &str =
    "无法确认插件进程组已终止；录音仍被保留，开始、重试和退出已暂停。再次点取消可检查一次。";

/// A caller must establish its clone before dispatching the worker. Neither a
/// completion channel nor the Job is the sole custodian of a spawned child.
#[derive(Clone, Default)]
pub(crate) struct Custody(Arc<CustodyState>);
#[derive(Default)]
struct CustodyState {
    owned: Mutex<Option<Owned>>,
    unresolved: AtomicBool,
    failure: Mutex<String>,
    #[cfg(test)]
    observations: std::sync::atomic::AtomicUsize,
}
impl std::fmt::Debug for Custody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Custody")
            .field("unresolved", &self.unresolved())
            .finish()
    }
}
#[derive(Debug)]
pub(crate) struct Failure {
    pub message: String,
    pub cleanup: Option<Custody>,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)?;
        if self.cleanup.is_some() {
            f.write_str(CLEANUP_WARNING)?;
        }
        Ok(())
    }
}
impl From<String> for Failure {
    fn from(message: String) -> Self {
        Self {
            message,
            cleanup: None,
        }
    }
}
impl From<&str> for Failure {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
impl Custody {
    pub(crate) fn same_owner(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    pub(crate) fn unresolved(&self) -> bool {
        self.0.unresolved.load(Ordering::Acquire)
    }
    pub(crate) fn remember_failure(&self, message: &str) {
        *self.0.failure.lock().unwrap_or_else(|e| e.into_inner()) = message.to_owned();
    }
    pub(crate) fn failure_message(&self) -> String {
        // UI never waits for a worker's native call or mutex.
        self.0
            .failure
            .try_lock()
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| s.clone())
            .unwrap_or_else(|| "插件任务结束，但清理结果尚未确认。".into())
    }
    fn install(&self, child: Child) {
        let mut slot = self.0.owned.lock().unwrap_or_else(|e| e.into_inner());
        assert!(slot.is_none() && !self.unresolved());
        *slot = Some(Owned {
            child,
            started: false,
            reaped: false,
            history: GroupHistory::default(),
        });
        self.0.unresolved.store(true, Ordering::Release);
    }
    fn finish_initial(&self) -> Result<(), CleanupIssue> {
        let mut slot = self.0.owned.lock().unwrap_or_else(|e| e.into_inner());
        let Some(owned) = slot.as_mut() else {
            return Ok(());
        };
        // Explicit cleanup and its RAII fallback share one attempt and budget.
        if owned.started {
            return Err(CleanupIssue::Pending);
        }
        owned.started = true;
        confirm_termination(owned, CONFIRMATION_BUDGET)?;
        slot.take(); // Direct child reaped, group terminal; Drop cannot signal.
        self.0.unresolved.store(false, Ordering::Release);
        Ok(())
    }
    /// One explicit observation pass, no signals, sleeps or renewed budget.
    /// try_lock also covers a disconnected worker still finishing its own call.
    pub(crate) fn observe_once(&self) -> bool {
        #[cfg(test)]
        self.0.observations.fetch_add(1, Ordering::SeqCst);
        let mut slot = match self.0.owned.try_lock() {
            Ok(slot) => slot,
            Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => return false,
        };
        let Some(owned) = slot.as_mut() else {
            return !self.unresolved();
        };
        if !owned.started {
            return false;
        }
        if owned.reap() != Ok(true) || owned.observe() != Ok(true) {
            return false;
        }
        slot.take();
        self.0.unresolved.store(false, Ordering::Release);
        true
    }
}
struct CleanupGuard(Custody);
impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if self.0.unresolved() {
            let _ = self.0.finish_initial();
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CleanupIssue {
    List,
    Truncated,
    State,
    Identity,
    Membership,
    Signal,
    Reap,
    Pending,
}
#[derive(Clone, Copy)]
struct MemberState {
    pid: i32,
    pgid: i32,
    start: (u64, u64),
    status: u32,
}
trait GroupProbe {
    fn members(&mut self, pgid: i32) -> Result<Vec<i32>, CleanupIssue>;
    fn state(&mut self, pid: i32) -> Result<Option<MemberState>, CleanupIssue>;
}
struct NativeProbe;
impl GroupProbe for NativeProbe {
    fn members(&mut self, pgid: i32) -> Result<Vec<i32>, CleanupIssue> {
        if pgid <= 1 {
            return Err(CleanupIssue::Identity);
        }
        let mut members = [0i32; 64];
        let count = unsafe {
            *libc::__error() = 0;
            libc::proc_listpgrppids(
                pgid,
                members.as_mut_ptr().cast(),
                std::mem::size_of_val(&members) as i32,
            )
        };
        let errno = std::io::Error::last_os_error().raw_os_error();
        if count < 0 || (count == 0 && errno != Some(0)) {
            return Err(CleanupIssue::List);
        }
        if count as usize >= members.len() {
            return Err(CleanupIssue::Truncated);
        }
        let members = members[..count as usize].to_vec();
        if members.iter().any(|pid| *pid <= 1) {
            return Err(CleanupIssue::Identity);
        }
        Ok(members)
    }
    fn state(&mut self, pid: i32) -> Result<Option<MemberState>, CleanupIssue> {
        let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
        let size = std::mem::size_of_val(&info) as i32;
        let count = unsafe {
            *libc::__error() = 0;
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
            return Ok(None);
        }
        if count != size {
            return Err(CleanupIssue::State);
        }
        let info = unsafe { info.assume_init() };
        Ok(Some(MemberState {
            pid: info.pbi_pid as i32,
            pgid: info.pbi_pgid as i32,
            start: (info.pbi_start_tvsec, info.pbi_start_tvusec),
            status: info.pbi_status,
        }))
    }
}
#[derive(Default)]
struct GroupHistory {
    initial: Option<BTreeSet<i32>>,
    identities: BTreeMap<i32, (u64, u64)>,
    vanished: BTreeSet<i32>,
}
fn observe_group(
    pgid: i32,
    history: &mut GroupHistory,
    probe: &mut impl GroupProbe,
) -> Result<bool, CleanupIssue> {
    let members = probe.members(pgid)?;
    let initial = history
        .initial
        .get_or_insert_with(|| members.iter().copied().collect());
    let mut verified = BTreeMap::new();
    for pid in members {
        if !initial.contains(&pid) || history.vanished.contains(&pid) {
            return Err(CleanupIssue::Membership);
        }
        let Some(info) = probe.state(pid)? else {
            history.vanished.insert(pid);
            continue;
        };
        if info.pid != pid || info.pgid != pgid {
            return Err(CleanupIssue::Identity);
        }
        if history
            .identities
            .get(&pid)
            .is_some_and(|start| *start != info.start)
        {
            return Err(CleanupIssue::Identity);
        }
        history.identities.insert(pid, info.start);
        if !(1..=libc::SZOMB).contains(&info.status) {
            return Err(CleanupIssue::State);
        }
        verified.insert(pid, info.status == libc::SZOMB);
    }
    let remaining = probe.members(pgid)?;
    // R1: ESRCH is never successful state evidence for a re-listed PID.
    if remaining.iter().any(|pid| !verified.contains_key(pid)) {
        return Err(CleanupIssue::Membership);
    }
    Ok(remaining.iter().all(|pid| verified[pid]))
}
trait Termination {
    fn observe(&mut self) -> Result<bool, CleanupIssue>;
    fn signal(&mut self) -> Result<(), CleanupIssue>;
    fn reap(&mut self) -> Result<bool, CleanupIssue>;
}
fn confirm_termination(owned: &mut impl Termination, budget: Duration) -> Result<(), CleanupIssue> {
    let started = Instant::now();
    let before = owned.observe();
    if before != Ok(true) {
        // The unreaped leader still anchors this group. Signal once even if
        // observation failed, but never hide that uncertainty with later polls.
        let signal = owned.signal();
        let reaped = owned.reap();
        before?;
        signal?;
        reaped?;
    }
    loop {
        let reaped = owned.reap()?;
        if owned.observe()? && reaped {
            return Ok(());
        }
        let remaining = budget
            .checked_sub(started.elapsed())
            .ok_or(CleanupIssue::Pending)?;
        if remaining.is_zero() {
            return Err(CleanupIssue::Pending);
        }
        std::thread::sleep(remaining.min(Duration::from_millis(1)));
        if started.elapsed() >= budget {
            return Err(CleanupIssue::Pending);
        }
    }
}
struct Owned {
    child: Child,
    started: bool,
    reaped: bool,
    history: GroupHistory,
}
impl Termination for Owned {
    fn observe(&mut self) -> Result<bool, CleanupIssue> {
        observe_group(self.child.id() as i32, &mut self.history, &mut NativeProbe)
    }
    fn signal(&mut self) -> Result<(), CleanupIssue> {
        // No wait/try_wait has occurred before the one group signal.
        if self.reaped {
            return Err(CleanupIssue::Identity);
        }
        if unsafe { libc::kill(-(self.child.id() as i32), libc::SIGKILL) } == 0 {
            return Ok(());
        }
        // One direct-child fallback remains safe through the owned Child, even
        // when group signalling fails. Its success cannot clear the group error.
        let _ = self.child.kill();
        Err(CleanupIssue::Signal)
    }
    fn reap(&mut self) -> Result<bool, CleanupIssue> {
        if !self.reaped {
            self.reaped = self
                .child
                .try_wait()
                .map_err(|_| CleanupIssue::Reap)?
                .is_some();
        }
        Ok(self.reaped)
    }
}
impl Drop for Owned {
    fn drop(&mut self) {
        if !self.started {
            self.started = true;
            let _ = confirm_termination(self, CONFIRMATION_BUDGET);
        }
    }
}
// WNOWAIT leaves the direct child owned until cleanup has submitted its signal.
fn exited_successfully(child: &Child) -> Result<Option<bool>, String> {
    let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
    if unsafe {
        libc::waitid(
            libc::P_PID,
            child.id(),
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    } != 0
    {
        return Err("无法查询插件进程。".into());
    }
    if unsafe { info.si_pid() } == 0 {
        return Ok(None);
    }
    Ok(Some(
        info.si_code == libc::CLD_EXITED && unsafe { info.si_status() } == 0,
    ))
}
// Fresh, single-child fixture for caller custody controls. It deliberately
// represents an already-attempted but unconfirmed cleanup; production never
// suppresses a signal. Its guard owns and reaps every synthetic child on unwind.
#[cfg(test)]
pub(crate) mod cleanup_fixture {
    use super::*;
    pub(crate) struct PendingChild {
        pub custody: Custody,
        pid: i32,
    }
    impl PendingChild {
        pub(crate) fn new() -> Self {
            Self::in_custody(Custody::default())
        }
        pub(crate) fn in_custody(custody: Custody) -> Self {
            custody.install(
                Command::new("/bin/sleep")
                    .arg("10")
                    .env_clear()
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .process_group(0)
                    .spawn()
                    .unwrap(),
            );
            custody.0.owned.lock().unwrap().as_mut().unwrap().started = true;
            custody.remember_failure("插件转写超时。");
            let pid = custody.0.owned.lock().unwrap().as_ref().unwrap().child.id() as i32;
            eprintln!("fresh pending cleanup fixture spawned: pid={pid}");
            Self { custody, pid }
        }
        pub(crate) fn observations(&self) -> usize {
            self.custody.0.observations.load(Ordering::SeqCst)
        }
        pub(crate) fn while_locked(&self, action: impl FnOnce()) {
            let _slot = self
                .custody
                .0
                .owned
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            action();
        }
        pub(crate) fn terminate(&self) {
            let mut slot = self
                .custody
                .0
                .owned
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(owned) = slot.as_mut() {
                assert!(!owned.reaped);
                owned.child.kill().unwrap();
                let mut info = unsafe { std::mem::zeroed::<libc::siginfo_t>() };
                assert_eq!(
                    unsafe {
                        libc::waitid(
                            libc::P_PID,
                            owned.child.id(),
                            &mut info,
                            libc::WEXITED | libc::WNOWAIT,
                        )
                    },
                    0
                );
            }
        }
    }
    impl Drop for PendingChild {
        fn drop(&mut self) {
            let mut slot = self
                .custody
                .0
                .owned
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(owned) = slot.as_mut()
                && !owned.reaped
            {
                let _ = owned.child.kill();
                owned
                    .child
                    .wait()
                    .expect("fresh fixture child reaped on unwind");
                owned.reaped = true;
            }
            slot.take();
            self.custody.0.unresolved.store(false, Ordering::Release);
            let result = unsafe { libc::kill(self.pid, 0) };
            let errno = std::io::Error::last_os_error().raw_os_error();
            eprintln!(
                "fresh pending cleanup fixture reaped: pid={} probe=({result},{errno:?})",
                self.pid
            );
            assert_eq!((result, errno), (-1, Some(libc::ESRCH)));
        }
    }
}
fn nonblocking(fd: RawFd) -> Result<(), String> {
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err("无法配置插件通信管道。".into());
        }
    }
    Ok(())
}
#[cfg(test)]
pub(crate) fn execute(
    manifest: &Manifest,
    config: &BTreeMap<String, String>,
    wav: &[u8],
    language: &str,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<String, Failure> {
    execute_owned(
        manifest,
        config,
        wav,
        language,
        cancel,
        timeout,
        &Custody::default(),
    )
}
pub(crate) fn execute_owned(
    manifest: &Manifest,
    config: &BTreeMap<String, String>,
    wav: &[u8],
    language: &str,
    cancel: &CancellationToken,
    timeout: Duration,
    custody: &Custody,
) -> Result<String, Failure> {
    execute_in(
        manifest,
        config,
        wav,
        language,
        cancel,
        timeout,
        custody,
        |_| {},
    )
}
// Observe the OS-assigned PID before adapter initialization. Tests can verify
// reaping even when the adapter never gets far enough to publish readiness.
#[cfg(test)]
fn execute_observed(
    manifest: &Manifest,
    config: &BTreeMap<String, String>,
    wav: &[u8],
    language: &str,
    cancel: &CancellationToken,
    timeout: Duration,
    spawned: impl FnOnce(u32),
) -> Result<String, Failure> {
    execute_in(
        manifest,
        config,
        wav,
        language,
        cancel,
        timeout,
        &Custody::default(),
        spawned,
    )
}
#[allow(clippy::too_many_arguments)] // Test-only spawn observation shares the real execution path.
fn execute_in(
    manifest: &Manifest,
    config: &BTreeMap<String, String>,
    wav: &[u8],
    language: &str,
    cancel: &CancellationToken,
    timeout: Duration,
    custody: &Custody,
    spawned: impl FnOnce(u32),
) -> Result<String, Failure> {
    check(cancel)?;
    manifest.validate_external()?;
    manifest.validate_program()?;
    if wav.len() > crate::plugin_runtime::MAX_WAV {
        return Err("插件音频超过大小限制。".into());
    }
    let request = serde_json::json!({"protocol_version":1,"event":"transcribe","plugin_id":manifest.id,"plugin_version":manifest.version,"config":config,"language":if language.is_empty(){None}else{Some(language)},"audio":{"format":"wav_pcm16_mono_16000","base64":base64::engine::general_purpose::STANDARD.encode(wav)}});
    let mut input = serde_json::to_vec(&request).map_err(|_| "无法编码插件请求。")?;
    input.push(b'\n');
    if input.len() > MAX_INPUT {
        return Err("插件请求超过大小限制。".into());
    }
    let program = manifest.program.as_ref().ok_or("插件程序未配置。")?;
    let started = Instant::now();
    let child = Command::new(&program.executable)
        .args(&program.args)
        .env_clear()
        .env("LANG", "C.UTF-8")
        .current_dir("/")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|_| "无法执行插件本地程序。")?;
    custody.install(child);
    let guard = CleanupGuard(custody.clone());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut slot = custody.0.owned.lock().unwrap_or_else(|e| e.into_inner());
        let child = &mut slot.as_mut().expect("installed owned child").child;
        spawned(child.id());
        let mut stdin = child.stdin.take();
        let mut stdout = child.stdout.take().ok_or("插件 stdout 不可用。")?;
        let mut stderr = child.stderr.take().ok_or("插件 stderr 不可用。")?;
        nonblocking(stdin.as_ref().ok_or("插件 stdin 不可用。")?.as_raw_fd())?;
        nonblocking(stdout.as_raw_fd())?;
        nonblocking(stderr.as_raw_fd())?;
        let mut written = 0;
        let mut output = vec![];
        let mut errors = 0usize;
        let mut out_eof = false;
        let mut err_eof = false;
        let mut buffer = [0; 8192];
        loop {
            check(cancel)?;
            if started.elapsed() >= timeout {
                return Err("插件转写超时。".into());
            }
            if let Some(pipe) = stdin.as_mut() {
                match pipe.write(&input[written..]) {
                    Ok(0) => return Err("插件提前关闭请求管道。".into()),
                    Ok(n) => written += n,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return Err("无法发送插件请求。".into()),
                }
                if written == input.len() {
                    stdin.take();
                }
            }
            // At most one chunk per pipe per iteration; noisy plugins cannot starve cancellation.
            if !out_eof {
                match stdout.read(&mut buffer) {
                    Ok(0) => out_eof = true,
                    Ok(n) => {
                        if output.len() + n > MAX_RESPONSE {
                            return Err("插件 stdout 或 JSONL 行超过大小限制。".into());
                        }
                        output.extend_from_slice(&buffer[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return Err("无法读取插件响应。".into()),
                }
            }
            if !err_eof {
                match stderr.read(&mut buffer) {
                    Ok(0) => err_eof = true,
                    Ok(n) => {
                        errors += n;
                        if errors > MAX_STDERR {
                            return Err("插件 stderr 超过大小限制。".into());
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return Err("无法读取插件错误管道。".into()),
                }
            }
            if let Some(success) = exited_successfully(child)?
                && out_eof
                && err_eof
            {
                if !success {
                    return Err("插件进程退出失败；未显示可能包含密钥的 stderr。".into());
                }
                if written != input.len() {
                    return Err("插件未接收完整请求。".into());
                }
                return parse(&output, config, &manifest.id);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }))
    .unwrap_or_else(|_| Err("插件任务异常结束。".into()));
    let original = result
        .as_ref()
        .err()
        .cloned()
        .unwrap_or_else(|| "插件已响应，但转写尚未完成。".into());
    custody.remember_failure(&original);
    let cleanup = custody.finish_initial();
    drop(guard); // No second attempt after either success or failure.
    if cleanup.is_err() {
        return Err(Failure {
            message: original,
            cleanup: Some(custody.clone()),
        });
    }
    result.map_err(Failure::from)
}
fn parse(
    bytes: &[u8],
    config: &BTreeMap<String, String>,
    plugin_id: &str,
) -> Result<String, String> {
    if !bytes.ends_with(b"\n") || bytes[..bytes.len().saturating_sub(1)].contains(&b'\n') {
        return Err("插件须返回恰好一行以换行结束的 final/error JSON。".into());
    }
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        protocol_version: u32,
        event: String,
        text: Option<String>,
        code: Option<String>,
    }
    let result: Response = serde_json::from_slice(bytes).map_err(|_| "插件响应 JSONL 无效。")?;
    if result.protocol_version != 1 {
        return Err("插件响应协议版本不受支持。".into());
    }
    match result.event.as_str() {
        "final" if result.code.is_none() => {
            let mut text = result.text.ok_or("插件 final 缺少字符串 text。")?;
            for (k, v) in config {
                if !v.is_empty() && k == "api_key" {
                    text = text.replace(v, "[redacted]");
                }
            }
            Ok(text.trim().to_owned())
        }
        "error"
            if result.text.is_none()
                && result
                    .code
                    .as_ref()
                    .is_some_and(|c| !c.is_empty() && c.len() <= 128) =>
        {
            // Error codes are untrusted and may contain credentials. Only map
            // exact known QQ codes to fixed text; never echo the raw value.
            let message = match (plugin_id, result.code.as_deref()) {
                ("qq.smartptt", Some("bridge_unavailable")) => {
                    "QQ 转写桥接未运行或连接地址已失效。普通 QQ 不会自动提供桥接；请先检查桥接的安装和运行状态。"
                }
                ("qq.smartptt", Some("bridge_disconnected" | "qq_bridge_stopped")) => {
                    "QQ 转写桥接连接已中断，本次转写未完成；请检查桥接状态后重新测试。"
                }
                ("qq.smartptt", Some("not_ready" | "qq_not_ready")) => {
                    "QQ 转写服务尚未就绪；请在 QQ 中完成正常登录，并等待桥接就绪后重试。"
                }
                ("qq.smartptt", Some("unauthorized")) => {
                    "QQ 桥接配对密钥不匹配或已失效；请重新配对此来源与当前桥接。"
                }
                ("qq.smartptt", Some("busy" | "qq_busy")) => {
                    "QQ 转写桥接仍在处理上一项任务；请等待任务结束后重试。"
                }
                ("qq.smartptt", Some("qq_cleanup_failed")) => {
                    "QQ 桥接未能完成上一项任务的清理；请检查桥接状态，并正常停止和重新启动桥接。"
                }
                ("qq.smartptt", Some("encoding_failed")) => {
                    "QQ 本地音频编码失败；请检查桥接适配器和编码依赖是否完整。"
                }
                ("qq.smartptt", Some("timeout" | "qq_smartptt_timeout")) => {
                    "QQ 转写等待超时；服务可能仍在处理，请确认桥接任务已结束后再重试。"
                }
                (
                    "qq.smartptt",
                    Some("qq_smartptt_error" | "qq_smartptt_call_failed" | "transcription_failed"),
                ) => "QQ 未能完成转写；请检查 QQ 的正常登录、网络及桥接状态。",
                _ => "插件报告转写失败；请检查服务或适配器配置。",
            };
            Err(message.into())
        }
        _ => Err("插件必须返回受支持的 final/error 事件。".into()),
    }
}
#[cfg(test)]
mod tests;
