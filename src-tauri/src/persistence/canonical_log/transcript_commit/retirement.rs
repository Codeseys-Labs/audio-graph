//! A stopped writer with an unsettled frame gives up its live session, but
//! its exact attempt lease still needs an owner until the descriptor retires.
//! One recovery thread services all such streams, with backoff per lease.
use super::*;
use crate::persistence::RecoveryBackoff;
use std::{collections::VecDeque, sync::Mutex, time::Instant};

struct Retired {
    resolve: Box<dyn FnMut() -> Result<(), TranscriptCommitError> + Send>,
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

// A missing or invalid admission proof requires a fresh open. Preserve
// the descriptor and the log for startup or explicit session deletion.
fn retryable(error: &TranscriptCommitError) -> bool {
    use TranscriptCommitError as Error;
    let (kind, code) = match error {
        Error::Coordination(CanonicalCoordinationError::Contended)
        | Error::ConflictingAttempt
        | Error::FrameWriteStorageFull
        | Error::StorageFullConfirmationPending => return true,
        Error::Io { kind, code, .. }
        | Error::Coordination(CanonicalCoordinationError::Io {
            kind,
            raw_os_error: code,
            ..
        })
        | Error::Rejected(CanonicalDurabilityRejection::IoFailedBeforeMutation {
            kind,
            raw_os_error: code,
            ..
        })
        | Error::Qualification(CanonicalFilesystemQualificationError::Io {
            kind,
            raw_os_error: code,
            ..
        }) => (*kind, *code),
        Error::Indeterminate(e) => (e.kind, e.raw_os_error),
        _ => return false,
    };
    if kind == io::ErrorKind::PermissionDenied {
        // Windows sharing and lock violations can clear as another handle closes.
        return cfg!(windows) && matches!(code, Some(32 | 33));
    }
    !matches!(
        kind,
        io::ErrorKind::InvalidInput
            | io::ErrorKind::InvalidData
            | io::ErrorKind::NotFound
            | io::ErrorKind::Unsupported
            | io::ErrorKind::NotADirectory
            | io::ErrorKind::IsADirectory
            | io::ErrorKind::ReadOnlyFilesystem
    )
}

struct Running<'a>(Option<&'a Mutex<Queue>>);

impl Drop for Running<'_> {
    fn drop(&mut self) {
        if let Some(queue) = self.0 {
            queue.lock().unwrap_or_else(|p| p.into_inner()).running = false;
        }
    }
}

fn run(queue: &Mutex<Queue>) {
    let mut running = Running(Some(queue));
    loop {
        let mut queue_guard = queue.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        if let Some(index) = queue_guard.jobs.iter().position(|j| j.retry.next <= now) {
            let mut job = queue_guard.jobs.remove(index).unwrap();
            drop(queue_guard);
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut job.resolve)) {
                Ok(Ok(())) => {}
                Ok(Err(error)) if retryable(&error) => {
                    job.retry.record(Instant::now(), false);
                    queue
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .jobs
                        .push_back(job);
                }
                Ok(Err(_)) => log::warn!("canonical_log.retired_attempt_requires_recovery"),
                Err(_) => log::warn!("canonical_log.retired_attempt_panicked"),
            }
        } else if let Some(next) = queue_guard.jobs.iter().map(|j| j.retry.next).min() {
            drop(queue_guard);
            std::thread::sleep(next.saturating_duration_since(now));
        } else {
            // Release ownership atomically with the empty-queue check.
            queue_guard.running = false;
            running.0 = None;
            return;
        }
    }
}

