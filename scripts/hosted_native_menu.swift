import AppKit
import ApplicationServices
import Darwin
import Security

struct ProbeError: Error { let message: String }

final class DriverProgress {
    enum Phase: String {
        case attachAuthorization, firstTick, identity, origin, initialState, statusItem, statusHit
        case visiblePopover, focusWindow, localDisclosure, localFocus, localClick, localState
        case focusRestored, closePopover, reopen, externalClick, externalModal, externalDecision
        case ready, menuAction, visibleContextMenu, hidden, updatesLink, budget
    }
    var phase: Phase = .attachAuthorization
    private var reported = false

    func reportFailure(sourceScene: Bool) {
        guard !reported else { return }
        reported = true
        let prefix = sourceScene ? "source-driver-phase" : "menu-driver-phase"
        FileHandle.standardError.write(Data(("\(prefix) \(phase.rawValue)\n").utf8))
    }
}
enum DriverUnavailable: Error { case permissions(Bool, Bool) }
func require(_ condition: Bool, _ message: String) throws {
    if !condition { throw ProbeError(message: message) }
}

func emit(_ row: [String: Any]) throws {
    let bytes = try JSONSerialization.data(withJSONObject: row, options: [.sortedKeys])
    FileHandle.standardOutput.write(bytes + Data([10]))
}

func metric(_ samples: [Double], scene: String) throws -> [String: Any] {
    try require(samples.count == 30 && samples.allSatisfy { $0.isFinite && $0 >= 0 }, "invalid sample set")
    let ranks = samples.sorted()
    return ["kind": "summary", "scene": scene, "n": ranks.count, "p50_ms": ranks[14],
            "p95_ms": ranks[28], "over_budget": ranks[28] > 100]
}

func elapsedMS(since start: TimeInterval) -> Double {
    (ProcessInfo.processInfo.systemUptime - start) * 1000
}

func selfTest() throws {
    let pass = try metric((1...30).map(Double.init), scene: "synthetic-fast")
    let stall = try metric((1...30).map { 400 + Double($0) }, scene: "synthetic-stall")
    try require(pass["p50_ms"] as? Double == 15 && pass["p95_ms"] as? Double == 29 && pass["over_budget"] as? Bool == false,
                "nearest-rank positive control failed")
    try require(stall["p95_ms"] as? Double == 429 && stall["over_budget"] as? Bool == true,
                "nearest-rank stall control failed")
    do {
        _ = try metric(Array(repeating: 0, count: 29), scene: "missing")
        throw ProbeError(message: "missing sample passed")
    } catch let error as ProbeError {
        try require(error.message == "invalid sample set", error.message)
    }
    try emit(pass)
    try emit(stall)

    var samples: [Double] = []
    let loop = CFRunLoopGetMain()
    for index in 1...30 {
        let start = ProcessInfo.processInfo.systemUptime
        var sample: Double?
        CFRunLoopPerformBlock(loop, CFRunLoopMode.defaultMode.rawValue) {
            precondition(Thread.isMainThread)
            if index > 28 { Thread.sleep(forTimeInterval: 0.4) }
            sample = elapsedMS(since: start)
            CFRunLoopStop(loop)
        }
        CFRunLoopWakeUp(loop)
        CFRunLoopRunInMode(.defaultMode, 1, false)
        guard let sample else { throw ProbeError(message: "main loop sample did not run") }
        samples.append(sample)
    }
    let injected = try metric(samples, scene: "main-run-loop-stall")
    try require((injected["p95_ms"] as? Double ?? 0) >= 390 && injected["over_budget"] as? Bool == true,
                "main run loop 400ms injection was not detected")
    try emit(injected)
}

func sysctl(_ key: String) -> String? {
    var count = 0
    guard sysctlbyname(key, nil, &count, nil, 0) == 0, (1...4096).contains(count) else { return nil }
    var data = [CChar](repeating: 0, count: count + 1)
    guard sysctlbyname(key, &data, &count, nil, 0) == 0 else { return nil }
    if key == "kern.hv_vmm_present" && count == MemoryLayout<Int32>.size {
        return data.withUnsafeBytes { String($0.loadUnaligned(as: Int32.self)) }
    }
    return data.withUnsafeBufferPointer { String(cString: $0.baseAddress!) }
}

