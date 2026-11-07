import Foundation

public enum VoicePortError: Error, LocalizedError, Equatable, Sendable {
    case signInRequired
    case invalidCredentials
    case expiredCredentials
    case invalidAudio
    case audioTooLarge
    case invalidLanguage
    case service(Int)
    case invalidResponse
    case emptyTranscript
    case network
    case timedOut

    public var errorDescription: String? {
        switch self {
        case .signInRequired:
            "未找到可用的 Codex ChatGPT 登录。请先运行 codex login；首版需文件形式的 auth.json。"
        case .invalidCredentials:
            "登录文件格式无效或不是 ChatGPT 登录。请用 Codex 重新登录。"
        case .expiredCredentials:
            "Codex 登录已到期，请在 Codex 中刷新或重新登录后重试。"
        case .invalidAudio:
            "请选择非空的 m4a、mp4、wav、mp3、webm、ogg 或 flac 音频文件。"
        case .audioTooLarge:
            "录音超过 20 MB，请缩短录音后重试。"
        case .invalidLanguage:
            "语言请留空自动识别，或使用 zh、en、zh-CN 等语言代码。"
        case .service(401):
            "登录已失效（401）。请在 Codex 中重新登录后重试。"
        case .service(403):
            "账号或服务拒绝了转写（403），请确认账号有权使用听写。"
        case .service(429):
            "转写暂时受限（429）。请稍后手动重试；当前接口用量规则未知。"
        case .service(let status):
            "转写服务返回 HTTP \(status)，请稍后重试。"
        case .invalidResponse:
            "转写服务返回了无法识别的响应。接口可能已变化。"
        case .emptyTranscript:
            "没有识别到文字，请确认麦克风和录音内容。"
        case .network:
            "无法连接转写服务，请检查网络后手动重试。"
        case .timedOut:
            "转写请求超时，音频可在本次会话内手动重试。"
        }
    }
}
