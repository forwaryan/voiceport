//! A collapsed-by-default view of the controller's process-local history.
use super::*;
use crate::controller::recent_dictations::{MAX_ENTRIES, RecentDictationMetadata};
use objc2_foundation::{NSDate, NSDateFormatter, NSDateFormatterStyle};
use voiceport_control::DeliveryState;

const TOOLBAR: f64 = 36.;
const ROW: f64 = 30.;
const DETAIL: f64 = 122.;

#[cfg(debug_assertions)]
#[path = "history_fixture.rs"]
pub(super) mod fixture;

struct Row {
    summary: RecentDictationMetadata,
    button: Retained<NSButton>,
}

struct Detail {
    id: String,
    view: Retained<NSView>,
    text: Retained<NSTextView>,
    copy: Retained<NSButton>,
    feedback: Retained<NSTextField>,
    copy_attempted: Cell<bool>,
}

pub(super) struct History {
    pub(super) view: Retained<NSView>,
    disclosure: Retained<NSButton>,
    clear: Retained<NSButton>,
    empty: Retained<NSTextField>,
    expanded: Cell<bool>,
    rows: RefCell<Vec<Row>>,
    detail: RefCell<Option<Detail>>,
    actions: Retained<HistoryActions>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Weak<Delegate>]
    struct HistoryActions;
    unsafe impl NSObjectProtocol for HistoryActions {}
    impl HistoryActions {
        #[unsafe(method(toggleQuickHistory:))]
        fn toggle(&self, sender: &NSButton) {
            let Some(owner) = self.ivars().load() else { return; };
            let Some(history) = owner.active_quick_history() else { return; };
            if std::ptr::eq(&*history.disclosure, sender) {
                history.expanded.set(!history.expanded.get());
                owner.refresh();
            }
        }
        #[unsafe(method(clearQuickHistory:))]
        fn clear(&self, sender: &NSButton) {
            let Some(owner) = self.ivars().load() else { return; };
            let Some(history) = owner.active_quick_history() else { return; };
            if history.expanded.get() && sender.isEnabled() && std::ptr::eq(&*history.clear, sender) {
                owner.with_controller(|c| { c.clear_recent_dictations(); });
                focus_if_current(sender, &history.disclosure);
                owner.refresh();
            }
        }
        #[unsafe(method(selectQuickHistory:))]
        fn select(&self, sender: &NSButton) {
            let Some(owner) = self.ivars().load() else { return; };
            let Some(history) = owner.active_quick_history() else { return; };
            if let Some(c) = owner.ivars().controller.borrow().as_ref().filter(|c| !c.busy()) {
                history.select(&owner, sender, c);
            }
            owner.refresh();
        }
        #[unsafe(method(copyQuickHistory:))]
        fn copy(&self, sender: &NSButton) {
            let Some(owner) = self.ivars().load() else { return; };
            let Some(history) = owner.active_quick_history() else { return; };
            let text = owner.ivars().controller.borrow().as_ref().filter(|c| !c.busy())
                .and_then(|c| history.claim_copy(sender, c).map(str::to_owned));
            if let Some(text) = text {
                let copied = !Delivery::secure_input() && Delivery::copy(&text);
                history.show_copy_feedback(sender, copied);
            }
        }
    }
);

impl Delegate {
    fn active_quick_history(&self) -> Option<&History> {
        let panel = self.ivars().quick_panel.get()?;
        (panel.popover.isShown() && self.quick_actions_unlocked() && !panel.history.view.isHidden())
            .then_some(&panel.history)
    }
}

impl History {
    pub(super) fn new(d: &Delegate, view: Retained<NSView>) -> Self {
        let actions = HistoryActions::alloc(d.mtm()).set_ivars(Weak::from_retained(&d.retain()));
        let actions: Retained<HistoryActions> = unsafe { msg_send![super(actions), init] };
        let disclosure = button(
            d,
            &view,
            &actions,
            "最近听写",
            sel!(toggleQuickHistory:),
            "disclosure",
            rect(16., 5., 178., 28.),
        );
        disclosure.setAlignment(NSTextAlignment::Left);
        let clear = button(
            d,
            &view,
            &actions,
            "清空",
            sel!(clearQuickHistory:),
            "clear",
            rect(WIDTH - 88., 5., 72., 28.),
        );
        clear.setAccessibilityLabel(Some(ns_string!("清空最近听写")));
        let empty = d.caption(
            &view,
            "本次运行暂无听写记录",
            rect(23., 6., WIDTH - 46., 19.),
        );
        view.setHidden(true);
        Self {
            view,
            disclosure,
            clear,
            empty,
            expanded: Cell::new(false),
            rows: RefCell::new(Vec::new()),
            detail: RefCell::new(None),
            actions,
        }
    }

    pub(super) fn reset(&self) {
        self.expanded.set(false);
        self.remove_detail();
        for row in self.rows.borrow_mut().drain(..) {
            row.button.removeFromSuperview();
        }
    }

