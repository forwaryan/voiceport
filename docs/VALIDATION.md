# 验证记录

更新：2026-11-06。本文件记录实际执行范围；历史 loop 任务执行状态以本机 .voiceport/loop-state.json 为准。

## Rust 原生 macOS 最终本地验证（2026-11-06）

当前交付为 Rust AppKit 主程序，版本0.3.0/build6；默认构建、打包及运行不依赖Swift或JSONL helper。旧 Swift 实现已于2026-11-07从当前文件树移除，可通过 Git 历史查看。本节描述迁移时的验证；下文各freeze历史接受结论仍仅绑定原快照。

最终命令 `python3 -I scripts/verify.py --app-smoke` 的17步全部符合预期，报告 `.voiceport/evidence/vp012-revision-01/verify-b8a8e86215b44f80a7ffaffb1c8fac62/report.json`。workspace全部160个顶层Rust测试、4个loop、16个packaging检查通过，fmt、严格Clippy、workspace release、CLI正反用例、host协议、签名/产物对应和无录音smoke通过。storage另外启动9个隔离子进程，其结果嵌套在父测试输出内，不重复相加；并发stdout有两条summary交错，不能只按行正则计算测试总数。验证入口起止全产品输入SHA均为 `5100c27b285fef85e916466eec2f96672d7a11f6fcfe691ee34971f284f937fd`。本节及后续状态文档在验证后更新，不冒充原输入指纹。

最终包：`dist/rust-native-20261106-final/VoicePort.app`。主程序SHA256 `3650421da32a625bf552f939388edbf565c008d500d6b1792b0b60db4148b33a`；签名前Cargo release产物SHA256 `3dbf5bdb91ed1b5cc10852465a9757ab7df52328b425bd68d37dbc9e415b0ce2`。打包checker确认主程序由本轮Cargo产物重签得到，153包许可材料检查通过。1950包保留为中间产物，不是当前最终包，旧运行应用未替换。

实际Launch Services启动、结果设置窗口AX/截图、自动输入设置1→0→1恢复均已执行。首轮真实空闲退出暴露一律TerminateLater配合仅默认模式timer的退出停滞；修复为空闲同步取消并TerminateNow，重入/活跃worker仍等待，poll timer加入common/modal模式。随后完整入口重跑通过，最终包PID26217真实正常退出，再启动PID28046供试用；不把活跃真实录音退出称作已验。

新增独立“准备麦克风与语音权限（不录音）”按钮，用专用回调通道请求权限，不进入录音控制器开始路径；等待期间禁开始按钮和录音快捷键。最终应用内回调确认麦克风、Speech均允许，仍为0秒/本机预览未启动。实际应用返回辅助功能未允许；终端直接执行diagnostic曾返回true，不能据此宣称应用已授权。已在系统设置选定最终包并请求本应用辅助功能，现有同名开启条目未使最终包授权生效；系统安全授权窗口需要本机确认，自动化对该窗口返回permission_denied。辅助功能/自动输入保持待完成；不绕过系统权限、不读取或代填密码、不重置TCC数据库。

未执行真实录音、真实登录内容读取、云端上传、目标输入或一般剪贴板复制；真实端到端接收、活跃录音/上传退出、Windows运行、公证仍未验。若用户先试手动结果模式，可在应用中自行关闭自动输入；自动输入模式须先确认最终包辅助功能授权生效。

## 历史 Swift 阶段验证记录

环境：macOS 26.2（25C56）、Apple Silicon、Swift 6.3.2、Command Line Tools。

**VP-006 freeze-01 本地离线独立验收通过，账本已完成 VP-006。** 原验证会话完整审阅录音存储与生命周期，自建并运行 20 项独立检查，亲自运行 31 核心/60 应用/4 状态回归及完整 release/CLI/bundle/签名/启动，共 12 个命令符合预期。独立报告 `.voiceport/evidence/vp006-independent/report.json`，完整回归 `.voiceport/evidence/20261104-191611-5b1818/report.json`。45 个冻结输入和 98 个历史证据文件前后无漂移；协调者另核对真实回执、attempt/Task/Dispatch、完整字节与日志后 finish，核对证据 `.voiceport/evidence/vp006-coordinator-acceptance.json`。验收绑定 snapshot；本次验收后的 README/SOLUTION/本文件仅更新状态，不把新文档字节冒充原冻结。

独立检查使用真实 Darwin 文件系统和子进程，包含持锁、SIGKILL 后 OS 释放锁、3 个并发清理器合计只删除 24 个会话一次、标记/模式/链接/时间/边界/失败/路径替换。设备、权限、网络和投递仍是受控替身，未操作用户默认缓存根。开发的 60 项自检证据保留在 `.voiceport/evidence/vp006-developer/`，不与本次独立结果混同。

**Swift 阶段剩余状态：VP-007 blocked，VP-008 等待依赖与样本。** 真实录音、权限、登录有效性、云端返回和 Orca/其他应用实际接收仍未验证。解除条件与本机执行步骤见下文；Swift 阶段曾按停止规则等待用户事件。用户随后授权 Rust 新版本，VP-009/VP-010/VP-011 已各自独立完成，VP-012 完整离线开发包已在freeze-02独立验收并完成账本，当前Rust loop开始VP-013 Windows平台适配；VP-011 最新接受结论见下文 freeze-02 段落，之前交付与退回段落保留历史。

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

## Rust 新版本验收

**VP-009 freeze-01 本机离线独立验收通过，账本已完成 VP-009。** 原独立验证会话亲自运行真实工作区 tests/fmt/clippy/release 与 4 项 loop 回归、36 项既有检查，另有 8 组独立核心反例、37 个实际 release CLI 离线用例与真实 release SIGINT 130；未确认阻断缺陷。59 个冻结文件、20 个目录、557 个历史证据文件和 25 个 Swift 输入前后无漂移，协调者核对实际回执、attempt、完整字节/模式/snapshot/集合和真实日志后 finish。独立报告 `.voiceport/evidence/vp009-independent/report.json`，协调核对 `.voiceport/evidence/vp009-coordinator-acceptance.json`。

验收绑定原 freeze-01 snapshot；本次 README/方案/跨平台/本文件更新只记录验收结论和 VP-010 新阶段，不把这些新文档字节当成旧冻结。VP-010 会话状态机与有界 JSONL 宿主协议也已通过独立验收，协调核对后已完成账本；当前实施 VP-011 原生接入。Swift 基线 6d6f42c 已推送 main；其历史检查不替代 Rust 新实现测试。Rust 本机工具链为 rustc/Cargo 1.95.0，仅实际运行 macOS Apple Silicon 目标。

开发报告 `.voiceport/evidence/vp009-developer/report.json` 保存真实命令、完整 stdout/stderr、退出码和 13 个输入快照。协调者核对当前 attempt、实际 Orca Task/Dispatch、13 个 Rust 输入与快照、25 个受保护 Swift 输入及 CLI 日志一致，证据 `.voiceport/evidence/vp009-coordinator-delivery-check.json`。协调核对没有重跑这些自检，也不构成独立接受。

| Rust 开发自检 | 实际结果 |
|---|---|
| cargo test --workspace --all-targets --locked | 36 项通过：28 core、2 CLI unit、6 CLI integration；0 失败/忽略 |
| cargo fmt --all -- --check | 退出 0 |
| cargo clippy --workspace --all-targets --locked -- -D warnings | 退出 0 |
| cargo build --workspace --release --locked | 退出 0 |
| 实际 target/release/voiceport-rs | 20 个离线正反用例的退出码、输出标记及脱敏检查符合预期；与 Cargo 测试有行为重叠，不相加为独立覆盖总数 |

网络用例使用真实 reqwest 栈和本次新建的 127.0.0.1 服务，凭据、音频和响应均为合成数据。已自检 multipart/安全头、逐请求重读、不转发重定向、无 cookie 持久化、不重放、超时、取消前/等待中/已收到正文 chunk 后取消，以及声明长度和 chunked 响应上限。生产端点不开放覆盖，内部回环入口只供测试。Unix 自建子进程已测试实际 SIGINT 与 CLI 共用处理器；不外推 Windows Ctrl-C。

首次 clippy 的 collapsible_if 失败和真实 HTTP 顶层数组误接受反例均保留（退出 101）。Serde 派生结构原可接受 `["text"]`；修复后显式要求 JSON 对象，最终回归通过。这些开发自检不构成独立结论；原独立验证随后对最终 Cargo/lock、crates、脚本和产品文档的 freeze-01 自行构造上述 8 组反例并亲自执行最终验证，已经通过。

