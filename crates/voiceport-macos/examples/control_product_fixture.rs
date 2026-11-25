#[cfg(all(target_os = "macos", debug_assertions))]
fn main() {
    let mut args = std::env::args_os().skip(1);
    let cli =
        std::path::PathBuf::from(args.next().expect("absolute path to a built voiceport CLI"));
    assert!(cli.is_absolute() && args.next().is_none());
    voiceport_macos::isolated_control_regression(objc2::MainThreadMarker::new().unwrap(), &cli);
}

#[cfg(not(all(target_os = "macos", debug_assertions)))]
fn main() {
    eprintln!("This fixture requires a debug build on macOS.");
    std::process::exit(1);
}
