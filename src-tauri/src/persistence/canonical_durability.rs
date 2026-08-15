//! Canonical file and namespace durability barriers.
//!
//! This dormant module names the barriers needed by later canonical writers.
//! It does not provision canonical parent directories, activate a runtime
//! writer, or claim more than the completed operating-system barriers.

use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// Opaque identity used to reconcile an operation whose durability is unknown.
///
/// The caller must supply the same value when reconciling the same logical
/// mutation. Its bytes are deliberately absent from `Debug` and there is no
/// accessor that could accidentally place them in diagnostics.
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

/// External qualification evidence supplied by the platform probe owned by a
/// later Wave 7B workstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalFilesystemQualification {
    Unqualified,
    /// A supported local Linux filesystem whose directory-sync probe passed.
    LinuxLocalDirectorySyncProven,
    /// APFS on a supported macOS host whose directory-sync probe passed.
    MacOsApfsDirectorySyncProven,
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

/// A refusal proven before canonical bytes or namespace state were mutated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalDurabilityRejection {
    ParentProvisioningRequired,
    NamespaceDurabilityUnsupported {
        platform: CanonicalPlatform,
        operation: CanonicalNamespaceOperation,
    },
    IoFailedBeforeMutation {
        stage: CanonicalDurabilityStage,
        kind: io::ErrorKind,
    },
    DestinationAlreadyExists,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CanonicalDurabilityIndeterminate {
    pub stage: CanonicalDurabilityStage,
    pub kind: io::ErrorKind,
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
    Open,
    Lock,
}

/// Content-free cooperative-lock failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalCoordinationError {
    ParentProvisioningRequired,
    Missing,
    Contended,
    Io {
        stage: CanonicalCoordinationStage,
        kind: io::ErrorKind,
    },
}

/// Exclusive guard for one stable coordination file.
///
/// The file is created only by this writer-side acquisition and is never
/// renamed or removed by the module. Dropping the guard releases the OS lock.
pub struct CanonicalExclusiveGuard {
    _lock_file: File,
}

/// Shared guard used by strict readers after a coordination file exists.
///
/// Shared acquisition never creates either the file or its parent.
pub struct CanonicalSharedGuard {
    _lock_file: File,
}

/// Deep, provider-neutral durability module. The interface returns only typed,
/// content-free outcomes; all filesystem sequencing remains internal.
#[derive(Default)]
pub struct CanonicalDurability {
    #[cfg(test)]
    injected_failure: Option<CanonicalDurabilityStage>,
}