未验证：Windows 编译/运行、真实 TLS/云端、企业证书/代理兼容性、GUI/IPC/系统录音/投递接入。Rust 禁用自动系统/环境代理；本地文件读取有字节上限，但不受网络 deadline 管理，也不能中断单次同步文件调用。后续 VP-010/011/012 分别覆盖会话/宿主协议、macOS 接入与整包离线交付；VP-013/014 分别覆盖 Windows 系统适配和 Rust 真实环境验收。

原 Swift 任务 VP-007/008 的外部条件继续保留，但不阻止可独立执行的 Rust 离线任务。Windows 本机运行、真实录音/账号/目标接收继续按实际证据分格记录。旧 verify.py 的源码摘要不包含 Cargo/crates，不能代表 Rust 验收或完整冻结。

### VP-010 freeze-01 本机离线独立验收通过

原独立会话 task_a6d21e4a5df1 / ctx_91078382b16b 已亲自完成验收，报告 `.voiceport/evidence/vp010-independent/report.json` 的限定范围接受字段为 true，未确认阻断缺陷。协调者核对实际回执、attempt、65 文件/21 目录的字节/模式/snapshot/完整集合、890 历史普通证据、2 个历史链接、4 个特殊文件、25 Swift/5 Rust 受保护输入，以及 7 个真实工作区命令、11 项命令日志审计和 122 个证据摘要一致，写入 `.voiceport/evidence/vp010-coordinator-acceptance.json` 后真实 finish。验收绑定原 snapshot；此后五份产品文档只更新结论及父端接入约定，不把新文档字节冒充原冻结。

41 项既有 Rust 检查、fmt/clippy/release 全部通过；另有 6 项独立 Rust 检查、46 个有效断言的真实子进程用例（23 个未修改 release、23 个单独合成服务入口）符合预期。包含全局身份并发及耗尽、精确输入/输出边界、确定性控制优先、取消/替换/退出迟到拒绝、逐请求重读、请求预算、真实 SIGINT/EPIPE/背压。各组有重叠，不相加为单一覆盖总数。

未修改 release 完全不读 stdout 时约 5.023 秒退出 1；连续小帧慢读可超过 8 秒存活，停读后约 4.847 秒退出。原夹具错误地给整段流设 8 秒时限，runner 退出 1，失败代码/日志原样保留；以原读取负载作正对照，再以单个 1.9MB 帧持续慢读验证每帧总 deadline，未修改生产行为。大帧中断留下 67,104 字节未终止尾片段，未当作 result；VP-011 父端必须在 EOF 丢弃。真实云端、原生接入、Windows 和整包交付仍未验。

以下保留开发交付范围，不与独立结果混同。

开发报告 `.voiceport/evidence/vp010-developer/report.json` 绑定当前 attempt 2c6c5e81-6e88-4feb-a73f-e602dddf164a 及实际 Task/Dispatch。协调者核对 19 个输入与开发 snapshot、30 个受保护输入、实际 10 文件变更范围及保存的日志一致，证据 `.voiceport/evidence/vp010-coordinator-delivery-check.json`。未重复运行这批开发自检；它们不构成独立接受。

| VP-010 开发自检 | 实际结果 |
|---|---|
| cargo test --workspace --all-targets --locked | 41 项通过：32 core、2 CLI unit、1 host integration、6 既有 CLI integration；0 失败 |
| fmt / strict clippy / release build / protocol_fixture build | 均退出 0 |
| 实际子进程反例 | 13 组通过：启动懒加载、严格输入/恢复、文件保留、替换/转义、重试、显式与服务取消、EOF、输出上限、慢读、EPIPE 和请求预算 |
| 旧 release CLI 用例 | 7 项退出码及 stdout/stderr 符合预期；与前面用例有重叠，不相加为独立覆盖总数 |

真实 release host 检查使用合成文件，含 FIFO auth 证明启动/status/shutdown 不读取登录。成功文字、慢服务及大输出通过单独 Cargo example 注入，复用原生产 host/session/parser/writer；其输出 deadline 为 150ms，生产固定 5 秒，无生产 mock/endpoint/timeout 开关。协议与资源上限见 [HOST_PROTOCOL.md](HOST_PROTOCOL.md)。原生进程接入和 Windows 仍未验证。

首次 clippy 的 while_let_loop 失败与跨引擎 session 身份复用失败均退出 101，日志原样保留。后者曾让同进程两个 engine 都产生 generation=1，现改用进程级 checked AtomicU64，最终回归通过。原独立验证会话随后针对新完整冻结自行构造反例并亲自运行最终 Cargo/真实进程验证，已经通过，没有沿用 VP-009 接受结论。Swift 录音、投递和已接受 Rust auth/audio/client/error 字节未变；本轮不代替 VP-011 接入或 VP-012 整包验收。

### VP-011 原生接入开发交付，独立验收待完成

开发 task_0d96cc9926a4 / ctx_e032746c68b8 的真实完成回执 msg_ea80b1edbd9f，绑定 attempt 8d12916b-8059-4f9a-8758-08db0b6d2c1a。实际默认控制器已走 RustBridge，正常打包安装并签名 `Contents/Helpers/voiceport-rs`；这属于开发交付，不能用 VP-010 接受结论完成 VP-011。

协调者读取八项改动与实际日志，核对 30 个当前输入、八个改动文件的 snapshot、54 个受保护输入与实际八项范围一致，产品集合为 68 文件。证据 `.voiceport/evidence/vp011-coordinator-delivery-check.json`。原开发快照只保存改动文件，另有生成的 pycache；完整集成冻结会独立保存全部产品字节。协调核对器三次失败来自错误假设快照覆盖全部输入、未排除 pycache、把 release metadata 内未参与构建的后续测试改动当成产物漂移，准确记录于 `.voiceport/evidence/vp011-coordinator-check-errors.json`，最终核对退出 0；未修改产品或重复已委派自检。

| VP-011 开发自检 | 实际结果与范围 |
|---|---|
| bash scripts/test_core.sh | 31 核心检查通过；对应十项核心源/测试字节未变；metadata 中较早的应用输入不代表最终应用版本 |
| bash scripts/test_app.sh | 最终 69 应用检查、0 失败；全部 30 个非生成记录输入匹配当前文件，既有 60 个注册保留 |
| swift build -c release | 退出 0；生产源码匹配，之后只有三项测试/测试脚本更新 |
| bash scripts/package_app.sh | 退出 0；生产源码与打包脚本匹配，实际 plist、helper 与外层签名检查通过 |
| Swift bridge 与真实 helper | 使用合成缺失登录，在联网前失败并手动 retry；预算耗尽与重启；另以原始管道核对 ready/status/shutdown、0755 与签名，退出 0 |
| 独立测试 child | 分片 UTF-8/原文、错误/尾片段/超限/错误关联、ready/读写/terminal 超时、取消与 SIGTERM/SIGKILL、录音 lease 次序等开发反例；其成功不代表真实云端成功 |

两次早期失败原样保留：`initial-app.log` 缺少异步 Bool 方法 return，退出 1；`app-2.log` 正常分片夹具超过 300ms，66 项中一项 timedOut，退出 1。普通成功夹具改用 2 秒，负面超时夹具保留 300ms；最终增加预算、默认接线、真实存储次序注册，69 项通过。各组覆盖有重叠，不相加为独立覆盖总数。

原生时限分别为 ready/每次写/每次控制回复各 5 秒，accepted 后 terminal 等待另为 130 秒；启动控制最多另加 15 秒，异常取消和 shutdown 再增加控制阶段及 0.05+2+2 秒退出观察。不得宣称 130 秒覆盖整个请求。无法确认停止时保留录音占用，单次同步系统调用不可中断、不可杀死进程和 PID 复用未实证。真实 GUI 活跃转写退出、麦克风/云端/目标接收、Windows 仍未验证；原独立会话须自行构造 child/协议/控制器/lease/退出反例并亲自执行冻结版本最终检查。完整离线整包、shipping 依赖许可与 smoke 另属 VP-012。

### VP-011 freeze-01 独立验收退回：VP011-I-001