impl<E: CommitPayload> PreparedStorage<E> {
    fn retired_job(self) -> Option<Retired> {
        let key = self.path.with_added_extension("attempt-lock");
        let attempt = E::attempts()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key)?;
        let mut lease = Lease {
            live_key: None,
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
                        Err(error) => return Err(TranscriptCommitError::Coordination(error)),
                    };
                if transcript
                    .with_added_extension("attempt")
                    .try_exists()
                    .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?
                {
                    // No live exclusive transcript lease owns this descriptor.
                    // Leave both streams for transcript-first startup recovery.
                    return Err(TranscriptCommitError::InvalidHistory);
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
            .map(|_| ());
            self.source_basis.take();
            settled
        };
        Some(Retired {
            resolve: Box::new(resolve),
            retry: RecoveryBackoff::new(),
        })
    }

    pub(crate) fn retire_pending(self) {
        let Some(job) = self.retired_job() else {
            return;
        };
        let mut queue_guard = queue().lock().unwrap_or_else(|p| p.into_inner());
        queue_guard.jobs.push_back(job);
        if !queue_guard.running {
            queue_guard.running = true;
            drop(queue_guard);
            if std::thread::Builder::new()
                .name("retired-stream-recovery".into())
                .spawn(|| run(queue()))
                .is_err()
            {
                // Keep the one recovery slot on the outgoing writer thread
                // when the OS refuses a thread. Rotation already bounds joins.
                run(queue());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{
        canonical_durability::lock_transcript_removal, shutdown_tests::transcript_event,
    };
    use crate::test_log_capture::{self, ThreadLogCapture};

    #[test]
    fn terminal_retirement_preserves_artifacts_and_releases_the_deletion_lock() {
        for floor_unavailable in [false, true] {
            let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
            let dir = root.join("transcripts");
            fs::create_dir_all(&dir).unwrap();
            let path = dir.join("orchard.events.jsonl");
            let storage = PreparedTranscriptStorage::open(&path, "orchard").unwrap();
            storage
                .commit_after(&transcript_event("seed", "leaf"), Some(0))
                .unwrap();
            CUT.set(Some(Cut::Sync));
            assert!(matches!(
                storage.commit_after(&transcript_event("fern", "berry"), Some(1)),
                Err(TranscriptCommitError::Io {
                    phase: CanonicalAppendPhase::Sync,
                    ..
                })
            ));
            if floor_unavailable {
                fs::write(
                    root.join(storage.store.control_identities().manifest.as_str()),
                    b"reed",
                )
                .unwrap();
            } else {
                let mut stream = fs::read(&path).unwrap();
                stream[0] = b'?';
                fs::write(&path, stream).unwrap();
            }
            let descriptor = path.with_added_extension("attempt");
            let before = (fs::read(&path).unwrap(), fs::read(&descriptor).unwrap());
            let expected = if floor_unavailable {
                TranscriptCommitError::SessionFloorUnavailable
            } else {
                TranscriptCommitError::InvalidHistory
            };
            let mut job = storage.retired_job().unwrap();
            let mut resolve = job.resolve;
            let mut first = true;
            job.resolve = Box::new(move || {
                assert!(
                    std::mem::take(&mut first),
                    "a terminal failure must release its lease"
                );
                let result = resolve();
                assert_eq!(result, Err(expected.clone()));
                result
            });
            assert!(matches!(
                lock_transcript_removal(&path, || Ok(true)),
                Err(CanonicalCoordinationError::Contended)
            ));
            let queue = Mutex::new(Queue {
                jobs: [job].into(),
                running: true,
            });
            let capture = ThreadLogCapture::start();
            run(&queue);
            let logs = test_log_capture::render(&capture.finish());
            assert_eq!(
                logs.matches("canonical_log.retired_attempt_requires_recovery")
                    .count(),
                1
            );
            assert!(!logs.contains("retired_attempt_panicked"));
            assert!(!logs.contains("berry"));
            let removal = lock_transcript_removal(&path, || Ok(true)).unwrap();
            assert_eq!(
                (fs::read(&path).unwrap(), fs::read(&descriptor).unwrap()),
                before
            );
            drop(removal);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn a_panicking_retired_step_releases_its_slot_and_services_the_next_lease() {
        let serviced = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mark = serviced.clone();
        let queue = Mutex::new(Queue {
            jobs: [
                Retired {
                    resolve: Box::new(|| panic!("reed")),
                    retry: RecoveryBackoff::new(),
                },
                Retired {
                    resolve: Box::new(move || {
                        mark.store(true, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    }),
                    retry: RecoveryBackoff::new(),
                },
            ]
            .into(),
            running: true,
        });
        let capture = ThreadLogCapture::start();
        run(&queue);
        let logs = test_log_capture::render(&capture.finish());
        assert_eq!(
            logs.matches("canonical_log.retired_attempt_panicked")
                .count(),
            1
        );
        assert!(serviced.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!queue.lock().unwrap().running);

        // An unwind outside the step handler must also allow a new worker to start.
        queue.lock().unwrap().running = true;
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _running = Running(Some(&queue));
                panic!("fern");
            }))
            .is_err()
        );
        assert!(!queue.lock().unwrap().running);
    }
}
