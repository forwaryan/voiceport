# 本机 SmartPtt 静态调用契约

2026-11-10，从 App Store QQ `7.0.1-52892` 的 arm64 原生绑定恢复。以下是静态代码证据，**没有完成真实调用或语音识别验证**。

## 上传参数

常规输入类型可以表达为：

```ts
uploadSmartPttSlice(taskId: string, slice: {
  chatType: number;
  sliceIndex: number;
  filePath: string;
  voiceOffset: number;
  isEnd: boolean;
  removePeriod: boolean;
}): void;
```

六个对象键来自 `napi_get_named_property` 的实际字符串引用，不是从日志字段推测。三个数字字段的转换助手调用 `napi_get_value_uint32`；两种布尔字段调用 `napi_get_value_bool`。绑定逐一读取这六个属性，没有据此认定任何属性可以省略。转换助手还存在字符串解析或其他转换分支，不能把这种宽松转换当成推荐输入类型。

`taskId` 和 `slice` 是便于说明的参数名，原始形参名未恢复。绑定没有构造返回结果或 Promise，完成结果由监听回调返回。

## 回调

```ts
onSmartPttUpdateText(taskId: string, text: string, segFinished: boolean): void;
onSmartPttTranslateFinished(taskId: string, errorCode: number, errMsg: string): void;
```

两种原生回调均为三个位置参数。完成回调通过 `napi_create_int32` 构造错误码；native 分发器把零错误码作为成功分支。后续静态解码的 AudioSDK/VoiceInputStore 显示：更新文字替换当前段，`segFinished` 后追加到已确认文本；整体完成走单独的 TranslateFinished 回调。下面记录具体指令依据；这仍未经真实识别验证。

## 已追踪的任务行为

- manager 在提交切片时按任务 ID 查找或创建 uploader；不要求先找到另一个显式“创建 uploader”的 JS 方法。后续缓存解码恢复了 QQ 调用方的 15 随机字节转 hex 约定，见下文；服务端对 ID 的全部接受规则仍未知。
- 空 `filePath` 在入队前被拒绝，因此不能用空路径发送一个纯结束请求。
- handler 收到 `isEnd` 会先记录该任务 ID 已结束，再提交当前切片；之后同 ID 的提交会被阻止。这不等于后端识别已经完成。
- uploader 拒绝已取消或已完成任务的后续切片。
- `sliceIndex == 0` 标记首片；有待处理切片时重复提交首片会被拒绝。接受的首片重置内部计数和最后索引。
- 非零索引必须严格大于保存的最后索引；小于或等于都会被拒绝。已观察的是递增要求，尚未证明必须连续编号。
- 提交前会打开文件并检查大小；空文件或打开失败不会进入正常上传路径。若它是最终片，会走显式失败通知。
- JS 绑定读取了 `voiceOffset`，但已追踪的 wrapper 到 service 请求构造中没有观察到复制该字段。不能据此认定它在所有版本无效。

## 音频请求中的静态线索

桌面端 `BuildUploadReqPb`（`0xbb89bc`）调用 metadata 构造函数 `0xbb87dc`。后者对嵌套消息写入常量 `1`、`6`、`16`、`16000`，具体写入位于 `0xbb8830` 至 `0xbb8874`；同时写入序列号、首末片和标点选项。

进一步追踪通用消息构造器及其实际序列化路径，已确认**数字选择器直接成为 protobuf 字段号，没有字段号偏移或 Android schema 映射**：

| 层次 | 字段号 | 已确认的值/来源 | wire 类型 |
| --- | --- | --- | --- |
| metadata 外层 | `1` | 常量 `5` | varint（`0`） |
| metadata 外层 | `4` | 下列嵌套消息 | length-delimited（`2`） |
| 嵌套消息 | `1` | 常量 `1` | varint（`0`） |
| 嵌套消息 | `2` | 常量 `6` | varint（`0`） |
| 嵌套消息 | `3` | 常量 `16` | varint（`0`） |
| 嵌套消息 | `4` | 常量 `16000` | varint（`0`） |

