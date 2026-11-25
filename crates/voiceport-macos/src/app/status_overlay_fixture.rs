//! Native fixture for the production status panel, plus pure synthetic geometry.
use super::*;
use block2::RcBlock;
use core_foundation::{
    base::{CFType, TCFType},
    boolean::CFBoolean,
    dictionary::{CFDictionary, CFDictionaryRef},
    number::CFNumber,
    string::CFString,
};
use objc2::runtime::AnyObject;
use objc2_app_kit::NSApplicationActivationPolicy;
use objc2_foundation::{NSBundle, NSObject, NSRange, NSTimer};
use std::{
    cell::RefCell,
    os::unix::fs::MetadataExt,
    panic::{AssertUnwindSafe, catch_unwind},
    process::Command,
    ptr::NonNull,
    time::{Duration, Instant},
};

#[test]
fn status_overlay_prefers_target_screen_and_leaves_the_input_clear() {
    let screens = [
        Screen {
            frame: rect(0., 0., 900., 600.),
            visible: rect(0., 20., 900., 552.),
        },
        Screen {
            frame: rect(-800., 200., 800., 550.),
            visible: rect(-800., 200., 800., 522.),
        },
    ];
    let field = rect(-620., 600., 500., 120.);
    let frame = place(
        &screens,
        Bounds {
            field: Some(field),
            window: None,
        },
        NSPoint::new(400., 400.),
    )
    .unwrap();
    assert_eq!(overlap(frame, screens[1].visible), WIDTH * HEIGHT);
    assert_eq!(overlap(frame, field), 0.);
    assert_eq!(frame.origin.y, screens[1].visible.origin.y + MARGIN);
    assert_eq!(
        place(
            &screens,
            Bounds {
                field: Some(screens[1].visible),
                window: None,
            },
            NSPoint::ZERO
        ),
        None
    );
    let window = rect(-720., 300., 500., 300.);
    let by_window = place(
        &screens,
        Bounds {
            field: None,
            window: Some(window),
        },
        NSPoint::ZERO,
    )
    .unwrap();
    assert!(by_window.origin.x < 0.);
    let by_pointer = place(&screens, Bounds::default(), NSPoint::new(-500., 440.)).unwrap();
    assert!(by_pointer.origin.x < 0.);
    assert!(place(&[], Bounds::default(), NSPoint::ZERO).is_none());
}

const BUNDLE_ID: &str = "org.voiceport.status-overlay-focus-fixture";
const UNAVAILABLE: i32 = 78;
const BASELINE_TIMEOUT: Duration = Duration::from_secs(5);
const SELECTION: NSRange = NSRange::new(1, 0);
const STAGES: [(Stage, &str, Option<u8>, bool); 5] = [
    (Stage::Preparing, "正在准备听写", None, false),
    (Stage::Recording, "正在录音", Some(3), false),
    (Stage::Recording, "正在录音", Some(3), true),
    (Stage::Transcribing, "正在转写", None, true),
    (Stage::Delivering, "正在输入文字", None, true),
];

#[link(name = "Security", kind = "framework")]
unsafe extern "C" {
    fn SessionGetInfo(session: u32, session_id: *mut u32, attributes: *mut u32) -> i32;
}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGSessionCopyCurrentDictionary() -> CFDictionaryRef;
}

struct ConsoleSession {
    uid: u32,
    session_id: u32,
}

