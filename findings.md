# 调研与发现

## 2026-11-08 — 插件开发交付与独立验收缺口

开发采用内置HTTP和外部JSONL v1 batch适配器；一个descriptor定义插件，Source稳定ID保存配置，多实例与独立Keychain revision引用并存。外部适配器env_clear、根工作目录、JSONL stdin音频与选定配置、有界非阻塞stdout/stderr、自建进程组和后台kill/wait；不是恶意代码沙箱，协议明确禁止派生/逃离进程组。样例是显式确认的一秒静音，不能证明真实ASR识别准确率。

开发61项macOS通过/1个原有真实模型测试显式忽略、完整17步与输入一致已交付；协调独立核对交付20项、34原始日志hash及3个实际产物，未重跑委派测试。报告明确未做完整GUI和原生Security framework Keychain存取，必须补不同真实会话的隔离App/临时HOME配置和唯一测试条目实测；不得读取用户既有来源、认证或改真实偏好。整合README/SOLUTION后应产生新的最终冻结摘要，不继续把开发1793d204…当整合版本。

## 2026-11-08 — 自定义来源与插件边界

当前Catalog仅有ChatGPT与两种已知Orca模型，SessionSource只有ChatGpt/Local；并无自定义客户端或管理UI。voiceport-core的ChatGPT客户端故意固定端点且加载Codex认证，不能通过开放endpoint参数实现通用服务，否则会破坏凭据边界。控制器中以is_local反推系统Speech/云端路径的分支需要改为明确能力与适配器派发。

本轮用户明确提出一个插件代表一种接入方式。同一插件应允许多个来源实例，插件描述协议/配置/实时能力，来源保存独立端点/模型/认证；来源在开始录音及重试时固定。第一版必须有实际可加载新增插件的扩展路径，不能只有新名称或硬编码枚举。通用HTTP只能承诺停录后转写，任意ONNX或WebSocket需对应适配器。

新增实现从当前main d3af863开始；README/SOLUTION/CUSTOM_SOURCES三项方案文档已存在但未实现。新Run独立于已完成本地模型Run与旧VP013。真实协调session状态已记录2次已完成压缩、terminal/helper匹配；旧协调lease的requested状态不重置、不冒领。

## 2026-11-08 — 本地原生运行时实现与实测边界

开发已采用准确sherpa-onnx v1.12.37 C头文件与C++小桥，经系统clang++/ar编译后Rust动态加载Orca外部C API及相邻ONNX Runtime，无Node/PATH依赖、额外模型下载或模型复制。仅当前arm64固定安装/版本可用；常规C++异常变错误，底层原生abort不可恢复，取消等单个native调用返回后释放，不阻塞UI等待join。

公开英文WAV实际推理已证明流式partial、停止排空及端点段落累积可跑通，但输出也暴露Paraformer该英文样本较Whisper Tiny更容易错词，不能据此声称中文或所有场景准确。Whisper明确停录识别且30秒静音边界分块，仍有跨块丢/重词限制；线性重采样不等于高阶滤波质量。完整17步开发离线自检通过不替代真实麦克风、GUI/TCC、中文质量或受保护目标接收。独立验证将绑定最终整合冻结版本。

## 2026-11-08 — Orca 已安装本地模型与实时能力

用户需要的是现有 Orca 模型而非仅 macOS SFSpeech。实际模型根为 `~/Library/Application Support/orca/speech-models`，已观察 paraformer-bilingual-zh-en（encoder/decoder int8+tokens，16kHz中英流式）及 whisper-tiny（多语离线，含int8变体和公开test_wavs）。运行库在 `/Applications/Orca.app/Contents/Resources/node_modules/sherpa-onnx-darwin-arm64`，版本1.12.37，含C API dylib与ONNX Runtime；不能错认app.asar.unpacked或仅凭模型文件存在声称识别通过。

Orca普通speech CLI未提供通用本地转写入口。可评估读取其原生运行库或独立推理helper，但必须使用准确ABI、离开主/音频线程、统一采样率、限制缓冲和离线分块；Finder环境不能假定继承nvm的node PATH。Paraformer停录需排空队列/inputFinished并保留endpoint累计文字；Whisper应明确停录识别，不能伪称流式。现有Controller所有最终结果都走ChatGPT，须按录音时快照的来源分派，本地失败不能静默转云端或访问云端认证。现有原生popup每100ms重复选择会关菜单，新增来源控件也须仅在实际变化时刷新。


## 2026-11-08 — 原生控件的定时刷新与真实验收

NSSegmentedControl将听写/设置分开后，NSSwitch与NSPopUpButton比整行循环按钮更符合原生习惯，但不能每100ms无条件selectItemAtIndex：即使选择未变，也会打断打开的原生菜单。已仅在state/index/enabled实际变化时修改，真实UI完成中文、English、两组快捷键和开关变化，并读回language=en/alternativeShortcut=1/automaticPaste=0；随后恢复自动检测/默认快捷键/自动输入1，原先不存在而被测试添加的defaults键也删除。

GUI工具的成功AXPress回执不能替代UI后置条件。菜单在不同工具调用间可能因前台切换失效，同一脚本连续打开两个菜单也须等前一个完成；使用有界原生menu item readiness与真实应用值/持久偏好核对后才计通过。最终原生窗口620×552（内容620×520）、顶部菜单(973,4)、Dock无VoicePort、同PID82829关窗后菜单恢复与完整详情0秒均有证据；实际读取dictation.png、settings.png、menu-open.png。最终包仍AX未授权，不等同于真实语音验收。

## 2026-11-08 — GitHub原生UI设计skill

按用户要求检索GitHub，找到nextlevelbuilder/ui-ux-pro-max-skill（跨平台）及ceorkm/macos-design-skill（更贴合原生macOS工具）。读取后者commit 8f528a2364f996cd42f02a10b1b27198a74ca2a3的SKILL、layout-and-composition、visual-design和interaction-patterns，原文保存在.voiceport/research/macos-ui-20261108。仓库API未声明license，仅参考设计思想，不复制技能文档进公开产品、不运行第三方脚本。

本项目只有听写与设置两个目的地，适用其“少于3个目的地不设侧栏”、约13pt正文、8pt间距、原生分段页签与渐进展示；不引入无用搜索/历史/分享或虚假成功反馈。当前大面积空白、同层级按钮和技术错误顶置应改成紧凑听写页、独立设置页、短权限提醒与完整详情可查看。

## 2026-11-08 — macOS菜单栏宿主组隐藏与无Dock窗口

macOS 26上菜单项在AX树可操作不代表物理可见。VoicePort早期菜单坐标(7,965)/(7,976)，唯一屏幕1512×982；Control Center日志明确Moving host to blocked list。只读trackedApplications显示VoicePort自身isAllowed=true，但它同时归入isAllowed=false的Orca menuItemLocations组。移动菜单注册时间、仅刷新VoicePort开关、Finder和独立launchctl启动均未逃离该组。正常系统UI开启Orca组后图标立即进入顶部；可能同时显示Orca自身入口，须明确披露，不混同辅助功能权限。

最终0.3.2/build8采用LSUIElement=true与Accessory policy1，PID45517在(965,4)显示30×24菜单入口，实际截图已读取；Dock无VoicePort条目，关闭窗口不退出，菜单在同PID恢复窗口。最终新包辅助功能仍未授权，不能外推旧签名包的授权；保持0秒、未录音。完整17步离线验证全通过仅证明其覆盖范围，不代替真实语音或长期耐用性。证据`dist/menubar-only-20261108-9eaa1fc4-final/report.json`。

## 2026-11-08 — Git 集成与并发修改

