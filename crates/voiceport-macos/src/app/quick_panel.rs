//! Menu-bar dictation status and progressive source switching.
use super::*;
use objc2::{Message, rc::Weak};
use objc2_foundation::NSRectEdge;
use std::time::{Duration, Instant};

#[cfg(debug_assertions)]
pub(super) mod fixture;
mod history;
mod recovery;
mod recovery_model;

#[cfg(debug_assertions)]
mod performance;
#[cfg(debug_assertions)]
pub(super) use performance::{
    finish_counting_detached, prepare_detached, refresh_detached, start_counting_detached,
};

const WIDTH: f64 = 320.;
const ROW_HEIGHT: f64 = 46.;
const VISIBLE_ROWS: usize = 4;
const HEADER_HEIGHT: f64 = 168.;
const UPDATE_REFRESH_INTERVAL: Duration = Duration::from_millis(250);
const GUIDANCE_HEIGHT: f64 = 74.;
const TOOLBAR_HEIGHT: f64 = 76.;
const SCREEN_MARGIN: f64 = 24.; // popover arrow and screen-edge clearance

#[derive(Debug, PartialEq)]
struct PanelLayout {
    document_height: f64,
    viewport_height: f64,
    toolbar_y: f64,
    guidance_y: f64,
    history_y: f64,
    recovery_y: f64,
    header_y: f64,
}

impl PanelLayout {
    fn new(
        rows: usize,
        guidance: bool,
        history: f64,
        recovery: f64,
        visible_screen_height: f64,
    ) -> Self {
        let toolbar_y = rows as f64 * ROW_HEIGHT;
        let guidance_y = toolbar_y + TOOLBAR_HEIGHT;
        let history_y = guidance_y + if guidance { GUIDANCE_HEIGHT } else { 0. };
        let recovery_y = history_y + history;
        let header_y = recovery_y + recovery;
        let document_height = header_y + HEADER_HEIGHT;
        let preferred_height =
            document_height - rows.saturating_sub(VISIBLE_ROWS) as f64 * ROW_HEIGHT;
        Self {
            document_height,
            viewport_height: preferred_height.min((visible_screen_height - SCREEN_MARGIN).max(1.)),
            toolbar_y,
            guidance_y,
            history_y,
            recovery_y,
            header_y,
        }
    }
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Weak<Delegate>]
    struct PanelActions;
    unsafe impl NSObjectProtocol for PanelActions {}
    unsafe impl NSPopoverDelegate for PanelActions {
        #[unsafe(method(popoverDidClose:))]
        fn did_close(&self, _notification: &NSNotification) {
            if let Some(owner) = self.ivars().load()
                && let Some(panel) = owner.ivars().quick_panel.get()
            {
                panel.clear_recovery();
                panel.history.reset();
            }
        }
    }
    impl PanelActions {
        #[unsafe(method(toggleQuickSources:))]
        fn toggle_sources(&self, sender: &NSButton) {
            if let Some(owner) = self.ivars().load()
                && let Some(panel) = owner.ivars().quick_panel.get()
                && panel.popover.isShown()
                && std::ptr::eq(&*panel.disclosure, sender)
            {
                panel.expanded.set(!panel.expanded.get());
                owner.refresh();
            }
        }
    }
);

impl PanelActions {
    fn new(owner: &Delegate) -> Retained<Self> {
        let actions = Self::alloc(owner.mtm()).set_ivars(Weak::from_retained(&owner.retain()));
        unsafe { msg_send![super(actions), init] }
    }
}

define_class!(
    #[unsafe(super = NSButton)]
    #[thread_kind = MainThreadOnly]
    struct ScrollingButton;
    impl ScrollingButton {
        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            let accepted: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
            if accepted {
                self.scrollRectToVisible(self.bounds());
            }
            accepted
        }
    }
);

impl ScrollingButton {
    fn new(mtm: MainThreadMarker, frame: NSRect) -> Retained<Self> {
        let button = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(button), initWithFrame: frame] }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Setting {
    Source,
    Language,
}

impl Setting {
    fn action_title(self) -> &'static NSString {
        match self {
            Self::Source => ns_string!("更换来源"),
            Self::Language => ns_string!("调整语言"),
        }
    }
}

