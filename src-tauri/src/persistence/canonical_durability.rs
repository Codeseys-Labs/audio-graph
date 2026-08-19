//! Canonical file and namespace durability barriers.
//!
//! This dormant module owns the filesystem sequencing behind a small,
//! provider-neutral interface. A mutation guard is bound to one exact target
//! parent, safe platform identity where available, and one deterministic
//! coordination file inside that directory. Append accepts only immediate
//! children and rename is same-directory only, so one target can never select
//! overlapping coordination roots. Locks remain cooperative: an uncooperative
//! process can still replace roots or race pathname operations outside this
//! contract.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{File, Metadata, OpenOptions, TryLockError};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(test)]
use std::sync::atomic::AtomicBool;
#[cfg(test)]
use std::sync::{Arc, Barrier};

const COORDINATION_FILE_NAME: &str = ".audio-graph-canonical.lock";
#[cfg(test)]
const ALGORITHM_TEST_ROOT_ANCHOR_PREFIX: &str = ".audio-graph-algorithm-test-root";

/// Conservative equivalence floor for reserved internal basenames.
///
/// Filesystem case behavior varies by volume, including within one platform.
/// The durability module therefore reserves every ASCII-case spelling on all
/// platforms. This intentionally makes no claim about arbitrary Unicode
/// filesystem equivalence.
#[derive(Clone, Copy)]
enum ReservedInternalNameEquivalence {
    AsciiCaseInsensitive,
}

impl ReservedInternalNameEquivalence {
    fn is_reserved_coordination_entry(self, path: &Path) -> bool {
        match self {
            Self::AsciiCaseInsensitive => path.file_name().is_some_and(|name| {
                name.eq_ignore_ascii_case(std::ffi::OsStr::new(COORDINATION_FILE_NAME))
            }),
        }
    }
}

/// Opaque identity used to reconcile an operation whose durability is unknown.
///
/// The caller must supply the same value when reconciling the same logical
/// mutation. Its bytes are deliberately absent from `Debug` and there is no
/// diagnostic accessor.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct CanonicalRecoveryKey([u8; 16]);

impl CanonicalRecoveryKey {
    pub const fn from_opaque_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for CanonicalRecoveryKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CanonicalRecoveryKey([REDACTED])")
    }
}

/// Opaque, non-forgeable namespace qualification evidence.
///
/// A later platform-probe workstream owns production construction. Evidence is
/// bound to the canonical root path plus safe filesystem volume and directory
/// object identity; it cannot be reused for a different root or replacement.
pub struct CanonicalFilesystemQualification {
    namespace: ManagedNamespace,
    token: Option<CanonicalQualificationToken>,
}

impl fmt::Debug for CanonicalFilesystemQualification {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CanonicalFilesystemQualification([BOUND])")
    }
}

impl CanonicalFilesystemQualification {
    /// Qualify one existing managed root from a fresh live filesystem
    /// inventory. The returned durability factory is the only production peer
    /// that can consume this opaque qualification token.
    pub fn for_existing_managed_root(
        root: &Path,
    ) -> Result<(Self, CanonicalDurability), CanonicalFilesystemQualificationError> {
        let platform = current_platform();
        if !namespace_supported_for(platform) {
            return Err(
                CanonicalFilesystemQualificationError::NamespaceDurabilityUnsupported { platform },
            );
        }
        let namespace =
            ManagedNamespace::load(root, CanonicalCoordinationError::ParentProvisioningRequired)
                .map_err(qualification_coordination_error)?;
        if !identity_is_available(&namespace.identity) {
            return Err(CanonicalFilesystemQualificationError::IdentityUnavailable);
        }
        let filesystem = resolve_live_filesystem(&namespace, platform)?;
        let token = next_qualification_token()
            .ok_or(CanonicalFilesystemQualificationError::IdentityUnavailable)?;
        let qualification = Self {
            namespace: namespace.clone(),
            token: Some(token),
        };
        let durability =
            CanonicalDurability::for_production_qualification(token, namespace, filesystem);
        Ok((qualification, durability))
    }

    #[cfg(test)]
    pub(crate) fn for_test_root(root: &Path) -> Result<Self, CanonicalCoordinationError> {
        let namespace =
            ManagedNamespace::load(root, CanonicalCoordinationError::ParentProvisioningRequired)?;
        if namespace.identity.volume.is_none() || namespace.identity.object.is_none() {
            return Err(CanonicalCoordinationError::IdentityUnavailable);
        }
        Ok(Self {
            namespace,
            token: None,
        })
    }
}

/// Opaque cfg(test)-only pairing for platform-independent algorithm fixtures.
///
/// One environment owns the exact root anchor, synthetic namespace token,
/// qualification, platform-policy override, and parent-barrier model. Tests
/// cannot construct a qualification independently from its durability peer.
#[cfg(test)]
pub(crate) struct AlgorithmTestEnvironment {
    qualification: CanonicalFilesystemQualification,
    durability: CanonicalDurability,
}

#[cfg(test)]
impl AlgorithmTestEnvironment {
    pub(crate) fn bind(root: &Path) -> Result<Self, CanonicalCoordinationError> {
        Self::bind_for_platform(root, current_platform())
    }

    pub(crate) fn bind_for_platform(
        root: &Path,
        platform: CanonicalPlatform,
    ) -> Result<Self, CanonicalCoordinationError> {
        static TOKEN: AtomicU64 = AtomicU64::new(1);

        let namespace =
            ManagedNamespace::load(root, CanonicalCoordinationError::ParentProvisioningRequired)?;
        let token = TOKEN.fetch_add(1, Ordering::Relaxed);
        let anchor_path = namespace
            .canonical_root
            .join(format!("{ALGORITHM_TEST_ROOT_ANCHOR_PREFIX}-{token}"));
        let mut anchor_file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&anchor_path)
            .map_err(|error| coordination_io(CanonicalCoordinationStage::Open, &error))?;
        anchor_file
            .write_all(&token.to_be_bytes())
            .and_then(|_| anchor_file.sync_all())
            .map_err(|error| coordination_io(CanonicalCoordinationStage::Open, &error))?;
        drop(anchor_file);
        let binding = AlgorithmTestRootBinding {
            token,
            canonical_root: namespace.canonical_root.clone(),
            anchor_path,
        };
        let namespace = ManagedNamespace {
            identity: binding.filesystem_identity(),
            ..namespace
        };
        Ok(Self {
            qualification: CanonicalFilesystemQualification {
                namespace,
                token: None,
            },
            durability: CanonicalDurability::for_algorithm_environment(platform, binding),
        })
    }

    pub(crate) fn into_parts(self) -> (CanonicalFilesystemQualification, CanonicalDurability) {
        (self.qualification, self.durability)
    }
}

#[cfg(test)]
#[derive(Clone)]
struct AlgorithmTestRootBinding {
    token: u64,
    canonical_root: PathBuf,
    anchor_path: PathBuf,
}

#[cfg(test)]
impl AlgorithmTestRootBinding {
    fn filesystem_identity(&self) -> FilesystemIdentity {
        synthetic_algorithm_filesystem_identity(self.token)
    }

    fn bind_namespace(
        &self,
        namespace: ManagedNamespace,
    ) -> Result<ManagedNamespace, CanonicalCoordinationError> {
        if namespace.canonical_root != self.canonical_root || !self.is_current() {
            return Err(CanonicalCoordinationError::IdentityUnavailable);
        }
        Ok(ManagedNamespace {
            identity: self.filesystem_identity(),
            ..namespace
        })
    }

