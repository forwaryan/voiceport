# QQ「语音输入文字」接入调查

调查日期：2026-11-10。状态：**未接通，没有新增可用的 QQ 转写来源。**

目标是复用 QQ 输入框工具栏的「语音输入文字」：VoicePort 提供录音，QQ 的识别能力返回文字。已有 QQ 语音消息的转文字接口不能替代这个目标。

## 与当前 ChatGPT 来源的区别

当前 ChatGPT 来源由 VoicePort 自己录音，停录后读取 Codex 文件登录的 access token 和 account ID，通过 multipart 表单将音频提交到 `https://chatgpt.com/backend-api/transcribe`，读取响应的 `text`。录音时的可选实时预览来自 macOS 设备端 Speech。实现见 `crates/voiceport-core/src/client.rs`、`auth.rs` 和 `crates/voiceport-macos/src/controller.rs`。

QQ 目前没有经过验证的对应外部调用链。找到内部方法名称不代表已确定接口，也不能据此注册一个声称可用的来源。

## 已确认的公开源码

- NapCat 的桌面 session 声明 `getBdhUploadService(): unknown`，没有给出 SmartPtt 参数类型：[wrapper.ts](https://github.com/NapNeko/NapCatQQ/blob/98d781d19815d1625b24d717ce280ecf0a4f6d0c/packages/napcat-core/wrapper.ts#L260)。
- Yui 将 `getBdhUploadService(...args)` 转发到 native session，但参数为 `any[]`：[NodeIQQNTWrapperSession.ts](https://github.com/project-yui/Yui/blob/7ab2ddaad71020e1e366e9ef984b1f88180e88b8/src/wrapper/exports/NodeIQQNTWrapperSession.ts#L173-L184)。其 [BDH wrapper](https://github.com/project-yui/Yui/blob/7ab2ddaad71020e1e366e9ef984b1f88180e88b8/src/wrapper/exports/NodeIKernelBdhUploadService.ts) 没有声明 SmartPtt 方法。
- 一个公开桥接实现尝试注册 `addKernelBdhUploadListener(...)`；调用在空 `catch` 内，不能证明注册成功或兼容本机 QQ：[bridge.js](https://github.com/Micuks/waylay-qq-bridge/blob/3dd3572a4e34f5b2f91ad6d645a16fe6cc34eadb/src/bridge.js#L410-L415)。
- NapCat [`fetch_ptt_text`](https://github.com/NapNeko/NapCatQQ/blob/98d781d19815d1625b24d717ce280ecf0a4f6d0c/packages/napcat-onebot/action/msg/FetchPttText.ts) 接收 `message_id`，获取已有语音消息并调用 `translatePtt2Text`。它不是任意录音上传接口。

以上源码均固定到调查时的提交，不构成对本机版本兼容性的验证。定向公开搜索未找到 SmartPtt 的完整调用契约；这不证明其他公开材料一定不存在。

## 本机程序资源的静态证据

本机 `/Applications/QQ.app/Contents/Resources/app/package.json` 标识 QQ `7.0.1-52892`，Darwin、App Store、universal、bytecode shell。

对 QQ 安装目录中 `wrapper.node` 的只读字符串检查发现：

```text
uploadSmartPttSlice
uploadSmartPttSlice: task_id={} slice_index={} is_end={} file_path={}
assertion (argc == 2) failed: NodeIKernelBdhUploadService::uploadSmartPttSlice needs 2 arguments
onSmartPttUpdateText
OnSmartPttUpdateText: task_id={} text_len={} seg_finished={}
onSmartPttTranslateFinished
OnSmartPttTranslateFinished: task_id={} error_code={} err_msg={}
```

同一绑定区还包含 `addKernelBdhUploadListener` 和 `removeKernelBdhUploadListener`，各自的参数数量断言为 1。

这些是静态绑定和日志字符串：可以确认方法名称及参数数量断言，**不能确认两个参数的类型、属性、顺序或语义，也不能将日志的 snake_case 名称当作 JavaScript 属性名**。

在本机 `application.asar` 原始字节中搜索 `SmartPtt`、`smartPtt`、`smart_ptt`、`uploadSmart`、`KVOICETOTEXT` 和 UTF-8「语音输入文字」均无匹配；安装目录外置 JS 的相同 SmartPtt 关键词搜索也无匹配。arm64 `nm` 输出没有匹配的 SmartPtt 符号。因此本轮没有取得按钮到 native 方法的可读调用点。

## GitHub 上进一步找到的 Android 实现

公开仓库不仅包含方法名，还保存了 Android QQ 的反编译上传实现：

- [`PttSliceUploadProcessor.java`](https://github.com/tsuzcx/qq_apk/blob/afe46ef5640d0ba6850cdefd3c11badbd725a3f6/com.tencent.mobileqq/classes.jar/com/tencent/mobileqq/transfile/PttSliceUploadProcessor.java#L111-L137)：`getTranBean()` 构造 `voiceId`、`offset`、`filePath`、`encodeType=6`、`isfirst`、`vegNum` 和 `vegPos`。偏移为零时标记首片，临时文件后缀为 `.silk`。
- [同一文件的音频处理](https://github.com/tsuzcx/qq_apk/blob/afe46ef5640d0ba6850cdefd3c11badbd725a3f6/com.tencent.mobileqq/classes.jar/com/tencent/mobileqq/transfile/PttSliceUploadProcessor.java#L254-L325) 从 PCM 文件读取分片，经处理器编码后写临时文件，再通过 BDH `Transaction` 的命令 68 上传。音频处理器在 [启动逻辑](https://github.com/tsuzcx/qq_apk/blob/afe46ef5640d0ba6850cdefd3c11badbd725a3f6/com.tencent.mobileqq/classes.jar/com/tencent/mobileqq/transfile/PttSliceUploadProcessor.java#L704-L719) 使用 `SilkCodecWrapper` 和 `processor.a(16000, 16000, 1)`。
- [`PttSliceInfoBean`](https://github.com/tsuzcx/qq_apk/blob/afe46ef5640d0ba6850cdefd3c11badbd725a3f6/com.tencent.mobileqq/classes.jar/com/tencent/mobileqq/transfile/SmartPttTranHelper%24PttSliceInfoBean.java) 明确声明 `chatType`、`encodeType`、`filePath`、`isLast`、`isfirst`、`md5`、`offset`、`vegNum`、`vegPos`、`voiceId`。
- 较新的 Android 9.2.5 [`SmartPttTranHelper.smali`](https://github.com/cxxsheng/Android_9.2.5_64/blob/bdf49f7db358393cc96a961fa50ceca4d33852f4/smali_classes14/com/tencent/mobileqq/transfile/SmartPttTranHelper.smali) 的 `getSmartPttReqBody` 明确设置 `bits_per_sample=16`、`samples_per_sec=16000`、`voice_file_type=1`、`service_id=1`，并将语音 ID、首末片标记、编码类型和偏移写入请求。

因此“音频格式和任务信息完全没有公开线索”需要修正：**Android 侧有具体实现可参考；macOS 侧两个参数的绑定契约仍未确认。** 这些仓库保存的是客户端反编译材料，并非已经验证可工作的第三方 macOS SmartPtt 适配器。不能把 Android 的 `encodeType=6`、6400 字节 PCM 分块或 Java 字段名直接当作桌面接口规范。

本轮 GitHub 定向查询 `uploadSmartPttSlice`、`SmartPttSlice` 及相关 issue 未找到桌面调用点；`SmartPtt`、`getSmartPttReqBody`、`PttSliceInfoBean` 找到了上述 Android 材料。搜索结果有索引和覆盖范围限制，不能据此断言无人做过。

音频编码有可复用项目：[idranme/silk-wasm](https://github.com/idranme/silk-wasm)。其 README 声明 `encode(input, sampleRate)` 支持 WAV 或单声道 `pcm_s16le`，支持 16000 Hz，返回 SILK 数据。它可以作为后续编码器候选，**不证明其输出符合 macOS SmartPtt 的切片、帧头或状态要求**；当前未安装或运行该依赖。

## 桌面插件与桥接的实现先例

### 复用正在运行的桌面 QQ session

NapCat Framework 的 [`napcat.cjs`](https://github.com/NapNeko/NapCatQQ/blob/98d781d19815d1625b24d717ce280ecf0a4f6d0c/packages/napcat-framework/napcat.cjs#L90-L165) 提供了具体机制：在 `wrapper.node` 加载前挂接 `process.dlopen`，包装 QQ 使用的 `NodeIQQNTWrapperSession` 构造或工厂方法，取得 QQ 自己创建的 session，包装 `init` 的 listener，并等待 `onOpentelemetryInit({is_init: true})`。调用保留原始 receiver，不预先创建固定的 `nt_1` session。[初始化代码](https://github.com/NapNeko/NapCatQQ/blob/98d781d19815d1625b24d717ce280ecf0a4f6d0c/packages/napcat-framework/napcat.ts#L174-L187) 等待捕获的 session 后才初始化服务。

这是可借鉴的早期加载和 session 捕获方式，不是一个能够直接连接未修改 QQ 的外部接口。VoicePort 应独立实现所需的最小诊断逻辑；完整框架还含 WebUI、机器人适配器和 native hooks，不能直接视作该功能所需的最小依赖。复用代码前还需核对项目许可。

### 保留正常 QQ 窗口的插件加载

LiteLoaderQQNT 的 [主入口](https://github.com/LiteLoaderQQNT/LiteLoaderQQNT/blob/1a8c9707c3673e0491445a1d6d7a86f0a1aa8000/src/main.js#L6-L21) 先安装 hooks、加载插件，再加载 QQ 原有 launcher。[安装说明](https://github.com/LiteLoaderQQNT/LiteLoaderQQNT/blob/1a8c9707c3673e0491445a1d6d7a86f0a1aa8000/README.md#L39-L59) 需要新增 bootstrap 文件并调整 QQ `package.json` 的入口。其 `onLogin(uid)` 事件不是原生 session 对象，单独取得 uid 不足以调用 BDH 服务。

NapCat Shell 和 Waylay 的独立登录模式不等于挂接用户已打开的 QQ。Waylay 的 [Electron 入口](https://github.com/Micuks/waylay-qq-bridge/blob/3dd3572a4e34f5b2f91ad6d645a16fe6cc34eadb/src/electron-entry.js#L3-L20) 明确走无 QQ GUI 的 standalone 模式。不能因为项目包含 Darwin 分支就断言适配当前 macOS App Store QQ。

### 本机 App Store 版本的限制

LiteLoader 历史 issue 中有 App Store QQ [6.9.19 的启动/退出问题](https://github.com/LiteLoaderQQNT/LiteLoaderQQNT/issues/130) 和 [6.9.33 因缺失 versions/config.json 加载失败](https://github.com/LiteLoaderQQNT/LiteLoaderQQNT/issues/303) 的记录。它们表明历史版本存在加载先例及版本差异，不能证明本机 `7.0.1-52892` 兼容。

所查 [macOS 安装脚本](https://github.com/Mzdyl/LiteLoaderQQNT_Install/blob/32a5eeda8768039dcba525984e32026122f802e6/install.sh#L332-L359) 修改 app 中的加载文件与入口，没有提供可据以认定本机需要或只需重新签名的依据。当前构建的签名、沙盒、插件路径访问和本机通信均需实际验证。本轮没有执行安装器或修改 QQ。

## 实现前仍需验证

1. 兼容该 QQ 版本的进程内入口和 session 获取方式，以及可审阅的加载方案。
2. `uploadSmartPttSlice` 两个参数的完整结构和返回值。
3. 任务 ID 来源、创建时机、切片顺序、结束标记、取消和超时行为。
4. 音频编码、采样率、通道数、容器、切片长度。
5. 两种回调的实际载荷、增量或累计文字语义、错误处理和监听释放。
6. 从 VoicePort 调用时的本机鉴权、并发隔离和音频临时文件清理。

当前阻塞是缺少经过验证的调用契约和外部入口。继续需要兼容的类型/调用点，或在 QQ 进程内进行受控验证。不能用猜测的请求结构编写“可用”适配器，也不能使用腾讯云 ASR 或其他识别引擎冒充 QQ 自带能力。

## 推荐的落地顺序

推荐先验证一个最小 QQ 进程内诊断桥接，再接入现有 VoicePort JSONL 插件链路：

1. **验证加载和 session。** 复用 QQ 自己创建、登录和初始化的 session；先确认当前 macOS 构建可加载及正常退出。App Store QQ `7.0.1-52892` 的兼容性仍是待验证项，不能由其他版本的成功案例推出。
2. **补齐桌面调用契约。** 观察正常「语音输入文字」调用时的参数类型、键名、切片时序和回调结构。诊断输出应限定为必要元数据，不记录令牌、账号、聊天内容或识别正文；不能把公开 Android 字段直接硬编码成已确认的 macOS 请求。
3. **验证最小音频任务。** 确定桌面音频契约后，使用显式确认上传的合成样例检查创建、上传、结束、错误和取消；真实语音识别效果另行验证。此步骤不需要发送聊天消息。
4. **连接 VoicePort。** 数据流为 VoicePort 录音 → JSONL 适配器 → QQ 内本机桥接 → SmartPtt → 结果回调 → JSONL `final`。现有适配器输入为 `wav_pcm16_mono_16000`，桥接按已验证的 QQ 格式转换并分片；QQ 保管自身登录态。适配器仅连接已经运行的桥接，不在受控转写子进程中偷偷拉起后台服务。
5. **收紧桥接能力并验收。** 本机传输需鉴权，只开放这条转写任务链路。优先考虑权限受限的 Unix socket，但须先验证 App Store 沙盒能否访问共享路径；否则使用仅监听回环地址、强制随机配对令牌和严格 Origin 检查的本机服务。按任务 ID 隔离结果，限制并发、音频大小和时长，处理断连、超时、取消、监听释放及临时文件清理。完整链路验证通过后再提供可用 QQ 来源。

这个方案复用现有 VoicePort 来源管理、录音和文字输入流程，也让 QQ 继续管理自己的登录与网络协议。首先要交付的是兼容本机 QQ 的诊断结果；仅靠增加一个 HTTP 地址字段或导入 manifest 不能补齐当前缺失的底层能力。

## 本轮操作范围

只检查安装目录的程序资源和公开源码。没有修改或重启 QQ，没有安装桥接，没有读取聊天记录或 QQ 凭据，没有录音、上传音频或发送消息。没有运行 QQ 转写端到端测试；本轮不宣称已验证识别效果。
