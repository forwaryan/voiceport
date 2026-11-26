//! In-memory completion source and owned selection for the offscreen panel fixture.
use super::super::*;
use crate::delivery::model::{Failure as InputFailure, Snapshot, Target, TextRange, byte_range};
use std::{cell::RefCell, rc::Rc};

pub(crate) struct SyntheticDictations {
    pub(crate) controller: Controller,
    completions: Sender<Completion>,
}

pub(crate) struct ReceiverState {
    pub(crate) snapshot: Snapshot,
    pub(crate) writes: usize,
    pub(crate) confirm: bool,
    pub(crate) failure: Option<InputFailure>,
}

struct SyntheticTarget(Rc<RefCell<ReceiverState>>);

impl Target for SyntheticTarget {
    fn read(&mut self) -> Result<Snapshot, InputFailure> {
        let target = self.0.borrow();
        if let Some(failure) = target.failure {
            return Err(failure);
        }
        Ok(target.snapshot.clone())
    }

    fn replace(
        &mut self,
        range: TextRange,
        text: &str,
        expected: &Snapshot,
    ) -> Result<(), InputFailure> {
        assert_eq!(&self.read()?, expected);
        let mut target = self.0.borrow_mut();
        let bytes = byte_range(&target.snapshot.text, range).unwrap();
        target.snapshot.text.replace_range(bytes, text);
        target.snapshot.selection = TextRange {
            location: range.location + text.encode_utf16().count(),
            length: 0,
        };
        target.writes += 1;
        Ok(())
    }

    fn confirm(&mut self, expected: &Snapshot) -> Result<bool, InputFailure> {
        if !self.0.borrow().confirm {
            return Ok(false);
        }
        if &self.read()? == expected {
            Ok(true)
        } else {
            Err(InputFailure::Ownership)
        }
    }
}

impl SyntheticDictations {
    pub(crate) fn new() -> Self {
        let mut controller = Controller::new();
        let (completions, results) = mpsc::channel();
        let (jobs, _) = mpsc::sync_channel(1);
        controller.worker = Some(Worker { jobs, results });
        controller.recent_dictations.fixture_focus_safe = Some(true);
        Self {
            controller,
            completions,
        }
    }

    pub(crate) fn set_focus_safe(&mut self, safe: bool) {
        self.controller.recent_dictations.fixture_focus_safe = Some(safe);
    }

    pub(crate) fn begin(
        &mut self,
        gui: bool,
        automatic: bool,
    ) -> (u64, Rc<RefCell<ReceiverState>>) {
        let receiver = Rc::new(RefCell::new(ReceiverState {
            snapshot: Snapshot {
                text: "A|B".into(),
                selection: TextRange {
                    location: 1,
                    length: 1,
                },
            },
            writes: 0,
            confirm: true,
            failure: None,
        }));
        let controller = &mut self.controller;
        let (generation, _) = controller
            .prepare_start_with(
                SessionSource::ChatGpt,
                "zh".into(),
                automatic,
                gui,
                |delivery| delivery.begin_fixture(Box::new(SyntheticTarget(receiver.clone()))),
            )
            .unwrap();
        controller
            .events_tx
            .send((
                generation,
                AudioEvent::Permissions {
                    microphone: true,
                    speech: false,
                },
            ))
            .unwrap();
        controller.poll_with_capture(|_| true, |_, _, _, _| Ok(false));
        // Move the synthetic capture to finalization without opening a device or an audio file.
        assert!(controller.state.transcribe());
        controller.active = Some(ActiveJob {
            generation,
            cancel: CancellationToken::new(),
            custody: Custody::default(),
            _recording: None,
        });
        (generation, receiver)
    }

    pub(crate) fn finish(&mut self, generation: u64, text: &str) {
        self.completions
            .send(Completion {
                generation,
                elapsed: None,
                result: Ok(text.to_owned()),
            })
            .unwrap();
        self.poll();
    }

    pub(crate) fn finish_at(&mut self, generation: u64, text: &str, timestamp_ms: u64) {
        self.finish(generation, text);
        let entry = self
            .controller
            .recent_dictations
            .entries
            .front_mut()
            .unwrap();
        assert_eq!(entry.generation, generation);
        entry.metadata.created_at_unix_ms = timestamp_ms;
    }

    pub(crate) fn poll(&mut self) {
        self.controller
            .poll_with_capture(|_| true, |_, _, _, _| panic!("unexpected capture"));
    }
}