impl CanonicalDurability {
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire the stable writer coordination file without provisioning its
    /// parent directory.
    pub fn try_lock_exclusive(
        &self,
        coordination_path: &Path,
    ) -> Result<CanonicalExclusiveGuard, CanonicalCoordinationError> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(coordination_path)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    CanonicalCoordinationError::ParentProvisioningRequired
                } else {
                    CanonicalCoordinationError::Io {
                        stage: CanonicalCoordinationStage::Open,
                        kind: error.kind(),
                    }
                }
            })?;
        try_lock_exclusive(file)
    }

    /// Acquire a shared strict-reader lock. Missing state stays missing and no
    /// directory or file is created as a side effect.
    pub fn try_lock_shared(
        &self,
        coordination_path: &Path,
    ) -> Result<CanonicalSharedGuard, CanonicalCoordinationError> {
        let file = OpenOptions::new()
            .read(true)
            .open(coordination_path)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::NotFound {
                    CanonicalCoordinationError::Missing
                } else {
                    CanonicalCoordinationError::Io {
                        stage: CanonicalCoordinationStage::Open,
                        kind: error.kind(),
                    }
                }
            })?;
        try_lock_shared(file)
    }

    /// Append bytes behind an exclusive stable coordination guard.
    ///
    /// Existing files are accepted only after buffered write, flush, and file
    /// `sync_all`. A first-created file additionally requires a qualified
    /// platform contract and successful parent-directory `sync_all`.
    pub fn append(
        &self,
        _guard: &CanonicalExclusiveGuard,
        path: &Path,
        bytes: &[u8],
        qualification: CanonicalFilesystemQualification,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        match self.open_for_append(path, qualification) {
            Ok(opened) => self.append_opened(opened, bytes, recovery_key),
            Err(rejection) => CanonicalDurabilityOutcome::Rejected(rejection),
        }
    }

    /// Publish a pre-existing synchronized file under a new unique name.
    ///
    /// The destination must not exist when checked under the cooperative lock.
    /// Uncooperative pathname races remain outside the advisory-lock contract.
    /// Windows and unqualified namespace contracts refuse before mutation.
    pub fn rename(
        &self,
        _guard: &CanonicalExclusiveGuard,
        source: &Path,
        destination: &Path,
        qualification: CanonicalFilesystemQualification,
        recovery_key: CanonicalRecoveryKey,
    ) -> CanonicalDurabilityOutcome {
        let platform = current_platform();
        let source_parent_path = parent_directory(source);
        let destination_parent_path = parent_directory(destination);

        if !namespace_is_qualified(platform, qualification) {
            if let Err(rejection) = ensure_parent_present(&source_parent_path) {
                return CanonicalDurabilityOutcome::Rejected(rejection);
            }
            if let Err(rejection) = ensure_parent_present(&destination_parent_path) {
                return CanonicalDurabilityOutcome::Rejected(rejection);
            }
            return CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform,
                    operation: CanonicalNamespaceOperation::Rename,
                },
            );
        }

        let source_file = match open_existing(source) {
            Ok(file) => file,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        if let Err(rejection) = ensure_destination_absent(destination) {
            return CanonicalDurabilityOutcome::Rejected(rejection);
        }
        let source_parent = match open_parent(&source_parent_path) {
            Ok(parent) => parent,
            Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
        };
        let destination_parent = if source_parent_path == destination_parent_path {
            None
        } else {
            match open_parent(&destination_parent_path) {
                Ok(parent) => Some(parent),
                Err(rejection) => return CanonicalDurabilityOutcome::Rejected(rejection),
            }
        };

        if let Err(error) = self.checked(CanonicalDurabilityStage::FileSync, || {
            source_file.sync_all()
        }) {
            return indeterminate(CanonicalDurabilityStage::FileSync, error, recovery_key);
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::Rename, || {
            std::fs::rename(source, destination)
        }) {
            return indeterminate(CanonicalDurabilityStage::Rename, error, recovery_key);
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::ParentSync, || {
            source_parent.sync_all()
        }) {
            return indeterminate(CanonicalDurabilityStage::ParentSync, error, recovery_key);
        }
        if let Some(destination_parent) = destination_parent
            && let Err(error) = self.checked(CanonicalDurabilityStage::ParentSync, || {
                destination_parent.sync_all()
            })
        {
            return indeterminate(CanonicalDurabilityStage::ParentSync, error, recovery_key);
        }

        CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
            mutation: CanonicalMutation::Rename,
            barrier: CanonicalDurabilityBarrier::FileAndParentNamespace,
        })
    }

    fn open_for_append(
        &self,
        path: &Path,
        qualification: CanonicalFilesystemQualification,
    ) -> Result<OpenedCanonicalFile, CanonicalDurabilityRejection> {
        let platform = current_platform();
        if namespace_is_qualified(platform, qualification) {
            let parent_path = parent_directory(path);
            let parent = open_parent(&parent_path)?;
            return match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(path)
            {
                Ok(file) => Ok(OpenedCanonicalFile::New { file, parent }),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    open_existing(path).map(OpenedCanonicalFile::Existing)
                }
                Err(error) => Err(CanonicalDurabilityRejection::IoFailedBeforeMutation {
                    stage: CanonicalDurabilityStage::CreateNew,
                    kind: error.kind(),
                }),
            };
        }

        match open_existing(path) {
            Ok(file) => Ok(OpenedCanonicalFile::Existing(file)),
            Err(CanonicalDurabilityRejection::IoFailedBeforeMutation {
                kind: io::ErrorKind::NotFound,
                ..
            }) => {
                ensure_parent_present(&parent_directory(path))?;
                Err(
                    CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                        platform,
                        operation: CanonicalNamespaceOperation::FirstCreate,
                    },
                )
            }
            Err(error) => Err(error),
        }
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
                    return before_mutation(CanonicalDurabilityStage::SeekEnd, error);
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
            return indeterminate(CanonicalDurabilityStage::Write, error, recovery_key);
        }
        if let Err(error) = self.checked(CanonicalDurabilityStage::Flush, || writer.flush()) {
            return indeterminate(CanonicalDurabilityStage::Flush, error, recovery_key);
        }
        drop(writer);
        if let Err(error) = self.checked(CanonicalDurabilityStage::FileSync, || file.sync_all()) {
            return indeterminate(CanonicalDurabilityStage::FileSync, error, recovery_key);
        }

        if let Some(parent) = parent {
            if let Err(error) =
                self.checked(CanonicalDurabilityStage::ParentSync, || parent.sync_all())
            {
                return indeterminate(CanonicalDurabilityStage::ParentSync, error, recovery_key);
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
        if self.injected_failure == Some(_stage) {
            return Err(io::Error::other("injected failure"));
        }
        operation()
    }

    #[cfg(test)]
    fn failing_at(stage: CanonicalDurabilityStage) -> Self {
        Self {
            injected_failure: Some(stage),
        }
    }
}

