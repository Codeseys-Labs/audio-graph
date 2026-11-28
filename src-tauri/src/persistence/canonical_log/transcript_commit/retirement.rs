//! A stopped writer with an unsettled frame gives up its live session, but
//! its exact attempt lease still needs an owner until the descriptor retires.
//! One recovery thread services all such streams, with backoff per lease.
use super::*;
use crate::persistence::RecoveryBackoff;
use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

struct Retired {
    resolve: Box<dyn FnMut() -> Result<(), TranscriptCommitError> + Send>,
    retry: RecoveryBackoff,
    first_ambiguous_failure: Option<Instant>,
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

// Leave time for a disconnected volume or delete-pending file to return.
// All ambiguous observations of one lease share the same elapsed-time budget.
const AMBIGUOUS_WINDOW: Duration = Duration::from_secs(5 * 60);

fn retryable(
    error: &TranscriptCommitError,
    first_ambiguous: &mut Option<Instant>,
    now: Instant,
) -> bool {
    use CanonicalCoordinationError as Coordination;
    use CanonicalDurabilityRejection as Rejection;
    use CanonicalFilesystemQualificationError as Qualification;
    use TranscriptCommitError as Error;
    let io_cause = match error {
        Error::Coordination(Coordination::Contended)
        | Error::ConflictingAttempt
        | Error::FrameWriteStorageFull
        | Error::StorageFullConfirmationPending => return true,
        Error::Io { kind, code, .. }
        | Error::Rejected(Rejection::IoFailedBeforeMutation {
            kind,
            raw_os_error: code,
            ..
        })
        | Error::Coordination(Coordination::Io {
            kind,
            raw_os_error: code,
            ..
        })
        | Error::Qualification(Qualification::Io {
            kind,
            raw_os_error: code,
            ..
        })
        | Error::Coordination(Coordination::QualificationRefused(Qualification::Io {
            kind,
            raw_os_error: code,
            ..
        })) => Some((*kind, *code)),
        Error::Indeterminate(error) => Some((error.kind, error.raw_os_error)),
        Error::Coordination(Coordination::IdentityUnavailable | Coordination::Missing)
        | Error::Qualification(
            Qualification::IdentityUnavailable | Qualification::NoMatchingMount,
        )
        | Error::Coordination(Coordination::QualificationRefused(
            Qualification::IdentityUnavailable | Qualification::NoMatchingMount,
        ))
        | Error::Rejected(Rejection::QualifiedDescendantVolumeUnavailable) => None,
        _ => return false,
    };
    if let Some((kind, code)) = io_cause {
        // Sharing/lock violations are actionable once the other handle closes.
        if cfg!(windows) && matches!(code, Some(32 | 33)) {
            return true;
        }
        if !matches!(
            kind,
            io::ErrorKind::PermissionDenied
                | io::ErrorKind::InvalidInput
                | io::ErrorKind::InvalidData
                | io::ErrorKind::NotFound
                | io::ErrorKind::Unsupported
                | io::ErrorKind::NotADirectory
                | io::ErrorKind::IsADirectory
                | io::ErrorKind::ReadOnlyFilesystem
        ) {
            return true;
        }
    }
    now.duration_since(*first_ambiguous.get_or_insert(now)) < AMBIGUOUS_WINDOW
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
    run_with_clock(queue, Instant::now, std::thread::sleep);
}

fn run_with_clock(queue: &Mutex<Queue>, clock: impl Fn() -> Instant, sleep: impl Fn(Duration)) {
    let mut running = Running(Some(queue));
    loop {
        let mut queue_guard = queue.lock().unwrap_or_else(|p| p.into_inner());
        let now = clock();
        if let Some(index) = queue_guard.jobs.iter().position(|j| j.retry.next <= now) {
            let mut job = queue_guard.jobs.remove(index).unwrap();
            drop(queue_guard);
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut job.resolve)) {
                Ok(Ok(())) => {}
                Ok(Err(error)) if retryable(&error, &mut job.first_ambiguous_failure, clock()) => {
                    job.retry.record(clock(), false);
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
            sleep(next.saturating_duration_since(now));
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
            first_ambiguous_failure: None,
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
    fn ambiguous_retirement_errors_share_one_retry_budget_per_lease() {
        let errors = [
            TranscriptCommitError::Io {
                phase: CanonicalAppendPhase::RecoveryRead,
                kind: io::ErrorKind::PermissionDenied,
                code: Some(5),
            },
            TranscriptCommitError::Rejected(
                CanonicalDurabilityRejection::QualifiedDescendantVolumeUnavailable,
            ),
            TranscriptCommitError::Coordination(CanonicalCoordinationError::IdentityUnavailable),
        ];
        let start = Instant::now();
        let mut first_ambiguous = None;
        for (i, error) in errors.iter().cycle().take(16).enumerate() {
            assert!(retryable(
                error,
                &mut first_ambiguous,
                start + Duration::from_secs(10 * i as u64)
            ));
        }
        for error in &errors {
            assert!(!retryable(
                error,
                &mut first_ambiguous,
                start + AMBIGUOUS_WINDOW
            ));
        }
    }

    #[test]
    fn prolonged_storage_full_settles_while_an_expired_ambiguous_lease_is_released() {
        for full_disk in [true, false] {
            let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
            let path = root.join("transcripts").join("orchard.events.jsonl");
            let storage = PreparedTranscriptStorage::open(&path, "orchard").unwrap();
            CUT.set(Some(Cut::Sync));
            assert!(
                storage
                    .commit_after(&transcript_event("seed", "leaf"), Some(0))
                    .is_err()
            );
            let mut job = storage.retired_job().unwrap();
            let start = job.retry.next;
            let clock = std::cell::Cell::new(start);
            let mut probes = 0;
            let mut settle = job.resolve;
            job.resolve = Box::new(move || {
                probes += 1;
                if !full_disk {
                    return Err(TranscriptCommitError::Coordination(
                        CanonicalCoordinationError::IdentityUnavailable,
                    ));
                }
                // Eighty backed-off probes span more than five minutes.
                match probes {
                    1..=40 => Err(TranscriptCommitError::FrameWriteStorageFull),
                    41..=80 => Err(TranscriptCommitError::StorageFullConfirmationPending),
                    _ => settle(),
                }
            });
            let queue = Mutex::new(Queue {
                jobs: [job].into(),
                running: true,
            });
            let capture = ThreadLogCapture::start();
            run_with_clock(
                &queue,
                || clock.get(),
                |delay| {
                    let next = clock.get() + delay;
                    assert!(
                        next - start < AMBIGUOUS_WINDOW * 2,
                        "unbounded ambiguous retry"
                    );
                    assert!(matches!(
                        lock_transcript_removal(&path, || Ok(true)),
                        Err(CanonicalCoordinationError::Contended)
                    ));
                    clock.set(next);
                },
            );
            let logs = test_log_capture::render(&capture.finish());
            assert_eq!(
                logs.matches("canonical_log.retired_attempt_requires_recovery")
                    .count(),
                usize::from(!full_disk)
            );
            assert!(clock.get() - start >= AMBIGUOUS_WINDOW);
            assert_eq!(path.with_added_extension("attempt").exists(), !full_disk);
            if full_disk {
                assert_eq!(
                    read_committed::<TranscriptEvent>(&path, "orchard")
                        .unwrap()
                        .head
                        .unwrap()
                        .sequence,
                    1
                );
            }
            drop(lock_transcript_removal(&path, || Ok(true)).unwrap());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn transient_manifest_read_retries_before_settling_the_dependent_projection() {
        exercise_transient_manifest(false);
        #[cfg(windows)]
        exercise_transient_manifest(true);
    }

    fn exercise_transient_manifest(sharing_violation: bool) {
        let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
        let session = "orchard";
        let path = |dir: &str| root.join(dir).join("orchard.events.jsonl");
        let transcript = PreparedTranscriptStorage::open(&path("transcripts"), session).unwrap();
        let projection = PreparedProjectionStorage::open(&path("projections"), session).unwrap();
        let first = transcript_event("seed", "leaf");
        transcript.commit_after(&first, Some(0)).unwrap();
        let mut patch = super::super::projection_tests::projection_patch(1, "fern");
        patch.basis = crate::projections::ProjectionBasis::from_transcript_events(&[first]);
        CUT.set(Some(Cut::Sync));
        assert!(projection.commit_after(&patch, Some(1)).is_err());
        CUT.set(Some(Cut::Sync));
        assert!(
            transcript
                .commit_after(&transcript_event("twig", "reed"), Some(1))
                .is_err()
        );
        let manifest = root.join(transcript.store.control_identities().manifest.as_str());
        fs::write(&manifest, b"birch").unwrap();
        #[cfg(windows)]
        let mut held = sharing_violation.then(|| {
            use std::os::windows::fs::OpenOptionsExt;
            File::options()
                .read(true)
                .share_mode(0)
                .open(&manifest)
                .unwrap()
        });
        if !sharing_violation {
            let changing = manifest.clone();
            crate::persistence::session_artifact_manifest::AFTER_MANIFEST_OPEN.set(Some(Box::new(
                move || {
                    fs::write(changing, b"willow").unwrap();
                },
            )));
        }
        let mut job = transcript.retired_job().unwrap();
        let mut step = job.resolve;
        let mut first_probe = true;
        job.resolve = Box::new(move || {
            let result = step();
            if std::mem::take(&mut first_probe) {
                if sharing_violation {
                    assert!(matches!(
                        result,
                        Err(TranscriptCommitError::Io { code: Some(32), .. })
                    ));
                } else {
                    assert!(matches!(
                        result,
                        Err(TranscriptCommitError::Io {
                            kind: io::ErrorKind::WouldBlock,
                            ..
                        })
                    ));
                }
                #[cfg(windows)]
                drop(held.take());
                fs::remove_file(&manifest).unwrap();
            }
            result
        });
        let queue = Mutex::new(Queue {
            jobs: [job, projection.retired_job().unwrap()].into(),
            running: true,
        });
        run(&queue);
        for dir in ["transcripts", "projections"] {
            assert!(!path(dir).with_added_extension("attempt").exists());
        }
        assert_eq!(
            read_committed::<TranscriptEvent>(&path("transcripts"), session)
                .unwrap()
                .head
                .unwrap()
                .sequence,
            2
        );
        assert_eq!(
            read_committed_projection::<ProjectionPatch>(&path("projections"), session)
                .unwrap()
                .head
                .unwrap()
                .sequence,
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

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
                    first_ambiguous_failure: None,
                },
                Retired {
                    resolve: Box::new(move || {
                        mark.store(true, std::sync::atomic::Ordering::SeqCst);
                        Ok(())
                    }),
                    retry: RecoveryBackoff::new(),
                    first_ambiguous_failure: None,
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