fn source_guidance(c: &Controller) -> Option<(Setting, std::borrow::Cow<'_, str>)> {
    use crate::local_models::SessionSource;
    use voiceport_core::language::LANGUAGES;

    let Some(entry) = c
        .catalog
        .entries
        .iter()
        .find(|entry| entry.id == c.source_id)
    else {
        return Some((Setting::Source, "已保存的来源不在当前列表中。".into()));
    };
    match &entry.source {
        Err(error) => Some((Setting::Source, error.as_str().into())),
        Ok(SessionSource::Local(model)) if !model.kind.supports_language(&c.language) => {
            let example = LANGUAGES
                .iter()
                .find(|(code, _)| !code.is_empty() && model.kind.supports_language(code));
            let suggestion = example.map_or("自动检测", |(_, name)| name);
            Some((
                Setting::Language,
                format!(
                    "当前模型不支持「{}」，可改选「{suggestion}」。",
                    language_name(&c.language)
                )
                .into(),
            ))
        }
        Ok(_) => None,
    }
}

fn language_name(language: &str) -> &str {
    voiceport_core::language::LANGUAGES
        .iter()
        .find(|(code, _)| *code == language)
        .map_or(language, |(_, name)| name)
}

pub(super) struct QuickPanel {
    popover: Retained<NSPopover>,
    scroll: Retained<NSScrollView>,
    document: Retained<NSView>,
    layout: RefCell<Option<PanelLayout>>,
    screen_height: Cell<f64>,
    header: Retained<NSView>,
    current_caption: Retained<NSTextField>,
    current: Retained<NSTextField>,
    checkmark: Retained<NSImageView>,
    language: Retained<NSTextField>,
    status: Retained<NSTextField>,
    shortcut: Retained<NSTextField>,
    device: Retained<NSTextField>,
    update: Retained<NSButton>,
    next_update_refresh: Cell<Option<Instant>>,
    open: Retained<NSButton>,
    toggle: Retained<NSButton>,
    cancel: Retained<NSButton>,
    files: Retained<NSButton>,
    guidance: Retained<NSView>,
    guidance_reason: Retained<NSTextField>,
    guidance_button: Retained<NSButton>,
    guidance_target: Cell<Option<Setting>>,
    source_lock: Cell<Option<&'static str>>,
    recovery_slot: Retained<NSView>,
    recovery: recovery::ResultCard,
    recovery_actions: Retained<recovery::RecoveryActions>,
    history: history::History,
    toolbar: Retained<NSView>,
    disclosure: Retained<NSButton>,
    manage: Retained<NSButton>,
    expanded: Cell<bool>,
    _actions: Retained<PanelActions>,
    sources: Retained<NSView>,
    rows: RefCell<Vec<SourceRow>>,
}

struct SourceRow {
    id: String,
    button: Retained<NSButton>,
    reason: Retained<NSTextField>,
    title: String,
    detail: String,
    unavailable: Option<String>,
    lock_reason: Option<&'static str>,
}

impl QuickPanel {
    fn update_guidance(&self, c: &Controller, configurable: bool) {
        let guidance = if c.catalog.entries.is_empty() || c.active_app_source().is_some() {
            None // The header already explains the active source or lack of one.
        } else {
            source_guidance(c)
        };
        if self.guidance.isHidden() != guidance.is_none() {
            self.guidance.setHidden(guidance.is_none());
        }
        let previous = self
            .guidance_target
            .replace(guidance.as_ref().map(|(setting, _)| *setting));
        if let Some((setting, reason)) = guidance {
            if previous != Some(setting) {
                self.guidance_button.setTitle(setting.action_title());
            }
            set_button_enabled(&self.guidance_button, configurable);
            if set_label_text(&self.guidance_reason, &reason) {
                self.guidance_reason
                    .setToolTip(Some(&self.guidance_reason.stringValue()));
            }
        }
    }

    fn relink_key_views(&self) {
        let mut controls: Vec<Retained<NSView>> = [&self.open, &self.update]
            .into_iter()
            .map(|button| button.clone().into_super().into_super())
            .collect();
        self.append_recovery_key_views(&mut controls);
        self.history.append_key_views(&mut controls);
        if !self.guidance.isHidden() {
            controls.push(self.guidance_button.clone().into_super().into_super());
        }
        controls.extend(
            [&self.toggle, &self.cancel, &self.files, &self.manage]
                .map(|button| button.clone().into_super().into_super()),
        );
        controls.push(self.disclosure.clone().into_super().into_super());
        if self.expanded.get() {
            controls.extend(
                self.rows
                    .borrow()
                    .iter()
                    .map(|row| row.button.clone().into_super().into_super()),
            );
        }
        for (view, next) in controls.iter().zip(controls.iter().cycle().skip(1)) {
            // All next views are retained by this panel's view hierarchy.
            unsafe { view.setNextKeyView(Some(next)) };
        }
        // The native key-view loop handles reverse traversal and skips
        // hidden/disabled controls without changing the user's system setting.
        if let Some(window) = self.document.window() {
            window.setAutorecalculatesKeyViewLoop(false);
            window.setInitialFirstResponder(Some(&self.open));
        }
    }

