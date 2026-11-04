# 私有交接与验收模板

本文件只保存结构；复制到 `.voiceport/handoffs/` 后填实际值。角色身份登记到 `.voiceport/roles.md`，报告放 `.voiceport/evidence/`；公开模板不得填现场句柄、真实消息、录音或凭据。未知填 `unknown`，不适用写明原因；不要保留会被误认为真实回执的示例 ID。

## A. 协调者的私有角色注册

```yaml
role: <developer | verifier | evaluator | protocol-support>
role_session: <实际宿主会话标识；不是自造的角色名>
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
workspace: <共享但文件隔离，或 Orca 创建的实际独立工作区>
allowed_files: <明确文件或目录；包括可写的私有证据范围>
forbidden_files: <至少明确其他 worker 文件、账本、根 planning 和未授权范围>
acceptance: <可观察、可判定的本次完成条件；不得弱化 VP 标准>
authorized_actions: <本地读写/测试等；真实账号/录音/提交/外部操作分别据实际授权>
required_checks: <与影响相关的检查及真实环境条件>
completion_evidence: <文件清单、版本、自检或独立报告、命令与退出码>
next_receiver: <原协调者角色；冻结后才交独立验证>
join_before_freeze: <必须结束的所有开发/协议/集成写者及核对方式>
pause_reference: <当前暂停来源及约束；无暂停也须核对>
```

发送前协调者检查允许集合无冲突、依赖满足、原角色可达。runtime run/task/dispatch 只能在 worker-start 实际返回后填注册；worker 使用注入的 preamble，不执行 spec 中伪造的 from/Task/Dispatch。接收与结果记录含 UTC 时间，文件写出本身不算送达。

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
additional_document_hashes: <docs/README 等 source_sha256 未覆盖的实际文件摘要>
self_checks: <开发自检的命令、退出码、输出/报告路径及运行版本>
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
