# macOS 与 Windows 路线

更新：2026-11-05。用户要求 macOS 和 Windows 都要支持，目前优先 macOS，并明确启动 Rust 新版本 loop。Swift/macOS 基线提交 6d6f42c 已推送 GitHub main；Rust 工作在 feat/rust-core 分支，VP-009 核心/CLI、VP-010 会话/宿主协议及 VP-011 macOS 原生接入均已通过各自本机离线独立验收。VP-011 freeze-01 曾因父端 session 复用防护缺失而退回；修复后的完整 freeze-02 已由原验证复核缺陷及全部剩余离线范围，协调核对后实际完成账本。VP-012 freeze-02已由原独立会话接受，协调核对762文件/270目录、30020历史普通材料、23554新证据及实际日志后已真实finish。23项清单、6项入口边界、20个有效重签变异包、24项源许可反例、实际全新Cargo target的25步入口及完整许可充分性通过；41 Rust/31核心/73应用/4 loop/11 packaging符合预期。原635材料保留并新增52份归属文件，120个metadata包的687材料全部来源对应。实际release normal/build树为103包，normal边为100包，workspace含dev树为107包；metadata另含13个未启用可选节点，不能视为实际链接清单。旧freeze-01三类四反例及协调摘要排序错误保留。接受绑定原完整快照，本次四文档状态更新另记。Rust 和 Windows 的结果分别验证，现有 Swift 验收不能替代。

当前补充（2026-11-05）：PR1 已合并并同步 main，当前修复分支为 `fix/windows-cleanup-stop`。freeze-04 被原独立以 VP013-I-003 退回：合法清理线程在 TLS 内尚可领取请求时误判停止，Poll/Exit/orphan 会 panic。返修03改用请求显式双重 Job/许可所有权和真实 receiver 生命周期；原四例自检由0/1/1/1变为全0，健康后续请求及原11断言保持。完整freeze-05现已由原独立验证者确认VP013-I-003修复并通过阶段检查：亲自运行原样四例、所有权/健康后续/丢失循环/自动释放及当前Mac25步55Rust和完整Windows默认目标检查。协调核对后接受返修阶段，不能外推为Windows产品接受。

## 执行架构

采用 Rust 共享核心与各系统适配层，逐步迁移已有行为。根 Cargo workspace 下建立 crates/voiceport-core 和 crates/voiceport-cli，独立二进制命名 voiceport-rs，避免与 Swift 的 VoicePort/voiceport-cli 碰撞。macOS 初期保留现有 Swift/AppKit/SwiftUI 外壳，后端通过有界 JSONL 子进程协议接入 Rust，明确会话身份、取消、重试和退出资源归属；凭据不通过 argv 传递，应用直接启动打包内二进制，不拼接 shell。

Windows选择增量Rust/windows-sys原生适配，直接调用未改动的共享TranscriptionClient API，避免再写一套JSONL父端；macOS保留现有Swift外壳。当前voiceport-windows crate已有转写阶段控制器、PCM WAV编码器，以及显式--run的Win32消息循环、托盘、线程归属快捷键分派、手动结果复制和取消退出协调。续作03进一步交付生产后台转写执行器、timer结果回收及显式手动Retry接线，完整freeze-03已被原独立会话退回：VP013-I-002在真实OsSpawner线程已进入且仍受门闩保持的线程局部析构期间，普通Poll、取消Poll及孤立任务回收同步join均阻塞，三路成功预期0而实际1；独立Mac25步/53Rust与完整Windows默认目标检查通过不覆盖该失败。协调完整核对777输入/59870历史普通材料/760保护及1489新证据后，已真实retry/start返修attempt1ed6204e-5637-4c5e-b059-51ac4256a4e6，将线程完成确认/非阻塞回收交回原开发；需保留唯一Job/File/进程许可到真实停止，不能以完成槽或超时假确认。返修02现已由原开发交付并完成协调材料核对：3修改1新增、778输入/275目录、772保护及62159历史普通/10链接/6特殊保持，1258新证据一致；单一进程级清理线程执行真实join，UI只收停止确认。原样after探针101原样保留，仅孤立任务恢复尾增加显式Retry的适配三路响应均0ms并exit0，原11断言及最终54Rust/fmt/严格clippy/完整Windows默认目标检查通过；新完整冻结交原独立复核前不接受本次修复。原生路径已完成目标类型检查，但未成功链接或在Windows运行，录音/安全存储尚缺，不能用于桌面听写。没有引入Tauri/Electron或重写macOS。

