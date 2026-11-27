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
    AppRules,
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPreview {
    #[default]
    Any,
    Live,
    AfterStop,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ModelsRequest {
    Details {
        id: String,
    },
    Recommend {
        language: Option<String>,
        #[serde(default)]
        preview: ModelPreview,
    },
    Plan {
        mutation: ModelMutation,
    },
    Start {
        operation: String,
        confirm: bool,
    },
    Status {
        operation: String,
    },
    Cancel {
        operation: String,
    },
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DictionaryImportMode {
    Merge,
    Replace,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DictionaryRequest {
    Show,
    ShowPage {
        offset: usize,
        revision: Option<String>,
    },
    Set {
        rules: String,
        enabled: bool,
    },
    Preview {
        text: String,
    },
    Import {
        path: PathBuf,
        mode: DictionaryImportMode,
    },
    ImportPreview {
        operation: String,
        offset: usize,
    },
    ImportApply {
        operation: String,
        confirm: bool,
    },
    ImportCancel {
        operation: String,
    },
    Export {
        path: PathBuf,
    },
    OutputShow,
    OutputSet {
        rule: OutputRule,
        enabled: bool,
    },
    OutputPreview {
        text: String,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputRule {
    TrimEdges,
    NormalizeNewlines,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum AppRulesRequest {
    List,
    Enable { enabled: bool },
    Set { bundle_id: String, scene: String },
    Remove { bundle_id: String },
    Match { bundle_id: Option<String> },
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
