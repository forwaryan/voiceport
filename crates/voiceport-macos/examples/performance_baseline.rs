//! Debug-only, owned measurement process. The example never launches VoicePort.
#[cfg(all(target_os = "macos", debug_assertions))]
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    voiceport_macos::isolated_performance_baseline(&args);
}

#[cfg(not(all(target_os = "macos", debug_assertions)))]
fn main() {
    eprintln!("This fixture requires a debug build on macOS.");
    std::process::exit(1);
}
