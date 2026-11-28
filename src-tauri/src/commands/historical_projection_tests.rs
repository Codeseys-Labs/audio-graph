// Shared by the unchanged-wire and lost-basis cases. Physical positions
// differ from lane sequence numbers once graph and notes are interleaved.
fn seed_historical_projection(session: &str) -> Vec<u8> {
    use crate::persistence::canonical_log::{
        CanonicalAppendOutcome, CanonicalAppender, CanonicalBasisHead, CanonicalEventMetadata,
        CanonicalTailRecovery,
    };
    use crate::projections::ProjectionBasis;
    let repo = FileMemoryRepository::user_data();
    let transcript = crate::user_data::transcript_events_path(session).unwrap();
    let mut writer = CanonicalAppender::open(
        &transcript,
        session,
        "transcript_revisions",
        1,
        CanonicalTailRecovery::Strict,
    )
    .unwrap();
    let mut first = projection_status_test_event("alder");
    first.text = "cattail".into();
    let mut second = projection_status_test_event("birch");
    second.start_time += 2.0;
    second.end_time += 2.0;
    second.received_at_ms += 50;
    assert!(matches!(
        writer.append(&CanonicalEventMetadata::new("one"), &first),
        CanonicalAppendOutcome::Accepted(_)
    ));
    assert!(matches!(
        writer.append(&CanonicalEventMetadata::new("two"), &second),
        CanonicalAppendOutcome::Accepted(_)
    ));
    drop(writer);
    let bytes = std::fs::read(&transcript).unwrap();
    let prefix = bytes
        .split_inclusive(|b| *b == b'\n')
        .next()
        .unwrap()
        .to_vec();
    let stream = repo.load_transcript_event_stream(session).unwrap();
    let first_basis = ProjectionBasis::from_transcript_events(std::slice::from_ref(&first));
    let both = ProjectionBasis::from_transcript_events(&[first, second]);
    let projection = crate::user_data::projection_events_path(session).unwrap();
    let mut writer = CanonicalAppender::open(
        &projection,
        session,
        "projection_patches",
        1,
        CanonicalTailRecovery::Strict,
    )
    .unwrap();
    for (source, key, patch) in [
        (0, "reed", report_note_patch(1, first_basis.clone(), "fern")),
        (1, "oak", report_graph_patch(1, both)),
        (0, "elm", report_note_patch(2, first_basis, "moss")),
    ] {
        let row = &stream.records()[source];
        let mut meta = CanonicalEventMetadata::new(key);
        meta.basis_heads.insert(
            "transcript_revisions".into(),
            CanonicalBasisHead {
                sequence: row.sequence,
                event_id: row.event_id.clone(),
                record_hash: row.record_hash.clone(),
            },
        );
        assert!(matches!(
            writer.append(&meta, &patch),
            CanonicalAppendOutcome::Accepted(_)
        ));
    }
    drop(writer);
    let view = repo.replay_projection_state(session).unwrap().state;
    repo.save_materialized_notes(session, &view.notes).unwrap();
    repo.save_materialized_graph(session, &view.graph).unwrap();
    prefix
}

