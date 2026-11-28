use super::*;
use crate::persistence::shutdown_tests::transcript_event;
use crate::projections::{ProjectionBasis, ProjectionKind};
const SESSION: &str = "session-orchard";

fn projection_patch(sequence: u64, note_id: &str) -> ProjectionPatch {
    ProjectionPatch {
        route: None,
        sequence,
        kind: crate::projections::ProjectionKind::Notes,
        llm_request_id: format!("llm-request-{sequence}"),
        basis: crate::projections::ProjectionBasis {
            span_revisions: vec![crate::projections::ProjectionBasisSpan {
                span_id: "span-1".into(),
                revision_number: sequence,
            }],
            covered_prefix: None,
            diarization_span_revisions: Vec::new(),
            transcript_hash: format!("fnv1a64:{sequence:016x}"),
            summarized_through_revision: None,
        },
        operations: vec![crate::projections::ProjectionOperation::UpsertNote {
            id: note_id.into(),
            title: "Decision".into(),
            body: "Persist projection patches.".into(),
            tags: vec!["decision".into()],
            evidence: crate::claim_evidence::EvidenceAnchor::default(),
            heading_level: None,
        }],
        confidence: 0.9,
        provenance: crate::projections::ProjectionProvenance {
            provider: "openrouter".into(),
            model: "anthropic/claude-sonnet-4".into(),
            prompt_id: "notes-v1".into(),
            route_id: None,
            model_source: crate::llm::route::ModelIdentitySource::Requested,
        },
        queued_at_ms: None,
        generation_latency_ms: None,
        apply_latency_ms: None,
        basis_currency_at_apply: None,
        created_at_ms: 1_700_000_000_000 + sequence,
    }
}

