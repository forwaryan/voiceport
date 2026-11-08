//! One owned process group, nonblocking pipes, no reader threads, bounded JSONL v1.
use crate::{
    plugin_runtime::{MAX_RESPONSE, check},
    plugins::Manifest,
};
use base64::Engine;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, RawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
use voiceport_core::CancellationToken;
const MAX_INPUT: usize = 8_000_000;
const MAX_STDERR: usize = 65_536;
struct Owned(Child);
impl Drop for Owned {
    fn drop(&mut self) {
        // CommandExt::process_group(0) created this group for this child. Even if
        // the leader exited, close descendants' pipes before releasing the lease.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
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
pub fn execute(
    manifest: &Manifest,
    config: &BTreeMap<String, String>,
    wav: &[u8],
    language: &str,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<String, String> {
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
    let mut child = Owned(
        Command::new(&program.executable)
            .args(&program.args)
            .env_clear()
            .env("LANG", "C.UTF-8")
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .map_err(|_| "无法执行插件本地程序。")?,
    );
    let mut stdin = child.0.stdin.take();
    let mut stdout = child.0.stdout.take().ok_or("插件 stdout 不可用。")?;
    let mut stderr = child.0.stderr.take().ok_or("插件 stderr 不可用。")?;
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
            return Err("插件转写超时，进程已停止。".into());
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
                        return Err("插件 stderr 超过大小限制，进程已停止。".into());
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => return Err("无法读取插件错误管道。".into()),
            }
        }
        if let Some(status) = child.0.try_wait().map_err(|_| "无法查询插件进程。")?
            && out_eof
            && err_eof
        {
            if !status.success() {
                return Err("插件进程退出失败；未显示可能包含密钥的 stderr。".into());
            }
            if written != input.len() {
                return Err("插件未接收完整请求。".into());
            }
            return parse(&output, config);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn parse(bytes: &[u8], config: &BTreeMap<String, String>) -> Result<String, String> {
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
            Err("插件报告转写失败；请检查服务或适配器配置。".into())
        }
        _ => Err("插件必须返回受支持的 final/error 事件。".into()),
    }
}
#[cfg(test)]
mod tests;
