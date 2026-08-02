//! Deterministic, secret-safe credential-service fakes.

use super::domain::{
    AuthorityJournal, CredentialAuthorityInstanceId, CredentialRecordEnvelope,
    CredentialStoreFailure, EncodedCredentialRecord, LoadedAuthorityJournal,
};
use super::service::{
    CredentialEntryStore, CredentialMutationSession, CredentialSettingsActivationPort,
    SettingsActivationIdentity, SettingsActivationTransaction,
};
use audio_graph_ipc_contract::credential_contract::{CredentialOperationId, CredentialSetId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FakeStoreCall {
    ReadActive,
    BeginMutation,
    LoadJournal,
    ReadActiveInSession,
    PersistIntent,
    ReplaceActive,
    ReadbackActive,
    WriteStaging,
    ReadStaging,
    DeleteStaging,
    CommitJournal,
    EndMutation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FakeSettingsCall {
    PersistPending,
    VerifyPending,
    VerifyCommitted,
    RestoreBackup,
    ClearPending,
}

pub(crate) struct FakeCredentialStore {
    state: Mutex<FakeCredentialState>,
    entry_reads: AtomicUsize,
    active_writes: AtomicUsize,
}

struct FakeCredentialState {
    authority_instance_id: CredentialAuthorityInstanceId,
    journal: AuthorityJournal,
    active: Vec<(CredentialSetId, Zeroizing<Vec<u8>>)>,
    staging: Vec<(CredentialOperationId, CredentialSetId, Zeroizing<Vec<u8>>)>,
    calls: Vec<FakeStoreCall>,
    failures: Vec<(FakeStoreCall, usize, CredentialStoreFailure)>,
    corrupt_next_readback: bool,
}

impl FakeCredentialStore {
    pub(crate) fn new(journal: AuthorityJournal) -> Self {
        Self::with_authority(
            journal,
            CredentialAuthorityInstanceId::from_test_bytes([0x11; 16]),
        )
    }

    pub(crate) fn with_authority(
        journal: AuthorityJournal,
        authority_instance_id: CredentialAuthorityInstanceId,
    ) -> Self {
        Self {
            state: Mutex::new(FakeCredentialState {
                authority_instance_id,
                journal,
                active: Vec::new(),
                staging: Vec::new(),
                calls: Vec::new(),
                failures: Vec::new(),
                corrupt_next_readback: false,
            }),
            entry_reads: AtomicUsize::new(0),
            active_writes: AtomicUsize::new(0),
        }
    }

    pub(crate) fn authority_instance_id(&self) -> CredentialAuthorityInstanceId {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .authority_instance_id
            .clone()
    }

    #[cfg(test)]
    pub(crate) fn replace_authority_instance_id_for_test(
        &self,
        authority_instance_id: CredentialAuthorityInstanceId,
    ) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .authority_instance_id = authority_instance_id;
    }

    pub(crate) fn entry_read_count(&self) -> usize {
        self.entry_reads.load(Ordering::SeqCst)
    }

    pub(crate) fn active_write_count(&self) -> usize {
        self.active_writes.load(Ordering::SeqCst)
    }

    pub(crate) fn active_record_is_tombstone(&self, set_id: &CredentialSetId) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .active
            .iter()
            .find(|(candidate, _)| candidate == set_id)
            .and_then(|(_, bytes)| {
                CredentialRecordEnvelope::decode(&EncodedCredentialRecord::from_boundary_bytes(
                    bytes.as_slice().to_vec(),
                ))
                .ok()
            })
            .is_some_and(|record| record.is_tombstone())
    }

    pub(crate) fn calls(&self) -> Vec<FakeStoreCall> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .calls
            .clone()
    }

    pub(crate) fn fail_next(&self, call: FakeStoreCall, failure: CredentialStoreFailure) {
        self.fail_after(call, 0, failure);
    }

    pub(crate) fn fail_after(
        &self,
        call: FakeStoreCall,
        successful_matches_before_failure: usize,
        failure: CredentialStoreFailure,
    ) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .failures
            .push((call, successful_matches_before_failure, failure));
    }

    pub(crate) fn corrupt_next_readback(&self) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .corrupt_next_readback = true;
    }

    pub(crate) fn staging_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .staging
            .len()
    }

    pub(crate) fn journal_snapshot(&self) -> AuthorityJournal {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .journal
            .clone()
    }
}

impl FakeCredentialState {
    fn record_call(&mut self, call: FakeStoreCall) -> Result<(), CredentialStoreFailure> {
        self.calls.push(call);
        if let Some((expected, remaining, _)) = self.failures.first_mut()
            && *expected == call
        {
            if *remaining == 0 {
                return Err(self.failures.remove(0).2);
            }
            *remaining -= 1;
        }
        Ok(())
    }
}

