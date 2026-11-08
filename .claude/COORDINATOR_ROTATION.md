# VoicePort 全角色会话轮换

协调者、开发者、独立验证者、协议支撑和效果评估者统一最多 **5 轮上下文**：初始上下文加最多 **4 次已完成压缩**。进入第 5 轮就利用剩余空间直接保存交接并换新窗口，不再进行第 5 次压缩。这里明确区分上下文轮数和已完成压缩次数；旧版“完成 5 次压缩后才轮换”已被替代。不额外压测，不强制压缩，不为测试轮换消耗真实上下文。

2026-11-05 用户发现验证窗口实际已有 10 次压缩。原因是旧实现只约束协调者 owner，worker 被明确排除；Orca follow-up 即使换了 terminal handle 仍可恢复旧对话。该旧验证进程已按用户指令经 guarded stop 确认退出并释放，原报告材料和 freeze07 快照保留，未完成评审不能变成产品退回或通过。当前配置修正须以新的完整冻结版本继续验收。

这是项目 hook 与模型执行交接步骤的机制，不是 `/loop` 内置功能或外部进程监督器。需要本机 Python3/fcntl 与 Orca；原 Run、产品 attempt、角色沿革、冻结与暂停跨窗口保留。所有产品账本仍只由唯一主协调者维护；各角色可以并行处理无写入冲突的真实任务。

## 宿主适配与计数

以下计数、身份校验、阻断与交接完整性均为必须满足的策略要求，不是当前实现已获独立接受的保证。首次操作实现独立评审失败，保存了 7 个可复现缺陷：同 inode/大小改写漏计、缺失 transcript 放行、外来 Claude 会话与缺失身份的原生 transcript 放行、通知后笔记可变、笔记符号链接被接受、未核对实际调用者 terminal。当前另有返修进行，完成后的新固定输入独立复核待办；此处不预判返修成败。真实新会话 PreToolUse 加载已有观察，SessionStart、实际 PreCompact veto 和完整自动生命周期仍未执行；原生信任/激活仍阻塞。

- Claude Code 使用 `.claude/settings.json` 的 PreCompact、PostCompact、SessionStart、PreToolUse、Stop。按当前会话 `compact_boundary` 的实际 UUID 去重。
- 原生 Codex 使用 `.codex/hooks.json` 的相同事件。要求按 `session_meta.payload.id` 验证所属会话，按真实 `compacted.payload.window_id` 去重。不能拿 Claude parser 猜 Codex 次数；要求未知 schema/缺失 transcript 明确失败并拦住后续工具或压缩，不能把该要求当成已验证的当前保证。
- 完整 JSONL 行、延迟记录、inode 替换与截断会补查；保留完成身份集合，不因恢复或新终端清零。初始状态不吞掉已存在的压缩。
- 旧协调者 lease/epoch 保留在 `.voiceport/coordinator-rotation.json`；显式注册的新 Run 在 `.voiceport/run-rotations/<run-id>.json`，通过实际 session 记录的 `coordinator_run` 路由；不迁移或改写无关历史 lease。所有非旧全局 owner 会话独立计数在 `.voiceport/session-rotations/<actual-session>.json`，没有全局 lease 也经过真实 hook 分派并计数。worker 的计数不能写协调者 lease 或产品账本。运行阈值字段 `threshold=5` 表示上下文轮数；配置不可超过 5，继任沿用。
- 最后一轮拦住新开发、测试、派发、收件箱消费与 loop 续订，只允许必要只读和明确交接操作。当前工具自然返回；不以静默或超时推断它退出。

两个宿主的压缩 veto 输出不同：Claude PreCompact 返回 `decision: block` / `reason`（或 exit2）；原生 Codex PreCompact 返回 `continue: false` / `stopReason`。Claude 的 PreCompact `continue:false` 不能当 veto；Codex 不接受此事件的 `decision:block`。Codex PreToolUse 使用 `decision:block`，不是 `continue:false`。共享 helper 显式选择 `hook --harness codex` 适配输出。

## 加载证明与信任条件

配置存在不代表当前进程已加载。每次新 worker 应核对真实 PreToolUse 写出的 session 状态：实际 session ID、terminal 与 Orca 回执匹配，首次计数 0，`hook_implementation_sha256` 对应当前 helper；再从真实 transcript 的初始身份/时间和当前 compact_boundary 核对确实为新会话及计数一致。SessionStart 若未观察到必须保留该事实，不能伪造事件填证明；PreToolUse 须补扫并执行上限；真实加载观察不等于身份/计数实现已独立接受，但不能因缺少辅助 SessionStart 标记而无理由卡住已按上述条件证明的新上下文。不得手造状态或把合成 stdin 测试说成实际 hook 激活。报告区分已观测的加载与未执行的 PreCompact 全生命周期；原会话已超过上限不能恢复工作。

