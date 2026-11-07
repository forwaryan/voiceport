import Foundation

struct TransportTests {
    var tests: [(String, () async throws -> Void)] {
        [("URLSession upload returns transcript", upload), ("401 does not repeat audio upload", noAutomaticRetry),
         ("timeout is sanitized", timeout), ("cancel before upload does no network I/O", cancelled),
         ("credentials reloaded for every request", reloadCredentials)]
    }
    private let fixture = CodexCredentials(accessToken: "fixture-token", accountID: "fixture-account", expiresAt: nil)

    private func withClient(_ action: TestURLProtocol.Action = .response(200, Data("{\"text\":\"保留 requestID\"}".utf8)),
                            run: (TranscriptionClient, URL) async throws -> Void) async throws {
        TestURLProtocol.store.reset(action)
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [TestURLProtocol.self]
        let session = URLSession(configuration: configuration)
        defer { session.invalidateAndCancel() }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appendingPathComponent("fixture.m4a")
        try Data([1, 2, 3]).write(to: file)
        let credentials = fixture
        let client = TranscriptionClient(session: session, credentials: {
            TestURLProtocol.store.countCredentialsRead()
            return credentials
        })
        try await run(client, file)
    }

    func upload() async throws {
        try await withClient { client, file in
            let text = try await client.transcribe(file: file, language: "zh")
            try equal(text, "保留 requestID")
            try equal(TestURLProtocol.store.requestCount, 1)
        }
    }
    func noAutomaticRetry() async throws {
        try await withClient(.response(401, Data("private error".utf8))) { client, file in
            try await raisesAsync(.service(401)) { _ = try await client.transcribe(file: file) }
            try equal(TestURLProtocol.store.requestCount, 1)
        }
    }
    func timeout() async throws {
        try await withClient(.failure(.timedOut)) { client, file in
            try await raisesAsync(.timedOut) { _ = try await client.transcribe(file: file) }
            try equal(TestURLProtocol.store.requestCount, 1)
        }
    }
    func cancelled() async throws {
        try await withClient { client, file in
            let task = Task {
                withUnsafeCurrentTask { $0?.cancel() }
                return try await client.transcribe(file: file)
            }
            do { _ = try await task.value; throw CheckFailure(description: "expected cancellation") }
            catch is CancellationError { }
            try equal(TestURLProtocol.store.requestCount, 0)
        }
    }
    func reloadCredentials() async throws {
        try await withClient { client, file in
            _ = try await client.transcribe(file: file)
            _ = try await client.transcribe(file: file)
            try equal(TestURLProtocol.store.credentialReads, 2)
            try equal(TestURLProtocol.store.requestCount, 2)
        }
    }
}

private func raisesAsync(_ expected: VoicePortError, _ operation: () async throws -> Void) async throws {
    do { try await operation() }
    catch let error as CheckFailure { throw error }
    catch { try equal(error as? VoicePortError, expected); return }
    throw CheckFailure(description: "expected \(expected)")
}

private final class TestURLProtocol: URLProtocol, @unchecked Sendable {
    enum Action: Sendable { case response(Int, Data), failure(URLError.Code) }
    final class Store: @unchecked Sendable {
        private let lock = NSLock()
        private var action: Action = .response(200, Data())
        private var requests = 0
        private var reads = 0
        var requestCount: Int { lock.withLock { requests } }
        var credentialReads: Int { lock.withLock { reads } }
        func reset(_ action: Action) { lock.withLock { self.action = action; requests = 0; reads = 0 } }
        func countCredentialsRead() { lock.withLock { reads += 1 } }
        func next() -> Action { lock.withLock { requests += 1; return action } }
    }
    static let store = Store()
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        switch Self.store.next() {
        case .response(let status, let data):
            let response = HTTPURLResponse(url: request.url!, statusCode: status, httpVersion: nil, headerFields: nil)!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        case .failure(let code):
            client?.urlProtocol(self, didFailWithError: URLError(code))
        }
    }
    override func stopLoading() {}
}
