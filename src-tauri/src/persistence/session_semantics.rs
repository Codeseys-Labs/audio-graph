//! Session-semantics compatibility-floor kernel.
//!
//! NOT dormant, as of seed audio-graph-e8e7. Two entry points are on production
//! paths: `open_session_for_content` gates the transcript fork reached from the
//! `load_session`, `load_session_transcript`, and `export_session_bundle`
//! commands, and `retire_session_control_plane` runs inside
//! `permanently_delete_session` and retention purge. Changing either one's
//! branch policy or residual classification is user-visible.
//!
//! Every other item here — the v1-to-v2 admission, the control-plane export
//! surface, and the historical bootstrap builder — still has no production
//! caller.

use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::canonical_durability::{
    CanonicalDurabilityIndeterminate, CanonicalFilesystemQualificationError, CanonicalUnlinkOutcome,
};
use super::session_artifact_manifest::{
    ArtifactAvailability, ArtifactContentIdentity, ArtifactPrivacyClass, ArtifactUnavailableReason,
    CheckedManifestReadError, MAX_SESSION_SEMANTICS_TRANSITION_PROOF_BYTES,
    ManagedArtifactIdentity, ManifestCasOutcome, ManifestCasRejection, ManifestLoadError,
    ManifestLoadOutcome, ManifestStoreError, ManifestTransition, ManifestTransitionState,
    ManifestValidationError, ManifestWriteTransaction, SessionArtifactEntry, SessionArtifactKind,
    SessionArtifactManifestStore, SessionArtifactManifestV1, SessionControlIdentities,
    SessionSemanticsTransitionProofError, SessionSemanticsTransitionProofV1, Sha256Digest,
    V2SessionProvenanceError, session_control_identities_for, validate_v2_session_provenance,
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
    /// The outcome handed to `admitted_session_semantics_floor` was `Rejected`
    /// or `DurabilityIndeterminate`.
    ///
    /// Unreachable *through* [`admit_session_semantics_v1_to_v2`], which
    /// classifies both of those outcomes itself and forwards them verbatim so a
    /// caller can reconcile their recovery keys. It stays reachable for a caller
    /// that hands this function a CAS outcome directly.
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
    let manifest = match outcome {
        ManifestCasOutcome::Accepted { manifest, .. }
        | ManifestCasOutcome::AlreadyCompleted { manifest } => manifest,
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
    // Preservation, not advance: an ordinary `Accepted` generation of an
    // already-advanced Session and an exact `AlreadyCompleted` retry both report
    // the floor the manifest already carries. Neither is a transition.
    if current == accepted {
        return Ok(current);
    }
    if current == SessionSemanticsVersion::V1 && accepted == SessionSemanticsVersion::V2 {
        return Ok(SessionSemanticsVersion::V2);
    }
    // Only `accepted < current` remains. The CAS already refuses that upstream as
    // `ManifestCasRejection::SessionSemanticsFloorRegression`, so this arm is
    // defence against a caller that reached this function some other way.
    Err(SessionSemanticsAdvanceError::IllegalTransition { current, accepted })
}

// ===========================================================================
// audio-graph-e8e7: guarded admission.
//
// Everything below is a consumer of `session_artifact_manifest`'s public API.
// It adds no CAS, validation, gate, or classification behaviour to that module.
// ===========================================================================

/// Why one v1-to-v2 admission did not produce a logical floor.
///
/// `SessionSemanticsAdvanceError` is `Copy`; `ManifestCasRejection` is not, so
/// this type is `Clone` only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSemanticsAdmissionError {
    /// The manifest CAS refused. Forwarded verbatim, including
    /// `TransitionProofRefusedAfterIntentStaged` and
    /// `ManifestInstallRefusedAfterProofAndIntentDurable` and their recovery
    /// keys: the caller must reconcile with those, not with a widened refusal.
    Refused(ManifestCasRejection),
    /// The manifest install's durability is unknown. The reconciliation is the
    /// exact rerun keyed by this outcome's recovery key.
    DurabilityIndeterminate(CanonicalDurabilityIndeterminate),
    /// The CAS was authoritative but its manifest does not admit the claimed
    /// floor.
    Floor(SessionSemanticsAdvanceError),
}

/// Advance one Session's floor and classify the result from the CAS outcome the
/// advance actually produced.
///
/// The outcome never leaves this function: no boolean, receipt, head re-read, or
/// caller-asserted success crosses the boundary, so no caller can synthesise the
/// evidence [`admitted_session_semantics_floor`] classifies. That is the
/// structural form of ADR-0044 §6's requirement that logical admission consume
/// the actual `ManifestCasOutcome`.
///
/// This has no production caller at this base: activating a v2 writer is out of
/// scope for seed audio-graph-e8e7.
pub fn admit_session_semantics_v1_to_v2(
    transaction: &mut ManifestWriteTransaction<'_>,
    expected_session_id: &str,
    current: SessionSemanticsVersion,
    expected_generation: u64,
    candidate: SessionArtifactManifestV1,
    proof: SessionSemanticsTransitionProofV1,
) -> Result<SessionSemanticsVersion, SessionSemanticsAdmissionError> {
    let outcome =
        transaction.advance_session_semantics_v1_to_v2(expected_generation, candidate, proof);
    match &outcome {
        ManifestCasOutcome::Rejected(rejection) => {
            return Err(SessionSemanticsAdmissionError::Refused(rejection.clone()));
        }
        ManifestCasOutcome::DurabilityIndeterminate(indeterminate) => {
            return Err(SessionSemanticsAdmissionError::DurabilityIndeterminate(
                *indeterminate,
            ));
        }
        ManifestCasOutcome::Accepted { .. } | ManifestCasOutcome::AlreadyCompleted { .. } => {}
    }
    admitted_session_semantics_floor(expected_session_id, current, &outcome)
        .map_err(SessionSemanticsAdmissionError::Floor)
}

/// What was observed, and under which guard, to admit a Session's floor.
///
/// Every variant states only what this call saw. None of them claims anything
/// about the platform, or about state that was never observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionFloorEvidence {
    /// A manifest was selected and floor-validated while this call held the
    /// store's shared coordination guard.
    GuardedManifest,
    /// No manifest was selected under the store's shared coordination guard.
    GuardedAbsence,
    /// This Session's manifest, temporary, and provenance identities and the
    /// store-owned coordination identity were all observed absent immediately
    /// before the content reader ran and again before its result escaped. No
    /// guard was held at any point.
    UnguardedObservedAbsence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdmittedSessionFloor {
    pub floor: SessionSemanticsVersion,
    pub evidence: SessionFloorEvidence,
}

/// A control identity could not be classified as present or absent.
///
/// `NotFound` is the only condition that means absence; every other error lands
/// here rather than being folded into "nothing is there".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionControlPlaneObservationError {
    pub kind: io::ErrorKind,
    pub raw_os_error: Option<i32>,
}

impl fmt::Display for SessionControlPlaneObservationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "control identity unclassifiable: {:?}",
            self.kind
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardedSessionOpenError<E> {
    UnsupportedReaderFloor {
        actual: u32,
    },
    InvalidSessionFloor {
        actual: u32,
    },
    UnsupportedSessionFloor {
        required: SessionSemanticsVersion,
        maximum_supported: SessionSemanticsVersion,
    },
    /// The control plane exists but could not be read strictly.
    ControlPlaneUnreadable(ManifestLoadError),
    /// A namespace-mutating platform requires live filesystem qualification
    /// before the coordination entry may be established.
    ControlPlaneQualificationRequired,
    /// The addressed store could not be constructed or qualified. Never widened
    /// into an admitted floor.
    ControlPlaneStore(ManifestStoreError),
    ControlPlaneObservation(SessionControlPlaneObservationError),
    /// A control identity was observed immediately before the unguarded content
    /// snapshot was built, or immediately before it escaped. ADR-0044 §5 requires
    /// a typed refusal here rather than v1.
    ControlPlaneAppearedDuringUnguardedRead,
    ContentReader(E),
}

impl<E: fmt::Display> fmt::Display for GuardedSessionOpenError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedReaderFloor { actual } => write!(
                formatter,
                "this reader declares an unsupported Session semantics floor ({actual})"
            ),
            Self::InvalidSessionFloor { actual } => write!(
                formatter,
                "this Session records an unsupported semantics floor ({actual})"
            ),
            Self::UnsupportedSessionFloor {
                required,
                maximum_supported,
            } => write!(
                formatter,
                "this Session requires semantics floor v{} and this reader supports at most v{}",
                required.as_u32(),
                maximum_supported.as_u32()
            ),
            Self::ControlPlaneUnreadable(error) => {
                write!(formatter, "Session control plane unreadable: {error:?}")
            }
            Self::ControlPlaneQualificationRequired => formatter.write_str(
                "Session control plane requires filesystem qualification on this platform",
            ),
            Self::ControlPlaneStore(error) => {
                write!(formatter, "Session control plane unavailable: {error:?}")
            }
            Self::ControlPlaneObservation(error) => write!(formatter, "{error}"),
            Self::ControlPlaneAppearedDuringUnguardedRead => formatter.write_str(
                "Session control plane changed during an unguarded read; retry required",
            ),
            Self::ContentReader(error) => write!(formatter, "{error}"),
        }
    }
}

