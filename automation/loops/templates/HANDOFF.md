# 私有交接与验收模板

本文件只保存结构；复制到 `.voiceport/handoffs/` 后填实际值。角色身份登记到 `.voiceport/roles.md`，报告放 `.voiceport/evidence/`；公开模板不得填现场句柄、真实消息、录音或凭据。未知填 `unknown`，不适用写明原因；不要保留会被误认为真实回执的示例 ID。身份、完整命令、调度停止 gate 以 [全角色会话轮换](../../../.claude/COORDINATOR_ROTATION.md) 为准。

直接打开且无 Run、非托管 worker 的顶层会话，安装 loop 前另写精确 `.voiceport/handoffs/session-contract-ACTUAL_SESSION_ID.json`，仅含 goal、完整原文 loop_prompt、authorization、paused 布尔值、active_tools 列表，并 `python3 -I .claude/hooks/coordinator_rotation.py session-register --session ACTUAL_SESSION_ID --contract .voiceport/handoffs/session-contract-ACTUAL_SESSION_ID.json`。真实 PreToolUse/terminal/transcript 与穷尽 all-Run worker-list、registry 非 worker 证明缺一即阻塞；不能套用角色模板虚构 Run。既有最终轮可补精确 contract/register，计数不清零。登记前结算已有工具，active_tools 须真实为 []；冻结契约是原始登记快照，由继任继承，不扩大产品协调或 Git 权限。退任前另查并结算当前工具，真实结果、最新用户约束与剩余工作写有界 notes；初始 [] 不证明当前无活动工具。

已登记独立顶层可用 `session-request --session ACTUAL_SESSION_ID --reason '实际原因'` 提前进入同一流程；记录 voluntary 原因和真实压缩数，不改阈值/计数，不覆盖自然上限请求；前代未关闭时拒绝。演练须分别记录真实开窗/认领/关闭/恢复调度与未执行的自然压缩。顶层 requested 后 notes/停止全部自有调度 → session-retire → 单次 session-launch → 新实际 initial0 会话 `session-claim --session NEW --source OLD --token TOKEN` → `session-close --session NEW --source OLD --terminal EXACT_OLD`。确认精确旧 PTY 退出后才按原授权/暂停恢复产品与一份完整原 prompt 的 loop。完整 helper 前缀和证明见中央文档，命令逐条调用。以下 Run/Task/Dispatch 字段只供实际协调者/worker 使用；托管 worker 不自行 launch 或装 loop。

## A. 协调者的私有角色注册

```yaml
role: <developer | verifier | evaluator | protocol-support>
role_session: <实际宿主会话标识；不是自造的角色名>
rotation_startup_proof: <真实新 session、terminal、harness、transcript 起始身份/UTC/实际压缩记录与首次计数0；实际 PreToolUse 状态路径及指定 helper 哈希；SessionStart 未观察到须披露>
hook_loading_and_trust: <当前进程真实加载证明；原生 Codex 用户原生 UI 信任状态，未证明则阻塞；配置存在不算加载，不代批或改信任库>
host: <回执所属宿主>
workspace: <实际工作区标识及路径>
agent: <实际启用的 agent；仅用户指定时设置模型>
original_dispatch: <首次实际 dispatch>
current_run_id: <实际 run>
current_task_id: <实际工作 task>
current_dispatch_id: <实际 dispatch>
predecessor: <有 follow-up/retry 时保留前次身份；否则不适用>
start_receipt_path: <原始启动回执私有路径>
message_receipts: <实际 send/ask/reply/worker_done 回执路径；含时间、确认结果及宿主 next>
resume_route: <原角色 dispatch；失效则记原因，不伪造已恢复>
```

只登记导航与实际回执，不在这里双写 ready/running/done 等产品当前状态。一个角色会话可以有后续多次实际 Task/Dispatch；沿真实回执更新，不用角色别名替代 runtime 身份。

## B. 分派 spec：与角色提示词合并后发送