原验证 task_2371c30faa69 / ctx_0ba1b8e7163d 的真实回执 msg_ea193654417e 为 failed，报告 `.voiceport/evidence/vp011-independent/report.json` 的 `product_accepted_for_local_offline_VP011_scope=false`。协调者核对身份/attempt、68 文件/22 目录、1309 历史普通证据/2 链接/6 特殊文件、50 保护输入和 139 份独立材料一致，并核对原始 child 帧、复制原生产字节与失败断言，记录 `.voiceport/evidence/vp011-coordinator-rejection-check.json`。旧 freeze-01 永久保留为退回快照，不再作为当前待接受版本。

确认 P2 阻断缺陷 VP011-I-001，位置 RustBridge.swift accepted 身份处理：同一活 child 接连返回 accepted/result(n1,s1)、accepted/result(n2,s1)，request_id 均正确；bridge 仅校验格式，第二段成功返回第一会话的旧文字。进一步以原 DictationController/AudioRecorder/RecordingStorage、真实私有文件与 flock 复现：两次录音路径不同，第二次仍进入 ready，第二次 TextDelivery 调用 automatic=true，第二段音频已删且 canRetry=false。编码器和投递为替身，无真实录音/按键/目标输入。冻结生产 Rust 自己仍生成递增身份，本缺陷是父端对异常协议身份缺少拒绝，不能声称实际 Rust 已自然复用。

原 bridge 与原控制器各一份断言程序预期退出 0、实际退出 1，属于同一产品缺陷；正常同 child s1→s2 两组正控制通过，尾部换行身份被正确拒绝。五个真实 Foundation child 均确认退出 0。独立亲自执行 31 核心/69 应用、release、从隔离 cwd 正常打包及 helper/外层签名，八命令全部退出 0；这些通过不能覆盖身份反例失败。Cargo release 在拥有范围内复制重签后的字节与安装 helper 一致，明确签名导致摘要变化。

独立完整性报告器初次因循环变量覆盖而只把 manifest_sha256 报错，固定摘要断言和材料核对原本通过；原脚本/报告/patch 保留，仅修正字段来源并全量重核，材料行与原测试前记录一致。协调者查看报告索引时曾对混合标量组错误遍历而 TypeError 退出 1，未执行产品测试、未改产品；最终完整核对退出 0。

旧 attempt 8d12916b-8059-4f9a-8758-08db0b6d2c1a 已按缺陷真实 retry，新 attempt 6adda6b1-822d-4516-8b98-87a0ee483656 running，原开发返修。需新增同 child 新录音/失败重试的重复/回退/溢出身份拒绝、正常推进与新 child s1 正控制、实际音频保留和禁错误投递回归；修复后新冻结，交原验证 ctx_0ba1b8e7163d。尚未完成的独立精确帧/输入上限/stderr 压力、失败重试/预算/重启迟到、完整取消/shutdown/unsafe-stop lease/时限矩阵仍需复核，不能只修一项就沿用全套接受。VP-012 尚未启动，真实 GUI/cloud/Windows 继续未验。

### VP011-I-001 返修开发交付，待新冻结独立复核

原开发 task_1a186c494e91 / ctx_b6f9b5a41516 的完成回执 msg_11317a25fadc，绑定新 attempt 6adda6b1-822d-4516-8b98-87a0ee483656。实际仅改 RustBridge、RustBridgeTests、合成 child 并新增 IdentityRegressionTests；控制器、深层存储/投递、Rust 与脚本均未改。协调者读取源码和实际日志，核对完整 69 当前输入、4 修改 snapshot、65 保护输入及范围一致，证据 `.voiceport/evidence/vp011-revision-01-coordinator-check.json`。

修复为同 child 有界 UInt64 高水位、完整 ASCII canonical 解析与严格推进；accepted 即占用、失败仍消耗身份，只在新建 child/确认回收时重置。新增四组注册覆盖跳号/UInt64 上界正控制、重复/回退/非法/溢出、failed retry 身份、原控制器/真实存储保留第二音频且无第二投递，再显式换 child 手动重试不自动投递。开发复制原独立 Swift 断言和 child 字节未改，原生产字节修前匹配旧 snapshot、修后匹配当前：bridge/控制器的 reuse 两项均从退出 1 转为 0，正常推进和坏尾换行相邻正控制保持通过；十份修前/后 child 记录均确认退出 0。这些是开发自检，不能改称独立复核。

31 核心/73 应用、Swift release、正常 package 均退出 0；最终 app metadata 全 69 输入匹配，core/release/package 后只有测试 child 一项修订，相关生产输入保持。首次应用回归 73 项中 failed retry 一项 protocolFailure、退出 1：夹具 accepted 使用自定义 s8，却错误发 error(session s1)，修为同 sid，保存 fixture-before-correction.py 和全部失败日志，负面断言未削弱。新打包 helper 的原始 ready/status/shutdown、0755、helper/外层严格签名及 bridge 合成缺失登录/手动 retry 均通过，签名副本与安装字节对应；未真实联网。

下一步完整 freeze-02 后回原验证 ctx_0ba1b8e7163d；必须复核 VP011-I-001，并继续前报告剩余精确帧/EOF/输入边界/stderr、错误重试/预算/重启迟到、取消/shutdown/unsafe-stop lease/时限矩阵。73 项自检通过不等于整轮接受，130 秒计时语义和真实 GUI/cloud/Windows/VP-012 未验边界保持。

### VP-011 freeze-02 本机离线独立验收通过

以上 VP-011 首轮交付、退回与返修段落保留各阶段历史状态。当前结论以本节为准：原独立 task_dde77a48fb69 / ctx_5281a4244276 的回执 msg_234d6f2a17a1 为 succeeded，产品 attempt 6adda6b1-822d-4516-8b98-87a0ee483656，报告 `.voiceport/evidence/vp011-independent-02/report.json` 明确接受本轮本机离线范围。协调者核对实际身份、完整冻结、原始结果和材料后已真实 finish VP-011，再 start VP-012（99a0852e-5d5b-4c3e-a3c5-d0b6482751d5），未以自检代替独立接受。

freeze-02 含69文件/22目录，manifest SHA256 `948e3fc3b0268f5b575cc855be57f42587a52ebdc976ab2abdb6ecfc6ad9f17d`，content SHA256 `6c9f4cdbb00eb16305fffd35042bfd017ff387a1d22bcd4b294f170207d555f9`。live/snapshot bytes/hash/文件模式、完整git cached+untracked集合、context/patch、61保护输入和1690旧普通证据/2链接/6特殊文件前后一致。1362项新普通证据索引逐项核对，索引及最终报告不自包含循环摘要。协调接受记录 `.voiceport/evidence/vp011-coordinator-acceptance-02.json`；未重跑委派测试。后续状态文档更新属于 VP-012，接受快照永久保留。

独立实际执行：原缺陷的 bridge/controller反例与正控制5项，补齐后的79项矩阵、额外bridge7项、含原控制对照的原控制器/真实存储13项、直接安装helper3项，以及真实工作区31核心/73应用、release/正常外部cwd打包/签名等八命令。各组重叠，不合计为唯一总数。精确2MB输出及16KB输入边界、LF/CRLF/无LF EOF、类型/关联/迟到、failed retry/语言/预算及无自动replay、手动重启禁自动投递均有实际child证据。原重复s1反例现在failed、无第二次投递、第二音频与lease保留；显式手动重试后沿用原音频/语言并关闭自动投递。

真实隔离音频与flock证明取消/shutdown时child仍读文件且忽略TERM，保留音频并禁止start/retry，确认exit9后才清理；MainActor仍可运行。unsafeStop/stop=false用披露的NativeTranscription包装器围绕实际bridge child诱发，仅证明控制器保留/禁用路径，不声称存在不可杀死内核进程。生产ready和每写5秒边界实际执行；130秒terminal用于64MiB stderr负载，未完整等待无输出130秒到期。64MiB stderr最终54.639秒通过，4121次拥有进程采样RSS最高9,093,120字节；40次child重启均确认exit0、最终fd为3，有限采样不等于通用内存/描述符定理。130秒仍非端到端上限，控制及回收另计。

安装helper0755，cargo原产物与签名安装产物分开记录；自有同名副本按正常命令重签后匹配安装字节，helper/外层严格签名通过，raw ready/status/shutdown和两个实际工作预算路径退出0。合成缺失/坏auth在网络前失败；未真实读取登录或调用ChatGPT，不声称捕获exec syscall。

