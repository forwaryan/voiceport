#[cfg(all(target_os = "macos", debug_assertions))]
fn main() {
    let mut args = std::env::args_os().skip(1);
    let cli =
        std::path::PathBuf::from(args.next().expect("absolute path to a built voiceport CLI"));
    assert!(cli.is_absolute());
    let mtm = objc2::MainThreadMarker::new().unwrap();
    match args.next().as_deref() {
        None => voiceport_macos::isolated_control_regression(mtm, &cli),
        Some(mode) if mode == "--audio-start" => {
            assert!(args.next().is_none());
            voiceport_macos::isolated_audio_start_regression(mtm, &cli, None);
        }
        Some(mode) if mode == "--audio-start-quit-child" => {
            let root = std::path::PathBuf::from(args.next().expect("parent fixture directory"));
            assert!(root.is_absolute() && args.next().is_none());
            voiceport_macos::isolated_audio_start_regression(mtm, &cli, Some(&root));
        }
        _ => panic!("unknown fixture mode"),
    }
}

#[cfg(not(all(target_os = "macos", debug_assertions)))]
fn main() {
    eprintln!("This fixture requires a debug build on macOS.");
    std::process::exit(1);
}
