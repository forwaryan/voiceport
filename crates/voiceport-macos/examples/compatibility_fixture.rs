//! Dedicated synthetic editor: accepts typing/Paste but exposes no AX text API.
//! Never records, sends a message or runs the text as a command. Exits in 60s.
#[cfg(target_os = "macos")]
mod fixture {
    use block2::RcBlock;
    use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, sel};
    use objc2_app_kit::*;
    use objc2_foundation::{
        MainThreadMarker, NSPoint, NSRect, NSSize, NSString, NSTimer, ns_string,
    };
    use std::{cell::RefCell, path::PathBuf, ptr::NonNull, time::Instant};

    struct State {
        text: RefCell<String>,
        label: Retained<NSTextField>,
        receipt: PathBuf,
        mode: &'static str,
        editable: Option<bool>,
    }
    define_class!(
        #[unsafe(super(NSView))]
        #[thread_kind = MainThreadOnly]
        #[ivars = State]
        struct Editor;
        impl Editor {
            #[unsafe(method(acceptsFirstResponder))]
            fn accepts_first_responder(&self) -> bool { true }
            #[unsafe(method_id(accessibilityAttributeValue:))]
            fn accessibility_attribute_value(&self, name: &NSString) -> Option<Retained<objc2::runtime::AnyObject>> {
                match (name == ns_string!("AXEditable"), self.ivars().editable) {
                    (true, Some(editable)) => Some(unsafe {
                        msg_send![objc2::class!(NSNumber), numberWithBool: editable]
                    }),
                    _ => unsafe { msg_send![super(self), accessibilityAttributeValue: name] },
                }
            }
            #[unsafe(method(paste:))]
            fn paste(&self, _sender: Option<&objc2::runtime::AnyObject>) {
                if let Some(text) = unsafe {
                    NSPasteboard::generalPasteboard().stringForType(NSPasteboardTypeString)
                } {
                    self.insert(&text.to_string());
                }
            }
            #[unsafe(method(keyDown:))]
            fn key_down(&self, event: &NSEvent) {
                if let Some(text) = event.characters() {
                    let text = text.to_string();
                    if !text.chars().any(char::is_control) { self.insert(&text); }
                }
            }
        }
    );
    impl Editor {
        fn insert(&self, text: &str) {
            let mut value = self.ivars().text.borrow_mut();
            if value.contains('|') {
                *value = value.replacen('|', text, 1);
            } else {
                value.push_str(text);
            }
            self.ivars()
                .label
                .setStringValue(&NSString::from_str(&value));
            let receipt = serde_json::json!({"pid": std::process::id(), "text": &*value, "mode": self.ivars().mode});
            std::fs::write(&self.ivars().receipt, receipt.to_string()).unwrap();
        }
    }

    pub fn run() {
        let args: Vec<_> = std::env::args().skip(1).collect();
        assert!(
            matches!(args.len(), 2 | 4),
            "--receipt ABSOLUTE_NEW_PATH [--mode custom|editable-cell|readonly-cell|button|secure]"
        );
        assert_eq!(args[0], "--receipt");
        if args.len() == 4 {
            assert_eq!(args[2], "--mode");
        }
        let (mode, role, editable) = match args.get(3).map(String::as_str).unwrap_or("custom") {
            "custom" => ("custom", ns_string!("AXGroup"), None),
            "editable-cell" => ("editable-cell", ns_string!("AXCell"), Some(true)),
            "readonly-cell" => ("readonly-cell", ns_string!("AXCell"), Some(false)),
            "button" => ("button", ns_string!("AXButton"), Some(true)),
            "secure" => ("secure", ns_string!("AXSecureTextField"), Some(true)),
            _ => panic!("unknown fixture mode"),
        };
        let receipt = PathBuf::from(&args[1]);
        assert!(receipt.is_absolute() && !receipt.exists());
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&receipt)
            .unwrap();
        let mtm = MainThreadMarker::new().unwrap();
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0., 0.), NSSize::new(640., 240.)),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe {
            window.setReleasedWhenClosed(false);
        }
        window.setTitle(ns_string!("VoicePort · 通用输入独立验证（无录音）"));
        let label = NSTextField::wrappingLabelWithString(ns_string!("左|右"), mtm);
        label.setFrame(NSRect::new(NSPoint::new(24., 60.), NSSize::new(590., 100.)));
        let allocated = Editor::alloc(mtm).set_ivars(State {
            text: RefCell::new("左|右".into()),
            label: label.clone(),
            receipt,
            mode,
            editable,
        });
        let editor: Retained<Editor> = unsafe {
            msg_send![super(allocated), initWithFrame: NSRect::new(NSPoint::new(0., 0.), NSSize::new(640., 240.))]
        };
        editor.setAccessibilityElement(true);
        editor.setAccessibilityRole(Some(role));
        editor.setAccessibilityLabel(Some(ns_string!("VoicePort compatibility fixture")));
        editor.addSubview(&label);
        window.setContentView(Some(&editor));
        let menu = NSMenu::new(mtm);
        let edit_item = NSMenuItem::new(mtm);
        let edit_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), ns_string!("Edit"));
        let paste = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(mtm),
                ns_string!("Paste"),
                Some(sel!(paste:)),
                ns_string!("v"),
            )
        };
        edit_menu.addItem(&paste);
        edit_item.setSubmenu(Some(&edit_menu));
        menu.addItem(&edit_item);
        app.setMainMenu(Some(&menu));
        window.center();
        window.makeKeyAndOrderFront(None);
        window.makeFirstResponder(Some(&editor));
        #[allow(deprecated)]
        app.activateIgnoringOtherApps(true);
        let baseline =
            serde_json::json!({"pid": std::process::id(), "text": "左|右", "mode": mode});
        std::fs::write(&editor.ivars().receipt, baseline.to_string()).unwrap();
        let started = Instant::now();
        let timer_block = RcBlock::new(move |_timer: NonNull<NSTimer>| {
            if started.elapsed().as_secs() >= 60 {
                std::process::exit(0);
            }
        });
        let _timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(1., true, &timer_block)
        };
        app.run();
    }
}
#[cfg(target_os = "macos")]
fn main() {
    fixture::run();
}
#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("compatibility_fixture requires macOS");
}
