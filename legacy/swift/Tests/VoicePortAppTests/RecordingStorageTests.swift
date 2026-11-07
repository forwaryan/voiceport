import Darwin
import Foundation

@MainActor
struct RecordingStorageTests {
    var tests: [(String, @MainActor () async throws -> Void)] {
        [("recording store initialization and smoke launch perform no storage I/O", inertInitialization),
         ("recording sessions have private modes and bound versioned markers", privateSessions),
         ("only expired confirmed unlocked recording sessions are removed", expiredSession),
         ("recording retention is strictly greater than 24 hours", retentionBoundary),
         ("recording lease excludes a separate cleanup process until release", crossProcessLease),
         ("process exit without destructors releases recording lease", crashRelease),
         ("simultaneous cleanup processes serialize session deletion", concurrentCleanup),
         ("stopped recorder retains audio and cross-process lease until cancel", stoppedRecorder),
         ("upload and retry retain recording lease until success cancel or shutdown", controllerRetention),
         ("recorder factory prepare record and output failures remove resources", failedStart),
         ("unsafe recording roots are preserved and reject creation", unsafeRoots),
         ("unknown malformed mismatched and mixed recording sessions are preserved", unknownSessions),
         ("recording cleanup never follows symbolic or hard links", links),
         ("recording cleanup retains future filesystem timestamps", futureMetadata),
         ("recording cleanup leaves unmarked legacy namespaces untouched", legacyAndUnknown),
         ("startup recording cleanup is best effort and smoke skips its factory", launchCleanup)]
    }

    // Child modes are test-only, in this check executable. All callers pass a
    // newly created test scope, never the real per-user store or temp root.
    static func childExitCode(arguments: [String]) -> Int32? {
        guard arguments.count > 1, arguments[1] == "--vp006-storage-child" else { return nil }
        guard arguments.count == 5, let time = Double(arguments[4]), time.isFinite else { return 2 }
        let root = URL(fileURLWithPath: arguments[3], isDirectory: true)
        guard root.lastPathComponent == "managed",
              root.deletingLastPathComponent().lastPathComponent.hasPrefix("VoicePort-VP006-Checks-"),
              (try? String(contentsOf: root.deletingLastPathComponent().appendingPathComponent("test-scope"), encoding: .utf8)) == "VP006 isolated checks" else { return 2 }
        let storage = RecordingStorage(rootURL: root, clock: { Date(timeIntervalSince1970: time) })
        do {
            if arguments[2] == "crash" {
                let session = try storage.createSession()
                let bytes = Array((session.audioURL.deletingLastPathComponent().path + "\n").utf8)
                bytes.withUnsafeBytes { _ = write(STDOUT_FILENO, $0.baseAddress!, $0.count) }
                // No Session deinit/discard, no AVFoundation: release is by OS exit.
                withExtendedLifetime(session) { _exit(0) }
            }
            guard arguments[2] == "cleanup" else { return 2 }
            let data = try JSONEncoder().encode(storage.cleanupExpired())
            print(String(decoding: data, as: UTF8.self))
            return 0
        } catch { print("storage child failed"); return 1 }
    }

    func inertInitialization() async throws {
        let f = try StoreFixture(); defer { f.close() }
        let absent = f.top.appendingPathComponent("absent-parent/managed")
        let storage = RecordingStorage(rootURL: absent)
        _ = AudioRecorder(storage: storage)
        try storageRequire(!FileManager.default.fileExists(atPath: absent.deletingLastPathComponent().path))
        try storageRequire(storage.cleanupExpired().rootUnavailable)
        try storageRequire(!FileManager.default.fileExists(atPath: absent.deletingLastPathComponent().path))
    }

    func privateSessions() async throws {
        let f = try StoreFixture(); defer { f.close() }
        let first = try f.store.createSession(); defer { first.discard() }
        let second = try f.store.createSession(); defer { second.discard() }
        try storageRequire(first.audioURL != second.audioURL)
        let directory = first.audioURL.deletingLastPathComponent()
        for url in [f.root, directory] { try storageRequire(try mode(url) == 0o700) }
        for url in [f.root.appendingPathComponent("owner.json"), directory.appendingPathComponent("session.json"),
                    directory.appendingPathComponent("lease"), first.audioURL] { try storageRequire(try mode(url) == 0o600) }
        let owner = try marker(f.root.appendingPathComponent("owner.json"))
        let session = try marker(directory.appendingPathComponent("session.json"))
        try storageRequire(owner["application"] as? String == "io.voiceport.recording-storage" && owner["version"] as? Int == 1)
        try storageRequire(owner["ownerUID"] as? UInt32 == geteuid())
        try storageRequire(session["rootID"] as? String == owner["rootID"] as? String)
        try storageRequire(session["sessionID"] as? String == directory.lastPathComponent)
    }