    fn is_current(&self) -> bool {
        let expected = self.token.to_be_bytes();
        matches!(std::fs::read(&self.anchor_path), Ok(bytes) if bytes == expected)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalPlatform {
    Linux,
    MacOs,
    Windows,
    Other,
}

/// Stable, content-free classification of a probed filesystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalFilesystemClass {
    Ext4,
    Apfs,
    Remote,
    Fuse,
    Temporary,
    Other,
}

/// Refusal to create production filesystem qualification authority.
///
/// Paths, mount sources, filesystem strings, object identifiers, and user
/// bytes are deliberately absent from every variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalFilesystemQualificationError {
    ParentProvisioningRequired,
    ManagedRootNotDirectory,
    IdentityUnavailable,
    NamespaceDurabilityUnsupported {
        platform: CanonicalPlatform,
    },
    NoMatchingMount,
    FilesystemUnsupported {
        platform: CanonicalPlatform,
        class: CanonicalFilesystemClass,
    },
    ReadOnlyFilesystem {
        class: CanonicalFilesystemClass,
    },
    RemovableFilesystem {
        class: CanonicalFilesystemClass,
    },
    Io {
        stage: CanonicalCoordinationStage,
        kind: io::ErrorKind,
        raw_os_error: Option<i32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CanonicalQualificationToken(u64);

#[derive(Clone)]
struct ProductionQualificationBinding {
    token: CanonicalQualificationToken,
    namespace: ManagedNamespace,
    filesystem: QualifiedFilesystemMount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QualifiedFilesystemMount {
    mount_point: PathBuf,
    class: CanonicalFilesystemClass,
    #[cfg(any(test, target_os = "macos"))]
    live_mount: Option<LiveMountIdentity>,
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum LiveMountIdentity {
    #[cfg(target_os = "macos")]
    MacOs(nix::sys::statfs::fsid_t),
    #[cfg(test)]
    Synthetic(u64),
}

#[derive(Clone)]
struct FilesystemObservation {
    mount_point: PathBuf,
    file_system: OsString,
    #[cfg(test)]
    volume: Option<VolumeIdentity>,
    #[cfg(any(test, target_os = "macos"))]
    live_mount: Option<LiveMountIdentity>,
    removable: bool,
    read_only: bool,
}

impl FilesystemObservation {
    #[cfg(test)]
    fn for_test(
        mount_point: impl Into<PathBuf>,
        file_system: impl Into<OsString>,
        removable: bool,
        read_only: bool,
    ) -> Self {
        Self {
            mount_point: mount_point.into(),
            file_system: file_system.into(),
            #[cfg(test)]
            volume: None,
            #[cfg(any(test, target_os = "macos"))]
            live_mount: None,
            removable,
            read_only,
        }
    }

    #[cfg(test)]
    fn for_test_exact_mount(
        mount_point: impl Into<PathBuf>,
        file_system: impl Into<OsString>,
        volume: Option<u64>,
        live_mount: Option<u64>,
        removable: bool,
        read_only: bool,
    ) -> Self {
        Self {
            mount_point: mount_point.into(),
            file_system: file_system.into(),
            #[cfg(test)]
            volume: volume.map(VolumeIdentity::SyntheticAlgorithmNamespace),
            live_mount: live_mount.map(LiveMountIdentity::Synthetic),
            removable,
            read_only,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalNamespaceOperation {
    FirstCreate,
    Rename,
    AtomicSnapshotInstall,
    Unlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalMutation {
    ExistingAppend,
    FirstCreate,
    ImmutableExactCreate,
    ImmutableExactReconcile,
    SnapshotIntentStage,
    Rename,
    InitialSnapshotInstall,
    SnapshotReplacement,
    Unlink,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalDurabilityBarrier {
    FileDataAndMetadata,
    FileAndParentNamespace,
    /// Only the qualified parent-directory namespace barrier was crossed. A
    /// removal claims no file barrier: the object is gone.
    ParentNamespace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalDurabilityReceipt {
    pub mutation: CanonicalMutation,
    pub barrier: CanonicalDurabilityBarrier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalDurabilityStage {
    ValidateNamespace,
    InspectEntry,
    OpenExisting,
    OpenParent,
    CreateNew,
    PostCreate,
    SeekEnd,
    Write,
    Flush,
    ProtectTemp,
    FileSync,
    Rename,
    /// Removal of one existing namespace entry, named for what it is: a stage
    /// called `Rename` inside a removal would be a doc/code disagreement.
    Unlink,
    ParentSync,
}

/// A refusal proven before the one requested operation mutated canonical bytes
/// or namespace.
///
/// The claim is scoped to that single operation, never to the caller's
/// transaction. A caller that composes several operations and has already
/// accepted durable state of its own — a staged snapshot temporary, say — must
/// classify that surviving state in its own vocabulary; forwarding this value
/// unqualified would tell its own callers that nothing outlives the refusal.
/// `ManifestCasRejection::TransitionProofRefusedAfterIntentStaged` is the
/// worked example.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalDurabilityRejection {
    ParentProvisioningRequired,
    NamespaceDurabilityUnsupported {
        platform: CanonicalPlatform,
        operation: CanonicalNamespaceOperation,
    },
    TargetOutsideManagedNamespace,
    IdentityChanged,
    CrossDeviceRenameRefused {
        raw_os_error: Option<i32>,
    },
    QualificationBindingMismatch,
    QualifiedDescendantVolumeMismatch,
    QualifiedDescendantVolumeUnavailable,
    ReservedCoordinationEntry,
    NonRegularCanonicalEntry,
    DestinationAlreadyExists,
    ImmutableExactConflict,
    SnapshotTempAlreadyExists,
    SnapshotPathOverlap,
    CoordinationPoisoned,
    IoFailedBeforeMutation {
        stage: CanonicalDurabilityStage,
        kind: io::ErrorKind,
        raw_os_error: Option<i32>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalDurabilityIndeterminate {
    pub stage: CanonicalDurabilityStage,
    pub kind: io::ErrorKind,
    pub raw_os_error: Option<i32>,
    pub recovery_key: CanonicalRecoveryKey,
}

#[must_use = "canonical durability outcomes must be reconciled before state advances"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalDurabilityOutcome {
    Accepted(CanonicalDurabilityReceipt),
    Rejected(CanonicalDurabilityRejection),
    DurabilityIndeterminate(CanonicalDurabilityIndeterminate),
}

/// Outcome of one durable removal.
///
/// Deliberately a separate type rather than a fourth `CanonicalDurabilityOutcome`
/// variant: a removal has an absence assessment that no other mutation has, and
/// the existing outcome is matched exhaustively across the crate.
#[must_use = "durable unlink outcomes must be reconciled before state advances"]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalUnlinkOutcome {
    /// One regular entry was unlinked and the qualified parent namespace
    /// barrier was crossed.
    Unlinked(CanonicalDurabilityReceipt),
    /// The pathname had no entry when the guard inspected it under the
    /// operation lock, so this call unlinked nothing. The parent barrier was
    /// still crossed, which is what makes an exact rerun the reconciliation for
    /// an unlink whose own barrier result was lost rather than a claim that an
    /// unpublished removal is durable. It asserts nothing about state the caller
    /// mutated through other operations before this call.
    AlreadyAbsent(CanonicalDurabilityBarrier),
    Rejected(CanonicalDurabilityRejection),
    DurabilityIndeterminate(CanonicalDurabilityIndeterminate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CanonicalImmutableExactState {
    Absent,
    Exact,
    StrictPrefix,
}

/// Expected snapshot-head identity at atomic installation time.
///
/// An existing snapshot is represented by the exact open file from which the
/// caller validated its generation. The guard verifies that handle still
/// names the destination before replacing it. `Debug` never exposes the file
/// descriptor or path.
pub enum CanonicalSnapshotExpectation<'a> {
    Absent,
    Existing(&'a File),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotDisposition {
    StageTemporary,
    Install,
}

impl fmt::Debug for CanonicalSnapshotExpectation<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => formatter.write_str("CanonicalSnapshotExpectation::Absent"),
            Self::Existing(_) => {
                formatter.write_str("CanonicalSnapshotExpectation::Existing([BOUND])")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalCoordinationStage {
    ResolveRoot,
    InspectLock,
    Open,
    Lock,
}

/// Content-free cooperative-lock failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalCoordinationError {
    ParentProvisioningRequired,
    Missing,
    Contended,
    ManagedRootNotDirectory,
    IdentityUnavailable,
    QualificationBindingMismatch,
    QualificationRefused(CanonicalFilesystemQualificationError),
    NonRegularLockFile,
    Io {
        stage: CanonicalCoordinationStage,
        kind: io::ErrorKind,
        raw_os_error: Option<i32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum VolumeIdentity {
    #[cfg(unix)]
    UnixDevice(u64),
    #[cfg(test)]
    SyntheticAlgorithmNamespace(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ObjectIdentity {
    #[cfg(unix)]
    UnixInode(u64),
    #[cfg(test)]
    SyntheticAlgorithmObject(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FilesystemIdentity {
    volume: Option<VolumeIdentity>,
    object: Option<ObjectIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedNamespace {
    canonical_root: PathBuf,
    identity: FilesystemIdentity,
}

impl ManagedNamespace {
    fn load(
        root: &Path,
        missing: CanonicalCoordinationError,
    ) -> Result<Self, CanonicalCoordinationError> {
        let canonical_root = std::fs::canonicalize(root).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                missing
            } else {
                coordination_io(CanonicalCoordinationStage::ResolveRoot, &error)
            }
        })?;
        let metadata = std::fs::metadata(&canonical_root)
            .map_err(|error| coordination_io(CanonicalCoordinationStage::ResolveRoot, &error))?;
        if !metadata.is_dir() {
            return Err(CanonicalCoordinationError::ManagedRootNotDirectory);
        }
        Ok(Self {
            canonical_root,
            identity: filesystem_identity(&metadata),
        })
    }

    fn observed_identity(&self, metadata: &Metadata) -> FilesystemIdentity {
        #[cfg(test)]
        if synthetic_algorithm_identity_token(&self.identity).is_some() {
            return self.identity.clone();
        }
        filesystem_identity(metadata)
    }

    fn validate_current(&self) -> Result<(), CanonicalDurabilityRejection> {
        let current_root = std::fs::canonicalize(&self.canonical_root)
            .map_err(|error| durability_io(CanonicalDurabilityStage::ValidateNamespace, &error))?;
        let metadata = std::fs::metadata(&current_root)
            .map_err(|error| durability_io(CanonicalDurabilityStage::ValidateNamespace, &error))?;
        if current_root != self.canonical_root
            || !metadata.is_dir()
            || self.observed_identity(&metadata) != self.identity
        {
            return Err(CanonicalDurabilityRejection::IdentityChanged);
        }
        Ok(())
    }

    fn bind_parent(
        &self,
        target: &Path,
        platform: CanonicalPlatform,
    ) -> Result<BoundParent, CanonicalDurabilityRejection> {
        self.validate_current()?;
        if target.file_name().is_none() {
            return Err(CanonicalDurabilityRejection::TargetOutsideManagedNamespace);
        }
        let parent = parent_directory(target);
        let canonical_parent = std::fs::canonicalize(&parent).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                CanonicalDurabilityRejection::ParentProvisioningRequired
            } else {
                durability_io(CanonicalDurabilityStage::OpenParent, &error)
            }
        })?;
        if canonical_parent != self.canonical_root {
            return Err(CanonicalDurabilityRejection::TargetOutsideManagedNamespace);
        }
        let metadata = std::fs::metadata(&canonical_parent)
            .map_err(|error| durability_io(CanonicalDurabilityStage::OpenParent, &error))?;
        let identity = self.observed_identity(&metadata);
        if !metadata.is_dir() {
            return Err(CanonicalDurabilityRejection::TargetOutsideManagedNamespace);
        }
        if identity_is_available(&self.identity) && identity != self.identity {
            return Err(CanonicalDurabilityRejection::IdentityChanged);
        }
        let directory = open_parent_directory(platform, &canonical_parent)?;
        if let Some(directory) = &directory {
            let opened_metadata = directory
                .metadata()
                .map_err(|error| durability_io(CanonicalDurabilityStage::OpenParent, &error))?;
            let opened_identity = self.observed_identity(&opened_metadata);
            if identity_is_available(&identity) && opened_identity != identity {
                return Err(CanonicalDurabilityRejection::IdentityChanged);
            }
        }
        Ok(BoundParent {
            canonical_path: canonical_parent,
            identity,
            barrier: ParentDurabilityBarrier::new(directory, &self.identity),
        })
    }

    fn bind_descendant_parent(
        &self,
        target: &Path,
        platform: CanonicalPlatform,
    ) -> Result<BoundParent, CanonicalDurabilityRejection> {
        self.validate_current()?;
        if target.file_name().is_none() {
            return Err(CanonicalDurabilityRejection::TargetOutsideManagedNamespace);
        }
        let parent = parent_directory(target);
        let canonical_parent = std::fs::canonicalize(&parent).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                CanonicalDurabilityRejection::ParentProvisioningRequired
            } else {
                durability_io(CanonicalDurabilityStage::OpenParent, &error)
            }
        })?;
        if !canonical_parent.starts_with(&self.canonical_root) {
            return Err(CanonicalDurabilityRejection::TargetOutsideManagedNamespace);
        }
        let metadata = std::fs::metadata(&canonical_parent)
            .map_err(|error| durability_io(CanonicalDurabilityStage::OpenParent, &error))?;
        if !metadata.is_dir() {
            return Err(CanonicalDurabilityRejection::TargetOutsideManagedNamespace);
        }
        let identity = self.observed_identity(&metadata);
        let directory = open_parent_directory(platform, &canonical_parent)?;
        if let Some(directory) = &directory {
            let opened_metadata = directory
                .metadata()
                .map_err(|error| durability_io(CanonicalDurabilityStage::OpenParent, &error))?;
            if identity_is_available(&identity)
                && self.observed_identity(&opened_metadata) != identity
            {
                return Err(CanonicalDurabilityRejection::IdentityChanged);
            }
        }
        Ok(BoundParent {
            canonical_path: canonical_parent,
            identity,
            barrier: ParentDurabilityBarrier::new(directory, &self.identity),
        })
    }
}

struct BoundParent {
    canonical_path: PathBuf,
    identity: FilesystemIdentity,
    barrier: ParentDurabilityBarrier,
}

struct ParentDurabilityBarrier {
    native: Option<File>,
    #[cfg(test)]
    algorithm_test: bool,
}

impl ParentDurabilityBarrier {
    fn new(native: Option<File>, _namespace_identity: &FilesystemIdentity) -> Self {
        Self {
            native,
            #[cfg(test)]
            algorithm_test: synthetic_algorithm_identity_token(_namespace_identity).is_some(),
        }
    }

    fn is_available(&self) -> bool {
        #[cfg(test)]
        if self.algorithm_test {
            return true;
        }
        self.native.is_some()
    }

    fn sync_all(&self) -> io::Result<()> {
        #[cfg(test)]
        if self.algorithm_test {
            return Ok(());
        }
        self.native
            .as_ref()
            .ok_or_else(|| io::Error::other("parent durability barrier unavailable"))?
            .sync_all()
    }
}

#[cfg(target_os = "windows")]
fn open_parent_directory(
    _platform: CanonicalPlatform,
    _path: &Path,
) -> Result<Option<File>, CanonicalDurabilityRejection> {
    // Rust's ordinary `File::open` does not request the directory-handle flags
    // required by Windows. Existing-file append needs no directory handle, and
    // every Windows namespace mutation is refused before a parent barrier.
    Ok(None)
}

#[cfg(not(target_os = "windows"))]
fn open_parent_directory(
    platform: CanonicalPlatform,
    path: &Path,
) -> Result<Option<File>, CanonicalDurabilityRejection> {
    // The test policy can exercise Windows refusal on a Unix host without
    // accidentally taking a Unix directory handle first.
    if platform == CanonicalPlatform::Windows {
        return Ok(None);
    }
    File::open(path)
        .map(Some)
        .map_err(|error| durability_io(CanonicalDurabilityStage::OpenParent, &error))
}

/// Exclusive mutation transaction bound to one managed namespace.
///
/// The deterministic coordination file and namespace identity are private, so
/// a caller cannot pair an arbitrary lock with an unrelated mutation target.
pub struct CanonicalExclusiveGuard {
    namespace: ManagedNamespace,
    qualification_token: Option<CanonicalQualificationToken>,
    platform: CanonicalPlatform,
    reserved_internal_names: ReservedInternalNameEquivalence,
    _lock_file: File,
    operation_lock: Mutex<()>,
    #[cfg(test)]
    injected_failure: Option<InjectedFailure>,
    #[cfg(test)]
    before_atomic_create: Option<Arc<Barrier>>,
    #[cfg(test)]
    before_existing_open: Option<Arc<Barrier>>,
    #[cfg(test)]
    before_snapshot_revalidation: Option<Arc<Barrier>>,
    #[cfg(test)]
    injected_rename_fault: Option<InjectedRenameFaultState>,
    #[cfg(test)]
    algorithm_test: Option<AlgorithmTestRootBinding>,
}

/// Shared strict-reader guard bound to the same deterministic coordination
/// file. Acquisition never creates the managed root or lock file.
pub struct CanonicalSharedGuard {
    _namespace: ManagedNamespace,
    _lock_file: File,
}

#[cfg(test)]
#[derive(Clone, Copy)]
struct InjectedFailure {
    stage: CanonicalDurabilityStage,
    raw_os_error: Option<i32>,
}

#[cfg(test)]
#[derive(Clone, Copy)]
enum InjectedRenameFault {
    PreflightDeviceMismatch,
    DescendantQualificationMismatch,
    DescendantQualificationUnavailable,
    RuntimeExdev { raw_os_error: i32 },
}

#[cfg(test)]
#[derive(Clone)]
struct InjectedRenameFaultState {
    fault: InjectedRenameFault,
    rename_invoked: Arc<AtomicBool>,
}

/// Factory for namespace-bound cooperative guards.
pub struct CanonicalDurability {
    platform: CanonicalPlatform,
    production_binding: Option<ProductionQualificationBinding>,
    reserved_internal_names: ReservedInternalNameEquivalence,
    #[cfg(test)]
    injected_failure: Option<InjectedFailure>,
    #[cfg(test)]
    before_atomic_create: Option<Arc<Barrier>>,
    #[cfg(test)]
    before_existing_open: Option<Arc<Barrier>>,
    #[cfg(test)]
    before_snapshot_revalidation: Option<Arc<Barrier>>,
    #[cfg(test)]
    injected_rename_fault: Option<InjectedRenameFaultState>,
    #[cfg(test)]
    algorithm_test: Option<AlgorithmTestRootBinding>,
}

impl Default for CanonicalDurability {
    fn default() -> Self {
        Self {
            platform: current_platform(),
            production_binding: None,
            reserved_internal_names: ReservedInternalNameEquivalence::AsciiCaseInsensitive,
            #[cfg(test)]
            injected_failure: None,
            #[cfg(test)]
            before_atomic_create: None,
            #[cfg(test)]
            before_existing_open: None,
            #[cfg(test)]
            before_snapshot_revalidation: None,
            #[cfg(test)]
            injected_rename_fault: None,
            #[cfg(test)]
            algorithm_test: None,
        }
    }
}

impl CanonicalDurability {
    pub fn new() -> Self {
        Self::default()
    }

    pub(crate) fn namespace_mutation_supported(&self) -> bool {
        namespace_supported_for(self.platform)
    }

    fn for_production_qualification(
        token: CanonicalQualificationToken,
        namespace: ManagedNamespace,
        filesystem: QualifiedFilesystemMount,
    ) -> Self {
        Self {
            production_binding: Some(ProductionQualificationBinding {
                token,
                namespace,
                filesystem,
            }),
            ..Self::default()
        }
    }

    /// Acquire the deterministic writer coordination file inside an existing
    /// exact target parent. The directory itself is never provisioned.
    pub fn try_lock_exclusive(
        &self,
        managed_root: &Path,
    ) -> Result<CanonicalExclusiveGuard, CanonicalCoordinationError> {
        self.try_lock_exclusive_inner(managed_root, None)
    }

    /// Acquire the writer coordination entry only after the production pair's
    /// opaque token, exact root identity, and live filesystem binding match.
    pub fn try_lock_exclusive_qualified(
        &self,
        managed_root: &Path,
        qualification: &CanonicalFilesystemQualification,
    ) -> Result<CanonicalExclusiveGuard, CanonicalCoordinationError> {
        self.try_lock_exclusive_inner(managed_root, Some(qualification))
    }

    fn try_lock_exclusive_inner(
        &self,
        managed_root: &Path,
        qualification: Option<&CanonicalFilesystemQualification>,
    ) -> Result<CanonicalExclusiveGuard, CanonicalCoordinationError> {
        let namespace = ManagedNamespace::load(
            managed_root,
            CanonicalCoordinationError::ParentProvisioningRequired,
        )?;
        #[cfg(test)]
        let namespace = match &self.algorithm_test {
            Some(binding) => binding.bind_namespace(namespace)?,
            None => namespace,
        };
        self.validate_guard_binding(&namespace, qualification)?;
        let lock_path = namespace.canonical_root.join(COORDINATION_FILE_NAME);
        let file = open_writer_coordination(&lock_path)?;
        let file = try_lock_exclusive(file)?;
        Ok(CanonicalExclusiveGuard {
            namespace,
            qualification_token: self
                .production_binding
                .as_ref()
                .map(|binding| binding.token),
            platform: self.platform,
            reserved_internal_names: self.reserved_internal_names,
            _lock_file: file,
            operation_lock: Mutex::new(()),
            #[cfg(test)]
            injected_failure: self.injected_failure,
            #[cfg(test)]
            before_atomic_create: self.before_atomic_create.clone(),
            #[cfg(test)]
            before_existing_open: self.before_existing_open.clone(),
            #[cfg(test)]
            before_snapshot_revalidation: self.before_snapshot_revalidation.clone(),
            #[cfg(test)]
            injected_rename_fault: self.injected_rename_fault.clone(),
            #[cfg(test)]
            algorithm_test: self.algorithm_test.clone(),
        })
    }

    fn validate_guard_binding(
        &self,
        namespace: &ManagedNamespace,
        qualification: Option<&CanonicalFilesystemQualification>,
    ) -> Result<(), CanonicalCoordinationError> {
        let Some(binding) = &self.production_binding else {
            if qualification.is_some_and(|qualification| qualification.token.is_some()) {
                return Err(CanonicalCoordinationError::QualificationBindingMismatch);
            }
            return Ok(());
        };
        let Some(qualification) = qualification else {
            return Err(CanonicalCoordinationError::QualificationBindingMismatch);
        };
        if qualification.token != Some(binding.token)
            || qualification.namespace != binding.namespace
            || *namespace != binding.namespace
        {
            return Err(CanonicalCoordinationError::QualificationBindingMismatch);
        }
        let filesystem = resolve_live_filesystem(namespace, self.platform)
            .map_err(CanonicalCoordinationError::QualificationRefused)?;
        if filesystem != binding.filesystem {
            return Err(CanonicalCoordinationError::QualificationBindingMismatch);
        }
        Ok(())
    }

    /// Acquire a shared strict-reader lock without creating missing state.
    pub fn try_lock_shared(
        &self,
        managed_root: &Path,
    ) -> Result<CanonicalSharedGuard, CanonicalCoordinationError> {
        self.try_lock_shared_inner(managed_root, None)
    }

    /// Acquire the reader coordination entry only after revalidating the
    /// production qualification pair. Missing state is never created.
    pub fn try_lock_shared_qualified(
        &self,
        managed_root: &Path,
        qualification: &CanonicalFilesystemQualification,
    ) -> Result<CanonicalSharedGuard, CanonicalCoordinationError> {
        self.try_lock_shared_inner(managed_root, Some(qualification))
    }

    fn try_lock_shared_inner(
        &self,
        managed_root: &Path,
        qualification: Option<&CanonicalFilesystemQualification>,
    ) -> Result<CanonicalSharedGuard, CanonicalCoordinationError> {
        let namespace = ManagedNamespace::load(managed_root, CanonicalCoordinationError::Missing)?;
        #[cfg(test)]
        let namespace = match &self.algorithm_test {
            Some(binding) => binding.bind_namespace(namespace)?,
            None => namespace,
        };
        self.validate_guard_binding(&namespace, qualification)?;
        let lock_path = namespace.canonical_root.join(COORDINATION_FILE_NAME);
        ensure_regular_lock_entry(&lock_path)?;
        let file = OpenOptions::new()
            .read(true)
            .open(&lock_path)
            .map_err(|error| coordination_io(CanonicalCoordinationStage::Open, &error))?;
        let file = try_lock_shared(file)?;
        Ok(CanonicalSharedGuard {
            _namespace: namespace,
            _lock_file: file,
        })
    }

    #[cfg(test)]
    fn failing_at(stage: CanonicalDurabilityStage) -> Self {
        Self {
            platform: current_platform(),
            production_binding: None,
            reserved_internal_names: ReservedInternalNameEquivalence::AsciiCaseInsensitive,
            injected_failure: Some(InjectedFailure {
                stage,
                raw_os_error: None,
            }),
            before_atomic_create: None,
            before_existing_open: None,
            before_snapshot_revalidation: None,
            injected_rename_fault: None,
            algorithm_test: None,
        }
    }

    #[cfg(test)]
    fn failing_at_with_raw_os_error(stage: CanonicalDurabilityStage, raw_os_error: i32) -> Self {
        Self {
            platform: current_platform(),
            production_binding: None,
            reserved_internal_names: ReservedInternalNameEquivalence::AsciiCaseInsensitive,
            injected_failure: Some(InjectedFailure {
                stage,
                raw_os_error: Some(raw_os_error),
            }),
            before_atomic_create: None,
            before_existing_open: None,
            before_snapshot_revalidation: None,
            injected_rename_fault: None,
            algorithm_test: None,
        }
    }

    #[cfg(test)]
    fn with_before_atomic_create(barrier: Arc<Barrier>) -> Self {
        Self {
            platform: current_platform(),
            production_binding: None,
            reserved_internal_names: ReservedInternalNameEquivalence::AsciiCaseInsensitive,
            injected_failure: None,
            before_atomic_create: Some(barrier),
            before_existing_open: None,
            before_snapshot_revalidation: None,
            injected_rename_fault: None,
            algorithm_test: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn for_test_platform(platform: CanonicalPlatform) -> Self {
        Self {
            platform,
            production_binding: None,
            reserved_internal_names: ReservedInternalNameEquivalence::AsciiCaseInsensitive,
            injected_failure: None,
            before_atomic_create: None,
            before_existing_open: None,
            before_snapshot_revalidation: None,
            injected_rename_fault: None,
            algorithm_test: None,
        }
    }

    #[cfg(test)]
    fn for_algorithm_environment(
        platform: CanonicalPlatform,
        binding: AlgorithmTestRootBinding,
    ) -> Self {
        Self {
            platform,
            production_binding: None,
            reserved_internal_names: ReservedInternalNameEquivalence::AsciiCaseInsensitive,
            injected_failure: None,
            before_atomic_create: None,
            before_existing_open: None,
            before_snapshot_revalidation: None,
            injected_rename_fault: None,
            algorithm_test: Some(binding),
        }
    }

    #[cfg(test)]
    fn with_before_existing_open(barrier: Arc<Barrier>) -> Self {
        Self {
            platform: current_platform(),
            production_binding: None,
            reserved_internal_names: ReservedInternalNameEquivalence::AsciiCaseInsensitive,
            injected_failure: None,
            before_atomic_create: None,
            before_existing_open: Some(barrier),
            before_snapshot_revalidation: None,
            injected_rename_fault: None,
            algorithm_test: None,
        }
    }

    #[cfg(test)]
    fn with_before_snapshot_revalidation(barrier: Arc<Barrier>) -> Self {
        Self {
            platform: current_platform(),
            production_binding: None,
            reserved_internal_names: ReservedInternalNameEquivalence::AsciiCaseInsensitive,
            injected_failure: None,
            before_atomic_create: None,
            before_existing_open: None,
            before_snapshot_revalidation: Some(barrier),
            injected_rename_fault: None,
            algorithm_test: None,
        }
    }

    #[cfg(test)]
    fn for_test_name_equivalence(reserved_internal_names: ReservedInternalNameEquivalence) -> Self {
        Self {
            platform: current_platform(),
            production_binding: None,
            reserved_internal_names,
            injected_failure: None,
            before_atomic_create: None,
            before_existing_open: None,
            before_snapshot_revalidation: None,
            injected_rename_fault: None,
            algorithm_test: None,
        }
    }

    #[cfg(test)]
    fn with_rename_fault(fault: InjectedRenameFault, rename_invoked: Arc<AtomicBool>) -> Self {
        Self {
            platform: current_platform(),
            production_binding: None,
            reserved_internal_names: ReservedInternalNameEquivalence::AsciiCaseInsensitive,
            injected_failure: None,
            before_atomic_create: None,
            before_existing_open: None,
            before_snapshot_revalidation: None,
            injected_rename_fault: Some(InjectedRenameFaultState {
                fault,
                rename_invoked,
            }),
            algorithm_test: None,
        }
    }
}

impl CanonicalExclusiveGuard {
    /// Prove that all recovery namespace mutations can use the qualified
    /// parent barrier before any recovery temp is created.
    pub(crate) fn preflight_recovery_namespace(
        &self,
        source: &Path,
        temporary: &Path,
        destination: &Path,
        qualification: Option<&CanonicalFilesystemQualification>,
    ) -> Result<(), CanonicalDurabilityRejection> {
        self.preflight_mutation_targets([source, temporary, destination])?;
        if !self.namespace_is_supported() {
            return Err(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: self.platform,
                    operation: CanonicalNamespaceOperation::Rename,
                },
            );
        }
        let Some(qualification) = qualification else {
            return Err(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: self.platform,
                    operation: CanonicalNamespaceOperation::Rename,
                },
            );
        };
        if !self.qualification_status(Some(qualification))? {
            return Err(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: self.platform,
                    operation: CanonicalNamespaceOperation::Rename,
                },
            );
        }
        self.namespace
            .bind_descendant_parent(source, self.platform)?;
        let temporary_parent = self
            .namespace
            .bind_descendant_parent(temporary, self.platform)?;
        let destination_parent = self
            .namespace
            .bind_descendant_parent(destination, self.platform)?;
        if !temporary_parent.barrier.is_available()
            || temporary_parent.canonical_path != destination_parent.canonical_path
            || temporary_parent.identity != destination_parent.identity
        {
            return Err(CanonicalDurabilityRejection::TargetOutsideManagedNamespace);
        }
        self.validate_qualified_descendant_volume(&temporary_parent, qualification)?;
        self.validate_qualified_descendant_volume(&destination_parent, qualification)?;
        Ok(())
    }

    /// Open one regular canonical source for recovery. The returned handle is
    /// the handle the recovery transaction must retain through truncation.
    pub(crate) fn open_recovery_source(
        &self,
        path: &Path,
    ) -> Result<File, CanonicalDurabilityRejection> {
        self.preflight_mutation_targets([path])?;
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|_| CanonicalDurabilityRejection::CoordinationPoisoned)?;
        self.namespace.bind_descendant_parent(path, self.platform)?;
        self.open_existing_regular(path)
    }

    /// Revalidate that the canonical pathname still names the exact retained
    /// source handle. This is the cooperative-process substitution fence used
    /// immediately before every destructive source mutation.
    pub(crate) fn revalidate_recovery_source(
        &self,
        path: &Path,
        source: &File,
    ) -> Result<(), CanonicalDurabilityRejection> {
        self.preflight_mutation_targets([path])?;
        self.namespace.bind_descendant_parent(path, self.platform)?;
        #[cfg(test)]
        if uses_synthetic_algorithm_identity(&self.namespace.identity) {
            return match algorithm_test_path_names_open_file(path, source) {
                Ok(true) => Ok(()),
                Ok(false) => Err(CanonicalDurabilityRejection::IdentityChanged),
                Err(error) => Err(durability_io(
                    CanonicalDurabilityStage::InspectEntry,
                    &error,
                )),
            };
        }
        validate_snapshot_destination(path, CanonicalSnapshotExpectation::Existing(source))?;
        Ok(())
    }

    /// Re-establish the qualified parent barrier for an already-published
    /// recovery artifact after a prior rename result was lost.
    pub(crate) fn sync_recovery_namespace_entry(
        &self,
        path: &Path,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        if let Err(rejection) = self.preflight_mutation_targets([path]) {
            return CanonicalDurabilityOutcome::Rejected(rejection);
        }
        let _operation = match self.operation_lock.lock() {
            Ok(operation) => operation,
            Err(_) => {
                return CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::CoordinationPoisoned,
                );
            }
        };
        if !self.namespace_is_supported() {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: self.platform,
                    operation: CanonicalNamespaceOperation::Rename,
                },
            );
        }
        let qualified = match self.qualification_status(qualification) {
            Ok(qualified) => qualified,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        if !qualified {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: self.platform,
                    operation: CanonicalNamespaceOperation::Rename,
                },
            );
        }
        let parent = match self.namespace.bind_descendant_parent(path, self.platform) {
            Ok(parent) => parent,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        if !parent.barrier.is_available() {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: self.platform,
                    operation: CanonicalNamespaceOperation::Rename,
                },
            );
        }
        if let Err(rejection) = self.open_existing_regular(path) {
            return CanonicalDurabilityOutcome::Rejected(rejection);
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::ParentSync, || {
            parent.barrier.sync_all()
        }) {
            return indeterminate(CanonicalDurabilityStage::ParentSync, &error, recovery_key);
        }
        CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
            mutation: CanonicalMutation::Rename,
            barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
        })
    }

    /// Durably unlink one regular entry that is an immediate child of this
    /// guard's managed root.
    ///
    /// TRUST BOUNDARY. This substrate refuses only what it can prove wrong
    /// without knowing the caller's intent: the store-owned coordination entry
    /// under every ASCII-case spelling, a non-regular entry, a target whose
    /// canonical parent is not exactly the managed root, and a pathname that no
    /// longer names the object this guard opened. It does NOT know which of the
    /// caller's records are meant to be immutable — keeping a canonical
    /// manifest head or an immutable proof record out of `path` is the caller's
    /// obligation. Its only production caller is
    /// `ManifestWriteTransaction::abandon_staged_transition`, which passes the
    /// Session's own derived manifest temporary.
    ///
    /// `Unlinked` follows the qualified parent-namespace barrier. No file
    /// barrier is claimed: the object is gone. `AlreadyAbsent` crosses the same
    /// barrier, which is what makes an exact rerun the reconciliation for an
    /// unlink whose own barrier result was lost. `recovery_key` names THIS
    /// unlink of THIS pathname, not whatever the removed entry contained; the
    /// caller must supply the same value when it reruns.
    ///
    /// The boundary sits at the removal call. Every refusal proven before
    /// `remove_file` is invoked — reserved name, poisoned lock, unsupported or
    /// unqualified namespace, out-of-root or non-regular target, changed
    /// identity, and each injectable pre-invocation cut — is `Rejected` with the
    /// entry intact. From that invocation onwards every failure is
    /// `DurabilityIndeterminate` carrying the failed stage and the caller's
    /// recovery key: the removal itself as stage `Unlink`, and the
    /// parent-namespace barrier that publishes either a removal or an observed
    /// absence as stage `ParentSync`.
    ///
    /// At stage `Unlink` that classification is deliberately CONSERVATIVE — it
    /// is drawn at the invocation, not at its effect, so it can report a
    /// durability question where nothing was removed. A `remove_file` denied by
    /// the parent directory's permission check (`EACCES` or `EPERM` under a
    /// read-only parent) cannot have changed a directory entry, and is still
    /// reported indeterminate rather than as a refusal; the honest cost is one
    /// exact rerun, where deciding removal-took-effect from an errno would
    /// eventually call an unpublished removal durable. A raced `NotFound` lands
    /// on the same arm for the opposite reason: the entry is gone, but no
    /// barrier has published its absence yet.
    /// `unlink_indeterminate_at_the_removal_call_is_conservative_and_rerunnable`
    /// pins both the classification and the exact rerun that resolves it.
    pub(crate) fn unlink_canonical_entry(
        &self,
        path: &Path,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalUnlinkOutcome {
        self.unlink_canonical_entry_inner(path, qualification, recovery_key, None)
    }

    #[cfg(test)]
    pub(crate) fn unlink_canonical_entry_with_fault(
        &self,
        path: &Path,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
        injected_fault: CanonicalDurabilityStage,
    ) -> CanonicalUnlinkOutcome {
        self.unlink_canonical_entry_inner(path, qualification, recovery_key, Some(injected_fault))
    }

    fn unlink_canonical_entry_inner(
        &self,
        path: &Path,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
        injected_fault: Option<CanonicalDurabilityStage>,
    ) -> CanonicalUnlinkOutcome {
        if let Err(rejection) = self.preflight_mutation_targets([path]) {
            return CanonicalUnlinkOutcome::Rejected(rejection);
        }
        let _operation = match self.operation_lock.lock() {
            Ok(operation) => operation,
            Err(_) => {
                return CanonicalUnlinkOutcome::Rejected(
                    CanonicalDurabilityRejection::CoordinationPoisoned,
                );
            }
        };
        if !self.namespace_is_supported() {
            return unlink_namespace_unsupported(self.platform);
        }
        match self.qualification_status(qualification) {
            Ok(true) => {}
            Ok(false) => return unlink_namespace_unsupported(self.platform),
            Err(rejection) => return CanonicalUnlinkOutcome::Rejected(rejection),
        }
        // `bind_parent`, never `bind_descendant_parent`: this primitive can only
        // reach immediate children of the managed root, so it can never become a
        // subtree purge.
        let parent = match self.namespace.bind_parent(path, self.platform) {
            Ok(parent) => parent,
            Err(rejection) => return CanonicalUnlinkOutcome::Rejected(rejection),
        };
        if !parent.barrier.is_available() {
            return unlink_namespace_unsupported(self.platform);
        }

        if injected_fault == Some(CanonicalDurabilityStage::InspectEntry) {
            return CanonicalUnlinkOutcome::Rejected(durability_io(
                CanonicalDurabilityStage::InspectEntry,
                &io::Error::other("injected unlink inspect cut"),
            ));
        }
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return self.publish_unlink_absence(&parent, recovery_key, injected_fault);
            }
            Err(error) => {
                return CanonicalUnlinkOutcome::Rejected(durability_io(
                    CanonicalDurabilityStage::InspectEntry,
                    &error,
                ));
            }
            Ok(metadata) if !metadata.file_type().is_file() => {
                return CanonicalUnlinkOutcome::Rejected(
                    CanonicalDurabilityRejection::NonRegularCanonicalEntry,
                );
            }
            Ok(_) => {}
        }

        if injected_fault == Some(CanonicalDurabilityStage::OpenExisting) {
            return CanonicalUnlinkOutcome::Rejected(durability_io(
                CanonicalDurabilityStage::OpenExisting,
                &io::Error::other("injected unlink open cut"),
            ));
        }
        let file = match self.open_existing_regular(path) {
            Ok(file) => file,
            Err(rejection) => return CanonicalUnlinkOutcome::Rejected(rejection),
        };
        if let Err(rejection) =
            self.validate_snapshot_destination(path, CanonicalSnapshotExpectation::Existing(&file))
        {
            return CanonicalUnlinkOutcome::Rejected(rejection);
        }

        if injected_fault == Some(CanonicalDurabilityStage::Unlink) {
            return CanonicalUnlinkOutcome::Rejected(durability_io(
                CanonicalDurabilityStage::Unlink,
                &io::Error::other("injected unlink cut"),
            ));
        }
        if let Some(error) = self.injected_error(CanonicalDurabilityStage::Unlink) {
            return CanonicalUnlinkOutcome::Rejected(durability_io(
                CanonicalDurabilityStage::Unlink,
                &error,
            ));
        }
        if let Err(error) = std::fs::remove_file(path) {
            // Includes a raced `NotFound`: the parent barrier has not been
            // crossed, so absence is not durable yet and the exact rerun is what
            // resolves it.
            return unlink_indeterminate(CanonicalDurabilityStage::Unlink, &error, recovery_key);
        }
        if let Err(error) =
            self.checked_unlink(CanonicalDurabilityStage::ParentSync, injected_fault, || {
                parent.barrier.sync_all()
            })
        {
            return unlink_indeterminate(
                CanonicalDurabilityStage::ParentSync,
                &error,
                recovery_key,
            );
        }
        CanonicalUnlinkOutcome::Unlinked(CanonicalDurabilityReceipt {
            mutation: CanonicalMutation::Unlink,
            barrier: CanonicalDurabilityBarrier::ParentNamespace,
        })
    }

    /// The absent arm still crosses the qualified parent barrier, so an exact
    /// rerun reconciles an unlink whose own barrier result was lost instead of
    /// calling an unpublished removal durable.
    fn publish_unlink_absence(
        &self,
        parent: &BoundParent,
        recovery_key: CanonicalRecoveryKey,
        injected_fault: Option<CanonicalDurabilityStage>,
    ) -> CanonicalUnlinkOutcome {
        if let Err(error) =
            self.checked_unlink(CanonicalDurabilityStage::ParentSync, injected_fault, || {
                parent.barrier.sync_all()
            })
        {
            return unlink_indeterminate(
                CanonicalDurabilityStage::ParentSync,
                &error,
                recovery_key,
            );
        }
        CanonicalUnlinkOutcome::AlreadyAbsent(CanonicalDurabilityBarrier::ParentNamespace)
    }

    fn checked_unlink(
        &self,
        stage: CanonicalDurabilityStage,
        injected_fault: Option<CanonicalDurabilityStage>,
        operation: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if injected_fault == Some(stage) {
            return Err(io::Error::other("injected unlink cut"));
        }
        self.checked(stage, operation)
    }

