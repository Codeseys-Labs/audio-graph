use super::keyring_entry::CREDENTIAL_SERVICE;
use crate::credentials::domain::{AuthorityJournal, CredentialStoreFailure};
#[cfg(any(windows, target_os = "macos"))]
use crate::credentials::filesystem_policy::{
    FilesystemStatusCode, PersistenceTarget, evaluate_filesystem,
};
use held_directory::{HeldDirectory, PublishFailure, StagedFile, take_ownership_if_refused};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

mod held_directory;

const JOURNAL_FILE_NAME: &str = "state.json";
const NAMESPACE_LOCK_FILE_NAME: &str = "v2-authority.lock";
const AUTHORITY_ENVELOPE_SCHEMA_VERSION: u32 = 1;
pub(super) const MAX_AUTHORITY_JOURNAL_BYTES: usize = 256 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityJournalEnvelope {
    schema_version: u32,
    authority_instance_id: String,
    journal: AuthorityJournal,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityMarkerEnvelope {
    schema_version: u32,
    authority_instance_id: String,
}

pub(super) struct DecodedAuthorityJournal {
    pub(super) authority_instance_id: String,
    pub(super) journal: AuthorityJournal,
}

pub(super) fn new_authority_instance_id() -> String {
    uuid::Uuid::new_v4().hyphenated().to_string()
}

pub(super) fn encode_authority_journal(
    authority_instance_id: &str,
    journal: &AuthorityJournal,
) -> Result<Vec<u8>, CredentialStoreFailure> {
    let bytes = serde_json::to_vec(&AuthorityJournalEnvelope {
        schema_version: AUTHORITY_ENVELOPE_SCHEMA_VERSION,
        authority_instance_id: authority_instance_id.to_owned(),
        journal: journal.clone(),
    })
    .map_err(|_| CredentialStoreFailure::Internal)?;
    if bytes.len() > MAX_AUTHORITY_JOURNAL_BYTES {
        return Err(CredentialStoreFailure::CorruptRecord);
    }
    Ok(bytes)
}

pub(super) fn encode_authority_marker(
    authority_instance_id: &str,
) -> Result<Vec<u8>, CredentialStoreFailure> {
    serde_json::to_vec(&AuthorityMarkerEnvelope {
        schema_version: AUTHORITY_ENVELOPE_SCHEMA_VERSION,
        authority_instance_id: authority_instance_id.to_owned(),
    })
    .map_err(|_| CredentialStoreFailure::Internal)
}

pub(super) fn decode_authority_journal(bytes: &[u8]) -> Option<DecodedAuthorityJournal> {
    if bytes.len() > MAX_AUTHORITY_JOURNAL_BYTES {
        return None;
    }
    let envelope: AuthorityJournalEnvelope = serde_json::from_slice(bytes).ok()?;
    if envelope.schema_version != AUTHORITY_ENVELOPE_SCHEMA_VERSION
        || !valid_authority_instance_id(&envelope.authority_instance_id)
    {
        return None;
    }
    Some(DecodedAuthorityJournal {
        authority_instance_id: envelope.authority_instance_id,
        journal: envelope.journal,
    })
}

pub(super) fn decode_authority_marker(bytes: &[u8]) -> Option<String> {
    let envelope: AuthorityMarkerEnvelope = serde_json::from_slice(bytes).ok()?;
    if envelope.schema_version != AUTHORITY_ENVELOPE_SCHEMA_VERSION
        || !valid_authority_instance_id(&envelope.authority_instance_id)
    {
        return None;
    }
    Some(envelope.authority_instance_id)
}

fn valid_authority_instance_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value)
        .map(|id| id.hyphenated().to_string() == value)
        .unwrap_or(false)
}

pub(super) trait AuthorityMutationLock: Send {}

/// Every method but `acquire_mutation_lock` runs with the mutation lock
/// held.
pub(super) trait AuthorityJournalBoundary: Send + Sync {
    fn read(&self) -> Result<Option<Vec<u8>>, CredentialStoreFailure>;
    /// `Ok` means these exact bytes were read back from the place they were
    /// published, so the caller need not read them again.
    fn replace(&self, bytes: &[u8]) -> Result<(), CredentialStoreFailure>;
    /// Removes what an earlier replace failed to discard, or fails closed
    /// when it cannot. Run when a store opens.
    fn remove_stray_staged(&self) -> Result<(), CredentialStoreFailure>;
    fn acquire_mutation_lock(
        &self,
    ) -> Result<Box<dyn AuthorityMutationLock>, CredentialStoreFailure>;
}

/// One staged journal replacement. Bytes become visible only through
/// `commit`; dropping an uncommitted writer discards them.
trait AtomicJournalWriter {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    /// A failure says whether the previous journal provably still stands.
    fn commit(self: Box<Self>) -> Result<(), PublishFailure>;
}

/// Journal file operations relative to an already held and hardened root.
trait AtomicJournalFileSystem: Send + Sync {
    fn open_atomic<'directory>(
        &self,
        directory: &'directory HeldDirectory,
        name: &str,
    ) -> Result<Box<dyn AtomicJournalWriter + 'directory>, CredentialStoreFailure>;
    /// Reads at most `limit` bytes; `None` when the file is absent.
    fn read(
        &self,
        directory: &HeldDirectory,
        name: &str,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, CredentialStoreFailure>;
}

struct SystemAtomicJournalFileSystem;

impl AtomicJournalWriter for StagedFile<'_> {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        StagedFile::write_all(self, bytes)
    }

    fn commit(self: Box<Self>) -> Result<(), PublishFailure> {
        StagedFile::commit(*self)
    }
}

impl AtomicJournalFileSystem for SystemAtomicJournalFileSystem {
    fn open_atomic<'directory>(
        &self,
        directory: &'directory HeldDirectory,
        name: &str,
    ) -> Result<Box<dyn AtomicJournalWriter + 'directory>, CredentialStoreFailure> {
        Ok(Box::new(directory.create_staged(name)?))
    }

    fn read(
        &self,
        directory: &HeldDirectory,
        name: &str,
        limit: usize,
    ) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
        directory.read_capped(name, limit)
    }
}

/// The per-user directory that names the native credential namespace.
///
/// It depends only on the OS user and the fixed native service name, never on
/// the journal root or the process environment. Every process that addresses
/// the same native targets therefore contends on one mutation lock, whichever
/// journal root it was given.
fn canonical_namespace_lock_root() -> Result<PathBuf, CredentialStoreFailure> {
    per_user_base().map(|base| base.join(CREDENTIAL_SERVICE))
}

/// The default `LocalAppData` location under the profile registered for the
/// process token's user. `SHGetKnownFolderPath` would expand `%USERPROFILE%`
/// from the process environment, so two processes of one user could name
/// different locks; only the token decides this path.
#[cfg(windows)]
fn per_user_base() -> Result<PathBuf, CredentialStoreFailure> {
    held_directory::token_profile_directory()
        .map(|profile| profile.join("AppData").join("Local"))
        .ok_or(CredentialStoreFailure::Unavailable)
}

/// The passwd home of the effective user, which ignores `HOME`.
#[cfg(target_os = "macos")]
fn per_user_base() -> Result<PathBuf, CredentialStoreFailure> {
    let user = nix::unistd::User::from_uid(nix::unistd::Uid::effective())
        .map_err(|_| CredentialStoreFailure::Unavailable)?
        .ok_or(CredentialStoreFailure::Unavailable)?;
    Ok(user.dir.join("Library").join("Application Support"))
}

#[cfg(not(any(windows, target_os = "macos")))]
fn per_user_base() -> Result<PathBuf, CredentialStoreFailure> {
    Err(CredentialStoreFailure::Unsupported)
}

/// Whether a held directory may carry authority state. Decided on the held
/// handle itself, before the lock file or any journal byte exists in it.
trait RootQualification {
    fn qualify(&self, root: &HeldDirectory) -> Result<(), CredentialStoreFailure>;
}

/// The production rule: the platform detector's observation of the held
/// directory must match a compiled journal evidence profile
/// (`filesystem_policy`). Both profile tables are empty until release
/// evidence lands (#210), so this refuses every root today.
struct CompiledEvidence;

/// No operating-system release probe exists yet for the detectors that take
/// one (Windows, macOS). Release 0 is below every profile's minimum, so a
/// profile cannot match until the probe lands with the first profile.
#[cfg(any(windows, target_os = "macos"))]
const UNPROBED_PLATFORM_RELEASE: crate::credentials::filesystem_policy::PlatformRelease =
    crate::credentials::filesystem_policy::PlatformRelease::new(0, 0, 0);

impl RootQualification for CompiledEvidence {
    #[cfg(any(windows, target_os = "macos"))]
    fn qualify(&self, root: &HeldDirectory) -> Result<(), CredentialStoreFailure> {
        match evaluate_filesystem(PersistenceTarget::Journal, inspect_held_root(root)).code {
            FilesystemStatusCode::Supported => Ok(()),
            FilesystemStatusCode::TargetUnavailable
            | FilesystemStatusCode::InspectionUnavailable
            | FilesystemStatusCode::TargetChanged => Err(CredentialStoreFailure::Unavailable),
            FilesystemStatusCode::ReadOnly
            | FilesystemStatusCode::Remote
            | FilesystemStatusCode::UserspaceFilesystem
            | FilesystemStatusCode::CloudManaged
            | FilesystemStatusCode::RemovableOrHotplug
            | FilesystemStatusCode::FilesystemUnproved
            | FilesystemStatusCode::AccessControlUnproved
            | FilesystemStatusCode::DurabilityUnproved
            | FilesystemStatusCode::ConfidentialityUnproved => {
                Err(CredentialStoreFailure::Unsupported)
            }
        }
    }

    /// No journal root exists on these platforms (`per_user_base`).
    #[cfg(not(any(windows, target_os = "macos")))]
    fn qualify(&self, _root: &HeldDirectory) -> Result<(), CredentialStoreFailure> {
        Err(CredentialStoreFailure::Unsupported)
    }
}

#[cfg(windows)]
fn inspect_held_root(
    root: &HeldDirectory,
) -> Result<
    crate::credentials::filesystem_policy::FilesystemObservation,
    crate::credentials::filesystem_policy::DetectorFault,
> {
    use crate::credentials::filesystem_policy::windows::{
        WindowsFilesystemDetector, WindowsHeldDirectory,
    };
    use crate::credentials::filesystem_policy::{DetectorFault, FilesystemDetector};
    let handle = root
        .duplicate_handle()
        .map_err(|_| DetectorFault::TargetUnavailable)?;
    WindowsFilesystemDetector::new(UNPROBED_PLATFORM_RELEASE)
        .inspect(&WindowsHeldDirectory::from_held_handle(handle))
}

#[cfg(target_os = "macos")]
fn inspect_held_root(
    root: &HeldDirectory,
) -> Result<
    crate::credentials::filesystem_policy::FilesystemObservation,
    crate::credentials::filesystem_policy::DetectorFault,
