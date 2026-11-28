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
    BUILT_IN_CREDENTIAL_SET_IDS, CredentialOperationId, CredentialRevision, CredentialSetId,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
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

/// Where a [`FaultPlan`] injects a failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FaultPoint {
    Store(FakeStoreCall),
    Settings(FakeSettingsCall),
}

/// One failure of a [`FaultPlan`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlannedFault {
    pub(crate) point: FaultPoint,
    /// Calls at `point` let through first, counted from when the fault
    /// before this one fired.
    pub(crate) skip: usize,
    pub(crate) failure: CredentialStoreFailure,
    /// Whether the call takes effect before it fails. Only the fake calls
    /// that change state honour it.
    pub(crate) after_effect: bool,
}

/// One call a [`FaultPlan`] saw, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TracedCall {
    pub(crate) point: FaultPoint,
    /// The call changed state; a call that refused before its effect did
    /// not.
    pub(crate) took_effect: bool,
}

/// Called at a free point: no session of the fake store is open, and a
/// session, a lease read, a repair or a settings call is about to start.
pub(crate) type FreePointHook = Arc<dyn Fn() + Send + Sync>;

/// Failures injected in order across a fake store and a fake settings
/// port that share the plan: only the first unfired fault is armed, so a
/// fault's count starts once the one before it fired, whichever fake that
/// was. The plan also traces every call it sees, and calls a hook at every
/// free point, so a test can interleave another service there.
pub(crate) struct FaultPlan {
    state: Mutex<FaultPlanState>,
}

struct FaultPlanState {
    pending: VecDeque<PlannedFault>,
    /// The first pending fault reached its call and fails once the call
    /// took effect.
    armed: bool,
    /// Each fault that fired, with the index of its call in `calls`.
    fired: Vec<(PlannedFault, usize)>,
    calls: Vec<TracedCall>,
    /// Faults and the trace concern only this thread's calls, when set.
    thread: Option<std::thread::ThreadId>,
    open_sessions: usize,
    free_point: Option<FreePointHook>,
}

impl FaultPlan {
    pub(crate) fn new(faults: impl IntoIterator<Item = PlannedFault>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(FaultPlanState {
                pending: faults.into_iter().collect(),
                armed: false,
                fired: Vec::new(),
                calls: Vec::new(),
                thread: None,
                open_sessions: 0,
                free_point: None,
            }),
        })
    }

    /// Applies the faults, and traces calls, only on the calling thread.
    pub(crate) fn restrict_to_current_thread(&self) {
        self.lock().thread = Some(std::thread::current().id());
    }

    /// Calls `hook` at every free point, on whichever thread reaches it.
    pub(crate) fn on_free_point(&self, hook: FreePointHook) {
        self.lock().free_point = Some(hook);
    }

    /// The calls traced since the last call, and each fault that fired in
    /// them with its call's index there.
    pub(crate) fn take_trace(&self) -> (Vec<TracedCall>, Vec<(PlannedFault, usize)>) {
        let mut state = self.lock();
        (
            std::mem::take(&mut state.calls),
            std::mem::take(&mut state.fired),
        )
    }

    /// How many planned faults have not fired.
    pub(crate) fn unfired(&self) -> usize {
        self.lock().pending.len()
    }

    /// Drops every fault that has not fired.
    pub(crate) fn disarm(&self) {
        let mut state = self.lock();
        state.pending.clear();
        state.armed = false;
    }

    fn lock(&self) -> MutexGuard<'_, FaultPlanState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn session_opened(&self) {
        self.lock().open_sessions += 1;
    }

    fn session_closed(&self) {
        self.lock().open_sessions -= 1;
    }

    fn free_point(&self) {
        let hook = {
            let state = self.lock();
            if state.open_sessions > 0 {
                return;
            }
            state.free_point.clone()
        };
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Before the call at `point` takes effect.
    fn before(&self, point: FaultPoint) -> Result<(), CredentialStoreFailure> {
        let mut state = self.lock();
        if !state.concerns_this_thread() {
            return Ok(());
        }
        state.calls.push(TracedCall {
            point,
            took_effect: false,
        });
        let Some(head) = state.pending.front_mut() else {
            return Ok(());
        };
        if head.point != point {
            return Ok(());
        }
        if head.skip > 0 {
            head.skip -= 1;
            return Ok(());
        }
        if head.after_effect {
            state.armed = true;
            return Ok(());
        }
        Err(state.fire())
    }

    /// After the call at `point` took effect.
    fn after(&self, point: FaultPoint) -> Result<(), CredentialStoreFailure> {
        let mut state = self.lock();
        if !state.concerns_this_thread() {
            return Ok(());
        }
        if let Some(call) = state.calls.last_mut() {
            call.took_effect = true;
        }
        if !state.armed || state.pending.front().is_none_or(|head| head.point != point) {
            return Ok(());
        }
        state.armed = false;
        Err(state.fire())
    }
}

