# QQ 语音输入诊断桥接

这是 VoicePort 接入 QQ「语音输入文字」的诊断实现，**还不是可用的 QQ 转写来源**，没有随 VoicePort App 打包。2026-11-11，在用户单独授权的临时 QQ 副本实验中，已验证自有主入口、QQ 的 simple-preload 导出及 QQ 自己创建的原生 session；离线业务启动尚未观察到 session 初始化回调或服务就绪。原版安装包未修改，各次副本探测进程已退出。2026-11-10 的失败加载记录保留在下方。

当前功能：

- 检查 QQ 安装目录和真实 `package.json` 入口，支持读取 ASAR 中的启动脚本，计算配置与入口指纹；不执行其中代码。
- 在全新的私有目录准备三个诊断文件、原配置备份、建议配置和带 SHA-256 的安装计划；不写 QQ 安装目录、不启动 QQ。
- 诊断模块在 QQ 内观察 `wrapper.node` 和 QQ 自己创建的 session；QQ 原有初始化回调成功报告就绪后，只调用 `getBdhUploadService()` 并检查三个方法是否存在。
- 保留原调用的接收对象、参数、返回值及异常；90 秒后撤除自己的观察。报告失败不阻止原启动入口执行。
- 报告只包含固定状态、布尔能力、运行时版本及固定错误代码，不包含初始化参数、账号、令牌、聊天、音频或识别正文。文件目录权限为 0700，报告为 0600。
- `stream_framing.cjs` 对调用方提供的 Buffer 数组做纯离线结构检查：首片解析文件头，续片从长度记录继续，校验完整记录并累计编码字节偏移。续片的长度与不透明 payload 可能恰好与文件头字节重合，检查器依完整帧结构判定，不凭签名碰撞拒绝。它不读文件、不执行编码器或 QQ，也不判断 payload 是否为有效 SILK。
- `smartptt_protocol.cjs` 根据静态恢复的 AudioSDK 调用构造请求数据（`chatType: 7`、编码字节偏移、数值末片标记），并按“当前段替换、完成段累加”合并调用方提供的文字。它隔离任务 ID，失败完成不会生成成功结果；不连接 QQ、不读文件、不录音或上传。

若同一个就绪 listener 被不同 session 复用，观察器标记 `shared_init_listener` 并放弃通过该 listener 判断服务就绪，避免错误检查尚未就绪的 session。原生工厂方法不可写时，使用独立 facade 转发工厂调用，保留原接收对象和原生属性描述符，避免代理不可变属性时违反 JavaScript Proxy 约束。若模块导出本身也不可替换，记录 `readonly_session_export`，不会把未捕获的 session 当作成功。撤除观察后，应用保留的构造器代理仍保持已包装工厂的原 receiver；随后安装的其他 hook 不被覆盖。

诊断不创建或登录第二个 session，不查消息，不注册上传监听器，不录音，不调用 `uploadSmartPttSlice`。即使报告显示方法存在，也不代表参数、编码格式或转写已验证。

## 本机只读检查

从仓库根目录执行：

```sh
python3 -I scripts/qq_bridge/diagnose.py inspect --qq /Applications/QQ.app
```

本轮实际检查对象是 App Store QQ `7.0.1-52892`，入口为 `./application.asar/app_launcher/index.js`。检查成功，但 `runtime_verified` 和 `smartptt_verified` 仍为 false。

## 准备可审阅文件

`--output` 必须是 QQ 应用包之外的不存在目录，且父目录已存在并使用规范路径，例如 macOS 上使用 `/private/tmp`：

```sh
python3 -I scripts/qq_bridge/diagnose.py prepare \
  --qq /Applications/QQ.app \
  --output /private/tmp/voiceport-qq-prepared
```

输出目录包含：

```text
plan.json
original-package.json
proposed-package.json
voiceport-qq-diagnostic/
  bootstrap.cjs
  diagnostic.cjs
  report.cjs
```

