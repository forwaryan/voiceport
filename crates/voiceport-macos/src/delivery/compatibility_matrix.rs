//! Post-capture integration coverage using isolated deterministic receivers.
//! Only the OS focus, readback, clipboard and key-event boundaries are faked.
use super::*;
use model::{DeliveryState, KeyboardPacing, Snapshot, TextRange, byte_range};
use std::{cell::RefCell, rc::Rc};

const TEXT: &str = "中🙂";
const BASELINE: &str = "AB";

#[derive(Debug, Clone, Copy)]
enum Surface {
    NativeField,
    WebRichText,
    OrcaComposer,
    Terminal,
    ClipboardFallback,
}

const SURFACES: [Surface; 5] = [
    Surface::NativeField,
    Surface::WebRichText,
    Surface::OrcaComposer,
    Surface::Terminal,
    Surface::ClipboardFallback,
];

struct Field {
    snapshot: Snapshot,
    focused: bool,
    ambiguous: bool,
    clipboard: Option<String>,
    writes: Vec<String>,
}

impl Field {
    fn replace(&mut self, range: TextRange, text: &str) {
        let bytes = byte_range(&self.snapshot.text, range).unwrap();
        self.snapshot.text.replace_range(bytes, text);
        self.snapshot.selection = TextRange {
            location: range.location + text.encode_utf16().count(),
            length: 0,
        };
        self.writes.push(text.into());
    }
}

struct Receiver(Rc<RefCell<Field>>);

impl Receiver {
    fn focused(&self) -> Result<(), Failure> {
        if self.0.borrow().focused {
            Ok(())
        } else {
            Err(Failure::FocusElementChanged)
        }
    }

    fn post(&self, text: &str) -> Result<(), Failure> {
        self.focused()?;
        let mut field = self.0.borrow_mut();
        let selection = field.snapshot.selection;
        field.replace(selection, text);
        // The synthetic receiver applied the event, then lost its transport
        // acknowledgement. The sender must preserve uncertainty and stop.
        if field.ambiguous {
            Err(Failure::Event)
        } else {
            Ok(())
        }
    }
}

impl Target for Receiver {
    fn read(&mut self) -> Result<Snapshot, Failure> {
        self.focused()?;
        Ok(self.0.borrow().snapshot.clone())
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
        self.0.borrow_mut().replace(range, text);
        Ok(())
    }

    fn confirm(&mut self, expected: &Snapshot) -> Result<bool, Failure> {
        if self.0.borrow().ambiguous {
            return Err(Failure::ReadbackTimeout);
        }
        Ok(self.read()? == *expected)
    }
}

impl PasteTarget for Receiver {
    fn check(&mut self) -> Result<(), Failure> {
        self.focused()
    }
    fn prepare(&mut self, text: &str) -> Result<(), Failure> {
        self.0.borrow_mut().clipboard = Some(text.into());
        Ok(())
    }
    fn ready(&mut self) -> Result<bool, Failure> {
        Ok(true)
    }
    fn paste(&mut self) -> Result<(), Failure> {
        let text = self.0.borrow().clipboard.clone().unwrap();
        self.post(&text).map_err(|_| Failure::PasteRequest)
    }
    fn abandon(&mut self) {}
}

impl TerminalTarget for Receiver {
    fn current(&mut self) -> Result<(), Failure> {
        self.focused()
    }
    fn post(&mut self, text: &str) -> Result<(), Failure> {
        Receiver::post(self, text)
    }
    fn end(&mut self) {}
}

impl Surface {
    fn start(self) -> (Delivery, Rc<RefCell<Field>>) {
        let field = Rc::new(RefCell::new(Field {
            snapshot: Snapshot {
                text: BASELINE.into(),
                selection: TextRange {
                    location: 1,
                    length: 0,
                },
            },
            focused: true,
            ambiguous: false,
            clipboard: None,
            writes: Vec::new(),
        }));
        let receiver = Receiver(field.clone());
        let capture = match self {
            Self::NativeField => {
                assert_eq!(
                    native::keyboard_pacing("fixture.native", "AXTextArea", false, false),
                    None
                );
                native::Captured::Ordinary(Box::new(receiver))
            }
            Self::WebRichText => {
                let pacing = native::keyboard_pacing("fixture.web", "AXTextArea", false, true);
                assert_eq!(pacing, Some(KeyboardPacing::Chunked));
                native::Captured::Keyboard(Box::new(receiver), pacing.unwrap())
            }
            Self::OrcaComposer => {
                let pacing =
                    native::keyboard_pacing("com.stablyai.orca", "AXTextArea", false, true);
                assert_eq!(pacing, Some(KeyboardPacing::Scalar));
                native::Captured::Keyboard(Box::new(receiver), pacing.unwrap())
            }
            Self::Terminal => native::Captured::Terminal(Box::new(receiver)),
            Self::ClipboardFallback => native::Captured::ClipboardPaste(Box::new(receiver)),
        };
        let mut delivery = Delivery::new();
        delivery.accept_capture(capture).unwrap();
        assert!(field.borrow().writes.is_empty());
        (delivery, field)
    }
}

