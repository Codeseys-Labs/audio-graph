//! Release benchmark counters on the projection actor thread. A strict
//! reader runs on its own thread, so its lock waits cannot enter a sample.
use std::cell::Cell;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Default)]
pub(crate) struct Sample {
    pub basis: Duration,
    pub commit: Duration,
    pub intent_publish: Duration,
    pub frame_write_sync: Duration,
    pub intent_retire: Duration,
    pub snapshot: Duration,
    pub exclusive_wait: Duration,
    pub shared_wait: Duration,
    pub retries: usize,
}

thread_local! {
    static SAMPLE: Cell<Sample> = Cell::default();
}

pub(crate) fn reset() {
    SAMPLE.set(Sample::default());
}

pub(crate) fn sample() -> Sample {
    SAMPLE.get()
}

pub(crate) fn record(update: impl FnOnce(&mut Sample)) {
    let mut sample = SAMPLE.get();
    update(&mut sample);
    SAMPLE.set(sample);
}

/// Count the complete interval from the first WouldBlock to acquisition
/// (or refusal), including time spent sleeping between lock attempts.
pub(super) struct LockWait {
    shared: bool,
    first: Option<Instant>,
    retries: usize,
}

impl LockWait {
    pub(super) fn new(shared: bool) -> Self {
        Self {
            shared,
            first: None,
            retries: 0,
        }
    }

    pub(super) fn blocked(&mut self) {
        self.first.get_or_insert_with(Instant::now);
        self.retries += 1;
    }
}

impl Drop for LockWait {
    fn drop(&mut self) {
        if let Some(first) = self.first {
            record(|s| {
                if self.shared {
                    s.shared_wait += first.elapsed();
                } else {
                    s.exclusive_wait += first.elapsed();
                }
                s.retries += self.retries;
            });
        }
    }
}
