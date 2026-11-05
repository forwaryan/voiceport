# VoicePort 协调者轮换

本项目默认在一个主协调者完成 **2 次压缩**后，停止该会话的动态 loop，保存精简交接包，在同一 checkout 启动新的 Claude 协调会话，并接管原 Orca Run。开发者和独立验证者保持原会话、Task/Dispatch、product_attempt 和冻结版本。

这是项目的协调工具，不是 Claude Code `/loop` 自带的换窗口功能。工具只维护 `.voiceport/coordinator-rotation.json`，不替代产品账本或 Orca 的实际 Run 所有权。当前实现面向本项目 macOS/POSIX 协调环境，需要 `python3`、`fcntl` 和支持 `agent launch`/`run-use` 的 Orca。

## 触发与计数

`.claude/settings.json` 安装 `PostCompact`、`SessionStart`、`PreToolUse` 和 `Stop` 命令 hook。PostCompact 之后扫描本会话 transcript 的完整 `compact_boundary` 记录，以实际 UUID 去重，自动/手动压缩均计数。SessionStart 和下次工具调用补查异步延迟写入的边界；不会按 hook 次数、summary 哈希或 prompt_id 计数。JSONL 的未完成末行留到下次处理，inode 变化或截断后重扫并保留 UUID 去重。边界 schema 变化会报告错误，不悄悄猜测次数。

状态锁保护计数、请求和 epoch 更新；一次 epoch 只产生一个确定的轮换请求。仅 `owner` 对应的会话计数。worker hook 不改轮换状态。阈值保存在私有状态的 `threshold` 字段，初始化时可用 `adopt --threshold N` 配置，后续协调者继承该值。活动协调者也可运行 `python3 -I .claude/hooks/coordinator_rotation.py configure --session ACTUAL_SESSION_ID --threshold N`，下次 hook 核对时生效；交接已触发后不能提高阈值逃过交接。不要直接改正在使用的状态文件。

## 首次启用

先读项目共用协调契约，确认原协调者已实际停 loop 并退任。现有用户对接管的授权可以直接执行，不再要求用户重复确认。保留旧协调者交接回执，不以终端输入 accepted 代替退任证据。

在新协调者自己的终端调用：

```sh
orca orchestration run-use --id ACTUAL_RUN --json
orca orchestration run-current --full --json
python3 -I .claude/hooks/coordinator_rotation.py adopt --session ACTUAL_SESSION_ID --run ACTUAL_RUN --transcript /absolute/path/to/session.jsonl --threshold 2 --goal '当前用户目标与边界' --loop-prompt '已授权的原 loop 提示词' --retired-session OLD_SESSION_ID
```

普通 Run 使用不带 `--takeover-legacy` 的 `run-use`；该额外参数仅用于自动迁移的 legacy Run，本项目当前 Run 已实测会拒绝误用。`adopt` 会核对调用者的实际 Run 绑定，并把当前会话已记录的压缩计入，不从零掩盖已有压缩。不得为接管另建 Run。运行中的本机身份和 token 只放 `.voiceport`，不提交到 Git。

## 达到阈值后的交接

达到阈值后，PreToolUse 拒绝协调者继续编辑、派发、消费收件箱或续订循环，允许只读 Read、Skill、停止 loop 和以下单条 helper 命令。已在执行的工具先自然返回；hook 不中断 worker 或修改冻结内容。Stop hook 最多在非递归停止时提示一次交接，不能靠无界阻止结束来维持循环。

1. 调用 `ScheduleWakeup`，参数 `{"stop":true}`，取得真实成功回执。不要安装新 wakeup。
2. 运行 `python3 -I .claude/hooks/coordinator_rotation.py retire --session ACTUAL_SESSION_ID --transcript /absolute/path/to/session.jsonl`。工具检查最后一次 loop 操作确实成功停止，原子写入最多 48KB 的私有交接包，然后登记退任。
3. 运行 `python3 -I .claude/hooks/coordinator_rotation.py launch --session ACTUAL_SESSION_ID`。它通过 Orca 在同一 checkout 启动一个新会话，传递实际交接路径、原 Run、一次性 token 和接管步骤。取得 launch 回执后结束本协调者的 turn；不要监督新协调者或继续调度。
4. 新会话读取交接包，加载 orchestration 技能，从自己的终端接管既有 Run，再执行 prompt 给出的 `claim --session ACTUAL_NEW_SESSION_ID --token TOKEN`。claim 核对真实 Run、交接包哈希和未消费 token，在锁内递增 epoch、清零新会话计数。旧会话仍被 fence，即便重新打开也不能恢复协调写入。
5. 新会话确认领取成功后再处理收件箱、核对 worker 和继续原任务。暂停状态跨交接保留；若没有暂停且仍有可推进任务，显式用交接包的 `loop_prompt` 启动 `/loop`。原会话定时器不会自动转移。等待 worker 以 Orca 完成事件为主，兜底至少 1200 秒。

