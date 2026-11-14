//! Native boundary. Nothing here captures a target until `capture` is called.
use super::model::{self, Failure, Snapshot, Target, TerminalTarget, TextRange};
use block2::RcBlock;
use core_foundation::{
    base::{CFEqual, CFGetTypeID, CFRange, CFRelease, CFTypeID, CFTypeRef, TCFType},
    boolean::{CFBooleanGetTypeID, kCFBooleanTrue},
    dictionary::CFDictionary,
    string::{CFString, CFStringGetCharacters, CFStringGetLength, CFStringGetTypeID},
};
use objc2::{msg_send, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{
    NSEvent, NSEventMask, NSEventType, NSPasteboard, NSPasteboardTypeString, NSWorkspace,
};
use objc2_foundation::NSString;
use std::{
    ffi::c_void,
    ptr::{self, NonNull},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

// AXUIElement / AXValue are CF objects. These declarations deliberately keep
// ownership explicit; successful Copy/Create calls are balanced by OwnedCf.
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> bool;
    fn AXIsProcessTrustedWithOptions(options: CFTypeRef) -> bool;
    fn AXUIElementCreateApplication(pid: libc::pid_t) -> CFTypeRef;
    fn AXUIElementGetTypeID() -> CFTypeID;
    fn AXUIElementCopyAttributeValue(
        element: CFTypeRef,
        key: CFTypeRef,
        value: *mut CFTypeRef,
    ) -> i32;
    fn AXUIElementIsAttributeSettable(element: CFTypeRef, key: CFTypeRef, result: *mut u8) -> i32;
    fn AXUIElementSetAttributeValue(element: CFTypeRef, key: CFTypeRef, value: CFTypeRef) -> i32;
    fn AXValueGetTypeID() -> CFTypeID;
    fn AXValueGetType(value: CFTypeRef) -> u32;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, result: *mut c_void) -> bool;
    fn AXValueCreate(kind: u32, value: *const c_void) -> CFTypeRef;
    fn CGEventGetIntegerValueField(event: *const c_void, field: u32) -> i64;
    fn CGEventSourceCreate(state: i32) -> CFTypeRef;
    fn CGEventCreateKeyboardEvent(source: CFTypeRef, key: u16, down: bool) -> CFTypeRef;
    fn CGEventKeyboardSetUnicodeString(event: CFTypeRef, length: usize, units: *const u16);
    fn CGEventSetFlags(event: CFTypeRef, flags: u64);
    fn CGEventSetIntegerValueField(event: CFTypeRef, field: u32, value: i64);
    fn CGEventPostToPid(pid: libc::pid_t, event: CFTypeRef);
}
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn IsSecureEventInputEnabled() -> bool;
}

const AX_CF_RANGE: u32 = 4;
const AX_UNSUPPORTED: i32 = -25205;
const AX_NO_VALUE: i32 = -25212;
const ORCA: &str = "com.stablyai.orca";
const CODEX: &str = "com.openai.codex";

struct OwnedCf(CFTypeRef);
impl OwnedCf {
    fn new(value: CFTypeRef) -> Option<Self> {
        if value.is_null() {
            None
        } else {
            Some(Self(value))
        }
    }
    fn string(&self, limit: usize) -> Option<String> {
        unsafe {
            if CFGetTypeID(self.0) != CFStringGetTypeID() {
                return None;
            }
            let length = CFStringGetLength(self.0.cast());
            if length < 0 || length as usize > limit {
                return None;
            }
            let mut units = vec![0u16; length as usize];
            CFStringGetCharacters(
                self.0.cast(),
                CFRange {
                    location: 0,
                    length,
                },
                units.as_mut_ptr(),
            );
            // AX may return malformed UTF-16. Never silently replace surrogates.
            String::from_utf16(&units).ok()
        }
    }
}
impl Drop for OwnedCf {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0);
        }
    }
}

pub(super) fn main_thread() -> bool {
    unsafe { libc::pthread_main_np() != 0 }
}
pub(super) fn trusted() -> bool {
    main_thread() && unsafe { AXIsProcessTrusted() }
}
pub(super) fn secure_input() -> bool {
    // Unknown security state off the main thread fails closed.
    !main_thread() || unsafe { IsSecureEventInputEnabled() }
}

pub(super) fn request_accessibility() {
    if !main_thread() {
        return;
    }
    let key = CFString::new("AXTrustedCheckOptionPrompt");
    let yes = core_foundation::boolean::CFBoolean::true_value();
    let options = CFDictionary::from_CFType_pairs(&[(key, yes)]);
    // Called only from the explicit permission button; capture never prompts.
    unsafe {
        AXIsProcessTrustedWithOptions(options.as_CFTypeRef());
    }
}

pub(super) fn copy(text: &str) -> bool {
    if !main_thread() {
        return false;
    }
    let board = NSPasteboard::generalPasteboard();
    board.clearContents();
    // Manual copy preserves newlines and control characters exactly.
    unsafe { board.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) }
}

