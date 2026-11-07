import AVFoundation
import Foundation
import Speech

@MainActor
protocol LiveAudioRecording: AudioRecording {
    var recordingFailure: (any Error)? { get }
    func prepareLive(language: String?,
                     onPartial: @escaping @MainActor (String) -> Void,
                     onStatus: @escaping @MainActor (String) -> Void) async -> Bool
}

extension LiveAudioRecording {
    var recordingFailure: (any Error)? { nil }
}

/// Construction is inert. Only an explicit prepare/start may ask for permissions
/// or create OS audio/Speech objects. A failed live preparation still permits batch recording.
@MainActor
final class LiveAudioRecorder: LiveAudioRecording {
    struct Dependencies {
        var microphonePermission: @MainActor () async -> Bool = { await AudioRecorder.requestPermission() }
        var microphoneAuthorized: @MainActor () -> Bool = {
            AVCaptureDevice.authorizationStatus(for: .audio) == .authorized
        }
        var speechPermission: @MainActor () async -> Bool = {
            switch SFSpeechRecognizer.authorizationStatus() {
            case .authorized: return true
            case .notDetermined:
                return await withCheckedContinuation { continuation in
                    SFSpeechRecognizer.requestAuthorization { @Sendable status in
                        continuation.resume(returning: status == .authorized)
                    }
                }
            default: return false
            }
        }
        var makeSpeech: @MainActor (String?) throws -> any LiveSpeechRecognizing = { try SystemLiveSpeech(language: $0) }
        var makeCapture: @MainActor () -> any LiveAudioCapturing = { EngineLiveCapture() }
        var makeWriter: (URL, AVAudioFormat) throws -> any LiveAudioWriting = { try LiveAACWriter(url: $0, inputFormat: $1) }
    }

    private let storage: RecordingStorage
    private let dependencies: Dependencies
    private var generation: UUID?
    private(set) var recordingFailure: (any Error)?
    private var onPartial: (@MainActor (String) -> Void)?
    private var onStatus: (@MainActor (String) -> Void)?
    private var speech: (any LiveSpeechRecognizing)?
    private var capture: (any LiveAudioCapturing)?
    private var sink: LiveAudioSink?
    private var session: RecordingStorage.Session?

    init(storage: RecordingStorage = RecordingStorage(), dependencies: Dependencies = Dependencies()) {
        self.storage = storage
        self.dependencies = dependencies
    }

    func prepareLive(language: String?,
                     onPartial: @escaping @MainActor (String) -> Void,
                     onStatus: @escaping @MainActor (String) -> Void) async -> Bool {
        cancel()
        recordingFailure = nil
        let id = UUID()
        generation = id
        self.onPartial = onPartial
        self.onStatus = onStatus
        let microphoneAllowed = await dependencies.microphonePermission()
        guard generation == id, !Task.isCancelled else { return false }
        guard microphoneAllowed else {
            onStatus("麦克风权限未获允许，无法录音。")
            return false
        }
        let speechAllowed = await dependencies.speechPermission()
        guard generation == id, !Task.isCancelled else { return false }
        guard speechAllowed else {
            onStatus("语音识别权限未获允许，将仅录音并在结束后转写。")
            return false
        }
        do {
            speech = try dependencies.makeSpeech(language)
            return true
        } catch {
            onStatus("\(error.localizedDescription) 将仅录音并在结束后转写。")
            return false
        }
    }

    func start() throws -> URL {
        if capture != nil || session != nil { cancel() }
        if generation == nil { generation = UUID() }
        let id = generation!
        recordingFailure = nil
        do {
            guard dependencies.microphoneAuthorized() else { throw LiveAudioError.microphoneDenied }
            let session = try storage.createSession()
            self.session = session
            let capture = dependencies.makeCapture()
            self.capture = capture
            let format = capture.inputFormat
            guard format.sampleRate.isFinite, (8_000...192_000).contains(format.sampleRate),
                  (1...8).contains(format.channelCount) else { throw LiveAudioError.invalidFormat }
            let writer = try dependencies.makeWriter(session.audioURL, format)
            // AVAudioFile may replace the prepared output; protect it before the tap/engine starts.
            try session.secureAudioFile()
            let sink = LiveAudioSink(format: format, writer: writer, request: speech?.audioRequest) { [weak self] event in
                Task { @MainActor [weak self] in self?.receive(event, generation: id) }
            }
            self.sink = sink
            speech?.start { [weak self] event in
                // Only value snapshots cross the callback/actor boundary, never Speech results or PCM buffers.
                Task { @MainActor [weak self] in self?.receive(event, generation: id) }
            }
            try capture.start(sink: sink)
            return session.audioURL
        } catch {
            let report = onStatus
            cancel()
            report?("无法安全开始录音：\(error.localizedDescription)")
            throw error
        }
    }

