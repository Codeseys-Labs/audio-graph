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
fn moving_transcript_and_speaker_bases_verify_only_new_frames() {
    use crate::{
        identity::tests::span,
        persistence::speaker_log::{SpeakerWrite, append},
        projections::SpeakerTimeline,
    };
    let fixture = Fixture::new();
    let mut patch = fixture.seeded_patch();
    let path = fixture
        .transcript
        .path
        .with_file_name(format!("{SESSION}.speaker.jsonl"));
    let speaker = span("reed", "mic");
    append(
        &path,
        SESSION,
        SpeakerWrite::SpanRevision { revision: &speaker },
    )
    .unwrap();
    let mut timeline = SpeakerTimeline::replay(SESSION, [speaker]).unwrap();
    patch.basis.diarization_span_revisions = timeline.current_basis_spans();
    fixture.projection.commit_after(&patch, Some(1)).unwrap();
    let before = fixture.projection.source_basis.borrow().source_work();
    let next = transcript_event("twig", "leaf");
    fixture.transcript.commit_after(&next, None).unwrap();
    let speaker = span("fern", "mic");
    append(
        &path,
        SESSION,
        SpeakerWrite::SpanRevision { revision: &speaker },
    )
    .unwrap();
    timeline.apply_event(speaker).unwrap();
    patch.sequence = 2;
    patch.basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
        &[transcript_event("branch", "wood"), next],
        &timeline.current_basis_spans(),
    );
    fixture.projection.commit_after(&patch, Some(2)).unwrap();
    let after = fixture.projection.source_basis.borrow().source_work();
    assert_eq!(
        after,
        [
            (before[0].0, before[0].1 + 1),
            (before[1].0, before[1].1 + 1)
        ]
    );
}

#[test]
fn live_basis_matches_ledger_currency_and_cited_evidence_as_revisions_change() {
    use crate::projections::{BasisCurrency, TranscriptEventStability, TranscriptLedger};
    let f = Fixture::new();
    let mut ledger = TranscriptLedger::new(SESSION);
    let mut patch = projection_patch(1, "leaf");
    let crate::projections::ProjectionOperation::UpsertNote { evidence, .. } =
        &mut patch.operations[0]
    else {
        unreachable!()
    };
    // The claim-evidence gate trims a cited ID before looking up its span.
    evidence.span_id = Some("  reed-1  ".into());
    let mut older = Vec::new();
    for n in 0..13 {
        let mut event = transcript_event(&format!("reed-{n}"), "leaf");
        event.start_time = n as f64;
        if n % 3 == 0 {
            event.is_final = false;
            event.end_of_turn = false;
            event.stability = TranscriptEventStability::Partial;
        }
        let mut changes = vec![event];
        if n == 10 {
            let mut correction = transcript_event("reed-1", "stem");
            correction.start_time = 1.;
            correction.revision_number = 2;
            changes.push(correction);
        }
        for row in changes {
            f.transcript.commit_after(&row, None).unwrap();
            ledger.apply_event(row).unwrap();
        }
        older.push(ledger.current_basis());
        older.push(ledger.current_projection_basis());
        for basis in &older {
            patch.basis = basis.clone();
            let proof = f
                .projection
                .live_basis(&patch, ledger.accepted_event_count)
                .unwrap();
            let expected = ledger.classify_basis_currency(basis, None);
            let actual = proof.currency();
            match expected {
                BasisCurrency::Current => assert_eq!(
                    actual,
                    Ok(crate::projections::AppliedBasisCurrency::Current)
                ),
                BasisCurrency::AppendOnlyStale(staleness) => assert_eq!(
                    actual,
                    Ok(crate::projections::AppliedBasisCurrency::AppendedTail { staleness })
                ),
                BasisCurrency::Revised(staleness) => {
                    assert_eq!(
                        actual,
                        Err(crate::projections::ProjectionApplyError::StaleBasis { staleness })
                    );
                    continue;
                }
            }
            let expected =
                crate::projections::resolve_claim_evidence_basis_events(basis, &ledger, None);
            assert_eq!(
                proof.evidence_for(&patch).get("reed-1").copied(),
                expected.iter().find(|e| e.span_id == "reed-1")
            );
        }
    }
    let valid = ledger.current_basis();
    let mut bad_prefix = valid.clone();
    bad_prefix
        .covered_prefix
        .as_mut()
        .unwrap()
        .content_hash
        .push('x');
    let mut bad_summary = valid.clone();
    bad_summary.summarized_through_revision = Some(100);
    let mut reordered = valid;
    reordered.span_revisions.reverse();
    for bad in [bad_prefix, bad_summary, reordered] {
        patch.basis = bad;
        let BasisCurrency::Revised(staleness) = ledger.classify_basis_currency(&patch.basis, None)
        else {
            panic!("malformed fixture");
        };
        assert_eq!(
            f.projection
                .live_basis(&patch, ledger.accepted_event_count)
                .unwrap()
                .currency(),
            Err(crate::projections::ProjectionApplyError::StaleBasis { staleness })
        );
    }
}

