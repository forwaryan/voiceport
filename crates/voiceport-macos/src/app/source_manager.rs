//! Modeless source management. Browsing and saving never run an adapter.
use super::*;
use crate::plugins::{Adapter, FieldKind, Manifest, Source, Store};
mod destination;
mod draft;
mod editor;
mod library;
mod presentation;
pub(in crate::app) use editor::{Editor as CloudEditor, show_cloud};
pub(super) use library::Library as Manager;

fn alert(d: &Delegate, title: &str, detail: &str, buttons: &[&str]) -> Retained<NSAlert> {
    let a = NSAlert::new(d.mtm());
    a.setMessageText(&NSString::from_str(title));
    a.setInformativeText(&NSString::from_str(detail));
    for b in buttons {
        a.addButtonWithTitle(&NSString::from_str(b));
    }
    a
}

pub(super) fn manage(d: &Delegate) -> Result<(), String> {
    let path = crate::plugins::default_path()?;
    let store = Store::load(&path)?;
    library::show(d, store, path);
    Ok(())
}

fn checkbox(
    d: &Delegate,
    view: &NSView,
    title: &str,
    checked: bool,
    frame: NSRect,
) -> Retained<NSButton> {
    let b = NSButton::initWithFrame(NSButton::alloc(d.mtm()), frame);
    b.setButtonType(NSButtonType::Switch);
    b.setTitle(&NSString::from_str(title));
    b.setAccessibilityLabel(Some(&NSString::from_str(title)));
    b.setState(if checked {
        NSControlStateValueOn
    } else {
        NSControlStateValueOff
    });
    view.addSubview(&b);
    b
}

/// Keep utility windows within the current display's usable area, including the titlebar.
pub(super) fn content_size(d: &Delegate, preferred: NSSize) -> NSSize {
    let screen = d
        .ivars()
        .window
        .get()
        .and_then(|w| w.screen())
        .or_else(|| NSScreen::mainScreen(d.mtm()));
    screen.map_or(preferred, |s| {
        let available = s.visibleFrame().size;
        NSSize::new(
            preferred.width.min((available.width - 32.).max(1.)),
            preferred.height.min((available.height - 72.).max(1.)),
        )
    })
}
