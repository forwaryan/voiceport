# QQ 渲染端 IPC 与调试入口证据

更新至 2026-11-12，本机 App Store QQ `7.0.1-52892`。本文区分静态字符串、静态控制流和实际探测。SmartPtt 的内部参数转换、回调和 session 来源已静态恢复，见 [本机 SmartPtt 静态调用契约](SMARTPTT_CONTRACT.md)；**尚未完成真实服务调用或转写**。后续单独获准的临时副本实验已验证自有主入口、隐藏 renderer 中 QQ simple-preload 的原始桥接导出成功返回，以及 QQ 自己创建原生 session。它们没有建立原版签名 QQ 的无修改外部调用入口，也没有证明 AudioSDK 就绪，完整阶段与失败回执见 [诊断说明](README.md#2026-11-11-临时副本的实际进展)。以下旧探测及静态分析保留发生时的范围。

## 实际探测

新增 `renderer_probe.cjs` 仅能在外部运行环境已确认调试端口属于目标 QQ 进程、且只监听回环地址后使用。它只请求 `/json/list`，选择最多四个 QQ 本地应用页面，通过固定 `Runtime.evaluate` 表达式读取已知 API 的 `typeof`。它不读取 DOM、聊天正文、账号、令牌或任意页面变量，不调用 IPC，也不启动录音或上传。

探测输出固定为能力类型、页面数量和状态，不输出页面标题、URL、协议异常正文或未知字段。HTTP 正文和单个协议消息上限均为 64 KiB；页面表达式设置 `throwOnSideEffect: true`，每页最多等待 2.5 秒。此参数被客户端拒绝或属性访问被判定有副作用时，探测失败，不自动降低约束。

2026-11-10 一次真实实验：

- 未修改的签名原版 QQ 带 `--remote-debugging-port=<临时端口>` 和 `--remote-debugging-address=127.0.0.1` 启动。
- 启动进程在 20 秒观察期间保持运行，没有观察到属于该进程的回环调试监听。
- 没有发送 CDP 表达式，没有执行任何 IPC 或服务方法。
- 探测进程在恢复阶段正常退出，退出码 `0`；实验整体退出码 `1`，因为能力验证失败。
- 调试监听未残留；原 `package.json` 指纹不变，严格/深度签名验证通过；普通 QQ 已重新打开。

这是渲染端调试的新机制探测，区别于此前主进程 `--inspect-brk` 尝试。本次观察不能反推此前主进程尝试的退出原因。

原始结果：私有调查目录 `/private/tmp/voiceport-qq-diagnostic.2drUlB/renderer-runtime-result-74e2cd8f.json`。该目录还保留本次有界启动/恢复脚本 `run_renderer_probe.py`。运行脚本只投影固定引擎错误分类，不保存 QQ 原始 stderr。

## session 初始化触发证据

2026-11-12，用自写读取器从同一 `major.node` 继续恢复 QQ 自己的调度。以下小缓存偏移均为 universal 文件起点的十进制字节偏移；读取程序只处理序列化数据，没有执行 QQ 字节码。

| 缓存 | 文件偏移 / 长度 | SHA-256 | 完整读取结果 |
| --- | --- | --- | --- |
| 初始化组件所在 chunk | `131625448` / `36376` | `f247b759d416c23d52286dde3fca9d2f715c1514d035b493d503b22bc6b4709b` | 1,020 对象、128 编译函数、0 forward registrations、9 字节 NOP 尾部。 |
| router 所在 chunk | `158677312` / `24304` | `b5c35aa1b283aaba0f6136adac067ad9ca9ea9595f66d3866bbe2e742a756b1c` | 636 对象、65 编译函数、0 forward registrations、5 字节 NOP 尾部。 |

组件 chunk 的 SFI `5` 在 `570`–`581` 将模块 `33913` 绑定到 SFI `862`。其 class boilerplate `876` 将 `created`、`listenNTSessionInitComplete`、`createNTSession`、`jumpToMainPage` 分别映射到 `913`、`920`、`939`、`949`。`913` 按顺序先安装监听，再请求创建；`920` 向 `ntApi.nodeIKernelSessionListener.onSessionInitComplete` 注册回调 `929`。`939` 在 `14`–`32` 调用 `ntApi.nodeIKernelLoginService.startWrapper()`，没有参数。回调 `929` 调用 `jumpToMainPage`，`949` 向 router 推入固定 `{path: '/main'}`。

router chunk 的模块 `23823` / SFI `16` 在路由数组下标 `1` 放入 `{path: '/loading', component: closure50}`；`50` 等待 28 个 chunk 加载请求后动态导入模块 `33913`。路由数组下标 `0` 是 `{path: '/', redirect: '/loading'}`。`16` 在 `138`–`178` 检查 `window.meta?.windowName === JV.ChatWindow`，成立则使用空路由数组，否则使用上述数组。这建立了组件与路由的静态关联，没有证明离线实验的 `login.html` 使用了这个 router，或实际组件已经创建。

主缓存的局部子树另外确认：业务 facade 模块 `72421` / SFI `50824` 在 `369`–`381` 导入 `51179` 到 context slot `10`；其 `startWrapper` 闭包 `50886` 转发到该模块的 `j.startWrapper()`。登录服务 `51179` 的 SFI `22905` 经 `60629` 的包装对象调用 `startSession`；该包装方法 `52610` 通过 `require(96414).g` 转发。wrapper 的 `53246.startSession` 在防重复检查后准备参数，并于 `182`–`206` 对存在的 `sessionNt` 调用 `init`。原先局部图中的外部字符串引用 `94` 已独立读取为 `init`：从主缓存序列化偏移 `32` 开始，读取 95 对象后立即停在 `7356`，字符串位于 `7340`–`7356`。它不是全主缓存重新解析，也没有调用 session 工厂或 native init。

`initWrapper` 创建 session 与 `startSession` 初始化 session 是两个阶段。另一个已恢复的 QR 登录成功回调 `24249` 也调用 `startSession`；该分支不能证明所有启动都必须先登录。主端子树每次仍限制 10,000 对象 / 256 KiB，hot-state 结构及属性名检查分别唯一选出 facade 的 phase `0`、登录服务的 phase `1` 和包装模块的 phase `2`。外部引用未全部解决，不能把这组局部结果称为整个主缓存验证。两块完整小缓存和这些子树都保留 producer typed layout 未验证、payload checksum 缺失的限制。

私有证据为 `qq-image-editor-static-7whpmcg1`、`qq-image-editor-static-n_nqb3yl`、`voiceport-qq-service-isolation-2oFRcs` 下的 `session-upstream-jcy0e_57`、`session-functions-p_jzlxta`、`session-caller-listings-wr45_nw6`，以及 `qq-init-name-prefix-hkk5ad6_/provenance.json`。同范围的后续被动路由实测为 PID `97183`、退出 `2`、外层未超时：唯一 renderer 为 `login.html` 根路由，DOM 和加载完成，没有观察到 `/loading` 或 `/main` 导航；加载错误码为空、preload/renderer error 计数均为 `0`、无 renderer 退出。session 已创建，init/ready/BDH 仍未观察到；8 个 xhr 被阻止。回执 `voiceport-qq-service-isolation-2oFRcs/service-route-evidence/result.json` 确认无剩余副本进程、原版指纹保持及深度严格签名通过。观察器只记录固定元数据，没有页面求值、强制导航或 IPC/session/init 方法调用，没有联网、登录或音频验证。

后续恢复了登录管理父模块 `10254` / SFI `23490`：主缓存序列化偏移 `1093140`–`1154266`、1,344 对象、2,336 字节 bytecode，代码 SHA-256 为 `7206408b11e0fd48c62d86ce7415addbde304070e42c77ca665aa1c4029cabea`。有限候选搜索从 128 扩至 256 个 header，每次解析仍遵守上述对象/字节限制；结构及属性名检查仅保留 phase `5`。这也解决了先前单独恢复子函数时的常量歧义，`24270` 的末尾错误标签可确定为 `sessionResult`，没有靠名称猜测分支。

该父模块进一步建立以下静态关系：

- 构造函数 `24206` 在 `69`–`74` 将 `sessionInited` 设为 false；`24459.start` 据此调用 `openLoginWindow()`，仅在它为 true 时调用 `showWindowsRoOpenMainWin()`。
- `24424.registerNTListener` 向业务 facade `72421` 的 session listener 分别注册 `handleNTSessionCreated` 和 `handleSessionInitComplete`，并向登录 listener 的 `onQRCodeLoginSucceed` 注册 `24249`。两个 session 回调没有混作一个阶段。
- `24220` 的成功分支注入 wrapper 业务服务并调用 `46301.nG()`；`24270` 的成功分支设置 `sessionInited=true`、初始化 app session/window pool，调用 `46301.EE()`，随后触发 `onSessionInitComplete`，后续异步路径再触发 `onMainWindowOpen`。事件名由独立的 548 对象 / 27 KiB 主缓存前缀确认；完整窗口创建 handler 仍未恢复。
- 父模块 context slot `10` 明确导入 `46301`。该辅助模块 `49950` 的 95 对象子树仅 phase `7` 通过检查；导出 getter `50027` / `50017` 分别返回 `49969` / `49976`。两者只更新本地阶段并用固定 `done` 完成对应 Promise，没有调用 `startWrapper`、`startSession` 或 native init。
- 父模块 context slot `21` 明确导入 `60629`。`24249` 是一条具体的 QR 登录成功路径：先调用 `setAuthData`，再于 `84`–`99` 调用 `60629.ntWrapper.startSession()`。这是一个初始化触发点，不能推出所有启动都必须登录或联网。

登录窗口 provider 的导入也已绑定到 `66869` / SFI `57`。从相同的 548 对象前缀解码可见，`200.init` 调用 `setup`，正常分支再调用 `showLoginWindowIfNeeded`；`307` 的 `--hide-login-window` 检查只控制 `show` / 首次显示报告。`262.setup` 注册 IPC、发送固定 `loginReady` 并安装窗口事件回调；其 `--from-installer` 分支调用 `60629.ntWrapper.readyToShow()`，没有直接调用 `startSession`。包装层 `53268.readyToShow` 仅转发到 native engine 的同名方法，其 native 内部行为没有由这段 JavaScript 解析建立；不能把它当作 session 初始化已完成。`212.load` 将 provider `21679.gW()` 的返回值交给 `loadURL`。继续核对既有完整 `background.js` 缓存：SFI `14` 在 `92`–`101` 注册 `21679 → 622`；`622` 的导出 getter `652` 返回 context slot `3` 中的 `677`，后者直接返回固定 `app://./renderer/login.html`，没有 hash。`670`、`684` 分别返回独立的 `index.html`、`newLogin.html` URL。缓存仍是上述已保存的 71,824 字节 / 1,991 对象结果，没有新的 QQ 执行。

随后用自写 ASAR 数据读取器核对安装包中的 `renderer/login.html` 和 `renderer/newLogin.html`，分别为 27,776 / 30,432 字节；实际 SHA-256 与归档各自声明的 SHA256 完整性值一致。对应 SHA-256 为 `49b2f760d3614012b224fc1d5e353d81cb3ee9ccbf9ff643c370c1268d2caac8`、`5ddce3f635314bfaeb8eb170d554cb46b3c42b9563643042bc7aba1d786b1149`，回执 `qq-login-html-static-u_iufvkr/result.json`。原始资源不能严格解码为 UTF-8；下述静态原生追踪及自写数据解码已恢复可读页面。

已安装 QQNT 的完整 SHA-256 仍为 `089a9ce1825e2e5d0ff2327b3ee8f24e0c329b1b90d5b0d71f18653d248170cc`。其内嵌 `asar-fs-wrapper.ts` 将绑定设为 `process._linkedBinding('electron_common_asar')`，在资源完整性检查后对 `application.asar` 数据调用 `getPlain`。自写 Mach-O 引用扫描与仅文件 LLDB 反汇编确认：arm64 中模块导出回调为 `0x2a167bc`，Archive 方法回调为 `0x2a17b7c`，经 `0x2a3ac14` / `0x2a3ac18`、`0x56d2a44` 到 `0x56d26e0`。算法描述表的 NID 为 `427`，block/key/IV 长度为 `16/32/16`，mode flags 为 `2`；初始化函数带有 BoringSSL `e_aes.cc.inc` 源文件标记。控制流取资源前 16 字节作 IV，使用静态程序资源常量中的 32 字节密钥解码余下字节；该入口保留默认填充。由此确定 AES-256-CBC / PKCS#7，区别于先前没有资源调用关联的通用 AES-128-ECB 工具。地址由指令和数据配对确认，没有沿用 LLDB 最近导出符号的名称作函数归属。

自写脚本只用 Node 内置 crypto 处理数据，未调用 QQ 绑定或执行 HTML；两份输出均通过填充校验、严格 UTF-8、doctype/head/html 结构检查。`login.html` 明文为 27,748 字节 / SHA-256 `687186ff3a7281f3f58dff1f45e9f5a314c472ca717ac0d1a73de7c42773f9b2`；`newLogin.html` 为 30,407 字节 / `4d815b81283ec5eacda57ec794f5fccd165a7f50bf660723061ddba35e277a59`。私有证据为 `qq-getplain-static-drapwg92` 和 `qq-login-html-decoded-iVkSXB/result.json`。两页分别有 25 / 22 个 script 标签，最后通过零延迟定时器调度 `window.electron.load('r_login')` / `r_newLogin`。本机原生缓存表分别将名称配对到 universal 文件偏移 `215469160` / `34968` 字节和 `215504128` / `37120` 字节；自写完整小缓存读取分别得到 790 对象 / 82 编译函数及 849 对象 / 88 编译函数，均无 forward registrations。缓存 SHA-256 分别为 `7805ab4c01594eb2a4d7b82306ffe41991737443ca5514d41681ec30ce8462af`、`bc8e4fab4f770bb6f36a2a23c95e3c6903315092fbf9ad94035afc656a3f86dd`。这是实际页面入口的静态绑定，未证明 native session 初始化、AudioSDK 就绪或转写成功。两个缓存继续保留 typed profile 未验证、payload checksum 缺失的限制。

### 登录页面的 UI 就绪与事件响应

`r_login` 的 SFI `50` 先检查 `window.isLoginReady`；已有真值则设置组件标志，否则注册 DOM `loginReady` 回调 `57`。渲染函数 `88` 仅在组件标志为真时渲染 `Login`，否则返回 `v-if` 占位。相应 `preload_login` 的 `2304` 注册 `ipcRenderer.once('loginReady', 2325)`；`2325` 先加载模块 `597`，再通过 `contextBridge.exposeInMainWorld('isLoginReady', true)` 暴露标志，最后派发 DOM `CustomEvent('loginReady')`。结合主端 `262.setup` 的发送点，这建立了主端、preload、DOM 和组件显示条件的静态链。`loginReady` 表示这里的 UI 显示条件，不等于 native session 已初始化；离线实测没有记录这个标志或事件，不能据此认定事件漏发。

为继续追踪 `r_login` 的动态导入模块 `17571`，只读取候选登录业务缓存的原顺序前缀：候选总长 `264312` 字节 / SHA-256 `23bec83ae545bbd51a318d06fd2f2fae00f51e78713318bb8c38e17a79942fe4`；解析从 `32` 到 `240873` 停止，见到 5,687 对象，仅保留 5,673 个已闭合对象，14 个未闭合祖先明确排除。没有放宽每次 10,000 对象 / 256 KiB 限制，也没有分段补齐全缓存。原始模块号附近呈注册指令形态，但父函数没有闭合，因此模块 `17571` 的完整归属与父级调用顺序仍待核对。以下仅描述已闭合函数的局部行为：

- 已闭合的 Store 类工厂 `248` 在 `218`–`229` 导入模块 `41386` 到 context slot `10`。其 class boilerplate `262` / 实例表 `265` 将 `bindPushEvent` 放在方法参数 `43`；工厂于 `732` 创建闭包 `918`，写入寄存器 `r52`，并在 `1101` 的 104 参数类创建调用中使用该位置。这建立了 `bindPushEvent → 918` 的指令绑定。`918` 使用上述 provider 的 `ntApi`，按顺序订阅登录事件，再订阅 `onSessionInitComplete` 和文件系统检查事件，保存各自 disposer；这个函数安装监听，没有发起 session 初始化。
- `1954.handleLoginConnected` 完成 `waitForLoginConnected`，再按模式分支：`SCAN` 设置 PENDING 并调用 `invokeGetQrcodeImg`；`AUTO` 仅在 `!isUserLogin && isAutoLogin && canAutoLogin` 时调用 `doAutoLogin` 并清除 `canAutoLogin`；其他模式更新 CONNECTED 状态。`969` 的二维码路径设置 LOADING/NONE 后等待 `getQRCodePicture()`。
- `2042.handleSessionInitComplete` 按 `result === 0` 分支：失败时报告、显示对话框并设置 FAILED，其中 `-10121` 使用特殊内容；成功时调用成功报告及 `report(account)`。这个处理函数没有调用 `startWrapper`、`startSession` 或 native init。分析只读取程序字段和指令，没有读取实际账号或回调参数。

另有已闭合函数 `4873` 的自身 ScopeInfo 明确命名为 `mounted`，在 `22` 调用 `this.loginStore.bindPushEvent()`；`4896` 命名为 `unmounted`，在 `10` 调用 `unbindPushEvent()` 并处理登录注册回调的清理。`4858.created` 先调用 `initLoginAegisService`，再调用 `initializeLoginPanel`；后者 `4913` 等待登录列表并处理页面显示、预览自动登录与预填。这些名称分别由自身 scope 和函数体支持。不过，UI 父函数 `2945` / bytecode `2946` 没有闭合，尚不能核对这些闭包实际安装到类 `4790` 的指令、其 context slot `8` 的 provider 赋值，以及 `loginStore` 的实际注入。构造器 `4810` 仅给 `loginStore` 设置 undefined 默认值，不能据此断言注入失败。登录统计服务的 `init` 也不是 native session init。

局部证据为 `qq-login-implementation-prefix-6a0vd7lm`、`qq-saved-function-listings-w3xz8w13` 和 `qq-selected-cache-listing-owxigcno`；UI 标志链见 `qq-selected-cache-listing-r179pyoh` 与 `qq-selected-cache-listing-njjxk0qw`。新增类绑定/生命周期指令见 `qq-selected-cache-listing-hq_u5mzy`，本会话对选定索引、寄存器和导入的自检回执为 `qq-login-closed-bindings-x50r0ibt/result.json`，绑定输入图 SHA-256 `82463043a3a58b2b0c3b9c0d5c7021af0bec4dcdc10647bee6d0c6ba489791c9`。这些事件响应不能解释全部初始化调度，也不证明运行中已收到连接、二维码或初始化事件。

新增私有证据为 `session-functions-0l4riekj`、`session-functions-kynm99h0`、`qq-login-callback-listings-x4vrqw45`、`qq-login-callback-listings-63p4fex_`、`qq-login-callback-listings-ykauw1i0`、`qq-session-prefix-names-mfkwoyqs` 及其选定函数 listings。它们解释了已恢复的创建、初始化和窗口分支，未证明离线登录页已执行某条初始化路径。没有读取回调的实际账号/session 参数，没有复用登录态、调用 IPC、强制路由或补造 native 配置；真实初始化与服务就绪仍是下一项运行验收条件。

## 运行库中的开关删除逻辑

文件：`/Applications/QQ.app/Contents/Frameworks/QQNT.framework/Versions/A/QQNT`。

SHA-256：`089a9ce1825e2e5d0ff2327b3ee8f24e0c329b1b90d5b0d71f18653d248170cc`。

以下地址为 arm64 slice 内的未加 ASLR slide 虚拟地址。符号已被剥离，LLDB 邻近的 V8 符号标签不能作为这些函数的名称。

| 地址 | 实际代码证据 |
| --- | --- |
| `0x13b83d0` | 接受参数字符串数组、长度及目标对象，循环处理参数；空数组直接转到下面的结束路径。 |
| `0x13b8470`–`0x13b8474` | 数组遍历完成后同样转到 `0x13b854c`。 |
| `0x13b854c`–`0x13b855c` | 向同一目标对象的方法 `0x13b9740` 传入字符串 `inspect`，长度 `7`。 |
| `0x13b8560`–`0x13b8570` | 再传入 `remote-debugging-port`，长度 `21`。 |
| `0x13b9830`–`0x13b98c0` | 被调用方法在目标对象的有序字符串键树中查找完全匹配项。 |
| `0x13b9914`–`0x13b9928` | 找到键后将树大小减一，调用移除节点的助手；随后释放字符串和节点。 |

这建立了该参数解析路径在结束阶段删除两种调试开关的静态证据，与真实实验未开放渲染调试端口的结果一致。它不证明所有代码路径均不能通过程序内部 API 开启调试，也不证明 `--inspect-brk` 被同一位置删除。

可复现命令（只加载文件，不启动或附加 QQ）：

```sh
/usr/bin/lldb --no-lldbinit -b \
  -o 'target create --arch arm64 /Applications/QQ.app/Contents/Frameworks/QQNT.framework/Versions/A/QQNT' \
  -o 'disassemble --start-address 0x13b83d0 --end-address 0x13b8574' \
  -o 'disassemble --start-address 0x13b9740 --end-address 0x13b9950' \
  -o quit
```

本轮没有修改这些指令、重新签名 QQ 或更改系统保护。直接重复相同渲染调试启动参数没有新增依据。

另通过只读菜单查询确认，当前 QQ 顶层菜单为 Apple、QQ、编辑、窗口、帮助；QQ 应用菜单和帮助菜单的已观察项目中没有开发者工具入口。没有展开聊天窗口列表、读取页面/聊天正文或尝试未知快捷键。此检查不能证明隐藏入口绝不存在。

## 调试 pipe 候选的有界静态复核

同一 QQNT 文件含 `remote-debugging-pipe` 字面量，arm64 未加 slide 地址为 `0x9659e04`（universal 文件偏移 `351772164`）。ADRP+ADD 扫描找到候选引用 `0x54a3740` / `0x54a3744`；该扫描不是所有引用的穷尽证明。

候选先在 `0x54a3748` 检查 pipe 键；不存在时在 `0x54a375c` 检查相邻 port 键。两条路径均汇合到 `0x54a3760`，没有各自启动 handler 的调用。进一步的八组有界文件反汇编确认：

- `0x508b94` 仅载入一个全局指针并返回；`0x22c260` 校验传入字符串的小写形式、查找字符串键树并返回是否存在。
- `0x29a559c` 覆盖前述检查结果，调用全局 getter `0x54a2ec8` 后返回对象 `+0x50`。该调用链没有把 pipe/port 结果作为参数继续传递。
- `0x29a6500` 转到共同 observer 分发器 `0x29a5784`，方法偏移为 `0x60`；实际遍历和虚调用位于 `0x29a58c4`–`0x29a58d0`。
- `0x29a7030` 与其依赖 `0x29a7300` 通过共享助手初始化对象字段 `+0x78`、`+0x80`、`+0x88`；其全部间接依赖和 observer 目标的具体语义尚未恢复。

这组证据没有建立可用的 pipe handler 启动路径，不能仅凭开关字符串和上游 Electron 支持就重启本机 QQ 试验；也不能据此断言所有隐藏入口均不存在。没有执行 pipe 启动或连接。LLDB 的邻近导出符号名称不能当作这些剥离函数的真实名称。

命令和完整输出保存在私有调查目录的 `qqnt-pipe-branch-a-*` 及 `qqnt-{pipe-switch-helper,commandline-getter,pipe-common-one,pipe-common-two,pipe-common-three,browser-instance,browser-observer-dispatch,common-three-dependency}-b-*`。八组新增调用均退出 `0`，只创建文件 target 并反汇编，没有 run/attach 或原生执行。

## 已恢复的渲染端与原生服务标识

文件：`/Applications/QQ.app/Contents/Resources/app/major.node`。

SHA-256：`0a7d8421e99ace344ff70720c6fb89389d7594e46d1ed22b60fa5207ab67cb79`。

下表是**文件起点的十进制字节偏移**，不是虚拟地址。内容来自序列化字节码中的字符串；不能把相邻字符串直接拼成 JavaScript 调用。

| 标识 | x86_64 文件偏移 | arm64 文件偏移 | 范围 |
| --- | --- | --- | --- |
| `app://./renderer/index.html` | 7397640 | 118632016 | 安装包内主页面 URL 字面量。 |
| `app://./renderer/login.html` | 7397832 | 118632208 | 登录页 URL 字面量。 |
| `IPC_UP_` | 16453357 | 127687661 | preload RPC 前缀。 |
| `IPC_DOWN_` | 16453393 | 127687697 | 相反方向 RPC 前缀。 |
| `eventName` | 16455414 | 127689718 | RPC 区域内的字段名。 |
| `cmdName` | 16456159 | 127690463 | RPC 区域内的字段名。 |
| `callbackId` | 16456567 | 127690871 | RPC 区域内的字段名。 |
| `preloadApi` | 16467794 | 127702098 | 靠近 `mounted` 的 preload 标识。 |
| `audioSDK` / `AudioSDK` | 16468428 / 16468448 | 127702732 / 127702752 | preload 模块注册标识。 |
| `nodeIKernelBdhUploadService` | 1508805 | 112743205 | AudioSDK 的 SmartPtt 上传区域。 |
| `taskId` / `sliceParam` | 1508866 / 1508888 | 112743266 / 112743288 | 同一服务区域的请求字段名。 |
| `proxyInvoke` | 16497819 | 221558851 | 与 Electron IPC/contextBridge 标识处于同一字节码区域。 |
| `browser-window-proxy` | 16497952 | 221558984 | 窗口代理通道，主端含 `winProxyMethods in CommonAuxWindow.ts` 白名单诊断。 |

另外，x86_64 文件偏移 `37939727`、`37939751`、`37939965`、`37942904`、`37944369` 分别有 `audioSDKApi`、`onStreamASRText`、`onStreamWaveData`、`streamCaptureStart`、`streamCaptureStop`。

`browser-window-proxy` 的证据指向带白名单的窗口方法调用，不能用作任意原生服务调用通道。`audioSDKApi` 等标识尚未证明暴露在 renderer 的 `globalThis` 上；探测器只检查固定候选，不假定存在。

ASAR 内 `preload.js`、`preload_chatwin.js`、`preload_login.js` 和 `renderer/index.html` 的载荷为不透明二进制，逐项 SHA-256 与索引相符。扫描未找到可读 SmartPtt 调用代码，不能从缺少明文推断功能不存在。

## 已解码的 preload 主世界能力

新定位的 `major.node` 程序缓存候选位于 universal 文件偏移 `127639664`、长度 `92048`、arm64 VM `0xf96070`，SHA-256 为 `f3fdca295e87640f9fb5a68d08092fc46038f9491f6759f3d7f7d03492adf84c`。匹配快照/header/flag/external-reference 检查通过，得到 2,414 个对象、273 个函数，无未解决引用或函数警告，尾部为允许的 9 字节 NOP。仍保留 Windows x64 typed profile 限制；没有执行缓存，也尚未将该候选与某个具体命名 preload 入口建立完整加载对应关系。

本缓存的 SFI `1367` 在偏移 `32`–`79` 调用 `contextBridge.exposeInMainWorld('ipcRenderer', {send, invoke, on})`。若 `window.hasLoadPreloadEx` 不为真，偏移 `95`–`153` 另外暴露 `webContentId` 和 `ipcImpl`。SFI `1531` 在 `63`–`86` 暴露 `waitForWindowTypeEnsure`。因此不能仅检查 `electron.ipcRenderer` 就判定这段 preload 没有 IPC 能力。

这里的 `window.ipcRenderer` 是 QQ 客户端的包装：`send`（SFI `1388`，`22`–`41`）转发到 `this.client.send`；`invoke`（`1396`，`26`–`54`）转发并等待 `this.client.invoke`；`on`（`1404`，`0`–`33`）通过 `this.client.handle` 注册，并保存 disposer。它们不能直接等同于 Electron 原始 IPC 方法，也不表示可以越过服务注册或权限约束。

已据此修正 `renderer_probe.cjs`：保留原有候选，新增直接 `globalThis.ipcRenderer.send/invoke/on`、`ipcImpl`、`waitForWindowTypeEnsure` 的 `typeof` 检查。表达式不调用这些方法，不读取 ID/页面/登录信息；仍要求真实 QQ 所属的回环调试端点及 `throwOnSideEffect: true`。新增夹具覆盖没有 `electron` 命名空间、只有 QQ 风格主世界能力的情况，完整 Node suite 为 26 项通过。没有因此重试真实调试启动；外部加载/执行入口仍是阻塞项。

SFI `1603` 的 `9`–`40` 另将 `{mounted: closure1612}` 暴露为 `preloadApi`，是生命周期接口；`1621` 的 `12`–`14` 仅定义枚举 `AudioSDK = 'audioSDK'`，并未在该处暴露 AudioSDK 对象。继续读取包含它的模块函数 `892`：`1122`–`1270` 的注册调用依次为 Meta、Argv、DevConfig、Qrtc、TitleMenu、Experimental、ExperimentalDebug，所检查的注册序列没有 AudioSDK；`1643` 仅定义窗口类型枚举。`1503.loadModules` 按配置中的 `identifies` 选模块并调用 init，不能从枚举字符串推断某个实际页面已经加载 AudioSDK。这些证据不排除其他缓存/入口注册该模块。

私有证据：`preload-cache-candidates-40ef-a.json`、`voiceport-qq-preload-extract-cuubgajx/provenance.json`、`voiceport-qq-preload-parse-{N4RCDk,1NGbaP}` 和 `voiceport-qq-disasm-{3pRsfP,qrUMFJ,4DwTUK,IrZzCf,Hvsn6J}`。选择性反汇编的 14 + 5 + 1 + 1 + 2 个函数全部退出 `0`，零指令警告/错误；最后两函数来自新一轮严格解析 `voiceport-qq-preload-parse-6PW2eL`。

## AudioSDK 的内部 RPC 分发与线格式

主缓存模块 `84565`（SFI `95784`）的 `audioSDKApi` getter `95922` 返回 context slot `23`；父函数偏移 `1996`–`2023` 用服务枚举 `AudioSDK` 和惰性闭包 `96719` 构造该对象。闭包实际导入模块 `42887.Z`，与已经恢复的 AudioSDK 实现相连。因此，之前没有发现直接 import 不能作为模块未连接的证据。

传输模块 `95501` 的 `fromService`（SFI `126695`）把服务名和第二个参数存入 `channelDynamicLoadHandler`；第二个参数是惰性实现来源。SFI `126406` 调用 loader，若结果通过 `126411` 的函数/prototype 检查，则从 `container.get` 取得实例，否则直接使用结果。入站 `126400` 读取 `cmdName` / `payload`，解析实现、检查方法、应用 `beforeRunHandle`，最后保持 receiver 调用 `handlers[cmdName](...payload)`。

通用底层调用是三个位置参数：

```text
ipcImpl.send(sendPrefix + peerId, metadata, body)

请求 metadata = {type: 'request', callbackId, eventName: service, peerId}
请求 body     = {cmdName, cmdType: 'invoke', payload: argumentArray}

事件 metadata = {type: 'request', eventName: service + '-emit', peerId}
事件 body     = {cmdName, cmdType: 'emit', payload: argumentArray}

响应 metadata = {callbackId, promiseStatue: 'full' | 'fail',
                 type: 'response', eventName, peerId}
响应结果或错误作为独立参数传递
```

`promiseStatue` 是程序中的实际拼写。请求元数据与方法参数不在同一个对象里；专用 main-to-renderer invoker 另有 `[payload]` 包装，不能混用。证据分别为 SFI `126356`（底层发送）、`126302`（请求）、`126484`（invoke body）、`126382` / `126294`（事件）、`126343` / `126260`（响应与 pending 清理）。通用窗口 peer 取 `window.webContents.id`。preload 的 `1196` / `1338` 确认客户端使用 `IPC_UP_` / `IPC_DOWN_`；主缓存相应模块 `28861` 的具体常量仍未定位，不能据此声称所有专用路由后缀都已验证。

模块 `14218` 的 `beforeRunHandle`（SFI `95729`）按接口元数据或包装标记决定是否给参数数组前置 `{win: getWin(), isEvent: true, ...ipcCtxExtends}`，否则保持原数组。这是上下文参数注入，不能单独当作权限检查。窗口路由还有 channel 实例、`registeredEvents` / `registeredInvokers` 条件；独立 webview 路由有自己的身份与权限回调，完整 AudioSDK 可达性仍待验证。

注册表模块 `3869` 的 AudioSDK 字面量列出 `bind` 事件 `onPlayEnd`、`onStreamASRText`、`onStreamWaveData`，以及 `invoke` 方法（包括 `streamCaptureStart` / `streamCaptureStop`）。主端出站工厂 `95793` / Proxy `95803` 读取 `bind` / `handle`，其中 `bind → emit` 对应向 renderer 发回事件；入站调用已通过 `fromService` 登记的惰性实现进入 `126400`，不经过这个出站 Proxy。因此不能将 `invoke` / `handle` 名称差异认定为 AudioSDK 入站必然缺少归一化或连接。此前怀疑 SFI `96606` 做归一化，后续解码确认它注册的是其他 webview IPC，该假设已排除。renderer 自己的客户端工厂与实际可达性仍需单独验证，不能直接据此生成外部调用器。

renderer 的 VoiceInputStore 模块 `60058`（SFI `655`，偏移 `152`–`166`）将 `require(41386)` 存入 context slot `5`，后续 `audioSDKApi` 都从该槽取得。消费者明确等待 `checkHasMicrophone()`、`streamCaptureStart({removePeriod})` 和 `streamCaptureStop()`，通过 `onStreamASRText(callback)` / `onStreamWaveData(callback)` 注册并保存返回的 unbind。模块 `41386` 不在这份 1,392 对象 / 110 个编译函数的小缓存中；这建立了准确下一查找目标，不证明完整方法白名单或外部运行入口。新选择性解码证据为 `voiceport-qq-disasm-kVmZip/655.txt`，消费者证据为 `r1Mz8F` 的 `1173` / `1228` / `1259`；producer typed layout 仍未验证，payload checksum 缺失，四个辅助对象未分类。

后续从 `major.node` 偏移 `199732656`、长度 `26080` 的缓存恢复 chunk `90865` 中模块 `41386`。整文件 SHA-256 与本文指纹一致，缓存 SHA-256 为 `45205deac29092a3f7dd2e2bb82f6457ba80a1ccbb8b99de6ad1db8aa3b08631`。SFI `5` 的偏移 `26`–`53` 将该模块注册到 `self.webpackChunkqq_chat`；SFI `16` 的 `303`–`307` 将 `audioSDKApi` getter 绑定到 SFI `168`，`845`–`868` 导入模块 `43145` / `63198`，`1053`–`1072` 执行 `require(63198).Z(require(43145).Qe.AudioSDK)` 并存入 context slot `11`，getter `168` 在 `0`–`4` 返回该槽。因此 renderer 客户端的创建点已建立，通用工厂 `63198` 与服务枚举 `43145` 的定义仍需定位，尚未证明其方法白名单及 IPC 方向映射。

该缓存严格解析得到 766 对象 / 89 个编译函数，图、forward 和 RO 引用全部解决，尾部 4 字节均为允许 NOP；4 个所选函数反汇编无指令警告。一个 `WeakFixedArrayMap` 辅助对象缺少布局，单独报告，不能表述为全对象布局已验证。producer typed layout 未验证，payload checksum 字段缺失；解析及两次反汇编进程均退出 `0`，Node 限制 512 MiB / 25 秒。私有证据为 `voiceport-qq-renderer41386-extract-xkucf8_y/provenance.json`、`voiceport-qq-renderer41386-parse-NOT8ww/summary.json`、`voiceport-qq-disasm-5x05jz/5.txt` 和 `voiceport-qq-disasm-3Ef32p/{16,168}.txt`。没有执行目标代码或取得真实转写。

继续定位到同一程序文件偏移 `149216816`、长度 `93776` 的 chunk `34876`，SHA-256 `9fc717d038fd1a83efd91cedd9121150110fa0abf49e87a81acb6035e316ad55`。这里包含通用工厂 `63198`（SFI `16`）和枚举 `43145`（SFI `561`）：getter `569` 导出枚举，初始化函数 `574` 在 `348`–`350` 将 `AudioSDK` 设为准确服务名 `AudioSDKApi`。工厂 `23` 取得相应 channel 实例及服务声明，Proxy getter `34` 按 `send → invoke → bind → handle` 检查，未声明的方法在 `194`–`230` 抛错。

`AudioSDKApi` 的声明如下；这是该版本 renderer 的静态方法集合，不能推广为任意 native 方法调用权限：

| 类别 | 方法 |
| --- | --- |
| `bind` | `onPlayEnd`、`onStreamASRText`、`onStreamWaveData` |
| `invoke` | `init`、`play`、`stop`、`pause`、`setPosition`、`getPosition`、`getDuration`、`checkHasMicrophone`、`captureInit`、`captureStart`、`captureStop`、`getWave`、`streamCaptureStart`、`streamCaptureStop` |

没有该服务的 `send` / `handle` 声明。SFI `174` 在 `10`–`24` 将 renderer 调用转为 `channel.invoke(methodName, argumentArray)`；SFI `188` 在 `7`–`21` 以 `channel.bind(eventName, callback)` 订阅并返回其结果。SFI `84` 使用 `window.webContentId` 与 `window.ipcImpl` 构建 manager，`105` 在 `80`–`104` 配置 `RM_IPCFROM_RENDERER` 发送前缀与 `RM_IPCFROM_MAIN` 接收前缀。此前通用层的 `IPC_UP_` / `IPC_DOWN_` 与这层 RM 配置不能混为同一通道；底层模块 `10413` 的完整线格式与主端配对仍待追踪。

本缓存解析得到 2,932 对象 / 77 个编译函数，9 字节允许 NOP 尾部，图/forward/RO 引用无未解决项、无校验或指令警告；`WeakFixedArrayMap` / `ClassPositionsMap` 两个辅助对象缺布局，producer typed layout 未验证、payload checksum 缺失。仅选择 23 个不同函数，解析/反汇编均退出 `0`，沿用 512 MiB / 25 秒限制。证据为 `voiceport-qq-renderer-factory-extract-mkmap475/provenance.json`、`voiceport-qq-renderer-factory-parse-YSEHgN/{summary,matching-units}.json`、`voiceport-qq-disasm-e0GiM4/{34,174,188,574}.txt` 和 `voiceport-qq-disasm-x9Dndd/105.txt`。没有执行目标代码、连接 IPC 或进行转写验证。

底层模块 `10413` 已在文件偏移 `180478912`、长度 `22656` 的缓存中恢复（SHA-256 `351becf4a432989d708012fe5da4dfaa9b3d79bc8d2657b387353fb4147a220a`，SFI `43`）。peer 构造 `93` 在 `303`–`344` 直接拼接 `receivePrefix + peerId` 并注册 `ipcImpl.on`；底层发送 `213` 在 `0`–`29` 使用 `ipcImpl.send(sendPrefix + peerId, header, body)`，中间没有额外分隔符。因此本层 outbound/inbound 分别为 `RM_IPCFROM_RENDERER${window.webContentId}` / `RM_IPCFROM_MAIN${window.webContentId}`。

调用 body 由 `398` 在 `30`–`65` 构造为 `{cmdName: methodName, cmdType: 'invoke', payload: argumentArray}`；`154` 在 `96`–`124` 建立 `{type: 'request', callbackId, eventName: 'AudioSDKApi', peerId}` header。事件订阅不是直接发送 invoke：首个 listener 经 `418` 在 `45`–`93` 发送 `AudioSDKApi-register`，body 为事件名；`436` / `442` / `451` 在 `AudioSDKApi-emit` 下按 `cmdName` 找 listener，以 body 的 `payload` 作为一个参数传入；最后一个 listener 释放时，`427` 在 `35`–`83` 发送 `AudioSDKApi-unregister`。

响应 `108` 在 `14`–`129` 按 callbackId 找 pending，`promiseStatue === 'full'` resolve，其他值 reject，随后删除 entry；请求 handler `199` 对结果/异常返回 full/fail 响应。发送失败的 `154` 在 `176`–`221` reject 并删除 entry。超时扫描 `175` / `181` 默认阈值 1000 ms，会 reject 非豁免超时调用并产生本地 `sendErrorMsg`，但所查路径不删除 pending entry；peer 销毁 `122` 只移除接收 channel 的 listeners，没有排空 pending promises。外部适配器不能据此假定内部已完整处理超时/销毁清理。

该块 566 对象 / 62 个编译函数，图/forward/RO 引用全部解决，4 字节允许 NOP，零校验/函数/指令警告；5 个辅助对象缺布局（4 个 `ClassPositionsMap`、1 个 `WeakFixedArrayMap`），producer typed layout 未验证、payload checksum 缺失。仅选 25 函数，解析/反汇编进程均退出 `0`，资源限制保持。私有证据为 `voiceport-qq-wire10413-extract-qusaiogf/provenance.json`、`voiceport-qq-wire10413-parse-3nomGC/summary.json` 与 `voiceport-qq-disasm-ZQCmHt/` 中上述 SFI 文件。下一连接是 preload 的 `window.ipcImpl` 具体适配与主端注册；仍没有兼容外部执行入口或真实服务结果。

preload 的适配已继续核对：模块 `298` / SFI `847` 在 `5`–`12` 导入 Electron；父函数 `892` 在 `469`–`501` 创建 `{send, on, removeAllListeners}` 并存到 context slot `30`。当 `hasLoadPreloadEx` 为 false，`1367` 在 `85`–`153` 将该对象暴露为 `window.ipcImpl`。`1127` 在 `8`–`27` 将 `send(channel, ...args)` 原样转给 Electron `ipcRenderer.send`；`1134` / `1140` 用 `ipcRenderer.on` 注册 callback，保留 Electron event 和其他参数；`1147` 在 `0`–`13` 转发 `removeAllListeners(channel)`。这些具体函数没有通道过滤或重写；另行暴露的 `window.ipcRenderer.send` 经 `this.client.send`，属于另一条链。

已有主端模块 `53694` 的 `126775.registerIpcMainForWindow` 按 `win.webContents.id` 缓存实例；构造 `126943` 在 `130`–`255` 通过 `jW(win, options)` 取得适配器，配置同一窗口 peerId 和 prefix，`126983.makeChannelInstance` 建立 service channel。这与 renderer 结构相符，但 `jW`、prefix 导出 `zD` / `w4` / `_W` 和具体主端 listener 尚未完整核对，不能据此声称已证明准确 RM channel 的 `ipcMain.on` 注册。`hasLoadPreloadEx === true` 的替代 provider 也未验证。

本轮无新提取，5 个 preload 函数新反汇编退出 `0`；复用严格解析为 2,414 对象 / 273 编译函数、9 字节允许 NOP、零 forward/RO 未解决引用和指令警告，16 个辅助对象缺布局，typed profile 未验证、payload checksum 缺失。证据为 `voiceport-qq-preload-parse-vQHDbX/summary.json`、`voiceport-qq-disasm-ybh23K/`、`voiceport-qq-disasm-9KQwLz/847.txt`、前述 `Hvsn6J/892.txt` / `3pRsfP/1367.txt` 和 `voiceport-qq-disasm-DnAuKb/{126775,126943,126983}.txt`。没有执行 preload 或调用真实 IPC。

主端缺失依赖已进一步恢复：模块 `53694` 的父函数 `126648` 在 `80`–`89` 导入 `28861` 至 context slot `5`，在 `91`–`102` 导入 `42261` 至 slot `6`；构造 `126943` 使用 `42261.jW(win, options)`、`28861.zD + w4` 作为发送前缀、`28861.zD + _W` 作为接收前缀。模块 `80416` 的 peer 构造 `126253` 在 `303`–`344` 注册 `ipcImpl.on(receivePrefix + peerId, handleEvent.bind(this))`，发送 `126356` 使用 `ipcImpl.send(sendPrefix + peerId, header, body)`。

在程序文件偏移 `112395752`、长度 `123080`、SHA-256 `2976a9c76995d19345b5bb25c3d6064b6fa3bd6ad1a0a2170497f6c217763aa5` 的缓存中，模块 `28861` / SFI `579` 在 `54`–`64` 写入准确字面量：

| 导出 | getter SFI | 值 |
| --- | --- | --- |
| `zD` | `601` | `RM_IPC` |
| `_W` | `589` | `FROM_RENDERER` |
| `w4` | `595` | `FROM_MAIN` |

因此主端接收 `RM_IPCFROM_RENDERER + win.webContents.id`，发送 `RM_IPCFROM_MAIN + win.webContents.id`，与 renderer 的方向准确配对。模块 `42261` / SFI `611` 的 getter `620` 导出适配工厂 `626`：`on`（`636` / `643`）调用 `require(72298).ipcMain.on(channel, wrapper)`，原样传递 event 和其余参数；`removeAllListeners`（`650`）调用该 provider 的同名方法；`send`（`657`）在窗口销毁时返回，否则通过传入的 webContents override 或 `win.webContents` 发送原 channel 与参数。这些 listener/转发函数没有 `event.sender` 比较；peer 后缀只证明路由，不能当作调用方鉴权或整条链无权限检查的结论。provider `72298` 的 Electron 绑定仍需单独核对。

该缓存严格解析为 2,970 对象 / 284 编译函数，5 字节允许 NOP，零校验/函数/指令警告和未解决图/forward/RO 引用；5 个辅助对象缺布局，producer typed layout 未验证、payload checksum 缺失。保持 10,000 对象上限、Node 512 MiB / 25 秒，只选 11 函数，解析与反汇编进程均退出 `0`。证据为 `voiceport-qq-main-adapter-extract-lo_lzjti/provenance.json`、`voiceport-qq-main-adapter-parse-zdHwrW/summary.json`、`voiceport-qq-disasm-88nFNs/579.txt` 和 `voiceport-qq-disasm-rZ4UBj/{589,595,601,626,636,643,650,657}.txt`。没有执行目标代码、注册真实监听、连接 IPC 或取得转写。

最后的 provider 来源也已核对：复用文件偏移 `118606376`、长度 `71824`、SHA-256 `681bc65fba69907758f8efb6b4f3a4f6e9b3d7a2a09fde55f9c608b29df5fb8a` 的缓存，外层 `5` 将形参 `require` 存入 context slot `3`，`14` 在 `346`–`357` 将 `1414` 注册为模块 `72298`；`1414` 在 `0`–`12` 调用该 `require('electron')` 并赋给 `module.exports`。因此 `42261` 的 `ipcMain` / `BrowserWindow` 来源已由代码建立，不能再写成仅由属性名推断或来源未知。

这补齐了已重建的静态路径：renderer `audioSDKApi` → 声明方法 Proxy → preload Electron `ipcRenderer` 适配 → `RM_IPCFROM_RENDERER + webContents ID` → 主端 Electron `ipcMain` 适配 → `AudioSDKApi` handler。它不证明这些 chunks 在实际窗口上已安装、服务已就绪、请求被接受或已经转写，也没有建立外部 VoicePort 调用入口。`hasLoadPreloadEx` 替代分支仍未验证。

本轮无新提取，只选 4 个函数，严格解析/反汇编退出 `0`：1,991 对象 / 216 编译函数、6 字节允许 NOP、零校验/函数/指令警告及图/forward/RO 未解决引用；2 个辅助对象缺布局，producer typed layout 未验证、payload checksum 缺失。保持 10,000 对象、512 MiB / 25 秒限制。证据为 `voiceport-qq-login-extract-ecp6_85_/provenance.json`、`voiceport-qq-electron-provider-parse-fn4Zbr/{summary,matching-units}.json` 和 `voiceport-qq-disasm-BLrjFF/{5,14,1414}.txt`。该缓存与某个命名启动入口的整体对应关系仍未验证；模块定义及 require 绑定来自所列具体指令与 scope 元数据。没有运行 QQ 或发起 IPC/音频任务。

主实现 `3812.emitStreamASRText` 在记录窗口存在且未销毁时，通过 `audioSDKApi.onStreamASRText(streamRecordWindow, eventPayload)` 将结果发回该窗口；否则使用只传 eventPayload 的广播分支，`eventPayload.isEnd` 时清空记录窗口。这是静态事件路由，不代表已取得真实文字或窗口状态。

传输分析选择性读取 100 个函数，六次解析/反汇编子进程退出 `0`、零警告；上下文 hook 后续 11 + 1 个函数同样通过。保留匹配快照与 Windows x64 typed profile 限制。私有原始证据为 `voiceport-qq-disasm-{cAN8Ef,DnAuKb,7Aq2WD,an0y1R,cBV9wf}`，以及 getter/注册表证据 `nFbvEr` / `yJKzsl`。没有连接或调用真实 QQ IPC。

## 程序内部开发工具候选

主缓存的 `SimpleLoginWindow`（模块 `66869`）有明确快捷键：SFI `286` 的 `0`–`36` 要求 `input.control && input.shift && input.code === 'KeyI' && input.type === 'keyDown'`，`38`–`58` 调用 `win?.webContents.toggleDevTools()`。这里检查的是 **Control**。SFI `262.setup` 在 `233`–`261` 注册 `before-input-event`，该注册附近没有开发配置谓词；调用者 `254.open` 在 `22`–`67` 使用现有 `global.loginWin`。

最初两组严格解析/反汇编退出 `0`、19 个不同函数零警告，证据为 `voiceport-qq-disasm-{V5yQqH,YlcqNW}`。后续对创建条件的检查补上了关键限制：

- 另一程序缓存位于文件偏移 `118606376`、长度 `71824`，SHA-256 为 `681bc65fba69907758f8efb6b4f3a4f6e9b3d7a2a09fde55f9c608b29df5fb8a`。SFI `64` 在 `104` 创建 `webPreferences` 字面量，明确包含 `devTools: false`、`nodeIntegration: false`、`contextIsolation: true`、`sandbox: true`；随后只补入 `preload_login.js` 和 partition，在 `146` 构造 `BrowserWindow`，`352`–`358` 将同一实例赋给 `global.loginWin`。字面量到构造调用之间没有开启开发工具的覆盖。
- 该块匹配快照的严格解析得到 1,991 个对象、216 个函数、6 字节允许 NOP 尾部，无未解决引用或函数警告；两函数反汇编退出 `0`。证据为 `voiceport-qq-login-parse-nuDr0J` / `voiceport-qq-disasm-Ze2Q9f`。尚未建立命名缓存入口及真实执行的完整对应关系，保留 typed profile 限制。
- 余下含登录窗口标识的 659,800 字节缓存（偏移 `111716840`，SHA-256 `e531ca56781c8d5b2b915c27e21845d336ac81424d96eaacc8f8d5ea9076fe79`）也做了严格解析：14,012 个对象、1,372 个函数、9 字节允许 NOP 尾部，无引用/函数警告。选出的几处群视频开发工具调用受模块 `6181.TQ` 控制。继续解码主缓存模块 `6181`（SFI `126128`）确认：偏移 `65`–`66` 将 `false` 存入 context slot `2`，getter `126136` 原样返回该槽。此处是编译后的常量，未发现可用的启动或环境配置开关。原始证据为 `voiceport-qq-disasm-{gzskEi,cKMCE9,LquXD0}`。

因此，登录快捷键与这些条件调用都不足以支持新的运行试验。不能推广为主窗口入口，也没有尝试改变常量或窗口设置。另一个 `showDevtool` 条件属于 `DebugBuiltInBrowserWindow` 的嵌入浏览器，尚未建立适用于语音服务的外部触发链；内部 `openDevtool` 事件的派发/权限路径也未完整恢复。没有发送快捷键、打开登录页、登出、调用 IPC 或重启 QQ。

### 截图贴图编辑器的独立 IPC 候选

在本机已有程序缓存中，贴图编辑器模块 `23066` 的 `283.initHtmlEvent` 注册 document `keydown` listener；`265.handleKeydown` 在字节码 `87`–`110` 检查 `ctrlKey && shiftKey && code === 'KeyI'`，在 `112`–`125` 调用模块 `93757.Z.openDevtool()`。这是 **Control+Shift+I**。模块 `93757` 把 `openDevtool` 声明为 `send` 方法，并以 `screenshot` 作为 API 实例 channel；通用方法生成器 `104` / `110` 最终经 `77.send` 调用 `window.ipc.send('screenshot', 'openDevtool')`。它与前述 `window.ipcImpl` / RM_IPC 传输不同。

后续用自写 Python 数据读取器恢复了此前缺失的函数体，静态候选链已补齐。主缓存模块 `82626` / SFI `76737` 的 `196`–`210` 将模块 `72298` 导入 context slot `11`，其宿主 Electron 来源见前文。`592`–`651` 的 class 动态参数顺序将构造、`on`、`init`、`handleSend` 分别关联到 `76758`、`76766`、`76813`、`76820`；`968`–`981` 注册 `on('openDevtool', 76970)`。

- `76758` 将 `channelName` 设为 `screenshot`，并调用 `init()`。
- `76813` 的 `0`–`34` 注册 `ipcMain.on(this.channelName, this.handleSend.bind(this))`，`40`–`74` 另注册 invoke handler。
- `76820` 的 `11`–`30` 将接收的 `(event, methodName, ...args)` 转为 `this.emitter.emit(methodName, event, ...args)`。
- `76970` 的 `8`–`23` 执行 `event.sender.openDevTools({mode: 'detach'})`。所查转发和这个回调中没有额外 sender 比较；这不证明整条运行路径没有其他限制。

提供者来自本机 `major.node` 偏移 `128985208`、长度 `2456`、SHA-256 `3ee0c38a0a707fd032b422e9dd952ea0431d05ac61f5790f1659112077308105` 的微型缓存。SFI `12` 导入 `electron`，经 `contextBridge.exposeInMainWorld` 暴露 `ipc` 对象；`27` 将 `send(channel, ...args)` 原样转给 Electron `ipcRenderer.send`，`38` / `45` 转发 `on` / `off`。`invoke` 是从 `ipcRenderer` 读取并交给 bridge 的函数，运行时 receiver 兼容性尚未测试。另行暴露的 `captureScreen` / SFI `52` 含屏幕捕获逻辑，本次没有调用它。

主截图窗口 `78280.createWindow` 在 `108`–`158` 直接构造 `BrowserWindow`：`show: false`，preload 为 `preload_simple.js`，所查 `webPreferences` 有 `nodeIntegration: false`、`contextIsolation: true`、`sandbox: false`，没有显式 `devTools` 键。`77196.setupAuxWindow` 注册回调 `77202`；后者区分 `PinMenu`，两个分支都使用同一 preload，并通过 helper `Z` 合并辅助窗口配置，局部字面量也没有显式 `devTools` 键。不能由此推断 helper、全局 hook 或实际窗口的最终设置。

`77857.init` 的 `1268`–`1285` 仅在 `cacheLiteWindowInFirstTime` 为真时预建隐藏截图窗口。`77243` 登记已创建的贴图窗口；`77165` 是已有贴图的保存对话框路径。这些函数尚未建立从外部提供图片、免屏幕捕获地进入编辑器的完整路径。另一个图像编辑器缓存的初始化仍包含截图组件，不能当作已找到独立文件编辑入口。

新方法只执行自己编写的 Python 读取器，JSON profile / RO 表作为数据读取，没有再执行下载的解析库或 QQ 目标代码。微型 preload 完整读出 `77` 对象、`7` 个 SFI、`5` 字节允许 NOP，图引用全部在本块内解决；独立复现一致，本样本没有触发 forward-registration 分支。主截图局部段只有 `1992` 对象，明确标记 `partial_graph: true`、`external_refs_complete: false`，不能称为整个主缓存的严格解析验收。两个外部字符串引用 `12755 = screenshot`、`58391 = channelName` 分别经已保存 bytecode 锚点的有界读取定位，每次少于 `10000` 对象；`427` 个本地字符串与原始字节一致。其他沿用旧产物的标签不算独立验证。Windows x64 typed profile 与 arm64 producer 的兼容性、缺失 payload checksum 的限制保留。

私有证据为 `qq-simple-preload-wv7ewlf1/`、`qq-local-pools-xbuuwfaq/`、`qq-recovered-listing-{1zehr2t0,550v7mq5}/`、`qq-reference-evidence-{yj24qdb6,thisl989}/provenance.json`。没有通过这条链发送真实 IPC、按快捷键、截屏、读取剪贴板或取得服务就绪报告，更没有真实转写。

两个小缓存的固定范围分别为偏移 `138267488` / 长度 `76768` / SHA-256 `351f749a8e22c8f1e09f099e912f96b0d471ad91157afe18fb199b81a48c9edd`，以及偏移 `200297096` / 长度 `19608` / SHA-256 `25e304c9b2ccdf73c34f6081f47a5c2579d39e8178d76d508e02644000c64249`。静态图校验、所选函数及反汇编无未解决引用或警告；仍保留 Windows x64 typed profile 与本机 arm64 producer 兼容性未验证、payload checksum 缺失的限制。私有产物为 `qq-local-entry-parse-{f8GsCG,wLk4mP,kY26eh}`、`voiceport-qq-disasm-{qaNJoq,qNYjhm,a32nTi}`。这两块的初始操作运行了静态解析库，没有执行 QQ 目标代码；上面新增的缺失函数恢复使用自写有界读取器，没有再执行该解析库。

据此为能力探针补上 `windowIpc` 及 `windowIpcSend` / `windowIpcInvoke` / `windowIpcOn`，分别只读取 `globalThis.ipc` 和其三个已知方法的类型。没有该对象时四项均为 `undefined`；存在该对象不等于传输已连接、服务已就绪或调用获准。新的离线夹具确认只有该桥接对象时仍可检测，且不调用方法、读取 document 或无关桥接字段。仍须先从外部证明 QQ 所属回环调试端点，保留 `throwOnSideEffect: true`；没有按快捷键、触发截图、打开 DevTools、发起真实 IPC 或重启 QQ，也没有取得服务就绪报告或转写。

另复核 `major.node` 的 `QQV8BytecodeDebug`：初始化仅在环境值精确为 `1` 时设标记；已检查分支调用 `getLogFilePath`，以 append 模式打开日志并输出路径/版本/flag hash。尚未找到由它开启任意模块加载或调试服务器的路径，没有设置环境变量或尝试启动。

## 本地多实例服务的后续候选

另从程序的 `afterIndex` 路径发现 `module41524.Z.createServerPortHeartBeat()`。已选择性解码确认模块 `41524` 是 `multi-instance-cs/BaseLocalServer`：SFI `98998` 等待心跳检查后返回端口；GET handler `99103` 按路径从模块 `57810` 取得 route handler，经过请求校验与封装解析后才分发。POST `99060` 的结束回调 `99082` 也按路径选取已注册 handler；`/tencent` 有专用 URL 分支，其他路径经校验/解包后以 `{port, data}` 信封传给选定 handler。该 dispatcher 没有建立任意 native、eval 或 AudioSDK 调用入口。

继续读取路由注册表模块 `57810`（SFI `99310`）及其构造方法：`100874.getRouteHandler` 只按路径索引 `routeHandlerMap`；构造函数 `100863` 从 context slot `44`、`51`、`54` 和 slot `53.Z` 收集四个 controller，随后用 `100895.generateRouteHandlerMap` 生成表。`100901` 读取 controller 构造方法上的 `_routeRoot` / `_routeHandlers`，按预先声明的路径和 methodName 建立闭包；`100912` 保持 controller receiver 调用预选方法，并非让外部任意选 native 方法。四个来源分别为 QQEX、multiinstance/shortcuts、multi-login-report 和导入模块 `47504`。仅检查必要类元数据与分发，没有展开账号/消息/凭据处理。

模块 `47504` 的 `websdk`（SFI `100990`）依次调用 `getParamsError`、`translateToShareQQData`，有分享数据时进入 `urlSchema`，再返回固定 SUCCESS 或 REQUEST_BODY_ERROR。所查方法体没有 generic invoke/eval/AudioSDK 转发；不能因名称 `websdk` 就把它当作 ASR 桥。新增六个所选函数严格解码退出 `0`、无指令错误/警告，证据为 `voiceport-qq-disasm-{RJMb7n,pUiMHy}`。这排除了这些具体分发/分享方法作为已建立语音入口的说法，不是对程序全部路由的穷尽证明。

这提供了一条区别于 renderer/CDP 的静态调查线索，尚未建立它暴露 AudioSDK、支持任意方法调用或可供 VoicePort 使用。所查 dispatcher 与分享方法没有建立服务路由到语音接口的关联。该阶段没有扫描端口、连接服务、调用端点、读取本地会话状态或追踪凭据处理。最初六个所选函数严格解析/反汇编退出 `0`、零警告，证据为 `voiceport-qq-disasm-{0h2QVi,FnUPmt}`；路由注册表与构造的另外五个函数同样通过，证据为 `voiceport-qq-disasm-{BnyOOK,zCxC5z}`。

### 外部协议与现有监听端口的有界复核

本机应用元数据注册 `tencent:` 和 `mqqapi:`。自写读取器仅重建三个具体主缓存子树，分别为协议 listener（`272` 对象）、dispatcher（`1910` 对象）和默认配置（`389` 对象），均保持每次 `10000` 对象 / `256 KiB` 上限，没有完整重解析大型主缓存。它们是部分对象图；根据多个既有子函数长度、对象形状及属性名必须是字符串的约束排除不一致 hot-ring 状态，不能当作 producer typed layout 已获验证。

`90539` 的 `429`–`452` 注册 macOS `open-url` 回调；`90521` 把合格 scheme 交给已初始化时的 `dispatchCacheUrlProtocolForMainWindow`。dispatcher `103881` 在 `168`–`278` 要求命令配置存在，并在 `505`–`550` 检查来源权限；`105137` 检查 `mqqapi` 命令白名单。默认配置模块 `80660` / `124711` 的 `OZ` getter `124737` 返回 context slot `5`，对应所查默认命令表；其中没有截图、图像编辑或调试命令。配置读取还会合并 `57390.Z.schemaConfig`；本次没有读取账号或热更新数据来检查运行时覆盖，不能断言实际命令表与默认表完全相同，也没有建立直接拼 URL 进入调试窗口的方案。

另作一次当前进程的只读观察：QQ 所属回环监听端口包括 `9210`。只对该端口的固定 `/json/version` 路径作一次有界 GET，返回 HTTP `200`，但缺少 CDP version / debugger socket 字段。没有保留原始响应，也没有调用其他端口、协议命令、账号、分享或服务端点。结果只表示该端口未被确认是 CDP，不表示 AudioSDK 就绪，也不构成新的启动开关兼容性证据。

私有证据为 `qq-protocol-subtree-{249g_tav,3948_st0,wmnr7md4}/` 和 `qq-protocol-listing-{zw3x8wzt,0m4ucpvk,eitapt1m}/`。新图像编辑器块 `qq-image-editor-static-bkid2114/` 为偏移 `167767296`、长度 `159160`、SHA-256 `18f0924f43c242bb4e08cc5b0bed4bb08666057a953c66d5daa343d9ff0a5834`，自写读取器读出 `3425` 对象 / `294` 编译函数 / `10` 字节 NOP；所查初始化包含截图组件，尚未建立独立的外部图片入口。没有因此截图、读剪贴板、调用 IPC、重启 QQ 或进行音频任务。

## 已解决与仍待验证的边界

后续对实际程序缓存的选择性解码已经恢复 AudioSDK 的调用对象 `{taskId, sliceParam}`、原生两个位置参数的投影、`chatType: 7`、15 随机字节转 hex 的任务 ID、切片索引与编码字节偏移映射、数值 `isLast === 1`、回调对象字段和文字替换/完成段累加。原生监听注册与 QQ 自己持有的 session 引用链也已恢复。流式模块首片 `02 + #!SILK_V3`、续片无头且保留 encoder、16 kHz 单通道的 AudioSDK 初始化均有静态指令依据，详见 [静态调用契约](SMARTPTT_CONTRACT.md)。这些已超出字符串关联，但仍没有真实服务接受或识别结果。

仍待解决：

- renderer 模块 `41386`、工厂 `63198` 和枚举 `43145` 的调用链、`AudioSDKApi` 声明方法及 RM 收发方向已恢复；底层 `10413` 的 prefix/peer 拼接、调用/事件/响应线格式和清理行为也已恢复；preload 的普通 `ipcImpl` 分支已确认直接转发 Electron IPC；主端适配函数与 prefix 字面量配对也已恢复；主端 `72298` 也已确认导出宿主 `require('electron')`；仍需验证实际 chunk 安装、preload 替代 provider 和窗口可达的服务注册，尚未运行验证。
- 从实际 renderer 可达的调用入口及其权限/状态条件，不能由内部 facade 链直接推断。
- 恢复出的任务、音频和回调约定在本机服务中的接受条件，以及取消/超时和完整错误行为。
- 临时副本已观察到 session 创建和普通 RM renderer listener 注册，但离线启动尚未观察到 session 初始化回调、BDH 或 AudioSDK 就绪；隐藏本地登录页加载完成不证明登录或网络是唯一前提。
- 在原版签名 App Store 构建中可用的无修改进程内加载方式；上述 pipe 候选尚未建立新启动路径。

因此当前不能提供可工作的 QQ JSONL 适配器或可导入的 QQ 来源。下一步查明并验证 QQ 自有 session 的启动和服务就绪条件，再进入单独获准的音频样例验证；副本入口成功不能代替真实服务接受或识别结果。

## 本地检查

```sh
node --test Tests/QQBridgeTests/renderer_probe.test.cjs
```

九项夹具测试覆盖目标/端点限制、流式正文大小上限、固定能力表达式不调用 IPC/读取页面、QQ 直接暴露的主世界能力、独立截图 IPC 对象的检测及无关字段隔离、外部页面拒绝、异常和未知值脱敏、顶层 null 协议消息的结构化失败及目标数量限制。夹具使用本地 HTTP 和受控协议对象，**不等于真实 QQ CDP 已兼容**。
