# VoicePort 全角色会话轮换

所有角色最多 **5 轮上下文 = 初始轮 + 最多 4 次已完成压缩**。进入第 5 轮即 `requested`，利用剩余空间保存交接，当前工具自然返回后停止新工作，禁止第 5 次压缩。不强制 compact，不额外压测，不为验证规则消耗真实上下文。

这是项目 hook 加模型交接程序，不是 `/loop` 内置轮换，也不是崩溃监督器；模型未执行交接、宿主退出或权限拒绝时，不承诺自动完成。需要本机 Python3/fcntl 与 Orca。本文区分三种身份，不因缺少本地注册就授予顶层权限：

| 实际身份 | 注册与轮换路径 | 边界 |
|---|---|---|
| 用户直接打开、未绑定 Run 且不属于任何 worker 的顶层会话 | `session-register → session-retire → session-launch → session-claim → session-close` | 继承冻结的原目标/完整提示词/授权；不新建 Run、不接管产品协调或无关 Git 权限 |
| 已绑定原 Run 的真实协调者 | 新 lease 用 `register`，旧全局 lease 先 `upgrade-legacy`；随后 `retire → launch → claim → close-retired` | 保留 Run、产品 attempt、角色、冻结、暂停与未完 Git/CI 步骤 |
| 托管 worker | `checkpoint` 后交协调者停止旧 dispatch、启动 fresh worker | 不自行 launch、不安装 loop、不接管协调者或结算未完成产品任务 |

## 宿主、计数与加载证明

- Claude Code 按当前会话 `compact_boundary` 的实际 UUID 去重。Claude PreCompact 的 veto 是 `decision:block` / `reason`（或 exit2）；此接口正确，无需改成 `continue:false`。
- 原生 Codex 0.167 的压缩适配支持 `session_meta.payload.id` 身份核验和 `compacted.payload.window_id` 去重。Codex PreCompact 使用 `continue:false` / `stopReason`，PreToolUse 使用 `decision:block`。helper 用 `hook --harness codex` 选择适配，不拿 Claude parser 猜次数。
- 每次 hook 重扫完整 JSONL 快照并复核前缀；延迟完整行、同大小改写、inode 替换和截断不清零已完成身份集合。缺失 transcript、身份错误或未知 schema 失败关闭，不能用重开 terminal、恢复旧 transcript 或手改状态逃避上限。
- 普通会话状态在 `.voiceport/session-rotations/<actual-session>.json`；显式 Run lease 在 `.voiceport/run-rotations/<run-id>.json`，通过 session 的 `coordinator_run` 路由；旧全局 lease 在 `.voiceport/coordinator-rotation.json`，由当前同 Run owner 按下节显式 `upgrade-legacy`，不隐式迁移或改写无关历史。`threshold=5` 表示上下文轮数，继任继承且不可提高到超过 5。

新会话先核对真实 PreToolUse 状态的实际 session、terminal、harness、`hook_implementation_sha256`，并从真实 transcript 起始身份/UTC 时间和完成压缩记录确认首次计数 0。新 terminal 不等于新上下文；继任还须证明状态创建与 transcript 起始时间均晚于 launch。SessionStart 未观察到要如实披露；真实 PreToolUse 加新 transcript 可补足启动证明，不伪造事件、accounting 或测试 stdin 激活。

原生 Codex 的未托管 hook 仍须用户在原生 UI 信任具体定义，修改定义后可能需重新信任。不写信任库、不代批、不改全局偏好；当前进程缺加载证明时该前端阻塞。历史失败的原生信任 Task 不因 Claude 支撑成功而改变。**压缩适配不代表调度适配：目前没有 VERIFIED 的原生 Codex scheduler 接口，原生 transcript 的退任调度适配仍阻塞。** 不猜原生工具名，不把任意 function_call_output 文本当停止证明。