    func expiredSession() async throws {
        let f = try StoreFixture(); defer { f.close() }
        let directory = try f.abandoned(age: 2 * RecordingStorage.retention)
        let report = f.clean()
        try storageRequire(report.removed == 1 && report.failed == 0 && !f.exists(directory))
        try storageRequire(f.exists(f.root.appendingPathComponent("owner.json")))
    }

    func retentionBoundary() async throws {
        for age in [RecordingStorage.retention - 1, RecordingStorage.retention, RecordingStorage.retention + 0.01, -1] {
            let f = try StoreFixture(); defer { f.close() }
            let directory = try f.abandoned(age: age)
            let report = f.clean()
            try storageRequire(report.removed == (age > RecordingStorage.retention ? 1 : 0))
            try storageRequire(f.exists(directory) == (age <= RecordingStorage.retention))
        }
    }

    func crossProcessLease() async throws {
        let f = try StoreFixture(); defer { f.close() }
        var session: RecordingStorage.Session? = try f.oldStore.createSession()
        let directory = session!.audioURL.deletingLastPathComponent()
        let held = try f.childCleanup()
        try storageRequire(held.active == 1 && held.removed == 0 && f.exists(directory))
        session = nil // Release handle without discard, as a disappeared owner would.
        let released = try f.childCleanup()
        try storageRequire(released.removed == 1 && !f.exists(directory))
    }

    func crashRelease() async throws {
        let f = try StoreFixture(); defer { f.close() }
        let child = try f.startChild("crash", time: f.now.timeIntervalSince1970 - 2 * RecordingStorage.retention)
        let output = try f.finishChild(child)
        let directory = URL(fileURLWithPath: output.trimmingCharacters(in: .whitespacesAndNewlines))
        try storageRequire(directory.deletingLastPathComponent().path == f.root.path && f.exists(directory))
        let result = try f.childCleanup()
        try storageRequire(result.removed == 1 && !f.exists(directory), "a real _exit must release flock without any Swift cleanup")
    }

    func concurrentCleanup() async throws {
        let f = try StoreFixture(); defer { f.close() }
        var directories: [URL] = []
        for _ in 0..<12 { directories.append(try f.abandoned(age: 2 * RecordingStorage.retention)) }
        let first = try f.startChild("cleanup")
        let second = try f.startChild("cleanup")
        let a = try JSONDecoder().decode(RecordingStorage.CleanupReport.self, from: Data(f.finishChild(first).utf8))
        let b = try JSONDecoder().decode(RecordingStorage.CleanupReport.self, from: Data(f.finishChild(second).utf8))
        try storageRequire(a.removed + b.removed == 12 && directories.allSatisfy { !f.exists($0) })
    }

    func stoppedRecorder() async throws {
        let f = try StoreFixture(); defer { f.close() }
        let device = StorageRecordingDevice()
        let recorder = AudioRecorder(storage: f.oldStore, makeRecorder: { _, _ in device })
        let file = try recorder.start()
        try storageRequire(device.prepares == 1 && device.records == 1 && f.exists(file))
        try storageRequire(try f.childCleanup().active == 1)
        recorder.stop()
        try storageRequire(device.stops == 1 && f.exists(file))
        try storageRequire(try f.childCleanup().active == 1, "stop/upload/retry must keep the OS lease")
        recorder.cancel()
        recorder.cancel()
        try storageRequire(device.stops == 1 && !f.exists(file.deletingLastPathComponent()))
    }

    func controllerRetention() async throws {
        for ending in ["success", "cancel", "shutdown"] {
            let f = try StoreFixture(); defer { f.close() }
            let pending = StoragePendingTranscription()
            let recorder = AudioRecorder(storage: f.oldStore, makeRecorder: { _, _ in StorageRecordingDevice() })
            let suite = "VoicePortStorageChecks.\(UUID().uuidString)"
            let defaults = UserDefaults(suiteName: suite)!
            let controller = DictationController(audio: recorder, paste: StorageTextDelivery(), defaults: defaults,
                requestMicrophonePermission: { true }, transcribeAudio: { _, _ in try await pending.call() })
            defer { controller.shutdown(); pending.releaseAll(); defaults.removePersistentDomain(forName: suite) }
            controller.start()
            try await eventually { controller.phase == .recording }
            controller.stop()
            try await eventually { pending.continuations.count == 1 }
            try storageRequire(try f.childCleanup().active == 1)
            pending.fail(0)
            try await eventually { controller.phase == .failed }
            try storageRequire(controller.canRetry && (try f.childCleanup()).active == 1)
            controller.retry()
            try await eventually { pending.continuations.count == 2 }
            try storageRequire(try f.childCleanup().active == 1)
            if ending == "success" {
                pending.succeed(1)
                try await eventually { controller.phase == .ready }
            } else if ending == "cancel" { controller.cancel() }
            else { controller.shutdown() }
            let names = try FileManager.default.contentsOfDirectory(atPath: f.root.path)
            try storageRequire(Set(names) == ["owner.json"], "\(ending) must synchronously discard recording resources")
        }
    }

