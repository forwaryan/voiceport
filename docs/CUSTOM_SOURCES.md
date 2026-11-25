# 自定义转写来源

2026-11-08 开发实现：可以在原生 AppKit 偏好设置中添加、编辑、删除自定义来源，导入外部插件，并显式发送内置样例。开发自检不等于独立验收；真实 GUI、麦克风、云端质量和目标接收的状态以各轮证据为准。

一个插件是一种接入方式，一个来源是该插件的一份配置。每次听写只使用一个选定来源。可以为同一兼容 HTTP 插件配置多个服务，不需要 Orca 模型或 ChatGPT 登录；现有 ChatGPT、Paraformer 和 Whisper 仍保留。插件清单、JSONL 扩展协议及参考适配器见 [转写插件](TRANSCRIPTION_PLUGINS.md)。

## 模型窗口中的云端配置

「语音模型… → 云端」提供 **GPT Transcribe** 与 **OpenAI-compatible** 两个入口。前者预填 OpenAI 的 API 基础地址与 `gpt-4o-transcribe`，使用独立 API 密钥和服务计费；不复用 ChatGPT 登录。后者可连接兼容服务或本机服务器。

配置包含显示名称、基础 URL、模型和可选 API 密钥。例如 `https://api.groq.com/openai/v1` 会转换为 `https://api.groq.com/openai/v1/audio/transcriptions`；已经带完整标准路径时不重复追加。基础 URL 不接受查询参数、用户名、密码或 fragment；本地服务器可使用 loopback HTTP 并留空密钥。未改动的旧自定义完整 endpoint 在编辑时原样保留。

保存后配置显示在云端列表，可编辑、移除或点击「使用」。保存和启用不联网、不录音，也不自动选择；只有之后主动听写或显式测试才向所选地址发送音频。每行显示音频目的地，质量显示「未测量」，配置完成不代表服务已连通。移除当前来源保留不可用选择，不自动回退。

## 添加兼容服务

在「设置 → 管理来源…」点击「添加转写方式… → 连接兼容 API…」，直接填写服务信息，**不需要下载插件或准备 JSON 文件**。服务须支持 OpenAI 兼容的音频文件转写接口；官网地址、聊天接口和控制台链接不能当作转写接口使用。也可在插件库的 HTTP 卡片点击「填写服务信息…」。

填写名称、完整接口地址、音频模型 ID 及服务要求的 API Key，勾选「启用此来源」并保存，再回到听写页选择该来源。表单提供地址和模型 ID 示例；示例地址不可直接使用，实际值以服务商文档为准。

如果拿到的是插件作者提供的 VoicePort `.json` 清单，选择「添加转写方式… → 导入本地插件…」。本地适配器必须已安装，确认清单所指向的程序后才导入。导入完成会定位到对应卡片，继续点击「配置此插件…」填写并启用来源。GitHub 链接、zip 压缩包和模型文件不能直接作为清单导入。

| 字段 | 规则 |
| --- | --- |
| 显示名称 | 用户定义，改名保留稳定来源 ID。 |
| 完整转写地址 | 原样请求，不补路径，例如 `http://127.0.0.1:8000/v1/audio/transcriptions`。 |
| 模型标识 | 服务提供方的音频模型标识，例如 `whisper-1`，不能使用只支持聊天的模型名。 |
| 独立 Bearer key | 可选，保存到此来源自己的 Keychain revision 条目，配置只保存引用。 |
| 显式启用 | 添加后只有启用并在主下拉框选中才会用于听写。 |

保存只校验并写入本地配置，不请求服务。VoicePort 不会自动安装模型、启动服务或验证服务可达性。缺失、禁用或损坏来源保持可见不可用，不会默默切换到云端。主来源下拉框仅在内容改变时更新，不在 100ms 刷新中反复重建或重复选择。

密钥字段留空保留原密钥，勾选「移除密钥」会清除。编辑、删除、选择和语言变更在 busy 期间禁用。来源选择延用原 `transcriptionSource` 偏好，原 ChatGPT/Orca IDs 保持兼容。

