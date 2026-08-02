use crate::credentials::domain::CredentialStoreFailure;
use audio_graph_ipc_contract::credential_contract::{CredentialOperationId, CredentialSetId};
use std::collections::HashMap;
use std::sync::Arc;
use zeroize::{Zeroize, Zeroizing};

const CREDENTIAL_SERVICE: &str = "com.codeseys.audiograph.credentials";
const WINDOWS_TARGET_PREFIX: &str = "Codeseys.AudioGraph.Credentials/";
const AUTHORITY_ACCOUNT: &str = "v2/_authority";

pub(super) struct EntryLocator {
    account: String,
    windows_target: String,
}

impl EntryLocator {
    pub(super) fn active(set_id: &CredentialSetId) -> Self {
        Self::from_account(format!("v2/{}", set_id.as_str()))
    }

    pub(super) fn staging(operation_id: &CredentialOperationId, set_id: &CredentialSetId) -> Self {
        Self::from_account(format!(
            "v2-staging/{}/{}",
            operation_id.as_str(),
            set_id.as_str()
        ))
    }

    pub(super) fn authority() -> Self {
        Self::from_account(AUTHORITY_ACCOUNT.to_owned())
    }

    fn from_account(account: String) -> Self {
        let windows_target = format!("{WINDOWS_TARGET_PREFIX}{account}");
        Self {
            account,
            windows_target,
        }
    }

    pub(super) fn service(&self) -> &'static str {
        CREDENTIAL_SERVICE
    }

    pub(super) fn account(&self) -> &str {
        &self.account
    }

    pub(super) fn windows_target(&self) -> &str {
        &self.windows_target
    }

    fn windows_modifiers(&self) -> HashMap<&'static str, &str> {
        HashMap::from([("target", self.windows_target()), ("persistence", "local")])
    }
}

pub(super) struct KeyringBoundaryFailure {
    error: keyring::Error,
    invocation_started: bool,
}

impl KeyringBoundaryFailure {
    pub(super) fn before_invocation(error: keyring::Error) -> Self {
        Self {
            error,
            invocation_started: false,
        }
    }

    pub(super) fn after_invocation(error: keyring::Error) -> Self {
        Self {
            error,
            invocation_started: true,
        }
    }
}

pub(super) trait KeyringBoundary: Send + Sync {
    fn get_secret(&self, locator: &EntryLocator) -> Result<Vec<u8>, KeyringBoundaryFailure>;
    fn set_secret(
        &self,
        locator: &EntryLocator,
        secret: &[u8],
    ) -> Result<(), KeyringBoundaryFailure>;
    fn delete_credential(&self, locator: &EntryLocator) -> Result<(), KeyringBoundaryFailure>;
}

struct SystemKeyringBoundary;

impl SystemKeyringBoundary {
    fn entry(locator: &EntryLocator) -> Result<keyring::Entry, KeyringBoundaryFailure> {
        #[cfg(target_os = "windows")]
        {
            // Initialize keyring's selected Windows store, then use its
            // keyring-core facade so v2 can freeze both target and persistence.
            let _ = keyring::Entry::new(locator.service(), locator.account())
                .map_err(KeyringBoundaryFailure::before_invocation)?;
            let modifiers = locator.windows_modifiers();
            let inner = keyring_core::Entry::new_with_modifiers(
                locator.service(),
                locator.account(),
                &modifiers,
            )
            .map_err(KeyringBoundaryFailure::before_invocation)?;
            Ok(keyring::Entry { inner })
        }

        #[cfg(not(target_os = "windows"))]
        {
            keyring::Entry::new(locator.service(), locator.account())
                .map_err(KeyringBoundaryFailure::before_invocation)
        }
    }
}

impl KeyringBoundary for SystemKeyringBoundary {
    fn get_secret(&self, locator: &EntryLocator) -> Result<Vec<u8>, KeyringBoundaryFailure> {
        Self::entry(locator)?
            .get_secret()
            .map_err(KeyringBoundaryFailure::after_invocation)
    }

