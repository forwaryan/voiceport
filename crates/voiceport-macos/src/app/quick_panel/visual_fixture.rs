//! Capture only the retained view of a window owned by this fixture process.
use super::*;
use objc2_foundation::NSDictionary;
use std::{io::Write, path::Path, time::Instant};

pub(super) fn capture_view(window: &NSWindow, view: &NSView, path: &Path) -> Instant {
    let app = NSApplication::sharedApplication(window.mtm());
    let target = NSApplicationActivationPolicy::Prohibited;
    let assert_owned_hidden_view = |phase: &str| {
        let current = app.activationPolicy();
        let app_active = app.isActive();
        let window_key = window.isKeyWindow();
        assert!(
            current == target && !app_active && !window_key,
            "native-ui-activation phase={phase} current={} target={} app_active={app_active} window_key={window_key}",
            current.0,
            target.0
        );
        assert!(
            view.window()
                .is_some_and(|owner| std::ptr::eq(&*owner, window))
        );
        assert!(!view.isHiddenOrHasHiddenAncestor());
        assert!(!window.isVisible() && !window.isMainWindow());
        assert!(app.keyWindow().is_none() && app.mainWindow().is_none());
    };
    assert_owned_hidden_view("before-capture");
    let responder = window.firstResponder();
    let bounds = view.bounds();
    assert!(bounds.size.width > 0. && bounds.size.height > 0.);
    view.layoutSubtreeIfNeeded();
    view.display();
    let rep = view
        .bitmapImageRepForCachingDisplayInRect(bounds)
        .expect("offscreen bitmap");
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    assert_owned_hidden_view("after-capture");
    assert_eq!(window.firstResponder(), responder);
    let rendered_at = Instant::now();
    // A valid PNG container can still hold a uniform render.
    let corner = rep.colorAtX_y(0, 0);
    assert!(
        (0..rep.pixelsHigh()).step_by(8).any(|y| {
            (0..rep.pixelsWide())
                .step_by(8)
                .any(|x| rep.colorAtX_y(x, y) != corner)
        }),
        "uniform offscreen render"
    );
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .expect("offscreen PNG");
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap()
        .write_all(&png.to_vec())
        .unwrap();
    rendered_at
}

pub(in crate::app::quick_panel) fn result_scene(
    d: &Delegate,
    snapshot: dictation_page::Snapshot<'_>,
    output: &Path,
    verify_edit: impl FnOnce(&dictation_page::Page),
) {
    let mtm = d.mtm();
    let bounds = rect(0., 0., 572., 450.);
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            bounds,
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    let light = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }).unwrap();
    window.setAppearance(Some(&light));
    let area = NSView::initWithFrame(NSView::alloc(mtm), bounds);
    window.setContentView(Some(&area));
    let background = NSBox::initWithFrame(NSBox::alloc(mtm), bounds);
    background.setBoxType(NSBoxType::Custom);
    background.setTitlePosition(NSTitlePosition::NoTitle);
    background.setBorderWidth(0.);
    background.setCornerRadius(0.);
    background.setFillColor(&NSColor::windowBackgroundColor().colorWithAlphaComponent(1.));
    background.setAccessibilityElement(false);
    area.addSubview(&background);
    let page = dictation_page::Page::build(mtm, d, &area);
    let final_text = snapshot.original;
    page.refresh(snapshot);
    assert!(!page.result.isEditable());
    assert_eq!(page.result.string().to_string(), final_text);
    capture_view(&window, &area, &output.join("light-result.png"));
    verify_edit(&page);
    capture_view(&window, &area, &output.join("light-result-edit.png"));
    window.close();
}
