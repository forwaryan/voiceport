//! Compact menu-bar source switcher, keyed by stable catalog IDs.
use super::*;
use objc2_foundation::NSRectEdge;

const WIDTH: f64 = 300.;
const ROW_HEIGHT: f64 = 40.;
const LIST_HEIGHT: f64 = ROW_HEIGHT * 3.;

pub(super) struct QuickPanel {
    popover: Retained<NSPopover>,
    current: Retained<NSTextField>,
    hint: Retained<NSTextField>,
    list: Retained<NSScrollView>,
    manage: Retained<NSButton>,
    rows: RefCell<Vec<SourceRow>>,
    catalog: RefCell<Vec<crate::local_models::Entry>>,
}
struct SourceRow {
    id: String,
    button: Retained<NSButton>,
}

pub(super) fn selected_title(c: &Controller) -> &str {
    c.catalog
        .entries
        .iter()
        .find(|e| e.id == c.source_id)
        .map_or(c.source_id.as_str(), |e| e.title.as_str())
}

impl Delegate {
    pub(super) fn close_quick_panel(&self) {
        if let Some(panel) = self.ivars().quick_panel.get() {
            panel.popover.close();
        }
    }
    pub(super) fn toggle_quick_panel(&self) {
        if self.ivars().quitting.get() {
            return;
        }
        let Some(button) = self.ivars().status.get().and_then(|s| s.button(self.mtm())) else {
            return;
        };
        let app = NSApplication::sharedApplication(self.mtm());
        if let Some(event) = app.currentEvent()
            && event.r#type() == NSEventType::RightMouseUp
            && let Some(menu) = self.ivars().context_menu.get()
        {
            self.close_quick_panel();
            NSMenu::popUpContextMenu_withEvent_forView(menu, &event, &button);
            return;
        }
        if self
            .ivars()
            .quick_panel
            .get()
            .is_some_and(|panel| panel.popover.isShown())
        {
            self.close_quick_panel();
        } else {
            self.show_quick_panel();
        }
    }
    pub(super) fn show_quick_panel(&self) {
        if self.ivars().quitting.get() {
            return;
        }
        let Some(button) = self.ivars().status.get().and_then(|s| s.button(self.mtm())) else {
            return;
        };
        let panel = self
            .ivars()
            .quick_panel
            .get_or_init(|| self.build_quick_panel());
        // Activating an accessory app can bring its existing windows forward.
        // The status icon presents only the quick panel, even after a full-window visit.
        if let Some(window) = self.ivars().window.get() {
            window.orderOut(None);
        }
        self.refresh();
        if !panel.popover.isShown() {
            #[allow(deprecated)]
            NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
            panel.popover.showRelativeToRect_ofView_preferredEdge(
                button.bounds(),
                &button,
                NSRectEdge::MinY,
            );
            // The popover is the only visible window on a normal launch.
            // Make it key explicitly so activation and Escape target this surface.
            if let Some(controller) = panel.popover.contentViewController()
                && let Some(window) = controller.view().window()
            {
                window.makeKeyAndOrderFront(None);
                // Opening with the pointer should not pre-focus the header button.
                // Tab can still enter the normal native keyboard navigation chain.
                window.makeFirstResponder(Some(&window));
            }
        }
    }
    pub(super) fn select_quick_source(&self, sender: &NSButton) {
        let id = self.ivars().quick_panel.get().and_then(|panel| {
            panel
                .rows
                .borrow()
                .iter()
                .find(|row| std::ptr::eq(&*row.button, sender))
                .map(|row| row.id.clone())
        });
        if let Some(id) = id {
            self.with_controller(|c| {
                if can_select(
                    c,
                    &id,
                    self.ivars().permission_rx.borrow().is_some(),
                    self.ivars().managing_sources.get(),
                    self.ivars().quitting.get(),
                ) {
                    self.set_source(c, id);
                }
            });
        }
        self.refresh();
    }
    fn build_quick_panel(&self) -> QuickPanel {
        let mtm = self.mtm();
        let height = 148. + LIST_HEIGHT;
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., WIDTH, height));
        let icon = self.icon(
            &view,
            "waveform.circle.fill",
            rect(14., height - 35., 22., 22.),
        );
        icon.setContentTintColor(Some(&NSColor::controlAccentColor()));
        let brand = self.label(&view, "VoicePort", rect(44., height - 33., 150., 22.), 14.);
        brand.setFont(Some(&NSFont::boldSystemFontOfSize(14.)));
        let open = self.button(&view, "", sel!(showWindow:), WIDTH - 44., height - 37., 28.);
        open.setFrame(rect(WIDTH - 44., height - 37., 28., 28.));
        open.setBordered(false);
        open.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        open.setImage(symbol_image("arrow.up.forward.square", 15.).as_deref());
        open.setAccessibilityLabel(Some(ns_string!("打开完整界面")));
        open.setToolTip(Some(ns_string!("打开完整界面")));
        self.surface(&view, rect(12., height - 100., WIDTH - 24., 54.));
        self.caption(&view, "当前转写", rect(24., height - 64., WIDTH - 48., 16.));
        let current = self.label(&view, "", rect(24., height - 89., WIDTH - 48., 23.), 15.);
        current.setMaximumNumberOfLines(1);
        current.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        current.setFont(Some(&NSFont::boldSystemFontOfSize(15.)));
        current.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.current")));
        let list = NSScrollView::initWithFrame(
            NSScrollView::alloc(mtm),
            rect(12., 48., WIDTH - 24., LIST_HEIGHT),
        );
        list.setDrawsBackground(false);
        list.setHasVerticalScroller(true);
        list.setAutohidesScrollers(true);
        list.setBorderType(NSBorderType::NoBorder);
        list.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.sources")));
        view.addSubview(&list);
        let manage = self.button(
            &view,
            "添加转写方式…",
            sel!(quickManageSources:),
            10.,
            7.,
            126.,
        );
        manage.setControlSize(NSControlSize::Small);
        let hint = self.caption(&view, "", rect(140., 17., WIDTH - 154., 16.));
        hint.setAlignment(NSTextAlignment::Right);
        let controller =
            NSViewController::initWithNibName_bundle(NSViewController::alloc(mtm), None, None);
        controller.setView(&view);
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        // Keep surface changes immediate; an in-flight close animation must not
        // make the next status click look like another request to close.
        popover.setAnimates(false);
        popover.setContentViewController(Some(&controller));
        popover.setContentSize(NSSize::new(WIDTH, height));
        QuickPanel {
            popover,
            current,
            hint,
            list,
            manage,
            rows: RefCell::new(Vec::new()),
            catalog: RefCell::new(Vec::new()),
        }
    }
    pub(super) fn refresh_quick_panel(
        &self,
        c: &Controller,
        state: &str,
        shortcut: &str,
        idle: bool,
    ) {
        let Some(panel) = self.ivars().quick_panel.get() else {
            return;
        };
        let title = NSString::from_str(selected_title(c));
        panel.current.setStringValue(&title);
        panel
            .current
            .setToolTip(Some(&NSString::from_str(c.catalog.detail(&c.source_id))));
        panel.hint.setStringValue(&NSString::from_str(if !idle {
            state
        } else if c.discovering() {
            "正在检查来源…"
        } else {
            shortcut
        }));
        panel.hint.setToolTip(Some(&NSString::from_str(&format!(
            "{state} · {shortcut} 开始 / 停止听写"
        ))));
        panel.manage.setEnabled(idle && !c.discovering());
        if *panel.catalog.borrow() != c.catalog.entries {
            let document_height = (c.catalog.entries.len() as f64 * ROW_HEIGHT).max(LIST_HEIGHT);
            let document = NSView::initWithFrame(
                NSView::alloc(self.mtm()),
                rect(0., 0., WIDTH - 24., document_height),
            );
            let mut rows = Vec::new();
            for (index, entry) in c.catalog.entries.iter().enumerate() {
                let y = document_height - (index + 1) as f64 * ROW_HEIGHT;
                let title = if entry.source.is_ok() {
                    entry.title.clone()
                } else {
                    format!("{} · 不可用", entry.title)
                };
                let button = self.button(
                    &document,
                    &title,
                    sel!(selectQuickSource:),
                    4.,
                    y + 2.,
                    WIDTH - 46.,
                );
                button.setFrame(rect(4., y + 2., WIDTH - 46., ROW_HEIGHT - 4.));
                button.setBordered(false);
                button.setButtonType(NSButtonType::Radio);
                button.setFont(Some(&NSFont::systemFontOfSize(13.)));
                button.setAlignment(NSTextAlignment::Left);
                button.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
                button.setAccessibilityIdentifier(Some(&NSString::from_str(&format!(
                    "voiceport.quick.source.{}",
                    entry.id
                ))));
                button.setToolTip(Some(&NSString::from_str(&format!(
                    "{}\n{}",
                    entry.title, entry.detail
                ))));
                rows.push(SourceRow {
                    id: entry.id.clone(),
                    button,
                });
            }
            panel.list.setDocumentView(Some(&document));
            document.scrollPoint(NSPoint::new(0., document_height));
            *panel.rows.borrow_mut() = rows;
            *panel.catalog.borrow_mut() = c.catalog.entries.clone();
        }
        for row in panel.rows.borrow().iter() {
            let selected = row.id == c.source_id;
            let selection = if selected {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            };
            // Leave active mouse/keyboard tracking alone on the 100ms timer.
            if row.button.state() != selection {
                row.button.setState(selection);
            }
            row.button.setEnabled(can_select(
                c,
                &row.id,
                !idle,
                false,
                self.ivars().quitting.get(),
            ));
        }
    }
}
fn can_select(c: &Controller, id: &str, preparing: bool, managing: bool, quitting: bool) -> bool {
    !c.busy()
        && !c.discovering()
        && !preparing
        && !managing
        && !quitting
        && c.catalog
            .entries
            .iter()
            .any(|e| e.id == id && e.source.is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_uses_available_stable_ids_and_respects_task_locks() {
        let mut c = Controller::new();
        let mut other = c.catalog.entries[0].clone();
        other.id = "other".into();
        c.catalog.entries.push(other);
        assert!(can_select(&c, "other", false, false, false));
        assert!(!can_select(&c, "missing", false, false, false));
        assert!(!can_select(&c, "other", true, false, false));
        assert!(!can_select(&c, "other", false, true, false));
        assert!(!can_select(&c, "other", false, false, true));
        c.catalog.entries[1].source = Err("disabled".into());
        assert!(!can_select(&c, "other", false, false, false));
        c.source_id = "other".into();
        assert_eq!(selected_title(&c), c.catalog.entries[1].title);
        c.catalog.entries[1].title = "Renamed source".into();
        assert_eq!(selected_title(&c), "Renamed source");
        c.source_id = "removed".into();
        assert_eq!(selected_title(&c), "removed");
        c.state.phase = Phase::Recording;
        assert!(!can_select(
            &c,
            crate::local_models::CHATGPT,
            false,
            false,
            false
        ));
    }
}