四次独立早期失败均保留：64MiB负载超过内部缩短20秒后，以生产130秒重跑同负载/断言通过；0.3秒控制握手首轮失败且报告器空列表IndexError（精确原生原因未捕获），后以披露的2秒控制限及相同断言通过；报告器跨child文件枚举顺序误判，改按incarnation/事件时间核对；测试Swift函数类型空白导致编译失败，仅修语法后通过。协调核对器另有两次退出1：误要求所有命令含可选expected_exit_code、误把替换后新child最终exit0当作旧child应有exit9；修正核对假设并保留脚本/错误材料，最终完整核对退出0。没有修改生产或独立历史材料。

本节不接受活跃普通AppDelegate GUI退出、真实录音/账号/云端/TLS上传/目标实际接收、Windows与同音频A/B；VP-012完整开发包、shipping适用许可与safe smoke仍需自身冻结和独立验收。未自动录音、读取真实凭据、访问一般剪贴板、发真实按键或清理用户真实缓存。

### VP-012 开发包与完整离线入口交付，待独立验收

原开发 task_cdc471996bd7 / ctx_9157b3116e64 回执 msg_9b3f9e899e0c succeeded，绑定attempt 99a0852e-5d5b-4c3e-a3c5-d0b6482751d5，源码已停止。报告 `.voiceport/evidence/vp012-developer/report.json` 记录4个既有修改和641个新文件；645份改动snapshot不是完整冻结。协调者实际读取全部本轮实现/说明/测试并核对710当前输入、64保护输入字节/模式、全部修改/新增快照和历史材料，记录 `.voiceport/evidence/vp012-coordinator-delivery-check.json`；未重跑委派测试。

最终 `python3 -I scripts/verify.py --app-smoke` 退出0，报告 `.voiceport/evidence/vp012-verify-d94e8ba29e0144ff8d80e679f42a0a71/report.json`。41 Rust、31核心、73应用、4 loop、7 packaging及25步实际命令符合预期，包括正常双release包、外部cwd/CARGO_TARGET_DIR、两种CLI预期负面退出、签名/当前Cargo产物对应、真实bundle helper ready/status/shutdown和safe菜单smoke。起止710项完整清单一致；签名前Cargo和安装helper字节分开记录。

随包材料含120个native metadata图crate和635份材料，runtime/build/dev与排除平台lock条目分别记录；协调逐文件比对缓存源和包内副本，bundle共641普通文件。覆盖/选择/内嵌组件充分性和缺失/篡改拒绝仍须独立反例，不把开发表或字节一致当法律结论。

首轮包装测试删除必需Cargo.toml后清单抛FileNotFoundError、退出1，修为显式missing并保留失败日志；原临时输入目录已由unittest清理，配方/异常保留，不声称原目录仍在。首次完整验证cargo test退出101，运行了当前受保护源中不存在的independent_generation用例并有三项失败；受保护源摘要匹配启动，`cargo clean -p voiceport-core`退出0后同一产品摘要完整重建通过。记录旧产物现象和处理，不推断已实证具体缓存/时间戳成因。独立验收将使用全新自有Cargo target，保留这些失败及其日志。

以上保留首次交付历史；其后完整freeze-01已独立退回，当前结论见下面返修段。当时VP-012尚未接受或finish；后续freeze-02接受见下文。Windows、真实账号/录音/云端/目标接收/A-B、活跃普通GUI退出、公证和外发/法律批准仍未验。

### VP-012 freeze-01 拒绝及返修01交付，待完整新冻结复核

原独立task_57f21cea2e5e / ctx_88ca56def413回执msg_d687cea5d770 failed，确认VP012-I-001根docs目录链接与普通目录清单相同、VP012-I-002签名包以实际Swift CLI替换主程序仍被接受、VP012-I-003未知声明/选择和删atomic-waker整包两例仍被接受。常规25步与safe smoke成功保留，不能掩盖四负例失败。710文件/265目录、3884历史普通材料/2链接/6特殊和61保护一致，7989新证据索引及实际日志经协调核对后真实retry/start；独立许可充分性与部分入口漂移组合尚未完成。

协调freeze-01错误地用path字符串排序的三字段行重建四字段摘要，而原公共inventory使用sorted(Path)分段顺序。710行值和模式一致，索引382首差，真实摘要2c440f92…，固定旧声明c44d8dfe…错误；原manifest/脚本/失败保留。未执行元数据单独更正草稿；后续完整冻结须直接用当前实际公共接口，并将schema2目录模式与独立三字段freeze content区分。

原开发返修task_ac869bb39540 / ctx_0f12ce81048b回执msg_1b5f33ab3169 succeeded，绑定新attempt ea6b723e-598a-4a2f-b4de-54aceffb68c4。8既有修改和52新增，无删除；完整762文件/270目录，65保护字节/模式、12656旧普通材料/3链接/6特殊不变，17377新普通证据/5合成链接已核对。原四反例在正确正控制下修前负面runner退出1，修后12项含六signed破坏对照全部通过。实际入口初始根链接exit1且不执行步骤，受控cargo进程改自有README后0/7，原verify观察inputs_unchanged=false并exit1；这是显式测试seam，不是正常构建通过。

最终自有含空格外部cwd、全新空Cargo target运行实际入口25步退出0：41 Rust、31core、73app、4loop、11packaging。主程序与Rust helper分别保留原产物/安装签名后同时字节副本，checker验证双产物对应；bundle共693普通文件，host和safe smoke符合预期。120依赖687材料的缓存源、Cargo.lock校验的公开crate归档及包内全文逐份对应，原635材料保留、新增52归属文件；分类/feature/选择与内嵌条款的开发审阅记录存在，不能据此跳过原独立的充分性判断。

首次返修before夹具误用旧cargo产物，正控制因helper不匹配失败，后以记录的真实产物重现四例；两次许可audit假设可选.cargo-checksum.json必有而FileNotFoundError，最后直接校验锁定.crate归档及全文通过。协调拒绝核对也曾把签名前历史摘要当签名后当前文件，原失败保留并按时点修正；本轮返修交付核对首次退出0，未改产品来做测试或重复执行委派验证。记录见`.voiceport/evidence/vp012-revision-01/report.json`、`license-review.md`及`.voiceport/evidence/vp012-revision-01-coordinator-check.json`。

本轮已停止源码写入，协调集成四份文档后建立完整新冻结，回原独立ctx_88ca56def413亲自复核三类四反例、剩余许可充分性和入口/目录漂移范围，并运行全新Cargo target完整入口。开发自检和材料核对不是接受；真实环境、Windows、活跃普通GUI及外发/法律边界保持。

### VP-012 freeze-02 本机完整离线独立验收通过

以上首次交付、拒绝和返修段落保留历史阶段。当前结论以本节为准：原独立 task_19a4b7f33eb4 / ctx_ec58a916167a 回执 msg_2fa94af1de04 succeeded，报告 `.voiceport/evidence/vp012-independent-02/report.json` 明确接受全部本轮本机离线范围，mandatory scope complete且无新阻断或未完成必选项。协调者核对后已真实finish attempt ea6b723e-598a-4a2f-b4de-54aceffb68c4，记录 `.voiceport/evidence/vp012-coordinator-acceptance-02.json`，再start VP-013，未重跑已委派测试。

完整freeze-02为762文件/270目录及模式，manifest `7638b1f363f0e868723478fbfff84d8dc1d5455ba4f7a517e4e986c943c312a0`，三字段content `36dbfde28ff5bc1f559ad219ecfea7cb1d9eb90a722efeeed8f75dd16e9d43eb`，实际schema2公共摘要 `c36d9683666d5ecfb4acea0a4dcd955dc74d5d39e97bb25f077ceb0eab417611`。live与完整snapshot的原公共接口逐项相等，62保护输入及30020历史普通材料/8链接/6特殊一致。23554新普通证据共417374403字节、2链接，索引 `f2dd93b76e4d874aea60455109cc36d1b3342aface1926582d040cf9a0cf718f`；协调逐文件及集合核对通过。接受报告SHA256为 `cdb7d5578035673f4349f36d17645532baed316760422037574465599d42e984`。本次验收后四文档状态更新另记，原快照保持不变。

原三类四例已独立确认修复，含23项inventory、6项初始/运行漂移、20个成功重签且deep strict验证的变异包、3个错误产物路径、缺第三参数及含空格路径控制、24项真实copied-source许可反例。入口漂移的独立自有工具seam中，正控制和三种漂移均先完成25个成功步骤，只有指纹变化使最终失败；初始清单错误零工具调用。该seam仅证明指纹边界，不当作真实构建证据。

