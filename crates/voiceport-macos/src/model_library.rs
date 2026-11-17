//! VoicePort-owned model installation. No package scripts or downloaded programs run.
use crate::local_models::{Model, ModelKind, RUNTIME_VERSION};
use base64::Engine;
use ring::digest::{Context, SHA256, SHA512};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

mod catalog;
#[cfg(test)]
mod tests;
pub use catalog::MODELS;
pub fn runtime_bytes() -> u64 {
    catalog::RUNTIME_ARCHIVE.bytes
        + catalog::RUNTIME_NOTICES
            .iter()
            .map(|a| a.bytes)
            .sum::<u64>()
}

#[derive(Debug)]
pub struct Asset {
    pub name: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}
#[derive(Debug)]
pub struct Definition {
    pub id: &'static str,
    pub directory: &'static str,
    pub name: &'static str,
    pub languages: &'static str,
    pub description: &'static str,
    pub recommendation: &'static str,
    pub kind: ModelKind,
    pub assets: &'static [Asset],
}
impl Definition {
    pub fn bytes(&self) -> u64 {
        self.assets.iter().map(|f| f.bytes).sum()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Installation {
    Missing,
    Current,
    Legacy,
    Incompatible,
}

#[derive(Clone, Debug)]
pub struct Paths {
    pub root: PathBuf,
}
impl Paths {
    pub fn system() -> Result<Self, String> {
        if let Some(value) = std::env::var_os("VOICEPORT_MODELS_PATH") {
            let root = PathBuf::from(value);
            if !root.is_absolute() {
                return Err("VOICEPORT_MODELS_PATH 必须是非空绝对目录路径。".into());
            }
            return Ok(Self { root });
        }
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .ok_or("无法定位用户模型目录。")?;
        Ok(Self {
            root: PathBuf::from(home).join("Library/Application Support/VoicePort/speech-models"),
        })
    }
    pub fn model(&self, d: &Definition) -> PathBuf {
        self.root.join(d.directory)
    }
    pub fn runtime(&self) -> PathBuf {
        self.root.join(format!("runtime-{RUNTIME_VERSION}"))
    }
    pub fn discover(&self, d: &Definition) -> Result<Model, String> {
        let directory = self.model(d);
        for asset in d.assets {
            let metadata = directory
                .join(asset.name)
                .symlink_metadata()
                .map_err(io_error)?;
            if !metadata.is_file() || metadata.len() != asset.bytes {
                return Err("模型副本格式或版本不兼容，请重新安装固定 ONNX 文件集。".into());
            }
        }
        Model::discover(directory, &self.runtime_for_kind(d.kind), d.kind)
    }
    /// A legacy ONNX copy can remain usable while the UI offers a transactional
    /// reinstall to add current provenance. Incompatible files are never selected.
    pub fn installation(&self, d: &Definition) -> Installation {
        let directory = self.model(d);
        if directory.symlink_metadata().is_err() {
            return Installation::Missing;
        }
        if self.discover(d).is_err() {
            return Installation::Incompatible;
        }
        let receipt = directory.join("VOICEPORT_MODEL.json");
        let current = file_matches(&receipt, &installation_receipt(d));
        if current
            && bundled_notices(d.kind)
                .iter()
                .all(|(name, text)| file_matches(&directory.join(name), text.as_bytes()))
        {
            Installation::Current
        } else {
            Installation::Legacy
        }
    }
    pub fn runtime_or_orca(&self) -> PathBuf {
        let own = self.runtime();
        if crate::local_models::validate_runtime(&own, std::env::consts::ARCH).is_ok() {
            own
        } else {
            let legacy = self.root.join(format!(
                "runtime-{}",
                crate::local_models::LEGACY_RUNTIME_VERSION
            ));
            if crate::local_models::validate_runtime(&legacy, std::env::consts::ARCH).is_ok() {
                legacy
            } else {
                PathBuf::from(crate::local_models::RUNTIME_DIRECTORY)
            }
        }
    }
    pub fn runtime_for_kind(&self, kind: ModelKind) -> PathBuf {
        if matches!(kind, ModelKind::ParakeetUnified | ModelKind::Nemotron) {
            self.runtime()
        } else {
            self.runtime_or_orca()
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Progress {
    pub message: String,
    pub completed: u64,
    pub total: u64,
}
pub struct Job {
    cancel: Arc<AtomicBool>,
    pub progress: Arc<Mutex<Progress>>,
    receiver: mpsc::Receiver<Result<String, String>>,
}
#[derive(Clone)]
pub enum Operation {
    Download,
    Engine,
    Import(PathBuf),
    Remove,
}
impl Job {
    pub fn start(
        paths: Paths,
        definition: &'static Definition,
        operation: Operation,
    ) -> Result<Self, String> {
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(Progress::default()));
        let (sender, receiver) = mpsc::sync_channel(1);
        let c = cancel.clone();
        let p = progress.clone();
        std::thread::Builder::new()
            .name("voiceport-model-install".into())
            .spawn(move || {
                let result = (|| {
                    if std::env::consts::ARCH != "aarch64" {
                        return Err("本地模型目前仅支持 Apple Silicon。".into());
                    }
                    let _lock = Lock::acquire(&paths.root)?;
                    match operation {
                        Operation::Remove => remove(&paths, definition, &c),
                        other => {
                            let runtime = tokio::runtime::Builder::new_current_thread()
                                .enable_all()
                                .build()
                                .map_err(|e| e.to_string())?;
                            runtime.block_on(async {
                                if matches!(other, Operation::Engine) {
                                    install_runtime(&paths, &client()?, &c, &p).await?;
                                    Ok("本地识别引擎已安装，可导入模型或使用兼容的 Orca 模型。"
                                        .into())
                                } else {
                                    install(&paths, definition, other, &c, &p).await
                                }
                            })
                        }
                    }
                })();
                // Temp files and installation lock are released before admission reopens.
                let _ = sender.send(result);
            })
            .map_err(|e| format!("无法开始模型操作：{e}"))?;
        Ok(Self {
            cancel,
            progress,
            receiver,
        })
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn poll(&self) -> Option<Result<String, String>> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Disconnected) => {
                Some(Err("模型任务意外退出；请检查安装状态后重试。".into()))
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct Lock(File);
impl Lock {
    fn acquire(root: &Path) -> Result<Self, String> {
        fs::create_dir_all(root).map_err(io_error)?;
        if !root.symlink_metadata().map_err(io_error)?.is_dir() {
            return Err("VoicePort 模型目录必须是普通文件夹。".into());
        }
        fs::set_permissions(root, fs::Permissions::from_mode(0o700)).map_err(io_error)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags_no_follow()
            .open(root.join(".install.lock"))
            .map_err(io_error)?;
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("另一个 VoicePort 实例正在管理模型，请稍后重试。".into());
        }
        Ok(Self(file))
    }
}
impl Drop for Lock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
trait NoFollow {
    fn custom_flags_no_follow(&mut self) -> &mut Self;
}
impl NoFollow for OpenOptions {
    fn custom_flags_no_follow(&mut self) -> &mut Self {
        use std::os::unix::fs::OpenOptionsExt;
        self.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
    }
}
fn io_error(error: std::io::Error) -> String {
    format!("模型文件操作失败：{error}")
}
fn check(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("已取消安装，未完成的文件已清理。".into())
    } else {
        Ok(())
    }
}
fn update(progress: &Mutex<Progress>, message: String, completed: u64, total: u64) {
    *progress.lock().unwrap_or_else(|p| p.into_inner()) = Progress {
        message,
        completed,
        total,
    };
}
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(5))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(30))
        .timeout(Duration::from_secs(1800))
        .build()
        .map_err(|e| e.to_string())
}
async fn network<T>(
    future: impl std::future::Future<Output = Result<T, reqwest::Error>>,
    cancel: &AtomicBool,
) -> Result<T, String> {
    tokio::pin!(future);
    loop {
        check(cancel)?;
        tokio::select! {
            result = &mut future => return result.map_err(|_| "模型下载连接失败或中断，请检查网络后重试。".into()),
            _ = tokio::time::sleep(Duration::from_millis(100)) => {},
        }
    }
}
async fn download(
    client: &reqwest::Client,
    asset: &Asset,
    destination: &Path,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
    offset: u64,
    total: u64,
) -> Result<(), String> {
    check(cancel)?;
    let mut response = network(client.get(asset.url).send(), cancel)
        .await?
        .error_for_status()
        .map_err(|e| {
            format!(
                "模型下载失败：{}",
                e.status()
                    .map_or_else(|| "网络错误".into(), |s| s.to_string())
            )
        })?;
    if response.content_length().is_some_and(|n| n != asset.bytes) {
        return Err("下载文件大小与模型目录不符，已停止安装。".into());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(io_error)?;
    let mut hash = Context::new(&SHA256);
    let mut bytes = 0u64;
    loop {
        check(cancel)?;
        let Some(chunk) = network(response.chunk(), cancel).await? else {
            break;
        };
        bytes = bytes
            .checked_add(chunk.len() as u64)
            .ok_or("下载文件过大。")?;
        if bytes > asset.bytes {
            return Err("下载文件超过预期大小。".into());
        }
        file.write_all(&chunk).map_err(io_error)?;
        hash.update(&chunk);
        update(
            progress,
            format!("正在下载 {}", asset.name),
            offset + bytes,
            total,
        );
    }
    verify(asset, bytes, hash.finish().as_ref())?;
    file.sync_all().map_err(io_error)?;
    check(cancel)
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn verify(asset: &Asset, bytes: u64, digest: &[u8]) -> Result<(), String> {
    if bytes != asset.bytes || hex(digest) != asset.sha256 {
        Err(format!(
            "{} 完整性校验失败，未安装；请重新下载官方文件。",
            asset.name
        ))
    } else {
        Ok(())
    }
}
fn import_file(
    asset: &Asset,
    source: &Path,
    target: &Path,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
    offset: u64,
    total: u64,
) -> Result<(), String> {
    use std::os::unix::fs::OpenOptionsExt;
    check(cancel)?;
    // A FIFO without a writer must not block before the descriptor type check.
    let mut input = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(source)
        .map_err(io_error)?;
    if !input.metadata().map_err(io_error)?.is_file() {
        return Err("请选择包含官方模型文件的文件夹。".into());
    }
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)
        .map_err(io_error)?;
    let mut hash = Context::new(&SHA256);
    let mut bytes = 0u64;
    let mut buffer = [0u8; 128 * 1024];
    loop {
        check(cancel)?;
        let count = input.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        if bytes > asset.bytes {
            return Err(format!("{} 与所选模型大小不符。", asset.name));
        }
        output.write_all(&buffer[..count]).map_err(io_error)?;
        hash.update(&buffer[..count]);
        update(
            progress,
            format!("正在导入 {}", asset.name),
            offset + bytes,
            total,
        );
    }
    verify(asset, bytes, hash.finish().as_ref())?;
    output.sync_all().map_err(io_error)?;
    check(cancel)
}
async fn install(
    paths: &Paths,
    d: &Definition,
    operation: Operation,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
) -> Result<String, String> {
    let target = paths.model(d);
    if let Ok(metadata) = target.symlink_metadata()
        && (!metadata.is_dir() || metadata.file_type().is_symlink())
    {
        return Err("模型目录类型异常，未执行替换。".into());
    }
    let stage = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(&paths.root)
        .map_err(io_error)?;
    let client = client()?;
    // Import is offline: never fetch executable dependencies without a download action.
    if crate::local_models::validate_runtime_for(
        &paths.runtime_for_kind(d.kind),
        std::env::consts::ARCH,
        d.kind,
    )
    .is_err()
    {
        match operation {
            Operation::Download => install_runtime(paths, &client, cancel, progress).await?,
            _ => return Err(
                "尚未安装识别引擎。请先点击下载安装一个模型，或安装兼容的 Orca 运行时后再导入。"
                    .into(),
            ),
        }
    }
    let total = d.bytes();
    let mut offset = 0;
    for asset in d.assets {
        match &operation {
            Operation::Download => {
                download(
                    &client,
                    asset,
                    &stage.path().join(asset.name),
                    cancel,
                    progress,
                    offset,
                    total,
                )
                .await?
            }
            Operation::Import(source) => import_file(
                asset,
                &source.join(asset.name),
                &stage.path().join(asset.name),
                cancel,
                progress,
                offset,
                total,
            )?,
            Operation::Remove | Operation::Engine => unreachable!(),
        }
        offset += asset.bytes;
    }
    // Both downloads and offline imports carry the same fixed license, notices,
    // conversion attribution and exact artifact identity.
    for (name, text) in bundled_notices(d.kind) {
        fs::write(stage.path().join(name), text).map_err(io_error)?;
    }
    fs::write(
        stage.path().join("VOICEPORT_MODEL.json"),
        installation_receipt(d),
    )
    .map_err(io_error)?;
    Model::discover(stage.path().into(), &paths.runtime_for_kind(d.kind), d.kind)?;
    check(cancel)?;
    publish_model(stage.path(), &target)?;
    Ok(format!("{} 已安装。点击“使用”后将在本机识别。", d.name))
}
fn file_matches(path: &Path, expected: &[u8]) -> bool {
    use std::os::unix::fs::OpenOptionsExt;
    (|| -> std::io::Result<bool> {
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != expected.len() as u64 {
            return Ok(false);
        }
        let mut bytes = vec![0; expected.len()];
        file.read_exact(&mut bytes)?;
        Ok(bytes == expected)
    })()
    .unwrap_or(false)
}
fn installation_receipt(d: &Definition) -> Vec<u8> {
    serde_json::to_vec_pretty(&serde_json::json!({
        "format": 1,
        "source_id": d.id,
        "runtime": RUNTIME_VERSION,
        "assets": d.assets.iter().map(|a| serde_json::json!({
            "name": a.name, "bytes": a.bytes, "sha256": a.sha256, "url": a.url,
        })).collect::<Vec<_>>(),
    }))
    .expect("static installation receipt")
}
pub(crate) fn bundled_notices(kind: ModelKind) -> Vec<(&'static str, &'static str)> {
    let (license, attribution) = match kind {
        ModelKind::SenseVoice => (catalog::SENSEVOICE_LICENSE, catalog::SENSEVOICE_ATTRIBUTION),
        ModelKind::Whisper | ModelKind::WhisperTurbo => {
            (catalog::WHISPER_LICENSE, catalog::WHISPER_ATTRIBUTION)
        }
        ModelKind::ParakeetTdtV3 => (catalog::CC_BY_4_LICENSE, catalog::PARAKEET_V3_ATTRIBUTION),
        ModelKind::ParakeetTdtV2 => (catalog::CC_BY_4_LICENSE, catalog::PARAKEET_V2_ATTRIBUTION),
        ModelKind::ParakeetUnified => (
            catalog::NVIDIA_OPEN_MODEL_LICENSE,
            catalog::PARAKEET_UNIFIED_ATTRIBUTION,
        ),
        ModelKind::Nemotron => (catalog::OPENMDW_LICENSE, catalog::NEMOTRON_ATTRIBUTION),
        ModelKind::Paraformer => return vec![],
    };
    let mut result = vec![
        ("MODEL_LICENSE.txt", license),
        ("ATTRIBUTION.txt", attribution),
    ];
    if kind == ModelKind::ParakeetUnified {
        result.push((
            "Notice.txt",
            "Licensed by NVIDIA Corporation under the NVIDIA Open Model License\n",
        ));
    }
    result
}
fn publish_model(stage: &Path, target: &Path) -> Result<(), String> {
    if let Ok(metadata) = target.symlink_metadata() {
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err("模型目录类型异常，保留现有副本。".into());
        }
        // A single filesystem transaction swaps fully verified staging with the
        // existing directory. On error the old model remains at target. On success
        // TempDir owns the old directory and removes it when install returns.
        use std::os::unix::ffi::OsStrExt;
        let from =
            std::ffi::CString::new(stage.as_os_str().as_bytes()).map_err(|_| "无效模型路径。")?;
        let to =
            std::ffi::CString::new(target.as_os_str().as_bytes()).map_err(|_| "无效模型路径。")?;
        if unsafe {
            libc::renameatx_np(
                libc::AT_FDCWD,
                from.as_ptr(),
                libc::AT_FDCWD,
                to.as_ptr(),
                libc::RENAME_SWAP,
            )
        } != 0
        {
            return Err(format!(
                "无法原子替换模型，保留现有副本：{}",
                std::io::Error::last_os_error()
            ));
        }
    } else {
        fs::rename(stage, target).map_err(io_error)?;
    }
    Ok(())
}
fn remove(paths: &Paths, d: &Definition, cancel: &AtomicBool) -> Result<String, String> {
    check(cancel)?;
    let target = paths.model(d);
    let metadata = target.symlink_metadata().map_err(io_error)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("模型目录类型异常，未执行删除。".into());
    }
    fs::remove_dir_all(target).map_err(io_error)?;
    Ok(format!("已删除 {} 的 VoicePort 副本。", d.name))
}