```yaml
request_reference: <用户原请求或协调者分派来源>
role_prompt: <automation/loops 中本次角色文件路径>
objective: <本次必须交付的具体结果>
product_task_id: <VP-xxx；纯支撑工作未关联时 unbound>
product_attempt_id: <loop_state start 真实返回值；未关联时 unbound>
relationship: <implements | verifies | evaluates | supports>
product_dependencies: <来自 automation/tasks.json；不适用需说明>
orca_dependencies: <实际前置 task ID 与完成事实；没有填空数组>
predecessor_assignment: <返修/复核时旧身份与反例报告路径>
workspace: <实际 Orca worktree 路径；只读共享且证据分离，写入验收输入时记录独占 owner>
allowed_files: <明确文件或目录；包括可写的私有证据范围>
forbidden_files: <至少明确其他 worker 文件、账本、根 planning 和未授权范围>
acceptance: <可观察、可判定的本次完成条件；不得弱化 VP 标准>
authorized_actions: <本地读写/测试等；真实账号/录音/提交/外部操作分别据实际授权>
required_checks: <与影响相关的检查及真实环境条件>
completion_evidence: <文件清单、版本、自检或独立报告、命令与退出码>
next_receiver: <原协调者角色；冻结后才交独立验证>
join_before_freeze: <所有相关写者和必要前置任务的交付、证据及写者结束确认>
verification_budget: <本机轻量检查或重型完整套件、资源时段、主机与 owner；有远端 CI 时附实际 run 关联>
pause_reference: <当前暂停来源及约束；无暂停也须核对>
```

收到目标或 worker 回报后，协调者先核对同 Run 的真实 ready Task、依赖、未结算 dispatch 与资源。两个以上互不依赖且可验证的工项，在原授权及资源允许时分别分派，每个 Task 记录唯一 owner。发送前核对允许集合、工作区归属和原角色可达性。runtime run/task/dispatch 只能在 worker-start 实际返回后填注册；worker 使用注入的 preamble，不执行 spec 中伪造的 from/Task/Dispatch。接收与结果记录含 UTC 时间，文件写出本身不算送达。

## C. 最终冻结包：所有相关写者结束后填写

```yaml
freeze_id: <协调者生成的版本导航编号；不是内容证明>
product_task_id: <当前 VP>
product_attempt_id: <当前真实 attempt>
producer_assignments: <本版开发/协议/集成的实际 run/task/dispatch 及已结束证据>
workspace: <实际验收工作区与平台>
base_commit: <完整 SHA；若基线含未提交内容，另附基线快照>
head_commit: <完整 SHA；有未提交内容须同时提供下面的实际快照>
changed_files: <修改、新增、删除完整清单，含未跟踪文件>
freeze_inputs: <本版相关源码、测试、脚本、配置、文档及其他验收输入的完整范围>
manifest_path: <相对路径、类型/必要模式、内容 SHA-256 与删除项；不得读取敏感文件>
snapshot_or_patch: <验证会话可读取的实际冻结文件或完整差异；含未跟踪内容>
inventory_rule: <如何发现相关新增/删除项，列明排除构建/证据/私有敏感目录>
source_sha256: <若引用 verify.py，给其真实摘要和报告路径>
additional_document_hashes: <按实际输入清单补 `.claude/`、`.codex/`、根 planning、`.planning/` 等；schema 2 已含 docs/README，不重复宣称未覆盖>
self_checks: <开发自检的命令、真实退出码、输出/报告路径及运行版本；取消/超时/未执行分开记录>
verification_resources: <证据目录、完整套件实际持有者和同用户主机/checkout 锁状态、active_check、真实 PID/进程组及清理确认或未知>
failure_history: <首次失败与全部 attempt 日志、诊断、责任人和剩余预算；后续绿色不覆盖首次失败>
evidence_integrity: <finish 保存的报告路径/SHA-256/字节数；audit intact/changed/missing/unsealed；字节完整不等于独立验收>
acceptance: <原验收标准、反例和必需真实场景>
verifier_assignment: <指定的不同真实会话；启动后填写实际 run/task/dispatch>
write_freeze_confirmed: <所有写者已停写相关输入的确认；检查开始与结束再核对>
known_limits: <已知缺陷、外部条件、未测、引用证据>
```

接收方亲自核对实际内容；哈希、HEAD、freeze_id 单独都不足以证明版本。新增/删除项须在开始和结束重新发现，不能只对旧清单里的文件算摘要。变更先作废旧结论并重冻，原验证复核修复与相邻路径；不得在验证者读取时静默修改。

