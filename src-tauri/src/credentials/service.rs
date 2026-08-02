use super::domain::{
    AuthorityJournal, CredentialAuthorityInstanceId, CredentialMutationKind,
    CredentialRecordEnvelope, CredentialRecordPayload, CredentialStoreFailure,
    EncodedCredentialRecord, IdempotencyJournalEntry, LoadedAuthorityJournal,
    PendingCredentialIntent, PendingSettingsActivation, StoredSecretBundle,
    ValidatedNonSecretSettingsDraft, content_free_error,
};
use audio_graph_ipc_contract::credential_contract::{
    AuthMethodId, CREDENTIAL_USE_POLICIES, CredentialActivationStage, CredentialActiveUseAction,
    CredentialAudience, CredentialBackendAvailability, CredentialError, CredentialErrorCode,
    CredentialIdempotencyToken, CredentialMutationReceipt, CredentialMutationResultCode,
    CredentialOperationId, CredentialPurpose, CredentialRevision, CredentialSafeRecoveryAction,
    CredentialServiceStatus, CredentialSetId, CredentialSetRecordState, CredentialSetRecoveryState,
    CredentialSetSource, CredentialUsePolicyDecisionDefinition, CredentialWorkerState,
    CredentialWorkerStatus, credential_use_policy_allows_audience,
};
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

#[cfg(test)]
use std::sync::mpsc::{Receiver, Sender};

const DEFAULT_EVENT_HISTORY_CAPACITY: usize = 64;

pub(crate) trait CredentialMutationSession {
    fn load_journal(&mut self) -> Result<LoadedAuthorityJournal, CredentialStoreFailure>;
    fn read_active(
        &mut self,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure>;
    fn persist_intent(&mut self, journal: &AuthorityJournal) -> Result<(), CredentialStoreFailure>;
    fn replace_active(
        &mut self,
        set_id: &CredentialSetId,
        record: EncodedCredentialRecord,
    ) -> Result<(), CredentialStoreFailure>;
    fn readback_active(
        &mut self,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure>;
    fn write_staging(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
        record: EncodedCredentialRecord,
    ) -> Result<(), CredentialStoreFailure>;
    fn read_staging(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure>;
    fn delete_staging(
        &mut self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<(), CredentialStoreFailure>;
    fn commit_journal(&mut self, journal: &AuthorityJournal) -> Result<(), CredentialStoreFailure>;
}

/// Entry reads and mutation-session creation are the only core-visible store
/// operations. Every write lives on the opaque session, so a concrete adapter
/// must retain its cooperating-process lock until that session is dropped.
pub(crate) trait CredentialEntryStore: Send + Sync {
    fn read_active(
        &self,
        set_id: &CredentialSetId,
    ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure>;

    fn begin_mutation(
        &self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<Box<dyn CredentialMutationSession + '_>, CredentialStoreFailure>;
}

fn verify_exact_store_readback(
    expected: &EncodedCredentialRecord,
    actual: Option<&EncodedCredentialRecord>,
) -> Result<(), CredentialStoreFailure> {
    if actual.map(EncodedCredentialRecord::as_bytes) == Some(expected.as_bytes()) {
        Ok(())
    } else {
        Err(CredentialStoreFailure::CommitUnknown)
    }
}

pub(crate) trait CredentialTokenSource: Send + Sync {
    fn next_operation_id(&self) -> CredentialOperationId;
    fn next_revision(&self) -> CredentialRevision;
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SettingsActivationIdentity {
    operation_id: CredentialOperationId,
    authority_instance_id: CredentialAuthorityInstanceId,
    set_id: CredentialSetId,
    expected_revision: Option<CredentialRevision>,
    proposed_revision: CredentialRevision,
    expected_settings_revision: u64,
    proposed_settings_revision: u64,
}

impl SettingsActivationIdentity {
    pub(crate) fn operation_id(&self) -> &CredentialOperationId {
        &self.operation_id
    }

    pub(crate) fn authority_instance_id(&self) -> &CredentialAuthorityInstanceId {
        &self.authority_instance_id
    }

    pub(crate) fn set_id(&self) -> &CredentialSetId {
        &self.set_id
    }

    pub(crate) fn expected_credential_revision(&self) -> Option<&CredentialRevision> {
        self.expected_revision.as_ref()
    }

    pub(crate) fn proposed_credential_revision(&self) -> &CredentialRevision {
        &self.proposed_revision
    }

    pub(crate) fn expected_settings_revision(&self) -> u64 {
        self.expected_settings_revision
    }

    pub(crate) fn proposed_settings_revision(&self) -> u64 {
        self.proposed_settings_revision
    }
}

impl fmt::Debug for SettingsActivationIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SettingsActivationIdentity([OPAQUE])")
    }
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct SettingsActivationTransaction {
    identity: SettingsActivationIdentity,
    settings_draft: ValidatedNonSecretSettingsDraft,
}

impl SettingsActivationTransaction {
    fn new(
        identity: SettingsActivationIdentity,
        settings_draft: ValidatedNonSecretSettingsDraft,
    ) -> Self {
        Self {
            identity,
            settings_draft,
        }
    }

    pub(crate) fn identity(&self) -> &SettingsActivationIdentity {
        &self.identity
    }

    pub(crate) fn settings_draft(&self) -> &ValidatedNonSecretSettingsDraft {
        &self.settings_draft
    }
}

impl fmt::Debug for SettingsActivationTransaction {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SettingsActivationTransaction([REDACTED])")
    }
}

pub(crate) trait CredentialSettingsActivationPort: Send + Sync {
    fn persist_pending_settings(
        &self,
        transaction: &SettingsActivationTransaction,
    ) -> Result<(), CredentialStoreFailure>;

    fn verify_pending_settings(
        &self,
        identity: &SettingsActivationIdentity,
    ) -> Result<(), CredentialStoreFailure>;

    fn verify_committed_settings(
        &self,
        identity: &SettingsActivationIdentity,
    ) -> Result<(), CredentialStoreFailure>;

    fn restore_settings_backup(
        &self,
        identity: &SettingsActivationIdentity,
    ) -> Result<(), CredentialStoreFailure>;

    fn clear_pending_settings(
        &self,
        identity: &SettingsActivationIdentity,
    ) -> Result<(), CredentialStoreFailure>;
}

struct UnsupportedSettingsActivationPort;

impl CredentialSettingsActivationPort for UnsupportedSettingsActivationPort {
    fn persist_pending_settings(
        &self,
        _transaction: &SettingsActivationTransaction,
    ) -> Result<(), CredentialStoreFailure> {
        Err(CredentialStoreFailure::Unsupported)
    }

    fn verify_pending_settings(
        &self,
        _identity: &SettingsActivationIdentity,
    ) -> Result<(), CredentialStoreFailure> {
        Err(CredentialStoreFailure::Unsupported)
    }

    fn verify_committed_settings(
        &self,
        _identity: &SettingsActivationIdentity,
    ) -> Result<(), CredentialStoreFailure> {
        Err(CredentialStoreFailure::Unsupported)
    }

    fn restore_settings_backup(
        &self,
        _identity: &SettingsActivationIdentity,
    ) -> Result<(), CredentialStoreFailure> {
        Err(CredentialStoreFailure::Unsupported)
    }

    fn clear_pending_settings(
        &self,
        _identity: &SettingsActivationIdentity,
    ) -> Result<(), CredentialStoreFailure> {
        Err(CredentialStoreFailure::Unsupported)
    }
}

pub(crate) struct ReplaceCredentialSet {
    pub(crate) set_id: CredentialSetId,
    pub(crate) auth_method_id: AuthMethodId,
    pub(crate) material: StoredSecretBundle,
    pub(crate) expected_revision: Option<CredentialRevision>,
    pub(crate) idempotency_token: CredentialIdempotencyToken,
}

pub(crate) struct DeleteCredentialSet {
    pub(crate) set_id: CredentialSetId,
    pub(crate) expected_revision: Option<CredentialRevision>,
    pub(crate) idempotency_token: CredentialIdempotencyToken,
}

pub(crate) struct PrepareCredentialActivation {
    pub(crate) set_id: CredentialSetId,
    pub(crate) auth_method_id: AuthMethodId,
    pub(crate) material: StoredSecretBundle,
    pub(crate) expected_revision: Option<CredentialRevision>,
    pub(crate) expected_settings_revision: u64,
    pub(crate) proposed_settings_revision: u64,
    pub(crate) idempotency_token: CredentialIdempotencyToken,
    pub(crate) settings_draft: ValidatedNonSecretSettingsDraft,
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct PreparedCredentialActivation {
    transaction: SettingsActivationTransaction,
    idempotency_token: CredentialIdempotencyToken,
    auth_method_id: AuthMethodId,
    expected_global_epoch: u64,
    proposed_global_epoch: u64,
}

impl PreparedCredentialActivation {
    pub(crate) fn transaction(&self) -> &SettingsActivationTransaction {
        &self.transaction
    }

    pub(crate) fn settings_identity(&self) -> &SettingsActivationIdentity {
        self.transaction.identity()
    }
}

impl std::ops::Deref for PreparedCredentialActivation {
    type Target = SettingsActivationIdentity;

    fn deref(&self) -> &Self::Target {
        self.transaction.identity()
    }
}

impl fmt::Debug for PreparedCredentialActivation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PreparedCredentialActivation([REDACTED])")
    }
}

#[derive(Clone, PartialEq, Eq)]
struct RestartedCredentialActivation {
    identity: SettingsActivationIdentity,
    idempotency_token: CredentialIdempotencyToken,
    auth_method_id: AuthMethodId,
    expected_global_epoch: u64,
    proposed_global_epoch: u64,
}

impl std::ops::Deref for RestartedCredentialActivation {
    type Target = SettingsActivationIdentity;

    fn deref(&self) -> &Self::Target {
        &self.identity
    }
}

impl fmt::Debug for RestartedCredentialActivation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RestartedCredentialActivation([OPAQUE])")
    }
}

trait CredentialActivationContext: std::ops::Deref<Target = SettingsActivationIdentity> {
    fn settings_identity(&self) -> &SettingsActivationIdentity {
        self.deref()
    }

    fn idempotency_token(&self) -> &CredentialIdempotencyToken;
    fn auth_method_id(&self) -> AuthMethodId;
    fn expected_global_epoch(&self) -> u64;
    fn proposed_global_epoch(&self) -> u64;
}

impl CredentialActivationContext for PreparedCredentialActivation {
    fn idempotency_token(&self) -> &CredentialIdempotencyToken {
        &self.idempotency_token
    }

    fn auth_method_id(&self) -> AuthMethodId {
        self.auth_method_id
    }

    fn expected_global_epoch(&self) -> u64 {
        self.expected_global_epoch
    }

    fn proposed_global_epoch(&self) -> u64 {
        self.proposed_global_epoch
    }
}

impl CredentialActivationContext for RestartedCredentialActivation {
    fn idempotency_token(&self) -> &CredentialIdempotencyToken {
        &self.idempotency_token
    }

    fn auth_method_id(&self) -> AuthMethodId {
        self.auth_method_id
    }

    fn expected_global_epoch(&self) -> u64 {
        self.expected_global_epoch
    }

    fn proposed_global_epoch(&self) -> u64 {
        self.proposed_global_epoch
    }
}

#[derive(Debug)]
enum ActivationStageTransitionError {
    Ineligible(CredentialError),
    Failed(CredentialError),
}

impl ActivationStageTransitionError {
    fn into_public(self) -> CredentialError {
        match self {
            Self::Ineligible(error) | Self::Failed(error) => error,
        }
    }
}

enum SettingsPendingAttemptFailure {
    Persist(CredentialStoreFailure),
    Verify(CredentialStoreFailure),
}

pub(crate) struct CredentialUseRequest {
    pub(crate) set_id: CredentialSetId,
    pub(crate) consumer_id: &'static str,
    pub(crate) auth_method_id: AuthMethodId,
    pub(crate) purpose: CredentialPurpose,
    pub(crate) audience: CredentialAudience,
}

pub(crate) struct CredentialLease {
    pub(crate) set_id: CredentialSetId,
    pub(crate) revision: CredentialRevision,
    material: StoredSecretBundle,
}

impl CredentialLease {
    pub(crate) fn expose_api_key<R>(&self, use_secret: impl FnOnce(&str) -> R) -> Option<R> {
        self.material.expose_api_key(use_secret)
    }

    pub(crate) fn expose_aws_static<R>(
        &self,
        use_secret: impl FnOnce(&str, &str, Option<&str>) -> R,
    ) -> Option<R> {
        self.material.expose_aws_static(use_secret)
    }
}

impl fmt::Debug for CredentialLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialLease")
            .field("set_id", &self.set_id)
            .field("revision", &self.revision)
            .field("material", &"[REDACTED]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NonStoreAuthority {
    pub(crate) set_id: CredentialSetId,
    pub(crate) auth_method_id: AuthMethodId,
    pub(crate) purpose: CredentialPurpose,
    pub(crate) active_use_action: CredentialActiveUseAction,
}

pub(crate) enum CredentialResolution {
    Stored(CredentialLease),
    NonStore(NonStoreAuthority),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialUseInvalidation {
    pub(crate) consumer_id: &'static str,
    pub(crate) purpose: CredentialPurpose,
    pub(crate) action: CredentialActiveUseAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialChangeEvent {
    pub(crate) global_epoch: u64,
    pub(crate) receipt: CredentialMutationReceipt,
    pub(crate) invalidations: Vec<CredentialUseInvalidation>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialEventBatch {
    pub(crate) latest_epoch: u64,
    pub(crate) gap_detected: bool,
    pub(crate) events: Vec<CredentialChangeEvent>,
}

struct CredentialEventLog {
    capacity: usize,
    service_start_epoch: u64,
    events: VecDeque<CredentialChangeEvent>,
}

impl CredentialEventLog {
    fn new(capacity: usize, service_start_epoch: u64) -> Self {
        Self {
            capacity: capacity.max(1),
            service_start_epoch,
            events: VecDeque::new(),
        }
    }

    fn publish(&mut self, event: CredentialChangeEvent) {
        self.events.push_back(event);
        while self.events.len() > self.capacity {
            self.events.pop_front();
        }
    }

    fn since(&self, after_epoch: u64, latest_epoch: u64) -> CredentialEventBatch {
        let first_retained_epoch = self.events.front().map(|event| event.global_epoch);
        let last_retained_epoch = self
            .events
            .back()
            .map_or(self.service_start_epoch, |event| event.global_epoch);
        let gap_detected = after_epoch < self.service_start_epoch
            || first_retained_epoch.is_some_and(|first_retained| {
                after_epoch
                    .checked_add(1)
                    .is_some_and(|next_epoch| next_epoch < first_retained)
            })
            || (after_epoch < latest_epoch && last_retained_epoch < latest_epoch);
        CredentialEventBatch {
            latest_epoch,
            gap_detected,
            events: if gap_detected {
                Vec::new()
            } else {
                self.events
                    .iter()
                    .filter(|event| {
                        event.global_epoch > after_epoch && event.global_epoch <= latest_epoch
                    })
                    .cloned()
                    .collect()
            },
        }
    }
}

impl fmt::Debug for CredentialResolution {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stored(lease) => formatter.debug_tuple("Stored").field(lease).finish(),
            Self::NonStore(authority) => {
                formatter.debug_tuple("NonStore").field(authority).finish()
            }
        }
    }
}

struct WorkerAdmission {
    serial: Mutex<()>,
    status: Mutex<CredentialWorkerStatus>,
}

impl WorkerAdmission {
    fn new() -> Self {
        Self {
            serial: Mutex::new(()),
            status: Mutex::new(CredentialWorkerStatus {
                state: CredentialWorkerState::Idle,
                operation_id: None,
                set_id: None,
            }),
        }
    }

    fn status(&self) -> CredentialWorkerStatus {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn admit(
        &self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
    ) -> Result<WorkerPermit<'_>, CredentialError> {
        if self.status().state == CredentialWorkerState::Stalled {
            return Err(CredentialStoreFailure::StalledWorker.into_public(Some(set_id.clone())));
        }
        let serial = self
            .serial
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if status.state == CredentialWorkerState::Stalled {
            return Err(CredentialStoreFailure::StalledWorker.into_public(Some(set_id.clone())));
        }
        *status = CredentialWorkerStatus {
            state: CredentialWorkerState::Busy,
            operation_id: Some(operation_id.clone()),
            set_id: Some(set_id.clone()),
        };
        drop(status);
        Ok(WorkerPermit {
            admission: self,
            _serial: serial,
        })
    }

    fn mark_stalled(&self) {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .state = CredentialWorkerState::Stalled;
    }

    fn complete_stalled_operation(&self, operation_id: &CredentialOperationId) -> bool {
        let mut status = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if status.state == CredentialWorkerState::Stalled
            && status.operation_id.as_ref() == Some(operation_id)
        {
            *status = CredentialWorkerStatus {
                state: CredentialWorkerState::Idle,
                operation_id: None,
                set_id: None,
            };
            return true;
        }
        false
    }
}

struct WorkerPermit<'a> {
    admission: &'a WorkerAdmission,
    _serial: MutexGuard<'a, ()>,
}

impl Drop for WorkerPermit<'_> {
    fn drop(&mut self) {
        let mut status = self
            .admission
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if status.state != CredentialWorkerState::Stalled {
            *status = CredentialWorkerStatus {
                state: CredentialWorkerState::Idle,
                operation_id: None,
                set_id: None,
            };
        }
    }
}

pub(crate) struct CredentialService {
    store: Arc<dyn CredentialEntryStore>,
    journal: Mutex<AuthorityJournal>,
    token_source: Arc<dyn CredentialTokenSource>,
    worker: WorkerAdmission,
    events: Mutex<CredentialEventLog>,
    settings_activation: Arc<dyn CredentialSettingsActivationPort>,
    #[cfg(test)]
    event_snapshot_test_hook: Mutex<Option<EventSnapshotTestHook>>,
    #[cfg(test)]
    activation_before_active_test_hook: Mutex<Option<ActivationBeforeActiveTestHook>>,
}

#[cfg(test)]
struct EventSnapshotTestHook {
    snapshot_captured: Sender<()>,
    resume: Receiver<()>,
}

#[cfg(test)]
struct ActivationBeforeActiveTestHook {
    credential_pending: Sender<()>,
    resume: Receiver<()>,
}

impl CredentialService {
    pub(crate) fn new(
        store: Arc<dyn CredentialEntryStore>,
        journal: AuthorityJournal,
        token_source: Arc<dyn CredentialTokenSource>,
    ) -> Self {
        Self::with_event_capacity(store, journal, token_source, DEFAULT_EVENT_HISTORY_CAPACITY)
    }

    fn with_event_capacity(
        store: Arc<dyn CredentialEntryStore>,
        journal: AuthorityJournal,
        token_source: Arc<dyn CredentialTokenSource>,
        event_capacity: usize,
    ) -> Self {
        Self::with_ports(
            store,
            journal,
            token_source,
            Arc::new(UnsupportedSettingsActivationPort),
            event_capacity,
        )
    }

    fn with_settings_activation_port(
        store: Arc<dyn CredentialEntryStore>,
        journal: AuthorityJournal,
        token_source: Arc<dyn CredentialTokenSource>,
        settings_activation: Arc<dyn CredentialSettingsActivationPort>,
    ) -> Self {
        Self::with_ports(
            store,
            journal,
            token_source,
            settings_activation,
            DEFAULT_EVENT_HISTORY_CAPACITY,
        )
    }

    fn with_ports(
        store: Arc<dyn CredentialEntryStore>,
        journal: AuthorityJournal,
        token_source: Arc<dyn CredentialTokenSource>,
        settings_activation: Arc<dyn CredentialSettingsActivationPort>,
        event_capacity: usize,
    ) -> Self {
        let service_start_epoch = journal.global_epoch;
        Self {
            store,
            journal: Mutex::new(journal),
            token_source,
            worker: WorkerAdmission::new(),
            events: Mutex::new(CredentialEventLog::new(event_capacity, service_start_epoch)),
            settings_activation,
            #[cfg(test)]
            event_snapshot_test_hook: Mutex::new(None),
            #[cfg(test)]
            activation_before_active_test_hook: Mutex::new(None),
        }
    }

    /// Returns only the committed, non-secret in-memory journal projection.
    /// This method has no path to [`CredentialEntryStore`].
    pub(crate) fn snapshot_status(&self) -> CredentialServiceStatus {
        self.journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot(self.worker.status())
    }

    pub(crate) fn events_since(&self, after_epoch: u64) -> CredentialEventBatch {
        let latest_epoch = self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .global_epoch;
        #[cfg(test)]
        self.run_event_snapshot_test_hook();
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .since(after_epoch, latest_epoch)
    }

    #[cfg(test)]
    fn pause_next_event_snapshot(&self, snapshot_captured: Sender<()>, resume: Receiver<()>) {
        *self
            .event_snapshot_test_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(EventSnapshotTestHook {
            snapshot_captured,
            resume,
        });
    }

    #[cfg(test)]
    fn run_event_snapshot_test_hook(&self) {
        let hook = self
            .event_snapshot_test_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(hook) = hook {
            hook.snapshot_captured
                .send(())
                .expect("event snapshot test receiver remains connected");
            hook.resume
                .recv()
                .expect("event snapshot test sender resumes the reader");
        }
    }

    #[cfg(test)]
    fn pause_next_activation_before_active_commit(
        &self,
        credential_pending: Sender<()>,
        resume: Receiver<()>,
    ) {
        *self
            .activation_before_active_test_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(ActivationBeforeActiveTestHook {
                credential_pending,
                resume,
            });
    }

    #[cfg(test)]
    fn run_activation_before_active_test_hook(&self) {
        let hook = self
            .activation_before_active_test_hook
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(hook) = hook {
            hook.credential_pending
                .send(())
                .expect("activation test receiver remains connected");
            hook.resume
                .recv()
                .expect("activation test sender resumes the commit");
        }
    }

    fn publish_committed_change(&self, receipt: CredentialMutationReceipt, global_epoch: u64) {
        let invalidations = match &receipt.set_id {
            CredentialSetId::BuiltIn(set_id) => CREDENTIAL_USE_POLICIES
                .iter()
                .filter(|policy| {
                    policy.set_id == *set_id
                        && matches!(
                            policy.decision,
                            CredentialUsePolicyDecisionDefinition::Authorized { .. }
                        )
                })
                .map(|policy| CredentialUseInvalidation {
                    consumer_id: policy.consumer_id,
                    purpose: policy.purpose,
                    action: policy.active_use_action,
                })
                .collect(),
            CredentialSetId::Custom(_) => Vec::new(),
        };
        self.events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .publish(CredentialChangeEvent {
                global_epoch,
                receipt,
                invalidations,
            });
    }

    fn observe_store_failure(&self, failure: &CredentialStoreFailure) {
        if *failure == CredentialStoreFailure::StalledWorker {
            self.worker.mark_stalled();
        }
    }

    fn map_store_failure(
        &self,
        failure: CredentialStoreFailure,
        set_id: &CredentialSetId,
    ) -> CredentialError {
        self.observe_store_failure(&failure);
        failure.into_public(Some(set_id.clone()))
    }

    fn require_core_managed_set(set_id: &CredentialSetId) -> Result<(), CredentialError> {
        if matches!(set_id, CredentialSetId::BuiltIn(_)) {
            return Ok(());
        }
        Err(content_free_error(
            CredentialErrorCode::InvalidCredentialSet,
            CredentialSafeRecoveryAction::ReenterCredential,
            Some(set_id.clone()),
        ))
    }

    fn ensure_target_has_no_pending_mutation(
        journal: &AuthorityJournal,
        set_id: &CredentialSetId,
    ) -> Result<(), CredentialError> {
        if journal.pending_activation.is_some()
            || journal
                .pending_intents
                .iter()
                .any(|intent| &intent.set_id == set_id)
        {
            return Err(
                CredentialStoreFailure::OperationInProgress.into_public(Some(set_id.clone()))
            );
        }
        if journal
            .set_state(set_id)
            .is_some_and(|set| set.recovery_state != CredentialSetRecoveryState::None)
        {
            return Err(content_free_error(
                CredentialErrorCode::RecoveryRequired,
                CredentialSafeRecoveryAction::Reconcile,
                Some(set_id.clone()),
            ));
        }
        Ok(())
    }

    fn next_committed_epoch(
        journal: &AuthorityJournal,
        set_id: &CredentialSetId,
    ) -> Result<u64, CredentialError> {
        journal.global_epoch.checked_add(1).ok_or_else(|| {
            content_free_error(
                CredentialErrorCode::RecoveryRequired,
                CredentialSafeRecoveryAction::Reconcile,
                Some(set_id.clone()),
            )
        })
    }

