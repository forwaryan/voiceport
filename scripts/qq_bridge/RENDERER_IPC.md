# QQ 渲染端 IPC 与调试入口证据

2026-11-10，本机 App Store QQ `7.0.1-52892`。本文区分静态字符串、静态控制流和实际探测。SmartPtt 的内部参数转换、回调和 session 来源已静态恢复，见 [本机 SmartPtt 静态调用契约](SMARTPTT_CONTRACT.md)；**没有验证外部可调用的渲染端入口，也没有完成真实转写**。

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

主实现 `3812.emitStreamASRText` 在记录窗口存在且未销毁时，通过 `audioSDKApi.onStreamASRText(streamRecordWindow, eventPayload)` 将结果发回该窗口；否则使用只传 eventPayload 的广播分支，`eventPayload.isEnd` 时清空记录窗口。这是静态事件路由，不代表已取得真实文字或窗口状态。

传输分析选择性读取 100 个函数，六次解析/反汇编子进程退出 `0`、零警告；上下文 hook 后续 11 + 1 个函数同样通过。保留匹配快照与 Windows x64 typed profile 限制。私有原始证据为 `voiceport-qq-disasm-{cAN8Ef,DnAuKb,7Aq2WD,an0y1R,cBV9wf}`，以及 getter/注册表证据 `nFbvEr` / `yJKzsl`。没有连接或调用真实 QQ IPC。

## 程序内部开发工具候选

主缓存的 `SimpleLoginWindow`（模块 `66869`）有明确快捷键：SFI `286` 的 `0`–`36` 要求 `input.control && input.shift && input.code === 'KeyI' && input.type === 'keyDown'`，`38`–`58` 调用 `win?.webContents.toggleDevTools()`。这里检查的是 **Control**。SFI `262.setup` 在 `233`–`261` 注册 `before-input-event`，该注册附近没有开发配置谓词；调用者 `254.open` 在 `22`–`67` 使用现有 `global.loginWin`。

最初两组严格解析/反汇编退出 `0`、19 个不同函数零警告，证据为 `voiceport-qq-disasm-{V5yQqH,YlcqNW}`。后续对创建条件的检查补上了关键限制：

- 另一程序缓存位于文件偏移 `118606376`、长度 `71824`，SHA-256 为 `681bc65fba69907758f8efb6b4f3a4f6e9b3d7a2a09fde55f9c608b29df5fb8a`。SFI `64` 在 `104` 创建 `webPreferences` 字面量，明确包含 `devTools: false`、`nodeIntegration: false`、`contextIsolation: true`、`sandbox: true`；随后只补入 `preload_login.js` 和 partition，在 `146` 构造 `BrowserWindow`，`352`–`358` 将同一实例赋给 `global.loginWin`。字面量到构造调用之间没有开启开发工具的覆盖。
- 该块匹配快照的严格解析得到 1,991 个对象、216 个函数、6 字节允许 NOP 尾部，无未解决引用或函数警告；两函数反汇编退出 `0`。证据为 `voiceport-qq-login-parse-nuDr0J` / `voiceport-qq-disasm-Ze2Q9f`。尚未建立命名缓存入口及真实执行的完整对应关系，保留 typed profile 限制。
- 余下含登录窗口标识的 659,800 字节缓存（偏移 `111716840`，SHA-256 `e531ca56781c8d5b2b915c27e21845d336ac81424d96eaacc8f8d5ea9076fe79`）也做了严格解析：14,012 个对象、1,372 个函数、9 字节允许 NOP 尾部，无引用/函数警告。选出的几处群视频开发工具调用受模块 `6181.TQ` 控制。继续解码主缓存模块 `6181`（SFI `126128`）确认：偏移 `65`–`66` 将 `false` 存入 context slot `2`，getter `126136` 原样返回该槽。此处是编译后的常量，未发现可用的启动或环境配置开关。原始证据为 `voiceport-qq-disasm-{gzskEi,cKMCE9,LquXD0}`。

因此，登录快捷键与这些条件调用都不足以支持新的运行试验。不能推广为主窗口入口，也没有尝试改变常量或窗口设置。另一个 `showDevtool` 条件属于 `DebugBuiltInBrowserWindow` 的嵌入浏览器，尚未建立适用于语音服务的外部触发链；内部 `openDevtool` 事件的派发/权限路径也未完整恢复。没有发送快捷键、打开登录页、登出、调用 IPC 或重启 QQ。

另复核 `major.node` 的 `QQV8BytecodeDebug`：初始化仅在环境值精确为 `1` 时设标记；已检查分支调用 `getLogFilePath`，以 append 模式打开日志并输出路径/版本/flag hash。尚未找到由它开启任意模块加载或调试服务器的路径，没有设置环境变量或尝试启动。

## 本地多实例服务的后续候选

