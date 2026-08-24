//! Strict, presence-bearing readers for AudioGraph's canonical event streams.
//!
//! These readers never repair, quarantine, rewrite, or preflight with
//! `Path::exists`. A single strict open decides whether a stream is missing or
//! present, preserving an existing empty stream as canonical authority.

use std::fmt;
use std::io;
use std::path::Path;

use serde::de::DeserializeOwned;

use super::canonical_log::{
    CanonicalIoOperation, CanonicalLogError, CanonicalLogSnapshot, CanonicalTailRecovery,
    load_canonical_stream,
};
use super::{DATA_MOVEMENT_SCHEMA_VERSION, DataMovementEvent};
use crate::projections::{DiarizationSpanRevision, ProjectionPatch, TranscriptEvent};
use crate::speech_span_revision::CompatibleSpeechSpanRevision;

const TRANSCRIPT_REVISIONS_STREAM_ID: &str = "transcript_revisions";
const SPEAKER_REVISIONS_STREAM_ID: &str = "speaker_revisions";
const PROJECTION_PATCHES_STREAM_ID: &str = "projection_patches";
const DATA_MOVEMENT_EVENTS_STREAM_ID: &str = "data_movement_events";

const TRANSCRIPT_REVISIONS_SCHEMA_VERSION: u32 = 1;
const SPEAKER_REVISIONS_SCHEMA_VERSION: u32 = 1;
const PROJECTION_PATCHES_SCHEMA_VERSION: u32 = 1;
const DATA_MOVEMENT_EVENTS_SCHEMA_VERSION: u32 = 1;
const DATA_MOVEMENT_EVENTS_EMBEDDED_SCHEMA_VERSION: u32 = 1;

// ADR-0043 pins the IPC payload version supported by outer stream schema v1.
// If the IPC crate advances, this must fail until an explicit multi-version
// mapping or migration is added instead of silently reinterpreting old rows.
const _: () = assert!(
    DATA_MOVEMENT_SCHEMA_VERSION == DATA_MOVEMENT_EVENTS_EMBEDDED_SCHEMA_VERSION,
    "data-movement IPC schema drift requires an explicit canonical mapping"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CanonicalStreamDescriptor {
    stream_id: &'static str,
    domain_schema_version: u32,
}

const TRANSCRIPT_REVISIONS: CanonicalStreamDescriptor = CanonicalStreamDescriptor {
    stream_id: TRANSCRIPT_REVISIONS_STREAM_ID,
    domain_schema_version: TRANSCRIPT_REVISIONS_SCHEMA_VERSION,
};
const SPEAKER_REVISIONS: CanonicalStreamDescriptor = CanonicalStreamDescriptor {
    stream_id: SPEAKER_REVISIONS_STREAM_ID,
    domain_schema_version: SPEAKER_REVISIONS_SCHEMA_VERSION,
};
const PROJECTION_PATCHES: CanonicalStreamDescriptor = CanonicalStreamDescriptor {
    stream_id: PROJECTION_PATCHES_STREAM_ID,
    domain_schema_version: PROJECTION_PATCHES_SCHEMA_VERSION,
};
const DATA_MOVEMENT_EVENTS: CanonicalStreamDescriptor = CanonicalStreamDescriptor {
    stream_id: DATA_MOVEMENT_EVENTS_STREAM_ID,
    domain_schema_version: DATA_MOVEMENT_EVENTS_SCHEMA_VERSION,
};

#[derive(Clone, PartialEq)]
pub(crate) enum StrictCanonicalRead<T> {
    Missing,
    Present(CanonicalLogSnapshot<T>),
}

impl<T> StrictCanonicalRead<T> {
    /// Compatibility projection for callers whose contract still models a
    /// missing stream and a present-empty stream as the same empty row list.
    pub(crate) fn into_payloads(self) -> Vec<T> {
        match self {
            Self::Missing => Vec::new(),
            Self::Present(snapshot) => snapshot
                .records
                .into_iter()
                .map(|record| record.payload)
                .collect(),
        }
    }
}

/// Content-redacted strict reader failures. No path, session id, event id, or
/// payload value is included in `Debug` or `Display`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum CanonicalReaderError {
    Canonical(CanonicalLogError),
    DataMovementSessionMismatch { record_index: usize },
    DataMovementSchemaVersionMismatch { record_index: usize },
}