    fn cache_journal(&self, journal: &AuthorityJournal) {
        *self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = journal.clone();
    }

    fn reconcile_authoritative_journal(
        &self,
        authoritative: &AuthorityJournal,
        set_id: &CredentialSetId,
    ) -> Result<(), CredentialError> {
        let mut cached = self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if authoritative.global_epoch < cached.global_epoch {
            return Err(content_free_error(
                CredentialErrorCode::RecoveryRequired,
                CredentialSafeRecoveryAction::Reconcile,
                Some(set_id.clone()),
            ));
        }
        *cached = authoritative.clone();
        Ok(())
    }

    fn active_readback_is_commit_unknown(
        &self,
        failure: CredentialStoreFailure,
        set_id: &CredentialSetId,
    ) -> CredentialError {
        self.observe_store_failure(&failure);
        CredentialStoreFailure::CommitUnknown.into_public(Some(set_id.clone()))
    }

    fn proposed_activation_record_matches(
        record: &CredentialRecordEnvelope,
        prepared: &impl CredentialActivationContext,
    ) -> bool {
        record.set_id == prepared.set_id
            && record.revision == prepared.proposed_revision
            && record.operation_id == prepared.operation_id
            && matches!(
                &record.payload,
                CredentialRecordPayload::Present { auth_method_id, .. }
                    if *auth_method_id == prepared.auth_method_id()
            )
    }

    fn expected_activation_record_is_authoritative(
        active_record: Option<&CredentialRecordEnvelope>,
        journal: &AuthorityJournal,
        prepared: &impl CredentialActivationContext,
    ) -> bool {
        match (
            active_record,
            prepared.expected_revision.as_ref(),
            journal.set_state(&prepared.set_id),
        ) {
            (None, None, Some(set)) => {
                set.revision.is_none() && set.record_state == CredentialSetRecordState::Missing
            }
            (Some(record), Some(expected_revision), Some(set)) => {
                record.set_id == prepared.set_id
                    && &record.revision == expected_revision
                    && set.revision.as_ref() == Some(expected_revision)
                    && matches!(
                        (set.record_state, &record.payload),
                        (
                            CredentialSetRecordState::Configured,
                            CredentialRecordPayload::Present { .. }
                        ) | (
                            CredentialSetRecordState::Tombstoned,
                            CredentialRecordPayload::Tombstone
                        )
                    )
            }
            _ => false,
        }
    }

    fn normal_activation_stage_transition_is_allowed(
        expected_stage: CredentialActivationStage,
        next_stage: CredentialActivationStage,
    ) -> bool {
        matches!(
            (expected_stage, next_stage),
            (
                CredentialActivationStage::Staged,
                CredentialActivationStage::SettingsPending
            ) | (
                CredentialActivationStage::SettingsPending,
                CredentialActivationStage::CredentialPending
            )
        )
    }

    fn pending_activation_matches_prepared(
        pending: &PendingSettingsActivation,
        prepared: &impl CredentialActivationContext,
    ) -> bool {
        pending.operation_id == prepared.operation_id
            && &pending.idempotency_token == prepared.idempotency_token()
            && pending.set_id == prepared.set_id
            && pending.auth_method_id == prepared.auth_method_id()
            && pending.expected_revision == prepared.expected_revision
            && pending.proposed_revision == prepared.proposed_revision
            && pending.expected_settings_revision == prepared.expected_settings_revision
            && pending.proposed_settings_revision == prepared.proposed_settings_revision
            && pending.expected_global_epoch == prepared.expected_global_epoch()
            && pending.proposed_global_epoch == prepared.proposed_global_epoch()
    }

    fn prepared_activation_epoch_is_authoritative(
        journal: &AuthorityJournal,
        prepared: &impl CredentialActivationContext,
    ) -> bool {
        journal.global_epoch == prepared.expected_global_epoch()
            && prepared.expected_global_epoch().checked_add(1)
                == Some(prepared.proposed_global_epoch())
            && journal
                .pending_activation
                .as_ref()
                .is_some_and(|pending| Self::pending_activation_matches_prepared(pending, prepared))
    }

    fn require_authoritative_prepared_activation(
        authority_instance_id: &CredentialAuthorityInstanceId,
        journal: &AuthorityJournal,
        prepared: &impl CredentialActivationContext,
    ) -> Result<(), CredentialError> {
        if authority_instance_id == &prepared.authority_instance_id
            && Self::prepared_activation_epoch_is_authoritative(journal, prepared)
        {
            return Ok(());
        }
        Err(content_free_error(
            CredentialErrorCode::RecoveryRequired,
            CredentialSafeRecoveryAction::Reconcile,
            Some(prepared.set_id.clone()),
        ))
    }

    /// Adapter completion signal for an OS call that previously crossed its
    /// deadline. A timeout alone never clears the stalled admission gate.
    pub(crate) fn complete_stalled_operation(&self, operation_id: &CredentialOperationId) -> bool {
        self.worker.complete_stalled_operation(operation_id)
    }

    pub(crate) fn resolve_for_use(
        &self,
        request: CredentialUseRequest,
    ) -> Result<CredentialResolution, CredentialError> {
        let CredentialSetId::BuiltIn(built_in_set_id) = &request.set_id else {
            return Err(content_free_error(
                CredentialErrorCode::AudienceNotAllowed,
                CredentialSafeRecoveryAction::None,
                Some(request.set_id),
            ));
        };
        let policy = CREDENTIAL_USE_POLICIES.iter().find(|policy| {
            policy.set_id == *built_in_set_id
                && policy.consumer_id == request.consumer_id
                && policy.auth_method_id == request.auth_method_id
                && policy.purpose == request.purpose
                && matches!(
                    policy.decision,
                    CredentialUsePolicyDecisionDefinition::Authorized { .. }
                )
                && credential_use_policy_allows_audience(policy, &request.audience)
        });
        let Some(policy) = policy else {
            return Err(content_free_error(
                CredentialErrorCode::AudienceNotAllowed,
                CredentialSafeRecoveryAction::None,
                Some(request.set_id),
            ));
        };

        if matches!(
            request.auth_method_id,
            AuthMethodId::GoogleServiceAccountFile
                | AuthMethodId::AwsProfile
                | AuthMethodId::AwsDefaultChain
        ) {
            return Ok(CredentialResolution::NonStore(NonStoreAuthority {
                set_id: request.set_id,
                auth_method_id: request.auth_method_id,
                purpose: request.purpose,
                active_use_action: policy.active_use_action,
            }));
        }

        if !matches!(
            request.auth_method_id,
            AuthMethodId::ApiKey | AuthMethodId::AwsStatic
        ) {
            return Err(content_free_error(
                CredentialErrorCode::AudienceNotAllowed,
                CredentialSafeRecoveryAction::None,
                Some(request.set_id),
            ));
        }

        let (committed_set, pending_activation) = {
            let journal = self
                .journal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                journal.set_state(&request.set_id).cloned(),
                journal.pending_activation.clone(),
            )
        };
        let Some(committed_set) = committed_set else {
            return Err(content_free_error(
                CredentialErrorCode::RecoveryRequired,
                CredentialSafeRecoveryAction::Reconcile,
                Some(request.set_id),
            ));
        };
        let recovery_allows_entry_read = match committed_set.recovery_state {
            CredentialSetRecoveryState::None => pending_activation
                .as_ref()
                .is_none_or(|pending| pending.set_id != request.set_id),
            CredentialSetRecoveryState::PendingIntent => {
                pending_activation.as_ref().is_some_and(|pending| {
                    pending.set_id == request.set_id
                        && pending.stage == CredentialActivationStage::Staged
                })
            }
            CredentialSetRecoveryState::RecordJournalMismatch
            | CredentialSetRecoveryState::CommitUnknown => false,
        };
        if !recovery_allows_entry_read {
            return Err(content_free_error(
                CredentialErrorCode::RecoveryRequired,
                CredentialSafeRecoveryAction::Reconcile,
                Some(request.set_id),
            ));
        }

        let operation_id = self.token_source.next_operation_id();
        let _permit = self.worker.admit(&operation_id, &request.set_id)?;
        let encoded = self
            .store
            .read_active(&request.set_id)
            .map_err(|failure| self.map_store_failure(failure, &request.set_id))?
            .ok_or_else(|| {
                content_free_error(
                    CredentialErrorCode::Missing,
                    CredentialSafeRecoveryAction::ReenterCredential,
                    Some(request.set_id.clone()),
                )
            })?;
        let record = CredentialRecordEnvelope::decode(&encoded)?;
        if record.set_id != request.set_id {
            return Err(content_free_error(
                CredentialErrorCode::CorruptRecord,
                CredentialSafeRecoveryAction::Reconcile,
                Some(request.set_id),
            ));
        }
        let state_matches = committed_set.revision.as_ref() == Some(&record.revision)
            && matches!(
                (committed_set.record_state, record.is_tombstone()),
                (CredentialSetRecordState::Configured, false)
                    | (CredentialSetRecordState::Tombstoned, true)
            );
        let recovery_allows_resolution = match committed_set.recovery_state {
            CredentialSetRecoveryState::None => pending_activation
                .as_ref()
                .is_none_or(|pending| pending.set_id != request.set_id),
            CredentialSetRecoveryState::PendingIntent => {
                pending_activation.as_ref().is_some_and(|pending| {
                    pending.set_id == request.set_id
                        && pending.expected_revision.as_ref() == Some(&record.revision)
                        && pending.stage == CredentialActivationStage::Staged
                })
            }
            CredentialSetRecoveryState::RecordJournalMismatch
            | CredentialSetRecoveryState::CommitUnknown => false,
        };
        if !state_matches || !recovery_allows_resolution {
            return Err(content_free_error(
                CredentialErrorCode::RecoveryRequired,
                CredentialSafeRecoveryAction::Reconcile,
                Some(request.set_id),
            ));
        }
        if record.is_tombstone() {
            return Err(content_free_error(
                CredentialErrorCode::Missing,
                CredentialSafeRecoveryAction::ReenterCredential,
                Some(request.set_id),
            ));
        }
        let revision = record.revision.clone();
        let Some((auth_method_id, material)) = record.into_present() else {
            unreachable!("tombstone returned above")
        };
        if auth_method_id != request.auth_method_id {
            return Err(content_free_error(
                CredentialErrorCode::CorruptRecord,
                CredentialSafeRecoveryAction::Reconcile,
                Some(request.set_id),
            ));
        }
        Ok(CredentialResolution::Stored(CredentialLease {
            set_id: request.set_id,
            revision,
            material,
        }))
    }

    pub(crate) fn replace_set(
        &self,
        request: ReplaceCredentialSet,
    ) -> Result<CredentialMutationReceipt, CredentialError> {
        Self::require_core_managed_set(&request.set_id)?;
        let operation_id = self.token_source.next_operation_id();
        let new_revision = self.token_source.next_revision();
        let record = CredentialRecordEnvelope::present(
            request.set_id.clone(),
            request.auth_method_id,
            new_revision.clone(),
            operation_id.clone(),
            request.material,
        )?;
        let encoded = record.encode()?;
        let expected_readback = encoded.copy_for_boundary();
        let _permit = self.worker.admit(&operation_id, &request.set_id)?;

        let (committed_journal, receipt) = {
            let mut session = self
                .store
                .begin_mutation(&operation_id, &request.set_id)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            let (_, mut journal) = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?
                .into_parts();

            if let Some(previous) = journal.idempotency_entry(&request.idempotency_token) {
                if previous.set_id == request.set_id
                    && previous.mutation_kind == CredentialMutationKind::Replace
                    && previous.expected_revision == request.expected_revision
                {
                    let receipt = previous.receipt.clone();
                    self.reconcile_authoritative_journal(&journal, &request.set_id)?;
                    return Ok(receipt);
                }
                Self::ensure_target_has_no_pending_mutation(&journal, &request.set_id)?;
                return Err(content_free_error(
                    CredentialErrorCode::Conflict,
                    CredentialSafeRecoveryAction::Retry,
                    Some(request.set_id),
                ));
            }
            Self::ensure_target_has_no_pending_mutation(&journal, &request.set_id)?;
            let committed_epoch = Self::next_committed_epoch(&journal, &request.set_id)?;

            let active = session
                .read_active(&request.set_id)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            let current_revision = match active {
                Some(encoded) => {
                    let record = CredentialRecordEnvelope::decode(&encoded)?;
                    if record.set_id != request.set_id {
                        return Err(content_free_error(
                            CredentialErrorCode::CorruptRecord,
                            CredentialSafeRecoveryAction::Reconcile,
                            Some(request.set_id),
                        ));
                    }
                    Some(record.revision)
                }
                None => None,
            };
            let journal_revision = journal
                .set_state(&request.set_id)
                .and_then(|set| set.revision.clone());
            if journal_revision != current_revision {
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(request.set_id),
                ));
            }
            if request.expected_revision != current_revision {
                return Err(
                    CredentialStoreFailure::RevisionConflict.into_public(Some(request.set_id))
                );
            }

            journal.pending_intents.push(PendingCredentialIntent {
                operation_id: operation_id.clone(),
                idempotency_token: request.idempotency_token.clone(),
                set_id: request.set_id.clone(),
                mutation_kind: CredentialMutationKind::Replace,
                expected_revision: current_revision.clone(),
                proposed_revision: new_revision.clone(),
                recovery_state: CredentialSetRecoveryState::PendingIntent,
            });
            if let Some(set) = journal.set_state_mut(&request.set_id) {
                set.recovery_state = CredentialSetRecoveryState::PendingIntent;
            }
            session
                .persist_intent(&journal)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            self.cache_journal(&journal);
            session
                .replace_active(&request.set_id, encoded)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            let readback = session
                .readback_active(&request.set_id)
                .map_err(|failure| {
                    self.active_readback_is_commit_unknown(failure, &request.set_id)
                })?;
            verify_exact_store_readback(&expected_readback, readback.as_ref())
                .map_err(|failure| failure.into_public(Some(request.set_id.clone())))?;

