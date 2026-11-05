mod host;
use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::process::ExitCode;
use voiceport_core::{CancellationToken, Error, TranscriptionClient, auth};

const HELP: &str = "VoicePort Rust — 便携转写核心（macOS 优先，Windows 系统适配尚未验收）

voiceport-rs help | --help | -h
voiceport-rs host [--auth PATH]
    有界 JSONL 管道宿主，启动不读取登录、不联网。
voiceport-rs demo
    离线模拟，不录音、不读取登录、不访问网络。
voiceport-rs doctor [--auth PATH]
    只检查登录文件存在性，不读取或验证凭据。
voiceport-rs transcribe AUDIO [--language zh] [--auth PATH]
    此命令会使用 Codex ChatGPT 文件登录上传音频，Ctrl-C 取消。
    默认使用 CODEX_HOME/auth.json，否则 ~/.codex/auth.json。

不刷新登录，不自动重传，不自动投递文字。接口与用量规则未公开保证。
退出码：0 成功，1 运行错误，2 参数错误，130 取消。";

enum Command {
    Help,
    Host {
        auth: Option<PathBuf>,
    },
    Demo,
    Doctor {
        auth: Option<PathBuf>,
    },
    Transcribe {
        audio: PathBuf,
        language: Option<String>,
        auth: Option<PathBuf>,
    },
}

fn parse(arguments: Vec<OsString>) -> Result<Command, ()> {
    let Some(command) = arguments.first() else {
        return Ok(Command::Help);
    };
    if ["help", "--help", "-h"]
        .iter()
        .any(|value| command == value)
    {
        return if arguments.len() == 1 {
            Ok(Command::Help)
        } else {
            Err(())
        };
    }
    if command == "demo" {
        return if arguments.len() == 1 {
            Ok(Command::Demo)
        } else {
            Err(())
        };
    }
    let is_host = command == "host";
    let transcribe = command == "transcribe";
    if !transcribe && !is_host && command != "doctor" {
        return Err(());
    }
    let mut index = if transcribe { 2 } else { 1 };
    let audio = if transcribe {
        let value = arguments
            .get(1)
            .filter(|v| !v.is_empty() && !v.as_encoded_bytes().starts_with(b"--"))
            .ok_or(())?;
        Some(PathBuf::from(value))
    } else {
        None
    };
    let mut auth = None;
    let mut language = None;
    while index < arguments.len() {
        let flag = &arguments[index];
        let value = arguments
            .get(index + 1)
            .filter(|v| !v.is_empty() && !v.as_encoded_bytes().starts_with(b"--"))
            .ok_or(())?;
        if flag == OsStr::new("--auth") && auth.is_none() {
            auth = Some(PathBuf::from(value));
        } else if flag == OsStr::new("--language") && transcribe && language.is_none() {
            language = Some(value.to_str().ok_or(())?.to_owned());
        } else {
            return Err(());
        }
        index += 2;
    }
    if let Some(audio) = audio {
        Ok(Command::Transcribe {
            audio,
            language,
            auth,
        })
    } else if is_host {
        Ok(Command::Host { auth })
    } else {
        Ok(Command::Doctor { auth })
    }
}

fn auth_path(path: Option<PathBuf>) -> voiceport_core::Result<PathBuf> {
    path.map_or_else(auth::default_path, Ok)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let command = match parse(std::env::args_os().skip(1).collect()) {
        Ok(command) => command,
        Err(()) => {
            eprintln!("VoicePort: 参数无效。运行 voiceport-rs --help 查看用法。");
            return ExitCode::from(2);
        }
    };
    let result = match command {
        Command::Host { auth } => {
            let service: host::Service = std::sync::Arc::new(move |job| {
                let auth = auth.clone();
                Box::pin(async move {
                    TranscriptionClient::new(auth_path(auth)?)?
                        .transcribe(&job.audio_path, job.language.as_deref(), &job.cancellation)
                        .await
                })
            });
            host::run(
                std::io::stdin(),
                std::io::stdout(),
                service,
                std::time::Duration::from_secs(5),
            )
            .await
        }
        Command::Help => {
            println!("{HELP}");
            Ok(())
        }
        Command::Demo => {
            println!(
                "[离线模拟，未调用云端] 保留 requestID，不要自动发送。这里没有读取凭据、录音或访问网络。"
            );
            Ok(())
        }
        Command::Doctor { auth } => auth_path(auth).map(|file| {
            println!(
                "Codex 登录文件：{}",
                if file.exists() {
                    "存在（未读取、未检查有效性）"
                } else {
                    "未找到，请运行 codex login"
                }
            );
            println!("仅支持 ChatGPT 文件登录；不输出路径或凭据。Rust 系统录音和投递尚未接入。");
        }),
        Command::Transcribe {
            audio,
            language,
            auth,
        } => transcribe(audio, language, auth).await,
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("VoicePort: {error}");
            ExitCode::from(error_exit_code(error))
        }
    }
}

async fn transcribe(
    audio: PathBuf,
    language: Option<String>,
    auth: Option<PathBuf>,
) -> voiceport_core::Result<()> {
    let client = TranscriptionClient::new(auth_path(auth)?)?;
    let cancellation = CancellationToken::new();
    let result = until_interrupt(
        &cancellation,
        client.transcribe(&audio, language.as_deref(), &cancellation),
    )
    .await;
    println!("{}", result?);
    Ok(())
}

fn error_exit_code(error: Error) -> u8 {
    if error == Error::Cancelled { 130 } else { 1 }
}

async fn until_interrupt<T>(
    cancellation: &CancellationToken,
    operation: impl std::future::Future<Output = voiceport_core::Result<T>>,
) -> voiceport_core::Result<T> {
    tokio::pin!(operation);
    tokio::select! {
        biased;
        signal = tokio::signal::ctrl_c() => {
            signal.map_err(|_| Error::Network)?;
            cancellation.cancel();
            operation.await
        },
        result = &mut operation => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_exit_codes_are_stable() {
        assert_eq!(error_exit_code(Error::Cancelled), 130);
        assert_eq!(error_exit_code(Error::InvalidCredentials), 1);
    }

    #[cfg(unix)]
    #[test]
    fn sigint_cancels_the_shared_cli_handler_in_a_real_child() {
        use std::io::{BufRead, Write};
        use std::process::{Command, Stdio};
        if std::env::var_os("VP009_SIGNAL_CHILD").is_some() {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let cancellation = CancellationToken::new();
                let operation = async {
                    // Biased select polls/installs the real Ctrl-C handler first.
                    println!("CTRL_C_READY");
                    std::io::stdout().flush().unwrap();
                    cancellation.cancelled().await;
                    Err::<(), _>(Error::Cancelled)
                };
                let error = until_interrupt(&cancellation, operation).await.unwrap_err();
                assert_eq!(error, Error::Cancelled);
                assert_eq!(error_exit_code(error), 130);
            });
            return;
        }
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::sigint_cancels_the_shared_cli_handler_in_a_real_child",
                "--nocapture",
            ])
            .env("VP009_SIGNAL_CHILD", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                reader.read_line(&mut line).unwrap() > 0,
                "child ended before signal registration"
            );
            if line.contains("CTRL_C_READY") {
                break;
            }
        }
        assert!(
            Command::new("/bin/kill")
                .args(["-INT", &child.id().to_string()])
                .status()
                .unwrap()
                .success()
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                panic!("SIGINT cancellation did not complete");
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