fn frontmost() -> Result<(libc::pid_t, String), Failure> {
    let app = NSWorkspace::sharedWorkspace()
        .frontmostApplication()
        .ok_or(Failure::Focus)?;
    let pid = app.processIdentifier();
    if pid == unsafe { libc::getpid() } {
        return Err(Failure::Application);
    }
    if pid <= 0 {
        return Err(Failure::Focus);
    }
    Ok((
        pid,
        app.bundleIdentifier()
            .map(|s| s.to_string())
            .unwrap_or_default(),
    ))
}

fn attribute(element: CFTypeRef, name: &str) -> Result<OwnedCf, i32> {
    let key = CFString::new(name);
    let mut value = ptr::null();
    let result = unsafe { AXUIElementCopyAttributeValue(element, key.as_CFTypeRef(), &mut value) };
    // AX output is valid only on success, even if a transport error leaves a
    // non-null output. Never inspect or release unowned error output.
    if result != 0 {
        return Err(result);
    }
    OwnedCf::new(value).ok_or(AX_NO_VALUE)
}

fn settable(element: CFTypeRef, name: &str) -> Result<bool, Failure> {
    let key = CFString::new(name);
    let mut value = 0;
    let status = unsafe { AXUIElementIsAttributeSettable(element, key.as_CFTypeRef(), &mut value) };
    writable_result(status, value)
}

fn writable_result(status: i32, value: u8) -> Result<bool, Failure> {
    match status {
        0 if value <= 1 => Ok(value == 1),
        // Explicitly absent capability is known false, permitting the other
        // supported write path. Transport/permission failures remain unknown.
        AX_UNSUPPORTED | AX_NO_VALUE => Ok(false),
        _ => Err(Failure::Writable),
    }
}

fn enabled_result(status: i32, enabled: Option<bool>) -> Result<(), Failure> {
    match (status, enabled) {
        (0, Some(true)) | (AX_UNSUPPORTED, _) => Ok(()),
        // NSTextView omits optional AXEnabled. Its explicit selection/text
        // write capabilities are still required below. A known disabled value,
        // malformed boolean or AX transport/permission failure remains blocked.
        _ => Err(Failure::Disabled),
    }
}

fn set(element: CFTypeRef, name: &str, value: CFTypeRef) -> Result<(), Failure> {
    let key = CFString::new(name);
    if unsafe { AXUIElementSetAttributeValue(element, key.as_CFTypeRef(), value) } == 0 {
        Ok(())
    } else {
        Err(Failure::Write)
    }
}

fn focused(pid: libc::pid_t) -> Result<OwnedCf, Failure> {
    let application =
        OwnedCf::new(unsafe { AXUIElementCreateApplication(pid) }).ok_or(Failure::Focus)?;
    let element = attribute(application.0, "AXFocusedUIElement").map_err(Failure::FocusRead)?;
    if unsafe { CFGetTypeID(element.0) != AXUIElementGetTypeID() } {
        return Err(Failure::Focus);
    }
    Ok(element)
}

/// Explicit diagnostic: reads only target metadata, never field text or permissions.
pub(super) fn diagnose_input() -> serde_json::Value {
    let mut result = serde_json::json!({
        "mainThread": main_thread(), "accessibilityTrusted": trusted(),
        "secureInput": secure_input(),
    });
    if !main_thread() || !trusted() {
        return result;
    }
    let (pid, bundle) = match frontmost() {
        Ok(target) => target,
        Err(failure) => {
            result["failure"] = failure.message().into();
            return result;
        }
    };
    result["pid"] = pid.into();
    result["bundle"] = bundle.into();
    let Some(application) = OwnedCf::new(unsafe { AXUIElementCreateApplication(pid) }) else {
        return result;
    };
    let element = match attribute(application.0, "AXFocusedUIElement") {
        Ok(element) => element,
        Err(status) => {
            result["focusedElementAXError"] = status.into();
            return result;
        }
    };
    if unsafe { CFGetTypeID(element.0) != AXUIElementGetTypeID() } {
        result["failure"] = "Focused value is not an AX element".into();
        return result;
    }
    for name in ["AXRole", "AXSubrole", "AXDescription", "AXIdentifier"] {
        match attribute(element.0, name) {
            Ok(value) => result[name] = value.string(1_000).into(),
            Err(status) => result[format!("{name}Error")] = status.into(),
        }
    }
    match attribute(element.0, "AXEnabled") {
        Ok(value) => {
            result["AXEnabledIsBoolean"] =
                (unsafe { CFGetTypeID(value.0) == CFBooleanGetTypeID() }).into();
            result["AXEnabled"] = (unsafe { CFEqual(value.0, kCFBooleanTrue.cast()) != 0 }).into();
        }
        Err(status) => result["AXEnabledError"] = status.into(),
    }
    for name in ["AXSelectedTextRange", "AXSelectedText", "AXValue"] {
        result[format!("{name}Settable")] = match settable(element.0, name) {
            Ok(value) => value.into(),
            Err(failure) => failure.message().into(),
        };
    }
    result
}