> {
    use crate::credentials::filesystem_policy::macos::{HeldMacOsTarget, MacOsFilesystemDetector};
    use crate::credentials::filesystem_policy::{DetectorFault, FilesystemDetector};
    let descriptor = root
        .duplicate_descriptor()
        .map_err(|_| DetectorFault::TargetUnavailable)?;
    MacOsFilesystemDetector::new(UNPROBED_PLATFORM_RELEASE)
        .inspect(&HeldMacOsTarget::from_held_descriptor(descriptor))
}

/// The journal and the namespace lock, each in a directory held open for the
/// boundary's lifetime. The paths are resolved once, when the boundary
/// opens; every later read, publication, sweep and lock goes through the
/// held handles, so a path whose ancestors change afterwards redirects
/// nothing. Each lock acquisition checks that both paths still resolve to
/// the held directories.
pub(super) struct FileAuthorityJournalBoundary {
    root: HeldDirectory,
    root_path: PathBuf,
    namespace_lock_root: HeldDirectory,
    namespace_lock_path: PathBuf,
    file_system: Arc<dyn AtomicJournalFileSystem>,
}

impl FileAuthorityJournalBoundary {
    /// Holds the caller-selected journal root and the canonical per-user
    /// namespace lock root, creating either one if it is missing, and
    /// requires each to qualify before the lock file or any journal byte
    /// exists and before the store does any native I/O.
    pub(super) fn open_qualified(root: impl Into<PathBuf>) -> Result<Self, CredentialStoreFailure> {
        Self::open(
            root.into(),
            canonical_namespace_lock_root()?,
            &CompiledEvidence,
            Arc::new(SystemAtomicJournalFileSystem),
        )
    }

    fn open(
        root_path: PathBuf,
        namespace_lock_path: PathBuf,
        qualification: &dyn RootQualification,
        file_system: Arc<dyn AtomicJournalFileSystem>,
    ) -> Result<Self, CredentialStoreFailure> {
        let namespace_lock_root = HeldDirectory::open_or_create(&namespace_lock_path)?;
        qualification.qualify(&namespace_lock_root)?;
        let root = HeldDirectory::open_or_create(&root_path)?;
        qualification.qualify(&root)?;
        Ok(Self {
            root,
            root_path,
            namespace_lock_root,
            namespace_lock_path,
            file_system,
        })
    }

    /// A boundary over `root` and `namespace_lock_root` that accepts both
    /// without the compiled evidence, which no test directory can meet.
    #[cfg(test)]
    pub(super) fn open_for_test(
        root: PathBuf,
        namespace_lock_root: PathBuf,
    ) -> Result<Self, CredentialStoreFailure> {
        Self::with_file_system(
            root,
            namespace_lock_root,
            Arc::new(SystemAtomicJournalFileSystem),
        )
    }

    #[cfg(test)]
    fn with_file_system(
        root: PathBuf,
        namespace_lock_root: PathBuf,
        file_system: Arc<dyn AtomicJournalFileSystem>,
    ) -> Result<Self, CredentialStoreFailure> {
        Self::open(
            root,
            namespace_lock_root,
            &tests::AcceptEveryRoot,
            file_system,
        )
    }
}

/// Explicit, user-invoked repair of journal roots that hardening refused
/// because another principal owns them or a file in them: the canonical
/// namespace lock root with its lock file, and `root` with its journal.
/// Where the platform lets this user take ownership (Windows, given
/// `WRITE_OWNER`), it does, then hardens. Nothing is read, written or
/// published, and no lock is taken: the lock file may be the refused object.
/// Returns whether any owner changed.
pub(super) fn repair_root_ownership(
    root: impl Into<PathBuf>,
) -> Result<bool, CredentialStoreFailure> {
    let lock_root = take_ownership_if_refused(
        &canonical_namespace_lock_root()?,
        &[NAMESPACE_LOCK_FILE_NAME],
    )?;
    let journal_root = take_ownership_if_refused(&root.into(), &[JOURNAL_FILE_NAME])?;
    Ok(lock_root || journal_root)
}

struct FileAuthorityMutationLock {
    file: File,
}

impl AuthorityMutationLock for FileAuthorityMutationLock {}

