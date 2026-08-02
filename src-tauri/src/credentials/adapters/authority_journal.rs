use crate::credentials::domain::{
    AuthorityJournal, CredentialAuthorityInstanceId, CredentialStoreFailure,
};
use atomic_write_file::AtomicWriteFile;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const JOURNAL_FILE_NAME: &str = "state.json";
const MUTATION_LOCK_FILE_NAME: &str = "mutation.lock";
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
    pub(super) authority: AuthorityJournalIdentity,
    pub(super) journal: AuthorityJournal,
}

#[derive(Clone, PartialEq, Eq)]
pub(super) struct AuthorityJournalIdentity {
    wire_value: String,
    opaque: CredentialAuthorityInstanceId,
}

impl AuthorityJournalIdentity {
    fn from_wire_value(value: String) -> Option<Self> {
        let uuid = uuid::Uuid::parse_str(&value).ok()?;
        if uuid.hyphenated().to_string() != value {
            return None;
        }
        Some(Self {
            wire_value: value,
            opaque: CredentialAuthorityInstanceId::from_validated_bytes(*uuid.as_bytes()),
        })
    }

    pub(super) fn opaque(&self) -> CredentialAuthorityInstanceId {
        self.opaque.clone()
    }
}

pub(super) fn new_authority_instance_id() -> AuthorityJournalIdentity {
    AuthorityJournalIdentity::from_wire_value(uuid::Uuid::new_v4().hyphenated().to_string())
        .expect("generated UUID is canonical")
}

#[cfg(test)]
pub(super) fn authority_identity_for_test(value: &str) -> AuthorityJournalIdentity {
    AuthorityJournalIdentity::from_wire_value(value.to_owned())
        .expect("test authority identity must be a canonical UUID")
}

pub(super) fn encode_authority_journal(
    authority: &AuthorityJournalIdentity,
    journal: &AuthorityJournal,
) -> Result<Vec<u8>, CredentialStoreFailure> {
    let bytes = serde_json::to_vec(&AuthorityJournalEnvelope {
        schema_version: AUTHORITY_ENVELOPE_SCHEMA_VERSION,
        authority_instance_id: authority.wire_value.clone(),
        journal: journal.clone(),
    })
    .map_err(|_| CredentialStoreFailure::Internal)?;
    if bytes.len() > MAX_AUTHORITY_JOURNAL_BYTES {
        return Err(CredentialStoreFailure::CorruptRecord);
    }
    Ok(bytes)
}

pub(super) fn encode_authority_marker(
    authority: &AuthorityJournalIdentity,
) -> Result<Vec<u8>, CredentialStoreFailure> {
    serde_json::to_vec(&AuthorityMarkerEnvelope {
        schema_version: AUTHORITY_ENVELOPE_SCHEMA_VERSION,
        authority_instance_id: authority.wire_value.clone(),
    })
    .map_err(|_| CredentialStoreFailure::Internal)
}

pub(super) fn decode_authority_journal(bytes: &[u8]) -> Option<DecodedAuthorityJournal> {
    if bytes.len() > MAX_AUTHORITY_JOURNAL_BYTES {
        return None;
    }
    let envelope: AuthorityJournalEnvelope = serde_json::from_slice(bytes).ok()?;
    if envelope.schema_version != AUTHORITY_ENVELOPE_SCHEMA_VERSION {
        return None;
    }
    let authority = AuthorityJournalIdentity::from_wire_value(envelope.authority_instance_id)?;
    Some(DecodedAuthorityJournal {
        authority,
        journal: envelope.journal,
    })
}

pub(super) fn decode_authority_marker(bytes: &[u8]) -> Option<AuthorityJournalIdentity> {
    let envelope: AuthorityMarkerEnvelope = serde_json::from_slice(bytes).ok()?;
    if envelope.schema_version != AUTHORITY_ENVELOPE_SCHEMA_VERSION {
        return None;
    }
    AuthorityJournalIdentity::from_wire_value(envelope.authority_instance_id)
}

pub(super) trait AuthorityMutationLock: Send {}

pub(super) trait AuthorityJournalBoundary: Send + Sync {
    fn read(&self) -> Result<Option<Vec<u8>>, CredentialStoreFailure>;
    fn replace(&self, bytes: &[u8]) -> Result<(), CredentialStoreFailure>;
    fn acquire_mutation_lock(
        &self,
    ) -> Result<Box<dyn AuthorityMutationLock>, CredentialStoreFailure>;
}

