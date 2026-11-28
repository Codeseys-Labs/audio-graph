use super::*;
use crate::persistence::shutdown_tests::transcript_event;
use crate::persistence::{FileMemoryRepository, TranscriptEventWriter};
use crate::test_log_capture;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, mpsc};
use std::time::Duration;

const SESSION: &str = "session-orchard";
const SPAN: &str = "orchard branch";
const CONTENT: &str = "plum basket";
const CHILD_PATH: &str = "AUDIOGRAPH_TEST_TRANSCRIPT_COMMIT_SOURCE";
const CHILD_CUT: &str = "AUDIOGRAPH_TEST_TRANSCRIPT_COMMIT_CUT";
const CHILD_TEST: &str =
    "persistence::canonical_log::transcript_commit::tests::interrupted_writer_child";
const HANG_LIMIT: Duration = Duration::from_secs(60);

struct Fixture {
    root: PathBuf,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("orchard-path-{}", Uuid::new_v4()));
        let dir = root.join("transcripts");
        fs::create_dir_all(&dir).unwrap();
        Self {
            path: dir.join(format!("{SESSION}.events.jsonl")),
            root,
        }
    }

    fn writer(&self) -> TranscriptEventWriter {
        self.writer_for(SESSION)
    }

    fn writer_for(&self, session: &str) -> TranscriptEventWriter {
        let repository = FileMemoryRepository::with_data_root(&self.root);
        TranscriptEventWriter::repository(session, Arc::new(repository))
            .expect("writer must reopen a verified stream")
    }

    fn replay(&self) -> CanonicalLogSnapshot<TranscriptEvent> {
        load_canonical_stream(
            &self.path,
            SESSION,
            STREAM,
            1,
            CanonicalTailRecovery::Strict,
        )
        .expect("acknowledged frames must strictly replay")
    }

    fn assert_only_first_is_visible(&self) {
        let revisions =
            crate::persistence::canonical_reader::load_transcript_revisions(&self.path, SESSION)
                .unwrap()
                .into_payloads();
        assert_eq!(revisions.len(), 1);
        assert_eq!(revisions[0].text, "first");
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn writer_boundary_errors_withhold_acknowledgement_and_require_the_exact_live_attempt() {
    use CanonicalAppendPhase::{Flush, Sync, Write};

    let capture = test_log_capture::ThreadLogCapture::start();
    for (cut, phase, kind) in [
        (Cut::Full, Write, io::ErrorKind::StorageFull),
        (Cut::Short, Write, io::ErrorKind::WriteZero),
        (Cut::Flush, Flush, io::ErrorKind::Other),
        (Cut::Sync, Sync, io::ErrorKind::Other),
    ] {
        let fixture = Fixture::new();
        let first = transcript_event("leaf", "first");
        let second = transcript_event(SPAN, CONTENT);
        let next = transcript_event("stem", "next");
        let first_receipt = commit_transcript(&fixture.path, SESSION, &first).unwrap();
        assert_eq!(first_receipt.sequence, 1);
        assert_eq!(
            fixture.replay().head.unwrap().sequence,
            first_receipt.sequence
        );

        CUT.set(Some(cut));
        let error = commit_transcript(&fixture.path, SESSION, &second).unwrap_err();
        assert_eq!(
            error,
            TranscriptCommitError::Io {
                phase,
                kind,
                code: None
            }
        );
        let diagnostic = format!("{error:?}");
        for private in [SPAN, CONTENT, SESSION, "orchard-path"] {
            assert!(!diagnostic.contains(private));
        }

        let uncertain_bytes = fs::read(&fixture.path).unwrap();
        fixture.assert_only_first_is_visible();
        assert_eq!(
            commit_transcript(&fixture.path, SESSION, &transcript_event(SPAN, "different")),
            Err(TranscriptCommitError::ConflictingAttempt)
        );
        assert_eq!(fs::read(&fixture.path).unwrap(), uncertain_bytes);
        // A cached matching prefix (even a complete frame after failed sync)
        // cannot satisfy exact retry. A short rewrite from the original offset
        // still refuses and does not extend the old partial suffix.
        CUT.set(Some(Cut::Short));
        assert_eq!(
            commit_transcript(&fixture.path, SESSION, &second),
            Err(TranscriptCommitError::Io {
                phase: Write,
                kind: io::ErrorKind::WriteZero,
                code: None
            })
        );
        if cut != Cut::Full {
            assert_eq!(fs::read(&fixture.path).unwrap(), uncertain_bytes);
        }

        let receipt = commit_transcript(&fixture.path, SESSION, &second).unwrap();
        assert_eq!(receipt.sequence, 2);
        let replay = fixture.replay();
        assert_eq!(replay.head.unwrap().sequence, receipt.sequence);
        assert_eq!(replay.records.len(), 2);
        assert_eq!(
            replay.records[1].encoding,
            CanonicalRecordEncoding::FramedV1
        );
        assert_eq!(replay.records[1].payload.text, CONTENT);
        let durable_bytes = fs::read(&fixture.path).unwrap();
        let retry = commit_transcript(&fixture.path, SESSION, &second).unwrap();
        assert!(retry.already_accepted);
        assert_eq!(retry.sequence, receipt.sequence);
        assert_eq!(fs::read(&fixture.path).unwrap(), durable_bytes);
        assert!(transcript_gaps(&fixture.path).unwrap().is_empty());
        assert_eq!(
            commit_transcript(&fixture.path, SESSION, &next)
                .unwrap()
                .sequence,
            3
        );
    }
    let logs = test_log_capture::render(&capture.finish());
    for private in [SPAN, CONTENT, SESSION, "orchard-path"] {
        assert!(!logs.contains(private));
    }
}

#[test]
fn historical_v1_revision_keeps_its_committed_sequence_across_the_framing_upgrade() {
    let fixture = Fixture::new();
    let original = transcript_event(SPAN, CONTENT);
    // Historical readers accepted a complete last JSON object without LF.
    let original_bytes = serde_json::to_vec(&original).unwrap();
    fs::write(&fixture.path, &original_bytes).unwrap();
    let writer = fixture.writer();
    let receipt = writer.append(&original).unwrap();
    assert_eq!(receipt.sequence, 1);
    assert!(receipt.already_accepted);
    assert_eq!(fs::read(&fixture.path).unwrap(), original_bytes);
    assert_eq!(
        writer.append(&transcript_event(SPAN, "changed")),
        Err(TranscriptCommitError::Frame(
            CanonicalAppendRejection::EventIdConflict
        ))
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), original_bytes);

    let next = writer.append(&transcript_event("stem", "next")).unwrap();
    assert_eq!(next.sequence, 2);
    let snapshot = fixture.replay();
    assert_eq!(
        snapshot.records[0].encoding,
        CanonicalRecordEncoding::LegacyJsonl
    );
    assert_eq!(
        snapshot.records[1].encoding,
        CanonicalRecordEncoding::FramedV1
    );
    assert_eq!(snapshot.records.len(), 2);
    let historical_retry = writer.append(&original).unwrap();
    assert!(historical_retry.already_accepted);
    assert_eq!(historical_retry.sequence, 1);
    assert_eq!(fixture.replay().head.unwrap().sequence, 2);
    assert!(writer.shutdown_with_timeout(HANG_LIMIT));
}

