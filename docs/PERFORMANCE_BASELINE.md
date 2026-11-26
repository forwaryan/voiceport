# macOS 性能基线

产品基线为 `0b6bd0cbaed04d14e66a0c3d8567c2185298f260`（VoicePort `0.3.16`，build `22`）。该提交缺少 Python 驱动和 Rust debug example。#54 同机配对先把冻结探针移植到产品基线，再测 #63 的 freeze-05。两侧均运行实际 `voiceport` CLI、生产 Controller、IPC 和 AppKit 定时器，设备、识别和更新使用合成输入。

## 固定并物化两侧源码

从 #54 配对归档取得 `reference.patch`、`freeze-05.patch` 和独立比较器 `check.py`。两份补丁均以产品基线为 HEAD。freeze-05 的物化树等于 #68 旧 head `1108b427be7b323a585c3de5611d5204976757b3` 的提交树。后续文档修订产生的提交有自己的树 ID，复跑历史配对仍固定下列输入。

| 输入 | SHA-256 | 应用后的 Git tree |
| --- | --- | --- |
| `reference.patch` | `ea96e8281134ad03237146c470e8e4767d30f74b256f851590c3d107e9484258` | `1a1b064c3725a255f3fbc70b09d35baccf6c71ad` |
| `freeze-05.patch` | `cde63d70357d2a81f3c2e1e390767221eb943730242d057e5fa7b23069ad1615` | `3815feb1d0e49d95e0960f64bd6df11735d108b1` |
| `check.py` | `27cb5da02d24bbe7232203e004728ca1399908b59d0f3beef91a8e5ac32d7213` | 在两侧 checkout 外运行 |

基线移植补丁共 14 个文件，带入驱动、样本清单、说明、debug example 及所需的 debug 接线。它导出合成音频和本地文件识别计时入口，在既有隐藏面板刷新路径加计数，并让 debug 音频夹具的 CLI `status` 使用合成辅助功能状态。基线沿用 `showQuickPanel:`、100 ms `Delegate::tick` 和关面板后仍调用 `refresh_contents` 的生产路径。候选的工作台、打开时准备和隐藏时刷新门控属于 freeze-05 的产品差异。保留整份基线探针补丁可复核原计时范围，直接从候选复制整个面板或 Controller 会改变被测基线。

从能够解析上述两个提交的本地仓库 Git 根运行以下命令。将 `inputs` 指向已取得的输入目录。两个新 checkout 只借用本地 Git 对象，应用补丁、写索引与输出证据均在私有目录内进行。

```sh
set -eu
umask 077
repo=$(git rev-parse --show-toplevel)
inputs=/absolute/approved/vp54-pair-inputs
base=0b6bd0cbaed04d14e66a0c3d8567c2185298f260
old_head=1108b427be7b323a585c3de5611d5204976757b3
reference_tree=1a1b064c3725a255f3fbc70b09d35baccf6c71ad
candidate_tree=3815feb1d0e49d95e0960f64bd6df11735d108b1
objects=$(git -C "$repo" rev-parse --path-format=absolute --git-path objects)
test "$(git -C "$repo" rev-parse "${old_head}^{tree}")" = "$candidate_tree"

verify_sha256() {
  test "$(shasum -a 256 "$1" | cut -d ' ' -f 1)" = "$2"
}
verify_sha256 "$inputs/reference.patch" ea96e8281134ad03237146c470e8e4767d30f74b256f851590c3d107e9484258
verify_sha256 "$inputs/freeze-05.patch" cde63d70357d2a81f3c2e1e390767221eb943730242d057e5fa7b23069ad1615
verify_sha256 "$inputs/check.py" 27cb5da02d24bbe7232203e004728ca1399908b59d0f3beef91a8e5ac32d7213

mkdir -p "$repo/.voiceport"
replay=$(mktemp -d "$repo/.voiceport/vp54-replay.XXXXXX")
materialize() (
  side=$1
  tree=$2
  checkout="$replay/$side"
  git init -q --template= --object-format=sha1 "$checkout"
  printf '%s\n' "$objects" > "$checkout/.git/objects/info/alternates"
  cd "$checkout"
  git checkout -q --detach "$base"
  git apply --check --index --whitespace=error "$inputs/$side.patch"
  git apply --index --whitespace=error "$inputs/$side.patch"
  test "$(git write-tree)" = "$tree"
  git read-tree "$base"
  python3 -I "$inputs/check.py" pin --head "$base" --tree "$tree" > "$replay/$side.pin.json"
)
materialize reference "$reference_tree"
materialize freeze-05 "$candidate_tree"
printf '%s\n' "$replay"
```

