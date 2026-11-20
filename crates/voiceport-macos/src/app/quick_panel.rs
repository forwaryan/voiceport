//! Compact menu-bar source switcher, keyed by stable catalog IDs.
use super::*;
use objc2_foundation::NSRectEdge;

const WIDTH: f64 = 300.;
const ROW_HEIGHT: f64 = 40.;
const LIST_HEIGHT: f64 = ROW_HEIGHT * 3.;
const HEADER_Y: f64 = 48. + LIST_HEIGHT;
const HEADER_HEIGHT: f64 = 100.;
const RECOVERY_HEIGHT: f64 = 104.;
const GUIDANCE_HEIGHT: f64 = 74.;

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
    header: Retained<NSView>,
    current: Retained<NSTextField>,
    language: Retained<NSTextField>,
    guidance: Retained<NSView>,
    guidance_reason: Retained<NSTextField>,
    guidance_button: Retained<NSButton>,
    guidance_target: Cell<Option<Setting>>,
    guidance_inputs: RefCell<Option<GuidanceInputs>>,
    recovery: Retained<NSView>,
    recovery_reason: Retained<NSTextField>,
    recovery_feedback: Retained<NSTextField>,
    recovery_generation: Cell<Option<u64>>,
    hint: Retained<NSTextField>,
    list: Retained<NSScrollView>,
    manage: Retained<NSButton>,
    rows: RefCell<Vec<SourceRow>>,
    catalog: RefCell<Vec<crate::local_models::Entry>>,
}
struct GuidanceInputs {
    source_id: String,
    language: String,
    configurable: bool,
}