impl ConsoleSession {
    fn authorize(boot_id: &str, session_id: &str) -> Result<Self, &'static str> {
        if sysctl("kern.hv_vmm_present").as_deref() != Some("1") {
            return Err("macOS virtual guest was not detected");
        }
        if boot_id.is_empty() || sysctl("kern.bootsessionuuid").as_deref() != Some(boot_id) {
            return Err("guest boot UUID does not match the authorization");
        }
        let session_id = session_id.parse().map_err(|_| "invalid Aqua session ID")?;
        let console = Self {
            uid: unsafe { libc::geteuid() },
            session_id,
        };
        if !console.is_current() {
            return Err("the caller is outside the authorized active Aqua console session");
        }
        if NSBundle::mainBundle()
            .bundleIdentifier()
            .is_none_or(|id| id.to_string() != BUNDLE_ID)
        {
            return Err("the executable is outside the dedicated synthetic fixture bundle");
        }
        Ok(console)
    }

    fn is_current(&self) -> bool {
        // AuthSession.h: callerSecuritySession, sessionHasGraphicAccess and sessionIsRemote.
        const CALLER_SESSION: u32 = u32::MAX;
        const GRAPHIC_ACCESS: u32 = 0x0010;
        const REMOTE: u32 = 0x1000;
        let mut id = 0;
        let mut attributes = 0;
        let status = unsafe { SessionGetInfo(CALLER_SESSION, &mut id, &mut attributes) };
        if self.uid == 0
            || unsafe { libc::getuid() } != self.uid
            || status != 0
            || id != self.session_id
            || attributes & (GRAPHIC_ACCESS | REMOTE) != GRAPHIC_ACCESS
            || std::fs::metadata("/dev/console").map(|m| m.uid()).ok() != Some(self.uid)
        {
            return false;
        }
        let dictionary = unsafe { CGSessionCopyCurrentDictionary() };
        if dictionary.is_null() {
            return false;
        }
        // CGSession.h declares these keys as CFString and the return value is owned.
        let dictionary =
            unsafe { CFDictionary::<CFString, CFType>::wrap_under_create_rule(dictionary) };
        let session_uid = dictionary
            .find(CFString::new("kCGSSessionUserIDKey"))
            .and_then(|value| value.downcast::<CFNumber>())
            .and_then(|number| number.to_i64());
        let is_true = |key| {
            dictionary
                .find(CFString::new(key))
                .and_then(|value| value.downcast::<CFBoolean>())
                .is_some_and(bool::from)
        };
        session_uid == Some(i64::from(self.uid))
            && is_true("kCGSessionOnConsoleKey")
            && is_true("kCGSessionLoginDoneKey")
    }
}

fn sysctl(name: &str) -> Option<String> {
    let result = Command::new("/usr/sbin/sysctl")
        .args(["-n", name])
        .output()
        .ok()?;
    if !result.status.success() {
        return None;
    }
    Some(std::str::from_utf8(&result.stdout).ok()?.trim().to_owned())
}

/// The gate runs before NSApplication is created. Only the dedicated guest may activate.
pub(crate) fn run(mtm: MainThreadMarker, boot_id: &str, session_id: &str) -> ! {
    let console = match ConsoleSession::authorize(boot_id, session_id) {
        Ok(console) => console,
        Err(reason) => {
            eprintln!("status-overlay-focus: refused before AppKit ({reason})");
            std::process::exit(UNAVAILABLE);
        }
    };
    println!(
        "status-overlay-focus: isolated guest, active Aqua session and fixture bundle verified"
    );
    let app = NSApplication::sharedApplication(mtm);
    if !app.setActivationPolicy(NSApplicationActivationPolicy::Regular) {
        eprintln!("status-overlay-focus: AppKit rejected the regular activation policy");
        std::process::exit(UNAVAILABLE);
    }

    let target_app = app.clone();
    let fixture = RefCell::new(None::<Fixture>);
    let tick = RcBlock::new(move |timer: NonNull<NSTimer>| {
        // Keep Rust assertion failures inside the Rust callback boundary.
        let result = catch_unwind(AssertUnwindSafe(|| {
            if !console.is_current() {
                eprintln!("status-overlay-focus: authorized Aqua session is no longer current");
                return Some(UNAVAILABLE);
            }
            let mut fixture = fixture.borrow_mut();
            if let Some(fixture) = fixture.as_mut() {
                return fixture.advance(mtm, &target_app);
            }
            match Fixture::new(mtm, &target_app) {
                Some(target) => {
                    *fixture = Some(target);
                    None
                }
                None => Some(UNAVAILABLE),
            }
        }));
        if let Ok(None) = result {
            return;
        }
        // The timer supplies a borrowed pointer valid during this call.
        unsafe { timer.as_ref() }.invalidate();
        if let Some(fixture) = fixture.borrow_mut().take() {
            fixture.close();
        }
        std::process::exit(result.unwrap_or(Some(1)).expect("terminal timer result"));
    });
    // NSTimer retains the block and runs it on this main thread's default run loop.
    let _timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.05, true, &tick) };
    app.run();
    eprintln!("status-overlay-focus: AppKit exited before the focus checks completed");
    std::process::exit(UNAVAILABLE);
}

