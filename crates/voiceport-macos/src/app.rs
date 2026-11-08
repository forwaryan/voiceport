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

#[derive(Default)]
struct Ivars {
    controller: RefCell<Option<Controller>>,
    status: OnceCell<Retained<NSStatusItem>>,
    menu_toggle: OnceCell<Retained<NSMenuItem>>,
    window: OnceCell<Retained<NSWindow>>,
    message: OnceCell<Retained<NSTextField>>,
    details: OnceCell<Retained<NSTextField>>,
    transcript: OnceCell<Retained<NSTextView>>,
    toggle: OnceCell<Retained<NSButton>>,
    retry: OnceCell<Retained<NSButton>>,
    automatic: OnceCell<Retained<NSButton>>,
    language: OnceCell<Retained<NSButton>>,
    shortcut: OnceCell<Retained<NSButton>>,
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
            self.with_controller(|c| c.message = "正在准备麦克风与语音权限；不会开始录音，请处理系统授权提示。".into());
            self.refresh();
            crate::audio::Recorder::request_permissions(0, sender);
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
        #[unsafe(method(requestAccessibility:))]
        fn accessibility_action(&self, _sender: Option<&AnyObject>) { Delivery::request_accessibility(); }
        #[unsafe(method(toggleAutomatic:))]
        fn automatic_action(&self, _sender: Option<&AnyObject>) {
            self.with_controller(|c| { c.automatic = !c.automatic; NSUserDefaults::standardUserDefaults().setBool_forKey(c.automatic, ns_string!("automaticPaste")); }); self.refresh();
        }
        #[unsafe(method(cycleLanguage:))]
        fn language_action(&self, _sender: Option<&AnyObject>) {
            self.with_controller(|c| {
                if c.busy() { return; }
                c.language = match c.language.as_str() { "" => "zh", "zh" => "en", _ => "" }.into();
                unsafe { NSUserDefaults::standardUserDefaults().setObject_forKey(Some(&NSString::from_str(&c.language)), ns_string!("language")); }
            }); self.refresh();
        }
        #[unsafe(method(cycleShortcut:))]
        fn shortcut_action(&self, _sender: Option<&AnyObject>) {
            let alternative = !self.ivars().alternative.get();
            self.ivars().alternative.set(alternative);
            NSUserDefaults::standardUserDefaults().setBool_forKey(alternative, ns_string!("alternativeShortcut"));
            self.register_hotkey(); self.refresh();
        }
        #[unsafe(method(tick:))]
        fn tick(&self, _sender: Option<&AnyObject>) {
            let permission_reply = self.ivars().permission_rx.borrow().as_ref().and_then(|rx| rx.try_recv().ok());
            if let Some((_, crate::audio::AudioEvent::Permissions { microphone, speech })) = permission_reply {
                self.ivars().permission_rx.borrow_mut().take();
                self.with_controller(|c| c.message = format!("权限检查：麦克风{}；语音识别{}；辅助功能{}。未开始录音。", if microphone { "已允许" } else { "未允许" }, if speech { "已允许" } else { "未允许（无实时预览）" }, if Delivery::trusted() { "已允许" } else { "未允许" }));
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
        for (title, action, key) in [
            ("打开结果与设置…", sel!(showWindow:), ""),
            ("开始听写", sel!(toggle:), ""),
            ("取消听写", sel!(cancel:), ""),
            ("退出 VoicePort", sel!(terminate:), ""),
        ] {
            let item = self.menu_item(title, action, key);
            menu.addItem(&item);
            if action == sel!(toggle:) {
                let _ = self.ivars().menu_toggle.set(item);
            }
        }
        let status =
            NSStatusBar::systemStatusBar().statusItemWithLength(NSVariableStatusItemLength);
        if let Some(button) = status.button(mtm) {
            button.setTitle(ns_string!("VP"));
            if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
                ns_string!("mic.fill"),
                Some(ns_string!("VoicePort · 打开结果与设置")),
            ) {
                image.setTemplate(true);
                image.setSize(NSSize::new(16., 16.));
                button.setImage(Some(&image));
                button.setImagePosition(NSCellImagePosition::ImageLeading);
            }
            button.setToolTip(Some(ns_string!(
                "VoicePort · 点击打开菜单，再选「打开结果与设置」"
            )));
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
    fn label(&self, view: &NSView, text: &str, y: f64, height: f64) -> Retained<NSTextField> {
        let label = NSTextField::wrappingLabelWithString(&NSString::from_str(text), self.mtm());
        label.setFrame(rect(24., y, 652., height));
        view.addSubview(&label);
        label
    }
    fn show_window(&self) {
        let mtm = self.mtm();
        if self.ivars().window.get().is_none() {
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    rect(0., 0., 700., 700.),
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
            window.setTitle(ns_string!("VoicePort · 结果与设置"));
            let view = window.contentView().expect("native window content view");
            self.label(&view, "VoicePort", 646., 30.);
            let _ = self.ivars().message.set(self.label(&view, "", 596., 48.));
            let _ = self.ivars().details.set(self.label(&view, "", 512., 82.));
            let scroll =
                NSScrollView::initWithFrame(NSScrollView::alloc(mtm), rect(24., 290., 652., 214.));
            scroll.setHasVerticalScroller(true);
            scroll.setBorderType(NSBorderType::BezelBorder);
            let text = NSTextView::initWithFrame(NSTextView::alloc(mtm), rect(0., 0., 630., 214.));
            text.setEditable(false);
            text.setSelectable(true);
            text.setAccessibilityIdentifier(Some(ns_string!("voiceport.transcript")));
            text.setAccessibilityLabel(Some(ns_string!("识别结果")));
            scroll.setDocumentView(Some(&text));
            view.addSubview(&scroll);
            let _ = self.ivars().transcript.set(text);
            let _ = self.ivars().toggle.set(self.button(
                &view,
                "开始听写",
                sel!(toggle:),
                24.,
                244.,
                130.,
            ));
            self.button(&view, "取消", sel!(cancel:), 164., 244., 90.);
            self.button(&view, "复制文字", sel!(copyText:), 264., 244., 110.);
            let _ = self.ivars().retry.set(self.button(
                &view,
                "手动重试",
                sel!(retry:),
                384.,
                244.,
                110.,
            ));
            let _ = self.ivars().automatic.set(self.button(
                &view,
                "",
                sel!(toggleAutomatic:),
                24.,
                198.,
                300.,
            ));
            let _ = self.ivars().language.set(self.button(
                &view,
                "",
                sel!(cycleLanguage:),
                350.,
                198.,
                326.,
            ));
            let _ = self.ivars().shortcut.set(self.button(
                &view,
                "",
                sel!(cycleShortcut:),
                24.,
                152.,
                350.,
            ));
            let _ = self.ivars().accessibility.set(self.button(
                &view,
                "检查辅助功能授权",
                sel!(requestAccessibility:),
                394.,
                152.,
                282.,
            ));
            let _ = self.ivars().prepare_permissions.set(self.button(
                &view,
                "准备麦克风与语音权限（不录音）",
                sel!(preparePermissions:),
                24.,
                106.,
                420.,
            ));
            self.label(&view, "自动输入：先点好目标输入框，再按全局快捷键开始；留在原输入位置，按同一快捷键结束。", 62., 38.);
            self.label(&view,"本机识别提供实时预览，停止后由 ChatGPT 完成最终转写。Orca 终端只追加稳定文字；需要改写的最终结果保留在此处。不会自动发送或按回车。",6.,52.);
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
            button.setEnabled(!c.busy() && !preparing_permissions);
        }
        let recording = c.state.phase == Phase::Recording;
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
        if let Some(label) = self.ivars().message.get() {
            label.setStringValue(&NSString::from_str(&c.message));
        }
        if let Some(label) = self.ivars().details.get() {
            label.setStringValue(&NSString::from_str(&format!(
                "{} · {} 秒\n{}",
                c.preview_status,
                c.seconds(),
                c.delivery_status
            )));
        }
        if let Some(text) = self.ivars().transcript.get()
            && text.string().to_string() != c.transcript
        {
            text.setString(&NSString::from_str(&c.transcript));
        }
        if let Some(button) = self.ivars().retry.get() {
            button.setEnabled(c.state.retry_available && c.state.phase == Phase::Failed);
        }
        if let Some(button) = self.ivars().automatic.get() {
            button.setTitle(&NSString::from_str(if c.automatic {
                "自动输入到原光标：开启"
            } else {
                "自动输入到原光标：关闭"
            }));
            button.setEnabled(!c.busy());
        }
        if let Some(button) = self.ivars().language.get() {
            button.setTitle(&NSString::from_str(&format!(
                "识别语言：{}（点击切换）",
                match c.language.as_str() {
                    "zh" => "中文",
                    "en" => "英文",
                    _ => "自动",
                }
            )));
            button.setEnabled(!c.busy());
        }
        if let Some(button) = self.ivars().shortcut.get() {
            let shortcut = if self.ivars().alternative.get() {
                "⌃⇧空格"
            } else {
                "⌃⌥空格"
            };
            let suffix = if self.ivars().hotkey_error.borrow().is_some() {
                "未注册，点击换一组"
            } else {
                "点击切换"
            };
            button.setTitle(&NSString::from_str(&format!(
                "快捷键：{shortcut}（{suffix}）"
            )));
            button.setEnabled(!c.busy());
        }
        if let Some(button) = self.ivars().accessibility.get() {
            button.setTitle(if Delivery::trusted() {
                ns_string!("辅助功能：已授权（检查）")
            } else {
                ns_string!("辅助功能：未授权（前往设置）")
            });
        }
        if let Some(status) = self.ivars().status.get().and_then(|s| s.button(self.mtm())) {
            status.setTitle(&NSString::from_str(if recording {
                "VP ●"
            } else if c.busy() {
                "VP …"
            } else {
                "VP"
            }));
        }
    }
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
    app.setActivationPolicy(if smoke {
        NSApplicationActivationPolicy::Accessory
    } else {
        NSApplicationActivationPolicy::Regular
    });
    let delegate = Delegate::new(mtm);
    delegate.build_menu();
    if smoke {
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
