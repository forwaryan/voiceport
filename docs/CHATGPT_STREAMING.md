# ChatGPT 账号实时转写验证

验证日期：2026-11-14。

**现有 ChatGPT 文件登录可以建立实时语音会话，并在音频尚未发送完时返回中文转写片段。** 本次通过隔离原型验证了这一点；当前 VoicePort App 尚未接入这个实验来源。

## 实测结果

使用 macOS 本地合成的固定中文语音，通过 WebRTC RTP 实时发送。未读取麦克风、未使用 OpenAI API key，也没有向用户的输入框写入文字。

| 项目 | 结果 |
| --- | --- |
| 测试音频 | 中文，13.2456 秒，单声道 24 kHz |
| 账号认证 | 现有 Codex ChatGPT OAuth 文件登录 |
| 创建会话 | HTTP 201，返回 SDP answer |
| 音频连接 | WebRTC ICE / DTLS connected |
| 字幕连接 | 官方 v3 sideband WebSocket 建连成功 |
| 输入文字事件 | 29 个 `input_transcript.added` |
| 音频结束前到达的文字事件 | 27 个 |
| 第一段文字延迟 | 2.838 秒，从开始发送测试音频计时 |
| 最终结果 | 收到 `turn.done`，`role=user` |
| 音频传输 | 667 个 RTP 包，52,739 字节 payload |

延迟是单次合成语音实测，不能作为日常麦克风输入的性能保证。

部分片段的到达时间：

```text
 2.838s  这是一次实时
 3.164s  语音
 3.205s  转
 3.395s  文字
 3.960s  测试
 ...
13.128s  点
13.246s  测试音频结束
13.795s  ,测试
14.570s  结束
```

拼接后的输入转写：

> 这是一次实时语音转文字测试,我还没有说完这句话,文字就应该逐步出现,现在继续说第二句话,检查中文识别和标点,测试结束

这是服务器在语音输入过程中产生的增量文字，不是等待整段完成，也不是把录音切成文件反复调用批量接口。

## 已核对的协议

现有批量实现仍是 `crates/voiceport-core/src/client.rs` 中的：

```text
POST https://chatgpt.com/backend-api/transcribe
完整文件 multipart -> JSON text
```

本次使用另一条实时会话路径：

```text
现有 ChatGPT OAuth
  -> POST https://chatgpt.com/backend-api/codex/realtime/calls?intent=quicksilver&architecture=avas
     JSON {sdp, session}
  -> SDP answer + Location 中的 call_id
  -> WebRTC RTP 发送音频
  -> wss://api.openai.com/v1/live/<call_id> 接收字幕
     input_transcript.added -> item.text（增量片段）
     turn.done -> turn.role + turn.transcript（完成结果）
```

`api.openai.com` 在这里是官方 Codex 对已创建账号语音会话使用的 sideband 地址。探针使用相同 ChatGPT 账号认证，没有 API key，也没有调用付费 API key 转写会话。此结果不说明账号语音额度无限或没有订阅限制。

固定的会话配置采用官方 v3 `gpt-live-1.1-codex` 对话语音协议。探针禁用工具执行，没有代理任务或项目上下文，也没有把任何账号凭据交给浏览器页面。

浏览器实验使用 `bundlePolicy=max-bundle`，收集本地 ICE candidates 后提交 offer，返回后立即应用 answer，并创建 negotiated datachannel 0。连接成功后，字幕从独立 sideband 接收；datachannel 本身未返回字幕。首次未连接的实验和后续成功实验同时改变了 offer、时序和 channel 配置，因此尚不能把首次失败归因于其中一个因素。

## 原型和运行边界

- `scripts/chatgpt_realtime_probe.py`：默认离线；显式 `--connect` 才读取登录并进行一次固定第一方信令请求。无重试、不跟随跳转。SDP 和私有 call reference 保存到新的权限 0700 目录，文件权限 0600；凭据不复制到输出。
- `scripts/chatgpt_realtime_observer.py`：默认离线；显式 `--observe` 才加入已有 call 的固定 sideband。只观察最多 25 秒，保存有限的字幕与事件摘要，不发送音频、session update 或工具结果。调用标识不进入其报告或 stdout。
- `Tests/StreamingTests/`：16 项离线测试，使用合成凭据和网络 mock。

默认离线检查和测试：

```bash
python3 -I scripts/chatgpt_realtime_probe.py
python3 -I scripts/chatgpt_realtime_observer.py
python3 -I -B -m unittest discover -s Tests/StreamingTests -v
```

实验的信令/观察步骤（需要自行持有仍然活动的 WebRTC peer；以下两个 CLI 不会创建音频 peer）：

