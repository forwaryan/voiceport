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
    cell::{Cell, RefCell},
    ffi::CStr,
    io::{self, Read},
    os::unix::{fs::MetadataExt, net::UnixStream},
    panic::{AssertUnwindSafe, catch_unwind},
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
const STAGES: [(Stage, &str, bool, usize); 9] = [
    (Stage::Preparing, "正在准备听写", false, 0),
    (
        Stage::Capture(CaptureStage::Opening),
        "正在打开麦克风",
        false,
        0,
    ),
    (
        Stage::Capture(CaptureStage::Recording),
        "正在录音",
        false,
        3,
    ),
    (Stage::Capture(CaptureStage::Recording), "正在录音", true, 0),
    (
        Stage::Capture(CaptureStage::Recovering),
        "正在恢复麦克风",
        false,
        0,
    ),
    (Stage::Capture(CaptureStage::Resumed), "正在录音", false, 3),
    (
        Stage::Capture(CaptureStage::Finalizing),
        "正在停止麦克风",
        false,
        0,
    ),
    (Stage::Transcribing, "正在转写", false, 0),
    (Stage::Delivering, "正在输入文字", false, 0),
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
        let mut vmm = [0; size_of::<libc::c_int>()];
        if read_sysctl(c"kern.hv_vmm_present", &mut vmm) != Some(vmm.len())
            || libc::c_int::from_ne_bytes(vmm) != 1
        {
            return Err("macOS virtual guest was not detected");
        }
        let mut boot = [0; 128];
        let uuid = read_sysctl(c"kern.bootsessionuuid", &mut boot)
            .and_then(|len| CStr::from_bytes_with_nul(&boot[..len]).ok());
        if boot_id.is_empty() || uuid.map(CStr::to_bytes) != Some(boot_id.as_bytes()) {
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
        // CGSession.h uses CFString keys. kCGSessionOnConsoleKey expands to
        // kCGSSessionOnConsoleKey. CGSessionCopyCurrentDictionary returns an owned value.
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
        let unlocked = dictionary
            .find(CFString::new("CGSSessionScreenIsLocked"))
            .is_none_or(|value| {
                value
                    .downcast::<CFBoolean>()
                    .is_some_and(|flag| !bool::from(flag))
            });
        session_uid == Some(i64::from(self.uid))
            && is_true("kCGSSessionOnConsoleKey")
            && is_true("kCGSessionLoginDoneKey")
            && unlocked
    }
}

fn read_sysctl(name: &CStr, buffer: &mut [u8]) -> Option<usize> {
    let mut len = buffer.len();
    // The two read-only queries need no child processes outside the runner's group.
    let status = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr().cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (status == 0 && len <= buffer.len()).then_some(len)
}

fn refuse(reason: &str) -> ! {
    eprintln!("status-overlay-focus: refused before AppKit ({reason})");
    std::process::exit(UNAVAILABLE);
}

fn watch_launcher(path: &str) -> io::Result<()> {
    // The app may belong to LaunchServices rather than the wrapper's process
    // group. Give it a hard deadline and exit on loss of this run's socket.
    unsafe {
        libc::signal(libc::SIGALRM, libc::SIG_DFL);
        libc::alarm(20);
    }
    let mut stream = UnixStream::connect(path)?;
    let mut byte = [0];
    stream.read_exact(&mut byte)?;
    if byte != [1] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid launch acknowledgement",
        ));
    }
    std::thread::Builder::new()
        .spawn(move || {
            let _ = stream.read_exact(&mut byte);
            // Exit only this process, even if its AppKit main thread is blocked.
            unsafe { libc::_exit(1) };
        })
        .map(drop)
}

