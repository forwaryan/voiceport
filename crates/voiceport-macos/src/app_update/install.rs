//! Only a verified, newer, same-identity bundle can reach the atomic swap.
use super::{Result, Version, bounded_output};
use serde::Deserialize;
use std::ffi::{CStr, OsStr};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Read};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};
use tempfile::{Builder, TempDir};
use voiceport_control::{Action, transport};

mod acl;
pub(crate) use acl::has_no_acl_grants;

const APP: &str = "VoicePort.app";
const ID: &str = "io.github.forwaryan.voiceport";
const MAIN: &str = "Contents/MacOS/voiceport-app";
const CLI: &str = "Contents/MacOS/voiceport";
const STAGE: &str = ".voiceport-update-";
const MAX_EXPANSION: u64 = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 16_384;
const TOOL_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const QUIT_ACK: [u8; 1] = [1];
pub(crate) const MANUAL_ONLY: &str = "此副本不能在应用内安装。请从发布页下载，按手动安装说明更新。";
const MANUAL_RECOVERY: &str = "无法确认新版启动或控制连接的归属。保留两个应用包；请退出 VoicePort 后按手动安装说明恢复，不要删除归属不明的控制连接。";

pub(crate) struct Staged {
    directory: TempDir,
    original: BundleId,
    pub(crate) version: String,
}

impl Staged {
    #[cfg(any(test, debug_assertions))]
    pub(crate) fn fixture(version: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let original = BundleId::read(directory.path()).unwrap();
        Self {
            directory,
            original,
            version: version.into(),
        }
    }