    fn position(&self, layout: PanelLayout) {
        let mut cached = self.layout.borrow_mut();
        if cached.as_ref() == Some(&layout) {
            return;
        }
        let visible = self.scroll.documentVisibleRect();
        let from_top = cached.as_ref().map_or(0., |previous| {
            (previous.document_height - visible.origin.y - visible.size.height).max(0.)
        });
        self.sources
            .setFrameSize(NSSize::new(WIDTH, layout.toolbar_y));
        self.toolbar
            .setFrameOrigin(NSPoint::new(0., layout.toolbar_y));
        self.guidance
            .setFrameOrigin(NSPoint::new(0., layout.guidance_y));
        self.history.view.setFrame(rect(
            0.,
            layout.history_y,
            WIDTH,
            layout.recovery_y - layout.history_y,
        ));
        self.recovery_slot.setFrame(rect(
            0.,
            layout.recovery_y,
            WIDTH,
            layout.header_y - layout.recovery_y,
        ));
        self.header
            .setFrameOrigin(NSPoint::new(0., layout.header_y));
        self.document
            .setFrameSize(NSSize::new(WIDTH, layout.document_height));
        let viewport = NSSize::new(WIDTH, layout.viewport_height);
        self.popover.setContentSize(viewport);
        self.scroll.setFrameSize(viewport);
        // The same scroll surface holds all sections, so even if the recovery
        // slot fills a very short screen the sources and Add remain reachable.
        self.scroll_from_top(from_top);
        *cached = Some(layout);
    }

    fn scroll_from_top(&self, inset: f64) {
        let clip = self.scroll.contentView();
        let max_y = (self.document.bounds().size.height - clip.bounds().size.height).max(0.);
        clip.scrollToPoint(NSPoint::new(0., (max_y - inset).max(0.)));
        self.scroll.reflectScrolledClipView(&clip);
    }

    fn scroll_to_top(&self) {
        self.scroll_from_top(0.);
    }
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
            self.ivars().context_menu_visible.set(true);
            self.prepare_context_menu();
            NSMenu::popUpContextMenu_withEvent_forView(menu, &event, &button);
            self.ivars().context_menu_visible.set(false);
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
    pub(super) fn hide_workbench_for_quick_panel(&self) {
        // Ordering out the window does not send a close or minimize notification.
        if let Some(window) = self.ivars().window.get() {
            self.clear_workbench_history();
            window.orderOut(None);
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
        self.hide_workbench_for_quick_panel();
        if let Some(screen) = button.window().and_then(|window| window.screen()) {
            panel.screen_height.set(screen.visibleFrame().size.height);
        }
        if !panel.popover.isShown() {
            self.prepare_quick_panel(panel);
            panel.scroll_to_top();
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
                panel.relink_key_views();
                window.makeFirstResponder(Some(&window));
                panel.scroll_to_top();
            }
        }
    }
    fn prepare_quick_panel(&self, panel: &QuickPanel) {
        // Start each showing with compact disclosures. The first render uses
        // only data already on the main thread; the tick starts due queries.
        panel.expanded.set(false);
        if let Ok(slot) = self.ivars().controller.try_borrow()
            && let Some(c) = slot.as_ref()
        {
            self.refresh_quick_panel(c, true);
        }
    }