远程 main 已到 `0d9e643`（PR12：CI 和离线首次运行诊断），该提交的 macOS CI 已成功。stash/快进/恢复后仅 README 冲突，已同时保留图标入口说明和上游 CODEX_HOME/--auth 优先级说明。发现另一活跃会话 voiceport-72 正在同工作区修改 app.rs 的菜单栏与界面，已通过会话消息协调其完成后停写，再做最终验证和 Git 交付。

首轮完整 verify（`verify-9c75bd7838d345e0aa1e35f63cab39da`）测试、fmt 通过，严格 Clippy 101：并发开发中的 StatusAppearance 及新增字段尚未接入，属于中间状态，不能记作最终验证通过或覆盖这些修改。

## 2026-11-08 — AX权限同名旧条目重建后生效

系统VoicePort switch=1与当前app AXIsProcessTrusted=false可同时存在；仅关开同名条目未修复。通过系统UI移除仅VoicePort条目、精确选择新包cursor-entry-20261108-9eaa1fc4/VoicePort.app重新添加后，原PID68143按钮实时变为「辅助功能：已授权（检查）」，无需退出进程，0秒/未录音。支持旧授权关联不匹配的判断，但未直接读取旧TCC条目的内部requirement；不能把推断写成已读取系统根因。面板旧失败文字仍保留，不等同当前权限状态。


## 2026-11-08 — Orca 真实终端接收

独立夹具 receiver-02 准确接收固定 `终端输入验证🙂` 共22个UTF-8字节，无Return；生产 Delivery::begin/deliver/poll 与原 CGEventPostToPid 分块发送路径通过。证据 dist/cursor-entry-20261108-9eaa1fc4。早期分散工具切焦尝试存在前台/活动tab漂移：两次拒写，另两次仅事件提交成功但指定receiver未收到，原报告保留不计通过。临时逐scalar实验也未获得独立receiver确认，已撤回，不能据此归因Chromium多字符事件问题。最终用单次有界脚本新建非执行夹具、确认ready、切换精确terminal、激活、立即探针并断言实际接收，原实现通过。GUI辅助输入尝试返回synthetic/unverified且活动工作区已变化，不作产品证据；无Return或命令发送。


## 2026-11-07 — 旧 Swift 清理

- 原归档共37文件，其中35个 `.swift`、1个Python宿主夹具、1个README；都不参与当前构建。当前核心/App兼容测试脚本实际调用Cargo，仍有用途，应保留。
- `scripts/product_inventory.py` 曾把 `legacy` 列为必需产品根；已同步移除该根并更新打包文档。删除后完整离线17步通过，Git archive干净检出清单与工作区一致，验证不依赖本机遗留目录。
- 当前README、方案、跨平台与验证首节曾指向归档，须改为Git历史；旧验收记录中的Swift命令属于历史快照，不伪改旧结果。LOOP_METHODS的清单说明仍指旧Swift路径且声称不覆盖README/docs，已按当前schema 2实现纠正。

## 2026-11-06 — 最终交付接续

- 当前 `verify.py --app-smoke` 使用合成 CODEX_HOME、离线 Cargo、全新临时 app 输出，覆盖 workspace tests/fmt/严格 Clippy/release、loop/packaging 回归、CLI/host/签名与安全 smoke。其权限 JSON 仅为直接执行上下文，不能证明 Launch Services 启动应用的权限。
- 打包脚本拒绝覆盖任何现有输出；最终试用包须选新路径。旧授权应用和中间 Rust 应用按完整可执行路径区分，不能按 bundle ID 批量退出。
- 交接最后 delta 为 AX_UNSUPPORTED/AX_NO_VALUE→Ok(false)，通信失败/异常 bool 仍拒绝；新增纯函数回归已通过。中间 Rust 窗口和截图正常，实际应用提示辅助功能未授权。
- 为满足用户“权限准备但不录音”，新增独立麦克风/Speech 授权按钮：使用专用回调通道，不进入 Controller::toggle 或 Recorder::start；请求期间禁开始按钮/热键。诊断 JSON 补充只读 microphone/speech 状态。完整17步验证通过（160顶层Rust、4 loop、16 packaging；storage嵌套子进程不重复相加，输入指纹前后一致）。
- 最终包实际窗口正常，独立权限准备回调确认麦克风和 Speech 均已允许、辅助功能未允许，0秒且未开始录音。直接终端运行 diagnostic 却给 accessibilityTrusted=true，实证该上下文不能代替实际 app 的 TCC 身份判断。
- 已在辅助功能页面点击添加当前包，系统出现 Touch ID/密码认证表单；后续观察进入文件选择器（未读取或代填密码）。已按最终1950包确切路径添加，并通过原生设置开关1→0→1刷新；未移除或重置任何TCC数据库。
- 最终包修复后完整17步通过；真实空闲PID26217正常退出且再启动PID28046，深层严格签名和whitespace检查通过。最终截图已实际读取，0秒/本机预览未启动/结果为空。麦克风与Speech授权已允许，辅助功能仍未允许；系统同名旧条目刷新和最终路径添加未改变本应用回调。已请求本应用原生辅助功能安全窗口，自动化操作该窗口返回permission_denied，需要用户本机确认；不移除旧条目或重置TCC。
- 原生 GUI 暴露真实退出缺陷：NSRunningApplication.terminate 返回true后 Rust 进程仍存活，退出标记使独立权限按钮不能再次执行。原实现一律 TerminateLater、NSTimer只在默认runloop模式；修复为空闲取消后同步 TerminateNow，借用重入/活跃worker仍Later，并把poll timer加入common与modal modes。完整验证须重新执行，不能沿用首轮通过作为修复后结论。
- 系统 picker 的 Orca background hotkey未改变UI、restore仍window_not_focused、AXOpen报错；改为原生激活系统设置、核对frontmost bundle后发送限定导航键，AX set-value确切路径已读回。一次AppleScript条件缺括号类型错误已修正；未访问凭据。

## 2026-11-06 — 集成与首次检查

- `cargo check -p voiceport-macos --all-targets` 已通过；首次 AppKit constructor unsafe 调用缺失已修正。严格 Clippy 首轮为局部风格告警，分别回所属模块修复。
- 音频/存储代理已实际验证原生合成 PCM→AAC、180 秒上限、迟到帧和损坏 writer；存储 12 项隔离测试通过。投递 2 unit + 20 合成目标测试通过；不等同于麦克风/Speech/Orca 实机接收。
- 独立检查发现 worker channel 断开恢复缺失，及同步长终端分块使主线程 monitor 回调延迟；正在补恢复与逐 tick 分块投递。
- 原 Swift 源码和行为测试无内容改动迁入 `legacy/swift/` 作为对照；根测试兼容入口已切 Rust，产品清单继续覆盖归档。历史冻结报告路径保持原义。
- 许可生成发现 objc2/block2 上游 crate 缺完整许可文本，代理正在按上游固定 commit 与标准 SPDX 文本补充来源，尚未宣布整包通过。

## 2026-11-06 — 原生迁移接口

- 新增 Rust 原生录音/存储、AX/CGEvent 投递模块；默认包将只包含 Rust 主程序，不再携带 JSONL helper。模块验证正在进行，尚未完成应用验收。
- AVAudio 原生类型来自 objc2-avf-audio，已添加直接依赖。共享转写核心新增显式 proxy 构造入口，避免在多线程应用里修改进程环境。
- UI 使用 AppKit 主线程；后台任务持有录音 Arc，释放后才回报完成。取消先使会话身份失效，等待文件使用结束再清理。

## 2026-11-06 — Rust macOS 迁移基线