这里区分**字段号**与**完整 wire tag**：数值字段的 tag 是 `field_number << 3`，所以上述四个内层字段的 tag 分别是十六进制 `08`、`10`、`18`、`20`；嵌套消息的 tag 为 `(field_number << 3) | 2`。

可复现的关键路径：

- setter `0x387de70` 将传入选择器保留在 `x20`，`0x387e194` 原样写入 entry key；序列化前复制同一 key。
- metadata 在 `0xbb895c` 调用 vtable `+0x100`，解析为 serializer `0x3880c78`。其类型表 `0x53783e0` 将内部数值类型 `3` 分派到 writer `0x3879f0c`；`0x38811b4` 直接载入 entry key 并传给 writer。
- 数值 writer 调用 `0x1dc2ff8`，后者在 `0x1dc300c` 执行 `lsl w1, w0, #3`，再由 varint writer 输出字节。内部紧凑存储使用的 ZigZag 在输出前还原，不把上述常量改写为 ZigZag wire 值。
- 消息 writer `0x387acbc` 在 `0x387ad3c` 递归调用子消息 serializer，在 `0x387ad54`–`0x387ad58` 构造 wire 类型 `2` 的 tag。

这些事实确认了本机消息的字段号、数值和编码方式，**尚未恢复字段名称，也没有确认音频文件格式或服务端接受条件**。公开 Android `Smartptt.PttUpReq` 的字段声明与本机字段号不同，不能直接套用其字段名称；也不能只凭 `6`、`16`、`16000` 就认定文件容器和音频编码已经验证。原始切片文件仍需确认是预编码音频还是由其他阶段处理过的数据。

本轮新增原始证据为私有调查目录中的 `selector-wire-*-a-disassembly.txt` 与对应命令记录，其中包括 serializer、numeric writer、tag emitter、最终 varint 字节写入和嵌套消息 writer。全部为加载文件的静态反汇编，没有运行或附加 QQ。

## 桌面流式采集模块的 SILK 分片

对同一 QQ 安装包中的 `Contents/Resources/app/AudioFoundation.node` 继续静态追踪，已经恢复其**流式采集模块的分片写入格式**。这比 Android 类比更直接，但仍需验证它与实际 SmartPtt 调用的准确衔接，不能当作转写服务已接受该格式。

文件 SHA-256：`f26c97843c5de133d8408b64a81d87cd035d53f5dcbba14d1b03a06c116a5576`。以下同样是 arm64 未加 slide 地址：

| 位置 | 已确认行为 |
| --- | --- |
| `AudioStreamCapture::Init`，`0x19428` | 接受输出目录、采样率、通道参数；后续 AudioSDK 缓存指令恢复其传入 `streamOutputDir, 16000, 1`，见下文。 |
| `StreamCapture_Create` / `Start`，`0x1c8e8` / `0x1ca94` | 建立流式分片状态，按配置的分片间隔与音频参数计算阈值。 |
| `StreamCaptureCallBack::AudioCallBack`，`0x1c04c` | 缓冲采集数据并生成编码分片；本轮没有调用该函数。 |
| `0x1c3b4`–`0x1c438` | 使用 `%s/chunk_%04d.silk` 构造路径，以 `wb` 打开分片文件。 |
| `0x1c444`–`0x1c448` | 检查状态 `+0x4ac`；仅未初始化时进入文件头和 encoder 初始化路径，续片跳过这两步。 |
| `0x1c648`–`0x1c67c` | 首片写单字节 `02`，再写 `#!SILK_V3` 的九个字节，总文件头长度为 10。字面量位于 `0x3e218`，已通过静态文件映射核对。 |
| `0x1c714` / `0x1c72c` / `0x1c810` | 获取 SILK encoder 大小并初始化，随后把 `+0x4ac` 置为 `1`；续片保留 encoder 和缓冲状态。 |
| `0x1c29c` | 调用 `SKP_Silk_SDK_Encode`（`0x2a658`）。 |
| `0x1c2c0`–`0x1c2e0` | 编码成功后先写两字节长度，再按该长度写 SILK payload；当前 arm64 为小端，长度前缀为小端。 |
| `NodeIStreamChunkListener::OnChunkReady`，`0x13228` | 向 JS `OnChunkReady` 传递八个位置参数：字符串及七个数值，顺序已通过异步 lambda 转发确认，见下方。 |

