#![cfg(target_os = "macos")]
//! All targets are synthetic. These tests never call Delivery::begin, inspect
//! Accessibility content, install monitors, touch the clipboard or post events.
use std::{cell::RefCell, rc::Rc};
use voiceport_macos::delivery::{
    Delivery,
    model::{
        ChunkedInput, EVENT_TAG, Failure, KeyboardPacing, OwnedSpan, ReplacementFailure, Snapshot,
        StopShortcut, Target, Terminal, TerminalTarget, TextRange, automatic, byte_range,
        invalidates_input, unicode_chunks,
    },
};

fn range(location: usize, length: usize) -> TextRange {
    TextRange { location, length }
}
fn state(text: &str, location: usize, length: usize) -> Snapshot {
    Snapshot {
        text: text.into(),
        selection: range(location, length),
    }
}

struct Field {
    snapshot: Snapshot,
    failure: Option<Failure>,
    writes: usize,
    reject: bool,
    corrupt_after_write: bool,
    change_before_write: bool,
}
#[derive(Clone)]
struct FakeTarget(Rc<RefCell<Field>>);
impl FakeTarget {
    fn new(snapshot: Snapshot) -> Self {
        Self(Rc::new(RefCell::new(Field {
            snapshot,
            failure: None,
            writes: 0,
            reject: false,
            corrupt_after_write: false,
            change_before_write: false,
        })))
    }
    fn snapshot(&self) -> Snapshot {
        self.0.borrow().snapshot.clone()
    }
    fn writes(&self) -> usize {
        self.0.borrow().writes
    }
}
impl Target for FakeTarget {
    fn read(&mut self) -> Result<Snapshot, Failure> {
        let field = self.0.borrow();
        match field.failure {
            Some(failure) => Err(failure),
            None => Ok(field.snapshot.clone()),
        }
    }
    fn replace(
        &mut self,
        span: TextRange,
        text: &str,
        expected: &Snapshot,
    ) -> Result<(), ReplacementFailure> {
        let mut field = self.0.borrow_mut();
        if field.change_before_write {
            field.snapshot.text.push('!');
        }
        if field.snapshot != *expected {
            return Err(ReplacementFailure::BeforeWrite(Failure::Ownership));
        }
        if let Some(failure) = field.failure {
            return Err(ReplacementFailure::BeforeWrite(failure));
        }
        if field.reject {
            return Err(ReplacementFailure::Uncertain(Failure::Write));
        }
        let bytes = byte_range(&field.snapshot.text, span)
            .ok_or(ReplacementFailure::BeforeWrite(Failure::Selection))?;
        field.snapshot.text.replace_range(bytes, text);
        field.snapshot.selection = range(span.location + text.encode_utf16().count(), 0);
        if field.corrupt_after_write {
            field.snapshot.text.push('!');
        }
        field.writes += 1;
        Ok(())
    }
}

#[derive(Clone)]
struct GatedTarget {
    field: FakeTarget,
    ready: Rc<RefCell<Result<bool, Failure>>>,
}

impl GatedTarget {
    fn new() -> Self {
        Self {
            field: FakeTarget::new(state("左|右", 1, 1)),
            ready: Rc::new(RefCell::new(Ok(false))),
        }
    }
}

impl Target for GatedTarget {
    fn read(&mut self) -> Result<Snapshot, Failure> {
        self.field.read()
    }
    fn ready(&mut self) -> Result<bool, Failure> {
        *self.ready.borrow()
    }
    fn replace(
        &mut self,
        span: TextRange,
        text: &str,
        expected: &Snapshot,
    ) -> Result<(), ReplacementFailure> {
        self.field.replace(span, text, expected)
    }
}

#[test]
fn keyboard_wait_preserves_first_and_following_chunks_through_boxed_targets() {
    let fake = GatedTarget::new();
    let mut input = ChunkedInput::capture_with_pacing(
        Box::new(fake.clone()) as Box<dyn Target>,
        KeyboardPacing::Scalar,
    )
    .unwrap();
    input.submit("中🙂尾").unwrap();
    for _ in 0..3 {
        assert!(!input.poll().unwrap());
        assert_eq!(fake.field.writes(), 0);
        assert_eq!(fake.field.snapshot(), state("左|右", 1, 1));
    }
    *fake.ready.borrow_mut() = Ok(true);
    assert!(!input.poll().unwrap());
    assert_eq!(fake.field.snapshot(), state("左中右", 2, 0));
    *fake.ready.borrow_mut() = Ok(false);
    for _ in 0..3 {
        assert!(!input.poll().unwrap());
        assert_eq!(fake.field.writes(), 1);
    }
    *fake.ready.borrow_mut() = Ok(true);
    for _ in 0..3 {
        input.poll().unwrap();
    }
    assert!(!input.has_pending());
    assert_eq!(fake.field.snapshot(), state("左中🙂尾右", 5, 0));
    assert_eq!(fake.field.writes(), 3);
}

