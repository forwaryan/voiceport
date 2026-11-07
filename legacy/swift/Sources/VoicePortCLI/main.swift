import Foundation
import VoicePortCore

@main
struct VoicePortCLI {
    static let help = """
    VoicePort — macOS 全局语音输入的转写核心

    voiceport-cli demo
        离线演示，不录音、不读取登录、不访问网络。
    voiceport-cli doctor
        只报告登录文件存在性，不读取或输出凭据。
    voiceport-cli transcribe AUDIO [--language zh] [--auth /path/to/auth.json]
        使用自己的 Codex ChatGPT 登录上传并转写音频。
        未指定 --auth 时使用 CODEX_HOME/auth.json 或 ~/.codex/auth.json。
    voiceport-cli --help

    云端接口未公开承诺稳定性与用量规则。转写只输出文字，不自动发送消息。
    """

    static func main() async {
        do {
            try await run(Array(CommandLine.arguments.dropFirst()))
        } catch {
            let message = (error as? LocalizedError)?.errorDescription ?? "命令执行失败。"
            FileHandle.standardError.write(Data("VoicePort: \(message)\n".utf8))
            exit(1)
        }
    }

    static func run(_ arguments: [String]) async throws {
        guard let command = arguments.first else { print(help); return }
        switch command {
        case "--help", "-h", "help":
            guard arguments.count == 1 else { throw UsageError() }
            print(help)
        case "demo":
            guard arguments.count == 1 else { throw UsageError() }
            print("[离线模拟，未调用云端] 请在 Orca 中实现重试逻辑，保留 requestID，不要自动发送。")
        case "doctor":
            guard arguments.count == 1 else { throw UsageError() }
            let file = CodexCredentials.defaultFile()
            let exists = FileManager.default.fileExists(atPath: file.path)
            print("系统：\(ProcessInfo.processInfo.operatingSystemVersionString)")
            print("Codex 登录文件：\(exists ? "存在（未检查有效性）" : "未找到，请运行 codex login")")
            print("首版支持 ChatGPT 文件登录；菜单栏录音/粘贴需 macOS 麦克风/辅助功能权限。")
        case "transcribe":
            guard arguments.count >= 2, !arguments[1].hasPrefix("--") else { throw UsageError() }
            var language: String?
            var auth = CodexCredentials.defaultFile()
            var seen = Set<String>()
            var index = 2
            while index < arguments.count {
                let flag = arguments[index]
                guard ["--language", "--auth"].contains(flag), seen.insert(flag).inserted,
                      index + 1 < arguments.count, !arguments[index + 1].isEmpty,
                      !arguments[index + 1].hasPrefix("--") else { throw UsageError() }
                if flag == "--language" { language = arguments[index + 1] }
                else { auth = URL(fileURLWithPath: arguments[index + 1]) }
                index += 2
            }
            let text = try await TranscriptionClient(authFile: auth).transcribe(
                file: URL(fileURLWithPath: arguments[1]), language: language)
            print(text)
        default:
            throw UsageError()
        }
    }

    struct UsageError: LocalizedError {
        var errorDescription: String? { "参数无效。运行 voiceport-cli --help 查看用法。" }
    }
}
