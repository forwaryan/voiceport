use std::{cell::RefCell, rc::Rc};
use voiceport_macos::delivery::model::{
    ChunkedInput, DeliveryState as S, Failure, KeyboardPacing, MenuPaste, OwnedSpan, PasteTarget,
    Snapshot, Target, Terminal, TerminalTarget, TextRange, byte_range,
};

struct State {
    snapshot: Snapshot,
    writes: usize,
    ready: bool,
    confirm: bool,
    failure: Option<Failure>,
    ambiguous: bool,
}
fn state() -> Rc<RefCell<State>> {
    Rc::new(RefCell::new(State {
        snapshot: Snapshot {
            text: "左|右".into(),
            selection: TextRange {
                location: 1,
                length: 1,
            },
        },
        writes: 0,
        ready: false,
        confirm: false,
        failure: None,
        ambiguous: false,
    }))
}
struct Field(Rc<RefCell<State>>);
impl Target for Field {
    fn read(&mut self) -> Result<Snapshot, Failure> {
        let s = self.0.borrow();
        s.failure.map_or_else(|| Ok(s.snapshot.clone()), Err)
    }
    fn ready(&mut self) -> Result<bool, Failure> {
        Ok(self.0.borrow().ready)
    }
    fn replace(
        &mut self,
        range: TextRange,
        text: &str,
        expected: &Snapshot,
    ) -> Result<(), Failure> {
        let mut s = self.0.borrow_mut();
        assert_eq!(&s.snapshot, expected);
        let bytes = byte_range(&s.snapshot.text, range).unwrap();
        s.snapshot.text.replace_range(bytes, text);
        s.snapshot.selection = TextRange {
            location: range.location + text.encode_utf16().count(),
            length: 0,
        };
        s.writes += 1;
        if s.ambiguous {
            Err(Failure::Write)
        } else {
            Ok(())
        }
    }
    fn confirm(&mut self, expected: &Snapshot) -> Result<bool, Failure> {
        let s = self.0.borrow();
        if let Some(failure) = s.failure {
            return Err(failure);
        }
        if !s.confirm {
            return Ok(false);
        }
        assert_eq!(&s.snapshot, expected);
        Ok(true)
    }
}

#[test]
fn ax_receipt_tracks_preview_final_and_readback() {
    let shared = state();
    let mut span = OwnedSpan::capture(Field(shared.clone())).unwrap();
    assert_eq!(span.receipt().state, S::NotSent);
    span.update("预览", false).unwrap();
    assert_eq!(span.receipt().state, S::AwaitingConfirmation);
    assert_eq!(span.receipt().submitted_operations, 1);
    span.update("最终🙂", true).unwrap();
    span.update("迟到", false).unwrap();
    shared.borrow_mut().confirm = true;
    assert!(span.poll().unwrap());
    assert_eq!(shared.borrow().snapshot.text, "左最终🙂右");
    assert_eq!(span.receipt().state, S::Confirmed);
    assert_eq!(span.receipt().confirmed_operations, 2);
    assert_eq!(span.update("duplicate", true), Err(Failure::Ended));
    assert_eq!(shared.borrow().writes, 2);
}

#[test]
fn cancel_pending_ax_and_ambiguous_write_latch_uncertainty() {
    for ambiguous in [false, true] {
        let shared = state();
        shared.borrow_mut().ambiguous = ambiguous;
        let mut span = OwnedSpan::capture(Field(shared.clone())).unwrap();
        let result = span.update("可能已写", true);
        if ambiguous {
            assert_eq!(result, Err(Failure::Write));
        } else {
            result.unwrap();
            span.cancel().unwrap();
        }
        assert_eq!(span.receipt().state, S::Uncertain);
        assert!(span.receipt().stopped);
        shared.borrow_mut().confirm = true;
        let _ = span.poll();
        assert!(span.update("duplicate", true).is_err());
        assert_eq!(span.receipt().state, S::Uncertain);
        assert_eq!(shared.borrow().writes, 1);
    }
}

#[test]
fn unicode_wait_partial_failure_and_late_release_never_replay() {
    let shared = state();
    let mut input =
        ChunkedInput::capture_with_pacing(Field(shared.clone()), KeyboardPacing::Scalar).unwrap();
    input.submit("中🙂尾").unwrap();
    assert_eq!(input.receipt().state, S::Waiting);
    assert_eq!(shared.borrow().writes, 0);
    shared.borrow_mut().ready = true;
    input.poll().unwrap();
    assert_eq!(input.receipt().state, S::AwaitingConfirmation);
    {
        let mut s = shared.borrow_mut();
        s.confirm = true;
        s.ready = false;
    }
    input.poll().unwrap();
    assert_eq!(input.receipt().state, S::PartiallySent);
    assert_eq!(input.receipt().confirmed_operations, 1);
    shared.borrow_mut().failure = Some(Failure::UserInput);
    assert_eq!(input.poll(), Err(Failure::UserInput));
    assert!(input.receipt().stopped);
    assert_eq!(input.receipt().state, S::PartiallySent);
    {
        let mut s = shared.borrow_mut();
        s.failure = None;
        s.ready = true;
    }
    assert_eq!(input.poll(), Err(Failure::UserInput));
    assert_eq!(input.submit("duplicate"), Err(Failure::UserInput));
    assert_eq!(shared.borrow().writes, 1);
}