- 用户新要求为完成 Rust 迁移；当前主应用仍是 Swift/AppKit/SwiftUI，Rust 仅负责共享转写/宿主及 Windows 适配。不能把现有 Rust 后端说成全应用迁移完成。
- 当前主分支 `80e5ada`，独立工作分支 `feat/rust-macos-native`。Sources/VoicePortApp 中录音、设备端 Speech、AX 输入目标、终端字符追加、菜单与热键均需对应 Rust 实现。
- 本机缓存包含 objc2 0.6.4、objc2-app-kit/foundation/core-foundation/core-graphics 0.3.2、block2 0.6.2、dispatch2 0.3.1，可优先采用原生 Rust bindings，避免新增 WebView/JS 或继续调用 Swift helper。
- 旧 Windows 冻结/多会话轮换状态是历史任务，不冒用旧角色或将其验收改为本次迁移结果。当前已授权应用需保留，新构建采用独立输出路径。

## 2026-11-05 — 策略要求不能写成未经接受的当前保证

策略独立P02-01确认五上下文及角色交接规则一致，但协调文档把未知schema/缺transcript失败和checkpoint身份校验写为当前无条件保证，与初次操作FAILED的确定反例冲突。应明确目标要求与实现返修/新pin验收待完成，加载证明不代替行为接受。P02-02发现README继承的Rust CLI段仍称存储未实现/旧同步join未验收，须与后来freeze05/06有限阶段接受和capture候选一致；历史拒绝继续保留。完成负向评审的runtime succeeded仅表示审查任务交付，policy verdict仍RETURNED。

## 2026-11-05 — 首次全角色操作审查的七个反例

原独立新上下文完成19项独立控制，发现7项确定反例，尽管23项已有单测通过：以inode/size/offset为唯一增量判断会漏同inode等长重写；协调者也须对缺transcript fail closed；Claude须证明真实身份，Codex无compacted时仍须已验证session_meta；通知后notes必须禁止继续写且缓存返回须校验hash；notes应无symlink/父目录替换并拒绝非普通文件；checkpoint须核实际环境terminal而非只信历史state/registry。此为临时项目对真实helper的可重复控制，不冒充宿主已出现这些故障或已实测压缩veto。FAILED报告和原字节保留，源码窄冻结显式结束后交开发返修，新的固定输入须再独立核查；不影响产品attempt/既有接受历史。

## 2026-11-05 — worker十次压缩的实际原因与双宿主修正

- 旧coordinator_rotation.py对非owner直接return，所谓5次限制没有覆盖worker。Orca新terminal handle不证明新对话，原独立preview实际Compactions:10。保留旧实现/测试沿革，新增按actual session独立计数、最后一轮限定交接并拦后续压缩；不能把worker续作身份变化当次数归零。
- 为消除“最后一次仍压缩”的歧义，现按最多5轮上下文=初始+最多4次完成压缩，进入最后一轮即直接保存换窗。Claude2.1.318的PreCompact支持decision:block/exit2，但continue:false不能veto；Codex0.166.0相反，此事件用continue:false，PreToolUse用decision:block。已安装实现/schema及只读官方文档为来源证据，不冒称真实compaction生命周期执行。
- 原生Codex项目hooks需具体定义hash获信任，文件存在不代表生效；每个新窗口先核对真实SessionStart/PreToolUse生成的状态、实际session/terminal和helperhash，缺证明停止该前端工作。原生rollout的session_meta.id和compacted.window_id已在本项目历史中确认，parser拒绝未知结构。实际compaction veto与跨窗全流程不强压验证。
- 新独立实际PreToolUse已加载但未观测SessionStart；初次把两个事件同时设为强制门槛导致无必要阻塞。原失败证据保留，改用真实PreToolUse+精确新session/terminal/helperhash+独立transcript身份/时间与0次边界核对，辅助事件未观察如实披露，不伪造或强压来补标记。native Codex实际被agent-hooks-review-prompt拦在接单前，不能从Claude成功外推native加载。
- 修轮换必须显式结束旧freeze07 live评审边界、保存行政交接后再改操作输入；旧快照/index/报告不改。产品attempt/接受/拒绝历史不因窗口轮换重写，新的操作审查与后续capture完整冻结分开。

## 2026-11-05 — Windows存储候选边界

- 存储续作把Recording路径/File/语言设为私有，并将根/祖先/marker guard通过Arc随Controller/Job保留；现有caller-owned构造只保证生命周期，不授予ACL/路径安全。原executor/actions真实停止规则未改。
- 候选原生存储限制在保守支持的本地固定NTFS路径/ACL；complex ACL/未知对象拒绝，不自动修复。Sharing不约束所有属性/安全请求，同用户/特权/预存恶意handle/驱动重映射等边界明确披露；失败可能留下私有不可接纳partial，无清理扫描。3项native测试只编译，实际Windows安全/运行仍须实证，不能由60项portable workspace通过外推。

## 2026-11-05 — freeze05阶段通过后的边界

- 原独立亲自确认VP013-I-003在freeze05修复，无新已实现范围阻断；原cleanupTLS四例全0，新请求所有权/receiver生命周期/健康后续与自动owner丢失释放控制通过。Mac25步55Rust和完整Windows默认目标检查通过。原serviceTLS恢复尾未经修改仍101，单独manualRetry适配0保持，不把两者混称。
- Git index原始字节包含源码树以外的状态；协调看到rawhash与验证记录不同，不能单凭它宣称源码漂移或推断具体变化原因。此次实际核对HEAD、完整输入/模式/集合、cached diff及index源码树一致后通过；原失败材料保留。
- Windows仍需私有ACL/sharing/reparse-safe存储与held lease/cleanup，再设备采集与Start/Stop接线；全链接/PE/实机、保护投递和分发材料也未完成。接受当前返修阶段不能完成VP-013。

## 2026-11-05 — 当前轮换规则与返修03

- 用户要求当前阈值和项目默认值统一为5，继任继承。到阈值的最后一轮直接保存和交接，不额外压测或强制压缩来触发或验证轮换；不得把合成边界测试称作真实压缩。运行configure、adopt默认、loop、文档、packet和launch提示已同步，16项隔离生命周期测试通过。
- VP013-I-003返修必须按请求真实所有权和receiver生命周期判断停止/健康，而非线程闭包结束信号；清理循环合法运行于所返回线程TLS的情况继续支持。开发原四例全0和健康后续请求是自检证据，仍须原独立新冻结复核，不能以普通workspace通过替代缺陷验收。

## 2026-11-05 — 压缩后的协调者轮换

- 原 loop 重复唤醒同一会话，自动压缩延续同一身份；共用契约此前没有达到阈值后换协调者的实现。主会话在交接期间由最初 11 次增至 12 次压缩。
- 上游调研：Anthropic PostCompact 可观察完成但没有决策控制；SessionStart(compact) 恢复上下文。hook payload 没有独立压缩 UUID，不能把触发次数当完成次数；项目从持久 transcript 的实际 compact_boundary UUID 去重，并在下次工具补查异步延迟记录。planning-with-files 提供磁盘恢复模式，HumanLayer 提供安全点与事务交接，Ralph 提供新进程间持久衔接；固定版本链接与取舍见 `.claude/COORDINATOR_ROTATION.md`。
- Orca 正常非 legacy Run 可由新协调者用普通 run-use 接管，真实回执更新 coordinator_handle 与 consumer_generation 并保留 worker；--takeover-legacy 仅用于自动迁移的 legacy Run，不能普遍套用。
- 实现是模型执行固定交接步骤的 hook 约束与持久状态机；不宣称模型失去响应后仍能自动复活的外部监督器。14 项隔离测试、当前 transcript 管道测试及真实 PreToolUse 加载检查通过；真实压缩触发跨窗口交接仍待验证。
- 最新 Windows 独立失败 VP013-I-003 已在交接中保留。普通 workspace/平台检查通过不能覆盖该 TLS 反例；源码提交和产品独立接受分别记录。

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
- Swift 阶段停止时只剩真实环境任务：VP-007 已 start 后按真实条件登记 blocked（db54bd23-2cce-4cf2-baa8-a0f82d40e9e2），未进行真实录音、权限申请、账号调用或目标注入。VP-008 在账本仍 ready，但受未完成依赖及同音频样本约束，不能 start；不伪造 blocked attempt。已准备解除条件，按契约停止空转。
- 真实账号、声音、系统授权和 Orca 实测已单列 docs/VALIDATION.md；目前没有进行真实云端请求。