`original-package.json` 保留原始字节；建议配置只将 `main` 改为 `./voiceport-qq-diagnostic/bootstrap.cjs`。bootstrap 安装观察后会加载原来的准确入口；作为主入口运行时保留原 launcher 的 `require.main === module` 与入口 argv，作为依赖被 require 时保留依赖加载语义。这两种情况以合成 launcher 验证，不代表 QQ 自定义启动器已兼容。计划包含原配置、原入口、建议配置及三个诊断文件的哈希。

**这个命令只准备文件，没有自动安装或恢复命令。** 建议配置描述的是把诊断目录放入 QQ 的 `Contents/Resources/app/`、切换入口并重启的加载方式；本机 App Store 版本在临时副本中实际尝试后被 Gatekeeper 拒绝，不能把这份计划当作已验证可用的安装步骤。任何后续兼容环境中的验证都应重新核对原配置和原入口哈希、确认 QQ 已退出及目标诊断目录不存在。恢复使用备份的原始配置字节，只移除本次新增且哈希匹配的三个文件，并检查签名。不能自动重新签名或关闭系统保护。

## 读取报告

加载后，bootstrap 会向 QQ 的 stderr 输出新建报告路径。报告存放在 QQ 自身可写的临时目录中，名称为 `voiceport-qq-diagnostic-*/report.json`。

```sh
python3 -I scripts/qq_bridge/diagnose.py status --report /absolute/private/report.json
```

命令只投影已知布尔字段，不回显报告中任意字符串或未知内容，始终保留 `transcription_verified: false`。

如果 wrapper 已被预加载，会标明 `wrapper_already_cached`。观察器可以包装缓存里的导出，但不会为找回既有 session 而调用未知工厂；`session_may_precede_probe` 表示入口可能太晚。看不到 session 或超时不能解释为 QQ 没有语音能力。

## 已验证范围与限制

```sh
node --test Tests/QQBridgeTests/*.test.cjs
python3 -I -m unittest discover -s Tests/QQBridgeTests -p 'test_*.py' -v
```

2026-11-11 当前完整离线检查为 35 项 Node 测试、8 项 Python 测试通过，需 Node.js 22+ / Python 3。检查使用合成原生导出和临时假应用，覆盖原生接收对象/原型/返回值、异常和回调转发、只读工厂及不可替换导出、撤除观察、缓存预加载、报告权限和脱敏，以及诊断成功/失败时执行原启动入口。本机 QQ 只读检查和外置文件准备命令也已实际执行。合成检查与下方真实副本证据分别记录，不能据测试通过推断语音识别成功。

本机 QQ 使用自定义主程序和 `QQNT.framework`。静态 fuse 标记存在，并不证明自定义启动器支持 Node 模式：本轮尝试 `ELECTRON_RUN_AS_NODE=1 .../QQ -e ...` 时，QQ 进入正常启动而非执行指定脚本，已停止此次新建的探测进程；原 QQ 进程保持运行。没有得到可用于无修改加载的路径，不应重复该尝试。该正常启动由 QQ 自身访问了应用配置；诊断脚本未得到执行，也未提取账号、聊天或凭据。

### 2026-11-11 临时副本的实际进展

以下实验使用现有 App Store QQ `7.0.1-52892` 的独立临时副本，分别获得用户对副本重签、自有入口、隐藏 renderer 以及离线 QQ 原业务启动的具体许可。未修改原版 QQ；这些许可不构成自动安装步骤或联网、登录、录音、上传授权。

