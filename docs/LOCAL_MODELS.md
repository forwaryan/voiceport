# 本地语音模型

在偏好设置的「转写来源」旁点击 **语音模型…**，或从菜单栏打开同名入口。模型窗口会列出可下载的本地模型、语言、大小、安装状态和当前选择；未安装的模型也能看到。

| 模型 | 下载大小 | 适合的用途 | 识别方式 |
| --- | ---: | --- | --- |
| SenseVoice Small | 239.5 MB | 中文日常输入，也支持英语、日语、韩语、粤语 | 停止后在本机识别 |
| Whisper Large v3 Turbo | 1.04 GB | 多语言输入，可接受较大的下载与内存开销 | 停止后在本机分段识别 |
| Paraformer · 中英双语 | 237.2 MB | 需要边说边显示文字的中英输入 | 流式预览并在本机完成最终识别 |
| Whisper Tiny | 103.6 MB | 小下载、快速体验本地识别 | 停止后在本机分段识别 |

窗口顶部显示当前来源；「全部模型 / 已安装」可筛选列表。每行分别展示推荐用途、支持语言、模型大小和安装状态。推荐依据是模型语言覆盖与使用方式，未将这些描述当作同音频准确率或速度排名。

本地识别目前支持 **Apple Silicon / arm64 macOS**，音频留在本机，不读取云端登录。模型下载期间需要联网，识别期间无需联网。默认仍是 ChatGPT；下载或导入不会自动更改转写来源，点击「使用」后才切换。模型缺失或识别失败会报告错误，不会自动上传至云端。

## 下载、导入与选择

1. 点击模型行的「下载」。第一次会同时安装约 **22.1 MB** 的 sherpa-onnx 1.12.37 引擎，之后其他模型共用。也可以先点击右上角「安装识别引擎」，再导入已有文件。
2. 窗口显示当前文件、进度条、百分比和已下载大小；正在处理的模型行会标明状态。下载、复制与校验均在后台进行；「取消下载」会取消当前任务，清理未完成文件。关闭窗口时如有任务，会先请求取消并等待清理。
3. 安装完成后点击「使用」，关闭模型窗口即可听写。选择与语言保存在 VoicePort 自己的偏好中，不修改 Orca 设置。
4. 已有官方模型时点击该行「导入…」，选择包含下列文件的文件夹。VoicePort 复制并逐件校验大小与 SHA-256；不会执行该目录里的脚本，也不会修改原文件。
5. 「删除」只移除 VoicePort 的副本；当前所选模型须先切换到其他来源，再删除。删除不会改动 Orca 的模型。损坏的副本需先删除再重新下载，本版不做原位更新或断点续传。

导入本身不联网。如果本机没有兼容引擎，先通过「安装识别引擎」明确下载，再导入模型。首次下载模型后即使模型阶段被取消，已经完整安装的共享引擎仍会保留。

下载源为 Hugging Face 模型仓库、npm 官方 registry，以及 ONNX Runtime 官方许可文件。文件 URL 固定到上游 revision/版本；目录随应用内置，安装时验证精确大小和哈希。所有文件先写入 VoicePort 下的临时目录，校验完整后才发布为可发现的模型。失败或取消不会让半成品成为可选来源。不同 VoicePort 实例通过文件锁串行安装。

窗口打开期间会暂停录音、新识别、快捷键启动及其他来源编辑，避免切换、删除或安装与识别争用模型。关闭窗口后恢复。退出 App 会请求取消安装，等后台释放文件后退出。

## 存储与格式

VoicePort 独立存储，不要求安装 Orca：

```text
~/Library/Application Support/VoicePort/speech-models/
  runtime-1.12.37/
    package.json
    libsherpa-onnx-c-api.dylib
    libonnxruntime.1.23.2.dylib
    LICENSE-sherpa-onnx.txt
    LICENSE-ONNX-Runtime.txt
    ThirdPartyNotices-ONNX-Runtime.txt
  paraformer-bilingual-zh-en/
    encoder.int8.onnx
    decoder.int8.onnx
    tokens.txt
  whisper-tiny/
    tiny-encoder.int8.onnx
    tiny-decoder.int8.onnx
    tiny-tokens.txt
    MODEL_LICENSE.txt
    ATTRIBUTION.txt
  whisper-large-v3-turbo/
    turbo-encoder.int8.onnx
    turbo-decoder.int8.onnx
    turbo-tokens.txt
    MODEL_LICENSE.txt
    ATTRIBUTION.txt
  sensevoice/
    model.int8.onnx
    tokens.txt
    MODEL_LICENSE.txt
    ATTRIBUTION.txt
```