    pub(crate) fn handoff(&mut self, installed: &Path) -> Result<ChildStdin> {
        let target = PersonalInstall::current(installed)?;
        target.staging_bundle(self.directory.path())?;
        if target.verify()? != self.original {
            return Err("已安装应用发生变化，请重新检查更新。");
        }
        let executable = installed.join(MAIN);
        let original = &self.original;
        let other = self.directory.path();
        let mut helper = Command::new(&executable)
            .arg("--apply-staged-update")
            .arg(std::process::id().to_string())
            .arg(installed)
            .arg(other)
            .arg(&self.version)
            .arg(original.dev.to_string())
            .arg(original.ino.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| "无法启动更新程序；已安装应用未改变。")?;
        let commit = helper.stdin.take().expect("piped helper stdin");
        self.directory.disable_cleanup(true);
        Ok(commit)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct BundleId {
    dev: u64,
    ino: u64,
}

impl BundleId {
    fn read(path: &Path) -> Result<Self> {
        let meta = fs::symlink_metadata(path).map_err(|_| "无法读取安装目录。")?;
        if !meta.is_dir() {
            return Err("安装目录不是普通目录。");
        }
        Ok(Self::from_metadata(&meta))
    }

    fn from_metadata(meta: &fs::Metadata) -> Self {
        Self {
            dev: meta.dev(),
            ino: meta.ino(),
        }
    }
}

fn real(path: &Path) -> bool {
    path.is_absolute()
        && fs::canonicalize(path).is_ok_and(|actual| actual.as_os_str() == path.as_os_str())
}

pub(crate) fn checkable_installation(path: &Path, home: &Path) -> bool {
    (path == Path::new("/Applications/VoicePort.app")
        || path == home.join("Applications").join(APP))
        && real(path)
}

fn account_home(uid: libc::uid_t) -> Option<PathBuf> {
    let mut buf = [0u8; 16 * 1024];
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result = std::ptr::null_mut();
    // Unlike HOME/CF preferences, the OS account record cannot be redirected by
    // a test launch or by settings inherited by the update helper.
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            &mut entry,
            buf.as_mut_ptr().cast(),
            buf.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() || entry.pw_dir.is_null() {
        return None;
    }
    let bytes = unsafe { CStr::from_ptr(entry.pw_dir) }.to_bytes();
    Some(PathBuf::from(OsStr::from_bytes(bytes)))
}

pub(super) struct PersonalInstall {
    app: PathBuf,
    home: PathBuf,
    home_id: BundleId,
    parent_id: BundleId,
    uid: libc::uid_t,
}

impl PersonalInstall {
    pub(super) fn current(app: &Path) -> Result<Self> {
        let uid = unsafe { libc::geteuid() };
        if uid == 0 || uid != unsafe { libc::getuid() } {
            return Err(MANUAL_ONLY);
        }
        let home = account_home(uid).ok_or(MANUAL_ONLY)?;
        let socket = transport::path_from_env().map_err(|_| MANUAL_ONLY)?;
        Self::new(app, &home, uid, &socket)
    }

    fn new(app: &Path, home: &Path, uid: libc::uid_t, socket: &Path) -> Result<Self> {
        let parent = home.join("Applications");
        if parent == Path::new("/Applications")
            || app != parent.join(APP)
            || socket != transport::default_path()
        {
            return Err(MANUAL_ONLY);
        }
        let target = Self {
            app: app.to_owned(),
            home: home.to_owned(),
            uid,
            home_id: owned_directory(home, uid, false)?,
            parent_id: owned_directory(&parent, uid, false)?,
        };
        target.verify()?;
        Ok(target)
    }

    fn parent(&self) -> &Path {
        self.app.parent().expect("validated installation path")
    }

    fn verify(&self) -> Result<BundleId> {
        if owned_directory(&self.home, self.uid, false)? != self.home_id
            || owned_directory(self.parent(), self.uid, false)? != self.parent_id
        {
            return Err(MANUAL_ONLY);
        }
        owned_directory(&self.app, self.uid, false)
    }

    fn staging_bundle(&self, directory: &Path) -> Result<PathBuf> {
        if directory.parent() != Some(self.parent())
            || !directory
                .file_name()
                .is_some_and(|name| name.as_bytes().starts_with(STAGE.as_bytes()))
        {
            return Err(MANUAL_ONLY);
        }
        owned_directory(directory, self.uid, true)?;
        let bundle = directory.join(APP);
        owned_directory(&bundle, self.uid, false)?;
        Ok(bundle)
    }
}

pub(crate) fn can_install_in_app(path: &Path) -> bool {
    PersonalInstall::current(path).is_ok()
}

fn owned_directory(path: &Path, uid: libc::uid_t, private: bool) -> Result<BundleId> {
    let directory = open_directory(path).map_err(|_| MANUAL_ONLY)?;
    let id = owned_directory_fd(&directory, uid, private)?;
    if !real(path) || BundleId::read(path).map_err(|_| MANUAL_ONLY)? != id {
        return Err(MANUAL_ONLY);
    }
    Ok(id)
}

fn owned_directory_fd(directory: &File, uid: libc::uid_t, private: bool) -> Result<BundleId> {
    let meta = directory.metadata().map_err(|_| MANUAL_ONLY)?;
    if !meta.is_dir()
        || meta.uid() != uid
        || meta.mode() & 0o022 != 0
        || private && meta.mode() & 0o777 != 0o700
        || !matches!(acl::has_no_acl_grants(directory.as_raw_fd()), Ok(true))
    {
        return Err(MANUAL_ONLY);
    }
    Ok(BundleId::from_metadata(&meta))
}

fn extract(bytes: &[u8], dir: &Path) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| "更新包不是有效的 ZIP。")?;
    if archive.is_empty() || archive.len() > MAX_ENTRIES {
        return Err("更新包文件数量无效。");
    }
    archive
        .validate_header_consistency()
        .map_err(|_| "更新包的 ZIP 结构无效或包含重名文件。")?;
    let mut total = 0;
    for i in 0..archive.len() {
        let mut item = archive.by_index(i).map_err(|_| "无法读取 ZIP 条目。")?;
        let name = item.name().map_err(|_| "更新包包含无效文件名。")?;
        let mut parts = name.strip_suffix('/').unwrap_or(&name).split('/');
        let root = parts.next().unwrap_or_default();
        if !matches!(root, APP | "__MACOSX")
            || parts.any(|p| p.is_empty() || p == "." || p == ".." || p.contains(['\\', ':', '\0']))
        {
            return Err("更新包包含无效路径。");
        }
        if root == "__MACOSX" {
            continue;
        }
        if item.encrypted() || name == APP && !item.is_dir() {
            return Err("更新包包含无法安装的条目。");
        }
        let mode = item.unix_mode().unwrap_or(0);
        let kind = mode & libc::S_IFMT as u32;
        if kind != 0
            && kind
                != if item.is_dir() {
                    libc::S_IFDIR as u32
                } else {
                    libc::S_IFREG as u32
                }
        {
            return Err("更新包包含链接或特殊文件。");
        }
        let path = dir.join(name.as_ref());
        if item.is_dir() {
            fs::create_dir_all(&path).map_err(|_| "无法建立暂存目录。")?;
            continue;
        }
        let size = item.size();
        total = u64::checked_add(total, size)
            .filter(|n| *n <= MAX_EXPANSION)
            .ok_or("更新包解压后超出大小限制。")?;
        fs::create_dir_all(path.parent().ok_or("更新包路径无效。")?)
            .map_err(|_| "无法建立暂存目录。")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| "无法写入暂存文件，或更新包包含重名文件。")?;
        let actual = io::copy(&mut item.by_ref().take(size + 1), &mut file)
            .map_err(|_| "无法解压更新包。")?;
        if actual != size {
            return Err("更新包包含大小不匹配的文件。");
        }
        let permissions = if mode & 0o111 != 0 { 0o755 } else { 0o644 };
        file.set_permissions(fs::Permissions::from_mode(permissions))
            .map_err(|_| "无法设置更新包文件权限。")?;
    }
    Ok(())
}

