# 验证记录

更新：2026-11-04。本文件记录实际执行范围；任务执行状态以本机 .voiceport/loop-state.json 为准。

## 当前验证状态

环境：macOS 26.2（25C56）、Apple Silicon、Swift 6.3.2、Command Line Tools。

**VP-006 freeze-01 本地离线独立验收通过，账本已完成 VP-006。** 原验证会话完整审阅录音存储与生命周期，自建并运行 20 项独立检查，亲自运行 31 核心/60 应用/4 状态回归及完整 release/CLI/bundle/签名/启动，共 12 个命令符合预期。独立报告 `.voiceport/evidence/vp006-independent/report.json`，完整回归 `.voiceport/evidence/20261104-191611-5b1818/report.json`。45 个冻结输入和 98 个历史证据文件前后无漂移；协调者另核对真实回执、attempt/Task/Dispatch、完整字节与日志后 finish，核对证据 `.voiceport/evidence/vp006-coordinator-acceptance.json`。验收绑定 snapshot；本次验收后的 README/SOLUTION/本文件仅更新状态，不把新文档字节冒充原冻结。

独立检查使用真实 Darwin 文件系统和子进程，包含持锁、SIGKILL 后 OS 释放锁、3 个并发清理器合计只删除 24 个会话一次、标记/模式/链接/时间/边界/失败/路径替换。设备、权限、网络和投递仍是受控替身，未操作用户默认缓存根。开发的 60 项自检证据保留在 `.voiceport/evidence/vp006-developer/`，不与本次独立结果混同。

**剩余状态：VP-007 blocked，VP-008 等待依赖与样本。** 真实录音、权限、登录有效性、云端返回和 Orca/其他应用实际接收仍未验证。解除条件与本机执行步骤见下文；当前只剩需要用户参与的外部条件，loop 停止等待新事件。

此前 **VP-005 freeze-02 本地离线范围** 已独立通过。原验证会话亲自运行 31 项核心、44 项应用、4 项状态回归，以及 20 项独立反例，均通过；43 个完整冻结输入前后一致，VP005-I-001 已确认修复。独立报告 `.voiceport/evidence/vp005-independent-02/report.json`；完整回归 `.voiceport/evidence/20261104-181322-4ddc5a/report.json`。协调者核对当前 attempt/实际 Task/Dispatch 与文件摘要后完成 VP-005；该结论不覆盖当前 VP-006 新增输入。

VP-005 freeze-01 独立验收已执行，结论为退回。完整 verifier --app-smoke 退出 0（31 核心 + 35 应用 + 4 状态），但独立默认 AX 采集反例 11 项中 3 项失败，确认同一缺陷 VP005-I-001：只按文本角色判定可编辑，导致只读/禁用控件仍调用粘贴边界。证据 `.voiceport/evidence/vp005-independent/report.json`；完整回归报告 `.voiceport/evidence/20261104-174143-8ab9f0/report.json`。原冻结 41 项前后无漂移，不能以全套回归通过代替产品验收。

返修开发已交付：默认采集增加已启用与 AXValue 写入能力判据，新增 9 项默认入口检查。`bash scripts/test_app.sh` 44 项、0 失败、退出码 0；复制到新目录的原 11 项反例修复前失败 3 项、修复后 0 失败；无测试替身的生产 API typecheck 退出 0。证据 `.voiceport/evidence/vp005-revision-01/report.json`。这些属于原开发自检，原独立验证会话随后对 freeze-02 亲自执行完整回归与 20 项独立反例，均通过；结论绑定新冻结，未沿用旧报告。

早期开发的 35 项自检保留在 `.voiceport/evidence/vp005-developer/report.json`，主协调者曾核对 14 个文件摘要。下表区分当前 VP-006 独立结果和历史结果。除明确标为历史的行，完整验证报告指 `.voiceport/evidence/20261104-191611-5b1818/report.json`；真实场景继续单列未验证。

