# macOS 更新与权限身份

日常使用固定的 `~/Applications/VoicePort.app`，bundle ID 始终为 `io.github.forwaryan.voiceport`。版本号可以变化；应用名称、安装位置和代码签名身份不应随一次调试构建改变。`dist/` 中的独立包只作构建产物，避免从多个预览包交替启动日常应用。

## 为什么更新后可能要重新授权

macOS 通过代码签名要求（designated requirement，DR）识别程序是否属于同一个应用。Apple TN2206 说明，新版本应该满足旧版本的 DR。固定 bundle ID 或目录名称本身不能证明签名身份连续。

未配置持久身份时使用的 ad-hoc 签名不使用持久证书，DR 可能绑定会随构建改变的代码哈希。系统设置中的开关即使显示开启，实际运行进程也可能未获辅助功能权限。更换签名证书、在本地构建与正式发行版之间切换，也可能需要重新授权。不能以固定路径或签名检查通过承诺所有系统版本永久免授权。

## 使用固定签名身份打包

先在本机查看可用证书的公开元数据：

```sh
security find-identity -v -p codesigning
```

正式分发应使用 Apple Developer 账号的 Developer ID Application 身份，保持 Team ID、bundle ID 和签名要求连续，并按 Apple 流程配置 hardened runtime 和公证。选定身份后长期复用，不要每次创建新证书。Apple Development 可用于本机开发，但不能替代正式分发流程。私钥保留在 Keychain，不放入源码、环境配置文件或 GitHub；本脚本的证书签名支持本身不等于已公证的发行包。

```sh
VOICEPORT_CODESIGN_IDENTITY='证书指纹' \
VOICEPORT_APP_DIRECTORY="$PWD/dist/release/VoicePort.app" \
bash scripts/package_app.sh
```

已有 release 编译结果时加 `--skip-build`。为了让后续打包默认复用同一身份，可以在获得可用证书后，将公开 SHA1 指纹保存为当前用户固定路径 `~/.config/voiceport/signing.json` 的普通 JSON 文件（路径及父目录不能是符号链接）：

```json
{"schema_version": 1, "identity": "0123456789ABCDEF0123456789ABCDEF01234567"}
```

上面的指纹仅为格式示例，须替换成 `security find-identity -v -p codesigning` 列出的真实 40 位十六进制 SHA1。文件只接受 `schema_version` 和 `identity` 两个字段，不存放私钥、密码或可执行配置。脚本只读取此固定用户路径，不执行或 source 配置，也不写入实际配置或 Keychain。

`VOICEPORT_CODESIGN_IDENTITY` 显式选择优先于保存的身份；环境变量支持 SHA1 指纹或完整且唯一的证书名称，最终传给 `codesign` 的是核验后的指纹。无显式选择时读取保存的指纹。打包在编译或暂存前通过 `security find-identity -v -p codesigning` 核验所选身份；不可用、名称不唯一或查询失败都会中止，不会降级。保存配置无效、无法读取或含符号链接时，即使有环境变量覆盖也会明确失败，须先修正配置。已保存身份时拒绝显式 `VOICEPORT_CODESIGN_IDENTITY=-`，避免意外切回 ad-hoc 导致权限身份变化。

只有没有保存配置、也没有选择证书时，才保留本地 ad-hoc 构建行为并明确警告权限可能需要重授；此时也可显式选择 `-`。固定签名身份用于保持后续更新的身份连续性，不保证所有 macOS 版本永远无需重新授权。这里不自动生成或信任自签名证书、更改 Keychain 搜索列表或修改 TCC 数据库；自签名不能替代上述正式发行流程。

## 成熟项目的实际做法

以下按固定提交核对，区分开发包、CI 产物与正式发布：