## Rust 新版本启动

- 用户明确启动 Rust loop，目标是 macOS 优先并支持 Windows；这使 Rust 核心/CLI 和宿主集成成为可独立推进的新任务，不再受 Swift 真实录音任务的外部条件阻塞。
- 工作区现可正常读写，基线 6d6f42c 已实际推送 main；Rust 开发位于 feat/rust-core。工具链为 Rust/Cargo 1.95.0，本机 rustfmt/clippy 已安装，尚无 Rust 实现验收结果。
- 首项 VP-009 独立迁移文件登录、固定端点 multipart、安全错误/取消和 CLI；后续用有界 JSONL 子进程协议接入 macOS 原生外壳，避免立即重写已验证的录音与投递边界。Windows 系统能力单独适配和验证，不从跨平台核心编译外推。
- 权限诊断曾在 affected/fresh 两个样本和共同宿主记录内核 System Policy file-read-data deny；恢复后不继续用旧错误阻止开发。原始诊断仅留私有本机，不纳入产品仓库；具体恢复原因未确认。
- VP-009 原开发回执 msg_ac55207573e5 已交付；协调者读取实现和真实日志并核对 13 个 Rust 输入/快照、25 个受保护 Swift 输入一致。实际 Cargo 测试 28 core + 2 CLI unit + 6 integration，共 36；release CLI 另有 20 个正反用例，有行为重叠，不当作 56 项独立覆盖。fmt/clippy/release 均退出 0。证据 .voiceport/evidence/vp009-coordinator-delivery-check.json；仍须冻结后独立验收。
- Rust reqwest 0.12.28 显式禁重定向/自动重试/代理，无 cookie jar；HeaderValue 对 Bearer 与 account ID 标敏感。连接 15s、读取停顿 90s、HTTP 总 110s，不能与 Swift URLSession 计时语义等同。音频读取上限 20,000,000，auth/保留响应各 1,000,000 字节；同步本地读取不在网络 deadline 内，取消只能在其前后检查，已收音频无法撤回。
- 实际 HTTP 反例曾发现 Serde 派生结构接受顶层数组 [text]；显式对象形状检查后回归通过。首轮 clippy collapsible_if 失败也保留，均退出 101；最终自检日志已匹配当前文件。未访问真实账号/云端，Windows、TLS 和宿主接入未验证。
- VP-009 freeze-01 已独立接受并真实 finish：原验证亲自运行 36 项既有检查、8 组独立核心反例、37 个 release CLI 用例、真实 release SIGINT 130 和 4 项 loop 回归；59 文件/20 目录、557 历史证据、25 Swift 输入无漂移。协调者核对身份、完整文件/模式/快照和实际日志，证据 .voiceport/evidence/vp009-coordinator-acceptance.json。验证计数有行为重叠，不相加。
- 独立代理反例有有效正控制：默认 reqwest 联系自建代理一次，配置客户端绕过；独立计时反例证明读停顿计时会重置、总 deadline 覆盖持续读取。真实 release SIGINT 在仅该子进程禁网条件下返回 130，RSS 不能证明信号具体发生在文件读取还是网络等待；两阶段实际 reqwest 取消另以回环核心和 CLI 测试入口验证。
- 已启动 VP-010（2c6c5e81-6e88-4feb-a73f-e602dddf164a），继续可独立的会话/宿主协议。协调核对曾因复制目录实际在 harness/workspace 和 optional marker 为 null 而两次退出 1；均为协调脚本字段/路径假设，定位修正后全部核对通过，没有修改产品或重跑无关测试。
- VP-010 开发已交付，19 个报告输入/snapshot、30 个受保护输入、实际 10 项改动与保存日志均由协调者核对一致，当前产品集合 65 文件。开发自检为 41 项 Cargo 检查、13 组进程与 7 项旧 release CLI 用例，各组有重叠，不相加，仍待独立验收。
- 身份复用反例实际发现每 engine 的局部 counter 不足以保证进程内唯一，两个 engine 都产生 1；已改进程级 checked AtomicU64。跨进程重启仍重置编号，因此 VP-011 必须同时绑定 child identity 与 session，不能只按 s1 等字符串接纳回调。
- JSONL 宿主终身去重最多 1024 个不同有效请求 ID，第 1025 个返回 request_limit 并退出 0；原生接入须处理预算退出/重启。输入/输出帧上限 16384/2000000 字节，写确认总 deadline 5 秒；两个 detached stdio 线程只在进程生命周期内使用。成功/慢服务/输出压力由独立 Cargo example 注入，同一 host/parser/writer，但不代表生产 release 的真实云端成功。Rust 不删除宿主音频，重试 is_retry=true 应关闭自动投递。
- VP-010 freeze-01 已由原独立角色通过，协调者核对完整冻结/保护输入/历史证据、7 个工作区命令、11 项命令审计和 122 个证据摘要后真实 finish，并启动 VP-011（8d12916b-8059-4f9a-8758-08db0b6d2c1a）。独立为 41 既有检查、6 Rust 独立注册、46 有效进程用例（23 未修改 release），不相加。
- 每帧 5 秒总 deadline 不限制连续及时完成小帧的全流生命，独立夹具曾错误设 8 秒全流时限；保留失败后用原慢读正对照及单大帧校正。输出 fatal 可留下不完整末尾 JSON，实际 67104 字节，原生父端应在 EOF 丢弃，不能当 result 或拼到新进程。
- VP-011 需要新增后端 seam：现有 controller 的 transcribe(file,language) 闭包不能单独表达 Rust retry/cancel/shutdown。继续保留本地 SessionGate 覆盖权限/录音/投递，实际默认转写接固定 Contents/Helpers/voiceport-rs。Task 取消不等于 child 已停读，原生 lease/清理必须与后台停止协调；child 丢失后的显式手动重试可新进程 transcribe，但仍禁止自动投递，不自动重放。
- VP-011 原开发实际交付八项修改，协调核对 30 当前摘要、八份修改快照、54 保护输入与 68 文件集合一致；31 核心/69 应用自检及 release/package 日志通过。核心 metadata 含较早应用字节、release/package metadata 含随后改动的三项测试输入，按实际命令相关输入核对而非把无关字节称成当前版本；最终 app metadata 全 30 项匹配。独立验收待完成。
- 当前 bridge 130 秒只限 accepted 后 terminal 等待，ready/写/accepted 各可 5 秒，取消/退出另有控制与 0.05+2+2 秒回收观察；不能写成 130 秒端到端封顶。取消保持 lease 至 child 停止确认，失败停止保留占用并禁新录音；异步 AppDelegate terminateLater 等待确认。生产系统调用不可中断、真实 GUI 活跃退出/云端/Windows 未验。
- 交付快照仅含八个改动产品文件与两份生成 pycache，完整冻结必须另扫全部 68 个产品输入；原始交付 evidence 不改。协调核对器三次错误和开发两次早期失败均已保留，最终 consistency check 退出 0。
- VP-011 freeze-01 已独立退回：VP011-I-001 是父端未校验同 child 的 session 单调不复用，两个不同 request_id 的 s1 均成功；原控制器与真实 RecordingStorage/两份音频证明第二次自动投递边界调用、音频删除且不可retry。生产 Rust 自己仍生成递增身份，不能把异常 child 反例说成实际 Rust 自然复用。正常 s1→s2 两组正控制通过；两个退出1断言属于同一产品缺陷，五个child均退出0。
- 协调者核对68文件/22目录/1309历史普通文件/2链接/6特殊文件、139独立证据和原始失败后真实retry，新attempt 6adda6b1-822d-4516-8b98-87a0ee483656。返修要求当前incarnation内有界高水位/等效严格推进，只在确认回收/新child重置；新child可再s1。原验证尚未完成的精确帧/stderr/失败重试/预算/迟到/取消/lease/退出时限矩阵必须在新冻结复核，不能只修一个反例直接finish。
- 原开发已交付 VP011-I-001 高水位修复：完整 ASCII/canonical UInt64 身份严格大于本 child 高水位，accepted 即占用且失败继续消耗，仅确认回收/新 incarnation 重置，允许跳号及新 child s1。协调者核对 69 当前输入/4 改动快照/65 保护输入及实际日志一致，原 bridge/控制器存储断言复制字节不变，修前两份 reuse 退出1、修后退出0；正控制保持。31核心/73应用/release/package通过，仍为开发自检，待freeze-02原独立复核剩余范围。
- 返修首次 failed retry 测试出现协议错误来自合成 child 自定义 accepted(s8) 与硬编码 error(s1) 不一致；原夹具和73项一失败日志保留，仅修错误关联为同sid，最终73项通过。生产时限和controller/storage/Rust/scripts未改，helper签名副本对应新安装产物，未真实联网。
- freeze-02已实际完整建立，69产品文件/22目录，1690份历史普通证据与2链接/6特殊文件完整保留。相对返修分派前恰为3个既有bridge/关联测试修改、1个新增身份回归和4份协调集成文档，无删除，其余61保护输入不变。已真实follow-up原验证，新ctx_5281a4244276独立复核缺陷及首轮未完成矩阵；开发31/73自检不能替代本次接受，VP-011当时仍running。
- VP-011 freeze-02现已独立接受且实际finish。原bridge/controller重复s1反例被拒、真实第二音频/lease保留，显式retry不自动投递；79项矩阵和补充bridge/controller/storage/helper范围及31/73/八命令一致，1362项新证据索引与全部历史材料已协调核对。四次独立夹具/报告/编译失败和两次协调假设错误原样保留，最终通过不抹除失败。真实活跃GUI/Windows/云端/目标及通用资源上限未验边界继续保留。
- VP-012已实际start并派原开发，必须补齐现有verify未覆盖Cargo/crates/README/docs的完整产品指纹和Rust检查，实际开发包必须包含适用传递/内嵌组件license/notice，而非仅直接依赖SPDX表。原生产Swift/Rust/录音投递与协议输入保护，新scope限包装/验证/许可资源/文档；safe smoke使用已知无I/O退出路径，不能当普通GUI/录音/云端验收。该task_cdc471996bd7 / ctx_9157b3116e64现已交付并通过协调一致性核对；完整新冻结后再原独立验收。
- VP-012交付为710输入、4既有修改/641新增，64保护输入字节和模式不变；635份许可材料与native metadata缓存源及包内字节一致，120 crate scope/选择/内嵌充分性仍须独立审查，非法律批准。当前全入口41 Rust/31核心/73应用/4 loop/7 packaging及25步命令通过；首次缺失清单异常和旧测试产物失败保留，缓存清理后重建只证明当前摘要通过，不证明具体mtime/缓存失配机制。独立须用新自有Cargo target；safe smoke不等于普通GUI/真实录音云端目标验收。
- VP-012 freeze-01现已完整建立，710文件/265目录并保存文件/目录模式，3884旧普通材料及2链接/6特殊文件保留；已真实派原独立新task_57f21cea2e5e / ctx_88ca56def413。冻结三字段content与verify四字段产品摘要分别记录；开发起止旧摘要仅因协调四文档集成改变，剩61保护输入不变。当前仅等待独立结论，不能finish或以641bundle文件/120依赖的材料字节对应推断法律或真实平台验收。
- VP-012独立升级msg_a0efb51c4f05确认协调冻结元数据缺陷：710行的值/模式和265目录相同，product_inventory.inventory使用sorted(Path)的分段顺序，freeze-vp012-01.py第52行误沿用sorted(path-string)的freeze行。索引382的ring/src/aead/aes/bs.rs与aead.rs顺序互换，实际四字段摘要2c440f92e4af22f9a3a7a8fb8c122e19cbe5222a4a7439ade2e8f1c89769c75b，旧声明c44d8dfec2373fe1e50542b6bd1bdc2a9e40399663ea3468d85a881980b0a85e错误。协调核对已用冻结原公共接口分别计算live和snapshot，两者一致，并重建字符串排序复现旧错误值；不是生产源漂移。原freeze-01/脚本/独立失败保留，只能明确新freeze-02，不能覆盖旧manifest或无视不一致接受。此次仅escalation，尚无worker_done或独立最终结论。
- 随后原验证msg_44b12995e3b6报告三类产品阻断：根docs目录symlink与普通目录清单相同且exit0；完整签名包以本轮真实Swift CLI替换主VoicePort后原checker仍接受；安装许可清单UNKNOWN声明/selection和删去整包atomic-waker及材料后保持自洽重签也被接受，正确helper仍对应全新Cargo产物，六项常规破坏对照拒绝。最终反例配方/日志/结束完整性仍由原验证收尾，协调未读半成品或预判接受。单独更正元数据不能解决产品缺口，freeze-vp012-02.py仅为未执行草稿、无新freeze-02；两条实际reply msg_9c12782d63b6 / msg_b18cff7e4e2b要求保留原失败并完成false报告，再核对材料返原开发。
- 最终msg_d687cea5d770 worker_done failed已核对：VP012-I-001/002/003三类四输入确认，7989普通证据（143847737字节）和1合成链接索引9f4908ac…、710/265/模式/3884历史/2链接/6特殊/61保护及25日志完整一致；六普通破坏对照拒绝，实际fresh Cargo产物和主程序/签名前后对应成立。独立完整许可充分性及部分入口/目录漂移组合仍未完成，明确不接受。
- 协调核对器初次错误把同一路径的copy_before_signing历史摘要要求为当前签名后文件，退出1；原脚本/error保存在vp012-coordinator-check-failure-01。原audit明确先记录unsigned副本再codesign覆盖同路径；修为历史before与保留cargo字节/hash/mode一致、当前after与安装helper一致，最终核对退出0，未改产品/独立材料或重跑测试。
- 已真实retry/start VP-012为attempt ea6b723e-598a-4a2f-b4de-54aceffb68c4；实际reopened原开发，新task_ac869bb39540 / ctx_0f12ce81048b。允许9项既有脚本/包装测试/说明及third-party资源的必要修复，其余65输入字节/模式保护；启动保留710输入与12656历史普通材料/3链接/6特殊文件。返修必须覆盖四原断言、完整入口初始错误/运行漂移/目录变化和许可信任/来源/图边界，开发自检仍待新完整冻结及原独立ctx_88ca56def413验收；未执行旧metadata-only freeze草稿。
- VP-012返修01原开发msg_1b5f33ab3169 succeeded，完整schema2为762文件/270目录，目录模式纳入实际公共摘要，sorted(Path)顺序未为旧错误摘要改变。8既有修改/52新增、65保护字节及模式、12656旧普通材料/3链接/6特殊一致；17377新普通证据/5链接全核对。四例在有效正控制下before负面runner1、after含六signed破坏对照12项全过；实际entry初始根链接无步骤exit1、受控cargo改README后0/7而原verify拒绝，seam已披露。
- 返修许可687份对照120个native metadata包和Cargo.lock校验的公开.crate归档逐份一致，原635行/字节保留，52新增为不按后缀过滤的完整归属补充；源/安装清单绑定真实图/材料而非自证，正确main与helper双产物记录原始/签名后同时副本。最终含空格外部cwd、新空Cargo target实际25步/41 Rust/31core/73app/4loop/11packaging通过；本机bundle693文件。协调材料核对首次0，不替原独立充分性/边界判断。
- 返修首次before用错旧cargo产物，正控制先因helper身份不符失败，改为记录中的真实独立产物才重现四例；两次audit假设可选.cargo-checksum.json存在而失败，最终直接核对锁定.crate归档及全文。原失败保留，未假称正常通过；早期audit源码仅trace保留相关旧语句，当前audit已修，不假称每个早期源码版本都有完整单独快照。
- 返修完整freeze-02已实际建立并send原独立：762文件/270目录/模式、30020旧普通材料/8链接/6特殊、62剩余保护；schema2公共接口在live和完整snapshot逐项相等，当前公共摘要c36d9683666d5ecfb4acea0a4dcd955dc74d5d39e97bb25f077ceb0eab417611，开发旧5c1b…因四文档集成不同。独立三字段content 36dbfde28ff5bc1f559ad219ecfea7cb1d9eb90a722efeeed8f75dd16e9d43eb，manifest 7638b1f363f0e868723478fbfff84d8dc1d5455ba4f7a517e4e986c943c312a0，不混算法或覆盖旧错误摘要。
- 实际follow-up原ctx_88ca56def413，新task_19a4b7f33eb4 / ctx_ec58a916167a，规格vp012-verifier-02.md明确复核四原断言、所有剩余许可充分性与入口/目录漂移，再亲自fresh Cargo target全入口。当时源码只读、等待事件；实际verify仅允许在旧开发目录下新建verify-UUID并镜像进独立新scope，不可覆盖任何旧材料。随后独立msg_2fa94af1de04 succeeded已接受freeze-02全部必需本机离线范围，协调核对23554新证据、30020历史材料、762文件/270目录、实际25步及687材料完整来源/条款后已真实finish。详见vp012-coordinator-acceptance-02.json；原三类四反例均修复，23清单/6入口/20signed变异/24源许可范围及真实skip-build通过。103个实际release normal/build依赖、107个workspace含dev依赖与120 metadata/13未启用节点已分开说明。VP-013 attempt4c66c1a3-58a4-46ec-930d-a007f19a4ace已开始，Windows和用户参与的真实环境尚未接受。实际原开发reopened为task_ca5492581a44 / ctx_2334a32d8f09；当前Mac仅原生Rust target，Orca无已连接远端/配对环境，所查常见Windows工具/VM应用未发现。交接要求有编译证据的适配进展、实际Windows验收门槛保留，不提交大块未编译代码后宣称完成；756保护包含所有旧687材料、54441历史普通文件/10链接/6特殊，完成事件唤醒后再核对。
- VP-013首份局部交付msg_c93126c789b7已经核对：4改动/10新增、772文件/275目录、756保护、54441历史普通材料/10链接/6特殊及132新证据一致。Rust/windows-sys方案直接复用核心API，7个新测试与现有Mac回归/15步入口通过，但完整Windows默认检查因缺x86_64-w64-mingw32-gcc退出101，adapter-only0不等于完整编译或运行。托盘/采集/原生ACL存储/手动复制/UIA剪贴板/GUI仍未完成。
- 未改动687材料不等于manifest只有输入摘要变化：新crate的dev依赖还使bytes/mio/pin-project-lite/signal-hook-registry/socket2/tokio六项scope新增dev，原报告不覆写，协调核对及集成文档补正。真实locked包项/版本和材料/选择均保持。
- 官方Zig工具包索引实际HTTP200可读，证明存在候选本地compiler下载路线，但未证明本项目C编译或链接可行；缺gcc暂不直接当外部不可解阻塞。已实际reopened原开发续行，新task_cca0083a8a38 / ctx_c168c4612f21，同产品attempt不变；新启动保护772输入/756保护及54581历史普通文件/10链接/6特殊。下一轮尝试独立目录校验工具包/真实完整目标检查与可编译原生功能，运行未验证仍明确保留，该次等待状态已由最新续作交付取代。
- VP-013续作02回执msg_ee51019e1f55已核对，报告SHA256088096b8f35432ea4ad98def2106bd3a63db612b53ad85aedecf093f3cacaa04，索引3eb7a10cc9f998622a23d923078e5ec796cd2a9148cdf5a524840bdfe2bda337。775文件/275目录、6修改3新增及161新证据一致，756保护和54581历史普通文件/10链接/6特殊不变。check-vp013-delivery-02.py首次0，没有重跑委派测试。官方Zig20792成员/归档/提取编译器字节核实，真实ring COFF对象及完整默认Windows GNU all-target检查0；dlltool配置后实际链接仍缺libpthread.a/msvcrt，两个build退出101，没有PE或Windows运行，缺运行库仍可继续研究而非宣称不可解。
- 当前原生壳已有显式--run托盘/隐藏窗口/消息循环/线程热键冲突与动作/手动复制/退出协调，原生FFI仅编译未执行。设备录音/安全存储/共享/reparse/held lease/后台执行器/保护投递/Windows打包未完成。广泛Mac15步通过41既有Rust+7controller/WAV+2初版actions和原Mac回归；退出取消优先复制修正后补跑3最终actions+7controller/WAV、lint/fmt与完整Windows检查，不能混淆时点。本轮manifest仅build_inputs变化，687材料未改，首轮6scope新增dev的精度更正保留。
- 协调仅更新四产品文档，其他771输入和全部模式保持，已建立775文件/275目录完整vp013-freeze-01，manifest77de301b79e672a5dcda0d0cf9f91014cf84582ea575684fffd76fbd5fe8ff65、content e10c40187d335b7f0f6be1bb3f27f4770a6ca8364b8ec4040793656ed24ccb6c、public915cfe6e76e9e75846b406985d48d8eabd9cbae7a631dc0bd72e61f074fcdd09，live/snapshot实际接口完整相等，54717历史普通/10链接/6特殊和752剩余保护记录。实际follow-up原独立，新task_eb01b66a93de / ctx_5e90436934ec，规格vp013-verifier-01.md SHA256855e4b13f6550d3bbe939982788b92d97ed88e6d522b7e4ed1fb7fcc9bee7e9d。该checkpoint审查现有实现和相关回归，全产品接受始终false；该次等待已结束，最新结论如下。
- 原独立msg_8a8b0c7fa982 failed已核对，确认VP013-I-001：普通Actions Poll仅closing时调用controller stop，取消期间Job尚存活，随后Job被迟到完成消费或drop后，八次普通Poll/timer仍使新准入Busy并保留recording。Controller直接stop轮询与正常Actions完成可恢复，额外Cancel也可恢复，定位实现分支。修正夹具显式加三次post-completion Poll后成功预期Cargo仍101（10通过1失败），runner1；diagnostic0是断言缺陷存在，不能当通过。关闭尾部另有正对照执行，未外推到尚无服务调度的Windows GUI。
- 原独立实际新target的完整Windows默认all-target检查0且16个COFF对象，最终冻结Mac25步51Rust/31core/73app/4loop/11packaging、双产物签名/693文件包/helper/safe smoke通过；1400新证据83361230字节、775/275/752保护/54717历史普通/10链接/6特殊完整一致。协调check-vp013-rejection-01.py首次执行0，报告6577a75923a38d052a24e9de67008a5b4fae71fefb25851f01c64345021105f3、index def841a405db2efec08cbe8a2c7d4bdc0ddcddef8a7572b270f572980b191ffd；通过回归不能抵消缺陷。初始Write工具入参截断在文件创建前被拒绝，改合法完整输入后创建checker，不涉及产品或委派检查失败。
- 已真实retry旧attempt并start新0d21e578-dac8-46ff-aa22-6d19b30b68e1，freeze-01永久rejected snapshot，当前仅为明确聚焦返修解冻。仅集成四进展文档后实际reopened原开发ctx_c168c4612f21，新task_5ec41d525e67 / ctx_0912c821c9d4 ready，终端term_2db262399fb141fdd50c278ba246efc4；规格vp013-revision-01.md SHA2560344c24e9a098c16e391971cfb6de27895234d7529d05ff38fe6fd9b7f7fe15b，真实回执vp013-revision-01-dispatch-receipt.json。启动775输入/772保护及56911历史普通材料/10链接/6特殊，允许actions/相关tests/Windows说明，其他生产/依赖/许可保持。要求原失败修前101修后0、Job尚存活时不误确认、释放后普通Poll恢复但开放状态仍false、closing确认才true；新完整freeze再回原独立ctx_5e90436934ec。全产品Windows运行/链接和缺失功能仍必需，无产品接受。
- 最新原开发msg_123d8c34ac00 succeeded已消费，返修报告234b53aee5f244bb4b39abd975670bb2f899660cfbf89d8a4efa7f57a825d21a、398新证据完整核对；三项允许改动/772保护及56911历史普通材料不变。原独立11项成功断言保持原字节，修前101/collector1，修后0/collector0；最终workspace52项、fmt/严格clippy和完整默认Windows all-target检查0。普通Poll推进stop，但open始终false、Ready结果保留，closing且确认才true；开发自检并非独立接受。
- freeze-vp013-02.py首次实际执行0，已建立775文件/275目录完整repair freeze-02，live/snapshot实际公共API逐项相等；57313历史普通材料/10链接/6特殊和768剩余保护核对保持。manifest 8f79152a7a50aea9a956854886f731b74a186d7bb2cdce66f8c04a945b237647，三字段content eb6e3a0a0d99cefab4d47a4c14033c3e6651d51cd81033d20ce3e62620627acd，公共schema2 e9b02da8320ea3915e3b1af4096e9b22be3d27eb1b612b4c8ef9d35c36874847；开发d06c…摘要因协调仅四文档更新而不同，其他771输入/全部模式保持。原freeze-01永久rejected，最新freeze-02仍为部分实现复核，未接受完整VP-013。
- 已实际follow-up原独立ctx_5e90436934ec，新task_42c9f403abd7 / ctx_d5ff27ba1a13 ready，spec SHA25641fc24157ff8a33a20330538dd9546704b384974cbdde2fa478f66e3250c1d58；真实receipt vp013-verifier-02-dispatch-receipt.json已先保存后登记。775输入只读，独立亲自原11断言/相邻矩阵及当前Mac入口/完整Windows检查，未变旧范围只按明确来源保留；等待完成事件。新缺陷返最新原开发ctx_0912c821c9d4，修复checkpoint通过仍需推进Windows缺失功能/完整链接和实机/打包。
- msg_e25452fc2195 succeeded已消费并完整核对：check-vp013-acceptance-02.py首次0，确认原独立report SHA25610c917be6086b13144a17e41652679937d902dade5d3be80d426097fd1153779，VP013-I-001 fixed、无新阻断，775/275及57313历史普通/10链接/6特殊、768保护和1234新材料74178041字节/307目录全部一致。原11断言亲自0、4取消组合/3优先级/2关闭释放正对照、Mac25步/52Rust和完整Windows默认检查0，693包文件与双产物签名对应，687材料保持；仅复制auditor子组3→4断言修正，原exit1保留。协调接受仅freeze-02部分返修checkpoint，不finish VP-013；下一批推进后台转写执行器、非阻塞消息循环轮询和显式手动Retry，录音/原生存储及链接/实机/保护投递另留必需范围。
- continue-vp013-after-checkpoint-02.py已首次0归档本次checkpoint，同attempt保持running，不retry/start/finish；协调仅集成四产品状态文档，其他771输入和全部模式不变。dispatch-vp013-developer-03.py实际exit0，reopened最新原开发ctx_0912c821c9d4，新task_f23c2d7491eb / ctx_f235b23b58b7 ready，codex，term_43d14036a089c62f368e33349e31cbab。启动775输入/764保护/59371历史普通材料（本次包括隐藏与pycache）/10链接/6特殊，specSHA2566e0afe987603a59f3fd00eb33ee639a8f428ef879dee903d8324613831729473；新public b650aa58a03a2cab407d5a212253b1ed8c1e03cf072229b959a6428d5f12904c。后台执行器/结果/显式Retry必须通过真实生产并发机制和隔离fake服务验证，GUI不能阻塞或假确认停止，Start/Stop仍禁用；旧687材料和核心/Swift等764输入保护。当前等完成事件，未来新完整冻结回最新原独立ctx_d5ff27ba1a13；该等待已由后续续作03交付取代。
- 续作03 msg_13b73ba1e907 succeeded已消费，reportSHA256612a5fd76fa2c2d1fb93337e7168f1d6b4888e5cdcf23d949fec8e7384de5132。生产executor、Actions/timer/Retry/结果/退出接线已交付，7修改2新增、777输入/275目录；764保护和59371历史普通/10链接/6特殊不变，385新证据/24目录、两轮新verify完整镜像来源通过协调核对。最终53Rust/Mac25步/31core/73app/4loop/11packaging和完整Windows默认检查0，原11兼容断言0，687材料/120包行不变、manifest仅build_inputs。单一worker许可与唯一Job/完成槽、实际线程停止确认及异常owner有界接管是本轮新增范围，尚待原独立亲自审查；Start/Stop因录音/安全存储缺失仍禁用。
- 协调checker初次把2个artifact样本要求成16个而exit1；原checker/error保留后，改为核对样本2及实际final target16个COFF头/字节/mode/hash，最终0，无产品或委派测试改动。只读schema查看files/regular差异和超量输出也记录，按实际schema核对完成。
- freeze-vp013-03.py首次0，已实际建立777文件/275目录完整freeze-03，59870历史普通材料（含隐藏pycache）/10链接/6特殊和760剩余保护保持，live/snapshot公共接口完全相等。manifest abc8748eca7241f4768cb515b02105942e11a32de3b42add7df8d8ce6642bf31，content8612d7cc43e0c21a4016777b129c8f0185392e0cbf344f212b9a83dc908c3feb，public schema2 9dedae441599405b43aa881c9fbee56775babec9466bf0aed672e3084e3141da；协调仅再集成四状态文档，其他773输入和全部模式保持。当前所有777输入只读，完整产品仍未接受。已实际follow-up最新原独立ctx_d5ff27ba1a13，新task_0d946ca10728 / ctx_3d12c3678161 ready；真实receipt vp013-verifier-03-dispatch-receipt.json，specSHA256fba6f89cf90a59d686ac01f2a6c6c6f73ba355c291baad53d368be2cefcd9d14。当前等完成事件，自行生产线程/生命周期/取消/Retry反例及当前冻结回归，缺陷回最新原开发ctx_f235b23b58b7。
- 本轮已消费msg_e46657b5c540：原独立task_0d946ca10728 / ctx_3d12c3678161 outcome=failed，freeze-03 checkpoint拒绝。VP013-I-002已用实际OsSpawner与TLS析构入口确认门闩复现：is_finished=true之后同步join仍可等待真实线程清理，ordinary Poll/cancelled Poll/orphan reap三路等待显式释放，实际1003/1005/1007ms，成功预期exit0/实际1、collector1；正常当前结果/copy/释放后恢复正控制通过。源码executor.rs62及Actions::drain/submit关系已读核，未改产品或重跑测试。报告SHA256d1b1aa211c05323a1acdaab47b0708e476d3ff63a2111622c35c993d98b05f29，独立声明777/275/59870历史普通/760保护及1489新材料无漂移，正在协调完整核对；Mac25步/53Rust和完整Windows默认检查通过不覆盖行为失败。原WindowsGUI/defaultCoreService TLS触发未实证，不外推；需在保留票据/File/permit直到真实stop前提下修复消息线程等待，回最新原开发。
- check-vp013-rejection-03.py最终0，完整777/275及59870历史普通含隐藏pycache/10链接/6特殊/760保护与1489新材料83348061字节/323目录核对一致；报告/index/原TLS失败及正控制、Mac25步53Rust、完整Windows默认0/16COFF/有效rmeta、693包/双产物/helper/smoke、687材料来源全部符合实际。协调wrapper嵌套和materials容器两次schema错误各自原脚本/error保留，按实际字段修正后通过，未动产品或重跑委派测试。原独立pin生成与两次tree/metadata假设失败保留，分别核对各平台身份不能混树与metadata数。
- retry-vp013-02.py真实两命令0，当前attempt1ed6204e-5637-4c5e-b059-51ac4256a4e6，旧freeze-03永久rejected，原freeze-02修复接受仍保持。仅四产品状态文档变化，其他773输入/模式不变；dispatch-vp013-revision-02.py实际0，reopened最新原开发ctx_f235b23b58b7，新task_aa40d6e71190 / ctx_e86b9f735149 ready，codex，term_65eaecc6c7e05d526d75cb66ce475c5e。启动777输入/772保护及62159历史普通材料含隐藏pycache/10链接/6特殊，specSHA2566e2a361deb07c09b455133744eaeaa5c8692d3de2c64a8ae158dcaf90ab5276f。仅executor/必要Actions/相关tests/Windows说明可改，不新增依赖或不相关功能；原门闩响应断言保持，若异步orphan回收改变立即准入假设，仅合法保留Failed/显式Retry恢复尾单列适配与patch、原after结果仍保留。当前等完成事件，新完整冻结再回原独立ctx_3d12c3678161；全产品false。该开发等待现已结束。
- 返修02 msg_df14f5209718 succeeded报告SHA2565ea45c19dfbdd822a612bb69739f2b87d8ec8ce22e0f575efc5a7c4cc7d098d8，原开发停写、产品接受false。check-vp013-revision-02.py首次0核对778文件/275目录、3修改1新增及原字节/快照/772保护/全模式、62159历史普通/10链接/6特殊、1258新材料20840524字节/313目录保持。生产join移到单一进程级清理线程，UI轮询release/acquire停止确认，不用result或is_finished推断worker停止；冷初始化失败不启动worker，发布后panic改静态失败。原after101来自立即成功orphan恢复假设，实际失败保留；只替换一处恢复尾显式Retry的适配保持三处原门闩响应断言，0ms/exit0，原11fixture0。最终54Rust/fmt/严格clippy/完整Windows默认检查0，final实际16COFF/有效960961rmeta；76artifact含本机对象，不能说全部COFF，Windows记录只2样本。此窄返修未跑Mac全入口，原独立须在新完整冻结亲自执行；清理线程新生命周期与恢复适配仍待独立接受。原两lint/发布后panic/helper路径失败均保留；协调只四文档集成，其余774输入保持，继续新freeze-04和实际回原独立。
- freeze-vp013-04.py首次0，完整778文件/275目录、63421历史普通材料含隐藏pycache/10链接/6特殊、768剩余保护与全部模式不变，live/snapshot实际公共API相等。manifest6488ad38fe9dae740e1a91e45bd988de8192788e6b4849d0bcac54bf25c63a6b、三字段content1d429e9e9d054b64d9a535a35dd5b848bff438309739456e38d8888ec2a9fda2、public schema2 313ebb8ed8da72ae42aed0accd3d67d71b0d4ce6ff0283db90c044a249d9a2b1。dispatch-vp013-verifier-04.py首次0，实际follow-up最新原独立ctx_3d12c3678161，新task_e173aaa67be8 / ctx_f27bbb6783ce ready，沿用term_8a7b0a0f-5f38-4fbf-9454-6235ce3ba28a；specSHA256c140e99172ebc3113de86021cdfef2a424378dbdb2bfcf17dac708b25c2fa19b，receipt先保存后登记。当前778输入只读，等待真实完成事件，不读半成品或重复测试；原开发ctx_e86b9f735149保留返修入口，新清理线程与恢复适配尚未独立接受，完整Windows必需范围不豁免。