trait AtomicJournalWriter: Send {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()>;
    fn commit(self: Box<Self>) -> std::io::Result<()>;
}

trait AtomicJournalFileSystem: Send + Sync {
    fn open_atomic(&self, path: &Path) -> std::io::Result<Box<dyn AtomicJournalWriter>>;
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>>;
}

struct SystemAtomicJournalFileSystem;

struct SystemAtomicJournalWriter(AtomicWriteFile);

impl AtomicJournalWriter for SystemAtomicJournalWriter {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        Write::write_all(&mut self.0, bytes)
    }

    fn commit(self: Box<Self>) -> std::io::Result<()> {
        let Self(file) = *self;
        file.commit()
    }
}

impl AtomicJournalFileSystem for SystemAtomicJournalFileSystem {
    fn open_atomic(&self, path: &Path) -> std::io::Result<Box<dyn AtomicJournalWriter>> {
        #[cfg(unix)]
        let file = {
            let mut options = atomic_write_file::OpenOptions::new();
            atomic_write_file::unix::OpenOptionsExt::preserve_mode(&mut options, false);
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            options.open(path)?
        };
        #[cfg(not(unix))]
        let file = AtomicWriteFile::open(path)?;

        Ok(Box::new(SystemAtomicJournalWriter(file)))
    }

    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        std::fs::read(path)
    }
}

pub(super) struct FileAuthorityJournalBoundary {
    root: PathBuf,
    journal_path: PathBuf,
    mutation_lock_path: PathBuf,
    file_system: Arc<dyn AtomicJournalFileSystem>,
}

impl FileAuthorityJournalBoundary {
    pub(super) fn new(root: impl Into<PathBuf>) -> Self {
        Self::with_file_system(root.into(), Arc::new(SystemAtomicJournalFileSystem))
    }

    fn with_file_system(root: PathBuf, file_system: Arc<dyn AtomicJournalFileSystem>) -> Self {
        Self {
            journal_path: root.join(JOURNAL_FILE_NAME),
            mutation_lock_path: root.join(MUTATION_LOCK_FILE_NAME),
            root,
            file_system,
        }
    }

    fn ensure_root(&self) -> Result<(), CredentialStoreFailure> {
        match std::fs::symlink_metadata(&self.root) {
            Ok(_) => return Self::harden_directory(&self.root),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(CredentialStoreFailure::PermissionHardeningFailed),
        }

        Self::create_root_owner_only(&self.root)?;
        Self::harden_directory(&self.root)
    }

    #[cfg(unix)]
    fn create_root_owner_only(path: &Path) -> Result<(), CredentialStoreFailure> {
        use std::os::unix::fs::DirBuilderExt;

        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        builder
            .create(path)
            .map_err(|_| CredentialStoreFailure::Unavailable)
    }

    #[cfg(not(unix))]
    fn create_root_owner_only(path: &Path) -> Result<(), CredentialStoreFailure> {
        std::fs::create_dir_all(path).map_err(|_| CredentialStoreFailure::Unavailable)
    }

