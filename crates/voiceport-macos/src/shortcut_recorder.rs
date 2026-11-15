//! Focused native recorder. Never posts keyboard events or requests permission.
use crate::shortcut::{COMMAND, CONTROL, OPTION, SHIFT, Shortcut};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained};
use objc2_app_kit::*;
use objc2_foundation::{
    MainThreadMarker, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, ns_string,
};
use std::cell::{Cell, OnceCell};

#[derive(Default)]
struct RecorderIvars {
    candidate: Cell<Option<Shortcut>>,
    label: OnceCell<Retained<NSTextField>>,
    save: OnceCell<Retained<NSButton>>,
}

define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = RecorderIvars]
    struct Recorder;
    unsafe impl NSObjectProtocol for Recorder {}
    impl Recorder {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool { true }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            if let Some(window) = self.window() { window.makeFirstResponder(Some(self)); }
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if !self.capture(event) {
                unsafe { let _: () = msg_send![super(self), keyDown: event]; }
            }
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn key_equivalent(&self, event: &NSEvent) -> bool {
            // Command combinations normally go to application menus first.
            // Only consume them while the recorder itself owns keyboard focus.
            if self.window().is_some_and(|w| w.firstResponder().is_some_and(|r| {
                std::ptr::eq(&*r as *const NSResponder, self as *const Self as *const NSResponder)
            })) { self.capture(event) } else { false }
        }
    }
);

impl Recorder {
    fn new(mtm: MainThreadMarker, save: Retained<NSButton>) -> Retained<Self> {
        let recorder = Self::alloc(mtm).set_ivars(RecorderIvars::default());
        let recorder: Retained<Self> = unsafe {
            msg_send![super(recorder), initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(340., 74.))]
        };
        recorder.setAccessibilityLabel(Some(ns_string!("快捷键录入区域")));
        let label =
            NSTextField::wrappingLabelWithString(ns_string!("请在键盘上按下新的组合键…"), mtm);
        label.setFrame(NSRect::new(NSPoint::new(10., 8.), NSSize::new(320., 58.)));
        label.setAlignment(NSTextAlignment::Center);
        label.setFont(Some(&NSFont::systemFontOfSize(14.)));
        recorder.addSubview(&label);
        let _ = recorder.ivars().label.set(label);
        save.setEnabled(false);
        let _ = recorder.ivars().save.set(save);
        recorder
    }

    fn capture(&self, event: &NSEvent) -> bool {
        if event.r#type() != NSEventType::KeyDown || matches!(event.keyCode(), 36 | 48 | 53 | 76) {
            return false;
        }
        if event.isARepeat() {
            return true;
        }
        let flags = event.modifierFlags();
        let mut modifiers = 0;
        for (flag, bit) in [
            (NSEventModifierFlags::Control, CONTROL),
            (NSEventModifierFlags::Option, OPTION),
            (NSEventModifierFlags::Shift, SHIFT),
            (NSEventModifierFlags::Command, COMMAND),
        ] {
            if flags.contains(flag) {
                modifiers |= bit;
            }
        }
        let result = Shortcut::new(event.keyCode(), modifiers);
        self.ivars().candidate.set(result.ok());
        if let Some(save) = self.ivars().save.get() {
            save.setEnabled(result.is_ok());
        }
        if let Some(label) = self.ivars().label.get() {
            label.setStringValue(&NSString::from_str(&match result {
                Ok(shortcut) => format!("{}\n点击保存，或继续按另一组。", shortcut.label()),
                Err(error) => error.into(),
            }));
        }
        true
    }
}

pub(crate) fn record(mtm: MainThreadMarker, current: Shortcut) -> Option<Shortcut> {
    let alert = NSAlert::new(mtm);
    alert.setMessageText(ns_string!("自定义开始 / 停止快捷键"));
    alert.setInformativeText(&NSString::from_str(&format!(
        "当前：{}\n请按住 Control、Option 或 Command，再按一个键；可加 Shift。\n按 Esc 取消。设置期间不会开始听写。", current.label()
    )));
    let recorder = Recorder::new(mtm, alert.addButtonWithTitle(ns_string!("保存")));
    alert.addButtonWithTitle(ns_string!("取消"));
    alert.setAccessoryView(Some(&recorder));
    // AppKit attaches the accessory during layout; establish focus afterwards.
    alert.layout();
    let window = alert.window();
    window.setInitialFirstResponder(Some(&recorder));
    window.makeFirstResponder(Some(&recorder));
    if alert.runModal() == NSAlertFirstButtonReturn {
        recorder.ivars().candidate.get()
    } else {
        None
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn regression(mtm: MainThreadMarker) {
    use objc2_foundation::NSPoint;
    let _app = NSApplication::sharedApplication(mtm);
    let alert = NSAlert::new(mtm);
    let save = alert.addButtonWithTitle(ns_string!("保存"));
    let recorder = Recorder::new(mtm, save.clone());
    alert.setAccessoryView(Some(&recorder));
    // AppKit attaches the accessory during layout; establish focus afterwards.
    alert.layout();
    let window = alert.window();
    assert!(window.makeFirstResponder(Some(&recorder)));
    let event = |code, flags, repeat| {
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown, NSPoint::new(0., 0.), flags, 0., window.windowNumber(), None,
            ns_string!("k"), ns_string!("k"), repeat, code,
        ).expect("synthetic local key event")
    };
    assert!(!save.isEnabled());
    let flags = NSEventModifierFlags::Control | NSEventModifierFlags::Option;
    assert!(recorder.capture(&event(40, flags, false)));
    assert_eq!(
        recorder.ivars().candidate.get(),
        Some(Shortcut::new(40, CONTROL | OPTION).unwrap())
    );
    assert!(save.isEnabled());
    // Bare and Shift-only input invalidate the prior candidate and disable Save.
    for flags in [NSEventModifierFlags::empty(), NSEventModifierFlags::Shift] {
        assert!(recorder.capture(&event(40, flags, false)));
        assert!(recorder.ivars().candidate.get().is_none());
        assert!(!save.isEnabled());
    }
    for code in [36, 48, 53, 76] {
        assert!(!recorder.capture(&event(code, NSEventModifierFlags::empty(), false)));
    }
    // Command events take the native key-equivalent path while focused.
    assert!(recorder.performKeyEquivalent(&event(40, NSEventModifierFlags::Command, false)));
    assert_eq!(
        recorder.ivars().candidate.get(),
        Some(Shortcut::new(40, COMMAND).unwrap())
    );
    assert!(recorder.capture(&event(0, flags, true)));
    assert_eq!(
        recorder.ivars().candidate.get(),
        Some(Shortcut::new(40, COMMAND).unwrap())
    );
    assert!(window.makeFirstResponder(Some(&*save)));
    assert!(!recorder.performKeyEquivalent(&event(40, NSEventModifierFlags::Command, false)));
    println!(
        "shortcut_recorder: native focus, Command routing, validation, repeat and dialog navigation passed; no window shown or key posted"
    );
}
