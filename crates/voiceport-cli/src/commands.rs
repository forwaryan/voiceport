//! Local automation commands. Parse completely before connecting; never retry mutations.
use serde_json::json;
use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};
use voiceport_control::{Action, Response};
const GUIDES: &[(&str, &str)] = &[
    ("core", include_str!("../../../skills/voiceport/SKILL.md")),
    (
        "cli",
        include_str!("../../../skills/voiceport/references/cli.md"),
    ),
    (
        "workflows",
        include_str!("../../../skills/voiceport/references/workflows.md"),
    ),
    (
        "plugins",
        include_str!("../../../skills/voiceport-plugins/SKILL.md"),
    ),
    (
        "plugin-examples",
        include_str!("../../../skills/voiceport-plugins/references/examples.md"),
    ),
    (
        "plugin-protocol",
        include_str!("../../../docs/TRANSCRIPTION_PLUGINS.md"),
    ),
    (
        "plugin-template",
        include_str!("../../../skills/voiceport-plugins/templates/json_http_adapter.py"),
    ),
];
const HELP: &str = "voiceport [--json] [--socket ABSOLUTE_PATH] COMMAND\n\nstatus | app status/show/quit\nsources list/show/add/update/remove/select/secret/test\nplugins list/show/import\ndictation start/stop/cancel/result\nqq doctor [--qq ABSOLUTE_APP_PATH] (offline installation check)\nskills list | skills get [GUIDE]\nGuides: core, cli, workflows, plugins, plugin-examples, plugin-protocol, plugin-template\n--version\n\nUse voiceport skills get cli for complete arguments.\nApp control requires the running macOS App at the chosen socket.\nNo command implicitly launches, records, uploads audio or delivers text.\nOnly dictation start records; sources test requires --accept-upload.\nLegacy help/host/demo/doctor/transcribe are also available.";
fn invalid() -> Response {
    Response::error(
        "invalid_arguments",
        "Invalid arguments. Run voiceport --help or voiceport skills get cli.",
    )
}
pub fn print_result(response: Response, json_output: bool) -> u8 {
    let exit = response.exit_code();
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&response).expect("response serializes")
        );
    } else if let Some(error) = &response.error {
        eprintln!("VoicePort: {}: {}", error.code, error.message);
        if !response.data.is_null() {
            println!("{}", serde_json::to_string_pretty(&response.data).unwrap());
        }
    } else if let Some(text) = response.data.get("text").and_then(|v| v.as_str()) {
        println!("{text}");
    } else {
        println!("{}", serde_json::to_string_pretty(&response.data).unwrap());
    }
    exit
}
/// None delegates the byte-preserving original command line to the portable CLI.
pub fn run(raw: &[OsString]) -> Option<u8> {
    let managed = raw
        .iter()
        .any(|a| ["--json", "--socket", "--version"].iter().any(|f| a == f))
        || raw.first().is_some_and(|a| {
            [
                "status",
                "app",
                "sources",
                "plugins",
                "dictation",
                "skills",
                "qq",
            ]
            .iter()
            .any(|v| a == v)
        });
    if !managed {
        return None;
    }
    let json_output = raw.iter().any(|a| a == "--json");
    let args: Result<Vec<_>, _> = raw
        .iter()
        .map(|a| a.to_str().map(str::to_owned).ok_or(()))
        .collect();
    let response = match args {
        Ok(args) => execute(args),
        Err(_) => Err(invalid()),
    }
    .unwrap_or_else(|e| e);
    Some(print_result(response, json_output))
}
fn execute(mut args: Vec<String>) -> Result<Response, Response> {
    let mut socket = None;
    let mut seen_json = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--json" if !seen_json => {
                seen_json = true;
                args.remove(i);
            }
            "--socket" if socket.is_none() => {
                if i + 1 >= args.len() {
                    return Err(invalid());
                }
                socket = Some(PathBuf::from(args.remove(i + 1)));
                args.remove(i);
            }
            _ => i += 1,
        }
    }
    if args.as_slice() == ["--version"] {
        return Ok(Response::ok(
            json!({"version":env!("CARGO_PKG_VERSION"),"control_schema":1}),
        ));
    }
    if args.is_empty()
        || matches!(args.as_slice(), [s] if matches!(s.as_str(), "help" | "--help" | "-h"))
    {
        return Ok(Response::ok(json!({"text":HELP})));
    }
    if args[0] == "skills" {
        return match args.get(1).map(String::as_str) {
            Some("list") if args.len() == 2 => Ok(Response::ok(
                json!({"version":env!("CARGO_PKG_VERSION"),"skills":GUIDES.iter().map(|(name, _)| *name).collect::<Vec<_>>()}),
            )),
            Some("get") if (2..=3).contains(&args.len()) => {
                let guide = args.get(2).map_or("core", String::as_str);
                let text = GUIDES
                    .iter()
                    .find_map(|(name, text)| (*name == guide).then_some(*text))
                    .ok_or_else(invalid)?;
                Ok(Response::ok(
                    json!({"version":env!("CARGO_PKG_VERSION"),"guide":guide,"text":text}),
                ))
            }
            _ => Err(invalid()),
        };
    }
    if args[0] == "qq" {
        if socket.is_some() {
            return Err(invalid());
        }
        let words: Vec<_> = args.iter().map(String::as_str).collect();
        return match words.as_slice() {
            ["qq", "doctor"] => crate::qq::doctor(None),
            ["qq", "doctor", "--qq", path] => crate::qq::doctor(Some(path)),
            _ => Err(invalid()),
        };
    }
    let action = parse(&args)?;
    #[cfg(target_os = "macos")]
    {
        let path = socket.map_or_else(voiceport_control::transport::path_from_env, Ok)?;
        voiceport_control::transport::call(&path, action)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (socket, action);
        Err(Response::error(
            "unsupported",
            "App control is supported only on macOS. Portable host/demo/doctor/transcribe remain available.",
        ))
    }
}
pub fn help() -> &'static str {
    HELP
}
fn parse(args: &[String]) -> Result<Action, Response> {
    let words: Vec<_> = args.iter().map(String::as_str).collect();
    let action = match words.as_slice() {
        ["status"] | ["app", "status"] => Action::Status,
        ["app", "show"] => Action::Show,
        ["app", "show", "--settings"] => Action::ShowSettings,
        ["app", "quit"] => Action::Quit,
        ["sources", "list"] => Action::SourcesList,
        ["sources", "show", id] => Action::SourcesShow { id: (*id).into() },
        ["sources", "remove", id] => Action::SourcesRemove { id: (*id).into() },
        ["sources", "select", id] => Action::SourcesSelect { id: (*id).into() },
        ["sources", "test", source] => Action::Test {
            source: (*source).into(),
            accept_upload: false,
        },
        ["sources", "test", source, "--accept-upload"] => Action::Test {
            source: (*source).into(),
            accept_upload: true,
        },
        ["sources", "secret", id, field, "--remove"] => Action::SourcesSecret {
            id: (*id).into(),
            field: (*field).into(),
            secret: None,
        },
        ["sources", "secret", id, field, "--stdin"] => Action::SourcesSecret {
            id: (*id).into(),
            field: (*field).into(),
            secret: Some(secret_stdin()?),
        },
        ["plugins", "list"] => Action::PluginsList,
        ["plugins", "show", id] => Action::PluginsShow { id: (*id).into() },
        ["plugins", "import", path] => import(path, false)?,
        ["plugins", "import", path, "--accept-program"] => import(path, true)?,
        ["dictation", "start", "--source", source] => Action::DictationStart {
            source: (*source).into(),
            language: String::new(),
        },
        [
            "dictation",
            "start",
            "--source",
            source,
            "--language",
            language,
        ] if matches!(*language, "zh" | "en" | "auto") => Action::DictationStart {
            source: (*source).into(),
            language: if *language == "auto" {
                String::new()
            } else {
                (*language).into()
            },
        },
        ["dictation", "stop", operation] => Action::DictationStop {
            operation: (*operation).into(),
        },
        ["dictation", "cancel", operation] => Action::Cancel {
            operation: (*operation).into(),
        },
        ["dictation", "result", operation] => Action::Result {
            operation: (*operation).into(),
        },
        ["sources", verb @ ("add" | "update"), id, rest @ ..] => {
            return source(id, *verb == "update", rest);
        }
        _ => return Err(invalid()),
    };
    Ok(action)
}
fn source(id: &str, update: bool, rest: &[&str]) -> Result<Action, Response> {
    let (mut plugin, mut name, mut enabled) = (None, None, None);
    let mut config = BTreeMap::new();
    let mut secret_field = None;
    let mut secrets_json = false;
    let mut index = 0;
    while index < rest.len() {
        if rest[index] == "--secrets-stdin" && secret_field.is_none() && !secrets_json {
            secrets_json = true;
            index += 1;
            continue;
        }
        let pair = rest.get(index..index + 2).ok_or_else(invalid)?;
        match pair {
            ["--plugin", value] if plugin.is_none() && !update => plugin = Some((*value).into()),
            ["--name", value] if name.is_none() => name = Some((*value).into()),
            ["--enabled", value] if enabled.is_none() => {
                enabled = Some(match *value {
                    "true" => true,
                    "false" => false,
                    _ => return Err(invalid()),
                })
            }
            ["--secret-stdin", field] if secret_field.is_none() && !secrets_json => {
                secret_field = Some(*field)
            }
            ["--set", value] => {
                let (key, value) = value.split_once('=').ok_or_else(invalid)?;
                if config.insert(key.into(), value.into()).is_some() {
                    return Err(invalid());
                }
            }
            _ => return Err(invalid()),
        }
        index += 2;
    }
    if !update && (plugin.is_none() || name.is_none()) {
        return Err(invalid());
    }
    Ok(Action::SourcesSave {
        id: id.into(),
        update,
        plugin,
        name,
        enabled,
        config,
        secrets: match secret_field {
            Some(field) => BTreeMap::from([(field.into(), secret_stdin()?)]),
            None if secrets_json => {
                let values: BTreeMap<String, String> = serde_json::from_str(&input_stdin(110_000)?).map_err(|_| Response::error("missing_secret", "Expected a JSON object of secret field names and string values on stdin."))?;
                if values.is_empty()
                    || values.len() > 12
                    || values.values().any(|v| v.is_empty() || v.len() > 8192)
                {
                    return Err(Response::error(
                        "missing_secret",
                        "Secret input exceeds field/count/size bounds.",
                    ));
                }
                values
            }
            None => BTreeMap::new(),
        },
    })
}
fn secret_stdin() -> Result<String, Response> {
    input_stdin(8192)
}
fn input_stdin(limit: u64) -> Result<String, Response> {
    use std::io::{IsTerminal, Read};
    if std::io::stdin().is_terminal() {
        return Err(Response::error(
            "missing_secret",
            "Pipe secret input on stdin; an interactive terminal is not read.",
        ));
    }
    let mut secret = String::new();
    std::io::stdin()
        .take(limit + 1)
        .read_to_string(&mut secret)
        .map_err(|_| Response::error("missing_secret", "Cannot read secret input."))?;
    let secret = secret.trim_end_matches(['\r', '\n']);
    if secret.is_empty() || secret.len() as u64 > limit {
        return Err(Response::error(
            "missing_secret",
            "Supply nonempty secret input within the documented byte limit on stdin; never put it in argv.",
        ));
    }
    Ok(secret.into())
}
fn import(path: &str, accept_program: bool) -> Result<Action, Response> {
    use std::io::Read;
    let path = PathBuf::from(path);
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|_| Response::error("invalid_config", "Cannot inspect manifest file."))?;
    if !metadata.is_file() || metadata.len() > 32768 {
        return Err(Response::error(
            "invalid_config",
            "Manifest must be a regular file up to 32768 bytes; symlinks are rejected.",
        ));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(32769).read_to_end(&mut bytes))
        .map_err(|_| invalid())?;
    if bytes.len() > 32768 {
        return Err(invalid());
    }
    let manifest = serde_json::from_slice(&bytes)
        .map_err(|_| Response::error("invalid_config", "Manifest JSON is invalid."))?;
    Ok(Action::PluginsImport {
        manifest,
        accept_program,
    })
}