fn codesign(args: &[&str], path: &Path) -> Result<()> {
    bounded_output(
        Command::new("/usr/bin/codesign").args(args).arg(path),
        4096,
        TOOL_TIMEOUT,
    )
    .map(|_| ())
    .map_err(|_| "应用签名校验失败；未安装更新。")
}

// codesign's -d output is written to stderr. Capture only this local tool's
// inspection output by merging descriptors without shell interpolation.
fn signature(args: &[&str], path: &Path) -> Result<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(args)
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .map_err(|_| "无法读取已安装应用签名。")?;
    if !output.status.success() {
        return Err("无法读取已安装应用签名。");
    }
    String::from_utf8([output.stdout, output.stderr].concat())
        .map_err(|_| "无法识别已安装应用签名。")
}

fn same_identity(installed: &Path, staged: &Path) -> Result<()> {
    let identity = signature(&["-dv", "--verbose=4"], installed)?;
    if identity.lines().any(|line| line == "Signature=adhoc")
        || !identity.lines().any(|line| line.starts_with("Authority="))
    {
        return Err("当前应用没有可延续的证书签名，请先手动安装固定签名的版本。");
    }
    let text = signature(&["-d", "-r-"], installed)?;
    let requirement = text
        .lines()
        .find_map(|s| s.strip_prefix("designated => "))
        .filter(|s| !s.is_empty())
        .ok_or("当前应用没有可延续的签名要求。")?;
    if !requirement.contains("certificate") && !requirement.contains("anchor") {
        return Err("当前应用的签名要求未绑定签名身份。");
    }
    codesign(
        &["--verify", "--strict", &format!("-R={requirement}")],
        staged,
    )
}

#[derive(Deserialize)]
struct Info {
    #[serde(rename = "CFBundleIdentifier")]
    id: String,
    #[serde(rename = "CFBundleExecutable")]
    executable: String,
    #[serde(rename = "CFBundleShortVersionString")]
    version: String,
    #[serde(rename = "CFBundleVersion")]
    build: String,
    #[serde(rename = "LSMinimumSystemVersion")]
    minimum: String,
}

fn inspect(bundle: &Path) -> Result<Info> {
    codesign(&["--verify", "--deep", "--strict"], bundle)?;
    let bytes = bounded_output(
        Command::new("/usr/bin/plutil")
            .args(["-convert", "json", "-o", "-"])
            .arg(bundle.join("Contents/Info.plist")),
        64 * 1024,
        TOOL_TIMEOUT,
    )
    .map_err(|_| "无法读出应用版本信息。")?;
    let info: Info = serde_json::from_slice(&bytes).map_err(|_| "应用版本信息不完整。")?;
    if info.id != ID || info.executable != "voiceport-app" {
        return Err("更新包不是 VoicePort 应用。");
    }
    for binary in [MAIN, CLI] {
        let path = bundle.join(binary);
        let mut header = [0u8; 8];
        if !real(&path)
            || File::open(&path)
                .and_then(|mut f| f.read_exact(&mut header))
                .is_err()
            || header != [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 1]
        {
            return Err("更新包不是 Apple Silicon 版本。");
        }
    }
    Ok(info)
}

