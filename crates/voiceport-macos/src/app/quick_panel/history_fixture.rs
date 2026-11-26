//! Offscreen history validation. The clipboard sink and target are in memory.
use super::*;
use crate::controller::recent_dictations::{MAX_ENTRY_BYTES, fixture::SyntheticDictations};
use crate::delivery::model::{Failure, TextRange};
use std::path::Path;

pub(in crate::app::quick_panel) fn run(
    panel: &QuickPanel,
    d: &Delegate,
    window: &NSWindow,
    output: &Path,
) {
    fn refresh(panel: &QuickPanel, d: &Delegate, c: &Controller) {
        panel.refresh_contents(d, c, "就绪", "⌃⌥空格", !c.busy());
    }

    let mut source = SyntheticDictations::new();
    let history = &panel.history;
    let (disabled, _) = source.begin(true, false);
    source.controller.set_recent_dictations_enabled(true);
    source.finish(disabled, "开始时未启用");
    assert!(source.controller.list_recent_dictations(5).is_empty());

    let (first, first_target) = source.begin(true, true);
    source.finish_at(first, "第一段🙂", 1_793_658_240_000);
    let first_summary = source.controller.list_recent_dictations(1).remove(0);
    assert_eq!(first_summary.delivery.state, DeliveryState::Confirmed);
    assert_eq!(first_target.borrow().snapshot.text, "A第一段🙂B");
    assert_eq!(
        first_target.borrow().snapshot.selection,
        TextRange {
            location: 6,
            length: 0
        }
    );
    assert_eq!(first_target.borrow().writes, 1);

    let (second, second_target) = source.begin(true, true);
    second_target.borrow_mut().confirm = false;
    source.finish(first, "迟到的第一段");
    assert_eq!(
        source.controller.list_recent_dictations(5).as_slice(),
        std::slice::from_ref(&first_summary)
    );
    source.finish_at(second, "第二段", 1_793_658_300_000);
    let pending = source.controller.list_recent_dictations(5);
    assert_eq!(pending.len(), 2);
    assert_eq!(
        pending[0].delivery.state,
        DeliveryState::AwaitingConfirmation
    );
    assert_eq!(pending[0].delivery.confirmed_operations, 0);
    assert_eq!(pending[1], first_summary);
    source.finish(second, "重复的第二段");
    assert_eq!(source.controller.list_recent_dictations(5), pending);
    second_target.borrow_mut().confirm = true;
    source.poll();
    let confirmed = source.controller.list_recent_dictations(5);
    assert_eq!(confirmed[0].id, pending[0].id);
    assert_eq!(confirmed[0].delivery.state, DeliveryState::Confirmed);
    assert_eq!(confirmed[0].delivery.submitted_operations, 1);
    assert_eq!(confirmed[0].delivery.confirmed_operations, 1);
    assert_eq!(confirmed[1], first_summary);
    assert_eq!(second_target.borrow().snapshot.text, "A第二段B");
    let json = serde_json::to_string(&confirmed).unwrap();
    assert!(!json.contains("第一段") && !json.contains("第二段"));
    assert!(uuid::Uuid::parse_str(&first_summary.id).is_ok());
    assert!(first_summary.created_at_unix_ms > 0);

    panel.screen_height.set(260.);
    refresh(panel, d, &source.controller);
    assert!(history.detail.borrow().is_none());
    assert_eq!(history.disclosure.title().to_string(), "最近听写");
    assert!(!history.disclosure.isAccessibilityExpanded());
    history.expanded.set(true);
    refresh(panel, d, &source.controller);
    let (newest_button, first_button) = {
        let rows = history.rows.borrow();
        assert_eq!(rows[1].summary.id, first_summary.id);
        (rows[0].button.clone(), rows[1].button.clone())
    };
    assert_eq!(
        unsafe { history.disclosure.nextKeyView() }.as_deref(),
        Some(&***history.clear)
    );
    assert_eq!(
        unsafe { history.clear.nextKeyView() }.as_deref(),
        Some(&***newest_button)
    );
    assert_eq!(
        unsafe { newest_button.nextKeyView() }.as_deref(),
        Some(&***first_button)
    );
    assert!(NSAccessibility::accessibilityLabel(&*history.clear).is_some());
    assert!(!first_button.isAccessibilityExpanded());
    history.select(d, &first_button, &source.controller);
    refresh(panel, d, &source.controller);
    let (first_copy, old_text) = {
        let detail = history.detail.borrow();
        let detail = detail.as_ref().unwrap();
        assert_eq!(detail.text.string().to_string(), "第一段🙂");
        (detail.copy.clone(), detail.text.clone())
    };
    assert!(first_button.isAccessibilityExpanded());
    assert_eq!(
        unsafe { first_button.nextKeyView() }.as_deref(),
        Some(&***first_copy)
    );
    assert!(window.makeFirstResponder(Some(&first_copy)));
    super::super::fixture::check_viewport(panel, &first_copy);
    assert_eq!(panel.scroll.bounds().size.height, 236.);
    let mut clipboard = Vec::new();
    for _ in 0..2 {
        if let Some(text) = history.claim_copy(&first_copy, &source.controller) {
            clipboard.push(text.to_owned());
            history.show_copy_feedback(&first_copy, true);
        }
    }
    assert_eq!(clipboard, ["第一段🙂"]);
    assert_eq!(first_target.borrow().writes, 1);
    assert_eq!(second_target.borrow().writes, 1);
    assert_eq!(source.controller.transcript, "第二段");
    let receipt = source.controller.gui_dictation_receipt().unwrap();
    let snapshot = dictation_page::Snapshot {
        attempt: Some(dictation_page::ResultDetails {
            receipt: &receipt,
            diagnostics: None,
            source: None,
            target: None,
        }),
        retained: None,
        origin: Some(dictation_page::TranscriptOrigin {
            generation: second,
            final_text: true,
        }),
        original: &source.controller.transcript,
        raw_available: false,
        can_open_raw: false,
        shortcut: "⌃⌥空格",
        activity: dictation_page::Activity {
            title: None,
            recording: false,
            can_toggle: true,
            can_cancel: false,
            can_retry: false,
        },
    };
    super::super::fixture::result_scene(d, snapshot, output, |page| {
        page.toggle_edit();
        assert!(page.result.isEditable());
        page.result.setString(ns_string!("只保留在临时编辑控件"));
        page.copy(|copy| {
            assert_eq!(copy, "只保留在临时编辑控件");
            true
        });
    });
    assert_eq!(
        source.controller.read_recent_dictation(&confirmed[0].id),
        Some("第二段")
    );
    assert_eq!(source.controller.list_recent_dictations(5), confirmed);
    super::super::fixture::capture(panel, window, output, "dark-small-history.png");
    println!(
        "recent-history: two real Controller finals, one old-entry memory copy, one target write each, delayed receipt retains ID and metadata contains no text"
    );

    history.select(d, &newest_button, &source.controller);
    refresh(panel, d, &source.controller);
    let second_copy = history.detail.borrow().as_ref().unwrap().copy.clone();
    assert_eq!(
        unsafe { newest_button.nextKeyView() }.as_deref(),
        Some(&***second_copy)
    );
    assert!(
        history
            .claim_copy(&first_copy, &source.controller)
            .is_none()
    );
    assert_eq!(old_text.string().to_string(), "");
    assert_ne!(first_copy, second_copy);

    let (selection_changed, target) = source.begin(true, true);
    target.borrow_mut().snapshot.selection = TextRange {
        location: 2,
        length: 0,
    };
    source.finish(selection_changed, "保留未投递文字");
    let missed = source.controller.list_recent_dictations(1).remove(0);
    assert_eq!(missed.delivery.state, DeliveryState::NotSent);
    assert_eq!(missed.failure_stage.as_deref(), Some("ownership"));
    assert_eq!(
        source.controller.read_recent_dictation(&missed.id),
        Some("保留未投递文字")
    );
    assert_eq!(target.borrow().writes, 0);
    assert_eq!(target.borrow().snapshot.text, "A|B");

    let (unknown, target) = source.begin(true, true);
    target.borrow_mut().confirm = false;
    source.finish(unknown, "保留未知结果");
    let uncertain_id = source.controller.list_recent_dictations(1).remove(0).id;
    target.borrow_mut().failure = Some(Failure::FocusElementChanged);
    target.borrow_mut().confirm = true;
    source.poll();
    let uncertain = source.controller.list_recent_dictations(1).remove(0);
    assert_eq!(uncertain.id, uncertain_id);
    assert_eq!(uncertain.delivery.state, DeliveryState::Uncertain);
    assert_eq!(target.borrow().writes, 1);

    let unchanged = source.controller.list_recent_dictations(5);
    let (late_sensitive, target) = source.begin(true, true);
    target.borrow_mut().confirm = false;
    source.finish(late_sensitive, "后续检查前的文字");
    let provisional = source.controller.list_recent_dictations(1).remove(0);
    assert_eq!(
        provisional.delivery.state,
        DeliveryState::AwaitingConfirmation
    );
    target.borrow_mut().failure = Some(Failure::Sensitive);
    target.borrow_mut().confirm = true;
    source.poll();
    assert!(
        source
            .controller
            .recent_dictation_metadata(&provisional.id)
            .is_none()
    );
    assert_eq!(source.controller.list_recent_dictations(5), unchanged);
    let (empty, _) = source.begin(true, false);
    source.finish(empty, " \n");
    let (background, _) = source.begin(false, false);
    source.finish(background, "后台操作文字");
    let (cancelled, _) = source.begin(true, false);
    source.controller.cancel();
    source.finish(cancelled, "取消后迟到的文字");
    let (secure, target) = source.begin(true, true);
    target.borrow_mut().failure = Some(Failure::SecureInput);
    source.finish(secure, "安全输入结果");
    let (during_secure, _) = source.begin(true, false);
    source.set_focus_safe(false);
    source.poll();
    source.set_focus_safe(true);
    source.finish(during_secure, "中途出现安全输入");
    source.set_focus_safe(false);
    let (secure_start, _) = source.begin(true, false);
    source.set_focus_safe(true);
    source.finish(secure_start, "在安全输入中开始");
    assert_eq!(source.controller.list_recent_dictations(5), unchanged);
    println!(
        "recent-history: failed and uncertain deliveries remain with actual receipt, changed selection gets no write, blank/non-GUI/cancelled/sensitive completions excluded"
    );

    refresh(panel, d, &source.controller);
    let newest_row = history.rows.borrow()[0].button.clone();
    history.select(d, &newest_row, &source.controller);
    let stale_copy = history.detail.borrow().as_ref().unwrap().copy.clone();
    for index in 0..MAX_ENTRIES {
        let (generation, _) = source.begin(true, false);
        source.finish(generation, &format!("新文字 {index}"));
    }
    assert_eq!(
        source.controller.list_recent_dictations(usize::MAX).len(),
        MAX_ENTRIES
    );
    assert!(source.controller.list_recent_dictations(0).is_empty());
    assert!(
        source
            .controller
            .recent_dictation_metadata(&uncertain.id)
            .is_none()
    );
    assert!(
        history
            .claim_copy(&stale_copy, &source.controller)
            .is_none()
    );
    history.select(d, &newest_row, &source.controller);
    assert!(
        history
            .claim_copy(&stale_copy, &source.controller)
            .is_none()
    );
    refresh(panel, d, &source.controller);
    assert!(history.detail.borrow().is_none());
    assert!(unsafe { newest_row.superview() }.is_none());

    let full = "🙂".repeat(MAX_ENTRY_BYTES / 4);
    for _ in 0..4 {
        let (generation, _) = source.begin(true, false);
        source.finish(generation, &full);
    }
    let within_budget = source.controller.list_recent_dictations(5);
    assert_eq!(within_budget.len(), 3);
    assert!(
        within_budget
            .iter()
            .all(|entry| source.controller.read_recent_dictation(&entry.id) == Some(full.as_str()))
    );
    let (oversize, _) = source.begin(true, false);
    source.finish(oversize, &format!("{full}字"));
    let oversize = source.controller.list_recent_dictations(1).remove(0);
    assert!(!oversize.text_available);
    assert!(
        source
            .controller
            .read_recent_dictation(&oversize.id)
            .is_none()
    );
    refresh(panel, d, &source.controller);
    let oversize_button = history.rows.borrow()[0].button.clone();
    history.select(d, &oversize_button, &source.controller);
    let (oversize_copy, oversized_body) = {
        let detail = history.detail.borrow();
        let detail = detail.as_ref().unwrap();
        (detail.copy.clone(), detail.text.clone())
    };
    assert!(oversize_copy.isHidden());
    assert!(oversized_body.string().to_string().contains("超过 16 KB"));
    assert!(
        history
            .claim_copy(&oversize_copy, &source.controller)
            .is_none()
    );

    let (in_flight, _) = source.begin(true, false);
    assert_eq!(source.controller.clear_recent_dictations(), 4);
    source.finish(in_flight, "清空前开始");
    assert!(source.controller.list_recent_dictations(5).is_empty());
    refresh(panel, d, &source.controller);
    assert!(!history.empty.isHidden() && !history.clear.isEnabled());
    assert_eq!(oversized_body.string().to_string(), "");
    let (generation, _) = source.begin(true, false);
    source.finish(generation, "关闭前一条");
    refresh(panel, d, &source.controller);
    let current_row = history.rows.borrow()[0].button.clone();
    history.select(d, &current_row, &source.controller);
    let current_copy = history.detail.borrow().as_ref().unwrap().copy.clone();
    source.controller.set_recent_dictations_enabled(false);
    assert!(
        history
            .claim_copy(&current_copy, &source.controller)
            .is_none()
    );
    refresh(panel, d, &source.controller);
    assert!(history.view.isHidden());
    source.controller.set_recent_dictations_enabled(true);
    assert!(source.controller.list_recent_dictations(5).is_empty());
    refresh(panel, d, &source.controller);
    assert!(!history.view.isHidden() && !history.expanded.get());
    let (after_opt_in, _) = source.begin(true, false);
    source.finish(after_opt_in, "重新开启的有效结果");
    let fresh_instance = Controller::new();
    assert!(!fresh_instance.recent_dictations_enabled());
    assert!(fresh_instance.list_recent_dictations(5).is_empty());
    *d.ivars().controller.borrow_mut() = Some(source.controller);
    let app = NSApplication::sharedApplication(d.mtm());
    let reply: NSApplicationTerminateReply =
        unsafe { msg_send![d, applicationShouldTerminate: &*app] };
    assert_eq!(reply, NSApplicationTerminateReply::TerminateNow);
    let slot = d.ivars().controller.borrow();
    let stopped = slot.as_ref().unwrap();
    assert!(!stopped.recent_dictations_enabled());
    assert!(stopped.list_recent_dictations(5).is_empty());
    refresh(panel, d, stopped);
    assert!(history.view.isHidden());
    assert!(history.claim_copy(&current_copy, stopped).is_none());
    drop(slot);
    d.ivars().controller.borrow_mut().take();
    d.ivars().quitting.set(false);
    println!(
        "recent-history: count 5, 16 KiB entry and 48 KiB text budget, whole or unavailable, stale control rejection, in-flight clear, opt-out and fresh instance"
    );
    crate::app::recent_dictations::workbench::run_fixture(d.mtm());
}
