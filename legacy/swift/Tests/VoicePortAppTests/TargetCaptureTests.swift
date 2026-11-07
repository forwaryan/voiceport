import AppKit
import ApplicationServices

@MainActor
struct TargetCaptureTests {
    var tests: [(String, @MainActor () async throws -> Void)] {
        [("default capture confirms enabled editable AppKit text view", editableTextView),
         ("default capture rejects initially read-only AppKit text view", readOnlyAtCapture),
         ("default capture rejects text view becoming read-only", becomesReadOnly),
         ("default capture rechecks read-only or disabled before key post", finalCapabilityChange),
         ("default capture rejects initially disabled text field", disabledField),
         ("default capture rejects unknown or malformed enabled state", unknownEnabled),
         ("default capture fails closed on AX capability query errors", queryErrors),
         ("default capture retains unsupported writable-value capability", unavailableWriteCapability),
         ("default capture still requires a known text role", unknownRole)]
    }

    func editableTextView() async throws {
        let fixture = CaptureFixture()
        defer { fixture.close() }
        fixture.useTextView(editable: true)
        fixture.paste.begin()
        _ = await fixture.paste.deliver("原文\n不要发送", automatic: true)
        try appRequire(fixture.posts == 1 && fixture.postedText == "原文 不要发送")
        try appRequire(fixture.text == CaptureFixture.original)
        try appRequire(fixture.boundary.requested.filter { $0 == kAXEnabledAttribute }.count == 3)
        try appRequire(fixture.boundary.requested.filter { $0 == "settable:" + kAXValueAttribute }.count == 3,
                       "default capture must query capabilities at capture, initial validation and final validation")
    }

    func readOnlyAtCapture() async throws {
        for becomesEditable in [false, true] {
            let fixture = CaptureFixture()
            defer { fixture.close() }
            fixture.useTextView(editable: false)
            fixture.paste.begin()
            if becomesEditable { fixture.boundary.textView?.isEditable = true }
            try await fixture.expectRetained()
        }
    }

    func becomesReadOnly() async throws {
        let fixture = CaptureFixture()
        defer { fixture.close() }
        fixture.useTextView(editable: true)
        fixture.paste.begin()
        fixture.boundary.textView?.isEditable = false
        try await fixture.expectRetained()
    }

    func finalCapabilityChange() async throws {
        for disabled in [false, true] {
            let fixture = CaptureFixture()
            defer { fixture.close() }
            fixture.useTextView(editable: true)
            fixture.paste.begin()
            fixture.boundary.onCapture = { capture in
                if capture == 3 {
                    if disabled { fixture.boundary.enabled = kCFBooleanFalse }
                    else { fixture.boundary.textView?.isEditable = false }
                }
            }
            try await fixture.expectRetained()
        }
    }

    func disabledField() async throws {
        let fixture = CaptureFixture()
        defer { fixture.close() }
        let field = NSTextField(frame: .zero)
        field.isEditable = true
        field.isEnabled = false
        // Unattached NSTextField reports AXUnknown on this host, so the role
        // is synthetic; the enabled state comes from the actual AppKit field.
        fixture.boundary.role = kAXTextFieldRole as CFString
        fixture.boundary.enabled = field.isEnabled ? kCFBooleanTrue : kCFBooleanFalse
        fixture.boundary.valueSettable = true
        fixture.paste.begin()
        try await fixture.expectRetained()
    }

    func unknownEnabled() async throws {
        // NSNumber(1) must not be accepted as a CFBoolean merely because Swift
        // can bridge some numeric values to Bool.
        let unknowns: [CFTypeRef?] = [nil, "true" as CFString, NSNumber(value: 1)]
        for enabled in unknowns {
            let fixture = CaptureFixture()
            defer { fixture.close() }
            fixture.boundary.enabled = enabled
            fixture.paste.begin()
            try await fixture.expectRetained()
        }
    }

    func queryErrors() async throws {
        for attribute in [kAXFocusedUIElementAttribute, kAXRoleAttribute, kAXEnabledAttribute, "settable:" + kAXValueAttribute] {
            for error: AXError in [.attributeUnsupported, .cannotComplete, .invalidUIElement, .noValue, .notImplemented] {
                for failureCapture in [1, 2, 3] {
                    let fixture = CaptureFixture()
                    defer { fixture.close() }
                    fixture.boundary.onCapture = { capture in
                        // Return valid-looking output even on failure, and
                        // recover on later reads: a captured failure still gates delivery.
                        if attribute.hasPrefix("settable:") {
                            fixture.boundary.settableError = capture == failureCapture ? error : .success
                        } else {
                            fixture.boundary.attributeErrors[attribute] = capture == failureCapture ? error : .success
                        }
                    }
                    fixture.paste.begin()
                    try await fixture.expectRetained()
                }
            }
        }
    }

    func unavailableWriteCapability() async throws {
        for role in [kAXTextFieldRole, kAXTextAreaRole, kAXComboBoxRole] {
            let fixture = CaptureFixture()
            defer { fixture.close() }
            fixture.boundary.role = role as CFString
            fixture.boundary.valueSettable = false
            fixture.paste.begin()
            let message = try await fixture.expectRetained()
            try appRequire(message.contains("手动复制"), "unconfirmed capability must offer manual fallback")
        }
    }

    func unknownRole() async throws {
        for role: CFTypeRef? in [nil, kAXButtonRole as CFString, NSNumber(value: 1)] {
            let fixture = CaptureFixture()
            defer { fixture.close() }
            fixture.boundary.role = role
            fixture.paste.begin()
            try await fixture.expectRetained()
        }
    }
}

@MainActor
private final class CaptureFixture {
    static let original = "能力检查前的剪贴板"
    let boundary = TargetAXBoundary.shared
    let board = NSPasteboard.withUniqueName()
    var posts = 0
    var postedText: String?
    lazy var paste = PasteCoordinator(board: board, postPaste: { [unowned self] in
        posts += 1
        postedText = board.string(forType: .string)
        return true
    }, waitForPaste: {}) // Deliberately omit readTarget: exercise the production default.
    var text: String? { board.string(forType: .string) }

    init() {
        boundary.reset()
        board.setString(Self.original, forType: .string)
    }

    func useTextView(editable: Bool) {
        let view = NSTextView(frame: .zero)
        view.isEditable = editable
        boundary.textView = view
        // Enabled is a controlled AX IPC result: an unattached NSTextView's
        // native accessibilityEnabled is false even when it is editable.
        boundary.enabled = kCFBooleanTrue
    }

    @discardableResult
    func expectRetained() async throws -> String {
        let message = await paste.deliver("应保留的原文", automatic: true)
        try appRequire(posts == 0, "unconfirmed/readonly/disabled target received \(posts) fake event(s); queries=\(boundary.requested)")
        try appRequire(text == Self.original, "failed capability check must restore or preserve the original clipboard")
        try appRequire(message.contains("手动复制"))
        return message
    }

    func close() {
        paste.shutdown()
        boundary.reset()
        board.releaseGlobally()
    }
}
