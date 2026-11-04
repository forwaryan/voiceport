import AVFoundation
import Foundation

@MainActor
protocol RecordingDevice: AnyObject {
    func prepareToRecord() -> Bool
    func record() -> Bool
    func stop()
}

extension AVAudioRecorder: RecordingDevice {}

@MainActor
final class AudioRecorder: AudioRecording {
    private let storage: RecordingStorage
    private let makeRecorder: @MainActor (URL, [String: Any]) throws -> any RecordingDevice
    private var recorder: (any RecordingDevice)?
    private var session: RecordingStorage.Session?

    init(storage: RecordingStorage = RecordingStorage(),
         makeRecorder: @escaping @MainActor (URL, [String: Any]) throws -> any RecordingDevice = {
             try AVAudioRecorder(url: $0, settings: $1)
         }) {
        self.storage = storage
        self.makeRecorder = makeRecorder
    }

    static func requestPermission() async -> Bool {
        switch AVCaptureDevice.authorizationStatus(for: .audio) {
        case .authorized: return true
        case .notDetermined: return await AVCaptureDevice.requestAccess(for: .audio)
        default: return false
        }
    }

    func start() throws -> URL {
        cancel()
        let session = try storage.createSession()
        self.session = session
        let settings: [String: Any] = [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: 24_000,
            AVNumberOfChannelsKey: 1,
            AVEncoderAudioQualityKey: AVAudioQuality.high.rawValue,
            AVEncoderBitRateKey: 64_000,
        ]
        do {
            let recorder = try makeRecorder(session.audioURL, settings)
            self.recorder = recorder
            guard recorder.prepareToRecord() else { throw RecordingError.startFailed }
            // Protect AVFoundation's prepared file before recording starts.
            try session.secureAudioFile()
            guard recorder.record() else { throw RecordingError.startFailed }
            return session.audioURL
        } catch {
            cancel()
            throw error
        }
    }

    func stop() {
        recorder?.stop()
        recorder = nil
    }

    func cancel() {
        stop()
        if let session {
            // A failed AVFoundation initializer may have replaced its output.
            // Re-secure only a regular, single-link file owned by this user.
            try? session.secureAudioFile()
            session.discard()
        }
        session = nil
    }

    enum RecordingError: LocalizedError {
        case startFailed
        var errorDescription: String? { "无法开始录音，请检查麦克风是否可用。" }
    }
}
