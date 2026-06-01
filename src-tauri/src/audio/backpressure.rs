//! Backpressure policy for the audio fan-out: drop-oldest on a full bounded
//! channel. Isolated here (crossbeam-only, no other crate deps) so it can be
//! unit-tested standalone even though the full crate's test harness can't
//! launch on Windows (the ML libs link a mismatched MSVC runtime — seeds 9f6e).

use crossbeam_channel::{Receiver, Sender, TrySendError};

/// Send `item` on a bounded channel, evicting the OLDEST queued item to make
/// room when the channel is full.
///
/// Keeping the most recent audio (rather than dropping the newest chunk) bounds
/// end-to-end latency under sustained overload, so transcription stays near
/// real time instead of falling further behind on stale audio. `drain_rx` is a
/// clone of the same channel's receiver, used solely to evict the oldest item
/// (crossbeam channels are MPMC, so this is safe alongside the real consumer).
///
/// Returns `true` if an item was dropped.
pub fn send_dropping_oldest<T>(tx: &Sender<T>, drain_rx: &Receiver<T>, item: T) -> bool {
    match tx.try_send(item) {
        Ok(()) => false,
        Err(TrySendError::Full(returned)) => {
            // Evict the oldest queued item, then retry once with the newest.
            let _ = drain_rx.try_recv();
            let _ = tx.try_send(returned);
            true
        }
        Err(TrySendError::Disconnected(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn does_not_drop_when_room_available() {
        let (tx, rx) = crossbeam_channel::bounded::<i32>(4);
        assert!(!send_dropping_oldest(&tx, &rx, 1));
        assert!(!send_dropping_oldest(&tx, &rx, 2));
        assert_eq!(rx.try_recv().unwrap(), 1);
        assert_eq!(rx.try_recv().unwrap(), 2);
    }

    #[test]
    fn evicts_oldest_and_keeps_newest_when_full() {
        let (tx, rx) = crossbeam_channel::bounded::<i32>(2);
        assert!(!send_dropping_oldest(&tx, &rx, 1)); // [1]
        assert!(!send_dropping_oldest(&tx, &rx, 2)); // [1,2] full
        // Full: should drop oldest (1) and keep newest (3) → [2,3].
        assert!(send_dropping_oldest(&tx, &rx, 3));
        assert_eq!(rx.try_recv().unwrap(), 2);
        assert_eq!(rx.try_recv().unwrap(), 3);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn sustained_overload_keeps_only_most_recent() {
        let (tx, rx) = crossbeam_channel::bounded::<i32>(3);
        for i in 0..100 {
            send_dropping_oldest(&tx, &rx, i);
        }
        // Buffer holds the last 3 values in order.
        let drained: Vec<i32> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert_eq!(drained, vec![97, 98, 99]);
    }

    #[test]
    fn disconnected_is_a_noop() {
        let (tx, rx) = crossbeam_channel::bounded::<i32>(1);
        drop(rx);
        let (_tx2, rx2) = crossbeam_channel::bounded::<i32>(1);
        // tx is disconnected; should report no drop and not panic.
        assert!(!send_dropping_oldest(&tx, &rx2, 5));
    }
}