impl CredentialEntryStore for FakeCredentialStore {
    fn read_active(
        &self,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        self.entry_reads.fetch_add(1, Ordering::SeqCst);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeStoreCall::ReadActive)?;
        Ok(state
            .active
            .iter()
            .find(|(candidate, _)| candidate == set_id)
            .map(|(_, bytes)| {
                EncodedCredentialRecord::from_boundary_bytes(bytes.as_slice().to_vec())
            }))
    }

    fn begin_mutation(
        &self,
        _operation_id: &CredentialOperationId,
        _set_id: &CredentialSetId,
    ) -> Result<Box<dyn CredentialMutationSession + '_>, CredentialStoreFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeStoreCall::BeginMutation)?;
        Ok(Box::new(FakeMutationSession {
            state,
            active_writes: &self.active_writes,
        }))
    }
}

struct FakeMutationSession<'a> {
    state: MutexGuard<'a, FakeCredentialState>,
    active_writes: &'a AtomicUsize,
}

impl Drop for FakeMutationSession<'_> {
    fn drop(&mut self) {
        self.state.calls.push(FakeStoreCall::EndMutation);
    }
}

impl CredentialMutationSession for FakeMutationSession<'_> {
    fn load_journal(&mut self) -> Result<LoadedAuthorityJournal, CredentialStoreFailure> {
        self.state.record_call(FakeStoreCall::LoadJournal)?;
        Ok(LoadedAuthorityJournal::new(
            self.state.authority_instance_id.clone(),
            self.state.journal.clone(),
        ))
    }

    fn read_active(
        &mut self,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        self.state.record_call(FakeStoreCall::ReadActiveInSession)?;
        Ok(self
            .state
            .active
            .iter()
            .find(|(candidate, _)| candidate == set_id)
            .map(|(_, bytes)| {
                EncodedCredentialRecord::from_boundary_bytes(bytes.as_slice().to_vec())
            }))
    }

    fn persist_intent(&mut self, journal: &AuthorityJournal) -> Result<(), CredentialStoreFailure> {
        self.state.record_call(FakeStoreCall::PersistIntent)?;
        self.state.journal = journal.clone();
        Ok(())
    }

    fn replace_active(
        &mut self,
        set_id: &CredentialSetId,
        record: EncodedCredentialRecord,
    ) -> Result<(), CredentialStoreFailure> {
        self.state.record_call(FakeStoreCall::ReplaceActive)?;
        self.active_writes.fetch_add(1, Ordering::SeqCst);
        if let Some((_, bytes)) = self
            .state
            .active
            .iter_mut()
            .find(|(candidate, _)| candidate == set_id)
        {
            *bytes = Zeroizing::new(record.as_bytes().to_vec());
        } else {
            self.state
                .active
                .push((set_id.clone(), Zeroizing::new(record.as_bytes().to_vec())));
        }
        Ok(())
    }

    fn readback_active(
        &mut self,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        self.state.record_call(FakeStoreCall::ReadbackActive)?;
        if self.state.corrupt_next_readback {
            self.state.corrupt_next_readback = false;
            return Ok(Some(EncodedCredentialRecord::from_boundary_bytes(
                b"corrupt-readback".to_vec(),
            )));
        }
        Ok(self
            .state
            .active
            .iter()
            .find(|(candidate, _)| candidate == set_id)
            .map(|(_, bytes)| {
                EncodedCredentialRecord::from_boundary_bytes(bytes.as_slice().to_vec())
            }))
    }

    fn write_staging(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
        record: EncodedCredentialRecord,
    ) -> Result<(), CredentialStoreFailure> {
        self.state.record_call(FakeStoreCall::WriteStaging)?;
        self.state.staging.push((
            operation_id.clone(),
            set_id.clone(),
            Zeroizing::new(record.as_bytes().to_vec()),
        ));
        Ok(())
    }

    fn read_staging(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
        self.state.record_call(FakeStoreCall::ReadStaging)?;
        Ok(self
            .state
            .staging
            .iter()
            .find(|(operation, candidate, _)| operation == operation_id && candidate == set_id)
            .map(|(_, _, bytes)| {
                EncodedCredentialRecord::from_boundary_bytes(bytes.as_slice().to_vec())
            }))
    }

    fn delete_staging(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<(), CredentialStoreFailure> {
        self.state.record_call(FakeStoreCall::DeleteStaging)?;
        self.state
            .staging
            .retain(|(operation, candidate, _)| operation != operation_id || candidate != set_id);
        Ok(())
    }

    fn commit_journal(&mut self, journal: &AuthorityJournal) -> Result<(), CredentialStoreFailure> {
        self.state.record_call(FakeStoreCall::CommitJournal)?;
        self.state.journal = journal.clone();
        Ok(())
    }
}

pub(crate) struct FakeSettingsActivationPort {
    state: Mutex<FakeSettingsState>,
}

struct FakeSettingsState {
    current_revision: u64,
    pending: Option<SettingsActivationTransaction>,
    restored: Option<SettingsActivationIdentity>,
    cleared: Option<SettingsActivationIdentity>,
    calls: Vec<FakeSettingsCall>,
    failures: Vec<(FakeSettingsCall, CredentialStoreFailure)>,
    after_effect_failures: Vec<(FakeSettingsCall, CredentialStoreFailure)>,
}

impl FakeSettingsActivationPort {
    pub(crate) fn new(current_revision: u64) -> Self {
        Self {
            state: Mutex::new(FakeSettingsState {
                current_revision,
                pending: None,
                restored: None,
                cleared: None,
                calls: Vec::new(),
                failures: Vec::new(),
                after_effect_failures: Vec::new(),
            }),
        }
    }