SenseVoice 安装时会写入应用携带的固定 FunASR 许可与来源说明；Whisper Tiny / Turbo 安装时会写入固定上游 MIT 完整版权与许可正文、来源说明。下载与离线导入均保留这些材料，无需额外联网获取。导入文件必须是普通文件；符号链接、FIFO 等特殊文件会被拒绝。

导入只接受上述四个模型对应的固定官方 ONNX 文件集；任意改名文件、GGML/GGUF、zip、其他精度或版本不会通过。模型大小与 Handy/Orca 当前显示的 GGUF 大小不同，因为这里使用 ONNX int8 模型。

兼容的既有 Orca 模型仍只读发现于 `~/Library/Application Support/orca/speech-models/`。可使用 VoicePort 自己的引擎；旧 Orca 的 `sherpa-onnx-darwin-arm64` 1.12.37 引擎存在时也可只读复用。Orca 近期改用 transcribe.cpp/GGUF，其新引擎和 SenseVoice GGUF 不能由当前 ONNX 桥直接加载。VoicePort 自有副本优先；若副本损坏，会明确显示不可用，不悄悄改用另一份文件。

保留原稳定来源 ID：`chatgpt`、`orca:paraformer-bilingual-zh-en`、`orca:whisper-tiny`；新增 `orca:sensevoice`、`orca:whisper-large-v3-turbo`。`orca:` 是历史 ID，不代表现在必须安装 Orca。保存的模型丢失时仍保留不可用选项，阻止启动，直到用户修复或更改来源。

## 参考与支持边界

