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
    let commit = Commit::open(&session.log, SESSION).unwrap();
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