补充静态 JS 入口形状：`NodeIStreamCapture::Init`（`0xfb5c`）与 `Start`（`0x11b34`）均要求准确三个参数，并通过 `napi_get_boolean` 返回原生结果。`Init` 按序转换为 UTF-8 字符串、两个 signed int32；`Start` 按序转换为 signed int32、UTF-8 字符串、signed int32。字符串助手 `0x4de8` 实际调用 `napi_get_value_string_utf8`；两个入口将这些参数原序传给接口虚方法。它们的形状分别是 `Init(string, int32, int32): boolean` 和 `Start(int32, string, int32): boolean`。本次没有调用入口，也没有因此恢复上层 AudioSDK 传值、任务 ID 或 SmartPtt 的切片映射。证据为 `audio-js-{init,start,string-helper}-a-disassembly.txt` 和对应命令记录。

**修正此前的阶段结论：并非每片都有文件头，也并非每片重建 encoder。** `StreamCapture_Start` 将 `+0x4ac` 清零；首片初始化成功后置为 `1`，后续文件继续同一 SILK 编码流。适配器应保持跨片 encoder 状态，不能将多份独立 SILK 文件直接作为续片。

因此已确认的正常写入结构为（文件头只在首片）：

```text
02 23 21 53 49 4c 4b 5f 56 33       # 02 + ASCII "#!SILK_V3"
[length: 2 bytes, little-endian] [SILK payload]
[length: 2 bytes, little-endian] [SILK payload]
...
```

正常关闭函数 `0x1cf64` 的最终分片路径在缓冲区还有不足一帧的数据时，以 `bzero` 补齐后再次编码，写入相同的长度/payload 记录，再在 `0x1d0c4` 关闭文件；该已追踪路径没有追加 `-1` 帧结束标记。它随后检查实际文件大小，并在必要时进入独立的静音最终片兜底函数 `0x1d38c`。后者按照调用方传入的 `write_header` 决定是否写头，使用独立的新 encoder、`16000/16000/320/25000` 控制向量，把 640 字节清零后编码 320 个样本，再写同样的长度/payload 记录。这是兜底分支的设置，不能据此认定普通采集也使用相同 bitrate。这个结构仍不能视作覆盖所有停止/错误分支的完整容器规范。采样率、通道、bitrate、分片时长和停止行为也不能仅由文件头确定。

异步转发 lambda `0x1b890` 在 `0x1b8ec`–`0x1b90c` 按序读取保存的字段，再由 N-API 绑定原序序列化，建立以下静态回调形状：

```ts
OnChunkReady(
  filePath: string,
  chunkIndex: number,
  chunkOffsetBytes: number,
  fileSizeBytes: number,
  durationMs: number,
  startMs: number,
  endMs: number,
  isLast: number,
): void;
```

`isLast` 通过 `napi_create_double` 输出，属于数值。`chunkOffsetBytes` 是累计**编码文件字节数**：关闭后用实际文件大小更新状态 `+0x49c`（`0x1d2dc`–`0x1d2e4`），不能当作 PCM 字节偏移或毫秒；这也尚未证明等于 SmartPtt 的 `voiceOffset`。

新增 `stream_framing.cjs` 只接收调用方给出的 Buffer 数组，校验首片头、无头续片、长度记录完整性及本地资源上限，并输出编码字节偏移等计数。它不读文件、不编码、不上传，也不判断 payload 是否为有效 SILK。其通过不代表上述 AudioSDK/SmartPtt 关联、QQ 接受条件或识别成功。