#[test]
fn keyboard_wait_still_rejects_target_changes_and_latches_modifier_timeout() {
    for failure in [
        Failure::UserInput,
        Failure::FocusElementChanged,
        Failure::Accessibility,
        Failure::SecureInput,
    ] {
        let fake = GatedTarget::new();
        let mut input = ChunkedInput::capture(fake.clone()).unwrap();
        input.submit("文字").unwrap();
        fake.field.0.borrow_mut().failure = Some(failure);
        assert_eq!(input.poll(), Err(failure));
        fake.field.0.borrow_mut().failure = None;
        *fake.ready.borrow_mut() = Ok(true);
        assert_eq!(input.poll(), Err(failure));
        assert_eq!(fake.field.writes(), 0);
    }
    let fake = GatedTarget::new();
    let mut input = ChunkedInput::capture(fake.clone()).unwrap();
    input.submit("文字").unwrap();
    fake.field.0.borrow_mut().snapshot.selection = range(0, 0);
    assert_eq!(input.poll(), Err(Failure::Ownership));
    assert_eq!(fake.field.writes(), 0);

    let fake = GatedTarget::new();
    let mut input =
        ChunkedInput::capture_with_pacing(fake.clone(), KeyboardPacing::Scalar).unwrap();
    *fake.ready.borrow_mut() = Ok(true);
    input.submit("中🙂尾").unwrap();
    assert_eq!(fake.field.writes(), 1);
    *fake.ready.borrow_mut() = Err(Failure::ModifiersHeld);
    assert_eq!(input.poll(), Err(Failure::ModifiersHeld));
    *fake.ready.borrow_mut() = Ok(true);
    assert_eq!(input.poll(), Err(Failure::ModifiersHeld));
    assert_eq!(input.submit("重投"), Err(Failure::ModifiersHeld));
    assert!(!input.has_pending());
    assert_eq!(fake.field.snapshot(), state("左中右", 2, 0));
    assert_eq!(fake.field.writes(), 1);
}

#[test]
fn cancellation_during_keyboard_wait_discards_only_unsent_chunks() {
    for partial in [false, true] {
        let fake = GatedTarget::new();
        *fake.ready.borrow_mut() = Ok(partial);
        let mut input =
            ChunkedInput::capture_with_pacing(fake.clone(), KeyboardPacing::Scalar).unwrap();
        input.submit("中🙂尾").unwrap();
        *fake.ready.borrow_mut() = Ok(false);
        input.end();
        *fake.ready.borrow_mut() = Ok(true);
        assert!(input.poll().unwrap());
        assert_eq!(fake.field.writes(), usize::from(partial));
        assert_eq!(
            fake.field.snapshot(),
            if partial {
                state("左中右", 2, 0)
            } else {
                state("左|右", 1, 1)
            }
        );
    }
}

struct DeferredField {
    snapshot: Snapshot,
    before: Option<Snapshot>,
    submitted: Option<Snapshot>,
    writes: usize,
    timed_out: bool,
    failure: Option<Failure>,
}

#[derive(Clone)]
struct DeferredTarget(Rc<RefCell<DeferredField>>);

impl DeferredTarget {
    fn new() -> Self {
        Self(Rc::new(RefCell::new(DeferredField {
            snapshot: state("左|右", 1, 0),
            before: None,
            submitted: None,
            writes: 0,
            timed_out: false,
            failure: None,
        })))
    }
    fn settle(&self) {
        let mut field = self.0.borrow_mut();
        field.snapshot = field.submitted.clone().expect("submitted write");
    }
}

impl Target for DeferredTarget {
    fn read(&mut self) -> Result<Snapshot, Failure> {
        let field = self.0.borrow();
        if let Some(failure) = field.failure {
            return Err(failure);
        }
        Ok(field.snapshot.clone())
    }
    fn replace(
        &mut self,
        span: TextRange,
        text: &str,
        expected: &Snapshot,
    ) -> Result<(), ReplacementFailure> {
        let mut field = self.0.borrow_mut();
        assert!(
            field.submitted.is_none(),
            "must confirm before another write"
        );
        if field.snapshot != *expected {
            return Err(ReplacementFailure::BeforeWrite(Failure::Ownership));
        }
        let mut next = expected.clone();
        let bytes = byte_range(&next.text, span)
            .ok_or(ReplacementFailure::BeforeWrite(Failure::Selection))?;
        next.text.replace_range(bytes, text);
        next.selection = range(span.location + text.encode_utf16().count(), 0);
        field.before = Some(expected.clone());
        field.submitted = Some(next);
        field.writes += 1;
        Ok(())
    }
    fn confirm(&mut self, expected: &Snapshot) -> Result<bool, Failure> {
        let snapshot = self.read()?;
        let mut field = self.0.borrow_mut();
        if snapshot == *expected {
            field.submitted = None;
            field.before = None;
            Ok(true)
        } else if field.before.as_ref() != Some(&snapshot) {
            Err(Failure::Ownership)
        } else if field.timed_out {
            Err(Failure::ReadbackTimeout)
        } else {
            Ok(false)
        }
    }
    fn abandon(&mut self) {
        let mut field = self.0.borrow_mut();
        field.submitted = None;
        field.before = None;
    }
}

