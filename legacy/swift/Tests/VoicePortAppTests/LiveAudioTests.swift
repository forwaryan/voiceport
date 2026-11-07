import AVFoundation
import Darwin
import Foundation

@MainActor
struct LiveAudioTests {
    var tests: [(String, @MainActor () async throws -> Void)] {
        [("live language codes resolve only to supported regional locales", languageLocales),
         ("live audio construction and cancel are inert", inert),
         ("live permission denial preserves batch recording", permissionFallback),
         ("live cancel rejects delayed microphone and Speech permissions", cancelledPreparation),
         ("live Speech callbacks cannot cross stop cancel or session replacement", callbackIsolation),
         ("live Speech failure and final result detach without stopping audio", speechTermination),
         ("live stopped audio is closed protected and leased until cancel", stoppedAudio),
         ("live start failure closes and removes its recording session", failedStart),
         ("live write and finalization failures prevent incomplete upload", failedWrites),
         ("live sample budget truncates the boundary and rejects delayed buffers", sampleBudget),
         ("live tap writes serialize with concurrent close", concurrentClose),
         ("live AAC converts synthetic mono and stereo PCM without microphone", offlineAAC)]
    }

    func languageLocales() async throws {
        let supported = Set(["zh_CN", "zh_TW", "en_US", "en_GB", "ja_JP"].map(Locale.init(identifier:)))
        try appRequire(LiveSpeechLocale.resolve("zh", supported: supported, preferred: []) == Locale(identifier: "zh_CN"))
        try appRequire(LiveSpeechLocale.resolve("en", supported: supported, preferred: ["en-GB"]) == Locale(identifier: "en_GB"))
        try appRequire(LiveSpeechLocale.resolve("zh-TW", supported: supported, preferred: ["zh-CN"]) == Locale(identifier: "zh_TW"))
        try appRequire(LiveSpeechLocale.resolve("en-AU", supported: supported, preferred: ["en-US"]) == nil)
        try appRequire(LiveSpeechLocale.resolve("xx", supported: supported, preferred: ["en-US"]) == nil)
        try appRequire(LiveSpeechLocale.resolve("ja", supported: supported, preferred: []) == Locale(identifier: "ja_JP"))
    }

    func inert() async throws {
        let f = try LiveScope(); defer { f.close() }
        let dependencies = LiveAudioRecorder.Dependencies(
            microphonePermission: { throwUnexpectedBoundary(); return false },
            microphoneAuthorized: { throwUnexpectedBoundary(); return false },
            speechPermission: { throwUnexpectedBoundary(); return false },
            makeSpeech: { _ in throwUnexpectedBoundary(); throw LiveTestError.injected },
            makeCapture: { throwUnexpectedBoundary(); return LiveTestCapture() },
            makeWriter: { _, _ in throwUnexpectedBoundary(); throw LiveTestError.injected })
        let recorder = LiveAudioRecorder(storage: f.storage, dependencies: dependencies)
        recorder.stop(); recorder.cancel()
        _ = LiveAudioRecorder(storage: f.storage) // Default construction must also avoid OS objects/storage I/O.
        try appRequire(!FileManager.default.fileExists(atPath: f.root.path))
    }

    func permissionFallback() async throws {
        for failure in ["microphone", "speech", "recognizer"] {
            let f = try LiveScope(); defer { f.close() }
            let capture = LiveTestCapture()
            let writer = LiveTestWriter()
            var speechPermissionCalls = 0
            var factoryCalls = 0
            var statuses: [String] = []
            var d = injected(capture: capture, writer: writer)
            d.microphonePermission = { failure != "microphone" }
            d.microphoneAuthorized = { failure != "microphone" }
            d.speechPermission = { speechPermissionCalls += 1; return failure != "speech" }
            d.makeSpeech = { _ in factoryCalls += 1; throw LiveTestError.injected }
            let recorder = LiveAudioRecorder(storage: f.storage, dependencies: d)
            let prepared = await recorder.prepareLive(language: "zh", onPartial: { _ in }, onStatus: { statuses.append($0) })
            try appRequire(!prepared && statuses.count == 1 && capture.starts == 0)
            try appRequire(speechPermissionCalls == (failure == "microphone" ? 0 : 1))
            try appRequire(factoryCalls == (failure == "recognizer" ? 1 : 0))
            if failure == "microphone" {
                do { _ = try recorder.start(); throw LiveTestError.expectedFailure } catch is LiveTestError { throw LiveTestError.expectedFailure } catch {}
                try appRequire(!FileManager.default.fileExists(atPath: f.root.path))
            } else {
                let file = try recorder.start()
                try appRequire(FileManager.default.fileExists(atPath: file.path) && capture.starts == 1)
            }
            recorder.cancel()
        }
    }

