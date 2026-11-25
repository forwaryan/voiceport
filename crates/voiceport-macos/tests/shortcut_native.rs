//! Native key registration and recorder checks on the OS main thread.
//! No key events are posted, windows shown, preferences written or audio started.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/hotkey.rs"]
mod hotkey;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/shortcut.rs"]
#[allow(unused_imports)]
mod shortcut;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/shortcut_recorder.rs"]
mod shortcut_recorder;
#[cfg(target_os = "macos")]
use voiceport_macos::delivery;

#[cfg(target_os = "macos")]
fn main() {
    use shortcut::{COMMAND, CONTROL, OPTION, SHIFT, Shortcut};
    use std::sync::mpsc;
    let mtm = objc2::MainThreadMarker::new().expect("requires the OS main thread");
    objc2::rc::autoreleasepool(|_| {
        shortcut_recorder::regression(mtm);
        voiceport_macos::isolated_shortcut_regression(mtm);
        if std::env::var_os("VOICEPORT_TEST_ISOLATED_UI").is_some() {
            println!("shortcut_native: skipped global registration in isolated UI mode");
            return;
        }
        let modifiers = COMMAND | CONTROL | OPTION | SHIFT;
        let original = Shortcut::new(64, modifiers).unwrap();
        let occupied = Shortcut::new(79, modifiers).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut previous = hotkey::GlobalHotKey::new(tx.clone());
        let mut blocker = hotkey::GlobalHotKey::new(tx.clone());
        previous
            .register(original)
            .expect("temporary F17 registration");
        blocker
            .register(occupied)
            .expect("temporary F18 conflict fixture");
        let mut candidate = hotkey::GlobalHotKey::new(tx.clone());
        assert!(
            candidate.register(occupied).is_err(),
            "occupied binding must fail"
        );
        // Failed candidate cleanup must leave both prior registrations active.
        assert!(
            candidate.register(original).is_err(),
            "previous binding was lost"
        );
        blocker.unregister();
        candidate
            .register(occupied)
            .expect("released binding must become available");
        drop(previous);
        let mut replacement = hotkey::GlobalHotKey::new(tx);
        replacement
            .register(original)
            .expect("drop releases previous registration");
        assert!(
            rx.try_recv().is_err(),
            "registration must never emit a press"
        );
        println!(
            "shortcut_native: conflict isolation, release, replacement and inert registration passed"
        );
    });
}

#[cfg(not(target_os = "macos"))]
fn main() {
    println!("shortcut_native: macOS-only; native checks not exercised on this platform");
}
