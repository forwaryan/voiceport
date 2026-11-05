import Foundation
import Darwin
import CoreFoundation
import VoicePortCore

protocol NativeTranscription: Sendable {
    func transcribe(_ file: URL, language: String?, retry: Bool) async throws -> String
    func stop() async -> Bool
}

enum RustBridgeError: LocalizedError, Equatable {
    case missing, protocolFailure, stopped, timedOut, unsafeStop, serviceCancelled
    var errorDescription: String? {
        switch self {
        case .serviceCancelled: "转写服务已中断，可手动重试这段录音。"
        case .unsafeStop: "无法确认 Rust 进程停止，已保留录音占用，请重新尝试退出。"
        case .missing: "找不到已打包的 Rust 转写程序，请重新构建安装包。录音仍可重试。"
        case .protocolFailure: "Rust 转写进程或协议异常，录音仍保留，可手动重试。"
        case .stopped: "转写已中断，录音仍保留，可手动重试。"
        case .timedOut: "Rust 转写等待超时，录音仍保留，可手动重试。"
        }
    }
}

/// All process/pipe state belongs to a single background queue. Cancellation is
/// the only cross-queue flag; continuations resume only after child reclamation.
final class RustBridge: NativeTranscription, @unchecked Sendable {
    struct Limits: Sendable { var control: Double = 5; var job: Double = 130; var terminate: Double = 2 }
    static var bundledExecutable: URL { Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/voiceport-rs") }
    private let executable: URL
    private let environment: [String: String]?
    private let limits: Limits
    private let queue = DispatchQueue(label: "VoicePort.RustBridge")
    private let lock = NSLock()
    private var cancelled = false
    private var exitStatus: Int32?
    var lastChildExitStatus: Int32? { lock.withLock { exitStatus } }
    private var process: Process?
    private var input: Pipe?
    private var output: Pipe?
    private var errors: Pipe?
    private var bytes = Data()
    private var incarnation = UUID()
    private var activeSession: String?
    private var sessionHighWater: UInt64 = 0
    private var count = 0
    private var failed: (String, URL, String?)?

    init(executable: URL = RustBridge.bundledExecutable, environment: [String: String]? = nil, limits: Limits = Limits()) {
        self.executable = executable; self.environment = environment; self.limits = limits
    }
    private func isCancelled() -> Bool { lock.withLock { cancelled } }
    func stop() async -> Bool {
        lock.withLock { cancelled = true }
        return await withCheckedContinuation { continuation in
            queue.async { continuation.resume(returning: self.reap()) }
        }
    }
    func transcribe(_ file: URL, language: String?, retry: Bool) async throws -> String {
        try Task.checkCancellation()
        lock.withLock { cancelled = false }
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                queue.async {
                    do { continuation.resume(returning: try self.perform(file, language: language, retry: retry)) }
                    catch {
                        if error is CancellationError, let session = self.activeSession {
                            self.count += 1
                            try? self.send(["version":1,"action":"cancel","request_id":"n\(self.count)","session_id":session], ignoringCancellation: true)
                            _ = try? self.receive(until: self.deadline(self.limits.control), ignoringCancellation: true)
                        }
                        if !(error is VoicePortError), error as? RustBridgeError != .serviceCancelled, !self.reap() { continuation.resume(throwing: RustBridgeError.unsafeStop) }
                        else { continuation.resume(throwing: error) }
                    }
                }
            }
        } onCancel: { self.lock.withLock { self.cancelled = true } }
    }
    func status() async throws -> String {
        return try await withCheckedThrowingContinuation { continuation in
            queue.async {
                self.lock.withLock { self.cancelled = false }
                do {
                    try self.spawn(); self.count += 1; let id = "n\(self.count)"
                    try self.send(["version":1,"action":"status","request_id":id])
                    let event = try self.receive(until: self.deadline(self.limits.control))
                    guard event["event"] as? String == "status", event["request_id"] as? String == id, let state = event["state"] as? String else { throw RustBridgeError.protocolFailure }
                    continuation.resume(returning: state)
                } catch { self.reap(); continuation.resume(throwing: error) }
            }
        }
    }
    private func deadline(_ seconds: Double) -> Double { ProcessInfo.processInfo.systemUptime + seconds }
    private func check(_ end: Double, ignoringCancellation: Bool = false) throws {
        if !ignoringCancellation && isCancelled() { throw CancellationError() }
        if ProcessInfo.processInfo.systemUptime >= end { throw RustBridgeError.timedOut }
    }
    private func spawn() throws {
        if let process, process.isRunning { return }
        guard reap() else { throw RustBridgeError.unsafeStop }
        guard FileManager.default.isExecutableFile(atPath: executable.path) else { throw RustBridgeError.missing }
        let child = Process(); let stdin = Pipe(); let stdout = Pipe(); let stderr = Pipe()
        child.executableURL = executable; child.arguments = ["host"]; child.environment = environment
        child.standardInput = stdin; child.standardOutput = stdout; child.standardError = stderr
        do { try child.run() } catch { throw RustBridgeError.missing }
        process = child; input = stdin; output = stdout; errors = stderr; incarnation = UUID(); sessionHighWater = 0; count = 0; failed = nil; bytes.removeAll()
        for fd in [stdin.fileHandleForWriting.fileDescriptor, stdout.fileHandleForReading.fileDescriptor, stderr.fileHandleForReading.fileDescriptor] { _ = fcntl(fd, F_SETFL, O_NONBLOCK) }
        let diagnosticsFD = dup(stderr.fileHandleForReading.fileDescriptor)
        if diagnosticsFD >= 0 {
            DispatchQueue.global(qos: .utility).async {
                defer { Darwin.close(diagnosticsFD) }
                var discarded = [UInt8](repeating: 0, count: 8192)
                while child.isRunning {
                    let count = Darwin.read(diagnosticsFD, &discarded, discarded.count)
                    if count == 0 { break }
                    if count < 0 { Thread.sleep(forTimeInterval: 0.01) }
                }
            }
        }
        _ = fcntl(stdin.fileHandleForWriting.fileDescriptor, F_SETNOSIGPIPE, 1)
        let ready = try receive(until: deadline(limits.control))
        guard ready["event"] as? String == "ready", ready["state"] as? String == "idle", ready["request_id"] == nil, ready["session_id"] == nil else { throw RustBridgeError.protocolFailure }
    }
    private func send(_ value: [String: Any], ignoringCancellation: Bool = false) throws {
        guard let fd = input?.fileHandleForWriting.fileDescriptor else { throw RustBridgeError.stopped }
        var data = try JSONSerialization.data(withJSONObject: value); guard data.count <= 16384 else { throw RustBridgeError.protocolFailure }; data.append(10)
        let end = deadline(limits.control)
        try data.withUnsafeBytes { buffer in
            var offset = 0
            while offset < buffer.count {
                try check(end, ignoringCancellation: ignoringCancellation)
                var p = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0); _ = poll(&p, 1, 20)
                let n = Darwin.write(fd, buffer.baseAddress!.advanced(by: offset), buffer.count-offset)
                if n < 0 && (errno == EAGAIN || errno == EINTR) { continue }
                guard n > 0 else { throw RustBridgeError.stopped }; offset += n
            }
        }
    }
    private func receive(until end: Double, ignoringCancellation: Bool = false) throws -> [String: Any] {
        let child = incarnation
        while true {
            try check(end, ignoringCancellation: ignoringCancellation)
            guard child == incarnation else { throw RustBridgeError.stopped }
            if let newline = bytes.firstIndex(of: 10) {
                let frame = Data(bytes[..<newline]); bytes.removeSubrange(...newline)
                guard frame.count <= 2_000_000 else { throw RustBridgeError.protocolFailure }
                return try Self.decode(frame)
            }
            guard bytes.count <= 2_000_000, let fd = output?.fileHandleForReading.fileDescriptor else { throw RustBridgeError.protocolFailure }
            var p = pollfd(fd: fd, events: Int16(POLLIN | POLLHUP), revents: 0); _ = poll(&p, 1, 20)
            var block = [UInt8](repeating: 0, count: 8192)
            let n = Darwin.read(fd, &block, block.count)
            if n < 0 && (errno == EAGAIN || errno == EINTR) { continue }
            guard n > 0 else { bytes.removeAll(); throw RustBridgeError.stopped }
            bytes.append(contentsOf: block.prefix(n))
        }
    }
    static func decode(_ frame: Data) throws -> [String: Any] {
        // Scan top-level JSON keys before Foundation collapses duplicate keys.
        var depth = 0; var index = 0; var keys = Set<String>(); let raw = Array(frame)
        while index < raw.count {
            let b = raw[index]
            if b == 34 {
                let start = index; index += 1
                while index < raw.count { if raw[index] == 92 { index += 2; continue }; if raw[index] == 34 { break }; index += 1 }
                guard index < raw.count else { throw RustBridgeError.protocolFailure }
                var next = index+1; while next < raw.count && [9,10,13,32].contains(raw[next]) { next += 1 }
                if depth == 1 && next < raw.count && raw[next] == 58 {
                    guard let key = try? JSONSerialization.jsonObject(with: Data(raw[start...index]), options: .fragmentsAllowed) as? String, keys.insert(key).inserted else { throw RustBridgeError.protocolFailure }
                }
            } else if b == 123 || b == 91 { depth += 1 } else if b == 125 || b == 93 { depth -= 1 }
            index += 1
        }
        guard String(data: frame, encoding: .utf8) != nil,
              let object = try? JSONSerialization.jsonObject(with: frame) as? [String: Any],
              let version = object["version"] as? NSNumber, CFGetTypeID(version) != CFBooleanGetTypeID(), version == 1, !["d","f"].contains(String(cString: version.objCType)),
              let kind = object["event"] as? String, ["ready","accepted","status","result","error","cancelled","replaced","shutdown"].contains(kind),
              let state = object["state"] as? String, ["idle","transcribing","ready","failed"].contains(state),
              Set(object.keys).isSubset(of: ["version","event","state","request_id","session_id","is_retry","code","text"]) else { throw RustBridgeError.protocolFailure }
        for key in ["request_id","session_id","code","text"] where object[key] != nil { guard object[key] is String else { throw RustBridgeError.protocolFailure } }
        if let retry = object["is_retry"] { guard CFGetTypeID(retry as CFTypeRef) == CFBooleanGetTypeID() else { throw RustBridgeError.protocolFailure } }
        let base: Set<String> = ["version","event","state"]
        let work: Set<String> = base.union(["request_id","session_id","is_retry"])
        if kind == "ready" { guard Set(object.keys) == base else { throw RustBridgeError.protocolFailure } }
        if kind == "accepted" { guard Set(object.keys) == work else { throw RustBridgeError.protocolFailure } }
        if kind == "result" { guard Set(object.keys) == work.union(["text"]) else { throw RustBridgeError.protocolFailure } }
        guard kind == "result" || object["text"] == nil else { throw RustBridgeError.protocolFailure }
        return object
    }
    static func canonicalGeneration(_ session: String) -> UInt64? {
        let bytes = Array(session.utf8)
        guard (2...21).contains(bytes.count), bytes[0] == 115,
              (49...57).contains(bytes[1]), bytes.dropFirst().allSatisfy({ (48...57).contains($0) }) else { return nil }
        return UInt64(String(decoding: bytes.dropFirst(), as: UTF8.self))
    }

    private func perform(_ file: URL, language: String?, retry: Bool) throws -> String {
        try spawn()
        count += 1; let id = "n\(count)"; let previous = failed
        let wireRetry = retry && previous?.1 == file && previous?.2 == language
        var command: [String: Any] = ["version":1,"request_id":id,"action":wireRetry ? "retry" : "transcribe"]
        if wireRetry { command["session_id"] = previous?.0 } else { command["audio_path"] = file.path; if let language { command["language"] = language } }
        failed = nil; try send(command)
        let accepted = try receive(until: deadline(limits.control))
        guard accepted["event"] as? String == "accepted", accepted["request_id"] as? String == id,
              let session = accepted["session_id"] as? String,
              let generation = Self.canonicalGeneration(session), generation > sessionHighWater,
              accepted["state"] as? String == "transcribing", accepted["is_retry"] as? Bool == wireRetry else { throw RustBridgeError.protocolFailure }
        // Admission consumes the generation even if the service later fails.
        // This bounded high-water mark belongs to the child, not the request.
        sessionHighWater = generation
        activeSession = session
        let terminal = try receive(until: deadline(limits.job))
        guard terminal["request_id"] as? String == id, terminal["session_id"] as? String == session,
              terminal["is_retry"] as? Bool == wireRetry else { throw RustBridgeError.protocolFailure }
        activeSession = nil
        if terminal["event"] as? String == "result", terminal["state"] as? String == "ready", let text = terminal["text"] as? String, !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return text }
        guard terminal["event"] as? String == "error", terminal["state"] as? String == "failed", let code = terminal["code"] as? String else { throw RustBridgeError.protocolFailure }
        failed = (session,file,language)
        switch code {
        case "Cancelled": throw RustBridgeError.serviceCancelled
        case "SignInRequired": throw VoicePortError.signInRequired
        case "InvalidCredentials": throw VoicePortError.invalidCredentials
        case "ExpiredCredentials": throw VoicePortError.expiredCredentials
        case "TimedOut": throw VoicePortError.timedOut
        case "InvalidAudio": throw VoicePortError.invalidAudio
        case "AudioTooLarge": throw VoicePortError.audioTooLarge
        case "InvalidLanguage": throw VoicePortError.invalidLanguage
        case "InvalidResponse": throw VoicePortError.invalidResponse
        case "EmptyTranscript": throw VoicePortError.emptyTranscript
        case "Network": throw VoicePortError.network
        default:
            if code.hasPrefix("Service("), code.hasSuffix(")"), let status = Int(code.dropFirst(8).dropLast()) { throw VoicePortError.service(status) }
            throw RustBridgeError.protocolFailure
        }
    }
    @discardableResult
    private func reap() -> Bool {
        guard let child = process else { return true }
        // No PID is retained beyond the Process object. Do not reuse an exited PID.
        if child.isRunning {
            count += 1
            try? send(["version":1,"action":"shutdown","request_id":"n\(count)"], ignoringCancellation: true)
            _ = try? receive(until: deadline(limits.control), ignoringCancellation: true)
        }
        let polite = deadline(0.05)
        while child.isRunning && ProcessInfo.processInfo.systemUptime < polite { Thread.sleep(forTimeInterval: 0.005) }
        if child.isRunning { child.terminate() }
        let end = deadline(limits.terminate)
        while child.isRunning && ProcessInfo.processInfo.systemUptime < end { Thread.sleep(forTimeInterval: 0.01) }
        if child.isRunning { _ = kill(child.processIdentifier, SIGKILL) }
        let killed = deadline(limits.terminate)
        while child.isRunning && ProcessInfo.processInfo.systemUptime < killed { Thread.sleep(forTimeInterval: 0.01) }
        if child.isRunning { return false } // caller must not claim safe cleanup
        lock.withLock { exitStatus = child.terminationStatus }
        try? input?.fileHandleForWriting.close(); try? output?.fileHandleForReading.close(); try? errors?.fileHandleForReading.close()
        activeSession = nil; process = nil; input = nil; output = nil; errors = nil; failed = nil; bytes.removeAll(); sessionHighWater = 0; incarnation = UUID()
        return true
    }
}
