//! Versioned plugin descriptions and source instances. Loading is local and never executes code.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub const VERSION: u32 = 1;
pub const HTTP: &str = "voiceport.http-transcription";
pub const CHATGPT_PLUGIN: &str = "voiceport.chatgpt";
pub const PARA_PLUGIN: &str = "voiceport.orca-paraformer";
pub const WHISPER_PLUGIN: &str = "voiceport.orca-whisper";
pub const SENSEVOICE_PLUGIN: &str = "voiceport.sensevoice";
pub const MAX_CONFIG: u64 = 262_144;
pub const MAX_MANIFEST: u64 = 32_768;
type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    String,
    Secret,
    Url,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
    pub required: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Adapter {
    Chatgpt,
    Paraformer,
    Whisper,
    SenseVoice,
    Http,
    Jsonl,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub batch: bool,
    pub streaming: bool,
    pub system_preview: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    pub executable: PathBuf,
    pub args: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema_version: u32,
    pub protocol_version: u32,
    pub id: String,
    pub version: String,
    pub name: String,
    pub origin: String,
    pub adapter: Adapter,
    pub capabilities: Capabilities,
    pub fields: Vec<Field>,
    pub program: Option<Program>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub id: String,
    pub name: String,
    pub plugin_id: String,
    pub plugin_version: String,
    pub enabled: bool,
    pub config: BTreeMap<String, String>,
    pub credential_refs: BTreeMap<String, String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Store {
    pub schema_version: u32,
    pub plugins: Vec<Manifest>,
    pub sources: Vec<Source>,
}
impl Default for Store {
    fn default() -> Self {
        Self {
            schema_version: VERSION,
            plugins: vec![],
            sources: vec![],
        }
    }
}
fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 120
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}
fn text(s: &str, max: usize) -> bool {
    !s.trim().is_empty() && s.len() <= max && !s.chars().any(char::is_control)
}
impl Manifest {
    pub fn validate_external(&self) -> Result<()> {
        if self.schema_version != VERSION || self.protocol_version != VERSION {
            return Err("插件清单或 JSONL 协议版本不受支持。".into());
        }
        if !identifier(&self.id)
            || self.id.starts_with("voiceport.")
            || !text(&self.version, 64)
            || !text(&self.name, 128)
            || !text(&self.origin, 1024)
        {
            return Err("插件身份、版本、名称或来源无效。".into());
        }
        if self.adapter != Adapter::Jsonl
            || self.capabilities
                != (Capabilities {
                    batch: true,
                    streaming: false,
                    system_preview: false,
                })
        {
            return Err(
                "外部插件第一版仅支持 JSONL v1 停止后转写，不支持流式或系统语音预览。".into(),
            );
        }
        let program = self
            .program
            .as_ref()
            .ok_or("插件必须声明本地程序及参数数组。")?;
        if !program.executable.is_absolute()
            || program.args.len() > 32
            || program
                .args
                .iter()
                .any(|s| s.len() > 4096 || s.contains('\0'))
        {
            return Err("插件程序必须为绝对本地路径，参数数组超限或无效。".into());
        }
        if self.fields.len() > 12 {
            return Err("插件字段数量超过 12。".into());
        }
        let mut ids = BTreeSet::new();
        for field in &self.fields {
            if !identifier(&field.id) || !text(&field.label, 128) || !ids.insert(&field.id) {
                return Err("插件配置字段无效或重复。".into());
            }
        }
        Ok(())
    }
    pub fn validate_program(&self) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let program = self.program.as_ref().ok_or("插件没有本地程序。")?;
        let metadata =
            fs::metadata(&program.executable).map_err(|_| "插件本地程序不存在或不可读取。")?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o111 == 0 {
            return Err("插件本地程序不是可执行文件。".into());
        }
        Ok(())
    }
}
pub fn builtins() -> Vec<Manifest> {
    [
        (CHATGPT_PLUGIN, "ChatGPT", Adapter::Chatgpt, true, false),
        (
            PARA_PLUGIN,
            "Paraformer 本地流式",
            Adapter::Paraformer,
            false,
            true,
        ),
        (
            WHISPER_PLUGIN,
            "Whisper Tiny 本地离线",
            Adapter::Whisper,
            false,
            false,
        ),
        (
            SENSEVOICE_PLUGIN,
            "SenseVoice 本地离线",
            Adapter::SenseVoice,
            false,
            false,
        ),
        (HTTP, "兼容音频转写 HTTP", Adapter::Http, false, false),
    ]
    .into_iter()
    .map(|(id, name, adapter, system_preview, streaming)| Manifest {
        schema_version: VERSION,
        protocol_version: VERSION,
        id: id.into(),
        version: "1".into(),
        name: name.into(),
        origin: "VoicePort 内置".into(),
        adapter,
        capabilities: Capabilities {
            batch: true,
            streaming,
            system_preview,
        },
        fields: if adapter == Adapter::Http {
            vec![
                Field {
                    id: "endpoint".into(),
                    label: "完整转写地址".into(),
                    kind: FieldKind::Url,
                    required: true,
                },
                Field {
                    id: "model".into(),
                    label: "模型标识".into(),
                    kind: FieldKind::String,
                    required: true,
                },
                Field {
                    id: "api_key".into(),
                    label: "Bearer API key（可选）".into(),
                    kind: FieldKind::Secret,
                    required: false,
                },
            ]
        } else {
            vec![]
        },
        program: None,
    })
    .collect()
}
impl Store {
    pub fn descriptors(&self) -> Vec<Manifest> {
        let mut p = builtins();
        p.extend(self.plugins.clone());
        p
    }
    pub fn plugin(&self, id: &str) -> Result<Manifest> {
        self.descriptors()
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| "来源引用的插件未安装。".into())
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != VERSION || self.plugins.len() > 64 || self.sources.len() > 128 {
            return Err("来源配置版本不受支持或数量超限。".into());
        }
        let mut plugins = BTreeSet::new();
        for p in &self.plugins {
            p.validate_external()?;
            if !plugins.insert(&p.id) {
                return Err("插件 ID 重复，拒绝加载。".into());
            }
        }
        let mut ids = BTreeSet::new();
        let mut credential_refs = BTreeSet::new();
        for s in &self.sources {
            if !identifier(&s.id)
                || s.id == crate::local_models::CHATGPT
                || !text(&s.name, 128)
                || !ids.insert(&s.id)
                || !identifier(&s.plugin_id)
                || !text(&s.plugin_version, 64)
            {
                return Err("来源身份、名称、版本无效或 ID 重复。".into());
            }
            if s.config.len() > 12
                || s.credential_refs.len() > 12
                || s.config
                    .iter()
                    .any(|(k, v)| !identifier(k) || v.len() > 4096 || v.contains('\0'))
                || s.credential_refs
                    .iter()
                    .any(|(k, v)| !identifier(k) || !identifier(v))
            {
                return Err("来源配置字段超限或无效。".into());
            }
            if s.credential_refs
                .values()
                .any(|reference| !credential_refs.insert(reference))
            {
                return Err("每个来源和密钥字段必须使用独立 Keychain 引用。".into());
            }
            // Never accept plaintext in a declared secret field, even for an unavailable source.
            if let Ok(p) = self.plugin(&s.plugin_id) {
                for f in &p.fields {
                    if f.kind == FieldKind::Secret && s.config.contains_key(&f.id) {
                        return Err("密钥只能以 Keychain 引用保存。".into());
                    }
                }
            }
        }
        Ok(())
    }
    pub fn validate_source(&self, s: &Source) -> Result<Manifest> {
        let p = self.plugin(&s.plugin_id)?;
        if p.version != s.plugin_version {
            return Err("来源绑定的插件版本不匹配，请编辑并确认。".into());
        }
        if !s.enabled {
            return Err("来源尚未显式启用。".into());
        }
        if !matches!(p.adapter, Adapter::Http | Adapter::Jsonl) {
            return Err("此内置插件使用保留来源，不能添加配置实例。".into());
        }
        for k in s.config.keys().chain(s.credential_refs.keys()) {
            if !p.fields.iter().any(|f| &f.id == k) {
                return Err("来源包含未声明的配置字段。".into());
            }
        }
        for f in &p.fields {
            let value = if f.kind == FieldKind::Secret {
                s.credential_refs.get(&f.id)
            } else {
                s.config.get(&f.id)
            };
            if f.required && value.is_none_or(|v| v.trim().is_empty()) {
                return Err(format!("缺少配置字段：{}", f.label));
            }
            if f.kind == FieldKind::Secret && s.config.contains_key(&f.id)
                || f.kind != FieldKind::Secret && s.credential_refs.contains_key(&f.id)
            {
                return Err("配置字段类型不匹配。".into());
            }
            if f.kind == FieldKind::Url
                && let Some(value) = value
            {
                validate_endpoint(value)?;
            }
        }
        if p.adapter == Adapter::Jsonl {
            p.validate_program()?;
        }
        Ok(p)
    }
    pub fn load(path: &Path) -> Result<Self> {
        match fs::symlink_metadata(path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(_) => return Err("无法检查来源配置文件，未使用默认配置覆盖。".into()),
        }
        let bytes = read_bounded(path, MAX_CONFIG)?;
        let store: Self =
            serde_json::from_slice(&bytes).map_err(|_| "来源配置 JSON 损坏或字段类型不支持。")?;
        store.validate()?;
        Ok(store)
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|_| "无法序列化来源配置。")?;
        if bytes.len() as u64 > MAX_CONFIG {
            return Err("来源配置过大。".into());
        }
        let parent = path.parent().ok_or("来源配置路径无效。")?;
        fs::create_dir_all(parent).map_err(|_| "无法创建来源配置目录。")?;
        let mut file =
            tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法创建来源配置临时文件。")?;
        file.as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| "无法保护来源配置。")?;
        file.write_all(&bytes)
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| "无法写入来源配置。")?;
        file.persist(path).map_err(|_| "无法原子保存来源配置。")?;
        fs::File::open(parent)
            .and_then(|f| f.sync_all())
            .map_err(|_| "无法同步来源配置目录。")?;
        Ok(())
    }
    /// A rename may have committed even if directory fsync failed. Never remove
    /// a credential that might now be referenced by the durable configuration.
    pub fn reference_is_confirmed_unused(path: &Path, reference: &str) -> bool {
        Self::load(path).is_ok_and(|store| {
            !store
                .sources
                .iter()
                .any(|source| source.credential_refs.values().any(|r| r == reference))
        })
    }
    pub fn import(&mut self, p: Manifest) -> Result<()> {
        p.validate_external()?;
        p.validate_program()?;
        if self.descriptors().iter().any(|old| old.id == p.id) {
            return Err("插件 ID 已存在，拒绝覆盖。".into());
        }
        self.plugins.push(p);
        Ok(())
    }
}
pub fn read_manifest(path: &Path) -> Result<Manifest> {
    let p: Manifest = serde_json::from_slice(&read_bounded(path, MAX_MANIFEST)?)
        .map_err(|_| "插件 JSON 无效、字段类型不支持或含未知字段。")?;
    p.validate_external()?;
    p.validate_program()?;
    Ok(p)
}
fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "无法读取本地普通配置文件（不接受符号链接）。")?;
    let metadata = file.metadata().map_err(|_| "无法检查配置文件。")?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err("配置必须为有界本地普通文件。".into());
    }
    let mut bytes = vec![];
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取配置文件。")?;
    if bytes.len() as u64 > limit {
        return Err("配置文件过大。".into());
    }
    Ok(bytes)
}
pub fn default_path() -> Result<PathBuf> {
    sources_path(std::env::var_os("VOICEPORT_SOURCES_PATH"), || {
        std::env::var_os("HOME")
    })
}
fn sources_path(
    override_path: Option<std::ffi::OsString>,
    home: impl FnOnce() -> Option<std::ffi::OsString>,
) -> Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    if let Some(value) = override_path {
        let path = PathBuf::from(&value);
        if !path.is_absolute()
            || path.file_name().is_none()
            || value.as_bytes().contains(&0)
            || value.as_bytes().ends_with(b"/")
        {
            return Err("VOICEPORT_SOURCES_PATH 必须为非空的绝对配置文件路径。".into());
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) if !metadata.is_file() => {
                return Err("VOICEPORT_SOURCES_PATH 不接受符号链接或非普通文件。".into());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("无法检查 VOICEPORT_SOURCES_PATH；未回落默认配置。".into()),
        }
        return Ok(path);
    }
    let home = home()
        .filter(|s| !s.is_empty())
        .ok_or("无法定位来源配置目录。")?;
    Ok(PathBuf::from(home)
        .join("Library/Application Support/VoicePort/transcription-sources-v1.json"))
}
pub fn validate_endpoint(endpoint: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| "转写地址无效。")?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || !(url.scheme() == "https" || url.scheme() == "http" && loopback)
    {
        return Err(
            "远程服务须为 HTTPS，HTTP 仅限明确 loopback；地址不能含用户名、密码或 fragment。"
                .into(),
        );
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(id: &str) -> Source {
        Source {
            id: id.into(),
            name: id.into(),
            plugin_id: HTTP.into(),
            plugin_version: "1".into(),
            enabled: true,
            config: BTreeMap::from([
                ("endpoint".into(), "http://127.0.0.1:8/transcribe".into()),
                ("model".into(), "local".into()),
            ]),
            credential_refs: BTreeMap::new(),
        }
    }
    #[test]
    fn sources_path_override_is_prioritized_and_invalid_values_fail_closed() {
        use std::ffi::OsString;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private/sources.json");
        let no_home = || panic!("override must not consult HOME");
        assert_eq!(
            sources_path(Some(path.clone().into_os_string()), no_home).unwrap(),
            path
        );
        let store = Store {
            sources: vec![source("stable-id")],
            ..Store::default()
        };
        store.save(&path).unwrap();
        assert_eq!(Store::load(&path).unwrap(), store);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        for invalid in [
            OsString::from(""),
            "relative.json".into(),
            "/".into(),
            "/tmp/".into(),
            "/tmp/a\0.json".into(),
        ] {
            assert!(sources_path(Some(invalid), no_home).is_err());
        }
        assert!(sources_path(Some(dir.path().into()), no_home).is_err());
        let link = dir.path().join("link.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(sources_path(Some(link.into_os_string()), no_home).is_err());
        let fifo = dir.path().join("fifo");
        use std::os::unix::ffi::OsStrExt;
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(sources_path(Some(fifo.into_os_string()), no_home).is_err());
        let not_directory = path.join("child.json");
        assert!(sources_path(Some(not_directory.into_os_string()), no_home).is_err());
        fs::write(&path, br#"{"schema_version":2,"plugins":[],"sources":[]}"#).unwrap();
        let resolved = sources_path(Some(path.into_os_string()), no_home).unwrap();
        assert!(Store::load(&resolved).is_err()); // No alternate/default Store on invalid data.
        let fake_home = dir.path().join("synthetic-home");
        assert_eq!(
            sources_path(None, || Some(fake_home.clone().into_os_string())).unwrap(),
            fake_home.join("Library/Application Support/VoicePort/transcription-sources-v1.json")
        );
        assert!(sources_path(None, || None).is_err());
    }
    #[test]
    fn sources_path_environment_wiring_uses_only_synthetic_files() {
        const CHILD: &str = "VOICEPORT_PATH_TEST_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let expected = std::env::var_os("VOICEPORT_PATH_TEST_EXPECTED").unwrap();
            match default_path() {
                Ok(path) => assert_eq!(path.as_os_str(), expected),
                Err(_) => assert_eq!(expected, "ERROR"),
            }
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let fake_home = dir.path().join("synthetic-home");
        let path = dir.path().join("override.json");
        for value in [
            None,
            Some(path.as_os_str()),
            Some(std::ffi::OsStr::new("")),
            Some(std::ffi::OsStr::new("relative")),
        ] {
            let expected = match value {
                None => fake_home
                    .join("Library/Application Support/VoicePort/transcription-sources-v1.json")
                    .into_os_string(),
                Some(v) if v == path.as_os_str() => path.clone().into_os_string(),
                Some(_) => "ERROR".into(),
            };
            let mut command = std::process::Command::new(std::env::current_exe().unwrap());
            command
                .env_clear()
                .env("HOME", &fake_home)
                .env(CHILD, "1")
                .env("VOICEPORT_PATH_TEST_EXPECTED", expected)
                .args([
                    "--exact",
                    "plugins::tests::sources_path_environment_wiring_uses_only_synthetic_files",
                    "--nocapture",
                ])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null());
            if let Some(v) = value {
                command.env("VOICEPORT_SOURCES_PATH", v);
            }
            let mut child = command.spawn().unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            let status = loop {
                if let Some(status) = child.try_wait().unwrap() {
                    break status;
                }
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("path wiring child exceeded deadline");
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            };
            assert!(status.success(), "path wiring child failed for {value:?}");
        }
    }
    #[test]
    fn endpoints_are_explicit_and_safe() {
        for url in [
            "http://localhost:9/x",
            "http://127.0.0.2/x",
            "http://[::1]/x",
            "https://example.com/full?route=x",
        ] {
            assert!(validate_endpoint(url).is_ok(), "{url}");
        }
        for url in [
            "http://example.com/x",
            "http://192.168.0.1/x",
            "https://u:p@example.com",
            "https://example.com/#x",
            "file:///tmp/x",
            "http://localhost.evil/x",
        ] {
            assert!(validate_endpoint(url).is_err(), "{url}");
        }
    }
    #[test]
    fn instances_roundtrip_and_missing_plugin_stays_visible() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sources.json");
        let mut store = Store {
            sources: vec![source("one"), source("two")],
            ..Store::default()
        };
        store.save(&path).unwrap();
        assert_eq!(Store::load(&path).unwrap(), store);
        store.sources[0].plugin_id = "missing.plugin".into();
        store.save(&path).unwrap();
        let loaded = Store::load(&path).unwrap();
        assert_eq!(loaded.sources.len(), 2);
        assert!(loaded.validate_source(&loaded.sources[0]).is_err());
        assert!(loaded.validate_source(&loaded.sources[1]).is_ok());
        store.sources[1].id = "one".into();
        assert!(store.save(&path).is_err());
        assert_eq!(Store::load(&path).unwrap(), loaded);
    }
    #[test]
    fn reload_preserves_selected_instance_and_deleted_or_invalid_choice_never_falls_back() {
        use crate::local_models::{CHATGPT, Catalog, PARAFORMER, SessionSource, WHISPER};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sources.json");
        let mut store = Store {
            sources: vec![source("one"), source("two")],
            ..Store::default()
        };
        store.save(&path).unwrap();
        let selected = "two"; // Models the saved preference, without reading user defaults.
        let catalog = |store: &Store, saved: &str| {
            let mut c = Catalog::discover(
                &dir.path().join("absent-models"),
                &dir.path().join("absent-runtime"),
                saved,
                "aarch64",
            );
            c.add_configured(store);
            c
        };
        let restarted = Store::load(&path).unwrap();
        let c = catalog(&restarted, selected);
        let SessionSource::Plugin(p) = c.snapshot(selected).unwrap() else {
            panic!("wrong adapter")
        };
        assert_eq!(p.source.id, selected);
        assert_eq!(p.source, store.sources[1]);
        assert!(c.snapshot("one").is_ok());
        store.sources[1].plugin_version = "unsupported".into();
        store.save(&path).unwrap();
        assert!(
            catalog(&Store::load(&path).unwrap(), selected)
                .snapshot(selected)
                .is_err()
        );
        store.sources.pop();
        store.save(&path).unwrap();
        let c = catalog(&Store::load(&path).unwrap(), selected);
        assert!(c.entries.iter().any(|e| e.id == selected));
        assert!(c.snapshot(selected).is_err());
        assert!(matches!(c.snapshot(CHATGPT), Ok(SessionSource::ChatGpt)));
        for saved in [PARAFORMER, WHISPER] {
            assert!(catalog(&store, saved).snapshot(saved).is_err());
        }
        std::fs::write(&path, b"broken json").unwrap();
        assert!(Store::load(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"broken json");
    }
    #[test]
    fn credentials_are_independent_and_ambiguous_commit_preserves_referenced_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sources.json");
        let mut one = source("one");
        one.credential_refs
            .insert("api_key".into(), "key-synthetic-one".into());
        let mut two = source("two");
        two.credential_refs = one.credential_refs.clone();
        let mut store = Store {
            sources: vec![one, two],
            ..Store::default()
        };
        assert!(store.validate().is_err());
        store.sources[1]
            .credential_refs
            .insert("api_key".into(), "key-synthetic-two".into());
        store.save(&path).unwrap();
        assert!(!Store::reference_is_confirmed_unused(
            &path,
            "key-synthetic-one"
        ));
        assert!(Store::reference_is_confirmed_unused(
            &path,
            "key-synthetic-uncommitted"
        ));
        std::fs::write(&path, b"damaged").unwrap();
        assert!(!Store::reference_is_confirmed_unused(
            &path,
            "key-synthetic-one"
        ));
    }
    #[test]
    fn configuration_load_rejects_symlinks_and_pipes_without_blocking() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sources.json");
        Store::default().save(&path).unwrap();
        let link = dir.path().join("linked.json");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Store::load(&link).is_err());
        let fifo = dir.path().join("fifo");
        let name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let now = std::time::Instant::now();
        assert!(Store::load(&fifo).is_err());
        assert!(now.elapsed() < std::time::Duration::from_secs(1));
    }
    #[test]
    fn version_and_plaintext_secret_are_rejected() {
        let mut store = Store {
            schema_version: 2,
            ..Store::default()
        };
        assert!(store.validate().is_err());
        store.schema_version = 1;
        let mut s = source("one");
        s.config.insert("api_key".into(), "synthetic-secret".into());
        store.sources.push(s);
        assert!(store.validate().is_err());
    }
}