`major.node` 的 AudioSDK 字节码区域还包含 `NodeIStreamChunkListener`、`OnChunkReady`、`filePath`、`chunkIndex`、`chunkOffsetBytes`、`isLast`，随后有 SmartPtt 的 `sliceIndex`、`voiceOffset`、`taskId`、`sliceParam` 和上传日志；renderer 的 `VoiceInputStore` 含 `audioSDKApi`、`onStreamASRText`、`streamCaptureStart`、`streamCaptureStop`。最初这些只建立目标功能的静态关联，不能把相邻字符串拼成已验证代码；后续实际指令解码恢复了下文所列的具体参数映射。

原始证据保存为私有调查目录中的 `audio-stream-{create-start,pcm-callback,chunk-callback,info-forward}-a-disassembly.txt`、`audio-stream-header-a.txt` 及对应命令。全程只加载文件，没有启动录音、调用 encoder、读取现有音频或上传文件。

## AudioSDK 封装的缓存定位

进一步静态追踪 `major.node` 的 `LoadInternal`（`0x9dc`）、`Load`（`0xf20`）和全局初始化路径，确认它按模块表提供 V8 缓存，再交给 `vm.Script` 的 `cachedData`。这条加载器证据与字符串邻接不同，但仍没有恢复 AudioSDK 的赋值表达式。

已在同一安装包的 arm64 slice 中定位并仅复制两个程序缓存块：

| 目标 | 整个 universal 文件中的偏移 | 缓存长度 | SHA-256 |
| --- | --- | --- | --- |
| 同时含 SmartPtt / AudioSDK 标识的块 | `112548576` | `6034456` | `2424f6b0e7111de27562395cab95bdfcf01d48ff12b4cd4cc2b08a73a9cd6774` |
| 含 VoiceInputStore 的目标块 | `149112464` | `62224` | `ca9ac33ee588b0c642d4ed2db6ba9227507508112c99f8831da93c7ed4ec59ff` |

第一块的虚拟地址为 `0x131ae0`，与构造代码 `0x7cec`–`0x7d00` 写入的指针及 `0x5c1418` 长度一致；附近模块键为 `b_86background`。第二块的地址 `0x2410690` 与构造代码 `0xe3c8`–`0xe3d0` 相符，长度来自缓存头。缓存头包含 magic `0xc0de0674`、version hash `0x12819a30` 和只读快照校验值 `0x559ba28c`。

QQNT 程序文件标明 V8 `14.4.258.16-electron.0`，安装包的 `v8_context_snapshot.arm64.bin` 头部只读校验值与上述缓存相同；同包 x86_64 快照的值不同。arm64 快照 SHA-256 为 `99069a6e4605234fe5ebc4fd85bd4ba8465401884f35d50f74339b0afbd0d0e4`。这些是选择匹配解析配置的依据，不能单独证明某个第三方解析器兼容；架构、构建配置和快照仍需核对。

原始定位证据保存在 `major-{load-internal,load-binding,load-binding-tail,global-init,bytecode-map-init,renderer-target-record}-a-disassembly.txt`、`major-cache-inventory-a.json`、`producer-snapshot-metadata-a.json` 与提取记录中。通用缓存头扫描器输出的是候选边界，不代表全部模块都已确认。后续已经进行下述静态反序列化及指令解码；没有执行目标缓存。

## 从缓存恢复的 AudioSDK 参数与文字合并

固定静态解析器源码版本为 `tgwsaki-meow/v8-jsc-decompiler@628f16c32ad097033e4f3ee157f79198f5fa8fcc`。没有安装依赖或导入其 CLI，仅调用审核过的本地数据解析库。默认 Windows x64 只读表与 QQ 校验值不同，因此没有使用它。自己的包装脚本从匹配的 arm64 快照建立对象表，修正两项具体布局：ARM64 Swiss 字典分组宽度为 8，空字典占 24 字节，其后独立识别出 8 字节 TwoPointerFiller；空 FeedbackMetadata 的两个计数均为零，按 12 字节头计算，取消后续根地址估算。原始解析失败 a/b 均保留。

