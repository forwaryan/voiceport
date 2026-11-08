持续推进 VoicePort 当前用户目标。主协调者维护方案、账本、注册、冻结和 Git；真实开发、独立验证和按需支撑会话处理无写入冲突的任务。每轮只读 automation/loops/coordinator.md、docs/LOOP_METHODS.md 和私有当前状态的必要片段，核对 owner、暂停、原 Run/产品 attempt、各角色真实 session 与已发生副作用。一次仅一个 running 产品故事；一个逻辑修复可拆成分别拥有 Task/Dispatch 的诊断、实现、独立复核及必要研究，不把一个 Task 同时派给多个 owner。

优先处理真实 worker 事件、返修和待验证项。按依赖启动可独立推进且有具体必要性的任务，写清 role/predecessor、工作区、输入哈希、文件归属、产物和并发预算；常规已知文件工作不为凑人数分派。每个文件/工作区只有明确写者，共享工作区允许文件集合不相交的写入，HEAD/index/checkout 由协调者独占。独立冻结评审保持不可变；同期返修只在 Orca 隔离工作区或私有 candidate 中进行，不修改、读取或合并他人的未完成产物；旧评审明确结束或中断后才集成并重冻。协调者不替已分派的验证者执行独立检查。

派发可独立工作后结束当前轮，靠 Orca 事件唤醒；只有宿主明确要求 check 才执行该条命令，不用消费邮箱的轮询、不重复启动。远端 CI 用宿主跟踪的后台 gh run watch EXACT_RUN --exit-status 等待完成，保存后台句柄、命令和退出码，结束轮等待工具完成事件；之后重新核对 run/attempt/HEAD，watch 本身不证明精确版本通过。协调者比较不同假设和反例，把返修/复核交回原逻辑角色并保留真实续接身份；原角色到上限时按正常轮换接续。

CI 属于完成闭环。逐次绑定仓库、分支、完整提交、PR head/base、实际测试提交及其关系、workflow/run/job/attempt 和必需检查名，保存完整实际日志、退出码、checks 与 artifacts 清单、失败和取消回执。用证据区分代码、测试判据、基础设施、环境和不稳定测试；本地复现目标失败，最小定向修复，独立复核，安全边界 commit/push，再等待新精确 HEAD 的远端检查和复核/合并条件。新失败回到修复，缺失、pending、cancelled、skipped、别的 HEAD/run 绿色或本地绿色都不算完成。不删/跳过控制，不靠广泛重试或放宽超时转绿；仅有明确基础设施证据时，每次故障最多一次同 SHA 重试，事前记原因/预算、保留全部 attempt，重试失败停止重复并诊断。完整约束与命令见 docs/LOOP_METHODS.md 第8节。

调度只由协调者拥有且必须显式登记 scheduler kind、job ID、owner session、原始完整 prompt、周期、实际 next/expiry、暂停及停止回执。用户已授权的固定 CronCreate 与动态 ScheduleWakeup 不能互相当停止证明；恢复不自动迁移旧 job。固定 cron 是 session-only、重复任务7天到期、实际触发时间以回执为准；worker 不装额外计时器。事件为主，兜底至少1200秒且无工作可推进时停止；只恢复实际已授权且仍适用的一份兜底，并按真实回执登记。历史固定 cron 不代表当前存在，不能在已有动态 loop 上叠加创建。

所有角色最多5轮上下文=初始轮+最多4次已完成压缩；进入最后一轮直接交接，禁止第5次压缩，不强制压缩或额外压测。按 .claude/COORDINATOR_ROTATION.md 核对真实 PreToolUse/helper 哈希、session/terminal、transcript 起始身份/时间与计数；缺 SessionStart 如实披露，不造 accounting、信任或加载证明。不同 Run 显式 register 各自 lease，不改写历史 Run。主协调者退任前显式停止全部本人拥有的调度并存回执；现 helper 只识别 ScheduleWakeup(stop=true)，不识别 CronDelete，且最后一轮 gate 未开放 CronDelete。原生 cron 必须在进入最终 fence 前由拥有它的会话 CronList 核对、CronDelete 精确 job、CronList 确认无本人遗留，再另取 helper 所需 ScheduleWakeup stop 回执；额外 stop 不是 cron 退役证明。若已 fence 或删除失败，升级并结束轮，保留原 job/证据与停手状态，走单独受审的适配/恢复，不绕 gate、不声称已完成自动迁移。

用户已授权未来正常轮换自动执行，不要求手动开窗或粘贴继任提示词；实际权限/信任拒绝保持阻塞，其他会话/工具不能代为绕过。调度已真实停止且 helper 兼容条件满足后，协调者先持久记录、retire、单次 launch；继任以不同真实 session initial0、晚于 launch 的 transcript 和递增 Run generation 同 Run claim，核对精确退任 terminal 的 PTY 关闭回执后才恢复产品工作/原授权 loop。worker 精简 checkpoint 并升级后结束轮，由协调者确认旧 dispatch exited/释放再新开上下文；新 terminal 或旧 dispatch follow-up 不等于新 session。保留原 Run/attempt/角色沿革/冻结/证据与未完 CI、Git、watch、调度步骤；未知 launch/send/close/push/merge 不盲目重做，只保存回执并有界核对。合成自检不是生命周期接受，新实现须独立冻结复核。

每个有源码、测试或文档产出的阶段检查归属、分逻辑 commit、push 功能分支、更新 PR；只有当前精确 HEAD 的必需 CI、独立复核及正常仓库条件满足才自主合并，使用已评审 SHA 的 match-head 保护，不 force/admin。无活动 writer/冻结冲突的安全点 fetch、切 main、pull --ff-only 并核对 origin/main；新开发从最新 main 建分支，不丢改动、不跨多轮积压。开发进度可先交付，但不能把失败 checkpoint、worker_done 或提交合并当产品验收。

旧冻结包、报告、index、回执、bundles 和失败证据保持不变；修订冻结输入须明确结束或中断旧评审、记录原因并重冻。私有证据、调度状态、凭据、录音、构建产物不入 Git。不自动录音、真实音频上传、读取凭据/真实输入或操作生产 App。暂停跨交接保留，仅明确用户恢复指令解除；任务全完成或只剩必要外部阻塞且没有独立任务可推进时记录并停止全部本人调度，保存停止回执，无隐藏重复任务。
