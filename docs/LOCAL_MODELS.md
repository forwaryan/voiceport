# 本地语音模型

在偏好设置的「转写来源」旁点击 **语音模型…**，或从菜单栏打开同名入口。「这台电脑」列出本地模型；「云端」配置 GPT Transcribe 或 OpenAI-compatible 服务，详见 [云端配置](CUSTOM_SOURCES.md#模型窗口中的云端配置)。

可以按支持的语言和预览方式筛选，按模型下载量从小到大或从大到小排序。「所有语言」是浏览筛选，不会改动右上角的「听写语言」。下载量不代表速度或识别质量；「已安装」与「使用中」分别表示模型可在本机找到和当前所用来源。

「本地推荐」按「听写语言」和所选预览方式，从固定 ONNX 目录中选下载量最小的兼容模型。大小相同时按稳定来源 ID 排序。建议同时显示模型的预览方式、下载量及安装状态。对于 VoicePort 已安装的自有副本，若模型文件或共享引擎有问题，推荐显示「当前不可用」，不直接建议重装。浏览语言、「已安装」和列表排序只影响下方列表。选择自动听写时，各模型仍按自身语言范围识别。中文或英语选择实时预览时建议 Paraformer，选择「全部预览方式」时建议 Whisper Tiny。粤语选择「全部预览方式」时建议 SenseVoice Small，选择实时预览时没有兼容模型。打开窗口、查看建议或改变浏览筛选均只读。要安装或切换，请在模型行明确点击「下载」或「使用」。CLI 查询见 [本地模型命令](../skills/voiceport/references/cli.md#local-models)。

| 模型 | 模型下载大小 | 支持范围 | 识别方式 |
| --- | ---: | --- | --- |
| SenseVoice Small | 239.5 MB | 中文、英语、日语、韩语、粤语 | 停止后在本机分段识别 |
| Whisper Large v3 Turbo | 1.04 GB | 100 种语言，含粤语 | 停止后在本机分段识别 |
| Paraformer · 中英双语 | 237.2 MB | 中文、英语 | 流式预览并在本机完成最终识别 |
| Whisper Tiny | 103.6 MB | 99 种语言，不含粤语 | 停止后在本机分段识别 |
| Parakeet TDT v3 | 670.6 MB | 25 种欧洲语言，自动识别 | 停止后在本机分段识别 |
| Parakeet TDT v2 | 661.3 MB | 英语 | 停止后在本机分段识别 |
| Parakeet Unified EN | 663.0 MB | 英语 | 流式预览，560 ms 缓冲上下文 |
| Nemotron 3.5 | 682.2 MB | 28 个可选择语言代码，包含中、英、日、韩 | 多语言流式预览，560 ms 缓存上下文 |

大小为固定 ONNX int8 文件集的十进制下载量，另计共享识别引擎约 **10.0 MB**。560 ms 是模型上下文配置，实际 CPU 识别延迟取决于设备。窗口质量统一显示「未测量」；没有同音频准确率或速度排名。语言选择不兼容时显示「不支持所选语言」，并在准备录音前拒绝启动。

本地识别支持 **Apple Silicon / arm64 macOS**。音频留在本机，不读取云端登录；下载需要联网，识别无需联网。下载、导入和更新不会自动更改转写来源，点击「使用」后才切换。模型缺失或识别失败会报告错误，不会自动上传云端。

安装后可关闭 App，用 `voiceport transcribe-local AUDIO --model ID` 转写已有文件。命令复用下文的模型/引擎发现和文件校验，不读取当前来源选择，不触发安装或下载。默认语言为 `auto`；需要显式语言时传 `--language CODE`。命令在加载前按固定发行物校验两份引擎库的字节数和 SHA-256，再从本次运行的私有副本加载。兼容 2.0.1 和 1.12.37 的原始发行库。缺少模型或引擎时，先在「语音模型」安装，再运行文件命令。若库与发行物不符，可先移出该引擎目录再安装，保留已有模型。支持范围与回执见 [独立本地文件转写](../skills/voiceport/references/cli.md#standalone-local-file-transcription)。

## 下载、导入、更新与删除

1. 在「全部模型」选择「下载」。首次会安装 sherpa-onnx **2.0.1** 共享引擎，也可先点击「安装识别引擎」，再离线导入模型。
2. 窗口显示当前文件、进度和已下载大小。下载失败时会显示文件名及原因。「取消下载」会取消当前任务并清理暂存文件。关闭窗口时若有任务，先请求取消并等待后台清理。
3. 安装完成后点击「使用」。来源和语言保存在 VoicePort 偏好中，不修改 Orca 设置。「已安装」可筛选本机已有的来源。
4. 「导入…」选择固定 ONNX 文件集所在文件夹。VoicePort 复制并核验每件文件的大小和 SHA-256；不执行其中脚本，不修改原文件。导入本身不联网；缺少兼容引擎时先明确安装引擎。
5. 「旧副本可用 · 可更新」表示权重仍可使用，但缺少当前安装回执或完整随附资料。可以继续「使用」，或点「更新」。当前自有副本也提供「重装」；不可用副本提供「更新 / 重装」。
6. 更新先在独立暂存目录完成下载、校验和许可写入，再用原子目录交换替换旧副本。失败或取消保留旧副本，安装期间不会发现半成品。升级不会改动 Orca 的文件。
7. 「删除」只移除 VoicePort 副本。当前所选模型须先切换到其他来源；保存的来源丢失后保留不可用选择，直到用户修复或主动更改，不自动回退。
8. 「详情」展示稳定来源 ID、语言、固定文件大小、完整 SHA-256、带 revision 的上游地址及许可/归属材料。

首次安装模型后即使模型阶段被取消，已经完整安装的共享引擎仍会保留。本版不做断点续传，也不自动后台升级。不同 VoicePort 实例通过文件锁串行安装。窗口打开期间暂停录音、快捷键启动和其他来源编辑，避免安装、删除与识别争用文件。退出 App 会请求取消安装，等后台释放文件后退出。

## 存储与格式

VoicePort 独立存储，不要求安装 Orca：

```text
~/Library/Application Support/VoicePort/speech-models/
  runtime-2.0.1/
    package.json
    libsherpa-onnx-c-api.dylib
    libonnxruntime.dylib
    LICENSE-sherpa-onnx.txt
    LICENSE-ONNX-Runtime.txt
    ThirdPartyNotices-ONNX-Runtime.txt
  paraformer-bilingual-zh-en/
  whisper-tiny/
  whisper-large-v3-turbo/
  sensevoice/
  parakeet-tdt-v3/
  parakeet-tdt-v2/
  parakeet-unified-en/
  nemotron-3.5/
```

每件模型的文件名以 [固定目录](../crates/voiceport-macos/src/model_library/catalog.rs) 和窗口「详情」为准。四种新增模型均为 `encoder.int8.onnx`、`decoder.int8.onnx`、`joiner.int8.onnx`、`tokens.txt`。新安装副本写入 `VOICEPORT_MODEL.json`；有权重许可与归属要求的模型同时保留 `MODEL_LICENSE.txt`、`ATTRIBUTION.txt` 以及适用的 `Notice.txt`。下载和离线导入使用相同的资料，不需要另行联网获取。

只接受目录中固定 revision 对应的 ONNX 文件集。任意改名文件、GGML/GGUF、zip、其他精度或版本不会通过。导入拒绝符号链接、FIFO 等特殊文件，打开后再次核验实际 descriptor 类型。

旧 `runtime-1.12.37` 可继续供原四模型和 TDT 模型使用；Unified EN 与 Nemotron 3.5 必须使用 2.0.1。安装引擎入口检查当前 2.0.1，不会因存在旧引擎而隐藏升级入口。C 桥针对两个版本分别使用其原始 C 头文件和配置布局，核对实际加载版本。

兼容的既有 Orca 模型仍只读发现于 `~/Library/Application Support/orca/speech-models/`，并可只读复用其旧 sherpa 1.12.37 引擎。Orca 新的 transcribe.cpp/GGUF 模型不能由此 ONNX 桥加载。VoicePort 自有副本优先；自有副本损坏时明确显示不可用，不悄悄改用另一份文件。

稳定来源 ID 保留 `chatgpt`、`orca:paraformer-bilingual-zh-en`、`orca:whisper-tiny`、`orca:sensevoice`、`orca:whisper-large-v3-turbo`，新增 `orca:parakeet-tdt-v3`、`orca:parakeet-tdt-v2`、`orca:parakeet-unified-en`、`orca:nemotron-3.5`。`orca:` 是历史 ID，不表示必须安装 Orca。

## 固定来源与许可

模型管理参考 [Handy](https://github.com/cjpais/Handy/tree/aca8985190c3ad7f361d8a80cb30f179513ed690) 的离线目录、安装与选择分离、逐文件校验和暂存发布机制，以及 Orca 的模型列表交互。没有执行第三方安装脚本。

下列转换仓库由 sherpa 维护者提供，原始模型作者另列。完整逐文件 SHA-256、大小、固定 URL 见 [catalog.rs](../crates/voiceport-macos/src/model_library/catalog.rs)，不从浮动 `main` 下载。

| 模型 | ONNX 转换仓库及固定 revision | 权重许可 |
| --- | --- | --- |
| Paraformer | `csukuangfj/sherpa-onnx-streaming-paraformer-bilingual-zh-en` · `8e40c43232a1c5c66c82111efc5820d3accca11b` | 转换模型卡声明 Apache-2.0，注明来源 ModelScope |
| Whisper Tiny | `csukuangfj/sherpa-onnx-whisper-tiny` · `65176e2deb88badc814a94058666cadccc29b61c` | OpenAI Whisper MIT |
| Whisper Large v3 Turbo | `csukuangfj/sherpa-onnx-whisper-turbo` · `2ca6ff69fc878651b770880507669577ac41c2ff` | OpenAI Whisper MIT |
| SenseVoice | `csukuangfj/sherpa-onnx-sense-voice-zh-en-ja-ko-yue-2024-07-17` · `2365baeacb507f821a0c8120fcee3d484dba7a07` | FunAudioLLM SenseVoiceSmall：FunASR 自定义模型许可 v1.1 |
| Parakeet TDT v3 | `csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8` · `ed5d745623472f97cfc2e6c2916ee248798fc5a8` | NVIDIA 原模型 CC BY 4.0 |
| Parakeet TDT v2 | `csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8` · `84b478d541cdbf0c296a7ab5f1e20a6f8f998114` | NVIDIA 原模型 CC BY 4.0 |
| Parakeet Unified EN | `csukuangfj2/sherpa-onnx-nemo-parakeet-unified-en-0.6b-int8-streaming-560ms` · `7551fd26fc810cc1e4e043e608db4d13b59be31e` | NVIDIA Open Model License Agreement（2025-10-24） |
| Nemotron 3.5 | `csukuangfj2/sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-560ms-int8-2026-06-11` · `ab43d895f5985b1bbab8b6eac8607fcdc05343f3` | OpenMDW License Agreement v1.1 |

Nemotron 的原始模型准确对应 [`nvidia/nemotron-3.5-asr-streaming-0.6b`](https://huggingface.co/nvidia/nemotron-3.5-asr-streaming-0.6b/tree/ea30d66debe3740a08b573244286791d423d6b3e)，没有用旧的仅英语 Nemotron 替代。Parakeet 原始模型分别是 `nvidia/parakeet-tdt-0.6b-v3`、`nvidia/parakeet-tdt-0.6b-v2`、`nvidia/parakeet-unified-en-0.6b`；原模型 revision、完整许可和 ONNX 导出/量化修改说明随副本保留。

引擎为 npm `sherpa-onnx-darwin-arm64@2.0.1`，实际版本来源 git `af2d8d81dbfa61c4cea4c568da78f3982edca005`。归档下载量为 10,013,200 bytes，同时核验 SHA-256 与 registry SHA-512 integrity。安装只提取白名单内的 `package.json` 和两个 dylib。应用随附 sherpa Apache-2.0 许可、ONNX Runtime 1.28.2 MIT 许可及第三方声明，安装时写入引擎目录。

## 语言、识别与权限

语言菜单与 CLI 使用同一稳定代码表。每种模型有独立支持集，菜单可选不代表每个模型都支持。自动模式使用模型自身检测；仅英语模型的自动模式仍按英语识别。

- Paraformer：自动、`zh`、`en`。
- SenseVoice：自动、`zh`、`en`、`ja`、`ko`、`yue`。
- Whisper Tiny：99 种 Whisper 语言；Turbo：100 种，增加 `yue`。显式语言作为 Whisper 解码提示。
- Parakeet TDT v3：`bg`、`hr`、`cs`、`da`、`nl`、`en`、`et`、`fi`、`fr`、`de`、`el`、`hu`、`it`、`lv`、`lt`、`mt`、`pl`、`pt`、`ro`、`ru`、`sk`、`sl`、`es`、`sv`、`uk`；模型自身自动识别，选择代码用于兼容性校验。
- TDT v2 与 Unified EN：自动或 `en`。
- Nemotron 3.5：`en`、`es`、`fr`、`it`、`pt`、`nl`、`de`、`tr`、`ru`、`ar`、`hi`、`ja`、`ko`、`vi`、`uk`、`pl`、`sv`、`cs`、`no`、`da`、`bg`、`fi`、`hr`、`sk`、`zh`、`hu`、`ro`、`et`，或自动。上游区域变体归并成稳定代码；八个仅供 adaptation/fine-tuning 的语言区域不计为开箱支持。

本地来源只申请麦克风权限，不创建系统 Speech 预览。自动输入仍需要辅助功能授权；关闭自动输入可将结果保留在面板。语言和来源在开始录音时固定；失败后的手动重试继续使用原快照，并关闭自动输入。

离线模型最多保留 180 秒归一化音频，每段最长 30 秒，在末尾五秒低能量窗口优先切分；长语句在分段边缘可能丢词或重复，没有跨段上下文或语义去重。流式模型在后台消费有界 PCM 队列，结束时补齐尾部上下文再获取最终文字；实时前缀投递继续遵循已有保护。单次录音最长三分钟。

任一时刻只有一个音频 tap，保留受保护 AAC 并将 PCM 放入有界队列，在后台混为单声道、重采样到 16 kHz。支持 float32/64、int16/32，交错/非交错，1–8 声道、8–192 kHz。[麦克风恢复](MICROPHONE.md#断连恢复)成功时，新 tap 会转换为本轮原始采集格式，再交给同一录音与识别流程。队列溢出时只在 AAC 已完整写入并验证的情况下保留录音供手动重试；无法确认设备或格式的录音不提交。

模型载入和一次原生解码不能中途强制打断，取消在返回后生效。推理与 App 位于同一进程；底层库崩溃仍可能结束应用。C 桥核对版本并将常规 C++ 异常转为错误。

## 开发验证

GUI 隔离验证为测试实例设置非空绝对 `VOICEPORT_MODELS_PATH`，将自有模型与引擎存到专用目录；保留正常 HOME，配合独立 bundle ID、`VOICEPORT_SOURCES_PATH`、合成 CODEX_HOME 和私有控制 socket。模型 override 不改变 Orca 模型目录。App 可用 `--show-models` 直接打开模型库。

普通测试不联网、不申请权限、不录音：

```sh
cargo test -p voiceport-macos --lib --offline --locked
```

显式 `official_download_installs_engine_and_model_and_decodes_silence` 用例下载固定引擎和模型后，通过生产桥解码合成静音。`VOICEPORT_TEST_MODEL` 指定目录中的稳定来源 ID；`VOICEPORT_TEST_RETAIN_ROOT` 如需保留安装必须是全新绝对目录，测试拒绝复用已有目标。`--offline` 约束 Cargo 依赖解析，此显式测试本身仍会下载。

`expanded_model_transcribes_pinned_public_fixture` 通过 `VOICEPORT_TEST_MODELS_ROOT`、`VOICEPORT_TEST_MODEL`、`VOICEPORT_TEST_PUBLIC_WAV` 指定专用安装和固定公开样例，对比正式文件转写与队列输入路径。公开英文 fixture 为固定 Tiny revision 的 `test_wavs/0.wav`（212,044 bytes，SHA-256 `6bc58a4efdf20daac252b6b1502632601a71efe0308f6757dc1eda34891a7e4f`）。单个英文样例不代表全部语言的准确率或跨模型质量评估；合成静音只证明协议与执行链。

2026-11-17 开发自检已对上述固定英文样例实际运行四个新增模型：Parakeet TDT v3、TDT v2、Unified EN、Nemotron 3.5 均产生非空转写，正式文件与队列输入的最终文字一致；Unified EN 与 Nemotron 也产生了非空实时预览。安装使用目录中固定的 ONNX 文件与摘要校验。同次兼容性自检也在 2.0.1 引擎下运行了原有 SenseVoice、Whisper Tiny、Whisper Large v3 Turbo 和 Paraformer，四种模型均产生非空转写且文件与队列路径一致。该单样例结果只证明这些执行路径可用，不提供准确率排名或全语言保证。

开发自检、不同会话的独立复核、GUI 操作、固定公开音频解码和真实麦克风/目标接收应分别记录。普通测试不外推真实录音、用户音频上传或生产 App 验收。
