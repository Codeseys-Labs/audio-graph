//! The session worker is the only publisher of its live ledger. Preserve FIFO
//! order across data, test controls and shutdown. An idle writer starts immediately;
//! two queued revisions can share a short collection window and one storage barrier.
use super::{
    LiveTranscriptResult, MAX_TRANSCRIPT_GROUP, POLL_INTERVAL, TranscriptEventWriteMsg as Message,
    app_handle,
    canonical_log::{PreparedTranscriptStorage, TranscriptCommitError as Error, TranscriptReceipt},
    commit_to_live_ledger, io,
};
use crate::projections::{TranscriptEvent, TranscriptLedger, TranscriptLiveIndex};
use std::{
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const GROUP_WINDOW: Duration = Duration::from_millis(8);

struct LiveRequests {
    ledger: Arc<Mutex<TranscriptLedger>>,
    events: Vec<TranscriptEvent>,
    replies: Vec<mpsc::Sender<LiveTranscriptResult>>,
}

impl LiveRequests {
    fn collect(
        event: TranscriptEvent,
        ledger: Arc<Mutex<TranscriptLedger>>,
        reply: mpsc::Sender<LiveTranscriptResult>,
        rx: &mpsc::Receiver<Message>,
        deferred: &mut Option<Message>,
    ) -> Self {
        let mut requests = Self {
            ledger,
            events: vec![event],
            replies: vec![reply],
        };
        let deadline = Instant::now() + GROUP_WINDOW;
        while requests.events.len() < MAX_TRANSCRIPT_GROUP {
            let next = if requests.events.len() == 1 {
                rx.try_recv().ok()
            } else {
                rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .ok()
            };
            match next {
                Some(Message::Live(event, ledger, reply))
                    if Arc::ptr_eq(&ledger, &requests.ledger) =>
                {
                    requests.events.push(event);
                    requests.replies.push(reply);
                }
                other => {
                    *deferred = other;
                    break;
                }
            }
        }
        requests
    }

    fn commit(
        self,
        storage: &PreparedTranscriptStorage,
        index: &mut TranscriptLiveIndex,
        path: &Path,
        session: &str,
    ) -> usize {
        if self.events.len() > 1 {
            let before = {
                let ledger = self.ledger.lock().unwrap_or_else(|e| e.into_inner());
                (ledger.session_id == session).then_some(ledger.accepted_event_count)
            };
            let result = before
                .ok_or(Error::LiveHeadChanged)
                .and_then(|previous| storage.commit_group_after(&self.events, previous));
            match result {
                Ok(Some(receipts)) => {
                    let count = self.events.len();
                    let mut live = self.ledger.lock().unwrap_or_else(|e| e.into_inner());
                    let results = self
                        .events
                        .into_iter()
                        .zip(receipts)
                        .map(|(event, receipt)| {
                            if live.session_id != session
                                || live.accepted_event_count.checked_add(1)
                                    != Some(receipt.sequence)
                            {
                                return Err(Error::LiveHeadChanged);
                            }
                            index
                                .apply(&mut live, event)
                                .map_err(|_| Error::InvalidRevision)?;
                            Ok((true, receipt))
                        })
                        .collect::<Vec<_>>();
                    drop(live);
                    for (reply, result) in self.replies.into_iter().zip(results) {
                        let _ = reply.send(result);
                    }
                    return count;
                }
                Err(error) => {
                    notify_storage_full(&error);
                    for reply in self.replies {
                        let _ = reply.send(Err(error.clone()));
                    }
                    return 0;
                }
                Ok(None) => {} // Preflight performed no group mutation. Resolve each request in order.
            }
        }
        for (event, reply) in self.events.into_iter().zip(self.replies) {
            let result = commit_to_live_ledger(index, storage, path, session, event, &self.ledger);
            if let Err(error) = &result {
                notify_storage_full(error);
            }
            let _ = reply.send(result);
        }
        0
    }
}

fn notify_storage_full(error: &Error) {
    if error.is_storage_full() {
        io::notify_transcript_storage_full(app_handle());
    }
}

pub(super) fn run(
    rx: mpsc::Receiver<Message>,
    storage: &PreparedTranscriptStorage,
    path: &Path,
    session: &str,
    shutdown: &AtomicBool,
) {
    let mut live_index = TranscriptLiveIndex::default();
    let mut deferred = None;
    #[cfg(test)]
    let mut group_counts = (0usize, 0usize);
    loop {
        match deferred
            .take()
            .map(Ok)
            .unwrap_or_else(|| rx.recv_timeout(POLL_INTERVAL))
        {
            Ok(Message::Live(event, ledger, reply)) => {
                let requests = LiveRequests::collect(event, ledger, reply, &rx, &mut deferred);
                let _group_size = requests.commit(storage, &mut live_index, path, session);
                #[cfg(test)]
                if _group_size > 0 {
                    group_counts.0 += 1;
                    group_counts.1 += _group_size;
                }
            }
            Ok(Message::Append(event, previous, reply)) => {
                let result: Result<TranscriptReceipt, _> = storage.commit_after(&event, previous);
                if let Err(error) = &result {
                    notify_storage_full(error);
                }
                let _ = reply.send(result);
            }
            #[cfg(test)]
            Ok(Message::SimulateDiskFull) => super::canonical_log::simulate_disk_full(),
            #[cfg(test)]
            Ok(Message::GroupCounts(reply)) => {
                let _ = reply.send(Ok(group_counts));
            }
            Ok(Message::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) if shutdown.load(Ordering::SeqCst) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}
