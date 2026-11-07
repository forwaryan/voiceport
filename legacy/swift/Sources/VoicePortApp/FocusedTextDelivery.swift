import AppKit
@preconcurrency import ApplicationServices
import Carbon
import VoicePortCore

@MainActor
protocol LiveTextDelivery: TextDelivery {
    var preparationFailure: InputTargetFailure? { get }
    func preview(_ text: String) -> String
    func retainPreview()
    func finishPreview()
}

extension LiveTextDelivery {
    var preparationFailure: InputTargetFailure? { nil }
}

enum InputTargetFailure: String {
    case accessibility, secureInput, application, focus, role, sensitive, disabled, writable, content, selection

    var message: String {
        switch self {
        case .accessibility:
            return "当前版本的辅助功能授权未生效，无法输入到光标处。请在系统设置 → 隐私与安全性 → 辅助功能中重新授权当前 VoicePort；开关已开时先移除旧条目，再添加当前应用。更新开发版后旧授权可能失效。"
        case .secureInput, .sensitive:
            return "当前处于安全输入或密码字段，已停止自动输入。"
        case .application:
            return "请先回到要输入文字的应用，点好光标，再按全局快捷键开始。"
        case .focus:
            return "无法读取原应用的输入焦点。请点好目标输入框后重新开始。"
        case .role, .disabled, .writable:
            return "原控件未确认支持文字输入；结果保留在面板。"
        case .content, .selection:
            return "无法读取原输入框的文字或光标位置；结果保留在面板。"
        }
    }
}

/// Text is held only for the current focused field, in memory, and never logged.
struct FocusedTextState: Equatable {
    var text: String
    var selection: NSRange
}

@MainActor
protocol FocusedTextTarget: AnyObject {
    /// nil means focus, security, capability, or readable selection is no longer known.
    func read() -> FocusedTextState?
    /// Rechecks the exact snapshot immediately before replacing the selected span.
    func replace(_ range: NSRange, with text: String, expecting: FocusedTextState) -> Bool
}

@MainActor
final class FocusedInsertion {
    private let target: any FocusedTextTarget
    private var expected: FocusedTextState
    private let original: String
    private var owned: NSRange
    private(set) var changed = false
    private(set) var valid = true
    private var finished = false

    init?(target: any FocusedTextTarget) {
        guard let state = target.read(), Self.validRange(state.selection, in: state.text) else { return nil }
        self.target = target
        expected = state
        owned = state.selection
        original = (state.text as NSString).substring(with: state.selection)
    }

    @discardableResult
    func update(_ raw: String, final: Bool = false) -> Bool {
        guard valid, !finished, target.read() == expected else { valid = false; return false }
        let text = PasteText.automatic(raw)
        guard text.utf16.count <= 100_000 else { valid = false; return false }
        let nextText = (expected.text as NSString).replacingCharacters(in: owned, with: text)
        let nextRange = NSRange(location: owned.location, length: text.utf16.count)
        let next = FocusedTextState(text: nextText, selection: NSRange(location: NSMaxRange(nextRange), length: 0))
        // Equal partials still validate focus/selection, but never rewrite the field.
        if changed && text == (expected.text as NSString).substring(with: owned) {
            finished = final
            return true
        }
        guard target.replace(owned, with: text, expecting: expected), target.read() == next else {
            valid = false
            return false
        }
        changed = true
        owned = nextRange
        expected = next
        finished = final
        return true
    }

    /// Never remove a preview after focus, selection, or user text has changed.
    func cancel() {
        guard changed, valid, !finished, target.read() == expected else { valid = false; return }
        _ = target.replace(owned, with: original, expecting: expected)
        finished = true
        valid = false
    }

    static func validRange(_ range: NSRange, in text: String) -> Bool {
        let utf16 = text.utf16
        guard range.location != NSNotFound, range.location >= 0, range.length >= 0,
              range.location <= utf16.count, range.length <= utf16.count - range.location else { return false }
        let start = utf16.index(utf16.startIndex, offsetBy: range.location)
        let end = utf16.index(start, offsetBy: range.length)
        // Range(NSRange,in:) alone accepts offsets inside surrogate pairs.
        return String.Index(start, within: text) != nil && String.Index(end, within: text) != nil
    }
}

/// Uses AXSelectedText when available, including editors whose AXValue is read-only.
/// Every write checks the original application/element, secure input and exact content.
@MainActor
final class NativeFocusedTextTarget: FocusedTextTarget {
    private let pid: pid_t
    private let element: AXUIElement
    private(set) var readFailure: InputTargetFailure?

    private init(pid: pid_t, element: AXUIElement) {
        self.pid = pid
        self.element = element
    }

    static func capture() -> NativeFocusedTextTarget? { captureWithFailure().target }

