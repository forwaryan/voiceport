//! Optional read-only geometry/focus for the status panel, not a delivery target.
use super::*;
use core_graphics::geometry::CGRect;

pub(crate) struct OverlayFocus {
    app: OwnedCf,
    window: Option<OwnedCf>,
    field: Option<OwnedCf>,
}

fn read(owner: &OwnedCf, key: &str, deadline: Instant) -> Option<OwnedCf> {
    let remaining = deadline.checked_duration_since(Instant::now())?;
    let timeout = remaining.min(Duration::from_millis(15)).as_secs_f32();
    if timeout == 0. || unsafe { AXUIElementSetMessagingTimeout(owner.0, timeout) } != 0 {
        return None;
    }
    attribute(owner.0, key)
        .ok()
        .filter(|_| Instant::now() < deadline)
}

fn element(owner: &OwnedCf, key: &str, deadline: Instant) -> Option<OwnedCf> {
    let value = read(owner, key, deadline)?;
    (unsafe { CFGetTypeID(value.0) == AXUIElementGetTypeID() }).then_some(value)
}

fn read_deadline() -> Instant {
    Instant::now() + Duration::from_millis(45)
}

impl OverlayFocus {
    pub(crate) fn capture(pid: i32) -> Option<Self> {
        // Do not start a tree scan, prompt, or read field values. In-process
        // focus is read directly by AppKit instead of sending AX IPC to self.
        if !trusted() || secure_input() || pid <= 0 || pid == unsafe { libc::getpid() } {
            return None;
        }
        let app = OwnedCf::new(unsafe { AXUIElementCreateApplication(pid) })?;
        let deadline = read_deadline();
        let window = element(&app, "AXFocusedWindow", deadline);
        let field = element(&app, "AXFocusedUIElement", deadline);
        (window.is_some() || field.is_some()).then_some(Self { app, window, field })
    }

    pub(crate) fn observe(&self) -> Option<(Option<CGRect>, Option<CGRect>)> {
        if !trusted() || secure_input() {
            return None;
        }
        let deadline = read_deadline();
        let matches = [
            (&self.window, "AXFocusedWindow"),
            (&self.field, "AXFocusedUIElement"),
        ]
        .into_iter()
        .all(|(original, key)| {
            original.as_ref().is_none_or(|original| {
                element(&self.app, key, deadline)
                    .is_some_and(|now| unsafe { CFEqual(original.0, now.0) != 0 })
            })
        });
        if !matches {
            return None;
        }

        // Window bounds are a screen hint, never evidence of the input area.
        let bounds = |owner: &OwnedCf| focus::bounds_rect(|key| read(owner, key, deadline));
        let field = self.field.as_ref().and_then(bounds);
        let window = if field.is_none() {
            self.window.as_ref().and_then(bounds)
        } else {
            None
        };
        Some((field, window))
    }
}
