import AppKit
@preconcurrency import ApplicationServices
import Carbon
import VoicePortCore

@MainActor
struct PasteTargetState {
    var applicationPID: pid_t?
    var focus: AXUIElement?
    var accessibility: Bool
    var sensitiveInput: Bool
    // Affirmatively confirmed capability for automatic text delivery. False
    // includes unknown/unsupported capability, not just proven read-only state.
    var editable: Bool
}

@MainActor
final class PasteCoordinator: TextDelivery {
    private struct ClipboardSnapshot {
        let changeCount: Int
        let items: [[NSPasteboard.PasteboardType: Data]]

        static func capture(_ board: NSPasteboard) -> Self? {
            let count = board.changeCount
            let originals = board.pasteboardItems ?? []
            guard originals.count <= 64 else { return nil }
            var size = 0
            var items: [[NSPasteboard.PasteboardType: Data]] = []
            for item in originals {
                guard item.types.count <= 32 else { return nil }
                var values: [NSPasteboard.PasteboardType: Data] = [:]
                for type in item.types {
                    guard let data = item.data(forType: type) else { return nil }
                    size += data.count
                    guard size <= 4_000_000 else { return nil }
                    values[type] = data
                }
                items.append(values)
            }
            guard board.changeCount == count else { return nil }
            return Self(changeCount: count, items: items)
        }

        func restore(_ board: NSPasteboard) {
            let restored = items.map { values in
                let item = NSPasteboardItem()
                for (type, data) in values { item.setData(data, forType: type) }
                return item
            }
            board.clearContents()
            if !restored.isEmpty { board.writeObjects(restored) }
        }
    }

    private struct ClipboardLease {
        let previous: ClipboardSnapshot
        let ownedChange: Int
    }

    private let board: NSPasteboard
    private let readTarget: @MainActor () -> PasteTargetState
    private let postPaste: @MainActor () -> Bool
    private let waitForPaste: @MainActor () async throws -> Void
    private var target: PasteTargetState?
    private var snapshot: ClipboardSnapshot?
    private var initialClipboardCount: Int?
    private var lease: ClipboardLease?
    private var deliveryID: UUID?
    private var stopped = false

    init(board: NSPasteboard = .general,
         readTarget: @escaping @MainActor () -> PasteTargetState = { PasteCoordinator.captureLiveTarget() },
         postPaste: @escaping @MainActor () -> Bool = { PasteCoordinator.postCommandV() },
         waitForPaste: @escaping @MainActor () async throws -> Void = { try await Task.sleep(for: .milliseconds(750)) }) {
        self.board = board
        self.readTarget = readTarget
        self.postPaste = postPaste
        self.waitForPaste = waitForPaste
    }

    func begin() {
        guard !stopped, deliveryID == nil else { return }
        target = readTarget()
        initialClipboardCount = board.changeCount
        snapshot = ClipboardSnapshot.capture(board)
    }

    func clear() {
        guard deliveryID == nil else { return }
        resetCapture()
    }

    /// Synchronous: applicationWillTerminate cannot wait for the asynchronous paste delay.
    func shutdown() {
        stopped = true
        restoreOwnedClipboard()
        deliveryID = nil
        resetCapture()
    }

    func deliver(_ text: String, automatic: Bool) async -> String {
        guard !stopped, deliveryID == nil else { return "投递尚未结束或应用正在退出，文字已保留。" }
        let id = UUID()
        deliveryID = id
        defer {
            // A shutdown may already have restored the clipboard and invalidated this delivery.
            if deliveryID == id {
                restoreOwnedClipboard()
                deliveryID = nil
                resetCapture()
            }
        }
        guard !Task.isCancelled else { return "投递已取消，文字已保留。" }
        let firstDecision = decision(current: readTarget(), automatic: automatic,
                                     expectedClipboardCount: initialClipboardCount)
        if case .retain(let reason) = firstDecision { return reason.message }
        guard !Task.isCancelled else { return "投递已取消，文字已保留。" }
        guard let previous = snapshot else { return DeliveryReason.clipboardUnavailable.message }
        // Capturing Accessibility metadata may take time. Recheck ownership immediately before writing.
        guard board.changeCount == previous.changeCount else { return DeliveryReason.clipboardChanged.message }
        let clearedChange = board.clearContents()
        guard board.changeCount == clearedChange else { return DeliveryReason.clipboardChanged.message }
        guard board.setString(PasteText.automatic(text), forType: .string) else {
            if board.changeCount == clearedChange { previous.restore(board) }
            return "无法写入剪贴板，文字已保留。"
        }
        let ownedChange = board.changeCount
        lease = ClipboardLease(previous: previous, ownedChange: ownedChange)

        // The app/focus or secure-input state could have changed while the pasteboard was prepared.
        let finalDecision = decision(current: readTarget(), automatic: automatic,
                                     expectedClipboardCount: ownedChange)
        if case .retain(let reason) = finalDecision { return reason.message }
        guard !Task.isCancelled else { return "投递已取消，文字已保留。" }
        guard postPaste() else { return "无法发送粘贴事件，文字已保留。" }
        do {
            try await waitForPaste()
        } catch {
            return "已发送粘贴事件，等待被中断；文字仍保留，请检查目标输入框。"
        }
        guard deliveryID == id else { return "投递已结束，剪贴板拥有权已释放。" }
        return "已向原输入框发送粘贴；请检查内容后自行发送。"
    }

