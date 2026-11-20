//! Read-only source discovery across VoicePort and compatible Orca installations.
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub const CHATGPT: &str = "chatgpt";
pub const PARAFORMER: &str = "orca:paraformer-bilingual-zh-en";
pub const WHISPER: &str = "orca:whisper-tiny";
pub const WHISPER_TURBO: &str = "orca:whisper-large-v3-turbo";
pub const SENSEVOICE: &str = "orca:sensevoice";
pub const PARAKEET_TDT_V3: &str = "orca:parakeet-tdt-v3";
pub const PARAKEET_TDT_V2: &str = "orca:parakeet-tdt-v2";
pub const PARAKEET_UNIFIED: &str = "orca:parakeet-unified-en";
pub const NEMOTRON: &str = "orca:nemotron-3.5";

/// NVIDIA's published language coverage; these transducers detect the spoken
/// language rather than using a Whisper-style language prompt.
pub const PARAKEET_V3_LANGUAGES: &[&str] = &[
    "bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv", "lt", "mt",
    "pl", "pt", "ro", "sk", "sl", "es", "sv", "ru", "uk",
];
/// Out-of-box transcription languages only. The eight adaptation-only locales
/// in the Nemotron 3.5 card are deliberately excluded. `no` selects nb-NO.
pub const NEMOTRON_LANGUAGES: &[&str] = &[
    "en", "es", "fr", "it", "pt", "nl", "de", "tr", "ru", "ar", "hi", "ja", "ko", "vi", "uk", "pl",
    "sv", "cs", "no", "da", "bg", "fi", "hr", "sk", "zh", "hu", "ro", "et",
];
pub const RUNTIME_VERSION: &str = "2.0.1";
pub const LEGACY_RUNTIME_VERSION: &str = "1.12.37";
pub const RUNTIME_DIRECTORY: &str =
    "/Applications/Orca.app/Contents/Resources/node_modules/sherpa-onnx-darwin-arm64";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Paraformer,
    Whisper,
    WhisperTurbo,
    SenseVoice,
    ParakeetTdtV3,
    ParakeetTdtV2,
    ParakeetUnified,
    Nemotron,
}
impl ModelKind {
    /// Must match the explicit modes in local_runtime/bridge.cc.
    pub(crate) fn native_mode(self) -> i32 {
        match self {
            Self::Paraformer => 1,
            Self::Whisper => 2,
            Self::WhisperTurbo => 3,
            Self::SenseVoice => 4,
            Self::ParakeetTdtV3 => 5,
            Self::ParakeetTdtV2 => 6,
            Self::ParakeetUnified => 7,
            Self::Nemotron => 8,
        }
    }
    pub fn streaming(self) -> bool {
        matches!(
            self,
            Self::Paraformer | Self::ParakeetUnified | Self::Nemotron
        )
    }
    pub fn supports_language(self, language: &str) -> bool {
        if language.is_empty() {
            return true;
        }
        match self {
            Self::Paraformer => matches!(language, "zh" | "en"),
            Self::Whisper => language != "yue" && voiceport_core::language::supported(language),
            Self::WhisperTurbo => voiceport_core::language::supported(language),
            Self::SenseVoice => matches!(language, "zh" | "en" | "ja" | "ko" | "yue"),
            Self::ParakeetTdtV3 => PARAKEET_V3_LANGUAGES.contains(&language),
            Self::ParakeetTdtV2 | Self::ParakeetUnified => language == "en",
            Self::Nemotron => NEMOTRON_LANGUAGES.contains(&language),
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub kind: ModelKind,
    pub directory: PathBuf,
    pub runtime: PathBuf,
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joiner: Option<PathBuf>,
    pub tokens: PathBuf,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSource {
    ChatGpt,
    Local(Model),
    #[cfg(target_os = "macos")]
    Plugin(Box<crate::plugin_runtime::Snapshot>),
}
impl SessionSource {
    pub fn uses_system_speech(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            self.descriptor().capabilities.system_preview
        }
        #[cfg(not(target_os = "macos"))]
        matches!(self, Self::ChatGpt)
    }
    #[cfg(target_os = "macos")]
    pub fn descriptor(&self) -> crate::plugins::Manifest {
        use crate::plugins::*;
        let id = match self {
            Self::ChatGpt => CHATGPT_PLUGIN,
            Self::Local(m) => match m.kind {
                ModelKind::Paraformer => PARA_PLUGIN,
                ModelKind::Whisper | ModelKind::WhisperTurbo => WHISPER_PLUGIN,
                ModelKind::SenseVoice => SENSEVOICE_PLUGIN,
                ModelKind::ParakeetTdtV3 | ModelKind::ParakeetTdtV2 => PARAKEET_PLUGIN,
                ModelKind::ParakeetUnified => PARAKEET_STREAMING_PLUGIN,
                ModelKind::Nemotron => NEMOTRON_PLUGIN,
            },
            Self::Plugin(p) => return p.manifest.clone(),
        };
        builtins()
            .into_iter()
            .find(|p| p.id == id)
            .expect("builtin descriptor")
    }
    #[cfg(target_os = "macos")]
    pub fn pin(self) -> Result<Self, String> {
        match self {
            Self::Plugin(p) => Ok(Self::Plugin(Box::new(p.pin()?))),
            other => Ok(other),
        }
    }
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local(_))
    }
    pub fn streaming(&self) -> bool {
        matches!(self, Self::Local(model) if model.kind.streaming())
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub detail: String,
    pub source: Result<SessionSource, String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub entries: Vec<Entry>,
}

const ORCA_HOME_ERROR: &str = "无法定位 Orca 模型目录。请检查 HOME。";

pub(crate) fn orca_model_root() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(|home| PathBuf::from(home).join("Library/Application Support/orca/speech-models"))
}

