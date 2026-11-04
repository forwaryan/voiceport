import Foundation

struct CredentialsTests {
    var tests: [(String, () throws -> Void)] {
        [("ChatGPT file login", chatGPTLogin), ("legacy file login", legacyLogin),
         ("API key and malformed login rejected", malformedLogin), ("header injection rejected", headerInjection),
         ("expired JWT rejected", expiredJWT), ("valid JWT accepted", validJWT),
         ("missing file actionable", missingFile), ("credentials descriptions redacted", redacted),
         ("custom Codex home respected", customHome)]
    }

    private func auth(token: String = "fixture-access", account: String = "fixture-account", mode: String? = "chatgpt") throws -> Data {
        var object: [String: Any] = ["tokens": ["access_token": token, "account_id": account]]
        if let mode { object["auth_mode"] = mode }
        return try JSONSerialization.data(withJSONObject: object)
    }

    func chatGPTLogin() throws {
        let credentials = try CodexCredentials.decode(auth())
        try equal(credentials.accountID, "fixture-account")
        try equal(credentials.accessToken, "fixture-access")
    }
    func legacyLogin() throws { _ = try CodexCredentials.decode(auth(mode: nil)) }
    func malformedLogin() throws {
        for data in [Data("not-json".utf8), Data("{\"OPENAI_API_KEY\":\"fixture-key\"}".utf8), try auth(mode: "apikey")] {
            try raises(.invalidCredentials) { _ = try CodexCredentials.decode(data) }
        }
    }
    func headerInjection() throws {
        for token in ["", " ", "fixture\r\nInjected: value", "non-ascii-令牌"] {
            try raises(.invalidCredentials) { _ = try CodexCredentials.decode(auth(token: token)) }
        }
        try raises(.invalidCredentials) { _ = try CodexCredentials.decode(auth(account: "\n")) }
    }
    func expiredJWT() throws {
        let payload = Data("{\"exp\":1000}".utf8).base64EncodedString()
        try raises(.expiredCredentials) {
            _ = try CodexCredentials.decode(auth(token: "header.\(payload).signature"), now: Date(timeIntervalSince1970: 2000))
        }
    }
    func validJWT() throws {
        let payload = Data("{\"exp\":5000}".utf8).base64EncodedString()
        let result = try CodexCredentials.decode(auth(token: "header.\(payload).signature"), now: Date(timeIntervalSince1970: 2000))
        try equal(result.expiresAt, Date(timeIntervalSince1970: 5000))
    }
    func missingFile() throws {
        try raises(.signInRequired) {
            _ = try CodexCredentials.load(from: URL(fileURLWithPath: "/nonexistent/voiceport-fixture/auth.json"))
        }
    }
    func redacted() throws {
        let credentials = try CodexCredentials.decode(auth())
        try require(!String(describing: credentials).contains("fixture-access"))
        try require(!String(reflecting: credentials).contains("fixture-account"))
    }
    func customHome() throws {
        try equal(CodexCredentials.defaultFile(environment: ["CODEX_HOME": "/tmp/voiceport-fixture"]).path, "/tmp/voiceport-fixture/auth.json")
        try require(CodexCredentials.defaultFile(environment: [:]).path.hasSuffix("/.codex/auth.json"))
    }
}
