//! Native AppKit shell. Diagnostic paths precede controller and device creation.
use crate::{controller::Controller, delivery::Delivery, lifecycle::Phase};
use objc2::{
    DefinedClass, MainThreadOnly, define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, ProtocolObject},
    sel,
};
use objc2_app_kit::*;
use objc2_foundation::{
    MainThreadMarker, NSBundle, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect,
    NSRunLoop, NSRunLoopCommonModes, NSSize, NSString, NSTimer, NSUserDefaults, ns_string,
};
use std::cell::{Cell, OnceCell, RefCell};

#[derive(Clone, Copy, PartialEq, Eq)]
enum StatusAppearance {
    Ready,
    Recording,
    Working,
    Attention,
}
impl StatusAppearance {
    fn symbol(self) -> &'static str {
        match self {
            Self::Ready => "waveform",
            Self::Recording => "record.circle.fill",
            Self::Working => "ellipsis.circle",
            Self::Attention => "mic.slash",
        }
    }
}

#[derive(Default)]
struct Ivars {
    controller: RefCell<Option<Controller>>,
    status: OnceCell<Retained<NSStatusItem>>,
    menu_toggle: OnceCell<Retained<NSMenuItem>>,
    menu_state: OnceCell<Retained<NSMenuItem>>,
    menu_shortcut: OnceCell<Retained<NSMenuItem>>,
    menu_cancel: OnceCell<Retained<NSMenuItem>>,
    menu_copy: OnceCell<Retained<NSMenuItem>>,
    status_appearance: Cell<Option<StatusAppearance>>,
    empty_hint: OnceCell<Retained<NSView>>,
    page_tabs: OnceCell<Retained<NSSegmentedControl>>,
    dictation_page: OnceCell<Retained<NSView>>,
    settings_page: OnceCell<Retained<NSView>>,
    notice_icon: OnceCell<Retained<NSImageView>>,
    notice_appearance: Cell<Option<StatusAppearance>>,
    notice_action: OnceCell<Retained<NSButton>>,
    shortcut_hint: OnceCell<Retained<NSTextField>>,
    cancel: OnceCell<Retained<NSButton>>,
    copy: OnceCell<Retained<NSButton>>,
    window: OnceCell<Retained<NSWindow>>,
    message: OnceCell<Retained<NSTextField>>,
    details: OnceCell<Retained<NSTextField>>,
    transcript: OnceCell<Retained<NSTextView>>,
    toggle: OnceCell<Retained<NSButton>>,
    retry: OnceCell<Retained<NSButton>>,
    automatic: OnceCell<Retained<NSSwitch>>,
    language: OnceCell<Retained<NSPopUpButton>>,
    source: OnceCell<Retained<NSPopUpButton>>,
    source_rows: RefCell<Vec<(String, String)>>,
    source_detail: OnceCell<Retained<NSTextField>>,
    refresh_models: OnceCell<Retained<NSButton>>,
    shortcut: OnceCell<Retained<NSPopUpButton>>,
    timer: OnceCell<Retained<NSTimer>>,
    quitting: Cell<bool>,
    shutdown_started: Cell<bool>,
    alternative: Cell<bool>,
    hotkey: RefCell<Option<crate::hotkey::GlobalHotKey>>,
    hotkey_rx: RefCell<Option<std::sync::mpsc::Receiver<()>>>,
    permission_rx: RefCell<Option<std::sync::mpsc::Receiver<(u64, crate::audio::AudioEvent)>>>,
    prepare_permissions: OnceCell<Retained<NSButton>>,
    accessibility: OnceCell<Retained<NSButton>>,
    hotkey_error: RefCell<Option<String>>,
}
define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct Delegate;
    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn launched(&self, _notification: &NSNotification) {
            self.build_menu();
            self.show_window();
        }
        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn reopen(&self, _app: &NSApplication, _visible: bool) -> bool {
            self.show_window();
            false
        }
        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn terminate_after_window_closed(&self, _app: &NSApplication) -> bool {
            false
        }
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            self.ivars().quitting.set(true);
            // Idle shutdown can finish synchronously. Reentrant termination
            // must wait until the controller borrow and any worker lease end.
            if let Ok(mut slot) = self.ivars().controller.try_borrow_mut() {
                if let Some(c) = slot.as_mut() {
                    if !self.ivars().shutdown_started.replace(true) { c.cancel(); }
                    if c.state.phase == Phase::Stopping { return NSApplicationTerminateReply::TerminateLater; }
                }
                return NSApplicationTerminateReply::TerminateNow;
            }
            NSApplicationTerminateReply::TerminateLater
        }
    }
    impl Delegate {
        #[unsafe(method(preparePermissions:))]
        fn prepare_permissions_action(&self, _sender: Option<&AnyObject>) {
            if self.ivars().permission_rx.borrow().is_some() { return; }
            let idle = self.ivars().controller.try_borrow().is_ok_and(|slot| slot.as_ref().is_some_and(|c| !c.busy()));
            if !idle || self.ivars().quitting.get() { return; }
            let (sender, receiver) = std::sync::mpsc::channel();
            *self.ivars().permission_rx.borrow_mut() = Some(receiver);
            let mut use_speech = true;
            self.with_controller(|c| {
                use_speech = c.uses_system_speech();
                c.message = if use_speech { "正在准备麦克风与系统语音权限；不会开始录音。" } else { "正在准备麦克风权限；本地模型不需要系统语音识别权限，不会开始录音。" }.into();
            });
            self.refresh();
            crate::audio::Recorder::request_permissions_for_source(0, sender, use_speech);
        }
        #[unsafe(method(toggle:))]
        fn toggle_action(&self, _sender: Option<&AnyObject>) {
            if self.ivars().permission_rx.borrow().is_none() { self.with_controller(|c| c.toggle()); }
            self.refresh();
        }
        #[unsafe(method(cancel:))]
        fn cancel_action(&self, _sender: Option<&AnyObject>) {
            self.with_controller(|c| c.cancel()); self.refresh();
        }
        #[unsafe(method(retry:))]
        fn retry_action(&self, _sender: Option<&AnyObject>) {
            self.with_controller(|c| c.retry()); self.refresh();
        }
        #[unsafe(method(copyText:))]
        fn copy_action(&self, _sender: Option<&AnyObject>) {
            self.with_controller(|c| { c.message = if Delivery::copy(&c.transcript) { "文字已复制。" } else { "复制失败，请选中文字手动复制。" }.into(); }); self.refresh();
        }
        #[unsafe(method(showWindow:))]
        fn show_action(&self, _sender: Option<&AnyObject>) { self.show_window(); }
        #[unsafe(method(selectPage:))]
        fn page_action(&self, _sender: Option<&AnyObject>) {
            let settings = self.ivars().page_tabs.get().is_some_and(|tabs| tabs.selectedSegment() == 1);
            self.select_page(settings);
        }
        #[unsafe(method(showPreferences:))]
        fn preferences_action(&self, _sender: Option<&AnyObject>) {
            self.show_window(); self.select_page(true);
        }
        #[unsafe(method(resolveNotice:))]
        fn notice_action(&self, _sender: Option<&AnyObject>) {
            let idle = self.ivars().controller.borrow().as_ref().is_some_and(|c| !c.busy());
            if idle && self.ivars().hotkey_error.borrow().is_some() {
                self.select_page(true);
            } else if idle && !Delivery::trusted() && self.ivars().controller.borrow().as_ref().is_some_and(|c| c.automatic) {
                Delivery::request_accessibility();
            } else { self.show_details(); }
        }
        #[unsafe(method(showDetails:))]
        fn details_action(&self, _sender: Option<&AnyObject>) { self.show_details(); }
        #[unsafe(method(requestAccessibility:))]
        fn accessibility_action(&self, _sender: Option<&AnyObject>) { Delivery::request_accessibility(); }
        #[unsafe(method(toggleAutomatic:))]
        fn automatic_action(&self, _sender: Option<&AnyObject>) {
            self.with_controller(|c| { c.automatic = !c.automatic; NSUserDefaults::standardUserDefaults().setBool_forKey(c.automatic, ns_string!("automaticPaste")); }); self.refresh();
        }
        #[unsafe(method(selectSource:))]
        fn source_action(&self, _sender: Option<&AnyObject>) {
            if self.ivars().permission_rx.borrow().is_some() { return; }
            self.with_controller(|c| {
                if c.busy() { return; }
                let index = self.ivars().source.get().map(|control| control.indexOfSelectedItem()).unwrap_or(-1);
                if let Some((id, _)) = usize::try_from(index).ok().and_then(|i| self.ivars().source_rows.borrow().get(i).cloned()) {
                    c.source_id = id;
                    unsafe { NSUserDefaults::standardUserDefaults().setObject_forKey(Some(&NSString::from_str(&c.source_id)), ns_string!("transcriptionSource")); }
                }
            }); self.refresh();
        }
        #[unsafe(method(refreshModels:))]
        fn refresh_models_action(&self, _sender: Option<&AnyObject>) {
            if self.ivars().permission_rx.borrow().is_none() { self.with_controller(|c| c.refresh_catalog()); }
            self.refresh();
        }
        #[unsafe(method(cycleLanguage:))]
        fn language_action(&self, _sender: Option<&AnyObject>) {
            self.with_controller(|c| {
                if c.busy() { return; }
                let selected = self.ivars().language.get().map(|control| control.indexOfSelectedItem()).unwrap_or(0);
                c.language = match selected { 1 => "zh", 2 => "en", _ => "" }.into();
                unsafe { NSUserDefaults::standardUserDefaults().setObject_forKey(Some(&NSString::from_str(&c.language)), ns_string!("language")); }
            }); self.refresh();
        }
        #[unsafe(method(cycleShortcut:))]
        fn shortcut_action(&self, _sender: Option<&AnyObject>) {
            let alternative = self.ivars().shortcut.get().is_some_and(|control| control.indexOfSelectedItem() == 1);
            self.ivars().alternative.set(alternative);
            NSUserDefaults::standardUserDefaults().setBool_forKey(alternative, ns_string!("alternativeShortcut"));
            self.register_hotkey(); self.refresh();
        }
        #[unsafe(method(tick:))]
        fn tick(&self, _sender: Option<&AnyObject>) {
            let permission_reply = self.ivars().permission_rx.borrow().as_ref().and_then(|rx| rx.try_recv().ok());
            if let Some((_, crate::audio::AudioEvent::Permissions { microphone, speech })) = permission_reply {
                self.ivars().permission_rx.borrow_mut().take();
                self.with_controller(|c| c.message = format!("权限检查：麦克风{}；{}；辅助功能{}。未开始录音。", if microphone { "已允许" } else { "未允许" }, if !c.uses_system_speech() { "本地模型无需系统语音识别权限" } else if speech { "系统语音识别已允许" } else { "系统语音识别未允许（无设备端预览）" }, if Delivery::trusted() { "已允许" } else { "未允许" }));
            }
            let pressed = self.ivars().hotkey_rx.borrow().as_ref().is_some_and(|rx| rx.try_recv().is_ok()) && self.ivars().permission_rx.borrow().is_none();
            self.with_controller(|c| {
                if self.ivars().quitting.get() && !self.ivars().shutdown_started.get() {
                    c.cancel(); self.ivars().shutdown_started.set(true);
                }
                c.poll();
                if pressed && !self.ivars().quitting.get() { c.toggle(); }
            });
            self.refresh();
            if self.ivars().quitting.get() {
                let stopped = self.ivars().shutdown_started.get()
                    && self.ivars().controller.try_borrow().is_ok_and(|slot| slot.as_ref().is_none_or(|c| c.state.phase != Phase::Stopping));
                if stopped { NSApplication::sharedApplication(self.mtm()).replyToApplicationShouldTerminate(true); }
            }
        }
    }
);
impl Delegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars::default());
        unsafe { msg_send![super(this), init] }
    }
    fn with_controller(&self, action: impl FnOnce(&mut Controller)) {
        if let Ok(mut slot) = self.ivars().controller.try_borrow_mut()
            && let Some(controller) = slot.as_mut()
        {
            action(controller);
        }
    }
    fn menu_item(
        &self,
        title: &str,
        action: objc2::runtime::Sel,
        key: &str,
    ) -> Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm()),
                &NSString::from_str(title),
                Some(action),
                &NSString::from_str(key),
            )
        };
        if action != sel!(terminate:) {
            unsafe {
                item.setTarget(Some(self));
            }
        }
        item
    }
    fn build_menu(&self) {
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        menu.setAutoenablesItems(false);
        let heading = self.menu_item("VoicePort · 就绪", sel!(showWindow:), "");
        heading.setEnabled(false);
        menu.addItem(&heading);
        let _ = self.ivars().menu_state.set(heading);
        let shortcut = self.menu_item("⌃⌥空格 · 开始 / 停止听写", sel!(showWindow:), "");
        shortcut.setEnabled(false);
        menu.addItem(&shortcut);
        let _ = self.ivars().menu_shortcut.set(shortcut);
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        for (title, action, symbol) in [
            ("打开结果与设置…", sel!(showWindow:), "macwindow"),
            ("开始听写", sel!(toggle:), "mic"),
            ("复制最近文字", sel!(copyText:), "doc.on.doc"),
            ("取消听写", sel!(cancel:), "xmark.circle"),
        ] {
            let item = self.menu_item(title, action, "");
            item.setImage(symbol_image(symbol, 15.).as_deref());
            menu.addItem(&item);
            if action == sel!(toggle:) {
                let _ = self.ivars().menu_toggle.set(item);
            } else if action == sel!(cancel:) {
                let _ = self.ivars().menu_cancel.set(item);
            } else if action == sel!(copyText:) {
                let _ = self.ivars().menu_copy.set(item);
            }
        }
        menu.addItem(&NSMenuItem::separatorItem(mtm));
        menu.addItem(&self.menu_item("退出 VoicePort", sel!(terminate:), ""));
        // A fixed, compact slot avoids shifting neighboring menu extras as
        // recording and processing states change. Template images follow macOS.
        let status = NSStatusBar::systemStatusBar().statusItemWithLength(28.);
        status.setAutosaveName(Some(ns_string!("VoicePortStatusItem")));
        if let Some(button) = status.button(mtm) {
            self.update_status_icon(&button, StatusAppearance::Ready);
            button.setAccessibilityIdentifier(Some(ns_string!("voiceport.menu-bar")));
            button.setAccessibilityLabel(Some(ns_string!("VoicePort · 就绪")));
            button.setToolTip(Some(ns_string!("VoicePort · 点击打开菜单")));
        }
        status.setVisible(true);
        status.setMenu(Some(&menu));
        let _ = self.ivars().status.set(status);

        let main_menu = NSMenu::new(mtm);
        let application_item = NSMenuItem::new(mtm);
        let application_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("VoicePort"));
        application_menu.addItem(&self.menu_item("结果与设置…", sel!(showWindow:), ","));
        application_menu.addItem(&NSMenuItem::separatorItem(mtm));
        application_menu.addItem(&self.menu_item("退出 VoicePort", sel!(terminate:), "q"));
        application_item.setSubmenu(Some(&application_menu));
        main_menu.addItem(&application_item);
        NSApplication::sharedApplication(mtm).setMainMenu(Some(&main_menu));
    }
    fn update_status_icon(&self, button: &NSStatusBarButton, appearance: StatusAppearance) {
        if self.ivars().status_appearance.get() == Some(appearance) {
            return;
        }
        let image = crate::status_icon::image();
        button.setTitle(ns_string!(""));
        button.setImage(Some(&image));
        button.setImagePosition(NSCellImagePosition::ImageOnly);
        let tint = (appearance == StatusAppearance::Recording).then(NSColor::systemRedColor);
        button.setContentTintColor(tint.as_deref());
        self.ivars().status_appearance.set(Some(appearance));
    }
    fn initialize(&self) {
        let defaults = NSUserDefaults::standardUserDefaults();
        let mut controller = Controller::new();
        if defaults
            .objectForKey(ns_string!("automaticPaste"))
            .is_some()
        {
            controller.automatic = defaults.boolForKey(ns_string!("automaticPaste"));
        }
        controller.language = defaults
            .stringForKey(ns_string!("language"))
            .map(|s| s.to_string())
            .filter(|s| matches!(s.as_str(), "" | "zh" | "en"))
            .unwrap_or_default();
        controller.source_id = defaults
            .stringForKey(ns_string!("transcriptionSource"))
            .map(|s| s.to_string())
            .unwrap_or_else(|| crate::local_models::CHATGPT.into());
        controller.catalog = crate::local_models::Catalog::pending(&controller.source_id);
        controller.refresh_catalog();
        self.ivars()
            .alternative
            .set(defaults.boolForKey(ns_string!("alternativeShortcut")));
        *self.ivars().controller.borrow_mut() = Some(controller);
        let _ = crate::storage::cleanup_expired();
        self.register_hotkey();
        let timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.1,
                self,
                sel!(tick:),
                None,
                true,
            )
        };
        // TerminateLater and native panels run a modal loop. The shutdown
        // poll must keep running there, as well as while a menu is tracking.
        unsafe {
            let run_loop = NSRunLoop::currentRunLoop();
            run_loop.addTimer_forMode(&timer, NSRunLoopCommonModes);
            run_loop.addTimer_forMode(&timer, NSModalPanelRunLoopMode);
        }
        let _ = self.ivars().timer.set(timer);
    }
    fn register_hotkey(&self) {
        self.ivars().hotkey.borrow_mut().take();
        self.ivars().hotkey_rx.borrow_mut().take();
        self.ivars().hotkey_error.borrow_mut().take();
        let (tx, rx) = std::sync::mpsc::channel();
        let mut hotkey = crate::hotkey::GlobalHotKey::new(tx);
        match hotkey.register(self.ivars().alternative.get()) {
            Ok(()) => {
                *self.ivars().hotkey.borrow_mut() = Some(hotkey);
                *self.ivars().hotkey_rx.borrow_mut() = Some(rx);
            }
            Err(message) => {
                *self.ivars().hotkey_error.borrow_mut() = Some(message.clone());
                self.with_controller(|c| c.message = message);
            }
        }
    }
    fn button(
        &self,
        view: &NSView,
        title: &str,
        action: objc2::runtime::Sel,
        x: f64,
        y: f64,
        width: f64,
    ) -> Retained<NSButton> {
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str(title),
                Some(self),
                Some(action),
                self.mtm(),
            )
        };
        button.setFrame(rect(x, y, width, 32.));
        button.setAccessibilityIdentifier(Some(&NSString::from_str(&format!(
            "voiceport.{}",
            action.name().to_string_lossy()
        ))));
        view.addSubview(&button);
        button
    }
    fn label(&self, view: &NSView, text: &str, frame: NSRect, size: f64) -> Retained<NSTextField> {
        let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), self.mtm());
        label.setFrame(frame);
        label.setFont(Some(&NSFont::systemFontOfSize(size)));
        label.setTextColor(Some(&NSColor::labelColor()));
        view.addSubview(&label);
        label
    }
    fn caption(&self, view: &NSView, text: &str, frame: NSRect) -> Retained<NSTextField> {
        let label = self.label(view, text, frame, 11.);
        label.setTextColor(Some(&NSColor::secondaryLabelColor()));
        label
    }
    fn surface(&self, view: &NSView, frame: NSRect) {
        let card = NSBox::initWithFrame(NSBox::alloc(self.mtm()), frame);
        card.setBoxType(NSBoxType::Custom);
        card.setTitlePosition(NSTitlePosition::NoTitle);
        card.setCornerRadius(10.);
        card.setBorderWidth(0.5);
        card.setBorderColor(&NSColor::separatorColor());
        card.setFillColor(&NSColor::controlBackgroundColor());
        view.addSubview(&card);
    }
    fn icon(&self, view: &NSView, symbol: &str, frame: NSRect) -> Retained<NSImageView> {
        let image = NSImageView::initWithFrame(NSImageView::alloc(self.mtm()), frame);
        image.setImage(symbol_image(symbol, frame.size.height).as_deref());
        image.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        view.addSubview(&image);
        image
    }
    fn popup(
        &self,
        view: &NSView,
        titles: &[&str],
        action: objc2::runtime::Sel,
        frame: NSRect,
    ) -> Retained<NSPopUpButton> {
        let popup =
            NSPopUpButton::initWithFrame_pullsDown(NSPopUpButton::alloc(self.mtm()), frame, false);
        for title in titles {
            popup.addItemWithTitle(&NSString::from_str(title));
        }
        unsafe {
            popup.setTarget(Some(self));
            popup.setAction(Some(action));
        }
        popup.setFont(Some(&NSFont::systemFontOfSize(12.)));
        view.addSubview(&popup);
        popup
    }
    fn select_page(&self, settings: bool) {
        if let Some(page) = self.ivars().dictation_page.get() {
            page.setHidden(settings);
        }
        if let Some(page) = self.ivars().settings_page.get() {
            page.setHidden(!settings);
        }
        if let Some(tabs) = self.ivars().page_tabs.get() {
            tabs.setSelectedSegment(if settings { 1 } else { 0 });
        }
    }
    fn show_details(&self) {
        let Some(window) = self.ivars().window.get() else {
            return;
        };
        if window.attachedSheet().is_some() {
            return;
        }
        let Ok(slot) = self.ivars().controller.try_borrow() else {
            return;
        };
        let Some(c) = slot.as_ref() else {
            return;
        };
        let details = format!(
            "{}\n\n{} · {} 秒\n{}",
            c.message,
            c.preview_status,
            c.seconds(),
            c.delivery_status
        );
        let alert = NSAlert::new(self.mtm());
        alert.setMessageText(ns_string!("听写状态详情"));
        alert.setInformativeText(&NSString::from_str(&details));
        alert.addButtonWithTitle(ns_string!("知道了"));
        alert.beginSheetModalForWindow_completionHandler(window, None);
    }
    fn build_dictation_page(&self, page: &NSView) {
        let title = self.label(page, "让想法，落在光标处。", rect(0., 380., 550., 28.), 21.);
        title.setFont(Some(&NSFont::systemFontOfSize_weight(21., unsafe {
            NSFontWeightSemibold
        })));
        self.caption(
            page,
            "先点好输入位置，再用快捷键开始。",
            rect(0., 358., 550., 18.),
        );

        self.surface(page, rect(0., 282., 572., 62.));
        let _ = self.ivars().notice_icon.set(self.icon(
            page,
            "checkmark.circle",
            rect(16., 302., 22., 22.),
        ));
        let message = self.label(page, "", rect(50., 315., 392., 20.), 13.);
        message.setFont(Some(&NSFont::systemFontOfSize_weight(13., unsafe {
            NSFontWeightMedium
        })));
        let _ = self.ivars().message.set(message);
        let details = self.caption(page, "", rect(50., 291., 392., 24.));
        details.setMaximumNumberOfLines(2);
        let _ = self.ivars().details.set(details);
        let action = self.button(page, "查看详情", sel!(resolveNotice:), 454., 297., 104.);
        action.setControlSize(NSControlSize::Small);
        let _ = self.ivars().notice_action.set(action);

        self.surface(page, rect(0., 88., 572., 178.));
        self.caption(page, "本次转写", rect(16., 236., 300., 18.));
        let copy = self.button(page, "复制文字", sel!(copyText:), 472., 228., 88.);
        copy.setImage(symbol_image("doc.on.doc", 12.).as_deref());
        copy.setImagePosition(NSCellImagePosition::ImageLeading);
        copy.setControlSize(NSControlSize::Small);
        let _ = self.ivars().copy.set(copy);
        let scroll = NSScrollView::initWithFrame(
            NSScrollView::alloc(self.mtm()),
            rect(10., 100., 552., 128.),
        );
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        let text =
            NSTextView::initWithFrame(NSTextView::alloc(self.mtm()), rect(0., 0., 530., 128.));
        text.setEditable(false);
        text.setSelectable(true);
        text.setFont(Some(&NSFont::systemFontOfSize(14.)));
        text.setTextColor(Some(&NSColor::textColor()));
        text.setDrawsBackground(false);
        text.setTextContainerInset(NSSize::new(6., 6.));
        text.setAccessibilityIdentifier(Some(ns_string!("voiceport.transcript")));
        text.setAccessibilityLabel(Some(ns_string!("识别结果")));
        scroll.setDocumentView(Some(&text));
        page.addSubview(&scroll);
        let _ = self.ivars().transcript.set(text);
        let empty = NSView::initWithFrame(NSView::alloc(self.mtm()), rect(0., 110., 572., 114.));
        self.icon(&empty, "waveform", rect(270., 72., 32., 28.));
        let hint = self.label(
            &empty,
            "说句话，让文字自然出现。",
            rect(0., 40., 572., 22.),
            14.,
        );
        hint.setAlignment(NSTextAlignment::Center);
        let hint = self.caption(
            &empty,
            "停录后生成最终文字，你来确认发送。",
            rect(0., 18., 572., 18.),
        );
        hint.setAlignment(NSTextAlignment::Center);
        page.addSubview(&empty);
        let _ = self.ivars().empty_hint.set(empty);

        let toggle = self.button(page, "开始听写", sel!(toggle:), 0., 30., 136.);
        toggle.setFrame(rect(0., 30., 136., 36.));
        toggle.setControlSize(NSControlSize::Large);
        toggle.setBezelStyle(NSBezelStyle::Push);
        toggle.setBezelColor(Some(&NSColor::controlAccentColor()));
        toggle.setImage(symbol_image("mic.fill", 15.).as_deref());
        toggle.setImagePosition(NSCellImagePosition::ImageLeading);
        let _ = self.ivars().toggle.set(toggle);
        let hint = self.caption(page, "", rect(152., 38., 168., 20.));
        let _ = self.ivars().shortcut_hint.set(hint);
        let _ =
            self.ivars()
                .cancel
                .set(self.button(page, "取消听写", sel!(cancel:), 360., 32., 96.));
        let _ =
            self.ivars()
                .retry
                .set(self.button(page, "重试转写", sel!(retry:), 464., 32., 108.));
        self.caption(
            page,
            "文字不会自动发送；关闭窗口后，可从右上角图标打开。",
            rect(0., 0., 540., 18.),
        );
        let info = self.button(page, "", sel!(showDetails:), 548., -4., 24.);
        info.setBordered(false);
        info.setImage(symbol_image("info.circle", 14.).as_deref());
        info.setToolTip(Some(ns_string!("查看完整听写状态")));
        info.setAccessibilityLabel(Some(ns_string!("查看完整听写状态")));
    }
    fn build_settings_page(&self, page: &NSView) {
        let title = self.label(page, "偏好设置", rect(0., 380., 550., 28.), 21.);
        title.setFont(Some(&NSFont::systemFontOfSize_weight(21., unsafe {
            NSFontWeightSemibold
        })));
        self.caption(
            page,
            "按你的习惯，调整每一次听写。",
            rect(0., 358., 550., 18.),
        );
        self.surface(page, rect(0., 100., 572., 244.));
        self.label(page, "转写来源", rect(16., 312., 90., 20.), 13.);
        let source = self.popup(page, &[], sel!(selectSource:), rect(112., 305., 360., 30.));
        source.setAccessibilityLabel(Some(ns_string!("转写来源")));
        let _ = self.ivars().source.set(source);
        let _ = self.ivars().refresh_models.set(self.button(
            page,
            "刷新",
            sel!(refreshModels:),
            480.,
            305.,
            76.,
        ));
        let _ = self.ivars().source_detail.set(self.caption(
            page,
            "正在检查已安装的模型…",
            rect(16., 268., 540., 32.),
        ));
        for (y, title, subtitle) in [
            (
                242.,
                "自动输入到原光标",
                "停止后，将文字送回开始时的输入位置。",
            ),
            (
                188.,
                "识别语言",
                "自动检测、中文或 English；Paraformer 为中英双语。",
            ),
            (
                134.,
                "开始 / 停止快捷键",
                "在其他应用中，也能随时开始听写。",
            ),
        ] {
            self.label(page, title, rect(16., y, 350., 20.), 13.);
            self.caption(page, subtitle, rect(16., y - 20., 350., 18.));
        }
        for y in [260., 206., 152.] {
            let line = NSBox::initWithFrame(NSBox::alloc(self.mtm()), rect(16., y, 540., 1.));
            line.setBoxType(NSBoxType::Separator);
            page.addSubview(&line);
        }
        let automatic =
            NSSwitch::initWithFrame(NSSwitch::alloc(self.mtm()), rect(512., 234., 42., 28.));
        unsafe {
            automatic.setTarget(Some(self));
            automatic.setAction(Some(sel!(toggleAutomatic:)));
        }
        automatic.setAccessibilityLabel(Some(ns_string!("自动输入到原光标")));
        page.addSubview(&automatic);
        let _ = self.ivars().automatic.set(automatic);
        let language = self.popup(
            page,
            &["自动检测", "中文", "English"],
            sel!(cycleLanguage:),
            rect(424., 180., 132., 30.),
        );
        language.setAccessibilityLabel(Some(ns_string!("识别语言")));
        let _ = self.ivars().language.set(language);
        let shortcut = self.popup(
            page,
            &["⌃ ⌥ 空格", "⌃ ⇧ 空格"],
            sel!(cycleShortcut:),
            rect(424., 126., 132., 30.),
        );
        shortcut.setAccessibilityLabel(Some(ns_string!("开始 / 停止快捷键")));
        let _ = self.ivars().shortcut.set(shortcut);

        self.surface(page, rect(0., 24., 572., 68.));
        self.icon(page, "hand.raised", rect(16., 60., 20., 22.));
        self.label(page, "辅助功能", rect(46., 68., 300., 20.), 13.);
        self.caption(
            page,
            "允许 VoicePort 将文字输入其他应用。",
            rect(46., 48., 360., 18.),
        );
        let accessibility = self.button(
            page,
            "前往授权",
            sel!(requestAccessibility:),
            440.,
            54.,
            116.,
        );
        let _ = self.ivars().accessibility.set(accessibility);
        let prepare = self.button(
            page,
            "检查麦克风与语音权限…",
            sel!(preparePermissions:),
            12.,
            24.,
            300.,
        );
        prepare.setBordered(false);
        prepare.setAlignment(NSTextAlignment::Left);
        prepare.setContentTintColor(Some(&NSColor::controlAccentColor()));
        prepare.setFont(Some(&NSFont::systemFontOfSize(12.)));
        let _ = self.ivars().prepare_permissions.set(prepare);
        self.caption(
            page,
            "检查权限不会开始录音。转写完成后，仍由你确认发送。",
            rect(0., 0., 572., 18.),
        );
    }
    fn show_window(&self) {
        let mtm = self.mtm();
        if self.ivars().window.get().is_none() {
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    rect(0., 0., 620., 520.),
                    NSWindowStyleMask::Titled
                        | NSWindowStyleMask::Closable
                        | NSWindowStyleMask::Miniaturizable,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe {
                window.setReleasedWhenClosed(false);
            }
            window.setTitle(ns_string!("VoicePort"));
            window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
            window.setTitlebarAppearsTransparent(true);
            window.setBackgroundColor(Some(&NSColor::windowBackgroundColor()));
            let view = window.contentView().expect("native window content view");
            let brand = self.icon(&view, "waveform.circle.fill", rect(24., 464., 32., 32.));
            brand.setContentTintColor(Some(&NSColor::controlAccentColor()));
            let heading = self.label(&view, "VoicePort", rect(66., 478., 280., 24.), 17.);
            heading.setFont(Some(&NSFont::systemFontOfSize_weight(17., unsafe {
                NSFontWeightSemibold
            })));
            self.caption(&view, "随口说，自然成文。", rect(66., 460., 280., 18.));
            let tabs = NSSegmentedControl::initWithFrame(
                NSSegmentedControl::alloc(mtm),
                rect(416., 468., 180., 28.),
            );
            tabs.setSegmentCount(2);
            tabs.setLabel_forSegment(ns_string!("听写"), 0);
            tabs.setLabel_forSegment(ns_string!("设置"), 1);
            tabs.setWidth_forSegment(86., 0);
            tabs.setWidth_forSegment(86., 1);
            tabs.setTrackingMode(NSSegmentSwitchTracking::SelectOne);
            tabs.setSelectedSegment(0);
            unsafe {
                tabs.setTarget(Some(self));
                tabs.setAction(Some(sel!(selectPage:)));
            }
            tabs.setAccessibilityIdentifier(Some(ns_string!("voiceport.pages")));
            view.addSubview(&tabs);
            let _ = self.ivars().page_tabs.set(tabs);
            let line = NSBox::initWithFrame(NSBox::alloc(mtm), rect(24., 444., 572., 1.));
            line.setBoxType(NSBoxType::Separator);
            view.addSubview(&line);
            let dictation = NSView::initWithFrame(NSView::alloc(mtm), rect(24., 20., 572., 408.));
            self.build_dictation_page(&dictation);
            view.addSubview(&dictation);
            let _ = self.ivars().dictation_page.set(dictation);
            let settings = NSView::initWithFrame(NSView::alloc(mtm), rect(24., 20., 572., 408.));
            self.build_settings_page(&settings);
            settings.setHidden(true);
            view.addSubview(&settings);
            let _ = self.ivars().settings_page.set(settings);
            window.center();
            let _ = self.ivars().window.set(window);
        }
        self.refresh();
        if let Some(window) = self.ivars().window.get() {
            window.makeKeyAndOrderFront(None);
        }
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
    }
    fn refresh(&self) {
        let Ok(slot) = self.ivars().controller.try_borrow() else {
            return;
        };
        let Some(c) = slot.as_ref() else {
            return;
        };
        let preparing_permissions = self.ivars().permission_rx.borrow().is_some();
        if let Some(button) = self.ivars().prepare_permissions.get() {
            let enabled = !c.busy() && !preparing_permissions;
            if button.isEnabled() != enabled {
                button.setEnabled(enabled);
            }
            let title = if c.uses_system_speech() {
                "检查麦克风与语音权限…"
            } else {
                "检查麦克风权限（本地模型）…"
            };
            if button.title().to_string() != title {
                button.setTitle(&NSString::from_str(title));
            }
        }
        let recording = c.state.phase == Phase::Recording;
        let trusted = Delivery::trusted();
        let shortcut = if self.ivars().alternative.get() {
            "⌃⇧空格"
        } else {
            "⌃⌥空格"
        };
        let hotkey_failed = self.ivars().hotkey_error.borrow().is_some();
        let (appearance, state_text) = match c.state.phase {
            Phase::Recording => (StatusAppearance::Recording, "正在听写"),
            Phase::Permissions => (StatusAppearance::Working, "正在准备权限"),
            Phase::Transcribing => (StatusAppearance::Working, "正在转写"),
            Phase::Delivering => (StatusAppearance::Working, "正在输入文字"),
            Phase::Stopping => (StatusAppearance::Working, "正在停止"),
            _ if preparing_permissions => (StatusAppearance::Working, "正在准备权限"),
            _ if hotkey_failed => (StatusAppearance::Attention, "快捷键需要设置"),
            _ if c.automatic && !trusted => (StatusAppearance::Attention, "需要辅助功能授权"),
            Phase::Failed => (StatusAppearance::Attention, "需要处理 · 打开查看"),
            Phase::Ready => (StatusAppearance::Ready, "转写完成"),
            Phase::Idle => (StatusAppearance::Ready, "就绪"),
        };
        if let Some(item) = self.ivars().menu_state.get() {
            item.setTitle(&NSString::from_str(&format!("VoicePort · {state_text}")));
        }
        if let Some(item) = self.ivars().menu_shortcut.get() {
            item.setTitle(&NSString::from_str(&format!(
                "{shortcut} · {}",
                if hotkey_failed {
                    "快捷键未注册，请在设置中更换"
                } else {
                    "开始 / 停止听写"
                }
            )));
        }
        if let Some(item) = self.ivars().menu_cancel.get() {
            item.setEnabled(c.busy());
        }
        if let Some(item) = self.ivars().menu_copy.get() {
            item.setEnabled(!c.transcript.is_empty());
        }
        if let Some(hint) = self.ivars().empty_hint.get() {
            hint.setHidden(!c.transcript.is_empty());
        }
        let title = if recording {
            "停止并转写"
        } else {
            "开始听写"
        };
        if let Some(item) = self.ivars().menu_toggle.get() {
            item.setTitle(&NSString::from_str(title));
            item.setEnabled(!preparing_permissions && (recording || c.state.can_start()));
        }
        if let Some(button) = self.ivars().toggle.get() {
            button.setTitle(&NSString::from_str(title));
            button.setEnabled(!preparing_permissions && (recording || c.state.can_start()));
        }
        let idle = !c.busy() && !preparing_permissions;
        if let Some(control) = self.ivars().source.get() {
            let rows: Vec<_> = c
                .catalog
                .entries
                .iter()
                .map(|entry| (entry.id.clone(), entry.title.clone()))
                .collect();
            if *self.ivars().source_rows.borrow() != rows {
                control.removeAllItems();
                for (_, title) in &rows {
                    control.addItemWithTitle(&NSString::from_str(title));
                }
                *self.ivars().source_rows.borrow_mut() = rows;
            }
            let selected = self
                .ivars()
                .source_rows
                .borrow()
                .iter()
                .position(|(id, _)| id == &c.source_id)
                .map(|i| i as isize);
            if let Some(selected) = selected
                && control.indexOfSelectedItem() != selected
            {
                control.selectItemAtIndex(selected);
            }
            if control.isEnabled() != idle {
                control.setEnabled(idle);
            }
        }
        if let Some(label) = self.ivars().source_detail.get() {
            let detail = c.catalog.detail(&c.source_id);
            if label.stringValue().to_string() != detail {
                label.setStringValue(&NSString::from_str(detail));
            }
            label.setToolTip(Some(&NSString::from_str(detail)));
        }
        if let Some(button) = self.ivars().refresh_models.get() {
            let enabled = idle && !c.discovering();
            if button.isEnabled() != enabled {
                button.setEnabled(enabled);
            }
        }
        let full_details = NSString::from_str(&format!(
            "{}\n{} · {} 秒\n{}",
            c.message,
            c.preview_status,
            c.seconds(),
            c.delivery_status
        ));
        if let Some(label) = self.ivars().message.get() {
            label.setStringValue(&NSString::from_str(state_text));
            label.setToolTip(Some(&full_details));
        }
        let description = if idle && hotkey_failed {
            "当前快捷键不可用，请到设置里换一组。".into()
        } else if idle && c.automatic && !trusted {
            "在系统设置中重新允许当前版本，即可输入到其他应用。".into()
        } else if recording {
            format!("{} · {} 秒", c.preview_status, c.seconds())
        } else {
            // Keep technical diagnostics available in the detail sheet while
            // presenting the actionable message without its internal stage tag.
            let message = if c.message.starts_with('[') {
                c.message
                    .split_once("] ")
                    .map_or(c.message.as_str(), |(_, text)| text)
            } else {
                &c.message
            };
            message.to_string()
        };
        if let Some(label) = self.ivars().details.get() {
            label.setStringValue(&NSString::from_str(&description));
            label.setToolTip(Some(&full_details));
        }
        if let Some(button) = self.ivars().notice_action.get() {
            button.setTitle(&NSString::from_str(if idle && hotkey_failed {
                "设置快捷键"
            } else if idle && c.automatic && !trusted {
                "前往授权"
            } else {
                "查看详情"
            }));
        }
        if self.ivars().notice_appearance.get() != Some(appearance)
            && let Some(icon) = self.ivars().notice_icon.get()
        {
            icon.setImage(symbol_image(appearance.symbol(), 22.).as_deref());
            let tint = match appearance {
                StatusAppearance::Ready => NSColor::systemGreenColor(),
                StatusAppearance::Recording => NSColor::systemRedColor(),
                StatusAppearance::Working => NSColor::controlAccentColor(),
                StatusAppearance::Attention => NSColor::systemOrangeColor(),
            };
            icon.setContentTintColor(Some(&tint));
            self.ivars().notice_appearance.set(Some(appearance));
        }
        if let Some(label) = self.ivars().shortcut_hint.get() {
            label.setStringValue(&NSString::from_str(&format!("{shortcut}  开始 / 停止")));
        }
        if let Some(button) = self.ivars().cancel.get() {
            button.setHidden(!c.busy());
        }
        if let Some(button) = self.ivars().copy.get() {
            button.setHidden(c.transcript.is_empty());
        }
        if let Some(text) = self.ivars().transcript.get()
            && text.string().to_string() != c.transcript
        {
            text.setString(&NSString::from_str(&c.transcript));
        }
        if let Some(button) = self.ivars().retry.get() {
            let available = c.state.retry_available && c.state.phase == Phase::Failed;
            button.setHidden(!available);
            button.setEnabled(available);
        }
        if let Some(control) = self.ivars().automatic.get() {
            let state = if c.automatic {
                NSControlStateValueOn
            } else {
                NSControlStateValueOff
            };
            if control.state() != state {
                control.setState(state);
            }
            if control.isEnabled() != idle {
                control.setEnabled(idle);
            }
        }
        if let Some(control) = self.ivars().language.get() {
            let selected = match c.language.as_str() {
                "zh" => 1,
                "en" => 2,
                _ => 0,
            };
            // Re-selecting even the same item dismisses an open native menu.
            // Common-mode timer ticks must leave active control tracking alone.
            if control.indexOfSelectedItem() != selected {
                control.selectItemAtIndex(selected);
            }
            if control.isEnabled() != idle {
                control.setEnabled(idle);
            }
        }
        if let Some(control) = self.ivars().shortcut.get() {
            let selected = if self.ivars().alternative.get() { 1 } else { 0 };
            if control.indexOfSelectedItem() != selected {
                control.selectItemAtIndex(selected);
            }
            control.setToolTip(Some(&NSString::from_str(if hotkey_failed {
                "快捷键未注册，请选择另一组。"
            } else {
                "在目标应用中按此快捷键开始和停止听写。"
            })));
            if control.isEnabled() != idle {
                control.setEnabled(idle);
            }
        }
        if let Some(button) = self.ivars().accessibility.get() {
            button.setTitle(if trusted {
                ns_string!("已授权 · 检查")
            } else {
                ns_string!("未授权 · 设置")
            });
            button.setAccessibilityLabel(Some(if trusted {
                ns_string!("辅助功能已授权，点击检查")
            } else {
                ns_string!("辅助功能未授权，前往系统设置")
            }));
        }
        if let Some(status) = self.ivars().status.get().and_then(|s| s.button(self.mtm())) {
            self.update_status_icon(&status, appearance);
            let description = NSString::from_str(&format!("VoicePort · {state_text}"));
            status.setAccessibilityLabel(Some(&description));
            status.setToolTip(Some(&NSString::from_str(&format!(
                "VoicePort · {state_text}\n{shortcut} 开始 / 停止听写\n点击打开菜单"
            ))));
        }
    }
}
fn symbol_image(name: &str, size: f64) -> Option<Retained<NSImage>> {
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &NSString::from_str(name),
        None,
    )?;
    let configuration =
        NSImageSymbolConfiguration::configurationWithPointSize_weight(size, unsafe {
            NSFontWeightRegular
        });
    let image = image
        .imageWithSymbolConfiguration(&configuration)
        .unwrap_or(image);
    image.setTemplate(true);
    image.setSize(NSSize::new(size, size));
    Some(image)
}

fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

pub fn run() {
    let arguments: Vec<_> = std::env::args().skip(1).collect();
    if arguments.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "VoicePort Rust macOS\n  --show-window             Show results and settings (also shown on normal launch)\n  --smoke-test              Build menu and exit without recording or network\n  --check-input-permission  Print input authorization JSON for this execution context\n  --diagnose-input          Inspect focused input metadata without writing\n  --input-self-test         Write fixed text only to the dedicated development fixture\n  --help                   Show this help"
        );
        return;
    }
    if arguments.iter().any(|a| {
        !matches!(
            a.as_str(),
            "--smoke-test"
                | "--check-input-permission"
                | "--show-window"
                | "--diagnose-input"
                | "--input-self-test"
        )
    }) {
        eprintln!("Unknown option; use --help");
        std::process::exit(2);
    }
    let mtm = MainThreadMarker::new().expect("VoicePort starts on the main thread");
    if arguments
        .iter()
        .any(|a| a == "--diagnose-input" || a == "--input-self-test")
    {
        let report = if arguments.iter().any(|a| a == "--input-self-test") {
            Delivery::input_self_test()
        } else {
            Delivery::diagnose_input()
        };
        println!("{report}");
        return;
    }
    if arguments.iter().any(|a| a == "--check-input-permission") {
        let bundle = NSBundle::mainBundle();
        let version = bundle
            .objectForInfoDictionaryKey(ns_string!("CFBundleShortVersionString"))
            .and_then(|o| o.downcast::<NSString>().ok())
            .map(|s| s.to_string())
            .unwrap_or_else(|| env!("CARGO_PKG_VERSION").into());
        let (microphone, speech) = crate::audio::Recorder::permission_status();
        println!(
            "{}",
            serde_json::json!({"accessibilityTrusted":Delivery::trusted(),"secureInput":Delivery::secure_input(),"microphone":microphone,"speech":speech,"bundlePath":bundle.bundlePath().to_string(),"version":version,"implementation":"rust"})
        );
        return;
    }
    let app = NSApplication::sharedApplication(mtm);
    let smoke = arguments.iter().any(|a| a == "--smoke-test");
    // A menu bar utility can still present a normal settings window without
    // occupying the Dock or replacing the foreground app's application menu.
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    let delegate = Delegate::new(mtm);
    if smoke {
        delegate.build_menu();
        println!("VoicePort Rust app launched: menu ready, no recording or network");
        return;
    }
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    delegate.initialize();
    app.run();
    if let Some(timer) = delegate.ivars().timer.get() {
        timer.invalidate();
    }
}
