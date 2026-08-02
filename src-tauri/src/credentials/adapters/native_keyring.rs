use super::authority_journal::{
    AuthorityJournalBoundary, AuthorityMutationLock, FileAuthorityJournalBoundary,
    decode_authority_journal, decode_authority_marker, encode_authority_journal,
    encode_authority_marker, new_authority_instance_id,
};
#[cfg(test)]
use super::keyring_entry::KeyringBoundary;
use super::keyring_entry::{EntryLocator, KeyringEntryAdapter};
use super::native_interaction::{
    NativeInteractionGate, NativeInteractionLease, process_native_interaction_gate,
};
use crate::credentials::domain::{
    AuthorityJournal, CredentialStoreFailure, EncodedCredentialRecord,
};
use crate::credentials::service::{CredentialEntryStore, CredentialMutationSession};
use audio_graph_ipc_contract::credential_contract::{
    BUILT_IN_CREDENTIAL_SET_IDS, CredentialBackendKind, CredentialOperationId, CredentialSetId,
};
use std::path::PathBuf;
use std::sync::Arc;

pub(crate) struct NativeKeyringCredentialStore {
    entries: KeyringEntryAdapter,
    journal: Arc<dyn AuthorityJournalBoundary>,
    interaction_gate: &'static NativeInteractionGate,
}

pub(crate) enum NativeCredentialStoreOpen {
    Uninitialized(NativeKeyringCredentialStore),
    Ready {
        store: NativeKeyringCredentialStore,
        journal: Box<AuthorityJournal>,
    },
    RecoveryRequired,
}

enum AuthorityPairState {
    Uninitialized,
    Ready {
        authority_instance_id: String,
        journal: Box<AuthorityJournal>,
    },
    RecoveryRequired,
}

struct NativeCredentialMutationSession<'a> {
    store: &'a NativeKeyringCredentialStore,
    operation_id: CredentialOperationId,
    set_id: CredentialSetId,
    authority_instance_id: Option<String>,
    interaction: NativeInteractionLease<'static>,
    expected_active_readback: Option<(CredentialSetId, EncodedCredentialRecord)>,
    expected_staging_readback: Option<(
        CredentialOperationId,
        CredentialSetId,
        EncodedCredentialRecord,
    )>,
    _mutation_lock: Box<dyn AuthorityMutationLock>,
}

impl NativeKeyringCredentialStore {
    pub(crate) fn open(
        root: impl Into<PathBuf>,
    ) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
        Self::open_with_adapters(
            KeyringEntryAdapter::production(),
            Arc::new(FileAuthorityJournalBoundary::new(root)),
        )
    }

    #[cfg(test)]
    fn open_with_boundaries(
        entries: Arc<dyn KeyringBoundary>,
        journal: Arc<dyn AuthorityJournalBoundary>,
    ) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
        Self::open_with_adapters_and_gate(
            KeyringEntryAdapter::new(entries),
            journal,
            NativeInteractionGate::isolated_for_test(),
        )
    }

    #[cfg(test)]
    fn open_with_boundaries_and_gate(
        entries: Arc<dyn KeyringBoundary>,
        journal: Arc<dyn AuthorityJournalBoundary>,
        interaction_gate: &'static NativeInteractionGate,
    ) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
        Self::open_with_adapters_and_gate(
            KeyringEntryAdapter::new(entries),
            journal,
            interaction_gate,
        )
    }

    fn open_with_adapters(
        entries: KeyringEntryAdapter,
        journal: Arc<dyn AuthorityJournalBoundary>,
    ) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
        Self::open_with_adapters_and_gate(entries, journal, process_native_interaction_gate())
    }

    fn open_with_adapters_and_gate(
        entries: KeyringEntryAdapter,
        journal: Arc<dyn AuthorityJournalBoundary>,
        interaction_gate: &'static NativeInteractionGate,
    ) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
        let store = Self {
            entries,
            journal,
            interaction_gate,
        };

        let authority_state = {
            let mut interaction = store.interaction_gate.acquire()?;
            let _mutation_lock = store.journal.acquire_mutation_lock()?;
            store.read_authority_state_unlocked(&mut interaction)?
        };

        match authority_state {
            AuthorityPairState::Uninitialized => {
                Ok(NativeCredentialStoreOpen::Uninitialized(store))
            }
            AuthorityPairState::Ready { journal, .. } => {
                Ok(NativeCredentialStoreOpen::Ready { store, journal })
            }
            AuthorityPairState::RecoveryRequired => Ok(NativeCredentialStoreOpen::RecoveryRequired),
        }
    }

    pub(crate) fn initialize(
        self,
        journal: AuthorityJournal,
    ) -> Result<NativeCredentialStoreOpen, CredentialStoreFailure> {
        journal.validate_persisted_for_backend(CredentialBackendKind::Native)?;

        {
            let mut interaction = self.interaction_gate.acquire()?;
            let _mutation_lock = self.journal.acquire_mutation_lock()?;
            if !matches!(
                self.read_authority_state_unlocked(&mut interaction)?,
                AuthorityPairState::Uninitialized
            ) {
                return Err(CredentialStoreFailure::RevisionConflict);
            }
            self.require_built_in_entries_absent_unlocked(&mut interaction)?;

            let authority_instance_id = new_authority_instance_id();
            let marker = encode_authority_marker(&authority_instance_id)?;
            let journal_bytes = encode_authority_journal(&authority_instance_id, &journal)?;
            self.entries
                .write_authority(&mut interaction, &EntryLocator::authority(), &marker)?;
            interaction.ensure_healthy()?;
            self.journal
                .replace(&journal_bytes)
                .map_err(|_| interaction.latch_commit_unknown())?;

            match self.read_authority_state_unlocked(&mut interaction) {
                Ok(AuthorityPairState::Ready {
                    journal: readback, ..
                }) if *readback == journal => {}
                _ => return Err(interaction.latch_commit_unknown()),
            }
        }

        Ok(NativeCredentialStoreOpen::Ready {
            store: self,
            journal: Box::new(journal),
        })
    }

    fn read_authority_state_unlocked(
        &self,
        interaction: &mut NativeInteractionLease<'_>,
    ) -> Result<AuthorityPairState, CredentialStoreFailure> {
        interaction.ensure_healthy()?;
        let journal_bytes = self.journal.read()?;
        let marker_bytes = match self.entries.read(interaction, &EntryLocator::authority()) {
            Ok(marker) => marker,
            Err(CredentialStoreFailure::CorruptRecord)
            | Err(CredentialStoreFailure::UnsupportedSchema) => {
                return Ok(AuthorityPairState::RecoveryRequired);
            }
            Err(failure) => return Err(failure),
        };
        match (journal_bytes, marker_bytes) {
            (None, None) => Ok(AuthorityPairState::Uninitialized),
            (Some(journal_bytes), Some(marker_bytes)) => {
                let Some(journal) = decode_authority_journal(&journal_bytes) else {
                    return Ok(AuthorityPairState::RecoveryRequired);
                };
                let Some(marker_id) = decode_authority_marker(marker_bytes.as_slice()) else {
                    return Ok(AuthorityPairState::RecoveryRequired);
                };
                if journal.authority_instance_id == marker_id
                    && journal
                        .journal
                        .validate_persisted_for_backend(CredentialBackendKind::Native)
                        .is_ok()
                {
                    Ok(AuthorityPairState::Ready {
                        authority_instance_id: marker_id,
                        journal: Box::new(journal.journal),
                    })
                } else {
                    Ok(AuthorityPairState::RecoveryRequired)
                }
            }
            _ => Ok(AuthorityPairState::RecoveryRequired),
        }
    }

    fn read_record_unlocked(
        &self,
        interaction: &mut NativeInteractionLease<'_>,
        locator: &EntryLocator,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        self.entries
            .read(interaction, locator)
            .map(|record| record.map(EncodedCredentialRecord::from_zeroizing_boundary_bytes))
    }

    fn require_built_in_entries_absent_unlocked(
        &self,
        interaction: &mut NativeInteractionLease<'_>,
    ) -> Result<(), CredentialStoreFailure> {
        for set_id in BUILT_IN_CREDENTIAL_SET_IDS {
            let locator = EntryLocator::active(&CredentialSetId::BuiltIn(*set_id));
            match self.entries.read(interaction, &locator) {
                Ok(None) => {}
                Ok(Some(_)) => return Err(CredentialStoreFailure::CorruptRecord),
                Err(failure) => return Err(failure),
            }
        }
        Ok(())
    }
}

impl CredentialEntryStore for NativeKeyringCredentialStore {
    fn read_active(
        &self,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        let mut interaction = self.interaction_gate.acquire()?;
        self.read_record_unlocked(&mut interaction, &EntryLocator::active(set_id))
    }

    fn begin_mutation(
        &self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<Box<dyn CredentialMutationSession + '_>, CredentialStoreFailure> {
        let interaction = self.interaction_gate.acquire()?;
        let mutation_lock = self.journal.acquire_mutation_lock()?;
        Ok(Box::new(NativeCredentialMutationSession {
            store: self,
            operation_id: operation_id.clone(),
            set_id: set_id.clone(),
            authority_instance_id: None,
            interaction,
            expected_active_readback: None,
            expected_staging_readback: None,
            _mutation_lock: mutation_lock,
        }))
    }
}

impl NativeCredentialMutationSession<'_> {
    fn require_set_scope(&self, set_id: &CredentialSetId) -> Result<(), CredentialStoreFailure> {
        if self.set_id == *set_id {
            Ok(())
        } else {
            Err(CredentialStoreFailure::Internal)
        }
    }

    fn require_staging_scope(
        &self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<(), CredentialStoreFailure> {
        if self.operation_id == *operation_id && self.set_id == *set_id {
            Ok(())
        } else {
            Err(CredentialStoreFailure::Internal)
        }
    }

    fn require_no_pending_readback(&self) -> Result<(), CredentialStoreFailure> {
        if self.expected_active_readback.is_none() && self.expected_staging_readback.is_none() {
            Ok(())
        } else {
            Err(CredentialStoreFailure::OperationInProgress)
        }
    }

    fn persist_journal(
        &mut self,
        journal: &AuthorityJournal,
    ) -> Result<(), CredentialStoreFailure> {
        self.interaction.ensure_healthy()?;
        self.require_no_pending_readback()?;
        journal.validate_persisted_for_backend(CredentialBackendKind::Native)?;

        let authority_instance_id = self
            .authority_instance_id
            .as_deref()
            .ok_or(CredentialStoreFailure::Internal)?;
        let bytes = encode_authority_journal(authority_instance_id, journal)?;
        match self.store.journal.replace(&bytes) {
            Ok(()) => {}
            Err(CredentialStoreFailure::CommitUnknown) => {
                return Err(self.interaction.latch_commit_unknown());
            }
            Err(failure) => return Err(failure),
        }
        match self
            .store
            .read_authority_state_unlocked(&mut self.interaction)
        {
            Ok(AuthorityPairState::Ready {
                authority_instance_id: readback_id,
                journal: readback,
            }) if readback_id == authority_instance_id && *readback == *journal => Ok(()),
            Ok(AuthorityPairState::Uninitialized)
            | Ok(AuthorityPairState::Ready { .. })
            | Ok(AuthorityPairState::RecoveryRequired)
            | Err(_) => Err(self.interaction.latch_commit_unknown()),
        }
    }
}

