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
use super::canonical_durability::{
    AlgorithmTestEnvironment, CanonicalDurabilityBarrier, CanonicalMutation, CanonicalPlatform,
};
use super::canonical_durability::{
    CanonicalCoordinationError, CanonicalDurability, CanonicalDurabilityIndeterminate,
    CanonicalDurabilityOutcome, CanonicalDurabilityReceipt, CanonicalDurabilityRejection,
    CanonicalExclusiveGuard, CanonicalFilesystemQualification,
    CanonicalFilesystemQualificationError, CanonicalRecoveryKey, CanonicalSnapshotExpectation,
    CanonicalUnlinkOutcome,
};
use super::session_semantics::SessionSemanticsVersion;

pub const SESSION_ARTIFACT_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const SESSION_SEMANTICS_TRANSITION_PROOF_SCHEMA_VERSION: u32 = 1;

/// Conservative cross-platform ceiling for the manifest-controlled relative
/// identity itself. Individual platforms still validate the resolved root plus
/// identity; V1 never persists an unbounded path-shaped value.
pub const MAX_MANAGED_ARTIFACT_IDENTITY_BYTES: usize = 1023;

const MANIFEST_FILE_NAME: &str = ".audio-graph-session-artifacts.v1.json";
const MANIFEST_TEMP_FILE_NAME: &str = ".audio-graph-session-artifacts.v1.tmp";
const COORDINATION_FILE_NAME: &str = ".audio-graph-canonical.lock";
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const SESSION_CONTROL_PREFIX: &str = ".audio-graph-session-";

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
    #[serde(default = "SessionSemanticsVersion::historical_default")]
    pub session_semantics_version: SessionSemanticsVersion,
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
            session_semantics_version: SessionSemanticsVersion::V1,
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

