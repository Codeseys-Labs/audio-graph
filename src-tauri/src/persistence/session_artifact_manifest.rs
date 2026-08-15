//! Durable, typed Session Artifact manifest kernel.
//!
//! This dormant deep module owns strict manifest decoding, portable managed
//! identities, manifest invariants, and generation compare-and-swap. Callers
//! provide one explicit already-provisioned root. The write transaction owns
//! the exact canonical exclusive guard for its lifetime and delegates the
//! physical snapshot install to the canonical durability substrate.

use std::collections::HashSet;
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::de::{IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

#[cfg(test)]
use super::canonical_durability::CanonicalPlatform;
use super::canonical_durability::{
    CanonicalCoordinationError, CanonicalDurability, CanonicalDurabilityIndeterminate,
    CanonicalDurabilityOutcome, CanonicalDurabilityReceipt, CanonicalDurabilityRejection,
    CanonicalExclusiveGuard, CanonicalFilesystemQualification, CanonicalRecoveryKey,
    CanonicalSnapshotExpectation,
};

pub const SESSION_ARTIFACT_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Conservative cross-platform ceiling for the manifest-controlled relative
/// identity itself. Individual platforms still validate the resolved root plus
/// identity; V1 never persists an unbounded path-shaped value.
pub const MAX_MANAGED_ARTIFACT_IDENTITY_BYTES: usize = 1023;

const MANIFEST_FILE_NAME: &str = ".audio-graph-session-artifacts.v1.json";
const MANIFEST_TEMP_FILE_NAME: &str = ".audio-graph-session-artifacts.v1.tmp";
const COORDINATION_FILE_NAME: &str = ".audio-graph-canonical.lock";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ManagedArtifactIdentity(String);

impl ManagedArtifactIdentity {
    pub fn new(identity: impl Into<String>) -> Result<Self, ManifestValidationError> {
        let identity = Self(identity.into());
        validate_managed_identity(&identity)?;
        Ok(identity)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn ascii_case_equivalent(&self, other: &Self) -> bool {
        self.0.eq_ignore_ascii_case(&other.0)
    }

    fn internal(identity: &'static str) -> Self {
        Self(identity.to_owned())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sha256Digest(String);

impl Sha256Digest {
    pub fn new(value: impl Into<String>) -> Result<Self, ManifestValidationError> {
        let value = Self(value.into());
        validate_sha256(&value)?;
        Ok(value)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactContentIdentity {
    pub sha256: Sha256Digest,
    pub byte_length: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManagedArtifactSourceIdentity {
    pub managed_identity: ManagedArtifactIdentity,
    pub content: ArtifactContentIdentity,
}

/// Complete manifest vocabulary for current canonical, derived, operational,
/// compatibility, and recovery artifacts. This is deliberately independent
/// from the older computed descriptor enum used by runtime consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionArtifactKind {
    OriginalSessionAudio,
    SessionMetadata,
    SessionProvenanceEvents,
    TranscriptRevisions,
    SpeakerRevisions,
    ProjectionPatches,
    DataMovementEvents,
    TranscriptSnapshot,
    SpeakerTimelineSnapshot,
    ProjectionStateSnapshot,
    MaterializedNotes,
    MaterializedGraph,
    SchedulerQueue,
    UsageLedger,
    LiveAssistCurrent,
    LiveAssistAudit,
    DataMovementLedger,
    QuarantineRecovery,
    RecoveryReceipt,
    LegacyTranscript,
    LegacyGraph,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactPrivacyClass {
    OriginalEvidence,
    CanonicalSessionMemory,
    DerivedSessionMemory,
    OperationalMetadata,
    AuditRecord,
    RecoveryMaterial,
}

/// Content-free explanation for a stable artifact identity with no available
/// bytes. Free-form strings are intentionally excluded from the durable wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactUnavailableReason {
    RetentionDisabled,
    NeverCaptured,
    Expired,
    DeletedByUser,
    Inaccessible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactResidualReason {
    DeletionFailed,
    QuarantinePrepared,
    QuarantineSourceTruncated,
    DurabilityIndeterminate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum ArtifactAvailability {
    Present {
        content: ArtifactContentIdentity,
    },
    Unavailable {
        reason: ArtifactUnavailableReason,
    },
    Residual {
        content: ArtifactContentIdentity,
        reason: ArtifactResidualReason,
    },
}

impl ArtifactAvailability {
    fn content(&self) -> Option<&ArtifactContentIdentity> {
        match self {
            Self::Present { content } | Self::Residual { content, .. } => Some(content),
            Self::Unavailable { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionArtifactEntry {
    pub kind: SessionArtifactKind,
    pub privacy_class: ArtifactPrivacyClass,
    pub managed_identity: ManagedArtifactIdentity,
    pub availability: ArtifactAvailability,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestTransitionState {
    Prepared,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestTransition {
    pub idempotency_id: String,
    pub fingerprint: Sha256Digest,
    pub state: ManifestTransitionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuarantineResidualState {
    SourceFull,
    SourceTruncated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct QuarantineTransaction {
    pub idempotency_id: String,
    pub fingerprint: Sha256Digest,
    pub state: ManifestTransitionState,
    pub source_before: ManagedArtifactSourceIdentity,
    pub source_after: ManagedArtifactSourceIdentity,
    pub quarantine: ManagedArtifactSourceIdentity,
    pub residual_state: QuarantineResidualState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionArtifactManifestV1 {
    pub schema_version: u32,
    pub session_id: String,
    pub generation: u64,
    pub transition: ManifestTransition,
    pub artifacts: Vec<SessionArtifactEntry>,
    pub quarantine_transaction: Option<QuarantineTransaction>,
}

impl SessionArtifactManifestV1 {
    /// Construct an uncommitted candidate. The store assigns the next
    /// generation while its exclusive guard is held.
    pub fn candidate(
        session_id: impl Into<String>,
        transition: ManifestTransition,
        artifacts: Vec<SessionArtifactEntry>,
        quarantine_transaction: Option<QuarantineTransaction>,
    ) -> Result<Self, ManifestValidationError> {
        let mut candidate = Self {
            schema_version: SESSION_ARTIFACT_MANIFEST_SCHEMA_VERSION,
            session_id: session_id.into(),
            generation: 0,
            transition,
            artifacts,
            quarantine_transaction,
        };
        validate_candidate_and_normalize(&mut candidate)?;
        Ok(candidate)
    }

    /// Exact typed inventory for later load/export/delete parity. Unavailable
    /// identities stay in the inventory so evidence annotations remain stable.
    pub fn managed_inventory(&self) -> Vec<ManagedArtifactIdentity> {
        self.artifacts
            .iter()
            .map(|artifact| artifact.managed_identity.clone())
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestInternalIdentities {
    pub manifest: ManagedArtifactIdentity,
    pub temporary: ManagedArtifactIdentity,
    pub coordination: ManagedArtifactIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestValidationError {
    UnsupportedSchema { actual: u32 },
    InvalidGeneration { actual: u64 },
    EmptySessionId,
    InvalidSessionId,
    EmptyIdempotencyId,
    InvalidIdempotencyId,
    InvalidSha256,
    EmptyArtifactInventory,
    PreparedWithoutQuarantine,
    InvalidManagedIdentity,
    ReservedInternalIdentity,
    CaseEquivalentManagedIdentity,
    MissingOriginalSessionAudio,
    DuplicateOriginalSessionAudio,
    OriginalAudioPrivacyMismatch,
    QuarantinePrivacyMismatch,
    TransitionMismatch,
    QuarantineSourceIdentityMismatch,
    QuarantineLengthMismatch,
    QuarantineEntryMismatch,
    QuarantineResidualMismatch,
    QuarantineSourceEntryMismatch,
    CompletedResidualMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestLoadError {
    Coordination(CanonicalCoordinationError),
    NonRegularManifest,
    TooLarge {
        byte_length: u64,
    },
    ChangedDuringRead,
    Io {
        kind: io::ErrorKind,
        raw_os_error: Option<i32>,
    },
    Malformed,
    Validation(ManifestValidationError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestStoreError {
    Coordination(CanonicalCoordinationError),
    Load(ManifestLoadError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestLoadOutcome {
    Absent,
    Present(Box<SessionArtifactManifestV1>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestCasRejection {
    Validation(ManifestValidationError),
    GenerationConflict { expected: u64, actual: u64 },
    GenerationOverflow,
    SessionMismatch,
    IdempotencyConflict,
    CompletionRequiresPrepared,
    PreparedCompletionConflict,
    CompletedRegression,
    PreparedTransitionReplacement,
    TransitionConflict,
    Serialization,
    ManifestTooLarge { byte_length: u64 },
    Durability(CanonicalDurabilityRejection),
}

#[must_use = "manifest CAS outcomes must be reconciled before state advances"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestCasOutcome {
    Accepted {
        manifest: SessionArtifactManifestV1,
        durability: CanonicalDurabilityReceipt,
    },
    AlreadyCompleted {
        manifest: SessionArtifactManifestV1,
    },
    Rejected(ManifestCasRejection),
    DurabilityIndeterminate(CanonicalDurabilityIndeterminate),
}

/// One explicit-root manifest store. It never resolves or provisions a
/// default application path.
pub struct SessionArtifactManifestStore {
    root: PathBuf,
    durability: CanonicalDurability,
    qualification: Option<CanonicalFilesystemQualification>,
}

impl SessionArtifactManifestStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            durability: CanonicalDurability::new(),
            qualification: None,
        }
    }

    pub fn internal_identities(&self) -> ManifestInternalIdentities {
        ManifestInternalIdentities {
            manifest: ManagedArtifactIdentity::internal(MANIFEST_FILE_NAME),
            temporary: ManagedArtifactIdentity::internal(MANIFEST_TEMP_FILE_NAME),
            coordination: ManagedArtifactIdentity::internal(COORDINATION_FILE_NAME),
        }
    }

    /// Strict, non-mutating load. Missing root, manifest, and coordination
    /// state is `Absent`; a present manifest without its coordination entry
    /// fails closed.
    pub fn load(&self) -> Result<ManifestLoadOutcome, ManifestLoadError> {
        let manifest_path = self.manifest_path();
        let coordination_path = self.root.join(COORDINATION_FILE_NAME);
        let manifest_exists = entry_exists(&manifest_path)?;
        let coordination_exists = entry_exists(&coordination_path)?;
        if !manifest_exists && !coordination_exists {
            return Ok(ManifestLoadOutcome::Absent);
        }
        let _guard = self
            .durability
            .try_lock_shared(&self.root)
            .map_err(ManifestLoadError::Coordination)?;
        if !entry_exists(&manifest_path)? {
            return Ok(ManifestLoadOutcome::Absent);
        }
        let (manifest, _) = load_manifest_file(&manifest_path)?;
        Ok(ManifestLoadOutcome::Present(Box::new(manifest)))
    }

    /// Begin one guard-owning write transaction. The root must already exist;
    /// only the canonical coordination entry may be created by acquisition.
    pub fn begin_write(&self) -> Result<ManifestWriteTransaction<'_>, ManifestStoreError> {
        let guard = self
            .durability
            .try_lock_exclusive(&self.root)
            .map_err(ManifestStoreError::Coordination)?;
        let manifest_path = self.manifest_path();
        let (head, head_file) = if entry_exists(&manifest_path).map_err(ManifestStoreError::Load)? {
            let (head, file) =
                load_manifest_file(&manifest_path).map_err(ManifestStoreError::Load)?;
            (Some(head), Some(file))
        } else {
            (None, None)
        };
        Ok(ManifestWriteTransaction {
            guard,
            qualification: self.qualification.as_ref(),
            manifest_path,
            temporary_path: self.root.join(MANIFEST_TEMP_FILE_NAME),
            head,
            head_file,
        })
    }

    fn manifest_path(&self) -> PathBuf {
        self.root.join(MANIFEST_FILE_NAME)
    }

    pub(crate) fn managed_root(&self) -> &Path {
        &self.root
    }

    #[cfg(test)]
    pub(crate) fn qualified_for_test(root: impl Into<PathBuf>) -> Result<Self, ManifestStoreError> {
        let root = root.into();
        let qualification = CanonicalFilesystemQualification::for_test_root(&root)
            .map_err(ManifestStoreError::Coordination)?;
        Ok(Self {
            root,
            durability: CanonicalDurability::new(),
            qualification: Some(qualification),
        })
    }

    #[cfg(test)]
    pub(crate) fn qualified_for_test_platform(
        root: impl Into<PathBuf>,
        platform: CanonicalPlatform,
    ) -> Result<Self, ManifestStoreError> {
        let root = root.into();
        let qualification = CanonicalFilesystemQualification::for_test_root(&root)
            .map_err(ManifestStoreError::Coordination)?;
        Ok(Self {
            root,
            durability: CanonicalDurability::for_test_platform(platform),
            qualification: Some(qualification),
        })
    }
}

/// CAS object that owns the exact canonical exclusive guard and the exact open
/// head handle used for replacement validation.
pub struct ManifestWriteTransaction<'store> {
    guard: CanonicalExclusiveGuard,
    qualification: Option<&'store CanonicalFilesystemQualification>,
    manifest_path: PathBuf,
    temporary_path: PathBuf,
    head: Option<SessionArtifactManifestV1>,
    head_file: Option<File>,
}

impl ManifestWriteTransaction<'_> {
    /// Narrow handoff for the lock-owned recovery transaction. The guard and
    /// qualification remain borrowed from this manifest transaction; callers
    /// cannot detach either from its lifetime.
    pub(crate) fn recovery_durability(
        &self,
    ) -> (
        &CanonicalExclusiveGuard,
        Option<&CanonicalFilesystemQualification>,
    ) {
        (&self.guard, self.qualification)
    }

    pub fn head(&self) -> ManifestLoadOutcome {
        self.head
            .clone()
            .map_or(ManifestLoadOutcome::Absent, |manifest| {
                ManifestLoadOutcome::Present(Box::new(manifest))
            })
    }

    pub fn compare_and_swap(
        &mut self,
        expected_generation: u64,
        candidate: SessionArtifactManifestV1,
    ) -> ManifestCasOutcome {
        self.compare_and_swap_inner(expected_generation, candidate, false, None)
    }

    pub(crate) fn compare_and_swap_recovery(
        &mut self,
        expected_generation: u64,
        candidate: SessionArtifactManifestV1,
    ) -> ManifestCasOutcome {
        self.compare_and_swap_inner(expected_generation, candidate, true, None)
    }

    #[cfg(test)]
    pub(crate) fn compare_and_swap_recovery_with_fault(
        &mut self,
        expected_generation: u64,
        candidate: SessionArtifactManifestV1,
        injected_fault: super::canonical_durability::CanonicalDurabilityStage,
    ) -> ManifestCasOutcome {
        self.compare_and_swap_inner(expected_generation, candidate, true, Some(injected_fault))
    }

    fn compare_and_swap_inner(
        &mut self,
        expected_generation: u64,
        mut candidate: SessionArtifactManifestV1,
        resume_temporary: bool,
        _injected_fault: Option<super::canonical_durability::CanonicalDurabilityStage>,
    ) -> ManifestCasOutcome {
        if let Err(error) = validate_candidate_and_normalize(&mut candidate) {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::Validation(error));
        }

        if let Some(head) = &self.head {
            if head.session_id != candidate.session_id {
                return ManifestCasOutcome::Rejected(ManifestCasRejection::SessionMismatch);
            }
            match head.transition.state {
                ManifestTransitionState::Prepared => {
                    if head.transition.idempotency_id != candidate.transition.idempotency_id {
                        return ManifestCasOutcome::Rejected(
                            ManifestCasRejection::PreparedTransitionReplacement,
                        );
                    }
                    if head.transition.fingerprint != candidate.transition.fingerprint {
                        return ManifestCasOutcome::Rejected(
                            ManifestCasRejection::IdempotencyConflict,
                        );
                    }
                    if candidate.transition.state != ManifestTransitionState::Completed
                        || candidate.quarantine_transaction.is_none()
                        || !prepared_completion_matches(head, &candidate)
                    {
                        return ManifestCasOutcome::Rejected(
                            ManifestCasRejection::PreparedCompletionConflict,
                        );
                    }
                }
                ManifestTransitionState::Completed
                    if head.transition.idempotency_id == candidate.transition.idempotency_id =>
                {
                    if head.transition.fingerprint != candidate.transition.fingerprint {
                        return ManifestCasOutcome::Rejected(
                            ManifestCasRejection::IdempotencyConflict,
                        );
                    }
                    if candidate.transition.state == ManifestTransitionState::Prepared {
                        return ManifestCasOutcome::Rejected(
                            ManifestCasRejection::CompletedRegression,
                        );
                    }
                    candidate.generation = head.generation;
                    if candidate == *head {
                        return ManifestCasOutcome::AlreadyCompleted {
                            manifest: head.clone(),
                        };
                    }
                    return ManifestCasOutcome::Rejected(ManifestCasRejection::TransitionConflict);
                }
                ManifestTransitionState::Completed
                    if candidate.quarantine_transaction.is_some()
                        && candidate.transition.state == ManifestTransitionState::Completed =>
                {
                    return ManifestCasOutcome::Rejected(
                        ManifestCasRejection::CompletionRequiresPrepared,
                    );
                }
                ManifestTransitionState::Completed => {}
            }
        } else if candidate.quarantine_transaction.is_some()
            && candidate.transition.state == ManifestTransitionState::Completed
        {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::CompletionRequiresPrepared);
        }

        let actual_generation = self.head.as_ref().map_or(0, |head| head.generation);
        if expected_generation != actual_generation {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::GenerationConflict {
                expected: expected_generation,
                actual: actual_generation,
            });
        }
        let Some(next_generation) = expected_generation.checked_add(1) else {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::GenerationOverflow);
        };
        candidate.generation = next_generation;
        if let Err(error) = validate_persisted_and_normalize(&mut candidate) {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::Validation(error));
        }
        let bytes = match serde_json::to_vec(&candidate) {
            Ok(bytes) => bytes,
            Err(_) => {
                return ManifestCasOutcome::Rejected(ManifestCasRejection::Serialization);
            }
        };
        let byte_length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if byte_length > MAX_MANIFEST_BYTES {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::ManifestTooLarge {
                byte_length,
            });
        }
        let recovery_key = recovery_key(&candidate.transition.fingerprint);
        let expectation = self
            .head_file
            .as_ref()
            .map_or(CanonicalSnapshotExpectation::Absent, |file| {
                CanonicalSnapshotExpectation::Existing(file)
            });
        let outcome = if resume_temporary {
            #[cfg(test)]
            if let Some(injected_fault) = _injected_fault {
                self.guard.install_snapshot_recovery_with_fault(
                    &self.temporary_path,
                    &self.manifest_path,
                    &bytes,
                    expectation,
                    self.qualification,
                    recovery_key,
                    injected_fault,
                )
            } else {
                self.guard.install_snapshot_recovery(
                    &self.temporary_path,
                    &self.manifest_path,
                    &bytes,
                    expectation,
                    self.qualification,
                    recovery_key,
                )
            }
            #[cfg(not(test))]
            self.guard.install_snapshot_recovery(
                &self.temporary_path,
                &self.manifest_path,
                &bytes,
                expectation,
                self.qualification,
                recovery_key,
            )
        } else {
            self.guard.install_snapshot(
                &self.temporary_path,
                &self.manifest_path,
                &bytes,
                expectation,
                self.qualification,
                recovery_key,
            )
        };
        match outcome {
            CanonicalDurabilityOutcome::Accepted(durability) => {
                match load_manifest_file(&self.manifest_path) {
                    Ok((installed, file)) if installed == candidate => {
                        self.head = Some(installed.clone());
                        self.head_file = Some(file);
                        ManifestCasOutcome::Accepted {
                            manifest: installed,
                            durability,
                        }
                    }
                    Ok(_) | Err(_) => ManifestCasOutcome::DurabilityIndeterminate(
                        CanonicalDurabilityIndeterminate {
                            stage:
                                super::canonical_durability::CanonicalDurabilityStage::InspectEntry,
                            kind: io::ErrorKind::InvalidData,
                            raw_os_error: None,
                            recovery_key,
                        },
                    ),
                }
            }
            CanonicalDurabilityOutcome::Rejected(rejection) => {
                ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(rejection))
            }
            CanonicalDurabilityOutcome::DurabilityIndeterminate(indeterminate) => {
                ManifestCasOutcome::DurabilityIndeterminate(indeterminate)
            }
        }
    }
}

/// A quarantine completion may change only the assigned generation, the two
/// phase fields, `SourceFull` to `SourceTruncated`, the source entry from the
/// recorded before content to the recorded target content, and the matching
/// quarantine entry's residual reason. Every identity, kind, privacy class,
/// hash, length, and unrelated inventory entry is immutable.
fn prepared_completion_matches(
    prepared: &SessionArtifactManifestV1,
    completed: &SessionArtifactManifestV1,
) -> bool {
    let (Some(prepared_quarantine), Some(completed_quarantine)) = (
        prepared.quarantine_transaction.as_ref(),
        completed.quarantine_transaction.as_ref(),
    ) else {
        return false;
    };
    if prepared.transition.state != ManifestTransitionState::Prepared
        || completed.transition.state != ManifestTransitionState::Completed
        || completed_quarantine.residual_state != QuarantineResidualState::SourceTruncated
    {
        return false;
    }

    let mut normalized = completed.clone();
    normalized.generation = prepared.generation;
    normalized.transition.state = ManifestTransitionState::Prepared;
    let Some(normalized_quarantine) = normalized.quarantine_transaction.as_mut() else {
        return false;
    };
    normalized_quarantine.state = ManifestTransitionState::Prepared;
    normalized_quarantine.residual_state = prepared_quarantine.residual_state;

    let Some(prepared_source) = prepared.artifacts.iter().find(|artifact| {
        artifact.managed_identity == prepared_quarantine.source_before.managed_identity
    }) else {
        return false;
    };
    let Some(normalized_source) = normalized.artifacts.iter_mut().find(|artifact| {
        artifact.managed_identity == completed_quarantine.source_before.managed_identity
    }) else {
        return false;
    };
    normalized_source.availability = prepared_source.availability.clone();

    let Some(prepared_recovery) = prepared.artifacts.iter().find(|artifact| {
        artifact.kind == SessionArtifactKind::QuarantineRecovery
            && artifact.managed_identity == prepared_quarantine.quarantine.managed_identity
    }) else {
        return false;
    };
    let Some(normalized_recovery) = normalized.artifacts.iter_mut().find(|artifact| {
        artifact.kind == SessionArtifactKind::QuarantineRecovery
            && artifact.managed_identity == completed_quarantine.quarantine.managed_identity
    }) else {
        return false;
    };
    normalized_recovery.availability = prepared_recovery.availability.clone();

    normalized == *prepared
}

fn entry_exists(path: &Path) -> Result<bool, ManifestLoadError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(load_io(&error)),
    }
}

fn load_manifest_file(path: &Path) -> Result<(SessionArtifactManifestV1, File), ManifestLoadError> {
    load_manifest_file_with_after_open(path, || {})
}

fn load_manifest_file_with_after_open(
    path: &Path,
    after_open: impl FnOnce(),
) -> Result<(SessionArtifactManifestV1, File), ManifestLoadError> {
    let path_metadata = std::fs::symlink_metadata(path).map_err(|error| load_io(&error))?;
    if !path_metadata.file_type().is_file() {
        return Err(ManifestLoadError::NonRegularManifest);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| load_io(&error))?;
    let opened_metadata = file.metadata().map_err(|error| load_io(&error))?;
    if !opened_metadata.file_type().is_file() {
        return Err(ManifestLoadError::NonRegularManifest);
    }
    if opened_metadata.len() > MAX_MANIFEST_BYTES {
        return Err(ManifestLoadError::TooLarge {
            byte_length: opened_metadata.len(),
        });
    }

    after_open();
    let capacity = usize::try_from(opened_metadata.len()).unwrap_or(0);
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| load_io(&error))?;
    let observed_byte_length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if observed_byte_length > MAX_MANIFEST_BYTES {
        return Err(ManifestLoadError::TooLarge {
            byte_length: observed_byte_length,
        });
    }
    let final_metadata = file.metadata().map_err(|error| load_io(&error))?;
    if !final_metadata.file_type().is_file()
        || final_metadata.len() != opened_metadata.len()
        || final_metadata.len() != observed_byte_length
    {
        return Err(ManifestLoadError::ChangedDuringRead);
    }
    let schema_version = probe_schema_version(&bytes)?;
    if schema_version != SESSION_ARTIFACT_MANIFEST_SCHEMA_VERSION {
        return Err(ManifestLoadError::Validation(
            ManifestValidationError::UnsupportedSchema {
                actual: schema_version,
            },
        ));
    }
    let mut manifest: SessionArtifactManifestV1 =
        serde_json::from_slice(&bytes).map_err(|_| ManifestLoadError::Malformed)?;
    validate_persisted_and_normalize(&mut manifest).map_err(ManifestLoadError::Validation)?;
    Ok((manifest, file))
}

fn load_io(error: &io::Error) -> ManifestLoadError {
    ManifestLoadError::Io {
        kind: error.kind(),
        raw_os_error: error.raw_os_error(),
    }
}

fn probe_schema_version(bytes: &[u8]) -> Result<u32, ManifestLoadError> {
    struct SchemaProbe;

    impl<'de> Visitor<'de> for SchemaProbe {
        type Value = u32;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a Session Artifact manifest object")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            let mut seen = HashSet::new();
            let mut schema_version = None;
            while let Some(key) = map.next_key::<String>()? {
                if !seen.insert(key.clone()) {
                    return Err(serde::de::Error::custom("duplicate manifest member"));
                }
                if key == "schema_version" {
                    schema_version = Some(map.next_value::<u32>()?);
                } else {
                    map.next_value::<IgnoredAny>()?;
                }
            }
            schema_version.ok_or_else(|| serde::de::Error::missing_field("schema_version"))
        }
    }

    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let schema_version = deserializer
        .deserialize_map(SchemaProbe)
        .map_err(|_| ManifestLoadError::Malformed)?;
    deserializer
        .end()
        .map_err(|_| ManifestLoadError::Malformed)?;
    Ok(schema_version)
}

fn validate_candidate_and_normalize(
    manifest: &mut SessionArtifactManifestV1,
) -> Result<(), ManifestValidationError> {
    validate_and_normalize(manifest, ManifestValidationContext::Candidate)
}

fn validate_persisted_and_normalize(
    manifest: &mut SessionArtifactManifestV1,
) -> Result<(), ManifestValidationError> {
    validate_and_normalize(manifest, ManifestValidationContext::Persisted)
}

#[derive(Clone, Copy)]
enum ManifestValidationContext {
    Candidate,
    Persisted,
}

fn validate_and_normalize(
    manifest: &mut SessionArtifactManifestV1,
    context: ManifestValidationContext,
) -> Result<(), ManifestValidationError> {
    if manifest.schema_version != SESSION_ARTIFACT_MANIFEST_SCHEMA_VERSION {
        return Err(ManifestValidationError::UnsupportedSchema {
            actual: manifest.schema_version,
        });
    }
    if matches!(context, ManifestValidationContext::Persisted) && manifest.generation == 0 {
        return Err(ManifestValidationError::InvalidGeneration {
            actual: manifest.generation,
        });
    }
    validate_session_id(&manifest.session_id)?;
    validate_idempotency_id(&manifest.transition.idempotency_id)?;
    validate_sha256(&manifest.transition.fingerprint)?;
    if manifest.artifacts.is_empty() {
        return Err(ManifestValidationError::EmptyArtifactInventory);
    }
    if manifest.transition.state == ManifestTransitionState::Prepared
        && manifest.quarantine_transaction.is_none()
    {
        return Err(ManifestValidationError::PreparedWithoutQuarantine);
    }

    let mut identities = HashSet::new();
    let mut original_audio_count = 0;
    for artifact in &manifest.artifacts {
        validate_managed_identity(&artifact.managed_identity)?;
        let folded = artifact.managed_identity.as_str().to_ascii_lowercase();
        if !identities.insert(folded) {
            return Err(ManifestValidationError::CaseEquivalentManagedIdentity);
        }
        if let Some(content) = artifact.availability.content() {
            validate_sha256(&content.sha256)?;
        }
        if artifact.kind == SessionArtifactKind::OriginalSessionAudio {
            original_audio_count += 1;
            if artifact.privacy_class != ArtifactPrivacyClass::OriginalEvidence {
                return Err(ManifestValidationError::OriginalAudioPrivacyMismatch);
            }
        }
        if artifact.kind == SessionArtifactKind::QuarantineRecovery
            && artifact.privacy_class != ArtifactPrivacyClass::RecoveryMaterial
        {
            return Err(ManifestValidationError::QuarantinePrivacyMismatch);
        }
    }
    match original_audio_count {
        0 => return Err(ManifestValidationError::MissingOriginalSessionAudio),
        1 => {}
        _ => return Err(ManifestValidationError::DuplicateOriginalSessionAudio),
    }

    if let Some(transaction) = &manifest.quarantine_transaction {
        validate_quarantine_transaction(manifest, transaction)?;
    }
    manifest.artifacts.sort_by(|left, right| {
        left.managed_identity
            .cmp(&right.managed_identity)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    Ok(())
}

fn validate_quarantine_transaction(
    manifest: &SessionArtifactManifestV1,
    transaction: &QuarantineTransaction,
) -> Result<(), ManifestValidationError> {
    validate_idempotency_id(&transaction.idempotency_id)?;
    validate_sha256(&transaction.fingerprint)?;
    if transaction.idempotency_id != manifest.transition.idempotency_id
        || transaction.fingerprint != manifest.transition.fingerprint
        || transaction.state != manifest.transition.state
    {
        return Err(ManifestValidationError::TransitionMismatch);
    }
    for source in [
        &transaction.source_before,
        &transaction.source_after,
        &transaction.quarantine,
    ] {
        validate_managed_identity(&source.managed_identity)?;
        validate_sha256(&source.content.sha256)?;
    }
    if transaction.source_before.managed_identity != transaction.source_after.managed_identity
        || transaction.source_before.managed_identity == transaction.quarantine.managed_identity
    {
        return Err(ManifestValidationError::QuarantineSourceIdentityMismatch);
    }
    let removed = transaction
        .source_before
        .content
        .byte_length
        .checked_sub(transaction.source_after.content.byte_length)
        .ok_or(ManifestValidationError::QuarantineLengthMismatch)?;
    if removed == 0 || removed != transaction.quarantine.content.byte_length {
        return Err(ManifestValidationError::QuarantineLengthMismatch);
    }
    if transaction.state == ManifestTransitionState::Completed
        && transaction.residual_state != QuarantineResidualState::SourceTruncated
    {
        return Err(ManifestValidationError::CompletedResidualMismatch);
    }

    let expected_quarantine_reason = match transaction.residual_state {
        QuarantineResidualState::SourceFull => ArtifactResidualReason::QuarantinePrepared,
        QuarantineResidualState::SourceTruncated => {
            ArtifactResidualReason::QuarantineSourceTruncated
        }
    };
    let quarantine_matches = manifest
        .artifacts
        .iter()
        .filter(|artifact| {
            artifact.kind == SessionArtifactKind::QuarantineRecovery
                && artifact.managed_identity == transaction.quarantine.managed_identity
                && artifact.availability
                    == ArtifactAvailability::Residual {
                        content: transaction.quarantine.content.clone(),
                        reason: expected_quarantine_reason,
                    }
        })
        .count();
    if quarantine_matches != 1 {
        let identity_matches = manifest.artifacts.iter().any(|artifact| {
            artifact.kind == SessionArtifactKind::QuarantineRecovery
                && artifact.managed_identity == transaction.quarantine.managed_identity
        });
        return Err(if identity_matches {
            ManifestValidationError::QuarantineResidualMismatch
        } else {
            ManifestValidationError::QuarantineEntryMismatch
        });
    }
    let expected_source = match transaction.residual_state {
        QuarantineResidualState::SourceFull => &transaction.source_before.content,
        QuarantineResidualState::SourceTruncated => &transaction.source_after.content,
    };
    let source_matches = manifest
        .artifacts
        .iter()
        .filter(|artifact| {
            artifact.managed_identity == transaction.source_before.managed_identity
                && artifact.availability.content() == Some(expected_source)
        })
        .count();
    if source_matches != 1 {
        return Err(ManifestValidationError::QuarantineSourceEntryMismatch);
    }
    Ok(())
}

fn validate_session_id(session_id: &str) -> Result<(), ManifestValidationError> {
    if session_id.is_empty() {
        return Err(ManifestValidationError::EmptySessionId);
    }
    if session_id.len() > 255
        || session_id == "."
        || session_id == ".."
        || session_id
            .chars()
            .any(|character| matches!(character, '/' | '\\' | '\0') || character.is_control())
    {
        return Err(ManifestValidationError::InvalidSessionId);
    }
    Ok(())
}

fn validate_idempotency_id(id: &str) -> Result<(), ManifestValidationError> {
    if id.is_empty() {
        return Err(ManifestValidationError::EmptyIdempotencyId);
    }
    if id.len() > 255
        || id
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | '\0'))
    {
        return Err(ManifestValidationError::InvalidIdempotencyId);
    }
    Ok(())
}

fn validate_sha256(digest: &Sha256Digest) -> Result<(), ManifestValidationError> {
    let Some(hex) = digest.as_str().strip_prefix("sha256:") else {
        return Err(ManifestValidationError::InvalidSha256);
    };
    if hex.len() != 64
        || !hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ManifestValidationError::InvalidSha256);
    }
    Ok(())
}

fn validate_managed_identity(
    identity: &ManagedArtifactIdentity,
) -> Result<(), ManifestValidationError> {
    let value = identity.as_str();
    if value.is_empty()
        || value.len() > MAX_MANAGED_ARTIFACT_IDENTITY_BYTES
        || value.starts_with('/')
        || value.ends_with('/')
        || value.contains('\\')
        || value.contains('\0')
        || value.chars().any(is_portable_control)
    {
        return Err(ManifestValidationError::InvalidManagedIdentity);
    }
    let invalid = value.split('/').any(|segment| {
        segment.is_empty()
            || segment.len() > 255
            || matches!(segment, "." | "..")
            || segment.ends_with(['.', ' '])
            || segment
                .chars()
                .any(|character| matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
            || is_windows_reserved_segment(segment)
    });
    if invalid {
        return Err(ManifestValidationError::InvalidManagedIdentity);
    }
    if is_internal_identity(value) {
        return Err(ManifestValidationError::ReservedInternalIdentity);
    }
    Ok(())
}

fn is_portable_control(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{00ad}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061c}'
                | '\u{06dd}'
                | '\u{070f}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08e2}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}'
                | '\u{110cd}'
                | '\u{13430}'..='\u{1343f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
        )
}

fn is_windows_reserved_segment(segment: &str) -> bool {
    let basename = segment.split('.').next().unwrap_or(segment);
    matches!(
        basename.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn is_internal_identity(identity: &str) -> bool {
    [
        MANIFEST_FILE_NAME,
        MANIFEST_TEMP_FILE_NAME,
        COORDINATION_FILE_NAME,
    ]
    .iter()
    .any(|internal| identity.eq_ignore_ascii_case(internal))
}

fn recovery_key(fingerprint: &Sha256Digest) -> CanonicalRecoveryKey {
    let digest = Sha256::digest(fingerprint.as_str().as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    CanonicalRecoveryKey::from_opaque_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-a596-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).expect("create fixture root");
        root
    }

    fn missing_root(name: &str) -> PathBuf {
        let root = root(name);
        std::fs::remove_dir(&root).expect("remove fixture root");
        root
    }

    fn digest(byte: char) -> Sha256Digest {
        Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).expect("digest")
    }

    fn content(byte: char, byte_length: u64) -> ArtifactContentIdentity {
        ArtifactContentIdentity {
            sha256: digest(byte),
            byte_length,
        }
    }

    fn identity(value: &str) -> ManagedArtifactIdentity {
        ManagedArtifactIdentity::new(value).expect("managed identity")
    }

    fn portable_identity_string(byte_length: usize) -> String {
        let component_count = (byte_length + 256) / 256;
        let mut remaining_characters = byte_length - component_count.saturating_sub(1);
        let mut components = Vec::with_capacity(component_count);
        for remaining_components in (1..=component_count).rev() {
            let component_length = (remaining_characters - (remaining_components - 1)).min(255);
            components.push("a".repeat(component_length));
            remaining_characters -= component_length;
        }
        assert_eq!(remaining_characters, 0);
        components.join("/")
    }

    fn transition(
        idempotency_id: &str,
        fingerprint_byte: char,
        state: ManifestTransitionState,
    ) -> ManifestTransition {
        ManifestTransition {
            idempotency_id: idempotency_id.to_owned(),
            fingerprint: digest(fingerprint_byte),
            state,
        }
    }

    fn original_audio(availability: ArtifactAvailability) -> SessionArtifactEntry {
        SessionArtifactEntry {
            kind: SessionArtifactKind::OriginalSessionAudio,
            privacy_class: ArtifactPrivacyClass::OriginalEvidence,
            managed_identity: identity("audio/original.wav"),
            availability,
        }
    }

    fn basic_candidate(id: &str, fingerprint: char) -> SessionArtifactManifestV1 {
        SessionArtifactManifestV1::candidate(
            "session-1",
            transition(id, fingerprint, ManifestTransitionState::Completed),
            vec![original_audio(ArtifactAvailability::Unavailable {
                reason: ArtifactUnavailableReason::RetentionDisabled,
            })],
            None,
        )
        .expect("candidate")
    }

    fn quarantine_candidate(
        state: ManifestTransitionState,
        residual_state: QuarantineResidualState,
    ) -> SessionArtifactManifestV1 {
        let source_before = ManagedArtifactSourceIdentity {
            managed_identity: identity("streams/transcript.jsonl"),
            content: content('b', 100),
        };
        let source_after = ManagedArtifactSourceIdentity {
            managed_identity: source_before.managed_identity.clone(),
            content: content('c', 60),
        };
        let quarantine = ManagedArtifactSourceIdentity {
            managed_identity: identity("recovery/transcript-tail.bin"),
            content: content('d', 40),
        };
        let visible_source = match residual_state {
            QuarantineResidualState::SourceFull => source_before.content.clone(),
            QuarantineResidualState::SourceTruncated => source_after.content.clone(),
        };
        SessionArtifactManifestV1::candidate(
            "session-1",
            transition("recover-1", 'e', state),
            vec![
                original_audio(ArtifactAvailability::Unavailable {
                    reason: ArtifactUnavailableReason::NeverCaptured,
                }),
                SessionArtifactEntry {
                    kind: SessionArtifactKind::TranscriptRevisions,
                    privacy_class: ArtifactPrivacyClass::CanonicalSessionMemory,
                    managed_identity: source_before.managed_identity.clone(),
                    availability: ArtifactAvailability::Present {
                        content: visible_source,
                    },
                },
                SessionArtifactEntry {
                    kind: SessionArtifactKind::QuarantineRecovery,
                    privacy_class: ArtifactPrivacyClass::RecoveryMaterial,
                    managed_identity: quarantine.managed_identity.clone(),
                    availability: ArtifactAvailability::Residual {
                        content: quarantine.content.clone(),
                        reason: match residual_state {
                            QuarantineResidualState::SourceFull => {
                                ArtifactResidualReason::QuarantinePrepared
                            }
                            QuarantineResidualState::SourceTruncated => {
                                ArtifactResidualReason::QuarantineSourceTruncated
                            }
                        },
                    },
                },
            ],
            Some(QuarantineTransaction {
                idempotency_id: "recover-1".to_owned(),
                fingerprint: digest('e'),
                state,
                source_before,
                source_after,
                quarantine,
                residual_state,
            }),
        )
        .expect("quarantine candidate")
    }

    fn create_coordination_entry(store: &SessionArtifactManifestStore) {
        drop(store.begin_write().expect("create coordination entry"));
    }

    fn write_manifest_bytes(store: &SessionArtifactManifestStore, bytes: &[u8]) {
        create_coordination_entry(store);
        std::fs::write(store.manifest_path(), bytes).expect("write manifest fixture");
    }

    #[test]
    fn public_manifest_store_seam_loads_an_absent_explicit_root_without_mutation() {
        let root = missing_root("missing-root");
        let store = SessionArtifactManifestStore::new(&root);

        assert_eq!(store.load(), Ok(ManifestLoadOutcome::Absent));
        assert!(!root.exists());
    }

    #[test]
    fn present_manifest_without_coordination_entry_fails_closed() {
        let root = root("missing-coordination");
        let store = SessionArtifactManifestStore::new(&root);
        let mut manifest = basic_candidate("tx-1", 'a');
        manifest.generation = 1;
        std::fs::write(
            store.manifest_path(),
            serde_json::to_vec(&manifest).expect("serialize fixture"),
        )
        .expect("write head");

        assert_eq!(
            store.load(),
            Err(ManifestLoadError::Coordination(
                CanonicalCoordinationError::Missing
            ))
        );
    }

    #[test]
    fn strict_load_rejects_malformed_duplicates_wrong_schema_and_unknown_wire_data() {
        let valid = basic_candidate("tx-1", 'a');
        let valid_value = serde_json::to_value(&valid).expect("value");
        let mut cases: Vec<(String, ManifestLoadError)> = vec![
            ("{".to_owned(), ManifestLoadError::Malformed),
            (
                r#"{"schema_version":1,"schema_version":1}"#.to_owned(),
                ManifestLoadError::Malformed,
            ),
            (
                r#"{"schema_version":2,"not_a_v1_manifest":true}"#.to_owned(),
                ManifestLoadError::Validation(ManifestValidationError::UnsupportedSchema {
                    actual: 2,
                }),
            ),
        ];
        let mut unknown_top = valid_value.clone();
        unknown_top["unknown"] = serde_json::json!(true);
        cases.push((
            serde_json::to_string(&unknown_top).expect("unknown top"),
            ManifestLoadError::Malformed,
        ));
        let mut unknown_nested = valid_value.clone();
        unknown_nested["artifacts"][0]["unknown"] = serde_json::json!(true);
        cases.push((
            serde_json::to_string(&unknown_nested).expect("unknown nested"),
            ManifestLoadError::Malformed,
        ));
        let mut unknown_enum = valid_value;
        unknown_enum["artifacts"][0]["availability"] =
            serde_json::json!({"unavailable":{"reason":"future_reason"}});
        cases.push((
            serde_json::to_string(&unknown_enum).expect("unknown enum"),
            ManifestLoadError::Malformed,
        ));
        let duplicate_nested = serde_json::to_string(&valid).expect("valid wire").replacen(
            "\"kind\":\"original_session_audio\"",
            "\"kind\":\"original_session_audio\",\"kind\":\"original_session_audio\"",
            1,
        );
        cases.push((duplicate_nested, ManifestLoadError::Malformed));

        for (index, (wire, expected)) in cases.into_iter().enumerate() {
            let root = root(&format!("strict-{index}"));
            let store = SessionArtifactManifestStore::new(&root);
            write_manifest_bytes(&store, wire.as_bytes());
            assert_eq!(store.load(), Err(expected), "wire case {index}");
        }
    }

    #[test]
    fn strict_load_revalidates_the_open_handle_after_a_bounded_read() {
        let root = root("strict-open-revalidation");
        let store = SessionArtifactManifestStore::new(&root);
        let mut manifest = basic_candidate("tx-open", 'a');
        manifest.generation = 1;
        write_manifest_bytes(
            &store,
            &serde_json::to_vec(&manifest).expect("manifest bytes"),
        );
        let manifest_path = store.manifest_path();

        assert!(matches!(
            load_manifest_file_with_after_open(&manifest_path, || {
                let mut writer = OpenOptions::new()
                    .append(true)
                    .open(&manifest_path)
                    .expect("open competing writer");
                std::io::Write::write_all(&mut writer, b" ").expect("append after open");
            }),
            Err(ManifestLoadError::ChangedDuringRead)
        ));
    }

    #[test]
    fn persisted_manifest_generation_zero_is_rejected_while_initial_cas_uses_coordinate_zero() {
        let root = root("persisted-generation-zero");
        let store = SessionArtifactManifestStore::new(&root);
        let manifest = basic_candidate("tx-zero", 'a');
        assert_eq!(
            manifest.generation, 0,
            "candidate coordinate is uncommitted"
        );
        write_manifest_bytes(
            &store,
            &serde_json::to_vec(&manifest).expect("zero-generation wire"),
        );

        assert_eq!(
            store.load(),
            Err(ManifestLoadError::Validation(
                ManifestValidationError::InvalidGeneration { actual: 0 }
            ))
        );
    }

    #[test]
    fn managed_identities_enforce_the_portable_floor_and_case_equivalence() {
        for invalid in [
            "",
            "/absolute",
            "C:/windows-prefix",
            "a\\b",
            "a//b",
            "a/./b",
            "a/../b",
            "a/trailing. ",
            "NUL.txt",
            "a\0b",
            ".AUDIO-GRAPH-CANONICAL.LOCK",
            ".audio-graph-session-artifacts.v1.json",
        ] {
            assert!(
                ManagedArtifactIdentity::new(invalid).is_err(),
                "identity must be rejected: {invalid:?}"
            );
        }
        for invalid_control in [
            "line\nbreak",
            "tab\tseparated",
            "delete\u{007f}control",
            "unicode\u{0085}control",
            "bidi\u{202e}control",
        ] {
            assert!(
                ManagedArtifactIdentity::new(invalid_control).is_err(),
                "control must be rejected: {invalid_control:?}"
            );
        }
        let overlong_component = format!("root/{}", "a".repeat(256));
        assert_eq!(
            ManagedArtifactIdentity::new(overlong_component),
            Err(ManifestValidationError::InvalidManagedIdentity)
        );
        assert_eq!(MAX_MANAGED_ARTIFACT_IDENTITY_BYTES, 1023);
        assert!(
            ManagedArtifactIdentity::new(portable_identity_string(
                MAX_MANAGED_ARTIFACT_IDENTITY_BYTES
            ))
            .is_ok()
        );
        assert_eq!(
            ManagedArtifactIdentity::new(portable_identity_string(
                MAX_MANAGED_ARTIFACT_IDENTITY_BYTES + 1
            )),
            Err(ManifestValidationError::InvalidManagedIdentity)
        );

        let duplicate = SessionArtifactManifestV1::candidate(
            "session-1",
            transition("tx-1", 'a', ManifestTransitionState::Completed),
            vec![
                original_audio(ArtifactAvailability::Unavailable {
                    reason: ArtifactUnavailableReason::NeverCaptured,
                }),
                SessionArtifactEntry {
                    kind: SessionArtifactKind::MaterializedNotes,
                    privacy_class: ArtifactPrivacyClass::DerivedSessionMemory,
                    managed_identity: identity("AUDIO/ORIGINAL.WAV"),
                    availability: ArtifactAvailability::Present {
                        content: content('b', 4),
                    },
                },
            ],
            None,
        );
        assert_eq!(
            duplicate,
            Err(ManifestValidationError::CaseEquivalentManagedIdentity)
        );
    }

    #[test]
    fn v1_wire_is_a_deterministic_golden_roundtrip() {
        let mut candidate = basic_candidate("tx-1", 'a');
        candidate.generation = 1;
        let wire = serde_json::to_string(&candidate).expect("serialize");
        assert_eq!(
            wire,
            format!(
                "{{\"schema_version\":1,\"session_id\":\"session-1\",\"generation\":1,\"transition\":{{\"idempotency_id\":\"tx-1\",\"fingerprint\":\"sha256:{}\",\"state\":\"completed\"}},\"artifacts\":[{{\"kind\":\"original_session_audio\",\"privacy_class\":\"original_evidence\",\"managed_identity\":\"audio/original.wav\",\"availability\":{{\"unavailable\":{{\"reason\":\"retention_disabled\"}}}}}}],\"quarantine_transaction\":null}}",
                "a".repeat(64)
            )
        );
        let mut decoded: SessionArtifactManifestV1 =
            serde_json::from_str(&wire).expect("decode golden");
        validate_persisted_and_normalize(&mut decoded).expect("validate golden");
        assert_eq!(decoded, candidate);
    }

    #[test]
    fn artifact_kind_vocabulary_has_distinct_stable_wire_names() {
        let kinds = [
            SessionArtifactKind::OriginalSessionAudio,
            SessionArtifactKind::SessionMetadata,
            SessionArtifactKind::SessionProvenanceEvents,
            SessionArtifactKind::TranscriptRevisions,
            SessionArtifactKind::SpeakerRevisions,
            SessionArtifactKind::ProjectionPatches,
            SessionArtifactKind::DataMovementEvents,
            SessionArtifactKind::TranscriptSnapshot,
            SessionArtifactKind::SpeakerTimelineSnapshot,
            SessionArtifactKind::ProjectionStateSnapshot,
            SessionArtifactKind::MaterializedNotes,
            SessionArtifactKind::MaterializedGraph,
            SessionArtifactKind::SchedulerQueue,
            SessionArtifactKind::UsageLedger,
            SessionArtifactKind::LiveAssistCurrent,
            SessionArtifactKind::LiveAssistAudit,
            SessionArtifactKind::DataMovementLedger,
            SessionArtifactKind::QuarantineRecovery,
            SessionArtifactKind::RecoveryReceipt,
            SessionArtifactKind::LegacyTranscript,
            SessionArtifactKind::LegacyGraph,
        ];
        let names: HashSet<String> = kinds
            .into_iter()
            .map(|kind| serde_json::to_string(&kind).expect("kind wire"))
            .collect();
        assert_eq!(names.len(), 21);
        for required in [
            "\"original_session_audio\"",
            "\"transcript_revisions\"",
            "\"speaker_revisions\"",
            "\"projection_patches\"",
            "\"data_movement_events\"",
            "\"scheduler_queue\"",
            "\"usage_ledger\"",
            "\"live_assist_audit\"",
            "\"quarantine_recovery\"",
        ] {
            assert!(names.contains(required), "missing artifact kind {required}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn initial_and_replacement_cas_assign_exact_generations() {
        let root = root("initial-replace");
        let store = SessionArtifactManifestStore::qualified_for_test(&root).expect("qualified");
        let mut transaction = store.begin_write().expect("transaction");

        let first = transaction.compare_and_swap(0, basic_candidate("tx-1", 'a'));
        assert!(matches!(
            first,
            ManifestCasOutcome::Accepted {
                manifest: SessionArtifactManifestV1 { generation: 1, .. },
                ..
            }
        ));
        let second = transaction.compare_and_swap(1, basic_candidate("tx-2", 'b'));
        assert!(matches!(
            second,
            ManifestCasOutcome::Accepted {
                manifest: SessionArtifactManifestV1 { generation: 2, .. },
                ..
            }
        ));
        drop(transaction);
        assert!(matches!(
            store.load().expect("load"),
            ManifestLoadOutcome::Present(manifest) if manifest.generation == 2
        ));
    }

    #[test]
    fn stale_and_overflow_generations_refuse_without_snapshot_mutation() {
        let root = root("generation-refusal");
        let store = SessionArtifactManifestStore::new(&root);
        let mut head = basic_candidate("tx-head", 'a');
        head.generation = u64::MAX;
        write_manifest_bytes(&store, &serde_json::to_vec(&head).expect("serialize head"));
        let mut transaction = store.begin_write().expect("transaction");

        assert_eq!(
            transaction.compare_and_swap(1, basic_candidate("tx-new", 'b')),
            ManifestCasOutcome::Rejected(ManifestCasRejection::GenerationConflict {
                expected: 1,
                actual: u64::MAX,
            })
        );
        assert_eq!(
            transaction.compare_and_swap(u64::MAX, basic_candidate("tx-new", 'b')),
            ManifestCasOutcome::Rejected(ManifestCasRejection::GenerationOverflow)
        );
        assert!(!store.root.join(MANIFEST_TEMP_FILE_NAME).exists());
        assert_eq!(
            std::fs::read(store.manifest_path()).expect("head"),
            serde_json::to_vec(&head).expect("head bytes")
        );
    }

    #[test]
    fn unqualified_namespace_never_reports_manifest_acceptance() {
        let root = root("unsupported");
        let store = SessionArtifactManifestStore::new(&root);
        let mut transaction = store.begin_write().expect("transaction");
        let outcome = transaction.compare_and_swap(0, basic_candidate("tx-1", 'a'));

        assert!(matches!(
            outcome,
            ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported { .. }
            ))
        ));
        assert!(!store.manifest_path().exists());
        assert!(!store.root.join(MANIFEST_TEMP_FILE_NAME).exists());
    }

    #[test]
    fn manifest_candidate_size_preflight_accepts_exact_boundary_and_rejects_oversize() {
        const MIN_UNIQUE_IDENTITY_BYTES: usize = 10;

        fn unique_bounded_identity(index: usize, byte_length: usize) -> ManagedArtifactIdentity {
            assert!(
                (MIN_UNIQUE_IDENTITY_BYTES..=MAX_MANAGED_ARTIFACT_IDENTITY_BYTES)
                    .contains(&byte_length)
            );
            let prefix = format!("{index:08x}/");
            let value = format!(
                "{prefix}{}",
                portable_identity_string(byte_length - prefix.len())
            );
            assert_eq!(value.len(), byte_length);
            ManagedArtifactIdentity::new(value).expect("bounded unique identity")
        }

        fn bulk_entry(index: usize, identity_length: usize) -> SessionArtifactEntry {
            SessionArtifactEntry {
                kind: SessionArtifactKind::MaterializedNotes,
                privacy_class: ArtifactPrivacyClass::DerivedSessionMemory,
                managed_identity: unique_bounded_identity(index, identity_length),
                availability: ArtifactAvailability::Present {
                    content: content('f', 1),
                },
            }
        }

        fn candidate_with_wire_size(byte_length: usize) -> SessionArtifactManifestV1 {
            let mut candidate = basic_candidate("tx-size", 'a');
            let baseline = serde_json::to_vec(&candidate).expect("baseline").len();
            let sample = bulk_entry(0, MIN_UNIQUE_IDENTITY_BYTES);
            let entry_fixed_bytes = serde_json::to_vec(&sample).expect("sample entry").len()
                - MIN_UNIQUE_IDENTITY_BYTES
                + 1;
            let additional_bytes = byte_length - baseline;
            let entry_count =
                additional_bytes.div_ceil(entry_fixed_bytes + MAX_MANAGED_ARTIFACT_IDENTITY_BYTES);
            let mut remaining_identity_bytes = additional_bytes - entry_count * entry_fixed_bytes;
            assert!(remaining_identity_bytes >= entry_count * MIN_UNIQUE_IDENTITY_BYTES);
            assert!(remaining_identity_bytes <= entry_count * MAX_MANAGED_ARTIFACT_IDENTITY_BYTES);

            for index in 0..entry_count {
                let remaining_entries = entry_count - index - 1;
                let minimum_for_remaining = remaining_entries * MIN_UNIQUE_IDENTITY_BYTES;
                let identity_length = (remaining_identity_bytes - minimum_for_remaining)
                    .min(MAX_MANAGED_ARTIFACT_IDENTITY_BYTES);
                candidate.artifacts.push(bulk_entry(index, identity_length));
                remaining_identity_bytes -= identity_length;
            }
            assert_eq!(remaining_identity_bytes, 0);
            assert_eq!(
                serde_json::to_vec(&candidate)
                    .expect("sized candidate")
                    .len(),
                byte_length
            );
            candidate
        }

        let exact_root = root("size-exact");
        let exact_store = SessionArtifactManifestStore::new(&exact_root);
        let mut exact_transaction = exact_store.begin_write().expect("exact transaction");
        assert!(matches!(
            exact_transaction
                .compare_and_swap(0, candidate_with_wire_size(MAX_MANIFEST_BYTES as usize)),
            ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported { .. }
            ))
        ));
        assert!(!exact_store.manifest_path().exists());
        assert!(!exact_store.root.join(MANIFEST_TEMP_FILE_NAME).exists());

        let oversized_root = root("size-oversized");
        let oversized_store = SessionArtifactManifestStore::new(&oversized_root);
        let mut oversized_transaction = oversized_store
            .begin_write()
            .expect("oversized transaction");
        assert_eq!(
            oversized_transaction
                .compare_and_swap(0, candidate_with_wire_size(MAX_MANIFEST_BYTES as usize + 1)),
            ManifestCasOutcome::Rejected(ManifestCasRejection::ManifestTooLarge {
                byte_length: MAX_MANIFEST_BYTES + 1,
            })
        );
        assert!(!oversized_store.manifest_path().exists());
        assert!(!oversized_store.root.join(MANIFEST_TEMP_FILE_NAME).exists());
    }

    #[cfg(unix)]
    #[test]
    fn prepared_completion_retry_conflict_and_restart_are_idempotent() {
        let root = root("prepare-complete");
        let store = SessionArtifactManifestStore::qualified_for_test(&root).expect("qualified");
        {
            let mut transaction = store.begin_write().expect("transaction");
            assert!(matches!(
                transaction.compare_and_swap(
                    0,
                    quarantine_candidate(
                        ManifestTransitionState::Prepared,
                        QuarantineResidualState::SourceFull,
                    )
                ),
                ManifestCasOutcome::Accepted {
                    manifest: SessionArtifactManifestV1 { generation: 1, .. },
                    ..
                }
            ));
            assert!(matches!(
                transaction.compare_and_swap(
                    1,
                    quarantine_candidate(
                        ManifestTransitionState::Completed,
                        QuarantineResidualState::SourceTruncated,
                    )
                ),
                ManifestCasOutcome::Accepted {
                    manifest: SessionArtifactManifestV1 { generation: 2, .. },
                    ..
                }
            ));
        }

        assert!(matches!(
            store.load().expect("restart load"),
            ManifestLoadOutcome::Present(manifest) if manifest.generation == 2
        ));
        let mut restarted = store.begin_write().expect("restart transaction");
        assert!(matches!(
            restarted.compare_and_swap(
                1,
                quarantine_candidate(
                    ManifestTransitionState::Completed,
                    QuarantineResidualState::SourceTruncated,
                )
            ),
            ManifestCasOutcome::AlreadyCompleted {
                manifest: SessionArtifactManifestV1 { generation: 2, .. }
            }
        ));
        let mut conflicting = quarantine_candidate(
            ManifestTransitionState::Completed,
            QuarantineResidualState::SourceTruncated,
        );
        conflicting.transition.fingerprint = digest('f');
        if let Some(quarantine) = &mut conflicting.quarantine_transaction {
            quarantine.fingerprint = digest('f');
        }
        assert_eq!(
            restarted.compare_and_swap(2, conflicting),
            ManifestCasOutcome::Rejected(ManifestCasRejection::IdempotencyConflict)
        );
        assert!(matches!(
            restarted.compare_and_swap(
                2,
                quarantine_candidate(
                    ManifestTransitionState::Prepared,
                    QuarantineResidualState::SourceFull,
                )
            ),
            ManifestCasOutcome::Rejected(ManifestCasRejection::CompletedRegression)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn completed_quarantine_requires_the_exact_prepared_immutable_transaction() {
        let direct_root = root("direct-completed");
        let store =
            SessionArtifactManifestStore::qualified_for_test(&direct_root).expect("qualified");
        let mut transaction = store.begin_write().expect("transaction");
        assert!(matches!(
            transaction.compare_and_swap(
                0,
                quarantine_candidate(
                    ManifestTransitionState::Completed,
                    QuarantineResidualState::SourceTruncated,
                )
            ),
            ManifestCasOutcome::Rejected(ManifestCasRejection::CompletionRequiresPrepared)
        ));
        assert!(!store.manifest_path().exists());
        assert!(!store.root.join(MANIFEST_TEMP_FILE_NAME).exists());

        let prior_root = root("completed-after-prior-completed");
        let prior_store =
            SessionArtifactManifestStore::qualified_for_test(&prior_root).expect("qualified");
        let mut prior_transaction = prior_store.begin_write().expect("transaction");
        assert!(matches!(
            prior_transaction.compare_and_swap(0, basic_candidate("prior-completed", 'a')),
            ManifestCasOutcome::Accepted { .. }
        ));
        assert!(matches!(
            prior_transaction.compare_and_swap(
                1,
                quarantine_candidate(
                    ManifestTransitionState::Completed,
                    QuarantineResidualState::SourceTruncated,
                )
            ),
            ManifestCasOutcome::Rejected(ManifestCasRejection::CompletionRequiresPrepared)
        ));

        fn assert_conflict(name: &str, mutate: impl FnOnce(&mut SessionArtifactManifestV1)) {
            let root = root(name);
            let store = SessionArtifactManifestStore::qualified_for_test(&root).expect("qualified");
            let mut transaction = store.begin_write().expect("transaction");
            assert!(matches!(
                transaction.compare_and_swap(
                    0,
                    quarantine_candidate(
                        ManifestTransitionState::Prepared,
                        QuarantineResidualState::SourceFull,
                    )
                ),
                ManifestCasOutcome::Accepted { .. }
            ));
            let mut completed = quarantine_candidate(
                ManifestTransitionState::Completed,
                QuarantineResidualState::SourceTruncated,
            );
            mutate(&mut completed);
            assert!(matches!(
                transaction.compare_and_swap(1, completed),
                ManifestCasOutcome::Rejected(ManifestCasRejection::PreparedCompletionConflict)
            ));
        }

        assert_conflict("completion-source-identity", |completed| {
            let replacement = identity("streams/replacement.jsonl");
            let quarantine = completed
                .quarantine_transaction
                .as_mut()
                .expect("quarantine");
            quarantine.source_before.managed_identity = replacement.clone();
            quarantine.source_after.managed_identity = replacement.clone();
            completed
                .artifacts
                .iter_mut()
                .find(|entry| entry.kind == SessionArtifactKind::TranscriptRevisions)
                .expect("source entry")
                .managed_identity = replacement;
        });
        assert_conflict("completion-source-hash", |completed| {
            completed
                .quarantine_transaction
                .as_mut()
                .expect("quarantine")
                .source_before
                .content
                .sha256 = digest('f');
        });
        assert_conflict("completion-lengths", |completed| {
            let quarantine = completed
                .quarantine_transaction
                .as_mut()
                .expect("quarantine");
            quarantine.source_before.content.byte_length = 110;
            quarantine.quarantine.content.byte_length = 50;
            completed
                .artifacts
                .iter_mut()
                .find(|entry| entry.kind == SessionArtifactKind::QuarantineRecovery)
                .expect("quarantine entry")
                .availability = ArtifactAvailability::Residual {
                content: quarantine.quarantine.content.clone(),
                reason: ArtifactResidualReason::QuarantineSourceTruncated,
            };
        });
        assert_conflict("completion-target", |completed| {
            let target = content('f', 60);
            completed
                .quarantine_transaction
                .as_mut()
                .expect("quarantine")
                .source_after
                .content = target.clone();
            completed
                .artifacts
                .iter_mut()
                .find(|entry| entry.kind == SessionArtifactKind::TranscriptRevisions)
                .expect("source entry")
                .availability = ArtifactAvailability::Present { content: target };
        });
        assert_conflict("completion-quarantine-identity", |completed| {
            let replacement = identity("recovery/replacement-tail.bin");
            completed
                .quarantine_transaction
                .as_mut()
                .expect("quarantine")
                .quarantine
                .managed_identity = replacement.clone();
            completed
                .artifacts
                .iter_mut()
                .find(|entry| entry.kind == SessionArtifactKind::QuarantineRecovery)
                .expect("quarantine entry")
                .managed_identity = replacement;
        });
        assert_conflict("completion-inventory", |completed| {
            completed.artifacts.push(SessionArtifactEntry {
                kind: SessionArtifactKind::MaterializedNotes,
                privacy_class: ArtifactPrivacyClass::DerivedSessionMemory,
                managed_identity: identity("derived/notes.json"),
                availability: ArtifactAvailability::Present {
                    content: content('f', 12),
                },
            });
        });
    }

    #[cfg(unix)]
    #[test]
    fn prepared_head_rejects_quarantine_transaction_removal_without_durability_mutation() {
        let root = root("prepared-removal-bypass");
        let store = SessionArtifactManifestStore::qualified_for_test(&root).expect("qualified");
        let mut transaction = store.begin_write().expect("transaction");
        assert!(matches!(
            transaction.compare_and_swap(
                0,
                quarantine_candidate(
                    ManifestTransitionState::Prepared,
                    QuarantineResidualState::SourceFull,
                )
            ),
            ManifestCasOutcome::Accepted {
                manifest: SessionArtifactManifestV1 { generation: 1, .. },
                ..
            }
        ));

        let mut removed = basic_candidate("recover-1", 'e');
        removed.transition.state = ManifestTransitionState::Completed;
        assert!(matches!(
            transaction.compare_and_swap(1, removed),
            ManifestCasOutcome::Rejected(ManifestCasRejection::PreparedCompletionConflict)
        ));
        let mut removed_prepared = basic_candidate("recover-1", 'e');
        removed_prepared.transition.state = ManifestTransitionState::Prepared;
        assert!(matches!(
            transaction.compare_and_swap(1, removed_prepared),
            ManifestCasOutcome::Rejected(ManifestCasRejection::Validation(
                ManifestValidationError::PreparedWithoutQuarantine
            ))
        ));
        assert!(matches!(
            transaction.head(),
            ManifestLoadOutcome::Present(head)
                if head.generation == 1
                    && head.transition.state == ManifestTransitionState::Prepared
                    && head.quarantine_transaction.is_some()
        ));
        assert!(!store.root.join(MANIFEST_TEMP_FILE_NAME).exists());
    }

    #[test]
    fn quarantine_reference_length_and_residual_invariants_fail_closed() {
        let completed_full = quarantine_candidate(
            ManifestTransitionState::Prepared,
            QuarantineResidualState::SourceFull,
        );
        let mut completed_full = completed_full;
        completed_full.transition.state = ManifestTransitionState::Completed;
        completed_full
            .quarantine_transaction
            .as_mut()
            .expect("transaction")
            .state = ManifestTransitionState::Completed;
        assert_eq!(
            validate_candidate_and_normalize(&mut completed_full),
            Err(ManifestValidationError::CompletedResidualMismatch)
        );

        let mut wrong_length = quarantine_candidate(
            ManifestTransitionState::Prepared,
            QuarantineResidualState::SourceFull,
        );
        wrong_length
            .quarantine_transaction
            .as_mut()
            .expect("transaction")
            .quarantine
            .content
            .byte_length = 39;
        assert_eq!(
            validate_candidate_and_normalize(&mut wrong_length),
            Err(ManifestValidationError::QuarantineLengthMismatch)
        );

        let mut missing_entry = quarantine_candidate(
            ManifestTransitionState::Prepared,
            QuarantineResidualState::SourceFull,
        );
        missing_entry
            .artifacts
            .retain(|entry| entry.kind != SessionArtifactKind::QuarantineRecovery);
        assert_eq!(
            validate_candidate_and_normalize(&mut missing_entry),
            Err(ManifestValidationError::QuarantineEntryMismatch)
        );
    }

    #[test]
    fn original_audio_unavailable_retains_stable_content_free_evidence_identity() {
        for reason in [
            ArtifactUnavailableReason::RetentionDisabled,
            ArtifactUnavailableReason::NeverCaptured,
            ArtifactUnavailableReason::Expired,
            ArtifactUnavailableReason::DeletedByUser,
            ArtifactUnavailableReason::Inaccessible,
        ] {
            let manifest = SessionArtifactManifestV1::candidate(
                "session-1",
                transition("tx-audio", 'a', ManifestTransitionState::Completed),
                vec![original_audio(ArtifactAvailability::Unavailable { reason })],
                None,
            )
            .expect("unavailable audio is valid");
            let wire = serde_json::to_string(&manifest).expect("wire");
            assert!(wire.contains("audio/original.wav"));
            let value: serde_json::Value = serde_json::from_str(&wire).expect("wire value");
            assert_eq!(
                value["artifacts"][0]["availability"],
                serde_json::json!({
                    "unavailable": {
                        "reason": serde_json::to_value(reason).expect("reason")
                    }
                })
            );
        }
    }

    #[test]
    fn managed_inventory_has_deletion_parity_and_excludes_internal_self_references() {
        let manifest = SessionArtifactManifestV1::candidate(
            "session-1",
            transition("tx-parity", 'a', ManifestTransitionState::Completed),
            vec![
                SessionArtifactEntry {
                    kind: SessionArtifactKind::MaterializedNotes,
                    privacy_class: ArtifactPrivacyClass::DerivedSessionMemory,
                    managed_identity: identity("derived/notes.json"),
                    availability: ArtifactAvailability::Present {
                        content: content('b', 20),
                    },
                },
                original_audio(ArtifactAvailability::Unavailable {
                    reason: ArtifactUnavailableReason::Expired,
                }),
            ],
            None,
        )
        .expect("manifest");
        assert_eq!(
            manifest.managed_inventory(),
            vec![
                identity("audio/original.wav"),
                identity("derived/notes.json")
            ]
        );

        let root = root("internal-identities");
        let internal = SessionArtifactManifestStore::new(&root).internal_identities();
        for identity in [
            &internal.manifest,
            &internal.temporary,
            &internal.coordination,
        ] {
            assert!(!manifest.managed_inventory().contains(identity));
            assert!(ManagedArtifactIdentity::new(identity.as_str()).is_err());
        }
    }
}