#[test]
fn interrupted_writer_child() {
    let Some(path) = std::env::var_os(CHILD_PATH) else {
        return;
    };
    let cut = match std::env::var(CHILD_CUT).unwrap().as_str() {
        "torn" => Cut::KillPartial,
        "complete" => Cut::KillComplete,
        "idle" => Cut::Short,
        "intent" => Cut::KillIntent,
        "retire-failed" => Cut::Retire,
        "supersede" => Cut::Sync,
        "recover-truncate" => Cut::KillTruncated,
        "recover-gap" => Cut::KillGap,
        "recover-retire" => Cut::KillRetired,
        _ => panic!("unknown writer checkpoint"),
    };
    CUT.set(Some(cut));
    if matches!(cut, Cut::KillTruncated | Cut::KillGap | Cut::KillRetired) {
        let outcome = ready(Path::new(&path), SESSION);
        panic!("recovery returned before its subprocess cut: {outcome:?}");
    }
    let event = transcript_event(SPAN, CONTENT);
    let outcome = commit_transcript(Path::new(&path), SESSION, &event);
    if cut == Cut::Sync {
        assert!(matches!(
            outcome,
            Err(TranscriptCommitError::Io {
                phase: CanonicalAppendPhase::Sync,
                ..
            })
        ));
        CUT.set(Some(Cut::KillQuarantine));
        let outcome =
            commit_transcript(Path::new(&path), SESSION, &transcript_event("stem", "next"));
        panic!("superseding write returned before quarantine cut: {outcome:?}");
    }
    if cut == Cut::Retire {
        assert!(matches!(
            outcome,
            Err(TranscriptCommitError::Io {
                phase: CanonicalAppendPhase::RecoverySync,
                ..
            })
        ));
        wait_for_kill();
    }
    if cut == Cut::Short {
        assert!(matches!(
            outcome,
            Err(TranscriptCommitError::Io {
                phase: CanonicalAppendPhase::Write,
                kind: io::ErrorKind::WriteZero,
                ..
            })
        ));
        // The directory guard is gone. The frame remains owned until exit.
        wait_for_kill();
    }
    panic!("writer returned before the process kill: {outcome:?}");
}