宿主可能在 transcript 创建前触发 SessionStart。新 session 此时只收到延后核验提示，不创建计数/加载证明；第一次真实 PreToolUse 必须成功核对实际 transcript 才可继续，不能把这次延后当观察到有效 SessionStart。已有状态不会用此分支覆盖或解锁。现有 PreToolUse 的 transcript 重扫会读取工具结果；本轮不需要新增 hook 配置或 PostToolUse 事件。配置存在本身不证明当前进程已加载。

2026-11-10 对已安装 Codex 0.167 与 Orca 的只读核查没有发现可验证的原生 loop/cron/停止接口；当前已查 Orca/Codex 自动化列表为空。Orca `automations --fresh-session` 虽可为每次派发新开上下文，但不等待前次 agent 完成，不保证不重叠；`--disabled` 不取消或等待已启动任务，reuse 也不能绑定任意继任 terminal。因此不能用它替换本流程的停止/认领证明。当前持续调度使用已验证的 Claude 宿主，原生 Codex worker 由其原协调者管理轮换；原生独立 loop 路径保留明确不支持，未安装新外部 scheduler，不以空列表或普通 shell 成功伪造已停止。

## 全部自有调度的停止证明

先按真实建立回执登记 scheduler kind、owner session/terminal、job ID、原始完整 prompt、周期、next/expiry、暂停和停止结果；只沿用实际授权且仍适用的一份兜底。固定 CronCreate 与动态 ScheduleWakeup 各自停止，不能互作证明：

1. 动态调度只接受同一 session 中配对的 `ScheduleWakeup({stop:true})` 工具调用和结构化 `toolUseResult.stopped:true`。必须是最后一次动态调度调用的结果；提交调用、无错误文字或外层成功不够，stop 后再续订使旧证明失效。
2. Cron ownership 来自同一 session 真实成功的 CronCreate 调用/结果，保留返回 ID、完整 prompt、cron 与 tool-use ID。CronList 看见某 job、私有手写声明或其他会话的结果都不能建立所有权。先 CronList 核对，再逐个 `CronDelete({id:OWNED_ID})`，每个删除都须有配对成功结果且返回同一 ID；最后在全部 cron mutation 之后 `CronList({})`，确认所有自有 ID 均已不存在。其他人的 job 可以保留，不能删除。
3. 任何自有 job 未删、创建/删除结果未决、删除失败、最终列表缺失或仍有自有 ID，均阻止退任。动态与 cron 混用时两组条件都须满足；只有已证明并全部删除的 cron 时无需额外动态 stop。没有可用 cron 停止链时，helper 仍要求真实动态 stop 证明，不能凭“没有 timer”跳过。

最终轮 gate 已允许精确 `ScheduleWakeup({stop:true})`、`CronList({})`，以及仅含 `id`、由本 session 成功 CronCreate 证明的 `CronDelete({id:OWNED_ID})`。不允许创建/续订或删未知 ID；不再要求提前第 4 轮删除 cron。来源缺失或无法证明归属时失败关闭，保留阻塞，不能删他人任务或改 accounting 解锁。terminal drift 的更窄恢复 gate 另见下节，不由这份白名单解除。

transcript scan 持久保存动态调用 ID、停止/未决结果，以及已确认 CronCreate 所有权、删除证明与未决 mutation；截断/压缩不能抹掉任何已知调度义务。动态 stop 必须通过同一已验证快照的先后顺序或真实 UTC 时间证明晚于所有已知动态调用；恢复旧前缀中的早期 stop 不能覆盖较晚调度，顺序无法证明时继续阻塞。持久来源仍须真实同 session 调用/结果，不接受手造记录；停止时合并该来源并复核当前结构化回执与最终列表，写入交接 packet。旧 job 不自动迁移。继任完成对应 claim/精确关闭、核对继承与当前暂停后，只有原授权允许且仍有工作时，才显式用原完整 prompt 恢复一份 loop，保存新回执和 predecessor ID。

## 直接打开的顶层会话

### 安装 loop 前登记原契约

