# Loop 执行、证据与资源

本页补充 [多会话 Loop 契约](LOOP_METHODS.md) 的可执行检查。适用范围是本地开发验证和产品账本；会话轮换仍按 [轮换规则](../.claude/COORDINATOR_ROTATION.md)。不安装新 scheduler，不扩大产品、Git、录音或外部动作授权。

## 采用 Magpie 的哪些经验

参考固定提交 `86825005e26412ab05533de668b654942b226b98`，区分源项目规则和本项目新增实现：

| 来源 | 本项目落实 |
|---|---|
| [LESSONS 完整日志与失败识别](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/LESSONS.md#L373-L383)、[重试不能掩盖首次失败](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/LESSONS.md#L812-L814) | 每次验证使用全新目录，完整 stdout/stderr 写盘，报告保留逐项实际退出、超时与清理状态；账本保留旧 attempts。失败不自动重跑。 |
| [测试启动的资源必须结束](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/docs/code-standards.md#L33-L35) | 验证 runner 为命令创建独立进程组；超时/正常终止信号先 TERM、必要时 KILL，只操作自己启动的组。清理未知或残留后代不能通过。 |
| [重型测试并发故障及 suite lock 缺口](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/LESSONS.md#L356-L371) | `verify.py` 对同一 checkout 的完整验证加互斥锁；轻量定向检查可按资源预算并行。这是本项目的可执行改进，不宣称 Magpie 已实现全局本机锁。 |
| [明确实际命令及未执行检查](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/docs/code-standards.md#L39-L46)、[reviewed head](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/docs/code-standards.md#L109-L119) | 完成证据记录命令、完整日志、输入清单及开始/结束摘要；报告的 checks 只列实际执行项。提交/输入变化使旧复核失效，已授权合并仍须 match-head 保护。 |
| [CI 为依赖安装设超时](https://github.com/yetone/magpie/blob/86825005e26412ab05533de668b654942b226b98/.github/workflows/test.yml#L35-L44) | 本地每条验证命令使用明确期限，命令开始前保存 active_check，完成后立即保存结果。按本项目命令保留原有期限，不照搬 CI 的 15 分钟数字。 |

这些来源没有提供本项目的身份认证、独立验收或远端 CI 真值。摘要和状态检查不能代替实际验证者、宿主回执和精确提交的必需检查。

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
```

完整验证入口调用 [verification_run.py](../scripts/verification_run.py)：

1. 先取得 `.voiceport/verification.lock` 的非阻塞独占锁，冲突时明确拒绝启动第二套检查，不排队偷偷启动。
2. 证据目录必须全新；在运行命令前先写 `status=running`、`passed=false` 的报告，随后保存 active_check 的命令、开始时间、期限与日志路径。
3. stdout/stderr 直接写入该次检查的独立日志，完整输出不堆在 Python 内存中。仅需解析的 metadata/协议输出才有界读回，超过 1 MiB 拒绝解析并保留完整原日志。
4. 每项结束立即记录实际 `exit_code`、开始/结束时间、超时/中断标志、日志字节数/摘要与进程组清理结果。启动失败没有进程退出码，记 null；不把人为的 124/127 当作子进程真实退出码。
5. 超时、收到 SIGINT/SIGTERM、意外残留同组后代或清理未确认都不能通过。一个检查失败后不再启动后面的检查；checks 中没有的项目没有执行，不能写“全套通过”。
6. 只有已执行检查满足判据、输入开始/结束清单一致，最终报告才标通过。目录和失败日志不被下次运行复用或覆盖。

锁的范围是同一 checkout 的 `verify.py` 完整入口，不是全主机全局锁，不覆盖任意手工 cargo 命令或其他 checkout。协调者仍要安排跨工作区的 CPU/内存预算；不要删除活动锁文件来启动第二套。锁文件保留正常，由内核锁判断是否被占用。

普通 SIGINT/SIGTERM 会使验收失败并留下 interrupted 结果。清理与报告写入期间再次收到这两种信号时，先记录请求，待有时限的清理和当前写入完成后退出；日志收尾或等待之外中断也须保存最终报告，清空 active_check，并以 incomplete_check 保留未完成命令与日志位置，不编造未知退出码。进程组探测或发送信号被权限拒绝时，记录 cleanup_confirmed=false、cleanup_error，保留实际直接子进程退出码及原日志；必要时只通过自己持有的 Popen 结束、回收直接子进程，不据此宣称整组已清理。

清理证明只覆盖 runner 创建的进程组；检查不得自行 daemonize 或另建 session 逃离该组。宿主崩溃、SIGKILL、机器断电时，报告可能停留在 running，日志仍在；这表示验收未完成。清理未知、报告未完成时，恢复须核对精确 PID/组、命令、实际存活和已有副作用，不能直接重跑或将超时当退出。报告初始化和逐步落盘不能承诺硬断电后的完整持久化。

## 交接与复核

交接保留原失败、全部 attempts、未完成 active_check、进程/工具句柄和实际清理结果，不只贴最后一次绿色。失败、取消、超时、未测各自注明；按既有单次基础设施重试预算判断，不增加重试次数。

`verify.py` 的 schema 2 输入覆盖 README、docs、automation、scripts、Tests 等产品输入及未跟踪文件，实际清单见 `inputs-before.json`；`.claude/`、`.codex/`、根 planning 和 `.planning/` 不在产品清单中。涉及这些轮换/配置输入时，冻结包须另列实际文件和哈希，不能用产品摘要声称全部规则已冻结。实现或输入变化后重新冻结，原验证者按变化复核，不复用旧版本批准。