#[test]
fn delayed_readback_confirms_once_and_coalesces_preview_into_final() {
    let target = DeferredTarget::new();
    // Exercise Box forwarding too: production holds a boxed native target.
    let mut span = OwnedSpan::capture(Box::new(target.clone()) as Box<dyn Target>).unwrap();
    span.update("预览", false).unwrap();
    span.update("过时预览", false).unwrap();
    span.update("最终🙂", true).unwrap();
    assert!(span.has_pending());
    assert_eq!(target.0.borrow().writes, 1);
    for _ in 0..3 {
        assert!(!span.poll().unwrap());
    }
    assert_eq!(target.0.borrow().writes, 1);
    target.settle();
    assert!(!span.poll().unwrap());
    assert_eq!(target.0.borrow().writes, 2);
    target.settle();
    assert!(span.poll().unwrap());
    assert!(!span.has_pending());
    assert_eq!(target.0.borrow().snapshot, state("左最终🙂|右", 5, 0));
}

#[test]
fn delayed_readback_does_not_accept_real_edits_or_later_recovery() {
    for failure in [
        None,
        Some(Failure::FocusApplicationChanged),
        Some(Failure::UserInput),
        Some(Failure::Accessibility),
    ] {
        let target = DeferredTarget::new();
        let mut span = OwnedSpan::capture(target.clone()).unwrap();
        span.update("预览", false).unwrap();
        span.update("最终", true).unwrap();
        let expected_failure = failure.unwrap_or(Failure::Ownership);
        if let Some(failure) = failure {
            target.0.borrow_mut().failure = Some(failure);
        } else {
            target.0.borrow_mut().snapshot = state("用户修改", 4, 0);
        }
        assert_eq!(span.poll(), Err(expected_failure));
        target.0.borrow_mut().failure = None;
        assert!(!span.has_pending());
        assert_eq!(span.update("不应重发", true), Err(expected_failure));
        assert_eq!(target.0.borrow().writes, 1);
    }
}

#[test]
fn delayed_readback_timeout_does_not_retry_an_uncertain_write() {
    let target = DeferredTarget::new();
    let mut span = OwnedSpan::capture(target.clone()).unwrap();
    span.update("结果", true).unwrap();
    target.0.borrow_mut().timed_out = true;
    assert_eq!(span.poll(), Err(Failure::ReadbackTimeout));
    assert!(!span.has_pending());
    assert_eq!(span.update("结果", true), Err(Failure::ReadbackTimeout));
    assert_eq!(target.0.borrow().writes, 1);
}

#[test]
fn delayed_final_result_cannot_be_replaced_by_a_late_partial() {
    let target = DeferredTarget::new();
    let mut span = OwnedSpan::capture(target.clone()).unwrap();
    span.update("最终", true).unwrap();
    span.update("迟到预览", false).unwrap();
    span.update("重复最终", true).unwrap();
    target.settle();
    assert!(span.poll().unwrap());
    assert!(!span.has_pending());
    assert_eq!(target.0.borrow().writes, 1);
    assert_eq!(target.0.borrow().snapshot.text, "左最终|右");
}

#[test]
fn cancellation_discards_queued_text_and_restores_only_confirmed_preview() {
    let target = DeferredTarget::new();
    let mut span = OwnedSpan::capture(target.clone()).unwrap();
    span.update("预览", false).unwrap();
    span.update("排队结果", true).unwrap();
    span.cancel().unwrap();
    assert!(!span.has_pending());
    assert!(span.poll().unwrap());
    assert_eq!(target.0.borrow().writes, 1);

    let target = DeferredTarget::new();
    let mut span = OwnedSpan::capture(target.clone()).unwrap();
    span.update("预览", false).unwrap();
    target.settle();
    assert!(!span.poll().unwrap());
    span.cancel().unwrap();
    span.cancel().unwrap();
    assert!(span.has_pending());
    assert_eq!(target.0.borrow().writes, 2);
    target.settle();
    assert!(span.poll().unwrap());
    assert_eq!(target.0.borrow().snapshot.text, "左|右");
}

#[test]
fn sealing_keeps_last_accepted_preview_until_confirmation() {
    let target = DeferredTarget::new();
    let mut span = OwnedSpan::capture(target.clone()).unwrap();
    span.update("第一版", false).unwrap();
    span.update("最后预览", false).unwrap();
    span.seal();
    target.settle();
    assert!(!span.poll().unwrap());
    target.settle();
    assert!(span.poll().unwrap());
    assert_eq!(target.0.borrow().snapshot.text, "左最后预览|右");
}

