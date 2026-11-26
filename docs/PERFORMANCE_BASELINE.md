# macOS 性能基线

基线源为 `0b6bd0cbaed04d14e66a0c3d8567c2185298f260`（VoicePort `0.3.16`，build `22`）。同一台机器、同一构建配置先测这版，再在集成候选上复跑。脚本使用 debug example、实际 `voiceport` CLI 和生产 Controller、IPC、AppKit 定时器。常规构建、安装包和 CLI 参数无需接入探针。

## 运行

```sh
python3 -I scripts/performance_baseline.py fixture \
  --evidence-dir .voiceport/perf-baseline-01
```

默认执行 30 轮合成听写、每轮一次受阻时的 CLI `status`，然后定时测 60 秒闲置。脚本使用项目的同 UID 主机锁与 checkout 锁、锁定且离线的 Cargo 依赖，以及该次运行独有的 target。`--target-dir` 可复用本 checkout `.voiceport` 下专用 target，仍执行增量编译。比较新提交时显式传 `--expected-head`。每次指定全新的证据目录。报告记录源 HEAD、锁后执行的命令、退出码、日志摘要、机器与二进制摘要。逐轮原始 JSONL 保存在对应 stdout，摘要使用最近秩 `ceil(p × n)`，30 次时取第 15 项和第 29 项。`--runs` 和 `--idle-seconds` 可缩到 smoke 范围，报告保留样本数及实际窗口长度。

默认模式不创建 `NSApplication`、状态栏项或窗口。输入来自合成设备和缓存，更新请求由内存夹具托管，CLI 通过本轮创建的私有 Unix socket 调度。该夹具的麦克风、语音识别和辅助功能状态均为合成。实际 CLI `status` 的延迟包含创建子进程、IPC 和 100 ms UI 调度的等待。每轮先留住音频打开门闩，确认 `status` 返回同一 PID、`permissions` 和 `opening_microphone=true`，再用 CLI 停止并读回取消。第二次启动允许合成设备接入，观察阶段至完成。

在授权的离屏 AppKit 环境可加 `--detached`。它创建脱离窗口的快捷面板控件，测量刷新后打开工作台的按钮可用所需时间，另在合成音频打开和更新查询都已进入工作线程的条件下重复 30 次，并保持两个门闩至少 2 秒。最后用独立进程测 60 秒已创建但隐藏的面板。真实菜单点击至可操作要在专用隔离图形会话从状态栏事件起计，到实际弹层中控件可操作为止。离屏测量不含 AppKit 事件投递、弹层展示、焦点切换或真实设备枚举。

## 指标边界

| 场景 | 起止和判据 |
| --- | --- |
| 首个有效状态 | 从发出 CLI Start 至第一次观察到 Controller `Recording`。接纳回复 `permissions` 单列。合成音频电平到达后才发出 Stop。 |
| 停止至终稿 | 从发出 Stop 至音频已封装且验证、Controller `Ready`，再至 CLI `dictation result` 返回固定合成终稿。三项分别报告。CLI 听写的 `delivery=none` 保持原语义。 |
| 投递确认 | 独立 GUI Controller 夹具注入终稿，至读取 `Confirmed` 及自有合成选择的准确内容。初始 `A|B` 最终为 `A合成回执 VoicePortB`，写入一次。该同步路径包含应用终稿和确认，和上一个 CLI 会话的时间不能相减。 |
| 隐藏刷新 | 面板内容刷新调用计数。另报 Darwin `proc_pid_rusage` 在 60 秒内的 user＋system CPU（单核百分比）、package idle wakeups、interrupt wakeups、结束 RSS 及进程存续峰值 phys footprint。两个唤醒计数按内核归因分别报告。 |
| 内存 | 使用被测进程的 phys footprint 高水位。CLI 子进程内存另属其进程。热态多次同进程的高水位是该进程累计值。 |

基线生产路径在 [app.rs](../crates/voiceport-macos/src/app.rs) 中把状态栏按钮绑到 `showQuickPanel:`，然后在 [quick_panel.rs](../crates/voiceport-macos/src/app/quick_panel.rs) 中构建、刷新并显示弹层。100 ms `Delegate::tick` 在同一线程处理音频状态、IPC 与刷新。快捷面板曾经建立后，`refresh_quick_panel` 在弹层关闭时重置历史后仍进入 `refresh_contents`。隐藏提示层另有可见性门，不能用面板计数推定提示层电平的像素渲染。两处计数单位不同。