先有真实 PreToolUse/session 证明，再写精确私有普通 JSON 文件 `.voiceport/handoffs/session-contract-ACTUAL_SESSION_ID.json`（无符号链接、最多 48KB）。**仅含以下五个字段**，全部值须来自实际用户任务；这里是结构占位，不是授权或回执：

```json
{
  "goal": "本次实际目标",
  "loop_prompt": "本次已授权 loop 提示词的完整原文，不以路径或摘要代替",
  "authorization": "实际用户授权来源、允许动作与边界",
  "paused": false,
  "active_tools": []
}
```

`paused` 必须是真实布尔值，`active_tools` 必须是实际列表；前三项为非空字符串。单独执行：

```sh
python3 -I .claude/hooks/coordinator_rotation.py session-register --session ACTUAL_SESSION_ID --contract .voiceport/handoffs/session-contract-ACTUAL_SESSION_ID.json
```

注册核对 authentic PreToolUse、当前 helper、实际 terminal/transcript，要求 `run-current` 明确无 Run；分页穷尽 all-Run `worker-list --full`（含 context-only 与 settled-but-retained worker），并核对本地 `.voiceport/coordination.json`。实际 full 信封以 worker 行的 `agentTerminalHandle` / `resource.terminalHandle` 核对 terminal，以 `result.page.hasMore` / `result.page.nextCursor` 核对分页；不把 concise 视图的 `terminal` / 顶层 `nextCursor` 当成 full schema。实际 terminal 匹配任何 worker 或 registry 即拒绝；分页缺失、截断、循环、超出有界检查或未知 schema 均失败关闭。再核对 terminal 工作区/incarnation/agent。缺少本地 worker 导航并不等于顶层身份，不能创建 Run、删 registry 或冒充协调者绕过。

契约是原始登记快照，按原字节摘要冻结并由继任继承；不通过重写注册清空计数、暂停或扩权。登记前须结算已有工具，真实 `active_tools` 为 `[]` 后才登记；非空冻结值会阻止退任，不能手改契约解锁。**初始空列表不证明后续没有活动工具：退任前须重新核对并结算当前所有工具，将实际句柄、结束结果、最新用户约束和剩余工作写入有界 notes。** 未知/仍运行不能记已结束；最新约束收紧原边界，不能借 notes 自行扩权。全局暂停另外持续核对。

### 主动提前交接

已登记的独立顶层会话可在真实上限前主动交接：

```sh
python3 -I .claude/hooks/coordinator_rotation.py session-request --session ACTUAL_SESSION_ID --reason '实际提前交接原因'
```

它核对当前 authentic proof、非 Run/worker 身份以及前代已确认关闭，记录 `trigger=voluntary`、原因和实际压缩数，再进入同一个 `requested` 流程。阈值、已完成压缩集合和已有自然上限请求均不重写。用于正常提前移交或有界真实生命周期演练；演练只证明实际执行的开窗、认领、精确关闭与调度步骤，不冒充自然四次压缩验收。未登记会话、托管 worker 或前代尚未关闭时拒绝。

### requested 后的正常顺序

既有顶层会话已到最后一轮却未注册时，gate 允许写自己的精确 contract 文件并执行上述 `session-register`；保留现有计数与 `requested`，不能借注册重新开始产品工作。托管 worker 仍走协调者路径。

