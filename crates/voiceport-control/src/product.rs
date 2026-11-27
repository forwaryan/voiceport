//! Additive product requests for the running App's control socket.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Page {
    Result,
    Settings,
    FirstRun,
    Permissions,
    Sources,
    SourceTests,
    Models,
    Microphone,
    Shortcut,
    Dictionary,
    Scenes,
    QuickPanel,
    File,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingMode {
    Toggle,
    Hold,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum PreferencesRequest {
    Get,
    Language { language: String },
    AutomaticInput { enabled: bool },
    StatusOverlay { enabled: bool },
    RecordingMode { mode: RecordingMode },
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum MicrophoneRequest {
    List,
    Status,
    Select { uid: Option<String> },
    TestStart { confirm: bool },
    TestStop { operation: String },
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelMutation {
    Download { id: String },
    Update { id: String },
    Import { id: String, directory: PathBuf },
    Remove { id: String },
    InstallEngine,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelsRequest {
    Details { id: String },
    Plan { mutation: ModelMutation },
    Start { operation: String, confirm: bool },
    Status { operation: String },
    Cancel { operation: String },
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DictionaryRequest {
    Show,
    Set { rules: String, enabled: bool },
    Preview { text: String },
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScenesRequest {
    List,
    Add {
        name: String,
        source: String,
        language: String,
        dictionary_enabled: bool,
    },
    Remove {
        name: String,
        confirm: bool,
    },
    Apply {
        name: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HistoryDelivery {
    Confirmed,
    Failed,
    Uncertain,
    NotSent,
    Pending,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum HistoryRequest {
    List {
        limit: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        query: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        delivery: Option<HistoryDelivery>,
    },
    Show {
        id: String,
    },
    Clear {
        confirm: bool,
    },
    Settings {
        enabled: Option<bool>,
        confirm: bool,
    },
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileFormat {
    Text,
    Markdown,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum FileRequest {
    Sources {
        language: String,
    },
    Plan {
        path: PathBuf,
        source: String,
        language: String,
    },
    Start {
        operation: String,
        confirm: bool,
    },
    Status {
        operation: String,
    },
    Result {
        operation: String,
    },
    Cancel {
        operation: String,
    },
    Export {
        operation: String,
        path: PathBuf,
        format: FileFormat,
    },
}
