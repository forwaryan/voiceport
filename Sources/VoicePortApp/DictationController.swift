import AppKit
import AVFoundation
import Combine
import VoicePortCore

@MainActor
final class DictationController: ObservableObject {
    enum Phase: Equatable { case idle, requestingPermission, recording, transcribing, delivering, ready, failed }
    @Published private(set) var phase: Phase = .idle
    @Published private(set) var transcript = ""
    @Published private(set) var message = "点好目标输入框，再按快捷键开始录音。"
    @Published private(set) var seconds = 0
    @Published var automaticPaste: Bool {
        didSet { defaults.set(automaticPaste, forKey: "automaticPaste") }
    }
    @Published var language: String {
        didSet { defaults.set(language, forKey: "language") }
    }
    @Published var alternativeShortcut: Bool {
        didSet { defaults.set(alternativeShortcut, forKey: "alternativeShortcut") }
    }
    @Published var hotkeyMessage = ""

    private let audio: any AudioRecording
    private let paste: any TextDelivery
    private let defaults: UserDefaults
    private let requestMicrophonePermission: @MainActor () async -> Bool
    private let backend: (any NativeTranscription)?
    private var cleanupPending = false
    private var recordedLanguage: String?
    private let transcribeAudio: @Sendable (URL, String?) async throws -> String
    private let waitForTick: @Sendable () async throws -> Void
    private var gate = SessionGate()
    private var audioFile: URL?
    private var operation: Task<Void, Never>?
    private var timer: Task<Void, Never>?

    init(audio: any AudioRecording = AudioRecorder(),
         paste: any TextDelivery = PasteCoordinator(),
         defaults: UserDefaults = .standard,
         requestMicrophonePermission: @escaping @MainActor () async -> Bool = { await AudioRecorder.requestPermission() },
         transcribeAudio: (@Sendable (URL, String?) async throws -> String)? = nil,
         backend: (any NativeTranscription)? = nil,
         waitForTick: @escaping @Sendable () async throws -> Void = { try await Task.sleep(for: .seconds(1)) }) {
        self.audio = audio
        self.paste = paste
        self.defaults = defaults
        self.requestMicrophonePermission = requestMicrophonePermission
        let selectedBackend: (any NativeTranscription)? = transcribeAudio == nil ? (backend ?? RustBridge()) : nil
        self.backend = selectedBackend
        self.transcribeAudio = transcribeAudio ?? { file, language in
            guard let selectedBackend else { throw RustBridgeError.missing }
            return try await selectedBackend.transcribe(file, language: language, retry: false)
        }
        self.waitForTick = waitForTick
        automaticPaste = defaults.object(forKey: "automaticPaste") as? Bool ?? true
        language = defaults.string(forKey: "language") ?? ""
        alternativeShortcut = defaults.bool(forKey: "alternativeShortcut")
    }

    var canCancel: Bool { !cleanupPending && [.requestingPermission, .recording, .transcribing].contains(phase) }
    var canStart: Bool { !cleanupPending && [.idle, .ready, .failed].contains(phase) }
    var canRetry: Bool { !cleanupPending && phase == .failed && audioFile != nil }
    var shortcutLabel: String { alternativeShortcut ? "⌃⇧Space" : "⌃⌥Space" }
    var status: String {
        switch phase {
        case .idle: "就绪"
        case .requestingPermission: "等待麦克风权限"
        case .recording: String(format: "录音 %02d:%02d", seconds / 60, seconds % 60)
        case .transcribing: "正在云端转写"
        case .delivering: "正在粘贴"
        case .ready: "文字已就绪"
        case .failed: "需要处理"
        }
    }

    func toggle() {
        if phase == .recording { stop() }
        else if canStart { start() }
    }

    func start() {
        guard canStart else { return }
        // A new recording replaces the previous in-memory result and retry audio.
        audio.cancel()
        audioFile = nil
        recordedLanguage = language.isEmpty ? nil : language
        let id = gate.begin()
        paste.begin()
        transcript = ""
        seconds = 0
        phase = .requestingPermission
        message = "正在检查麦克风权限。"
        operation = Task {
            guard await requestMicrophonePermission(), gate.accepts(id), !Task.isCancelled else {
                if gate.accepts(id) {
                    phase = .failed
                    message = "请在系统设置 → 隐私与安全性 → 麦克风中允许 VoicePort。"
                    paste.clear()
                }
                return
            }
            do {
                audioFile = try audio.start()
                phase = .recording
                message = "正在录音，再按 \(shortcutLabel) 结束。最长 3 分钟。"
                startTimer()
            } catch {
                fail(error, id: id)
            }
        }
    }