#[test]
fn replaced_or_rewritten_transcript_never_reuses_the_old_projection_proof() {
    let f = Fixture::new();
    let mut old = f.seeded_patch();
    f.projection.commit_after(&old, Some(1)).unwrap();
    old.sequence = 2;
    let fork = Fixture::new();
    let changed = transcript_event("branch", "sand");
    fork.transcript.commit_after(&changed, None).unwrap();
    // An external equal-height in-place rewrite, then a legitimate append.
    fs::copy(&fork.transcript.path, &f.transcript.path).unwrap();
    f.transcript
        .commit_after(&transcript_event("stem", "reed"), None)
        .unwrap();
    assert_eq!(
        f.projection.commit_after(&old, Some(1)),
        Err(TranscriptCommitError::InvalidRevision)
    );
    // A replacement file with a shorter verified history invalidates the cursor too.
    let moved = f.root.join("retired");
    fs::rename(&f.transcript.path, &moved).unwrap();
    fs::copy(&fork.transcript.path, &f.transcript.path).unwrap();
    assert_eq!(
        f.projection.commit_after(&old, Some(1)),
        Err(TranscriptCommitError::InvalidRevision)
    );
    old.basis = ProjectionBasis::from_transcript_events(&[changed]);
    assert_eq!(
        f.projection
            .commit_after(&old, Some(1))
            .unwrap()
            .head
            .sequence,
        2
    );
}

#[test]
fn projection_append_rechecks_the_candidate_head_but_accepts_later_appends() {
    use crate::projections::{AppliedBasisCurrency, TranscriptEventStability};
    let f = Fixture::new();
    let patch = f.seeded_patch();
    let mut tail = transcript_event("twig", "reed");
    tail.start_time = 10.0;
    tail.end_time = 11.0;
    let mut partial = tail.clone();
    partial.is_final = false;
    partial.end_of_turn = false;
    partial.stability = TranscriptEventStability::Partial;
    f.transcript.commit_after(&partial, None).unwrap();
    let old = f.projection.live_basis(&patch, 2).unwrap();
    assert_eq!(old.currency(), Ok(AppliedBasisCurrency::Current));

    let fork = Fixture::new();
    fork.seeded_patch();
    fork.transcript.commit_after(&tail, None).unwrap();
    fs::copy(&fork.transcript.path, &f.transcript.path).unwrap();
    let current = f.projection.live_basis(&patch, 2).unwrap();
    assert!(matches!(
        current.currency(),
        Ok(AppliedBasisCurrency::AppendedTail { .. })
    ));
    assert_eq!(
        f.projection
            .commit_at_lane(&patch, Some(2), &[], Some(0), Some(old.heads())),
        Err(TranscriptCommitError::InvalidRevision),
    );
    assert!(!f.projection.path.exists());
    f.transcript
        .commit_after(&transcript_event("moss", "green"), None)
        .unwrap();
    let result = f
        .projection
        .commit_at_lane(&patch, Some(2), &[], Some(0), Some(current.heads()))
        .unwrap();
    assert_eq!(result.head.sequence, 1);
}