/// The child signals from between the write and its durability barrier. The
/// deadline bounds a broken handshake; scheduling never selects a crash cut.
fn kill_at_writer_checkpoint(path: &Path, checkpoint: &str, while_alive: impl FnOnce()) {
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD_PATH, path)
        .env(CHILD_CUT, checkpoint)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, marker) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if line.ends_with("TRANSCRIPT_WRITER_CUT") {
                let _ = sender.send(());
                break;
            }
        }
    });
    let reached = marker.recv_timeout(HANG_LIMIT);
    let alive = child.try_wait().unwrap().is_none();
    // Reap the child even if its parent's concurrent observation fails.
    let observation = if reached.is_ok() && alive {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(while_alive))
    } else {
        Ok(())
    };
    if alive {
        child.kill().expect("kill checkpointed child");
    }
    let (reaped, status) = mpsc::channel();
    let reaper = std::thread::spawn(move || {
        let _ = reaped.send(child.wait());
    });
    let status = status
        .recv_timeout(HANG_LIMIT)
        .expect("child reap timed out")
        .expect("reap checkpointed child");
    reaper.join().unwrap();
    reader.join().unwrap();
    reached.expect("child must signal the exact cut");
    assert!(alive, "child exited before the requested kill");
    assert!(
        !status.success(),
        "the writer must die before acknowledging"
    );
    if let Err(payload) = observation {
        std::panic::resume_unwind(payload);
    }
}

#[test]
fn another_process_cannot_abandon_an_idle_live_writers_uncertain_frame() {
    let fixture = Fixture::new();
    commit_transcript(&fixture.path, SESSION, &transcript_event("leaf", "first")).unwrap();
    kill_at_writer_checkpoint(&fixture.path, "idle", || {
        let bytes = fs::read(&fixture.path).unwrap();
        assert_eq!(
            ready(&fixture.path, SESSION),
            Err(TranscriptCommitError::ConflictingAttempt)
        );
        assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
        assert!(transcript_gaps(&fixture.path).unwrap().is_empty());
    });
    let writer = fixture.writer();
    assert_eq!(transcript_gaps(&fixture.path).unwrap().len(), 1);
    assert_eq!(
        writer
            .append(&transcript_event("stem", "next"))
            .unwrap()
            .sequence,
        2
    );
    assert!(writer.shutdown_with_timeout(HANG_LIMIT));
}