/// Admit a Session's compatibility floor with the manifest the store's own
/// coordination boundary selected, then run the content reader.
///
/// This is the guard-owned wrapper around [`checked_session_open`]. The inner
/// primitive validates support; this one supplies the manifest that validation
/// must apply to, so no caller can hand it a manifest it chose itself.
///
/// On a namespace-mutating platform this READ MAY CREATE the store-owned
/// `.audio-graph-canonical.lock`: `SessionArtifactManifestStore::checked_read`
/// establishes the coordination entry when it is missing, by taking and dropping
/// an exclusive guard before re-acquiring a shared one. That is ADR-0044 §5's
/// qualified branch, and it is the only namespace mutation any path in this
/// function can perform.
///
/// An `UncoordinatedAbsence` — reachable only on the unqualified read-only
/// branch, where nothing at all exists — is admitted as `V1` through the
/// pre/post absence sandwich ADR-0044 §5 mandates, never from a single unlocked
/// look.
pub fn guarded_session_open<T, E>(
    store: &SessionArtifactManifestStore,
    maximum_supported: SessionSemanticsVersion,
    content_reader: impl FnOnce(AdmittedSessionFloor) -> Result<T, E>,
) -> Result<T, GuardedSessionOpenError<E>> {
    if !maximum_supported.is_supported() {
        return Err(GuardedSessionOpenError::UnsupportedReaderFloor {
            actual: maximum_supported.as_u32(),
        });
    }
    let mut pending = Some(content_reader);
    let read = store.checked_read(|head| match head {
        ManifestLoadOutcome::Present(manifest) => {
            let reader = pending
                .take()
                .expect("checked_read invokes its reader at most once");
            checked_session_open(&manifest, maximum_supported, || {
                reader(AdmittedSessionFloor {
                    floor: manifest.session_semantics_version,
                    evidence: SessionFloorEvidence::GuardedManifest,
                })
            })
            .map_err(|error| match error {
                CheckedSessionOpenError::UnsupportedReaderFloor { actual } => {
                    GuardedSessionOpenError::UnsupportedReaderFloor { actual }
                }
                CheckedSessionOpenError::InvalidSessionFloor { actual } => {
                    GuardedSessionOpenError::InvalidSessionFloor { actual }
                }
                CheckedSessionOpenError::UnsupportedSessionFloor {
                    required,
                    maximum_supported,
                } => GuardedSessionOpenError::UnsupportedSessionFloor {
                    required,
                    maximum_supported,
                },
                CheckedSessionOpenError::ContentReader(error) => {
                    GuardedSessionOpenError::ContentReader(error)
                }
            })
        }
        ManifestLoadOutcome::Absent => {
            let reader = pending
                .take()
                .expect("checked_read invokes its reader at most once");
            reader(AdmittedSessionFloor {
                floor: SessionSemanticsVersion::V1,
                evidence: SessionFloorEvidence::GuardedAbsence,
            })
            .map_err(GuardedSessionOpenError::ContentReader)
        }
    });
    match read {
        Ok(value) => Ok(value),
        Err(CheckedManifestReadError::Reader(error)) => Err(error),
        Err(CheckedManifestReadError::Load(error)) => {
            Err(GuardedSessionOpenError::ControlPlaneUnreadable(error))
        }
        Err(CheckedManifestReadError::NamespaceQualificationRequired) => {
            Err(GuardedSessionOpenError::ControlPlaneQualificationRequired)
        }
        Err(CheckedManifestReadError::UncoordinatedAbsence) => {
            let reader = pending
                .take()
                .expect("an uncoordinated absence never invokes the content reader");
            let Some(identities) = store.addressed_control_identities() else {
                return Err(GuardedSessionOpenError::ControlPlaneStore(
                    ManifestStoreError::InvalidSessionAddress,
                ));
            };
            let root = store.managed_root();
            let paths = control_plane_paths_from(root, identities);
            let coordination = root.join(identities.coordination.as_str());
            unguarded_absence_admission(&paths, &coordination, reader)
        }
    }
}

/// Admit `V1` from observed absence on a path where no guard can be held, with
/// the exact pre/post re-validation ADR-0044 §5 requires of that branch: the
/// content snapshot is built without releasing bytes to the caller, both the
/// Session identities and the store-owned coordination identity are checked
/// again immediately before the snapshot escapes, and any appearance returns a
/// typed refusal rather than v1.
fn unguarded_absence_admission<T, E>(
    paths: &SessionControlPlanePaths,
    coordination: &Path,
    content_reader: impl FnOnce(AdmittedSessionFloor) -> Result<T, E>,
) -> Result<T, GuardedSessionOpenError<E>> {
    if any_control_entry_present(paths, coordination)
        .map_err(GuardedSessionOpenError::ControlPlaneObservation)?
    {
        return Err(GuardedSessionOpenError::ControlPlaneAppearedDuringUnguardedRead);
    }
    let snapshot = content_reader(AdmittedSessionFloor {
        floor: SessionSemanticsVersion::V1,
        evidence: SessionFloorEvidence::UnguardedObservedAbsence,
    })
    .map_err(GuardedSessionOpenError::ContentReader)?;
    if any_control_entry_present(paths, coordination)
        .map_err(GuardedSessionOpenError::ControlPlaneObservation)?
    {
        return Err(GuardedSessionOpenError::ControlPlaneAppearedDuringUnguardedRead);
    }
    Ok(snapshot)
}

/// Open one Session for a content read, choosing the ADR-0044 §5 branch from the
/// control-plane state that actually exists at this root.
///
/// With no control plane at all — every Session at this base — this constructs no
/// store, scans no mount table, creates no coordination entry, and performs only
/// `symlink_metadata` calls, so the read's filesystem effect is byte-identical to
/// having no control plane check at all.
///
/// CONSTRAINT the code cannot show: the branch choice itself is made from an
/// unlocked observation, which ADR-0044 §5 fixes by capability rather than by
/// observation. It is admissible only while nothing can create a control plane
/// or a v2 artifact concurrently: at this base
/// `ManifestWriteTransaction::advance_session_semantics_v1_to_v2` has no caller
/// and `validate_artifact_semantics` has no caller outside this module. When
/// either is activated, this preflight must be replaced by a single
/// `guarded_session_open` call site. It is NOT race-safe and is not described as
/// such anywhere.
pub fn open_session_for_content<T, E>(
    data_root: &Path,
    session_id: &str,
    maximum_supported: SessionSemanticsVersion,
    content_reader: impl FnOnce(AdmittedSessionFloor) -> Result<T, E>,
) -> Result<T, GuardedSessionOpenError<E>> {
    // Refuse an unsupported reader floor here, not only inside
    // `guarded_session_open`: otherwise the same caller error is classified two
    // different ways depending on whether control-plane bytes happen to exist.
    if !maximum_supported.is_supported() {
        return Err(GuardedSessionOpenError::UnsupportedReaderFloor {
            actual: maximum_supported.as_u32(),
        });
    }
    let identities = session_control_identities_for(session_id)
        .map_err(GuardedSessionOpenError::ControlPlaneStore)?;
    let paths = control_plane_paths_from(data_root, &identities);
    let coordination = data_root.join(identities.coordination.as_str());
    if !any_control_entry_present(&paths, &coordination)
        .map_err(GuardedSessionOpenError::ControlPlaneObservation)?
    {
        return unguarded_absence_admission(&paths, &coordination, content_reader);
    }
    match SessionArtifactManifestStore::qualified_existing_session(data_root, session_id) {
        Ok(store) => guarded_session_open(&store, maximum_supported, content_reader),
        // The production namespace policy makes manifest mutation unavailable on
        // this platform, which is exactly ADR-0044 §5's read-only branch.
        Err(ManifestStoreError::Qualification(
            CanonicalFilesystemQualificationError::NamespaceDurabilityUnsupported { .. },
        )) => {
            let store = SessionArtifactManifestStore::for_session(data_root, session_id)
                .map_err(GuardedSessionOpenError::ControlPlaneStore)?;
            guarded_session_open(&store, maximum_supported, content_reader)
        }
        // Every other refusal is a refusal. A control plane that exists and
        // cannot be read under a guard is never admitted as historical v1.
        Err(error) => Err(GuardedSessionOpenError::ControlPlaneStore(error)),
    }
}

// ---------------------------------------------------------------------------
// Session-owned control-plane surface (ADR-0044 §3).
// ---------------------------------------------------------------------------

/// The three Session-OWNED control paths.
///
/// The store-owned global `.audio-graph-canonical.lock` is deliberately
/// unrepresentable here, so no inventory, export, recovery, or delete caller can
/// reach it by iterating this value. `SessionControlIdentities` bundles the lock
/// with these three and must not be iterated by a Session-lifecycle caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionControlPlanePaths {
    pub manifest: PathBuf,
    pub temporary: PathBuf,
    pub provenance: PathBuf,
}

impl SessionControlPlanePaths {
    pub fn all(&self) -> [&Path; 3] {
        [&self.manifest, &self.temporary, &self.provenance]
    }
}

fn control_plane_paths_from(
    data_root: &Path,
    identities: &SessionControlIdentities,
) -> SessionControlPlanePaths {
    SessionControlPlanePaths {
        manifest: data_root.join(identities.manifest.as_str()),
        temporary: data_root.join(identities.temporary.as_str()),
        provenance: data_root.join(identities.provenance.as_str()),
    }
}

/// Derive one Session's three owned control paths. No I/O, and an ineligible
/// Session id is refused before any path is derived.
pub fn session_control_plane_paths(
    data_root: &Path,
    session_id: &str,
) -> Result<SessionControlPlanePaths, ManifestStoreError> {
    Ok(control_plane_paths_from(
        data_root,
        &session_control_identities_for(session_id)?,
    ))
}

/// The store-owned coordination path, returned SEPARATELY from
/// [`SessionControlPlanePaths`] on purpose: a caller that wants it must ask for
/// it by name, and no Session-lifecycle operation can pick it up by accident.
pub fn store_coordination_path(
    data_root: &Path,
    session_id: &str,
) -> Result<PathBuf, ManifestStoreError> {
    Ok(data_root.join(
        session_control_identities_for(session_id)?
            .coordination
            .as_str(),
    ))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionControlPlaneResidue {
    pub manifest: bool,
    pub temporary: bool,
    pub provenance: bool,
}

impl SessionControlPlaneResidue {
    pub fn is_empty(&self) -> bool {
        !self.manifest && !self.temporary && !self.provenance
    }
}

/// Classify each Session-owned control identity as present or absent.
///
/// Uses `symlink_metadata`, so a symlink or directory at a control identity is
/// PRESENT rather than followed, and `NotFound` is the only condition that means
/// absent. `Path::exists()` is deliberately not used: it folds a permission
/// error into "nothing is there", which on the unguarded read path would turn an
/// unreadable control plane into an admitted historical v1.
pub fn observe_session_control_plane(
    paths: &SessionControlPlanePaths,
) -> Result<SessionControlPlaneResidue, SessionControlPlaneObservationError> {
    Ok(SessionControlPlaneResidue {
        manifest: control_entry_present(&paths.manifest)?,
        temporary: control_entry_present(&paths.temporary)?,
        provenance: control_entry_present(&paths.provenance)?,
    })
}

fn control_entry_present(path: &Path) -> Result<bool, SessionControlPlaneObservationError> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(SessionControlPlaneObservationError {
            kind: error.kind(),
            raw_os_error: error.raw_os_error(),
        }),
    }
}

fn any_control_entry_present(
    paths: &SessionControlPlanePaths,
    coordination: &Path,
) -> Result<bool, SessionControlPlaneObservationError> {
    if !observe_session_control_plane(paths)?.is_empty() {
        return Ok(true);
    }
    control_entry_present(coordination)
}

// ---------------------------------------------------------------------------
// Per-Session control-plane export (ADR-0044 §3).
// ---------------------------------------------------------------------------

