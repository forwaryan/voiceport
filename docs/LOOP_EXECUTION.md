# Loop 执行、证据与资源

本页补充 [多会话 Loop 契约](LOOP_METHODS.md) 的可执行检查。适用范围是本地开发验证和产品账本；会话轮换仍按 [轮换规则](../.claude/COORDINATOR_ROTATION.md)。不安装新 scheduler，不扩大产品、Git、录音或外部动作授权。

## 采用 Magpie 的哪些经验

工作区归属、重型并跑及 CI 并行的来源固定在 `270235adbb44a9a3d0c195c87388ab77ed4b43dc`。既有日志与清理规则保留其固定提交来源。

| 来源 | 本项目落实 |
|---|---|
| [LESSONS 完整日志与失败识别](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/LESSONS.md#L373-L383)、[重试不能掩盖首次失败](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/LESSONS.md#L812-L814) | 每次验证使用全新目录，完整 stdout/stderr 写盘，报告保留逐项实际退出、超时与清理状态；账本保留旧 attempts。失败不自动重跑。 |
| [测试启动的资源必须结束](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/docs/code-standards.md#L33-L35) | 验证 runner 为命令创建独立进程组；超时/正常终止信号先 TERM、必要时 KILL，只操作自己启动的组。清理未知或残留后代不能通过。 |
| [工作区归属](https://github.com/yetone/magpie/blob/270235adbb44a9a3d0c195c87388ab77ed4b43dc/docs/code-standards.md#L247-L255) | VoicePort 的并行写任务独占各自的 Orca worktree，只读工作可共享，协调者收齐交付后集成冻结。见 [分派规则](LOOP_METHODS.md#31-一个逻辑问题多个独立归属的工作任务)。 |
| [同机重型检查并跑超时](https://github.com/yetone/magpie/blob/270235adbb44a9a3d0c195c87388ab77ed4b43dc/LESSONS.md#L70-L78) | VoicePort 的两个本地完整验证入口共用同用户的主机锁，轻量定向检查按资源预算并行。 |
| [CI 并行和取消](https://github.com/yetone/magpie/blob/270235adbb44a9a3d0c195c87388ab77ed4b43dc/.github/workflows/test.yml#L9-L26)、[评审后按 SHA 合并](https://github.com/yetone/magpie/blob/270235adbb44a9a3d0c195c87388ab77ed4b43dc/docs/code-standards.md#L126-L135) | 远端并行按实际 workflow 和 runner 预算执行，记下被取消的 run。合并沿用 VoicePort 的 [精确版本门槛](LOOP_METHODS.md#84-完成门槛与-git-接续)。 |
| [明确实际命令及未执行检查](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/docs/code-standards.md#L39-L46)、[reviewed head](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/docs/code-standards.md#L109-L119) | 完成证据记录命令、完整日志、输入清单及开始/结束摘要；报告的 checks 只列实际执行项。提交/输入变化使旧复核失效，已授权合并仍须 match-head 保护。 |
| [CI 为依赖安装设超时](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/.github/workflows/test.yml#L35-L44) | 本地每条验证命令使用明确期限，命令开始前保存 active_check，完成后立即保存结果。按本项目命令保留原有期限，不照搬 CI 的 15 分钟数字。 |

这些来源没有提供本项目的身份认证、独立验收或远端 CI 真值。摘要和状态检查不能代替实际验证者、宿主回执和精确提交的必需检查。

## 行为变更与文档复核

- **同一 PR 更新权威出处。** 修改前找到对应的行为文档，从文档跟到当前实现、调用方和测试。状态含义、选择条件、失败与兼容策略只在对应文档完整定义；使用指南讲操作，CLI/Skill 只保留接口或离线诊断需要的简述。改变这些约定时，同一 PR 更新出处并检查其他入口的引用。内部重构没有改变已记录行为时，不为凑清单改文档。计划与历史验证各自标明范围，不写成当前行为。
- **验收对照改前改后。** 交付说明写触发条件和用户可观察结果；从发生问题的入口追到输出或副作用，再核对共享 helper、同类调用方及预期不变的路径。实际变化用过去会失败的检查验证；纯文档纠偏用源码和历史版本证据说明，不冒称修复了运行时。不同入口有意不同则写清理由，不顺带扩改需求。
- **结果只写到证据到达的一环。** 区分返回成功、选路、请求提交、合成目标接收和真实用户目标接收。按用户使用的界面和条件验证最终效果；需要账号、真实录音、上传或输入授权但未获得时，记录未测与解除条件，不用本地单测或合成接收替代。以[投递状态](INPUT_DELIVERY.md#cli-状态字段与投递回执)为例，`clipboard_final` 是选路，`submitted` 是事件提交，两者都不是微信字段的接收回执。

参考 Magpie 的[子系统文档与评审规则](https://github.com/yetone/magpie/blob/235f50a7a83269ff3411265fbdc4db4af161d812/docs/subsystems/README.md#L24-L49)、[同类调用方检查](https://github.com/yetone/magpie/blob/235f50a7a83269ff3411265fbdc4db4af161d812/docs/code-standards.md#L12-L17)和[按用户场景复现](https://github.com/yetone/magpie/blob/235f50a7a83269ff3411265fbdc4db4af161d812/LESSONS.md#L93-L98)。真实返修、结论错误或验证遗漏记录到[已证实的反例](LOOP_LESSONS.md)，不安装每夜调度。

## 并行开发的逐会话核对

对照分派清单逐一查实际 Task/Dispatch、工作区与基线，按各会话最新的可核对证据记录进展。同一会话可能正在工作，但尚未到产品写入。交付仍按 [冻结和独立验证](LOOP_METHODS.md#3-分派依赖与冻结) 判定。

| 阶段 | 核对证据 |
|---|---|
| 窗口 ready 或 worker 启动 | 真实启动回执、任务关联、宿主与工作区匹配。`working` 只描述正在活动。 |
| 宿主绑定与 LoopX 领取 | 对使用 LoopX 的任务，分别读回实际宿主绑定、具名身份对应的任务领取与各自回执。详见 [LoopX 登记与领取](#loopx-登记与领取)。 |
| 产品写入 | 逐会话查看最新工具写入回执，再对照该任务允许的产品源码、配置或测试的实际差异。看可读取的交付补丁或获准只读的实际内容，包含相关新增和删除项。计划、说明、自报开始、仅绑定或领取的回执仍按其阶段记录。 |
| 验证通过 | 核对命令、退出、判据、实际输入版本和覆盖范围。开发自检与独立验证各自记录。只有对应版本已执行的实际检查可报通过。 |

按当前 Orca 版本的 `orchestration worker-read --dispatch <实际 dispatch>` 读取有界输出。游标绑定到上一次的输出源。收到 `source_changed` 时，去掉旧 `--cursor`，按新源重新读取，继续翻页只使用新回执给出的游标。原读数保留其来源。旧游标缺页、截断或短期静默时，标明证据待补，重新核对实际进展。实际标识、游标、原始输出和差异证据只进入私有交接。

### LoopX 登记与领取

先记本次 `loopx --version`，按已安装版本的帮助和实际回执核对目标及预期身份列表。`register-agent` 先用单身份预检核对参数形态，批量时每个身份各传一次 `--agent-id`，预检的 `requested_agents` 须精确匹配本次请求，并核对合并已有身份后的计划列表。空清单、缺项或意外身份先诊断。已有登记、身份冲突及全局写入权限按实际回执处理。

获准执行后核对 `registration_readback` 的 `requested_agents`、`source_registered_agents` 和 `global_registered_agents`。请求须与预检一致，两处实际登记须与预期结果一致，包含应保留的已有身份。`ok` 和 `verified` 单独出现不足以确认写入。保存原异常回执，未知或部分写入先读回实际项目与全局登记再决定修正，避免盲目重发。按具名 lane 重新预览，核对返回的身份及 `activation_state=selected`。出现 `legacy_unscoped` 时继续查登记和选择。

隔离 worktree 初报未连接时，先核对现有目标的 canonical route、实际项目和工作区、来源与全局登记中的已登记 lane。读回路由和目标归属后，使用对应 lane 核对宿主绑定及领取回执。已有目标或绑定的情况保留原身份和记录，路由仍无法确认时保留具体缺失条件。

分别保存登记与预览选中、实际宿主绑定、Todo 写回与领取、原生 LoopX 启动、Orca 启动的回执。Todo 完成须读回同一任务的实际状态，本地产品交付另核对实际改动和对应版本的验证。核对原生启动时读回实际宿主与工作区上的启动回执及运行状态。Turn 是否结算按该 Turn 的结束回执单独确认，宿主仍在运行时也可有已结算的 Turn。未知结果先查原回执和当前状态。

### 共享 provider 修订冲突

并行 lane 对同一 canonical Goal 的 provider 执行 refine 前，紧邻写入读取当前修订。若收到明确的 CAS 拒绝 `goal_acceptance_provider_revision_mismatch`，保留原请求和回执，重新读取该 provider 与当前 lane 的需求关联，核对原 operation ID 对应的更新是否已经落地。已落地时直接核对结果。只有确认原操作未产生写入、原内容与授权仍适用时，才用当前修订和同一 operation ID 有界重试。收到 `status=applied` 后，从 canonical 来源读回本 lane 的实际需求和 Todo 关联。结果未知或无法确认未落地时，继续核对原回执和实际状态。此恢复仅涵盖已证实未写入的修订冲突，权限、暂停和其他 lane 的归属仍按原约束。

## 账本：查询不写入，证据可追查

```sh
python3 -I scripts/loop_state.py status
python3 -I scripts/loop_state.py audit
```

`status`、`audit` 是只读入口。存在锁时使用共享锁；不存在账本或状态目录时只返回内存中的默认投影，不创建目录、锁或账本。其他生命周期命令仍仅由原授权协调者执行。

`finish TASK --attempt ACTUAL_ATTEMPT --evidence ACTUAL_REPORT` 在确认当前 running attempt 后，读取非空、普通、无最终符号链接的证据快照（最多 16 MiB）。读取期间发生变化则拒绝；成功记录规范绝对路径、`evidence_sha256`、`evidence_bytes` 到 attempt 和事件。大日志单独保存，由有界报告引用；不能以空占位文件完成任务。

`audit` 对全部已完成 attempts 重新读取证据并报告：

- `intact`：当前字节与保存的路径、大小、摘要相符。
- `missing`：原证据已不存在。
- `changed`：字节、大小变化，或路径被符号链接/非普通文件替换等无法核验。
- `unsealed_legacy`：历史记录没有保存内容摘要，完整性未知；不按当前文件补造历史证明。

任一任务尚未 done、done 没有对应完成 attempt，或任一历史完成证据无法确认 intact，`audit` 返回非零。开发进行中出现 incomplete 是实际未完成状态，不是要求自动 reopen、重新验收或改写历史。保留旧记录，报告对应缺失条件；只根据实际新任务和授权启动新 attempt。

**非空文件和摘要只证明记录时的字节及其后是否改变。** `finish` 仍不解析任意报告中的 CI、角色独立性或真实产品结果；协调者必须核对当前任务、冻结版本、指定验证者、必需 checks、失败历史与未测范围。把“FAIL”文本或未完成的 running report 保存为文件不会使它成为通过证据。

## 验证：逐项写盘，退出与清理分开记录

```sh
python3 -I scripts/verify.py
python3 -I scripts/verify.py --evidence-directory /absolute/new-owned-directory
python3 -I scripts/ci_checks.py
```

按任务入口和证据需要选择完整套件。`ci_checks.py` 的完整入口沿用已有 release 构建作为前置，内部并行检查由 `--workers` 控制。轻量定向 Python 检查可单独运行，协调者按实际 CPU、内存、构建目录和证据归属安排。远端 GitHub Actions 按现有 workflow 与 runner 资源执行，工作流的并行/取消记录及合并接受按 [远端 CI 规则](LOOP_METHODS.md#8-远端-ci-修复与接受闭环) 核对。

完整验证入口调用 [verification_run.py](../scripts/verification_run.py)：

1. 两个完整入口先取得本机同 effective UID 的 `/tmp/voiceport-verification-<uid>/verification.lock`，再取得原有 `.voiceport/verification.lock`，两者均为非阻塞独占锁。任一冲突返回 1 并说明原因，检查尚未启动也没有创建本次证据目录。没有自动排队或 timer。
2. 证据目录必须全新；在运行命令前先写 `status=running`、`passed=false` 的报告，随后保存 active_check 的命令、开始时间、期限与日志路径。
3. stdout/stderr 直接写入该次检查的独立日志，完整输出不堆在 Python 内存中。仅需解析的 metadata/协议输出才有界读回，超过 1 MiB 拒绝解析并保留完整原日志。
4. 每项结束立即记录实际 `exit_code`、开始/结束时间、超时/中断标志、日志字节数/摘要与进程组清理结果。启动失败没有进程退出码，记 null；不把人为的 124/127 当作子进程真实退出码。
5. 超时、收到 SIGINT/SIGTERM、意外残留同组后代或清理未确认都不能通过。一个检查失败后不再启动后面的检查；checks 中没有的项目没有执行，不能写“全套通过”。
6. 只有已执行检查满足判据、输入开始/结束清单一致，最终报告才标通过。目录和失败日志不被下次运行复用或覆盖。

主机锁使用系统 `/tmp`，在 macOS 与 `/private/tmp` 是同一路径身份。目录与锁文件须由当前 effective UID 持有，权限分别为 `0700` 和 `0600`，拒绝最终路径上的符号链接及异常锁文件。`TMPDIR`、`CODEX_HOME` 和 checkout 路径都不会改变共享锁域。锁文件正常保留，由内核 `flock` 判断占用，禁止删除或替换文件绕过互斥。

锁覆盖同一 POSIX 主机共享该 `/tmp` 的同用户完整入口。不同用户、其他主机、手工 cargo/构建命令及旧版本的其他 checkout 不参加共享锁。同 checkout 的旧入口仍受原锁约束；CI runner 启动的内部 unittest ownership 与 packaging shard 复用父入口的资源时段，不重新加锁。

锁外的手工重型工作与 GUI 检查由协调者按实际资源安排互斥时段。在私有交接中登记共享资源、持有者、检查范围和释放条件。争用同一构建目录、桌面会话、焦点、剪贴板、全局快捷键或被测 App 实例的检查须排开。现有完整入口的文件锁覆盖上述验证入口，独立 worktree 中的手工 GUI 操作仍需协调实际桌面资源。锁冲突时按回执记为未启动。确认持有者实际退出、资源释放和必要清理后，再次执行时用新的验证记录保存实际结果。资源被占用时，仍可在其他工作区继续已授权的实现、文档和轻量验证。

普通 SIGINT/SIGTERM 会使验收失败并留下 interrupted 结果。两层锁覆盖正常的有界清理与报告收尾。清理与报告写入期间再次收到这两种信号时，先记录请求，待有时限的清理和当前写入完成后退出；日志收尾或等待之外中断也须保存最终报告，清空 active_check，并以 incomplete_check 保留未完成命令与日志位置，不编造未知退出码。进程组探测或发送信号被权限拒绝时，记录 cleanup_confirmed=false、cleanup_error，保留实际直接子进程退出码及原日志；必要时只通过自己持有的 Popen 结束、回收直接子进程，不据此宣称整组已清理。

清理证明只覆盖 runner 创建的进程组；检查不得自行 daemonize 或另建 session 逃离该组。宿主崩溃、SIGKILL、机器断电时，报告可能停在 running。进程退出会释放锁，先核对遗留进程和构建资源是否已经清理，再启动下一套。清理未知、报告未完成时，恢复须核对精确 PID/组、命令、实际存活和已有副作用，不能直接重跑或将超时当退出。报告初始化和逐步落盘不能承诺硬断电后的完整持久化。

### 原生焦点与分块输入反例

- **先建立焦点正基线。** 在获准的隔离 GUI 会话，用专用合成目标分别记录原窗口确为 `keyWindow`、目标元素取得 AX 焦点，以及 first responder 和插入位置的初始值。再分别检查动作前后是否保持。原生夹具若只证实 AX 焦点或光标保持，把对应断言和缺失的 key window 基线分开记录。动作前后均为 `nil` 时，key window 保持项标为缺少前置条件，保留该次真实退出码。
- **在首片提交与后续轮询之间切焦。** 就绪的分块路径在 `ChunkedInput::submit` 内同步推进首片。调用返回后先核对目标只收到首片，再在首次外部轮询前让合成目标失焦。随后核对剩余分块和重复回调没有新增写入，并保留切焦前后的提交与接收证据。切焦发生在后续轮询之后时，夹具可能已经推进了下一片，须按实际时序定位失败。见 [分块提交实现](../crates/voiceport-macos/src/delivery/model.rs)和[投递验证范围](INPUT_DELIVERY.md#验证范围)。

## 发布包隐私扫描与重新验签

在实际发布授权范围内，对最终拟发布的 ZIP 计算 SHA-256，核对归档成员后在全新独占目录解包。检查解包后的文件清单和实际内容，扫描二进制可见字符串、随包文本和元数据中的本机绝对路径、用户与工作区标识、凭据及私有内容。发布核验绑定实际解包的 ZIP 摘要。扫描范围及命中数量写入报告，原始命中仅保存在私有证据中。包内文件、应用与 CLI 对应关系参照 [打包检查](PACKAGING.md#packaging-and-product-correspondence)。

1. 有隐私命中时停在候选阶段，保留原候选摘要、首次扫描和已有验签回执。能由编译路径映射解决的，从同一冻结源码在独立输出目录重建，记录工具链和映射配置。例如 Rust 源路径可用 `--remap-path-prefix`。检查修复覆盖实际命中，不在已签名包上直接替换内容。
2. 对新候选重新签名、生成最终 ZIP，并用新解包目录重新核对包体、版本、App/CLI 签名、上一发布版的 designated requirement、归档文件范围和隐私扫描，再封存 SHA-256。开发预览按实际自签名身份核对；正式 Developer ID 发行另须对新候选完成适用的公证、staple、票据核验和 Gatekeeper 检查。见 [安装、签名与更新](MACOS_UPDATES.md#正式发布与首次配置)。
3. 获准发布时先暂存 Release，逐项读回远端资产的 SHA-256 并比对验收清单。公开后读回标签和全部资产摘要，确认与实测资产一致。写入回执未知时核对原尝试和远端状态；摘要、签名或扫描结果不匹配时保留证据并暂停发布。一次实际修复及读回的范围见 [v0.3.14 的复盘](LOOP_LESSONS.md#签名通过后仍须扫描最终-zip2026-11-25)。

## 冻结补丁的实际物化

在获准写入的 worktree 中，用 `git rev-parse --show-toplevel` 核对实际仓库根，确认分支、基线提交、tree 和工作区状态。读回补丁 SHA-256，再从该根对精确基线执行 `git apply --check` 和实际应用，保存命令与回执。从 Git 子目录执行时，目录外的补丁路径可能被跳过且仍退出 0。

应用后对照冻结清单逐项核对实际差异、文件字节、摘要和模式，含暂存、未暂存、未跟踪、新增、删除及重命名。空差异、漏项或摘要不一致时，从实际仓库根、工作区归属和初始状态查明原因。已存在部分写入时先对照现场再决定恢复。只在物化结果与冻结输入一致后开始验证。

## 交接与复核

交接保留原失败、全部 attempts、未完成 active_check、进程/工具句柄和实际清理结果，不只贴最后一次绿色。失败、取消、超时、未测各自注明；按既有单次基础设施重试预算判断，不增加重试次数。

`verify.py` 的 schema 2 输入覆盖 README、docs、automation、scripts、Tests 等产品输入及未跟踪文件，实际清单见 `inputs-before.json`；`.claude/`、`.codex/`、根 planning 和 `.planning/` 不在产品清单中。涉及这些轮换/配置输入时，冻结包须另列实际文件和哈希，不能用产品摘要声称全部规则已冻结。实现或输入变化后重新冻结，原验证者按变化复核，不复用旧版本批准。