VP-013续作02交付6改动/3新增，775文件/275目录；756保护及54581历史普通材料/10链接/6特殊不变，161新证据已由协调逐项核对。官方Zig0.17.1归档及编译器字节核实后，真实ring C编译和完整默认功能Windows GNU all-target类型检查通过，包含原生壳/共享service/tests，无须关闭核心。实际链接先缺dlltool，配置Zig真实工具后仍因libpthread.a/msvcrt运行库缺失退出101；没有PE成品和Windows运行。首份缺gcc的101及adapter-only通过记录保留，不覆盖最终完整类型检查结果。全新Mac目标15步入口通过41既有Rust、7controller/WAV和2初版actions检查及原Mac包/签名、31core/73app/4loop/11packaging；随后修正退出/取消优先于合并复制事件，补跑3最终actions加7controller/WAV、lint/fmt和最终完整Windows目标检查。原生FFI仍仅编译未执行；设备采集、ACL/共享/reparse-safe存储、后台服务执行器、保护投递及Windows打包仍缺。完整freeze-01已由原独立角色审查并因VP013-I-001退回：取消时任务仍存活，任务随后释放后普通Actions Poll不推进停止，下一次录音持续Busy。独立真实新target完整Windows类型检查和Mac25步（51 Rust/31core/73app/4loop/11packaging）均通过，但不能覆盖该行为失败。协调已核对775输入/54717历史材料/1400新证据与原失败日志，并真实retry/start新attempt0d21e578-dac8-46ff-aa22-6d19b30b68e1，回原开发修复后重新完整冻结、交原独立复核。返修01已交付，协调核对三项允许修改、772保护输入/56911历史普通材料/10链接/6特殊和398新证据一致。同一独立11项断言保持原字节，修前Cargo101/collector1、修后Cargo0/collector0；新增生产回归后工作区52 Rust、fmt/严格clippy及完整默认Windows all-target检查通过。普通Poll推进真实stop确认，开放状态始终false并保留Ready结果，closing确认才true；未新增链接或实机证据。完整freeze-02已由原独立task_42c9f403abd7 / ctx_d5ff27ba1a13确认VP013-I-001修复，无新已实现范围阻断；原11项断言、4取消恢复组合/3优先级/2关闭释放对照、当前Mac25步/52Rust/31core/73app/4loop/11packaging和新目标完整Windows默认检查通过。协调首次核对775输入/275目录、57313历史普通材料/10链接/6特殊、768保护及1234新证据74178041字节后，仅接受返修阶段检查；已归档原快照，同一attempt继续后台执行器/结果/手动重试，VP-013仍running且全产品未接受。独立auditor子组3→实际4的测试断言修正和原失败保留，未改产品。自动投递和清理保持禁用。687材料保持，本轮manifest仅build_inputs变化；首轮6项metadata新增dev分类的精度更正保留。边界详见 [Windows进展](WINDOWS.md)。

| 部分 | 共享范围 | 平台责任 |
|---|---|---|
| 登录文件 | 解析与校验、API-key 拒绝、脱敏、到期判断 | 各系统定位用户目录/文件，验证实际 Codex 登录存储；Keychain 等后续单独适配 |
| 转写 | multipart、端点限制、响应/错误、取消、手动重试 | 系统代理/TLS 与打包环境验证；保留不自动重传/刷新约束 |
| 录音流程 | session、状态机、时限、迟到结果拒绝 | 设备采集、编码、系统权限；跨平台音频采集不等于已有 AAC 编码 |
| 暂存 | 生命周期、保留阈值与保守归属规则 | macOS POSIX/flock 与 Windows ACL/文件共享/锁语义分别实现和验证 |
| 投递 | 原目标匹配、未知即回退、仅投递不发送、新复制优先 | macOS Accessibility/Command+V，Windows UI Automation/Ctrl+V；各自验证焦点、密码字段、事件权限和剪贴板格式 |
| 界面/托盘/快捷键 | 业务动作与可见状态约定 | 各系统注册、冲突、托盘交互和权限提示 |

