# 转写插件与来源实例

VoicePort 的一个插件定义一种接入方式；一个来源保存该插件的一份配置。可以为同一插件添加多个服务，每次听写只选择一个来源。第一版提供原生兼容 HTTP 插件和可导入的 JSONL 本地程序适配器，保留 ChatGPT、Orca Paraformer 和 Whisper Tiny。

这是实现契约和使用说明，不是独立验收报告。开发自检与真实麦克风、真实服务、GUI 和目标接收的验证范围须分别记录。

## 使用

打开「设置 → 管理来源…」，进入原生「插件与转写来源」窗口。点击「添加转写方式…」，按手头的材料选择：有兼容转写 API 的地址、模型名和密钥时直接连接并填表，无需导入文件；已有插件作者提供的 VoicePort `.json` 清单且适配器已安装时，选择导入本地插件。只有官网、GitHub 链接、zip 或模型文件不能直接导入，需确认兼容 API 或向作者索取清单和适配器。

「插件库」按卡片展示内置接入方式与本地导入的适配器，显示用途、版本、转写能力和已配置来源数。HTTP 卡片点击「填写服务信息…」直接配置；本地插件导入后自动定位到对应卡片，再点「配置此插件…」，填写名称和字段，显式勾选「启用此来源」，保存后回到听写页选用。「我的来源」集中展示独立配置、启用状态及需要修复的配置，逐行提供编辑和删除。顶部搜索支持插件/来源名称与稳定 ID；没有来源时直接提供添加引导。

编辑已有来源直接进入对应插件表单。保存前校验完整候选配置，包括显示名称的非空/UTF-8 字节长度、重复身份、字段与配置容量；新增密钥也使用与 Keychain 写入相同的纯校验规则（最多 4096 个 UTF-8 字节，不含控制字符）。校验失败保留已填写内容，便于修正；所有待保存密钥都通过检查后才开始创建密钥引用。一次修改触发来源目录刷新后，下一次添加、编辑、删除或导入须等刷新完成，期间仍可搜索和查看插件详情。事务结束后重新读取已持久化配置，提交后密钥清理失败仍显示原错误并刷新来源目录。若重新读取失败，本次管理窗口停止写入，提示关闭后重新打开。保存不会联网或执行插件。已保存的自定义来源出现在主来源下拉框中，选中后才用于下一次听写；「已启用」仅是配置状态，不能代表服务连接或识别质量已验证。

「编辑来源」保留来源 ID。密钥留空表示保留旧密钥；「移除密钥」会清除该字段。删除、禁用、缺失插件、版本不匹配或服务失败都不会自动切换到 ChatGPT。旧的 `chatgpt`、`orca:paraformer-bilingual-zh-en`、`orca:whisper-tiny` 选择保持兼容。

「测试转写（使用内置样例）」先展示目标，然后仅在确认后后台发送内置一秒静音 PCM16 WAV；不会启用麦克风或读取输入目标，可通过主界面「取消」停止。空 `text` 表示接口响应但没有识别到语音，返回固定文字也不能证明 ASR 识别质量。此入口用于配置实例，内置 ChatGPT 和 Orca 模型不使用该样例入口。

## 内置插件

| 稳定插件 ID | 实际能力 | 认证 |
| --- | --- | --- |
| `voiceport.chatgpt` | 设备端 Speech 预览，停止后固定 ChatGPT 端点转写 | 现有专用 Codex 登录客户端 |
| `voiceport.orca-paraformer` | 本地流式与最终转写 | 无 |
| `voiceport.orca-whisper` | 本地批量转写 | 无 |
| `voiceport.http-transcription` | 后台生成 16 kHz、mono、PCM16 WAV，停止后 HTTP 转写 | 每来源独立可选 Bearer |

