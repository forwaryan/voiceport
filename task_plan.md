# VoicePort 实施计划

## 2026-11-09 当前组合：CLI/Skill 交付、终止确认返修与规则集成

本节覆盖后文历史快照，全部旧失败、包和冻结保留。P1 来源身份、CI07 oracle 及两处夹具接线已在 freeze06 独立接受；现场评审已关闭。9bf5514 的远端 CI42512271396 仍失败，原生状态证据为组内成员 state=2，不能用本地绿色或早期 EPERM 解释结算。CLI 与使用文档已保存本地 checkpoint febe25d，三文件规则已保存250ef14；尚未推送，待 CI09 组合修复及新冻结核对后一起交付，避免对已知未修后端重复触发同类 CI。

- CLI/Skill 原任务已正式 failed/BLOCKED：实现和开发检查交付，必需 GUI 证据受 Accessibility permission_denied 阻塞。21 步最终自检、232 次 Rust 测试通过、4 LoopTests、18 PackagingTests 与 73 次实际合成 CLI 调用已核对原始日志对应；整体集成仍保留 passed:false/exit1。协调核对5514封存证据、1096产品输入/377目录模式、34变更、源码与签名 App 归档、5产物；所有11个记录测试PID和私有socket均已无。未授予进一步 AX、系统权限、真实音频或账号操作。
- 主工作区已按相同基线完整集成该34文件交付，并修正 README/SOLUTION 的旧 CLI 限制说明；这是待评审候选，未把工作任务完成当产品接受。GUI/CLI 使用 voiceport-app 与 voiceport 两个真实文件；Skill 在根 skills/voiceport 并随 CLI/App 分发。
- CI09 的四文件 typed-owner/recording-lease 返修仍在隔离工作区。开发报告16 process、17 controller、library83/1ignored、六 Stop 与严格 all-target Clippy 通过，重型时段已归还；原生 ABI 负向控制和最终封存尚待交付，协调者未读取或合并未完成实现。
- Magpie 三文件规则提案已核对61封存文件、目标 before/after 及补丁后集成，补齐当前动态调度、skills 清单和实际继任记录。精确 HEAD/测试 merge/run/attempt、单写者、独立复核、完整失败、一次有证据的基础设施重试和重型资源时段成为候选规则；未改 helper，不声称原生 cron stop-proof 或可执行 CI gate 已实现。
- 下一步收齐 CI09 封存，合并重叠 Controller 并验证 Stopping、录音持有、CLI busy/quit/cancel；对最终代码、文档和规则建立新完整冻结，派不同真实会话独立复核，再按持续授权提交/推送、检查精确新 HEAD CI、满足门槛后正常合并和同步 main。PR15 保持 draft/阻塞；无产品 PASS。

## 2026-11-09 当前接续：来源身份返修、CI 复核与原 CLI 恢复

以下状态覆盖后文旧快照，旧失败和报告保留。PR16 已独立接受、正常合并且 main CI 成功；本地 main 已 ff-only 同步，功能分支已合入最新 main。

- PR15 全差异评审完成并退回重复菜单标题导致来源身份错配；旧冻结已显式关闭。已交付稳定 represented ID 修复及两处合并后测试夹具接线修正，原生菜单反例、library65/1ignored、storage12、delivery23、六项 Stop、fmt/严格 all-target clippy 均为开发自检。需要最终组合输入独立复核。
- CI 测试 oracle 已用真实 owned zombie 控制证明 ESRCH-only 组检查过强；首个候选独立退回了 ESRCH PID 再出现时缺状态证据的问题，当前 CI07 窄修正对最终成员要求已验证 SZOMB 状态。11 scripted/5 native 通过仅为开发自检；新冻结独立复核及精确新 HEAD CI 待执行。历史远端组成员和单独 storage retained0 根因仍未证明。
- 原 CLI/Skill Task、Dispatch、session、terminal 和独立工作区保持。经独立有限契约接受的六文件 helper 已在真实协作静止窗口安装，原备份/模式/完成标记已核对；真实 retained-session PreToolUse、当前 helper、实际 terminal 和 transcript 零计数已独立核对，解除其基础设施实现暂停。仍须实际交付 CLI/IPC/可分发 Skill 及独立验收，不把加载证明当产品完成。
- 协调者继任已实际绑定原 Run、claim、确认投递并收到精确旧终端 ptyKilled 回执，阈值仍为初始加最多四次压缩；当前动态 loop 事件优先、20 分钟兜底，无原计时器自动迁移声明。原生/structured-chat/PreCompact 全生命周期和旧 reviewer 事件/边界计数不一致仍保留未验范围。
- 下一步提交组合版本、推送并记录精确 HEAD CI，建立最终冻结和真实独立复核；原 CLI 在隔离工作区推进，重编译资源串行。Magpie 私有三文件规则候选尚未采用，不能称已部署。PR15 仍 draft，不合并未验版本，不宣布产品 PASS。

## 2026-11-08 当前推进：插件、CLI/Skill 与自动轮换

用户新增 CLI 控制 App 和配套可分发 Skill，继续原插件目标；同时要求协调者自动完成正常会话轮换并写入 loop 规则。历史会话只读，不恢复或修改。当前已绑定原插件 Run，未创建重复 Run，旧 VP013 lease 不接管。

- R1（complete）：新协调会话真实 PreToolUse、terminal、transcript 初始零计数已核对；当前两次自然压缩，真实 PostCompact/SessionStart 已观察。不是已完成自动轮换的证明。
- R2（complete，评审收尾）：freeze02 最终检查 matches:true；独立02正式 failed/BLOCKED、exited/released。现场评审已显式关闭，历史快照和报告保持。HTTP GUI 来源新增/编辑/选择/重启/合成样例/删除及已知 Keychain 修订清理有实测；GUI 导入确认归属与后续外部来源/错误/空结果/取消路径未完成。
- R3（R1独立FIXED，部署D1-stage返修pending）：freeze04独立msg_00835240f08a完整评审，78helper/20独立R1控制通过，安装信封修复R1与原D1父路径逃逸FIXED；新proposal f984caf7…在os.replace边界会先安装被替换的staging symlink再拒绝，故部署整体RETURNED。39观察控制不抵消目标安全断言exit1。协调核对201hash/模式/字节及真实initial0/final0，最终freeze matches:true后显式关闭live评审，历史报告/快照保持。沿原已exited/released开发角色派仅私有新proposal返修，不重复改已通过helper；未真实register、CLI部署或自动生命周期。
- R4（in_progress，保留已接受离线范围，修复新远端失败）：HTTP夹具经freeze03独立接受，节流9项/禁用guard负向101保留；checkpoint81c9341的17步独立通过与CI42499019278 success仅对应原提交。新68e2e6b的CI42502439010失败plugin_process进程回收errno1vs3（64pass/1fail/1ignored）；独立一次本地exact及12owned process观测未复现，原因未定。隔离CI开发task_81587872fb63/ctx_a9bb43af05f5正式failed/unresolved：本地65owned组与required workspace/fmt/clippy通过，未复现根因；已核对75封存项并整合strict errno即时捕获/分支PID归属诊断补丁，仍要求-1/ESRCH。下一步新冻结独立审查与新精确HEAD诊断CI补证，禁止原样盲重跑或声称已修复；原stale binary/历史失败保持，GUI/CLI/真实ASR不在离线接受范围。
- R5（blocked，原任务保留）：CLI/Skill 开发位于独立工作区，尚未开始产品实现。helper 在无全局 lease 时提前返回，导致真实启动证明缺失。待经复核 helper 部署后补实际 hook 证明，沿原任务继续；不手造状态或改信任。
- R6（pending）：集成 CLI/Skill 与修复，完整新冻结、不同真实会话独立验收、全量离线检查及新远端 CI；按持续授权完成正常 Git/PR/main 同步，再交付已验证 App。PR15 仍 draft，无产品 PASS。
- R7（in_progress，用户新增CI loop/多agent方法改进）：规则task_a94ba83876a4/ctx_7dd1da129b80只写私有candidate，核对Magpie实际固定commit机制并完善精确HEAD远端CI修复、证据/停止条件、单writer隔离、逻辑任务多agent子分工及独立验证；整合前审查base/链接/与现有接口一致。CronCreate f0ad2ff1每20分钟兜底已启用（session-only/7天），其退休stop-proof与helper现有ScheduleWakeup语义的兼容缺口须明确修正/约束，不能称真实自动轮换通过。

本轮工具差错：仓库根 CLAUDE.md 不存在，已转读现有角色/loop 契约；未创建替代规则。保留所有旧包，不自动录音、读真实凭据、上传真实音频或向真实目标输入。

## 2026-11-08 前阶段记录：插件式转写来源与真实验证

用户在自定义来源方案后明确要求实际实现并测试，随后要求类似 harness 的插件机制：一个插件代表一种接入方式，并 loop 持续完成。每次听写只使用一个来源；同一插件可配置多个独立来源。不要求安装 Orca，不扩展为多服务对比或词典。

- P1（complete）：新Run run_a9b3754f7871已注册；插件/来源实例/能力/凭据边界和自包含交接已完成。当前协调者实际 session637eb70f、terminal term_dbb1ea5d、真实 hook helper014eeb1e及2次已完成压缩已核对；旧VP013 requested lease/attempt保留不接管。新目标产品关联unbound，基线main d3af863，分支feat/transcription-plugins，保留三项方案文档修改。
- P2（complete，开发自检）：真实开发sessiondf6fa345/task_38af9f83182c/ctx_3f257aa3b76f正式worker_done msg_dce1e74219cf并停写。20项源码/文档/许可输入、61个macOS测试通过与1个原有真实模型测试忽略、完整17步通过；协调核对20项实际/快照字节、34份原始日志、3个产物与产品摘要1793d204…，未重跑开发测试。真实PreToolUse/helper/精确terminal/首次和最终0已核对，SessionStart未观察。只有开发自检，GUI及原生Keychain未验。
- P3（in_progress，首轮BLOCKED后的返修）：checkpoint0277551已push，PR15 draft；完整freeze01由真实独立session5da9b40b审查，62个lib测试通过/1原有测试忽略、原生Keychain探针通过，但隔离HOME下GUI保存合成key进入系统授权等待，GUI评审未完成。真实failed/BLOCKED回执msg_bfb3a15a65aa，不能算PASS或完整RETURNED。测试PID45269经正常退出请求后ps确认已无；未操作系统认证、未猜删未知条目。CI42483256463为cancelled：两个取消/超时测试FAILED，响应边界测试挂起至取消，根因待诊断。两原worker均实际exited/released，freeze01当前matches true后显式关闭live评审，原快照/报告不改。新changed-spec开发successor session555e44ba/task_03ef6325c033/ctx_a3aa8344a7e6已msg_c86b86cba37c正式交付6项窄改并停写：受控服务有界I/O和异步收尾、取消/实际spawn PID同步、VOICEPORT_SOURCES_PATH。修前背压/提前取消挂起及Python PID竞争已复现，4次23项串并行与完整17步通过（64 macOS passed/1原有ignored），仅自检；协调首次核对6源码/快照、160封存文件、34完整入口日志和3产物hash/模式/字节、前后当前f8bc5108…一致，未重跑委派测试。先提交推送修复并触发新远端CI，再新完整冻结、真实独立GUI和反例验收。协调实际已3次完成压缩，未达最后一轮；旧生产App保持。
- P4（pending）：验收后按持续授权正常commit/push/PR/CI/merge与main同步，交付验证过的新包并停止loop。不得用方案文档、编译或固定文本测试替代实际已实现能力/真实识别效果；未测真实录音及云端列明。

