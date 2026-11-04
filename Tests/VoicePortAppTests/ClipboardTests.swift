import AppKit
import ApplicationServices
import Foundation

@MainActor
struct ClipboardTests {
    var tests: [(String, @MainActor () async throws -> Void)] {
        [("clipboard paste sanitizes text then restores original", normalPaste),
         ("clipboard restores multiple items and binary types", richClipboard),
         ("new clipboard before delivery is untouched", changedBeforeDelivery),
         ("new clipboard during wait is not restored over", changedDuringWait),
         ("switched app receives no paste event", changedApplication),
         ("changed focus receives no paste event", changedFocus),
         ("unknown focus receives no paste event", unknownFocus),
         ("secure input at capture or delivery blocks paste", secureInput),
         ("permission and automatic-paste setting gate actual writes", permissions),
         ("oversized clipboard remains untouched", oversizedClipboard),
         ("event creation failure restores original immediately", failedEvent),
         ("shutdown restores owned clipboard before returning", shutdownRestores),
         ("shutdown preserves a newer clipboard", shutdownPreserves),
         ("late delivery completion cannot undo shutdown recovery", lateCompletion),
         ("target change after validation prevents the key event", targetRace),
         ("non-editable focused element receives no paste", nonEditable),
         ("cancelled delivery never posts or retains temporary text", cancelledDelivery),
         ("interrupted wait restores only owned clipboard", interruptedWait),
         ("clipboard lease rejects overlapping delivery and copy", overlappingDelivery),
         ("final target validation rechecks all safety guards", finalSafetyGuards),
         ("new clipboard during final validation is preserved", clipboardDuringValidation),
         ("empty clipboard is restored to empty", emptyClipboard)]
    }

    func normalPaste() async throws {
        let fixture = ClipboardFixture()
        defer { fixture.close() }
        fixture.coordinator.begin()
        let message = await fixture.coordinator.deliver("口述\r\n不要发送\0", automatic: true)
        try appRequire(fixture.environment.events == 1)
        try appRequire(fixture.environment.pastedTexts == ["口述 不要发送 "])
        try appRequire(fixture.text == "原始剪贴板" && message.contains("发送粘贴"))
    }

    func richClipboard() async throws {
        let fixture = ClipboardFixture()
        defer { fixture.close() }
        let custom = NSPasteboard.PasteboardType("io.voiceport.checks.binary")
        let binary = Data([0, 255, 17, 10])
        let first = NSPasteboardItem()
        first.setString("第一项", forType: .string)
        first.setData(binary, forType: custom)
        let second = NSPasteboardItem()
        second.setString("第二项", forType: .string)
        fixture.board.clearContents()
        try appRequire(fixture.board.writeObjects([first, second]))
        fixture.coordinator.begin()
        _ = await fixture.coordinator.deliver("转写", automatic: true)
        let items = fixture.board.pasteboardItems ?? []
        try appRequire(items.count == 2)
        try appRequire(items[0].string(forType: .string) == "第一项" && items[0].data(forType: custom) == binary)
        try appRequire(items[1].string(forType: .string) == "第二项")
    }

    func changedBeforeDelivery() async throws {
        let fixture = ClipboardFixture()
        defer { fixture.close() }
        fixture.coordinator.begin()
        fixture.write("新复制的内容")
        _ = await fixture.coordinator.deliver("转写", automatic: true)
        try appRequire(fixture.environment.events == 0 && fixture.text == "新复制的内容")
    }

    func changedDuringWait() async throws {
        try await withWaiting({ fixture in
            fixture.write("等待期间的新复制")
        }, after: { fixture in
            try appRequire(fixture.text == "等待期间的新复制")
        })
    }

    func changedApplication() async throws {
        let fixture = ClipboardFixture()
        defer { fixture.close() }
        fixture.coordinator.begin()
        fixture.environment.current.applicationPID = 424_243
        _ = await fixture.coordinator.deliver("转写", automatic: true)
        try appRequire(fixture.environment.events == 0 && fixture.text == "原始剪贴板")
    }

    func changedFocus() async throws {
        let fixture = ClipboardFixture()
        defer { fixture.close() }
        fixture.coordinator.begin()
        fixture.environment.current.focus = AXUIElementCreateApplication(424_243)
        _ = await fixture.coordinator.deliver("转写", automatic: true)
        try appRequire(fixture.environment.events == 0 && fixture.text == "原始剪贴板")
    }

    func unknownFocus() async throws {
        let fixture = ClipboardFixture()
        defer { fixture.close() }
        fixture.environment.current.focus = nil
        fixture.coordinator.begin()
        _ = await fixture.coordinator.deliver("转写", automatic: true)
        try appRequire(fixture.environment.events == 0 && fixture.text == "原始剪贴板")
    }