## D. 结果报告：绑定本次实际身份与冻结版本

```yaml
role: <实际角色>
product_task_id: <VP 或 unbound>
product_attempt_id: <本次真实 attempt 或 unbound>
orca_run_id: <本次 preamble/回执中的实际值>
orca_task_id: <本次实际值>
orca_dispatch_id: <本次实际值>
freeze_id_and_manifest: <实际验证版本；未进入冻结阶段说明不适用>
started_at_utc: <实际开始>
finished_at_utc: <实际结束>
files_modified: <仅本角色实际修改/新增/删除的文件>
checks_executed: <逐项命令、实际退出码、输出摘要与证据路径>
checks_cited: <引用者、路径、版本及无需重复执行的依据；不冒称亲自执行>
counterexamples: <输入、预期、实际结果及旧实现目标断言失败/修复证据>
freeze_check_start_and_end: <实际清单/摘要是否一致，是否出现新文件/删除>
review_conclusion: <通过 | 退回 | 阻塞 | 不适用；开发不得填独立通过>
effect_matrix: <离线、无 I/O 启动、真实录音、云端、目标接收、同音频 A/B 各格结论及证据>
limits_and_unblock: <未验证范围、缺失条件、负责人及解除事件>
next_action: <具体接收者和待做事项，不替协调者 finish VP>
runtime_outcome: <succeeded | failed；本次工作完成与验收通过分别判断>
completion_receipt: <实际发送后由协调者关联原始 worker_done 回执；未确认则记未送达>
```

开发“完成实现”、验证“完成评审但退回”与产品验收通过不同。验收本身未能完成时 runtime outcome 用 failed；完成明确的负面评审可 succeeded，但必须保留退回/失败的产品结论，协调者不能据此 finish。纯文档报告实际内容/链接/接口检查即可，不填未执行的产品测试。

## E. worker 的消息和结束

使用当前 preamble 给出的命令、from、Task 和 Dispatch；不要从本模板猜运行身份。实际 orchestration 技能选定的可执行文件替换下面的 `ORCA` 占位符，消息正文正确引用，避免 shell 展开。

```text
ORCA orchestration ask --from <本次from> --question <需协调者决定的具体问题>
ORCA orchestration send --from <本次from> --type escalation --subject <具体阻塞> --body <证据与解除动作> --task-id <本次task> --dispatch-id <本次dispatch>
ORCA orchestration send --from <本次from> --type worker_done --subject <准确状态> --body <已做一句、发现一句、剩余一句> --task-id <本次task> --dispatch-id <本次dispatch> --outcome <succeeded或failed> --files-modified <真实逗号分隔列表> --report-path <真实报告路径>
```

没有修改文件时省略 files-modified，没有报告时省略 report-path；不能填不存在的值。ask 后结束当前轮等待回复；worker_done 仅发一次，失败按宿主回执恢复，确认后结束当前轮。现场协议冲突或不能判定时交协调者，不用本地提问工具替代 Orca ask，不循环 check，不开始其他任务。

## F. 全角色上下文轮换

所有会话（含直接顶层、协调者、开发、独立验证、协议支撑和效果评估）最多 **5 轮上下文 = 初始轮 + 最多 4 次已完成压缩**。进入最后一轮立即利用剩余空间保存交接；当前工具自然返回后不再开新工作，禁止第 5 次压缩，不强制 compact、不额外压测。实际接口与宿主边界见 [全角色会话轮换](../../../.claude/COORDINATOR_ROTATION.md)。旧全局 lease 的同一实际 owner/Run 须在 active/requested、未尝试 transfer 时，先 `python3 -I .claude/hooks/coordinator_rotation.py upgrade-legacy --session ACTUAL_SESSION --run ACTUAL_RUN`；保留 epoch/计数/原目标/完整 prompt/request/history/worker 冻结，转 per-Run 后带 --run 退任/接管/精确关闭。已有 Run lease、transfer 已尝试或沿革不明时阻塞，不手改状态。隔离测试不证明真实迁移已经执行。

