# VoicePort

macOS 全局语音输入工具：使用自己的 Codex / ChatGPT 登录调用云端听写，将文字输入 Orca 等 coding 软件。用户检查后自行发送。

当前桌面应用保留 Swift/macOS 原生外壳，默认转写已接到打包内 Rust 子进程。VP-006 Swift 行为基线、VP-009 Rust 核心/CLI、VP-010 会话/宿主协议已通过各自本机离线独立验收；VP-011 原生接入 freeze-01 曾因同一 child 重复 session 可误接旧文字而独立退回；修复后的 freeze-02 已由原独立会话完成缺陷及剩余矩阵复核，31 核心/73 应用与本轮离线检查通过，协调核对后已完成 VP-011。VP-012 完整开发包的 freeze-02 已通过原独立会话验收和协调核对，账本已完成：762文件/270目录、schema2目录模式指纹、双产物签名对应、120个metadata依赖的687份材料，以及41 Rust、31核心、73应用、4 loop、11 packaging和真实25步入口符合预期。首次freeze-01三类四反例的退回历史保留；新冻结独立复核了原缺陷、23项清单边界、6项入口边界、20个有效重签变异包、24项源许可反例及完整条款充分性。Windows平台任务VP-013已开始，当前可用主机为macOS，尚无已连接Windows环境。真实录音、账号转写和 Orca 接收待用户参与验证。产品目标同时支持 macOS 和 Windows，当前优先 macOS；Windows完整默认目标类型检查已通过，链接和运行尚未通过；当前部分实现曾因取消后普通轮询无法恢复新任务而被独立退回；修复后的完整freeze-02已由原独立会话确认缺陷修复，协调核对后接受本次阶段检查。原11项断言、相邻取消/退出路径及当前Mac25步/52Rust和完整Windows目标检查通过。VP-013整体仍未接受。续作03已交付后台执行器、结果回收和显式手动重试，新完整freeze-03已由原独立会话退回：VP013-I-002确认工作线程的is_finished可先于线程局部资源清理结束，同步join使普通Poll、取消Poll及孤立任务回收等待，实际门闩反例成功预期0、退出1。独立53项Rust、Mac25步和完整Windows默认目标检查通过，未覆盖该行为失败；协调核对后已真实retry/start返修attempt。返修02已交付并通过材料核对：join移到单一进程级清理线程，保留唯一Job/File/许可直到真实停止；54项Rust及完整Windows默认目标检查通过。原样after探针仍101，孤立任务恢复尾增加显式Retry的适配保持三路响应断言且通过，原失败不改称成功；新完整冻结仍须原独立复核。录音、安全存储、保护投递及完整Windows链接/实机/打包仍未完成。迁移步骤见 [跨平台路线](docs/CROSS_PLATFORM.md)。方案与实施记录见 [完整方案](docs/SOLUTION.md)、[Loop 规则](docs/LOOP_METHODS.md)、[任务计划](task_plan.md)、[进度](progress.md) 和 [验证与解除条件](docs/VALIDATION.md)。

最新协调状态（2026-11-05）：Windows freeze-04 独立复核发现 **VP013-I-003**，清理线程仍在 TLS 析构中时可能误判停止并触发 panic；返修03已交付显式请求所有权与真实 receiver 生命周期方案，原清理 TLS 探针四例自检均通过，仍须新完整冻结和原独立复核，VP-013 整体未接受。主协调者已按原 Run 完成交接；当前运行阈值与项目默认值统一为每个协调会话完成 **5 次压缩后轮换**，后续继任继承。达到阈值的最后一轮直接保存和交接，不额外压测或强制压缩，见 [协调者轮换](.claude/COORDINATOR_ROTATION.md)。原 14 项隔离测试及真实 PreToolUse hook 加载证据保留，不为验收轮换额外触发压缩。

## 构建与启动

需要 macOS 13+、Swift 6 Command Line Tools 和 Rust 1.95+。正常打包构建 Swift 与 locked Rust release，安装并先签名 `Contents/Helpers/voiceport-rs`，再签外层 .app；本机 arm64 构建。`--skip-build` 使用现有 release 产物，要求 Swift 和 Rust 产物都已存在。运行已打包应用无需安装 Rust 工具链。

```bash
bash scripts/package_app.sh
open dist/VoicePort.app --args --show-window
```

应用常驻菜单栏。点好目标输入框，按 **Control + Option + Space** 开始录音，再按一次结束并转写。也可以使用菜单开始/停止。首次录音申请麦克风权限；自动粘贴需在结果/设置面板授予辅助功能权限。冲突时切换为 Control + Shift + Space。