    func secureInput() async throws {
        for sensitiveAtStart in [true, false] {
            let fixture = ClipboardFixture()
            defer { fixture.close() }
            fixture.environment.current.sensitiveInput = sensitiveAtStart
            fixture.coordinator.begin()
            fixture.environment.current.sensitiveInput = !sensitiveAtStart
            _ = await fixture.coordinator.deliver("转写", automatic: true)
            try appRequire(fixture.environment.events == 0 && fixture.text == "原始剪贴板",
                           "secure input at either end must prevent injection")
        }
    }

    func permissions() async throws {
        for automatic in [true, false] {
            let fixture = ClipboardFixture()
            defer { fixture.close() }
            fixture.environment.current.accessibility = !automatic
            fixture.coordinator.begin()
            _ = await fixture.coordinator.deliver("转写", automatic: automatic)
            try appRequire(fixture.environment.events == 0 && fixture.text == "原始剪贴板")
        }
    }

    func oversizedClipboard() async throws {
        let fixture = ClipboardFixture()
        defer { fixture.close() }
        let type = NSPasteboard.PasteboardType("io.voiceport.checks.large")
        let data = Data(repeating: 7, count: 4_000_001)
        fixture.board.clearContents()
        try appRequire(fixture.board.setData(data, forType: type))
        let originalCount = fixture.board.changeCount
        fixture.coordinator.begin()
        _ = await fixture.coordinator.deliver("转写", automatic: true)
        try appRequire(fixture.environment.events == 0 && fixture.board.changeCount == originalCount)
        try appRequire(fixture.board.data(forType: type) == data)
    }

    func failedEvent() async throws {
        for newCopy in [false, true] {
            let fixture = ClipboardFixture()
            defer { fixture.close() }
            fixture.environment.postSucceeds = false
            if newCopy { fixture.environment.afterPost = { fixture.write("按键失败时的新复制") } }
            fixture.coordinator.begin()
            let message = await fixture.coordinator.deliver("转写", automatic: true)
            try appRequire(fixture.text == (newCopy ? "按键失败时的新复制" : "原始剪贴板"))
            try appRequire(message.contains("无法发送"))
        }
    }

    func shutdownRestores() async throws {
        try await withWaiting({ fixture in
            try appRequire(fixture.text == "测试转写")
            fixture.coordinator.shutdown()
            try appRequire(fixture.text == "原始剪贴板", "shutdown must restore before the event loop exits")
        })
    }

    func shutdownPreserves() async throws {
        try await withWaiting({ fixture in
            fixture.write("别的应用新复制")
            fixture.coordinator.shutdown()
            try appRequire(fixture.text == "别的应用新复制")
        }, after: { fixture in
            try appRequire(fixture.text == "别的应用新复制")
        })
    }

    func lateCompletion() async throws {
        try await withWaiting({ fixture in
            fixture.coordinator.shutdown()
            fixture.write("退出后的新复制")
            fixture.coordinator.shutdown()
            fixture.coordinator.begin()
            try appRequire(!fixture.coordinator.copy("退出后的手动复制"))
            _ = await fixture.coordinator.deliver("退出后的旧投递", automatic: true)
        }, after: { fixture in
            try appRequire(fixture.text == "退出后的新复制" && fixture.environment.events == 1)
        })
    }

    func targetRace() async throws {
        let fixture = ClipboardFixture()
        defer { fixture.close() }
        fixture.coordinator.begin()
        fixture.environment.afterRead = { reads in
            if reads == 2 { fixture.environment.current.applicationPID = 424_243 }
        }
        _ = await fixture.coordinator.deliver("转写", automatic: true)
        try appRequire(fixture.environment.events == 0, "a focus/app change before key posting must be checked again")
        try appRequire(fixture.text == "原始剪贴板")
    }

    func nonEditable() async throws {
        let fixture = ClipboardFixture()
        defer { fixture.close() }
        fixture.environment.current.editable = false
        fixture.coordinator.begin()
        _ = await fixture.coordinator.deliver("转写", automatic: true)
        try appRequire(fixture.environment.events == 0 && fixture.text == "原始剪贴板")
    }

    func emptyClipboard() async throws {
        let fixture = ClipboardFixture()
        defer { fixture.close() }
        fixture.board.clearContents()
        fixture.coordinator.begin()
        _ = await fixture.coordinator.deliver("转写", automatic: true)
        try appRequire(fixture.text == nil && (fixture.board.pasteboardItems ?? []).isEmpty)
    }

    func cancelledDelivery() async throws {
        for cancellationRead in [0, 2, 3] {
            let fixture = ClipboardFixture()
            defer { fixture.close() }
            fixture.coordinator.begin()
            fixture.environment.afterRead = { read in
                if read == cancellationRead { withUnsafeCurrentTask { $0?.cancel() } }
            }
            let delivery = Task { await fixture.coordinator.deliver("已取消文字", automatic: true) }
            if cancellationRead == 0 { delivery.cancel() }
            _ = await delivery.value
            try appRequire(fixture.environment.events == 0 && fixture.text == "原始剪贴板",
                           "cancellation at read \(cancellationRead) must prevent key posting and release ownership")
        }
    }

