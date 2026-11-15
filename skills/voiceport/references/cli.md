# VoicePort CLI reference

`voiceport [--json] [--socket ABSOLUTE_PATH] COMMAND`

`voiceport-rs` accepts the same commands. Global automation flags can appear before or after an App/Skill command; duplicate or unknown options fail. `--socket` takes precedence over `VOICEPORT_CONTROL_SOCKET`, then the default `/private/tmp/voiceport-control-UID/control.sock` on macOS. The chosen socket must be inside a same-user directory with mode 0700; its file has mode 0600. It must be an absolute real path with no symbolic-link ancestors. No implicit fallback to another instance occurs. A connection failure never starts the App.

## Offline commands

- `--version`: CLI package version and control schema.
- `help`, `--help`, `-h`: usage; legacy portable commands included.
- `skills list`: bundled guides and installed version: `core`, `cli`, `workflows`, `plugins`, `plugin-examples`, `plugin-protocol`, `plugin-template`.
- `skills get [GUIDE]`: embedded content matching this binary; default `core`. `plugins` is the new-service onboarding Skill, `plugin-examples` covers compatible/local APIs and JSONL conversion, `plugin-protocol` is the manifest/execution contract, and `plugin-template` is the example Python adapter source (plain text; retrieval never executes it).

## QQ installation diagnostics (macOS)

`voiceport --json qq doctor [--qq /absolute/path/QQ.app]` runs the version-matched embedded, read-only installation checker without a running App, a checkout, a socket or network access. It requires an installed, usable `/usr/bin/python3`; the checker uses isolated Python mode and only the standard library. Omitting `--qq` checks `/Applications/QQ.app` and `~/Applications/QQ.app`; multiple installations require an explicit absolute path. `--socket` is rejected for this offline command.

The report checks the QQ bundle identity, official Tencent signature/team, sandbox entitlement and sealed entry resources. `checks` explains each inspection and `blockers` gives fixed reasons and recovery guidance. Exit 0 means the diagnostic report was produced, including when QQ is absent or unsupported; inspect `data.blockers`. It always reports `connection_status:"not_tested"`, `transcription_verified:false` and `automatic_setup_supported:false`. An installation check does not prove QQ can transcribe, and ordinary QQ does not automatically expose a VoicePort bridge. The checker never launches or modifies QQ, logs in, reads account/chat/Keychain data, connects to the bridge or uploads audio.

Missing Python, failed helper execution, invalid output and the 45-second outer deadline return fixed `diagnostic_unavailable`, `diagnostic_failed` or `diagnostic_timeout` errors without raw helper output. Other platforms return `unsupported`; their real QQ connection is unverified. This command does not install a bridge or offer an automatic connection route.

## App commands (macOS)

- `status` or `app status`: state, busy, pending/warning, discovering, selected source, language, version, bundle path, PID, socket and permission metadata. While stopping, `pending:true` and `warning:"cancellation_pending"` do not claim cleanup has completed. No permission prompt, auth read, transcript, target content or audio. `data.input` adds the GUI automatic-input preference, a content-free failure stage, and the last explicit start route. Hotkey start metadata includes only captured/dispatch application PID and bundle ID plus a fixed outcome code, never field text. A start blocked before recording reports failed rather than leaving a previous ready state looking like a successful new recording.
- `app show [--settings]`: show the running App window, optionally on its settings page.
- `data.input.method` is `wechat_menu_paste` for the GUI's guarded native-menu compatibility path. It acknowledges a Paste request, not verified field contents; final text remains in the clipboard. CLI dictation still disables automatic delivery and never uses this path.
- `app quit`: graceful idle-only quit; busy or configuration work returns `busy`. It never force-kills an instance.

Launch is explicit and external: invoke the intended bundle's `Contents/MacOS/voiceport-app` or open that exact bundle after checking its path/version. There is no `app launch` command. For isolated tests, supply the socket/config environment to that exact child process before launch. Do not control a production App while validating a test copy.

## Sources and plugins

