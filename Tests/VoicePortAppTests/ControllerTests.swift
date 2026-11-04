import Foundation
import VoicePortCore

@MainActor
struct ControllerTests {
    var tests: [(String, @MainActor () async throws -> Void)] {
        [("record-stop-result lifecycle and duplicate stop", lifecycle),
         ("denied microphone never records", deniedPermission),
         ("cancel pending permission rejects late grant", cancelPermission),
         ("recording failure does not upload", recordingFailure),
         ("upload blocks another recording", busyUpload),
         ("180 ticks stop recording exactly once", durationLimit),
         ("late cancelled result cannot reach next session", lateResult),
         ("retry preserves audio and never auto-pastes", retry),
         ("service-side cancellation becomes retryable failure", unexpectedCancellation),
         ("delivery lease blocks cancellation and new recording", deliveryLease),
         ("shutdown rejects late upload result", shutdown),
         ("shutdown rejects late delivery status", shutdownDuringDelivery),
         ("settings use isolated preferences", isolatedSettings)]
    }

    func lifecycle() async throws {
        let fixture = Fixture()
        defer { fixture.close() }
        fixture.controller.start()
        try await eventually { fixture.controller.phase == .recording }
        try appRequire(fixture.audio.starts == 1 && fixture.paste.deliveries.isEmpty)
        fixture.controller.stop()
        fixture.controller.stop()
        try await eventually { fixture.controller.phase == .ready }
        try appRequire(fixture.controller.transcript == "保留 requestID\n不要发送")
        try appRequire(fixture.paste.deliveries.count == 1 && fixture.paste.deliveries[0].automatic)
        try appRequire(!fixture.controller.canRetry && fixture.controller.canStart)
        fixture.controller.copyResult()
        try appRequire(fixture.paste.copied == "保留 requestID\n不要发送")
    }

    func deniedPermission() async throws {
        let fixture = Fixture(permission: { false })
        defer { fixture.close() }
        fixture.controller.start()
        try await eventually { fixture.controller.phase == .failed }
        try appRequire(fixture.audio.starts == 0 && !fixture.controller.canRetry)
        try appRequire(fixture.paste.deliveries.isEmpty)
    }

    func cancelPermission() async throws {
        let permission = PendingPermission()
        let fixture = Fixture(permission: { await permission.wait() })
        defer { fixture.close(); permission.complete(true) }
        fixture.controller.start()
        try await eventually { permission.pending }
        fixture.controller.cancel()
        permission.complete(true)
        // Let the resumed old operation run before asserting it did not start the microphone.
        for _ in 0..<10 { await Task.yield() }
        try appRequire(fixture.controller.phase == .idle && fixture.audio.starts == 0)
    }

    func recordingFailure() async throws {
        let fixture = Fixture()
        fixture.audio.shouldFail = true
        defer { fixture.close() }
        fixture.controller.start()
        try await eventually { fixture.controller.phase == .failed }
        try appRequire(!fixture.controller.canRetry && fixture.paste.deliveries.isEmpty)
    }

    func busyUpload() async throws {
        let pending = PendingTranscription()
        let fixture = Fixture(transcribe: { file, language in try await pending.wait(file, language) })
        defer { fixture.close(); pending.failAll() }
        fixture.controller.start()
        try await eventually { fixture.controller.phase == .recording }
        fixture.controller.stop()
        try await eventually { pending.count == 1 }
        fixture.controller.start()
        fixture.controller.toggle()
        try appRequire(fixture.audio.starts == 1 && !fixture.controller.canStart)
        pending.complete(0, text: "完成")
        try await eventually { fixture.controller.phase == .ready }
    }

    func durationLimit() async throws {
        let pending = PendingTranscription()
        let fixture = Fixture(transcribe: { file, language in try await pending.wait(file, language) },
                              tick: { await Task.yield() })
        defer { fixture.close(); pending.failAll() }
        fixture.controller.start()
        try await eventually { pending.count == 1 }
        try appRequire(fixture.controller.seconds == 180)
        try appRequire(fixture.controller.phase == .transcribing && fixture.audio.stops == 1)
        pending.complete(0, text: "到达时长上限")
        try await eventually { fixture.controller.phase == .ready }
    }