    fn set_secret(
        &self,
        locator: &EntryLocator,
        secret: &[u8],
    ) -> Result<(), KeyringBoundaryFailure> {
        Self::entry(locator)?
            .set_secret(secret)
            .map_err(KeyringBoundaryFailure::after_invocation)
    }

    fn delete_credential(&self, locator: &EntryLocator) -> Result<(), KeyringBoundaryFailure> {
        Self::entry(locator)?
            .delete_credential()
            .map_err(KeyringBoundaryFailure::after_invocation)
    }
}

pub(super) struct KeyringEntryAdapter {
    boundary: Arc<dyn KeyringBoundary>,
}

impl KeyringEntryAdapter {
    pub(super) fn production() -> Self {
        Self::new(Arc::new(SystemKeyringBoundary))
    }

    pub(super) fn new(boundary: Arc<dyn KeyringBoundary>) -> Self {
        Self { boundary }
    }

    pub(super) fn read(
        &self,
        locator: &EntryLocator,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, CredentialStoreFailure> {
        match self.boundary.get_secret(locator) {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(KeyringBoundaryFailure {
                error: keyring::Error::NoEntry,
                ..
            }) => Ok(None),
            Err(failure) => Err(map_read_failure(failure)),
        }
    }

    pub(super) fn write(
        &self,
        locator: &EntryLocator,
        secret: &[u8],
    ) -> Result<(), CredentialStoreFailure> {
        self.boundary
            .set_secret(locator, secret)
            .map_err(map_write_failure)
    }

    pub(super) fn write_authority(
        &self,
        locator: &EntryLocator,
        secret: &[u8],
    ) -> Result<(), CredentialStoreFailure> {
        self.boundary
            .set_secret(locator, secret)
            .map_err(map_authority_write_failure)
    }

    pub(super) fn delete_and_verify_absent(
        &self,
        locator: &EntryLocator,
    ) -> Result<(), CredentialStoreFailure> {
        match self.boundary.delete_credential(locator) {
            Ok(())
            | Err(KeyringBoundaryFailure {
                error: keyring::Error::NoEntry,
                ..
            }) => {}
            Err(failure) => return Err(map_write_failure(failure)),
        }

        match self.boundary.get_secret(locator) {
            Err(KeyringBoundaryFailure {
                error: keyring::Error::NoEntry,
                ..
            }) => Ok(()),
            Ok(mut secret) => {
                secret.zeroize();
                Err(CredentialStoreFailure::CommitUnknown)
            }
            Err(mut failure) => {
                zeroize_owned_error_payloads(&mut failure.error);
                Err(CredentialStoreFailure::CommitUnknown)
            }
        }
    }
}

fn map_authority_write_failure(mut failure: KeyringBoundaryFailure) -> CredentialStoreFailure {
    if failure.invocation_started {
        zeroize_owned_error_payloads(&mut failure.error);
        CredentialStoreFailure::CommitUnknown
    } else {
        map_write_failure(failure)
    }
}

fn zeroize_owned_error_payloads(error: &mut keyring::Error) {
    match error {
        keyring::Error::BadEncoding(bytes) | keyring::Error::BadDataFormat(bytes, _) => {
            bytes.zeroize();
        }
        keyring::Error::BadStoreFormat(value)
        | keyring::Error::TooLong(value, _)
        | keyring::Error::NotSupportedByStore(value) => {
            value.zeroize();
        }
        keyring::Error::Invalid(field, reason) => {
            field.zeroize();
            reason.zeroize();
        }
        _ => {}
    }
}

fn map_read_failure(failure: KeyringBoundaryFailure) -> CredentialStoreFailure {
    match failure.error {
        keyring::Error::NoEntry => CredentialStoreFailure::Missing,
        keyring::Error::NoStorageAccess(_) | keyring::Error::NoDefaultStore => {
            CredentialStoreFailure::Unavailable
        }
        keyring::Error::BadEncoding(mut bytes) | keyring::Error::BadDataFormat(mut bytes, _) => {
            bytes.zeroize();
            CredentialStoreFailure::CorruptRecord
        }
        keyring::Error::BadStoreFormat(mut value) => {
            value.zeroize();
            CredentialStoreFailure::CorruptRecord
        }
        keyring::Error::TooLong(mut field, _) => {
            field.zeroize();
            CredentialStoreFailure::PayloadTooLarge
        }
        keyring::Error::Invalid(mut field, mut reason) => {
            field.zeroize();
            reason.zeroize();
            CredentialStoreFailure::Internal
        }
        keyring::Error::Ambiguous(_) => CredentialStoreFailure::AmbiguousMatch,
        keyring::Error::NotSupportedByStore(mut reason) => {
            reason.zeroize();
            CredentialStoreFailure::Unsupported
        }
        keyring::Error::PlatformFailure(_) => CredentialStoreFailure::Internal,
        _ => CredentialStoreFailure::Internal,
    }
}

fn map_write_failure(failure: KeyringBoundaryFailure) -> CredentialStoreFailure {
    let invocation_started = failure.invocation_started;
    match failure.error {
        keyring::Error::NoStorageAccess(_) | keyring::Error::NoDefaultStore => {
            CredentialStoreFailure::Unavailable
        }
        keyring::Error::BadEncoding(mut bytes) | keyring::Error::BadDataFormat(mut bytes, _) => {
            bytes.zeroize();
            CredentialStoreFailure::CorruptRecord
        }
        keyring::Error::BadStoreFormat(mut value) => {
            value.zeroize();
            CredentialStoreFailure::CorruptRecord
        }
        keyring::Error::TooLong(mut field, _) => {
            field.zeroize();
            CredentialStoreFailure::PayloadTooLarge
        }
        keyring::Error::Invalid(mut field, mut reason) => {
            field.zeroize();
            reason.zeroize();
            CredentialStoreFailure::Internal
        }
        keyring::Error::Ambiguous(_) => CredentialStoreFailure::AmbiguousMatch,
        keyring::Error::NotSupportedByStore(mut reason) => {
            reason.zeroize();
            CredentialStoreFailure::Unsupported
        }
        keyring::Error::NoEntry if !invocation_started => CredentialStoreFailure::Missing,
        _ if invocation_started => CredentialStoreFailure::CommitUnknown,
        _ => CredentialStoreFailure::Internal,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        EntryLocator, KeyringBoundary, KeyringBoundaryFailure, KeyringEntryAdapter,
        zeroize_owned_error_payloads,
    };
    use crate::credentials::domain::CredentialStoreFailure;
    use audio_graph_ipc_contract::credential_contract::{BuiltInCredentialSetId, CredentialSetId};
    use std::collections::HashMap;
    use std::fmt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct MemoryKeyringBoundary {
        entries: Mutex<HashMap<(String, String), Vec<u8>>>,
    }

    impl KeyringBoundary for MemoryKeyringBoundary {
        fn get_secret(&self, locator: &EntryLocator) -> Result<Vec<u8>, KeyringBoundaryFailure> {
            self.entries
                .lock()
                .expect("memory keyring lock")
                .get(&(locator.service().to_owned(), locator.account().to_owned()))
                .cloned()
                .ok_or_else(|| KeyringBoundaryFailure::after_invocation(keyring::Error::NoEntry))
        }

        fn set_secret(
            &self,
            locator: &EntryLocator,
            secret: &[u8],
        ) -> Result<(), KeyringBoundaryFailure> {
            self.entries.lock().expect("memory keyring lock").insert(
                (locator.service().to_owned(), locator.account().to_owned()),
                secret.to_vec(),
            );
            Ok(())
        }

        fn delete_credential(&self, locator: &EntryLocator) -> Result<(), KeyringBoundaryFailure> {
            self.entries
                .lock()
                .expect("memory keyring lock")
                .remove(&(locator.service().to_owned(), locator.account().to_owned()))
                .map(|_| ())
                .ok_or_else(|| KeyringBoundaryFailure::after_invocation(keyring::Error::NoEntry))
        }
    }

    struct ReadFailureBoundary(Mutex<Option<keyring::Error>>);

    impl KeyringBoundary for ReadFailureBoundary {
        fn get_secret(&self, _locator: &EntryLocator) -> Result<Vec<u8>, KeyringBoundaryFailure> {
            let error = self
                .0
                .lock()
                .expect("failure boundary lock")
                .take()
                .expect("one scripted read failure");
            Err(KeyringBoundaryFailure::after_invocation(error))
        }

        fn set_secret(
            &self,
            _locator: &EntryLocator,
            _secret: &[u8],
        ) -> Result<(), KeyringBoundaryFailure> {
            unreachable!("read-only failure boundary")
        }

        fn delete_credential(&self, _locator: &EntryLocator) -> Result<(), KeyringBoundaryFailure> {
            unreachable!("read-only failure boundary")
        }
    }

    struct StickyDeleteBoundary;

    impl KeyringBoundary for StickyDeleteBoundary {
        fn get_secret(&self, _locator: &EntryLocator) -> Result<Vec<u8>, KeyringBoundaryFailure> {
            Ok(b"still-present-after-delete".to_vec())
        }

        fn set_secret(
            &self,
            _locator: &EntryLocator,
            _secret: &[u8],
        ) -> Result<(), KeyringBoundaryFailure> {
            unreachable!("delete-only boundary")
        }

        fn delete_credential(&self, _locator: &EntryLocator) -> Result<(), KeyringBoundaryFailure> {
            // Models the Apple legacy adapter discarding the native delete
            // result and reporting success while the entry remains.
            Ok(())
        }
    }

    struct DeleteReadFailureBoundary {
        error: Mutex<Option<keyring::Error>>,
    }

    impl KeyringBoundary for DeleteReadFailureBoundary {
        fn get_secret(&self, _locator: &EntryLocator) -> Result<Vec<u8>, KeyringBoundaryFailure> {
            Err(KeyringBoundaryFailure::after_invocation(
                self.error
                    .lock()
                    .expect("delete read failure lock")
                    .take()
                    .expect("one scripted delete read failure"),
            ))
        }

        fn set_secret(
            &self,
            _locator: &EntryLocator,
            _secret: &[u8],
        ) -> Result<(), KeyringBoundaryFailure> {
            unreachable!("delete-only failure boundary")
        }

        fn delete_credential(&self, _locator: &EntryLocator) -> Result<(), KeyringBoundaryFailure> {
            Ok(())
        }
    }

    struct WriteFailureBoundary(Mutex<Option<keyring::Error>>);

    impl KeyringBoundary for WriteFailureBoundary {
        fn get_secret(&self, _locator: &EntryLocator) -> Result<Vec<u8>, KeyringBoundaryFailure> {
            unreachable!("write-only failure boundary")
        }

        fn set_secret(
            &self,
            _locator: &EntryLocator,
            _secret: &[u8],
        ) -> Result<(), KeyringBoundaryFailure> {
            let error = self
                .0
                .lock()
                .expect("failure boundary lock")
                .take()
                .expect("one scripted write failure");
            Err(KeyringBoundaryFailure::after_invocation(error))
        }

        fn delete_credential(&self, _locator: &EntryLocator) -> Result<(), KeyringBoundaryFailure> {
            unreachable!("write-only failure boundary")
        }
    }

    struct FormatObservedPlatformError {
        format_count: Arc<AtomicUsize>,
    }

    impl fmt::Debug for FormatObservedPlatformError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.format_count.fetch_add(1, Ordering::SeqCst);
            formatter.write_str("native-error-canary")
        }
    }

    impl fmt::Display for FormatObservedPlatformError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.format_count.fetch_add(1, Ordering::SeqCst);
            formatter.write_str("native-error-canary")
        }
    }

    impl std::error::Error for FormatObservedPlatformError {}

    #[test]
    fn active_locator_and_binary_round_trip_are_exact() {
        let boundary = Arc::new(MemoryKeyringBoundary::default());
        let adapter = KeyringEntryAdapter::new(boundary);
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        let locator = EntryLocator::active(&set_id);

        assert_eq!(locator.service(), "com.codeseys.audiograph.credentials");
        assert_eq!(locator.account(), "v2/openai");
        assert_eq!(
            locator.windows_target(),
            "Codeseys.AudioGraph.Credentials/v2/openai"
        );

        let binary = [0_u8, 0x80, 0xff, b'\n'];
        adapter
            .write(&locator, &binary)
            .expect("write binary record");
        let stored = adapter
            .read(&locator)
            .expect("read binary record")
            .expect("record exists");

        assert_eq!(stored.as_slice(), binary.as_slice());
    }

    #[test]
    fn staging_authority_and_windows_creation_literals_are_exact() {
        let operation_id =
            audio_graph_ipc_contract::credential_contract::CredentialOperationId::parse(
                "11111111-2222-3333-4444-555555555555",
            )
            .expect("canonical operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        let staging = EntryLocator::staging(&operation_id, &set_id);
        let authority = EntryLocator::authority();

        assert_eq!(
            staging.account(),
            "v2-staging/11111111-2222-3333-4444-555555555555/openai"
        );
        assert_eq!(
            staging.windows_target(),
            "Codeseys.AudioGraph.Credentials/v2-staging/11111111-2222-3333-4444-555555555555/openai"
        );
        assert_eq!(authority.account(), "v2/_authority");
        assert_eq!(
            authority.windows_target(),
            "Codeseys.AudioGraph.Credentials/v2/_authority"
        );

        let modifiers = staging.windows_modifiers();
        assert_eq!(modifiers.len(), 2);
        assert_eq!(modifiers.get("target"), Some(&staging.windows_target()));
        assert_eq!(modifiers.get("persistence"), Some(&"local"));
    }

    #[test]
    fn ambiguous_native_match_stays_distinct_through_the_entry_seam() {
        let adapter = KeyringEntryAdapter::new(Arc::new(ReadFailureBoundary(Mutex::new(Some(
            keyring::Error::Ambiguous(Vec::new()),
        )))));
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);

        assert_eq!(
            adapter.read(&EntryLocator::active(&set_id)),
            Err(CredentialStoreFailure::AmbiguousMatch)
        );
    }

    #[test]
    fn missing_staging_delete_is_idempotent_but_false_success_is_commit_unknown() {
        let operation_id =
            audio_graph_ipc_contract::credential_contract::CredentialOperationId::parse(
                "11111111-2222-3333-4444-555555555555",
            )
            .expect("canonical operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        let staging = EntryLocator::staging(&operation_id, &set_id);

        let missing = KeyringEntryAdapter::new(Arc::new(MemoryKeyringBoundary::default()));
        assert_eq!(missing.delete_and_verify_absent(&staging), Ok(()));

        let sticky = KeyringEntryAdapter::new(Arc::new(StickyDeleteBoundary));
        assert_eq!(
            sticky.delete_and_verify_absent(&staging),
            Err(CredentialStoreFailure::CommitUnknown)
        );
    }

    #[test]
    fn operation_aware_variant_mapping_never_formats_native_errors_or_bytes() {
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        let locator = EntryLocator::active(&set_id);
        let read_format_count = Arc::new(AtomicUsize::new(0));
        let read_adapter =
            KeyringEntryAdapter::new(Arc::new(ReadFailureBoundary(Mutex::new(Some(
                keyring::Error::PlatformFailure(Box::new(FormatObservedPlatformError {
                    format_count: read_format_count.clone(),
                })),
            )))));
        let read_failure = match read_adapter.read(&locator) {
            Err(failure) => failure,
            Ok(_) => panic!("scripted native read must fail"),
        };
        assert_eq!(read_failure, CredentialStoreFailure::Internal);
        assert_eq!(read_format_count.load(Ordering::SeqCst), 0);

        let write_format_count = Arc::new(AtomicUsize::new(0));
        let write_adapter =
            KeyringEntryAdapter::new(Arc::new(WriteFailureBoundary(Mutex::new(Some(
                keyring::Error::PlatformFailure(Box::new(FormatObservedPlatformError {
                    format_count: write_format_count.clone(),
                })),
            )))));
        assert_eq!(
            write_adapter.write(&locator, b"opaque-record"),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        assert_eq!(write_format_count.load(Ordering::SeqCst), 0);

        let access_format_count = Arc::new(AtomicUsize::new(0));
        let access_adapter =
            KeyringEntryAdapter::new(Arc::new(ReadFailureBoundary(Mutex::new(Some(
                keyring::Error::NoStorageAccess(Box::new(FormatObservedPlatformError {
                    format_count: access_format_count.clone(),
                })),
            )))));
        assert!(matches!(
            access_adapter.read(&locator),
            Err(CredentialStoreFailure::Unavailable)
        ));
        assert_eq!(access_format_count.load(Ordering::SeqCst), 0);

        let byte_canary = b"bad-data-byte-canary".to_vec();
        let bytes_adapter = KeyringEntryAdapter::new(Arc::new(ReadFailureBoundary(Mutex::new(
            Some(keyring::Error::BadEncoding(byte_canary)),
        ))));
        let bytes_failure = match bytes_adapter.read(&locator) {
            Err(failure) => failure,
            Ok(_) => panic!("scripted bad bytes must fail"),
        };
        assert_eq!(bytes_failure, CredentialStoreFailure::CorruptRecord);
        assert!(!format!("{bytes_failure:?}").contains("bad-data-byte-canary"));
    }

    #[test]
    fn discarded_native_error_byte_payloads_are_zeroized_in_place() {
        let mut error = keyring::Error::BadEncoding(b"delete-read-secret-canary".to_vec());

        zeroize_owned_error_payloads(&mut error);

        match error {
            keyring::Error::BadEncoding(bytes) => {
                assert!(bytes.iter().all(|byte| *byte == 0));
            }
            _ => panic!("expected bad-encoding error"),
        }

        let mut error = keyring::Error::BadDataFormat(
            b"delete-read-record-canary".to_vec(),
            Box::new(FormatObservedPlatformError {
                format_count: Arc::new(AtomicUsize::new(0)),
            }),
        );
        zeroize_owned_error_payloads(&mut error);
        match error {
            keyring::Error::BadDataFormat(bytes, _) => {
                assert!(bytes.iter().all(|byte| *byte == 0));
            }
            _ => panic!("expected bad-data-format error"),
        }
    }

    #[test]
    fn delete_readback_failure_is_scrubbed_and_collapsed_to_commit_unknown() {
        let format_count = Arc::new(AtomicUsize::new(0));
        let adapter = KeyringEntryAdapter::new(Arc::new(DeleteReadFailureBoundary {
            error: Mutex::new(Some(keyring::Error::BadDataFormat(
                b"deleted-secret-record-canary".to_vec(),
                Box::new(FormatObservedPlatformError {
                    format_count: format_count.clone(),
                }),
            ))),
        }));
        let operation_id =
            audio_graph_ipc_contract::credential_contract::CredentialOperationId::parse(
                "11111111-2222-3333-4444-555555555555",
            )
            .expect("canonical operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);

        assert_eq!(
            adapter.delete_and_verify_absent(&EntryLocator::staging(&operation_id, &set_id)),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        assert_eq!(format_count.load(Ordering::SeqCst), 0);
    }
}