HTTP 配置字段为完整 `endpoint`、`model`、可选 `api_key`。直接 POST 到完整地址，不补路径；multipart 字段为 `file`（`dictation.wav`、`audio/wav`）、`model`、`response_format=json` 和显式语言 `zh` / `en`（自动时省略）。成功仅接受 2xx JSON 对象中的字符串 `text`，允许空字符串。见 [自定义来源](CUSTOM_SOURCES.md)。

## 外部插件清单 v1

「管理来源 → 添加转写方式… → 导入本地插件…」读取本地 JSON，并在确认页显示清单路径、声明来源、插件 ID/版本、可执行路径和参数数组。导入不执行程序。第一版不自动安装、下载、更新或发现市场插件；版本不可静默覆盖，重复 ID 拒绝导入。新版本应使用独立 ID，或在关闭应用后由用户明确维护配置并重新确认来源。

```json
{
  "schema_version": 1,
  "protocol_version": 1,
  "id": "example.adapter",
  "version": "1.0.0",
  "name": "我的音频适配器",
  "origin": "本机安装目录或发布者说明，供用户核对",
  "adapter": "jsonl",
  "capabilities": {"batch": true, "streaming": false, "system_preview": false},
  "fields": [
    {"id": "endpoint", "label": "完整地址", "kind": "url", "required": true},
    {"id": "model", "label": "模型", "kind": "string", "required": true},
    {"id": "api_key", "label": "独立密钥", "kind": "secret", "required": false}
  ],
  "program": {"executable": "/absolute/local/program", "args": ["--jsonl"]}
}
```

只支持 `string`、`secret`、`url` 三种字段，最多 12 项。未知字段类型、未知清单字段、不支持的版本或能力声明均拒绝。外部 ID 不得占用 `voiceport.` 前缀。程序必须是本机绝对路径的可执行普通文件，参数最多 32 项，每项最多 4096 字节；不用 shell，不做 eval。程序或参数变化需要用户审核；这不是可执行文件签名、沙箱或供应链验证系统。

插件具有当前用户权限，能够读写用户文件或联网。仅导入你信任、已审核的本地程序。适配器不得自行派生子进程、后台驻留或逃离进程组；VoicePort 为每次执行创建独立进程组，结束、失败、超时、取消时杀掉该组并后台 wait。恶意程序主动脱离进程组不在本协议支持范围内。

## JSONL stdin/stdout v1

每次执行一个请求，写完后关闭 stdin。程序在 `/` 工作目录运行，不继承 HOME、Codex/OpenAI 认证、代理或其他环境，仅设置 `LANG=C.UTF-8`。配置、凭据和音频仅通过 stdin 传入；不将凭据放进 argv、磁盘或日志。

请求为一行 UTF-8 JSON，末尾换行，最多 8,000,000 字节：

```json
{"protocol_version":1,"event":"transcribe","plugin_id":"example.adapter","plugin_version":"1.0.0","config":{"endpoint":"http://127.0.0.1:8000/v1/audio/transcriptions","model":"local-model","api_key":"selected-source-secret"},"language":"zh","audio":{"format":"wav_pcm16_mono_16000","base64":"..."}}
```

`language` 自动时为 `null`。`config` 仅含该来源声明字段，`secret` 在执行前从固定 Keychain 引用解析。音频最多 180 秒（5,760,044 字节 WAV）。停止后转换在工作线程进行，不运行于音频回调或 AppKit 线程。

程序必须退出成功，并返回恰好一行以换行结尾的 final 或 error：

```json
{"protocol_version":1,"event":"final","text":"识别结果；没有语音时可为空"}
```

```json
{"protocol_version":1,"event":"error","code":"transcription_failed"}
```

总 stdout/单行最多 1,000,000 字节，stderr 总计最多 65,536 字节，总运行时间最多 110 秒。不得在 stdout 输出日志、进度或多行响应。stderr 与 error 详情不直接回显，防止凭据泄露；已知秘密也会从返回文字中遮盖。第一版没有 streaming、partial、WebSocket 或多请求驻留能力，不能通过声明 `streaming:true` 开启。

## 实际加载参考适配器

