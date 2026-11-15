//! Fixed-text production delivery probe, guarded by the exact dedicated fixture
//! bundle/PID and its untouched synthetic receipt. Never records or uploads.
#[cfg(target_os = "macos")]
fn main() {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSDate, NSRunLoop};
    use voiceport_macos::delivery::Delivery;
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert_eq!(args.len(), 2, "PID FIXTURE_RECEIPT");
    let pid: u32 = args[0].parse().unwrap();
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&args[1]).unwrap()).unwrap();
    assert_eq!(receipt["pid"], pid);
    assert_eq!(receipt["text"], "左|右");
    let mtm = MainThreadMarker::new().unwrap();
    let _app = NSApplication::sharedApplication(mtm);
    let mut report = Delivery::diagnose_input();
    if report["bundle"] != "io.github.forwaryan.voiceport.compatibility-fixture"
        || report["pid"] != pid
    {
        report["refused"] = "dedicated compatibility fixture is not focused".into();
        println!("{report}");
        std::process::exit(2);
    }
    let mut delivery = Delivery::new();
    let expected = voiceport_macos::delivery::model::ApplicationTarget {
        pid: i32::try_from(pid).unwrap(),
        bundle: "io.github.forwaryan.voiceport.compatibility-fixture".into(),
    };
    match delivery.begin_in_application(Some(&expected)) {
        Ok(()) => {
            report["begin"] = "ok".into();
            report["method"] = delivery.method().into();
            report["preview"] = delivery.preview("过时预览").into();
            report["stop"] = delivery.finish_preview().into();
            let before: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&args[1]).unwrap()).unwrap();
            assert_eq!(before["text"], "左|右", "preview/stop must not write");
            report["final"] = delivery.deliver("通用输入🙂\n第二行", true).into();
            for _ in 0..40 {
                NSRunLoop::currentRunLoop()
                    .runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.05));
                if let Some(status) = delivery.poll() {
                    report["poll"] = status.into();
                }
                if !delivery.has_pending() {
                    break;
                }
            }
            report["pending"] = delivery.has_pending().into();
            // Let the receiver process its queued paste before reading its own
            // explicit receipt. Event posting itself is never the success proof.
            for _ in 0..20 {
                NSRunLoop::currentRunLoop()
                    .runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(0.05));
                let received: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&args[1]).unwrap()).unwrap();
                if received["text"] == "左通用输入🙂 第二行右" {
                    report["received"] = true.into();
                    break;
                }
            }
            report["failureStage"] = delivery.failure_stage().into();
        }
        Err(error) => report["begin"] = error.into(),
    }
    println!("{report}");
    if report["received"] != true {
        std::process::exit(1);
    }
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("compatibility_delivery_probe requires macOS");
}