            let result_code = if current_revision.is_some() {
                CredentialMutationResultCode::Replaced
            } else {
                CredentialMutationResultCode::Created
            };
            let receipt = CredentialMutationReceipt {
                operation_id: operation_id.clone(),
                idempotency_token: request.idempotency_token.clone(),
                set_id: request.set_id.clone(),
                previous_revision: current_revision.clone(),
                new_revision: Some(new_revision.clone()),
                result_code,
                recovery_action: CredentialSafeRecoveryAction::None,
            };
            journal
                .pending_intents
                .retain(|intent| intent.operation_id != operation_id);
            journal.backend.availability = CredentialBackendAvailability::Available;
            let Some(set) = journal.set_state_mut(&request.set_id) else {
                return Err(content_free_error(
                    CredentialErrorCode::InvalidCredentialSet,
                    CredentialSafeRecoveryAction::ReenterCredential,
                    Some(request.set_id),
                ));
            };
            set.record_state = CredentialSetRecordState::Configured;
            set.source = CredentialSetSource::NativeV2;
            set.revision = Some(new_revision);
            set.recovery_state = CredentialSetRecoveryState::None;
            set.pending_activation = false;
            journal.record_idempotency(IdempotencyJournalEntry {
                idempotency_token: request.idempotency_token,
                set_id: request.set_id,
                mutation_kind: CredentialMutationKind::Replace,
                expected_revision: current_revision,
                receipt: receipt.clone(),
            });
            journal.global_epoch = committed_epoch;
            session.commit_journal(&journal).map_err(|failure| {
                self.observe_store_failure(&failure);
                CredentialStoreFailure::CommitUnknown.into_public(Some(receipt.set_id.clone()))
            })?;
            (journal, receipt)
        };

        let global_epoch = committed_journal.global_epoch;
        *self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = committed_journal;
        self.publish_committed_change(receipt.clone(), global_epoch);
        Ok(receipt)
    }

    pub(crate) fn delete_set(
        &self,
        request: DeleteCredentialSet,
    ) -> Result<CredentialMutationReceipt, CredentialError> {
        Self::require_core_managed_set(&request.set_id)?;
        let operation_id = self.token_source.next_operation_id();
        let new_revision = self.token_source.next_revision();
        let encoded = CredentialRecordEnvelope::tombstone(
            request.set_id.clone(),
            new_revision.clone(),
            operation_id.clone(),
        )
        .encode()?;
        let expected_readback = encoded.copy_for_boundary();
        let _permit = self.worker.admit(&operation_id, &request.set_id)?;

        let (committed_journal, receipt) = {
            let mut session = self
                .store
                .begin_mutation(&operation_id, &request.set_id)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            let (_, mut journal) = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?
                .into_parts();

            if let Some(previous) = journal.idempotency_entry(&request.idempotency_token) {
                if previous.set_id == request.set_id
                    && previous.mutation_kind == CredentialMutationKind::Delete
                    && previous.expected_revision == request.expected_revision
                {
                    let receipt = previous.receipt.clone();
                    self.reconcile_authoritative_journal(&journal, &request.set_id)?;
                    return Ok(receipt);
                }
                Self::ensure_target_has_no_pending_mutation(&journal, &request.set_id)?;
                return Err(content_free_error(
                    CredentialErrorCode::Conflict,
                    CredentialSafeRecoveryAction::Retry,
                    Some(request.set_id),
                ));
            }
            Self::ensure_target_has_no_pending_mutation(&journal, &request.set_id)?;
            let committed_epoch = Self::next_committed_epoch(&journal, &request.set_id)?;

            let active = session
                .read_active(&request.set_id)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            let current_revision = match active {
                Some(encoded) => {
                    let record = CredentialRecordEnvelope::decode(&encoded)?;
                    if record.set_id != request.set_id {
                        return Err(content_free_error(
                            CredentialErrorCode::CorruptRecord,
                            CredentialSafeRecoveryAction::Reconcile,
                            Some(request.set_id),
                        ));
                    }
                    Some(record.revision)
                }
                None => None,
            };
            let journal_revision = journal
                .set_state(&request.set_id)
                .and_then(|set| set.revision.clone());
            if journal_revision != current_revision {
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(request.set_id),
                ));
            }
            if request.expected_revision != current_revision {
                return Err(
                    CredentialStoreFailure::RevisionConflict.into_public(Some(request.set_id))
                );
            }
            if current_revision.is_none() {
                return Err(CredentialStoreFailure::Missing.into_public(Some(request.set_id)));
            }

            journal.pending_intents.push(PendingCredentialIntent {
                operation_id: operation_id.clone(),
                idempotency_token: request.idempotency_token.clone(),
                set_id: request.set_id.clone(),
                mutation_kind: CredentialMutationKind::Delete,
                expected_revision: current_revision.clone(),
                proposed_revision: new_revision.clone(),
                recovery_state: CredentialSetRecoveryState::PendingIntent,
            });
            if let Some(set) = journal.set_state_mut(&request.set_id) {
                set.recovery_state = CredentialSetRecoveryState::PendingIntent;
            }
            session
                .persist_intent(&journal)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            self.cache_journal(&journal);
            session
                .replace_active(&request.set_id, encoded)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            let readback = session
                .readback_active(&request.set_id)
                .map_err(|failure| {
                    self.active_readback_is_commit_unknown(failure, &request.set_id)
                })?;
            verify_exact_store_readback(&expected_readback, readback.as_ref())
                .map_err(|failure| failure.into_public(Some(request.set_id.clone())))?;

            let receipt = CredentialMutationReceipt {
                operation_id: operation_id.clone(),
                idempotency_token: request.idempotency_token.clone(),
                set_id: request.set_id.clone(),
                previous_revision: current_revision.clone(),
                new_revision: Some(new_revision.clone()),
                result_code: CredentialMutationResultCode::Tombstoned,
                recovery_action: CredentialSafeRecoveryAction::None,
            };
            journal
                .pending_intents
                .retain(|intent| intent.operation_id != operation_id);
            journal.global_epoch = committed_epoch;
            journal.backend.availability = CredentialBackendAvailability::Available;
            let Some(set) = journal.set_state_mut(&request.set_id) else {
                return Err(content_free_error(
                    CredentialErrorCode::InvalidCredentialSet,
                    CredentialSafeRecoveryAction::ReenterCredential,
                    Some(request.set_id),
                ));
            };
            set.record_state = CredentialSetRecordState::Tombstoned;
            set.source = CredentialSetSource::NativeV2;
            set.revision = Some(new_revision);
            set.recovery_state = CredentialSetRecoveryState::None;
            set.pending_activation = false;
            journal.record_idempotency(IdempotencyJournalEntry {
                idempotency_token: request.idempotency_token,
                set_id: request.set_id,
                mutation_kind: CredentialMutationKind::Delete,
                expected_revision: current_revision,
                receipt: receipt.clone(),
            });
            session.commit_journal(&journal).map_err(|failure| {
                self.observe_store_failure(&failure);
                CredentialStoreFailure::CommitUnknown.into_public(Some(receipt.set_id.clone()))
            })?;
            (journal, receipt)
        };

        let global_epoch = committed_journal.global_epoch;
        *self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = committed_journal;
        self.publish_committed_change(receipt.clone(), global_epoch);
        Ok(receipt)
    }

    pub(crate) fn prepare_settings_activation(
        &self,
        request: PrepareCredentialActivation,
    ) -> Result<PreparedCredentialActivation, CredentialError> {
        Self::require_core_managed_set(&request.set_id)?;
        if request.proposed_settings_revision <= request.expected_settings_revision {
            return Err(content_free_error(
                CredentialErrorCode::Conflict,
                CredentialSafeRecoveryAction::Retry,
                Some(request.set_id),
            ));
        }
        let operation_id = self.token_source.next_operation_id();
        let proposed_revision = self.token_source.next_revision();
        let encoded = CredentialRecordEnvelope::present(
            request.set_id.clone(),
            request.auth_method_id,
            proposed_revision.clone(),
            operation_id.clone(),
            request.material,
        )?
        .encode()?;
        let expected_staging = encoded.copy_for_boundary();
        let _permit = self.worker.admit(&operation_id, &request.set_id)?;

        let (committed_journal, settings_identity, expected_global_epoch, proposed_global_epoch) = {
            let mut session = self
                .store
                .begin_mutation(&operation_id, &request.set_id)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            let (authority_instance_id, mut journal) = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?
                .into_parts();
            if journal.pending_activation.is_some() || !journal.pending_intents.is_empty() {
                return Err(
                    CredentialStoreFailure::OperationInProgress.into_public(Some(request.set_id))
                );
            }
            Self::ensure_target_has_no_pending_mutation(&journal, &request.set_id)?;
            let expected_global_epoch = journal.global_epoch;
            let proposed_global_epoch = Self::next_committed_epoch(&journal, &request.set_id)?;

            let active = session
                .read_active(&request.set_id)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            let current_revision = match active {
                Some(encoded) => {
                    let record = CredentialRecordEnvelope::decode(&encoded)?;
                    if record.set_id != request.set_id {
                        return Err(content_free_error(
                            CredentialErrorCode::CorruptRecord,
                            CredentialSafeRecoveryAction::Reconcile,
                            Some(request.set_id),
                        ));
                    }
                    Some(record.revision)
                }
                None => None,
            };
            if journal
                .set_state(&request.set_id)
                .and_then(|set| set.revision.clone())
                != current_revision
            {
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(request.set_id),
                ));
            }
            if request.expected_revision != current_revision {
                return Err(
                    CredentialStoreFailure::RevisionConflict.into_public(Some(request.set_id))
                );
            }

            journal.pending_intents.push(PendingCredentialIntent {
                operation_id: operation_id.clone(),
                idempotency_token: request.idempotency_token.clone(),
                set_id: request.set_id.clone(),
                mutation_kind: CredentialMutationKind::Activate,
                expected_revision: current_revision,
                proposed_revision: proposed_revision.clone(),
                recovery_state: CredentialSetRecoveryState::PendingIntent,
            });
            journal.pending_activation = Some(PendingSettingsActivation {
                operation_id: operation_id.clone(),
                idempotency_token: request.idempotency_token.clone(),
                set_id: request.set_id.clone(),
                auth_method_id: request.auth_method_id,
                expected_revision: request.expected_revision.clone(),
                proposed_revision: proposed_revision.clone(),
                expected_settings_revision: request.expected_settings_revision,
                proposed_settings_revision: request.proposed_settings_revision,
                expected_global_epoch,
                proposed_global_epoch,
                stage: CredentialActivationStage::Staged,
            });
            if let Some(set) = journal.set_state_mut(&request.set_id) {
                set.pending_activation = true;
                set.recovery_state = CredentialSetRecoveryState::PendingIntent;
            }
            session
                .persist_intent(&journal)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            self.cache_journal(&journal);
            session
                .write_staging(&operation_id, &request.set_id, encoded)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            let readback = session
                .read_staging(&operation_id, &request.set_id)
                .map_err(|failure| {
                    self.observe_store_failure(&failure);
                    CredentialStoreFailure::CommitUnknown.into_public(Some(request.set_id.clone()))
                })?;
            verify_exact_store_readback(&expected_staging, readback.as_ref())
                .map_err(|failure| failure.into_public(Some(request.set_id.clone())))?;
            session
                .commit_journal(&journal)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            let settings_identity = SettingsActivationIdentity {
                operation_id: operation_id.clone(),
                authority_instance_id,
                set_id: request.set_id.clone(),
                expected_revision: request.expected_revision.clone(),
                proposed_revision: proposed_revision.clone(),
                expected_settings_revision: request.expected_settings_revision,
                proposed_settings_revision: request.proposed_settings_revision,
            };
            (
                journal,
                settings_identity,
                expected_global_epoch,
                proposed_global_epoch,
            )
        };

        *self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = committed_journal;
        Ok(PreparedCredentialActivation {
            transaction: SettingsActivationTransaction::new(
                settings_identity,
                request.settings_draft,
            ),
            idempotency_token: request.idempotency_token,
            auth_method_id: request.auth_method_id,
            expected_global_epoch,
            proposed_global_epoch,
        })
    }

    pub(crate) fn commit_settings_activation(
        &self,
        prepared: PreparedCredentialActivation,
    ) -> Result<CredentialMutationReceipt, CredentialError> {
        self.transition_activation_stage(
            &prepared,
            CredentialActivationStage::Staged,
            CredentialActivationStage::SettingsPending,
        )
        .map_err(ActivationStageTransitionError::into_public)?;
        let settings_attempt = {
            let _permit = self
                .worker
                .admit(&prepared.operation_id, &prepared.set_id)?;
            match self
                .settings_activation
                .persist_pending_settings(prepared.transaction())
            {
                Ok(()) => match self
                    .settings_activation
                    .verify_pending_settings(prepared.settings_identity())
                {
                    Ok(()) => Ok(()),
                    Err(failure) => {
                        self.observe_store_failure(&failure);
                        Err(SettingsPendingAttemptFailure::Verify(failure))
                    }
                },
                Err(failure) => {
                    self.observe_store_failure(&failure);
                    Err(SettingsPendingAttemptFailure::Persist(failure))
                }
            }
        };
        match settings_attempt {
            Ok(()) => {}
            Err(SettingsPendingAttemptFailure::Persist(failure)) => {
                let error = failure.into_public(Some(prepared.set_id.clone()));
                if matches!(
                    error.code,
                    CredentialErrorCode::CommitUnknown | CredentialErrorCode::StalledWorker
                ) {
                    let _ = self.mark_activation_recovery_required(&prepared);
                    return Err(error);
                }
                self.rollback_settings_then_abort(&prepared)?;
                return Err(error);
            }
            Err(SettingsPendingAttemptFailure::Verify(failure)) => {
                let error = failure.into_public(Some(prepared.set_id.clone()));
                self.rollback_settings_then_abort(&prepared)?;
                return Err(error);
            }
        }
        if let Err(transition_error) = self.transition_activation_stage(
            &prepared,
            CredentialActivationStage::SettingsPending,
            CredentialActivationStage::CredentialPending,
        ) {
            match transition_error {
                ActivationStageTransitionError::Ineligible(error) => return Err(error),
                ActivationStageTransitionError::Failed(error) => {
                    if matches!(
                        error.code,
                        CredentialErrorCode::CommitUnknown | CredentialErrorCode::StalledWorker
                    ) {
                        let _ = self.mark_activation_recovery_required(&prepared);
                        return Err(error);
                    }
                    self.rollback_settings_then_abort(&prepared)?;
                    return Err(error);
                }
            }
        }
        #[cfg(test)]
        self.run_activation_before_active_test_hook();

        match self.commit_active_activation(&prepared) {
            Ok(receipt) => self.complete_activation_cleanup(&prepared, receipt),
            Err(error) => {
                if error.code == CredentialErrorCode::CommitUnknown {
                    let _ = self.mark_activation_recovery_required(&prepared);
                    return Err(error);
                }
                if error.code == CredentialErrorCode::StalledWorker {
                    let _ = self.mark_activation_recovery_required(&prepared);
                    return Err(error);
                }
                self.rollback_settings_then_abort(&prepared)?;
                Err(error)
            }
        }
    }

    fn rollback_settings_then_abort(
        &self,
        prepared: &impl CredentialActivationContext,
    ) -> Result<(), CredentialError> {
        let recovery_required = || {
            content_free_error(
                CredentialErrorCode::RecoveryRequired,
                CredentialSafeRecoveryAction::Reconcile,
                Some(prepared.set_id.clone()),
            )
        };
        let _permit = self
            .worker
            .admit(&prepared.operation_id, &prepared.set_id)?;
        let mut session = self
            .store
            .begin_mutation(&prepared.operation_id, &prepared.set_id)
            .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
        let (authority_instance_id, mut journal) = session
            .load_journal()
            .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?
            .into_parts();
        Self::require_authoritative_prepared_activation(
            &authority_instance_id,
            &journal,
            prepared,
        )?;
        let Some(pending) = journal.pending_activation.as_ref() else {
            return Err(recovery_required());
        };
        if !matches!(
            pending.stage,
            CredentialActivationStage::SettingsPending
                | CredentialActivationStage::CredentialPending
        ) {
            return Err(content_free_error(
                CredentialErrorCode::Conflict,
                CredentialSafeRecoveryAction::Reconcile,
                Some(prepared.set_id.clone()),
            ));
        }

        let active = session
            .read_active(&prepared.set_id)
            .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
        let active_record = active
            .as_ref()
            .map(CredentialRecordEnvelope::decode)
            .transpose()?;
        if !Self::expected_activation_record_is_authoritative(
            active_record.as_ref(),
            &journal,
            prepared,
        ) {
            return Err(recovery_required());
        }

        let changed = Self::apply_activation_recovery_gate(&mut journal, prepared)?;
        debug_assert!(
            changed,
            "rollback eligibility must claim a new recovery gate"
        );
        session
            .commit_journal(&journal)
            .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
        self.cache_journal(&journal);

        if let Err(failure) = self
            .settings_activation
            .restore_settings_backup(prepared.settings_identity())
        {
            let _ = self.map_store_failure(failure, &prepared.set_id);
            return Err(recovery_required());
        }
        if let Err(failure) = session.delete_staging(&prepared.operation_id, &prepared.set_id) {
            let _ = self.map_store_failure(failure, &prepared.set_id);
            return Err(recovery_required());
        }
        journal.pending_activation = None;
        journal
            .pending_intents
            .retain(|intent| intent.operation_id != prepared.operation_id);
        if let Some(set) = journal.set_state_mut(&prepared.set_id) {
            set.pending_activation = false;
            set.recovery_state = CredentialSetRecoveryState::None;
        }
        if let Err(failure) = session.commit_journal(&journal) {
            let _ = self.map_store_failure(failure, &prepared.set_id);
            return Err(recovery_required());
        }
        self.cache_journal(&journal);
        Ok(())
    }

    fn transition_activation_stage(
        &self,
        prepared: &impl CredentialActivationContext,
        expected_stage: CredentialActivationStage,
        next_stage: CredentialActivationStage,
    ) -> Result<(), ActivationStageTransitionError> {
        if !Self::normal_activation_stage_transition_is_allowed(expected_stage, next_stage) {
            return Err(ActivationStageTransitionError::Ineligible(
                content_free_error(
                    CredentialErrorCode::Conflict,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ),
            ));
        }
        let _permit = self
            .worker
            .admit(&prepared.operation_id, &prepared.set_id)
            .map_err(ActivationStageTransitionError::Failed)?;
        let committed_journal = {
            let mut session = self
                .store
                .begin_mutation(&prepared.operation_id, &prepared.set_id)
                .map_err(|failure| {
                    ActivationStageTransitionError::Failed(
                        self.map_store_failure(failure, &prepared.set_id),
                    )
                })?;
            let (authority_instance_id, mut journal) = session
                .load_journal()
                .map_err(|failure| {
                    ActivationStageTransitionError::Failed(
                        self.map_store_failure(failure, &prepared.set_id),
                    )
                })?
                .into_parts();
            Self::require_authoritative_prepared_activation(
                &authority_instance_id,
                &journal,
                prepared,
            )
            .map_err(ActivationStageTransitionError::Ineligible)?;
            let Some(pending) = journal.pending_activation.as_mut() else {
                return Err(ActivationStageTransitionError::Ineligible(
                    content_free_error(
                        CredentialErrorCode::RecoveryRequired,
                        CredentialSafeRecoveryAction::Reconcile,
                        Some(prepared.set_id.clone()),
                    ),
                ));
            };
            if pending.operation_id != prepared.operation_id || pending.stage != expected_stage {
                return Err(ActivationStageTransitionError::Ineligible(
                    content_free_error(
                        CredentialErrorCode::Conflict,
                        CredentialSafeRecoveryAction::Reconcile,
                        Some(prepared.set_id.clone()),
                    ),
                ));
            }
            pending.stage = next_stage;
            if let Some(set) = journal.set_state_mut(&prepared.set_id) {
                set.pending_activation = true;
                set.recovery_state = CredentialSetRecoveryState::PendingIntent;
            }
            session.commit_journal(&journal).map_err(|failure| {
                ActivationStageTransitionError::Failed(
                    self.map_store_failure(failure, &prepared.set_id),
                )
            })?;
            journal
        };
        *self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = committed_journal;
        Ok(())
    }

    fn commit_active_activation(
        &self,
        prepared: &impl CredentialActivationContext,
    ) -> Result<CredentialMutationReceipt, CredentialError> {
        let _permit = self
            .worker
            .admit(&prepared.operation_id, &prepared.set_id)?;
        let (committed_journal, receipt) = {
            let mut session = self
                .store
                .begin_mutation(&prepared.operation_id, &prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let (authority_instance_id, mut journal) = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?
                .into_parts();
            Self::require_authoritative_prepared_activation(
                &authority_instance_id,
                &journal,
                prepared,
            )?;
            if journal.pending_activation.as_ref().is_none_or(|pending| {
                pending.operation_id != prepared.operation_id
                    || pending.stage != CredentialActivationStage::CredentialPending
            }) {
                return Err(content_free_error(
                    CredentialErrorCode::Conflict,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            let active = session
                .read_active(&prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let active_revision = active
                .as_ref()
                .map(CredentialRecordEnvelope::decode)
                .transpose()?
                .map(|record| record.revision);
            if active_revision != prepared.expected_revision
                || journal
                    .set_state(&prepared.set_id)
                    .and_then(|set| set.revision.clone())
                    != active_revision
            {
                return Err(CredentialStoreFailure::RevisionConflict
                    .into_public(Some(prepared.set_id.clone())));
            }
            let staged = session
                .read_staging(&prepared.operation_id, &prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?
                .ok_or_else(|| {
                    CredentialStoreFailure::Missing.into_public(Some(prepared.set_id.clone()))
                })?;
            let staged_record = CredentialRecordEnvelope::decode(&staged)?;
            if !Self::proposed_activation_record_matches(&staged_record, prepared) {
                return Err(content_free_error(
                    CredentialErrorCode::CorruptRecord,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            let expected_readback = staged.copy_for_boundary();
            session
                .replace_active(&prepared.set_id, staged)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let readback = session
                .readback_active(&prepared.set_id)
                .map_err(|failure| {
                    self.active_readback_is_commit_unknown(failure, &prepared.set_id)
                })?;
            verify_exact_store_readback(&expected_readback, readback.as_ref())
                .map_err(|failure| failure.into_public(Some(prepared.set_id.clone())))?;
            let receipt = CredentialMutationReceipt {
                operation_id: prepared.operation_id.clone(),
                idempotency_token: prepared.idempotency_token().clone(),
                set_id: prepared.set_id.clone(),
                previous_revision: active_revision.clone(),
                new_revision: Some(prepared.proposed_revision.clone()),
                result_code: if active_revision.is_some() {
                    CredentialMutationResultCode::Replaced
                } else {
                    CredentialMutationResultCode::Created
                },
                recovery_action: CredentialSafeRecoveryAction::None,
            };
            let Some(pending) = journal.pending_activation.as_mut() else {
                unreachable!("pending checked above")
            };
            pending.stage = CredentialActivationStage::CleanupPending;
            journal.backend.availability = CredentialBackendAvailability::Available;
            let Some(set) = journal.set_state_mut(&prepared.set_id) else {
                return Err(content_free_error(
                    CredentialErrorCode::InvalidCredentialSet,
                    CredentialSafeRecoveryAction::ReenterCredential,
                    Some(prepared.set_id.clone()),
                ));
            };
            set.record_state = CredentialSetRecordState::Configured;
            set.source = CredentialSetSource::NativeV2;
            set.revision = Some(prepared.proposed_revision.clone());
            set.recovery_state = CredentialSetRecoveryState::PendingIntent;
            set.pending_activation = true;
            session.commit_journal(&journal).map_err(|failure| {
                self.observe_store_failure(&failure);
                CredentialStoreFailure::CommitUnknown.into_public(Some(prepared.set_id.clone()))
            })?;
            (journal, receipt)
        };
        *self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = committed_journal;
        Ok(receipt)
    }

    fn complete_activation_cleanup(
        &self,
        prepared: &impl CredentialActivationContext,
        receipt: CredentialMutationReceipt,
    ) -> Result<CredentialMutationReceipt, CredentialError> {
        let _permit = self
            .worker
            .admit(&prepared.operation_id, &prepared.set_id)?;
        let committed_journal = {
            let mut session = self
                .store
                .begin_mutation(&prepared.operation_id, &prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let (authority_instance_id, mut journal) = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?
                .into_parts();
            Self::require_authoritative_prepared_activation(
                &authority_instance_id,
                &journal,
                prepared,
            )?;
            if journal.pending_activation.as_ref().is_none_or(|pending| {
                pending.operation_id != prepared.operation_id
                    || pending.stage != CredentialActivationStage::CleanupPending
            }) {
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            let active = session
                .read_active(&prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?
                .ok_or_else(|| {
                    content_free_error(
                        CredentialErrorCode::RecoveryRequired,
                        CredentialSafeRecoveryAction::Reconcile,
                        Some(prepared.set_id.clone()),
                    )
                })?;
            let active_record = CredentialRecordEnvelope::decode(&active)?;
            if !Self::proposed_activation_record_matches(&active_record, prepared) {
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            if let Err(failure) = self
                .settings_activation
                .verify_committed_settings(prepared.settings_identity())
            {
                self.observe_store_failure(&failure);
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            if let Err(failure) = self
                .settings_activation
                .clear_pending_settings(prepared.settings_identity())
            {
                self.observe_store_failure(&failure);
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            session
                .delete_staging(&prepared.operation_id, &prepared.set_id)
                .map_err(|failure| {
                    self.observe_store_failure(&failure);
                    content_free_error(
                        CredentialErrorCode::RecoveryRequired,
                        CredentialSafeRecoveryAction::Reconcile,
                        Some(prepared.set_id.clone()),
                    )
                })?;
            journal.pending_activation = None;
            journal
                .pending_intents
                .retain(|intent| intent.operation_id != prepared.operation_id);
            if let Some(set) = journal.set_state_mut(&prepared.set_id) {
                set.recovery_state = CredentialSetRecoveryState::None;
                set.pending_activation = false;
            }
            journal.record_idempotency(IdempotencyJournalEntry {
                idempotency_token: prepared.idempotency_token().clone(),
                set_id: prepared.set_id.clone(),
                mutation_kind: CredentialMutationKind::Activate,
                expected_revision: prepared.expected_revision.clone(),
                receipt: receipt.clone(),
            });
            journal.global_epoch = prepared.proposed_global_epoch();
            session.commit_journal(&journal).map_err(|failure| {
                self.observe_store_failure(&failure);
                content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                )
            })?;
            journal
        };
        let global_epoch = committed_journal.global_epoch;
        *self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = committed_journal;
        self.publish_committed_change(receipt.clone(), global_epoch);
        Ok(receipt)
    }

    fn apply_activation_recovery_gate(
        journal: &mut AuthorityJournal,
        prepared: &impl CredentialActivationContext,
    ) -> Result<bool, CredentialError> {
        let Some(pending) = journal.pending_activation.as_mut() else {
            return Err(content_free_error(
                CredentialErrorCode::RecoveryRequired,
                CredentialSafeRecoveryAction::Reconcile,
                Some(prepared.set_id.clone()),
            ));
        };
        if !Self::pending_activation_matches_prepared(pending, prepared) {
            return Err(content_free_error(
                CredentialErrorCode::Conflict,
                CredentialSafeRecoveryAction::Reconcile,
                Some(prepared.set_id.clone()),
            ));
        }
        if matches!(
            pending.stage,
            CredentialActivationStage::CleanupPending | CredentialActivationStage::RecoveryRequired
        ) {
            return Ok(false);
        }
        pending.stage = CredentialActivationStage::RecoveryRequired;
        if let Some(set) = journal.set_state_mut(&prepared.set_id) {
            set.recovery_state = CredentialSetRecoveryState::CommitUnknown;
            set.pending_activation = true;
        }
        Ok(true)
    }

    fn mark_activation_recovery_required(
        &self,
        prepared: &impl CredentialActivationContext,
    ) -> Result<(), CredentialError> {
        let _permit = self
            .worker
            .admit(&prepared.operation_id, &prepared.set_id)?;
        let recovery_journal = {
            let mut session = self
                .store
                .begin_mutation(&prepared.operation_id, &prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let (authority_instance_id, mut journal) = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?
                .into_parts();
            Self::require_authoritative_prepared_activation(
                &authority_instance_id,
                &journal,
                prepared,
            )?;
            if Self::apply_activation_recovery_gate(&mut journal, prepared)? {
                session
                    .commit_journal(&journal)
                    .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            }
            journal
        };
        *self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = recovery_journal;
        Ok(())
    }

    pub(crate) fn recover_settings_activation(
        &self,
        operation_id: &CredentialOperationId,
    ) -> Result<CredentialMutationReceipt, CredentialError> {
        let set_id_hint = {
            let journal = self
                .journal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(completed) = journal.idempotency_history.iter().find(|entry| {
                entry.mutation_kind == CredentialMutationKind::Activate
                    && entry.receipt.operation_id == *operation_id
            }) {
                return Ok(completed.receipt.clone());
            }
            journal
                .pending_activation
                .as_ref()
                .map(|pending| pending.set_id.clone())
                .ok_or_else(|| {
                    content_free_error(
                        CredentialErrorCode::Missing,
                        CredentialSafeRecoveryAction::Reconcile,
                        None,
                    )
                })?
        };

        let (authoritative_authority, authoritative_journal, completed_receipt) = {
            let _permit = self.worker.admit(operation_id, &set_id_hint)?;
            let mut session = self
                .store
                .begin_mutation(operation_id, &set_id_hint)
                .map_err(|failure| self.map_store_failure(failure, &set_id_hint))?;
            let (authority_instance_id, journal) = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &set_id_hint))?
                .into_parts();
            let completed = journal
                .idempotency_history
                .iter()
                .find(|entry| {
                    entry.mutation_kind == CredentialMutationKind::Activate
                        && entry.receipt.operation_id == *operation_id
                })
                .map(|entry| (entry.set_id.clone(), entry.receipt.clone()));
            if completed.as_ref().is_some_and(|(set_id, receipt)| {
                *set_id != set_id_hint || receipt.set_id != set_id_hint
            }) {
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(set_id_hint),
                ));
            }
            self.reconcile_authoritative_journal(&journal, &set_id_hint)?;
            (
                authority_instance_id,
                journal,
                completed.map(|(_, receipt)| receipt),
            )
        };
        if let Some(receipt) = completed_receipt {
            return Ok(receipt);
        }

        let pending = authoritative_journal
            .pending_activation
            .clone()
            .ok_or_else(|| {
                content_free_error(
                    CredentialErrorCode::Missing,
                    CredentialSafeRecoveryAction::Reconcile,
                    None,
                )
            })?;
        if pending.operation_id != *operation_id {
            return Err(content_free_error(
                CredentialErrorCode::Conflict,
                CredentialSafeRecoveryAction::Reconcile,
                Some(pending.set_id),
            ));
        }
        let pending_stage = pending.stage;
        let prepared = RestartedCredentialActivation {
            identity: SettingsActivationIdentity {
                operation_id: pending.operation_id,
                authority_instance_id: authoritative_authority,
                set_id: pending.set_id,
                expected_revision: pending.expected_revision,
                proposed_revision: pending.proposed_revision,
                expected_settings_revision: pending.expected_settings_revision,
                proposed_settings_revision: pending.proposed_settings_revision,
            },
            idempotency_token: pending.idempotency_token,
            auth_method_id: pending.auth_method_id,
            expected_global_epoch: pending.expected_global_epoch,
            proposed_global_epoch: pending.proposed_global_epoch,
        };
        {
            let journal = self
                .journal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            Self::require_authoritative_prepared_activation(
                &prepared.authority_instance_id,
                &journal,
                &prepared,
            )?;
        }

        if pending_stage == CredentialActivationStage::SettingsPending {
            let verification = {
                let _permit = self
                    .worker
                    .admit(&prepared.operation_id, &prepared.set_id)?;
                let result = self
                    .settings_activation
                    .verify_pending_settings(prepared.settings_identity());
                if let Err(failure) = &result {
                    self.observe_store_failure(failure);
                }
                result
            };
            if verification.is_err() {
                let _ = self.mark_activation_recovery_required(&prepared);
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            self.transition_activation_stage(
                &prepared,
                CredentialActivationStage::SettingsPending,
                CredentialActivationStage::CredentialPending,
            )
            .map_err(ActivationStageTransitionError::into_public)?;
        }
        if pending_stage == CredentialActivationStage::CleanupPending {
            let receipt = CredentialMutationReceipt {
                operation_id: prepared.operation_id.clone(),
                idempotency_token: prepared.idempotency_token().clone(),
                set_id: prepared.set_id.clone(),
                previous_revision: prepared.expected_revision.clone(),
                new_revision: Some(prepared.proposed_revision.clone()),
                result_code: CredentialMutationResultCode::Recovered,
                recovery_action: CredentialSafeRecoveryAction::None,
            };
            return self.complete_activation_cleanup(&prepared, receipt);
        }

        let recovery = {
            let _permit = self
                .worker
                .admit(&prepared.operation_id, &prepared.set_id)?;
            let mut session = self
                .store
                .begin_mutation(&prepared.operation_id, &prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let (authority_instance_id, mut journal) = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?
                .into_parts();
            Self::require_authoritative_prepared_activation(
                &authority_instance_id,
                &journal,
                &prepared,
            )?;
            if journal
                .pending_activation
                .as_ref()
                .is_none_or(|candidate| candidate.operation_id != prepared.operation_id)
            {
                return Err(content_free_error(
                    CredentialErrorCode::Conflict,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            let active = session
                .read_active(&prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let active_record = active
                .as_ref()
                .map(CredentialRecordEnvelope::decode)
                .transpose()?;
            let proposed_active_is_authoritative = active_record
                .as_ref()
                .is_some_and(|record| Self::proposed_activation_record_matches(record, &prepared));
            let expected_active_is_authoritative =
                Self::expected_activation_record_is_authoritative(
                    active_record.as_ref(),
                    &journal,
                    &prepared,
                );

            if proposed_active_is_authoritative {
                self.settings_activation
                    .verify_pending_settings(prepared.settings_identity())
                    .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
                let receipt = CredentialMutationReceipt {
                    operation_id: prepared.operation_id.clone(),
                    idempotency_token: prepared.idempotency_token().clone(),
                    set_id: prepared.set_id.clone(),
                    previous_revision: prepared.expected_revision.clone(),
                    new_revision: Some(prepared.proposed_revision.clone()),
                    result_code: CredentialMutationResultCode::Recovered,
                    recovery_action: CredentialSafeRecoveryAction::None,
                };
                journal.backend.availability = CredentialBackendAvailability::Available;
                let Some(pending) = journal.pending_activation.as_mut() else {
                    unreachable!("pending checked above")
                };
                pending.stage = CredentialActivationStage::CleanupPending;
                let Some(set) = journal.set_state_mut(&prepared.set_id) else {
                    return Err(content_free_error(
                        CredentialErrorCode::InvalidCredentialSet,
                        CredentialSafeRecoveryAction::ReenterCredential,
                        Some(prepared.set_id.clone()),
                    ));
                };
                set.record_state = CredentialSetRecordState::Configured;
                set.source = CredentialSetSource::NativeV2;
                set.revision = Some(prepared.proposed_revision.clone());
                set.recovery_state = CredentialSetRecoveryState::PendingIntent;
                set.pending_activation = true;
                session
                    .commit_journal(&journal)
                    .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
                Ok((journal, receipt, true))
            } else if expected_active_is_authoritative {
                self.settings_activation
                    .restore_settings_backup(prepared.settings_identity())
                    .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
                let receipt = CredentialMutationReceipt {
                    operation_id: prepared.operation_id.clone(),
                    idempotency_token: prepared.idempotency_token().clone(),
                    set_id: prepared.set_id.clone(),
                    previous_revision: prepared.expected_revision.clone(),
                    new_revision: prepared.expected_revision.clone(),
                    result_code: CredentialMutationResultCode::NoChange,
                    recovery_action: CredentialSafeRecoveryAction::None,
                };
                journal.pending_activation = None;
                journal
                    .pending_intents
                    .retain(|intent| intent.operation_id != prepared.operation_id);
                if let Some(set) = journal.set_state_mut(&prepared.set_id) {
                    set.pending_activation = false;
                    set.recovery_state = CredentialSetRecoveryState::None;
                }
                journal.record_idempotency(IdempotencyJournalEntry {
                    idempotency_token: prepared.idempotency_token().clone(),
                    set_id: prepared.set_id.clone(),
                    mutation_kind: CredentialMutationKind::Activate,
                    expected_revision: prepared.expected_revision.clone(),
                    receipt: receipt.clone(),
                });
                session
                    .delete_staging(&prepared.operation_id, &prepared.set_id)
                    .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
                session
                    .commit_journal(&journal)
                    .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
                Ok((journal, receipt, false))
            } else {
                Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ))
            }
        }?;
        let (recovered_journal, receipt, publish) = recovery;
        let global_epoch = recovered_journal.global_epoch;
        *self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = recovered_journal;
        if publish {
            debug_assert_eq!(global_epoch, self.snapshot_status().global_epoch);
            return self.complete_activation_cleanup(&prepared, receipt);
        }
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CredentialEntryStore, CredentialMutationSession, CredentialResolution, CredentialService,
        CredentialSettingsActivationPort, CredentialUseRequest, DeleteCredentialSet,
        PrepareCredentialActivation, ReplaceCredentialSet, SettingsActivationIdentity,
        SettingsActivationTransaction, verify_exact_store_readback,
    };
    use crate::credentials::domain::{
        AuthorityJournal, CredentialAuthorityInstanceId, CredentialRecordEnvelope,
        CredentialStoreFailure, EncodedCredentialRecord, LoadedAuthorityJournal,
        StoredSecretBundle, ValidatedNonSecretSettingsDraft,
    };
    use crate::credentials::fake::{
        FakeCredentialStore, FakeSettingsActivationPort, FakeSettingsCall, FakeStoreCall,
    };
    use crate::credentials::test_support::DeterministicTokenSource;
    use audio_graph_ipc_contract::credential_contract::{
        AuthMethodId, AwsPartition, AwsSdkService, BuiltInCredentialSetId, CREDENTIAL_USE_POLICIES,
        CredentialActivationStage, CredentialAudience, CredentialAudiencePolicyDefinition,
        CredentialBackendKind, CredentialErrorCode, CredentialIdempotencyToken,
        CredentialMutationResultCode, CredentialOperationId, CredentialPurpose,
        CredentialSafeRecoveryAction, CredentialSetId, CredentialSetRecordState,
        CredentialSetRecoveryState, CredentialUsePolicyDecisionDefinition, CredentialWorkerState,
        CustomCredentialSetId, PORTABLE_ENCODED_RECORD_MAX_BYTES, SecureTransportScheme,
    };
    use std::sync::{Arc, Barrier, Mutex, TryLockError, Weak, mpsc};
    use std::time::Duration;

    fn idempotency(value: &str) -> CredentialIdempotencyToken {
        CredentialIdempotencyToken::parse(value).expect("canonical idempotency token")
    }

    fn settings_draft() -> ValidatedNonSecretSettingsDraft {
        ValidatedNonSecretSettingsDraft::from_validated_bytes(
            b"validated-non-secret-settings-draft".to_vec(),
        )
    }

    struct WorkerInspectingSettingsPort {
        inner: FakeSettingsActivationPort,
        service: Mutex<Option<Weak<CredentialService>>>,
        expected: Mutex<Option<(CredentialOperationId, CredentialSetId)>>,
    }

    impl WorkerInspectingSettingsPort {
        fn new(current_revision: u64) -> Self {
            Self {
                inner: FakeSettingsActivationPort::new(current_revision),
                service: Mutex::new(None),
                expected: Mutex::new(None),
            }
        }

        fn arm(
            &self,
            service: &Arc<CredentialService>,
            operation_id: CredentialOperationId,
            set_id: CredentialSetId,
        ) {
            *self
                .service
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::downgrade(service));
            *self
                .expected
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some((operation_id, set_id));
        }

        fn persist_without_inspection(
            &self,
            transaction: &SettingsActivationTransaction,
        ) -> Result<(), CredentialStoreFailure> {
            self.inner.persist_pending_settings(transaction)
        }

        fn assert_authoritative_worker_permit(&self) {
            let service = self
                .service
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_ref()
                .and_then(Weak::upgrade)
                .expect("settings port is armed with the live service");
            let (expected_operation, expected_set) = self
                .expected
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
                .expect("settings port is armed with one operation identity");
            let worker = service.snapshot_status().worker;
            assert_eq!(worker.state, CredentialWorkerState::Busy);
            assert_eq!(worker.operation_id.as_ref(), Some(&expected_operation));
            assert_eq!(worker.set_id.as_ref(), Some(&expected_set));
            assert!(matches!(
                service.worker.serial.try_lock(),
                Err(TryLockError::WouldBlock)
            ));
        }
    }

    impl CredentialSettingsActivationPort for WorkerInspectingSettingsPort {
        fn persist_pending_settings(
            &self,
            transaction: &SettingsActivationTransaction,
        ) -> Result<(), CredentialStoreFailure> {
            self.assert_authoritative_worker_permit();
            self.inner.persist_pending_settings(transaction)
        }

        fn verify_pending_settings(
            &self,
            identity: &SettingsActivationIdentity,
        ) -> Result<(), CredentialStoreFailure> {
            self.assert_authoritative_worker_permit();
            self.inner.verify_pending_settings(identity)
        }

        fn verify_committed_settings(
            &self,
            identity: &SettingsActivationIdentity,
        ) -> Result<(), CredentialStoreFailure> {
            self.inner.verify_committed_settings(identity)
        }

        fn restore_settings_backup(
            &self,
            identity: &SettingsActivationIdentity,
        ) -> Result<(), CredentialStoreFailure> {
            self.inner.restore_settings_backup(identity)
        }

        fn clear_pending_settings(
            &self,
            identity: &SettingsActivationIdentity,
        ) -> Result<(), CredentialStoreFailure> {
            self.inner.clear_pending_settings(identity)
        }
    }

    struct AfterEffectCommitUnknownStore {
        inner: FakeCredentialStore,
        successful_commits_before_failure: Mutex<Option<usize>>,
    }

    impl AfterEffectCommitUnknownStore {
        fn new(journal: AuthorityJournal) -> Self {
            Self {
                inner: FakeCredentialStore::new(journal),
                successful_commits_before_failure: Mutex::new(None),
            }
        }

        fn fail_commit_after_effect_after(&self, successful_commits: usize) {
            *self
                .successful_commits_before_failure
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(successful_commits);
        }

        fn journal_snapshot(&self) -> AuthorityJournal {
            self.inner.journal_snapshot()
        }

        fn active_write_count(&self) -> usize {
            self.inner.active_write_count()
        }

        fn staging_count(&self) -> usize {
            self.inner.staging_count()
        }

        fn calls(&self) -> Vec<FakeStoreCall> {
            self.inner.calls()
        }
    }

    impl CredentialEntryStore for AfterEffectCommitUnknownStore {
        fn read_active(
            &self,
            set_id: &CredentialSetId,
        ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
            self.inner.read_active(set_id)
        }

        fn begin_mutation(
            &self,
            operation_id: &CredentialOperationId,
            set_id: &CredentialSetId,
        ) -> Result<Box<dyn CredentialMutationSession + '_>, CredentialStoreFailure> {
            Ok(Box::new(AfterEffectCommitUnknownSession {
                inner: self.inner.begin_mutation(operation_id, set_id)?,
                successful_commits_before_failure: &self.successful_commits_before_failure,
            }))
        }
    }

    struct AfterEffectCommitUnknownSession<'a> {
        inner: Box<dyn CredentialMutationSession + 'a>,
        successful_commits_before_failure: &'a Mutex<Option<usize>>,
    }

    impl CredentialMutationSession for AfterEffectCommitUnknownSession<'_> {
        fn load_journal(&mut self) -> Result<LoadedAuthorityJournal, CredentialStoreFailure> {
            self.inner.load_journal()
        }

        fn read_active(
            &mut self,
            set_id: &CredentialSetId,
        ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
            self.inner.read_active(set_id)
        }

        fn persist_intent(
            &mut self,
            journal: &AuthorityJournal,
        ) -> Result<(), CredentialStoreFailure> {
            self.inner.persist_intent(journal)
        }

        fn replace_active(
            &mut self,
            set_id: &CredentialSetId,
            record: EncodedCredentialRecord,
        ) -> Result<(), CredentialStoreFailure> {
            self.inner.replace_active(set_id, record)
        }

        fn readback_active(
            &mut self,
            set_id: &CredentialSetId,
        ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
            self.inner.readback_active(set_id)
        }

        fn write_staging(
            &mut self,
            operation_id: &CredentialOperationId,
            set_id: &CredentialSetId,
            record: EncodedCredentialRecord,
        ) -> Result<(), CredentialStoreFailure> {
            self.inner.write_staging(operation_id, set_id, record)
        }

        fn read_staging(
            &mut self,
            operation_id: &CredentialOperationId,
            set_id: &CredentialSetId,
        ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
            self.inner.read_staging(operation_id, set_id)
        }

        fn delete_staging(
            &mut self,
            operation_id: &CredentialOperationId,
            set_id: &CredentialSetId,
        ) -> Result<(), CredentialStoreFailure> {
            self.inner.delete_staging(operation_id, set_id)
        }

        fn commit_journal(
            &mut self,
            journal: &AuthorityJournal,
        ) -> Result<(), CredentialStoreFailure> {
            self.inner.commit_journal(journal)?;
            let mut remaining = self
                .successful_commits_before_failure
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(successful_commits) = remaining.as_mut() else {
                return Ok(());
            };
            if *successful_commits == 0 {
                *remaining = None;
                return Err(CredentialStoreFailure::CommitUnknown);
            }
            *successful_commits -= 1;
            Ok(())
        }
    }

    fn deepgram_use_request() -> CredentialUseRequest {
        CredentialUseRequest {
            set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
            consumer_id: "asr.deepgram",
            auth_method_id: AuthMethodId::ApiKey,
            purpose: CredentialPurpose::Asr,
            audience: CredentialAudience::SecureNetworkOrigin {
                scheme: SecureTransportScheme::Wss,
                canonical_host: "api.deepgram.com".to_string(),
                effective_port: 443,
            },
        }
    }

    fn assert_stalled_worker_rejects_followup_without_store_io(
        service: &CredentialService,
        store: &FakeCredentialStore,
        expected_set_id: &CredentialSetId,
        expected_operation_id: Option<&CredentialOperationId>,
        idempotency_token: &str,
    ) {
        let stalled = service.snapshot_status().worker;
        assert_eq!(stalled.state, CredentialWorkerState::Stalled);
        assert_eq!(stalled.set_id.as_ref(), Some(expected_set_id));
        let stalled_operation = stalled
            .operation_id
            .expect("a stalled worker remains bound to its operation");
        if let Some(expected_operation_id) = expected_operation_id {
            assert_eq!(&stalled_operation, expected_operation_id);
        }
        let calls_before_followup = store.calls().len();
        let error = service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Openai),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("admission-must-reject").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency(idempotency_token),
            })
            .expect_err("stalled admission rejects a follow-up mutation");

        assert_eq!(error.code, CredentialErrorCode::StalledWorker);
        assert_eq!(store.calls().len(), calls_before_followup);

        let unrelated_operation =
            CredentialOperationId::parse("ffffffff-eeee-4ddd-8ccc-bbbbbbbbbbbb")
                .expect("unrelated operation id");
        assert!(!service.complete_stalled_operation(&unrelated_operation));
        assert_eq!(
            service.snapshot_status().worker.state,
            CredentialWorkerState::Stalled
        );
        assert!(service.complete_stalled_operation(&stalled_operation));
        assert_eq!(
            service.snapshot_status().worker.state,
            CredentialWorkerState::Idle
        );
        assert_eq!(store.calls().len(), calls_before_followup);
    }

    #[test]
    fn passive_status_is_a_stable_zero_io_journal_projection() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        let first = service.snapshot_status();
        let second = service.snapshot_status();

        assert_eq!(first, second);
        assert_eq!(first.global_epoch, 0);
        assert_eq!(first.backend.kind, CredentialBackendKind::InMemory);
        assert_eq!(first.worker.state, CredentialWorkerState::Idle);
        assert_eq!(store.entry_read_count(), 0);
    }

    #[test]
    fn rejected_audience_never_reaches_the_entry_store() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        let result = service.resolve_for_use(CredentialUseRequest {
            set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
            consumer_id: "asr.deepgram",
            auth_method_id: AuthMethodId::ApiKey,
            purpose: CredentialPurpose::Asr,
            audience: CredentialAudience::SecureNetworkOrigin {
                scheme: SecureTransportScheme::Wss,
                canonical_host: "attacker.example".to_string(),
                effective_port: 443,
            },
        });

        assert_eq!(
            result.expect_err("untrusted audience must fail").code,
            CredentialErrorCode::AudienceNotAllowed
        );
        assert_eq!(store.entry_read_count(), 0);
    }

    #[test]
    fn non_store_auth_method_returns_explicit_authority_without_an_entry_read() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        let resolution = service
            .resolve_for_use(CredentialUseRequest {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Aws),
                consumer_id: "llm.aws_bedrock",
                auth_method_id: AuthMethodId::AwsDefaultChain,
                purpose: CredentialPurpose::Llm,
                audience: CredentialAudience::AwsSdk {
                    partition: AwsPartition::Aws,
                    service: AwsSdkService::BedrockRuntime,
                    region: "us-west-2".to_string(),
                },
            })
            .expect("ambient AWS chain is explicit non-store authority");

        assert!(matches!(resolution, CredentialResolution::NonStore(_)));
        assert_eq!(store.entry_read_count(), 0);
    }

    #[test]
    fn every_declared_use_policy_is_decided_before_entry_bytes_are_requested() {
        for policy in CREDENTIAL_USE_POLICIES {
            let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
            let store = Arc::new(FakeCredentialStore::new(journal.clone()));
            let service = CredentialService::new(
                store.clone(),
                journal,
                Arc::new(DeterministicTokenSource::default()),
            );
            let audience = match policy.decision {
                CredentialUsePolicyDecisionDefinition::Authorized { audience } => match audience {
                    CredentialAudiencePolicyDefinition::ExactSecureOrigin { origin } => {
                        let (scheme, authority) =
                            if let Some(authority) = origin.strip_prefix("https://") {
                                (SecureTransportScheme::Https, authority)
                            } else if let Some(authority) = origin.strip_prefix("wss://") {
                                (SecureTransportScheme::Wss, authority)
                            } else {
                                panic!("contract origin must be secure: {origin}")
                            };
                        let (canonical_host, effective_port) = authority
                            .rsplit_once(':')
                            .and_then(|(host, port)| {
                                port.parse::<u16>().ok().map(|port| (host, port))
                            })
                            .map_or_else(
                                || (authority.to_string(), 443),
                                |(host, port)| (host.to_string(), port),
                            );
                        CredentialAudience::SecureNetworkOrigin {
                            scheme,
                            canonical_host,
                            effective_port,
                        }
                    }
                    CredentialAudiencePolicyDefinition::BackendDerivedVertexOrigin {
                        scheme,
                        host_suffix,
                        effective_port,
                    } => CredentialAudience::SecureNetworkOrigin {
                        scheme,
                        canonical_host: format!("unit-test-{host_suffix}"),
                        effective_port,
                    },
                    CredentialAudiencePolicyDefinition::AwsSdk { partition, service } => {
                        CredentialAudience::AwsSdk {
                            partition,
                            service,
                            region: "us-west-2".to_string(),
                        }
                    }
                },
                CredentialUsePolicyDecisionDefinition::Disabled { .. } => {
                    CredentialAudience::SecureNetworkOrigin {
                        scheme: SecureTransportScheme::Https,
                        canonical_host: "disabled.invalid".to_string(),
                        effective_port: 443,
                    }
                }
            };

            let result = service.resolve_for_use(CredentialUseRequest {
                set_id: CredentialSetId::BuiltIn(policy.set_id),
                consumer_id: policy.consumer_id,
                auth_method_id: policy.auth_method_id,
                purpose: policy.purpose,
                audience,
            });
            match policy.decision {
                CredentialUsePolicyDecisionDefinition::Disabled { .. } => {
                    assert_eq!(
                        result.expect_err("disabled policy").code,
                        CredentialErrorCode::AudienceNotAllowed
                    );
                    assert_eq!(store.entry_read_count(), 0);
                }
                CredentialUsePolicyDecisionDefinition::Authorized { .. }
                    if matches!(
                        policy.auth_method_id,
                        AuthMethodId::GoogleServiceAccountFile
                            | AuthMethodId::AwsProfile
                            | AuthMethodId::AwsDefaultChain
                    ) =>
                {
                    assert!(matches!(result, Ok(CredentialResolution::NonStore(_))));
                    assert_eq!(store.entry_read_count(), 0);
                }
                CredentialUsePolicyDecisionDefinition::Authorized { .. } => {
                    assert_eq!(
                        result.expect_err("authorized empty store").code,
                        CredentialErrorCode::Missing
                    );
                    assert_eq!(store.entry_read_count(), 1);
                }
            }
        }
    }

    #[test]
    fn oversized_material_is_rejected_before_store_or_worker_entry() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        let error = service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key(
                    "x".repeat(PORTABLE_ENCODED_RECORD_MAX_BYTES + 1),
                )
                .expect("non-empty API key"),
                expected_revision: None,
                idempotency_token: idempotency("12121212-3434-5656-7878-909090909090"),
            })
            .expect_err("portable record ceiling");

        assert_eq!(error.code, CredentialErrorCode::PayloadTooLarge);
        assert!(store.calls().is_empty());
        assert_eq!(
            service.snapshot_status().worker.state,
            CredentialWorkerState::Idle
        );
    }

    #[test]
    fn custom_set_mutations_are_rejected_without_store_or_worker_entry() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let custom = CredentialSetId::Custom(
            CustomCredentialSetId::parse("custom.11111111-2222-3333-4444-555555555555")
                .expect("canonical backend-issued custom id"),
        );

        let replace_error = service
            .replace_set(ReplaceCredentialSet {
                set_id: custom.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("custom-replace").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("23232323-4545-6767-8989-101010101010"),
            })
            .expect_err("custom replace belongs to the custom-origin workstream");
        let delete_error = service
            .delete_set(DeleteCredentialSet {
                set_id: custom.clone(),
                expected_revision: None,
                idempotency_token: idempotency("34343434-5656-7878-9090-121212121212"),
            })
            .expect_err("custom delete belongs to the custom-origin workstream");
        let activation_error = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: custom,
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("custom-activation").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 1,
                proposed_settings_revision: 2,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("45454545-6767-8989-1010-232323232323"),
            })
            .expect_err("custom activation belongs to the custom-origin workstream");

        assert_eq!(
            replace_error.code,
            CredentialErrorCode::InvalidCredentialSet
        );
        assert_eq!(delete_error.code, CredentialErrorCode::InvalidCredentialSet);
        assert_eq!(
            activation_error.code,
            CredentialErrorCode::InvalidCredentialSet
        );
        assert!(store.calls().is_empty());
        assert_eq!(store.active_write_count(), 0);
        assert_eq!(store.staging_count(), 0);
        assert_eq!(
            service.snapshot_status().worker.state,
            CredentialWorkerState::Idle
        );
    }

    #[test]
    fn replace_creates_one_committed_revision_and_advances_status_epoch() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);

        let receipt = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("new-generation").expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "99999999-9999-9999-9999-999999999999",
                )
                .expect("canonical idempotency token"),
            })
            .expect("create succeeds");
        let status = service.snapshot_status();
        let set_status = status
            .sets
            .iter()
            .find(|set| set.set_id == set_id)
            .expect("Deepgram status");

        assert_eq!(receipt.result_code, CredentialMutationResultCode::Created);
        assert_eq!(receipt.previous_revision, None);
        assert_eq!(receipt.new_revision, set_status.revision);
        assert_eq!(
            set_status.record_state,
            CredentialSetRecordState::Configured
        );
        assert_eq!(status.global_epoch, 1);
    }

    #[test]
    fn matching_idempotency_replay_returns_original_receipt_without_second_write() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let token = CredentialIdempotencyToken::parse("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")
            .expect("canonical idempotency token");

        let first = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("same-operation").expect("API key"),
                expected_revision: None,
                idempotency_token: token.clone(),
            })
            .expect("first write");
        let replay = service
            .replace_set(ReplaceCredentialSet {
                set_id,
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("ignored-on-safe-replay").expect("API key"),
                expected_revision: None,
                idempotency_token: token,
            })
            .expect("safe replay");

        assert_eq!(replay, first);
        assert_eq!(store.active_write_count(), 1);
        assert_eq!(service.snapshot_status().global_epoch, 1);
    }

    #[test]
    fn delete_commits_a_retained_tombstone_revision() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let created = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("delete-me").expect("API key"),
                expected_revision: None,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
                )
                .expect("token"),
            })
            .expect("create");

        let deleted = service
            .delete_set(DeleteCredentialSet {
                set_id: set_id.clone(),
                expected_revision: created.new_revision,
                idempotency_token: CredentialIdempotencyToken::parse(
                    "cccccccc-cccc-cccc-cccc-cccccccccccc",
                )
                .expect("token"),
            })
            .expect("delete");
        let status = service.snapshot_status();
        let set_status = status
            .sets
            .iter()
            .find(|set| set.set_id == set_id)
            .expect("Deepgram status");

        assert_eq!(
            deleted.result_code,
            CredentialMutationResultCode::Tombstoned
        );
        assert_eq!(deleted.new_revision, set_status.revision);
        assert_eq!(
            set_status.record_state,
            CredentialSetRecordState::Tombstoned
        );
        assert!(store.active_record_is_tombstone(&set_id));
    }

    #[test]
    fn mutation_session_spans_intent_write_through_journal_commit() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("lock-order-canary").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("dddddddd-dddd-dddd-dddd-dddddddddddd"),
            })
            .expect("mutation");

        assert_eq!(
            store.calls(),
            vec![
                FakeStoreCall::BeginMutation,
                FakeStoreCall::LoadJournal,
                FakeStoreCall::ReadActiveInSession,
                FakeStoreCall::PersistIntent,
                FakeStoreCall::ReplaceActive,
                FakeStoreCall::ReadbackActive,
                FakeStoreCall::CommitJournal,
                FakeStoreCall::EndMutation,
            ]
        );
    }

    #[test]
    fn activation_intent_is_durable_before_staging_and_restart_clears_failed_cuts() {
        let cuts = [
            (
                FakeStoreCall::WriteStaging,
                CredentialStoreFailure::AccessDenied,
                CredentialErrorCode::AccessDenied,
                0,
            ),
            (
                FakeStoreCall::ReadStaging,
                CredentialStoreFailure::Unavailable,
                CredentialErrorCode::CommitUnknown,
                1,
            ),
            (
                FakeStoreCall::CommitJournal,
                CredentialStoreFailure::Unavailable,
                CredentialErrorCode::StoreUnavailable,
                1,
            ),
        ];

        for (index, (cut, failure, expected_code, expected_staging)) in cuts.into_iter().enumerate()
        {
            let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
            let store = Arc::new(FakeCredentialStore::new(journal.clone()));
            let settings = Arc::new(FakeSettingsActivationPort::new(60));
            let service = CredentialService::with_settings_activation_port(
                store.clone(),
                journal,
                Arc::new(DeterministicTokenSource::default()),
                settings.clone(),
            );
            store.fail_next(cut, failure);

            let error = service
                .prepare_settings_activation(PrepareCredentialActivation {
                    set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                    auth_method_id: AuthMethodId::ApiKey,
                    material: StoredSecretBundle::api_key(format!("staging-cut-{index}"))
                        .expect("API key"),
                    expected_revision: None,
                    expected_settings_revision: 60,
                    proposed_settings_revision: 61,
                    settings_draft: settings_draft(),
                    idempotency_token: idempotency(
                        [
                            "31313131-4242-5353-6464-757575757571",
                            "31313131-4242-5353-6464-757575757572",
                            "31313131-4242-5353-6464-757575757573",
                        ][index],
                    ),
                })
                .expect_err("injected staging cut");

            assert_eq!(error.code, expected_code);
            assert_eq!(store.staging_count(), expected_staging);
            let persisted = store.journal_snapshot();
            let pending = persisted
                .pending_activation
                .clone()
                .expect("durable operation identifies staged bytes after every cut");
            assert_eq!(pending.stage, CredentialActivationStage::Staged);
            assert_eq!(persisted.pending_intents.len(), 1);
            assert_eq!(
                service
                    .snapshot_status()
                    .pending_activation
                    .expect("live status exposes the recovery operation")
                    .operation_id,
                pending.operation_id
            );
            let calls = store.calls();
            assert!(
                calls
                    .iter()
                    .position(|call| *call == FakeStoreCall::PersistIntent)
                    < calls
                        .iter()
                        .position(|call| *call == FakeStoreCall::WriteStaging)
            );

            let restarted = CredentialService::with_settings_activation_port(
                store.clone(),
                persisted,
                Arc::new(DeterministicTokenSource::default()),
                settings.clone(),
            );
            let receipt = restarted
                .recover_settings_activation(&pending.operation_id)
                .expect("restart rolls back a non-authoritative staging cut");
            assert_eq!(receipt.result_code, CredentialMutationResultCode::NoChange);
            assert!(restarted.snapshot_status().pending_activation.is_none());
            assert_eq!(store.staging_count(), 0);
            assert!(restarted.events_since(0).events.is_empty());
            let restarted_identity = settings
                .restored_identity()
                .expect("restart passes an identity-only settings fence");
            assert_eq!(restarted_identity.operation_id(), &pending.operation_id);
            assert_eq!(restarted_identity.set_id(), &pending.set_id);
            assert_eq!(
                restarted_identity.authority_instance_id(),
                &store.authority_instance_id()
            );
            assert_eq!(
                restarted_identity.expected_credential_revision(),
                pending.expected_revision.as_ref()
            );
            assert_eq!(
                restarted_identity.proposed_credential_revision(),
                &pending.proposed_revision
            );
            assert_eq!(
                restarted_identity.expected_settings_revision(),
                pending.expected_settings_revision
            );
            assert_eq!(
                restarted_identity.proposed_settings_revision(),
                pending.proposed_settings_revision
            );
            assert!(settings.pending_transaction().is_none());
        }
    }

    #[test]
    fn failed_activation_intent_persistence_never_writes_secret_staging() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        store.fail_next(
            FakeStoreCall::PersistIntent,
            CredentialStoreFailure::Unavailable,
        );
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        let error = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("never-staged").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 70,
                proposed_settings_revision: 71,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("41414141-5252-6363-7474-858585858585"),
            })
            .expect_err("intent persistence is the first durable write");

        assert_eq!(error.code, CredentialErrorCode::StoreUnavailable);
        assert_eq!(store.staging_count(), 0);
        assert!(store.journal_snapshot().pending_activation.is_none());
        assert!(service.snapshot_status().pending_activation.is_none());
        assert!(!store.calls().contains(&FakeStoreCall::WriteStaging));
    }

    #[test]
    fn target_set_pending_intent_gates_followup_after_every_post_intent_cut() {
        let cuts = [
            (
                FakeStoreCall::ReplaceActive,
                CredentialStoreFailure::AccessDenied,
                CredentialErrorCode::AccessDenied,
                0,
            ),
            (
                FakeStoreCall::ReadbackActive,
                CredentialStoreFailure::Unavailable,
                CredentialErrorCode::CommitUnknown,
                1,
            ),
            (
                FakeStoreCall::CommitJournal,
                CredentialStoreFailure::Unavailable,
                CredentialErrorCode::CommitUnknown,
                1,
            ),
        ];

        for (index, (cut, failure, expected_code, expected_writes)) in cuts.into_iter().enumerate()
        {
            let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
            let store = Arc::new(FakeCredentialStore::new(journal.clone()));
            let service = CredentialService::new(
                store.clone(),
                journal,
                Arc::new(DeterministicTokenSource::default()),
            );
            let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
            store.fail_next(cut, failure);

            let first = service
                .replace_set(ReplaceCredentialSet {
                    set_id: set_id.clone(),
                    auth_method_id: AuthMethodId::ApiKey,
                    material: StoredSecretBundle::api_key(format!("post-intent-cut-{index}"))
                        .expect("API key"),
                    expected_revision: None,
                    idempotency_token: idempotency(
                        [
                            "51515151-6262-7373-8484-959595959591",
                            "51515151-6262-7373-8484-959595959592",
                            "51515151-6262-7373-8484-959595959593",
                        ][index],
                    ),
                })
                .expect_err("injected post-intent cut");
            assert_eq!(first.code, expected_code);
            assert_eq!(store.active_write_count(), expected_writes);
            let persisted = store.journal_snapshot();
            assert_eq!(persisted.pending_intents.len(), 1);
            assert_eq!(persisted.pending_intents[0].set_id, set_id);
            assert_eq!(
                service
                    .snapshot_status()
                    .sets
                    .iter()
                    .find(|set| set.set_id == set_id)
                    .expect("target status")
                    .recovery_state,
                CredentialSetRecoveryState::PendingIntent
            );

            let followup = service
                .replace_set(ReplaceCredentialSet {
                    set_id,
                    auth_method_id: AuthMethodId::ApiKey,
                    material: StoredSecretBundle::api_key("must-not-write").expect("API key"),
                    expected_revision: None,
                    idempotency_token: idempotency(
                        [
                            "61616161-7272-8383-9494-a5a5a5a5a5a1",
                            "61616161-7272-8383-9494-a5a5a5a5a5a2",
                            "61616161-7272-8383-9494-a5a5a5a5a5a3",
                        ][index],
                    ),
                })
                .expect_err("target-set pending intent gates a second CAS");
            assert_eq!(followup.code, CredentialErrorCode::OperationInProgress);
            assert_eq!(store.active_write_count(), expected_writes);
            assert_eq!(store.journal_snapshot().pending_intents.len(), 1);
        }
    }

    #[test]
    fn committed_events_are_epoch_ordered_and_a_retention_gap_requires_resnapshot() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::with_event_capacity(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
            2,
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);

        let first = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("generation-one").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee"),
            })
            .expect("first");
        let second = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("generation-two").expect("API key"),
                expected_revision: first.new_revision,
                idempotency_token: idempotency("ffffffff-ffff-ffff-ffff-ffffffffffff"),
            })
            .expect("second");
        service
            .delete_set(DeleteCredentialSet {
                set_id,
                expected_revision: second.new_revision,
                idempotency_token: idempotency("11111111-aaaa-bbbb-cccc-222222222222"),
            })
            .expect("third");

        let gap = service.events_since(0);
        assert!(gap.gap_detected);
        assert!(gap.events.is_empty());
        assert_eq!(gap.latest_epoch, 3);

        let retained = service.events_since(1);
        assert!(!retained.gap_detected);
        assert_eq!(
            retained
                .events
                .iter()
                .map(|event| event.global_epoch)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
        assert!(retained.events.iter().all(|event| {
            event.receipt.set_id == CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram)
                && !event.invalidations.is_empty()
        }));
    }

    #[test]
    fn terminal_epoch_rejects_replace_before_any_write_or_publication() {
        let mut journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        journal.global_epoch = u64::MAX;
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal.clone(),
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let before = service.snapshot_status();

        let error = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("must-not-commit-past-max").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("d1d1d1d1-e2e2-f3f3-a4a4-b5b5b5b5b5b5"),
            })
            .expect_err("terminal epoch cannot publish another committed change");

        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);
        assert_eq!(
            error.recovery_action,
            CredentialSafeRecoveryAction::Reconcile
        );
        assert!(!error.retryable);
        assert_eq!(error.set_id, Some(set_id));
        assert_eq!(service.snapshot_status(), before);
        assert_eq!(store.journal_snapshot(), journal);
        assert_eq!(store.active_write_count(), 0);
        assert_eq!(store.staging_count(), 0);
        assert!(store.calls().iter().all(|call| !matches!(
            call,
            FakeStoreCall::PersistIntent
                | FakeStoreCall::ReplaceActive
                | FakeStoreCall::WriteStaging
                | FakeStoreCall::DeleteStaging
                | FakeStoreCall::CommitJournal
        )));
        let at_max = service.events_since(u64::MAX);
        assert_eq!(at_max.latest_epoch, u64::MAX);
        assert!(!at_max.gap_detected);
        assert!(at_max.events.is_empty());
    }

    #[test]
    fn max_minus_one_allows_one_final_commit_then_delete_is_write_free() {
        let mut journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        journal.global_epoch = u64::MAX - 1;
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);

        let final_commit = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("final-epoch-generation").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("e2e2e2e2-f3f3-a4a4-b5b5-c6c6c6c6c6c6"),
            })
            .expect("MAX minus one can publish the final epoch");
        assert_eq!(service.snapshot_status().global_epoch, u64::MAX);
        let final_batch = service.events_since(u64::MAX - 1);
        assert!(!final_batch.gap_detected);
        assert_eq!(final_batch.events.len(), 1);
        assert_eq!(final_batch.events[0].global_epoch, u64::MAX);
        let max_cursor = service.events_since(u64::MAX);
        assert!(!max_cursor.gap_detected);
        assert!(max_cursor.events.is_empty());

        let before_status = service.snapshot_status();
        let before_journal = store.journal_snapshot();
        let before_calls = store.calls().len();
        let before_active_writes = store.active_write_count();
        let error = service
            .delete_set(DeleteCredentialSet {
                set_id: set_id.clone(),
                expected_revision: final_commit.new_revision,
                idempotency_token: idempotency("f3f3f3f3-a4a4-b5b5-c6c6-d7d7d7d7d7d7"),
            })
            .expect_err("no committed event exists past MAX");

        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);
        assert_eq!(
            error.recovery_action,
            CredentialSafeRecoveryAction::Reconcile
        );
        assert_eq!(service.snapshot_status(), before_status);
        assert_eq!(store.journal_snapshot(), before_journal);
        assert_eq!(store.active_write_count(), before_active_writes);
        assert!(store.calls()[before_calls..].iter().all(|call| !matches!(
            call,
            FakeStoreCall::PersistIntent
                | FakeStoreCall::ReplaceActive
                | FakeStoreCall::WriteStaging
                | FakeStoreCall::DeleteStaging
                | FakeStoreCall::CommitJournal
        )));
        assert!(service.events_since(u64::MAX).events.is_empty());
    }

    #[test]
    fn ambiguous_final_replace_commit_replay_reconciles_terminal_authority_without_writes() {
        let mut journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        journal.global_epoch = u64::MAX - 1;
        let store = Arc::new(AfterEffectCommitUnknownStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let token = idempotency("f4f4f4f4-a5a5-b6b6-c7c7-d8d8d8d8d8d8");
        store.fail_commit_after_effect_after(0);

        let error = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("ambiguous-final-replace").expect("API key"),
                expected_revision: None,
                idempotency_token: token.clone(),
            })
            .expect_err("final journal readback is ambiguous");

        assert_eq!(error.code, CredentialErrorCode::CommitUnknown);
        assert_eq!(service.snapshot_status().global_epoch, u64::MAX - 1);
        let authoritative = store.journal_snapshot();
        assert_eq!(authoritative.global_epoch, u64::MAX);
        let persisted_receipt = authoritative
            .idempotency_entry(&token)
            .expect("committed replace history")
            .receipt
            .clone();
        let calls_before_replay = store.calls().len();
        let writes_before_replay = store.active_write_count();

        let replay = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("ignored-replay-material").expect("API key"),
                expected_revision: None,
                idempotency_token: token,
            })
            .expect("exact replay reconciles authoritative completion");

        assert_eq!(replay, persisted_receipt);
        assert_eq!(service.snapshot_status().global_epoch, u64::MAX);
        assert_eq!(store.active_write_count(), writes_before_replay);
        assert!(
            store.calls()[calls_before_replay..]
                .iter()
                .all(|call| !matches!(
                    call,
                    FakeStoreCall::PersistIntent
                        | FakeStoreCall::ReplaceActive
                        | FakeStoreCall::WriteStaging
                        | FakeStoreCall::DeleteStaging
                        | FakeStoreCall::CommitJournal
                ))
        );
        let missed = service.events_since(u64::MAX - 1);
        assert_eq!(missed.latest_epoch, u64::MAX);
        assert!(missed.gap_detected);
        assert!(missed.events.is_empty());
        let terminal = service.events_since(u64::MAX);
        assert_eq!(terminal.latest_epoch, u64::MAX);
        assert!(!terminal.gap_detected);
        assert!(terminal.events.is_empty());
    }

    #[test]
    fn ambiguous_final_delete_commit_replay_reconciles_terminal_authority_without_writes() {
        let mut journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        journal.global_epoch = u64::MAX - 2;
        let store = Arc::new(AfterEffectCommitUnknownStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let created = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("delete-before-terminal").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("a5a5a5a5-b6b6-c7c7-d8d8-e9e9e9e9e9e9"),
            })
            .expect("seed MAX minus one authority");
        let expected_revision = created.new_revision.clone();
        let token = idempotency("b6b6b6b6-c7c7-d8d8-e9e9-f0f0f0f0f0f0");
        store.fail_commit_after_effect_after(0);

        let error = service
            .delete_set(DeleteCredentialSet {
                set_id: set_id.clone(),
                expected_revision: expected_revision.clone(),
                idempotency_token: token.clone(),
            })
            .expect_err("final delete journal readback is ambiguous");

        assert_eq!(error.code, CredentialErrorCode::CommitUnknown);
        assert_eq!(service.snapshot_status().global_epoch, u64::MAX - 1);
        let authoritative = store.journal_snapshot();
        assert_eq!(authoritative.global_epoch, u64::MAX);
        let persisted_receipt = authoritative
            .idempotency_entry(&token)
            .expect("committed delete history")
            .receipt
            .clone();
        let calls_before_replay = store.calls().len();
        let writes_before_replay = store.active_write_count();

        let replay = service
            .delete_set(DeleteCredentialSet {
                set_id,
                expected_revision,
                idempotency_token: token,
            })
            .expect("exact delete replay reconciles authoritative completion");

        assert_eq!(replay, persisted_receipt);
        assert_eq!(service.snapshot_status().global_epoch, u64::MAX);
        assert_eq!(store.active_write_count(), writes_before_replay);
        assert!(
            store.calls()[calls_before_replay..]
                .iter()
                .all(|call| !matches!(
                    call,
                    FakeStoreCall::PersistIntent
                        | FakeStoreCall::ReplaceActive
                        | FakeStoreCall::WriteStaging
                        | FakeStoreCall::DeleteStaging
                        | FakeStoreCall::CommitJournal
                ))
        );
        let missed = service.events_since(u64::MAX - 1);
        assert_eq!(missed.latest_epoch, u64::MAX);
        assert!(missed.gap_detected);
        assert!(missed.events.is_empty());
        assert!(service.events_since(u64::MAX).events.is_empty());
    }

    #[test]
    fn terminal_epoch_rejects_activation_prepare_before_intent_or_staging() {
        let mut journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        journal.global_epoch = u64::MAX;
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(23));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal.clone(),
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let before = service.snapshot_status();

        let error = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("must-not-stage-past-max").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 23,
                proposed_settings_revision: 24,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("a4a4a4a4-b5b5-c6c6-d7d7-e8e8e8e8e8e8"),
            })
            .expect_err("terminal epoch cannot reserve an activation event");

        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);
        assert_eq!(
            error.recovery_action,
            CredentialSafeRecoveryAction::Reconcile
        );
        assert_eq!(service.snapshot_status(), before);
        assert_eq!(store.journal_snapshot(), journal);
        assert_eq!(store.active_write_count(), 0);
        assert_eq!(store.staging_count(), 0);
        assert!(settings.calls().is_empty());
        assert!(store.calls().iter().all(|call| !matches!(
            call,
            FakeStoreCall::PersistIntent
                | FakeStoreCall::ReplaceActive
                | FakeStoreCall::WriteStaging
                | FakeStoreCall::DeleteStaging
                | FakeStoreCall::CommitJournal
        )));
    }

    #[test]
    fn event_cursor_detects_a_committed_epoch_missing_from_the_published_tail() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("published-generation").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("c2c2c2c2-d3d3-e4e4-f5f5-a6a6a6a6a6a6"),
            })
            .expect("first committed event");

        // Deterministically model the observable interval after journal epoch
        // installation and before the matching event-log insertion.
        service
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .global_epoch = 2;

        let batch = service.events_since(1);
        assert_eq!(batch.latest_epoch, 2);
        assert!(batch.gap_detected);
        assert!(batch.events.is_empty());
    }

    #[test]
    fn event_snapshot_reports_gap_when_retention_overtakes_a_captured_latest_epoch() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = Arc::new(CredentialService::with_event_capacity(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
            1,
        ));
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let initial = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("snapshot-generation-one").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("73737373-8484-9595-a6a6-b7b7b7b7b7b7"),
            })
            .expect("initial event");
        let reader_cursor = service.snapshot_status().global_epoch;
        assert_eq!(reader_cursor, 1);

        let (snapshot_captured_tx, snapshot_captured_rx) = mpsc::channel();
        let (resume_reader_tx, resume_reader_rx) = mpsc::channel();
        service.pause_next_event_snapshot(snapshot_captured_tx, resume_reader_rx);
        let reader_service = Arc::clone(&service);
        let reader = std::thread::spawn(move || reader_service.events_since(reader_cursor));
        snapshot_captured_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("reader captures journal epoch before later commits");

        let second = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("snapshot-generation-two").expect("API key"),
                expected_revision: initial.new_revision,
                idempotency_token: idempotency("84848484-9595-a6a6-b7b7-c8c8c8c8c8c8"),
            })
            .expect("epoch N publishes");
        service
            .replace_set(ReplaceCredentialSet {
                set_id,
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("snapshot-generation-three")
                    .expect("API key"),
                expected_revision: second.new_revision,
                idempotency_token: idempotency("95959595-a6a6-b7b7-c8c8-d9d9d9d9d9d9"),
            })
            .expect("epoch N plus one publishes and evicts N");
        resume_reader_tx.send(()).expect("resume event reader");

        let batch = reader.join().expect("event reader completes");
        assert_eq!(batch.latest_epoch, reader_cursor);
        assert!(batch.gap_detected);
        assert!(batch.events.is_empty());
        assert_eq!(service.snapshot_status().global_epoch, reader_cursor + 2);
    }

    #[test]
    fn event_snapshot_never_returns_events_newer_than_its_advertised_latest_epoch() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = Arc::new(CredentialService::with_event_capacity(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
            2,
        ));
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let initial = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("bounded-generation-one").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("a7a7a7a7-b8b8-c9c9-dada-ebebebebebeb"),
            })
            .expect("initial event");
        let reader_cursor = service.snapshot_status().global_epoch;
        assert_eq!(reader_cursor, 1);

        let (snapshot_captured_tx, snapshot_captured_rx) = mpsc::channel();
        let (resume_reader_tx, resume_reader_rx) = mpsc::channel();
        service.pause_next_event_snapshot(snapshot_captured_tx, resume_reader_rx);
        let reader_service = Arc::clone(&service);
        let reader = std::thread::spawn(move || reader_service.events_since(reader_cursor));
        snapshot_captured_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("reader captures journal epoch before later commits");

        let second = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("bounded-generation-two").expect("API key"),
                expected_revision: initial.new_revision,
                idempotency_token: idempotency("b8b8b8b8-c9c9-dada-ebeb-fcfcfcfcfcfc"),
            })
            .expect("first future event publishes");
        service
            .replace_set(ReplaceCredentialSet {
                set_id,
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("bounded-generation-three").expect("API key"),
                expected_revision: second.new_revision,
                idempotency_token: idempotency("c9c9c9c9-dada-ebeb-fcfc-adadadadadad"),
            })
            .expect("second future event publishes");
        resume_reader_tx.send(()).expect("resume event reader");

        let batch = reader.join().expect("event reader completes");
        assert_eq!(batch.latest_epoch, reader_cursor);
        assert!(!batch.gap_detected);
        assert!(batch.events.is_empty());
        assert_eq!(service.snapshot_status().global_epoch, reader_cursor + 2);
    }

    #[test]
    fn failed_exact_readback_never_publishes_a_success_event() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        store.corrupt_next_readback();
        let service = CredentialService::new(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        let error = service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("unverified").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("22222222-aaaa-bbbb-cccc-333333333333"),
            })
            .expect_err("exact readback mismatch");

        assert_eq!(error.code, CredentialErrorCode::CommitUnknown);
        assert!(service.events_since(0).events.is_empty());
        assert_eq!(service.snapshot_status().global_epoch, 0);
    }

    #[test]
    fn shared_active_and_staging_verifier_maps_absent_or_mismatched_readback_to_commit_unknown() {
        let expected = EncodedCredentialRecord::from_boundary_bytes(b"expected-record".to_vec());
        let exact = EncodedCredentialRecord::from_boundary_bytes(b"expected-record".to_vec());
        let mismatch = EncodedCredentialRecord::from_boundary_bytes(b"mismatched-record".to_vec());

        assert_eq!(verify_exact_store_readback(&expected, Some(&exact)), Ok(()));
        assert_eq!(
            verify_exact_store_readback(&expected, None),
            Err(CredentialStoreFailure::CommitUnknown)
        );
        assert_eq!(
            verify_exact_store_readback(&expected, Some(&mismatch)),
            Err(CredentialStoreFailure::CommitUnknown)
        );
    }

    #[test]
    fn concurrent_writers_with_one_expected_revision_have_one_cas_winner() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let services = [
            Arc::new(CredentialService::new(
                store.clone(),
                journal.clone(),
                Arc::new(DeterministicTokenSource::default()),
            )),
            Arc::new(CredentialService::new(
                store.clone(),
                journal,
                Arc::new(DeterministicTokenSource::default()),
            )),
        ];
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for (service, (secret, token)) in services.iter().cloned().zip([
            ("race-one", "33333333-aaaa-bbbb-cccc-444444444444"),
            ("race-two", "44444444-aaaa-bbbb-cccc-555555555555"),
        ]) {
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                service.replace_set(ReplaceCredentialSet {
                    set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                    auth_method_id: AuthMethodId::ApiKey,
                    material: StoredSecretBundle::api_key(secret).expect("API key"),
                    expected_revision: None,
                    idempotency_token: idempotency(token),
                })
            }));
        }
        barrier.wait();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().expect("worker does not panic"))
            .collect::<Vec<_>>();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter_map(|result| result.as_ref().err())
                .filter(|error| error.code == CredentialErrorCode::RevisionConflict)
                .count(),
            1
        );
        assert_eq!(store.active_write_count(), 1);
        assert_eq!(
            services
                .iter()
                .map(|service| service.snapshot_status().global_epoch)
                .sum::<u64>(),
            1
        );
    }

    #[test]
    fn native_failure_kinds_remain_distinct_content_free_public_errors() {
        let cases = [
            (
                CredentialStoreFailure::Missing,
                CredentialErrorCode::Missing,
            ),
            (CredentialStoreFailure::Locked, CredentialErrorCode::Locked),
            (
                CredentialStoreFailure::AccessDenied,
                CredentialErrorCode::AccessDenied,
            ),
            (
                CredentialStoreFailure::Cancelled,
                CredentialErrorCode::Cancelled,
            ),
            (
                CredentialStoreFailure::Unavailable,
                CredentialErrorCode::StoreUnavailable,
            ),
            (
                CredentialStoreFailure::PermissionHardeningFailed,
                CredentialErrorCode::PermissionHardeningFailed,
            ),
            (
                CredentialStoreFailure::Unsupported,
                CredentialErrorCode::StoreUnsupported,
            ),
            (
                CredentialStoreFailure::CorruptRecord,
                CredentialErrorCode::CorruptRecord,
            ),
            (
                CredentialStoreFailure::UnsupportedSchema,
                CredentialErrorCode::UnsupportedSchema,
            ),
            (
                CredentialStoreFailure::PayloadTooLarge,
                CredentialErrorCode::PayloadTooLarge,
            ),
            (
                CredentialStoreFailure::AmbiguousMatch,
                CredentialErrorCode::AmbiguousMatch,
            ),
            (
                CredentialStoreFailure::OperationInProgress,
                CredentialErrorCode::OperationInProgress,
            ),
            (
                CredentialStoreFailure::CommitUnknown,
                CredentialErrorCode::CommitUnknown,
            ),
            (
                CredentialStoreFailure::Internal,
                CredentialErrorCode::Internal,
            ),
        ];

        for (failure, expected_code) in cases {
            let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
            let store = Arc::new(FakeCredentialStore::new(journal.clone()));
            store.fail_next(FakeStoreCall::ReadActive, failure);
            let service = CredentialService::new(
                store.clone(),
                journal,
                Arc::new(DeterministicTokenSource::default()),
            );

            let error = service
                .resolve_for_use(CredentialUseRequest {
                    set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                    consumer_id: "asr.deepgram",
                    auth_method_id: AuthMethodId::ApiKey,
                    purpose: CredentialPurpose::Asr,
                    audience: CredentialAudience::SecureNetworkOrigin {
                        scheme: SecureTransportScheme::Wss,
                        canonical_host: "api.deepgram.com".to_string(),
                        effective_port: 443,
                    },
                })
                .expect_err("scripted store failure");

            assert_eq!(error.code, expected_code, "failure {failure:?}");
            assert_eq!(store.entry_read_count(), 1);
            assert_eq!(store.active_write_count(), 0);
        }
    }

    #[test]
    fn a_stalled_worker_blocks_retry_until_the_adapter_reports_completion() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        store.fail_next(
            FakeStoreCall::BeginMutation,
            CredentialStoreFailure::StalledWorker,
        );
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);

        let stalled = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("first-attempt").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("55555555-aaaa-bbbb-cccc-666666666666"),
            })
            .expect_err("deadline crosses into stalled state");
        assert_eq!(stalled.code, CredentialErrorCode::StalledWorker);
        let first_stalled_operation = service
            .snapshot_status()
            .worker
            .operation_id
            .expect("stalled operation identity");

        let rejected = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("must-not-run").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("66666666-aaaa-bbbb-cccc-777777777777"),
            })
            .expect_err("competing mutation rejected");
        assert_eq!(rejected.code, CredentialErrorCode::StalledWorker);
        assert_eq!(
            store
                .calls()
                .iter()
                .filter(|call| **call == FakeStoreCall::BeginMutation)
                .count(),
            1
        );

        let stale_operation = CredentialOperationId::parse("ffffffff-eeee-dddd-cccc-bbbbbbbbbbbb")
            .expect("canonical stale operation");
        assert!(!service.complete_stalled_operation(&stale_operation));
        assert_eq!(
            service.snapshot_status().worker.state,
            CredentialWorkerState::Stalled
        );
        assert!(service.complete_stalled_operation(&first_stalled_operation));
        assert!(!service.complete_stalled_operation(&first_stalled_operation));

        store.fail_next(
            FakeStoreCall::BeginMutation,
            CredentialStoreFailure::StalledWorker,
        );
        let second_stalled = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("second-stall").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("67676767-7878-8989-9090-a1a1a1a1a1a1"),
            })
            .expect_err("a later operation stalls independently");
        assert_eq!(second_stalled.code, CredentialErrorCode::StalledWorker);
        let second_stalled_operation = service
            .snapshot_status()
            .worker
            .operation_id
            .expect("second stalled operation identity");
        assert_ne!(second_stalled_operation, first_stalled_operation);
        assert!(!service.complete_stalled_operation(&first_stalled_operation));
        assert_eq!(
            service.snapshot_status().worker.state,
            CredentialWorkerState::Stalled
        );
        assert!(service.complete_stalled_operation(&second_stalled_operation));
        assert!(!service.complete_stalled_operation(&second_stalled_operation));

        service
            .replace_set(ReplaceCredentialSet {
                set_id,
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("after-return").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("77777777-aaaa-bbbb-cccc-888888888888"),
            })
            .expect("explicit completion permits a new mutation");
        assert_eq!(
            service.snapshot_status().worker.state,
            CredentialWorkerState::Idle
        );
    }

    #[test]
    fn live_settings_pending_calls_hold_the_authoritative_worker_permit() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(WorkerInspectingSettingsPort::new(7));
        let service = Arc::new(CredentialService::with_settings_activation_port(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        ));
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("worker-bound-live-settings")
                    .expect("API key"),
                expected_revision: None,
                expected_settings_revision: 7,
                proposed_settings_revision: 8,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("01010101-1212-4343-8484-000000000001"),
            })
            .expect("prepare activation");
        settings.arm(
            &service,
            prepared.operation_id.clone(),
            prepared.set_id.clone(),
        );

        let receipt = service
            .commit_settings_activation(prepared)
            .expect("settings and credential commit under one serialized identity");

        assert_eq!(receipt.set_id, set_id);
        assert_eq!(
            service.snapshot_status().worker.state,
            CredentialWorkerState::Idle
        );
    }

    #[test]
    fn restarted_settings_verification_holds_the_authoritative_worker_permit() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(WorkerInspectingSettingsPort::new(17));
        let service = Arc::new(CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        ));
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("worker-bound-restarted-settings")
                    .expect("API key"),
                expected_revision: None,
                expected_settings_revision: 17,
                proposed_settings_revision: 18,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("02020202-1313-4444-8585-000000000002"),
            })
            .expect("prepare activation");
        service
            .transition_activation_stage(
                &prepared,
                CredentialActivationStage::Staged,
                CredentialActivationStage::SettingsPending,
            )
            .expect("persist settings-pending stage");
        settings
            .persist_without_inspection(prepared.transaction())
            .expect("persist exact pending settings before restart");
        let operation_id = prepared.operation_id.clone();
        let set_id = prepared.set_id.clone();
        let restart_journal = store.journal_snapshot();
        drop(service);

        let restarted = Arc::new(CredentialService::with_settings_activation_port(
            store,
            restart_journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        ));
        settings.arm(&restarted, operation_id.clone(), set_id.clone());

        let receipt = restarted
            .recover_settings_activation(&operation_id)
            .expect("restart completes under the pending operation identity");

        assert_eq!(receipt.set_id, set_id);
        assert_eq!(
            restarted.snapshot_status().worker.state,
            CredentialWorkerState::Idle
        );
    }

    #[test]
    fn staged_activation_is_invisible_until_settings_and_active_record_commit() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(7));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let initial = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("old-generation").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("88888888-aaaa-bbbb-cccc-999999999999"),
            })
            .expect("initial active generation");

        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("staged-generation").expect("API key"),
                expected_revision: initial.new_revision.clone(),
                expected_settings_revision: 7,
                proposed_settings_revision: 8,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("99999999-aaaa-bbbb-cccc-aaaaaaaaaaaa"),
            })
            .expect("prepare");

        let prepared_status = service.snapshot_status();
        assert_eq!(prepared_status.global_epoch, 1);
        assert!(prepared_status.pending_activation.is_some());
        assert_eq!(store.staging_count(), 1);
        assert!(service.events_since(1).events.is_empty());
        let before_commit = service
            .resolve_for_use(CredentialUseRequest {
                set_id: set_id.clone(),
                consumer_id: "asr.deepgram",
                auth_method_id: AuthMethodId::ApiKey,
                purpose: CredentialPurpose::Asr,
                audience: CredentialAudience::SecureNetworkOrigin {
                    scheme: SecureTransportScheme::Wss,
                    canonical_host: "api.deepgram.com".to_string(),
                    effective_port: 443,
                },
            })
            .expect("old active remains resolvable");
        assert_eq!(
            match before_commit {
                CredentialResolution::Stored(lease) => {
                    lease.expose_api_key(str::to_owned)
                }
                CredentialResolution::NonStore(_) => None,
            },
            Some("old-generation".to_string())
        );
        let reserved = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("must-not-overtake").expect("API key"),
                expected_revision: initial.new_revision,
                idempotency_token: idempotency("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
            })
            .expect_err("pending activation reserves its set");
        assert_eq!(reserved.code, CredentialErrorCode::OperationInProgress);

        let activated = service
            .commit_settings_activation(prepared)
            .expect("commit staged credential and settings");
        let status = service.snapshot_status();
        assert_eq!(status.global_epoch, 2);
        assert!(status.pending_activation.is_none());
        assert_eq!(
            activated.new_revision,
            status
                .sets
                .iter()
                .find(|set| { set.set_id == set_id })
                .and_then(|set| set.revision.clone())
        );
        assert_eq!(store.staging_count(), 0);
        assert_eq!(settings.current_revision(), 8);
        assert!(!settings.has_pending_marker());
        assert_eq!(
            settings.calls(),
            vec![
                FakeSettingsCall::PersistPending,
                FakeSettingsCall::VerifyPending,
                FakeSettingsCall::VerifyCommitted,
                FakeSettingsCall::ClearPending,
            ]
        );
        let event_batch = service.events_since(1);
        assert!(!event_batch.gap_detected);
        assert_eq!(event_batch.events.len(), 1);
        assert_eq!(event_batch.events[0].receipt, activated);
    }

    #[test]
    fn prepare_freezes_generated_operation_authority_and_exact_same_revision_draft_into_one_transaction()
     {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(7));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let selected_draft = ValidatedNonSecretSettingsDraft::from_validated_bytes(
            b"selected-non-secret-settings-draft".to_vec(),
        );
        let swapped_draft = ValidatedNonSecretSettingsDraft::from_validated_bytes(
            b"same-revision-swapped-settings-draft".to_vec(),
        );

        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("staged-generation").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 7,
                proposed_settings_revision: 8,
                idempotency_token: idempotency("67676767-8989-1010-2323-454545454545"),
                settings_draft: selected_draft.clone(),
            })
            .expect("prepare binds generated authority transaction");
        let transaction = prepared.transaction().clone();
        let expected_operation =
            CredentialOperationId::parse("00000000-0000-0000-0000-000000000001")
                .expect("deterministic operation");

        assert_eq!(transaction.identity().operation_id(), &expected_operation);
        assert_eq!(transaction.identity().set_id(), &set_id);
        assert_eq!(
            transaction.identity().authority_instance_id(),
            &store.authority_instance_id()
        );
        assert_eq!(transaction.identity().expected_credential_revision(), None);
        assert_eq!(transaction.identity().expected_settings_revision(), 7);
        assert_eq!(transaction.identity().proposed_settings_revision(), 8);
        assert_eq!(transaction.settings_draft(), &selected_draft);

        settings
            .persist_pending_settings(&transaction)
            .expect("first exact transaction persists");
        settings
            .persist_pending_settings(&transaction)
            .expect("exact transaction replay is idempotent");

        let swapped =
            SettingsActivationTransaction::new(transaction.identity().clone(), swapped_draft);
        let wrong_operation = SettingsActivationTransaction::new(
            SettingsActivationIdentity {
                operation_id: CredentialOperationId::parse("ffffffff-ffff-4fff-8fff-ffffffffffff")
                    .expect("canonical wrong operation"),
                ..transaction.identity().clone()
            },
            selected_draft.clone(),
        );
        let foreign_authority = SettingsActivationTransaction::new(
            SettingsActivationIdentity {
                authority_instance_id: CredentialAuthorityInstanceId::from_test_bytes([0x5a; 16]),
                ..transaction.identity().clone()
            },
            selected_draft,
        );

        for rejected in [&swapped, &wrong_operation, &foreign_authority] {
            assert_eq!(
                settings.persist_pending_settings(rejected),
                Err(CredentialStoreFailure::RevisionConflict)
            );
            assert_eq!(settings.pending_transaction(), Some(transaction.clone()));
            assert_eq!(settings.current_revision(), 8);
            assert_eq!(store.active_write_count(), 0);
            assert_eq!(store.staging_count(), 1);
        }
    }

    #[test]
    fn concurrent_same_revision_drafts_have_one_exact_winner_and_stale_replay_fails_closed() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(7));
        let service = CredentialService::with_settings_activation_port(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("concurrent-draft").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 7,
                proposed_settings_revision: 8,
                idempotency_token: idempotency("77777777-8989-4a4a-8b8b-454545454545"),
                settings_draft: ValidatedNonSecretSettingsDraft::from_validated_bytes(
                    b"concurrent-draft-a".to_vec(),
                ),
            })
            .expect("prepare one closed identity");
        let first = prepared.transaction().clone();
        let second = SettingsActivationTransaction::new(
            first.identity().clone(),
            ValidatedNonSecretSettingsDraft::from_validated_bytes(b"concurrent-draft-b".to_vec()),
        );
        let barrier = Arc::new(Barrier::new(3));
        let mut workers = Vec::new();
        for candidate in [first.clone(), second.clone()] {
            let settings = settings.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                settings.persist_pending_settings(&candidate)
            }));
        }
        barrier.wait();
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().expect("draft writer thread"))
            .collect();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| {
                    result.as_ref().err() == Some(&CredentialStoreFailure::RevisionConflict)
                })
                .count(),
            1
        );
        let winner = settings
            .pending_transaction()
            .expect("one exact transaction wins the settings CAS");
        assert!(winner == first || winner == second);
        settings
            .persist_pending_settings(&winner)
            .expect("only the exact winning identity and draft replay idempotently");
        settings
            .clear_pending_settings(winner.identity())
            .expect("clear the winning settings transaction");
        assert_eq!(
            settings.persist_pending_settings(&winner),
            Err(CredentialStoreFailure::RevisionConflict)
        );
    }

    #[test]
    fn stale_clone_rollback_race_preserves_the_winner_cleanup_authority() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(7));
        let service = Arc::new(CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        ));
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("exact-clone-race").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 7,
                proposed_settings_revision: 8,
                idempotency_token: idempotency("79797979-8989-4a4a-8b8b-454545454545"),
                settings_draft: settings_draft(),
            })
            .expect("prepare one closed activation identity");
        let proposed_revision = prepared.proposed_revision.clone();
        let (credential_pending_tx, credential_pending_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        service.pause_next_activation_before_active_commit(credential_pending_tx, resume_rx);

        let paused_service = service.clone();
        let paused_prepared = prepared.clone();
        let paused =
            std::thread::spawn(move || paused_service.commit_settings_activation(paused_prepared));
        credential_pending_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("first invoker pauses after claiming credential-pending authority");
        assert_eq!(
            service
                .snapshot_status()
                .pending_activation
                .expect("prepared activation remains pending")
                .stage,
            CredentialActivationStage::CredentialPending
        );

        settings.fail_next(
            FakeSettingsCall::ClearPending,
            CredentialStoreFailure::Unavailable,
        );
        let replay_service = service.clone();
        let replay =
            std::thread::spawn(move || replay_service.commit_settings_activation(prepared));
        let replay_result = replay.join().expect("exact-clone replay thread");
        resume_tx
            .send(())
            .expect("resume the original credential-pending invoker");
        let paused_result = paused.join().expect("original activation thread");

        assert!(
            replay_result.is_err(),
            "one exact clone must lose authority"
        );
        assert!(
            paused_result.is_err(),
            "the winner must expose its injected cleanup failure"
        );
        assert_eq!(
            replay_result.expect_err("exact clone is stale").code,
            CredentialErrorCode::Conflict
        );
        assert_eq!(
            paused_result
                .expect_err("winner cleanup remains pending")
                .code,
            CredentialErrorCode::RecoveryRequired
        );
        assert_eq!(store.active_write_count(), 1);
        assert_eq!(settings.current_revision(), 8);
        assert!(settings.has_pending_marker());
        assert_eq!(store.staging_count(), 1);
        assert!(!settings.calls().contains(&FakeSettingsCall::RestoreBackup));
        assert!(!store.calls().contains(&FakeStoreCall::DeleteStaging));
        assert_eq!(
            settings.calls(),
            vec![
                FakeSettingsCall::PersistPending,
                FakeSettingsCall::VerifyPending,
                FakeSettingsCall::VerifyCommitted,
                FakeSettingsCall::ClearPending,
            ]
        );

        let status = service.snapshot_status();
        assert_eq!(status.global_epoch, 0);
        assert_eq!(
            status
                .pending_activation
                .expect("winner cleanup authority remains durable")
                .stage,
            CredentialActivationStage::CleanupPending
        );
        assert!(service.events_since(0).events.is_empty());
        let persisted = store.journal_snapshot();
        assert_eq!(
            persisted
                .pending_activation
                .as_ref()
                .expect("persisted cleanup authority remains durable")
                .stage,
            CredentialActivationStage::CleanupPending
        );
        assert_eq!(
            persisted
                .set_state(&set_id)
                .and_then(|state| state.revision.as_ref()),
            Some(&proposed_revision)
        );
    }

    #[test]
    fn rollback_final_stalled_journal_cut_latches_worker_status() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(37));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        settings.fail_next(
            FakeSettingsCall::VerifyPending,
            CredentialStoreFailure::Unavailable,
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("rollback-stalled-cut").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 37,
                proposed_settings_revision: 38,
                idempotency_token: idempotency("70707070-8989-4a4a-8b8b-454545454545"),
                settings_draft: settings_draft(),
            })
            .expect("prepare activation");
        store.fail_after(
            FakeStoreCall::CommitJournal,
            2,
            CredentialStoreFailure::StalledWorker,
        );

        let error = service
            .commit_settings_activation(prepared)
            .expect_err("final rollback journal interaction stalls");
        let status_after_stall = service.snapshot_status();
        let rollback_calls = store.calls();
        let calls_before_competing_mutation = rollback_calls.len();
        let competing = service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Openai),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("must-not-enter-store").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("71717171-8989-4a4a-8b8b-565656565656"),
            })
            .expect_err("competing mutation remains admission-blocked");

        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);
        assert_eq!(settings.current_revision(), 37);
        assert!(!settings.has_pending_marker());
        assert_eq!(store.staging_count(), 0);
        assert_eq!(
            store
                .journal_snapshot()
                .pending_activation
                .expect("recovery claim remains authoritative")
                .stage,
            CredentialActivationStage::RecoveryRequired
        );
        assert_eq!(
            settings.calls(),
            vec![
                FakeSettingsCall::PersistPending,
                FakeSettingsCall::VerifyPending,
                FakeSettingsCall::RestoreBackup,
            ]
        );
        assert_eq!(
            &rollback_calls[rollback_calls.len() - 3..],
            &[
                FakeStoreCall::DeleteStaging,
                FakeStoreCall::CommitJournal,
                FakeStoreCall::EndMutation,
            ]
        );
        assert_eq!(
            status_after_stall.worker.state,
            CredentialWorkerState::Stalled
        );
        assert_eq!(competing.code, CredentialErrorCode::StalledWorker);
        assert_eq!(store.calls().len(), calls_before_competing_mutation);
    }

    #[test]
    fn every_phase_specific_activation_remap_latches_stalled_worker_before_returning() {
        #[derive(Clone, Copy, Debug)]
        enum Cut {
            StagingReadback,
            PersistPendingSettings,
            VerifyPendingSettings,
            ActiveFinalJournal,
            CleanupVerifyCommittedSettings,
            CleanupClearPendingSettings,
            CleanupDeleteStaging,
            CleanupFinalJournal,
        }

        let cuts = [
            (Cut::StagingReadback, CredentialErrorCode::CommitUnknown),
            (
                Cut::PersistPendingSettings,
                CredentialErrorCode::StalledWorker,
            ),
            (
                Cut::VerifyPendingSettings,
                CredentialErrorCode::StalledWorker,
            ),
            (Cut::ActiveFinalJournal, CredentialErrorCode::CommitUnknown),
            (
                Cut::CleanupVerifyCommittedSettings,
                CredentialErrorCode::RecoveryRequired,
            ),
            (
                Cut::CleanupClearPendingSettings,
                CredentialErrorCode::RecoveryRequired,
            ),
            (
                Cut::CleanupDeleteStaging,
                CredentialErrorCode::RecoveryRequired,
            ),
            (
                Cut::CleanupFinalJournal,
                CredentialErrorCode::RecoveryRequired,
            ),
        ];

        for (index, (cut, expected_code)) in cuts.into_iter().enumerate() {
            let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
            let store = Arc::new(FakeCredentialStore::new(journal.clone()));
            let settings_revision = 60 + index as u64;
            let settings = Arc::new(FakeSettingsActivationPort::new(settings_revision));
            let service = CredentialService::with_settings_activation_port(
                store.clone(),
                journal,
                Arc::new(DeterministicTokenSource::default()),
                settings.clone(),
            );
            let expected_operation =
                CredentialOperationId::parse("00000000-0000-0000-0000-000000000001")
                    .expect("deterministic activation operation");

            if matches!(cut, Cut::StagingReadback) {
                store.fail_next(
                    FakeStoreCall::ReadStaging,
                    CredentialStoreFailure::StalledWorker,
                );
            }
            let prepared = service.prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key(format!("activation-cut-{index}"))
                    .expect("API key"),
                expected_revision: None,
                expected_settings_revision: settings_revision,
                proposed_settings_revision: settings_revision + 1,
                idempotency_token: idempotency(&format!(
                    "81818181-9292-4a3a-b4b4-{suffix:012x}",
                    suffix = index + 1
                )),
                settings_draft: settings_draft(),
            });

            let error = if matches!(cut, Cut::StagingReadback) {
                prepared.expect_err("staging readback stalls after the write")
            } else {
                let prepared = prepared.expect("prepare activation before selected cut");
                match cut {
                    Cut::StagingReadback => unreachable!("handled before prepare"),
                    Cut::PersistPendingSettings => settings.fail_next(
                        FakeSettingsCall::PersistPending,
                        CredentialStoreFailure::StalledWorker,
                    ),
                    Cut::VerifyPendingSettings => settings.fail_next(
                        FakeSettingsCall::VerifyPending,
                        CredentialStoreFailure::StalledWorker,
                    ),
                    Cut::ActiveFinalJournal => store.fail_after(
                        FakeStoreCall::CommitJournal,
                        2,
                        CredentialStoreFailure::StalledWorker,
                    ),
                    Cut::CleanupVerifyCommittedSettings => settings.fail_next(
                        FakeSettingsCall::VerifyCommitted,
                        CredentialStoreFailure::StalledWorker,
                    ),
                    Cut::CleanupClearPendingSettings => settings.fail_next(
                        FakeSettingsCall::ClearPending,
                        CredentialStoreFailure::StalledWorker,
                    ),
                    Cut::CleanupDeleteStaging => store.fail_next(
                        FakeStoreCall::DeleteStaging,
                        CredentialStoreFailure::StalledWorker,
                    ),
                    Cut::CleanupFinalJournal => store.fail_after(
                        FakeStoreCall::CommitJournal,
                        3,
                        CredentialStoreFailure::StalledWorker,
                    ),
                }
                service
                    .commit_settings_activation(prepared)
                    .expect_err("selected activation cut stalls")
            };

            assert_eq!(error.code, expected_code, "public semantics for {cut:?}");
            assert_stalled_worker_rejects_followup_without_store_io(
                &service,
                &store,
                &CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                Some(&expected_operation),
                "82828282-9393-4b4b-8c8c-000000000001",
            );
        }
    }

    #[test]
    fn restarted_settings_verification_remap_latches_stalled_worker_before_recovery_io() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(71));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("restart-verification-cut").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 71,
                proposed_settings_revision: 72,
                idempotency_token: idempotency("83838383-9494-4c4c-8d8d-000000000002"),
                settings_draft: settings_draft(),
            })
            .expect("prepare activation");
        service
            .transition_activation_stage(
                &prepared,
                CredentialActivationStage::Staged,
                CredentialActivationStage::SettingsPending,
            )
            .expect("persist settings-pending stage");
        settings
            .persist_pending_settings(prepared.transaction())
            .expect("persist exact pending settings");
        let operation_id = prepared.operation_id.clone();
        let restart_journal = store.journal_snapshot();
        drop(service);

        settings.fail_next(
            FakeSettingsCall::VerifyPending,
            CredentialStoreFailure::StalledWorker,
        );
        let restarted = CredentialService::with_settings_activation_port(
            store.clone(),
            restart_journal,
            Arc::new(DeterministicTokenSource::default()),
            settings,
        );
        let error = restarted
            .recover_settings_activation(&operation_id)
            .expect_err("restart settings verification stalls");

        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);
        let calls_before_second_recovery = store.calls().len();
        let second = restarted
            .recover_settings_activation(&operation_id)
            .expect_err("stalled restart recovery is admission-blocked");
        assert_eq!(second.code, CredentialErrorCode::StalledWorker);
        assert_eq!(store.calls().len(), calls_before_second_recovery);
        assert_stalled_worker_rejects_followup_without_store_io(
            &restarted,
            &store,
            &CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
            Some(&operation_id),
            "84848484-9595-4d4d-8e8e-000000000003",
        );
    }

    #[test]
    fn ordinary_final_journal_remaps_latch_stalled_worker_before_returning_commit_unknown() {
        for delete in [false, true] {
            let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
            let store = Arc::new(FakeCredentialStore::new(journal.clone()));
            let service = CredentialService::new(
                store.clone(),
                journal,
                Arc::new(DeterministicTokenSource::default()),
            );
            let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
            let expected_revision = if delete {
                service
                    .replace_set(ReplaceCredentialSet {
                        set_id: set_id.clone(),
                        auth_method_id: AuthMethodId::ApiKey,
                        material: StoredSecretBundle::api_key("ordinary-delete-seed")
                            .expect("API key"),
                        expected_revision: None,
                        idempotency_token: idempotency("85858585-9696-4e4e-8f8f-000000000004"),
                    })
                    .expect("seed ordinary delete")
                    .new_revision
            } else {
                None
            };
            store.fail_next(
                FakeStoreCall::CommitJournal,
                CredentialStoreFailure::StalledWorker,
            );

            let error = if delete {
                service
                    .delete_set(DeleteCredentialSet {
                        set_id,
                        expected_revision,
                        idempotency_token: idempotency("86868686-9797-4f4f-8080-000000000005"),
                    })
                    .expect_err("delete final journal stalls")
            } else {
                service
                    .replace_set(ReplaceCredentialSet {
                        set_id,
                        auth_method_id: AuthMethodId::ApiKey,
                        material: StoredSecretBundle::api_key("ordinary-replace-cut")
                            .expect("API key"),
                        expected_revision: None,
                        idempotency_token: idempotency("87878787-9898-4040-8181-000000000006"),
                    })
                    .expect_err("replace final journal stalls")
            };

            assert_eq!(error.code, CredentialErrorCode::CommitUnknown);
            assert_stalled_worker_rejects_followup_without_store_io(
                &service,
                &store,
                &CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                None,
                "88888888-9999-4141-8282-000000000007",
            );
        }
    }

    #[test]
    fn normal_activation_stage_vocabulary_allows_only_two_forward_edges() {
        let stages = [
            CredentialActivationStage::Staged,
            CredentialActivationStage::SettingsPending,
            CredentialActivationStage::CredentialPending,
            CredentialActivationStage::CleanupPending,
            CredentialActivationStage::RecoveryRequired,
        ];

        for expected_stage in stages {
            for next_stage in stages {
                assert_eq!(
                    CredentialService::normal_activation_stage_transition_is_allowed(
                        expected_stage,
                        next_stage,
                    ),
                    matches!(
                        (expected_stage, next_stage),
                        (
                            CredentialActivationStage::Staged,
                            CredentialActivationStage::SettingsPending
                        ) | (
                            CredentialActivationStage::SettingsPending,
                            CredentialActivationStage::CredentialPending
                        )
                    ),
                    "unexpected normal activation edge {expected_stage:?} -> {next_stage:?}"
                );
            }
        }
    }

    #[test]
    fn backward_activation_stage_transition_is_rejected_without_effects() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(9));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("backward-stage").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 9,
                proposed_settings_revision: 10,
                idempotency_token: idempotency("7a7a7a7a-8989-4a4a-8b8b-454545454545"),
                settings_draft: settings_draft(),
            })
            .expect("prepare activation");
        service
            .transition_activation_stage(
                &prepared,
                CredentialActivationStage::Staged,
                CredentialActivationStage::SettingsPending,
            )
            .expect("claim the first forward stage");
        let status_before = service.snapshot_status();
        let journal_before = store.journal_snapshot();
        let calls_before = store.calls().len();

        let error = service
            .transition_activation_stage(
                &prepared,
                CredentialActivationStage::SettingsPending,
                CredentialActivationStage::Staged,
            )
            .expect_err("normal stage transitions cannot move backward")
            .into_public();

        assert_eq!(error.code, CredentialErrorCode::Conflict);
        assert_eq!(service.snapshot_status(), status_before);
        assert_eq!(store.journal_snapshot(), journal_before);
        assert!(settings.calls().is_empty());
        assert!(
            store.calls()[calls_before..]
                .iter()
                .all(|call| !matches!(call, FakeStoreCall::CommitJournal))
        );
    }

    #[test]
    fn rollback_after_cleanup_pending_preserves_winner_state_without_effects() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(11));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("cleanup-winner").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 11,
                proposed_settings_revision: 12,
                idempotency_token: idempotency("7b7b7b7b-8989-4a4a-8b8b-454545454545"),
                settings_draft: settings_draft(),
            })
            .expect("prepare activation");
        settings.fail_next(
            FakeSettingsCall::ClearPending,
            CredentialStoreFailure::Unavailable,
        );
        service
            .commit_settings_activation(prepared.clone())
            .expect_err("winner remains CleanupPending");
        let status_before = service.snapshot_status();
        let journal_before = store.journal_snapshot();
        let settings_calls_before = settings.calls().len();
        let store_calls_before = store.calls().len();

        let error = service
            .rollback_settings_then_abort(&prepared)
            .expect_err("advanced winner is not rollback eligible");

        assert_eq!(error.code, CredentialErrorCode::Conflict);
        assert_eq!(service.snapshot_status(), status_before);
        assert_eq!(store.journal_snapshot(), journal_before);
        assert_eq!(settings.current_revision(), 12);
        assert!(settings.has_pending_marker());
        assert_eq!(store.active_write_count(), 1);
        assert_eq!(store.staging_count(), 1);
        assert!(
            !settings.calls()[settings_calls_before..].contains(&FakeSettingsCall::RestoreBackup)
        );
        assert!(
            store.calls()[store_calls_before..]
                .iter()
                .all(|call| !matches!(
                    call,
                    FakeStoreCall::DeleteStaging | FakeStoreCall::CommitJournal
                ))
        );

        let calls_before_recovery_mark = store.calls().len();
        service
            .mark_activation_recovery_required(&prepared)
            .expect("CleanupPending is already a durable recovery gate");
        assert_eq!(
            service
                .snapshot_status()
                .pending_activation
                .expect("winner cleanup authority remains")
                .stage,
            CredentialActivationStage::CleanupPending
        );
        assert!(
            store.calls()[calls_before_recovery_mark..]
                .iter()
                .all(|call| !matches!(call, FakeStoreCall::CommitJournal))
        );
    }

    #[test]
    fn prepared_activation_rejects_a_foreign_authority_before_settings_or_entry_io() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(7));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("foreign-authority").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 7,
                proposed_settings_revision: 8,
                idempotency_token: idempotency("78787878-8989-4a4a-8b8b-454545454545"),
                settings_draft: settings_draft(),
            })
            .expect("prepare under the original authority");
        let calls_before_commit = store.calls().len();
        store.replace_authority_instance_id_for_test(
            CredentialAuthorityInstanceId::from_test_bytes([0x5a; 16]),
        );

        let error = service
            .commit_settings_activation(prepared)
            .expect_err("foreign authority cannot claim a prepared activation");

        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);
        assert!(settings.calls().is_empty());
        assert_eq!(settings.current_revision(), 7);
        assert_eq!(store.active_write_count(), 0);
        assert_eq!(store.staging_count(), 1);
        let commit_calls = &store.calls()[calls_before_commit..];
        assert!(!commit_calls.contains(&FakeStoreCall::ReadActiveInSession));
        assert!(!commit_calls.contains(&FakeStoreCall::ReplaceActive));
        assert!(!commit_calls.contains(&FakeStoreCall::ReadbackActive));
    }

    #[test]
    fn activation_transaction_and_renderer_safe_artifacts_hide_draft_and_authority_canaries() {
        const DRAFT_CANARY: &str = "SETTINGS_DRAFT_CANARY_NEVER_RENDER";
        const PATH_CANARY: &str = "/authority/path/canary";
        const PROVIDER_CANARY: &str = "AUTHORITY_PROVIDER_CANARY";
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let authority = CredentialAuthorityInstanceId::from_test_bytes([0xab; 16]);
        let store = Arc::new(FakeCredentialStore::with_authority(
            journal.clone(),
            authority.clone(),
        ));
        let settings = Arc::new(FakeSettingsActivationPort::new(7));
        let service = CredentialService::with_settings_activation_port(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings,
        );
        let draft = ValidatedNonSecretSettingsDraft::from_validated_bytes(
            format!("{DRAFT_CANARY}:{PATH_CANARY}:{PROVIDER_CANARY}").into_bytes(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("debug-secret-canary").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 7,
                proposed_settings_revision: 8,
                idempotency_token: idempotency("79797979-8989-4a4a-8b8b-454545454545"),
                settings_draft: draft.clone(),
            })
            .expect("prepare content-free debug artifacts");
        let status_json = serde_json::to_string(&service.snapshot_status()).expect("status JSON");
        let journal_json = serde_json::to_string(
            &*service
                .journal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
        .expect("journal JSON");
        let artifacts = [
            format!("{authority:?}"),
            format!("{draft:?}"),
            format!("{:?}", prepared.settings_identity()),
            format!("{:?}", prepared.transaction()),
            format!("{prepared:?}"),
            status_json.clone(),
            journal_json,
        ];

        assert_eq!(artifacts[0], "CredentialAuthorityInstanceId([OPAQUE])");
        assert_eq!(artifacts[1], "ValidatedNonSecretSettingsDraft([REDACTED])");
        assert_eq!(artifacts[2], "SettingsActivationIdentity([OPAQUE])");
        assert_eq!(artifacts[3], "SettingsActivationTransaction([REDACTED])");
        assert_eq!(artifacts[4], "PreparedCredentialActivation([REDACTED])");
        for artifact in artifacts {
            assert!(!artifact.contains(DRAFT_CANARY));
            assert!(!artifact.contains(PATH_CANARY));
            assert!(!artifact.contains(PROVIDER_CANARY));
        }
        assert!(!status_json.contains("settings_draft"));
        assert!(!status_json.contains("authority_instance_id"));
        assert!(!status_json.contains("settings_activation_transaction"));
    }

    #[test]
    fn pending_activation_reserves_only_its_target_set_for_resolution() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(5));
        let service = CredentialService::with_settings_activation_port(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings,
        );
        service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Openai),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("unrelated-openai").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("56565656-7878-9090-1212-343434343434"),
            })
            .expect("unrelated committed set");
        service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("deepgram-staging").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 5,
                proposed_settings_revision: 6,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("67676767-8989-1010-2323-454545454545"),
            })
            .expect("prepare Deepgram");

        let resolution = service
            .resolve_for_use(CredentialUseRequest {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Openai),
                consumer_id: "asr.api",
                auth_method_id: AuthMethodId::ApiKey,
                purpose: CredentialPurpose::Asr,
                audience: CredentialAudience::SecureNetworkOrigin {
                    scheme: SecureTransportScheme::Https,
                    canonical_host: "api.openai.com".to_string(),
                    effective_port: 443,
                },
            })
            .expect("unrelated committed authority remains resolvable");
        let CredentialResolution::Stored(lease) = resolution else {
            panic!("stored lease expected")
        };
        assert_eq!(
            lease.expose_api_key(str::to_owned),
            Some("unrelated-openai".to_string())
        );
    }

    #[test]
    fn pending_activation_reserves_its_global_successor_against_interleaving_commits() {
        let mut journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        journal.global_epoch = u64::MAX - 1;
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(31));
        let tokens = Arc::new(DeterministicTokenSource::default());
        let activation_service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            tokens.clone(),
            settings.clone(),
        );
        let prepared = activation_service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("reserved-final-generation")
                    .expect("API key"),
                expected_revision: None,
                expected_settings_revision: 31,
                proposed_settings_revision: 32,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("b5b5b5b5-c6c6-d7d7-e8e8-f9f9f9f9f9f9"),
            })
            .expect("prepare reserves MAX as its successor");

        let interleaver = CredentialService::new(store.clone(), store.journal_snapshot(), tokens);
        let before_status = activation_service.snapshot_status();
        let before_journal = store.journal_snapshot();
        let before_calls = store.calls().len();
        let before_active_writes = store.active_write_count();
        let before_staging = store.staging_count();
        let error = interleaver
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Openai),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("interleaving-final-epoch").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("c6c6c6c6-d7d7-e8e8-f9f9-a0a0a0a0a0a0"),
            })
            .expect_err("pending activation globally reserves the next epoch");

        assert_eq!(error.code, CredentialErrorCode::OperationInProgress);
        assert_eq!(activation_service.snapshot_status(), before_status);
        assert_eq!(store.journal_snapshot(), before_journal);
        assert_eq!(store.active_write_count(), before_active_writes);
        assert_eq!(store.staging_count(), before_staging);
        assert!(settings.calls().is_empty());
        assert!(store.calls()[before_calls..].iter().all(|call| !matches!(
            call,
            FakeStoreCall::PersistIntent
                | FakeStoreCall::ReplaceActive
                | FakeStoreCall::WriteStaging
                | FakeStoreCall::DeleteStaging
                | FakeStoreCall::CommitJournal
        )));

        let receipt = activation_service
            .commit_settings_activation(prepared)
            .expect("reserved activation publishes the final epoch");
        assert_eq!(receipt.result_code, CredentialMutationResultCode::Created);
        assert_eq!(activation_service.snapshot_status().global_epoch, u64::MAX);
        assert_eq!(store.journal_snapshot().global_epoch, u64::MAX);
        assert_eq!(store.staging_count(), 0);
        let final_batch = activation_service.events_since(u64::MAX - 1);
        assert!(!final_batch.gap_detected);
        assert_eq!(final_batch.events.len(), 1);
        assert_eq!(final_batch.events[0].global_epoch, u64::MAX);
        assert!(activation_service.events_since(u64::MAX).events.is_empty());
    }

    #[test]
    fn completed_replace_and_delete_replay_while_activation_holds_global_reservation() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(61));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let replay_set = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Openai);
        let replace_token = idempotency("f9f9f9f9-a0a0-b1b1-c2c2-d3d3d3d3d3d3");
        let replace = service
            .replace_set(ReplaceCredentialSet {
                set_id: replay_set.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("completed-replace").expect("API key"),
                expected_revision: None,
                idempotency_token: replace_token.clone(),
            })
            .expect("replace commits");
        let delete_token = idempotency("a0a0a0a0-b1b1-c2c2-d3d3-e4e4e4e4e4e4");
        let delete = service
            .delete_set(DeleteCredentialSet {
                set_id: replay_set.clone(),
                expected_revision: replace.new_revision.clone(),
                idempotency_token: delete_token.clone(),
            })
            .expect("delete commits");
        service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("reserved-activation").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 61,
                proposed_settings_revision: 62,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("b1b1b1b1-c2c2-d3d3-e4e4-f5f5f5f5f5f5"),
            })
            .expect("activation reserves epoch three");

        let before_status = service.snapshot_status();
        let before_journal = store.journal_snapshot();
        let before_calls = store.calls().len();
        let before_active_writes = store.active_write_count();
        let before_staging = store.staging_count();
        let replayed_replace = service
            .replace_set(ReplaceCredentialSet {
                set_id: replay_set.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("completed-replace").expect("API key"),
                expected_revision: None,
                idempotency_token: replace_token,
            })
            .expect("completed replace replays without claiming the reservation");
        let replayed_delete = service
            .delete_set(DeleteCredentialSet {
                set_id: replay_set,
                expected_revision: replace.new_revision.clone(),
                idempotency_token: delete_token,
            })
            .expect("completed delete replays without claiming the reservation");

        assert_eq!(replayed_replace, replace);
        assert_eq!(replayed_delete, delete);
        assert_eq!(service.snapshot_status(), before_status);
        assert_eq!(store.journal_snapshot(), before_journal);
        assert_eq!(store.active_write_count(), before_active_writes);
        assert_eq!(store.staging_count(), before_staging);
        assert!(settings.calls().is_empty());
        assert!(store.calls()[before_calls..].iter().all(|call| !matches!(
            call,
            FakeStoreCall::PersistIntent
                | FakeStoreCall::ReplaceActive
                | FakeStoreCall::WriteStaging
                | FakeStoreCall::DeleteStaging
                | FakeStoreCall::CommitJournal
        )));
        let no_new_event = service.events_since(before_status.global_epoch);
        assert!(!no_new_event.gap_detected);
        assert!(no_new_event.events.is_empty());
    }

    #[test]
    fn unresolved_pending_intent_blocks_activation_reservation_without_writes() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        store.fail_next(
            FakeStoreCall::ReplaceActive,
            CredentialStoreFailure::Unavailable,
        );
        let settings = Arc::new(FakeSettingsActivationPort::new(41));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("unresolved-intent").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("d7d7d7d7-e8e8-f9f9-a0a0-b1b1b1b1b1b1"),
            })
            .expect_err("scripted active write leaves a pending intent");
        assert_eq!(store.journal_snapshot().pending_intents.len(), 1);

        let before_status = service.snapshot_status();
        let before_journal = store.journal_snapshot();
        let before_calls = store.calls().len();
        let before_active_writes = store.active_write_count();
        let error = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Openai),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("must-not-reserve").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 41,
                proposed_settings_revision: 42,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("e8e8e8e8-f9f9-a0a0-b1b1-c2c2c2c2c2c2"),
            })
            .expect_err("unresolved intent must settle before activation reserves an epoch");

        assert_eq!(error.code, CredentialErrorCode::OperationInProgress);
        assert_eq!(service.snapshot_status(), before_status);
        assert_eq!(store.journal_snapshot(), before_journal);
        assert_eq!(store.active_write_count(), before_active_writes);
        assert_eq!(store.staging_count(), 0);
        assert!(settings.calls().is_empty());
        assert!(store.calls()[before_calls..].iter().all(|call| !matches!(
            call,
            FakeStoreCall::PersistIntent
                | FakeStoreCall::ReplaceActive
                | FakeStoreCall::WriteStaging
                | FakeStoreCall::DeleteStaging
                | FakeStoreCall::CommitJournal
        )));
    }

    #[test]
    fn prepared_activation_with_tampered_successor_is_rejected_write_free() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(71));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let mut prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("exact-successor").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 71,
                proposed_settings_revision: 72,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("c2c2c2c2-d3d3-e4e4-f5f5-a6a6a6a6a6a6"),
            })
            .expect("prepare exact successor");
        prepared.proposed_global_epoch = 2;

        let before_status = service.snapshot_status();
        let before_journal = store.journal_snapshot();
        let before_calls = store.calls().len();
        let before_active_writes = store.active_write_count();
        let before_staging = store.staging_count();
        let error = service
            .commit_settings_activation(prepared)
            .expect_err("tampered prepared successor cannot continue");

        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);
        assert_eq!(
            error.recovery_action,
            CredentialSafeRecoveryAction::Reconcile
        );
        assert_eq!(service.snapshot_status(), before_status);
        assert_eq!(store.journal_snapshot(), before_journal);
        assert_eq!(store.active_write_count(), before_active_writes);
        assert_eq!(store.staging_count(), before_staging);
        assert!(settings.calls().is_empty());
        assert!(store.calls()[before_calls..].iter().all(|call| !matches!(
            call,
            FakeStoreCall::PersistIntent
                | FakeStoreCall::ReplaceActive
                | FakeStoreCall::WriteStaging
                | FakeStoreCall::DeleteStaging
                | FakeStoreCall::CommitJournal
        )));
    }

    #[test]
    fn settings_failure_discards_staging_without_epoch_or_event() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(12));
        settings.fail_next(
            FakeSettingsCall::PersistPending,
            CredentialStoreFailure::Unavailable,
        );
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );

        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("never-active").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 12,
                proposed_settings_revision: 13,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("bbbbbbbb-cccc-dddd-eeee-ffffffffffff"),
            })
            .expect("prepare");
        assert_eq!(store.staging_count(), 1);

        let error = service
            .commit_settings_activation(prepared)
            .expect_err("settings persistence fails");

        assert_eq!(error.code, CredentialErrorCode::StoreUnavailable);
        assert_eq!(service.snapshot_status().global_epoch, 0);
        assert!(service.snapshot_status().pending_activation.is_none());
        assert_eq!(store.staging_count(), 0);
        assert_eq!(store.active_write_count(), 0);
        assert_eq!(settings.current_revision(), 12);
        assert!(!settings.has_pending_marker());
        assert!(service.events_since(0).events.is_empty());
    }

    #[test]
    fn every_active_readback_error_after_a_write_is_commit_unknown() {
        let failures = [
            CredentialStoreFailure::Missing,
            CredentialStoreFailure::Locked,
            CredentialStoreFailure::AccessDenied,
            CredentialStoreFailure::Cancelled,
            CredentialStoreFailure::Unavailable,
            CredentialStoreFailure::PermissionHardeningFailed,
            CredentialStoreFailure::Unsupported,
            CredentialStoreFailure::CorruptRecord,
            CredentialStoreFailure::UnsupportedSchema,
            CredentialStoreFailure::PayloadTooLarge,
            CredentialStoreFailure::AmbiguousMatch,
            CredentialStoreFailure::RevisionConflict,
            CredentialStoreFailure::OperationInProgress,
            CredentialStoreFailure::StalledWorker,
            CredentialStoreFailure::CommitUnknown,
            CredentialStoreFailure::Internal,
        ];

        for (index, failure) in failures.into_iter().enumerate() {
            let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
            let store = Arc::new(FakeCredentialStore::new(journal.clone()));
            let service = CredentialService::new(
                store.clone(),
                journal,
                Arc::new(DeterministicTokenSource::default()),
            );
            store.fail_next(FakeStoreCall::ReadbackActive, failure);

            let error = service
                .replace_set(ReplaceCredentialSet {
                    set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                    auth_method_id: AuthMethodId::ApiKey,
                    material: StoredSecretBundle::api_key(format!("readback-cut-{index}"))
                        .expect("API key"),
                    expected_revision: None,
                    idempotency_token: idempotency(&format!(
                        "a2a2a2a2-b3b3-c4c4-d5d5-{suffix:012x}",
                        suffix = index + 1
                    )),
                })
                .expect_err("post-write readback failure is ambiguous");

            assert_eq!(error.code, CredentialErrorCode::CommitUnknown);
            assert_eq!(store.active_write_count(), 1);
            assert_eq!(store.journal_snapshot().pending_intents.len(), 1);
            assert_eq!(service.snapshot_status().global_epoch, 0);
            assert!(service.events_since(0).events.is_empty());
        }
    }

    #[test]
    fn uncertain_settings_write_retains_recovery_gate_and_staging_until_reconciled() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(14));
        settings.fail_after_effect_next(
            FakeSettingsCall::PersistPending,
            CredentialStoreFailure::CommitUnknown,
        );
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("uncertain-settings").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 14,
                proposed_settings_revision: 15,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("71717171-8282-9393-a4a4-b5b5b5b5b5b5"),
            })
            .expect("prepare");
        let operation_id = prepared.operation_id.clone();

        let error = service
            .commit_settings_activation(prepared)
            .expect_err("settings write outcome is uncertain");

        assert_eq!(error.code, CredentialErrorCode::CommitUnknown);
        assert_eq!(settings.current_revision(), 15);
        assert!(settings.has_pending_marker());
        assert_eq!(store.staging_count(), 1);
        assert_eq!(
            service
                .snapshot_status()
                .pending_activation
                .expect("uncertain settings remain gated")
                .stage,
            CredentialActivationStage::RecoveryRequired
        );
        assert_eq!(
            service
                .resolve_for_use(deepgram_use_request())
                .expect_err("uncertain settings cannot activate a credential")
                .code,
            CredentialErrorCode::RecoveryRequired
        );
        assert!(service.events_since(0).events.is_empty());

        let receipt = service
            .recover_settings_activation(&operation_id)
            .expect("recovery rolls back the revision-fenced settings marker");
        assert_eq!(receipt.result_code, CredentialMutationResultCode::NoChange);
        assert_eq!(settings.current_revision(), 14);
        assert!(!settings.has_pending_marker());
        assert_eq!(store.staging_count(), 0);
        assert!(service.snapshot_status().pending_activation.is_none());
    }

    #[test]
    fn definite_active_write_failure_restores_settings_and_discards_activation() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(20));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("definite-failure").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 20,
                proposed_settings_revision: 21,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("cccccccc-dddd-eeee-ffff-000000000001"),
            })
            .expect("prepare");
        store.fail_next(
            FakeStoreCall::ReplaceActive,
            CredentialStoreFailure::AccessDenied,
        );

        let error = service
            .commit_settings_activation(prepared)
            .expect_err("definite active write failure");

        assert_eq!(error.code, CredentialErrorCode::AccessDenied);
        assert_eq!(settings.current_revision(), 20);
        assert!(!settings.has_pending_marker());
        assert_eq!(store.staging_count(), 0);
        assert_eq!(store.active_write_count(), 0);
        assert_eq!(service.snapshot_status().global_epoch, 0);
        assert!(service.snapshot_status().pending_activation.is_none());
        assert!(service.events_since(0).events.is_empty());
        assert_eq!(
            settings.calls(),
            vec![
                FakeSettingsCall::PersistPending,
                FakeSettingsCall::VerifyPending,
                FakeSettingsCall::RestoreBackup,
            ]
        );
    }

    #[test]
    fn failed_revision_fenced_rollback_preserves_recovery_and_staging() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(24));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("rollback-required").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 24,
                proposed_settings_revision: 25,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("81818181-9292-a3a3-b4b4-c5c5c5c5c5c5"),
            })
            .expect("prepare");
        let operation_id = prepared.operation_id.clone();
        store.fail_next(
            FakeStoreCall::ReplaceActive,
            CredentialStoreFailure::AccessDenied,
        );
        settings.fail_next(
            FakeSettingsCall::RestoreBackup,
            CredentialStoreFailure::Unavailable,
        );

        let error = service
            .commit_settings_activation(prepared)
            .expect_err("failed rollback cannot be reported as a definite abort");

        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);
        assert_eq!(settings.current_revision(), 25);
        assert!(settings.has_pending_marker());
        assert_eq!(store.staging_count(), 1);
        assert_eq!(store.active_write_count(), 0);
        assert_eq!(
            service
                .snapshot_status()
                .pending_activation
                .expect("rollback failure remains durable")
                .stage,
            CredentialActivationStage::RecoveryRequired
        );
        assert!(service.events_since(0).events.is_empty());

        let receipt = service
            .recover_settings_activation(&operation_id)
            .expect("retry can complete the rollback");
        assert_eq!(receipt.result_code, CredentialMutationResultCode::NoChange);
        assert_eq!(settings.current_revision(), 24);
        assert!(!settings.has_pending_marker());
        assert_eq!(store.staging_count(), 0);
        assert!(service.snapshot_status().pending_activation.is_none());
    }

    #[test]
    fn uncertain_activation_commit_gates_then_recovers_idempotently() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(30));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("recover-generation").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 30,
                proposed_settings_revision: 31,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("ffffffff-0000-1111-2222-000000000004"),
            })
            .expect("prepare");
        let operation_id = prepared.operation_id.clone();
        store.fail_after(
            FakeStoreCall::CommitJournal,
            2,
            CredentialStoreFailure::Unavailable,
        );

        let uncertain = service
            .commit_settings_activation(prepared)
            .expect_err("final journal commit is uncertain");
        assert_eq!(uncertain.code, CredentialErrorCode::CommitUnknown);
        assert_eq!(service.snapshot_status().global_epoch, 0);
        assert_eq!(
            service
                .snapshot_status()
                .pending_activation
                .expect("recovery gate")
                .stage,
            CredentialActivationStage::RecoveryRequired
        );
        assert!(settings.has_pending_marker());
        assert!(service.events_since(0).events.is_empty());
        assert_eq!(
            service
                .resolve_for_use(deepgram_use_request())
                .expect_err("uncommitted active generation stays gated")
                .code,
            CredentialErrorCode::RecoveryRequired
        );

        let recovered = service
            .recover_settings_activation(&operation_id)
            .expect("active operation identity establishes commit");
        assert_eq!(
            recovered.result_code,
            CredentialMutationResultCode::Recovered
        );
        assert_eq!(service.snapshot_status().global_epoch, 1);
        assert!(service.snapshot_status().pending_activation.is_none());
        assert!(!settings.has_pending_marker());
        assert_eq!(service.events_since(0).events.len(), 1);

        let replay = service
            .recover_settings_activation(&operation_id)
            .expect("completed recovery replay");
        assert_eq!(replay, recovered);
        assert_eq!(service.snapshot_status().global_epoch, 1);
        assert_eq!(service.events_since(0).events.len(), 1);
    }

    #[test]
    fn activation_readback_error_is_commit_unknown_and_retains_recovery() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(32));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("readback-unknown").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 32,
                proposed_settings_revision: 33,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("b2b2b2b2-c3c3-d4d4-e5e5-f6f6f6f6f6f6"),
            })
            .expect("prepare");
        let operation_id = prepared.operation_id.clone();
        store.fail_next(
            FakeStoreCall::ReadbackActive,
            CredentialStoreFailure::Unavailable,
        );

        let error = service
            .commit_settings_activation(prepared)
            .expect_err("active readback cannot establish the write");

        assert_eq!(error.code, CredentialErrorCode::CommitUnknown);
        assert_eq!(store.active_write_count(), 1);
        assert_eq!(store.staging_count(), 1);
        assert!(settings.has_pending_marker());
        assert_eq!(service.snapshot_status().global_epoch, 0);
        assert_eq!(
            service
                .snapshot_status()
                .pending_activation
                .expect("ambiguous active write remains recoverable")
                .stage,
            CredentialActivationStage::RecoveryRequired
        );
        assert!(service.events_since(0).events.is_empty());

        let receipt = service
            .recover_settings_activation(&operation_id)
            .expect("readback recovery verifies the active operation identity");
        assert_eq!(receipt.result_code, CredentialMutationResultCode::Recovered);
        assert_eq!(service.snapshot_status().global_epoch, 1);
        assert!(!settings.has_pending_marker());
        assert_eq!(store.staging_count(), 0);
        assert_eq!(service.events_since(0).events.len(), 1);
    }

    #[test]
    fn recovery_rejects_a_wrong_set_proposed_record_even_when_tokens_match() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(35));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("proposed-generation").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 35,
                proposed_settings_revision: 36,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("01010101-2323-4545-6767-898989898989"),
            })
            .expect("prepare");
        store.fail_after(
            FakeStoreCall::CommitJournal,
            2,
            CredentialStoreFailure::Unavailable,
        );
        let error = service
            .commit_settings_activation(prepared.clone())
            .expect_err("active commit remains uncertain");
        assert_eq!(error.code, CredentialErrorCode::CommitUnknown);

        let wrong_set_record = CredentialRecordEnvelope::present(
            CredentialSetId::BuiltIn(BuiltInCredentialSetId::Openai),
            AuthMethodId::ApiKey,
            prepared.proposed_revision.clone(),
            prepared.operation_id.clone(),
            StoredSecretBundle::api_key("wrong-set-generation").expect("API key"),
        )
        .expect("valid record for the wrong set")
        .encode()
        .expect("encode");
        {
            let mut session = store
                .begin_mutation(&prepared.operation_id, &prepared.set_id)
                .expect("tamper session");
            session
                .replace_active(&prepared.set_id, wrong_set_record)
                .expect("inject wrong-set envelope under target key");
        }

        let recovery = service
            .recover_settings_activation(&prepared.operation_id)
            .expect_err("matching random tokens are not structural authority");

        assert_eq!(recovery.code, CredentialErrorCode::RecoveryRequired);
        assert_eq!(service.snapshot_status().global_epoch, 0);
        assert!(service.snapshot_status().pending_activation.is_some());
        assert!(settings.has_pending_marker());
        assert!(service.events_since(0).events.is_empty());
    }

    #[test]
    fn recovery_rejects_a_wrong_state_expected_record_before_rollback() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(37));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let initial = service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("expected-generation").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("12121212-3434-5656-7878-909090909090"),
            })
            .expect("initial generation");
        let expected_revision = initial.new_revision.clone().expect("active revision");
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("next-generation").expect("API key"),
                expected_revision: Some(expected_revision.clone()),
                expected_settings_revision: 37,
                proposed_settings_revision: 38,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("23232323-4545-6767-8989-010101010101"),
            })
            .expect("prepare");
        store.fail_next(
            FakeStoreCall::ReplaceActive,
            CredentialStoreFailure::CommitUnknown,
        );
        let error = service
            .commit_settings_activation(prepared.clone())
            .expect_err("active replacement remains uncertain");
        assert_eq!(error.code, CredentialErrorCode::CommitUnknown);

        let wrong_state_record = CredentialRecordEnvelope::tombstone(
            prepared.set_id.clone(),
            expected_revision,
            prepared.operation_id.clone(),
        )
        .encode()
        .expect("encode");
        {
            let mut session = store
                .begin_mutation(&prepared.operation_id, &prepared.set_id)
                .expect("tamper session");
            session
                .replace_active(&prepared.set_id, wrong_state_record)
                .expect("inject a tombstone where configured state is expected");
        }

        let recovery = service
            .recover_settings_activation(&prepared.operation_id)
            .expect_err("record state must agree with journal authority before rollback");

        assert_eq!(recovery.code, CredentialErrorCode::RecoveryRequired);
        assert_eq!(service.snapshot_status().global_epoch, 1);
        assert!(service.snapshot_status().pending_activation.is_some());
        assert!(settings.has_pending_marker());
        assert!(service.events_since(1).events.is_empty());
    }

    #[test]
    fn ambiguous_final_activation_cleanup_recovers_terminal_authority_without_writes() {
        let mut journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        journal.global_epoch = u64::MAX - 1;
        let store = Arc::new(AfterEffectCommitUnknownStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(39));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let token = idempotency("c7c7c7c7-d8d8-e9e9-f0f0-a1a1a1a1a1a1");
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("ambiguous-terminal-activation")
                    .expect("API key"),
                expected_revision: None,
                expected_settings_revision: 39,
                proposed_settings_revision: 40,
                settings_draft: settings_draft(),
                idempotency_token: token.clone(),
            })
            .expect("prepare terminal activation reservation");
        let operation_id = prepared.operation_id.clone();
        store.fail_commit_after_effect_after(3);

        let error = service
            .commit_settings_activation(prepared)
            .expect_err("final cleanup journal readback is ambiguous");

        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);
        assert_eq!(service.snapshot_status().global_epoch, u64::MAX - 1);
        assert_eq!(
            service
                .snapshot_status()
                .pending_activation
                .expect("cached cleanup gate")
                .stage,
            CredentialActivationStage::CleanupPending
        );
        let authoritative = store.journal_snapshot();
        assert_eq!(authoritative.global_epoch, u64::MAX);
        assert!(authoritative.pending_activation.is_none());
        let persisted_receipt = authoritative
            .idempotency_entry(&token)
            .expect("committed activation history")
            .receipt
            .clone();
        let calls_before_recovery = store.calls().len();
        let writes_before_recovery = store.active_write_count();
        let staging_before_recovery = store.staging_count();
        let settings_calls_before_recovery = settings.calls();

        let recovered = service
            .recover_settings_activation(&operation_id)
            .expect("authoritative completion is recovered in-process");

        assert_eq!(recovered, persisted_receipt);
        assert_eq!(service.snapshot_status().global_epoch, u64::MAX);
        assert!(service.snapshot_status().pending_activation.is_none());
        assert_eq!(store.active_write_count(), writes_before_recovery);
        assert_eq!(store.staging_count(), staging_before_recovery);
        assert_eq!(settings.calls(), settings_calls_before_recovery);
        assert!(
            store.calls()[calls_before_recovery..]
                .iter()
                .all(|call| !matches!(
                    call,
                    FakeStoreCall::PersistIntent
                        | FakeStoreCall::ReplaceActive
                        | FakeStoreCall::WriteStaging
                        | FakeStoreCall::DeleteStaging
                        | FakeStoreCall::CommitJournal
                ))
        );
        let missed = service.events_since(u64::MAX - 1);
        assert_eq!(missed.latest_epoch, u64::MAX);
        assert!(missed.gap_detected);
        assert!(missed.events.is_empty());
        let terminal = service.events_since(u64::MAX);
        assert_eq!(terminal.latest_epoch, u64::MAX);
        assert!(!terminal.gap_detected);
        assert!(terminal.events.is_empty());
    }

    #[test]
    fn cleanup_marker_failure_persists_a_restart_safe_gate_without_success_event() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(40));
        settings.fail_next(
            FakeSettingsCall::ClearPending,
            CredentialStoreFailure::Unavailable,
        );
        let service = CredentialService::with_settings_activation_port(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("cleanup-marker").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 40,
                proposed_settings_revision: 41,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("abababab-cdcd-efef-0101-000000000005"),
            })
            .expect("prepare");
        let operation_id = prepared.operation_id.clone();

        let error = service
            .commit_settings_activation(prepared)
            .expect_err("cleanup marker failure is not success");
        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);
        let status = service.snapshot_status();
        assert_eq!(status.global_epoch, 0);
        let cleanup_snapshot_epoch = status.global_epoch;
        assert_eq!(
            status.pending_activation.expect("cleanup gate").stage,
            CredentialActivationStage::CleanupPending
        );
        assert!(settings.has_pending_marker());
        assert!(service.events_since(0).events.is_empty());
        assert_eq!(
            service
                .resolve_for_use(deepgram_use_request())
                .expect_err("cleanup gate blocks new lease")
                .code,
            CredentialErrorCode::RecoveryRequired
        );

        let recovered = service
            .recover_settings_activation(&operation_id)
            .expect("cleanup retry");
        assert_eq!(
            recovered.result_code,
            CredentialMutationResultCode::Recovered
        );
        assert!(!settings.has_pending_marker());
        assert!(service.snapshot_status().pending_activation.is_none());
        assert_eq!(service.snapshot_status().global_epoch, 1);
        let committed = service.events_since(cleanup_snapshot_epoch);
        assert!(!committed.gap_detected);
        assert_eq!(committed.events.len(), 1);
        assert_eq!(committed.events[0].global_epoch, cleanup_snapshot_epoch + 1);
    }

    #[test]
    fn cleanup_pending_revalidates_the_active_pair_before_event_publication() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(44));
        settings.fail_next(
            FakeSettingsCall::ClearPending,
            CredentialStoreFailure::Unavailable,
        );
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("cleanup-authority").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 44,
                proposed_settings_revision: 45,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("91919191-a2a2-b3b3-c4c4-d5d5d5d5d5d5"),
            })
            .expect("prepare");
        let error = service
            .commit_settings_activation(prepared.clone())
            .expect_err("cleanup marker failure leaves CleanupPending");
        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);

        let wrong_set = CredentialRecordEnvelope::present(
            CredentialSetId::BuiltIn(BuiltInCredentialSetId::Openai),
            AuthMethodId::ApiKey,
            prepared.proposed_revision.clone(),
            prepared.operation_id.clone(),
            StoredSecretBundle::api_key("wrong-cleanup-set").expect("API key"),
        )
        .expect("wrong-set envelope is structurally valid for its declared set")
        .encode()
        .expect("encode");
        {
            let mut session = store
                .begin_mutation(&prepared.operation_id, &prepared.set_id)
                .expect("external-editor simulation");
            session
                .replace_active(&prepared.set_id, wrong_set)
                .expect("replace target bytes outside service authority");
        }

        let recovery = service
            .recover_settings_activation(&prepared.operation_id)
            .expect_err("cleanup cannot bless a mismatched active envelope");

        assert_eq!(recovery.code, CredentialErrorCode::RecoveryRequired);
        assert_eq!(service.snapshot_status().global_epoch, 0);
        assert_eq!(
            service
                .snapshot_status()
                .pending_activation
                .expect("cleanup gate remains")
                .stage,
            CredentialActivationStage::CleanupPending
        );
        assert!(settings.has_pending_marker());
        assert!(service.events_since(0).events.is_empty());
    }

    #[test]
    fn cleanup_pending_rejects_a_rolled_back_settings_revision() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(46));
        settings.fail_next(
            FakeSettingsCall::ClearPending,
            CredentialStoreFailure::Unavailable,
        );
        let service = CredentialService::with_settings_activation_port(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("settings-fence").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 46,
                proposed_settings_revision: 47,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("a1a1a1a1-b2b2-c3c3-d4d4-e5e5e5e5e5e5"),
            })
            .expect("prepare");
        service
            .commit_settings_activation(prepared.clone())
            .expect_err("cleanup marker failure leaves CleanupPending");
        settings
            .restore_settings_backup(prepared.settings_identity())
            .expect("simulate settings rollback outside credential cleanup");

        let recovery = service
            .recover_settings_activation(&prepared.operation_id)
            .expect_err("cleanup requires the proposed settings revision");

        assert_eq!(recovery.code, CredentialErrorCode::RecoveryRequired);
        assert_eq!(service.snapshot_status().global_epoch, 0);
        assert!(service.snapshot_status().pending_activation.is_some());
        assert!(service.events_since(0).events.is_empty());
    }

    #[test]
    fn staging_cleanup_failure_is_recoverable_after_settings_marker_clears() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let settings = Arc::new(FakeSettingsActivationPort::new(50));
        let service = CredentialService::with_settings_activation_port(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
            settings.clone(),
        );
        let prepared = service
            .prepare_settings_activation(PrepareCredentialActivation {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("cleanup-staging").expect("API key"),
                expected_revision: None,
                expected_settings_revision: 50,
                proposed_settings_revision: 51,
                settings_draft: settings_draft(),
                idempotency_token: idempotency("bcbcbcbc-dede-fafa-0202-000000000006"),
            })
            .expect("prepare");
        let operation_id = prepared.operation_id.clone();
        store.fail_next(
            FakeStoreCall::DeleteStaging,
            CredentialStoreFailure::Unavailable,
        );

        let error = service
            .commit_settings_activation(prepared)
            .expect_err("staging cleanup remains gated");
        assert_eq!(error.code, CredentialErrorCode::RecoveryRequired);
        assert!(!settings.has_pending_marker());
        assert_eq!(store.staging_count(), 1);
        assert!(service.events_since(0).events.is_empty());
        let status = service.snapshot_status();
        assert_eq!(status.global_epoch, 0);
        let cleanup_snapshot_epoch = status.global_epoch;
        assert_eq!(
            status.pending_activation.expect("cleanup gate").stage,
            CredentialActivationStage::CleanupPending
        );

        let recovered = service
            .recover_settings_activation(&operation_id)
            .expect("idempotent settings clear plus staging retry");
        assert_eq!(
            recovered.result_code,
            CredentialMutationResultCode::Recovered
        );
        assert_eq!(store.staging_count(), 0);
        let committed = service.events_since(cleanup_snapshot_epoch);
        assert!(!committed.gap_detected);
        assert_eq!(committed.events.len(), 1);
        assert_eq!(committed.events[0].global_epoch, cleanup_snapshot_epoch + 1);
    }

    #[test]
    fn journal_commit_failure_is_commit_unknown_and_never_publishes() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        store.fail_next(
            FakeStoreCall::CommitJournal,
            CredentialStoreFailure::Unavailable,
        );
        let service = CredentialService::new(
            store,
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );

        let error = service
            .replace_set(ReplaceCredentialSet {
                set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key("indeterminate-commit").expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("dddddddd-eeee-ffff-0000-000000000002"),
            })
            .expect_err("journal commit cannot be established");

        assert_eq!(error.code, CredentialErrorCode::CommitUnknown);
        assert_eq!(service.snapshot_status().global_epoch, 0);
        assert!(service.events_since(0).events.is_empty());
        assert_eq!(
            service
                .resolve_for_use(deepgram_use_request())
                .expect_err("active record without journal authority is gated")
                .code,
            CredentialErrorCode::RecoveryRequired
        );
    }

    #[test]
    fn deterministic_mutation_trace_preserves_cas_epoch_and_idempotency_properties() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let mut current_revision = None;

        for step in 0_u64..140 {
            let expected_revision = current_revision.clone();
            let token = idempotency(&format!(
                "10000000-2000-3000-4000-{value:012x}",
                value = step + 1
            ));
            let receipt = if step % 2 == 0 {
                service
                    .replace_set(ReplaceCredentialSet {
                        set_id: set_id.clone(),
                        auth_method_id: AuthMethodId::ApiKey,
                        material: StoredSecretBundle::api_key(format!("trace-{step}"))
                            .expect("API key"),
                        expected_revision: expected_revision.clone(),
                        idempotency_token: token.clone(),
                    })
                    .expect("model replace")
            } else {
                service
                    .delete_set(DeleteCredentialSet {
                        set_id: set_id.clone(),
                        expected_revision: expected_revision.clone(),
                        idempotency_token: token.clone(),
                    })
                    .expect("model delete")
            };
            current_revision = receipt.new_revision.clone();
            let status = service.snapshot_status();
            assert_eq!(status.global_epoch, step + 1);
            let set = status
                .sets
                .iter()
                .find(|candidate| candidate.set_id == set_id)
                .expect("set status");
            assert_eq!(set.revision, current_revision);
            assert_eq!(
                set.record_state,
                if step % 2 == 0 {
                    CredentialSetRecordState::Configured
                } else {
                    CredentialSetRecordState::Tombstoned
                }
            );

            let replay = if step % 2 == 0 {
                service
                    .replace_set(ReplaceCredentialSet {
                        set_id: set_id.clone(),
                        auth_method_id: AuthMethodId::ApiKey,
                        material: StoredSecretBundle::api_key("ignored-replay").expect("API key"),
                        expected_revision: expected_revision.clone(),
                        idempotency_token: token,
                    })
                    .expect("replace replay")
            } else {
                service
                    .delete_set(DeleteCredentialSet {
                        set_id: set_id.clone(),
                        expected_revision: expected_revision.clone(),
                        idempotency_token: token,
                    })
                    .expect("delete replay")
            };
            assert_eq!(replay, receipt);
            assert_eq!(service.snapshot_status().global_epoch, step + 1);

            if step % 7 == 0 {
                let stale = service
                    .replace_set(ReplaceCredentialSet {
                        set_id: set_id.clone(),
                        auth_method_id: AuthMethodId::ApiKey,
                        material: StoredSecretBundle::api_key("stale-cas").expect("API key"),
                        expected_revision: None,
                        idempotency_token: idempotency(&format!(
                            "50000000-6000-7000-8000-{value:012x}",
                            value = step + 1
                        )),
                    })
                    .expect_err("stale expected revision");
                assert_eq!(stale.code, CredentialErrorCode::RevisionConflict);
                assert_eq!(service.snapshot_status().global_epoch, step + 1);
            }
        }

        assert_eq!(store.active_write_count(), 140);
        assert_eq!(service.snapshot_status().global_epoch, 140);
        assert_eq!(
            service
                .journal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .idempotency_history
                .len(),
            128
        );
        let gap = service.events_since(0);
        assert!(gap.gap_detected);
        assert_eq!(gap.latest_epoch, 140);
    }

    #[test]
    fn secret_canary_never_appears_in_safe_service_artifacts() {
        let canary = "A6BF_SERVICE_SECRET_CANARY";
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let service = CredentialService::new(
            store.clone(),
            journal,
            Arc::new(DeterministicTokenSource::default()),
        );
        let set_id = CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram);
        let receipt = service
            .replace_set(ReplaceCredentialSet {
                set_id: set_id.clone(),
                auth_method_id: AuthMethodId::ApiKey,
                material: StoredSecretBundle::api_key(canary).expect("API key"),
                expected_revision: None,
                idempotency_token: idempotency("eeeeeeee-ffff-0000-1111-000000000003"),
            })
            .expect("save canary");
        let resolution = service
            .resolve_for_use(CredentialUseRequest {
                set_id,
                consumer_id: "asr.deepgram",
                auth_method_id: AuthMethodId::ApiKey,
                purpose: CredentialPurpose::Asr,
                audience: CredentialAudience::SecureNetworkOrigin {
                    scheme: SecureTransportScheme::Wss,
                    canonical_host: "api.deepgram.com".to_string(),
                    effective_port: 443,
                },
            })
            .expect("resolve");
        let CredentialResolution::Stored(lease) = resolution else {
            panic!("stored lease expected")
        };
        assert_eq!(
            lease.expose_api_key(str::to_owned),
            Some(canary.to_string())
        );

        let status_json = serde_json::to_string(&service.snapshot_status()).expect("status JSON");
        let receipt_json = serde_json::to_string(&receipt).expect("receipt JSON");
        let journal_json = serde_json::to_string(
            &*service
                .journal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
        .expect("journal JSON");
        let artifacts = [
            status_json,
            receipt_json,
            journal_json,
            format!("{lease:?}"),
            format!("{:?}", service.events_since(0)),
            format!("{:?}", store.calls()),
        ];
        for artifact in artifacts {
            assert!(!artifact.contains(canary));
            assert!(!artifact.contains("secret_length"));
            assert!(!artifact.contains("fingerprint"));
        }
    }
}