    fn open_lock_file(path: &Path) -> Result<File, CredentialStoreFailure> {
        Self::harden_file_if_present(path)?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let file = options
            .open(path)
            .map_err(|_| CredentialStoreFailure::Unavailable)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)?;
        }
        Ok(file)
    }

    #[cfg(unix)]
    fn harden_directory(path: &Path) -> Result<(), CredentialStoreFailure> {
        use std::os::unix::fs::PermissionsExt;

        let metadata = std::fs::symlink_metadata(path)
            .map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)?;
        if !metadata.file_type().is_dir() {
            return Err(CredentialStoreFailure::PermissionHardeningFailed);
        }
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)
    }

    #[cfg(not(unix))]
    fn harden_directory(_path: &Path) -> Result<(), CredentialStoreFailure> {
        Ok(())
    }

    #[cfg(unix)]
    fn harden_file_if_present(path: &Path) -> Result<(), CredentialStoreFailure> {
        use std::os::unix::fs::PermissionsExt;

        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_file() => {
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                    .map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)
            }
            Ok(_) => Err(CredentialStoreFailure::PermissionHardeningFailed),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(CredentialStoreFailure::PermissionHardeningFailed),
        }
    }

    #[cfg(not(unix))]
    fn harden_file_if_present(_path: &Path) -> Result<(), CredentialStoreFailure> {
        Ok(())
    }
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
        Self::harden_file_if_present(&self.journal_path)?;
        match self.file_system.read(&self.journal_path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(CredentialStoreFailure::Unavailable),
        }
    }

    fn replace(&self, bytes: &[u8]) -> Result<(), CredentialStoreFailure> {
        self.ensure_root()?;
        Self::harden_file_if_present(&self.journal_path)?;
        let mut file = self
            .file_system
            .open_atomic(&self.journal_path)
            .map_err(|_| CredentialStoreFailure::Unavailable)?;
        file.write_all(bytes)
            .map_err(|_| CredentialStoreFailure::Unavailable)?;
        file.commit()
            .map_err(|_| CredentialStoreFailure::CommitUnknown)?;
        Self::harden_file_if_present(&self.journal_path)
            .map_err(|_| CredentialStoreFailure::CommitUnknown)?;
        match self.file_system.read(&self.journal_path) {
            Ok(readback) if readback == bytes => Ok(()),
            Ok(_) | Err(_) => Err(CredentialStoreFailure::CommitUnknown),
        }
    }

    fn acquire_mutation_lock(
        &self,
    ) -> Result<Box<dyn AuthorityMutationLock>, CredentialStoreFailure> {
        self.ensure_root()?;
        let file = Self::open_lock_file(&self.mutation_lock_path)?;
        match file.try_lock() {
            Ok(()) => Ok(Box::new(FileAuthorityMutationLock { file })),
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
        FileAuthorityJournalBoundary, JOURNAL_FILE_NAME, MAX_AUTHORITY_JOURNAL_BYTES,
        MUTATION_LOCK_FILE_NAME, authority_identity_for_test, decode_authority_journal,
        encode_authority_journal,
    };
    use crate::credentials::domain::{AuthorityJournal, CredentialStoreFailure};
    use audio_graph_ipc_contract::credential_contract::CredentialBackendKind;
    use std::io::{BufRead, Read, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum AtomicFileCut {
        None,
        Open,
        Write,
        Commit,
        PostCommitPermission,
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
        path: PathBuf,
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
            let cut = self.cut;
            let path = self.path.clone();
            let mut published = self.published.lock().expect("scripted publication lock");
            let mut bytes = self.pending;
            if self.cut == AtomicFileCut::Mismatch {
                bytes.push(b'!');
            }
            *published = Some(bytes);
            drop(published);
            if cut == AtomicFileCut::PostCommitPermission {
                std::fs::create_dir(path)?;
            }
            Ok(())
        }
    }

    impl AtomicJournalFileSystem for ScriptedAtomicFileSystem {
        fn open_atomic(&self, path: &Path) -> std::io::Result<Box<dyn AtomicJournalWriter>> {
            if self.cut == AtomicFileCut::Open {
                return Err(std::io::Error::other("scripted prepublication open cut"));
            }
            Ok(Box::new(ScriptedAtomicWriter {
                cut: self.cut,
                pending: Vec::new(),
                published: self.published.clone(),
                path: path.to_path_buf(),
            }))
        }

        fn read(&self, _path: &Path) -> std::io::Result<Vec<u8>> {
            if self.cut == AtomicFileCut::Readback {
                return Err(std::io::Error::other("scripted uncertain readback cut"));
            }
            self.published()
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
        }
    }

    #[test]
    fn journal_byte_ceiling_is_enforced_before_whitespace_tolerant_deserialization() {
        let mut bytes = encode_authority_journal(
            &authority_identity_for_test("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
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
            &authority_identity_for_test("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee"),
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
        let root = std::env::temp_dir().join(format!(
            "audio-graph-fb2b-stage-map-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
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
            let case_root = root.join(format!("{cut:?}"));
            let file_system = Arc::new(ScriptedAtomicFileSystem::new(cut, Some(b"old-journal")));
            let boundary =
                FileAuthorityJournalBoundary::with_file_system(case_root, file_system.clone());

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

        let success_root = root.join("success");
        let file_system = Arc::new(ScriptedAtomicFileSystem::new(AtomicFileCut::None, None));
        let boundary =
            FileAuthorityJournalBoundary::with_file_system(success_root, file_system.clone());
        assert_eq!(boundary.replace(replacement), Ok(()));
        assert_eq!(
            file_system.published().as_deref(),
            Some(replacement.as_slice())
        );

        std::fs::remove_dir_all(&root).expect("remove isolated stage-map test directory");
    }

    #[test]
    fn ordinary_journal_read_failure_remains_unavailable() {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-fb2b-read-failure-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        let file_system = Arc::new(ScriptedAtomicFileSystem::new(AtomicFileCut::Readback, None));
        let boundary = FileAuthorityJournalBoundary::with_file_system(root, file_system);

        assert_eq!(boundary.read(), Err(CredentialStoreFailure::Unavailable));
    }

    #[cfg(unix)]
    #[test]
    fn postpublication_permission_hardening_failure_is_commit_unknown() {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-fb2b-postcommit-permission-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        let replacement = b"published-before-permission-hardening";
        let file_system = Arc::new(ScriptedAtomicFileSystem::new(
            AtomicFileCut::PostCommitPermission,
            Some(b"old-journal"),
        ));
        let boundary =
            FileAuthorityJournalBoundary::with_file_system(root.clone(), file_system.clone());

        assert_eq!(
            boundary.replace(replacement),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        assert_eq!(
            file_system.published().as_deref(),
            Some(replacement.as_slice()),
            "publication occurred before permission hardening failed"
        );
        assert!(root.join(JOURNAL_FILE_NAME).is_dir());

        std::fs::remove_dir_all(&root).expect("remove postcommit permission test root");
    }

    #[cfg(unix)]
    #[test]
    fn unix_wrong_type_and_symlink_hardening_returns_typed_permission_failure() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!(
            "audio-graph-fb2b-permission-denials-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        std::fs::create_dir_all(&base).expect("create isolated permission denial base");

        let target = base.join("root-symlink-target");
        let root_symlink = base.join("root-symlink");
        std::fs::create_dir(&target).expect("create root symlink target");
        symlink(&target, &root_symlink).expect("create root symlink");
        assert!(matches!(
            FileAuthorityJournalBoundary::new(&root_symlink).acquire_mutation_lock(),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        ));

        let lock_root = base.join("wrong-lock-type");
        std::fs::create_dir_all(lock_root.join(MUTATION_LOCK_FILE_NAME))
            .expect("create directory at lock path");
        assert!(matches!(
            FileAuthorityJournalBoundary::new(&lock_root).acquire_mutation_lock(),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        ));

        let journal_root = base.join("journal-symlink");
        std::fs::create_dir_all(&journal_root).expect("create journal symlink root");
        let journal_target = journal_root.join("journal-target");
        std::fs::write(&journal_target, b"target").expect("create journal symlink target");
        symlink(&journal_target, journal_root.join(JOURNAL_FILE_NAME))
            .expect("create journal symlink");
        assert_eq!(
            FileAuthorityJournalBoundary::new(&journal_root).read(),
            Err(CredentialStoreFailure::PermissionHardeningFailed)
        );

        std::fs::remove_dir_all(&base).expect("remove isolated permission denial base");
    }

    #[cfg(unix)]
    #[test]
    fn unix_journal_root_state_and_lock_are_owner_only_and_broad_modes_are_corrected() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "audio-graph-fb2b-permissions-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        std::fs::create_dir_all(&root).expect("create isolated permission test root");
        let journal_path = root.join(JOURNAL_FILE_NAME);
        let lock_path = root.join(MUTATION_LOCK_FILE_NAME);
        std::fs::write(&journal_path, b"old-journal").expect("create broad journal");
        std::fs::write(&lock_path, b"").expect("create broad lock");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o777))
            .expect("broaden root mode");
        std::fs::set_permissions(&journal_path, std::fs::Permissions::from_mode(0o666))
            .expect("broaden journal mode");
        std::fs::set_permissions(&lock_path, std::fs::Permissions::from_mode(0o666))
            .expect("broaden lock mode");

        let boundary = FileAuthorityJournalBoundary::new(&root);
        let guard = boundary
            .acquire_mutation_lock()
            .expect("acquire hardened mutation lock");
        boundary
            .replace(b"new-journal")
            .expect("replace hardened journal");

        let mode = |path: &Path| {
            std::fs::metadata(path)
                .expect("permission metadata")
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&journal_path), 0o600);
        assert_eq!(mode(&lock_path), 0o600);

        drop(guard);
        std::fs::remove_dir_all(&root).expect("remove isolated permission test directory");
    }

    #[test]
    fn stable_file_lock_contends_until_the_owned_guard_is_dropped() {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-fb2b-lock-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        let first_boundary = FileAuthorityJournalBoundary::new(&root);
        let second_boundary = FileAuthorityJournalBoundary::new(&root);

        let first_guard = first_boundary
            .acquire_mutation_lock()
            .expect("first process-shaped owner acquires lock");
        assert!(matches!(
            second_boundary.acquire_mutation_lock(),
            Err(CredentialStoreFailure::OperationInProgress)
        ));
        assert!(root.join(MUTATION_LOCK_FILE_NAME).is_file());

        drop(first_guard);
        let second_guard = second_boundary
            .acquire_mutation_lock()
            .expect("lock releases with the owned guard");
        drop(second_guard);
        assert!(root.join(MUTATION_LOCK_FILE_NAME).is_file());

        std::fs::remove_dir_all(&root).expect("remove isolated lock test directory");
    }

    #[test]
    fn file_lock_child_harness() {
        let Ok(mode) = std::env::var("AUDIO_GRAPH_FB2B_LOCK_CHILD_MODE") else {
            return;
        };
        let root = PathBuf::from(
            std::env::var_os("AUDIO_GRAPH_FB2B_LOCK_CHILD_ROOT")
                .expect("child lock root environment"),
        );
        let boundary = FileAuthorityJournalBoundary::new(&root);

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
                use std::os::unix::fs::PermissionsExt;

                rustix::process::umask(rustix::fs::Mode::empty());
                FileAuthorityJournalBoundary::create_root_owner_only(&root)
                    .expect("create owner-only root under child umask");
                assert_eq!(
                    std::fs::metadata(&root)
                        .expect("new root metadata")
                        .permissions()
                        .mode()
                        & 0o777,
                    0o700,
                    "root must be private before final hardening"
                );
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

    fn lock_child_command(mode: &str, root: &Path) -> Command {
        let mut command = Command::new(std::env::current_exe().expect("current test executable"));
        command
            .arg("file_lock_child_harness")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env("AUDIO_GRAPH_FB2B_LOCK_CHILD_MODE", mode)
            .env("AUDIO_GRAPH_FB2B_LOCK_CHILD_ROOT", root);
        command
    }

    #[cfg(unix)]
    #[test]
    fn unix_owner_only_paths_survive_a_permissive_child_umask() {
        use std::os::unix::fs::PermissionsExt;

        let root = std::env::temp_dir().join(format!(
            "audio-graph-fb2b-child-umask-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        let output = lock_child_command("create_owner_only", &root)
            .output()
            .expect("run permissive-umask child");
        assert!(
            output.status.success(),
            "permissive-umask child failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let mode = |path: &Path| {
            std::fs::metadata(path)
                .expect("permission metadata")
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(mode(&root), 0o700);
        assert_eq!(mode(&root.join(JOURNAL_FILE_NAME)), 0o600);
        assert_eq!(mode(&root.join(MUTATION_LOCK_FILE_NAME)), 0o600);

        std::fs::remove_dir_all(&root).expect("remove permissive-umask child root");
    }

    #[test]
    fn file_lock_contends_across_processes_and_releases_when_owner_is_killed() {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-fb2b-child-lock-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        let mut owner = lock_child_command("owner", &root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn lock owner child");
        let mut owner_output =
            std::io::BufReader::new(owner.stdout.take().expect("owner child stdout pipe"));
        loop {
            let mut line = String::new();
            assert_ne!(
                owner_output
                    .read_line(&mut line)
                    .expect("read owner readiness"),
                0,
                "owner exited before acquiring the lock"
            );
            if line.contains("AUDIO_GRAPH_FB2B_LOCKED") {
                break;
            }
        }

        let contender = lock_child_command("contend", &root)
            .output()
            .expect("run lock contender child");
        assert!(
            contender.status.success(),
            "contender child failed: {}",
            String::from_utf8_lossy(&contender.stderr)
        );

        owner.kill().expect("kill lock owner child");
        owner.wait().expect("reap killed lock owner child");

        let reacquirer = lock_child_command("acquire", &root)
            .output()
            .expect("run lock reacquirer child");
        assert!(
            reacquirer.status.success(),
            "reacquirer child failed: {}",
            String::from_utf8_lossy(&reacquirer.stderr)
        );

        std::fs::remove_dir_all(&root).expect("remove isolated child-lock test directory");
    }
}