#[test]
fn empty_initial_preview_does_not_edit_an_unchanged_field() {
    let target = FakeTarget::new(state("", 0, 0));
    target.0.borrow_mut().reject = true;
    let mut span = OwnedSpan::capture(target.clone()).unwrap();
    span.update("", false).unwrap();
    assert_eq!(target.writes(), 0);
}

#[test]
fn final_keyboard_input_waits_for_each_chunk_and_does_not_repeat_submission() {
    let target = DeferredTarget::new();
    let mut input = ChunkedInput::capture(Box::new(target.clone()) as Box<dyn Target>).unwrap();
    for _ in 0..3 {
        input.verify().unwrap();
    }
    assert_eq!(target.0.borrow().writes, 0);
    let text = "中文🙂".repeat(18);
    input.submit(&text).unwrap();
    assert_eq!(input.submit("late"), Err(Failure::Ended));
    for _ in 0..3 {
        assert!(!input.poll().unwrap());
    }
    assert_eq!(target.0.borrow().writes, 1);
    while input.has_pending() {
        target.settle();
        input.poll().unwrap();
    }
    assert_eq!(target.0.borrow().snapshot.text, format!("左{text}|右"));
    assert_eq!(target.0.borrow().writes, unicode_chunks(&text).len());
}

#[test]
fn scalar_keyboard_pacing_waits_for_chinese_surrogates_and_combining_characters() {
    let target = DeferredTarget::new();
    let mut input =
        ChunkedInput::capture_with_pacing(target.clone(), KeyboardPacing::Scalar).unwrap();
    let text = "中🙂e\u{301}";
    input.submit(text).unwrap();
    let mut accepted = String::new();
    for (index, ch) in text.chars().enumerate() {
        accepted.push(ch);
        assert_eq!(target.0.borrow().writes, index + 1);
        assert_eq!(
            target.0.borrow().submitted.as_ref().unwrap().text,
            format!("左{accepted}|右")
        );
        assert!(!input.poll().unwrap());
        assert_eq!(target.0.borrow().writes, index + 1);
        target.settle();
        input.poll().unwrap();
    }
    assert!(!input.has_pending());
    assert_eq!(
        target.0.borrow().snapshot,
        state(&format!("左{text}|右"), 1 + text.encode_utf16().count(), 0)
    );
}

#[test]
fn final_keyboard_input_rejects_edits_before_sending_any_text() {
    let target = DeferredTarget::new();
    let mut input = ChunkedInput::capture(target.clone()).unwrap();
    target.0.borrow_mut().snapshot = state("用户修改", 4, 0);
    assert_eq!(input.submit("speech"), Err(Failure::Ownership));
    target.0.borrow_mut().snapshot = state("左|右", 1, 0);
    assert_eq!(input.submit("retry"), Err(Failure::Ownership));
    assert_eq!(target.0.borrow().writes, 0);
}

#[test]
fn final_keyboard_input_timeout_and_real_input_stop_every_remaining_chunk() {
    for failure in [
        None,
        Some(Failure::UserInput),
        Some(Failure::FocusElementChanged),
    ] {
        let target = DeferredTarget::new();
        let mut input = ChunkedInput::capture(target.clone()).unwrap();
        input.submit(&"长文本".repeat(20)).unwrap();
        if let Some(failure) = failure {
            target.0.borrow_mut().failure = Some(failure);
        } else {
            target.0.borrow_mut().timed_out = true;
        }
        let expected = failure.unwrap_or(Failure::ReadbackTimeout);
        assert_eq!(input.poll(), Err(expected));
        assert!(!input.has_pending());
        target.0.borrow_mut().failure = None;
        assert_eq!(input.poll(), Err(expected));
        assert_eq!(target.0.borrow().writes, 1);
    }
}

#[test]
fn final_keyboard_cancellation_never_undoes_an_already_posted_chunk() {
    let target = DeferredTarget::new();
    let mut input = ChunkedInput::capture(target.clone()).unwrap();
    input.submit(&"固定文本".repeat(20)).unwrap();
    let posted = target.0.borrow().submitted.clone().unwrap();
    input.end();
    target.0.borrow_mut().snapshot = posted.clone();
    assert!(input.poll().unwrap());
    assert_eq!(target.0.borrow().snapshot, posted);
    assert_eq!(target.0.borrow().writes, 1);
}

#[test]
fn final_keyboard_replaces_only_the_initial_selection_and_sanitizes_controls() {
    let target = FakeTarget::new(state("leftOLDright", 4, 3));
    let mut input = ChunkedInput::capture(target.clone()).unwrap();
    input.submit("你好\n🙂").unwrap();
    assert!(input.poll().unwrap());
    assert_eq!(target.snapshot(), state("left你好 🙂right", 9, 0));
    assert_eq!(target.writes(), 1);
}

