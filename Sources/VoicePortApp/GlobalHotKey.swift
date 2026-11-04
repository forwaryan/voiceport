import Carbon

@MainActor
final class GlobalHotKey {
    private var reference: EventHotKeyRef?
    private var handler: EventHandlerRef?
    private let action: @MainActor () -> Void

    init(action: @escaping @MainActor () -> Void) {
        self.action = action
    }

    func register(alternative: Bool = false) -> Bool {
        unregister()
        var specification = EventTypeSpec(eventClass: OSType(kEventClassKeyboard), eventKind: UInt32(kEventHotKeyPressed))
        let result = InstallEventHandler(GetApplicationEventTarget(), { _, _, context in
            guard let context else { return OSStatus(eventNotHandledErr) }
            MainActor.assumeIsolated {
                Unmanaged<GlobalHotKey>.fromOpaque(context).takeUnretainedValue().action()
            }
            return noErr
        }, 1, &specification, Unmanaged.passUnretained(self).toOpaque(), &handler)
        guard result == noErr else { return false }
        let modifiers = UInt32(controlKey | (alternative ? shiftKey : optionKey))
        let identity = EventHotKeyID(signature: 0x56504F52, id: 1)
        let registration = RegisterEventHotKey(UInt32(kVK_Space), modifiers, identity,
                                              GetApplicationEventTarget(), 0, &reference)
        if registration != noErr { unregister(); return false }
        return true
    }

    func unregister() {
        if let reference { UnregisterEventHotKey(reference) }
        if let handler { RemoveEventHandler(handler) }
        reference = nil
        handler = nil
    }
}
