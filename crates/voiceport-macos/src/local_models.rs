//! Small, read-only catalog. Discovery never loads a model or a dynamic library.
use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
};

pub const CHATGPT: &str = "chatgpt";
pub const PARAFORMER: &str = "orca:paraformer-bilingual-zh-en";
pub const WHISPER: &str = "orca:whisper-tiny";
pub const RUNTIME_VERSION: &str = "1.12.37";
pub const RUNTIME_DIRECTORY: &str =
    "/Applications/Orca.app/Contents/Resources/node_modules/sherpa-onnx-darwin-arm64";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    Paraformer,
    Whisper,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
    pub kind: ModelKind,
    pub directory: PathBuf,
    pub runtime: PathBuf,
    pub encoder: PathBuf,
    pub decoder: PathBuf,
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
                ModelKind::Whisper => WHISPER_PLUGIN,
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
        matches!(
            self,
            Self::Local(Model {
                kind: ModelKind::Paraformer,
                ..
            })
        )
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
        let runtime_check = validate_runtime(runtime, architecture);
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
        ] {
            let result = runtime_check
                .clone()
                .and_then(|_| Model::discover(root.join(directory), runtime, kind));
            match result {
                Ok(model) => catalog.entries.push(Entry {
                    id: id.into(),
                    title: title(id).into(),
                    detail: description(id).into(),
                    source: Ok(SessionSource::Local(model)),
                }),
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
    pub fn system(saved: &str) -> Self {
        let Some(home) = std::env::var_os("HOME").filter(|h| !h.is_empty()) else {
            let mut catalog = Self::pending(saved);
            if saved != CHATGPT {
                catalog.entries[1] = unavailable(saved, "无法定位 Orca 模型目录。请检查 HOME。");
            }
            return catalog;
        };
        let catalog = Self::discover(
            &PathBuf::from(home).join("Library/Application Support/orca/speech-models"),
            Path::new(RUNTIME_DIRECTORY),
            saved,
            std::env::consts::ARCH,
        );
        #[cfg(target_os = "macos")]
        {
            let mut catalog = catalog;
            catalog.load_configured(saved);
            catalog
        }
        #[cfg(not(target_os = "macos"))]
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
    fn discover(directory: PathBuf, runtime: &Path, kind: ModelKind) -> Result<Self, String> {
        let (encoder, decoder, tokens) = match kind {
            ModelKind::Paraformer => ("encoder.int8.onnx", "decoder.int8.onnx", "tokens.txt"),
            ModelKind::Whisper => (
                "tiny-encoder.int8.onnx",
                "tiny-decoder.int8.onnx",
                "tiny-tokens.txt",
            ),
        };
        let model = Self {
            kind,
            encoder: directory.join(encoder),
            decoder: directory.join(decoder),
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
        validate_runtime(&self.runtime, std::env::consts::ARCH)
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
fn unavailable(id: &str, reason: &str) -> Entry {
    Entry {
        id: id.into(),
        title: format!("{}（不可用）", title(id)),
        detail: format!("{reason} 不会自动切换来源；请刷新或重新选择。"),
        source: Err(format!("{}不可用：{reason}", title(id))),
    }
}
pub fn title(id: &str) -> &str {
    match id {
        CHATGPT => "ChatGPT",
        PARAFORMER => "Paraformer · 本地流式",
        WHISPER => "Whisper Tiny · 本地离线",
        other => other,
    }
}
fn description(id: &str) -> &'static str {
    match id {
        CHATGPT => "系统设备端预览；停止后上传至 ChatGPT 完成转写。",
        PARAFORMER => "中英双语边说边出字；停止后在本地完成，无需联网。语言提示不限制双语识别。",
        WHISPER => "停止后在本地分段转写，无实时预览；无需联网。",
        _ => "保存的转写来源不可用。",
    }
}
fn readable(path: &Path) -> Result<(), String> {
    let result = (|| {
        let mut file = File::open(path)?;
        if !file.metadata()?.is_file() || file.metadata()?.len() == 0 {
            return Err(std::io::Error::other("empty or not a file"));
        }
        let mut byte = [0];
        file.read_exact(&mut byte)
    })();
    result.map_err(|_| format!("缺少可读的非空文件：{}", path.display()))
}
fn validate_runtime(directory: &Path, architecture: &str) -> Result<(), String> {
    if architecture != "aarch64" {
        return Err("本地模型目前仅支持 Apple Silicon arm64。".into());
    }
    let package = directory.join("package.json");
    readable(&package)?;
    let metadata: serde_json::Value =
        serde_json::from_reader(File::open(&package).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if metadata["version"] != RUNTIME_VERSION {
        return Err(format!("需要 Orca sherpa-onnx {RUNTIME_VERSION} 运行时。"));
    }
    for name in ["libsherpa-onnx-c-api.dylib", "libonnxruntime.1.23.2.dylib"] {
        let path = directory.join(name);
        readable(&path)?;
        let mut header = [0; 8];
        File::open(&path)
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
