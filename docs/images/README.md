# README 配图

两张图通过 `image-use` 的 Codex 后端调用 `image_generation` 工具直接生成。实际返回的图像模型均为 **`gpt-image-2-codex`**，未使用社区风格、参考图或已有图片进行编辑。

画面采用白色背景、深蓝文字与青蓝重点色，突出产品方向：**ChatGPT、微信语音等识别引擎可以替换，快捷键和目标输入流程保持一致。**

这些是产品方向插画，不是应用截图。ChatGPT 标为 `Connected`；微信语音和其他引擎标为 `Planned`，图中同时注明引擎切换尚在规划。它们不代表当前应用已能选择多个转写后端，也不代表某个编辑器、平台或完整语音输入流程已经通过验收。

| 图片 | 用途 | 实际尺寸 | 实际质量 |
| --- | --- | --- | --- |
| [voiceport-hero.png](voiceport-hero.png) | ChatGPT、微信语音与更多引擎汇入 VoicePort，再将文字送到当前光标。 | 1774 × 887 | medium |
| [voiceport-workflow.png](voiceport-workflow.png) | 选择引擎 → 定位光标 → 说话 → 检查文字，最后由使用者自行发送。 | 1774 × 887 | medium |

README 的显示宽度为 880px，原图宽度超过其两倍。文件直接保留后端返回的 PNG，没有进行像素放大或锐化处理。

## 生成方式与实际返回

- 工具：`image-use` 0.34.6。
- 指定后端：`--backend codex`，使用已有的 Codex ChatGPT 文件登录。
- 关闭风格与参考图：`--no-style`，两次生成均未附加图片。
- 请求参数：`--size 3072x1536 --quality high --action generate --quiet --timeout 300`。
- 实际结果：后端将 `quality=high` 改为 `auto`，最终两张图均返回 **1774 × 887、medium、gpt-image-2-codex**。请求尺寸与质量不等于实际输出，以上表格采用实际结果。
- 兼容处理：使用本地临时包装，省略 0.34.6 固定的 `reasoning` 参数，未修改已安装的 CLI。上游 [issue #69](https://github.com/leeguooooo/image-use/issues/69) 记录了同一问题，0.34.7 已包含修复。后续重新生成请使用修复后的版本。

以下保留本次成功生成使用的完整内容提示词。

## 产品概念图提示词

```text
Use the image_generation tool to create a crisp professional conceptual product illustration for VoicePort. This artwork must emphasize a choice of interchangeable speech recognition providers, rather than implying ChatGPT is the whole product. Wide 2:1 landscape, request 3072 by 1536 PNG. Pure white, deep navy typography, blue and cyan accents, smooth anti-aliased vector-like flat shapes, generous whitespace, no sketching, texture, rough edges or tiny text. At top exact title VoicePort and exact subtitle Your voice. Your engine. On the left, three vertically stacked clean provider cards labelled exactly ChatGPT, WeChat Voice, More engines. Each card has a smaller readable status: Connected under ChatGPT, Planned under WeChat Voice, Planned under More engines. Use generic icons only, no company logos. All three paths converge through a clean central VoicePort hub with a microphone, then one shared waveform leads right into a cyan insertion caret in a generic editor. The editor contains only the phrase Your words, right here. Along the bottom a neat shared keyboard shortcut Ctrl + Option + Space, and a readable note Product vision: provider switching is planned. The visual should communicate replace the recognition provider while keeping the same input workflow. This is explicitly a conceptual product direction, not a real screenshot or an already available provider selection interface. Keep every label large and sharply readable.
```

## 使用分镜提示词

```text
Use the image_generation tool to create a crisp professional four-step conceptual workflow illustration for VoicePort, a desktop voice input tool whose product direction is interchangeable recognition providers. Wide 2:1 composition, request 3072 by 1536 PNG. Pure white background, deep navy readable modern sans serif lettering, cyan and blue accents, precise smooth vector-like flat lines, no doodles, grain, fuzzy edges or rough brush marks. Four balanced columns with subtle vertical dividers. Large exact headings 01 CHOOSE, 02 FOCUS, 03 SPEAK, 04 REVIEW. In CHOOSE show three tidy generic cards: ChatGPT with status Connected, WeChat Voice with status Planned, Other engines with status Planned. Do not copy any real settings UI or company logos. In FOCUS show a generic input field with a bright insertion caret and three shortcut keycaps Ctrl, Option, Space. In SPEAK show an elegant microphone, waveform and the same text field receiving a blue word. In REVIEW show a stopped microphone, finished text field and magnifier with the caption You send it. Across the bottom a readable note Product vision: provider switching is planned. The point is choose a provider then reuse one consistent voice input flow; switching providers is a future capability, only ChatGPT is currently connected. Use sparse, large typography, conceptual icons and ample whitespace; no Enter key or automatic sending. This is product direction artwork, not a screenshot or proof of current provider switching.
```