    fn remove_detail(&self) {
        if let Some(detail) = self.detail.borrow_mut().take() {
            if let Some(window) = detail.view.window()
                && window.firstResponder().is_some_and(|responder| {
                    std::ptr::eq(&*responder, &****detail.copy)
                        || std::ptr::eq(&*responder, &****detail.text)
                })
            {
                let rows = self.rows.borrow();
                let next = rows
                    .iter()
                    .find(|row| row.summary.id == detail.id)
                    .map_or(&*self.disclosure, |row| &*row.button);
                window.makeFirstResponder(Some(next));
            }
            detail.text.setString(ns_string!(""));
            detail.view.removeFromSuperview();
        }
    }

    /// Returns document height and whether AppKit's key-view chain needs relinking.
    pub(super) fn update(&self, d: &Delegate, c: &Controller, unlocked: bool) -> (f64, bool) {
        let enabled = c.recent_dictations_enabled() && !d.ivars().quitting.get();
        if !enabled {
            self.reset();
            self.view.setHidden(true);
            return (0., false);
        }
        self.view.setHidden(false);
        let expanded = self.expanded.get();
        let summaries = if expanded {
            c.list_recent_dictations(MAX_ENTRIES)
        } else {
            Vec::new()
        };
        let mut relink = false;
        if self.detail.borrow().as_ref().is_some_and(|detail| {
            !unlocked || !summaries.iter().any(|summary| summary.id == detail.id)
        }) {
            self.remove_detail();
            relink = true;
        }
        let detail = self.detail.borrow();
        let mut rows = self.rows.borrow_mut();
        let rows_changed = !rows
            .iter()
            .map(|row| &row.summary.id)
            .eq(summaries.iter().map(|summary| &summary.id));
        if rows_changed {
            for row in rows.drain(..) {
                focus_if_current(&row.button, &self.disclosure);
                row.button.removeFromSuperview();
            }
        }
        for (index, summary) in summaries.into_iter().enumerate() {
            let selected = detail
                .as_ref()
                .is_some_and(|detail| detail.id == summary.id);
            if rows_changed {
                let button = button(
                    d,
                    &self.view,
                    &self.actions,
                    "",
                    sel!(selectQuickHistory:),
                    &summary.id,
                    rect(16., 0., WIDTH - 32., 28.),
                );
                button.setAlignment(NSTextAlignment::Left);
                button.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                rows.push(Row { summary, button });
                update_row(&rows[index], selected);
            } else {
                let row = &mut rows[index];
                let detail_changed = row.button.isAccessibilityExpanded() != selected;
                relink |= detail_changed;
                if row.summary != summary || detail_changed {
                    row.summary = summary;
                    update_row(row, selected);
                }
            }
            let row = &rows[index];
            row.button.setEnabled(unlocked);
        }
        let mut y = 0.;
        for row in rows.iter().rev() {
            if let Some(detail) = detail.as_ref().filter(|detail| detail.id == row.summary.id) {
                detail.view.setFrameOrigin(NSPoint::new(0., y));
                y += DETAIL;
            }
            row.button.setFrameOrigin(NSPoint::new(16., y + 1.));
            y += ROW;
        }
        if expanded && rows.is_empty() {
            y = ROW;
        }
        self.empty.setHidden(!expanded || !rows.is_empty());
        self.clear.setHidden(!expanded);
        self.clear.setEnabled(unlocked && !rows.is_empty());
        self.clear.setFrameOrigin(NSPoint::new(WIDTH - 88., y + 4.));
        self.disclosure.setEnabled(unlocked);
        self.disclosure.setFrameOrigin(NSPoint::new(16., y + 4.));
        let title = if expanded {
            ns_string!("收起最近听写")
        } else {
            ns_string!("最近听写")
        };
        if *self.disclosure.title() != *title {
            self.disclosure.setTitle(title);
        }
        self.disclosure.setAccessibilityExpanded(expanded);
        (y + TOOLBAR, relink || rows_changed)
    }

    pub(super) fn append_key_views(&self, views: &mut Vec<Retained<NSView>>) {
        if self.view.isHidden() {
            return;
        }
        views.push(self.disclosure.clone().into_super().into_super());
        if self.expanded.get() {
            views.push(self.clear.clone().into_super().into_super());
            let detail = self.detail.borrow();
            for row in self.rows.borrow().iter() {
                views.push(row.button.clone().into_super().into_super());
                if let Some(detail) = detail.as_ref().filter(|detail| detail.id == row.summary.id) {
                    if !detail.copy.isHidden() {
                        views.push(detail.copy.clone().into_super().into_super());
                    }
                    views.push(detail.text.clone().into_super().into_super());
                }
            }
        }
    }

    fn select(&self, d: &Delegate, sender: &NSButton, c: &Controller) {
        if !self.expanded.get() || !sender.isEnabled() || !c.recent_dictations_enabled() {
            return;
        }
        let id = self
            .rows
            .borrow()
            .iter()
            .find(|row| std::ptr::eq(&*row.button, sender))
            .map(|row| row.summary.id.clone());
        let Some(id) = id else {
            return;
        };
        // Revalidate the selected ID even when the old row was left onscreen.
        if c.recent_dictation_metadata(&id).is_none() {
            return;
        }
        let text = c.read_recent_dictation(&id);
        let selected = self
            .detail
            .borrow()
            .as_ref()
            .is_some_and(|detail| detail.id == id);
        self.remove_detail();
        if !selected {
            *self.detail.borrow_mut() = Some(Detail::new(d, self, id, text));
        }
    }