func authorize(boot: String, session: String) throws {
    try require(!boot.isEmpty && sysctl("kern.bootsessionuuid") == boot && sysctl("kern.hv_vmm_present") == "1",
                "requires authorized VM boot UUID")
    var id: SecuritySessionId = 0
    var attributes = SessionAttributeBits(rawValue: 0)
    let result = SessionGetInfo(SecuritySessionId.max, &id, &attributes)
    var console = stat()
    let host = CGSessionCopyCurrentDictionary() as? [String: Any]
    try require(getuid() == geteuid() && geteuid() > 0 && stat("/dev/console", &console) == 0 && console.st_uid == geteuid()
                && result == 0 && UInt32(session) == id && attributes.rawValue & 0x1010 == 0x0010
                && (host?["kCGSSessionUserIDKey"] as? NSNumber)?.uint32Value == geteuid()
                && host?["kCGSSessionOnConsoleKey"] as? Bool == true && host?["kCGSessionLoginDoneKey"] as? Bool == true
                && (host?["CGSSessionScreenIsLocked"] == nil || host?["CGSSessionScreenIsLocked"] as? Bool == false),
                "requires authorized active local Aqua session")
    let ax = AXIsProcessTrusted()
    let post = CGPreflightPostEventAccess()
    guard ax && post else { throw DriverUnavailable.permissions(ax, post) }
}

func attr(_ element: AXUIElement, _ key: String) -> CFTypeRef? {
    var value: CFTypeRef?
    return AXUIElementCopyAttributeValue(element, key as CFString, &value) == .success ? value : nil
}

func children(_ element: AXUIElement, key: String = kAXChildrenAttribute) -> [AXUIElement] {
    attr(element, key) as? [AXUIElement] ?? []
}

func element(_ raw: CFTypeRef?) -> AXUIElement? {
    guard let raw, CFGetTypeID(raw) == AXUIElementGetTypeID() else { return nil }
    return unsafeDowncast(raw, to: AXUIElement.self)
}

func identifier(_ element: AXUIElement) -> String? { attr(element, kAXIdentifierAttribute) as? String }
func title(_ element: AXUIElement) -> String {
    let value = attr(element, kAXTitleAttribute) as? String ?? ""
    return value.isEmpty ? attr(element, kAXDescriptionAttribute) as? String ?? "" : value
}
func enabled(_ element: AXUIElement) -> Bool { attr(element, kAXEnabledAttribute) as? Bool == true }
func owner(_ element: AXUIElement) -> pid_t {
    var pid: pid_t = 0
    return AXUIElementGetPid(element, &pid) == .success ? pid : 0
}

func find(_ roots: [AXUIElement], deadline: TimeInterval? = nil,
          where matches: (AXUIElement) -> Bool) -> AXUIElement? {
    var pending = roots
    var visited = Set<AXUIElement>()
    while let node = pending.popLast(), visited.count < 150 {
        if let deadline, ProcessInfo.processInfo.systemUptime >= deadline { return nil }
        guard visited.insert(node).inserted else { continue }
        if matches(node) { return node }
        pending.append(contentsOf: children(node))
    }
    return nil
}

func center(_ element: AXUIElement) -> CGPoint? {
    guard let rawPosition = attr(element, kAXPositionAttribute),
          let rawSize = attr(element, kAXSizeAttribute),
          CFGetTypeID(rawPosition) == AXValueGetTypeID(), CFGetTypeID(rawSize) == AXValueGetTypeID() else { return nil }
    var position = CGPoint.zero
    var size = CGSize.zero
    guard AXValueGetValue(unsafeDowncast(rawPosition, to: AXValue.self), .cgPoint, &position),
          AXValueGetValue(unsafeDowncast(rawSize, to: AXValue.self), .cgSize, &size),
          size.width > 0, size.height > 0 else { return nil }
    return CGPoint(x: position.x + size.width / 2, y: position.y + size.height / 2)
}

