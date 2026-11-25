# VoicePort CLI and product Skill

`voiceport` is the public executable. `voiceport-rs` retains the original help/host/demo/doctor/file-transcription interface and accepts the same new automation commands. macOS App control uses a local Unix socket connected to the running App's existing Controller, source Store and Keychain transactions. It does not simulate window clicks, start the App implicitly, or run a second transcription daemon.

## Install and find the CLI

Build the locked workspace with `cargo build --workspace --release --locked`. The public executable is in Cargo's release target directory. The signed App package includes `Contents/MacOS/voiceport` alongside `Contents/MacOS/voiceport-app`, and the product Skill under `Contents/Resources/skills/voiceport/`. For an installed bundle use its exact path:

```sh
"$HOME/Applications/VoicePort.app/Contents/MacOS/voiceport" --version
"$HOME/Applications/VoicePort.app/Contents/MacOS/voiceport" --help
"$HOME/Applications/VoicePort.app/Contents/MacOS/voiceport" skills get cli
```

These examples use the README's default installation at `~/Applications/VoicePort.app`. If the user selected another bundle location, substitute its actual path; packaging itself writes only the requested output path. Nothing writes `/usr/local/bin` or changes shell startup files. A user can add the chosen bundle's `Contents/MacOS` to their own PATH, or continue using the full executable path. App launch is explicit: open the intended bundle after checking its path/version. There is no implicit launch and no `app launch` subcommand.

## Commands and automation

The single versioned detailed command reference is [skills/voiceport/references/cli.md](../skills/voiceport/references/cli.md). It is compiled into both CLI entry points, together with [the core Skill](../skills/voiceport/SKILL.md) and [workflows](../skills/voiceport/references/workflows.md). `voiceport skills list` and `voiceport skills get [GUIDE]` work without an App or network and return the matching installed version; with `--json` the content is in `data.text`. Guides include `core`, `cli`, `workflows`, `plugins`, `plugin-examples`, `plugin-protocol`, and `plugin-template`. The last four expose the [plugin onboarding Skill](../skills/voiceport-plugins/SKILL.md), [three examples](../skills/voiceport-plugins/references/examples.md), the existing manifest/protocol document, and the example Python bridge source. Reading the template does not execute it.

The command groups are `status` / `app status/show/quit`, `sources list/show/add/update/remove/select/secret/test/probe/probe-result/probe-cancel`, `plugins list/show/import`, and `dictation start/stop/retry/cancel/result`. Source IDs are stable instance IDs, distinct from plugin IDs; one plugin may supply multiple independent instances. Selection persists in the same NSUserDefaults domain as GUI selection. Disabling/removing a selected source keeps that ID unavailable; it never falls back to ChatGPT.

For automation use `voiceport --json [--socket ABSOLUTE_PATH] COMMAND`. The versioned response is `{schema_version:1,status:"ok"|"error",data:...,error:null|{code,message}}`; stdout contains one result object and diagnostics use stderr. Status includes state/busy/source/language/version/bundle/PID/permissions and no transcript, credentials or target content. The exit codes are 0 success, 1 operational failure, 2 invalid arguments/request/config, 3 App missing, 4 busy, 5 explicit confirmation missing, 6 ID missing, 7 unsupported platform/operation, and 130 cancelled. An asynchronous submission succeeds when accepted; inspect the returned operation with `dictation result ID` for its final outcome. Parse `error.code`, not message text.

