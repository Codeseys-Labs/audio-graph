use super::domain::{
    AuthorityJournal, CredentialMutationKind, CredentialRecordEnvelope, CredentialRecordPayload,
    CredentialStoreFailure, EncodedCredentialRecord, IdempotencyJournalEntry,
    PendingCredentialIntent, PendingSettingsActivation, StoredSecretBundle, content_free_error,
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
    fn load_journal(&mut self) -> Result<AuthorityJournal, CredentialStoreFailure>;
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

pub(crate) trait CredentialTokenSource: Send + Sync {
    fn next_operation_id(&self) -> CredentialOperationId;
    fn next_revision(&self) -> CredentialRevision;
}

pub(crate) trait CredentialSettingsActivationPort: Send + Sync {
    fn persist_pending_settings(
        &self,
        operation_id: &CredentialOperationId,
        set_id: &CredentialSetId,
        expected_revision: u64,
        proposed_revision: u64,
    ) -> Result<(), CredentialStoreFailure>;

    fn verify_pending_settings(
        &self,
        operation_id: &CredentialOperationId,
        expected_revision: u64,
    ) -> Result<(), CredentialStoreFailure>;

    fn verify_committed_settings(
        &self,
        operation_id: &CredentialOperationId,
        expected_revision: u64,
    ) -> Result<(), CredentialStoreFailure>;

    fn restore_settings_backup(
        &self,
        operation_id: &CredentialOperationId,
        expected_revision: u64,
    ) -> Result<(), CredentialStoreFailure>;

    fn clear_pending_settings(
        &self,
        operation_id: &CredentialOperationId,
        committed_revision: u64,
    ) -> Result<(), CredentialStoreFailure>;
}

struct UnsupportedSettingsActivationPort;

impl CredentialSettingsActivationPort for UnsupportedSettingsActivationPort {
    fn persist_pending_settings(
        &self,
        _operation_id: &CredentialOperationId,
        _set_id: &CredentialSetId,
        _expected_revision: u64,
        _proposed_revision: u64,
    ) -> Result<(), CredentialStoreFailure> {
        Err(CredentialStoreFailure::Unsupported)
    }

    fn verify_pending_settings(
        &self,
        _operation_id: &CredentialOperationId,
        _expected_revision: u64,
    ) -> Result<(), CredentialStoreFailure> {
        Err(CredentialStoreFailure::Unsupported)
    }

    fn verify_committed_settings(
        &self,
        _operation_id: &CredentialOperationId,
        _expected_revision: u64,
    ) -> Result<(), CredentialStoreFailure> {
        Err(CredentialStoreFailure::Unsupported)
    }

    fn restore_settings_backup(
        &self,
        _operation_id: &CredentialOperationId,
        _expected_revision: u64,
    ) -> Result<(), CredentialStoreFailure> {
        Err(CredentialStoreFailure::Unsupported)
    }

    fn clear_pending_settings(
        &self,
        _operation_id: &CredentialOperationId,
        _committed_revision: u64,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedCredentialActivation {
    operation_id: CredentialOperationId,
    idempotency_token: CredentialIdempotencyToken,
    set_id: CredentialSetId,
    auth_method_id: AuthMethodId,
    expected_revision: Option<CredentialRevision>,
    proposed_revision: CredentialRevision,
    expected_settings_revision: u64,
    proposed_settings_revision: u64,
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
        let first_retained_epoch = self
            .events
            .front()
            .map_or(latest_epoch.saturating_add(1), |event| event.global_epoch);
        let last_retained_epoch = self
            .events
            .back()
            .map_or(self.service_start_epoch, |event| event.global_epoch);
        let gap_detected = after_epoch < self.service_start_epoch
            || after_epoch.saturating_add(1) < first_retained_epoch
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
}

#[cfg(test)]
struct EventSnapshotTestHook {
    snapshot_captured: Sender<()>,
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

    fn map_store_failure(
        &self,
        failure: CredentialStoreFailure,
        set_id: &CredentialSetId,
    ) -> CredentialError {
        if failure == CredentialStoreFailure::StalledWorker {
            self.worker.mark_stalled();
        }
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
        if journal
            .pending_intents
            .iter()
            .any(|intent| &intent.set_id == set_id)
            || journal
                .pending_activation
                .as_ref()
                .is_some_and(|pending| &pending.set_id == set_id)
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

    fn cache_journal(&self, journal: &AuthorityJournal) {
        *self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = journal.clone();
    }

    fn active_readback_is_commit_unknown(
        &self,
        failure: CredentialStoreFailure,
        set_id: &CredentialSetId,
    ) -> CredentialError {
        if failure == CredentialStoreFailure::StalledWorker {
            self.worker.mark_stalled();
        }
        CredentialStoreFailure::CommitUnknown.into_public(Some(set_id.clone()))
    }

    fn proposed_activation_record_matches(
        record: &CredentialRecordEnvelope,
        prepared: &PreparedCredentialActivation,
    ) -> bool {
        record.set_id == prepared.set_id
            && record.revision == prepared.proposed_revision
            && record.operation_id == prepared.operation_id
            && matches!(
                &record.payload,
                CredentialRecordPayload::Present { auth_method_id, .. }
                    if *auth_method_id == prepared.auth_method_id
            )
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
            let mut journal = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;

            Self::ensure_target_has_no_pending_mutation(&journal, &request.set_id)?;

            if let Some(previous) = journal.idempotency_entry(&request.idempotency_token) {
                if previous.set_id == request.set_id
                    && previous.mutation_kind == CredentialMutationKind::Replace
                    && previous.expected_revision == request.expected_revision
                {
                    return Ok(previous.receipt.clone());
                }
                return Err(content_free_error(
                    CredentialErrorCode::Conflict,
                    CredentialSafeRecoveryAction::Retry,
                    Some(request.set_id),
                ));
            }

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
            if readback.as_ref().map(EncodedCredentialRecord::as_bytes)
                != Some(expected_readback.as_bytes())
            {
                return Err(CredentialStoreFailure::CommitUnknown.into_public(Some(request.set_id)));
            }

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
            journal.global_epoch = journal.global_epoch.saturating_add(1);
            session.commit_journal(&journal).map_err(|_| {
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
            let mut journal = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;

            Self::ensure_target_has_no_pending_mutation(&journal, &request.set_id)?;

            if let Some(previous) = journal.idempotency_entry(&request.idempotency_token) {
                if previous.set_id == request.set_id
                    && previous.mutation_kind == CredentialMutationKind::Delete
                    && previous.expected_revision == request.expected_revision
                {
                    return Ok(previous.receipt.clone());
                }
                return Err(content_free_error(
                    CredentialErrorCode::Conflict,
                    CredentialSafeRecoveryAction::Retry,
                    Some(request.set_id),
                ));
            }

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
            if readback.as_ref().map(EncodedCredentialRecord::as_bytes)
                != Some(expected_readback.as_bytes())
            {
                return Err(CredentialStoreFailure::CommitUnknown.into_public(Some(request.set_id)));
            }

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
            journal.global_epoch = journal.global_epoch.saturating_add(1);
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
            session.commit_journal(&journal).map_err(|_| {
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

        let committed_journal = {
            let mut session = self
                .store
                .begin_mutation(&operation_id, &request.set_id)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            let mut journal = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            if journal.pending_activation.is_some() {
                return Err(
                    CredentialStoreFailure::OperationInProgress.into_public(Some(request.set_id))
                );
            }
            Self::ensure_target_has_no_pending_mutation(&journal, &request.set_id)?;

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
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            if readback.as_ref().map(EncodedCredentialRecord::as_bytes)
                != Some(expected_staging.as_bytes())
            {
                return Err(CredentialStoreFailure::CommitUnknown.into_public(Some(request.set_id)));
            }
            session
                .commit_journal(&journal)
                .map_err(|failure| self.map_store_failure(failure, &request.set_id))?;
            journal
        };

        *self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = committed_journal;
        Ok(PreparedCredentialActivation {
            operation_id,
            idempotency_token: request.idempotency_token,
            set_id: request.set_id,
            auth_method_id: request.auth_method_id,
            expected_revision: request.expected_revision,
            proposed_revision,
            expected_settings_revision: request.expected_settings_revision,
            proposed_settings_revision: request.proposed_settings_revision,
        })
    }

    pub(crate) fn commit_settings_activation(
        &self,
        prepared: PreparedCredentialActivation,
    ) -> Result<CredentialMutationReceipt, CredentialError> {
        self.transition_activation_stage(&prepared, CredentialActivationStage::SettingsPending)?;
        if let Err(failure) = self.settings_activation.persist_pending_settings(
            &prepared.operation_id,
            &prepared.set_id,
            prepared.expected_settings_revision,
            prepared.proposed_settings_revision,
        ) {
            let error = self.map_store_failure(failure, &prepared.set_id);
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
        if let Err(failure) = self
            .settings_activation
            .verify_pending_settings(&prepared.operation_id, prepared.proposed_settings_revision)
        {
            let error = self.map_store_failure(failure, &prepared.set_id);
            self.rollback_settings_then_abort(&prepared)?;
            return Err(error);
        }
        if let Err(error) = self
            .transition_activation_stage(&prepared, CredentialActivationStage::CredentialPending)
        {
            self.rollback_settings_then_abort(&prepared)?;
            return Err(error);
        }

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
        prepared: &PreparedCredentialActivation,
    ) -> Result<(), CredentialError> {
        if self
            .settings_activation
            .restore_settings_backup(&prepared.operation_id, prepared.expected_settings_revision)
            .is_err()
        {
            let _ = self.mark_activation_recovery_required(prepared);
            return Err(content_free_error(
                CredentialErrorCode::RecoveryRequired,
                CredentialSafeRecoveryAction::Reconcile,
                Some(prepared.set_id.clone()),
            ));
        }
        if self.abort_settings_activation(prepared).is_err() {
            let _ = self.mark_activation_recovery_required(prepared);
            return Err(content_free_error(
                CredentialErrorCode::RecoveryRequired,
                CredentialSafeRecoveryAction::Reconcile,
                Some(prepared.set_id.clone()),
            ));
        }
        Ok(())
    }

    fn transition_activation_stage(
        &self,
        prepared: &PreparedCredentialActivation,
        stage: CredentialActivationStage,
    ) -> Result<(), CredentialError> {
        let _permit = self
            .worker
            .admit(&prepared.operation_id, &prepared.set_id)?;
        let committed_journal = {
            let mut session = self
                .store
                .begin_mutation(&prepared.operation_id, &prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let mut journal = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let Some(pending) = journal.pending_activation.as_mut() else {
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            };
            if pending.operation_id != prepared.operation_id {
                return Err(content_free_error(
                    CredentialErrorCode::Conflict,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            pending.stage = stage;
            if let Some(set) = journal.set_state_mut(&prepared.set_id) {
                set.pending_activation = true;
                set.recovery_state = if stage == CredentialActivationStage::RecoveryRequired {
                    CredentialSetRecoveryState::CommitUnknown
                } else {
                    CredentialSetRecoveryState::PendingIntent
                };
            }
            session
                .commit_journal(&journal)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
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
        prepared: &PreparedCredentialActivation,
    ) -> Result<CredentialMutationReceipt, CredentialError> {
        let _permit = self
            .worker
            .admit(&prepared.operation_id, &prepared.set_id)?;
        let (committed_journal, receipt) = {
            let mut session = self
                .store
                .begin_mutation(&prepared.operation_id, &prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let mut journal = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
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
            if readback.as_ref().map(EncodedCredentialRecord::as_bytes)
                != Some(expected_readback.as_bytes())
            {
                return Err(CredentialStoreFailure::CommitUnknown
                    .into_public(Some(prepared.set_id.clone())));
            }
            let receipt = CredentialMutationReceipt {
                operation_id: prepared.operation_id.clone(),
                idempotency_token: prepared.idempotency_token.clone(),
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
            session.commit_journal(&journal).map_err(|_| {
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
        prepared: &PreparedCredentialActivation,
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
            let mut journal = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
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
            if self
                .settings_activation
                .verify_committed_settings(
                    &prepared.operation_id,
                    prepared.proposed_settings_revision,
                )
                .is_err()
            {
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            if self
                .settings_activation
                .clear_pending_settings(&prepared.operation_id, prepared.proposed_settings_revision)
                .is_err()
            {
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            session
                .delete_staging(&prepared.operation_id, &prepared.set_id)
                .map_err(|_| {
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
                idempotency_token: prepared.idempotency_token.clone(),
                set_id: prepared.set_id.clone(),
                mutation_kind: CredentialMutationKind::Activate,
                expected_revision: prepared.expected_revision.clone(),
                receipt: receipt.clone(),
            });
            journal.global_epoch = journal.global_epoch.saturating_add(1);
            session.commit_journal(&journal).map_err(|_| {
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

    fn mark_activation_recovery_required(
        &self,
        prepared: &PreparedCredentialActivation,
    ) -> Result<(), CredentialError> {
        {
            let mut cached = self
                .journal
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if let Some(pending) = cached
                .pending_activation
                .as_mut()
                .filter(|pending| pending.operation_id == prepared.operation_id)
            {
                pending.stage = CredentialActivationStage::RecoveryRequired;
            }
            if let Some(set) = cached.set_state_mut(&prepared.set_id) {
                set.recovery_state = CredentialSetRecoveryState::CommitUnknown;
                set.pending_activation = true;
            }
        }
        let _permit = self
            .worker
            .admit(&prepared.operation_id, &prepared.set_id)?;
        let recovery_journal = {
            let mut session = self
                .store
                .begin_mutation(&prepared.operation_id, &prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let mut journal = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let Some(pending) = journal.pending_activation.as_mut() else {
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            };
            if pending.operation_id != prepared.operation_id {
                return Err(content_free_error(
                    CredentialErrorCode::Conflict,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            pending.stage = CredentialActivationStage::RecoveryRequired;
            if let Some(set) = journal.set_state_mut(&prepared.set_id) {
                set.recovery_state = CredentialSetRecoveryState::CommitUnknown;
                set.pending_activation = true;
            }
            session
                .commit_journal(&journal)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
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
        let pending = {
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
            journal.pending_activation.clone().ok_or_else(|| {
                content_free_error(
                    CredentialErrorCode::Missing,
                    CredentialSafeRecoveryAction::Reconcile,
                    None,
                )
            })?
        };
        if pending.operation_id != *operation_id {
            return Err(content_free_error(
                CredentialErrorCode::Conflict,
                CredentialSafeRecoveryAction::Reconcile,
                Some(pending.set_id),
            ));
        }
        let pending_stage = pending.stage;
        let prepared = PreparedCredentialActivation {
            operation_id: pending.operation_id,
            idempotency_token: pending.idempotency_token,
            set_id: pending.set_id,
            auth_method_id: pending.auth_method_id,
            expected_revision: pending.expected_revision,
            proposed_revision: pending.proposed_revision,
            expected_settings_revision: pending.expected_settings_revision,
            proposed_settings_revision: pending.proposed_settings_revision,
        };

        if pending_stage == CredentialActivationStage::SettingsPending {
            if self
                .settings_activation
                .verify_pending_settings(
                    &prepared.operation_id,
                    prepared.proposed_settings_revision,
                )
                .is_err()
            {
                let _ = self.mark_activation_recovery_required(&prepared);
                return Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id),
                ));
            }
            self.transition_activation_stage(
                &prepared,
                CredentialActivationStage::CredentialPending,
            )?;
        }
        if pending_stage == CredentialActivationStage::CleanupPending {
            let receipt = CredentialMutationReceipt {
                operation_id: prepared.operation_id.clone(),
                idempotency_token: prepared.idempotency_token.clone(),
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
            let mut journal = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
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
            let expected_active_is_authoritative = match (
                active_record.as_ref(),
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
            };

            if proposed_active_is_authoritative {
                self.settings_activation
                    .verify_pending_settings(
                        &prepared.operation_id,
                        prepared.proposed_settings_revision,
                    )
                    .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
                let receipt = CredentialMutationReceipt {
                    operation_id: prepared.operation_id.clone(),
                    idempotency_token: prepared.idempotency_token.clone(),
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
                        Some(prepared.set_id),
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
                    .restore_settings_backup(
                        &prepared.operation_id,
                        prepared.expected_settings_revision,
                    )
                    .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
                let receipt = CredentialMutationReceipt {
                    operation_id: prepared.operation_id.clone(),
                    idempotency_token: prepared.idempotency_token.clone(),
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
                    idempotency_token: prepared.idempotency_token.clone(),
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

    fn abort_settings_activation(
        &self,
        prepared: &PreparedCredentialActivation,
    ) -> Result<(), CredentialError> {
        let _permit = self
            .worker
            .admit(&prepared.operation_id, &prepared.set_id)?;
        let committed_journal = {
            let mut session = self
                .store
                .begin_mutation(&prepared.operation_id, &prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            let mut journal = session
                .load_journal()
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            if journal
                .pending_activation
                .as_ref()
                .is_none_or(|pending| pending.operation_id != prepared.operation_id)
            {
                return Err(content_free_error(
                    CredentialErrorCode::Conflict,
                    CredentialSafeRecoveryAction::Reconcile,
                    Some(prepared.set_id.clone()),
                ));
            }
            session
                .delete_staging(&prepared.operation_id, &prepared.set_id)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            journal.pending_activation = None;
            journal
                .pending_intents
                .retain(|intent| intent.operation_id != prepared.operation_id);
            if let Some(set) = journal.set_state_mut(&prepared.set_id) {
                set.pending_activation = false;
                set.recovery_state = CredentialSetRecoveryState::None;
            }
            session
                .commit_journal(&journal)
                .map_err(|failure| self.map_store_failure(failure, &prepared.set_id))?;
            journal
        };
        *self
            .journal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = committed_journal;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CredentialEntryStore, CredentialResolution, CredentialService,
        CredentialSettingsActivationPort, CredentialUseRequest, DeleteCredentialSet,
        PrepareCredentialActivation, ReplaceCredentialSet,
    };
    use crate::credentials::domain::{
        AuthorityJournal, CredentialRecordEnvelope, CredentialStoreFailure, StoredSecretBundle,
    };
    use crate::credentials::fake::{
        FakeCredentialStore, FakeSettingsActivationPort, FakeSettingsCall, FakeStoreCall,
    };
    use crate::credentials::test_support::DeterministicTokenSource;
    use audio_graph_ipc_contract::credential_contract::{
        AuthMethodId, AwsPartition, AwsSdkService, BuiltInCredentialSetId, CREDENTIAL_USE_POLICIES,
        CredentialActivationStage, CredentialAudience, CredentialAudiencePolicyDefinition,
        CredentialBackendKind, CredentialErrorCode, CredentialIdempotencyToken,
        CredentialMutationResultCode, CredentialOperationId, CredentialPurpose, CredentialSetId,
        CredentialSetRecordState, CredentialSetRecoveryState,
        CredentialUsePolicyDecisionDefinition, CredentialWorkerState, CustomCredentialSetId,
        PORTABLE_ENCODED_RECORD_MAX_BYTES, SecureTransportScheme,
    };
    use std::sync::{Arc, Barrier, mpsc};
    use std::time::Duration;

    fn idempotency(value: &str) -> CredentialIdempotencyToken {
        CredentialIdempotencyToken::parse(value).expect("canonical idempotency token")
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
                CredentialErrorCode::StoreUnavailable,
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
                settings,
            );
            let receipt = restarted
                .recover_settings_activation(&pending.operation_id)
                .expect("restart rolls back a non-authoritative staging cut");
            assert_eq!(receipt.result_code, CredentialMutationResultCode::NoChange);
            assert!(restarted.snapshot_status().pending_activation.is_none());
            assert_eq!(store.staging_count(), 0);
            assert!(restarted.events_since(0).events.is_empty());
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
            CredentialStoreFailure::Unsupported,
            CredentialStoreFailure::CorruptRecord,
            CredentialStoreFailure::UnsupportedSchema,
            CredentialStoreFailure::PayloadTooLarge,
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
                idempotency_token: idempotency("a1a1a1a1-b2b2-c3c3-d4d4-e5e5e5e5e5e5"),
            })
            .expect("prepare");
        service
            .commit_settings_activation(prepared.clone())
            .expect_err("cleanup marker failure leaves CleanupPending");
        settings
            .restore_settings_backup(&prepared.operation_id, 46)
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
