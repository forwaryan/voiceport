//! Explicit background application diagnosis. Never activates, reads field text,
//! records audio, accesses credentials, writes text, or arms automatic delivery.
#[cfg(target_os = "macos")]
fn main() {
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::MainThreadMarker;
    use voiceport_macos::delivery::Delivery;
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(args.len(), 2, "usage: input_metadata_probe --pid PID");
    assert_eq!(args[0], "--pid");
    let pid: i32 = args[1].parse().expect("positive application PID");
    assert!(pid > 0);
    let mtm = MainThreadMarker::new().expect("main thread");
    NSApplication::sharedApplication(mtm)
        .setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
    let report = Delivery::diagnose_application(pid);
    let success = report["focusSource"].is_string() && report["AXRole"].is_string();
    println!("{report}");
    if !success {
        std::process::exit(1);
    }
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("input_metadata_probe requires macOS");
    std::process::exit(1);
}