模型管理参考 [Handy](https://github.com/cjpais/Handy/tree/aca8985190c3ad7f361d8a80cb30f179513ed690) 的离线目录、安装与选择分离、逐文件校验和暂存发布机制，以及 Orca 的模型列表交互。没有复制第三方应用代码或执行其安装脚本。

Paraformer、Whisper Tiny、SenseVoice 与 Whisper Large v3 Turbo 均接入正式识别链路。SenseVoice 适合中文为主的使用，Turbo 提供更大的多语言模型选项；较大权重不构成具体语音质量或延迟承诺，当前运行时使用 CPU。Nemotron 与 Parakeet 尚未接入，不在可下载列表中；它们需要各自模型格式、推理配置和验证，不能仅下载同名文件就视为支持。

固定下载清单、大小、哈希与运行时白名单见 [catalog.rs](../crates/voiceport-macos/src/model_library/catalog.rs)：

- Paraformer：`csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en`，revision `8e40c43232a1c5c66c82111efc5820d3accca11b`。转换仓库模型卡声明 Apache-2.0，并注明来源 ModelScope。
- Whisper Tiny：`csukuangfj/sherpa-onnx-whisper-tiny`，revision `65176e2deb88badc814a94058666cadccc29b61c`。Whisper 上游代码和权重采用 [OpenAI MIT License](https://github.com/openai/whisper/blob/main/LICENSE)；ONNX 转换仓库未单独提供许可文件，不将权重标为 sherpa 的 Apache-2.0。
- Whisper Large v3 Turbo：`csukuangfj/sherpa-onnx-whisper-turbo`，revision `2ca6ff69fc878651b770880507669577ac41c2ff`。上游 Whisper 代码和权重为 MIT；使用固定 int8 编码器、解码器和词表。
- SenseVoice：`csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17`，revision `2365baeacb507f821a0c8120fcee3d484dba7a07`。原始模型为 FunAudioLLM 的 SenseVoiceSmall，适用 [FunASR 模型许可](https://github.com/modelscope/FunASR/blob/main/MODEL_LICENSE) 自定义 v1.1 条款，包含模型名称、作者与来源保留要求；不能将其当作 MIT 或 Apache-2.0 权重。安装副本附带完整固定许可与归属说明。
- 引擎：npm `sherpa-onnx-darwin-arm64@1.12.37`，下载同时核验 SHA-256 和 registry SHA-512 integrity；仅提取固定的 package.json 与两件 dylib，不运行 npm、Node 或包脚本。安装目录携带 sherpa Apache-2.0、ONNX Runtime MIT 及其第三方 notices。

## 识别与权限

本地来源只申请麦克风权限，不创建系统 Speech 预览。自动输入仍需要辅助功能授权；关闭自动输入可将结果保留在面板。语言和来源在开始录音时固定；失败后的手动重试继续使用原来源与语言，并关闭自动输入。

Paraformer 始终为中英双语。SenseVoice 的自动模式覆盖中文、英语、日语、韩语和粤语；窗口可选择自动、中文或 English。Tiny 与 Turbo 使用 Whisper 自动检测或中文 / English 提示。离线模型最多保留 180 秒归一化音频，每段最长 30 秒，在末尾五秒低能量窗口优先切分；长语句在分段边缘可能丢词或重复，没有跨段上下文或语义去重。部分结果可修改当前片段；实时输入遵循已有前缀保护，最终改写可能只保留在面板。

现有单音频 tap 保留受保护的 AAC，并将 PCM 放入有界队列，在后台混为单声道、线性重采样为 16 kHz，再交给本地模型。支持 float32/64、int16/32，交错/非交错，1–8 声道、8–192 kHz。队列溢出或格式变化是显式失败，保留可用录音供手动重试。单次录音最长三分钟。

模型载入和一次原生解码不能中途强制打断，取消在其返回后生效。推理与 App 位于同一进程；底层库崩溃仍可能结束应用。C 桥核对实际 sherpa 版本并将常规 C++ 异常转为错误。本版不自动升级引擎或模型。

## 开发验证

GUI 隔离验证可为测试实例设置非空绝对路径 `VOICEPORT_MODELS_PATH`，将 VoicePort 自有模型与引擎存到专用目录；保留正常 HOME，再配合独立 bundle ID、`VOICEPORT_SOURCES_PATH`、合成 CODEX_HOME 与私有控制 socket。该变量不改变 Orca 模型目录。App 可用 `--show-models` 直接打开模型库。

普通测试不联网、不申请权限、不录音：

```sh
cargo test -p voiceport-macos --lib --offline --locked
```

以下显式测试会在临时目录实际下载约 126 MB 的引擎、许可和 Tiny 模型，校验、安装，再通过正式桥解码一秒合成静音。结束后临时目录释放，不更改已安装 App、来源选择或 Orca：

```sh
cargo test -p voiceport-macos --lib --offline --locked \
  official_download_installs_engine_and_model_and_decodes_silence -- --ignored --nocapture
```

`--offline` 约束 Cargo 依赖解析；该显式用例仍会发起上述下载。可通过 `VOICEPORT_TEST_MODEL=orca:sensevoice` 或 `VOICEPORT_TEST_MODEL=orca:whisper-large-v3-turbo` 选择新增模型；如需保留模型给隔离 GUI 测试，`VOICEPORT_TEST_RETAIN_ROOT` 必须是全新绝对目录，测试拒绝复用已有安装。

新增模型另有显式 `expanded_model_transcribes_pinned_public_fixture` 用例：通过 `VOICEPORT_TEST_MODELS_ROOT`、`VOICEPORT_TEST_MODEL` 和 `VOICEPORT_TEST_PUBLIC_WAV` 指定专用安装及固定公开样例，对比正式文件转写和队列输入路径。开发验证已用固定 Tiny revision 的 `test_wavs/0.wav`（212044 bytes，SHA-256 `6bc58a4efdf20daac252b6b1502632601a71efe0308f6757dc1eda34891a7e4f`）确认 SenseVoice 与 Turbo 均识别出预期英文关键词，且两路径结果相同。单个公开英文样例不代表五语言质量评估。

这些用例验证真实安装、引擎加载及指定公开音频的识别，不代表真人语音准确度、麦克风权限或目标输入验收。