#[test]
fn a_new_live_revision_resolves_an_uncertain_predecessor_before_committing() {
    for cut in [Cut::Full, Cut::Short, Cut::Flush, Cut::Sync] {
        let fixture = Fixture::new();
        let first = transcript_event("leaf", "first");
        commit_after(&fixture.path, SESSION, &first, Some(0)).unwrap();
        let prefix = fs::read(&fixture.path).unwrap();
        CUT.set(Some(cut));
        let error = commit_after(
            &fixture.path,
            SESSION,
            &transcript_event(SPAN, CONTENT),
            Some(1),
        )
        .unwrap_err();
        assert_eq!(error.is_storage_full(), cut == Cut::Full);
        let unresolved = fs::read(&fixture.path).unwrap();
        let suffix = unresolved.strip_prefix(prefix.as_slice()).unwrap();
        let next = commit_after(
            &fixture.path,
            SESSION,
            &transcript_event("stem", "next"),
            Some(1),
        )
        .unwrap();
        assert_eq!(next.sequence, 2);
        let replay = fixture.replay();
        assert_eq!(replay.records.len(), 2);
        assert_eq!(replay.records[1].payload.text, "next");
        let gaps = transcript_gaps(&fixture.path).unwrap();
        assert_eq!(gaps.len(), 1);
        assert_eq!(
            gaps[0].after.as_ref().unwrap().record_hash,
            replay.records[0].record_hash
        );
        assert_eq!(gaps[0].suffix_quarantined, !suffix.is_empty());
        let evidence: Vec<_> = transcript_recovery_artifacts(&fixture.path)
            .unwrap()
            .into_iter()
            .filter(|p| p.extension().is_some_and(|x| x == "quarantine"))
            .collect();
        assert_eq!(evidence.len(), usize::from(!suffix.is_empty()));
        if let Some(path) = evidence.first() {
            assert_eq!(fs::read(path).unwrap(), suffix);
        }
    }
}

#[test]
fn restart_quarantines_a_corrupt_suffix_of_the_exact_planned_frame_length() {
    let fixture = Fixture::new();
    commit_transcript(&fixture.path, SESSION, &transcript_event("leaf", "first")).unwrap();
    let prefix = fs::read(&fixture.path).unwrap();
    kill_at_writer_checkpoint(&fixture.path, "complete", || {});
    let mut bytes = fs::read(&fixture.path).unwrap();
    let last_json_byte = bytes.len() - 2;
    bytes[last_json_byte] ^= 1;
    fs::write(&fixture.path, &bytes).unwrap();
    let suffix = &bytes[prefix.len()..];
    let writer = fixture.writer();
    assert_eq!(fs::read(&fixture.path).unwrap(), prefix);
    let gaps = transcript_gaps(&fixture.path).unwrap();
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].suffix_quarantined);
    let evidence = transcript_recovery_artifacts(&fixture.path)
        .unwrap()
        .into_iter()
        .find(|p| p.extension().is_some_and(|x| x == "quarantine"))
        .unwrap();
    assert_eq!(fs::read(evidence).unwrap(), suffix);
    assert_eq!(
        writer
            .append(&transcript_event("stem", "next"))
            .unwrap()
            .sequence,
        2
    );
    assert!(writer.shutdown_with_timeout(HANG_LIMIT));
}

#[test]
fn process_kills_during_gap_recovery_keep_one_quarantine_one_gap_and_the_verified_head() {
    let fixture = Fixture::new();
    commit_transcript(&fixture.path, SESSION, &transcript_event("leaf", "first")).unwrap();
    let prefix = fs::read(&fixture.path).unwrap();
    let head = fixture.replay().head.unwrap();
    // The old sync failed but its full matching frame remains in cache.
    // A new live revision chooses to abandon, then dies after evidence publish.
    kill_at_writer_checkpoint(&fixture.path, "supersede", || {});
    let interrupted = fs::read(&fixture.path).unwrap();
    let suffix = interrupted.strip_prefix(prefix.as_slice()).unwrap();
    assert!(suffix.ends_with(b"\n"));
    fixture.assert_only_first_is_visible();
    kill_at_writer_checkpoint(&fixture.path, "recover-truncate", || {});
    assert_eq!(fs::read(&fixture.path).unwrap(), prefix);
    kill_at_writer_checkpoint(&fixture.path, "recover-gap", || {});
    assert!(fixture.path.with_added_extension("attempt").is_file());
    kill_at_writer_checkpoint(&fixture.path, "recover-retire", || {});
    assert!(!fixture.path.with_added_extension("attempt").exists());
    let writer = fixture.writer();
    let gaps = transcript_gaps(&fixture.path).unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(
        gaps[0].after.as_ref().unwrap().record_hash,
        head.record_hash
    );
    assert!(gaps[0].suffix_quarantined);
    let evidence: Vec<_> = transcript_recovery_artifacts(&fixture.path)
        .unwrap()
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x == "quarantine"))
        .collect();
    assert_eq!(evidence.len(), 1);
    assert_eq!(fs::read(&evidence[0]).unwrap(), suffix);
    assert_eq!(
        writer
            .append(&transcript_event("stem", "next"))
            .unwrap()
            .sequence,
        2
    );
    assert!(writer.shutdown_with_timeout(HANG_LIMIT));
}