独立另以全新空Cargo target实际运行25步原入口、6个预期CLI非零退出，41 Rust/31核心/73应用/4loop/11packaging符合预期，无旧independent_generation marker。693文件正常包及两种原始/签名后产物来源分别对应；实际helper ready/status/shutdown与safe菜单smoke通过。真实skip-build验证双产物缺失拒绝、外部cwd/空格及实际metadata target；正常校验不生成资源。未声称Swift完全clean构建。

许可充分性已补齐：独立校验120份锁定公开归档与687份完整材料，原635份保留、52补充；全部选择表达式及完整条款、ring247份内嵌分配/归属、Unicode/CDLA、完整MPL原归档8文件及额外Cargo bookkeeping、原GPT Voice告知均审阅。实际release normal/build树103包、normal边100包、workspace含dev树107包；metadata120含13个未在实际workspace树启用的可选节点，另28项other-platform/lock-only分别记录，不能把保守图角色写成实际链接清单。没有法律批准或外发授权。

独立首次测试parser缺WITH/legacy slash支持，修正后完整重跑通过；非材料once_cell路径查找和list/dict诊断错误分别保留。协调核对首次把workspace-relative索引路径按scope-relative拼接而FileNotFoundError，原脚本/错误保存在 `vp012-coordinator-acceptance-check-failure-01`，只修协调路径解析后核对退出0。未改产品或独立材料、未把失败当通过。

Windows构建/运行、真实录音/账号/云端/目标接收/A-B、活跃普通GUI退出、Intel/universal及公证仍未验。VP-013于本轮开始；只读环境盘点确认当前Darwin arm64、仅安装aarch64-apple-darwin Rust target、所查常见Windows工具/VM应用未发现、Orca无已连接远端或已配对environment；这不证明用户没有其他Windows设备。系统适配可推进的工作与Windows实际运行证据分别保留，不能以交叉检查完成任务。

### VP-013 首次部分交付，继续开发，尚未独立接受

原开发task_ca5492581a44 / ctx_2334a32d8f09回执msg_c93126c789b7 succeeded，明确 `product_accepted_for_VP013_scope=false`；这里只交付可核验进展，不finish产品。4改动/10新增，772完整文件/275目录、756保护输入、54441历史普通材料/10链接/6特殊与132新证据逐项一致，协调核对 `.voiceport/evidence/vp013-coordinator-delivery-check.json` 首次退出0。原报告 `.voiceport/evidence/vp013-developer/report.json` SHA256 `5b75b5e6746f56f25a0f1c480a5a986c43b685a74ccd356f0491c27c8391ecf7`；14份改动snapshot不是完整冻结。

新增控制器用不可克隆work ticket持有录音及取消令牌，取消先失效，旧ticket未释放时禁新工作；手动retry保留语言且禁自动投递。WAV仅为16kHz/mono/16bit、最多180秒PCM编码，不是设备采集。唯一原生功能为线程归属RegisterHotKey/UnregisterHotKey原语，尚无消息循环、托盘或实际快捷键运行。诊断CLI仅help/smoke，无普通录音入口。直接核心调用仅在显式Job.transcribe时发生，测试使用缺失合成auth与预取消；不宣称真实云端成功或文件ACL/占用已实现。

官方rustup安装x86_64-pc-windows-gnu成功，实际adapter-only no-default-features目标类型检查退出0，**完整默认功能检查因ring找不到x86_64-w64-mingw32-gcc退出101**。未链接Windows可执行文件，也未在Windows运行。最终新空自有Cargo target实际15步通过：41既有Rust与7适配测试、fmt/clippy、adapter-only类型检查、正常Mac双release包/签名、31core/73app/4loop/11packaging、诊断CLI/Python WAV及safe smoke，含预期参数错误退出2。它没有替代原25步整包入口或Windows本机验收。

687份原许可材料字节未变、现有锁定依赖包项均未改，仅新增本项目Windows package。原generator重建manifest除Cargo输入/lock摘要外，bytes/mio/pin-project-lite/signal-hook-registry/socket2/tokio六项scope从runtime变为dev+runtime，来自新crate的dev依赖路径。开发报告中“仅更新输入摘要”的表述不完整，此处及协调核对明确补正，原报告不覆写；未改变这些材料或许可选择。Windows实际依赖/分发许可仍未验。

首份交付结束时尚未冻结或分派独立验收。缺少C编译器是可继续调查的工具条件，不能直接视为外部硬阻塞：协调已只读取得官方Zig稳定版本机工具包索引及摘要，尚未下载或执行编译器，也未证明它适配本项目。后续原开发在自有目录尝试完整目标编译/链接并推进能核验的原生功能，保留每次失败。设备采集、原生存储/ACL/共享/reparse、托盘/交互、手动复制、UIA/剪贴板投递、GUI退出和实际Windows运行仍为必需剩余项；自动投递和清理保持禁用直到边界可验证。

### VP-013 续作02：完整Windows目标检查通过，后续独立退回见下节

原开发task_cca0083a8a38 / ctx_c168c4612f21已由msg_ee51019e1f55交付并停止写入。协调核对 `.voiceport/evidence/vp013-coordinator-delivery-check-02.json` 首次退出0，最新报告SHA256 `088096b8f35432ea4ad98def2106bd3a63db612b53ad85aedecf093f3cacaa04`；775文件/275目录、6改动/3新增及原字节快照、756保护、54581历史普通材料/10链接/6特殊与161新证据一致。此项是交付材料一致性核对，`product_accepted_for_VP013_scope=false`。

官方Zig0.17.1归档54001240字节及广告SHA256、20792个成员的规范路径/包含关系和提取编译器原字节已核实。初始Python lzma/filter兼容失败、CC目标规范化、dlltool探测和实际link失败均保留。实际ring C对象是AMD64 COFF；包含原生壳、默认共享service及测试的 `cargo check -p voiceport-windows --all-targets --target x86_64-pc-windows-gnu --locked` 退出0。实际build先因dlltool缺失退出101，配置Zig真实工具后仍缺libpthread.a/msvcrt运行库而退出101。没有成功PE产物或Windows执行，不能从类型检查外推链接/运行。

显式--run已实现隐藏窗口、消息循环、托盘、线程热键冲突/动作、当前结果手动复制及取消退出协调。Start/Stop和Retry因采集/安全存储/后台调度缺失而禁用；应用不产生录音或调度转写。WndProc仅保留有界合并动作位，不存Rust对象裸指针；原生FFI生命周期和错误路径仍未运行。合并动作中的Exit/Cancel优先复制已修正并以fake sink覆盖；不以消息来源证明可信人类身份。自动投递与清理禁用。

新自有目标Mac15步先通过41既有Rust+7controller/WAV+2初版actions、fmt/clippy、adapter-only检查、正常包/签名、31core/73app/4loop/11packaging、真实诊断CLI、Python WAV解码和安全smoke。其后priority修正补跑3最终actions+7controller/WAV、lint/fmt及最终完整Windows目标检查；不把较早Mac入口宣称为最后全部源码字节的重跑。相关Swift/核心/包入口未改，不机械重跑无关委派检查。687许可材料不变，本轮manifest仅build_inputs变化，首轮6项scope精度更正保留；不接受Windows分发许可完整性。

协调集成进展文档后，对全部当前输入建立完整冻结，交原独立角色审查现有控制器/WAV/actions/FFI与交叉编译证据、相关Mac回归。该阶段检查即使通过也不能finish VP-013；完整链接、真实Windows运行、设备采集、安全存储/held lease、后台服务执行器、保护投递与打包仍为必要后续工作。未访问真实账号、麦克风、一般剪贴板或目标输入。

### VP-013 freeze-01独立退回：取消后普通轮询不恢复准入

原独立task_eb01b66a93de / ctx_5e90436934ec由msg_8a8b0c7fa982 failed交付，报告 `.voiceport/evidence/vp013-independent-01/report.json` SHA256 `6577a75923a38d052a24e9de67008a5b4fae71fefb25851f01c64345021105f3`。协调核对 `.voiceport/evidence/vp013-coordinator-rejection-check-01.json` 首次执行退出0：775文件/275目录、752保护、54717旧普通证据/10链接/6特殊、1400新普通证据83361230字节及完整集合/模式与实际日志一致；index SHA256 `def841a405db2efec08cbe8a2c7d4bdc0ddcddef8a7572b270f572980b191ffd`。旧freeze-01和失败保留，不覆写为通过。

