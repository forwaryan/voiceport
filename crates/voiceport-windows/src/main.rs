fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [a] if a == "--help" => {
            println!("voiceport-windows [--help | --smoke-test | --run]");
            if cfg!(all(windows, feature = "service")) {
                println!(
                    "--run opens the Windows tray. Use Start dictation and Stop and transcribe, or Ctrl+Alt+Space. Copy results manually."
                );
            } else if cfg!(windows) {
                println!("--run opens the Windows tray. Dictation requires the service feature.");
            } else {
                println!("--run requires Windows. The smoke test performs no I/O.");
            }
        }
        [a] if a == "--smoke-test" => {
            let c = voiceport_windows::Controller::default();
            println!(
                "No-I/O adapter smoke: {:?}; automatic delivery disabled",
                c.phase()
            );
        }
        [a] if a == "--run" => {
            #[cfg(windows)]
            if let Err(error) = voiceport_windows::shell::run() {
                eprintln!("{error}");
                std::process::exit(1);
            }
            #[cfg(not(windows))]
            {
                eprintln!("Native shell requires Windows");
                std::process::exit(2);
            }
        }
        _ => {
            eprintln!("Windows desktop integration is incomplete; no recording or input started.");
            std::process::exit(2);
        }
    }
}
