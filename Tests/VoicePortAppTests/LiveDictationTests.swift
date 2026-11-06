import Foundation
import ApplicationServices
import VoicePortCore

@MainActor
struct LiveDictationTests {
    var tests: [(String, @MainActor () async throws -> Void)] {
        [("live partial revisions and final replace the same span", revisions),
         ("live UTF16 selection replacement and cancel restore original", unicode),
         ("live focus security text and selection changes permanently stop writes", invalidation),
         ("live failed writes never retry or append final elsewhere", failedWrite),
         ("live text is sanitized and bounded", bounds),
         ("live controller streams before stop and ignores stale callbacks", controller),
         ("live permission cancellation rejects late startup", pendingPreparation),
         ("local recognition fallback still records and finalizes", fallback),
         ("automatic input opt-out never streams into target", optOut),
         ("native selected-text path works with read-only AXValue", selectedTextNative),
         ("native value path preserves surrounding text", valueNative),
         ("native trust secure read-only and AX failures block updates", nativeGuards),
         ("audio write failure stops UI and prevents batch upload", audioFailure),
         ("cloud failure preserves live words and local status", cloudFailure),
         ("missing input trust blocks automatic recording before microphone permission", inputPermission),
         ("capture failures preserve the actual failing stage", captureFailure)]
    }

    private func nativeBoundary() -> TargetAXBoundary {
        let b = TargetAXBoundary.shared
        b.reset()
        b.selectedRangeSettable = true
        b.liveState = FocusedTextState(text: "前旧后", selection: NSRange(location: 1, length: 1))
        return b
    }

    func inputPermission() async throws {
        let b = TargetAXBoundary.shared
        defer { b.reset() }
        for automatic in [true, false] {
            b.reset()
            b.trusted = false
            let audio = SyntheticLiveAudio()
            let delivery = FocusedTextDelivery(clipboard: NoClipboard())
            let suite = "InputPermission.\(UUID())"
            let defaults = UserDefaults(suiteName: suite)!
            defer { defaults.removePersistentDomain(forName: suite) }
            defaults.set(automatic, forKey: "automaticPaste")
            var micRequests = 0
            let c = DictationController(audio: audio, paste: delivery, defaults: defaults,
                requestMicrophonePermission: { micRequests += 1; return true },
                transcribeAudio: { _, _ in "unused" })
            defer { c.shutdown() }
            c.checkInputPermission()
            try appRequire(automatic ? c.inputDeliveryMessage.contains("授权未生效") : c.inputDeliveryMessage.isEmpty)
            c.start()
            if automatic {
                for _ in 0..<10 { await Task.yield() }
                try appRequire(c.phase == .failed && micRequests == 0 && audio.starts == 0 && audio.partial == nil)
                try appRequire(c.message.contains("授权未生效") && c.inputDeliveryMessage == c.message)
                try appRequire(delivery.preparationFailure == .accessibility && b.captures == 0)
            } else {
                try await eventually { c.phase == .recording }
                try appRequire(micRequests == 1 && audio.starts == 1)
            }
        }
    }

    func captureFailure() async throws {
        let b = TargetAXBoundary.shared
        defer { b.reset() }
        for reason: InputTargetFailure in [.accessibility, .secureInput, .focus, .role, .sensitive, .disabled, .writable, .content, .selection] {
            b.reset()
            b.selectedRangeSettable = true
            switch reason {
            case .accessibility: b.trusted = false
            case .secureInput: b.secure = true
            case .focus: b.attributeErrors[kAXFocusedUIElementAttribute] = .cannotComplete
            case .role: b.role = kAXButtonRole as CFString
            case .sensitive: b.subrole = kAXSecureTextFieldSubrole
            case .disabled: b.enabled = kCFBooleanFalse
            case .writable: b.selectedRangeSettable = false
            case .content: b.attributeErrors[kAXValueAttribute] = .cannotComplete
            case .selection: b.liveState.selection = NSRange(location: 1, length: 0)
            case .application: break
            }
            let delivery = FocusedTextDelivery(clipboard: NoClipboard())
            delivery.begin()
            try appRequire(delivery.preparationFailure == reason)
            try appRequire(delivery.preview("不得输入") == reason.message)
            let result = await delivery.deliver("不得输入", automatic: true)
            try appRequire(result == reason.message && b.writes == 0)
        }
    }

