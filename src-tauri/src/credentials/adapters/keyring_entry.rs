use super::native_interaction::{
    ForbidPrompt, MutationInvocation, NativeInteractionLease, ensure_forbid_prompt,
};
use crate::credentials::domain::CredentialStoreFailure;
use audio_graph_ipc_contract::credential_contract::{CredentialOperationId, CredentialSetId};
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
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

pub(super) trait KeyringBoundary: Send + Sync {
    fn get_secret(
        &self,
        prompt: &ForbidPrompt<'_>,
        locator: &EntryLocator,
    ) -> Result<Vec<u8>, CredentialStoreFailure>;
    fn set_secret(
        &self,
        prompt: &ForbidPrompt<'_>,
        invocation: &mut MutationInvocation<'_>,
        locator: &EntryLocator,
        secret: &[u8],
    ) -> Result<(), CredentialStoreFailure>;
    fn delete_credential(
        &self,
        prompt: &ForbidPrompt<'_>,
        invocation: &mut MutationInvocation<'_>,
        locator: &EntryLocator,
    ) -> Result<(), CredentialStoreFailure>;
}

struct SystemKeyringBoundary;

impl SystemKeyringBoundary {
    fn entry(locator: &EntryLocator) -> Result<keyring::Entry, keyring::Error> {
        #[cfg(target_os = "windows")]
        {
            // Initialize keyring's selected Windows store, then use its
            // keyring-core facade so v2 can freeze both target and persistence.
            let _ = keyring::Entry::new(locator.service(), locator.account())?;
            let modifiers = locator.windows_modifiers();
            let inner = keyring_core::Entry::new_with_modifiers(
                locator.service(),
                locator.account(),
                &modifiers,
            )?;
            Ok(keyring::Entry { inner })
        }

        #[cfg(not(target_os = "windows"))]
        {
            keyring::Entry::new(locator.service(), locator.account())
        }
    }

