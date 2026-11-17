#![cfg(target_os = "macos")]
//! Synthetic menu/clipboard only. No native applications, clipboard or events.
use std::{cell::RefCell, rc::Rc};
use voiceport_macos::delivery::model::{Failure, MAX_TEXT, MenuPaste, PasteTarget};

#[derive(Default)]
struct State {
    failure: Option<Failure>,
    prepare_error: Option<Failure>,
    paste_error: Option<Failure>,
    menu_error: Option<Failure>,
    ready: bool,
    copies: usize,
    pastes: usize,
    clipboard: String,
}

#[derive(Clone, Default)]
struct Fake(Rc<RefCell<State>>);
impl PasteTarget for Fake {
    fn check(&mut self) -> Result<(), Failure> {
        self.0.borrow().failure.map_or(Ok(()), Err)
    }
    fn prepare(&mut self, text: &str) -> Result<(), Failure> {
        let mut s = self.0.borrow_mut();
        s.copies += 1;
        s.clipboard = text.into();
        s.prepare_error.map_or(Ok(()), Err)
    }
    fn ready(&mut self) -> Result<bool, Failure> {
        let s = self.0.borrow();
        s.menu_error.map_or(Ok(s.ready), Err)
    }
    fn paste(&mut self) -> Result<(), Failure> {
        let mut s = self.0.borrow_mut();
        s.pastes += 1;
        s.paste_error.map_or(Ok(()), Err)
    }
    fn abandon(&mut self) {}
}

#[test]
fn preview_is_inert_and_final_paste_waits_without_repeating_any_action() {
    let fake = Fake::default();
    let mut paste = MenuPaste::capture(Box::new(fake.clone()) as Box<dyn PasteTarget>).unwrap();
    paste.verify().unwrap();
    paste.verify().unwrap();
    assert_eq!(fake.0.borrow().copies, 0);
    paste.submit("中文\n🙂\r\n尾").unwrap();
    assert_eq!(fake.0.borrow().clipboard, "中文 🙂 尾");
    assert_eq!(paste.submit("duplicate"), Err(Failure::Ended));
    for _ in 0..3 {
        assert!(!paste.poll().unwrap());
    }
    assert_eq!(fake.0.borrow().copies, 1);
    assert_eq!(fake.0.borrow().pastes, 0);
    fake.0.borrow_mut().ready = true;
    assert!(paste.poll().unwrap());
    assert!(paste.requested());
    assert!(!paste.has_pending());
    assert!(paste.poll().unwrap());
    assert_eq!(fake.0.borrow().copies, 1);
    assert_eq!(fake.0.borrow().pastes, 1);
}

#[test]
fn focus_user_input_security_and_clipboard_changes_stop_before_or_after_copy() {
    for failure in [
        Failure::FocusApplicationChanged,
        Failure::FocusElementChanged,
        Failure::UserInput,
        Failure::Accessibility,
        Failure::SecureInput,
        Failure::ClipboardChanged,
        Failure::ModifiersHeld,
    ] {
        for before in [true, false] {
            let fake = Fake::default();
            let mut paste = MenuPaste::capture(fake.clone()).unwrap();
            if !before {
                paste.submit("speech").unwrap();
            }
            fake.0.borrow_mut().failure = Some(failure);
            let result = if before {
                paste.submit("speech")
            } else {
                paste.poll().map(|_| ())
            };
            assert_eq!(result, Err(failure));
            assert!(!paste.has_pending());
            fake.0.borrow_mut().failure = None;
            fake.0.borrow_mut().ready = true;
            assert_eq!(paste.poll(), Err(failure));
            assert_eq!(fake.0.borrow().copies, usize::from(!before));
            assert_eq!(fake.0.borrow().pastes, 0);
        }
    }
}

#[test]
fn failed_preparation_or_unavailable_menu_does_not_retry_or_paste() {
    for during_prepare in [true, false] {
        let fake = Fake::default();
        let mut paste = MenuPaste::capture(fake.clone()).unwrap();
        let result = if during_prepare {
            fake.0.borrow_mut().prepare_error = Some(Failure::Menu);
            paste.submit("speech")
        } else {
            paste.submit("speech").unwrap();
            fake.0.borrow_mut().menu_error = Some(Failure::Menu);
            paste.poll().map(|_| ())
        };
        assert_eq!(result, Err(Failure::Menu));
        assert_eq!(paste.submit("retry"), Err(Failure::Menu));
        assert_eq!(fake.0.borrow().copies, 1);
        assert_eq!(fake.0.borrow().pastes, 0);
    }
}

#[test]
fn ambiguous_paste_failure_is_never_replayed() {
    let fake = Fake::default();
    fake.0.borrow_mut().ready = true;
    fake.0.borrow_mut().paste_error = Some(Failure::PasteRequest);
    let mut paste = MenuPaste::capture(fake.clone()).unwrap();
    paste.submit("speech").unwrap();
    assert_eq!(paste.poll(), Err(Failure::PasteRequest));
    fake.0.borrow_mut().paste_error = None;
    assert_eq!(paste.poll(), Err(Failure::PasteRequest));
    assert_eq!(fake.0.borrow().pastes, 1);
    assert!(!paste.requested());
}

#[test]
fn cancellation_preserves_clipboard_without_requesting_paste() {
    let fake = Fake::default();
    let mut paste = MenuPaste::capture(fake.clone()).unwrap();
    paste.submit("speech").unwrap();
    paste.end();
    fake.0.borrow_mut().ready = true;
    assert!(paste.poll().unwrap());
    assert_eq!(fake.0.borrow().clipboard, "speech");
    assert_eq!(fake.0.borrow().pastes, 0);
    assert!(!paste.requested());
}

#[test]
fn empty_or_oversize_results_never_touch_clipboard_or_menu() {
    for text in [String::new(), "a".repeat(MAX_TEXT + 1)] {
        let fake = Fake::default();
        let mut paste = MenuPaste::capture(fake.clone()).unwrap();
        let result = paste.submit(&text);
        if text.is_empty() {
            result.unwrap();
            assert!(paste.poll().unwrap());
        } else {
            assert_eq!(result, Err(Failure::Content));
        }
        assert_eq!(fake.0.borrow().copies, 0);
        assert_eq!(fake.0.borrow().pastes, 0);
    }
}