#[test]
fn construction_is_inert_and_failure_gate_is_precise() {
    // This is the only production facade call; new/default are allocation-only.
    let delivery = Delivery::new();
    assert!(!delivery.blocks_recording());
    assert_eq!(delivery.failure_stage(), None);
    for failure in [
        Failure::MainThread,
        Failure::Accessibility,
        Failure::SecureInput,
        Failure::Application,
        Failure::Focus,
        Failure::Role,
        Failure::Sensitive,
        Failure::Disabled,
        Failure::Writable,
        Failure::Content,
        Failure::Selection,
        Failure::Ownership,
        Failure::Write,
        Failure::Monitor,
        Failure::UserInput,
        Failure::Revision,
        Failure::Event,
        Failure::Ended,
    ] {
        assert_eq!(
            failure.blocks_recording(),
            matches!(failure, Failure::Accessibility | Failure::Application)
        );
        assert!(
            failure
                .message()
                .starts_with(&format!("[{}]", failure.stage()))
        );
    }
}

#[test]
fn utf16_ranges_reject_surrogate_splits_overflow_and_out_of_bounds() {
    let text = "A😀中Z";
    assert_eq!(byte_range(text, range(1, 2)), Some(1..5));
    assert_eq!(byte_range(text, range(3, 1)), Some(5..8));
    assert_eq!(byte_range(text, range(5, 0)), Some(9..9));
    for invalid in [
        range(2, 0),
        range(1, 1),
        range(2, 1),
        range(5, 1),
        range(6, 0),
        range(usize::MAX, 1),
        range(1, usize::MAX),
    ] {
        assert_eq!(byte_range(text, invalid), None, "{invalid:?}");
        assert!(
            OwnedSpan::capture(FakeTarget::new(Snapshot {
                text: text.into(),
                selection: invalid
            }))
            .is_err()
        );
    }
}

#[test]
fn ordinary_revisions_own_only_original_selection_and_final_does_not_duplicate() {
    let target = FakeTarget::new(state("前旧后", 1, 1));
    let mut span = OwnedSpan::capture(target.clone()).unwrap();
    span.update("你好😀", false).unwrap();
    assert_eq!(target.snapshot(), state("前你好😀后", 5, 0));
    span.update("你好😀", false).unwrap();
    assert_eq!(target.writes(), 1);
    span.update("校正", true).unwrap();
    assert_eq!(target.snapshot(), state("前校正后", 3, 0));
    assert_eq!(span.update("迟到", true), Err(Failure::Ended));
    span.cancel().unwrap();
    assert_eq!(target.snapshot().text, "前校正后");
}

#[test]
fn cancellation_restores_only_owned_unicode_span() {
    let original = "A👨‍👩‍👧‍👦原文Z";
    let target = FakeTarget::new(state(original, 1, "👨‍👩‍👧‍👦原文".encode_utf16().count()));
    let mut span = OwnedSpan::capture(target.clone()).unwrap();
    span.update("中文😀", false).unwrap();
    span.update("识别修订🧑🏽‍💻", false).unwrap();
    span.cancel().unwrap();
    assert_eq!(target.snapshot().text, original);
    let writes = target.writes();
    assert_eq!(span.update("迟到", false), Err(Failure::Ended));
    assert_eq!(target.writes(), writes);
}

#[test]
fn changed_content_or_caret_in_same_element_permanently_stops_owned_span() {
    for kind in 0..4 {
        let target = FakeTarget::new(state("前旧后", 1, 1));
        let mut span = OwnedSpan::capture(target.clone()).unwrap();
        span.update("预览", false).unwrap();
        match kind {
            0 => target.0.borrow_mut().snapshot.text.push('!'),
            1 => target.0.borrow_mut().snapshot.selection = range(0, 0),
            2 => target.0.borrow_mut().snapshot.text.replace_range(..3, "改"),
            _ => target.0.borrow_mut().snapshot.text = "同长替换文".into(),
        }
        let preserved = target.snapshot();
        let writes = target.writes();
        assert_eq!(span.update("最终", true), Err(Failure::Ownership));
        assert_eq!(span.cancel(), Err(Failure::Ownership));
        assert_eq!(target.snapshot(), preserved);
        assert_eq!(target.writes(), writes);
    }
}

#[test]
fn wrong_focus_or_revoked_capability_never_recovers_within_session() {
    for failure in [
        Failure::Accessibility,
        Failure::SecureInput,
        Failure::Application,
        Failure::Focus,
        Failure::Role,
        Failure::Sensitive,
        Failure::Disabled,
        Failure::Writable,
        Failure::Content,
        Failure::Selection,
    ] {
        let target = FakeTarget::new(state("原", 1, 0));
        let mut span = OwnedSpan::capture(target.clone()).unwrap();
        span.update("预览", false).unwrap();
        let preserved = target.snapshot();
        target.0.borrow_mut().failure = Some(failure);
        assert_eq!(span.update("最终", true), Err(failure));
        target.0.borrow_mut().failure = None;
        assert_eq!(span.update("恢复", true), Err(failure));
        assert_eq!(span.cancel(), Err(failure));
        assert_eq!(target.snapshot(), preserved);
        assert_eq!(target.writes(), 1);
    }
}