impl CredentialMutationSession for NativeCredentialMutationSession<'_> {
    fn load_journal(&mut self) -> Result<AuthorityJournal, CredentialStoreFailure> {
        self.interaction.ensure_healthy()?;
        self.require_no_pending_readback()?;
        match self
            .store
            .read_authority_state_unlocked(&mut self.interaction)?
        {
            AuthorityPairState::Ready {
                authority_instance_id,
                journal,
            } => {
                self.authority_instance_id = Some(authority_instance_id);
                Ok(*journal)
            }
            AuthorityPairState::Uninitialized | AuthorityPairState::RecoveryRequired => {
                Err(CredentialStoreFailure::CorruptRecord)
            }
        }
    }

    fn read_active(
        &mut self,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        self.require_set_scope(set_id)?;
        self.require_no_pending_readback()?;
        self.store
            .read_record_unlocked(&mut self.interaction, &EntryLocator::active(set_id))
    }

    fn persist_intent(&mut self, journal: &AuthorityJournal) -> Result<(), CredentialStoreFailure> {
        self.persist_journal(journal)
    }

    fn replace_active(
        &mut self,
        set_id: &CredentialSetId,
        record: EncodedCredentialRecord,
    ) -> Result<(), CredentialStoreFailure> {
        self.require_set_scope(set_id)?;
        self.require_no_pending_readback()?;
        self.store.entries.write(
            &mut self.interaction,
            &EntryLocator::active(set_id),
            record.as_bytes(),
        )?;
        self.expected_active_readback = Some((set_id.clone(), record));
        Ok(())
    }

    fn readback_active(
        &mut self,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        self.require_set_scope(set_id)?;
        if self.expected_staging_readback.is_some() {
            return Err(CredentialStoreFailure::OperationInProgress);
        }
        let readback = self
            .store
            .read_record_unlocked(&mut self.interaction, &EntryLocator::active(set_id));
        let Some((expected_set_id, expected)) = self.expected_active_readback.take() else {
            return readback;
        };
        if expected_set_id != *set_id {
            self.expected_active_readback = Some((expected_set_id, expected));
            return readback;
        }
        match readback {
            Ok(Some(record)) if record.as_bytes() == expected.as_bytes() => Ok(Some(record)),
            Ok(None) | Ok(Some(_)) | Err(_) => Err(self.interaction.latch_commit_unknown()),
        }
    }

    fn write_staging(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
        record: EncodedCredentialRecord,
    ) -> Result<(), CredentialStoreFailure> {
        self.require_staging_scope(operation_id, set_id)?;
        self.require_no_pending_readback()?;
        self.store.entries.write(
            &mut self.interaction,
            &EntryLocator::staging(operation_id, set_id),
            record.as_bytes(),
        )?;
        self.expected_staging_readback = Some((operation_id.clone(), set_id.clone(), record));
        Ok(())
    }

    fn read_staging(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        self.require_staging_scope(operation_id, set_id)?;
        if self.expected_active_readback.is_some() {
            return Err(CredentialStoreFailure::OperationInProgress);
        }
        let readback = self.store.read_record_unlocked(
            &mut self.interaction,
            &EntryLocator::staging(operation_id, set_id),
        );
        let Some((expected_operation_id, expected_set_id, expected)) =
            self.expected_staging_readback.take()
        else {
            return readback;
        };
        if expected_operation_id != *operation_id || expected_set_id != *set_id {
            self.expected_staging_readback =
                Some((expected_operation_id, expected_set_id, expected));
            return readback;
        }
        match readback {
            Ok(Some(record)) if record.as_bytes() == expected.as_bytes() => Ok(Some(record)),
            Ok(None) | Ok(Some(_)) | Err(_) => Err(self.interaction.latch_commit_unknown()),
        }
    }

    fn delete_staging(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<(), CredentialStoreFailure> {
        self.require_staging_scope(operation_id, set_id)?;
        self.require_no_pending_readback()?;
        self.store.entries.delete_and_verify_absent(
            &mut self.interaction,
            &EntryLocator::staging(operation_id, set_id),
        )
    }

    fn commit_journal(&mut self, journal: &AuthorityJournal) -> Result<(), CredentialStoreFailure> {
        self.persist_journal(journal)
    }
}

