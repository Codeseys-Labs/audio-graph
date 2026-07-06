//! Shared reconnect ladder helpers for every streaming WebSocket provider.
//!
//! Previously each transport carried its own copy of the 1/2/5/10s backoff
//! schedule: the ASR clients shared this module, while the Gemini Live S2S
//! client and the Deepgram Aura TTS client hand-rolled byte-identical
//! `backoff_for_attempt` functions of their own (review n2). Consolidating them
//! here — and offering jitter as an option rather than a second copy — removes
//! the risk of the schedules silently diverging between transports.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Default reconnect backoff schedule (seconds), then give up.
///
/// COLD-RESTART TAIL (review m1) — the ladder keeps its original fast head
/// (1s, 2s, 5s, 10s) so a common transient blip recovers exactly as before, then
/// adds a slower **cold-restart tail** (20s, 30s, then 60s rungs) so a long-lived
/// capture can ride out a multi-minute partition (captive portal, roaming
/// handoff, VPN reconnect) instead of giving up permanently after only ~18s and
/// killing transcription for the whole recording. Total budget is ~5 minutes
/// across 10 attempts, after which the session still surfaces a fatal error
/// rather than looping forever against a genuinely dead provider.
///
/// The ladder is *resettable*, not just longer: every provider resets
/// `reconnect_attempts` to 0 after a successful reconnect (Deepgram/AssemblyAI/
/// Soniox/OpenAI-realtime/Gemini/Aura), and AWS resets only after the stream
/// stays healthy past `HEALTHY_STREAM_RESET_SECS` — so a capture that recovers
/// mid-way gets a fresh full budget for the next outage. A flapping link that
/// never sustains still climbs to give-up (it cannot loop at attempt 1).
pub(crate) const DEFAULT_BACKOFF_SECONDS: [u64; 10] = [1, 2, 5, 10, 20, 30, 60, 60, 60, 60];

/// One step of a reconnect ladder, computed from the number of reconnect
/// attempts already made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReconnectStep {
    /// Try a reconnect as 1-based `attempt` after sleeping `backoff_secs`.
    Retry { attempt: u32, backoff_secs: u64 },
    /// The backoff schedule is exhausted after `attempted` failed attempts.
    GiveUp { attempted: u32 },
}

/// Return the default backoff duration for a 1-based reconnect attempt.
pub(crate) fn backoff_for_attempt(attempt: u32) -> Option<u64> {
    let index = attempt.checked_sub(1)? as usize;
    DEFAULT_BACKOFF_SECONDS.get(index).copied()
}

/// Advance the reconnect ladder by exactly one attempt.
pub(crate) fn next_reconnect_step(prior_attempts: u32) -> ReconnectStep {
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

/// Apply plus-or-minus 20% jitter to a backoff value in seconds, returning the
/// jittered duration.
///
/// Uses a low-quality clock-derived pseudo-random multiplier — we only need
/// enough variance to de-synchronize concurrent reconnects across clients (so a
/// shared outage doesn't produce a synchronized reconnect thundering herd), not
/// crypto-quality randomness. Jitter is opt-in: the ASR ladder sleeps the raw
/// `backoff_secs`, while the Aura TTS client wraps it through here.
pub(crate) fn jittered_backoff(base_secs: u64) -> Duration {
    if base_secs == 0 {
        return Duration::ZERO;
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    // Map nanos in [0, 1_000_000_000) to a multiplier in [0.8, 1.2].
    let frac = (nanos as f64) / 1_000_000_000_f64;
    let multiplier = 0.8 + 0.4 * frac;
    let scaled = (base_secs as f64) * multiplier;
    let millis = (scaled * 1000.0).round().max(1.0) as u64;
    Duration::from_millis(millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_backoff_schedule_matches_provider_contract() {
        // Fast head (unchanged) then the cold-restart tail (review m1).
        assert_eq!(backoff_for_attempt(0), None);
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(2), Some(2));
        assert_eq!(backoff_for_attempt(3), Some(5));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), Some(20));
        assert_eq!(backoff_for_attempt(6), Some(30));
        assert_eq!(backoff_for_attempt(7), Some(60));
        assert_eq!(backoff_for_attempt(8), Some(60));
        assert_eq!(backoff_for_attempt(9), Some(60));
        assert_eq!(backoff_for_attempt(10), Some(60));
        assert_eq!(backoff_for_attempt(11), None);
        assert_eq!(backoff_for_attempt(99), None);
    }

    #[test]
    fn total_reconnect_budget_rides_out_multi_minute_partition() {
        // The whole ladder should sum to a few minutes so a long capture can
        // survive a real partition, not just an ~18s blip (review m1).
        let total: u64 = (1..=DEFAULT_BACKOFF_SECONDS.len() as u32)
            .filter_map(backoff_for_attempt)
            .sum();
        assert_eq!(
            total, 308,
            "ladder budget must be ~5 minutes across 10 rungs"
        );
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
        // The ladder now continues into the cold-restart tail instead of giving
        // up at attempt 4 (review m1).
        assert_eq!(
            next_reconnect_step(4),
            ReconnectStep::Retry {
                attempt: 5,
                backoff_secs: 20
            }
        );
        assert_eq!(
            next_reconnect_step(DEFAULT_BACKOFF_SECONDS.len() as u32),
            ReconnectStep::GiveUp {
                attempted: DEFAULT_BACKOFF_SECONDS.len() as u32
            }
        );
    }

    #[test]
    fn jittered_backoff_stays_within_twenty_percent_band() {
        // Sample repeatedly; every draw must land in [0.8x, 1.2x] of the base.
        for _ in 0..1000 {
            let d = jittered_backoff(10);
            assert!(
                d >= Duration::from_millis(8000) && d <= Duration::from_millis(12000),
                "jittered backoff {d:?} out of the ±20% band for a 10s base"
            );
        }
    }

    #[test]
    fn jittered_backoff_zero_is_zero() {
        assert_eq!(jittered_backoff(0), Duration::ZERO);
    }
}
