import Foundation

/// OS boundaries are injected so controller checks never request a real microphone or touch the user's clipboard.
@MainActor
protocol AudioRecording: AnyObject {
    func start() throws -> URL
    func stop()
    func cancel()
}

@MainActor
protocol TextDelivery: AnyObject {
    func begin()
    func clear()
    func deliver(_ text: String, automatic: Bool) async -> String
    func copy(_ text: String) -> Bool
    func requestAccessibility()
    func shutdown()
}
