//! Backend helpers for the session data-movement ledger (seed audio-graph-70a3).
//!
//! The audit event *schema* lives in the dependency-light
//! `audio-graph-ipc-contract` crate (so the frontend TS contract in
//! `src/generated/sessionDataMovement.ts` can be generated without linking the
//! full app). This module owns the app-side ergonomics: a builder that stamps
//! the `event_id`, `schema_version`, and `created_at_ms`, and a redaction-safe
//! path hasher for artifact references.
//!
//! Persistence itself (append/load to the per-session ledger JSONL) is wired
//! through [`LocalMemoryRepository`](super::LocalMemoryRepository) and
//! [`FileMemoryRepository`](super::FileMemoryRepository) in the parent module,
//! matching the transcript/projection/diarization event-log pattern.

use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    ArtifactRef, ArtifactStorageKind, DATA_MOVEMENT_SCHEMA_VERSION, DataClass, DataMovementActor,
    DataMovementDestination, DataMovementEvent, DataMovementEventType, DataMovementResult,
    DataMovementSource, MovementBasis, MovementCounts, MovementModel, MovementPolicy,
};

/// Redaction-safe fingerprint of an artifact path/uri.
///
/// The data-movement ledger must be able to say *which* artifact moved without
/// leaking a filesystem path (which can embed a username or session title). We
/// therefore never store the raw path — only this opaque fingerprint.
///
/// This is intentionally *not* a cryptographic hash: the requirement is
/// redaction (one-way, non-reversible display token), not integrity or
/// collision resistance, so we use the standard-library hasher and avoid
/// pulling in a crypto dependency. The `h64:` prefix documents the algorithm
/// honestly rather than masquerading as `sha256:`.
pub fn hash_artifact_path(path: &Path) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hasher);
    format!("h64:{:016x}", hasher.finish())
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Ergonomic builder for a [`DataMovementEvent`].
///
/// Stamps `event_id` (uuid v4), `schema_version`, and `created_at_ms` so
/// call-sites only supply the trust-relevant fields. Every optional field
/// defaults to omitted, keeping pure lifecycle events (e.g. a readiness check
/// that sends no content) compact.
///
/// The builder cannot express a field capable of carrying a raw secret: error
/// messages must go through [`DataMovementResult::failed`], which redacts, and
/// there is no free-form "payload" field by design.
#[derive(Debug, Clone)]
pub struct DataMovementLedgerBuilder {
    event: DataMovementEvent,
}

impl DataMovementLedgerBuilder {
    /// Start a new event for `session_id` with the given actor, type, policy,
    /// and destination. `result` defaults to `succeeded`; override with
    /// [`Self::result`].
    pub fn new(
        session_id: impl Into<String>,
        actor: DataMovementActor,
        event_type: DataMovementEventType,
        policy: MovementPolicy,
        destination: DataMovementDestination,
    ) -> Self {
        Self {
            event: DataMovementEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                schema_version: DATA_MOVEMENT_SCHEMA_VERSION,
                session_id: session_id.into(),
                created_at_ms: now_millis(),
                actor,
                event_type,
                data_classes: Vec::new(),
                source: None,
                destination,
                artifact_refs: Vec::new(),
                basis: None,
                model: None,
                counts: None,
                policy,
                result: DataMovementResult::succeeded(),
            },
        }
    }

    /// Override the auto-generated timestamp (primarily for deterministic
    /// tests and replayed backfills).
    pub fn created_at_ms(mut self, created_at_ms: u64) -> Self {
        self.event.created_at_ms = created_at_ms;
        self
    }

    pub fn data_classes(mut self, classes: impl IntoIterator<Item = DataClass>) -> Self {
        self.event.data_classes = classes.into_iter().collect();
        self
    }

    pub fn source(mut self, source: DataMovementSource) -> Self {
        self.event.source = Some(source);
        self
    }

    pub fn model(mut self, model: MovementModel) -> Self {
        self.event.model = Some(model);
        self
    }

    pub fn counts(mut self, counts: MovementCounts) -> Self {
        self.event.counts = Some(counts);
        self
    }

    pub fn basis(mut self, basis: MovementBasis) -> Self {
        self.event.basis = Some(basis);
        self
    }

    /// Add an artifact reference, hashing the concrete path so the raw path
    /// never lands in the ledger.
    pub fn artifact(
        mut self,
        kind: impl Into<String>,
        storage: ArtifactStorageKind,
        path: &Path,
    ) -> Self {
        self.event.artifact_refs.push(ArtifactRef {
            kind: kind.into(),
            storage,
            path_hash: Some(hash_artifact_path(path)),
        });
        self
    }

    pub fn result(mut self, result: DataMovementResult) -> Self {
        self.event.result = result;
        self
    }

    /// Finish and return the event.
    pub fn build(self) -> DataMovementEvent {
        self.event
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{DestinationBoundary, MovementStatus, PrivacyMode, RetentionClass};

    fn cloud_policy() -> MovementPolicy {
        MovementPolicy {
            privacy_mode: PrivacyMode::ByokCloud,
            user_visible: true,
            retention_class: RetentionClass::Transient,
        }
    }

    #[test]
    fn hash_artifact_path_is_stable_and_hides_raw_path() {
        let path = Path::new("/home/alice/.audiograph/transcripts/session-secret.events.jsonl");
        let hash = hash_artifact_path(path);
        assert!(hash.starts_with("h64:"));
        assert!(!hash.contains("alice"));
        assert!(!hash.contains("session-secret"));
        // Deterministic across calls.
        assert_eq!(hash, hash_artifact_path(path));
        // Different paths hash differently.
        assert_ne!(hash, hash_artifact_path(Path::new("/tmp/other.jsonl")));
    }

    #[test]
    fn builder_stamps_identity_fields_and_defaults_to_succeeded() {
        let event = DataMovementLedgerBuilder::new(
            "session-1",
            DataMovementActor::System,
            DataMovementEventType::CaptureStarted,
            cloud_policy(),
            DataMovementDestination::local(),
        )
        .build();

        assert_eq!(event.schema_version, DATA_MOVEMENT_SCHEMA_VERSION);
        assert_eq!(event.session_id, "session-1");
        assert!(!event.event_id.is_empty());
        assert!(event.created_at_ms > 0);
        assert_eq!(event.result.status, MovementStatus::Succeeded);
        assert_eq!(event.destination.boundary, DestinationBoundary::Local);
    }

    #[test]
    fn builder_hashes_artifact_path() {
        let path = Path::new("/home/bob/.audiograph/ledgers/s.movements.jsonl");
        let event = DataMovementLedgerBuilder::new(
            "session-1",
            DataMovementActor::System,
            DataMovementEventType::ArtifactWritten,
            cloud_policy(),
            DataMovementDestination::local(),
        )
        .artifact("data_movement_ledger", ArtifactStorageKind::File, path)
        .build();

        let artifact = &event.artifact_refs[0];
        assert_eq!(artifact.kind, "data_movement_ledger");
        let hash = artifact.path_hash.as_deref().expect("path hash");
        assert!(hash.starts_with("h64:"));
        assert!(!hash.contains("bob"));
    }
}
