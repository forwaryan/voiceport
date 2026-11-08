# 本地语音模型

VoicePort 原生 macOS 设置页的「转写来源」一次选择一个来源。默认保持 ChatGPT；本地模型只在用户选择后使用，不会在模型丢失或失败时切换到云端。选择保存在 VoicePort 自己的 `transcriptionSource` 偏好中，不修改 Orca 设置。语言与来源在开始录音时固定；失败后的手动重试继续使用原来源、原语言，并关闭自动输入。

| 来源 | 录音期间 | 停止后 | 数据去向 |
| --- | --- | --- | --- |
| ChatGPT | 可选的 macOS 设备端预览 | ChatGPT 最终转写 | 上传至 ChatGPT，需自己的登录 |
| Paraformer · 本地流式 | 模型载入后持续返回累积的部分文字 | 排空音频、补齐尾音并完成本地最终结果 | 本机 CPU，不读云端登录，不联网 |
| Whisper Tiny · 本地离线 | 不产生实时预览 | 按最长 30 秒的音频片段逐段识别，合并最终结果 | 本机 CPU，不读云端登录，不联网 |

Paraformer 支持中英双语，语言下拉中的提示不会把双语模型限制成单语言。Whisper 支持自动检测或显式中文 / English 提示。Whisper 分段优先在片段末尾五秒内的低能量窗口切分，长语句在分段边缘仍可能丢词或重复词；这里没有语义去重或跨段上下文。实时识别的部分结果可以修改当前片段，终端输入仍遵循已有的前缀保护规则。停止后需要改写已输入文字的最终结果可能只保留在面板。

## 安装与发现

当前本地运行时仅支持 Apple Silicon / arm64，要求 Orca 安装在 `/Applications/Orca.app`，其原生 `sherpa-onnx-darwin-arm64` 包版本为 **1.12.37**。从 Finder 启动时使用固定绝对路径，不依赖 Node、nvm、终端 PATH 或 Orca CLI。VoicePort 不会自动下载模型或运行时。

运行时目录：

```text
/Applications/Orca.app/Contents/Resources/node_modules/sherpa-onnx-darwin-arm64/
  package.json
  libsherpa-onnx-c-api.dylib
  libonnxruntime.1.23.2.dylib
```

通过 Orca 的模型管理安装相应模型；VoicePort 仅读取以下目录：

```text
~/Library/Application Support/orca/speech-models/
  paraformer-bilingual-zh-en/
    encoder.int8.onnx
    decoder.int8.onnx
    tokens.txt
  whisper-tiny/
    tiny-encoder.int8.onnx
    tiny-decoder.int8.onnx
    tiny-tokens.txt
```

本版 Whisper 使用实际已安装的 int8 模型对；仅有非量化文件时不会将其列为可用。来源 ID 分别为 `chatgpt`、`orca:paraformer-bilingual-zh-en` 和 `orca:whisper-tiny`。

启动时在后台检查文件可读、非空、运行时版本与 arm64 Mach-O 架构，不加载大模型。设置中的「刷新」可重新发现刚安装的模型。未安装的本地来源不会显示为可用；曾选中的模型即使丢失，也会保留为「不可用」，显示错误且阻止该次启动，直到用户刷新或更改选择。运行时二进制加载失败、版本变化、无效模型或解码失败同样显示为本地错误，不会上传录音。

## 录音、线程和权限

本地来源仅申请麦克风权限，不创建 macOS Speech 预览，也不申请系统语音识别权限。自动输入仍需要辅助功能授权；关闭自动输入可只把结果保留在面板。显式检查权限不会录音。

现有单个音频 tap 同时保留受保护的 AAC 录音，并复制原始 PCM 到有界队列。支持 float32 / float64 / int16 / int32、交错或非交错、1–8 声道和 8–192 kHz（包括小数采样率）。输入先做边界和非有限值检查，在工作线程混成单声道并持续线性重采样到固定 16 kHz，之后才进入本地模型。线性重采样的质量与专用高阶滤波器存在差异。

队列最多 128 包，每包最多 8192 帧、8 声道，最坏 PCM 队列约 32 MiB；通常 1024 帧回调会小得多。队列溢出是显式失败，保留可用的 AAC 录音供手动本地重试。设备格式变化或无效 PCM 使录音失败。Whisper 的归一化音频最多保留 180 秒（约 11 MiB），每次原生离线解码最多 30 秒；单次录音仍最多三分钟。

载入、推理和重试读音频均在工作线程；界面和实时音频回调不载入或推理模型。部分结果只保留最新一份，带录音代次，端点确认的片段与当前部分文字组成完整预览。停止后先移除 tap、关闭 AAC，再排空已排队 PCM、冲刷重采样余量、为 Paraformer 加 0.8 秒静音并通知输入结束，最后发布完整最终结果。界面不会阻塞等待线程 join。停止期间仍可取消；取消 / 退出等待当前工作释放录音租约后才允许新的会话或退出。

C ABI 桥将常规 C++ 推理异常转成可显示的错误。模型加载与单个原生解码调用不能在中途强制打断；取消会在其返回后生效。原生运行时与应用处于同一进程，底层库崩溃或恶意/损坏的 ONNX 文件可能终止应用；只使用 Orca 安装的可信模型，并避免录音期间更新或删除运行时。模型与运行时版本升级需要重新检查支持的 ABI。本地依赖缺失时 ChatGPT 仍可由用户显式选择。

录音仍使用描述符相对路径、私有权限和租约清理策略。成功、取消或正常退出会释放；失败可保留供重试。VoicePort 不改写模型目录、不保存转写历史、不读取 Orca 云端密钥。本地推理不发起网络请求。

## 构建与开发自检

Cargo 自动使用 macOS Command Line Tools 的 clang / ar 编译小型 C 桥，使用仓库中精确版本的 C ABI 头文件；没有新增 Cargo 依赖、JS UI、辅助进程或需要打包的动态库。标准 `scripts/package_app.sh` 构建的主程序已包含本地实现，实际运行时从上述 Orca 目录动态加载。模型及原生运行时仍属于外部安装依赖，不复制入包。来源、版权和 Apache-2.0 全文见 `crates/voiceport-macos/src/local_runtime/` 与顶层第三方声明。

常规测试只使用惰性构造、临时目录和合成 PCM，不会申请权限或录制麦克风：

```sh
CARGO_TARGET_DIR=/tmp/voiceport-local-models-check CARGO_NET_OFFLINE=true \
  cargo test -p voiceport-macos --lib --locked
```

已安装上述模型时，可显式执行附带公共 WAV 的真实推理开发自检：

```sh
CARGO_TARGET_DIR=/tmp/voiceport-local-models-check CARGO_NET_OFFLINE=true \
  cargo test -p voiceport-macos --lib --locked \
  installed_models_transcribe_public_fixture -- --ignored --nocapture
```

该用例通过正式音频读取、归一化、本地推理和实时队列路径验证 16 kHz / 8 kHz 公共样本、停止时的完整排空、端点段落累积、静音及 Whisper 无部分结果。它不读取用户录音、登录或剪贴板，不操作输入目标。它属于开发自检，不能代表真实麦克风、Finder/TCC 权限、实际目标接收、所有语言、最长录音识别质量或独立验收。
