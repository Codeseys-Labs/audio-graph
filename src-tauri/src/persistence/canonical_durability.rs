//! Canonical file and namespace durability barriers.
//!
//! This dormant module owns the filesystem sequencing behind a small,
//! provider-neutral interface. A mutation guard is bound to one existing
//! managed namespace, safe platform identity where available, and one
//! deterministic coordination file inside that namespace. Guard-owned
//! operations cannot be redirected to another root. Locks remain cooperative:
//! an uncooperative process can still replace roots or race pathname operations
//! outside this contract.

use std::fmt;
use std::fs::{File, Metadata, OpenOptions, TryLockError};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[cfg(test)]
use std::sync::{Arc, Barrier};

const COORDINATION_FILE_NAME: &str = ".audio-graph-canonical.lock";

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
}

impl fmt::Debug for CanonicalFilesystemQualification {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CanonicalFilesystemQualification([BOUND])")
    }
}

impl CanonicalFilesystemQualification {
    #[cfg(test)]
    fn for_test_root(root: &Path) -> Result<Self, CanonicalCoordinationError> {
        let namespace =
            ManagedNamespace::load(root, CanonicalCoordinationError::ParentProvisioningRequired)?;
        if namespace.identity.volume.is_none() || namespace.identity.object.is_none() {
            return Err(CanonicalCoordinationError::IdentityUnavailable);
        }
        Ok(Self { namespace })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalPlatform {
    Linux,
    MacOs,
    Windows,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalNamespaceOperation {
    FirstCreate,
    Rename,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalMutation {
    ExistingAppend,
    FirstCreate,
    Rename,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalDurabilityBarrier {
    FileDataAndMetadata,
    FileAndParentNamespace,
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
    SeekEnd,
    Write,
    Flush,
    FileSync,
    Rename,
    ParentSync,
}

/// A refusal proven before this module mutated canonical bytes or namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalDurabilityRejection {
    ParentProvisioningRequired,
    NamespaceDurabilityUnsupported {
        platform: CanonicalPlatform,
        operation: CanonicalNamespaceOperation,
    },
    TargetOutsideManagedNamespace,
    ManagedNamespaceChanged,
    QualificationBindingMismatch,
    NonRegularCanonicalEntry,
    DestinationAlreadyExists,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ObjectIdentity {
    #[cfg(unix)]
    UnixInode(u64),
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

    fn validate_current(&self) -> Result<(), CanonicalDurabilityRejection> {
        let current_root = std::fs::canonicalize(&self.canonical_root)
            .map_err(|error| durability_io(CanonicalDurabilityStage::ValidateNamespace, &error))?;
        let metadata = std::fs::metadata(&current_root)
            .map_err(|error| durability_io(CanonicalDurabilityStage::ValidateNamespace, &error))?;
        if current_root != self.canonical_root
            || !metadata.is_dir()
            || filesystem_identity(&metadata) != self.identity
        {
            return Err(CanonicalDurabilityRejection::ManagedNamespaceChanged);
        }
        Ok(())
    }

    fn bind_parent(&self, target: &Path) -> Result<BoundParent, CanonicalDurabilityRejection> {
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
        let identity = filesystem_identity(&metadata);
        if !metadata.is_dir() || identity.volume != self.identity.volume {
            return Err(CanonicalDurabilityRejection::TargetOutsideManagedNamespace);
        }
        let directory = File::open(&canonical_parent)
            .map_err(|error| durability_io(CanonicalDurabilityStage::OpenParent, &error))?;
        Ok(BoundParent {
            canonical_path: canonical_parent,
            directory,
        })
    }
}

struct BoundParent {
    canonical_path: PathBuf,
    directory: File,
}

/// Exclusive mutation transaction bound to one managed namespace.
///
/// The deterministic coordination file and namespace identity are private, so
/// a caller cannot pair an arbitrary lock with an unrelated mutation target.
pub struct CanonicalExclusiveGuard {
    namespace: ManagedNamespace,
    _lock_file: File,
    operation_lock: Mutex<()>,
    #[cfg(test)]
    injected_failure: Option<InjectedFailure>,
    #[cfg(test)]
    before_atomic_create: Option<Arc<Barrier>>,
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

/// Factory for namespace-bound cooperative guards.
#[derive(Default)]
pub struct CanonicalDurability {
    #[cfg(test)]
    injected_failure: Option<InjectedFailure>,
    #[cfg(test)]
    before_atomic_create: Option<Arc<Barrier>>,
}

impl CanonicalDurability {
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire the deterministic writer coordination file inside an existing
    /// managed root. The root itself is never provisioned.
    pub fn try_lock_exclusive(
        &self,
        managed_root: &Path,
    ) -> Result<CanonicalExclusiveGuard, CanonicalCoordinationError> {
        let namespace = ManagedNamespace::load(
            managed_root,
            CanonicalCoordinationError::ParentProvisioningRequired,
        )?;
        let lock_path = namespace.canonical_root.join(COORDINATION_FILE_NAME);
        let file = open_writer_coordination(&lock_path)?;
        let file = try_lock_exclusive(file)?;
        Ok(CanonicalExclusiveGuard {
            namespace,
            _lock_file: file,
            operation_lock: Mutex::new(()),
            #[cfg(test)]
            injected_failure: self.injected_failure,
            #[cfg(test)]
            before_atomic_create: self.before_atomic_create.clone(),
        })
    }

    /// Acquire a shared strict-reader lock without creating missing state.
    pub fn try_lock_shared(
        &self,
        managed_root: &Path,
    ) -> Result<CanonicalSharedGuard, CanonicalCoordinationError> {
        let namespace = ManagedNamespace::load(managed_root, CanonicalCoordinationError::Missing)?;
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
            injected_failure: Some(InjectedFailure {
                stage,
                raw_os_error: None,
            }),
            before_atomic_create: None,
        }
    }

    #[cfg(test)]
    fn failing_at_with_raw_os_error(stage: CanonicalDurabilityStage, raw_os_error: i32) -> Self {
        Self {
            injected_failure: Some(InjectedFailure {
                stage,
                raw_os_error: Some(raw_os_error),
            }),
            before_atomic_create: None,
        }
    }

    #[cfg(test)]
    fn with_before_atomic_create(barrier: Arc<Barrier>) -> Self {
        Self {
            injected_failure: None,
            before_atomic_create: Some(barrier),
        }
    }
}

impl CanonicalExclusiveGuard {
    /// Append bytes inside the bound managed namespace.
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
        let _operation = match self.operation_lock.lock() {
            Ok(operation) => operation,
            Err(_) => {
                return CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::CoordinationPoisoned,
                );
            }
        };
        let parent = match self.namespace.bind_parent(path) {
            Ok(parent) => parent,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        let namespace_qualified = match self.qualification_status(qualification) {
            Ok(status) => status,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };

        let opened = if namespace_qualified {
            self.wait_before_atomic_create();
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(file) => OpenedCanonicalFile::New {
                    file,
                    parent: parent.directory,
                },
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    match open_existing_regular(path) {
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
            match open_existing_regular(path) {
                Ok(file) => OpenedCanonicalFile::Existing(file),
                Err(CanonicalDurabilityRejection::IoFailedBeforeMutation {
                    kind: io::ErrorKind::NotFound,
                    ..
                }) => {
                    return CanonicalDurabilityOutcome::Rejected(
                        CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                            platform: current_platform(),
                            operation: CanonicalNamespaceOperation::FirstCreate,
                        },
                    );
                }
                Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
            }
        };

        self.append_opened(opened, bytes, recovery_key)
    }

    /// Publish a regular source file under a new unique name inside the bound
    /// managed namespace.
    pub fn rename(
        &self,
        source: &Path,
        destination: &Path,
        qualification: Option<&CanonicalFilesystemQualification>,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        let _operation = match self.operation_lock.lock() {
            Ok(operation) => operation,
            Err(_) => {
                return CanonicalDurabilityOutcome::Rejected(
                    CanonicalDurabilityRejection::CoordinationPoisoned,
                );
            }
        };
        let source_parent = match self.namespace.bind_parent(source) {
            Ok(parent) => parent,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        let destination_parent = match self.namespace.bind_parent(destination) {
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
                    platform: current_platform(),
                    operation: CanonicalNamespaceOperation::Rename,
                },
            );
        }

        let source_file = match open_existing_regular(source) {
            Ok(file) => file,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        if let Err(rejection) = ensure_destination_absent(destination) {
            return CanonicalDurabilityOutcome::Rejected(rejection);
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::FileSync, || {
            source_file.sync_all()
        }) {
            return indeterminate(CanonicalDurabilityStage::FileSync, &error, recovery_key);
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::Rename, || {
            std::fs::rename(source, destination)
        }) {
            return indeterminate(CanonicalDurabilityStage::Rename, &error, recovery_key);
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::ParentSync, || {
            source_parent.directory.sync_all()
        }) {
            return indeterminate(CanonicalDurabilityStage::ParentSync, &error, recovery_key);
        }
        if source_parent.canonical_path != destination_parent.canonical_path
            && let Err(error) = self.checked(CanonicalDurabilityStage::ParentSync, || {
                destination_parent.directory.sync_all()
            })
        {
            return indeterminate(CanonicalDurabilityStage::ParentSync, &error, recovery_key);
        }

        CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
            mutation: CanonicalMutation::Rename,
            barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
        })
    }

    fn qualification_status(
        &self,
        qualification: Option<&CanonicalFilesystemQualification>,
    ) -> Result<bool, CanonicalDurabilityRejection> {
        let Some(qualification) = qualification else {
            return Ok(false);
        };
        if qualification.namespace != self.namespace {
            return Err(CanonicalDurabilityRejection::QualificationBindingMismatch);
        }
        Ok(namespace_supported_for(current_platform()))
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
        if let Err(error) = self.checked(CanonicalDurabilityStage::FileSync, || file.sync_all()) {
            return indeterminate(CanonicalDurabilityStage::FileSync, &error, recovery_key);
        }

        if let Some(parent) = parent {
            if let Err(error) =
                self.checked(CanonicalDurabilityStage::ParentSync, || parent.sync_all())
            {
                return indeterminate(CanonicalDurabilityStage::ParentSync, &error, recovery_key);
            }
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

    fn checked(
        &self,
        _stage: CanonicalDurabilityStage,
        operation: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        #[cfg(test)]
        if let Some(failure) = self.injected_failure
            && failure.stage == _stage
        {
            return Err(match failure.raw_os_error {
                Some(raw_os_error) => io::Error::from_raw_os_error(raw_os_error),
                None => io::Error::other("injected failure"),
            });
        }
        operation()
    }

    fn wait_before_atomic_create(&self) {
        #[cfg(test)]
        if let Some(barrier) = &self.before_atomic_create {
            barrier.wait();
            barrier.wait();
        }
    }
}

enum OpenedCanonicalFile {
    Existing(File),
    New { file: File, parent: File },
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

fn open_existing_regular(path: &Path) -> Result<File, CanonicalDurabilityRejection> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| durability_io(CanonicalDurabilityStage::InspectEntry, &error))?;
    if !metadata.file_type().is_file() {
        return Err(CanonicalDurabilityRejection::NonRegularCanonicalEntry);
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| durability_io(CanonicalDurabilityStage::OpenExisting, &error))
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

fn parent_directory(path: &Path) -> PathBuf {
    path.parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf()
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    #[cfg(unix)]
    use std::os::unix::fs::FileTypeExt;

    fn temp_root(label: &str) -> PathBuf {
        static NONCE: AtomicU64 = AtomicU64::new(0);
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "audio-graph-canonical-durability-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn qualification(root: &Path) -> CanonicalFilesystemQualification {
        CanonicalFilesystemQualification::for_test_root(root)
            .expect("bind test qualification to root identity")
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
    fn public_rename_syncs_the_file_and_changed_parent_namespace() {
        let root = temp_root("rename");
        let destination_parent = root.join("quarantine");
        fs::create_dir_all(&destination_parent).expect("create fixture directories");
        let source = root.join("tail.tmp");
        let destination = destination_parent.join("tail.quarantine");
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
    fn platform_namespace_contract_is_linux_macos_only() {
        assert!(namespace_supported_for(CanonicalPlatform::Linux));
        assert!(namespace_supported_for(CanonicalPlatform::MacOs));
        assert!(!namespace_supported_for(CanonicalPlatform::Windows));
        assert!(!namespace_supported_for(CanonicalPlatform::Other));
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
}