    func selectedTextNative() async throws {
        let b = nativeBoundary()
        defer { b.reset() }
        b.valueSettable = false
        b.selectedTextSettable = true
        guard let target = NativeFocusedTextTarget.capture(), let session = FocusedInsertion(target: target) else {
            throw AppCheckFailure(description: "selected-text editor rejected")
        }
        try appRequire(session.update("你好😀"))
        try appRequire(session.update("校正", final: true))
        try appRequire(b.liveState.text == "前校正后")
    }

    func valueNative() async throws {
        let b = nativeBoundary()
        defer { b.reset() }
        guard let target = NativeFocusedTextTarget.capture(), let session = FocusedInsertion(target: target) else {
            throw AppCheckFailure(description: "value editor rejected")
        }
        try appRequire(session.update("实时"))
        session.cancel()
        try appRequire(b.liveState.text == "前旧后")
    }

    func nativeGuards() async throws {
        for failure in 0..<8 {
            let b = nativeBoundary()
            defer { b.reset() }
            guard let target = NativeFocusedTextTarget.capture(), let session = FocusedInsertion(target: target) else {
                throw AppCheckFailure(description: "initial native target")
            }
            if failure == 0 { b.trusted = false }
            if failure == 1 { b.secure = true }
            if failure == 2 { b.subrole = kAXSecureTextFieldSubrole }
            if failure == 3 { b.valueSettable = false }
            if failure == 4 { b.enabled = kCFBooleanFalse }
            if failure == 5 { b.attributeErrors[kAXFocusedUIElementAttribute] = .cannotComplete }
            if failure == 6 { b.settableError = .cannotComplete }
            if failure == 7 { b.attributeErrors[kAXSubroleAttribute] = .cannotComplete }
            try appRequire(!session.update("不能写"))
            try appRequire(b.writes == 0 && b.liveState.text == "前旧后")
        }
    }

    func revisions() async throws {
        let target = MemoryTextTarget("前缀后缀", NSRange(location: 2, length: 0))
        let session = FocusedInsertion(target: target)!
        try appRequire(session.update("你"))
        try appRequire(target.state.text == "前缀你后缀")
        try appRequire(session.update("你好"))
        try appRequire(target.state.text == "前缀你好后缀")
        let writes = target.writes
        try appRequire(session.update("你好") && target.writes == writes)
        try appRequire(session.update("您好。", final: true))
        try appRequire(target.state.text == "前缀您好。后缀")
        try appRequire(!session.update("重复", final: true))
        session.cancel()
        try appRequire(target.state.text == "前缀您好。后缀")
    }

    func unicode() async throws {
        let target = MemoryTextTarget("A👨‍👩‍👧‍👦原文Z", NSRange(location: 1, length: "👨‍👩‍👧‍👦原文".utf16.count))
        let original = target.state.text
        let session = FocusedInsertion(target: target)!
        try appRequire(session.update("中文😀"))
        try appRequire(target.state.text == "A中文😀Z" && target.state.selection.location == 5)
        try appRequire(session.update("识别修订🧑🏽‍💻"))
        session.cancel()
        try appRequire(target.state.text == original)
        let invalid = MemoryTextTarget("😀", NSRange(location: 1, length: 0))
        try appRequire(FocusedInsertion(target: invalid) == nil)
    }