impl FaultPlanState {
    fn concerns_this_thread(&self) -> bool {
        self.thread
            .is_none_or(|thread| thread == std::thread::current().id())
    }

    fn fire(&mut self) -> CredentialStoreFailure {
        let fault = self
            .pending
            .pop_front()
            .expect("only the first pending fault fires");
        let index = self.calls.len() - 1;
        self.fired.push((fault, index));
        fault.failure
    }
}

/// An in-memory store that makes the same scope and admission decisions as
/// the native one, without its native parts: it has no staging guards and
/// no uncertainty latch, so an unknown outcome it reports does not refuse
/// the next call the way the native store does. Tests that depend on either
/// run against the native store over an in-memory keyring.
pub(crate) struct FakeCredentialStore {
    state: Mutex<FakeCredentialState>,
    entry_reads: AtomicUsize,
    active_writes: AtomicUsize,
    authority: &'static str,
    /// The plan's handle outside `state`, for free points, which come
    /// before `state` is locked.
    plan: Mutex<Option<Arc<FaultPlan>>>,
}

struct FakeCredentialState {
    journal: AuthorityJournal,
    active: Vec<(CredentialSetId, Zeroizing<Vec<u8>>)>,
    staging: Vec<(CredentialOperationId, CredentialSetId, Zeroizing<Vec<u8>>)>,
    calls: Vec<FakeStoreCall>,
    failures: Vec<(FakeStoreCall, usize, CredentialStoreFailure)>,
    corrupt_next_readback: bool,
    /// A journal to put back as the persisted one right before a session
    /// load, once this many more loads have passed.
    restore_before_load: Option<(usize, AuthorityJournal)>,
    plan: Option<Arc<FaultPlan>>,
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
                restore_before_load: None,
                plan: None,
            }),
            entry_reads: AtomicUsize::new(0),
            active_writes: AtomicUsize::new(0),
            authority,
            plan: Mutex::new(None),
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

    /// Puts `journal` back as the persisted journal right before the session
    /// load that follows `loads_before` more loads, as another process's
    /// restore between two sessions of one operation would.
    pub(crate) fn restore_journal_before_load(
        &self,
        loads_before: usize,
        journal: AuthorityJournal,
    ) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .restore_before_load = Some((loads_before, journal));
    }

    /// Puts `record` in place as `set_id`'s active record without any
    /// journal change, as another process writing the native record
    /// directly would.
    pub(crate) fn substitute_active(
        &self,
        set_id: &CredentialSetId,
        record: &EncodedCredentialRecord,
    ) {
        let bytes = Zeroizing::new(record.as_bytes().to_vec());
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match state
            .active
            .iter_mut()
            .find(|(candidate, _)| candidate == set_id)
        {
            Some((_, active)) => *active = bytes,
            None => state.active.push((set_id.clone(), bytes)),
        }
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

    /// Puts `record` in `operation_id`'s staging slot for `set_id`, in
    /// place of whatever it holds, as another process writing the native
    /// staging record directly would.
    pub(crate) fn substitute_staging(
        &self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
        record: &EncodedCredentialRecord,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state
            .staging
            .retain(|(operation, staged, _)| operation != operation_id || staged != set_id);
        state.staging.push((
            operation_id.clone(),
            set_id.clone(),
            Zeroizing::new(record.as_bytes().to_vec()),
        ));
    }

    /// The revision of `set_id`'s active record, if it has one that
    /// decodes.
    pub(crate) fn active_revision(&self, set_id: &CredentialSetId) -> Option<CredentialRevision> {
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
            .map(|record| record.revision)
    }

    /// Injects the store's part of `plan`.
    pub(crate) fn set_fault_plan(&self, plan: Arc<FaultPlan>) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .plan = Some(Arc::clone(&plan));
        *self
            .plan
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(plan);
    }

    fn plan(&self) -> Option<Arc<FaultPlan>> {
        self.plan
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn free_point(&self) {
        if let Some(plan) = self.plan() {
            plan.free_point();
        }
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
        if let Some(plan) = &self.plan {
            plan.before(FaultPoint::Store(call))?;
        }
        Ok(())
    }

    /// After `call` took effect.
    fn took_effect(&self, call: FakeStoreCall) -> Result<(), CredentialStoreFailure> {
        match &self.plan {
            Some(plan) => plan.after(FaultPoint::Store(call)),
            None => Ok(()),
        }
    }
}

