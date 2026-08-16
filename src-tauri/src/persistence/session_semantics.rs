//! Dormant Session-semantics compatibility-floor kernel.

use serde::{Deserialize, Serialize};

use super::session_artifact_manifest::{
    ManifestCasOutcome, SessionArtifactManifestV1, V2SessionProvenanceError,
    validate_v2_session_provenance,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionSemanticsVersion(u32);

impl SessionSemanticsVersion {
    pub const V1: Self = Self(1);
    pub const V2: Self = Self(2);

    pub const fn historical_default() -> Self {
        Self::V1
    }

    pub const fn as_u32(self) -> u32 {
        self.0
    }

    pub const fn is_supported(self) -> bool {
        matches!(self.0, 1 | 2)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSemanticsAdvanceError {
    UnsupportedCurrentFloor {
        actual: u32,
    },
    EvidenceSessionMismatch,
    EvidenceUnsupportedFloor {
        actual: u32,
    },
    InvalidV2SessionProvenance(V2SessionProvenanceError),
    IllegalTransition {
        current: SessionSemanticsVersion,
        accepted: SessionSemanticsVersion,
    },
    ManifestCasNotAccepted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSemanticsArtifact {
    TranscriptRevisionV2,
    ProjectionBasisV2,
    ProjectionPatchV2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSemanticsCorruption {
    V2TranscriptUnderV1,
    V2ProjectionBasisUnderV1,
    V2ProjectionPatchUnderV1,
    UnsupportedSessionFloor { actual: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedSessionOpenError<E> {
    UnsupportedReaderFloor {
        actual: u32,
    },
    UnsupportedSessionFloor {
        required: SessionSemanticsVersion,
        maximum_supported: SessionSemanticsVersion,
    },
    InvalidSessionFloor {
        actual: u32,
    },
    ContentReader(E),
}

/// Validate the compatibility floor before any canonical or legacy content
/// reader is allowed to observe Session bytes.
pub fn checked_session_open<T, E>(
    manifest: &SessionArtifactManifestV1,
    maximum_supported: SessionSemanticsVersion,
    content_reader: impl FnOnce() -> Result<T, E>,
) -> Result<T, CheckedSessionOpenError<E>> {
    if !maximum_supported.is_supported() {
        return Err(CheckedSessionOpenError::UnsupportedReaderFloor {
            actual: maximum_supported.as_u32(),
        });
    }
    let required = manifest.session_semantics_version;
    if !required.is_supported() {
        return Err(CheckedSessionOpenError::InvalidSessionFloor {
            actual: required.as_u32(),
        });
    }
    if required > maximum_supported {
        return Err(CheckedSessionOpenError::UnsupportedSessionFloor {
            required,
            maximum_supported,
        });
    }
    content_reader().map_err(CheckedSessionOpenError::ContentReader)
}

/// Refuse artifact-ahead state without inspecting or copying artifact content.
pub fn validate_artifact_semantics(
    floor: SessionSemanticsVersion,
    artifact: SessionSemanticsArtifact,
) -> Result<(), SessionSemanticsCorruption> {
    match floor {
        SessionSemanticsVersion::V1 => Err(match artifact {
            SessionSemanticsArtifact::TranscriptRevisionV2 => {
                SessionSemanticsCorruption::V2TranscriptUnderV1
            }
            SessionSemanticsArtifact::ProjectionBasisV2 => {
                SessionSemanticsCorruption::V2ProjectionBasisUnderV1
            }
            SessionSemanticsArtifact::ProjectionPatchV2 => {
                SessionSemanticsCorruption::V2ProjectionPatchUnderV1
            }
        }),
        SessionSemanticsVersion::V2 => Ok(()),
        unsupported => Err(SessionSemanticsCorruption::UnsupportedSessionFloor {
            actual: unsupported.as_u32(),
        }),
    }
}

/// Resolve the logical Session floor from the manifest CAS result itself.
///
/// No receipt-shaped public input or caller-controlled success flag can cross
/// this boundary: the authoritative manifest carried by the CAS outcome is
/// the evidence under classification.
pub fn admitted_session_semantics_floor(
    expected_session_id: &str,
    current: SessionSemanticsVersion,
    outcome: &ManifestCasOutcome,
) -> Result<SessionSemanticsVersion, SessionSemanticsAdvanceError> {
    if !current.is_supported() {
        return Err(SessionSemanticsAdvanceError::UnsupportedCurrentFloor {
            actual: current.as_u32(),
        });
    }
    let (manifest, exact_retry) = match outcome {
        ManifestCasOutcome::Accepted { manifest, .. } => (manifest, false),
        ManifestCasOutcome::AlreadyCompleted { manifest } => (manifest, true),
        ManifestCasOutcome::Rejected(_) | ManifestCasOutcome::DurabilityIndeterminate(_) => {
            return Err(SessionSemanticsAdvanceError::ManifestCasNotAccepted);
        }
    };
    if manifest.session_id != expected_session_id {
        return Err(SessionSemanticsAdvanceError::EvidenceSessionMismatch);
    }
    let accepted = manifest.session_semantics_version;
    if !accepted.is_supported() {
        return Err(SessionSemanticsAdvanceError::EvidenceUnsupportedFloor {
            actual: accepted.as_u32(),
        });
    }
    if accepted == SessionSemanticsVersion::V2 {
        validate_v2_session_provenance(manifest)
            .map_err(SessionSemanticsAdvanceError::InvalidV2SessionProvenance)?;
    }
    if exact_retry && current == accepted {
        return Ok(current);
    }
    if current == SessionSemanticsVersion::V1 && accepted == SessionSemanticsVersion::V2 {
        return Ok(SessionSemanticsVersion::V2);
    }
    Err(SessionSemanticsAdvanceError::IllegalTransition { current, accepted })
}

#[cfg(test)]
mod tests {
    use super::{
        CheckedSessionOpenError, SessionSemanticsAdvanceError, SessionSemanticsArtifact,
        SessionSemanticsCorruption, SessionSemanticsVersion, admitted_session_semantics_floor,
        checked_session_open, validate_artifact_semantics,
    };
    use crate::persistence::session_artifact_manifest::{
        ArtifactAvailability, ArtifactContentIdentity, ArtifactPrivacyClass,
        ArtifactUnavailableReason, ManagedArtifactIdentity, ManifestCasOutcome, ManifestStoreError,
        ManifestTransition, ManifestTransitionState, SessionArtifactEntry, SessionArtifactKind,
        SessionArtifactManifestStore, SessionArtifactManifestV1, Sha256Digest,
        V2SessionProvenanceError,
    };

    fn identity(value: &str) -> ManagedArtifactIdentity {
        ManagedArtifactIdentity::new(value).expect("managed identity")
    }

    fn digest(byte: char) -> Sha256Digest {
        Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).expect("digest")
    }

    fn v2_candidate() -> SessionArtifactManifestV1 {
        let mut candidate = SessionArtifactManifestV1::candidate(
            "session-floor",
            ManifestTransition {
                idempotency_id: "advance-floor-v2".to_owned(),
                fingerprint: digest('b'),
                state: ManifestTransitionState::Completed,
            },
            vec![
                SessionArtifactEntry {
                    kind: SessionArtifactKind::OriginalSessionAudio,
                    privacy_class: ArtifactPrivacyClass::OriginalEvidence,
                    managed_identity: identity("audio/original.wav"),
                    availability: ArtifactAvailability::Unavailable {
                        reason: ArtifactUnavailableReason::RetentionDisabled,
                    },
                },
                SessionArtifactEntry {
                    kind: SessionArtifactKind::SessionProvenanceEvents,
                    privacy_class: ArtifactPrivacyClass::CanonicalSessionMemory,
                    managed_identity: identity("streams/session-provenance.jsonl"),
                    availability: ArtifactAvailability::Present {
                        content: ArtifactContentIdentity {
                            sha256: digest('b'),
                            byte_length: 48,
                        },
                    },
                },
            ],
            None,
        )
        .expect("candidate");
        candidate.session_semantics_version = SessionSemanticsVersion::V2;
        candidate
    }

    #[test]
    fn historical_missing_floor_resolves_to_v1() {
        assert_eq!(
            SessionSemanticsVersion::historical_default(),
            SessionSemanticsVersion::V1
        );
    }

    #[cfg(unix)]
    #[test]
    fn accepted_manifest_cas_advances_the_logical_floor() {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-b887-accepted-floor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("root");
        let store = SessionArtifactManifestStore::qualified_for_test(&root).expect("qualified");
        let mut transaction = store.begin_write().expect("transaction");
        let outcome = transaction.compare_and_swap(0, v2_candidate());
        assert!(matches!(outcome, ManifestCasOutcome::Accepted { .. }));

        assert_eq!(
            admitted_session_semantics_floor(
                "session-floor",
                SessionSemanticsVersion::V1,
                &outcome,
            ),
            Ok(SessionSemanticsVersion::V2)
        );
    }

    #[cfg(unix)]
    #[test]
    fn forged_accepted_manifest_proof_cannot_advance_the_logical_floor() {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-b887-forged-accepted-floor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("root");
        let store = SessionArtifactManifestStore::qualified_for_test(&root).expect("qualified");
        let mut transaction = store.begin_write().expect("transaction");
        let mut outcome = transaction.compare_and_swap(0, v2_candidate());
        let ManifestCasOutcome::Accepted { manifest, .. } = &mut outcome else {
            panic!("qualified v2 CAS must be accepted");
        };
        manifest
            .artifacts
            .iter_mut()
            .find(|artifact| artifact.kind == SessionArtifactKind::SessionProvenanceEvents)
            .expect("provenance")
            .privacy_class = ArtifactPrivacyClass::AuditRecord;

        assert_eq!(
            admitted_session_semantics_floor(
                "session-floor",
                SessionSemanticsVersion::V1,
                &outcome,
            ),
            Err(SessionSemanticsAdvanceError::InvalidV2SessionProvenance(
                V2SessionProvenanceError::PrivacyMismatch
            ))
        );
    }

    #[cfg(unix)]
    #[test]
    fn exact_guard_ahead_manifest_retry_is_idempotent() {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-b887-guard-ahead-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("root");
        let store = SessionArtifactManifestStore::qualified_for_test(&root).expect("qualified");
        let mut transaction = store.begin_write().expect("transaction");
        let candidate = v2_candidate();
        assert!(matches!(
            transaction.compare_and_swap(0, candidate.clone()),
            ManifestCasOutcome::Accepted { .. }
        ));
        let retry = transaction.compare_and_swap(1, candidate);
        assert!(matches!(retry, ManifestCasOutcome::AlreadyCompleted { .. }));

        assert_eq!(
            admitted_session_semantics_floor("session-floor", SessionSemanticsVersion::V2, &retry,),
            Ok(SessionSemanticsVersion::V2)
        );
    }

    #[cfg(unix)]
    #[test]
    fn forged_already_completed_manifest_proof_cannot_preserve_the_logical_floor() {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-b887-forged-retry-floor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("root");
        let store = SessionArtifactManifestStore::qualified_for_test(&root).expect("qualified");
        let mut transaction = store.begin_write().expect("transaction");
        let candidate = v2_candidate();
        assert!(matches!(
            transaction.compare_and_swap(0, candidate.clone()),
            ManifestCasOutcome::Accepted { .. }
        ));
        let mut retry = transaction.compare_and_swap(1, candidate);
        let ManifestCasOutcome::AlreadyCompleted { manifest } = &mut retry else {
            panic!("exact retry must be already completed");
        };
        manifest.transition.fingerprint = digest('c');

        assert_eq!(
            admitted_session_semantics_floor("session-floor", SessionSemanticsVersion::V2, &retry,),
            Err(SessionSemanticsAdvanceError::InvalidV2SessionProvenance(
                V2SessionProvenanceError::TransitionFingerprintMismatch
            ))
        );
    }

    #[test]
    fn artifact_ahead_under_v1_has_distinct_content_free_corruption() {
        for (artifact, expected) in [
            (
                SessionSemanticsArtifact::TranscriptRevisionV2,
                SessionSemanticsCorruption::V2TranscriptUnderV1,
            ),
            (
                SessionSemanticsArtifact::ProjectionBasisV2,
                SessionSemanticsCorruption::V2ProjectionBasisUnderV1,
            ),
            (
                SessionSemanticsArtifact::ProjectionPatchV2,
                SessionSemanticsCorruption::V2ProjectionPatchUnderV1,
            ),
        ] {
            assert_eq!(
                validate_artifact_semantics(SessionSemanticsVersion::V1, artifact),
                Err(expected)
            );
        }
    }

    #[test]
    fn checked_open_refuses_unsupported_floor_before_content_reader() {
        let manifest = v2_candidate();
        let reader_invoked = std::cell::Cell::new(false);

        assert_eq!(
            checked_session_open(
                &manifest,
                SessionSemanticsVersion::V1,
                || -> Result<&'static str, ()> {
                    reader_invoked.set(true);
                    Ok("opened")
                },
            ),
            Err(CheckedSessionOpenError::UnsupportedSessionFloor {
                required: SessionSemanticsVersion::V2,
                maximum_supported: SessionSemanticsVersion::V1,
            })
        );
        assert!(!reader_invoked.get());
    }

    #[test]
    fn unqualified_manifest_begin_refuses_before_floor_evidence_exists() {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-b887-unqualified-floor-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("root");
        let store = SessionArtifactManifestStore::new(&root);
        assert!(matches!(
            store.begin_write(),
            Err(ManifestStoreError::NamespaceQualificationRequired)
        ));
        assert_eq!(
            std::fs::read_dir(root)
                .expect("unqualified root remains readable")
                .count(),
            0
        );
    }

    #[test]
    fn checked_open_invokes_supported_content_reader_once() {
        let mut manifest = v2_candidate();
        manifest.session_semantics_version = SessionSemanticsVersion::V1;
        let reader_invocations = std::cell::Cell::new(0);

        assert_eq!(
            checked_session_open(&manifest, SessionSemanticsVersion::V1, || {
                reader_invocations.set(reader_invocations.get() + 1);
                Ok::<_, ()>("canonical-or-legacy-content")
            }),
            Ok("canonical-or-legacy-content")
        );
        assert_eq!(reader_invocations.get(), 1);
    }

    #[test]
    fn v2_floor_allows_each_v2_artifact_classification() {
        for artifact in [
            SessionSemanticsArtifact::TranscriptRevisionV2,
            SessionSemanticsArtifact::ProjectionBasisV2,
            SessionSemanticsArtifact::ProjectionPatchV2,
        ] {
            assert_eq!(
                validate_artifact_semantics(SessionSemanticsVersion::V2, artifact),
                Ok(())
            );
        }
    }
}