    func invalidation() async throws {
        for change in 0..<4 {
            let target = MemoryTextTarget("开始", NSRange(location: 2, length: 0))
            let session = FocusedInsertion(target: target)!
            try appRequire(session.update("预览"))
            if change == 0 { target.available = false } // focus, secure or capability changed
            if change == 1 { target.state.text += "用户输入" }
            if change == 2 { target.state.selection = NSRange(location: 0, length: 0) }
            if change == 3 { target.state.text = "另一个输入框" }
            let preserved = target.state
            let writes = target.writes
            try appRequire(!session.update("最终结果", final: true))
            target.available = true
            session.cancel()
            try appRequire(target.state == preserved && target.writes == writes)
            try appRequire(!session.update("焦点恢复也不再写入"))
        }
    }

    func failedWrite() async throws {
        let target = MemoryTextTarget("", NSRange(location: 0, length: 0))
        let delivery = FocusedTextDelivery(makeTarget: { target }, clipboard: NoClipboard())
        delivery.begin()
        _ = delivery.preview("第一段")
        target.reject = true
        _ = delivery.preview("修订")
        let writes = target.writes
        target.reject = false
        let message = await delivery.deliver("最终", automatic: true)
        try appRequire(message.contains("停止") && target.writes == writes && target.state.text == "第一段")
        delivery.begin()
        _ = delivery.preview("新会话")
        delivery.clear()
        try appRequire(target.state.text == "第一段")
    }

    func bounds() async throws {
        let target = MemoryTextTarget("", NSRange(location: 0, length: 0))
        let session = FocusedInsertion(target: target)!
        try appRequire(session.update("一\n二\t三\0"))
        try appRequire(target.state.text == "一 二 三 ")
        try appRequire(!session.update(String(repeating: "a", count: 100_001)))
    }

    func controller() async throws {
        let f = LiveFixture()
        defer { f.close() }
        f.c.start()
        try await eventually { f.c.phase == .recording }
        let oldPartial = f.audio.partial!
        oldPartial("实时")
        try appRequire(f.target.state.text == "实时" && f.c.transcript == "实时")
        oldPartial("实时修订")
        f.c.stop()
        oldPartial("停录后的迟到回调")
        try await eventually { f.c.phase == .ready }
        try appRequire(f.target.state.text == "最终校正" && f.c.transcript == "最终校正")
        f.c.start()
        try await eventually { f.c.phase == .recording }
        oldPartial("旧会话")
        try appRequire(f.target.state.text == "最终校正")
        f.audio.partial?("新的预览")
        f.c.cancel()
        try appRequire(f.target.state.text == "最终校正")
        f.audio.partial?("取消后的文字")
        try appRequire(f.target.state.text == "最终校正")
    }

    func pendingPreparation() async throws {
        let f = LiveFixture()
        f.audio.hold = true
        defer { f.audio.resume(); f.close() }
        f.c.start()
        try await eventually { f.audio.pending != nil }
        f.c.cancel()
        f.audio.resume()
        for _ in 0..<10 { await Task.yield() }
        try appRequire(f.audio.starts == 0 && f.c.phase == .idle)
    }

    func fallback() async throws {
        let f = LiveFixture()
        f.audio.supported = false
        defer { f.close() }
        f.c.start()
        try await eventually { f.c.phase == .recording }
        try appRequire(f.c.message.contains("停录后") && f.audio.starts == 1)
        f.c.stop()
        try await eventually { f.c.phase == .ready }
        try appRequire(f.target.state.text == "最终校正")
    }

    func cloudFailure() async throws {
        let f = LiveFixture(transcribe: { _, _ in throw VoicePortError.network })
        defer { f.close() }
        f.c.start()
        try await eventually { f.c.phase == .recording }
        f.audio.partial?("已经实时输入的文字")
        f.audio.status?("本机识别错误码 1101")
        f.c.stop()
        try await eventually { f.c.phase == .failed }
        try appRequire(f.target.state.text == "已经实时输入的文字")
        try appRequire(f.c.transcript == f.target.state.text && f.c.canRetry)
        try appRequire(f.c.localRecognitionMessage.contains("1101"))
        f.c.shutdown()
        try appRequire(f.target.state.text == "已经实时输入的文字")
    }

