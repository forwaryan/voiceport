//! GitHub release selection and verified update staging.
mod download_progress;
mod github;
mod install;

use ring::digest;
use serde::Deserialize;
use std::io::{self, Read};
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub(super) use download_progress::{DownloadProgress, DownloadSnapshot};
pub(crate) use install::has_no_extended_acl;
pub(super) use install::{
    MANUAL_ONLY, QUIT_ACK, Staged, apply, can_install_in_app, checkable_installation,
};

pub(super) const RELEASES_PAGE: &str = "https://github.com/forwaryan/voiceport/releases";
const MAX_ZIP: u64 = 64 * 1024 * 1024;
pub(super) type Result<T> = std::result::Result<T, &'static str>;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct Version([u32; 3]);

impl Version {
    fn parse(input: &str) -> Option<Self> {
        let mut parts = input.split('.');
        let mut values = [0; 3];
        for value in &mut values {
            let part = parts.next()?;
            if part.is_empty()
                || (part.len() > 1 && part.starts_with('0'))
                || !part.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            *value = part.parse().ok()?;
        }
        parts.next().is_none().then_some(Self(values))
    }
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    id: u64,
    name: String,
    digest: Option<String>,
    size: u64,
    state: String,
}

#[derive(Debug)]
pub(super) struct Update {
    pub(super) version: String,
    tag: String,
    asset_name: String,
    id: u64,
    digest: [u8; 32],
    bytes: u64,
    source: github::Source,
}

#[derive(Debug)]
pub(super) enum Check {
    Current {
        latest: String,
        local_newer: bool,
    },
    Available(Update),
    Incompatible {
        latest: String,
        reason: &'static str,
    },
}

impl Release {
    fn mac_asset(&self) -> Result<(&Asset, [u8; 32])> {
        if !cfg!(target_arch = "aarch64") {
            return Err("此发行包仅支持搭载 Apple 芯片的 Mac。");
        }
        let versioned = format!("VoicePort-{}-macos-arm64.zip", self.tag_name);
        let mut candidates = self
            .assets
            .iter()
            .filter(|asset| asset.name == versioned || asset.name == "VoicePort-macos-arm64.zip");
        let asset = candidates.next().ok_or("此版本缺少 macOS arm64 更新包。")?;
        if candidates.next().is_some()
            || asset.id == 0
            || !(1..=MAX_ZIP).contains(&asset.size)
            || asset.state != "uploaded"
        {
            return Err("macOS 更新包的发布信息无效或存在重名候选。");
        }
        let hash = asset
            .digest
            .as_deref()
            .and_then(|s| s.strip_prefix("sha256:"))
            .filter(|s| s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .ok_or("发布缺少有效的 SHA-256，无法校验更新包。")?;
        let mut digest = [0; 32];
        for (i, byte) in digest.iter_mut().enumerate() {
            *byte =
                u8::from_str_radix(&hash[i * 2..i * 2 + 2], 16).map_err(|_| "更新包摘要无效。")?;
        }
        Ok((asset, digest))
    }
}

fn release_update(bytes: &[u8], current: Version, source: github::Source) -> Result<Check> {
    let release: Release =
        serde_json::from_slice(bytes).map_err(|_| "无法读取 GitHub 发布信息。")?;
    if release.draft || release.prerelease {
        return Err("GitHub 未返回正式可用的发行版本。");
    }
    let latest = release
        .tag_name
        .strip_prefix('v')
        .ok_or("GitHub 发布版本号无效。")?;
    let next = Version::parse(latest).ok_or("GitHub 发布版本号无效。")?;
    let (asset, digest) = match release.mac_asset() {
        Ok(asset) => asset,
        Err(reason) => {
            return Ok(Check::Incompatible {
                latest: latest.into(),
                reason,
            });
        }
    };
    if next <= current {
        return Ok(Check::Current {
            latest: latest.into(),
            local_newer: next < current,
        });
    }
    Ok(Check::Available(Update {
        version: latest.into(),
        tag: release.tag_name.clone(),
        asset_name: asset.name.clone(),
        id: asset.id,
        digest,
        bytes: asset.size,
        source,
    }))
}

pub(super) fn check(home: &Path, running: &str) -> Result<Check> {
    check_with(running, || github::latest(home))
}

fn check_with(
    running: &str,
    fetch_latest: impl FnOnce() -> Result<(Vec<u8>, github::Source)>,
) -> Result<Check> {
    let current = Version::parse(running).ok_or("无法识别当前应用版本号。")?;
    let (bytes, source) = fetch_latest()?;
    release_update(&bytes, current, source)
}

#[cfg(debug_assertions)]
pub(crate) fn check_fixture_release(running: &str, release: Result<Vec<u8>>) -> Result<Check> {
    check_with(running, || Ok((release?, github::Source::Public)))
}

#[derive(Debug)]
struct CommandFailed(ExitStatus);

impl std::fmt::Display for CommandFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "command failed ({})", self.0)
    }
}

impl std::error::Error for CommandFailed {}

fn bounded_output(command: &mut Command, limit: u64, timeout: Duration) -> io::Result<Vec<u8>> {
    bounded_output_with_progress(command, limit, timeout, None)
}

fn bounded_output_with_progress(
    command: &mut Command,
    limit: u64,
    timeout: Duration,
    progress: Option<DownloadProgress>,
) -> io::Result<Vec<u8>> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut output = child.stdout.take().expect("piped stdout").take(limit + 1);
    let (tx, rx) = mpsc::sync_channel(1);
    let read_thread = thread::Builder::new().spawn(move || {
        let result = if let Some(progress) = progress {
            let mut bytes = Vec::new();
            let mut chunk = [0; 8192];
            loop {
                match output.read(&mut chunk) {
                    Ok(0) => break Ok(bytes),
                    Ok(n) => {
                        bytes.extend_from_slice(&chunk[..n]);
                        if bytes.len() as u64 <= limit {
                            progress.received(bytes.len() as u64);
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => break Err(error),
                }
            }
        } else {
            let mut bytes = Vec::new();
            output.read_to_end(&mut bytes).map(|_| bytes)
        };
        let _ = tx.send(result);
    });
    let deadline = Instant::now() + timeout;
    let result = (|| {
        read_thread?;
        let bytes = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "command output deadline"))??;
        if bytes.len() as u64 > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "command output limit",
            ));
        }
        loop {
            if let Some(status) = child.try_wait()? {
                return if status.success() {
                    Ok(bytes)
                } else {
                    Err(io::Error::other(CommandFailed(status)))
                };
            }
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "command exit deadline",
                ));
            }
            thread::sleep(Duration::from_millis(20));
        }
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn verify_download(bytes: &[u8], update: &Update) -> Result<()> {
    if bytes.len() as u64 != update.bytes
        || digest::digest(&digest::SHA256, bytes).as_ref() != update.digest
    {
        return Err("更新包的大小或 SHA-256 不匹配；未安装。");
    }
    Ok(())
}

pub(super) fn stage(
    home: &Path,
    destination: &Path,
    update: &Update,
    progress: &DownloadProgress,
) -> Result<Staged> {
    let target = install::PersonalInstall::current(destination)?;
    let bytes = github::download(home, update, progress)?;
    progress.verifying();
    verify_download(&bytes, update)?;
    install::stage(&target, &update.version, &bytes)
}

#[cfg(test)]
mod tests;
#[cfg(all(test, target_arch = "aarch64"))]
pub(crate) use tests::fixture_release_check;