| 项目 | 已核对的实现 | 对本项目的启示 |
| --- | --- | --- |
| [AltTab 正式 CI](https://github.com/lwouis/alt-tab-macos/blob/ec4f94a8b98a5b9df152af64f74aa4f9f811dd18/scripts/codesign/setup_ci_master.sh) / [公证打包](https://github.com/lwouis/alt-tab-macos/blob/ec4f94a8b98a5b9df152af64f74aa4f9f811dd18/scripts/package_and_notarize_release.sh) | 从受保护配置导入证书，公证必须得到 Accepted，再 staple；[本机 setup](https://github.com/lwouis/alt-tab-macos/blob/ec4f94a8b98a5b9df152af64f74aa4f9f811dd18/scripts/codesign/setup_local.sh) 单独生成自签名证书 | 本机开发签名与正式发布分开；不能从开发脚本推断发行包身份 |
| [VS Code 签名](https://github.com/microsoft/vscode/blob/d53b9e1def17b1919021ee08c87a5b9453207fe2/build/darwin/sign.ts) / [发布公证](https://github.com/microsoft/vscode/blob/d53b9e1def17b1919021ee08c87a5b9453207fe2/build/azure-pipelines/darwin/codesign.ts) | 从构建配置读取签名身份与专用 Keychain，启用 hardened runtime，发布执行签名和公证 | 使用明确身份与受保护凭据；证书不可用时中止发布 |
| [GitHub CLI 发布说明](https://github.com/cli/cli/blob/3d5a9e355a837f2f5ee78d7484c118cc1342ad1a/docs/release-process-deep-dive.md#signing) | production 导入 Developer ID Application，使用 `--timestamp --options=runtime`，通过 Keychain profile 调用 notarytool | 正式发布需要开发者证书和独立公证资格；GitHub 托管代码不会自动提供 Apple 签名 |
| [Rectangle 项目设置](https://github.com/rxhanson/Rectangle/blob/875182d2690e4b8fdf485467b31a3233d630581f/Rectangle.xcodeproj/project.pbxproj) / [公开构建 CI](https://github.com/rxhanson/Rectangle/blob/875182d2690e4b8fdf485467b31a3233d630581f/.github/workflows/build.yml) | 固定 Team ID，开发/正式 bundle ID 分开，Release 启用 hardened runtime，使用 Sparkle；公开 CI 显式 ad-hoc | CI 构建成功不等于正式发行；自动更新框架与 TCC 身份连续是不同层次 |

VoicePort 的推荐流程是：取得可用的 Developer ID Application 身份并固定 Team ID/bundle ID，保存公开指纹，构建时复用身份；正式发行还要启用 hardened runtime、时间戳、公证及 staple，验收后再分发。首次从 ad-hoc 迁移只做一次正常系统授权；后续每个包继续满足已安装版本的 DR，并以实际运行进程核验权限。公证解决 Gatekeeper 分发检查，不代替用户授予辅助功能、麦克风或语音识别权限。

**当前实现范围：** `package_app.sh --distribution` 已支持 Developer ID、hardened runtime 和安全时间戳，`release_macos.py` 管理公证、staple、Gatekeeper 与最终 ZIP；`install_app.py` 验证更新签名连续性。没有真实证书时不能把本地包称为已正式签名或已实现跨更新保留权限。证书续期也应核验 DR 连续性，不能仅因 SHA1 改变就断言必须重新授权。

## 正式发布与首次配置

需要 Apple Developer Program 成员资格、Keychain 中包含私钥的 Developer ID Application 证书，以及 Apple 公证凭据。GitHub 仓库、免费构建额度或普通 Apple ID 不会自动提供这些资格。由有权限的团队成员在 [Apple 开发者账户](https://developer.apple.com/account/)中创建或管理证书；不要将私钥、密码、`.p12` 或 `.p8` 发进聊天或提交到 Git。当前脚本不会创建开发者账号、购买资格、导出私钥或修改本机信任。

选择该证书的真实 SHA1，并按上文保存 `signing.json`；Team ID 是该证书所属团队的 10 位标识。正式打包显式核验这两个条件，不能用 Apple Development、自签名或 ad-hoc 替代：

```sh
export VOICEPORT_TEAM_ID='你的10位TeamID'
# 已保存指纹时不必再设置 VOICEPORT_CODESIGN_IDENTITY。
VOICEPORT_APP_DIRECTORY="$PWD/dist/signed/VoicePort.app" \
  bash scripts/package_app.sh --distribution --skip-build
```

GUI 与 CLI 均带 hardened runtime 和安全时间戳。主程序仅带 `audio-input` 与 `disable-library-validation` 两项 entitlement：前者用于麦克风，后者用于现有本地模型后端加载单独安装在 Orca 中、由其他身份签名的 sherpa/onnx 动态库。CLI 不带这两项权限；不启用 JIT、调试或不受限可执行内存。依据：[Apple hardened runtime](https://developer.apple.com/documentation/xcode/configuring-the-hardened-runtime)、[audio-input](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.device.audio-input)、[library validation](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.cs.disable-library-validation)。这些 entitlement 不自动授予 TCC 权限。

完整发行使用以下三个入口；输出目录必须尚不存在：

```sh
python3 -I scripts/release_macos.py prepare dist/release-0.3.3 \
  --team-id "$VOICEPORT_TEAM_ID" --skip-build
# 在本机交互配置公证凭据；按工具提示在终端输入，不写进命令或源码。
xcrun notarytool store-credentials voiceport-notary
python3 -I scripts/release_macos.py submit dist/release-0.3.3 \
  --notary-profile voiceport-notary
```

`prepare` 只构建/签名并生成提交候选；代码签名的安全时间戳会访问 Apple，尚不会上传 App 公证。`submit` 将该候选提交 Apple，记录原始回执与 submission UUID，再等待 Accepted，执行 staple、票据校验和 Gatekeeper assessment。全部通过后才生成 `VoicePort-macos-arm64.zip` 与带 SHA256 的 `release.json`，供人工检查及分发。`notarization-upload.zip` 是恢复候选，不能当成已验收发行包。流程不会安装、退出或启动日常 App，也不录音、不上传音频或触发输入。

中断后从原目录按已保存 ID 恢复，不重新提交：

```sh
python3 -I scripts/release_macos.py resume dist/release-0.3.3 \
  --notary-profile voiceport-notary
```

每次命令调用前将意图 fsync 到 `journal.jsonl`，随后保存独立结果。已有 submit intent 会阻止再次上传；`resume` 可从成功上传的原始回执恢复 UUID，包括回执已保存但 completed 事件尚未写入的中断。如果上传回执确实未知且无 UUID，先核查原 journal 和 Apple 公证历史，不能新建目录盲目重发。遗留 `.release-lock` 表示需确认原进程已退出和所有未知副作用；脚本不自动删除锁或接管仍运行流程。公证未 Accepted、文件变化、票据校验或 Gatekeeper 失败均不会产生可分发 ZIP。

## GitHub 手动发布构建

[Signed macOS package](../.github/workflows/macos-release.yml) 只允许在 `main` 手动触发，使用独立 `macos-release` environment；应在 GitHub 设置中为该 environment 配置 required reviewers 和 main 部署限制。它不在 PR 中读取签名凭据，不自动创建公开 Release。当前工作流文件不代表这些仓库设置或 secrets 已经配置。

在该 environment 中配置：

| 类型 | 名称 | 内容 |
| --- | --- | --- |
| Variable | `APPLE_TEAM_ID` | 真实 10 位团队标识 |
| Variable | `MACOS_SIGNING_IDENTITY` | Developer ID Application 的 40 位 SHA1 |
| Secret | `MACOS_CERTIFICATE_P12_BASE64` | 通过团队正常流程准备的证书及私钥导出，base64 编码 |
| Secret | `MACOS_CERTIFICATE_PASSWORD` | 导出文件密码 |
| Secret | `APPSTORE_CONNECT_PRIVATE_KEY` | Apple 公证 API 的 `.p8` 内容 |
| Secret | `APPSTORE_CONNECT_KEY_ID` / `APPSTORE_CONNECT_ISSUER_ID` | 该 API key 的标识与 issuer |

这些值仅在 GitHub secrets UI/团队已有凭据管理流程中设置，不能写入源码。CI 在导入凭据前构建并验证产品；在临时 Keychain 导入证书、保存公证 profile，随后移除明文临时文件，always 步骤清理 Keychain。签名身份必须匹配预期指纹与 Team ID，缺失不降级。正式产物 artifact 只含已验收 ZIP 和 manifest。

失败时另保存公证回执，以及清楚标注不可分发的原始提交候选。不要直接 Re-run jobs：工作流会拒绝同一 run 的重跑，避免对未知提交重复上传。恢复时将原始候选与 receipts 下载到新的独立目录，先以团队预期 Team ID 核验包；用 `ditto -x -k notarization-upload.zip 恢复目录` 还原 `VoicePort.app`、保留原 prepared/journal/receipts，并运行 `resume`。CI 的临时 Keychain 不可恢复，需要在本机配置自己的公证 profile。没有已确认 UUID 时仍按上文核对 Apple 历史，不自动重交。首次运行还需实际验证 GitHub runner 为 arm64、证书链和公证资格；本地模拟测试不能证明真实 Apple/CI 发布成功。

## 从菜单检查更新

右键点击 VoicePort 菜单栏图标，选择「检查更新…」。检查期间菜单显示当前应用版本。查询成功后显示当前版本与 GitHub 最新发行版本，并区分发现更新、两者相同、本机版本较新和发行包不可用。查询失败时显示当前版本，选择该菜单项可查看失败原因。只有已经核对的发行标签才会作为最新版本显示。

「自动检查更新」默认关闭。开启后会后台查询，发现新版时菜单显示同样的版本信息。发现新版或下载完成后，仍可选择「重新检查更新…」。重新查询成功会按新结果刷新并清理旧暂存包；查询失败则把缓存明确标为「上次已知」，不会把它报作最新版本。下载与安装均需明确确认，后台检查不会弹窗或自动安装。

应用检查固定 GitHub 仓库的正式发行。公开查询遇到限流或访问限制时，使用在常见固定位置找到的 GitHub CLI（`gh`）及其现有 GitHub 登录。私有发布需要已登录且有仓库权限的 `gh`。应用接受 `vX.Y.Z` 标签下明确上传的 `VoicePort-vX.Y.Z-macos-arm64.zip` 或签名发布脚本生成的 `VoicePort-macos-arm64.zip`，同一个发行只能有一个候选，且 GitHub 必须提供有效的资产 SHA-256。

系统 `/Applications/VoicePort.app` 也可检查版本，但必须手动安装。应用内安装只允许当前 OS 用户拥有的真实 `~/Applications/VoicePort.app`，不接受符号链接、其他用户可写的目录或自定义控制端点。主目录、`Applications`、应用包和私有暂存目录都要通过所有者、POSIX 模式和 macOS 扩展 ACL 检查。无 ACL 或仅有拒绝项的目录可通过 ACL 检查。存在授予项或无法读取 ACL 时，请按手动安装说明更新。下载后检查大小和 SHA-256，再验证包版本、构建号、系统兼容性和签名连续性。

选择安装后，若听写、投递或其他任务仍在运行，弹窗会说明需先完成的操作。空闲时可确认安装。若仍保留文字或失败录音，可以选择「查看听写」，先保存文字或重试录音，处理后从菜单再次选择安装。若提示有命令行转写结果，可用原操作编号运行 `voiceport dictation result ID` 取回。「放弃并安装」只适用于已确认可以舍弃这些内容的情况。确认后应用再次检查任务和保留内容。取消、被任务阻止或未能启动更新助手时，菜单会保留原已校验版本，供重试或重新检查更新。

更新助手等待正常退出，复核实际用于交换的两个目录，再原子替换并核对新版是否启动。只有控制端点可证明空闲时才回退，回退前也会复核这两个目录。若无法确认启动的结果或控制端点归属，则保留两个包，不重试不确定的动作。

从出现「目前不能重启」的旧本地 `0.3.13` 迁移时，请先保存需要保留的文字。如果还需要失败录音的转写，可在旧应用内明确选择重试。重试可能再次上传录音。等任务结束并确认可以舍弃剩余内容后，再正常退出。在同一路径用下方手动安装器安装签名连续的已校验新版。对于仍处于 `ready` 或 `failed` 的旧应用，`--quit-idle` 会拒绝替换。系统目录安装还应先确认所有用户均已退出 VoicePort。若启动或回退受旧 socket 阻塞，请先由该用户确认没有实例仍在使用，再按 [CLI 的控制端点说明](CLI.md) 处理。

### 在命令行查询更新

运行 `voiceport --json app capabilities` 检查当前 App 是否公布 `app_update`，然后用同一控制端点执行 `voiceport --json app update status`。这是进程内快照，只读当前版本、最近一次验证的发行版本及检查结果。新查询正在运行或失败时，如果仍有上次已知版本，`latest_version_stale` 为 `true`。启动后尚无成功查询时，`latest_version` 为 `null`。快照随 App 退出清除。

需要重新查询时，明确执行一次 `voiceport --json app update check`，随后用 `app update status` 查看结果。退出码 0 表示请求已接收或快照已读取，查询结论见 `last_check_state` 和 `reason`。忙碌时检查请求返回退出码 4，并带有当前快照。两条命令共用 App 菜单的更新状态。下载和安装仍从菜单确认，安装前执行原有任务、保留结果和签名检查。完整字段见 [CLI 命令参考](../skills/voiceport/references/cli.md#app-commands-macos)。

## 安装到同一个位置

```sh
python3 -I scripts/install_app.py dist/release/VoicePort.app --quit-idle
open "$HOME/Applications/VoicePort.app"
"$HOME/Applications/VoicePort.app/Contents/MacOS/voiceport" --json app status
```

安装器验证 bundle 身份和完整签名，在目标目录旁暂存完整新包。默认拒绝替换仍运行的 App；`--quit-idle` 只允许正常退出 `state=idle` 且无待处理任务的实例，不强制杀进程。已完成的文字或失败录音可能仍保留在内存中；`ready`、`failed` 或未知状态均中止安装，先保存结果、处理录音，再由用户明确退出。旧包保存到同目录的 `.voiceport-backup-*/VoicePort.app`，替换失败则尝试恢复旧包。安装不会自动启动、录音、上传、清空听写或申请权限。

升级已有安装时，新包必须满足旧包的 DR；身份变化默认中止。首次从 ad-hoc 迁移到固定证书，或明确接受一次需要重新授权的本地 ad-hoc 更新时，可以显式传入 `--allow-signing-change`。它只允许这次文件替换，不授予任何系统权限，也不修改系统信任。后续正常更新应使用同一证书并省略该参数。

`--destination /Applications/VoicePort.app` 可选择系统应用目录，前提是当前用户有写权限；不使用 sudo 自动提权。选择后继续固定该目录。安装器不允许预览名称或符号链接目标，不覆盖已存在的安装锁。新版手动安装器和应用内更新都独占持有目录 `flock` 与 `.voiceport-install.lock`，从而兼容只使用其中一种的旧安装器。进程异常退出可能留下 marker；确认记录的原持有者已退出、无待处理的替换或恢复之前，不要移动它或再次安装。

最终以 Launch Services 启动后 App 的 `status.permissions` 为准，直接从终端执行诊断可能继承宿主的权限，不能代替日常 App 的实际授权证明。缺失权限时在 App 中使用权限准备入口，并由用户完成 macOS 要求的 Touch ID／密码确认。

## 参考

- [Apple TN2206：Code Designated Requirement 与自签名身份](https://developer.apple.com/library/archive/technotes/tn2206/_index.html)：身份连续性的官方规则，不代表固定路径即可保留权限。
- [VoiceInk Makefile，固定提交 b5d1be9](https://github.com/Beingpax/VoiceInk/blob/b5d1be9a7c7377dd059f37bb3e0f791b802d4001/Makefile)：支持复用本地代码签名身份，无身份时明确提示 ad-hoc 限制。
- [VoiceInk issue #829](https://github.com/Beingpax/VoiceInk/issues/829)：本地包被另一种签名的发行包替换所造成的问题报告，属于项目观察，不是系统保证。