Rust 能复用业务流程并减少两套实现漂移，但无法把 macOS 的权限、AX 属性或 flock 直接搬到 Windows。尤其不能把无法确认的 Windows 输入控件一律当作可安全自动粘贴；没有明确能力时保留结果供手动复制。Windows 的进程权限级别、UI Automation 能力和剪贴板格式需要独立反例，不能以 macOS 验收替代。

## 分阶段实施建议

1. 保留当前 Swift 可运行版本及其验收记录作为迁移对照基线，先提交到仓库。迁移期间每份报告注明实际运行的实现，不能把现有通过项改名为 Rust 的结果。
2. 定义语言无关的行为样例和平台接口，覆盖凭据、请求/响应、取消/重试、session 失效、投递资格与音频保留规则。对复杂系统行为保留当前独立反例。
3. 建立 Rust 转写核心与 CLI，首先迁移不依赖界面的认证、HTTP 和错误处理。选择依赖时核对重定向、凭据/缓存、取消和超时行为；用离线 transport 与固定样例验证，再与 Swift 基线对照。此步骤不需要真实账号或录音。
4. 在 macOS 接入 Rust 核心，明确跨进程/库边界的取消、错误、session 身份、音频所有权和退出回收。凭据不在命令行参数或日志中传递。继续复用现有原生录音和受保护投递，完成整包回归后再逐步替换其他共享流程。
5. 在 Windows 环境实现并验证平台适配：真实编译/运行、录音格式、托盘与全局快捷键、权限/焦点/密码控件、剪贴板恢复及退出资源清理。仅在 Mac 交叉编译不算 Windows 运行验收。
6. 两平台分别完成用户参与的真实录音、账号转写和目标接收；识别质量继续使用同音频对照，不从实现语言推断准确率或计费归属。

## Rust 核心与 CLI 的实际实现

VP-009 已交付两个 workspace 成员及 Cargo.lock，要求 Rust 1.95+、edition 2024。`voiceport-core` 使用 reqwest 0.12.28/rustls、Tokio/tokio-util、Serde/serde_json、base64 和 dirs；tempfile 仅用于隔离测试。实际依赖版本由 Cargo.lock 固定，运行与验证命令见 [README](../README.md)。

核心逐请求只读加载 ChatGPT 文件登录，拒绝 API key 模式，解析 JWT 到期并保留 30 秒余量。生产端点固定为 HTTPS；不跟随重定向、不自动重试/刷新、不保存 cookie，禁用自动系统/环境代理。音频实际读取上限为 20,000,000 字节；认证文件和保留的响应正文分别上限 1,000,000 字节。响应超过上限时不继续累积，HTTP 栈仍可能交付一个使其超限的 chunk，因此不是精确的网络收包字节上限。

连接超时 15 秒、每次读取停顿上限 90 秒、HTTP 请求至正文读取总上限 110 秒；读取超时随成功读取重置，不宣称与 Swift URLSession 的 request/resource 计时完全相同。本地同步文件读取在网络计时前发生，仅有字节限制及前后取消检查；单次文件系统调用不能中途取消，同 UID 并发替换下不承诺文件身份/内容原子性。CLI Ctrl-C 对等待网络/正文读取显式取消，退出 130；已被服务接收的上传不能远程撤回。

## Rust 会话与宿主协议（VP-010）

已实现 `voiceport-rs host [--auth PATH]` 的 stdin/stdout JSONL 接口，启动只发 ready，不读登录/音频或联网。会话由进程级递增且不回绕的 generation 识别，取消/替换先失效再取消，迟到完成不能改写新会话。仅当前失败会话可按同路径/语言手动重试，获得新身份并标记 is_retry；成功后不保存转写历史或重试路径。Rust 不拥有宿主录音，不删除传入文件。

