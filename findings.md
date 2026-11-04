# 调研与发现

## 来源

- 用户指定会话：0004ca7f-8d0e-4201-82c5-42ad7e7fce20，已在本机 ArchitectureDesign 历史记录中找到；1041 行，约 5.7 MB。只提炼需求，不复制完整记录。
- 参考项目：../x-insight-hub。
- 已读：参考项目 docs/LOOP_METHODS.md；其执行入口为 automation/loops/PLAYBOOK.md。

## 项目现状

- 初始化时只有 README.md；当前已实现 Swift 核心、CLI、原生菜单栏应用和离线验证入口，尚未提交本轮业务代码。
- main 跟踪 origin/main；本地执行账本保存在 .voiceport，VP-005 已经 freeze-02 独立复核完成，本轮实施 VP-006 录音残留清理与打包。
- 本次唤醒发现 .claude/scheduled_tasks.lock，为会话调度产生的本地文件，应忽略，不作为业务源码提交。

## Loop 方法初步结论

参考项目将持久任务与单次 attempt 分开，依赖文件/SQLite 保存事实；一轮完成一个可验收故事，验证保留证据；区分代码修订、技术重试、外部阻塞与用户暂停。第三次相同失败重新诊断，两轮相同阻塞没有新证据则等待解除事件。单协调者负责当前状态，不能让 Markdown 和数据库双写互相争夺真相。

其 PR、独立角色、main 合并和外部消息策略是参考项目的约定，不能当作本项目已有授权。参考项目也明确未实现宿主自动事件订阅和完整费用采集，不将文档模板宣称为已具备的运行时能力。

## 指定会话还原出的需求

已逐条读取该会话全部实际用户文本与助手结论（排除工具输出、界面截图和技能注入）。需求主线在原记录行 946、985、992、998、1004；产品树与技术结论在行 975、987、994、1037。

- 产品名称 VoicePort；仓库 forwaryan/voiceport 在前序会话创建为 Private。用途为 macOS 全局语音输入，把 Codex / ChatGPT 云端听写用于 Orca 等应用。
- 用户重视识别质量：中文夹英文技术名词、数字、否定词、口头修正；仅换输入界面不足以满足目标。
- 默认路径：点目标输入框 → 全局快捷键开始/结束录音 → 录完上传 → 返回文字 → 输入当前应用 → 用户检查并发送。不要求首版边说边出字。
- 优先复用用户自己的 Codex / ChatGPT 登录。社区路线使用 https://chatgpt.com/backend-api/transcribe；没有公开稳定性承诺。
- 首版采用独立快捷键，全应用共享；Orca 原麦克风后端适配是后续选项，需要先查扩展能力。
- 参考 entropyconquers/gpt-voice（完整菜单栏应用）、Wangnov/codex-asr（CLI/本地服务）和 SergeyStepanenko/gpt-transcription（按住说话、剪贴板恢复交互）。需重新检查当前实现与许可再借鉴。
- 录音期间切换应用、同应用换焦点、剪贴板改变、网络/认证失败时应避免误投递并保留可恢复结果。
- 默认最小化本地数据留存，历史功能应明确开关；不得把用户登录凭据、录音或口述文本提交仓库。

## 证据边界

- 原会话对更早 ChatGPT 聊天只拿到部分副本，不能称已读全量最初聊天；本次指定的 ArchitectureDesign 会话文本已完整提炼。
- 原会话没有直接捕获当前 Codex App 请求，也没有同一录音 A/B 实测。相同接口不证明模型、处理、识别结果或用量来源完全一致。
- 不需要单独 API key 不代表免费、无限或不占订阅额度。

## 本机与重新核验结果

- 本机为 arm64 macOS，Swift 6.3.2 和 Command Line Tools 可用，不依赖完整 Xcode 才能执行 SwiftPM 构建。
- gh 已有可用 GitHub 登录；只读 API 确认 forwaryan/voiceport 仍为 Private。
- gpt-voice 当前 main 固定版本 0f3f29ffcb2bfe83806c0530559549b61d8f7f28，MIT；codex-asr 当前 main 固定版本 479f6a7a3db81fe2a23d4755b0ccbeb4400317d4，MIT。
- 当前基线采用原生 Swift/AppKit 菜单栏应用：直接使用 macOS 录音、全局快捷键、Accessibility 和剪贴板，无需 Electron 和常驻本地 Web 服务。用户随后补充 macOS/Windows 双平台目标、当前优先 macOS，并提出 Rust；后续建议把认证/转写/状态机逐步迁移为 Rust 共享核心，系统能力保留各平台适配，路线在 docs/CROSS_PLATFORM.md。此项是新目标下的迁移建议，Rust 和 Windows 尚未实现。