    func stop() {
        // Revoke callbacks before stopping OS objects, which may synchronously call back.
        generation = nil
        let report = onStatus
        onPartial = nil
        onStatus = nil
        capture?.stop()
        capture = nil
        let valid = sink?.finish() ?? true
        sink = nil
        // endAudio has run under the sink lock. Cancel also prevents a long tail
        // of final recognition from changing text after the controller starts batch transcription.
        speech?.cancel()
        speech = nil
        var protected = true
        if let session {
            do { try session.secureAudioFile() } catch { protected = false }
        }
        if !valid || !protected {
            recordingFailure = LiveAudioError.writeFailed
            session?.discard()
            session = nil
            report?("录音写入失败，已停止并丢弃不完整音频，请重新录音。")
        }
        // A successful stop retains the lease and closed AAC file through upload/retry.
    }

    func cancel() {
        stop()
        session?.discard()
        session = nil
    }

    private func receive(_ event: LiveSpeechEvent, generation id: UUID) {
        guard generation == id, sink != nil, speech != nil else { return }
        switch event {
        case let .result(text, final):
            let deliver = onPartial
            if final { detachSpeech() }
            if !text.isEmpty { deliver?(text) }
        case .failed:
            detachSpeech()
            onStatus?("本机实时识别已停止，将继续录音并在结束后转写。")
        case .failureCode(let code):
            detachSpeech()
            onStatus?("本机语音识别失败（错误码 \(code)），将继续录音并在结束后转写。")
        }
    }

    private func detachSpeech() {
        sink?.detachSpeech()
        speech?.cancel()
        speech = nil
    }

    private func receive(_ event: LiveAudioSink.Event, generation id: UUID) {
        guard generation == id, sink != nil else { return }
        let report = onStatus
        stop()
        if event == .limitReached { report?("录音已达到 3 分钟采样上限。") }
        // stop inspects the sink failure and reports it after closing/discarding the file.
    }
}

// These boundaries keep offline tests independent of actual microphones and recognizers.
@MainActor
protocol LiveAudioCapturing: AnyObject {
    var inputFormat: AVAudioFormat { get }
    func start(sink: LiveAudioSink) throws
    func stop()
}

@MainActor
protocol LiveSpeechRecognizing: AnyObject {
    var audioRequest: any LiveAudioRequest { get }
    func start(onEvent: @escaping @Sendable (LiveSpeechEvent) -> Void)
    func cancel()
}

enum LiveSpeechEvent: Sendable {
    case result(String, final: Bool)
    case failed
    case failureCode(Int)
}

/// append/endAudio are invoked exclusively under LiveAudioSink's lock.
protocol LiveAudioRequest: AnyObject, Sendable {
    func append(_ buffer: AVAudioPCMBuffer)
    func endAudio()
}

/// Created before tap installation, then owned exclusively by the locked sink.
protocol LiveAudioWriting: AnyObject {
    func write(_ buffer: AVAudioPCMBuffer) throws
    func finish() throws
}

@MainActor
private final class EngineLiveCapture: LiveAudioCapturing {
    private let engine = AVAudioEngine()
    private var tapped = false
    var inputFormat: AVAudioFormat { engine.inputNode.outputFormat(forBus: 0) }

    func start(sink: LiveAudioSink) throws {
        engine.inputNode.installTap(onBus: 0, bufferSize: 1024, format: inputFormat) { @Sendable buffer, _ in
            sink.consume(buffer)
        }
        tapped = true
        engine.prepare()
        try engine.start()
    }

    func stop() {
        engine.stop()
        if tapped { engine.inputNode.removeTap(onBus: 0); tapped = false }
    }
}

@MainActor
private final class SystemLiveSpeech: LiveSpeechRecognizing {
    private let recognizer: SFSpeechRecognizer
    private let request: SystemLiveRequest
    private var task: SFSpeechRecognitionTask?
    var audioRequest: any LiveAudioRequest { request }