| 阶段 | 实际结果 | 仍未验证 |
| --- | --- | --- |
| 自有主入口 | 修正临时副本的 Helper 名称配置后，自己的 bootstrap 正常执行，实际 Electron `40.0.0`，进程退出 `0`。 | 原版签名 QQ 的无修改加载，以及已有登录会话访问。 |
| 隐藏 renderer / QQ simple-preload | 实际 QQ preload 的 `contextBridge.exposeInMainWorld('ipc', value)` 调用成功返回；描述符所见对象及 `send/invoke/on` 类型正确，DOM 和页面加载完成，进程退出 `0`。 | 没有主世界读回、页面表达式或桥接方法调用；不代表普通 RM_IPC / AudioSDK 服务已安装。 |
| 离线 QQ 原业务启动 | 在文件、写入和网络限制下重定向 `appData`，观察到应用 ready、一个隐藏窗口、普通 RM renderer listener 注册；只读工厂兼容修正后，观察到 QQ 自己创建原生 session。 | 15 秒内部观察期限内 `init_observed`、`ready`、`bdh_service` 均为 false；BDH getter 未调用，音频方法未调用。 |
| 加载阶段定位 | 后续同范围观察确认隐藏本地登录页 DOM 和加载完成，没有页面加载错误，8 个 xhr 请求被阻止；到期退出 `2`，外层未超时。 | 不能由本地登录页和离线期限推断登录或网络是唯一前提；继续静态核对 `startSession` 调用条件。 |

原始私有回执为 `voiceport-qq-entry-proposal-*/{verified-result,renderer-verified-result}.json`、`voiceport-qq-service-isolation-*/service-readonly-evidence/result.json` 及 `service-stage-evidence/result.json`。每次结果均确认无剩余副本进程、原版配置指纹未变和深度严格签名验证通过。自有入口的 Helper 配置失败、appData 权限失败和修正前只读工厂失败均保留，未覆盖成成功。限制策略验证的是已测试的文件、网络及子进程行为，不是通用原生 IPC 隔离证明。

### 2026-11-12 session 初始化路径与离线路由观察

用自写有界数据读取器继续核对已安装程序资源，恢复以下初始化相关路径：

- renderer 模块 `33913` 的 Vue `created` 先调用 `listenNTSessionInitComplete()`，再调用 `createNTSession()`。后者执行 `ntApi.nodeIKernelLoginService.startWrapper()`；监听到 `onSessionInitComplete` 才跳转 `/main`。模块 `23823` 将它注册到 `/loading`，默认 `/` 重定向到 `/loading`；该 router 在 `window.meta?.windowName === ChatWindow` 时使用空路由表。
- 主端业务 facade `72421` 的 `startWrapper` 转发到登录服务 `51179`，再经 `60629.startSession()` 进入 `96414` 的 wrapper。`initWrapper` 负责创建 session，`startSession` 才在防重复及 session 存在检查后调用 `sessionNt.init(...)`。方法名 `init` 已从独立的 95 对象缓存前缀读取确认。另有 QR 登录成功回调调用 `startSession`，不能据此推断所有初始化都必须先登录。

登录管理模块 `10254` 的完整父函数局部子树还确认：构造时 `sessionInited=false`，`start()` 据此打开登录窗口；成功的初始化回调才把该字段设为 true。session 创建回调中的 `46301.nG()` 与初始化完成回调中的 `46301.EE()` 分别更新阶段并完成各自 Promise，没有发起 native 初始化。QR 登录成功回调则先 `setAuthData`，再调用已绑定到 `60629` 的 `ntWrapper.startSession()`。登录窗口的 `--hide-login-window` 分支只控制显示，所查 setup 中的 installer 分支调用 `readyToShow`，未直接调用 `startSession`。这些是一条具体登录成功路径及窗口选择条件，不能证明所有初始化都必须登录。

登录窗口 URL 的导出绑定也已从既有 `background.js` 缓存核对：`21679.gW()` 固定返回 `app://./renderer/login.html`，没有 `/loading` hash，与根路由实测相符。安装包两份登录 HTML 的实际哈希均与归档完整性声明一致。继续静态追踪 QQNT 内嵌文件系统包装层的 `electron_common_asar.getPlain`，已确认其资源格式为前 16 字节 IV、随后 AES-256-CBC 密文及默认 PKCS#7 填充；固定密钥来自程序资源常量。自写脚本仅用 Node 内置 crypto 离线解码，两份输出均通过填充、严格 UTF-8 和 HTML 结构检查，没有调用 QQ 的解码器或执行页面脚本。页面明确分别调度 `window.electron.load('r_login')` / `r_newLogin`；名称已与本机原生缓存表配对，相应小缓存完成静态读取。