impl Drop for NativeCredentialMutationSession<'_> {
    fn drop(&mut self) {
        if self.expected_active_readback.is_some() || self.expected_staging_readback.is_some() {
            let _ = self.interaction.latch_commit_unknown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NativeCredentialStoreOpen, NativeKeyringCredentialStore};
    use crate::credentials::adapters::authority_journal::{
        AuthorityJournalBoundary, AuthorityMutationLock, FileAuthorityJournalBoundary,
        encode_authority_journal, encode_authority_marker,
    };
    use crate::credentials::adapters::keyring_entry::{
        EntryLocator, KeyringBoundary, KeyringEntryAdapter,
    };
    use crate::credentials::adapters::native_interaction::{
        ForbidPrompt, MutationInvocation, NativeInteractionGate,
    };
    use crate::credentials::domain::{
        AuthorityJournal, CredentialMutationKind, CredentialRecordEnvelope, CredentialStoreFailure,
        EncodedCredentialRecord, PendingCredentialIntent, PendingSettingsActivation,
        StoredSecretBundle,
    };
    use crate::credentials::service::{
        CredentialEntryStore, CredentialService, DeleteCredentialSet, ReplaceCredentialSet,
    };
    use crate::credentials::test_support::DeterministicTokenSource;
    use audio_graph_ipc_contract::credential_contract::{
        AuthMethodId, BUILT_IN_CREDENTIAL_SET_IDS, BuiltInCredentialSetId,
        CredentialActivationStage, CredentialBackendKind, CredentialErrorCode,
        CredentialIdempotencyToken, CredentialOperationId, CredentialRevision,
        CredentialSafeRecoveryAction, CredentialSetId, CredentialSetRecoveryState,
        PORTABLE_ENCODED_RECORD_MAX_BYTES,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};

    #[derive(Clone, Copy)]
    enum AuthorityWriteCut {
        BeforeInvocation,
        AfterPersist,
    }

    #[derive(Clone, Copy)]
    enum JournalReplaceCut {
        BeforePublish,
        AfterPublish,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum PolicyTraceTarget {
        Authority,
        Staging,
        Active(BuiltInCredentialSetId),
        OtherActive,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct PolicyTraceStep {
        operation: &'static str,
        prompt_id: usize,
        target: PolicyTraceTarget,
    }

    #[derive(Default)]
    struct MemoryKeyringBoundary {
        entries: Mutex<Vec<(String, Vec<u8>)>>,
        reads: AtomicUsize,
        writes: AtomicUsize,
        deletes: AtomicUsize,
        authority_reads: AtomicUsize,
        fail_authority_read_at: AtomicUsize,
        authority_write_cut: Mutex<Option<AuthorityWriteCut>>,
        fail_active_reads: AtomicBool,
        corrupt_active_readback: AtomicBool,
        corrupt_staging_readback: AtomicBool,
        policy_trace: Mutex<Vec<PolicyTraceStep>>,
    }

    impl MemoryKeyringBoundary {
        fn trace_policy(&self, prompt: &ForbidPrompt<'_>, operation: &str, locator: &EntryLocator) {
            let kind = if locator.account() == "v2/_authority" {
                "authority"
            } else if locator.account().starts_with("v2-staging/") {
                "staging"
            } else {
                "active"
            };
            let step = match (operation, kind) {
                ("get", "authority") => "forbid:get:authority",
                ("get", "staging") => "forbid:get:staging",
                ("get", "active") => "forbid:get:active",
                ("set", "authority") => "forbid:set:authority",
                ("set", "staging") => "forbid:set:staging",
                ("set", "active") => "forbid:set:active",
                ("delete", "staging") => "forbid:delete:staging",
                ("delete", "authority") => "forbid:delete:authority",
                ("delete", "active") => "forbid:delete:active",
                _ => unreachable!("closed policy trace operation"),
            };
            let target = if locator.account() == "v2/_authority" {
                PolicyTraceTarget::Authority
            } else if locator.account().starts_with("v2-staging/") {
                PolicyTraceTarget::Staging
            } else {
                locator
                    .account()
                    .strip_prefix("v2/")
                    .and_then(|account| {
                        BUILT_IN_CREDENTIAL_SET_IDS
                            .iter()
                            .copied()
                            .find(|candidate| candidate.as_str() == account)
                    })
                    .map_or(PolicyTraceTarget::OtherActive, PolicyTraceTarget::Active)
            };
            self.policy_trace
                .lock()
                .expect("memory keyring policy trace lock")
                .push(PolicyTraceStep {
                    operation: step,
                    prompt_id: prompt as *const ForbidPrompt<'_> as usize,
                    target,
                });
        }
    }

    impl KeyringBoundary for MemoryKeyringBoundary {
        fn get_secret(
            &self,
            prompt: &ForbidPrompt<'_>,
            locator: &EntryLocator,
        ) -> Result<Vec<u8>, CredentialStoreFailure> {
            self.trace_policy(prompt, "get", locator);
            self.reads.fetch_add(1, Ordering::SeqCst);
            if locator.account() == "v2/_authority" {
                let authority_read = self.authority_reads.fetch_add(1, Ordering::SeqCst) + 1;
                if self.fail_authority_read_at.load(Ordering::SeqCst) == authority_read {
                    return Err(CredentialStoreFailure::Unavailable);
                }
            } else if locator.account().starts_with("v2/")
                && self.fail_active_reads.load(Ordering::SeqCst)
            {
                return Err(CredentialStoreFailure::Unavailable);
            }
            let mut value = self
                .entries
                .lock()
                .expect("memory keyring lock")
                .iter()
                .find(|(account, _)| account == locator.account())
                .map(|(_, value)| value.clone())
                .ok_or(CredentialStoreFailure::Missing)?;
            if (locator.account().starts_with("v2/")
                && locator.account() != "v2/_authority"
                && self.corrupt_active_readback.load(Ordering::SeqCst))
                || (locator.account().starts_with("v2-staging/")
                    && self.corrupt_staging_readback.load(Ordering::SeqCst))
            {
                value.push(0xff);
            }
            Ok(value)
        }

        fn set_secret(
            &self,
            prompt: &ForbidPrompt<'_>,
            invocation: &mut MutationInvocation<'_>,
            locator: &EntryLocator,
            secret: &[u8],
        ) -> Result<(), CredentialStoreFailure> {
            self.trace_policy(prompt, "set", locator);
            self.writes.fetch_add(1, Ordering::SeqCst);
            let authority_cut = if locator.account() == "v2/_authority" {
                self.authority_write_cut
                    .lock()
                    .expect("authority write cut lock")
                    .take()
            } else {
                None
            };
            if matches!(authority_cut, Some(AuthorityWriteCut::BeforeInvocation)) {
                return Err(CredentialStoreFailure::Unavailable);
            }
            invocation.mark_started();
            let mut entries = self.entries.lock().expect("memory keyring lock");
            entries.retain(|(account, _)| account != locator.account());
            entries.push((locator.account().to_owned(), secret.to_vec()));
            if matches!(authority_cut, Some(AuthorityWriteCut::AfterPersist)) {
                return Err(CredentialStoreFailure::Unavailable);
            }
            Ok(())
        }

        fn delete_credential(
            &self,
            prompt: &ForbidPrompt<'_>,
            invocation: &mut MutationInvocation<'_>,
            locator: &EntryLocator,
        ) -> Result<(), CredentialStoreFailure> {
            self.trace_policy(prompt, "delete", locator);
            self.deletes.fetch_add(1, Ordering::SeqCst);
            invocation.mark_started();
            let mut entries = self.entries.lock().expect("memory keyring lock");
            let before = entries.len();
            entries.retain(|(account, _)| account != locator.account());
            if entries.len() == before {
                Err(CredentialStoreFailure::Missing)
            } else {
                Ok(())
            }
        }
    }

    struct CorruptMarkerBoundary;

    impl KeyringBoundary for CorruptMarkerBoundary {
        fn get_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            locator: &EntryLocator,
        ) -> Result<Vec<u8>, CredentialStoreFailure> {
            if locator.account() == "v2/_authority" {
                Err(CredentialStoreFailure::CorruptRecord)
            } else {
                Err(CredentialStoreFailure::Missing)
            }
        }

        fn set_secret(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
            _secret: &[u8],
        ) -> Result<(), CredentialStoreFailure> {
            unreachable!("read-only corrupt marker boundary")
        }

        fn delete_credential(
            &self,
            _prompt: &ForbidPrompt<'_>,
            _invocation: &mut MutationInvocation<'_>,
            _locator: &EntryLocator,
        ) -> Result<(), CredentialStoreFailure> {
            unreachable!("read-only corrupt marker boundary")
        }
    }

    #[derive(Default)]
    struct MemoryJournalBoundary {
        bytes: Mutex<Option<Vec<u8>>>,
        reads: AtomicUsize,
        writes: AtomicUsize,
        corrupt_replacements: AtomicBool,
        fail_read_at: AtomicUsize,
        replace_cut: Mutex<Option<JournalReplaceCut>>,
    }

    struct MemoryMutationLock;
    impl AuthorityMutationLock for MemoryMutationLock {}

    impl AuthorityJournalBoundary for MemoryJournalBoundary {
        fn read(&self) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
            let read = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
            if self.fail_read_at.load(Ordering::SeqCst) == read {
                return Err(CredentialStoreFailure::Unavailable);
            }
            Ok(self.bytes.lock().expect("memory journal lock").clone())
        }

        fn replace(&self, bytes: &[u8]) -> Result<(), CredentialStoreFailure> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            let cut = self
                .replace_cut
                .lock()
                .expect("journal replace cut lock")
                .take();
            if matches!(cut, Some(JournalReplaceCut::BeforePublish)) {
                return Err(CredentialStoreFailure::Unavailable);
            }
            *self.bytes.lock().expect("memory journal lock") =
                Some(if self.corrupt_replacements.load(Ordering::SeqCst) {
                    b"{corrupt-authority-journal".to_vec()
                } else {
                    bytes.to_vec()
                });
            if matches!(cut, Some(JournalReplaceCut::AfterPublish)) {
                return Err(CredentialStoreFailure::CommitUnknown);
            }
            Ok(())
        }

        fn acquire_mutation_lock(
            &self,
        ) -> Result<Box<dyn AuthorityMutationLock>, CredentialStoreFailure> {
            Ok(Box::new(MemoryMutationLock))
        }
    }

    fn ready_store_with_gate(
        keyring: Arc<MemoryKeyringBoundary>,
        journal: Arc<MemoryJournalBoundary>,
        gate: &'static NativeInteractionGate,
    ) -> NativeKeyringCredentialStore {
        let opened =
            NativeKeyringCredentialStore::open_with_boundaries_and_gate(keyring, journal, gate)
                .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        match store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority")
        {
            NativeCredentialStoreOpen::Ready { store, .. } => store,
            _ => panic!("expected ready authority"),
        }
    }

    #[test]
    fn absent_journal_and_marker_open_uninitialized_without_authority_state_writes() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());

        let opened =
            NativeKeyringCredentialStore::open_with_boundaries(keyring.clone(), journal.clone())
                .expect("open absent authority");

        assert!(matches!(
            opened,
            NativeCredentialStoreOpen::Uninitialized(_)
        ));
        assert_eq!(keyring.reads.load(Ordering::SeqCst), 1);
        assert_eq!(keyring.writes.load(Ordering::SeqCst), 0);
        assert_eq!(journal.reads.load(Ordering::SeqCst), 1);
        assert_eq!(journal.writes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn poisoned_process_entry_gate_fails_closed_before_any_keyring_call() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let gate = NativeInteractionGate::isolated_for_test();
        let poisoner = std::thread::spawn(move || {
            gate.poison_for_test();
        });
        assert!(poisoner.join().is_err());

        let failure = match NativeKeyringCredentialStore::open_with_boundaries_and_gate(
            keyring.clone(),
            journal.clone(),
            gate,
        ) {
            Err(failure) => failure,
            Ok(_) => panic!("poisoned entry gate must fail closed"),
        };
        assert_eq!(failure, CredentialStoreFailure::StalledWorker);
        let public = failure.into_public(None);
        assert_eq!(public.code, CredentialErrorCode::StalledWorker);
        assert_eq!(
            public.recovery_action,
            CredentialSafeRecoveryAction::RestartApplication
        );
        assert!(!public.retryable);
        assert_eq!(keyring.reads.load(Ordering::SeqCst), 0);
        assert_eq!(keyring.writes.load(Ordering::SeqCst), 0);
        assert_eq!(journal.reads.load(Ordering::SeqCst), 0);
        assert_eq!(journal.writes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn invalid_initial_journal_is_rejected_before_any_authority_write() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };

        assert!(matches!(
            store.initialize(AuthorityJournal::new(CredentialBackendKind::InMemory)),
            Err(CredentialStoreFailure::CorruptRecord)
        ));
        assert_eq!(keyring.writes.load(Ordering::SeqCst), 0);
        assert_eq!(journal_boundary.writes.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn initialization_rejects_orphan_built_in_entries_before_authority_publication() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring
            .entries
            .lock()
            .expect("memory keyring lock")
            .push(("v2/openai".to_owned(), b"orphan-secret-canary".to_vec()));
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };

        assert!(matches!(
            store.initialize(AuthorityJournal::new(CredentialBackendKind::Native)),
            Err(CredentialStoreFailure::CorruptRecord)
        ));
        assert_eq!(keyring.writes.load(Ordering::SeqCst), 0);
        assert_eq!(journal_boundary.writes.load(Ordering::SeqCst), 0);
        assert!(
            keyring
                .entries
                .lock()
                .expect("memory keyring lock")
                .iter()
                .all(|(account, _)| account != "v2/_authority")
        );
    }

    #[test]
    fn initialization_preserves_typed_orphan_preflight_read_failures_without_writing() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.fail_active_reads.store(true, Ordering::SeqCst);
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };

        assert!(matches!(
            store.initialize(AuthorityJournal::new(CredentialBackendKind::Native)),
            Err(CredentialStoreFailure::Unavailable)
        ));
        assert_eq!(keyring.writes.load(Ordering::SeqCst), 0);
        assert_eq!(journal_boundary.writes.load(Ordering::SeqCst), 0);
        assert!(
            keyring
                .entries
                .lock()
                .expect("memory keyring lock")
                .iter()
                .all(|(account, _)| account != "v2/_authority")
        );
    }

    #[test]
    fn initialization_cut_points_preserve_definite_prewrite_errors_and_normalize_uncertainty() {
        #[derive(Clone, Copy)]
        enum ReopenState {
            Uninitialized,
            Ready,
            RecoveryRequired,
        }

        fn run_case(
            keyring: Arc<MemoryKeyringBoundary>,
            journal_boundary: Arc<MemoryJournalBoundary>,
            expected_error: CredentialStoreFailure,
            expected_reopen: ReopenState,
        ) {
            let opened = NativeKeyringCredentialStore::open_with_boundaries(
                keyring.clone(),
                journal_boundary.clone(),
            )
            .expect("open absent authority");
            let store = match opened {
                NativeCredentialStoreOpen::Uninitialized(store) => store,
                _ => panic!("expected uninitialized authority"),
            };
            let error = match store.initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            {
                Ok(_) => panic!("scripted initialization cut must fail"),
                Err(error) => error,
            };
            assert_eq!(error, expected_error);

            let reopened =
                NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                    .expect("reopen after scripted cut");
            assert!(match expected_reopen {
                ReopenState::Uninitialized => {
                    matches!(reopened, NativeCredentialStoreOpen::Uninitialized(_))
                }
                ReopenState::Ready => {
                    matches!(reopened, NativeCredentialStoreOpen::Ready { .. })
                }
                ReopenState::RecoveryRequired => {
                    matches!(reopened, NativeCredentialStoreOpen::RecoveryRequired)
                }
            });
        }

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        *keyring
            .authority_write_cut
            .lock()
            .expect("authority write cut lock") = Some(AuthorityWriteCut::BeforeInvocation);
        run_case(
            keyring,
            Arc::new(MemoryJournalBoundary::default()),
            CredentialStoreFailure::Unavailable,
            ReopenState::Uninitialized,
        );

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        *keyring
            .authority_write_cut
            .lock()
            .expect("authority write cut lock") = Some(AuthorityWriteCut::AfterPersist);
        run_case(
            keyring,
            Arc::new(MemoryJournalBoundary::default()),
            CredentialStoreFailure::CommitUnknown,
            ReopenState::RecoveryRequired,
        );

        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        *journal_boundary
            .replace_cut
            .lock()
            .expect("journal replace cut lock") = Some(JournalReplaceCut::BeforePublish);
        run_case(
            Arc::new(MemoryKeyringBoundary::default()),
            journal_boundary,
            CredentialStoreFailure::CommitUnknown,
            ReopenState::RecoveryRequired,
        );

        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        *journal_boundary
            .replace_cut
            .lock()
            .expect("journal replace cut lock") = Some(JournalReplaceCut::AfterPublish);
        run_case(
            Arc::new(MemoryKeyringBoundary::default()),
            journal_boundary,
            CredentialStoreFailure::CommitUnknown,
            ReopenState::Ready,
        );

        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        journal_boundary.fail_read_at.store(3, Ordering::SeqCst);
        run_case(
            Arc::new(MemoryKeyringBoundary::default()),
            journal_boundary,
            CredentialStoreFailure::CommitUnknown,
            ReopenState::Ready,
        );

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.fail_authority_read_at.store(3, Ordering::SeqCst);
        run_case(
            keyring,
            Arc::new(MemoryJournalBoundary::default()),
            CredentialStoreFailure::CommitUnknown,
            ReopenState::Ready,
        );
    }

    #[test]
    fn explicit_initialization_pairs_marker_and_secret_free_journal_then_reopens_ready() {
        const SECRET_CANARY: &[u8] = b"must-not-enter-authority-metadata";
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.entries.lock().expect("memory keyring lock").push((
            "v2-staging/11111111-2222-4333-8444-555555555555/openai".to_owned(),
            SECRET_CANARY.to_vec(),
        ));
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let journal = AuthorityJournal::new(CredentialBackendKind::Native);

        let initialized = store
            .initialize(journal.clone())
            .expect("initialize native authority");
        match initialized {
            NativeCredentialStoreOpen::Ready {
                journal: initialized,
                ..
            } => assert_eq!(*initialized, journal),
            _ => panic!("expected ready authority"),
        }

        let marker_bytes = keyring
            .entries
            .lock()
            .expect("memory keyring lock")
            .iter()
            .find(|(account, _)| account == "v2/_authority")
            .expect("authority marker")
            .1
            .clone();
        let journal_bytes = journal_boundary
            .bytes
            .lock()
            .expect("memory journal lock")
            .clone()
            .expect("authority journal");
        assert!(
            !marker_bytes
                .windows(SECRET_CANARY.len())
                .any(|bytes| bytes == SECRET_CANARY)
        );
        assert!(
            !journal_bytes
                .windows(SECRET_CANARY.len())
                .any(|bytes| bytes == SECRET_CANARY)
        );

        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                .expect("reopen paired authority"),
            NativeCredentialStoreOpen::Ready { .. }
        ));
    }

    #[test]
    fn matching_authority_ids_do_not_override_unsupported_inner_journal() {
        fn pending_activation_journal(auth_method_id: AuthMethodId) -> AuthorityJournal {
            let mut journal = AuthorityJournal::new(CredentialBackendKind::Native);
            let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
            let operation_id = CredentialOperationId::parse("abababab-cdcd-4efe-8010-121212121212")
                .expect("operation id");
            let idempotency_token =
                CredentialIdempotencyToken::parse("bcbcbcbc-dede-4afa-8121-232323232323")
                    .expect("idempotency token");
            let proposed_revision =
                CredentialRevision::parse("cdcdcdcd-efef-4b0b-8232-343434343434")
                    .expect("revision");
            journal.pending_intents.push(PendingCredentialIntent {
                operation_id: operation_id.clone(),
                idempotency_token: idempotency_token.clone(),
                set_id: set_id.clone(),
                mutation_kind: CredentialMutationKind::Activate,
                expected_revision: None,
                proposed_revision: proposed_revision.clone(),
                recovery_state: CredentialSetRecoveryState::PendingIntent,
            });
            journal.pending_activation = Some(PendingSettingsActivation {
                operation_id,
                idempotency_token,
                set_id: set_id.clone(),
                auth_method_id,
                expected_revision: None,
                proposed_revision,
                expected_settings_revision: 52,
                proposed_settings_revision: 53,
                expected_global_epoch: 0,
                proposed_global_epoch: 1,
                stage: CredentialActivationStage::Staged,
            });
            let set = journal.set_state_mut(&set_id).expect("built-in set state");
            set.pending_activation = true;
            set.recovery_state = CredentialSetRecoveryState::PendingIntent;
            journal
        }

        fn duplicate_pending_token_journal() -> AuthorityJournal {
            let mut journal = AuthorityJournal::new(CredentialBackendKind::Native);
            let shared_token =
                CredentialIdempotencyToken::parse("dededede-fafa-4c1c-8343-454545454545")
                    .expect("idempotency token");
            for (set_id, operation_id, proposed_revision) in [
                (
                    BuiltInCredentialSetId::Openai,
                    "efefefef-0b0b-4d2d-8454-565656565656",
                    "f0f0f0f0-1c1c-4e3e-8565-676767676767",
                ),
                (
                    BuiltInCredentialSetId::Deepgram,
                    "01010101-2d2d-4f4f-8676-787878787878",
                    "12121212-3e3e-4050-8787-898989898989",
                ),
            ] {
                let set_id = CredentialSetId::from(set_id);
                journal.pending_intents.push(PendingCredentialIntent {
                    operation_id: CredentialOperationId::parse(operation_id).expect("operation id"),
                    idempotency_token: shared_token.clone(),
                    set_id: set_id.clone(),
                    mutation_kind: CredentialMutationKind::Replace,
                    expected_revision: None,
                    proposed_revision: CredentialRevision::parse(proposed_revision)
                        .expect("revision"),
                    recovery_state: CredentialSetRecoveryState::PendingIntent,
                });
                journal
                    .set_state_mut(&set_id)
                    .expect("built-in set state")
                    .recovery_state = CredentialSetRecoveryState::PendingIntent;
            }
            journal
        }

        fn assert_matching_invalid_journal_requires_recovery(journal: &AuthorityJournal) {
            let authority_instance_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
            let keyring = Arc::new(MemoryKeyringBoundary::default());
            keyring.entries.lock().expect("memory keyring lock").push((
                "v2/_authority".to_owned(),
                encode_authority_marker(authority_instance_id).expect("marker envelope"),
            ));
            let journal_boundary = Arc::new(MemoryJournalBoundary {
                bytes: Mutex::new(Some(
                    encode_authority_journal(authority_instance_id, journal)
                        .expect("journal envelope"),
                )),
                ..MemoryJournalBoundary::default()
            });

            assert!(matches!(
                NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                    .expect("open exact but invalid authority pair"),
                NativeCredentialStoreOpen::RecoveryRequired
            ));
        }

        let authority_instance_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.entries.lock().expect("memory keyring lock").push((
            "v2/_authority".to_owned(),
            encode_authority_marker(authority_instance_id).expect("marker envelope"),
        ));
        let mut unsupported = AuthorityJournal::new(CredentialBackendKind::Native);
        unsupported.schema_version = u32::MAX;
        let journal_boundary = Arc::new(MemoryJournalBoundary {
            bytes: Mutex::new(Some(
                encode_authority_journal(authority_instance_id, &unsupported)
                    .expect("journal envelope"),
            )),
            ..MemoryJournalBoundary::default()
        });

        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                .expect("open invalid inner journal"),
            NativeCredentialStoreOpen::RecoveryRequired
        ));

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.entries.lock().expect("memory keyring lock").push((
            "v2/_authority".to_owned(),
            encode_authority_marker(authority_instance_id).expect("marker envelope"),
        ));
        let mut duplicated_set = AuthorityJournal::new(CredentialBackendKind::Native);
        duplicated_set.sets.push(duplicated_set.sets[0].clone());
        let journal_boundary = Arc::new(MemoryJournalBoundary {
            bytes: Mutex::new(Some(
                encode_authority_journal(authority_instance_id, &duplicated_set)
                    .expect("journal envelope"),
            )),
            ..MemoryJournalBoundary::default()
        });

        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                .expect("open duplicate-set journal"),
            NativeCredentialStoreOpen::RecoveryRequired
        ));

        let wrong_auth_method = pending_activation_journal(AuthMethodId::AwsStatic);
        assert_matching_invalid_journal_requires_recovery(&wrong_auth_method);

        let mut drifted_epoch = pending_activation_journal(AuthMethodId::ApiKey);
        drifted_epoch.global_epoch = 1;
        assert_matching_invalid_journal_requires_recovery(&drifted_epoch);

        let duplicate_pending_token = duplicate_pending_token_journal();
        assert_matching_invalid_journal_requires_recovery(&duplicate_pending_token);
    }

    #[test]
    fn matching_authority_pending_activation_excludes_unrelated_pending_mutation() {
        let authority_instance_id = "89898989-9a9a-4b1b-8c2c-d3d3d3d3d3d3";
        let activation_set = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let activation_operation =
            CredentialOperationId::parse("8a8a8a8a-9b9b-4c2c-8d3d-e4e4e4e4e4e4")
                .expect("operation id");
        let activation_token =
            CredentialIdempotencyToken::parse("8b8b8b8b-9c9c-4d3d-8e4e-f5f5f5f5f5f5")
                .expect("idempotency token");
        let activation_revision =
            CredentialRevision::parse("8c8c8c8c-9d9d-4e4e-8f5f-a6a6a6a6a6a6").expect("revision");
        let mut journal = AuthorityJournal::new(CredentialBackendKind::Native);
        journal.pending_intents.push(PendingCredentialIntent {
            operation_id: activation_operation.clone(),
            idempotency_token: activation_token.clone(),
            set_id: activation_set.clone(),
            mutation_kind: CredentialMutationKind::Activate,
            expected_revision: None,
            proposed_revision: activation_revision.clone(),
            recovery_state: CredentialSetRecoveryState::PendingIntent,
        });
        journal.pending_activation = Some(PendingSettingsActivation {
            operation_id: activation_operation,
            idempotency_token: activation_token,
            set_id: activation_set.clone(),
            auth_method_id: AuthMethodId::ApiKey,
            expected_revision: None,
            proposed_revision: activation_revision,
            expected_settings_revision: 61,
            proposed_settings_revision: 62,
            expected_global_epoch: 0,
            proposed_global_epoch: 1,
            stage: CredentialActivationStage::Staged,
        });
        let activation_state = journal
            .set_state_mut(&activation_set)
            .expect("built-in activation set state");
        activation_state.pending_activation = true;
        activation_state.recovery_state = CredentialSetRecoveryState::PendingIntent;

        let unrelated_set = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        journal.pending_intents.push(PendingCredentialIntent {
            operation_id: CredentialOperationId::parse("8d8d8d8d-9e9e-4f5f-8060-b7b7b7b7b7b7")
                .expect("operation id"),
            idempotency_token: CredentialIdempotencyToken::parse(
                "8e8e8e8e-9f9f-4060-8171-c8c8c8c8c8c8",
            )
            .expect("idempotency token"),
            set_id: unrelated_set.clone(),
            mutation_kind: CredentialMutationKind::Replace,
            expected_revision: None,
            proposed_revision: CredentialRevision::parse("8f8f8f8f-a0a0-4171-8282-d9d9d9d9d9d9")
                .expect("revision"),
            recovery_state: CredentialSetRecoveryState::PendingIntent,
        });
        journal
            .set_state_mut(&unrelated_set)
            .expect("built-in unrelated set state")
            .recovery_state = CredentialSetRecoveryState::PendingIntent;

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.entries.lock().expect("memory keyring lock").push((
            "v2/_authority".to_owned(),
            encode_authority_marker(authority_instance_id).expect("marker envelope"),
        ));
        let journal_boundary = Arc::new(MemoryJournalBoundary {
            bytes: Mutex::new(Some(
                encode_authority_journal(authority_instance_id, &journal)
                    .expect("journal envelope"),
            )),
            ..MemoryJournalBoundary::default()
        });

        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                .expect("open exact matching authority pair"),
            NativeCredentialStoreOpen::RecoveryRequired
        ));
    }

    #[test]
    fn matching_terminal_epoch_journal_reopens_ready() {
        let authority_instance_id = "23232323-4545-4676-8789-010101010101";
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        keyring.entries.lock().expect("memory keyring lock").push((
            "v2/_authority".to_owned(),
            encode_authority_marker(authority_instance_id).expect("marker envelope"),
        ));
        let mut terminal = AuthorityJournal::new(CredentialBackendKind::Native);
        terminal.global_epoch = u64::MAX;
        let journal_boundary = Arc::new(MemoryJournalBoundary {
            bytes: Mutex::new(Some(
                encode_authority_journal(authority_instance_id, &terminal)
                    .expect("terminal journal envelope"),
            )),
            ..MemoryJournalBoundary::default()
        });

        match NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
            .expect("terminal journal remains readable")
        {
            NativeCredentialStoreOpen::Ready { journal, .. } => {
                assert_eq!(journal.global_epoch, u64::MAX);
                assert!(journal.pending_activation.is_none());
                assert!(journal.pending_intents.is_empty());
            }
            _ => panic!("matching terminal authority should reopen ready"),
        }
    }

    #[test]
    fn one_sided_malformed_mismatched_and_wrong_backend_authority_require_recovery() {
        fn assert_recovery(marker: Option<Vec<u8>>, journal: Option<Vec<u8>>) {
            let keyring = Arc::new(MemoryKeyringBoundary::default());
            if let Some(marker) = marker {
                keyring
                    .entries
                    .lock()
                    .expect("memory keyring lock")
                    .push(("v2/_authority".to_owned(), marker));
            }
            let journal_boundary = Arc::new(MemoryJournalBoundary {
                bytes: Mutex::new(journal),
                ..MemoryJournalBoundary::default()
            });
            assert!(matches!(
                NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                    .expect("invalid authority opens as a closed state"),
                NativeCredentialStoreOpen::RecoveryRequired
            ));
        }

        let first_id = "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee";
        let second_id = "11111111-2222-4333-8444-555555555555";
        let native_journal = AuthorityJournal::new(CredentialBackendKind::Native);
        let first_marker = encode_authority_marker(first_id).expect("first marker");
        let second_marker = encode_authority_marker(second_id).expect("second marker");
        let first_journal =
            encode_authority_journal(first_id, &native_journal).expect("first journal");

        assert_recovery(Some(first_marker.clone()), None);
        assert_recovery(None, Some(first_journal.clone()));
        assert_recovery(
            Some(b"{malformed-marker".to_vec()),
            Some(first_journal.clone()),
        );
        assert_recovery(
            Some(first_marker.clone()),
            Some(b"{malformed-journal".to_vec()),
        );
        assert_recovery(Some(second_marker), Some(first_journal));

        let wrong_backend = AuthorityJournal::new(CredentialBackendKind::InMemory);
        assert_recovery(
            Some(first_marker.clone()),
            Some(
                encode_authority_journal(first_id, &wrong_backend).expect("wrong-backend journal"),
            ),
        );

        let mut unsupported_marker: serde_json::Value =
            serde_json::from_slice(&first_marker).expect("marker JSON");
        unsupported_marker["schema_version"] = serde_json::json!(u32::MAX);
        assert_recovery(
            Some(serde_json::to_vec(&unsupported_marker).expect("unsupported marker")),
            Some(encode_authority_journal(first_id, &native_journal).expect("supported journal")),
        );
    }

    #[test]
    fn byte_malformed_authority_marker_is_recovery_required_not_parser_error() {
        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(
                Arc::new(CorruptMarkerBoundary),
                Arc::new(MemoryJournalBoundary::default()),
            )
            .expect("malformed marker is a closed composition state"),
            NativeCredentialStoreOpen::RecoveryRequired
        ));
    }

    #[test]
    fn ready_adapter_executes_one_service_replace_and_commits_secret_free_journal() {
        const SECRET_CANARY: &str = "native-entry-only-secret-canary";
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let (store, journal) = match initialized {
            NativeCredentialStoreOpen::Ready { store, journal } => (store, *journal),
            _ => panic!("expected ready authority"),
        };
        let service = CredentialService::new(
            Arc::new(store),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::from(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key(SECRET_CANARY).expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
                )
                .expect("canonical idempotency token"),
            })
            .expect("replace credential set");

        assert!(
            keyring
                .entries
                .lock()
                .expect("memory keyring lock")
                .iter()
                .any(|(account, _)| account == "v2/deepgram")
        );
        let persisted_journal = journal_boundary
            .bytes
            .lock()
            .expect("memory journal lock")
            .clone()
            .expect("persisted journal");
        assert!(
            !persisted_journal
                .windows(SECRET_CANARY.len())
                .any(|bytes| bytes == SECRET_CANARY.as_bytes())
        );

        match NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
            .expect("reopen committed authority")
        {
            NativeCredentialStoreOpen::Ready { journal, .. } => {
                assert_eq!(journal.global_epoch, 1)
            }
            _ => panic!("expected ready committed authority"),
        }
    }

    #[test]
    fn native_mutation_session_round_trips_and_scrubs_staging_before_release() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened =
            NativeKeyringCredentialStore::open_with_boundaries(keyring.clone(), journal_boundary)
                .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let store = match initialized {
            NativeCredentialStoreOpen::Ready { store, .. } => store,
            _ => panic!("expected ready authority"),
        };
        let operation_id = CredentialOperationId::parse("10101010-2020-4030-8040-505050505050")
            .expect("canonical operation id");
        let revision = CredentialRevision::parse("60606060-7070-4080-8090-a0a0a0a0a0a0")
            .expect("canonical revision");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let encoded = CredentialRecordEnvelope::present(
            set_id.clone(),
            AuthMethodId::ApiKey,
            revision,
            operation_id.clone(),
            StoredSecretBundle::api_key("native-staging-session-canary").expect("API key"),
        )
        .expect("valid staging record")
        .encode()
        .expect("encode staging record");
        let expected = encoded.as_bytes().to_vec();

        let mut session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin native mutation session");
        session.load_journal().expect("load paired native journal");
        session
            .write_staging(&operation_id, &set_id, encoded)
            .expect("write staging record");
        let readback = session
            .read_staging(&operation_id, &set_id)
            .expect("read staging record")
            .expect("staging record exists");
        assert_eq!(readback.as_bytes(), expected.as_slice());
        session
            .delete_staging(&operation_id, &set_id)
            .expect("delete and verify staging record");
        assert!(
            session
                .read_staging(&operation_id, &set_id)
                .expect("read deleted staging record")
                .is_none()
        );
        drop(session);

        assert!(
            keyring
                .entries
                .lock()
                .expect("memory keyring lock")
                .iter()
                .all(|(account, _)| !account.starts_with("v2-staging/"))
        );
    }

    #[test]
    fn mutation_session_rejects_scope_drift_and_unverified_overwrite_before_io() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal = Arc::new(MemoryJournalBoundary::default());
        let gate = NativeInteractionGate::isolated_for_test();
        let store = ready_store_with_gate(keyring.clone(), journal.clone(), gate);
        let operation_id = CredentialOperationId::parse("78787878-9a9a-4bcd-8f01-232323232323")
            .expect("canonical scoped operation id");
        let other_operation = CredentialOperationId::parse("89898989-abab-4cde-8012-343434343434")
            .expect("canonical other operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let other_set = CredentialSetId::from(BuiltInCredentialSetId::Assemblyai);
        let staging_record = CredentialRecordEnvelope::present(
            set_id.clone(),
            AuthMethodId::ApiKey,
            CredentialRevision::parse("dededede-f0f0-4123-a456-789abcdef012")
                .expect("canonical scoped revision"),
            operation_id.clone(),
            StoredSecretBundle::api_key("scope-drift-canary").expect("API key"),
        )
        .expect("valid scoped record")
        .encode()
        .expect("encode scoped record");
        let active_record = staging_record.copy_for_boundary();
        let replacement_record = active_record.copy_for_boundary();
        let mut session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin scoped mutation");
        let paired = session.load_journal().expect("load scoped journal");

        let reads_before_scope_drift = keyring.reads.load(Ordering::SeqCst);
        assert!(matches!(
            session.read_active(&other_set),
            Err(CredentialStoreFailure::Internal)
        ));
        assert_eq!(
            keyring.reads.load(Ordering::SeqCst),
            reads_before_scope_drift
        );
        session
            .write_staging(&operation_id, &set_id, staging_record)
            .expect("write scoped staging record");
        let reads_before_wrong_staging = keyring.reads.load(Ordering::SeqCst);
        assert!(matches!(
            session.read_staging(&other_operation, &set_id),
            Err(CredentialStoreFailure::Internal)
        ));
        assert_eq!(
            keyring.reads.load(Ordering::SeqCst),
            reads_before_wrong_staging
        );
        assert!(matches!(
            session.read_active(&set_id),
            Err(CredentialStoreFailure::OperationInProgress)
        ));
        assert!(matches!(
            session.readback_active(&set_id),
            Err(CredentialStoreFailure::OperationInProgress)
        ));
        assert_eq!(
            keyring.reads.load(Ordering::SeqCst),
            reads_before_wrong_staging
        );
        let journal_reads_before_pending = journal.reads.load(Ordering::SeqCst);
        assert_eq!(
            session.load_journal(),
            Err(CredentialStoreFailure::OperationInProgress)
        );
        assert_eq!(
            journal.reads.load(Ordering::SeqCst),
            journal_reads_before_pending
        );
        let journal_writes_before_pending = journal.writes.load(Ordering::SeqCst);
        assert_eq!(
            session.commit_journal(&paired),
            Err(CredentialStoreFailure::OperationInProgress)
        );
        assert_eq!(
            journal.writes.load(Ordering::SeqCst),
            journal_writes_before_pending
        );
        session
            .read_staging(&operation_id, &set_id)
            .expect("verify scoped staging record")
            .expect("scoped staging exists");
        session
            .delete_staging(&operation_id, &set_id)
            .expect("delete verified staging record");

        session
            .replace_active(&set_id, active_record)
            .expect("write scoped active record");
        let writes_before_overwrite = keyring.writes.load(Ordering::SeqCst);
        assert_eq!(
            session.replace_active(&set_id, replacement_record),
            Err(CredentialStoreFailure::OperationInProgress)
        );
        assert_eq!(
            keyring.writes.load(Ordering::SeqCst),
            writes_before_overwrite
        );
        let reads_before_wrong_active = keyring.reads.load(Ordering::SeqCst);
        assert!(matches!(
            session.readback_active(&other_set),
            Err(CredentialStoreFailure::Internal)
        ));
        assert_eq!(
            keyring.reads.load(Ordering::SeqCst),
            reads_before_wrong_active
        );
        assert!(matches!(
            session.read_active(&set_id),
            Err(CredentialStoreFailure::OperationInProgress)
        ));
        assert!(matches!(
            session.read_staging(&operation_id, &set_id),
            Err(CredentialStoreFailure::OperationInProgress)
        ));
        assert_eq!(
            keyring.reads.load(Ordering::SeqCst),
            reads_before_wrong_active
        );
        let journal_reads_before_active_pending = journal.reads.load(Ordering::SeqCst);
        assert_eq!(
            session.load_journal(),
            Err(CredentialStoreFailure::OperationInProgress)
        );
        assert_eq!(
            journal.reads.load(Ordering::SeqCst),
            journal_reads_before_active_pending
        );
        assert_eq!(
            session.commit_journal(&paired),
            Err(CredentialStoreFailure::OperationInProgress)
        );
        session
            .readback_active(&set_id)
            .expect("verify scoped active record")
            .expect("scoped active exists");
        session
            .commit_journal(&paired)
            .expect("commit after all native readbacks");
    }

    #[test]
    fn dropping_pending_readback_latches_active_and_staging_but_verified_drop_remains_healthy() {
        fn record(
            set_id: &CredentialSetId,
            operation_id: &CredentialOperationId,
            revision: &str,
            secret: &str,
        ) -> EncodedCredentialRecord {
            CredentialRecordEnvelope::present(
                set_id.clone(),
                AuthMethodId::ApiKey,
                CredentialRevision::parse(revision).expect("canonical drop-test revision"),
                operation_id.clone(),
                StoredSecretBundle::api_key(secret).expect("API key"),
            )
            .expect("valid drop-test record")
            .encode()
            .expect("encode drop-test record")
        }

        let active_keyring = Arc::new(MemoryKeyringBoundary::default());
        let active_journal = Arc::new(MemoryJournalBoundary::default());
        let active_store = ready_store_with_gate(
            active_keyring.clone(),
            active_journal.clone(),
            NativeInteractionGate::isolated_for_test(),
        );
        let active_operation = CredentialOperationId::parse("90909090-bcbc-4def-8123-454545454545")
            .expect("canonical pending active operation id");
        let active_set = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let mut active_session = active_store
            .begin_mutation(&active_operation, &active_set)
            .expect("begin pending active mutation");
        active_session
            .load_journal()
            .expect("load pending active journal");
        active_session
            .replace_active(
                &active_set,
                record(
                    &active_set,
                    &active_operation,
                    "efefefef-0101-4234-b567-89abcdef0123",
                    "pending-active-drop-canary",
                ),
            )
            .expect("write pending active record");
        let active_reads = active_keyring.reads.load(Ordering::SeqCst);
        let active_journal_reads = active_journal.reads.load(Ordering::SeqCst);
        let active_journal_writes = active_journal.writes.load(Ordering::SeqCst);
        drop(active_session);
        assert!(matches!(
            active_store.read_active(&active_set),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(active_keyring.reads.load(Ordering::SeqCst), active_reads);
        assert_eq!(
            active_journal.reads.load(Ordering::SeqCst),
            active_journal_reads
        );
        assert_eq!(
            active_journal.writes.load(Ordering::SeqCst),
            active_journal_writes
        );

        let staging_keyring = Arc::new(MemoryKeyringBoundary::default());
        let staging_journal = Arc::new(MemoryJournalBoundary::default());
        let staging_store = ready_store_with_gate(
            staging_keyring.clone(),
            staging_journal.clone(),
            NativeInteractionGate::isolated_for_test(),
        );
        let staging_operation =
            CredentialOperationId::parse("a1a1a1a1-cdcd-4ef0-8234-565656565656")
                .expect("canonical pending staging operation id");
        let staging_set = CredentialSetId::from(BuiltInCredentialSetId::Assemblyai);
        let mut staging_session = staging_store
            .begin_mutation(&staging_operation, &staging_set)
            .expect("begin pending staging mutation");
        staging_session
            .load_journal()
            .expect("load pending staging journal");
        staging_session
            .write_staging(
                &staging_operation,
                &staging_set,
                record(
                    &staging_set,
                    &staging_operation,
                    "f0f0f0f0-1212-4345-8678-9abcdef01234",
                    "pending-staging-drop-canary",
                ),
            )
            .expect("write pending staging record");
        let staging_reads = staging_keyring.reads.load(Ordering::SeqCst);
        let staging_journal_reads = staging_journal.reads.load(Ordering::SeqCst);
        let staging_journal_writes = staging_journal.writes.load(Ordering::SeqCst);
        drop(staging_session);
        assert!(matches!(
            staging_store.read_active(&staging_set),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(staging_keyring.reads.load(Ordering::SeqCst), staging_reads);
        assert_eq!(
            staging_journal.reads.load(Ordering::SeqCst),
            staging_journal_reads
        );
        assert_eq!(
            staging_journal.writes.load(Ordering::SeqCst),
            staging_journal_writes
        );

        let verified_keyring = Arc::new(MemoryKeyringBoundary::default());
        let verified_store = ready_store_with_gate(
            verified_keyring.clone(),
            Arc::new(MemoryJournalBoundary::default()),
            NativeInteractionGate::isolated_for_test(),
        );
        let verified_operation =
            CredentialOperationId::parse("b2b2b2b2-dede-4f01-8345-676767676767")
                .expect("canonical verified operation id");
        let verified_set = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let staging = record(
            &verified_set,
            &verified_operation,
            "01010101-2323-4456-8789-abcdef012345",
            "verified-drop-canary",
        );
        let active = staging.copy_for_boundary();
        let mut verified_session = verified_store
            .begin_mutation(&verified_operation, &verified_set)
            .expect("begin verified mutation");
        verified_session
            .load_journal()
            .expect("load verified journal");
        verified_session
            .write_staging(&verified_operation, &verified_set, staging)
            .expect("write verified staging");
        verified_session
            .read_staging(&verified_operation, &verified_set)
            .expect("verify staging")
            .expect("verified staging exists");
        verified_session
            .delete_staging(&verified_operation, &verified_set)
            .expect("delete verified staging");
        verified_session
            .replace_active(&verified_set, active)
            .expect("write verified active");
        verified_session
            .readback_active(&verified_set)
            .expect("verify active")
            .expect("verified active exists");
        drop(verified_session);
        let verified_reads = verified_keyring.reads.load(Ordering::SeqCst);
        assert!(matches!(
            verified_store.read_active(&verified_set),
            Ok(Some(_))
        ));
        assert_eq!(
            verified_keyring.reads.load(Ordering::SeqCst),
            verified_reads + 1
        );
    }

    #[test]
    fn wrong_active_and_staging_readback_latch_before_later_native_io() {
        fn ready_store(
            keyring: Arc<MemoryKeyringBoundary>,
            journal: Arc<MemoryJournalBoundary>,
            gate: &'static NativeInteractionGate,
        ) -> NativeKeyringCredentialStore {
            let opened =
                NativeKeyringCredentialStore::open_with_boundaries_and_gate(keyring, journal, gate)
                    .expect("open absent authority");
            let store = match opened {
                NativeCredentialStoreOpen::Uninitialized(store) => store,
                _ => panic!("expected uninitialized authority"),
            };
            match store
                .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
                .expect("initialize native authority")
            {
                NativeCredentialStoreOpen::Ready { store, .. } => store,
                _ => panic!("expected ready authority"),
            }
        }

        let active_keyring = Arc::new(MemoryKeyringBoundary::default());
        let active_journal = Arc::new(MemoryJournalBoundary::default());
        let active_gate = NativeInteractionGate::isolated_for_test();
        let active_store = ready_store(active_keyring.clone(), active_journal.clone(), active_gate);
        let active_operation = CredentialOperationId::parse("12121212-3434-4567-89ab-cdcdcdcdcdcd")
            .expect("canonical active operation id");
        let active_set = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let active_record = CredentialRecordEnvelope::present(
            active_set.clone(),
            AuthMethodId::ApiKey,
            CredentialRevision::parse("abababab-cdcd-4efe-8123-456789abcdef")
                .expect("canonical active revision"),
            active_operation.clone(),
            StoredSecretBundle::api_key("wrong-active-readback-canary").expect("API key"),
        )
        .expect("valid active record")
        .encode()
        .expect("encode active record");
        let mut active_session = active_store
            .begin_mutation(&active_operation, &active_set)
            .expect("begin active mutation");
        let active_paired = active_session.load_journal().expect("load active journal");
        active_session
            .replace_active(&active_set, active_record)
            .expect("write active record");
        active_keyring
            .corrupt_active_readback
            .store(true, Ordering::SeqCst);
        assert!(matches!(
            active_session.readback_active(&active_set),
            Err(CredentialStoreFailure::CommitUnknown)
        ));
        let active_reads = active_keyring.reads.load(Ordering::SeqCst);
        let active_writes = active_keyring.writes.load(Ordering::SeqCst);
        let active_deletes = active_keyring.deletes.load(Ordering::SeqCst);
        let active_journal_reads = active_journal.reads.load(Ordering::SeqCst);
        let active_journal_writes = active_journal.writes.load(Ordering::SeqCst);
        assert_eq!(
            active_session.load_journal(),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert_eq!(
            active_session.commit_journal(&active_paired),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert!(matches!(
            active_session.read_active(&active_set),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(active_keyring.reads.load(Ordering::SeqCst), active_reads);
        assert_eq!(active_keyring.writes.load(Ordering::SeqCst), active_writes);
        assert_eq!(
            active_keyring.deletes.load(Ordering::SeqCst),
            active_deletes
        );
        assert_eq!(
            active_journal.reads.load(Ordering::SeqCst),
            active_journal_reads
        );
        assert_eq!(
            active_journal.writes.load(Ordering::SeqCst),
            active_journal_writes
        );
        drop(active_session);
        assert!(matches!(
            active_store.read_active(&active_set),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(active_keyring.reads.load(Ordering::SeqCst), active_reads);

        let staging_keyring = Arc::new(MemoryKeyringBoundary::default());
        let staging_journal = Arc::new(MemoryJournalBoundary::default());
        let staging_gate = NativeInteractionGate::isolated_for_test();
        let staging_store = ready_store(
            staging_keyring.clone(),
            staging_journal.clone(),
            staging_gate,
        );
        let staging_operation =
            CredentialOperationId::parse("23232323-4545-4678-8abc-dededededede")
                .expect("canonical staging operation id");
        let staging_set = CredentialSetId::from(BuiltInCredentialSetId::Assemblyai);
        let staging_record = CredentialRecordEnvelope::present(
            staging_set.clone(),
            AuthMethodId::ApiKey,
            CredentialRevision::parse("bcbcbcbc-dede-4f0f-9234-56789abcdef0")
                .expect("canonical staging revision"),
            staging_operation.clone(),
            StoredSecretBundle::api_key("wrong-staging-readback-canary").expect("API key"),
        )
        .expect("valid staging record")
        .encode()
        .expect("encode staging record");
        let mut staging_session = staging_store
            .begin_mutation(&staging_operation, &staging_set)
            .expect("begin staging mutation");
        let staging_paired = staging_session
            .load_journal()
            .expect("load staging journal");
        staging_session
            .write_staging(&staging_operation, &staging_set, staging_record)
            .expect("write staging record");
        staging_keyring
            .corrupt_staging_readback
            .store(true, Ordering::SeqCst);
        assert!(matches!(
            staging_session.read_staging(&staging_operation, &staging_set),
            Err(CredentialStoreFailure::CommitUnknown)
        ));
        let staging_reads = staging_keyring.reads.load(Ordering::SeqCst);
        let staging_writes = staging_keyring.writes.load(Ordering::SeqCst);
        let staging_deletes = staging_keyring.deletes.load(Ordering::SeqCst);
        let staging_journal_reads = staging_journal.reads.load(Ordering::SeqCst);
        let staging_journal_writes = staging_journal.writes.load(Ordering::SeqCst);
        assert_eq!(
            staging_session.load_journal(),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert_eq!(
            staging_session.commit_journal(&staging_paired),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert!(matches!(
            staging_session.read_active(&staging_set),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(staging_keyring.reads.load(Ordering::SeqCst), staging_reads);
        assert_eq!(
            staging_keyring.writes.load(Ordering::SeqCst),
            staging_writes
        );
        assert_eq!(
            staging_keyring.deletes.load(Ordering::SeqCst),
            staging_deletes
        );
        assert_eq!(
            staging_journal.reads.load(Ordering::SeqCst),
            staging_journal_reads
        );
        assert_eq!(
            staging_journal.writes.load(Ordering::SeqCst),
            staging_journal_writes
        );
        drop(staging_session);
        assert!(matches!(
            staging_store.read_active(&staging_set),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(staging_keyring.reads.load(Ordering::SeqCst), staging_reads);
    }

    #[test]
    fn complete_ordinary_policy_trace_is_forbid_prompt_only() {
        fn take_trace(keyring: &MemoryKeyringBoundary) -> Vec<PolicyTraceStep> {
            std::mem::take(&mut *keyring.policy_trace.lock().expect("policy trace lock"))
        }

        fn assert_single_lease_trace(
            actual: &[PolicyTraceStep],
            expected_steps: &[(&'static str, PolicyTraceTarget)],
        ) {
            assert_eq!(
                actual
                    .iter()
                    .map(|step| (step.operation, step.target))
                    .collect::<Vec<_>>(),
                expected_steps
            );
            let prompt_id = actual.first().expect("non-empty policy trace").prompt_id;
            assert!(
                actual
                    .iter()
                    .all(|candidate| candidate.prompt_id == prompt_id),
                "one full interaction lease must supply one opaque prompt identity"
            );
        }

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let gate = NativeInteractionGate::isolated_for_test();
        let journal = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries_and_gate(
            keyring.clone(),
            journal,
            gate,
        )
        .expect("open absent authority");
        assert_single_lease_trace(
            &take_trace(&keyring),
            &[("forbid:get:authority", PolicyTraceTarget::Authority)],
        );
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        assert_eq!(BUILT_IN_CREDENTIAL_SET_IDS.len(), 17);
        let mut expected_initialize = vec![("forbid:get:authority", PolicyTraceTarget::Authority)];
        expected_initialize.extend(
            BUILT_IN_CREDENTIAL_SET_IDS
                .iter()
                .copied()
                .map(|set_id| ("forbid:get:active", PolicyTraceTarget::Active(set_id))),
        );
        expected_initialize.extend([
            ("forbid:set:authority", PolicyTraceTarget::Authority),
            ("forbid:get:authority", PolicyTraceTarget::Authority),
        ]);
        assert_single_lease_trace(&take_trace(&keyring), &expected_initialize);
        let store = match initialized {
            NativeCredentialStoreOpen::Ready { store, .. } => store,
            _ => panic!("expected ready authority"),
        };
        let operation_id = CredentialOperationId::parse("34343434-5656-4789-8bcd-efefefefefef")
            .expect("canonical trace operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        assert!(
            store
                .read_active(&set_id)
                .expect("provider-shaped active read")
                .is_none()
        );
        assert_single_lease_trace(
            &take_trace(&keyring),
            &[(
                "forbid:get:active",
                PolicyTraceTarget::Active(BuiltInCredentialSetId::Deepgram),
            )],
        );
        let staging_record = CredentialRecordEnvelope::present(
            set_id.clone(),
            AuthMethodId::ApiKey,
            CredentialRevision::parse("cdcdcdcd-efef-4012-a345-6789abcdef01")
                .expect("canonical trace revision"),
            operation_id.clone(),
            StoredSecretBundle::api_key("policy-trace-canary").expect("API key"),
        )
        .expect("valid trace record")
        .encode()
        .expect("encode trace record");
        let active_record = staging_record.copy_for_boundary();
        let mut session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin trace mutation");
        let paired = session.load_journal().expect("load trace journal");
        session
            .write_staging(&operation_id, &set_id, staging_record)
            .expect("write trace staging");
        session
            .read_staging(&operation_id, &set_id)
            .expect("read trace staging")
            .expect("trace staging exists");
        session
            .delete_staging(&operation_id, &set_id)
            .expect("cleanup trace staging");
        session
            .replace_active(&set_id, active_record)
            .expect("replace trace active");
        session
            .readback_active(&set_id)
            .expect("read back trace active")
            .expect("trace active exists");
        session
            .commit_journal(&paired)
            .expect("commit trace journal readback");
        drop(session);
        assert_single_lease_trace(
            &take_trace(&keyring),
            &[
                ("forbid:get:authority", PolicyTraceTarget::Authority),
                ("forbid:set:staging", PolicyTraceTarget::Staging),
                ("forbid:get:staging", PolicyTraceTarget::Staging),
                ("forbid:delete:staging", PolicyTraceTarget::Staging),
                ("forbid:get:staging", PolicyTraceTarget::Staging),
                (
                    "forbid:set:active",
                    PolicyTraceTarget::Active(BuiltInCredentialSetId::Deepgram),
                ),
                (
                    "forbid:get:active",
                    PolicyTraceTarget::Active(BuiltInCredentialSetId::Deepgram),
                ),
                ("forbid:get:authority", PolicyTraceTarget::Authority),
            ],
        );
    }

    #[test]
    fn corrupt_successful_journal_replacement_is_commit_unknown_and_reopens_recovery_required() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let (store, journal) = match initialized {
            NativeCredentialStoreOpen::Ready { store, journal } => (store, *journal),
            _ => panic!("expected ready authority"),
        };
        journal_boundary
            .corrupt_replacements
            .store(true, Ordering::SeqCst);
        let service = CredentialService::new(
            Arc::new(store),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        let error = service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::from(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("journal-readback-canary").expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "bbbbbbbb-cccc-dddd-eeee-ffffffffffff",
                )
                .expect("canonical idempotency token"),
            })
            .expect_err("corrupt journal readback cannot commit");

        assert_eq!(
            error.code,
            audio_graph_ipc_contract::credential_contract::CredentialErrorCode::CommitUnknown
        );
        assert!(
            !keyring
                .entries
                .lock()
                .expect("memory keyring lock")
                .iter()
                .any(|(account, _)| account == "v2/deepgram")
        );
        assert!(matches!(
            NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
                .expect("reopen corrupt authority"),
            NativeCredentialStoreOpen::RecoveryRequired
        ));
    }

    #[test]
    fn opaque_mutation_session_owns_the_file_lock_until_drop() {
        let root = std::env::temp_dir().join(format!(
            "audio-graph-fb2b-session-lock-{}",
            uuid::Uuid::new_v4().hyphenated()
        ));
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(FileAuthorityJournalBoundary::new(&root));
        let opened = NativeKeyringCredentialStore::open_with_adapters(
            KeyringEntryAdapter::new(keyring),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let store = match initialized {
            NativeCredentialStoreOpen::Ready { store, .. } => store,
            _ => panic!("expected ready authority"),
        };
        let operation_id = CredentialOperationId::parse("11111111-2222-3333-4444-555555555555")
            .expect("canonical operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);

        let session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin mutation session");
        assert!(matches!(
            journal_boundary.acquire_mutation_lock(),
            Err(CredentialStoreFailure::OperationInProgress)
        ));

        drop(session);
        let reacquired = journal_boundary
            .acquire_mutation_lock()
            .expect("session drop releases file lock");
        drop(reacquired);
        drop(store);
        std::fs::remove_dir_all(&root).expect("remove isolated session-lock test directory");
    }

    #[test]
    fn mutation_session_serializes_all_in_process_entry_access_until_drop() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(keyring, journal_boundary)
            .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let store = match initialized {
            NativeCredentialStoreOpen::Ready { store, .. } => Arc::new(store),
            _ => panic!("expected ready authority"),
        };
        let operation_id = CredentialOperationId::parse("11111111-2222-3333-4444-555555555555")
            .expect("canonical operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let session = store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin mutation session");
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let reader_store = store.clone();
        let reader_set_id = set_id.clone();
        let reader = std::thread::spawn(move || {
            started_tx.send(()).expect("announce blocked read");
            let result = reader_store.read_active(&reader_set_id);
            finished_tx
                .send(result.is_ok())
                .expect("report read result");
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("reader started");

        assert!(
            finished_rx
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
        drop(session);
        assert!(
            finished_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("reader completes after session drop")
        );
        reader.join().expect("reader thread");
    }

    #[test]
    fn ready_session_journal_cuts_preserve_prepublication_error_and_latch_uncertainty() {
        let before_keyring = Arc::new(MemoryKeyringBoundary::default());
        let before_journal = Arc::new(MemoryJournalBoundary::default());
        let before_gate = NativeInteractionGate::isolated_for_test();
        let before_store =
            ready_store_with_gate(before_keyring.clone(), before_journal.clone(), before_gate);
        let before_operation = CredentialOperationId::parse("45454545-6767-489a-8cde-f0f0f0f0f0f0")
            .expect("canonical before-publish operation id");
        let before_set = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let mut before_session = before_store
            .begin_mutation(&before_operation, &before_set)
            .expect("begin before-publish mutation");
        let before_paired = before_session
            .load_journal()
            .expect("load before-publish journal");
        *before_journal
            .replace_cut
            .lock()
            .expect("journal replace cut lock") = Some(JournalReplaceCut::BeforePublish);
        assert_eq!(
            before_session.commit_journal(&before_paired),
            Err(CredentialStoreFailure::Unavailable)
        );
        drop(before_session);
        let before_reads = before_keyring.reads.load(Ordering::SeqCst);
        assert!(matches!(before_store.read_active(&before_set), Ok(None)));
        assert_eq!(
            before_keyring.reads.load(Ordering::SeqCst),
            before_reads + 1
        );

        let after_keyring = Arc::new(MemoryKeyringBoundary::default());
        let after_journal = Arc::new(MemoryJournalBoundary::default());
        let after_gate = NativeInteractionGate::isolated_for_test();
        let after_store =
            ready_store_with_gate(after_keyring.clone(), after_journal.clone(), after_gate);
        let after_operation = CredentialOperationId::parse("56565656-7878-49ab-8def-010101010101")
            .expect("canonical after-publish operation id");
        let after_set = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let mut after_session = after_store
            .begin_mutation(&after_operation, &after_set)
            .expect("begin after-publish mutation");
        let after_paired = after_session
            .load_journal()
            .expect("load after-publish journal");
        *after_journal
            .replace_cut
            .lock()
            .expect("journal replace cut lock") = Some(JournalReplaceCut::AfterPublish);
        assert_eq!(
            after_session.commit_journal(&after_paired),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        let latched_keyring_reads = after_keyring.reads.load(Ordering::SeqCst);
        let latched_keyring_writes = after_keyring.writes.load(Ordering::SeqCst);
        let latched_journal_reads = after_journal.reads.load(Ordering::SeqCst);
        let latched_journal_writes = after_journal.writes.load(Ordering::SeqCst);
        assert_eq!(
            after_session.load_journal(),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert_eq!(
            after_session.commit_journal(&after_paired),
            Err(CredentialStoreFailure::StalledWorker)
        );
        assert!(matches!(
            after_session.read_active(&after_set),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(
            after_keyring.reads.load(Ordering::SeqCst),
            latched_keyring_reads
        );
        assert_eq!(
            after_keyring.writes.load(Ordering::SeqCst),
            latched_keyring_writes
        );
        assert_eq!(
            after_journal.reads.load(Ordering::SeqCst),
            latched_journal_reads
        );
        assert_eq!(
            after_journal.writes.load(Ordering::SeqCst),
            latched_journal_writes
        );
        drop(after_session);
        let after_keyring_reads = after_keyring.reads.load(Ordering::SeqCst);
        let after_journal_reads = after_journal.reads.load(Ordering::SeqCst);
        assert!(matches!(
            after_store.read_active(&after_set),
            Err(CredentialStoreFailure::StalledWorker)
        ));
        assert_eq!(
            after_keyring.reads.load(Ordering::SeqCst),
            after_keyring_reads
        );
        assert_eq!(
            after_journal.reads.load(Ordering::SeqCst),
            after_journal_reads
        );
    }

    #[test]
    fn shared_injected_gate_serializes_distinct_store_instances() {
        let gate = NativeInteractionGate::isolated_for_test();
        let first_keyring = Arc::new(MemoryKeyringBoundary::default());
        let first_store = Arc::new(ready_store_with_gate(
            first_keyring,
            Arc::new(MemoryJournalBoundary::default()),
            gate,
        ));
        let second_keyring = Arc::new(MemoryKeyringBoundary::default());
        let second_store = Arc::new(ready_store_with_gate(
            second_keyring.clone(),
            Arc::new(MemoryJournalBoundary::default()),
            gate,
        ));
        let operation_id = CredentialOperationId::parse("67676767-8989-4abc-8ef0-121212121212")
            .expect("canonical shared-gate operation id");
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let session = first_store
            .begin_mutation(&operation_id, &set_id)
            .expect("begin first-store mutation");
        let second_reads_before = second_keyring.reads.load(Ordering::SeqCst);
        let (started_tx, started_rx) = mpsc::channel();
        let (finished_tx, finished_rx) = mpsc::channel();
        let reader_store = second_store.clone();
        let reader_set_id = set_id.clone();
        let reader = std::thread::spawn(move || {
            started_tx.send(()).expect("announce second-store read");
            let result = reader_store.read_active(&reader_set_id);
            finished_tx
                .send(result)
                .expect("report second-store read result");
        });
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("second-store reader started");
        let deadline = Instant::now() + Duration::from_secs(1);
        while gate.queued_waiters_for_test() == 0 {
            assert!(
                Instant::now() < deadline,
                "second store did not queue on shared native interaction gate"
            );
            std::thread::yield_now();
        }

        assert_eq!(
            second_keyring.reads.load(Ordering::SeqCst),
            second_reads_before
        );
        assert!(finished_rx.try_recv().is_err());
        drop(session);
        assert!(matches!(
            finished_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("second-store read completes after lease drop"),
            Ok(None)
        ));
        reader.join().expect("second-store reader thread");
        assert_eq!(
            second_keyring.reads.load(Ordering::SeqCst),
            second_reads_before + 1
        );
    }

    #[test]
    fn portable_ceiling_accepts_2560_and_rejects_2561_before_native_invocation() {
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let revision = CredentialRevision::parse("77777777-7777-7777-7777-777777777777")
            .expect("canonical revision");
        let operation_id = CredentialOperationId::parse("88888888-8888-8888-8888-888888888888")
            .expect("canonical operation id");
        let mut largest_fitting_secret = None;
        for secret_length in 1..=PORTABLE_ENCODED_RECORD_MAX_BYTES + 1 {
            let record = CredentialRecordEnvelope::present(
                set_id.clone(),
                AuthMethodId::ApiKey,
                revision.clone(),
                operation_id.clone(),
                StoredSecretBundle::api_key("x".repeat(secret_length)).expect("API key"),
            )
            .expect("valid record shape");
            match record.encode() {
                Ok(encoded) => {
                    if encoded.as_bytes().len() == PORTABLE_ENCODED_RECORD_MAX_BYTES {
                        largest_fitting_secret = Some(secret_length);
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let largest_fitting_secret =
            largest_fitting_secret.expect("one exact 2,560-byte record exists");

        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened = NativeKeyringCredentialStore::open_with_boundaries(
            keyring.clone(),
            journal_boundary.clone(),
        )
        .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let (store, journal) = match initialized {
            NativeCredentialStoreOpen::Ready { store, journal } => (store, *journal),
            _ => panic!("expected ready authority"),
        };
        let service = CredentialService::new(
            Arc::new(store),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("x".repeat(largest_fitting_secret))
                    .expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "12121212-3434-5656-7878-909090909090",
                )
                .expect("canonical idempotency token"),
            })
            .expect("exact portable ceiling");
        let writes_after_exact_ceiling = keyring.writes.load(Ordering::SeqCst);
        let journal_writes_after_exact_ceiling = journal_boundary.writes.load(Ordering::SeqCst);

        let error = service
            .replace_set(ReplaceCredentialSet {
                set_id,
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("x".repeat(largest_fitting_secret + 1))
                    .expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "23232323-4545-6767-8989-101010101010",
                )
                .expect("canonical idempotency token"),
            })
            .expect_err("2,561-byte encoded record is rejected");

        assert_eq!(error.code, CredentialErrorCode::PayloadTooLarge);
        assert_eq!(
            keyring.writes.load(Ordering::SeqCst),
            writes_after_exact_ceiling
        );
        assert_eq!(
            journal_boundary.writes.load(Ordering::SeqCst),
            journal_writes_after_exact_ceiling
        );
    }

    #[test]
    fn active_delete_replaces_the_entry_with_a_tombstone_without_native_deletion() {
        let keyring = Arc::new(MemoryKeyringBoundary::default());
        let journal_boundary = Arc::new(MemoryJournalBoundary::default());
        let opened =
            NativeKeyringCredentialStore::open_with_boundaries(keyring.clone(), journal_boundary)
                .expect("open absent authority");
        let store = match opened {
            NativeCredentialStoreOpen::Uninitialized(store) => store,
            _ => panic!("expected uninitialized authority"),
        };
        let initialized = store
            .initialize(AuthorityJournal::new(CredentialBackendKind::Native))
            .expect("initialize native authority");
        let (store, journal) = match initialized {
            NativeCredentialStoreOpen::Ready { store, journal } => (store, *journal),
            _ => panic!("expected ready authority"),
        };
        let service = CredentialService::new(
            Arc::new(store),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let created = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("delete-native-canary").expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "34343434-5656-7878-9090-121212121212",
                )
                .expect("canonical idempotency token"),
            })
            .expect("create active entry");

        service
            .delete_set(DeleteCredentialSet {
                set_id: set_id.clone(),
                expected_revision: created.new_revision,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "45454545-6767-8989-1010-232323232323",
                )
                .expect("canonical idempotency token"),
            })
            .expect("tombstone active entry");

        let active_bytes = keyring
            .entries
            .lock()
            .expect("memory keyring lock")
            .iter()
            .find(|(account, _)| account == "v2/deepgram")
            .expect("retained active entry")
            .1
            .clone();
        let active = CredentialRecordEnvelope::decode(
            &EncodedCredentialRecord::from_boundary_bytes(active_bytes),
        )
        .expect("decode active tombstone");
        assert!(active.is_tombstone());
        assert_eq!(keyring.deletes.load(Ordering::SeqCst), 0);
    }
}
