//! Deterministic, secret-safe credential-service fakes.

use super::domain::{
    AuthorityJournal, CredentialRecordEnvelope, CredentialStoreFailure, EncodedCredentialRecord,
};
use super::service::{
    AuthoritativeActiveRead, CredentialEntryStore, CredentialMutationSession,
    CredentialSettingsActivationPort, ObservedActivation, UntrackedRecordOutcome,
    UntrackedRecordRepair,
};
use audio_graph_ipc_contract::credential_contract::{
    BUILT_IN_CREDENTIAL_SET_IDS, CredentialOperationId, CredentialSetId,
};
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
    RepairUntracked,
    /// One built-in set's part of an untracked-record repair, after the
    /// journal was admitted.
    RepairSet,
}

/// The authority a fake store is bound to unless a test names another.
pub(crate) const FAKE_AUTHORITY: &str = "fake-authority";

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
    authority: &'static str,
}

struct FakeCredentialState {
    journal: AuthorityJournal,
    active: Vec<(CredentialSetId, Zeroizing<Vec<u8>>)>,
    staging: Vec<(CredentialOperationId, CredentialSetId, Zeroizing<Vec<u8>>)>,
    calls: Vec<FakeStoreCall>,
    failures: Vec<(FakeStoreCall, usize, CredentialStoreFailure)>,
    corrupt_next_readback: bool,
}

impl FakeCredentialStore {
    pub(crate) fn new(journal: AuthorityJournal) -> Self {
        Self::bound_to(journal, FAKE_AUTHORITY)
    }

    /// A store bound to `authority`, as a store opened over another
    /// authority pair would be.
    pub(crate) fn bound_to(journal: AuthorityJournal, authority: &'static str) -> Self {
        Self {
            state: Mutex::new(FakeCredentialState {
                journal,
                active: Vec::new(),
                staging: Vec::new(),
                calls: Vec::new(),
                failures: Vec::new(),
                corrupt_next_readback: false,
            }),
            entry_reads: AtomicUsize::new(0),
            active_writes: AtomicUsize::new(0),
            authority,
        }
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

    /// Puts `journal` back as the persisted journal, as a restore from an
    /// older copy of the same authority would.
    pub(crate) fn restore_journal(&self, journal: AuthorityJournal) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .journal = journal;
    }