fn version_with_optional_patch(version: &str) -> Option<Version> {
    if version.matches('.').count() == 1 {
        Version::parse(&format!("{version}.0"))
    } else {
        Version::parse(version)
    }
}

fn preflight(installed: &Path, candidate: &Path, expected: &str) -> Result<()> {
    let old = inspect(installed)?;
    let new = inspect(candidate)?;
    let versions = (Version::parse(&old.version), Version::parse(&new.version));
    let builds = (old.build.parse::<u64>(), new.build.parse::<u64>());
    if old.version != env!("CARGO_PKG_VERSION")
        || new.version != expected
        || !matches!(versions, (Some(old), Some(new)) if new > old)
        || !matches!(builds, (Ok(old), Ok(new)) if new > old)
    {
        return Err("更新包的版本或构建号不匹配，或试图降级。");
    }
    let product_version = bounded_output(
        Command::new("/usr/bin/sw_vers").arg("-productVersion"),
        64,
        TOOL_TIMEOUT,
    )
    .map_err(|_| "无法识别当前 macOS 版本。")?;
    let current_os = std::str::from_utf8(&product_version)
        .ok()
        .and_then(|value| version_with_optional_patch(value.trim()));
    let minimum_os = version_with_optional_patch(&new.minimum);
    if !matches!((current_os, minimum_os), (Some(now), Some(minimum)) if now >= minimum) {
        return Err("更新包不支持当前 macOS 版本。");
    }
    same_identity(installed, candidate)?;
    same_identity(&installed.join(CLI), &candidate.join(CLI))?;
    Ok(())
}

pub(super) fn stage(target: &PersonalInstall, version: &str, bytes: &[u8]) -> Result<Staged> {
    let original = target.verify()?;
    let directory = Builder::new()
        .prefix(STAGE)
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in(target.parent())
        .map_err(|_| "应用程序目录不可写，请手动更新。")?;
    owned_directory(directory.path(), target.uid, true)?;
    extract(bytes, directory.path())?;
    let candidate = target.staging_bundle(directory.path())?;
    preflight(&target.app, &candidate, version)?;
    if target.verify()? != original {
        return Err("已安装应用发生变化，请重新检查更新。");
    }
    Ok(Staged {
        directory,
        original,
        version: version.into(),
    })
}

fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
}