    pub(super) fn select_quick_source(&self, sender: &NSButton) {
        let Some(panel) = self.ivars().quick_panel.get() else {
            return;
        };
        if !panel.popover.isShown() || !panel.expanded.get() || !self.quick_actions_unlocked() {
            return;
        }
        let Some(id) = panel.source_for_sender(sender) else {
            return;
        };
        let can_select_source = self
            .ivars()
            .controller
            .borrow()
            .as_ref()
            .is_some_and(|c| can_select(c, &id, false, false, false));
        if can_select_source {
            self.request_dictation_source(&id);
            if self
                .ivars()
                .controller
                .borrow()
                .as_ref()
                .is_some_and(|c| c.source_id == id)
            {
                panel.expanded.set(false);
                // The source refresh may already have removed the selected row.
                if panel.popover.isShown()
                    && let Some(window) = panel.disclosure.window()
                {
                    window.makeFirstResponder(Some(&panel.disclosure));
                }
            }
        }
        self.refresh();
    }
    pub(super) fn resolve_quick_source(&self) {
        let Some(panel) = self.ivars().quick_panel.get() else {
            return;
        };
        if !panel.popover.isShown() || !self.quick_actions_unlocked() {
            return;
        }
        let target = self
            .ivars()
            .controller
            .borrow()
            .as_ref()
            .filter(|c| !c.busy() && !c.discovering())
            .and_then(source_guidance)
            .map(|(setting, _)| setting);
        let Some(target) = target.filter(|t| Some(*t) == panel.guidance_target.get()) else {
            return;
        };
        self.show_window();
        self.select_page(true);
        let picker = match target {
            Setting::Source => self.ivars().source.get(),
            Setting::Language => self.ivars().language.get(),
        };
        if let (Some(window), Some(picker)) = (self.ivars().window.get(), picker) {
            window.makeFirstResponder(Some(picker));
            // The native pop-up owns its menu and targets the live settings delegate.
            unsafe { picker.performClick(None) };
        }
    }
    fn quick_actions_unlocked(&self) -> bool {
        let iv = self.ivars();
        !iv.quitting.get()
            && !self.settings_locked()
            && iv.permission_rx.borrow().is_none()
            && iv.pending_hotkey.borrow().is_none()
    }
    fn build_quick_panel(&self) -> QuickPanel {
        let mtm = self.mtm();
        let height = HEADER_HEIGHT + TOOLBAR_HEIGHT;
        let scroll =
            NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(0., 0., WIDTH, height));
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setScrollerStyle(NSScrollerStyle::Overlay);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setAccessibilityLabel(Some(ns_string!("VoicePort 快捷面板")));
        let document = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., WIDTH, height));
        scroll.setDocumentView(Some(&document));
        let section = |y, height| {
            let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0., y, WIDTH, height));
            document.addSubview(&view);
            view
        };
        // In the default top-to-bottom reading order: overview, current result,
        // source guidance, source actions, then alternatives when expanded.
        let header = section(TOOLBAR_HEIGHT, HEADER_HEIGHT);
        let recovery_slot = section(TOOLBAR_HEIGHT, 0.);
        let history = history::History::new(self, section(TOOLBAR_HEIGHT, 0.));
        let guidance = section(TOOLBAR_HEIGHT, GUIDANCE_HEIGHT);
        let toolbar = section(0., TOOLBAR_HEIGHT);
        let sources = section(0., 0.);
        guidance.setHidden(true);
        sources.setHidden(true);

        let icon = self.icon(&header, "waveform.circle.fill", rect(16., 133., 21., 21.));
        icon.setContentTintColor(Some(&NSColor::controlAccentColor()));
        let brand = self.label(
            &header,
            voiceport_core::test_build::application_name(),
            rect(45., 134., 135., 21.),
            14.,
        );
        brand.setFont(Some(&NSFont::boldSystemFontOfSize(14.)));
        let open = panel_button(
            self,
            &header,
            "打开完整界面",
            sel!(showWindow:),
            rect(WIDTH - 134., 130., 118., 29.),
        );
        open.setAccessibilityLabel(Some(ns_string!("打开 VoicePort 完整界面")));
        open.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.open")));

        let separator = NSBox::initWithFrame(NSBox::alloc(mtm), rect(16., 125., WIDTH - 32., 1.));
        separator.setBoxType(NSBoxType::Separator);
        header.addSubview(&separator);
        let current_caption = self.caption(&header, "当前来源", rect(16., 108., 72., 17.));
        let language = self.caption(&header, "", rect(94., 108., WIDTH - 110., 17.));
        language.setAlignment(NSTextAlignment::Right);
        single_line(&language);
        let checkmark = self.icon(&header, "checkmark.circle.fill", rect(16., 85., 17., 17.));
        checkmark.setContentTintColor(Some(&NSColor::controlAccentColor()));
        checkmark.setAccessibilityElement(false);
        let current = self.label(&header, "", rect(40., 82., WIDTH - 56., 24.), 15.);
        single_line(&current);
        current.setFont(Some(&NSFont::boldSystemFontOfSize(15.)));
        current.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.current")));
        let status = self.label(&header, "", rect(16., 62., WIDTH - 32., 19.), 13.);
        single_line(&status);
        status.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.state")));
        let shortcut = self.caption(&header, "", rect(16., 42., WIDTH - 32., 17.));
        single_line(&shortcut);
        shortcut.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.shortcut")));
        let device = self.caption(&header, "", rect(16., 24., WIDTH - 32., 17.));
        single_line(&device);
        device.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.device")));
        let update = panel_button(
            self,
            &header,
            "",
            sel!(updateVoicePort:),
            rect(16., 0., WIDTH - 32., 24.),
        );
        update.setAlignment(NSTextAlignment::Left);
        update.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        update.setAccessibilityHelp(Some(ns_string!("打开工作台的软件更新页面")));
        update.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.update")));

        let guidance_reason = self.label(&guidance, "", rect(16., 33., WIDTH - 32., 36.), 12.);
        guidance_reason.setMaximumNumberOfLines(2);
        let guidance_button = panel_button(
            self,
            &guidance,
            "",
            sel!(resolveQuickSource:),
            rect(16., 2., 102., 28.),
        );
        guidance_button
            .setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.resolve-source")));
        let toggle = panel_button(
            self,
            &toolbar,
            "开始听写",
            sel!(toggle:),
            rect(16., 41., 107., 28.),
        );
        toggle.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.toggle")));
        toggle.setEnabled(false);
        let cancel = panel_button(
            self,
            &toolbar,
            "取消",
            sel!(cancel:),
            rect(123., 41., 58., 28.),
        );
        cancel.setAccessibilityLabel(Some(ns_string!("取消听写")));
        cancel.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.cancel")));
        cancel.setEnabled(false);
        let files = panel_button(
            self,
            &toolbar,
            "转写音频文件…",
            sel!(showFileTranscription:),
            rect(181., 41., 123., 28.),
        );
        files.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.files")));
        let manage = panel_button(
            self,
            &toolbar,
            "添加转写方式…",
            sel!(quickManageSources:),
            rect(16., 8., 130., 28.),
        );
        manage.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.add")));
        let disclosure = panel_button(
            self,
            &toolbar,
            "",
            sel!(toggleQuickSources:),
            rect(WIDTH - 164., 8., 148., 28.),
        );
        disclosure.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.more-sources")));
        disclosure.setAccessibilityHelp(Some(ns_string!("展开后用 Tab 或滚动查看其他来源")));
        let actions = PanelActions::new(self);
        unsafe { disclosure.setTarget(Some(&*actions)) };

        let controller =
            NSViewController::initWithNibName_bundle(NSViewController::alloc(mtm), None, None);
        controller.setView(&scroll);
        let popover = NSPopover::new(mtm);
        popover.setBehavior(NSPopoverBehavior::Transient);
        // A close animation must not turn the next status click into another close.
        popover.setAnimates(false);
        popover.setDelegate(Some(ProtocolObject::from_ref(&*actions)));
        popover.setContentViewController(Some(&controller));
        popover.setContentSize(NSSize::new(WIDTH, height));
        QuickPanel {
            popover,
            scroll,
            document,
            layout: RefCell::new(None),
            screen_height: Cell::new(f64::INFINITY),
            header,
            current_caption,
            current,
            checkmark,
            language,
            status,
            shortcut,
            device,
            update,
            next_update_refresh: Cell::new(None),
            open,
            toggle,
            cancel,
            files,
            guidance,
            guidance_reason,
            guidance_button,
            guidance_target: Cell::new(None),
            source_lock: Cell::new(None),
            recovery_slot,
            recovery: recovery::ResultCard::default(),
            recovery_actions: recovery::RecoveryActions::new(self),
            history,
            toolbar,
            disclosure,
            manage,
            expanded: Cell::new(false),
            _actions: actions,
            sources,
            rows: RefCell::new(Vec::new()),
        }
    }

    pub(super) fn refresh_quick_panel(&self, c: &Controller, opening: bool) {
        let iv = self.ivars();
        if let Some(panel) = iv
            .quick_panel
            .get()
            .filter(|panel| opening || panel.is_shown())
            && let Some(snapshot) = iv.menu_snapshot.borrow().as_ref()
        {
            let device = iv.menu_device.borrow().label(c.microphone_uid.as_deref());
            let title = NSString::from_str(snapshot.toggle_title);
            if !panel.toggle.title().isEqualToString(&title) {
                panel.toggle.setTitle(&title);
            }
            set_button_enabled(&panel.toggle, snapshot.can_toggle);
            set_button_enabled(&panel.cancel, snapshot.can_cancel);
            set_label_text(&panel.device, &format!("麦克风：{device}"));
            panel.refresh_update(self, opening, Instant::now());
            panel.refresh_contents(self, c, snapshot.state, &snapshot.shortcut, snapshot.idle);
        }
    }
}