图形验收时在同机分别量热态无阻塞、设备与更新各延迟 2 秒。核对开过再关闭的快捷面板、听写提示层、电平和历史各自的刷新入口。未创建面板的零次调用不覆盖关闭后场景。真实交互点击 p95 和阻塞 `status` p95 的原型目标分别为 100 ms、250 ms，建议作为同机复跑的候选阈值记录。

## 中文和中英混合语音

[样本清单](../fixtures/performance/samples.json) 固定四条原创拟制语句、顺序、分段、声音和语言。清单语句采用 CC0-1.0，可公开复用。这里没有真实录音。测试使用 eSpeak NG 1.52.0 的本地命令生成普通话和英语片段，生成过程留存合成器摘要及各 WAV 精确摘要。`cmn` 与 `en-us` 交替模拟中英混排，不能推断自然口音、真实环境或跨语种产品准确率。跨版本比较复用生成出来的一套实际 WAV。

```sh
python3 -I scripts/performance_baseline.py synth \
  --espeak /absolute/approved/espeak-ng \
  --evidence-dir .voiceport/perf-samples-01

python3 -I scripts/performance_baseline.py model \
  --model-id orca:paraformer-bilingual-zh-en \
  --model-dir /absolute/approved/model \
  --runtime-dir /absolute/approved/runtime \
  --samples-dir .voiceport/perf-samples-01 \
  --evidence-dir .voiceport/perf-model-01
```

只传已获授权、已就绪的本地模型和运行时。按[固定目录](../crates/voiceport-macos/src/model_library/catalog.rs)核对可用 ID、版本和资产。脚本留存实际模型、dylib 和样本文件的大小、SHA-256，以及文件输入起止一致性。CER 对参考和假设应用 Unicode NFKC、完整 casefold，移除空白及 Unicode P/Z 类别，保留拉丁字母、汉字、数字和其他符号。按 Unicode 码点计算 Levenshtein 插入、删除、替换的最小距离，以归一化参考码点数为分母，重复轮次用分子与分母分别求和。保留简繁差异、数字写法差异和拼词差异。不用合成终稿的匹配率代替 ASR CER。模型失败时保留该次日志，套件退出非零。

每个样本的进程冷态为 30 个新进程中的第一次识别。进程热态在一个新进程中预跑一轮，再测 30 次。在每次调用内部观察 `Session::new`（路径检查、dylib 打开、识别器创建）、读入和送帧、flush 与终稿，另收集首个非空局部结果。离线模型若无局部结果，则该指标 `n=0`。读文件和 flush 是文件管线边界。带真实模型的 Stop 至终稿须使用按时间供给的合成流和显式 Stop 标记另测，文件读完不能替代。进程冷态没有清 OS 页缓存，文件摘要核对会先读取资产。热态每轮仍创建识别器，故也不能用来推算保留识别器后的真实延迟。

## 预热是否值得

[生产本地路径](../crates/voiceport-macos/src/local_runtime.rs) 的 `transcribe_live` 和 `transcribe_file` 都先调用 `Session::new`。流式来源在开始听写后启动工作，离线来源等缓存封装后才提交。`Native::open` 调用 [C++ 桥](../crates/voiceport-macos/src/local_runtime/bridge.cc)，打开两份 dylib 并创建 recognizer。作用域结束时销毁 recognizer 并 `dlclose`。这证明每轮的构造成本处于关键路径，无法据此判断构造在同机实测中占多少时间或是否划算。获得获准的模型与合成 WAV 后，对照每样本 cold/warm 的 `open_ms`、`final_ms`、`recognizer_open_share` 和峰值内存，再决定是否实施有界预热。模型不可用时，保留 synthetic/IPC/闲置结果，标记 ASR 和预热收益为未测。公开参考为 [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) 和 [eSpeak NG 1.52.0](https://github.com/espeak-ng/espeak-ng/releases/tag/1.52.0)。
