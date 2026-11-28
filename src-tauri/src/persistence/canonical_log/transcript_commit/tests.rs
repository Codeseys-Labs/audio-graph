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
        for conflicting in [transcript_event(SPAN, "different"), next.clone()] {
            assert_eq!(
                commit_transcript(&fixture.path, SESSION, &conflicting),
                Err(TranscriptCommitError::ConflictingAttempt)
            );
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
        _ => panic!("unknown writer checkpoint"),
    };
    CUT.set(Some(cut));
    let event = transcript_event(SPAN, CONTENT);
    let outcome = commit_transcript(Path::new(&path), SESSION, &event);
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