## HTTP 接口契约 v1

停止后，在后台从保留录音生成 16 kHz、单声道、signed PCM16 little-endian WAV，最长 180 秒，然后向完整地址发送一次 `POST multipart/form-data`。

| 字段 | 值 |
| --- | --- |
| `file` | `dictation.wav`、`Content-Type: audio/wav`。 |
| `model` | 配置中的音频模型标识。 |
| `response_format` | `json`。 |
| `language` | 按选择发送稳定语言代码，如 `zh`、`en`、`ja`、`ko`、`fr`、`ar`、`hi`；自动时省略。服务的支持范围以其文档为准。 |
| `Authorization` | 有独立 key 时 `Bearer <此来源密钥>`，无 key 时省略。 |

成功要求 HTTP 2xx 和 JSON 对象内的字符串 `text`，额外字段忽略：

```json
{"text":"识别出的文字"}
```

空字符串表示没有识别到语音。非 2xx、超时、无效 JSON、非对象、缺少或非字符串 `text` 都是失败。响应最大 1,000,000 字节；非 2xx 不显示可能含凭据的服务正文。连接 timeout 10 秒、read timeout 30 秒、总 timeout 110 秒；禁止重定向、隐式重试和系统代理。远程仅 HTTPS，HTTP 仅明确 loopback；拒绝 URL 用户名、密码和 fragment，保持证书验证。

自定义客户端与 ChatGPT 生产客户端完全分开。它不读 Codex 登录，不带 `chatgpt-account-id`、`originator`，不会把 ChatGPT 凭据发到自定义 endpoint。ChatGPT 生产固定端点与专用 headers 沿用原实现。

## 样例测试

在「设置 → 测试各来源连通性与耗时…」打开来源测试列表，或从「我的来源」行的「查看测试…」定位。点击某一行的「测试」即可单独测试，也可以点击「测试可用来源」逐个测试目录中就绪的来源，不受当前听写选择影响。打开、定位和查看结果不会请求服务；实际点击测试才执行内置一秒静音样例。云端及本机适配器可能产生服务用量，窗口会说明这一点。

- ChatGPT 使用固定转写端点和现有登录；HTTP 服务使用各自的地址、模型和独立密钥。仅凭登录文件存在或端口可连接不会显示成功，必须收到正确协议响应。正确响应中的空 `text` 对静音样例也算接口已响应。
- 本地模型实际初始化运行时并解码合成静音，不使用网络；显示的耗时包括模型加载。JSONL（如 QQ）走既有适配器执行、取消和资源释放流程，本机程序可能继续请求其服务。
- 每行显示排队、测试中、可用、失败、超时或取消状态、样例总耗时和最近测试时间；错误详情可在结果提示中查看。耗时从该行开始执行到完成，不含排队等待，也不是网络 ping，不能单凭跨类型耗时排名判断识别质量。
- 一项失败不阻止后续来源。每项 30 秒后请求取消；等待后台退出及子进程清理确认期间仍保持忙碌，不会启动下一项或放开录音。清理无法确认时停止队列，可用「取消 / 检查清理」显式检查一次，不重试转写。
- 取消或关闭测试窗口会取消剩余队列，已完成结果保留在当前 App 会话中，重开可查看。来源配置变更后旧结果失效；不会把结果写入来源配置或跨启动冒充最新状态。

样例不含用户声音；不请求麦克风、不读取真实输入目标、不自动投递文字，也不改变选中的来源、语言、自动输入偏好或已有听写结果。测试期间暂停录音及配置修改。适配器返回固定文字或静音得到非空文字也只能证明接口/模型响应，不能宣称真实识别质量通过。