    func interruptedWait() async throws {
        for newCopy in [false, true] {
            try await withWaiting({ fixture in
                if newCopy { fixture.write("中断前的新复制") }
                fixture.delay.release(throwing: CancellationError())
            }, after: { fixture in
                try appRequire(fixture.text == (newCopy ? "中断前的新复制" : "原始剪贴板"))
            })
        }
    }

    func overlappingDelivery() async throws {
        try await withWaiting({ fixture in
            fixture.coordinator.begin()
            fixture.coordinator.clear()
            try appRequire(!fixture.coordinator.copy("不应覆盖临时文字"))
            _ = await fixture.coordinator.deliver("第二次投递", automatic: true)
            try appRequire(fixture.environment.events == 1 && fixture.text == "测试转写")
        }, after: { fixture in
            try appRequire(fixture.text == "原始剪贴板")
            try appRequire(fixture.coordinator.copy("手动复制\n保留原文"))
            try appRequire(fixture.text == "手动复制\n保留原文")
        })
    }

    func finalSafetyGuards() async throws {
        let changes: [(PasteEnvironmentFixture) -> Void] = [
            { $0.current.sensitiveInput = true },
            { $0.current.accessibility = false },
            { $0.current.editable = false },
            { $0.current.focus = AXUIElementCreateApplication(424_243) }
        ]
        for change in changes {
            let fixture = ClipboardFixture()
            defer { fixture.close() }
            fixture.coordinator.begin()
            fixture.environment.afterRead = { read in
                if read == 2 { change(fixture.environment) }
            }
            _ = await fixture.coordinator.deliver("转写", automatic: true)
            try appRequire(fixture.environment.events == 0 && fixture.text == "原始剪贴板")
        }
    }

    func clipboardDuringValidation() async throws {
        for copyRead in [2, 3] {
            let fixture = ClipboardFixture()
            defer { fixture.close() }
            fixture.coordinator.begin()
            fixture.environment.afterRead = { read in
                if read == copyRead { fixture.write("目标复核时的新复制") }
            }
            _ = await fixture.coordinator.deliver("转写", automatic: true)
            try appRequire(fixture.environment.events == 0 && fixture.text == "目标复核时的新复制")
        }
    }

    private func withWaiting(_ action: (ClipboardFixture) async throws -> Void,
                             after: (ClipboardFixture) throws -> Void = { _ in }) async throws {
        let fixture = ClipboardFixture(hold: true)
        defer { fixture.close() }
        fixture.coordinator.begin()
        let delivery = Task { await fixture.coordinator.deliver("测试转写", automatic: true) }
        do {
            try await eventually { fixture.delay.waiting }
            try await action(fixture)
        } catch {
            fixture.delay.release()
            _ = await delivery.value
            throw error
        }
        fixture.delay.release()
        _ = await delivery.value
        try after(fixture)
    }
}

@MainActor
private final class ClipboardFixture {
    let board: NSPasteboard
    let environment: PasteEnvironmentFixture
    let delay: PasteDelayFixture
    let coordinator: PasteCoordinator
    var text: String? { board.string(forType: .string) }

    init(hold: Bool = false) {
        let board = NSPasteboard.withUniqueName()
        let environment = PasteEnvironmentFixture()
        let delay = PasteDelayFixture()
        self.board = board
        self.environment = environment
        self.delay = delay
        coordinator = PasteCoordinator(board: board, readTarget: { environment.read() }, postPaste: {
            environment.events += 1
            environment.pastedTexts.append(board.string(forType: .string) ?? "")
            environment.afterPost?()
            return environment.postSucceeds
        }, waitForPaste: {
            if hold { try await delay.wait() }
        })
        write("原始剪贴板")
    }
    func write(_ text: String) {
        board.clearContents()
        board.setString(text, forType: .string)
    }
    func close() {
        coordinator.shutdown()
        delay.release()
        environment.afterRead = nil
        environment.afterPost = nil
        board.releaseGlobally()
    }
}

@MainActor
private final class PasteEnvironmentFixture {
    var current = PasteTargetState(applicationPID: 424_242, focus: AXUIElementCreateApplication(424_242),
                                   accessibility: true, sensitiveInput: false, editable: true)
    var events = 0
    var pastedTexts: [String] = []
    var postSucceeds = true
    var afterRead: ((Int) -> Void)?
    var afterPost: (() -> Void)?
    private var reads = 0
    func read() -> PasteTargetState {
        reads += 1
        let captured = current
        afterRead?(reads)
        return captured
    }
}

@MainActor
private final class PasteDelayFixture {
    private var continuation: CheckedContinuation<Void, Error>?
    var waiting: Bool { continuation != nil }
    func wait() async throws { try await withCheckedThrowingContinuation { continuation = $0 } }
    func release(throwing error: Error? = nil) {
        if let error { continuation?.resume(throwing: error) }
        else { continuation?.resume() }
        continuation = nil
    }
}