    fn map_read_error(error: keyring::Error) -> CredentialStoreFailure {
        match error {
            keyring::Error::NoEntry => CredentialStoreFailure::Missing,
            keyring::Error::NoStorageAccess(_) | keyring::Error::NoDefaultStore => {
                CredentialStoreFailure::Unavailable
            }
            keyring::Error::BadEncoding(mut bytes)
            | keyring::Error::BadDataFormat(mut bytes, _) => {
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

    fn map_write_error(error: keyring::Error, invocation_started: bool) -> CredentialStoreFailure {
        match error {
            keyring::Error::NoStorageAccess(_) | keyring::Error::NoDefaultStore => {
                CredentialStoreFailure::Unavailable
            }
            keyring::Error::BadEncoding(mut bytes)
            | keyring::Error::BadDataFormat(mut bytes, _) => {
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

    fn map_delete_error(error: keyring::Error) -> CredentialStoreFailure {
        if matches!(error, keyring::Error::NoEntry) {
            CredentialStoreFailure::Missing
        } else {
            Self::map_write_error(error, true)
        }
    }
}

impl KeyringBoundary for SystemKeyringBoundary {
    fn get_secret(
        &self,
        prompt: &ForbidPrompt<'_>,
        locator: &EntryLocator,
    ) -> Result<Vec<u8>, CredentialStoreFailure> {
        ensure_forbid_prompt(prompt)?;
        Self::entry(locator)
            .map_err(Self::map_read_error)?
            .get_secret()
            .map_err(Self::map_read_error)
    }

    fn set_secret(
        &self,
        prompt: &ForbidPrompt<'_>,
        invocation: &mut MutationInvocation<'_>,
        locator: &EntryLocator,
        secret: &[u8],
    ) -> Result<(), CredentialStoreFailure> {
        ensure_forbid_prompt(prompt)?;
        let entry = Self::entry(locator).map_err(|error| Self::map_write_error(error, false))?;
        invocation.mark_started();
        entry
            .set_secret(secret)
            .map_err(|error| Self::map_write_error(error, true))
    }

    fn delete_credential(
        &self,
        prompt: &ForbidPrompt<'_>,
        invocation: &mut MutationInvocation<'_>,
        locator: &EntryLocator,
    ) -> Result<(), CredentialStoreFailure> {
        ensure_forbid_prompt(prompt)?;
        let entry = Self::entry(locator).map_err(|error| Self::map_write_error(error, false))?;
        invocation.mark_started();
        entry.delete_credential().map_err(Self::map_delete_error)
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
        lease: &mut NativeInteractionLease<'_>,
        locator: &EntryLocator,
    ) -> Result<Option<Zeroizing<Vec<u8>>>, CredentialStoreFailure> {
        let (prompt, invocation) = lease.mutation_capabilities();
        ensure_forbid_prompt(prompt)?;
        let result = catch_unwind(AssertUnwindSafe(|| {
            self.boundary.get_secret(prompt, locator)
        }));
        match result {
            Err(_) => Err(invocation.latch_uncertainty()),
            Ok(Ok(secret)) => Ok(Some(Zeroizing::new(secret))),
            Ok(Err(CredentialStoreFailure::Missing)) => Ok(None),
            Ok(Err(CredentialStoreFailure::StalledWorker)) => Err(invocation.latch_uncertainty()),
            Ok(Err(CredentialStoreFailure::CommitUnknown)) => {
                Err(invocation.latch_commit_unknown())
            }
            Ok(Err(failure)) => Err(failure),
        }
    }

    pub(super) fn write(
        &self,
        lease: &mut NativeInteractionLease<'_>,
        locator: &EntryLocator,
        secret: &[u8],
    ) -> Result<(), CredentialStoreFailure> {
        let (prompt, invocation) = lease.mutation_capabilities();
        invocation.prepare()?;
        match catch_unwind(AssertUnwindSafe(|| {
            self.boundary
                .set_secret(prompt, invocation, locator, secret)
        })) {
            Ok(Err(CredentialStoreFailure::CommitUnknown)) => {
                Err(invocation.latch_commit_unknown())
            }
            Ok(Err(CredentialStoreFailure::StalledWorker)) | Err(_) => {
                Err(invocation.latch_uncertainty())
            }
            Ok(result) => result,
        }
    }

    pub(super) fn write_authority(
        &self,
        lease: &mut NativeInteractionLease<'_>,
        locator: &EntryLocator,
        secret: &[u8],
    ) -> Result<(), CredentialStoreFailure> {
        let (prompt, invocation) = lease.mutation_capabilities();
        invocation.prepare()?;
        let result = match catch_unwind(AssertUnwindSafe(|| {
            self.boundary
                .set_secret(prompt, invocation, locator, secret)
        })) {
            Ok(Err(CredentialStoreFailure::CommitUnknown)) => {
                Err(invocation.latch_commit_unknown())
            }
            Ok(Err(CredentialStoreFailure::StalledWorker)) | Err(_) => {
                Err(invocation.latch_uncertainty())
            }
            Ok(result) => result,
        };
        match result {
            Err(_) if invocation.has_started() => Err(invocation.latch_uncertainty()),
            result => result,
        }
    }

    pub(super) fn delete_and_verify_absent(
        &self,
        lease: &mut NativeInteractionLease<'_>,
        locator: &EntryLocator,
    ) -> Result<(), CredentialStoreFailure> {
        let (prompt, invocation) = lease.mutation_capabilities();
        invocation.prepare()?;
        let deleted = catch_unwind(AssertUnwindSafe(|| {
            self.boundary.delete_credential(prompt, invocation, locator)
        }));
        match deleted {
            Ok(Err(CredentialStoreFailure::CommitUnknown)) => {
                return Err(invocation.latch_commit_unknown());
            }
            Err(_) | Ok(Err(CredentialStoreFailure::StalledWorker)) => {
                return Err(invocation.latch_uncertainty());
            }
            Ok(Ok(())) | Ok(Err(CredentialStoreFailure::Missing)) => {}
            Ok(Err(failure)) => return Err(failure),
        }

        let readback = catch_unwind(AssertUnwindSafe(|| {
            self.boundary.get_secret(prompt, locator)
        }));
        match readback {
            Err(_) | Ok(Err(CredentialStoreFailure::StalledWorker)) => {
                Err(invocation.latch_uncertainty())
            }
            Ok(Err(CredentialStoreFailure::Missing)) => Ok(()),
            Ok(Ok(mut secret)) => {
                secret.zeroize();
                Err(invocation.latch_commit_unknown())
            }
            Ok(Err(_)) => Err(invocation.latch_commit_unknown()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EntryLocator, KeyringBoundary, KeyringEntryAdapter, SystemKeyringBoundary};
    use crate::credentials::adapters::native_interaction::{
        ForbidPrompt, MutationInvocation, NativeInteractionGate,
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
        fn get_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            locator: &EntryLocator,
        ) -> Result<Vec<u8>, CredentialStoreFailure> {
            self.entries
                .lock()
                .expect("memory keyring lock")
                .get(&(locator.service().to_owned(), locator.account().to_owned()))
                .cloned()
                .ok_or(CredentialStoreFailure::Missing)
        }

        fn set_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            invocation: &mut MutationInvocation<'_>,
            locator: &EntryLocator,
            secret: &[u8],
        ) -> Result<(), CredentialStoreFailure> {
            invocation.mark_started();
            self.entries.lock().expect("memory keyring lock").insert(
                (locator.service().to_owned(), locator.account().to_owned()),
                secret.to_vec(),
            );
            Ok(())
        }

        fn delete_credential(
            &self,
            _prompt: &ForbidPrompt<'_>,
            invocation: &mut MutationInvocation<'_>,
            locator: &EntryLocator,
        ) -> Result<(), CredentialStoreFailure> {
            invocation.mark_started();
            self.entries
                .lock()
                .expect("memory keyring lock")
                .remove(&(locator.service().to_owned(), locator.account().to_owned()))
                .map(|_| ())
                .ok_or(CredentialStoreFailure::Missing)
        }
    }

    struct ReadFailureBoundary(Mutex<Option<CredentialStoreFailure>>);

    impl KeyringBoundary for ReadFailureBoundary {
        fn get_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _locator: &EntryLocator,
        ) -> Result<Vec<u8>, CredentialStoreFailure> {
            let failure = self
                .0
                .lock()
                .expect("failure boundary lock")
                .take()
                .expect("one scripted read failure");
            Err(failure)
        }

        fn set_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
            _secret: &[u8],
        ) -> Result<(), CredentialStoreFailure> {
            unreachable!("read-only failure boundary")
        }

        fn delete_credential(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
        ) -> Result<(), CredentialStoreFailure> {
            unreachable!("read-only failure boundary")
        }
    }

    struct StickyDeleteBoundary {
        reads: Arc<AtomicUsize>,
        deletes: Arc<AtomicUsize>,
    }

    impl KeyringBoundary for StickyDeleteBoundary {
        fn get_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _locator: &EntryLocator,
        ) -> Result<Vec<u8>, CredentialStoreFailure> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(b"still-present-after-delete".to_vec())
        }

        fn set_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
            _secret: &[u8],
        ) -> Result<(), CredentialStoreFailure> {
            unreachable!("delete-only boundary")
        }

        fn delete_credential(
            &self,
            _prompt: &ForbidPrompt<'_>,
            invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
        ) -> Result<(), CredentialStoreFailure> {
            // Models the Apple legacy adapter discarding the native delete
            // result and reporting success while the entry remains.
            self.deletes.fetch_add(1, Ordering::SeqCst);
            invocation.mark_started();
            Ok(())
        }
    }

    struct DeleteReadFailureBoundary {
        format_count: Arc<AtomicUsize>,
        reads: Arc<AtomicUsize>,
        deletes: Arc<AtomicUsize>,
    }

    impl KeyringBoundary for DeleteReadFailureBoundary {
        fn get_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _locator: &EntryLocator,
        ) -> Result<Vec<u8>, CredentialStoreFailure> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Err(SystemKeyringBoundary::map_read_error(
                keyring::Error::BadDataFormat(
                    b"deleted-secret-record-canary".to_vec(),
                    Box::new(FormatObservedPlatformError {
                        format_count: self.format_count.clone(),
                    }),
                ),
            ))
        }