/// One Session's exported control plane.
///
/// The store-owned lock is absent by construction, and the manifest temporary is
/// reported as residue rather than exported: a staged intent is recovery
/// material, not Session evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionControlPlaneExport {
    pub manifest: Option<SessionArtifactManifestV1>,
    pub proof: Option<Vec<u8>>,
    pub temporary_residual: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionControlPlaneExportError {
    ControlPlaneUnreadable(ManifestLoadError),
    ControlPlaneQualificationRequired,
    /// Nothing exists at this Session's control identities and no coordination
    /// entry exists either, so there is no control plane to export.
    UncoordinatedControlPlaneAbsence,
    ControlPlaneStore(ManifestStoreError),
    ControlPlaneObservation(SessionControlPlaneObservationError),
    /// The provenance identity resolves to a non-regular entry.
    ProofNonRegular,
    ProofExceedsCanonicalBound,
    ProofNotCanonical(SessionSemanticsTransitionProofError),
    /// The record is a canonical proof for a different Session than this store's
    /// address.
    ProofSessionMismatch,
    /// A V2 manifest is present and its durable proof is absent. ADR-0044 §3: a
    /// successful export contains the manifest AND an available proof, so this is
    /// never reported as a successful export with no proof.
    V2ProofMissing,
    /// The V2 manifest's provenance entry is not a valid V2 provenance entry.
    V2ProvenanceEntryInvalid(V2SessionProvenanceError),
    /// The V2 manifest's provenance entry names something other than this
    /// Session's derived provenance identity.
    V2ProvenanceIdentityMismatch,
    /// The proof bytes on disk are not the bytes the V2 manifest inventories.
    V2ProofContentMismatch,
}

/// Export one Session's control plane under the store's shared guard.
///
/// The manifest, the proof bytes, and the temporary's residue are all observed
/// inside one `checked_read` closure, so the export is one coordinated snapshot
/// rather than three independent looks.
///
/// The proof bytes are AUTHENTICATED before they are included: regular file,
/// within the canonical proof ceiling, exactly one canonical transition proof,
/// and — for a V2 manifest — a digest and length equal to the manifest
/// provenance entry's. An unverified byte string is never exported as evidence
/// for an irreversible transition.
pub fn export_session_control_plane(
    store: &SessionArtifactManifestStore,
) -> Result<SessionControlPlaneExport, SessionControlPlaneExportError> {
    let Some(identities) = store.addressed_control_identities() else {
        return Err(SessionControlPlaneExportError::ControlPlaneStore(
            ManifestStoreError::InvalidSessionAddress,
        ));
    };
    let paths = control_plane_paths_from(store.managed_root(), identities);
    let read = store.checked_read(|head| {
        let temporary_residual = control_entry_present(&paths.temporary)
            .map_err(SessionControlPlaneExportError::ControlPlaneObservation)?;
        let observed = read_session_transition_proof(&paths.provenance)?;
        if let Some((_, _, proof)) = observed.as_ref() {
            let derived = session_control_identities_for(proof.session_id())
                .map_err(SessionControlPlaneExportError::ControlPlaneStore)?;
            if derived.provenance != identities.provenance {
                return Err(SessionControlPlaneExportError::ProofSessionMismatch);
            }
        }
        let manifest = match head {
            ManifestLoadOutcome::Absent => {
                return Ok(SessionControlPlaneExport {
                    manifest: None,
                    proof: observed.map(|(bytes, _, _)| bytes),
                    temporary_residual,
                });
            }
            ManifestLoadOutcome::Present(manifest) => *manifest,
        };
        if manifest.session_semantics_version != SessionSemanticsVersion::V2 {
            return Ok(SessionControlPlaneExport {
                manifest: Some(manifest),
                proof: observed.map(|(bytes, _, _)| bytes),
                temporary_residual,
            });
        }
        validate_v2_session_provenance(&manifest)
            .map_err(SessionControlPlaneExportError::V2ProvenanceEntryInvalid)?;
        let entry = manifest
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == SessionArtifactKind::SessionProvenanceEvents)
            .ok_or(SessionControlPlaneExportError::V2ProvenanceEntryInvalid(
                V2SessionProvenanceError::Missing,
            ))?;
        let ArtifactAvailability::Present { content } = &entry.availability else {
            return Err(SessionControlPlaneExportError::V2ProvenanceEntryInvalid(
                V2SessionProvenanceError::Unavailable,
            ));
        };
        let Some((bytes, digest, _)) = observed else {
            return Err(SessionControlPlaneExportError::V2ProofMissing);
        };
        if entry.managed_identity != identities.provenance {
            return Err(SessionControlPlaneExportError::V2ProvenanceIdentityMismatch);
        }
        if content.sha256 != digest
            || content.byte_length != u64::try_from(bytes.len()).unwrap_or(u64::MAX)
        {
            return Err(SessionControlPlaneExportError::V2ProofContentMismatch);
        }
        Ok(SessionControlPlaneExport {
            manifest: Some(manifest),
            proof: Some(bytes),
            temporary_residual,
        })
    });
    match read {
        Ok(export) => Ok(export),
        Err(CheckedManifestReadError::Reader(error)) => Err(error),
        Err(CheckedManifestReadError::Load(error)) => Err(
            SessionControlPlaneExportError::ControlPlaneUnreadable(error),
        ),
        Err(CheckedManifestReadError::NamespaceQualificationRequired) => {
            Err(SessionControlPlaneExportError::ControlPlaneQualificationRequired)
        }
        Err(CheckedManifestReadError::UncoordinatedAbsence) => {
            Err(SessionControlPlaneExportError::UncoordinatedControlPlaneAbsence)
        }
    }
}

/// Read and authenticate the durable transition-proof record, if one exists.
///
/// `Ok(None)` means the record is absent. Anything present that is not exactly
/// one canonical proof is a classified refusal, never partially trusted bytes.
#[allow(clippy::type_complexity)]
fn read_session_transition_proof(
    path: &Path,
) -> Result<
    Option<(Vec<u8>, Sha256Digest, SessionSemanticsTransitionProofV1)>,
    SessionControlPlaneExportError,
> {
    let observation = |error: &io::Error| {
        SessionControlPlaneExportError::ControlPlaneObservation(
            SessionControlPlaneObservationError {
                kind: error.kind(),
                raw_os_error: error.raw_os_error(),
            },
        )
    };
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.file_type().is_file() => {
            return Err(SessionControlPlaneExportError::ProofNonRegular);
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(observation(&error)),
    }
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(observation(&error)),
    };
    let metadata = file.metadata().map_err(|error| observation(&error))?;
    if !metadata.file_type().is_file() {
        return Err(SessionControlPlaneExportError::ProofNonRegular);
    }
    if metadata.len() > MAX_SESSION_SEMANTICS_TRANSITION_PROOF_BYTES {
        return Err(SessionControlPlaneExportError::ProofExceedsCanonicalBound);
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(MAX_SESSION_SEMANTICS_TRANSITION_PROOF_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| observation(&error))?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_SESSION_SEMANTICS_TRANSITION_PROOF_BYTES
    {
        return Err(SessionControlPlaneExportError::ProofExceedsCanonicalBound);
    }
    let proof = SessionSemanticsTransitionProofV1::from_canonical_bytes(&bytes)
        .map_err(SessionControlPlaneExportError::ProofNotCanonical)?;
    let (_, digest) = proof
        .canonical_bytes_and_digest()
        .map_err(SessionControlPlaneExportError::ProofNotCanonical)?;
    Ok(Some((bytes, digest, proof)))
}

// ---------------------------------------------------------------------------
// Per-Session control-plane retirement (ADR-0044 §3 delete parity).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionControlPlaneRetirement {
    /// No Session-owned control identity was observed, so nothing was locked,
    /// opened, or removed. This is every Session's state at this base.
    Nothing,
    /// Every Session-owned control identity is durably absent.
    Retired,
    /// At least one Session-owned control identity may still exist. `failures`
    /// names the reason for each one, and the fact of a recovery key without its
    /// bytes.
    Residual { failures: Vec<String> },
}

fn control_plane_residual(reason: String) -> SessionControlPlaneRetirement {
    SessionControlPlaneRetirement::Residual {
        failures: vec![reason],
    }
}

/// Retire one Session's three owned control identities, observing before
/// constructing anything.
///
/// The observation deliberately precedes store construction. With no residue —
/// every Session at this base — this performs three `symlink_metadata` calls and
/// returns `Nothing`: no qualification probe, no mount scan, no lock, and
/// therefore no way for a data root the substrate cannot qualify to turn a
/// succeeding delete into a failure.
pub fn retire_session_control_plane(
    data_root: &Path,
    session_id: &str,
) -> SessionControlPlaneRetirement {
    let paths = match session_control_plane_paths(data_root, session_id) {
        Ok(paths) => paths,
        Err(error) => {
            return control_plane_residual(format!(
                "session control-plane address refused: {error:?}"
            ));
        }
    };
    match observe_session_control_plane(&paths) {
        Ok(residue) if residue.is_empty() => return SessionControlPlaneRetirement::Nothing,
        Ok(_) => {}
        Err(error) => {
            return control_plane_residual(format!("session control plane unobservable: {error}"));
        }
    }
    match SessionArtifactManifestStore::qualified_existing_session(data_root, session_id) {
        Ok(store) => retire_qualified_control_plane(&store),
        Err(error) => control_plane_residual(format!(
            "session control-plane residue has no durable removal path at this root: {error:?}"
        )),
    }
}

/// The store-injected leg of [`retire_session_control_plane`], for callers that
/// already hold the addressed store whose control plane is to be retired.
pub fn retire_observed_session_control_plane(
    store: &SessionArtifactManifestStore,
) -> SessionControlPlaneRetirement {
    let Some(identities) = store.addressed_control_identities() else {
        return control_plane_residual(
            "session control-plane retirement requires an addressed store".to_owned(),
        );
    };
    let paths = control_plane_paths_from(store.managed_root(), identities);
    match observe_session_control_plane(&paths) {
        Ok(residue) if residue.is_empty() => SessionControlPlaneRetirement::Nothing,
        Ok(_) => retire_qualified_control_plane(store),
        Err(error) => {
            control_plane_residual(format!("session control plane unobservable: {error}"))
        }
    }
}

fn retire_qualified_control_plane(
    store: &SessionArtifactManifestStore,
) -> SessionControlPlaneRetirement {
    let report = match store.retire_owned_control_plane() {
        Ok(report) => report,
        Err(error) => {
            return control_plane_residual(format!(
                "session control-plane retirement refused before any removal: {error:?}"
            ));
        }
    };
    if report.is_complete() {
        return SessionControlPlaneRetirement::Retired;
    }
    let mut failures = Vec::new();
    for (label, outcome) in [
        ("manifest temporary", Some(report.temporary)),
        ("manifest head", report.manifest),
        ("transition proof", report.provenance),
    ] {
        match outcome {
            Some(
                CanonicalUnlinkOutcome::Unlinked(_) | CanonicalUnlinkOutcome::AlreadyAbsent(_),
            ) => {}
            Some(outcome) => failures.push(format!(
                "session {label} control entry was not retired: {outcome:?}"
            )),
            None => failures.push(format!(
                "session {label} control entry was not attempted; an earlier control-plane step did not reach durable absence"
            )),
        }
    }
    SessionControlPlaneRetirement::Residual { failures }
}

// ---------------------------------------------------------------------------
// Historical bootstrap (ADR-0044 §8).
// ---------------------------------------------------------------------------