impl QuickPanel {
    pub(super) fn is_shown(&self) -> bool {
        self.popover.isShown()
    }

    fn refresh_update(&self, delegate: &Delegate, opening: bool, now: Instant) {
        if !opening
            && self
                .next_update_refresh
                .get()
                .is_some_and(|next| now < next)
        {
            return;
        }
        self.next_update_refresh
            .set(Some(now + UPDATE_REFRESH_INTERVAL));
        let title = delegate.compact_update_title();
        let title = NSString::from_str(&format!("软件更新：{title}"));
        if self.update.title() != title {
            self.update.setTitle(&title);
            self.update.setToolTip(Some(&title));
        }
    }

    fn refresh_contents(
        &self,
        d: &Delegate,
        c: &Controller,
        state: &str,
        shortcut: &str,
        idle: bool,
    ) {
        #[cfg(debug_assertions)]
        if !self.is_shown() {
            performance::count_hidden_refresh();
        }
        let recovery_height = self.update_recovery(d, c, idle);
        let (history_height, history_changed) =
            self.history
                .update(d, c, idle && d.quick_actions_unlocked());
        let configurable = idle && !c.discovering() && !d.ivars().quitting.get();
        self.update_guidance(c, configurable);
        let (caption, title, language, available, tooltip) =
            if let Some((title, language)) = c.active_app_source() {
                (
                    "本轮来源",
                    title,
                    language,
                    true,
                    c.gui_result_context().source.clone(),
                )
            } else {
                let current = c
                    .catalog
                    .entries
                    .iter()
                    .find(|entry| entry.id == c.source_id);
                let title = current.map_or_else(
                    || {
                        if c.catalog.entries.is_empty() {
                            "未配置来源"
                        } else {
                            "当前来源已不可用"
                        }
                    },
                    |entry| entry.title.as_str(),
                );
                (
                    "当前来源",
                    title,
                    c.language.as_str(),
                    current.is_some_and(|entry| entry.source.is_ok()),
                    current.map(|entry| format!("{}\n{}", entry.title, entry.detail)),
                )
            };
        if self.checkmark.isHidden() == available {
            self.checkmark.setHidden(!available);
        }
        let caption_changed = set_label_text(&self.current_caption, caption);
        if set_label_text(&self.current, title) || caption_changed {
            self.current
                .setAccessibilityLabel(Some(&NSString::from_str(&format!("{caption}：{title}"))));
        }
        let tooltip = Some(NSString::from_str(tooltip.as_deref().unwrap_or(title)));
        if self.current.toolTip() != tooltip {
            self.current.setToolTip(tooltip.as_deref());
        }
        set_label_text(
            &self.language,
            &format!("识别语言：{}", language_name(language)),
        );
        let issue = c.app_source_issue();
        let message = if let Some((title, _)) = issue {
            title
        } else if idle && c.discovering() {
            "正在检查来源…"
        } else {
            state
        };
        let active_rule = c.active_app_rule();
        let status = match active_rule {
            Some(rule) => format!("听写状态：{message} · {rule}"),
            None => format!("听写状态：{message}"),
        };
        set_label_text(&self.status, &status);
        let tooltip = issue
            .map(|(_, detail)| NSString::from_str(detail))
            .or_else(|| active_rule.map(|_| self.status.stringValue()));
        if self.status.toolTip() != tooltip {
            self.status.setToolTip(tooltip.as_deref());
        }
        let has_shortcut = d.ivars().hotkey_error.borrow().is_none();
        let shortcut_help = if has_shortcut {
            d.ivars().shortcut_mode.get().instruction()
        } else {
            "未注册，请在设置中更换"
        };
        let shortcut_text = format!("快捷键：{shortcut} · {shortcut_help}");
        if set_label_text(&self.shortcut, &shortcut_text) {
            self.shortcut.setToolTip(Some(&self.shortcut.stringValue()));
        }
        let lock_reason = if c.discovering() {
            Some("正在检查来源，请稍候")
        } else if !configurable {
            Some("当前有任务或设置操作，请完成后再更换来源")
        } else {
            None
        };
        set_button_enabled(&self.manage, configurable);
        if self.source_lock.replace(lock_reason) != lock_reason {
            let disabled = lock_reason.map(NSString::from_str);
            self.manage.setToolTip(disabled.as_deref());
            self.manage.setAccessibilityHelp(disabled.as_deref());
        }

        let row_count = c
            .catalog
            .entries
            .iter()
            .filter(|entry| entry.id != c.source_id)
            .count();
        if self.expanded.get()
            && row_count == 0
            && let Some(window) = self.document.window()
            && window.firstResponder().is_some_and(|responder| {
                self.rows
                    .borrow()
                    .iter()
                    .any(|row| std::ptr::eq(&*responder, &****row.button))
                    || std::ptr::eq(&*responder, &****self.disclosure)
            })
        {
            window.makeFirstResponder(Some(&self.manage));
        }
        if row_count == 0 {
            self.expanded.set(false);
        }
        let expanded = self.expanded.get();
        let changed = expanded && self.update_source_rows(d, c, row_count, lock_reason);
        if self.sources.isHidden() == expanded {
            self.sources.setHidden(!expanded);
        }
        if self.disclosure.isHidden() != (row_count == 0) {
            self.disclosure.setHidden(row_count == 0);
        }
        let disclosure_title = if expanded {
            "收起来源".to_owned()
        } else {
            format!("更多来源（{row_count}）")
        };
        let title = NSString::from_str(&disclosure_title);
        if !self.disclosure.title().isEqualToString(&title) {
            self.disclosure.setTitle(&title);
            self.disclosure.setAccessibilityLabel(Some(&title));
        }
        if self.disclosure.isAccessibilityExpanded() != expanded {
            self.disclosure.setAccessibilityExpanded(expanded);
        }

        let layout = PanelLayout::new(
            if expanded { row_count } else { 0 },
            !self.guidance.isHidden(),
            history_height,
            recovery_height,
            self.screen_height.get(),
        );
        if self.layout.borrow().as_ref() != Some(&layout) || changed || history_changed {
            self.position(layout);
            self.relink_key_views();
        }
    }

