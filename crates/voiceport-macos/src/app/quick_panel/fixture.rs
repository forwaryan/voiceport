//! Non-presented, main-thread AppKit fixture; never starts the VoicePort app.
use super::*;
use objc2_foundation::NSDictionary;
use std::path::Path;

pub(super) fn check_viewport(panel: &QuickPanel, view: &NSView) {
    let frame = view.convertRect_toView(view.bounds(), Some(&panel.document));
    let visible = panel.scroll.documentVisibleRect();
    let id = NSAccessibility::accessibilityIdentifier(view);
    assert!(
        frame.origin.x >= visible.origin.x
            && frame.origin.y >= visible.origin.y - 1.
            && frame.origin.x + frame.size.width <= visible.origin.x + visible.size.width + 1.
            && frame.origin.y + frame.size.height <= visible.origin.y + visible.size.height + 1.,
        "{id:?}: frame {frame:?} outside {visible:?} (clip {:?})",
        panel.scroll.contentView().bounds()
    );
}

pub(super) fn capture(panel: &QuickPanel, dir: &Path, file: &str) {
    let view = &panel.scroll;
    let bounds = view.bounds();
    let window = view.window().unwrap();
    window.setContentSize(bounds.size);
    view.layoutSubtreeIfNeeded();
    view.display();
    let rep = view
        .bitmapImageRepForCachingDisplayInRect(bounds)
        .expect("offscreen bitmap");
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &rep);
    let png = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }
    .expect("offscreen PNG");
    let path = dir.join(file);
    // Do not reuse an evidence filename from an earlier attempt.
    use std::io::Write;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap()
        .write_all(&png.to_vec())
        .unwrap();
}