1. 当前工具自然返回后，在 `.voiceport/handoffs/session-rotation-OLD_SESSION.md` 保存非空、最多 48KB 的精简笔记：实际 session/terminal、目标、已做/未做、命令/退出码/证据、真实活动工具及状态、授权与暂停。只写自己的精确交接路径，不开新产品操作。
2. 按上节停止全部本人调度，取得各自真实结构化结果及 cron 最终不存在证明。
3. 单独运行 `session-retire`。helper 复核身份、契约、笔记和 stop 证明，冻结 packet/笔记摘要并 fence 旧 session。
4. 单独运行 `session-launch` **一次**。helper 先持久保存 token、argv、attempt 时间，再调用实际 harness 的启动入口。本流程按用户要求接续 CLI 窗口，单次启动显式使用 `--surface terminal`，不修改全局 Chat/CLI 偏好；只接受 terminal outcome 及其投递回执，结构化 journaled 不满足该生命周期。取得回执后结束旧 turn。失败/超时/未知都不能再 fresh launch。
5. 新实际 session 获得真实 fresh initial0 PreToolUse 证明，读取 packet/notes，再执行带 OLD 来源和 token 的 `session-claim`；须不同 session/terminal、晚于 launch 的 transcript 与状态创建时间，并重新证明不属于 Run/worker。继承冻结契约、阈值和暂停。
6. 新会话用 `session-close` 关闭精确旧 terminal。helper 核对 launch 的实际目标等于继任 terminal、旧 handle/incarnation/工作区，持久保存关闭 attempt；只有返回精确旧 handle 且 `result.close.ptyKilled=true` 才解除产品/loop fence。若 launch 回执尚未保存，先等其实际回执，不抢先关闭。

每条命令单独调用；OLD/NEW/TOKEN/EXACT_OLD_TERMINAL 都取真实状态，不自造：

```sh
python3 -I .claude/hooks/coordinator_rotation.py session-retire --session OLD_SESSION
python3 -I .claude/hooks/coordinator_rotation.py session-launch --session OLD_SESSION
python3 -I .claude/hooks/coordinator_rotation.py session-claim --session NEW_SESSION --source OLD_SESSION --token TOKEN
python3 -I .claude/hooks/coordinator_rotation.py session-close --session NEW_SESSION --source OLD_SESSION --terminal EXACT_OLD_TERMINAL
```

关闭成功前不做产品工作、不安装 loop；之后再次核对 packet 与当前暂停、原授权及未完事项，只恢复一份完整 `contract.loop_prompt`。顶层轮换不授予新 Run、产品账本、派发或无关 Git 权限。检查用精确 `session-status --session ACTUAL_SESSION`；未知副作用保留原始回执、有界核对并报告阻塞，不手改状态或重复启动/关闭。

## 已绑定 Run 的协调者

用户已授权未来正常自动开窗、交接和关闭精确退任 terminal，无需手动开窗或粘贴提示词；真实权限/信任拒绝仍阻塞。尚无同 Run lease 的协调者在自己的实际终端确认原 Run 后注册；已有旧全局 lease 则先走下节 `upgrade-legacy`，不能用 register 重置。不新建 Run、不让 worker run-use：

```sh
python3 -I .claude/hooks/coordinator_rotation.py register --session ACTUAL_SESSION --run ACTUAL_RUN --transcript /absolute/session.jsonl --goal '当前目标' --loop-prompt-file .claude/loop.md --navigation .voiceport/handoffs/CURRENT-RUN.json
python3 -I .claude/hooks/coordinator_rotation.py status --run ACTUAL_RUN
```

注册核对真实 PreToolUse/helper/terminal/transcript、`run-current`/`run-show` 当前 coordinator_handle/generation 与 terminal incarnation，保留已有计数；最后一轮允许精确 register 恢复行政交接。navigation 为私有无符号链接普通 JSON、最多 48KB，含准确 `run_id`、`product_task_id`、`product_attempt_id`（未绑定填 `unbound`）。完整提示词使用 `--loop-prompt-file .claude/loop.md` 或实际单行 `--loop-prompt '原文'`。不把其他 Run 的角色/attempt 当成本 Run。

进入最后一轮后只保留读取、精确行政和上节调度停止；禁止产品修改、测试、派发、邮箱消费、loop 续订和继续压缩。每条命令单独执行：