func hit(_ point: CGPoint) -> AXUIElement? {
    var result: AXUIElement?
    let system = AXUIElementCreateSystemWide()
    AXUIElementSetMessagingTimeout(system, 0.12)
    let status = AXUIElementCopyElementAtPosition(system, Float(point.x), Float(point.y), &result)
    return status == .success ? result : nil
}

func onScreen(_ point: CGPoint, pid: pid_t) -> Bool {
    guard let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
        as? [[String: Any]] else { return false }
    return windows.contains { window in
        guard (window[kCGWindowOwnerPID as String] as? NSNumber)?.int32Value == pid,
              (window[kCGWindowAlpha as String] as? NSNumber)?.doubleValue ?? 0 > 0,
              let rect = window[kCGWindowBounds as String] as? [String: Any],
              let bounds = CGRect(dictionaryRepresentation: rect as CFDictionary) else { return false }
        return bounds.contains(point)
    }
}

func until<T>(_ label: String, seconds: Double = 3, interval: Double = 0.004,
              startedAt: TimeInterval? = nil, _ operation: () -> T?) throws -> T {
    let start = startedAt ?? ProcessInfo.processInfo.systemUptime
    while ProcessInfo.processInfo.systemUptime - start < seconds {
        if let result = operation() { return result }
        Thread.sleep(forTimeInterval: interval)
    }
    throw ProbeError(message: "timed out: \(label)")
}

func role(_ element: AXUIElement) -> String? { attr(element, kAXRoleAttribute) as? String }

func matchesHit(_ element: AXUIElement, at point: CGPoint, pid: pid_t) -> Bool {
    guard owner(element) == pid, let target = hit(point), owner(target) == pid else { return false }
    if CFEqual(target, element) { return true }
    if let id = identifier(element) { return identifier(target) == id }
    // AppKit context-menu rows may have no AXIdentifier.
    return role(element) == kAXMenuItemRole && role(target) == kAXMenuItemRole
        && !title(element).isEmpty && title(target) == title(element) && center(target) == point
}

// Values are synthetic per-driver ordinals, carried only by status-item mouse-up events.
let menuClickTag: Int64 = 0x56504D00

enum MenuGate: Int {
    case none, frontmost, disclosure, menuItem, caption, enabled, center, onScreen, exactHit, actionable
}

struct MenuPoll {
    let gate: MenuGate
    let started: Int
    let finished: Int
    let crossedDeadline: Bool

    var fields: String { "\(gate) \(started) \(finished) \(crossedDeadline)" }
}

func menuClickTrace(_ ordinal: Int, _ fields: String) {
    FileHandle.standardError.write(Data("menu-driver-click \(ordinal) \(fields)\n".utf8))
}

@discardableResult
func click(_ button: AXUIElement, pid: pid_t, right: Bool = false, menuOrdinal: Int? = nil) throws -> Double {
    guard let point = center(button), matchesHit(button, at: point, pid: pid),
          let down = CGEvent(mouseEventSource: nil, mouseType: right ? .rightMouseDown : .leftMouseDown,
                             mouseCursorPosition: point, mouseButton: right ? .right : .left),
          let up = CGEvent(mouseEventSource: nil, mouseType: right ? .rightMouseUp : .leftMouseUp,
                           mouseCursorPosition: point, mouseButton: right ? .right : .left) else {
        throw ProbeError(message: "mouse target is not the exact visible fixture control")
    }
    if let menuOrdinal {
        up.setIntegerValueField(.eventSourceUserData, value: menuClickTag | Int64(menuOrdinal))
    }
    let start = ProcessInfo.processInfo.systemUptime
    down.post(tap: .cghidEventTap)
    up.post(tap: .cghidEventTap)
    return start
}

func key(_ code: CGKeyCode, pid: pid_t) throws {
    try require(NSWorkspace.shared.frontmostApplication?.processIdentifier == pid, "fixture is not frontmost for the key event")
    for keyDown in [true, false] {
        guard let event = CGEvent(keyboardEventSource: nil, virtualKey: code, keyDown: keyDown) else {
            throw ProbeError(message: "could not create key event")
        }
        event.post(tap: .cghidEventTap)
    }
}

func escape(pid: pid_t) throws { try key(53, pid: pid) }