/// The builder-owned managed identity for the mandatory Original Session Audio
/// entry.
///
/// It carries no container extension on purpose: with no observed bytes the
/// format is unknown, and naming one would fabricate evidence. No writer owns
/// this identity — [`ArtifactAvailability::Unavailable`] carries no content
/// identity, so the entry is a stable artifact identity with no available bytes.
pub const HISTORICAL_ORIGINAL_AUDIO_IDENTITY: &str = "audio/original-session-audio";

/// The Session-index identity in the flat artifact root. Reserved against
/// caller-supplied observed identities here and, since the R4 closure, against
/// an ordinary inventory entry in the manifest validator.
///
/// This is the ONE reserved name that is not derived from its writer. The index
/// writers spell it independently — `user_data.rs` joins the literal onto the
/// data root, as does `persistence/mod.rs` — so a rename there silently
/// unreserves it here. Every other reserved name comes from
/// `session_control_address` or from the manifest module's own root-wide
/// constants, which ARE the writers' source. Keep this const the single copy on
/// the reservation side and change it in the same commit as the writers.
pub(crate) const SESSIONS_INDEX_IDENTITY: &str = "sessions.json";

/// First path segments an observed identity may have.
///
/// Every flat-root name is therefore refused, which is what closes the
/// `validate_managed_identity` gap this list exists for. `audio` is present
/// because an observed original-audio file would legitimately live there; the
/// live inventory never produces one. If an observed identity ever equals the
/// builder-owned audio identity above, both entries reach the manifest validator
/// and it refuses the pair as `CaseEquivalentManagedIdentity`.
const MANAGED_ARTIFACT_ROOTS: [&str; 8] = [
    "audio",
    "transcripts",
    "projections",
    "notes",
    "graphs",
    "ledgers",
    "usage",
    "live_assist",
];

const HISTORICAL_BOOTSTRAP_FINGERPRINT_DOMAIN: &[u8] =
    b"audio-graph/session-historical-bootstrap-inventory/v1\0";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoricalBootstrapError {
    /// The Session id is not eligible for production control addressing.
    UnaddressableSession(ManifestStoreError),
    /// The observed identity is the Session index.
    SessionIndexIdentity,
    /// The observed identity is one of THIS Session's own control identities.
    SessionControlIdentity,
    /// The observed identity's first segment is not a known managed artifact
    /// subdirectory. Another Session's control identity lands here, as does any
    /// other flat-root name.
    IdentityOutsideManagedArtifactTree,
    /// An observed entry claimed the Original Session Audio kind.
    /// `HistoricalOriginalAudio` is the only channel for that entry.
    ObservedOriginalSessionAudio,
    /// The mirror of `ObservedOriginalSessionAudio`: an entry routed through the
    /// Original Session Audio channel that classifies itself as something else.
    /// Refused rather than coerced — the channel does not get to overwrite what
    /// the observation said it read.
    MisChanneledOriginalAudioObservation {
        kind: SessionArtifactKind,
        privacy_class: ArtifactPrivacyClass,
    },
    /// The observed path resolves to a directory, symlink, or device.
    NonRegularObservedEntry,
    ObservedEntryUnreadable {
        kind: io::ErrorKind,
        raw_os_error: Option<i32>,
    },
    /// The manifest validator refused an identity or the assembled candidate.
    Candidate(ManifestValidationError),
    /// The observed inventory could not be canonicalised for fingerprinting.
    InventorySerialization,
}

/// One managed artifact whose bytes THIS call read.
///
/// The only constructor streams the file and derives `sha256` and `byte_length`
/// from exactly those bytes, and it is the only way to reach
/// `ArtifactAvailability::Present` in the bootstrap. There is no code path from a
/// caller-supplied digest to `Present`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedManagedArtifact {
    kind: SessionArtifactKind,
    privacy_class: ArtifactPrivacyClass,
    managed_identity: ManagedArtifactIdentity,
    content: ArtifactContentIdentity,
}

impl ObservedManagedArtifact {
    /// Observe one managed artifact. `Ok(None)` means no entry exists at `path`.
    ///
    /// A non-regular entry is refused rather than followed: without that fence a
    /// symlink at a managed identity would be hashed and recorded `Present`
    /// under an identity it does not own.
    pub fn observe(
        kind: SessionArtifactKind,
        privacy_class: ArtifactPrivacyClass,
        managed_identity: ManagedArtifactIdentity,
        path: &Path,
    ) -> Result<Option<Self>, HistoricalBootstrapError> {
        let unreadable = |error: &io::Error| HistoricalBootstrapError::ObservedEntryUnreadable {
            kind: error.kind(),
            raw_os_error: error.raw_os_error(),
        };
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if !metadata.file_type().is_file() => {
                return Err(HistoricalBootstrapError::NonRegularObservedEntry);
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unreadable(&error)),
        }
        let mut file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unreadable(&error)),
        };
        if !file
            .metadata()
            .map_err(|error| unreadable(&error))?
            .file_type()
            .is_file()
        {
            return Err(HistoricalBootstrapError::NonRegularObservedEntry);
        }
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut byte_length = 0_u64;
        loop {
            let read = file.read(&mut buffer).map_err(|error| unreadable(&error))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            byte_length = byte_length.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        }
        let sha256 = Sha256Digest::new(format!("sha256:{:x}", hasher.finalize()))
            .map_err(HistoricalBootstrapError::Candidate)?;
        Ok(Some(Self {
            kind,
            privacy_class,
            managed_identity,
            content: ArtifactContentIdentity {
                sha256,
                byte_length,
            },
        }))
    }
}

/// Whether historical original-audio bytes were observable.
///
/// `NoObservableBytes` is the normal case: this product retains no Original
/// Session Audio, so no production writer creates any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoricalOriginalAudio {
    /// One observation of real audio bytes. Carrying the observation itself,
    /// rather than a content identity, is what makes the guarantee structural:
    /// the mandatory entry's `Present` availability and managed identity both
    /// come from bytes this process read, with no path from a caller-supplied
    /// digest.
    ObservedBytes(ObservedManagedArtifact),
    NoObservableBytes,
}

/// One entry of the historical per-Session managed inventory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoricalInventoryEntry {
    pub kind: SessionArtifactKind,
    pub privacy_class: ArtifactPrivacyClass,
    pub managed_identity: String,
}

/// The twelve durable per-Session artifact identities the bootstrap inspects.
///
/// CONSTRAINT the code cannot show: this mirrors
/// `sessions::default_session_artifact_paths` minus its six `*.json.tmp` write
/// sidecars, which are interrupted-write residue with no kind in the manifest
/// vocabulary. `sessions::tests::bootstrap_inventory_matches_the_live_session_artifact_inventory`
/// fails if either side drifts.
pub fn historical_managed_inventory(session_id: &str) -> Vec<HistoricalInventoryEntry> {
    let entry = |kind, privacy_class, managed_identity: String| HistoricalInventoryEntry {
        kind,
        privacy_class,
        managed_identity,
    };
    vec![
        entry(
            SessionArtifactKind::LegacyTranscript,
            ArtifactPrivacyClass::CanonicalSessionMemory,
            format!("transcripts/{session_id}.jsonl"),
        ),
        entry(
            SessionArtifactKind::TranscriptRevisions,
            ArtifactPrivacyClass::CanonicalSessionMemory,
            format!("transcripts/{session_id}.events.jsonl"),
        ),
        entry(
            SessionArtifactKind::SpeakerRevisions,
            ArtifactPrivacyClass::CanonicalSessionMemory,
            format!("transcripts/{session_id}.speaker.jsonl"),
        ),
        entry(
            SessionArtifactKind::ProjectionPatches,
            ArtifactPrivacyClass::CanonicalSessionMemory,
            format!("projections/{session_id}.events.jsonl"),
        ),
        entry(
            SessionArtifactKind::MaterializedNotes,
            ArtifactPrivacyClass::DerivedSessionMemory,
            format!("notes/{session_id}.json"),
        ),
        entry(
            SessionArtifactKind::LegacyGraph,
            ArtifactPrivacyClass::DerivedSessionMemory,
            format!("graphs/{session_id}.json"),
        ),
        entry(
            SessionArtifactKind::MaterializedGraph,
            ArtifactPrivacyClass::DerivedSessionMemory,
            format!("graphs/{session_id}.materialized.json"),
        ),
        entry(
            SessionArtifactKind::DataMovementLedger,
            ArtifactPrivacyClass::AuditRecord,
            format!("ledgers/{session_id}.movements.jsonl"),
        ),
        entry(
            SessionArtifactKind::SchedulerQueue,
            ArtifactPrivacyClass::OperationalMetadata,
            format!("projections/{session_id}.scheduler_queue.json"),
        ),
        entry(
            SessionArtifactKind::UsageLedger,
            ArtifactPrivacyClass::OperationalMetadata,
            format!("usage/{session_id}.json"),
        ),
        entry(
            SessionArtifactKind::LiveAssistAudit,
            ArtifactPrivacyClass::AuditRecord,
            format!("live_assist/{session_id}.jsonl"),
        ),
        entry(
            SessionArtifactKind::LiveAssistCurrent,
            ArtifactPrivacyClass::DerivedSessionMemory,
            format!("live_assist/{session_id}.current.json"),
        ),
    ]
}