    func cancelledPreparation() async throws {
        for waitingForSpeech in [false, true] {
            let f = try LiveScope(); defer { f.close() }
            let permission = LivePendingPermission()
            let capture = LiveTestCapture()
            var factoryCalls = 0
            var statuses: [String] = []
            var d = injected(capture: capture)
            if waitingForSpeech { d.speechPermission = { await permission.wait() } }
            else { d.microphonePermission = { await permission.wait() } }
            d.makeSpeech = { _ in factoryCalls += 1; return LiveTestSpeech() }
            let recorder = LiveAudioRecorder(storage: f.storage, dependencies: d)
            let pending = Task { await recorder.prepareLive(language: nil, onPartial: { _ in }, onStatus: { statuses.append($0) }) }
            try await eventually { permission.pending }
            recorder.cancel()
            // Begin a new generation before the old permission callback returns.
            let file = try recorder.start()
            permission.complete()
            let prepared = await pending.value
            try appRequire(!prepared && factoryCalls == 0 && statuses.isEmpty && capture.starts == 1)
            try appRequire(FileManager.default.fileExists(atPath: file.path))
            recorder.cancel()
        }
    }

    func callbackIsolation() async throws {
        let f = try LiveScope(); defer { f.close() }
        let capture = LiveTestCapture()
        var recognizers: [LiveTestSpeech] = []
        var partials: [String] = []
        var statuses: [String] = []
        var d = injected(capture: capture)
        d.makeSpeech = { _ in let speech = LiveTestSpeech(); recognizers.append(speech); return speech }
        let recorder = LiveAudioRecorder(storage: f.storage, dependencies: d)
        for ending in ["stop", "cancel", "replace"] {
            let prepared = await recorder.prepareLive(language: "en", onPartial: { partials.append($0) }, onStatus: { statuses.append($0) })
            try appRequire(prepared)
            _ = try recorder.start()
            let old = recognizers.last!
            old.emit(.result("accepted-\(ending)", final: false))
            try await eventually { partials.last == "accepted-\(ending)" }
            old.emit(.result("queued-old", final: false))
            if ending == "stop" { recorder.stop() }
            else if ending == "cancel" { recorder.cancel() }
            else {
                _ = await recorder.prepareLive(language: "zh", onPartial: { partials.append($0) }, onStatus: { statuses.append($0) })
                _ = try recorder.start()
            }
            old.emit(.result("late-old", final: true)); old.emit(.failed)
            for _ in 0..<20 { await Task.yield() }
            try appRequire(!partials.contains("queued-old") && !partials.contains("late-old") && statuses.isEmpty)
            try appRequire(old.cancels == 1 && old.request.ends == 1)
        }
        recorder.cancel()
    }

    func speechTermination() async throws {
        for final in [false, true] {
            let f = try LiveScope(); defer { f.close() }
            let capture = LiveTestCapture()
            let speech = LiveTestSpeech()
            var statuses: [String] = []
            var partials: [String] = []
            let recorder = LiveAudioRecorder(storage: f.storage, dependencies: injected(capture: capture, speech: speech))
            _ = await recorder.prepareLive(language: nil, onPartial: { partials.append($0) }, onStatus: { statuses.append($0) })
            _ = try recorder.start()
            speech.emit(final ? .result("final", final: true) : .failed)
            try await eventually { speech.cancels == 1 }
            try appRequire(capture.stops == 0 && speech.request.ends == 1)
            speech.emit(.result("late", final: false))
            for _ in 0..<10 { await Task.yield() }
            try appRequire(partials == (final ? ["final"] : []) && statuses.count == (final ? 0 : 1))
            recorder.cancel()
        }
    }

