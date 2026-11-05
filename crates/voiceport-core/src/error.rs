use std::fmt;

pub type Result<T> = std::result::Result<T, Error>;

/// Intentionally stores no I/O, JSON, reqwest error, URL, headers or body: all
/// those sources can contain credentials or private upstream response text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    SignInRequired,
    InvalidCredentials,
    ExpiredCredentials,
    InvalidAudio,
    AudioTooLarge,
    InvalidLanguage,
    Service(u16),
    InvalidResponse,
    EmptyTranscript,
    Network,
    TimedOut,
    Cancelled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let text = match self {
            Self::SignInRequired => {
                "未找到可用的 Codex ChatGPT 登录。请先运行 codex login；需要文件形式的 auth.json。"
            }
            Self::InvalidCredentials => {
                "登录文件格式无效或不是 ChatGPT 登录。请用 Codex 重新登录。"
            }
            Self::ExpiredCredentials => "Codex 登录已到期，请在 Codex 中刷新或重新登录后重试。",
            Self::InvalidAudio => {
                "请选择非空的 m4a、mp4、wav、mp3、webm、ogg 或 flac 普通音频文件。"
            }
            Self::AudioTooLarge => "录音超过 20 MB，请缩短录音后重试。",
            Self::InvalidLanguage => "语言请留空自动识别，或使用 zh、en、zh-CN 等语言代码。",
            Self::Service(401) => "登录已失效（401）。请在 Codex 中重新登录后重试。",
            Self::Service(403) => "账号或服务拒绝了转写（403），请确认账号有权使用听写。",
            Self::Service(429) => "转写暂时受限（429）。请稍后手动重试；当前接口用量规则未知。",
            Self::Service(status) => {
                return write!(f, "转写服务返回 HTTP {status}，请稍后手动重试。");
            }
            Self::InvalidResponse => "转写服务返回了无法识别或超过大小限制的响应。接口可能已变化。",
            Self::EmptyTranscript => "没有识别到文字，请确认麦克风和录音内容。",
            Self::Network => "无法连接转写服务，请检查网络后手动重试。",
            Self::TimedOut => "转写请求超时，请手动重试。",
            Self::Cancelled => "转写已取消，未自动重试。",
        };
        f.write_str(text)
    }
}
impl std::error::Error for Error {}
