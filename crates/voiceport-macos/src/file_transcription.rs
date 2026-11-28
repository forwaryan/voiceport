//! Single-file work owns its audio and cancellation independently of dictation.
use crate::{
    local_models::{Catalog, SessionSource},
    plugin_process::Custody,
    plugins::{Adapter, validate_endpoint},
};
use serde::Serialize;
use std::{
    collections::VecDeque,
    fs::{File, Metadata, OpenOptions, Permissions},
    io::Write,
    net::IpAddr,
    os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::{
        Arc,
        mpsc::{self, Receiver, TryRecvError},
    },
};
use voiceport_core::{CancellationToken, TranscriptionClient, audio::MAXIMUM_AUDIO_BYTES};

const RECEIPTS: usize = 4;
// Leave room for Status + response envelope even if every transcript byte
// expands to a six-byte JSON escape in the shared CLI transport.
const MAX_TEXT_BYTES: usize = 40_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Destination {
    OnDevice,
    ChatGpt,
    Http { origin: String, loopback: bool },
    ExternalProgram,
}

impl Destination {
    pub(crate) fn requires_confirmation(&self) -> bool {
        !matches!(self, Self::OnDevice)
    }

    pub(crate) fn label(&self) -> String {
        match self {
            Self::OnDevice => "本机模型".into(),
            Self::ChatGpt => "ChatGPT · chatgpt.com".into(),
            Self::Http {
                origin,
                loopback: true,
            } => format!("{origin}（本机服务，可能转发）"),
            Self::Http {
                origin,
                loopback: false,
            } => origin.clone(),
            Self::ExternalProgram => "外部程序（可能转发）".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Target {
    pub source: String,
    pub source_name: String,
    pub language: String,
    pub destination: Destination,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum State {
    Working,
    Cancelling,
    Ready,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Status {
    pub operation: String,
    pub generation: u64,
    pub state: State,
    #[serde(flatten)]
    pub target: Target,
    pub delivery: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<Error>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Error {
    pub code: &'static str,
    pub message: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<Box<Target>>,
}

impl Error {
    pub(crate) const fn new(code: &'static str, message: &'static str) -> Self {
        Self {
            code,
            message,
            target: None,
        }
    }
}

pub(crate) const BUSY: Error = Error::new("busy", "请先结束当前听写或文件转写。");
const MISSING: Error = Error::new("not_found", "找不到这次文件转写。");
const UNAVAILABLE: Error = Error::new("source_unavailable", "所选来源不可用，请选择可用来源。");
const INVALID_AUDIO: Error = Error::new(
    "invalid_audio",
    "请选择可完整解码的 WAV、M4A 或 MP3，时长最多 180 秒。",
);
const INVALID_FILE: Error = Error::new("invalid_file", "请选择一个非空的本地普通文件。");
const TOO_LARGE: Error = Error::new("audio_too_large", "音频文件最多 20 MB。");
const CHANGED: Error = Error::new("file_changed", "文件在转写前发生变化，请重新选择。");
const STAGING: Error = Error::new("file_read_error", "无法读取或准备所选文件。");
const CANCELLED: Error = Error::new("cancelled", "文件转写已取消。");
const TRANSCRIPTION: Error =
    Error::new("transcription_error", "所选来源转写失败，请检查来源配置。");
const EMPTY_TRANSCRIPT: Error = Error::new("empty_transcript", "未识别到文字。");
const CLEANUP: Error = Error::new("cleanup_pending", "外部程序尚未确认退出，请检查退出状态。");
const EXPORT: Error = Error::new(
    "export_error",
    "无法保存转写结果，请选择可写目录中的新文件名。",
);

#[derive(Clone, Copy)]
enum Format {
    Wav,
    M4a,
    Mp3,
}

impl Format {
    fn for_path(path: &Path) -> Result<Self, Error> {
        let extension = path.extension().and_then(|value| value.to_str());
        match extension.map(str::to_ascii_lowercase).as_deref() {
            Some("wav") => Ok(Self::Wav),
            Some("m4a") => Ok(Self::M4a),
            Some("mp3") => Ok(Self::Mp3),
            _ => Err(INVALID_AUDIO),
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::Wav => ".wav",
            Self::M4a => ".m4a",
            Self::Mp3 => ".mp3",
        }
    }

    fn matches_header(self, header: &[u8]) -> bool {
        match self {
            Self::Wav => header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WAVE"),
            Self::M4a => header.get(4..8) == Some(b"ftyp"),
            Self::Mp3 => {
                header.starts_with(b"ID3")
                    || matches!(header, [0xff, next, ..] if next & 0xe0 == 0xe0)
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct Input {
    file: Arc<File>,
    selected: Metadata,
    format: Format,
}

struct Snapshot {
    _directory: tempfile::TempDir,
    path: std::path::PathBuf,
}

impl Input {
    pub(crate) fn open(path: &Path) -> Result<Self, Error> {
        if !path.is_absolute() {
            return Err(INVALID_FILE);
        }
        let format = Format::for_path(path)?;
        let before = path.symlink_metadata().map_err(|_| INVALID_FILE)?;
        if !before.is_file() {
            return Err(INVALID_FILE);
        }
        // No final symlink traversal or blocking on a FIFO substituted at open.
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|_| INVALID_FILE)?;
        let selected = file.metadata().map_err(|_| INVALID_FILE)?;
        if !selected.is_file() || selected.len() == 0 {
            return Err(INVALID_FILE);
        }
        if (before.dev(), before.ino()) != (selected.dev(), selected.ino()) {
            return Err(CHANGED);
        }
        if selected.len() > MAXIMUM_AUDIO_BYTES {
            return Err(TOO_LARGE);
        }
        Ok(Self {
            file: Arc::new(file),
            selected,
            format,
        })
    }

    fn stage(self, cancel: &CancellationToken) -> Result<Snapshot, Error> {
        check_cancel(cancel)?;
        self.verify_unchanged()?;
        let directory = tempfile::Builder::new()
            .prefix("voiceport-file-")
            .permissions(Permissions::from_mode(0o700))
            .tempdir_in(std::env::temp_dir().canonicalize().map_err(|_| STAGING)?)
            .map_err(|_| STAGING)?;
        let path = directory
            .path()
            .join(format!("audio{}", self.format.suffix()));
        let mut snapshot = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|_| STAGING)?;
        let mut buffer = [0; 32 * 1024];
        let mut remaining = self.selected.len();
        while remaining > 0 {
            check_cancel(cancel)?;
            let count = remaining.min(buffer.len() as u64) as usize;
            let chunk = &mut buffer[..count];
            // A GUI retry uses the original file descriptor and starts at byte zero.
            let offset = self.selected.len() - remaining;
            self.file
                .read_exact_at(chunk, offset)
                .map_err(|_| CHANGED)?;
            if offset == 0 && !self.format.matches_header(chunk) {
                return Err(INVALID_AUDIO);
            }
            snapshot.write_all(chunk).map_err(|_| STAGING)?;
            remaining -= count as u64;
        }
        self.verify_unchanged()?;
        Ok(Snapshot {
            _directory: directory,
            path,
        })
    }

    fn verify_unchanged(&self) -> Result<(), Error> {
        let current = self.file.metadata().map_err(|_| STAGING)?;
        let fingerprint = |metadata: &Metadata| {
            (
                metadata.len(),
                metadata.mtime(),
                metadata.mtime_nsec(),
                metadata.ctime(),
                metadata.ctime_nsec(),
            )
        };
        if fingerprint(&self.selected) != fingerprint(&current) {
            return Err(CHANGED);
        }
        Ok(())
    }
}

#[derive(Clone)]
pub(crate) struct Plan {
    pub target: Target,
    source: SessionSource,
}

fn destination(source: &SessionSource) -> Option<Destination> {
    match source {
        SessionSource::Local(_) => Some(Destination::OnDevice),
        SessionSource::ChatGpt => Some(Destination::ChatGpt),
        SessionSource::Plugin(plugin) if plugin.manifest.capabilities.batch => {
            match plugin.manifest.adapter {
                Adapter::Http => {
                    let endpoint = validate_endpoint(plugin.source.config.get("endpoint")?).ok()?;
                    let loopback = endpoint.host_str().is_some_and(|host| {
                        host == "localhost"
                            || host
                                .trim_matches(['[', ']'])
                                .parse::<IpAddr>()
                                .is_ok_and(|ip| ip.is_loopback())
                    });
                    Some(Destination::Http {
                        origin: endpoint.origin().ascii_serialization(),
                        loopback,
                    })
                }
                Adapter::Jsonl => Some(Destination::ExternalProgram),
                _ => None,
            }
        }
        _ => None,
    }
}

impl Plan {
    pub(crate) fn resolve(
        catalog: &Catalog,
        source_id: &str,
        language: &str,
    ) -> Result<Self, Error> {
        let entry = catalog
            .entries
            .iter()
            .find(|entry| entry.id == source_id)
            .ok_or(UNAVAILABLE)?;
        let source = entry.source.clone().map_err(|_| UNAVAILABLE)?;
        let destination = destination(&source).ok_or(Error::new(
            "unsupported_source",
            "所选来源不支持文件转写，请选择支持文件的来源。",
        ))?;
        let language = if language == "auto" { "" } else { language };
        if !voiceport_core::language::supported(language)
            || matches!(&source, SessionSource::Local(model) if !model.kind.supports_language(language))
        {
            return Err(Error::new(
                "unsupported_language",
                "所选来源不支持当前语言。",
            ));
        }
        Ok(Self {
            target: Target {
                source: entry.id.clone(),
                source_name: entry.title.clone(),
                language: language.into(),
                destination,
            },
            source,
        })
    }

    pub(crate) fn verify_source(&self, catalog: &Catalog) -> Result<(), Error> {
        if catalog.snapshot(&self.target.source).as_ref() != Ok(&self.source) {
            return Err(Error::new(
                "source_changed",
                "所选来源配置已变化，请重新选取音频。",
            ));
        }
        Ok(())
    }
}

pub(crate) fn sources(catalog: &Catalog, language: &str) -> Vec<Target> {
    catalog
        .entries
        .iter()
        .filter_map(|entry| {
            Plan::resolve(catalog, &entry.id, language)
                .ok()
                .map(|plan| plan.target)
        })
        .collect()
}

#[derive(Clone, Copy)]
pub(crate) enum ExportFormat {
    Text,
    Markdown,
}

impl ExportFormat {
    pub(crate) fn extension(self) -> &'static str {
        match self {
            Self::Text => "txt",
            Self::Markdown => "md",
        }
    }
}

pub(crate) struct Work {
    input: Input,
    plan: Plan,
    cancel: CancellationToken,
    custody: Custody,
}

impl Work {
    fn execute(
        self,
        transcribe: impl FnOnce(&Plan, &Path, &CancellationToken, &Custody) -> Result<String, Error>,
    ) -> Result<String, Error> {
        let snapshot = self.input.stage(&self.cancel)?;
        let path = if matches!(self.plan.source, SessionSource::ChatGpt) {
            // The portable ChatGPT client uploads its input bytes as-is. Give
            // it only resampled audio, never source containers or metadata.
            stage_upload_wav(&snapshot.path, &self.cancel)?
        } else {
            // Use the same bounded decoder as the local engine before dispatch.
            // Configured adapters already normalize before sending audio.
            validate_audio(&snapshot.path, &self.cancel)?;
            snapshot.path.clone()
        };
        check_cancel(&self.cancel)?;
        let text = transcribe(&self.plan, &path, &self.cancel, &self.custody)?;
        check_cancel(&self.cancel)?;
        finish_text(&text)
    }
}

fn validate_audio(path: &Path, cancel: &CancellationToken) -> Result<(), Error> {
    crate::audio::read_pcm(path, |_| check_cancel(cancel).map_err(|e| e.message.into()))
        .map_err(|_| INVALID_AUDIO)
}

fn finish_text(text: &str) -> Result<String, Error> {
    let text = text.trim();
    if text.is_empty() {
        return Err(EMPTY_TRANSCRIPT);
    }
    if text.len() > MAX_TEXT_BYTES {
        return Err(Error::new("transcript_too_large", "转写结果超过大小限制。"));
    }
    Ok(text.to_owned())
}

/// Decode a user-selected file via one explicit local model, without opening
/// AppKit, reading configured sources, or invoking any installer/provider.
#[cfg(target_arch = "aarch64")]
pub fn transcribe_local_file(
    path: &Path,
    model_id: &str,
    language: &str,
    cancel: &CancellationToken,
) -> Result<String, voiceport_control::Response> {
    let run = || {
        check_cancel(cancel)?;
        let (mut model, language) = select_local_model(model_id, language)?;
        transcribe_local_input(path, cancel, |audio| {
            // Keep single-file error precedence: validate input before staging the engine.
            let _runtime = stage_local_runtime(&mut model)?;
            check_cancel(cancel)?;
            crate::local_runtime::transcribe_file_classified(
                &model,
                language,
                audio,
                cancel,
                |_| {},
            )
            .map_err(local_file_error)
        })
    };
    local_response(run(), cancel)
}

/// Prepare the installed engine before reading any input, then process in order.
/// The outer error is for batch setup or cancellation; each row is a file outcome.
#[cfg(target_arch = "aarch64")]
pub fn transcribe_local_batch(
    paths: &[std::path::PathBuf],
    model_id: &str,
    language: &str,
    cancel: &CancellationToken,
) -> Result<Vec<Result<String, voiceport_control::Failure>>, voiceport_control::Response> {
    let run = || {
        check_cancel(cancel)?;
        let (mut model, language) = select_local_model(model_id, language)?;
        check_cancel(cancel)?;
        let _runtime = stage_local_runtime(&mut model)?;
        let mut recognizer = crate::local_runtime::BatchRecognizer::open(&model, language, cancel)
            .map_err(|_| LOCAL_UNAVAILABLE)?;
        transcribe_local_batch_inputs(paths, cancel, |audio| {
            recognizer
                .transcribe(audio, cancel)
                .map_err(local_file_error)
        })
    };
    Ok(local_response(run(), cancel)?
        .into_iter()
        .map(|result| {
            result.map_err(|e| voiceport_control::Failure {
                code: e.code.into(),
                message: e.message.into(),
            })
        })
        .collect())
}

#[cfg(target_arch = "aarch64")]
const LOCAL_UNAVAILABLE: Error = Error::new(
    "source_unavailable",
    "本地模型或识别引擎未安装或不可用。请打开 VoicePort 的「语音模型」安装或修复后再试。",
);

#[cfg(target_arch = "aarch64")]
fn select_local_model<'a>(
    model_id: &str,
    language: &'a str,
) -> Result<(crate::local_models::Model, &'a str), Error> {
    use crate::model_library::{MODELS, Paths};
    let definition = MODELS.iter().find(|m| m.id == model_id).ok_or(Error::new(
        "not_found",
        "未知本地模型 ID。请查看 voiceport transcribe-local --help。",
    ))?;
    let language = if language == "auto" { "" } else { language };
    if !voiceport_core::language::supported(language)
        || !definition.kind.supports_language(language)
    {
        return Err(Error::new(
            "unsupported_language",
            "所选来源不支持当前语言。",
        ));
    }
    let paths = Paths::system().map_err(|_| {
        Error::new(
            "invalid_config",
            "无法定位模型目录。请检查 HOME 或非空绝对路径 VOICEPORT_MODELS_PATH。",
        )
    })?;
    let catalog = Catalog::system_models(model_id, Some(&paths));
    match catalog.snapshot(model_id) {
        Ok(SessionSource::Local(model)) => Ok((model, language)),
        _ => Err(LOCAL_UNAVAILABLE),
    }
}

#[cfg(target_arch = "aarch64")]
fn stage_local_runtime(model: &mut crate::local_models::Model) -> Result<tempfile::TempDir, Error> {
    let runtime = crate::local_runtime::stage_verified_runtime(&model.runtime).map_err(|_| {
        Error::new(
            "source_unavailable",
            "无法验证或暂存识别引擎。请检查临时目录，并确认已安装原始发行版引擎文件。",
        )
    })?;
    model.runtime = runtime.path().to_path_buf();
    Ok(runtime)
}

#[cfg(target_arch = "aarch64")]
fn local_file_error(error: crate::local_runtime::FileError) -> Error {
    use crate::local_runtime::FileError;
    match error {
        FileError::NoSpeech | FileError::NoText => EMPTY_TRANSCRIPT,
        FileError::Failed(_) => TRANSCRIPTION,
    }
}

#[cfg(target_arch = "aarch64")]
fn local_response<T>(
    result: Result<T, Error>,
    cancel: &CancellationToken,
) -> Result<T, voiceport_control::Response> {
    // Cancellation takes precedence even when a blocking native call fails.
    check_cancel(cancel)
        .and(result)
        .map_err(|e| voiceport_control::Response::error(e.code, e.message))
}

#[cfg(target_arch = "aarch64")]
fn transcribe_local_batch_inputs(
    paths: &[std::path::PathBuf],
    cancel: &CancellationToken,
    mut recognize: impl FnMut(&Path) -> Result<String, Error>,
) -> Result<Vec<Result<String, Error>>, Error> {
    paths
        .iter()
        .map(|path| {
            let result = transcribe_local_input(path, cancel, &mut recognize);
            check_cancel(cancel)?;
            Ok(result)
        })
        .collect()
}

#[cfg(target_arch = "aarch64")]
fn transcribe_local_input(
    path: &Path,
    cancel: &CancellationToken,
    recognize: impl FnOnce(&Path) -> Result<String, Error>,
) -> Result<String, Error> {
    check_cancel(cancel)?;
    let path = std::path::absolute(path).map_err(|_| INVALID_FILE)?;
    let snapshot = Input::open(&path)?.stage(cancel)?;
    validate_audio(&snapshot.path, cancel)?;
    check_cancel(cancel)?;
    let result = recognize(&snapshot.path);
    check_cancel(cancel)?;
    finish_text(&result?)
}

fn stage_upload_wav(path: &Path, cancel: &CancellationToken) -> Result<std::path::PathBuf, Error> {
    let wav = crate::plugin_runtime::convert(path, cancel).map_err(|_| INVALID_AUDIO)?;
    check_cancel(cancel)?;
    let normalized = path.with_file_name("upload.wav");
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&normalized)
        .and_then(|mut file| file.write_all(&wav))
        .map_err(|_| STAGING)?;
    Ok(normalized)
}

type Transcriber = fn(&Plan, &Path, &CancellationToken, &Custody) -> Result<String, Error>;

fn run_source(
    plan: &Plan,
    path: &Path,
    cancel: &CancellationToken,
    custody: &Custody,
) -> Result<String, Error> {
    let language = &plan.target.language;
    let source = plan.source.clone().pin().map_err(|_| UNAVAILABLE)?;
    check_cancel(cancel)?;
    match source {
        SessionSource::Local(model) => {
            crate::local_runtime::transcribe_file(&model, language, path, cancel, |_| {})
                .map_err(|_| TRANSCRIPTION)
        }
        SessionSource::Plugin(plugin) => plugin
            .transcribe(path, language, cancel, custody)
            .map_err(|_| TRANSCRIPTION),
        SessionSource::ChatGpt => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|_| TRANSCRIPTION)?;
            runtime.block_on(async {
                let auth = voiceport_core::auth::default_path().map_err(|_| UNAVAILABLE)?;
                let proxy = crate::proxy::https_proxy();
                let client = TranscriptionClient::new_with_proxy(auth, proxy.as_deref())
                    .map_err(|_| TRANSCRIPTION)?;
                client
                    .transcribe(
                        path,
                        (!language.is_empty()).then_some(language.as_str()),
                        cancel,
                    )
                    .await
                    .map_err(|error| match error {
                        voiceport_core::Error::EmptyTranscript => EMPTY_TRANSCRIPT,
                        _ => TRANSCRIPTION,
                    })
            })
        }
    }
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), Error> {
    if cancel.is_cancelled() {
        Err(CANCELLED)
    } else {
        Ok(())
    }
}

struct Active {
    status: Status,
    receiver: Receiver<(u64, Result<String, Error>)>,
    completion: Option<Result<String, Error>>,
    cancel: CancellationToken,
    custody: Custody,
}

struct Completed {
    status: Status,
    text: Option<String>,
}

#[derive(Default)]
pub(crate) struct Jobs {
    generation: u64,
    active: Option<Active>,
    completed: VecDeque<Completed>,
    page_operation: Option<String>,
    #[cfg(any(test, debug_assertions))]
    transcriber: Option<Transcriber>,
}

impl Jobs {
    pub(crate) fn busy(&self) -> bool {
        self.active.is_some()
    }

    pub(crate) fn start(
        &mut self,
        input: Input,
        plan: Plan,
        accept_upload: bool,
    ) -> Result<Status, Error> {
        if self.busy() {
            return Err(BUSY);
        }
        if plan.target.destination.requires_confirmation() && !accept_upload {
            return Err(Error {
                target: Some(Box::new(plan.target)),
                ..Error::new("confirmation_required", "请先确认音频去向。")
            });
        }
        let generation = self.generation.checked_add(1).ok_or(TRANSCRIPTION)?;
        let status = Status {
            operation: uuid::Uuid::new_v4().to_string(),
            generation,
            state: State::Working,
            target: plan.target.clone(),
            delivery: "none",
            error: None,
        };
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancel = CancellationToken::new();
        let custody = Custody::default();
        let work = Work {
            input,
            plan,
            cancel: cancel.clone(),
            custody: custody.clone(),
        };
        let transcriber: Transcriber = run_source;
        #[cfg(any(test, debug_assertions))]
        let transcriber = self.transcriber.unwrap_or(transcriber);
        std::thread::Builder::new()
            .name("voiceport-file-transcription".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(|| work.execute(transcriber))
                    .unwrap_or(Err(TRANSCRIPTION));
                let _ = sender.send((generation, result));
            })
            .map_err(|_| TRANSCRIPTION)?;
        self.generation = generation;
        self.active = Some(Active {
            status: status.clone(),
            receiver,
            completion: None,
            cancel,
            custody,
        });
        Ok(status)
    }

    pub(crate) fn poll(&mut self) {
        let Some(active) = &mut self.active else {
            return;
        };
        if active.completion.is_none() {
            active.completion = match active.receiver.try_recv() {
                Ok((generation, result)) if generation == active.status.generation => Some(result),
                Ok(_) | Err(TryRecvError::Empty) => None,
                Err(TryRecvError::Disconnected) => Some(Err(TRANSCRIPTION)),
            };
        }
        if active.completion.is_none() {
            return;
        }
        if active.custody.unresolved() {
            active.status.error = Some(CLEANUP);
            return;
        }
        let Some(mut active) = self.active.take() else {
            return;
        };
        let result = if active.cancel.is_cancelled() {
            Err(CANCELLED)
        } else {
            active.completion.take().expect("settled job")
        };
        let text = match result {
            Ok(text) => {
                active.status.state = State::Ready;
                active.status.error = None;
                Some(text)
            }
            Err(error) => {
                active.status.state = if error == CANCELLED {
                    State::Cancelled
                } else {
                    State::Failed
                };
                active.status.error = Some(error);
                None
            }
        };
        if self.completed.len() == RECEIPTS {
            let oldest_unpinned = self
                .completed
                .iter()
                .rposition(|job| {
                    self.page_operation.as_deref() != Some(job.status.operation.as_str())
                })
                .expect("at most one page operation is pinned");
            self.completed.remove(oldest_unpinned);
        }
        self.completed.push_front(Completed {
            status: active.status,
            text,
        });
    }

    // Other automation may run while the user chooses an export location.
    // Budget the visible GUI operation within the same bounded receipts.
    pub(crate) fn keep_for_page(&mut self, operation: &str) {
        self.page_operation = Some(operation.to_owned());
    }

    pub(crate) fn status(&self, operation: &str) -> Result<&Status, Error> {
        if let Some(active) = &self.active
            && active.status.operation == operation
        {
            return Ok(&active.status);
        }
        Ok(&self.completed(operation)?.status)
    }

    fn completed(&self, operation: &str) -> Result<&Completed, Error> {
        self.completed
            .iter()
            .find(|job| job.status.operation == operation)
            .ok_or(MISSING)
    }

    /// A pending, failed or cancelled result carries status and has no text.
    pub(crate) fn result(&self, operation: &str) -> Result<(&Status, Option<&str>), Error> {
        let status = self.status(operation)?;
        let text = if status.state == State::Ready {
            self.completed(operation)?.text.as_deref()
        } else {
            None
        };
        Ok((status, text))
    }

    pub(crate) fn cancel(&mut self, operation: &str) -> Result<Status, Error> {
        if let Some(active) = &mut self.active
            && active.status.operation == operation
        {
            // Once an adapter reports unresolved cleanup, a further explicit
            // action only observes its original process group, never re-signals.
            if active.completion.is_some() && active.custody.unresolved() {
                active.custody.observe_once();
            } else {
                active.status.state = State::Cancelling;
                active.cancel.cancel();
            }
            self.poll();
        }
        self.status(operation).cloned()
    }

    pub(crate) fn cancel_for_shutdown(&mut self) {
        if let Some(active) = &self.active {
            active.cancel.cancel();
            if active.completion.is_some() && active.custody.unresolved() {
                active.custody.observe_once();
            }
        }
    }

    pub(crate) fn export(
        &self,
        operation: &str,
        path: &Path,
        format: ExportFormat,
    ) -> Result<(), Error> {
        let (_, text) = self.result(operation)?;
        let text = text.ok_or(Error::new("invalid_state", "转写结果尚不可导出。"))?;
        if !path.is_absolute()
            || path.extension().and_then(|value| value.to_str()) != Some(format.extension())
        {
            return Err(EXPORT);
        }
        let mut temporary =
            tempfile::NamedTempFile::new_in(path.parent().ok_or(EXPORT)?).map_err(|_| EXPORT)?;
        // Markdown preserves the transcript as plain text blocks, so source text
        // cannot introduce links or executable markup into the exported document.
        let file = temporary.as_file_mut();
        match format {
            ExportFormat::Text => file.write_all(text.as_bytes()),
            ExportFormat::Markdown => {
                writeln!(file, "# 转写结果\n").map_err(|_| EXPORT)?;
                text.lines()
                    .try_for_each(|line| writeln!(file, "    {line}"))
            }
        }
        .and_then(|_| file.sync_all())
        .map_err(|_| EXPORT)?;
        temporary.persist_noclobber(path).map_err(|_| EXPORT)?;
        Ok(())
    }

    #[cfg(any(test, debug_assertions))]
    pub(crate) fn use_test_transcriber(&mut self, transcriber: Transcriber) {
        self.transcriber = Some(transcriber);
    }
}

impl Drop for Jobs {
    fn drop(&mut self) {
        self.cancel_for_shutdown();
    }
}

#[cfg(test)]
#[path = "file_transcription_tests.rs"]
mod tests;