## 2026-11-05 — 存储freeze06独立阶段结论

- 后续freeze05显式请求Job/permit所有权修复已独立接受并通过PR2合并。私有存储续作01现也通过原独立freeze06源码/编译/离线所有权阶段，未确认新已实现范围阻断。协调首次完整核对0，782公共+5操作输入、766保护、69107历史普通及1851新证据119311468字节一致；实际Windows3native测试只编译，不能从16COFF或1735541字节rmeta推断NTFS安全/运行接受。
- 新Recording私有字段保持File-before-guard析构顺序，实际服务线程TLS/失败/manualRetry/cancel/Exit/异常owner的guard控制通过。Weak失效先于析构体事件是测试同步错误，原101/poison日志保留，等实际析构事件后通过；无生产修改。fixture构造器迁移与一处只读path accessor额外迁移显式保存，原serviceTLS语义101及manualRetry-only适配0不得混称原样通过。
- 当前Mac25步60Rust与31core/73app/4loop/11packaging、693包/签名/helper/safe smoke通过。120包行/687材料未变不等于Windows分发范围完整。历史Git回执误覆盖后已恢复字节/hash，inode/mtime变化仍如实保留并由freeze06核对，不能称从未触碰。
- 设备采集基础可在现有条件下实现并以合成后端、所有权/失败反例及完整目标编译检查验证；普通UI的Start/Stop须等原生安全存储及设备运行门槛，不以静态阶段接受启用真实录音。完整VP013仍running/false。

## 2026-11-05 — capture01候选边界

- 原开发惰性采集候选通过材料核对，尚非独立接受。WinMM事件/poll、固定180秒主buffer+4096sample哨兵避免动态补队列，代价是批量数据通常直到显式stop/reset才可读；没有自主墙钟停止保证。所有native调用与Drop可阻塞，禁止直接接消息线程，当前Start/Stop未启用。
- reset成功不等于buffer释放，必须继续actual-return/unprepare/close及backend析构体完成；未知释放保留一个有界owner/预算且拒绝替换。独立应自行挑战全部失败/取消/Drop/格式/缓冲身份和exact-max路径，不把合成后端当driver/NTFS保证。
- 71workspace/12最终命令0为开发自测，1采集+3存储native只编译。101新材料2234880字节及778公共+5操作保护/71765历史普通核对通过，未重跑测试。既有核心/所有权/Swift与120包687材料保持；原native和fake失败、生成器单组件exit缺口及记录regeneration0均保留。下一完整freeze07必须回原独立，产品仍未接受。
