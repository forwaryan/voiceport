//! Real CLI and AppKit actions with a synthetic receiver and clipboard.
use super::*;
use crate::delivery::model::{
    ApplicationTarget, Failure, ReplacementFailure, Snapshot, Target, TextRange, byte_range,
};
use std::{cell::RefCell, rc::Rc};

const TEXT: &str = "Synthetic final 聽寫🙂";
const BUNDLE: &str = "io.github.forwaryan.voiceport.synthetic-editor";
const PASTE: &[&str] = &[
    "dictation",
    "paste-last",
    "--target-pid",
    "202",
    "--target-bundle",
    BUNDLE,
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    OldTarget,
    Ready,
    Secure,
    Unknown,
    MovedCaret,
    MovedCaretAtWrite,
    PendingConfirmation,
    UncertainWrite,
}

struct Editor {
    mode: Mode,
    reads: usize,
    writes: usize,
    snapshot: Snapshot,
}

struct Receiver(Rc<RefCell<Editor>>);

impl Target for Receiver {
    fn read(&mut self) -> Result<Snapshot, Failure> {
        let mut editor = self.0.borrow_mut();
        match editor.mode {
            Mode::OldTarget => return Err(Failure::FocusApplicationChanged),
            Mode::Secure => return Err(Failure::Sensitive),
            Mode::Unknown => return Err(Failure::Focus),
            _ => {}
        }
        editor.reads += 1;
        if editor.mode == Mode::MovedCaret && editor.reads == 2 {
            editor.snapshot.selection = TextRange {
                location: 0,
                length: 0,
            };
        }
        Ok(editor.snapshot.clone())
    }

    fn replace(
        &mut self,
        range: TextRange,
        text: &str,
        expected: &Snapshot,
    ) -> Result<(), ReplacementFailure> {
        if self.0.borrow().mode == Mode::MovedCaretAtWrite {
            self.0.borrow_mut().snapshot.selection = TextRange {
                location: 0,
                length: 0,
            };
        }
        if self.read().map_err(ReplacementFailure::BeforeWrite)? != *expected {
            return Err(ReplacementFailure::BeforeWrite(Failure::Ownership));
        }
        let mut editor = self.0.borrow_mut();
        let bytes = byte_range(&editor.snapshot.text, range).unwrap();
        editor.snapshot.text.replace_range(bytes, text);
        editor.snapshot.selection = TextRange {
            location: range.location + text.encode_utf16().count(),
            length: 0,
        };
        editor.writes += 1;
        if editor.mode == Mode::UncertainWrite {
            return Err(ReplacementFailure::Uncertain(Failure::Write));
        }
        Ok(())
    }

    fn confirm(&mut self, expected: &Snapshot) -> Result<bool, Failure> {
        if self.0.borrow().mode == Mode::PendingConfirmation {
            return Ok(false);
        }
        Ok(&self.read()? == expected)
    }
}