async fn install_runtime(
    paths: &Paths,
    client: &reqwest::Client,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
) -> Result<(), String> {
    use catalog::{RUNTIME_ARCHIVE, RUNTIME_FILES, RUNTIME_INTEGRITY};
    let target = paths.runtime();
    if crate::local_models::validate_runtime(&target, std::env::consts::ARCH).is_ok() {
        return Ok(());
    }
    if target
        .symlink_metadata()
        .is_ok_and(|m| !m.is_dir() || m.file_type().is_symlink())
    {
        return Err("识别引擎目录类型异常，未执行替换。".into());
    }
    let stage = tempfile::Builder::new()
        .prefix(".engine-")
        .tempdir_in(&paths.root)
        .map_err(io_error)?;
    let archive = stage.path().join("engine.tgz");
    download(
        client,
        &RUNTIME_ARCHIVE,
        &archive,
        cancel,
        progress,
        0,
        RUNTIME_ARCHIVE.bytes,
    )
    .await?;
    let mut input = File::open(&archive).map_err(io_error)?;
    let mut hash = Context::new(&SHA512);
    let mut buffer = [0u8; 128 * 1024];
    loop {
        let n = input.read(&mut buffer).map_err(io_error)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    if base64::engine::general_purpose::STANDARD.encode(hash.finish().as_ref()) != RUNTIME_INTEGRITY
    {
        return Err("识别引擎校验失败。".into());
    }
    let extracted = stage.path().join("runtime");
    fs::create_dir(&extracted).map_err(io_error)?;
    for name in RUNTIME_FILES {
        check(cancel)?;
        update(progress, "正在安装本地识别引擎…".into(), 0, 0);
        // Read only known regular members to stdout; tar never writes archive paths.
        let output = std::process::Command::new("/usr/bin/tar")
            .args(["-xOf"])
            .arg(&archive)
            .arg(format!("package/{name}"))
            .output()
            .map_err(io_error)?;
        if !output.status.success()
            || output.stdout.is_empty()
            || output.stdout.len() > 256 * 1024 * 1024
        {
            return Err(format!("无法解包识别引擎文件：{name}"));
        }
        fs::write(extracted.join(name), &output.stdout).map_err(io_error)?;
    }
    fs::write(
        extracted.join("LICENSE-sherpa-onnx.txt"),
        include_str!("local_runtime/LICENSE"),
    )
    .map_err(io_error)?;
    for notice in catalog::RUNTIME_NOTICES {
        download(
            client,
            notice,
            &extracted.join(notice.name),
            cancel,
            progress,
            0,
            notice.bytes,
        )
        .await?;
    }
    crate::local_models::validate_runtime(&extracted, std::env::consts::ARCH)?;
    check(cancel)?;
    publish_model(&extracted, &target)?;
    Ok(())
}