/// Only the dedicated synthetic fixture may receive the fixed self-test string.
pub(super) fn self_test_fixture_focused() -> bool {
    let Ok((pid, bundle)) = frontmost() else {
        return false;
    };
    if bundle != "io.github.forwaryan.voiceport.input-fixture" {
        return false;
    }
    let Ok(element) = focused(pid) else {
        return false;
    };
    attribute(element.0, "AXIdentifier")
        .ok()
        .and_then(|v| v.string(100))
        .as_deref()
        == Some("voiceport.input-self-test")
        && attribute(element.0, "AXValue")
            .ok()
            .and_then(|v| v.string(100))
            .as_deref()
            == Some("左|右")
}

pub(super) enum Captured {
    Ordinary(Box<dyn Target>),
    Keyboard(Box<dyn Target>),
    Terminal(Box<dyn TerminalTarget>),
    Preparing(Box<dyn Preparation>),
}

pub(super) trait Preparation {
    fn poll(&mut self) -> Result<Option<Captured>, Failure>;
}

/// Electron can build its native accessibility tree asynchronously after an
/// assistive client first reads the application role. Bind the original window
/// and observe user activity BEFORE that request; never adopt a later focus.
struct FocusPreparation {
    pid: libc::pid_t,
    bundle: String,
    window: OwnedCf,
    monitor: Option<UserMonitor>,
    started: Instant,
    _main_only: std::marker::PhantomData<Rc<()>>,
}

fn application_window(pid: libc::pid_t) -> Result<OwnedCf, Failure> {
    let app = OwnedCf::new(unsafe { AXUIElementCreateApplication(pid) }).ok_or(Failure::Focus)?;
    let window = attribute(app.0, "AXFocusedWindow").map_err(Failure::FocusRead)?;
    if unsafe { CFGetTypeID(window.0) != AXUIElementGetTypeID() } {
        return Err(Failure::Focus);
    }
    Ok(window)
}

impl FocusPreparation {
    fn start(pid: libc::pid_t, bundle: String) -> Result<Self, Failure> {
        let monitor = UserMonitor::start()?;
        let window = application_window(pid)?;
        let preparation = Self {
            pid,
            bundle,
            window,
            monitor: Some(monitor),
            started: Instant::now(),
            _main_only: std::marker::PhantomData,
        };
        preparation.check()?;
        let app =
            OwnedCf::new(unsafe { AXUIElementCreateApplication(pid) }).ok_or(Failure::Focus)?;
        // A standard read, not AXManualAccessibility / AXEnhancedUserInterface
        // setters. Electron enables native APIs when an assistive client asks
        // for its application role. Do this once, only on explicit dictation.
        if attribute(app.0, "AXRole")
            .ok()
            .and_then(|value| value.string(100))
            .as_deref()
            != Some("AXApplication")
        {
            return Err(Failure::Role);
        }
        preparation.check()?;
        Ok(preparation)
    }

    fn check(&self) -> Result<(), Failure> {
        if !main_thread() {
            return Err(Failure::MainThread);
        }
        if !trusted() {
            return Err(Failure::Accessibility);
        }
        if secure_input() {
            return Err(Failure::SecureInput);
        }
        self.monitor.as_ref().ok_or(Failure::Monitor)?.check()?;
        let (pid, bundle) = frontmost()?;
        if pid != self.pid || bundle != self.bundle {
            return Err(Failure::FocusApplicationChanged);
        }
        let window = application_window(pid)?;
        if unsafe { CFEqual(window.0, self.window.0) } == 0 {
            return Err(Failure::FocusElementChanged);
        }
        Ok(())
    }
}

fn waiting_for_focus(failure: Failure, elapsed: Duration) -> bool {
    failure == Failure::FocusRead(AX_NO_VALUE) && elapsed < Duration::from_secs(2)
}

impl Preparation for FocusPreparation {
    fn poll(&mut self) -> Result<Option<Captured>, Failure> {
        self.check()?;
        let element = match focused(self.pid) {
            Ok(element) => element,
            Err(failure) if waiting_for_focus(failure, self.started.elapsed()) => {
                return Ok(None);
            }
            Err(failure) => return Err(failure),
        };
        self.check()?;
        captured_target(self.pid, self.bundle.clone(), element, self.monitor.take()).map(Some)
    }
}