impl QuickPanel {
    fn update_guidance(&self, c: &Controller, catalog_changed: bool, configurable: bool) {
        let mut inputs = self.guidance_inputs.borrow_mut();
        if !catalog_changed
            && inputs.as_ref().is_some_and(|previous| {
                previous.source_id == c.source_id
                    && previous.language == c.language
                    && previous.configurable == configurable
            })
        {
            return;
        }
        let guidance = if configurable {
            source_guidance(c)
        } else {
            None
        };
        self.guidance_target
            .set(guidance.as_ref().map(|(setting, _)| *setting));
        if let Some((setting, reason)) = guidance {
            self.guidance_button.setTitle(setting.action_title());
            set_label_text(&self.guidance_reason, &reason);
            self.guidance_reason
                .setToolTip(Some(&self.guidance_reason.stringValue()));
        }
        set_label_text(
            &self.language,
            &format!("识别语言：{}", language_name(&c.language)),
        );
        *inputs = Some(GuidanceInputs {
            source_id: c.source_id.clone(),
            language: c.language.clone(),
            configurable,
        });
    }
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
            if let Some(c) = self.ivars().controller.borrow().as_ref()
                && let Some(row) = panel.rows.borrow().iter().find(|row| row.id == c.source_id)
            {
                row.button.scrollRectToVisible(row.button.bounds());
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
                    self.settings_locked(),
                    self.ivars().quitting.get(),
                ) {
                    self.set_source(c, id);
                }
            });
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
    pub(super) fn copy_quick_recovery(&self) {
        let Some(panel) = self.ivars().quick_panel.get() else {
            return;
        };
        if !panel.popover.isShown() || !self.quick_actions_unlocked() {
            return;
        }
        self.with_controller(|c| {
            let reason = quick_recovery_reason(c);
            if !c.busy()
                && let Some(feedback) = copy_recovery_result(
                    panel.recovery_generation.get(),
                    c.state.generation(),
                    reason,
                    &c.transcript,
                    Delivery::copy,
                )
            {
                c.message = feedback.into();
            }
        });
        self.refresh();
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
        let height = HEADER_Y + HEADER_HEIGHT;
        let view = NSView::initWithFrame(NSView::alloc(mtm), rect(0., 0., WIDTH, height));
        let header =
            NSView::initWithFrame(NSView::alloc(mtm), rect(0., HEADER_Y, WIDTH, HEADER_HEIGHT));
        view.addSubview(&header);
        let icon = self.icon(&header, "waveform.circle.fill", rect(14., 65., 22., 22.));
        icon.setContentTintColor(Some(&NSColor::controlAccentColor()));
        let brand = self.label(&header, "VoicePort", rect(44., 67., 150., 22.), 14.);
        brand.setFont(Some(&NSFont::boldSystemFontOfSize(14.)));
        let open = self.button(&header, "", sel!(showWindow:), WIDTH - 44., 63., 28.);
        open.setFrame(rect(WIDTH - 44., 63., 28., 28.));
        open.setBordered(false);
        open.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        open.setImage(symbol_image("arrow.up.forward.square", 15.).as_deref());
        open.setAccessibilityLabel(Some(ns_string!("打开完整界面")));
        open.setToolTip(Some(ns_string!("打开完整界面")));
        self.surface(&header, rect(12., 0., WIDTH - 24., 54.));
        self.caption(&header, "当前转写", rect(24., 36., 66., 16.));
        let language = self.caption(&header, "", rect(92., 36., WIDTH - 116., 16.));
        language.setAlignment(NSTextAlignment::Right);
        language.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let current = self.label(&header, "", rect(24., 11., WIDTH - 48., 23.), 15.);
        current.setMaximumNumberOfLines(1);
        current.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        current.setFont(Some(&NSFont::boldSystemFontOfSize(15.)));
        current.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.current")));

        let guidance = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0., HEADER_Y, WIDTH, GUIDANCE_HEIGHT),
        );
        guidance.setHidden(true);
        view.addSubview(&guidance);
        let guidance_reason = self.label(&guidance, "", rect(24., 33., WIDTH - 48., 36.), 12.);
        guidance_reason.setMaximumNumberOfLines(2);
        let guidance_button = self.button(&guidance, "", sel!(resolveQuickSource:), 20., 0., 96.);
        guidance_button.setControlSize(NSControlSize::Small);
        guidance_button
            .setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.resolve-source")));

        let recovery = NSView::initWithFrame(
            NSView::alloc(mtm),
            rect(0., HEADER_Y, WIDTH, RECOVERY_HEIGHT),
        );
        recovery.setHidden(true);
        view.addSubview(&recovery);
        self.surface(&recovery, rect(12., 6., WIDTH - 24., 92.));
        let title = self.label(&recovery, "转写完成", rect(24., 75., WIDTH - 48., 17.), 12.);
        title.setFont(Some(&NSFont::boldSystemFontOfSize(12.)));
        let recovery_reason = self.caption(&recovery, "", rect(24., 39., WIDTH - 48., 34.));
        let copy = self.button(
            &recovery,
            "复制本次文字",
            sel!(copyQuickRecovery:),
            20.,
            8.,
            100.,
        );
        copy.setControlSize(NSControlSize::Small);
        copy.setAccessibilityIdentifier(Some(ns_string!("voiceport.quick.copy-recovery")));
        let recovery_feedback = self.caption(&recovery, "", rect(126., 15., WIDTH - 150., 21.));
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
            header,
            current,
            language,
            guidance,
            guidance_reason,
            guidance_button,
            guidance_target: Cell::new(None),
            guidance_inputs: RefCell::new(None),
            recovery,
            recovery_reason,
            recovery_feedback,
            recovery_generation: Cell::new(None),
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
        let recovery = quick_recovery_reason(c).filter(|_| idle && !self.ivars().quitting.get());
        panel
            .recovery_generation
            .set(recovery.map(|_| c.state.generation()));
        let catalog_changed = *panel.catalog.borrow() != c.catalog.entries;
        let configurable = idle && !c.discovering() && !self.ivars().quitting.get();
        panel.update_guidance(c, catalog_changed, configurable);
        let show_guidance = panel.guidance_target.get().is_some();
        if panel.recovery.isHidden() == recovery.is_some()
            || panel.guidance.isHidden() == show_guidance
        {
            let recovery_height = if recovery.is_some() {
                RECOVERY_HEIGHT
            } else {
                0.
            };
            let guidance_height = if show_guidance { GUIDANCE_HEIGHT } else { 0. };
            panel.recovery.setHidden(recovery.is_none());
            panel.guidance.setHidden(!show_guidance);
            panel
                .guidance
                .setFrameOrigin(NSPoint::new(0., HEADER_Y + recovery_height));
            let header_y = HEADER_Y + recovery_height + guidance_height;
            panel.header.setFrameOrigin(NSPoint::new(0., header_y));
            panel
                .popover
                .setContentSize(NSSize::new(WIDTH, header_y + HEADER_HEIGHT));
        }
        if let Some(reason) = recovery {
            set_label_text(&panel.recovery_reason, reason);
            set_label_text(
                &panel.recovery_feedback,
                match c.message.as_str() {
                    COPY_SUCCESS => "文字已复制",
                    COPY_FAILURE => "复制失败，请重试",
                    _ => "",
                },
            );
        }
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
        if catalog_changed {
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

fn set_label_text(label: &NSTextField, text: &str) {
    let text = NSString::from_str(text);
    if *label.stringValue() != *text {
        label.setStringValue(&text);
    }
}

fn quick_recovery_reason(c: &Controller) -> Option<&'static str> {
    let has_transcript = !c.transcript.is_empty();
    completed_delivery_failure(c.state.phase, has_transcript, c.delivery_failure_stage()).or_else(
        || {
            (c.state.phase == Phase::Ready && has_transcript && c.has_unconfirmed_paste())
                .then_some("粘贴请求已提交，请先检查输入框，避免重复粘贴。")
        },
    )
}