    pub(crate) fn calls(&self) -> Vec<FakeSettingsCall> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .calls
            .clone()
    }

    pub(crate) fn current_revision(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .current_revision
    }

    pub(crate) fn has_pending_marker(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .is_some()
    }

    pub(crate) fn pending_transaction(&self) -> Option<SettingsActivationTransaction> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .clone()
    }

    pub(crate) fn restored_identity(&self) -> Option<SettingsActivationIdentity> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .restored
            .clone()
    }

    pub(crate) fn fail_next(&self, call: FakeSettingsCall, failure: CredentialStoreFailure) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .failures
            .push((call, failure));
    }

    pub(crate) fn fail_after_effect_next(
        &self,
        call: FakeSettingsCall,
        failure: CredentialStoreFailure,
    ) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .after_effect_failures
            .push((call, failure));
    }
}

impl FakeSettingsState {
    fn record_call(&mut self, call: FakeSettingsCall) -> Result<(), CredentialStoreFailure> {
        self.calls.push(call);
        if self
            .failures
            .first()
            .is_some_and(|(expected, _)| *expected == call)
        {
            return Err(self.failures.remove(0).1);
        }
        Ok(())
    }

    fn fail_after_effect(&mut self, call: FakeSettingsCall) -> Result<(), CredentialStoreFailure> {
        if self
            .after_effect_failures
            .first()
            .is_some_and(|(expected, _)| *expected == call)
        {
            return Err(self.after_effect_failures.remove(0).1);
        }
        Ok(())
    }
}

impl CredentialSettingsActivationPort for FakeSettingsActivationPort {
    fn persist_pending_settings(
        &self,
        transaction: &SettingsActivationTransaction,
    ) -> Result<(), CredentialStoreFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeSettingsCall::PersistPending)?;
        if let Some(pending) = state.pending.as_ref() {
            return if pending == transaction
                && state.current_revision == transaction.identity().proposed_settings_revision()
            {
                Ok(())
            } else {
                Err(CredentialStoreFailure::RevisionConflict)
            };
        }
        if state.current_revision != transaction.identity().expected_settings_revision() {
            return Err(CredentialStoreFailure::RevisionConflict);
        }
        state.pending = Some(transaction.clone());
        state.restored = None;
        state.cleared = None;
        state.current_revision = transaction.identity().proposed_settings_revision();
        state.fail_after_effect(FakeSettingsCall::PersistPending)?;
        Ok(())
    }

    fn verify_pending_settings(
        &self,
        identity: &SettingsActivationIdentity,
    ) -> Result<(), CredentialStoreFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeSettingsCall::VerifyPending)?;
        if state.current_revision != identity.proposed_settings_revision()
            || state
                .pending
                .as_ref()
                .is_none_or(|pending| pending.identity() != identity)
        {
            return Err(CredentialStoreFailure::RevisionConflict);
        }
        Ok(())
    }

    fn verify_committed_settings(
        &self,
        identity: &SettingsActivationIdentity,
    ) -> Result<(), CredentialStoreFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeSettingsCall::VerifyCommitted)?;
        if state.current_revision != identity.proposed_settings_revision()
            || match state.pending.as_ref() {
                Some(pending) => pending.identity() != identity,
                None => state.cleared.as_ref() != Some(identity),
            }
        {
            return Err(CredentialStoreFailure::RevisionConflict);
        }
        Ok(())
    }

    fn restore_settings_backup(
        &self,
        identity: &SettingsActivationIdentity,
    ) -> Result<(), CredentialStoreFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeSettingsCall::RestoreBackup)?;
        let Some(pending) = state.pending.as_ref() else {
            if state.current_revision == identity.expected_settings_revision()
                && state.cleared.is_none()
                && state
                    .restored
                    .as_ref()
                    .is_none_or(|restored| restored == identity)
            {
                state.restored = Some(identity.clone());
                return Ok(());
            }
            return Err(CredentialStoreFailure::RevisionConflict);
        };
        if pending.identity() != identity {
            return Err(CredentialStoreFailure::RevisionConflict);
        }
        state.current_revision = identity.expected_settings_revision();
        state.pending = None;
        state.restored = Some(identity.clone());
        Ok(())
    }

    fn clear_pending_settings(
        &self,
        identity: &SettingsActivationIdentity,
    ) -> Result<(), CredentialStoreFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeSettingsCall::ClearPending)?;
        if state.pending.is_none() {
            return if state.cleared.as_ref() == Some(identity)
                && state.current_revision == identity.proposed_settings_revision()
            {
                Ok(())
            } else {
                Err(CredentialStoreFailure::RevisionConflict)
            };
        }
        if state.current_revision != identity.proposed_settings_revision()
            || state
                .pending
                .as_ref()
                .is_none_or(|pending| pending.identity() != identity)
        {
            return Err(CredentialStoreFailure::RevisionConflict);
        }
        state.pending = None;
        state.cleared = Some(identity.clone());
        Ok(())
    }
}
