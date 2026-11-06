import AppKit
@preconcurrency import ApplicationServices
import Carbon

// Adapted from .voiceport/evidence/vp005-independent/BoundaryFakes.swift.
// These module-local OS boundaries run the production DEFAULT target capture.
// They never query another process, request trust or post a real event. Tests
// with a textView use its actual AppKit role and setter-availability response;
// enabled state and AX IPC success/errors remain explicitly synthetic.
@MainActor
final class TargetAXBoundary {
    static let shared = TargetAXBoundary()
    let focus = AXUIElementCreateApplication(424_242)
    var textView: NSTextView?
    var role: CFTypeRef? = kAXTextAreaRole as CFString
    var enabled: CFTypeRef? = kCFBooleanTrue
    var valueSettable = true
    var settableError: AXError = .success
    var attributeErrors: [String: AXError] = [:]
    var requested: [String] = []
    var captures = 0
    var onCapture: ((Int) -> Void)?
    var trusted = true
    var secure = false
    var subrole = ""
    var selectedTextSettable = false
    var selectedRangeSettable = false
    var liveState = FocusedTextState(text: "", selection: NSRange(location: 0, length: 0))
    var writes = 0
    var writeError: AXError = .success

    func reset() {
        textView = nil
        role = kAXTextAreaRole as CFString
        enabled = kCFBooleanTrue
        valueSettable = true
        settableError = .success
        attributeErrors = [:]
        requested = []
        captures = 0
        onCapture = nil
        trusted = true; secure = false; subrole = ""
        selectedTextSettable = false; selectedRangeSettable = false
        liveState = FocusedTextState(text: "", selection: NSRange(location: 0, length: 0))
        writes = 0; writeError = .success
    }
}

@MainActor
final class NSWorkspace {
    static let shared = NSWorkspace()
    let frontmostApplication: TargetRunningApplication? = TargetRunningApplication()
}

struct TargetRunningApplication { let processIdentifier: pid_t = 424_242 }

@MainActor
func AXIsProcessTrusted() -> Bool { TargetAXBoundary.shared.trusted }

@MainActor
func IsSecureEventInputEnabled() -> Bool { TargetAXBoundary.shared.secure }

@MainActor
func AXUIElementCopyAttributeValue(_ element: AXUIElement, _ attribute: CFString,
                                   _ value: UnsafeMutablePointer<CFTypeRef?>) -> AXError {
    let boundary = TargetAXBoundary.shared
    let name = attribute as String
    boundary.requested.append(name)
    switch name {
    case kAXFocusedUIElementAttribute:
        boundary.captures += 1
        boundary.onCapture?(boundary.captures)
        value.pointee = boundary.focus
    case kAXRoleAttribute:
        if let view = boundary.textView { value.pointee = view.accessibilityRole()?.rawValue as CFString? }
        else { value.pointee = boundary.role }
    case kAXSubroleAttribute: value.pointee = boundary.subrole as CFString
    case kAXEnabledAttribute: value.pointee = boundary.enabled
    case kAXValueAttribute: value.pointee = boundary.liveState.text as CFString
    case kAXSelectedTextRangeAttribute:
        var range = CFRange(location: boundary.liveState.selection.location, length: boundary.liveState.selection.length)
        value.pointee = AXValueCreate(.cfRange, &range)
    default: return .attributeUnsupported
    }
    return boundary.attributeErrors[name] ?? .success
}

@MainActor
func AXUIElementIsAttributeSettable(_ element: AXUIElement, _ attribute: CFString,
                                    _ settable: UnsafeMutablePointer<DarwinBoolean>) -> AXError {
    let boundary = TargetAXBoundary.shared
    let name = attribute as String
    boundary.requested.append("settable:" + name)
    if name == kAXSelectedTextAttribute {
        settable.pointee = DarwinBoolean(boundary.selectedTextSettable)
        return boundary.settableError
    }
    if name == kAXSelectedTextRangeAttribute {
        settable.pointee = DarwinBoolean(boundary.selectedRangeSettable)
        return boundary.settableError
    }
    guard name == kAXValueAttribute else { return .attributeUnsupported }
    // The AppKit protocol reports whether this setter is permitted on the
    // in-memory view. This is a controlled adapter, not cross-process AX IPC.
    let allowed = boundary.textView?.isAccessibilitySelectorAllowed(NSSelectorFromString("setAccessibilityValue:"))
        ?? boundary.valueSettable
    settable.pointee = DarwinBoolean(allowed)
    return boundary.settableError
}

@MainActor
func AXUIElementSetAttributeValue(_ element: AXUIElement, _ attribute: CFString, _ value: CFTypeRef) -> AXError {
    let b = TargetAXBoundary.shared
    guard b.writeError == .success else { return b.writeError }
    switch attribute as String {
    case kAXSelectedTextRangeAttribute:
        guard b.selectedRangeSettable, CFGetTypeID(value) == AXValueGetTypeID() else { return .attributeUnsupported }
        var range = CFRange()
        guard AXValueGetValue(value as! AXValue, .cfRange, &range) else { return .illegalArgument }
        b.liveState.selection = NSRange(location: range.location, length: range.length)
    case kAXSelectedTextAttribute:
        guard b.selectedTextSettable, let text = value as? String else { return .attributeUnsupported }
        let range = b.liveState.selection
        b.liveState.text = (b.liveState.text as NSString).replacingCharacters(in: range, with: text)
        b.liveState.selection = NSRange(location: range.location + text.utf16.count, length: 0)
    case kAXValueAttribute:
        guard b.valueSettable, let text = value as? String else { return .attributeUnsupported }
        b.liveState.text = text
    default: return .attributeUnsupported
    }
    b.writes += 1
    return .success
}