轮换需要协调模型执行上述固定交接步骤；hook 本身不在 PostCompact 中直接杀进程或伪造 ScheduleWakeup 回执。若模型停止响应，系统保持请求/退任状态，不能保证无人值守自愈。工具 hook 是协调约束，不是阻止任意 shell 或恶意会话绕过的安全沙箱；非 owner 的普通 worker 仍按原任务允许范围工作。

## 每轮 Git 交付跨交接保留

用户已明确要求把分逻辑 commit、push、PR 创建/更新与合并，以及拉取最新 main 纳入持续 loop。当前协调者按 `docs/LOOP_METHODS.md` 第 6 节和 `.claude/loop.md` 执行；交接包包含 `git_delivery_policy`，继任继续未完成的 Git 步骤。每个有实际代码/测试/文档产出的阶段及时提交推送，满足仓库合并条件后合并，在无活动 writer/冻结验证冲突的安全点同步 main，并从最新 main 建下一功能分支。公开保留失败和 WIP 边界，不将提交或合并当作 VP 产品验收。

## 中断和恢复

```sh
python3 -I .claude/hooks/coordinator_rotation.py status
python3 -I .claude/hooks/test_coordinator_rotation.py
```

- `requested`：原协调者尚未完成退任；只恢复停 loop 和 retire，不重复派发。
- `retired`：原 loop 已停止，交接包已落盘；可执行一次 launch。
- `launching`：启动已尝试，即使超时也禁止 fresh launch。查看私有 `request.launch_receipt` / `launch_error` 和 Orca 终端。存在 operationId 时，按 Orca 文档用同一命令加 `--retry-launch` 重放；没有可验证回执时先检查实际会话，不凭静默创建第二个。
- 新会话已启动但尚未 claim：恢复该会话，让它绑定原 Run 并使用原 token。不能让旧会话重获写权。
- claim 失败：不写产品账本，不开 loop。保留证据、核对宿主所有权和 packet 哈希；不自动清空锁或新建 Run。
- 存在未知旧 session、失联或权限菜单：保持停止状态，按宿主真实回执处理。不能将超时当作退出。

没有状态文件时 hook 不自动认领任何会话。首次 adopt 必须由实际协调者执行。现有 `.claude` 目录启动时若尚无 settings 文件，当前会话可能需要用户打开 `/hooks` 或重启才加载新增 hooks；新启动的接任会话会加载项目配置。未取得真实触发证据之前只称配置已安装。

## 来源与取舍

2026-11-05 只读调研，未运行上游下载代码：

- [Anthropic hooks：PostCompact](https://code.claude.com/docs/en/hooks#postcompact)、[SessionStart](https://code.claude.com/docs/en/hooks#sessionstart)、[Stop](https://code.claude.com/docs/en/hooks#stop)。PostCompact 没有决策控制；用它观察完成，用 SessionStart/PreToolUse 补查和执行约束。官方 [CHANGELOG 固定版本](https://github.com/anthropics/claude-code/blob/26382c08758260c9f5deb2019d8fe851f89fa9ae/CHANGELOG.md) 记录生命周期 hook 支持。
- [planning-with-files 恢复逻辑](https://github.com/OthmanAdi/planning-with-files/blob/8eef6dc6540dde5eef3fdc0ac487ad9c48ef3ba5/hooks/claude-hook.sh#L304-L345)：从磁盘恢复精简状态；不把全部旧对话复制给下一任。
- [HumanLayer handoff](https://github.com/humanlayer/humanlayer/blob/f9bd73534a9fb7099a274f951b698b1297122fd9/apps/riptide-api/src/orpc/api/sessions/handoff.ts#L40-L114)：在安全状态交接、用事务绑定继任；本项目额外保留旧会话退任和 epoch。
- [Ralph 顺序新会话](https://github.com/snarktank/ralph/blob/6c53cb0b831ebe8739c6a003e22af14902d8b0b5/ralph.sh#L84-L108)：跨新进程用持久进度衔接；不复制其权限绕过、文本完成哨兵或忽略失败行为。

## 验证范围

单元测试在临时项目中模拟 Orca，覆盖重复/延迟/截断边界、worker 隔离、阈值后拦截、真实停止回执检查、失败启动锁存、token 单次领取、Run 绑定失败、交接包篡改、暂停/worker/attempt 保留以及快速继任与 launch 回执竞争。模拟测试不代表已完成真实压缩后的跨窗口轮换；真实人工接管和真实 hook 触发的结果另记在本机交接记录。
