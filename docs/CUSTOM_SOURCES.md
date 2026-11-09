# 自定义转写来源

2026-11-08 开发实现：可以在原生 AppKit 偏好设置中添加、编辑、删除自定义来源，导入外部插件，并显式发送内置样例。开发自检不等于独立验收；真实 GUI、麦克风、云端质量和目标接收的状态以各轮证据为准。

一个插件是一种接入方式，一个来源是该插件的一份配置。每次听写只使用一个选定来源。可以为同一兼容 HTTP 插件配置多个服务，不需要 Orca 模型或 ChatGPT 登录；现有 ChatGPT、Paraformer 和 Whisper 仍保留。插件清单、JSONL 扩展协议及参考适配器见 [转写插件](TRANSCRIPTION_PLUGINS.md)。

## 添加兼容服务

在「偏好设置 → 管理来源… → 添加来源…」选择「兼容音频转写 HTTP」。

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
| `language` | 显式中文为 `zh`，English 为 `en`，自动时省略。 |
| `Authorization` | 有独立 key 时 `Bearer <此来源密钥>`，无 key 时省略。 |

成功要求 HTTP 2xx 和 JSON 对象内的字符串 `text`，额外字段忽略：

```json
{"text":"识别出的文字"}
```

空字符串表示没有识别到语音。非 2xx、超时、无效 JSON、非对象、缺少或非字符串 `text` 都是失败。响应最大 1,000,000 字节；非 2xx 不显示可能含凭据的服务正文。连接 timeout 10 秒、read timeout 30 秒、总 timeout 110 秒；禁止重定向、隐式重试和系统代理。远程仅 HTTPS，HTTP 仅明确 loopback；拒绝 URL 用户名、密码和 fragment，保持证书验证。

自定义客户端与 ChatGPT 生产客户端完全分开。它不读 Codex 登录，不带 `chatgpt-account-id`、`originator`，不会把 ChatGPT 凭据发到自定义 endpoint。ChatGPT 生产固定端点与专用 headers 沿用原实现。

## 样例测试

选择来源后点击「测试转写（使用内置样例）」，确认目标后发送内置一秒静音 WAV。样例不含用户声音；不请求麦克风、不读取真实输入目标、不自动投递文字，支持取消。空 `text` 的界面措辞是「接口已响应，没有识别到语音」，不能宣称识别质量通过。适配器返回固定文字也只能证明协议/传输。

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
