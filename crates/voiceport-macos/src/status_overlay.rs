//! Content-free projection of the current GUI dictation attempt.
use crate::lifecycle::{Lifecycle, Phase};
use voiceport_control::TranscriptionState;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Stage {
    Preparing,
    Recording,
    Transcribing,
    Delivering,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Snapshot {
    pub generation: u64,
    pub stage: Option<Stage>,
    pub origin_pid: Option<i32>,
    pub level: Option<u8>,
}

impl Snapshot {
    pub(crate) fn project(
        state: &Lifecycle,
        gui_attempt: Option<(u64, TranscriptionState)>,
        pending_final_delivery: bool,
        origin_pid: Option<i32>,
        level: Option<u8>,
    ) -> Self {
        let generation = state.generation();
        use TranscriptionState::{Completed, Pending};
        let stage = match (state.phase, gui_attempt) {
            (_, Some((attempt, _))) if attempt != generation => None,
            (Phase::Permissions, Some((_, Pending))) => Some(Stage::Preparing),
            (Phase::Recording, Some((_, Pending))) => Some(Stage::Recording),
            (Phase::Transcribing, Some((_, Pending))) => Some(Stage::Transcribing),
            (Phase::Delivering, Some((_, Completed))) if pending_final_delivery => {
                Some(Stage::Delivering)
            }
            _ => None,
        };
        Self {
            generation,
            stage,
            origin_pid,
            level: level.filter(|_| stage == Some(Stage::Recording)),
        }
    }
}

#[derive(Default)]
pub(crate) struct Visibility {
    generation: Option<u64>,
    stage: Option<Stage>,
    dismissed: bool,
    closed: bool,
}

impl Visibility {
    /// Ignore observations older than the last generation or phase, and never
    /// resurrect a completed/cancelled/dismissed attempt. Only quit is permanent.
    pub(crate) fn observe(&mut self, snapshot: Snapshot, enabled: bool) -> Option<Stage> {
        if self
            .generation
            .is_some_and(|current| current > snapshot.generation)
        {
            return self.stage;
        }
        if self.generation != Some(snapshot.generation) {
            self.generation = Some(snapshot.generation);
            self.stage = None;
            self.dismissed = false;
        }
        if self.closed || !enabled || snapshot.stage.is_none() {
            self.dismiss();
        } else if !self.dismissed && self.stage <= snapshot.stage {
            self.stage = snapshot.stage;
        }
        self.stage
    }

    pub(crate) fn dismiss(&mut self) {
        self.dismissed = true;
        self.stage = None;
    }

    pub(crate) fn close(&mut self) {
        self.closed = true;
        self.dismiss();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn real_lifecycle_projects_only_the_live_gui_attempt_and_final_queue() {
        use super::{Lifecycle, Phase, Snapshot, Stage, Visibility};
        use voiceport_control::TranscriptionState::{Completed, Empty, Pending};
        let mut lifecycle = Lifecycle::default();
        let old = lifecycle.begin().unwrap();
        let project = |state: &Lifecycle, receipt, pending| {
            Snapshot::project(state, receipt, pending, Some(7), Some(4))
        };
        let gui = Some((old, Pending));
        let mut view = Visibility::default();
        assert_eq!(project(&lifecycle, None, false).stage, None); // CLI/source test
        let preparing = project(&lifecycle, gui, false);
        assert_eq!(preparing.level, None);
        assert_eq!(view.observe(preparing, true), Some(Stage::Preparing));
        assert!(lifecycle.permissions(old, true));
        let recording = project(&lifecycle, gui, true);
        assert_eq!(recording.level, Some(4));
        assert_eq!(view.observe(recording, true), Some(Stage::Recording));
        assert_eq!(view.observe(preparing, true), Some(Stage::Recording));
        assert!(lifecycle.transcribe());
        assert_eq!(
            view.observe(project(&lifecycle, gui, true), true),
            Some(Stage::Transcribing)
        );
        assert!(lifecycle.complete(old, true, true));
        lifecycle.phase = Phase::Delivering; // Controller's final queue, not a receipt of success.
        assert_eq!(project(&lifecycle, Some((old, Empty)), true).stage, None);
        assert_eq!(
            project(&lifecycle, Some((old, Completed)), false).stage,
            None
        );
        let delivering = project(&lifecycle, Some((old, Completed)), true);
        assert_eq!(view.observe(delivering, true), Some(Stage::Delivering));
        lifecycle.phase = Phase::Ready;
        assert_eq!(
            view.observe(project(&lifecycle, Some((old, Completed)), false), true),
            None
        );
        assert_eq!(view.observe(delivering, true), None);
        assert!(lifecycle.begin().is_some());
        assert_eq!(project(&lifecycle, gui, true).stage, None);
    }

    #[test]
    fn cancel_focus_loss_disable_and_quit_cannot_be_reopened_by_old_callbacks() {
        use super::{Lifecycle, Snapshot, Stage, Visibility};
        use voiceport_control::TranscriptionState;

        let mut lifecycle = Lifecycle::default();
        let mut view = Visibility::default();
        let first_gen = lifecycle.begin().unwrap();
        let snapshot = |state: &Lifecycle, attempt| {
            Snapshot::project(
                state,
                Some((attempt, TranscriptionState::Pending)),
                false,
                Some(9),
                None,
            )
        };
        let first = snapshot(&lifecycle, first_gen);
        assert_eq!(view.observe(first, true), Some(Stage::Preparing));
        view.dismiss(); // lost original focus; returning can't reopen this attempt
        assert_eq!(view.observe(first, true), None);
        lifecycle.cancel(false);
        assert!(!lifecycle.permissions(first_gen, true));
        assert_eq!(view.observe(snapshot(&lifecycle, first_gen), true), None);
        assert_eq!(view.observe(first, true), None);
        let second = lifecycle.begin().unwrap();
        let second = snapshot(&lifecycle, second);
        assert_eq!(view.observe(second, false), None);
        assert_eq!(view.observe(second, true), None);
        lifecycle.cancel(false);
        let third = lifecycle.begin().unwrap();
        let third = snapshot(&lifecycle, third);
        assert_eq!(view.observe(third, true), Some(Stage::Preparing));
        assert_eq!(view.observe(first, true), Some(Stage::Preparing));
        view.close();
        lifecycle.cancel(false);
        let fourth = lifecycle.begin().unwrap();
        assert_eq!(view.observe(snapshot(&lifecycle, fourth), true), None);
    }
}
