use super::keyring_entry::CREDENTIAL_SERVICE;
use crate::credentials::domain::{AuthorityJournal, CredentialStoreFailure};
use held_directory::{HeldDirectory, StagedFile};
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

pub(super) trait AuthorityJournalBoundary: Send + Sync {
    fn read(&self) -> Result<Option<Vec<u8>>, CredentialStoreFailure>;
    fn replace(&self, bytes: &[u8]) -> Result<(), CredentialStoreFailure>;
    fn acquire_mutation_lock(
        &self,
    ) -> Result<Box<dyn AuthorityMutationLock>, CredentialStoreFailure>;
}

/// One staged journal replacement. Bytes become visible only through
/// `commit`; dropping an uncommitted writer discards them.
trait AtomicJournalWriter {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    fn commit(self: Box<Self>) -> std::io::Result<()>;
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

    fn commit(self: Box<Self>) -> std::io::Result<()> {
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

pub(super) struct FileAuthorityJournalBoundary {
    root: PathBuf,
    namespace_lock_root: PathBuf,
    file_system: Arc<dyn AtomicJournalFileSystem>,
}

impl FileAuthorityJournalBoundary {
    /// The journal root is caller-selected. The mutation lock is not: it lives
    /// in the canonical per-user namespace directory.
    pub(super) fn new(root: impl Into<PathBuf>) -> Result<Self, CredentialStoreFailure> {
        Ok(Self::with_namespace_lock_root(
            root.into(),
            canonical_namespace_lock_root()?,
        ))
    }

    pub(super) fn with_namespace_lock_root(root: PathBuf, namespace_lock_root: PathBuf) -> Self {
        Self::with_file_system(
            root,
            namespace_lock_root,
            Arc::new(SystemAtomicJournalFileSystem),
        )
    }

    fn with_file_system(
        root: PathBuf,
        namespace_lock_root: PathBuf,
        file_system: Arc<dyn AtomicJournalFileSystem>,
    ) -> Self {
        Self {
            root,
            namespace_lock_root,
            file_system,
        }
    }
}

struct FileAuthorityMutationLock {
    file: File,
    _directory: HeldDirectory,
}

impl AuthorityMutationLock for FileAuthorityMutationLock {}

impl Drop for FileAuthorityMutationLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl AuthorityJournalBoundary for FileAuthorityJournalBoundary {
    fn read(&self) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
        let Some(directory) = HeldDirectory::open_existing(&self.root)? else {
            return Ok(None);
        };
        // One byte past the ceiling is enough for the decoder to refuse an
        // oversized journal as recovery-required.
        self.file_system.read(
            &directory,
            JOURNAL_FILE_NAME,
            MAX_AUTHORITY_JOURNAL_BYTES + 1,
        )
    }

    fn replace(&self, bytes: &[u8]) -> Result<(), CredentialStoreFailure> {
        // The root and the staged file are both hardened before any byte is
        // written; a hardening failure stops here as a definite failure.
        let directory = HeldDirectory::open_or_create(&self.root)?;
        let mut file = self
            .file_system
            .open_atomic(&directory, JOURNAL_FILE_NAME)?;
        file.write_all(bytes)
            .map_err(|_| CredentialStoreFailure::Unavailable)?;
        file.commit()
            .map_err(|_| CredentialStoreFailure::CommitUnknown)?;
        match self.file_system.read(
            &directory,
            JOURNAL_FILE_NAME,
            MAX_AUTHORITY_JOURNAL_BYTES + 1,
        ) {
            Ok(Some(readback)) if readback == bytes => Ok(()),
            Ok(_) | Err(_) => Err(CredentialStoreFailure::CommitUnknown),
        }
    }

    fn acquire_mutation_lock(
        &self,
    ) -> Result<Box<dyn AuthorityMutationLock>, CredentialStoreFailure> {
        let directory = HeldDirectory::open_or_create(&self.namespace_lock_root)?;
        let file = directory.open_lock_file(NAMESPACE_LOCK_FILE_NAME)?;
        match file.try_lock() {
            Ok(()) => Ok(Box::new(FileAuthorityMutationLock {
                file,
                _directory: directory,
            })),
            Err(std::fs::TryLockError::WouldBlock) => {
                Err(CredentialStoreFailure::OperationInProgress)
            }
            Err(std::fs::TryLockError::Error(_)) => Err(CredentialStoreFailure::Unavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AtomicJournalFileSystem, AtomicJournalWriter, AuthorityJournalBoundary,
        FileAuthorityJournalBoundary, HeldDirectory, JOURNAL_FILE_NAME,
        MAX_AUTHORITY_JOURNAL_BYTES, NAMESPACE_LOCK_FILE_NAME, decode_authority_journal,
        encode_authority_journal,
    };
    use crate::credentials::domain::{AuthorityJournal, CredentialStoreFailure};
    use audio_graph_ipc_contract::credential_contract::CredentialBackendKind;
    use std::io::{BufRead, Read, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};

    const CHILD_MODE_ENV: &str = "AUDIO_GRAPH_FB2B_LOCK_CHILD_MODE";
    const CHILD_ROOT_ENV: &str = "AUDIO_GRAPH_FB2B_LOCK_CHILD_ROOT";
    const CHILD_NAMESPACE_ENV: &str = "AUDIO_GRAPH_FB2B_LOCK_CHILD_NAMESPACE";
    #[cfg(any(windows, target_os = "macos"))]
    const CHILD_EXPECTED_ENV: &str = "AUDIO_GRAPH_FB2B_LOCK_CHILD_EXPECTED";

    fn scratch(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "audio-graph-journal-{label}-{}",
            uuid::Uuid::new_v4().simple()
        ))
    }

    /// A journal root and a namespace lock directory side by side in `base`.
    fn boundary(base: &Path, root: &str, namespace: &str) -> FileAuthorityJournalBoundary {
        FileAuthorityJournalBoundary::with_namespace_lock_root(
            base.join(root),
            base.join(namespace),
        )
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum AtomicFileCut {
        None,
        Open,
        Write,
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

        fn commit(self: Box<Self>) -> std::io::Result<()> {
            if self.cut == AtomicFileCut::Commit {
                return Err(std::io::Error::other("scripted uncertain commit cut"));
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
                AtomicFileCut::Open | AtomicFileCut::Write | AtomicFileCut::Commit
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

        std::fs::remove_dir_all(&base).expect("remove isolated stage-map test directory");
    }

    #[test]
    fn ordinary_journal_read_failure_remains_unavailable() {
        let base = scratch("read-failure");
        std::fs::create_dir_all(base.join("journal")).expect("create journal root");
        let file_system = Arc::new(ScriptedAtomicFileSystem::new(AtomicFileCut::Readback, None));
        let boundary = scripted_boundary(&base, file_system);

        assert_eq!(boundary.read(), Err(CredentialStoreFailure::Unavailable));
        std::fs::remove_dir_all(&base).expect("remove isolated read-failure test directory");
    }

    #[test]
    fn absent_root_reads_as_absent_without_creating_it() {
        let base = scratch("absent-root");
        assert_eq!(boundary(&base, "journal", "namespace").read(), Ok(None));
        assert!(!base.exists());
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
        let journal = boundary(&base, "journal", "namespace");
        journal.replace(b"first-journal").expect("first replace");
        journal.replace(b"second-journal").expect("second replace");

        assert_eq!(journal.read(), Ok(Some(b"second-journal".to_vec())));
        let names: Vec<_> = std::fs::read_dir(base.join("journal"))
            .expect("list journal root")
            .map(|entry| entry.expect("journal root entry").file_name())
            .collect();
        assert_eq!(names, [std::ffi::OsString::from(JOURNAL_FILE_NAME)]);

        std::fs::remove_dir_all(&base).expect("remove isolated replace test directory");
    }

    #[cfg(unix)]
    #[test]
    fn unix_wrong_type_and_symlink_hardening_returns_typed_permission_failure() {
        use std::os::unix::fs::symlink;

        let base = scratch("permission-denials");
        std::fs::create_dir_all(base.join("target")).expect("create symlink target");
        symlink(base.join("target"), base.join("root-link")).expect("create root symlink");
        assert_eq!(
            boundary(&base, "root-link", "namespace").replace(b"journal"),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        );
        symlink(base.join("target"), base.join("namespace-link"))
            .expect("create namespace symlink");
        assert!(matches!(
            boundary(&base, "journal", "namespace-link").acquire_mutation_lock(),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        ));

        std::fs::create_dir_all(base.join("namespace").join(NAMESPACE_LOCK_FILE_NAME))
            .expect("create directory at lock path");
        assert!(matches!(
            boundary(&base, "journal", "namespace").acquire_mutation_lock(),
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
            boundary(&base, "journal", "namespace").read(),
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

        let journal = boundary(&base, "journal", "namespace");
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
        std::fs::remove_dir_all(&base).expect("remove isolated permission test directory");
    }

    #[test]
    fn two_journal_roots_contend_on_one_namespace_lock_until_the_guard_drops() {
        let base = scratch("lock");
        let first = boundary(&base, "journal-a", "namespace");
        let second = boundary(&base, "journal-b", "namespace");
        let elsewhere = boundary(&base, "journal-b", "other-namespace");

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

        std::fs::remove_dir_all(&base).expect("remove isolated lock test directory");
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
        let path = |name| PathBuf::from(std::env::var_os(name).expect("child path environment"));
        let boundary = FileAuthorityJournalBoundary::with_namespace_lock_root(
            path(CHILD_ROOT_ENV),
            path(CHILD_NAMESPACE_ENV),
        );

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
                rustix::process::umask(rustix::fs::Mode::empty());
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

        let journal = boundary(&base, "journal", "namespace");
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
        );
        assert_eq!(
            journal.replace(b"journal"),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        );

        assert_protected_owner_only(&windows_dacl(&held, &base));
        assert_eq!(windows_dacl(&decoy, &base), decoy_dacl);
        assert!(!held.join(JOURNAL_FILE_NAME).exists());
        assert!(!decoy.join(JOURNAL_FILE_NAME).exists());
        for entry in std::fs::read_dir(&decoy).expect("list decoy root") {
            let entry = entry.expect("decoy entry");
            assert_eq!(
                entry.metadata().expect("decoy entry metadata").len(),
                0,
                "journal bytes reached the decoy"
            );
        }

        std::fs::remove_dir(base.join("parent")).expect("remove parent junction");
        std::fs::remove_dir_all(&base).expect("remove isolated Windows swap test directory");
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

    #[cfg(windows)]
    fn staged_leftovers(root: &Path) -> Vec<std::ffi::OsString> {
        std::fs::read_dir(root)
            .expect("list journal root")
            .map(|entry| entry.expect("journal root entry").file_name())
            .filter(|name| name.to_string_lossy().ends_with(".tmp"))
            .collect()
    }

    /// Points the ancestor at the decoy after the staged bytes are flushed and
    /// before they are published, then points it back for the readback.
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

        fn commit(self: Box<Self>) -> std::io::Result<()> {
            let Self { mut staged, base } = *self;
            staged.sync()?;
            point_parent_junction(&base, "decoy");
            let published = staged.publish();
            point_parent_junction(&base, "held");
            published
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
        );

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

        std::fs::remove_dir(base.join("parent")).expect("remove parent junction");
        std::fs::remove_dir_all(&base).expect("remove isolated Windows publish-swap directory");
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

        fn commit(self: Box<Self>) -> std::io::Result<()> {
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
        );

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
    /// owner keeps implicit `WRITE_DAC` and could undo any hardening.
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
    fn windows_foreign_owned_root_is_refused_before_it_is_hardened() {
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

        assert_eq!(
            boundary(&base, "journal", "namespace").replace(b"journal"),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        );
        assert_eq!(
            windows_dacl(&root, &base),
            dacl_before,
            "a foreign-owned root was hardened"
        );
        assert!(!root.join(JOURNAL_FILE_NAME).exists());
        std::fs::remove_dir_all(&base).expect("remove isolated foreign-owner directory");
    }
}
