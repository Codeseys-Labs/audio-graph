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
    for loss in ["truncated", "rolled_back", "missing", "torn"] {
        let dir = unique_tempdir("reeds");
        let _home = HomeGuard::set(&dir);
        let session = "orchard";
        let prefix = seed_historical_projection(session);
        let source = crate::user_data::resolve_transcript_events_path(session).unwrap();
        match loss {
            "missing" => std::fs::remove_file(&source).unwrap(),
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
        let capture_files = || {
            [
                source.clone(),
                crate::user_data::resolve_projection_events_path(session).unwrap(),
                crate::user_data::resolve_notes_path(session).unwrap(),
                crate::user_data::resolve_materialized_graph_path(session).unwrap(),
            ]
            .map(|p| match std::fs::read(p) {
                Ok(bytes) => Some(bytes),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => panic!("fixture file read: {:?}", e.kind()),
            })
        };
        let before = capture_files();
        let capture = crate::test_log_capture::ThreadLogCapture::start();
        let transcript =
            read_session_transcript_snapshot(&FileMemoryRepository::user_data(), session);
        let notes = load_session_notes_artifacts_impl(session.into());
        let graph = load_session_graph_artifact_impl(session.into());
        let export = session_export_bundle(session);
        let report = projection_replay_report_for_session(session);
        assert_eq!(capture_files(), before);
        let logs = crate::test_log_capture::render(&capture.finish());
        if loss == "torn" {
            assert!(
                transcript.is_err()
                    && notes.is_err()
                    && graph.is_err()
                    && export.is_err()
                    && report.is_err()
            );
        } else {
            let notes = notes.unwrap();
            let graph = graph.unwrap().unwrap();
            let export = export.unwrap();
            let report = report.unwrap();
            let kept = usize::from(loss != "missing");
            for mark in [
                notes
                    .notes
                    .as_ref()
                    .unwrap()
                    .basis_unavailable
                    .as_ref()
                    .unwrap(),
                graph.basis_unavailable.as_ref().unwrap(),
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
            assert_eq!(transcript.unwrap().events.len(), expected_transcript);
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