`git write-tree` 核对全部索引内容。`git read-tree` 将私有索引还原到基线，保留物化文件，复现原报告的未提交补丁状态。`pin` 再核对实际文件字节、模式、额外文件和最小输入。任何一步失败都应保留退出码并停止。裸基线 `git show "$base:scripts/performance_baseline.py"` 会失败，物化成功后两侧均可运行 `python3 -I scripts/performance_baseline.py fixture --help`。

## 在同机顺序复跑

在同一开机期、同一 macOS/SDK、Rust/Cargo、Python、构建环境、电源及负载条件下，使用已就绪的离线依赖和获准的离屏 AppKit 环境。等同 UID 主机锁与各 checkout 锁空闲，先基线、后候选。沿用上段变量，每侧使用全新的证据目录及其独立 target。

```sh
(
  cd "$replay/reference"
  python3 -I "$inputs/check.py" run .voiceport/vp54-30x60 \
    --head "$base" --tree "$reference_tree"
)
(
  cd "$replay/freeze-05"
  python3 -I "$inputs/check.py" run .voiceport/vp54-30x60 \
    --head "$base" --tree "$candidate_tree"
)
python3 -I "$inputs/check.py" compare \
  "$replay/reference/.voiceport/vp54-30x60" \
  "$replay/freeze-05/.voiceport/vp54-30x60" \
  "$replay/freeze-05/fixtures/performance/samples.json" \
  --head "$base" --tree "$candidate_tree" > "$replay/pair-comparison.json"
```

`run` 在两侧实际调用 `scripts/performance_baseline.py fixture --detached --runs 30 --idle-seconds 60 --expected-head "$base"`，另传各自的 `--evidence-dir`。驱动持有双锁，以 `cargo build --offline --locked` 构建 debug example 和 CLI，再依序启动 `headless`、`detached` 两个进程。保留 `report.json`、各检查的 stdout/stderr、`comparison-input.json` 和 `pair-comparison.json`。两侧均须核对源码与环境的运行前后摘要、二进制与样本清单身份、三项实际退出码和清理。锁冲突应记录为未启动，待锁释放后换全新证据目录。缩减 `--runs` 或 `--idle-seconds` 的直接驱动运行只用于 smoke。最近秩 `ceil(p × n)` 在 30 次时取第 15 项和第 29 项。

## 指标边界

| 场景 | 起止和判据 |
| --- | --- |
| 受阻 IPC | 合成设备确认进入打开门闩后，从发起实际 CLI `status` 子进程到读回回复。包含进程启动、私有 socket 和 100 ms 主线程调度。逐轮校验服务 PID、`permissions`、`opening_microphone=true`，再发 Stop 并读回取消。 |
| 首个有效状态 | 第二次 CLI Start 放行合成设备。分别记录发出 Start 到读回接纳及首次观察到 Controller `Recording`。合成音频电平到达后才发出 Stop。 |
| 停止至终稿 | 从发出 Stop 分别计至 AAC 已封装且验证、Controller `Ready` 和 CLI `dictation result` 返回固定合成终稿。CLI 听写为 `delivery=none`。 |
| 投递确认 | 独立 GUI Controller 夹具注入终稿，至确认 `Confirmed` 和自有合成目标内容。初始 `A|B` 最终为 `A合成回执 VoicePortB`，写入一次。该同步计时属于另一会话。 |
| 隐藏刷新 | `headless` 完成听写后测面板未创建的 60 秒窗口。`detached` 释放并收束双门闩后测已创建且隐藏的 60 秒窗口。计数只覆盖快面板内容刷新。 |
| CPU、唤醒、内存 | Darwin `proc_pid_rusage` 给出分窗口的 user＋system 单核 CPU 百分比、两种内核唤醒增量、结束 RSS 和被测进程存续峰值 phys footprint。峰值含该进程在窗口前的工作，CLI 子进程内存另属其进程。 |

`headless` 不创建 `NSApplication`、状态栏项或窗口。`detached` 预建脱离窗口的快捷面板并预跑一次，再测 30 次普通路径。合成设备打开和更新查询均进入门闩后，再在同一个至少 2 秒的阻塞窗口内测 30 次。计时从调用离屏刷新探针至滚动复位且打开按钮通过 `enabled` 和无 window 的断言。基线走 `owner.refresh()`，候选额外走 `prepare_quick_panel(panel)`。因此 `detached_probe_source_equal=false`，两侧离屏 p95 各自报告，不能直接相减、算改进率或套用点击预算。