    /// Classify one immutable exact target under this exclusive guard without
    /// mutating it.
    ///
    /// `Absent` is a missing entry, `Exact` is a regular file whose bytes
    /// already equal `expected`, and `StrictPrefix` is a regular file whose
    /// bytes are a shorter prefix of `expected`. A longer, non-prefix,
    /// non-regular, or case-aliased entry is a typed conflict. This performs no
    /// create, write, truncate, or barrier, but it is not a read-only open: the
    /// existing entry is opened read/write for handle identity revalidation, so
    /// a target that cannot be opened for writing is rejected here.
    pub(crate) fn preflight_immutable_exact(
        &self,
        path: &Path,
        expected: &[u8],
        qualification: Option<&CanonicalFilesystemQualification>,
    ) -> Result<CanonicalImmutableExactState, CanonicalDurabilityRejection> {
        self.preflight_mutation_targets([path])?;
        let _operation = self
            .operation_lock
            .lock()
            .map_err(|_| CanonicalDurabilityRejection::CoordinationPoisoned)?;
        if !self.namespace_is_supported() || !self.qualification_status(qualification)? {
            return Err(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: self.platform,
                    operation: CanonicalNamespaceOperation::AtomicSnapshotInstall,
                },
            );
        }
        let parent = self.namespace.bind_parent(path, self.platform)?;
        if !parent.barrier.is_available() {
            return Err(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: self.platform,
                    operation: CanonicalNamespaceOperation::AtomicSnapshotInstall,
                },
            );
        }
        ensure_no_ascii_case_alias(path)?;
        match std::fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(CanonicalImmutableExactState::Absent);
            }
            Err(error) => {
                return Err(durability_io(
                    CanonicalDurabilityStage::InspectEntry,
                    &error,
                ));
            }
            Ok(_) => {}
        }
        let mut file = self.open_existing_regular(path)?;
        let mut observed = Vec::new();
        (&mut file)
            .take(u64::try_from(expected.len()).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut observed)
            .map_err(|error| durability_io(CanonicalDurabilityStage::OpenExisting, &error))?;
        if observed.len() > expected.len() || !expected.starts_with(&observed) {
            return Err(CanonicalDurabilityRejection::ImmutableExactConflict);
        }
        self.validate_snapshot_destination(path, CanonicalSnapshotExpectation::Existing(&file))?;
        if observed.len() == expected.len() {
            Ok(CanonicalImmutableExactState::Exact)
        } else {
            Ok(CanonicalImmutableExactState::StrictPrefix)
        }
    }

    #[cfg(test)]
    pub(crate) fn create_or_reconcile_immutable_exact(
        &self,
        path: &Path,
        expected: &[u8],
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        self.create_or_reconcile_immutable_exact_inner(
            path,
            expected,
            0,
            None,
            qualification,
            recovery_key,
            None,
        )
    }

    /// Require a strict-prefix recovery artifact to contain at least the
    /// caller's complete immutable identity prefix before it can be completed.
    pub(crate) fn create_or_reconcile_immutable_exact_with_identity_prefix(
        &self,
        path: &Path,
        expected: &[u8],
        minimum_recoverable_prefix_len: usize,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        self.create_or_reconcile_immutable_exact_inner(
            path,
            expected,
            minimum_recoverable_prefix_len,
            None,
            qualification,
            recovery_key,
            None,
        )
    }

    /// Complete an immutable exact record from any regular strict prefix only
    /// after validating a separate, already-durable exact authentication
    /// record under this same exclusive guard.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_or_reconcile_immutable_exact_with_authentication(
        &self,
        path: &Path,
        expected: &[u8],
        authentication_path: &Path,
        authentication_bytes: &[u8],
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        self.create_or_reconcile_immutable_exact_inner(
            path,
            expected,
            0,
            Some((authentication_path, authentication_bytes)),
            qualification,
            recovery_key,
            None,
        )
    }

    #[cfg(test)]
    pub(crate) fn create_or_reconcile_immutable_exact_with_fault(
        &self,
        path: &Path,
        expected: &[u8],
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
        injected_fault: CanonicalDurabilityStage,
    ) -> CanonicalDurabilityOutcome {
        self.create_or_reconcile_immutable_exact_inner(
            path,
            expected,
            0,
            None,
            qualification,
            recovery_key,
            Some(injected_fault),
        )
    }

    #[cfg(test)]
    pub(crate) fn create_or_reconcile_immutable_exact_with_identity_prefix_and_fault(
        &self,
        path: &Path,
        expected: &[u8],
        minimum_recoverable_prefix_len: usize,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
        injected_fault: CanonicalDurabilityStage,
    ) -> CanonicalDurabilityOutcome {
        self.create_or_reconcile_immutable_exact_inner(
            path,
            expected,
            minimum_recoverable_prefix_len,
            None,
            qualification,
            recovery_key,
            Some(injected_fault),
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create_or_reconcile_immutable_exact_with_authentication_and_fault(
        &self,
        path: &Path,
        expected: &[u8],
        authentication_path: &Path,
        authentication_bytes: &[u8],
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
        injected_fault: CanonicalDurabilityStage,
    ) -> CanonicalDurabilityOutcome {
        self.create_or_reconcile_immutable_exact_inner(
            path,
            expected,
            0,
            Some((authentication_path, authentication_bytes)),
            qualification,
            recovery_key,
            Some(injected_fault),
        )
    }

    /// Durably establish one immutable exact record at its final identity.
    ///
    /// A missing file is created exclusively. An exact regular file is
    /// reconciled by re-establishing its protection, file, and parent barriers.
    /// A regular strict prefix may be completed through its retained handle
    /// after path/handle identity revalidation so a real partial-write restart
    /// converges without introducing another namespace identity. Strict-prefix
    /// recovery is additionally constrained by the caller: when
    /// `authentication` is supplied, that separate record must already exist as
    /// an exact regular file under the same bound parent, and an observed
    /// prefix shorter than `expected` must still reach
    /// `minimum_recoverable_prefix_len`. Every other existing entry is a typed
    /// conflict.
    #[allow(clippy::too_many_arguments)]
    fn create_or_reconcile_immutable_exact_inner(
        &self,
        path: &Path,
        expected: &[u8],
        minimum_recoverable_prefix_len: usize,
        authentication: Option<(&Path, &[u8])>,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
        injected_fault: Option<CanonicalDurabilityStage>,
    ) -> CanonicalDurabilityOutcome {
        if let Err(rejection) = self.preflight_mutation_targets([path]) {
            return CanonicalDurabilityOutcome::Rejected(rejection);
        }
        if let Some((authentication_path, _)) = authentication
            && let Err(rejection) = self.preflight_mutation_targets([authentication_path])
        {
            return CanonicalDurabilityOutcome::Rejected(rejection);
        }
        if minimum_recoverable_prefix_len > expected.len() {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::ImmutableExactConflict,
            );
        }
        let _operation = match self.operation_lock.lock() {
            Ok(operation) => operation,
            Err(_) => {
                return CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::CoordinationPoisoned,
                );
            }
        };
        if !self.namespace_is_supported() {
            return immutable_namespace_unsupported(self.platform);
        }
        match self.qualification_status(qualification) {
            Ok(true) => {}
            Ok(false) => return immutable_namespace_unsupported(self.platform),
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        }
        let parent = match self.namespace.bind_parent(path, self.platform) {
            Ok(parent) => parent,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        if !parent.barrier.is_available() {
            return immutable_namespace_unsupported(self.platform);
        }
        if let Err(rejection) = ensure_no_ascii_case_alias(path) {
            return CanonicalDurabilityOutcome::Rejected(rejection);
        }
        if let Some((authentication_path, authentication_bytes)) = authentication {
            if snapshot_paths_overlap(path, authentication_path)
                || ensure_no_ascii_case_alias(authentication_path).is_err()
            {
                return CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::ImmutableExactConflict,
                );
            }
            let authentication_parent = match self
                .namespace
                .bind_parent(authentication_path, self.platform)
            {
                Ok(parent) => parent,
                Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
            };
            if authentication_parent.canonical_path != parent.canonical_path
                || authentication_parent.identity != parent.identity
            {
                return CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::TargetOutsideManagedNamespace,
                );
            }
            let mut authentication_file = match self.open_existing_regular(authentication_path) {
                Ok(file) => file,
                Err(_) => {
                    return CanonicalDurabilityOutcome::Rejected(
                        CanonicalDurabilityRejection::ImmutableExactConflict,
                    );
                }
            };
            let mut observed = Vec::new();
            if (&mut authentication_file)
                .take(u64::try_from(authentication_bytes.len()).unwrap_or(u64::MAX) + 1)
                .read_to_end(&mut observed)
                .is_err()
                || observed != authentication_bytes
                || self
                    .validate_snapshot_destination(
                        authentication_path,
                        CanonicalSnapshotExpectation::Existing(&authentication_file),
                    )
                    .is_err()
            {
                return CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::ImmutableExactConflict,
                );
            }
        }

        if injected_fault == Some(CanonicalDurabilityStage::CreateNew) {
            return CanonicalDurabilityOutcome::Rejected(durability_io(
                CanonicalDurabilityStage::CreateNew,
                &io::Error::other("injected immutable create cut"),
            ));
        }
        if let Some(error) = self.injected_error(CanonicalDurabilityStage::CreateNew) {
            return CanonicalDurabilityOutcome::Rejected(durability_io(
                CanonicalDurabilityStage::CreateNew,
                &error,
            ));
        }

        self.wait_before_atomic_create();
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        configure_owner_only_creation(&mut options);
        let (mut file, observed_len, mutation) = match options.open(path) {
            Ok(file) => (file, 0, CanonicalMutation::ImmutableExactCreate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                let mut file = match self.open_existing_regular(path) {
                    Ok(file) => file,
                    Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
                };
                let mut observed = Vec::new();
                if let Err(error) = (&mut file)
                    .take(u64::try_from(expected.len()).unwrap_or(u64::MAX) + 1)
                    .read_to_end(&mut observed)
                {
                    return CanonicalDurabilityOutcome::Rejected(durability_io(
                        CanonicalDurabilityStage::OpenExisting,
                        &error,
                    ));
                }
                if observed.len() > expected.len()
                    || !expected.starts_with(&observed)
                    || (observed.len() < expected.len()
                        && observed.len() < minimum_recoverable_prefix_len)
                {
                    return CanonicalDurabilityOutcome::Rejected(
                        CanonicalDurabilityRejection::ImmutableExactConflict,
                    );
                }
                if let Err(rejection) = self.validate_snapshot_destination(
                    path,
                    CanonicalSnapshotExpectation::Existing(&file),
                ) {
                    return CanonicalDurabilityOutcome::Rejected(rejection);
                }
                if let Err(error) = file.seek(SeekFrom::Start(observed.len() as u64)) {
                    return CanonicalDurabilityOutcome::Rejected(durability_io(
                        CanonicalDurabilityStage::SeekEnd,
                        &error,
                    ));
                }
                (
                    file,
                    observed.len(),
                    CanonicalMutation::ImmutableExactReconcile,
                )
            }
            Err(error) => {
                return CanonicalDurabilityOutcome::Rejected(durability_io(
                    CanonicalDurabilityStage::CreateNew,
                    &error,
                ));
            }
        };

        let remaining = &expected[observed_len..];
        if injected_fault == Some(CanonicalDurabilityStage::PostCreate) && observed_len == 0 {
            return indeterminate(
                CanonicalDurabilityStage::PostCreate,
                &io::Error::other("injected immutable post-create cut"),
                recovery_key,
            );
        }
        if injected_fault == Some(CanonicalDurabilityStage::Write) && !remaining.is_empty() {
            let partial = 1.min(remaining.len());
            if let Err(error) = file.write_all(&remaining[..partial]) {
                return indeterminate(CanonicalDurabilityStage::Write, &error, recovery_key);
            }
            return indeterminate(
                CanonicalDurabilityStage::Write,
                &io::Error::other("injected immutable partial write cut"),
                recovery_key,
            );
        }
        if let Err(error) =
            self.checked_immutable(CanonicalDurabilityStage::Write, injected_fault, || {
                file.write_all(remaining)
            })
        {
            return indeterminate(CanonicalDurabilityStage::Write, &error, recovery_key);
        }
        if let Err(error) =
            self.checked_immutable(CanonicalDurabilityStage::Flush, injected_fault, || {
                file.flush()
            })
        {
            return indeterminate(CanonicalDurabilityStage::Flush, &error, recovery_key);
        }
        if let Err(error) = self.checked_immutable(
            CanonicalDurabilityStage::ProtectTemp,
            injected_fault,
            || apply_owner_only_protection(&file),
        ) {
            return indeterminate(CanonicalDurabilityStage::ProtectTemp, &error, recovery_key);
        }
        if self
            .validate_snapshot_destination(path, CanonicalSnapshotExpectation::Existing(&file))
            .is_err()
        {
            return indeterminate(
                CanonicalDurabilityStage::InspectEntry,
                &io::Error::other("immutable exact identity changed"),
                recovery_key,
            );
        }
        if let Err(error) =
            self.checked_immutable(CanonicalDurabilityStage::FileSync, injected_fault, || {
                file.sync_all()
            })
        {
            return indeterminate(CanonicalDurabilityStage::FileSync, &error, recovery_key);
        }
        if let Err(error) =
            self.checked_immutable(CanonicalDurabilityStage::ParentSync, injected_fault, || {
                parent.barrier.sync_all()
            })
        {
            return indeterminate(CanonicalDurabilityStage::ParentSync, &error, recovery_key);
        }

        CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
            mutation,
            barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
        })
    }

    fn checked_immutable(
        &self,
        stage: CanonicalDurabilityStage,
        injected_fault: Option<CanonicalDurabilityStage>,
        operation: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if injected_fault == Some(stage) {
            return Err(io::Error::other("injected immutable exact cut"));
        }
        self.checked(stage, operation)
    }

    /// Append bytes whose immediate canonical parent is the bound directory.
    ///
    /// Existing files require write, flush, and file `sync_all`. A first-create
    /// additionally requires matching opaque qualification evidence and parent
    /// directory `sync_all` on a supported host platform.
    pub fn append(
        &self,
        path: &Path,
        bytes: &[u8],
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        if let Err(rejection) = self.preflight_mutation_targets([path]) {
            return CanonicalDurabilityOutcome::Rejected(rejection);
        }
        let _operation = match self.operation_lock.lock() {
            Ok(operation) => operation,
            Err(_) => {
                return CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::CoordinationPoisoned,
                );
            }
        };
        let parent = match self.namespace.bind_parent(path, self.platform) {
            Ok(parent) => parent,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        let namespace_qualified = match self.qualification_status(qualification) {
            Ok(status) => status,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };

        let opened = if namespace_qualified {
            if !parent.barrier.is_available() {
                return CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                        platform: self.platform,
                        operation: CanonicalNamespaceOperation::FirstCreate,
                    },
                );
            }
            self.wait_before_atomic_create();
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(file) => OpenedCanonicalFile::New {
                    file,
                    parent: parent.barrier,
                },
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    match self.open_existing_regular(path) {
                        Ok(file) => OpenedCanonicalFile::Existing(file),
                        Err(rejection) => {
                            return CanonicalDurabilityOutcome::Rejected(rejection);
                        }
                    }
                }
                Err(error) => {
                    return CanonicalDurabilityOutcome::Rejected(durability_io(
                        CanonicalDurabilityStage::CreateNew,
                        &error,
                    ));
                }
            }
        } else {
            match self.open_existing_regular(path) {
                Ok(file) => OpenedCanonicalFile::Existing(file),
                Err(CanonicalDurabilityRejection::IoFailedBeforeMutation {
                    kind: io::ErrorKind::NotFound,
                    ..
                }) => {
                    return CanonicalDurabilityOutcome::Rejected(
                        CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                            platform: self.platform,
                            operation: CanonicalNamespaceOperation::FirstCreate,
                        },
                    );
                }
                Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
            }
        };

        self.append_opened(opened, bytes, recovery_key)
    }

    /// Publish a regular source file under a new unique name in the same bound
    /// directory. Cross-directory rename is outside this module's interface.
    pub fn rename(
        &self,
        source: &Path,
        destination: &Path,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        self.rename_inner(source, destination, qualification, recovery_key, false)
    }

    pub(crate) fn rename_recovery(
        &self,
        source: &Path,
        destination: &Path,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        self.rename_inner(source, destination, qualification, recovery_key, true)
    }

    fn rename_inner(
        &self,
        source: &Path,
        destination: &Path,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
        descendant: bool,
    ) -> CanonicalDurabilityOutcome {
        if let Err(rejection) = self.preflight_mutation_targets([source, destination]) {
            return CanonicalDurabilityOutcome::Rejected(rejection);
        }
        let _operation = match self.operation_lock.lock() {
            Ok(operation) => operation,
            Err(_) => {
                return CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::CoordinationPoisoned,
                );
            }
        };
        if !self.namespace_is_supported() {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: self.platform,
                    operation: CanonicalNamespaceOperation::Rename,
                },
            );
        }
        let bind_parent = |path| {
            if descendant {
                self.namespace.bind_descendant_parent(path, self.platform)
            } else {
                self.namespace.bind_parent(path, self.platform)
            }
        };
        let source_parent = match bind_parent(source) {
            Ok(parent) => parent,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        let destination_parent = match bind_parent(destination) {
            Ok(parent) => parent,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        let namespace_qualified = match self.qualification_status(qualification) {
            Ok(status) => status,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        if !namespace_qualified {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: self.platform,
                    operation: CanonicalNamespaceOperation::Rename,
                },
            );
        }
        let source_parent_directory = &source_parent.barrier;
        if !source_parent_directory.is_available() {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: self.platform,
                    operation: CanonicalNamespaceOperation::Rename,
                },
            );
        }
        if source_parent.canonical_path != destination_parent.canonical_path {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::TargetOutsideManagedNamespace,
            );
        }

        let source_file = match self.open_existing_regular(source) {
            Ok(file) => file,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        let source_metadata = match source_file.metadata() {
            Ok(metadata) => metadata,
            Err(error) => {
                return CanonicalDurabilityOutcome::Rejected(durability_io(
                    CanonicalDurabilityStage::OpenExisting,
                    &error,
                ));
            }
        };
        if self.preflight_volumes_differ(
            &self.observed_filesystem_identity(&source_metadata),
            &source_parent.identity,
        ) {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::CrossDeviceRenameRefused { raw_os_error: None },
            );
        }
        if let Err(rejection) = ensure_destination_absent(destination) {
            return CanonicalDurabilityOutcome::Rejected(rejection);
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::FileSync, || {
            source_file.sync_all()
        }) {
            return indeterminate(CanonicalDurabilityStage::FileSync, &error, recovery_key);
        }
        #[cfg(test)]
        if descendant {
            crate::persistence::canonical_crash_harness::checkpoint("quarantine_rename_before");
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::Rename, || {
            self.rename_source(source, destination)
        }) {
            return indeterminate(CanonicalDurabilityStage::Rename, &error, recovery_key);
        }
        #[cfg(test)]
        if descendant {
            crate::persistence::canonical_crash_harness::checkpoint("quarantine_rename_after");
            crate::persistence::canonical_crash_harness::checkpoint(
                "quarantine_parent_sync_before",
            );
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::ParentSync, || {
            source_parent_directory.sync_all()
        }) {
            return indeterminate(CanonicalDurabilityStage::ParentSync, &error, recovery_key);
        }
        #[cfg(test)]
        if descendant {
            crate::persistence::canonical_crash_harness::checkpoint("quarantine_parent_sync_after");
        }

        CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
            mutation: CanonicalMutation::Rename,
            barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
        })
    }

    /// Atomically install a complete snapshot under this guard.
    ///
    /// The caller supplies one exact temporary pathname in the same managed
    /// directory. It is created exclusively and never opened as an append
    /// fallback. An existing destination expectation must carry the same open
    /// file from which the caller validated its generation. `Accepted` follows
    /// owner-only protection, complete temp write and flush, temp `sync_all`,
    /// one atomic same-directory install, and the qualified parent-directory
    /// barrier. Windows and unqualified namespaces refuse before temp creation.
    /// Once the temp may be visible, every failed barrier or destination
    /// revalidation is indeterminate and retains a caller recovery key.
    pub fn install_snapshot(
        &self,
        temporary: &Path,
        destination: &Path,
        bytes: &[u8],
        expectation: CanonicalSnapshotExpectation<'_>,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        self.install_snapshot_inner(
            temporary,
            destination,
            bytes,
            expectation,
            qualification,
            recovery_key,
            false,
            SnapshotDisposition::Install,
            None,
        )
    }

    /// Install a snapshot that may resume an already-existing exact temporary.
    ///
    /// `Rejected` means THIS operation mutated nothing — every refusal is proven
    /// before a byte or namespace entry moved. It is NOT a claim that the
    /// temporary pathname is empty: the resumed temporary may pre-date this call
    /// and outlives every refusal, so the caller owns classifying that survivor
    /// in its own vocabulary.
    /// `ManifestCasRejection::ManifestInstallRefusedAfterProofAndIntentDurable`
    /// is the worked example.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn install_snapshot_recovery(
        &self,
        temporary: &Path,
        destination: &Path,
        bytes: &[u8],
        expectation: CanonicalSnapshotExpectation<'_>,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        self.install_snapshot_inner(
            temporary,
            destination,
            bytes,
            expectation,
            qualification,
            recovery_key,
            true,
            SnapshotDisposition::Install,
            None,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn install_snapshot_recovery_with_fault(
        &self,
        temporary: &Path,
        destination: &Path,
        bytes: &[u8],
        expectation: CanonicalSnapshotExpectation<'_>,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
        injected_fault: CanonicalDurabilityStage,
    ) -> CanonicalDurabilityOutcome {
        self.install_snapshot_inner(
            temporary,
            destination,
            bytes,
            expectation,
            qualification,
            recovery_key,
            true,
            SnapshotDisposition::Install,
            Some(injected_fault),
        )
    }

    /// Durably stage one exact snapshot temporary without installing it.
    /// Exact recovery may resume an existing regular prefix; the destination
    /// expectation remains validated under this guard.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn stage_snapshot_temporary(
        &self,
        temporary: &Path,
        destination: &Path,
        bytes: &[u8],
        expectation: CanonicalSnapshotExpectation<'_>,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        self.install_snapshot_inner(
            temporary,
            destination,
            bytes,
            expectation,
            qualification,
            recovery_key,
            true,
            SnapshotDisposition::StageTemporary,
            None,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn stage_snapshot_temporary_with_fault(
        &self,
        temporary: &Path,
        destination: &Path,
        bytes: &[u8],
        expectation: CanonicalSnapshotExpectation<'_>,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
        injected_fault: CanonicalDurabilityStage,
    ) -> CanonicalDurabilityOutcome {
        self.install_snapshot_inner(
            temporary,
            destination,
            bytes,
            expectation,
            qualification,
            recovery_key,
            true,
            SnapshotDisposition::StageTemporary,
            Some(injected_fault),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn install_snapshot_inner(
        &self,
        temporary: &Path,
        destination: &Path,
        bytes: &[u8],
        expectation: CanonicalSnapshotExpectation<'_>,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
        resume_temporary: bool,
        disposition: SnapshotDisposition,
        injected_fault: Option<CanonicalDurabilityStage>,
    ) -> CanonicalDurabilityOutcome {
        if let Err(rejection) = self.preflight_mutation_targets([temporary, destination]) {
            return CanonicalDurabilityOutcome::Rejected(rejection);
        }
        if snapshot_paths_overlap(temporary, destination) {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::SnapshotPathOverlap,
            );
        }
        let _operation = match self.operation_lock.lock() {
            Ok(operation) => operation,
            Err(_) => {
                return CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::CoordinationPoisoned,
                );
            }
        };
        if !self.namespace_is_supported() {
            return snapshot_namespace_unsupported(self.platform);
        }
        let namespace_qualified = match self.qualification_status(qualification) {
            Ok(true) => true,
            Ok(false) => false,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        if !namespace_qualified {
            return snapshot_namespace_unsupported(self.platform);
        }

        let temporary_parent = match self.namespace.bind_parent(temporary, self.platform) {
            Ok(parent) => parent,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        let destination_parent = match self.namespace.bind_parent(destination, self.platform) {
            Ok(parent) => parent,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        if temporary_parent.canonical_path != destination_parent.canonical_path {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::TargetOutsideManagedNamespace,
            );
        }
        if snapshot_basenames_overlap(temporary, destination) {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::SnapshotPathOverlap,
            );
        }
        if !temporary_parent.barrier.is_available() {
            return snapshot_namespace_unsupported(self.platform);
        }

        self.wait_before_existing_open();
        let validated_destination =
            match self.validate_snapshot_destination(destination, expectation) {
                Ok(destination) => destination,
                Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
            };
        if self.preflight_volumes_differ(
            validated_destination.filesystem_identity(),
            &self.namespace.identity,
        ) {
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::CrossDeviceRenameRefused { raw_os_error: None },
            );
        }

        if injected_fault == Some(CanonicalDurabilityStage::CreateNew) {
            return CanonicalDurabilityOutcome::Rejected(durability_io(
                CanonicalDurabilityStage::CreateNew,
                &io::Error::other("injected snapshot create cut"),
            ));
        }
        if let Some(error) = self.injected_error(CanonicalDurabilityStage::CreateNew) {
            return CanonicalDurabilityOutcome::Rejected(durability_io(
                CanonicalDurabilityStage::CreateNew,
                &error,
            ));
        }
        self.wait_before_atomic_create();
        let (mut temporary_file, already_written) =
            match open_snapshot_temporary(temporary, bytes, resume_temporary) {
                Ok(opened) => opened,
                Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
            };
        if injected_fault == Some(CanonicalDurabilityStage::PostCreate) && already_written == 0 {
            return indeterminate(
                CanonicalDurabilityStage::PostCreate,
                &io::Error::other("injected snapshot post-create cut"),
                recovery_key,
            );
        }

        let mut writer = BufWriter::new(&mut temporary_file);
        let remaining = &bytes[already_written..];
        if injected_fault == Some(CanonicalDurabilityStage::Write) && !remaining.is_empty() {
            let partial = (remaining.len() / 2).max(1).min(remaining.len());
            if let Err(error) = writer.write_all(&remaining[..partial]) {
                return indeterminate(CanonicalDurabilityStage::Write, &error, recovery_key);
            }
            return indeterminate(
                CanonicalDurabilityStage::Write,
                &io::Error::other("injected recovery snapshot write cut"),
                recovery_key,
            );
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::Write, || {
            writer.write_all(remaining)
        }) {
            return indeterminate(CanonicalDurabilityStage::Write, &error, recovery_key);
        }
        if let Err(error) =
            self.checked_snapshot(CanonicalDurabilityStage::Flush, injected_fault, || {
                writer.flush()
            })
        {
            return indeterminate(CanonicalDurabilityStage::Flush, &error, recovery_key);
        }
        drop(writer);
        if let Err(error) = self.checked_snapshot(
            CanonicalDurabilityStage::ProtectTemp,
            injected_fault,
            || apply_owner_only_protection(&temporary_file),
        ) {
            return indeterminate(CanonicalDurabilityStage::ProtectTemp, &error, recovery_key);
        }
        if let Err(error) =
            self.checked_snapshot(CanonicalDurabilityStage::FileSync, injected_fault, || {
                temporary_file.sync_all()
            })
        {
            return indeterminate(CanonicalDurabilityStage::FileSync, &error, recovery_key);
        }

        self.wait_before_snapshot_revalidation();
        if self
            .validate_snapshot_destination(
                temporary,
                CanonicalSnapshotExpectation::Existing(&temporary_file),
            )
            .is_err()
        {
            return indeterminate(
                CanonicalDurabilityStage::InspectEntry,
                &io::Error::other("snapshot temporary identity changed"),
                recovery_key,
            );
        }
        if let Err(error) = revalidate_snapshot_destination(destination, &validated_destination) {
            return indeterminate(CanonicalDurabilityStage::InspectEntry, &error, recovery_key);
        }
        if disposition == SnapshotDisposition::StageTemporary {
            if let Err(error) =
                self.checked_snapshot(CanonicalDurabilityStage::ParentSync, injected_fault, || {
                    temporary_parent.barrier.sync_all()
                })
            {
                return indeterminate(CanonicalDurabilityStage::ParentSync, &error, recovery_key);
            }
            return CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::SnapshotIntentStage,
                barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
            });
        }
        if let Err(error) =
            self.checked_snapshot(CanonicalDurabilityStage::Rename, injected_fault, || {
                self.rename_source(temporary, destination)
            })
        {
            return indeterminate(CanonicalDurabilityStage::Rename, &error, recovery_key);
        }
        if let Err(error) =
            self.checked_snapshot(CanonicalDurabilityStage::ParentSync, injected_fault, || {
                temporary_parent.barrier.sync_all()
            })
        {
            return indeterminate(CanonicalDurabilityStage::ParentSync, &error, recovery_key);
        }

        CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
            mutation: validated_destination.mutation(),
            barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
        })
    }

    fn checked_snapshot(
        &self,
        stage: CanonicalDurabilityStage,
        injected_fault: Option<CanonicalDurabilityStage>,
        operation: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if injected_fault == Some(stage) {
            return Err(io::Error::other("injected recovery snapshot cut"));
        }
        self.checked(stage, operation)
    }

    fn preflight_mutation_targets<const N: usize>(
        &self,
        targets: [&Path; N],
    ) -> Result<(), CanonicalDurabilityRejection> {
        #[cfg(test)]
        if self
            .algorithm_test
            .as_ref()
            .is_some_and(|binding| !binding.is_current())
        {
            return Err(CanonicalDurabilityRejection::IdentityChanged);
        }
        if targets.into_iter().any(|target| {
            self.reserved_internal_names
                .is_reserved_coordination_entry(target)
        }) {
            return Err(CanonicalDurabilityRejection::ReservedCoordinationEntry);
        }
        Ok(())
    }

    fn qualification_status(
        &self,
        qualification: Option<&CanonicalFilesystemQualification>,
    ) -> Result<bool, CanonicalDurabilityRejection> {
        let Some(qualification) = qualification else {
            return Ok(false);
        };
        if qualification.namespace != self.namespace
            || qualification.token != self.qualification_token
        {
            return Err(CanonicalDurabilityRejection::QualificationBindingMismatch);
        }
        Ok(self.namespace_is_supported())
    }

    fn namespace_is_supported(&self) -> bool {
        #[cfg(test)]
        if self.algorithm_test.is_some() {
            return true;
        }
        namespace_supported_for(self.platform)
    }

    fn validate_qualified_descendant_volume(
        &self,
        parent: &BoundParent,
        qualification: &CanonicalFilesystemQualification,
    ) -> Result<(), CanonicalDurabilityRejection> {
        if qualification.namespace != self.namespace {
            return Err(CanonicalDurabilityRejection::QualificationBindingMismatch);
        }
        #[cfg(test)]
        if let Some(injection) = &self.injected_rename_fault {
            match injection.fault {
                InjectedRenameFault::DescendantQualificationMismatch => {
                    return Err(CanonicalDurabilityRejection::QualifiedDescendantVolumeMismatch);
                }
                InjectedRenameFault::DescendantQualificationUnavailable => {
                    return Err(CanonicalDurabilityRejection::QualifiedDescendantVolumeUnavailable);
                }
                _ => {}
            }
        }
        match (
            qualification.namespace.identity.volume.as_ref(),
            parent.identity.volume.as_ref(),
        ) {
            (Some(qualified), Some(observed)) if qualified == observed => Ok(()),
            (Some(_), Some(_)) => {
                Err(CanonicalDurabilityRejection::QualifiedDescendantVolumeMismatch)
            }
            _ => Err(CanonicalDurabilityRejection::QualifiedDescendantVolumeUnavailable),
        }
    }

    fn preflight_volumes_differ(
        &self,
        source: &FilesystemIdentity,
        parent: &FilesystemIdentity,
    ) -> bool {
        #[cfg(test)]
        if self
            .injected_rename_fault
            .as_ref()
            .is_some_and(|injection| {
                matches!(
                    injection.fault,
                    InjectedRenameFault::PreflightDeviceMismatch
                )
            })
        {
            return true;
        }
        volumes_differ(source, parent)
    }

    fn rename_source(&self, source: &Path, destination: &Path) -> io::Result<()> {
        #[cfg(test)]
        if let Some(injection) = &self.injected_rename_fault {
            injection.rename_invoked.store(true, Ordering::SeqCst);
            if let InjectedRenameFault::RuntimeExdev { raw_os_error } = injection.fault {
                return Err(io::Error::from_raw_os_error(raw_os_error));
            }
        }
        std::fs::rename(source, destination)
    }

    fn append_opened(
        &self,
        opened: OpenedCanonicalFile,
        bytes: &[u8],
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        let (mut file, parent) = match opened {
            OpenedCanonicalFile::Existing(mut file) => {
                if let Err(error) = self.checked(CanonicalDurabilityStage::SeekEnd, || {
                    file.seek(SeekFrom::End(0)).map(|_| ())
                }) {
                    return CanonicalDurabilityOutcome::Rejected(durability_io(
                        CanonicalDurabilityStage::SeekEnd,
                        &error,
                    ));
                }
                (file, None)
            }
            OpenedCanonicalFile::New { file, parent } => (file, Some(parent)),
        };
        let mutation = if parent.is_some() {
            CanonicalMutation::FirstCreate
        } else {
            CanonicalMutation::ExistingAppend
        };

        let mut writer = BufWriter::new(&mut file);
        if let Err(error) =
            self.checked(CanonicalDurabilityStage::Write, || writer.write_all(bytes))
        {
            return indeterminate(CanonicalDurabilityStage::Write, &error, recovery_key);
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::Flush, || writer.flush()) {
            return indeterminate(CanonicalDurabilityStage::Flush, &error, recovery_key);
        }
        drop(writer);
        #[cfg(test)]
        if parent.is_some() {
            crate::persistence::canonical_crash_harness::checkpoint(
                "first_create_file_sync_before",
            );
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::FileSync, || file.sync_all()) {
            return indeterminate(CanonicalDurabilityStage::FileSync, &error, recovery_key);
        }
        #[cfg(test)]
        if parent.is_some() {
            crate::persistence::canonical_crash_harness::checkpoint("first_create_file_sync_after");
        }

        if let Some(parent) = parent {
            #[cfg(test)]
            crate::persistence::canonical_crash_harness::checkpoint(
                "first_create_parent_sync_before",
            );
            if let Err(error) =
                self.checked(CanonicalDurabilityStage::ParentSync, || parent.sync_all())
            {
                return indeterminate(CanonicalDurabilityStage::ParentSync, &error, recovery_key);
            }
            #[cfg(test)]
            crate::persistence::canonical_crash_harness::checkpoint(
                "first_create_parent_sync_after",
            );
            return CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation,
                barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
            });
        }

        CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
            mutation,
            barrier: CanonicalDurabilityBarrier::FileDataAndMetadata,
        })
    }

    fn open_existing_regular(&self, path: &Path) -> Result<File, CanonicalDurabilityRejection> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|error| durability_io(CanonicalDurabilityStage::InspectEntry, &error))?;
        if !metadata.file_type().is_file() {
            return Err(CanonicalDurabilityRejection::NonRegularCanonicalEntry);
        }
        let expected_identity = self.observed_filesystem_identity(&metadata);
        self.wait_before_existing_open();
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map_err(|error| durability_io(CanonicalDurabilityStage::OpenExisting, &error))?;
        let opened_metadata = file
            .metadata()
            .map_err(|error| durability_io(CanonicalDurabilityStage::OpenExisting, &error))?;
        #[cfg(test)]
        if uses_synthetic_algorithm_identity(&self.namespace.identity)
            && !algorithm_test_path_names_open_file(path, &file)
                .map_err(|error| durability_io(CanonicalDurabilityStage::OpenExisting, &error))?
        {
            return Err(CanonicalDurabilityRejection::IdentityChanged);
        }
        #[cfg(not(test))]
        let observed_identity = filesystem_identity(&opened_metadata);
        #[cfg(test)]
        let observed_identity = self.observed_filesystem_identity(&opened_metadata);
        if identity_is_available(&expected_identity) && observed_identity != expected_identity {
            return Err(CanonicalDurabilityRejection::IdentityChanged);
        }
        Ok(file)
    }

    fn observed_filesystem_identity(&self, metadata: &Metadata) -> FilesystemIdentity {
        #[cfg(test)]
        if uses_synthetic_algorithm_identity(&self.namespace.identity) {
            return self.namespace.identity.clone();
        }
        filesystem_identity(metadata)
    }

    fn validate_snapshot_destination(
        &self,
        destination: &Path,
        expectation: CanonicalSnapshotExpectation<'_>,
    ) -> Result<ValidatedSnapshotDestination, CanonicalDurabilityRejection> {
        #[cfg(test)]
        if uses_synthetic_algorithm_identity(&self.namespace.identity) {
            return validate_algorithm_test_snapshot_destination(
                destination,
                expectation,
                &self.namespace.identity,
            );
        }
        validate_snapshot_destination(destination, expectation)
    }

    fn checked(
        &self,
        _stage: CanonicalDurabilityStage,
        operation: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if let Some(error) = self.injected_error(_stage) {
            return Err(error);
        }
        operation()
    }

    fn injected_error(&self, _stage: CanonicalDurabilityStage) -> Option<io::Error> {
        #[cfg(test)]
        if let Some(failure) = self.injected_failure
            && failure.stage == _stage
        {
            return Some(match failure.raw_os_error {
                Some(raw_os_error) => io::Error::from_raw_os_error(raw_os_error),
                None => io::Error::other("injected failure"),
            });
        }
        None
    }

    fn wait_before_atomic_create(&self) {
        #[cfg(test)]
        if let Some(barrier) = &self.before_atomic_create {
            barrier.wait();
            barrier.wait();
        }
    }

    fn wait_before_existing_open(&self) {
        #[cfg(test)]
        if let Some(barrier) = &self.before_existing_open {
            barrier.wait();
            barrier.wait();
        }
    }

    fn wait_before_snapshot_revalidation(&self) {
        #[cfg(test)]
        if let Some(barrier) = &self.before_snapshot_revalidation {
            barrier.wait();
            barrier.wait();
        }
    }
}