/// Portable Session-owned control identities plus the store-owned lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionControlIdentities {
    pub manifest: ManagedArtifactIdentity,
    pub temporary: ManagedArtifactIdentity,
    pub provenance: ManagedArtifactIdentity,
    pub coordination: ManagedArtifactIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionControlAddress {
    session_id: String,
    identities: SessionControlIdentities,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SessionSemanticsTransitionKind {
    SessionSemanticsAdvance,
}

/// Immutable, digest-free evidence for exactly the v1-to-v2 semantics advance.
///
/// Field declaration order is the canonical compact JSON order. A digest is
/// derived only after these complete bytes exist and is never embedded in the
/// proof itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSemanticsTransitionProofV1 {
    schema_version: u32,
    session_id: String,
    from: u32,
    to: u32,
    idempotency_id: String,
    transition_kind: SessionSemanticsTransitionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSemanticsTransitionProofError {
    Malformed,
    NonCanonical,
    UnsupportedSchema { actual: u32 },
    InvalidTransition,
    InvalidSessionId,
    InvalidIdempotencyId,
}

impl SessionSemanticsTransitionProofV1 {
    pub fn v1_to_v2(
        session_id: impl Into<String>,
        idempotency_id: impl Into<String>,
    ) -> Result<Self, SessionSemanticsTransitionProofError> {
        let proof = Self {
            schema_version: SESSION_SEMANTICS_TRANSITION_PROOF_SCHEMA_VERSION,
            session_id: session_id.into(),
            from: SessionSemanticsVersion::V1.as_u32(),
            to: SessionSemanticsVersion::V2.as_u32(),
            idempotency_id: idempotency_id.into(),
            transition_kind: SessionSemanticsTransitionKind::SessionSemanticsAdvance,
        };
        proof.validate()?;
        Ok(proof)
    }

    pub fn from_canonical_bytes(
        bytes: &[u8],
    ) -> Result<Self, SessionSemanticsTransitionProofError> {
        let proof: Self = serde_json::from_slice(bytes)
            .map_err(|_| SessionSemanticsTransitionProofError::Malformed)?;
        proof.validate()?;
        let canonical = proof.canonical_bytes()?;
        if canonical != bytes {
            return Err(SessionSemanticsTransitionProofError::NonCanonical);
        }
        Ok(proof)
    }

    pub fn canonical_bytes_and_digest(
        &self,
    ) -> Result<(Vec<u8>, Sha256Digest), SessionSemanticsTransitionProofError> {
        self.validate()?;
        let bytes = self.canonical_bytes()?;
        let digest = Sha256::digest(&bytes);
        let digest = Sha256Digest::new(format!("sha256:{digest:x}"))
            .map_err(|_| SessionSemanticsTransitionProofError::Malformed)?;
        Ok((bytes, digest))
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn idempotency_id(&self) -> &str {
        &self.idempotency_id
    }

    fn canonical_bytes(&self) -> Result<Vec<u8>, SessionSemanticsTransitionProofError> {
        serde_json::to_vec(self).map_err(|_| SessionSemanticsTransitionProofError::Malformed)
    }

    fn recovery_identity_prefix_len(
        canonical_bytes: &[u8],
    ) -> Result<usize, SessionSemanticsTransitionProofError> {
        const NEXT_FIELD: &[u8] = b",\"transition_kind\":";
        canonical_bytes
            .windows(NEXT_FIELD.len())
            .position(|window| window == NEXT_FIELD)
            .map(|position| position + 1)
            .ok_or(SessionSemanticsTransitionProofError::Malformed)
    }

    fn validate(&self) -> Result<(), SessionSemanticsTransitionProofError> {
        if self.schema_version != SESSION_SEMANTICS_TRANSITION_PROOF_SCHEMA_VERSION {
            return Err(SessionSemanticsTransitionProofError::UnsupportedSchema {
                actual: self.schema_version,
            });
        }
        if self.from != SessionSemanticsVersion::V1.as_u32()
            || self.to != SessionSemanticsVersion::V2.as_u32()
            || self.transition_kind != SessionSemanticsTransitionKind::SessionSemanticsAdvance
        {
            return Err(SessionSemanticsTransitionProofError::InvalidTransition);
        }
        validate_session_id(&self.session_id)
            .map_err(|_| SessionSemanticsTransitionProofError::InvalidSessionId)?;
        validate_idempotency_id(&self.idempotency_id)
            .map_err(|_| SessionSemanticsTransitionProofError::InvalidIdempotencyId)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V2SessionProvenanceError {
    Missing,
    Duplicate,
    PrivacyMismatch,
    Unavailable,
    Residual,
    TransitionNotCompleted,
    TransitionFingerprintMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestValidationError {
    UnsupportedSchema { actual: u32 },
    UnsupportedSessionSemanticsVersion { actual: u32 },
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
    InvalidV2SessionProvenance(V2SessionProvenanceError),
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
    SessionMismatch,
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
    InvalidSessionAddress,
    NamespaceQualificationRequired,
    Qualification(CanonicalFilesystemQualificationError),
    Coordination(CanonicalCoordinationError),
    Load(ManifestLoadError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestLoadOutcome {
    Absent,
    Present(Box<SessionArtifactManifestV1>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckedManifestReadError<E> {
    Load(ManifestLoadError),
    NamespaceQualificationRequired,
    UncoordinatedAbsence,
    Reader(E),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManifestCasRejection {
    SessionAddressRequired,
    TransitionProof(SessionSemanticsTransitionProofError),
    TransitionProofRequired,
    Validation(ManifestValidationError),
    GenerationConflict {
        expected: u64,
        actual: u64,
    },
    GenerationOverflow,
    SessionMismatch,
    SessionSemanticsFloorRegression {
        current: SessionSemanticsVersion,
        candidate: SessionSemanticsVersion,
    },
    IdempotencyConflict,
    CompletionRequiresPrepared,
    PreparedCompletionConflict,
    CompletedRegression,
    PreparedTransitionReplacement,
    TransitionConflict,
    Serialization,
    ManifestTooLarge {
        byte_length: u64,
    },
    /// A durability refusal forwarded verbatim from the substrate, raised
    /// before the refused call staged, proved, or installed anything of its
    /// own.
    ///
    /// The no-survivor claim is scoped to the ONE refused call, exactly like
    /// the substrate's `CanonicalDurabilityRejection` it forwards — never to
    /// the transaction. Nothing that call made durable outlives this variant,
    /// because every refusal routed here is proven before that call mutated a
    /// canonical byte or namespace entry, and the two states a transition CAN
    /// leave behind carry their own variants:
    /// `TransitionProofRefusedAfterIntentStaged` and
    /// `ManifestInstallRefusedAfterProofAndIntentDurable`.
    ///
    /// It is NOT a claim that the store holds nothing, and NOT a claim about
    /// earlier calls on the same `ManifestWriteTransaction`: a transaction that
    /// already installed a head or made a transition proof durable keeps both
    /// when a later call returns this variant, so a caller must not read it as
    /// "nothing I wrote survives". A temporary the refused call did not stage
    /// likewise outlives the refusal, and this variant carries no key for it
    /// because that call never owned one: on the fresh path such an orphan is
    /// *evidenced* here — a plain `compare_and_swap` refused
    /// `SnapshotTempAlreadyExists` is exactly that evidence — and on the
    /// lock-owned recovery path (`compare_and_swap_recovery`) an adopted orphan
    /// is likewise not the refused call's.
    /// `ManifestWriteTransaction::abandon_staged_transition` is the escape that
    /// needs no key.
    Durability(CanonicalDurabilityRejection),
    /// The immutable transition proof was refused after this transaction had
    /// already durably staged its manifest intent temporary. No canonical
    /// manifest byte moved and the proof record is absent, but the staged
    /// temporary outlives this outcome, and until it is retired every
    /// `compare_and_swap` refuses with `SnapshotTempAlreadyExists`.
    /// `recovery_key` names the staged intent. Two escapes exist: retain the
    /// exact `candidate` and `proof` and replay them byte-exactly through
    /// `advance_session_semantics_v1_to_v2`, or call
    /// `ManifestWriteTransaction::abandon_staged_transition`, which durably
    /// unlinks the temporary and needs no candidate. The inner refusal is
    /// reported verbatim and is never widened into `Durability`, whose contract
    /// is that the refused call staged nothing that survives.
    TransitionProofRefusedAfterIntentStaged {
        rejection: CanonicalDurabilityRejection,
        recovery_key: CanonicalRecoveryKey,
    },
    /// The manifest install was refused after this transaction had already made
    /// BOTH its intent temporary and the immutable transition proof durable.
    ///
    /// The canonical manifest and its generation are unchanged: every
    /// install-stage refusal is proven before the install mutated a byte or a
    /// namespace entry. What this variant asserts about the survivors is that
    /// THIS transaction made them durable and this refusal did not consume
    /// them — the intent temporary named by `recovery_key`, and the proof at
    /// the Session's provenance control identity. It deliberately does NOT
    /// re-verify their current pathnames: for an `IdentityChanged`-class
    /// rejection the refusal itself reports that the namespace moved, so the
    /// records may no longer be reachable under the paths this transaction
    /// wrote them to.
    ///
    /// `recovery_key` is `recovery_key(&candidate.transition.fingerprint)`,
    /// which after fingerprint assignment equals `recovery_key(&proof_digest)`
    /// — the same key `TransitionProofRefusedAfterIntentStaged` reports for the
    /// same temporary. Two escapes exist: a byte-exact replay of the same
    /// candidate and proof through `advance_session_semantics_v1_to_v2`, or
    /// `ManifestWriteTransaction::abandon_staged_transition`, which unlinks the
    /// temporary and leaves the immutable proof in place. The inner refusal is
    /// reported verbatim and is never widened into `Durability`.
    ManifestInstallRefusedAfterProofAndIntentDurable {
        rejection: CanonicalDurabilityRejection,
        recovery_key: CanonicalRecoveryKey,
    },
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
    address: Option<SessionControlAddress>,
    durability: CanonicalDurability,
    qualification: Option<CanonicalFilesystemQualification>,
}

impl SessionArtifactManifestStore {
    #[cfg(test)]
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            address: None,
            durability: CanonicalDurability::new(),
            qualification: None,
        }
    }

    /// Construct a production-addressable, non-mutating Session store.
    ///
    /// Session validation deliberately precedes root conversion, path
    /// derivation, qualification, and filesystem access.
    pub fn for_session(
        root: impl Into<PathBuf>,
        session_id: &str,
    ) -> Result<Self, ManifestStoreError> {
        let address = session_control_address(session_id)?;
        Ok(Self {
            root: root.into(),
            address: Some(address),
            durability: CanonicalDurability::new(),
            qualification: None,
        })
    }

    /// Bind one existing managed root to live production filesystem evidence.
    #[cfg(test)]
    pub(crate) fn qualified_existing_root(
        root: impl Into<PathBuf>,
    ) -> Result<Self, ManifestStoreError> {
        let root = root.into();
        let (qualification, durability) =
            CanonicalFilesystemQualification::for_existing_managed_root(&root)
                .map_err(ManifestStoreError::Qualification)?;
        Ok(Self {
            root,
            address: None,
            durability,
            qualification: Some(qualification),
        })
    }

    /// Bind one production-addressable Session at an existing managed root to
    /// live filesystem qualification.
    pub fn qualified_existing_session(
        root: impl Into<PathBuf>,
        session_id: &str,
    ) -> Result<Self, ManifestStoreError> {
        let address = session_control_address(session_id)?;
        let root = root.into();
        let (qualification, durability) =
            CanonicalFilesystemQualification::for_existing_managed_root(&root)
                .map_err(ManifestStoreError::Qualification)?;
        Ok(Self {
            root,
            address: Some(address),
            durability,
            qualification: Some(qualification),
        })
    }

    pub fn control_identities(&self) -> &SessionControlIdentities {
        &self
            .address
            .as_ref()
            .expect("production Session store has a control address")
            .identities
    }

    pub(crate) fn internal_identities(&self) -> ManifestInternalIdentities {
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
        let guard = match self.qualification.as_ref() {
            Some(qualification) => self
                .durability
                .try_lock_shared_qualified(&self.root, qualification),
            None => self.durability.try_lock_shared(&self.root),
        };
        let _guard = guard.map_err(ManifestLoadError::Coordination)?;
        if !entry_exists(&manifest_path)? {
            return Ok(ManifestLoadOutcome::Absent);
        }
        let (manifest, _) = load_manifest_file(&manifest_path)?;
        self.validate_requested_session(&manifest)?;
        Ok(ManifestLoadOutcome::Present(Box::new(manifest)))
    }

    /// Read one complete Session snapshot while the store's coordination
    /// boundary remains owned by this call.
    pub fn checked_read<T, E>(
        &self,
        reader: impl FnOnce(ManifestLoadOutcome) -> Result<T, E>,
    ) -> Result<T, CheckedManifestReadError<E>> {
        self.checked_read_inner(reader, || {})
    }

    fn checked_read_inner<T, E>(
        &self,
        reader: impl FnOnce(ManifestLoadOutcome) -> Result<T, E>,
        after_establishment: impl FnOnce(),
    ) -> Result<T, CheckedManifestReadError<E>> {
        let Some(qualification) = self.qualification.as_ref() else {
            if self.durability.namespace_mutation_supported() {
                return Err(CheckedManifestReadError::NamespaceQualificationRequired);
            }
            let manifest_before =
                entry_exists(&self.manifest_path()).map_err(CheckedManifestReadError::Load)?;
            let coordination_path = self.root.join(COORDINATION_FILE_NAME);
            let coordination_before =
                entry_exists(&coordination_path).map_err(CheckedManifestReadError::Load)?;
            if coordination_before {
                let _guard = self
                    .durability
                    .try_lock_shared(&self.root)
                    .map_err(|error| {
                        CheckedManifestReadError::Load(ManifestLoadError::Coordination(error))
                    })?;
                let head = self
                    .load_selected_manifest()
                    .map_err(CheckedManifestReadError::Load)?;
                return reader(head).map_err(CheckedManifestReadError::Reader);
            }
            if manifest_before {
                return Err(CheckedManifestReadError::Load(
                    ManifestLoadError::Coordination(CanonicalCoordinationError::Missing),
                ));
            }
            return Err(CheckedManifestReadError::UncoordinatedAbsence);
        };
        let guard = match self
            .durability
            .try_lock_shared_qualified(&self.root, qualification)
        {
            Ok(guard) => guard,
            Err(CanonicalCoordinationError::Missing) => {
                drop(
                    self.durability
                        .try_lock_exclusive_qualified(&self.root, qualification)
                        .map_err(|error| {
                            CheckedManifestReadError::Load(ManifestLoadError::Coordination(error))
                        })?,
                );
                after_establishment();
                self.durability
                    .try_lock_shared_qualified(&self.root, qualification)
                    .map_err(|error| {
                        CheckedManifestReadError::Load(ManifestLoadError::Coordination(error))
                    })?
            }
            Err(error) => {
                return Err(CheckedManifestReadError::Load(
                    ManifestLoadError::Coordination(error),
                ));
            }
        };
        let _guard = guard;
        let head = self
            .load_selected_manifest()
            .map_err(CheckedManifestReadError::Load)?;
        reader(head).map_err(CheckedManifestReadError::Reader)
    }

    #[cfg(test)]
    fn checked_read_with_after_establishment<T, E>(
        &self,
        reader: impl FnOnce(ManifestLoadOutcome) -> Result<T, E>,
        after_establishment: impl FnOnce(),
    ) -> Result<T, CheckedManifestReadError<E>> {
        self.checked_read_inner(reader, after_establishment)
    }

    fn load_selected_manifest(&self) -> Result<ManifestLoadOutcome, ManifestLoadError> {
        let manifest_path = self.manifest_path();
        if !entry_exists(&manifest_path)? {
            return Ok(ManifestLoadOutcome::Absent);
        }
        let (manifest, _) = load_manifest_file(&manifest_path)?;
        self.validate_requested_session(&manifest)?;
        Ok(ManifestLoadOutcome::Present(Box::new(manifest)))
    }

    /// Begin one guard-owning write transaction. The root must already exist;
    /// only the canonical coordination entry may be created by acquisition.
    pub fn begin_write(&self) -> Result<ManifestWriteTransaction<'_>, ManifestStoreError> {
        let qualification = self
            .qualification
            .as_ref()
            .ok_or(ManifestStoreError::NamespaceQualificationRequired)?;
        let guard = self
            .durability
            .try_lock_exclusive_qualified(&self.root, qualification)
            .map_err(ManifestStoreError::Coordination)?;
        let manifest_path = self.manifest_path();
        let (head, head_file) = if entry_exists(&manifest_path).map_err(ManifestStoreError::Load)? {
            let (head, file) =
                load_manifest_file(&manifest_path).map_err(ManifestStoreError::Load)?;
            self.validate_requested_session(&head)
                .map_err(ManifestStoreError::Load)?;
            (Some(head), Some(file))
        } else {
            (None, None)
        };
        Ok(ManifestWriteTransaction {
            guard,
            qualification: self.qualification.as_ref(),
            expected_session_id: self
                .address
                .as_ref()
                .map(|address| address.session_id.as_str()),
            manifest_path,
            temporary_path: self.temporary_path(),
            provenance_path: self
                .address
                .as_ref()
                .map(|address| self.root.join(address.identities.provenance.as_str())),
            provenance_identity: self
                .address
                .as_ref()
                .map(|address| address.identities.provenance.clone()),
            head,
            head_file,
        })
    }

    fn manifest_path(&self) -> PathBuf {
        self.root
            .join(self.address.as_ref().map_or(MANIFEST_FILE_NAME, |address| {
                address.identities.manifest.as_str()
            }))
    }

    fn temporary_path(&self) -> PathBuf {
        self.root.join(
            self.address
                .as_ref()
                .map_or(MANIFEST_TEMP_FILE_NAME, |address| {
                    address.identities.temporary.as_str()
                }),
        )
    }

    fn validate_requested_session(
        &self,
        manifest: &SessionArtifactManifestV1,
    ) -> Result<(), ManifestLoadError> {
        if self
            .address
            .as_ref()
            .is_some_and(|address| address.session_id != manifest.session_id)
        {
            return Err(ManifestLoadError::SessionMismatch);
        }
        Ok(())
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
            address: None,
            durability: CanonicalDurability::new(),
            qualification: Some(qualification),
        })
    }

    #[cfg(test)]
    pub(crate) fn qualified_for_test_session(
        root: impl Into<PathBuf>,
        session_id: &str,
    ) -> Result<Self, ManifestStoreError> {
        let address = session_control_address(session_id)?;
        let root = root.into();
        let qualification = CanonicalFilesystemQualification::for_test_root(&root)
            .map_err(ManifestStoreError::Coordination)?;
        Ok(Self {
            root,
            address: Some(address),
            durability: CanonicalDurability::new(),
            qualification: Some(qualification),
        })
    }

    #[cfg(test)]
    pub(crate) fn qualified_for_algorithm_test(
        root: impl Into<PathBuf>,
    ) -> Result<Self, ManifestStoreError> {
        let root = root.into();
        let environment =
            AlgorithmTestEnvironment::bind(&root).map_err(ManifestStoreError::Coordination)?;
        let (qualification, durability) = environment.into_parts();
        Ok(Self {
            root,
            address: None,
            durability,
            qualification: Some(qualification),
        })
    }

    #[cfg(test)]
    pub(crate) fn qualified_for_algorithm_test_platform(
        root: impl Into<PathBuf>,
        platform: CanonicalPlatform,
    ) -> Result<Self, ManifestStoreError> {
        let root = root.into();
        let environment = AlgorithmTestEnvironment::bind_for_platform(&root, platform)
            .map_err(ManifestStoreError::Coordination)?;
        let (qualification, durability) = environment.into_parts();
        Ok(Self {
            root,
            address: None,
            durability,
            qualification: Some(qualification),
        })
    }

    #[cfg(test)]
    pub(crate) fn for_test_platform(root: impl Into<PathBuf>, platform: CanonicalPlatform) -> Self {
        Self {
            root: root.into(),
            address: None,
            durability: CanonicalDurability::for_test_platform(platform),
            qualification: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test_session_platform(
        root: impl Into<PathBuf>,
        session_id: &str,
        platform: CanonicalPlatform,
    ) -> Result<Self, ManifestStoreError> {
        Ok(Self {
            root: root.into(),
            address: Some(session_control_address(session_id)?),
            durability: CanonicalDurability::for_test_platform(platform),
            qualification: None,
        })
    }
}

/// CAS object that owns the exact canonical exclusive guard and the exact open
/// head handle used for replacement validation.
pub struct ManifestWriteTransaction<'store> {
    guard: CanonicalExclusiveGuard,
    qualification: Option<&'store CanonicalFilesystemQualification>,
    expected_session_id: Option<&'store str>,
    manifest_path: PathBuf,
    temporary_path: PathBuf,
    provenance_path: Option<PathBuf>,
    provenance_identity: Option<ManagedArtifactIdentity>,
    head: Option<SessionArtifactManifestV1>,
    head_file: Option<File>,
}

enum ManifestCasPreparation {
    Install {
        candidate: SessionArtifactManifestV1,
        bytes: Vec<u8>,
        recovery_key: CanonicalRecoveryKey,
    },
    AlreadyCompleted {
        manifest: SessionArtifactManifestV1,
    },
}

enum ManifestCasPreparationResult {
    Prepared(Box<ManifestCasPreparation>),
    Rejected(ManifestCasRejection),
}

/// What this transaction has already made durable when the manifest install
/// runs. It decides how an install-stage refusal must be classified, and it is
/// keyed on the proven staged key rather than on whether the install resumes a
/// temporary: a resumed temporary that pre-dates the transaction has no key
/// this transaction can hand back.
enum ManifestInstallDisposition {
    /// Fresh install; this transaction staged nothing and owns no proof.
    Fresh,
    /// Lock-owned recovery resume. Any temporary this install adopts pre-dates
    /// the transaction, so this transaction has no key that names it.
    ResumeUnstaged,
    /// Transition install. The intent temporary named by this key and the
    /// immutable proof are both already durable before the install runs.
    ResumeStagedTransition {
        intent_recovery_key: CanonicalRecoveryKey,
    },
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
        self.compare_and_swap_inner(
            expected_generation,
            candidate,
            false,
            ManifestInstallDisposition::Fresh,
            None,
        )
    }

    /// Durably establish the exact immutable v1-to-v2 proof before attempting
    /// the manifest generation CAS. The returned value is the authoritative
    /// manifest outcome; proof success alone is never reported as admission.
    pub fn advance_session_semantics_v1_to_v2(
        &mut self,
        expected_generation: u64,
        candidate: SessionArtifactManifestV1,
        proof: SessionSemanticsTransitionProofV1,
    ) -> ManifestCasOutcome {
        self.advance_session_semantics_v1_to_v2_inner(
            expected_generation,
            candidate,
            proof,
            None,
            None,
            None,
        )
    }

    #[cfg(test)]
    fn advance_session_semantics_v1_to_v2_with_faults(
        &mut self,
        expected_generation: u64,
        candidate: SessionArtifactManifestV1,
        proof: SessionSemanticsTransitionProofV1,
        intent_fault: Option<super::canonical_durability::CanonicalDurabilityStage>,
        proof_fault: Option<super::canonical_durability::CanonicalDurabilityStage>,
        manifest_fault: Option<super::canonical_durability::CanonicalDurabilityStage>,
    ) -> ManifestCasOutcome {
        self.advance_session_semantics_v1_to_v2_inner(
            expected_generation,
            candidate,
            proof,
            intent_fault,
            proof_fault,
            manifest_fault,
        )
    }

    fn advance_session_semantics_v1_to_v2_inner(
        &mut self,
        expected_generation: u64,
        mut candidate: SessionArtifactManifestV1,
        proof: SessionSemanticsTransitionProofV1,
        _intent_fault: Option<super::canonical_durability::CanonicalDurabilityStage>,
        _proof_fault: Option<super::canonical_durability::CanonicalDurabilityStage>,
        _manifest_fault: Option<super::canonical_durability::CanonicalDurabilityStage>,
    ) -> ManifestCasOutcome {
        let (Some(expected_session_id), Some(provenance_path), Some(provenance_identity)) = (
            self.expected_session_id,
            self.provenance_path.as_ref(),
            self.provenance_identity.as_ref(),
        ) else {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::SessionAddressRequired);
        };
        if proof.session_id() != expected_session_id || candidate.session_id != expected_session_id
        {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::SessionMismatch);
        }
        if proof.idempotency_id() != candidate.transition.idempotency_id {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::IdempotencyConflict);
        }
        if candidate.session_semantics_version != SessionSemanticsVersion::V2
            || candidate.transition.state != ManifestTransitionState::Completed
        {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::TransitionProof(
                SessionSemanticsTransitionProofError::InvalidTransition,
            ));
        }
        let (proof_bytes, proof_digest) = match proof.canonical_bytes_and_digest() {
            Ok(proof) => proof,
            Err(error) => {
                return ManifestCasOutcome::Rejected(ManifestCasRejection::TransitionProof(error));
            }
        };
        let proof_length = u64::try_from(proof_bytes.len()).unwrap_or(u64::MAX);
        let proof_recovery_prefix_len =
            match SessionSemanticsTransitionProofV1::recovery_identity_prefix_len(&proof_bytes) {
                Ok(prefix_len) => prefix_len,
                Err(error) => {
                    return ManifestCasOutcome::Rejected(ManifestCasRejection::TransitionProof(
                        error,
                    ));
                }
            };
        candidate.transition.fingerprint = proof_digest.clone();
        let mut provenance_entries = candidate
            .artifacts
            .iter_mut()
            .filter(|artifact| artifact.kind == SessionArtifactKind::SessionProvenanceEvents);
        let Some(provenance) = provenance_entries.next() else {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::Validation(
                ManifestValidationError::InvalidV2SessionProvenance(
                    V2SessionProvenanceError::Missing,
                ),
            ));
        };
        if provenance_entries.next().is_some() {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::Validation(
                ManifestValidationError::InvalidV2SessionProvenance(
                    V2SessionProvenanceError::Duplicate,
                ),
            ));
        }
        provenance.managed_identity = provenance_identity.clone();
        match &mut provenance.availability {
            ArtifactAvailability::Present { content } => {
                *content = ArtifactContentIdentity {
                    sha256: proof_digest.clone(),
                    byte_length: proof_length,
                };
            }
            ArtifactAvailability::Unavailable { .. } => {
                return ManifestCasOutcome::Rejected(ManifestCasRejection::Validation(
                    ManifestValidationError::InvalidV2SessionProvenance(
                        V2SessionProvenanceError::Unavailable,
                    ),
                ));
            }
            ArtifactAvailability::Residual { .. } => {
                return ManifestCasOutcome::Rejected(ManifestCasRejection::Validation(
                    ManifestValidationError::InvalidV2SessionProvenance(
                        V2SessionProvenanceError::Residual,
                    ),
                ));
            }
        }
        if let Err(error) = validate_candidate_and_normalize(&mut candidate) {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::Validation(error));
        }
        let preparation = match self.prepare_compare_and_swap(expected_generation, candidate, true)
        {
            ManifestCasPreparationResult::Prepared(preparation) => *preparation,
            ManifestCasPreparationResult::Rejected(rejection) => {
                return ManifestCasOutcome::Rejected(rejection);
            }
        };

        if let Err(rejection) =
            self.guard
                .preflight_immutable_exact(provenance_path, &proof_bytes, self.qualification)
        {
            return ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(rejection));
        }

        // `Some(key)` from here on means this transaction owns a durable intent
        // temporary that outlives any later refusal.
        let staged_intent_recovery_key = if let ManifestCasPreparation::Install {
            bytes,
            recovery_key,
            ..
        } = &preparation
        {
            let expectation = self
                .head_file
                .as_ref()
                .map_or(CanonicalSnapshotExpectation::Absent, |file| {
                    CanonicalSnapshotExpectation::Existing(file)
                });
            let intent_outcome = {
                #[cfg(test)]
                if let Some(intent_fault) = _intent_fault {
                    self.guard.stage_snapshot_temporary_with_fault(
                        &self.temporary_path,
                        &self.manifest_path,
                        bytes,
                        expectation,
                        self.qualification,
                        *recovery_key,
                        intent_fault,
                    )
                } else {
                    self.guard.stage_snapshot_temporary(
                        &self.temporary_path,
                        &self.manifest_path,
                        bytes,
                        expectation,
                        self.qualification,
                        *recovery_key,
                    )
                }

                #[cfg(not(test))]
                self.guard.stage_snapshot_temporary(
                    &self.temporary_path,
                    &self.manifest_path,
                    bytes,
                    expectation,
                    self.qualification,
                    *recovery_key,
                )
            };
            match intent_outcome {
                CanonicalDurabilityOutcome::Accepted(_) => Some(*recovery_key),
                CanonicalDurabilityOutcome::Rejected(rejection) => {
                    return ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(
                        rejection,
                    ));
                }
                CanonicalDurabilityOutcome::DurabilityIndeterminate(indeterminate) => {
                    return ManifestCasOutcome::DurabilityIndeterminate(indeterminate);
                }
            }
        } else {
            None
        };

        let proof_recovery_key = recovery_key(&proof_digest);
        let proof_outcome = {
            #[cfg(test)]
            if let Some(proof_fault) = _proof_fault {
                match &preparation {
                    ManifestCasPreparation::Install { bytes, .. } => self
                        .guard
                        .create_or_reconcile_immutable_exact_with_authentication_and_fault(
                            provenance_path,
                            &proof_bytes,
                            &self.temporary_path,
                            bytes,
                            self.qualification,
                            proof_recovery_key,
                            proof_fault,
                        ),
                    ManifestCasPreparation::AlreadyCompleted { .. } => self
                        .guard
                        .create_or_reconcile_immutable_exact_with_identity_prefix_and_fault(
                            provenance_path,
                            &proof_bytes,
                            proof_recovery_prefix_len,
                            self.qualification,
                            proof_recovery_key,
                            proof_fault,
                        ),
                }
            } else {
                match &preparation {
                    ManifestCasPreparation::Install { bytes, .. } => self
                        .guard
                        .create_or_reconcile_immutable_exact_with_authentication(
                            provenance_path,
                            &proof_bytes,
                            &self.temporary_path,
                            bytes,
                            self.qualification,
                            proof_recovery_key,
                        ),
                    ManifestCasPreparation::AlreadyCompleted { .. } => self
                        .guard
                        .create_or_reconcile_immutable_exact_with_identity_prefix(
                            provenance_path,
                            &proof_bytes,
                            proof_recovery_prefix_len,
                            self.qualification,
                            proof_recovery_key,
                        ),
                }
            }

            #[cfg(not(test))]
            match &preparation {
                ManifestCasPreparation::Install { bytes, .. } => self
                    .guard
                    .create_or_reconcile_immutable_exact_with_authentication(
                        provenance_path,
                        &proof_bytes,
                        &self.temporary_path,
                        bytes,
                        self.qualification,
                        proof_recovery_key,
                    ),
                ManifestCasPreparation::AlreadyCompleted { .. } => self
                    .guard
                    .create_or_reconcile_immutable_exact_with_identity_prefix(
                        provenance_path,
                        &proof_bytes,
                        proof_recovery_prefix_len,
                        self.qualification,
                        proof_recovery_key,
                    ),
            }
        };
        match proof_outcome {
            CanonicalDurabilityOutcome::Accepted(_) => {}
            // A proof refusal is only a refusal of the whole transaction when
            // this transaction staged nothing of its own. Once the intent
            // temporary is durable it survives the refusal, so the outcome must
            // name it and hand back the key that identifies it.
            CanonicalDurabilityOutcome::Rejected(rejection) => {
                return ManifestCasOutcome::Rejected(match staged_intent_recovery_key {
                    Some(recovery_key) => {
                        ManifestCasRejection::TransitionProofRefusedAfterIntentStaged {
                            rejection,
                            recovery_key,
                        }
                    }
                    None => ManifestCasRejection::Durability(rejection),
                });
            }
            CanonicalDurabilityOutcome::DurabilityIndeterminate(indeterminate) => {
                return ManifestCasOutcome::DurabilityIndeterminate(indeterminate);
            }
        }

        // The install runs with both this transaction's intent temporary and its
        // immutable proof already durable, so an install refusal cannot borrow
        // the pre-mutation `Durability` vocabulary. `AlreadyCompleted`
        // preparations return before the install and never reach that arm.
        let disposition = match staged_intent_recovery_key {
            Some(intent_recovery_key) => ManifestInstallDisposition::ResumeStagedTransition {
                intent_recovery_key,
            },
            None => ManifestInstallDisposition::ResumeUnstaged,
        };

        #[cfg(test)]
        if let Some(manifest_fault) = _manifest_fault {
            return self.commit_prepared_compare_and_swap(
                preparation,
                disposition,
                Some(manifest_fault),
            );
        }
        self.commit_prepared_compare_and_swap(preparation, disposition, None)
    }

    /// Abandon this Session's staged manifest-intent temporary.
    ///
    /// This is the reachable escape for the survivors named by
    /// `ManifestCasRejection::TransitionProofRefusedAfterIntentStaged` and
    /// `ManifestCasRejection::ManifestInstallRefusedAfterProofAndIntentDurable`:
    /// it durably unlinks the Session's own manifest temporary, so a later
    /// `compare_and_swap` is no longer refused `SnapshotTempAlreadyExists` and a
    /// different candidate can install.
    ///
    /// It removes ONLY the temporary. This transaction's cached head is not
    /// re-read, and neither the canonical manifest, its generation, nor the
    /// immutable transition proof is written or removed: a durable v2 proof
    /// therefore still refuses a DIFFERENT transition id with
    /// `ImmutableExactConflict`. Removing or re-keying that proof is out of
    /// scope — audio-graph-68a1 owns the missing proof binding.
    ///
    /// It is NOT the reconciliation for a `DurabilityIndeterminate` outcome.
    /// After any indeterminate the reconciliation is the exact rerun keyed by
    /// that outcome's recovery key. An indeterminate install whose rename was
    /// invoked but unacknowledged may already have consumed the temporary, so
    /// abandon would find it absent and its parent barrier could itself become
    /// the durability point of that unacknowledged install. `AlreadyAbsent`
    /// therefore asserts only that this call removed nothing; it does not assert
    /// that the manifest head is unchanged.
    ///
    /// The temporary's content is deliberately NOT authenticated. Abandon exists
    /// for a caller that no longer holds the candidate; requiring its bytes
    /// would reinstate the wedge it removes. Authorization is the exclusive
    /// guard, the derived Session-owned temporary identity — never
    /// caller-supplied, and never equal to the manifest identity — and the
    /// substrate's reserved-name, regular-file, and open-handle identity fences.
    ///
    /// The recovery key names THIS unlink of THIS temporary pathname, not the
    /// abandoned candidate: abandon needs no candidate. `AlreadyAbsent` is the
    /// no-effect assessment of an exact rerun.
    pub fn abandon_staged_transition(&self) -> CanonicalUnlinkOutcome {
        self.guard.unlink_canonical_entry(
            &self.temporary_path,
            self.qualification,
            temporary_abandon_recovery_key(&self.temporary_path),
        )
    }

    pub(crate) fn compare_and_swap_recovery(
        &mut self,
        expected_generation: u64,
        candidate: SessionArtifactManifestV1,
    ) -> ManifestCasOutcome {
        self.compare_and_swap_inner(
            expected_generation,
            candidate,
            false,
            ManifestInstallDisposition::ResumeUnstaged,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn compare_and_swap_recovery_with_fault(
        &mut self,
        expected_generation: u64,
        candidate: SessionArtifactManifestV1,
        injected_fault: super::canonical_durability::CanonicalDurabilityStage,
    ) -> ManifestCasOutcome {
        self.compare_and_swap_inner(
            expected_generation,
            candidate,
            false,
            ManifestInstallDisposition::ResumeUnstaged,
            Some(injected_fault),
        )
    }

    fn prepare_compare_and_swap(
        &self,
        expected_generation: u64,
        mut candidate: SessionArtifactManifestV1,
        proof_owned: bool,
    ) -> ManifestCasPreparationResult {
        let reject = ManifestCasPreparationResult::Rejected;
        if self
            .expected_session_id
            .is_some_and(|expected| expected != candidate.session_id)
        {
            return reject(ManifestCasRejection::SessionMismatch);
        }
        if let Err(error) = validate_candidate_and_normalize(&mut candidate) {
            return reject(ManifestCasRejection::Validation(error));
        }
        // audio-graph-3b53 B4 is KNOWN OPEN here, deliberately: an addressed
        // Session that reaches V2 cannot record a later generation, because this
        // gate keys on the candidate's floor rather than on whether this call
        // performs the transition.
        //
        // Re-keying it on "this call advances the head" was implemented and
        // REVERTED. Relaxing the gate for an already-advanced head lets the
        // generic `compare_and_swap` install a V2 candidate carrying a FORGED
        // provenance entry: `validate_v2_session_provenance` only checks that
        // `manifest.transition.fingerprint == content.sha256` for the candidate's
        // own entry (:1868), which is internal self-consistency with no
        // reference to the durable proof record, so a caller that controls both
        // fields satisfies it while the real proof sits untouched at the control
        // identity. Pre-fix that path was unreachable because every V2 candidate
        // through the generic CAS was refused here.
        //
        // Re-keying is therefore blocked on a binding that does not exist yet:
        // the candidate's provenance entry must be proven to reference the
        // durable proof record. Until that lands, this gate trades a liveness
        // wedge for a forgery hole, so the wedge stays.
        if self.expected_session_id.is_some()
            && candidate.session_semantics_version == SessionSemanticsVersion::V2
            && !proof_owned
        {
            return reject(ManifestCasRejection::TransitionProofRequired);
        }

        if let Some(head) = &self.head {
            if head.session_id != candidate.session_id {
                return reject(ManifestCasRejection::SessionMismatch);
            }
            if candidate.session_semantics_version < head.session_semantics_version {
                return reject(ManifestCasRejection::SessionSemanticsFloorRegression {
                    current: head.session_semantics_version,
                    candidate: candidate.session_semantics_version,
                });
            }
            match head.transition.state {
                ManifestTransitionState::Prepared => {
                    if head.transition.idempotency_id != candidate.transition.idempotency_id {
                        return reject(ManifestCasRejection::PreparedTransitionReplacement);
                    }
                    if head.transition.fingerprint != candidate.transition.fingerprint {
                        return reject(ManifestCasRejection::IdempotencyConflict);
                    }
                    if candidate.transition.state != ManifestTransitionState::Completed
                        || candidate.quarantine_transaction.is_none()
                        || !prepared_completion_matches(head, &candidate)
                    {
                        return reject(ManifestCasRejection::PreparedCompletionConflict);
                    }
                }
                ManifestTransitionState::Completed
                    if head.transition.idempotency_id == candidate.transition.idempotency_id =>
                {
                    if head.transition.fingerprint != candidate.transition.fingerprint {
                        return reject(ManifestCasRejection::IdempotencyConflict);
                    }
                    if candidate.transition.state == ManifestTransitionState::Prepared {
                        return reject(ManifestCasRejection::CompletedRegression);
                    }
                    candidate.generation = head.generation;
                    if candidate == *head {
                        return ManifestCasPreparationResult::Prepared(Box::new(
                            ManifestCasPreparation::AlreadyCompleted {
                                manifest: head.clone(),
                            },
                        ));
                    }
                    return reject(ManifestCasRejection::TransitionConflict);
                }
                ManifestTransitionState::Completed
                    if candidate.quarantine_transaction.is_some()
                        && candidate.transition.state == ManifestTransitionState::Completed =>
                {
                    return reject(ManifestCasRejection::CompletionRequiresPrepared);
                }
                ManifestTransitionState::Completed => {}
            }
        } else if candidate.quarantine_transaction.is_some()
            && candidate.transition.state == ManifestTransitionState::Completed
        {
            return reject(ManifestCasRejection::CompletionRequiresPrepared);
        }

        let actual_generation = self.head.as_ref().map_or(0, |head| head.generation);
        if expected_generation != actual_generation {
            return reject(ManifestCasRejection::GenerationConflict {
                expected: expected_generation,
                actual: actual_generation,
            });
        }
        let Some(next_generation) = expected_generation.checked_add(1) else {
            return reject(ManifestCasRejection::GenerationOverflow);
        };
        candidate.generation = next_generation;
        if let Err(error) = validate_persisted_and_normalize(&mut candidate) {
            return reject(ManifestCasRejection::Validation(error));
        }
        let bytes = match serde_json::to_vec(&candidate) {
            Ok(bytes) => bytes,
            Err(_) => return reject(ManifestCasRejection::Serialization),
        };
        let byte_length = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if byte_length > MAX_MANIFEST_BYTES {
            return reject(ManifestCasRejection::ManifestTooLarge { byte_length });
        }
        let recovery_key = recovery_key(&candidate.transition.fingerprint);
        ManifestCasPreparationResult::Prepared(Box::new(ManifestCasPreparation::Install {
            candidate,
            bytes,
            recovery_key,
        }))
    }

    fn commit_prepared_compare_and_swap(
        &mut self,
        preparation: ManifestCasPreparation,
        disposition: ManifestInstallDisposition,
        _injected_fault: Option<super::canonical_durability::CanonicalDurabilityStage>,
    ) -> ManifestCasOutcome {
        let resume_temporary = !matches!(disposition, ManifestInstallDisposition::Fresh);
        let (candidate, bytes, recovery_key) = match preparation {
            ManifestCasPreparation::Install {
                candidate,
                bytes,
                recovery_key,
            } => (candidate, bytes, recovery_key),
            ManifestCasPreparation::AlreadyCompleted { manifest } => {
                return ManifestCasOutcome::AlreadyCompleted { manifest };
            }
        };
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
            // An install-stage refusal may be forwarded as a plain
            // `Durability` only when this call had made nothing of its own
            // durable. On the transition path the intent temporary and the
            // immutable proof are both already durable here, so the outcome
            // must name them and hand back the key that identifies the
            // temporary.
            CanonicalDurabilityOutcome::Rejected(rejection) => {
                ManifestCasOutcome::Rejected(match disposition {
                    ManifestInstallDisposition::ResumeStagedTransition {
                        intent_recovery_key,
                    } => ManifestCasRejection::ManifestInstallRefusedAfterProofAndIntentDurable {
                        rejection,
                        recovery_key: intent_recovery_key,
                    },
                    ManifestInstallDisposition::Fresh
                    | ManifestInstallDisposition::ResumeUnstaged => {
                        ManifestCasRejection::Durability(rejection)
                    }
                })
            }
            CanonicalDurabilityOutcome::DurabilityIndeterminate(indeterminate) => {
                ManifestCasOutcome::DurabilityIndeterminate(indeterminate)
            }
        }
    }

    fn compare_and_swap_inner(
        &mut self,
        expected_generation: u64,
        candidate: SessionArtifactManifestV1,
        proof_owned: bool,
        disposition: ManifestInstallDisposition,
        _injected_fault: Option<super::canonical_durability::CanonicalDurabilityStage>,
    ) -> ManifestCasOutcome {
        let preparation =
            match self.prepare_compare_and_swap(expected_generation, candidate, proof_owned) {
                ManifestCasPreparationResult::Prepared(preparation) => *preparation,
                ManifestCasPreparationResult::Rejected(rejection) => {
                    return ManifestCasOutcome::Rejected(rejection);
                }
            };
        self.commit_prepared_compare_and_swap(preparation, disposition, _injected_fault)
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
    if !manifest.session_semantics_version.is_supported() {
        return Err(
            ManifestValidationError::UnsupportedSessionSemanticsVersion {
                actual: manifest.session_semantics_version.as_u32(),
            },
        );
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
    if manifest.session_semantics_version == SessionSemanticsVersion::V2 {
        validate_v2_session_provenance(manifest)
            .map_err(ManifestValidationError::InvalidV2SessionProvenance)?;
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

pub(crate) fn validate_v2_session_provenance(
    manifest: &SessionArtifactManifestV1,
) -> Result<(), V2SessionProvenanceError> {
    let mut proofs = manifest
        .artifacts
        .iter()
        .filter(|artifact| artifact.kind == SessionArtifactKind::SessionProvenanceEvents);
    let Some(proof) = proofs.next() else {
        return Err(V2SessionProvenanceError::Missing);
    };
    if proofs.next().is_some() {
        return Err(V2SessionProvenanceError::Duplicate);
    }
    if proof.privacy_class != ArtifactPrivacyClass::CanonicalSessionMemory {
        return Err(V2SessionProvenanceError::PrivacyMismatch);
    }
    let content = match &proof.availability {
        ArtifactAvailability::Present { content } => content,
        ArtifactAvailability::Unavailable { .. } => {
            return Err(V2SessionProvenanceError::Unavailable);
        }
        ArtifactAvailability::Residual { .. } => {
            return Err(V2SessionProvenanceError::Residual);
        }
    };
    if manifest.transition.state != ManifestTransitionState::Completed {
        return Err(V2SessionProvenanceError::TransitionNotCompleted);
    }
    if manifest.transition.fingerprint != content.sha256 {
        return Err(V2SessionProvenanceError::TransitionFingerprintMismatch);
    }
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

/// Domain separator so an abandon key can never equal a candidate-fingerprint
/// key for any input.
const TEMPORARY_ABANDON_KEY_DOMAIN: &[u8] = b"audio-graph/session-manifest-temporary-abandon/v1\0";

/// Recovery key for one abandon of one derived Session temporary.
///
/// Keyed on the temporary's own pathname, which is derived from the store root
/// and the validated Session id: distinct across Sessions and roots, and
/// identical on every rerun by any transaction of a store built from the same
/// root SPELLING. The root is taken verbatim from the caller and is not
/// canonicalized here, so two equivalent spellings of one root (a trailing
/// separator, a `..` segment, a symlinked data dir) derive DIFFERENT keys for
/// the same logical unlink of the same inode. A caller that matches outcome keys
/// across a restart must therefore reconcile through a store built from the same
/// spelling it used originally. It is deliberately independent of the manifest
/// head and of any candidate, because abandon reconciles its own unlink and
/// holds no candidate.
fn temporary_abandon_recovery_key(temporary_path: &Path) -> CanonicalRecoveryKey {
    let mut hasher = Sha256::new();
    hasher.update(TEMPORARY_ABANDON_KEY_DOMAIN);
    hasher.update(temporary_path.as_os_str().as_encoded_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    CanonicalRecoveryKey::from_opaque_bytes(bytes)
}

fn session_control_address(session_id: &str) -> Result<SessionControlAddress, ManifestStoreError> {
    if !crate::sessions::session_id_is_valid(session_id) {
        return Err(ManifestStoreError::InvalidSessionAddress);
    }
    let key = encode_lowercase_base32(session_id.as_bytes());
    let identity =
        |suffix: &str| ManagedArtifactIdentity(format!("{SESSION_CONTROL_PREFIX}{key}{suffix}"));
    Ok(SessionControlAddress {
        session_id: session_id.to_owned(),
        identities: SessionControlIdentities {
            manifest: identity("-artifacts.v1.json"),
            temporary: identity("-artifacts.v1.tmp"),
            provenance: identity("-v1-v2.provenance"),
            coordination: ManagedArtifactIdentity::internal(COORDINATION_FILE_NAME),
        },
    })
}

fn encode_lowercase_base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
    let mut encoded = String::with_capacity((bytes.len() * 8).div_ceil(5));
    let mut accumulator = 0_u16;
    let mut available_bits = 0_u8;
    for &byte in bytes {
        accumulator = (accumulator << 8) | u16::from(byte);
        available_bits += 8;
        while available_bits >= 5 {
            available_bits -= 5;
            let index = usize::from((accumulator >> available_bits) & 0x1f);
            encoded.push(char::from(ALPHABET[index]));
            accumulator &= (1_u16 << available_bits).saturating_sub(1);
        }
    }
    if available_bits != 0 {
        let index = usize::from((accumulator << (5 - available_bits)) & 0x1f);
        encoded.push(char::from(ALPHABET[index]));
    }
    encoded
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

    fn session_provenance() -> SessionArtifactEntry {
        SessionArtifactEntry {
            kind: SessionArtifactKind::SessionProvenanceEvents,
            privacy_class: ArtifactPrivacyClass::CanonicalSessionMemory,
            managed_identity: identity("streams/session-provenance.jsonl"),
            availability: ArtifactAvailability::Present {
                content: content('e', 48),
            },
        }
    }

    fn v2_candidate(id: &str) -> SessionArtifactManifestV1 {
        let mut candidate = basic_candidate(id, 'e');
        candidate.session_semantics_version =
            super::super::session_semantics::SessionSemanticsVersion::V2;
        candidate.artifacts.push(session_provenance());
        candidate
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
        drop(
            OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(store.root.join(COORDINATION_FILE_NAME))
                .expect("create strict-load coordination fixture"),
        );
    }

    fn write_manifest_bytes(store: &SessionArtifactManifestStore, bytes: &[u8]) {
        create_coordination_entry(store);
        std::fs::write(store.manifest_path(), bytes).expect("write manifest fixture");
    }

    #[test]
    fn production_session_store_derives_exact_portable_control_identities_without_io() {
        let root = missing_root("production-session-address");

        let store = SessionArtifactManifestStore::for_session(&root, "A")
            .expect("address one validated Session");
        let identities = store.control_identities();

        assert_eq!(
            identities.manifest.as_str(),
            ".audio-graph-session-ie-artifacts.v1.json"
        );
        assert_eq!(
            identities.temporary.as_str(),
            ".audio-graph-session-ie-artifacts.v1.tmp"
        );
        assert_eq!(
            identities.provenance.as_str(),
            ".audio-graph-session-ie-v1-v2.provenance"
        );
        assert_eq!(identities.coordination.as_str(), COORDINATION_FILE_NAME);
        assert!(!root.exists(), "address derivation must not touch the root");
    }

    #[test]
    fn production_session_address_uses_narrow_sessions_validation_without_narrowing_wire() {
        let missing = missing_root("production-address-validation");
        let max_addressable = "a".repeat(128);
        let too_long = "a".repeat(129);
        let broad_max = "b".repeat(255);

        assert!(SessionArtifactManifestStore::for_session(&missing, &max_addressable).is_ok());
        for ineligible in [&too_long, "session-é", &broad_max] {
            assert!(matches!(
                SessionArtifactManifestStore::for_session(&missing, ineligible),
                Err(ManifestStoreError::InvalidSessionAddress)
            ));
        }
        assert!(!missing.exists(), "address refusal must perform no I/O");

        for wire_only in ["session-é", broad_max.as_str()] {
            assert!(
                SessionArtifactManifestV1::candidate(
                    wire_only,
                    transition("wire-only", 'a', ManifestTransitionState::Completed),
                    vec![original_audio(ArtifactAvailability::Unavailable {
                        reason: ArtifactUnavailableReason::NeverCaptured,
                    })],
                    None,
                )
                .is_ok(),
                "dormant manifest wire remains broad for {wire_only:?}"
            );
        }

        let upper = SessionArtifactManifestStore::for_session(&missing, "Session-A")
            .expect("uppercase address");
        let lower = SessionArtifactManifestStore::for_session(&missing, "session-a")
            .expect("lowercase address");
        assert_ne!(
            upper.control_identities().manifest,
            lower.control_identities().manifest
        );
    }

    #[test]
    fn two_session_addresses_share_only_the_store_coordination_identity() {
        let root = missing_root("two-session-addresses");
        let first = SessionArtifactManifestStore::for_session(&root, "session-1")
            .expect("first Session address");
        let second = SessionArtifactManifestStore::for_session(&root, "session-2")
            .expect("second Session address");
        let first = first.control_identities();
        let second = second.control_identities();

        assert_ne!(first.manifest, second.manifest);
        assert_ne!(first.temporary, second.temporary);
        assert_ne!(first.provenance, second.provenance);
        assert_eq!(first.coordination, second.coordination);
        assert!(!root.exists());
    }

    #[test]
    fn production_session_load_refuses_requested_manifest_mismatch() {
        let root = root("production-session-mismatch");
        let store = SessionArtifactManifestStore::for_session(&root, "session-1")
            .expect("requested Session store");
        let mut foreign = basic_candidate("foreign-head", 'a');
        foreign.session_id = "session-2".to_owned();
        foreign.generation = 1;
        write_manifest_bytes(
            &store,
            &serde_json::to_vec(&foreign).expect("foreign manifest bytes"),
        );

        assert_eq!(store.load(), Err(ManifestLoadError::SessionMismatch));
    }

    #[cfg(unix)]
    #[test]
    fn two_qualified_session_stores_persist_independent_manifest_heads() {
        let root = root("two-qualified-session-heads");
        let first = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("first qualified Session store");
        let second = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-2")
            .expect("second qualified Session store");

        let mut first_write = first.begin_write().expect("first transaction");
        assert!(matches!(
            first_write.compare_and_swap(0, basic_candidate("first", 'a')),
            ManifestCasOutcome::Accepted { .. }
        ));
        drop(first_write);

        let mut second_candidate = basic_candidate("second", 'b');
        second_candidate.session_id = "session-2".to_owned();
        let mut second_write = second.begin_write().expect("second transaction");
        assert!(matches!(
            second_write.compare_and_swap(0, second_candidate),
            ManifestCasOutcome::Accepted { .. }
        ));
        drop(second_write);

        assert!(matches!(
            first.load().expect("first head"),
            ManifestLoadOutcome::Present(manifest) if manifest.session_id == "session-1"
        ));
        assert!(matches!(
            second.load().expect("second head"),
            ManifestLoadOutcome::Present(manifest) if manifest.session_id == "session-2"
        ));
        assert_ne!(first.manifest_path(), second.manifest_path());
        assert!(root.join(COORDINATION_FILE_NAME).is_file());
    }

    #[test]
    fn qualified_checked_read_establishes_global_lock_and_revalidates_absent_session() {
        let root = root("qualified-checked-read-absent");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified Session store");
        assert!(!root.join(COORDINATION_FILE_NAME).exists());

        let observed = store
            .checked_read(|head| {
                assert!(root.join(COORDINATION_FILE_NAME).is_file());
                Ok::<_, ()>(head)
            })
            .expect("checked absent Session read");

        assert_eq!(observed, ManifestLoadOutcome::Absent);
        assert!(root.join(COORDINATION_FILE_NAME).is_file());
        assert!(!store.manifest_path().exists());
    }

    #[cfg(unix)]
    #[test]
    fn qualified_checked_read_holds_shared_guard_through_present_snapshot_closure() {
        let root = root("qualified-checked-read-present");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified Session store");
        let mut write = store.begin_write().expect("seed manifest head");
        assert!(matches!(
            write.compare_and_swap(0, basic_candidate("seed-head", 'a')),
            ManifestCasOutcome::Accepted { .. }
        ));
        drop(write);

        store
            .checked_read(|head| {
                assert!(matches!(head, ManifestLoadOutcome::Present(_)));
                assert!(matches!(
                    store.begin_write(),
                    Err(ManifestStoreError::Coordination(
                        CanonicalCoordinationError::Contended
                    ))
                ));
                Ok::<_, ()>(())
            })
            .expect("guard-owned present read");
    }

    #[cfg(unix)]
    #[test]
    fn qualified_checked_read_observes_writer_winning_after_lock_establishment() {
        let root = root("qualified-checked-read-writer-win");
        let reader = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified reader");
        let writer = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified writer");

        let observed = reader
            .checked_read_with_after_establishment(Ok::<_, ()>, || {
                let mut transaction = writer.begin_write().expect("winning writer");
                assert!(matches!(
                    transaction.compare_and_swap(0, basic_candidate("writer-win", 'a')),
                    ManifestCasOutcome::Accepted { .. }
                ));
            })
            .expect("checked reader observes winning writer");

        assert!(matches!(
            observed,
            ManifestLoadOutcome::Present(manifest) if manifest.transition.idempotency_id == "writer-win"
        ));
    }

    #[test]
    fn unqualified_absent_checked_read_refuses_before_reader_when_aba_is_unobservable() {
        use std::cell::Cell;

        for platform in [CanonicalPlatform::Windows, CanonicalPlatform::Other] {
            for transient in ["manifest", "lock"] {
                let root = root(&format!("unqualified-{platform:?}-{transient}-aba-refusal"));
                let store = SessionArtifactManifestStore::for_test_session_platform(
                    &root,
                    "session-1",
                    platform,
                )
                .expect("read-only Session store");
                let transient_path = if transient == "manifest" {
                    store.manifest_path()
                } else {
                    root.join(COORDINATION_FILE_NAME)
                };
                std::fs::write(&transient_path, b"transient appearance")
                    .expect("seed transient identity");
                std::fs::remove_file(&transient_path).expect("complete ABA disappearance");
                let reader_invoked = Cell::new(false);

                let outcome = store.checked_read(|_| {
                    reader_invoked.set(true);
                    Ok::<_, ()>("manifest-or-lock ABA must not escape")
                });

                assert_eq!(outcome, Err(CheckedManifestReadError::UncoordinatedAbsence));
                assert!(!reader_invoked.get());
                assert!(
                    std::fs::read_dir(&root)
                        .expect("read unchanged root")
                        .next()
                        .is_none(),
                    "fail-closed absent read must create nothing"
                );
            }
        }
    }

    #[test]
    fn unqualified_windows_other_checked_read_uses_existing_global_shared_guard() {
        for platform in [CanonicalPlatform::Windows, CanonicalPlatform::Other] {
            let root = root(&format!("unqualified-{platform:?}-present-read"));
            let store = SessionArtifactManifestStore::for_test_session_platform(
                &root,
                "session-1",
                platform,
            )
            .expect("read-only Session store");
            let mut present = basic_candidate("present-head", 'a');
            present.generation = 1;
            write_manifest_bytes(
                &store,
                &serde_json::to_vec(&present).expect("present manifest bytes"),
            );

            let observed = store
                .checked_read(Ok::<_, ()>)
                .expect("guarded read-only present Session");
            assert!(matches!(
                observed,
                ManifestLoadOutcome::Present(manifest)
                    if manifest.transition.idempotency_id == "present-head"
            ));
        }
    }

    #[test]
    fn public_manifest_store_seam_loads_an_absent_explicit_root_without_mutation() {
        let root = missing_root("missing-root");
        let store = SessionArtifactManifestStore::new(&root);

        assert_eq!(store.load(), Ok(ManifestLoadOutcome::Absent));
        assert!(!root.exists());
    }

    #[test]
    fn unqualified_begin_write_refuses_before_coordination_mutation() {
        let root = root("unqualified-begin-refusal-private-root");
        let store = SessionArtifactManifestStore::new(&root);
        let before = std::fs::read_dir(&root)
            .expect("read empty root before")
            .map(|entry| entry.expect("read before entry").file_name())
            .collect::<Vec<_>>();
        assert!(before.is_empty());

        let error = match store.begin_write() {
            Err(error) => error,
            Ok(_) => panic!("unqualified begin_write unexpectedly acquired a writer guard"),
        };

        assert_eq!(error, ManifestStoreError::NamespaceQualificationRequired);
        assert_eq!(
            std::fs::read_dir(&root)
                .expect("read empty root after")
                .map(|entry| entry.expect("read after entry").file_name())
                .collect::<Vec<_>>(),
            before
        );
        assert!(!root.join(COORDINATION_FILE_NAME).exists());
        assert!(!root.join(MANIFEST_TEMP_FILE_NAME).exists());
        assert!(!root.join(MANIFEST_FILE_NAME).exists());
        assert!(!format!("{error:?}").contains("private-root"));
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
    fn v1_to_v2_transition_proof_has_exact_digest_free_canonical_wire() {
        let proof = SessionSemanticsTransitionProofV1::v1_to_v2("session-1", "advance-floor-v2")
            .expect("valid transition proof");
        let (bytes, digest) = proof
            .canonical_bytes_and_digest()
            .expect("canonical proof bytes");
        let expected = b"{\"schema_version\":1,\"session_id\":\"session-1\",\"from\":1,\"to\":2,\"idempotency_id\":\"advance-floor-v2\",\"transition_kind\":\"session_semantics_advance\"}";

        assert_eq!(bytes, expected);
        assert_eq!(bytes.len(), 143);
        assert_eq!(
            digest.as_str(),
            "sha256:1d796c12d556471fbfca4381957a80e82f3139259c6c974a190730fa69c6b0e6"
        );
        assert_eq!(
            SessionSemanticsTransitionProofV1::from_canonical_bytes(&bytes),
            Ok(proof)
        );

        for forbidden in [
            b"{\"schema_version\":1,\"session_id\":\"session-1\",\"from\":1,\"to\":2,\"idempotency_id\":\"advance-floor-v2\",\"transition_kind\":\"session_semantics_advance\",\"proof_sha256\":\"self\"}".as_slice(),
            b"{\"schema_version\":1,\"session_id\":\"session-1\",\"from\":1,\"to\":2,\"idempotency_id\":\"advance-floor-v2\",\"transition_kind\":\"session_semantics_advance\",\"content_digest\":\"self\"}".as_slice(),
        ] {
            assert_eq!(
                SessionSemanticsTransitionProofV1::from_canonical_bytes(forbidden),
                Err(SessionSemanticsTransitionProofError::Malformed)
            );
        }
    }

    #[test]
    fn v1_wire_is_a_deterministic_golden_roundtrip() {
        let mut candidate = basic_candidate("tx-1", 'a');
        candidate.generation = 1;
        let wire = serde_json::to_string(&candidate).expect("serialize");
        assert_eq!(
            wire,
            format!(
                "{{\"schema_version\":1,\"session_id\":\"session-1\",\"session_semantics_version\":1,\"generation\":1,\"transition\":{{\"idempotency_id\":\"tx-1\",\"fingerprint\":\"sha256:{}\",\"state\":\"completed\"}},\"artifacts\":[{{\"kind\":\"original_session_audio\",\"privacy_class\":\"original_evidence\",\"managed_identity\":\"audio/original.wav\",\"availability\":{{\"unavailable\":{{\"reason\":\"retention_disabled\"}}}}}}],\"quarantine_transaction\":null}}",
                "a".repeat(64)
            )
        );
        let mut decoded: SessionArtifactManifestV1 =
            serde_json::from_str(&wire).expect("decode golden");
        validate_persisted_and_normalize(&mut decoded).expect("validate golden");
        assert_eq!(decoded, candidate);
    }

    #[test]
    fn historical_wire_defaults_floor_to_v1_and_new_candidate_wire_is_explicit() {
        let candidate = basic_candidate("tx-floor", 'f');
        let mut historical = serde_json::to_value(&candidate).expect("candidate value");
        historical
            .as_object_mut()
            .expect("manifest object")
            .remove("session_semantics_version");

        let decoded: SessionArtifactManifestV1 =
            serde_json::from_value(historical).expect("historical manifest");
        assert_eq!(
            decoded.session_semantics_version,
            super::super::session_semantics::SessionSemanticsVersion::V1
        );

        let explicit = serde_json::to_value(candidate).expect("candidate wire");
        assert_eq!(explicit["session_semantics_version"], serde_json::json!(1));
    }

    #[test]
    fn v2_candidate_requires_exact_bound_session_provenance_proof() {
        use super::V2SessionProvenanceError;

        fn assert_rejected(
            label: &str,
            candidate: SessionArtifactManifestV1,
            expected: V2SessionProvenanceError,
        ) {
            let root = root(label);
            let store = SessionArtifactManifestStore::qualified_for_test(&root)
                .expect("qualified validation fixture");
            let mut transaction = store.begin_write().expect("transaction");
            assert_eq!(
                transaction.compare_and_swap(0, candidate),
                ManifestCasOutcome::Rejected(ManifestCasRejection::Validation(
                    ManifestValidationError::InvalidV2SessionProvenance(expected)
                )),
                "v2 proof case {label}"
            );
        }

        let missing = {
            let mut candidate = v2_candidate("v2-missing-provenance");
            candidate
                .artifacts
                .retain(|artifact| artifact.kind != SessionArtifactKind::SessionProvenanceEvents);
            candidate
        };
        assert_rejected(
            "v2-missing-provenance",
            missing,
            V2SessionProvenanceError::Missing,
        );

        let duplicate = {
            let mut candidate = v2_candidate("v2-duplicate-provenance");
            candidate.artifacts.push(session_provenance());
            candidate
        };
        assert_rejected(
            "v2-duplicate-provenance",
            duplicate,
            V2SessionProvenanceError::Duplicate,
        );

        let wrong_privacy = {
            let mut candidate = v2_candidate("v2-wrong-provenance-privacy");
            candidate
                .artifacts
                .iter_mut()
                .find(|artifact| artifact.kind == SessionArtifactKind::SessionProvenanceEvents)
                .expect("provenance")
                .privacy_class = ArtifactPrivacyClass::AuditRecord;
            candidate
        };
        assert_rejected(
            "v2-wrong-provenance-privacy",
            wrong_privacy,
            V2SessionProvenanceError::PrivacyMismatch,
        );

        let unavailable = {
            let mut candidate = v2_candidate("v2-unavailable-provenance");
            candidate
                .artifacts
                .iter_mut()
                .find(|artifact| artifact.kind == SessionArtifactKind::SessionProvenanceEvents)
                .expect("provenance")
                .availability = ArtifactAvailability::Unavailable {
                reason: ArtifactUnavailableReason::NeverCaptured,
            };
            candidate
        };
        assert_rejected(
            "v2-unavailable-provenance",
            unavailable,
            V2SessionProvenanceError::Unavailable,
        );

        let residual = {
            let mut candidate = v2_candidate("v2-residual-provenance");
            candidate
                .artifacts
                .iter_mut()
                .find(|artifact| artifact.kind == SessionArtifactKind::SessionProvenanceEvents)
                .expect("provenance")
                .availability = ArtifactAvailability::Residual {
                content: content('e', 48),
                reason: ArtifactResidualReason::DeletionFailed,
            };
            candidate
        };
        assert_rejected(
            "v2-residual-provenance",
            residual,
            V2SessionProvenanceError::Residual,
        );

        let noncompleted = {
            let mut candidate = v2_candidate("v2-noncompleted-transition");
            candidate.transition.state = ManifestTransitionState::Prepared;
            candidate
        };
        assert_rejected(
            "v2-noncompleted-transition",
            noncompleted,
            V2SessionProvenanceError::TransitionNotCompleted,
        );

        let mismatched = {
            let mut candidate = v2_candidate("v2-mismatched-transition");
            candidate.transition.fingerprint = digest('f');
            candidate
        };
        assert_rejected(
            "v2-mismatched-transition",
            mismatched,
            V2SessionProvenanceError::TransitionFingerprintMismatch,
        );
    }

    #[test]
    fn load_rejects_unsupported_session_semantics_floor() {
        let root = root("unsupported-session-floor");
        let store = SessionArtifactManifestStore::new(&root);
        let mut persisted = basic_candidate("tx-unsupported-floor", 'f');
        persisted.generation = 1;
        let mut wire = serde_json::to_value(persisted).expect("manifest value");
        wire["session_semantics_version"] = serde_json::json!(3);
        write_manifest_bytes(
            &store,
            &serde_json::to_vec(&wire).expect("unsupported floor wire"),
        );

        assert_eq!(
            store.load(),
            Err(ManifestLoadError::Validation(
                ManifestValidationError::UnsupportedSessionSemanticsVersion { actual: 3 }
            ))
        );
    }

    #[cfg(unix)]
    #[test]
    fn accepted_v2_manifest_cannot_regress_to_v1() {
        let root = root("session-floor-regression");
        let store = SessionArtifactManifestStore::qualified_for_test(&root).expect("qualified");
        let mut transaction = store.begin_write().expect("transaction");
        let v2 = v2_candidate("tx-floor-v2");
        assert!(matches!(
            transaction.compare_and_swap(0, v2),
            ManifestCasOutcome::Accepted {
                manifest: SessionArtifactManifestV1 {
                    session_semantics_version:
                        super::super::session_semantics::SessionSemanticsVersion::V2,
                    ..
                },
                ..
            }
        ));

        assert_eq!(
            transaction.compare_and_swap(1, basic_candidate("tx-regression", 'd')),
            ManifestCasOutcome::Rejected(ManifestCasRejection::SessionSemanticsFloorRegression {
                current: super::super::session_semantics::SessionSemanticsVersion::V2,
                candidate: super::super::session_semantics::SessionSemanticsVersion::V1,
            })
        );
    }

    #[cfg(unix)]
    #[test]
    fn proof_before_manifest_transition_returns_actual_accepted_and_already_completed() {
        let root = root("proof-before-manifest-accepted-retry");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified Session store");
        let proof = SessionSemanticsTransitionProofV1::v1_to_v2("session-1", "advance-floor-v2")
            .expect("transition proof");
        let candidate = v2_candidate("advance-floor-v2");
        let expected_digest =
            "sha256:1d796c12d556471fbfca4381957a80e82f3139259c6c974a190730fa69c6b0e6";
        let proof_path = root.join(store.control_identities().provenance.as_str());
        let mut transaction = store.begin_write().expect("transition transaction");

        let accepted =
            transaction.advance_session_semantics_v1_to_v2(0, candidate.clone(), proof.clone());
        let accepted_manifest = match accepted {
            ManifestCasOutcome::Accepted { manifest, .. } => manifest,
            other => panic!("expected actual Accepted outcome, got {other:?}"),
        };
        assert_eq!(accepted_manifest.generation, 1);
        assert_eq!(
            accepted_manifest.transition.fingerprint.as_str(),
            expected_digest
        );
        let provenance = accepted_manifest
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == SessionArtifactKind::SessionProvenanceEvents)
            .expect("Session provenance entry");
        assert_eq!(
            provenance.managed_identity,
            store.control_identities().provenance,
            "persisted provenance inventory must name the proof that was written"
        );
        assert_eq!(
            provenance.availability,
            ArtifactAvailability::Present {
                content: ArtifactContentIdentity {
                    sha256: Sha256Digest::new(expected_digest).expect("expected digest"),
                    byte_length: 143,
                }
            }
        );
        let proof_bytes = std::fs::read(&proof_path).expect("durable proof precedes manifest");
        assert_eq!(proof_bytes.len(), 143);

        assert_eq!(
            transaction.advance_session_semantics_v1_to_v2(1, candidate, proof),
            ManifestCasOutcome::AlreadyCompleted {
                manifest: accepted_manifest.clone(),
            }
        );
        assert_eq!(
            std::fs::read(&proof_path).expect("exact proof remains one record"),
            proof_bytes
        );
        drop(transaction);
        let reopened = match store.load().expect("reopen persisted manifest") {
            ManifestLoadOutcome::Present(manifest) => manifest,
            ManifestLoadOutcome::Absent => panic!("accepted manifest disappeared"),
        };
        let reopened_provenance = reopened
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == SessionArtifactKind::SessionProvenanceEvents)
            .expect("reopened Session provenance entry");
        assert_eq!(
            reopened_provenance.managed_identity,
            store.control_identities().provenance
        );
    }

    #[cfg(unix)]
    #[test]
    fn addressed_generic_cas_cannot_install_v2_without_owning_proof_bytes() {
        let root = root("addressed-generic-v2-bypass");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified addressed store");
        let manifest_path = store.manifest_path();
        let temporary_path = store.temporary_path();
        let provenance_path = root.join(store.control_identities().provenance.as_str());
        let mut transaction = store.begin_write().expect("addressed transaction");

        assert_eq!(
            transaction.compare_and_swap(0, v2_candidate("advance-only-with-proof")),
            ManifestCasOutcome::Rejected(ManifestCasRejection::TransitionProofRequired)
        );
        assert!(!manifest_path.exists());
        assert!(!temporary_path.exists());
        assert!(!provenance_path.exists());

        assert!(matches!(
            transaction.advance_session_semantics_v1_to_v2(
                0,
                v2_candidate("advance-only-with-proof"),
                SessionSemanticsTransitionProofV1::v1_to_v2(
                    "session-1",
                    "advance-only-with-proof",
                )
                .expect("proof-owning transition"),
            ),
            ManifestCasOutcome::Accepted { .. }
        ));
    }

    /// audio-graph-3b53 B4 is KNOWN OPEN and this test pins it as a REFUSAL, not
    /// as working behaviour.
    ///
    /// Once an addressed Session has a committed V2 head it cannot record a later
    /// generation: the transition-proof gate keys on the candidate's floor rather
    /// than on whether this call performs the advance. That is a liveness wedge.
    ///
    /// Re-keying the gate on "this call advances the head" was implemented and
    /// reverted, because relaxing it for an already-advanced head lets the generic
    /// CAS install a V2 candidate carrying a FORGED provenance entry:
    /// `validate_v2_session_provenance` only compares the candidate's own
    /// `transition.fingerprint` against its own provenance entry's `content.sha256`,
    /// which never references the durable proof record. A caller controlling both
    /// fields satisfies it while the real proof sits untouched.
    ///
    /// So this asserts the wedge deliberately. Re-keying is blocked until the
    /// candidate's provenance entry can be proven to reference the durable proof;
    /// when that binding exists, this test should be inverted, not deleted.
    #[cfg(unix)]
    #[test]
    fn committed_v2_head_refuses_later_generations_until_proof_binding_exists() {
        let root = root("committed-v2-second-generation");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified addressed store");
        let transition_id = "advance-then-export";
        let mut transaction = store.begin_write().expect("addressed transaction");
        let advanced = match transaction.advance_session_semantics_v1_to_v2(
            0,
            v2_candidate(transition_id),
            SessionSemanticsTransitionProofV1::v1_to_v2("session-1", transition_id)
                .expect("transition proof"),
        ) {
            ManifestCasOutcome::Accepted { manifest, .. } => manifest,
            other => panic!("expected the advance to be accepted, got {other:?}"),
        };
        assert_eq!(advanced.generation, 1);

        let export = SessionArtifactEntry {
            kind: SessionArtifactKind::MaterializedNotes,
            privacy_class: ArtifactPrivacyClass::DerivedSessionMemory,
            managed_identity: identity("exports/notes.md"),
            availability: ArtifactAvailability::Present {
                content: content('c', 64),
            },
        };
        let mut second = advanced.clone();
        second.generation = 0;
        second.transition.idempotency_id = "export-notes-1".to_owned();
        second.artifacts.push(export);

        // The wedge: a same-floor generation that transitions nothing is still
        // refused for want of proof bytes this call does not own.
        assert_eq!(
            transaction.compare_and_swap(1, second),
            ManifestCasOutcome::Rejected(ManifestCasRejection::TransitionProofRequired),
            "B4 is open: an advanced Session still cannot record a later generation"
        );

        // The property the wedge is protecting, and the reason it cannot simply be
        // relaxed: nothing here binds a candidate's provenance entry to the
        // durable proof record.
        assert_eq!(
            transaction.compare_and_swap(1, basic_candidate("tx-post-v2-regression", 'd')),
            ManifestCasOutcome::Rejected(ManifestCasRejection::SessionSemanticsFloorRegression {
                current: super::super::session_semantics::SessionSemanticsVersion::V2,
                candidate: super::super::session_semantics::SessionSemanticsVersion::V1,
            }),
            "a V1 candidate against a V2 head is still a floor regression"
        );
    }

    #[cfg(unix)]
    #[test]
    fn proof_conflict_and_indeterminate_prevent_manifest_mutation_then_retry_converges() {
        let conflict_root = root("proof-conflict-before-manifest");
        let conflict_store =
            SessionArtifactManifestStore::qualified_for_test_session(&conflict_root, "session-1")
                .expect("qualified conflict store");
        let conflict_proof_path =
            conflict_root.join(conflict_store.control_identities().provenance.as_str());
        let conflict_temp_path = conflict_store.temporary_path();
        let conflict_manifest_path = conflict_store.manifest_path();
        std::fs::write(&conflict_proof_path, b"foreign proof bytes")
            .expect("seed conflicting proof");
        let mut conflict_transaction = conflict_store.begin_write().expect("conflict transaction");
        assert_eq!(
            conflict_transaction.advance_session_semantics_v1_to_v2(
                0,
                v2_candidate("advance-conflict"),
                SessionSemanticsTransitionProofV1::v1_to_v2("session-1", "advance-conflict",)
                    .expect("conflict proof input"),
            ),
            ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(
                CanonicalDurabilityRejection::ImmutableExactConflict,
            ))
        );
        assert_eq!(
            std::fs::read(&conflict_proof_path).expect("conflict remains immutable"),
            b"foreign proof bytes"
        );
        assert!(!conflict_manifest_path.exists());
        assert!(!conflict_temp_path.exists());
        drop(conflict_transaction);

        let retry_root = root("proof-indeterminate-retry");
        let retry_store =
            SessionArtifactManifestStore::qualified_for_test_session(&retry_root, "session-1")
                .expect("qualified retry store");
        let retry_proof_path =
            retry_root.join(retry_store.control_identities().provenance.as_str());
        let retry_temp_path = retry_store.temporary_path();
        let retry_manifest_path = retry_store.manifest_path();
        let retry_candidate = v2_candidate("advance-proof-retry");
        let retry_proof =
            SessionSemanticsTransitionProofV1::v1_to_v2("session-1", "advance-proof-retry")
                .expect("retry proof");
        let retry_proof_length = retry_proof
            .canonical_bytes_and_digest()
            .expect("retry proof bytes")
            .0
            .len();
        let mut first_transaction = retry_store.begin_write().expect("first retry transaction");
        assert!(matches!(
            first_transaction.advance_session_semantics_v1_to_v2_with_faults(
                0,
                retry_candidate.clone(),
                retry_proof.clone(),
                None,
                Some(super::super::canonical_durability::CanonicalDurabilityStage::Write),
                None,
            ),
            ManifestCasOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: super::super::canonical_durability::CanonicalDurabilityStage::Write,
                ..
            })
        ));
        let partial = std::fs::read(&retry_proof_path).expect("partial proof remains");
        assert!(!partial.is_empty());
        assert!(partial.len() < retry_proof_length);
        assert!(!retry_manifest_path.exists());
        assert!(retry_temp_path.exists());
        drop(first_transaction);

        let mut restarted = retry_store.begin_write().expect("restarted transaction");
        assert!(matches!(
            restarted.advance_session_semantics_v1_to_v2(0, retry_candidate, retry_proof,),
            ManifestCasOutcome::Accepted { .. }
        ));
        assert_eq!(
            std::fs::read(retry_proof_path)
                .expect("strict-prefix proof converges")
                .len(),
            retry_proof_length
        );
    }

    #[cfg(unix)]
    #[test]
    fn stale_existing_head_rejects_different_transition_before_proof_mutation() {
        let root = root("stale-existing-head-preflight");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified Session store");
        let manifest_path = store.manifest_path();
        let temporary_path = store.temporary_path();
        let provenance_path = root.join(store.control_identities().provenance.as_str());
        let mut transaction = store.begin_write().expect("existing-head transaction");
        assert!(matches!(
            transaction.compare_and_swap(0, basic_candidate("seed-v1", 'a')),
            ManifestCasOutcome::Accepted { .. }
        ));
        let head_before = std::fs::read(&manifest_path).expect("read existing v1 head");

        assert_eq!(
            transaction.advance_session_semantics_v1_to_v2(
                0,
                v2_candidate("wrong-stale-transition"),
                SessionSemanticsTransitionProofV1::v1_to_v2("session-1", "wrong-stale-transition",)
                    .expect("different stale proof"),
            ),
            ManifestCasOutcome::Rejected(ManifestCasRejection::GenerationConflict {
                expected: 0,
                actual: 1,
            })
        );
        assert_eq!(
            std::fs::read(&manifest_path).expect("existing head remains unchanged"),
            head_before
        );
        assert!(!temporary_path.exists());
        assert!(!provenance_path.exists());

        assert!(matches!(
            transaction.advance_session_semantics_v1_to_v2(
                1,
                v2_candidate("correct-transition"),
                SessionSemanticsTransitionProofV1::v1_to_v2("session-1", "correct-transition",)
                    .expect("correct proof"),
            ),
            ManifestCasOutcome::Accepted { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn partial_proof_orphan_cannot_be_claimed_by_another_proof_with_common_prefix() {
        let root = root("partial-proof-cross-claim");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified Session store");
        let manifest_path = store.manifest_path();
        let temporary_path = store.temporary_path();
        let provenance_path = root.join(store.control_identities().provenance.as_str());
        let first_id = "advance-shared-a";
        let other_id = "advance-shared-b";

        let mut first = store.begin_write().expect("first proof transaction");
        assert!(matches!(
            first.advance_session_semantics_v1_to_v2_with_faults(
                0,
                v2_candidate(first_id),
                SessionSemanticsTransitionProofV1::v1_to_v2("session-1", first_id)
                    .expect("first proof"),
                None,
                Some(super::super::canonical_durability::CanonicalDurabilityStage::Write),
                None,
            ),
            ManifestCasOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: super::super::canonical_durability::CanonicalDurabilityStage::Write,
                ..
            })
        ));
        let first_partial = std::fs::read(&provenance_path).expect("partial first proof");
        assert!(!first_partial.is_empty());
        assert!(!manifest_path.exists());
        let first_intent = std::fs::read(&temporary_path).expect("durable first transition intent");
        drop(first);

        let mut other = store.begin_write().expect("other proof transaction");
        assert_eq!(
            other.advance_session_semantics_v1_to_v2(
                0,
                v2_candidate(other_id),
                SessionSemanticsTransitionProofV1::v1_to_v2("session-1", other_id)
                    .expect("other proof"),
            ),
            ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(
                CanonicalDurabilityRejection::SnapshotTempAlreadyExists,
            ))
        );
        assert_eq!(
            std::fs::read(&provenance_path).expect("first partial remains unchanged"),
            first_partial
        );
        assert_eq!(
            std::fs::read(&temporary_path).expect("first intent remains unchanged"),
            first_intent
        );
        assert!(!manifest_path.exists());
        drop(other);

        let mut correct = store.begin_write().expect("correct proof transaction");
        assert!(matches!(
            correct.advance_session_semantics_v1_to_v2(
                0,
                v2_candidate(first_id),
                SessionSemanticsTransitionProofV1::v1_to_v2("session-1", first_id)
                    .expect("correct proof"),
            ),
            ManifestCasOutcome::Accepted { .. }
        ));
        assert!(!temporary_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn exact_transition_restarts_after_post_create_empty_proof_final() {
        let root = root("empty-proof-final-restart");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified Session store");
        let manifest_path = store.manifest_path();
        let temporary_path = store.temporary_path();
        let provenance_path = root.join(store.control_identities().provenance.as_str());
        let transition_id = "advance-empty-proof";
        let candidate = v2_candidate(transition_id);
        let proof = SessionSemanticsTransitionProofV1::v1_to_v2("session-1", transition_id)
            .expect("transition proof");

        let mut first = store.begin_write().expect("first transition transaction");
        assert!(matches!(
            first.advance_session_semantics_v1_to_v2_with_faults(
                0,
                candidate.clone(),
                proof.clone(),
                None,
                Some(super::super::canonical_durability::CanonicalDurabilityStage::PostCreate),
                None,
            ),
            ManifestCasOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: super::super::canonical_durability::CanonicalDurabilityStage::PostCreate,
                ..
            })
        ));
        assert_eq!(
            std::fs::read(&provenance_path).expect("empty proof final remains"),
            b""
        );
        assert!(!manifest_path.exists());
        drop(first);

        let mut restarted = store
            .begin_write()
            .expect("restarted transition transaction");
        assert!(matches!(
            restarted.advance_session_semantics_v1_to_v2(0, candidate, proof),
            ManifestCasOutcome::Accepted { .. }
        ));
        assert!(manifest_path.exists());
        assert!(!temporary_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn exact_transition_restarts_after_one_byte_proof_final() {
        let root = root("one-byte-proof-final-restart");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified Session store");
        let temporary_path = store.temporary_path();
        let provenance_path = root.join(store.control_identities().provenance.as_str());
        let transition_id = "advance-one-byte-proof";
        let candidate = v2_candidate(transition_id);
        let proof = SessionSemanticsTransitionProofV1::v1_to_v2("session-1", transition_id)
            .expect("transition proof");
        let expected_proof = proof
            .canonical_bytes_and_digest()
            .expect("canonical proof")
            .0;

        let mut first = store.begin_write().expect("first transition transaction");
        assert!(matches!(
            first.advance_session_semantics_v1_to_v2_with_faults(
                0,
                candidate.clone(),
                proof.clone(),
                None,
                Some(super::super::canonical_durability::CanonicalDurabilityStage::Write),
                None,
            ),
            ManifestCasOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: super::super::canonical_durability::CanonicalDurabilityStage::Write,
                ..
            })
        ));
        assert_eq!(
            std::fs::read(&provenance_path).expect("one-byte proof final remains"),
            &expected_proof[..1]
        );
        assert!(temporary_path.exists());
        drop(first);

        let mut restarted = store
            .begin_write()
            .expect("restarted transition transaction");
        assert!(matches!(
            restarted.advance_session_semantics_v1_to_v2(0, candidate, proof),
            ManifestCasOutcome::Accepted { .. }
        ));
        assert_eq!(
            std::fs::read(&provenance_path).expect("complete proof final"),
            expected_proof
        );
        assert!(!temporary_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn transition_intent_stage_faults_are_honest_and_exact_retry_converges() {
        use super::super::canonical_durability::CanonicalDurabilityStage;

        for stage in [
            CanonicalDurabilityStage::CreateNew,
            CanonicalDurabilityStage::PostCreate,
            CanonicalDurabilityStage::Write,
            CanonicalDurabilityStage::Flush,
            CanonicalDurabilityStage::ProtectTemp,
            CanonicalDurabilityStage::FileSync,
            CanonicalDurabilityStage::ParentSync,
        ] {
            let root = root(&format!("transition-intent-cut-{stage:?}"));
            let store =
                SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
                    .expect("qualified Session store");
            let manifest_path = store.manifest_path();
            let temporary_path = store.temporary_path();
            let provenance_path = root.join(store.control_identities().provenance.as_str());
            let transition_id = format!("advance-intent-{stage:?}");
            let candidate = v2_candidate(&transition_id);
            let proof = SessionSemanticsTransitionProofV1::v1_to_v2("session-1", &transition_id)
                .expect("transition proof");

            let mut first = store.begin_write().expect("first transition transaction");
            let outcome = first.advance_session_semantics_v1_to_v2_with_faults(
                0,
                candidate.clone(),
                proof.clone(),
                Some(stage),
                None,
                None,
            );
            if stage == CanonicalDurabilityStage::CreateNew {
                assert!(matches!(
                    outcome,
                    ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(
                        CanonicalDurabilityRejection::IoFailedBeforeMutation {
                            stage: CanonicalDurabilityStage::CreateNew,
                            ..
                        }
                    ))
                ));
                assert!(!temporary_path.exists());
            } else {
                assert!(matches!(
                    outcome,
                    ManifestCasOutcome::DurabilityIndeterminate(
                        CanonicalDurabilityIndeterminate {
                            stage: actual_stage,
                            ..
                        }
                    ) if actual_stage == stage
                ));
                assert!(temporary_path.exists());
            }
            assert!(!provenance_path.exists());
            assert!(!manifest_path.exists());
            drop(first);

            let mut restarted = store
                .begin_write()
                .expect("restarted transition transaction");
            assert!(matches!(
                restarted.advance_session_semantics_v1_to_v2(0, candidate, proof),
                ManifestCasOutcome::Accepted { .. }
            ));
            assert!(manifest_path.exists());
            assert!(!temporary_path.exists());
        }
    }

    /// A refused proof create leaves the intent temporary durably staged. The
    /// outcome must never borrow the pre-mutation durability vocabulary for
    /// that state, and it must name the staged intent the caller still owns.
    #[cfg(unix)]
    #[test]
    fn refused_proof_create_after_durable_intent_is_never_reported_as_unstaged() {
        use super::super::canonical_durability::CanonicalDurabilityStage;

        let root = root("proof-create-refusal-after-durable-intent");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified Session store");
        let manifest_path = store.manifest_path();
        let temporary_path = store.temporary_path();
        let provenance_path = root.join(store.control_identities().provenance.as_str());
        let transition_id = "advance-refused-proof-create";
        let candidate = v2_candidate(transition_id);
        let proof = SessionSemanticsTransitionProofV1::v1_to_v2("session-1", transition_id)
            .expect("transition proof");
        let proof_digest = proof
            .canonical_bytes_and_digest()
            .expect("canonical proof")
            .1;
        let staged_intent_key = recovery_key(&proof_digest);

        let mut first = store.begin_write().expect("first transition transaction");
        let outcome = first.advance_session_semantics_v1_to_v2_with_faults(
            0,
            candidate.clone(),
            proof.clone(),
            None,
            Some(CanonicalDurabilityStage::CreateNew),
            None,
        );

        // The intent temporary is already file- and parent-synced here, and the
        // proof create definitively never happened.
        assert!(temporary_path.exists());
        assert!(!provenance_path.exists());
        assert!(!manifest_path.exists());

        match &outcome {
            ManifestCasOutcome::Rejected(
                ManifestCasRejection::TransitionProofRefusedAfterIntentStaged {
                    rejection,
                    recovery_key: staged_key,
                },
            ) => {
                assert!(matches!(
                    rejection,
                    CanonicalDurabilityRejection::IoFailedBeforeMutation {
                        stage: CanonicalDurabilityStage::CreateNew,
                        ..
                    }
                ));
                assert_eq!(*staged_key, staged_intent_key);
            }
            other => panic!("post-staging proof refusal misclassified as {other:?}"),
        }
        drop(first);

        // The staged temporary is real: the public generation CAS cannot get
        // past it for any other candidate.
        let mut wedged = store.begin_write().expect("wedged transaction");
        assert_eq!(
            wedged.compare_and_swap(0, basic_candidate("wedged-v1", 'a')),
            ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(
                CanonicalDurabilityRejection::SnapshotTempAlreadyExists,
            ))
        );
        assert!(temporary_path.exists());
        assert!(!manifest_path.exists());
        drop(wedged);

        // One of the two reachable escapes is a byte-exact replay of the
        // candidate and proof the refusal named, through the public transition
        // entry point. The other is `abandon_staged_transition`, pinned by
        // `abandoned_transition_unwedges_a_different_candidate`.
        let mut resumed = store.begin_write().expect("resumed transaction");
        assert!(matches!(
            resumed.advance_session_semantics_v1_to_v2(0, candidate, proof),
            ManifestCasOutcome::Accepted { .. }
        ));
        assert!(manifest_path.exists());
        assert!(!temporary_path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn transition_proof_stage_faults_are_honest_and_exact_retry_converges() {
        use super::super::canonical_durability::CanonicalDurabilityStage;

        for stage in [
            CanonicalDurabilityStage::CreateNew,
            CanonicalDurabilityStage::Flush,
            CanonicalDurabilityStage::ProtectTemp,
            CanonicalDurabilityStage::FileSync,
            CanonicalDurabilityStage::ParentSync,
        ] {
            let root = root(&format!("transition-proof-cut-{stage:?}"));
            let store =
                SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
                    .expect("qualified Session store");
            let manifest_path = store.manifest_path();
            let temporary_path = store.temporary_path();
            let provenance_path = root.join(store.control_identities().provenance.as_str());
            let transition_id = format!("advance-proof-{stage:?}");
            let candidate = v2_candidate(&transition_id);
            let proof = SessionSemanticsTransitionProofV1::v1_to_v2("session-1", &transition_id)
                .expect("transition proof");
            let (expected_proof, proof_digest) = proof
                .canonical_bytes_and_digest()
                .expect("canonical proof bytes");
            let staged_intent_key = recovery_key(&proof_digest);

            let mut first = store.begin_write().expect("first transition transaction");
            let outcome = first.advance_session_semantics_v1_to_v2_with_faults(
                0,
                candidate.clone(),
                proof.clone(),
                None,
                Some(stage),
                None,
            );
            if stage == CanonicalDurabilityStage::CreateNew {
                match &outcome {
                    ManifestCasOutcome::Rejected(
                        ManifestCasRejection::TransitionProofRefusedAfterIntentStaged {
                            rejection,
                            recovery_key: staged_key,
                        },
                    ) => {
                        assert!(matches!(
                            rejection,
                            CanonicalDurabilityRejection::IoFailedBeforeMutation {
                                stage: CanonicalDurabilityStage::CreateNew,
                                ..
                            }
                        ));
                        assert_eq!(*staged_key, staged_intent_key);
                    }
                    other => panic!("refused proof create misclassified as {other:?}"),
                }
                assert!(!provenance_path.exists());
            } else {
                assert!(matches!(
                    &outcome,
                    ManifestCasOutcome::DurabilityIndeterminate(
                        CanonicalDurabilityIndeterminate {
                            stage: actual_stage,
                            recovery_key: actual_key,
                            ..
                        }
                    ) if *actual_stage == stage && *actual_key == staged_intent_key
                ));
                assert_eq!(
                    std::fs::read(&provenance_path).expect("unsynced proof bytes remain"),
                    expected_proof
                );
            }
            assert!(temporary_path.exists());
            assert!(!manifest_path.exists());
            drop(first);

            let mut restarted = store
                .begin_write()
                .expect("restarted transition transaction");
            assert!(matches!(
                restarted.advance_session_semantics_v1_to_v2(0, candidate, proof),
                ManifestCasOutcome::Accepted { .. }
            ));
            assert_eq!(
                std::fs::read(&provenance_path).expect("converged exact proof"),
                expected_proof
            );
            assert!(manifest_path.exists());
            assert!(!temporary_path.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn every_strict_proof_prefix_is_bound_to_its_exact_durable_intent() {
        use super::super::canonical_durability::CanonicalDurabilityStage;

        let first_id = "advance-prefix-owner-a";
        let other_id = "advance-prefix-owner-b";
        let first_proof = SessionSemanticsTransitionProofV1::v1_to_v2("session-1", first_id)
            .expect("first proof");
        let first_bytes = first_proof
            .canonical_bytes_and_digest()
            .expect("first proof bytes")
            .0;
        let identity_boundary =
            SessionSemanticsTransitionProofV1::recovery_identity_prefix_len(&first_bytes)
                .expect("proof identity boundary");

        for prefix_length in [0, 1, identity_boundary - 1, first_bytes.len() - 1] {
            let root = root(&format!("proof-prefix-intent-binding-{prefix_length}"));
            let store =
                SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
                    .expect("qualified Session store");
            let manifest_path = store.manifest_path();
            let temporary_path = store.temporary_path();
            let provenance_path = root.join(store.control_identities().provenance.as_str());

            let mut first = store.begin_write().expect("first transition transaction");
            assert!(matches!(
                first.advance_session_semantics_v1_to_v2_with_faults(
                    0,
                    v2_candidate(first_id),
                    first_proof.clone(),
                    None,
                    Some(CanonicalDurabilityStage::PostCreate),
                    None,
                ),
                ManifestCasOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                    stage: CanonicalDurabilityStage::PostCreate,
                    ..
                })
            ));
            std::fs::write(&provenance_path, &first_bytes[..prefix_length])
                .expect("materialize exact crash prefix");
            let first_intent = std::fs::read(&temporary_path).expect("read exact durable intent");
            drop(first);

            let mut other = store.begin_write().expect("different proof transaction");
            assert!(matches!(
                other.advance_session_semantics_v1_to_v2(
                    0,
                    v2_candidate(other_id),
                    SessionSemanticsTransitionProofV1::v1_to_v2("session-1", other_id)
                        .expect("different proof"),
                ),
                ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(
                    CanonicalDurabilityRejection::SnapshotTempAlreadyExists
                        | CanonicalDurabilityRejection::ImmutableExactConflict
                ))
            ));
            assert_eq!(
                std::fs::read(&provenance_path).expect("owner proof prefix remains"),
                &first_bytes[..prefix_length]
            );
            assert_eq!(
                std::fs::read(&temporary_path).expect("owner intent remains"),
                first_intent
            );
            assert!(!manifest_path.exists());
            drop(other);

            let mut exact = store
                .begin_write()
                .expect("exact proof restart transaction");
            assert!(matches!(
                exact.advance_session_semantics_v1_to_v2(
                    0,
                    v2_candidate(first_id),
                    first_proof.clone(),
                ),
                ManifestCasOutcome::Accepted { .. }
            ));
            assert_eq!(
                std::fs::read(&provenance_path).expect("complete exact proof"),
                first_bytes
            );
            assert!(manifest_path.exists());
            assert!(!temporary_path.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn manifest_cas_consumes_staged_intent_and_restarts_every_install_cut() {
        use super::super::canonical_durability::CanonicalDurabilityStage;

        // `Write` is deliberately absent: the install resumes a complete
        // temporary, so `already_written == bytes.len()` leaves no remaining
        // byte for the injected write cut to sever.
        for stage in [
            CanonicalDurabilityStage::CreateNew,
            CanonicalDurabilityStage::Flush,
            CanonicalDurabilityStage::ProtectTemp,
            CanonicalDurabilityStage::FileSync,
            CanonicalDurabilityStage::Rename,
            CanonicalDurabilityStage::ParentSync,
        ] {
            let root = root(&format!("staged-intent-cas-cut-{stage:?}"));
            let store =
                SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
                    .expect("qualified Session store");
            let manifest_path = store.manifest_path();
            let temporary_path = store.temporary_path();
            let provenance_path = root.join(store.control_identities().provenance.as_str());
            let transition_id = format!("advance-cas-{stage:?}");
            let candidate = v2_candidate(&transition_id);
            let proof = SessionSemanticsTransitionProofV1::v1_to_v2("session-1", &transition_id)
                .expect("transition proof");
            let expected_proof = proof
                .canonical_bytes_and_digest()
                .expect("canonical proof")
                .0;

            let mut first = store.begin_write().expect("first transition transaction");
            let first_outcome = first.advance_session_semantics_v1_to_v2_with_faults(
                0,
                candidate.clone(),
                proof.clone(),
                None,
                None,
                Some(stage),
            );
            if stage == CanonicalDurabilityStage::CreateNew {
                assert!(matches!(
                    first_outcome,
                    ManifestCasOutcome::Rejected(
                        ManifestCasRejection::ManifestInstallRefusedAfterProofAndIntentDurable {
                            rejection: CanonicalDurabilityRejection::IoFailedBeforeMutation {
                                stage: CanonicalDurabilityStage::CreateNew,
                                ..
                            },
                            ..
                        }
                    )
                ));
            } else {
                assert!(matches!(
                    first_outcome,
                    ManifestCasOutcome::DurabilityIndeterminate(
                        CanonicalDurabilityIndeterminate {
                            stage: actual_stage,
                            ..
                        }
                    ) if actual_stage == stage
                ));
            }
            assert_eq!(
                std::fs::read(&provenance_path).expect("durable proof before manifest cut"),
                expected_proof
            );
            if stage == CanonicalDurabilityStage::ParentSync {
                assert!(manifest_path.exists());
                assert!(!temporary_path.exists());
            } else {
                assert!(!manifest_path.exists());
                assert!(temporary_path.exists());
            }
            drop(first);

            let mut restarted = store
                .begin_write()
                .expect("restarted transition transaction");
            let restart = restarted.advance_session_semantics_v1_to_v2(0, candidate, proof);
            if stage == CanonicalDurabilityStage::ParentSync {
                assert!(matches!(
                    restart,
                    ManifestCasOutcome::AlreadyCompleted { .. }
                ));
            } else {
                assert!(matches!(restart, ManifestCasOutcome::Accepted { .. }));
            }
            assert!(manifest_path.exists());
            assert!(!temporary_path.exists());
        }
    }

    #[cfg(unix)]
    #[test]
    fn manifest_indeterminate_preserves_preflighted_proof_and_retry() {
        let cut_root = root("proof-orphan-manifest-indeterminate");
        let cut_store =
            SessionArtifactManifestStore::qualified_for_test_session(&cut_root, "session-1")
                .expect("qualified cut store");
        let cut_proof_path = cut_root.join(cut_store.control_identities().provenance.as_str());
        let cut_temp_path = cut_store.temporary_path();
        let cut_manifest_path = cut_store.manifest_path();
        let cut_candidate = v2_candidate("advance-manifest-cut");
        let cut_proof =
            SessionSemanticsTransitionProofV1::v1_to_v2("session-1", "advance-manifest-cut")
                .expect("manifest-cut proof");
        let cut_proof_length = cut_proof
            .canonical_bytes_and_digest()
            .expect("manifest-cut proof bytes")
            .0
            .len();
        let mut cut_transaction = cut_store.begin_write().expect("cut transaction");
        assert!(matches!(
            cut_transaction.advance_session_semantics_v1_to_v2_with_faults(
                0,
                cut_candidate.clone(),
                cut_proof.clone(),
                None,
                None,
                Some(super::super::canonical_durability::CanonicalDurabilityStage::FileSync),
            ),
            ManifestCasOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: super::super::canonical_durability::CanonicalDurabilityStage::FileSync,
                ..
            })
        ));
        assert_eq!(
            std::fs::read(&cut_proof_path)
                .expect("durable proof remains after manifest cut")
                .len(),
            cut_proof_length
        );
        assert!(!cut_manifest_path.exists());
        assert!(cut_temp_path.exists());
        assert!(matches!(
            cut_transaction.advance_session_semantics_v1_to_v2(0, cut_candidate, cut_proof,),
            ManifestCasOutcome::Accepted { .. }
        ));
        assert!(cut_manifest_path.exists());
        assert!(!cut_temp_path.exists());
    }

    #[test]
    fn windows_other_session_transition_refuses_before_any_control_mutation() {
        for platform in [CanonicalPlatform::Windows, CanonicalPlatform::Other] {
            let root = root(&format!("{platform:?}-transition-refusal"));
            let store = SessionArtifactManifestStore::for_test_session_platform(
                &root,
                "session-1",
                platform,
            )
            .expect("addressed read-only store");
            let identities = store.control_identities().clone();

            assert!(matches!(
                store.begin_write(),
                Err(ManifestStoreError::NamespaceQualificationRequired)
            ));
            for identity in [
                identities.manifest,
                identities.temporary,
                identities.provenance,
                identities.coordination,
            ] {
                assert!(!root.join(identity.as_str()).exists());
            }
        }
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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn cc9a_native_qualified_initial_cas_has_parent_barrier() {
        let root = root("production-qualified-initial");
        let store = SessionArtifactManifestStore::qualified_existing_root(&root)
            .expect("qualify existing live manifest root");
        let mut transaction = store.begin_write().expect("qualified transaction");

        assert!(matches!(
            transaction.compare_and_swap(0, basic_candidate("production-tx-1", 'a')),
            ManifestCasOutcome::Accepted {
                manifest: SessionArtifactManifestV1 { generation: 1, .. },
                durability: CanonicalDurabilityReceipt {
                    mutation: CanonicalMutation::InitialSnapshotInstall,
                    barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
                },
            }
        ));
        drop(transaction);
        assert!(matches!(
            store.load().expect("load qualified manifest"),
            ManifestLoadOutcome::Present(manifest) if manifest.generation == 1
        ));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn cc9a_native_qualified_replacement_refuses_foreign_open_head_before_temp_creation() {
        let root = root("production-qualified-replacement");
        let store = SessionArtifactManifestStore::qualified_existing_root(&root)
            .expect("qualify existing live manifest root");
        {
            let mut transaction = store.begin_write().expect("initial transaction");
            assert!(matches!(
                transaction.compare_and_swap(0, basic_candidate("production-tx-1", 'a')),
                ManifestCasOutcome::Accepted { .. }
            ));
            assert!(matches!(
                transaction.compare_and_swap(1, basic_candidate("production-tx-2", 'b')),
                ManifestCasOutcome::Accepted {
                    manifest: SessionArtifactManifestV1 { generation: 2, .. },
                    durability: CanonicalDurabilityReceipt {
                        mutation: CanonicalMutation::SnapshotReplacement,
                        barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
                    },
                }
            ));
        }

        let mut raced = store.begin_write().expect("open exact generation-two head");
        let manifest_path = store.manifest_path();
        let displaced_path = root.join("manifest.displaced");
        let validated_bytes = std::fs::read(&manifest_path).expect("read validated head bytes");
        std::fs::rename(&manifest_path, &displaced_path).expect("displace validated head object");
        std::fs::write(&manifest_path, &validated_bytes)
            .expect("install byte-identical foreign head object");

        assert_eq!(
            raced.compare_and_swap(2, basic_candidate("production-tx-3", 'c')),
            ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(
                CanonicalDurabilityRejection::IdentityChanged,
            ))
        );
        assert!(!root.join(MANIFEST_TEMP_FILE_NAME).exists());
        assert_eq!(
            std::fs::read(manifest_path).expect("foreign head retained"),
            validated_bytes
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn cc9a_native_windows_qualified_existing_root_refuses_before_manifest_coordination_or_temp_mutation()
     {
        fn root_entries(root: &Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
            let mut entries = std::fs::read_dir(root)
                .expect("read fixture root")
                .map(|entry| {
                    let entry = entry.expect("read fixture entry");
                    (
                        entry.file_name(),
                        std::fs::read(entry.path()).expect("read fixture entry bytes"),
                    )
                })
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            entries
        }

        let root = root("cc9a-native-windows-production-refusal");
        std::fs::write(root.join("sentinel.bin"), b"entry-identical").expect("write root sentinel");
        let before_entries = root_entries(&root);
        let error = match SessionArtifactManifestStore::qualified_existing_root(&root) {
            Err(error) => error,
            Ok(_) => panic!("Windows production qualification unexpectedly succeeded"),
        };

        assert_eq!(
            error,
            ManifestStoreError::Qualification(
                CanonicalFilesystemQualificationError::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Windows,
                },
            )
        );
        let after_entries = root_entries(&root);
        assert_eq!(after_entries, before_entries);
        let manifest_path = root.join(MANIFEST_FILE_NAME);
        assert!(!root.join(COORDINATION_FILE_NAME).exists());
        assert!(!root.join(MANIFEST_TEMP_FILE_NAME).exists());
        assert!(!manifest_path.exists());
    }

    #[test]
    fn algorithm_qualification_and_windows_policy_refusal_are_explicitly_separate() {
        let algorithm_root = root("algorithm-qualified");
        let algorithm_store = SessionArtifactManifestStore::qualified_for_algorithm_test_platform(
            &algorithm_root,
            CanonicalPlatform::Windows,
        )
        .expect("Windows-injected synthetic algorithm-qualified store");
        let mut algorithm_write = algorithm_store.begin_write().expect("algorithm write");
        assert!(matches!(
            algorithm_write.compare_and_swap(0, basic_candidate("algorithm-tx-1", 'a')),
            ManifestCasOutcome::Accepted {
                manifest: SessionArtifactManifestV1 { generation: 1, .. },
                ..
            }
        ));
        assert!(matches!(
            algorithm_write.compare_and_swap(1, basic_candidate("algorithm-tx-2", 'b')),
            ManifestCasOutcome::Accepted {
                manifest: SessionArtifactManifestV1 { generation: 2, .. },
                ..
            }
        ));
        drop(algorithm_write);

        let windows_root = root("windows-policy-unqualified");
        let windows_store = SessionArtifactManifestStore::for_test_platform(
            &windows_root,
            CanonicalPlatform::Windows,
        );
        assert!(windows_store.qualification.is_none());
        assert!(matches!(
            windows_store.begin_write(),
            Err(ManifestStoreError::NamespaceQualificationRequired)
        ));
        assert!(!windows_root.join(COORDINATION_FILE_NAME).exists());
        assert!(!windows_store.manifest_path().exists());
        assert!(!windows_root.join(MANIFEST_TEMP_FILE_NAME).exists());
    }

    #[test]
    fn stale_and_overflow_generations_refuse_without_snapshot_mutation() {
        let root = root("generation-refusal");
        let store =
            SessionArtifactManifestStore::qualified_for_test(&root).expect("qualified fixture");
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

        assert!(matches!(
            store.begin_write(),
            Err(ManifestStoreError::NamespaceQualificationRequired)
        ));
        assert!(!store.root.join(COORDINATION_FILE_NAME).exists());
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
        let exact_store = SessionArtifactManifestStore::qualified_for_test(&exact_root)
            .expect("qualified exact-size fixture");
        let mut exact_transaction = exact_store.begin_write().expect("exact transaction");
        assert!(matches!(
            exact_transaction
                .compare_and_swap(0, candidate_with_wire_size(MAX_MANIFEST_BYTES as usize)),
            ManifestCasOutcome::Accepted { .. }
        ));
        assert!(exact_store.manifest_path().exists());
        assert!(!exact_store.root.join(MANIFEST_TEMP_FILE_NAME).exists());

        let oversized_root = root("size-oversized");
        let oversized_store = SessionArtifactManifestStore::qualified_for_test(&oversized_root)
            .expect("qualified oversized fixture");
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

    // audio-graph-3cf2 — install-stage refusal survivors and the abandon path.
    // Grouped as one contiguous trailing block so a later rebase of
    // audio-graph-68a1 (which owns `prepare_compare_and_swap`'s proof gate and
    // `validate_v2_session_provenance`) stays clean.

    /// A refused manifest install leaves BOTH the staged intent temporary and
    /// the durable immutable transition proof behind. The outcome must never
    /// borrow the pre-mutation durability vocabulary for that state, and it
    /// must name the staged intent the caller still owns.
    #[cfg(unix)]
    #[test]
    fn refused_manifest_install_after_durable_proof_is_never_reported_as_unstaged() {
        use super::super::canonical_durability::CanonicalDurabilityStage;

        let root = root("manifest-install-refusal-after-durable-proof");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified Session store");
        let manifest_path = store.manifest_path();
        let temporary_path = store.temporary_path();
        let provenance_path = root.join(store.control_identities().provenance.as_str());
        let transition_id = "advance-refused-manifest-install";
        let candidate = v2_candidate(transition_id);
        let proof = SessionSemanticsTransitionProofV1::v1_to_v2("session-1", transition_id)
            .expect("transition proof");
        let (expected_proof, proof_digest) = proof
            .canonical_bytes_and_digest()
            .expect("canonical proof bytes");
        let staged_intent_key = recovery_key(&proof_digest);

        let mut first = store.begin_write().expect("first transition transaction");
        let outcome = first.advance_session_semantics_v1_to_v2_with_faults(
            0,
            candidate.clone(),
            proof.clone(),
            None,
            None,
            Some(CanonicalDurabilityStage::CreateNew),
        );

        // Which files survive: the intent temporary is file- and parent-synced,
        // the immutable proof is complete and durable, and the install
        // definitively never happened.
        assert!(temporary_path.exists());
        assert_eq!(
            std::fs::read(&provenance_path).expect("durable proof survives the install refusal"),
            expected_proof
        );
        assert!(!manifest_path.exists());

        match &outcome {
            ManifestCasOutcome::Rejected(
                ManifestCasRejection::ManifestInstallRefusedAfterProofAndIntentDurable {
                    rejection,
                    recovery_key: staged_key,
                },
            ) => {
                assert!(matches!(
                    rejection,
                    CanonicalDurabilityRejection::IoFailedBeforeMutation {
                        stage: CanonicalDurabilityStage::CreateNew,
                        ..
                    }
                ));
                assert_eq!(*staged_key, staged_intent_key);
            }
            other => panic!("install refusal after durable proof misclassified as {other:?}"),
        }
        drop(first);

        // The staged temporary is real: the public generation CAS cannot get
        // past it for any other candidate.
        let mut wedged = store.begin_write().expect("wedged transaction");
        assert_eq!(
            wedged.compare_and_swap(0, basic_candidate("wedged-v1", 'a')),
            ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(
                CanonicalDurabilityRejection::SnapshotTempAlreadyExists,
            ))
        );
        assert!(temporary_path.exists());
        assert!(!manifest_path.exists());
        drop(wedged);

        let mut resumed = store.begin_write().expect("resumed transaction");
        assert!(matches!(
            resumed.advance_session_semantics_v1_to_v2(0, candidate, proof),
            ManifestCasOutcome::Accepted { .. }
        ));
        assert!(manifest_path.exists());
        assert!(!temporary_path.exists());
    }

    /// Criterion 3: a caller that gives up on a transition can abandon it and
    /// install a different candidate. Both arms are in one test so the boundary
    /// of abandon is pinned rather than implied: it retires the temporary and
    /// never the immutable proof.
    #[cfg(unix)]
    #[test]
    fn abandoned_transition_unwedges_a_different_candidate() {
        use super::super::canonical_durability::CanonicalDurabilityStage;

        // Arm 1: the proof create was refused, so no proof record exists.
        let absent_root = root("abandon-unwedges-proof-absent");
        let absent_store =
            SessionArtifactManifestStore::qualified_for_test_session(&absent_root, "session-1")
                .expect("qualified Session store");
        let absent_manifest_path = absent_store.manifest_path();
        let absent_temporary_path = absent_store.temporary_path();
        let absent_provenance_path =
            absent_root.join(absent_store.control_identities().provenance.as_str());
        let mut absent = absent_store
            .begin_write()
            .expect("proof-absent transaction");
        assert!(matches!(
            absent.advance_session_semantics_v1_to_v2_with_faults(
                0,
                v2_candidate("advance-abandon-absent"),
                SessionSemanticsTransitionProofV1::v1_to_v2("session-1", "advance-abandon-absent")
                    .expect("abandoned proof"),
                None,
                Some(CanonicalDurabilityStage::CreateNew),
                None,
            ),
            ManifestCasOutcome::Rejected(
                ManifestCasRejection::TransitionProofRefusedAfterIntentStaged { .. }
            )
        ));
        assert!(absent_temporary_path.exists());
        assert!(!absent_provenance_path.exists());
        assert_eq!(
            absent.abandon_staged_transition(),
            CanonicalUnlinkOutcome::Unlinked(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::Unlink,
                barrier: CanonicalDurabilityBarrier::ParentNamespace,
            })
        );
        assert!(!absent_temporary_path.exists());
        assert!(!absent_manifest_path.exists());

        // A DIFFERENT transition now installs through the public entry point.
        assert!(matches!(
            absent.advance_session_semantics_v1_to_v2(
                0,
                v2_candidate("advance-abandon-replacement"),
                SessionSemanticsTransitionProofV1::v1_to_v2(
                    "session-1",
                    "advance-abandon-replacement",
                )
                .expect("replacement proof"),
            ),
            ManifestCasOutcome::Accepted { .. }
        ));
        assert!(absent_manifest_path.exists());
        assert!(!absent_temporary_path.exists());
        drop(absent);

        // Arm 2: the manifest install was refused after the proof went durable.
        let durable_root = root("abandon-unwedges-proof-durable");
        let durable_store =
            SessionArtifactManifestStore::qualified_for_test_session(&durable_root, "session-1")
                .expect("qualified Session store");
        let durable_manifest_path = durable_store.manifest_path();
        let durable_temporary_path = durable_store.temporary_path();
        let durable_provenance_path =
            durable_root.join(durable_store.control_identities().provenance.as_str());
        let abandoned_proof =
            SessionSemanticsTransitionProofV1::v1_to_v2("session-1", "advance-abandon-durable")
                .expect("abandoned durable proof");
        let abandoned_proof_bytes = abandoned_proof
            .canonical_bytes_and_digest()
            .expect("canonical proof bytes")
            .0;
        let mut durable = durable_store
            .begin_write()
            .expect("proof-durable transaction");
        assert!(matches!(
            durable.advance_session_semantics_v1_to_v2_with_faults(
                0,
                v2_candidate("advance-abandon-durable"),
                abandoned_proof,
                None,
                None,
                Some(CanonicalDurabilityStage::CreateNew),
            ),
            ManifestCasOutcome::Rejected(
                ManifestCasRejection::ManifestInstallRefusedAfterProofAndIntentDurable { .. }
            )
        ));
        assert!(durable_temporary_path.exists());
        assert_eq!(
            std::fs::read(&durable_provenance_path).expect("durable proof before abandon"),
            abandoned_proof_bytes
        );
        assert_eq!(
            durable.abandon_staged_transition(),
            CanonicalUnlinkOutcome::Unlinked(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::Unlink,
                barrier: CanonicalDurabilityBarrier::ParentNamespace,
            })
        );
        assert!(!durable_temporary_path.exists());
        assert!(!durable_manifest_path.exists());

        // The wedge is gone: a different v1 candidate installs through the
        // public generation CAS.
        assert!(matches!(
            durable.compare_and_swap(0, basic_candidate("abandoned-then-v1", 'a')),
            ManifestCasOutcome::Accepted { .. }
        ));
        assert!(durable_manifest_path.exists());

        // Abandon did NOT remove the immutable proof, so a DIFFERENT transition
        // id is still refused at the proof identity. Re-keying that gate is
        // audio-graph-68a1's territory.
        assert_eq!(
            durable.advance_session_semantics_v1_to_v2(
                1,
                v2_candidate("advance-abandon-other"),
                SessionSemanticsTransitionProofV1::v1_to_v2("session-1", "advance-abandon-other")
                    .expect("other proof"),
            ),
            ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(
                CanonicalDurabilityRejection::ImmutableExactConflict,
            ))
        );
        assert_eq!(
            std::fs::read(&durable_provenance_path).expect("abandoned proof survives abandon"),
            abandoned_proof_bytes
        );
        assert!(!durable_temporary_path.exists());
    }

    /// Criterion 4: an exact rerun of an abandon is a no-effect assessment, and
    /// no abandon touches the manifest, its generation, or the proof.
    #[cfg(unix)]
    #[test]
    fn exact_rerun_of_abandon_is_a_no_effect_assessment() {
        use super::super::canonical_durability::CanonicalDurabilityStage;

        let root = root("abandon-exact-rerun");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified Session store");
        let manifest_path = store.manifest_path();
        let temporary_path = store.temporary_path();
        let provenance_path = root.join(store.control_identities().provenance.as_str());
        let coordination_path = root.join(store.control_identities().coordination.as_str());
        let mut transaction = store.begin_write().expect("abandon transaction");

        // Nothing staged: the assessment creates nothing. "Nothing" excludes the
        // coordination entry, which `begin_write` establishes before this call.
        assert_eq!(
            transaction.abandon_staged_transition(),
            CanonicalUnlinkOutcome::AlreadyAbsent(CanonicalDurabilityBarrier::ParentNamespace)
        );
        assert!(!temporary_path.exists());
        assert!(!manifest_path.exists());
        assert!(!provenance_path.exists());
        assert!(coordination_path.exists());

        // Establish a v1 head, then wedge a v2 transition with a durable proof.
        assert!(matches!(
            transaction.compare_and_swap(0, basic_candidate("abandon-rerun-v1", 'a')),
            ManifestCasOutcome::Accepted { .. }
        ));
        assert!(matches!(
            transaction.advance_session_semantics_v1_to_v2_with_faults(
                1,
                v2_candidate("advance-abandon-rerun"),
                SessionSemanticsTransitionProofV1::v1_to_v2("session-1", "advance-abandon-rerun")
                    .expect("rerun proof"),
                None,
                None,
                Some(CanonicalDurabilityStage::CreateNew),
            ),
            ManifestCasOutcome::Rejected(
                ManifestCasRejection::ManifestInstallRefusedAfterProofAndIntentDurable { .. }
            )
        ));
        let head_bytes = std::fs::read(&manifest_path).expect("v1 head before abandon");
        let proof_bytes = std::fs::read(&provenance_path).expect("durable proof before abandon");
        let head_generation = match transaction.head() {
            ManifestLoadOutcome::Present(manifest) => manifest.generation,
            ManifestLoadOutcome::Absent => panic!("v1 head must be present"),
        };
        assert_eq!(head_generation, 1);

        assert_eq!(
            transaction.abandon_staged_transition(),
            CanonicalUnlinkOutcome::Unlinked(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::Unlink,
                barrier: CanonicalDurabilityBarrier::ParentNamespace,
            })
        );
        for _ in 0..2 {
            assert_eq!(
                transaction.abandon_staged_transition(),
                CanonicalUnlinkOutcome::AlreadyAbsent(CanonicalDurabilityBarrier::ParentNamespace)
            );
        }
        assert!(!temporary_path.exists());
        assert_eq!(
            std::fs::read(&manifest_path).expect("head after three abandons"),
            head_bytes
        );
        assert_eq!(
            std::fs::read(&provenance_path).expect("proof after three abandons"),
            proof_bytes
        );
        assert!(matches!(
            transaction.head(),
            ManifestLoadOutcome::Present(manifest) if manifest.generation == head_generation
        ));
    }

    /// Abandon is NOT the reconciliation for a `DurabilityIndeterminate`: an
    /// install that renamed but lost its barrier already consumed the temporary,
    /// so the abandon finds it absent and its own parent barrier can be what
    /// makes that install durable. `AlreadyAbsent` must therefore not be read as
    /// "the head is unchanged" — the documented caveat, pinned.
    #[cfg(unix)]
    #[test]
    fn abandon_after_an_indeterminate_install_publishes_rather_than_retracts() {
        use super::super::canonical_durability::CanonicalDurabilityStage;

        let root = root("abandon-after-indeterminate-install");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, "session-1")
            .expect("qualified Session store");
        let manifest_path = store.manifest_path();
        let temporary_path = store.temporary_path();
        let transition_id = "advance-abandon-indeterminate";
        let mut first = store.begin_write().expect("indeterminate transaction");

        assert!(matches!(
            first.advance_session_semantics_v1_to_v2_with_faults(
                0,
                v2_candidate(transition_id),
                SessionSemanticsTransitionProofV1::v1_to_v2("session-1", transition_id)
                    .expect("indeterminate proof"),
                None,
                None,
                Some(CanonicalDurabilityStage::ParentSync),
            ),
            ManifestCasOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: CanonicalDurabilityStage::ParentSync,
                ..
            })
        ));
        // The rename ran; only its namespace barrier was lost.
        assert!(manifest_path.exists());
        assert!(!temporary_path.exists());

        assert_eq!(
            first.abandon_staged_transition(),
            CanonicalUnlinkOutcome::AlreadyAbsent(CanonicalDurabilityBarrier::ParentNamespace)
        );
        assert!(manifest_path.exists());
        drop(first);

        // The head advanced to the candidate the caller tried to abandon.
        let next = store.begin_write().expect("post-abandon transaction");
        assert!(matches!(
            next.head(),
            ManifestLoadOutcome::Present(manifest)
                if manifest.generation == 1
                    && manifest.transition.idempotency_id == transition_id
                    && manifest.session_semantics_version == SessionSemanticsVersion::V2
        ));
    }
}