    func stoppedAudio() async throws {
        let f = try LiveScope(); defer { f.close() }
        let capture = LiveTestCapture()
        let writer = LiveTestWriter()
        let speech = LiveTestSpeech()
        let recorder = LiveAudioRecorder(storage: f.storage, dependencies: injected(capture: capture, writer: writer, speech: speech))
        _ = await recorder.prepareLive(language: nil, onPartial: { _ in }, onStatus: { _ in })
        let file = try recorder.start()
        let delayedSink = capture.sink!
        capture.feed(try pcm(format: capture.inputFormat, frames: 256))
        recorder.stop(); recorder.stop()
        let data = try Data(contentsOf: file)
        try appRequire(data == Data("closed".utf8) && writer.finishes == 1 && capture.stops == 1)
        let mode = try FileManager.default.attributesOfItem(atPath: file.path)[.posixPermissions] as! NSNumber
        try appRequire(mode.intValue == 0o600)
        try appRequire(f.cleaner.cleanupExpired().active == 1, "stopped live audio must retain its flock lease")
        delayedSink.consume(try pcm(format: capture.inputFormat, frames: 256))
        try appRequire(writer.frames == 256 && speech.request.frames == 256 && speech.request.ends == 1)
        recorder.cancel(); recorder.cancel()
        try appRequire(!FileManager.default.fileExists(atPath: file.deletingLastPathComponent().path))
    }

    func failedStart() async throws {
        for failure in ["writer", "engine", "protection"] {
            let f = try LiveScope(); defer { f.close() }
            let capture = LiveTestCapture()
            let writer = LiveTestWriter()
            capture.fails = failure == "engine"
            var d = injected(capture: capture, writer: writer)
            d.makeWriter = { url, _ in
                if failure == "writer" { throw LiveTestError.injected }
                if failure == "protection" { try FileManager.default.removeItem(at: url) }
                writer.url = url
                return writer
            }
            let recorder = LiveAudioRecorder(storage: f.storage, dependencies: d)
            do { _ = try recorder.start(); throw LiveTestError.expectedFailure } catch LiveTestError.expectedFailure { throw LiveTestError.expectedFailure } catch {}
            let names = try FileManager.default.contentsOfDirectory(atPath: f.root.path)
            try appRequire(Set(names) == ["owner.json"])
            try appRequire(capture.starts == (failure == "engine" ? 1 : 0))
            try appRequire(writer.finishes == (failure == "engine" ? 1 : 0))
        }
    }

    func failedWrites() async throws {
        for failureAtFinish in [false, true] {
            let f = try LiveScope(); defer { f.close() }
            let capture = LiveTestCapture()
            let writer = LiveTestWriter()
            writer.failWrite = !failureAtFinish
            writer.failFinish = failureAtFinish
            var statuses: [String] = []
            let recorder = LiveAudioRecorder(storage: f.storage, dependencies: injected(capture: capture, writer: writer))
            _ = await recorder.prepareLive(language: nil, onPartial: { _ in }, onStatus: { statuses.append($0) })
            let file = try recorder.start()
            capture.feed(try pcm(format: capture.inputFormat, frames: 256))
            // Deliberately stop before the queued failure callback has run.
            recorder.stop()
            try appRequire(!FileManager.default.fileExists(atPath: file.path), "sync stop must invalidate the upload URL on any write failure")
            try appRequire(statuses.count == 1 && writer.finishes == 1 && capture.stops == 1)
            for _ in 0..<10 { await Task.yield() }
            try appRequire(statuses.count == 1)
            recorder.cancel()
        }
    }