VP013-I-001确认：Actions Cancel时unique Job仍存活，初始stop未能确认；随后Job作为迟到完成被消费或直接drop，普通Poll及timer bit7即使重复八次仍不调用Controller::poll_stopped，保持Stopping/录音引用，admit_finalized返回Busy。Controller直接轮询与正常Actions完成正对照能恢复，额外Cancel可恢复，定位在actions.rs普通轮询分支。成功预期的独立Cargo测试实际101（10通过1失败），采集runner仍exit1；诊断程序exit0只证明它成功断言了缺陷，不能当产品通过。初版遗漏post-completion Poll的夹具源码和失败保留；加入三次显式Poll后同样失败。关闭路径及迟到文本/重复closed dispatch尾部另有成功执行对照。缺陷在冻结可移植公共Actions逻辑实证，未宣称当前尚无服务调度的Windows GUI已触发。

独立当前冻结版本的新空target Mac完整25步通过51 Rust/31core/73app/4loop/11packaging及六个预期CLI非零，693文件包/实际双产物签名对应、helper ready/status/shutdown和safe smoke通过。另一个新空target的完整默认Windows all-target检查通过，16个真实COFF C对象、非空rmeta及官方归档/提取字节核对成立，无PE或Windows运行。四个编译正反约束、五个独立Python WAV解析包含最大180秒全部样本通过。687材料/120包身份及当前generator/安装清单一致，已接受VP012条款充分性未重复；图分类不等于Windows实际链接或分发许可。以上通过不能抵消原缺陷。

协调已真实retry旧attempt4c66c1a3-58a4-46ec-930d-a007f19a4ace并start新attempt `0d21e578-dac8-46ff-aa22-6d19b30b68e1`，返原开发ctx_c168c4612f21修复普通取消停止推进，要求任务存活时仍阻塞、释放后普通轮询恢复且开放状态不误发退出true，保留全部迟到拒绝/手动复制/Exit语义。开发交付后必须新完整freeze，交原独立ctx_5e90436934ec复核原反例及相邻路径。VP-013保持未接受；链接/真实Windows运行、设备采集/安全存储/lease、后台执行器、保护投递和打包等后续必需范围不豁免。

### VP-013返修01开发自检通过，待新freeze-02原独立复核

原开发task_5ec41d525e67 / ctx_0912c821c9d4由msg_123d8c34ac00 succeeded交付，绑定attempt0d21e578-dac8-46ff-aa22-6d19b30b68e1，源码/证据停止写入。报告 `.voiceport/evidence/vp013-revision-01/report.json` SHA256 `234b53aee5f244bb4b39abd975670bb2f899660cfbf89d8a4efa7f57a825d21a`；协调 `.voiceport/evidence/vp013-revision-01-coordinator-check.json` 首次执行0，775文件/275目录、3允许改动/无新增删除、772保护及全部模式、56911历史普通/10链接/6特殊、398新证据与完整集合一致。索引SHA256 `d2ca9ce4f3e9b3b06633d7cc71e2bc1950119bbc78e056501c1f8953147121fd`。这仍是开发修复自检与材料一致性，不是独立接受。

Actions普通Poll现始终调用Controller::poll_stopped；unique Job存活时仍不确认停止且阻塞新准入，释放后推进Idle。非closing即使确认idle仍false，保留开放消息循环；只有closing确认才true。等待/恢复状态按实际stop更新，Ready结果及其状态不被普通timer清除。三份改动为actions.rs、既有actions测试及WINDOWS说明，未改依赖/共享核心/Swift/原生壳和687材料，未删录音或开启自动投递。

原独立修正后的11项成功断言字节完全相同，修前真实Cargo101（10过1失败）/collector1，修后Cargo0（11过0失败）/collector0，原关闭尾断言可达并通过。原Busy诊断保留，另存显式成功期待适配与patch；完成和drop两路均在普通轮询后、额外Cancel前成功准入。生产新增回归覆盖完成/drop、direct/timer、存活票据八次拒绝、恢复后八次不退出、Ready保留、迟到不复制及重复Cancel/Exit/Closed。最终新自有target工作区52 Rust、fmt、严格clippy及完整默认Windows all-target检查均0，最终源码字节与after断言及检查一致；本轮未重试链接或Windows运行。相关Swift/核心/材料由保护清单核对，未机械重复无关审计，最终冻结版本的Mac完整入口留给原独立亲自执行。

协调集成四份当前状态文档后，建立新的775文件完整freeze-02，交原独立ctx_5e90436934ec复核VP013-I-001及相邻状态/退出/复制路径、全冻结完整性和当前Mac完整入口。原freeze-01拒绝与失败永久保留；此部分checkpoint即使通过，也不能完成全部VP-013。Windows实际链接/运行和未实现平台功能仍是后续必需范围。

### VP-013 freeze-02原独立返修阶段检查通过，整体产品仍未接受

原独立task_42c9f403abd7 / ctx_d5ff27ba1a13由msg_e25452fc2195 succeeded交付，`partial_checkpoint_review_passed=true`、`product_accepted_for_VP013_scope=false`。报告 `.voiceport/evidence/vp013-independent-02/report.json` SHA256 `10c917be6086b13144a17e41652679937d902dade5d3be80d426097fd1153779`；协调 `.voiceport/evidence/vp013-coordinator-checkpoint-acceptance-02.json` 首次0，775文件/275目录、768保护、57313历史普通证据/10链接/6特殊及1234新材料74178041字节/307目录和完整集合/模式一致。manifest `8f79152a7a50aea9a956854886f731b74a186d7bb2cdce66f8c04a945b237647`、content `eb6e3a0a0d99cefab4d47a4c14033c3e6651d51cd81033d20ce3e62620627acd`、公共schema2 `e9b02da8320ea3915e3b1af4096e9b22be3d27eb1b612b4c8ef9d35c36874847`。

VP013-I-001已由原独立确认修复。原11项成功断言字节不变，亲自执行Cargo0/collector0并达到原关闭尾部；另有4种complete/drop与direct/timer组合，各8次活跃拒绝和8次释放恢复，不需额外Cancel。开放轮询不退出，Ready结果/错误提示保留，3个非空复制正控制下的Cancel/Exit优先级和2个closing释放对照通过。本机自建File描述符观察确认Rust引用保留/释放，65个合成文件仍在；不代表Windows原生ACL/lease。52工作区检查和独立11项有覆盖重叠，不相加为63。

原独立从新自有外部cwd和新空Cargo target实际完成当前Mac25步：52Rust、31core、73app、4loop、11packaging及6个预期非零CLI；693包文件、正确Swift/Cargo原产物到独立重签副本到installed对应、helper ready/status/shutdown与安全smoke通过，不声称clean Swift或普通活跃GUI退出。另一新空target/cache完整默认Windows all-target检查0，16个真实ring COFF对象和非空rmeta；没有新链接、PE或Windows运行。旧未改WAV解析/编译约束/FFI源码审查按精确来源沿用，当前原11fixture包含编码器断言且5个输出字节等于旧独立解析文件，不重复广泛无关审计。687材料/120身份和generator/源/安装/metadata图保持；不接受未来Windows分发材料。

独立auditor首次仍要求旧3项Actions子组而实际正确为4，退出1；仅该断言改为4，原脚本/失败/patch保留，完整Mac入口未重复，随后产物审阅0。协调核对不重跑委派测试。已归档freeze-02为不可变返修阶段接受快照，同一VP-013 attempt继续后台执行器、结果回收和显式手动重试；未finish产品。录音、原生安全存储/held lease/清理、保护投递、完整Windows链接/实机及打包仍必需；用户真实录音/账号/云端/目标交互另需明确开始。

### VP-013续作03后台执行器交付，待完整freeze-03原独立验收

原开发task_f23c2d7491eb / ctx_f235b23b58b7由msg_13b73ba1e907 succeeded交付，报告 `.voiceport/evidence/vp013-developer-03/report.json` SHA256 `612a5fd76fa2c2d1fb93337e7168f1d6b4888e5cdcf23d949fec8e7384de5132`。协调 `.voiceport/evidence/vp013-coordinator-delivery-check-03.json` 最终0：777输入/275目录、7既有修改2新增快照/原字节、764保护输入/所有既有模式、59371历史普通材料/10链接/6特殊及385新证据/24目录完整核对。原687材料、120包身份/scope/选择和Cargo.lock不变，manifest仅build_inputs变化；没有新Windows分发许可接受。

