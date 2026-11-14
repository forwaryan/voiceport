//! Opt-in inspection of production focus preparation. Never submits text,
//! starts a recorder, accesses credentials, or makes network requests.
#[cfg(target_os = "macos")]
fn main() {
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace,
    };
    use objc2_foundation::{MainThreadMarker, NSDate, NSRunLoop};
    use voiceport_macos::delivery::Delivery;
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert!(
        args.len() == 2 || (args.len() == 3 && args[2] == "--activate-target"),
        "usage: focus_capture_probe --expected-pid PID [--activate-target]"
    );
    assert_eq!(args[0], "--expected-pid");
    let expected: i64 = args[1].parse().expect("target PID");
    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    let previous = NSWorkspace::sharedWorkspace().frontmostApplication();
    if args.len() == 3 {
        let target = NSRunningApplication::runningApplicationWithProcessIdentifier(expected as i32)
            .expect("target process");
        #[allow(deprecated)]
        target.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions::empty());
        NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.1));
    }
    let before = Delivery::diagnose_input();
    assert_eq!(before["pid"], expected, "foreground target changed");
    let mut delivery = Delivery::new();
    let begin = delivery.begin();
    let was_preparing = delivery.preparing_input();
    for _ in 0..100 {
        if !delivery.preparing_input() {
            break;
        }
        NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.025));
        delivery.poll();
    }
    let after = Delivery::diagnose_input();
    let stage = delivery.failure_stage();
    let success = begin.is_ok()
        && stage.is_none()
        && !delivery.preparing_input()
        && after["pid"] == expected
        && after["AXRole"].is_string();
    println!(
        "{}",
        serde_json::json!({
            "begin":begin, "wasPreparing":was_preparing, "stillPreparing":delivery.preparing_input(),
            "failureStage":stage, "success":success,
            "beforeFocusError":before["focusedElementAXError"],
            "afterRole":after["AXRole"], "afterPid":after["pid"],
            "isTerminal":after["AXDescription"] == "Terminal input", "textSubmitted":false,
        })
    );
    // Capture alone owns no edited span; clear never writes to the target.
    delivery.clear();
    if args.len() == 3
        && NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .is_some_and(|app| app.processIdentifier() as i64 == expected)
        && let Some(previous) = previous
        && previous.processIdentifier() as i64 != expected
    {
        #[allow(deprecated)]
        previous.activateWithOptions(objc2_app_kit::NSApplicationActivationOptions::empty());
    }
    if !success {
        std::process::exit(1);
    }
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("focus_capture_probe requires macOS");
}
