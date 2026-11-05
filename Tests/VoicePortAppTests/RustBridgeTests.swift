import Foundation
import Darwin
import VoicePortCore
@MainActor struct RustBridgeTests {
    var tests: [(String,@MainActor () async throws -> Void)] { IdentityRegressionTests().tests + [("Rust bridge budget exhausts without replay and restarts", budget), ("default controller uses lazy Rust backend", defaultController), ("native cancel keeps audio until child stops", cleanupOrder), ("Rust bridge split UTF8 and original text", original), ("Rust bridge failed session uses wire retry", retry), ("Rust bridge rejects tails frames identities and timeouts", failures), ("Rust bridge cancellation reaps child before completion", cancel), ("Rust bridge real Rust offline errors and retry", realRust), ("Rust bridge strict parser rejects duplicate and mistyped fields", parser)] }
    var realExecutable: URL {
        let packaged = URL(fileURLWithPath:"dist/VoicePort.app/Contents/Helpers/voiceport-rs")
        return FileManager.default.isExecutableFile(atPath:packaged.path) ? packaged : URL(fileURLWithPath:".build/vp011-real-helper")
    }
    func bridge(_ mode:String) -> RustBridge {
        RustBridge(executable: URL(fileURLWithPath: ".build/vp011-host-fixture"),environment: ["VP011_MODE":mode],limits:.init(control: ["ok","retry"].contains(mode) ? 2 : 0.3,job: ["ok","retry"].contains(mode) ? 2 : 0.3,terminate:0.1))
    }
    func original() async throws {
        let b=bridge("ok");let text=try await b.transcribe(URL(fileURLWithPath:"/synthetic/audio"),language:nil,retry:false)
        try appRequire(text == "原始\n文字\t\0");let stopped=await b.stop();try appRequire(stopped);print("OBS bridge child exit=\(String(describing: b.lastChildExitStatus))")
        let restarted = try await b.transcribe(URL(fileURLWithPath:"/synthetic/audio"), language:nil,retry:false)
        try appRequire(restarted == text);_ = await b.stop()
    }
    func retry() async throws {
        let b=bridge("retry");do {_ = try await b.transcribe(URL(fileURLWithPath:"/synthetic/audio"),language:"zh",retry:false);throw AppCheckFailure(description:"expected failure")} catch let e as AppCheckFailure {throw e} catch {}
        let text=try await b.transcribe(URL(fileURLWithPath:"/synthetic/audio"),language:"zh",retry:true);try appRequire(text == "原始\n文字\t\0");_ = await b.stop()
    }
    func failures() async throws {
        for mode in ["tail","large","bad","no-ready","slow","stubborn","no-read","exit"] {
            let b=bridge(mode)
            do {_ = try await b.transcribe(URL(fileURLWithPath:"/synthetic/audio"),language:nil,retry:false);throw AppCheckFailure(description:"accepted \(mode)")} catch let e as AppCheckFailure {throw e} catch {}
            let stopped=await b.stop();try appRequire(stopped);print("OBS bridge child exit=\(String(describing: b.lastChildExitStatus))")
        }
    }
    func cancel() async throws {
        let b=bridge("slow");let task=Task {try await b.transcribe(URL(fileURLWithPath:"/synthetic/audio"),language:nil,retry:false)}
        try await Task.sleep(for:.milliseconds(70));task.cancel();do {_ = try await task.value;throw AppCheckFailure(description:"cancellation ignored")} catch is CancellationError {};
        let stopped=await b.stop();try appRequire(stopped);print("OBS bridge child exit=\(String(describing: b.lastChildExitStatus))")
    }
    func realRust() async throws {
        let root=FileManager.default.temporaryDirectory.appendingPathComponent("vp011-\(UUID())")
        try FileManager.default.createDirectory(at:root,withIntermediateDirectories:false);defer {try? FileManager.default.removeItem(at:root)}
        let b=RustBridge(executable:realExecutable,environment:["CODEX_HOME":root.path],limits:.init(control:2,job:2,terminate:0.2))
        let initial = try await b.status(); try appRequire(initial == "idle")
        for retry in [false,true] {
            do {_ = try await b.transcribe(root.appendingPathComponent("audio.m4a"),language:nil,retry:retry);throw AppCheckFailure(description:"missing auth accepted")} catch let e as VoicePortError {try appRequire(e == .signInRequired)}
        }
        let stopped=await b.stop();try appRequire(stopped);print("OBS bridge child exit=\(String(describing: b.lastChildExitStatus))")
    }
    func defaultController() async throws {
        let suite = "vp011-default-\(UUID())"; let defaults = UserDefaults(suiteName: suite)!
        let c = DictationController(audio: BridgeAudio(), paste: BridgePaste(), defaults: defaults, requestMicrophonePermission: { true })
        defer { c.shutdown(); defaults.removePersistentDomain(forName: suite) }
        c.start(); try await eventually { c.phase == .recording }; c.stop()
        try await eventually { c.phase == .failed }
        try appRequire(c.canRetry && c.message.contains("Rust"))
    }
    func cleanupOrder() async throws {
        let suite = "vp011-order-\(UUID())"; let defaults = UserDefaults(suiteName: suite)!
        let temporary = FileManager.default.temporaryDirectory
        guard let canonical = realpath(temporary.path, nil) else { throw AppCheckFailure(description: "temp path") }
        let top = URL(fileURLWithPath: String(cString: canonical)).appendingPathComponent("vp011-lease-\(UUID())"); free(canonical)
        try FileManager.default.createDirectory(at: top, withIntermediateDirectories: false)
        defer { try? FileManager.default.removeItem(at: top) }
        let root = top.appendingPathComponent("managed")
        let storage = RecordingStorage(rootURL: root)
        let cleaner = RecordingStorage(rootURL: root, clock: { Date().addingTimeInterval(172800) })
        let audio = AudioRecorder(storage: storage, makeRecorder: { _, _ in BridgeDevice() }); let b = bridge("slow")
        let c = DictationController(audio: audio, paste: BridgePaste(), defaults: defaults, requestMicrophonePermission: { true }, backend: b)
        defer { c.shutdown(); defaults.removePersistentDomain(forName: suite) }
        c.start(); try await eventually { c.phase == .recording }; c.stop()
        try await Task.sleep(for: .milliseconds(100))
        try appRequire(cleaner.cleanupExpired().active == 1)
        c.cancel(); try appRequire(!c.canStart && cleaner.cleanupExpired().active == 1)
        try await eventually { c.phase == .idle }
        let names = try FileManager.default.contentsOfDirectory(atPath: root.path)
        try appRequire(Set(names) == ["owner.json"])

    }
    func budget() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("vp011-budget-\(UUID())")
        try FileManager.default.createDirectory(at:root,withIntermediateDirectories:false);defer {try? FileManager.default.removeItem(at:root)}
        let b = RustBridge(executable:realExecutable,environment:["CODEX_HOME":root.path],limits:.init(control:2,job:2,terminate:0.2))
        for _ in 0..<1024 { let state = try await b.status(); try appRequire(state == "idle") }
        do { _ = try await b.status(); throw AppCheckFailure(description:"budget overflow accepted") } catch let e as AppCheckFailure { throw e } catch {}
        let restarted = try await b.status(); try appRequire(restarted == "idle"); _ = await b.stop()
    }
    func parser() async throws {
        for text in ["{\"version\":1,\"event\":\"ready\",\"event\":\"result\",\"state\":\"idle\"}","{\"version\":true,\"event\":\"ready\",\"state\":\"idle\"}","{\"version\":1,\"event\":\"status\",\"state\":\"idle\",\"text\":\"private\"}"] {
            do {_ = try RustBridge.decode(Data(text.utf8));throw AppCheckFailure(description:"invalid frame accepted")}catch let e as AppCheckFailure{throw e}catch{}
        }
    }
}

@MainActor private final class BridgeAudio: AudioRecording {
    var cancels = 0
    func start() throws -> URL { URL(fileURLWithPath: "/synthetic/audio.m4a") }
    func stop() {}
    func cancel() { cancels += 1 }
}
@MainActor private final class BridgePaste: TextDelivery {
    func begin() {}
    func clear() {}
    func deliver(_ text: String, automatic: Bool) async -> String { "offline" }
    func copy(_ text: String) -> Bool { true }
    func requestAccessibility() {}
    func shutdown() {}
}

@MainActor private final class BridgeDevice: RecordingDevice {
    func prepareToRecord() -> Bool { true }
    func record() -> Bool { true }
    func stop() {}
}
