//! A stopped writer with an unsettled frame gives up its live session, but
//! its exact attempt lease still needs an owner until the descriptor retires.
//! One recovery thread services all such streams, with backoff per lease.
use super::*;
use crate::persistence::RecoveryBackoff;
use std::{collections::VecDeque, sync::Mutex, time::Instant};

struct Retired {
    resolve: Box<dyn FnMut() -> bool + Send>,
    retry: RecoveryBackoff,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Retired>,
    running: bool,
}

fn queue() -> &'static Mutex<Queue> {
    static QUEUE: OnceLock<Mutex<Queue>> = OnceLock::new();
    QUEUE.get_or_init(Default::default)
}

fn run() {
    loop {
        let mut queue_guard = queue().lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        if let Some(index) = queue_guard.jobs.iter().position(|j| j.retry.next <= now) {
            let mut job = queue_guard.jobs.remove(index).unwrap();
            drop(queue_guard);
            if !(job.resolve)() {
                job.retry.record(Instant::now(), false);
                queue()
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .jobs
                    .push_back(job);
            }
        } else if let Some(next) = queue_guard.jobs.iter().map(|j| j.retry.next).min() {
            drop(queue_guard);
            std::thread::sleep(next.saturating_duration_since(now));
        } else {
            queue_guard.running = false;
            return;
        }
    }
}

impl<E: CommitPayload> PreparedStorage<E> {
    pub(crate) fn retire_pending(self) {
        let key = self.path.with_added_extension("attempt-lock");
        let Some(attempt) = E::attempts()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key)
        else {
            return;
        };
        let mut lease = Lease {
            path: key,
            file: Some(attempt.file),
            pending: Some(attempt.frame),
        };
        log::warn!("canonical_log.retired_attempt_pending");
        self.cache.take();
        self.source_basis.take();
        self.take_recovered();
        let transcript = self
            .root
            .join("transcripts")
            .join(format!("{}.events.jsonl", self.session));
        let resolve = move || {
            // A failed descriptor unlink may already be invisible by path.
            // Its owner still holds the attempt lock until its parent sync.
            let _preceding = if E::STREAM == GapStream::ProjectionPatches {
                let held =
                    match crate::persistence::canonical_durability::try_shared_transcript_lock(
                        &transcript.with_added_extension("attempt-lock"),
                    ) {
                        Ok(file) => Some(file),
                        Err(CanonicalCoordinationError::Missing) => None,
                        Err(_) => return false,
                    };
                if !matches!(
                    transcript.with_added_extension("attempt").try_exists(),
                    Ok(false)
                ) {
                    return false;
                }
                held
            } else {
                None
            };
            // Retain the issuing writer's tier (including an unqualified
            // alias). Never requalify from the storage's canonicalized path.
            let settled = with_writable_v1(&self.root, &self.store, || {
                Commit::open(&self)?.resolve_lease(&mut lease)
            })
            .is_ok();
            self.source_basis.take();
            settled
        };
        let mut queue_guard = queue().lock().unwrap_or_else(|p| p.into_inner());
        queue_guard.jobs.push_back(Retired {
            resolve: Box::new(resolve),
            retry: RecoveryBackoff::new(),
        });
        if !queue_guard.running {
            queue_guard.running = true;
            drop(queue_guard);
            if std::thread::Builder::new()
                .name("retired-stream-recovery".into())
                .spawn(run)
                .is_err()
            {
                // Keep the one recovery slot on the outgoing writer thread
                // when the OS refuses a thread. Rotation already bounds joins.
                run();
            }
        }
    }
}