退任前停止全部自有调度，私有记录须保留 scheduler kind、owner、ID、完整 prompt 与 tool-use/result 证据。动态须最后一次 ScheduleWakeup({stop:true}) 的结构化 stopped:true；cron 须同 session 成功 CronCreate 的 ID/prompt 来源、所有自有 ID 的配对成功 CronDelete 和最后 mutation 后 CronList 全部自有 ID 不存在。最终 gate 允许精确 stop、CronList({}) 与仅含已证明自有 id 的 CronDelete；未知/缺失 ownership 失败关闭，不删他人 job、不创建或续订。顶层冻结 active_tools 非空时当前 helper 阻止退任，无手改契约解锁接口。登记时须真实结算为 []，退任时另查当前工具并保存真实结果，不能以初始快照代替退出证明。scan 持久保存动态调用/停止/未决结果和 cron ownership/未决 mutation；截断不清除任何已知调度义务，动态 stop 须以同一快照先后关系或真实 UTC 时间证明晚于所有已知调用，恢复早期 stop 不覆盖较晚调度；停止仍须对应回执与最终列表，不手写状态充证明。

新角色接单前核对真正新 session，不能仅凭新 terminal：从真实 transcript 起始身份/时间与完成压缩记录，核对 hook 状态的 `owner`、`terminal`、`harness`、`boundaries=[]`、PreToolUse 与当前 `hook_implementation_sha256`；继任还核对状态/起始时间均晚于 launch。普通 session 状态在 `.voiceport/session-rotations/<actual-session>.json`，Run 协调者另有 lease，不混写。Claude 按 `compact_boundary` UUID 去重；Codex 0.167 适配按 `session_meta.payload.id` 核验并以 `compacted.payload.window_id` 去重，恢复不清零。SessionStart 缺失如实披露，不伪造状态/事件或把测试 stdin 当加载证明。原生 Codex 具体 hook 须用户原生 UI 信任并证明当前进程加载，不写信任库或代批；历史失败 Task 不因 Claude 支撑成功而改变。Claude PreCompact 的 `decision:block` / reason（或 exit2）正确，无需改成 continue:false；Codex PreCompact 用 `continue:false`，PreToolUse 用 `decision:block`。原生 scheduler 尚无 VERIFIED 接口，退任适配仍阻塞。CLI 窗口轮换单次显式 --surface terminal，不改全局偏好，只接受 terminal outcome；hook 加模型程序不是崩溃监督器，隔离测试不是完整自然压缩生命周期，结构化 launch 送达不是 PTY 证明；未执行的实际 veto/全流程继续标未测。

**先检查实际 terminal 绑定。** 同一 session 的实际 `ORCA_TERMINAL_HANDLE` 与 hook 保存 terminal 不一致或缺失时，即使计数 0、角色已结算后重开或 registry 已移到新 terminal，也立即停止普通工作与压缩。helper 锁存首个 `terminal_drift`，保留实际 session、旧 terminal、单调计数和旧笔记；返回原 terminal 仍 fence，checkpoint 不得冒用旧 caller 或宣称成功。worker 可 Read、精确 session-status、本人调度的精确停止接口，以及以当前真实 preamble 的 from/Task/Dispatch 发有界 escalation，然后结束 turn；不能 Write/Edit 笔记、执行 Skill 或任意 shell。escalation 命令只用 `orca orchestration send` 的六组参数 `--from ACTUAL_TERMINAL --type escalation --subject '简短主题' --body '简短事实与停止请求' --task-id ACTUAL_TASK --dispatch-id ACTUAL_DISPATCH`，不重复、不加参数，最多 48KB 且无 shell 元字符/换行/分号。Orca 校验真实 dispatch，不能借旧 registry 代发；Stop（含递归）允许结束，双宿主 veto 格式保持上述规则。

协调者从真实 state/transcript 与保留工作采集行政 handoff，记录实际/保存 terminal、session、完成压缩数、原角色/Task/Dispatch 沿革、笔记/证据、活动工具和旧 checkpoint 未成功。依 recovery 停止精确 mismatched assignment，取得真实 exited/released 后启动 genuinely fresh session；spec/freeze 不变走同 Task retry/new-start，改变则建显式 successor，保留 Run/产品 attempt 和历史。验证新 ID、实际 terminal 与 hook 状态一致、真实 PreToolUse 当前 helper hash、bounded transcript 初始身份/时间与零计数；SessionStart 缺失如实写 startup addendum。旧会话持续 fence，不能重绑/清零/改信任或仅 send 恢复旧 transcript。该 worker drift 恢复由协调者执行，helper 不自动 stop/launch worker；mock 恢复模型不代表真实生命周期接受。历史 operational03 FAILED、policy03 旧九文档快照和原生信任 Task 的失败/未执行结论保留，不用修订文档改写；当前输入须新冻结复核，Full VP-013/capture 未接受的范围保持。

