# 光标输入实现比较与采用决策

2026-11-16 读取以下固定版本的公开源代码；结论限所列版本及已检查的调用路径。未运行上游代码。研究目的为改进 macOS 原输入位置的文字投递。

## 对比

| 项目与固定版本 | 实际文字投递 | 焦点与确认 | 对 VoicePort 的意义 |
| --- | --- | --- | --- |
| [Handy](https://github.com/cjpais/Handy/tree/aca8985190c3ad7f361d8a80cb30f179513ed690)，MIT | 剪贴板与组合键，或 Enigo `text()`；macOS 按布局解析 Command+V | 面板不激活，尽量保留焦点；所读粘贴入口未绑定原 AX 字段。实验剪贴板事务能观察读取，不能证明文字已插入 | 支持不抢焦点、布局解析、投递状态分层的方向；现有能力继续保留 |
| [VoiceInk](https://github.com/Beingpax/VoiceInk/tree/b5d1be9a7c7377dd059f37bb3e0f791b802d4001)，LICENSE 标识 GPLv3 | 默认剪贴板+Command+V；另有 AppleScript、Unicode 键入、远程键入选项 | MainActor 串行化；区分未投递、已投递、部分投递；诊断明确 insertion=unverified | 保留串行投递、部分结果不重投、元数据诊断的原则；不将全局当前焦点替代原目标绑定 |
| [Espanso](https://github.com/espanso/espanso/tree/80d9e353e3c0a58c77feb72671cec4a61958366f)，GPLv3 | 依平台、应用设置及文本长度选择事件或剪贴板；macOS Unicode 分段 | 注入前等待修饰键缓存释放，约3秒超时后继续；所读核心路径未保存原字段/选区 | 采用有界等待修饰键释放的设计，独立实现并明确超时拒绝 |

`Direct`、Unicode 模拟键入与 AX 直接修改是不同机制。这些项目的直接键入路径不应被描述成 AX 文字写入。VoicePort 已有按目标能力分流：标准 AX 范围编辑、带读回的 Unicode 输入、终端专用追加，以及缺少 AX 编辑接口时的最终粘贴。

## 本次采用

在共享 `clipboard_final` 后端加入修饰键检查，覆盖微信缺少焦点字段的主窗口与其他已通过兼容性检查的目标：

1. 准备最终剪贴板文字后，在主循环中等待 Shift、Control、Option、Command、Fn 松开。使用 macOS `CGEventSourceFlagsState(kCGEventSourceStateHIDSystemState)` 查询硬件状态，不依赖合成事件的 flags 或容易过期的键盘事件缓存。
2. 保留原来的30ms剪贴板等待与自准备完成起1秒的期限；等待期间继续核验原应用、原窗口/字段、权限、安全输入、用户操作和剪贴板版本。无需阻塞主线程或普遍增加正常投递延迟。
3. 创建完整按下/松开事件后、真正发出前再检查一次。持续按键超时以 `modifiers` 阶段停止；最终文字保留在面板和剪贴板。超过期限后松键不会自动重投。
4. 不合成释放用户按住的键。Caps Lock、数字键盘标志不阻塞普通粘贴。

这沿用平台 API 独立实现，没有复制或翻译 GPL 源码。它缩小按键竞争窗口，不能将硬件状态查询与跨进程事件接收变成原子操作。

## 推荐组合与继续完善

默认选择由目标能力决定：标准可写输入框用 AX 精确范围编辑；已知网页/特殊编辑器用带逐段回读的最终 Unicode；缺少 AX 编辑能力但可绑定原目标的控件用最终 Command+V。三者已有同一目标捕获入口；首次修改前选择，写入后不自动换路。

继续完善将修饰键保护扩展到最终 Unicode：每段发送前检查原快照和就绪状态，按键未松开时保留该段并让主循环继续，连续等待1秒后停止；部分已确认文字保留，后续失败或取消只丢弃未发段。物理状态判定与粘贴共享，标准 AX 预览和终端专用语义保持原样。

界面为修饰键冲突、读回未确认、剪贴板更新和粘贴请求失败分别给出恢复提示，要求先检查已有文字。避免让“等待入口”文案暗示仍在展开菜单，也不把事件提交当成接收确认。

## 保留的取舍

- 原 PID、窗口和可确认字段在开始时绑定；不将投递时新出现的前台输入框作为替代目标。
- 已经可能写入后不切后端重投。事件发出、剪贴板读取与目标文字确认分开描述。
- 不引入 AppleScript 自动化权限；不主动展开菜单，不发送 Return 或自动发送消息。
- 不导入旧剪贴板快照/恢复。最终文字继续供手动恢复，新复制操作优先。Handy 普通路径与 Espanso 的固定等待恢复不能提供完整的竞争保护；VoiceInk 的会话标记恢复更谨慎，但不符合本项目当前不读取旧剪贴板的政策。
- 不仅因上游采用60/100/200ms等延迟而调大全局等待；不同应用需要实际接收证据。
- 不扩大 Orca 的专用终端与编辑器兼容入口，不覆盖并行 QQ 焦点解析工作。

## 固定版本源码依据

- Handy：[不激活录音面板](https://github.com/cjpais/Handy/blob/aca8985190c3ad7f361d8a80cb30f179513ed690/src-tauri/src/overlay.rs#L488-L508)、[普通粘贴流程](https://github.com/cjpais/Handy/blob/aca8985190c3ad7f361d8a80cb30f179513ed690/src-tauri/src/clipboard.rs#L74-L164)、[组合键与布局解析](https://github.com/cjpais/Handy/blob/aca8985190c3ad7f361d8a80cb30f179513ed690/src-tauri/src/input.rs#L133-L200)、[Direct 委托 Enigo](https://github.com/cjpais/Handy/blob/aca8985190c3ad7f361d8a80cb30f179513ed690/src-tauri/src/input.rs#L270-L277)、[实验恢复保护](https://github.com/cjpais/Handy/blob/aca8985190c3ad7f361d8a80cb30f179513ed690/src-tauri/src/paste_tx/macos.rs#L157-L175)。
- VoiceInk：[投递串行化与方法选择](https://github.com/Beingpax/VoiceInk/blob/b5d1be9a7c7377dd059f37bb3e0f791b802d4001/VoiceInk/Infrastructure/SystemIntegration/Paste/CursorPaster.swift#L30-L123)、[恢复前会话所有权检查](https://github.com/Beingpax/VoiceInk/blob/b5d1be9a7c7377dd059f37bb3e0f791b802d4001/VoiceInk/Infrastructure/SystemIntegration/Paste/CursorPaster.swift#L216-L253)、[Unicode 分段与部分结果](https://github.com/Beingpax/VoiceInk/blob/b5d1be9a7c7377dd059f37bb3e0f791b802d4001/VoiceInk/Infrastructure/SystemIntegration/Paste/DirectTextTyper.swift#L14-L96)、[元数据诊断与未确认标记](https://github.com/Beingpax/VoiceInk/blob/b5d1be9a7c7377dd059f37bb3e0f791b802d4001/VoiceInk/Infrastructure/SystemIntegration/Paste/PasteFocusDiagnostics.swift#L42-L111)。
- Espanso：[等待中间件接入](https://github.com/espanso/espanso/blob/80d9e353e3c0a58c77feb72671cec4a61958366f/espanso-engine/src/process/default.rs#L123-L125)、[修饰键等待与超时后继续](https://github.com/espanso/espanso/blob/80d9e353e3c0a58c77feb72671cec4a61958366f/espanso-engine/src/process/middleware/delay_modifiers.rs#L27-L84)、[修饰键缓存过期](https://github.com/espanso/espanso/blob/80d9e353e3c0a58c77feb72671cec4a61958366f/espanso/src/cli/worker/engine/funnel/modifier.rs#L29-L75)、[macOS Unicode/组合键](https://github.com/espanso/espanso/blob/80d9e353e3c0a58c77feb72671cec4a61958366f/espanso-inject/src/mac/native.mm#L30-L163)、[后端选择](https://github.com/espanso/espanso/blob/80d9e353e3c0a58c77feb72671cec4a61958366f/espanso-engine/src/dispatch/executor/text_inject.rs#L69-L108)。

具体验证与真实应用验收边界见 [全局光标输入](INPUT_DELIVERY.md)。
