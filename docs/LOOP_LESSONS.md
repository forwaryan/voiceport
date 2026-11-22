# Loop 已证实的反例

只记录真实返修、错误验收或验证遗漏。每条依次写规则、原因、可复核证据，并注明独立观察日期。同一次问题在多份文档或日志中出现只算一次；同类问题在三个不同日期出现后，考虑提炼为项目规范。保留首次失败及后续修复范围，公开记录只链接可公开的源码、测试、文档或 PR，不附私有日志、用户内容或凭据。不新增定时任务。格式参考 [Magpie 的复盘记录](https://github.com/yetone/magpie/blob/235f50a7a83269ff3411265fbdc4db4af161d812/LESSONS.md#L2-L9)。

## 投递路径名不能当作接收回执（2026-11-23）

- **规则：** 投递路径或回执变更时，同一 PR 核对代码、行为文档、操作指南和离线 Skill 的描述。分别记录选路、事件提交和目标控件接收，不用前一环替后一环。
- **原因：** 微信兼容路径改用 `clipboard_final` 后，仓库 CLI 指南和离线排查流程仍把 `wechat_menu_paste` 当成当前路径。选路时就会写入 `input.method`，之后仍可能取消或失败。独立复核还发现，面板和 CLI 听写跳过[自动捕获](../crates/voiceport-macos/src/controller.rs)，所以 `status` 可能沿用前一轮投递的路径和失败阶段。
- **证据：** 执行基线 `91bc8d146cdee8a5b70200da89094dd081e0524b` 中，`git grep -n wechat_menu_paste 91bc8d1 -- docs/CLI.md skills/voiceport/references/` 同时显示两处过时说明和一处已正确标旧版的说明。[选路实现](../crates/voiceport-macos/src/delivery.rs)的 `Delivery::accept_capture` 在实际提交前设置 `clipboard_final`；[回执回归](../crates/voiceport-macos/tests/delivery_receipt.rs)的 `clipboard_submission_is_not_confirmation_and_uncertainty_cannot_retry` 区分提交与确认。历史[合成目标记录](INPUT_DELIVERY.md#验证范围)证明专用控件能收到文字，不证明真实微信聊天框接收。本次只纠正文档，不改变投递代码。

独立观察：1 次（2026-11-23）。