本机快照整体验证与只读校验值通过，1,045 个静态根均落在 10,300 个对象边界，无类型不一致、估算大小、遍历错误或尾部残留。两个目标缓存均严格检查版本 hash、flag hash、external-reference 数量、快照 checksum、长度和对象预算；尾部仅为允许范围内的 NOP，没有未解决的 forward reference。VoiceInputStore 得到 110 个函数，主程序块得到 12,169 个函数；主程序块 9,076 次只读引用全部解析。已选择的 31 个主程序候选函数及 43 个 renderer 候选函数均完成指令解码，无非法指令、跳转边界或寄存器范围警告。

限制仍然明确：解析器配置来自 Windows x64，不能声称整体已验证 macOS ARM64 构建兼容。主程序块另有 515 个辅助对象未分类（ClassPositions、NameToIndexHashTable、WeakFixedArray），renderer 有 4 个；函数本身没有解码警告。这些数据支持下述具体静态调用链，不证明真实运行或服务端接受。

| 函数 / 缓存内 SFI ID | 指令偏移 | 静态行为 |
| --- | --- | --- |
| AudioSDK 模块 `42887` / `3501` | `312`–`344` | 支持条件为 `process.platform === 'darwin' || process.platform === 'win32'`。 |
| `generateTaskId` / `3803` | `0`–`34` | 调用所引入模块的 `randomBytes(15).toString('hex')`；30 位小写十六进制是 QQ 调用方约定，不据此断言服务端格式限制。 |
| `initStreamCapture` / `3844` | `0`–`23` | `streamCapture.Init(streamOutputDir, 16000, 1)`。原生参数语义对应输出目录、采样率、通道。 |
| `streamCaptureStart` / `3748` | `306`–`387` | 生成新任务 ID，调用 init，然后 `streamCapture.Start(1, '', 1)`；后两数值的完整模式语义仍未知。 |
| `OnChunkReady` / `3873` | `0`–`274`、`337`–`374` | 从 rest 参数提取第 1、2、3、8 项，构造 `{filePath, chunkIndex, chunkOffsetBytes, isLast}`。与原生回调的八项位置一致。 |
| `handleStreamChunkReady` / `3936` | `0`–`112` | `filePath` 原样传递，`sliceIndex = chunkIndex`、`voiceOffset = chunkOffsetBytes`、`isEnd = (isLast === 1)`。 |
| `uploadSmartPttSlice` / `3949` | `18`–`108` | 调用 JS facade 的 `nodeIKernelBdhUploadService.uploadSmartPttSlice({taskId, sliceParam})`，其中 `chatType: 7`，另五字段来自当前切片及 `streamRemovePeriod`。这是对象 envelope，须与本文开头的原生两个位置参数区分。 |
| 更新监听闭包 / `3976` | `0`–`109` | 接收 `{taskId, text, segFinished}`；仅处理当前非空且相等的任务 ID，发出 `{text, isEnd: false, segFinished}`。 |
| 完成监听闭包 / `3988` | `0`–`187` | 接收 `{taskId, result, errMsg}`；匹配任务后清空 ID，发出空文字的 `isEnd: true` 事件并映射错误；仍在采集时强制停止。 |
| `streamCaptureStop` / `3790` | `28`–`38`、`137`–`138` | 停止采集并清除 recording 标记；没有在这个方法中清空任务 ID，因此最终切片仍可沿相同任务提交。 |
| renderer `onASRText` / `1197` | `213`–`238`、`357`–`440` | 用每次 `text` 替换 `streamText`；段完成后 `confirmedText += text` 并清空 `streamText`。整体完成时追加剩余 `streamText`。 |
| renderer `insertASRText` / `1215` | `21`–`113` | 后续部分结果替换已有 ASR 文本范围，与当前段累计快照的解释一致；不能逐回调直接追加文字。 |

上述 SFI ID 和偏移只属于表中 SHA-256 对应的缓存，不是原生虚拟地址，也不保证跨版本稳定。AudioSDK 的 facade 来自模块 `72421`，外层 RPC 来自 `84565`；facade 的转换与监听注册已继续恢复，见下节。RPC 的完整外部调用封装和可用进程入口仍未验证。