`r_login` 的显示条件也已静态连接：主端发送 `loginReady`，登录 preload 接收后暴露 `isLoginReady=true` 并派发 DOM 事件，组件才渲染 `Login`。这是 UI 就绪条件，实测尚未观察这个标志或事件，不能把它当作 session 初始化或认定事件漏发。候选登录业务缓存的有界前缀另恢复了监听注册、连接后扫码/自动登录分支和初始化完成后的成功/失败响应。Store 类工厂已将 `bindPushEvent` 绑定到监听函数，也找到 scope 名为 `mounted` 的函数调用它；UI 父级类安装和实际 `loginStore` 注入仍未核对，尚不能拼成完整启动顺序。这些处理函数没有直接初始化 session。具体边界见 [登录页面的 UI 就绪与事件响应](RENDERER_IPC.md#登录页面的-ui-就绪与事件响应)。

这些是静态控制流，尚未观察到离线进程执行上述 renderer 调用或完成 native 初始化。两块小缓存的对象图读取完整，主缓存仅恢复局部子树；producer typed layout 未验证、payload checksum 缺失的限制仍保留。具体函数、缓存指纹和证据范围见 [session 初始化触发证据](RENDERER_IPC.md#session-初始化触发证据)。

随后在原有离线临时副本范围内完成了一次被动路由观察：PID `97183`，内部 15 秒观察到期后退出 `2`，外层未超时。唯一 renderer 是 `login.html` 的根路由；DOM 和加载完成，没有观察到 `/loading` 或 `/main` 导航，加载错误码为空、preload 错误与 renderer error 计数均为 `0`，没有 renderer 退出事件。session 仍仅创建、未观察到 init 或服务就绪，8 个 xhr 被阻止。没有页面求值、强制导航、IPC 或 session/init 调用；策略未放宽，副本进程已退出，原版指纹和严格签名检查通过。回执为 `voiceport-qq-service-isolation-*/service-route-evidence/result.json`。这将离线停点定位在登录页根路由，仍不能证明登录或联网是唯一缺失条件。

### 2026-11-10 实际加载结果（历史）

在用户明确授权临时诊断加载和重启后，实际执行了以下路径：

| 路径 | 实际结果 | 恢复情况 |
| --- | --- | --- |
| 在私有 APFS QQ 副本中安装 bootstrap 并切换入口 | 副本进程退出码为 `-9`；系统 `syspolicyd` 日志明确显示 Gatekeeper 拒绝。未取得诊断报告。 | 副本原配置已恢复，签名验证通过；原安装包未修改，原版 QQ 已重新打开。 |
| 原版签名 QQ 使用 `NODE_OPTIONS --require` 加载 `preload.cjs` | 启动后约 35 秒没有观察到诊断路径标记，运行结果为 `report: null`。未取得会话或能力报告。 | 已结束探测进程、删除自己的临时缓存模块，原版 QQ 已在不带预加载参数的情况下重新打开。 |
| 原版签名 QQ 使用 `--inspect-brk=127.0.0.1:<port>` | 隔离 Node 夹具验证了诊断加载、原入口恢复和调试端口关闭；真实 QQ 未观察到属于启动进程的回环调试监听，`report: null`，没有执行加载表达式。 | 探测进程已退出、临时目录已移除，原 QQ 已重新打开，原配置和签名检查通过。 |

第二条路径的临时运行脚本只通过 stderr 标记发现报告路径，没有独立扫描自己的私有报告目录。因此，能确认的是没有取得报告，不能据此断言模块绝未执行、QQ 必然忽略 `NODE_OPTIONS`，或确定失败的具体原因。仓库中的 `preload.cjs` 是这条可选加载路径的入口，使用同目录的 `diagnostic.cjs` 和 `report.cjs`，不负责加载 QQ 原入口。它通过了 Node 语法检查；上面 9 项 Node 与 6 项 Python 合成测试不代表该入口在真实 QQ 中兼容。

结束后再次核对：原版 QQ 正在运行，诊断副本未运行，原 `package.json` 指纹不变，`codesign --verify --deep --strict` 通过。没有修改 Gatekeeper/SIP、移除隔离属性或重新签名 QQ，也没有录音、上传音频或发送消息。

后续已从本机原生绑定恢复准确参数结构与部分任务行为，见 [SmartPtt 静态契约](SMARTPTT_CONTRACT.md)。`uploadSmartPttSlice` 的第二个参数包含 `chatType`、`sliceIndex`、`filePath`、`voiceOffset`、`isEnd`、`removePeriod`；两个文字/完成回调均有三个位置参数。它们来自 N-API 属性读取和转换代码，不等于真实任务已经跑通。后续静态追踪还确认 metadata 的 protobuf 字段号，并在桌面 `AudioFoundation.node` 恢复了 `02 + #!SILK_V3` 文件头、两字节长度/payload 记录及最终短帧补零路径；后续使用匹配的本机 arm64 快照做静态缓存解析，恢复了 AudioSDK 中 `chunkIndex → sliceIndex`、`chunkOffsetBytes → voiceOffset`、`isLast === 1 → isEnd` 的具体指令，以及 `Init(outputDir, 16000, 1)`、30 位随机十六进制任务 ID 和文字分段合并行为。新增五项协议夹具后 Node suite 共 25 项通过。解析器仍有构建配置与辅助对象布局限制，真实进程入口和服务端接受条件尚未验证，详见上述契约。

也已通过 GitHub 读取 [Electron 环境变量文档](https://github.com/electron/electron/blob/f90ff22d985c714cb77d6d220648e57a5f89287c/docs/api/environment-variables.md)：打包应用的 `NODE_OPTIONS` 只允许少量指定选项，不包含 `--require`。[主进程调试文档](https://github.com/electron/electron/blob/f90ff22d985c714cb77d6d220648e57a5f89287c/docs/tutorial/debugging-main-process.md) 则单独提供 `--inspect` / `--inspect-brk`；隔离夹具的成功不能证明 QQ 自定义启动器兼容。这些文档解释了下一步应检查的机制边界，尚未确定本机 QQ 探测退出的原因。

### 2026-11-10 渲染端路径验证

已新增固定表达式的 `renderer_probe.cjs`，并在原版签名 QQ 上实际尝试独立的渲染调试入口。QQ 在 20 秒观察期间正常运行，但没有观察到属于该进程的回环调试端口；没有执行页面表达式或 IPC。探测进程正常退出、普通 QQ 已恢复，配置指纹与严格签名验证保持。最初六项夹具测试验证目标/端点限制、正文上限、固定能力读取和输出脱敏。后续静态解码确认 QQ preload 将 `ipcRenderer` 直接暴露到主世界，因此补上了直接 `send/invoke/on`、`ipcImpl` 和 `waitForWindowTypeEnsure` 的类型检测；新增夹具确认没有 `electron` 命名空间时仍可检测且不调用方法或读取页面。随后在截图贴图编辑器的静态路径中找到独立的 `window.ipc` 通道，现补上该对象和 `send/invoke/on` 的类型检测，避免只有此对象时漏报能力。后续自写有界数据读取器恢复了 `preload_simple.js` 的提供者、主端 dispatcher 和局部窗口配置：`window.ipc.send('screenshot', 'openDevtool')` 经 Electron IPC 到达 `event.sender.openDevTools({mode: 'detach'})`；截图主窗口及辅助窗口的所查配置没有显式 `devTools: false`。这补齐了静态候选链，仍没有验证免截图的窗口进入方式、有效运行配置或 AudioSDK 服务就绪。新增夹具不调用桥接方法或读取页面，PR26 验证的完整 Node suite 为 33 项通过，不能当作真实 QQ 兼容性证明。

进一步静态追踪在 QQNT 的命令行参数处理路径中发现：参数遍历结束后会删除 `inspect` 和 `remote-debugging-port` 键，和真实探测结果一致。这不解释此前 `--inspect-brk` 的具体退出原因，也不证明所有程序内部调试入口均不存在。QQ 当前顶层菜单及应用/帮助菜单中也未观察到开发者工具入口，未读取聊天窗口。完整可复现地址、二进制指纹、preload/RPC 标识和观察限制见 [渲染端 IPC 与调试入口证据](RENDERER_IPC.md)。

```sh
node --test Tests/QQBridgeTests/renderer_probe.test.cjs
```

当前阻塞是 **临时副本的执行入口和 session 创建已验证，但 session 初始化、真实服务就绪与音频/任务接受条件仍未验证**。原版签名 QQ 的无修改加载及已有登录会话访问也未建立。以下静态链路和旧入口比较保留各自阶段的证据范围：内部 AudioSDK 惰性实现连接、通用 IPC 请求/事件/响应封装、窗口 peer 和上下文注入已静态恢复；renderer 模块 `41386` 的 `audioSDKApi` 已确认由 `63198.Z(43145.Qe.AudioSDK)` 创建，通用工厂 `63198` 已恢复其声明方法映射，枚举 `43145` 确认准确服务名为 `AudioSDKApi`，renderer 的 RM 收发前缀分别为 `RM_IPCFROM_RENDERER` / `RM_IPCFROM_MAIN`。底层 `10413` 的完整通道拼接、请求/订阅/响应线格式与清理行为也已恢复；preload 普通分支已确认原样转发 Electron IPC，主端适配函数和相反方向的 RM 字面量配对也已确认；主端 `72298` 已确认导出宿主 `require('electron')`，普通分支的静态调用链已还原。实际 chunk 安装、preload 替代 provider 和运行可达注册仍待验证，不能混用另一通用层的 `IPC_UP_` / `IPC_DOWN_`。主端入站惰性实现与出站事件 Proxy 已区分，不能把两者的 `invoke` / `handle` 字段差异当成入站必然断开的证据。另找到登录窗口 `Control+Shift+I` 调用开发工具的代码；后续从另一缓存恢复的创建路径明确设置 `devTools: false`，几处群视频调用的 `TQ` 条件也静态为 `false`，因此没有执行快捷键或再次启动诊断。本地多实例服务的 GET/POST 分发仅按已注册 controller 路由执行；所查 `websdk` 方法处理分享数据与 URL schema，尚未建立通用 AudioSDK 桥。没有连接或调用服务，当前没有运行中的 QQ 探测。官网发行版与新版加载材料也已核对；某个入口失败不能证明 QQ 接入不可实现。官网当前配置为 QQ `7.0.3`（2026-11-03），该 DMG 获取仍返回 HTTP 403。已从 Homebrew 固定 cask 指向的官方地址取得旧独立版 `7.0.0-52194`，公开 SHA-256 匹配；只读静态比较确认它通过严格签名验证，但同样启用沙箱、封存启动资源，并删除上述两个调试开关。挂载已卸载，没有运行或安装该旧版，不能用它推断当前 `7.0.3` 兼容性。GitHub 的 `7.0.2` 插件加载报告、后续原生 wrapper 适配及实际旧版比较分别保留证据边界，见 [官网发行版与加载线索](../../docs/QQ_SPEECH_INPUT.md#当前官网发行版与新版加载线索)。

当前离线实验停在登录页根路由；窗口选择、阶段 Promise 和一条 QR 登录成功后的初始化路径已静态恢复。下一项运行验收需要 QQ 自己在合法启动条件下触发 session 初始化回调，并真正达到服务就绪；当前离线回执尚未满足这一条件。不能用手动调用 `startSession` 或猜测初始化参数代替 QQ 自己的调度。不会重复已失败的调试参数或盲目延长离线期限。原版 QQ、系统保护和账号目录不在临时副本许可内；联网、登录和音频样例需要具体方案及单独授权。各次运行均为 `transcription_verified: false`，尚未新增可用 QQ 来源。
