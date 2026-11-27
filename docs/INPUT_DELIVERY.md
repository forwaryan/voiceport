# 全局光标输入

在目标应用中点击可编辑位置，让文字光标进入其中，然后按全局听写快捷键开始说话；结束动作取决于[快捷键模式](USAGE.md#快捷键与一次听写)：默认再按一次，按住说话则松开。同样使用按下时绑定的目标，最终文字送入原输入位置，由使用者检查和发送。鼠标悬停不代表文字焦点。识别来源（QQ、ChatGPT、本地模型或自定义服务）与输入路径独立。

## 推荐默认策略

保持自动输入开启，由程序根据捕获到的目标能力选择路径，无需日常手动选择某一种后端：

1. 标准、可读可写的原输入框采用 AX 范围编辑，保留光标两侧文字，并对本次修改读回确认。
2. 可读文字和选区的网页输入框，以及已知需要键盘兼容的编辑器，采用最终 Unicode 分段键入；每段确认后才发送下一段，实时预览留在面板。
3. 原窗口可以绑定、但缺少可用 AX 文字编辑接口的兼容目标，采用最终剪贴板 + Command+V；不展开菜单，界面只报告已请求粘贴，接收效果由使用者检查。

选择发生在首次文字修改之前。原目标改变、出现用户编辑或结果不明时停止，保留面板结果；不将写入失败当作自动切换后端和重投的理由。Orca 终端保持专用追加路径。

## 输入路径

| 目标能力 | 使用的路径 | 写入确认 |
| --- | --- | --- |
| 标准 AX 文字/选区读写 | 现有范围编辑，支持本次预览和最终校正 | 读取原字段确认 |
| 可读文字/选区且位于 `AXWebArea` 内的网页输入控件 | 最终文字分段 Unicode 键盘输入 | 每段读回确认 |
| Codex、Orca 编辑区 | 现有分段 Unicode 键盘输入，保留各自节奏 | 每段读回确认 |
| Orca Terminal input | 现有稳定前缀追加 | 原终端目标与输入活动检查，不回车 |
| 微信缺少焦点字段的主窗口 | 最终文字一次性剪贴板 + Command+V，不展开菜单 | 仅确认事件提交；微信接收须独立实测 |
| 已确认缺少标准 AX 文字能力的其他编辑区 | 最终文字一次性剪贴板 + Command+V | 仅确认事件提交；应用接收须独立实测 |

兼容路径不要求读取正文或光标范围，支持暴露为容器的自定义编辑区以及没有焦点字段接口但能绑定原窗口的应用。表格单元格 `AXCell` 必须明确返回布尔 `AXEditable=true` 才能进入该路径；缺少、错误类型或只读声明都不放行，也不扩大到整张表、按钮或任意未知角色。

最终 Unicode 键入与兼容粘贴共用物理修饰键判定。Unicode 的每个待发段在原快照仍匹配时，非阻塞等待 Shift、Control、Option、Command、Fn 松开；连续等待最多1秒，超时停止，较晚松键不重投。等待期间不丢弃待发段，已发段先完成读回；取消只丢弃未发部分。事件构造后、真正发出前再查物理状态，新按键冲突时停止。标准 AX 编辑不依赖键盘组合，Orca 终端保持原专用策略。

WebKit 网页可能声明 AX 文字接口可写，却不把 `AXSelectedText` 更新应用到网页。对于可读文字/选区的标准输入控件，沿最多 16 层 AX 父元素元数据识别 `AXWebArea`，在首次写入前选定带读回的键盘路径；实时预览留在面板，最终文字才进入原光标。Codex、Orca 编辑器及 Orca 终端保持各自专用路径。先检查已有能力，在任何文字修改前固定路径；一旦有写入或无法确认的副作用，不换路径自动重投。

## 目标与剪贴板

- 开始时固定应用 PID、bundle 和原窗口；可读取焦点元素时也固定该元素。
- 微信缺少焦点字段时，只允许原来的非模态主窗口；登录、通话、独立聊天窗口不进入该兼容路径。最终提交复用 Command+V，不展开「编辑」菜单，也不再使用菜单点击作为后备重投。
- 准备辅助功能树期间保留同一个用户输入监测器，不因选择兼容路径清除已经发生的点击或按键。
- 每次最终提交前重新检查原目标、当前辅助功能权限及系统安全输入状态。目标变更、真实用户操作或新增剪贴板内容使本轮自动写入停止。
- Orca 必须先确认编辑器／终端分类。描述不可读、缺少可确认焦点或专用能力时保留面板结果，不进入通用剪贴板路径；直接捕获、异步准备和兼容目标构造都执行这一限制。
- 只使用最终转写文字；实时预览和 Stop 不向兼容目标写入，也不修改剪贴板。
- 复制后等待一个短主循环间隔，按当前键盘布局解析 Command+V 对应的物理键，再向原进程提交一次按下/松开事件。不切换输入源，不附加 Unicode 字符覆盖；布局无法解析时停止。取消只丢弃尚未提交的粘贴；已提交的文字不撤回。
- 粘贴前查询物理修饰键状态，等待 Shift、Control、Option、Command、Fn 松开；沿用准备完成起1秒的期限，持续按住则以 `modifiers` 阶段停止，文字留在面板和剪贴板。等待通过主循环推进，继续执行原目标和剪贴板检查；发键前再次确认。Caps Lock、数字键盘标志不阻塞，不合成释放用户按键，超时后松键不重投。
- 最终文字留在剪贴板供手动恢复，不读取旧剪贴板内容、不恢复过期内容、不覆盖新的复制操作。
- 自动输入将换行和控制字符转为空格，不生成 Return、Delete 或鼠标事件。

[GitHub 实现比较与采用决策](CURSOR_INPUT_RESEARCH.md) 记录 Handy、VoiceInk、Espanso 的固定版本源码、不同取舍与本次独立实现的依据。

已知密码字段、系统安全输入、明确禁用/只读字段仍拒绝自动输入。权限、通信异常和无法绑定原窗口的情况不会被当作“不支持 AX”而绕过。目标应用若拦截普通粘贴或使用特殊键盘协议，不能保证兼容；此时结果保留在面板。

## 统一投递状态与重复回调

### CLI 状态字段与投递回执

`voiceport --json status` 的 `data.input.method` 表示 GUI 选用的投递路径，不表示文字已经送出。缺少焦点字段的微信主窗口通过保护检查后，与其他兼容控件共用 `clipboard_final`。当前路径提交 Command+V，不打开「编辑」菜单；旧版的 `wechat_menu_paste` 仅用于辨认旧 App 的菜单路径。排查前先核对 `version`、`bundle_path` 与 PID。

`data.input.method` 和 `data.input.failure_stage` 来自 App 保留的投递状态。自动投递开始捕获目标时会重置这两个字段。仅面板听写和 CLI 听写跳过目标捕获，因此字段可能仍显示较早一轮的路径和失败阶段。`data.input.start` 记录最近一次 GUI 启动请求的路径，分配标识后可用 `input.start.session` 与新回执关联。CLI 听写操作始终为 `delivery:"none"`。

`data.input.gui_receipt` 是最近一次 GUI 听写的只读回执。首次 GUI 听写前为 `null`，旧版 App 可能缺少该字段。以 App 实例和 `session` 关联一轮听写，`generation` 标记当前转写尝试。明确被拒绝的 GUI 启动也获得新标识并报告 `not_started`。手动重试保留 `session`、使用新 `generation`，只重试转写并保留原投递证据。CLI 听写与来源测试保留最近的 GUI 回执，CLI 自身仍使用独立 operation ID。

- `automatic` 是原请求的自动输入设置，`method` 和 `failure_stage` 属于该轮。`block_reason` 给出固定原因，例如 `panel_only`、`retry_panel_only`、`start_rejected`、`source_unavailable`、`unsupported_language`、`microphone_denied`、`capture_failed`、`transcription_failed`、`empty_transcript` 或 `cancelled`。自动投递的具体阻止阶段由 `failure_stage` 给出，例如 `focus`、`ownership` 或 `readback`。
- `transcription` 独立标记 `not_started`、`pending`、`completed`、`empty`、`failed` 或 `cancelled`。已完成转写后取消未结算的投递时，转写状态仍表示结果已取得。
- `delivery` 包含下表中的状态和操作计数。对于 `clipboard_final`，选路后仍可能等待、取消或失败。事件提交后的 `submitted` 表示请求已发出，接收效果需要另行检查。

同用户 IPC 的 schema 仍为 1。回执只含进程内标识、固定状态和原因、投递方法及次数。`input.start` 保留启动路径和对应的 `session`，旧的目标 PID 和 bundle 字段返回 `null`。空白最终结果不提交新的输入，有效的已确认 AX 预览仍仅在拥有原范围且未被改动时可安全恢复。

`data.input.gui_diagnostics` 使用相同 `session` 和 `generation`。`capture` 在实际采集封存后报告采集 `generation`、可观测的回调数与帧数，以及 `no_callbacks`、`no_frames`、`digital_silence`、`low_energy`、`signal` 或 `unknown`。信号档基于混音前的采集数据，转写返回空文字时仍保留有声记录。重试保留原录音的采集代次及首帧耗时，后续耗时只属于新的尝试。

`timings` 使用单调时钟，单位为毫秒。`capture_first_frame_ms` 从 Recorder 开始准备到首个有效输入帧，`preparation_ms` 从自动输入目标捕获开始到准备结算，`transcription_ms` 从转写 worker 接手到该次结果，`delivery_ms` 从非空最终结果进入自动投递到投递尝试结算。阶段可以重叠。缺少开始或结束证据时返回 `null`。

### 统一回执与重复结果

所有输入路径使用同一份不含正文的 `DeliveryReceipt`；`Delivery::receipt()` 在目标会话结束或失败后继续保留回执。`compatibility_delivery_probe` 的 `deliveryReceipt` 输出同一结构，不会因为合成接收器另行报告收到文字而升级生产路径的确认级别。实现见 [`Delivery::accept_capture` / `receipt`](../crates/voiceport-macos/src/delivery.rs)、[`ClipboardTarget::paste`](../crates/voiceport-macos/src/delivery/native/clipboard.rs) 和 [App `status` 输出](../crates/voiceport-macos/src/app/control.rs)。

| 状态 | 含义 |
| --- | --- |
| `notSent` | 没有已提交的目标写入操作 |
| `waiting` | 等待焦点准备、修饰键释放或最终粘贴准备，尚未提交目标写入 |
| `awaitingConfirmation` | 已调用写入边界，等待原字段的读回确认 |
| `partiallySent` | 已完成部分操作，仍有后续文字或最终校正；是否读回确认须结合确认计数，终端事件的确认计数始终为零 |
| `confirmed` | 本轮已完成，全部写入操作均由原字段读回确认 |
| `submitted` | 剪贴板粘贴或终端事件已提交，目标文字接收未验证 |
| `uncertain` | 写入边界报错、读回失败，或待确认时被取消；可能已有副作用，停止且不重发 |

`stopped` 独立表示取消或故障已停止本轮；它不抹掉此前的确认或事件提交。`submittedOperations`、`confirmedOperations` 计算写入操作次数，AX 预览、校正和安全恢复也会计入，不能当作字符进度。写入边界可能先完成内部校验再报错；无法证明无副作用时保守标记 `uncertain`，不声称目标一定发生写入。

首次最终结果一经接受，后续重复最终结果和迟到预览均不替换它，也不清除正在发送或等待确认的队列；即使迟到预览超过长度上限也一样。等待辅助功能初始化期间同样保留已接受的最终结果。结果不明时不切换后端、不猜测撤回、不自动重试。取消正在等待确认的输入保留已发生的变化，清掉未发部分；已确认的普通 AX 预览仍只在原范围未变化时恢复。

### 快捷面板的恢复动作

「本次转写」用当前 GUI 尝试的回执显示状态和下一步。`waiting` 与 `awaitingConfirmation` 期间暂不提供展开或复制。`submitted`、`partiallySent` 和 `uncertain` 提示先检查原输入框，只有 `confirmed` 表示已读回。手动重试的新结果只留在面板，原尝试的投递状态仍可用于判断原输入框是否已有文字。

快捷面板的「复制本次文字」每个转写代次最多尝试一次。面板关闭会收起正文，之后仍可展开、选择文字。

### 显式重贴最近听写

重贴在动作入口捕获当前应用、焦点控件和 UTF-16 选区，并在写入和读回时复核现有所有权约束。桌面快捷键在按下时捕获。CLI 调用须指定目标 PID 和 bundle ID，App 在处理请求时校验前台身份并捕获输入框。切换应用或控件、原选区或正文变化、安全输入、密码框、权限不足，以及无法确认写入能力或光标的兼容目标都会阻止后续写入。对支持读回的目标使用现有 AX 或键盘最终投递，保留原有控制字符处理。需手动恢复时可显式复制；拒绝重贴不会替用户更新剪贴板。

原投递处于不明、部分写入或已提交但未确认时，重贴报告「可能重复」并提示先检查目标、再手动处理。手动重试若沿用原尝试已确认的预览写入计数，也显示同一提示。一次显式重贴可能写入后，同一结果后续只允许复制和手动粘贴。一次键入可能由现有保护路径分段，任何失焦或结果不明都会停止后续分段。首次写入前的最后一次校验明确拒绝时，回执为 `notSent`，用户可以重新定位后发起一次新请求。后续分段被拒绝时保留先前的提交与确认计数。调用系统写入接口后若返回错误，仍按 `uncertain` 处理，即使提交计数为零。收到超时或丢失回执时，通过 `status.data.input.repaste` 查看本 App 进程保留的最近一次动作，核对结果代次和投递状态。原 `input.gui_receipt` 继续描述最初的听写投递。

### 听写结果的投递详情

「听写结果」的正文、原目标、投递回执及来源设置按转写代次关联。如果新的启动未接替保留正文，展开详情仍显示这段正文原有的回执，并单独提示最近一次尝试。来源和语言取自开始时固定的设置。目标只显示捕获时的应用，焦点变化后仍指向原应用。音频来源设置显示名称、语言和去向类别。页面不展示原输入框内容、窗口标题、服务地址或凭据。

「原位确认」只有在本轮投递完成，且全部写入由原输入框读回时才显示已确认。部分操作通过读回时显示整轮未确认。投递结果不明、部分写入中断或只有提交回执时，先检查原输入框，再显式复制正文并补写。编辑副本和重试录音在这些结果上不可用，页面不会自动重投。其他已结算结果可复制或编辑临时副本。重试仅生成正文，详情保留原尝试的投递记录。

已保留的断连录音可在确认原来源、语言和去向后单独转写。VoicePort 停止原投递队列，保留已经写入的预览及原回执。标为「不完整」的结果仅供查看或显式复制，禁用编辑副本与重贴，也不进入最近听写历史。详见 [断连恢复](MICROPHONE.md#断连恢复)。

## QQ 等自定义编辑器的焦点识别

某些应用在输入框节点上正确提供 `AXFocused=true`，应用级 `AXFocusedUIElement` 却返回空值。此时先绑定原焦点窗口，仅遍历该窗口的辅助功能结构，寻找唯一明确聚焦的后代控件；不会按位置、可编辑角色或列表顺序猜选输入框。遍历限制为 1024 个不同节点、32 层和 200 毫秒，单次应用辅助功能消息限时 50 毫秒。窗口变化、多个焦点、错误类型、通信失败、扫描未完成或候选失焦均拒绝该解析。应用级焦点正常时继续使用原路径。

开始捕获和后续焦点复核使用同一个解析器；仍须通过原应用、权限、安全输入、用户活动和已有输入拥有范围检查。解析后台应用的焦点元数据不代表允许向后台应用投递，也不会让进行中的听写切换到新点击的位置。

`voiceport-app --diagnose-input` 的显式诊断增加 `focusSource`（`application` 或 `window_focused_descendant`）与 `bounds`。几何使用屏幕左上角为原点的逻辑点，允许其他显示器的负坐标；不可读、非有限或无面积的几何返回 `null`，不影响已有输入能力。这里报告输入控件矩形，不是文字插入光标矩形。不会读取字段正文；非 Orca 应用跳过可能包含草稿的 `AXDescription`。

开发者可用 `cargo run -p voiceport-macos --example input_metadata_probe -- --pid PID` 显式检查某个运行中应用，无需激活窗口、录音、投递或读取字段文字。报告的 `frontmost=false` 明确表示后台元数据观察，不能当作前台听写验收。日常 `voiceport status` 中的 `input.start` 仍是最近一次启动记录，不是实时焦点观察。

2026-11-16 的 QQ 桥接副本实测：应用级焦点返回 `-25212`，原窗口原始树共 462 节点，唯一已聚焦节点为 `AXTextArea`；修复后的生产解析器连续三次返回该节点及 `572×119` 点的矩形，Orca 仍使用 `application` 路径。该验证未激活 QQ、读写输入正文、录音或上传，不代表 QQ 真实语音输入端到端已经验收。

## 验证范围

[共享最终粘贴回归](../crates/voiceport-macos/tests/wechat_paste.rs)覆盖预览不写入、仅提交一次、Unicode/控制字符处理、焦点/权限/安全输入/剪贴板变更、取消和未知失败不重放。[投递回执回归](../crates/voiceport-macos/tests/delivery_receipt.rs)用 `clipboard_submission_is_not_confirmation_and_uncertainty_cannot_retry` 区分提交与确认。原生投递的库内回归另覆盖直接及异步准备后的通用路径选择。

`compatibility_fixture` 是只接受键盘和粘贴、不提供 AX 文字编辑接口的专用合成目标，支持 `custom`、`editable-cell`、`readonly-cell`、`button`、`secure` 五种模式。`compatibility_delivery_probe` 仅在该目标 bundle、PID 和初始合成回执均匹配时调用生产 Delivery；通过目标自身的接收回执核对文字，事件提交不单独算成功。探针不录音、不上传、不向真实用户输入框写入。

`scripts/input_editor_fixture.swift` 使用标准 `NSTextView` 和仅加载本地 HTML 的 `WKWebView`，分别提供 `standard`、`standard-descendant`、`web-textarea`、`web-contenteditable` 模式。`standard-descendant` 让应用级焦点返回空值，并在实际拥有键盘焦点的字段上显式声明 `AXFocused=true`，用于验证窗口后代解析与原范围 AX 输入的衔接。用 `xcrun swiftc` 构建后，只在专用测试 App 包中启动（bundle ID 为 `io.github.forwaryan.voiceport.compatibility-fixture`），传入模式和不存在的绝对回执路径；等待回执的 PID、初始「左|右」及 `ready=true` 匹配，再运行 `compatibility_delivery_probe PID RECEIPT --final-only`。该选项只跳过兼容路径的预览断言，目标保护及最终文字接收判据保持不变。网页就绪还要求原生键盘焦点和 `document.hasFocus()`；接收器会在 45 秒后退出。

2026-11-15 验证记录：

- 初版物理按键在当前布局的专用控件中通过：目标暴露 `AXGroup`，`AXSelectedTextRange`、`AXSelectedText`、`AXValue` 均不可写；生产路径选择 `clipboard_final`。预览与 Stop 不写入，最终回执精确为「左通用输入🙂 第二行右」，没有回车，等待队列清空。
- 随后的 Unicode 字符覆盖方案实际收到普通 `v`，因此废弃；事件构造的静态检查通过不能代替目标接收。
- 去掉 Unicode 覆盖、改为解析真实键盘布局后，OS 主线程只读回归确认美式、Dvorak、Dvorak-QWERTY Command 分别映射到物理键 9、47、9，未切换输入源。该阶段通过 workspace 全目标测试、格式、严格 Clippy、release 构建与 App 打包检查。
- 当时的接收复验遇到 macOS 锁屏：前台为登录窗口，探针在捕获、复制和投递前退出，专用控件初始文字未变。随后在解锁后的新专用目标复验，布局解析版 `AXGroup` 路径真实收到「左通用输入🙂 第二行右」，预览与 Stop 不写入，队列清空。
- 新增单元格反例：原实现拒绝明确 `AXEditable=true` 的 `AXCell`；修复后该目标通过 `clipboard_final` 收到相同精确文字。只读单元格、即使声明可编辑的按钮以及密码字段均被拒绝，目标保持「左|右」。
- 标准 `NSTextView` 通过 `accessibility` 路径，将所选竖线替换为最终文字，前后文字及 UTF-16 插入位置准确。
- 专用 WebKit `textarea` 原先声明三项 AX 接口可写，但替换后网页未变，生产读回检查超时停止；AX 初始化就绪和原生焦点复核排除了这两种测试前置问题。改为首次写入前识别网页并选择 `keyboard_final` 后，`textarea` 和 `contenteditable` 均真实收到「左通用输入🙂 第二行右」，逐段读回通过且队列清空。
- 独立复核发现通用兼容分流可能先于 Orca 描述／终端分类返回；该版本已退回。修复将 Orca 排除在全部通用兼容入口之外，先保留专用分类；定向回归和从实际源码提取的隔离反例确认：能力缺失不绕过不可读描述拒绝，`Terminal input` 仍进入专用分类，其他应用的兼容路径保留。隔离反例只证明分流，不证明真实终端接收。
- 本轮终端覆盖为专用路由和稳定前缀、用户输入、取消及未知失败不重放等回归；未新增真实终端接收验收。上述全局输入修复尚未替换安装 App，安装 App 的实际辅助功能权限需单独确认。

2026-11-16 微信兼容路径调整：缺少焦点字段的微信主窗口改用共享 `clipboard_final`，移除主动展开「编辑」菜单和点击「粘贴」的动作。保留原主窗口、非模态、用户操作、权限、安全输入与剪贴板检查。专用 `AXGroup` 合成接收窗口通过生产路径准确收到「左通用输入🙂 第二行右」，粘贴一次，菜单展开及菜单跟踪均为零；显式弹出菜单的对照记录到了菜单跟踪。该验证覆盖共享投递后端，不代表微信真实聊天框已接收，也未替换已安装 App。

2026-11-16 GitHub 实现调研后的修饰键改进：新增三项策略回归，覆盖五类冲突修饰键、释放后恢复、Caps Lock/数字键盘标志、期限边界及超时后不重投；共享粘贴状态机的失败锁存回归纳入 `ModifiersHeld`。macOS 库168项通过、3项原有忽略，delivery36项与wechat_paste6项通过，三种实际键盘布局解析通过，严格 Clippy、候选打包与无录音无网络启动检查通过。重建后的生产探针在专用合成接收窗口准确插入「左通用输入🙂 第二行右」，pasteCount=1、menuOpenCount=0、menuTrackingCount=0，队列清空。持续物理按键的超时分支由策略/状态机测试覆盖；没有在真实应用中合成按住用户修饰键或据此声称实机超时验收。

2026-11-16 组合策略继续完善：共享物理修饰键判定扩展到最终 Unicode 输入，新增3项原生等待策略与3项队列回归，覆盖就绪等待经 Box 转发、首段/后续段不丢失、等待期间焦点/选区/权限/用户操作变化、部分输入超时锁存和取消不撤回已发文字。macOS 全目标库171项通过、3项既有忽略，delivery39项、storage12项、wechat_paste6项、三布局与原生 harness 通过；格式、严格 Clippy、候选打包/签名/启动检查通过。

同一重建生产探针的四个专用接收目标均准确收到「左通用输入🙂 第二行右」且队列清空：

| 专用目标 | 实际自动路径 | 额外核对 |
| --- | --- | --- |
| 标准 NSTextView | `accessibility` | UTF-16 光标为11、选区长度0，剪贴板版本不变 |
| WebKit textarea | `keyboard_final` | DOM 回执准确，剪贴板版本不变 |
| WebKit contenteditable | `keyboard_final` | DOM 回执准确，剪贴板版本不变 |
| 自定义 AXGroup | `clipboard_final` | 粘贴一次，菜单展开/跟踪均零，剪贴板版本增加一次 |

四个接收进程均已精确退出；测试前确认实际运行 App 空闲。上述结果证明开发探针下的组合选路及接收，不证明已安装 App 的辅助功能权限连续性，不包含真实 QQ/微信正文或真人听写验收。

2026-11-17 两项 P0 整合验证：最终基于 main `3298d74`（包含通用焦点修复与 CLI/Skill 后续功能），整合无菜单粘贴、Unicode 修饰键等待和统一投递回执。workspace 全目标测试通过，其中 macOS 库186项通过／3项既有忽略，delivery39项、独立回执7项、storage12项、最终粘贴6项及三种实际布局检查通过；新增3项 Delivery 回调／保留预览／准备取消测试包含在库测试中。格式、严格 Clippy、examples、专用夹具构建签名、候选打包签名及无录音启动检查通过。

同一生产探针在五个专用目标实际接收准确文字，且重复 final、超长迟到 preview、完成后重复回调均未触发额外投递或改变回执：

| 专用目标 | 焦点来源 | 实际输入路径 | 保留回执 |
| --- | --- | --- | --- |
| 标准 NSTextView | `application` | `accessibility` | `confirmed`，提交／确认各1次 |
| 应用级焦点空值、字段显式聚焦 | `window_focused_descendant` | `accessibility` | `confirmed`，提交／确认各1次 |
| WebKit textarea | `application` | `keyboard_final` | `confirmed`，提交／确认各1次 |
| WebKit contenteditable | `application` | `keyboard_final` | `confirmed`，提交／确认各1次 |
| 自定义 AXGroup | `application` | `clipboard_final` | `submitted`，提交1次／确认0次 |

前四例剪贴板版本不变；AXGroup 只增加一次，pasteCount=1、menuOpenCount=0、menuTrackingCount=0。所有目标精确收到「左通用输入🙂 第二行右」，两个标准框的 UTF-16 光标均为11。五个接收进程均已精确退出。部分写入、未知失败及取消分支由状态机回归覆盖；本轮不声称真实终端、QQ／微信正文或真人听写端到端接收通过。

以上测试进程均已退出；未录音、上传或写入真实用户字段。开发探针的实际授权和接收证明不代表安装 App 的权限连续性或真人语音端到端已验。

单元测试、编译和此专用控件验证不能替代各种编辑器、网页、终端及真人听写端到端验收。