新增 `smartptt_protocol.cjs` 只构造上述对象 envelope 并维护一个任务的文字状态。它验证本地输入预算、忽略其他任务与结束后的回调，在失败完成时不返回成功 final；没有文件读取、IPC、原生加载、录音或上传。五项新增夹具覆盖编码偏移、`isLast === 1`、修订文字替换、跨段累加、过期任务隔离和错误完成等行为。它不是可用的转写来源或 JSONL 适配器。

私有证据包括 `ro-layout-source-check.md`、`producer-ro-derivation-{a,b,c}.json`；通过检查的表在 `/private/tmp/voiceport-qq-ro-d2RhB0/ro-heap.json`。缓存解析输出在 `/private/tmp/voiceport-qq-cache-parse-{xANLzp,M0fzAK,QAc2Zi}/`，指令列表在 `/private/tmp/voiceport-qq-disasm-{G0b3M4,r1Mz8F,45cwhG}/`。路径仅用于本次复核；缓存 SHA、SFI 和指令偏移是复现依据。

## Facade 参数转换、监听注册与 session 来源

对同一主程序缓存继续选择性解码，已恢复 facade 到原生 BDH 服务的完整参数投影，以及原生位置回调到对象事件的转换。下列是静态调用链，不表示已在运行中的 QQ 取得这些模块。

| SFI / 模块 | 指令偏移 | 已恢复行为 |
| --- | --- | --- |
| `51955` / `72421` | `9`–`43` | 优先返回服务 override，否则转到模块 `99990` 的通用 facade；BDH 没有专用 override。 |
| `113701` / `27218` | `54`–`70`，常量 `c9` / `c11` | 元数据把 `nodeIKernelBdhUploadService` 映射到 `getBdhUploadService`，把 `uploadSmartPttSlice` 参数定义为有序的 `[taskId, sliceParam]`。 |
| `49795` | `45`–`162` | 通过服务元数据取得 session 的 getter，取得真实服务及方法的字段列表，再建立调用闭包。 |
| `49805` / `49812` | `14`–`31`、`91`–`107` / `0`–`8` | 按字段列表取 `objParams[fieldName]`；用服务对象作为 receiver 执行 `CallWithSpread`。`sliceParam` 保持对象，不展开六个内部字段。 |
| `49681` / `49689` | `151`–`169`、`197`–`217` / `14`–`33` | 收集原生位置参数，按元数据字段名 reduce 成一个对象，再发送给订阅方。 |
| `53303` / `53482` | `107`–`113` / `46`–`157`、`299`–`397` | 监听表将 `NodeIKernelBdhUploadListener` 映射为 `addKernelBdhUploadListener`；注册代码取得 BDH 服务 getter，再以回调对象调用该注册方法。 |
| `49736` / `49742` / `49665` | `13`–`56` / `0`–`43` / `34`–`66` | 为监听元数据中的每个方法安装相应 `fire` 函数，正是上面将位置参数转为对象的函数。 |

因此 AudioSDK 的 `{taskId, sliceParam}` envelope 在原生边界变为 `service.uploadSmartPttSlice(taskId, sliceParam)`。原生更新 `(taskId, text, segFinished)` 转成 `{taskId, text, segFinished}`；原生完成 `(taskId, errorCode, errMsg)` 转成 `{taskId, result: errorCode, errMsg}`。这里的 `result` 是 facade 字段名，值仍为原生 int32 错误码，不能当成转写文本；已追踪的原生成功值为零。

session 来源也已静态追踪：模块 `60629` 的 getter 经 `52578` 到模块 `96414.g.sessionNt`；`52697` 返回模块单例 context slot `29`，由 `52669` 初始化。`initWrapper`（`53237`，偏移 `93`–`107`）把 `sessionCreator.createNt()` 赋给 `sessionNt`。factory `53149` 调用 `NodeIQQNTWrapperSession.getNTWrapperSession(sessionIdList.get('nt') || 'nt_1')`，而 startup factory `53140` 从 startup wrapper 取得 `getSessionIdList()`。这说明应观察 QQ 自己持有的 session；本次没有调用工厂、创建第二个 session 或读取登录信息。

