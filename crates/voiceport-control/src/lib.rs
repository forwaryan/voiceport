//! Versioned local control protocol. No audio, credentials or transcripts in diagnostics.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

mod dictation;
pub use dictation::{
    CaptureSignal, CaptureSummary, DeliveryReceipt, DeliveryState, DictationBlockReason,
    DictationTimings, GuiDictationDiagnostics, GuiDictationReceipt, TranscriptionState,
};
mod product;
pub use product::{
    DictionaryRequest, FileFormat, FileRequest, HistoryDelivery, HistoryRequest, MicrophoneRequest,
    ModelMutation, ModelPreview, ModelsRequest, OutputRule, Page, PreferencesRequest,
    RecordingMode, ScenesRequest,
};
pub mod readiness;

pub const VERSION: u32 = 1;
pub const MAX_FRAME: usize = 262_144;
pub const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub action: Action,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
    Status,
    Show,
    ShowSettings,
    Quit,
    SourcesList,
    SourcesShow {
        id: String,
    },
    SourcesSave {
        id: String,
        update: bool,
        plugin: Option<String>,
        name: Option<String>,
        enabled: Option<bool>,
        config: BTreeMap<String, String>,
        secrets: BTreeMap<String, String>,
    },
    SourcesRemove {
        id: String,
    },
    SourcesSelect {
        id: String,
    },
    SourcesSecret {
        id: String,
        field: String,
        secret: Option<String>,
    },
    PluginsList,
    PluginsShow {
        id: String,
    },
    PluginsImport {
        manifest: Value,
        accept_program: bool,
    },
    Test {
        source: String,
        accept_upload: bool,
    },
    SourcesProbe {
        source: Option<String>,
        language: String,
        accept_upload: bool,
    },
    SourcesProbeResult {
        operation: String,
    },
    SourcesProbeCancel {
        operation: String,
    },
    DictationStart {
        source: String,
        language: String,
    },
    DictationStop {
        operation: String,
    },
    DictationRetry {
        operation: String,
    },
    Cancel {
        operation: String,
    },
    Result {
        operation: String,
    },
    Capabilities,
    UpdateCheck,
    UpdateStatus,
    Open {
        page: Page,
    },
    Preferences {
        request: PreferencesRequest,
    },
    Microphone {
        request: MicrophoneRequest,
    },
    SourcesDestination {
        id: String,
    },
    ModelsList,
    ModelsRefresh,
    Models {
        request: ModelsRequest,
    },
    Dictionary {
        request: DictionaryRequest,
    },
    Scenes {
        request: ScenesRequest,
    },
    GuiResult {
        generation: u64,
        original: bool,
    },
    CopyLastDictation,
    PasteLastDictation {
        target_pid: i32,
        target_bundle: String,
    },
    History {
        request: HistoryRequest,
    },
    File {
        request: FileRequest,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Failure {
    pub code: String,
    pub message: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Response {
    pub schema_version: u32,
    pub status: String,
    pub data: Value,
    pub error: Option<Failure>,
}
impl Response {
    pub fn ok(data: impl Into<Value>) -> Self {
        Self {
            schema_version: VERSION,
            status: "ok".into(),
            data: data.into(),
            error: None,
        }
    }
    pub fn error(code: &str, message: &str) -> Self {
        Self {
            schema_version: VERSION,
            status: "error".into(),
            data: Value::Null,
            error: Some(Failure {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
    pub fn exit_code(&self) -> u8 {
        match self.error.as_ref().map(|e| e.code.as_str()) {
            None => 0,
            Some("invalid_arguments" | "invalid_request" | "invalid_config") => 2,
            Some("app_not_running") => 3,
            Some("busy") => 4,
            Some("confirmation_required") => 5,
            Some("not_found") => 6,
            Some("unsupported") => 7,
            Some("cancelled") => 130,
            _ => 1,
        }
    }
}
#[cfg(unix)]
pub mod transport;