```bash
python3 -I scripts/chatgpt_realtime_probe.py \
  --connect --offer /absolute/path/offer.sdp \
  --output /absolute/path/new-call-directory

# 立即在生成 offer 的同一 peer 上应用 answer.sdp，等待连接成功。
# 保持 peer 活动，再开启观察；同时由该 peer 发送经授权的测试音频。
python3 -I scripts/chatgpt_realtime_observer.py \
  --observe --call-reference /absolute/path/new-call-directory/call-reference.json \
  --output /absolute/path/new-observation-directory
```

只有观察器的实时模式需要已安装的 `websocket-client`；其默认离线命令不加载该依赖。私有 call reference 是短期实验状态，不应加入 Git 或公开文档。观察器的 `audio_uploaded=false` 只描述观察器进程；整个实验由浏览器的 RTP 通道发送了合成音频。

本机证据保存在被 Git 忽略的 `.voiceport/research/chatgpt-streaming-20261114/`：

- `attempt-01/`：信令成功，ICE 未连接，无音频。
- `attempt-02/`：连接成功并发送合成音频，仅观察 datachannel，未收到字幕。
- `attempt-03/` + `attempt-03-sideband/`：sideband 成功，收到音频结束前的输入字幕事件；最初的摘要未提取嵌套 `item.text`。
- `attempt-04/verification.json` + `attempt-04-sideband/report.json`：已提取真实中文片段和完成结果，验证音频结束前持续出字。

每次后续实验都修正了已定位的缺口，事前记录了目的；没有对未变的失败自动重试。实验 peer、音频 context 和观察器均已关闭。

## 产品接入建议

保留当前“说完后整段转写”，新增独立的“ChatGPT 实时（实验）”来源。这条实时路径已经有本机账号验证依据，不必把下载本地 Paraformer 模型作为先决条件。

接入尚需完成：

1. 在原生 App 中实现 WebRTC 音频传输与 sideband 的启动、取消、结束和断线处理。当前实验依赖独立浏览器，不是已经打包进 App 的功能。
2. 将 `input_transcript.added.item.text` 映射为用户 partial，将 `turn.done` 中 `role=user` 的文字映射为用户 final，过滤 assistant 回复。v3 片段应顺序拼接。
3. 复用现有目标捕获和终端稳定前缀写入规则；最终修订可能无法直接覆盖已经发送给终端的内容。
4. 核对语音对话协议产生 assistant 音频、会话时长和账号额度的行为。隔离实验中未播放 assistant 音频；纯转写模式和关闭服务端回复尚未验证。
5. 为启动失败保留现有批量路径，清楚展示来源和降级状态，避免将没有实时能力的来源标记为实时。

当前外部 JSONL v1 插件只支持完整音频和单条 final/error，不能仅将 manifest 的 `streaming` 改成 true 来接入。适合新增原生内置来源，或先明确设计具备 partial/取消/会话生命周期的新插件协议。

## 固定版本参考

主要依据是官方 Codex，而不是执行第三方下载代码：

- [官方 Codex commit](https://github.com/openai/codex/commit/7924f95421a326bb66426a3ce232b72550948470)
- [call 创建与 ChatGPT 请求形状](https://github.com/openai/codex/blob/7924f95421a326bb66426a3ce232b72550948470/codex-rs/codex-api/src/endpoint/realtime_call.rs)
- [sideband URL、认证和会话初始化](https://github.com/openai/codex/blob/7924f95421a326bb66426a3ce232b72550948470/codex-rs/codex-api/src/endpoint/realtime_websocket/methods.rs#L946)
- [官方核心的 call + sideband 组合](https://github.com/openai/codex/blob/7924f95421a326bb66426a3ce232b72550948470/codex-rs/core/src/realtime_conversation.rs#L790)
- [v3 增量文字与最终结果解析](https://github.com/openai/codex/blob/7924f95421a326bb66426a3ce232b72550948470/codex-rs/codex-api/src/endpoint/realtime_websocket/protocol_frameless_bidi.rs#L48)
- [社区浏览器 ICE/channel 实现参考](https://github.com/opencoredev/login-with-chatgpt/blob/a1330697f35a1c6429009953acc8ec3e2f9f1f1c/packages/core/src/realtime-browser.ts#L118)
- [codexbot 的账号实时语音参考](https://github.com/10K-Digital/codexbot/blob/f870fd9f3111b0fc35d043bbe51520f396852cdb/live-voice.mjs)
- [codex-transcribe 的批量实现参考](https://github.com/nakasyou/codex-transcribe/tree/7b15b53b216163dcb2c49556c99f9b7716b0baad)

社区 `/realtime/wm` 路线使用 ChatGPT Web session bearer，与本次 Codex OAuth 路线不同；分段批量转写也不等于上述实时会话。两者均未被混作本次验证结果。
