# 插件管理界面设计

插件接入已经实现，但原来的「一个下拉框 + 多个弹窗按钮」难以解释插件与来源的关系，也不适合管理多份配置。本轮将管理入口改为原生插件库与来源列表。

## 参考来源

以下来源是设计参考，未安装或执行第三方脚本，也未复制 Magpie 的页面代码。

- [ceorkm/macos-design-skill](https://github.com/ceorkm/macos-design-skill/blob/8f528a2364f996cd42f02a10b1b27198a74ca2a3/SKILL.md)：已固定版本的 Skill 与布局、交互、视觉三篇参考。采用系统窗口、原生分段导航和搜索、13pt 正文附近的文字层级、系统语义颜色、空状态与渐进展示。两种视图使用分段控件，避免为小工具增加多层导航。
- [Maccy 紧凑设置行](https://github.com/p0deje/Maccy/blob/f0b571e32e4031d7b91ba811f50d0f33e8e5548b/Maccy/Settings/GeneralSettingsPane.swift#L20-L115)：固定提交 `f0b571e32e4031d7b91ba811f50d0f33e8e5548b`，采用窄内容列、标签与控件对齐、较小次级提示和显式 accessibility label。VoicePort 使用 AppKit 原生行，不引入其 Settings/SwiftUI 依赖，也不采用其剪贴板 popup 的 FloatingPanel 行为。
- [CotEditor 设置窗口控制器](https://github.com/coteditor/CotEditor/blob/0bab1dd9a1212bc9d1d755a1f6735e85c40dfec7/Packages/MacUI/Sources/ControlUI/SettingsWindow/SettingsWindowController.swift#L29-L68)：固定提交 `0bab1dd9a1212bc9d1d755a1f6735e85c40dfec7`，普通 titled/closable 设置窗口通过控制器持有并调用 showWindow，不运行 App-wide modal loop。[SettingsWindow](https://github.com/coteditor/CotEditor/blob/0bab1dd9a1212bc9d1d755a1f6735e85c40dfec7/Packages/MacUI/Sources/ControlUI/SettingsWindow/SettingsWindow.swift#L29-L45) 的 Escape 关闭与 [FormatSettingsView](https://github.com/coteditor/CotEditor/blob/0bab1dd9a1212bc9d1d755a1f6735e85c40dfec7/CotEditor/Sources/Settings%20Window/Panes/FormatSettingsView.swift#L83-L205) 的对齐表单、辅助帮助、fitted sheet 用于本轮管理与编辑生命周期。VoicePort 是 accessory 菜单栏 App，需要显式保留 manager/delegate 及来源互斥，不能依赖文档窗口或 SwiftUI scene 的所有权。
- [Magpie 插件管理](https://github.com/yetone/magpie/blob/b1e8eb15a668bbccfcd46977c75602ceaa4d66b3/internal/gui/assets/plugins.js)：`card` 与 `drawInstalled` 将接入方式的发现卡片和已安装条目分开；`optionsEditor` 保留草稿及校验错误。借鉴卡片承载用途和一个主要操作、列表承载实例状态、局部错误不清空已有内容。
- [Magpie 插件样式](https://github.com/yetone/magpie/blob/b1e8eb15a668bbccfcd46977c75602ceaa4d66b3/internal/gui/assets/plugins.css)：参考卡片信息层级、轻量状态、搜索、空状态与次要操作的权重。本项目通过 AppKit 控件与系统语义颜色实现，不引入 WebView/CSS。
- [Magpie 插件页面运行时](https://github.com/yetone/magpie/blob/b1e8eb15a668bbccfcd46977c75602ceaa4d66b3/internal/gui/assets/pluginpages.js)：它另有插件自带页面和权限桥接。本轮不实现这套运行时；VoicePort 当前的插件仍是转写适配器描述，统一由原生表单配置。

固定参考原文在私有 `.voiceport/research/plugin-ui-magpie-20261109/` 与既有 `.voiceport/research/macos-ui-20261108/`，各自保留来源记录。

## 在 VoicePort 中的落实

| 用户任务 | 界面行为 |
| --- | --- |
| 不知道如何接入新服务 | 常驻「添加转写方式」入口，按已有 API 信息或本地清单分流；空来源页直接进入同一引导 |
| 希望 AI 帮忙接入 | 添加引导内「用 AI 帮我接入…」显示可选择的说明；用户点击复制后写入剪贴板，说明含当前 App 的 CLI 绝对路径及离线 Skill/例子入口，不自动执行或修改配置 |
| 有服务地址、模型和密钥 | 直接进入兼容 API 表单，无需 JSON；提供完整转写地址和音频模型示例，提示不能填写官网或控制台链接 |
| 拿到本地插件 | 解释需作者提供 VoicePort 清单及已安装适配器；选文件后审阅程序，确认导入后按完整 ID 精确匹配置顶对应卡片并提示继续配置，避免短 ID 被普通搜索淹没 |
| 只有 GitHub 链接、zip 或模型文件 | 在添加引导说明不能直接导入，需向服务商确认兼容 API，或向作者索取清单和适配器 |
| 选择接入方式 | 紧凑原生行显示用途及主要操作；版本、声明来源、来源数和程序信息在详情中查看 |
| 为同一插件连接多个服务 | 卡片内「填写服务信息」或「配置此插件」直接进入该插件表单，不额外询问插件 |
| 管理已有配置 | 「我的来源」逐行展示名称、音频去向、启用与配置状态，以及与当前配置匹配的独立样例回执；仍可编辑/删除 |
| 查找多个同名来源 | 搜索支持显示名称和稳定 ID；操作绑定稳定 ID，不按标题查找 |
| 填错名称、字段或密钥 | 完整候选配置与所有待保存密钥先做纯校验；名称非空/字节长度、重复身份、字段/容量和密钥长度/控制字符错误均保留当前表单 |
| 多个秘密字段中有无效值 | 事务在创建第一个密钥前校验全部待保存值，不先创建前几个再因后续输入错误回滚 |
| 连续修改来源 | 前一次目录刷新完成前暂停新的添加、编辑、删除和导入，提示稍候；搜索与详情仍可用 |
| 保存后某一步失败 | 重新加载持久配置，显示原始事务错误，并请求目录刷新 |
| 重新加载也失败 | 本次窗口禁止继续写入，提示关闭后重开，避免用旧快照覆盖磁盘 |
| 判断是否可以连接服务 | 「启用」与「连接未检测」分别表达；本页不执行连接检查 |
| 确认音频去向与失败重试 | 「音频去向」取自已保存的 HTTP 地址或 JSONL 插件类型，内置来源在插件「详情」说明。有效的 HTTP 地址只显示站点地址，完整配置仍在编辑表单。本机 HTTP 服务和 JSONL 程序可能另行转发。可重试的失败录音在本机保留，手动重试使用原来源配置，仍可能将音频交给原目的地。远端服务与外部程序按各自规则处理音频，VoicePort 无法确认对方的留存或删除情况。 |
| 查看程序路径等技术信息 | 放在卡片「详情」及导入确认中，避免占满日常列表 |

管理窗口在整个操作期间继续保持 App 的来源管理互斥。浏览、搜索、导入和保存不触发录音、凭据读取、转写程序或网络请求。样例测试通过设置页的独立「测试各来源连通性与耗时…」窗口进行。来源行的「查看测试…」关闭管理窗口并按稳定 ID 定位；打开或定位不会执行测试。测试窗口预先说明合成样例、可能的服务用量和耗时范围，只有点击逐行「测试」或「测试可用来源」才执行，不弹出重复确认。参见[听写界面无障碍验收清单](DICTATION_UX_ACCESSIBILITY.md)。

## 紧凑窗口与所有权（2026-11-09）

管理窗口默认内容尺寸为 700×480 logical points，并按当前屏幕 visibleFrame 留出标题栏和边距。标题只出现在原生标题栏；顶行合并分段视图、按名称/稳定 ID 搜索和「添加转写方式」。插件行约 80pt、来源行约 106pt；来源行将去向、启用或配置问题、样例回执分行。插件版本与程序细节保留在详情，来源 ID 可搜索并可区分同名操作。更多来源或小屏幕使用原生滚动视图。

编辑器使用独立 NSWindow 作为管理窗口拥有的 sheet，普通 HTTP 表单约 620×428pt（字段较多时上限 480pt，表单滚动）；显示名称、转写地址、模型、secure 密钥字段、移除密钥和启用选项按标签对齐。简短示例在字段下方，较长说明由「填写帮助」显式展开，不再用带大图标的 NSAlert accessory form。名称是初始第一响应者，显式设置循环 key-view 顺序，Return 保存、Escape 取消。纯校验失败只更新行内提示，保留全部控件及输入；实际事务返回后重载磁盘并在管理窗口显示结果，避免提交后错误导致旧草稿覆盖新配置。

Delegate 明确保留 manager，manager 保留其 window/目标对象与 editor，editor 保留 library；管理窗口关闭时清除 Delegate 的 manager 与 managing_sources，编辑器结束后清除 library 的 editor，断开所有权环。整个可见管理会话继续阻止录音、控制器来源变更、CLI 来源写入等互斥操作；返回 manage() 不再释放 admission。重复入口聚焦同一窗口。同步来源写入期间另持有 write admission，防止钥匙串交互引起的重入保存/删除；退出请求先 fence 后续回调并等待当前写入返回，再由主循环显式取消草稿、结束 sheet、关闭窗口并等待控制器停机。

管理、编辑、添加分流、AI/Skill 帮助、插件详情、NSOpenPanel、导入和删除确认均不使用 runModal / modal session，也不设置 floating level。每次 sheet 绑定 presentation token，旧/重复 completion 不能释放后来 sheet 的 admission；shutdown 先进入 closed 状态再结束 sheet，阻止迟到回调继续写配置。小型确认使用异步 parent sheet；打开下一步发生在前一 sheet 的 completion 后，不嵌套 App-wide 模态循环。来源测试窗口同样不使用 App-wide 模态循环；状态按来源 ID 更新，不在 100ms 定时刷新时重建列表或改变滚动位置。队列与结果由 Controller 持有，关闭窗口只发取消，后台未释放前仍阻止新任务，重开可继续查看状态。

以上为实现与开发自检范围。普通 window/sheet 的跨 App 层级、close/cancel/reopen、真实键盘遍历、深浅色及缩小屏幕表现仍需新预览 App 的实际观察；未获得专属打包/启动时段前不启动预览，不重试既有 AX 拒绝。

## 验证边界

代码编译、测试、独立审阅与实际视觉体验分别记录。界面布局变化不能仅凭 CI 绿色宣布体验验收。真实 GUI、VoiceOver/键盘遍历、长名称与大量条目、深浅模式截图和用户实际使用仍需单独观察；已有 Accessibility 拒绝不通过其他前端或自动化工具绕过。