原始 facade 报告为私有目录的 `facade-trace-report.md`，含五组选择性提取及反汇编，共 67 个函数、零指令警告/错误；后续 session 提取输出为 `/private/tmp/voiceport-qq-cache-parse-{e4T8Fm,1SnkmN}` 和 `/private/tmp/voiceport-qq-disasm-{FIucTG,HFRTnb}`。这些结果沿用上节的严格 header/RO 检查和 Windows x64 typed profile 限制；`typed_layout_verified` 仍为 `false`。缓存 payload checksum 字段为零，因此不存在可验证的 payload checksum；文件 SHA-256 与 header/RO 检查不能表述为通过了该项校验。

目前未解决的是实际模块访问、兼容加载和运行中服务/监听验证。恢复出的内部引用链没有建立可从外部调用的入口，也没有产生真实识别结果。

## 可复现的二进制位置

文件：`/Applications/QQ.app/Contents/Resources/app/wrapper.node`。

SHA-256：`841a897e5c6df3d9713f337941d27d59efc831aa37aed560db8b89e2c6258020`。

Mach-O arm64 UUID：`2A2868E8-0FC3-3344-8FBF-45614997DA74`。以下为未加载、未加 ASLR slide 的 arm64 虚拟地址，不能直接用于其他版本。

| 位置 | 证据 |
| --- | --- |
| `0x890e80` | JS upload 绑定，两个参数数量检查位于 `0x890ef4`。 |
| `0x890fec` | 第二个参数的对象转换；键名引用依次位于 `0x89101c`、`0x891044`、`0x891070`、`0x89109c`、`0x8910c8`、`0x8910f8`。 |
| `0x2b3fd24` | 数字转换助手；`napi_get_value_uint32` 调用位于 `0x2b3fd60`。 |
| `0x88fb38` / `0x88fe98` | 更新与完成回调的参数序列化。 |
| `0x8900f0` | 以 `argc=3` 调用 JS listener 方法。 |
| `0x4fd268` → `0xba392c` → `0xbb5efc` | wrapper 虚调用、SmartPttService、manager 的提交路径。 |
| `0xbb67f4` → `0xbb6344` → `0xbb7924` | 排队 handler、查找/创建 uploader、提交切片。 |

可使用 LLDB 只加载文件并反汇编，不运行或附加 QQ：

```sh
/usr/bin/lldb --no-lldbinit -b \
  -o 'target create --arch arm64 /Applications/QQ.app/Contents/Resources/app/wrapper.node' \
  -o 'disassemble --start-address 0x890e80 --end-address 0x891140' \
  -o quit
```

原始命令和输出保存在本轮私有工作目录的 `evidence-report.txt`、`binding-disassembly.txt`、`service-disassembly.txt`、`handler-disassembly.txt`、`uploader-disassembly.txt` 及对应 `*-commands.txt` 中。此文档保留主要可复现地址，避免依赖临时目录永久存在。

## 尚需解决

1. 让诊断在本机 QQ 中兼容加载并捕获已经登录的 session，或验证可用的 renderer IPC 封装。渲染调试端口的真实探测和运行库删除调试开关的静态证据见 [渲染端 IPC 与调试入口证据](RENDERER_IPC.md)；尚无可用入口。
2. 验证恢复出的 `chatType: 7`、随机任务 ID 和切片映射能被本机服务接受，以及任务复用/取消的边界。
3. 验证正常连续 SILK 流、16 kHz 单通道和编码字节偏移的服务端接受条件，继续恢复普通编码 bitrate、分片配置及 Start 模式参数。
4. 将静态恢复的当前段替换/完成段累加与错误完成逻辑接到真实监听，补齐取消和超时行为。
5. 通过本机桥接连接 VoicePort JSONL 链路，再做获准的音频样例验证。

Android 的 SILK 实现只能作为追踪线索，不能替代第 3 项的 macOS 证据。当前没有可导入即用的 QQ 转写来源。