边界：不自动录音、不读取真实账号/无关输入、不上传真实音频、不操作真实输入目标；合成音频与受控本机服务可用于验证。保留当前运行App和所有旧构建/快照/报告，不修改Orca模型、偏好或系统信任。

## 2026-11-08 当前任务：复用 Orca 本地语音模型与来源选择

用户明确要求复用 Orca 已下载的本地语音模型，在原生设置中下拉选择单一转写来源；Paraformer 提供实时预览及本地最终结果，Whisper Tiny 停录后本地识别，保留 ChatGPT 选项，不做多源对比或词典。

- L1（complete）：已只读发现 Orca 的 paraformer-bilingual-zh-en、whisper-tiny 和 sherpa-onnx 1.12.37 原生运行库，核对实时/离线能力。当前会话真实 hook、session/terminal 及一次压缩已核对；为新用户目标创建独立 Run run_6f1edc8ca1f3，不接管旧 VP013 attempt/lease 或改写历史。新任务产品关联为 unbound。
- L2（complete，开发交付）：真实 Orca task_c891fc8528e0/ctx_d32d69cd0690完成来源选择、只读发现、C ABI本地推理、PCM/异步生命周期与测试并停写。开发完整17项自检与公开夹具实际推理通过，产品SHA5e501840…起止一致，报告`.voiceport/evidence/local-models-development-20261108/report.md`；不是独立验收。已保留8个继承UI dirty文件与图标窄补丁。图标会话随后收到熊+语音设计新要求，仅在独立候选中修订，待交付后由协调者集成再冻结。
- L3 首轮审查（历史退回）：1078文件freeze01经真实独立session9c6eb66b/ctx_38dc2665e9da审查，整合版17步、公开夹具、额外运行时/控件检查及隔离GUI来源/重启记忆通过；确认LM-01 P2麦克风权限文案无条件声称ChatGPT上传。独立报告`.voiceport/evidence/local-models-independent-20261108/report.md`，产品尚未接受。旧freeze01显式关闭返修且快照/报告不变；原开发恢复发生terminal drift并经guarded stop/退出/释放后，真正新session6fa25592在同一Task完成仅Info.plist修正及17个packaging自检。原验证终端已实际exited/released，不能在新terminal恢复而违反身份保护；由新真实独立会话接续freeze02的聚焦复核并重建最终包，保留原审查链。熊+话筒模块SHA a1ada958…已集成，A为peer建议。
- L3 最终结果（complete）：freeze02（完整e5ff82d5…/产品3a64a677…）由新真实独立session111f5cbd/ctx_d168251845c2聚焦复核PASS，LM-01 FIXED。原17步/实际本地推理/隔离GUI证据及实现逐文件不变已核对；新包从最终源码正常release/package，17个packaging/严格签名/源产物对应/全部17个Mach-O sections通过。旧失败及报告不变，未做真实麦克风/中文质量/目标接收验收。
- L4（交付执行记录）：用户随后明确要求替换、合并main并在本机启动最新版。最终包`dist/local-models-20261108-final-02/VoicePort.app`已独立验收；源码冻结评审结束后仅补行政记录再正常commit/push/PR/CI/merge与本地main同步，实际Git回执和启动结果持续保存`.voiceport/handoffs/local-models-coordination-20261108.json`并向用户交付。图标A已由另一会话转来用户后续明确选择「就左边第一个吧」，与已验收模块一致。可正常退出经核对路径且空闲的旧实例，但不自动录音、上传、读真实账号或操作真实目标输入，不覆盖历史包或修改Orca模型与偏好。


## 2026-11-08 当前任务：按GitHub原生UI skill重做紧凑界面

用户认为0.3.2界面一般，要求在GitHub寻找UI skill并实际优化。已检索UI UX Pro Max与ceorkm/macos-design-skill，选择后者原生工具布局参考，固定commit 8f528a2364f996cd42f02a10b1b27198a74ca2a3的SKILL及三篇引用已读取，原文仅保存在.voiceport/research/macos-ui-20261108，未安装远端脚本。

- V1（complete）：完成skill调研与当前布局评估；方向为紧凑窗口、听写/设置页签、简短可操作权限提示、按状态展示次要动作、系统语义配色与SF Symbols。
- V2（complete）：0.3.3/build9紧凑620×520内容窗口实现，听写/设置原生页签、条件动作、短提示与完整详情sheet、NSSwitch与语言/快捷键下拉；实际发现100ms重复selectItemAtIndex会打断菜单，已改为值变化才更新，图标亦缓存。右上角常驻/无Dock与录音投递边界保留。
- V3（complete）：完整离线17步通过（verify-2482a31db1ec48638525b7b628cfc9f9），起止SHA e466950c745d76cbb2ce8de0f1a5e561f39298dda7b215c8bcfbf10dc7619fab一致。新包dist/native-ui-20261108-9eaa1fc4-final/VoicePort.app签名/Cargo对应通过，PID82829实测中/英语言、替代快捷键、开关与保存值，恢复原偏好；页签/详情sheet0秒/窗口关闭同PID菜单重开、顶部(973,4)/Dock空均通过，实际读两页及菜单截图。当前AX仍未授权，未自动录音或重绑。仅正常退出本轮新建且已核对0秒的预览PID66378，未向原应用发送退出。report.json同目录，未提交/推送。

本轮GUI错误记录：后台popup点击background_unavailable、直接set-value为value_not_settable；前台辅助动作有未获验证的非零回执，均不算成功。跨工具菜单引用失效和同脚本连续菜单未ready出现-1719，改为每个原生菜单前台单次打开、有界等待目标item再点，并用应用UI与defaults读回独立核对。get-app-state不支持--full-tree，按有效参数纠正；首轮大段Edit旧片段font误写导致不匹配无修改，受控替换后fmt/严格Clippy通过。

调研错误：首次gh tree路径未加引号触发zsh no matches found，改为整体单引号URL后读取成功，未运行第三方安装器。

## 2026-11-08 当前任务：同步主分支并提交合并本地修改

用户明确授权获取远程 main、更新本地、提交全部当前源码修改、创建 PR，并修复实际 CI 问题后合并。

- G1（complete）：已安全同步 origin/main `0d9e643` 并恢复原修改；README 唯一冲突保留双方说明。为保护并行 UI 开发，原有完整修改在独立 worktree 提交。
- G2（complete）：全新独立 Cargo target 的完整离线 verify --app-smoke 17 步通过，含 4 loop/17 packaging、fmt/严格 Clippy/release/签名/安全 smoke，起止产品指纹一致；证据 `.voiceport/evidence/git-pr-13-verify-4265b885/`。
- G3（complete）：提交 `89ee01c` 已推送，PR13 的 GitHub macOS CI 通过后正常合并为 `980a5ee06757915a386ffe72d18a3c748cdf5286`；无须修改 CI 配置。
- G4（complete）：HEAD、本地 main、origin/main 与 GitHub main 均为 `980a5ee`。只移动引用并重建 index，未改工作文件；剩余未提交修改为另一会话的新 UI 开发及根状态记录，完整保留。PR：https://github.com/forwaryan/voiceport/pull/13 。

保留已有历史记录、本机证据和运行中应用。验证不自动录音、读取真实账号或向真实输入框投递。

## 2026-11-08 当前任务：菜单栏与原生界面美化

用户接受停止后整段最终转写，希望界面更美观；进一步明确只需macOS右上角菜单栏小图标，底部Dock图标不需要。改为0.3.2/build8、LSUIElement=true、Accessory策略，保留可打开的设置窗口。沿用已有修改与保护约束，不退出原已授权应用、不自动录音、不覆盖旧包。

- U1（complete）：菜单栏改为28pt模板语音图标，图形/菜单文字表达就绪、听写、处理和待处理状态。最终PID45517实际坐标(965,4)，已读取menu-open.png确认右上角显示；系统将VoicePort归入隐藏的Orca宿主组，正常系统UI开启该组后可见，Orca自身图标也可能随之显示。保留VoicePort可访问名称；深浅适配使用系统语义色，未实际切换外观验收。
- U2（complete）：原生标题层次、圆角结果卡片、空状态、主操作与设置分区已实现并读取实际截图；保留全部功能入口，重复投递错误行不再显示两遍。
- U3（complete）：最终完整离线17步全通过（verify-d063b3b372364ea9883fc0adf87ebf97），4 loop/17 packaging，起止产品SHA f4be731c83b7f03d61391601843a6f17d92f83a24e26bd883f4f3cb6c99e6d25一致。新包dist/menubar-only-20261108-9eaa1fc4-final/VoicePort.app为0.3.2/build8，签名/Cargo对应通过；实际PID45517为Accessory policy1、Dock无VoicePort，关闭后窗口数0，经右上角菜单在同PID恢复为1，0秒/未录音。report.json与已读截图同目录。新包自身AX仍未授权，未重绑；真实语音/长期耐用性尚未验收。原先屏幕外菜单及旧进程消失记录保留；旧PID68143/42195未由本会话退出，原因未确认。


## 2026-11-08 当前任务：光标输入与 macOS 图标入口接续

保留分支 fix/macos-app-entry-and-status 的全部未提交修改。旧 session fe46d843-d296-4499-9a98-3a90c077057a 的 terminal drift fence 永久保留，不恢复、不重绑、不清零。不自动录音，不覆盖旧包，不退出任何已有应用。