    fn source_for_sender(&self, sender: &NSButton) -> Option<String> {
        self.rows
            .borrow()
            .iter()
            .find(|row| std::ptr::eq(&*row.button, sender) && row.button.isEnabled())
            .map(|row| row.id.clone())
    }

    fn update_source_rows(
        &self,
        d: &Delegate,
        c: &Controller,
        count: usize,
        lock_reason: Option<&'static str>,
    ) -> bool {
        let alternatives = c
            .catalog
            .entries
            .iter()
            .filter(|entry| entry.id != c.source_id);
        let mut rows = self.rows.borrow_mut();
        let reordered = !rows
            .iter()
            .map(|row| &row.id)
            .eq(alternatives.clone().map(|entry| &entry.id));
        if reordered {
            let mut prior: std::collections::HashMap<_, _> =
                rows.drain(..).map(|row| (row.id.clone(), row)).collect();
            rows.extend(alternatives.clone().map(|entry| {
                prior
                    .remove(&entry.id)
                    .unwrap_or_else(|| SourceRow::new(d, &self.sources, &entry.id))
            }));
            for row in prior.into_values() {
                if let Some(window) = row.button.window()
                    && window.firstResponder().as_deref() == Some(&****row.button)
                {
                    window.makeFirstResponder(Some(&self.disclosure));
                }
                row.button.removeFromSuperview();
                row.reason.removeFromSuperview();
            }
        }
        for (index, (row, entry)) in rows.iter_mut().zip(alternatives).enumerate() {
            row.update(entry, (count - index - 1) as f64 * ROW_HEIGHT, lock_reason);
        }
        reordered
    }
}