登录由 Codex 管理，请先在自己的终端运行 `codex login`。首版读取 `CODEX_HOME/auth.json`，默认 `~/.codex/auth.json`，不修改该文件。只支持 ChatGPT 文件登录；API key 模式、仅存于 Keychain 的登录和应用内 token 刷新尚未支持。

录音结束后上传至 ChatGPT。原应用、焦点和剪贴板仍匹配，且能确认控件已启用和支持 AXValue 写入时才发送 Command + V；不发送 Return。无法确认这些能力的编辑器会保留结果，可在菜单的“结果与设置”中复制；这不表示该编辑器不能键盘输入。自动粘贴把换行和控制字符转为空格，手动复制保留原文。粘贴事件成功不等于每个输入控件都已接收文字。

## CLI

CLI 名为 `voiceport-cli`，避免在 macOS 大小写不敏感磁盘上与应用 `VoicePort` 冲突。

```bash
swift run voiceport-cli --help
swift run voiceport-cli demo
swift run voiceport-cli doctor
swift run voiceport-cli transcribe /path/to/audio.m4a --language zh
```

`demo` 明确为离线模拟，不录音、不读取凭据、不调用云端。`doctor` 只报告登录文件是否存在。可使用 `--auth /path/to/auth.json` 指定自己的登录文件，勿把凭据复制到仓库。支持 m4a/mp4、wav、mp3、webm、ogg、flac，最大 20 MB。

## Rust CLI（VP-009 本机离线独立验收通过）

需要 Rust 1.95+。根 workspace 包含 `voiceport-core`、`voiceport-cli` 和开发中的 `voiceport-windows`，版本由 Cargo.lock 固定；当前 macOS 应用通过独立的 Rust host 转写，Swift CLI 保留为行为基线与对照工具。Windows crate已有转写阶段控制器、PCM WAV编码及显式 `--run` 的Win32托盘/消息循环/快捷键动作/手动复制壳。完整默认功能Windows目标类型检查已通过，但链接仍缺GNU运行库、没有PE成品或Windows运行；录音和安全存储未实现；后台转写调度已交付，但因线程清理期间同步join阻塞未通过独立验收，尚不能用于Windows桌面听写。

```bash
cargo build --workspace --release --locked
./target/release/voiceport-rs --help
./target/release/voiceport-rs demo
```

`doctor [--auth PATH]` 只检查登录文件是否存在，不读内容、不证明有效。`transcribe AUDIO [--language zh] [--auth PATH]` 会使用自己的 ChatGPT 文件登录上传指定音频，运行示例为：

```bash
./target/release/voiceport-rs transcribe /path/to/audio.m4a --language zh
```

默认路径为 `CODEX_HOME/auth.json`，否则平台用户目录下的 `.codex/auth.json`。不刷新 token、不自动重传、不录音或投递；Ctrl-C 取消网络等待与响应读取。退出码为 0 成功、1 运行错误、2 参数错误、130 取消；错误写入 stderr，转写原文写入 stdout。

固定 HTTPS 端点，禁重定向、cookie 持久化和自动系统/环境代理。支持的格式与 Swift CLI 相同，实际音频读取上限 20,000,000 字节；认证文件及保留的响应正文分别限 1,000,000 字节。连接/读取停顿/总请求上限分别为 15/90/110 秒，本地文件读取不计入网络超时。代理/TLS 兼容性、Windows 构建运行及系统集成尚未验证，详见 [跨平台路线](docs/CROSS_PLATFORM.md)。

## Rust 子进程接口（VP-010 本机离线独立验收通过）

`./target/release/voiceport-rs host [--auth PATH]` 使用 stdin/stdout JSONL 管道。启动只输出 `ready`，不读取登录或音频、不联网；只有明确的 `transcribe` 或 `retry` 命令调用转写。可依次输入以下两行检查空闲状态并退出：

```json
{"version":1,"action":"status","request_id":"q1"}
{"version":1,"action":"shutdown","request_id":"end"}
```

协议包含转写、手动重试、取消、状态和退出；完整字段及事件见 [宿主协议 v1](docs/HOST_PROTOCOL.md)。输入帧最多 16,384 字节，输出帧最多 2,000,000 字节（均不含 LF），每帧写出确认总上限 5 秒。每个进程最多保留 1,024 个不同有效请求 ID，达到预算后返回错误并正常退出，宿主须处理重启。会话身份只在同一子进程内有效，重启后须以新进程身份隔离旧回调。手动重试结果带 `is_retry=true`，原生宿主须关闭自动投递；Rust 子进程不删除传入的音频文件，文件持有与清理由原生层负责。macOS 原生接入已通过 VP-011 本机离线独立验收；完整开发包已通过 VP-012 freeze-02 本机离线独立验收，Windows 未验证。