## 实施证据与调整

- 已读取固定版本的 TranscriptionClient.swift、CodexCredentials.swift、MultipartAudio.swift、PasteCoordinator.swift 和 MIT 许可。确认 `file` / 可选 `language` multipart 字段，Bearer / ChatGPT-Account-Id / originator 请求头，固定 HTTPS 端点。
- 首版自行实现代码，保留 gpt-voice 许可告知；不运行下载的上游代码，不读取本机真实 token。
- 本机 Command Line Tools 可以编译产品，但不包含 XCTest 或 Swift Testing。改为直接 swiftc 编译核心源文件与离线检查，保留内部 API 隔离，不增加测试框架依赖。
- 第一批 26 个同步离线检查已通过，4 个 Python 状态回归已通过；另添加 URLSession stub 的上传/401/超时/取消/每次重新读取凭据检查。
- macOS 完整录音、Accessibility 和 Orca 输入仍需真实运行验证；编译不等于已通过这些检查。

## 最新验证与下一轮重点

- 完整离线验证通过：31 项 Swift 核心检查、4 项 Python 回归、release 构建、CLI 正/反用例、.app 打包/签名与启动。报告 .voiceport/evidence/20261104-164319-006e41/report.json。
- 实际打开结果/设置窗口并查看截图，设置开关读回 1→0→1，已恢复。
- 真实运行抓到大小写不敏感磁盘上的二进制重名，CLI 已改为 voiceport-cli，并用输出断言回归。
- GUI 工具普通列表不显示 accessory 菜单栏应用；使用实际 PID 可正常读取窗口。其简化树省略某些 SwiftUI AXDescription；第二轮已通过原生属性确认名称和稳定 ID。
- 第二轮加入控制器依赖注入与 12 项离线系统边界检查。旧实现已复现服务 CancellationError 后不进入 failed；修复后 12 项全过，当前会话转为可重试错误，失效会话仍忽略。
- 最新完整验证 .voiceport/evidence/20261104-165628-e079ca/report.json 已通过 31 核心 + 12 控制器 + 4 状态回归，以及 release / CLI / bundle / 启动。
- 新构建实际窗口正常。直接读取原生 AX 属性证实 5 个控件的名称和 ID 正确；GUI 工具简化树会省略某些 AXDescription，不能把简化输出空白当作原生标签缺失。
- VP-005 已开始：原实现只在异步 deliver 的局部变量中保存旧剪贴板与 changeCount，正常退出没有可同步调用的恢复入口；控制器 shutdown 也未通知投递者。正在用独立 NSPasteboard 复现并验证。
- 目标与按键发送、恢复时钟已可注入；新增实际 coordinator 的隔离剪贴板检查，覆盖多项/二进制内容、前后发生的新复制、退出和目标变化。以上已通过的完整报告属于上轮代码，不能代替本轮验收。
- 本轮旧行为已实际复现 29 项应用检查中的 5 项失败，证据 .voiceport/evidence/vp005-before-fix/。开发工作会话现已接通 shutdown 恢复和迟到状态保护，补取消前后投递检查与 6 项反例，35 项 app checks 全部通过（退出码 0）。主协调者核对完整日志与 14 项实际文件 SHA256 均匹配，证据 .voiceport/evidence/vp005-developer/；这是开发自检，尚未独立验收。
- 用户纠正多会话 loop 缺失：参考项目 PLAYBOOK 明确保留原开发与独立验证会话，单协调者仅负责统一状态。之前只采用账本、attempt、证据与停止条件，没有落实会话分派与独立验收。
- 已用本机版本匹配 Orca 启动真实开发、协议会话，均收到 succeeded 完成回执。协议交付 7 文档/13 相对链接；两路报告共 21 个文件哈希均核对匹配。主协调者集成更新 SOLUTION/VALIDATION，建立 41 文件完整冻结包 .voiceport/evidence/vp005-freeze-01/manifest.json，并已收到第三个不同真实会话的独立验证启动回执，实际 deps 关联前两个任务。
- verify.py 的现有摘要不覆盖 docs 和 README；完整冻结另将其纳入并保存全部字节、基线、未跟踪输入、tracked patch 和目录清单。根 planning 与私有注册为可变协调日志，其启动前副本仅作上下文。验证期间冻结的源码、自动化和产品文档不再修改。
- 产品账本依旧一次一个 running 故事，支持同故事多个 Orca 工作任务。账本 status 也会初始化/原子写回，worker 不运行该 CLI。实际角色身份与回执关联由协调者登记，协议模板不替代宿主能力。
- 独立验证已实际退回 freeze-01：完整 verifier --app-smoke 退出 0（31 核心 + 35 应用 + 4 状态，报告 .voiceport/evidence/20261104-174143-8ab9f0/report.json），但独立默认 AX 采集检查 11 项有 3 项行为失败，属于同一已确认缺陷 VP005-I-001。NSTextView 只读仍报告 AXTextArea，仅用 role 白名单推断 editable 会对只读/禁用控件调用粘贴边界；已有检查直接注入 editable=false，绕过了错误的采集入口。
- 独立报告 .voiceport/evidence/vp005-independent/report.json 绑定原 attempt、verifier dispatch 和 freeze-01；41 项输入前后无漂移，协议 19 链接通过。不能以完整回归全绿完成 VP-005。
- 原开发返修已交付（新 attempt fcef5792-7bbe-4143-9d3e-2bdbecf9be93）：默认采集要求已知文本角色、真正 CFBoolean AXEnabled=true、AXValue settable 成功且为 true；无法确认能力时手动回退。新增 9 项默认入口检查，开发报告 44 应用检查和复制的 11 项独立反例均通过，并对无测试替身的原生 API 做 typecheck。证据 .voiceport/evidence/vp005-revision-01/report.json，仍属开发自检，待新冻结与原验证复核。
- AXValue 不可 settable 不等于所有控件都不能键盘输入；当前自动投递只接受明确能力证明，缺少该能力的编辑器保留原文供手动复制。真实 Orca/编辑器兼容性仍待实测，不从内存 AppKit 控件或合成 AX IPC 外推。
- freeze-02 已由原验证会话实际通过：31 核心 + 44 应用 + 4 状态回归，20 项独立检查，原三项失败反例均被拒绝；完整 verifier .voiceport/evidence/20261104-181322-4ddc5a/report.json。主协调者核对当前 attempt、实际 Task/Dispatch、43 文件字节/清单与限定本地离线的验收字段一致后，已 finish VP-005。
- VP-006（attempt ba30faf1-05b0-416b-9067-121f757be13e）开发已交付：录音改用 ~/Library/Caches/io.voiceport.recordings-v1 版本化私有根，新增 root/session 归属标记及贯穿 stop/上传/重试的 flock，占用释放且创建时间严格超过 86400 秒的确认残留才清理。采用 Caches 是为了稳定限定本用户命名空间，不扫描一般临时根或旧无标记 VoicePort-UUID 目录。
- 开发报告 .voiceport/evidence/vp006-developer/report.json：60 项应用自检与原生 VoicePortApp target 构建通过，真实子进程验证活动锁、进程退出释放和并发清理；旧失败路径 4 项行为失败已保存。尚未完整冻结/独立验收，不把上述自检当作完成。
- 当前仅承诺 POSIX 0700/0600 和归属/链接/占用/内容形状的保守检查；未知混合内容、未来时间、错误标记/权限/链接均保留。ACL、同 UID 任意修改原子性和断电持久性未验证；初始 root 标记写到一半的崩溃可能留下拒绝自动接管的根。测试仅清理自建隔离目录，smoke 在创建 storage 前跳过清理。
- VP-006 协调者已实际核对 app/native metadata 的 input_sha256 联集：21 项与当前文件全部匹配；PasteCoordinator、DictationController、DictationDependencies 也与已验收 VP-005 snapshot 字节一致。产品文档已集成并冻结为 vp006-freeze-01（45 文件），实际发送到原验证上下文，宿主确认 follow_up 并生成新 Task/Dispatch。已收到 msg_60f347a6358c：该独立会话完成 20 项独立存储/生命周期检查、31 核心/60 应用/4 状态回归和 12 个完整命令，报告限定本地离线通过；协调者已实际核对当前版本/身份、45 文件清单/字节/snapshot 与 12 个命令输出，写入 vp006-coordinator-acceptance.json 并 finish VP-006，不用旧 VP-005 完整报告覆盖本轮变更。
- 当前只剩真实环境任务：VP-007 已 start 后按真实条件登记 blocked（db54bd23-2cce-4cf2-baa8-a0f82d40e9e2），未进行真实录音、权限申请、账号调用或目标注入。VP-008 在账本仍 ready，但受未完成依赖及同音频样本约束，不能 start；不伪造 blocked attempt。已准备解除条件，按契约停止空转。
- 真实账号、声音、系统授权和 Orca 实测已单列 docs/VALIDATION.md；目前没有进行真实云端请求。
