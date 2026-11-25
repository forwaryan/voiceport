//! Run on the process main thread so AppKit never leaves its required thread.
#[cfg(all(target_os = "macos", debug_assertions))]
fn main() {
    let main = objc2::MainThreadMarker::new().expect("fixture requires the process main thread");
    objc2::rc::autoreleasepool(|_| {
        voiceport_macos::isolated_file_transcription_regression(main);
    });
}

#[cfg(not(all(target_os = "macos", debug_assertions)))]
fn main() {
    eprintln!("File transcription AppKit fixture requires a macOS debug build.");
    std::process::exit(78);
}