fn swap(candidate_parent: &File, installed_parent: &File) -> io::Result<()> {
    // Use the checked directory descriptors, never a newly resolved path.
    let status = unsafe {
        libc::renameatx_np(
            candidate_parent.as_raw_fd(),
            c"VoicePort.app".as_ptr(),
            installed_parent.as_raw_fd(),
            c"VoicePort.app".as_ptr(),
            libc::RENAME_SWAP,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn verified_swap(
    target: &PersonalInstall,
    installed_parent: &File,
    stage_dir: &Path,
    installed_id: &BundleId,
    staged_id: &BundleId,
) -> Result<()> {
    let moved = "安装位置已变化；未替换应用。";
    let candidate_parent = open_directory(stage_dir).map_err(|_| MANUAL_ONLY)?;
    let verified_stage = BundleId::read(stage_dir).map_err(|_| MANUAL_ONLY)?;
    let bundle = target.staging_bundle(stage_dir)?;
    if &target.verify()? != installed_id
        || &BundleId::read(&bundle)? != staged_id
        || owned_directory_fd(installed_parent, target.uid, false)? != target.parent_id
        || owned_directory_fd(&candidate_parent, target.uid, true)? != verified_stage
    {
        return Err(moved);
    }
    swap(&candidate_parent, installed_parent).map_err(|_| "无法原子替换应用；保留原安装。")
}

const INSTALL_MARKER: &CStr = c".voiceport-install.lock";

struct InstallLock {
    directory: File,
    marker: File,
}

impl Drop for InstallLock {
    fn drop(&mut self) {
        let fd = self.directory.as_raw_fd();
        let mut current: libc::stat = unsafe { std::mem::zeroed() };
        let status = unsafe {
            libc::fstatat(
                fd,
                INSTALL_MARKER.as_ptr(),
                &mut current,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status == 0
            && self.marker.metadata().is_ok_and(|meta| {
                meta.dev() == current.st_dev as u64 && meta.ino() == current.st_ino
            })
        {
            unsafe { libc::unlinkat(fd, INSTALL_MARKER.as_ptr(), 0) };
        }
    }
}

fn lock_installation(target: &PersonalInstall) -> Result<InstallLock> {
    target.verify()?;
    let parent = target.parent();
    let unavailable = "另一个安装任务正在运行或安装目录不可用。";
    let lock = open_directory(parent).map_err(|_| MANUAL_ONLY)?;
    lock.try_lock().map_err(|_| unavailable)?;
    if owned_directory_fd(&lock, target.uid, false)? != target.parent_id {
        return Err(MANUAL_ONLY);
    }
    // The old installer exclusively creates this marker without taking the
    // directory flock. Hold both until replacement, launch and rollback settle.
    let flag = libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC;
    let fd = unsafe {
        libc::openat(
            lock.as_raw_fd(),
            INSTALL_MARKER.as_ptr(),
            flag,
            0o600 as libc::c_int,
        )
    };
    if fd < 0 {
        return Err("另一个安装任务或旧锁阻止更新；请核对归属后按手动安装说明处理。");
    }
    let mut lock = InstallLock {
        directory: lock,
        marker: unsafe { File::from_raw_fd(fd) },
    };
    use std::io::Write;
    writeln!(lock.marker, "{}", std::process::id()).map_err(|_| unavailable)?;
    Ok(lock)
}

fn install_new(
    target: &PersonalInstall,
    installed_parent: &File,
    stage_dir: &Path,
    socket: &Path,
    version: &str,
    original: &BundleId,
) -> Result<BundleId> {
    if &target.verify()? != original {
        return Err("安装位置已变化，未替换应用。");
    }
    let _startup = transport::lock_quiet(socket)
        .map_err(|_| "VoicePort 已经在运行或控制连接不可用；未替换应用。")?;
    let candidate = target.staging_bundle(stage_dir)?;
    let replacement = BundleId::read(&candidate)?;
    preflight(&target.app, &candidate, version)?;
    verified_swap(target, installed_parent, stage_dir, original, &replacement)?;
    Ok(replacement)
}

fn restore_previous(
    target: &PersonalInstall,
    installed_parent: &File,
    stage_dir: &Path,
    socket: &Path,
    original: &BundleId,
    replacement: &BundleId,
) -> Result<()> {
    if &target.verify()? != replacement {
        return Err("安装位置已变化；保留两个应用包供手动恢复。");
    }
    let _startup = transport::lock_for_owned_recovery(socket).map_err(|_| MANUAL_RECOVERY)?;
    let backup = target.staging_bundle(stage_dir)?;
    if &BundleId::read(&backup)? != original {
        return Err("安装位置已变化；保留两个应用包供手动恢复。");
    }
    codesign(&["--verify", "--deep", "--strict"], &backup)?;
    verified_swap(target, installed_parent, stage_dir, replacement, original)
}

fn apply_after_quit(
    target: &PersonalInstall,
    stage_dir: &Path,
    pid: i32,
    version: &str,
    original: BundleId,
) -> Result<()> {
    if target.verify()? != original {
        return Err("安装位置已变化，未替换应用。");
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if unsafe { libc::kill(pid, 0) } != 0
            && io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            break;
        }
        if Instant::now() >= deadline {
            return Err("旧版尚未退出，未替换应用。");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // Keep manual and automatic installers out until launch or rollback settles.
    let installation = lock_installation(target)?;
    let socket = transport::default_path();
    let replacement = match install_new(
        target,
        &installation.directory,
        stage_dir,
        &socket,
        version,
        &original,
    ) {
        Ok(replacement) => replacement,
        Err(error) => {
            if transport::lock_quiet(&socket).is_err() {
                return Err(MANUAL_RECOVERY);
            }
            let _ = reopen_verified(target, &original, open);
            return Err(error);
        }
    };
    let launch = reopen_verified(target, &replacement, open);
    let confirmed = running_update(&target.app, &socket, pid, version)
        && target.verify().is_ok_and(|id| id == replacement);
    finish_launch(launch, confirmed, || {
        restore_previous(
            target,
            &installation.directory,
            stage_dir,
            &socket,
            &original,
            &replacement,
        )?;
        if reopen_verified(target, &original, open).is_ok() {
            Err("无法确认新版启动，已恢复旧版并请求重新打开。")
        } else {
            Err("无法启动新版。旧版已恢复到应用程序目录，请手动打开 VoicePort。")
        }
    })
}

fn finish_launch(
    launch: Result<()>,
    confirmed: bool,
    recover: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if confirmed {
        return Ok(());
    }
    // `open` can fail after dispatching. Do not retry or roll back blindly.
    launch.map_err(|_| MANUAL_RECOVERY)?;
    recover()
}

fn running_update(installed: &Path, socket: &Path, pid: i32, version: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if transport::call(socket, Action::Status)
            .ok()
            .filter(|r| r.status == "ok")
            .map(|r| r.data)
            .is_some_and(|v| {
                v.get("version").and_then(|x| x.as_str()) == Some(version)
                    && v.get("bundle_path").and_then(|x| x.as_str()) == installed.to_str()
                    && v.get("pid")
                        .and_then(|x| x.as_i64())
                        .is_some_and(|new| new != i64::from(pid))
            })
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn reopen_verified(
    target: &PersonalInstall,
    expected: &BundleId,
    launch: impl FnOnce(&Path) -> Result<()>,
) -> Result<()> {
    if &target.verify()? != expected {
        return Err(MANUAL_ONLY);
    }
    launch(&target.app)
}

fn open(installed: &Path) -> Result<()> {
    bounded_output(
        Command::new("/usr/bin/open").arg("-a").arg(installed),
        4096,
        TOOL_TIMEOUT,
    )
    .map(|_| ())
    .map_err(|_| "无法重新打开 VoicePort；旧包已保留。")
}

fn confirmed_shutdown(mut input: impl Read + AsRawFd, timeout_ms: i32) -> bool {
    let mut pipe = libc::pollfd {
        fd: input.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    let mut ack = [0u8; 1];
    // EOF, an invalid byte or a timeout means the running app did not perform a
    // normal AppKit shutdown. In particular a force quit never applies the update.
    (unsafe { libc::poll(&mut pipe, 1, timeout_ms) } == 1)
        && pipe.revents & libc::POLLIN != 0
        && input.read_exact(&mut ack).is_ok()
        && ack == QUIT_ACK
}

pub(crate) fn apply(args: &[String]) -> Result<()> {
    let [pid, app, dir, version, dev, ino] = args else {
        return Err("更新交接参数无效。");
    };
    let installed = Path::new(app);
    let stage_dir = Path::new(dir);
    let target = PersonalInstall::current(installed)?;
    target.staging_bundle(stage_dir)?;
    if std::env::current_exe().ok().as_deref() != Some(installed.join(MAIN).as_path()) {
        return Err("更新交接目录或安装位置无效。");
    }
    let pid = pid
        .parse::<i32>()
        .ok()
        .filter(|p| *p > 1)
        .ok_or("旧版进程无效。")?;
    let original = BundleId {
        dev: dev.parse().map_err(|_| "应用身份无效。")?,
        ino: ino.parse().map_err(|_| "应用身份无效。")?,
    };
    let acknowledged = confirmed_shutdown(io::stdin().lock(), 15_000);
    let result = if acknowledged {
        apply_after_quit(&target, stage_dir, pid, version, original)
    } else {
        Err("没有收到正常退出确认；未替换应用。")
    };
    let message = result
        .as_ref()
        .map(|_| "新版已启动，旧应用包保留在此目录。\n")
        .unwrap_or_else(|e| e);
    // Each install uses a new private directory; never overwrite a previous receipt.
    use std::io::Write;
    if let Ok(mut file) = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(stage_dir.join("update-result.txt"))
    {
        let _ = writeln!(file, "{message}");
    }
    if acknowledged && let Err(error) = result {
        // This runs in the separate installer, after releasing both locks.
        // Pass text as arguments so filesystem paths cannot change the script.
        let mut dialog = Command::new("/usr/bin/osascript");
        dialog
            .args([
                "-e",
                "on run argv\n	display alert \"VoicePort 更新未完成\" message (item 1 of argv & return & return & item 2 of argv) as warning giving up after 30\nend run",
                error,
            ])
            .arg(stage_dir);
        let _ = bounded_output(&mut dialog, 4096, Duration::from_secs(35));
    }
    result
}

#[cfg(test)]
mod tests;