| 检查 | 结果 | 证据 |
|---|---|---|
| 核心离线契约 | 31 项通过，0 失败 | bash scripts/test_core.sh |
| 实际控制器/投递者 + 受控系统边界 | VP-005 freeze-02 独立执行 44 项通过；另 20 项独立反例通过 | .voiceport/evidence/vp005-independent-02/report.json |
| VP-006 应用及存储独立验收 | 亲自运行 60 项应用检查、另 20 项独立检查；均为 0 失败 | .voiceport/evidence/vp006-independent/report.json |
| VP-006 早期开发自检 | 60 项应用检查及原生 target 构建通过；自检历史保留 | .voiceport/evidence/vp006-developer/report.json |
| freeze-01 独立反例 | 11 项中 3 项失败，已退回修复；历史结论不覆盖返修版本 | .voiceport/evidence/vp005-independent/report.json |
| loop 状态回归 | 4 项通过 | python3 -I -m unittest discover -s Tests/LoopTests -v |
| Release 构建 | 通过 | swift build -c release |
| CLI help / demo / doctor | 输出断言通过 | 完整验证报告 |
| CLI 错误参数 | 预期退出 1，通过 | 完整验证报告 |
| .app 打包、plist 和 ad-hoc 签名 | 通过 | 完整验证报告 |
| 菜单栏启动 smoke | 通过；未录音/未访问网络 | 完整验证报告 |
| 实际结果/设置窗口 | 已打开并读取 Accessibility 树、查看截图，界面正常 | 本机 GUI 证据 |
| 自动粘贴设置交互 | 观察到 1 → 0 → 1，已恢复原值 | 本机 GUI 证据 |
| 原生控件可访问名称 | 5 个控件的 AXDescription 与 AXIdentifier 直接读回正确 | .voiceport/evidence/gui-vp004/accessibility.json |

上轮完整自检报告：`.voiceport/evidence/20261104-165628-e079ca/report.json`，属于 VP-004。报告绑定源代码摘要，保留每个命令和退出码；共 31 核心 + 12 控制器 + 4 状态检查通过。首轮报告保留在 `.voiceport/evidence/20261104-164319-006e41/report.json`。

多会话 loop 契约切片已交付 7 份文档，13 个相对链接及内容检查通过；证据 `.voiceport/evidence/loop-contract/`。该报告只覆盖协议会话的文档切片，最终集成冻结另含源码、测试、构建入口、README 和方案文档；`verify.py` 自带摘要不覆盖 docs 或 README，不能单独代表完整冻结。

GUI 证据保留在 `.voiceport/evidence/gui/` 和 `.voiceport/evidence/gui-vp004/`，不提交私有现场数据。第二轮重启新构建，查看实际窗口，并直接读取本应用的 AXDescription / AXIdentifier；未点击麦克风或系统授权按钮。

CLI doctor 只确认登录文件存在，未读取其内容、验证有效性或调用账号。

## 待完成的系统层验证

| 场景 | 当前状态 | 验收方法 |
|---|---|---|
| 录音开始/停止/取消与麦克风拒绝 | 控制器离线验证通过；真实麦克风待交互验证 | 用户明确开始录音；观察状态、权限和文件清理 |
| ChatGPT 真实转写 | 待账号和真实录音 | 上传用户选择的录音；检查非空文字、错误和延迟 |
| Orca 输入框接收 | 待真实转写 | 只填文字，确认未发送；切焦点/窗口时应保留结果 |
| 其他编辑器/浏览器输入框 | 待验证 | 同一流程，分别记录实际兼容结果 |
| 剪贴板改动/恢复与退出 | 独立命名剪贴板及真实控制器 shutdown 的独立检查通过；真实应用消费事件与活跃进程退出实测待验 | 不访问一般剪贴板；记录拥有权、新复制、同步退出和迟到完成反例 |
| 取消后的迟到请求 | 上传前取消、控制器上传中取消/替换/退出后迟到结果均已离线验证；真实网络取消待验 | 可控 transport 延迟和会话替换反例 |
| 错误录音的重试 | 控制器已验证同一音频重试与关闭自动投递；真实账号恢复待验 | 失败后手动重试只保留结果，不自动投递到新焦点 |
| 崩溃残留音频 | VP-006 已独立离线验收；真实编码器和断电未验 | 已以自建隔离目录验证归属、严格 >24h、活动锁、进程退出、并发清理、链接和未知内容保留 |
| 与 Codex App 同音频质量对照 | 待用户选样本 | 5–10 段，核对技术 token、否定词、数字、延迟 |
| 用量/额度归属 | 未知 | 仅凭接口地址或免 API key 不能下结论 |