/// Assemble one historical bootstrap candidate from observed bytes only.
///
/// The candidate's floor is `V1` — `SessionArtifactManifestV1::candidate`
/// hardcodes it and this function never raises it. Installing the candidate is a
/// `compare_and_swap`, which STAYS UNCALLED IN PRODUCTION per ADR-0044 §11 and
/// this seed's "no writer activation": nothing here writes a manifest.
pub fn historical_session_bootstrap_candidate(
    session_id: &str,
    idempotency_id: &str,
    observed: Vec<ObservedManagedArtifact>,
    original_audio: HistoricalOriginalAudio,
) -> Result<SessionArtifactManifestV1, HistoricalBootstrapError> {
    let identities = session_control_identities_for(session_id)
        .map_err(HistoricalBootstrapError::UnaddressableSession)?;
    let mut artifacts = Vec::with_capacity(observed.len() + 1);
    for artifact in observed {
        if artifact.kind == SessionArtifactKind::OriginalSessionAudio {
            return Err(HistoricalBootstrapError::ObservedOriginalSessionAudio);
        }
        refuse_reserved_observed_identity(&artifact.managed_identity, &identities)?;
        artifacts.push(SessionArtifactEntry {
            kind: artifact.kind,
            privacy_class: artifact.privacy_class,
            managed_identity: artifact.managed_identity,
            availability: ArtifactAvailability::Present {
                content: artifact.content,
            },
        });
    }
    let (audio_identity, audio_availability) = match original_audio {
        HistoricalOriginalAudio::ObservedBytes(observed) => {
            if observed.kind != SessionArtifactKind::OriginalSessionAudio
                || observed.privacy_class != ArtifactPrivacyClass::OriginalEvidence
            {
                return Err(
                    HistoricalBootstrapError::MisChanneledOriginalAudioObservation {
                        kind: observed.kind,
                        privacy_class: observed.privacy_class,
                    },
                );
            }
            refuse_reserved_observed_identity(&observed.managed_identity, &identities)?;
            (
                observed.managed_identity,
                ArtifactAvailability::Present {
                    content: observed.content,
                },
            )
        }
        // Not `RetentionDisabled`, `NeverCaptured`, `Expired`, `DeletedByUser`,
        // or `Inaccessible`: every one of those asserts a history this call did
        // not observe.
        HistoricalOriginalAudio::NoObservableBytes => (
            ManagedArtifactIdentity::new(HISTORICAL_ORIGINAL_AUDIO_IDENTITY)
                .map_err(HistoricalBootstrapError::Candidate)?,
            ArtifactAvailability::Unavailable {
                reason: ArtifactUnavailableReason::HistoricalUnknown,
            },
        ),
    };
    artifacts.push(SessionArtifactEntry {
        kind: SessionArtifactKind::OriginalSessionAudio,
        privacy_class: ArtifactPrivacyClass::OriginalEvidence,
        managed_identity: audio_identity,
        availability: audio_availability,
    });
    artifacts.sort_by(|left, right| {
        left.managed_identity
            .cmp(&right.managed_identity)
            .then_with(|| left.kind.cmp(&right.kind))
    });
    let fingerprint = historical_inventory_fingerprint(session_id, &artifacts)?;
    SessionArtifactManifestV1::candidate(
        session_id,
        ManifestTransition {
            idempotency_id: idempotency_id.to_owned(),
            fingerprint,
            state: ManifestTransitionState::Completed,
        },
        artifacts,
        None,
    )
    .map_err(HistoricalBootstrapError::Candidate)
}

/// Walk the live per-Session inventory, hashing only files that exist.
///
/// The inventory contains no audio identity — the live consumer artifact enum has
/// no audio member and `sessions::default_session_artifact_paths` lists no audio
/// path — so this always records the mandatory audio entry as
/// `Unavailable { reason: HistoricalUnknown }`.
pub fn historical_session_bootstrap_from_live_inventory(
    data_root: &Path,
    session_id: &str,
    idempotency_id: &str,
) -> Result<SessionArtifactManifestV1, HistoricalBootstrapError> {
    let mut observed = Vec::new();
    for entry in historical_managed_inventory(session_id) {
        let identity = ManagedArtifactIdentity::new(entry.managed_identity.clone())
            .map_err(HistoricalBootstrapError::Candidate)?;
        let path = data_root.join(&entry.managed_identity);
        if let Some(artifact) =
            ObservedManagedArtifact::observe(entry.kind, entry.privacy_class, identity, &path)?
        {
            observed.push(artifact);
        }
    }
    historical_session_bootstrap_candidate(
        session_id,
        idempotency_id,
        observed,
        HistoricalOriginalAudio::NoObservableBytes,
    )
}

/// Reserve every flat-root control and index identity against a caller-supplied
/// observed identity.
///
/// `validate_managed_identity` does NOT reserve these: `is_internal_identity`
/// compares only the three root-wide constants, so
/// `.audio-graph-session-<key>-artifacts.v1.json` passes every one of its
/// checks. This closes that gap at the bootstrap builder. Classification uses
/// only DERIVED public identities, never a duplicated filename prefix.
///
/// The manifest VALIDATOR now reserves the same names too, in
/// `session_artifact_manifest::refuse_reserved_control_identities`, which is what
/// covers the `compare_and_swap` and load paths this builder is not on. Since
/// audio-graph-f629 closed residual R10, that validator also refuses ANOTHER
/// Session's control identity, by an address-independent namespace ban rather than
/// by this function's allow-list.
///
/// This check still earns its place, on two counts. It runs before the candidate
/// exists, so it gives three distinct classifications (`SessionIndexIdentity`,
/// `SessionControlIdentity`, `IdentityOutsideManagedArtifactTree`) where the
/// validator gives one. And `MANAGED_ARTIFACT_ROOTS` is strictly stronger for THIS
/// builder's inputs: it refuses every flat-root and unknown-root name, not just
/// the reserved ones, which is correct here because this builder's whole inventory
/// comes from `historical_managed_inventory` and lives under those roots. The
/// validator cannot adopt that allow-list, because it is the common seam for
/// producers whose identities legitimately sit outside this list — `canonical_log`
/// inventories flat-root `events.jsonl` and `streams/events.jsonl`.
///
/// The store-owned coordination identity needs no arm of its own: it is the one
/// flat-root name `ManagedArtifactIdentity::new` already refuses, as
/// `ReservedInternalIdentity`, so it cannot reach an `ObservedManagedArtifact`.
/// Were that ever to change, it has no `/` and would land in
/// `IdentityOutsideManagedArtifactTree`.
fn refuse_reserved_observed_identity(
    identity: &ManagedArtifactIdentity,
    identities: &SessionControlIdentities,
) -> Result<(), HistoricalBootstrapError> {
    let value = identity.as_str();
    if value == SESSIONS_INDEX_IDENTITY {
        return Err(HistoricalBootstrapError::SessionIndexIdentity);
    }
    if value == identities.manifest.as_str()
        || value == identities.temporary.as_str()
        || value == identities.provenance.as_str()
    {
        return Err(HistoricalBootstrapError::SessionControlIdentity);
    }
    let first = value.split('/').next().unwrap_or_default();
    if first == value || !MANAGED_ARTIFACT_ROOTS.contains(&first) {
        return Err(HistoricalBootstrapError::IdentityOutsideManagedArtifactTree);
    }
    Ok(())
}