- N1（complete）：新 session 9eaa1fc4-9e2b-470c-a2a0-ef0f1de7fde6、实际 Orca terminal term_6ed39ed6-8f1f-49a4-bb8c-e5a1791bff0b 与真实 PreToolUse 状态一致；初始 boundaries=[]，helper SHA 014eeb1e43729675ab5a97ae42fde3aaac1f0f460230e449683c85a0f9c7daaa。SessionStart 未观察。
- N2（complete）：独立 Orca 终端夹具精确接收固定22字节 UTF-8 文字，无Return；原发送实现通过。新证据 dist/cursor-entry-20261108-9eaa1fc4/orca-terminal-receiver-02.json，旧 NSTextView 证据保留。
- N3（complete）：图标、Dock/菜单栏、正常启动与窗口关闭后重开均完成；完整离线17步、4 loop/17 packaging、fmt/严格Clippy/release/safe smoke全通过，起止产品SHA e5a7b98ae485405abb12b7bca755101ad3612aa37916f2c4a0f1da569140fb32。
- N5（complete）：用户授权后通过系统UI移除仅VoicePort旧条目，明确选择cursor-entry-20261108-9eaa1fc4/VoicePort.app重新添加。PID68143同进程实时按钮已为「辅助功能：已授权（检查）」、0秒/未录音；新证据permission-repair-20261108-161159.json。原report.json保留历史未授权事实。当前真实压缩1，绑定不变、无drift。
- N4（complete）：新包 dist/cursor-entry-20261108-9eaa1fc4/VoicePort.app（0.3.1/build7）签名与Cargo产物对应通过。实际PID68143同进程关闭后经Launch Services、Dock点击及VP菜单恢复窗口，最终截图0秒/无录音；新包辅助功能未授权，留给本机手动处理。report.json、MANUAL_TEST.md、截图同目录。未提交或推送。

本轮错误与处理：早期探针前台/活动tab漂移、指定receiver timeout不计成功；新有界create/switch/probe并核对receiver后通过。首次GUI close返回permission_denied，但原生窗口数0/同PID仍存活证明已关闭，后续重开确认，不修改系统权限；System Events可选AXSubrole缺失查询报-1728，改为容错枚举后定位close。全部旧错误回执保留。



## 2026-11-07 当前任务：移除旧版 Swift 残留并更新 main

用户明确要求“旧版没用的都删掉吧，然后更新一下主分支”。从已合并 Rust 迁移的 main `def833a` 建立清理分支；移除 `legacy/swift` 的旧源码、SwiftPM 清单、测试、夹具和说明，修正当前文档与产品输入清单。现有 Rust 测试入口仍有实际用途；历史验收记录不改写，运行中的应用、本地包、凭据、录音和私有证据不动，不自动录音。

### C1：移除旧实现与失效引用
Status: complete
- 删除37个归档文件（35个 Swift 文件、1个 Python 夹具、1个说明文件），从产品清单移除 legacy 必需目录；更新当前文档，旧实现由 Git 历史保存。

### C2：清理后离线验证
Status: complete
- 完整离线17步通过，含Rust工作区测试、fmt/严格Clippy/release、4 loop/16 packaging、签名和安全smoke；起止清单SHA `e9b3d5c353b32835045f08f56586fa513e376ba1844fbba8531d5ca1d137de7f` 一致。
- 跟踪Swift文件为0，37项删除均为旧归档；当前文档链接有效，干净Git archive与工作区产品清单相同，不依赖本机旧目录。

### C3：Git 交付准备
Status: complete
- 清理提交已推送并创建PR11；完整验证通过后补充记录与PR结果，按用户授权正常合并。合并结果及本地main同步以实际GitHub回执和Git引用核对为准。

## 2026-11-06 当前任务：完成 Rust macOS 迁移

用户明确要求“你继续吧，把rust迁移完啊”。基线为 main `80e5ada`，工作分支 `feat/rust-macos-native`。本轮目标是用 Rust 实现并打包 macOS 应用，运行链路不再依赖 Swift。保留已有 Windows 工作和历史证据，不将编译通过等同于真实语音/Orca 验收；不自动录音、读真实登录、上传录音或向用户真实输入框写测试内容。当前正在运行的已授权应用不退出、不替换；新包先使用独立输出目录验证。

### R1：迁移设计和行为对照
Status: complete
- 核对 Swift/macOS 各功能与 Rust 接口，确定原生 API、模块归属、资源和验证边界。
- 使用 Rust 调用 macOS 系统框架；保留同等录音、本机识别、权限和输入所有权保护。

### R2：Rust macOS 功能实现
Status: complete
- 录音与本机识别、私有录音存储、普通输入框与 Orca 投递。
- 菜单栏/设置/快捷键、会话控制、转写与手动重试、非阻塞取消/退出。

### R3：构建、打包与迁移收尾
Status: complete
- Cargo macOS 应用入口与独立 Rust 打包/验证；更新依赖许可材料。
- 移除默认构建运行对 Swift 的依赖，保留可追溯行为对照及必要历史记录。

### R4：验证与交付
Status: complete
- 最终完整17步、160顶层Rust/4 loop/16 packaging、签名和实际空闲退出均通过。最终包已启动且0秒，麦克风/Speech已允许；辅助功能仍待下述本机系统授权，不等于真实语音/自动输入验收完成。
- 有针对性的 Rust 回归、fmt/Clippy、release 构建、签名和无录音启动验证。
- 实际打开新包检查界面与设置交互，不触发真实录音或投递。
- 独立检查关键生命周期、输入所有权与打包边界；记录实测范围和未执行事项。

迁移发现和运行结果持续写入 findings.md / progress.md。以下旧任务与失败记录仅作历史背景，不继续旧 Windows 冻结任务或轮换流程。

### 试用外部条件：最终包辅助功能授权
Status: blocked
- 最终包已启动 `dist/rust-native-20261106-final/VoicePort.app`（PID28046），麦克风与语音识别均允许。
- 系统中同名条目开启且已选择最终包，但本应用回调仍显示辅助功能未允许；已触发系统“辅助功能访问”安全窗口，自动化对该窗口返回permission_denied，需要用户在本机系统界面完成。未移除旧条目、未重置TCC或读取/代填密码。
- 不自动开始录音，真实录音/云端/目标输入不纳入本次自动检查。详细报告见`.voiceport/evidence/rust-native-final-20261106/report.json`。

本轮最终验证发现/处理：
- 首轮完整17步通过（160顶层Rust/4 loop/16 packaging，另9个storage子进程不相加）；1950包实际窗口和不录音授权回调通过，但空闲TerminateLater退出未完成。已修正同步空闲退出和modal/common poll timer，须以修复后新包重验；1950包保留作中间产物。
- 系统认证由本机系统处理，不代填密码。权限picker的后台导航失败已改用核对前台的原生事件；不把工具返回成功当作UI成功。原设置automaticPaste=1已恢复，language为空，alternativeShortcut未设置。
- 输出新目录，不覆盖dist旧包或交接中间包；旧授权应用PID75191保持运行。

## 目标与授权

依据用户指定的历史会话 0004ca7f-8d0e-4201-82c5-42ad7e7fce20 还原产品目标，借鉴 x-insight-hub 的 loop 策略，先给出完整可迭代方案，再持续实现、验证并修订方案。2026-11-04 用户进一步明确希望同时支持 macOS 和 Windows、目前主要做 macOS，并询问 Rust 方案；现有 Swift/macOS 作为可运行基线，Rust 共享核心与平台适配路线见 docs/CROSS_PLATFORM.md。用户随后明确要求“执行一个新版本rust的loop开发开始”，现已恢复工作区访问，按 Phase 5 启动实际迁移。

授权范围：本项目本地方案、代码、依赖安装、测试、运行验证和会话内 loop。2026-11-04 的初始化请求已完成；同日用户明确要求“给我都push一下啊”，授权将本轮源码、测试、脚本和文档提交并推送到 forwaryan/voiceport。凭据、录音、构建产物和私有运行证据不纳入 Git；外部部署另按当前用户授权判断。历史会话是需求材料，不直接执行其中的旧操作命令。

## 2026-11-05 协调交接与提交优先级

- 用户明确要求由新会话接任，并支持压缩达到阈值切换协调者；随后要求先将已写代码拆成多个 commit 并 push，并将 commit/push、合并 PR、拉取最新 main 都写入持续 loop。首批已由 PR1 合并并同步 main；当前分支 `fix/windows-cleanup-stop`，交付 Windows 聚焦返修和阈值规则更新；保留私有运行证据、凭据、录音和构建产物在本地。
- 主协调者沿用原Run，维护方案/账本并通过多会话loop推进开发与独立验证。2026-11-05用户指出验证窗口已压缩10次：旧实现只约束协调者，未覆盖worker。现统一所有角色最多5轮上下文=初始+4次压缩，进入最后一轮即保存换窗并veto第5次压缩，无额外压测/强制压缩。双宿主按真实session计数，新窗口须新session ID/初始0及实际hook加载证明；原生Codex信任缺失先阻塞，不能只凭JSON声称生效。保留Run/产品attempt/逻辑角色与Task/Dispatch沿革，旧证据不变。
- 自动跨窗口交接尚未通过真实压缩触发验收；不能把模型按固定步骤交接的 hook 约束说成外部进程监督器。后续协调者须按 `.claude/COORDINATOR_ROTATION.md` 处理阈值和失败恢复。
- Windows freeze-04 独立失败 VP013-I-003 永久保留。原开发返修03已停写并交付：请求显式双重 Job/许可所有权、真实 receiver 生命周期替代 is_finished 推断；原清理 TLS 探针修前 control0/poll1/exit1/orphan1，修后四例均0，健康后续请求可继续执行。55 Rust、fmt、严格 clippy、完整 Windows 默认目标检查均为开发自检0。当前源码已分逻辑commit/push，PR2已创建；完整freeze-05现已由原独立确认VP013-I-003修复且通过阶段检查。协调核对779公共+5操作输入、771保护、66261历史普通及1742新证据107932774字节后接受返修阶段，继续PR合并/main同步，再以Windows私有存储/held lease作为下一切片；VP-013整体仍未接受。

- Windows私有存储续作01已提交推送PR3，完整freeze-06由原独立task_efd03ffe42f8 / ctx_79e1ee651d6f通过源码/编译/离线所有权阶段；协调check-vp013-accept-06.py首次0核对782公共+5操作输入/276+2目录/766保护/69107历史普通及1851新证据119311468字节后接受阶段。独立Mac25步60Rust、完整默认Windows目标检查及新存储/真实TLS guard控制通过；3native测试仅编译，实际NTFS安全与平台运行未接受。原service-TLS101与manualRetry适配0分列，历史构造器和一处只读accessor迁移均显式保存。VP-013原attempt保持running，不finish。接着按授权更新状态、提交推送、正常合并PR3并安全同步main；下一切片为惰性Windows设备采集基础，合成后端与交叉编译验证，保持桌面Start/Stop禁用及原生安全/设备实测门槛。

