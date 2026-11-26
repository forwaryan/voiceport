use super::invalid;
use std::{
    io::{IsTerminal, Read},
    path::PathBuf,
};
use voiceport_control::{
    Action, DictionaryRequest, FileFormat, FileRequest, HistoryRequest, MicrophoneRequest,
    ModelMutation, ModelsRequest, Page, PreferencesRequest, Response, ScenesRequest,
};

pub(super) const GROUPS: &[&str] = &[
    "preferences",
    "microphone",
    "models",
    "dictionary",
    "scenes",
    "history",
    "file",
];

pub(super) fn parse(words: &[&str]) -> Result<Action, Response> {
    Ok(match words {
        ["app", "capabilities"] => Action::Capabilities,
        ["app", "open", page] => Action::Open {
            page: match *page {
                "result" => Page::Result,
                "settings" => Page::Settings,
                "first-run" => Page::FirstRun,
                "permissions" => Page::Permissions,
                "sources" => Page::Sources,
                "source-tests" => Page::SourceTests,
                "models" => Page::Models,
                "microphone" => Page::Microphone,
                "shortcut" => Page::Shortcut,
                "dictionary" => Page::Dictionary,
                "scenes" => Page::Scenes,
                "quick-panel" => Page::QuickPanel,
                "file" | "files" => Page::File,
                _ => return Err(invalid()),
            },
        },
        ["sources", "destination", id] if positional(id) => {
            Action::SourcesDestination { id: (*id).into() }
        }
        ["models", "list"] => Action::ModelsList,
        ["models", "refresh"] => Action::ModelsRefresh,
        ["models", "open"] => Action::Open { page: Page::Models },
        ["models", rest @ ..] => Action::Models {
            request: models(rest)?,
        },
        ["preferences", rest @ ..] => Action::Preferences {
            request: preferences(rest)?,
        },
        ["microphone", "open"] => Action::Open {
            page: Page::Microphone,
        },
        ["microphone", rest @ ..] => Action::Microphone {
            request: microphone(rest)?,
        },
        ["dictionary", rest @ ..] => Action::Dictionary {
            request: dictionary(rest)?,
        },
        ["scenes", rest @ ..] => Action::Scenes {
            request: scenes(rest)?,
        },
        ["dictation", "gui-result", generation, rest @ ..]
            if rest.is_empty() || *rest == ["--original"] =>
        {
            Action::GuiResult {
                generation: generation
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(invalid)?,
                original: !rest.is_empty(),
            }
        }
        ["history", rest @ ..] => Action::History {
            request: history(rest)?,
        },
        ["file", "open"] => Action::Open { page: Page::File },
        ["file", rest @ ..] => Action::File {
            request: file(rest)?,
        },
        _ => return Err(invalid()),
    })
}

fn positional(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('-')
}

fn boolean(value: &str) -> Result<bool, Response> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(invalid()),
    }
}

fn language(value: &str) -> Result<String, Response> {
    match value {
        "auto" => Ok(String::new()),
        code if !code.is_empty() && voiceport_core::language::supported(code) => Ok(code.into()),
        _ => Err(invalid()),
    }
}

fn absolute_path(value: &str) -> Result<PathBuf, Response> {
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(invalid());
    }
    Ok(path)
}

fn models(words: &[&str]) -> Result<ModelsRequest, Response> {
    let mutation = match words {
        ["download", id] if positional(id) => ModelMutation::Download { id: (*id).into() },
        ["update", id] if positional(id) => ModelMutation::Update { id: (*id).into() },
        ["import", id, directory] if positional(id) => ModelMutation::Import {
            id: (*id).into(),
            directory: absolute_path(directory)?,
        },
        ["remove", id] if positional(id) => ModelMutation::Remove { id: (*id).into() },
        ["install-engine"] => ModelMutation::InstallEngine,
        _ => {
            return match words {
                ["details", id] if positional(id) => {
                    Ok(ModelsRequest::Details { id: (*id).into() })
                }
                ["status", operation] if positional(operation) => Ok(ModelsRequest::Status {
                    operation: (*operation).into(),
                }),
                ["cancel", operation] if positional(operation) => Ok(ModelsRequest::Cancel {
                    operation: (*operation).into(),
                }),
                ["start", operation, rest @ ..]
                    if positional(operation) && (rest.is_empty() || *rest == ["--confirm"]) =>
                {
                    Ok(ModelsRequest::Start {
                        operation: (*operation).into(),
                        confirm: !rest.is_empty(),
                    })
                }
                _ => Err(invalid()),
            };
        }
    };
    Ok(ModelsRequest::Plan { mutation })
}

fn preferences(words: &[&str]) -> Result<PreferencesRequest, Response> {
    use PreferencesRequest::*;
    Ok(match words {
        ["get"] => Get,
        ["set", "language", code] => Language {
            language: language(code)?,
        },
        ["set", "automatic-input", enabled] => AutomaticInput {
            enabled: boolean(enabled)?,
        },
        ["set", "status-overlay", enabled] => StatusOverlay {
            enabled: boolean(enabled)?,
        },
        ["set", "recording-mode", "toggle"] => RecordingMode {
            mode: voiceport_control::RecordingMode::Toggle,
        },
        ["set", "recording-mode", "hold"] => RecordingMode {
            mode: voiceport_control::RecordingMode::Hold,
        },
        _ => return Err(invalid()),
    })
}