pub(super) fn capture(
    expected: Option<&crate::delivery::model::ApplicationTarget>,
) -> Result<Captured, Failure> {
    if !main_thread() {
        return Err(Failure::MainThread);
    }
    if !trusted() {
        return Err(Failure::Accessibility);
    }
    if secure_input() {
        return Err(Failure::SecureInput);
    }
    let (pid, bundle) = frontmost()?;
    // Reject a changed shortcut target before any asynchronous preparation.
    if expected.is_some_and(|target| target.pid != pid || target.bundle != bundle) {
        return Err(Failure::FocusApplicationChanged);
    }
    let element = match focused(pid) {
        Ok(element) => element,
        Err(Failure::FocusRead(AX_NO_VALUE)) => {
            return FocusPreparation::start(pid, bundle)
                .map(|preparation| Captured::Preparing(Box::new(preparation)));
        }
        Err(failure) => return Err(failure),
    };
    captured_target(pid, bundle, element, None)
}

fn captured_target(
    pid: libc::pid_t,
    bundle: String,
    element: OwnedCf,
    monitor: Option<UserMonitor>,
) -> Result<Captured, Failure> {
    // An unreadable Orca description cannot safely distinguish an editor from
    // its hidden terminal transport, so never fall back to document writes.
    let terminal = if bundle == ORCA {
        attribute(element.0, "AXDescription")
            .ok()
            .and_then(|v| v.string(1_000))
            .ok_or(Failure::Role)?
            == "Terminal input"
    } else {
        false
    };
    let mut target = NativeTarget {
        pid,
        bundle,
        element,
        terminal,
        // All targets are monitored before the first snapshot. Reuse the
        // preparation monitor without restarting it or losing invalidation.
        monitor: Some(match monitor {
            Some(monitor) => monitor,
            None => UserMonitor::start()?,
        }),
        pending: None,
        _main_only: std::marker::PhantomData,
    };
    target.read()?;
    if terminal {
        target.current()?;
        Ok(Captured::Terminal(Box::new(target)))
    } else if target.bundle == CODEX {
        Ok(Captured::Keyboard(Box::new(KeyboardTarget {
            inner: target,
            pending: None,
        })))
    } else {
        Ok(Captured::Ordinary(Box::new(target)))
    }
}

struct KeyboardTarget {
    inner: NativeTarget,
    pending: Option<PendingWrite>,
}

impl Target for KeyboardTarget {
    fn read(&mut self) -> Result<Snapshot, Failure> {
        self.inner.read()
    }

    fn replace(
        &mut self,
        range: TextRange,
        text: &str,
        expected: &Snapshot,
    ) -> Result<(), Failure> {
        if self.pending.is_some() || range != expected.selection {
            return Err(Failure::Write);
        }
        if self.read()? != *expected {
            return Err(Failure::Ownership);
        }
        let bytes = model::byte_range(&expected.text, range).ok_or(Failure::Selection)?;
        let mut next = expected.clone();
        next.text.replace_range(bytes, text);
        next.selection = TextRange {
            location: range.location + text.encode_utf16().count(),
            length: 0,
        };
        let (_source, down, up) = unicode_events(text)?;
        // Recheck immediately before the paired key events. No AX text setter,
        // clipboard, Delete or Return is used for this known application.
        if self.read()? != *expected {
            return Err(Failure::Ownership);
        }
        unsafe {
            CGEventPostToPid(self.inner.pid, down.0);
            CGEventPostToPid(self.inner.pid, up.0);
        }
        self.pending = Some(PendingWrite {
            step: WriteStep::Text {
                before: expected.clone(),
            },
            next,
            deadline: Instant::now() + READBACK_TIMEOUT,
        });
        Ok(())
    }

    fn confirm(&mut self, expected: &Snapshot) -> Result<bool, Failure> {
        let Some(pending) = self.pending.take() else {
            return Err(Failure::Write);
        };
        if &pending.next != expected {
            return Err(Failure::Ownership);
        }
        let current = self.read()?;
        match pending.observe(&current, Instant::now())? {
            ConfirmationAction::Done => Ok(true),
            ConfirmationAction::Wait | ConfirmationAction::SetCaret => {
                // Normal typing owns caret movement too. Observe it; never
                // repair a rich editor with an AX selection write.
                self.pending = Some(pending);
                Ok(false)
            }
            ConfirmationAction::SubmitText => Err(Failure::Write),
        }
    }

    fn abandon(&mut self) {
        self.pending = None;
    }
}

struct NativeTarget {
    pid: libc::pid_t,
    bundle: String,
    element: OwnedCf,
    terminal: bool,
    monitor: Option<UserMonitor>,
    pending: Option<PendingWrite>,
    // AX/AppKit work and destruction stay on the main thread.
    _main_only: std::marker::PhantomData<Rc<()>>,
}

const READBACK_TIMEOUT: Duration = Duration::from_millis(800);

enum WriteStep {
    Selection {
        before: Snapshot,
        selected: Snapshot,
        text: String,
    },
    Text {
        before: Snapshot,
    },
    Caret {
        before: Snapshot,
    },
}

