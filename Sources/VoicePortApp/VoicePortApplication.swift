import AppKit
import ApplicationServices
import Carbon
import Combine
import SwiftUI

@main
struct VoicePortApplication {
    @MainActor
    static func main() {
        let application = NSApplication.shared
        // Diagnose this signed bundle's effective trust without constructing audio,
        // storage or backend dependencies, prompting, or inspecting user text.
        if ProcessInfo.processInfo.arguments.contains("--check-input-permission") {
            let result: [String: Any] = [
                "accessibilityTrusted": AXIsProcessTrusted(),
                "secureInput": IsSecureEventInputEnabled(),
                "bundlePath": Bundle.main.bundlePath,
                "version": Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "unknown"
            ]
            if let data = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]),
               let json = String(data: data, encoding: .utf8) { print(json) }
            return
        }
        let delegate = AppDelegate()
        application.delegate = delegate
        application.setActivationPolicy(.accessory)
        withExtendedLifetime(delegate) { application.run() }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private let controller = DictationController()
    private var item: NSStatusItem?
    private var window: NSWindow?
    private var hotkey: GlobalHotKey?
    private var observation: AnyCancellable?
    private var shortcutObservation: AnyCancellable?
    private let recordItem = NSMenuItem(title: "开始录音", action: #selector(toggle), keyEquivalent: "")
    private let cancelItem = NSMenuItem(title: "取消录音 / 转写", action: #selector(cancel), keyEquivalent: "")
    private let statusItem = NSMenuItem(title: "就绪", action: nil, keyEquivalent: "")

    func applicationDidFinishLaunching(_ notification: Notification) {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        self.item = item
        let menu = NSMenu()
        statusItem.isEnabled = false
        menu.addItem(statusItem)
        menu.addItem(.separator())
        recordItem.target = self
        cancelItem.target = self
        menu.addItem(recordItem)
        menu.addItem(cancelItem)
        let showItem = NSMenuItem(title: "结果与设置…", action: #selector(showWindow), keyEquivalent: ",")
        showItem.target = self
        menu.addItem(showItem)
        menu.addItem(.separator())
        let quitItem = NSMenuItem(title: "退出 VoicePort", action: #selector(quit), keyEquivalent: "q")
        quitItem.target = self
        menu.addItem(quitItem)
        menu.autoenablesItems = false
        item.menu = menu
        hotkey = GlobalHotKey { [weak self] in self?.controller.toggle() }
        shortcutObservation = controller.$alternativeShortcut.sink { [weak self] alternative in
            guard let self else { return }
            let success = self.hotkey?.register(alternative: alternative) == true
            self.controller.hotkeyMessage = success ? "全局快捷键已启用" : "快捷键被占用，请换一个预设，或用菜单录音。"
        }
        observation = controller.objectWillChange.sink { [weak self] in
            Task { @MainActor [weak self] in self?.refreshMenu() }
        }
        refreshMenu()
        // A launch smoke mode exits without requesting microphone/Accessibility or reading credentials.
        if ProcessInfo.processInfo.arguments.contains("--smoke-test") {
            print("VoicePort app launched: menu ready, no recording or network")
            NSApplication.shared.terminate(nil)
            return
        }
        controller.checkInputPermission()
        // Best-effort metadata-only cleanup. Smoke mode returns before this;
        // constructing the controller/recorder/store above performs no store I/O.
        _ = RecordingStorage.cleanupAtLaunch(arguments: ProcessInfo.processInfo.arguments)
        if ProcessInfo.processInfo.arguments.contains("--show-window") { showWindow() }
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        Task { @MainActor in
            let stopped = await controller.shutdownAndWait()
            sender.reply(toApplicationShouldTerminate: stopped)
        }
        return .terminateLater
    }

    func applicationWillTerminate(_ notification: Notification) {
        hotkey?.unregister()
        controller.shutdown()
    }

    private func refreshMenu() {
        statusItem.title = "VoicePort · \(controller.status)"
        recordItem.title = controller.phase == .recording ? "结束录音并转写" : "开始录音  \(controller.shortcutLabel)"
        recordItem.isEnabled = controller.canStart || controller.phase == .recording
        cancelItem.isEnabled = controller.canCancel
        item?.button?.image = NSImage(systemSymbolName: controller.phase == .recording ? "record.circle.fill" : "mic", accessibilityDescription: controller.status)
        item?.button?.title = controller.phase == .recording ? " \(controller.seconds)s" : ""
        item?.button?.toolTip = "VoicePort · \(controller.status) · \(controller.message)"
    }

    @objc private func toggle() { controller.toggle() }
    @objc private func cancel() { controller.cancel() }
    @objc private func quit() { NSApplication.shared.terminate(nil) }
    @objc private func showWindow() {
        if window == nil {
            let created = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 580, height: 570),
                                   styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
            created.title = "VoicePort"
            created.isReleasedWhenClosed = false
            created.contentView = NSHostingView(rootView: ResultView(controller: controller))
            created.center()
            window = created
        }
        NSApplication.shared.activate(ignoringOtherApps: true)
        window?.makeKeyAndOrderFront(nil)
    }
}

private struct ResultView: View {
    @ObservedObject var controller: DictationController
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Image(systemName: "mic.circle.fill").font(.largeTitle).foregroundStyle(.tint)
                VStack(alignment: .leading) {
                    Text("VoicePort").font(.title2.bold())
                    Text(controller.status).foregroundStyle(.secondary)
                }
                Spacer()
                Text(controller.shortcutLabel).font(.system(.body, design: .monospaced))
            }
            Text(controller.message).font(.callout).textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            if !controller.inputDeliveryMessage.isEmpty && controller.inputDeliveryMessage != controller.message {
                Text(controller.inputDeliveryMessage).font(.callout).foregroundStyle(.secondary)
                    .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
            }
            if !controller.localRecognitionMessage.isEmpty {
                Text(controller.localRecognitionMessage).font(.caption).foregroundStyle(.secondary)
                    .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
            }
            ScrollView {
                Text(controller.transcript.isEmpty ? "转写结果会保留在这里。" : controller.transcript)
                    .textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading).padding(12)
            }
            .frame(minHeight: 120).background(.quaternary.opacity(0.3), in: RoundedRectangle(cornerRadius: 8))
            HStack {
                Button("复制原始文字") { controller.copyResult() }
                    .disabled(controller.transcript.isEmpty || controller.phase == .delivering)
                    .accessibilityLabel("复制原始文字")
                    .accessibilityIdentifier("copyTranscript")
                if controller.canRetry {
                    Button("重试这段录音") { controller.retry() }
                        .accessibilityLabel("重试这段录音")
                        .accessibilityIdentifier("retryRecording")
                }
                if controller.canCancel {
                    Button("取消") { controller.cancel() }
                        .accessibilityLabel("取消录音或转写")
                        .accessibilityIdentifier("cancelDictation")
                }
            }
            Divider()
            Toggle("在原输入框实时输入，停录后校正", isOn: $controller.automaticPaste)
                .accessibilityLabel("在原输入框实时输入，停录后校正")
                .accessibilityIdentifier("automaticPaste")
            Picker("识别语言", selection: $controller.language) {
                Text("自动识别").tag("")
                Text("中文").tag("zh")
                Text("English").tag("en")
            }
            .accessibilityLabel("识别语言")
            .accessibilityIdentifier("recognitionLanguage")
            Picker("全局快捷键", selection: $controller.alternativeShortcut) {
                Text("Control + Option + Space").tag(false)
                Text("Control + Shift + Space").tag(true)
            }
            .accessibilityLabel("全局快捷键")
            .accessibilityIdentifier("globalShortcut")
            Text(controller.hotkeyMessage).font(.caption).foregroundStyle(.secondary)
            HStack {
                Button("授权辅助功能") { controller.requestAccessibility() }
                    .accessibilityLabel("授权辅助功能")
                    .accessibilityIdentifier("requestAccessibility")
                Text("首次开始时申请麦克风与语音识别权限。登录请在终端运行 codex login。")
                    .font(.caption).foregroundStyle(.secondary)
            }
            Text("实时预览仅用本机识别；停录后录音上传至 ChatGPT 校正。Orca 终端只追加文字，校正差异保留在面板。本机识别不可用时改为停录后转写。切换输入框或编辑文字会停止自动写入，结果仍在面板。不保存转写历史，也不会自动发送。")
                .font(.caption).foregroundStyle(.secondary)
        }
        .padding(24).frame(minWidth: 510, minHeight: 540)
    }
}