#[test]
fn historical_projection_intact_wire_matches_the_predecessor() {
    use sha2::{Digest, Sha256};
    let _lock = crate::sessions::TEST_HOME_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = unique_tempdir("reeds");
    let _home = HomeGuard::set(&dir);
    let session = "orchard";
    seed_historical_projection(session);
    let results = [
        serde_json::to_vec(&load_session_notes_artifacts_impl(session.into()).unwrap()).unwrap(),
        serde_json::to_vec(&load_session_graph_artifact_impl(session.into()).unwrap()).unwrap(),
        serde_json::to_vec(&session_export_bundle(session).unwrap()).unwrap(),
    ];
    assert_eq!(
        results.map(|bytes| hex::encode(Sha256::digest(bytes))),
        // Serialized on the predecessor (0f1b703e) with this fixture.
        [
            "74558b4ee71a28c9b25b8369f1f41a1853e0de93bcaee4c0290a9e1f477be664",
            "7a2c3c59cf667facefbbe81d4e08525d7a2766f69e57b5bb3544f84d6d1ac5d2",
            "4a56cd63c92b89367f8a6dafa51e54cfba265a78b29c12f5ae60686886fffec5"
        ]
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn historical_projection_load_export_and_report_withhold_only_the_unverified_suffix() {
    use crate::persistence::canonical_log::{
        CanonicalAppendOutcome, CanonicalAppender, CanonicalEventMetadata, CanonicalTailRecovery,
    };
    let _lock = crate::sessions::TEST_HOME_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    for loss in [
        "truncated",
        "rolled_back",
        "missing",
        "legacy",
        "torn",
        "foreign",
        "tampered",
    ] {
        let dir = unique_tempdir("reeds");
        let _home = HomeGuard::set(&dir);
        let session = "orchard";
        let prefix = seed_historical_projection(session);
        let source = crate::user_data::resolve_transcript_events_path(session).unwrap();
        match loss {
            "missing" | "legacy" => {
                std::fs::remove_file(&source).unwrap();
                if loss == "legacy" {
                    seed_legacy_transcript(session, "willow");
                }
            }
            "foreign" => {
                seed_historical_projection("meadow");
                std::fs::copy(
                    crate::user_data::resolve_transcript_events_path("meadow").unwrap(),
                    &source,
                )
                .unwrap();
            }
            "tampered" => {
                let bytes = std::fs::read_to_string(&source).unwrap();
                assert!(bytes.contains("cattail"));
                std::fs::write(&source, bytes.replace("cattail", "bulrush")).unwrap();
            }
            "torn" => {
                let mut broken = prefix.clone();
                broken.extend_from_slice(b"{");
                std::fs::write(&source, broken).unwrap();
            }
            _ => {
                std::fs::write(&source, prefix).unwrap();
                if loss == "rolled_back" {
                    let mut writer = CanonicalAppender::open(
                        &source,
                        session,
                        "transcript_revisions",
                        1,
                        CanonicalTailRecovery::Strict,
                    )
                    .unwrap();
                    let mut revised = projection_status_test_event("alder");
                    revised.text = "willow".into();
                    revised.revision_number = 2;
                    revised.received_at_ms += 50;
                    assert!(matches!(
                        writer.append(&CanonicalEventMetadata::new("fork"), &revised),
                        CanonicalAppendOutcome::Accepted(_)
                    ));
                }
            }
        }
        let before = snapshot_tree(&dir);
        let capture = crate::test_log_capture::ThreadLogCapture::start();
        let transcript =
            read_session_transcript_snapshot(&FileMemoryRepository::user_data(), session);
        let notes = load_session_notes_artifacts_impl(session.into());
        let graph = load_session_graph_artifact_impl(session.into());
        let export = session_export_bundle(session);
        let report = projection_replay_report_for_session(session);
        assert_eq!(snapshot_tree(&dir), before);
        let logs = crate::test_log_capture::render(&capture.finish());
        if matches!(loss, "torn" | "foreign" | "tampered") {
            let errors = [
                transcript.err().unwrap(),
                notes.unwrap_err().to_string(),
                graph.unwrap_err().to_string(),
                export.unwrap_err().to_string(),
                report.unwrap_err().to_string(),
            ];
            let reason = match loss {
                "foreign" => "SessionMismatch",
                "tampered" => "PayloadHashMismatch",
                _ => "corrupt",
            };
            for error in errors {
                assert!(error.contains(reason), "{error}");
            }
            assert!(!logs.contains("basis_unavailable"));
        } else {
            let notes = notes.unwrap();
            let graph = graph.unwrap().unwrap();
            let export = export.unwrap();
            let report = report.unwrap();
            let kept = usize::from(!matches!(loss, "legacy" | "missing"));
            for mark in [
                notes
                    .notes
                    .as_ref()
                    .unwrap()
                    .basis_unavailable
                    .as_ref()
                    .unwrap(),
                graph.basis_unavailable.as_ref().unwrap(),
                export
                    .notes
                    .as_ref()
                    .unwrap()
                    .basis_unavailable
                    .as_ref()
                    .unwrap(),
                export
                    .materialized_graph
                    .as_ref()
                    .unwrap()
                    .basis_unavailable
                    .as_ref()
                    .unwrap(),
                report.basis_unavailable.as_ref().unwrap(),
            ] {
                assert_eq!(
                    (mark.first_sequence, mark.last_sequence),
                    (kept as u64 + 1, 3)
                );
                assert_eq!(
                    mark.verified_sequences,
                    if kept != 0 { vec![3] } else { vec![] }
                );
            }
            for mark in [
                &export.projection_loss.as_ref().unwrap().basis_unavailable,
                notes.basis_unavailable.as_ref().unwrap(),
            ] {
                assert_eq!(
                    (mark.first_sequence, mark.last_sequence),
                    (kept as u64 + 1, 3)
                );
                assert_eq!(mark.verified_frames.len(), kept);
                if kept != 0 {
                    let head = &mark.verified_frames[0].basis_heads["transcript_revisions"];
                    assert_eq!(head.sequence, 1);
                    assert_eq!(head.event_id, "one");
                    assert_eq!(mark.verified_frames[0].sequence, 3);
                    assert_eq!(mark.verified_frames[0].patch.sequence, 2);
                }
            }
            let expected_transcript = kept + usize::from(loss == "rolled_back");
            let transcript = transcript.unwrap();
            assert_eq!(transcript.events.len(), expected_transcript);
            if loss == "legacy" {
                assert_eq!(transcript.transcript[0].text, "willow");
                assert_eq!(export.transcript[0].text, "willow");
            }
            assert_eq!(export.transcript_events.len(), expected_transcript);
            assert_eq!(
                export.schema_version,
                PROJECTION_BASIS_LOSS_EXPORT_SCHEMA_VERSION
            );
            assert_eq!(report.projection_event_count, 3);
            assert_eq!(report.projection_checked_patch_count, 3);
            assert_eq!(report.projection_invalid_basis_count, 3 - kept * 2);
            assert_eq!(report.transcript_event_count, expected_transcript);
            assert_eq!(report.replayed.notes_last_sequence, kept as u64);
            assert_eq!(notes.projection_events.len(), kept);
            assert_eq!(export.projection_events.len(), kept);
            assert_eq!(export.notes.as_ref().unwrap().last_sequence, kept as u64);
            if kept != 0 {
                assert_eq!(notes.notes.unwrap().notes[0].body, "fern");
            }
            assert!(
                graph.nodes.is_empty()
                    && export.materialized_graph.as_ref().unwrap().nodes.is_empty()
            );
            let wire = serde_json::to_value(&export).unwrap();
            for view in ["notes", "materialized_graph"] {
                assert_eq!(
                    wire[view]["basis_unavailable"]["first_sequence"],
                    kept as u64 + 1
                );
                assert_eq!(wire[view]["basis_unavailable"]["last_sequence"], 3);
                assert!(wire[view].get("projection_notice").is_none());
            }
            assert!(
                wire["projection_notice"]["en"].is_string()
                    && wire["projection_notice"]["pt"].is_string()
            );
            assert_eq!(logs.matches("basis_unavailable").count(), 4);
            for canary in ["reeds", "orchard", "cattail", "fern", "moss"] {
                assert!(!logs.contains(canary));
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

fn physical_basis<T>(
    row: &crate::persistence::canonical_log::CanonicalRecord<T>,
) -> crate::persistence::canonical_log::CanonicalBasisHead {
    crate::persistence::canonical_log::CanonicalBasisHead {
        sequence: row.sequence,
        event_id: row.event_id.clone(),
        record_hash: row.record_hash.clone(),
    }
}

#[test]
fn historical_projection_conflicting_source_generations_only_settle_gaps_inside_the_verified_prefix()
 {
    use crate::persistence::canonical_log::{
        CanonicalAppendOutcome, CanonicalAppender, CanonicalEventMetadata, CanonicalTailRecovery,
    };
    use crate::projections::{ProjectionBasis, ProjectionRecoveryStatus};

    let _lock = crate::sessions::TEST_HOME_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = unique_tempdir("rush");
    let _home = HomeGuard::set(&dir);
    let session = "orchard";
    let repo = FileMemoryRepository::user_data();
    let prefix = seed_historical_projection(session);
    let source = crate::user_data::resolve_transcript_events_path(session).unwrap();
    let original = repo.load_transcript_event_stream(session).unwrap();
    let path = crate::user_data::resolve_projection_events_path(session).unwrap();
    let frames = repo.load_projection_patch_stream(session).unwrap();
    // A notes gap follows frame one. A graph gap follows frame two, which
    // will itself be withheld. Only the former belongs to the returned view.
    let mut cause = String::new();
    for (i, lane) in [(0, "notes"), (1, "graph")] {
        let row = &frames.records()[i];
        let id = uuid::Uuid::new_v4();
        let gap = serde_json::json!({"stream":"projection_patches", "after":{"sequence":row.sequence, "record_hash":row.record_hash}, "suffix_quarantined":true, "projection_lanes":[lane]});
        std::fs::write(
            path.with_added_extension(format!("{id}.gap")),
            serde_json::to_vec(&gap).unwrap(),
        )
        .unwrap();
        std::fs::write(
            path.with_added_extension(format!("{id}.quarantine")),
            b"fern",
        )
        .unwrap();
        if i == 0 {
            cause = format!("projection-gap-{id}");
        }
    }
    std::fs::write(&source, prefix).unwrap();
    let fork = projection_status_test_event("spruce");
    let mut writer = CanonicalAppender::open(
        &source,
        session,
        "transcript_revisions",
        1,
        CanonicalTailRecovery::Strict,
    )
    .unwrap();
    assert!(matches!(
        writer.append(&CanonicalEventMetadata::new("fork"), &fork),
        CanonicalAppendOutcome::Accepted(_)
    ));
    drop(writer);
    let current = repo.load_transcript_event_stream(session).unwrap();
    let old = physical_basis(&original.records()[1]);
    let new = physical_basis(&current.records()[1]);
    assert_eq!(old.sequence, new.sequence);
    assert_ne!(old.record_hash, new.record_hash);
    assert_eq!(frames.records()[1].basis_heads["transcript_revisions"], old);
    let basis = ProjectionBasis::from_transcript_events(&current.into_payloads());
    let mut repair = CanonicalEventMetadata::new("maple");
    repair
        .basis_heads
        .insert("transcript_revisions".into(), new);
    repair.causal_event_ids.push(cause);
    let mut writer = CanonicalAppender::open(
        &path,
        session,
        "projection_patches",
        1,
        CanonicalTailRecovery::Strict,
    )
    .unwrap();
    assert!(matches!(
        writer.append(&repair, &report_note_patch(3, basis, "pine")),
        CanonicalAppendOutcome::Accepted(_)
    ));
    drop(writer);

    let before = snapshot_tree(&dir);
    let notes = load_session_notes_artifacts_impl(session.into())
        .unwrap()
        .notes
        .unwrap();
    let graph = load_session_graph_artifact_impl(session.into())
        .unwrap()
        .unwrap();
    let export = session_export_bundle(session).unwrap();
    let report = projection_replay_report_for_session(session).unwrap();
    let mark = notes.basis_unavailable.as_ref().unwrap();
    assert_eq!((mark.first_sequence, mark.last_sequence), (2, 4));
    assert_eq!(mark.verified_sequences, [3, 4]);
    assert_eq!(report.basis_unavailable.as_ref(), Some(mark));
    assert_eq!(notes.last_sequence, 1);
    assert_eq!(
        notes.projection_recovery,
        Some(ProjectionRecoveryStatus::RecomputeRequired)
    );
    assert_eq!(
        export.notes.as_ref().unwrap().projection_recovery,
        notes.projection_recovery
    );
    assert_eq!(graph.projection_recovery, None);
    assert_eq!(snapshot_tree(&dir), before);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn historical_projection_losing_speaker_history_preserves_all_revisions_in_a_retained_batch() {
    use crate::identity::{ClusterRef, ManualEdit, manual_batch, tests::span};
    use crate::persistence::canonical_log::{
        CanonicalAppendOutcome, CanonicalAppender, CanonicalEventMetadata, CanonicalTailRecovery,
    };
    use crate::projections::{ProjectionBasis, SpeakerTimeline};
    let _lock = crate::sessions::TEST_HOME_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let dir = unique_tempdir("marsh");
    let _home = HomeGuard::set(&dir);
    let session = "grove";
    let repo = FileMemoryRepository::user_data();
    let event = projection_status_test_event("elm");
    repo.append_transcript_event(session, &event).unwrap();
    let transcript = repo.load_transcript_event_stream(session).unwrap();
    let a = span("alder", "mic");
    let b = span("beech", "mic");
    repo.append_diarization_span_revision(session, &a).unwrap();
    repo.append_diarization_span_revision(session, &b).unwrap();
    let origin = ClusterRef::origin(&a).unwrap();
    let mut timeline = SpeakerTimeline::replay(session, [a, b]).unwrap();
    let batch = manual_batch(
        &timeline,
        &origin,
        ManualEdit::Name("Ash".into()),
        10,
        || unreachable!(),
    )
    .unwrap();
    assert_eq!(batch.revisions.len(), 2);
    repo.append_speaker_relabel_batch(session, &batch).unwrap();
    let source = crate::user_data::resolve_diarization_events_path(session).unwrap();
    let prefix = std::fs::read(&source).unwrap();
    timeline.apply_batch(&batch).unwrap();
    let fork = manual_batch(
        &timeline,
        &origin,
        ManualEdit::Name("Birch".into()),
        20,
        || unreachable!(),
    )
    .unwrap();
    let basis = |timeline: &SpeakerTimeline| {
        ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&event),
            &timeline.current_basis_spans(),
        )
    };
    let notes_patch = report_note_patch(1, basis(&timeline), "fern");
    // The new speaker belongs to the later graph patch's source generation.
    let mut cedar = span("cedar", "sys");
    cedar.received_at_ms = notes_patch.created_at_ms + 1_000;
    repo.append_diarization_span_revision(session, &cedar)
        .unwrap();
    timeline.apply_event(cedar).unwrap();
    let graph_patch = report_graph_patch(1, basis(&timeline));
    let speakers = repo.load_speaker_revision_stream(session).unwrap();
    let path = crate::user_data::projection_events_path(session).unwrap();
    let mut writer = CanonicalAppender::open(
        &path,
        session,
        "projection_patches",
        1,
        CanonicalTailRecovery::Strict,
    )
    .unwrap();
    for (index, key, patch) in [(2, "ash", notes_patch), (3, "elm", graph_patch)] {
        let mut meta = CanonicalEventMetadata::new(key);
        meta.basis_heads.insert(
            "transcript_revisions".into(),
            physical_basis(&transcript.records()[0]),
        );
        meta.basis_heads.insert(
            "speaker_revisions".into(),
            physical_basis(&speakers.records()[index]),
        );
        assert!(matches!(
            writer.append(&meta, &patch),
            CanonicalAppendOutcome::Accepted(_)
        ));
    }
    drop(writer);
    let intact = projection_replay_report_for_session(session).unwrap();
    assert!(intact.basis_unavailable.is_none());
    assert_eq!(intact.projection_invalid_basis_count, 0);
    assert_eq!(intact.replayed.graph_last_sequence, 1);
    for (loss, returned_rows) in [("shorter", 4), ("fork", 6), ("missing", 0)] {
        if loss == "missing" {
            std::fs::remove_file(&source).unwrap();
        } else {
            std::fs::write(&source, &prefix).unwrap();
            if loss == "fork" {
                repo.append_speaker_relabel_batch(session, &fork).unwrap();
            }
        }
        let before = snapshot_tree(&dir);
        let notes = load_session_notes_artifacts_impl(session.into())
            .unwrap()
            .notes
            .unwrap();
        let export = session_export_bundle(session).unwrap();
        let report = projection_replay_report_for_session(session).unwrap();
        let expected = u64::from(loss != "missing");
        assert_eq!(report.replayed.notes_last_sequence, expected);
        assert_eq!(notes.last_sequence, expected);
        assert_eq!(
            notes.basis_unavailable.as_ref().unwrap().first_sequence,
            expected + 1
        );
        assert_eq!(export.diarization_events.len(), returned_rows);
        assert!(export.materialized_graph.as_ref().unwrap().nodes.is_empty());
        assert_eq!(snapshot_tree(&dir), before);
    }
    std::fs::remove_dir_all(dir).unwrap();
}