#[test]
fn process_kill_with_only_a_durable_intent_records_an_empty_suffix_and_restores_absence() {
    let fixture = Fixture::new();
    kill_at_writer_checkpoint(&fixture.path, "intent", || {});
    assert!(!fixture.path.exists());
    assert!(fixture.path.with_added_extension("attempt").is_file());
    let writer = fixture.writer();
    assert!(!fixture.path.exists());
    assert_eq!(
        transcript_gaps(&fixture.path).unwrap(),
        vec![TranscriptGap {
            stream: GapStream::TranscriptRevisions,
            after: None,
            suffix_quarantined: false
        }]
    );
    assert!(
        !transcript_recovery_artifacts(&fixture.path)
            .unwrap()
            .iter()
            .any(|p| p.extension().is_some_and(|x| x == "quarantine"))
    );
    assert_eq!(
        writer
            .append(&transcript_event("stem", "next"))
            .unwrap()
            .sequence,
        1
    );
    assert!(writer.shutdown_with_timeout(HANG_LIMIT));
}

#[test]
fn process_kill_after_failed_descriptor_retirement_recovers_the_synced_frame_exactly_once() {
    let fixture = Fixture::new();
    commit_transcript(&fixture.path, SESSION, &transcript_event("leaf", "first")).unwrap();
    kill_at_writer_checkpoint(&fixture.path, "retire-failed", || {});
    assert!(fixture.path.with_added_extension("attempt").is_file());
    fixture.assert_only_first_is_visible();
    let writer = fixture.writer();
    let recovered = fixture.replay();
    assert_eq!(recovered.records.len(), 2);
    assert_eq!(recovered.records[1].payload.text, CONTENT);
    let retry = writer.append(&transcript_event(SPAN, CONTENT)).unwrap();
    assert!(retry.already_accepted);
    assert_eq!(retry.sequence, 2);
    assert!(transcript_gaps(&fixture.path).unwrap().is_empty());
    assert!(writer.shutdown_with_timeout(HANG_LIMIT));
}

#[test]
fn interrupted_first_revision_preserves_the_legacy_display_and_leaves_sequence_one_available() {
    use crate::persistence::canonical_reader::{StrictCanonicalRead, load_transcript_revisions};
    use crate::persistence::local_memory_repository_tests::{DataDirGuard, legacy_segment};

    let _lock = crate::sessions::TEST_HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let fixture = Fixture::new();
    let _data = DataDirGuard::set(&fixture.root);
    let legacy_path = fixture.path.with_file_name(format!("{SESSION}.jsonl"));
    let segment = legacy_segment("leaf", "microphone", "older words", 0.0, 1.0);
    crate::persistence::append_jsonl(&segment, &legacy_path, "legacy segment").unwrap();
    let legacy_bytes = fs::read(&legacy_path).unwrap();
    let assert_legacy_visible = || {
        assert!(matches!(
            load_transcript_revisions(&fixture.path, SESSION).unwrap(),
            StrictCanonicalRead::Missing
        ));
        let display =
            crate::persistence::load_transcript_segments_preferring_ledger(SESSION).unwrap();
        assert_eq!(display.len(), 1);
        assert_eq!(display[0].id, "leaf");
        assert_eq!(display[0].text, "older words");
        assert_eq!(fs::read(&legacy_path).unwrap(), legacy_bytes);
    };
    assert!(!fixture.path.exists());
    kill_at_writer_checkpoint(&fixture.path, "torn", assert_legacy_visible);
    assert_legacy_visible();
    let interrupted = fs::read(&fixture.path).unwrap();
    assert!(!interrupted.is_empty());
    let fresh_writer = fixture.writer_for(&Uuid::new_v4().to_string());
    assert!(!fixture.path.exists());
    assert_legacy_visible();
    let gaps = transcript_gaps(&fixture.path).unwrap();
    assert_eq!(
        gaps,
        vec![TranscriptGap {
            stream: GapStream::TranscriptRevisions,
            after: None,
            suffix_quarantined: true,
        }]
    );
    let evidence: Vec<_> = transcript_recovery_artifacts(&fixture.path)
        .unwrap()
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "quarantine"))
        .collect();
    assert_eq!(evidence.len(), 1);
    assert_eq!(fs::read(&evidence[0]).unwrap(), interrupted);
    assert!(fresh_writer.shutdown_with_timeout(HANG_LIMIT));
    let writer = fixture.writer();
    let receipt = writer
        .append(&transcript_event("stem", "first confirmed"))
        .unwrap();
    assert_eq!(receipt.sequence, 1);
    assert!(!receipt.already_accepted);
    assert_eq!(fixture.replay().records.len(), 1);
    assert_eq!(fixture.replay().head.unwrap().sequence, 1);
    assert!(writer.shutdown_with_timeout(HANG_LIMIT));
}

