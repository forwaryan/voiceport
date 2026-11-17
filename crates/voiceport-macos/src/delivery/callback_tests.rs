use super::*;
use std::{cell::Cell, rc::Rc};

struct Receiver(Rc<Cell<usize>>);
impl Target for Receiver {
    fn read(&mut self) -> Result<model::Snapshot, Failure> {
        Ok(model::Snapshot {
            text: String::new(),
            selection: model::TextRange {
                location: 0,
                length: 0,
            },
        })
    }
    fn replace(
        &mut self,
        _: model::TextRange,
        _: &str,
        _: &model::Snapshot,
    ) -> Result<(), Failure> {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
    fn confirm(&mut self, _: &model::Snapshot) -> Result<bool, Failure> {
        Ok(false)
    }
}
impl PasteTarget for Receiver {
    fn check(&mut self) -> Result<(), Failure> {
        Ok(())
    }
    fn prepare(&mut self, _: &str) -> Result<(), Failure> {
        Ok(())
    }
    fn ready(&mut self) -> Result<bool, Failure> {
        Ok(true)
    }
    fn paste(&mut self) -> Result<(), Failure> {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
    fn abandon(&mut self) {}
}
impl TerminalTarget for Receiver {
    fn current(&mut self) -> Result<(), Failure> {
        Ok(())
    }
    fn post(&mut self, _: &str) -> Result<(), Failure> {
        self.0.set(self.0.get() + 1);
        Ok(())
    }
    fn end(&mut self) {}
}

#[test]
fn duplicate_final_and_late_preview_preserve_every_pending_transport() {
    for route in 0..4 {
        let writes = Rc::new(Cell::new(0));
        let receiver = Receiver(writes.clone());
        let captured = match route {
            0 => native::Captured::Ordinary(Box::new(receiver)),
            1 => native::Captured::Keyboard(Box::new(receiver), model::KeyboardPacing::Scalar),
            2 => native::Captured::ClipboardPaste(Box::new(receiver)),
            _ => native::Captured::Terminal(Box::new(receiver)),
        };
        let mut delivery = Delivery::new();
        delivery.accept_capture(captured).unwrap();
        delivery.deliver_inner("中🙂尾", true);
        assert!(delivery.has_pending());
        let receipt = delivery.receipt();
        let count = writes.get();
        delivery.deliver_inner("重复", true);
        delivery.preview_inner(&"迟".repeat(model::MAX_TEXT + 1));
        assert!(delivery.has_pending());
        assert_eq!(delivery.failure_stage(), None);
        assert_eq!(delivery.receipt(), receipt);
        assert_eq!(writes.get(), count);
    }
}

#[test]
fn retaining_terminal_preview_finishes_accepted_queue_despite_late_callbacks() {
    let writes = Rc::new(Cell::new(0));
    let mut delivery = Delivery::new();
    delivery
        .accept_capture(native::Captured::Terminal(Box::new(Receiver(
            writes.clone(),
        ))))
        .unwrap();
    let text = "中".repeat(45);
    delivery.preview_inner(&text);
    delivery.preview_inner(&text);
    assert!(delivery.has_pending());
    delivery.retain_preview_inner();
    delivery.deliver_inner("迟到最终", true);
    delivery.preview_inner("迟到预览");
    assert_eq!(delivery.failure_stage(), None);
    assert!(!delivery.receipt().stopped);
    let Some(Session::Terminal(terminal)) = delivery.session.as_mut() else {
        panic!("accepted terminal queue lost")
    };
    assert!(!terminal.poll().unwrap());
    assert_eq!(
        terminal.receipt().state,
        model::DeliveryState::PartiallySent
    );
    assert_eq!(terminal.receipt().confirmed_operations, 0);
    while !terminal.poll().unwrap() {}
    assert_eq!(writes.get(), 3);
    assert_eq!(terminal.receipt().state, model::DeliveryState::Submitted);
    assert!(!terminal.receipt().stopped);
}

struct Preparing;
impl native::Preparation for Preparing {
    fn poll(&mut self) -> Result<Option<native::Captured>, Failure> {
        panic!("cancelled preparation must not poll")
    }
}
#[test]
fn cancelled_preparation_retains_stopped_receipt_and_rejects_late_results() {
    let mut delivery = Delivery::new();
    delivery
        .accept_capture(native::Captured::Preparing(Box::new(Preparing)))
        .unwrap();
    delivery.deliver_inner("最终", true);
    assert_eq!(delivery.receipt().state, model::DeliveryState::Waiting);
    delivery.clear_inner();
    let receipt = delivery.receipt();
    assert!(receipt.stopped);
    assert_eq!(receipt.submitted_operations, 0);
    assert!(!delivery.has_pending());
    delivery.deliver_inner("迟到", true);
    delivery.preview_inner("迟到");
    assert_eq!(delivery.receipt(), receipt);
    assert!(delivery.session.is_none());
}