另从程序的 `afterIndex` 路径发现 `module41524.Z.createServerPortHeartBeat()`。已选择性解码确认模块 `41524` 是 `multi-instance-cs/BaseLocalServer`：SFI `98998` 等待心跳检查后返回端口；GET handler `99103` 按路径从模块 `57810` 取得 route handler，经过请求校验与封装解析后才分发。POST `99060` 的结束回调 `99082` 也按路径选取已注册 handler；`/tencent` 有专用 URL 分支，其他路径经校验/解包后以 `{port, data}` 信封传给选定 handler。该 dispatcher 没有建立任意 native、eval 或 AudioSDK 调用入口。

继续读取路由注册表模块 `57810`（SFI `99310`）及其构造方法：`100874.getRouteHandler` 只按路径索引 `routeHandlerMap`；构造函数 `100863` 从 context slot `44`、`51`、`54` 和 slot `53.Z` 收集四个 controller，随后用 `100895.generateRouteHandlerMap` 生成表。`100901` 读取 controller 构造方法上的 `_routeRoot` / `_routeHandlers`，按预先声明的路径和 methodName 建立闭包；`100912` 保持 controller receiver 调用预选方法，并非让外部任意选 native 方法。四个来源分别为 QQEX、multiinstance/shortcuts、multi-login-report 和导入模块 `47504`。仅检查必要类元数据与分发，没有展开账号/消息/凭据处理。

模块 `47504` 的 `websdk`（SFI `100990`）依次调用 `getParamsError`、`translateToShareQQData`，有分享数据时进入 `urlSchema`，再返回固定 SUCCESS 或 REQUEST_BODY_ERROR。所查方法体没有 generic invoke/eval/AudioSDK 转发；不能因名称 `websdk` 就把它当作 ASR 桥。新增六个所选函数严格解码退出 `0`、无指令错误/警告，证据为 `voiceport-qq-disasm-{RJMb7n,pUiMHy}`。这排除了这些具体分发/分享方法作为已建立语音入口的说法，不是对程序全部路由的穷尽证明。

这提供了一条区别于 renderer/CDP 的静态调查线索，尚未建立它暴露 AudioSDK、支持任意方法调用或可供 VoicePort 使用。所查 dispatcher 与分享方法没有建立服务路由到语音接口的关联。没有扫描端口、连接服务、调用端点、读取本地会话状态或追踪凭据处理。最初六个所选函数严格解析/反汇编退出 `0`、零警告，证据为 `voiceport-qq-disasm-{0h2QVi,FnUPmt}`；路由注册表与构造的另外五个函数同样通过，证据为 `voiceport-qq-disasm-{BnyOOK,zCxC5z}`。

## 已解决与仍待验证的边界

后续对实际程序缓存的选择性解码已经恢复 AudioSDK 的调用对象 `{taskId, sliceParam}`、原生两个位置参数的投影、`chatType: 7`、15 随机字节转 hex 的任务 ID、切片索引与编码字节偏移映射、数值 `isLast === 1`、回调对象字段和文字替换/完成段累加。原生监听注册与 QQ 自己持有的 session 引用链也已恢复。流式模块首片 `02 + #!SILK_V3`、续片无头且保留 encoder、16 kHz 单通道的 AudioSDK 初始化均有静态指令依据，详见 [静态调用契约](SMARTPTT_CONTRACT.md)。这些已超出字符串关联，但仍没有真实服务接受或识别结果。

仍待解决：

- renderer 模块 `41386`、工厂 `63198` 和枚举 `43145` 的调用链、`AudioSDKApi` 声明方法及 RM 收发方向已恢复；底层 `10413` 的 prefix/peer 拼接、调用/事件/响应线格式和清理行为也已恢复；preload 的普通 `ipcImpl` 分支已确认直接转发 Electron IPC；仍需核对替代 provider、主端适配/prefix 配对和实际窗口可达的服务注册，尚未运行验证。
- 从实际 renderer 可达的调用入口及其权限/状态条件，不能由内部 facade 链直接推断。
- 恢复出的任务、音频和回调约定在本机服务中的接受条件，以及取消/超时和完整错误行为。
- 在当前签名 App Store 构建中可用的进程内加载方式；上述 pipe 候选尚未建立新启动路径。

因此当前不能提供可工作的 QQ JSONL 适配器或可导入的 QQ 来源。需要先取得兼容且可调试的运行环境，或解出/验证当前客户端本身支持的加载入口；有了这些证据才进入服务和获准音频样例验证。

## 本地检查

```sh
node --test Tests/QQBridgeTests/renderer_probe.test.cjs
```

八项夹具测试覆盖目标/端点限制、流式正文大小上限、固定能力表达式不调用 IPC/读取页面、QQ 直接暴露的主世界能力、外部页面拒绝、异常和未知值脱敏、顶层 null 协议消息的结构化失败及目标数量限制。夹具使用本地 HTTP 和受控协议对象，**不等于真实 QQ CDP 已兼容**。