    func audioFailure() async throws {
        for synchronous in [true, false] {
            let f = LiveFixture()
            defer { f.close() }
            f.c.start()
            try await eventually { f.c.phase == .recording }
            f.audio.partial?("临时预览")
            f.audio.recordingFailure = AppCheckFailure(description: "synthetic audio write failure")
            if synchronous { f.c.stop() } else { f.audio.status?("录音写入失败") }
            try appRequire(f.c.phase == .failed && !f.c.canRetry && f.target.state.text.isEmpty)
            for _ in 0..<10 { await Task.yield() }
            try appRequire(f.c.phase == .failed && f.c.canStart)
        }
    }

    func optOut() async throws {
        let f = LiveFixture()
        defer { f.close() }
        f.c.start()
        try await eventually { f.c.phase == .recording }
        f.audio.partial?("临时文字")
        try appRequire(f.target.state.text == "临时文字")
        f.c.automaticPaste = false
        try appRequire(f.target.state.text.isEmpty)
        f.audio.partial?("只在面板")
        try appRequire(f.target.state.text.isEmpty && f.c.transcript == "只在面板")
        f.c.stop()
        try await eventually { f.c.phase == .ready }
        try appRequire(f.target.state.text.isEmpty && f.c.transcript == "最终校正")
    }
}

@MainActor
private final class MemoryTextTarget: FocusedTextTarget {
    var state: FocusedTextState
    var available = true
    var reject = false
    var writes = 0
    init(_ text: String, _ selection: NSRange) { state = .init(text: text, selection: selection) }
    func read() -> FocusedTextState? { available ? state : nil }
    func replace(_ range: NSRange, with text: String, expecting expected: FocusedTextState) -> Bool {
        guard !reject, read() == expected else { return false }
        state.text = (state.text as NSString).replacingCharacters(in: range, with: text)
        state.selection = NSRange(location: range.location + text.utf16.count, length: 0)
        writes += 1
        return true
    }
}
@MainActor
private final class NoClipboard: TextDelivery {
    func begin() {}
    func clear() {}
    func deliver(_ text: String, automatic: Bool) async -> String { "unused" }
    func copy(_ text: String) -> Bool { true }
    func requestAccessibility() {}
    func shutdown() {}
}
@MainActor
private final class SyntheticLiveAudio: LiveAudioRecording {
    var partial: (@MainActor (String) -> Void)?
    var status: (@MainActor (String) -> Void)?
    var recordingFailure: (any Error)?
    var supported = true
    var starts = 0
    var hold = false
    var pending: CheckedContinuation<Void, Never>?
    func prepareLive(language: String?, onPartial: @escaping @MainActor (String) -> Void,
                     onStatus: @escaping @MainActor (String) -> Void) async -> Bool {
        partial = onPartial
        status = onStatus
        if hold { await withCheckedContinuation { pending = $0 } }
        if !supported { onStatus("本机实时识别不可用。") }
        return supported
    }
    func resume() { pending?.resume(); pending = nil }
    func start() throws -> URL { starts += 1; return URL(fileURLWithPath: "/synthetic/live.m4a") }
    func stop() {}
    func cancel() {}
}
@MainActor
private final class LiveFixture {
    let target = MemoryTextTarget("", NSRange(location: 0, length: 0))
    let audio = SyntheticLiveAudio()
    let suite = "LiveDictationTests.\(UUID())"
    let defaults: UserDefaults
    let c: DictationController
    init(transcribe: @escaping @Sendable (URL, String?) async throws -> String = { _, _ in "最终校正" }) {
        defaults = UserDefaults(suiteName: suite)!
        c = DictationController(audio: audio,
            paste: FocusedTextDelivery(makeTarget: { [target] in target }, clipboard: NoClipboard()),
            defaults: defaults, requestMicrophonePermission: { true },
            transcribeAudio: transcribe)
    }
    func close() { c.shutdown(); defaults.removePersistentDomain(forName: suite) }
}
