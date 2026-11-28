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

thread_local! {
    pub(super) static BEFORE_FRAME_SYNC: std::cell::Cell<Option<Box<dyn FnOnce()>>> = const { std::cell::Cell::new(None) };
}

pub(super) fn pause_before_frame_sync_if_armed() {
    if let Some(pause) = BEFORE_FRAME_SYNC.take() {
        pause();
    }
}

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
fn writer_boundary_errors_withhold_acknowledgement_and_retry_keeps_only_verified_frames() {
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
            if cut == Cut::Full {
                TranscriptCommitError::FrameWriteStorageFull
            } else {
                TranscriptCommitError::Io {
                    phase,
                    kind,
                    code: None,
                }
            }
        );
        let diagnostic = format!("{error:?}");
        for private in [SPAN, CONTENT, SESSION, "orchard-path"] {
            assert!(!diagnostic.contains(private));
        }

        fixture.assert_only_first_is_visible();
        let complete = matches!(cut, Cut::Flush | Cut::Sync);
        if complete {
            // A matching cached frame still requires a rewrite and fresh barrier.
            let uncertain = fs::read(&fixture.path).unwrap();
            CUT.set(Some(Cut::Short));
            assert_eq!(
                commit_transcript(&fixture.path, SESSION, &second),
                Err(io_failure(Write, io::ErrorKind::WriteZero.into()))
            );
            assert_eq!(fs::read(&fixture.path).unwrap(), uncertain);
            fixture.assert_only_first_is_visible();
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
        assert_eq!(
            transcript_gaps(&fixture.path).unwrap().len(),
            usize::from(!complete)
        );
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
    let checkpoint = std::env::var(CHILD_CUT).unwrap();
    if let Some(projection_cut) = checkpoint.strip_prefix("projection-") {
        super::projection_tests::interrupt_patch(Path::new(&path), projection_cut);
    }
    let cut = match checkpoint.as_str() {
        "group-torn" => Cut::KillGroupTorn,
        "group-member" => Cut::KillGroupMember,
        "group-complete" => Cut::KillComplete,
        "torn" => Cut::KillPartial,
        "complete" => Cut::KillComplete,
        "idle" => Cut::Short,
        "intent" => Cut::KillIntent,
        "retire-failed" => Cut::Retire,
        "resolve-torn" => Cut::Short,
        "recover-truncate" => Cut::KillTruncated,
        "recover-gap" => Cut::KillGap,
        "recover-retire" => Cut::KillRetired,
        _ => panic!("unknown writer checkpoint"),
    };
    if std::env::var(CHILD_CUT).unwrap().starts_with("group-") {
        let prepared = PreparedTranscriptStorage::open(Path::new(&path), SESSION).unwrap();
        prepared.ready().unwrap();
        CUT.set(Some(cut));
        let outcome = prepared.commit_group_after(&group_events(), 1);
        panic!("group returned before the process cut: {outcome:?}");
    }
    CUT.set(Some(cut));
    if matches!(cut, Cut::KillTruncated | Cut::KillGap | Cut::KillRetired) {
        let outcome = ready(Path::new(&path), SESSION);
        panic!("recovery returned before its subprocess cut: {outcome:?}");
    }
    let event = transcript_event(SPAN, CONTENT);
    let outcome = commit_transcript(Path::new(&path), SESSION, &event);
    if std::env::var(CHILD_CUT).as_deref() == Ok("resolve-torn") {
        assert!(matches!(
            outcome,
            Err(TranscriptCommitError::Io {
                phase: CanonicalAppendPhase::Write,
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
pub(super) fn kill_at_writer_checkpoint(path: &Path, checkpoint: &str, while_alive: impl FnOnce()) {
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

/// Leave a complete first frame on disk at the subprocess's pre-sync cut.
pub(crate) fn interrupted_historical_caption(root: &Path) -> (&'static str, TranscriptEvent) {
    let directory = root.join("transcripts");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("{SESSION}.events.jsonl"));
    kill_at_writer_checkpoint(&path, "complete", || {});
    assert!(path.with_added_extension("attempt").is_file());
    (SESSION, transcript_event(SPAN, CONTENT))
}

fn group_events() -> [TranscriptEvent; 3] {
    [
        transcript_event("willow", "second"),
        transcript_event("birch", "third"),
        transcript_event("ash", "fourth"),
    ]
}

#[test]
fn group_preflight_is_nonmutating_and_the_complete_group_shares_one_file_write() {
    let fixture = Fixture::new();
    let storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
    let events = group_events();
    assert_eq!(storage.commit_group_after(&events, 0).unwrap(), None);
    assert!(!fixture.path.exists());
    storage
        .commit_after(&transcript_event("leaf", "first"), Some(0))
        .unwrap();
    let original = fs::read(&fixture.path).unwrap();
    let retained = transcript_recovery_artifacts(&fixture.path).unwrap();
    assert_eq!(storage.commit_group_after(&events, 0).unwrap(), None);
    assert_eq!(
        storage
            .commit_group_after(&[events[0].clone(), events[0].clone()], 1)
            .unwrap(),
        None
    );
    assert_eq!(
        storage
            .commit_group_after(&[events[0].clone(), transcript_event("leaf", "first")], 1)
            .unwrap(),
        None
    );
    let mut invalid = events.clone();
    invalid[1].confidence = f32::NAN;
    assert_eq!(storage.commit_group_after(&invalid, 1).unwrap(), None);
    assert_eq!(
        transcript_recovery_artifacts(&fixture.path).unwrap(),
        retained
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), original);
    let before = group::GROUP_WRITES.get();
    let receipts = storage.commit_group_after(&events, 1).unwrap().unwrap();
    assert!(
        receipts
            .iter()
            .all(|r| !r.already_accepted && r.barrier == receipts[0].barrier)
    );
    assert_eq!(
        receipts.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        [2, 3, 4]
    );
    assert_eq!(group::GROUP_WRITES.get() - before, 1);
    assert_eq!(fixture.replay().head.unwrap().sequence, 4);
    assert!(transcript_gaps(&fixture.path).unwrap().is_empty());
}

#[test]
fn a_failed_group_barrier_withholds_all_receipts_and_exact_retry_rewrites_the_group() {
    let fixture = Fixture::new();
    let storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
    storage
        .commit_after(&transcript_event("leaf", "first"), Some(0))
        .unwrap();
    let events = group_events();
    CUT.set(Some(Cut::Sync));
    assert!(matches!(
        storage.commit_group_after(&events, 1),
        Err(TranscriptCommitError::Io {
            phase: CanonicalAppendPhase::Sync,
            ..
        })
    ));
    fixture.assert_only_first_is_visible();
    let bytes = fs::read(&fixture.path).unwrap();
    CUT.set(Some(Cut::Short));
    assert!(matches!(
        storage.commit_group_after(&events, 1),
        Err(TranscriptCommitError::Io {
            kind: io::ErrorKind::WriteZero,
            ..
        })
    ));
    fixture.assert_only_first_is_visible();
    assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
    let receipts = storage.commit_group_after(&events, 1).unwrap().unwrap();
    assert_eq!(
        receipts.iter().map(|r| r.sequence).collect::<Vec<_>>(),
        [2, 3, 4]
    );
    assert_eq!(fs::read(&fixture.path).unwrap(), bytes);
    for event in &events {
        assert!(storage.commit_after(event, None).unwrap().already_accepted);
    }
    assert_eq!(fixture.replay().records.len(), 4);
    assert!(transcript_gaps(&fixture.path).unwrap().is_empty());
}

#[test]
fn an_individual_group_member_retry_cannot_duplicate_or_change_the_live_owners_frames() {
    let fixture = Fixture::new();
    let storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
    storage
        .commit_after(&transcript_event("leaf", "first"), Some(0))
        .unwrap();
    let events = group_events();
    CUT.set(Some(Cut::Short));
    assert!(matches!(
        storage.commit_group_after(&events, 1),
        Err(TranscriptCommitError::Io {
            kind: io::ErrorKind::WriteZero,
            ..
        })
    ));
    // The first member is complete, the second torn. Redelivering the second
    // can re-use the discarded attempt's sequence but never the first one's.
    let member = storage.commit_after(&events[1], None).unwrap();
    assert_eq!(member.sequence, 3);
    assert!(!member.already_accepted);
    assert_eq!(fixture.replay().records.len(), 3);
    let recovered = storage.take_recovered();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].event, events[0]);
    assert_eq!(recovered[0].receipt.sequence, 2);
    let gaps = transcript_gaps(&fixture.path).unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].after.as_ref().unwrap().sequence, 2);
    let mut changed = events[1].clone();
    changed.text = "changed".into();
    assert_eq!(
        storage.commit_after(&changed, None),
        Err(TranscriptCommitError::Frame(
            CanonicalAppendRejection::EventIdConflict
        ))
    );
}

#[test]
fn a_killed_group_replays_its_complete_members_and_quarantines_only_the_torn_remainder() {
    for (checkpoint, complete) in [
        ("group-torn", 1),
        ("group-member", 1),
        ("group-complete", 3),
    ] {
        let fixture = Fixture::new();
        commit_transcript(&fixture.path, SESSION, &transcript_event("leaf", "first")).unwrap();
        kill_at_writer_checkpoint(&fixture.path, checkpoint, || {});
        fixture.assert_only_first_is_visible();
        let before = fs::read(&fixture.path).unwrap();
        let end = before.iter().rposition(|&b| b == b'\n').unwrap() + 1;
        if checkpoint == "group-torn" {
            // Crash a second time after quarantine publication and truncation,
            // before the gap has become durable.
            kill_at_writer_checkpoint(&fixture.path, "recover-truncate", || {});
            fixture.assert_only_first_is_visible();
        }
        let storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
        // A different next request drives reconciliation. Its stale live head
        // cannot advance past recovered members until they have been replayed.
        let next = transcript_event("elm", "next");
        assert_eq!(
            storage.commit_after(&next, Some(1)),
            Err(TranscriptCommitError::LiveHeadChanged)
        );
        let snapshot = fixture.replay();
        assert_eq!(snapshot.records.len(), 1 + complete);
        assert_eq!(fs::read(&fixture.path).unwrap(), before[..end]);
        let gaps = transcript_gaps(&fixture.path).unwrap();
        if complete == 1 {
            assert_eq!(gaps.len(), 1);
            assert_eq!(
                gaps[0].after.as_ref().unwrap().record_hash,
                snapshot.records[1].record_hash
            );
            assert_eq!(gaps[0].suffix_quarantined, end != before.len());
            let quarantine = transcript_recovery_artifacts(&fixture.path)
                .unwrap()
                .into_iter()
                .find(|p| p.extension().is_some_and(|x| x == "quarantine"));
            assert_eq!(quarantine.is_some(), end != before.len());
            if let Some(quarantine) = quarantine {
                assert_eq!(fs::read(quarantine).unwrap(), before[end..]);
            }
        } else {
            assert!(gaps.is_empty());
        }
        for event in group_events().iter().take(complete) {
            assert!(storage.commit_after(event, None).unwrap().already_accepted);
        }
        let head = 1 + complete as u64;
        assert_eq!(
            storage.commit_after(&next, Some(head)).unwrap().sequence,
            head + 1
        );
        assert_eq!(fixture.replay().records.len(), 2 + complete);
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
fn repeat_recovery_keeps_a_member_even_if_its_last_byte_equals_the_quarantined_suffix() {
    let fixture = Fixture::new();
    commit_transcript(&fixture.path, SESSION, &transcript_event("leaf", "first")).unwrap();
    kill_at_writer_checkpoint(&fixture.path, "group-member", || {});
    let through_first_member = fs::read(&fixture.path).unwrap();
    assert!(through_first_member.ends_with(b"\n"));
    OpenOptions::new()
        .append(true)
        .open(&fixture.path)
        .unwrap()
        .write_all(b"\n")
        .unwrap();
    kill_at_writer_checkpoint(&fixture.path, "recover-truncate", || {});
    assert_eq!(fs::read(&fixture.path).unwrap(), through_first_member);
    let storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
    storage.resolve_pending().unwrap();
    let saved = storage.take_recovered();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].event, group_events()[0]);
    assert_eq!(saved[0].receipt.sequence, 2);
    let gaps = transcript_gaps(&fixture.path).unwrap();
    assert_eq!(gaps.len(), 1);
    assert!(gaps[0].suffix_quarantined);
    assert_eq!(gaps[0].after.as_ref().unwrap().sequence, 2);
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
        let storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
        let next = transcript_event("stem", "next");
        let result = storage.commit_after(&next, Some(1));
        if matches!(cut, Cut::Flush | Cut::Sync) {
            assert_eq!(result, Err(TranscriptCommitError::LiveHeadChanged));
            let kept = storage.take_recovered();
            assert_eq!(kept.len(), 1);
            assert_eq!(kept[0].receipt.sequence, 2);
            assert_eq!(kept[0].event, transcript_event(SPAN, CONTENT));
            assert_eq!(storage.commit_after(&next, Some(2)).unwrap().sequence, 3);
            assert!(transcript_gaps(&fixture.path).unwrap().is_empty());
            continue;
        }
        assert!(storage.take_recovered().is_empty());
        let next = result.unwrap();
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
    // A torn member is quarantined, then recovery dies after evidence publication.
    kill_at_writer_checkpoint(&fixture.path, "resolve-torn", || {});
    let interrupted = fs::read(&fixture.path).unwrap();
    let suffix = interrupted.strip_prefix(prefix.as_slice()).unwrap();
    assert!(!suffix.ends_with(b"\n"));
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
            projection_lanes: Vec::new(),
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
    let fresh_session = Uuid::new_v4().to_string();
    let fresh_writer = fixture.writer_for(&fresh_session);
    crate::persistence::transcript_startup::recover_in_directory(
        fixture.path.parent().unwrap(),
        &fresh_session,
        |_, _, _| panic!("a torn first member cannot yield a receipt"),
    )
    .unwrap();
    assert!(!fixture.path.exists());
    assert_legacy_visible();
    let gaps = transcript_gaps(&fixture.path).unwrap();
    assert_eq!(
        gaps,
        vec![TranscriptGap {
            projection_lanes: Vec::new(),
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

    // Startup reconciles the previous session before the first historical
    // load even when the new session receives no speech.
    let fresh_session = Uuid::new_v4().to_string();
    let fresh_writer = fixture.writer_for(&fresh_session);
    crate::persistence::transcript_startup::recover_in_directory(
        fixture.path.parent().unwrap(),
        &fresh_session,
        |_, _, _| panic!("a torn first member cannot yield a receipt"),
    )
    .unwrap();
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
            projection_lanes: Vec::new(),
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
fn startup_delivers_a_kept_group_in_order_without_new_speech_or_a_second_notification() {
    let fixture = Fixture::new();
    commit_transcript(&fixture.path, SESSION, &transcript_event("leaf", "first")).unwrap();
    kill_at_writer_checkpoint(&fixture.path, "group-complete", || {});
    let mut confirmations = Vec::new();
    for _ in 0..2 {
        crate::persistence::transcript_startup::recover_in_directory(
            fixture.path.parent().unwrap(),
            "fresh-session",
            |session, event, receipt| {
                assert_eq!(session, SESSION);
                confirmations.push((event.clone(), receipt.sequence));
            },
        )
        .unwrap();
    }
    let expected: Vec<_> = group_events().into_iter().zip(2_u64..).collect();
    assert_eq!(confirmations, expected);
    let read =
        crate::persistence::canonical_reader::load_transcript_revisions(&fixture.path, SESSION)
            .unwrap()
            .into_payloads();
    assert_eq!(read[1..], group_events());
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

#[test]
fn retry_fences_an_uncertain_intent_publication_before_touching_the_source() {
    use crate::persistence::canonical_durability::{
        completed_parent_flushes, sync_unqualified_transcript_parent,
    };
    for (grouped, force_weak) in [(false, false), (true, false), (false, true), (true, true)] {
        let fixture = Fixture::new();
        let mut storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
        if force_weak {
            storage.tier = None;
        }
        let expected_parent_flushes = if storage.is_unqualified() {
            // Some weak-tier filesystems cannot flush a directory. Match the
            // actual capability of this fixture's directory.
            let before = completed_parent_flushes();
            sync_unqualified_transcript_parent(fixture.path.parent().unwrap()).unwrap();
            completed_parent_flushes() - before
        } else {
            1
        };
        storage
            .commit_after(&transcript_event("seed", CONTENT), Some(0))
            .unwrap();
        let original = fs::read(&fixture.path).unwrap();
        let events = group_events();
        let append = || {
            if grouped {
                storage
                    .commit_group_after(&events, 1)
                    .map(|r| r.unwrap().len())
            } else {
                storage.commit_after(&events[0], Some(1)).map(|_| 1)
            }
        };
        CUT.set(Some(Cut::IntentPublish));
        assert!(matches!(
            append(),
            Err(TranscriptCommitError::Indeterminate(_)
                | TranscriptCommitError::Io {
                    phase: CanonicalAppendPhase::RecoverySync,
                    ..
                })
        ));
        assert!(fixture.path.with_added_extension("attempt").is_file());
        LAST_INTENT_PARENT_FLUSHES.set(None);
        CUT.set(Some(Cut::IntentResync));
        assert_eq!(
            append(),
            Err(io_failure(
                CanonicalAppendPhase::RecoverySync,
                io::ErrorKind::Other.into()
            ))
        );
        assert_eq!(fs::read(&fixture.path).unwrap(), original);
        assert_eq!(LAST_INTENT_PARENT_FLUSHES.get(), None);
        assert_eq!(append().unwrap(), if grouped { events.len() } else { 1 });
        assert_eq!(
            LAST_INTENT_PARENT_FLUSHES.take(),
            Some(expected_parent_flushes)
        );
    }
}

/// Keep an entire unacknowledged frame on disk and its descriptor visible.
fn stopped_after_second_frame(fixture: &Fixture) -> PreparedTranscriptStorage {
    let storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
    storage
        .commit_after(&transcript_event("seed", CONTENT), Some(0))
        .unwrap();
    CUT.set(Some(Cut::Sync));
    assert!(matches!(
        storage.commit_after(&transcript_event("leaf", CONTENT), Some(1)),
        Err(TranscriptCommitError::Io {
            phase: CanonicalAppendPhase::Sync,
            ..
        })
    ));
    storage
}

#[test]
fn startup_leaves_the_active_sessions_pending_lease_and_receipt_with_its_writer() {
    let fixture = Fixture::new();
    let active = stopped_after_second_frame(&fixture);
    let descriptor = fixture.path.with_added_extension("attempt");
    let before = fs::read(&descriptor).unwrap();
    crate::persistence::transcript_startup::recover_in_directory(
        fixture.path.parent().unwrap(),
        SESSION,
        |_, _, _| panic!("the active writer owns this receipt"),
    )
    .unwrap();
    assert_eq!(fs::read(descriptor).unwrap(), before);
    assert!(active.has_pending_attempt());
    active.resolve_pending().unwrap();
    let kept = active.take_recovered();
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].receipt.sequence, 2);
    assert_eq!(kept[0].event, transcript_event("leaf", CONTENT));
}

#[test]
fn contended_reader_with_unchanged_attempt_returns_its_verified_prefix() {
    use crate::persistence::canonical_durability::{
        transcript_coordination_path, wait_for_transcript_lock,
    };
    let fixture = Fixture::new();
    let storage = stopped_after_second_frame(&fixture);
    let lock = transcript_coordination_path(&fixture.path);
    let held = wait_for_transcript_lock(&lock, false, true).unwrap();
    let read = read_committed::<TranscriptEvent>(&fixture.path, SESSION).unwrap();
    assert_eq!(read.head.unwrap().sequence, 1);
    assert_eq!(read.records.len(), 1);
    assert_eq!(read.records[0].payload.span_id, "seed");
    drop(held);
    storage.resolve_pending().unwrap();
}

#[derive(Clone, Copy)]
enum AfterContendedCopy {
    CommitTwoMore,
    ReplaceAttempt,
}

fn check_contended_recapture(change: AfterContendedCopy) {
    use crate::persistence::canonical_durability::{
        observe_next_transcript_lock_wait, transcript_coordination_path, wait_for_transcript_lock,
    };
    let fixture = Fixture::new();
    let storage = stopped_after_second_frame(&fixture);
    let lock = transcript_coordination_path(&fixture.path);
    let held = wait_for_transcript_lock(&lock, false, true).unwrap();
    let (copied_tx, copied_rx) = mpsc::channel();
    let (resume_tx, resume_rx) = mpsc::channel();
    let (waiting_tx, waiting_rx) = mpsc::channel();
    let descriptor = fixture.path.with_added_extension("attempt");
    let attempt_id = || {
        serde_json::from_slice::<Intent>(&fs::read(&descriptor).unwrap())
            .unwrap()
            .attempt
    };
    let original_id = attempt_id();
    std::thread::scope(|scope| {
        let path = &fixture.path;
        let reader = scope.spawn(move || {
            observe_next_transcript_lock_wait(waiting_tx);
            AFTER_SOURCE_READ.set(Some(Box::new(move || {
                copied_tx.send(()).unwrap();
                resume_rx.recv_timeout(HANG_LIMIT).unwrap();
            })));
            read_committed::<TranscriptEvent>(path, SESSION).unwrap()
        });
        copied_rx.recv_timeout(HANG_LIMIT).unwrap();
        drop(held);
        storage.resolve_pending().unwrap();
        let third = transcript_event("stem", CONTENT);
        let expected = match change {
            AfterContendedCopy::CommitTwoMore => {
                storage.commit_after(&third, Some(2)).unwrap();
                storage
                    .commit_after(&transcript_event("root", CONTENT), Some(3))
                    .unwrap();
                assert!(!descriptor.try_exists().unwrap());
                4
            }
            AfterContendedCopy::ReplaceAttempt => {
                CUT.set(Some(Cut::Sync));
                assert!(matches!(
                    storage.commit_after(&third, Some(2)),
                    Err(TranscriptCommitError::Io {
                        phase: CanonicalAppendPhase::Sync,
                        ..
                    })
                ));
                assert_ne!(attempt_id(), original_id);
                2
            }
        };
        let held = wait_for_transcript_lock(&lock, false, true).unwrap();
        resume_tx.send(()).unwrap();
        waiting_rx
            .recv_timeout(HANG_LIMIT)
            .expect("reader requested a coordinated recapture");
        drop(held);
        let read = reader.join().unwrap();
        assert_eq!(read.head.unwrap().sequence, expected);
        assert_eq!(read.records.len() as u64, expected);
    });
    storage.resolve_pending().unwrap();
}

#[test]
fn contended_reader_waits_and_recaptures_when_the_descriptor_retires_after_copy() {
    check_contended_recapture(AfterContendedCopy::CommitTwoMore);
}

#[test]
fn contended_reader_waits_and_recaptures_when_a_different_attempt_replaces_the_first() {
    check_contended_recapture(AfterContendedCopy::ReplaceAttempt);
}

#[test]
fn first_writer_overlapping_an_unlocked_read_returns_a_consistent_prefix() {
    let fixture = Fixture::new();
    let path = fixture.path.clone();
    // The reader has observed absence and copied no source bytes. Let the
    // first writer publish, then leave the second frame torn before rechecking.
    AFTER_SOURCE_READ.set(Some(Box::new(move || {
        commit_transcript(&path, SESSION, &transcript_event("seed", CONTENT)).unwrap();
        CUT.set(Some(Cut::Short));
        assert!(commit_transcript(&path, SESSION, &transcript_event("leaf", CONTENT)).is_err());
    })));
    let snapshot = read_committed::<TranscriptEvent>(&fixture.path, SESSION).unwrap();
    assert_eq!(snapshot.head.unwrap().sequence, 1);
    assert_eq!(snapshot.records[0].payload.span_id, "seed");
    commit_transcript(&fixture.path, SESSION, &transcript_event("leaf", CONTENT)).unwrap();
}

#[test]
fn same_session_commit_finishes_while_the_reader_parses_the_previous_prefix() {
    thread_local! { static PARSING: std::cell::RefCell<Option<(mpsc::Sender<()>,mpsc::Receiver<()>)>> = const { std::cell::RefCell::new(None) }; }
    struct SlowRow;
    impl<'de> Deserialize<'de> for SlowRow {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            if let Some((entered, resume)) = PARSING.take() {
                entered.send(()).unwrap();
                resume.recv_timeout(HANG_LIMIT).unwrap();
            }
            TranscriptEvent::deserialize(deserializer)?;
            Ok(Self)
        }
    }
    let fixture = Fixture::new();
    let storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
    storage
        .commit_after(&transcript_event("seed", CONTENT), Some(0))
        .unwrap();
    let (entered, parsing) = mpsc::channel();
    let (resume, released) = mpsc::channel();
    std::thread::scope(|scope| {
        let path = &fixture.path;
        let reader = scope.spawn(move || {
            PARSING.set(Some((entered, released)));
            read_committed::<SlowRow>(path, SESSION)
                .unwrap()
                .head
                .unwrap()
                .sequence
        });
        parsing.recv_timeout(HANG_LIMIT).unwrap();
        let committed = storage.commit_after(&transcript_event("leaf", CONTENT), Some(1));
        resume.send(()).unwrap();
        assert_eq!(reader.join().unwrap(), 1);
        assert_eq!(committed.unwrap().sequence, 2);
    });
    assert_eq!(fixture.replay().head.unwrap().sequence, 2);
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
    let event = |sequence: u64| {
        let mut row = transcript_event(&format!("span-{sequence}"), "reed");
        row.start_time = sequence as f64;
        row.end_time = row.start_time + 0.5;
        row
    };
    // Optional load used to measure actual reader/writer interference. The
    // reader takes strict snapshots every 250 ms (or as fast as parsing allows).
    fn sample(path: PathBuf, with_reader: bool, write: impl FnOnce()) -> usize {
        if !with_reader {
            write();
            return 0;
        }
        let (stop, stopped) = mpsc::channel::<()>();
        let (start, ready) = mpsc::sync_channel(0);
        std::thread::scope(|scope| {
            let reader = scope.spawn(move || {
                start.send(()).unwrap();
                let mut completed = 0;
                loop {
                    let began = Instant::now();
                    read_committed::<TranscriptEvent>(&path, SESSION).unwrap();
                    completed += 1;
                    if !matches!(
                        stopped.recv_timeout(
                            Duration::from_millis(250).saturating_sub(began.elapsed())
                        ),
                        Err(mpsc::RecvTimeoutError::Timeout)
                    ) {
                        return completed;
                    }
                }
            });
            ready.recv().unwrap();
            write();
            drop(stop);
            reader.join().unwrap()
        })
    }
    let with_reader = std::env::var_os("AUDIOGRAPH_BENCH_SAME_SESSION_READER").is_some();
    struct Probe {
        fixture: Fixture,
        reads: usize,
        writer: TranscriptEventWriter,
        ledger: Arc<Mutex<TranscriptLedger>>,
        depth: u64,
        next: u64,
        solo: Vec<Duration>,
        burst: Vec<Duration>,
        burst_elapsed: Duration,
    }
    let mut probes = Vec::new();
    for depth in [100_u64, 10_000, 50_000] {
        let fixture = Fixture::new();
        let mut source = BufWriter::new(File::create(&fixture.path).unwrap());
        let mut ledger = TranscriptLedger::new(SESSION);
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
        for sequence in depth + 1..=depth + 16 {
            let (_, receipt) = client
                .append_live(&event(sequence), ledger.clone())
                .unwrap();
            assert_eq!(receipt.sequence, sequence);
        }
        probes.push(Probe {
            fixture,
            reads: 0,
            writer,
            ledger,
            depth,
            next: depth + 17,
            solo: Vec::with_capacity(256),
            burst: Vec::with_capacity(256),
            burst_elapsed: Duration::ZERO,
        });
    }
    // Rotate between depths in each round so that competing disk activity is
    // sampled by all three sessions instead of falling into one depth bucket.
    for round in 0..16 {
        for offset in 0..3 {
            let probe = &mut probes[(round + offset) % 3];
            let reads = sample(probe.fixture.path.clone(), with_reader, || {
                let client = probe.writer.client();
                for _ in 0..16 {
                    let revision = event(probe.next);
                    let started = Instant::now();
                    let (_, receipt) = client.append_live(&revision, probe.ledger.clone()).unwrap();
                    probe.solo.push(started.elapsed());
                    assert_eq!(receipt.sequence, probe.next);
                    probe.next += 1;
                }
                let burst_start = Instant::now();
                let pending: Vec<_> = (probe.next..probe.next + 16)
                    .map(|sequence| {
                        let revision = event(sequence);
                        let started = Instant::now();
                        let pending = client.submit_live(&revision, probe.ledger.clone()).unwrap();
                        (sequence, started, pending)
                    })
                    .collect();
                for (sequence, started, pending) in pending {
                    assert_eq!(pending.wait().unwrap().1.sequence, sequence);
                    probe.burst.push(started.elapsed());
                }
                probe.burst_elapsed += burst_start.elapsed();
                probe.next += 16;
            });
            probe.reads += reads;
        }
    }
    for mut probe in probes {
        probe.solo.sort_unstable();
        probe.burst.sort_unstable();
        let (groups, members) = probe.writer.group_counts();
        println!(
            "concurrent_reads depth={} completed={}",
            probe.depth, probe.reads
        );
        println!(
            "receipt_latency depth={} samples={} p50_us={} p95_us={}",
            probe.depth,
            probe.solo.len(),
            probe.solo[127].as_micros(),
            probe.solo[243].as_micros()
        );
        println!(
            "burst_latency depth={} samples={} p50_us={} p95_us={} elapsed_ms={} groups={} grouped_members={}",
            probe.depth,
            probe.burst.len(),
            probe.burst[127].as_micros(),
            probe.burst[243].as_micros(),
            probe.burst_elapsed.as_millis(),
            groups,
            members
        );
        assert!(probe.writer.shutdown_with_timeout(HANG_LIMIT));
    }
}

#[test]
fn descriptor_staging_write_fault_refuses_before_the_log_and_next_revision_can_commit() {
    use crate::persistence::canonical_durability::CanonicalDurabilityStage;

    for force_weak in [false, true] {
        let fixture = Fixture::new();
        let mut storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
        if force_weak {
            storage.tier = None;
        }
        let seed = transcript_event("seed", "first");
        storage.commit_after(&seed, Some(0)).unwrap();
        let prefix = fs::read(&fixture.path).unwrap();
        CUT.set(Some(Cut::IntentStageWrite));
        let error = storage
            .commit_after(&transcript_event("twig", "second"), Some(1))
            .unwrap_err();
        if storage.is_unqualified() {
            assert_eq!(
                error,
                io_failure(CanonicalAppendPhase::Write, io::ErrorKind::WriteZero.into())
            );
        } else {
            assert!(
                matches!(error, TranscriptCommitError::Indeterminate(e) if e.stage == CanonicalDurabilityStage::Write)
            );
        }
        let staging = fixture.path.with_added_extension("attempt-stage");
        let torn_descriptor = fs::read(&staging).unwrap();
        assert!(!torn_descriptor.is_empty());
        assert!(serde_json::from_slice::<serde_json::Value>(&torn_descriptor).is_err());
        assert!(
            !fixture
                .path
                .with_added_extension("attempt")
                .try_exists()
                .unwrap()
        );
        assert!(!storage.has_pending_attempt());
        assert_eq!(fs::read(&fixture.path).unwrap(), prefix);

        let next = transcript_event("leaf", "third");
        assert_eq!(storage.commit_after(&next, Some(1)).unwrap().sequence, 2);
        assert!(!staging.try_exists().unwrap());
        let events: Vec<_> = fixture
            .replay()
            .records
            .into_iter()
            .map(|row| row.payload)
            .collect();
        assert_eq!(events, [seed, next]);
    }
}

#[test]
fn shutdown_refuses_queued_live_replies_and_releases_the_abandoned_owner() {
    use crate::persistence::{TranscriptEventWriteMsg as Message, live_receipt, transcript_batch};
    use crate::projections::TranscriptLedger;
    use std::sync::{Mutex, atomic::AtomicBool};

    let fixture = Fixture::new();
    let storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
    let (tx, rx) = mpsc::sync_channel(3);
    tx.send(Message::Shutdown).unwrap();
    let ledger = Arc::new(Mutex::new(TranscriptLedger::new(SESSION)));
    let (result, late) = mpsc::channel();
    let owner: live_receipt::LatePublication = Arc::new(move |reply| result.send(reply).unwrap());
    let retained = Arc::downgrade(&owner);
    let (reply, waiting) = live_receipt::channel(HANG_LIMIT, None);
    tx.send(Message::Live(
        transcript_event("stem", "green"),
        ledger.clone(),
        reply,
    ))
    .unwrap();
    let (reply, abandoned) = live_receipt::channel(Duration::ZERO, Some(owner));
    tx.send(Message::Live(
        transcript_event("fern", "leaf"),
        ledger.clone(),
        reply,
    ))
    .unwrap();
    assert_eq!(abandoned.wait(), Err(TranscriptCommitError::AckTimeout));

    transcript_batch::run(
        rx,
        &storage,
        &fixture.path,
        SESSION,
        &AtomicBool::new(false),
    );
    assert_eq!(
        waiting.wait(),
        Err(TranscriptCommitError::UncommittedAttempt)
    );
    assert_eq!(
        late.try_recv().unwrap(),
        Err(TranscriptCommitError::UncommittedAttempt)
    );
    assert!(retained.upgrade().is_none());
    assert!(!fixture.path.try_exists().unwrap());
    assert_eq!(ledger.lock().unwrap().accepted_event_count, 0);
}

#[test]
fn a_failed_recovered_ledger_install_allows_the_next_revision_to_rebuild() {
    use crate::projections::TranscriptLedger;
    let fixture = Fixture::new();
    let writer = fixture.writer();
    let client = writer.client();
    let ledger = Arc::new(std::sync::Mutex::new(TranscriptLedger::new(SESSION)));
    let seed = transcript_event("oak", "first");
    let submit = |event: &TranscriptEvent| {
        client
            .submit_live_observed(event, ledger.clone(), HANG_LIMIT, None)
            .unwrap()
    };
    submit(&seed).wait().unwrap();
    let mut second = transcript_event("oak", "second");
    second.revision_number = seed.revision_number + 1;
    let (begin, cut, resume) = writer.pause_recovery_after_next_sync_failure();
    let pending = submit(&second);
    begin.send(()).unwrap();
    cut.recv_timeout(HANG_LIMIT).unwrap();
    assert_eq!(
        pending.wait(),
        Err(TranscriptCommitError::ConfirmationPending)
    );
    // The display row now conflicts with the kept frame at the next sequence.
    ledger.lock().unwrap().latest_spans[0].revision_number = second.revision_number + 1;
    let next = transcript_event("fern", "third");
    let ticket = submit(&next);
    resume.send(()).unwrap();
    assert_eq!(ticket.wait().unwrap().1.sequence, 3);
    assert_eq!(
        *ledger.lock().unwrap(),
        TranscriptLedger::replay(SESSION, [seed, second, next]).unwrap()
    );
    assert_eq!(fixture.replay().records.len(), 3);
    assert!(writer.shutdown_with_timeout(HANG_LIMIT));
}

#[test]
fn grouped_worker_replies_remain_pending_until_the_frame_sync_completes() {
    use crate::persistence::{TranscriptEventWriteMsg as Message, live_receipt, transcript_batch};
    use crate::projections::TranscriptLedger;
    use std::sync::{Mutex, atomic::AtomicBool};

    let fixture = Fixture::new();
    let storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
    let seed = transcript_event("seed", "first");
    storage.commit_after(&seed, Some(0)).unwrap();
    let ledger = Arc::new(Mutex::new(
        TranscriptLedger::replay(SESSION, [seed]).unwrap(),
    ));
    // Queue the entire group and its fence before starting the real worker.
    let (tx, rx) = mpsc::sync_channel(8);
    let mut tickets = Vec::new();
    for event in group_events() {
        let (reply, ticket) = live_receipt::channel(HANG_LIMIT, None);
        tx.send(Message::Live(event, ledger.clone(), reply))
            .unwrap();
        tickets.push(ticket);
    }
    let (counts, groups) = mpsc::channel();
    tx.send(Message::GroupCounts(counts)).unwrap();
    tx.send(Message::Shutdown).unwrap();
    let (entered, at_sync) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let path = fixture.path.clone();
    let worker = std::thread::spawn(move || {
        BEFORE_FRAME_SYNC.set(Some(Box::new(move || {
            entered.send(()).unwrap();
            resume.recv_timeout(HANG_LIMIT).unwrap();
        })));
        transcript_batch::run(rx, &storage, &path, SESSION, &AtomicBool::new(false));
    });
    at_sync.recv_timeout(HANG_LIMIT).unwrap();
    assert!(tickets.iter().all(|ticket| ticket.is_waiting()));
    assert_eq!(ledger.lock().unwrap().accepted_event_count, 1);
    release.send(()).unwrap();
    for (index, ticket) in tickets.into_iter().enumerate() {
        let (published, receipt) = ticket.wait().unwrap();
        assert!(published);
        assert_eq!(receipt.sequence, 2 + index as u64);
    }
    assert_eq!(groups.recv_timeout(HANG_LIMIT).unwrap().unwrap(), (1, 3));
    worker.join().unwrap();
    assert_eq!(fixture.replay().records.len(), 4);
    assert_eq!(ledger.lock().unwrap().accepted_event_count, 4);
}

#[test]
fn queued_live_revisions_share_a_receipt_barrier_and_publish_in_order() {
    use crate::projections::TranscriptLedger;
    let fixture = Fixture::new();
    let writer = fixture.writer();
    let client = writer.client();
    let ledger = Arc::new(std::sync::Mutex::new(TranscriptLedger::new(SESSION)));
    client
        .append_live(&transcript_event("seed", CONTENT), ledger.clone())
        .unwrap();
    let held = ledger.lock().unwrap();
    let pending: Vec<_> = (1..=8)
        .map(|n| {
            client
                .submit_live(
                    &transcript_event(&format!("leaf-{n}"), CONTENT),
                    ledger.clone(),
                )
                .unwrap()
        })
        .collect();
    assert_eq!(held.accepted_event_count, 1);
    assert_eq!(
        read_committed::<TranscriptEvent>(&fixture.path, SESSION)
            .unwrap()
            .records
            .len(),
        1
    );
    drop(held);
    for (i, ticket) in pending.into_iter().enumerate() {
        let (published, receipt) = ticket.wait().unwrap();
        assert!(published);
        assert_eq!(receipt.sequence, 2 + i as u64);
    }
    let snapshot = read_committed::<TranscriptEvent>(&fixture.path, SESSION).unwrap();
    assert_eq!(snapshot.records.len(), 9);
    assert_eq!(ledger.lock().unwrap().accepted_event_count, 9);
    let (groups, members) = writer.group_counts();
    assert!(
        groups >= 1 && members >= 2,
        "the burst shared a storage transaction"
    );
    assert!(writer.shutdown_with_timeout(HANG_LIMIT));
}

#[test]
fn a_late_commit_delivers_its_own_receipt_without_another_revision() {
    use crate::projections::TranscriptLedger;
    let fixture = Fixture::new();
    let writer = fixture.writer();
    let client = writer.client();
    let live = Arc::new(std::sync::Mutex::new(TranscriptLedger::new(SESSION)));
    let held = live.lock().unwrap();
    let (late, observed) = mpsc::channel();
    let ticket = client
        .submit_live_observed(
            &transcript_event("elm", "first"),
            live.clone(),
            Duration::ZERO,
            Some(Arc::new(move |result| late.send(result).unwrap())),
        )
        .unwrap();
    assert_eq!(ticket.wait(), Err(TranscriptCommitError::AckTimeout));
    drop(held);
    let (published, receipt) = observed.recv_timeout(HANG_LIMIT).unwrap().unwrap();
    assert!(published);
    assert_eq!(receipt.sequence, 1);
    assert_eq!(live.lock().unwrap().accepted_event_count, 1);
    assert_eq!(fixture.replay().records.len(), 1);
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
        let other = "session-pear";
        let path = f.path.with_file_name(format!("{other}.events.jsonl"));
        assert_eq!(commit_transcript(&path, other, &first).unwrap().sequence, 1);
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

#[test]
fn exact_first_create_retry_keeps_its_namespace_receipt() {
    let f = Fixture::new();
    let prepared = PreparedTranscriptStorage::open(&f.path, SESSION).unwrap();
    let event = transcript_event("leaf", "first");
    CUT.set(Some(Cut::Full));
    assert!(
        prepared
            .commit_after(&event, Some(0))
            .unwrap_err()
            .is_storage_full()
    );
    assert!(f.path.exists());
    let receipt = prepared.commit_after(&event, Some(0)).unwrap();
    assert_eq!(receipt.sequence, 1);
    let expected = if prepared.is_unqualified() {
        CanonicalDurabilityBarrier::SyncedUnqualified
    } else if cfg!(windows) {
        CanonicalDurabilityBarrier::FileAndWindowsNtfsParentFlush
    } else {
        CanonicalDurabilityBarrier::FileAndParentNamespace
    };
    assert_eq!(receipt.barrier, expected);
}

#[test]
fn a_failed_tier_witness_keeps_the_receipt_and_retries_the_verified_head() {
    for force_weak in [false, true] {
        let fixture = Fixture::new();
        let mut storage = PreparedTranscriptStorage::open(&fixture.path, SESSION).unwrap();
        if force_weak {
            storage.tier = None;
        }
        CUT.set(Some(Cut::WitnessWrite));
        let first = storage
            .commit_after(&transcript_event("seed", CONTENT), Some(0))
            .unwrap();
        assert_eq!(first.sequence, 1);
        assert!(!storage.has_pending_attempt());
        assert!(
            !fixture
                .path
                .with_added_extension("attempt")
                .try_exists()
                .unwrap()
        );
        let records = fixture.replay().records;
        assert_eq!(records.len(), 1);
        assert!(tier::receipt_evidence(&fixture.path, &records).is_empty());
        let next = storage
            .commit_after(&transcript_event("leaf", CONTENT), Some(1))
            .unwrap();
        assert_eq!(next.sequence, 2);
        let evidence = tier::receipt_evidence(&fixture.path, &fixture.replay().records);
        let witnessed = if storage.is_unqualified() {
            evidence.synced_unqualified
        } else {
            evidence.qualified
        };
        assert_eq!(
            witnessed.unwrap(),
            VerifiedTranscriptHead {
                sequence: 1,
                record_hash: records[0].record_hash.clone()
            }
        );
    }
}

#[test]
fn tier_witness_distinguishes_a_kept_weak_origin_from_a_successful_weak_sync() {
    let f = Fixture::new();
    let first = transcript_event("leaf", CONTENT);
    let second = transcript_event("root", CONTENT);
    let s = PreparedTranscriptStorage::open(&f.path, SESSION).unwrap();
    let originally_qualified = !s.is_unqualified();
    s.commit_after(&first, Some(0)).unwrap();
    let initial = f.replay();
    let mut weak = PreparedTranscriptStorage::open(&f.path, SESSION).unwrap();
    weak.tier = None;
    CUT.set(Some(Cut::Sync));
    assert!(weak.commit_after(&second, Some(1)).is_err());
    let prefix_evidence = tier::receipt_evidence(&f.path, &initial.records);
    if originally_qualified {
        assert!(prefix_evidence.synced_unqualified.is_none());
        assert_eq!(prefix_evidence.qualified.as_ref().unwrap().sequence, 1);
    }
    let descriptor = f.path.with_added_extension("attempt");
    let origin = f.path.with_added_extension("attempt-origin");
    let bytes = fs::read(&descriptor).unwrap();
    let intent: Intent = serde_json::from_slice(&bytes).unwrap();
    // Old floor-v1 readers reject unknown descriptor fields.
    let old_keys = [
        "attempt",
        "owner",
        "after",
        "base_len",
        "source_was_absent",
        "frame_len",
        "frame_hash",
        "members",
    ];
    let wire: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        wire.as_object()
            .unwrap()
            .keys()
            .all(|k| old_keys.contains(&k.as_str()))
    );
    if originally_qualified {
        assert_eq!(fs::read(&origin).unwrap(), intent.attempt.as_bytes());
        let (_, ongoing) =
            super::super::transcript_export::read_for_export(&f.path, 65536, || Ok(())).unwrap();
        assert_eq!(
            ongoing.unwrap().unqualified_attempt_id.as_deref(),
            Some(intent.attempt.as_str())
        );
    }
    let resumed = PreparedTranscriptStorage::open(&f.path, SESSION).unwrap();
    resumed.resolve_pending().unwrap();
    let rows = f.replay().records;
    let evidence = tier::receipt_evidence(&f.path, &rows);
    if originally_qualified {
        let kept = evidence.recovered_from_unqualified_attempt.unwrap();
        assert_eq!(kept.sequence, 2);
        assert_eq!(kept.record_hash, rows[1].record_hash);
        assert!(evidence.synced_unqualified.is_none());
    } else {
        assert_eq!(evidence.synced_unqualified.unwrap().sequence, 1);
        assert!(evidence.recovered_from_unqualified_attempt.is_none());
    }
    assert!(!origin.exists() && !descriptor.exists());
}

#[test]
fn abandoned_weak_witness_is_replaced_and_subsequent_receipts_reuse_verified_evidence() {
    let f = Fixture::new();
    let mut s = PreparedTranscriptStorage::open(&f.path, SESSION).unwrap();
    s.tier = None;
    CUT.set(Some(Cut::Short));
    assert!(
        s.commit_after(&transcript_event("seed", CONTENT), Some(0))
            .is_err()
    );
    assert!(tier::receipt_evidence::<TranscriptEvent>(&f.path, &[]).is_empty());
    s.resolve_pending().unwrap();
    s.commit_after(&transcript_event("twig", CONTENT), Some(0))
        .unwrap();
    let marker = f.path.with_added_extension("unqualified-receipt");
    let observed = fs::read(&marker).unwrap();
    let first = f.replay().records;
    assert_eq!(
        tier::receipt_evidence(&f.path, &first)
            .synced_unqualified
            .unwrap()
            .record_hash,
        first[0].record_hash
    );
    for (prior, span) in [(1, "root"), (2, "stem")] {
        s.commit_after(&transcript_event(span, CONTENT), Some(prior))
            .unwrap();
    }
    assert_eq!(fs::read(&marker).unwrap(), observed);
    let inventory = transcript_recovery_artifacts(&f.path).unwrap();
    assert!(inventory.contains(&marker));
}

#[test]
fn legacy_frames_and_unreadable_evidence_make_no_positive_qualification_claim() {
    let f = Fixture::new();
    let original = transcript_event("seed", CONTENT);
    fs::write(&f.path, serde_json::to_vec(&original).unwrap()).unwrap();
    let records = f.replay().records;
    assert!(tier::receipt_evidence(&f.path, &records).is_empty());
    fs::write(f.path.with_added_extension("qualified-receipt"), b"plum").unwrap();
    let evidence = tier::receipt_evidence(&f.path, &records);
    assert!(evidence.evidence_unavailable);
    assert!(evidence.qualified.is_none() && evidence.synced_unqualified.is_none());
}

#[test]
fn identical_redelivery_after_a_torn_or_empty_weak_attempt_never_claims_a_weak_sync() {
    for cut in [Cut::Short, Cut::Full] {
        let f = Fixture::new();
        let qualified = PreparedTranscriptStorage::open(&f.path, SESSION).unwrap();
        if qualified.is_unqualified() {
            return;
        }
        let mut weak = PreparedTranscriptStorage::open(&f.path, SESSION).unwrap();
        weak.tier = None;
        let event = transcript_event("twig", CONTENT);
        CUT.set(Some(cut));
        assert!(weak.commit_after(&event, Some(0)).is_err());
        let intent: Intent =
            serde_json::from_slice(&fs::read(f.path.with_added_extension("attempt")).unwrap())
                .unwrap();
        let origin = f.path.with_added_extension("attempt-origin");
        assert_eq!(fs::read(&origin).unwrap(), intent.attempt.as_bytes());
        assert!(!f.path.with_added_extension("unqualified-receipt").exists());
        let receipt = qualified.commit_after(&event, Some(0)).unwrap();
        assert_eq!(receipt.sequence, 1);
        assert_eq!(digest(&fs::read(&f.path).unwrap()), intent.frame_hash);
        let evidence = tier::receipt_evidence(&f.path, &f.replay().records);
        assert_eq!(evidence.qualified.unwrap().sequence, 1);
        assert!(
            evidence.synced_unqualified.is_none()
                && evidence.recovered_from_unqualified_attempt.is_none()
        );
        assert!(!origin.exists());
    }
}

#[test]
fn exact_retry_records_the_new_barrier_and_stale_origin_ids_cannot_label_recovery() {
    let f = Fixture::new();
    let native = PreparedTranscriptStorage::open(&f.path, SESSION).unwrap();
    let origin = f.path.with_added_extension("attempt-origin");
    fs::write(&origin, Uuid::new_v4().to_string()).unwrap();
    native.ready().unwrap();
    assert!(!origin.exists());
    if native.is_unqualified() {
        return;
    }
    let mut weak = PreparedTranscriptStorage::open(&f.path, SESSION).unwrap();
    weak.tier = None;
    let first = transcript_event("leaf", CONTENT);
    native.commit_after(&first, Some(0)).unwrap();
    let retried = weak.commit_after(&first, Some(0)).unwrap();
    assert!(retried.already_accepted);
    let evidence = tier::receipt_evidence(&f.path, &f.replay().records);
    assert_eq!(evidence.qualified, evidence.synced_unqualified);
    assert!(evidence.qualified.is_some() && evidence.recovered_from_unqualified_attempt.is_none());
    // A previous weak receipt already witnesses the stream. Another weak
    // attempt must not add a per-event origin publication.
    CUT.set(Some(Cut::Sync));
    assert!(
        weak.commit_after(&transcript_event("stem", CONTENT), Some(1))
            .is_err()
    );
    assert!(!origin.exists());
    fs::write(&origin, Uuid::new_v4().to_string()).unwrap();
    let (_, ongoing) =
        super::super::transcript_export::read_for_export(&f.path, 65536, || Ok(())).unwrap();
    assert!(ongoing.unwrap().unqualified_attempt_id.is_none());
    native.resolve_pending().unwrap();
    let evidence = tier::receipt_evidence(&f.path, &f.replay().records);
    assert!(evidence.recovered_from_unqualified_attempt.is_none());
    assert!(!origin.exists());
}
