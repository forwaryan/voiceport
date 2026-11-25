//! Only the panel-owned AppKit text view is reachable through this target.
use crate::delivery::{
    Delivery,
    model::{DeliveryState, Failure, Snapshot, Target, TextRange, byte_range},
};
use objc2::rc::Retained;
use objc2_app_kit::NSTextView;
use objc2_foundation::{NSRange, NSString, ns_string};
use std::{cell::Cell, rc::Rc};

const SAMPLE: &str = "输入验证🙂";

struct FieldTarget {
    field: Retained<NSTextView>,
    writes: Rc<Cell<usize>>,
}

fn snapshot(field: &NSTextView) -> Snapshot {
    let range = field.selectedRange();
    Snapshot {
        text: field.string().to_string(),
        selection: TextRange {
            location: range.location,
            length: range.length,
        },
    }
}

impl Target for FieldTarget {
    fn read(&mut self) -> Result<Snapshot, Failure> {
        Ok(snapshot(&self.field))
    }

    fn replace(
        &mut self,
        range: TextRange,
        text: &str,
        expected: &Snapshot,
    ) -> Result<(), Failure> {
        if snapshot(&self.field) != *expected {
            return Err(Failure::Ownership);
        }
        byte_range(&expected.text, range).ok_or(Failure::Selection)?;
        self.writes.set(self.writes.get() + 1);
        self.field.replaceCharactersInRange_withString(
            NSRange::new(range.location, range.length),
            &NSString::from_str(text),
        );
        self.field.setSelectedRange(NSRange::new(
            range.location + text.encode_utf16().count(),
            0,
        ));
        Ok(())
    }
}

pub(super) fn reset(field: &NSTextView) {
    field.setString(ns_string!("AB"));
    field.setSelectedRange(NSRange::new(1, 0));
}

pub(super) fn check(field: &Retained<NSTextView>) -> Result<(), &'static str> {
    reset(field);
    let start = snapshot(field);
    if start.text != "AB"
        || start.selection
            != (TextRange {
                location: 1,
                length: 0,
            })
    {
        return Err("无法将测试光标定位于 A 和 B 之间。");
    }

    let mut delivery = Delivery::new();
    let writes = Rc::new(Cell::new(0));
    let bind = |delivery: &mut Delivery| {
        delivery.begin_fixture(Box::new(FieldTarget {
            field: field.clone(),
            writes: writes.clone(),
        }))
    };
    bind(&mut delivery).map_err(|_| "无法绑定专用测试输入框。")?;
    // Move the caret after capture to exercise refusal and final-callback replay.
    field.setSelectedRange(NSRange::new(0, 0));
    let moved = snapshot(field);
    delivery.deliver(SAMPLE, true);
    delivery.deliver(SAMPLE, true);
    if moved.selection.location != 0
        || delivery.failure_stage() != Some("ownership")
        || delivery.receipt().state != DeliveryState::NotSent
        || !delivery.receipt().stopped
        || writes.get() != 0
        || snapshot(field) != moved
    {
        return Err("移动光标后的拦截检查失败。");
    }

    reset(field);
    bind(&mut delivery).map_err(|_| "无法重新绑定专用测试输入框。")?;
    delivery.deliver(SAMPLE, true);
    delivery.deliver(SAMPLE, true);
    let receipt = delivery.receipt();
    let end = snapshot(field);
    if receipt.state != DeliveryState::Confirmed
        || receipt.stopped
        || receipt.submitted_operations != 1
        || receipt.confirmed_operations != 1
        || writes.get() != 1
        || end.text != format!("A{SAMPLE}B")
        || end.selection
            != (TextRange {
                location: 1 + SAMPLE.encode_utf16().count(),
                length: 0,
            })
    {
        return Err("原位输入、光标读回或单次写入检查失败。");
    }
    Ok(())
}