    func lateResult() async throws {
        let pending = PendingTranscription()
        let fixture = Fixture(transcribe: { file, language in try await pending.wait(file, language) })
        defer { fixture.close(); pending.failAll() }
        fixture.controller.start()
        try await eventually { fixture.controller.phase == .recording }
        fixture.controller.stop()
        try await eventually { pending.count == 1 }
        fixture.controller.cancel()
        fixture.controller.start()
        try await eventually { fixture.controller.phase == .recording }
        fixture.controller.stop()
        try await eventually { pending.count == 2 }
        pending.complete(0, text: "已取消的旧文字")
        for _ in 0..<10 { await Task.yield() }
        try appRequire(fixture.controller.phase == .transcribing && fixture.paste.deliveries.isEmpty)
        pending.complete(1, text: "新会话文字")
        try await eventually { fixture.controller.phase == .ready }
        try appRequire(fixture.controller.transcript == "新会话文字")
        try appRequire(fixture.paste.deliveries.map(\.text) == ["新会话文字"])
    }

    func retry() async throws {
        let pending = PendingTranscription()
        let fixture = Fixture(transcribe: { file, language in try await pending.wait(file, language) })
        defer { fixture.close(); pending.failAll() }
        fixture.controller.language = "zh"
        fixture.controller.start()
        try await eventually { fixture.controller.phase == .recording }
        fixture.controller.stop()
        try await eventually { pending.count == 1 }
        pending.fail(0, error: VoicePortError.service(401))
        try await eventually { fixture.controller.phase == .failed }
        try appRequire(fixture.controller.canRetry)
        fixture.controller.retry()
        try await eventually { pending.count == 2 }
        try appRequire(pending.files[0] == pending.files[1])
        try appRequire(pending.languages == ["zh", "zh"])
        pending.complete(1, text: "手动重试结果")
        try await eventually { fixture.controller.phase == .ready }
        try appRequire(fixture.paste.deliveries.count == 1 && !fixture.paste.deliveries[0].automatic)
    }

    func unexpectedCancellation() async throws {
        let fixture = Fixture(transcribe: { _, _ in throw CancellationError() })
        defer { fixture.close() }
        fixture.controller.start()
        try await eventually { fixture.controller.phase == .recording }
        fixture.controller.stop()
        try await eventually { fixture.controller.phase == .failed }
        try appRequire(fixture.controller.canRetry && fixture.controller.canStart)
    }

    func deliveryLease() async throws {
        let fixture = Fixture()
        fixture.paste.holdDelivery = true
        defer { fixture.close(); fixture.paste.releaseDelivery() }
        fixture.controller.start()
        try await eventually { fixture.controller.phase == .recording }
        fixture.controller.stop()
        try await eventually { fixture.controller.phase == .delivering }
        fixture.controller.start()
        fixture.controller.cancel()
        try appRequire(fixture.controller.phase == .delivering && fixture.audio.starts == 1)
        fixture.paste.releaseDelivery()
        try await eventually { fixture.controller.phase == .ready }
    }

    func shutdown() async throws {
        let pending = PendingTranscription()
        let fixture = Fixture(transcribe: { file, language in try await pending.wait(file, language) })
        defer { fixture.close(); pending.failAll() }
        fixture.controller.start()
        try await eventually { fixture.controller.phase == .recording }
        fixture.controller.stop()
        try await eventually { pending.count == 1 }
        fixture.controller.shutdown()
        try appRequire(fixture.paste.shutdowns == 1, "shutdown must notify the delivery owner before the app exits")
        pending.complete(0, text: "退出后的旧结果")
        for _ in 0..<10 { await Task.yield() }
        try appRequire(fixture.controller.transcript.isEmpty && fixture.paste.deliveries.isEmpty)
    }

    func isolatedSettings() async throws {
        let fixture = Fixture()
        defer { fixture.close() }
        fixture.controller.automaticPaste = false
        fixture.controller.language = "en"
        fixture.controller.alternativeShortcut = true
        try appRequire(fixture.defaults.object(forKey: "automaticPaste") as? Bool == false)
        try appRequire(fixture.defaults.string(forKey: "language") == "en")
        try appRequire(fixture.defaults.bool(forKey: "alternativeShortcut"))
    }