        fn set_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
            _secret: &[u8],
        ) -> Result<(), CredentialStoreFailure> {
            unreachable!("delete-only failure boundary")
        }

        fn delete_credential(
            &self,
            _prompt: &ForbidPrompt<'_>,
            invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
        ) -> Result<(), CredentialStoreFailure> {
            self.deletes.fetch_add(1, Ordering::SeqCst);
            invocation.mark_started();
            Ok(())
        }
    }

    struct WriteFailureBoundary {
        failure: Mutex<Option<CredentialStoreFailure>>,
        mark_started: bool,
        writes: Arc<AtomicUsize>,
    }

    impl KeyringBoundary for WriteFailureBoundary {
        fn get_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _locator: &EntryLocator,
        ) -> Result<Vec<u8>, CredentialStoreFailure> {
            unreachable!("write-only failure boundary")
        }

        fn set_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
            _secret: &[u8],
        ) -> Result<(), CredentialStoreFailure> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            if self.mark_started {
                invocation.mark_started();
            }
            let failure = self
                .failure
                .lock()
                .expect("failure boundary lock")
                .take()
                .expect("one scripted write failure");
            Err(failure)
        }

        fn delete_credential(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
        ) -> Result<(), CredentialStoreFailure> {
            unreachable!("write-only failure boundary")
        }
    }

    #[derive(Clone, Copy)]
    enum PanicPhase {
        Read,
        BeforeMutation,
        AfterMutation,
        ReadAfterMutation,
    }

    struct PanicBoundary {
        phase: PanicPhase,
        reads: Arc<AtomicUsize>,
        writes: Arc<AtomicUsize>,
    }

    impl KeyringBoundary for PanicBoundary {
        fn get_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _locator: &EntryLocator,
        ) -> Result<Vec<u8>, CredentialStoreFailure> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            match self.phase {
                PanicPhase::Read | PanicPhase::ReadAfterMutation => {
                    panic!("scripted native read panic canary")
                }
                PanicPhase::BeforeMutation | PanicPhase::AfterMutation => {
                    unreachable!("write-only panic boundary")
                }
            }
        }

        fn set_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
            _secret: &[u8],
        ) -> Result<(), CredentialStoreFailure> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            match self.phase {
                PanicPhase::BeforeMutation => panic!("scripted pre-invocation panic canary"),
                PanicPhase::AfterMutation => {
                    invocation.mark_started();
                    panic!("scripted post-invocation panic canary")
                }
                PanicPhase::ReadAfterMutation => {
                    invocation.mark_started();
                    Ok(())
                }
                PanicPhase::Read => unreachable!("read-only panic boundary"),
            }
        }

        fn delete_credential(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
        ) -> Result<(), CredentialStoreFailure> {
            unreachable!("panic boundary does not delete")
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
        let mut lease = NativeInteractionGate::isolated_for_test()
            .acquire()
            .expect("ordinary interaction lease");
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
            .write(&mut lease, &locator, &binary)
            .expect("write binary record");
        let stored = adapter
            .read(&mut lease, &locator)
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
            CredentialStoreFailure::AmbiguousMatch,
        )))));
        let mut lease = NativeInteractionGate::isolated_for_test()
            .acquire()
            .expect("ordinary interaction lease");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);

        assert_eq!(
            adapter.read(&mut lease, &EntryLocator::active(&set_id)),
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
        let gate = NativeInteractionGate::isolated_for_test();
        let mut lease = gate.acquire().expect("ordinary interaction lease");

        let missing = KeyringEntryAdapter::new(Arc::new(MemoryKeyringBoundary::default()));
        assert_eq!(
            missing.delete_and_verify_absent(&mut lease, &staging),
            Ok(())
        );

        let sticky_reads = Arc::new(AtomicUsize::new(0));
        let sticky_deletes = Arc::new(AtomicUsize::new(0));
        let sticky = KeyringEntryAdapter::new(Arc::new(StickyDeleteBoundary {
            reads: sticky_reads.clone(),
            deletes: sticky_deletes.clone(),
        }));
        assert_eq!(
            sticky.delete_and_verify_absent(&mut lease, &staging),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        drop(lease);

        let reads_after_uncertainty = sticky_reads.load(Ordering::SeqCst);
        let deletes_after_uncertainty = sticky_deletes.load(Ordering::SeqCst);
        assert!(matches!(
            gate.acquire(),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(sticky_reads.load(Ordering::SeqCst), reads_after_uncertainty);
        assert_eq!(
            sticky_deletes.load(Ordering::SeqCst),
            deletes_after_uncertainty
        );
    }

    #[test]
    fn operation_aware_variant_mapping_never_formats_native_errors_or_bytes() {
        let read_format_count = Arc::new(AtomicUsize::new(0));
        let read_failure = SystemKeyringBoundary::map_read_error(keyring::Error::PlatformFailure(
            Box::new(FormatObservedPlatformError {
                format_count: read_format_count.clone(),
            }),
        ));
        assert_eq!(read_failure, CredentialStoreFailure::Internal);
        assert_eq!(read_format_count.load(Ordering::SeqCst), 0);

        let write_format_count = Arc::new(AtomicUsize::new(0));
        let write_failure = SystemKeyringBoundary::map_write_error(
            keyring::Error::PlatformFailure(Box::new(FormatObservedPlatformError {
                format_count: write_format_count.clone(),
            })),
            true,
        );
        assert_eq!(write_failure, CredentialStoreFailure::CommitUnknown);
        assert_eq!(write_format_count.load(Ordering::SeqCst), 0);

        let access_format_count = Arc::new(AtomicUsize::new(0));
        let access_failure = SystemKeyringBoundary::map_read_error(
            keyring::Error::NoStorageAccess(Box::new(FormatObservedPlatformError {
                format_count: access_format_count.clone(),
            })),
        );
        assert_eq!(access_failure, CredentialStoreFailure::Unavailable);
        assert_eq!(access_format_count.load(Ordering::SeqCst), 0);

        let byte_canary = b"bad-data-byte-canary".to_vec();
        let bytes_failure =
            SystemKeyringBoundary::map_read_error(keyring::Error::BadEncoding(byte_canary));
        assert_eq!(bytes_failure, CredentialStoreFailure::CorruptRecord);
        assert!(!format!("{bytes_failure:?}").contains("bad-data-byte-canary"));
    }

    #[test]
    fn discarded_native_error_byte_payloads_are_zeroized_in_place() {
        let bad_encoding = SystemKeyringBoundary::map_read_error(keyring::Error::BadEncoding(
            b"delete-read-secret-canary".to_vec(),
        ));
        let bad_format = SystemKeyringBoundary::map_read_error(keyring::Error::BadDataFormat(
            b"delete-read-record-canary".to_vec(),
            Box::new(FormatObservedPlatformError {
                format_count: Arc::new(AtomicUsize::new(0)),
            }),
        ));

        assert_eq!(bad_encoding, CredentialStoreFailure::CorruptRecord);
        assert_eq!(bad_format, CredentialStoreFailure::CorruptRecord);
    }

    #[test]
    fn delete_readback_failure_is_scrubbed_and_collapsed_to_commit_unknown() {
        let format_count = Arc::new(AtomicUsize::new(0));
        let reads = Arc::new(AtomicUsize::new(0));
        let deletes = Arc::new(AtomicUsize::new(0));
        let adapter = KeyringEntryAdapter::new(Arc::new(DeleteReadFailureBoundary {
            format_count: format_count.clone(),
            reads: reads.clone(),
            deletes: deletes.clone(),
        }));
        let gate = NativeInteractionGate::isolated_for_test();
        let mut lease = gate.acquire().expect("ordinary interaction lease");
        let operation_id =
            audio_graph_ipc_contract::credential_contract::CredentialOperationId::parse(
                "11111111-2222-3333-4444-555555555555",
            )
            .expect("canonical operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);

        assert_eq!(
            adapter.delete_and_verify_absent(
                &mut lease,
                &EntryLocator::staging(&operation_id, &set_id),
            ),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        assert_eq!(format_count.load(Ordering::SeqCst), 0);
        drop(lease);

        let reads_after_uncertainty = reads.load(Ordering::SeqCst);
        let deletes_after_uncertainty = deletes.load(Ordering::SeqCst);
        assert!(matches!(
            gate.acquire(),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(reads.load(Ordering::SeqCst), reads_after_uncertainty);
        assert_eq!(deletes.load(Ordering::SeqCst), deletes_after_uncertainty);
        assert_eq!(format_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn pre_and_post_invocation_panics_latch_with_phase_accurate_failures() {
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        let locator = EntryLocator::active(&set_id);

        let read_gate = NativeInteractionGate::isolated_for_test();
        let read_calls = Arc::new(AtomicUsize::new(0));
        let read_adapter = KeyringEntryAdapter::new(Arc::new(PanicBoundary {
            phase: PanicPhase::Read,
            reads: read_calls.clone(),
            writes: Arc::new(AtomicUsize::new(0)),
        }));
        let mut read_lease = read_gate.acquire().expect("read interaction lease");
        assert!(matches!(
            read_adapter.read(&mut read_lease, &locator),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        drop(read_lease);
        assert!(matches!(
            read_gate.acquire(),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(read_calls.load(Ordering::SeqCst), 1);

        let pre_gate = NativeInteractionGate::isolated_for_test();
        let pre_adapter = KeyringEntryAdapter::new(Arc::new(PanicBoundary {
            phase: PanicPhase::BeforeMutation,
            reads: Arc::new(AtomicUsize::new(0)),
            writes: Arc::new(AtomicUsize::new(0)),
        }));
        let mut pre_lease = pre_gate.acquire().expect("pre-mutation interaction lease");
        assert_eq!(
            pre_adapter.write(&mut pre_lease, &locator, b"opaque-record"),
            Err(CredentialStoreFailure::StalledWorker)
        );
        drop(pre_lease);
        assert!(matches!(
            pre_gate.acquire(),
            Err(CredentialStoreFailure::StalledWorker)
        ));

        let post_gate = NativeInteractionGate::isolated_for_test();
        let post_adapter = KeyringEntryAdapter::new(Arc::new(PanicBoundary {
            phase: PanicPhase::AfterMutation,
            reads: Arc::new(AtomicUsize::new(0)),
            writes: Arc::new(AtomicUsize::new(0)),
        }));
        let mut post_lease = post_gate
            .acquire()
            .expect("post-mutation interaction lease");
        assert_eq!(
            post_adapter.write(&mut post_lease, &locator, b"opaque-record"),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        drop(post_lease);
        assert!(matches!(
            post_gate.acquire(),
            Err(CredentialStoreFailure::StalledWorker)
        ));

        let sticky_gate = NativeInteractionGate::isolated_for_test();
        let sticky_adapter = KeyringEntryAdapter::new(Arc::new(PanicBoundary {
            phase: PanicPhase::ReadAfterMutation,
            reads: Arc::new(AtomicUsize::new(0)),
            writes: Arc::new(AtomicUsize::new(0)),
        }));
        let mut sticky_lease = sticky_gate
            .acquire()
            .expect("sticky mutation interaction lease");
        sticky_adapter
            .write(&mut sticky_lease, &locator, b"opaque-record")
            .expect("scripted mutation succeeds");
        assert!(matches!(
            sticky_adapter.read(&mut sticky_lease, &locator),
            Err(CredentialStoreFailure::CommitUnknown)
        ));
        drop(sticky_lease);
        assert!(matches!(
            sticky_gate.acquire(),
            Err(CredentialStoreFailure::StalledWorker)
        ));
    }

    #[test]
    fn returned_commit_unknown_latches_with_or_without_started_marker() {
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        let locator = EntryLocator::active(&set_id);
        for mark_started in [false, true] {
            let gate = NativeInteractionGate::isolated_for_test();
            let writes = Arc::new(AtomicUsize::new(0));
            let adapter = KeyringEntryAdapter::new(Arc::new(WriteFailureBoundary {
                failure: Mutex::new(Some(CredentialStoreFailure::CommitUnknown)),
                mark_started,
                writes: writes.clone(),
            }));
            let mut lease = gate.acquire().expect("returned uncertainty lease");
            assert_eq!(
                adapter.write(&mut lease, &locator, b"opaque-record"),
                Err(CredentialStoreFailure::CommitUnknown)
            );
            drop(lease);
            assert!(matches!(
                gate.acquire(),
                Err(CredentialStoreFailure::StalledWorker)
            ));
            assert_eq!(writes.load(Ordering::SeqCst), 1);
        }

        let read_gate = NativeInteractionGate::isolated_for_test();
        let read_adapter = KeyringEntryAdapter::new(Arc::new(ReadFailureBoundary(Mutex::new(
            Some(CredentialStoreFailure::CommitUnknown),
        ))));
        let mut read_lease = read_gate
            .acquire()
            .expect("returned read uncertainty lease");
        assert!(matches!(
            read_adapter.read(&mut read_lease, &locator),
            Err(CredentialStoreFailure::CommitUnknown)
        ));
        drop(read_lease);
        assert!(matches!(
            read_gate.acquire(),
            Err(CredentialStoreFailure::StalledWorker)
        ));
    }
}
