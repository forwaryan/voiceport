# VoicePort 完整方案

## 当前实现（2026-11-17）

VoicePort 的目标仍是可替换识别引擎、统一光标输入。macOS 保留 ChatGPT 与插件式来源管理：一个插件定义接入方式，同一插件可保存多个独立来源，每次听写只使用一个。本地模型库包含 Paraformer、Whisper Tiny、SenseVoice Small、Whisper Large v3 Turbo、Parakeet TDT v3/v2、Parakeet Unified EN 和准确的多语言 Nemotron 3.5 ASR Streaming 0.6B。原生窗口以「这台电脑 / 云端」组织本地安装与独立 HTTP 来源配置。模型支持下载、固定文件导入、暂存校验后原子更新、删除与显式使用；首次下载安装 VoicePort 自有 sherpa-onnx 2.0.1 引擎，兼容旧模型可继续只读复用 1.12.37。质量无同音频评估时显示未测量。详见 [LOCAL_MODELS.md](LOCAL_MODELS.md)。

macOS 应用按选定来源运行：ChatGPT 经 `voiceport-core` 请求固定 ChatGPT 转写端点；八种本地模型通过后台原生桥接调用已安装的 sherpa-onnx 运行时，不读取 ChatGPT 登录或上传音频。UI、菜单、Carbon 快捷键、AVAudioEngine 录音与 PCM 分流、24kHz 单声道 AAC 编码、可选设备端 Speech、私有存储与 flock、AX 普通字段拥有范围编辑及 Orca 稳定前缀字符追加均在 Rust 中实现。内置来源不启动 Swift 或 Rust helper；用户导入并启用的 JSONL 插件则在后台执行明确确认的本机程序。HTTP 插件使用独立客户端和来源凭据，不转发 Codex 登录；通用 HTTP / JSONL 第一版只支持停止后转写，Paraformer、Unified EN 与 Nemotron 提供本地流式预览。云端页复用来源编辑器、草稿验证与 Keychain 事务；基础 URL 仅在表单保存时规范化为完整转写地址，保存不联网、不录音、不自动选择。共享语言表用于 UI/CLI/HTTP 校验，本地模型按真实支持子集在录音准备前检查，来源和语言保留到手动重试。旧 Swift 源码、测试和 SwiftPM 清单已从当前文件树移除，历史对照可从 Git 历史查看。

会话通过递增 generation 拒绝迟到权限与识别回调；录音仅在主动操作后打开。云端任务在后台线程执行，录音 Arc 租约保持到工作资源释放后才回报完成。取消先使会话失效，界面继续轮询停止确认；手动重试清除旧输入目标，不把旧音频的结果写入新焦点。设备端 Speech 不可用时继续录音并等停录后转写，不回退 Apple 云端识别。

普通字段每次更新检查焦点、选区和前后文，只改本次拥有的文字；终端只追加、不删除、不回车，用户活动使未发送部分停止。最终结果无法安全改写时保留面板。录音存储使用独立 `io.voiceport.rust-recordings-v1` 缓存域，避免迁移时触碰运行中的旧版；失败录音只允许显式重试，启动清理仅针对严格超过 24 小时且可确认归属/未占用的已知残留。

默认打包和验证全部使用 Cargo，旧测试兼容脚本也转向 Rust。构建细节见 [PACKAGING.md](PACKAGING.md)，实际测试和仍未执行的真实语音/账号/Orca 验收见 [VALIDATION.md](VALIDATION.md)。Windows 的既有阶段及未验边界独立保留。

自定义来源的添加、编辑、删除及显式样例测试见 [CUSTOM_SOURCES.md](CUSTOM_SOURCES.md)；版本化清单、配置字段、JSONL 协议和可实际加载的参考适配器见 [TRANSCRIPTION_PLUGINS.md](TRANSCRIPTION_PLUGINS.md)。保存和导入不执行插件或联网，密钥使用每来源独立 Keychain 引用，取消保持 busy 直至后台释放。开发自检与独立验收、真实 GUI / 录音 / 服务质量 / 目标接收分别记录，不能互相替代；现有 ChatGPT 客户端仍保持固定端点。

## 历史方案与验收背景

以下章节保存最初 Swift 方案及后续 Rust 核心/JSONL/Windows 阶段的历史记录。其旧目录、双语言打包和 Swift 命令不是当前构建要求；当时的检查结果只绑定原快照。

版本：0.1（2026-11-04，实施中）。执行进度见 ../task_plan.md；调研证据见 ../findings.md；循环执行契约见 LOOP_METHODS.md。此文件随代码和验证结果修订，拟议能力不视为已实现。

## 1. 产品目标

产品目标是在 macOS 和 Windows 上提供统一的桌面语音输入入口，将识别引擎与文字投递分开：用户可以按习惯选择、替换 ChatGPT、微信语音或其他识别后端，继续使用相同的快捷键和输入流程，将文字送入 Orca、其他 coding 工具以及支持的文本输入框。文字由用户检查并发送。

当前已接入的最终转写来源只有 ChatGPT，使用自己的 Codex / ChatGPT 文件登录；macOS 设备端识别用于实时预览。微信语音、其他最终转写后端和用户可操作的引擎切换仍是产品方向，尚未实现，微信语音的具体接入方式也未确定。当前生产转写端点保持固定，不代表已经存在通用插件或后端选择接口。

