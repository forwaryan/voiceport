//! Isolated, synthetic target for manual native input verification; never records.
#[cfg(target_os = "macos")]
fn main() {
    use objc2::{MainThreadOnly, rc::Retained};
    use objc2_app_kit::*;
    use objc2_foundation::{MainThreadMarker, NSPoint, NSRange, NSRect, NSSize, ns_string};
    let mtm = MainThreadMarker::new().expect("fixture main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(0., 0.), NSSize::new(620., 260.)),
            NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe {
        window.setReleasedWhenClosed(false);
    }
    window.setTitle(ns_string!("VoicePort · 独立输入自测（固定文字，无录音）"));
    let view = window.contentView().unwrap();
    let label = NSTextField::wrappingLabelWithString(
        ns_string!(
            "仅用于验证 VoicePort 在其他应用的光标位置写入。初始为「左|右」，选中竖线；预期为「左输入验证🙂右」。不会发送或按回车。"
        ),
        mtm,
    );
    label.setFrame(NSRect::new(NSPoint::new(24., 175.), NSSize::new(572., 60.)));
    view.addSubview(&label);
    let text: Retained<NSTextView> = NSTextView::initWithFrame(
        NSTextView::alloc(mtm),
        NSRect::new(NSPoint::new(24., 24.), NSSize::new(572., 140.)),
    );
    text.setString(ns_string!("左|右"));
    text.setAccessibilityIdentifier(Some(ns_string!("voiceport.input-self-test")));
    text.setAccessibilityLabel(Some(ns_string!("VoicePort 独立测试输入框")));
    view.addSubview(&text);
    window.center();
    window.makeKeyAndOrderFront(None);
    window.makeFirstResponder(Some(&text));
    text.setSelectedRange(NSRange::new(1, 1));
    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);
    app.run();
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("input_fixture requires macOS");
}
