//! Observe physical modifiers without changing the user's key state.
use super::*;

const CONFLICTING: u64 = (1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 23);
const RELEASE_TIMEOUT: Duration = Duration::from_secs(1);

pub(super) fn physical() -> u64 {
    unsafe extern "C" {
        fn CGEventSourceFlagsState(state: i32) -> u64;
    }
    // kCGEventSourceStateHIDSystemState = 1, independent of private CGEvents.
    unsafe { CGEventSourceFlagsState(1) }
}

pub(super) fn held(flags: u64) -> bool {
    flags & CONFLICTING != 0
}

#[derive(Default)]
pub(super) struct ReleaseWait {
    blocked_since: Option<Instant>,
}

impl ReleaseWait {
    pub(super) fn ready(&mut self, now: Instant, flags: u64) -> Result<bool, Failure> {
        // A release observed after the deadline cannot revive this request.
        if self
            .blocked_since
            .is_some_and(|start| now.duration_since(start) >= RELEASE_TIMEOUT)
        {
            return Err(Failure::ModifiersHeld);
        }
        if held(flags) {
            self.blocked_since.get_or_insert(now);
            Ok(false)
        } else {
            self.blocked_since = None;
            Ok(true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_shortcut_flags_are_conflicts_but_locks_and_keypad_are_not() {
        for flag in [1 << 17, 1 << 18, 1 << 19, 1 << 20, 1 << 23] {
            assert!(held(flag));
        }
        assert!(!held(0));
        assert!(!held((1 << 16) | (1 << 21)));
    }

    #[test]
    fn release_wait_is_nonblocking_and_each_observed_release_ends_that_wait() {
        let start = Instant::now();
        let mut wait = ReleaseWait::default();
        assert_eq!(wait.ready(start, 0), Ok(true));
        assert_eq!(wait.ready(start, 1 << 20), Ok(false));
        assert_eq!(
            wait.ready(start + Duration::from_millis(900), 1 << 19),
            Ok(false)
        );
        assert_eq!(wait.ready(start + Duration::from_millis(950), 0), Ok(true));
        assert_eq!(
            wait.ready(start + Duration::from_secs(5), 1 << 17),
            Ok(false)
        );
        assert_eq!(wait.ready(start + Duration::from_millis(5100), 0), Ok(true));
    }

    #[test]
    fn late_release_cannot_restart_a_timed_out_wait() {
        for flags in [0, 1 << 20] {
            let start = Instant::now();
            let mut wait = ReleaseWait::default();
            assert_eq!(wait.ready(start, 1 << 20), Ok(false));
            assert_eq!(
                wait.ready(start + RELEASE_TIMEOUT, flags),
                Err(Failure::ModifiersHeld)
            );
            assert_eq!(
                wait.ready(start + RELEASE_TIMEOUT + Duration::from_secs(1), 0),
                Err(Failure::ModifiersHeld)
            );
        }
    }
}