1. 保存交接并停止全部本人调度，取得上节对应证明。
2. `python3 -I .claude/hooks/coordinator_rotation.py retire --run ACTUAL_RUN --session OLD_SESSION --transcript /absolute/actual-old-session.jsonl`。须为 hook 绑定的实际 transcript，保存有界 packet/哈希并 fence 旧 session。
3. `python3 -I .claude/hooks/coordinator_rotation.py launch --run ACTUAL_RUN --session OLD_SESSION`。先持久记录后单次 fresh launch，确认送达后结束旧 turn；此 Run 路径沿用既有 Claude CLI 启动入口，单次显式 `--surface terminal` 接续要求的 CLI 窗口，不改模型或全局 Chat/CLI 偏好；结构化 outcome 保留为不满足条件的回执，不能据此接管。
4. 继任取得真实 fresh initial0 PreToolUse 证明，自行 `orca orchestration run-use --id ACTUAL_RUN --json`，再 `python3 -I .claude/hooks/coordinator_rotation.py claim --run ACTUAL_RUN --session NEW_SESSION --token TOKEN`。须不同 session/terminal、晚于 launch 的起始时间、Run coordinator_handle 等于 caller 且 generation 递增；普通 Run 不加 legacy takeover 参数。
5. 读 `status --run ACTUAL_RUN` 核对送达与继任证明，再 `python3 -I .claude/hooks/coordinator_rotation.py close-retired --run ACTUAL_RUN --session NEW_SESSION --terminal EXACT_OLD_TERMINAL`。只关闭已保存 incarnation/工作区的精确旧 handle，须 `result.close.ptyKilled=true`；外层 RPC 成功不足。
6. 确认关闭与暂停/工作边界后，显式按 packet 完整 `loop_prompt` 恢复原授权 loop；保留 Run、worker、attempt、冻结、证据和未完 CI/Git，不自动迁移 timer。

hook 使用注册缓存但每次核对实际 terminal、锁存 drift 并重扫 transcript；行政命令重查真实 Run owner/generation/incarnation。外部接管不能凭旧缓存完成退任/启动。

### 旧全局 lease：先受保护升级，再退任

旧 `.voiceport/coordinator-rotation.json` 仅由其当前实际 owner 在同一原 Run、`active` 或 `requested` 且尚未尝试 transfer 时升级；不要用 register/adopt 覆盖旧 lease。legacy owner hook 也生成 authentic per-session 证明，最终轮 gate 允许以下精确命令：

```sh
python3 -I .claude/hooks/coordinator_rotation.py upgrade-legacy --session ACTUAL_SESSION --run ACTUAL_RUN
```

helper 核对当前真实 PreToolUse/helper/transcript/terminal、同 Run coordinator 身份、generation 与 incarnation；从既有同 Run registry 和当前产品 attempt 生成有界 `.voiceport/handoffs/legacy-navigation-ACTUAL_SESSION.json`，建立 `.voiceport/run-rotations/<run-id>.json`。保留 epoch、request/history、目标、完整 loop_prompt、worker/冻结及产品沿革，合并已完成压缩记录而不降低计数；已有 requested 不借升级恢复普通工作。

升级给旧 lease 标记 `migrated_run`，把原 session 路由到 per-Run。随后必须使用带 `--run ACTUAL_RUN` 的状态/变更接口，旧全局变更命令会拒绝。**正常后续轮换须在 retire 前完成升级**，再按上节停止全部本人调度、per-Run retire/单次 launch/claim/close-retired；确认精确旧 PTY 退出后才恢复产品/loop。

已尝试 transfer、已有 per-Run lease、导航已存在或产品/registry 沿革不明时失败关闭；保留现场并受审恢复，不能删状态重试、创建新 Run 或跳过精确关闭。此升级接口的隔离测试不代表真实迁移、继任启动或完整压缩生命周期已经执行；真实结果由实际执行者另附。

## Worker 交接与 terminal drift

worker 最后一轮保存 `.voiceport/handoffs/session-rotation-ACTUAL_SESSION_ID.md`：实际 Run/Task/Dispatch、role/predecessor、attempt/freeze pins、命令/原始日志/退出码、真实工具句柄/状态、暂定发现、未做项和权限范围。然后：

