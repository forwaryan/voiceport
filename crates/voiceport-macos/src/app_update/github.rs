use super::{CommandFailed, RELEASES_PAGE, Result, Update, bounded_output};
use reqwest::{StatusCode, redirect::Policy};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

const REPO: &str = "repos/forwaryan/voiceport";
const LATEST: &str = "repos/forwaryan/voiceport/releases/latest";
const MAX_METADATA: u64 = 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub(super) enum Source {
    Public,
    Cli(PathBuf),
}

#[derive(Debug, Clone, Copy)]
enum PublicError {
    Restricted,
    RateLimited,
    Connection,
    Response,
}

async fn get(url: &str, limit: u64, download: bool) -> std::result::Result<Vec<u8>, PublicError> {
    let redirect = if download {
        Policy::custom(|attempt| {
            let url = attempt.url();
            if attempt.previous().len() <= 3
                && url.scheme() == "https"
                && url.port_or_known_default() == Some(443)
                && url.username().is_empty()
                && url.password().is_none()
                && matches!(
                    url.host_str(),
                    Some("release-assets.githubusercontent.com" | "objects.githubusercontent.com")
                )
            {
                attempt.follow()
            } else {
                attempt.error("unexpected GitHub asset redirect")
            }
        })
    } else {
        Policy::none()
    };
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(redirect)
        .referer(false)
        .user_agent("VoicePort-Updater")
        .connect_timeout(Duration::from_secs(10))
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| PublicError::Connection)?;
    let mut response = client
        .get(url)
        .header(
            reqwest::header::ACCEPT,
            if download {
                "application/octet-stream"
            } else {
                "application/vnd.github+json"
            },
        )
        .send()
        .await
        .map_err(|error| {
            if error.is_redirect() {
                PublicError::Response
            } else {
                PublicError::Connection
            }
        })?;
    match response.status() {
        StatusCode::OK => (),
        StatusCode::TOO_MANY_REQUESTS => return Err(PublicError::RateLimited),
        StatusCode::FORBIDDEN
            if response
                .headers()
                .get("x-ratelimit-remaining")
                .is_some_and(|v| v == "0") =>
        {
            return Err(PublicError::RateLimited);
        }
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN | StatusCode::NOT_FOUND => {
            return Err(PublicError::Restricted);
        }
        _ => return Err(PublicError::Response),
    }
    if response.content_length().is_some_and(|size| size > limit) {
        return Err(PublicError::Response);
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| PublicError::Connection)?
    {
        if bytes.len() as u64 + chunk.len() as u64 > limit {
            return Err(PublicError::Response);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn public_get(url: &str, limit: u64, download: bool) -> std::result::Result<Vec<u8>, PublicError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| PublicError::Connection)?
        .block_on(get(url, limit, download))
}

fn executable(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    candidates.into_iter().find(|path| {
        fs::metadata(path)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    })
}

fn github_client(home: &Path) -> Option<PathBuf> {
    executable([
        PathBuf::from("/opt/homebrew/bin/gh"),
        PathBuf::from("/usr/local/bin/gh"),
        home.join(".local/bin/gh"),
    ])
}

fn github(gh: &Path, home: &Path, endpoint: &str, media_type: &str) -> Command {
    let mut command = Command::new(gh);
    // The CLI manages its saved GitHub login. Do not inherit shell tokens,
    // inspect credentials, run a shell, or let the release specify a host.
    command
        .env_clear()
        .env("HOME", home)
        .env("PATH", "/usr/bin:/bin")
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_NO_UPDATE_NOTIFIER", "1")
        .env("GH_NO_EXTENSION_UPDATE_NOTIFIER", "1")
        .current_dir("/")
        .args([
            "api",
            "--hostname",
            "github.com",
            "--method",
            "GET",
            endpoint,
            "--header",
            media_type,
        ]);
    command
}

fn latest_with(
    home: &Path,
    public: std::result::Result<Vec<u8>, PublicError>,
    find_cli: impl FnOnce(&Path) -> Option<PathBuf>,
) -> Result<(Vec<u8>, Source)> {
    let unavailable = match public {
        Ok(bytes) => return Ok((bytes, Source::Public)),
        Err(PublicError::Restricted) => {
            "GitHub 未公开可用发行，且未找到 GitHub CLI（gh）；私有发布需要登录并拥有仓库权限。"
        }
        Err(PublicError::RateLimited) => {
            "GitHub 限制了公开查询，且未找到 GitHub CLI（gh）；请稍后重试。"
        }
        Err(PublicError::Connection) => {
            return Err("无法连接 GitHub 或查询超时，请检查网络后重试。");
        }
        Err(PublicError::Response) => return Err("GitHub 返回了异常的发布响应，请稍后重试。"),
    };
    let gh = find_cli(home).ok_or(unavailable)?;
    let bytes = bounded_output(
        &mut github(&gh, home, LATEST, "Accept: application/vnd.github+json"),
        MAX_METADATA,
        REQUEST_TIMEOUT,
    )
    .map_err(|error| match error.kind() {
        io::ErrorKind::TimedOut => "GitHub CLI 查询超时，请检查网络后重试。",
        io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied => {
            "无法启动 GitHub CLI（gh），请检查其安装。"
        }
        io::ErrorKind::InvalidData => "GitHub CLI 返回了异常的发布响应。",
        // gh documents exit 4 as authentication required. Never surface stderr.
        _ if error
            .get_ref()
            .and_then(|e| e.downcast_ref::<CommandFailed>())
            .is_some_and(|failed| failed.0.code() == Some(4)) =>
        {
            "GitHub CLI 尚未登录 GitHub，请使用有仓库访问权限的账号登录。"
        }
        _ => "GitHub CLI 查询失败，请检查 GitHub 登录状态、仓库权限和网络。",
    })?;
    Ok((bytes, Source::Cli(gh)))
}

pub(super) fn latest(home: &Path) -> Result<(Vec<u8>, Source)> {
    let response = public_get(
        &format!("https://api.github.com/{LATEST}"),
        MAX_METADATA,
        false,
    );
    latest_with(home, response, github_client)
}

pub(super) fn download(home: &Path, update: &Update) -> Result<Vec<u8>> {
    match &update.source {
        Source::Public => public_get(
            &format!(
                "{RELEASES_PAGE}/download/{}/{}",
                update.tag, update.asset_name
            ),
            update.bytes,
            true,
        )
        .map_err(|_| "无法从 GitHub 下载更新包，请检查网络或查看发布页。"),
        Source::Cli(gh) => bounded_output(
            &mut github(
                gh,
                home,
                &format!("{REPO}/releases/assets/{}", update.id),
                "Accept: application/octet-stream",
            ),
            update.bytes,
            REQUEST_TIMEOUT,
        )
        .map_err(|_| "GitHub CLI 下载失败，请检查 GitHub 登录状态、仓库权限和网络。"),
    }
}

#[cfg(test)]
mod tests;
