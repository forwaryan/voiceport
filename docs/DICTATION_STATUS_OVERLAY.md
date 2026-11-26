# macOS 听写状态提示

默认开启。菜单栏右键菜单中的「显示听写提示」可关闭，选择会保留。关闭时立即隐藏；重新打开从下一轮听写开始生效。此选项不改变录音、转写或[自动投递策略](INPUT_DELIVERY.md)。

| 通用文字 | 当前 GUI 听写在做什么 |
| --- | --- |
| 正在准备听写 | 已接受本轮请求，等待开始打开麦克风，例如等待权限结果 |
| 正在打开麦克风 | 正在后台打开并核对录音输入 |
| 正在录音 | 输入已接入；设备恢复后回到此阶段，有最近的有效 PCM 峰值时显示粗粒度电平 |
| 正在恢复麦克风 | 本轮输入断开，正在后台尝试接入替代输入；恢复期间暂停电平显示 |
| 麦克风已断开 | 恢复后的输入再次断开，本轮将结束 |
| 正在停止麦克风 | 已请求停止，后台正在释放输入并完成录音文件 |
| 正在转写 | 正在等待最终转写；手动重试也显示此阶段 |
| 正在输入文字 | 非空最终转写的自动投递仍在排队、执行或等待确认；不是目标接收回执 |

提示随当前阶段更新，不为已经结束的同步操作补播动画或延迟隐藏。因此非常短的阶段可能没有单独渲染一帧。源测试、CLI 听写、独立权限检查、清理及完成后均不显示。取消、已观察到的原焦点变化、会话结束和退出会隐藏本轮提示；返回原位置不会复活同轮提示。取消或超时后，如果后台仍占用音频槽，听写结果页显示「正在等待音频设备释放」，开始听写暂不可用。后台实际释放后提示自动消失，可手动重新开始。详见[打开和停止麦克风](MICROPHONE.md#打开和停止麦克风)。

提示是无按钮的原生浮层，不能成为 key/main window，点击穿透。优先采用已知原输入框所在显示器并避开该矩形；只有原窗口几何时优先其显示器。均不可读时，使用开始时的指针所在屏幕，再回退到主显示器。目标显示器没有不遮挡已知输入区域的安全位置时不显示。焦点检测使用原前台应用，以及可用的原窗口／输入控件身份，无额外权限提示。若应用不公开原焦点或几何，只能保护能观察到的身份和位置；不能保证检测同应用内不可见的焦点变化或避开不可读的控件。提示不显示正文、源名称、目标名称或设备标识，不把电平写入回执或日志。浮层隐藏时跳过电平读取。打开系统「减少动态效果」后只显示静态状态图标与文字；各阶段文字同时有原生辅助功能标签。

## 定向验证

在 macOS 上使用专用 `CARGO_TARGET_DIR` 及项目验证锁串行执行以下无界面检查。

```sh
cargo test --offline --locked -p voiceport-macos --lib status_overlay
cargo test --offline --locked -p voiceport-macos --test stop_failure -- --case gui-delivery
```

第一条覆盖状态顺序、旧代次与布局。第二条以无设备的合成 Recorder、转写和投递替身运行实际 Controller 路径。继续用同一独立 target 与验证锁运行后台音频夹具，覆盖取消后槽仍占用、阻塞期间的私有 CLI 回执、释放后的重新开始、同代次的恢复和转写，以及释放阻塞时退出。

```sh
cargo build --offline --locked -p voiceport-cli --bin voiceport-rs \
  -p voiceport-macos --example control_product_fixture
"$CARGO_TARGET_DIR/debug/examples/control_product_fixture" \
  "$CARGO_TARGET_DIR/debug/voiceport-rs" --audio-start
```

该入口使用合成音频、合成权限结果和同 UID 私有 IPC。后台音频夹具不显示原生浮层。

### 原生焦点验收

先为本次测试预留一台已启动的 macOS VM 及其已登录且解锁的 Aqua 控制台。整次测试须独占该客户机的前台，执行者须获准让合成 App 成为该控制台的活动应用。启动前确认没有其他人在该 VM 工作，且运行入口来自 VM 内的 Aqua 会话。VM 检测和会话校验可核对运行位置，独占安排和授权由执行者确认。

在该 VM 内运行 `/usr/sbin/sysctl -n kern.bootsessionuuid` 读取本次启动 UUID，并在 `launchctl print gui/$(id -u)` 的 `security context` 下读取 `asid`。用已确认的两个值运行以下命令。

```sh
python3 -I -B scripts/verify_status_overlay_focus.py \
  --authorized-isolated-vm BOOT_UUID AQUA_SESSION_ID
```

入口使用项目验证锁、离线构建和临时的 `org.voiceport.status-overlay-focus-fixture` 应用包。原生进程在创建 `NSApplication` 前核对虚拟客户机、启动 UUID、当前本地 Aqua 控制台会话和包身份，并在每个检查时再次核对控制台归属。夹具进入正常 AppKit 事件循环，只请求激活自身一次，随后请求真实目标窗口成为 key。合成 `NSTextView` 中只有「左|右」，插入点为 UTF‑16 偏移 1。夹具不发送鼠标或键盘事件，不使用系统剪贴板，也不运行产品应用。

阶段检查要求 `keyWindow` 指向目标且目标为 key、AX 焦点为同一个编辑控件、first responder 和插入点正确。生产浮层从合成快照进入显示路径，各阶段及取消和退出时均须保持这些条件，显示期间还检查面板非 key/main、忽略鼠标、可访问文字和减少动效行为。减少动效和隐藏时验证跳过电平读取。退出 78 表示隔离校验或真实焦点基线未建立，阶段断言失败为非零退出。通过须有实际退出 0 和成功标记，逐项退出、完整日志及进程组清理记录保存在打印的 `.voiceport/evidence/status-overlay-focus.*` 目录。验收覆盖同一合成 App 的 AppKit 焦点及面板鼠标穿透标记。