#[test]
fn initial_capture_records_exact_failure_and_writes_nothing() {
    for failure in [
        Failure::Accessibility,
        Failure::SecureInput,
        Failure::Focus,
        Failure::Role,
        Failure::Sensitive,
        Failure::Disabled,
        Failure::Writable,
        Failure::Content,
        Failure::Selection,
    ] {
        let target = FakeTarget::new(state("", 0, 0));
        target.0.borrow_mut().failure = Some(failure);
        assert!(matches!(OwnedSpan::capture(target.clone()), Err(actual) if actual == failure));
        assert_eq!(target.writes(), 0);
    }
}

#[test]
fn failed_or_partial_write_never_retries_or_cancels_uncertain_text() {
    for partial in [false, true] {
        let target = FakeTarget::new(state("原", 1, 0));
        let mut span = OwnedSpan::capture(target.clone()).unwrap();
        target.0.borrow_mut().reject = !partial;
        target.0.borrow_mut().corrupt_after_write = partial;
        assert!(span.update("预览", false).is_err());
        let preserved = target.snapshot();
        let writes = target.writes();
        target.0.borrow_mut().reject = false;
        target.0.borrow_mut().corrupt_after_write = false;
        assert!(span.update("最终", true).is_err());
        let _ = span.cancel();
        assert_eq!(target.snapshot(), preserved);
        assert_eq!(target.writes(), writes);
    }
}

#[test]
fn boundary_rechecks_immediately_before_replacement() {
    let target = FakeTarget::new(state("原", 1, 0));
    let mut span = OwnedSpan::capture(target.clone()).unwrap();
    target.0.borrow_mut().change_before_write = true;
    assert_eq!(span.update("预览", false), Err(Failure::Ownership));
    assert_eq!(target.snapshot().text, "原!");
    assert_eq!(target.writes(), 0);
}

#[test]
fn sanitizer_matches_character_based_swift_behavior_and_bounds_output() {
    assert_eq!(
        automatic("请保留 requestID\r\n不要发送\n"),
        "请保留 requestID 不要发送 "
    );
    assert_eq!(
        automatic("echo\u{1b}[31m\ttext\0\u{7f}\u{85}\u{2028}\u{2029}"),
        format!("echo [31m text{}", " ".repeat(5))
    );
    assert_eq!(
        automatic("中文 mixedCase，数字 123。👨‍👩‍👧‍👦"),
        "中文 mixedCase，数字 123。 "
    );
    assert_eq!(automatic("A\u{200b}B\u{202e}C\u{feff}"), "A B C ");
    let target = FakeTarget::new(state("", 0, 0));
    let mut span = OwnedSpan::capture(target.clone()).unwrap();
    assert_eq!(
        span.update(&"a".repeat(100_001), false),
        Err(Failure::Content)
    );
    assert_eq!(target.writes(), 0);
}

#[derive(Default)]
struct TerminalState {
    sent: String,
    posts: usize,
    failure: Option<Failure>,
    ended: bool,
    fail_post: bool,
}
#[derive(Clone, Default)]
struct FakeTerminal(Rc<RefCell<TerminalState>>);
impl TerminalTarget for FakeTerminal {
    fn current(&mut self) -> Result<(), Failure> {
        let data = self.0.borrow();
        if let Some(failure) = data.failure {
            return Err(failure);
        }
        if data.ended {
            return Err(Failure::Ended);
        }
        Ok(())
    }
    fn post(&mut self, text: &str) -> Result<(), Failure> {
        self.current()?;
        assert!(text.encode_utf16().count() <= 20);
        let mut data = self.0.borrow_mut();
        if data.fail_post {
            data.sent.push('!');
            return Err(Failure::Event);
        }
        data.sent.push_str(text);
        data.posts += 1;
        Ok(())
    }
    fn end(&mut self) {
        self.0.borrow_mut().ended = true;
    }
}

fn drain(terminal: &mut Terminal<FakeTerminal>) {
    while terminal.has_pending() {
        terminal.poll().unwrap();
    }
}

#[test]
fn terminal_appends_stable_graphemes_without_duplicates_or_rewriting() {
    let target = FakeTerminal::default();
    let mut terminal = Terminal::capture(target.clone()).unwrap();
    terminal.update("你好😀", false).unwrap();
    assert!(target.0.borrow().sent.is_empty());
    terminal.update("你好😀世界", false).unwrap();
    drain(&mut terminal);
    assert_eq!(target.0.borrow().sent, "你好😀");
    terminal.update("你好😀世界。", false).unwrap();
    terminal.update("你好😀世界。", true).unwrap();
    drain(&mut terminal);
    assert_eq!(target.0.borrow().sent, "你好😀世界。");
    assert_eq!(terminal.update("迟到", false), Err(Failure::Ended));
}

