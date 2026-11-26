//! Native workbench page for the controller's process-local recent dictations.
use super::*;
use crate::controller::recent_dictations::MAX_ENTRIES;
use objc2::{Message, rc::Weak};

#[cfg(debug_assertions)]
pub(in crate::app) mod fixture;
#[cfg(debug_assertions)]
pub(in crate::app) use fixture::run as run_fixture;

struct Controls {
    keep: Retained<NSButton>,
    clear: Retained<NSButton>,
    empty: Retained<NSTextField>,
    rows: [Retained<NSButton>; MAX_ENTRIES],
    detail: Retained<NSScrollView>,
    text: Retained<NSTextView>,
    copy: Retained<NSButton>,
    feedback: Retained<NSTextField>,
}

pub(in crate::app) struct Ivars {
    owner: Weak<Delegate>,
    controls: OnceCell<Controls>,
    entries: RefCell<Vec<RecentDictationMetadata>>,
    selection: RefCell<Option<String>>,
    copy_attempted: Cell<bool>,
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    pub(in crate::app) struct HistoryPage;

    impl HistoryPage {
        #[unsafe(method(viewDidHide))]
        fn view_did_hide(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidHide] };
            if self.ivars().controls.get().is_some() {
                self.clear_display();
            }
        }

        #[unsafe(method(viewDidUnhide))]
        fn view_did_unhide(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidUnhide] };
            if self.ivars().controls.get().is_some() {
                self.refresh_from_owner();
            }
        }

        #[unsafe(method(keepWorkbenchDictations:))]
        fn keep(&self, sender: &NSButton) {
            let Some(owner) = self.action_owner() else { return; };
            if std::ptr::eq(&*self.controls().keep, sender) && sender.isEnabled() {
                owner.set_recent_dictations_enabled(sender.state() == NSControlStateValueOn);
                self.refresh_from_owner();
            }
        }

        #[unsafe(method(clearWorkbenchDictations:))]
        fn clear(&self, sender: &NSButton) {
            let Some(owner) = self.action_owner() else { return; };
            if std::ptr::eq(&*self.controls().clear, sender) && sender.isEnabled() {
                owner.with_controller(|c| { c.clear_recent_dictations(); });
                self.refresh_from_owner();
            }
        }

        #[unsafe(method(selectWorkbenchDictation:))]
        fn select(&self, sender: &NSButton) {
            let Some(owner) = self.action_owner() else { return; };
            if let Ok(slot) = owner.ivars().controller.try_borrow()
                && let Some(c) = slot.as_ref()
            {
                self.select_row(c, sender);
            }
        }

        #[unsafe(method(copyWorkbenchDictation:))]
        fn copy(&self, sender: &NSButton) {
            let Some(owner) = self.action_owner() else { return; };
            if let Ok(slot) = owner.ivars().controller.try_borrow()
                && let Some(c) = slot.as_ref()
            {
                self.copy_with(c, sender, |text| !Delivery::secure_input() && Delivery::copy(text));
            }
        }
    }
);