    init(language: String?) throws {
        let recognizer: SFSpeechRecognizer?
        if let language, !language.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            guard let locale = LiveSpeechLocale.resolve(language, supported: SFSpeechRecognizer.supportedLocales(),
                                                       preferred: Locale.preferredLanguages) else {
                throw LiveAudioError.unsupportedLanguage
            }
            recognizer = SFSpeechRecognizer(locale: locale)
        } else { recognizer = SFSpeechRecognizer() }
        guard let recognizer else { throw LiveAudioError.unsupportedLanguage }
        guard recognizer.isAvailable else { throw LiveAudioError.speechUnavailable }
        guard recognizer.supportsOnDeviceRecognition else { throw LiveAudioError.onDeviceUnavailable }
        self.recognizer = recognizer
        self.request = SystemLiveRequest()
    }

    func start(onEvent: @escaping @Sendable (LiveSpeechEvent) -> Void) {
        task = recognizer.recognitionTask(with: request.value) { @Sendable result, error in
            if let result { onEvent(.result(result.bestTranscription.formattedString, final: result.isFinal)) }
            if let error { onEvent(.failureCode((error as NSError).code)) }
        }
    }

    func cancel() { task?.cancel(); task = nil }
}

/// Batch settings use language-only codes; Speech requires a supported regional
/// locale. Never silently substitute another language or an explicitly chosen region.
enum LiveSpeechLocale {
    static func resolve(_ language: String, supported: Set<Locale>, preferred: [String]) -> Locale? {
        func key(_ value: String) -> String {
            Locale(identifier: value).identifier.replacingOccurrences(of: "_", with: "-").lowercased()
        }
        let wanted = key(language.trimmingCharacters(in: .whitespacesAndNewlines))
        let locales = supported.sorted { $0.identifier < $1.identifier }
        if let exact = locales.first(where: { key($0.identifier) == wanted }) { return exact }
        guard !wanted.contains("-"), !wanted.isEmpty else { return nil }
        let candidates = locales.filter { key($0.identifier).split(separator: "-").first.map(String.init) == wanted }
        for preference in preferred + ["zh-CN", "en-US"] {
            if let match = candidates.first(where: { key($0.identifier) == key(preference) }) { return match }
        }
        return candidates.first
    }
}

/// The request is configured before publication. Its mutable append/end operations
/// are serialized by LiveAudioSink; Speech itself owns the recognition consumer.
private final class SystemLiveRequest: LiveAudioRequest, @unchecked Sendable {
    let value = SFSpeechAudioBufferRecognitionRequest()
    init() {
        value.requiresOnDeviceRecognition = true
        value.shouldReportPartialResults = true
    }
    func append(_ buffer: AVAudioPCMBuffer) { value.append(buffer) }
    func endAudio() { value.endAudio() }
}

/// No task-per-buffer or unbounded queue: the tap consumes synchronously. All
/// mutable writer/request state, including close, is protected by one lock.
/// A delayed tap after finish sees accepting=false and cannot touch closed resources.
final class LiveAudioSink: @unchecked Sendable {
    enum Event: Sendable { case failed, limitReached }
    static let maximumSeconds: Double = 180
    private let lock = NSLock()
    private let format: AVAudioFormat
    private let maximumFrames: Int64
    private let onEvent: @Sendable (Event) -> Void
    private var frames: Int64 = 0
    private var accepting = true
    private var valid = true
    private var writer: (any LiveAudioWriting)?
    private var request: (any LiveAudioRequest)?

    init(format: AVAudioFormat, writer: any LiveAudioWriting, request: (any LiveAudioRequest)?,
         onEvent: @escaping @Sendable (Event) -> Void) {
        self.format = format
        self.maximumFrames = Int64(format.sampleRate * Self.maximumSeconds)
        self.writer = writer
        self.request = request
        self.onEvent = onEvent
    }

    func consume(_ buffer: AVAudioPCMBuffer) {
        lock.lock()
        var event: Event?
        defer { lock.unlock(); if let event { onEvent(event) } }
        guard accepting else { return }
        do {
            guard buffer.format.isEqual(format), buffer.frameLength <= 1_048_576 else { throw LiveAudioError.invalidFormat }
            let count = min(Int64(buffer.frameLength), maximumFrames - frames)
            guard count > 0 else { return }
            let input = count == buffer.frameLength ? buffer : try prefix(buffer, frames: AVAudioFrameCount(count))
            try writer?.write(input)
            request?.append(input)
            frames += count
            if frames == maximumFrames { accepting = false; event = .limitReached }
        } catch {
            valid = false
            accepting = false
            event = .failed
        }
    }

    func detachSpeech() {
        lock.lock()
        defer { lock.unlock() }
        request?.endAudio()
        request = nil
    }