#[test]
fn startup_for_a_fresh_session_recovers_a_torn_frame_and_records_a_gap_at_the_old_head() {
    let fixture = Fixture::new();
    let first = transcript_event("leaf", "first");
    let first_receipt = commit_transcript(&fixture.path, SESSION, &first).unwrap();
    let first_head = fixture.replay().head.unwrap();
    assert_eq!(first_receipt.sequence, first_head.sequence);
    let prefix = fs::read(&fixture.path).unwrap();
    kill_at_writer_checkpoint(&fixture.path, "torn", || {});
    let interrupted = fs::read(&fixture.path).unwrap();
    let suffix = interrupted.strip_prefix(prefix.as_slice()).unwrap();
    assert!(!suffix.is_empty() && !suffix.ends_with(b"\n"));
    fixture.assert_only_first_is_visible();

    // Production chooses a new session at startup. Opening only its writer
    // must reconcile the previous session without any input from its provider.
    let fresh_session = Uuid::new_v4().to_string();
    let fresh_writer = fixture.writer_for(&fresh_session);
    assert!(
        !fixture
            .path
            .with_file_name(format!("{fresh_session}.events.jsonl"))
            .exists()
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), prefix);
    assert!(!fixture.path.with_added_extension("attempt").exists());
    let gaps = transcript_gaps(&fixture.path).unwrap();
    assert_eq!(
        gaps,
        vec![TranscriptGap {
            stream: GapStream::TranscriptRevisions,
            after: Some(VerifiedTranscriptHead {
                sequence: first_receipt.sequence,
                record_hash: first_head.record_hash,
            }),
            suffix_quarantined: true,
        }]
    );
    let quarantines: Vec<_> = transcript_recovery_artifacts(&fixture.path)
        .unwrap()
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "quarantine"))
        .collect();
    assert_eq!(quarantines.len(), 1);
    assert_eq!(fs::read(&quarantines[0]).unwrap(), suffix);
    assert!(fresh_writer.shutdown_with_timeout(HANG_LIMIT));
    let writer = fixture.writer();
    let next = transcript_event("stem", "next");
    let receipt = writer.append(&next).unwrap();
    assert!(!receipt.already_accepted);
    assert_eq!(receipt.sequence, 2);
    assert!(writer.shutdown_with_timeout(HANG_LIMIT));
    let reopened = fixture.writer();
    assert_eq!(
        fixture
            .replay()
            .records
            .iter()
            .map(|r| r.payload.text.as_str())
            .collect::<Vec<_>>(),
        ["first", "next"]
    );
    assert_eq!(transcript_gaps(&fixture.path).unwrap(), gaps);
    assert_eq!(fs::read(&quarantines[0]).unwrap(), suffix);
    assert!(reopened.shutdown_with_timeout(HANG_LIMIT));
}

