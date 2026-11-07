import Foundation
import Darwin
import VoicePortCore

@MainActor
struct IdentityRegressionTests {
    var tests: [(String, @MainActor () async throws -> Void)] {
        [("child generations advance with gaps and UInt64 boundary", advancing),
         ("child generations reject reuse rollback noncanonical and overflow", invalid),
         ("failed generations remain consumed across manual retry", failedRetry),
         ("bad second identity retains real audio and manual retry never auto delivers", controller)]
    }
    private func bridge(_ sequence: [String], failing: Bool = false) throws -> RustBridge {
        let encoded = String(decoding: try JSONSerialization.data(withJSONObject: sequence), as: UTF8.self)
        return RustBridge(executable: URL(fileURLWithPath: ".build/vp011-host-fixture"),
                          environment: ["VP011_MODE": failing ? "retry" : "ok", "VP011_SESSION_SEQUENCE": encoded],
                          limits: .init(control: 2, job: 2, terminate: 0.3))
    }
    private let file = URL(fileURLWithPath: "/synthetic/identity.m4a")
    func advancing() async throws {
        for sequence in [["s1", "s2", "s9"], ["s18446744073709551614", "s18446744073709551615"]] {
            let b = try bridge(sequence)
            for _ in sequence { let text = try await b.transcribe(file, language: "zh", retry: false); try appRequire(text == "原始\n文字\t\0") }
            let stopped = await b.stop(); try appRequire(stopped)
        }
        let b = try bridge(["s1"])
        _ = try await b.transcribe(file, language: nil, retry: false)
        let stopped = await b.stop(); try appRequire(stopped)
        _ = try await b.transcribe(file, language: nil, retry: false) // new child may reuse s1
        _ = await b.stop()
    }
    func invalid() async throws {
        for bad in ["s1", "s0", "s01", "s2\n", "s２", "s+2", "s-2", "s2.0", "s2x", "s18446744073709551616", "s9999999999999999999999999"] {
            let b = try bridge(["s1", bad])
            _ = try await b.transcribe(file, language: nil, retry: false)
            try await rejected(b, retry: false)
        }
        for sequence in [["s8", "s7"], ["s18446744073709551615", "s18446744073709551615"]] {
            let b = try bridge(sequence)
            _ = try await b.transcribe(file, language: nil, retry: false)
            try await rejected(b, retry: false)
        }
        for bad in ["s0", "s01", "s1\n", "s١", "s+1", "s-1", "s1.0", "s18446744073709551616"] {
            let b = try bridge([bad]); try await rejected(b, retry: false)
        }
    }
    private func rejected(_ b: RustBridge, retry: Bool) async throws {
        do { _ = try await b.transcribe(file, language: "zh", retry: retry); throw AppCheckFailure(description: "invalid generation accepted") }
        catch let failure as AppCheckFailure { throw failure }
        catch let error as RustBridgeError { try appRequire(error == .protocolFailure) }
        let stopped = await b.stop(); try appRequire(stopped && b.lastChildExitStatus != nil)
    }
    func failedRetry() async throws {
        for (sequence, succeeds) in [(["s1", "s7"], true), (["s1", "s1"], false), (["s8", "s7"], false)] {
            let b = try bridge(sequence, failing: true)
            do { _ = try await b.transcribe(file, language: "zh", retry: false); throw AppCheckFailure(description: "expected failure") }
            catch let failure as AppCheckFailure { throw failure }
            catch let error as VoicePortError { try appRequire(error == .network) }
            if succeeds {
                let text = try await b.transcribe(file, language: "zh", retry: true)
                try appRequire(text == "原始\n文字\t\0"); _ = await b.stop()
            } else { try await rejected(b, retry: true) }
        }
    }
    func controller() async throws {
        guard let canonical = realpath(FileManager.default.temporaryDirectory.path, nil) else { throw AppCheckFailure(description: "test root") }
        let scope = URL(fileURLWithPath: String(cString: canonical)).appendingPathComponent("vp011-identity-\(UUID())"); free(canonical)
        try FileManager.default.createDirectory(at: scope, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: scope) }
        let root = scope.appendingPathComponent("managed")
        let storage = RecordingStorage(rootURL: root)
        let cleaner = RecordingStorage(rootURL: root, clock: { Date().addingTimeInterval(172800) })
        var files: [URL] = []
        let audio = AudioRecorder(storage: storage, makeRecorder: { file, _ in files.append(file); return IdentityDevice() })
        let delivery = IdentityDelivery()
        let suite = "VP011Identity.\(UUID())"; let defaults = UserDefaults(suiteName: suite)!
        let b = try bridge(["s1", "s1"])
        let c = DictationController(audio: audio, paste: delivery, defaults: defaults,
                                   requestMicrophonePermission: { true }, backend: b)
        c.language = "zh"
        defer { c.shutdown(); defaults.removePersistentDomain(forName: suite) }
        for recording in 0..<2 {
            c.start(); try await eventually { c.phase == .recording }; try appRequire(cleaner.cleanupExpired().active == 1)
            c.stop(); try await eventually { c.phase == .ready || c.phase == .failed }
            if recording == 0 { try appRequire(c.phase == .ready && delivery.automatic == [true]) }
        }
        try appRequire(files.count == 2 && files[0] != files[1])
        try appRequire(c.phase == .failed && c.transcript.isEmpty && c.canRetry && delivery.automatic == [true])
        try appRequire(FileManager.default.fileExists(atPath: files[1].path) && cleaner.cleanupExpired().active == 1)
        c.language = "en"; c.retry() // replacement child transcribe, native manual flag stays false
        try await eventually { c.phase == .ready || c.phase == .failed }
        try appRequire(c.phase == .ready && delivery.automatic == [true, false])
        try appRequire(!FileManager.default.fileExists(atPath: files[1].path))
        let stopped = await c.shutdownAndWait(); try appRequire(stopped)
    }
}
@MainActor private final class IdentityDevice: RecordingDevice {
    func prepareToRecord() -> Bool { true }
    func record() -> Bool { true }
    func stop() {}
}
@MainActor private final class IdentityDelivery: TextDelivery {
    var automatic: [Bool] = []
    func begin() {}
    func clear() {}
    func deliver(_ text: String, automatic: Bool) async -> String { self.automatic.append(automatic); return "synthetic delivery" }
    func copy(_ text: String) -> Bool { true }
    func requestAccessibility() {}
    func shutdown() {}
}