    func sampleBudget() async throws {
        let format = AVAudioFormat(standardFormatWithSampleRate: 8_000, channels: 2)!
        let writer = LiveTestWriter()
        let request = LiveTestRequest()
        let events = LiveTestEvents()
        let sink = LiveAudioSink(format: format, writer: writer, request: request, onEvent: { events.add($0) })
        let input = try pcm(format: format, frames: 65_537)
        for _ in 0..<24 { sink.consume(input) }
        try appRequire(writer.frames == 8_000 * 180 && request.frames == writer.frames)
        try appRequire(events.values == [.limitReached] && input.frameLength == 65_537)
        try appRequire(sink.finish())
        sink.consume(input)
        try appRequire(writer.frames == 8_000 * 180 && writer.finishes == 1 && request.ends == 1)
    }

    func concurrentClose() async throws {
        let format = AVAudioFormat(standardFormatWithSampleRate: 24_000, channels: 1)!
        let writer = LiveTestWriter()
        let request = LiveTestRequest()
        let sink = LiveAudioSink(format: format, writer: writer, request: request, onEvent: { _ in })
        await withTaskGroup(of: Void.self) { group in
            for _ in 0..<12 {
                group.addTask {
                    // Each producer owns its own synthetic buffer; no PCM objects cross executors.
                    let input = try! pcm(format: AVAudioFormat(standardFormatWithSampleRate: 24_000, channels: 1)!, frames: 64)
                    for _ in 0..<100 { sink.consume(input) }
                }
            }
            group.addTask { _ = sink.finish() }
        }
        try appRequire(sink.finish())
        try appRequire(writer.finishes == 1 && request.ends == 1 && writer.frames == request.frames)
        try appRequire(!writer.wroteAfterFinish && !request.appendedAfterEnd)
    }

    func offlineAAC() async throws {
        for (rate, channels) in [(24_000.0, UInt32(1)), (44_100.0, UInt32(2)), (48_000.0, UInt32(2))] {
            let f = try LiveScope(); defer { f.close() }
            let session = try f.storage.createSession()
            defer { session.discard() }
            let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: channels)!
            let writer = try LiveAACWriter(url: session.audioURL, inputFormat: format)
            try session.secureAudioFile()
            let sink = LiveAudioSink(format: format, writer: writer, request: nil, onEvent: { _ in })
            // One second of deterministic synthetic tone, no OS recording or recognition.
            var remaining = Int(rate)
            while remaining > 0 {
                let count = min(1024, remaining)
                sink.consume(try pcm(format: format, frames: AVAudioFrameCount(count)))
                remaining -= count
            }
            try appRequire(sink.finish(), "AAC conversion or converter drain failed")
            let audio = try AVAudioFile(forReading: session.audioURL)
            try appRequire(audio.fileFormat.sampleRate == 24_000 && audio.fileFormat.channelCount == 1)
            try appRequire(audio.fileFormat.streamDescription.pointee.mFormatID == kAudioFormatMPEG4AAC)
            try appRequire(abs(Double(audio.length) - 24_000) < 2048, "AAC duration must track the input sample rate")
            let decoded = AVAudioPCMBuffer(pcmFormat: audio.processingFormat, frameCapacity: AVAudioFrameCount(audio.length))!
            try audio.read(into: decoded)
            let samples = UnsafeBufferPointer(start: decoded.floatChannelData![0], count: Int(decoded.frameLength))
            try appRequire(samples.contains { abs($0) > 0.01 }, "synthetic audio must survive conversion and encoding")
        }
    }
}

@MainActor
private func injected(capture: LiveTestCapture, writer: LiveTestWriter = LiveTestWriter(), speech: LiveTestSpeech = LiveTestSpeech()) -> LiveAudioRecorder.Dependencies {
    LiveAudioRecorder.Dependencies(microphonePermission: { true }, microphoneAuthorized: { true }, speechPermission: { true },
        makeSpeech: { _ in speech }, makeCapture: { capture }, makeWriter: { url, _ in writer.url = url; return writer })
}

private enum LiveTestError: Error { case injected, expectedFailure }
private func throwUnexpectedBoundary() { fatalError("Inert construction crossed an OS boundary") }

