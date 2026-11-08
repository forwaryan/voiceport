//! Opt-in development probe. Focus ONLY a dedicated input_terminal_fixture.py
//! terminal first. Writes a fixed synthetic string, never Return or a command.
#[cfg(target_os = "macos")]
fn main() {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSDate, NSRunLoop};
    use voiceport_macos::delivery::Delivery;
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(
        args.len(),
        2,
        "usage: terminal_delivery_probe --dedicated-fixture-pid PID"
    );
    assert_eq!(args[0], "--dedicated-fixture-pid");
    let pid: i64 = args[1].parse().expect("expected Orca PID");
    let mtm = MainThreadMarker::new().expect("probe main thread");
    let _app = NSApplication::sharedApplication(mtm);
    let mut report = Delivery::diagnose_input();
    if report["bundle"] != "com.stablyai.orca"
        || report["pid"] != pid
        || report["AXDescription"] != "Terminal input"
    {
        report["refused"] = "dedicated Orca terminal is not focused".into();
        println!("{report}");
        std::process::exit(2);
    }
    let mut delivery = Delivery::new();
    match delivery.begin() {
        Ok(()) => {
            report["begin"] = "ok".into();
            report["final"] = delivery.deliver("终端输入验证🙂", true).into();
            for _ in 0..50 {
                NSRunLoop::currentRunLoop()
                    .runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.1));
                if let Some(message) = delivery.poll() {
                    report["poll"] = message.into();
                }
                if !delivery.has_pending() {
                    break;
                }
            }
            report["pending"] = delivery.has_pending().into();
        }
        Err(error) => report["begin"] = error.into(),
    }
    println!("{report}");
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("terminal_delivery_probe requires macOS");
}
