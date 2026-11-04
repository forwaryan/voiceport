import Foundation

/// Read-only view of a Codex file login. Never persists or refreshes credentials.
public struct CodexCredentials: Sendable, CustomStringConvertible, CustomDebugStringConvertible {
    let accessToken: String
    let accountID: String
    let expiresAt: Date?

    public var description: String { "CodexCredentials(<redacted>)" }
    public var debugDescription: String { description }

    public static func defaultFile(environment: [String: String] = ProcessInfo.processInfo.environment) -> URL {
        let directory = environment["CODEX_HOME"].flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0) }
            ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".codex", isDirectory: true)
        return directory.appendingPathComponent("auth.json")
    }

    public static func load(from file: URL = defaultFile(), now: Date = Date()) throws -> Self {
        guard FileManager.default.fileExists(atPath: file.path) else { throw VoicePortError.signInRequired }
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: file.path),
              let size = attributes[.size] as? NSNumber, size.intValue <= 1_000_000,
              let data = try? Data(contentsOf: file) else { throw VoicePortError.invalidCredentials }
        return try decode(data, now: now)
    }

    static func decode(_ data: Data, now: Date = Date()) throws -> Self {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              object["auth_mode"] == nil || object["auth_mode"] as? String == "chatgpt",
              let tokens = object["tokens"] as? [String: Any],
              let token = tokens["access_token"] as? String,
              let account = tokens["account_id"] as? String,
              validHeaderValue(token), validHeaderValue(account) else {
            throw VoicePortError.invalidCredentials
        }
        let expiration = expirationDate(token)
        if let expiration, expiration.timeIntervalSince(now) <= 30 {
            throw VoicePortError.expiredCredentials
        }
        return Self(accessToken: token, accountID: account, expiresAt: expiration)
    }

    private static func validHeaderValue(_ value: String) -> Bool {
        !value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            && value.unicodeScalars.allSatisfy { $0.value >= 0x21 && $0.value <= 0x7E }
    }

    private static func expirationDate(_ token: String) -> Date? {
        let segments = token.split(separator: ".", omittingEmptySubsequences: false)
        guard segments.count == 3 else { return nil }
        var encoded = String(segments[1]).replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/")
        encoded += String(repeating: "=", count: (4 - encoded.count % 4) % 4)
        guard let data = Data(base64Encoded: encoded),
              let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let expiry = object["exp"] as? Double else { return nil }
        return Date(timeIntervalSince1970: expiry)
    }
}
