//! Final-only compatibility input for controls without AX text editing APIs.
//! Bind metadata, never read field contents or the previous clipboard. Post one
//! ordinary Cmd+V to the original process; posting is not text acknowledgement.
use super::*;

const PASTE_TURN: Duration = Duration::from_millis(30);
const PASTE_DEADLINE: Duration = Duration::from_secs(1);
const COMMAND: u64 = 1 << 20;

fn paste_events(key: u16) -> Result<(OwnedCf, OwnedCf, OwnedCf), Failure> {
    let source = OwnedCf::new(unsafe { CGEventSourceCreate(-1) }).ok_or(Failure::Event)?;
    let down = OwnedCf::new(unsafe { CGEventCreateKeyboardEvent(source.0, key, true) })
        .ok_or(Failure::Event)?;
    let up = OwnedCf::new(unsafe { CGEventCreateKeyboardEvent(source.0, key, false) })
        .ok_or(Failure::Event)?;
    for event in [&down, &up] {
        unsafe {
            // Do not set a Unicode override: AppKit may treat it as text input
            // instead of dispatching the Command key equivalent.
            CGEventSetFlags(event.0, COMMAND);
            CGEventSetIntegerValueField(event.0, 42, model::EVENT_TAG);
        }
    }
    Ok((source, down, up))
}

fn missing(status: i32) -> bool {
    matches!(status, AX_UNSUPPORTED | AX_NO_VALUE | -25208)
}

pub(super) fn missing_focus(failure: Failure) -> bool {
    matches!(failure, Failure::FocusRead(status) if missing(status))
}

fn optional_text(element: &OwnedCf, key: &str) -> Result<Option<String>, Failure> {
    match attribute(element.0, key) {
        Ok(value) => value.string(256).map(Some).ok_or(Failure::Role),
        Err(status) if missing(status) => Ok(None),
        Err(_) => Err(Failure::Focus),
    }
}

fn optional_enabled(element: &OwnedCf, key: &str) -> Result<(), Failure> {
    match attribute(element.0, key) {
        Ok(value) => unsafe {
            if CFGetTypeID(value.0) != CFBooleanGetTypeID()
                || CFEqual(value.0, kCFBooleanTrue.cast()) == 0
            {
                return Err(Failure::Disabled);
            }
            Ok(())
        },
        Err(status) if missing(status) => Ok(()),
        Err(_) => Err(Failure::Disabled),
    }
}

fn paste_role(role: Option<&str>) -> bool {
    // Some canvas/Java/Electron editors expose a container instead of a text
    // field. Known buttons, links, tables and static text are not input targets.
    matches!(
        role,
        None | Some(
            "AXTextField"
                | "AXTextArea"
                | "AXComboBox"
                | "AXGroup"
                | "AXUnknown"
                | "AXWebArea"
                | "AXScrollArea"
                | "AXLayoutArea"
        )
    )
}

fn check_element(element: &OwnedCf) -> Result<Option<String>, Failure> {
    let role = optional_text(element, "AXRole")?;
    let subrole = optional_text(element, "AXSubrole")?;
    if role.as_deref() == Some("AXSecureTextField")
        || subrole.as_deref() == Some("AXSecureTextField")
    {
        return Err(Failure::Sensitive);
    }
    if !paste_role(role.as_deref()) {
        return Err(Failure::Role);
    }
    optional_enabled(element, "AXEnabled")?;
    optional_enabled(element, "AXEditable")?;
    Ok(role)
}

fn write_capability(element: &OwnedCf, key: &str) -> Result<bool, Failure> {
    let key = CFString::new(key);
    let mut value = 0;
    let status =
        unsafe { AXUIElementIsAttributeSettable(element.0, key.as_CFTypeRef(), &mut value) };
    if missing(status) {
        Ok(false)
    } else {
        writable_result(status, value)
    }
}

/// Only known absent capabilities select this backend. Permission/transport
/// errors, malformed content and errors after a write must never downgrade.
pub(super) fn needs_compatibility(element: &OwnedCf) -> Result<bool, Failure> {
    let role = check_element(element)?;
    if !matches!(
        role.as_deref(),
        Some("AXTextField" | "AXTextArea" | "AXComboBox")
    ) {
        return Ok(true);
    }
    if !write_capability(element, "AXSelectedTextRange")?
        || !(write_capability(element, "AXSelectedText")? || write_capability(element, "AXValue")?)
    {
        return Ok(true);
    }
    for key in ["AXValue", "AXSelectedTextRange"] {
        match attribute(element.0, key) {
            Ok(_) => {}
            Err(status) if missing(status) => return Ok(true),
            Err(_) => return Err(Failure::Focus),
        }
    }
    Ok(false)
}

pub(super) struct ClipboardTarget {
    pid: libc::pid_t,
    bundle: String,
    window: OwnedCf,
    element: Option<OwnedCf>,
    monitor: UserMonitor,
    initial_clipboard: isize,
    owned_clipboard: Option<isize>,
    prepared_at: Option<Instant>,
    attempted: bool,
}