fn settle(delivery: &mut Delivery) {
    for _ in 0..10 {
        if !delivery.has_pending() {
            return;
        }
        delivery.poll_captured();
    }
    assert!(!delivery.has_pending(), "unexpected pending test receipt");
}

fn send_duplicates(delivery: &mut Delivery) {
    delivery.deliver_inner("重复最终", true);
    delivery.preview_inner("迟到预览");
    settle(delivery);
}

#[test]
fn compatibility_matrix_caret_emoji_and_exactly_one_logical_final() {
    for surface in SURFACES {
        let (mut delivery, field) = surface.start();
        delivery.deliver_inner(TEXT, true);
        send_duplicates(&mut delivery);
        let receipt = delivery.receipt();
        let writes = field.borrow().writes.clone();
        send_duplicates(&mut delivery);
        assert_eq!(receipt, delivery.receipt(), "{surface:?}");
        assert_eq!(
            field.borrow().writes.as_slice(),
            writes.as_slice(),
            "{surface:?}"
        );
        assert_eq!(field.borrow().snapshot.text, "A中🙂B", "{surface:?}");
        assert_eq!(field.borrow().snapshot.selection.location, 4, "{surface:?}");
        let (state, operations) = match surface {
            Surface::OrcaComposer => (DeliveryState::Confirmed, 2),
            Surface::Terminal | Surface::ClipboardFallback => (DeliveryState::Submitted, 1),
            _ => (DeliveryState::Confirmed, 1),
        };
        assert_eq!(
            (receipt.state, receipt.submitted_operations, writes.len()),
            (state, operations, operations),
            "{surface:?}"
        );
        if state == DeliveryState::Submitted {
            assert_eq!(receipt.confirmed_operations, 0, "{surface:?}");
        }
    }
}

#[test]
fn compatibility_matrix_focus_change_refuses_the_final_and_reentry() {
    for surface in SURFACES {
        let (mut delivery, field) = surface.start();
        field.borrow_mut().focused = false;
        delivery.deliver_inner(TEXT, true);
        settle(&mut delivery);
        assert_eq!(delivery.failure_stage(), Some("focus"), "{surface:?}");
        assert_eq!(
            delivery.receipt().state,
            DeliveryState::NotSent,
            "{surface:?}"
        );
        field.borrow_mut().focused = true;
        send_duplicates(&mut delivery);
        assert_eq!(field.borrow().snapshot.text, BASELINE, "{surface:?}");
        assert!(field.borrow().writes.is_empty(), "{surface:?}");
        assert!(field.borrow().clipboard.is_none(), "{surface:?}");
    }
}

#[test]
fn compatibility_matrix_unknown_write_stops_every_transport_without_retry() {
    for surface in SURFACES {
        let (mut delivery, field) = surface.start();
        field.borrow_mut().ambiguous = true;
        delivery.deliver_inner(TEXT, true);
        settle(&mut delivery);
        let receipt = delivery.receipt();
        assert_eq!(receipt.state, DeliveryState::Uncertain, "{surface:?}");
        assert!(receipt.stopped, "{surface:?}");
        let expected = if matches!(surface, Surface::OrcaComposer) {
            "A中B"
        } else {
            "A中🙂B"
        };
        assert_eq!(field.borrow().snapshot.text, expected, "{surface:?}");
        assert_eq!(field.borrow().writes.len(), 1, "{surface:?}");
        field.borrow_mut().ambiguous = false;
        send_duplicates(&mut delivery);
        assert_eq!(delivery.receipt(), receipt, "{surface:?}");
        assert_eq!(field.borrow().snapshot.text, expected, "{surface:?}");
        assert_eq!(field.borrow().writes.len(), 1, "{surface:?}");
    }
}

#[test]
fn compatibility_matrix_focus_leaving_during_queued_delivery_stops_remaining_chunks() {
    let (mut delivery, field) = Surface::OrcaComposer.start();
    delivery.deliver_inner(TEXT, true);
    assert!(delivery.has_pending());
    // submit sends the first scalar immediately, then waits for readback.
    assert_eq!(field.borrow().snapshot.text, "A中B");
    field.borrow_mut().focused = false;
    settle(&mut delivery);
    assert_eq!(delivery.failure_stage(), Some("focus"));
    assert!(delivery.receipt().stopped);
    field.borrow_mut().focused = true;
    send_duplicates(&mut delivery);
    assert_eq!(field.borrow().writes.as_slice(), ["中"]);
    assert_eq!(field.borrow().snapshot.text, "A中B");
}
