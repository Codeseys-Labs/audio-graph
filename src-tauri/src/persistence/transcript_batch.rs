//! The session worker is the only publisher of its live ledger. Preserve FIFO
//! order across data, test controls and shutdown. An idle writer starts immediately;
//! two queued revisions can share a short collection window and one storage barrier.
use super::{
    LiveReply, MAX_TRANSCRIPT_GROUP, POLL_INTERVAL, TranscriptEventWriteMsg as Message, app_handle,
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

/// Retain only members of the single attempt the worker can have in flight.
/// Resolve it before planning new work, or on an idle tick when speech stops.
#[derive(Default)]
struct Recovery {
    ledger: Option<Arc<Mutex<TranscriptLedger>>>,
    outstanding: Vec<TranscriptEvent>,
    needs_resolution: bool,
}

impl Recovery {
    fn classify(
        &mut self,
        storage: &PreparedTranscriptStorage,
        events: &[TranscriptEvent],
        error: Error,
    ) -> Error {
        notify_storage_full(&error);
        if storage.has_pending_attempt() {
            self.needs_resolution = true;
            if error == Error::FrameWriteStorageFull {
                // The descriptor still needs its gap and retirement, but no
                // source bytes were written and no publication remains owed.
                return error;
            }
            self.outstanding.extend_from_slice(events);
            if error.is_storage_full() {
                Error::StorageFullConfirmationPending
            } else {
                Error::ConfirmationPending
            }
        } else {
            error
        }
    }

    fn resolve(
        &mut self,
        storage: &PreparedTranscriptStorage,
        index: &mut TranscriptLiveIndex,
        path: &Path,
        session: &str,
    ) -> Result<(), Error> {
        if storage.has_pending_attempt()
            && let Err(error) = storage.resolve_pending()
        {
            if error.is_storage_full() {
                notify_storage_full(&error);
                for event in &self.outstanding {
                    crate::speech::resolve_recovery_publication(
                        session,
                        event,
                        Err(Error::StorageFullConfirmationPending),
                    );
                }
            }
            return Err(error);
        }
        for kept in storage.take_recovered() {
            self.outstanding.retain(|pending| pending != &kept.event);
            if let Some(live) = &self.ledger
                && install_recovered(
                    live,
                    index,
                    path,
                    session,
                    &kept.event,
                    kept.receipt.sequence,
                )
                .is_err()
            {
                // The canonical commit is complete. Leave the display update
                // to the next ledger rebuild and release the publication owner.
                log::warn!("transcript_recovery.live_install_deferred");
                crate::speech::resolve_recovery_publication(
                    session,
                    &kept.event,
                    Ok((false, kept.receipt)),
                );
                continue;
            }
            crate::speech::publish_recovered_transcript(session, &kept.event, kept.receipt);
        }
        for discarded in self.outstanding.drain(..) {
            crate::speech::discard_recovery_publication(session, &discarded);
        }
        self.needs_resolution = false;
        Ok(())
    }
}

/// Recovery can be the first operation against a newly attached live ledger.
/// Restore a missing prefix before publishing its recovered tail.
fn install_recovered(
    live: &Mutex<TranscriptLedger>,
    index: &mut TranscriptLiveIndex,
    path: &Path,
    session: &str,
    event: &TranscriptEvent,
    sequence: u64,
) -> Result<(), Error> {
    {
        let mut ledger = live.lock().unwrap_or_else(|p| p.into_inner());
        if ledger.session_id != session || ledger.accepted_event_count >= sequence {
            return Ok(());
        }
        if ledger.accepted_event_count.checked_add(1) == Some(sequence) {
            return index
                .apply(&mut ledger, event.clone())
                .map(|_| ())
                .map_err(|_| Error::InvalidHistory);
        }
    }
    let rows = super::canonical_log::read_committed::<TranscriptEvent>(path, session)
        .map_err(|_| Error::InvalidHistory)?;
    let count = usize::try_from(sequence).map_err(|_| Error::InvalidHistory)?;
    if rows
        .records
        .get(count - 1)
        .is_none_or(|r| &r.payload != event)
    {
        return Err(Error::InvalidHistory);
    }
    let restored = TranscriptLedger::replay(
        session,
        rows.records.into_iter().take(count).map(|r| r.payload),
    )
    .map_err(|_| Error::InvalidHistory)?;
    let mut ledger = live.lock().unwrap_or_else(|p| p.into_inner());
    if ledger.session_id == session && ledger.accepted_event_count < sequence {
        *ledger = restored;
        *index = TranscriptLiveIndex::default();
    }
    Ok(())
}

struct LiveRequests {
    ledger: Arc<Mutex<TranscriptLedger>>,
    events: Vec<TranscriptEvent>,
    replies: Vec<LiveReply>,
}

impl LiveRequests {
    fn collect(
        event: TranscriptEvent,
        ledger: Arc<Mutex<TranscriptLedger>>,
        reply: LiveReply,
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
        mut self,
        storage: &PreparedTranscriptStorage,
        index: &mut TranscriptLiveIndex,
        path: &Path,
        session: &str,
        recovery: &mut Recovery,
    ) -> usize {
        recovery.ledger = Some(self.ledger.clone());
        if let Err(error) = recovery.resolve(storage, index, path, session) {
            for reply in self.replies {
                reply.finish(Err(error.clone()));
            }
            return 0;
        }
        for reply in &mut self.replies {
            reply.mark_started();
        }
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
                        .iter()
                        .zip(receipts)
                        .map(|(event, receipt)| {
                            if live.session_id != session
                                || live.accepted_event_count.checked_add(1)
                                    != Some(receipt.sequence)
                            {
                                return Err(Error::LiveHeadChanged);
                            }
                            index
                                .apply(&mut live, event.clone())
                                .map_err(|_| Error::InvalidRevision)?;
                            Ok((true, receipt))
                        })
                        .collect::<Vec<_>>();
                    drop(live);
                    for (reply, result) in self.replies.into_iter().zip(results) {
                        reply.finish(result);
                    }
                    return count;
                }
                Err(error) => {
                    let error = recovery.classify(storage, &self.events, error);
                    for reply in self.replies {
                        reply.finish(Err(error.clone()));
                    }
                    return 0;
                }
                Ok(None) => {} // Preflight performed no group mutation. Resolve each request in order.
            }
        }
        for (event, reply) in self.events.into_iter().zip(self.replies) {
            let result = recovery
                .resolve(storage, index, path, session)
                .and_then(|()| {
                    commit_to_live_ledger(
                        index,
                        storage,
                        path,
                        session,
                        event.clone(),
                        &self.ledger,
                    )
                    .map_err(|error| {
                        recovery.classify(storage, std::slice::from_ref(&event), error)
                    })
                });
            reply.finish(result);
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
    let mut recovery = Recovery::default();
    let mut idle_retry = Duration::from_millis(100);
    let mut retry_at = Instant::now();
    #[cfg(test)]
    let mut paused = None::<(mpsc::Sender<()>, mpsc::Receiver<()>)>;
    #[cfg(test)]
    let mut group_counts = (0usize, 0usize);
    for saved in storage.take_recovered() {
        crate::speech::publish_recovered_transcript(session, &saved.event, saved.receipt);
    }
    loop {
        match deferred
            .take()
            .map(Ok)
            .unwrap_or_else(|| rx.recv_timeout(POLL_INTERVAL))
        {
            Ok(Message::Live(event, ledger, reply)) => {
                let requests = LiveRequests::collect(event, ledger, reply, &rx, &mut deferred);
                let _group_size =
                    requests.commit(storage, &mut live_index, path, session, &mut recovery);
                #[cfg(test)]
                if let Some((failed, resumed)) = paused.take() {
                    failed.send(()).unwrap();
                    resumed.recv_timeout(Duration::from_secs(60)).unwrap();
                }
                #[cfg(test)]
                if _group_size > 0 {
                    group_counts.0 += 1;
                    group_counts.1 += _group_size;
                }
            }
            Ok(Message::Append(event, previous, reply)) => {
                let result: Result<TranscriptReceipt, _> = recovery
                    .resolve(storage, &mut live_index, path, session)
                    .and_then(|()| storage.commit_after(&event, previous));
                if let Err(error) = &result {
                    notify_storage_full(error);
                }
                let _ = reply.send(result);
            }
            #[cfg(test)]
            Ok(Message::PauseAfterFault(fault, started, failed, resumed)) => {
                started.recv_timeout(Duration::from_secs(60)).unwrap();
                fault();
                paused = Some((failed, resumed));
            }
            #[cfg(test)]
            Ok(Message::GroupCounts(reply)) => {
                let _ = reply.send(Ok(group_counts));
            }
            Ok(Message::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) if shutdown.load(Ordering::SeqCst) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if recovery.needs_resolution && Instant::now() >= retry_at {
                    if recovery
                        .resolve(storage, &mut live_index, path, session)
                        .is_err()
                    {
                        idle_retry = (idle_retry * 2).min(Duration::from_secs(5));
                    } else {
                        idle_retry = Duration::from_millis(100);
                    }
                    retry_at = Instant::now() + idle_retry;
                }
            }
        }
    }
    if recovery.needs_resolution {
        // One last resolution while the outgoing session still owns its sink.
        // A failed barrier leaves the descriptor for startup recovery.
        let _ = recovery.resolve(storage, &mut live_index, path, session);
    }
}
