fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [a] if a == "--help" => println!(
            "voiceport-windows [--help | --smoke-test | --run]\nPartial Windows adapter; native tray available on Windows; capture/storage unavailable; finalized-recording backend uses the service feature."
        ),
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
