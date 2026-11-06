import Foundation
import ApplicationServices

@MainActor
struct TerminalInsertionTests {
    var tests: [(String, @MainActor () async throws -> Void)] {
        [("terminal streams stable Unicode prefix without duplicates", stable),
         ("terminal recognizer revision never deletes committed input", revisions),
         ("terminal focus or user input invalidation prevents late writes", invalidate),
         ("terminal final correction cannot rewrite existing CLI input", final),
         ("terminal stop flushes last local words and end rejects callbacks", flush),
         ("terminal sanitizes controls and bounds text", controls),
         ("Chromium text field with missing subrole is supported", missingSubrole)]
    }

    func stable() async throws {
        var sent: [String] = []
        let insertion = TerminalTextInsertion(isCurrent: { true }, post: { sent.append($0); return true }, monitorUserInput: false)
        try appRequire(insertion.update("你好😀"))
        try appRequire(sent.isEmpty)
        try appRequire(insertion.update("你好😀世界"))
        try appRequire(sent == ["你好😀"])
        try appRequire(insertion.update("你好😀世界。"))
        try appRequire(insertion.update("你好😀世界。", final: true))
        try appRequire(sent.joined() == "你好😀世界。")
        try appRequire(!insertion.update("迟到"))
    }

    func revisions() async throws {
        var sent = ""
        let insertion = TerminalTextInsertion(isCurrent: { true }, post: { sent += $0; return true }, monitorUserInput: false)
        _ = insertion.update("你好")
        _ = insertion.update("您好") // uncommitted revision is harmless
        _ = insertion.update("您好世界")
        try appRequire(sent == "您好")
        try appRequire(!insertion.update("另一个开始"))
        try appRequire(sent == "您好" && !insertion.valid)
        try appRequire(!insertion.update("您好世界", final: true))
    }

    func invalidate() async throws {
        for focusChange in [false, true] {
            var current = true
            var sent = ""
            let insertion = TerminalTextInsertion(isCurrent: { current }, post: { sent += $0; return true }, monitorUserInput: false)
            _ = insertion.update("初始")
            _ = insertion.update("初始内容")
            if focusChange { current = false } else { insertion.invalidate() }
            try appRequire(!insertion.update("初始内容更多", final: true))
            current = true
            try appRequire(!insertion.flush() && sent == "初始")
        }
    }

    func final() async throws {
        var sent = ""
        let insertion = TerminalTextInsertion(isCurrent: { true }, post: { sent += $0; return true }, monitorUserInput: false)
        _ = insertion.update("已输入")
        _ = insertion.flush()
        try appRequire(!insertion.update("校正后的不同文字", final: true))
        try appRequire(sent == "已输入")
    }

    func flush() async throws {
        var sent = ""
        let insertion = TerminalTextInsertion(isCurrent: { true }, post: { sent += $0; return true }, monitorUserInput: false)
        _ = insertion.update("只有一次回调")
        try appRequire(insertion.flush() && sent == "只有一次回调")
        insertion.end()
        try appRequire(!insertion.update("尾部") && !insertion.flush())
        try appRequire(sent == "只有一次回调")
    }

    func controls() async throws {
        var sent = ""
        let insertion = TerminalTextInsertion(isCurrent: { true }, post: { sent += $0; return true }, monitorUserInput: false)
        _ = insertion.update("a\r\nb\t\u{7f}")
        _ = insertion.flush()
        try appRequire(!sent.contains(where: { $0.isNewline || $0.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) }))
        try appRequire(!insertion.update(String(repeating: "a", count: 100_001)))
    }

    func missingSubrole() async throws {
        let b = TargetAXBoundary.shared
        defer { b.reset() }
        for error: AXError in [.noValue, .attributeUnsupported] {
            b.reset()
            b.role = kAXTextFieldRole as CFString
            b.attributeErrors[kAXSubroleAttribute] = error
            b.selectedRangeSettable = true
            b.selectedTextSettable = true
            guard let target = NativeFocusedTextTarget.capture(), let session = FocusedInsertion(target: target) else {
                throw AppCheckFailure(description: "ordinary Chromium text field with absent subrole rejected")
            }
            try appRequire(session.update("可输入"))
            try appRequire(b.liveState.text == "可输入")
        }
    }
}