- PR3已正常合并，main同步a86934eb5765869ddc32836cbe58471b52d885d2后建立feat/windows-capture-foundation。原开发续作05 task_a1066a7a191a / ctx_1f746fb9da0b已msg_c973ec95a9cd succeeded并停写，交付惰性capture/Session/WinMM候选（4修改3新增）。check-vp013-capture-01.py首次0核对785输入/277目录/778保护公共+5操作输入/71765历史普通及101新证据2234880字节。开发71Rust、12最终命令及完整Windows目标0，1采集+3存储native测试仅编译。提交推送后完整freeze07回原独立ctx_79e1ee651d6f；仅开发自检，全VP013同attempt保持running/false，真实设备和桌面Start/Stop仍禁用。用户再次明确允许协调者自主合并PR，满足正常条件后继续合并同步main，无需逐次确认。

- 当前优先级：旧freeze07验证已按用户轮换指令guarded-stop确认退出并释放，16项未完成证据保留，行政中断不算产品退回。协调者只改操作机制/文档；native开发启动被新增hooks信任提示挡在agent_readiness，Task未送达，保留该实际阻塞；已另起Claude文档支撑窗口同步角色文档，新独立窗口核查固定的四个操作输入与窄范围控制。配置/文档完成提交推送后建立完整freeze08再由原独立角色新上下文继续采集验收。VP-013原attempt仍running/false；PR4待独立，不合并未完成评审。

## 2026-11-05 操作审查失败后的当前优先级

独立task_bc7c0039fdce/ctx_a9a953b3b687已真实msg_81ea1bb2c3c1报告FAILED：23项套件通过，但19项独立控制中7项失败（同inode重写漏计、协调者缺transcript放行、Claude/Codex身份缺口、通知后notes可改、notes跟随symlink、checkpoint实际terminal不核对）。失败报告/初始阻塞/四输入原字节完整保留，明确结束该窄范围固定输入边界，仅授权helper/tests返修02。文档支撑task_d776312c40fe/ctx_d2cc78eb7bfe已msg_3e971eea8611成功，六文档交付hash核对一致；两worker均实际exited/released。接续两路真实会话：实现返修与九份固定策略文档只读独立核查；源码完成后重新pin，由原独立逻辑角色复核，再提交推送、新完整freeze08恢复采集。native信任UI仍阻塞且Task未执行，VP013同attempt保持running/false。详见私有evidence/session-rotation-02/transition.json。

## 2026-11-05 最新Git执行状态

用户明确要求所有PR立即合并并同步本地，已正常合并唯一open PR4，merge bf128fe2295f94cbabf388bdf41ce264180bac8a；open PR为0。当前分支HEAD、本地main、origin/main都已到此提交，核对合并文件树与原候选相同后仅快进refs，未覆盖当前两worker的未提交改动。采集独立和全产品仍未接受；新指令改变Git集成时点，不伪造验收。旧open回执保留，新merge/local-sync回执见evidence/git-merge-capture-04-user-request/receipt.json。后续继续操作返修/文档独立核查，再提交轮换修正并在新完整freeze继续采集独立，不重复合并PR4。

## 最新工作边界

owner01已由原开发msg_dc9ee6e2ccb9封存交付且实际exited/released；协调evidence/vp013-capture-owner-01-coordinator-check.json通过124新普通/22目录/521产物、789公共/278目录/784原公共未变+6操作、77222历史SHA/25134目录mode/104676初始历史身份。开发17组owner、99Rust、byte-exact16/13/2、fmt/host-native严格clippy及完整locked DEFAULTWindows通过，4native仅编译；协调未重跑委派测试。初次协调checker1把正常PR6/main同步后字节相同测试的旧inode/mtime误当当前约束，原脚本/输出保存在handoffs/vp013-owner01-checker-failure-01；纠正按精确12条历史live-source引用分类，新当前身份另核对，历史row/report/snapshot不改，纠正0。开发初始101/脚本/发现错误保持。专用off-UI owner/只读query交付目前仅开发自检，先分逻辑commit/push和draftPR，再完整freeze11交原独立角色真新会话复核线程/析构/TLS/有界控制/显式恢复与完整离线入口。当前同attemptcd2c8d7d running/false，不retry/finish；GUI未接，无真实设备/录音/上传/输入。

PR6已正常merge41667c09b0ef32ec83cd2d58330450984d8dd216，接受状态304d7aa已push，本地main/origin/main/远端一致、工作区clean且候选tree相同，回执evidence/vp013-capture-10-git-merge.json。新分支feat/windows-capture-owner从该merge沿原开发角色派发task_9b4a5d143b12/ctx_6aa95cc7a52f；actual新native session01a1abda、精确terminal、当前helper、真实SessionStart/PreToolUse、初始0已由协调对live状态及authentic transcript核对。完整spec handoffs/vp013-capture-owner-01.md不改；question msg_69f185fb1ad4已按源码/design审查授权仅native facade只读cleanup_status委托和capture.rs查询，addendum同前缀-cleanup-query-addendum.md，避免begin隐藏清理失败导致后台隐式重试。旧方法/释放转换不改，新增delta纳入最终封存/新完整冻结/原独立角色复核；当前仅开发进行中，未有新自检或验收结论。协调者本轮只维护计划/角色/私有决策；同attemptcd2c8d7d running/false，GUI未接，不自动真实设备/录音/上传/输入。

完整freeze10已由原独立msg_171500dd9ff5通过限定源码/编译/离线所有权阶段：C09-001独立FIXED，C08-001/002仍FIXED，无新增本范围缺陷。原final16/original13/adjacent2分别16/13/2pass0，六组新增挑战381例全0；fresh完整25步含82Rust/31core/73app/4loop/11packaging及6预期负例、当前693文件helper/main签名与安全smoke、完整locked DEFAULTWindows/16COFF/2120719字节有效metadata通过，1采集+3存储仅编译。协调check-vp013-accept-10.py首次0核对929新普通/260目录/129056217字节/1466产物、787公共/278目录+6操作/3目录、783保护、77222历史普通/25134历史目录mode/10links/6specials及796语义Git输入；未重跑委派测试。真新独立session08a2b209/精确terminal/当前helper真实PreToolUse/初始最终0核对；项目SessionStart因transcript尚未存在exit1保持失败，不冒称成功激活，实际PreCompact/完整生命周期未执行。旧08/09失败、生成dist重建例外和全部checker错误/纠正历史保留。

原开发03和独立10现已实际exited/released，协调接受记录evidence/vp013-coordinator-checkpoint-acceptance-10.json；当前VP013同attemptcd2c8d7d仍running/false，不finish或再retry。安全点先状态commit/push、PR6正常ready/merge、fetch与main安全快进同步，再从合并main沿原开发角色真新changed-scope续作专用off-UI capture owner：惰性构造、有界控制/结果、取消/迟到拒绝和实际线程/析构释放证明，合成后端与编译先行，不将Start/Stop接到真实GUI或自动开启设备。实际NTFS/device/driver/security/runtime、cleanup/protected delivery、完整Windowslink/PE/packaging与真实轮换生命周期仍是必须门槛；无自动录音/上传/真实目标输入。

前轮边界沿革：

原开发角色返修03已真实msg_139db395f13f交付并停写。独立的explicit_recovery_required记录正常返回的reset/unprepare/close错误，直接Drop保留owner/output/permit；新显式stop/cancel/poll成功完成该阶段后才解除，后续新阶段失败重新要求显式恢复。首错与永久panic中断标记不混用，先前C08的字段展开保留和普通有限排空行为继续。原first13挑战修前10pass3fail101、三暂时故障修前0pass3fail101不改；修后原final16/original13/adjacent2分别16/13/2pass0。新增6组在旧源3pass3fail101，最终工作区82/fmt/host严格clippy/完整DEFAULT Windows及native严格clippy均0，1采集+3存储仅编译。

协调核对126封存普通/12目录/267指定外部产物、787公共/278目录、783未改公共+6操作、75338冻结历史普通/24573目录mode/102228现有历史身份条目；新session01a1ab9e真实native SessionStart/PreToolUse/当前helper/精确terminal/初始最终0通过，不改旧失败激活Task或实际PreCompact/完整生命周期未执行事实。协调checker首次对旧repair02索引误取selected_external_artifacts而1，原脚本/输出保留后按其external_key_records及09独立索引分别核对0，未重跑委派测试。当前attemptcd2c8d7d保持running/false；交付核对不是独立接受。先分逻辑commit/push源码测试与文档状态、更新draftPR6，再完整freeze10沿原09/08独立角色真新changed-freeze会话复核C09反例及显式恢复/首错/panic/有界Drop相邻路径。失败08/09/旧证据不变，修复独立通过前不合并，全产品/native/runtime/GUI/fullWindows仍false。

前轮边界沿革：

完整freeze09由原独立采集角色真实msg_82ffcbb35b57退回。原VP013-C08-001/002已独立确认修复；新增P2 VP013-C09-001：显式stop在reset/unprepare/close正常返回错误后，Session Drop首次无条件cancel又poll，隐式重试未知释放步骤。持续失败三例expected调用1/actual2，首次挑战10pass3fail101；移除暂时故障后直接Drop三例expected(1,容量占用,guard析构0)，actual(2,容量释放,guard析构1)，均101。显式poll恢复正控制与原13/相邻2仍通过；这违反已指定的显式恢复边界，不声称已演示释放仍由driver持有的指针或实际Windows复现。完整25步含76Rust/31core/73app/4loop/11packaging和DEFAULT Windows编译通过，不能抵消失败。

协调核对932封存普通/262目录/819外部产物、786公共/278目录+6操作/3目录、782保护、75338历史普通/24573历史目录mode/10links/6specials及795 Git输入通过，未重跑委派测试。协调新checker首次错选接管前worker stub而AssertionError1，原脚本/日志保留；改用实际协调lease、Run绑定和真实transcript两次压缩后核对0，未改worker证据或计数。原开发和独立均实际exited/released。真实实现退回已retry旧attempt7feb749c并start新attemptcd2c8d7d-c39a-43ac-893c-15e140b88c87，同Run/角色沿革/旧快照不变，非行政换窗重开。PR6保持draft；先提交推送本次失败状态，再沿原开发角色changed-spec真新会话返修03，仅capture释放错误的隐式/显式重试边界与必要测试/Windows说明。旧native开发session绑定已释放终端，不send恢复到新terminal、不重绑/清零；真实新session/current hook/初始0证明先于源码工作，缺加载/信任即升级停止。修后完整新冻结和原独立复核仍必需，全VP013/native/runtime/GUI/fullWindows仍false。

前轮边界沿革：