impl SourceRow {
    fn new(d: &Delegate, parent: &NSView, id: &str) -> Self {
        let button = panel_button(
            d,
            parent,
            "",
            sel!(selectQuickSource:),
            rect(16., 0., WIDTH - 32., 30.),
        );
        button.setBordered(false);
        button.setAlignment(NSTextAlignment::Left);
        button.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        button.setFont(Some(&NSFont::systemFontOfSize(13.)));
        button.setAccessibilityIdentifier(Some(&NSString::from_str(&format!(
            "voiceport.quick.source.{id}"
        ))));
        let reason = d.caption(parent, "", rect(16., 0., WIDTH - 32., 17.));
        single_line(&reason);
        reason.setHidden(true);
        Self {
            id: id.to_owned(),
            button,
            reason,
            title: String::new(),
            detail: String::new(),
            unavailable: None,
            lock_reason: None,
        }
    }

    fn update(
        &mut self,
        entry: &crate::local_models::Entry,
        y: f64,
        lock_reason: Option<&'static str>,
    ) {
        let unavailable = entry.source.as_ref().err();
        if self.title != entry.title || self.unavailable.as_ref() != unavailable {
            self.button.setTitle(&NSString::from_str(&entry.title));
            let label = if unavailable.is_some() {
                format!("{}，不可用", entry.title)
            } else {
                format!("切换到 {}", entry.title)
            };
            self.button
                .setAccessibilityLabel(Some(&NSString::from_str(&label)));
            self.reason.setHidden(unavailable.is_none());
            if let Some(reason) = unavailable {
                let text = NSString::from_str(&format!("不可用：{reason}"));
                self.reason.setStringValue(&text);
                self.reason.setToolTip(Some(&text));
            }
        }
        if self.title != entry.title
            || self.detail != entry.detail
            || self.unavailable.as_ref() != unavailable
            || self.lock_reason != lock_reason
        {
            let reason = unavailable.map(String::as_str).or(lock_reason);
            set_button_enabled(&self.button, reason.is_none());
            let tooltip = format!("{}\n{}", entry.title, reason.unwrap_or(&entry.detail));
            self.button.setToolTip(Some(&NSString::from_str(&tooltip)));
            self.button
                .setAccessibilityHelp(reason.map(NSString::from_str).as_deref());
            self.title.clone_from(&entry.title);
            self.detail.clone_from(&entry.detail);
            self.unavailable = unavailable.cloned();
            self.lock_reason = lock_reason;
        }
        let button_y = y + if unavailable.is_some() { 16. } else { 7. };
        if self.button.frame().origin.y != button_y {
            self.button.setFrameOrigin(NSPoint::new(16., button_y));
        }
        if self.reason.frame().origin.y != y + 2. {
            self.reason.setFrameOrigin(NSPoint::new(16., y + 2.));
        }
    }
}