原生 bridge 按需直接启动打包 helper，参数仅 `host`；缺 helper 或进程失败会保留录音并提示手动重试。父端只接受完整 LF 帧，EOF 丢弃未完成片段；后台串行处理管道并丢弃 stderr，不把原始协议或诊断展示到界面。ready、每次写入、每次控制回复各限 5 秒，终态等待另限 130 秒；启动控制阶段可另加 15 秒，异常取消/退出还会增加控制等待及最多 4.05 秒的终止/回收等待，因此 130 秒不是端到端总时限。同步系统调用不保证可中断。

取消先失效本地会话，确认子进程停止后才清理音频、释放占用；无法确认时保留录音占用并禁用新录音/重试。正常应用退出异步等待停止确认。失败重试保留录音时的语言；手动重试始终关闭自动投递。子进程丢失或预算退出不自动重传，用户显式重试可以新建进程并重新提交。

## 验证

```bash
# Rust workspace tests、格式和严格 lint；使用合成登录/文件和本次回环服务
bash scripts/test_rust.sh

# Rust release 构建（不在上述脚本内）
cargo build --workspace --release --locked

# 核心认证、请求/响应、异步网络、取消和投递反例；不需要 Xcode 测试框架
bash scripts/test_core.sh

# 控制器/投递、默认目标采集、隔离录音存储与子进程锁；不录音、不访问一般剪贴板
bash scripts/test_app.sh

# Loop 状态恢复、依赖、暂停和迟到结果
python3 -I -m unittest discover -s Tests/LoopTests -v

# Release 构建、CLI、.app 打包/签名和启动检查
python3 -I scripts/verify.py --app-smoke
```

本机 Command Line Tools 不含 XCTest / Swift Testing，因此核心检查用 swiftc 编译独立断言程序。完整报告和日志保存在被 Git 忽略的 `.voiceport/evidence/`；这套离线验证不调用真实账号，不替代麦克风/Orca 实测。

## 开发 loop

```bash
python3 -I scripts/loop_state.py status
```

任务定义在 `automation/tasks.json`，执行状态在 `.voiceport/loop-state.json`。主协调者一次选择一个依赖满足的产品任务，通过 Orca 分派不同真实开发、独立验证和按需评估会话；开发自检后冻结完整输入，再独立验收，返修回到原角色上下文。工作会话完成事件唤醒协调者，动态 `/loop` 作恢复兜底。状态工具只维护账本，宿主退出后不承诺继续运行。角色、暂停、外部阻塞和停止规则见 [Loop 契约](docs/LOOP_METHODS.md)。

## 数据与已知边界

- 音频上传到 `chatgpt.com/backend-api/transcribe`，使用当前用户自己的 ChatGPT 登录；接口未公开承诺稳定性和扣量规则。
- 与 Codex App 的模型、识别质量和用量是否一致，尚需同音频对照，不能仅凭接口地址判断。
- 默认无转写历史或遥测，结果仅保留在内存。录音位于 `~/Library/Caches/io.voiceport.recordings-v1`；管理目录使用 POSIX 0700，录音、归属标记和占用文件使用 0600。成功、取消和正常退出时尝试删除本次录音，失败时为手动重试保留。
- 普通启动只尝试清理创建时间严格超过 24 小时、归属标记有效且未被活动会话占用的残留；停止录音后的上传和重试仍持有占用锁。`--smoke-test` 跳过清理。未知内容、链接、异常权限/时间、旧版无标记目录均保留，清理失败也可能留下文件；这不是到时必删的保证。
- 当前验证覆盖本机隔离目录中的归属、POSIX 权限和跨进程占用；ACL、断电持久性、真实编码器输出及其他 macOS/文件系统兼容性仍待验证。
- 开发包采用本机 ad-hoc 签名，尚未公证；更新二进制后系统可能要求重新授权。Intel、所有应用输入框兼容性尚未全面验证。
- Orca 原生麦克风按钮的后端适配属于后续工作；首版使用 VoicePort 独立快捷键。

参考及许可见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 完整离线开发包验证

`python3 -I scripts/verify.py --app-smoke` 实际执行 locked Rust/Swift 检查、正常构建打包、两种 release CLI、许可材料/签名/helper 对应和安全菜单 smoke，保存完整起止产品清单与原始命令日志。验证强制 Cargo 离线缓存及本次自建合成 CODEX_HOME/TMPDIR；不运行普通 GUI 或真实转写。完整资源、时限、skip-build 和许可边界见 [PACKAGING.md](docs/PACKAGING.md)。普通开发运行属于自检；VP-012 freeze-02已由另一真实会话亲自运行最终入口、独立反例和完整许可审查后接受，协调核对后完成账本。接受绑定保存的完整快照；后续实现变更仍需新的冻结与独立验收。