生产Executor使用单进程worker许可、唯一Job、单完成槽；实际线程运行共享Job::transcribe，消息线程不读auth/audio、不等待活线程或满channel/runtime。只有线程确已结束才关联结果和释放票据；Cancel/Exit先失效再回收，普通Poll推进停止且开放状态false，Ready结果/错误状态保留。显式手动Retry仅当前Failed、保留录音/路径/语言并发新手动票据，automatic=false。异常owner drop非阻塞移交唯一活线程/票据至有界进程owner，后续显式提交才尝试回收已完成线程；不能中断的同步调用仍保留引用并阻塞新worker。Start/Stop因录音和安全存储缺失继续禁用；caller File/path仍不是原生ACL/lease保证，未开启自动投递或删除音频。

真实线程门闩假服务矩阵涵盖活跃/排队取消、失败重试、线程尾部未结束的完成槽、spawn/runtime/panic/无完成、超大结果、异常drop/回收和16轮正常退出；该矩阵按一个Rust测试统计。原11成功断言字节保持、实际0，最终同源Mac完整25步通过53Rust/31core/73app/4loop/11packaging及fmt/严格clippy/正常包/签名/helper/safe smoke；help/smoke实际0、参数错误2。完整Windows默认all-target检查0，实际最终目标16COFF和有用rmeta，但没有PE/link/runtime成功。初期语法/API/import/lint和末轮cfg语法失败保留；首轮完整入口通过后help/cfg整理又跑最终入口，两份新verify原件及镜像来源明确，不混称同一源码时点，也不声称clean Swift。

协调首次将开发记录的2个COFF样本误要求为全部16，AssertionError退出1；原脚本/错误保存后，改为样本身份加实际最终目标16对象头与完整身份核对，最终0，未改产品或重跑委派测试。当前仅开发自检及材料核对完成，须新完整freeze-03交最新原独立角色审查生产executor并发/生命周期/原生接线和当前冻结回归。完整Windows录音/安全存储/lease/保护投递/链接/实机及打包仍为必需后续，VP-013整体未接受。

### VP-013 freeze-03独立拒绝：线程局部析构期间同步join阻塞

原独立task_0d946ca10728 / ctx_3d12c3678161由msg_e46657b5c540实际报告failed，报告`.voiceport/evidence/vp013-independent-03/report.json` SHA256 d1b1aa211c05323a1acdaab47b0708e476d3ff63a2111622c35c993d98b05f29。VP013-I-002已确认：真实OsSpawner工作返回后，线程局部析构发送入口确认并等待释放；此时is_finished可为true，生产Running::finish同步join仍等待。普通Poll、取消Poll及孤立任务回收提交三路在明确release前均未返回，耗时1003/1005/1007ms；成功预期0、实际1，collector1。正常当前结果/手动复制及释放后取消/孤立任务恢复正控制均通过。另一个std诊断exit0仅证明is_finished/join行为，不是产品通过。这里的TLS指线程局部存储，未实证Windows GUI或默认CoreService出现同一清理状态。

原独立在当前冻结新空target亲自运行Mac25步、53Rust/31core/73app/4loop/11packaging及完整默认Windows all-target检查，均符合预期；16真实ring COFF和949552字节有效rmeta、693文件包/双产物签名/helper/safe smoke、原11断言、12新executor检查及2生产API加1std诊断通过。通过项不抵消三路行为失败，重复覆盖不相加。687材料和120包行/分类保持，manifest仅build_inputs；native与Windows实际树和各自metadata分别核对，不能视为链接清单。

协调check-vp013-rejection-03.py最终0，完整777文件/275目录、59870历史普通材料含隐藏pycache/10链接/6特殊、760保护及1489新普通材料/83348061字节/323目录核对一致，index SHA25676a37bbe82162572ea011d58eda14832e6aa31442eebca1a03c8e22042635982。协调wrapper嵌套字段和材料容器两次schema错误的原脚本/exit1分别保留，再按真实schema核对；无产品修改/委派测试重跑。独立pin生成及两次平台依赖集合假设错误也保留原失败，仅校正验证脚本后通过，不改材料断言或产品。

freeze-03永久保留为rejected executor checkpoint。retry-vp013-02.py已真实retry旧attempt并start新1ed6204e-5637-4c5e-b059-51ac4256a4e6，仅为聚焦修复消息线程等待而解冻；原开发返修后需新完整冻结并交原独立。票据/File/许可必须保持到实际stop，结果槽、取消或时间流逝均不能冒充停止。全Windows链接/PE/运行及录音/安全存储/lease/保护投递/打包仍必需，未自动执行真实录音/账号/云端/目标操作。

### VP013-I-002返修02开发交付，待新完整冻结原独立复核

原开发task_aa40d6e71190 / ctx_e86b9f735149已由msg_df14f5209718 succeeded交付并停写，attempt1ed6204e-5637-4c5e-b059-51ac4256a4e6，报告`.voiceport/evidence/vp013-revision-02/report.json` SHA2565ea45c19dfbdd822a612bb69739f2b87d8ec8ce22e0f575efc5a7c4cc7d098d8。现将worker启动与唯一join交给单一进程级清理线程；UI Poll/submit/retry/reap/Exit/Drop不join，停止确认在真实join返回之后发布，唯一Job/File/进程许可在此之前保持。初始化失败启动零worker，线程发布成功后panic转静态失败，闲置清理线程随进程保留；无强制停止或音频删除，异常基础设施丢失仍保守持有。

原before探针三路阻塞，实际oracle1；初始记录器自身报告0的精度已披露，另一个完整拒绝快照副本亲自重复oracle1/collector1。修后原样probe仍101：普通及取消响应已0ms，孤立任务提交及时返回后进入保留Failed，原恢复尾假定马上成功而panic。该失败保留，不宣称未改夹具全过。单独适配只将孤立任务恢复尾的pump改为显式eligible Retry/Poll，原三处acknowledgement/响应断言与正控制完全保持，三路均release前响应、elapsed0ms、退出0；原11断言字节相同并退出0，关闭尾执行。此适配符合当前手动重试契约，是否完整覆盖新清理线程边界仍留原独立亲自判断。

最终新target工作区54项Rust、fmt、严格clippy及完整默认Windows all-target检查均0。真实TLS生产回归新增Exit与合成Unix FD保持；原executor矩阵增加两次冷启动清理线程失败和发布后worker panic。首次两次clippy101、join-error-before101、第三轮helper路径替换错误1均保留，第四轮实际全部0；此窄返修未重跑Mac完整入口/广泛法律审计，原独立须对新冻结亲自执行。16真实Windows COFF与960961字节有效rmeta实证存在，开发artifact记录仅2个Windows对象样本，76总artifact还包含本机对象，不能把全部当COFF。无新link/PE/Windows运行。

协调check-vp013-revision-02.py首次实际0核对778输入/275目录、3修改1新增/原字节快照/772保护及全部既有模式、62159历史普通材料含隐藏pycache/10链接/6特殊、1258新证据20840524字节/313目录。index SHA256dc6bf78832ea715dee63115f4dd1430ce94a62f093e1b903b7de0f939d45f8d4，全部report纳入索引，仅自身排除；完整历史与原反例/适配patch/实际日志和最终16对象身份核对一致。开发公共摘要57b889b5c2ce735025b2ed8f66fbd1ffc4ab358d65fddaced1d3c52c48141a43，协调本次四文档集成后另建完整冻结，不把两种输入时点混称。此为开发自检与材料核对，非独立接受；完整VP-013仍未完成。

### VP013-I-003返修03开发交付，待原独立复核

freeze-04的独立失败保持：合法清理Spawner的实际线程在TLS析构内执行所给循环，未领取请求仍可被领取，is_finished不能证明请求已释放；Poll/Exit/orphan三例误判停止并panic。原开发返修03仅修改executor并新增cleanup_tls聚焦回归：请求与UI显式共同持有Job/进程许可，真实请求销毁或后台OS join后才释放并发布停止确认；清理健康状态按真实receiver生命周期判断。UI丢失后清理侧保留资源至真实停止并自行释放，仍保持单worker/单进程清理线程/单请求及结果槽。

原样cleanup TLS探针修前control0/poll1/exit1/orphan1，最终修后四例均0；没有压低成功断言、抑制panic或改恢复尾。新增测试另检同一TLS清理循环的健康后续请求及冷启动丢失循环，既有service-worker TLS/FD及显式Retry回归保持。原11断言和最终55 workspace Rust、fmt、严格clippy、完整默认Windows all-target检查均为开发自检0；没有新增链接/PE/实机证据，也未重复广泛Mac/许可审计。

