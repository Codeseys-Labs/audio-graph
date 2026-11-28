//! `speaker_revisions` reader floor is independent of the session semantics floor.
use super::canonical_log::{CanonicalAppendOutcome, CanonicalAppender, CanonicalEventMetadata};
use super::canonical_reader::StrictCanonicalRead;
use crate::identity::relabel::SpeakerRelabelBatch;
use crate::projections::{DiarizationSpanRevision, SpeakerTimeline};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum CompatibleSpeakerRevisionRecord {
    Versioned(SpeakerRevisionV2),
    Legacy(Box<DiarizationSpanRevision>),
}
impl std::fmt::Debug for CompatibleSpeakerRevisionRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompatibleSpeakerRevisionRecord { .. }")
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SpeakerRevisionV2 {
    SpanRevision {
        revision: Box<DiarizationSpanRevision>,
    },
    RelabelBatch(SpeakerRelabelBatch),
}
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum SpeakerWrite<'a> {
    SpanRevision {
        revision: &'a DiarizationSpanRevision,
    },
    RelabelBatch(&'a SpeakerRelabelBatch),
}

/// Sidecars created by the legacy recovery protocol belong to the same session as
/// the speaker log. Only recognize exact numeric recovery names, never symlinks.
pub(crate) fn legacy_quarantines(path: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let Some(parent) = path.parent() else {
        return Ok(Vec::new());
    };
    let prefix = format!(
        "{}.corrupt-tail-",
        path.file_name()
            .and_then(|n| n.to_str())
            .ok_or("invalid speaker log name")?
    );
    let entries = match std::fs::read_dir(parent) {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.to_string()),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if let Some(suffix) = name.to_str().and_then(|s| s.strip_prefix(&prefix)) {
            let mut fields = suffix.split('-');
            if (0..3).all(|_| {
                fields
                    .next()
                    .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            }) && fields.next().is_none()
                && entry.file_type().map_err(|e| e.to_string())?.is_file()
            {
                paths.push(entry.path())
            }
        }
    }
    paths.sort();
    Ok(paths)
}
type SpeakerAppender = CanonicalAppender<CompatibleSpeakerRevisionRecord>;

fn open_writer(
    path: &Path,
    session_id: &str,
    row: &SpeakerWrite<'_>,
) -> Result<SpeakerAppender, String> {
    use super::canonical_log::{
        CanonicalLogError, CanonicalRecoveryDescriptor, CanonicalRecoveryOutcome,
        CanonicalRecoveryTransaction, CanonicalTailRecovery,
    };
    use super::session_artifact_manifest::{
        ManagedArtifactIdentity, SessionArtifactManifestStore, Sha256Digest,
    };
    use sha2::{Digest, Sha256};
    let open = |mode| SpeakerAppender::open_speaker(path, session_id, mode);
    match open(CanonicalTailRecovery::Strict) {
        Err(CanonicalLogError::CorruptRecord {
            newline_terminated: false,
            ..
        }) => {}
        other => return other.map_err(|e| e.to_string()),
    }
    let root = path
        .parent()
        .and_then(Path::parent)
        .ok_or("invalid speaker log root")?;
    let control = super::session_semantics::session_control_plane_paths(root, session_id)
        .map_err(|_| "invalid speaker session")?;
    let mut managed = false;
    for p in control.all() {
        match std::fs::symlink_metadata(p) {
            Ok(_) => managed = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("cannot classify speaker recovery namespace".into()),
        }
    }
    if !managed {
        return open(CanonicalTailRecovery::QuarantineUnterminatedTail).map_err(|e| e.to_string());
    }
    // Never use legacy quarantine/truncation on manifest-owned content. A platform
    // without qualified namespace durability fails closed before mutating the source.
    let store = SessionArtifactManifestStore::qualified_existing_session(root, session_id)
        .map_err(|_| "managed speaker recovery unavailable")?;
    let digest = hex::encode(Sha256::digest(
        serde_json::to_vec(row).map_err(|_| "invalid speaker record")?,
    ));
    let src = format!("transcripts/{session_id}.speaker.jsonl");
    let identity = |path| ManagedArtifactIdentity::new(path).map_err(|_| "invalid recovery path");
    let descriptor = CanonicalRecoveryDescriptor::new(
        session_id,
        "speaker_revisions",
        2,
        format!("speaker-recovery:{digest}"),
        Sha256Digest::new(format!("sha256:{digest}")).map_err(|_| "invalid recovery digest")?,
        identity(src.clone())?,
        identity(format!("{src}.recovery-{digest}.tmp"))?,
        identity(format!("{src}.recovery-{digest}.bin"))?,
    )
    .map_err(|_| "invalid speaker recovery descriptor")?;
    let mut transaction =
        CanonicalRecoveryTransaction::begin::<CompatibleSpeakerRevisionRecord>(&store, descriptor)
            .map_err(|_| "managed speaker recovery refused")?;
    match transaction.execute() {
        CanonicalRecoveryOutcome::Accepted(_) | CanonicalRecoveryOutcome::AlreadyCompleted(_) => {}
        _ => return Err("managed speaker recovery not committed".into()),
    };
    drop(transaction);
    open(CanonicalTailRecovery::Strict).map_err(|e| e.to_string())
}
pub(super) fn append(path: &Path, session_id: &str, row: SpeakerWrite<'_>) -> Result<(), String> {
    let mut writer = open_writer(path, session_id, &row)?;
    let outcome = match row {
        SpeakerWrite::SpanRevision { revision } => {
            let id = CanonicalEventMetadata::new(format!(
                "speaker-span:{}@rev{}",
                revision.span_id, revision.revision_number
            ));
            if writer.speaker_version() == 1 && revision.identity.is_none() {
                writer.append_serializable(&id, revision)
            } else {
                writer.activate_speaker_v2();
                writer.append_serializable(&id, &row)
            }
        }
        SpeakerWrite::RelabelBatch(batch) => {
            batch.validate_header()?;
            writer.activate_speaker_v2();
            writer.append_serializable(
                &CanonicalEventMetadata::new(format!("speaker-relabel:{}", batch.batch_id)),
                &row,
            )
        }
    };
    match outcome {
        CanonicalAppendOutcome::Accepted(_) | CanonicalAppendOutcome::AlreadyAccepted(_) => {
            // Pin this file while the appender still owns its lock. NTFS can
            // finalize write/change times only when the write handle closes.
            let pinned = std::fs::File::open(path).ok();
            let head = writer.head().cloned();
            drop(writer);
            if let Some(file) = pinned {
                super::canonical_log::remember_speaker_head(path, &file, head.as_ref());
            }
            Ok(())
        }
        other => Err(format!("speaker append: {other:?}")),
    }
}