原生 Codex 0.166.0 的未托管 hook 需要用户通过 `/hooks` 信任具体定义哈希；修改定义后可能需要重新信任。不自动改信任数据库或全局配置。缺少真实加载证明时该前端工作应报告阻塞，不能先做产品改动再补证明。Claude 新窗口加载项目 settings；已在运行的窗口也须凭实际 hook 结果确认，不能只依赖文件落盘。

宿主对手动/自动压缩的实际跨窗全流程不通过强制压缩来验收。隔离单元测试验证逻辑、真实新窗口验证身份和加载；未执行的完整生命周期路径继续如实标注。

## 协调者交接

首次 adopt 要先确认原协调者实际停 loop/退任，绑定既有 Run，不新建 Run。原有授权足够，不反复向用户确认。普通 Run 不加仅适用于 legacy 的 takeover 参数。

```sh
orca orchestration run-use --id ACTUAL_RUN --json
orca orchestration run-current --full --json
python3 -I .claude/hooks/coordinator_rotation.py adopt --session ACTUAL_SESSION_ID --run ACTUAL_RUN --transcript /absolute/session.jsonl --threshold 5 --goal '当前目标' --loop-prompt '原始已授权 loop 提示词' --retired-session OLD_SESSION_ID
```

活动 owner 可用 `configure --session ACTUAL_SESSION_ID --threshold 5 --goal '当前目标'` 同步运行阈值和持久目标；交接期间不能提高阈值逃避轮换。运行状态不要直接手改。

进入最后一轮后：

1. 成功调用 `ScheduleWakeup({stop:true})`，保留真实回执，不安装新 wakeup。
2. 运行 `python3 -I .claude/hooks/coordinator_rotation.py retire --session ACTUAL_SESSION_ID --transcript /absolute/session.jsonl`。核对最后一次调度确实停止，保存最多 48KB 的交接包并退任。
3. 运行 `python3 -I .claude/hooks/coordinator_rotation.py launch --session ACTUAL_SESSION_ID`，取得 Orca prompt 已送达回执后结束 turn。新 session 读取 packet、从自己的终端绑定原 Run，然后用一次性 token 执行 `claim --session ACTUAL_NEW_SESSION_ID --token TOKEN`。
4. claim 验证 Run、packet 哈希、旧 session fencing、token/epoch。继任核对自身确为新 session 及自身实际计数，不以新 terminal handle 代替。保留活动 worker、角色、attempt 和冻结；某 worker 需要轮换时按下面的独立流程处理。
5. 接管成功、未暂停且仍可推进时，显式用 packet 的原始 `loop_prompt` 启动 `/loop`。原 timer 不自动迁移。Orca 完成/问题/升级事件是主唤醒，1200 秒以上 timer 只兜底。

Stop 提示只在非递归停止时给出，不无限阻止结束。旧 owner 被 fence，即便重新打开也不能消费邮箱或恢复调度。

### 按 Run 注册与自动正常轮换（待独立复核）

用户已授权未来正常轮换自动创建真实新会话、发送有界交接并关闭精确退任窗口，不要求用户手工开窗口或粘贴提示词。实际权限/信任拒绝仍为阻塞，禁止换工具、会话或改信任库绕过。此处是新实现接口及临时 fixture 自检契约，未执行真实压缩、启动、接管或关闭全流程，不代表已独立接受。

全局 lease 属于其他 Run 时，当前协调者先从**自己的实际终端**确认已绑定原 Run，再注册；worker 不得 run-use。注册读取现有真实 PreToolUse 状态、当前 helper 哈希、terminal 与 transcript 身份/单调计数，查询 `run-current`、`run-show` 的当前 `coordinator_handle`/generation 和 `terminal show` 的工作区/incarnation。只给 Run ID 不授予协调者身份；缺证明则失败，不手造 SessionStart 或 accounting 文件。注册保留已有计数，包括已经进入最后一轮的 requested 状态；该轮只开放精确 register 命令以恢复行政交接。