无 Run、非 worker、已登记原五字段契约的 Claude 顶层 drift，不依赖不存在的协调者。先精确停止全部本人调度，再执行唯一开放的 `python3 -I .claude/hooks/coordinator_rotation.py session-recover --session ACTUAL_FENCED_SESSION`，由单次 fresh 管理会话采集行政 handoff，核验精确冲突 PTY 退出后启动 fresh 产品会话；继任真实 initial0、启动后身份/时间、契约/暂停和 notes 哈希复核后接续原任务。每次副作用先 journal，未知不 fresh 重试；旧计数/fence 保留，不伪造旧 checkpoint。未暂停且未尝试恢复时的非递归 Stop 提醒须执行，不能让用户手动复制提示词。暂停、权限/信任、Run/worker 边界仍按中央文档执行。

terminal 绑定一致且没有 drift 锁存的 worker 在 `.voiceport/handoffs/session-rotation-ACTUAL_SESSION_ID.md` 保存非空、最多 48KB 的精简笔记，使用真实身份，不填示例回执：

```yaml
actual_session_and_terminal: <实际 session/terminal/harness 与当前轮数、完成压缩记录和加载证据>
run_task_dispatch: <当前 preamble 的实际 Run/Task/Dispatch>
role_predecessor: <原角色、原 Task/Dispatch 与实际续接链>
product_attempt_and_freeze: <同一产品 attempt、当前冻结 pins；行政中断也保留旧快照>
completed_evidence: <已完成命令、原始日志/报告路径、退出码；引用与亲自执行分开>
active_tools: <真实工具句柄/运行或结束状态；未知写 unknown，不以超时当退出>
provisional_findings_and_remaining: <暂定发现、未做项、权限与证据写入范围>
```

先保存笔记，再执行以下接口。原生 Codex 若尚未创建该笔记文件，可改用同命令的 `--notes-text '单行精简笔记'` 一次创建并提交；已有笔记时不传此参数，helper 不覆盖旧文件，不能借此执行其他命令：

```sh
python3 -I .claude/hooks/coordinator_rotation.py checkpoint --session ACTUAL_SESSION_ID
```

helper 对照实际 caller terminal/session 与精确注册身份，只接受限定私有路径内普通笔记文件（非符号链接），记录并复核哈希，再单次 escalation。首次操作评审的7个反例与 R1/D1 退回是历史证据，当前输入须新冻结独立复核，不用修订文档改写旧结论，也不把旧缺口继续当成当前实现描述。实际加载、未观察的 SessionStart、未执行的 PreCompact/完整生命周期和当前原生信任条件分别记录，不能外推通过。未知送达先交协调者查实际回执，不盲重发。checkpoint 只是行政交接，不结束未完成产品工作、不伪造 worker_done；worker 随即结束 turn，不启动其他 agent、worker 或循环，不接管主协调者 loop、账本或 Git。

协调者核对交接和活动工具后，对精确旧 dispatch 依 Orca recovery 作 guarded stop，必须取得实际 exited/资源释放证明；取消请求、静默或未知存活都不是退出。之后用 `worker-start` 和真实 fresh-agent 回执续接，禁止把 send 恢复超限 transcript 当新上下文。原 Task/spec/freeze 未变可按实际 retry/new-start 恢复；已改变则建立显式 successor assignment，保留原角色/Task/Dispatch lineage、同一 Run/产品 attempt 和旧证据，新证据用新路径。修订冻结输入前须明确中断旧评审并建立新完整冻结，不能静默漂移，也不能把行政中断当产品通过或退回。协调者仍在原 loop 处理事件；自身轮换另依 lease 的停止调度、retire/launch/claim 流程，继任显式恢复已授权的 loop。普通工作真实完成时才按 E 节发送一次准确 outcome 的 worker_done。
