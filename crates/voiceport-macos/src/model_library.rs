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
        Model::discover(self.model(d), &self.runtime_or_orca(), d.kind)
    }
    pub fn runtime_or_orca(&self) -> PathBuf {
        let own = self.runtime();
        if crate::local_models::validate_runtime(&own, std::env::consts::ARCH).is_ok() {
            own
        } else {
            PathBuf::from(crate::local_models::RUNTIME_DIRECTORY)
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
    if target.symlink_metadata().is_ok() {
        return Err("此模型目录已存在；如需重新安装，请先删除 VoicePort 中的副本。".into());
    }
    let stage = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(&paths.root)
        .map_err(io_error)?;
    let client = client()?;
    // Import is offline: never fetch executable dependencies without a download action.
    if crate::local_models::validate_runtime(&paths.runtime_or_orca(), std::env::consts::ARCH)
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
    // Fixed bundled notices accompany both downloads and offline imports.
    let notices = match d.kind {
        ModelKind::SenseVoice => {
            Some((catalog::SENSEVOICE_LICENSE, catalog::SENSEVOICE_ATTRIBUTION))
        }
        ModelKind::Whisper | ModelKind::WhisperTurbo => {
            Some((catalog::WHISPER_LICENSE, catalog::WHISPER_ATTRIBUTION))
        }
        ModelKind::Paraformer => None,
    };
    if let Some((license, attribution)) = notices {
        fs::write(stage.path().join("MODEL_LICENSE.txt"), license).map_err(io_error)?;
        fs::write(stage.path().join("ATTRIBUTION.txt"), attribution).map_err(io_error)?;
    }
    Model::discover(stage.path().into(), &paths.runtime_or_orca(), d.kind)?;
    check(cancel)?;
    fs::rename(stage.path(), &target).map_err(io_error)?;
    Ok(format!("{} 已安装。点击“使用”后将在本机识别。", d.name))
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
    if target.symlink_metadata().is_ok() {
        return Err("识别引擎目录损坏，请检查 VoicePort 模型目录。".into());
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
    fs::rename(extracted, target).map_err(io_error)?;
    Ok(())
}