enum OpenedCanonicalFile {
    Existing(File),
    New {
        file: File,
        parent: ParentDurabilityBarrier,
    },
}

enum ValidatedSnapshotDestination {
    Absent,
    Existing(FilesystemIdentity),
    #[cfg(test)]
    ExistingAlgorithmTest {
        expected_file: File,
        identity: FilesystemIdentity,
    },
}

impl ValidatedSnapshotDestination {
    fn filesystem_identity(&self) -> &FilesystemIdentity {
        match self {
            Self::Absent => &ABSENT_FILESYSTEM_IDENTITY,
            Self::Existing(identity) => identity,
            #[cfg(test)]
            Self::ExistingAlgorithmTest { identity, .. } => identity,
        }
    }

    fn mutation(&self) -> CanonicalMutation {
        match self {
            Self::Absent => CanonicalMutation::InitialSnapshotInstall,
            Self::Existing(_) => CanonicalMutation::SnapshotReplacement,
            #[cfg(test)]
            Self::ExistingAlgorithmTest { .. } => CanonicalMutation::SnapshotReplacement,
        }
    }
}

const ABSENT_FILESYSTEM_IDENTITY: FilesystemIdentity = FilesystemIdentity {
    volume: None,
    object: None,
};

fn validate_snapshot_destination(
    destination: &Path,
    expectation: CanonicalSnapshotExpectation<'_>,
) -> Result<ValidatedSnapshotDestination, CanonicalDurabilityRejection> {
    match expectation {
        CanonicalSnapshotExpectation::Absent => {
            ensure_destination_absent(destination)?;
            Ok(ValidatedSnapshotDestination::Absent)
        }
        CanonicalSnapshotExpectation::Existing(expected_file) => {
            let expected_metadata = expected_file
                .metadata()
                .map_err(|error| durability_io(CanonicalDurabilityStage::InspectEntry, &error))?;
            if !expected_metadata.file_type().is_file() {
                return Err(CanonicalDurabilityRejection::NonRegularCanonicalEntry);
            }
            let actual_metadata = std::fs::symlink_metadata(destination).map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    CanonicalDurabilityRejection::IdentityChanged
                } else {
                    durability_io(CanonicalDurabilityStage::InspectEntry, &error)
                }
            })?;
            if !actual_metadata.file_type().is_file() {
                return Err(CanonicalDurabilityRejection::NonRegularCanonicalEntry);
            }
            let expected_identity = filesystem_identity(&expected_metadata);
            let actual_identity = filesystem_identity(&actual_metadata);
            if !identity_is_available(&expected_identity) || actual_identity != expected_identity {
                return Err(CanonicalDurabilityRejection::IdentityChanged);
            }
            Ok(ValidatedSnapshotDestination::Existing(expected_identity))
        }
    }
}

#[cfg(test)]
fn validate_algorithm_test_snapshot_destination(
    destination: &Path,
    expectation: CanonicalSnapshotExpectation<'_>,
    identity: &FilesystemIdentity,
) -> Result<ValidatedSnapshotDestination, CanonicalDurabilityRejection> {
    match expectation {
        CanonicalSnapshotExpectation::Absent => {
            ensure_destination_absent(destination)?;
            Ok(ValidatedSnapshotDestination::Absent)
        }
        CanonicalSnapshotExpectation::Existing(expected_file) => {
            let expected_metadata = expected_file
                .metadata()
                .map_err(|error| durability_io(CanonicalDurabilityStage::InspectEntry, &error))?;
            if !expected_metadata.file_type().is_file() {
                return Err(CanonicalDurabilityRejection::NonRegularCanonicalEntry);
            }
            let actual_metadata = std::fs::symlink_metadata(destination).map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    CanonicalDurabilityRejection::IdentityChanged
                } else {
                    durability_io(CanonicalDurabilityStage::InspectEntry, &error)
                }
            })?;
            if !actual_metadata.file_type().is_file() {
                return Err(CanonicalDurabilityRejection::NonRegularCanonicalEntry);
            }
            if !algorithm_test_path_names_open_file(destination, expected_file)
                .map_err(|error| durability_io(CanonicalDurabilityStage::InspectEntry, &error))?
            {
                return Err(CanonicalDurabilityRejection::IdentityChanged);
            }
            let retained = expected_file
                .try_clone()
                .map_err(|error| durability_io(CanonicalDurabilityStage::InspectEntry, &error))?;
            Ok(ValidatedSnapshotDestination::ExistingAlgorithmTest {
                expected_file: retained,
                identity: identity.clone(),
            })
        }
    }
}

fn revalidate_snapshot_destination(
    destination: &Path,
    validated: &ValidatedSnapshotDestination,
) -> io::Result<()> {
    match validated {
        ValidatedSnapshotDestination::Absent => match std::fs::symlink_metadata(destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "snapshot destination appeared before installation",
            )),
            Err(error) => Err(error),
        },
        ValidatedSnapshotDestination::Existing(expected_identity) => {
            let metadata = std::fs::symlink_metadata(destination)?;
            if metadata.file_type().is_file()
                && filesystem_identity(&metadata) == *expected_identity
            {
                Ok(())
            } else {
                Err(io::Error::other(
                    "snapshot destination identity changed before installation",
                ))
            }
        }
        #[cfg(test)]
        ValidatedSnapshotDestination::ExistingAlgorithmTest { expected_file, .. } => {
            match algorithm_test_path_names_open_file(destination, expected_file) {
                Ok(true) => Ok(()),
                Ok(false) => Err(io::Error::other(
                    "snapshot destination identity changed before installation",
                )),
                Err(error) => Err(error),
            }
        }
    }
}

#[cfg(unix)]
fn configure_owner_only_creation(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
}

fn ensure_no_ascii_case_alias(path: &Path) -> Result<(), CanonicalDurabilityRejection> {
    let parent = path
        .parent()
        .ok_or(CanonicalDurabilityRejection::TargetOutsideManagedNamespace)?;
    let requested = path
        .file_name()
        .ok_or(CanonicalDurabilityRejection::TargetOutsideManagedNamespace)?;
    let entries = std::fs::read_dir(parent)
        .map_err(|error| durability_io(CanonicalDurabilityStage::InspectEntry, &error))?;
    for entry in entries {
        let observed = entry
            .map_err(|error| durability_io(CanonicalDurabilityStage::InspectEntry, &error))?
            .file_name();
        if observed != requested && observed.eq_ignore_ascii_case(requested) {
            return Err(CanonicalDurabilityRejection::ImmutableExactConflict);
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn configure_owner_only_creation(_options: &mut OpenOptions) {}

#[cfg(unix)]
fn apply_owner_only_protection(file: &File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn apply_owner_only_protection(_file: &File) -> io::Result<()> {
    Ok(())
}

fn open_snapshot_temporary(
    path: &Path,
    expected: &[u8],
    resume: bool,
) -> Result<(File, usize), CanonicalDurabilityRejection> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    configure_owner_only_creation(&mut options);
    match options.open(path) {
        Ok(file) => Ok((file, 0)),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists && resume => {
            let metadata = std::fs::symlink_metadata(path)
                .map_err(|error| durability_io(CanonicalDurabilityStage::InspectEntry, &error))?;
            if !metadata.file_type().is_file() {
                return Err(CanonicalDurabilityRejection::SnapshotTempAlreadyExists);
            }
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .map_err(|error| durability_io(CanonicalDurabilityStage::OpenExisting, &error))?;
            let mut observed = Vec::new();
            (&mut file)
                .take(u64::try_from(expected.len()).unwrap_or(u64::MAX) + 1)
                .read_to_end(&mut observed)
                .map_err(|error| durability_io(CanonicalDurabilityStage::OpenExisting, &error))?;
            if observed.len() > expected.len() || !expected.starts_with(&observed) {
                return Err(CanonicalDurabilityRejection::SnapshotTempAlreadyExists);
            }
            file.seek(SeekFrom::Start(observed.len() as u64))
                .map_err(|error| durability_io(CanonicalDurabilityStage::SeekEnd, &error))?;
            Ok((file, observed.len()))
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(CanonicalDurabilityRejection::SnapshotTempAlreadyExists)
        }
        Err(error) => Err(durability_io(CanonicalDurabilityStage::CreateNew, &error)),
    }
}

fn open_writer_coordination(path: &Path) -> Result<File, CanonicalCoordinationError> {
    match OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => Ok(file),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            ensure_regular_lock_entry(path)?;
            OpenOptions::new()
                .read(true)
                .write(true)
                .open(path)
                .map_err(|error| coordination_io(CanonicalCoordinationStage::Open, &error))
        }
        Err(error) => Err(coordination_io(CanonicalCoordinationStage::Open, &error)),
    }
}

fn ensure_regular_lock_entry(path: &Path) -> Result<(), CanonicalCoordinationError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(CanonicalCoordinationError::NonRegularLockFile),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(CanonicalCoordinationError::Missing)
        }
        Err(error) => Err(coordination_io(
            CanonicalCoordinationStage::InspectLock,
            &error,
        )),
    }
}

fn try_lock_exclusive(file: File) -> Result<File, CanonicalCoordinationError> {
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(CanonicalCoordinationError::Contended),
        Err(TryLockError::Error(error)) => {
            Err(coordination_io(CanonicalCoordinationStage::Lock, &error))
        }
    }
}

fn try_lock_shared(file: File) -> Result<File, CanonicalCoordinationError> {
    match file.try_lock_shared() {
        Ok(()) => Ok(file),
        Err(TryLockError::WouldBlock) => Err(CanonicalCoordinationError::Contended),
        Err(TryLockError::Error(error)) => {
            Err(coordination_io(CanonicalCoordinationStage::Lock, &error))
        }
    }
}

#[cfg(test)]
struct AlgorithmTestFileUnlock<'file>(&'file File);

#[cfg(test)]
impl Drop for AlgorithmTestFileUnlock<'_> {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

/// Compare a retained handle with a pathname without relying on production
/// filesystem identity. This is only an algorithm-test oracle: the retained
/// file is locked briefly and a separately opened pathname must contend on
/// the same object. It is never compiled into production qualification.
#[cfg(test)]
fn algorithm_test_path_names_open_file(path: &Path, expected: &File) -> io::Result<bool> {
    match expected.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "algorithm test identity lock contended",
            ));
        }
        Err(TryLockError::Error(error)) => return Err(error),
    }
    let _unlock = AlgorithmTestFileUnlock(expected);
    let observed = OpenOptions::new().read(true).write(true).open(path)?;
    match observed.try_lock_shared() {
        Err(TryLockError::WouldBlock) => Ok(true),
        Ok(()) => Ok(false),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

fn ensure_destination_absent(path: &Path) -> Result<(), CanonicalDurabilityRejection> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Err(CanonicalDurabilityRejection::DestinationAlreadyExists),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(durability_io(
            CanonicalDurabilityStage::InspectEntry,
            &error,
        )),
    }
}

fn snapshot_paths_overlap(temporary: &Path, destination: &Path) -> bool {
    temporary == destination
        || (parent_directory(temporary) == parent_directory(destination)
            && snapshot_basenames_overlap(temporary, destination))
}

fn snapshot_basenames_overlap(temporary: &Path, destination: &Path) -> bool {
    temporary.file_name().is_some_and(|temporary_name| {
        destination
            .file_name()
            .is_some_and(|destination_name| temporary_name.eq_ignore_ascii_case(destination_name))
    })
}

fn parent_directory(path: &Path) -> PathBuf {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
}

fn next_qualification_token() -> Option<CanonicalQualificationToken> {
    static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);
    NEXT_TOKEN
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .ok()
        .map(CanonicalQualificationToken)
}

fn live_filesystem_inventory() -> Vec<FilesystemObservation> {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    disks
        .list()
        .iter()
        .map(|disk| {
            #[cfg(test)]
            let volume = std::fs::metadata(disk.mount_point())
                .ok()
                .and_then(|metadata| filesystem_identity(&metadata).volume);
            FilesystemObservation {
                mount_point: disk.mount_point().to_path_buf(),
                file_system: disk.file_system().to_os_string(),
                #[cfg(test)]
                volume,
                #[cfg(any(test, target_os = "macos"))]
                live_mount: None,
                removable: disk.is_removable(),
                read_only: disk.is_read_only(),
            }
        })
        .collect()
}

fn resolve_live_filesystem(
    namespace: &ManagedNamespace,
    platform: CanonicalPlatform,
) -> Result<QualifiedFilesystemMount, CanonicalFilesystemQualificationError> {
    if !namespace_supported_for(platform) {
        return Err(
            CanonicalFilesystemQualificationError::NamespaceDurabilityUnsupported { platform },
        );
    }
    match platform {
        CanonicalPlatform::Linux => {
            let observations = live_filesystem_inventory();
            let filesystem =
                assess_filesystem_policy(&namespace.canonical_root, platform, &observations)?;
            validate_mount_volume(&filesystem, &namespace.identity)?;
            Ok(filesystem)
        }
        CanonicalPlatform::MacOs => {
            #[cfg(target_os = "macos")]
            {
                resolve_exact_macos_mount(namespace)
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(CanonicalFilesystemQualificationError::IdentityUnavailable)
            }
        }
        CanonicalPlatform::Windows | CanonicalPlatform::Other => {
            unreachable!("unsupported platforms return before filesystem inventory")
        }
    }
}

#[cfg(any(test, target_os = "macos"))]
fn select_exact_macos_mount(
    root_mount: &LiveMountIdentity,
    observations: &[FilesystemObservation],
) -> Result<QualifiedFilesystemMount, CanonicalFilesystemQualificationError> {
    if observations
        .iter()
        .any(|observation| observation.live_mount.is_none())
    {
        return Err(CanonicalFilesystemQualificationError::IdentityUnavailable);
    }
    let mut matching = observations
        .iter()
        .filter(|observation| observation.live_mount.as_ref() == Some(root_mount));
    let observation = matching
        .next()
        .ok_or(CanonicalFilesystemQualificationError::IdentityUnavailable)?;
    if matching.next().is_some() {
        return Err(CanonicalFilesystemQualificationError::IdentityUnavailable);
    }
    let class = classify_filesystem(&observation.file_system);
    if observation.read_only {
        return Err(CanonicalFilesystemQualificationError::ReadOnlyFilesystem { class });
    }
    if observation.removable {
        return Err(CanonicalFilesystemQualificationError::RemovableFilesystem { class });
    }
    if class != CanonicalFilesystemClass::Apfs {
        return Err(
            CanonicalFilesystemQualificationError::FilesystemUnsupported {
                platform: CanonicalPlatform::MacOs,
                class,
            },
        );
    }
    Ok(QualifiedFilesystemMount {
        mount_point: observation.mount_point.clone(),
        class,
        live_mount: Some(root_mount.clone()),
    })
}

#[cfg(any(test, target_os = "macos"))]
fn require_stable_live_mount(
    before: &LiveMountIdentity,
    after: &LiveMountIdentity,
) -> Result<(), CanonicalFilesystemQualificationError> {
    if before == after {
        Ok(())
    } else {
        Err(CanonicalFilesystemQualificationError::IdentityUnavailable)
    }
}

#[cfg(target_os = "macos")]
fn resolve_exact_macos_mount(
    namespace: &ManagedNamespace,
) -> Result<QualifiedFilesystemMount, CanonicalFilesystemQualificationError> {
    let root_dir = File::open(&namespace.canonical_root)
        .map_err(|error| qualification_io_error(CanonicalCoordinationStage::ResolveRoot, &error))?;
    require_macos_handle_identity(&root_dir, &namespace.identity)?;
    let before = macos_live_mount_identity(&root_dir)?;

    let disks = sysinfo::Disks::new_with_refreshed_list();
    let mut observations = Vec::with_capacity(disks.list().len());
    for disk in disks.list() {
        let mount_dir = File::open(disk.mount_point()).map_err(|error| {
            qualification_io_error(CanonicalCoordinationStage::ResolveRoot, &error)
        })?;
        let metadata = mount_dir.metadata().map_err(|error| {
            qualification_io_error(CanonicalCoordinationStage::ResolveRoot, &error)
        })?;
        if !metadata.is_dir() {
            return Err(CanonicalFilesystemQualificationError::IdentityUnavailable);
        }
        #[cfg(test)]
        let volume = filesystem_identity(&metadata).volume;
        observations.push(FilesystemObservation {
            mount_point: disk.mount_point().to_path_buf(),
            file_system: disk.file_system().to_os_string(),
            #[cfg(test)]
            volume,
            live_mount: Some(macos_live_mount_identity(&mount_dir)?),
            removable: disk.is_removable(),
            read_only: disk.is_read_only(),
        });
    }

    let filesystem = select_exact_macos_mount(&before, &observations)?;
    let after = macos_live_mount_identity(&root_dir)?;
    require_stable_live_mount(&before, &after)?;
    require_macos_handle_identity(&root_dir, &namespace.identity)?;

    let refreshed = ManagedNamespace::load(
        &namespace.canonical_root,
        CanonicalCoordinationError::ParentProvisioningRequired,
    )
    .map_err(qualification_coordination_error)?;
    if &refreshed != namespace {
        return Err(CanonicalFilesystemQualificationError::IdentityUnavailable);
    }
    let refreshed_dir = File::open(&refreshed.canonical_root)
        .map_err(|error| qualification_io_error(CanonicalCoordinationStage::ResolveRoot, &error))?;
    require_macos_handle_identity(&refreshed_dir, &namespace.identity)?;
    let refreshed_mount = macos_live_mount_identity(&refreshed_dir)?;
    require_stable_live_mount(&before, &refreshed_mount)?;
    Ok(filesystem)
}

#[cfg(target_os = "macos")]
fn require_macos_handle_identity(
    directory: &File,
    expected: &FilesystemIdentity,
) -> Result<(), CanonicalFilesystemQualificationError> {
    let metadata = directory
        .metadata()
        .map_err(|error| qualification_io_error(CanonicalCoordinationStage::ResolveRoot, &error))?;
    if metadata.is_dir() && filesystem_identity(&metadata) == *expected {
        Ok(())
    } else {
        Err(CanonicalFilesystemQualificationError::IdentityUnavailable)
    }
}

#[cfg(target_os = "macos")]
fn macos_live_mount_identity(
    directory: &File,
) -> Result<LiveMountIdentity, CanonicalFilesystemQualificationError> {
    nix::sys::statfs::fstatfs(directory)
        .map(|statfs| LiveMountIdentity::MacOs(statfs.filesystem_id()))
        .map_err(|error| {
            let error: io::Error = error.into();
            qualification_io_error(CanonicalCoordinationStage::ResolveRoot, &error)
        })
}

#[cfg(all(test, target_os = "macos"))]
fn emit_cc9a_macos_mount_diagnostics(root: &Path) {
    use std::os::unix::fs::MetadataExt;

    const MARKER: &str = "CC9A_MACOS_DIAGNOSTIC";

    let canonical_root = std::fs::canonicalize(root).ok();
    let root_metadata = canonical_root
        .as_deref()
        .and_then(|canonical_root| std::fs::metadata(canonical_root).ok());
    let root_dev = root_metadata.as_ref().map(MetadataExt::dev);
    let root_dir = canonical_root
        .as_deref()
        .and_then(|canonical_root| File::open(canonical_root).ok());
    let root_mount_before = root_dir
        .as_ref()
        .and_then(|directory| macos_live_mount_identity(directory).ok());
    let observations = live_filesystem_inventory();
    eprintln!(
        "{MARKER} root canonical_root={} root_dev={} inventory_count={} root_fsid_result={}",
        canonical_root
            .as_deref()
            .map_or_else(|| "unavailable".into(), |path| path.display().to_string()),
        root_dev.map_or_else(|| "unavailable".into(), |dev| dev.to_string()),
        observations.len(),
        if root_mount_before.is_some() {
            "available"
        } else {
            "unavailable"
        },
    );

    let mut metadata_unavailable_count = 0_u64;
    let mut same_root_dev_count = 0_u64;
    let mut probe_unavailable_count = 0_u64;
    let mut same_root_fsid_count = 0_u64;
    let mut root_equals_data = None;
    let mut root_differs_system = None;
    let mut unique_mount = None;
    for (index, observation) in observations.iter().enumerate() {
        let mount_dir = File::open(&observation.mount_point).ok();
        let metadata = mount_dir
            .as_ref()
            .and_then(|directory| directory.metadata().ok());
        let observation_mount = mount_dir
            .as_ref()
            .and_then(|directory| macos_live_mount_identity(directory).ok());
        let observation_dev = metadata.as_ref().map(MetadataExt::dev);
        if metadata.is_none() {
            metadata_unavailable_count += 1;
        }
        if observation_mount.is_none() {
            probe_unavailable_count += 1;
        }
        let same_root_dev = match (root_dev, observation_dev) {
            (Some(root_dev), Some(observation_dev)) => {
                let same = root_dev == observation_dev;
                if same {
                    same_root_dev_count += 1;
                    unique_mount = Some(observation.mount_point.clone());
                }
                same.to_string()
            }
            _ => "unavailable".into(),
        };
        let same_root_fsid = match (&root_mount_before, &observation_mount) {
            (Some(root_mount), Some(observation_mount)) => {
                let same = root_mount == observation_mount;
                if same {
                    same_root_fsid_count += 1;
                }
                if observation.mount_point == Path::new("/System/Volumes/Data") {
                    root_equals_data = Some(same);
                }
                if observation.mount_point == Path::new("/") {
                    root_differs_system = Some(!same);
                }
                same.to_string()
            }
            _ => "unavailable".into(),
        };
        eprintln!(
            "{MARKER} observation index={index} mount_path={} filesystem_class={:?} filesystem_string={} metadata_result={} dev={} same_root_dev={} fsid_result={} same_root_fsid={} read_only={} removable={}",
            observation.mount_point.display(),
            classify_filesystem(&observation.file_system),
            observation.file_system.to_string_lossy(),
            if metadata.is_some() {
                "available"
            } else {
                "unavailable"
            },
            observation_dev.map_or_else(|| "unavailable".into(), |dev| dev.to_string()),
            same_root_dev,
            if observation_mount.is_some() {
                "available"
            } else {
                "unavailable"
            },
            same_root_fsid,
            observation.read_only,
            observation.removable,
        );
    }

    let branch = match root_metadata {
        None => "root_missing",
        Some(_) if same_root_dev_count == 0 && metadata_unavailable_count == 0 => {
            "zero_match_clean"
        }
        Some(_) if same_root_dev_count == 0 => "zero_match_with_unavailable",
        Some(_) if same_root_dev_count > 1 => "ambiguous",
        Some(root_metadata) => {
            let root_identity = filesystem_identity(&root_metadata);
            let selected = QualifiedFilesystemMount {
                mount_point: unique_mount.expect("one matching mount must be retained"),
                class: CanonicalFilesystemClass::Apfs,
                #[cfg(any(test, target_os = "macos"))]
                live_mount: None,
            };
            if validate_mount_volume(&selected, &root_identity).is_ok() {
                "unique"
            } else {
                "unique_then_validate_mismatch"
            }
        }
    };
    eprintln!(
        "{MARKER} summary metadata_unavailable_count={metadata_unavailable_count} same_root_dev_count={same_root_dev_count} branch={branch}",
    );
    let root_mount_after = root_dir
        .as_ref()
        .and_then(|directory| macos_live_mount_identity(directory).ok());
    let root_before_after_stable = match (&root_mount_before, &root_mount_after) {
        (Some(before), Some(after)) => Some(before == after),
        _ => None,
    };
    let relation = |value: Option<bool>| {
        value.map_or_else(|| "unavailable".to_owned(), |value| value.to_string())
    };
    eprintln!(
        "{MARKER} exact root_equals_data={} root_differs_system={} same_root_fsid_count={same_root_fsid_count} probe_unavailable_count={probe_unavailable_count} root_before_after_stable={} selection_authority=fsid mounted_on_text_authority=false",
        relation(root_equals_data),
        relation(root_differs_system),
        relation(root_before_after_stable),
    );
}