原开发 capture返修02 已正式交付并停写：OwnerSlot在字段展开时保留未确认Owner/Permit，normal-return latch覆盖backend/output调用、完整析构与finish；普通off-UI Drop只在满批已返回数据后有限续排，poll仍限四批，Idle/未知释放立即保留。原13测试体/Toml/lock/相邻夹具字节相同，开发修前10pass3fail101、修后13pass0；新增5组及工作区76、fmt、严格clippy、完整默认Windows检查/native lint均0。协调94封存普通/17目录、786公共/278目录、781保护+6操作、73453历史普通和旧965独立记录核对通过；协调checker首次结束证明schema误取字段1已保存原脚本/输出后纠正，未重跑开发测试。原开发真实resumed Codex session处于第3轮，当前SessionStart/PreToolUse加载已观察，仅此会话证明，旧native激活失败Task和实际PreCompact/完整生命周期未执行事实不重分类。当前7feb749c返修attempt running/false，先分逻辑commit/push并更新draftPR6，再完整freeze09交原独立采集逻辑角色真新会话复核两缺陷及相邻路径；开发自检不算独立接受，旧freeze08 FAILED和三失败断言永久保留。

前轮边界沿革：

capture freeze08 已由指定独立角色完成并真实退回：VP013-C08-001 析构 panic 使未确认释放的 Owner/Permit 在展开中提前释放；VP013-C08-002 普通 Drop 只排空四批，正常已返回的第五批使容量永久占用。原成功断言保持，独立13项为10通过/3失败（expected0/actual101），另2相邻控制通过；完整Mac25步含71Rust/31core/73app/4loop/11packaging及完整默认Windows目标检查通过，均不抵消已实现范围失败。协调封存/源码/785公共+6操作/768保护/73453历史普通及965新证据核对首次0，明确结束freeze08现场只读边界，原快照/失败不改。按真实退回关闭旧attempt并start返修attempt，原Run和开发/独立角色沿革保留；修复仅capture所有权/有界正常Drop与必要测试、Windows说明，修后必须完整新冻结回原独立。Git从已同步main建立fix/windows-capture-drop；真实设备、原生安全/runtime、GUI owner/Start-Stop及全产品仍未接受，不自动录音/上传/真实输入。原开发恢复须先证明当前真实hook和上下文未超限；无加载/信任则停止并升级，不绕过。

前轮边界沿革：

独立06已真实msg_caf06f0e7daf SUCCEEDED/PASS_WITH_EXECUTION_LIMITS，R05-01/02/03均独立解决、无本范围实质残留；55/19/10/原04五组/review05四组及三组独立事务控制各一次通过。协调核对69封存证据、32历史身份及六输入/七文档不变，接受精确repair06源码与两恢复文档支持的隔离范围，记录evidence/session-rotation-06/coordinator-acceptance-06.json。实际PreCompact/四真实压缩/完整自动生命周期/native信任激活仍未执行，VP013同attempt running/false；原FAILED不重分类。源码/证据停写，开发06已exited/released，独立完成后live/reclaimable不称退出；安全点执行final source/status commit/push、PR5正常ready/merge/main同步，再从同步main制作完整capture freeze08并沿原独立采集角色新上下文续审。旧freeze07行政未完成及16项材料保留，不自动录音/上传/真实输入。

前轮边界沿革：

返修06已真实msg_acf6c299283d succeeded并停写，开发55生产、原19/10控制、原04五组和review05四组控制各一次通过，原51测试体及旧断言不改。协调80封存条目/32历史身份/六新输入/七文档核对，原六恢复诊断失败、四畸形history veto和八语义拒绝结果符合自检；review-pins-06固定helper014eeb1e/testsbf440d07，两设置/两恢复文档不变。新独立06真新session/terminal/真实hook/初始0复核源与政策支持及新事务/兼容路径，不把开发自检当接受。完成后早期进程live/reclaimable与停写声明分列；失败历史/初始oracle错误保留。待实际独立结论再final commit/push/正常mergePR5/main同步和完整capture freeze08，VP013同attempt running/false。证据evidence/session-rotation-06/coordinator-repair-reconciliation.json。

前轮边界沿革：

独立05已真实msg_dda1b489d629 FAILED，51/19/10及适配后五组原04探针独立通过，R04-01核心已独立确认；剩余R05-01诊断写失败阻断恢复、R05-02 hook_events:null漏校验导致exit1无有效veto、R05-03四种畸形schema/cursor/drift证据仍获窄恢复权。协调核对62封存条目与六输入/七文档不变，结束review05现场边界并保留快照/全部失败；返修05开发已实际exited/released，独立完成后仍live/reclaimable不冒称退出。先把已完成返修05源码/状态分逻辑commit/push并更新draftPR5，再明确新开发沿革repair06只helper/tests聚焦诊断与权威分离、完整消费字段校验/双宿主错误否决；不以新增日志路径变更恢复前提。完成新pin独立后才正常merge/main同步及capture freeze08。原Run/VP013attempt running/false，native/实际生命周期未执行。证据evidence/session-rotation-06/coordinator-return-reconciliation.json。

前轮边界沿革：

返修05已msg_d0db2013f50c正式succeeded并停写，协调核对62封存证据/21历史身份/46原测试体和探针一行适配，开发51/19/10与修正后五组控制通过，原失败exit1保留。已固定review-pins-05六输入（helper bd0950e9/tests f6659342、两设置/两恢复文档不变）及七未改政策文档；新独立05以真新session/精确terminal/真实PreToolUse/初始0复核源与政策支持，不重复开发测试、不提前接受。返修完成后进程早期观察live/reclaimable、autoRelease08:59:01，与源码/证据停写声明分列；不称实际退出。待独立实际结论再final commit/push、PR5正常merge/main同步、完整capture freeze08。VP013同attempt running/false，原FAILED和原生/真实生命周期未执行边界不变。证据evidence/session-rotation-05/coordinator-repair-reconciliation.json。

此前边界沿革：

2026-11-05 新协调4f882144/term_c7ce7c33已从自己的terminal绑定原Run，generation4/claim epoch3成功，paused=false，真实PreToolUse/当前helper/新transcript初始0已核对，SessionStart未观察。接回真实msg_c763265d9b12：独立04 FAILED，46/19/10通过但R04-01（transcript错误丢漂移锁存，A返回可工作/压缩）、R04-02（坏transcript阻断Read/status/escalation恢复）未解决。协调核对45封存证据和六输入/七文档不变，两旧进程现已真实exited/released；旧review04现场冻结边界结束，快照/失败不改，仅下一明确helper/tests返修05可写。先commit/push返修04候选并更新draft PR5真实失败，再原开发逻辑沿革的真新会话聚焦两反例；完成后新pin独立，不重开产品attempt。VP013仍running/false，native信任/真实生命周期未执行，capture freeze08继续待办。证据evidence/session-rotation-05/coordinator-return-reconciliation.json。

以下保留前轮边界沿革：

返修04已真实交付，开发46/19/10和双宿主合成恢复通过；协调69证据/六固定输入/七未改政策文档核对完成。新review-pins-04冻结helper/tests、两设置及两恢复文档，回原独立逻辑角色的真新会话同时接受操作源/两文档；根规划/registry不纳入固定输入。旧失败/政策03旧快照接受不改，PR5仍draft，待新独立后commit/push final修复、正常merge及main同步，再完整capture freeze。source/evidence停写声明与早先live/reclaimable进程观察分列，不伪造exit。

操作03已真实FAILED：原7反例全部解决，但恢复同session到新terminal时活跃工具放行而到上限checkpoint拒绝；整体生命周期不接受。明确结束旧四输入评审边界并保留报告，下一返修04在普通工作前拦terminal漂移，采用主协调精确停止/释放后新真实session续接的窄恢复路线，不静默改state/清次数/冒用caller。仅helper/tests和协调轮换/HANDOFF文档获授权，须新pin回独立；既有九文档政策接受仍绑定原快照。先保存候选阶段逻辑commit/push，剩余失败显式标注，随后真实开发返修。VP013同attempt仍running/false，采集独立待新完整freeze。

九文档策略03已由原独立msg_a2720cf8090a ACCEPTED并协调核对，P02-01/P02-02解决；仅文档范围接受。当前等待新真实操作独立task_5c798b4fa492/ctx_3e1715b9efa8对四固定操作输入给实际结论，不改输入/不重跑委派测试；再分逻辑commit/push轮换修正、新完整freeze08继续采集独立。旧失败、终端例外与原生/产品未验收边界保持。

实时扫描续作已msg_7186df11709d完成且exited/released，开发40测试/19控制自检通过。协调核对四输入及两manifest共71项证据身份，当前真实Read重试通过，新操作pins03已固定，转原独立逻辑角色的新实际上下文验收。并行策略03恢复旧session于新terminal，state仍旧terminal；真实transcript/0计数/当前加载一致，仅文档只读工作获明确例外（msg_598f5a8121e7），不改state、不宣称checkpoint/全生命周期通过；操作独立须评估影响。两独立结论后继续commit/push轮换修正和新完整capture freeze，原产品attempt仍false。

此前工作边界（沿革）：

文档返修03已真实交付并exited/released，三文件和15证据核对一致，新的九文档pins03回原独立逻辑角色复核。源码返修02已交开发自检37测试/19迁移控制，不能称独立通过；实际协调Read出现transcript扫描期间变化拦截，原开发同会话新任务task_36fc3677e4bc/ctx_aada54d3498d继续核查正常追加/改写语义，旧repair02证据不改，新live-scan-03路径，源码尚未重新pin。两路结束后独立源码接受，再commit/push轮换修正与新完整freeze08继续采集。PR4已merged，本地bf128fe同步，无需重复合并，产品同attempt保持false。

## 文档返修03沿革

九文档独立核查已完成并RETURNED两项：P02-01现行fail-closed/身份/checkpoint保证须注明初次操作审查7反例、返修和新pin独立未完成；P02-02 README Rust CLI状态须反映已接受的执行器/存储阶段与待独立采集候选，不能仍说未实现/旧join拒绝。核对保存九输入原字节后显式结束该文档固定边界，仅授权README、COORDINATOR_ROTATION、HANDOFF三文件返修，与helper/tests开发无冲突。两路真实交付后再pin回原独立逻辑角色；PR4已按用户新指令合并同步，无需重复，产品仍false。

## 阶段

### Phase 1: 还原需求与约束
Status: complete
- 提取指定会话中的用户目标、接受的建议与未决问题。
- 阅读参考项目 loop 方法并选择适合空项目的最小机制。
- 记录需求来源、事实与假设。

### Phase 2: 完整方案与执行契约
Status: complete
- 写产品范围、架构、数据流、接口、风险、里程碑和验收标准。
- 写 docs/LOOP_METHODS.md 和机器可读取的任务状态。
- 根据明确目标选择技术栈；保留协调、开发、独立验证和按需效果评估的多会话职责，角色身份与实际投递回执保存在私有运行记录。

