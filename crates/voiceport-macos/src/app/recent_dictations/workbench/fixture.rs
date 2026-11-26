//! Exercises the workbench's native history controls in an unshown AppKit window.
use super::*;
use crate::controller::recent_dictations::{MAX_ENTRY_BYTES, fixture::SyntheticDictations};
use objc2::AnyThread;

fn with_controller<R>(
    owner: &Delegate,
    source: &mut SyntheticDictations,
    run: impl FnOnce() -> R,
) -> R {
    std::mem::swap(
        owner.ivars().controller.borrow_mut().as_mut().unwrap(),
        &mut source.controller,
    );
    let result = run();
    std::mem::swap(
        owner.ivars().controller.borrow_mut().as_mut().unwrap(),
        &mut source.controller,
    );
    result
}

fn click(button: &NSButton) {
    unsafe { button.performClick(None) };
}

fn check_quick_panel_hide(mtm: MainThreadMarker) {
    let owner = Delegate::new(mtm);
    let mut source = SyntheticDictations::new();
    source.controller.set_recent_dictations_enabled(true);
    let (generation, _) = source.begin(true, false);
    let body = "快捷面板隐藏前展开的合成听写🙂";
    source.finish(generation, body);
    let id = source.controller.list_recent_dictations(1).remove(0).id;
    *owner.ivars().controller.borrow_mut() = Some(source.controller);

    let window = owner.prepare_workbench(Some(crate::app::workbench::Page::History));
    let page = owner.ivars().history_page.get().unwrap();
    let controls = page.controls();
    page.refresh_from_owner();
    assert!(!controls.rows[0].isHidden());
    click(&controls.rows[0]);
    let storage = unsafe { controls.text.textStorage() }.unwrap();
    assert_eq!(storage.string().to_string(), body);
    assert_eq!(
        page.ivars().selection.borrow().as_deref(),
        Some(id.as_str())
    );

    owner.hide_workbench_for_quick_panel();
    assert_eq!(storage.length(), 0);
    assert!(page.ivars().selection.borrow().is_none());
    assert!(controls.detail.isHidden() && controls.copy.isHidden());

    assert!(std::ptr::eq(window, owner.prepare_workbench(None)));
    // The presentation path refreshes this page after ordering the window front.
    page.refresh_from_owner();
    assert_eq!(page.ivars().entries.borrow()[0].id, id);
    assert!(!controls.rows[0].isHidden() && !controls.rows[0].isAccessibilityExpanded());
    assert!(page.ivars().selection.borrow().is_none());
    assert!(storage.length() == 0 && controls.detail.isHidden());
    click(&controls.rows[0]);
    assert_eq!(storage.string().to_string(), body);
    assert!(!controls.detail.isHidden());
    assert!(!window.isVisible() && !window.isKeyWindow());
    window.close();
    println!(
        "workbench recent dictations: quick panel hide cleared the body and selection before reopen"
    );
}