enum OpenedCanonicalFile {
    Existing(File),
    New { file: File, parent: File },
}

fn try_lock_exclusive(file: File) -> Result<CanonicalExclusiveGuard, CanonicalCoordinationError> {
    match file.try_lock() {
        Ok(()) => Ok(CanonicalExclusiveGuard { _lock_file: file }),
        Err(TryLockError::WouldBlock) => Err(CanonicalCoordinationError::Contended),
        Err(TryLockError::Error(error)) => Err(CanonicalCoordinationError::Io {
            stage: CanonicalCoordinationStage::Lock,
            kind: error.kind(),
        }),
    }
}

fn try_lock_shared(file: File) -> Result<CanonicalSharedGuard, CanonicalCoordinationError> {
    match file.try_lock_shared() {
        Ok(()) => Ok(CanonicalSharedGuard { _lock_file: file }),
        Err(TryLockError::WouldBlock) => Err(CanonicalCoordinationError::Contended),
        Err(TryLockError::Error(error)) => Err(CanonicalCoordinationError::Io {
            stage: CanonicalCoordinationStage::Lock,
            kind: error.kind(),
        }),
    }
}

fn open_existing(path: &Path) -> Result<File, CanonicalDurabilityRejection> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(
            |error| CanonicalDurabilityRejection::IoFailedBeforeMutation {
                stage: CanonicalDurabilityStage::OpenExisting,
                kind: error.kind(),
            },
        )
}

fn open_parent(path: &Path) -> Result<File, CanonicalDurabilityRejection> {
    File::open(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            CanonicalDurabilityRejection::ParentProvisioningRequired
        } else {
            CanonicalDurabilityRejection::IoFailedBeforeMutation {
                stage: CanonicalDurabilityStage::OpenParent,
                kind: error.kind(),
            }
        }
    })
}

fn ensure_destination_absent(path: &Path) -> Result<(), CanonicalDurabilityRejection> {
    match File::open(path) {
        Ok(_) => Err(CanonicalDurabilityRejection::DestinationAlreadyExists),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(CanonicalDurabilityRejection::IoFailedBeforeMutation {
            stage: CanonicalDurabilityStage::OpenExisting,
            kind: error.kind(),
        }),
    }
}

fn ensure_parent_present(path: &Path) -> Result<(), CanonicalDurabilityRejection> {
    std::fs::metadata(path).map(|_| ()).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            CanonicalDurabilityRejection::ParentProvisioningRequired
        } else {
            CanonicalDurabilityRejection::IoFailedBeforeMutation {
                stage: CanonicalDurabilityStage::OpenParent,
                kind: error.kind(),
            }
        }
    })
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

const fn namespace_is_qualified(
    platform: CanonicalPlatform,
    qualification: CanonicalFilesystemQualification,
) -> bool {
    matches!(
        (platform, qualification),
        (
            CanonicalPlatform::Linux,
            CanonicalFilesystemQualification::LinuxLocalDirectorySyncProven
        ) | (
            CanonicalPlatform::MacOs,
            CanonicalFilesystemQualification::MacOsApfsDirectorySyncProven
        )
    )
}

fn before_mutation(
    stage: CanonicalDurabilityStage,
    error: io::Error,
) -> CanonicalDurabilityOutcome {
    CanonicalDurabilityOutcome::Rejected(CanonicalDurabilityRejection::IoFailedBeforeMutation {
        stage,
        kind: error.kind(),
    })
}