impl ClipboardTarget {
    pub(super) fn capture(
        pid: libc::pid_t,
        bundle: String,
        element: Option<OwnedCf>,
        monitor: Option<UserMonitor>,
    ) -> Result<Self, Failure> {
        let monitor = match monitor {
            Some(monitor) => monitor,
            None => UserMonitor::start()?,
        };
        let target = Self {
            pid,
            bundle,
            window: application_window(pid)?,
            element,
            monitor,
            initial_clipboard: NSPasteboard::generalPasteboard().changeCount(),
            owned_clipboard: None,
            prepared_at: None,
            attempted: false,
        };
        target.validate()?;
        Ok(target)
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
        self.monitor.check()?;
        let (pid, bundle) = frontmost()?;
        if pid != self.pid || bundle != self.bundle {
            return Err(Failure::FocusApplicationChanged);
        }
        let window = application_window(pid)?;
        if unsafe { CFEqual(window.0, self.window.0) } == 0 {
            return Err(Failure::FocusElementChanged);
        }
        match (&self.element, focused(pid)) {
            (Some(original), Ok(current)) => {
                if unsafe { CFEqual(original.0, current.0) } == 0 {
                    return Err(Failure::FocusElementChanged);
                }
                check_element(&current)?;
            }
            (None, Err(failure)) if missing_focus(failure) => {}
            (_, Err(failure)) => return Err(failure),
            (None, Ok(_)) => return Err(Failure::FocusElementChanged),
        }
        let expected = self.owned_clipboard.unwrap_or(self.initial_clipboard);
        if expected < 0 || NSPasteboard::generalPasteboard().changeCount() != expected {
            return Err(Failure::ClipboardChanged);
        }
        Ok(())
    }
}

impl PasteTarget for ClipboardTarget {
    fn check(&mut self) -> Result<(), Failure> {
        self.validate()
    }

    fn prepare(&mut self, text: &str) -> Result<(), Failure> {
        if self.owned_clipboard.is_some() || self.attempted {
            return Err(Failure::Ended);
        }
        self.validate()?;
        let board = NSPasteboard::generalPasteboard();
        let count = board.clearContents();
        self.owned_clipboard = Some(count);
        if count < 0 || board.changeCount() != count {
            return Err(Failure::ClipboardChanged);
        }
        if !unsafe { board.setString_forType(&NSString::from_str(text), NSPasteboardTypeString) } {
            return Err(Failure::Clipboard);
        }
        self.validate()?;
        self.prepared_at = Some(Instant::now());
        Ok(())
    }

    fn ready(&mut self) -> Result<bool, Failure> {
        self.validate()?;
        let elapsed = self.prepared_at.ok_or(Failure::PasteRequest)?.elapsed();
        if elapsed >= PASTE_DEADLINE {
            return Err(Failure::PasteRequest);
        }
        Ok(elapsed >= PASTE_TURN)
    }

    fn paste(&mut self) -> Result<(), Failure> {
        if self.attempted || !self.ready()? {
            return Err(Failure::PasteRequest);
        }
        // Allocate both events before either is sent. No Return, Delete or
        // synthetic mouse event; plain text was sanitized by the shared policy.
        let key = paste_key::current().ok_or(Failure::Event)?;
        let (_source, down, up) = paste_events(key)?;
        self.validate()?;
        self.attempted = true;
        self.prepared_at = None;
        unsafe {
            CGEventPostToPid(self.pid, down.0);
            CGEventPostToPid(self.pid, up.0);
        }
        Ok(())
    }

    fn abandon(&mut self) {
        self.prepared_at = None;
        // Leave the final text for manual recovery. Never read the previous
        // clipboard, restore a stale value or overwrite a newer clipboard owner.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_events_use_the_resolved_physical_key_without_posting_anything() {
        unsafe extern "C" {
            fn CGEventGetFlags(event: CFTypeRef) -> u64;
            fn CGEventGetType(event: CFTypeRef) -> u32;
        }
        // US and ordinary Dvorak use different physical keys for Command+V.
        for key in [9, 47] {
            let (_source, down, up) = paste_events(key).unwrap();
            for (event, kind) in [(down, 10), (up, 11)] {
                unsafe {
                    assert_eq!(CGEventGetType(event.0), kind);
                    assert_eq!(CGEventGetFlags(event.0), COMMAND);
                    assert_eq!(CGEventGetIntegerValueField(event.0, 9), i64::from(key));
                    assert_eq!(CGEventGetIntegerValueField(event.0, 42), model::EVENT_TAG);
                }
            }
        }
    }

    #[test]
    fn missing_capabilities_do_not_include_transport_or_permission_errors() {
        for status in [AX_UNSUPPORTED, AX_NO_VALUE, -25208] {
            assert!(missing_focus(Failure::FocusRead(status)));
        }
        for failure in [
            Failure::FocusRead(-25204),
            Failure::FocusRead(-25211),
            Failure::Accessibility,
            Failure::Sensitive,
            Failure::Content,
            Failure::ReadbackTimeout,
            Failure::Write,
        ] {
            assert!(!missing_focus(failure));
        }
    }

    #[test]
    fn custom_editors_are_candidates_but_known_nontext_controls_are_not() {
        for role in [
            None,
            Some("AXTextArea"),
            Some("AXGroup"),
            Some("AXUnknown"),
            Some("AXWebArea"),
            Some("AXScrollArea"),
        ] {
            assert!(paste_role(role));
        }
        for role in [
            "AXButton",
            "AXStaticText",
            "AXLink",
            "AXTable",
            "AXSecureTextField",
        ] {
            assert!(!paste_role(Some(role)));
        }
    }
}
