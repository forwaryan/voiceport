use voiceport_windows::{
    Recording,
    actions::{Action, Actions, ManualClipboard},
};
struct Sink {
    copies: Vec<String>,
    fail: bool,
}
impl ManualClipboard for Sink {
    fn copy(&mut self, s: &str) -> Result<(), String> {
        if self.fail {
            return Err("synthetic error".into());
        }
        self.copies.push(s.into());
        Ok(())
    }
}
#[test]
fn user_copy_only_after_current_result_and_failed_copy_retains_it() {
    let d = tempfile::tempdir().unwrap();
    let mut ui = Actions::default();
    let mut sink = Sink {
        copies: vec![],
        fail: false,
    };
    for action in [Action::Copy, Action::Start, Action::Stop, Action::Retry] {
        assert!(!ui.act(action, &mut sink));
    }
    assert!(sink.copies.is_empty());
    let path = d.path().join("owned.wav");
    let job = ui
        .admit_finalized(Recording::caller_owned(
            std::fs::File::create_new(&path).unwrap(),
            path,
            Some("zh".into()),
        ))
        .unwrap();
    ui.complete(job, Ok("raw\ntext".into()));
    assert!(ui.can_copy());
    sink.fail = true;
    ui.act(Action::Copy, &mut sink);
    assert!(ui.can_copy());
    sink.fail = false;
    ui.act(Action::Copy, &mut sink);
    assert_eq!(sink.copies, ["raw\ntext"]);
    ui.act(Action::Cancel, &mut sink);
    assert!(!ui.can_copy());
    ui.act(Action::Copy, &mut sink);
    assert_eq!(sink.copies.len(), 1);
}
#[test]
fn exit_keeps_actions_responsive_until_ticket_drop_and_ignores_late_text() {
    let d = tempfile::tempdir().unwrap();
    let mut ui = Actions::default();
    let path = d.path().join("owned.wav");
    let job = ui
        .admit_finalized(Recording::caller_owned(
            std::fs::File::create_new(&path).unwrap(),
            path.clone(),
            None,
        ))
        .unwrap();
    let mut sink = Sink {
        copies: vec![],
        fail: false,
    };
    assert!(!ui.act(Action::Exit, &mut sink));
    assert!(job.cancelled());
    assert!(!ui.act(Action::Poll, &mut sink));
    assert!(!ui.can_copy());
    ui.complete(job, Ok("late text".into()));
    assert!(ui.act(Action::Poll, &mut sink));
    assert!(path.exists());
    assert!(!ui.can_copy());
    assert!(sink.copies.is_empty());
}
#[test]
fn coalesced_exit_and_cancel_override_queued_copy() {
    for control in [3, 6] {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("owned.wav");
        let mut ui = Actions::default();
        let job = ui
            .admit_finalized(Recording::caller_owned(
                std::fs::File::create_new(&path).unwrap(),
                path,
                None,
            ))
            .unwrap();
        ui.complete(job, Ok("private text".into()));
        let mut sink = Sink {
            copies: vec![],
            fail: false,
        };
        ui.dispatch((1 << control) | (1 << 4), &mut sink);
        assert!(sink.copies.is_empty());
        assert!(!ui.can_copy());
    }
}
#[test]
fn ordinary_cancel_poll_recovers_after_completion_or_drop_without_extra_cancel() {
    // Both public polling paths, and both ways of releasing a unique ticket.
    for dispatch in [false, true] {
        for complete in [false, true] {
            let d = tempfile::tempdir().unwrap();
            let mut ui = Actions::default();
            let mut sink = Sink {
                copies: vec![],
                fail: false,
            };
            let recording = |name: &str| {
                let path = d.path().join(name);
                Recording::caller_owned(
                    std::fs::File::create_new(&path).unwrap(),
                    path,
                    Some("zh".into()),
                )
            };
            let poll = |ui: &mut Actions, sink: &mut Sink| {
                if dispatch {
                    ui.dispatch(1 << 7, sink)
                } else {
                    ui.act(Action::Poll, sink)
                }
            };
            for _ in 0..3 {
                assert!(!poll(&mut ui, &mut sink));
            }
            let job = ui.admit_finalized(recording("old.wav")).unwrap();
            assert!(!ui.act(Action::Cancel, &mut sink));
            assert!(!ui.act(Action::Cancel, &mut sink));
            assert!(job.cancelled());
            for n in 0..8 {
                assert!(!poll(&mut ui, &mut sink));
                assert!(matches!(
                    ui.admit_finalized(recording(&format!("blocked-{n}.wav"))),
                    Err(voiceport_windows::Refused::Busy)
                ));
                assert_eq!(ui.status, "Cancelled; waiting for service stop");
            }
            if complete {
                ui.complete(job, Ok("late text must not copy".into()));
            } else {
                drop(job);
            }
            for _ in 0..8 {
                assert!(!poll(&mut ui, &mut sink));
            }
            assert_eq!(ui.status, "Cancelled; service stopped");
            assert!(!ui.can_copy());
            assert!(!ui.act(Action::Copy, &mut sink));
            assert!(sink.copies.is_empty());
            let next = ui.admit_finalized(recording("new.wav")).unwrap();
            ui.complete(next, Ok("current\nraw".into()));
            let status = ui.status.clone();
            for _ in 0..8 {
                assert!(!poll(&mut ui, &mut sink));
            }
            assert!(ui.can_copy());
            assert_eq!(ui.status, status);
            ui.act(Action::Copy, &mut sink);
            assert_eq!(sink.copies, ["current\nraw"]);
            assert!(d.path().join("old.wav").exists());
            let active = ui.admit_finalized(recording("exit.wav")).unwrap();
            for _ in 0..3 {
                assert!(!ui.act(Action::Exit, &mut sink));
                assert!(!poll(&mut ui, &mut sink));
            }
            drop(active);
            for _ in 0..3 {
                assert!(poll(&mut ui, &mut sink));
                assert!(ui.act(Action::Exit, &mut sink));
            }
            assert!(matches!(
                ui.admit_finalized(recording("closed.wav")),
                Err(voiceport_windows::Refused::Closed)
            ));
            assert!(!ui.can_copy());
        }
    }
}
