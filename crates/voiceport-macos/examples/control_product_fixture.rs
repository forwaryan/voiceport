#[cfg(all(target_os = "macos", debug_assertions))]
#[path = "../fixtures/hosted_aqua_entry.rs"]
mod hosted_aqua_entry;

#[cfg(all(target_os = "macos", debug_assertions))]
fn main() {
    let mut args = std::env::args_os().skip(1);
    let cli =
        std::path::PathBuf::from(args.next().expect("absolute path to a built voiceport CLI"));
    assert!(cli.is_absolute());
    let mtm = objc2::MainThreadMarker::new().unwrap();
    let mode = args.next();
    if std::env::current_exe().is_ok_and(|path| path.file_name().is_some_and(|name| name == "voiceport-files-fixture"))
        && mode.as_deref() != Some(std::ffi::OsStr::new("--hosted-files"))
    {
        std::process::exit(78);
    }
    match mode.as_deref() {
        None => voiceport_macos::isolated_control_regression(mtm, &cli),
        Some(mode) if mode == "--hosted-files" => {
            let args: Vec<_> = args.map(|word| word.into_string().unwrap()).collect();
            let [boot, aqua, launch @ ..] = args.as_slice() else { std::process::exit(78) };
            hosted_aqua_entry::enter(boot, aqua, launch, "org.voiceport.files-acceptance-fixture", 100);
            assert!(std::env::var_os("VOICEPORT_HOSTED_FILES_DIR").is_some());
            objc2::rc::autoreleasepool(|_| voiceport_macos::isolated_control_regression(mtm, &cli));
        }
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
