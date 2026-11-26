//! Observe AppKit view preparation without presenting a menu or popover.
use super::*;

thread_local! {
    static HIDDEN_REFRESHES: Cell<Option<u64>> = const { Cell::new(None) };
}

pub(in crate::app) fn prepare_detached(owner: &Delegate) {
    owner
        .ivars()
        .quick_panel
        .get_or_init(|| owner.build_quick_panel());
}

pub(in crate::app) fn refresh_detached(owner: &Delegate) {
    let panel = owner.ivars().quick_panel.get().unwrap();
    assert!(!panel.popover.isShown());
    owner.refresh();
    owner.prepare_quick_panel(panel);
    panel.scroll_to_top();
    assert!(panel.open.isEnabled());
    assert!(panel.open.window().is_none());
}

pub(in crate::app) fn start_counting_detached() {
    HIDDEN_REFRESHES.set(Some(0));
}

pub(in crate::app) fn finish_counting_detached() -> u64 {
    HIDDEN_REFRESHES.take().unwrap()
}

pub(super) fn count_hidden_refresh() {
    if let Some(count) = HIDDEN_REFRESHES.get() {
        HIDDEN_REFRESHES.set(Some(count + 1));
    }
}