fn panel_button(
    d: &Delegate,
    view: &NSView,
    title: &str,
    action: objc2::runtime::Sel,
    frame: NSRect,
) -> Retained<NSButton> {
    let button = ScrollingButton::new(d.mtm(), frame);
    button.setTitle(&NSString::from_str(title));
    button.setBezelStyle(NSBezelStyle::Push);
    button.setControlSize(NSControlSize::Small);
    unsafe {
        button.setTarget(Some(d));
        button.setAction(Some(action));
    }
    view.addSubview(&button);
    button.into_super()
}

fn single_line(label: &NSTextField) {
    label.setMaximumNumberOfLines(1);
    label.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
}

fn set_label_text(label: &NSTextField, text: &str) -> bool {
    let text = NSString::from_str(text);
    if label.stringValue().isEqualToString(&text) {
        return false;
    }
    label.setStringValue(&text);
    true
}

fn set_button_enabled(button: &NSButton, enabled: bool) {
    if button.isEnabled() != enabled {
        button.setEnabled(enabled);
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
    fn viewport_caps_long_lists_and_keeps_recovery_above_source_actions() {
        let compact = PanelLayout::new(0, false, 0., 0., 720.);
        assert_eq!(compact.document_height, compact.viewport_height);
        let long = PanelLayout::new(40, false, 0., 0., 720.);
        assert_eq!(
            long.viewport_height,
            compact.viewport_height + 4. * ROW_HEIGHT
        );
        assert_eq!(
            long.document_height,
            compact.document_height + 40. * ROW_HEIGHT
        );

        let small = PanelLayout::new(40, true, 0., 104., 260.);
        assert_eq!(small.viewport_height, 260. - SCREEN_MARGIN);
        assert_eq!(small.header_y, small.recovery_y + 104.);
        assert_eq!(small.recovery_y, small.guidance_y + GUIDANCE_HEIGHT);
        assert_eq!(small.guidance_y, small.toolbar_y + TOOLBAR_HEIGHT);
        assert!(small.viewport_height >= HEADER_HEIGHT);
    }

    #[test]
    fn guidance_uses_selected_id_and_its_availability_diagnostic() {
        let mut c = Controller::new();
        assert!(source_guidance(&c).is_none());
        let mut unavailable = c.catalog.entries[0].clone();
        unavailable.id = "unavailable".into();
        unavailable.source = Err("未找到模型文件。".into());
        c.catalog.entries.push(unavailable);
        c.source_id = "unavailable".into();
        assert_eq!(
            source_guidance(&c),
            Some((Setting::Source, "未找到模型文件。".into()))
        );
        c.catalog.entries.pop();
        assert_eq!(
            source_guidance(&c),
            Some((Setting::Source, "已保存的来源不在当前列表中。".into()))
        );
        assert_eq!(c.source_id, "unavailable");
    }

    #[test]
    fn guidance_names_an_incompatible_language_and_a_supported_choice() {
        use crate::local_models::{Model, ModelKind, SessionSource};

        let mut c = Controller::new();
        c.catalog.entries[0].source = Ok(SessionSource::Local(Model {
            kind: ModelKind::ParakeetUnified,
            directory: Default::default(),
            runtime: Default::default(),
            encoder: Default::default(),
            decoder: Default::default(),
            joiner: None,
            tokens: Default::default(),
        }));
        c.language = "zh".into();
        assert_eq!(
            source_guidance(&c),
            Some((
                Setting::Language,
                "当前模型不支持「中文」，可改选「English」。".into()
            ))
        );
        for language in ["en", ""] {
            c.language = language.into();
            assert!(source_guidance(&c).is_none());
        }
        assert_eq!(language_name(""), "自动检测");
    }

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
