//! Metadata-only binding of a shortcut to its application at key-down time.
//! No AX contents, windows, permissions, recording or native calls live here.
pub(crate) use crate::delivery::model::ApplicationTarget;
use std::time::Duration;

pub(crate) const START_DEADLINE: Duration = Duration::from_millis(750);

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StartRoute {
    Direct,
    RestoreCapturedApplication,
    Reject(&'static str),
}

pub(crate) fn route(
    own_pid: i32,
    captured: Option<&ApplicationTarget>,
    current: Option<&ApplicationTarget>,
    elapsed: Duration,
) -> StartRoute {
    let Some(target) = captured.filter(|t| t.pid > 0 && !t.bundle.is_empty()) else {
        return StartRoute::Reject("missing_keypress_target");
    };
    if target.pid == own_pid {
        return StartRoute::Reject("self_at_keypress");
    }
    if elapsed >= START_DEADLINE {
        return StartRoute::Reject("expired_keypress");
    }
    match current {
        Some(current) if current == target => StartRoute::Direct,
        Some(current) if current.pid == own_pid => StartRoute::RestoreCapturedApplication,
        _ => StartRoute::Reject("focus_changed_after_keypress"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app(pid: i32, bundle: &str) -> ApplicationTarget {
        ApplicationTarget {
            pid,
            bundle: bundle.into(),
        }
    }

    #[test]
    fn keypress_target_is_not_replaced_by_the_later_frontmost_app() {
        let target = app(12, "test.editor");
        let own = app(1, "test.voiceport");
        assert_eq!(
            route(1, Some(&target), Some(&target), Duration::ZERO),
            StartRoute::Direct
        );
        assert_eq!(
            route(1, Some(&target), Some(&own), Duration::from_millis(40)),
            StartRoute::RestoreCapturedApplication
        );
        assert_eq!(
            route(
                1,
                Some(&target),
                Some(&app(24, "test.other")),
                Duration::ZERO
            ),
            StartRoute::Reject("focus_changed_after_keypress")
        );
    }

    #[test]
    fn missing_self_stale_and_pid_reuse_fail_without_guessing_a_target() {
        let target = app(12, "test.editor");
        assert_eq!(
            route(1, None, Some(&target), Duration::ZERO),
            StartRoute::Reject("missing_keypress_target")
        );
        assert_eq!(
            route(
                1,
                Some(&app(0, "test.editor")),
                Some(&target),
                Duration::ZERO
            ),
            StartRoute::Reject("missing_keypress_target")
        );
        assert_eq!(
            route(
                1,
                Some(&app(1, "test.voiceport")),
                Some(&target),
                Duration::ZERO
            ),
            StartRoute::Reject("self_at_keypress")
        );
        assert_eq!(
            route(1, Some(&target), Some(&target), START_DEADLINE),
            StartRoute::Reject("expired_keypress")
        );
        assert_eq!(
            route(
                1,
                Some(&target),
                Some(&app(12, "test.reused")),
                Duration::ZERO
            ),
            StartRoute::Reject("focus_changed_after_keypress")
        );
        assert_eq!(
            route(1, Some(&target), None, Duration::ZERO),
            StartRoute::Reject("focus_changed_after_keypress")
        );
    }
}