    func failedStart() async throws {
        for failure in ["factory", "prepare", "record", "missing-output"] {
            let f = try StoreFixture(); defer { f.close() }
            let device = StorageRecordingDevice()
            let recorder = AudioRecorder(storage: f.store, makeRecorder: { file, _ in
                if failure == "factory" {
                    // Simulate AVFoundation replacing its output before throwing.
                    try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: file.path)
                    throw StorageTestError.injected
                }
                device.prepareResult = failure != "prepare"
                device.recordResult = failure != "record"
                if failure == "missing-output" { device.onPrepare = { try? FileManager.default.removeItem(at: file) } }
                return device
            })
            do { _ = try recorder.start(); throw AppCheckFailure(description: "expected start failure") }
            catch is AppCheckFailure { throw AppCheckFailure(description: "expected start failure") }
            catch {}
            try storageRequire(Set(try FileManager.default.contentsOfDirectory(atPath: f.root.path)) == ["owner.json"],
                           "\(failure) must not leave an unused session")
            try storageRequire(failure == "record" || device.records == 0, "permission/output protection must precede recording")
        }
    }

    func unsafeRoots() async throws {
        let mutations: [(URL) throws -> Void] = [
            { try FileManager.default.removeItem(at: $0.appendingPathComponent("owner.json")) },
            { try Data("malformed".utf8).write(to: $0.appendingPathComponent("owner.json")) },
            { try modifyMarker($0.appendingPathComponent("owner.json")) { $0["ownerUID"] = geteuid() + 1 } },
            { try modifyMarker($0.appendingPathComponent("owner.json")) { $0["version"] = 2 } },
            { try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: $0.path) },
            { try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: $0.appendingPathComponent("owner.json").path) }
        ]
        for mutate in mutations {
            let f = try StoreFixture(); defer { f.close() }
            let directory = try f.abandoned(age: 2 * RecordingStorage.retention)
            try mutate(f.root)
            try storageRequire(f.clean().rootUnavailable && f.exists(directory))
            do { _ = try f.store.createSession(); throw AppCheckFailure(description: "unsafe root accepted") }
            catch is AppCheckFailure { throw AppCheckFailure(description: "unsafe root accepted") }
            catch {}
            try storageRequire(f.exists(directory))
        }
    }

    func unknownSessions() async throws {
        var mutations: [(URL) throws -> Void] = [
            { try FileManager.default.removeItem(at: $0.appendingPathComponent("session.json")) },
            { try FileManager.default.removeItem(at: $0.appendingPathComponent("lease")) },
            { try Data("malformed".utf8).write(to: $0.appendingPathComponent("session.json")) },
            { try Data("not empty".utf8).write(to: $0.appendingPathComponent("lease")) },
            { try Data().write(to: $0.appendingPathComponent("unknown-file")) },
            { try FileManager.default.createDirectory(at: $0.appendingPathComponent("unknown-directory"), withIntermediateDirectories: false) },
            { try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: $0.path) }
        ]
        for file in ["session.json", "lease", "dictation.m4a"] {
            mutations.append { try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: $0.appendingPathComponent(file).path) }
        }
        let changes: [[String: Any]] = [
            ["application": "unknown"], ["version": 2], ["ownerUID": geteuid() + 1], ["kind": "unknown"],
            ["rootID": UUID().uuidString], ["sessionID": UUID().uuidString], ["createdAt": "not a number"],
            ["createdAt": -1], ["extra-field": true]
        ]
        for values in changes {
            mutations.append { try modifyMarker($0.appendingPathComponent("session.json")) { object in values.forEach { object[$0.key] = $0.value } } }
        }
        for mutate in mutations {
            let f = try StoreFixture(); defer { f.close() }
            let directory = try f.abandoned(age: 2 * RecordingStorage.retention)
            try mutate(directory)
            let report = f.clean()
            try storageRequire(report.removed == 0 && f.exists(directory), "unknown or anomalous content must remain intact")
        }
    }

    func links() async throws {
        for kind in ["root", "parent", "session", "owner.json", "session.json", "lease", "dictation.m4a", "hard-audio", "hard-marker", "hard-lease"] {
            let f = try StoreFixture(); defer { f.close() }
            let directory = try f.abandoned(age: 2 * RecordingStorage.retention)
            let external = f.top.appendingPathComponent("external")
            if kind == "root" || kind == "parent" || kind == "session" {
                let source = kind == "root" ? f.root : (kind == "parent" ? f.root : directory)
                try FileManager.default.moveItem(at: source, to: external)
                if kind == "parent" {
                    let linkedParent = f.top.appendingPathComponent("linked-parent")
                    try FileManager.default.createSymbolicLink(at: linkedParent, withDestinationURL: f.top)
                    try FileManager.default.moveItem(at: external, to: f.root)
                    let storage = RecordingStorage(rootURL: linkedParent.appendingPathComponent("managed"))
                    try storageRequire(storage.cleanupExpired().rootUnavailable && f.exists(directory))
                    continue
                }
                try FileManager.default.createSymbolicLink(at: source, withDestinationURL: external)
                try storageRequire(f.clean().removed == 0 && f.exists(external))
            } else {
                try Data("outside sentinel".utf8).write(to: external)
                try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: external.path)
                let file: URL
                if kind == "owner.json" { file = f.root.appendingPathComponent(kind) }
                else if kind.hasPrefix("hard-") {
                    let name = kind == "hard-audio" ? "dictation.m4a" : (kind == "hard-marker" ? "session.json" : "lease")
                    file = directory.appendingPathComponent(name)
                } else { file = directory.appendingPathComponent(kind) }
                try FileManager.default.removeItem(at: file)
                if kind.hasPrefix("hard-") { try FileManager.default.linkItem(at: external, to: file) }
                else { try FileManager.default.createSymbolicLink(at: file, withDestinationURL: external) }
                try storageRequire(f.clean().removed == 0 && f.exists(directory))
                try storageRequire(try String(contentsOf: external, encoding: .utf8) == "outside sentinel")
            }
        }
    }

    func futureMetadata() async throws {
        for file in ["root", "session", "owner.json", "session.json", "lease", "dictation.m4a"] {
            let f = try StoreFixture(); defer { f.close() }
            let directory = try f.abandoned(age: 2 * RecordingStorage.retention)
            let target = file == "root" ? f.root : (file == "session" ? directory :
                (file == "owner.json" ? f.root.appendingPathComponent(file) : directory.appendingPathComponent(file)))
            try FileManager.default.setAttributes([.modificationDate: f.now.addingTimeInterval(3600)], ofItemAtPath: target.path)
            try storageRequire(f.clean().removed == 0 && f.exists(directory))
        }
    }

    func legacyAndUnknown() async throws {
        let f = try StoreFixture(); defer { f.close() }
        let old = f.top.appendingPathComponent("VoicePort-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: old, withIntermediateDirectories: false)
        let expired = try f.abandoned(age: 2 * RecordingStorage.retention)
        let unknown = f.root.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: unknown, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        let report = f.clean()
        try storageRequire(report.removed == 1 && !f.exists(expired) && f.exists(old) && f.exists(unknown))
    }

    func launchCleanup() async throws {
        let f = try StoreFixture(); defer { f.close() }
        let expired = try f.abandoned(age: 2 * RecordingStorage.retention)
        var factories = 0
        let smoke = RecordingStorage.cleanupAtLaunch(arguments: ["VoicePort", "--smoke-test"], makeStorage: {
            factories += 1
            return f.store
        })
        try storageRequire(smoke == nil && factories == 0 && f.exists(expired))
        let normal = RecordingStorage.cleanupAtLaunch(arguments: ["VoicePort"], makeStorage: { f.cleanupStore })
        try storageRequire(normal?.removed == 1 && !f.exists(expired))
        let unavailable = RecordingStorage(rootURL: f.top.appendingPathComponent("missing/root"))
        try storageRequire(RecordingStorage.cleanupAtLaunch(arguments: [], makeStorage: { unavailable })?.rootUnavailable == true)
    }
}