struct PendingWrite {
    step: WriteStep,
    next: Snapshot,
    deadline: Instant,
}

#[derive(Debug, PartialEq, Eq)]
enum ConfirmationAction {
    Done,
    Wait,
    SubmitText,
    SetCaret,
}

impl PendingWrite {
    fn observe(&self, current: &Snapshot, now: Instant) -> Result<ConfirmationAction, Failure> {
        if current == &self.next {
            return Ok(ConfirmationAction::Done);
        }
        let action = match &self.step {
            WriteStep::Selection { selected, .. } if current == selected => {
                ConfirmationAction::SubmitText
            }
            WriteStep::Selection { before, .. } if current == before => ConfirmationAction::Wait,
            WriteStep::Text { before } | WriteStep::Caret { before } if current == before => {
                ConfirmationAction::Wait
            }
            WriteStep::Text { before }
                if current.text == self.next.text && current.selection == before.selection =>
            {
                ConfirmationAction::SetCaret
            }
            _ => return Err(Failure::Ownership),
        };
        if now >= self.deadline {
            return Err(Failure::ReadbackTimeout);
        }
        Ok(action)
    }
}

impl NativeTarget {
    fn is_terminal(&self) -> bool {
        self.bundle == ORCA
            && attribute(self.element.0, "AXDescription")
                .ok()
                .and_then(|v| v.string(1_000))
                .as_deref()
                == Some("Terminal input")
    }

    fn validate(&self) -> Result<(), Failure> {
        if !main_thread() {
            return Err(Failure::MainThread);
        }
        if !trusted() {
            return Err(Failure::Accessibility);
        }
        if secure_input() {
            return Err(Failure::SecureInput);
        }
        if let Some(monitor) = &self.monitor {
            monitor.check()?;
        }
        let (pid, bundle) = frontmost()?;
        if pid != self.pid || bundle != self.bundle {
            return Err(Failure::FocusApplicationChanged);
        }
        let current = focused(pid)?;
        if unsafe { CFEqual(current.0, self.element.0) } == 0 {
            return Err(Failure::FocusElementChanged);
        }
        if self.bundle == ORCA {
            let description = attribute(self.element.0, "AXDescription")
                .ok()
                .and_then(|v| v.string(1_000))
                .ok_or(Failure::Role)?;
            if (description == "Terminal input") != self.terminal {
                return Err(Failure::Focus);
            }
        }
        let role = attribute(self.element.0, "AXRole")
            .ok()
            .and_then(|v| v.string(100))
            .ok_or(Failure::Role)?;
        if !matches!(role.as_str(), "AXTextField" | "AXTextArea" | "AXComboBox") {
            return Err(Failure::Role);
        }
        match attribute(self.element.0, "AXSubrole") {
            Ok(value) => {
                let subrole = value.string(1_000).ok_or(Failure::Sensitive)?;
                if subrole == "AXSecureTextField" {
                    return Err(Failure::Sensitive);
                }
            }
            // Chromium deliberately omits this attribute on ordinary fields.
            Err(AX_UNSUPPORTED | AX_NO_VALUE) => {}
            Err(_) => return Err(Failure::Sensitive),
        }
        let (status, enabled) = match attribute(self.element.0, "AXEnabled") {
            Ok(value) => (0, unsafe {
                (CFGetTypeID(value.0) == CFBooleanGetTypeID())
                    .then(|| CFEqual(value.0, kCFBooleanTrue.cast()) != 0)
            }),
            Err(status) => (status, None),
        };
        enabled_result(status, enabled)?;
        if !settable(self.element.0, "AXSelectedTextRange")? {
            return Err(Failure::Writable);
        }
        if !settable(self.element.0, "AXSelectedText")? && !settable(self.element.0, "AXValue")? {
            return Err(Failure::Writable);
        }
        Ok(())
    }

    fn select(&self, range: TextRange) -> Result<(), Failure> {
        let location = isize::try_from(range.location).map_err(|_| Failure::Selection)?;
        let length = isize::try_from(range.length).map_err(|_| Failure::Selection)?;
        let raw = CFRange { location, length };
        let value =
            OwnedCf::new(unsafe { AXValueCreate(AX_CF_RANGE, (&raw as *const CFRange).cast()) })
                .ok_or(Failure::Selection)?;
        set(self.element.0, "AXSelectedTextRange", value.0)
    }
}