参考程序使用系统 Python 标准库，将 JSONL 音频转成规定的 HTTP multipart 请求。它不包含 ASR 模型，仍需你自己的兼容服务。内置原生 HTTP 插件不需要 Python。

在仓库根目录执行以下命令，只生成一个带实际绝对脚本路径的清单，不运行适配器：

```sh
python3 -I crates/voiceport-macos/examples/plugins/prepare_manifest.py \
  --output /tmp/voiceport-reference-plugin.json
```

若输出已存在，生成器拒绝覆盖。查看生成的 JSON 和 [参考程序](../crates/voiceport-macos/examples/plugins/reference_adapter.py)，在「添加转写方式… → 导入本地插件…」中选择该文件并确认程序及参数。导入后，在定位到的「参考 HTTP 适配器（Python）」卡片点击「配置此插件…」，配置完整 endpoint、model 和独立 key，显式启用，选择该来源，再点击样例测试。仓库移动后需重新生成并确认路径；导入本身不验证服务可达性。

[模板清单](../crates/voiceport-macos/examples/plugins/reference-manifest.json) 内的 `/absolute/path/to/reference_adapter.py` 是占位符，使用上述生成器获得可实际加载的文件。参考适配器不会发起下载、派生其他进程或读取 Codex 登录。

## 配置、会话与资源边界

来源配置位于 `~/Library/Application Support/VoicePort/transcription-sources-v1.json`，版本为 1，以 0600 临时文件写入、fsync、同目录原子替换。配置文件上限 262,144 字节，清单上限 32,768 字节；最多 64 个外部插件、128 个配置实例。损坏或未来版本配置会显示不可用错误并阻止管理保存，不覆盖为默认配置。来源选择继续通过现有 `transcriptionSource` 偏好保存，缺失选择保留不可用。

可为单个 App 进程显式设置 `VOICEPORT_SOURCES_PATH=/absolute/test-directory/sources.json`，优先于上述默认来源配置路径。未设置时保留原默认路径和来源 ID；设置为空、相对路径、目录、符号链接或无法检查的路径会报错，不回落到用户默认配置。指定文件损坏、版本不支持、非普通文件或超限时仍使用原 Store 的拒绝规则；保存仍为 0600、fsync 和原子替换。路径可以尚不存在，由保存创建父目录。

此变量只改变来源配置位置，不改变 Orca 模型、Codex auth、系统 HOME/Keychain 定位或 NSUserDefaults。GUI 验证应保留正常系统 HOME，使用独立 bundle ID 隔离偏好、专用绝对配置路径和合成 CODEX_HOME，并关闭自动输入；不要通过改变 HOME 隔离来源配置。具体启动示例见 [自定义来源的隔离验证](CUSTOM_SOURCES.md#隔离-gui-验证)。

每次更新密钥创建独立 Keychain revision 引用；普通配置只含引用。录音开始固定插件版本、完整来源配置、语言、凭据引用及其值。执行和重试前确认原引用仍存在且值未替换，否则明确失败。busy 期间管理、来源切换和语言变更禁止；手动重试仍绑定原快照，且不自动投递到新焦点。

HTTP 强制远程 HTTPS，HTTP 只允许明确 loopback；拒绝 user/password、fragment，不跟随重定向、不重试、不使用系统代理、不关闭证书验证。连接 timeout 10 秒，总 timeout 110 秒，read timeout 30 秒，响应上限 1,000,000 字节。非 2xx 不显示服务正文。自定义来源不读取 Codex 登录，不带 ChatGPT 账号头，不调用系统 Speech 做第二来源预览。

取消时保持 Stopping/busy，直到同 generation 的后台任务释放录音 Arc 和子进程资源。UI 不 join 工作线程；stdout/stderr 采用同一后台线程上的非阻塞有界读取，无额外 reader 线程。保存、加载、发现和导入不联网、不执行插件。真实 GUI、真实麦克风和云端识别质量必须另外验证。