pub(in crate::app) fn run(mtm: MainThreadMarker, output: &Path) {
    let _app = NSApplication::sharedApplication(mtm);
    let d = Delegate::new(mtm);
    let panel = d.build_quick_panel();
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            rect(0., 0., WIDTH, HEADER_HEIGHT + TOOLBAR_HEIGHT),
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    let host = NSView::initWithFrame(NSView::alloc(mtm), panel.scroll.frame());
    host.addSubview(&panel.scroll);
    window.setContentView(Some(&host));
    panel.scroll.setDrawsBackground(true);
    assert!(!window.isVisible() && !window.isKeyWindow());
    let mut c = Controller::new();
    let mut template = c.catalog.entries[0].clone();
    template.title = "短来源".into();
    c.catalog.entries = vec![template.clone()];
    panel.refresh_contents(&d, &c, "就绪", "⌃⌥空格", true);
    let light = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameAqua }).unwrap();
    window.setAppearance(Some(&light));
    assert_eq!(panel.header.effectiveAppearance().name(), light.name());
    assert_eq!(panel.current.stringValue().to_string(), "短来源");
    assert!(!panel.checkmark.isHidden());
    assert!(panel.disclosure.isHidden());
    assert!(panel.sources.isHidden());
    check_viewport(&panel, &panel.open);
    check_viewport(&panel, &panel.current);
    check_viewport(&panel, &panel.status);
    check_viewport(&panel, &panel.shortcut);
    check_viewport(&panel, &panel.manage);
    capture(&panel, output, "light-short.png");
    println!("light: short name, persistent status and shortcut, add and full window");

    let long = "长标题 · A very long synthetic source with a distinct destination · ".repeat(4);
    c.catalog.entries[0].title = long.clone();
    for index in 0..40 {
        let mut entry = template.clone();
        entry.id = format!("fixture-{index}");
        entry.title = if index == 39 {
            long.clone()
        } else {
            format!("来源 {index:02}")
        };
        if index == 1 {
            entry.source = Err("找不到合成目录中的模型文件".into());
        }
        c.catalog.entries.push(entry);
    }
    panel.refresh_contents(&d, &c, "正在转写", "⌃⌥空格", false);
    assert!(
        panel
            .current
            .accessibilityLabel()
            .unwrap()
            .to_string()
            .contains(&long)
    );
    assert_eq!(
        panel.current.lineBreakMode(),
        NSLineBreakMode::ByTruncatingTail
    );
    assert_ne!(panel.status.stringValue(), panel.shortcut.stringValue());
    assert!(!panel.manage.isEnabled());
    assert!(panel.manage.accessibilityHelp().is_some());
    assert!(panel.sources.isHidden());
    assert_eq!(panel.disclosure.title().to_string(), "更多来源（40）");
    let collapsed = panel.scroll.bounds().size.height;
    panel.expanded.set(true);
    panel.refresh_contents(&d, &c, "需要处理 · 打开查看", "⌃⌥空格", true);
    assert!(panel.disclosure.isAccessibilityExpanded());
    assert!(panel.scroll.bounds().size.height > collapsed);
    assert!(panel.document.bounds().size.height > panel.scroll.bounds().size.height);
    {
        let rows = panel.rows.borrow();
        assert_eq!(rows.len(), 40);
        assert!(!rows.iter().any(|r| r.id == c.source_id));
        assert_eq!(
            panel.source_for_sender(&rows[0].button).as_deref(),
            Some("fixture-0")
        );
        assert!(!rows[1].button.isEnabled());
        assert!(
            rows[1]
                .button
                .accessibilityHelp()
                .unwrap()
                .to_string()
                .contains("找不到合成目录")
        );
        assert_eq!(panel.source_for_sender(&rows[1].button), None);
        assert_eq!(
            rows[39].button.lineBreakMode(),
            NSLineBreakMode::ByTruncatingTail
        );
        assert!(
            NSAccessibility::accessibilityLabel(&*rows[39].button)
                .unwrap()
                .to_string()
                .contains(&long)
        );
        assert_eq!(
            unsafe { panel.disclosure.nextKeyView() }.as_deref(),
            Some(&***rows[0].button)
        );
        assert_eq!(
            unsafe { rows[39].button.nextKeyView() }.as_deref(),
            Some(&***panel.open)
        );
        assert!(
            window.makeFirstResponder(Some(&rows[39].button)),
            "last source accepts native keyboard focus"
        );
        check_viewport(&panel, &rows[39].button);
        assert_eq!(
            unsafe { rows[39].button.previousKeyView() }.as_deref(),
            Some(&***rows[38].button)
        );
        window.selectPreviousKeyView(None);
        assert_eq!(
            window.firstResponder().as_deref(),
            Some(&****rows[38].button)
        );
        window.selectNextKeyView(None);
        assert_eq!(
            window.firstResponder().as_deref(),
            Some(&****rows[39].button)
        );
        assert!(window.makeFirstResponder(Some(&rows[0].button)));
        window.selectNextKeyView(None);
        assert_eq!(
            window.firstResponder().as_deref(),
            Some(&****rows[2].button)
        );
    }
    let dark = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }).unwrap();
    window.setAppearance(Some(&dark));
    assert_eq!(panel.header.effectiveAppearance().name(), dark.name());
    panel.scroll_to_top();
    check_viewport(&panel, &panel.status);
    capture(&panel, output, "dark-long.png");
    println!(
        "dark: long duplicate titles, 40 alternatives, stable IDs, disabled reason, key-view links and scroll-to-last focus"
    );

    panel.expanded.set(false);
    c.catalog.entries[0].source = Err("合成来源不可用".into());
    panel.screen_height.set(260.);
    panel.refresh_contents(&d, &c, "转写完成 · 自动输入未完成", "⌃⌥空格", true);
    assert!(panel.checkmark.isHidden());
    assert!(!panel.guidance.isHidden());
    let (recovery, show) = panel.fixture_recovery(&d, false);
    panel.position(PanelLayout::new(
        0,
        true,
        0.,
        recovery,
        panel.screen_height.get(),
    ));
    panel.relink_key_views();
    panel.scroll_to_top();
    assert_eq!(panel.scroll.bounds().size.height, 236.);
    check_viewport(&panel, &panel.open);
    check_viewport(&panel, &panel.shortcut);
    check_viewport(&panel, &show);
    assert!(window.makeFirstResponder(Some(&show)));
    check_viewport(&panel, &show);
    panel.scroll_to_top();
    capture(&panel, output, "dark-small-recovery.png");
    let (recovery, copy) = panel.fixture_recovery(&d, true);
    panel.position(PanelLayout::new(
        0,
        true,
        0.,
        recovery,
        panel.screen_height.get(),
    ));
    panel.relink_key_views();
    assert!(window.makeFirstResponder(Some(&copy)));
    check_viewport(&panel, &copy);
    assert!(window.makeFirstResponder(Some(&panel.manage)));
    check_viewport(&panel, &panel.manage);
    println!(
        "260-point screen: synthetic uncertain receipt, show/copy slot and add independently reachable by local focus"
    );
    panel.fixture_recovery_copy_once(&d);
    history::fixture::run(&panel, &d, &window, output);

    c.catalog.entries.clear();
    panel.refresh_contents(&d, &c, "就绪", "⌃⌥空格", true);
    panel.scroll_to_top();
    window.setAppearance(Some(&light));
    assert_eq!(panel.current.stringValue().to_string(), "未配置来源");
    assert!(
        panel.disclosure.isHidden()
            && panel.guidance.isHidden()
            && panel.fixture_recovery_is_empty()
    );
    assert!(panel.manage.isEnabled());
    check_viewport(&panel, &panel.manage);
    capture(&panel, output, "light-empty.png");
    assert!(!window.isVisible() && !window.isKeyWindow());
    window.close();
    println!("empty: add remains available; never showed or made the fixture window key");
}