### Phase 3: 首个可运行的端到端切片
Status: complete
- Swift 转写核心、CLI 和原生菜单栏应用已实现；离线传输 stub 验证完整请求/响应。
- 31 项核心检查、4 项状态回归、release/CLI/.app 启动通过；实际设置窗口已打开、查看截图并测试开关。
- 真实录音、云端与 Orca 端到端验收明确留在 Phase 4 / VP-007，不用离线结果代替。
- 方案已按工具链限制、二进制命名碰撞和投递恢复要求修订。

### Phase 4: 持续循环交付
Status: blocked
- 本地可独立实施的 VP-001–VP-006 已完成；VP-007 需要用户真实录音/账号/权限/目标接收，已记录外部阻塞，VP-008 等待它及同音频样本。按停止条件结束定时 loop，收到解除事件后恢复，未宣称完整产品验收。
- 每轮选择一个可验收任务，分别分派开发与独立验证会话，主协调者负责状态、方案和结果回收；VP-004 的早期结果保留为开发自检，VP-005/VP-006 已实际多会话交付。
- VP-005 已完成：原验证会话确认 VP005-I-001 修复，freeze-02 的 43 文件无漂移，31 核心/44 应用/4 状态回归及 20 项独立检查全部通过；协调者核对真实回执与当前 attempt 后已 finish，真实环境边界保留。
- VP-006 已完成（ba30faf1-05b0-416b-9067-121f757be13e）：版本化私有缓存、归属标记、跨进程占用和严格 >24h 残留清理已由原验证会话对 45 文件 freeze-01 独立验收。20 项独立检查、31 核心/60 应用/4 状态回归及完整 release/CLI/bundle/启动通过；协调者核对真实回执、当前 attempt、完整字节和日志后实际 finish。
- 多会话契约已完成独立内容核对；原冻结 41 项无漂移。保持开发、验证和效果评估的证据边界，不能以常规测试全绿掩盖独立反例失败。
- 独立剪贴板的拥有权/恢复与同步退出验证已完成；崩溃残留清理也已在 VP-006 完成独立本地离线验收。
- 真实录音、云端/Orca 与 Codex App A/B 在本机按 docs/VALIDATION.md 逐项验收。
- 会话动态 loop 持续推进；完成后停止；仅剩无事件的外部阻塞时不空转。

### Phase 5: Rust 新版本，多会话循环
Status: in_progress
- 用户已明确授权开始 Rust 新版本 loop；工作区 EPERM 已恢复，Swift 基线 6d6f42c 已成功推送 main，Rust 开发分支为 feat/rust-core。
- VP-009 已完成（63e8eb1f-6cb6-411a-9875-b0d742636cf0）：59 文件 freeze-01 已由原独立验证会话接受，36 项既有检查、8 组独立核心反例、37 个 release CLI 用例、真实 SIGINT 130、4 项 loop 回归与 fmt/clippy/release 通过；协调者核对完整输入/历史证据/实际日志后真实 finish，未以开发自检代替接受。
- VP-010 已完成（2c6c5e81-6e88-4feb-a73f-e602dddf164a）：65 文件/21 目录 freeze-01 已由原独立会话通过。41 项既有检查、6 项独立 Rust 检查及 46 个有效断言的真实子进程用例符合预期（23 个未修改 release）；协调者核对完整字节/模式/历史证据和实际日志后真实 finish。
- VP-011 已完成（6adda6b1-822d-4516-8b98-87a0ee483656）：freeze-01 的 VP011-I-001 退回历史保留；高水位修复后的69文件/22目录 freeze-02 已由原独立 task_dde77a48fb69 / ctx_5281a4244276 确认修复并完成剩余离线范围。补齐79项矩阵、原bridge/controller反例与正控制5项、额外bridge7项、含原对照的controller/storage13项、raw helper3项及31核心/73应用/八命令符合预期，各组覆盖有重叠。协调者完整核对1690旧普通材料/2链接/6特殊文件、61保护输入、1362新证据和实际结果后真实finish，130秒仍仅terminal阶段、控制及回收另计。
- VP-012 done（ea6b723e-598a-4a2f-b4de-54aceffb68c4，freeze-02独立接受）：以下保留实施与拒绝历史。旧attempt99a0852e-5d5b-4c3e-a3c5-d0b6482751d5的freeze-01已独立退回；首次开发真实start后完善Rust/macOS开发包、完整Rust+Swift离线验证入口、全输入指纹、实际helper/CLI/签名/safe smoke及适用依赖许可材料；原开发 task_cdc471996bd7 / ctx_9157b3116e64 已交付，协调核对710当前输入、4修改/641新增快照、64保护输入、635许可材料与源/包内副本、25步实际日志通过；41 Rust/31核心/73应用/4 loop/7 packaging仍为开发自检。已集成四份状态/方案文档并实际建立freeze-01（710文件/265目录，文件及目录模式、3884历史普通材料/2链接/6特殊文件），实际follow-up原验证，新task_57f21cea2e5e / ctx_88ca56def413使用新自有Cargo target进行完整入口与许可/清单/包/CLI/smoke独立验收，最终msg_d687cea5d770 failed确认VP012-I-001/002/003三类四反例；协调核对710/265/模式/3884历史/2链接/6特殊/61保护与7989新普通证据和原日志后拒绝freeze-01。协调四字段摘要的Path分段/字符串排序错误也确认，旧manifest及失败保留，元数据单独更正草稿未执行。已真实retry/start本轮并reopened原开发ctx_9157b3116e64，新task_ac869bb39540 / ctx_0f12ce81048b，启动保护65输入/12656历史普通材料/3链接/6特殊。返修覆盖根链接、正确main产物对应、许可未知/漏包及未完成入口/许可范围；原开发msg_1b5f33ab3169 succeeded已交付，协调首次核对退出0：762文件/270目录、8修改/52新增、65保护及12656历史普通材料/3链接/6特殊、17377新证据/5链接一致；原四例修前失败修后通过，最终41 Rust/31core/73app/4loop/11packaging及25步入口通过，120依赖687材料来源对应。已集成四份方案文档，实际建立freeze-02：762文件/270目录及模式、30020历史普通材料/8链接/6特殊和余62保护输入，公共schema2由live及完整snapshot实际接口逐项相等。已实际follow-up原独立ctx_88ca56def413，新task_19a4b7f33eb4 / ctx_ec58a916167a复核四原例及全部剩余许可/入口范围，源码冻结，随后原独立msg_2fa94af1de04 succeeded接受完整freeze-02，确认三类四例修复、全部必需本机许可/入口范围完成；协调逐项核对23554新证据/417374403字节/2链接、原冻结与历史及真实日志后已实际finish。独立23清单/6入口/20signed变异/24源许可与真实新target25步符合预期；报告SHA256 cdb7d5578035673f4349f36d17645532baed316760422037574465599d42e984，协调证据vp012-coordinator-acceptance-02.json。原快照保持，验收后只更新四文档，758其他产品输入不变。
- VP-013首次启动attempt4c66c1a3-58a4-46ec-930d-a007f19a4ace（已因独立缺陷retry，当前attempt见最新项）：VP-012 finish后真实start。启动时Darwin arm64仅Mac Rust target；首份交付已安装官方Windows GNU std，完整目标检查仍因缺C编译器退出101，所查常见Windows工具/VM应用未发现，Orca无已连接远端/已配对environment，证据vp013-environment-initial；推进可编译可验证的原生适配与共享接口，实际Windows构建/运行缺少环境时保留外部阻塞，不将Mac/交叉检查当完成。已实际reopened原开发ctx_0f12ce81048b，新task_ca5492581a44 / ctx_2334a32d8f09，终端term_8f600ab28f148cb81d0320f5cd27d25b，规格vp013-developer.md、启动vp013-coordinator-start.json保存762输入/756保护和54441历史普通材料/10链接/6特殊。首份msg_c93126c789b7 succeeded_partial已交付，协调首次核对772文件/275目录、14改动快照、756保护、54441历史普通材料及132新证据一致。7新适配测试及既有Mac回归/15步入口通过，adapter-only Windows类型检查0、完整默认目标检查因缺gcc真实101，未链接或运行。官方Zig索引HTTP200表明本地工具包有候选路线，尚未证明本项目编译可行，因此不直接block。已实际reopened原开发续行，新task_cca0083a8a38 / ctx_c168c4612f21，终端term_8e78d354da8a6699f8d4281eb55ded75，规格vp013-developer-02.md；772输入/756保护及54581历史普通材料/10链接/6特殊启动记录vp013-continuation-02-start.json。继续工具链/完整目标检查及可编译原生UI工作。原独立ctx_ec58a916167a保留，当前尚无VP-013完整冻结/独立接受；同attempt续作02现已交付，当前状态见下项。
- VP-013最新续作02已由msg_ee51019e1f55交付，原开发task_cca0083a8a38 / ctx_c168c4612f21停止写入。协调check-vp013-delivery-02.py首次0：775文件/275目录、6改动/3新增、756保护及54581旧普通材料/10链接/6特殊、161新证据一致；官方Zig归档/成员与编译器原字节核实，真实ring C编译及完整默认Windows all-target检查0，实际链接两次101，仍无PE/Windows运行。新Mac15步先通过50项Rust与既有回归，priority修正后10项适配/lint/fmt和最终完整Windows检查通过，保留时点。仅集成四文档后已实际建立完整vp013-freeze-01（775文件/275目录、54717历史普通/10链接/6特殊、752剩余保护）；实际follow-up原独立ctx_ec58a916167a，新task_eb01b66a93de / ctx_5e90436934ec，终端term_8a7b0a0f-5f38-4fbf-9454-6235ce3ba28a，回执ready。该次等待已结束：原独立msg_8a8b0c7fa982 failed确认VP013-I-001，普通Cancel后任务释放，Actions Poll仍不推进stop，下一次录音Busy。独立Mac25步/51 Rust及完整Windows类型检查通过不能覆盖行为失败。协调核对775/275/752保护/54717历史及1400新证据后真实retry/start新attempt0d21e578-dac8-46ff-aa22-6d19b30b68e1，旧freeze-01归档为rejected snapshot；仅为明确返修解冻当前源。已实际reopened原开发ctx_c168c4612f21，当前task_5ec41d525e67 / ctx_0912c821c9d4，终端term_2db262399fb141fdd50c278ba246efc4，ready。聚焦actions.rs/tests/actions.rs/docs/WINDOWS.md，其他772输入保护，启动保留775输入与56911历史普通证据/10链接/6特殊；规格vp013-revision-01.md。该返修等待已结束：msg_123d8c34ac00 succeeded交付，协调check-vp013-revision-01.py首次0核对三项改动/772保护/56911历史普通/398新证据，原11断言同字节101→0，最终52workspace/fmt/严格clippy及完整Windows默认all-target检查0。已仅集成四产品文档并实际建立完整freeze-02，再实际follow-up原独立，当前状态如下，不沿用旧接受。仍须完整链接/真实Windows运行、录音/安全存储/lease、后台执行器、保护投递及打包，全产品未接受。
- VP-013返修01及续作03历史attempt0d21e578-dac8-46ff-aa22-6d19b30b68e1（已因VP013-I-002真实retry）：完整freeze-02首次实际执行0，775文件/275目录及模式、57313历史普通材料/10链接/6特殊和768剩余保护保持，live/snapshot公共API逐项相等。manifest8f79152a7a50aea9a956854886f731b74a186d7bb2cdce66f8c04a945b237647，content eb6e3a0a0d99cefab4d47a4c14033c3e6651d51cd81033d20ce3e62620627acd，public schema2 e9b02da8320ea3915e3b1af4096e9b22be3d27eb1b612b4c8ef9d35c36874847。已实际follow-up原独立ctx_5e90436934ec，新task_42c9f403abd7 / ctx_d5ff27ba1a13 ready，term_8a7b0a0f-5f38-4fbf-9454-6235ce3ba28a，真实vp013-verifier-02-dispatch-receipt.json。该次独立等待已结束：msg_e25452fc2195 succeeded确认VP013-I-001 fixed，无新已实现范围阻断；协调check-vp013-acceptance-02.py首次0核对原11断言及相邻矩阵、Mac25步/52Rust、完整Windows默认检查0、1234新材料和全历史/保护/冻结，已接受本次返修checkpoint并归档原snapshot。当前同attempt继续后台执行器、结果回收和显式手动Retry，规格vp013-developer-03.md；已实际reopened原开发ctx_0912c821c9d4，新task_f23c2d7491eb / ctx_f235b23b58b7 ready，codex，term_43d14036a089c62f368e33349e31cbab；真实回执vp013-developer-03-dispatch-receipt.json、启动vp013-continuation-03-start.json保存775输入/764保护和59371历史普通材料/10链接/6特殊，规格SHA2566e0afe987603a59f3fd00eb33ee639a8f428ef879dee903d8324613831729473。该次开发等待已结束，msg_13b73ba1e907 succeeded交付；协调最终核对777输入/7修改2新增/764保护/59371历史普通/385新证据一致，最终53Rust及Mac25步、完整Windows默认检查0。原checker误把2样本当16对象的失败保留后纠正，未改产品。已仅集成四文档并实际完整freeze-03、实际回原独立；当前状态见最新项。此次partial checkpoint通过不能finish全部VP-013，Windows录音/安全存储/lease/保护投递/完整链接/实机及打包仍必需，全产品接受false。
- VP-013 freeze-03分派历史（等待已结束、最新拒绝见下一项）：续作03已交付生产executor/结果/手动Retry并完成协调材料核对；完整freeze-03已实际建立（777文件/275目录、59870历史普通/10链接/6特殊、760剩余保护，live/snapshot公共接口相等），manifest abc8748eca7241f4768cb515b02105942e11a32de3b42add7df8d8ce6642bf31。已实际follow-up原独立ctx_d5ff27ba1a13，新task_0d946ca10728 / ctx_3d12c3678161 ready，term_8a7b0a0f-5f38-4fbf-9454-6235ce3ba28a；规格vp013-verifier-03.md、真实vp013-verifier-03-dispatch-receipt.json，specSHA256fba6f89cf90a59d686ac01f2a6c6c6f73ba355c291baad53d368be2cefcd9d14。当前全部777产品输入只读，等真实独立完成事件；自行并发/生命周期/取消/Retry/原生接线反例及原11断言、当前冻结Mac完整入口和Windows默认目标检查。确认缺陷返最新原开发ctx_f235b23b58b7；当时sameattempt running、全产品未接受，后因下项确认缺陷执行真实retry/start。
- VP-013最新running attempt1ed6204e-5637-4c5e-b059-51ac4256a4e6：已消费msg_e46657b5c540，原独立task_0d946ca10728 / ctx_3d12c3678161 failed确认VP013-I-002，实际OsSpawner的is_finished不保证TLS析构结束，同步join使普通Poll/取消Poll/orphan reap三路等待显式release。成功预期0实际1、collector1，正控制通过。协调check-vp013-rejection-03.py最终0核对777输入/275目录、59870历史普通/10链接/6特殊、760保护及1489新证据83348061字节；独立Mac25步/53Rust和完整Windows默认检查0不能覆盖行为失败。wrapper嵌套与材料容器两次协调schema错误及原独立三次脚本假设失败均原样保留。已真实retry/start并永久归档freeze-03 rejected；仅集成四产品文档，其余773输入及全模式保持，已实际reopened最新原开发ctx_f235b23b58b7，新task_aa40d6e71190 / ctx_e86b9f735149 ready，codex，term_65eaecc6c7e05d526d75cb66ce475c5e。规格vp013-revision-02.md SHA2566e2a361deb07c09b455133744eaeaa5c8692d3de2c64a8ae158dcaf90ab5276f；真实vp013-revision-02-dispatch-receipt.json，启动保存777输入/772保护及62159历史普通材料含隐藏pycache/10链接/6特殊。聚焦线程完成确认与消息线程非阻塞join/reap，持有唯一Job/File/许可到实际停止；旧成功断言/正控制保留，任何合法显式Retry恢复适配单列原结果和patch。当前等原开发完成事件，不读半成品、不重复实施/测试；新完整freeze回原独立ctx_3d12c3678161。完整Windows录音/安全存储/lease/保护投递/链接/实机/打包仍必需，全产品false。该开发等待已由下项实际交付取代。
- VP-013返修02已由msg_df14f5209718 succeeded交付并停写，task_aa40d6e71190 / ctx_e86b9f735149及当前attempt保持。check-vp013-revision-02.py首次实际0核对778文件/275目录、3修改1新增/772保护/62159历史普通/10链接/6特殊、1258新材料20840524字节/313目录一致；原样before1、confirmed collector1，原after101保留，仅恢复尾显式Retry适配三路原门闩响应0ms/exit0，原11断言0；最终54Rust/fmt/严格clippy/完整Windows默认all-target0。真实final target16COFF及960961有效rmeta核对，artifact只留2个Windows对象样本，未混全部76artifact。开发join移至一个进程级清理线程，票据/File/许可保留到实际停止；新线程初始化/确认/异常持有等仍须原独立亲自复核。协调仅四文档集成，其他774产品输入及全模式不变；freeze-vp013-04.py首次实际0，完整778文件/275目录、63421历史普通材料含隐藏pycache/10链接/6特殊、768剩余保护保持，live/snapshot实际公共API相等。manifest6488ad38fe9dae740e1a91e45bd988de8192788e6b4849d0bcac54bf25c63a6b、content1d429e9e9d054b64d9a535a35dd5b848bff438309739456e38d8888ec2a9fda2、schema2 public313ebb8ed8da72ae42aed0accd3d67d71b0d4ce6ff0283db90c044a249d9a2b1。已实际follow-up最新原独立ctx_3d12c3678161，新task_e173aaa67be8 / ctx_f27bbb6783ce ready，term_8a7b0a0f-5f38-4fbf-9454-6235ce3ba28a，规格vp013-verifier-04.md SHA256c140e99172ebc3113de86021cdfef2a424378dbdb2bfcf17dac708b25c2fa19b，真实vp013-verifier-04-dispatch-receipt.json。当前778输入只读，等真实完成事件；要求自行核对原TLS缺陷及恢复适配、清理线程/票据/许可/取消/异常owner和当前冻结Mac完整25步及Windows默认目标检查。确认新缺陷返最新原开发ctx_e86b9f735149，阶段通过仍不能finish全部VP-013，不读半成品或重复测试。
- VP-014 ready：Rust macOS用户参与的真实录音/云端/目标接收，等待用户明确开始及账号/权限/目标实测，不自动执行。
- 保留 Swift 行为基线和历史证据，不以 Swift 测试替代 Rust 验收；不自动录音/读取真实凭据/上传或操作其他应用输入。
- 工作会话完成事件主唤醒，动态 loop 1200 秒仅作恢复兜底；真实外部阻塞不影响可独立执行的 Rust 任务，只有无可推进任务时停止。