impl CredentialEntryStore for FakeCredentialStore {
    fn read_active(
        &self,
        set_id: &CredentialSetId,
        may_read: &dyn Fn(&AuthorityJournal) -> bool,
    ) -> Result<AuthoritativeActiveRead, CredentialStoreFailure> {
        self.entry_reads.fetch_add(1, Ordering::SeqCst);
        self.free_point();
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
        self.free_point();
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.record_call(FakeStoreCall::BeginMutation)?;
        let plan = self.plan();
        if let Some(plan) = &plan {
            plan.session_opened();
        }
        Ok(Box::new(FakeMutationSession {
            state,
            active_writes: &self.active_writes,
            plan,
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
        self.free_point();
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
    plan: Option<Arc<FaultPlan>>,
}

impl Drop for FakeMutationSession<'_> {
    fn drop(&mut self) {
        self.state.calls.push(FakeStoreCall::EndMutation);
        if let Some(plan) = &self.plan {
            plan.session_closed();
        }
    }
}

impl CredentialMutationSession for FakeMutationSession<'_> {
    fn load_journal(&mut self) -> Result<AuthorityJournal, CredentialStoreFailure> {
        self.state.record_call(FakeStoreCall::LoadJournal)?;
        match self.state.restore_before_load.take() {
            Some((0, journal)) => self.state.journal = journal,
            Some((remaining, journal)) => {
                self.state.restore_before_load = Some((remaining - 1, journal));
            }
            None => {}
        }
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
        self.state.took_effect(FakeStoreCall::PersistIntent)
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
        self.state.took_effect(FakeStoreCall::ReplaceActive)
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
        self.state.took_effect(FakeStoreCall::WriteStaging)
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
        self.state.took_effect(FakeStoreCall::DeleteStaging)
    }

    fn commit_journal(&mut self, journal: &AuthorityJournal) -> Result<(), CredentialStoreFailure> {
        self.state.record_call(FakeStoreCall::CommitJournal)?;
        self.state.journal = journal.clone();
        self.state.took_effect(FakeStoreCall::CommitJournal)
    }
}

pub(crate) struct FakeSettingsActivationPort {
    state: Mutex<FakeSettingsState>,
    /// The plan's handle outside `state`, for free points.
    plan: Mutex<Option<Arc<FaultPlan>>>,
}

struct FakeSettingsState {
    current_revision: u64,
    pending: Option<(CredentialOperationId, u64, u64)>,
    /// Operations whose settings write the restore fenced.
    fenced: Vec<CredentialOperationId>,
    calls: Vec<FakeSettingsCall>,
    failures: Vec<(FakeSettingsCall, CredentialStoreFailure)>,
    after_effect_failures: Vec<(FakeSettingsCall, CredentialStoreFailure)>,
    plan: Option<Arc<FaultPlan>>,
}

impl FakeSettingsActivationPort {
    pub(crate) fn new(current_revision: u64) -> Self {
        Self {
            state: Mutex::new(FakeSettingsState {
                current_revision,
                pending: None,
                fenced: Vec::new(),
                calls: Vec::new(),
                failures: Vec::new(),
                after_effect_failures: Vec::new(),
                plan: None,
            }),
            plan: Mutex::new(None),
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

    /// The operation whose marker is pending, if any.
    pub(crate) fn pending_operation(&self) -> Option<CredentialOperationId> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .pending
            .as_ref()
            .map(|(operation_id, _, _)| operation_id.clone())
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

    /// Injects the settings part of `plan`.
    pub(crate) fn set_fault_plan(&self, plan: Arc<FaultPlan>) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .plan = Some(Arc::clone(&plan));
        *self
            .plan
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(plan);
    }

    /// Locks the settings at a free point, after its hook ran.
    fn lock_at_free_point(&self) -> MutexGuard<'_, FakeSettingsState> {
        let plan = self
            .plan
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if let Some(plan) = plan {
            plan.free_point();
        }
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
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
        if let Some(plan) = &self.plan {
            plan.before(FaultPoint::Settings(call))?;
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
        if let Some(plan) = &self.plan {
            plan.after(FaultPoint::Settings(call))?;
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
        let mut state = self.lock_at_free_point();
        state.record_call(FakeSettingsCall::PersistPending)?;
        if state.fenced.contains(operation_id) {
            return Err(CredentialStoreFailure::RevisionConflict);
        }
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
        let mut state = self.lock_at_free_point();
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
        let mut state = self.lock_at_free_point();
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
        let mut state = self.lock_at_free_point();
        state.record_call(FakeSettingsCall::RestoreBackup)?;
        let restored = match state.pending.as_ref() {
            None => None,
            Some((pending_operation, backup, _))
                if pending_operation == operation_id && *backup == expected_revision =>
            {
                Some(*backup)
            }
            Some(_) => return Err(CredentialStoreFailure::RevisionConflict),
        };
        if let Some(backup) = restored {
            state.current_revision = backup;
            state.pending = None;
        }
        if !state.fenced.contains(operation_id) {
            state.fenced.push(operation_id.clone());
        }
        state.fail_after_effect(FakeSettingsCall::RestoreBackup)
    }

    fn clear_pending_settings(
        &self,
        operation_id: &CredentialOperationId,
        committed_revision: u64,
    ) -> Result<(), CredentialStoreFailure> {
        let mut state = self.lock_at_free_point();
        state.record_call(FakeSettingsCall::ClearPending)?;
        if state.current_revision == committed_revision && state.pending.is_none() {
            return state.fail_after_effect(FakeSettingsCall::ClearPending);
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
        state.fail_after_effect(FakeSettingsCall::ClearPending)
    }
}