历史配对的受阻 `status` p95 为 `123.783 / 123.332 ms`（基线 / 候选），两侧均在 `250 ms` 预算内。已创建且隐藏面板的 60 秒刷新为 `600 / 0`，未创建面板为 `0 / 0`。阶段耗时及单窗口 CPU、唤醒、内存是观测值。隐藏刷新计数的入口见 [quick_panel.rs](../crates/voiceport-macos/src/app/quick_panel.rs)。听写提示层、电平和历史各有自己的刷新入口与可见性条件，需各自观测。

真实状态栏点击需在获准的专用隔离图形会话中另测，从系统分发状态栏点击事件计到实际弹层可见且目标控件可交互。每轮关闭并复位，在两侧用同样的合成注入方法各测 30 次暖态无阻塞、30 次设备和更新各延迟 2 秒，并单记首次构建。真实点击 p95 的原型预算是 `100 ms`。#54 的离屏配对未覆盖事件投递、弹层展示、焦点切换、真实设备枚举或点击首屏。

## 中文和中英混合语音

[样本清单](../fixtures/performance/samples.json) 固定四条原创拟制语句、顺序、分段、声音和语言。清单语句采用 CC0-1.0，可公开复用。#54 配对记录了清单摘要，合成听写的音频来自夹具，终稿为固定字符串。以下文件识别实验使用 eSpeak NG 1.52.0 在本地生成普通话和英语片段，留存合成器及各 WAV 的精确摘要。`cmn` 与 `en-us` 交替模拟中英混排，跨版本比较复用同一组实际 WAV。自然口音、真实环境和跨语种产品准确率需用各自的样本评估。

在包含探针的 checkout 根运行。合成一次，随后给每侧 `model` 传同一份样本目录、模型和 runtime，分别指定一致的轮数、对应的 `--expected-head` 和新证据目录。下例对应上述以基线为 HEAD 的物化树。

```sh
python3 -I scripts/performance_baseline.py synth \
  --espeak /absolute/approved/espeak-ng \
  --evidence-dir .voiceport/perf-samples-01

python3 -I scripts/performance_baseline.py model \
  --model-id orca:paraformer-bilingual-zh-en \
  --model-dir /absolute/approved/model \
  --runtime-dir /absolute/approved/runtime \
  --samples-dir /absolute/path/to/perf-samples-01 \
  --expected-head 0b6bd0cbaed04d14e66a0c3d8567c2185298f260 \
  --runs 30 \
  --evidence-dir .voiceport/perf-model-01
```

只传已获授权、已就绪的本地模型和运行时。按[固定目录](../crates/voiceport-macos/src/model_library/catalog.rs)核对 ID、版本和资产。脚本留存模型、dylib 和样本文件的大小、SHA-256 及文件输入起止一致性。CER 对参考和假设应用 Unicode NFKC、完整 casefold，移除空白及 Unicode P/Z 类别，保留拉丁字母、汉字、数字和其他符号。按 Unicode 码点计算 Levenshtein 插入、删除、替换的最小距离，以归一化参考码点数为分母，重复轮次分别累加分子和分母。保留简繁、数字写法和拼词差异。模型失败时保留日志，套件退出非零。

每个样本的进程冷态为 30 个新进程中的第一次识别。进程热态在另一个新进程中预跑一轮，再测 30 次。每轮观察 `Session::new`（路径检查、dylib 打开、识别器创建）、读入和送帧、flush 与终稿，并收集首个非空局部结果。无局部结果时该指标为 `n=0`。进程冷态保留 OS 页缓存，文件摘要核对会先读取资产，热态每轮仍创建识别器。使用真实模型的 Stop 至终稿需以按时序供给的合成流和显式 Stop 标记另测。#54 配对未测真模型、CER、真实模型 Stop 延迟或预热收益。

## 预热是否值得

[生产本地路径](../crates/voiceport-macos/src/local_runtime.rs) 的 `transcribe_live` 和 `transcribe_file` 都先调用 `Session::new`。流式来源在开始听写后启动工作，离线来源等缓存封装后才提交。`Native::open` 调用 [C++ 桥](../crates/voiceport-macos/src/local_runtime/bridge.cc)，打开两份 dylib 并创建 recognizer。作用域结束时销毁 recognizer 并 `dlclose`。取得配对真模型结果后，可对照各样本冷、热态的 `open_ms`、`final_ms`、`recognizer_open_share` 和峰值内存，评估构造成本。要量化保留识别器的收益，还需实际测复用路径。公开参考见 [CC0 1.0](https://creativecommons.org/publicdomain/zero/1.0/) 和 [eSpeak NG 1.52.0](https://github.com/espeak-ng/espeak-ng/releases/tag/1.52.0)。