## 真实测试的解除条件与步骤

VP-007 当前 blocked 的原因是缺少用户选定的真实测试输入和交互事件，不是已发现登录失效、权限拒绝或 Orca 不兼容。私有阻塞记录 `.voiceport/evidence/vp007-external-blocker.json` 保留当前 attempt 和恢复入口。VP-008 在账本为 ready，但依赖 VP-007 完成，还需要同音频对照样本，因此当前不可启动。只剩这些条件时停止 loop；收到明确开始动作或选定样本后，协调者先核对状态，再 reopen/start 新 attempt，不重用已结束的阻塞 attempt。

1. 用户在本机运行 `open dist/VoicePort.app --args --show-window`。当前 dist 包已通过 VP-006 release/签名/smoke 验证。普通启动会按已记录规则尝试清理确认过期的本应用录音残留。
2. 登录由用户在本机的 Codex 管理；需要时运行 `codex login`。不要把 auth.json、token 或账号原始响应放进聊天和仓库。已有 doctor 结果只证明文件存在。
3. 用户点好目标输入框，再用 Control + Option + Space 明确开始一段自己希望上传的短录音，按同一快捷键结束；按系统提示处理麦克风权限。自动粘贴需要用户在本机授予辅助功能权限。结果应显示原文，目标应收到文字且未自动发送；无法确认 AX 写能力时应保留结果供用户手动复制，记录为回退而非自动粘贴成功。
4. 记录实际目标应用/版本、输入控件、授权结果、停止到出字延迟、云端成功或脱敏错误、目标实际收到的文字与未发送状态。成功后本次音频应尝试清理。逐项验证取消、手动重试、切应用/焦点和复制新内容；涉及一般剪贴板/真实目标的交互由用户明确参与，不由自动离线 QA 代做。
5. 真实结果只记录脱敏结论；原音频和参考文字由用户本机保存，不提交。每项标通过/失败/阻塞，失败带具体触发条件。先完成 VP-007 的必要场景，再决定是否启动独立效果评估。
6. VP-008 使用 5–10 段用户认可的同一音频，先确认 VoicePort CLI 和 Codex App 都能使用该音频。无法做到相同输入时保持对照未验，不用两次口述替代。记录技术 token、数字/否定词、语义和延迟；模型及用量归属没有直接证据时仍为未知。

## 已发现并处理

- Command Line Tools 缺少 XCTest / Swift Testing：改用独立 swiftc 检查，产品无需完整 Xcode。
- Swift @main 入口命名和 AX 旧模块并发桥接：已修复并完成构建。
- macOS 大小写不敏感磁盘覆盖了 CLI：`voiceport` / `VoicePort` 同 inode；CLI 改为 `voiceport-cli`，实际命令输出已验证。
- 重命名后 CLI 用法空格丢失：输出断言捕获并修正。
- 菜单栏 accessory 应用未出现在 GUI 工具普通应用列表中：通过实际 PID 读取自身窗口；不代表应用未运行。
- 服务自行 CancellationError 会令旧实现停在 transcribing：12 项控制器检查中反例实际失败；修复后 12 项全过。旧行为失败证据在 .voiceport/evidence/vp004-before-fix/。
- GUI 工具简化树省略部分 SwiftUI 按钮/开关名称；第二轮补显式标签和稳定 ID，直接读取 macOS 原始 AXDescription 确认 5 个控件正确。简化树留空不能证明系统中没有名称。
- 测试路径 tests / Tests 在 macOS 指向同目录，已统一为 Tests 下 Core/App/Loop 子目录，避免重复遍历与大小写歧义。

## 同音频对照模板

原音频与参考文字只留本机；公开记录仅使用用户认可的脱敏摘要。

| 样本 ID | 场景 | 关键技术 token 正确数/总数 | 语义错误 | 停止录音至出字耗时 | Orca 是否接收 | 对照说明 |
|---|---|---|---|---|---|---|
| 待用户选定 | 中文 + 英文技术名词 | 待测 | 待测 | 待测 | 待测 | 使用同一音频，不用不同口述比较 |