/// The gate runs before NSApplication is created. Only the dedicated guest may activate.
pub(crate) fn run(mtm: MainThreadMarker, boot_id: &str, session_id: &str, launch: &[String]) -> ! {
    let attest_only = match launch {
        [] => false,
        [mode] if mode == "--attest-aqua" => true,
        [mode, path] if mode == "--launch-socket" => {
            if watch_launcher(path).is_err() {
                refuse("the private launch supervisor was unavailable");
            }
            false
        }
        _ => refuse("invalid fixture launch mode"),
    };
    let console = match ConsoleSession::authorize(boot_id, session_id) {
        Ok(console) => console,
        Err(reason) => refuse(reason),
    };
    if attest_only {
        let desktop = NSWorkspace::sharedWorkspace()
            .frontmostApplication()
            .is_some_and(|front| {
                front.isActive()
                    && front.activationPolicy() == NSApplicationActivationPolicy::Regular
                    && front.bundleIdentifier().is_some_and(|id| {
                        !matches!(
                            id.to_string().as_str(),
                            "" | "com.apple.loginwindow"
                                | "com.apple.SecurityAgent"
                                | "com.apple.ScreenSaver.Engine"
                                | "com.apple.screensaver"
                        )
                    })
            });
        if !desktop {
            refuse("the launcher has no active regular Aqua desktop application");
        }
        println!("status-overlay-focus: launcher Aqua session and fixture bundle verified");
        std::process::exit(0);
    }
    println!(
        "status-overlay-focus: isolated guest, active Aqua session and fixture bundle verified"
    );
    let app = NSApplication::sharedApplication(mtm);
    let initial = app.activationPolicy();
    let regular = NSApplicationActivationPolicy::Regular;
    let request = if !launch.is_empty() && initial == regular {
        "already-regular"
    } else if app.setActivationPolicy(regular) {
        "accepted"
    } else {
        "rejected"
    };
    let current = app.activationPolicy();
    println!(
        "status-overlay-focus: activation_policy initial={} current={} request={request}",
        initial.0, current.0
    );
    if request == "rejected" || current != regular {
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
        let safe = screen.visibleFrame();
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
            let mut inspection = Inspection::new(mtm, self.target.frame());
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
            inspection.overlay.close();
        }
        self.target.close();
    }
}

struct Inspection {
    mtm: MainThreadMarker,
    overlay: Overlay,
    target_frame: NSRect,
    pointer: NSPoint,
    next_step: usize,
}

impl Inspection {
    fn new(mtm: MainThreadMarker, target_frame: NSRect) -> Self {
        Self {
            mtm,
            overlay: Overlay::default(),
            target_frame,
            pointer: NSEvent::mouseLocation(),
            next_step: 0,
        }
    }

    fn update(&mut self, generation: u64, stage: Option<Stage>, reduce_motion: bool) -> bool {
        let sampled = Cell::new(false);
        self.overlay.update_with_motion(
            self.mtm,
            Snapshot {
                generation,
                stage,
                origin_pid: Some(std::process::id() as i32),
            },
            true,
            || {
                sampled.set(true);
                Some(3)
            },
            || reduce_motion,
        );
        sampled.get()
    }

    fn show_next(&mut self) {
        if let Some(&(stage, _, reduce_motion, bars)) = STAGES.get(self.next_step) {
            assert_eq!(self.update(1, Some(stage), reduce_motion), bars > 0);
        } else {
            let recording = Some(Stage::Capture(CaptureStage::Recording));
            match self.next_step - STAGES.len() {
                0 => {
                    assert!(!self.update(2, None, false)); // Cancel invalidates generation 1.
                    assert!(!self.update(1, recording, false));
                }
                1 => {
                    assert!(self.update(3, recording, false));
                    assert!(self.overlay.panel.as_ref().unwrap().window.isVisible());
                    self.overlay.close();
                    assert!(!self.update(4, recording, false));
                }
                _ => unreachable!(),
            }
        }
        self.next_step += 1;
    }

    fn check_previous_and_advance(&mut self) -> bool {
        let panel = self.overlay.panel.as_ref().unwrap();
        let Some(&(stage, text, reduce_motion, bars)) = STAGES.get(self.next_step - 1) else {
            assert!(!panel.window.isVisible());
            if self.next_step == STAGES.len() + 1 {
                self.show_next();
                return false;
            }
            println!(
                "status-overlay-focus: cancellation and quit preserved key window, AX identity, responder and caret"
            );
            return true;
        };
        assert!(panel.window.isVisible());
        assert!(
            panel
                .window
                .styleMask()
                .contains(NSWindowStyleMask::NonactivatingPanel)
        );
        assert!(panel.window.ignoresMouseEvents());
        assert!(!panel.window.canBecomeKeyWindow() && !panel.window.canBecomeMainWindow());
        assert!(!panel.window.isKeyWindow() && !panel.window.isMainWindow());
        assert_eq!(overlap(panel.window.frame(), self.target_frame), 0.);
        assert_eq!(panel.label.stringValue().to_string(), text);
        assert_eq!(panel.label.accessibilityLabel().unwrap().to_string(), text);
        assert!(panel.label.fittingSize().width <= panel.label.frame().size.width);
        assert_eq!(
            panel.bars.iter().filter(|bar| !bar.isHidden()).count(),
            bars
        );
        println!(
            "status-overlay-focus: {stage:?} reduce_motion={reduce_motion} preserved native focus"
        );
        self.show_next();
        false
    }
}
