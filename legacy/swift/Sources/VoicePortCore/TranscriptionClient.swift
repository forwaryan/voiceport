import Foundation

final class RejectRedirects: NSObject, URLSessionTaskDelegate, Sendable {
    func urlSession(_ session: URLSession, task: URLSessionTask,
                    willPerformHTTPRedirection response: HTTPURLResponse,
                    newRequest request: URLRequest,
                    completionHandler: @escaping @Sendable (URLRequest?) -> Void) {
        completionHandler(nil)
    }
}

public struct TranscriptionClient: Sendable {
    public static let maximumAudioBytes = 20_000_000
    static let endpoint = URL(string: "https://chatgpt.com/backend-api/transcribe")!
    private let session: URLSession
    private let credentials: @Sendable () throws -> CodexCredentials

    public init(authFile: URL = CodexCredentials.defaultFile()) {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.timeoutIntervalForRequest = 90
        configuration.timeoutIntervalForResource = 110
        configuration.httpShouldSetCookies = false
        configuration.httpCookieStorage = nil
        configuration.urlCredentialStorage = nil
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        self.session = URLSession(configuration: configuration, delegate: RejectRedirects(), delegateQueue: nil)
        self.credentials = { try CodexCredentials.load(from: authFile) }
    }

    // Test transport injection is internal; production always uses the fixed HTTPS host and redirect policy.
    init(session: URLSession, credentials: @escaping @Sendable () throws -> CodexCredentials) {
        self.session = session
        self.credentials = credentials
    }

    public func transcribe(file: URL, language: String? = nil) async throws -> String {
        try Task.checkCancellation()
        let request = try Self.makeRequest(file: file, language: language, credentials: credentials())
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: request)
        } catch let error as URLError {
            if error.code == .cancelled { throw CancellationError() }
            throw error.code == .timedOut ? VoicePortError.timedOut : VoicePortError.network
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            throw VoicePortError.network
        }
        try Task.checkCancellation()
        guard let http = response as? HTTPURLResponse else { throw VoicePortError.invalidResponse }
        return try Self.decodeResponse(data, status: http.statusCode)
    }

    static func makeRequest(file: URL, language: String?, credentials: CodexCredentials) throws -> URLRequest {
        let mimeTypes = ["wav": "audio/wav", "m4a": "audio/mp4", "mp4": "audio/mp4", "mp3": "audio/mpeg",
                         "webm": "audio/webm", "ogg": "audio/ogg", "flac": "audio/flac"]
        let ext = file.pathExtension.lowercased()
        guard let mime = mimeTypes[ext],
              let attributes = try? FileManager.default.attributesOfItem(atPath: file.path),
              attributes[.type] as? FileAttributeType == .typeRegular,
              let size = attributes[.size] as? NSNumber, size.intValue > 0 else {
            throw VoicePortError.invalidAudio
        }
        guard size.intValue <= maximumAudioBytes else { throw VoicePortError.audioTooLarge }
        let normalizedLanguage = language?.trimmingCharacters(in: .whitespacesAndNewlines)
        if let value = normalizedLanguage, !value.isEmpty,
           value.range(of: "^[A-Za-z]{2,3}(-[A-Za-z0-9]{2,8})?$", options: .regularExpression) == nil {
            throw VoicePortError.invalidLanguage
        }
        guard let audio = try? Data(contentsOf: file), !audio.isEmpty else { throw VoicePortError.invalidAudio }
        // Check again after reading: another process could have changed the file after stat.
        guard audio.count <= maximumAudioBytes else { throw VoicePortError.audioTooLarge }
        let boundary = "VoicePort-\(UUID().uuidString)"
        var body = Data()
        func append(_ text: String) { body.append(Data(text.utf8)) }
        append("--\(boundary)\r\nContent-Disposition: form-data; name=\"file\"; filename=\"dictation.\(ext)\"\r\n")
        append("Content-Type: \(mime)\r\n\r\n")
        body.append(audio)
        append("\r\n")
        if let value = normalizedLanguage, !value.isEmpty {
            append("--\(boundary)\r\nContent-Disposition: form-data; name=\"language\"\r\n\r\n\(value)\r\n")
        }
        append("--\(boundary)--\r\n")
        var request = URLRequest(url: endpoint)
        request.httpMethod = "POST"
        request.setValue("Bearer \(credentials.accessToken)", forHTTPHeaderField: "Authorization")
        request.setValue(credentials.accountID, forHTTPHeaderField: "ChatGPT-Account-Id")
        request.setValue("codex_dictation", forHTTPHeaderField: "originator")
        request.setValue("VoicePort/0.1", forHTTPHeaderField: "User-Agent")
        request.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        request.httpBody = body
        return request
    }

    static func decodeResponse(_ data: Data, status: Int) throws -> String {
        guard status == 200 else { throw VoicePortError.service(status) }
        struct Response: Decodable { let text: String }
        guard let response = try? JSONDecoder().decode(Response.self, from: data) else {
            throw VoicePortError.invalidResponse
        }
        guard !response.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            throw VoicePortError.emptyTranscript
        }
        return response.text
    }
}