    static func captureWithFailure() -> (target: NativeFocusedTextTarget?, failure: InputTargetFailure?) {
        guard AXIsProcessTrusted() else { return (nil, .accessibility) }
        guard !IsSecureEventInputEnabled() else { return (nil, .secureInput) }
        guard let pid = NSWorkspace.shared.frontmostApplication?.processIdentifier,
              pid != ProcessInfo.processInfo.processIdentifier else { return (nil, .application) }
        if NSRunningApplication(processIdentifier: pid)?.bundleIdentifier == "com.stablyai.orca" {
            let application = AXUIElementCreateApplication(pid)
            var enabled: CFTypeRef?
            if AXUIElementCopyAttributeValue(application, "AXManualAccessibility" as CFString, &enabled) == .success,
               enabled as? Bool == false {
                var writable: DarwinBoolean = false
                if AXUIElementIsAttributeSettable(application, "AXManualAccessibility" as CFString, &writable) == .success,
                   writable.boolValue {
                    _ = AXUIElementSetAttributeValue(application, "AXManualAccessibility" as CFString, kCFBooleanTrue)
                }
            }
        }
        guard let element = focused(pid) else { return (nil, .focus) }
        let target = NativeFocusedTextTarget(pid: pid, element: element)
        guard target.read() != nil else { return (nil, target.readFailure ?? .focus) }
        return (target, nil)
    }

    func read() -> FocusedTextState? {
        func reject(_ failure: InputTargetFailure) -> FocusedTextState? {
            readFailure = failure
            return nil
        }
        guard AXIsProcessTrusted() else { return reject(.accessibility) }
        guard !IsSecureEventInputEnabled() else { return reject(.secureInput) }
        guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid,
              let current = Self.focused(pid), CFEqual(current, element) else { return reject(.focus) }
        guard let role = attribute(kAXRoleAttribute) as? String,
              [kAXTextFieldRole, kAXTextAreaRole, kAXComboBoxRole].contains(role) else { return reject(.role) }
        guard nonSensitiveSubrole(role: role) else { return reject(.sensitive) }
        guard let enabled = attribute(kAXEnabledAttribute), CFGetTypeID(enabled) == CFBooleanGetTypeID(),
              CFEqual(enabled, kCFBooleanTrue) else { return reject(.disabled) }
        guard settable(kAXSelectedTextRangeAttribute),
              (settable(kAXSelectedTextAttribute) || settable(kAXValueAttribute)) else { return reject(.writable) }
        guard let text = attribute(kAXValueAttribute) as? String,
              text.utf16.count <= 1_000_000 else { return reject(.content) }
        guard let raw = attribute(kAXSelectedTextRangeAttribute),
              CFGetTypeID(raw) == AXValueGetTypeID() else { return reject(.selection) }
        var range = CFRange()
        guard AXValueGetValue(raw as! AXValue, .cfRange, &range),
              range.location >= 0, range.length >= 0,
              range.location <= text.utf16.count, range.length <= text.utf16.count - range.location,
              FocusedInsertion.validRange(NSRange(location: range.location, length: range.length), in: text) else { return reject(.selection) }
        readFailure = nil
        return FocusedTextState(text: text, selection: NSRange(location: range.location, length: range.length))
    }

    var isOrcaTerminal: Bool {
        NSRunningApplication(processIdentifier: pid)?.bundleIdentifier == "com.stablyai.orca"
            && attribute(kAXDescriptionAttribute) as? String == "Terminal input"
    }

    /// Deliver actual input events; AXValue on xterm's textarea is not its CLI line.
    func typeTerminalText(_ raw: String) -> Bool {
        TerminalKeyEvents.post(raw, to: pid) { isOrcaTerminal && read() != nil }
    }

    func replace(_ range: NSRange, with text: String, expecting state: FocusedTextState) -> Bool {
        guard read() == state, range.location >= 0, range.length >= 0,
              range.location <= state.text.utf16.count,
              range.length <= state.text.utf16.count - range.location,
              FocusedInsertion.validRange(range, in: state.text) else { return false }
        let nextText = (state.text as NSString).replacingCharacters(in: range, with: text)
        if settable(kAXSelectedTextAttribute) {
            guard select(range) else { return false }
            // Selection change is our own, but focus and the field contents must still match.
            guard read() == FocusedTextState(text: state.text, selection: range),
                  AXUIElementSetAttributeValue(element, kAXSelectedTextAttribute as CFString, text as CFString) == .success
            else { return false }
        } else {
            guard read() == state,
                  AXUIElementSetAttributeValue(element, kAXValueAttribute as CFString, nextText as CFString) == .success
            else { return false }
        }
        // Do not move a caret if an intervening edit or focus change is observable.
        guard let after = read(), after.text == nextText else { return false }
        return select(NSRange(location: range.location + text.utf16.count, length: 0))
    }

