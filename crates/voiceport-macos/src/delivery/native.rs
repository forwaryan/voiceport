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
    let element = attribute(application.0, "AXFocusedUIElement").map_err(|_| Failure::Focus)?;
    if unsafe { CFGetTypeID(element.0) != AXUIElementGetTypeID() } {
        return Err(Failure::Focus);
    }
    Ok(element)
}

pub(super) enum Captured {
    Ordinary(Box<dyn Target>),
    Terminal(Box<dyn TerminalTarget>),
}

pub(super) fn capture() -> Result<Captured, Failure> {
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
    // Do not toggle AXManualAccessibility or any frontend permission/state.
    let element = focused(pid)?;
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
        monitor: None,
        _main_only: std::marker::PhantomData,
    };
    target.read()?;
    if terminal {
        target.monitor = Some(UserMonitor::start()?);
        target.current()?;
        Ok(Captured::Terminal(Box::new(target)))
    } else {
        Ok(Captured::Ordinary(Box::new(target)))
    }
}

struct NativeTarget {
    pid: libc::pid_t,
    bundle: String,
    element: OwnedCf,
    terminal: bool,
    monitor: Option<UserMonitor>,
    // AX/AppKit work and destruction stay on the main thread.
    _main_only: std::marker::PhantomData<Rc<()>>,
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
        let (pid, bundle) = frontmost()?;
        if pid != self.pid || bundle != self.bundle {
            return Err(Failure::Focus);
        }
        let current = focused(pid)?;
        if unsafe { CFEqual(current.0, self.element.0) } == 0 {
            return Err(Failure::Focus);
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
        let enabled = attribute(self.element.0, "AXEnabled").map_err(|_| Failure::Disabled)?;
        if unsafe {
            CFGetTypeID(enabled.0) != CFBooleanGetTypeID()
                || CFEqual(enabled.0, kCFBooleanTrue.cast()) == 0
        } {
            return Err(Failure::Disabled);
        }
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
        let intermediate;
        if settable(self.element.0, "AXSelectedText")? {
            // Don't select anything after an observable intervening edit.
            self.select(range)?;
            intermediate = Snapshot {
                text: expected.text.clone(),
                selection: range,
            };
            if self.read()? != intermediate {
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
            intermediate = expected.clone();
            set(
                self.element.0,
                "AXValue",
                CFString::new(&next_text).as_CFTypeRef(),
            )?;
        }
        let after = self.read()?;
        if after.text != next_text {
            return Err(Failure::Ownership);
        }
        // AXSelectedText usually moves the caret. AXValue may preserve it.
        // Accept only these known positions before setting our final caret.
        if after.selection != caret {
            if after.selection != intermediate.selection {
                return Err(Failure::Ownership);
            }
            if self.read()? != after {
                return Err(Failure::Ownership);
            }
            self.select(caret)?;
        }
        if self.read()?
            != (Snapshot {
                text: next_text,
                selection: caret,
            })
        {
            return Err(Failure::Ownership);
        }
        Ok(())
    }
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

impl UserMonitor {
    fn start() -> Result<Self, Failure> {
        if !main_thread() {
            return Err(Failure::MainThread);
        }
        let invalidated = Arc::new(AtomicBool::new(false));
        let mask = NSEventMask::KeyDown
            | NSEventMask::LeftMouseDown
            | NSEventMask::RightMouseDown
            | NSEventMask::OtherMouseDown
            | NSEventMask::ScrollWheel;
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