impl Target for NativeTarget {
    fn read(&mut self) -> Result<Snapshot, Failure> {
        self.validate()?;
        let text = attribute(self.element.0, "AXValue")
            .ok()
            .and_then(|v| v.string(model::MAX_FIELD))
            .ok_or(Failure::Content)?;
        let value =
            attribute(self.element.0, "AXSelectedTextRange").map_err(|_| Failure::Selection)?;
        let mut raw = CFRange {
            location: -1,
            length: -1,
        };
        unsafe {
            if CFGetTypeID(value.0) != AXValueGetTypeID()
                || AXValueGetType(value.0) != AX_CF_RANGE
                || !AXValueGetValue(value.0, AX_CF_RANGE, (&mut raw as *mut CFRange).cast())
            {
                return Err(Failure::Selection);
            }
        }
        let selection = TextRange {
            location: usize::try_from(raw.location).map_err(|_| Failure::Selection)?,
            length: usize::try_from(raw.length).map_err(|_| Failure::Selection)?,
        };
        model::byte_range(&text, selection).ok_or(Failure::Selection)?;
        Ok(Snapshot { text, selection })
    }

    fn replace(
        &mut self,
        range: TextRange,
        text: &str,
        expected: &Snapshot,
    ) -> Result<(), Failure> {
        if self.pending.is_some() {
            return Err(Failure::Write);
        }
        if self.read()? != *expected {
            return Err(Failure::Ownership);
        }
        let bytes = model::byte_range(&expected.text, range).ok_or(Failure::Selection)?;
        let mut next_text = expected.text.clone();
        next_text.replace_range(bytes, text);
        let caret = TextRange {
            location: range.location + text.encode_utf16().count(),
            length: 0,
        };
        let next = Snapshot {
            text: next_text,
            selection: caret,
        };
        let deadline = Instant::now() + READBACK_TIMEOUT;
        if settable(self.element.0, "AXSelectedText")? {
            if expected.selection != range {
                self.select(range)?;
                self.pending = Some(PendingWrite {
                    step: WriteStep::Selection {
                        before: expected.clone(),
                        selected: Snapshot {
                            text: expected.text.clone(),
                            selection: range,
                        },
                        text: text.to_owned(),
                    },
                    next,
                    deadline,
                });
                return Ok(());
            }
            if self.read()? != *expected {
                return Err(Failure::Ownership);
            }
            set(
                self.element.0,
                "AXSelectedText",
                CFString::new(text).as_CFTypeRef(),
            )?;
        } else {
            if !settable(self.element.0, "AXValue")? {
                return Err(Failure::Writable);
            }
            if self.read()? != *expected {
                return Err(Failure::Ownership);
            }
            set(
                self.element.0,
                "AXValue",
                CFString::new(&next.text).as_CFTypeRef(),
            )?;
        }
        self.pending = Some(PendingWrite {
            step: WriteStep::Text {
                before: expected.clone(),
            },
            next,
            deadline,
        });
        Ok(())
    }

    fn confirm(&mut self, expected: &Snapshot) -> Result<bool, Failure> {
        let Some(mut pending) = self.pending.take() else {
            return if self.read()? == *expected {
                Ok(true)
            } else {
                Err(Failure::Ownership)
            };
        };
        if pending.next != *expected {
            return Err(Failure::Ownership);
        }
        // Poll on a subsequent AppKit timer turn; no sleep, nested run loop,
        // repeated text submission or silent acceptance of unrelated changes.
        let current = self.read()?;
        match pending.observe(&current, Instant::now())? {
            ConfirmationAction::Done => return Ok(true),
            ConfirmationAction::Wait => {}
            ConfirmationAction::SubmitText => {
                let WriteStep::Selection { text, .. } = &pending.step else {
                    return Err(Failure::Write);
                };
                if self.read()? != current {
                    return Err(Failure::Ownership);
                }
                set(
                    self.element.0,
                    "AXSelectedText",
                    CFString::new(text).as_CFTypeRef(),
                )?;
                pending.step = WriteStep::Text { before: current };
            }
            ConfirmationAction::SetCaret => {
                if self.read()? != current {
                    return Err(Failure::Ownership);
                }
                self.select(pending.next.selection)?;
                pending.step = WriteStep::Caret { before: current };
            }
        }
        self.pending = Some(pending);
        Ok(false)
    }

    fn abandon(&mut self) {
        self.pending = None;
    }
}

fn unicode_events(raw: &str) -> Result<(OwnedCf, OwnedCf, OwnedCf), Failure> {
    let units: Vec<u16> = raw.encode_utf16().collect();
    if units.is_empty() || units.len() > 20 || model::automatic(raw) != raw {
        return Err(Failure::Event);
    }
    // kCGEventSourceStatePrivate = -1. The direct C ABI avoids the Rust
    // wrapper's optional OS-version feature gate for CGEventPostToPid.
    let source = OwnedCf::new(unsafe { CGEventSourceCreate(-1) }).ok_or(Failure::Event)?;
    let down = OwnedCf::new(unsafe { CGEventCreateKeyboardEvent(source.0, 0, true) })
        .ok_or(Failure::Event)?;
    let up = OwnedCf::new(unsafe { CGEventCreateKeyboardEvent(source.0, 0, false) })
        .ok_or(Failure::Event)?;
    for event in [&down, &up] {
        unsafe {
            CGEventKeyboardSetUnicodeString(event.0, units.len(), units.as_ptr());
            CGEventSetFlags(event.0, 0);
            CGEventSetIntegerValueField(event.0, 42, model::EVENT_TAG);
        }
    }
    Ok((source, down, up))
}