#[test]
fn terminal_can_revise_uncommitted_text_but_never_committed_prefix() {
    let target = FakeTerminal::default();
    let mut terminal = Terminal::capture(target.clone()).unwrap();
    terminal.update("你好", false).unwrap();
    terminal.update("您好", false).unwrap();
    terminal.update("您好世界", false).unwrap();
    drain(&mut terminal);
    assert_eq!(target.0.borrow().sent, "您好");
    assert_eq!(terminal.update("其他开始", false), Err(Failure::Revision));
    assert_eq!(terminal.update("您好世界", true), Err(Failure::Revision));
    assert_eq!(target.0.borrow().sent, "您好");
}

#[test]
fn terminal_stable_prefix_never_splits_a_grapheme() {
    let target = FakeTerminal::default();
    let mut terminal = Terminal::capture(target.clone()).unwrap();
    terminal.update("a\u{301}😀", false).unwrap();
    terminal.update("a\u{300}😀", false).unwrap();
    assert!(target.0.borrow().sent.is_empty());
    terminal.update("a\u{300}😀后", false).unwrap();
    drain(&mut terminal);
    assert_eq!(target.0.borrow().sent, "a\u{300}😀");
}

#[test]
fn terminal_stop_flushes_local_words_and_cloud_cannot_rewrite_them() {
    let target = FakeTerminal::default();
    let mut terminal = Terminal::capture(target.clone()).unwrap();
    terminal.update("一次回调\n", false).unwrap();
    terminal.flush().unwrap();
    drain(&mut terminal);
    assert_eq!(target.0.borrow().sent, "一次回调 ");
    assert_eq!(terminal.update("校正后不同", true), Err(Failure::Revision));
    assert_eq!(target.0.borrow().sent, "一次回调 ");
}

#[test]
fn terminal_late_final_can_only_append_exact_prefix() {
    let target = FakeTerminal::default();
    let mut terminal = Terminal::capture(target.clone()).unwrap();
    terminal.update("本地", false).unwrap();
    terminal.flush().unwrap();
    terminal.update("本地补全", true).unwrap();
    drain(&mut terminal);
    assert_eq!(target.0.borrow().sent, "本地补全");
}

#[test]
fn terminal_monitor_failure_wrong_focus_and_user_input_block_late_events() {
    for failure in [
        Failure::Monitor,
        Failure::Focus,
        Failure::UserInput,
        Failure::SecureInput,
        Failure::Disabled,
        Failure::Writable,
    ] {
        let target = FakeTerminal::default();
        target.0.borrow_mut().failure = Some(failure);
        assert!(matches!(Terminal::capture(target.clone()), Err(actual) if actual == failure));
        target.0.borrow_mut().failure = None;
        let mut terminal = Terminal::capture(target.clone()).unwrap();
        terminal.update("已输入", false).unwrap();
        terminal.flush().unwrap();
        drain(&mut terminal);
        target.0.borrow_mut().failure = Some(failure);
        assert_eq!(terminal.update("已输入更多", true), Err(failure));
        target.0.borrow_mut().failure = None;
        assert_eq!(terminal.flush(), Err(failure));
        assert_eq!(target.0.borrow().sent, "已输入");
    }
}

#[test]
fn terminal_end_and_partial_post_failure_never_retract_or_retry() {
    let target = FakeTerminal::default();
    let mut terminal = Terminal::capture(target.clone()).unwrap();
    terminal.update("留下", false).unwrap();
    terminal.flush().unwrap();
    drain(&mut terminal);
    terminal.end();
    assert_eq!(terminal.flush(), Err(Failure::Ended));
    assert_eq!(target.0.borrow().sent, "留下");
    let target = FakeTerminal::default();
    let mut terminal = Terminal::capture(target.clone()).unwrap();
    terminal.update("文本", false).unwrap();
    target.0.borrow_mut().fail_post = true;
    terminal.flush().unwrap();
    assert_eq!(terminal.poll(), Err(Failure::Event));
    target.0.borrow_mut().fail_post = false;
    assert_eq!(terminal.update("文本", true), Err(Failure::Event));
    assert_eq!(target.0.borrow().sent, "!");
}

#[test]
fn terminal_posts_one_chunk_per_turn_and_observes_user_input_between_chunks() {
    let target = FakeTerminal::default();
    let mut terminal = Terminal::capture(target.clone()).unwrap();
    terminal.update(&"😀".repeat(30), true).unwrap();
    assert!(target.0.borrow().sent.is_empty());
    assert_eq!(terminal.poll(), Ok(false));
    assert_eq!(target.0.borrow().sent, "😀".repeat(10));
    assert_eq!(target.0.borrow().posts, 1);
    // Simulates AppKit dispatching a user event in the intervening run-loop turn.
    target.0.borrow_mut().failure = Some(Failure::UserInput);
    assert_eq!(terminal.poll(), Err(Failure::UserInput));
    assert!(!terminal.has_pending());
    assert_eq!(target.0.borrow().posts, 1);
    target.0.borrow_mut().failure = None;
    assert_eq!(terminal.poll(), Err(Failure::UserInput));
}