`plugins import MANIFEST` shows the local program/argv/origin and returns confirmation_required without persistence until `--accept-program` is explicitly supplied. Import never executes code. `sources test ID` likewise requires `--accept-upload`; the accepted action sends built-in one-second silence and returns an operation ID, without microphone/target/automatic delivery. Empty text is a valid result, not recognition-quality proof. Only explicit `dictation start --source ID` records; it pins source/language and fixes automatic delivery to false without modifying the GUI's preferences or current target. Stop/cancel/result require the current CLI operation ID and cannot control an unrelated GUI generation. Busy operations reject mutations and graceful quit. Cancellation remains observable as stopping until the worker releases its resources. Local adapter cleanup has one bounded initial confirmation phase. If termination cannot be confirmed, the App retains the recording and blocks new operations and graceful quit. A further explicit `dictation cancel ID` requests one background observation without another signal or a renewed termination budget; status/result polling does not request recovery. Do not automate repeated cancellation or force an exit to clear this state. Once cleanup is confirmed, an original cancellation releases the recording; an original transcription error retains it for a later explicit retry.

`dictation retry OPERATION` explicitly re-transcribes that retained failed CLI recording using the same pinned source/language and `delivery:"none"`. It requires the original matching ID and Controller generation, failed state with current `retry_available:true`, and no busy work or pending cleanup. GUI recordings, superseded generations and legacy synthetic `sources test` operations are ineligible. Acceptance returns a new operation ID; use it for result/cancel, as the old ID then returns `not_found`. Terminal dictation errors include data `{operation,state:"failed",retry_available:bool,delivery:"none"}`. Retry availability reflects current ownership/retention, so later GUI work can invalidate a previous report. Retrying may upload audio again: only explicit user consent authorizes it, and a lost reply must never trigger automatic resubmission.

`--language CODE|auto` accepts the same stable speech-language codes as the model window, including `zh`, `en`, `ja`, `ko`, `yue`, `fr`, `de`, `ar` and `hi`. Models and services may support a subset. An unsupported local-model language is rejected before recording; HTTP forwards an explicit code and omits it for `auto`. Source and language remain pinned for the entire recording/retry.

CLI recording uses existing microphone/Speech grants only and never invokes OS authorization requests. Microphone permission must already be granted; missing optional Speech permission only disables device-side preview. Inspect `status` for permissions without changing them.

Secret values are accepted only through dedicated stdin options (`sources secret ID FIELD --stdin`, or `--secret-stdin FIELD` during add/update); stdin must be a pipe/file, with nonempty bounded input. Values never appear in argv, response, exported Store or diagnostics. The App stores independent Keychain references using the same validated transaction as GUI editing. Normal metadata commands do not read secrets; credential values are pinned only for an explicit operation. Runtime errors omit raw service bodies and adapter stderr.

The original portable `host`, `demo`, `doctor`, and `transcribe AUDIO` contracts remain unchanged, including host's own JSONL protocol, auth-path precedence and Ctrl-C cancellation. Automation `--json`/`--socket` flags do not extend these legacy commands. File transcription remains the explicit ChatGPT operation; `transcribe --source` is not implemented. Windows can use the portable commands and offline Skill/help; App operations return unsupported.

## 来源连通性与耗时测试

`voiceport sources --help`、`voiceport dictation start --help`、`voiceport dictation retry --help` 或 `voiceport help sources` 可离线查看对应指南，JSON 模式仍通过 `data.text` 返回。

新增 `sources probe` 复用 GUI 的独立测试引擎，覆盖 ChatGPT、本地模型和自定义来源，保留当前听写文字、录音及设置。先查看测试目标，再按明确授权提交一次：

```sh
voiceport --json sources probe chatgpt
voiceport --json sources probe chatgpt --language zh --accept-upload
voiceport --json sources probe-result OPERATION
voiceport --json sources probe-cancel OPERATION
```

`OPERATION` 使用实际返回的 ID；隔离实例的每条命令均携带同一个 `--socket`。`sources probe --all --language en --accept-upload` 串行测试全部目录项，需已授权所有目的地；不可用项只记失败，不执行。报告返回逐来源固定状态码、样例执行耗时和完成时间，不返回样例识别文字或原始错误正文。退出 0 表示报告已读取，完成且全部通过须检查 `state:"completed"` 与 `all_passed:true`。静音测试不代表识别准确率。