    func copy(_ text: String) -> Bool {
        guard !stopped, deliveryID == nil else { return false }
        board.clearContents()
        return board.setString(text, forType: .string)
    }

    func requestAccessibility() {
        let options = [kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String: true] as CFDictionary
        _ = AXIsProcessTrustedWithOptions(options)
    }

    private func decision(current: PasteTargetState, automatic: Bool,
                          expectedClipboardCount: Int?) -> DeliveryDecision {
        let knownFocus = target?.focus != nil && current.focus != nil && target?.editable == true && current.editable
        let sameFocus: Bool
        if let original = target?.focus, let current = current.focus { sameFocus = CFEqual(original, current) }
        else { sameFocus = false }
        return DeliveryContext(
            automaticPaste: automatic, accessibility: current.accessibility,
            sameApplication: target?.applicationPID != nil && current.applicationPID == target?.applicationPID
                && target?.applicationPID != ProcessInfo.processInfo.processIdentifier,
            knownFocus: knownFocus, sameFocus: sameFocus,
            sensitiveInput: target?.sensitiveInput == true || current.sensitiveInput,
            sameClipboard: expectedClipboardCount == board.changeCount,
            restorableClipboard: snapshot != nil).decision
    }

    private func restoreOwnedClipboard() {
        guard let lease else { return }
        self.lease = nil
        if board.changeCount == lease.ownedChange { lease.previous.restore(board) }
    }

    private func resetCapture() {
        target = nil
        snapshot = nil
        initialClipboardCount = nil
    }

    private static func captureLiveTarget() -> PasteTargetState {
        let pid = NSWorkspace.shared.frontmostApplication?.processIdentifier
        let trusted = AXIsProcessTrusted()
        let focus = trusted ? pid.flatMap(Self.focusedElement) : nil
        return PasteTargetState(applicationPID: pid, focus: focus, accessibility: trusted,
                                sensitiveInput: IsSecureEventInputEnabled() || Self.isSecure(focus),
                                editable: Self.confirmsTextEntry(focus))
    }

    private static func confirmsTextEntry(_ element: AXUIElement?) -> Bool {
        guard let element else { return false }
        var role: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXRoleAttribute as CFString, &role) == .success,
              [kAXTextFieldRole, kAXTextAreaRole, kAXComboBoxRole].contains(role as? String ?? "") else { return false }
        var enabled: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXEnabledAttribute as CFString, &enabled) == .success,
              let enabled, CFGetTypeID(enabled) == CFBooleanGetTypeID(),
              CFEqual(enabled, kCFBooleanTrue) else { return false }
        // A known text role alone also describes read-only views. Require an
        // affirmative writable-value capability as well as explicit enabled=true.
        // Some editors support typing without a settable AXValue; that cannot
        // establish this capability, so they conservatively use manual copy.
        var settable: DarwinBoolean = false
        return AXUIElementIsAttributeSettable(element, kAXValueAttribute as CFString, &settable) == .success
            && settable.boolValue
    }

    private static func focusedElement(_ pid: pid_t) -> AXUIElement? {
        guard AXIsProcessTrusted() else { return nil }
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(AXUIElementCreateApplication(pid), kAXFocusedUIElementAttribute as CFString, &value) == .success,
              let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
        return (value as! AXUIElement)
    }

    private static func isSecure(_ element: AXUIElement?) -> Bool {
        guard let element else { return false }
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, kAXSubroleAttribute as CFString, &value) == .success else { return false }
        return value as? String == kAXSecureTextFieldSubrole
    }

    private static func postCommandV() -> Bool {
        let source = CGEventSource(stateID: .privateState)
        guard let down = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_ANSI_V), keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: CGKeyCode(kVK_ANSI_V), keyDown: false) else { return false }
        down.flags = .maskCommand
        up.flags = .maskCommand
        down.post(tap: .cghidEventTap)
        up.post(tap: .cghidEventTap)
        return true
    }
}