#[test]
fn restart_syncs_a_complete_frame_then_replays_it_once_and_never_reuses_its_sequence() {
    let fixture = Fixture::new();
    commit_transcript(&fixture.path, SESSION, &transcript_event("leaf", "first")).unwrap();
    kill_at_writer_checkpoint(&fixture.path, "complete", || {});
    let written = fs::read(&fixture.path).unwrap();
    assert!(written.ends_with(b"\n"));
    fixture.assert_only_first_is_visible();

    CUT.set(Some(Cut::RecoverySync));
    assert_eq!(
        ready(&fixture.path, SESSION),
        Err(TranscriptCommitError::Io {
            phase: CanonicalAppendPhase::RecoverySync,
            kind: io::ErrorKind::Other,
            code: None,
        })
    );
    assert!(fixture.path.with_added_extension("attempt").is_file());
    fixture.assert_only_first_is_visible();

    let writer = fixture.writer();
    let recovered = fixture.replay();
    assert_eq!(recovered.head.unwrap().sequence, 2);
    assert_eq!(recovered.records[1].payload.text, CONTENT);
    let retry = writer.append(&transcript_event(SPAN, CONTENT)).unwrap();
    assert_eq!(retry.sequence, 2);
    assert!(retry.already_accepted);
    assert_eq!(fs::read(&fixture.path).unwrap(), written);
    let third = writer.append(&transcript_event("stem", "next")).unwrap();
    assert_eq!(third.sequence, 3);
    let snapshot = fixture.replay();
    assert_eq!(snapshot.head.unwrap().sequence, third.sequence);
    assert_eq!(
        snapshot
            .records
            .iter()
            .map(|r| r.payload.text.as_str())
            .collect::<Vec<_>>(),
        ["first", CONTENT, "next"]
    );
    assert!(transcript_gaps(&fixture.path).unwrap().is_empty());
    assert!(!fixture.path.with_added_extension("attempt").exists());
    assert!(writer.shutdown_with_timeout(HANG_LIMIT));
}

/// Release-only timing probe. Each sample uses the production queue, live
/// ledger and receipt path. Cold verification and fixture creation are untimed.
#[test]
#[ignore = "manual release SSD measurement"]
fn release_transcript_receipt_latency() {
    use crate::projections::TranscriptLedger;
    use std::io::BufWriter;
    use std::sync::Mutex;
    use std::time::Instant;
    for depth in [100_u64, 10_000, 50_000] {
        let fixture = Fixture::new();
        let mut source = BufWriter::new(File::create(&fixture.path).unwrap());
        let mut ledger = TranscriptLedger::new(SESSION);
        let event = |sequence: u64| {
            let mut row = transcript_event(&format!("span-{sequence}"), "reed");
            row.start_time = sequence as f64;
            row.end_time = row.start_time + 0.5;
            row
        };
        for sequence in 1..=depth {
            let revision = event(sequence);
            serde_json::to_writer(&mut source, &revision).unwrap();
            source.write_all(b"\n").unwrap();
            ledger.latest_spans.push(revision);
        }
        ledger.accepted_event_count = depth;
        source.flush().unwrap();
        source.get_ref().sync_all().unwrap();
        drop(source);
        let ledger = Arc::new(Mutex::new(ledger));
        let writer = fixture.writer();
        let client = writer.client();
        let mut timings = Vec::with_capacity(256);
        for sequence in depth + 1..=depth + 272 {
            let revision = event(sequence);
            let start = Instant::now();
            let (_, receipt) = client.append_live(&revision, ledger.clone()).unwrap();
            let elapsed = start.elapsed();
            assert_eq!(receipt.sequence, sequence);
            if sequence > depth + 16 {
                timings.push(elapsed);
            }
        }
        timings.sort_unstable();
        println!(
            "receipt_latency depth={depth} samples={} p50_us={} p95_us={}",
            timings.len(),
            timings[127].as_micros(),
            timings[243].as_micros()
        );
        assert!(writer.shutdown_with_timeout(HANG_LIMIT));
    }
}

#[test]
fn late_receipt_after_bounded_wait_commits_before_the_next_live_revision() {
    use crate::persistence::TranscriptEventWriteMsg;
    use crate::projections::TranscriptLedger;
    let fixture = Fixture::new();
    let writer = fixture.writer();
    let client = writer.client();
    let live = Arc::new(std::sync::Mutex::new(TranscriptLedger::new(SESSION)));
    let held = live.lock().unwrap();
    // Hold the publication target so the queued worker cannot finish before
    // the injected zero budget. Returning to the caller leaves it ownership.
    let outcome = client.request_with_timeout(Duration::ZERO, |reply| {
        TranscriptEventWriteMsg::Live(transcript_event("elm", "first"), live.clone(), reply)
    });
    assert_eq!(outcome, Err(TranscriptCommitError::AckTimeout));
    drop(held);
    let (_, receipt) = client
        .append_live(&transcript_event("fern", "next"), live.clone())
        .unwrap();
    assert_eq!(receipt.sequence, 2);
    assert_eq!(live.lock().unwrap().accepted_event_count, 2);
    assert_eq!(fixture.replay().records.len(), 2);
    assert!(writer.shutdown_with_timeout(HANG_LIMIT));
}