pub(in crate::app) fn prove_visible_quick_panel_hide(
    owner: &Delegate,
    app: &NSApplication,
    reopen: impl FnOnce(),
) {
    use objc2_app_kit::NSTextInputClient;
    use objc2_foundation::NSRange;

    let mut rig = SyntheticDictations::new();
    rig.controller.set_recent_dictations_enabled(true);
    let bodies = ["较早的合成听写", "快捷面板隐藏前展开的合成听写🙂"];
    let mut ids = Vec::new();
    for body in bodies {
        let (generation, _) = rig.begin(true, false);
        rig.finish(generation, body);
        ids.push(rig.controller.list_recent_dictations(1).remove(0).id);
    }
    assert_ne!(ids[0], ids[1]);
    let previous = owner.ivars().controller.replace(Some(rig.controller));
    owner.show_workbench_page(crate::app::workbench::Page::History);
    let window = owner.ivars().window.get().unwrap();
    let is_key = || {
        app.isActive()
            && window.isVisible()
            && window.isKeyWindow()
            && app.keyWindow().as_deref() == Some(&**window)
    };
    assert!(is_key());
    let page = owner.ivars().history_page.get().unwrap();
    let controls = page.controls();
    assert_eq!(page.ivars().entries.borrow()[0].id, ids[1]);
    assert_eq!(page.ivars().entries.borrow()[1].id, ids[0]);
    assert!(!controls.rows[0].isHidden() && !controls.rows[1].isHidden());
    click(&controls.rows[0]);
    let storage = unsafe { controls.text.textStorage() }.unwrap();
    assert_eq!(storage.string().to_string(), bodies[1]);
    let selection = NSRange::new(0, 2);
    controls.text.setSelectedRange(selection);
    assert_eq!(NSTextInputClient::selectedRange(&*controls.text), selection);
    assert_eq!(
        page.ivars().selection.borrow().as_deref(),
        Some(ids[1].as_str())
    );
    assert!(controls.rows[0].isAccessibilityExpanded());
    let mut simulated_copies = 0;
    page.copy_with(
        owner.ivars().controller.borrow().as_ref().unwrap(),
        &controls.copy,
        |body| {
            assert_eq!(body, bodies[1]);
            simulated_copies += 1;
            true
        },
    );
    assert_eq!(simulated_copies, 1);
    assert!(!controls.feedback.stringValue().is_empty());

    // This calls the production opener, which orders out the visible Workbench.
    owner.build_menu();
    owner.show_quick_panel();
    assert!(owner.ivars().quick_panel.get().unwrap().is_shown());
    assert!(!window.isVisible() && !window.isMiniaturized() && !window.isKeyWindow());
    let cleared = || {
        storage.length() == 0
            && NSTextInputClient::selectedRange(&*controls.text).length == 0
            && page.ivars().selection.borrow().is_none()
            && controls.detail.isHidden()
            && controls.copy.isHidden()
            && controls.feedback.stringValue().is_empty()
    };
    assert!(cleared(), "ordering out History left detail state behind");

    reopen();
    assert!(is_key() && !owner.ivars().quick_panel.get().unwrap().is_shown());
    assert!(std::ptr::eq(
        &**window,
        &**owner.ivars().window.get().unwrap()
    ));
    assert!(
        cleared(),
        "reopening the same History window restored its detail"
    );
    assert_eq!(page.ivars().entries.borrow()[0].id, ids[1]);
    assert_eq!(page.ivars().entries.borrow()[1].id, ids[0]);
    assert!(
        controls.rows[..2]
            .iter()
            .all(|row| !row.isHidden() && !row.isAccessibilityExpanded())
    );
    let slot = owner.ivars().controller.borrow();
    let controller = slot.as_ref().unwrap();
    for (id, body) in ids.iter().zip(bodies) {
        assert_eq!(controller.read_recent_dictation(id), Some(body));
    }
    drop(slot);
    click(&controls.rows[0]);
    assert_eq!(storage.string().to_string(), bodies[1]);
    assert_eq!(
        page.ivars().selection.borrow().as_deref(),
        Some(ids[1].as_str())
    );
    page.clear_display();
    owner.ivars().controller.replace(previous);
    println!(
        "shell_gui_history_order_out {}",
        serde_json::json!({
            "opener": "native-programmatic", "reopen": "cli-app-show", "key_baseline": true,
            "popover_shown": true, "hidden_without_minimize": true, "same_key_window": true,
            "body_selection_feedback_cleared_while_hidden_and_reopened": true,
            "original_ids_and_bodies": 2, "same_id_expandable": true, "clipboard_writes": 0
        })
    );
}