    /// Leaves a staging record for `operation_id` in `set_id` that no journal
    /// records, as a lost journal publication would.
    pub(crate) fn plant_staging(
        &self,
        operation_id: CredentialOperationId,
        set_id: CredentialSetId,
    ) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .staging
            .push((operation_id, set_id, Zeroizing::new(b"planted".to_vec())));
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
        may_read: &dyn Fn(&AuthorityJournal) -> bool,
    ) -> Result<AuthoritativeActiveRead, CredentialStoreFailure> {
        self.entry_reads.fetch_add(1, Ordering::SeqCst);
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeStoreCall::ReadActive)?;
        let journal = state.journal.clone();
        let record = if may_read(&journal) {
            state
                .active
                .iter()
                .find(|(candidate, _)| candidate == set_id)
                .map(|(_, bytes)| {
                    EncodedCredentialRecord::from_boundary_bytes(bytes.as_slice().to_vec())
                })
        } else {
            None
        };
        Ok(AuthoritativeActiveRead { journal, record })
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

    /// Models what the native repair removes, not how it finds it. The fake
    /// has no staging guards, and it lists every staging record of a set,
    /// which the native store cannot: there, candidates are only the
    /// operation a guard names and those the journal history records, so a
    /// staging record neither names stays. Every staging record of a set is
    /// untracked here except the one of the journal's pending activation and
    /// those of `observed`. What the native repair can and cannot find is
    /// tested against the native store in `native_keyring.rs`.
    fn repair_untracked_records(
        &self,
        observed: &[ObservedActivation],
        admit: &dyn Fn(&AuthorityJournal) -> bool,
    ) -> Result<Option<Vec<UntrackedRecordRepair>>, CredentialStoreFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeStoreCall::RepairUntracked)?;
        let journal = state.journal.clone();
        if !admit(&journal) {
            return Ok(None);
        }
        let mut sets = Vec::with_capacity(BUILT_IN_CREDENTIAL_SET_IDS.len());
        for built_in in BUILT_IN_CREDENTIAL_SET_IDS {
            state.record_call(FakeStoreCall::RepairSet)?;
            let set_id = CredentialSetId::BuiltIn(*built_in);
            let spared: Vec<&CredentialOperationId> = journal
                .pending_activation
                .iter()
                .filter(|pending| pending.set_id == set_id)
                .map(|pending| &pending.operation_id)
                .chain(
                    observed
                        .iter()
                        .filter(|observed| observed.set_id == set_id)
                        .map(|observed| &observed.operation_id),
                )
                .collect();
            let before = state.staging.len();
            state.staging.retain(|(operation_id, staged, _)| {
                *staged != set_id || spared.contains(&operation_id)
            });
            let kept = state.staging.iter().any(|(_, staged, _)| *staged == set_id);
            let outcome = if state.staging.len() != before {
                UntrackedRecordOutcome::Removed
            } else if journal
                .pending_activation
                .as_ref()
                .is_some_and(|pending| pending.set_id == set_id)
                || kept
            {
                UntrackedRecordOutcome::Tracked
            } else {
                UntrackedRecordOutcome::Clean
            };
            sets.push(UntrackedRecordRepair { set_id, outcome });
        }
        Ok(Some(sets))
    }

    fn authority_instance(&self) -> &str {
        self.authority
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
    fn load_journal(&mut self) -> Result<AuthorityJournal, CredentialStoreFailure> {
        self.state.record_call(FakeStoreCall::LoadJournal)?;
        Ok(self.state.journal.clone())
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
    pending: Option<(CredentialOperationId, u64, u64)>,
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
        operation_id: &CredentialOperationId,
        _set_id: &CredentialSetId,
        expected_revision: u64,
        proposed_revision: u64,
    ) -> Result<(), CredentialStoreFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeSettingsCall::PersistPending)?;
        if state.pending.is_some() {
            return Err(CredentialStoreFailure::OperationInProgress);
        }
        if state.current_revision != expected_revision {
            return Err(CredentialStoreFailure::RevisionConflict);
        }
        state.pending = Some((operation_id.clone(), expected_revision, proposed_revision));
        state.current_revision = proposed_revision;
        state.fail_after_effect(FakeSettingsCall::PersistPending)?;
        Ok(())
    }

    fn verify_pending_settings(
        &self,
        operation_id: &CredentialOperationId,
        expected_revision: u64,
    ) -> Result<(), CredentialStoreFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeSettingsCall::VerifyPending)?;
        if state.current_revision != expected_revision
            || state
                .pending
                .as_ref()
                .is_none_or(|(pending_operation, _, proposed)| {
                    pending_operation != operation_id || *proposed != expected_revision
                })
        {
            return Err(CredentialStoreFailure::RevisionConflict);
        }
        Ok(())
    }

    fn verify_committed_settings(
        &self,
        operation_id: &CredentialOperationId,
        expected_revision: u64,
    ) -> Result<(), CredentialStoreFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeSettingsCall::VerifyCommitted)?;
        if state.current_revision != expected_revision
            || state
                .pending
                .as_ref()
                .is_some_and(|(pending_operation, _, proposed)| {
                    pending_operation != operation_id || *proposed != expected_revision
                })
        {
            return Err(CredentialStoreFailure::RevisionConflict);
        }
        Ok(())
    }

    fn restore_settings_backup(
        &self,
        operation_id: &CredentialOperationId,
        expected_revision: u64,
    ) -> Result<(), CredentialStoreFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeSettingsCall::RestoreBackup)?;
        let Some((pending_operation, backup, _)) = state.pending.as_ref() else {
            return Ok(());
        };
        if pending_operation != operation_id || *backup != expected_revision {
            return Err(CredentialStoreFailure::RevisionConflict);
        }
        state.current_revision = *backup;
        state.pending = None;
        Ok(())
    }

    fn clear_pending_settings(
        &self,
        operation_id: &CredentialOperationId,
        committed_revision: u64,
    ) -> Result<(), CredentialStoreFailure> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeSettingsCall::ClearPending)?;
        if state.current_revision == committed_revision && state.pending.is_none() {
            return Ok(());
        }
        if state.current_revision != committed_revision
            || state
                .pending
                .as_ref()
                .is_none_or(|(pending_operation, _, proposed)| {
                    pending_operation != operation_id || *proposed != committed_revision
                })
        {
            return Err(CredentialStoreFailure::RevisionConflict);
        }
        state.pending = None;
        Ok(())
    }
}
