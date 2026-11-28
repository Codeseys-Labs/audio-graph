use super::*;
use crate::persistence::shutdown_tests::transcript_event;

const SESSION: &str = "orchard";

fn event(revision: u64) -> TranscriptEvent {
    TranscriptEvent {
        revision_number: revision,
        ..transcript_event("branch", "quiet pond")
    }
}

struct Session {
    root: PathBuf,
    log: PathBuf,
}

impl Session {
    fn new() -> Self {
        let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
        let directory = root.join("transcripts");
        fs::create_dir_all(&directory).unwrap();
        Self {
            log: directory.join(format!("{SESSION}.events.jsonl")),
            root,
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn a_stale_live_ledger_cannot_append_past_a_committed_sequence() {
    let session = Session::new();
    commit_after(&session.log, SESSION, &event(1), Some(0)).unwrap();
    let committed = fs::read(&session.log).unwrap();
    assert_eq!(
        commit_after(&session.log, SESSION, &event(2), Some(0)),
        Err(TranscriptCommitError::LiveHeadChanged),
    );
    assert_eq!(fs::read(&session.log).unwrap(), committed);
    assert!(!session.log.with_added_extension("attempt").exists());
    assert_eq!(
        commit_after(&session.log, SESSION, &event(2), Some(1))
            .unwrap()
            .sequence,
        2
    );
}

#[test]
fn invalid_stale_or_already_accepted_callbacks_do_not_abandon_an_uncertain_live_revision() {
    let session = Session::new();
    let commit = |revision: &TranscriptEvent, head| {
        commit_after(&session.log, SESSION, revision, Some(head))
    };
    commit(&event(1), 0).unwrap();
    CUT.set(Some(Cut::Short));
    assert!(commit(&event(2), 1).is_err());
    let pending = fs::read(&session.log).unwrap();
    let malformed = TranscriptEvent {
        confidence: f32::INFINITY,
        ..event(3)
    };
    assert_eq!(
        commit(&malformed, 1),
        Err(TranscriptCommitError::InvalidRevision)
    );
    assert_eq!(
        commit(&event(3), 0),
        Err(TranscriptCommitError::LiveHeadChanged)
    );
    assert!(commit(&event(1), 1).unwrap().already_accepted);
    assert_eq!(fs::read(&session.log).unwrap(), pending);
    assert!(session.log.with_added_extension("attempt").is_file());
    assert_eq!(commit(&event(2), 1).unwrap().sequence, 2);
    assert!(transcript_gaps(&session.log).unwrap().is_empty());
}

#[test]
fn a_synced_frame_survives_failure_on_either_side_of_descriptor_removal() {
    for cut in [Cut::Retire, Cut::RemovedWithoutBarrier] {
        let session = Session::new();
        commit_after(&session.log, SESSION, &event(1), Some(0)).unwrap();
        CUT.set(Some(cut));
        assert!(matches!(
            commit_after(&session.log, SESSION, &event(2), Some(1)),
            Err(TranscriptCommitError::Io {
                phase: CanonicalAppendPhase::RecoverySync,
                ..
            })
        ));
        assert_eq!(
            session.log.with_added_extension("attempt").exists(),
            cut == Cut::Retire
        );
        // A caller still at sequence one must first publish the verified second
        // event in live memory before trying to advance with a third revision.
        assert_eq!(
            commit_after(&session.log, SESSION, &event(3), Some(1)),
            Err(TranscriptCommitError::LiveHeadChanged)
        );
        let recovered = read_committed::<TranscriptEvent>(&session.log, SESSION).unwrap();
        assert_eq!(recovered.records.len(), 2);
        assert_eq!(recovered.records[1].payload.revision_number, 2);
        assert_eq!(
            commit_after(&session.log, SESSION, &event(3), Some(2))
                .unwrap()
                .sequence,
            3
        );
        assert!(transcript_gaps(&session.log).unwrap().is_empty());
    }
}

#[test]
fn nonfinite_revisions_and_unseen_stale_revisions_cannot_own_a_sequence() {
    let session = Session::new();
    let commit = |event: &TranscriptEvent| commit_transcript(&session.log, SESSION, event);
    for invalid in [
        TranscriptEvent {
            start_time: f64::NAN,
            ..event(1)
        },
        TranscriptEvent {
            end_time: f64::INFINITY,
            ..event(1)
        },
        TranscriptEvent {
            confidence: f32::NEG_INFINITY,
            ..event(1)
        },
    ] {
        assert_eq!(
            commit(&invalid),
            Err(TranscriptCommitError::InvalidRevision)
        );
        assert!(!session.log.exists());
        assert!(!session.log.with_added_extension("attempt").exists());
    }
    assert_eq!(commit(&event(1)).unwrap().sequence, 1);
    assert_eq!(commit(&event(3)).unwrap().sequence, 2);
    let before = fs::read(&session.log).unwrap();
    assert_eq!(
        commit(&event(2)),
        Err(TranscriptCommitError::InvalidRevision)
    );
    let retry = commit(&event(1)).unwrap();
    assert!(retry.already_accepted);
    assert_eq!(retry.sequence, 1);
    assert_eq!(fs::read(&session.log).unwrap(), before);
    assert_eq!(commit(&event(4)).unwrap().sequence, 3);
}

#[test]
fn restart_refuses_a_matching_complete_frame_that_cannot_replay() {
    let session = Session::new();
    commit_transcript(&session.log, SESSION, &event(2)).unwrap();
    let bytes = fs::read(&session.log).unwrap();
    let prepared = PreparedTranscriptStorage::open(&session.log, SESSION).unwrap();
    let commit = Commit::open(&prepared).unwrap();
    let (cache, _) = commit.cache(&bytes).unwrap();
    let basis = FrameBasis {
        session_id: SESSION,
        stream_id: STREAM,
        domain_schema_version: 1,
        head: cache.head.as_ref(),
        events: &cache.events,
        byte_len: cache.byte_len,
        ends_with_newline: cache.ends_with_newline,
        poisoned: None,
    };
    let stale = event(1);
    let PlannedAppend::New(frame) = basis.plan(
        &CanonicalEventMetadata::new(revision_id(&stale).unwrap()),
        &stale,
    ) else {
        panic!("expected a structurally valid frame");
    };
    let intent = Intent {
        attempt: Uuid::new_v4().to_string(),
        owner: Uuid::new_v4().to_string(),
        source_was_absent: false,
        after: head(&cache),
        base_len: cache.byte_len,
        frame_len: frame.frame.len() as u64,
        frame_hash: digest(&frame.frame),
    };
    let descriptor = session.log.with_added_extension("attempt");
    fs::write(&descriptor, serde_json::to_vec(&intent).unwrap()).unwrap();
    OpenOptions::new()
        .append(true)
        .open(&session.log)
        .unwrap()
        .write_all(&frame.frame)
        .unwrap();
    drop(commit);

    assert_eq!(
        ready(&session.log, SESSION),
        Err(TranscriptCommitError::InvalidHistory)
    );
    assert!(
        descriptor.is_file(),
        "recovery must keep the unresolved intent"
    );
    assert!(transcript_gaps(&session.log).unwrap().is_empty());
}

#[test]
fn steady_state_appends_reuse_the_verified_generation() {
    let session = Session::new();
    let writer = PreparedTranscriptStorage::open(&session.log, SESSION).unwrap();
    writer.ready().unwrap();
    writer.commit_after(&event(1), Some(0)).unwrap();
    let initial = PREFIX_SCANS.get();
    for revision in 2..=20 {
        writer
            .commit_after(&event(revision), Some(revision - 1))
            .unwrap();
    }
    assert_eq!(PREFIX_SCANS.get(), initial);
}
