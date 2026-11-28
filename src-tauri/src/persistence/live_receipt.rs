//! The waiter and worker transfer publication ownership under one mutex. A
//! timeout and successful completion cannot both claim (or both lose) a row.
use super::{LiveTranscriptResult, canonical_log::TranscriptCommitError};

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

enum State {
    Waiting,
    Ready(Delivery),
    Abandoned,
    Disconnected,
}

struct Delivery {
    result: LiveTranscriptResult,
    unpublished: Option<LatePublication>,
}

impl Drop for Delivery {
    fn drop(&mut self) {
        if let Some(publish) = self.unpublished.take() {
            publish(self.result.clone());
        }
    }
}

type Shared = Arc<(Mutex<State>, Condvar)>;
pub(crate) type LatePublication = Arc<dyn Fn(LiveTranscriptResult) + Send + Sync>;
pub(crate) struct LiveReply(Shared, Option<LatePublication>);
pub(crate) struct PendingLiveTranscript {
    shared: Shared,
    deadline: Instant,
}

pub(super) fn channel(
    timeout: Duration,
    publication: Option<LatePublication>,
) -> (LiveReply, PendingLiveTranscript) {
    let shared = Arc::new((Mutex::new(State::Waiting), Condvar::new()));
    (
        LiveReply(shared.clone(), publication),
        PendingLiveTranscript {
            shared,
            deadline: Instant::now() + timeout,
        },
    )
}

impl LiveReply {
    pub(super) fn finish(mut self, result: LiveTranscriptResult) {
        let delivery = Delivery {
            unpublished: self.1.take(),
            result,
        };
        let mut state = self.0.0.lock().unwrap_or_else(|p| p.into_inner());
        if matches!(*state, State::Waiting) {
            *state = State::Ready(delivery);
            self.0.1.notify_one();
        } else {
            drop(state);
            drop(delivery);
        }
    }
}

impl Drop for LiveReply {
    fn drop(&mut self) {
        let mut state = self.0.0.lock().unwrap_or_else(|p| p.into_inner());
        if matches!(*state, State::Waiting) {
            *state = State::Disconnected;
            self.0.1.notify_one();
        }
    }
}

impl PendingLiveTranscript {
    pub(crate) fn wait(self) -> LiveTranscriptResult {
        let (mutex, cv) = &*self.shared;
        let state = mutex.lock().unwrap_or_else(|p| p.into_inner());
        let (mut state, _) = cv
            .wait_timeout_while(
                state,
                self.deadline.saturating_duration_since(Instant::now()),
                |s| matches!(s, State::Waiting),
            )
            .unwrap_or_else(|p| p.into_inner());
        match std::mem::replace(&mut *state, State::Abandoned) {
            State::Ready(mut delivery) => {
                delivery.unpublished = None;
                delivery.result.clone()
            }
            State::Waiting => Err(TranscriptCommitError::AckTimeout),
            _ => Err(TranscriptCommitError::ReplyLost),
        }
    }
}

impl Drop for PendingLiveTranscript {
    fn drop(&mut self) {
        let delivery = std::mem::replace(
            &mut *self.shared.0.lock().unwrap_or_else(|p| p.into_inner()),
            State::Abandoned,
        );
        drop(delivery);
    }
}
