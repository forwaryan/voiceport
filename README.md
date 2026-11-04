# VoicePort

macOS 全局语音输入工具：使用自己的 Codex / ChatGPT 登录调用云端听写，将文字输入 Orca 等 coding 软件。用户检查后自行发送。

当前可运行版本为原生 Swift/macOS 首版；VP-006 的完整离线独立验收已通过，真实录音、账号转写和 Orca 接收待用户参与验证。产品目标同时支持 macOS 和 Windows，当前优先 macOS；后续建议逐步引入 Rust 共享核心与平台适配，详见 [跨平台路线](docs/CROSS_PLATFORM.md)，Rust/Windows 版本尚未实现。方案与实施记录见 [完整方案](docs/SOLUTION.md)、[Loop 规则](docs/LOOP_METHODS.md)、[任务计划](task_plan.md)、[进度](progress.md) 和 [验证与解除条件](docs/VALIDATION.md)。

## 构建与启动

需要 macOS 13+ 和 Swift 6 Command Line Tools。本机 arm64 构建；无第三方运行时依赖。

```bash
bash scripts/package_app.sh
open dist/VoicePort.app --args --show-window
```

应用常驻菜单栏。点好目标输入框，按 **Control + Option + Space** 开始录音，再按一次结束并转写。也可以使用菜单开始/停止。首次录音申请麦克风权限；自动粘贴需在结果/设置面板授予辅助功能权限。冲突时切换为 Control + Shift + Space。

登录由 Codex 管理，请先在自己的终端运行 `codex login`。首版读取 `CODEX_HOME/auth.json`，默认 `~/.codex/auth.json`，不修改该文件。只支持 ChatGPT 文件登录；API key 模式、仅存于 Keychain 的登录和应用内 token 刷新尚未支持。

录音结束后上传至 ChatGPT。原应用、焦点和剪贴板仍匹配，且能确认控件已启用和支持 AXValue 写入时才发送 Command + V；不发送 Return。无法确认这些能力的编辑器会保留结果，可在菜单的“结果与设置”中复制；这不表示该编辑器不能键盘输入。自动粘贴把换行和控制字符转为空格，手动复制保留原文。粘贴事件成功不等于每个输入控件都已接收文字。

## CLI

CLI 名为 `voiceport-cli`，避免在 macOS 大小写不敏感磁盘上与应用 `VoicePort` 冲突。

```bash
swift run voiceport-cli --help
swift run voiceport-cli demo
swift run voiceport-cli doctor
swift run voiceport-cli transcribe /path/to/audio.m4a --language zh
```

`demo` 明确为离线模拟，不录音、不读取凭据、不调用云端。`doctor` 只报告登录文件是否存在。可使用 `--auth /path/to/auth.json` 指定自己的登录文件，勿把凭据复制到仓库。支持 m4a/mp4、wav、mp3、webm、ogg、flac，最大 20 MB。

## 验证

```bash
# 核心认证、请求/响应、异步网络、取消和投递反例；不需要 Xcode 测试框架
bash scripts/test_core.sh

# 控制器/投递、默认目标采集、隔离录音存储与子进程锁；不录音、不访问一般剪贴板
bash scripts/test_app.sh

# Loop 状态恢复、依赖、暂停和迟到结果
python3 -I -m unittest discover -s Tests/LoopTests -v

# Release 构建、CLI、.app 打包/签名和启动检查
python3 -I scripts/verify.py --app-smoke
```

本机 Command Line Tools 不含 XCTest / Swift Testing，因此核心检查用 swiftc 编译独立断言程序。完整报告和日志保存在被 Git 忽略的 `.voiceport/evidence/`；这套离线验证不调用真实账号，不替代麦克风/Orca 实测。

## 开发 loop

```bash
python3 -I scripts/loop_state.py status
```

任务定义在 `automation/tasks.json`，执行状态在 `.voiceport/loop-state.json`。主协调者一次选择一个依赖满足的产品任务，通过 Orca 分派不同真实开发、独立验证和按需评估会话；开发自检后冻结完整输入，再独立验收，返修回到原角色上下文。工作会话完成事件唤醒协调者，动态 `/loop` 作恢复兜底。状态工具只维护账本，宿主退出后不承诺继续运行。角色、暂停、外部阻塞和停止规则见 [Loop 契约](docs/LOOP_METHODS.md)。

## 数据与已知边界

- 音频上传到 `chatgpt.com/backend-api/transcribe`，使用当前用户自己的 ChatGPT 登录；接口未公开承诺稳定性和扣量规则。
- 与 Codex App 的模型、识别质量和用量是否一致，尚需同音频对照，不能仅凭接口地址判断。
- 默认无转写历史或遥测，结果仅保留在内存。录音位于 `~/Library/Caches/io.voiceport.recordings-v1`；管理目录使用 POSIX 0700，录音、归属标记和占用文件使用 0600。成功、取消和正常退出时尝试删除本次录音，失败时为手动重试保留。
- 普通启动只尝试清理创建时间严格超过 24 小时、归属标记有效且未被活动会话占用的残留；停止录音后的上传和重试仍持有占用锁。`--smoke-test` 跳过清理。未知内容、链接、异常权限/时间、旧版无标记目录均保留，清理失败也可能留下文件；这不是到时必删的保证。
- 当前验证覆盖本机隔离目录中的归属、POSIX 权限和跨进程占用；ACL、断电持久性、真实编码器输出及其他 macOS/文件系统兼容性仍待验证。
- 开发包采用本机 ad-hoc 签名，尚未公证；更新二进制后系统可能要求重新授权。Intel、所有应用输入框兼容性尚未全面验证。
- Orca 原生麦克风按钮的后端适配属于后续工作；首版使用 VoicePort 独立快捷键。

参考及许可见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。