    private func select(_ range: NSRange) -> Bool {
        var value = CFRange(location: range.location, length: range.length)
        guard let axRange = AXValueCreate(.cfRange, &value) else { return false }
        return AXUIElementSetAttributeValue(element, kAXSelectedTextRangeAttribute as CFString, axRange) == .success
    }

    private func nonSensitiveSubrole(role: String) -> Bool {
        var value: CFTypeRef?
        let result = AXUIElementCopyAttributeValue(element, kAXSubroleAttribute as CFString, &value)
        if result == .success, let subrole = value as? String {
            return subrole != kAXSecureTextFieldSubrole
        }
        // Chromium's ordinary terminal text field omits AXSubrole. An explicit
        // no-value/unsupported response is distinct from an AX transport error.
        return [kAXTextFieldRole, kAXTextAreaRole, kAXComboBoxRole].contains(role)
            && (result == .attributeUnsupported || result == .noValue)
    }

    private func settable(_ key: String) -> Bool {
        var result: DarwinBoolean = false
        return AXUIElementIsAttributeSettable(element, key as CFString, &result) == .success && result.boolValue
    }

    private func attribute(_ key: String) -> CFTypeRef? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, key as CFString, &value) == .success else { return nil }
        return value
    }

    private static func focused(_ pid: pid_t) -> AXUIElement? {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(AXUIElementCreateApplication(pid), kAXFocusedUIElementAttribute as CFString, &value) == .success,
              let value, CFGetTypeID(value) == AXUIElementGetTypeID() else { return nil }
        return (value as! AXUIElement)
    }
}

@MainActor
final class FocusedTextDelivery: LiveTextDelivery {
    private let makeTarget: (@MainActor () -> (any FocusedTextTarget)?)?
    private(set) var preparationFailure: InputTargetFailure?
    private let clipboard: any TextDelivery
    private var insertion: FocusedInsertion?
    private var terminal: TerminalTextInsertion?
    private var stopped = false
    private let retained = "原输入框、光标或文字已改变，已停止自动写入；结果保留在面板。"

    init(makeTarget: (@MainActor () -> (any FocusedTextTarget)?)? = nil,
         clipboard: any TextDelivery = PasteCoordinator()) {
        self.makeTarget = makeTarget
        self.clipboard = clipboard
    }

    func begin() {
        guard !stopped else { return }
        clear()
        preparationFailure = nil
        let target: (any FocusedTextTarget)?
        if let makeTarget {
            target = makeTarget()
            if target == nil { preparationFailure = .focus }
        } else {
            let capture = NativeFocusedTextTarget.captureWithFailure()
            target = capture.target
            preparationFailure = capture.failure
        }
        guard let target else { return }
        if let native = target as? NativeFocusedTextTarget, native.isOrcaTerminal {
            terminal = TerminalTextInsertion(isCurrent: { native.read() != nil && native.isOrcaTerminal },
                                             post: { native.typeTerminalText($0) })
        } else { insertion = FocusedInsertion(target: target) }
    }

    func preview(_ text: String) -> String {
        if !stopped, let terminal {
            return terminal.update(text)
                ? "正在向原 Orca 终端追加稳定文字；不会删除内容或自动发送。"
                : "终端焦点、输入或识别前缀已改变，已停止后续输入；结果保留在面板。"
        }
        guard !stopped, let insertion else {
            return preparationFailure?.message ?? "当前输入框不支持安全的实时写入；实时文字保留在面板。"
        }
        return insertion.update(text) ? "正在原输入框实时听写；停录后用 ChatGPT 校正。" : retained
    }

    func deliver(_ text: String, automatic: Bool) async -> String {
        guard !stopped else { return retained }
        guard automatic else {
            clear()
            return "自动输入已关闭，文字保留在面板。"
        }
        if let terminal {
            let success = terminal.update(text, final: true)
            terminal.end()
            self.terminal = nil
            return success ? "已向原 Orca 终端输入文字，请检查后自行发送。"
                : "终端保留已输入文字；无法安全改写的最终校正结果留在面板。"
        }
        guard let insertion else {
            return preparationFailure?.message ?? "无法确认原输入框支持安全写入，文字保留在面板。"
        }
        let success = insertion.update(text, final: true)
        self.insertion = nil
        return success ? "已在原输入框完成听写和最终校正，请检查后自行发送。" : retained
    }

    /// A failed cloud correction must not erase already useful local text.
    func retainPreview() {
        insertion = nil
        terminal?.end(); terminal = nil
    }

    func finishPreview() { _ = terminal?.flush() }

    func clear() {
        insertion?.cancel()
        insertion = nil
        terminal?.end(); terminal = nil
    }
    func copy(_ text: String) -> Bool { clipboard.copy(text) }
    func requestAccessibility() { clipboard.requestAccessibility() }
    func shutdown() { stopped = true; clear(); clipboard.shutdown() }
}