```sh
python3 -I .claude/hooks/coordinator_rotation.py register --session ACTUAL_SESSION --run ACTUAL_RUN --transcript /absolute/session.jsonl --goal '当前目标' --loop-prompt-file .claude/loop.md --navigation .voiceport/handoffs/CURRENT-RUN.json
python3 -I .claude/hooks/coordinator_rotation.py status --run ACTUAL_RUN
```

`--loop-prompt-file .claude/loop.md` 保存项目已授权 loop 提示词的完整文本（最多 48KB），使最后一轮无需 shell 多行代码或用户粘贴；已有其他单行原始提示词也可显式用 `--loop-prompt '原文'`。navigation 必须是 `.voiceport/handoffs/` 内无符号链接的普通 JSON，最多 48KB，包含准确 `run_id`、`product_task_id` 和 `product_attempt_id`（未绑定填 `unbound`），由协调者继续维护。退任时保存该 Run 当前 navigation 的完整有界快照及哈希、全局暂停和准确产品关联；不会把其他 Run 的私有注册表或其 attempt 当成本 Run。已有全局 lease 仍使用前节不带 `--run` 的原接口，禁止借注册重置同 Run 的旧 lease；新 per-Run 证明和精确关闭保护适用于带 `--run` 的路径。

hook 使用已验证的注册缓存，不逐次查询 Orca、不执行 run-use；仍逐次检查实际 terminal 并锁存 drift、重新核对 transcript。行政命令重新查询真实 Run owner、consumer generation 与 terminal incarnation；外部接管不能凭旧缓存完成退任或启动。

进入最后一轮只允许 Read、停止 loop、精确 retire/launch/status 和有界只读 Orca 身份查询，禁止产品修改、测试、派发、邮箱消费、续订 loop 和继续压缩。正常顺序如下，每条命令单独调用：

1. 取得真实 `ScheduleWakeup({stop:true})` 成功回执；此处沿用 Claude transcript 的 ScheduleWakeup schema，不宣称已实现原生 Codex 协调者 loop-stop 适配。
2. `python3 -I .claude/hooks/coordinator_rotation.py retire --run ACTUAL_RUN --session OLD_SESSION --transcript /absolute/actual-old-session.jsonl`。必须是实际 hook 绑定 transcript；保存有界 packet/哈希并 fence 旧 session。
3. `python3 -I .claude/hooks/coordinator_rotation.py launch --run ACTUAL_RUN --session OLD_SESSION`。调用 Orca 前持久记录 token、完整 argv 和 attempt 时间；只有一次 fresh launch。收到已 journaled/handed-to-terminal 的投递回执后结束旧 turn。该实现沿用既有 Claude successor 启动入口，不改模型/用户偏好。
4. 继任先取得真实当前 PreToolUse 状态，再自行 `orca orchestration run-use --id ACTUAL_RUN --json`（普通 Run 不加 legacy 参数），然后 `python3 -I .claude/hooks/coordinator_rotation.py claim --run ACTUAL_RUN --session NEW_SESSION --token TRANSFER_TOKEN`。必须是不同 session/terminal、当前 helper、零次完成压缩，hook 创建时间和 transcript 最早已观察身份时间都晚于 launch；Run coordinator_handle 必须准确等于实际 caller，generation 必须递增。缺时间/外来 transcript/旧上下文恢复均拒绝；不把新 accounting 文件当成新上下文。
5. claim 后先读 `status --run ACTUAL_RUN` 核对投递与继任证明，然后 `python3 -I .claude/hooks/coordinator_rotation.py close-retired --run ACTUAL_RUN --session NEW_SESSION --terminal EXACT_OLD_TERMINAL`。helper 核对已保存的旧 handle/incarnation/工作区，先持久记录 attempt，再只关闭该 terminal，不关闭整个 tab/工作区。只有精确 handle 的 `result.close.ptyKilled=true` 回执才解除继任产品/loop fence；成功 RPC 外壳本身不足。
6. 确认关闭、原暂停条件和工作边界后，才按 packet 原始 loop_prompt 恢复原授权 loop；不清理旧证据、不改 Run、产品 attempt 或 worker lineage。

未知 launch/send/close 一律锁存，保留收到的原始 stdout/stderr/退出码或 timeout 信息。可用 `status --run ACTUAL_RUN` 检查保存状态，以及精确 `orca orchestration run-show --id ACTUAL_RUN --full --json`、`orca orchestration run-current --full --json`、`orca terminal show --terminal ACTUAL_HANDLE --json`，或 `orca terminal list --worktree path:/absolute/worktree --limit 20 --json` 作有界检查。不得直接再次 fresh launch/send/close；没有可靠证明时升级阻塞，不手改 accounting 解锁。当前没有自动 reconcile/重发接口；未知结果须由协调者核实并安排受审恢复，不能以超时或静默判断退出。