fn assess_filesystem_policy(
    canonical_root: &Path,
    platform: CanonicalPlatform,
    observations: &[FilesystemObservation],
) -> Result<QualifiedFilesystemMount, CanonicalFilesystemQualificationError> {
    if !namespace_supported_for(platform) {
        return Err(
            CanonicalFilesystemQualificationError::NamespaceDurabilityUnsupported { platform },
        );
    }
    let observation = match platform {
        CanonicalPlatform::Linux => observations
            .iter()
            .filter(|observation| canonical_root.starts_with(&observation.mount_point))
            .max_by_key(|observation| observation.mount_point.components().count())
            .ok_or(CanonicalFilesystemQualificationError::NoMatchingMount)?,
        CanonicalPlatform::MacOs => {
            return Err(CanonicalFilesystemQualificationError::IdentityUnavailable);
        }
        CanonicalPlatform::Windows | CanonicalPlatform::Other => {
            unreachable!("unsupported platforms return before filesystem observation selection")
        }
    };
    let class = classify_filesystem(&observation.file_system);
    if observation.read_only {
        return Err(CanonicalFilesystemQualificationError::ReadOnlyFilesystem { class });
    }
    if observation.removable {
        return Err(CanonicalFilesystemQualificationError::RemovableFilesystem { class });
    }
    let allowed = matches!(
        (platform, class),
        (CanonicalPlatform::Linux, CanonicalFilesystemClass::Ext4)
            | (CanonicalPlatform::MacOs, CanonicalFilesystemClass::Apfs)
    );
    if !allowed {
        return Err(
            CanonicalFilesystemQualificationError::FilesystemUnsupported { platform, class },
        );
    }
    Ok(QualifiedFilesystemMount {
        mount_point: observation.mount_point.clone(),
        class,
        #[cfg(any(test, target_os = "macos"))]
        live_mount: None,
    })
}

fn classify_filesystem(file_system: &OsStr) -> CanonicalFilesystemClass {
    let Some(file_system) = file_system.to_str() else {
        return CanonicalFilesystemClass::Other;
    };
    let normalized = file_system.to_ascii_lowercase();
    match normalized.as_str() {
        "ext4" => CanonicalFilesystemClass::Ext4,
        "apfs" => CanonicalFilesystemClass::Apfs,
        "tmpfs" | "devtmpfs" | "ramfs" => CanonicalFilesystemClass::Temporary,
        "nfs" | "nfs4" | "cifs" | "smbfs" | "9p" | "afs" | "ceph" => {
            CanonicalFilesystemClass::Remote
        }
        other if other == "fuse" || other.starts_with("fuse.") => CanonicalFilesystemClass::Fuse,
        _ => CanonicalFilesystemClass::Other,
    }
}

fn validate_mount_volume(
    filesystem: &QualifiedFilesystemMount,
    root_identity: &FilesystemIdentity,
) -> Result<(), CanonicalFilesystemQualificationError> {
    let metadata = std::fs::metadata(&filesystem.mount_point)
        .map_err(|error| qualification_io_error(CanonicalCoordinationStage::ResolveRoot, &error))?;
    let mount_identity = filesystem_identity(&metadata);
    match (
        root_identity.volume.as_ref(),
        mount_identity.volume.as_ref(),
    ) {
        (Some(root), Some(mount)) if root == mount => Ok(()),
        _ => Err(CanonicalFilesystemQualificationError::IdentityUnavailable),
    }
}

fn qualification_io_error(
    stage: CanonicalCoordinationStage,
    error: &io::Error,
) -> CanonicalFilesystemQualificationError {
    CanonicalFilesystemQualificationError::Io {
        stage,
        kind: error.kind(),
        raw_os_error: error.raw_os_error(),
    }
}

fn qualification_coordination_error(
    error: CanonicalCoordinationError,
) -> CanonicalFilesystemQualificationError {
    match error {
        CanonicalCoordinationError::ParentProvisioningRequired
        | CanonicalCoordinationError::Missing => {
            CanonicalFilesystemQualificationError::ParentProvisioningRequired
        }
        CanonicalCoordinationError::ManagedRootNotDirectory => {
            CanonicalFilesystemQualificationError::ManagedRootNotDirectory
        }
        CanonicalCoordinationError::IdentityUnavailable
        | CanonicalCoordinationError::Contended
        | CanonicalCoordinationError::NonRegularLockFile
        | CanonicalCoordinationError::QualificationBindingMismatch => {
            CanonicalFilesystemQualificationError::IdentityUnavailable
        }
        CanonicalCoordinationError::QualificationRefused(error) => error,
        CanonicalCoordinationError::Io {
            stage,
            kind,
            raw_os_error,
        } => CanonicalFilesystemQualificationError::Io {
            stage,
            kind,
            raw_os_error,
        },
    }
}

const fn current_platform() -> CanonicalPlatform {
    if cfg!(target_os = "linux") {
        CanonicalPlatform::Linux
    } else if cfg!(target_os = "macos") {
        CanonicalPlatform::MacOs
    } else if cfg!(target_os = "windows") {
        CanonicalPlatform::Windows
    } else {
        CanonicalPlatform::Other
    }
}

const fn namespace_supported_for(platform: CanonicalPlatform) -> bool {
    matches!(
        platform,
        CanonicalPlatform::Linux | CanonicalPlatform::MacOs
    )
}

fn filesystem_identity(metadata: &Metadata) -> FilesystemIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        FilesystemIdentity {
            volume: Some(VolumeIdentity::UnixDevice(metadata.dev())),
            object: Some(ObjectIdentity::UnixInode(metadata.ino())),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        FilesystemIdentity {
            volume: None,
            object: None,
        }
    }
}

#[cfg(test)]
fn synthetic_algorithm_filesystem_identity(token: u64) -> FilesystemIdentity {
    FilesystemIdentity {
        volume: Some(VolumeIdentity::SyntheticAlgorithmNamespace(token)),
        object: Some(ObjectIdentity::SyntheticAlgorithmObject(token)),
    }
}

#[cfg(test)]
fn synthetic_algorithm_identity_token(identity: &FilesystemIdentity) -> Option<u64> {
    match (&identity.volume, &identity.object) {
        (
            Some(VolumeIdentity::SyntheticAlgorithmNamespace(volume)),
            Some(ObjectIdentity::SyntheticAlgorithmObject(object)),
        ) if volume == object => Some(*volume),
        _ => None,
    }
}

#[cfg(test)]
fn uses_synthetic_algorithm_identity(identity: &FilesystemIdentity) -> bool {
    synthetic_algorithm_identity_token(identity).is_some()
}

fn identity_is_available(identity: &FilesystemIdentity) -> bool {
    identity.volume.is_some() && identity.object.is_some()
}

fn volumes_differ(left: &FilesystemIdentity, right: &FilesystemIdentity) -> bool {
    matches!(
        (&left.volume, &right.volume),
        (Some(left), Some(right)) if left != right
    )
}

fn coordination_io(
    stage: CanonicalCoordinationStage,
    error: &io::Error,
) -> CanonicalCoordinationError {
    CanonicalCoordinationError::Io {
        stage,
        kind: error.kind(),
        raw_os_error: error.raw_os_error(),
    }
}

fn durability_io(
    stage: CanonicalDurabilityStage,
    error: &io::Error,
) -> CanonicalDurabilityRejection {
    CanonicalDurabilityRejection::IoFailedBeforeMutation {
        stage,
        kind: error.kind(),
        raw_os_error: error.raw_os_error(),
    }
}

fn indeterminate(
    stage: CanonicalDurabilityStage,
    error: &io::Error,
    recovery_key: CanonicalRecoveryKey,
) -> CanonicalDurabilityOutcome {
    CanonicalDurabilityOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
        stage,
        kind: error.kind(),
        raw_os_error: error.raw_os_error(),
        recovery_key,
    })
}

fn snapshot_namespace_unsupported(platform: CanonicalPlatform) -> CanonicalDurabilityOutcome {
    CanonicalDurabilityOutcome::Rejected(
        CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
            platform,
            operation: CanonicalNamespaceOperation::AtomicSnapshotInstall,
        },
    )
}

fn immutable_namespace_unsupported(platform: CanonicalPlatform) -> CanonicalDurabilityOutcome {
    CanonicalDurabilityOutcome::Rejected(
        CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
            platform,
            operation: CanonicalNamespaceOperation::FirstCreate,
        },
    )
}

fn unlink_namespace_unsupported(platform: CanonicalPlatform) -> CanonicalUnlinkOutcome {
    CanonicalUnlinkOutcome::Rejected(
        CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
            platform,
            operation: CanonicalNamespaceOperation::Unlink,
        },
    )
}

