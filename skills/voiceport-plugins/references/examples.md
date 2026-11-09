# 三个接入例子

先选路线。下面的域名、端口和模型名是示例值，不能据此认定某家服务可用。把真实服务文档和本指南交给 AI，就能让它输出你的配置或适配器；密钥在 VoicePort 表单中填写，不发给 AI。

已安装的 App 无需拉取源码：用其 `Contents/MacOS/voiceport skills get plugins` 获取接入 Skill，`skills get plugin-examples` 获取本文，`skills get plugin-protocol` 获取当前协议。若命令不在 PATH，使用该 App 的 CLI 完整路径。完整 Skill 目录也随 App 放在 `Contents/Resources/skills/voiceport-plugins`；可手动复制整个目录到 Agent 支持的 Skill 目录，先检查已有安装，不自动覆盖。

## 例一：远程兼容转写 API，不用导入插件

假设文档明确说明：

- `POST https://api.example.com/v1/audio/transcriptions`。
- multipart 字段 `file`（WAV）、`model`、`response_format=json`；可选 `language=zh/en`。
- 返回 `{"text":"识别文字"}`；API Key 使用 Bearer 鉴权。
- 示例模型 ID 为 `whisper-1`；实际以服务文档为准。

可以这样向 AI 提需求：

> 用 VoicePort 插件接入 Skill 帮我配置这个服务。先阅读我提供的音频转写文档，核对内置兼容 API 是否足够，列出完整地址、模型和界面填写步骤。没有确认的字段标出来。不要让我上传 JSON，也不要读取密钥或直接录音。

应得到的交付：

| 界面项 | 示例填写值 |
| --- | --- |
| 路线 | 添加转写方式… → 连接兼容 API… |
| 来源名称 | 我的云端转写 |
| 完整转写接口地址 | `https://api.example.com/v1/audio/transcriptions` |
| 模型 ID | `whisper-1` |
| API Key | 用户自行在表单填写 |
| 导入文件 | 无需文件 |

勾选启用并保存，回听写页选择“我的云端转写”。保存不会发请求；样例测试需另外确认发送目标。若服务要求自定义签名头、JSON 音频或返回别的文字字段，就不满足本例的兼容条件，转到例三。

## 例二：本机 Whisper 兼容服务，也不用导入插件

前提：用户已经按对应项目文档启动本机服务，它实际支持上例的 multipart 请求及 `text` 响应。仅下载了模型权重还不满足条件。本例不安装或启动第三方服务，也不猜某个项目的启动参数。

例如，本机服务文档给出的完整地址为 `http://127.0.0.1:8000/v1/audio/transcriptions`，模型 ID 为 `local-whisper`，无需鉴权：

| 界面项 | 示例填写值 |
| --- | --- |
| 路线 | 添加转写方式… → 连接兼容 API… |
| 来源名称 | 我的本地 Whisper |
| 完整转写接口地址 | `http://127.0.0.1:8000/v1/audio/transcriptions` |
| 模型 ID | `local-whisper` |
| API Key | 留空；仅在服务明确无需鉴权时 |
| 导入文件 | 无需文件 |

向 AI 提需求的例子：

> 这是我本机转写服务的项目文档、启动配置和接口说明。请用 VoicePort Skill 判断是否兼容，给出实际填写表。不要把 GitHub 链接当插件导入；如果只有命令行工具，说明还缺什么，不要虚构一个端口。

用户请求 CLI 配置时，先检查目标 App 的 status、插件及现有来源。下面只适用于用户控制、具有上述契约的示例服务；`local-whisper` 来源 ID 应先确认未占用：

```sh
voiceport --json sources add local-whisper --plugin voiceport.http-transcription --name "我的本地 Whisper" --enabled true --set endpoint=http://127.0.0.1:8000/v1/audio/transcriptions --set model=local-whisper
```

配置成功后可按用户要求执行 `sources select local-whisper`。若使用隔离 App，每条命令都需加该实例的 `--socket`。普通局域网 IP 的明文 HTTP 不支持；远程服务须使用 HTTPS。不要为使示例通过而关闭证书验证。

## 例三：自有 JSON 音频接口，需要适配器和清单

