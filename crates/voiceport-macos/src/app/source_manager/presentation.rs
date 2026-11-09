//! AppKit completions may arrive after shutdown. A presentation token prevents
//! an old or duplicate completion from reopening admission for a later sheet.
use std::cell::Cell;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Browsing,
    Sheet(u64),
    Closed,
}
#[derive(Default)]
pub(super) struct Presentation {
    phase: Cell<Phase>,
    generation: Cell<u64>,
    writing: Cell<bool>,
}
impl Presentation {
    pub(super) fn browsing(&self) -> bool {
        self.phase.get() == Phase::Browsing && !self.writing.get()
    }
    pub(super) fn closed(&self) -> bool {
        self.phase.get() == Phase::Closed
    }
    pub(super) fn begin(&self) -> Option<u64> {
        if !self.browsing() {
            return None;
        }
        let token = self.generation.get().checked_add(1)?;
        self.generation.set(token);
        self.phase.set(Phase::Sheet(token));
        Some(token)
    }
    pub(super) fn complete(&self, token: u64) -> bool {
        if self.phase.get() != Phase::Sheet(token) {
            return false;
        }
        self.phase.set(Phase::Browsing);
        true
    }
    pub(super) fn writing(&self) -> bool {
        self.writing.get()
    }
    pub(super) fn begin_write(&self) -> bool {
        !self.closed() && !self.writing.replace(true)
    }
    pub(super) fn end_write(&self) {
        self.writing.set(false);
    }
    pub(super) fn close(&self) {
        self.phase.set(Phase::Closed);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sheets_serialize_and_stale_completions_cannot_release_a_new_editor() {
        let session = Presentation::default();
        let first = session.begin().unwrap();
        assert!(session.begin().is_none());
        assert!(session.complete(first));
        let second = session.begin().unwrap();
        assert!(!session.complete(first));
        assert!(!session.browsing());
        assert!(session.complete(second));
        assert!(!session.complete(second));
        assert!(session.browsing());
    }
    #[test]
    fn a_write_serializes_reentrant_saves_and_keeps_shutdown_pending_until_return() {
        let session = Presentation::default();
        let editor = session.begin().unwrap();
        assert!(session.begin_write());
        assert!(!session.begin_write());
        assert!(session.begin().is_none());
        session.close();
        assert!(session.writing());
        assert!(!session.complete(editor));
        assert!(!session.begin_write());
        session.end_write();
        assert!(!session.writing());
        assert!(session.closed());
        assert!(session.begin().is_none());
    }
    #[test]
    fn shutdown_fences_deferred_actions_and_reopen_has_separate_admission() {
        let old = Presentation::default();
        let token = old.begin().unwrap();
        old.close();
        old.close();
        assert!(!old.complete(token));
        assert!(old.closed());
        assert!(old.begin().is_none());
        let reopened = Presentation::default();
        assert!(reopened.browsing());
        assert!(reopened.begin().is_some());
        assert!(old.closed());
    }
}