private func pcm(format: AVAudioFormat, frames: AVAudioFrameCount) throws -> AVAudioPCMBuffer {
    guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames), let channels = buffer.floatChannelData else { throw LiveTestError.injected }
    buffer.frameLength = frames
    for channel in 0..<Int(format.channelCount) {
        for frame in 0..<Int(frames) { channels[channel][frame] = 0.2 * sin(Float(frame) * 0.1) }
    }
    return buffer
}

@MainActor
private final class LiveTestCapture: LiveAudioCapturing {
    var inputFormat = AVAudioFormat(standardFormatWithSampleRate: 48_000, channels: 2)!
    var sink: LiveAudioSink?
    var starts = 0
    var stops = 0
    var fails = false
    func start(sink: LiveAudioSink) throws {
        starts += 1
        self.sink = sink
        if fails { throw LiveTestError.injected }
    }
    func stop() { stops += 1; sink = nil }
    func feed(_ buffer: AVAudioPCMBuffer) { sink?.consume(buffer) }
}

@MainActor
private final class LiveTestSpeech: LiveSpeechRecognizing {
    let request = LiveTestRequest()
    var audioRequest: any LiveAudioRequest { request }
    var callback: (@Sendable (LiveSpeechEvent) -> Void)?
    var cancels = 0
    func start(onEvent: @escaping @Sendable (LiveSpeechEvent) -> Void) { callback = onEvent }
    func cancel() { cancels += 1 }
    func emit(_ event: LiveSpeechEvent) { callback?(event) }
}

private final class LiveTestWriter: LiveAudioWriting, @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    private var closed = 0
    private var afterFinish = false
    // Configuration is set before publication to the sink.
    var url: URL?
    var failWrite = false
    var failFinish = false
    var frames: Int { lock.withLock { count } }
    var finishes: Int { lock.withLock { closed } }
    var wroteAfterFinish: Bool { lock.withLock { afterFinish } }
    func write(_ buffer: AVAudioPCMBuffer) throws {
        try lock.withLock {
            if closed > 0 { afterFinish = true }
            if failWrite { throw LiveTestError.injected }
            count += Int(buffer.frameLength)
        }
    }
    func finish() throws {
        try lock.withLock {
            closed += 1
            if failFinish { throw LiveTestError.injected }
            if let url { try Data("closed".utf8).write(to: url) }
        }
    }
}

private final class LiveTestRequest: LiveAudioRequest, @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    private var ended = 0
    private var afterEnd = false
    var frames: Int { lock.withLock { count } }
    var ends: Int { lock.withLock { ended } }
    var appendedAfterEnd: Bool { lock.withLock { afterEnd } }
    func append(_ buffer: AVAudioPCMBuffer) { lock.withLock { if ended > 0 { afterEnd = true }; count += Int(buffer.frameLength) } }
    func endAudio() { lock.withLock { ended += 1 } }
}

private final class LiveTestEvents: @unchecked Sendable {
    private let lock = NSLock()
    private var events: [LiveAudioSink.Event] = []
    var values: [LiveAudioSink.Event] { lock.withLock { events } }
    func add(_ event: LiveAudioSink.Event) { lock.withLock { events.append(event) } }
}

@MainActor
private final class LivePendingPermission {
    var continuation: CheckedContinuation<Bool, Never>?
    var pending: Bool { continuation != nil }
    func wait() async -> Bool { await withCheckedContinuation { continuation = $0 } }
    func complete() { continuation?.resume(returning: true); continuation = nil }
}

private struct LiveScope {
    let top: URL
    let root: URL
    var storage: RecordingStorage { RecordingStorage(rootURL: root) }
    var cleaner: RecordingStorage { RecordingStorage(rootURL: root, clock: { Date().addingTimeInterval(2 * RecordingStorage.retention) }) }
    init() throws {
        guard let canonical = realpath(FileManager.default.temporaryDirectory.path, nil) else { throw LiveTestError.injected }
        defer { free(canonical) }
        top = URL(fileURLWithPath: String(cString: canonical)).appendingPathComponent("VoicePort-LiveAudio-Checks-\(UUID().uuidString)")
        root = top.appendingPathComponent("managed")
        try FileManager.default.createDirectory(at: top, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
    }
    func close() { try? FileManager.default.removeItem(at: top) }
}