impl Drop for FileAuthorityMutationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl AuthorityJournalBoundary for FileAuthorityJournalBoundary {
    fn read(&self) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
        // One byte past the ceiling is enough for the decoder to refuse an
        // oversized journal as recovery-required.
        self.file_system.read(
            &self.root,
            JOURNAL_FILE_NAME,
            MAX_AUTHORITY_JOURNAL_BYTES + 1,
        )
    }

    /// A publication that provably left the previous journal in place
    /// latches nothing. Another program holding the journal open without
    /// sharing delete refuses the rename with `OperationInProgress`, which a
    /// caller that has changed nothing native can retry once it lets go; any
    /// other definite refusal is `Unavailable`. Only an outcome the
    /// publication cannot rule out is `CommitUnknown`.
    fn replace(&self, bytes: &[u8]) -> Result<(), CredentialStoreFailure> {
        // The staged file is hardened before any byte is written; a
        // hardening failure stops here as a definite failure.
        let mut file = self
            .file_system
            .open_atomic(&self.root, JOURNAL_FILE_NAME)?;
        file.write_all(bytes)
            .map_err(|_| CredentialStoreFailure::Unavailable)?;
        file.commit().map_err(|failure| match failure {
            PublishFailure::Contended => CredentialStoreFailure::OperationInProgress,
            PublishFailure::NotPublished => CredentialStoreFailure::Unavailable,
            PublishFailure::Unknown => CredentialStoreFailure::CommitUnknown,
        })?;
        match self.file_system.read(
            &self.root,
            JOURNAL_FILE_NAME,
            MAX_AUTHORITY_JOURNAL_BYTES + 1,
        ) {
            Ok(Some(readback)) if readback == bytes => Ok(()),
            Ok(_) | Err(_) => Err(CredentialStoreFailure::CommitUnknown),
        }
    }

    fn remove_stray_staged(&self) -> Result<(), CredentialStoreFailure> {
        self.root.sweep_staged(JOURNAL_FILE_NAME)
    }

    /// The lock counts only while the lock file's name still designates the
    /// file that was locked and both held roots are still what their paths
    /// resolve to. Otherwise another process opening those paths now would
    /// lock a second file, or use another journal, and both would proceed.
    /// A held root that moved is a closed `Unavailable`: only a reopen, which
    /// resolves the paths again, can reach the directories other processes
    /// use. A root moved after this check, while the lock is held, is not
    /// detected until the next acquisition.
    fn acquire_mutation_lock(
        &self,
    ) -> Result<Box<dyn AuthorityMutationLock>, CredentialStoreFailure> {
        let file = self
            .namespace_lock_root
            .open_lock_file(NAMESPACE_LOCK_FILE_NAME)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(CredentialStoreFailure::OperationInProgress);
            }
            Err(std::fs::TryLockError::Error(_)) => {
                return Err(CredentialStoreFailure::Unavailable);
            }
        }
        let lock = FileAuthorityMutationLock { file };
        if self
            .namespace_lock_root
            .names_file(NAMESPACE_LOCK_FILE_NAME, &lock.file)?
            && self.namespace_lock_root.is_at(&self.namespace_lock_path)?
            && self.root.is_at(&self.root_path)?
        {
            Ok(Box::new(lock))
        } else {
            Err(CredentialStoreFailure::Unavailable)
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::{
        AtomicJournalFileSystem, AtomicJournalWriter, AuthorityJournalBoundary,
        FileAuthorityJournalBoundary, HeldDirectory, JOURNAL_FILE_NAME,
        MAX_AUTHORITY_JOURNAL_BYTES, NAMESPACE_LOCK_FILE_NAME, PublishFailure, RootQualification,
        decode_authority_journal, encode_authority_journal,
    };
    use crate::credentials::domain::{AuthorityJournal, CredentialStoreFailure};
    use audio_graph_ipc_contract::credential_contract::CredentialBackendKind;
    use std::io::{BufRead, Read, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    #[cfg(windows)]
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    const CHILD_MODE_ENV: &str = "AUDIO_GRAPH_FB2B_LOCK_CHILD_MODE";
    const CHILD_ROOT_ENV: &str = "AUDIO_GRAPH_FB2B_LOCK_CHILD_ROOT";
    const CHILD_NAMESPACE_ENV: &str = "AUDIO_GRAPH_FB2B_LOCK_CHILD_NAMESPACE";
    #[cfg(any(windows, target_os = "macos"))]
    const CHILD_EXPECTED_ENV: &str = "AUDIO_GRAPH_FB2B_LOCK_CHILD_EXPECTED";

    /// Accepts every root: test directories cannot meet the compiled
    /// evidence, which no profile satisfies yet.
    pub(super) struct AcceptEveryRoot;

    impl RootQualification for AcceptEveryRoot {
        fn qualify(&self, _root: &HeldDirectory) -> Result<(), CredentialStoreFailure> {
            Ok(())
        }
    }

    fn scratch(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "audio-graph-journal-{label}-{}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    /// A journal root and a namespace lock directory side by side in `base`.
    fn boundary(
        base: &Path,
        root: &str,
        namespace: &str,
    ) -> Result<FileAuthorityJournalBoundary, CredentialStoreFailure> {
        FileAuthorityJournalBoundary::open_for_test(base.join(root), base.join(namespace))
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum AtomicFileCut {
        None,
        Open,
        Write,
        CommitRefused,
        CommitContended,
        Commit,
        Readback,
        Mismatch,
    }

    struct ScriptedAtomicFileSystem {
        cut: AtomicFileCut,
        published: Arc<Mutex<Option<Vec<u8>>>>,
    }

    impl ScriptedAtomicFileSystem {
        fn new(cut: AtomicFileCut, initial: Option<&[u8]>) -> Self {
            Self {
                cut,
                published: Arc::new(Mutex::new(initial.map(<[u8]>::to_vec))),
            }
        }

        fn published(&self) -> Option<Vec<u8>> {
            self.published
                .lock()
                .expect("scripted publication lock")
                .clone()
        }
    }

    struct ScriptedAtomicWriter {
        cut: AtomicFileCut,
        pending: Vec<u8>,
        published: Arc<Mutex<Option<Vec<u8>>>>,
    }

    impl AtomicJournalWriter for ScriptedAtomicWriter {
        fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            if self.cut == AtomicFileCut::Write {
                return Err(std::io::Error::other("scripted prepublication write cut"));
            }
            self.pending.extend_from_slice(bytes);
            Ok(())
        }

        fn commit(self: Box<Self>) -> Result<(), PublishFailure> {
            match self.cut {
                AtomicFileCut::CommitRefused => return Err(PublishFailure::NotPublished),
                AtomicFileCut::CommitContended => return Err(PublishFailure::Contended),
                AtomicFileCut::Commit => return Err(PublishFailure::Unknown),
                _ => {}
            }
            let mut bytes = self.pending;
            if self.cut == AtomicFileCut::Mismatch {
                bytes.push(b'!');
            }
            *self.published.lock().expect("scripted publication lock") = Some(bytes);
            Ok(())
        }
    }

    impl AtomicJournalFileSystem for ScriptedAtomicFileSystem {
        fn open_atomic<'directory>(
            &self,
            _directory: &'directory HeldDirectory,
            _name: &str,
        ) -> Result<Box<dyn AtomicJournalWriter + 'directory>, CredentialStoreFailure> {
            if self.cut == AtomicFileCut::Open {
                return Err(CredentialStoreFailure::Unavailable);
            }
            Ok(Box::new(ScriptedAtomicWriter {
                cut: self.cut,
                pending: Vec::new(),
                published: self.published.clone(),
            }))
        }

        fn read(
            &self,
            _directory: &HeldDirectory,
            _name: &str,
            limit: usize,
        ) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
            if self.cut == AtomicFileCut::Readback {
                return Err(CredentialStoreFailure::Unavailable);
            }
            Ok(self
                .published()
                .map(|bytes| bytes.into_iter().take(limit).collect()))
        }
    }

    fn scripted_boundary(
        base: &Path,
        file_system: Arc<ScriptedAtomicFileSystem>,
    ) -> FileAuthorityJournalBoundary {
        FileAuthorityJournalBoundary::with_file_system(
            base.join("journal"),
            base.join("namespace"),
            file_system,
        )
        .expect("open scripted boundary")
    }

    #[test]
    fn journal_byte_ceiling_is_enforced_before_whitespace_tolerant_deserialization() {
        let mut bytes = encode_authority_journal(
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            &AuthorityJournal::new(CredentialBackendKind::Native),
        )
        .expect("encode supported journal");
        assert!(bytes.len() < MAX_AUTHORITY_JOURNAL_BYTES);
        bytes.resize(MAX_AUTHORITY_JOURNAL_BYTES, b' ');
        assert!(decode_authority_journal(&bytes).is_some());

        bytes.push(b' ');
        assert!(decode_authority_journal(&bytes).is_none());
    }

    #[test]
    fn unknown_journal_fields_are_rejected_instead_of_silently_authorized() {
        let bytes = encode_authority_journal(
            "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee",
            &AuthorityJournal::new(CredentialBackendKind::Native),
        )
        .expect("encode supported journal");
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("journal JSON");
        value["journal"]["future_authority_field"] = serde_json::json!(true);

        assert!(
            decode_authority_journal(&serde_json::to_vec(&value).expect("mutated journal JSON"))
                .is_none()
        );

        let mut value: serde_json::Value = serde_json::from_slice(&bytes).expect("journal JSON");
        value["journal"]["backend"]["future_backend_field"] = serde_json::json!(true);
        assert!(
            decode_authority_journal(&serde_json::to_vec(&value).expect("mutated backend JSON"))
                .is_none()
        );

        fn unknown_set(value: &mut serde_json::Value) {
            value["journal"]["sets"][0]["future_set_field"] = serde_json::json!(true);
        }

        fn unknown_intent(value: &mut serde_json::Value) {
            value["journal"]["pending_intents"] = serde_json::json!([{
                "operation_id": "10000000-0000-4000-8000-000000000001",
                "idempotency_token": "20000000-0000-4000-8000-000000000001",
                "set_id": "openai",
                "mutation_kind": "replace",
                "expected_revision": null,
                "proposed_revision": "30000000-0000-4000-8000-000000000001",
                "recovery_state": "pending_intent",
                "future_intent_field": true
            }]);
        }

        fn unknown_activation(value: &mut serde_json::Value) {
            value["journal"]["pending_activation"] = serde_json::json!({
                "operation_id": "10000000-0000-4000-8000-000000000001",
                "idempotency_token": "20000000-0000-4000-8000-000000000001",
                "set_id": "openai",
                "auth_method_id": "api_key",
                "expected_revision": null,
                "proposed_revision": "30000000-0000-4000-8000-000000000001",
                "expected_settings_revision": 10,
                "proposed_settings_revision": 11,
                "stage": "staged",
                "future_activation_field": true
            });
        }

        fn unknown_history_entry(value: &mut serde_json::Value) {
            value["journal"]["idempotency_history"] = serde_json::json!([{
                "idempotency_token": "20000000-0000-4000-8000-000000000001",
                "set_id": "openai",
                "mutation_kind": "replace",
                "expected_revision": null,
                "receipt": {
                    "operation_id": "10000000-0000-4000-8000-000000000001",
                    "idempotency_token": "20000000-0000-4000-8000-000000000001",
                    "set_id": "openai",
                    "previous_revision": null,
                    "new_revision": "30000000-0000-4000-8000-000000000001",
                    "result_code": "created",
                    "recovery_action": "none"
                },
                "future_history_field": true
            }]);
        }

        fn unknown_receipt(value: &mut serde_json::Value) {
            unknown_history_entry(value);
            let entry = &mut value["journal"]["idempotency_history"][0];
            entry
                .as_object_mut()
                .expect("history entry object")
                .remove("future_history_field");
            entry["receipt"]["future_receipt_field"] = serde_json::json!(true);
        }

        for (name, mutate) in [
            ("set", unknown_set as fn(&mut serde_json::Value)),
            ("intent", unknown_intent),
            ("activation", unknown_activation),
            ("history", unknown_history_entry),
            ("receipt", unknown_receipt),
        ] {
            let mut value: serde_json::Value =
                serde_json::from_slice(&bytes).expect("journal JSON");
            mutate(&mut value);
            assert!(
                decode_authority_journal(&serde_json::to_vec(&value).expect("mutated nested JSON"))
                    .is_none(),
                "accepted unknown {name} field"
            );
        }
    }

    #[test]
    fn atomic_journal_stage_mapping_preserves_only_provable_prepublication_failures() {
        let base = scratch("stage-map");
        let replacement = b"replacement-journal";

        for (cut, expected) in [
            (AtomicFileCut::Open, CredentialStoreFailure::Unavailable),
            (AtomicFileCut::Write, CredentialStoreFailure::Unavailable),
            (
                AtomicFileCut::CommitRefused,
                CredentialStoreFailure::Unavailable,
            ),
            (
                AtomicFileCut::CommitContended,
                CredentialStoreFailure::OperationInProgress,
            ),
            (AtomicFileCut::Commit, CredentialStoreFailure::CommitUnknown),
            (
                AtomicFileCut::Readback,
                CredentialStoreFailure::CommitUnknown,
            ),
            (
                AtomicFileCut::Mismatch,
                CredentialStoreFailure::CommitUnknown,
            ),
        ] {
            let file_system = Arc::new(ScriptedAtomicFileSystem::new(cut, Some(b"old-journal")));
            let boundary = scripted_boundary(&base.join(format!("{cut:?}")), file_system.clone());

            assert_eq!(boundary.replace(replacement), Err(expected), "cut {cut:?}");
            if matches!(
                cut,
                AtomicFileCut::Open
                    | AtomicFileCut::Write
                    | AtomicFileCut::CommitRefused
                    | AtomicFileCut::CommitContended
                    | AtomicFileCut::Commit
            ) {
                assert_eq!(
                    file_system.published().as_deref(),
                    Some(b"old-journal".as_slice())
                );
            }
        }

        let file_system = Arc::new(ScriptedAtomicFileSystem::new(AtomicFileCut::None, None));
        let boundary = scripted_boundary(&base.join("success"), file_system.clone());
        assert_eq!(boundary.replace(replacement), Ok(()));
        assert_eq!(
            file_system.published().as_deref(),
            Some(replacement.as_slice())
        );

        drop(boundary);
        std::fs::remove_dir_all(&base).expect("remove isolated stage-map test directory");
    }

    #[test]
    fn ordinary_journal_read_failure_remains_unavailable() {
        let base = scratch("read-failure");
        std::fs::create_dir_all(base.join("journal")).expect("create journal root");
        let file_system = Arc::new(ScriptedAtomicFileSystem::new(AtomicFileCut::Readback, None));
        let boundary = scripted_boundary(&base, file_system);

        assert_eq!(boundary.read(), Err(CredentialStoreFailure::Unavailable));
        drop(boundary);
        std::fs::remove_dir_all(&base).expect("remove isolated read-failure test directory");
    }

    /// Opening creates the missing roots and their missing ancestors, holds
    /// both roots owner-only, and finds no journal; nothing else is created.
    #[test]
    fn opening_creates_and_holds_both_roots_and_reads_no_journal() {
        let base = scratch("fresh-roots");
        let journal = boundary(&base, "nested/journal", "nested/namespace").expect("open boundary");

        assert_eq!(journal.read(), Ok(None));
        for root in ["journal", "namespace"] {
            let root = base.join("nested").join(root);
            assert!(root.is_dir(), "root missing");
            assert_eq!(
                std::fs::read_dir(&root).expect("list root").count(),
                0,
                "opening wrote into a root"
            );
            #[cfg(unix)]
            assert_eq!(unix_mode(&root), 0o700);
        }

        drop(journal);
        std::fs::remove_dir_all(&base).expect("remove isolated fresh-root directory");
    }

    /// Production opening qualifies each held root before the lock file or
    /// a journal exists in it, and the compiled evidence refuses every root
    /// until release evidence lands.
    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn the_compiled_evidence_refuses_a_root_before_any_lock_or_journal_exists() {
        let base = scratch("unqualified");
        let opened = FileAuthorityJournalBoundary::open(
            base.join("journal"),
            base.join("namespace"),
            &super::CompiledEvidence,
            Arc::new(super::SystemAtomicJournalFileSystem),
        );

        assert!(
            matches!(
                opened,
                Err(CredentialStoreFailure::Unsupported | CredentialStoreFailure::Unavailable)
            ),
            "an unqualified root was accepted"
        );
        assert!(
            !base
                .join("namespace")
                .join(NAMESPACE_LOCK_FILE_NAME)
                .exists()
        );
        assert!(!base.join("journal").exists());
        std::fs::remove_dir_all(&base).expect("remove isolated qualification directory");
    }

    #[test]
    fn oversized_journal_is_read_only_to_the_ceiling_and_refused_by_the_decoder() {
        let base = scratch("oversized");
        let root = base.join("journal");
        std::fs::create_dir_all(&root).expect("create journal root");
        std::fs::write(
            root.join(JOURNAL_FILE_NAME),
            vec![b' '; MAX_AUTHORITY_JOURNAL_BYTES * 4],
        )
        .expect("write oversized journal");

        let bytes = boundary(&base, "journal", "namespace")
            .expect("open boundary")
            .read()
            .expect("read oversized journal")
            .expect("oversized journal present");
        assert_eq!(bytes.len(), MAX_AUTHORITY_JOURNAL_BYTES + 1);
        assert!(decode_authority_journal(&bytes).is_none());

        std::fs::remove_dir_all(&base).expect("remove isolated oversized test directory");
    }

    #[test]
    fn system_replace_publishes_exact_bytes_and_leaves_no_staged_file() {
        let base = scratch("replace");
        let journal = boundary(&base, "journal", "namespace").expect("open boundary");
        journal.replace(b"first-journal").expect("first replace");
        journal.replace(b"second-journal").expect("second replace");

        assert_eq!(journal.read(), Ok(Some(b"second-journal".to_vec())));
        let names: Vec<_> = std::fs::read_dir(base.join("journal"))
            .expect("list journal root")
            .map(|entry| entry.expect("journal root entry").file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from(JOURNAL_FILE_NAME)]);

        drop(journal);
        std::fs::remove_dir_all(&base).expect("remove isolated replace test directory");
    }

    /// Another program reading the journal (a scanner, an indexer) neither
    /// blocks a publication nor sees it change under its handle when it
    /// shares delete. One that refuses to share delete makes the publication
    /// a definite, retryable `OperationInProgress` that leaves the previous
    /// journal in place, and the next publication succeeds once it lets go.
    #[cfg(windows)]
    #[test]
    fn windows_a_journal_another_program_holds_open_is_replaced_or_definitely_refused() {
        use ::windows::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        use std::os::windows::fs::OpenOptionsExt;

        let base = scratch("windows-open-target");
        let journal = boundary(&base, "journal", "namespace").expect("open boundary");
        journal.replace(b"first-journal").expect("first replace");
        let path = base.join("journal").join(JOURNAL_FILE_NAME);
        let open_with = |share: u32| {
            std::fs::OpenOptions::new()
                .read(true)
                .share_mode(share)
                .open(&path)
                .expect("open the journal as another program")
        };

        let mut sharing = open_with(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
        assert_eq!(journal.replace(b"second-journal"), Ok(()));
        let mut seen = Vec::new();
        sharing
            .read_to_end(&mut seen)
            .expect("read through the other handle");
        assert_eq!(seen, b"first-journal");
        assert_eq!(journal.read(), Ok(Some(b"second-journal".to_vec())));
        drop(sharing);

        let refusing = open_with(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0);
        assert_eq!(
            journal.replace(b"third-journal"),
            Err(CredentialStoreFailure::OperationInProgress)
        );
        assert_eq!(journal.read(), Ok(Some(b"second-journal".to_vec())));
        assert!(staged_leftovers(&base.join("journal")).is_empty());
        drop(refusing);
        assert_eq!(journal.replace(b"third-journal"), Ok(()));
        assert_eq!(journal.read(), Ok(Some(b"third-journal".to_vec())));

        drop(journal);
        std::fs::remove_dir_all(&base).expect("remove isolated open-target directory");
    }

    /// While a process holds the namespace lock, its lock file cannot be
    /// removed or replaced by another file, so a later opener of the name
    /// contends on the same file.
    #[cfg(windows)]
    #[test]
    fn windows_a_held_lock_file_cannot_be_removed_or_replaced() {
        let base = scratch("windows-lock-identity");
        let journal = boundary(&base, "journal", "namespace").expect("open boundary");
        let lock = base.join("namespace").join(NAMESPACE_LOCK_FILE_NAME);
        let decoy = base.join("decoy.lock");
        std::fs::write(&decoy, b"").expect("create decoy lock file");

        let guard = journal.acquire_mutation_lock().expect("acquire lock");
        assert!(
            std::fs::remove_file(&lock).is_err(),
            "held lock file removed"
        );
        assert!(
            std::fs::rename(&decoy, &lock).is_err(),
            "held lock file replaced"
        );
        assert!(matches!(
            journal.acquire_mutation_lock(),
            Err(CredentialStoreFailure::OperationInProgress)
        ));

        drop(guard);
        drop(journal);
        std::fs::remove_dir_all(&base).expect("remove isolated lock-identity directory");
    }

    /// The roots are resolved once: a root moved away after opening keeps
    /// receiving the journal, and a directory created at its old path gets
    /// nothing.
    #[cfg(unix)]
    #[test]
    fn unix_a_root_moved_after_opening_keeps_its_journal() {
        let base = scratch("moved-root");
        let journal = boundary(&base, "journal", "namespace").expect("open boundary");
        std::fs::rename(base.join("journal"), base.join("moved")).expect("move the held root");
        std::fs::create_dir(base.join("journal")).expect("create a decoy at the old path");

        journal.replace(b"journal").expect("replace after the move");

        assert_eq!(
            std::fs::read(base.join("moved").join(JOURNAL_FILE_NAME)).expect("read held journal"),
            b"journal"
        );
        assert_eq!(
            std::fs::read_dir(base.join("journal"))
                .expect("list decoy")
                .count(),
            0
        );
        drop(journal);
        std::fs::remove_dir_all(&base).expect("remove isolated moved-root directory");
    }

    /// A lock taken through a held root that is no longer at its path is
    /// not the lock another process opening that path now takes, so it is
    /// refused. Moving either root and putting a new directory at its path
    /// leaves the first boundary unable to lock, and the boundary opened
    /// over the replacement locks as usual.
    #[cfg(unix)]
    #[test]
    fn unix_a_moved_root_refuses_a_lock_another_opener_would_not_share() {
        for moved in ["namespace", "journal"] {
            let base = scratch("moved-lock-root");
            let first = boundary(&base, "journal", "namespace").expect("open the first boundary");
            std::fs::rename(base.join(moved), base.join("moved")).expect("move a held root");
            let second =
                boundary(&base, "journal", "namespace").expect("open over the replacement");

            assert!(
                matches!(
                    first.acquire_mutation_lock(),
                    Err(CredentialStoreFailure::Unavailable)
                ),
                "a moved {moved} root still locked"
            );
            drop(
                second
                    .acquire_mutation_lock()
                    .expect("the replacement's boundary locks"),
            );
            drop((first, second));
            std::fs::remove_dir_all(&base).expect("remove isolated moved-lock directory");
        }
    }

    /// A journal with another name is refused before its mode changes, so
    /// hardening never reaches the file under its other name.
    #[cfg(unix)]
    #[test]
    fn unix_a_hard_linked_journal_is_refused_before_its_mode_changes() {
        use std::os::unix::fs::PermissionsExt;

        let base = scratch("hard-link");
        let journal = boundary(&base, "journal", "namespace").expect("open boundary");
        let outside = base.join("outside");
        std::fs::write(&outside, b"outside").expect("write the other name");
        std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644))
            .expect("set the other name's mode");
        std::fs::hard_link(&outside, base.join("journal").join(JOURNAL_FILE_NAME))
            .expect("link the journal name");

        assert_eq!(
            journal.read(),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        );
        assert_eq!(unix_mode(&outside), 0o644);
        drop(journal);
        std::fs::remove_dir_all(&base).expect("remove isolated hard-link directory");
    }

    /// Each barrier of a publication can fail on its own. A failed sync of
    /// the staged bytes is a definite failure. A rename whose outcome is
    /// unknown, or a failed sync after the rename, is `CommitUnknown`, and
    /// the journal then holds the previous or the new bytes, never neither:
    /// a staged file that may be the published journal is never deleted.
    /// Whatever staged file is left behind goes in the next sweep.
    #[test]
    fn each_publication_barrier_failure_leaves_one_whole_journal_and_a_sweepable_root() {
        use super::held_directory::faults::{Fault, arm};

        let mut cases = vec![
            (
                Fault::StagedSync,
                CredentialStoreFailure::Unavailable,
                b"old-journal",
            ),
            (
                Fault::RenameUnknownBeforeEffect,
                CredentialStoreFailure::CommitUnknown,
                b"old-journal",
            ),
            (
                Fault::RenameUnknownAfterEffect,
                CredentialStoreFailure::CommitUnknown,
                b"new-journal",
            ),
            (
                Fault::HeldDirectorySync,
                CredentialStoreFailure::CommitUnknown,
                b"new-journal",
            ),
        ];
        if cfg!(windows) {
            cases.push((
                Fault::PublishedFileSync,
                CredentialStoreFailure::CommitUnknown,
                b"new-journal",
            ));
        }
        for (fault, expected, journal_after) in cases {
            let base = scratch("barrier-fault");
            let journal = boundary(&base, "journal", "namespace").expect("open boundary");
            journal
                .replace(b"old-journal")
                .expect("publish the previous journal");

            arm(fault);
            assert_eq!(journal.replace(b"new-journal"), Err(expected), "{fault:?}");
            assert_eq!(
                journal.read(),
                Ok(Some(journal_after.to_vec())),
                "{fault:?}"
            );
            journal.remove_stray_staged().expect("sweep the root");
            assert!(
                staged_leftovers(&base.join("journal")).is_empty(),
                "{fault:?}"
            );

            drop(journal);
            std::fs::remove_dir_all(&base).expect("remove isolated barrier-fault directory");
        }
    }

    /// A parent sync that fails while a root is opened fails the open, which
    /// latches nothing: the next open syncs that parent and succeeds.
    #[test]
    fn a_failed_parent_sync_fails_the_open_until_an_open_syncs_it() {
        use super::held_directory::faults::{Fault, arm};

        let base = scratch("parent-sync-fault");
        arm(Fault::ParentSync);
        assert_eq!(
            boundary(&base, "journal", "namespace").err(),
            Some(CredentialStoreFailure::Unavailable)
        );
        let journal = boundary(&base, "journal", "namespace").expect("the next open syncs");
        assert_eq!(journal.read(), Ok(None));
        drop(journal);
        std::fs::remove_dir_all(&base).expect("remove isolated parent-sync directory");
    }

    /// A staged file whose discard failed is cleaned up by the next staging
    /// in the same root. Only the exact shape of a generated staged name is
    /// reserved: a lookalike whose token is not a version 4 UUID stays.
    #[test]
    fn the_next_replace_removes_a_stray_staged_file() {
        let base = scratch("stray-sweep");
        let root = base.join("journal");
        let journal = boundary(&base, "journal", "namespace").expect("open boundary");
        journal.replace(b"first-journal").expect("first replace");
        let stray = root.join(format!(
            ".{JOURNAL_FILE_NAME}.{}.tmp",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&stray, b"stray-journal").expect("plant stray staged file");
        let mut token = uuid::Uuid::new_v4().simple().to_string();
        token.replace_range(12..13, "1");
        let lookalike = root.join(format!(".{JOURNAL_FILE_NAME}.{token}.tmp"));
        std::fs::write(&lookalike, b"not-ours").expect("plant lookalike");

        journal.replace(b"second-journal").expect("second replace");

        assert!(!stray.exists(), "stray staged file survived");
        assert_eq!(
            std::fs::read(&lookalike).expect("lookalike survives"),
            b"not-ours"
        );
        assert_eq!(journal.read(), Ok(Some(b"second-journal".to_vec())));
        drop(journal);
        std::fs::remove_dir_all(&base).expect("remove isolated stray-sweep directory");
    }

    /// A stray that cannot be removed is reported as a closed failure
    /// rather than left behind in silence, and nothing is published.
    #[test]
    fn a_stray_that_cannot_be_removed_fails_the_replace_closed() {
        let base = scratch("stray-refused");
        let root = base.join("journal");
        let journal = boundary(&base, "journal", "namespace").expect("open boundary");
        journal.replace(b"first-journal").expect("first replace");
        std::fs::create_dir(root.join(format!(
            ".{JOURNAL_FILE_NAME}.{}.tmp",
            uuid::Uuid::new_v4().simple()
        )))
        .expect("plant unremovable stray");

        assert_eq!(
            journal.replace(b"second-journal"),
            Err(CredentialStoreFailure::Unavailable)
        );
        assert_eq!(journal.read(), Ok(Some(b"first-journal".to_vec())));
        drop(journal);
        std::fs::remove_dir_all(&base).expect("remove isolated stray-refused directory");
    }

    #[cfg(unix)]
    #[test]
    fn unix_wrong_type_and_symlink_hardening_returns_typed_permission_failure() {
        use std::os::unix::fs::symlink;

        let base = scratch("permission-denials");
        std::fs::create_dir_all(base.join("target")).expect("create symlink target");
        symlink(base.join("target"), base.join("root-link")).expect("create root symlink");
        assert_eq!(
            boundary(&base, "root-link", "namespace").err(),
            Some(CredentialStoreFailure::PermissionHardeningFailed)
        );
        symlink(base.join("target"), base.join("namespace-link"))
            .expect("create namespace symlink");
        assert_eq!(
            boundary(&base, "journal", "namespace-link").err(),
            Some(CredentialStoreFailure::PermissionHardeningFailed)
        );

        std::fs::create_dir_all(base.join("namespace").join(NAMESPACE_LOCK_FILE_NAME))
            .expect("create directory at lock path");
        assert!(matches!(
            boundary(&base, "journal", "namespace")
                .expect("open boundary")
                .acquire_mutation_lock(),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        ));

        std::fs::create_dir_all(base.join("journal")).expect("create journal root");
        std::fs::write(base.join("target").join("journal-target"), b"target")
            .expect("create journal symlink target");
        symlink(
            base.join("target").join("journal-target"),
            base.join("journal").join(JOURNAL_FILE_NAME),
        )
        .expect("create journal symlink");
        assert_eq!(
            boundary(&base, "journal", "namespace")
                .expect("open boundary")
                .read(),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        );

        std::fs::remove_dir_all(&base).expect("remove isolated permission denial base");
    }

    #[cfg(unix)]
    fn unix_mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;

        std::fs::metadata(path)
            .expect("permission metadata")
            .permissions()
            .mode()
            & 0o777
    }

    #[cfg(unix)]
    #[test]
    fn unix_journal_root_state_and_lock_are_owner_only_and_broad_modes_are_corrected() {
        use std::os::unix::fs::PermissionsExt;

        let base = scratch("permissions");
        let root = base.join("journal");
        let namespace = base.join("namespace");
        std::fs::create_dir_all(&root).expect("create isolated journal root");
        std::fs::create_dir_all(&namespace).expect("create isolated namespace root");
        let journal_path = root.join(JOURNAL_FILE_NAME);
        let lock_path = namespace.join(NAMESPACE_LOCK_FILE_NAME);
        std::fs::write(&journal_path, b"old-journal").expect("create broad journal");
        std::fs::write(&lock_path, b"").expect("create broad lock");
        for (path, mode) in [
            (&root, 0o777),
            (&namespace, 0o777),
            (&journal_path, 0o666),
            (&lock_path, 0o666),
        ] {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                .expect("broaden mode");
        }

        let journal = boundary(&base, "journal", "namespace").expect("open boundary");
        let guard = journal
            .acquire_mutation_lock()
            .expect("acquire hardened mutation lock");
        journal
            .replace(b"new-journal")
            .expect("replace hardened journal");

        assert_eq!(unix_mode(&root), 0o700);
        assert_eq!(unix_mode(&journal_path), 0o600);
        assert_eq!(unix_mode(&namespace), 0o700);
        assert_eq!(unix_mode(&lock_path), 0o600);

        drop(guard);
        drop(journal);
        std::fs::remove_dir_all(&base).expect("remove isolated permission test directory");
    }

    #[test]
    fn two_journal_roots_contend_on_one_namespace_lock_until_the_guard_drops() {
        let base = scratch("lock");
        let first = boundary(&base, "journal-a", "namespace").expect("open first boundary");
        let second = boundary(&base, "journal-b", "namespace").expect("open second boundary");
        let elsewhere =
            boundary(&base, "journal-b", "other-namespace").expect("open third boundary");

        let first_guard = first
            .acquire_mutation_lock()
            .expect("first journal root acquires the namespace lock");
        assert!(matches!(
            second.acquire_mutation_lock(),
            Err(CredentialStoreFailure::OperationInProgress)
        ));
        drop(
            elsewhere
                .acquire_mutation_lock()
                .expect("a different namespace does not contend"),
        );
        assert!(
            base.join("namespace")
                .join(NAMESPACE_LOCK_FILE_NAME)
                .is_file()
        );

        drop(first_guard);
        drop(
            second
                .acquire_mutation_lock()
                .expect("lock releases with the owned guard"),
        );

        drop((first, second, elsewhere));
        std::fs::remove_dir_all(&base).expect("remove isolated lock test directory");
    }

    /// Ownership repair of roots the user already owns changes no owner and
    /// leaves them hardened; a root that does not exist is not created.
    #[test]
    fn ownership_repair_of_owned_roots_changes_nothing() {
        let base = scratch("owned-repair");
        let journal = boundary(&base, "journal", "namespace").expect("open boundary");
        journal.replace(b"journal").expect("publish a journal");
        drop(journal);

        assert_eq!(
            super::take_ownership_if_refused(&base.join("journal"), &[JOURNAL_FILE_NAME]),
            Ok(false)
        );
        assert_eq!(
            super::take_ownership_if_refused(&base.join("missing"), &[JOURNAL_FILE_NAME]),
            Ok(false)
        );
        assert!(!base.join("missing").exists());
        assert_eq!(
            boundary(&base, "journal", "namespace")
                .expect("reopen boundary")
                .read(),
            Ok(Some(b"journal".to_vec()))
        );
        std::fs::remove_dir_all(&base).expect("remove isolated owned-repair directory");
    }

    #[test]
    fn file_lock_child_harness() {
        let Ok(mode) = std::env::var(CHILD_MODE_ENV) else {
            return;
        };
        if mode == "canonical" {
            #[cfg(any(windows, target_os = "macos"))]
            assert_eq!(
                super::canonical_namespace_lock_root().expect("canonical namespace root"),
                PathBuf::from(std::env::var_os(CHILD_EXPECTED_ENV).expect("expected root"))
            );
            return;
        }
        // The permissive umask is in force before the roots are created, so
        // every path this child creates is created under it.
        #[cfg(unix)]
        if mode == "create_owner_only" {
            rustix::process::umask(rustix::fs::Mode::empty());
        }
        let path = |name| PathBuf::from(std::env::var_os(name).expect("child path environment"));
        let boundary = FileAuthorityJournalBoundary::open_for_test(
            path(CHILD_ROOT_ENV),
            path(CHILD_NAMESPACE_ENV),
        )
        .expect("child opens the boundary");

        match mode.as_str() {
            "owner" => {
                let _guard = boundary
                    .acquire_mutation_lock()
                    .expect("owner child acquires mutation lock");
                println!("AUDIO_GRAPH_FB2B_LOCKED");
                std::io::stdout().flush().expect("flush owner readiness");
                let mut input = Vec::new();
                std::io::stdin()
                    .read_to_end(&mut input)
                    .expect("wait for parent control pipe");
            }
            "contend" => assert!(matches!(
                boundary.acquire_mutation_lock(),
                Err(CredentialStoreFailure::OperationInProgress)
            )),
            "acquire" => drop(
                boundary
                    .acquire_mutation_lock()
                    .expect("fresh child reacquires mutation lock"),
            ),
            #[cfg(unix)]
            "create_owner_only" => {
                let guard = boundary
                    .acquire_mutation_lock()
                    .expect("create mutation lock under child umask");
                boundary
                    .replace(b"owner-only-journal")
                    .expect("create journal under child umask");
                drop(guard);
            }
            _ => panic!("unknown child lock mode"),
        }
    }

    fn lock_child_command(mode: &str, root: &Path, namespace: &Path) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("current test executable"));
        command
            .arg("file_lock_child_harness")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(CHILD_MODE_ENV, mode)
            .env(CHILD_ROOT_ENV, root)
            .env(CHILD_NAMESPACE_ENV, namespace);
        command
    }

    #[cfg(unix)]
    #[test]
    fn unix_owner_only_paths_survive_a_permissive_child_umask() {
        let base = scratch("child-umask");
        let root = base.join("journal");
        let namespace = base.join("namespace");
        let output = lock_child_command("create_owner_only", &root, &namespace)
            .output()
            .expect("run permissive-umask child");
        assert!(output.status.success(), "permissive-umask child failed");

        assert_eq!(unix_mode(&root), 0o700);
        assert_eq!(unix_mode(&root.join(JOURNAL_FILE_NAME)), 0o600);
        assert_eq!(unix_mode(&namespace), 0o700);
        assert_eq!(unix_mode(&namespace.join(NAMESPACE_LOCK_FILE_NAME)), 0o600);

        std::fs::remove_dir_all(&base).expect("remove permissive-umask child root");
    }

    /// Starts an owner child holding the namespace lock through `root` and
    /// returns it once it reports the lock is held.
    fn spawn_lock_owner(root: &Path, namespace: &Path) -> std::process::Child {
        let mut owner = lock_child_command("owner", root, namespace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn lock owner child");
        let mut owner_output =
            std::io::BufReader::new(owner.stdout.take().expect("owner child stdout pipe"));
        let mut line = String::new();
        while !line.contains("AUDIO_GRAPH_FB2B_LOCKED") {
            line.clear();
            if owner_output.read_line(&mut line).unwrap_or(0) == 0 {
                let _ = owner.kill();
                owner.wait().expect("reap failed lock owner child");
                panic!("owner exited before acquiring the lock");
            }
        }
        owner
    }

    fn run_lock_child(mode: &str, root: &Path, namespace: &Path) {
        let output = lock_child_command(mode, root, namespace)
            .output()
            .expect("run lock child");
        assert!(output.status.success(), "{mode} child failed");
    }

    #[test]
    fn file_lock_contends_across_processes_and_releases_when_owner_is_killed() {
        let base = scratch("child-lock");
        let root = base.join("journal");
        let namespace = base.join("namespace");
        let mut owner = spawn_lock_owner(&root, &namespace);

        run_lock_child("contend", &root, &namespace);
        owner.kill().expect("kill lock owner child");
        owner.wait().expect("reap killed lock owner child");
        run_lock_child("acquire", &root, &namespace);

        std::fs::remove_dir_all(&base).expect("remove isolated child-lock test directory");
    }

    /// Two processes given different journal roots for the same native
    /// namespace must still exclude each other: before this binding, each took
    /// a lock under its own root and both could pass the absence checks before
    /// either wrote the shared native marker.
    #[test]
    fn processes_with_different_journal_roots_contend_on_the_namespace_lock() {
        let base = scratch("two-root-lock");
        let namespace = base.join("namespace");
        let mut owner = spawn_lock_owner(&base.join("journal-a"), &namespace);

        run_lock_child("contend", &base.join("journal-b"), &namespace);
        run_lock_child(
            "acquire",
            &base.join("journal-b"),
            &base.join("other-namespace"),
        );
        owner.kill().expect("kill lock owner child");
        owner.wait().expect("reap killed lock owner child");
        run_lock_child("acquire", &base.join("journal-b"), &namespace);

        std::fs::remove_dir_all(&base).expect("remove isolated two-root lock test directory");
    }

    #[cfg(any(windows, target_os = "macos"))]
    #[test]
    fn canonical_namespace_lock_root_ignores_the_environment_and_names_the_namespace() {
        let expected = super::canonical_namespace_lock_root().expect("canonical namespace root");
        assert_eq!(
            expected.file_name(),
            Some(std::ffi::OsStr::new(super::CREDENTIAL_SERVICE))
        );
        let decoy = scratch("canonical-decoy");
        let output = Command::new(std::env::current_exe().expect("current test executable"))
            .arg("file_lock_child_harness")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(CHILD_MODE_ENV, "canonical")
            .env(CHILD_EXPECTED_ENV, &expected)
            .env("LOCALAPPDATA", &decoy)
            .env("USERPROFILE", &decoy)
            .env("HOME", &decoy)
            .env(crate::user_data::DATA_DIR_ENV, &decoy)
            .output()
            .expect("run canonical-root child");
        assert!(
            output.status.success(),
            "canonical root followed the environment"
        );
        assert!(!decoy.exists());
    }

    #[cfg(target_os = "macos")]
    fn macos_acl_lines(path: &Path) -> Vec<String> {
        let output = Command::new("/bin/ls")
            .arg("-lde")
            .arg(path)
            .output()
            .expect("list extended ACL");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .skip(1)
            .map(str::to_owned)
            .collect()
    }

    /// An inherited allow entry on the root would otherwise reach every file
    /// created under it. The root and the published journal must both end up
    /// with no extended ACL entry.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_inherited_allow_entry_is_cleared_before_any_byte() {
        let base = scratch("macos-acl");
        let root = base.join("journal");
        std::fs::create_dir_all(&root).expect("create journal root");
        let granted = Command::new("/bin/chmod")
            .arg("+a")
            .arg("everyone allow list,search,add_file,read,write,file_inherit,directory_inherit")
            .arg(&root)
            .status()
            .expect("add inherited allow entry");
        assert!(granted.success());
        assert!(!macos_acl_lines(&root).is_empty(), "fixture ACL missing");

        boundary(&base, "journal", "namespace")
            .expect("open boundary")
            .replace(b"journal")
            .expect("replace under a cleared ACL");

        assert_eq!(macos_acl_lines(&root), Vec::<String>::new());
        assert_eq!(
            macos_acl_lines(&root.join(JOURNAL_FILE_NAME)),
            Vec::<String>::new()
        );
        std::fs::remove_dir_all(&base).expect("remove isolated macOS ACL test directory");
    }

    #[cfg(windows)]
    fn windows_command(program: &str, args: &[&std::ffi::OsStr], cwd: &Path) {
        let status = Command::new(program)
            .args(args)
            .current_dir(cwd)
            .stdout(Stdio::null())
            .status()
            .expect("run Windows fixture command");
        assert!(status.success(), "{program} fixture command failed");
    }

    /// The DACL of `path` as SDDL, read with `icacls /save` so the control
    /// flags are included independent of localized display text.
    #[cfg(windows)]
    fn windows_dacl(path: &Path, scratch_dir: &Path) -> String {
        let saved = scratch_dir.join(format!("{}.acl", uuid::Uuid::new_v4().simple()));
        windows_command(
            "icacls.exe",
            &[path.as_os_str(), "/save".as_ref(), saved.as_os_str()],
            scratch_dir,
        );
        let bytes = std::fs::read(&saved).expect("read saved ACL");
        std::fs::remove_file(&saved).expect("remove saved ACL");
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
            .lines()
            .nth(1)
            .expect("saved DACL line")
            .trim()
            .to_owned()
    }

    #[cfg(windows)]
    fn windows_current_sid() -> String {
        let output = Command::new("whoami.exe")
            .args(["/user", "/fo", "csv", "/nh"])
            .output()
            .expect("query current user SID");
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout)
            .trim()
            .rsplit(',')
            .next()
            .expect("SID column")
            .trim_matches('"')
            .to_owned()
    }

    #[cfg(windows)]
    fn assert_protected_owner_only(sddl: &str) {
        assert!(sddl.starts_with("D:P"), "DACL is not protected: {sddl}");
        assert_eq!(
            sddl.matches('(').count(),
            1,
            "DACL has extra entries: {sddl}"
        );
        assert!(
            sddl.ends_with(&format!(";;;{})", windows_current_sid())),
            "DACL is not owner-only: {sddl}"
        );
    }

    /// A root whose DACL grants Everyone full control is hardened to a
    /// protected owner-only DACL before the write stage runs; the scripted
    /// writer refuses the write, so no journal byte ever exists.
    #[cfg(windows)]
    #[test]
    fn windows_root_granting_another_principal_is_hardened_before_any_byte() {
        let base = scratch("windows-dacl");
        let root = base.join("journal");
        std::fs::create_dir_all(&root).expect("create journal root");
        windows_command(
            "icacls.exe",
            &[
                root.as_os_str(),
                "/grant".as_ref(),
                "*S-1-1-0:(OI)(CI)F".as_ref(),
            ],
            &base,
        );
        assert!(
            windows_dacl(&root, &base).contains(";;;WD)"),
            "fixture grant missing"
        );

        let file_system = Arc::new(ScriptedAtomicFileSystem::new(AtomicFileCut::Write, None));
        assert_eq!(
            scripted_boundary(&base, file_system.clone()).replace(b"journal"),
            Err(CredentialStoreFailure::Unavailable)
        );
        assert_eq!(file_system.published(), None);
        assert_protected_owner_only(&windows_dacl(&root, &base));

        let journal = boundary(&base, "journal", "namespace").expect("open boundary");
        journal
            .replace(b"journal")
            .expect("replace hardened journal");
        assert_protected_owner_only(&windows_dacl(&root.join(JOURNAL_FILE_NAME), &base));
        drop(
            journal
                .acquire_mutation_lock()
                .expect("acquire hardened lock"),
        );
        assert_protected_owner_only(&windows_dacl(&base.join("namespace"), &base));
        assert_protected_owner_only(&windows_dacl(
            &base.join("namespace").join(NAMESPACE_LOCK_FILE_NAME),
            &base,
        ));

        drop(journal);
        std::fs::remove_dir_all(&base).expect("remove isolated Windows DACL test directory");
    }

    /// Swaps an ancestor junction to a decoy after the root is held and before
    /// the staged file is created.
    #[cfg(windows)]
    struct SwapBeforeStaging {
        base: PathBuf,
    }

    #[cfg(windows)]
    impl AtomicJournalFileSystem for SwapBeforeStaging {
        fn open_atomic<'directory>(
            &self,
            directory: &'directory HeldDirectory,
            name: &str,
        ) -> Result<Box<dyn AtomicJournalWriter + 'directory>, CredentialStoreFailure> {
            std::fs::remove_dir(self.base.join("parent")).expect("remove parent junction");
            windows_command(
                "cmd.exe",
                &[
                    "/d".as_ref(),
                    "/c".as_ref(),
                    "mklink".as_ref(),
                    "/j".as_ref(),
                    "parent".as_ref(),
                    "decoy".as_ref(),
                ],
                &self.base,
            );
            super::SystemAtomicJournalFileSystem.open_atomic(directory, name)
        }

        fn read(
            &self,
            directory: &HeldDirectory,
            name: &str,
            limit: usize,
        ) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
            super::SystemAtomicJournalFileSystem.read(directory, name, limit)
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_reparse_swap_after_open_does_not_redirect_hardening_or_bytes() {
        let base = scratch("windows-swap");
        let held = base.join("held").join("journal");
        let decoy = base.join("decoy").join("journal");
        std::fs::create_dir_all(&held).expect("create held journal root");
        std::fs::create_dir_all(&decoy).expect("create decoy journal root");
        windows_command(
            "cmd.exe",
            &[
                "/d".as_ref(),
                "/c".as_ref(),
                "mklink".as_ref(),
                "/j".as_ref(),
                "parent".as_ref(),
                "held".as_ref(),
            ],
            &base,
        );
        windows_command(
            "icacls.exe",
            &[decoy.as_os_str(), "/grant".as_ref(), "*S-1-1-0:R".as_ref()],
            &base,
        );
        let decoy_dacl = windows_dacl(&decoy, &base);

        let journal = FileAuthorityJournalBoundary::with_file_system(
            base.join("parent").join("journal"),
            base.join("namespace"),
            Arc::new(SwapBeforeStaging { base: base.clone() }),
        )
        .expect("open boundary");
        // Every child of the held root is opened relative to its handle, so
        // the swapped ancestor never comes into play: the journal lands in
        // the held root and nothing at all reaches the decoy.
        assert_eq!(journal.replace(b"journal"), Ok(()));

        assert_protected_owner_only(&windows_dacl(&held, &base));
        assert_eq!(windows_dacl(&decoy, &base), decoy_dacl);
        assert_eq!(
            std::fs::read(held.join(JOURNAL_FILE_NAME)).expect("read held journal"),
            b"journal"
        );
        assert_eq!(
            std::fs::read_dir(&decoy).expect("list decoy root").count(),
            0,
            "an entry reached the decoy"
        );

        drop(journal);
        std::fs::remove_dir(base.join("parent")).expect("remove parent junction");
        std::fs::remove_dir_all(&base).expect("remove isolated Windows swap test directory");
    }

    /// The stray sweep lists the held root itself, so an ancestor swapped to
    /// a directory with no journal root neither hides a stray nor fails the
    /// replace.
    #[cfg(windows)]
    #[test]
    fn windows_stray_sweep_lists_the_held_root_after_an_ancestor_swap() {
        let base = scratch("windows-swap-sweep");
        let held = base.join("held").join("journal");
        let decoy = base.join("decoy");
        std::fs::create_dir_all(&held).expect("create held journal root");
        std::fs::create_dir_all(&decoy).expect("create decoy without a journal root");
        point_parent_junction(&base, "held");
        let stray = held.join(format!(
            ".{JOURNAL_FILE_NAME}.{}.tmp",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::write(&stray, b"stray-journal").expect("plant stray staged file");

        let journal = FileAuthorityJournalBoundary::with_file_system(
            base.join("parent").join("journal"),
            base.join("namespace"),
            Arc::new(SwapBeforeStaging { base: base.clone() }),
        )
        .expect("open boundary");
        assert_eq!(journal.replace(b"journal"), Ok(()));

        assert!(!stray.exists(), "stray staged file survived");
        assert_eq!(
            std::fs::read(held.join(JOURNAL_FILE_NAME)).expect("read held journal"),
            b"journal"
        );
        assert_eq!(
            std::fs::read_dir(&decoy).expect("list decoy").count(),
            0,
            "an entry reached the decoy"
        );

        drop(journal);
        std::fs::remove_dir(base.join("parent")).expect("remove parent junction");
        std::fs::remove_dir_all(&base).expect("remove isolated Windows sweep test directory");
    }

    /// Points the `parent` junction in `base` at `target`, replacing it if it
    /// already exists.
    #[cfg(windows)]
    fn point_parent_junction(base: &Path, target: &str) {
        if base.join("parent").exists() {
            std::fs::remove_dir(base.join("parent")).expect("remove parent junction");
        }
        windows_command(
            "cmd.exe",
            &[
                "/d".as_ref(),
                "/c".as_ref(),
                "mklink".as_ref(),
                "/j".as_ref(),
                "parent".as_ref(),
                target.as_ref(),
            ],
            base,
        );
    }

    /// A held root reached through `parent` -> `held`, plus a decoy root
    /// holding a decoy journal, for ancestor-swap tests.
    #[cfg(windows)]
    fn junction_swap_fixture(label: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = scratch(label);
        let held = base.join("held").join("journal");
        let decoy = base.join("decoy").join("journal");
        std::fs::create_dir_all(&held).expect("create held journal root");
        std::fs::create_dir_all(&decoy).expect("create decoy journal root");
        std::fs::write(decoy.join(JOURNAL_FILE_NAME), b"decoy").expect("write decoy journal");
        point_parent_junction(&base, "held");
        (base, held, decoy)
    }

    /// The staged files left in `root`.
    fn staged_leftovers(root: &Path) -> Vec<std::ffi::OsString> {
        std::fs::read_dir(root)
            .expect("list journal root")
            .map(|entry| entry.expect("journal root entry").file_name())
            .filter(|name| name.to_string_lossy().ends_with(".tmp"))
            .collect()
    }

    /// Points the ancestor at the decoy after the staged bytes are flushed and
    /// before they are published, and leaves it there through the readback.
    #[cfg(windows)]
    struct SwapBetweenFlushAndRename {
        base: PathBuf,
    }

    #[cfg(windows)]
    struct SwapBetweenFlushAndRenameWriter<'directory> {
        staged: super::held_directory::StagedFile<'directory>,
        base: PathBuf,
    }

    #[cfg(windows)]
    impl AtomicJournalWriter for SwapBetweenFlushAndRenameWriter<'_> {
        fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            self.staged.write_all(bytes)
        }

        fn commit(self: Box<Self>) -> Result<(), PublishFailure> {
            let Self { mut staged, base } = *self;
            staged.sync().map_err(|_| PublishFailure::NotPublished)?;
            point_parent_junction(&base, "decoy");
            staged.publish()
        }
    }

    #[cfg(windows)]
    impl AtomicJournalFileSystem for SwapBetweenFlushAndRename {
        fn open_atomic<'directory>(
            &self,
            directory: &'directory HeldDirectory,
            name: &str,
        ) -> Result<Box<dyn AtomicJournalWriter + 'directory>, CredentialStoreFailure> {
            Ok(Box::new(SwapBetweenFlushAndRenameWriter {
                staged: directory.create_staged(name)?,
                base: self.base.clone(),
            }))
        }

        fn read(
            &self,
            directory: &HeldDirectory,
            name: &str,
            limit: usize,
        ) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
            super::SystemAtomicJournalFileSystem.read(directory, name, limit)
        }
    }

    /// The rename is bound to the held root and the staged file, not to the
    /// path they were reached by: an ancestor pointing elsewhere at the moment
    /// of publication neither redirects it nor touches the decoy.
    #[cfg(windows)]
    #[test]
    fn windows_ancestor_swap_between_flush_and_rename_publishes_into_the_held_root() {
        let (base, held, decoy) = junction_swap_fixture("windows-swap-publish");
        let journal = FileAuthorityJournalBoundary::with_file_system(
            base.join("parent").join("journal"),
            base.join("namespace"),
            Arc::new(SwapBetweenFlushAndRename { base: base.clone() }),
        )
        .expect("open boundary");

        assert_eq!(journal.replace(b"published"), Ok(()));
        assert_eq!(
            std::fs::read(held.join(JOURNAL_FILE_NAME)).expect("read held journal"),
            b"published"
        );
        assert_eq!(
            std::fs::read(decoy.join(JOURNAL_FILE_NAME)).expect("read decoy journal"),
            b"decoy"
        );
        assert!(staged_leftovers(&held).is_empty());
        assert!(staged_leftovers(&decoy).is_empty());

        drop(journal);
        std::fs::remove_dir(base.join("parent")).expect("remove parent junction");
        std::fs::remove_dir_all(&base).expect("remove isolated Windows publish-swap directory");
    }

    /// Between flush and rename of the next publication after it is armed,
    /// swaps the `parent` junction from the held root to a decoy that has no
    /// journal root, and leaves it there.
    #[cfg(windows)]
    struct ArmedSwapBetweenFlushAndRename {
        base: PathBuf,
        armed: Arc<AtomicBool>,
    }

    #[cfg(windows)]
    struct ArmedSwapWriter<'directory> {
        staged: super::held_directory::StagedFile<'directory>,
        swap_in: Option<PathBuf>,
    }

    #[cfg(windows)]
    impl AtomicJournalWriter for ArmedSwapWriter<'_> {
        fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
            self.staged.write_all(bytes)
        }

        fn commit(self: Box<Self>) -> Result<(), PublishFailure> {
            let Self {
                mut staged,
                swap_in,
            } = *self;
            staged.sync().map_err(|_| PublishFailure::NotPublished)?;
            if let Some(base) = swap_in {
                point_parent_junction(&base, "decoy");
            }
            staged.publish()
        }
    }

    #[cfg(windows)]
    impl AtomicJournalFileSystem for ArmedSwapBetweenFlushAndRename {
        fn open_atomic<'directory>(
            &self,
            directory: &'directory HeldDirectory,
            name: &str,
        ) -> Result<Box<dyn AtomicJournalWriter + 'directory>, CredentialStoreFailure> {
            Ok(Box::new(ArmedSwapWriter {
                staged: directory.create_staged(name)?,
                swap_in: self
                    .armed
                    .swap(false, Ordering::SeqCst)
                    .then(|| self.base.clone()),
            }))
        }

        fn read(
            &self,
            directory: &HeldDirectory,
            name: &str,
            limit: usize,
        ) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
            super::SystemAtomicJournalFileSystem.read(directory, name, limit)
        }
    }

    /// A journal boundary reached through a `parent` junction, with a switch
    /// that arms one ancestor swap during its next publication. Returns the
    /// scratch base, the held journal root, the switch and the boundary.
    #[cfg(windows)]
    pub(in crate::credentials::adapters) fn armed_ancestor_swap_boundary(
        label: &str,
    ) -> (
        PathBuf,
        PathBuf,
        Arc<AtomicBool>,
        FileAuthorityJournalBoundary,
    ) {
        let base = scratch(label);
        let held = base.join("held").join("journal");
        std::fs::create_dir_all(&held).expect("create held journal root");
        std::fs::create_dir_all(base.join("decoy")).expect("create decoy without a journal root");
        point_parent_junction(&base, "held");
        let armed = Arc::new(AtomicBool::new(false));
        let boundary = FileAuthorityJournalBoundary::with_file_system(
            base.join("parent").join("journal"),
            base.join("namespace"),
            Arc::new(ArmedSwapBetweenFlushAndRename {
                base: base.clone(),
                armed: armed.clone(),
            }),
        )
        .expect("open boundary");
        (base, held, armed, boundary)
    }

    #[cfg(windows)]
    pub(in crate::credentials::adapters) fn remove_ancestor_swap_fixture(base: &Path) {
        std::fs::remove_dir(base.join("parent")).expect("remove parent junction");
        std::fs::remove_dir_all(base).expect("remove isolated ancestor-swap directory");
    }

    /// Points the ancestor at the decoy, then fails the write, so the staged
    /// file is discarded while its path names the decoy.
    #[cfg(windows)]
    struct SwapThenFailWrite {
        base: PathBuf,
    }

    #[cfg(windows)]
    struct SwapThenFailWriteWriter<'directory> {
        _staged: super::held_directory::StagedFile<'directory>,
        base: PathBuf,
    }

    #[cfg(windows)]
    impl AtomicJournalWriter for SwapThenFailWriteWriter<'_> {
        fn write_all(&mut self, _bytes: &[u8]) -> std::io::Result<()> {
            point_parent_junction(&self.base, "decoy");
            Err(std::io::Error::from(std::io::ErrorKind::Other))
        }

        fn commit(self: Box<Self>) -> Result<(), PublishFailure> {
            unreachable!("the write always fails")
        }
    }

    #[cfg(windows)]
    impl AtomicJournalFileSystem for SwapThenFailWrite {
        fn open_atomic<'directory>(
            &self,
            directory: &'directory HeldDirectory,
            name: &str,
        ) -> Result<Box<dyn AtomicJournalWriter + 'directory>, CredentialStoreFailure> {
            Ok(Box::new(SwapThenFailWriteWriter {
                _staged: directory.create_staged(name)?,
                base: self.base.clone(),
            }))
        }

        fn read(
            &self,
            _directory: &HeldDirectory,
            _name: &str,
            _limit: usize,
        ) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
            unreachable!("a failed write never reads back")
        }
    }

    /// Discarding a staged file deletes that file, not whatever its path
    /// names once an ancestor points elsewhere.
    #[cfg(windows)]
    #[test]
    fn windows_ancestor_swap_before_cleanup_deletes_only_the_held_staged_file() {
        let (base, held, decoy) = junction_swap_fixture("windows-swap-cleanup");
        let journal = FileAuthorityJournalBoundary::with_file_system(
            base.join("parent").join("journal"),
            base.join("namespace"),
            Arc::new(SwapThenFailWrite { base: base.clone() }),
        )
        .expect("open boundary");

        assert_eq!(
            journal.replace(b"journal"),
            Err(CredentialStoreFailure::Unavailable)
        );
        assert!(staged_leftovers(&held).is_empty(), "staged file leaked");
        assert!(!held.join(JOURNAL_FILE_NAME).exists());
        assert_eq!(
            std::fs::read(decoy.join(JOURNAL_FILE_NAME)).expect("read decoy journal"),
            b"decoy"
        );
        assert!(staged_leftovers(&decoy).is_empty());

        drop(journal);
        std::fs::remove_dir(base.join("parent")).expect("remove parent junction");
        std::fs::remove_dir_all(&base).expect("remove isolated Windows cleanup-swap directory");
    }

    /// A base directory whose inheritable ACL grants Everyone full control,
    /// so anything created under it without its own protection is exposed.
    #[cfg(windows)]
    fn permissive_base(label: &str) -> PathBuf {
        let base = scratch(label);
        std::fs::create_dir_all(&base).expect("create permissive base");
        windows_command(
            "icacls.exe",
            &[
                base.as_os_str(),
                "/grant".as_ref(),
                "*S-1-1-0:(OI)(CI)F".as_ref(),
            ],
            &base,
        );
        base
    }

    /// The root is created already carrying its protected owner-only DACL;
    /// it is inspected before anything opens or hardens it.
    #[cfg(windows)]
    #[test]
    fn windows_new_root_is_owner_only_from_the_moment_it_is_created() {
        let base = permissive_base("windows-create-protected");
        let root = base.join("journal");

        crate::fs_util::create_owner_only_directory(&root).expect("create protected root");

        let dacl = windows_dacl(&root, &base);
        assert_protected_owner_only(&dacl);
        assert!(
            dacl.contains("(A;OICI;FA;;;"),
            "owner ACE is not inherited by children: {dacl}"
        );
        assert_eq!(
            crate::fs_util::create_owner_only_directory(&root).map_err(|error| error.kind()),
            Err(std::io::ErrorKind::AlreadyExists)
        );
        std::fs::remove_dir_all(&base).expect("remove isolated protected-create directory");
    }

    /// A file created inside a held root, even by code that never hardens
    /// it, starts out owner-only through the root's inherited ACE. So the
    /// lock and staged files are never exposed between creation and their
    /// own hardening.
    #[cfg(windows)]
    #[test]
    fn windows_children_of_a_held_root_start_owner_only() {
        let base = permissive_base("windows-child-inherits");
        let root = base.join("journal");
        let held = HeldDirectory::open_or_create(&root).expect("hold root");

        std::fs::write(root.join("unhardened-child"), b"child").expect("create plain child");

        let dacl = windows_dacl(&root.join("unhardened-child"), &base);
        assert_eq!(
            dacl.matches('(').count(),
            1,
            "child has extra entries: {dacl}"
        );
        assert!(
            dacl.contains("(A;ID;FA;;;")
                && dacl.ends_with(&format!(";;;{})", windows_current_sid())),
            "child is not owner-only by inheritance: {dacl}"
        );
        drop(held);
        std::fs::remove_dir_all(&base).expect("remove isolated child-inheritance directory");
    }

    #[cfg(windows)]
    fn open_for_read_control(path: &Path) -> std::fs::File {
        use ::windows::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, READ_CONTROL};
        use std::os::windows::fs::OpenOptionsExt;

        std::fs::OpenOptions::new()
            .access_mode(READ_CONTROL.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(path)
            .expect("open for READ_CONTROL")
    }

    /// The owner is read from the open handle. The Windows directory is owned
    /// by TrustedInstaller, which is not a trusted owner for credential state.
    #[cfg(windows)]
    #[test]
    fn windows_owner_trust_accepts_the_user_and_refuses_another_owner() {
        let base = scratch("windows-owner-trust");
        std::fs::create_dir_all(&base).expect("create own directory");
        let system_root =
            PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot is set on Windows"));

        assert!(
            crate::fs_util::owner_is_trusted(&open_for_read_control(&base))
                .expect("read own directory owner")
        );
        assert!(
            !crate::fs_util::owner_is_trusted(&open_for_read_control(&system_root))
                .expect("read Windows directory owner")
        );
        std::fs::remove_dir_all(&base).expect("remove isolated owner-trust directory");
    }

    /// A root someone else owns is refused before its DACL is touched: its
    /// owner keeps implicit `WRITE_DAC` and could undo any hardening. Opening
    /// again refuses again; only the explicit ownership repair, which may
    /// assign this user because the fixture's inherited DACL grants it full
    /// control, makes the root usable, and it leaves it owner-only.
    /// Assigning another owner needs `SeRestorePrivilege`, so the fixture can
    /// only be built by an elevated run. It is compiled as ignored unless the
    /// build runs under GitHub Actions, whose Windows runners are elevated;
    /// run it elsewhere with `--ignored` from an elevated shell.
    #[cfg(windows)]
    #[cfg_attr(
        not(audiograph_github_actions),
        ignore = "needs an elevated run to assign a foreign owner; runs under GitHub Actions"
    )]
    #[test]
    fn windows_a_foreign_owned_root_is_refused_until_an_explicit_ownership_repair() {
        let base = scratch("windows-foreign-owner");
        let root = base.join("journal");
        std::fs::create_dir_all(&root).expect("create journal root");
        let assigned = Command::new("icacls.exe")
            .arg(&root)
            .args(["/setowner", "*S-1-5-19"])
            .current_dir(&base)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("run icacls");
        assert!(
            assigned.success(),
            "assigning LocalService as owner needs an elevated run (icacls {assigned})"
        );
        // The production access mask still opens the fixture and its owner
        // reads as untrusted, so the refusal below comes from the owner check
        // and not from a denied open.
        let opened = super::held_directory::open_directory_handle(&root)
            .expect("the held-directory access mask opens the foreign-owned fixture");
        assert!(
            !crate::fs_util::owner_is_trusted(&opened).expect("read the fixture owner"),
            "the fixture owner reads as trusted"
        );
        drop(opened);
        let dacl_before = windows_dacl(&root, &base);

        for _ in 0..2 {
            assert_eq!(
                boundary(&base, "journal", "namespace").err(),
                Some(CredentialStoreFailure::PermissionHardeningFailed)
            );
        }
        assert_eq!(
            windows_dacl(&root, &base),
            dacl_before,
            "a foreign-owned root was hardened"
        );
        assert!(!root.join(JOURNAL_FILE_NAME).exists());

        assert_eq!(
            super::take_ownership_if_refused(&root, &[JOURNAL_FILE_NAME]),
            Ok(true)
        );
        assert_eq!(
            super::take_ownership_if_refused(&root, &[JOURNAL_FILE_NAME]),
            Ok(false)
        );
        assert_protected_owner_only(&windows_dacl(&root, &base));
        let journal = boundary(&base, "journal", "namespace").expect("open repaired root");
        journal
            .replace(b"journal")
            .expect("publish into the repaired root");
        drop(journal);
        std::fs::remove_dir_all(&base).expect("remove isolated foreign-owner directory");
    }

    /// The explicit ownership repair changes only the object whose refusal
    /// it recorded. Once an ancestor of the refused root is repointed, the
    /// path reaches another directory, which the repair refuses with its
    /// DACL untouched.
    #[cfg(windows)]
    #[test]
    fn windows_the_ownership_repair_refuses_a_root_an_ancestor_swap_put_at_the_path() {
        let base = scratch("windows-repair-swap");
        let decoy = base.join("decoy").join("journal");
        std::fs::create_dir_all(base.join("held").join("journal")).expect("create the held root");
        std::fs::create_dir_all(&decoy).expect("create the decoy root");
        windows_command(
            "icacls.exe",
            &[decoy.as_os_str(), "/grant".as_ref(), "*S-1-1-0:R".as_ref()],
            &base,
        );
        let decoy_dacl = windows_dacl(&decoy, &base);
        point_parent_junction(&base, "held");
        let root = base.join("parent").join("journal");
        super::held_directory::record_refused_owner_for_test(&root, None);
        point_parent_junction(&base, "decoy");

        assert_eq!(
            super::take_ownership_if_refused(&root, &[JOURNAL_FILE_NAME]),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        );
        assert_eq!(windows_dacl(&decoy, &base), decoy_dacl);

        std::fs::remove_dir(base.join("parent")).expect("remove parent junction");
        std::fs::remove_dir_all(&base).expect("remove isolated repair-swap directory");
    }

    /// A journal with another name is refused before its DACL changes, by an
    /// ordinary read and by the explicit ownership repair alike: changing it
    /// through this name would change the file under its other name too.
    #[cfg(windows)]
    #[test]
    fn windows_a_hard_linked_journal_is_refused_by_hardening_and_by_the_repair() {
        let base = scratch("windows-hard-link");
        let root = base.join("journal");
        let journal = boundary(&base, "journal", "namespace").expect("open boundary");
        let outside = base.join("outside.json");
        std::fs::write(&outside, b"outside").expect("write the other name");
        windows_command(
            "icacls.exe",
            &[
                outside.as_os_str(),
                "/grant".as_ref(),
                "*S-1-1-0:R".as_ref(),
            ],
            &base,
        );
        let outside_dacl = windows_dacl(&outside, &base);
        std::fs::hard_link(&outside, root.join(JOURNAL_FILE_NAME)).expect("link the journal name");

        assert_eq!(
            journal.read(),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        );
        drop(journal);
        super::held_directory::record_refused_owner_for_test(&root, Some(JOURNAL_FILE_NAME));
        assert_eq!(
            super::take_ownership_if_refused(&root, &[JOURNAL_FILE_NAME]),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        );
        assert_eq!(windows_dacl(&outside, &base), outside_dacl);

        std::fs::remove_dir_all(&base).expect("remove isolated hard-link directory");
    }

    /// While a process holds a root reached through a junction, the junction
    /// can be pointed elsewhere, so another process opening the same paths
    /// gets other directories. The first boundary's lock is then refused,
    /// and the boundary opened over the new target locks as usual.
    #[cfg(windows)]
    #[test]
    fn windows_a_root_whose_ancestor_was_repointed_refuses_the_lock() {
        let base = scratch("windows-repointed-lock");
        std::fs::create_dir_all(base.join("held")).expect("create the held parent");
        std::fs::create_dir_all(base.join("decoy")).expect("create the decoy parent");
        point_parent_junction(&base, "held");
        let open = || {
            FileAuthorityJournalBoundary::open_for_test(
                base.join("parent").join("journal"),
                base.join("parent").join("namespace"),
            )
        };
        let first = open().expect("open the first boundary");
        point_parent_junction(&base, "decoy");
        let second = open().expect("open over the repointed junction");

        assert!(matches!(
            first.acquire_mutation_lock(),
            Err(CredentialStoreFailure::Unavailable)
        ));
        drop(
            second
                .acquire_mutation_lock()
                .expect("the repointed boundary locks"),
        );

        drop((first, second));
        std::fs::remove_dir(base.join("parent")).expect("remove parent junction");
        std::fs::remove_dir_all(&base).expect("remove isolated repointed-lock directory");
    }
}