取消和清理未确认时仍保持 busy，不能启动新任务或正常退出；只读轮询不触发新的清理观察。再次显式取消只检查一次，不能自动反复调用。独立操作 ID 不接管 GUI 测试；旧 `sources test` 与 `dictation result/cancel` 的插件样例协议继续兼容。新操作需要同样包含这些命令的 App，旧 App 会返回 `invalid_request`，应显式启动匹配版本后再使用。

## 已识别但未输入

先用同一 `--socket` 读取 `status`，核对 `bundle_path`、PID 和 `permissions`，然后查看 `input.gui_receipt` 的 `session`、`generation`、转写和投递状态。`input.automatic` 与 `input.start` 仍用于检查设置和启动路径。旧版 App 可能缺少 `gui_receipt`，旧 `input.method` 和 `input.failure_stage` 可能沿用此前一轮的值。需要展示设置时用 `app show --settings`。CLI 听写始终为 `delivery:"none"`。

`voiceport --json --socket ABSOLUTE_SOCKET status --expect-bundle ABSOLUTE_APP [--expect-pid PID]` 从一次状态查询计算 `data.readiness`。它核对预期 App 实例，给出麦克风、可选语音预览、辅助功能、快捷键状态及可执行的检查项。路径必须为绝对路径，PID 可选。它沿用原 `status` 的退出码和同用户 IPC；旧 App 缺失的字段报告 `unknown`。诊断只读取现有权限与状态。 [CLI 指南](../skills/voiceport/references/cli.md#app-commands-macos)

GUI 的来源 App 负责识别，目标 App 接收文字。QQ 或微信目标输入失败时，先按[投递排查流程](../skills/voiceport/references/workflows.md#recognized-text-not-delivered)检查原目标；不要自动探测来源、重传音频、重试识别或重新录音。微信当前与旧版路径、选路字段和接收证据的区别，见[投递状态契约](INPUT_DELIVERY.md#cli-状态字段与投递回执)。

## QQ 安装诊断

`voiceport --json qq doctor` 自动检查 `/Applications/QQ.app` 和 `~/Applications/QQ.app`；多份安装时使用 `qq doctor --qq /绝对路径/QQ.app` 指定。诊断器随 CLI 编译分发，无需源码目录、运行中的 VoicePort 或桥接 socket，依赖可用的 `/usr/bin/python3`。它只读安装元数据并用系统 codesign 验证官方签名、腾讯团队、沙盒及入口资源封存，不启动或修改 QQ，不登录、不读聊天/账户/Keychain、不上传音频。完整命令与错误码见 [CLI 指南](../skills/voiceport/references/cli.md#qq-installation-diagnostics-macos)。

`status:"ok"`/退出 0 表示诊断报告已产生；安装问题在 `data.blockers` 中。`connection_status:"not_tested"`、`transcription_verified:false`、`automatic_setup_supported:false` 明确说明尚未测试真实连接。普通 QQ 不会自动提供 VoicePort 桥接，安装检查通过也不能作为转写联通证明。缺运行环境、失败或超时均返回固定可读错误，原始子进程输出不进入错误信息；其他平台返回 `unsupported`。

## IPC boundaries

The shared `crates/voiceport-control/` crate holds protocol and transport only. Schema 1 uses a four-byte big-endian length and JSON, with a 262144-byte limit, one request per connection, five-second total transport deadline, at most four active connections and a four-request queue. Reads/writes recompute remaining deadline, so slow partial frames cannot extend it indefinitely. The listener runs separately from AppKit and never joins on the UI thread. Excess connections receive busy when possible; invalid version/frame/JSON never reaches Controller dispatch. Request deadlines are checked before main-thread dispatch and before starting a prepared recording/sample.

The default macOS path is `/private/tmp/voiceport-control-UID/control.sock`. Override with `VOICEPORT_CONTROL_SOCKET` on the App and explicit `--socket` on the CLI. Empty/relative/overlong paths, linked ancestors, unsafe ownership/modes and non-socket objects fail rather than falling back. The immediate directory must be owned by the effective user with mode 0700, the socket mode 0600. Both ends verify peer UID. Startup serializes publication and recovery. It reclaims a socket from a crashed instance only when the public path still matches VoicePort’s private ownership link and a nonblocking connect returns connection refused. A live listener, a changed path or a socket of unknown origin is preserved. Graceful shutdown removes only its own matching paths. For an older socket without ownership evidence, use a fresh explicit path for a test instance or have the owner verify that the original process has exited and move that socket aside. No TCP management endpoint exists. This is a same-user boundary, not a sandbox against hostile same-UID code.

The App timer drains bounded requests on its main thread, and all Controller transitions stay there. Store I/O and credential pinning run on a bounded background path serialized with GUI edits, permissions and sessions. GUI/CLI use the same Store validation and save/credential transaction. A socket timeout or lost reply is ambiguous for a mutation already dispatched; no write/upload is automatically retried. Inspect state before a new user-requested action. Configuration persistence errors after atomic rename may likewise have committed. Full lifecycle and real device/service behavior require their own evidence.

## Test instances

Keep normal HOME; use an independent bundle ID and ad-hoc signature, synthetic CODEX_HOME, a unique 0700 socket parent and explicit `VOICEPORT_SOURCES_PATH` on the test App child. Always pass its `--socket` to the CLI and compare status bundle_path/PID before mutations. These overrides do not isolate system Keychain or TCC automatically. Never use production audio, keys, App paths or targets in automated validation. Source tests use synthetic silence and controlled services; recording tests use inert controller/recorder fixtures and never request permissions. See the [workflow guide](../skills/voiceport/references/workflows.md) and [custom-source isolation notes](CUSTOM_SOURCES.md).

## Skill layout and installation

Product Skills live at repository root: `skills/voiceport/SKILL.md` covers App control with CLI/workflow references; `skills/voiceport-plugins/SKILL.md` covers connecting a new service, with examples, a JSON HTTP adapter template, and a generator that creates a new reviewable adapter/manifest directory without executing or importing it. Both complete directories are bundled under `Contents/Resources/skills/`. The CLI source remains in `crates/voiceport-cli/`; protocol code lives in its small shared crate. The Skill is not a repository-development instruction in `.claude/skills`, and packaging does not modify the user's global Agent configuration.

The structure follows two inspected fixed-version examples: [agent-browser](https://github.com/vercel-labs/agent-browser/tree/4b0e9f01fa8d214d39bbfe5391257640634fb9a0/skills/agent-browser) keeps a short discovery entry and returns installed-version guidance through its CLI; [Google Workspace CLI](https://github.com/googleworkspace/cli/tree/cd3697e07eb520056dcf142d0c7439bbb9ba32e9/skills) separates root skills from Rust crates and uses capability references. VoicePort embeds its repository Skill files directly, avoiding a second drifting command manual or a Node runtime.

To install manually, copy the complete `skills/voiceport` or `skills/voiceport-plugins` directory from the chosen checkout or App resources into the supported Agent's skill directory, retaining its directory name (for example `~/.claude/skills/voiceport-plugins` or `~/.codex/skills/voiceport-plugins`), after inspecting the destination and deciding whether to replace an existing installation. Alternatively, a standard skills installer supporting repository selection can select only the `voiceport` skill from this repository; use that installer's actual help for its syntax. No installer is invoked automatically. Installing a Skill is not authorization to record/upload audio or deliver text. Agents should start with status, configuration inspection and explicit synthetic testing, not GUI clicks.

The product inventory includes `skills/` files and directory modes, including untracked Skill inputs. Bundle checks require the exact source Skill tree, the current signed CLI and App executable, locked license manifest and signatures. No new external dependency is introduced; the shared protocol uses already-locked serde/serde_json/libc. License metadata must still be regenerated when Cargo build inputs change.