#[test]
fn unicode_acknowledges_every_chunk_and_cancel_pending_is_uncertain() {
    for cancel in [false, true] {
        let shared = state();
        shared.borrow_mut().ready = true;
        let mut input =
            ChunkedInput::capture_with_pacing(Field(shared.clone()), KeyboardPacing::Scalar)
                .unwrap();
        input.submit("中🙂尾").unwrap();
        if cancel {
            input.end();
            assert_eq!(input.receipt().state, S::Uncertain);
            assert!(input.poll().unwrap());
            assert_eq!(shared.borrow().writes, 1);
        } else {
            shared.borrow_mut().confirm = true;
            while !input.poll().unwrap() {}
            assert_eq!(input.receipt().state, S::Confirmed);
            assert_eq!(input.receipt().submitted_operations, 3);
            assert_eq!(input.receipt().confirmed_operations, 3);
            assert!(!input.receipt().stopped);
        }
        assert!(input.submit("duplicate").is_err());
    }
}

#[test]
fn sealed_preview_finishes_and_oversized_late_preview_cannot_erase_final() {
    let shared = state();
    shared.borrow_mut().confirm = true;
    let mut span = OwnedSpan::capture(Field(shared.clone())).unwrap();
    span.update("预览", false).unwrap();
    assert_eq!(span.receipt().state, S::PartiallySent);
    span.seal();
    assert_eq!(span.receipt().state, S::Confirmed);

    let shared = state();
    let mut span = OwnedSpan::capture(Field(shared.clone())).unwrap();
    span.update("最终", true).unwrap();
    span.update(&"迟".repeat(100_001), false).unwrap();
    assert_eq!(span.receipt().state, S::AwaitingConfirmation);
    shared.borrow_mut().confirm = true;
    assert!(span.poll().unwrap());
    assert_eq!(span.receipt().state, S::Confirmed);
    assert_eq!(shared.borrow().writes, 1);
}

struct Paste(Rc<RefCell<State>>);
impl PasteTarget for Paste {
    fn check(&mut self) -> Result<(), Failure> {
        Ok(())
    }
    fn prepare(&mut self, _: &str) -> Result<(), Failure> {
        Ok(())
    }
    fn ready(&mut self) -> Result<bool, Failure> {
        Ok(self.0.borrow().ready)
    }
    fn paste(&mut self) -> Result<(), Failure> {
        let mut s = self.0.borrow_mut();
        s.writes += 1;
        if s.ambiguous {
            Err(Failure::PasteRequest)
        } else {
            Ok(())
        }
    }
    fn abandon(&mut self) {}
}
#[test]
fn clipboard_submission_is_not_confirmation_and_uncertainty_cannot_retry() {
    for ambiguous in [false, true] {
        let shared = state();
        shared.borrow_mut().ambiguous = ambiguous;
        let mut paste = MenuPaste::capture(Paste(shared.clone())).unwrap();
        paste.submit("最终").unwrap();
        assert_eq!(paste.receipt().state, S::Waiting);
        assert!(!paste.poll().unwrap());
        shared.borrow_mut().ready = true;
        let _ = paste.poll();
        assert_eq!(
            paste.receipt().state,
            if ambiguous {
                S::Uncertain
            } else {
                S::Submitted
            }
        );
        assert_eq!(paste.receipt().confirmed_operations, 0);
        for _ in 0..3 {
            let _ = paste.poll();
        }
        assert!(paste.submit("duplicate").is_err());
        assert_eq!(shared.borrow().writes, 1);
    }
}

struct Console(Rc<RefCell<State>>);
impl TerminalTarget for Console {
    fn current(&mut self) -> Result<(), Failure> {
        Ok(())
    }
    fn post(&mut self, _: &str) -> Result<(), Failure> {
        let mut s = self.0.borrow_mut();
        s.writes += 1;
        if s.ambiguous {
            Err(Failure::Event)
        } else {
            Ok(())
        }
    }
    fn end(&mut self) {}
}
#[test]
fn terminal_only_reports_submission_and_retains_ambiguous_failure() {
    for ambiguous in [false, true] {
        let shared = state();
        shared.borrow_mut().ambiguous = ambiguous;
        let mut terminal = Terminal::capture(Console(shared.clone())).unwrap();
        terminal.update("最终🙂", true).unwrap();
        let _ = terminal.poll();
        assert_eq!(
            terminal.receipt().state,
            if ambiguous {
                S::Uncertain
            } else {
                S::Submitted
            }
        );
        assert_eq!(terminal.receipt().confirmed_operations, 0);
        assert_eq!(terminal.receipt().stopped, ambiguous);
        let _ = terminal.poll();
        assert!(terminal.update("duplicate", true).is_err());
        assert_eq!(shared.borrow().writes, 1);
    }
}