    func stop() {
        guard phase == .recording, let id = gate.current, let file = audioFile else { return }
        timer?.cancel()
        audio.stop()
        transcribe(file: file, id: id, allowAutomaticPaste: true)
    }

    func retry() {
        guard canRetry, let file = audioFile else { return }
        // An old recording must not be injected into a newly focused app after an error.
        paste.clear()
        transcribe(file: file, id: gate.begin(), allowAutomaticPaste: false)
    }

    func cancel() {
        guard phase != .delivering else { return }
        gate.cancel()
        operation?.cancel()
        timer?.cancel()
        paste.clear()
        if let backend {
            cleanupPending = true
            message = "正在停止转写并回收进程，录音仍保持占用。"
            Task {
                let stopped = await backend.stop()
                if stopped { audio.cancel(); audioFile = nil; cleanupPending = false; phase = .idle; message = "已取消，未投递文字。" }
                else { message = "无法确认转写进程停止，录音保持占用，请稍后退出重试。" }
            }
        } else {
            audio.cancel(); audioFile = nil; phase = .idle; message = "已取消，未投递文字。"
        }
    }

    func copyResult() {
        guard !transcript.isEmpty, phase != .delivering else { return }
        message = paste.copy(transcript) ? "已复制原始文字，请在目标输入框手动粘贴。" : "复制失败，文字仍保留在面板中。"
    }

    func requestAccessibility() { paste.requestAccessibility() }

    func shutdown() {
        gate.cancel()
        operation?.cancel()
        timer?.cancel()
        paste.shutdown()
        if let backend {
            cleanupPending = true
            Task { if await backend.stop() { audio.cancel(); cleanupPending = false } }
        } else { audio.cancel() }
    }

    func shutdownAndWait() async -> Bool {
        cleanupPending = true
        gate.cancel(); operation?.cancel(); timer?.cancel(); paste.shutdown()
        if let backend, !(await backend.stop()) { return false }
        audio.cancel(); audioFile = nil; return true
    }

    private func startTimer() {
        timer?.cancel()
        timer = Task {
            while !Task.isCancelled {
                do { try await waitForTick() } catch { return }
                guard phase == .recording else { return }
                seconds += 1
                if seconds >= 180 { stop(); return }
            }
        }
    }

    private func transcribe(file: URL, id: String, allowAutomaticPaste: Bool) {
        phase = .transcribing
        message = "正在上传到 ChatGPT 听写服务；可取消。"
        let selectedLanguage = recordedLanguage
        operation = Task {
            do {
                let text: String
                if let backend { text = try await backend.transcribe(file, language: selectedLanguage, retry: !allowAutomaticPaste) }
                else { text = try await transcribeAudio(file, selectedLanguage) }
                guard gate.accepts(id), !Task.isCancelled else { return }
                transcript = text
                audio.cancel()
                audioFile = nil
                phase = .delivering
                let deliveryMessage = await paste.deliver(text, automatic: automaticPaste && allowAutomaticPaste)
                guard gate.accepts(id), !Task.isCancelled else { return }
                message = deliveryMessage
                phase = .ready
            } catch is CancellationError {
                // User cancellation invalidates the gate first. A service-side cancellation of the
                // still-current session must leave a visible, retryable state instead of hanging.
                guard gate.accepts(id) else { return }
                paste.clear()
                phase = .failed
                message = "转写已中断，可手动重试这段录音或重新录音。"
            } catch {
                fail(error, id: id)
            }
        }
    }

    private func fail(_ error: Error, id: String) {
        guard gate.accepts(id) else { return }
        audio.stop()
        timer?.cancel()
        paste.clear()
        phase = .failed
        if let error = error as? RustBridgeError, case .unsafeStop = error { cleanupPending = true }
        message = (error as? LocalizedError)?.errorDescription ?? "操作失败，请检查麦克风和网络后重试。"
    }
}