#[test]
fn an_invalid_semantic_tail_cannot_pollute_a_later_shorter_dependency_fold() {
    let f = Fixture::new();
    let patch = f.seeded_patch();
    f.projection.live_basis(&patch, 1).unwrap();
    // Valid physical legacy rows can carry a stale semantic revision.
    let first = transcript_event("branch", "wood");
    let mut second = transcript_event("leaf", "green");
    second.revision_number = 2;
    let stale = transcript_event("leaf", "fallen");
    let mut bytes = Vec::new();
    for event in [&first, &second, &stale] {
        serde_json::to_writer(&mut bytes, event).unwrap();
        bytes.push(b'\n');
    }
    fs::write(&f.transcript.path, bytes).unwrap();
    assert!(matches!(
        f.projection.live_basis(&patch, 3),
        Err(TranscriptCommitError::InvalidHistory)
    ));
    let mut valid = patch;
    valid.basis = ProjectionBasis::from_transcript_events(&[first, second]);
    assert_eq!(
        f.projection.live_basis(&valid, 2).unwrap().currency(),
        Ok(crate::projections::AppliedBasisCurrency::Current)
    );
}

#[test]
fn a_dependency_lock_still_held_after_the_wait_reports_contention() {
    use crate::persistence::canonical_durability::{
        transcript_coordination_path, wait_for_transcript_lock,
    };
    let f = Fixture::new();
    let patch = f.seeded_patch();
    let held = wait_for_transcript_lock(
        &transcript_coordination_path(&f.transcript.path),
        false,
        true,
    )
    .unwrap();
    assert!(matches!(
        f.projection.live_basis(&patch, 1),
        Err(TranscriptCommitError::ConflictingAttempt)
    ));
    drop(held);
    assert!(f.projection.live_basis(&patch, 1).is_ok());
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
fn a_projection_receipt_survives_a_failed_witness_write() {
    let fixture = Fixture::new();
    let first = fixture.seeded_patch();
    CUT.set(Some(Cut::WitnessWrite));
    let receipt = fixture.projection.commit_after(&first, Some(1)).unwrap();
    assert!(!receipt.already_accepted);
    assert!(!fixture.projection.has_pending_attempt());
    let verified =
        read_committed_projection::<ProjectionPatch>(&fixture.projection.path, SESSION).unwrap();
    assert_eq!(verified.records[0].record_hash, receipt.head.record_hash);
    assert!(tier::receipt_evidence(&fixture.projection.path, &verified.records).is_empty());
    let repeated = fixture.projection.commit_after(&first, Some(1)).unwrap();
    assert!(repeated.already_accepted);
    assert_eq!(receipt.head, repeated.head);
    assert!(!tier::receipt_evidence(&fixture.projection.path, &verified.records).is_empty());
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
        recover_projection_directory(&f.root, "fresh-session").unwrap();
        assert!(descriptor.exists());
        assert_eq!(fs::read(&f.projection.path).unwrap(), interrupted);
        let mut transcript_receipts = Vec::new();
        for _ in 0..2 {
            recover_in_directory(
                &f.root.join("transcripts"),
                "fresh-session",
                |_, _, receipt| transcript_receipts.push(receipt.sequence),
            )
            .unwrap();
            recover_projection_directory(&f.root, "fresh-session").unwrap();
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
        f.projection
            .commit_at_lane(&row, Some(1), &[], Some(0), None),
        Err(TranscriptCommitError::LiveHeadChanged)
    );
    assert!(
        f.projection
            .commit_at_lane(&row, Some(1), &[], Some(1), None)
            .unwrap()
            .already_accepted
    );
    let prior = row.clone();
    row.sequence = 5;
    f.projection.commit_after(&row, Some(1)).unwrap();
    assert_eq!(
        f.projection
            .commit_at_lane(&prior, Some(1), &[], Some(1), None),
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