## Worker 交接

worker 不启动继任、不停协调者 loop、不改 ledger/角色注册或结算自己的未完成产品任务。

1. 在 `.voiceport/handoffs/session-rotation-ACTUAL_SESSION_ID.md` 写入精简交接：实际 Run/Task/Dispatch、role predecessor、attempt/freeze pins，已完成命令/原始日志/退出码，正在运行工具的真实句柄与状态，暂定发现、未做项、证据范围和权限。不能把未完成操作写成成功。
2. 执行 `python3 -I .claude/hooks/coordinator_rotation.py checkpoint --session ACTUAL_SESSION_ID`。原生 Codex 可通过同一命令的 `--notes-text '单行精简交接内容'` 安全保存，避免被工具 fence 后再编辑任意文件。策略要求 helper 核对实际调用者的当前 terminal/session 与精确注册身份，只接受限定私有路径内的普通笔记文件（非符号链接），记录 SHA/身份并在通知后保持/复核笔记完整性，发送一次 escalation 并保留未知送达锁存；这些身份与完整性要求尚待新固定输入独立复核，不伪造 worker_done 或产品验收。
3. 结束 turn。协调者核对交接及活动工具，在用户已授权轮换范围内 guarded stop 精确 dispatch；只有确认退出/释放后才能开始新上下文。不把取消请求或未知存活当成已退出。
4. 若原 Task/spec/freeze 仍适用，在同一 Task 上按 Orca recovery 的 retry/new-start 继续。若 spec/freeze 已改变，建立显式 successor assignment，记录原 Task/Dispatch/role lineage；产品 attempt 不因行政轮换重开，也不凭行政失败拒绝产品。
5. 新窗口用 `worker-start` 和实际 fresh-agent 回执。对已到上限的旧 dispatch 直接 `send` 可能恢复原 transcript，禁止当作换新上下文。实际 session ID 必须变化、首次计数为 0，旧证据保持只读，新证据使用新路径。

### 实际 terminal 与保存绑定不一致：先 fence，再由协调者新开会话

同一 session 从 terminal A 恢复到实际 terminal B（包括已经结算的角色被重新打开）不能继续普通工作，即使完成压缩次数为 0、注册表已改到 B 或旧 Task 尚未完成。worker hook 在普通工具与 PreCompact 前核对实际 `ORCA_TERMINAL_HANDLE`；发现缺失或不一致会锁存 `terminal_drift` 的首个实际/保存 terminal、事件与时间。原 `owner`、保存 terminal、笔记及完成压缩记录保留；后续有效 transcript 扫描只增加已完成记录。返回 terminal A 也不解除 fence，checkpoint 仍核对实际 caller 相等并拒绝已锁存 drift；不发送旧 terminal 的成功交接。

1. worker 立即停止普通工作与压缩，披露实际和保存的 terminal、session 与计数，按当前真实 Orca preamble 的 from/Task/Dispatch 发 escalation，然后结束 turn。不要手改绑定/清零、冒用 A、补造 SessionStart 或把旧笔记当成功 checkpoint。drift 路径不拦 Stop，包含递归 Stop；双宿主保持各自的 PreCompact/PreToolUse veto 输出。
2. fence 只允许 Read、精确 `python3 -I .claude/hooks/coordinator_rotation.py session-status --session ACTUAL_SESSION_ID` 和一条有界的 `orca orchestration send` escalation。该 shell 命令仅接受 `--from ACTUAL_TERMINAL --type escalation --subject '简短主题' --body '简短事实与停止请求' --task-id ACTUAL_TASK --dispatch-id ACTUAL_DISPATCH` 六组不重复参数，Task/Dispatch 必须来自实际 preamble 并由 Orca 验证；不加其他参数。命令最多 48KB，避免 shell 元字符（正文也不要换行、分号或命令替换）；不允许任意 Bash/exec_command、Skill、Write/Edit，连原 checkpoint 笔记也不开放。该例是接口格式，不是实际回执。
3. 协调者从真实状态、transcript 与已保留工作采集行政 handoff，记下未成功的旧 terminal checkpoint、原角色/Task/Dispatch history、实际绑定差异、计数、证据和仍运行工具。行政 handoff 由协调者保存，worker 不通过 fence 外写文件；此采集不替代旧 worker 的 checkpoint 成功。
4. 协调者依 Orca recovery 对精确 mismatched assignment guarded stop，取得实际 exited/资源释放后再开 genuinely fresh session。spec/freeze 不变走同一 Task 的 retry/new-start；改变则建显式 successor，保留原角色/Task/Dispatch 沿革、Run 和产品 attempt。只发 send 恢复旧对话、只改 registry、收到 stop 请求、静默或超时均不算退出或新会话。
5. 新 session ID 必须不同，实际 terminal 与新 hook 状态完全相等，真实 PreToolUse 加载当前指定 helper hash，bounded transcript 初始身份/时间与首次完成压缩数 0 一致。SessionStart 未观察到用上述 startup addendum 如实补证，不伪造。旧 session 永久保留 fence 与旧证据；正常 fresh session 仍按初始加最多四次压缩、最后一轮直接 checkpoint/handoff 执行。