fn indeterminate(
    stage: CanonicalDurabilityStage,
    error: io::Error,
    recovery_key: CanonicalRecoveryKey,
) -> CanonicalDurabilityOutcome {
    CanonicalDurabilityOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
        stage,
        kind: error.kind(),
        recovery_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn temp_root(label: &str) -> PathBuf {
        static NONCE: AtomicU64 = AtomicU64::new(0);
        let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "audio-graph-canonical-durability-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn public_append_classifies_existing_and_first_created_files_atomically() {
        let root = temp_root("classification");
        fs::create_dir_all(&root).expect("create fixture root");
        let coordination = root.join("canonical.lock");
        let existing = root.join("existing.log");
        let newly_created = root.join("new.log");
        fs::write(&existing, b"prefix").expect("seed existing file");

        let durability = CanonicalDurability::new();
        let guard = durability
            .try_lock_exclusive(&coordination)
            .expect("acquire stable coordination lock");
        let recovery_key = CanonicalRecoveryKey::from_opaque_bytes([7; 16]);

        let existing_outcome = durability.append(
            &guard,
            &existing,
            b"-append",
            CanonicalFilesystemQualification::Unqualified,
            recovery_key,
        );
        assert_eq!(
            existing_outcome,
            CanonicalDurabilityOutcome::Accepted(CanonicalDurabilityReceipt {
                mutation: CanonicalMutation::ExistingAppend,
                barrier: CanonicalDurabilityBarrier::FileDataAndMetadata,
            })
        );

        let new_outcome = durability.append(
            &guard,
            &newly_created,
            b"first",
            CanonicalFilesystemQualification::LinuxLocalDirectorySyncProven,
            recovery_key,
        );
        assert_eq!(
            new_outcome,
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
    fn public_rename_syncs_the_file_and_changed_parent_namespace() {
        let root = temp_root("rename");
        let destination_parent = root.join("quarantine");
        fs::create_dir_all(&destination_parent).expect("create fixture directories");
        let coordination = root.join("canonical.lock");
        let source = root.join("tail.tmp");
        let destination = destination_parent.join("tail.quarantine");
        fs::write(&source, b"recoverable tail").expect("seed rename source");

        let durability = CanonicalDurability::new();
        let guard = durability
            .try_lock_exclusive(&coordination)
            .expect("acquire stable coordination lock");
        let outcome = durability.rename(
            &guard,
            &source,
            &destination,
            CanonicalFilesystemQualification::LinuxLocalDirectorySyncProven,
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
            fs::read(destination).expect("read renamed file"),
            b"recoverable tail"
        );

        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn missing_canonical_parent_requires_external_provisioning() {
        let root = temp_root("missing-parent");
        fs::create_dir_all(&root).expect("create fixture root");
        let durability = CanonicalDurability::new();
        let guard = durability
            .try_lock_exclusive(&root.join("canonical.lock"))
            .expect("acquire stable coordination lock");
        let target = root.join("absent").join("events.log");

        let outcome = durability.append(
            &guard,
            &target,
            b"must not be written",
            CanonicalFilesystemQualification::LinuxLocalDirectorySyncProven,
            CanonicalRecoveryKey::from_opaque_bytes([1; 16]),
        );

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::ParentProvisioningRequired
            )
        );
        assert!(!target.parent().expect("target parent").exists());
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    fn unqualified_first_create_is_refused_before_mutation() {
        let root = temp_root("unqualified-create");
        fs::create_dir_all(&root).expect("create fixture root");
        let durability = CanonicalDurability::new();
        let guard = durability
            .try_lock_exclusive(&root.join("canonical.lock"))
            .expect("acquire stable coordination lock");
        let target = root.join("events.log");

        let outcome = durability.append(
            &guard,
            &target,
            b"must not be written",
            CanonicalFilesystemQualification::Unqualified,
            CanonicalRecoveryKey::from_opaque_bytes([2; 16]),
        );

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::NamespaceDurabilityUnsupported {
                    platform: current_platform(),
                    operation: CanonicalNamespaceOperation::FirstCreate,
                }
            )
        );
        assert!(!target.exists());
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    fn shared_coordination_lock_on_missing_state_is_non_mutating() {
        let root = temp_root("strict-lock-missing");
        let coordination = root.join("canonical.lock");
        let durability = CanonicalDurability::new();

        let result = durability.try_lock_shared(&coordination);

        assert!(matches!(result, Err(CanonicalCoordinationError::Missing)));
        assert!(!coordination.exists());
        assert!(!root.exists());
    }

    #[test]
    fn stable_coordination_locks_contend_share_and_release() {
        let root = temp_root("coordination-locks");
        fs::create_dir_all(&root).expect("create fixture root");
        let coordination = root.join("canonical.lock");
        let durability = CanonicalDurability::new();

        let exclusive = durability
            .try_lock_exclusive(&coordination)
            .expect("acquire exclusive lock");
        assert!(matches!(
            durability.try_lock_exclusive(&coordination),
            Err(CanonicalCoordinationError::Contended)
        ));
        assert!(matches!(
            durability.try_lock_shared(&coordination),
            Err(CanonicalCoordinationError::Contended)
        ));
        drop(exclusive);

        let shared_one = durability
            .try_lock_shared(&coordination)
            .expect("acquire first shared lock after release");
        let shared_two = durability
            .try_lock_shared(&coordination)
            .expect("acquire second shared lock");
        assert!(matches!(
            durability.try_lock_exclusive(&coordination),
            Err(CanonicalCoordinationError::Contended)
        ));
        drop(shared_two);
        drop(shared_one);

        let released = durability
            .try_lock_exclusive(&coordination)
            .expect("exclusive lock is available after shared release");
        drop(released);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    fn exclusive_coordination_lock_does_not_provision_its_parent() {
        let root = temp_root("coordination-parent");
        let coordination = root.join("canonical.lock");
        let durability = CanonicalDurability::new();

        let result = durability.try_lock_exclusive(&coordination);

        assert!(matches!(
            result,
            Err(CanonicalCoordinationError::ParentProvisioningRequired)
        ));
        assert!(!root.exists());
    }

    #[test]
    fn platform_contract_is_linux_qualified_macos_apfs_conditional_and_windows_refused() {
        assert!(namespace_is_qualified(
            CanonicalPlatform::Linux,
            CanonicalFilesystemQualification::LinuxLocalDirectorySyncProven
        ));
        assert!(!namespace_is_qualified(
            CanonicalPlatform::Linux,
            CanonicalFilesystemQualification::MacOsApfsDirectorySyncProven
        ));
        assert!(namespace_is_qualified(
            CanonicalPlatform::MacOs,
            CanonicalFilesystemQualification::MacOsApfsDirectorySyncProven
        ));
        assert!(!namespace_is_qualified(
            CanonicalPlatform::MacOs,
            CanonicalFilesystemQualification::Unqualified
        ));
        for qualification in [
            CanonicalFilesystemQualification::Unqualified,
            CanonicalFilesystemQualification::LinuxLocalDirectorySyncProven,
            CanonicalFilesystemQualification::MacOsApfsDirectorySyncProven,
        ] {
            assert!(!namespace_is_qualified(
                CanonicalPlatform::Windows,
                qualification
            ));
        }
    }

    #[test]
    fn existing_append_barrier_failures_are_indeterminate() {
        for stage in [
            CanonicalDurabilityStage::Write,
            CanonicalDurabilityStage::Flush,
            CanonicalDurabilityStage::FileSync,
        ] {
            let root = temp_root(&format!("append-failure-{stage:?}"));
            fs::create_dir_all(&root).expect("create fixture root");
            let target = root.join("events.log");
            fs::write(&target, b"prefix").expect("seed existing file");
            let durability = CanonicalDurability::failing_at(stage);
            let guard = durability
                .try_lock_exclusive(&root.join("canonical.lock"))
                .expect("acquire stable coordination lock");
            let recovery_key = CanonicalRecoveryKey::from_opaque_bytes([3; 16]);

            let outcome = durability.append(
                &guard,
                &target,
                b"possibly visible",
                CanonicalFilesystemQualification::Unqualified,
                recovery_key,
            );

            assert_eq!(
                outcome,
                CanonicalDurabilityOutcome::DurabilityIndeterminate(
                    CanonicalDurabilityIndeterminate {
                        stage,
                        kind: io::ErrorKind::Other,
                        recovery_key,
                    }
                )
            );
            drop(guard);
            fs::remove_dir_all(root).expect("clean fixture root");
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn first_create_parent_barrier_failure_is_indeterminate() {
        let root = temp_root("first-create-parent-failure");
        fs::create_dir_all(&root).expect("create fixture root");
        let target = root.join("events.log");
        let durability = CanonicalDurability::failing_at(CanonicalDurabilityStage::ParentSync);
        let guard = durability
            .try_lock_exclusive(&root.join("canonical.lock"))
            .expect("acquire stable coordination lock");
        let recovery_key = CanonicalRecoveryKey::from_opaque_bytes([4; 16]);

        let outcome = durability.append(
            &guard,
            &target,
            b"visible but not namespace accepted",
            CanonicalFilesystemQualification::LinuxLocalDirectorySyncProven,
            recovery_key,
        );

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::DurabilityIndeterminate(CanonicalDurabilityIndeterminate {
                stage: CanonicalDurabilityStage::ParentSync,
                kind: io::ErrorKind::Other,
                recovery_key,
            })
        );
        assert!(target.exists());
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn rename_failures_after_a_possible_mutation_are_indeterminate() {
        for stage in [
            CanonicalDurabilityStage::FileSync,
            CanonicalDurabilityStage::Rename,
            CanonicalDurabilityStage::ParentSync,
        ] {
            let root = temp_root(&format!("rename-failure-{stage:?}"));
            fs::create_dir_all(&root).expect("create fixture root");
            let source = root.join("source.tmp");
            let destination = root.join("destination.quarantine");
            fs::write(&source, b"recoverable").expect("seed rename source");
            let durability = CanonicalDurability::failing_at(stage);
            let guard = durability
                .try_lock_exclusive(&root.join("canonical.lock"))
                .expect("acquire stable coordination lock");
            let recovery_key = CanonicalRecoveryKey::from_opaque_bytes([5; 16]);

            let outcome = durability.rename(
                &guard,
                &source,
                &destination,
                CanonicalFilesystemQualification::LinuxLocalDirectorySyncProven,
                recovery_key,
            );

            assert_eq!(
                outcome,
                CanonicalDurabilityOutcome::DurabilityIndeterminate(
                    CanonicalDurabilityIndeterminate {
                        stage,
                        kind: io::ErrorKind::Other,
                        recovery_key,
                    }
                )
            );
            drop(guard);
            fs::remove_dir_all(root).expect("clean fixture root");
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn rename_refuses_an_existing_destination_before_mutation() {
        let root = temp_root("rename-collision");
        fs::create_dir_all(&root).expect("create fixture root");
        let source = root.join("source.tmp");
        let destination = root.join("destination.quarantine");
        fs::write(&source, b"source").expect("seed rename source");
        fs::write(&destination, b"destination").expect("seed rename destination");
        let durability = CanonicalDurability::new();
        let guard = durability
            .try_lock_exclusive(&root.join("canonical.lock"))
            .expect("acquire stable coordination lock");

        let outcome = durability.rename(
            &guard,
            &source,
            &destination,
            CanonicalFilesystemQualification::LinuxLocalDirectorySyncProven,
            CanonicalRecoveryKey::from_opaque_bytes([6; 16]),
        );

        assert_eq!(
            outcome,
            CanonicalDurabilityOutcome::Rejected(
                CanonicalDurabilityRejection::DestinationAlreadyExists
            )
        );
        assert_eq!(fs::read(source).expect("source retained"), b"source");
        assert_eq!(
            fs::read(destination).expect("destination retained"),
            b"destination"
        );
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }

    #[test]
    fn indeterminate_diagnostics_redact_path_payload_and_recovery_identity() {
        let root = temp_root("redacted-diagnostics");
        fs::create_dir_all(&root).expect("create fixture root");
        let target = root.join("private-session-name.log");
        fs::write(&target, b"prefix").expect("seed existing file");
        let durability = CanonicalDurability::failing_at(CanonicalDurabilityStage::Flush);
        let guard = durability
            .try_lock_exclusive(&root.join("canonical.lock"))
            .expect("acquire stable coordination lock");
        let secret_payload = "sensitive spoken words";
        let outcome = durability.append(
            &guard,
            &target,
            secret_payload.as_bytes(),
            CanonicalFilesystemQualification::Unqualified,
            CanonicalRecoveryKey::from_opaque_bytes([0xab; 16]),
        );

        let diagnostic = format!("{outcome:?}");
        assert!(!diagnostic.contains("private-session-name"));
        assert!(!diagnostic.contains(secret_payload));
        assert!(!diagnostic.contains("abab"));
        assert!(diagnostic.contains("[REDACTED]"));
        drop(guard);
        fs::remove_dir_all(root).expect("clean fixture root");
    }
}