@MainActor
private final class StoreFixture {
    let top: URL
    let root: URL
    let now = Date().addingTimeInterval(60)
    var store: RecordingStorage { RecordingStorage(rootURL: root) }
    var cleanupStore: RecordingStorage { RecordingStorage(rootURL: root, clock: { self.now }) }
    var oldStore: RecordingStorage { RecordingStorage(rootURL: root, clock: { self.now.addingTimeInterval(-2 * RecordingStorage.retention) }) }

    init() throws {
        // Foundation intentionally retains the /var alias here; realpath gives
        // the canonical test parent so O_NOFOLLOW_ANY need not traverse it.
        guard let canonical = realpath(FileManager.default.temporaryDirectory.path, nil) else { throw StorageTestError.injected }
        defer { free(canonical) }
        top = URL(fileURLWithPath: String(cString: canonical)).appendingPathComponent("VoicePort-VP006-Checks-\(UUID().uuidString)")
        root = top.appendingPathComponent("managed")
        try FileManager.default.createDirectory(at: top, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        try Data("VP006 isolated checks".utf8).write(to: top.appendingPathComponent("test-scope"))
    }
    func abandoned(age: TimeInterval) throws -> URL {
        let storage = RecordingStorage(rootURL: root, clock: { self.now.addingTimeInterval(-age) })
        let session = try storage.createSession()
        return session.audioURL.deletingLastPathComponent() // ARC releases lease, does not discard.
    }
    func clean() -> RecordingStorage.CleanupReport { cleanupStore.cleanupExpired() }
    func exists(_ url: URL) -> Bool { FileManager.default.fileExists(atPath: url.path) }
    func startChild(_ operation: String, time: Double? = nil) throws -> (Process, Pipe) {
        let process = Process()
        let pipe = Pipe()
        process.executableURL = URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL
        process.arguments = ["--vp006-storage-child", operation, root.path, String(time ?? now.timeIntervalSince1970)]
        process.standardOutput = pipe
        process.standardError = pipe
        try process.run()
        return (process, pipe)
    }
    func finishChild(_ child: (Process, Pipe)) throws -> String {
        let data = child.1.fileHandleForReading.readDataToEndOfFile()
        child.0.waitUntilExit()
        let operation = child.0.arguments?[1] ?? "unknown"
        let detail = operation == "cleanup" ? String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
            : "session created; exited without running destructors"
        print("OBS recording storage child operation=\(operation) exit=\(child.0.terminationStatus) \(detail)")
        try storageRequire(child.0.terminationReason == .exit && child.0.terminationStatus == 0,
                       "native child exit=\(child.0.terminationStatus): \(String(decoding: data, as: UTF8.self))")
        return String(decoding: data, as: UTF8.self)
    }
    func childCleanup() throws -> RecordingStorage.CleanupReport {
        try JSONDecoder().decode(RecordingStorage.CleanupReport.self, from: Data(finishChild(startChild("cleanup")).utf8))
    }
    func close() { try? FileManager.default.removeItem(at: top) } // Only the scope this fixture just created.
}

private func mode(_ url: URL) throws -> Int {
    (try FileManager.default.attributesOfItem(atPath: url.path)[.posixPermissions] as! NSNumber).intValue
}
private func marker(_ url: URL) throws -> [String: Any] {
    try JSONSerialization.jsonObject(with: Data(contentsOf: url)) as! [String: Any]
}
private func modifyMarker(_ url: URL, _ change: (inout [String: Any]) -> Void) throws {
    var object = try marker(url)
    change(&object)
    try JSONSerialization.data(withJSONObject: object).write(to: url)
}

private enum StorageTestError: Error { case injected }
@MainActor private final class StorageRecordingDevice: RecordingDevice {
    var prepareResult = true
    var recordResult = true
    var onPrepare: (() -> Void)?
    var prepares = 0
    var records = 0
    var stops = 0
    func prepareToRecord() -> Bool { prepares += 1; onPrepare?(); return prepareResult }
    func record() -> Bool { records += 1; return recordResult }
    func stop() { stops += 1 }
}
@MainActor private final class StorageTextDelivery: TextDelivery {
    func begin() {}
    func clear() {}
    func deliver(_ text: String, automatic: Bool) async -> String { "offline storage delivery" }
    func copy(_ text: String) -> Bool { true }
    func requestAccessibility() {}
    func shutdown() {}
}
@MainActor private final class StoragePendingTranscription {
    var continuations: [CheckedContinuation<String, Error>?] = []
    func call() async throws -> String { try await withCheckedThrowingContinuation { continuations.append($0) } }
    func fail(_ index: Int) { continuations[index]?.resume(throwing: StorageTestError.injected); continuations[index] = nil }
    func succeed(_ index: Int) { continuations[index]?.resume(returning: "offline text"); continuations[index] = nil }
    func releaseAll() {
        for index in continuations.indices { continuations[index]?.resume(throwing: CancellationError()); continuations[index] = nil }
    }
}

private func storageRequire(_ value: Bool, _ message: String = "condition failed",
                            file: StaticString = #fileID, line: UInt = #line) throws {
    try appRequire(value, message, file: file, line: line)
}