impl fmt::Display for CanonicalReaderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Canonical(error) => error.fmt(formatter),
            Self::DataMovementSessionMismatch { record_index } => write!(
                formatter,
                "canonical data-movement record {record_index} has the wrong session"
            ),
            Self::DataMovementSchemaVersionMismatch { record_index } => write!(
                formatter,
                "canonical data-movement record {record_index} has an unsupported embedded schema"
            ),
        }
    }
}

impl std::error::Error for CanonicalReaderError {}

impl From<CanonicalLogError> for CanonicalReaderError {
    fn from(error: CanonicalLogError) -> Self {
        Self::Canonical(error)
    }
}

fn load_strict_canonical_stream<T: DeserializeOwned>(
    path: &Path,
    session_id: &str,
    descriptor: CanonicalStreamDescriptor,
) -> Result<StrictCanonicalRead<T>, CanonicalReaderError> {
    match load_canonical_stream(
        path,
        session_id,
        descriptor.stream_id,
        descriptor.domain_schema_version,
        CanonicalTailRecovery::Strict,
    ) {
        Ok(snapshot) => Ok(StrictCanonicalRead::Present(snapshot)),
        Err(CanonicalLogError::Io {
            operation: CanonicalIoOperation::Read,
            kind: io::ErrorKind::NotFound,
        }) => Ok(StrictCanonicalRead::Missing),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn load_transcript_revisions(
    path: &Path,
    session_id: &str,
) -> Result<StrictCanonicalRead<TranscriptEvent>, CanonicalReaderError> {
    load_strict_canonical_stream(path, session_id, TRANSCRIPT_REVISIONS)
}

/// Reader-first migration seam for the versioned Speech Span Revision payload.
/// The production transcript writer remains on the isolated v1
/// [`TranscriptEvent`] path until adapter activation lands in audio-graph-48de.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) fn load_speech_span_revisions(
    path: &Path,
    session_id: &str,
) -> Result<StrictCanonicalRead<CompatibleSpeechSpanRevision>, CanonicalReaderError> {
    load_strict_canonical_stream(path, session_id, TRANSCRIPT_REVISIONS)
}

pub(super) fn load_speaker_revisions(
    path: &Path,
    session_id: &str,
) -> Result<StrictCanonicalRead<DiarizationSpanRevision>, CanonicalReaderError> {
    load_strict_canonical_stream(path, session_id, SPEAKER_REVISIONS)
}

pub(super) fn load_projection_patches(
    path: &Path,
    session_id: &str,
) -> Result<StrictCanonicalRead<ProjectionPatch>, CanonicalReaderError> {
    load_strict_canonical_stream(path, session_id, PROJECTION_PATCHES)
}

pub(super) fn load_data_movement_events(
    path: &Path,
    session_id: &str,
) -> Result<StrictCanonicalRead<DataMovementEvent>, CanonicalReaderError> {
    let read =
        load_strict_canonical_stream::<DataMovementEvent>(path, session_id, DATA_MOVEMENT_EVENTS)?;
    if let StrictCanonicalRead::Present(snapshot) = &read {
        for (record_index, record) in snapshot.records.iter().enumerate() {
            if record.payload.session_id != session_id {
                return Err(CanonicalReaderError::DataMovementSessionMismatch { record_index });
            }
            if record.payload.schema_version != DATA_MOVEMENT_EVENTS_EMBEDDED_SCHEMA_VERSION {
                return Err(CanonicalReaderError::DataMovementSchemaVersionMismatch {
                    record_index,
                });
            }
        }
    }
    Ok(read)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde::Serialize;
    use serde::de::DeserializeOwned;

    use super::super::canonical_log::{
        CanonicalAppendOutcome, CanonicalAppender, CanonicalEventMetadata, CanonicalRecordEncoding,
        CanonicalTailRecovery,
    };
    use super::super::{
        DataMovementActor, DataMovementDestination, DataMovementEventType,
        DataMovementLedgerBuilder, FileMemoryRepository, LocalMemoryRepository, MovementPolicy,
        PrivacyMode, RetentionClass,
    };
    use super::*;
    use crate::projections::{
        DiarizationEventStability, MaterializedProjectionState, ProjectionBasis,
        ProjectionBasisSpan, ProjectionKind, ProjectionOperation, ProjectionProvenance,
        TranscriptEventStability, TranscriptHashVersion, TranscriptLedger,
        transcript_events_hash_v1,
    };

    fn unique_tempdir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!(
            "audio-graph-strict-reader-{label}-{}-{nanos}-{n}",
            std::process::id()
        ))
    }

    fn transcript_event(text: &str, revision_number: u64) -> TranscriptEvent {
        TranscriptEvent {
            span_id: "span-1".into(),
            provider: "fixture-asr".into(),
            source_id: "source-1".into(),
            provider_item_id: Some(format!("provider-{revision_number}")),
            transcript_segment_id: Some("segment-1".into()),
            speaker_id: Some("speaker-1".into()),
            speaker_label: Some("Speaker 1".into()),
            channel: None,
            text: text.into(),
            start_time: 0.0,
            end_time: 1.0,
            confidence: 0.95,
            is_final: true,
            stability: TranscriptEventStability::Final,
            revision_number,
            supersedes: None,
            turn_id: Some("turn-1".into()),
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: Some(10),
            asr_latency_ms: Some(20),
            received_at_ms: 1_700_000_000_000 + revision_number,
        }
    }

    fn speaker_revision(revision_number: u64) -> DiarizationSpanRevision {
        DiarizationSpanRevision {
            span_id: "speaker-span-1".into(),
            provider: "fixture-diarizer".into(),
            timeline_id: "source-1".into(),
            source_id: Some("source-1".into()),
            speaker_id: Some("speaker-1".into()),
            speaker_label: Some("Speaker 1".into()),
            provider_speaker_id: Some("0".into()),
            channel: None,
            start_time: 0.0,
            end_time: 1.0,
            confidence: Some(0.9),
            is_final: true,
            stability: DiarizationEventStability::Final,
            revision_number,
            supersedes: None,
            basis_asr_span_ids: vec!["span-1".into()],
            basis_transcript_segment_ids: vec!["segment-1".into()],
            raw_event_ref: None,
            capture_latency_ms: Some(10),
            asr_latency_ms: Some(20),
            received_at_ms: 1_700_000_000_000 + revision_number,
        }
    }

    fn projection_patch(sequence: u64) -> ProjectionPatch {
        ProjectionPatch {
            sequence,
            kind: ProjectionKind::Notes,
            llm_request_id: format!("request-{sequence}"),
            basis: ProjectionBasis {
                span_revisions: vec![ProjectionBasisSpan {
                    span_id: "span-1".into(),
                    revision_number: sequence,
                }],
                covered_prefix: None,
                diarization_span_revisions: Vec::new(),
                transcript_hash: format!("fnv1a64:{sequence:016x}"),
                summarized_through_revision: None,
            },
            operations: vec![ProjectionOperation::UpsertNote {
                id: "note-1".into(),
                title: "Fixture".into(),
                body: "Strict reader fixture".into(),
                tags: vec!["test".into()],
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
                heading_level: None,
            }],
            confidence: 0.9,
            provenance: ProjectionProvenance {
                provider: "fixture-llm".into(),
                model: "fixture-model".into(),
                prompt_id: "notes-v1".into(),
                // Pre-contract record shape (ADR-0038): absent route identity,
                // model id recorded as requested — the same values serde
                // defaults when reading a patch written before the route table.
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            route: None,
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            basis_currency_at_apply: None,
            created_at_ms: 1_700_000_000_000 + sequence,
        }
    }

    fn movement_event(session_id: &str, created_at_ms: u64) -> DataMovementEvent {
        DataMovementLedgerBuilder::new(
            session_id,
            DataMovementActor::System,
            DataMovementEventType::ProviderCallSucceeded,
            MovementPolicy {
                privacy_mode: PrivacyMode::ByokCloud,
                user_visible: true,
                retention_class: RetentionClass::Transient,
            },
            DataMovementDestination::provider("fixture-provider", "fixture-endpoint"),
        )
        .created_at_ms(created_at_ms)
        .build()
    }

    fn present_snapshot<T>(read: &StrictCanonicalRead<T>) -> &CanonicalLogSnapshot<T> {
        match read {
            StrictCanonicalRead::Missing => panic!("stream is missing"),
            StrictCanonicalRead::Present(snapshot) => snapshot,
        }
    }

    fn assert_legacy_then_mixed<T, F>(
        path: &Path,
        session_id: &str,
        descriptor: CanonicalStreamDescriptor,
        legacy: T,
        framed: T,
        load: F,
    ) where
        T: Clone + PartialEq + std::fmt::Debug + Serialize + DeserializeOwned,
        F: Fn(&Path, &str) -> Result<StrictCanonicalRead<T>, CanonicalReaderError>,
    {
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("create fixture parent");
        let mut legacy_bytes = serde_json::to_vec(&legacy).expect("serialize legacy payload");
        legacy_bytes.push(b'\n');
        fs::write(path, legacy_bytes).expect("write legacy fixture");

        let legacy_read = load(path, session_id).expect("load legacy-only stream");
        let legacy_snapshot = present_snapshot(&legacy_read);
        assert_eq!(legacy_snapshot.records.len(), 1);
        assert_eq!(
            legacy_snapshot.records[0].encoding,
            CanonicalRecordEncoding::LegacyJsonl
        );
        assert_eq!(legacy_snapshot.records[0].payload, legacy);

        let mut appender = CanonicalAppender::<T>::open(
            path,
            session_id,
            descriptor.stream_id,
            descriptor.domain_schema_version,
            CanonicalTailRecovery::Strict,
        )
        .expect("open fixture appender");
        assert!(matches!(
            appender.append(&CanonicalEventMetadata::new("framed-event"), &framed),
            CanonicalAppendOutcome::Accepted(_)
        ));
        drop(appender);

        let mixed_read = load(path, session_id).expect("load mixed stream");
        let mixed_snapshot = present_snapshot(&mixed_read);
        assert_eq!(mixed_snapshot.records.len(), 2);
        assert_eq!(
            mixed_snapshot.records[0].encoding,
            CanonicalRecordEncoding::LegacyJsonl
        );
        assert_eq!(
            mixed_snapshot.records[1].encoding,
            CanonicalRecordEncoding::FramedV1
        );
        assert_eq!(mixed_snapshot.records[0].payload, legacy);
        assert_eq!(mixed_snapshot.records[1].payload, framed);
        assert_eq!(
            mixed_snapshot.head.as_ref().map(|head| head.sequence),
            Some(2)
        );
    }

    fn write_legacy_payload<T: Serialize>(path: &Path, payload: &T) {
        fs::create_dir_all(path.parent().expect("artifact parent"))
            .expect("create artifact parent");
        let mut bytes = serde_json::to_vec(payload).expect("serialize legacy payload");
        bytes.push(b'\n');
        fs::write(path, bytes).expect("write legacy payload");
    }

    fn assert_single_legacy_snapshot<T: PartialEq>(read: StrictCanonicalRead<T>, expected: &T) {
        let snapshot = present_snapshot(&read);
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(
            snapshot.records[0].encoding,
            CanonicalRecordEncoding::LegacyJsonl
        );
        assert!(snapshot.records[0].payload.eq(expected));
        assert_eq!(snapshot.head.as_ref().map(|head| head.sequence), Some(1));
    }

    #[test]
    fn strict_reader_registry_is_stable_and_independently_versioned() {
        assert_eq!(TRANSCRIPT_REVISIONS.stream_id, "transcript_revisions");
        assert_eq!(SPEAKER_REVISIONS.stream_id, "speaker_revisions");
        assert_eq!(PROJECTION_PATCHES.stream_id, "projection_patches");
        assert_eq!(DATA_MOVEMENT_EVENTS.stream_id, "data_movement_events");
        assert_eq!(TRANSCRIPT_REVISIONS_SCHEMA_VERSION, 1);
        assert_eq!(SPEAKER_REVISIONS_SCHEMA_VERSION, 1);
        assert_eq!(PROJECTION_PATCHES_SCHEMA_VERSION, 1);
        assert_eq!(DATA_MOVEMENT_EVENTS_SCHEMA_VERSION, 1);
        assert_eq!(DATA_MOVEMENT_EVENTS_EMBEDDED_SCHEMA_VERSION, 1);
        assert_eq!(
            DATA_MOVEMENT_SCHEMA_VERSION, DATA_MOVEMENT_EVENTS_EMBEDDED_SCHEMA_VERSION,
            "IPC schema drift requires an explicit canonical version mapping"
        );
    }

    #[test]
    fn strict_reader_preserves_missing_and_present_empty_without_creating_roots() {
        let root = unique_tempdir("presence");
        let repo = FileMemoryRepository::with_data_root(&root);
        let session_id = "session-presence";

        assert!(matches!(
            repo.load_transcript_event_stream(session_id).unwrap(),
            StrictCanonicalRead::Missing
        ));
        assert!(matches!(
            repo.load_speaker_revision_stream(session_id).unwrap(),
            StrictCanonicalRead::Missing
        ));
        assert!(matches!(
            repo.load_projection_patch_stream(session_id).unwrap(),
            StrictCanonicalRead::Missing
        ));
        assert!(matches!(
            repo.load_data_movement_event_stream(session_id).unwrap(),
            StrictCanonicalRead::Missing
        ));
        assert!(repo.load_transcript_events(session_id).unwrap().is_empty());
        assert!(repo.load_projection_patches(session_id).unwrap().is_empty());
        assert!(
            repo.load_diarization_span_revisions(session_id)
                .unwrap()
                .is_empty()
        );
        assert!(
            repo.load_data_movement_events(session_id)
                .unwrap()
                .is_empty()
        );
        assert!(
            !root.exists(),
            "missing reads must not create the repository root"
        );

        let path = root
            .join("transcripts")
            .join("session-presence.events.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"\n \r\n").unwrap();
        let read = load_transcript_revisions(&path, session_id).unwrap();
        let snapshot = present_snapshot(&read);
        assert!(snapshot.records.is_empty());
        assert!(snapshot.head.is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn strict_reader_decodes_all_real_payloads_as_legacy_only_and_mixed() {
        let root = unique_tempdir("real-payloads");
        let session_id = "session-real-payloads";

        assert_legacy_then_mixed(
            &root.join("transcript.jsonl"),
            session_id,
            TRANSCRIPT_REVISIONS,
            transcript_event("legacy transcript", 1),
            transcript_event("framed transcript", 2),
            load_transcript_revisions,
        );
        assert_legacy_then_mixed(
            &root.join("speaker.jsonl"),
            session_id,
            SPEAKER_REVISIONS,
            speaker_revision(1),
            speaker_revision(2),
            load_speaker_revisions,
        );
        assert_legacy_then_mixed(
            &root.join("projection.jsonl"),
            session_id,
            PROJECTION_PATCHES,
            projection_patch(1),
            projection_patch(2),
            load_projection_patches,
        );
        assert_legacy_then_mixed(
            &root.join("movement.jsonl"),
            session_id,
            DATA_MOVEMENT_EVENTS,
            movement_event(session_id, 1),
            movement_event(session_id, 2),
            load_data_movement_events,
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn speech_contract_reader_decodes_framed_v1_without_rewriting_bytes() {
        let root = unique_tempdir("speech-v1-frame");
        let path = root.join("transcript.jsonl");
        let session_id = "session-speech-v1-frame";
        let expected = transcript_event("legacy framed transcript", 1);
        let mut appender = CanonicalAppender::<TranscriptEvent>::open(
            &path,
            session_id,
            TRANSCRIPT_REVISIONS.stream_id,
            TRANSCRIPT_REVISIONS.domain_schema_version,
            CanonicalTailRecovery::Strict,
        )
        .expect("open canonical transcript appender");
        assert!(matches!(
            appender.append(&CanonicalEventMetadata::new("legacy-v1"), &expected),
            CanonicalAppendOutcome::Accepted(_)
        ));
        drop(appender);
        let bytes_before = fs::read(&path).expect("read framed bytes");

        let read = load_speech_span_revisions(&path, session_id)
            .expect("compatibility reader decodes framed v1");
        let snapshot = present_snapshot(&read);
        assert_eq!(snapshot.records.len(), 1);
        assert_eq!(
            snapshot.records[0].encoding,
            CanonicalRecordEncoding::FramedV1
        );
        let decoded = snapshot.records[0].payload.clone();
        assert!(matches!(decoded, CompatibleSpeechSpanRevision::LegacyV1(_)));
        let replay_event = decoded
            .into_legacy_transcript_event()
            .expect("fully populated v1 projects without fabrication");
        assert_eq!(replay_event, expected);

        let ledger = TranscriptLedger::replay(session_id, [replay_event.clone()])
            .expect("compatible event replays through transcript ledger");
        let basis = ledger.current_basis();
        assert_eq!(basis.hash_version(), TranscriptHashVersion::V1);
        assert_eq!(
            basis.transcript_hash,
            transcript_events_hash_v1(std::slice::from_ref(&replay_event))
        );
        assert_eq!(basis.transcript_hash, "fnv1a64:1708ff3ca940aa59");
        assert_eq!(ledger.validate_basis(&basis), Ok(()));
        assert_eq!(fs::read(&path).expect("re-read framed bytes"), bytes_before);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pre_hash_version_projection_patch_decodes_and_replays_without_rewrite() {
        let root = unique_tempdir("pre-hash-version-patch");
        let path = root.join("projection.jsonl");
        let session_id = "session-pre-hash-version";
        let expected = projection_patch(1);
        let mut legacy_value = serde_json::to_value(&expected).expect("serialize patch fixture");
        legacy_value["basis"]
            .as_object_mut()
            .expect("basis object")
            .remove("hash_version");
        let mut bytes = serde_json::to_vec(&legacy_value).expect("serialize legacy patch");
        bytes.push(b'\n');
        fs::create_dir_all(&root).expect("create fixture root");
        fs::write(&path, &bytes).expect("write legacy accepted patch");

        let read = load_projection_patches(&path, session_id)
            .expect("decode accepted patch with absent hash version");
        let snapshot = present_snapshot(&read);
        assert_eq!(snapshot.records[0].payload, expected);
        let replayed = MaterializedProjectionState::replay_accepted_patches(
            session_id,
            [snapshot.records[0].payload.clone()],
        )
        .expect("replay pre-version accepted patch");
        assert_eq!(replayed.notes.notes.len(), 1);
        assert_eq!(fs::read(&path).expect("re-read legacy bytes"), bytes);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn strict_reader_file_repository_routes_all_actual_stream_paths() {
        let root = unique_tempdir("repository-routes");
        let repo = FileMemoryRepository::with_data_root(&root);
        let session_id = "session-repository-routes";
        let transcript = transcript_event("repository transcript", 1);
        let speaker = speaker_revision(1);
        let projection = projection_patch(1);
        let movement = movement_event(session_id, 1);

        write_legacy_payload(
            &repo.resolve_transcript_events_path(session_id).unwrap(),
            &transcript,
        );
        write_legacy_payload(
            &repo.resolve_diarization_events_path(session_id).unwrap(),
            &speaker,
        );
        write_legacy_payload(
            &repo.resolve_projection_events_path(session_id).unwrap(),
            &projection,
        );
        write_legacy_payload(
            &repo.resolve_data_movement_ledger_path(session_id).unwrap(),
            &movement,
        );

        assert_single_legacy_snapshot(
            repo.load_transcript_event_stream(session_id).unwrap(),
            &transcript,
        );
        assert_single_legacy_snapshot(
            repo.load_speaker_revision_stream(session_id).unwrap(),
            &speaker,
        );
        assert_single_legacy_snapshot(
            repo.load_projection_patch_stream(session_id).unwrap(),
            &projection,
        );
        assert_single_legacy_snapshot(
            repo.load_data_movement_event_stream(session_id).unwrap(),
            &movement,
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn strict_reader_fails_closed_and_does_not_mutate_corrupt_content() {
        let root = unique_tempdir("corrupt");
        fs::create_dir_all(&root).unwrap();
        let path = root.join("stream.jsonl");
        let secret = "payload-secret-must-not-escape";
        fs::write(&path, format!("{{\"text\":\"{secret}\"\n")).unwrap();
        let before = fs::read(&path).unwrap();
        let before_entries = fs::read_dir(&root).unwrap().count();

        let error = match load_transcript_revisions(&path, "session-corrupt") {
            Ok(_) => panic!("corrupt content must fail"),
            Err(error) => error,
        };
        assert!(!error.to_string().contains(secret));
        assert!(!format!("{error:?}").contains(secret));
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(&root).unwrap().count(), before_entries);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn strict_reader_validates_movement_session_and_schema_for_legacy_and_framed_rows() {
        let root = unique_tempdir("movement-context");
        fs::create_dir_all(&root).unwrap();
        let session_id = "session-movement";

        let legacy_path = root.join("legacy.jsonl");
        let wrong_session = movement_event("another-session", 1);
        fs::write(&legacy_path, serde_json::to_vec(&wrong_session).unwrap()).unwrap();
        assert!(matches!(
            load_data_movement_events(&legacy_path, session_id),
            Err(CanonicalReaderError::DataMovementSessionMismatch { record_index: 0 })
        ));

        let framed_path = root.join("framed.jsonl");
        let mut wrong_schema = movement_event(session_id, 2);
        wrong_schema.schema_version = DATA_MOVEMENT_EVENTS_EMBEDDED_SCHEMA_VERSION + 1;
        let mut appender = CanonicalAppender::<DataMovementEvent>::open(
            &framed_path,
            session_id,
            DATA_MOVEMENT_EVENTS.stream_id,
            DATA_MOVEMENT_EVENTS.domain_schema_version,
            CanonicalTailRecovery::Strict,
        )
        .unwrap();
        assert!(matches!(
            appender.append(&CanonicalEventMetadata::new("wrong-schema"), &wrong_schema),
            CanonicalAppendOutcome::Accepted(_)
        ));
        drop(appender);
        assert!(matches!(
            load_data_movement_events(&framed_path, session_id),
            Err(CanonicalReaderError::DataMovementSchemaVersionMismatch { record_index: 0 })
        ));

        let _ = fs::remove_dir_all(root);
    }
}
