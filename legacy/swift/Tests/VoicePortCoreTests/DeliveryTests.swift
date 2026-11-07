import Foundation

struct DeliveryTests {
    var tests: [(String, () throws -> Void)] {
        [("matching target allowed", allowedTarget), ("switched app retains text", switchedApp),
         ("different input retains text", switchedFocus), ("unknown focus retains text", unknownFocus),
         ("changed clipboard preserved", changedClipboard), ("secure input blocked", sensitiveInput),
         ("permission and snapshot required", prerequisites), ("cancelled session rejects late results", cancellation),
         ("automatic paste strips control characters", safeText)]
    }
    private var allowed: DeliveryContext {
        DeliveryContext(automaticPaste: true, accessibility: true, sameApplication: true,
                        knownFocus: true, sameFocus: true, sensitiveInput: false,
                        sameClipboard: true, restorableClipboard: true)
    }
    func allowedTarget() throws { try equal(allowed.decision, .paste) }
    func switchedApp() throws {
        var context = allowed; context.sameApplication = false
        try equal(context.decision, .retain(.targetChanged))
    }
    func switchedFocus() throws {
        var context = allowed; context.sameFocus = false
        try equal(context.decision, .retain(.focusChanged))
    }
    func unknownFocus() throws {
        var context = allowed; context.knownFocus = false
        try equal(context.decision, .retain(.unknownFocus))
    }
    func changedClipboard() throws {
        var context = allowed; context.sameClipboard = false
        try equal(context.decision, .retain(.clipboardChanged))
    }
    func sensitiveInput() throws {
        var context = allowed; context.sensitiveInput = true
        try equal(context.decision, .retain(.sensitiveInput))
    }
    func prerequisites() throws {
        var context = allowed; context.accessibility = false
        try equal(context.decision, .retain(.accessibilityMissing))
        context = allowed; context.restorableClipboard = false
        try equal(context.decision, .retain(.clipboardUnavailable))
        context = allowed; context.automaticPaste = false
        try equal(context.decision, .retain(.automaticPasteOff))
    }
    func cancellation() throws {
        var gate = SessionGate()
        let first = gate.begin()
        try require(gate.accepts(first))
        gate.cancel()
        try require(!gate.accepts(first))
        let second = gate.begin()
        try require(!gate.accepts(first))
        try require(gate.accepts(second))
    }
    func safeText() throws {
        try equal(PasteText.automatic("请保留 requestID\r\n不要发送\n"), "请保留 requestID 不要发送 ")
        try equal(PasteText.automatic("echo\u{1b}[31m\ttext\0"), "echo [31m text ")
        try equal(PasteText.automatic("中文 mixedCase，数字 123。"), "中文 mixedCase，数字 123。")
    }
}