pub(super) fn verify(fixture: &Fixture<'_>, rig: &mut SyntheticDictations) {
    let clipboard = Rc::new(RefCell::new(String::new()));
    let initial = Snapshot {
        text: "A|B".into(),
        selection: TextRange {
            location: 1,
            length: 1,
        },
    };
    let editor = Rc::new(RefCell::new(Editor {
        mode: Mode::Ready,
        reads: 0,
        writes: 0,
        snapshot: initial.clone(),
    }));
    let captures = Rc::new(Cell::new(0));
    let recorder = super::super::super::latest_output::Fixture {
        copy: Box::new({
            let clipboard = clipboard.clone();
            move |text| {
                clipboard.replace(text.to_owned());
                true
            }
        }),
        capture: Box::new({
            let editor = editor.clone();
            let captures = captures.clone();
            move |target, delivery| {
                captures.set(captures.get() + 1);
                let expected = ApplicationTarget {
                    pid: 202,
                    bundle: BUNDLE.into(),
                };
                let mut state = editor.borrow_mut();
                state.reads = 0;
                if target != &expected {
                    state.mode = Mode::OldTarget;
                }
                drop(state);
                delivery.begin_fixture(Box::new(Receiver(editor.clone())))
            }
        }),
    };
    assert!(
        fixture
            .owner
            .ivars()
            .latest_output_fixture
            .set(recorder)
            .is_ok()
    );
    let call = |words: &[&str], exit| fixture.call(words, exit);
    assert_eq!(
        call(&["dictation", "copy-last"], 6)["error"]["code"],
        "not_found"
    );
    assert_eq!(call(PASTE, 6)["error"]["code"], "not_found");
    assert_eq!(captures.get(), 0);
    assert_eq!(fixture.finish_gui(rig, TEXT), 1);
    assert_eq!(
        call(&["history", "list"], 0)["data"],
        json!({"enabled":false,"entries":[]})
    );

    // The native menu takes the exact final text from the same result as CLI.
    fixture.owner.copy_action(sel!(copyText:), None);
    assert_eq!(clipboard.borrow().as_str(), TEXT);
    clipboard.borrow_mut().clear();
    let copied = call(&["dictation", "copy-last"], 0);
    assert_eq!(copied["data"]["copied"], true);
    assert!(!copied.to_string().contains(TEXT));
    assert_eq!(clipboard.borrow().as_str(), TEXT);
    let original = call(&["status"], 0)["data"]["input"]["gui_receipt"].clone();

    let old_target = [
        "dictation",
        "paste-last",
        "--target-pid",
        "101",
        "--target-bundle",
        BUNDLE,
    ];
    assert_eq!(call(&old_target, 1)["data"]["failure_stage"], "focus");
    for (mode, stage) in [
        (Mode::Secure, "sensitive"),
        (Mode::Unknown, "focus"),
        (Mode::MovedCaret, "ownership"),
    ] {
        editor.borrow_mut().mode = mode;
        let rejected = call(PASTE, 1);
        assert_eq!(rejected["error"]["code"], "paste_blocked");
        assert_eq!(rejected["data"]["failure_stage"], stage);
        assert_eq!(rejected["data"]["delivery"]["submittedOperations"], 0);
        assert_eq!(editor.borrow().writes, 0);
        assert_eq!(clipboard.borrow().as_str(), TEXT);
    }
    editor.borrow_mut().snapshot = initial.clone();
    editor.borrow_mut().mode = Mode::MovedCaretAtWrite;
    let rejected = call(PASTE, 1);
    assert_eq!(rejected["error"]["code"], "paste_blocked");
    assert_eq!(rejected["data"]["failure_stage"], "ownership");
    assert_eq!(
        rejected["data"]["delivery"]["state"], "notSent",
        "repaste fixture: final guard rejected before any write"
    );
    assert_eq!(rejected["data"]["delivery"]["submittedOperations"], 0);
    assert_eq!(rejected["data"]["delivery"]["confirmedOperations"], 0);
    assert_eq!(editor.borrow().snapshot.text, initial.text);
    assert_eq!(editor.borrow().snapshot.selection.location, 0);
    assert_eq!(clipboard.borrow().as_str(), TEXT);
    let captured = captures.get();
    editor.borrow_mut().snapshot = initial.clone();
    editor.borrow_mut().mode = Mode::Ready;
    fixture
        .owner
        .with_controller(|c| fixture.owner.poll_dictation(c));
    assert_eq!(captures.get(), captured);
    assert_eq!(editor.borrow().writes, 0);

    let pasted = call(PASTE, 0);
    assert_eq!(captures.get(), captured + 1);
    assert_eq!(pasted["data"]["generation"], rejected["data"]["generation"]);
    assert_eq!(pasted["data"]["delivery"]["state"], "confirmed");
    assert_eq!(pasted["data"]["delivery"]["submittedOperations"], 1);
    assert_eq!(editor.borrow().snapshot.text, format!("A{TEXT}B"));
    let captured = captures.get();
    assert_eq!(call(PASTE, 1)["error"]["code"], "possible_duplicate");
    assert_eq!(captures.get(), captured);
    assert_eq!(editor.borrow().writes, 1);
    assert_eq!(
        call(&["dictation", "copy-last"], 0)["data"]["possible_duplicate"],
        true
    );
    let status = call(&["status"], 0);
    assert_eq!(status["data"]["input"]["gui_receipt"], original);
    assert_eq!(status["data"]["input"]["repaste"], pasted["data"]);
    assert!(!status.to_string().contains(TEXT));

    // Use the actual global shortcut handler and do not redirect a later poll.
    assert_eq!(fixture.finish_gui(rig, "Another final"), 1);
    editor.borrow_mut().snapshot = initial.clone();
    editor.borrow_mut().mode = Mode::PendingConfirmation;
    fixture
        .owner
        .handle_repaste_shortcut(crate::hotkey::KeyPress {
            stroke: crate::hotkey::Stroke::fixture_stroke(1),
            at: Instant::now(),
            application: None,
            target: Some(ApplicationTarget {
                pid: 202,
                bundle: BUNDLE.into(),
            }),
        });
    assert_eq!(editor.borrow().writes, 2);
    assert_eq!(call(PASTE, 4)["error"]["code"], "busy");
    editor.borrow_mut().mode = Mode::Unknown;
    fixture
        .owner
        .with_controller(|c| fixture.owner.poll_dictation(c));
    assert_eq!(
        call(&["status"], 0)["data"]["input"]["repaste"]["delivery"]["state"],
        "uncertain"
    );
    assert_eq!(call(PASTE, 1)["error"]["code"], "possible_duplicate");
    assert_eq!(editor.borrow().writes, 2);

    // A previous automatic write can be unconfirmed before any repaste action.
    fixture.owner.with_controller(|controller| {
        std::mem::swap(controller, &mut rig.controller);
        let (generation, original_target) = rig.begin(true, true);
        original_target.borrow_mut().confirm = false;
        rig.finish(generation, "Possibly written");
        assert_eq!(original_target.borrow().writes, 1);
        original_target.borrow_mut().failure = Some(Failure::FocusElementChanged);
        original_target.borrow_mut().confirm = true;
        rig.poll();
        std::mem::swap(controller, &mut rig.controller);
    });
    assert_eq!(
        call(&["status"], 0)["data"]["input"]["gui_receipt"]["delivery"]["state"],
        "uncertain"
    );
    assert_eq!(call(PASTE, 1)["error"]["code"], "possible_duplicate");
    assert_eq!(
        call(&["dictation", "copy-last"], 0)["data"]["possible_duplicate"],
        true
    );
    assert_eq!(clipboard.borrow().as_str(), "Possibly written");
    assert_eq!(editor.borrow().writes, 2);

    // Confirmed preview writes from a failed ASR do not confirm the retry's
    // final text. The original preview still makes another paste ambiguous.
    assert_eq!(
        call(&["history", "settings", "on"], 0)["data"]["enabled"],
        true
    );
    editor.borrow_mut().snapshot = Snapshot {
        text: "left|right".into(),
        selection: TextRange {
            location: 4,
            length: 1,
        },
    };
    editor.borrow_mut().mode = Mode::Ready;
    fixture.owner.with_controller(|controller| {
        std::mem::swap(controller, &mut rig.controller);
        rig.retry_after_confirmed_preview("Final from retry");
        std::mem::swap(controller, &mut rig.controller);
    });
    let retry = &call(&["status"], 0)["data"]["input"]["gui_receipt"];
    assert_eq!(retry["block_reason"], "retry_panel_only");
    assert_eq!(retry["delivery"]["state"], "confirmed");
    assert_eq!(retry["delivery"]["confirmedOperations"], 1);
    let history = call(&["history", "list"], 0);
    assert_eq!(
        history["data"]["entries"][0]["delivery"]["state"],
        "notSent"
    );
    eprintln!("repaste fixture: confirmed preview retry duplicate guard");
    let captured = captures.get();
    assert_eq!(call(PASTE, 1)["error"]["code"], "possible_duplicate");
    assert_eq!(captures.get(), captured);
    assert_eq!(
        call(&["dictation", "copy-last"], 0)["data"]["possible_duplicate"],
        true
    );
    assert_eq!(clipboard.borrow().as_str(), "Final from retry");
    assert_eq!(editor.borrow().writes, 2);

    fixture.owner.with_controller(|controller| {
        std::mem::swap(controller, &mut rig.controller);
        rig.set_focus_safe(false);
        let (generation, _) = rig.begin(true, false);
        rig.finish(generation, "Ineligible final");
        rig.set_focus_safe(true);
        std::mem::swap(controller, &mut rig.controller);
    });
    assert_eq!(call(PASTE, 1)["error"]["code"], "possible_duplicate");
    assert_eq!(captures.get(), captured);
    assert_eq!(
        call(&["dictation", "copy-last"], 0)["data"]["possible_duplicate"],
        true
    );
    assert_eq!(clipboard.borrow().as_str(), "Final from retry");
    call(&["history", "settings", "off", "--confirm"], 0);
    assert_eq!(
        call(&["dictation", "copy-last"], 6)["error"]["code"],
        "not_found"
    );
    // A setter error can leave text even when it reports no successful submission.
    let uncertain_text = "Final with uncertain write";
    assert_eq!(fixture.finish_gui(rig, uncertain_text), 1);
    editor.borrow_mut().snapshot = initial;
    editor.borrow_mut().mode = Mode::UncertainWrite;
    let writes = editor.borrow().writes;
    let uncertain = call(PASTE, 1);
    assert_eq!(uncertain["error"]["code"], "paste_blocked");
    assert_eq!(uncertain["data"]["delivery"]["state"], "uncertain");
    assert_eq!(uncertain["data"]["delivery"]["submittedOperations"], 0);
    assert_eq!(uncertain["data"]["delivery"]["confirmedOperations"], 0);
    assert_eq!(editor.borrow().snapshot.text, format!("A{uncertain_text}B"));
    assert_eq!(clipboard.borrow().as_str(), "Final from retry");
    let captured = captures.get();
    editor.borrow_mut().mode = Mode::Ready;
    fixture
        .owner
        .with_controller(|c| fixture.owner.poll_dictation(c));
    let duplicate = call(PASTE, 1);
    assert_eq!(duplicate["error"]["code"], "possible_duplicate");
    assert_eq!(
        duplicate["data"]["generation"],
        uncertain["data"]["generation"]
    );
    assert_eq!(captures.get(), captured);
    assert_eq!(editor.borrow().writes, writes + 1);
    assert_eq!(
        call(&["dictation", "copy-last"], 0)["data"]["possible_duplicate"],
        true
    );
    assert_eq!(clipboard.borrow().as_str(), uncertain_text);
    println!(
        "PASS actual CLI and AppKit action handlers: copy, confirmed repaste, stale target, secure/unknown focus, changed caret, and duplicate/uncertain receipts; synthetic boundaries"
    );
}