[协议 v1](HOST_PROTOCOL.md) 明确了 16,384 字节输入帧、2,000,000 字节输出帧、容量 1 的输入/输出队列、单点 writer 与每帧 5 秒写出确认总 deadline。请求 ID 进程终身去重，最多 1,024 个，预算用尽后报错并退出 0；EOF/shutdown、慢读/EPIPE、Ctrl-C 的行为和退出码均有约定。stdio 使用两个进程级 detached OS 线程；这是随子进程退出的处理器，不适合在长驻进程内反复调用。已接受客户端中的单次同步文件调用仍不能中断。

VP-011 接入时须同时校验子进程身份与协议会话身份，重启后不得复用旧关联；处理请求预算退出、异常 EOF、超时和取消，重试不得自动投递。Foundation Process 直接启动打包内二进制，保留 Swift 的录音 lease、焦点与剪贴板保护。VP-010 的 65 文件完整冻结已通过独立验收：41 项既有检查、6 项独立 Rust 检查、46 个有效断言的真实子进程用例符合预期，其中 23 个使用未修改 release。实际大帧输出中断可留下不完整尾片段，VP-011 必须在 EOF 丢弃，不能把片段当结果。每帧 5 秒允许连续小帧各自及时完成，不是整个进程的生存时限；原生请求还需独立的总 deadline。原生接入已在 VP-011 独立离线验收；当前整包交付及后续 Windows 仍需各自验收。

## 当前状态与边界

- 已有实现：Swift 核心、CLI、macOS 菜单栏应用及离线验证；辅助脚本使用 Python/Shell。
- VP-009 本机离线独立验收已通过：59 文件 freeze-01 的 36 项 Cargo 检查、fmt/clippy/release、8 组独立核心反例、37 个 release CLI 离线用例、真实 release SIGINT 130 和 4 项 loop 回归符合预期。历史 557 证据文件与 25 个 Swift 输入保持不变；各组覆盖有重叠，不相加为单一用例总数。
- VP-010 本机离线独立验收已通过并完成账本：65 文件/21 目录、890 普通历史证据及 2 个链接/4 个特殊文件均保持一致，实际命令和进程日志由协调者核对。各组检查有重叠，不相加为独立覆盖总数。
- VP-011 freeze-02 本机离线独立验收通过并真实完成账本。freeze-01 的 VP011-I-001 退回历史保留；返修使用有界 UInt64 高水位及 canonical 身份严格推进，原验证亲自确认同child重复身份被拒，原控制器保留第二音频且不错误投递，新child手动retry不自动投递。69文件/22目录、1690旧普通证据和1362项新材料一致；原5项反例/正控制、补齐后的79项矩阵、额外bridge7项、含原对照的controller/storage13项、raw helper3项及31核心/73应用/八命令符合预期，各组覆盖有重叠。ready/写/控制分别5秒，terminal另130秒，控制及回收另计；替身、有限资源采样和活跃普通GUI未验边界见 [验证记录](VALIDATION.md)。VP-012完整Rust/macOS离线开发包已在freeze-02接受；当前VP-013 Windows适配及后续VP-014真实录音/云端/目标接收分别验收。
- 当前 VP-007/VP-008 的真实环境条件及已接受 VP-006 快照保持可追溯；跨平台目标不把这些未验项改为完成。
- Rust 迁移 loop 已按用户指令启动，分成 VP-009–VP-014，一次一个 running 产品任务，继续采用原开发、冻结、原独立验证和原角色返修的多会话机制。Swift 基线的接受快照保持可追溯；开发分支的实际默认后端已切换 Rust，当前实现是否通过由本轮独立验收判断。
- VP-013已真实start，当前原生Windows系统适配与构建/运行仍未验。只读盘点确认本机Darwin arm64、仅安装aarch64-apple-darwin Rust target，所查常见Windows工具/VM应用未发现，Orca已登记SSH主机均未连接、无配对environment；未连接这些主机或扫描私人配置。先推进现有条件可验证的适配工作，实际Windows构建和运行仍须独立留证，缺环境时记录阻塞而不伪称完成。

当前原生方案见 [SOLUTION.md](SOLUTION.md)，实际验证状态见 [VALIDATION.md](VALIDATION.md)。
