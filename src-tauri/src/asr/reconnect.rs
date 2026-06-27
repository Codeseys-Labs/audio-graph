//! Shared reconnect ladder helpers for streaming ASR WebSocket providers.

/// Default reconnect backoff schedule: 1s, 2s, 5s, 10s, then give up.
pub(super) const DEFAULT_BACKOFF_SECONDS: [u64; 4] = [1, 2, 5, 10];

/// One step of a reconnect ladder, computed from the number of reconnect
/// attempts already made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ReconnectStep {
    /// Try a reconnect as 1-based `attempt` after sleeping `backoff_secs`.
    Retry { attempt: u32, backoff_secs: u64 },
    /// The backoff schedule is exhausted after `attempted` failed attempts.
    GiveUp { attempted: u32 },
}

/// Return the default backoff duration for a 1-based reconnect attempt.
pub(super) fn backoff_for_attempt(attempt: u32) -> Option<u64> {
    let index = attempt.checked_sub(1)? as usize;
    DEFAULT_BACKOFF_SECONDS.get(index).copied()
}

/// Advance the reconnect ladder by exactly one attempt.
pub(super) fn next_reconnect_step(prior_attempts: u32) -> ReconnectStep {
    let attempt = prior_attempts + 1;
    match backoff_for_attempt(attempt) {
        Some(backoff_secs) => ReconnectStep::Retry {
            attempt,
            backoff_secs,
        },
        None => ReconnectStep::GiveUp {
            attempted: prior_attempts,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_backoff_schedule_matches_provider_contract() {
        assert_eq!(backoff_for_attempt(0), None);
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(2), Some(2));
        assert_eq!(backoff_for_attempt(3), Some(5));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), None);
        assert_eq!(backoff_for_attempt(99), None);
    }

    #[test]
    fn next_step_advances_once_and_reports_attempted_budget() {
        assert_eq!(
            next_reconnect_step(0),
            ReconnectStep::Retry {
                attempt: 1,
                backoff_secs: 1
            }
        );
        assert_eq!(
            next_reconnect_step(1),
            ReconnectStep::Retry {
                attempt: 2,
                backoff_secs: 2
            }
        );
        assert_eq!(
            next_reconnect_step(2),
            ReconnectStep::Retry {
                attempt: 3,
                backoff_secs: 5
            }
        );
        assert_eq!(
            next_reconnect_step(3),
            ReconnectStep::Retry {
                attempt: 4,
                backoff_secs: 10
            }
        );
        assert_eq!(
            next_reconnect_step(4),
            ReconnectStep::GiveUp { attempted: 4 }
        );
    }
}