    /// Returns only after all writes have drained and AVAudioFile has closed.
    @discardableResult
    func finish() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        accepting = false
        request?.endAudio()
        request = nil
        do { try writer?.finish() } catch { valid = false }
        writer = nil
        return valid
    }

    private func prefix(_ input: AVAudioPCMBuffer, frames: AVAudioFrameCount) throws -> AVAudioPCMBuffer {
        guard let copy = AVAudioPCMBuffer(pcmFormat: input.format, frameCapacity: frames) else { throw LiveAudioError.invalidFormat }
        copy.frameLength = frames
        let source = UnsafeMutableAudioBufferListPointer(input.mutableAudioBufferList)
        let destination = UnsafeMutableAudioBufferListPointer(copy.mutableAudioBufferList)
        let bytes = Int(frames) * Int(input.format.streamDescription.pointee.mBytesPerFrame)
        for index in source.indices {
            guard let from = source[index].mData, let to = destination[index].mData,
                  bytes <= source[index].mDataByteSize, bytes <= destination[index].mDataByteSize else { throw LiveAudioError.invalidFormat }
            memcpy(to, from, bytes)
        }
        return copy
    }
}

/// The file and converter never escape the sink after construction. AAC remains
/// the existing 24 kHz mono upload format, regardless of the hardware input rate.
final class LiveAACWriter: LiveAudioWriting {
    private var file: AVAudioFile?
    private var converter: AVAudioConverter?
    private let outputFormat: AVAudioFormat
    private var framesWritten: AVAudioFramePosition = 0
    private let maximumFrames = AVAudioFramePosition(24_000 * 180)

    init(url: URL, inputFormat: AVAudioFormat) throws {
        let file = try AVAudioFile(forWriting: url, settings: [
            AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: 24_000,
            AVNumberOfChannelsKey: 1, AVEncoderBitRateKey: 64_000,
            AVEncoderAudioQualityKey: AVAudioQuality.high.rawValue,
        ], commonFormat: .pcmFormatFloat32, interleaved: false)
        outputFormat = file.processingFormat
        guard let converter = AVAudioConverter(from: inputFormat, to: outputFormat) else { throw LiveAudioError.invalidFormat }
        converter.downmix = true
        self.file = file
        self.converter = converter
    }

    func write(_ buffer: AVAudioPCMBuffer) throws {
        let capacity = AVAudioFrameCount(ceil(Double(buffer.frameLength) * outputFormat.sampleRate / buffer.format.sampleRate)) + 256
        try convert(buffer, capacity: capacity)
    }

    func finish() throws {
        defer { converter = nil; file = nil }
        if converter != nil { try convert(nil, capacity: 4096) }
    }

    private func convert(_ input: AVAudioPCMBuffer?, capacity: AVAudioFrameCount) throws {
        guard let converter, let file,
              let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: capacity) else { throw LiveAudioError.invalidFormat }
        let feeder = ConverterInput(input)
        // Capacity normally drains in one call. Bound unexpected converter behavior.
        for _ in 0..<16 {
            var error: NSError?
            let result = converter.convert(to: output, error: &error) { _, status in
                feeder.take(status: status)
            }
            guard error == nil, result != .error else { throw error ?? LiveAudioError.writeFailed as NSError }
            output.frameLength = min(output.frameLength, AVAudioFrameCount(max(0, maximumFrames - framesWritten)))
            if output.frameLength > 0 {
                try file.write(from: output)
                framesWritten += AVAudioFramePosition(output.frameLength)
            }
            if result == .inputRanDry || result == .endOfStream { return }
        }
        throw LiveAudioError.writeFailed
    }
}

/// AVAudioConverter annotates its synchronous pull block @Sendable. Keep the
/// once-only input cursor locked instead of capturing mutable locals or claiming
/// that AVAudioPCMBuffer itself is Sendable. The caller retains exclusive buffer
/// ownership for the duration of convert; this feeder never outlives that call.
private final class ConverterInput: @unchecked Sendable {
    private let lock = NSLock()
    private let buffer: AVAudioPCMBuffer?
    private var supplied = false
    init(_ buffer: AVAudioPCMBuffer?) { self.buffer = buffer }
    func take(status: UnsafeMutablePointer<AVAudioConverterInputStatus>) -> AVAudioBuffer? {
        lock.lock()
        defer { lock.unlock() }
        if let buffer, !supplied {
            supplied = true
            status.pointee = .haveData
            return buffer
        }
        status.pointee = buffer == nil ? .endOfStream : .noDataNow
        return nil
    }
}

private enum LiveAudioError: LocalizedError {
    case microphoneDenied, invalidFormat, writeFailed, unsupportedLanguage, speechUnavailable, onDeviceUnavailable
    var errorDescription: String? {
        switch self {
        case .microphoneDenied: "麦克风权限未获允许。"
        case .invalidFormat: "麦克风音频格式不可用。"
        case .writeFailed: "无法写入完整录音。"
        case .unsupportedLanguage: "系统不支持所选识别语言。"
        case .speechUnavailable: "系统语音识别当前不可用。"
        case .onDeviceUnavailable: "所选语言的本机识别不可用，可能尚未安装语言模型。"
        }
    }
}
