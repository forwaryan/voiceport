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
    }
}

@MainActor
final class NSWorkspace {
    static let shared = NSWorkspace()
    let frontmostApplication: TargetRunningApplication? = TargetRunningApplication()
}

struct TargetRunningApplication { let processIdentifier: pid_t = 424_242 }

@MainActor
func AXIsProcessTrusted() -> Bool { true }

@MainActor
func IsSecureEventInputEnabled() -> Bool { false }

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
    case kAXSubroleAttribute: value.pointee = "" as CFString
    case kAXEnabledAttribute: value.pointee = boundary.enabled
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
    guard name == kAXValueAttribute else { return .attributeUnsupported }
    // The AppKit protocol reports whether this setter is permitted on the
    // in-memory view. This is a controlled adapter, not cross-process AX IPC.
    let allowed = boundary.textView?.isAccessibilitySelectorAllowed(NSSelectorFromString("setAccessibilityValue:"))
        ?? boundary.valueSettable
    settable.pointee = DarwinBoolean(allowed)
    return boundary.settableError
}