- `sources list`: configured instances plus discovered catalog entries.
- `sources show ID`: one configured instance with config and Keychain references; secret values are never returned. Built-in sources appear in list/catalog.
- `sources add ID --plugin PLUGIN --name NAME [--enabled true|false] [--set FIELD=VALUE]... [--secret-stdin FIELD | --secrets-stdin]`: create a stable instance ID; disabled by default. Required configuration must be valid even when disabled. `--secret-stdin FIELD` supplies one required/optional secret atomically from piped stdin during creation. For multiple required secret fields, `--secrets-stdin` reads one JSON object mapping declared secret field names to string values (maximum 12 fields, 8192 bytes per value, 110000 bytes input). The two stdin options are mutually exclusive; no secret values go in arguments. A plugin can have multiple instances.
- `sources update ID [--name NAME] [--enabled true|false] [--set FIELD=VALUE]... [--secret-stdin FIELD | --secrets-stdin]`: update only supplied fields; empty VALUE removes a config field. Plugin identity/version and stable source ID are retained.
- `sources remove ID`: delete instance and its own Keychain items; selected source ID is retained as unavailable, with no fallback to cloud.
- `sources select ID`: persist the available source selection using the App's preference domain. Disabled/missing sources cannot be newly selected. Disabling/removing the selected instance preserves its unavailable selection.
- `sources secret ID FIELD --stdin`: read a nonempty secret (maximum 8192 bytes, trailing CR/LF removed) from stdin into the App's Keychain transaction. Never supply the value as an argument. EOF without input fails; no interactive prompt is generated. The shared App Keychain transaction additionally requires each stored value to be at most 4096 bytes without control characters, matching GUI validation.
- `sources secret ID FIELD --remove`: remove that field's independent Keychain item, subject to the same validation as GUI edits.
- `plugins list` / `plugins show ID`: built-in and imported versioned descriptors, schema fields, program/argv/origin.
- `plugins import MANIFEST.json [--accept-program]`: inspect a local regular manifest up to 32768 bytes. Without acceptance, return `confirmation_required` and descriptor details without persisting or executing. With acceptance, validate and import, still without execution. JSONL programs run with current-user privileges when later explicitly tested or used; no sandbox is provided. Duplicate plugin IDs are rejected.
- `sources test ID [--accept-upload]`: without acceptance, show destination/sample information and fail with `confirmation_required`; with acceptance, submit built-in one-second silence through that plugin instance and return an operation ID. No microphone, target capture or delivery. Test is limited to configured HTTP/JSONL instances.

Configuration reads/writes and credential pinning use a background worker. Busy/discovery/configuration transitions can return `busy`; inspect status and wait for idle before a new user-requested mutation. Failed validation never silently changes selection or falls back to ChatGPT. A storage error after rename can have committed; inspect the store before deciding to retry.

## Dictation and test operations

- `dictation start --source ID [--language zh|en|auto]`: explicitly start recording using a pinned source/language and no automatic text delivery. The microphone permission must already be granted; the CLI does not request it. GUI preference values are unchanged.
- `dictation stop OPERATION`: stop that CLI recording and perform same-source transcription.
- `dictation cancel OPERATION`: request cancellation of that CLI operation, including a synthetic test. `stopping` remains busy until the worker has released its resources.
- `dictation result OPERATION`: read only the current CLI-owned operation. Running operations return state/busy; completed ones return text/empty or a structured error. Results are ephemeral; a newer CLI operation replaces the previous result. The App's GUI transcript is never returned by default or for an unrelated generation.

These commands never deliver text to an input target or clipboard. Stop/result/cancel require the ID returned by the start/test command; they cannot take over a GUI operation. `result` may contain explicit user-requested text and should not be logged as default diagnostics.

## JSON and exits

Automation commands with `--json` produce exactly one stdout object:

```json
{"schema_version":1,"status":"ok","data":{"state":"idle"},"error":null}
```

An error has `status:"error"`, `error:{"code":"busy","message":"..."}` and usually null data. Confirmation errors contain a proposed descriptor/destination in data. Parse `error.code`, never localized text. Diagnostics use stderr. Exit codes: 0 success; 1 operational/plugin/transport/permission failure; 2 invalid arguments/request/config; 3 app_not_running; 4 busy; 5 confirmation_required; 6 not_found; 7 unsupported; 130 cancelled. An accepted asynchronous operation exits 0 for submission; inspect `dictation result` for final success/error/cancellation. A completed plugin failure uses `plugin_error`; no raw service body, plugin stderr or secret is included.

IPC schema is 1, four-byte big-endian frame length plus JSON, maximum 262144 bytes, one request per connection and five-second total transport deadline. No automatic retries. If a reply is lost or deadline expires, a mutation may have happened: inspect state before deciding what to do next. Polling read-only status/result does not repeat uploads. Sockets are same-user controls, not protection from hostile code already running as that user.

## Original portable contract

`voiceport host [--auth PATH]`, `demo`, `doctor [--auth PATH]`, and `transcribe AUDIO [--language zh] [--auth PATH]` retain their original text/JSONL behavior and argument handling. The new global `--json`/`--socket` flags apply to automation commands, not these legacy commands. `host` already has its own bounded JSONL protocol. `transcribe` explicitly uploads the supplied file to ChatGPT using Codex file login and supports Ctrl-C cancellation; no automatic resubmission or delivery. This release has no `transcribe --source` option. Doctor checks only auth-file metadata. Windows retains the portable commands and offline guides; App control returns `unsupported`.
