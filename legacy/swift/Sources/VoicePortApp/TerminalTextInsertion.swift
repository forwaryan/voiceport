import AppKit
import VoicePortCore

/// A terminal's hidden textarea is an input transport, not the CLI's text buffer.
/// Never rewrite it as a document or emit Delete/Return. Only append a prefix
/// that recognition has agreed on twice; revisions of committed text stop input.
@MainActor
final class TerminalTextInsertion {
    static let eventTag = TerminalKeyEvents.eventTag
    private let isCurrent: @MainActor () -> Bool
    private let post: @MainActor (String) -> Bool
    private var previous = ""
    private(set) var committed = ""
    private(set) var valid = true
    private var ended = false
    private var monitor: Any?

    init(isCurrent: @escaping @MainActor () -> Bool,
         post: @escaping @MainActor (String) -> Bool,
         monitorUserInput: Bool = true) {
        self.isCurrent = isCurrent
        self.post = post
        if monitorUserInput {
            monitor = NSEvent.addGlobalMonitorForEvents(matching: [.keyDown, .leftMouseDown, .rightMouseDown]) { [weak self] event in
                let own = event.cgEvent?.getIntegerValueField(.eventSourceUserData) == Self.eventTag
                let modifiers = event.modifierFlags.intersection([.control, .option, .shift, .command])
                let stopShortcut = event.type == .keyDown && event.keyCode == 49
                    && (modifiers == [.control, .option] || modifiers == [.control, .shift])
                guard !own, !stopShortcut else { return }
                Task { @MainActor [weak self] in self?.invalidate() }
            }
            if monitor == nil { valid = false; ended = true }
        }
    }

    @discardableResult
    func update(_ raw: String, final: Bool = false) -> Bool {
        guard valid, !ended, isCurrent() else { invalidate(); return false }
        let text = PasteText.automatic(raw)
        guard text.utf16.count <= 100_000 else { invalidate(); return false }
        let stable: String
        if final { stable = text }
        else {
            stable = String(zip(previous, text).prefix { $0.0 == $0.1 }.map { $0.0 })
            previous = text
        }
        let success = append(stable)
        if final { end() }
        return success
    }

    /// Flush local recognition at Stop before cloud correction starts. A later
    /// differing cloud result stays in the panel rather than deleting CLI input.
    @discardableResult
    func flush() -> Bool {
        guard valid, !ended, isCurrent() else { invalidate(); return false }
        return append(previous)
    }

    private func append(_ candidate: String) -> Bool {
        guard candidate.hasPrefix(committed) else { invalidate(); return false }
        let suffix = String(candidate.dropFirst(committed.count))
        guard suffix.isEmpty || (isCurrent() && post(suffix)) else { invalidate(); return false }
        committed = candidate
        return true
    }

    func invalidate() { valid = false; end() }
    func end() {
        ended = true
        if let monitor { NSEvent.removeMonitor(monitor); self.monitor = nil }
    }
}