pub(in crate::app) fn run(mtm: MainThreadMarker) {
    let owner = Delegate::new(mtm);
    let domain = NSString::from_str(&format!(
        "voiceport.history.fixture.{}",
        uuid::Uuid::new_v4()
    ));
    let defaults =
        NSUserDefaults::initWithSuiteName(NSUserDefaults::alloc(), Some(&domain)).unwrap();
    struct RemoveDefaults<'a>(&'a NSUserDefaults, &'a NSString);
    impl Drop for RemoveDefaults<'_> {
        fn drop(&mut self) {
            self.0.removePersistentDomainForName(self.1);
        }
    }
    let _remove_defaults = RemoveDefaults(&defaults, &domain);
    let _ = owner
        .ivars()
        .recent_dictations_fixture_defaults
        .set(defaults.clone());
    *owner.ivars().controller.borrow_mut() = Some(Controller::new());
    let frame = rect(0., 0., 516., 408.);
    let page = HistoryPage::build(&owner, frame);
    let controls = page.controls();
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Titled,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    window.setContentView(Some(&page));
    let keys = page.key_views();
    window.setAutorecalculatesKeyViewLoop(false);
    for (from, to) in keys.iter().zip(keys.iter().cycle().skip(1)) {
        unsafe { from.setNextKeyView(Some(to)) };
    }
    window.setInitialFirstResponder(Some(keys[0]));
    assert!(!window.isVisible() && !window.isKeyWindow());
    assert_eq!(controls.keep.state(), NSControlStateValueOff);
    assert!(!preference(&defaults));
    assert_eq!(controls.empty.stringValue().to_string(), "最近听写已关闭");
    assert!(!controls.clear.isEnabled());
    assert!(controls.rows.iter().all(|row| row.isHidden()));

    let mut source = SyntheticDictations::new();
    let (started_off, _) = source.begin(true, false);
    with_controller(&owner, &mut source, || click(&controls.keep));
    assert!(source.controller.recent_dictations_enabled() && preference(&defaults));
    source.finish(started_off, "开启前开始");
    page.refresh(&source.controller);
    assert_eq!(
        controls.empty.stringValue().to_string(),
        "本次运行暂无听写记录"
    );
    assert!(!controls.empty.isHidden() && !controls.clear.isEnabled());
    assert!(page.ivars().entries.borrow().is_empty());

    source.set_focus_safe(false);
    let (unsafe_focus, _) = source.begin(true, false);
    page.refresh(&source.controller);
    assert!(!controls.clear.isEnabled());
    source.finish(unsafe_focus, "不符合保留条件");
    source.set_focus_safe(true);

    let (pending, target) = source.begin(true, true);
    page.refresh(&source.controller);
    assert!(!controls.empty.isHidden());
    assert!(controls.rows.iter().all(|row| row.isHidden()));
    assert!(
        controls.clear.isEnabled(),
        "clear must reach the first pending entry"
    );
    assert!(window.makeFirstResponder(Some(&controls.clear)));
    with_controller(&owner, &mut source, || click(&controls.clear));
    assert!(page.has_focus(&controls.keep));
    assert!(!controls.clear.isEnabled() && preference(&defaults));
    source.finish(pending, "首轮清空后完成");
    page.refresh(&source.controller);
    assert_eq!(target.borrow().snapshot.text, "A首轮清空后完成B");
    assert_eq!(target.borrow().writes, 1);
    assert!(!controls.empty.isHidden() && !controls.clear.isEnabled());
    assert!(
        source
            .controller
            .list_recent_dictations(MAX_ENTRIES)
            .is_empty()
    );
    println!(
        "workbench recent dictations: cleared first retention candidate without cancelling dictation or delivery"
    );

    let (first, target) = source.begin(true, true);
    target.borrow_mut().confirm = false;
    source.finish(first, "第一条🙂");
    page.refresh(&source.controller);
    let first_id = page.ivars().entries.borrow()[0].id.clone();
    assert!(controls.rows[0].title().to_string().contains("等待确认"));
    assert!(!controls.rows[0].isHidden() && controls.rows[1].isHidden());
    assert!(controls.detail.isHidden());
    assert!(window.makeFirstResponder(Some(&controls.rows[0])));
    with_controller(&owner, &mut source, || click(&controls.rows[0]));
    assert_eq!(controls.text.string().to_string(), "第一条🙂");
    assert_eq!(
        page.ivars().selection.borrow().as_deref(),
        Some(first_id.as_str())
    );
    target.borrow_mut().confirm = true;
    source.poll();
    page.refresh(&source.controller);
    assert_eq!(page.ivars().entries.borrow()[0].id, first_id);
    assert!(controls.rows[0].title().to_string().contains("已确认"));
    assert_eq!(controls.text.string().to_string(), "第一条🙂");
    assert_eq!(target.borrow().writes, 1);

    for (from, to) in keys.iter().zip(keys.iter().cycle().skip(1)) {
        assert_eq!(unsafe { from.nextKeyView() }.as_deref(), Some(*to));
        assert_eq!(unsafe { to.previousKeyView() }.as_deref(), Some(*from));
    }
    assert_eq!(
        unsafe { controls.rows[0].nextValidKeyView() }.as_deref(),
        Some(&***controls.text)
    );
    assert!(window.makeFirstResponder(Some(&controls.text)));
    assert!(page.has_focus(&controls.text));
    let mut clipboard = Vec::new();
    for _ in 0..2 {
        page.copy_with(&source.controller, &controls.copy, |text| {
            clipboard.push(text.to_owned());
            true
        });
    }
    assert_eq!(clipboard, ["第一条🙂"]);
    assert_eq!(target.borrow().writes, 1);
    assert_eq!(controls.feedback.stringValue().to_string(), "已复制");

    for (count, text) in ["第二条", "第三条", "第四条", "第五条"]
        .into_iter()
        .enumerate()
    {
        let (generation, _) = source.begin(true, false);
        source.finish(generation, text);
        page.refresh(&source.controller);
        assert_eq!(
            controls.rows.iter().filter(|row| !row.isHidden()).count(),
            count + 2
        );
        assert_eq!(
            page.ivars().selection.borrow().as_deref(),
            Some(first_id.as_str())
        );
        assert_eq!(controls.text.string().to_string(), "第一条🙂");
    }
    let (sixth, _) = source.begin(true, false);
    source.finish(sixth, &"a".repeat(MAX_ENTRY_BYTES + 1));
    page.refresh(&source.controller);
    assert_eq!(page.ivars().entries.borrow().len(), MAX_ENTRIES);
    assert_eq!(
        controls.rows.iter().filter(|row| !row.isHidden()).count(),
        MAX_ENTRIES
    );
    assert!(
        source
            .controller
            .recent_dictation_metadata(&first_id)
            .is_none()
    );
    assert!(controls.detail.isHidden() && controls.text.string().is_empty());
    assert!(page.has_focus(&controls.keep));
    with_controller(&owner, &mut source, || click(&controls.rows[0]));
    assert!(controls.text.string().to_string().contains("超过保留上限"));
    assert!(controls.copy.isHidden());
    page.setHidden(true);
    assert!(controls.text.string().is_empty() && controls.detail.isHidden());
    with_controller(&owner, &mut source, || page.setHidden(false));
    assert!(!controls.rows[0].isHidden() && controls.detail.isHidden());

    let (in_flight, _) = source.begin(true, false);
    assert!(window.makeFirstResponder(Some(&controls.clear)));
    with_controller(&owner, &mut source, || click(&controls.clear));
    assert!(page.has_focus(&controls.keep));
    assert!(!controls.clear.isEnabled());
    assert_eq!(
        controls.empty.stringValue().to_string(),
        "本次运行暂无听写记录"
    );
    source.finish(in_flight, "清空前开始");
    assert!(
        source
            .controller
            .list_recent_dictations(MAX_ENTRIES)
            .is_empty()
    );

    let (confirming, receiver) = source.begin(true, true);
    receiver.borrow_mut().confirm = false;
    source.finish(confirming, "清空前等待确认");
    page.refresh(&source.controller);
    assert!(controls.rows[0].title().to_string().contains("等待确认"));
    with_controller(&owner, &mut source, || click(&controls.clear));
    receiver.borrow_mut().confirm = true;
    source.poll();
    page.refresh(&source.controller);
    assert_eq!(
        source
            .controller
            .gui_dictation_receipt()
            .unwrap()
            .delivery
            .state,
        voiceport_control::DeliveryState::Confirmed
    );
    assert_eq!(receiver.borrow().writes, 1);
    assert!(!controls.clear.isEnabled() && controls.rows.iter().all(|row| row.isHidden()));
    assert!(page.ivars().entries.borrow().is_empty());
    println!("workbench recent dictations: late confirmation did not restore cleared history");

    let (generation, _) = source.begin(true, false);
    source.finish(generation, "关闭前完成");
    page.refresh(&source.controller);
    with_controller(&owner, &mut source, || {
        click(&controls.rows[0]);
        assert_eq!(controls.text.string().to_string(), "关闭前完成");
        click(&controls.keep);
    });
    assert!(!source.controller.recent_dictations_enabled() && !preference(&defaults));
    assert!(controls.text.string().is_empty() && controls.detail.isHidden());
    assert_eq!(controls.empty.stringValue().to_string(), "最近听写已关闭");
    with_controller(&owner, &mut source, || click(&controls.keep));
    assert!(
        source
            .controller
            .list_recent_dictations(MAX_ENTRIES)
            .is_empty()
    );
    assert_eq!(
        controls.empty.stringValue().to_string(),
        "本次运行暂无听写记录"
    );

    let (generation, _) = source.begin(true, false);
    source.finish(generation, "退出前完成");
    page.refresh(&source.controller);
    with_controller(&owner, &mut source, || {
        click(&controls.rows[0]);
        let app = NSApplication::sharedApplication(mtm);
        let reply: NSApplicationTerminateReply =
            unsafe { msg_send![&*owner, applicationShouldTerminate: &*app] };
        assert_eq!(reply, NSApplicationTerminateReply::TerminateNow);
    });
    page.refresh(&source.controller);
    assert!(
        source
            .controller
            .list_recent_dictations(MAX_ENTRIES)
            .is_empty()
    );
    assert!(controls.text.string().is_empty() && controls.detail.isHidden());
    assert!(controls.rows.iter().all(|row| row.isHidden()));
    assert!(!window.isVisible() && !window.isKeyWindow());
    window.close();
    check_quick_panel_hide(mtm);
    println!(
        "workbench recent dictations: opt-in, empty state, five rows, receipt update, in-memory copy, clear, disable, quit and local keyboard focus"
    );
}