impl TerminalTarget for NativeTarget {
    fn current(&mut self) -> Result<(), Failure> {
        self.monitor.as_ref().ok_or(Failure::Monitor)?.check()?;
        self.read()?;
        if !self.is_terminal() {
            return Err(Failure::Focus);
        }
        Ok(())
    }

    fn post(&mut self, raw: &str) -> Result<(), Failure> {
        self.current()?;
        let (_source, down, up) = unicode_events(raw)?;
        // Construct both events before posting either; key-up must pair a
        // posted key-down even if a later validation would fail. The next
        // chunk waits for another main run-loop turn and monitor callbacks.
        self.current()?;
        unsafe {
            CGEventPostToPid(self.pid, down.0);
            CGEventPostToPid(self.pid, up.0);
        }
        Ok(())
    }

    fn end(&mut self) {
        self.monitor = None;
    }
}

/// AppKit global monitors exclude this process, so a local pass-through monitor
/// covers clicks/keys in VoicePort too. No callback stores input text.
struct UserMonitor {
    global: Retained<AnyObject>,
    local: Retained<AnyObject>,
    invalidated: Arc<AtomicBool>,
}

fn observe(event: &NSEvent, invalidated: &AtomicBool) {
    let raw: *const c_void = unsafe { msg_send![event, CGEvent] };
    let tag = if raw.is_null() {
        0
    } else {
        unsafe { CGEventGetIntegerValueField(raw, 42) }
    };
    let key_down = event.r#type() == NSEventType::KeyDown;
    // keyCode is meaningful only for keyboard events.
    let key = if key_down { event.keyCode() } else { 0 };
    if model::invalidates_input(tag, key_down, key, event.modifierFlags().0 as u64) {
        invalidated.store(true, Ordering::SeqCst);
    }
}

fn user_input_event_mask() -> NSEventMask {
    // Scrolling a transcript does not edit the terminal's pending input. If a
    // scroll does change focus, content or selection, current() still rejects
    // that target before posting any more text.
    NSEventMask::KeyDown
        | NSEventMask::LeftMouseDown
        | NSEventMask::RightMouseDown
        | NSEventMask::OtherMouseDown
}

impl UserMonitor {
    fn start() -> Result<Self, Failure> {
        if !main_thread() {
            return Err(Failure::MainThread);
        }
        let invalidated = Arc::new(AtomicBool::new(false));
        let mask = user_input_event_mask();
        let global_flag = invalidated.clone();
        let global_block = RcBlock::new(move |event: NonNull<NSEvent>| {
            observe(unsafe { event.as_ref() }, &global_flag);
        });
        let global = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global_block)
            .ok_or(Failure::Monitor)?;
        let local_flag = invalidated.clone();
        let local_block = RcBlock::new(move |event: NonNull<NSEvent>| -> *mut NSEvent {
            observe(unsafe { event.as_ref() }, &local_flag);
            event.as_ptr()
        });
        let local =
            unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local_block) };
        let Some(local) = local else {
            unsafe {
                NSEvent::removeMonitor(&global);
            }
            return Err(Failure::Monitor);
        };
        Ok(Self {
            global,
            local,
            invalidated,
        })
    }

    fn check(&self) -> Result<(), Failure> {
        if self.invalidated.load(Ordering::SeqCst) {
            Err(Failure::UserInput)
        } else {
            Ok(())
        }
    }
}