impl CompatibleSpeakerRevisionRecord {
    pub(super) fn fold(
        self,
        version: u32,
        event_id: &str,
        timeline: Option<&mut SpeakerTimeline>,
        history: Option<&mut Vec<DiarizationSpanRevision>>,
    ) -> Result<(), String> {
        let span = match (version, self) {
            (2, Self::Versioned(SpeakerRevisionV2::RelabelBatch(batch))) => {
                if event_id != format!("speaker-relabel:{}", batch.batch_id) {
                    return Err("invalid speaker batch event id".into());
                }
                timeline
                    .ok_or("missing batch replay context")?
                    .apply_batch(&batch)?;
                if let Some(rows) = history {
                    rows.extend(batch.revisions)
                }
                return Ok(());
            }
            (1, Self::Legacy(mut span)) => {
                span.identity = None;
                span.provider_revision_number = None;
                span
            }
            (2, Self::Versioned(SpeakerRevisionV2::SpanRevision { revision })) => {
                if revision.identity.as_ref().is_some_and(|s| {
                    s.schema_version != 1
                        || s.batch.is_some()
                        || s.state == crate::identity::BindingState::Suggested
                }) {
                    return Err("invalid singleton identity".into());
                }
                revision
            }
            _ => return Err("unexpected speaker record version".into()),
        };
        match (timeline, history) {
            (Some(t), Some(rows)) => {
                t.apply_event((*span).clone())
                    .map_err(|_| "invalid speaker history")?;
                rows.push(*span)
            }
            (Some(t), None) => {
                t.apply_event(*span)
                    .map_err(|_| "invalid speaker history")?;
            }
            (_, Some(rows)) => rows.push(*span),
            _ => {}
        }
        Ok(())
    }
}
impl StrictCanonicalRead<CompatibleSpeakerRevisionRecord> {
    /// Validate each transaction against its accepted prefix before exposing any of its rows.
    /// Uniform receipt times keep projection cutoffs atomic after flattening.
    pub(crate) fn speaker_history(
        self,
        session_id: &str,
    ) -> Result<Option<Vec<DiarizationSpanRevision>>, String> {
        let Self::Present(snapshot) = self else {
            return Ok(None);
        };
        let mut timeline = snapshot
            .records
            .iter()
            .any(|r| r.domain_schema_version == 2)
            .then(|| SpeakerTimeline::new(session_id));
        let mut rows = Vec::with_capacity(snapshot.records.len());
        for record in snapshot.records {
            record.payload.fold(
                record.domain_schema_version,
                &record.event_id,
                timeline.as_mut(),
                Some(&mut rows),
            )?;
        }
        Ok(Some(rows))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{ClusterRef, ManualEdit, manual_batch, tests::span};
    use crate::persistence::{
        canonical_log::{
            CanonicalCorruptionReason, CanonicalLogError, CanonicalTailRecovery,
            load_canonical_stream,
        },
        canonical_reader::load_speaker_revisions,
    };
    use std::fs;
    #[test]
    fn mixed_stream_has_one_atomic_batch_sticky_floor_and_torn_tail_recovery() {
        let dir = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&dir).unwrap();
        let path = dir.join("s.speaker.jsonl");
        let a = span("a", "mic");
        let b = span("b", "mic");
        let c = span("c", "sys");
        let origin = ClusterRef::origin(&a).unwrap();
        fs::write(&path, format!("{}\n", serde_json::to_string(&a).unwrap())).unwrap();
        append(&path, "s", SpeakerWrite::SpanRevision { revision: &b }).unwrap();
        let tl = SpeakerTimeline::replay("s", [a, b]).unwrap();
        let batch = manual_batch(
            &tl,
            &origin,
            ManualEdit::Name("Ada".into()),
            10,
            || unreachable!(),
        )
        .unwrap();
        append(&path, "s", SpeakerWrite::RelabelBatch(&batch)).unwrap();
        let committed = fs::read(&path).unwrap();
        append(&path, "s", SpeakerWrite::RelabelBatch(&batch)).unwrap();
        assert_eq!(fs::read(&path).unwrap(), committed);
        let mut conflicting = batch.clone();
        conflicting.effective_at_ms += 1;
        assert!(append(&path, "s", SpeakerWrite::RelabelBatch(&conflicting)).is_err());
        assert_eq!(committed.iter().filter(|&&b| b == b'\n').count(), 3);
        let err = load_canonical_stream::<serde_json::Value>(
            &path,
            "s",
            "speaker_revisions",
            1,
            CanonicalTailRecovery::Strict,
        )
        .unwrap_err();
        assert!(matches!(
            err,
            CanonicalLogError::CorruptRecord {
                record_index: 3,
                reason: CanonicalCorruptionReason::DomainSchemaVersionMismatch,
                ..
            }
        ));
        let read = || {
            load_speaker_revisions(&path, "s")
                .unwrap()
                .speaker_history("s")
                .unwrap()
                .unwrap()
        };
        assert_eq!(read().iter().filter(|r| r.received_at_ms == 10).count(), 2);
        fs::write(&path, &committed[..committed.len() - 1]).unwrap();
        assert_eq!(read().len(), 2);
        append(&path, "s", SpeakerWrite::RelabelBatch(&batch)).unwrap();
        let quarantines = legacy_quarantines(&path).unwrap();
        assert_eq!(quarantines.len(), 1);
        assert!(!fs::read(&quarantines[0]).unwrap().is_empty());
        assert_eq!(fs::read(&path).unwrap(), committed);
        append(&path, "s", SpeakerWrite::SpanRevision { revision: &c }).unwrap();
        let StrictCanonicalRead::Present(snapshot) = load_speaker_revisions(&path, "s").unwrap()
        else {
            panic!()
        };
        assert_eq!(
            snapshot
                .records
                .iter()
                .map(|r| r.domain_schema_version)
                .collect::<Vec<_>>(),
            [1, 1, 2, 2]
        );
        assert_eq!(read().len(), 5);
        let mut bad = fs::read(&path).unwrap();
        bad.extend_from_slice(b"bad\n");
        fs::write(&path, &bad).unwrap();
        assert!(load_speaker_revisions(&path, "s").is_err());
        assert!(append(&path, "s", SpeakerWrite::RelabelBatch(&batch)).is_err());
        assert_eq!(fs::read(&path).unwrap(), bad);
        // A syntactically valid, correctly hashed frame with contradictory membership
        // must also poison the stream, not become a prefix to later accepted edits.
        fs::write(&path, &committed).unwrap();
        let mut invalid = batch.clone();
        invalid.batch_id = "invalid".into();
        {
            let mut writer =
                SpeakerAppender::open_speaker(&path, "s", CanonicalTailRecovery::Strict).unwrap();
            assert!(matches!(
                writer.append_serializable(
                    &CanonicalEventMetadata::new("speaker-relabel:invalid"),
                    &SpeakerWrite::RelabelBatch(&invalid)
                ),
                CanonicalAppendOutcome::Accepted(_)
            ));
        }
        assert!(
            load_speaker_revisions(&path, "s")
                .unwrap()
                .speaker_history("s")
                .is_err()
        );
        assert!(append(&path, "s", SpeakerWrite::SpanRevision { revision: &c }).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
