import Foundation

// A small throwing assertion harness for Command Line Tools, which ships neither XCTest nor Testing.
// Core sources and checks are compiled into one test executable, leaving the public API unchanged.
struct CheckFailure: Error, CustomStringConvertible {
    let description: String
}

func require(_ condition: @autoclosure () throws -> Bool, _ message: String = "condition failed",
             file: StaticString = #fileID, line: UInt = #line) throws {
    if try !condition() { throw CheckFailure(description: "\(file):\(line): \(message)") }
}

func equal<T: Equatable>(_ actual: @autoclosure () throws -> T, _ expected: T,
                         file: StaticString = #fileID, line: UInt = #line) throws {
    let value = try actual()
    try require(value == expected, "expected \(expected), got \(value)", file: file, line: line)
}

func raises(_ expected: VoicePortError? = nil, _ operation: () throws -> Void,
            file: StaticString = #fileID, line: UInt = #line) throws {
    do { try operation() }
    catch let error as CheckFailure { throw error }
    catch {
        if let expected { try equal(error as? VoicePortError, expected, file: file, line: line) }
        return
    }
    throw CheckFailure(description: "\(file):\(line): expected an error")
}

@main
struct CoreChecks {
    static func main() async {
        let suites = CredentialsTests().tests + TranscriptionTests().tests + DeliveryTests().tests
        var failures = 0
        for (name, test) in suites {
            do {
                try test()
                print("PASS \(name)")
            } catch {
                failures += 1
                print("FAIL \(name): \(error)")
            }
        }
        let asynchronous = TransportTests().tests
        for (name, test) in asynchronous {
            do {
                try await test()
                print("PASS \(name)")
            } catch {
                failures += 1
                print("FAIL \(name): \(error)")
            }
        }
        print("\(suites.count + asynchronous.count) checks, \(failures) failures (offline, no credentials or network)")
        if failures > 0 { exit(1) }
    }
}