fn unlink_indeterminate(
    stage: CanonicalDurabilityStage,
    error: &io::Error,
    recovery_key: CanonicalRecoveryKey,
) -> CanonicalUnlinkOutcome {
    CanonicalUnlinkOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
        stage,
        kind: error.kind(),
        raw_os_error: error.raw_os_error(),
        recovery_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::thread;

    #[cfg(unix)]
    use std::os::unix::fs::FileTypeExt;

    fn temp_root(label: &str) -> PathBuf {
        static NONCE: AtomicU64 = AtomicU64::new(0);
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "ag-canonical-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn qualification(root: &Path) -> CanonicalFilesystemQualification {
        CanonicalFilesystemQualification::for_test_root(root)
            .expect("bind test qualification to root identity")
    }

    #[test]
    fn production_filesystem_policy_is_longest_mount_and_fail_closed() {
        let root = Path::new("/managed/session");
        let ext4 = FilesystemObservation::for_test("/", "ext4", false, false);
        let nested_ext4 = FilesystemObservation::for_test("/managed", "ext4", false, false);
        let nested_apfs = FilesystemObservation::for_test_exact_mount(
            "/managed",
            "apfs",
            Some(1),
            Some(1),
            false,
            false,
        );

        assert_eq!(
            assess_filesystem_policy(
                root,
                CanonicalPlatform::Linux,
                &[ext4.clone(), nested_ext4.clone()],
            ),
            Ok(QualifiedFilesystemMount {
                mount_point: PathBuf::from("/managed"),
                class: CanonicalFilesystemClass::Ext4,
                live_mount: None,
            })
        );
        assert_eq!(
            select_exact_macos_mount(&LiveMountIdentity::Synthetic(1), &[nested_apfs]),
            Ok(QualifiedFilesystemMount {
                mount_point: PathBuf::from("/managed"),
                class: CanonicalFilesystemClass::Apfs,
                live_mount: Some(LiveMountIdentity::Synthetic(1)),
            })
        );

        for (label, platform, observations, expected) in [
            (
                "windows",
                CanonicalPlatform::Windows,
                vec![nested_ext4.clone()],
                CanonicalFilesystemQualificationError::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Windows,
                },
            ),
            (
                "other-platform",
                CanonicalPlatform::Other,
                vec![nested_ext4.clone()],
                CanonicalFilesystemQualificationError::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Other,
                },
            ),
            (
                "no-mount",
                CanonicalPlatform::Linux,
                Vec::new(),
                CanonicalFilesystemQualificationError::NoMatchingMount,
            ),
            (
                "apfs-on-linux",
                CanonicalPlatform::Linux,
                vec![FilesystemObservation::for_test(
                    "/managed", "apfs", false, false,
                )],
                CanonicalFilesystemQualificationError::FilesystemUnsupported {
                    platform: CanonicalPlatform::Linux,
                    class: CanonicalFilesystemClass::Apfs,
                },
            ),
            (
                "remote",
                CanonicalPlatform::Linux,
                vec![FilesystemObservation::for_test(
                    "/managed", "nfs4", false, false,
                )],
                CanonicalFilesystemQualificationError::FilesystemUnsupported {
                    platform: CanonicalPlatform::Linux,
                    class: CanonicalFilesystemClass::Remote,
                },
            ),
            (
                "fuse",
                CanonicalPlatform::Linux,
                vec![FilesystemObservation::for_test(
                    "/managed",
                    "fuse.portal",
                    false,
                    false,
                )],
                CanonicalFilesystemQualificationError::FilesystemUnsupported {
                    platform: CanonicalPlatform::Linux,
                    class: CanonicalFilesystemClass::Fuse,
                },
            ),
            (
                "temporary",
                CanonicalPlatform::Linux,
                vec![FilesystemObservation::for_test(
                    "/managed", "tmpfs", false, false,
                )],
                CanonicalFilesystemQualificationError::FilesystemUnsupported {
                    platform: CanonicalPlatform::Linux,
                    class: CanonicalFilesystemClass::Temporary,
                },
            ),
            (
                "read-only",
                CanonicalPlatform::Linux,
                vec![FilesystemObservation::for_test(
                    "/managed", "ext4", false, true,
                )],
                CanonicalFilesystemQualificationError::ReadOnlyFilesystem {
                    class: CanonicalFilesystemClass::Ext4,
                },
            ),
            (
                "removable",
                CanonicalPlatform::Linux,
                vec![FilesystemObservation::for_test(
                    "/managed", "ext4", true, false,
                )],
                CanonicalFilesystemQualificationError::RemovableFilesystem {
                    class: CanonicalFilesystemClass::Ext4,
                },
            ),
        ] {
            assert_eq!(
                assess_filesystem_policy(root, platform, &observations),
                Err(expected),
                "policy case {label}",
            );
        }
    }

    #[test]
    fn production_qualification_errors_and_debug_are_content_free() {
        let sensitive_root = Path::new("/managed/private-user-root");
        let error = assess_filesystem_policy(
            sensitive_root,
            CanonicalPlatform::Linux,
            &[FilesystemObservation::for_test(
                "/managed",
                "private-remote-filesystem-source",
                false,
                false,
            )],
        )
        .expect_err("unknown filesystem must refuse");
        let rendered = format!("{error:?}");
        for sensitive in [
            "private-user-root",
            "private-remote-filesystem-source",
            "/managed",
        ] {
            assert!(!rendered.contains(sensitive));
        }
        assert_eq!(
            error,
            CanonicalFilesystemQualificationError::FilesystemUnsupported {
                platform: CanonicalPlatform::Linux,
                class: CanonicalFilesystemClass::Other,
            }
        );

        let unavailable_identity = FilesystemIdentity {
            volume: None,
            object: None,
        };
        assert_eq!(
            validate_mount_volume(
                &QualifiedFilesystemMount {
                    mount_point: PathBuf::from("/"),
                    class: CanonicalFilesystemClass::Ext4,
                    live_mount: None,
                },
                &unavailable_identity,
            ),
            Err(CanonicalFilesystemQualificationError::IdentityUnavailable)
        );

        #[cfg(target_os = "linux")]
        {
            let root = temp_root("private-debug-root");
            fs::create_dir_all(&root).expect("create qualification debug fixture");
            let (qualification, _) =
                CanonicalFilesystemQualification::for_existing_managed_root(&root)
                    .expect("qualify live ext4 debug fixture");
            assert_eq!(
                format!("{qualification:?}"),
                "CanonicalFilesystemQualification([BOUND])"
            );
            assert!(!format!("{qualification:?}").contains("private-debug-root"));
            fs::remove_dir_all(root).expect("clean qualification debug fixture");
        }
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn macos_volume_group_selection_binds_logical_root_to_unique_data_volume() {
        let root = Path::new("/Users/example/Library/Application Support/AudioGraph");
        let root_mount = LiveMountIdentity::Synthetic(42);
        let system = FilesystemObservation::for_test_exact_mount(
            "/",
            "apfs",
            Some(42),
            Some(7),
            false,
            true,
        );
        let data = FilesystemObservation::for_test_exact_mount(
            "/System/Volumes/Data",
            "apfs",
            Some(42),
            Some(42),
            false,
            false,
        );

        let qualified = QualifiedFilesystemMount {
            mount_point: PathBuf::from("/System/Volumes/Data"),
            class: CanonicalFilesystemClass::Apfs,
            live_mount: Some(LiveMountIdentity::Synthetic(42)),
        };
        assert_eq!(system.volume, data.volume);
        assert_eq!(
            select_exact_macos_mount(&root_mount, &[system.clone(), data.clone()]),
            Ok(qualified)
        );
        assert!(root.starts_with(&system.mount_point));
        assert!(!root.starts_with(&data.mount_point));

        let read_only_data = FilesystemObservation {
            read_only: true,
            ..data.clone()
        };
        assert_eq!(
            select_exact_macos_mount(&root_mount, &[system.clone(), read_only_data]),
            Err(CanonicalFilesystemQualificationError::ReadOnlyFilesystem {
                class: CanonicalFilesystemClass::Apfs,
            })
        );

        let removable_data = FilesystemObservation {
            removable: true,
            ..data.clone()
        };
        assert_eq!(
            select_exact_macos_mount(&root_mount, &[system.clone(), removable_data]),
            Err(CanonicalFilesystemQualificationError::RemovableFilesystem {
                class: CanonicalFilesystemClass::Apfs,
            })
        );

        let non_apfs_data = FilesystemObservation {
            file_system: OsString::from("hfs"),
            ..data.clone()
        };
        assert_eq!(
            select_exact_macos_mount(&root_mount, &[system.clone(), non_apfs_data]),
            Err(
                CanonicalFilesystemQualificationError::FilesystemUnsupported {
                    platform: CanonicalPlatform::MacOs,
                    class: CanonicalFilesystemClass::Other,
                }
            )
        );

        let foreign_data = FilesystemObservation::for_test_exact_mount(
            "/System/Volumes/Data",
            "apfs",
            Some(42),
            Some(99),
            false,
            false,
        );
        assert_eq!(
            select_exact_macos_mount(&root_mount, &[system.clone(), foreign_data]),
            Err(CanonicalFilesystemQualificationError::IdentityUnavailable)
        );

        let unavailable_system = FilesystemObservation {
            live_mount: None,
            ..system.clone()
        };
        assert_eq!(
            select_exact_macos_mount(&root_mount, &[unavailable_system, data.clone()]),
            Err(CanonicalFilesystemQualificationError::IdentityUnavailable)
        );

        let duplicate_data = FilesystemObservation::for_test_exact_mount(
            "/Volumes/DataAlias",
            "apfs",
            Some(42),
            Some(42),
            false,
            false,
        );
        assert_eq!(
            select_exact_macos_mount(&root_mount, &[system, data, duplicate_data]),
            Err(CanonicalFilesystemQualificationError::IdentityUnavailable)
        );

        assert_eq!(require_stable_live_mount(&root_mount, &root_mount), Ok(()));
        assert_eq!(
            require_stable_live_mount(&root_mount, &LiveMountIdentity::Synthetic(43)),
            Err(CanonicalFilesystemQualificationError::IdentityUnavailable)
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn live_linux_existing_root_qualification_is_ext4_or_typed_refusal() {
        let root = temp_root("live-production-qualification");
        fs::create_dir_all(&root).expect("create live qualification root");
        let lock_path = root.join(COORDINATION_FILE_NAME);

        match CanonicalFilesystemQualification::for_existing_managed_root(&root) {
            Ok((qualification, durability)) => {
                let guard = durability
                    .try_lock_exclusive_qualified(&root, &qualification)
                    .expect("qualified live ext4 root acquires guard");
                eprintln!("live Linux ext4 qualification admitted");
                let target = root.join("first-created.bin");
                assert_eq!(
                    guard.append(
                        &target,
                        b"qualified bytes",
                        Some(&qualification),
                        CanonicalRecoveryKey::from_opaque_bytes([91; 16]),
                    ),
                    CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                        mutation: CanonicalMutation::FirstCreate,
                        barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
                    })
                );
                assert!(lock_path.exists());
                drop(guard);
            }
            Err(error @ CanonicalFilesystemQualificationError::FilesystemUnsupported { .. })
            | Err(error @ CanonicalFilesystemQualificationError::NoMatchingMount)
            | Err(error @ CanonicalFilesystemQualificationError::ReadOnlyFilesystem { .. })
            | Err(error @ CanonicalFilesystemQualificationError::RemovableFilesystem { .. }) => {
                assert!(
                    !lock_path.exists(),
                    "typed refusal must not create lock state"
                );
                eprintln!("live Linux qualification explicitly refused: {error:?}");
            }
            Err(other) => panic!("unexpected live Linux qualification result: {other:?}"),
        }

        fs::remove_dir_all(root).expect("clean live qualification root");
    }

    #[test]
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn cc9a_native_qualified_guard_refuses_recreated_root_before_coordination_mutation() {
        let first_root = temp_root("production-binding-first");
        let second_root = temp_root("production-binding-second");
        let ancestor_root = temp_root("production-binding-ancestor");
        let nested_root = ancestor_root.join("nested");
        let moved_root = temp_root("production-binding-moved");
        let displaced_root = temp_root("production-binding-displaced");
        for root in [&first_root, &second_root, &nested_root, &moved_root] {
            fs::create_dir_all(root).expect("create production binding fixture");
        }
        #[cfg(target_os = "macos")]
        emit_cc9a_macos_mount_diagnostics(&first_root);
        let assert_binding_refusal =
            |result: Result<CanonicalExclusiveGuard, CanonicalCoordinationError>| match result {
                Err(actual) => assert_eq!(
                    actual,
                    CanonicalCoordinationError::QualificationBindingMismatch
                ),
                Ok(_) => panic!("mismatched qualification unexpectedly acquired guard"),
            };

        let (first_qualification, first_durability) =
            CanonicalFilesystemQualification::for_existing_managed_root(&first_root)
                .expect("qualify first root");
        assert_binding_refusal(
            first_durability.try_lock_exclusive_qualified(&second_root, &first_qualification),
        );
        assert!(!second_root.join(COORDINATION_FILE_NAME).exists());

        let (foreign_qualification, _) =
            CanonicalFilesystemQualification::for_existing_managed_root(&first_root)
                .expect("create foreign token for same root");
        assert_binding_refusal(
            first_durability.try_lock_exclusive_qualified(&first_root, &foreign_qualification),
        );
        assert!(!first_root.join(COORDINATION_FILE_NAME).exists());

        let (ancestor_qualification, ancestor_durability) =
            CanonicalFilesystemQualification::for_existing_managed_root(&ancestor_root)
                .expect("qualify ancestor root");
        assert_binding_refusal(
            ancestor_durability.try_lock_exclusive_qualified(&nested_root, &ancestor_qualification),
        );
        assert!(!nested_root.join(COORDINATION_FILE_NAME).exists());

        let (nested_qualification, nested_durability) =
            CanonicalFilesystemQualification::for_existing_managed_root(&nested_root)
                .expect("qualify nested root");
        assert_binding_refusal(
            nested_durability.try_lock_exclusive_qualified(&ancestor_root, &nested_qualification),
        );
        assert!(!ancestor_root.join(COORDINATION_FILE_NAME).exists());

        let (moved_qualification, moved_durability) =
            CanonicalFilesystemQualification::for_existing_managed_root(&moved_root)
                .expect("qualify root before replacement");
        fs::rename(&moved_root, &displaced_root).expect("displace qualified root");
        fs::create_dir_all(&moved_root).expect("recreate qualified pathname");
        assert_binding_refusal(
            moved_durability.try_lock_exclusive_qualified(&moved_root, &moved_qualification),
        );
        assert!(!moved_root.join(COORDINATION_FILE_NAME).exists());

        let exact_guard = first_durability
            .try_lock_exclusive_qualified(&first_root, &first_qualification)
            .expect("exact qualification pair acquires guard");
        assert!(first_root.join(COORDINATION_FILE_NAME).exists());
        drop(exact_guard);

        for root in [
            first_root,
            second_root,
            ancestor_root,
            moved_root,
            displaced_root,
        ] {
            fs::remove_dir_all(root).expect("clean production binding fixture");
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn public_append_classifies_existing_and_first_created_files_atomically() {
        let root = temp_root("classification");
        fs::create_dir_all(&root).expect("create fixture root");
        let existing = root.join("existing.log");
        let newly_created = root.join("new.log");
        fs::write(&existing, b"prefix").expect("seed existing file");
        let proof = qualification(&root);
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire namespace-bound guard");
        let recovery_key = CanonicalRecoveryKey::from_opaque_bytes([7; 16]);

        assert_eq!(
            guard.append(&existing, b"-append", None, recovery_key),
            CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::ExistingAppend,
                barrier: CanonicalDurabilityBarrier::FileDataAndMetadata,
            })
        );
        assert_eq!(
            guard.append(&newly_created, b"first", Some(&proof), recovery_key),
            CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::FirstCreate,
                barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
            })
        );
        assert_eq!(fs::read(existing).expect("read existing"), b"prefix-append");
        assert_eq!(fs::read(newly_created).expect("read new"), b"first");
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn immutable_exact_create_recovers_a_true_partial_final_after_restart() {
        let root = temp_root("immutable-exact-partial-restart");
        fs::create_dir_all(&root).expect("create fixture root");
        let proof = root.join("session-proof.json");
        let expected = b"one complete immutable proof record";
        let qualification = qualification(&root);
        let recovery_key = CanonicalRecoveryKey::from_opaque_bytes([0x39; 16]);

        let first_guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire first namespace guard");
        assert!(matches!(
            first_guard.create_or_reconcile_immutable_exact_with_fault(
                &proof,
                expected,
                Some(&qualification),
                recovery_key,
                CanonicalDurabilityStage::Write,
            ),
            CanonicalDurabilityOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: CanonicalDurabilityStage::Write,
                ..
            })
        ));
        let partial = fs::read(&proof).expect("read real partial final proof");
        assert!(!partial.is_empty());
        assert!(partial.len() < expected.len());
        assert!(expected.starts_with(&partial));
        drop(first_guard);

        let retry_guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire restarted namespace guard");
        assert!(matches!(
            retry_guard.create_or_reconcile_immutable_exact(
                &proof,
                expected,
                Some(&qualification),
                recovery_key,
            ),
            CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::ImmutableExactReconcile,
                barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
            })
        ));
        assert_eq!(fs::read(&proof).expect("read reconciled proof"), expected);

        drop(retry_guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn immutable_exact_create_reconciles_without_append_and_refuses_collisions() {
        use std::os::unix::fs::symlink;
        use std::process::Command;

        let root = temp_root("immutable-exact-collisions");
        fs::create_dir_all(&root).expect("create fixture root");
        let qualification = qualification(&root);
        let key = CanonicalRecoveryKey::from_opaque_bytes([0x3a; 16]);
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire namespace guard");
        let exact = root.join("proof-exact.json");
        let expected = b"exact immutable bytes";

        assert_eq!(
            guard.create_or_reconcile_immutable_exact(&exact, expected, Some(&qualification), key,),
            CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::ImmutableExactCreate,
                barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
            })
        );
        for _ in 0..2 {
            assert_eq!(
                guard.create_or_reconcile_immutable_exact(
                    &exact,
                    expected,
                    Some(&qualification),
                    key,
                ),
                CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                    mutation: CanonicalMutation::ImmutableExactReconcile,
                    barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
                })
            );
            assert_eq!(fs::read(&exact).expect("read exact proof"), expected);
        }

        let mismatched = root.join("proof-mismatch.json");
        fs::write(&mismatched, b"not a prefix").expect("seed mismatched regular file");
        let longer = root.join("proof-longer.json");
        fs::write(&longer, [expected.as_slice(), b"-extra"].concat())
            .expect("seed longer regular file");
        let directory = root.join("proof-directory.json");
        fs::create_dir(&directory).expect("seed directory collision");
        let dangling = root.join("proof-symlink.json");
        symlink(root.join("missing-proof"), &dangling).expect("seed symlink collision");
        let fifo = root.join("proof-fifo.json");
        let status = Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("run mkfifo");
        assert!(status.success());
        let alias = root.join("PROOF-ALIAS.JSON");
        fs::write(&alias, expected).expect("seed case alias");

        for path in [&mismatched, &longer] {
            let before = fs::read(path).expect("read regular collision");
            assert_eq!(
                guard.create_or_reconcile_immutable_exact(
                    path,
                    expected,
                    Some(&qualification),
                    key,
                ),
                CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::ImmutableExactConflict
                )
            );
            assert_eq!(fs::read(path).expect("collision remains unchanged"), before);
        }
        for path in [&directory, &dangling, &fifo] {
            assert_eq!(
                guard.create_or_reconcile_immutable_exact(
                    path,
                    expected,
                    Some(&qualification),
                    key,
                ),
                CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::NonRegularCanonicalEntry
                )
            );
        }
        let requested_alias = root.join("proof-alias.json");
        assert_eq!(
            guard.create_or_reconcile_immutable_exact(
                &requested_alias,
                expected,
                Some(&qualification),
                key,
            ),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::ImmutableExactConflict
            )
        );
        assert!(!requested_alias.exists());
        assert_eq!(fs::read(alias).expect("alias remains unchanged"), expected);

        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn immutable_exact_create_fault_cuts_are_honest_and_restart_converges() {
        let expected = b"one complete proof record";
        let key = CanonicalRecoveryKey::from_opaque_bytes([0x3b; 16]);

        let create_root = temp_root("immutable-exact-fault-CreateNew");
        fs::create_dir_all(&create_root).expect("create fault root");
        let create_path = create_root.join("proof.json");
        let create_qualification = qualification(&create_root);
        let create_guard = CanonicalDurability::new()
            .try_lock_exclusive(&create_root)
            .expect("acquire create fault guard");
        assert!(matches!(
            create_guard.create_or_reconcile_immutable_exact_with_fault(
                &create_path,
                expected,
                Some(&create_qualification),
                key,
                CanonicalDurabilityStage::CreateNew,
            ),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::IoFailedBeforeMutation {
                    stage: CanonicalDurabilityStage::CreateNew,
                    ..
                }
            )
        ));
        assert!(!create_path.exists());
        drop(create_guard);
        fs::remove_dir_all(create_root).expect("clean create fault root");

        for stage in [
            CanonicalDurabilityStage::Write,
            CanonicalDurabilityStage::Flush,
            CanonicalDurabilityStage::ProtectTemp,
            CanonicalDurabilityStage::FileSync,
            CanonicalDurabilityStage::ParentSync,
        ] {
            let root = temp_root(&format!("immutable-exact-fault-{stage:?}"));
            fs::create_dir_all(&root).expect("create fault root");
            let path = root.join("proof.json");
            let qualification = qualification(&root);
            let first_guard = CanonicalDurability::new()
                .try_lock_exclusive(&root)
                .expect("acquire fault guard");
            assert!(matches!(
                first_guard.create_or_reconcile_immutable_exact_with_fault(
                    &path,
                    expected,
                    Some(&qualification),
                    key,
                    stage,
                ),
                CanonicalDurabilityOutcome::DurabilityIndeterminate(
                    CanonicalDurabilityIndeterminate {
                        stage: actual_stage,
                        ..
                    }
                ) if actual_stage == stage
            ));
            let observed = fs::read(&path).expect("reopen indeterminate proof");
            assert!(expected.starts_with(&observed));
            assert!(!observed.is_empty());
            drop(first_guard);

            let retry_guard = CanonicalDurability::new()
                .try_lock_exclusive(&root)
                .expect("acquire restart guard");
            assert!(matches!(
                retry_guard.create_or_reconcile_immutable_exact(
                    &path,
                    expected,
                    Some(&qualification),
                    key,
                ),
                CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                    mutation: CanonicalMutation::ImmutableExactReconcile,
                    barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
                })
            ));
            assert_eq!(fs::read(&path).expect("read recovered proof"), expected);
            drop(retry_guard);
            fs::remove_dir_all(root).expect("clean fault root");
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn concurrent_creator_is_atomically_classified_as_existing() {
        let root = temp_root("concurrent-create");
        fs::create_dir_all(&root).expect("create fixture root");
        let proof = Arc::new(qualification(&root));
        let barrier = Arc::new(Barrier::new(2));
        let durability = CanonicalDurability::with_before_atomic_create(barrier.clone());
        let guard = Arc::new(
            durability
                .try_lock_exclusive(&root)
                .expect("acquire namespace-bound guard"),
        );
        let target = root.join("events.log");
        let worker_target = target.clone();
        let worker_guard = guard.clone();
        let worker_proof = proof.clone();
        let worker = thread::spawn(move || {
            worker_guard.append(
                &worker_target,
                b"-append",
                Some(worker_proof.as_ref()),
                CanonicalRecoveryKey::from_opaque_bytes([12; 16]),
            )
        });

        barrier.wait();
        fs::write(&target, b"racer").expect("concurrent creator wins create race");
        barrier.wait();
        let outcome = worker.join().expect("join append worker");

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::ExistingAppend,
                barrier: CanonicalDurabilityBarrier::FileDataAndMetadata,
            })
        );
        assert_eq!(
            fs::read(&target).expect("read raced target"),
            b"racer-append"
        );
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn public_same_directory_rename_syncs_the_file_and_parent_namespace() {
        let root = temp_root("rename");
        fs::create_dir_all(&root).expect("create fixture directory");
        let source = root.join("tail.tmp");
        let destination = root.join("tail.quarantine");
        fs::write(&source, b"recoverable tail").expect("seed rename source");
        let proof = qualification(&root);
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire namespace-bound guard");

        let outcome = guard.rename(
            &source,
            &destination,
            Some(&proof),
            CanonicalRecoveryKey::from_opaque_bytes([9; 16]),
        );

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::Rename,
                barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
            })
        );
        assert!(!source.exists());
        assert_eq!(
            fs::read(destination).expect("read renamed"),
            b"recoverable tail"
        );
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn public_snapshot_install_creates_an_owner_only_durable_head() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("snapshot-initial-install");
        fs::create_dir_all(&root).expect("create fixture root");
        let temporary = root.join("manifest.snapshot.tmp");
        let destination = root.join("manifest.snapshot.json");
        let proof = qualification(&root);
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire namespace-bound guard");

        let outcome = guard.install_snapshot(
            &temporary,
            &destination,
            b"{\"generation\":1}",
            CanonicalSnapshotExpectation::Absent,
            Some(&proof),
            CanonicalRecoveryKey::from_opaque_bytes([23; 16]),
        );

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::InitialSnapshotInstall,
                barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
            })
        );
        assert!(!temporary.exists());
        assert_eq!(
            fs::read(&destination).expect("read installed snapshot"),
            b"{\"generation\":1}"
        );
        assert_eq!(
            fs::metadata(&destination)
                .expect("read installed metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn public_snapshot_install_replaces_only_the_open_expected_head() {
        use std::io::Read;

        let root = temp_root("snapshot-replace-existing");
        fs::create_dir_all(&root).expect("create fixture root");
        let temporary = root.join("manifest.snapshot.tmp");
        let destination = root.join("manifest.snapshot.json");
        let old_bytes = b"{\"generation\":1}";
        let new_bytes = b"{\"generation\":2}";
        fs::write(&destination, old_bytes).expect("seed old snapshot");
        let mut expected_head = File::open(&destination).expect("open validated old head");
        let mut validated_generation = String::new();
        expected_head
            .read_to_string(&mut validated_generation)
            .expect("validate old generation through expected handle");
        assert_eq!(validated_generation.as_bytes(), old_bytes);
        let proof = qualification(&root);
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire namespace-bound guard");

        let outcome = guard.install_snapshot(
            &temporary,
            &destination,
            new_bytes,
            CanonicalSnapshotExpectation::Existing(&expected_head),
            Some(&proof),
            CanonicalRecoveryKey::from_opaque_bytes([24; 16]),
        );

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::SnapshotReplacement,
                barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
            })
        );
        assert!(!temporary.exists());
        assert_eq!(
            fs::read(&destination).expect("reopen installed head"),
            new_bytes
        );
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn snapshot_temp_collisions_refuse_regular_symlink_directory_and_fifo_entries() {
        use std::os::unix::fs::symlink;
        use std::process::Command;

        for entry_kind in ["regular", "symlink", "directory", "fifo", "socket"] {
            let root = temp_root(&format!("snapshot-temp-collision-{entry_kind}"));
            fs::create_dir_all(&root).expect("create fixture root");
            let temporary = root.join("manifest.snapshot.tmp");
            let destination = root.join("manifest.snapshot.json");
            match entry_kind {
                "regular" => fs::write(&temporary, b"competitor").expect("seed regular temp"),
                "symlink" => symlink(root.join("missing"), &temporary).expect("seed temp symlink"),
                "directory" => fs::create_dir(&temporary).expect("seed temp directory"),
                "fifo" => {
                    let status = Command::new("mkfifo")
                        .arg(&temporary)
                        .status()
                        .expect("run mkfifo");
                    assert!(status.success());
                }
                "socket" => {
                    std::os::unix::net::UnixListener::bind(&temporary).expect("seed temp socket");
                }
                _ => unreachable!(),
            }
            let proof = qualification(&root);
            let guard = CanonicalDurability::new()
                .try_lock_exclusive(&root)
                .expect("acquire namespace-bound guard");

            assert_eq!(
                guard.install_snapshot(
                    &temporary,
                    &destination,
                    b"must not publish",
                    CanonicalSnapshotExpectation::Absent,
                    Some(&proof),
                    CanonicalRecoveryKey::from_opaque_bytes([25; 16]),
                ),
                CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::SnapshotTempAlreadyExists
                )
            );
            assert!(fs::symlink_metadata(&temporary).is_ok());
            assert!(!destination.exists());
            drop(guard);
            fs::remove_dir_all(root).expect("clean fixture root");
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn snapshot_destination_collisions_refuse_regular_symlink_directory_and_fifo_entries() {
        use std::os::unix::fs::symlink;
        use std::process::Command;

        for entry_kind in ["regular", "symlink", "directory", "fifo", "socket"] {
            let root = temp_root(&format!("snapshot-destination-collision-{entry_kind}"));
            fs::create_dir_all(&root).expect("create fixture root");
            let temporary = root.join("manifest.snapshot.tmp");
            let destination = root.join("manifest.snapshot.json");
            match entry_kind {
                "regular" => {
                    fs::write(&destination, b"existing").expect("seed regular destination")
                }
                "symlink" => {
                    symlink(root.join("missing"), &destination).expect("seed destination symlink")
                }
                "directory" => fs::create_dir(&destination).expect("seed destination directory"),
                "fifo" => {
                    let status = Command::new("mkfifo")
                        .arg(&destination)
                        .status()
                        .expect("run mkfifo");
                    assert!(status.success());
                }
                "socket" => {
                    std::os::unix::net::UnixListener::bind(&destination)
                        .expect("seed destination socket");
                }
                _ => unreachable!(),
            }
            let proof = qualification(&root);
            let guard = CanonicalDurability::new()
                .try_lock_exclusive(&root)
                .expect("acquire namespace-bound guard");

            assert_eq!(
                guard.install_snapshot(
                    &temporary,
                    &destination,
                    b"must not publish",
                    CanonicalSnapshotExpectation::Absent,
                    Some(&proof),
                    CanonicalRecoveryKey::from_opaque_bytes([26; 16]),
                ),
                CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::DestinationAlreadyExists
                )
            );
            assert!(!temporary.exists());
            assert!(fs::symlink_metadata(&destination).is_ok());
            drop(guard);
            fs::remove_dir_all(root).expect("clean fixture root");
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn competing_snapshot_temp_create_is_atomically_refused_without_fallback() {
        let root = temp_root("snapshot-competing-create");
        fs::create_dir_all(&root).expect("create fixture root");
        let temporary = root.join("manifest.snapshot.tmp");
        let destination = root.join("manifest.snapshot.json");
        let proof = Arc::new(qualification(&root));
        let barrier = Arc::new(Barrier::new(2));
        let guard = Arc::new(
            CanonicalDurability::with_before_atomic_create(barrier.clone())
                .try_lock_exclusive(&root)
                .expect("acquire namespace-bound guard"),
        );
        let worker_guard = guard.clone();
        let worker_proof = proof.clone();
        let worker_temporary = temporary.clone();
        let worker_destination = destination.clone();
        let worker = thread::spawn(move || {
            worker_guard.install_snapshot(
                &worker_temporary,
                &worker_destination,
                b"candidate",
                CanonicalSnapshotExpectation::Absent,
                Some(worker_proof.as_ref()),
                CanonicalRecoveryKey::from_opaque_bytes([27; 16]),
            )
        });

        barrier.wait();
        fs::write(&temporary, b"competitor").expect("competitor creates exact temp");
        barrier.wait();
        let outcome = worker.join().expect("join snapshot worker");

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::SnapshotTempAlreadyExists
            )
        );
        assert_eq!(fs::read(&temporary).expect("temp retained"), b"competitor");
        assert!(!destination.exists());
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn snapshot_destination_identity_change_refuses_before_temp_creation() {
        let root = temp_root("snapshot-destination-identity-change");
        fs::create_dir_all(&root).expect("create fixture root");
        let temporary = root.join("manifest.snapshot.tmp");
        let destination = root.join("manifest.snapshot.json");
        let displaced = root.join("manifest.snapshot.displaced");
        fs::write(&destination, b"validated-old").expect("seed validated head");
        let expected_head = File::open(&destination).expect("open validated head");
        let proof = Arc::new(qualification(&root));
        let barrier = Arc::new(Barrier::new(2));
        let guard = Arc::new(
            CanonicalDurability::with_before_existing_open(barrier.clone())
                .try_lock_exclusive(&root)
                .expect("acquire namespace-bound guard"),
        );
        let worker_guard = guard.clone();
        let worker_proof = proof.clone();
        let worker_temporary = temporary.clone();
        let worker_destination = destination.clone();
        let worker = thread::spawn(move || {
            worker_guard.install_snapshot(
                &worker_temporary,
                &worker_destination,
                b"candidate-new",
                CanonicalSnapshotExpectation::Existing(&expected_head),
                Some(worker_proof.as_ref()),
                CanonicalRecoveryKey::from_opaque_bytes([28; 16]),
            )
        });

        barrier.wait();
        fs::rename(&destination, &displaced).expect("displace validated destination");
        fs::write(&destination, b"uncooperative-replacement")
            .expect("replace destination identity");
        barrier.wait();
        let outcome = worker.join().expect("join snapshot worker");

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::Rejected(CanonicalDurabilityRejection::IdentityChanged)
        );
        assert!(!temporary.exists());
        assert_eq!(
            fs::read(&destination).expect("replacement retained"),
            b"uncooperative-replacement"
        );
        assert_eq!(
            fs::read(&displaced).expect("validated head retained"),
            b"validated-old"
        );
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn snapshot_destination_change_after_temp_sync_is_indeterminate_and_preserved() {
        let root = temp_root("snapshot-late-destination-change");
        fs::create_dir_all(&root).expect("create fixture root");
        let temporary = root.join("manifest.snapshot.tmp");
        let destination = root.join("manifest.snapshot.json");
        let displaced = root.join("manifest.snapshot.displaced");
        fs::write(&destination, b"validated-old").expect("seed validated head");
        let expected_head = File::open(&destination).expect("open validated head");
        let proof = Arc::new(qualification(&root));
        let barrier = Arc::new(Barrier::new(2));
        let recovery_key = CanonicalRecoveryKey::from_opaque_bytes([35; 16]);
        let guard = Arc::new(
            CanonicalDurability::with_before_snapshot_revalidation(barrier.clone())
                .try_lock_exclusive(&root)
                .expect("acquire namespace-bound guard"),
        );
        let worker_guard = guard.clone();
        let worker_proof = proof.clone();
        let worker_temporary = temporary.clone();
        let worker_destination = destination.clone();
        let worker = thread::spawn(move || {
            worker_guard.install_snapshot(
                &worker_temporary,
                &worker_destination,
                b"candidate-new",
                CanonicalSnapshotExpectation::Existing(&expected_head),
                Some(worker_proof.as_ref()),
                recovery_key,
            )
        });

        barrier.wait();
        assert_eq!(
            fs::read(&temporary).expect("candidate temp is synchronized"),
            b"candidate-new"
        );
        fs::rename(&destination, &displaced).expect("displace validated destination");
        fs::write(&destination, b"uncooperative-replacement")
            .expect("replace destination identity");
        barrier.wait();
        let outcome = worker.join().expect("join snapshot worker");

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: CanonicalDurabilityStage::InspectEntry,
                kind: io::ErrorKind::Other,
                raw_os_error: None,
                recovery_key,
            })
        );
        assert_eq!(
            fs::read(&temporary).expect("recoverable candidate retained"),
            b"candidate-new"
        );
        assert_eq!(
            fs::read(&destination).expect("replacement retained"),
            b"uncooperative-replacement"
        );
        assert_eq!(
            fs::read(&displaced).expect("validated old head retained"),
            b"validated-old"
        );
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn snapshot_absent_destination_appearance_after_temp_sync_is_preserved() {
        let root = temp_root("snapshot-late-destination-appearance");
        fs::create_dir_all(&root).expect("create fixture root");
        let temporary = root.join("manifest.snapshot.tmp");
        let destination = root.join("manifest.snapshot.json");
        let proof = Arc::new(qualification(&root));
        let barrier = Arc::new(Barrier::new(2));
        let recovery_key = CanonicalRecoveryKey::from_opaque_bytes([37; 16]);
        let guard = Arc::new(
            CanonicalDurability::with_before_snapshot_revalidation(barrier.clone())
                .try_lock_exclusive(&root)
                .expect("acquire namespace-bound guard"),
        );
        let worker_guard = guard.clone();
        let worker_proof = proof.clone();
        let worker_temporary = temporary.clone();
        let worker_destination = destination.clone();
        let worker = thread::spawn(move || {
            worker_guard.install_snapshot(
                &worker_temporary,
                &worker_destination,
                b"candidate-initial-head",
                CanonicalSnapshotExpectation::Absent,
                Some(worker_proof.as_ref()),
                recovery_key,
            )
        });

        barrier.wait();
        assert_eq!(
            fs::read(&temporary).expect("candidate temp is synchronized"),
            b"candidate-initial-head"
        );
        assert!(!destination.exists());
        fs::write(&destination, b"concurrent-complete-head")
            .expect("concurrent writer publishes a complete head");
        barrier.wait();
        let outcome = worker.join().expect("join snapshot worker");

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: CanonicalDurabilityStage::InspectEntry,
                kind: io::ErrorKind::AlreadyExists,
                raw_os_error: None,
                recovery_key,
            })
        );
        assert_eq!(
            fs::read(&destination).expect("concurrent head retained"),
            b"concurrent-complete-head"
        );
        assert_eq!(
            fs::read(&temporary).expect("recoverable candidate retained"),
            b"candidate-initial-head"
        );
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn snapshot_temp_and_destination_aliases_refuse_before_mutation() {
        let root = temp_root("snapshot-path-overlap");
        fs::create_dir_all(&root).expect("create fixture root");
        let proof = qualification(&root);
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire namespace-bound guard");

        for (temporary, destination) in [
            (root.join("manifest.tmp"), root.join("manifest.tmp")),
            (root.join("Manifest.TMP"), root.join("manifest.tmp")),
        ] {
            assert_eq!(
                guard.install_snapshot(
                    &temporary,
                    &destination,
                    b"must not publish",
                    CanonicalSnapshotExpectation::Absent,
                    Some(&proof),
                    CanonicalRecoveryKey::from_opaque_bytes([36; 16]),
                ),
                CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::SnapshotPathOverlap
                )
            );
            assert!(!temporary.exists());
            assert!(!destination.exists());
        }
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn snapshot_refuses_overlapping_and_replaced_managed_roots_before_mutation() {
        let ancestor_root = temp_root("snapshot-overlapping-root");
        let exact_root = ancestor_root.join("nested");
        fs::create_dir_all(&exact_root).expect("create nested root");
        let ancestor_proof = qualification(&ancestor_root);
        let ancestor_guard = CanonicalDurability::new()
            .try_lock_exclusive(&ancestor_root)
            .expect("acquire ancestor guard");
        let nested_temporary = exact_root.join("manifest.tmp");
        let nested_destination = exact_root.join("manifest.json");

        assert_eq!(
            ancestor_guard.install_snapshot(
                &nested_temporary,
                &nested_destination,
                b"must not publish",
                CanonicalSnapshotExpectation::Absent,
                Some(&ancestor_proof),
                CanonicalRecoveryKey::from_opaque_bytes([29; 16]),
            ),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::TargetOutsideManagedNamespace
            )
        );
        assert!(!nested_temporary.exists());
        assert!(!nested_destination.exists());
        drop(ancestor_guard);
        fs::remove_dir_all(&ancestor_root).expect("clean ancestor fixture");

        let root = temp_root("snapshot-replaced-root");
        let displaced = temp_root("snapshot-replaced-root-displaced");
        fs::create_dir_all(&root).expect("create managed root");
        let proof = qualification(&root);
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire exact-root guard");
        fs::rename(&root, &displaced).expect("displace managed root");
        fs::create_dir_all(&root).expect("replace managed root path");
        let temporary = root.join("manifest.tmp");
        let destination = root.join("manifest.json");

        assert_eq!(
            guard.install_snapshot(
                &temporary,
                &destination,
                b"must not publish",
                CanonicalSnapshotExpectation::Absent,
                Some(&proof),
                CanonicalRecoveryKey::from_opaque_bytes([30; 16]),
            ),
            CanonicalDurabilityOutcome::Rejected(CanonicalDurabilityRejection::IdentityChanged)
        );
        assert!(!temporary.exists());
        assert!(!destination.exists());
        drop(guard);
        fs::remove_dir_all(root).expect("clean replacement root");
        fs::remove_dir_all(displaced).expect("clean displaced root");
    }

    #[test]
    fn algorithm_snapshot_temp_revalidation_stays_on_the_guard_identity_seam() {
        let source = include_str!("canonical_durability.rs");
        let install = source
            .split_once("    fn install_snapshot_inner(")
            .expect("snapshot installer source")
            .1
            .split_once("    fn checked_snapshot(")
            .expect("snapshot installer boundary")
            .0;

        assert!(
            install.contains(
                "if self\n            .validate_snapshot_destination(\n                temporary,"
            ),
            "snapshot temp must use the cfg(test)-aware guard identity seam"
        );
        assert!(
            !install.contains("if validate_snapshot_destination(\n            temporary,"),
            "snapshot temp must not bypass the guard identity seam"
        );
    }

    #[test]
    fn snapshot_windows_and_unqualified_paths_refuse_before_temp_or_head_mutation() {
        let root = temp_root("snapshot-unsupported");
        fs::create_dir_all(&root).expect("create fixture root");
        let temporary = root.join("manifest.snapshot.tmp");
        let destination = root.join("manifest.snapshot.json");
        fs::write(&destination, b"old-head").expect("seed existing head");
        let expected_head = File::open(&destination).expect("open expected head");
        let key = CanonicalRecoveryKey::from_opaque_bytes([31; 16]);

        let windows_guard = CanonicalDurability::for_test_platform(CanonicalPlatform::Windows)
            .try_lock_exclusive(&root)
            .expect("acquire simulated Windows guard");
        assert_eq!(
            windows_guard.install_snapshot(
                &temporary,
                &destination,
                b"must not install",
                CanonicalSnapshotExpectation::Existing(&expected_head),
                None,
                key,
            ),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Windows,
                    operation: CanonicalNamespaceOperation::AtomicSnapshotInstall,
                }
            )
        );
        assert!(!temporary.exists());
        assert_eq!(fs::read(&destination).expect("head retained"), b"old-head");
        drop(windows_guard);

        let unqualified_guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire unqualified guard");
        assert_eq!(
            unqualified_guard.install_snapshot(
                &temporary,
                &destination,
                b"must not install",
                CanonicalSnapshotExpectation::Existing(&expected_head),
                None,
                key,
            ),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: current_platform(),
                    operation: CanonicalNamespaceOperation::AtomicSnapshotInstall,
                }
            )
        );
        assert!(!temporary.exists());
        assert_eq!(fs::read(&destination).expect("head retained"), b"old-head");
        drop(unqualified_guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn snapshot_preflight_cross_device_refuses_and_runtime_exdev_is_indeterminate() {
        let preflight_root = temp_root("snapshot-preflight-exdev");
        fs::create_dir_all(&preflight_root).expect("create preflight root");
        let preflight_temp = preflight_root.join("manifest.tmp");
        let preflight_destination = preflight_root.join("manifest.json");
        let preflight_proof = qualification(&preflight_root);
        let preflight_invoked = Arc::new(AtomicBool::new(false));
        let preflight_guard = CanonicalDurability::with_rename_fault(
            InjectedRenameFault::PreflightDeviceMismatch,
            preflight_invoked.clone(),
        )
        .try_lock_exclusive(&preflight_root)
        .expect("acquire preflight guard");

        assert_eq!(
            preflight_guard.install_snapshot(
                &preflight_temp,
                &preflight_destination,
                b"candidate",
                CanonicalSnapshotExpectation::Absent,
                Some(&preflight_proof),
                CanonicalRecoveryKey::from_opaque_bytes([32; 16]),
            ),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::CrossDeviceRenameRefused { raw_os_error: None }
            )
        );
        assert!(!preflight_invoked.load(Ordering::SeqCst));
        assert!(!preflight_temp.exists());
        assert!(!preflight_destination.exists());
        drop(preflight_guard);
        fs::remove_dir_all(preflight_root).expect("clean preflight root");

        let runtime_root = temp_root("snapshot-runtime-exdev");
        fs::create_dir_all(&runtime_root).expect("create runtime root");
        let runtime_temp = runtime_root.join("manifest.tmp");
        let runtime_destination = runtime_root.join("manifest.json");
        fs::write(&runtime_destination, b"old-head").expect("seed old head");
        let expected_head = File::open(&runtime_destination).expect("open old head");
        let runtime_proof = qualification(&runtime_root);
        let runtime_invoked = Arc::new(AtomicBool::new(false));
        let recovery_key = CanonicalRecoveryKey::from_opaque_bytes([33; 16]);
        let runtime_guard = CanonicalDurability::with_rename_fault(
            InjectedRenameFault::RuntimeExdev { raw_os_error: 18 },
            runtime_invoked.clone(),
        )
        .try_lock_exclusive(&runtime_root)
        .expect("acquire runtime guard");

        assert_eq!(
            runtime_guard.install_snapshot(
                &runtime_temp,
                &runtime_destination,
                b"candidate-new",
                CanonicalSnapshotExpectation::Existing(&expected_head),
                Some(&runtime_proof),
                recovery_key,
            ),
            CanonicalDurabilityOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: CanonicalDurabilityStage::Rename,
                kind: io::ErrorKind::CrossesDevices,
                raw_os_error: Some(18),
                recovery_key,
            })
        );
        assert!(runtime_invoked.load(Ordering::SeqCst));
        assert_eq!(
            fs::read(&runtime_destination).expect("old head retained"),
            b"old-head"
        );
        assert_eq!(
            fs::read(&runtime_temp).expect("recoverable temp retained"),
            b"candidate-new"
        );
        drop(runtime_guard);
        fs::remove_dir_all(runtime_root).expect("clean runtime root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn snapshot_fault_cuts_reopen_to_only_old_or_new_complete_bytes() {
        let old_bytes = b"{\"generation\":7,\"state\":\"old\"}";
        let new_bytes = b"{\"generation\":8,\"state\":\"new\"}";

        for stage in [
            CanonicalDurabilityStage::CreateNew,
            CanonicalDurabilityStage::Write,
            CanonicalDurabilityStage::Flush,
            CanonicalDurabilityStage::ProtectTemp,
            CanonicalDurabilityStage::FileSync,
            CanonicalDurabilityStage::Rename,
            CanonicalDurabilityStage::ParentSync,
        ] {
            let root = temp_root(&format!("snapshot-fault-{stage:?}"));
            fs::create_dir_all(&root).expect("create fixture root");
            let temporary = root.join("manifest.snapshot.tmp");
            let destination = root.join("manifest.snapshot.json");
            fs::write(&destination, old_bytes).expect("seed old head");
            let expected_head = File::open(&destination).expect("open expected head");
            let proof = qualification(&root);
            let recovery_key = CanonicalRecoveryKey::from_opaque_bytes([34; 16]);
            let guard = CanonicalDurability::failing_at(stage)
                .try_lock_exclusive(&root)
                .expect("acquire fault guard");

            let outcome = guard.install_snapshot(
                &temporary,
                &destination,
                new_bytes,
                CanonicalSnapshotExpectation::Existing(&expected_head),
                Some(&proof),
                recovery_key,
            );

            if stage == CanonicalDurabilityStage::CreateNew {
                assert_eq!(
                    outcome,
                    CanonicalDurabilityOutcome::Rejected(
                        CanonicalDurabilityRejection::IoFailedBeforeMutation {
                            stage,
                            kind: io::ErrorKind::Other,
                            raw_os_error: None,
                        }
                    )
                );
            } else {
                assert_eq!(
                    outcome,
                    CanonicalDurabilityOutcome::DurabilityIndeterminate(
                        CanonicalDurabilityIndeterminate {
                            stage,
                            kind: io::ErrorKind::Other,
                            raw_os_error: None,
                            recovery_key,
                        }
                    )
                );
            }
            drop(guard);

            let reopened = fs::read(&destination).expect("reopen installed head");
            if stage == CanonicalDurabilityStage::ParentSync {
                assert_eq!(reopened, new_bytes, "post-rename head must be complete");
                assert!(!temporary.exists());
            } else {
                assert_eq!(reopened, old_bytes, "pre-rename head must stay complete");
                assert_eq!(
                    temporary.exists(),
                    stage != CanonicalDurabilityStage::CreateNew,
                    "only a successful create may leave a recoverable temp"
                );
            }
            fs::remove_dir_all(root).expect("clean fixture root");
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn initial_snapshot_pre_and_post_rename_faults_reopen_to_absent_or_complete_new_head() {
        let new_bytes = b"{\"generation\":1,\"state\":\"complete\"}";

        for stage in [
            CanonicalDurabilityStage::Rename,
            CanonicalDurabilityStage::ParentSync,
        ] {
            let root = temp_root(&format!("initial-snapshot-fault-{stage:?}"));
            fs::create_dir_all(&root).expect("create fixture root");
            let temporary = root.join("manifest.snapshot.tmp");
            let destination = root.join("manifest.snapshot.json");
            let proof = qualification(&root);
            let recovery_key = CanonicalRecoveryKey::from_opaque_bytes([38; 16]);
            let guard = CanonicalDurability::failing_at(stage)
                .try_lock_exclusive(&root)
                .expect("acquire fault guard");

            assert_eq!(
                guard.install_snapshot(
                    &temporary,
                    &destination,
                    new_bytes,
                    CanonicalSnapshotExpectation::Absent,
                    Some(&proof),
                    recovery_key,
                ),
                CanonicalDurabilityOutcome::DurabilityIndeterminate(
                    CanonicalDurabilityIndeterminate {
                        stage,
                        kind: io::ErrorKind::Other,
                        raw_os_error: None,
                        recovery_key,
                    }
                )
            );
            drop(guard);

            if stage == CanonicalDurabilityStage::Rename {
                assert!(matches!(
                    fs::read(&destination),
                    Err(error) if error.kind() == io::ErrorKind::NotFound
                ));
                assert_eq!(
                    fs::read(&temporary).expect("recoverable pre-rename temp retained"),
                    new_bytes
                );
            } else {
                assert_eq!(
                    fs::read(&destination).expect("complete post-rename head retained"),
                    new_bytes
                );
                assert!(!temporary.exists());
            }
            fs::remove_dir_all(root).expect("clean fixture root");
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn snapshot_indeterminate_diagnostics_are_content_free() {
        let root = temp_root("snapshot-redacted-diagnostics");
        fs::create_dir_all(&root).expect("create fixture root");
        let temporary = root.join("private-session-manifest.tmp");
        let destination = root.join("private-session-manifest.json");
        fs::write(&destination, b"old").expect("seed old head");
        let expected_head = File::open(&destination).expect("open old head");
        let proof = qualification(&root);
        let payload = b"sensitive inferred user-world detail";
        let guard = CanonicalDurability::failing_at_with_raw_os_error(
            CanonicalDurabilityStage::FileSync,
            28,
        )
        .try_lock_exclusive(&root)
        .expect("acquire fault guard");

        let outcome = guard.install_snapshot(
            &temporary,
            &destination,
            payload,
            CanonicalSnapshotExpectation::Existing(&expected_head),
            Some(&proof),
            CanonicalRecoveryKey::from_opaque_bytes([0xcd; 16]),
        );

        let diagnostic = format!("{outcome:?}");
        assert!(!diagnostic.contains("private-session"));
        assert!(!diagnostic.contains("sensitive inferred"));
        assert!(!diagnostic.contains("cdcd"));
        assert!(diagnostic.contains("raw_os_error: Some(28)"));
        assert!(diagnostic.contains("[REDACTED]"));
        assert_eq!(
            format!(
                "{:?}",
                CanonicalSnapshotExpectation::Existing(&expected_head)
            ),
            "CanonicalSnapshotExpectation::Existing([BOUND])"
        );
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn missing_parent_and_unqualified_create_refuse_without_mutation() {
        let root = temp_root("create-refusals");
        fs::create_dir_all(&root).expect("create fixture root");
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire namespace-bound guard");
        let missing_parent_target = root.join("absent").join("events.log");
        let unqualified_target = root.join("unqualified.log");
        let key = CanonicalRecoveryKey::from_opaque_bytes([1; 16]);

        assert_eq!(
            guard.append(&missing_parent_target, b"no", None, key),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::ParentProvisioningRequired
            )
        );
        assert_eq!(
            guard.append(&unqualified_target, b"no", None, key),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Linux,
                    operation: CanonicalNamespaceOperation::FirstCreate,
                }
            )
        );
        assert!(!missing_parent_target.exists());
        assert!(!unqualified_target.exists());
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    fn shared_coordination_lock_on_missing_state_is_non_mutating() {
        let missing_root = temp_root("strict-lock-missing-root");
        let existing_root = temp_root("strict-lock-missing-entry");
        fs::create_dir_all(&existing_root).expect("create existing managed root");
        let durability = CanonicalDurability::new();
        assert!(matches!(
            durability.try_lock_shared(&missing_root),
            Err(CanonicalCoordinationError::Missing)
        ));
        assert!(matches!(
            durability.try_lock_shared(&existing_root),
            Err(CanonicalCoordinationError::Missing)
        ));
        assert!(!missing_root.exists());
        assert!(!existing_root.join(COORDINATION_FILE_NAME).exists());
        assert_eq!(
            fs::read_dir(&existing_root)
                .expect("read existing root")
                .count(),
            0
        );
        fs::remove_dir_all(existing_root).expect("clean existing root");
    }

    #[test]
    fn stable_coordination_locks_contend_share_and_release() {
        let root = temp_root("coordination-locks");
        fs::create_dir_all(&root).expect("create fixture root");
        let durability = CanonicalDurability::new();
        let exclusive = durability
            .try_lock_exclusive(&root)
            .expect("acquire exclusive lock");
        assert!(matches!(
            durability.try_lock_exclusive(&root),
            Err(CanonicalCoordinationError::Contended)
        ));
        assert!(matches!(
            durability.try_lock_shared(&root),
            Err(CanonicalCoordinationError::Contended)
        ));
        drop(exclusive);
        let shared_one = durability
            .try_lock_shared(&root)
            .expect("acquire first shared lock");
        let shared_two = durability
            .try_lock_shared(&root)
            .expect("acquire second shared lock");
        assert!(matches!(
            durability.try_lock_exclusive(&root),
            Err(CanonicalCoordinationError::Contended)
        ));
        drop(shared_two);
        drop(shared_one);
        let released = durability
            .try_lock_exclusive(&root)
            .expect("exclusive lock available after release");
        drop(released);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    fn exclusive_coordination_lock_does_not_provision_managed_root() {
        let root = temp_root("coordination-parent");
        assert!(matches!(
            CanonicalDurability::new().try_lock_exclusive(&root),
            Err(CanonicalCoordinationError::ParentProvisioningRequired)
        ));
        assert!(!root.exists());
    }

    #[test]
    fn guard_owned_append_rejects_a_target_outside_its_managed_namespace() {
        let managed_root = temp_root("bound-guard");
        let outside_root = temp_root("bound-guard-outside");
        fs::create_dir_all(&managed_root).expect("create managed root");
        fs::create_dir_all(&outside_root).expect("create outside root");
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&managed_root)
            .expect("acquire namespace-bound guard");
        let outside_target = outside_root.join("events.log");

        assert_eq!(
            guard.append(
                &outside_target,
                b"must not be written",
                None,
                CanonicalRecoveryKey::from_opaque_bytes([11; 16]),
            ),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::TargetOutsideManagedNamespace
            )
        );
        assert!(!outside_target.exists());
        drop(guard);
        fs::remove_dir_all(managed_root).expect("clean managed root");
        fs::remove_dir_all(outside_root).expect("clean outside root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn only_the_exact_target_parent_can_coordinate_a_nested_target() {
        let ancestor_root = temp_root("overlapping-root");
        let exact_root = ancestor_root.join("nested");
        fs::create_dir_all(&exact_root).expect("create nested target parent");
        let target = exact_root.join("events.log");
        let proof = qualification(&exact_root);
        let durability = CanonicalDurability::new();
        let ancestor_guard = durability
            .try_lock_exclusive(&ancestor_root)
            .expect("acquire ancestor guard");
        let exact_guard = durability
            .try_lock_exclusive(&exact_root)
            .expect("acquire exact-parent guard");

        assert_eq!(
            ancestor_guard.append(
                &target,
                b"must not be written",
                Some(&proof),
                CanonicalRecoveryKey::from_opaque_bytes([15; 16]),
            ),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::TargetOutsideManagedNamespace
            )
        );
        assert!(!target.exists());
        assert!(matches!(
            exact_guard.append(
                &target,
                b"exact parent",
                Some(&proof),
                CanonicalRecoveryKey::from_opaque_bytes([16; 16]),
            ),
            CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::FirstCreate,
                ..
            })
        ));
        assert_eq!(
            fs::read(&target).expect("read exact target"),
            b"exact parent"
        );
        drop(exact_guard);
        drop(ancestor_guard);
        fs::remove_dir_all(ancestor_root).expect("clean overlapping roots");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn coordination_entry_is_reserved_from_append_and_rename() {
        let root = temp_root("reserved-coordination-entry");
        fs::create_dir_all(&root).expect("create fixture root");
        let source = root.join("source.tmp");
        let destination = root.join("destination.quarantine");
        let lock_path = root.join(COORDINATION_FILE_NAME);
        fs::write(&source, b"source").expect("seed source");
        let proof = qualification(&root);
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire exact-parent guard");
        let key = CanonicalRecoveryKey::from_opaque_bytes([17; 16]);

        for outcome in [
            guard.append(&lock_path, b"must not write", None, key),
            guard.rename(&lock_path, &destination, Some(&proof), key),
            guard.rename(&source, &lock_path, Some(&proof), key),
        ] {
            assert_eq!(
                outcome,
                CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::ReservedCoordinationEntry
                )
            );
        }
        assert_eq!(fs::metadata(&lock_path).expect("lock retained").len(), 0);
        assert_eq!(fs::read(&source).expect("source retained"), b"source");
        assert!(!destination.exists());
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn mixed_case_coordination_aliases_are_reserved_before_filesystem_access() {
        let root = temp_root("mixed-case-coordination-entry");
        let displaced_root = temp_root("mixed-case-coordination-entry-displaced");
        fs::create_dir_all(&root).expect("create fixture root");
        let source = root.join("source.tmp");
        let destination = root.join("destination.quarantine");
        let mixed_case_alias = root.join(".AuDiO-gRaPh-CaNoNiCaL.LoCk");
        fs::write(&source, b"source").expect("seed source");
        let durability = CanonicalDurability::for_test_name_equivalence(
            ReservedInternalNameEquivalence::AsciiCaseInsensitive,
        );
        let guard = durability
            .try_lock_exclusive(&root)
            .expect("acquire exact-parent guard");
        let key = CanonicalRecoveryKey::from_opaque_bytes([21; 16]);

        // Make every later namespace lookup fail. Reserved-name rejection must
        // still win before validation or any other filesystem access.
        fs::rename(&root, &displaced_root).expect("displace managed root path");

        for outcome in [
            guard.append(&mixed_case_alias, b"must not write", None, key),
            guard.rename(&mixed_case_alias, &destination, None, key),
            guard.rename(&source, &mixed_case_alias, None, key),
            guard.install_snapshot(
                &mixed_case_alias,
                &destination,
                b"must not install",
                CanonicalSnapshotExpectation::Absent,
                None,
                key,
            ),
            guard.install_snapshot(
                &source,
                &mixed_case_alias,
                b"must not install",
                CanonicalSnapshotExpectation::Absent,
                None,
                key,
            ),
        ] {
            assert_eq!(
                outcome,
                CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::ReservedCoordinationEntry
                )
            );
        }

        let displaced_lock = displaced_root.join(COORDINATION_FILE_NAME);
        let displaced_source = displaced_root.join("source.tmp");
        assert_eq!(
            fs::metadata(&displaced_lock).expect("lock retained").len(),
            0
        );
        assert_eq!(
            fs::read(&displaced_source).expect("source retained"),
            b"source"
        );
        assert!(!displaced_root.join("destination.quarantine").exists());
        assert!(!displaced_root.join(".AuDiO-gRaPh-CaNoNiCaL.LoCk").exists());
        assert!(matches!(
            durability.try_lock_shared(&displaced_root),
            Err(CanonicalCoordinationError::Contended)
        ));

        drop(guard);
        let shared = durability
            .try_lock_shared(&displaced_root)
            .expect("held coordination lock releases normally");
        drop(shared);
        fs::remove_dir_all(displaced_root).expect("clean displaced fixture root");
    }

    #[test]
    fn reserved_name_policy_covers_every_ascii_case_permutation_only() {
        let policy = ReservedInternalNameEquivalence::AsciiCaseInsensitive;
        let mut candidate = COORDINATION_FILE_NAME.as_bytes().to_vec();
        let alphabetic_positions = candidate
            .iter()
            .enumerate()
            .filter_map(|(index, byte)| byte.is_ascii_alphabetic().then_some(index))
            .collect::<Vec<_>>();
        let permutation_count = 1_u32 << alphabetic_positions.len();
        let mut previous_gray_code = 0_u32;

        // Gray-code traversal changes exactly one ASCII letter at a time and
        // covers the full 2^N spelling space without allocating per case.
        for ordinal in 0..permutation_count {
            let gray_code = ordinal ^ (ordinal >> 1);
            if ordinal != 0 {
                let changed_bit = (gray_code ^ previous_gray_code).trailing_zeros() as usize;
                let position = alphabetic_positions[changed_bit];
                if gray_code & (1 << changed_bit) == 0 {
                    candidate[position].make_ascii_lowercase();
                } else {
                    candidate[position].make_ascii_uppercase();
                }
            }
            let spelling = std::str::from_utf8(&candidate).expect("ASCII spelling remains UTF-8");
            assert!(
                policy.is_reserved_coordination_entry(Path::new(spelling)),
                "ASCII-case spelling was not reserved: {spelling}"
            );
            previous_gray_code = gray_code;
        }

        for distinct_name in [
            ".audio-graph-canonical.locked",
            ".audio-graph-canonical.lock.",
            ".áudio-graph-canonical.lock",
            ".audio-graph-canonical.löck",
        ] {
            assert!(!policy.is_reserved_coordination_entry(Path::new(distinct_name)));
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn qualification_cannot_be_reused_for_another_root() {
        let first_root = temp_root("proof-first-root");
        let second_root = temp_root("proof-second-root");
        fs::create_dir_all(&first_root).expect("create first root");
        fs::create_dir_all(&second_root).expect("create second root");
        let first_proof = qualification(&first_root);
        let second_target = second_root.join("events.log");
        let second_guard = CanonicalDurability::new()
            .try_lock_exclusive(&second_root)
            .expect("acquire second root guard");

        assert_eq!(
            second_guard.append(
                &second_target,
                b"must not be written",
                Some(&first_proof),
                CanonicalRecoveryKey::from_opaque_bytes([13; 16]),
            ),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::QualificationBindingMismatch
            )
        );
        assert!(!second_target.exists());
        drop(second_guard);
        fs::remove_dir_all(first_root).expect("clean first root");
        fs::remove_dir_all(second_root).expect("clean second root");
    }

    #[test]
    fn algorithm_test_environment_is_bound_and_windows_policy_independent() {
        let first_root = temp_root("algorithm-proof-first-root");
        let second_root = temp_root("algorithm-proof-second-root");
        let displaced_root = temp_root("algorithm-proof-displaced-root");
        fs::create_dir_all(&first_root).expect("create first algorithm root");
        fs::create_dir_all(&second_root).expect("create second algorithm root");
        let environment =
            AlgorithmTestEnvironment::bind_for_platform(&first_root, CanonicalPlatform::Windows)
                .expect("bind opaque algorithm environment");
        let (first_proof, durability) = environment.into_parts();
        let second_target = second_root.join("events.log");
        assert!(matches!(
            durability.try_lock_exclusive(&second_root),
            Err(CanonicalCoordinationError::IdentityUnavailable)
        ));
        assert!(!second_target.exists());

        let first_target = first_root.join("events.log");
        let guard = durability
            .try_lock_exclusive(&first_root)
            .expect("acquire bound synthetic algorithm guard");
        assert_eq!(
            guard.append(
                &first_target,
                b"algorithm bytes",
                Some(&first_proof),
                CanonicalRecoveryKey::from_opaque_bytes([42; 16]),
            ),
            CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::FirstCreate,
                barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
            })
        );
        drop(guard);

        fs::rename(&first_root, &displaced_root).expect("displace bound algorithm root");
        fs::create_dir_all(&first_root).expect("replace bound algorithm root path");
        let replacement_target = first_root.join("replacement.log");

        assert!(matches!(
            durability.try_lock_exclusive(&first_root),
            Err(CanonicalCoordinationError::IdentityUnavailable)
        ));
        assert!(!replacement_target.exists());
        fs::remove_dir_all(first_root).expect("clean first algorithm root");
        fs::remove_dir_all(second_root).expect("clean second algorithm root");
        fs::remove_dir_all(displaced_root).expect("clean displaced algorithm root");
    }

    #[test]
    fn platform_namespace_contract_is_linux_macos_only() {
        assert!(namespace_supported_for(CanonicalPlatform::Linux));
        assert!(namespace_supported_for(CanonicalPlatform::MacOs));
        assert!(!namespace_supported_for(CanonicalPlatform::Windows));
        assert!(!namespace_supported_for(CanonicalPlatform::Other));
    }

    #[test]
    fn windows_policy_path_allows_existing_append_and_refuses_namespace_mutation() {
        let root = temp_root("windows-policy-path");
        fs::create_dir_all(&root).expect("create fixture root");
        let existing = root.join("existing.log");
        let absent = root.join("absent.log");
        let destination = root.join("destination.log");
        let recovery_temporary = root.join("recovery.tmp");
        let recovery_destination = root.join("recovery.bin");
        fs::write(&existing, b"prefix").expect("seed existing file");
        fs::write(&recovery_temporary, b"stable recovery temp").expect("seed recovery temp");
        let guard = CanonicalDurability::for_test_platform(CanonicalPlatform::Windows)
            .try_lock_exclusive(&root)
            .expect("acquire simulated Windows guard");
        let key = CanonicalRecoveryKey::from_opaque_bytes([18; 16]);

        assert!(matches!(
            guard.append(&existing, b"-append", None, key),
            CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::ExistingAppend,
                ..
            })
        ));
        assert_eq!(
            fs::read(&existing).expect("read existing"),
            b"prefix-append"
        );
        assert_eq!(
            guard.append(&absent, b"must not create", None, key),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Windows,
                    operation: CanonicalNamespaceOperation::FirstCreate,
                }
            )
        );
        assert_eq!(
            guard.rename(&existing, &destination, None, key),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Windows,
                    operation: CanonicalNamespaceOperation::Rename,
                }
            )
        );
        assert_eq!(
            guard.preflight_recovery_namespace(
                &existing,
                &recovery_temporary,
                &recovery_destination,
                None,
            ),
            Err(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Windows,
                    operation: CanonicalNamespaceOperation::Rename,
                }
            )
        );
        assert_eq!(
            guard.rename_recovery(&recovery_temporary, &recovery_destination, None, key),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: CanonicalPlatform::Windows,
                    operation: CanonicalNamespaceOperation::Rename,
                }
            )
        );
        assert!(!absent.exists());
        assert!(!destination.exists());
        assert_eq!(
            fs::read(&existing).expect("source retained"),
            b"prefix-append"
        );
        assert_eq!(
            fs::read(&recovery_temporary).expect("recovery temp retained"),
            b"stable recovery temp"
        );
        assert!(!recovery_destination.exists());
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn source_identity_replacement_is_refused_before_rename() {
        let root = temp_root("source-identity-change");
        fs::create_dir_all(&root).expect("create fixture root");
        let source = root.join("source.tmp");
        let displaced = root.join("source.displaced");
        let destination = root.join("destination.quarantine");
        fs::write(&source, b"original").expect("seed source");
        let proof = Arc::new(qualification(&root));
        let barrier = Arc::new(Barrier::new(2));
        let guard = Arc::new(
            CanonicalDurability::with_before_existing_open(barrier.clone())
                .try_lock_exclusive(&root)
                .expect("acquire exact-parent guard"),
        );
        let worker_guard = guard.clone();
        let worker_proof = proof.clone();
        let worker_source = source.clone();
        let worker_destination = destination.clone();
        let worker = thread::spawn(move || {
            worker_guard.rename(
                &worker_source,
                &worker_destination,
                Some(worker_proof.as_ref()),
                CanonicalRecoveryKey::from_opaque_bytes([19; 16]),
            )
        });

        barrier.wait();
        fs::rename(&source, &displaced).expect("displace validated source");
        fs::write(&source, b"replacement").expect("replace source identity");
        barrier.wait();
        let outcome = worker.join().expect("join rename worker");

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::Rejected(CanonicalDurabilityRejection::IdentityChanged)
        );
        assert_eq!(
            fs::read(&source).expect("replacement retained"),
            b"replacement"
        );
        assert_eq!(
            fs::read(&displaced).expect("original retained"),
            b"original"
        );
        assert!(!destination.exists());
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn preflight_device_mismatch_refuses_before_invoking_rename() {
        let root = temp_root("preflight-cross-device-refusal");
        fs::create_dir_all(&root).expect("create fixture root");
        let source = root.join("source.tmp");
        let destination = root.join("destination.quarantine");
        fs::write(&source, b"source").expect("seed source");
        let proof = qualification(&root);
        let rename_invoked = Arc::new(AtomicBool::new(false));
        let guard = CanonicalDurability::with_rename_fault(
            InjectedRenameFault::PreflightDeviceMismatch,
            rename_invoked.clone(),
        )
        .try_lock_exclusive(&root)
        .expect("acquire exact-parent guard");

        let outcome = guard.rename(
            &source,
            &destination,
            Some(&proof),
            CanonicalRecoveryKey::from_opaque_bytes([20; 16]),
        );

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::CrossDeviceRenameRefused { raw_os_error: None }
            )
        );
        assert!(!rename_invoked.load(Ordering::SeqCst));
        assert_eq!(fs::read(&source).expect("source retained"), b"source");
        assert!(!destination.exists());
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn recovery_descendant_volume_must_match_opaque_root_qualification_before_mutation() {
        let root = temp_root("recovery-descendant-volume-binding");
        fs::create_dir_all(root.join("streams")).expect("create streams parent");
        fs::create_dir_all(root.join("recovery")).expect("create recovery parent");
        let source = root.join("streams/events.jsonl");
        let temporary = root.join("recovery/events.tmp");
        let destination = root.join("recovery/events.bin");
        let manifest = root.join(".audio-graph-session-artifacts.v1.json");
        fs::write(&source, b"full source").expect("seed source");
        fs::write(&manifest, b"stable manifest").expect("seed manifest marker");
        fs::write(root.join(COORDINATION_FILE_NAME), b"").expect("seed coordination entry");
        let proof = qualification(&root);
        let rename_invoked = Arc::new(AtomicBool::new(false));
        let guard = CanonicalDurability::with_rename_fault(
            InjectedRenameFault::DescendantQualificationMismatch,
            rename_invoked.clone(),
        )
        .try_lock_exclusive(&root)
        .expect("acquire qualified guard");

        assert_eq!(
            guard.preflight_recovery_namespace(&source, &temporary, &destination, Some(&proof),),
            Err(CanonicalDurabilityRejection::QualifiedDescendantVolumeMismatch)
        );
        assert!(!rename_invoked.load(Ordering::SeqCst));
        assert_eq!(fs::read(&source).expect("source unchanged"), b"full source");
        assert_eq!(
            fs::read(&manifest).expect("manifest unchanged"),
            b"stable manifest"
        );
        assert!(!temporary.exists());
        assert!(!destination.exists());
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn recovery_descendant_unavailable_volume_is_named_and_non_mutating() {
        let root = temp_root("recovery-descendant-volume-unavailable");
        fs::create_dir_all(root.join("streams")).expect("create streams parent");
        fs::create_dir_all(root.join("recovery")).expect("create recovery parent");
        let source = root.join("streams/events.jsonl");
        let temporary = root.join("recovery/events.tmp");
        let destination = root.join("recovery/events.bin");
        fs::write(&source, b"full source").expect("seed source");
        let proof = qualification(&root);
        let guard = CanonicalDurability::with_rename_fault(
            InjectedRenameFault::DescendantQualificationUnavailable,
            Arc::new(AtomicBool::new(false)),
        )
        .try_lock_exclusive(&root)
        .expect("acquire qualified guard");

        assert_eq!(
            guard.preflight_recovery_namespace(&source, &temporary, &destination, Some(&proof),),
            Err(CanonicalDurabilityRejection::QualifiedDescendantVolumeUnavailable)
        );
        assert_eq!(fs::read(&source).expect("source unchanged"), b"full source");
        assert!(!temporary.exists());
        assert!(!destination.exists());
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn runtime_rename_exdev_is_indeterminate_after_invocation() {
        let root = temp_root("runtime-rename-exdev");
        fs::create_dir_all(&root).expect("create fixture root");
        let source = root.join("source.tmp");
        let destination = root.join("destination.quarantine");
        fs::write(&source, b"source").expect("seed source");
        let proof = qualification(&root);
        let rename_invoked = Arc::new(AtomicBool::new(false));
        let recovery_key = CanonicalRecoveryKey::from_opaque_bytes([22; 16]);
        let guard = CanonicalDurability::with_rename_fault(
            InjectedRenameFault::RuntimeExdev { raw_os_error: 18 },
            rename_invoked.clone(),
        )
        .try_lock_exclusive(&root)
        .expect("acquire exact-parent guard");

        let outcome = guard.rename(&source, &destination, Some(&proof), recovery_key);

        assert!(rename_invoked.load(Ordering::SeqCst));
        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: CanonicalDurabilityStage::Rename,
                kind: io::ErrorKind::CrossesDevices,
                raw_os_error: Some(18),
                recovery_key,
            })
        );

        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    fn append_barrier_failures_are_indeterminate_and_keep_raw_os_error() {
        for (stage, raw_os_error) in [
            (CanonicalDurabilityStage::Write, None),
            (CanonicalDurabilityStage::Flush, Some(28)),
            (CanonicalDurabilityStage::FileSync, None),
        ] {
            let root = temp_root(&format!("append-failure-{stage:?}"));
            fs::create_dir_all(&root).expect("create fixture root");
            let target = root.join("events.log");
            fs::write(&target, b"prefix").expect("seed existing file");
            let durability = match raw_os_error {
                Some(raw) => CanonicalDurability::failing_at_with_raw_os_error(stage, raw),
                None => CanonicalDurability::failing_at(stage),
            };
            let guard = durability
                .try_lock_exclusive(&root)
                .expect("acquire namespace-bound guard");
            let key = CanonicalRecoveryKey::from_opaque_bytes([3; 16]);
            let outcome = guard.append(&target, b"possibly visible", None, key);
            let expected_error = raw_os_error.map(io::Error::from_raw_os_error);
            let expected_kind = expected_error
                .as_ref()
                .map_or(io::ErrorKind::Other, io::Error::kind);

            assert_eq!(
                outcome,
                CanonicalDurabilityOutcome::DurabilityIndeterminate(
                    CanonicalDurabilityIndeterminate {
                        stage,
                        kind: expected_kind,
                        raw_os_error,
                        recovery_key: key,
                    }
                )
            );
            drop(guard);
            fs::remove_dir_all(root).expect("clean fixture root");
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn first_create_and_rename_barrier_failures_are_indeterminate() {
        let root = temp_root("namespace-failures");
        fs::create_dir_all(&root).expect("create fixture root");
        let proof = qualification(&root);
        let key = CanonicalRecoveryKey::from_opaque_bytes([4; 16]);
        let create_target = root.join("create.log");
        let create_durability =
            CanonicalDurability::failing_at(CanonicalDurabilityStage::ParentSync);
        let create_guard = create_durability
            .try_lock_exclusive(&root)
            .expect("acquire create guard");
        assert!(matches!(
            create_guard.append(&create_target, b"visible", Some(&proof), key),
            CanonicalDurabilityOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: CanonicalDurabilityStage::ParentSync,
                ..
            })
        ));
        assert!(create_target.exists());
        drop(create_guard);

        for stage in [
            CanonicalDurabilityStage::FileSync,
            CanonicalDurabilityStage::Rename,
            CanonicalDurabilityStage::ParentSync,
        ] {
            let source = root.join(format!("source-{stage:?}.tmp"));
            let destination = root.join(format!("destination-{stage:?}.quarantine"));
            fs::write(&source, b"recoverable").expect("seed rename source");
            let durability = CanonicalDurability::failing_at(stage);
            let guard = durability
                .try_lock_exclusive(&root)
                .expect("acquire rename guard");
            assert!(matches!(
                guard.rename(&source, &destination, Some(&proof), key),
                CanonicalDurabilityOutcome::DurabilityIndeterminate(
                    CanonicalDurabilityIndeterminate { stage: actual, .. }
                ) if actual == stage
            ));
            drop(guard);
        }
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn rename_refuses_regular_dangling_symlink_and_fifo_destinations() {
        use std::os::unix::fs::symlink;
        use std::process::Command;

        for entry_kind in ["regular", "symlink", "fifo"] {
            let root = temp_root(&format!("rename-collision-{entry_kind}"));
            fs::create_dir_all(&root).expect("create fixture root");
            let source = root.join("source.tmp");
            let destination = root.join("destination.quarantine");
            fs::write(&source, b"source").expect("seed rename source");
            match entry_kind {
                "regular" => {
                    fs::write(&destination, b"destination").expect("seed regular destination")
                }
                "symlink" => {
                    symlink(root.join("missing"), &destination).expect("seed dangling symlink")
                }
                "fifo" => {
                    let status = Command::new("mkfifo")
                        .arg(&destination)
                        .status()
                        .expect("run mkfifo");
                    assert!(status.success());
                }
                _ => unreachable!(),
            }
            let proof = qualification(&root);
            let guard = CanonicalDurability::new()
                .try_lock_exclusive(&root)
                .expect("acquire namespace-bound guard");

            assert_eq!(
                guard.rename(
                    &source,
                    &destination,
                    Some(&proof),
                    CanonicalRecoveryKey::from_opaque_bytes([6; 16]),
                ),
                CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::DestinationAlreadyExists
                )
            );
            assert_eq!(fs::read(&source).expect("source retained"), b"source");
            assert!(fs::symlink_metadata(&destination).is_ok());
            drop(guard);
            fs::remove_dir_all(root).expect("clean fixture root");
        }
    }

    #[test]
    #[cfg(unix)]
    fn existing_non_regular_canonical_entry_is_refused_without_opening() {
        use std::process::Command;

        let root = temp_root("append-fifo");
        fs::create_dir_all(&root).expect("create fixture root");
        let target = root.join("events.log");
        let status = Command::new("mkfifo")
            .arg(&target)
            .status()
            .expect("run mkfifo");
        assert!(status.success());
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire namespace-bound guard");

        assert_eq!(
            guard.append(
                &target,
                b"must not block or write",
                None,
                CanonicalRecoveryKey::from_opaque_bytes([14; 16]),
            ),
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NonRegularCanonicalEntry
            )
        );
        assert!(
            fs::symlink_metadata(&target)
                .expect("fifo retained")
                .file_type()
                .is_fifo()
        );
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    fn indeterminate_diagnostics_redact_content_and_keep_numeric_os_error() {
        let root = temp_root("redacted-diagnostics");
        fs::create_dir_all(&root).expect("create fixture root");
        let target = root.join("private-session-name.log");
        fs::write(&target, b"prefix").expect("seed existing file");
        let durability =
            CanonicalDurability::failing_at_with_raw_os_error(CanonicalDurabilityStage::Flush, 28);
        let guard = durability
            .try_lock_exclusive(&root)
            .expect("acquire namespace-bound guard");
        let secret_payload = "sensitive spoken words";
        let outcome = guard.append(
            &target,
            secret_payload.as_bytes(),
            None,
            CanonicalRecoveryKey::from_opaque_bytes([0xab; 16]),
        );

        let diagnostic = format!("{outcome:?}");
        assert!(!diagnostic.contains("private-session-name"));
        assert!(!diagnostic.contains(secret_payload));
        assert!(!diagnostic.contains("abab"));
        assert!(diagnostic.contains("raw_os_error: Some(28)"));
        assert!(diagnostic.contains("[REDACTED]"));
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    // audio-graph-3cf2 — the durable-unlink primitive: guard preflight,
    // namespace qualification, parent barrier, and its own fault table.

    #[test]
    #[cfg(target_os = "linux")]
    fn unlink_refuses_reserved_windows_unqualified_and_non_regular_entries_before_mutation() {
        use std::os::unix::fs::symlink;
        use std::process::Command;

        let root = temp_root("unlink-reserved-and-unsupported");
        fs::create_dir_all(&root).expect("create fixture root");
        let target = root.join("session-artifacts.tmp");
        fs::write(&target, b"staged intent").expect("seed staged intent");
        let lock_path = root.join(COORDINATION_FILE_NAME);
        let mixed_case_alias = root.join(".AuDiO-gRaPh-CaNoNiCaL.LoCk");
        let proof = qualification(&root);
        let key = CanonicalRecoveryKey::from_opaque_bytes([70; 16]);

        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire namespace-bound guard");

        // The store-owned coordination entry is unremovable under every
        // ASCII-case spelling, before any filesystem access.
        for reserved in [&lock_path, &mixed_case_alias] {
            assert_eq!(
                guard.unlink_canonical_entry(reserved, Some(&proof), key),
                CanonicalUnlinkOutcome::Rejected(
                    CanonicalDurabilityRejection::ReservedCoordinationEntry
                )
            );
        }
        assert_eq!(fs::metadata(&lock_path).expect("lock retained").len(), 0);

        // No qualification evidence: the parent barrier cannot be claimed.
        assert_eq!(
            guard.unlink_canonical_entry(&target, None, key),
            CanonicalUnlinkOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: current_platform(),
                    operation: CanonicalNamespaceOperation::Unlink,
                }
            )
        );

        // Qualification bound to a different root.
        let foreign_root = temp_root("unlink-foreign-qualification");
        fs::create_dir_all(&foreign_root).expect("create foreign root");
        let foreign_proof = qualification(&foreign_root);
        assert_eq!(
            guard.unlink_canonical_entry(&target, Some(&foreign_proof), key),
            CanonicalUnlinkOutcome::Rejected(
                CanonicalDurabilityRejection::QualificationBindingMismatch
            )
        );

        // A target outside the managed namespace, and a nested descendant: this
        // primitive reaches immediate children of the managed root only.
        let nested = root.join("nested");
        fs::create_dir_all(&nested).expect("create nested directory");
        let nested_target = nested.join("session-artifacts.tmp");
        fs::write(&nested_target, b"nested intent").expect("seed nested intent");
        for outside in [foreign_root.join("stray.tmp"), nested_target.clone()] {
            assert_eq!(
                guard.unlink_canonical_entry(&outside, Some(&proof), key),
                CanonicalUnlinkOutcome::Rejected(
                    CanonicalDurabilityRejection::TargetOutsideManagedNamespace
                )
            );
        }
        assert_eq!(
            fs::read(&nested_target).expect("nested entry retained"),
            b"nested intent"
        );

        // Non-regular entries are refused and survive.
        for entry_kind in ["symlink", "directory", "fifo", "socket"] {
            let non_regular = root.join(format!("non-regular-{entry_kind}"));
            match entry_kind {
                "symlink" => symlink(root.join("missing"), &non_regular).expect("seed symlink"),
                "directory" => fs::create_dir(&non_regular).expect("seed directory"),
                "fifo" => {
                    let status = Command::new("mkfifo")
                        .arg(&non_regular)
                        .status()
                        .expect("run mkfifo");
                    assert!(status.success());
                }
                "socket" => {
                    std::os::unix::net::UnixListener::bind(&non_regular).expect("seed socket");
                }
                _ => unreachable!(),
            }
            assert_eq!(
                guard.unlink_canonical_entry(&non_regular, Some(&proof), key),
                CanonicalUnlinkOutcome::Rejected(
                    CanonicalDurabilityRejection::NonRegularCanonicalEntry
                )
            );
            assert!(fs::symlink_metadata(&non_regular).is_ok());
        }
        assert_eq!(
            fs::read(&target).expect("staged intent retained"),
            b"staged intent"
        );
        drop(guard);

        // Windows and Other refuse before inspecting or removing anything.
        for platform in [CanonicalPlatform::Windows, CanonicalPlatform::Other] {
            let platform_guard = CanonicalDurability::for_test_platform(platform)
                .try_lock_exclusive(&root)
                .expect("acquire simulated platform guard");
            assert_eq!(
                platform_guard.unlink_canonical_entry(&target, Some(&proof), key),
                CanonicalUnlinkOutcome::Rejected(
                    CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                        platform,
                        operation: CanonicalNamespaceOperation::Unlink,
                    }
                )
            );
            assert_eq!(
                fs::read(&target).expect("staged intent retained"),
                b"staged intent"
            );
            drop(platform_guard);
        }

        fs::remove_dir_all(root).expect("clean fixture root");
        fs::remove_dir_all(foreign_root).expect("clean foreign root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn unlink_refuses_a_replaced_managed_root_before_mutation() {
        let root = temp_root("unlink-replaced-root");
        let displaced = temp_root("unlink-replaced-root-displaced");
        fs::create_dir_all(&root).expect("create managed root");
        let target = root.join("session-artifacts.tmp");
        fs::write(&target, b"staged intent").expect("seed staged intent");
        let proof = qualification(&root);
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire exact-root guard");
        fs::rename(&root, &displaced).expect("displace managed root");
        fs::create_dir_all(&root).expect("replace managed root path");

        assert_eq!(
            guard.unlink_canonical_entry(
                &target,
                Some(&proof),
                CanonicalRecoveryKey::from_opaque_bytes([71; 16]),
            ),
            CanonicalUnlinkOutcome::Rejected(CanonicalDurabilityRejection::IdentityChanged)
        );
        assert_eq!(
            fs::read(displaced.join("session-artifacts.tmp")).expect("displaced entry retained"),
            b"staged intent"
        );
        drop(guard);
        fs::remove_dir_all(root).expect("clean replacement root");
        fs::remove_dir_all(displaced).expect("clean displaced root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn unlink_fault_cuts_are_honest_and_exact_rerun_is_a_no_effect_assessment() {
        let key = CanonicalRecoveryKey::from_opaque_bytes([72; 16]);

        // Every injectable cut before the removal call is a refusal, and the
        // entry is provably intact.
        for stage in [
            CanonicalDurabilityStage::InspectEntry,
            CanonicalDurabilityStage::OpenExisting,
            CanonicalDurabilityStage::Unlink,
        ] {
            let root = temp_root(&format!("unlink-cut-{stage:?}"));
            fs::create_dir_all(&root).expect("create fixture root");
            let target = root.join("session-artifacts.tmp");
            fs::write(&target, b"staged intent").expect("seed staged intent");
            let proof = qualification(&root);
            let guard = CanonicalDurability::new()
                .try_lock_exclusive(&root)
                .expect("acquire namespace-bound guard");

            assert_eq!(
                guard.unlink_canonical_entry_with_fault(&target, Some(&proof), key, stage),
                CanonicalUnlinkOutcome::Rejected(
                    CanonicalDurabilityRejection::IoFailedBeforeMutation {
                        stage,
                        kind: io::ErrorKind::Other,
                        raw_os_error: None,
                    }
                )
            );
            assert_eq!(
                fs::read(&target).expect("entry retained after refused cut"),
                b"staged intent"
            );
            drop(guard);
            fs::remove_dir_all(root).expect("clean fixture root");
        }

        // The guard-level failure injection lands on the same pre-invocation arm.
        let injected_root = temp_root("unlink-injected-failure");
        fs::create_dir_all(&injected_root).expect("create fixture root");
        let injected_target = injected_root.join("session-artifacts.tmp");
        fs::write(&injected_target, b"staged intent").expect("seed staged intent");
        let injected_proof = qualification(&injected_root);
        let injected_guard = CanonicalDurability::failing_at(CanonicalDurabilityStage::Unlink)
            .try_lock_exclusive(&injected_root)
            .expect("acquire failing guard");
        assert_eq!(
            injected_guard.unlink_canonical_entry(&injected_target, Some(&injected_proof), key),
            CanonicalUnlinkOutcome::Rejected(
                CanonicalDurabilityRejection::IoFailedBeforeMutation {
                    stage: CanonicalDurabilityStage::Unlink,
                    kind: io::ErrorKind::Other,
                    raw_os_error: None,
                }
            )
        );
        assert_eq!(
            fs::read(&injected_target).expect("entry retained after injected failure"),
            b"staged intent"
        );
        drop(injected_guard);
        fs::remove_dir_all(injected_root).expect("clean fixture root");

        // A lost parent barrier after a real removal is indeterminate, and the
        // exact rerun publishes the absence.
        let barrier_root = temp_root("unlink-parent-sync-cut");
        fs::create_dir_all(&barrier_root).expect("create fixture root");
        let barrier_target = barrier_root.join("session-artifacts.tmp");
        fs::write(&barrier_target, b"staged intent").expect("seed staged intent");
        let barrier_proof = qualification(&barrier_root);
        let barrier_guard = CanonicalDurability::new()
            .try_lock_exclusive(&barrier_root)
            .expect("acquire namespace-bound guard");
        assert!(matches!(
            barrier_guard.unlink_canonical_entry_with_fault(
                &barrier_target,
                Some(&barrier_proof),
                key,
                CanonicalDurabilityStage::ParentSync,
            ),
            CanonicalUnlinkOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: CanonicalDurabilityStage::ParentSync,
                ..
            })
        ));
        assert!(!barrier_target.exists());
        assert_eq!(
            barrier_guard.unlink_canonical_entry(&barrier_target, Some(&barrier_proof), key),
            CanonicalUnlinkOutcome::AlreadyAbsent(CanonicalDurabilityBarrier::ParentNamespace)
        );
        drop(barrier_guard);
        fs::remove_dir_all(barrier_root).expect("clean fixture root");

        // A clean removal crosses the parent barrier only, and its exact rerun
        // is a no-effect assessment.
        let clean_root = temp_root("unlink-clean");
        fs::create_dir_all(&clean_root).expect("create fixture root");
        let clean_target = clean_root.join("session-artifacts.tmp");
        fs::write(&clean_target, b"staged intent").expect("seed staged intent");
        let clean_proof = qualification(&clean_root);
        let clean_guard = CanonicalDurability::new()
            .try_lock_exclusive(&clean_root)
            .expect("acquire namespace-bound guard");
        assert_eq!(
            clean_guard.unlink_canonical_entry(&clean_target, Some(&clean_proof), key),
            CanonicalUnlinkOutcome::Unlinked(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::Unlink,
                barrier: CanonicalDurabilityBarrier::ParentNamespace,
            })
        );
        assert!(!clean_target.exists());
        assert_eq!(
            clean_guard.unlink_canonical_entry(&clean_target, Some(&clean_proof), key),
            CanonicalUnlinkOutcome::AlreadyAbsent(CanonicalDurabilityBarrier::ParentNamespace)
        );
        assert!(!clean_target.exists());
        drop(clean_guard);
        fs::remove_dir_all(clean_root).expect("clean fixture root");
    }

    // audio-graph-dbd4 — the one unlink arm that can leave a real durability
    // question open in production: `remove_file` itself failing.

    /// Measure whether a read-only parent directory actually denies removal on
    /// this host, and report the exact error it denies with.
    ///
    /// This is a capability, never an assumption: an effective root uid keeps
    /// write access to a `0o555` directory, so there is nothing to deny and no
    /// `EACCES` for the primitive to classify. Returning the observed
    /// `ErrorKind`/errno pair also keeps the test free of a hardcoded errno
    /// constant — it asserts the error the host raised, not one this file
    /// guessed.
    #[cfg(target_os = "linux")]
    /// The effective uid, without taking a `libc` dependency for one call: a
    /// file this process just created is owned by exactly that uid.
    fn effective_uid() -> u32 {
        use std::os::unix::fs::MetadataExt;

        let probe_root = temp_root("euid-probe");
        fs::create_dir_all(&probe_root).expect("create euid probe root");
        let probe = probe_root.join("owner");
        fs::write(&probe, b"owner").expect("seed euid probe");
        let uid = fs::metadata(&probe)
            .expect("read euid probe metadata")
            .uid();
        fs::remove_dir_all(probe_root).expect("clean euid probe root");
        uid
    }

    fn read_only_parent_removal_denial() -> Option<(io::ErrorKind, Option<i32>)> {
        use std::os::unix::fs::PermissionsExt;

        let probe_root = temp_root("unlink-denial-probe");
        fs::create_dir_all(&probe_root).expect("create probe root");
        let probe = probe_root.join("probe");
        fs::write(&probe, b"probe").expect("seed probe entry");
        fs::set_permissions(&probe_root, fs::Permissions::from_mode(0o555))
            .expect("make the probe parent read-only");
        let denial = fs::remove_file(&probe)
            .err()
            .map(|error| (error.kind(), error.raw_os_error()));
        fs::set_permissions(&probe_root, fs::Permissions::from_mode(0o755))
            .expect("restore the probe parent");
        fs::remove_dir_all(probe_root).expect("clean probe root");
        denial
    }

    /// Both injectable `Unlink` cuts sit BEFORE the removal call, so the
    /// `DurabilityIndeterminate { stage: Unlink }` arm shipped classified but
    /// unexercised. This drives it from a real `EACCES` on a read-only parent
    /// rather than from a new post-invocation seam, which also proves the
    /// conservatism the rustdoc now states: the permission check refuses before
    /// any directory entry can change, so the entry provably survives an outcome
    /// that reports a durability question about it. The recovery key that
    /// outcome hands back is then the only input the resolving rerun needs.
    #[test]
    #[cfg(target_os = "linux")]
    fn unlink_indeterminate_at_the_removal_call_is_conservative_and_rerunnable() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("unlink-denied-removal");
        fs::create_dir_all(&root).expect("create fixture root");
        let target = root.join("session-artifacts.tmp");
        fs::write(&target, b"staged intent").expect("seed staged intent");
        let proof = qualification(&root);
        let key = CanonicalRecoveryKey::from_opaque_bytes([73; 16]);
        // The guard's coordination entry must exist before the parent turns
        // read-only, so the lock is taken first.
        let guard = CanonicalDurability::new()
            .try_lock_exclusive(&root)
            .expect("acquire namespace-bound guard");
        // Skipping is keyed on the PRIVILEGE that explains a missing denial, not
        // on the missing denial itself. Root legitimately bypasses a read-only
        // parent; a non-root uid that is not denied means this arm silently went
        // untested, which must fail rather than print a reassuring skip line.
        let denial = match read_only_parent_removal_denial() {
            Some(denial) => denial,
            None => {
                let euid = effective_uid();
                drop(guard);
                fs::remove_dir_all(root).expect("clean fixture root");
                assert_eq!(
                    euid, 0,
                    "a read-only parent did not deny removal for euid {euid}, which is not root: \
                     the indeterminate-at-removal arm went untested and this host cannot prove it"
                );
                println!(
                    "outcome=unavailable_evidence \
                     detail=running-as-root-so-a-read-only-parent-cannot-deny-removal"
                );
                return;
            }
        };

        fs::set_permissions(&root, fs::Permissions::from_mode(0o555))
            .expect("make the managed root read-only");
        let outcome = guard.unlink_canonical_entry(&target, Some(&proof), key);
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755))
            .expect("restore the managed root");

        // The classification: indeterminate at the removal call, reporting the
        // error the host actually raised and the caller's own recovery key.
        let CanonicalUnlinkOutcome::DurabilityIndeterminate(indeterminate) = outcome else {
            panic!("a denied removal must be indeterminate, reported {outcome:?}");
        };
        assert_eq!(
            (
                indeterminate.stage,
                indeterminate.kind,
                indeterminate.raw_os_error
            ),
            (CanonicalDurabilityStage::Unlink, denial.0, denial.1),
        );
        assert_eq!(indeterminate.recovery_key, key);
        // Conservative, and provably so: the refused permission check removed
        // nothing, yet the outcome still names a durability question.
        assert_eq!(
            fs::read(&target).expect("entry survives a denied removal"),
            b"staged intent"
        );

        // The key it handed back is exactly what the resolving rerun needs: the
        // same key against the same pathname, which then publishes the removal.
        assert_eq!(
            guard.unlink_canonical_entry(&target, Some(&proof), indeterminate.recovery_key),
            CanonicalUnlinkOutcome::Unlinked(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::Unlink,
                barrier: CanonicalDurabilityBarrier::ParentNamespace,
            })
        );
        assert!(!target.exists());
        assert_eq!(
            guard.unlink_canonical_entry(&target, Some(&proof), indeterminate.recovery_key),
            CanonicalUnlinkOutcome::AlreadyAbsent(CanonicalDurabilityBarrier::ParentNamespace)
        );
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }
}