    func shutdownDuringDelivery() async throws {
        let fixture = Fixture()
        fixture.paste.holdDelivery = true
        defer { fixture.close(); fixture.paste.releaseDelivery() }
        fixture.controller.start()
        try await eventually { fixture.controller.phase == .recording }
        fixture.controller.stop()
        try await eventually { fixture.controller.phase == .delivering && fixture.paste.waiting }
        let message = fixture.controller.message
        fixture.controller.shutdown()
        fixture.paste.releaseDelivery()
        try await eventually { fixture.paste.completedDeliveries == 1 }
        try appRequire(fixture.controller.phase == .delivering && fixture.controller.message == message,
                       "a delivery resumed after shutdown must not publish a late result")
    }
}

@MainActor
private final class Fixture {
    let audio = FakeAudio()
    let paste = FakePaste()
    let defaults: UserDefaults
    let controller: DictationController
    private let suite = "VoicePortChecks.\(UUID().uuidString)"

    init(permission: @escaping @MainActor () async -> Bool = { true },
         transcribe: @escaping @Sendable (URL, String?) async throws -> String = { _, _ in "保留 requestID\n不要发送" },
         tick: @escaping @Sendable () async throws -> Void = { try await Task.sleep(for: .seconds(60)) }) {
        defaults = UserDefaults(suiteName: suite)!
        controller = DictationController(audio: audio, paste: paste, defaults: defaults,
                                         requestMicrophonePermission: permission,
                                         transcribeAudio: transcribe, waitForTick: tick)
    }
    func close() {
        controller.shutdown()
        defaults.removePersistentDomain(forName: suite)
    }
}

@MainActor
private final class FakeAudio: AudioRecording {
    var starts = 0
    var stops = 0
    var shouldFail = false
    func start() throws -> URL {
        starts += 1
        if shouldFail { throw AudioRecorder.RecordingError.startFailed }
        return URL(fileURLWithPath: "/voiceport-offline-fixture/recording-\(starts).m4a")
    }
    func stop() { stops += 1 }
    func cancel() {}
}

@MainActor
private final class FakePaste: TextDelivery {
    struct Delivery { let text: String; let automatic: Bool }
    var deliveries: [Delivery] = []
    var copied = ""
    var holdDelivery = false
    var shutdowns = 0
    var completedDeliveries = 0
    private var continuation: CheckedContinuation<Void, Never>?
    var waiting: Bool { continuation != nil }
    func begin() {}
    func clear() {}
    func requestAccessibility() {}
    func shutdown() { shutdowns += 1 }
    func deliver(_ text: String, automatic: Bool) async -> String {
        deliveries.append(Delivery(text: text, automatic: automatic))
        if holdDelivery { await withCheckedContinuation { continuation = $0 } }
        completedDeliveries += 1
        return "fake delivery; no clipboard or keyboard I/O"
    }
    func releaseDelivery() { continuation?.resume(); continuation = nil }
    func copy(_ text: String) -> Bool { copied = text; return true }
}

@MainActor
private final class PendingPermission {
    private var continuation: CheckedContinuation<Bool, Never>?
    var pending: Bool { continuation != nil }
    func wait() async -> Bool { await withCheckedContinuation { continuation = $0 } }
    func complete(_ result: Bool) { continuation?.resume(returning: result); continuation = nil }
}

@MainActor
private final class PendingTranscription {
    private var continuations: [CheckedContinuation<String, Error>?] = []
    private(set) var files: [URL] = []
    private(set) var languages: [String?] = []
    var count: Int { files.count }
    func wait(_ file: URL, _ language: String?) async throws -> String {
        files.append(file)
        languages.append(language)
        return try await withCheckedThrowingContinuation { continuations.append($0) }
    }
    func complete(_ index: Int, text: String) {
        continuations[index]?.resume(returning: text)
        continuations[index] = nil
    }
    func fail(_ index: Int, error: Error) {
        continuations[index]?.resume(throwing: error)
        continuations[index] = nil
    }
    func failAll() {
        for index in continuations.indices { fail(index, error: CancellationError()) }
    }
}