struct Fixture {
    screen: Screen,
    target: Retained<NSWindow>,
    editor: Retained<NSTextView>,
    starting: Instant,
    requested_key: bool,
    inspection: Option<Inspection>,
}

#[derive(Debug)]
struct Focus {
    active: bool,
    key_window_present: bool,
    key_window_is_target: bool,
    target_is_key: bool,
    ax_is_editor: bool,
    responder_is_editor: bool,
    caret_is_expected: bool,
}

impl Focus {
    fn is_positive(&self) -> bool {
        self.active
            && self.key_window_present
            && self.key_window_is_target
            && self.target_is_key
            && self.ax_is_editor
            && self.responder_is_editor
            && self.caret_is_expected
    }
}

impl Fixture {
    fn new(mtm: MainThreadMarker, app: &NSApplication) -> Option<Self> {
        assert!(
            app.isRunning(),
            "the normal AppKit event loop must be running"
        );
        let Some(screen) = NSScreen::screens(mtm).firstObject() else {
            eprintln!("status-overlay-focus: no NSScreen / WindowServer fixture surface");
            return None;
        };
        let screen = Screen {
            frame: screen.frame(),
            visible: screen.visibleFrame(),
        };
        let safe = screen.visible;
        let target_frame = rect(safe.origin.x + 36., safe.origin.y + 70., 360., 160.);
        let target = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                target_frame,
                NSWindowStyleMask::Titled,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { target.setReleasedWhenClosed(false) };
        let editor = NSTextView::initWithFrame(NSTextView::alloc(mtm), rect(0., 0., 360., 160.));
        editor.setString(ns_string!("左|右"));
        target.setTitle(ns_string!("VoicePort isolated overlay focus fixture"));
        target.setContentView(Some(&editor));
        assert!(target.makeFirstResponder(Some(&editor)));
        editor.setSelectedRange(SELECTION);
        let eligible = target.canBecomeKeyWindow();
        println!("status-overlay-focus: app_event_loop=true target_can_become_key={eligible}");
        if !eligible {
            target.close();
            return None;
        }
        target.orderFront(None);
        // This is the fixture's only activation request. The guest gate precedes it,
        // and AppKit's real async result is verified before showing the panel.
        #[allow(deprecated)] // The test also supports macOS 13.
        app.activateIgnoringOtherApps(true);
        Some(Self {
            screen,
            target,
            editor,
            starting: Instant::now(),
            requested_key: false,
            inspection: None,
        })
    }

    fn focus(&self, app: &NSApplication) -> Focus {
        let key_window = app.keyWindow();
        let app_obj: &NSObject = app;
        let editor_obj: &AnyObject = &self.editor;
        let editor_responder: &NSResponder = &self.editor;
        Focus {
            active: app.isActive(),
            key_window_present: key_window.is_some(),
            key_window_is_target: key_window
                .as_deref()
                .is_some_and(|key| std::ptr::eq(key, &*self.target)),
            target_is_key: self.target.isKeyWindow(),
            ax_is_editor: NSObjectNSAccessibility::accessibilityFocusedUIElement(app_obj)
                .as_deref()
                .is_some_and(|focus| std::ptr::eq(focus, editor_obj)),
            responder_is_editor: self
                .target
                .firstResponder()
                .as_deref()
                .is_some_and(|current| std::ptr::eq(current, editor_responder)),
            caret_is_expected: NSTextInputClient::selectedRange(&*self.editor) == SELECTION,
        }
    }