impl HistoryPage {
    pub(in crate::app) fn build(owner: &Delegate, frame: NSRect) -> Retained<Self> {
        let mtm = owner.mtm();
        let page = Self::alloc(mtm).set_ivars(Ivars {
            owner: Weak::from_retained(&owner.retain()),
            controls: OnceCell::new(),
            entries: RefCell::new(Vec::new()),
            selection: RefCell::new(None),
            copy_attempted: Cell::new(false),
        });
        let page: Retained<Self> = unsafe { msg_send![super(page), initWithFrame: frame] };
        let width = frame.size.width;
        let height = frame.size.height;
        let top = NSAutoresizingMaskOptions::ViewMinYMargin;
        let wide = NSAutoresizingMaskOptions::ViewWidthSizable;
        page.setAutoresizingMask(wide | NSAutoresizingMaskOptions::ViewHeightSizable);
        page.setAccessibilityIdentifier(Some(ns_string!("voiceport.workbench.history")));
        let title = owner.label(
            &page,
            "最近听写",
            rect(16., height - 38., width - 32., 24.),
            17.,
        );
        title.setFont(Some(&NSFont::systemFontOfSize_weight(17., unsafe {
            NSFontWeightSemibold
        })));
        title.setAutoresizingMask(top | wide);
        owner
            .caption(
                &page,
                "本次运行最多在内存保留五条听写，退出时清空。",
                rect(16., height - 58., width - 32., 18.),
            )
            .setAutoresizingMask(top | wide);
        let keep = unsafe {
            NSButton::checkboxWithTitle_target_action(
                ns_string!("保留最近听写"),
                Some(&*page),
                Some(sel!(keepWorkbenchDictations:)),
                mtm,
            )
        };
        keep.setFrame(rect(16., height - 91., 152., 24.));
        keep.setAutoresizingMask(top);
        keep.setAccessibilityIdentifier(Some(ns_string!("voiceport.workbench.history.keep")));
        page.addSubview(&keep);
        let clear = page.button(
            "清空",
            sel!(clearWorkbenchDictations:),
            "clear",
            rect(width - 100., height - 93., 84., 28.),
        );
        clear.setAutoresizingMask(top | NSAutoresizingMaskOptions::ViewMinXMargin);
        clear.setAccessibilityLabel(Some(ns_string!("清空最近听写")));
        let empty = owner.label(&page, "", rect(16., height - 128., width - 32., 24.), 13.);
        empty.setTextColor(Some(&NSColor::secondaryLabelColor()));
        empty.setAutoresizingMask(top | wide);
        let rows = std::array::from_fn(|index| {
            let row = page.button(
                "",
                sel!(selectWorkbenchDictation:),
                &format!("row-{index}"),
                rect(16., height - 128. - 29. * index as f64, width - 32., 28.),
            );
            row.setAutoresizingMask(top | wide);
            row.setButtonType(NSButtonType::PushOnPushOff);
            row.setAlignment(NSTextAlignment::Left);
            row.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
            row.setHidden(true);
            row
        });
        let detail_height = height - 294.;
        let detail = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(16., 48., width - 32., detail_height),
        );
        detail.setAutoresizingMask(wide | NSAutoresizingMaskOptions::ViewHeightSizable);
        detail.setHasVerticalScroller(true);
        detail.setAutohidesScrollers(true);
        detail.setBorderType(NSBorderType::BezelBorder);
        let text = NSTextView::initWithFrame(
            NSTextView::alloc(mtm),
            rect(0., 0., detail.contentSize().width, detail_height),
        );
        text.setAutoresizingMask(wide);
        text.setEditable(false);
        text.setSelectable(true);
        text.setRichText(false);
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(false);
        if let Some(container) = unsafe { text.textContainer() } {
            container.setWidthTracksTextView(true);
        }
        text.setFont(Some(&NSFont::systemFontOfSize(13.)));
        text.setTextColor(Some(&NSColor::textColor()));
        text.setAccessibilityLabel(Some(ns_string!("这条听写的正文")));
        detail.setDocumentView(Some(&text));
        page.addSubview(&detail);
        detail.setHidden(true);
        let copy = page.button(
            "复制",
            sel!(copyWorkbenchDictation:),
            "copy",
            rect(16., 12., 84., 28.),
        );
        copy.setAccessibilityLabel(Some(ns_string!("复制这条听写")));
        copy.setHidden(true);
        let feedback = owner.caption(&page, "", rect(112., 17., width - 128., 18.));
        feedback.setAutoresizingMask(wide);
        let _ = page.ivars().controls.set(Controls {
            keep,
            clear,
            empty,
            rows,
            detail,
            text,
            copy,
            feedback,
        });
        page.refresh_from_owner();
        page
    }

    pub(in crate::app) fn key_views(&self) -> Vec<&NSView> {
        let controls = self.controls();
        let mut views: Vec<&NSView> = vec![&controls.keep, &controls.clear];
        views.extend(controls.rows.iter().map(|row| &****row));
        views.extend([&*controls.text as &NSView, &controls.copy]);
        views
    }

    fn button(
        &self,
        title: &str,
        action: objc2::runtime::Sel,
        id: &str,
        frame: NSRect,
    ) -> Retained<NSButton> {
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(self),
                Some(action),
                self.mtm(),
            )
        };
        button.setFrame(frame);
        button.setAccessibilityIdentifier(Some(&NSString::from_str(&format!(
            "voiceport.workbench.history.{id}"
        ))));
        self.addSubview(&button);
        button
    }

    fn controls(&self) -> &Controls {
        self.ivars().controls.get().expect("built history page")
    }

    fn action_owner(&self) -> Option<Retained<Delegate>> {
        let owner = self.ivars().owner.load()?;
        (!owner.ivars().quitting.get()
            && !self.isHiddenOrHasHiddenAncestor()
            && self.window().is_some())
        .then_some(owner)
    }

    fn refresh_from_owner(&self) {
        let Some(owner) = self.ivars().owner.load() else {
            return;
        };
        if let Ok(slot) = owner.ivars().controller.try_borrow()
            && let Some(c) = slot.as_ref()
        {
            self.refresh(c);
        }
    }

    pub(in crate::app) fn refresh(&self, c: &Controller) {
        if self.isHiddenOrHasHiddenAncestor() {
            return;
        }
        let controls = self.controls();
        let enabled = c.recent_dictations_enabled();
        let quitting = self
            .ivars()
            .owner
            .load()
            .is_none_or(|owner| owner.ivars().quitting.get());
        controls.keep.setEnabled(!quitting);
        controls.keep.setState(if enabled && !quitting {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        if quitting {
            self.clear_display();
            return;
        }
        let summaries = c.list_recent_dictations(MAX_ENTRIES);
        let mut entries = self.ivars().entries.borrow_mut();
        if *entries != summaries {
            let focused_id = entries
                .iter()
                .zip(&controls.rows)
                .find_map(|(entry, row)| self.has_focus(row).then_some(&entry.id));
            for (index, row) in controls.rows.iter().enumerate() {
                let summary = summaries.get(index);
                if summary != entries.get(index) {
                    if let Some(summary) = summary {
                        let (date, status) = row_presentation(summary);
                        row.setTitle(&NSString::from_str(&format!("{date} · {status}")));
                    } else {
                        row.setTitle(ns_string!(""));
                    }
                }
                row.setHidden(summary.is_none());
            }
            if let Some(id) = focused_id {
                let next = summaries
                    .iter()
                    .position(|summary| &summary.id == id)
                    .map_or(&controls.keep, |index| &controls.rows[index]);
                self.focus(next);
            }
            *entries = summaries;
        }
        let can_clear = c.can_clear_recent_dictations();
        if !can_clear && self.has_focus(&controls.clear) {
            self.focus(&controls.keep);
        }
        controls.clear.setEnabled(can_clear);
        controls.empty.setHidden(!entries.is_empty());
        let empty_text = if enabled {
            ns_string!("本次运行暂无听写记录")
        } else {
            ns_string!("最近听写已关闭")
        };
        if *controls.empty.stringValue() != *empty_text {
            controls.empty.setStringValue(empty_text);
        }
        self.refresh_selection(c, &entries);
    }

    fn refresh_selection(&self, c: &Controller, entries: &[RecentDictationMetadata]) {
        let mut selection = self.ivars().selection.borrow_mut();
        let controls = self.controls();
        let index = selection
            .as_ref()
            .and_then(|id| entries.iter().position(|summary| &summary.id == id));
        for (position, row) in controls.rows.iter().enumerate().take(entries.len()) {
            let selected = index == Some(position);
            let state = if selected {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            };
            if row.state() != state {
                row.setState(state);
            }
            if row.isAccessibilityExpanded() != selected {
                row.setAccessibilityExpanded(selected);
            }
        }
        let Some(index) = index else {
            if self.has_focus(&controls.text) || self.has_focus(&controls.copy) {
                self.focus(&controls.keep);
            }
            selection.take();
            self.hide_detail();
            return;
        };
        // A receipt-only update leaves the visible text, selection and copy feedback intact.
        if controls.detail.isHidden() {
            let text = c.read_recent_dictation(&entries[index].id);
            controls.text.setString(&NSString::from_str(
                text.unwrap_or("这条听写的文字超过保留上限，仅显示时间和投递状态。"),
            ));
            controls.detail.setHidden(false);
            controls.copy.setHidden(text.is_none());
        }
        let can_copy = !controls.copy.isHidden() && !c.busy() && !self.ivars().copy_attempted.get();
        if !can_copy && self.has_focus(&controls.copy) {
            self.focus(&controls.text);
        }
        controls.copy.setEnabled(can_copy);
    }

    pub(in crate::app) fn clear_display(&self) {
        let controls = self.controls();
        self.ivars().entries.borrow_mut().clear();
        for row in &controls.rows {
            row.setTitle(ns_string!(""));
            row.setHidden(true);
        }
        controls.clear.setEnabled(false);
        self.ivars().selection.borrow_mut().take();
        self.hide_detail();
    }

    fn hide_detail(&self) {
        let controls = self.controls();
        if controls.detail.isHidden() {
            return;
        }
        controls.text.setString(ns_string!(""));
        controls.detail.setHidden(true);
        controls.copy.setHidden(true);
        controls.feedback.setStringValue(ns_string!(""));
    }

    fn select_row(&self, c: &Controller, sender: &NSButton) {
        let controls = self.controls();
        let entries = self.ivars().entries.borrow();
        let id = controls
            .rows
            .iter()
            .zip(entries.iter())
            .find_map(|(row, summary)| {
                (std::ptr::eq(&**row, sender) && !row.isHidden()).then_some(&summary.id)
            });
        let Some(id) = id.filter(|id| c.recent_dictation_metadata(id).is_some()) else {
            return;
        };
        let reselect = self.ivars().selection.borrow().as_ref() == Some(id);
        self.hide_detail();
        self.ivars().copy_attempted.set(false);
        *self.ivars().selection.borrow_mut() = (!reselect).then(|| id.clone());
        self.refresh_selection(c, &entries);
    }

    fn copy_with(&self, c: &Controller, sender: &NSButton, copy: impl FnOnce(&str) -> bool) {
        let controls = self.controls();
        if c.busy()
            || !std::ptr::eq(&*controls.copy, sender)
            || sender.isHidden()
            || !sender.isEnabled()
        {
            return;
        }
        let selection = self.ivars().selection.borrow();
        let Some(text) = selection
            .as_ref()
            .and_then(|id| c.read_recent_dictation(id))
        else {
            return;
        };
        if self.ivars().copy_attempted.replace(true) {
            return;
        }
        let copied = copy(text);
        controls.copy.setEnabled(false);
        controls.feedback.setStringValue(if copied {
            ns_string!("已复制")
        } else {
            ns_string!("未复制，请重新选择后重试")
        });
        if self.has_focus(&controls.copy) {
            self.focus(&controls.text);
        }
    }

    fn has_focus(&self, view: &NSView) -> bool {
        self.window()
            .and_then(|window| window.firstResponder())
            .as_deref()
            == Some(&**view)
    }

    fn focus(&self, view: &NSView) {
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(view));
        }
    }
}