fn copy_recovery_result(
    displayed_generation: Option<u64>,
    generation: u64,
    reason: Option<&str>,
    transcript: &str,
    copy: impl FnOnce(&str) -> bool,
) -> Option<&'static str> {
    reason
        .filter(|_| displayed_generation == Some(generation))
        .map(|_| copy_feedback(transcript, copy))
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
    fn recovery_copies_only_the_final_text_for_the_displayed_warning() {
        let mut copied = Vec::new();
        let reason = completed_delivery_failure(Phase::Ready, true, Some("readback"));
        assert!(reason.unwrap().contains("避免重复粘贴"));
        let mut copy = |text: &str| {
            copied.push(text.to_string());
            true
        };
        assert_eq!(
            copy_recovery_result(None, 7, reason, "最终文字", &mut copy),
            None
        );
        assert_eq!(
            copy_recovery_result(Some(6), 7, reason, "最终文字", &mut copy),
            None
        );
        assert_eq!(
            copy_recovery_result(Some(7), 7, reason, "最终文字", &mut copy),
            Some(COPY_SUCCESS)
        );
        assert_eq!(copied, ["最终文字"]);
    }

    #[test]
    fn recovery_preserves_the_warning_when_clipboard_write_fails() {
        let reason = completed_delivery_failure(Phase::Ready, true, Some("focus"));
        assert_eq!(
            copy_recovery_result(Some(1), 1, reason, "最终文字", |_| false),
            Some(COPY_FAILURE)
        );
        assert!(reason.unwrap().contains("无法确认原输入框"));
        let no_copy = |_: &str| panic!("there is no current delivery warning");
        for phase in [Phase::Recording, Phase::Failed, Phase::Stopping] {
            let reason = completed_delivery_failure(phase, true, Some("focus"));
            assert_eq!(
                copy_recovery_result(Some(1), 1, reason, "旧文字", no_copy),
                None
            );
        }
        for (text, stage) in [("", Some("focus")), ("最终文字", None)] {
            let reason = completed_delivery_failure(Phase::Ready, !text.is_empty(), stage);
            assert_eq!(
                copy_recovery_result(Some(1), 1, reason, text, no_copy),
                None
            );
        }
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