#[test]
fn terminal_retained_local_queue_finishes_but_cancel_discards_unsent_chunks() {
    let target = FakeTerminal::default();
    let mut terminal = Terminal::capture(target.clone()).unwrap();
    terminal.update(&"a".repeat(45), false).unwrap();
    terminal.flush().unwrap();
    terminal.seal();
    assert_eq!(terminal.update("late", false), Err(Failure::Ended));
    assert!(terminal.has_pending());
    drain(&mut terminal);
    assert_eq!(target.0.borrow().sent, "a".repeat(45));
    assert_eq!(target.0.borrow().posts, 3);
    assert!(target.0.borrow().ended);
    let target = FakeTerminal::default();
    let mut terminal = Terminal::capture(target.clone()).unwrap();
    terminal.update(&"b".repeat(45), true).unwrap();
    terminal.poll().unwrap();
    terminal.end();
    assert_eq!(terminal.poll(), Ok(true));
    assert_eq!(target.0.borrow().sent, "b".repeat(20));
    assert!(!terminal.has_pending());
}

#[test]
fn terminal_revision_discards_queued_text_before_any_post() {
    let target = FakeTerminal::default();
    let mut terminal = Terminal::capture(target.clone()).unwrap();
    terminal.update("原识别", false).unwrap();
    terminal.flush().unwrap();
    assert_eq!(terminal.update("不同结果", true), Err(Failure::Revision));
    assert!(!terminal.has_pending());
    assert!(target.0.borrow().sent.is_empty());
}

#[test]
fn terminal_bounds_are_enforced_before_any_post() {
    let target = FakeTerminal::default();
    let mut terminal = Terminal::capture(target.clone()).unwrap();
    assert_eq!(
        terminal.update(&"😀".repeat(50_001), true),
        Err(Failure::Content)
    );
    assert!(target.0.borrow().sent.is_empty());
}

#[test]
fn unicode_event_chunks_are_at_most_twenty_utf16_units_and_lossless() {
    for text in [
        String::new(),
        "😀".repeat(21),
        "a".repeat(19) + "😀z",
        "👨‍👩‍👧‍👦".repeat(9),
        "a".to_owned() + &"\u{301}".repeat(80),
    ] {
        let chunks = unicode_chunks(&text);
        assert_eq!(chunks.concat(), text);
        for chunk in chunks {
            let units: Vec<u16> = chunk.encode_utf16().collect();
            assert!(!units.is_empty() && units.len() <= 20);
            assert!(String::from_utf16(&units).is_ok());
        }
    }
}

#[test]
fn input_monitor_ignores_only_own_events_and_exact_stop_shortcuts() {
    const SHIFT: u64 = 1 << 17;
    const CONTROL: u64 = 1 << 18;
    const OPTION: u64 = 1 << 19;
    const COMMAND: u64 = 1 << 20;
    let chords = [
        StopShortcut {
            key_code: 49,
            modifiers: CONTROL | OPTION,
        },
        StopShortcut {
            key_code: 49,
            modifiers: CONTROL | SHIFT,
        },
        StopShortcut {
            key_code: 0,
            modifiers: CONTROL | OPTION,
        },
        StopShortcut {
            key_code: 40,
            modifiers: COMMAND | OPTION | SHIFT,
        },
    ];
    for active in chords {
        for chord in chords {
            assert_eq!(
                invalidates_input(0, true, chord.key_code, chord.modifiers, Some(active)),
                chord != active,
                "only the registered chord is exempt: active={active:?}, event={chord:?}"
            );
            assert!(invalidates_input(
                0,
                true,
                chord.key_code,
                chord.modifiers,
                None
            ));
        }
        // Caps Lock and device flags do not change a registered chord.
        assert!(!invalidates_input(
            0,
            true,
            active.key_code,
            active.modifiers | (1 << 16) | 1,
            Some(active)
        ));
        for modifier in [SHIFT, CONTROL, OPTION, COMMAND] {
            assert!(invalidates_input(
                0,
                true,
                active.key_code,
                active.modifiers ^ modifier,
                Some(active)
            ));
        }
        assert!(invalidates_input(
            0,
            true,
            1,
            active.modifiers,
            Some(active)
        ));
        assert!(invalidates_input(0, true, active.key_code, 0, Some(active)));
        assert!(invalidates_input(
            0,
            false,
            active.key_code,
            active.modifiers,
            Some(active)
        ));
        assert!(invalidates_input(0, false, 0, 0, Some(active)));
        assert!(!invalidates_input(EVENT_TAG, true, 0, 0, Some(active)));
        assert!(!invalidates_input(EVENT_TAG, false, 0, 0, Some(active)));
    }
    assert!(!invalidates_input(EVENT_TAG, true, 0, 0, None));
    assert!(invalidates_input(0, true, 0, 0, None));
    assert!(invalidates_input(0, false, 0, 0, None));
}