pub(crate) struct OrcaDiscovery<'a>(Result<(&'a Path, PathBuf), String>);

impl<'a> OrcaDiscovery<'a> {
    fn new(root: &'a Path, runtime: PathBuf, architecture: &str) -> Self {
        Self(validate_runtime(&runtime, architecture).map(|()| (root, runtime)))
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn system(
        root: Option<&'a Path>,
        paths: Option<&crate::model_library::Paths>,
    ) -> Self {
        let result = (|| {
            let root = root.ok_or(ORCA_HOME_ERROR)?;
            let metadata = root.metadata().map_err(|error| error.to_string())?;
            if !metadata.is_dir() {
                return Err(format!("Orca 模型路径不是目录：{}", root.display()));
            }
            let runtime = paths.map_or_else(
                || PathBuf::from(RUNTIME_DIRECTORY),
                |paths| paths.runtime_or_orca(),
            );
            validate_runtime(&runtime, std::env::consts::ARCH)?;
            Ok((root, runtime))
        })();
        Self(result)
    }

    pub(crate) fn discover(&self, directory: &str, kind: ModelKind) -> Result<Model, String> {
        let (root, runtime) = self.0.as_ref().map_err(Clone::clone)?;
        Model::discover(root.join(directory), runtime, kind)
    }
}

impl Catalog {
    pub fn pending(saved: &str) -> Self {
        let mut catalog = Self {
            entries: vec![cloud()],
        };
        if saved != CHATGPT {
            catalog
                .entries
                .push(unavailable(saved, "正在检查本地模型…"));
        }
        catalog
    }
    pub fn discover(root: &Path, runtime: &Path, saved: &str, architecture: &str) -> Self {
        Self::discover_orca(
            &OrcaDiscovery::new(root, runtime.into(), architecture),
            saved,
        )
    }
    fn discover_orca(orca: &OrcaDiscovery<'_>, saved: &str) -> Self {
        let mut catalog = Self {
            entries: vec![cloud()],
        };
        for (id, directory, kind) in [
            (
                PARAFORMER,
                "paraformer-bilingual-zh-en",
                ModelKind::Paraformer,
            ),
            (WHISPER, "whisper-tiny", ModelKind::Whisper),
            (
                WHISPER_TURBO,
                "whisper-large-v3-turbo",
                ModelKind::WhisperTurbo,
            ),
            (SENSEVOICE, "sensevoice", ModelKind::SenseVoice),
            (PARAKEET_TDT_V3, "parakeet-tdt-v3", ModelKind::ParakeetTdtV3),
            (PARAKEET_TDT_V2, "parakeet-tdt-v2", ModelKind::ParakeetTdtV2),
            (
                PARAKEET_UNIFIED,
                "parakeet-unified-en",
                ModelKind::ParakeetUnified,
            ),
            (NEMOTRON, "nemotron-3.5", ModelKind::Nemotron),
        ] {
            match orca.discover(directory, kind) {
                Ok(model) => catalog.entries.push(local_entry(id, model)),
                Err(error) if id == saved => catalog.entries.push(unavailable(id, &error)),
                Err(_) => {}
            }
        }
        if !catalog.entries.iter().any(|entry| entry.id == saved) {
            catalog
                .entries
                .push(unavailable(saved, "保存的转写来源不受支持，请重新选择。"));
        }
        catalog
    }
    #[cfg(target_os = "macos")]
    pub fn system(saved: &str) -> Self {
        let root = orca_model_root();
        let paths = crate::model_library::Paths::system().ok();
        let orca = OrcaDiscovery::system(root.as_deref(), paths.as_ref());
        let mut catalog = Self::discover_orca(&orca, saved);
        if let Some(paths) = paths {
            for definition in crate::model_library::MODELS {
                if let Some(result) = paths.discover_owned(definition) {
                    let entry = match result {
                        Ok(model) => local_entry(definition.id, model),
                        Err(error) => unavailable(definition.id, &error),
                    };
                    catalog.entries.retain(|e| e.id != definition.id);
                    catalog.entries.push(entry);
                }
            }
        }
        catalog.load_configured(saved);
        catalog
    }
    #[cfg(not(target_os = "macos"))]
    pub fn system(saved: &str) -> Self {
        if let Some(root) = orca_model_root() {
            return Self::discover(
                &root,
                Path::new(RUNTIME_DIRECTORY),
                saved,
                std::env::consts::ARCH,
            );
        }
        let mut catalog = Self::pending(saved);
        if saved != CHATGPT {
            catalog.entries[1] = unavailable(saved, ORCA_HOME_ERROR);
        }
        catalog
    }
    #[cfg(target_os = "macos")]
    fn load_configured(&mut self, saved: &str) {
        match crate::plugins::default_path().and_then(|path| crate::plugins::Store::load(&path)) {
            Ok(store) => self.add_configured(&store),
            Err(error) => {
                self.entries.push(Entry {
                    id: "configuration-error".into(),
                    title: "自定义来源配置不可用".into(),
                    detail: error.clone(),
                    source: Err(error.clone()),
                });
                if saved != CHATGPT && !self.entries.iter().any(|e| e.id == saved) {
                    self.entries.push(unavailable(saved, &error));
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    pub fn add_configured(&mut self, store: &crate::plugins::Store) {
        for source in &store.sources {
            self.entries.retain(|e| e.id != source.id);
            let snapshot = crate::plugin_runtime::Snapshot::configured(store, source);
            let detail = match &snapshot {
                Ok(s) => format!(
                    "{} · {} · 停止后转写；仅使用此来源，不启用系统语音预览。",
                    s.manifest.name, s.manifest.version
                ),
                Err(e) => e.clone(),
            };
            self.entries.push(Entry {
                id: source.id.clone(),
                title: format!(
                    "{}{}",
                    source.name,
                    if snapshot.is_ok() {
                        ""
                    } else {
                        "（不可用）"
                    }
                ),
                detail,
                source: snapshot.map(|p| SessionSource::Plugin(Box::new(p))),
            });
        }
    }
    pub fn snapshot(&self, selected: &str) -> Result<SessionSource, String> {
        self.entries
            .iter()
            .find(|e| e.id == selected)
            .ok_or_else(|| "所选转写来源不可用，请在设置中重新选择或刷新模型。".to_string())?
            .source
            .clone()
    }
    pub fn detail(&self, selected: &str) -> &str {
        self.entries
            .iter()
            .find(|e| e.id == selected)
            .map_or("所选来源不可用，请重新选择。", |e| {
                e.detail.as_str()
            })
    }
}
impl Model {
    pub(crate) fn discover(
        directory: PathBuf,
        runtime: &Path,
        kind: ModelKind,
    ) -> Result<Self, String> {
        let (encoder, decoder, tokens) = match kind {
            ModelKind::Paraformer => ("encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt"),
            ModelKind::Whisper => (
                "tiny-encoder.int8.onnx",
                "tiny-decoder.int8.onnx",
                "tiny-tokens.txt",
            ),
            ModelKind::WhisperTurbo => (
                "turbo-encoder.int8.onnx",
                "turbo-decoder.int8.onnx",
                "turbo-tokens.txt",
            ),
            // The single SenseVoice model occupies encoder; decoder is unused by
            // its explicit native mode. Preserve the existing Model struct API.
            ModelKind::SenseVoice => ("model.int8.onnx", "model.int8.onnx", "tokens.txt"),
            ModelKind::ParakeetTdtV3
            | ModelKind::ParakeetTdtV2
            | ModelKind::ParakeetUnified
            | ModelKind::Nemotron => ("encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt"),
        };
        let joiner = match kind {
            ModelKind::ParakeetTdtV3
            | ModelKind::ParakeetTdtV2
            | ModelKind::ParakeetUnified
            | ModelKind::Nemotron => Some(directory.join("joiner.int8.onnx")),
            _ => None,
        };
        let model = Self {
            kind,
            encoder: directory.join(encoder),
            decoder: directory.join(decoder),
            joiner,
            tokens: directory.join(tokens),
            directory,
            runtime: runtime.into(),
        };
        model.validate()?;
        Ok(model)
    }
    pub fn validate(&self) -> Result<(), String> {
        for path in [&self.encoder, &self.decoder, &self.tokens] {
            readable(path)?;
        }
        let requires_joiner = matches!(
            self.kind,
            ModelKind::ParakeetTdtV3
                | ModelKind::ParakeetTdtV2
                | ModelKind::ParakeetUnified
                | ModelKind::Nemotron
        );
        if requires_joiner {
            readable(self.joiner.as_deref().ok_or("转写模型缺少 joiner 路径。")?)?;
        }
        validate_runtime_for(&self.runtime, std::env::consts::ARCH, self.kind)
    }
}
fn cloud() -> Entry {
    Entry {
        id: CHATGPT.into(),
        title: title(CHATGPT).into(),
        detail: description(CHATGPT).into(),
        source: Ok(SessionSource::ChatGpt),
    }
}
fn local_entry(id: &str, model: Model) -> Entry {
    Entry {
        id: id.into(),
        title: title(id).into(),
        detail: description(id).into(),
        source: Ok(SessionSource::Local(model)),
    }
}
pub(crate) fn unavailable_reason(id: &str, reason: &str) -> String {
    format!("{}不可用：{reason}", title(id))
}
fn unavailable(id: &str, reason: &str) -> Entry {
    Entry {
        id: id.into(),
        title: format!("{}（不可用）", title(id)),
        detail: format!("{reason} 不会自动切换来源；请刷新或重新选择。"),
        source: Err(unavailable_reason(id, reason)),
    }
}
pub fn title(id: &str) -> &str {
    match id {
        CHATGPT => "ChatGPT",
        PARAFORMER => "Paraformer · 本地流式",
        WHISPER => "Whisper Tiny · 本地离线",
        WHISPER_TURBO => "Whisper Large v3 Turbo · 本地离线",
        SENSEVOICE => "SenseVoice · 本地离线",
        PARAKEET_TDT_V3 => "Parakeet TDT v3 · 本地离线",
        PARAKEET_TDT_V2 => "Parakeet TDT v2 · 本地离线",
        PARAKEET_UNIFIED => "Parakeet Unified EN · 本地流式",
        NEMOTRON => "Nemotron 3.5 · 本地流式",
        other => other,
    }
}
fn description(id: &str) -> &'static str {
    match id {
        CHATGPT => "系统设备端预览；停止后上传至 ChatGPT 完成转写。",
        PARAFORMER => "中英双语边说边出字；停止后在本地完成，无需联网。语言提示不限制双语识别。",
        WHISPER | WHISPER_TURBO => {
            "停止后在本地分段转写；支持自动检测或选择受支持语言，无实时预览；无需联网。"
        }
        SENSEVOICE => {
            "停止后在本地分段转写；自动识别中文、English、日本語、한국어、粤语，无实时预览；无需联网。"
        }
        PARAKEET_TDT_V3 => "停止后本机分段识别 25 种欧洲语言；自动判断语言，无需联网。",
        PARAKEET_TDT_V2 => "停止后本机分段识别英语；无需联网。",
        PARAKEET_UNIFIED => "英语流式预览与本机最终识别；固定 560ms 模型上下文，无需联网。",
        NEMOTRON => "Nemotron 3.5 多语言流式预览与本机最终识别；无需联网。",
        _ => "保存的转写来源不可用。",
    }
}
fn model_file(path: &Path) -> std::io::Result<File> {
    #[cfg(target_os = "macos")]
    let file = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?
    };
    #[cfg(not(target_os = "macos"))]
    let file = File::open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err(std::io::Error::other("empty or not a regular file"));
    }
    Ok(file)
}
fn readable(path: &Path) -> Result<(), String> {
    model_file(path)
        .and_then(|mut file| file.read_exact(&mut [0]))
        .map_err(|_| format!("缺少可读的非空普通文件：{}", path.display()))
}
pub(crate) fn runtime_version(directory: &Path) -> Result<String, String> {
    let file = model_file(&directory.join("package.json")).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > 65536 {
        return Err("本地运行时元数据过大。".into());
    }
    let metadata: serde_json::Value = serde_json::from_reader(file).map_err(|e| e.to_string())?;
    metadata["version"]
        .as_str()
        .map(str::to_owned)
        .ok_or("识别引擎缺少版本。".into())
}
pub(crate) fn validate_runtime_for(
    directory: &Path,
    architecture: &str,
    kind: ModelKind,
) -> Result<(), String> {
    validate_runtime(directory, architecture)?;
    if matches!(kind, ModelKind::ParakeetUnified | ModelKind::Nemotron)
        && runtime_version(directory)? != RUNTIME_VERSION
    {
        return Err(format!(
            "此模型需要 sherpa-onnx {RUNTIME_VERSION}，请安装新版识别引擎。"
        ));
    }
    Ok(())
}
pub(crate) fn validate_runtime(directory: &Path, architecture: &str) -> Result<(), String> {
    if architecture != "aarch64" {
        return Err("本地模型目前仅支持 Apple Silicon arm64。".into());
    }
    let version = runtime_version(directory)?;
    let ort = match version.as_str() {
        RUNTIME_VERSION => "libonnxruntime.dylib",
        LEGACY_RUNTIME_VERSION => "libonnxruntime.1.23.2.dylib",
        _ => {
            return Err(format!(
                "需要兼容的 sherpa-onnx {RUNTIME_VERSION} 或 {LEGACY_RUNTIME_VERSION} 运行时。"
            ));
        }
    };
    for name in ["libsherpa-onnx-c-api.dylib", ort] {
        let path = directory.join(name);
        readable(&path)?;
        let mut header = [0; 8];
        model_file(&path)
            .and_then(|mut f| f.read_exact(&mut header))
            .map_err(|e| e.to_string())?;
        if u32::from_le_bytes(header[..4].try_into().unwrap()) != 0xfeed_facf
            || u32::from_le_bytes(header[4..].try_into().unwrap()) != 0x0100_000c
        {
            return Err(format!("本地运行时不是 arm64 Mach-O：{}", path.display()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn installed(root: &Path) -> PathBuf {
        let runtime = root.join("runtime");
        std::fs::create_dir(&runtime).unwrap();
        std::fs::write(runtime.join("package.json"), br#"{"version":"1.12.37"}"#).unwrap();
        for name in ["libsherpa-onnx-c-api.dylib", "libonnxruntime.1.23.2.dylib"] {
            std::fs::write(runtime.join(name), [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 1]).unwrap();
        }
        for (dir, names) in [
            (
                "paraformer-bilingual-zh-en",
                ["encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt"],
            ),
            (
                "whisper-tiny",
                [
                    "tiny-encoder.int8.onnx",
                    "tiny-decoder.int8.onnx",
                    "tiny-tokens.txt",
                ],
            ),
        ] {
            let dir = root.join(dir);
            std::fs::create_dir(&dir).unwrap();
            for name in names {
                std::fs::write(dir.join(name), [1]).unwrap();
            }
        }
        runtime
    }
    #[test]
    fn discovery_is_read_only_and_keeps_missing_saved_source() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = installed(temp.path());
        let catalog = Catalog::discover(temp.path(), &runtime, PARAFORMER, "aarch64");
        assert_eq!(catalog.entries.len(), 3);
        assert!(catalog.snapshot(PARAFORMER).unwrap().streaming());
        std::fs::write(
            temp.path()
                .join("paraformer-bilingual-zh-en/encoder.int8.onnx"),
            [],
        )
        .unwrap();
        let catalog = Catalog::discover(temp.path(), &runtime, PARAFORMER, "aarch64");
        assert!(catalog.snapshot(PARAFORMER).is_err());
        assert_eq!(catalog.entries[1].id, PARAFORMER);
        let catalog = Catalog::discover(temp.path(), &runtime, CHATGPT, "aarch64");
        assert_eq!(catalog.entries.len(), 2);
        assert!(catalog.snapshot(PARAFORMER).is_err());
    }
    #[test]
    fn expanded_models_discover_distinct_files_and_preserve_old_sources() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = installed(temp.path());
        for (id, directory, kind, names) in [
            (
                WHISPER_TURBO,
                "whisper-large-v3-turbo",
                ModelKind::WhisperTurbo,
                &[
                    "turbo-encoder.int8.onnx",
                    "turbo-decoder.int8.onnx",
                    "turbo-tokens.txt",
                ][..],
            ),
            (
                SENSEVOICE,
                "sensevoice",
                ModelKind::SenseVoice,
                &["model.int8.onnx", "tokens.txt"][..],
            ),
        ] {
            let path = temp.path().join(directory);
            std::fs::create_dir(&path).unwrap();
            // A different model's filenames must not satisfy this model.
            for name in [
                "tiny-encoder.int8.onnx",
                "tiny-decoder.int8.onnx",
                "tiny-tokens.txt",
            ] {
                std::fs::write(path.join(name), [1]).unwrap();
            }
            assert!(
                Catalog::discover(temp.path(), &runtime, id, "aarch64")
                    .snapshot(id)
                    .is_err()
            );
            for name in names {
                std::fs::write(path.join(name), [1]).unwrap();
            }
            let catalog = Catalog::discover(temp.path(), &runtime, id, "aarch64");
            let source = catalog.snapshot(id).unwrap();
            assert!(!source.streaming());
            let SessionSource::Local(model) = &source else {
                panic!("local model")
            };
            assert_eq!(model.kind, kind);
            assert_eq!(model.encoder, path.join(names[0]));
            #[cfg(target_os = "macos")]
            {
                let descriptor = source.descriptor();
                assert_eq!(
                    descriptor.id,
                    if kind == ModelKind::SenseVoice {
                        crate::plugins::SENSEVOICE_PLUGIN
                    } else {
                        crate::plugins::WHISPER_PLUGIN
                    }
                );
                assert!(descriptor.capabilities.batch);
                assert!(!descriptor.capabilities.streaming);
                assert!(!descriptor.capabilities.system_preview);
            }
            assert!(catalog.snapshot(PARAFORMER).unwrap().streaming());
            assert!(catalog.snapshot(WHISPER).is_ok());
            for name in names {
                std::fs::write(path.join(name), []).unwrap();
                assert!(
                    Catalog::discover(temp.path(), &runtime, id, "aarch64")
                        .snapshot(id)
                        .is_err(),
                    "empty {name}"
                );
                std::fs::remove_file(path.join(name)).unwrap();
                assert!(
                    Catalog::discover(temp.path(), &runtime, id, "aarch64")
                        .snapshot(id)
                        .is_err(),
                    "missing {name}"
                );
                std::fs::write(path.join(name), [1]).unwrap();
            }
        }
    }
    #[test]
    fn model_modes_and_language_limits_are_explicit() {
        assert_eq!(ModelKind::Paraformer.native_mode(), 1);
        assert_eq!(ModelKind::Whisper.native_mode(), 2);
        assert_eq!(ModelKind::WhisperTurbo.native_mode(), 3);
        assert_eq!(ModelKind::SenseVoice.native_mode(), 4);
        assert_eq!(ModelKind::ParakeetTdtV3.native_mode(), 5);
        assert_eq!(ModelKind::ParakeetTdtV2.native_mode(), 6);
        assert_eq!(ModelKind::ParakeetUnified.native_mode(), 7);
        assert_eq!(ModelKind::Nemotron.native_mode(), 8);
        for kind in [
            ModelKind::Paraformer,
            ModelKind::Whisper,
            ModelKind::WhisperTurbo,
            ModelKind::SenseVoice,
            ModelKind::ParakeetTdtV3,
            ModelKind::ParakeetTdtV2,
            ModelKind::ParakeetUnified,
            ModelKind::Nemotron,
        ] {
            assert!(kind.supports_language(""));
            for invalid in ["unsupported", "en-US", "en\r\nmodel=other", "JA"] {
                assert!(!kind.supports_language(invalid));
            }
        }
        assert!(ModelKind::Whisper.supports_language("ja"));
        assert!(!ModelKind::Whisper.supports_language("yue"));
        assert!(ModelKind::WhisperTurbo.supports_language("yue"));
        assert!(ModelKind::WhisperTurbo.supports_language("haw"));
        for &(language, _) in voiceport_core::language::LANGUAGES {
            if language.is_empty() {
                continue;
            }
            assert_eq!(
                ModelKind::ParakeetTdtV3.supports_language(language),
                PARAKEET_V3_LANGUAGES.contains(&language)
            );
            assert_eq!(
                ModelKind::Nemotron.supports_language(language),
                NEMOTRON_LANGUAGES.contains(&language)
            );
            assert_eq!(
                ModelKind::ParakeetTdtV2.supports_language(language),
                language == "en"
            );
            assert_eq!(
                ModelKind::ParakeetUnified.supports_language(language),
                language == "en"
            );
        }
        assert_eq!(PARAKEET_V3_LANGUAGES.len(), 25);
        assert_eq!(NEMOTRON_LANGUAGES.len(), 28);
        for adaptation_only in ["el", "lt", "lv", "mt", "sl", "he", "th", "nn"] {
            assert!(!ModelKind::Nemotron.supports_language(adaptation_only));
        }
    }
    #[test]
    fn transducer_discovery_requires_joiner_and_exact_streaming_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = installed(temp.path());
        for (id, directory, kind) in [
            (PARAKEET_TDT_V3, "parakeet-tdt-v3", ModelKind::ParakeetTdtV3),
            (PARAKEET_TDT_V2, "parakeet-tdt-v2", ModelKind::ParakeetTdtV2),
            (
                PARAKEET_UNIFIED,
                "parakeet-unified-en",
                ModelKind::ParakeetUnified,
            ),
            (NEMOTRON, "nemotron-3.5", ModelKind::Nemotron),
        ] {
            let directory = temp.path().join(directory);
            std::fs::create_dir(&directory).unwrap();
            for file in ["encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt"] {
                std::fs::write(directory.join(file), [1]).unwrap();
            }
            assert!(Model::discover(directory.clone(), &runtime, kind).is_err());
            std::fs::write(directory.join("joiner.int8.onnx"), [1]).unwrap();
            let legacy = Model::discover(directory.clone(), &runtime, kind);
            assert_eq!(legacy.is_ok(), !kind.streaming());
            let modern = temp.path().join(format!("modern-{}", kind.native_mode()));
            std::fs::create_dir(&modern).unwrap();
            std::fs::write(modern.join("package.json"), br#"{"version":"2.0.1"}"#).unwrap();
            for file in ["libsherpa-onnx-c-api.dylib", "libonnxruntime.dylib"] {
                std::fs::write(modern.join(file), [0xcf, 0xfa, 0xed, 0xfe, 0x0c, 0, 0, 1]).unwrap();
            }
            let catalog = Catalog::discover(temp.path(), &modern, id, "aarch64");
            let source = catalog.snapshot(id).unwrap();
            assert_eq!(source.streaming(), kind.streaming());
            #[cfg(target_os = "macos")]
            {
                let descriptor = source.descriptor();
                assert!(descriptor.capabilities.batch);
                assert_eq!(descriptor.capabilities.streaming, kind.streaming());
                assert!(!descriptor.capabilities.system_preview);
            }
        }
    }
    #[test]
    fn unsupported_runtime_and_unknown_saved_choice_never_fall_back() {
        let temp = tempfile::tempdir().unwrap();
        let runtime = installed(temp.path());
        for arch in ["x86_64", "unknown"] {
            let c = Catalog::discover(temp.path(), &runtime, WHISPER, arch);
            assert_eq!(c.entries.len(), 2);
            assert!(c.snapshot(WHISPER).is_err());
        }
        std::fs::write(runtime.join("package.json"), br#"{"version":"2"}"#).unwrap();
        assert!(
            Catalog::discover(temp.path(), &runtime, WHISPER, "aarch64")
                .snapshot(WHISPER)
                .is_err()
        );
        let c = Catalog::discover(temp.path(), &runtime, "future:model", "aarch64");
        assert_eq!(c.entries.last().unwrap().id, "future:model");
        assert!(c.snapshot("future:model").is_err());
        assert_eq!(
            Catalog::pending(CHATGPT).snapshot(CHATGPT),
            Ok(SessionSource::ChatGpt)
        );
    }
}