impl Drop for UserMonitor {
    fn drop(&mut self) {
        // Delivery is !Send and can only acquire monitors on the main thread.
        unsafe {
            NSEvent::removeMonitor(&self.global);
            NSEvent::removeMonitor(&self.local);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_foundation::{base::kCFAllocatorDefault, string::CFStringCreateWithCharacters};

    fn snapshot(text: &str, location: usize, length: usize) -> Snapshot {
        Snapshot {
            text: text.into(),
            selection: TextRange { location, length },
        }
    }

    #[test]
    fn readback_observation_waits_for_each_step_without_repeating_text() {
        let now = Instant::now();
        let before = snapshot("abc", 3, 0);
        let selected = snapshot("abc", 1, 1);
        let next = snapshot("a新c", 2, 0);
        let mut pending = PendingWrite {
            step: WriteStep::Selection {
                before: before.clone(),
                selected: selected.clone(),
                text: "新".into(),
            },
            next: next.clone(),
            deadline: now + READBACK_TIMEOUT,
        };
        assert_eq!(pending.observe(&before, now), Ok(ConfirmationAction::Wait));
        assert_eq!(
            pending.observe(&selected, now),
            Ok(ConfirmationAction::SubmitText)
        );
        pending.step = WriteStep::Text {
            before: selected.clone(),
        };
        assert_eq!(
            pending.observe(&selected, now),
            Ok(ConfirmationAction::Wait)
        );
        let text_updated = snapshot("a新c", 1, 1);
        assert_eq!(
            pending.observe(&text_updated, now),
            Ok(ConfirmationAction::SetCaret)
        );
        pending.step = WriteStep::Caret {
            before: text_updated.clone(),
        };
        assert_eq!(
            pending.observe(&text_updated, now),
            Ok(ConfirmationAction::Wait)
        );
        assert_eq!(pending.observe(&next, now), Ok(ConfirmationAction::Done));
    }

    #[test]
    fn readback_observation_rejects_foreign_changes_and_times_out_without_a_write() {
        let now = Instant::now();
        let before = snapshot("abc", 0, 0);
        let next = snapshot("新abc", 1, 0);
        let pending = PendingWrite {
            step: WriteStep::Text {
                before: before.clone(),
            },
            next: next.clone(),
            deadline: now,
        };
        assert_eq!(pending.observe(&before, now), Err(Failure::ReadbackTimeout));
        assert_eq!(
            pending.observe(&snapshot("用户编辑", 4, 0), now),
            Err(Failure::Ownership)
        );
        assert_eq!(
            pending.observe(&snapshot("新abc", 3, 0), now),
            Err(Failure::Ownership)
        );
        assert_eq!(pending.observe(&next, now), Ok(ConfirmationAction::Done));
    }

    #[test]
    fn preparation_waits_only_for_missing_focus_within_its_deadline() {
        let missing = Failure::FocusRead(AX_NO_VALUE);
        assert!(waiting_for_focus(missing, Duration::ZERO));
        assert!(waiting_for_focus(missing, Duration::from_millis(1999)));
        assert!(!waiting_for_focus(missing, Duration::from_secs(2)));
        for failure in [
            Failure::FocusRead(-25204),
            Failure::FocusRead(-25211),
            Failure::Accessibility,
            Failure::SecureInput,
            Failure::UserInput,
            Failure::FocusApplicationChanged,
            Failure::FocusElementChanged,
        ] {
            assert!(!waiting_for_focus(failure, Duration::ZERO));
        }
    }

    #[test]
    fn terminal_monitor_allows_scroll_but_still_observes_typing_and_clicks() {
        let mask = user_input_event_mask();
        assert!(!mask.contains(NSEventMask::ScrollWheel));
        for event in [
            NSEventMask::KeyDown,
            NSEventMask::LeftMouseDown,
            NSEventMask::RightMouseDown,
            NSEventMask::OtherMouseDown,
        ] {
            assert!(mask.contains(event));
        }
    }

    #[test]
    fn capability_transport_failures_are_unknown_but_unsupported_is_known_false() {
        assert_eq!(writable_result(0, 0), Ok(false));
        assert_eq!(writable_result(0, 1), Ok(true));
        for status in [AX_UNSUPPORTED, AX_NO_VALUE] {
            assert_eq!(writable_result(status, 0), Ok(false));
            assert_eq!(writable_result(status, 255), Ok(false));
        }
        for (status, value) in [(0, 2), (0, 255), (-25202, 1), (-25211, 0), (-25204, 1)] {
            assert_eq!(writable_result(status, value), Err(Failure::Writable));
        }
    }

    #[test]
    fn optional_enabled_attribute_does_not_reject_native_text_views() {
        assert_eq!(enabled_result(AX_UNSUPPORTED, None), Ok(()));
        assert_eq!(enabled_result(0, Some(true)), Ok(()));
        for (status, enabled) in [
            (0, Some(false)),
            (0, None),
            (AX_NO_VALUE, None),
            (-25204, None),
            (-25211, None),
        ] {
            assert_eq!(enabled_result(status, enabled), Err(Failure::Disabled));
        }
    }

    #[test]
    fn failed_native_allocation_does_not_release_a_null_pointer() {
        assert!(OwnedCf::new(ptr::null()).is_none());
    }

    #[test]
    fn native_string_decoder_rejects_unpaired_utf16_without_reading_any_target() {
        for units in [&[0xd800u16][..], &[0xdc00][..], &[0xd800, 0x0041][..]] {
            let raw = unsafe {
                CFStringCreateWithCharacters(
                    kCFAllocatorDefault,
                    units.as_ptr(),
                    units.len() as isize,
                )
            };
            let value = OwnedCf::new(raw.cast()).unwrap();
            assert!(value.string(100).is_none());
        }
        let text = CFString::new("A😀中");
        let value =
            OwnedCf::new(unsafe { core_foundation::base::CFRetain(text.as_CFTypeRef()) }).unwrap();
        assert_eq!(value.string(4).as_deref(), Some("A😀中"));
        assert!(value.string(3).is_none());
    }
}
