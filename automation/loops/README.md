# VoicePort 工作会话入口

共用规则以 [多会话 Loop 契约](../../docs/LOOP_METHODS.md) 为准；本目录提供可用于真实 Orca 分派的角色提示词和交接模板，不负责启动模型或安装后台循环。

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

私有角色注册约定为 `.voiceport/roles.md`，交接记录为 `.voiceport/handoffs/`，证据为 `.voiceport/evidence/`；这些路径按需由协调者维护，当前脚本不会自动创建角色注册或投递消息。现场句柄、回执和音频不写入本目录。产品当前状态只查 `.voiceport/loop-state.json`，不在注册文件另存一份产品状态。