当前优先 macOS，已有实现和下文原生 API 均指 Swift/macOS 基线；Rust 共享核心与平台适配迁移的当前任务和未实现范围见 [跨平台路线](CROSS_PLATFORM.md)。下方流程保留 ChatGPT 首版方案的说明；当前实时听写与投递行为以 [README](../README.md#听写如何工作) 为准。

```text
VoicePort
├── 全局入口：Control + Option + Space 开始 / 结束录音
├── 本机录音：单声道 m4a，最长 180 秒，可取消
├── 云端听写：Codex 登录 → ChatGPT transcribe → 原始文字
├── 文字投递
│   ├── 原应用、原焦点、原剪贴板仍匹配 → Command + V
│   └── 条件不满足 → 保留结果，提示手动复制
├── 恢复：取消、登录指引、可见错误、手动重试、结果复制
└── 验证：离线契约 → 本机权限/录音 → Orca → 同音频 A/B
```

首版录完才转写。用户口述的英文标识符不会因 Orca 当前打开了代码而自动纠正。首版不读取项目代码、屏幕内容或目标输入框文字作为上下文。

## 2. 范围与分期

| 阶段 | 交付内容 | 验收条件 |
|---|---|---|
| M0 方案和执行基线 | 需求、架构、任务队列、loop 规则、验证入口 | 来源和未验证结论可追溯；恢复可定位下一任务 |
| M1 转写核心 + CLI | 登录文件解析、受限 multipart 上传、响应/错误处理、离线 demo | 无账号离线检查通过；CLI 可处理真实文件并给明确错误 |
| M2 macOS 菜单栏应用 | 全局快捷键、麦克风授权、开始/停止/取消、状态/结果界面 | 可构建 .app；真实启动；录音入口只在用户动作后请求授权 |
| M3 可靠投递和恢复 | 目标/焦点/剪贴板校验、安全输入保护、原剪贴板恢复、结果复制、手动重试 | 反例测试通过；取消后迟到结果无效；无 Return 发送事件 |
| M4 本机真实验收 | 本机录音上传、Orca/编辑器/浏览器验证、错误恢复、质量对照 | 保留实测结果；区分已验证、失败、待用户参与 |
| M5 后续增强 | Keychain 登录、凭据刷新、按住说话、自定义快捷键、可选历史、Orca 原按钮适配、签名/公证 | 根据 M4 证据确定顺序，不阻塞首版 |

M0–M3 是本地可独立实施范围。M4 中用户声音、账号调用和受系统管理的权限可能需要交互；不伪造录音或将离线 demo 当成云端成功。安装目录、开机启动、系统设置和外部发布不由 loop 擅自更改。

## 3. 技术选型

当前 macOS 产品使用 Rust 原生 AppKit 应用，默认启动不依赖旧 Swift 或转写子进程。`voiceport` 是公开 CLI，`voiceport-rs` 保留文件转写、demo、doctor 和 JSONL host 兼容入口；macOS App 控制经共享 `voiceport-control` 协议和受限本机 Unix socket，复用同一个 Controller、来源 Store 与 Keychain 事务。CLI 支持状态、窗口、来源/插件管理及显式听写操作，不隐式启动 App；CLI 听写按 operation ID 管理且固定关闭自动投递。Skill 位于根目录 `skills/voiceport/`，版本匹配的指南嵌入 CLI 并随 App 分发，见 [CLI 与 Skill](CLI.md)。

`VoicePort.app` 内使用不同叶名称 `Contents/MacOS/voiceport-app` 与 `Contents/MacOS/voiceport`，避免默认大小写不敏感卷上的 GUI/CLI 文件冲突；二者分别匹配对应的 Cargo 产物并验证签名。原生权限、真实语音和目标接收仍需分别验收，合成 IPC/服务检查不代表 GUI 或真实录音通过。

### 历史 Swift 首版选型

以下条目及目录树记录最初设计，已被当前 Rust 原生实现替代；历史检查不作为新版本验收。

- SwiftPM，macOS 13+，Swift 6；首版本机 Apple Silicon 构建，Intel 构建与发行待验证。
- `VoicePortCore`：Foundation 核心库，隔离认证、HTTP、错误和投递策略，便于离线测试。
- 历史 `voiceport-cli`：首版提供 `demo`、`doctor` 和 `transcribe`，便于同音频对照与诊断。当前公开 Rust CLI 还提供同一 App 的控制和 Skill，实际包内名称见本节开头；不能在默认大小写不敏感卷上同时安装仅大小写不同的 `voiceport` / `VoicePort`。
- `VoicePort`：AppKit 菜单栏和全局快捷键；SwiftUI 结果/设置面板；AVFoundation 录音；Accessibility/CGEvent 粘贴。
- Swift/macOS 基线不添加第三方运行时依赖。Rust 新版本的依赖与范围见下节；脚本负责 .app 打包、验证和开发 loop 状态记录。
- 本机 Command Line Tools 缺少 XCTest / Swift Testing，因此核心检查采用 `bash scripts/test_core.sh`：将核心源文件与独立断言程序一并编译执行。公开 API 不因测试暴露内部认证细节；`python3 -I scripts/verify.py` 汇总完整验证。首版不以 `swift test` 作为入口。
- 开发 loop 与产品运行状态完全分开；应用本身不调用模型编程工具。

```text
Sources/
├── VoicePortCore/       认证、multipart、转写、纯投递策略
├── VoicePortCLI/        离线演示、诊断、音频文件转写
└── VoicePortApp/        菜单、快捷键、录音、目标捕获、粘贴、结果窗口
Tests/VoicePortCoreTests/ 离线契约和安全反例
Tests/VoicePortAppTests/  实际控制器 + 受控系统边界
Tests/LoopTests/          Python 状态恢复与迁移检查
scripts/                 打包与验证
resources/Info.plist      Bundle ID、麦克风用途、菜单栏应用配置
automation/              开发任务定义
.voiceport/              被 Git 忽略的执行账本与验证证据
```

### Rust 新版本切片（VP-009）

Rust转写由根Cargo workspace的 `crates/voiceport-core` 和 `crates/voiceport-cli` 提供，二进制 `voiceport-rs`，Rust 1.95+ / edition 2024。VP-013另新增开发中的 `crates/voiceport-windows`：直接复用核心API的控制器、PCM WAV及显式--run的Win32托盘/消息循环/快捷键动作/手动复制壳已有部分编译/测试证据；完整默认Windows all-target类型检查已通过，实际链接仍缺GNU运行库，没有PE成品和Windows运行，录音/安全存储等平台能力未完成，后台转写调度已交付但未通过独立验收，见 [Windows进展](WINDOWS.md)。VP-009 核心切片实现文件登录与固定端点转写，当时的 CLI 范围为 help/demo/doctor/transcribe；后续 macOS App 控制、来源/插件命令和 Skill 见本节开头与 [CLI 文档](CLI.md)。VP-009 的 59 文件 freeze-01 已通过本机离线独立验收，协调者核对完整字节与实际日志后完成账本。36 项既有检查、8 组独立反例、37 个 release CLI 用例、真实 release SIGINT 130 和 4 项 loop 回归通过，边界见验证记录。VP-010 会话状态机与有界 JSONL 也已通过本机离线独立验收。VP-011 已把 .app 默认转写切换为打包内 Rust host，修复后的 freeze-02 已完成独立离线验收与协调核对；VP-012完整开发包、入口及依赖许可已通过freeze-02独立验收并完成账本，当前VP-013 Windows系统适配的部分freeze-01已独立退回，确认Actions取消后普通轮询不恢复停止状态，任务释放后新录音仍Busy；原开发交付修复后，新完整freeze-02已由原独立会话亲自确认缺陷修复和相邻状态行为，无新已实现范围阻断；原11断言修前101历史保留，当前独立执行0，当前Mac25步/52Rust及完整Windows目标检查通过，协调完整核对后接受本次返修阶段检查。完整VP-013仍未接受。续作03已交付生产后台执行器、结果回收与显式手动重试：进程单worker许可和完成槽保留唯一票据到实际线程结束，取消/退出先失效，手动Retry保留路径语言且不自动投递；异常owner drop通过有界进程持有保留活线程及录音引用，后续显式提交才回收已结束线程。该实现的完整freeze-03已由原独立会话以VP013-I-002退回：is_finished可先于线程局部析构完成，消息线程在Poll/取消Poll/orphan reap同步join，明确门闩反例三路均等待释放，成功预期0实际1。独立53项Rust、Mac25步和完整Windows默认目标检查仍通过；协调核对完整冻结/历史/1489新证据后真实retry/start新attempt，原开发须将可能等待的join移到受控后台路径或证明实际OS停止，保证消息线程不等待并保留唯一票据/File/许可到真实停止。不能只检查结果槽或计时后释放；原开发返修02已交付并通过协调材料核对：单一进程级清理线程启动至多一个worker并真实join，release/acquire停止确认后才准许消费Job/File/许可；一条闲置清理线程随进程保留，初始化失败不启动worker，异常owner仍有界持有。新增真实TLS及发布后panic检查，最终54Rust/fmt/严格clippy/完整Windows默认目标检查通过。原样after探针仍因孤立任务即时成功假设退出101，保留失败；仅恢复尾增加显式Retry的适配三处原响应断言均通过，原11断言保持。新设计的初始化、确认时序、清理线程退出和异常资源边界仍须原独立在新完整冻结自行复核。Windows类型检查不能代替完整链接/实机或录音/安全存储/保护投递及打包验收。

当前返修03更新：freeze-04已由原独立以VP013-I-003退回，合法清理线程仍在TLS中可领取请求时的is_finished推断会误消费Job并panic。现以UI和请求显式双重Job/许可所有权替代推断，清理请求真实销毁或后台实际join后才释放并确认；健康状态绑定真实receiver生命周期。UI owner丢失后清理侧也可在真实停止后自行释放，无需另一次UI回收。原清理TLS四例自检均0、健康后续请求可执行；最终55Rust/fmt/严格clippy/完整Windows默认目标检查0。完整freeze-05已由原独立亲自确认原样清理TLS四例及新所有权/健康后续/丢失循环/自动释放控制通过，当前Mac25步55Rust及完整Windows默认目标检查0。协调核对完整冻结/证据后接受返修阶段；全产品未接受，旧freeze和失败保持。下一实施切片先补Windows私有存储与held lease，设备采集和Start/Stop接线须在安全存储边界成立后推进；Windows原生运行分别留证。

Windows私有存储续作01的完整freeze-06已由原独立审查并通过源码/编译/离线所有权阶段，协调完整核对后接受本阶段。惰性Storage仅显式创建才做I/O，受限本地NTFS路径/祖先ACL策略、私有根与marker、原子随机新文件和同一持有对象的有界WAV完成；Recording私有字段携带Arc根/祖先/marker引用，Controller/Job/失败重试/取消/TLS/异常owner持有到真实停止。独立Mac25步60Rust、完整默认Windows all-target检查、新存储策略与实际线程TLS所有权反例/正控制通过。无删除扫描、设备采集或Start/Stop启用；3项native测试仅编译，ACL/sharing/reparse安全和平台运行仍未接受。保守威胁边界与私有partial残留限制见WINDOWS.md。下一独立可实施切片为惰性设备采集基础与受控合成后端验证；原生采集及安全存储实测前保持桌面Start/Stop禁用，完整VP-013所有必需范围保留。

Windows采集续作01现已交付未接线候选：NativeCapture构造无I/O，显式prepare从持有Root取得Pending，begin才打开/准备/开始WinMM；生产Session管道也用于合成后端控制。Windows格式仍为16kHz单声道PCM16位，与Swift AAC24kHz不同；固定180秒主缓冲加溢出哨兵，不引入重采样。停止、reset与实际缓冲归还、unprepare/close和backend析构完成后才完成同一Pending；失败/取消禁止部分成功，未知释放保留有界owner/许可。同步调用及Drop可能阻塞，只允许未来后台owner接入，无自主墙钟停止保证。开发71Rust及完整Windows检查通过，新增1项native测试未执行；尚须新完整冻结和原独立审查，不能启用桌面Start/Stop或声称原生安全/运行已接受。

Rust 使用 reqwest 0.12.28（关闭默认 features，仅 rustls-tls/multipart）、Tokio/tokio-util、Serde/serde_json、base64 和 dirs；测试使用 tempfile。Cargo.lock 固定解析版本，`scripts/test_rust.sh` 依次执行 tests/fmt/clippy，release 单独构建。依赖范围见 [第三方说明](../THIRD_PARTY_NOTICES.md)，具体命令见 [README](../README.md)。

Rust 保留逐请求重读、API-key 拒绝、JWT 到期 30 秒余量、安全头脱敏、禁重定向/自动重试/刷新和原文返回。生产 User-Agent 为 `VoicePort-Rust/0.1`，下节 `VoicePort/0.1` 对应 Swift。Rust 另禁用自动系统/环境代理；此选择和真实 TLS/企业证书兼容性需要后续验证。

Rust 实际读取音频不超过 20,000,000 字节，认证文件和保留的响应正文各不超过 1,000,000 字节；正文超限 chunk 不追加，不把缓冲限制写成精确网络收包限制。连接上限 15 秒、每次读取停顿上限 90 秒、请求至正文读取总上限 110 秒；与 Swift URLSession 计时语义有差异。本地同步文件读取发生在 HTTP 计时前，有字节限制和前后取消检查，但单次文件调用不能中途取消，文件内容/身份也不承诺在同 UID 并发修改下原子。网络等待及逐 chunk 读取支持取消；CLI Ctrl-C 退出 130，无法撤回已被服务接收的音频。

便携文件转写命令不录音或自动投递；`demo` 无网络/登录读取，`doctor` 只检查文件系统元数据，不证明登录有效。该入口成功退出 0、运行错误 1、参数错误 2。当前 macOS App 控制另有 missing/busy/confirmation 等稳定退出码，只有显式 `dictation start --source ID` 才录音，且固定关闭自动投递，完整语法与状态语义见 [CLI 文档](CLI.md)。Windows完整默认目标类型检查已通过，链接/运行及真实云端仍未验证；历史 macOS 宿主集成与 VP-012 freeze-02 开发包的离线接受只对应各自冻结，不能覆盖新增 CLI/IPC、组合修复或 GUI 验收，具体边界见 [跨平台路线](CROSS_PLATFORM.md) 和 [验证记录](VALIDATION.md)。

### Rust 会话与 JSONL 子进程（VP-010 本机离线独立验收通过）

`voiceport-rs host [--auth PATH]` 通过 stdin/stdout 处理协议 v1，启动只发 ready。只有 transcribe/retry 调用既有转写客户端，status 不返回文字或路径。进程级 generation 不依赖调用方 request_id；取消/替换/退出先失效当前身份再取消，拒绝旧成功、错误及服务取消。服务自行取消进入 failed，只有当前失败会话能以原路径/语言手动重试，使用新身份并带 is_retry=true；原生层须关闭自动投递。成功只保留当前身份、不保存转写历史或重试路径；录音文件始终由原生层持有与清理，Rust host 不删除文件。

完整 [JSONL 契约](HOST_PROTOCOL.md) 定义严格字段、关联事件、无效帧恢复和资源上限：输入 16,384 字节、输出 2,000,000 字节（不含 LF），输入/输出队列容量各 1、一个异步服务任务、单 writer 序列化并确认 flush，每帧 enqueue/write/flush 合计最多 5 秒。进程终身最多保留 1,024 个不同有效请求 ID，不淘汰重用；第 1,025 个返回 request_limit 后退出 0。EOF/shutdown 取消并退出；输出阻塞/EPIPE/超限失败退出，Ctrl-C 退出 130。两个 detached stdio 线程只用于本次进程生命周期，不作为可反复调用的常驻 daemon。

VP-011 原生接入须处理子进程退出/重启，并同时绑定进程身份与内部 session，拒绝跨重启的迟到回调。使用 Foundation Process 直接启动打包内二进制，不拼接 shell、不传原始凭据；继续复用已验收 Swift 录音占用与受保护投递。当前开发自检包含真实 release 的启动/错误/控制路径，合成成功服务仅在单独 Cargo example 复用实际 host/session/parser/writer；不把夹具成功当作生产云端成功。VP-010 freeze-01 已独立接受：41 项既有检查、6 项独立 Rust 检查和 46 个有效断言的真实子进程用例通过，协调者核对后完成账本。真实输出中断可能留下不完整 JSON 尾片段，VP-011 必须只接受完整 LF 帧并在 EOF 丢弃尾片段；每帧 deadline 不替代原生请求总 deadline。macOS 接入和 Windows 仍各自验收。

### macOS 原生 Rust 接入（VP-011 freeze-02 本机离线独立验收通过）

原独立验证已确认 VP011-I-001 修复，并完成首轮剩余协议/重试/预算/进程/lease 矩阵；69 文件/22 目录完整冻结及历史材料无漂移，31 核心/73 应用和八项最终命令符合预期。协调核对后已实际完成 VP-011；VP-012完整离线开发包和适用依赖许可现已在freeze-02独立接受。各组检查覆盖有重叠，不相加为单一覆盖总数；真实活跃普通 GUI 退出、云端/目标接收和 Windows 仍未验。

实际 DictationController 默认构造懒加载 RustBridge，经 Foundation Process 直接运行 `Contents/Helpers/voiceport-rs host`。没有 shell/PATH 查找、自动构建或 Swift 网络回退；缺 helper 时给可读错误并保留手动重试音频。默认构造和 smoke 不启动 child 或读取凭据。正常打包构建 Swift 与 locked Rust release，通过 cargo metadata 定位实际 target_directory，安装 0755 helper，先签 helper 再签外层；`--skip-build` 要求两种 release 产物均已存在。

bridge 在后台串行队列持有 Process 与 pipe，非阻塞读写、按字节累积完整 LF 帧；输出累计最多 2,000,000 字节加一个 8192 字节读取块，超限或不完整 EOF 失败，尾片段不带入新 child。stderr 使用独立描述符持续丢弃，不进 UI 或历史。协议处理核对 version/event/type/重复顶层键与 request/session/is_retry/阶段；每次建立或回收 child 更换 incarnation。freeze-01 独立反例确认 VP011-I-001：父端仅检查 session 字符串格式，未保留同一 incarnation 内的已接受身份，第二个 request_id 复用 s1 仍返回旧文字，原控制器随后调用自动投递边界并删除第二段音频。返修已实现每 child 的 UInt64 高水位：完整 ASCII canonical session 解析成功后要求 generation 严格大于高水位，accepted 即占用身份，无论终态成功或失败；允许跳号，拒绝重复、回退、格式错误和溢出，不使用无界身份集合。只在新建 child 或确认回收后重置，新 child 的 s1 仍可用。异常仍走原有界回收/unsafeStop 路径，不能提前释放可能在读的录音。原验证随后在完整 freeze-02 亲自复核原断言及剩余范围，已独立接受；接受绑定保存的完整快照，不把后续 VP-012 文档或包装改动当作旧冻结。

实际时限分别为 ready 5 秒、每次写 5 秒、每次控制回复 5 秒、accepted 后 terminal 等待 130 秒。启动至 accepted 可额外耗费 15 秒；异常取消/退出另有控制等待，再给 0.05 秒自然退出宽限、SIGTERM 后 2 秒及 SIGKILL 后 2 秒观察。130 秒不是整个操作总时限，单次同步 Process/文件系统调用仍无中途取消保证。该串行 API 供单控制器操作生命周期使用，不承诺多调用方并发 RPC。

用户取消先失效本地 SessionGate 并取消任务，保持录音 lease，待 bridge 确认 child 已停才清理音频、恢复可开始状态；停止无法确认时保留占用并禁用新录音/重试。AppDelegate 返回 terminateLater，通过 shutdownAndWait 的停止结果决定是否继续退出；投递者 shutdown 仍同步执行。服务返回明确 failed 时保留录音，活着的失败会话发 retry；子进程丢失/预算退出不自动重传，用户显式手动重试可新建 child 发 transcribe。录音开始时固定本段 language，重试沿用该值，且无论 wire is_retry 是否为 true，手动重试都禁自动投递。

### Rust/macOS 完整离线开发包（VP-012 freeze-02 独立验收通过）

`python3 -I scripts/verify.py --app-smoke` 运行locked Rust tests/fmt/clippy、正常双release打包、Swift core/app/loop与包装反例、两种release CLI正反路径、双产物/签名对应及已知无I/O菜单smoke。首次freeze-01虽通过常规入口，仍因根目录链接、替换主程序、未知许可和漏整包四输入被错误接受而独立退回。返修后的schema2完整清单覆盖762文件/270目录，lstat检查必需根和后代普通类型、链接与特殊文件，并将目录POSIX模式纳入指纹。根planning与私有协调材料可变；产品输入的初始错误和运行漂移必须失败，运行期反例使用自建副本与明确测试seam。

正常package与skip-build继续要求Swift和Rust两个实际release产物，Cargo target_directory由metadata取得。checker显式接收APP、RUST_RELEASE和SWIFT_VOICEPORT_RELEASE；Info与入口/模式须匹配，Rust同名签名副本和含正确Swift产物的同名重签bundle分别证明安装helper及主程序身份，不能以任意arm64有效签名代替正确产品。未知主程序不会为识别身份被执行。

实际包安装120个native metadata图crate的687份完整材料，原635份逐项保留，新增52份不按后缀限制的归属补充。runtime可达、build/proc-macro、dev重叠与其他平台lock分别说明，不把图可达或资料文件都描述为实际链接。许可校验只读重建锁定本机target/feature图、声明/选择与缓存完整材料集，源manifest须与此相符，安装manifest再绑定已验证源；未知/少包/重复/缺build_inputs及材料集合漂移不能只靠自洽通过，正常检查不生成资源。开发材料审阅和归档校验保留ring内嵌BoringSSL/fiat/once_cell、Unicode/证书、完整MPL包源及原GPT Voice归属；原独立随后完成覆盖、分类和条款充分性审阅：实际release normal/build树103包、normal边100包、workspace含dev树107包，metadata另13个可选节点未在实际workspace树启用。保守图角色不是实际链接清单；不声称法律审定、公证或外发批准。

返修01开发41 Rust、31核心、73应用、4 loop、11 packaging及25命令通过；原四例在正控制有效时修前负面runner退出1、修后退出0，六signed破坏对照仍拒绝。协调核对全部当前输入/快照、65保护字节与模式、历史材料及实际日志来源，不代替独立接受。原删除清单异常、旧Rust产物现象、返修首次错误helper夹具及两次可选cache校验文件假设失败均保留；具体旧构建缓存失配机制未实证。原独立现已在完整freeze-02及全新自有Cargo target完成全部原反例和剩余许可/入口范围：23项清单、6项入口边界、20个有效重签变异包、24项源许可反例及实际25步入口符合预期，完整条款充分性通过。协调核对762文件/270目录、30020历史材料及23554新证据后已真实finish；本次四文档仅同步接受状态，原快照保持不变。safe smoke在缓存清理/录音/auth/上传前退出，不能代表活跃普通GUI或真实录音/云端/目标接收。

## 4. 身份与接口契约

凭据由用户安装的 Codex 管理。首版只读取 `CODEX_HOME/auth.json`，未设置时读取 `~/.codex/auth.json`；CLI 可用 `--auth` 显式指定文件。只支持包含 ChatGPT access token / account ID 的登录，不接受 API key 模式。凭据每次请求重新加载，不缓存、不输出、不上传到其他主机、不写回 Codex 文件。

检测可识别 JWT 的到期时间。已过期时明确提示在 Codex 中重新登录；401/403 给登录或权限提示。首版不自行刷新 token，避免多个客户端竞争轮换。只有 Keychain 登录时给不支持的指引，后续单独实现官方存储读取与刷新协调。

云端请求固定为：

```text
POST https://chatgpt.com/backend-api/transcribe
Authorization: Bearer <本机登录的 access token>
ChatGPT-Account-Id: <account id>
originator: codex_dictation
User-Agent: VoicePort/0.1
Content-Type: multipart/form-data; boundary=<随机 boundary>

file = dictation.<扩展名>
language = 可选语言（默认自动识别）
```

请求不使用 cookie、不接受重定向、不附加提示词、项目代码或目标应用数据。支持 m4a/mp4、wav、mp3、webm、ogg、flac，上传前拒绝空文件、目录和超过 20 MB 的文件。上传/总耗时有上限；网络失败保留明确错误，手动重试，不自动重复提交音频。

200 响应必须含非空 `text`。错误不展示上游原始响应正文，避免错误页或凭据进入日志。403、429、5xx、超时、无效 JSON 和空文字分别映射可读提示。

此契约来自固定版本社区代码，不是 OpenAI 的公开稳定承诺。与当前 Codex App 使用完全相同模型、识别结果和扣量方式尚未证明。

## 5. 录音与状态机

```text
idle → requestingPermission → recording → transcribing → delivering → ready
          │                      │             │
          └─ cancel ─────────────┴─ cancel ────┴─ cancel → idle
                                               └─ error → failed → 手动重试
```

每次录音生成 session ID。异步返回只允许修改当前 session；取消先使 ID 无效，再停止后台，确认安全后尝试删除本次音频。Rust 接入中，Swift Task 取消不能单独证明 child 已停读，音频占用必须维持到停止确认。快速连按、正在上传时新录音、待剪贴板恢复时新会话都应被串行化。投递恢复窗口期间不接受新会话或取消。

服务层自行取消仍有效的请求时进入 failed，并保留音频供手动重试；用户取消和旧 session 的迟到结果不修改当前状态。权限拒绝和录音启动失败不提供无效音频重试。

控制器通过 AudioRecording / TextDelivery 协议以及权限、转写和计时闭包隔离系统边界。`bash scripts/test_app.sh` 编译实际控制器和 PasteCoordinator，使用受控系统替身与独立命名剪贴板验证 180 次计时、取消/迟到结果、重试、重复开始/结束、独立设置存储和剪贴板恢复；不录制声音、不触发系统权限、不访问一般剪贴板。退出先使 session 无效并取消任务，再同步调用投递者 shutdown；异步投递返回后重新检查 session 和取消标志，迟到提示不得改写状态。

录音为 AAC / m4a、单声道、24 kHz；最长 180 秒，达到上限自动结束并转写。麦克风仅在显式开始录音后请求。录音提示在菜单栏可见；结果窗口由用户主动打开，转写期间不抢输入焦点。

录音由 RecordingStorage 管理，固定放在 `~/Library/Caches/io.voiceport.recordings-v1`。选择 Caches 是为了限定稳定的本用户命名空间，不扫描系统临时根、其他缓存或旧版无标记的 VoicePort-UUID 目录。初始化仅保存路径与时钟，不创建目录；首次开始录音才创建管理根和 UUID 会话目录。目录要求当前 UID 所有且 POSIX 权限严格为 0700；归属标记、零字节 lease 和音频要求为单链接普通文件、当前 UID 所有且权限为 0600。AVFoundation 构造前预建音频文件，prepare 后、record 前再次检查权限。

根 owner.json 和 session.json 的严格字段集绑定应用、版本、UID、根 UUID、会话 UUID 和创建时间。每个会话使用独占 flock，覆盖录音、stop 后上传、失败保留和手动重试整个期间；stop 只停止编码器，不释放占用。成功、取消和正常退出尝试删除本次会话，录音设备构造、prepare、record 或输出检查失败也释放本次资源。进程终止后操作系统释放锁，不能用 PID 或音频修改时间推断会话是否仍活动。

普通启动执行尽力而为的元数据清理；不存在的根不创建，smoke 在构造清理存储前跳过。只处理归属、权限、预期内容和时间均可确认且可取得非阻塞独占锁的会话，持锁重查后，创建时间严格超过 86400 秒才删除；恰好 24 小时保留。未知混合内容、无效/缺失/未来版本标记、未来时间、错误权限、符号链接和硬链接均保留。实现使用目录描述符与 openat/fstatat/unlinkat，逐项复核身份，不递归删除未知目录，也不读取音频内容。目录枚举和标记读取有上限，超限保留。

清理不是到时必删保证，文件系统拒绝可能留下部分残留。当前只验证 POSIX 模式，未验证 ACL 或断电持久性；同 UID 任意替换/伪造并非原子防护目标，带符号链接的父目录保守拒绝。初始根标记写入途中崩溃可能留下不自动接管的根，旧无标记目录也不自动迁移或清理。60 项应用检查包含隔离目录、真实子进程锁/退出/并发清理与受控录音设备失败路径；原独立验证会话已对 VP-006 freeze-01 亲自执行完整离线回归及另 20 项独立检查并通过，协调者核对后完成 VP-006。真实 AVFoundation 编码输出另在 VP-007 验证。首版不保存转写历史，结果在进程内保留到下一会话/退出。

## 6. 投递与剪贴板约束

开始录音时捕获：前台进程、Accessibility 焦点元素及可编辑性、安全输入状态、剪贴板 changeCount 和可恢复内容。投递前验证：

1. 用户启用了自动粘贴，且辅助功能已授权。
2. 原应用仍在前台，开始与当前焦点元素均可确认、可编辑，且仍相同。自动投递的能力判据为：成功取得已知文本角色、AXEnabled 返回真正的 CFBoolean true、AXValue 的 settable 查询成功且为 true；未知、错误或不支持均保留结果供手动复制。该判据不把所有不可 settable 的控件断言为无法键盘输入，其真实编辑器兼容性另行验证。
3. 开始和当前均非密码/安全输入；任一端存在安全输入则不注入按键。
4. 剪贴板没有被其他操作改变，快照完整且大小受限。
5. 当前 session 未取消，且没有其他粘贴待恢复。

满足才暂存文字；完成临时剪贴板写入后再次校验应用、焦点、可编辑性、安全输入、权限、剪贴板拥有权和取消状态，才发送 Command + V，从不发送 Return。自动粘贴将换行与控制字符转为空格，结果面板和手动复制保留原文。

协调器保存旧快照与本次写入的 changeCount，并用独立 delivery ID 串行化投递。默认等待 750 毫秒后，仅在剪贴板仍归本次写入所有时恢复旧内容；事件创建失败、等待中断和正常退出也走拥有权检查。正常退出同步释放拥有权，迟到完成不能再次恢复或清除新状态；用户或其他应用的新复制始终优先。投递等待期间拒绝新的捕获、手动复制和重叠投递。

macOS 未提供把焦点检查、剪贴板和事件发送合成原子操作的接口，跨进程最后观察与实际操作之间仍存在窗口。固定等待也不保证目标已读取；离线检查只证明调用了按键发送边界，不能宣称真实控件已接收。强制终止进程时的剪贴板恢复尚未验证。

切换窗口/焦点、剪贴板改变、权限不足或无法确认目标时，结果保留在面板中并显示原因；不强行激活旧应用、不覆盖新剪贴板。手动“复制”是用户明确操作，可覆盖剪贴板。只恢复自己持有的剪贴板写入。

freeze-01 独立验收曾发现 VP005-I-001：原生采集仅用 AX 角色白名单判定可编辑，未确认只读/禁用状态，反例仍调用粘贴边界。原开发已按上述能力判据修复默认采集入口，新增 9 项默认入口检查，44 项应用自检及复制的 11 项原独立反例均通过；原验证会话已对 freeze-02 独立复核通过，20 项独立检查及完整离线回归全部通过，验收仅限 VP-005 本地离线范围。新增测试执行实际默认采集代码，并在下层模拟 AX IPC；内存 AppKit 控件的角色和写入能力不能外推真实窗口及跨应用兼容性。

## 7. 使用与设置

菜单提供开始/结束录音、取消、显示结果/设置、登录与权限状态、退出。设置包括自动粘贴、语言、快捷键预设；设置本机保存。默认关闭转写历史，无遥测。

CLI 提供录音文件转写用于可复现对照。`demo` 必须清晰标为离线模拟；`doctor` 只报告环境/登录文件存在性，不输出 token。开发模式打包到 `dist/VoicePort.app`；用户以后可自行移入 Applications，系统权限应绑定稳定 bundle。

## 8. 验证与质量评估

离线必测：登录文件缺失/畸形/API-key/到期；multipart 二进制/语言字段；大小/扩展名；HTTP 401/403/429/5xx；重定向拒绝；空/畸形响应；取消；目标切换/焦点切换/安全输入/剪贴板改动；旧结果不可投递。

本机实测矩阵：Orca、常用编辑器、浏览器 textarea；分别验证普通输入、多行、切换应用、同应用切焦点、录音中复制其他内容、断网、取消/重试、权限拒绝。无法运行的格子标待验收。

识别评估用同一录音：中文需求、英文标识符、路径/数字/否定词和口头修正。人工记录关键 token 正确率、语义错误、录音停止到出字延迟、粘贴成功率，与 Codex App 对照。首轮样本建议 5–10 段，不预设优于 Codex 的结论；不把原始音频/口述内容提交仓库。

## 9. Loop 与方案修订

每轮只交付一个可验收任务。先读取当前状态和工作区，执行代码、验证，再写证据和方案差异；失败返回修订，外部阻塞写解除条件并选其他 ready 项。任务全完成或只剩无新事件的外部阻塞时停止，不无限增加需求。

方案调整记录“触发证据 → 变更 → 对验收的影响”。阶段顺序可变，产品目标不能靠自报完成改变。主协调者维护一个产品任务账本，通过 Orca 分派不同真实开发、独立验证和按需评估会话；各会话按文件归属工作，开发自检后冻结完整输入，再独立验收。问题返回原开发会话，修订后由原验证会话复核。

工作会话完成事件唤醒协调者；动态 `/loop` 仅作持续任务恢复和长延时兜底。角色身份、Task/Dispatch、回执和版本映射保存在私有注册，开发脚本只维护产品账本。会话创建、消息和唤醒依赖 Orca 宿主；没有安装独立后台模型服务，宿主退出后不承诺继续运行。

## 10. 修订记录

- 2026-11-04 v0.1：从指定会话恢复目标；选择原生 Swift；凭据首版只读、刷新交回 Codex；原按钮适配、Keychain、公证留后续；加入焦点/剪贴板/取消保护与真实验收边界。
- 2026-11-04 实施修订：Command Line Tools 无 XCTest / Swift Testing，离线验收改用 swiftc 独立检查；自动粘贴移除换行和控制字符，手动复制保留原文；失败录音的手动重试只保留结果，不自动投向新的输入框。
- 2026-11-04 VP-004：受控测试复现服务取消后停留 transcribing，改为明确失败并可重试；控制器系统边界可注入，设置使用注入的独立存储；补显式 AX 标签；测试目录统一为 Tests 下不同子目录。
- 2026-11-04 VP-005 开发交付：旧行为实际复现退出通知、安全输入、同步恢复、最终目标复核和不可编辑控件 5 项失败；新增取消、迟到提示和剪贴板交互反例，35 项应用开发自检通过，等待冻结版本独立验收。补持久拥有权与同步退出恢复，保留 macOS 非原子观察和真实目标消费未验证边界。
- 2026-11-04 loop 修订：用户指出多会话缺失，纠正单协调者与单执行会话混同；已通过 Orca 实际分派开发和协议两个会话，交付后由不同真实会话独立验证。完整冻结除脚本摘要范围外另纳入 docs 与 README。
- 2026-11-04 VP-005 独立验收修订：默认 AX 角色判据被真实行为反例退回；原开发增加启用/写能力确认，原验证对 freeze-02 独立复核通过，保留真实跨应用接收未验证边界。
- 2026-11-04 VP-006 开发交付：采用版本化 Caches 根与精确归属标记，flock 覆盖录音到重试保留期，普通启动只清理严格超过 24 小时且无占用的已确认残留。16 项新增检查使应用检查达到 60 项；45 文件 freeze-01 已由原验证会话亲自完成完整回归及另 20 项独立检查，协调者核对后 finish VP-006。保留 ACL/断电/真实录音未验证边界；VP-007 外部阻塞、VP-008 等待依赖/同音频样本，loop 按停止条件等待解除事件。
