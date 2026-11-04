import Foundation

struct AppCheckFailure: Error, CustomStringConvertible {
    let description: String
}

func appRequire(_ value: @autoclosure () -> Bool, _ message: String = "condition failed",
                file: StaticString = #fileID, line: UInt = #line) throws {
    if !value() { throw AppCheckFailure(description: "\(file):\(line): \(message)") }
}

@MainActor
func eventually(_ condition: @MainActor () -> Bool,
                file: StaticString = #fileID, line: UInt = #line) async throws {
    for _ in 0..<1_000 {
        if condition() { return }
        try await Task.sleep(for: .milliseconds(1))
    }
    throw AppCheckFailure(description: "\(file):\(line): expected state not reached")
}

@main
struct AppChecks {
    @MainActor
    static func main() async {
        if let code = RecordingStorageTests.childExitCode(arguments: CommandLine.arguments) { exit(code) }
        let tests = ControllerTests().tests + ClipboardTests().tests + TargetCaptureTests().tests + RecordingStorageTests().tests
        var failures = 0
        for (name, test) in tests {
            do { try await test(); print("PASS \(name)") }
            catch { failures += 1; print("FAIL \(name): \(error)") }
        }
        print("\(tests.count) app checks, \(failures) failures (fake audio/permission/network/keystrokes; isolated test pasteboards)")
        if failures > 0 { exit(1) }
    }
}