## 决策

- 需求恢复完成前不猜测 VoicePort 的产品内容。
- 不把原始会话、凭据或参考项目 private 内容写入公开源码。
- 先完成可运行切片，再按验证证据调整后续里程碑。

## 错误与处理

| 错误 | 次数 | 处理 |
|---|---|---|
| 首次 swift test：Command Line Tools 缺少 XCTest 模块 | 1 | Swift Testing 也缺失；改用 swiftc 编译独立契约检查，26 项已通过 |
| 原生构建：多文件 target 的 main.swift 与 @main 冲突；AX 全局常量触发 Swift 6 并发限制 | 1 | 入口文件改名；旧系统模块加 @preconcurrency；debug 构建已通过 |
| CLI smoke 挂起：大小写不敏感文件系统将 voiceport / VoicePort 映射到同一 inode，CLI 被 GUI 覆盖 | 1 | 已停止错误 smoke；CLI 改为 voiceport-cli；补命令输出验证后重跑 |
| CLI help 输出断言失败：重命名时用法字符串空格丢失 | 1 | 31 项核心检查和 release 构建已过；修正用法文字再运行总体验证 |
| VP-004 服务端自行 CancellationError 后没有进入 failed，12 项控制器检查中此项失败 | 1 | 旧行为失败证据在 .voiceport/evidence/vp004-before-fix；已修复并通过全部 12 项及完整 release 验证 |
| VP-005 初始 29 项检查中 5 个投递/退出行为断言失败 | 1 | 保存旧源与失败日志；开发修复后 35 项全过，仍须独立验收 |
| VP-005 独立验收发现 AX role 无法证明只读/禁用目标可编辑，11 项独立检查中 3 项失败 | 1 | 原开发修复角色/启用/写能力判据；freeze-02 原验证复核 20 项反例全过，VP005-I-001 确认修复，VP-005 已完成 |
| VP-006 开发首次存储检查出现 15 项失败 | 1 | macOS O_NOFOLLOW 与 O_NOFOLLOW_ANY 组合返回 EINVAL；修正父目录 flags，隔离夹具以 POSIX realpath 规范化系统父路径，失败日志保留 |
| VP-006 第二次存储检查因目录 URL 尾斜杠比较出现 1 项失败 | 1 | 隔离夹具改比较实际 path；最终 60 项应用自检通过，随后独立验收已通过 |
| 首次提交暂存检查发现 DeliveryPolicy.swift 文件末尾多余空行 | 1 | 之前未跟踪文件未纳入普通 git diff --check；暂存后已捕获。仅删除末尾一个空行，保留已验收快照并记录字节差异，再检查整个暂存区 |
| VP-009 首轮 clippy collapsible_if 失败，退出 101 | 1 | 原开发按诊断合并语言验证条件；最终严格 lint 退出 0，保留首次失败 |
| VP-009 实际 HTTP 反例发现 Serde 结构可接受顶层数组，退出 101 | 1 | 显式要求对象形状，最终 36 项回归通过；原独立验收通过 |
| VP-009 协调核对假设 harness 根目录和每项 CLI marker 非空，两次退出 1 | 2 | 实际复制目录位于 harness/workspace；部分用例 marker 为 null。按真实数据结构修正核对器后完整验收证据通过，未修改产品 |
| VP-010 首次 clippy while_let_loop 失败，退出 101 | 1 | 原开发按诊断改为 while let；最终严格 lint 退出 0，首次日志保留 |
| VP-010 同进程不同 session engine 重用 generation=1，回归退出 101 | 1 | 原开发改为进程级 checked AtomicU64、不回绕；最终回归及独立验收通过 |
| VP-010 冻结脚本把历史合成符号链接当普通证据拒绝，退出 1 | 1 | 新冻结尚未创建；保留夹具，以 lstat/readlink 另记链接/特殊文件，不跟随链接或读 FIFO；普通证据继续逐文件哈希，未改历史数据或产品 |
| VP-010 独立慢读夹具误要求小帧全流 8 秒内结束，runner 退出 1 | 1 | 保留失败和未记录原 child 最终退出码的事实；原负载正对照证明连续小帧可各自及时完成，停读后退出；另以单大帧验证 5 秒总 deadline，不改生产或削弱单帧断言 |
| VP-011 初次 Swift bridge 编译缺少 Bool 返回，退出 1 | 1 | 原开发补 return；完整早期编译日志保留，最终检查通过 |
| VP-011 正常分片夹具在 300ms 预算下 timedOut，66 项中一项失败，退出 1 | 1 | 普通成功夹具改 2 秒，负面超时夹具仍 300ms；最终 69 项通过，保留早期日志 |
| VP-011 协调核对误假设全部输入有快照、未排除 pycache、对 release 核对无关后续测试改动，各退出 1 | 3 | 按实际八份产品快照、30 当前摘要、54 保护输入和每条命令的相关输入核对；无关测试漂移显式记录，最终退出 0，未改产品或重跑自检 |
| VP-011 独立分派已成功后，注册器假设回执必有 agent，KeyError 退出 1 | 1 | 复用已保存成功回执与原角色已知 agent，仅补齐注册；未重复发送、未生成第二任务，修正退出 0 |
| VP-011 独立 bridge 与原控制器断言各退出 1，同 child 重复 s1 被接受 | 1 缺陷 | 确认 VP011-I-001，第二次 automatic=true 投递边界与删第二音频；拒绝 freeze-01，新 attempt 原开发返修，正常推进正控制通过 |
| VP-011 独立完整性报告器循环变量覆盖 manifest_sha256 报告字段 | 1 | 固定摘要断言和材料检查原本通过；原脚本/报告归档，仅修字段来源后完整重核，材料行一致 |
| VP-011 协调查看反例索引时把混合标量组当容器，TypeError 退出 1 | 1 | 只按真实 raw_bridge/original_controller_and_storage 两组核对，最终完整拒绝证据核对退出 0；无产品修改或测试重跑 |
| VP011-I-001 返修首轮 73 项中 failed retry 一项 protocolFailure，退出 1 | 1 | 夹具 accepted 使用自定义 s8 却发硬编码 error(s1)，仅改合成错误关联同 sid；失败夹具/日志保留，原负面断言不变，最终73项通过 |
| VP-011 freeze-02 独立验证早期负载/握手、报告排序与测试编译各退出1 | 4 | 保留20秒stderr负载失败、0.3秒握手及空列表IndexError、跨child枚举误判、Swift函数类型语法失败；同负载用生产130秒、披露2秒控制限、按incarnation排序、只修测试语法后通过，未改生产或削弱原断言 |
| VP-011 接受协调核对器误要求可选expected_exit_code、混淆旧child exit9与新child最终exit0，各退出1 | 2 | 两份原脚本和错误JSON保留；按实际命令schema核对退出0，并分别核对旧child停止断言和替换child终态，最终完整核对退出0，未改产品或独立证据 |
| VP-012首次包装测试删除Cargo.toml后清单抛FileNotFoundError，退出1 | 1 | 改显式missing并保留异常/配方；原临时目录已自动清理不补造，最终7项反例通过 |
| VP-012首次完整Cargo测试运行当前源没有的旧用例，三失败、退出101 | 1 | 受保护Rust源摘要未变；cargo clean -p voiceport-core后同产品摘要完整重建通过，保留首次日志与清理记录，具体缓存失配机制未实证；独立使用新自有target避免复用 |
| VP-012独立发现freeze-01四字段摘要与实际入口不符 | 1 | 协调脚本复用按路径字符串排序的三字段行；产品inventory按Path分段排序，数组首差在索引382。710文件/265目录/模式完全相同，已复现实际摘要2c440f92…及错误声明c44d8dfe…；保留旧冻结/失败，新冻结须调用实际公共接口并核对live/snapshot。元数据单独更正草稿未执行，已完整拒绝后返原开发 |
| VP012-I-001：原inventory接受根docs目录符号链接，负面runner退出1 | 1 缺陷 | 原公共接口exit0且完整清单/摘要与普通目录相同；保留原树/driver/日志，freeze-01拒绝，新attempt原开发返修，尚未修复或接受 |
| VP012-I-002：原bundle checker接受真实Swift CLI替换主VoicePort后的签名包 | 1 缺陷 | 正确fresh Cargo helper及strict签名仍通过，原checker exit0；需绑定正确本轮Swift app产物，已交原开发返修 |
| VP012-I-003：原许可checker接受未知声明/selection及漏atomic-waker整包两反例 | 1 类2例 | 自有bundle改清单/材料后重签仍exit0，120/119包；源清单和原checker未改。需修复来源/完整锁定图/语义信任边界并补齐许可充分性，已交原开发 |
| VP-012拒绝协调核对误把copy_before_signing历史摘要当当前签名后文件，退出1 | 1 | 原脚本/error保存在vp012-coordinator-check-failure-01；按原audit时序核对before与unsigned cargo、当前after与installed helper后完整退出0，无产品改动或测试重跑 |
| VP-012返修首次before夹具因选错旧Cargo产物导致正控制失败，退出1 | 1 | 失败日志/结果保留；before-02使用已记录的真实fresh Cargo产物，正控制过且四原负例确实失败，after-final修后12项通过，不能把首次无关helper拒绝当目标修复 |
| VP-012返修许可audit假设可选.cargo-checksum.json存在，FileNotFoundError退出1 | 2 | 两次原日志/exit保留；03直接校验Cargo.lock中的.crate归档hash并从归档读取全文对照缓存/安装材料，687项通过。相关旧源码语句保留在trace，不假称所有早期源码均另有完整snapshot |
| VP-012 freeze-02独立测试parser缺WITH/legacy slash支持，audit退出1 | 1 | 原完整脚本/异常/patch保留，仅修测试parser并完整重跑120包/687材料后通过；非材料路径查找及list/dict诊断错误另保留，不视为产品通过或缺许可 |
| VP-012接受协调checker误拼接workspace-relative索引路径，FileNotFoundError退出1 | 1 | 原脚本/error保存在vp012-coordinator-acceptance-check-failure-01；按真实索引schema对ROOT解析并做完整集合核对后退出0，无产品改动或委派测试重跑 |
| VP-013完整默认Windows目标检查因ring缺x86_64-w64-mingw32-gcc退出101 | 1 | 原命令/日志保留，adapter-only类型检查0不替代它；续行隔离Zig使完整默认all-target检查通过，链接仍缺GNU运行库，Windows运行未验 |
| VP013-I-001：独立Actions取消恢复成功断言exit101，10通过1失败 | 1 缺陷 | Cancel时Job存活，释放后普通Poll未推进stop，下一录音Busy；原正反控制与失败保留，协调核对后真实retry/start新attempt，原开发聚焦返修，新完整freeze再回原独立 |
| VP-013协调Write调用JSON截断被工具拒绝 | 1 | 未创建文件或执行代码；改完整合法工具输入后check-vp013-rejection-01.py成功创建并首次执行0，无产品改动或委派检查重跑 |
| VP-013续作03协调checker误要求artifact样本包含全部16个COFF，实际只保留2个样本，exit1 | 1 | 原checker/error保留vp013-coordinator-delivery-check-03-failure-01，改核对2样本和实际最终目标16对象COFF头/完整身份后整体0；未改产品或重跑委派检查 |
| VP-013续作03只读index查看假设files字段而实际名regular，exit1 | 1 | 按实际schema有界读取后核对385材料/24目录成功，原工具错误留转录，未修改工作证据；若干诊断输出超预期，后续只显示所需计数与字段 |
| VP013-I-002：真实OsSpawner线程TLS清理期间Poll/取消Poll/orphan reap同步join阻塞 | 1 缺陷3路径 | 独立成功预期0实际1、collector1及全部正控制/冻结保持；协调核对后真实retry/start返最新原开发修复，旧freeze-03永久rejected，不以其他绿色检查覆盖 |
| freeze-03独立核对脚本pin被数字替换破坏、两次误比较依赖集合，exit1；另一次Write截断 | 3脚本+1工具输入 | 原失败/源码/patch保留；精确token重生pin、各平台树与各自metadata/旧身份分别核对后0；截断未写文件，不造日志，无产品修改 |
| freeze-03协调核对器把nested wrapper当identity、把materials列表当整数，exit1 | 2 | 原脚本/error分别保存在vp013-coordinator-rejection-check-03-failure-01/02；按source/owned_copy与687材料实际容器核对后0，无产品/工作证据修改或委派测试重跑 |
| 返修02原after TLS探针保持101，孤立任务恢复尾假定即刻成功 | 原after两轮 | 失败保持，不改称原fixture通过；仅一处恢复尾加显式Retry的适配保留三处响应门闩断言并0ms/exit0，合法契约及新机制仍待原独立复核 |
| 返修02初两次clippy101、发布成功后worker panic回归101、第三轮helper路径错误1 | 4 | 原源码/日志保留；修&Path/unused import、join错误转静态失败、限定helper路径后第四轮54Rust/fmt/clippy/完整Windows检查0；初次before记录器0只记录oracle1，拒绝快照重复collector1另留证 |
