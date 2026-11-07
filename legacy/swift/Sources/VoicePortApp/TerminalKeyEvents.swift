import AppKit
import VoicePortCore

/// The caller owns target validation; each chunk rechecks it immediately before posting.
@MainActor
enum TerminalKeyEvents {
    static let eventTag: Int64 = 0x56504F5254545950

    static func post(_ raw: String, to pid: pid_t, isCurrent: () -> Bool) -> Bool {
        guard isCurrent() else { return false }
        let text = PasteText.automatic(raw)
        let source = CGEventSource(stateID: .privateState)
        func send(_ chunk: String) -> Bool {
            guard isCurrent(),
                  let down = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: true),
                  let up = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: false) else { return false }
            let units = Array(chunk.utf16)
            units.withUnsafeBufferPointer { buffer in
                down.keyboardSetUnicodeString(stringLength: buffer.count, unicodeString: buffer.baseAddress!)
                up.keyboardSetUnicodeString(stringLength: buffer.count, unicodeString: buffer.baseAddress!)
            }
            for event in [down, up] {
                event.flags = []
                event.setIntegerValueField(.eventSourceUserData, value: eventTag)
                event.postToPid(pid)
            }
            return true
        }
        var chunk = ""
        for scalar in text.unicodeScalars {
            let part = String(scalar)
            if chunk.utf16.count + part.utf16.count > 20 {
                guard send(chunk) else { return false }
                chunk = ""
            }
            chunk += part
        }
        return chunk.isEmpty || send(chunk)
    }
}