#[test]
fn session_lock_serializes_readers_and_writers_without_blocking_another_session() {
    use crate::persistence::canonical_durability::{
        observe_next_transcript_lock_wait, transcript_coordination_path, wait_for_transcript_lock,
    };
    let f = Fixture::new();
    let first = transcript_event("leaf", "first");
    commit_transcript(&f.path, SESSION, &first).unwrap();
    let lock_path = transcript_coordination_path(&f.path);
    for writer_waits in [true, false] {
        let held = wait_for_transcript_lock(&lock_path, writer_waits, false).unwrap();
        let path = f.path.clone();
        let (observed, waiter) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            observe_next_transcript_lock_wait(observed);
            if writer_waits {
                assert_eq!(
                    commit_transcript(&path, SESSION, &transcript_event("twig", "second"))
                        .unwrap()
                        .sequence,
                    2
                );
            } else {
                assert_eq!(
                    read_committed::<TranscriptEvent>(&path, SESSION)
                        .unwrap()
                        .head
                        .unwrap()
                        .sequence,
                    2
                );
            }
        });
        waiter
            .recv_timeout(HANG_LIMIT)
            .expect("peer actually attempted the conflicting lock");
        let other = "session-pear";
        let path = f.path.with_file_name(format!("{other}.events.jsonl"));
        assert_eq!(commit_transcript(&path, other, &first).unwrap().sequence, 1);
        drop(held);
        worker.join().unwrap();
    }
}

#[test]
fn weak_transcript_tier_requires_file_sync_and_preserves_the_checked_floor() {
    let f = Fixture::new();
    let mut storage = PreparedTranscriptStorage::open(&f.path, SESSION).unwrap();
    storage.tier = None;
    storage.ready().unwrap();
    assert!(storage.is_unqualified());
    let first = transcript_event("leaf", "first");
    assert_eq!(
        storage.commit_after(&first, Some(0)).unwrap().barrier,
        CanonicalDurabilityBarrier::SyncedUnqualified
    );
    let second = transcript_event("twig", "second");
    CUT.set(Some(Cut::Sync));
    assert!(matches!(
        storage.commit_after(&second, Some(1)),
        Err(TranscriptCommitError::Io {
            phase: CanonicalAppendPhase::Sync,
            ..
        })
    ));
    assert_eq!(
        read_committed::<TranscriptEvent>(&f.path, SESSION)
            .unwrap()
            .head
            .unwrap()
            .sequence,
        1
    );
    let receipt = storage.commit_after(&second, Some(1)).unwrap();
    assert_eq!(receipt.sequence, 2);
    assert_eq!(
        receipt.barrier,
        CanonicalDurabilityBarrier::SyncedUnqualified
    );
    let before = fs::read(&f.path).unwrap();
    let control =
        crate::persistence::session_semantics::session_control_plane_paths(&f.root, SESSION)
            .unwrap();
    fs::write(control.manifest, b"invalid").unwrap();
    assert_eq!(
        storage.commit_after(&transcript_event("stem", "third"), Some(2)),
        Err(TranscriptCommitError::SessionFloorUnavailable)
    );
    assert_eq!(fs::read(&f.path).unwrap(), before);
}

#[cfg(target_os = "linux")]
#[test]
fn writable_tmpfs_records_on_the_unqualified_tier() {
    let mount = Path::new("/dev/shm");
    if !mount.is_dir() {
        return;
    }
    let root = mount.join(format!("audiograph-{}", Uuid::new_v4()));
    fs::create_dir(&root).unwrap();
    let path = root
        .join("transcripts")
        .join(format!("{SESSION}.events.jsonl"));
    let f = Fixture { root, path };
    let storage = PreparedTranscriptStorage::open(&f.path, SESSION).unwrap();
    storage.ready().unwrap();
    assert!(storage.is_unqualified());
    assert_eq!(
        storage
            .commit_after(&transcript_event("leaf", "first"), None)
            .unwrap()
            .barrier,
        CanonicalDurabilityBarrier::SyncedUnqualified
    );
    assert_eq!(f.replay().head.unwrap().sequence, 1);
}
