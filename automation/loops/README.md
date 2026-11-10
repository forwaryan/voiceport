# VoicePort 工作会话入口

共用规则以 [多会话 Loop 契约](../../docs/LOOP_METHODS.md) 为准，身份、最终轮 gate 和完整命令集中在 [全角色会话轮换](../../.claude/COORDINATOR_ROTATION.md)。本目录提供真实 Orca 分派的角色提示词和交接模板，本身不启动模型或安装后台循环。开发自检/验收须同时遵守 [执行、证据与资源](../../docs/LOOP_EXECUTION.md)：只读账本查询/摘要 audit、稳定非空完成证据、同 checkout 完整 suite 互斥、逐步报告、完整日志与超时进程组清理。

直接打开、无 Run 且非任何托管 worker 的顶层会话，安装 loop 前先写自己的精确私有 session-contract JSON（goal、完整 loop_prompt、authorization、paused 布尔值、active_tools 列表），按中央文档 session-register；真实加载与穷尽 all-Run worker-list/registry 的非 worker 证明缺失时阻塞。最后一轮 notes/停止全部自有调度 → session-retire → 单次 session-launch → 新实际 initial0 会话 session-claim → session-close 精确旧 terminal，确认 PTY 停止后才恢复原授权 loop。既有最终轮可补精确 contract/register，不能清零计数；登记前须结算工具使 active_tools 真实为 []；冻结契约仅是原始登记快照，退任前仍须重新检查/结算当前工具，将真实结果、最新用户约束和剩余工作写有界 notes。不授予新 Run 或无关 Git 权限。以下角色分派仍由真实协调者控制，托管 worker 不自行 launch 或装 loop。

已登记的独立顶层可按中央文档 `session-request --session ACTUAL_SESSION_ID --reason '实际原因'` 主动提前交接，保留真实压缩计数、阈值和原请求，沿用同一认领/精确关闭步骤；有界演练不代表自然压缩到上限的验收。前代尚未确认关闭时不能开始下一代。

旧全局 lease 的当前同一实际 owner/Run 须在 retire 前按中央文档执行 `upgrade-legacy --session ACTUAL_SESSION --run ACTUAL_RUN`，仅 active/requested、未尝试 transfer 可升级；保留 epoch/计数/完整 prompt/worker 冻结和沿革，转 per-Run 后带 --run 完成退任/接管/精确关闭。已有 Run lease、transfer 已尝试或沿革不明则阻塞；不声称已执行真实迁移。

CLI 窗口轮换单次显式 `--surface terminal`，不修改全局偏好，只接受 terminal outcome；动态调用和其停止/未决结果跨 transcript 截断保存，恢复早期 stop 不覆盖较晚调度。所有角色均为初始轮加最多 4 次已完成压缩，第 5 轮立即交接。最终轮仅开放精确动态 stop、CronList({}) 与同 session 成功 CronCreate 证明所属 ID 的 CronDelete；停止须结构化回执及全部自有 cron 最终不存在证明。原生 Codex 0.167 压缩适配不等于 scheduler 支持，原生调度退任仍阻塞。hook 加模型程序不是崩溃监督器，隔离测试和结构化投递均不证明完整自然压缩/PTY 生命周期。

| 角色 | 提示词 | 接单条件与交付 |
|---|---|---|
| 协调者 | [coordinator.md](coordinator.md) | 唯一产品账本写者，分派、冻结、恢复与验收归档 |
| 开发者 | [developer.md](developer.md) | 接收具体范围，实现并自检，原会话处理返修 |
| 独立验证者 | [verifier.md](verifier.md) | 不同真实会话，在最终冻结后读实现、构造反例并报告 |
| 效果评估者 | [evaluator.md](evaluator.md) | 按需以独立会话核对用户效果、真实环境与 A/B 覆盖 |

1. 协调者将角色提示词和已填的 [HANDOFF 模板](templates/HANDOFF.md) 合成私有 spec；不得只发送一条“看某文件”而缺少目标、文件范围和验收条件。
2. 通过当前 orchestration 技能的真实 `worker-start` 分派，保存宿主返回的 Run/Task/Dispatch 和投递回执；原会话返修与复核用 dispatch 定向 send，按新回执更新身份。
3. worker 开始时读共用契约、自己的角色文件与本次私有交接包，遵守实际 preamble 的 Task/Dispatch；没有具体已分派任务时结束当前轮等待。
4. 每个角色只做分派范围。实现与协议文档可在无冲突的文件集合上并行，验收等待所有影响最终版本的写者结束并冻结；自检不能替代独立验收。
5. 完成发送一次有明确 outcome 的 worker_done，确认后结束当前轮；只记录文件不算已发送交接。worker 不另起角色、子代理或独立定时 loop。

私有角色注册约定为 `.voiceport/roles.md`，交接记录为 `.voiceport/handoffs/`，证据为 `.voiceport/evidence/`；角色导航由协调者按真实回执维护。轮换 helper 只执行中央文档限定的登记、交接和投递，不自动分配任意角色或产品任务。现场句柄、回执和音频不写入本目录。产品当前状态只查 `.voiceport/loop-state.json`，不在注册文件另存一份产品状态。