worker drift 路径没有 terminal/token migration 或自行 stop/launch/信任绕过；正常协调者的显式 per-Run 自动轮换接口见上节。临时根目录/mock Orca 的 gate 与恢复模型是开发控制；实际 stop/release/launch、PreCompact veto 及完整生命周期未因这些控制获得接受。独立 operational03 的 FAILED 和 policy03 对旧九文档精确快照的结论均保留；本次改动须重新固定相关 operational/docs 输入并独立复核。原生 Codex 信任 Task 仍失败/未执行，Full VP-013/capture 仍 false。

## 失败恢复和 Git

`requested` 只做交接；`retired` 可单次 launch；`launching` 或 checkpoint `notifying` 表示已尝试，超时也不能重复 fresh 启动/发送。按实际 Orca receipt 的 replay ID 恢复；没有证明则报告未知状态，不能凭静默另开重复写者。claim/身份/哈希失败不写产品、不起 loop。worker `handoff_ready` 只证明交接准备与升级送达，不证明旧进程已经退出。

```sh
python3 -I .claude/hooks/coordinator_rotation.py status
python3 -I .claude/hooks/coordinator_rotation.py session-status --session ACTUAL_WORKER_SESSION
python3 -I .claude/hooks/test_coordinator_rotation.py
```

每个有实际产出的阶段仍须分逻辑 commit/push、更新 PR，满足正常仓库条件后自主合并，在无写者/冻结冲突的安全点同步 main；用户已持续授权，不逐 PR 重问。不能 force/admin 绕过。交接携带未完 Git 步骤，Git 集成不代表产品验收。配置/角色文档属于冻结输入，调整时先明确停止旧评审并建立新冻结，不能静默漂移。

## 来源与验证边界

2026-11-05 核对 Claude Code 2.1.318、原生 Codex 0.166.0 的已安装实现/schema，并只读查阅 [Claude PreCompact](https://code.claude.com/docs/en/hooks#precompact) 与 [Codex hooks](https://learn.chatgpt.com/docs/hooks)。本机原生 Codex rollout 确认 `session_meta` 和带 window_id 的 `compacted` 结构。上述是来源和 schema 证据，不能代替真实 compaction veto/跨窗口全流程执行。

原 16 项协调者测试保留沿革；本次修正保留既有全部 55 项，新增 20 项，合计 75 项开发自检通过。隔离测试覆盖无全局 lease 的真实 hook 分派、per-Run 注册与冒用拒绝、最后一轮行政 gate、未知启动/关闭去重、initial0/transcript 起始时间/Run generation、精确关闭与下一轮回执保存，以及全部角色、四次后交接、双宿主 veto、超过上限不清零、checkpoint 单次通知、错误身份/schema 和旧 lease/token/epoch/暂停规则。只在临时项目/mock Orca 中执行，不运行真实压缩/启动压测。原 [planning-with-files](https://github.com/OthmanAdi/planning-with-files/blob/8eef6dc6540dde5eef3fdc0ac487ad9c48ef3ba5/hooks/claude-hook.sh#L304-L345)、[HumanLayer handoff](https://github.com/humanlayer/humanlayer/blob/f9bd73534a9fb7099a274f951b698b1297122fd9/apps/riptide-api/src/orpc/api/sessions/handoff.ts#L40-L114)、[Ralph](https://github.com/snarktank/ralph/blob/6c53cb0b831ebe8739c6a003e22af14902d8b0b5/ralph.sh#L84-L108) 的持久状态/事务/新进程思路仍适用，不继承权限绕过或文本完成哨兵。