```sh
python3 -I .claude/hooks/coordinator_rotation.py checkpoint --session ACTUAL_SESSION_ID
```

原生 Codex 尚无笔记时可用同命令的 `--notes-text '单行精简笔记'` 一次创建提交；已有文件不传此参数，不覆盖旧证据。helper 核对实际 caller terminal/session 与精确注册身份，要求私有普通笔记文件、无符号链接、最多 48KB，记录并复核哈希，单次 escalation；未知送达锁存。checkpoint 不是 worker_done 或产品验收，worker 随即结束 turn，不启动其他 agent/worker/loop。

协调者核对交接和活动工具，依 Orca recovery guarded stop 精确旧 dispatch；须真实 exited/资源释放后才用 `worker-start` 与 fresh-agent 回执继续。Task/spec/freeze 未变可走同 Task retry/new-start；改变则显式 successor assignment，保留 role/Task/Dispatch lineage、同一 Run/产品 attempt 与旧证据，新证据用新路径。行政中断不等于产品退回/通过，修改冻结输入前明确中断旧评审并重新完整冻结。send 恢复旧超限 transcript、新 terminal handle、取消请求、静默和超时都不能证明新上下文或旧进程退出。

同一 session 的实际 `ORCA_TERMINAL_HANDLE` 缺失或不等于保存 terminal 时立即锁存首个 `terminal_drift`，即使计数 0、角色已结算或 registry 已改到新 terminal 也停止普通工作与压缩。保留 owner、旧 terminal、单调计数与笔记；返回原 terminal 不解除 fence，checkpoint 不得冒用旧 caller。此时只允许 Read、精确 `session-status --session ACTUAL_SESSION_ID` 和一条有界 escalation：`orca orchestration send --from ACTUAL_TERMINAL --type escalation --subject '简短主题' --body '简短事实与停止请求' --task-id ACTUAL_TASK --dispatch-id ACTUAL_DISPATCH`。六组参数不重复、不增项，身份来自实际 preamble，由 Orca 校验；最多 48KB、无换行/分号/命令替换等 shell 元字符。禁止 Write/Edit、Skill 和任意 shell；Stop 含递归 Stop 可结束。

协调者从实际 state/transcript 与保留工作采集行政 handoff，记录实际/保存 terminal、session、计数、原 lineage、工具与旧 checkpoint 未成功。确认精确 mismatched assignment exited/released 后再开 fresh session，按前述新会话证明接续；旧 session 保持 fence。不改绑定/清零/信任，不把协调者采集说成 worker checkpoint 成功。此 drift 恢复仍由协调者执行，helper 不自动 stop/launch worker。

## 未知结果、历史与验证边界

launch 解析已安装 CLI 的 `result.prompt`：仅 `delivery=submit` 且 `outcome=journaled`（含字符串 messageId）或 `handed-to-terminal` 确认送达，不接受 `result.launch.prompt`。结构化 `journaled` 只证明投递，不证明真实 PTY 继任者；还须实际目标、fresh initial0 claim 与精确 PTY 关闭证明。claim 早于 launch 回执时先保存实际后到回执，不以缺回执猜成功。

`launching`、checkpoint `notifying` 或 close attempt 已记录都表示副作用可能发生；失败/超时/未知不 fresh 重试。保留原始 stdout/stderr/退出码或 timeout、用精确 status/session-status 与有界 `orca terminal show --terminal ACTUAL_HANDLE --json`、`orca terminal list --worktree path:/absolute/worktree --limit 20 --json` 检查；Run 路径还可查精确 run-show/run-current。缺可靠证明则阻塞并安排受审恢复，无自动 reconcile/重发解锁接口。`handoff_ready` 不证明旧 worker 已退出。

