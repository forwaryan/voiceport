//! Non-presented, main-thread AppKit fixture; never starts the VoicePort app.
use super::*;
use std::{path::Path, time::Instant};

#[path = "visual_fixture.rs"]
mod visual;
pub(super) use visual::result_scene;

mod refresh;

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

pub(super) fn capture(panel: &QuickPanel, window: &NSWindow, dir: &Path, file: &str) -> Instant {
    window.setContentSize(panel.scroll.bounds().size);
    visual::capture_view(window, &panel.scroll, &dir.join(file))
}

pub(in crate::app) fn run(mtm: MainThreadMarker, output: &Path, cli: Option<&Path>) {
    let app = NSApplication::sharedApplication(mtm);
    let target = NSApplicationActivationPolicy::Prohibited;
    let setter_accepted = app.setActivationPolicy(target);
    let current = app.activationPolicy();
    let app_active = app.isActive();
    eprintln!(
        "native-ui-activation phase=startup current={} target={} setter_accepted={setter_accepted} app_active={app_active}",
        current.0, target.0
    );
    assert_eq!(
        current, target,
        "offscreen fixture activation policy mismatch"
    );
    assert!(!app_active && app.keyWindow().is_none() && app.mainWindow().is_none());
    let first_frame = Instant::now();
    let d = Delegate::new(mtm);
    let panel = d.build_quick_panel();
    panel.refresh_update(&d, true, first_frame);
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
    let rendered_at = capture(&panel, &window, output, "light-short.png");
    println!(
        "native-ui-timing: first_offscreen_frame_us {}",
        rendered_at.duration_since(first_frame).as_micros()
    );
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
    let dark = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }).unwrap();
    window.setAppearance(Some(&dark));
    assert_eq!(panel.header.effectiveAppearance().name(), dark.name());
    let busy_update = Instant::now();
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
    let rendered_at = capture(&panel, &window, output, "dark-busy.png");
    println!(
        "native-ui-timing: busy_offscreen_refresh_us {}",
        rendered_at.duration_since(busy_update).as_micros()
    );
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
        assert_eq!(
            unsafe { panel.open.nextKeyView() }.as_deref(),
            Some(&***panel.update)
        );
        assert_eq!(panel.update.action(), Some(sel!(updateVoicePort:)));
        assert!(panel.update.isEnabled());
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
    let retained = panel.rows.borrow()[2].button.clone();
    let visible = panel.scroll.documentVisibleRect();
    c.catalog.entries[3].title = "合成来源更新".into();
    c.catalog.entries.remove(4);
    panel.refresh_contents(&d, &c, "就绪", "⌃⌥空格", true);
    assert!(std::ptr::eq(&*panel.rows.borrow()[2].button, &*retained));
    assert!(std::ptr::eq(
        window.firstResponder().as_deref().unwrap(),
        &****retained
    ));
    assert_eq!(
        panel.rows.borrow()[2].button.title().to_string(),
        "合成来源更新"
    );
    assert_eq!(c.source_id, template.id);
    let new_visible = panel.scroll.documentVisibleRect();
    assert_eq!(visible.origin.y - new_visible.origin.y, ROW_HEIGHT);
    println!("source delta: retained native control, responder, selection and viewport inset");

    panel.scroll_to_top();
    check_viewport(&panel, &panel.status);
    capture(&panel, &window, output, "dark-long.png");
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
    assert!(window.makeFirstResponder(Some(&show)));
    check_viewport(&panel, &show);
    panel.scroll_to_top();
    capture(&panel, &window, output, "dark-small-recovery.png");
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
    capture(&panel, &window, output, "dark-small-revealed.png");
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
    capture(&panel, &window, output, "light-empty.png");
    assert!(!window.isVisible() && !window.isKeyWindow());
    window.close();
    println!("empty: add remains available; never showed or made the fixture window key");
    if let Some(cli) = cli {
        refresh::run(mtm, cli);
    }
    println!("native-ui: owned, non-presented AppKit captures completed without activation");
}