QQ SmartPtt 适配器的失败会区分桥接未运行/地址失效、连接中断、QQ 未就绪、配对失效、仍在处理、清理失败、编码失败和转写超时，并给出对应检查方向。普通 QQ 不会自动提供本机桥接；检查前提及当前支持范围见 [QQ 桥接说明](../scripts/qq_bridge/README.md#换电脑时先检查接入前提)。目前没有可交付的一键安装或一键修复普通 QQ 的流程。

错误提示只把 QQ 适配器的已知固定错误码映射为固定文字，未知错误、原始 stderr 和任意错误正文不会显示，以免泄露凭据。超时不代表 QQ 已停止处理，不自动重试，也不切换来源。

## 预览、重试和其他协议

通用 HTTP 和外部 JSONL 适配器第一版仅停止后转写，不启动系统 Speech 充当第二来源。Paraformer 的本地 streaming 保留。外部清单声明 streaming 或不支持协议会明确拒绝；任意 WebSocket、任意模型文件、供应商专用实时协议不属于兼容接口。

录音开始固定插件版本、地址、模型、语言与凭据关联。重试保留原快照，不重新读取当前选中的来源，也不自动写入新焦点。原凭据被删或替换时明确失败，需用户重新确认配置。取消和退出保持 busy，直至后台同 generation 任务释放录音和子进程资源。

非兼容服务通过 [JSONL v1 本地适配器](TRANSCRIPTION_PLUGINS.md) 接入：显式本地可执行路径和参数数组，无 shell eval、自动安装或下载器。导入页展示来源与将执行的程序，导入本身不执行。插件有当前用户权限，用户需信任程序；程序不得派生后台进程或逃离 VoicePort 为其创建的进程组。

## 隔离 GUI 验证

来源配置默认写入 `~/Library/Application Support/VoicePort/transcription-sources-v1.json`。仅为测试 App 子进程设置 `VOICEPORT_SOURCES_PATH` 可将它改为专用绝对文件路径；override 优先于 HOME 派生的默认路径。空值、相对路径、目录、符号链接或无法检查的路径明确失败，不回落默认配置。指定文件仍经过同一 Store 的普通文件、大小、版本、0600 权限和原子保存规则。已有默认路径及来源 ID 保持兼容。

此变量只隔离来源配置，不改变 Orca 模型位置、Codex auth、系统 Keychain 或 NSUserDefaults。保留正常系统 HOME；另使用独立测试 bundle ID 和合成 CODEX_HOME。先复制本轮新包到一个不存在的测试包路径，修改副本 bundle ID 并重新 ad-hoc 签名；不要修改生产包。以下命令假设独立验收者已经准备好该副本，并将 `test_scope`、`test_app` 和 `test_domain` 换为自身唯一实际路径/域：

```sh
test_scope=/absolute/private-test-directory
test_app=/absolute/isolated-test/VoicePort.app
test_domain=dev.voiceport.gui.unique-test-id
mkdir -p "$test_scope/codex-synthetic"
defaults write "$test_domain" automaticPaste -bool false
defaults write "$test_domain" alternativeShortcut -bool true
VOICEPORT_SOURCES_PATH="$test_scope/sources.json" \
  CODEX_HOME="$test_scope/codex-synthetic" \
  "$test_app/Contents/MacOS/voiceport-app"
```

直接启动可执行程序使上述环境仅传给测试实例。启动前核对副本 Info.plist 的 `CFBundleIdentifier` 与 `test_domain` 一致；启动后核对实际路径/PID及自动输入为关闭，再进行管理来源和显式合成样例测试。不要按录音键，不读真实凭据或真实配置；仅使用合成 key 和受控 loopback。该来源路径机制不提供 Keychain 授权豁免；如出现系统认证等待，停止并报告，不处理认证、改变默认 Keychain 或枚举/猜删既有条目。清理凭据时仅精确删除本轮成功返回并记录的自建引用。

## 开发自检范围

受控测试覆盖实际 multipart WAV bytes、model、language、独立 Bearer、无 ChatGPT headers、单请求、失败、超时、取消、重定向、畸形和过大响应；JSONL 测试使用合成音频及受控程序验证 schema/version、加载不执行、错误输出、边界和进程回收。多实例配置、原子保存、缺失来源和凭据替换使用隔离数据与内存凭据替身，不读取真实用户 Keychain。

上述测试不外推真实识别质量。最终版本仍需由不同会话做独立 GUI、真实麦克风/服务及目标接收验证。