2026-11-05 发现验证会话已有 10 次压缩：旧实现只约束协调者且 follow-up 可恢复旧对话。该旧进程已按用户指令 guarded stop，原报告和 freeze07 保留。首次操作评审的 7 个反例、R1/D1 退回、operational03 FAILED 和 policy03 对旧九文档快照的结论均为历史证据，修复不会改写它们；当前输入须新冻结复核，不能把旧缺口继续描述为现实现行为，也不预判新实现已获独立接受。历史原生信任失败/未执行与 Full VP-013/capture 未接受的边界保留。

历史来源包括 2026-11-05 对 Claude Code 2.1.318、Codex 0.166.0 的本机实现/schema 核对，以及 [Claude PreCompact](https://code.claude.com/docs/en/hooks#precompact)、[Codex hooks](https://learn.chatgpt.com/docs/hooks)；当前 Codex 0.167 压缩适配与 scheduler 阻塞分别按上文执行。[planning-with-files](https://github.com/OthmanAdi/planning-with-files/blob/8eef6dc6540dde5eef3fdc0ac487ad9c48ef3ba5/hooks/claude-hook.sh#L304-L345)、[HumanLayer handoff](https://github.com/humanlayer/humanlayer/blob/f9bd73534a9fb7099a274f951b698b1297122fd9/apps/riptide-api/src/orpc/api/sessions/handoff.ts#L40-L114)、[Ralph](https://github.com/snarktank/ralph/blob/6c53cb0b831ebe8739c6a003e22af14902d8b0b5/ralph.sh#L84-L108) 的持久状态/事务/新进程思路不授予权限绕过或文本完成哨兵。

隔离 fixture/mock 仅验证对应逻辑与失败分支，不能当实际完整 compaction、stop/release、launch/claim/close 或 scheduler 迁移生命周期。真实只读登记身份核验只证明所查身份与来源，不证明继任已启动、claim 或旧 PTY 已关闭。真实新会话加载观察与历史单步回执按其实际范围引用；未执行的 PreCompact veto、完整自然压缩跨窗和原生 scheduler 路径继续标未测/阻塞。本轮验证由实际执行者另附版本、命令、退出码与范围，不从本文推断测试总数或 smoke 成功。

2026-11-10 本轮开发验证：Claude Code 2.1.322、Codex CLI 0.167.0；`python3 -I -m unittest discover -s .claude/hooks -p 'test_*rotation.py'` 共 110 项通过（原 78 项 + 新 32 项）。覆盖最终轮补登记、full membership 分页、原契约与暂停、fresh claim、单次副作用、精确关闭、legacy 升级、动态/cron 持久来源，以及主动提前交接和真实启动前 transcript 缺失的时序回归。24 个本地文档链接、hook JSON、Python 语法及差异检查通过。

同日通过 `orca-cli` 执行了一次真实、独立、无产品操作的 Claude 顶层交接：本人 cron 建立 → 主动 requested 后删除/最终列表为空 → retire → 单次真实 CLI launch → 不同 session/terminal、真实 initial0、晚于 launch 的 transcript/状态 → claim → 精确旧 handle/incarnation 关闭且 ptyKilled=true → 核对暂停后按完整原 prompt 建立恰好一份 cron → 立即删除并确认列表为空。监督会话从实际状态、packet/notes 哈希、原始配对 scheduler 回执和关闭回执另行复核；最后确认继任 idle，再精确关闭该测试窗口，两代均无遗留调度。私有证据为 `.voiceport/handoffs/rotation-acceptance-20261110-{result,verified,cleanup}.json`，不提交 Git。此次没有强制 compact 或清零计数；两代真实计数均为 0，使用主动交接入口。因此已验证开窗/认领/关闭/cron 恢复链，未验证自然四次压缩到上限、实际定时触发、Run/worker 完整实机生命周期或原生独立 scheduler。历史产品/独立验收结论不由此改写。

Git、产品账本与外部动作只按本次实际授权；顶层轮换本身不扩大权限。具备原产品协调授权者继续携带未完 Git 步骤，不能把集成或行政交接当验收。配置/角色文档属于冻结输入，修改前明确中断旧评审并保留快照，不能静默漂移。