**这是教学接口契约，不代表已支持任何真实供应商。** 假设你控制的服务只接受下面的协议：

```json
{"model":"local-json-model","audio_base64":"完整 WAV 的 base64","language":"zh"}
```

`POST` 完整 endpoint，`Content-Type: application/json`；自动语言时省略 `language`。成功响应为：

```json
{"result":{"transcript":"识别文字"}}
```

它与内置 multipart/`text` 协议不兼容。所附 `templates/json_http_adapter.py` 将 VoicePort stdin 的 WAV/base64 转成该 JSON，再把 `result.transcript` 转为 JSONL final。它不识别语音，仍需一个实际提供 ASR 的对应服务。请求格式、鉴权头、结果取值三处标了修改位置；真实服务契约不同时，必须先修改并测试，不能只换 endpoint。

向 AI 提需求的例子：

> 这是我的批量 ASR 接口文档。请用 VoicePort 插件 Skill，按 JSONL v1 编写适配器和本机绝对路径清单。沿用示例的输入/响应上限、无重试、无重定向和凭据隔离；只按文档改请求、鉴权和响应取值。给我实际文件路径、GUI 导入步骤，并用 localhost 模拟接口和合成 WAV 测试。不要读取真实 Key 或发送真实录音。

### 生成一份可以审阅的示例

需要可用的 Python 3；内置兼容 API 路线不需要 Python。选择实际 Skill 目录；以下假定 App 在 `/Applications/VoicePort.app`。源码用户可把 `SKILL_DIR` 改成仓库下 `skills/voiceport-plugins` 的绝对路径，Agent 手动安装的 Skill 也可以用它自身目录。

```sh
SKILL_DIR="/Applications/VoicePort.app/Contents/Resources/skills/voiceport-plugins"
python3 -I "$SKILL_DIR/scripts/prepare_example.py" --directory "$HOME/VoicePort-JSON-example"
```

生成器只在**不存在**的新目录中创建 `adapter.py` 和 `plugin.json`，不运行/导入适配器、不联网、不安装依赖、不存密钥。父目录必须存在；目录已存在时拒绝，选择新的目录名。返回 JSON 中的 `manifest` 是下一步应选的实际绝对路径。程序字段使用运行生成器的 Python 的真实绝对路径，`args` 包含 `-I` 和刚复制的脚本绝对路径。生成后保持目录位置；移动文件需重新生成/审阅清单。Python 解释器必须持续可用。

先审阅这两个文件及程序权限。示例 ID 固定为 `example.json-http`，只能导入一次；做自己的插件时改为自己独立的稳定 ID、名称和 origin，并按真实服务调整代码和字段。它没有自动安装和更新机制。

### 导入后仍要配置来源

1. 打开设置 → 管理来源… → 添加转写方式… → 导入本地插件…，选择生成器返回的 `plugin.json`。
2. 核对清单来源、插件 ID、Python 路径和脚本参数。接受后仅保存清单，不执行程序。
3. 在定位到的“示例 JSON 音频接口（需对应服务）”卡片点击“配置此插件…”。
4. 填来源名称、你实际运行的 JSON ASR 完整 endpoint、实际模型 ID；需要 Bearer 时由用户填写 API Key。不要使用例一/例二的 multipart endpoint。
5. 显式启用并保存，回听写页选择该来源。

CLI 同样先 `plugins import /实际绝对路径/plugin.json` 查看程序，接受后才加 `--accept-program`；再用 `sources add` 绑定 `--plugin example.json-http`，而不是把插件 ID 当成已经存在的来源 ID。完整参数以 `voiceport skills get cli` 为准。

### 如何报告测试结果

先验证清单和文件路径，再以受控 localhost 服务和合成 WAV 验证请求转换、返回格式及失败处理。模拟服务返回的固定文字必须标为模拟结果，不能声称识别成功。App 的样例测试发送一秒静音，只有目标和发送动作获准时才提交；获得 operation ID 后用 result 查询，不能反复提交测试来轮询。

可以报告“文件已生成、已导入、来源已保存、合成接口测试通过”，但每一项须确实完成。未做真实服务测试时，明确仍需用户验证服务连接与识别质量。
