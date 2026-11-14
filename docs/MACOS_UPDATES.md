# macOS 更新与权限身份

日常使用固定的 `~/Applications/VoicePort.app`，bundle ID 始终为 `io.github.forwaryan.voiceport`。版本号可以变化；应用名称、安装位置和代码签名身份不应随一次调试构建改变。`dist/` 中的独立包只作构建产物，避免从多个预览包交替启动日常应用。

## 为什么更新后可能要重新授权

macOS 通过代码签名要求（designated requirement，DR）识别程序是否属于同一个应用。Apple TN2206 说明，新版本应该满足旧版本的 DR。固定 bundle ID 或目录名称本身不能证明签名身份连续。

默认 ad-hoc 签名不使用持久证书，DR 可能绑定会随构建改变的代码哈希。系统设置中的开关即使显示开启，实际运行进程也可能未获辅助功能权限。更换签名证书、在本地构建与正式发行版之间切换，也可能需要重新授权。不能以固定路径或签名检查通过承诺所有系统版本永久免授权。

## 使用固定签名身份打包

先在本机查看可用证书的公开元数据：

```sh
security find-identity -v -p codesigning
```

选定一个长期复用的 Apple Development 或 Developer ID Application 身份，使用其指纹打包；不要每次创建新证书。私钥保留在 Keychain，不放入源码、环境配置文件或 GitHub。正式对外分发还需要按 Apple 流程配置 Developer ID、hardened runtime 和公证；本脚本的证书签名支持本身不等于已公证的发行包。

```sh
VOICEPORT_CODESIGN_IDENTITY='证书指纹' \
VOICEPORT_APP_DIRECTORY="$PWD/dist/release/VoicePort.app" \
bash scripts/package_app.sh
```

已有 release 编译结果时加 `--skip-build`。签名身份由调用方明确选择；指定身份不可用会直接失败，不会静默降级成 ad-hoc。未指定时仍允许无开发者账号的本地构建，但会明确警告权限可能需要重授。

自签名证书在部分本机开发场景也可提供持久签名身份，但适用性取决于系统子策略和信任配置；这里不自动生成证书、信任新根证书、更改 Keychain 搜索列表或修改 TCC 数据库。没有有效身份时，先获得可用签名证书，再接入上面的参数。

## 安装到同一个位置

```sh
python3 -I scripts/install_app.py dist/release/VoicePort.app --quit-idle
open "$HOME/Applications/VoicePort.app"
"$HOME/Applications/VoicePort.app/Contents/MacOS/voiceport" --json app status
```

安装器验证 bundle 身份和完整签名，在目标目录旁暂存完整新包。默认拒绝替换仍运行的 App；`--quit-idle` 只允许正常退出 `state=idle` 且无待处理任务的实例，不强制杀进程。已完成的文字或失败录音可能仍保留在内存中；`ready`、`failed` 或未知状态均中止安装，先保存结果、处理录音，再由用户明确退出。旧包保存到同目录的 `.voiceport-backup-*/VoicePort.app`，替换失败则尝试恢复旧包。安装不会自动启动、录音、上传、清空听写或申请权限。

升级已有安装时，新包必须满足旧包的 DR；身份变化默认中止。首次从 ad-hoc 迁移到固定证书，或明确接受一次需要重新授权的本地 ad-hoc 更新时，可以显式传入 `--allow-signing-change`。它只允许这次文件替换，不授予任何系统权限，也不修改系统信任。后续正常更新应使用同一证书并省略该参数。

`--destination /Applications/VoicePort.app` 可选择系统应用目录，前提是当前用户有写权限；不使用 sudo 自动提权。选择后继续固定该目录。安装器不允许预览名称或符号链接目标，不覆盖已存在的安装锁。

最终以 Launch Services 启动后 App 的 `status.permissions` 为准，直接从终端执行诊断可能继承宿主的权限，不能代替日常 App 的实际授权证明。缺失权限时在 App 中使用权限准备入口，并由用户完成 macOS 要求的 Touch ID／密码确认。

## 参考

- [Apple TN2206：Code Designated Requirement 与自签名身份](https://developer.apple.com/library/archive/technotes/tn2206/_index.html)：身份连续性的官方规则，不代表固定路径即可保留权限。
- [VoiceInk Makefile，固定提交 b5d1be9](https://github.com/Beingpax/VoiceInk/blob/b5d1be9a7c7377dd059f37bb3e0f791b802d4001/Makefile)：支持复用本地代码签名身份，无身份时明确提示 ad-hoc 限制。
- [VoiceInk issue #829](https://github.com/Beingpax/VoiceInk/issues/829)：本地包被另一种签名的发行包替换所造成的问题报告，属于项目观察，不是系统保证。