协调材料核对779输入/275目录、777原有保护输入、273条新普通证据6581392字节及79项产物身份一致；报告SHA25600ffc3287837b253e3a25adef623f4a8664b6aef7b99b62e5a0acaeed213b6e7，index153e404e3519dfd9b8921fb6c7cb561951d65639fe9539d580fa5adb2276af52。首次协调核对把schema2库存摘要误当文件序列化hash而exit1，原脚本/错误保留；纠正字段后exit0，未改产品或重跑开发测试。新完整冻结和原独立复核仍必需，VP-013整体未接受。

### freeze-05原独立返修阶段接受（全Windows产品未接受）

原独立已确认VP013-I-003修复且未发现新已实现范围阻断。原样清理TLS四例亲自执行均0，六组新所有权/健康后续/丢失循环/自动释放控制通过；早前service TLS、原11及相邻行为保持。当前Mac25步入口包含55Rust/31core/73app/4loop/11packaging及签名/693包文件/helper/safe smoke；完整默认Windows all-target检查0，16真实ring COFF与961938字节有效rmeta。原service-TLS未经修改仍101/collector1，其恢复尾显式manual Retry适配另为0，原失败保持。

协调核对779公共+5操作输入、275+2目录模式、771保护、66261历史普通/10链接/6特殊及1742新证据107932774字节后接受本次返修阶段。报告SHA256e57897d7afa8123cf871028f02e0ddf2349b314bdb96836d0cc8147d27480f50，index b99e2331fc41c9b863d9e79ea6a80e106ccd04fae338a199d53018bf2302f21e。首次协调检查发现当前Git index原始字节hash与验证记录不同；原脚本/错误保留，具体变化原因未实证。随后确认HEAD、完整文件/模式/集合及index对应源码树无变化、cached diff为空，按源码语义核对后通过，未重跑委派测试。freeze-05的接受仅绑定原快照，后续状态文档集成另记；不重写freeze-04拒绝及早前历史。

实际Windows链接/PE/运行、设备采集、ACL/sharing/reparse-safe存储与held lease/cleanup、保护投递、Windows包/许可仍未完成。项目阈值5和最后一轮直接保存交接规则由独立只读核对，不额外压测或强制压缩。

### Windows私有存储续作01开发交付（未独立接受）

原开发在已同步main的新分支交付9修改/3新增：私有存储模块与原生实现、Recording私有字段/所有权guard、仅构造表达式适配的既有测试、存储聚焦测试、Windows绑定features及WINDOWS说明。接受过的executor/actions/core/Swift字节不变。资源manifest仅Windows Cargo输入hash更新，120包行/特性/范围/选择和687材料保持。

最终60workspace Rust、fmt、严格clippy、完整默认Windows all-target及native严格clippy、材料一致性、已拥有Darwin debug help/safe smoke均为开发自检0；14命令的原始argv/env/日志与最终源摘要保持。原11断言用一处构造表达式适配执行0，原字节/patch单独保留，不称字节不变执行。3项原生集成测试只编译未执行，5项portable新测试不能代替NTFS权限/共享/路径实测。两次native编译101及失败源码保留，最终检查0。

协调材料核对782文件/276目录、770保护、288新证据8713408字节、最终快照/原字节和5项外部产物身份一致。首次checker误将external artifact记录路径字符串当列表，原脚本/错误保存后按真实schema更正并0；未改开发证据或重跑开发测试。报告c164ed14ab7e326ffc4d73a4f6f708981611dea129546819fa77d93a786335a0、index97b9d7038c05d26d7d6890a1eefefcbf5e42df263ffffdbc8e8fb4cda855ddbb。待新完整冻结原独立复核，VP-013整体false；未操作真实录音/auth/云端/目标。

### freeze-06原独立存储阶段接受（原生安全与完整产品未接受）

原独立亲自审查全部新storage/native/Recording源码、测试与威胁边界，未确认新已实现范围阻断。Mac完整25步通过60Rust/31core/73app/4loop/11packaging、fmt/严格clippy及六项预期非零CLI；693包文件/签名来源/helper ready-status-shutdown/safe smoke通过。全新Windows默认all-target检查0，16真实ring AMD64 COFF、1735541字节有效adapter rmeta；三个空测试rmeta不作为运行证据，3项native测试只编译未执行。

原11、cleanup-TLS四例、prior12与真实服务TLS/冷失败/160生命周期通过，历史原文与必要构造器迁移补丁保持；ownership另需一处只读path accessor迁移，首次编译101单列。9项新存储策略/WAV/guard控制通过；初次异常drop控制将Weak失效误当析构体完成而101，增加实际析构事件确认后通过，全部原所有权断言保留。负向API实际编译101并确认E0616/E0277。原service-TLS构造器迁移版仍101/collector1，只有两个已打印Poll观察通过，缺失第三项不计成功；恢复尾manual-Retry语义适配另为0。独立诊断collector与self-exclusion排序假设失败/修正均保留，无生产修复或自动重传。

协调check-vp013-accept-06.py首次0：782公共+5操作输入、276+2目录、766保护、69107历史普通/10链接/6特殊及完整集合/模式保持，1851新普通证据119311468字节/337目录和37项外部产物身份一致。报告SHA256c6f3e2d7f89ddb234a06bb19809265b2291882b94fc0ae3a45e92d907f7244fe，index12ccf7a5e688cf9d8a383b004f5202fa4ae908ce165d395d88dfe576f3d89e67。只接受原freeze-06源码/编译/离线所有权阶段；状态文档集成另记，原快照/报告/index保持。120包行和687材料未变；Windows Cargo features变化不代表Windows分发许可完整。

历史Git回执曾在freeze-05接受后被协调误写，已恢复原字节/hash，但inode/mtime变化未伪造恢复；原独立在freeze-06明确核对该披露和旧记录，不能声称历史文件从未被触碰。无协调测试重跑、真实录音/auth/云端/剪贴板/目标或权限操作。实际NTFS ACL/sharing/reparse/安全与竞态、Windows运行、采集和Start/Stop、残留清理、保护投递、完整link/PE与打包材料仍必需；完整VP-013未接受。

### Windows采集续作01开发交付（待独立审查）

新增惰性采集facade、共用合成后端的生产Session管道与固定缓冲WinMM候选；4修改/3新增，785公共文件/277目录。保持778公共+5操作保护输入、71765历史普通材料/10链接/6特殊，101新证据2234880字节及21外部产物行一致。开发12最终命令退出0，源摘要全程不变：71workspace Rust（60既有+11采集组）、fmt/严格clippy、完整默认Windows all-target、native严格clippy、材料一致性及自有Darwin help/safe smoke。16ring COFF只证明目标编译，1采集+3存储native测试均未执行；没有link/PE/Windows运行。

初次9组采集测试8过1失败101：合成backend未打开也发数据，保留原始9组源码重建来源与后加partial-IO版本区别，仅修fake使未打开返回End；随后10组、最终11组通过。native初次严格clippy101与失败源码保留，修三项lint后通过。首次生成器shell组合未单独捕获组件exit，原输出保留，后续独立记录regeneration0且最终产品字节未变。未知reset/buffer归还/close及析构体门闩、空/最大/溢出、真实合成部分写45字节错误、外来/重复/迟到frame等控制为portable证据，不替代设备/NTFS安全。

协调check-vp013-capture-01.py首次0，仅材料核对、不重跑开发测试。开发报告SHA2560115356bb5f1f5478da108de01c8fb05979c288798edd5cde4d87d8f6b02530d，index5c09b67860f07f43da14f0b587fdc86ed056cddc810037afb2c093a891544a08。120包行/687材料保持，manifest只改Windows Cargo输入hash；不代表Windows分发许可完整。新完整冻结和原独立审查仍必需；同步设备调用/Drop可阻塞，未轮询或停止时无自主墙钟停止保证。桌面Start/Stop禁用，无真实录音/auth/云端/目标/权限操作。产品未接受。

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

1. 用户在本机运行 `open dist/VoicePort.app --args --show-window`。当前 dist 已包含 VP-012 freeze-02 独立构建/检查的 Rust helper；原 VP-006 包的历史 release/签名/smoke 结论不覆盖它，当前整包本机离线验收以VP-012新报告为准，真实使用仍需本步骤。普通启动会按已记录规则尝试清理确认过期的本应用录音残留。
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
