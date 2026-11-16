# README 素材

README 使用真实产品截图展示界面，再用一张机制图解释“来源 App → 插件 → VoicePort → 目标 App”。两类素材分别标注，避免把示意图当作实际界面或兼容性证据。

## 当前素材

更新日期：2026-11-16。

| 图片 | 类型与内容 | 实际尺寸 | 格式与大小 |
| --- | --- | --- | --- |
| [真实设置界面](voiceport-settings-qq-20261116.webp) | macOS 0.3.8 开发版；已配置 QQ 来源、自动输入与快捷键。 | 1240 × 1104 | 无损 WebP，53,006 字节 |
| [插件转接机制](voiceport-app-bridge-20261116.webp) | 不同来源 App 经各自插件接入，再用于编辑器、终端与聊天输入。 | 1536 × 768 | WebP，61,466 字节 |

截图以 620px 显示，机制图以 880px 显示，窄屏按可用宽度缩放。两张素材合计约 112 KiB。

### 页首徽章

[preview.svg](badges/preview.svg)、[rust.svg](badges/rust.svg)、[macos.svg](badges/macos.svg) 和 [license.svg](badges/license.svg) 为仓库内自绘的静态 SVG，采用深灰标签和彩色值的矩形样式。它们分别对应现有 v0.3.8 开发预览标签、workspace 的 Rust 1.95 要求、macOS 13+ / ARM64 支持范围和项目 MIT 许可。相关信息变化时需同步更新 SVG 与 README 链接；徽章不表示最新正式版本或实时 CI 状态。

页首的居中标题、定位、徽章与导航结构参考 [feishu-cli](https://github.com/riba2534/feishu-cli)，未复用其品牌标志或统计数字。

### 真实截图

截图来自固定安装路径中的 VoicePort，仅截取应用窗口，未拼接或替换控件、文字与状态。截图时没有开始录音、执行来源测试或修改来源配置。

界面展示已经配置的 QQ 来源，不能据此认为 QQ 桥接可以免配置使用或已经完成所有语音验收。接入要求见 [QQ 桥接说明](../../scripts/qq_bridge/README.md)。

### 生成图

- 工具：`image-use` 0.34.6；保留 `auto` 后端，因 chrome-use 未安装，实际使用 Codex。
- 兼容处理：临时包装仅移除旧版固定发送的 `reasoning` 字段，不修改已安装 CLI。相关问题见上游 [issue #69](https://github.com/leeguooooo/image-use/issues/69)。
- 请求：`--size 1536x1024 --format png --no-style --quiet --timeout 300`，无参考图或社区风格，一次生成完成。
- 实际回报：`gpt-image-2-codex`、质量 `medium`、原始 PNG 为 1536 × 1024。后端另有内部提示词改写，下文记录实际提交文本。
- 发布处理：只裁去上下空白，范围为原图 x=0…1536、y=128…896，保留全部内容；以 Pillow quality=90、method=6 转为 WebP。

图中的通用 App 表示可扩展方向，每个来源仍需对应适配器。多条连接表示可选择的来源和输入位置，不表示同时调用多个识别 App 或同时向多个目标写入。具体依赖和兼容性以功能文档为准。

#### 提交的提示词

```text
Use the image_generation tool to render a restrained, precise software documentation figure for VoicePort. This is an explanatory schematic for a serious GitHub README, not a marketing poster. White background, ink-black modern sans-serif typography, thin cool-gray strokes, square-corner or subtly rounded rectangles, one cobalt-blue accent. No gradients, shadows, waves, giant microphones, hands, characters, plants, decorative blobs, serif typography or vendor logos. Landscape canvas 1536 by 1024, with the actual content in a wide horizontal band through the middle and generous whitespace above and below. Show the core idea with a clearly readable left-to-right path: on the left, a group titled SOURCE APPS containing three small generic application cards labeled App A, App B, App C, each with a tiny speech bubble; the middle has three corresponding small adapter cards grouped under PLUGINS, then one modest blue outlined VoicePort hub; on the right, a group titled WHERE YOU TYPE containing three simple generic target cards labeled Editor, Terminal, Chat. Connect each source app to its own plugin with a thin directional line, converge the three plugins into VoicePort, then branch VoicePort to the three targets. Above the figure place the compact exact heading Bring app speech-to-text everywhere. Below the figure place the exact short caption App capability. Plugin adapter. Any input. Use large readable labels and enough space that the route is obvious. Source applications and destination applications are independent; this is about reusing an existing app's speech recognition through an adapter, not merely switching AI models. It is an architecture concept, not a screenshot and not proof of universal compatibility. No credentials, no model metrics, no fake UI controls or download buttons.
```

## 历史素材

[voiceport-hero.png](voiceport-hero.png) 和 [voiceport-workflow.png](voiceport-workflow.png) 是早期产品方向图，已退出当前 README。它们把来源切换标为规划中，并包含尚未接入的微信语音设想，不能作为当前功能说明。文件保留供历史引用，完整生成记录可通过本文件的 Git 历史查看。

## 文档结构参考

查看日期：2026-11-16。参考以下项目的信息组织，未复制文案、图片或标志：

- [Zed](https://github.com/zed-industries/zed)：简短明确的产品定位，安装与开发入口优先。
- [Tauri](https://github.com/tauri-apps/tauri)：解释架构如何支撑产品价值，区分使用与扩展入口。
- [Home Assistant](https://github.com/home-assistant/core)：将第三方集成组织成共同的用户价值，配合真实产品界面与扩展开发入口。