func scenario(binary: String, pid: pid_t, scene: String, directory: URL, appLog: URL,
              progress: DriverProgress?) throws {
    let fileManager = FileManager.default
    func exists(_ name: String) -> Bool { fileManager.fileExists(atPath: directory.appendingPathComponent(name).path) }
    progress?.phase = scene == "sources" ? .firstTick : .ready
    _ = try until("candidate ready", seconds: 8) { exists("ready") ? true : nil }
    progress?.phase = .identity
    let expected = URL(fileURLWithPath: binary).resolvingSymlinksInPath()
    _ = try until("owned executable registration", seconds: 1) {
        NSRunningApplication(processIdentifier: pid)?.executableURL?.resolvingSymlinksInPath() == expected ? true : nil
    }
    let app = AXUIElementCreateApplication(pid)
    AXUIElementSetMessagingTimeout(app, 0.12)
    func extra() -> [AXUIElement] { [element(attr(app, "AXExtrasMenuBar"))].compactMap { $0 } }
    func statusItem() throws -> AXUIElement {
        try until("VoicePort status item") { find(extra() + children(app)) { identifier($0) == "voiceport.menu-bar" } }
    }
    func control(_ id: String, deadline: TimeInterval? = nil) -> AXUIElement? {
        let window = element(attr(app, kAXFocusedWindowAttribute))
        return find(children(app, key: kAXWindowsAttribute) + children(app) + [window].compactMap { $0 },
                    deadline: deadline) {
            identifier($0) == "voiceport.quick." + id
        }
    }
    func visible(_ element: AXUIElement, observe: (MenuGate) -> Void = { _ in }) -> Bool {
        guard let point = center(element) else { return false }
        observe(.center)
        guard onScreen(point, pid: pid) else { return false }
        observe(.onScreen)
        guard matchesHit(element, at: point, pid: pid) else { return false }
        observe(.exactHit)
        return true
    }
    func showing(_ id: String) -> AXUIElement? {
        guard let button = control(id), visible(button) else { return nil }
        return button
    }
    func updateRow(_ prefix: String, observe: (MenuGate) -> Void = { _ in }) -> AXUIElement? {
        let focus = element(attr(app, kAXFocusedUIElementAttribute))
        let parent = focus.flatMap { element(attr($0, kAXParentAttribute)) }
        return find(extra() + children(app) + [focus, parent].compactMap { $0 }) {
            guard role($0) == kAXMenuItemRole else { return false }
            observe(.menuItem)
            guard title($0).hasPrefix(prefix) else { return false }
            observe(.caption)
            guard enabled($0) else { return false }
            observe(.enabled)
            return visible($0, observe: observe)
        }
    }
    let updatePrefix = scene == "blocked" ? "正在检查更新" : "软件更新"
    var menuOrdinal = 0
    func open(_ button: String, observeFirstLeft: Bool = false) throws -> Double {
        menuOrdinal += 1
        let ordinal = menuOrdinal
        menuClickTrace(ordinal, "begin \(button)")
        progress?.phase = .statusItem
        let item = try statusItem()
        progress?.phase = .statusHit
        let right = button == "right"
        let start = try click(item, pid: pid, right: right, menuOrdinal: ordinal)
        var first: MenuPoll?, last: MenuPoll?, best: MenuPoll?
        defer {
            menuClickTrace(ordinal, "posted")
            for (name, poll) in [("first", first), ("last", last), ("best", best)] {
                if let poll { menuClickTrace(ordinal, "\(name) \(poll.fields)") }
            }
            if observeFirstLeft && last?.gate != .actionable {
                // Preserve the original failure phase before synchronous AX diagnostics.
                progress?.reportFailure(sourceScene: scene == "sources")
                let diagnosticStart = ProcessInfo.processInfo.systemUptime
                let frontmost = NSWorkspace.shared.frontmostApplication?.processIdentifier == pid
                let deadline = ProcessInfo.processInfo.systemUptime + 0.5
                let action = control("more-sources", deadline: deadline)
                let point = action.flatMap(center)
                let ownedWindowAtPoint = point.map { onScreen($0, pid: pid) } ?? false
                let exactHit = action.flatMap { candidate in
                    point.map { matchesHit(candidate, at: $0, pid: pid) }
                } ?? false
                let actionEnabled = action.map(enabled) ?? false
                let line = "menu-ax-first-left frontmost=\(frontmost) disclosure=\(action != nil) "
                    + "center=\(point != nil) owned_window_at_point=\(ownedWindowAtPoint) exact_hit=\(exactHit) enabled=\(actionEnabled)\n"
                FileHandle.standardError.write(Data(line.utf8))
                menuClickTrace(ordinal, "postmortem \(Int((diagnosticStart - start) * 1000)) \(Int(elapsedMS(since: start)))")
            }
        }
        progress?.phase = right ? .visibleContextMenu : .visiblePopover
        let waitSeconds = 3.0
        let waitStart = ProcessInfo.processInfo.systemUptime
        func sample(_ evaluate: ((MenuGate) -> Void) -> Bool) -> TimeInterval? {
            let begin = ProcessInfo.processInfo.systemUptime
            var gate: MenuGate = .none
            let actionable = evaluate { if $0.rawValue > gate.rawValue { gate = $0 } }
            let end = ProcessInfo.processInfo.systemUptime
            let poll = MenuPoll(gate: actionable ? .actionable : gate,
                                started: Int((begin - start) * 1000), finished: Int((end - start) * 1000),
                                crossedDeadline: end - waitStart >= waitSeconds)
            if first == nil { first = poll }
            last = poll
            if best.map({ poll.gate.rawValue > $0.gate.rawValue }) ?? true { best = poll }
            return actionable ? end : nil
        }
        let endpoint: TimeInterval
        if right {
            endpoint = try until("on-screen actionable context-menu update item", seconds: waitSeconds, startedAt: waitStart) {
                sample { updateRow(updatePrefix, observe: $0) != nil }
            }
        } else {
            endpoint = try until("on-screen actionable popover disclosure", seconds: waitSeconds, startedAt: waitStart) {
                sample { observe in
                    guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid else { return false }
                    observe(.frontmost)
                    guard let action = control("more-sources") else { return false }
                    observe(.disclosure)
                    return visible(action, observe: observe) && enabled(action)
                }
            }
        }
        if scene != "sources" { progress?.phase = .menuAction }
        return (endpoint - start) * 1000
    }
    func close(_ button: String) throws {
        try escape(pid: pid)
        _ = try until("\(button) surface dismissed") {
            let present = button == "right" ? updateRow(updatePrefix) != nil : showing("more-sources") != nil
            return present ? nil : true
        }
    }
    func proveDisclosure() throws {
        let action = try until("disclosure present") { showing("more-sources") }
        try require(title(action).hasPrefix("更多来源"), "disclosure started expanded")
        try click(action, pid: pid)
        _ = try until("disclosure action changed UI") {
            guard let button = showing("more-sources"), title(button).contains("收起来源") else { return nil as Bool? }
            return true
        }
    }
    func requirePending(_ stage: String) throws {
        if scene != "blocked" { return }
        try require(exists("device-started") && exists("update-started")
                    && !exists("device-done") && !exists("update-done")
                    && !exists("device-timeout") && !exists("update-timeout") && !exists("device-duplicate"),
                    "both slow queries must remain entered at \(stage)")
    }
    func proveUpdatesLink(_ entry: String, action: AXUIElement) throws {
        progress?.phase = .updatesLink
        try require(enabled(action), "update action is disabled")
        try click(action, pid: pid)
        _ = try until("\(entry) deep link opened Updates in the focused workbench") {
            guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid,
                  let window = element(attr(app, kAXFocusedWindowAttribute)), owner(window) == pid,
                  let check = find([window], where: { identifier($0) == "voiceport.update.check" }),
                  enabled(check), visible(check), showing("more-sources") == nil, updateRow(updatePrefix) == nil else { return nil as Bool? }
            return true
        }
        try emit(["kind": "updates_deep_link", "scene": scene, "button": entry, "focused_workbench": true])
    }

    func keyboardFocus(_ id: String) throws {
        func focused() -> Bool {
            guard let target = showing(id), enabled(target),
                  let actual = element(attr(app, kAXFocusedUIElementAttribute)), owner(actual) == pid else { return false }
            return CFEqual(actual, target)
        }
        for _ in 0..<20 {
            if focused() { return }
            try key(48, pid: pid)
            // Hardware event posting is asynchronous. Wait one bounded AX round trip before the next key.
            Thread.sleep(forTimeInterval: 0.08)
        }
        try require(focused(), "could not reach the intended source control with Tab")
    }
    if scene == "sources" {
        let localA = "fixture-local-a", localB = "fixture-local-b", external = "fixture-external"
        let disclosure = "more-sources"
        let stateURL = directory.appendingPathComponent("source-state.json")
        progress?.phase = .origin
        let origin = try String(contentsOf: directory.appendingPathComponent("source-origin"), encoding: .utf8)
        try require(origin.range(of: #"^http://127\.0\.0\.1:[1-9][0-9]{0,4}$"#, options: .regularExpression) != nil,
                    "source origin is outside the synthetic loopback service")
        func state(_ id: String, _ count: Int, modal: Bool, popover: Bool) throws -> [String: Any] {
            try until("native source, preference and surface state") {
                guard let bytes = try? Data(contentsOf: stateURL),
                      let row = (try? JSONSerialization.jsonObject(with: bytes)) as? [String: Any],
                      row["source"] as? String == id, row["saved"] as? String == id,
                      row["writes"] as? Int == count, row["external_calls"] as? Int == 0,
                      row["modal"] as? Bool == modal, row["modal_key"] as? Bool == modal,
                      ((row["popover"] as? Int ?? 0) > 0) == popover else { return nil }
                return row
            }
        }
        progress?.phase = .initialState
        _ = try state(localA, 1, modal: false, popover: false)
        _ = try open("left", observeFirstLeft: true)
        progress?.phase = .focusWindow
        let window = try until("owned quick popover focus window") {
            guard let window = element(attr(app, kAXFocusedWindowAttribute)), owner(window) == pid else { return nil as AXUIElement? }
            return window
        }
        for (prior, selected, titleText, count, fromRow) in [
            (localA, localB, "Synthetic Local B", 2, true),
            (localB, localA, "Synthetic Local A", 3, false),
            (localA, localB, "Synthetic Local B", 4, false),
        ] {
            progress?.phase = .localDisclosure
            try proveDisclosure()
            let rowID = "source." + selected
            let before = try state(prior, count - 1, modal: false, popover: true)
            try require(before["expanded"] as? Bool == true, "source list did not open")
            progress?.phase = .localFocus
            try keyboardFocus(fromRow ? rowID : disclosure)
            progress?.phase = .localClick
            try click(try until("exact synthetic local source") { showing(rowID) }, pid: pid)
            progress?.phase = .localState
            let after = try state(selected, count, modal: false, popover: true)
            try require(after["popover"] as? Int == before["popover"] as? Int && after["expanded"] as? Bool == false
                        && after["native_disclosure_focus"] as? Bool == true, "AppKit did not restore disclosure in the same collapsed popover")
            progress?.phase = .focusRestored
            _ = try until("focused visible disclosure in the original popover") {
                guard let focusedWindow = element(attr(app, kAXFocusedWindowAttribute)), CFEqual(window, focusedWindow),
                      let button = showing(disclosure), enabled(button), title(button).hasPrefix("更多来源"),
                      let focus = element(attr(app, kAXFocusedUIElementAttribute)), owner(focus) == pid, CFEqual(focus, button),
                      let current = control("current"),
                      [title(current), attr(current, kAXValueAttribute) as? String ?? ""].contains(where: { $0.contains(titleText) }),
                      showing(rowID) == nil else { return nil as Bool? }
                return true
            }
            try emit(["kind": "local_source", "scene": scene, "from": prior, "to": selected,
                      "entry_focus": fromRow ? "source_row" : "disclosure", "same_popover": true,
                      "collapsed": true, "current_label": true, "native_focus": true, "ax_focus": true,
                      "preference_writes": count, "delivery": "hid-mouse"])
        }
        progress?.phase = .closePopover
        try close("left")
        _ = try state(localB, 4, modal: false, popover: false)
        _ = try open("left")
        progress?.phase = .reopen
        let reopened = try state(localB, 4, modal: false, popover: true)
        try require(reopened["expanded"] as? Bool == false, "reopened source list was not collapsed")
        _ = try until("reopened panel renders the saved local source") {
            guard let current = control("current"),
                  [title(current), attr(current, kAXValueAttribute) as? String ?? ""].contains(where: { $0.contains("Synthetic Local B") }) else { return nil as Bool? }
            return true
        }
        try emit(["kind": "source_reopen", "scene": scene, "saved_local_b": true])
        for operation in ["cancel", "confirm"] {
            if operation == "confirm" { _ = try open("left") }
            progress?.phase = .externalClick
            try proveDisclosure()
            try click(try until("exact synthetic external source") { showing("source." + external) }, pid: pid)
            progress?.phase = .externalModal
            let nativeModal = try state(localB, 4, modal: true, popover: false)
            try require(nativeModal["closed_before_alert"] as? Int == (operation == "cancel" ? 1 : 2),
                        "the popover was not closed before constructing each confirmation")
            let confirm = try until("owned focused modal with the expected synthetic destination") {
                guard NSWorkspace.shared.frontmostApplication?.processIdentifier == pid,
                      let modal = element(attr(app, kAXFocusedWindowAttribute)), owner(modal) == pid,
                      let focus = element(attr(app, kAXFocusedUIElementAttribute)), owner(focus) == pid,
                      CFEqual(modal, focus) || element(attr(focus, kAXWindowAttribute)).map({ CFEqual(modal, $0) }) == true,
                      showing(disclosure) == nil,
                      let message = find([modal], where: {
                          let text = (attr($0, kAXValueAttribute) as? String ?? "") + title($0)
                          return text.contains(origin) && text.contains(external) && !text.contains("/private-fixture")
                      }), owner(message) == pid,
                      let button = find([modal], where: { role($0) == kAXButtonRole && title($0) == "确认切换" }),
                      enabled(button), visible(button) else { return nil as AXUIElement? }
                return button
            }
            progress?.phase = .externalDecision
            if operation == "cancel" { try escape(pid: pid) } else { try click(confirm, pid: pid) }
            let confirmed = operation == "confirm"
            _ = try state(confirmed ? external : localB, confirmed ? 5 : 4, modal: false, popover: false)
            try require(showing(disclosure) == nil, "confirmation reopened the source panel")
            try emit(["kind": "external_source", "scene": scene, "operation": operation,
                      "stable_external_id": true, "destination": "synthetic-loopback", "focused_modal": true,
                      "popover_hidden_before_modal": true, "unchanged_before_decision": true,
                      "both_closed": true, "persisted": confirmed ? external : localB,
                      "decision_writes": confirmed ? 1 : 0, "external_calls": 0,
                      "delivery": confirmed ? "hid-mouse" : "hid-escape"])
        }
        return
    }

    progress?.phase = .menuAction
    let deviceStartedBeforeFirst = exists("device-started")
    try require(!deviceStartedBeforeFirst, "a device request launched before opening the popover")
    try emit(["kind": "first_click", "scene": scene, "button": "left", "ms": try open("left", observeFirstLeft: true),
              "device_started_before_click": deviceStartedBeforeFirst])
    try proveDisclosure()
    if scene == "blocked" {
        _ = try until("both synthetic queries entered") { exists("device-started") && exists("update-started") ? true : nil }
    }
    try close("left")
    try requirePending("first right click")
    try emit(["kind": "first_click", "scene": scene, "button": "right", "ms": try open("right")])
    try close("right")

    var exceededBudget: [String] = []
    for button in ["left", "right"] {
        var samples: [Double] = []
        for index in 1...30 {
            try requirePending("\(button) \(index) input")
            let ms = try open(button)
            try requirePending("\(button) \(index) readiness")
            if scene == "blocked" && button == "left" {
                try require(control("update").map(title)?.contains("正在检查更新") == true,
                            "popover update caption lost its pending state")
            }
            samples.append(ms)
            try emit(["kind": "click", "scene": scene, "button": button, "iteration": index, "ms": ms])
            try close(button)
        }
        var summary = try metric(samples, scene: scene)
        summary["button"] = button
        try emit(summary)
        if scene == "stall" {
            try require(summary["over_budget"] as? Bool == true && (summary["p95_ms"] as? Double ?? 0) >= 390,
                        "400ms main-thread negative control was not detected for \(button)")
        } else if summary["over_budget"] as? Bool == true {
            exceededBudget.append(button)
        }
    }
    if scene == "stall" {
        let log = try String(contentsOf: appLog, encoding: .utf8)
        let injections = log.split(separator: "\n").filter { $0 == "menu-probe: main-thread stall 400ms" }.count
        try require(injections == 62, "expected a native selector stall for each measured click and two first clicks")
        try emit(["kind": "stall_receipts", "scene": scene, "count": injections])
    }
    if scene == "warm" {
        progress?.phase = .hidden
        try Data().write(to: directory.appendingPathComponent("hidden-request"), options: .withoutOverwriting)
        let hidden: (Int, Double) = try until("60-second hidden-content counter", seconds: 65, interval: 0.1) {
            guard let line = try? String(contentsOf: directory.appendingPathComponent("hidden-result"), encoding: .utf8) else { return nil }
            let fields = line.split(separator: " ")
            guard fields.count == 2, let count = Int(fields[0]), let seconds = Double(fields[1]) else { return nil }
            return (count, seconds)
        }
        try require(!exists("hidden-invalid") && hidden.0 == 0 && hidden.1 >= 60,
                    "content rendered during the hidden interval")
        try emit(["kind": "hidden", "scene": scene, "render_calls": hidden.0, "seconds": hidden.1])
        _ = try open("left")
        try proveUpdatesLink("left", action: try until("popover update button") { showing("update") })
    }
    if scene == "blocked" {
        let rightClickMS = try open("right")
        let checking = try until("native right-click tracking with pending update") { updateRow("正在检查更新") }
        let before = center(checking)
        try require(before != nil, "right-menu row has no screen coordinates")
        try requirePending("native tracking before releasing held queries")
        try Data().write(to: directory.appendingPathComponent("release"), options: .withoutOverwriting)
        let failed = try until("update item changes while native menu is tracking", seconds: 5) { updateRow("无法检查更新") }
        try require(center(failed) == before, "update row moved while the menu was tracking")
        _ = try until("both slow query receipts") { exists("device-done") && exists("update-done") ? true : nil }
        try require(!exists("device-duplicate"), "a second device query started while the original remained blocked")
        try emit(["kind": "tracking_update", "scene": scene, "row_stable": true, "queries_completed": 2,
                  "right_click_ms": rightClickMS])
        try proveUpdatesLink("right", action: failed)
    }
    progress?.phase = .budget
    try require(exceededBudget.isEmpty, "click p95 exceeds 100ms: \(exceededBudget.joined(separator: ", "))")
}

var progress: DriverProgress?
var sourceScene = false
do {
    let args = Array(CommandLine.arguments.dropFirst())
    if args == ["--self-test"] {
        try selfTest()
    } else if args.count == 3 && args[0] == "--preflight" {
        try authorize(boot: args[1], session: args[2])
    } else {
        try require(args.count == 8 && args[0] == "--attach", "usage: menu-click --attach PID BINARY DATA LOG BOOT_UUID AQUA_ID SCENE")
        sourceScene = args[7] == "sources"
        progress = DriverProgress()
        guard let pid = pid_t(args[1]), pid > 0 else { throw ProbeError(message: "invalid candidate PID") }
        try authorize(boot: args[5], session: args[6])
        try scenario(binary: args[2], pid: pid, scene: args[7],
                     directory: URL(fileURLWithPath: args[3], isDirectory: true),
                     appLog: URL(fileURLWithPath: args[4]), progress: progress)
    }
} catch DriverUnavailable.permissions(let ax, let post) {
    try? emit(["ax": ax, "event_post": post])
    exit(77)
} catch {
    if let progress {
        progress.reportFailure(sourceScene: sourceScene)
    } else {
        let message = (error as? ProbeError)?.message ?? String(describing: error)
        FileHandle.standardError.write(Data(("menu-click: \(message)\n").utf8))
    }
    exit(1)
}