struct Fixture {
    root: PathBuf,
    transcript: PreparedTranscriptStorage,
    projection: PreparedProjectionStorage,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(Uuid::new_v4().to_string());
        let path = |dir: &str| root.join(dir).join(format!("{SESSION}.events.jsonl"));
        let transcript = PreparedTranscriptStorage::open(&path("transcripts"), SESSION).unwrap();
        let projection = PreparedProjectionStorage::open(&path("projections"), SESSION).unwrap();
        Self {
            root,
            transcript,
            projection,
        }
    }
    fn seeded_patch(&self) -> ProjectionPatch {
        let event = transcript_event("branch", "wood");
        self.transcript.commit_after(&event, None).unwrap();
        let mut patch = projection_patch(1, "note");
        patch.basis = ProjectionBasis::from_transcript_events(&[event]);
        patch
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
#[test]
fn projections_commit_in_physical_order_and_keep_the_original_basis_on_exact_retry() {
    let fixture = Fixture::new();
    let notes = fixture.seeded_patch();
    let receipt = fixture.projection.commit_after(&notes, Some(1)).unwrap();
    assert_eq!(receipt.head.sequence, 1);
    fixture
        .transcript
        .commit_after(&transcript_event("stem", "pond"), None)
        .unwrap();
    assert!(
        fixture
            .projection
            .commit_after(&notes, None)
            .unwrap()
            .already_accepted
    );
    let mut graph = notes.clone();
    graph.kind = ProjectionKind::Graph;
    graph.operations.clear();
    let second = fixture.projection.commit_after(&graph, Some(2)).unwrap();
    assert_eq!(second.head.sequence, 2);
    let verified =
        read_committed_projection::<ProjectionPatch>(&fixture.projection.path, SESSION).unwrap();
    assert_eq!(verified.records[0].basis_heads[STREAM].sequence, 1);
    assert_eq!(verified.records[1].basis_heads[STREAM].sequence, 2);
    assert_eq!(verified.records[1].record_hash, second.head.record_hash);
}
#[test]
fn verified_projection_member_with_a_lost_transcript_basis_is_quarantined() {
    let fixture = Fixture::new();
    let patch = fixture.seeded_patch();
    CUT.set(Some(Cut::Sync));
    assert!(fixture.projection.commit_after(&patch, Some(1)).is_err());
    let suffix = fs::read(&fixture.projection.path).unwrap();
    // Replace transcript history with an equal-height, different verified head.
    fs::remove_file(&fixture.transcript.path).unwrap();
    fixture
        .transcript
        .commit_after(&transcript_event("stem", "grass"), None)
        .unwrap();
    fixture.projection.resolve_pending().unwrap();
    assert!(fixture.projection.take_recovered().is_empty());
    let gaps = transcript_gaps(&fixture.projection.path).unwrap();
    assert_eq!(gaps.len(), 1);
    assert_eq!(gaps[0].stream, GapStream::ProjectionPatches);
    assert_eq!(gaps[0].projection_lanes, [ProjectionKind::Notes]);
    assert_eq!(gaps[0].after, None);
    let evidence = transcript_recovery_artifacts(&fixture.projection.path)
        .unwrap()
        .into_iter()
        .find(|p| p.extension().is_some_and(|e| e == "quarantine"))
        .unwrap();
    assert_eq!(fs::read(evidence).unwrap(), suffix);
    assert!(!fixture.projection.path.exists());
}

pub(super) fn interrupt_patch(path: &Path, checkpoint: &str) -> ! {
    let storage = PreparedProjectionStorage::open(path, SESSION).unwrap();
    let mut patch = projection_patch(1, "note");
    patch.basis = ProjectionBasis::from_transcript_events(&[transcript_event("branch", "wood")]);
    CUT.set(Some(match checkpoint {
        "complete" => Cut::KillComplete,
        "torn" => Cut::KillPartial,
        _ => panic!("unknown projection checkpoint"),
    }));
    let result = storage.commit_after(&patch, Some(1));
    panic!("projection returned before its checkpoint: {result:?}");
}

#[test]
fn startup_recovers_the_transcript_before_a_projection_attempt_and_replays_each_kept_patch_once() {
    use crate::persistence::transcript_startup::{
        recover_in_directory, recover_projection_directory,
    };
    for (checkpoint, keep) in [("projection-complete", true), ("projection-torn", false)] {
        let f = Fixture::new();
        let patch = f.seeded_patch();
        super::tests::kill_at_writer_checkpoint(&f.projection.path, checkpoint, || {});
        let descriptor = f.projection.path.with_added_extension("attempt");
        let interrupted = fs::read(&f.projection.path).unwrap();
        super::tests::kill_at_writer_checkpoint(&f.transcript.path, "complete", || {});
        // An unresolved transcript attempt postpones decisions on its dependent log.
        recover_projection_directory(&f.root).unwrap();
        assert!(descriptor.exists());
        assert_eq!(fs::read(&f.projection.path).unwrap(), interrupted);
        let mut transcript_receipts = Vec::new();
        for _ in 0..2 {
            recover_in_directory(&f.root.join("transcripts"), |_, _, receipt| {
                transcript_receipts.push(receipt.sequence)
            })
            .unwrap();
            recover_projection_directory(&f.root).unwrap();
        }
        assert_eq!(transcript_receipts, [2]);
        assert!(!descriptor.exists());
        let gaps = transcript_gaps(&f.projection.path).unwrap();
        if keep {
            let stored =
                read_committed_projection::<ProjectionPatch>(&f.projection.path, SESSION).unwrap();
            assert_eq!(stored.records.len(), 1);
            assert_eq!(stored.records[0].payload, patch);
            assert!(gaps.is_empty());
        } else {
            assert_eq!(gaps.len(), 1);
            assert_eq!(gaps[0].projection_lanes, [ProjectionKind::Notes]);
            assert_eq!(gaps[0].after, None);
            let files = transcript_recovery_artifacts(&f.projection.path).unwrap();
            let evidence: Vec<_> = files
                .iter()
                .filter(|p| p.extension().is_some_and(|e| e == "quarantine"))
                .collect();
            assert_eq!(evidence.len(), 1);
            assert_eq!(fs::read(evidence[0]).unwrap(), interrupted);
            let next = f.projection.commit_after(&patch, Some(1)).unwrap();
            assert_eq!(next.head.sequence, 1);
            assert!(!next.already_accepted);
        }
    }
}

#[test]
fn an_unsettled_transcript_suffix_allows_a_patch_against_its_verified_prefix() {
    let fixture = Fixture::new();
    let patch = fixture.seeded_patch();
    CUT.set(Some(Cut::Sync));
    assert!(
        fixture
            .transcript
            .commit_after(&transcript_event("tree", "shade"), Some(1))
            .is_err()
    );
    assert!(fixture.transcript.has_pending_attempt());
    let receipt = fixture.projection.commit_after(&patch, Some(1)).unwrap();
    assert_eq!(receipt.head.sequence, 1);
    let stored =
        read_committed_projection::<ProjectionPatch>(&fixture.projection.path, SESSION).unwrap();
    assert_eq!(stored.records[0].basis_heads[STREAM].sequence, 1);
    fixture.transcript.resolve_pending().unwrap();
}

#[test]
fn late_projection_retry_cannot_skip_a_preceding_or_subsequent_commit_in_its_lane() {
    let f = Fixture::new();
    let mut row = f.seeded_patch();
    f.projection.commit_after(&row, Some(1)).unwrap();
    row.sequence = 3;
    f.projection.commit_after(&row, Some(1)).unwrap();
    assert_eq!(
        f.projection.commit_at_lane(&row, Some(1), &[], Some(0)),
        Err(TranscriptCommitError::LiveHeadChanged)
    );
    assert!(
        f.projection
            .commit_at_lane(&row, Some(1), &[], Some(1))
            .unwrap()
            .already_accepted
    );
    let prior = row.clone();
    row.sequence = 5;
    f.projection.commit_after(&row, Some(1)).unwrap();
    assert_eq!(
        f.projection.commit_at_lane(&prior, Some(1), &[], Some(1)),
        Err(TranscriptCommitError::LiveHeadChanged)
    );
}

#[test]
fn hidden_first_transcript_attempt_can_neither_admit_nor_quarantine_a_dependent_projection() {
    let fixture = Fixture::new();
    let event = transcript_event("tree", "shade");
    CUT.set(Some(Cut::Sync));
    assert!(fixture.transcript.commit_after(&event, Some(0)).is_err());
    let mut row = projection_patch(1, "leaf");
    row.basis = ProjectionBasis::from_transcript_events(&[event]);
    assert!(
        capture_committed(&fixture.transcript.path, SESSION)
            .unwrap()
            .is_none()
    );
    // Retire the descriptor strictly between capture and dependency
    // classification. A fresh stat here would report no pending attempt.
    let source = fixture.transcript.path.clone();
    let observed = std::rc::Rc::new(std::cell::Cell::new(false));
    let mark = observed.clone();
    domain::AFTER_BASIS_CAPTURE.set(Some(Box::new(move || {
        let before = source.with_added_extension("attempt");
        assert!(before.exists());
        PreparedTranscriptStorage::open(&source, SESSION)
            .unwrap()
            .resolve_pending()
            .unwrap();
        assert!(!before.exists());
        mark.set(true);
    })));
    assert_eq!(
        fixture.projection.commit_after(&row, Some(1)),
        Err(TranscriptCommitError::ConflictingAttempt)
    );
    assert!(!fixture.projection.path.exists());
    assert!(observed.get());
    assert_eq!(
        fixture
            .projection
            .commit_after(&row, Some(1))
            .unwrap()
            .head
            .sequence,
        1
    );
}