/// Fingerprint the canonicalised observed inventory, so an unchanged rerun of the
/// bootstrap produces a byte-identical candidate and the manifest CAS reports
/// `AlreadyCompleted` rather than a conflict.
fn historical_inventory_fingerprint(
    session_id: &str,
    artifacts: &[SessionArtifactEntry],
) -> Result<Sha256Digest, HistoricalBootstrapError> {
    let encoded = serde_json::to_vec(artifacts)
        .map_err(|_| HistoricalBootstrapError::InventorySerialization)?;
    let mut hasher = Sha256::new();
    hasher.update(HISTORICAL_BOOTSTRAP_FINGERPRINT_DOMAIN);
    hasher.update(session_id.as_bytes());
    hasher.update([0_u8]);
    hasher.update(&encoded);
    Sha256Digest::new(format!("sha256:{:x}", hasher.finalize()))
        .map_err(HistoricalBootstrapError::Candidate)
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

#[cfg(test)]
mod guarded_admission_tests {
    use super::{
        AdmittedSessionFloor, GuardedSessionOpenError, HISTORICAL_ORIGINAL_AUDIO_IDENTITY,
        HistoricalBootstrapError, HistoricalOriginalAudio, ObservedManagedArtifact,
        SessionControlPlaneExportError, SessionControlPlaneRetirement, SessionFloorEvidence,
        SessionSemanticsAdmissionError, SessionSemanticsVersion, admit_session_semantics_v1_to_v2,
        admitted_session_semantics_floor, export_session_control_plane, guarded_session_open,
        historical_session_bootstrap_candidate, historical_session_bootstrap_from_live_inventory,
        open_session_for_content, retire_observed_session_control_plane,
        session_control_plane_paths, store_coordination_path,
    };
    use crate::persistence::canonical_durability::{
        CanonicalDurabilityRejection, CanonicalPlatform,
    };
    use crate::persistence::session_artifact_manifest::{
        ArtifactAvailability, ArtifactContentIdentity, ArtifactPrivacyClass,
        ArtifactUnavailableReason, ManagedArtifactIdentity, ManifestCasOutcome,
        ManifestCasRejection, ManifestLoadOutcome, ManifestStoreError, ManifestTransition,
        ManifestTransitionState, ManifestValidationError, SessionArtifactEntry,
        SessionArtifactKind, SessionArtifactManifestStore, SessionArtifactManifestV1,
        SessionSemanticsTransitionProofV1, Sha256Digest,
    };
    use sha2::{Digest as _, Sha256};
    use std::cell::Cell;
    use std::path::{Path, PathBuf};

    const SESSION: &str = "e8e7-session";

    fn root(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-e8e7-{}-{}-{}",
            label,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).expect("fixture root");
        path
    }

    fn identity(value: &str) -> ManagedArtifactIdentity {
        ManagedArtifactIdentity::new(value).expect("managed identity")
    }

    fn digest(byte: char) -> Sha256Digest {
        Sha256Digest::new(format!("sha256:{}", byte.to_string().repeat(64))).expect("digest")
    }

    fn v2_candidate(idempotency_id: &str) -> SessionArtifactManifestV1 {
        let mut candidate = SessionArtifactManifestV1::candidate(
            SESSION,
            ManifestTransition {
                idempotency_id: idempotency_id.to_owned(),
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
        .expect("v2 candidate");
        candidate.session_semantics_version = SessionSemanticsVersion::V2;
        candidate
    }

    /// A genuine, durably Accepted later generation of an advanced Session
    /// PRESERVES the committed floor. Nothing about it is an illegal transition.
    #[cfg(unix)]
    #[test]
    fn accepted_later_generation_preserves_the_committed_floor() {
        let root = root("accepted-preserves-floor");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, SESSION)
            .expect("qualified addressed store");
        let mut transaction = store.begin_write().expect("transaction");
        let advanced = match transaction.advance_session_semantics_v1_to_v2(
            0,
            v2_candidate("advance-1"),
            SessionSemanticsTransitionProofV1::v1_to_v2(SESSION, "advance-1").expect("proof"),
        ) {
            ManifestCasOutcome::Accepted { manifest, .. } => manifest,
            other => panic!("expected the advance to be accepted, got {other:?}"),
        };
        assert_eq!(advanced.generation, 1);

        let mut later = advanced.clone();
        later.generation = 0;
        later.transition.idempotency_id = "later-generation-1".to_owned();
        later.artifacts.push(SessionArtifactEntry {
            kind: SessionArtifactKind::MaterializedNotes,
            privacy_class: ArtifactPrivacyClass::DerivedSessionMemory,
            managed_identity: identity("notes/e8e7.json"),
            availability: ArtifactAvailability::Present {
                content: ArtifactContentIdentity {
                    sha256: digest('c'),
                    byte_length: 12,
                },
            },
        });
        let accepted = transaction.compare_and_swap(1, later);
        assert!(
            matches!(&accepted, ManifestCasOutcome::Accepted { manifest, .. }
                if manifest.generation == 2
                    && manifest.session_semantics_version == SessionSemanticsVersion::V2),
            "expected a genuine later V2 generation to be accepted, got {accepted:?}"
        );

        assert_eq!(
            admitted_session_semantics_floor(SESSION, SessionSemanticsVersion::V2, &accepted),
            Ok(SessionSemanticsVersion::V2),
            "a durably Accepted later generation preserves the committed V2 floor"
        );
    }

    /// The same preservation on a V1 Session: an ordinary Accepted V1 generation
    /// is not an illegal transition either.
    #[cfg(unix)]
    #[test]
    fn accepted_v1_generation_preserves_the_v1_floor() {
        let root = root("accepted-preserves-v1");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, SESSION)
            .expect("qualified addressed store");
        let mut transaction = store.begin_write().expect("transaction");
        let candidate = SessionArtifactManifestV1::candidate(
            SESSION,
            ManifestTransition {
                idempotency_id: "v1-generation-1".to_owned(),
                fingerprint: digest('a'),
                state: ManifestTransitionState::Completed,
            },
            vec![SessionArtifactEntry {
                kind: SessionArtifactKind::OriginalSessionAudio,
                privacy_class: ArtifactPrivacyClass::OriginalEvidence,
                managed_identity: identity("audio/original.wav"),
                availability: ArtifactAvailability::Unavailable {
                    reason: ArtifactUnavailableReason::RetentionDisabled,
                },
            }],
            None,
        )
        .expect("v1 candidate");
        let accepted = transaction.compare_and_swap(0, candidate);
        assert!(matches!(&accepted, ManifestCasOutcome::Accepted { .. }));

        assert_eq!(
            admitted_session_semantics_floor(SESSION, SessionSemanticsVersion::V1, &accepted),
            Ok(SessionSemanticsVersion::V1),
        );
    }

    const SESSION_OTHER: &str = "e8e7-other";

    fn write_fixture(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture parent");
        }
        std::fs::write(path, bytes).expect("fixture bytes");
    }

    fn v1_candidate(idempotency_id: &str) -> SessionArtifactManifestV1 {
        SessionArtifactManifestV1::candidate(
            SESSION,
            ManifestTransition {
                idempotency_id: idempotency_id.to_owned(),
                fingerprint: digest('a'),
                state: ManifestTransitionState::Completed,
            },
            vec![SessionArtifactEntry {
                kind: SessionArtifactKind::OriginalSessionAudio,
                privacy_class: ArtifactPrivacyClass::OriginalEvidence,
                managed_identity: identity(HISTORICAL_ORIGINAL_AUDIO_IDENTITY),
                availability: ArtifactAvailability::Unavailable {
                    reason: ArtifactUnavailableReason::HistoricalUnknown,
                },
            }],
            None,
        )
        .expect("v1 candidate")
    }

    /// Hand-install one persisted manifest at a Session's derived control
    /// identity, for stores whose platform cannot mutate the namespace.
    fn persist_manifest(root: &Path, session_id: &str, mut manifest: SessionArtifactManifestV1) {
        manifest.generation = 1;
        let paths = session_control_plane_paths(root, session_id).expect("control paths");
        write_fixture(
            &paths.manifest,
            &serde_json::to_vec(&manifest).expect("persisted wire"),
        );
    }

    /// A Session advanced to a durable V2 floor, with its write transaction
    /// released so the exclusive guard is free.
    fn advanced_v2_session(label: &str) -> (PathBuf, SessionArtifactManifestStore, Vec<u8>) {
        let root = root(label);
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, SESSION)
            .expect("qualified addressed store");
        let mut transaction = store.begin_write().expect("transaction");
        let advanced = transaction.advance_session_semantics_v1_to_v2(
            0,
            v2_candidate("advance-1"),
            SessionSemanticsTransitionProofV1::v1_to_v2(SESSION, "advance-1").expect("proof"),
        );
        assert!(
            matches!(&advanced, ManifestCasOutcome::Accepted { manifest, .. }
                if manifest.session_semantics_version == SessionSemanticsVersion::V2),
            "the fixture advance must be accepted, got {advanced:?}"
        );
        drop(transaction);
        let paths = session_control_plane_paths(&root, SESSION).expect("control paths");
        let proof_bytes = std::fs::read(&paths.provenance).expect("durable transition proof");
        (root, store, proof_bytes)
    }

    // -- acceptance (3): the actual CAS outcome ------------------------------

    /// The admission wrapper classifies the outcome the advance actually
    /// produced, and forwards a refusal verbatim instead of reporting a floor.
    #[cfg(unix)]
    #[test]
    fn admission_passes_only_the_real_cas_outcome() {
        let root = root("admission-real-outcome");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, SESSION)
            .expect("qualified addressed store");
        let mut transaction = store.begin_write().expect("transaction");
        assert_eq!(
            admit_session_semantics_v1_to_v2(
                &mut transaction,
                SESSION,
                SessionSemanticsVersion::V1,
                0,
                v2_candidate("advance-1"),
                SessionSemanticsTransitionProofV1::v1_to_v2(SESSION, "advance-1").expect("proof"),
            ),
            Ok(SessionSemanticsVersion::V2)
        );

        // A second advance whose proof bytes differ from the durable record is
        // refused by the immutable-exact preflight. The refusal is forwarded
        // verbatim and no floor is reported.
        assert_eq!(
            admit_session_semantics_v1_to_v2(
                &mut transaction,
                SESSION,
                SessionSemanticsVersion::V2,
                1,
                v2_candidate("advance-2"),
                SessionSemanticsTransitionProofV1::v1_to_v2(SESSION, "advance-2").expect("proof"),
            ),
            Err(SessionSemanticsAdmissionError::Refused(
                ManifestCasRejection::Durability(
                    CanonicalDurabilityRejection::ImmutableExactConflict
                )
            ))
        );
    }

    // -- acceptance (2): guard-owned open -----------------------------------

    #[cfg(unix)]
    #[test]
    fn v2_session_bytes_never_reach_a_v1_only_content_reader() {
        let (_root, store, _proof) = advanced_v2_session("v2-gated-reader");
        let reader_invoked = Cell::new(false);
        assert_eq!(
            guarded_session_open(&store, SessionSemanticsVersion::V1, |_floor| {
                reader_invoked.set(true);
                Ok::<&'static str, ()>("content")
            }),
            Err(GuardedSessionOpenError::UnsupportedSessionFloor {
                required: SessionSemanticsVersion::V2,
                maximum_supported: SessionSemanticsVersion::V1,
            })
        );
        assert!(
            !reader_invoked.get(),
            "a v1-only content reader must never observe v2 Session bytes"
        );

        // The live reason the wrapper exists: the bare store read hands the same
        // manifest to its closure with no compatibility-floor check at all.
        let bare_closure_invoked = Cell::new(false);
        let observed_floor = store.checked_read(|head| {
            bare_closure_invoked.set(true);
            match head {
                ManifestLoadOutcome::Present(manifest) => {
                    Ok::<_, ()>(manifest.session_semantics_version)
                }
                ManifestLoadOutcome::Absent => panic!("the advanced head disappeared"),
            }
        });
        assert_eq!(observed_floor, Ok(SessionSemanticsVersion::V2));
        assert!(bare_closure_invoked.get());
    }

    #[cfg(unix)]
    #[test]
    fn guarded_open_admits_historical_absence_only_under_the_guard() {
        let root = root("guarded-historical-absence");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, SESSION)
            .expect("qualified addressed store");
        let invocations = Cell::new(0_u32);
        let admitted = guarded_session_open(&store, SessionSemanticsVersion::V1, |floor| {
            invocations.set(invocations.get() + 1);
            Ok::<_, ()>(floor)
        })
        .expect("an absent manifest under the guard admits historical v1");
        assert_eq!(
            admitted,
            AdmittedSessionFloor {
                floor: SessionSemanticsVersion::V1,
                evidence: SessionFloorEvidence::GuardedAbsence,
            }
        );
        assert_eq!(invocations.get(), 1);
        // Documented side effect of the qualified branch: the read establishes
        // the store-owned coordination entry when it is missing.
        assert!(
            store_coordination_path(&root, SESSION)
                .expect("coordination path")
                .exists(),
            "the qualified guarded read establishes the store-owned lock"
        );
    }

    #[cfg(unix)]
    #[test]
    fn legacy_and_canonical_readers_are_both_gated() {
        let (_root, store, _proof) = advanced_v2_session("both-readers-gated");
        for label in ["legacy-jsonl-transcript", "canonical-revision-replay"] {
            let invoked = Cell::new(false);
            let refusal = guarded_session_open(&store, SessionSemanticsVersion::V1, |_floor| {
                invoked.set(true);
                Ok::<&'static str, ()>(label)
            });
            assert_eq!(
                refusal,
                Err(GuardedSessionOpenError::UnsupportedSessionFloor {
                    required: SessionSemanticsVersion::V2,
                    maximum_supported: SessionSemanticsVersion::V1,
                }),
                "the {label} reader must be refused before invocation"
            );
            assert!(!invoked.get(), "the {label} reader ran");
        }
    }

    #[test]
    fn open_session_for_content_refuses_an_unsupported_reader_floor_on_a_bare_root() {
        // Review found this refusal unreachable on the no-control-plane branch —
        // the branch every Session takes at this base. Verified by removing the
        // guard: a bare root then returned Ok(floor V1, UnguardedObservedAbsence)
        // and RAN the reader for a caller declaring floor 0.
        let root = root("bare-root-unsupported-reader");
        let invoked = std::cell::Cell::new(false);
        let error =
            open_session_for_content(&root, SESSION, SessionSemanticsVersion(0), |_admitted| {
                invoked.set(true);
                Ok::<(), ()>(())
            })
            .expect_err("an unsupported reader floor is refused before any admission");
        assert!(matches!(
            error,
            GuardedSessionOpenError::UnsupportedReaderFloor { actual: 0 }
        ));
        assert!(
            !invoked.get(),
            "the reader ran despite an unsupported floor"
        );
        assert_eq!(
            std::fs::read_dir(&root).expect("root readable").count(),
            0,
            "the refusal created no control-plane state"
        );
    }

    #[test]
    fn open_session_for_content_admits_a_bare_root_without_touching_it() {
        let root = root("bare-root-open");
        let admitted =
            open_session_for_content(&root, SESSION, SessionSemanticsVersion::V1, Ok::<_, ()>)
                .expect("a root with no control plane admits historical v1");
        assert_eq!(
            admitted,
            AdmittedSessionFloor {
                floor: SessionSemanticsVersion::V1,
                evidence: SessionFloorEvidence::UnguardedObservedAbsence,
            }
        );
        assert_eq!(
            std::fs::read_dir(&root).expect("root readable").count(),
            0,
            "the read created no lock, no manifest, and no temporary"
        );
    }

    #[cfg(unix)]
    #[test]
    fn open_session_for_content_refuses_residue_on_an_unqualifiable_root() {
        let root = root("residue-refusal");
        let paths = session_control_plane_paths(&root, SESSION).expect("control paths");
        write_fixture(&paths.manifest, b"not a manifest");
        let invoked = Cell::new(false);
        let refusal =
            open_session_for_content(&root, SESSION, SessionSemanticsVersion::V1, |_floor| {
                invoked.set(true);
                Ok::<&'static str, ()>("content")
            });
        assert!(
            matches!(
                refusal,
                Err(GuardedSessionOpenError::ControlPlaneStore(_)
                    | GuardedSessionOpenError::ControlPlaneUnreadable(_))
            ),
            "control-plane residue that cannot be read under a guard is a classified refusal, got {refusal:?}"
        );
        assert!(!invoked.get(), "the content reader ran on refused state");
    }

    // -- acceptance (1): historical bootstrap -------------------------------

    #[test]
    fn historical_bootstrap_records_present_only_from_observed_bytes() {
        let root = root("bootstrap-observed-bytes");
        let transcript_identity = format!("transcripts/{SESSION}.jsonl");
        let notes_identity = format!("notes/{SESSION}.json");
        write_fixture(&root.join(&transcript_identity), b"{\"seq\":1}\n");
        write_fixture(&root.join(&notes_identity), b"{}");

        let candidate =
            historical_session_bootstrap_from_live_inventory(&root, SESSION, "bootstrap-1")
                .expect("live-inventory bootstrap");

        for wanted in [&transcript_identity, &notes_identity] {
            let bytes = std::fs::read(root.join(wanted)).expect("fixture bytes");
            let expected = ArtifactContentIdentity {
                sha256: Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(&bytes)))
                    .expect("independent digest"),
                byte_length: u64::try_from(bytes.len()).expect("fixture length"),
            };
            let entry = candidate
                .artifacts
                .iter()
                .find(|artifact| artifact.managed_identity.as_str() == wanted.as_str())
                .expect("observed entry");
            assert_eq!(
                entry.availability,
                ArtifactAvailability::Present { content: expected },
                "{wanted} must be Present from exactly the bytes on disk"
            );
        }

        assert!(
            !candidate
                .artifacts
                .iter()
                .any(|artifact| artifact.managed_identity.as_str()
                    == format!("graphs/{SESSION}.json")),
            "a NotFound inventory path yields no entry at all"
        );
        assert_eq!(
            candidate.artifacts.len(),
            3,
            "two observed artifacts plus the mandatory audio entry"
        );
        assert_eq!(
            candidate.session_semantics_version,
            SessionSemanticsVersion::V1
        );
    }

    #[cfg(unix)]
    #[test]
    fn historical_bootstrap_refuses_non_regular_observed_entries() {
        let root = root("bootstrap-non-regular");
        let target = root.join("outside-the-managed-inventory");
        write_fixture(&target, b"forged bytes");
        let link = root.join("transcripts").join(format!("{SESSION}.jsonl"));
        std::fs::create_dir_all(link.parent().expect("parent")).expect("transcripts dir");
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        assert_eq!(
            historical_session_bootstrap_from_live_inventory(&root, SESSION, "bootstrap-1"),
            Err(HistoricalBootstrapError::NonRegularObservedEntry),
            "a symlink at a managed identity is refused, never hashed as Present"
        );
    }

    #[test]
    fn historical_bootstrap_original_audio_is_unknown_not_fabricated() {
        let root = root("bootstrap-audio-unknown");
        let candidate =
            historical_session_bootstrap_from_live_inventory(&root, SESSION, "bootstrap-1")
                .expect("live-inventory bootstrap");
        let audio = candidate
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == SessionArtifactKind::OriginalSessionAudio)
            .expect("the mandatory Original Session Audio entry");
        assert_eq!(
            audio.managed_identity.as_str(),
            HISTORICAL_ORIGINAL_AUDIO_IDENTITY
        );
        assert_eq!(audio.privacy_class, ArtifactPrivacyClass::OriginalEvidence);
        assert_eq!(
            audio.availability,
            ArtifactAvailability::Unavailable {
                reason: ArtifactUnavailableReason::HistoricalUnknown,
            }
        );
        for fabricated in [
            ArtifactUnavailableReason::RetentionDisabled,
            ArtifactUnavailableReason::NeverCaptured,
            ArtifactUnavailableReason::Expired,
            ArtifactUnavailableReason::DeletedByUser,
            ArtifactUnavailableReason::Inaccessible,
        ] {
            assert_ne!(
                audio.availability,
                ArtifactAvailability::Unavailable { reason: fabricated },
                "bootstrap must not infer {fabricated:?}"
            );
        }

        let wire = serde_json::to_string(&candidate).expect("candidate wire");
        assert!(
            wire.contains("\"historical_unknown\""),
            "the historical-unknown state needs its own wire value: {wire}"
        );
        assert_eq!(
            serde_json::from_str::<SessionArtifactManifestV1>(&wire).expect("wire round trip"),
            candidate
        );
    }

    #[test]
    fn historical_bootstrap_records_observed_audio_bytes_when_they_exist() {
        let root = root("bootstrap-observed-audio");
        let audio_identity = "audio/original.wav";
        let audio_path = root.join(audio_identity);
        write_fixture(&audio_path, b"RIFF-observed-fixture");
        let observed = ObservedManagedArtifact::observe(
            SessionArtifactKind::OriginalSessionAudio,
            ArtifactPrivacyClass::OriginalEvidence,
            identity(audio_identity),
            &audio_path,
        )
        .expect("observe")
        .expect("audio bytes exist");

        // The observed-artifact list is not the channel for the mandatory entry.
        assert_eq!(
            historical_session_bootstrap_candidate(
                SESSION,
                "bootstrap-1",
                vec![observed.clone()],
                HistoricalOriginalAudio::NoObservableBytes,
            ),
            Err(HistoricalBootstrapError::ObservedOriginalSessionAudio)
        );

        // And the mirror, which review found was silently coerced rather than
        // refused: an entry routed through the audio channel that classifies
        // itself as something else does not get its kind overwritten.
        let mis_channeled = ObservedManagedArtifact::observe(
            SessionArtifactKind::LegacyTranscript,
            ArtifactPrivacyClass::DerivedSessionMemory,
            identity(audio_identity),
            &audio_path,
        )
        .expect("observe")
        .expect("audio bytes exist");
        assert_eq!(
            historical_session_bootstrap_candidate(
                SESSION,
                "bootstrap-1",
                vec![],
                HistoricalOriginalAudio::ObservedBytes(mis_channeled),
            ),
            Err(
                HistoricalBootstrapError::MisChanneledOriginalAudioObservation {
                    kind: SessionArtifactKind::LegacyTranscript,
                    privacy_class: ArtifactPrivacyClass::DerivedSessionMemory,
                }
            )
        );

        let bytes = std::fs::read(&audio_path).expect("fixture bytes");
        let expected = ArtifactContentIdentity {
            sha256: Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(&bytes)))
                .expect("independent digest"),
            byte_length: u64::try_from(bytes.len()).expect("fixture length"),
        };
        let candidate = historical_session_bootstrap_candidate(
            SESSION,
            "bootstrap-1",
            Vec::new(),
            HistoricalOriginalAudio::ObservedBytes(observed),
        )
        .expect("candidate with observed audio");
        let audio = candidate
            .artifacts
            .iter()
            .find(|artifact| artifact.kind == SessionArtifactKind::OriginalSessionAudio)
            .expect("the mandatory Original Session Audio entry");
        assert_eq!(audio.managed_identity.as_str(), audio_identity);
        assert_eq!(
            audio.availability,
            ArtifactAvailability::Present { content: expected }
        );
    }

    #[test]
    fn historical_bootstrap_refuses_control_and_index_identities() {
        let root = root("bootstrap-reserved-identities");
        let own =
            crate::persistence::session_artifact_manifest::session_control_identities_for(SESSION)
                .expect("own control identities");
        let other = crate::persistence::session_artifact_manifest::session_control_identities_for(
            SESSION_OTHER,
        )
        .expect("other control identities");

        // The store-owned lock cannot even become a managed identity: the
        // manifest module already reserves it by name.
        assert_eq!(
            ManagedArtifactIdentity::new(own.coordination.as_str()),
            Err(ManifestValidationError::ReservedInternalIdentity)
        );

        for (candidate_identity, expected) in [
            (
                own.manifest.as_str().to_owned(),
                HistoricalBootstrapError::SessionControlIdentity,
            ),
            (
                own.temporary.as_str().to_owned(),
                HistoricalBootstrapError::SessionControlIdentity,
            ),
            (
                own.provenance.as_str().to_owned(),
                HistoricalBootstrapError::SessionControlIdentity,
            ),
            (
                other.manifest.as_str().to_owned(),
                HistoricalBootstrapError::IdentityOutsideManagedArtifactTree,
            ),
            (
                "sessions.json".to_owned(),
                HistoricalBootstrapError::SessionIndexIdentity,
            ),
        ] {
            let path = root.join(&candidate_identity);
            write_fixture(&path, b"residue");
            let observed = ObservedManagedArtifact::observe(
                SessionArtifactKind::SessionMetadata,
                ArtifactPrivacyClass::OperationalMetadata,
                ManagedArtifactIdentity::new(candidate_identity.clone())
                    .expect("wire-valid identity"),
                &path,
            )
            .expect("observe")
            .expect("residue exists");
            assert_eq!(
                historical_session_bootstrap_candidate(
                    SESSION,
                    "bootstrap-1",
                    vec![observed],
                    HistoricalOriginalAudio::NoObservableBytes,
                ),
                Err(expected),
                "identity {candidate_identity} must be refused with its own classification"
            );
        }

        // The accepted set is exactly the known-subdirectory identities.
        let accepted_identity = format!("transcripts/{SESSION}.jsonl");
        let accepted_path = root.join(&accepted_identity);
        write_fixture(&accepted_path, b"{}\n");
        let accepted = ObservedManagedArtifact::observe(
            SessionArtifactKind::LegacyTranscript,
            ArtifactPrivacyClass::CanonicalSessionMemory,
            identity(&accepted_identity),
            &accepted_path,
        )
        .expect("observe")
        .expect("fixture exists");
        assert!(
            historical_session_bootstrap_candidate(
                SESSION,
                "bootstrap-1",
                vec![accepted],
                HistoricalOriginalAudio::NoObservableBytes,
            )
            .is_ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn historical_bootstrap_candidate_is_accepted_by_the_manifest_validator() {
        let root = root("bootstrap-validator-accepts");
        write_fixture(
            &root.join(format!("transcripts/{SESSION}.jsonl")),
            b"{\"seq\":1}\n",
        );
        let candidate =
            historical_session_bootstrap_from_live_inventory(&root, SESSION, "bootstrap-1")
                .expect("live-inventory bootstrap");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, SESSION)
            .expect("qualified addressed store");
        let mut transaction = store.begin_write().expect("transaction");
        let accepted = transaction.compare_and_swap(0, candidate.clone());
        assert!(
            matches!(&accepted, ManifestCasOutcome::Accepted { manifest, .. }
                if manifest.generation == 1
                    && manifest.session_semantics_version == SessionSemanticsVersion::V1),
            "the bootstrap candidate must install at floor v1, got {accepted:?}"
        );

        let rerun = historical_session_bootstrap_from_live_inventory(&root, SESSION, "bootstrap-1")
            .expect("unchanged rerun");
        assert_eq!(
            rerun, candidate,
            "an unchanged rerun must be byte-identical for idempotence"
        );
        assert!(
            matches!(
                transaction.compare_and_swap(1, rerun),
                ManifestCasOutcome::AlreadyCompleted { .. }
            ),
            "an unchanged rerun is the idempotent AlreadyCompleted"
        );
    }

    // -- acceptance (4): per-Session control-plane parity -------------------

    #[test]
    fn session_control_plane_paths_exclude_the_store_owned_lock() {
        let root = root("control-paths-exclude-lock");
        let paths = session_control_plane_paths(&root, SESSION).expect("control paths");
        let coordination = store_coordination_path(&root, SESSION).expect("coordination path");
        assert_eq!(paths.all().len(), 3);
        for path in paths.all() {
            assert_ne!(
                path, coordination,
                "no Session-owned control path may be the store-owned lock"
            );
        }
        assert_eq!(
            coordination.file_name().expect("lock basename"),
            std::ffi::OsStr::new(".audio-graph-canonical.lock")
        );
        let store = SessionArtifactManifestStore::for_session(&root, SESSION).expect("store");
        assert_eq!(
            coordination,
            root.join(store.control_identities().coordination.as_str()),
            "the coordination identity is the shared store-owned lock"
        );
    }

    #[cfg(unix)]
    #[test]
    fn control_plane_retirement_removes_all_three_and_nothing_else() {
        let (root, store, _proof) = advanced_v2_session("retire-only-three");
        let paths = session_control_plane_paths(&root, SESSION).expect("control paths");
        write_fixture(&paths.temporary, b"staged intent");
        let coordination = store_coordination_path(&root, SESSION).expect("coordination path");
        let index = root.join("sessions.json");
        write_fixture(&index, b"[]");
        let other = session_control_plane_paths(&root, SESSION_OTHER).expect("other paths");
        for path in other.all() {
            write_fixture(path, b"other session control bytes");
        }
        let lock_bytes = std::fs::read(&coordination).expect("lock bytes");

        assert_eq!(
            retire_observed_session_control_plane(&store),
            SessionControlPlaneRetirement::Retired
        );

        for path in paths.all() {
            assert!(
                !path.exists(),
                "retirement must remove every Session-owned control entry: {path:?}"
            );
        }
        assert_eq!(
            std::fs::read(&coordination).expect("lock survives"),
            lock_bytes,
            "the store-owned lock is never retired"
        );
        assert_eq!(std::fs::read(&index).expect("index survives"), b"[]");
        for path in other.all() {
            assert_eq!(
                std::fs::read(path).expect("other Session survives"),
                b"other session control bytes",
                "another Session's control entries are unreachable: {path:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn control_plane_temporary_is_retired_through_abandon() {
        let root = root("retire-staged-temporary");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, SESSION)
            .expect("qualified addressed store");
        let paths = session_control_plane_paths(&root, SESSION).expect("control paths");
        {
            let mut transaction = store.begin_write().expect("transaction");
            write_fixture(&paths.temporary, b"staged intent");
            assert_eq!(
                transaction.compare_and_swap(0, v1_candidate("wedged-1")),
                ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(
                    CanonicalDurabilityRejection::SnapshotTempAlreadyExists
                )),
                "a staged temporary wedges every install until it is retired"
            );
        }

        assert_eq!(
            retire_observed_session_control_plane(&store),
            SessionControlPlaneRetirement::Retired
        );
        assert!(!paths.temporary.exists());

        let mut transaction = store.begin_write().expect("transaction");
        assert!(
            matches!(
                transaction.compare_and_swap(0, v1_candidate("unwedged-1")),
                ManifestCasOutcome::Accepted { .. }
            ),
            "retiring the temporary unwedges a later compare-and-swap"
        );
    }

    #[cfg(unix)]
    #[test]
    fn control_plane_retirement_without_a_guard_is_a_residual_not_a_silent_skip() {
        let root = root("retire-without-a-guard");
        let store = SessionArtifactManifestStore::for_test_session_platform(
            &root,
            SESSION,
            CanonicalPlatform::Windows,
        )
        .expect("windows-platform store");
        let paths = session_control_plane_paths(&root, SESSION).expect("control paths");
        for path in paths.all() {
            write_fixture(path, b"control residue");
        }

        let retirement = retire_observed_session_control_plane(&store);
        let SessionControlPlaneRetirement::Residual { failures } = &retirement else {
            panic!(
                "a platform with no durable removal path must report a residual, got {retirement:?}"
            );
        };
        assert_eq!(failures.len(), 1);
        assert!(
            failures[0].contains("NamespaceQualificationRequired"),
            "the residual must name the classified reason: {}",
            failures[0]
        );
        for path in paths.all() {
            assert!(
                path.exists(),
                "a refused retirement must not remove anything: {path:?}"
            );
        }
    }

    /// Delete parity must survive a head the manifest loader cannot read. If
    /// retirement went through `begin_write` this Session would be undeletable
    /// forever.
    #[cfg(unix)]
    #[test]
    fn control_plane_retirement_ignores_an_unreadable_head() {
        let root = root("retire-unreadable-head");
        let store = SessionArtifactManifestStore::qualified_for_test_session(&root, SESSION)
            .expect("qualified addressed store");
        let paths = session_control_plane_paths(&root, SESSION).expect("control paths");
        write_fixture(&paths.manifest, b"{ truncated");
        assert!(
            matches!(store.begin_write(), Err(ManifestStoreError::Load(_))),
            "the write path cannot even open a truncated head"
        );

        assert_eq!(
            retire_observed_session_control_plane(&store),
            SessionControlPlaneRetirement::Retired
        );
        assert!(!paths.manifest.exists());
    }

    #[cfg(unix)]
    #[test]
    fn control_plane_export_carries_manifest_and_proof_but_never_the_lock() {
        let (root, store, proof_bytes) = advanced_v2_session("export-manifest-and-proof");
        let coordination = store_coordination_path(&root, SESSION).expect("coordination path");
        let lock_bytes = std::fs::read(&coordination).expect("lock bytes");

        let export = export_session_control_plane(&store).expect("guarded export");
        let manifest = export.manifest.as_ref().expect("exported manifest");
        assert_eq!(manifest.session_id, SESSION);
        assert_eq!(
            manifest.session_semantics_version,
            SessionSemanticsVersion::V2
        );
        assert_eq!(
            export.proof.as_deref(),
            Some(proof_bytes.as_slice()),
            "the export carries the exact durable proof bytes"
        );
        assert!(!export.temporary_residual);
        assert_ne!(
            export.proof.as_deref(),
            Some(lock_bytes.as_slice()),
            "the store-owned lock is not exportable as Session evidence"
        );
        assert_eq!(
            std::fs::read(&coordination).expect("lock survives"),
            lock_bytes
        );

        // The temporary is reported as residue and never exported.
        let paths = session_control_plane_paths(&root, SESSION).expect("control paths");
        write_fixture(&paths.temporary, b"staged intent");
        let with_residue = export_session_control_plane(&store).expect("guarded export");
        assert!(with_residue.temporary_residual);
        assert_eq!(with_residue.proof.as_deref(), Some(proof_bytes.as_slice()));
    }

    #[cfg(unix)]
    #[test]
    fn control_plane_export_refuses_a_v2_manifest_whose_proof_is_missing() {
        let (root, store, _proof) = advanced_v2_session("export-missing-proof");
        let paths = session_control_plane_paths(&root, SESSION).expect("control paths");
        std::fs::remove_file(&paths.provenance).expect("remove the durable proof");

        assert_eq!(
            export_session_control_plane(&store),
            Err(SessionControlPlaneExportError::V2ProofMissing),
            "a v2 export without an available proof is a refusal, never a partial success"
        );
    }

    // -- acceptance (5): Windows -------------------------------------------

    #[cfg(unix)]
    #[test]
    fn windows_platform_reads_compatible_state_through_the_guarded_open() {
        let root = root("windows-compatible-read");
        let store = SessionArtifactManifestStore::for_test_session_platform(
            &root,
            SESSION,
            CanonicalPlatform::Windows,
        )
        .expect("windows-platform store");
        persist_manifest(&root, SESSION, v1_candidate("historical-1"));
        write_fixture(
            &store_coordination_path(&root, SESSION).expect("coordination path"),
            b"",
        );

        let admitted = guarded_session_open(&store, SessionSemanticsVersion::V1, |floor| {
            Ok::<_, ()>(floor)
        })
        .expect("windows reads compatible persisted v1 state");
        assert_eq!(
            admitted,
            AdmittedSessionFloor {
                floor: SessionSemanticsVersion::V1,
                evidence: SessionFloorEvidence::GuardedManifest,
            }
        );

        persist_manifest(&root, SESSION, v2_candidate("advance-1"));
        let invoked = Cell::new(false);
        let refusal = guarded_session_open(&store, SessionSemanticsVersion::V1, |_floor| {
            invoked.set(true);
            Ok::<&'static str, ()>("content")
        });
        assert_eq!(
            refusal,
            Err(GuardedSessionOpenError::UnsupportedSessionFloor {
                required: SessionSemanticsVersion::V2,
                maximum_supported: SessionSemanticsVersion::V1,
            })
        );
        assert!(!invoked.get());
    }

    #[cfg(unix)]
    #[test]
    fn windows_platform_refuses_v2_mutation_before_any_side_effect() {
        let root = root("windows-refuses-mutation");
        let store = SessionArtifactManifestStore::for_test_session_platform(
            &root,
            SESSION,
            CanonicalPlatform::Windows,
        )
        .expect("windows-platform store");
        assert!(matches!(
            store.begin_write(),
            Err(ManifestStoreError::NamespaceQualificationRequired)
        ));
        assert_eq!(
            std::fs::read_dir(&root).expect("root readable").count(),
            0,
            "the mutation refusal preceded every side effect"
        );

        let admitted = guarded_session_open(&store, SessionSemanticsVersion::V1, |floor| {
            Ok::<_, ()>(floor)
        })
        .expect("an entirely absent control plane admits historical v1");
        assert_eq!(
            admitted,
            AdmittedSessionFloor {
                floor: SessionSemanticsVersion::V1,
                evidence: SessionFloorEvidence::UnguardedObservedAbsence,
            }
        );
        assert_eq!(
            std::fs::read_dir(&root).expect("root readable").count(),
            0,
            "the read created nothing on a platform that cannot mutate the namespace"
        );
    }
}