    fn claim_copy<'a>(&self, sender: &NSButton, c: &'a Controller) -> Option<&'a str> {
        let detail = self.detail.borrow();
        let detail = detail.as_ref()?;
        if !self.expanded.get()
            || c.busy()
            || !sender.isEnabled()
            || !std::ptr::eq(&*detail.copy, sender)
        {
            return None;
        }
        let text = c.read_recent_dictation(&detail.id)?;
        if detail.copy_attempted.replace(true) {
            return None;
        }
        Some(text)
    }

    fn show_copy_feedback(&self, sender: &NSButton, copied: bool) {
        let detail = self.detail.borrow();
        if let Some(detail) = detail
            .as_ref()
            .filter(|detail| std::ptr::eq(&*detail.copy, sender))
        {
            detail.copy.setEnabled(false);
            let feedback = if copied {
                "已复制"
            } else {
                "未复制，请重新展开重试"
            };
            set_label_text(&detail.feedback, feedback);
            detail
                .copy
                .setAccessibilityHelp(Some(&NSString::from_str(feedback)));
        }
    }
}

impl Detail {
    fn new(d: &Delegate, history: &History, id: String, text: Option<&str>) -> Self {
        let mtm = d.mtm();
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., WIDTH, DETAIL));
        history.view.addSubview(&view);
        let copy = button(
            d,
            &view,
            &history.actions,
            "复制",
            sel!(copyQuickHistory:),
            "copy",
            rect(16., 3., 78., 27.),
        );
        copy.setAccessibilityLabel(Some(ns_string!("复制这条听写")));
        copy.setHidden(text.is_none());
        let feedback = d.caption(&view, "", rect(101., 7., WIDTH - 121., 18.));
        feedback
            .setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.history.copy-feedback")));
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(22., 33., WIDTH - 44., DETAIL - 37.),
        );
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        let body = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            rect(0., 0., scroll.contentSize().width, DETAIL - 37.),
        );
        body.setEditable(false);
        body.setSelectable(true);
        body.setRichText(false);
        body.setVerticallyResizable(true);
        body.setHorizontallyResizable(false);
        if let Some(container) = unsafe { body.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        body.setFont(Some(&NSFont::systemFontOfSize(13.)));
        body.setTextColor(Some(&NSColor::textColor()));
        body.setDrawsBackground(false);
        body.setAccessibilityLabel(Some(ns_string!("这条听写的正文")));
        body.setString(&NSString::from_str(
            text.unwrap_or("文字超过 16 KB，本条仅保留时间与投递状态。"),
        ));
        scroll.setDocumentView(Some(&body));
        view.addSubview(&scroll);
        Self {
            id,
            view,
            text: body,
            copy,
            feedback,
            copy_attempted: Cell::new(false),
        }
    }
}

fn button(
    d: &Delegate,
    parent: &NSView,
    actions: &HistoryActions,
    title: &str,
    action: objc2::runtime::Sel,
    id: &str,
    frame: NSRect,
) -> Retained<NSButton> {
    let button = panel_button(d, parent, title, action, frame);
    unsafe { button.setTarget(Some(actions)) };
    button.setAccessibilityIdentifier(Some(&NSString::from_str(&format!(
        "voiceport.quick.history.{id}"
    ))));
    button
}

fn focus_if_current(sender: &NSButton, next: &NSButton) {
    if let Some(window) = sender.window()
        && window.firstResponder().as_deref() == Some(&****sender)
    {
        window.makeFirstResponder(Some(next));
    }
}

fn update_row(row: &Row, selected: bool) {
    let date =
        NSDate::dateWithTimeIntervalSince1970(row.summary.created_at_unix_ms as f64 / 1_000.);
    let date = NSDateFormatter::localizedStringFromDate_dateStyle_timeStyle(
        &date,
        NSDateFormatterStyle::ShortStyle,
        NSDateFormatterStyle::MediumStyle,
    );
    let receipt = row.summary.delivery;
    let status = match (receipt.state, receipt.stopped) {
        (DeliveryState::NotSent, true) => "投递已停止",
        (DeliveryState::NotSent, false) => "未投递",
        (DeliveryState::Waiting, _) => "等待投递",
        (DeliveryState::AwaitingConfirmation, _) => "等待确认",
        (DeliveryState::PartiallySent, _) => "部分投递",
        (DeliveryState::Confirmed, _) => "已确认",
        (DeliveryState::Submitted, _) => "已提交，未验证",
        (DeliveryState::Uncertain, _) => "结果不明",
    };
    let title = NSString::from_str(&format!(
        "{} {date} · {status}",
        if selected { "▾" } else { "▸" }
    ));
    row.button.setTitle(&title);
    row.button
        .setAccessibilityLabel(Some(&NSString::from_str(&format!("{date}，{status}"))));
    row.button.setAccessibilityExpanded(selected);
    row.button.setToolTip(Some(&title));
}
