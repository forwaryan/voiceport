use super::{
    CommandFailed, DownloadProgress, RELEASES_PAGE, Result, Update, bounded_output,
    bounded_output_with_progress,
};
use reqwest::{Response, StatusCode, header, redirect::Policy};
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

async fn get(
    url: &str,
    limit: u64,
    progress: Option<&DownloadProgress>,
) -> std::result::Result<Vec<u8>, PublicError> {
    let redirect = if progress.is_some() {
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
    let response = client
        .get(url)
        .header(
            header::ACCEPT,
            if progress.is_some() {
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
    read_body(response, limit, progress).await
}

async fn read_body(
    mut response: Response,
    limit: u64,
    progress: Option<&DownloadProgress>,
) -> std::result::Result<Vec<u8>, PublicError> {
    let length = response.content_length();
    if length.is_some_and(|size| size > limit) {
        return Err(PublicError::Response);
    }
    if let Some(progress) = progress {
        // Public URLs address the asset by name. If the response does not
        // give a usable size, an earlier release lookup may describe another asset.
        let headers = response.headers();
        if !headers.contains_key(header::CONTENT_RANGE)
            && !headers.contains_key(header::CONTENT_ENCODING)
        {
            progress.set_total(length);
        }
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
        if let Some(progress) = progress {
            progress.received(bytes.len() as u64);
        }
    }
    Ok(bytes)
}

fn public_get(
    url: &str,
    limit: u64,
    progress: Option<&DownloadProgress>,
) -> std::result::Result<Vec<u8>, PublicError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| PublicError::Connection)?
        .block_on(get(url, limit, progress))
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
        None,
    );
    latest_with(home, response, github_client)
}

pub(super) fn download(
    home: &Path,
    update: &Update,
    progress: &DownloadProgress,
) -> Result<Vec<u8>> {
    match &update.source {
        Source::Public => public_get(
            &format!(
                "{RELEASES_PAGE}/download/{}/{}",
                update.tag, update.asset_name
            ),
            update.bytes,
            Some(progress),
        )
        .map_err(|_| "无法从 GitHub 下载更新包，请检查网络或查看发布页。"),
        Source::Cli(gh) => {
            // gh emits the body without headers. This endpoint pins the same
            // asset ID whose validated size we received with the release.
            progress.set_total(Some(update.bytes));
            bounded_output_with_progress(
                &mut github(
                    gh,
                    home,
                    &format!("{REPO}/releases/assets/{}", update.id),
                    "Accept: application/octet-stream",
                ),
                update.bytes,
                REQUEST_TIMEOUT,
                Some(progress.clone()),
            )
            .map_err(|_| "GitHub CLI 下载失败，请检查 GitHub 登录状态、仓库权限和网络。")
        }
    }
}

#[cfg(test)]
mod tests;