fn microphone(words: &[&str]) -> Result<MicrophoneRequest, Response> {
    use MicrophoneRequest::*;
    Ok(match words {
        ["list"] => List,
        ["status"] => Status,
        ["select", "--system"] => Select { uid: None },
        ["select", "--device", uid] if positional(uid) => Select {
            uid: Some((*uid).into()),
        },
        ["test-start", "--confirm"] => TestStart { confirm: true },
        ["test-start"] => {
            return Err(Response::error(
                "confirmation_required",
                "Use --confirm after the user authorizes a local microphone level test.",
            ));
        }
        ["test-stop", operation] if positional(operation) => TestStop {
            operation: (*operation).into(),
        },
        _ => return Err(invalid()),
    })
}

fn dictionary(words: &[&str]) -> Result<DictionaryRequest, Response> {
    use DictionaryRequest::*;
    Ok(match words {
        ["show"] => Show,
        ["set", enabled, "--stdin"] => Set {
            enabled: boolean(enabled)?,
            rules: stdin_text()?,
        },
        ["preview", "--stdin"] => Preview {
            text: stdin_text()?,
        },
        _ => return Err(invalid()),
    })
}

fn stdin_text() -> Result<String, Response> {
    const LIMIT: u64 = 24_000;
    let stdin = std::io::stdin();
    let failure = || {
        Response::error(
            "invalid_arguments",
            "Supply UTF-8 text of at most 24000 bytes through a pipe or file on stdin.",
        )
    };
    if stdin.is_terminal() {
        return Err(failure());
    }
    let mut result = String::new();
    stdin
        .take(LIMIT + 1)
        .read_to_string(&mut result)
        .map_err(|_| failure())?;
    if result.len() as u64 > LIMIT {
        return Err(failure());
    }
    Ok(result)
}

fn scenes(words: &[&str]) -> Result<ScenesRequest, Response> {
    use ScenesRequest::*;
    Ok(match words {
        ["list"] => List,
        [
            "add",
            name,
            "--source",
            source,
            "--language",
            code,
            "--dictionary",
            enabled,
        ] if positional(name) && positional(source) => Add {
            name: (*name).into(),
            source: (*source).into(),
            language: language(code)?,
            dictionary_enabled: boolean(enabled)?,
        },
        ["remove", name, rest @ ..]
            if positional(name) && (rest.is_empty() || *rest == ["--confirm"]) =>
        {
            Remove {
                name: (*name).into(),
                confirm: !rest.is_empty(),
            }
        }
        ["apply", name] if positional(name) => Apply {
            name: (*name).into(),
        },
        _ => return Err(invalid()),
    })
}

fn history(words: &[&str]) -> Result<HistoryRequest, Response> {
    use HistoryRequest::*;
    Ok(match words {
        ["list"] => List { limit: 20 },
        ["list", "--limit", limit] => List {
            limit: limit
                .parse()
                .ok()
                .filter(|n| (1..=100).contains(n))
                .ok_or_else(invalid)?,
        },
        ["show", id] if positional(id) => Show { id: (*id).into() },
        ["clear"] => Clear { confirm: false },
        ["clear", "--confirm"] => Clear { confirm: true },
        ["settings"] => Settings {
            enabled: None,
            confirm: false,
        },
        ["settings", enabled] => Settings {
            enabled: Some(boolean(enabled)?),
            confirm: false,
        },
        ["settings", "off", "--confirm"] => Settings {
            enabled: Some(false),
            confirm: true,
        },
        _ => return Err(invalid()),
    })
}

fn file(words: &[&str]) -> Result<FileRequest, Response> {
    use FileRequest::*;
    Ok(match words {
        ["sources"] => Sources {
            language: String::new(),
        },
        ["sources", "--language", code] => Sources {
            language: language(code)?,
        },
        ["plan", path, "--source", source, rest @ ..] if positional(source) => Plan {
            path: absolute_path(path)?,
            source: (*source).into(),
            language: match rest {
                [] => String::new(),
                ["--language", code] => language(code)?,
                _ => return Err(invalid()),
            },
        },
        ["start", operation, rest @ ..]
            if positional(operation) && (rest.is_empty() || *rest == ["--confirm"]) =>
        {
            Start {
                operation: (*operation).into(),
                confirm: !rest.is_empty(),
            }
        }
        ["status", operation] if positional(operation) => Status {
            operation: (*operation).into(),
        },
        ["result", operation] if positional(operation) => Result {
            operation: (*operation).into(),
        },
        ["cancel", operation] if positional(operation) => Cancel {
            operation: (*operation).into(),
        },
        ["export", operation, "--output", path, rest @ ..] if positional(operation) => Export {
            operation: (*operation).into(),
            path: absolute_path(path)?,
            format: match rest {
                [] | ["--format", "text"] => FileFormat::Text,
                ["--format", "markdown"] => FileFormat::Markdown,
                _ => return Err(invalid()),
            },
        },
        _ => return Err(invalid()),
    })
}
