fn main() {
    #[cfg(target_os = "macos")]
    voiceport_macos::run();
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("voiceport-macos requires macOS 13 or newer");
        std::process::exit(2);
    }
}
