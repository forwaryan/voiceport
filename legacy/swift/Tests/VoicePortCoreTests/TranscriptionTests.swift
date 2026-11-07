import Foundation

struct TranscriptionTests {
    private let credentials = CodexCredentials(accessToken: "fixture-token", accountID: "fixture-account", expiresAt: nil)
    var tests: [(String, () throws -> Void)] {
        [("multipart and binary contract", multipart), ("automatic language omitted", automaticLanguage),
         ("invalid and oversized audio rejected", invalidAudio), ("language injection rejected", languageInjection),
         ("original transcript preserved", originalText), ("empty and malformed responses rejected", malformedResponse),
         ("HTTP status errors sanitized", httpFailures), ("redirect destination rejected", rejectRedirect)]
    }
    private func withAudio(_ data: Data = Data([0, 1, 255, 10]), ext: String = "m4a", run: (URL) throws -> Void) throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appendingPathComponent("user-secret-filename.\(ext)")
        try data.write(to: file)
        try run(file)
    }
    func multipart() throws {
        let audio = Data([0, 255, 1, 13, 10])
        try withAudio(audio) { file in
            let request = try TranscriptionClient.makeRequest(file: file, language: "zh", credentials: credentials)
            try equal(request.url?.absoluteString, "https://chatgpt.com/backend-api/transcribe")
            try equal(request.httpMethod, "POST")
            try equal(request.value(forHTTPHeaderField: "Authorization"), "Bearer fixture-token")
            try equal(request.value(forHTTPHeaderField: "ChatGPT-Account-Id"), "fixture-account")
            try equal(request.value(forHTTPHeaderField: "originator"), "codex_dictation")
            try require(request.value(forHTTPHeaderField: "Cookie") == nil)
            guard let body = request.httpBody else { throw CheckFailure(description: "missing multipart body") }
            try require(body.range(of: audio) != nil)
            let text = String(decoding: body, as: UTF8.self)
            try require(text.contains("name=\"language\"\r\n\r\nzh\r\n"))
            try require(text.contains("filename=\"dictation.m4a\""))
            try require(!text.contains("user-secret-filename"))
            guard let contentType = request.value(forHTTPHeaderField: "Content-Type"),
                  let boundary = contentType.components(separatedBy: "boundary=").last else {
                throw CheckFailure(description: "missing content type")
            }
            try require(text.hasSuffix("--\(boundary)--\r\n"))
        }
    }
    func automaticLanguage() throws {
        try withAudio { file in
            let request = try TranscriptionClient.makeRequest(file: file, language: nil, credentials: credentials)
            try require(!String(decoding: request.httpBody!, as: UTF8.self).contains("name=\"language\""))
        }
    }
    func invalidAudio() throws {
        try withAudio(Data()) { file in
            try raises(.invalidAudio) { _ = try TranscriptionClient.makeRequest(file: file, language: nil, credentials: credentials) }
        }
        try withAudio(ext: "txt") { file in
            try raises(.invalidAudio) { _ = try TranscriptionClient.makeRequest(file: file, language: nil, credentials: credentials) }
        }
        try withAudio(Data(repeating: 0, count: TranscriptionClient.maximumAudioBytes + 1)) { file in
            try raises(.audioTooLarge) { _ = try TranscriptionClient.makeRequest(file: file, language: nil, credentials: credentials) }
        }
    }
    func languageInjection() throws {
        try withAudio { file in
            try raises(.invalidLanguage) { _ = try TranscriptionClient.makeRequest(file: file, language: "zh\r\n--boundary", credentials: credentials) }
        }
    }
    func originalText() throws {
        let text = "保留 requestID，不要重试。\n下一行。"
        try equal(TranscriptionClient.decodeResponse(JSONSerialization.data(withJSONObject: ["text": text]), status: 200), text)
    }
    func malformedResponse() throws {
        for body in ["not-json", "{}", "{\"text\":7}"] {
            try raises(.invalidResponse) { _ = try TranscriptionClient.decodeResponse(Data(body.utf8), status: 200) }
        }
        try raises(.emptyTranscript) { _ = try TranscriptionClient.decodeResponse(Data("{\"text\":\" \\n\"}".utf8), status: 200) }
    }
    func httpFailures() throws {
        for status in [301, 302, 401, 403, 429, 500, 503] {
            try raises(.service(status)) { _ = try TranscriptionClient.decodeResponse(Data("fixture-private-error".utf8), status: status) }
            try require(!VoicePortError.service(status).localizedDescription.contains("fixture-private"))
        }
    }
    func rejectRedirect() throws {
        let session = URLSession(configuration: .ephemeral)
        defer { session.invalidateAndCancel() }
        let task = session.dataTask(with: TranscriptionClient.endpoint)
        let response = HTTPURLResponse(url: TranscriptionClient.endpoint, statusCode: 302, httpVersion: nil, headerFields: nil)!
        let result = RedirectResult()
        RejectRedirects().urlSession(session, task: task, willPerformHTTPRedirection: response,
                                    newRequest: URLRequest(url: URL(string: "https://example.invalid/redirect")!)) { request in
            result.store(request)
        }
        try require(result.called && result.rejected)
    }
}

private final class RedirectResult: @unchecked Sendable {
    private let lock = NSLock()
    private var value: (Bool, Bool) = (false, false)
    var called: Bool { lock.withLock { value.0 } }
    var rejected: Bool { lock.withLock { value.1 } }
    func store(_ request: URLRequest?) { lock.withLock { value = (true, request == nil) } }
}