    fn advance(&mut self, mtm: MainThreadMarker, app: &NSApplication) -> Option<i32> {
        if !self.requested_key && app.isActive() {
            self.target.makeKeyWindow();
            self.requested_key = true;
            println!("status-overlay-focus: fixture requested key window after becoming active");
        }
        let focus = self.focus(app);
        if let Some(inspection) = self.inspection.as_mut() {
            assert!(
                focus.is_positive(),
                "focus changed after overlay step {}: {focus:?}",
                inspection.next_step
            );
            assert_eq!(self.editor.string().to_string(), "左|右");
            assert_eq!(NSEvent::mouseLocation(), inspection.pointer);
            if inspection.check_previous_and_advance() {
                println!(
                    "status-overlay-focus: passed positive native key/AX baseline and all overlay transitions"
                );
                return Some(0);
            }
        } else if focus.is_positive() {
            println!("status-overlay-focus: positive native baseline {focus:?}");
            let mut inspection = Inspection::new(mtm, &self.screen, self.target.frame());
            inspection.show_next();
            self.inspection = Some(inspection);
        } else if self.starting.elapsed() >= BASELINE_TIMEOUT {
            eprintln!(
                "status-overlay-focus: native baseline unavailable after activation request: {focus:?}"
            );
            return Some(UNAVAILABLE);
        }
        None
    }

    fn close(mut self) {
        if let Some(inspection) = &mut self.inspection {
            inspection.panel.hide();
        }
        self.target.close();
    }
}

struct Inspection {
    panel: Panel,
    frame: NSRect,
    pointer: NSPoint,
    next_step: usize,
}

impl Inspection {
    fn new(mtm: MainThreadMarker, screen: &Screen, target_frame: NSRect) -> Self {
        let panel = Panel::new(mtm);
        let pointer = NSEvent::mouseLocation();
        let frame = place(
            std::slice::from_ref(screen),
            Bounds {
                field: Some(target_frame),
                window: Some(target_frame),
            },
            pointer,
        )
        .expect("the synthetic window needs room for an unobstructed panel");
        assert!(
            panel
                .window
                .styleMask()
                .contains(NSWindowStyleMask::NonactivatingPanel)
        );
        assert!(panel.window.ignoresMouseEvents());
        assert!(!panel.window.canBecomeKeyWindow());
        assert!(!panel.window.canBecomeMainWindow());
        assert_eq!(overlap(frame, target_frame), 0.);
        Self {
            panel,
            frame,
            pointer,
            next_step: 0,
        }
    }

    fn show_next(&mut self) {
        if let Some(&(stage, _, level, reduce_motion)) = STAGES.get(self.next_step) {
            self.panel.show(self.frame, stage, level, reduce_motion);
        } else {
            self.panel.hide();
        }
        self.next_step += 1;
    }

    fn check_previous_and_advance(&mut self) -> bool {
        let Some(&(stage, text, level, reduce_motion)) = STAGES.get(self.next_step - 1) else {
            assert!(!self.panel.window.isVisible());
            println!(
                "status-overlay-focus: hide preserved key window, AX identity, responder and caret"
            );
            return true;
        };
        assert!(self.panel.window.isVisible());
        assert!(!self.panel.window.isKeyWindow() && !self.panel.window.isMainWindow());
        assert_eq!(self.panel.label.stringValue().to_string(), text);
        assert_eq!(
            self.panel.label.accessibilityLabel().unwrap().to_string(),
            text
        );
        assert!(self.panel.label.fittingSize().width <= self.panel.label.frame().size.width);
        let visible_bars = self.panel.bars.iter().filter(|bar| !bar.isHidden()).count();
        assert_eq!(
            visible_bars,
            if reduce_motion {
                0
            } else {
                usize::from(level.unwrap_or(0))
            }
        );
        println!(
            "status-overlay-focus: {stage:?} reduce_motion={reduce_motion} preserved native focus"
        );
        self.show_next();
        false
    }
}
