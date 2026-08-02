#![forbid(unsafe_code)]

use super::domain::{
    AuthorityJournal, CredentialStoreFailure, StoredSecretBundle, content_free_error,
};
use super::service::{
    CredentialChangeEvent, CredentialResolution, CredentialService, CredentialTokenSource,
    CredentialUseRequest, DeleteCredentialSet, PrepareCredentialActivation,
    PreparedCredentialActivation, ReplaceCredentialSet,
};
use audio_graph_ipc_contract::credential_contract::{
    AuthMethodId, BuiltInCredentialSetId, CredentialBackendAvailability, CredentialBackendKind,
    CredentialError, CredentialErrorCode, CredentialIdempotencyToken, CredentialMigrationState,
    CredentialMutationReceipt, CredentialOperationId, CredentialRevision,
    CredentialSafeRecoveryAction, CredentialServiceStatus, CredentialSetId,
    CredentialSetRecordState, CredentialWorkerState, CredentialWorkerStatus,
};
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(test)]
use std::sync::Condvar;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;
use zeroize::Zeroizing;

const CREDENTIAL_RUNTIME_THREAD_NAME: &str = "credential-v2-runtime";
const CREDENTIAL_RUNTIME_CHANNEL_CAPACITY: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialRuntimeLifecycle {
    Dormant,
    Opening,
    Uninitialized,
    Ready,
    RecoveryRequired,
    Unavailable,
    Stalled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CredentialRuntimeStatus {
    pub(crate) lifecycle: CredentialRuntimeLifecycle,
    pub(crate) service: CredentialServiceStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CredentialEventSinkFailure;

pub(crate) trait CredentialRuntimeEventSink: Send + Sync + 'static {
    fn emit(&self, event: CredentialChangeEvent) -> Result<(), CredentialEventSinkFailure>;
}

pub(crate) enum CredentialRuntimeOpen {
    Uninitialized,
    Ready(Box<CredentialService>),
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialRuntimeRecovery {
    Ready,
    RecoveryRequired,
}

/// Construction-time backend port. Concrete native factories remain in the
/// adapter assembly lane; this module never names a prompt policy or locator.
pub(crate) trait CredentialRuntimeBackend: Send + 'static {
    fn open(
        &mut self,
        token_source: Arc<dyn CredentialTokenSource>,
    ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure>;

    fn initialize(
        &mut self,
        token_source: Arc<dyn CredentialTokenSource>,
    ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure>;

    fn diagnose_or_unlock(&mut self) -> Result<CredentialRuntimeRecovery, CredentialStoreFailure>;
}

enum CredentialSecretDraft {
    ApiKey(Zeroizing<String>),
    AwsStatic {
        access_key_id: Zeroizing<String>,
        secret_access_key: Zeroizing<String>,
        session_token: Option<Zeroizing<String>>,
    },
}

#[derive(Clone, Copy)]
enum CredentialSecretKind {
    ApiKey,
    AwsStatic,
}

fn credential_secret_shape_is_valid(
    set_id: &CredentialSetId,
    auth_method_id: AuthMethodId,
    kind: CredentialSecretKind,
) -> bool {
    match (set_id, auth_method_id, kind) {
        (
            CredentialSetId::BuiltIn(built_in),
            AuthMethodId::ApiKey,
            CredentialSecretKind::ApiKey,
        ) => *built_in != BuiltInCredentialSetId::Aws,
        (
            CredentialSetId::BuiltIn(BuiltInCredentialSetId::Aws),
            AuthMethodId::AwsStatic,
            CredentialSecretKind::AwsStatic,
        ) => true,
        _ => false,
    }
}

fn validate_activation_shape(request: &PrepareCredentialActivation) -> Result<(), CredentialError> {
    let kind = match &request.material {
        StoredSecretBundle::ApiKey { .. } => CredentialSecretKind::ApiKey,
        StoredSecretBundle::AwsStatic { .. } => CredentialSecretKind::AwsStatic,
    };
    if credential_secret_shape_is_valid(&request.set_id, request.auth_method_id, kind) {
        return Ok(());
    }
    Err(content_free_error(
        CredentialErrorCode::InvalidCredentialSet,
        CredentialSafeRecoveryAction::ReenterCredential,
        Some(request.set_id.clone()),
    ))
}

/// Backend-only secret-bearing input. It intentionally implements neither
/// `Debug`, `Clone`, nor serialization; conversion consumes its allocation
/// into the domain's zeroizing value before channel admission.
pub(crate) struct CredentialReplaceDraft {
    set_id: CredentialSetId,
    auth_method_id: AuthMethodId,
    material: CredentialSecretDraft,
    expected_revision: Option<CredentialRevision>,
    idempotency_token: CredentialIdempotencyToken,
}

impl CredentialReplaceDraft {
    pub(crate) fn api_key(
        set_id: CredentialSetId,
        auth_method_id: AuthMethodId,
        api_key: String,
        expected_revision: Option<CredentialRevision>,
        idempotency_token: CredentialIdempotencyToken,
    ) -> Self {
        Self {
            set_id,
            auth_method_id,
            material: CredentialSecretDraft::ApiKey(Zeroizing::new(api_key)),
            expected_revision,
            idempotency_token,
        }
    }

    pub(crate) fn aws_static(
        set_id: CredentialSetId,
        auth_method_id: AuthMethodId,
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
        expected_revision: Option<CredentialRevision>,
        idempotency_token: CredentialIdempotencyToken,
    ) -> Self {
        Self {
            set_id,
            auth_method_id,
            material: CredentialSecretDraft::AwsStatic {
                access_key_id: Zeroizing::new(access_key_id),
                secret_access_key: Zeroizing::new(secret_access_key),
                session_token: session_token.map(Zeroizing::new),
            },
            expected_revision,
            idempotency_token,
        }
    }

    fn into_service_request(self) -> Result<ReplaceCredentialSet, CredentialError> {
        let Self {
            set_id,
            auth_method_id,
            material,
            expected_revision,
            idempotency_token,
        } = self;
        let kind = match &material {
            CredentialSecretDraft::ApiKey(_) => CredentialSecretKind::ApiKey,
            CredentialSecretDraft::AwsStatic { .. } => CredentialSecretKind::AwsStatic,
        };
        if !credential_secret_shape_is_valid(&set_id, auth_method_id, kind) {
            return Err(content_free_error(
                CredentialErrorCode::InvalidCredentialSet,
                CredentialSafeRecoveryAction::ReenterCredential,
                Some(set_id),
            ));
        }
        let material = match material {
            CredentialSecretDraft::ApiKey(mut api_key) => {
                StoredSecretBundle::api_key(std::mem::take(&mut *api_key))?
            }
            CredentialSecretDraft::AwsStatic {
                mut access_key_id,
                mut secret_access_key,
                mut session_token,
            } => StoredSecretBundle::aws_static(
                std::mem::take(&mut *access_key_id),
                std::mem::take(&mut *secret_access_key),
                session_token
                    .as_mut()
                    .map(|token| std::mem::take(&mut **token)),
            )?,
        };
        Ok(ReplaceCredentialSet {
            set_id,
            auth_method_id,
            material,
            expected_revision,
            idempotency_token,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum AdmissionState {
    Idle = 0,
    Busy = 1,
    TimedOut = 2,
    TerminalStalled = 3,
}

impl AdmissionState {
    fn load(value: u8) -> Self {
        match value {
            0 => Self::Idle,
            1 => Self::Busy,
            2 => Self::TimedOut,
            _ => Self::TerminalStalled,
        }
    }
}

#[cfg(test)]
struct RuntimeReleaseBarrier {
    entered: Mutex<bool>,
    entered_cv: Condvar,
    timeout_attempted: Mutex<bool>,
    timeout_attempted_cv: Condvar,
    released: Mutex<bool>,
    released_cv: Condvar,
}

#[cfg(test)]
impl RuntimeReleaseBarrier {
    fn new() -> Self {
        Self {
            entered: Mutex::new(false),
            entered_cv: Condvar::new(),
            timeout_attempted: Mutex::new(false),
            timeout_attempted_cv: Condvar::new(),
            released: Mutex::new(false),
            released_cv: Condvar::new(),
        }
    }

    fn wait_until_entered(&self, timeout: Duration) -> bool {
        let entered = self
            .entered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (entered, _) = self
            .entered_cv
            .wait_timeout_while(entered, timeout, |entered| !*entered)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *entered
    }

    fn release(&self) {
        *self
            .released
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.released_cv.notify_all();
    }

    fn mark_timeout_attempted(&self) {
        *self
            .timeout_attempted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.timeout_attempted_cv.notify_all();
    }

    fn wait_until_timeout_attempted(&self, timeout: Duration) -> bool {
        let attempted = self
            .timeout_attempted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (attempted, _) = self
            .timeout_attempted_cv
            .wait_timeout_while(attempted, timeout, |attempted| !*attempted)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *attempted
    }

    fn enter_and_wait(&self) {
        *self
            .entered
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
        self.entered_cv.notify_all();
        let released = self
            .released
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        drop(
            self.released_cv
                .wait_while(released, |released| !*released)
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
    }
}

struct RuntimeShared {
    admission: AtomicU8,
    status: Mutex<CredentialRuntimeStatus>,
    closed_error: Mutex<Option<CredentialError>>,
    sender: Mutex<Option<mpsc::SyncSender<RuntimeRequest>>>,
    #[cfg(test)]
    release_barrier: Mutex<Option<Arc<RuntimeReleaseBarrier>>>,
    #[cfg(test)]
    timeout_barrier: Mutex<Option<Arc<RuntimeReleaseBarrier>>>,
}

impl RuntimeShared {
    fn admission(&self) -> AdmissionState {
        AdmissionState::load(self.admission.load(Ordering::Acquire))
    }

    fn claim(&self, set_id: Option<&CredentialSetId>) -> Result<(), CredentialError> {
        let mut cached = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match self.admission.compare_exchange(
            AdmissionState::Idle as u8,
            AdmissionState::Busy as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => {
                cached.service.worker = CredentialWorkerStatus {
                    state: CredentialWorkerState::Busy,
                    operation_id: None,
                    set_id: set_id.cloned(),
                };
                Ok(())
            }
            Err(state) if AdmissionState::load(state) == AdmissionState::TerminalStalled => {
                Err(runtime_stalled_error(None))
            }
            Err(_) => Err(operation_in_progress_error(set_id.cloned())),
        }
    }

    fn cached_closed_error(
        &self,
        set_id: Option<&CredentialSetId>,
        allow_uninitialized: bool,
    ) -> Option<CredentialError> {
        let cached = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(mut error) = self
            .closed_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            error.set_id = set_id.cloned();
            return Some(error);
        }
        let set_id = || set_id.cloned();
        match cached.lifecycle {
            CredentialRuntimeLifecycle::Uninitialized if !allow_uninitialized => {
                Some(content_free_error(
                    CredentialErrorCode::MigrationRequired,
                    CredentialSafeRecoveryAction::InitializeStore,
                    set_id(),
                ))
            }
            CredentialRuntimeLifecycle::RecoveryRequired => Some(content_free_error(
                CredentialErrorCode::RecoveryRequired,
                CredentialSafeRecoveryAction::Reconcile,
                set_id(),
            )),
            CredentialRuntimeLifecycle::Unavailable => Some(
                match cached.service.backend.availability {
                    CredentialBackendAvailability::Locked => CredentialStoreFailure::Locked,
                    CredentialBackendAvailability::AccessDenied => {
                        CredentialStoreFailure::AccessDenied
                    }
                    CredentialBackendAvailability::Unsupported => {
                        CredentialStoreFailure::Unsupported
                    }
                    CredentialBackendAvailability::RecoveryRequired => {
                        return Some(content_free_error(
                            CredentialErrorCode::RecoveryRequired,
                            CredentialSafeRecoveryAction::Reconcile,
                            set_id(),
                        ));
                    }
                    CredentialBackendAvailability::Unknown
                    | CredentialBackendAvailability::Available
                    | CredentialBackendAvailability::Unavailable => {
                        CredentialStoreFailure::Unavailable
                    }
                }
                .into_public(set_id()),
            ),
            CredentialRuntimeLifecycle::Stalled => Some(runtime_stalled_error(None)),
            CredentialRuntimeLifecycle::Dormant
            | CredentialRuntimeLifecycle::Opening
            | CredentialRuntimeLifecycle::Uninitialized
            | CredentialRuntimeLifecycle::Ready => None,
        }
    }

    fn mark_opening(&self, set_id: Option<&CredentialSetId>) {
        let mut cached = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cached.lifecycle = CredentialRuntimeLifecycle::Opening;
        cached.service.backend.availability = CredentialBackendAvailability::Unknown;
        cached.service.worker = CredentialWorkerStatus {
            state: CredentialWorkerState::Busy,
            operation_id: None,
            set_id: set_id.cloned(),
        };
    }

    fn mark_timed_out_if_busy(&self, set_id: Option<&CredentialSetId>) {
        #[cfg(test)]
        if let Some(barrier) = self
            .timeout_barrier
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            barrier.mark_timeout_attempted();
        }
        let mut cached = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self
            .admission
            .compare_exchange(
                AdmissionState::Busy as u8,
                AdmissionState::TimedOut as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return;
        }
        cached.lifecycle = CredentialRuntimeLifecycle::Stalled;
        cached.service.worker = CredentialWorkerStatus {
            state: CredentialWorkerState::Stalled,
            operation_id: None,
            set_id: set_id.cloned(),
        };
    }

    fn cache_service_while_admitted(&self, mut service: CredentialServiceStatus) {
        let mut cached = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let timed_out = self.admission() == AdmissionState::TimedOut;
        *self
            .closed_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        service.worker = CredentialWorkerStatus {
            state: if timed_out {
                CredentialWorkerState::Stalled
            } else {
                CredentialWorkerState::Busy
            },
            operation_id: None,
            set_id: service.worker.set_id,
        };
        *cached = CredentialRuntimeStatus {
            lifecycle: if timed_out {
                CredentialRuntimeLifecycle::Stalled
            } else {
                CredentialRuntimeLifecycle::Ready
            },
            service,
        };
    }

    fn cache_open_state(
        &self,
        lifecycle: CredentialRuntimeLifecycle,
        availability: CredentialBackendAvailability,
        set_id: Option<&CredentialSetId>,
    ) {
        let mut cached = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *self
            .closed_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let backend_kind = cached.service.backend.kind;
        let mut service = pre_authority_service_status(backend_kind);
        if lifecycle == CredentialRuntimeLifecycle::Uninitialized {
            for set in &mut service.sets {
                set.record_state = CredentialSetRecordState::Missing;
            }
        }
        service.backend.availability = availability;
        service.migration_state = if lifecycle == CredentialRuntimeLifecycle::RecoveryRequired {
            CredentialMigrationState::RecoveryRequired
        } else {
            CredentialMigrationState::Uninitialized
        };
        service.worker = CredentialWorkerStatus {
            state: CredentialWorkerState::Busy,
            operation_id: None,
            set_id: set_id.cloned(),
        };
        *cached = CredentialRuntimeStatus { lifecycle, service };
    }

    fn release(&self, service: Option<&CredentialService>) {
        let mut cached = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        #[cfg(test)]
        if let Some(barrier) = self
            .release_barrier
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            barrier.enter_and_wait();
        }
        if let Some(service) = service {
            let mut projection = service.snapshot_status();
            projection.worker = idle_worker();
            *self
                .closed_error
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
            *cached = CredentialRuntimeStatus {
                lifecycle: CredentialRuntimeLifecycle::Ready,
                service: projection,
            };
        } else {
            cached.service.worker = idle_worker();
        }
        self.admission
            .store(AdmissionState::Idle as u8, Ordering::Release);
    }

    fn close_ready_service(&self, mut service: CredentialServiceStatus, error: &CredentialError) {
        let (lifecycle, availability) = closed_service_projection(error.code)
            .expect("closed service error has a lifecycle projection");
        let mut cached = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        service.backend.availability = availability;
        if lifecycle == CredentialRuntimeLifecycle::RecoveryRequired {
            service.migration_state = CredentialMigrationState::RecoveryRequired;
        }
        service.worker = idle_worker();
        *self
            .closed_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error.clone());
        *cached = CredentialRuntimeStatus { lifecycle, service };
        self.admission
            .store(AdmissionState::Idle as u8, Ordering::Release);
    }

    fn remember_closed_error(&self, error: CredentialError) {
        let _status_guard = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *self
            .closed_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error);
    }

    fn terminal_stall(&self, set_id: Option<&CredentialSetId>) {
        self.admission
            .store(AdmissionState::TerminalStalled as u8, Ordering::Release);
        let mut cached = self
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        cached.lifecycle = CredentialRuntimeLifecycle::Stalled;
        cached.service.worker = CredentialWorkerStatus {
            state: CredentialWorkerState::Stalled,
            operation_id: None,
            set_id: set_id.cloned(),
        };
    }
}

pub(crate) struct CredentialRuntime {
    shared: Arc<RuntimeShared>,
    token_source: Arc<dyn CredentialTokenSource>,
}

impl CredentialRuntime {
    pub(crate) fn dormant(backend_kind: CredentialBackendKind) -> Self {
        Self::dormant_with_token_source(backend_kind, Arc::new(SecureUuidCredentialTokenSource))
    }

    fn dormant_with_token_source(
        backend_kind: CredentialBackendKind,
        token_source: Arc<dyn CredentialTokenSource>,
    ) -> Self {
        Self {
            shared: Arc::new(RuntimeShared {
                admission: AtomicU8::new(AdmissionState::Idle as u8),
                status: Mutex::new(CredentialRuntimeStatus {
                    lifecycle: CredentialRuntimeLifecycle::Dormant,
                    service: pre_authority_service_status(backend_kind),
                }),
                closed_error: Mutex::new(None),
                sender: Mutex::new(None),
                #[cfg(test)]
                release_barrier: Mutex::new(None),
                #[cfg(test)]
                timeout_barrier: Mutex::new(None),
            }),
            token_source,
        }
    }

    /// Installs the backend and event sink without opening either one. The
    /// named worker blocks on the bounded channel until an admitted operation.
    pub(crate) fn configure(
        &self,
        backend: Box<dyn CredentialRuntimeBackend>,
        event_sink: Arc<dyn CredentialRuntimeEventSink>,
    ) -> Result<(), CredentialError> {
        let mut configured = self
            .shared
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if configured.is_some() {
            return Err(content_free_error(
                CredentialErrorCode::Conflict,
                CredentialSafeRecoveryAction::Retry,
                None,
            ));
        }
        let (sender, receiver) = mpsc::sync_channel(CREDENTIAL_RUNTIME_CHANNEL_CAPACITY);
        let shared = Arc::downgrade(&self.shared);
        let token_source = Arc::clone(&self.token_source);
        std::thread::Builder::new()
            .name(CREDENTIAL_RUNTIME_THREAD_NAME.to_owned())
            .spawn(move || runtime_worker_loop(receiver, shared, backend, event_sink, token_source))
            .map_err(|_| internal_runtime_error(None))?;
        *configured = Some(sender);
        Ok(())
    }

    pub(crate) fn status(&self) -> CredentialRuntimeStatus {
        self.shared
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    #[cfg(test)]
    fn block_next_release(&self, barrier: Arc<RuntimeReleaseBarrier>) {
        *self
            .shared
            .release_barrier
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Arc::clone(&barrier));
        *self
            .shared
            .timeout_barrier
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(barrier);
    }

    async fn dispatch<T>(
        &self,
        set_id: Option<CredentialSetId>,
        deadline: Duration,
        request: impl FnOnce(oneshot::Sender<Result<T, CredentialError>>) -> RuntimeRequest,
    ) -> Result<T, CredentialError> {
        self.shared.claim(set_id.as_ref())?;
        let sender = self
            .shared
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(sender) = sender else {
            self.shared.release(None);
            return Err(content_free_error(
                CredentialErrorCode::StoreUnavailable,
                CredentialSafeRecoveryAction::Retry,
                set_id,
            ));
        };
        let (reply, response) = oneshot::channel();
        if sender.try_send(request(reply)).is_err() {
            self.shared.terminal_stall(None);
            return Err(runtime_stalled_error(None));
        }
        match tokio::time::timeout(deadline, response).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                self.shared.terminal_stall(None);
                Err(runtime_stalled_error(None))
            }
            Err(_) => {
                self.shared.mark_timed_out_if_busy(set_id.as_ref());
                Err(operation_in_progress_error(set_id))
            }
        }
    }

    pub(crate) async fn initialize(
        &self,
        deadline: Duration,
    ) -> Result<CredentialRuntimeStatus, CredentialError> {
        self.dispatch(None, deadline, |reply| RuntimeRequest::Initialize { reply })
            .await
    }

    pub(crate) async fn diagnose_or_unlock(
        &self,
        deadline: Duration,
    ) -> Result<CredentialRuntimeStatus, CredentialError> {
        self.dispatch(None, deadline, |reply| RuntimeRequest::DiagnoseOrUnlock {
            reply,
        })
        .await
    }

    pub(crate) async fn replace_set(
        &self,
        draft: CredentialReplaceDraft,
        deadline: Duration,
    ) -> Result<CredentialMutationReceipt, CredentialError> {
        let request = draft.into_service_request()?;
        let set_id = request.set_id.clone();
        self.dispatch(Some(set_id), deadline, |reply| RuntimeRequest::Replace {
            request,
            reply,
        })
        .await
    }

    pub(crate) async fn delete_set(
        &self,
        request: DeleteCredentialSet,
        deadline: Duration,
    ) -> Result<CredentialMutationReceipt, CredentialError> {
        let set_id = request.set_id.clone();
        self.dispatch(Some(set_id), deadline, |reply| RuntimeRequest::Delete {
            request,
            reply,
        })
        .await
    }

    pub(crate) async fn resolve_for_use(
        &self,
        request: CredentialUseRequest,
        deadline: Duration,
    ) -> Result<CredentialResolution, CredentialError> {
        let set_id = request.set_id.clone();
        self.dispatch(Some(set_id), deadline, |reply| RuntimeRequest::Resolve {
            request,
            reply,
        })
        .await
    }

    pub(crate) async fn prepare_settings_activation(
        &self,
        request: PrepareCredentialActivation,
        deadline: Duration,
    ) -> Result<PreparedCredentialActivation, CredentialError> {
        validate_activation_shape(&request)?;
        let set_id = request.set_id.clone();
        self.dispatch(Some(set_id), deadline, |reply| {
            RuntimeRequest::PrepareActivation { request, reply }
        })
        .await
    }

    pub(crate) async fn commit_settings_activation(
        &self,
        prepared: PreparedCredentialActivation,
        deadline: Duration,
    ) -> Result<CredentialMutationReceipt, CredentialError> {
        self.dispatch(None, deadline, |reply| RuntimeRequest::CommitActivation {
            prepared,
            reply,
        })
        .await
    }

    pub(crate) async fn recover_settings_activation(
        &self,
        operation_id: CredentialOperationId,
        deadline: Duration,
    ) -> Result<CredentialMutationReceipt, CredentialError> {
        self.dispatch(None, deadline, |reply| RuntimeRequest::RecoverActivation {
            operation_id,
            reply,
        })
        .await
    }
}

struct SecureUuidCredentialTokenSource;

impl CredentialTokenSource for SecureUuidCredentialTokenSource {
    fn next_operation_id(&self) -> CredentialOperationId {
        CredentialOperationId::parse(Uuid::new_v4().to_string())
            .expect("UUID v4 is a canonical credential operation id")
    }

    fn next_revision(&self) -> CredentialRevision {
        CredentialRevision::parse(Uuid::new_v4().to_string())
            .expect("UUID v4 is a canonical credential revision")
    }
}

enum RuntimeRequest {
    Initialize {
        reply: oneshot::Sender<Result<CredentialRuntimeStatus, CredentialError>>,
    },
    DiagnoseOrUnlock {
        reply: oneshot::Sender<Result<CredentialRuntimeStatus, CredentialError>>,
    },
    Replace {
        request: ReplaceCredentialSet,
        reply: oneshot::Sender<Result<CredentialMutationReceipt, CredentialError>>,
    },
    Delete {
        request: DeleteCredentialSet,
        reply: oneshot::Sender<Result<CredentialMutationReceipt, CredentialError>>,
    },
    Resolve {
        request: CredentialUseRequest,
        reply: oneshot::Sender<Result<CredentialResolution, CredentialError>>,
    },
    PrepareActivation {
        request: PrepareCredentialActivation,
        reply: oneshot::Sender<Result<PreparedCredentialActivation, CredentialError>>,
    },
    CommitActivation {
        prepared: PreparedCredentialActivation,
        reply: oneshot::Sender<Result<CredentialMutationReceipt, CredentialError>>,
    },
    RecoverActivation {
        operation_id: CredentialOperationId,
        reply: oneshot::Sender<Result<CredentialMutationReceipt, CredentialError>>,
    },
}

struct RuntimeWorker {
    backend: Box<dyn CredentialRuntimeBackend>,
    event_sink: Arc<dyn CredentialRuntimeEventSink>,
    token_source: Arc<dyn CredentialTokenSource>,
    service: Option<CredentialService>,
    event_cursor: u64,
}

impl RuntimeWorker {
    fn handle(&mut self, request: RuntimeRequest, shared: &RuntimeShared) {
        match request {
            RuntimeRequest::Initialize { reply } => {
                let result = self.initialize_service(shared);
                let response = self.finalize(shared, None, result).map(|()| {
                    shared
                        .status
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone()
                });
                let _ = reply.send(response);
            }
            RuntimeRequest::DiagnoseOrUnlock { reply } => {
                let result = self.run_diagnosis(shared);
                let response = self.finalize(shared, None, result).map(|()| {
                    shared
                        .status
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .clone()
                });
                let _ = reply.send(response);
            }
            RuntimeRequest::Replace { request, reply } => {
                let set_id = request.set_id.clone();
                let result = self.run_service_operation(shared, Some(&set_id), |service| {
                    service.replace_set(request)
                });
                let _ = reply.send(result);
            }
            RuntimeRequest::Delete { request, reply } => {
                let set_id = request.set_id.clone();
                let result = self.run_service_operation(shared, Some(&set_id), |service| {
                    service.delete_set(request)
                });
                let _ = reply.send(result);
            }
            RuntimeRequest::Resolve { request, reply } => {
                let set_id = request.set_id.clone();
                let result = self.run_service_operation(shared, Some(&set_id), |service| {
                    service.resolve_for_use(request)
                });
                let _ = reply.send(result);
            }
            RuntimeRequest::PrepareActivation { request, reply } => {
                let set_id = request.set_id.clone();
                let result = self.run_service_operation(shared, Some(&set_id), |service| {
                    service.prepare_settings_activation(request)
                });
                let _ = reply.send(result);
            }
            RuntimeRequest::CommitActivation { prepared, reply } => {
                let result = self.run_service_operation(shared, None, |service| {
                    service.commit_settings_activation(prepared)
                });
                let _ = reply.send(result);
            }
            RuntimeRequest::RecoverActivation {
                operation_id,
                reply,
            } => {
                let result = self.run_service_operation(shared, None, |service| {
                    service.recover_settings_activation(&operation_id)
                });
                let _ = reply.send(result);
            }
        }
    }

    fn run_service_operation<T>(
        &mut self,
        shared: &RuntimeShared,
        set_id: Option<&CredentialSetId>,
        operation: impl FnOnce(&CredentialService) -> Result<T, CredentialError>,
    ) -> Result<T, CredentialError> {
        let result = self.ensure_ready(shared, set_id).and_then(|()| {
            operation(
                self.service
                    .as_ref()
                    .expect("ready runtime owns credential service"),
            )
        });
        self.finalize(shared, set_id, result)
    }

    fn finalize<T>(
        &mut self,
        shared: &RuntimeShared,
        set_id: Option<&CredentialSetId>,
        mut result: Result<T, CredentialError>,
    ) -> Result<T, CredentialError> {
        if self.service.is_some()
            && let Err(reconcile) = self.reconcile_service(shared)
        {
            result = Err(reconcile);
        }
        if result.as_ref().is_err_and(terminal_service_error) {
            shared.terminal_stall(set_id);
        } else if result
            .as_ref()
            .err()
            .is_some_and(|error| closed_service_projection(error.code).is_some())
            && self.service.is_some()
        {
            let service = self
                .service
                .take()
                .expect("closed ready runtime owns credential service");
            let Err(error) = &result else {
                unreachable!("closed result is an error");
            };
            shared.close_ready_service(service.snapshot_status(), error);
        } else {
            shared.release(self.service.as_ref());
        }
        result
    }

    fn run_diagnosis(&mut self, shared: &RuntimeShared) -> Result<(), CredentialError> {
        shared.mark_opening(None);
        match self.backend.diagnose_or_unlock() {
            Ok(CredentialRuntimeRecovery::Ready) => {
                self.service = None;
                match self.backend.open(Arc::clone(&self.token_source)) {
                    Ok(CredentialRuntimeOpen::Ready(service)) => {
                        self.adopt_service(shared, *service);
                        Ok(())
                    }
                    Ok(CredentialRuntimeOpen::Uninitialized) => {
                        shared.cache_open_state(
                            CredentialRuntimeLifecycle::Uninitialized,
                            CredentialBackendAvailability::Available,
                            None,
                        );
                        Ok(())
                    }
                    Ok(CredentialRuntimeOpen::RecoveryRequired) => {
                        shared.cache_open_state(
                            CredentialRuntimeLifecycle::RecoveryRequired,
                            CredentialBackendAvailability::RecoveryRequired,
                            None,
                        );
                        Ok(())
                    }
                    Err(failure) => {
                        cache_open_failure(shared, failure, None);
                        Err(failure.into_public(None))
                    }
                }
            }
            Ok(CredentialRuntimeRecovery::RecoveryRequired) => {
                self.service = None;
                shared.cache_open_state(
                    CredentialRuntimeLifecycle::RecoveryRequired,
                    CredentialBackendAvailability::RecoveryRequired,
                    None,
                );
                Ok(())
            }
            Err(failure) => {
                cache_open_failure(shared, failure, None);
                Err(failure.into_public(None))
            }
        }
    }

    fn initialize_service(&mut self, shared: &RuntimeShared) -> Result<(), CredentialError> {
        if self.service.is_some() {
            return Ok(());
        }
        if let Some(error) = shared.cached_closed_error(None, true) {
            return Err(error);
        }
        shared.mark_opening(None);
        let opened = self.backend.open(Arc::clone(&self.token_source));
        match opened {
            Ok(CredentialRuntimeOpen::Ready(service)) => {
                self.adopt_service(shared, *service);
                Ok(())
            }
            Ok(CredentialRuntimeOpen::Uninitialized) => {
                match self.backend.initialize(Arc::clone(&self.token_source)) {
                    Ok(CredentialRuntimeOpen::Ready(service)) => {
                        self.adopt_service(shared, *service);
                        Ok(())
                    }
                    Ok(CredentialRuntimeOpen::Uninitialized) => {
                        shared.cache_open_state(
                            CredentialRuntimeLifecycle::Uninitialized,
                            CredentialBackendAvailability::Available,
                            None,
                        );
                        Err(content_free_error(
                            CredentialErrorCode::MigrationRequired,
                            CredentialSafeRecoveryAction::InitializeStore,
                            None,
                        ))
                    }
                    Ok(CredentialRuntimeOpen::RecoveryRequired) => {
                        shared.cache_open_state(
                            CredentialRuntimeLifecycle::RecoveryRequired,
                            CredentialBackendAvailability::RecoveryRequired,
                            None,
                        );
                        Err(content_free_error(
                            CredentialErrorCode::RecoveryRequired,
                            CredentialSafeRecoveryAction::Reconcile,
                            None,
                        ))
                    }
                    Err(failure) => {
                        cache_open_failure(shared, failure, None);
                        Err(failure.into_public(None))
                    }
                }
            }
            Ok(CredentialRuntimeOpen::RecoveryRequired) => {
                shared.cache_open_state(
                    CredentialRuntimeLifecycle::RecoveryRequired,
                    CredentialBackendAvailability::RecoveryRequired,
                    None,
                );
                Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    None,
                ))
            }
            Err(failure) => {
                cache_open_failure(shared, failure, None);
                Err(failure.into_public(None))
            }
        }
    }

    fn adopt_service(&mut self, shared: &RuntimeShared, service: CredentialService) {
        let snapshot = service.snapshot_status();
        self.event_cursor = snapshot.global_epoch;
        shared.cache_service_while_admitted(snapshot);
        self.service = Some(service);
    }

    fn ensure_ready(
        &mut self,
        shared: &RuntimeShared,
        set_id: Option<&CredentialSetId>,
    ) -> Result<(), CredentialError> {
        if self.service.is_some() {
            return Ok(());
        }
        if let Some(error) = shared.cached_closed_error(set_id, false) {
            return Err(error);
        }
        shared.mark_opening(set_id);
        match self.backend.open(Arc::clone(&self.token_source)) {
            Ok(CredentialRuntimeOpen::Ready(service)) => {
                self.adopt_service(shared, *service);
                Ok(())
            }
            Ok(CredentialRuntimeOpen::Uninitialized) => {
                shared.cache_open_state(
                    CredentialRuntimeLifecycle::Uninitialized,
                    CredentialBackendAvailability::Available,
                    set_id,
                );
                Err(content_free_error(
                    CredentialErrorCode::MigrationRequired,
                    CredentialSafeRecoveryAction::InitializeStore,
                    set_id.cloned(),
                ))
            }
            Ok(CredentialRuntimeOpen::RecoveryRequired) => {
                shared.cache_open_state(
                    CredentialRuntimeLifecycle::RecoveryRequired,
                    CredentialBackendAvailability::RecoveryRequired,
                    set_id,
                );
                Err(content_free_error(
                    CredentialErrorCode::RecoveryRequired,
                    CredentialSafeRecoveryAction::Reconcile,
                    set_id.cloned(),
                ))
            }
            Err(failure) => {
                cache_open_failure(shared, failure, set_id);
                Err(failure.into_public(set_id.cloned()))
            }
        }
    }

    fn reconcile_service(&mut self, shared: &RuntimeShared) -> Result<(), CredentialError> {
        let service = self
            .service
            .as_ref()
            .expect("reconciliation requires ready credential service");
        let snapshot = service.snapshot_status();
        let batch = service.events_since(self.event_cursor);
        let mut expected = self.event_cursor;
        let ordered = !batch.gap_detected
            && batch.latest_epoch >= self.event_cursor
            && batch.latest_epoch == snapshot.global_epoch
            && batch.events.iter().all(|event| {
                expected = match expected.checked_add(1) {
                    Some(next) => next,
                    None => return false,
                };
                event.global_epoch == expected
            })
            && expected == batch.latest_epoch;
        if !ordered {
            return Err(runtime_stalled_error(None));
        }
        shared.cache_service_while_admitted(snapshot);
        for event in batch.events {
            self.event_sink
                .emit(event)
                .map_err(|_| runtime_stalled_error(None))?;
        }
        self.event_cursor = batch.latest_epoch;
        Ok(())
    }
}

fn runtime_worker_loop(
    receiver: mpsc::Receiver<RuntimeRequest>,
    shared: Weak<RuntimeShared>,
    backend: Box<dyn CredentialRuntimeBackend>,
    event_sink: Arc<dyn CredentialRuntimeEventSink>,
    token_source: Arc<dyn CredentialTokenSource>,
) {
    let mut worker = RuntimeWorker {
        backend,
        event_sink,
        token_source,
        service: None,
        event_cursor: 0,
    };
    while let Ok(request) = receiver.recv() {
        let Some(shared) = shared.upgrade() else {
            break;
        };
        if catch_unwind(AssertUnwindSafe(|| worker.handle(request, &shared))).is_err() {
            shared.terminal_stall(None);
        }
    }
}

fn pre_authority_service_status(backend_kind: CredentialBackendKind) -> CredentialServiceStatus {
    let mut status = AuthorityJournal::new(backend_kind).snapshot(idle_worker());
    for set in &mut status.sets {
        set.record_state = CredentialSetRecordState::Unknown;
    }
    status
}

fn idle_worker() -> CredentialWorkerStatus {
    CredentialWorkerStatus {
        state: CredentialWorkerState::Idle,
        operation_id: None,
        set_id: None,
    }
}

fn operation_in_progress_error(set_id: Option<CredentialSetId>) -> CredentialError {
    content_free_error(
        CredentialErrorCode::OperationInProgress,
        CredentialSafeRecoveryAction::Retry,
        set_id,
    )
}

fn runtime_stalled_error(set_id: Option<CredentialSetId>) -> CredentialError {
    content_free_error(
        CredentialErrorCode::StalledWorker,
        CredentialSafeRecoveryAction::RestartApplication,
        set_id,
    )
}

fn internal_runtime_error(set_id: Option<CredentialSetId>) -> CredentialError {
    content_free_error(
        CredentialErrorCode::Internal,
        CredentialSafeRecoveryAction::Retry,
        set_id,
    )
}

fn terminal_service_error(error: &CredentialError) -> bool {
    matches!(
        error.code,
        CredentialErrorCode::StalledWorker | CredentialErrorCode::CommitUnknown
    )
}

fn closed_service_projection(
    code: CredentialErrorCode,
) -> Option<(CredentialRuntimeLifecycle, CredentialBackendAvailability)> {
    match code {
        CredentialErrorCode::Locked => Some((
            CredentialRuntimeLifecycle::Unavailable,
            CredentialBackendAvailability::Locked,
        )),
        CredentialErrorCode::AccessDenied | CredentialErrorCode::PermissionHardeningFailed => {
            Some((
                CredentialRuntimeLifecycle::Unavailable,
                CredentialBackendAvailability::AccessDenied,
            ))
        }
        CredentialErrorCode::StoreUnavailable => Some((
            CredentialRuntimeLifecycle::Unavailable,
            CredentialBackendAvailability::Unavailable,
        )),
        CredentialErrorCode::StoreUnsupported => Some((
            CredentialRuntimeLifecycle::Unavailable,
            CredentialBackendAvailability::Unsupported,
        )),
        CredentialErrorCode::RecoveryRequired
        | CredentialErrorCode::CorruptRecord
        | CredentialErrorCode::UnsupportedSchema
        | CredentialErrorCode::AmbiguousMatch => Some((
            CredentialRuntimeLifecycle::RecoveryRequired,
            CredentialBackendAvailability::RecoveryRequired,
        )),
        _ => None,
    }
}

fn cache_open_failure(
    shared: &RuntimeShared,
    failure: CredentialStoreFailure,
    set_id: Option<&CredentialSetId>,
) {
    let public_error = failure.into_public(set_id.cloned());
    let (lifecycle, availability) = match failure {
        CredentialStoreFailure::Locked => (
            CredentialRuntimeLifecycle::Unavailable,
            CredentialBackendAvailability::Locked,
        ),
        CredentialStoreFailure::AccessDenied => (
            CredentialRuntimeLifecycle::Unavailable,
            CredentialBackendAvailability::AccessDenied,
        ),
        CredentialStoreFailure::Unsupported => (
            CredentialRuntimeLifecycle::Unavailable,
            CredentialBackendAvailability::Unsupported,
        ),
        CredentialStoreFailure::StalledWorker | CredentialStoreFailure::CommitUnknown => (
            CredentialRuntimeLifecycle::Stalled,
            CredentialBackendAvailability::RecoveryRequired,
        ),
        CredentialStoreFailure::CorruptRecord
        | CredentialStoreFailure::UnsupportedSchema
        | CredentialStoreFailure::AmbiguousMatch => (
            CredentialRuntimeLifecycle::RecoveryRequired,
            CredentialBackendAvailability::RecoveryRequired,
        ),
        _ => (
            CredentialRuntimeLifecycle::Unavailable,
            CredentialBackendAvailability::Unavailable,
        ),
    };
    shared.cache_open_state(lifecycle, availability, set_id);
    shared.remember_closed_error(public_error);
}

#[cfg(test)]
mod tests {
    use super::{
        AdmissionState, CredentialEventSinkFailure, CredentialReplaceDraft, CredentialRuntime,
        CredentialRuntimeBackend, CredentialRuntimeEventSink, CredentialRuntimeLifecycle,
        CredentialRuntimeOpen, CredentialRuntimeRecovery, RuntimeReleaseBarrier,
        SecureUuidCredentialTokenSource,
    };
    use crate::credentials::domain::{
        AuthorityJournal, CredentialStoreFailure, EncodedCredentialRecord, StoredSecretBundle,
    };
    use crate::credentials::fake::{FakeCredentialStore, FakeStoreCall};
    use crate::credentials::service::{
        CredentialChangeEvent, CredentialEntryStore, CredentialMutationSession,
        CredentialResolution, CredentialService, CredentialTokenSource, CredentialUseRequest,
        DeleteCredentialSet, PrepareCredentialActivation,
    };
    use crate::credentials::test_support::DeterministicTokenSource;
    use audio_graph_ipc_contract::credential_contract::{
        AuthMethodId, BuiltInCredentialSetId, CredentialAudience, CredentialBackendAvailability,
        CredentialBackendKind, CredentialErrorCode, CredentialIdempotencyToken,
        CredentialOperationId, CredentialPurpose, CredentialRevision, CredentialSetId,
        CredentialSetRecordState, CredentialWorkerState, SecureTransportScheme,
    };
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Condvar, Mutex, MutexGuard, mpsc};
    use std::time::Duration;
    use uuid::Uuid;
    use zeroize::Zeroizing;

    struct DelayedCommitControl {
        block_next_commit: AtomicBool,
        entered: Mutex<bool>,
        entered_cv: Condvar,
        released: Mutex<bool>,
        released_cv: Condvar,
    }

    impl DelayedCommitControl {
        fn new() -> Self {
            Self {
                block_next_commit: AtomicBool::new(true),
                entered: Mutex::new(false),
                entered_cv: Condvar::new(),
                released: Mutex::new(false),
                released_cv: Condvar::new(),
            }
        }

        fn wait_until_entered(&self, timeout: Duration) -> bool {
            let entered = self
                .entered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (entered, _) = self
                .entered_cv
                .wait_timeout_while(entered, timeout, |entered| !*entered)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *entered
        }

        fn release(&self) {
            *self
                .released
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            self.released_cv.notify_all();
        }

        fn block_commit_if_requested(&self) {
            if !self.block_next_commit.swap(false, Ordering::AcqRel) {
                return;
            }
            *self
                .entered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            self.entered_cv.notify_all();
            let released = self
                .released
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            drop(
                self.released_cv
                    .wait_while(released, |released| !*released)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
        }
    }

    struct DelayedCredentialStore {
        state: Mutex<DelayedCredentialState>,
        control: Arc<DelayedCommitControl>,
        begin_mutations: AtomicUsize,
    }

    struct DelayedCredentialState {
        journal: AuthorityJournal,
        active: Vec<(CredentialSetId, Zeroizing<Vec<u8>>)>,
        staging: Vec<(CredentialOperationId, CredentialSetId, Zeroizing<Vec<u8>>)>,
    }

    impl DelayedCredentialStore {
        fn new(journal: AuthorityJournal, control: Arc<DelayedCommitControl>) -> Self {
            Self {
                state: Mutex::new(DelayedCredentialState {
                    journal,
                    active: Vec::new(),
                    staging: Vec::new(),
                }),
                control,
                begin_mutations: AtomicUsize::new(0),
            }
        }

        fn begin_mutation_count(&self) -> usize {
            self.begin_mutations.load(Ordering::SeqCst)
        }
    }

    impl CredentialEntryStore for DelayedCredentialStore {
        fn read_active(
            &self,
            set_id: &CredentialSetId,
        ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
            let state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
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
            self.begin_mutations.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(DelayedMutationSession {
                state: self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
                control: Arc::clone(&self.control),
            }))
        }
    }

    struct DelayedMutationSession<'a> {
        state: MutexGuard<'a, DelayedCredentialState>,
        control: Arc<DelayedCommitControl>,
    }

    impl CredentialMutationSession for DelayedMutationSession<'_> {
        fn load_journal(&mut self) -> Result<AuthorityJournal, CredentialStoreFailure> {
            Ok(self.state.journal.clone())
        }

        fn read_active(
            &mut self,
            set_id: &CredentialSetId,
        ) -> Result<Option<EncodedCredentialRecord>, CredentialStoreFailure> {
            Ok(self
                .state
                .active
                .iter()
                .find(|(candidate, _)| candidate == set_id)
                .map(|(_, bytes)| {
                    EncodedCredentialRecord::from_boundary_bytes(bytes.as_slice().to_vec())
                }))
        }

        fn persist_intent(
            &mut self,
            journal: &AuthorityJournal,
        ) -> Result<(), CredentialStoreFailure> {
            self.state.journal = journal.clone();
            Ok(())
        }

        fn replace_active(
            &mut self,
            set_id: &CredentialSetId,
            record: EncodedCredentialRecord,
        ) -> Result<(), CredentialStoreFailure> {
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
            self.read_active(set_id)
        }

        fn write_staging(
            &mut self,
            operation_id: &CredentialOperationId,
            set_id: &CredentialSetId,
            record: EncodedCredentialRecord,
        ) -> Result<(), CredentialStoreFailure> {
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
            self.state.staging.retain(|(operation, candidate, _)| {
                operation != operation_id || candidate != set_id
            });
            Ok(())
        }

        fn commit_journal(
            &mut self,
            journal: &AuthorityJournal,
        ) -> Result<(), CredentialStoreFailure> {
            self.control.block_commit_if_requested();
            self.state.journal = journal.clone();
            Ok(())
        }
    }

    struct ReadyBackend {
        store: Arc<DelayedCredentialStore>,
        initial_journal: AuthorityJournal,
        open_calls: Arc<AtomicUsize>,
    }

    impl CredentialRuntimeBackend for ReadyBackend {
        fn open(
            &mut self,
            token_source: Arc<dyn CredentialTokenSource>,
        ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
            self.open_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CredentialRuntimeOpen::Ready(Box::new(
                CredentialService::new(
                    self.store.clone(),
                    self.initial_journal.clone(),
                    token_source,
                ),
            )))
        }

        fn initialize(
            &mut self,
            _token_source: Arc<dyn CredentialTokenSource>,
        ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
            Err(CredentialStoreFailure::RevisionConflict)
        }

        fn diagnose_or_unlock(
            &mut self,
        ) -> Result<CredentialRuntimeRecovery, CredentialStoreFailure> {
            Ok(CredentialRuntimeRecovery::RecoveryRequired)
        }
    }

    struct OpeningControl {
        entered: Mutex<bool>,
        entered_cv: Condvar,
        released: Mutex<bool>,
        released_cv: Condvar,
    }

    impl OpeningControl {
        fn new() -> Self {
            Self {
                entered: Mutex::new(false),
                entered_cv: Condvar::new(),
                released: Mutex::new(false),
                released_cv: Condvar::new(),
            }
        }

        fn enter_and_wait(&self) {
            *self
                .entered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            self.entered_cv.notify_all();
            let released = self
                .released
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            drop(
                self.released_cv
                    .wait_while(released, |released| !*released)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            );
        }

        fn wait_until_entered(&self, timeout: Duration) -> bool {
            let entered = self
                .entered
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (entered, _) = self
                .entered_cv
                .wait_timeout_while(entered, timeout, |entered| !*entered)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *entered
        }

        fn release(&self) {
            *self
                .released
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = true;
            self.released_cv.notify_all();
        }
    }

    struct InitiallyUninitializedBackend {
        control: Arc<OpeningControl>,
        journal: AuthorityJournal,
        store: Arc<FakeCredentialStore>,
        open_calls: Arc<AtomicUsize>,
        initialize_calls: Arc<AtomicUsize>,
        worker_names: Arc<Mutex<Vec<String>>>,
    }

    impl InitiallyUninitializedBackend {
        fn record_worker_name(&self) {
            self.worker_names
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .push(
                    std::thread::current()
                        .name()
                        .unwrap_or("unnamed")
                        .to_owned(),
                );
        }
    }

    impl CredentialRuntimeBackend for InitiallyUninitializedBackend {
        fn open(
            &mut self,
            _token_source: Arc<dyn CredentialTokenSource>,
        ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
            self.record_worker_name();
            self.open_calls.fetch_add(1, Ordering::SeqCst);
            self.control.enter_and_wait();
            Ok(CredentialRuntimeOpen::Uninitialized)
        }

        fn initialize(
            &mut self,
            token_source: Arc<dyn CredentialTokenSource>,
        ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
            self.record_worker_name();
            self.initialize_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CredentialRuntimeOpen::Ready(Box::new(
                CredentialService::new(self.store.clone(), self.journal.clone(), token_source),
            )))
        }

        fn diagnose_or_unlock(
            &mut self,
        ) -> Result<CredentialRuntimeRecovery, CredentialStoreFailure> {
            Ok(CredentialRuntimeRecovery::RecoveryRequired)
        }
    }

    struct LockedThenRecoveredBackend {
        journal: AuthorityJournal,
        store: Arc<FakeCredentialStore>,
        open_calls: Arc<AtomicUsize>,
        recovery_calls: Arc<AtomicUsize>,
    }

    impl CredentialRuntimeBackend for LockedThenRecoveredBackend {
        fn open(
            &mut self,
            token_source: Arc<dyn CredentialTokenSource>,
        ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
            let call = self.open_calls.fetch_add(1, Ordering::SeqCst);
            if call == 0 {
                return Err(CredentialStoreFailure::Locked);
            }
            Ok(CredentialRuntimeOpen::Ready(Box::new(
                CredentialService::new(self.store.clone(), self.journal.clone(), token_source),
            )))
        }

        fn initialize(
            &mut self,
            _token_source: Arc<dyn CredentialTokenSource>,
        ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
            Err(CredentialStoreFailure::RevisionConflict)
        }

        fn diagnose_or_unlock(
            &mut self,
        ) -> Result<CredentialRuntimeRecovery, CredentialStoreFailure> {
            self.recovery_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CredentialRuntimeRecovery::Ready)
        }
    }

    struct AlwaysReadyBackend {
        journal: AuthorityJournal,
        store: Arc<FakeCredentialStore>,
    }

    impl CredentialRuntimeBackend for AlwaysReadyBackend {
        fn open(
            &mut self,
            token_source: Arc<dyn CredentialTokenSource>,
        ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
            Ok(CredentialRuntimeOpen::Ready(Box::new(
                CredentialService::new(self.store.clone(), self.journal.clone(), token_source),
            )))
        }

        fn initialize(
            &mut self,
            _token_source: Arc<dyn CredentialTokenSource>,
        ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
            Err(CredentialStoreFailure::RevisionConflict)
        }

        fn diagnose_or_unlock(
            &mut self,
        ) -> Result<CredentialRuntimeRecovery, CredentialStoreFailure> {
            Ok(CredentialRuntimeRecovery::Ready)
        }
    }

    struct CountingReadyBackend {
        journal: AuthorityJournal,
        store: Arc<FakeCredentialStore>,
        open_calls: Arc<AtomicUsize>,
        recovery_calls: Arc<AtomicUsize>,
    }

    impl CredentialRuntimeBackend for CountingReadyBackend {
        fn open(
            &mut self,
            token_source: Arc<dyn CredentialTokenSource>,
        ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
            self.open_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CredentialRuntimeOpen::Ready(Box::new(
                CredentialService::new(self.store.clone(), self.journal.clone(), token_source),
            )))
        }

        fn initialize(
            &mut self,
            _token_source: Arc<dyn CredentialTokenSource>,
        ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
            Err(CredentialStoreFailure::RevisionConflict)
        }

        fn diagnose_or_unlock(
            &mut self,
        ) -> Result<CredentialRuntimeRecovery, CredentialStoreFailure> {
            self.recovery_calls.fetch_add(1, Ordering::SeqCst);
            Ok(CredentialRuntimeRecovery::Ready)
        }
    }

    struct RecordingEventSink {
        sender: Mutex<mpsc::Sender<CredentialChangeEvent>>,
        emitted: AtomicUsize,
    }

    impl CredentialRuntimeEventSink for RecordingEventSink {
        fn emit(&self, event: CredentialChangeEvent) -> Result<(), CredentialEventSinkFailure> {
            self.emitted.fetch_add(1, Ordering::SeqCst);
            self.sender
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .send(event)
                .map_err(|_| CredentialEventSinkFailure)
        }
    }

    struct RejectingEventSink {
        emitted: Arc<AtomicUsize>,
    }

    impl CredentialRuntimeEventSink for RejectingEventSink {
        fn emit(&self, _event: CredentialChangeEvent) -> Result<(), CredentialEventSinkFailure> {
            self.emitted.fetch_add(1, Ordering::SeqCst);
            Err(CredentialEventSinkFailure)
        }
    }

    #[derive(Clone, Copy)]
    enum FixedOpenOutcome {
        Uninitialized,
        RecoveryRequired,
        Failure(CredentialStoreFailure),
    }

    struct FixedOpenBackend {
        outcome: FixedOpenOutcome,
    }

    impl CredentialRuntimeBackend for FixedOpenBackend {
        fn open(
            &mut self,
            _token_source: Arc<dyn CredentialTokenSource>,
        ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
            match self.outcome {
                FixedOpenOutcome::Uninitialized => Ok(CredentialRuntimeOpen::Uninitialized),
                FixedOpenOutcome::RecoveryRequired => Ok(CredentialRuntimeOpen::RecoveryRequired),
                FixedOpenOutcome::Failure(failure) => Err(failure),
            }
        }

        fn initialize(
            &mut self,
            _token_source: Arc<dyn CredentialTokenSource>,
        ) -> Result<CredentialRuntimeOpen, CredentialStoreFailure> {
            Ok(CredentialRuntimeOpen::Uninitialized)
        }

        fn diagnose_or_unlock(
            &mut self,
        ) -> Result<CredentialRuntimeRecovery, CredentialStoreFailure> {
            Ok(CredentialRuntimeRecovery::RecoveryRequired)
        }
    }

    fn idempotency(value: &str) -> CredentialIdempotencyToken {
        CredentialIdempotencyToken::parse(value).expect("canonical idempotency token")
    }

    fn api_key_draft(
        token: &str,
        expected_revision: Option<CredentialRevision>,
    ) -> CredentialReplaceDraft {
        CredentialReplaceDraft::api_key(
            CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
            AuthMethodId::ApiKey,
            "runtime-secret-canary".to_owned(),
            expected_revision,
            idempotency(token),
        )
    }

    fn deepgram_use_request() -> CredentialUseRequest {
        CredentialUseRequest {
            set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
            consumer_id: "asr.deepgram",
            auth_method_id: AuthMethodId::ApiKey,
            purpose: CredentialPurpose::Asr,
            audience: CredentialAudience::SecureNetworkOrigin {
                scheme: SecureTransportScheme::Wss,
                canonical_host: "api.deepgram.com".to_owned(),
                effective_port: 443,
            },
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn timed_out_replace_remains_admitted_until_late_commit_and_emits_once() {
        let initial_journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let control = Arc::new(DelayedCommitControl::new());
        let store = Arc::new(DelayedCredentialStore::new(
            initial_journal.clone(),
            Arc::clone(&control),
        ));
        let open_calls = Arc::new(AtomicUsize::new(0));
        let (event_tx, event_rx) = mpsc::channel();
        let event_sink = Arc::new(RecordingEventSink {
            sender: Mutex::new(event_tx),
            emitted: AtomicUsize::new(0),
        });
        let runtime = Arc::new(CredentialRuntime::dormant_with_token_source(
            CredentialBackendKind::InMemory,
            Arc::new(DeterministicTokenSource::default()),
        ));
        runtime
            .configure(
                Box::new(ReadyBackend {
                    store: Arc::clone(&store),
                    initial_journal,
                    open_calls: Arc::clone(&open_calls),
                }),
                event_sink.clone(),
            )
            .expect("configure zero-I/O runtime");

        let first_runtime = Arc::clone(&runtime);
        let first = tokio::spawn(async move {
            first_runtime
                .replace_set(
                    api_key_draft("00000000-0000-0000-0000-000000000101", None),
                    Duration::from_millis(100),
                )
                .await
        });
        let entered_control = Arc::clone(&control);
        let entered = tokio::task::spawn_blocking(move || {
            entered_control.wait_until_entered(Duration::from_secs(2))
        })
        .await
        .expect("join commit-entry waiter");
        assert!(entered, "first replace reaches the delayed native commit");

        let timed_out = first
            .await
            .expect("join first replace")
            .expect_err("caller deadline expires while native commit stays live");
        assert_eq!(timed_out.code, CredentialErrorCode::OperationInProgress);
        let stalled = runtime.status();
        assert_eq!(stalled.lifecycle, CredentialRuntimeLifecycle::Stalled);
        assert_eq!(stalled.service.worker.state, CredentialWorkerState::Stalled);

        let begin_count = store.begin_mutation_count();
        let overtaking = runtime
            .replace_set(
                api_key_draft("00000000-0000-0000-0000-000000000102", None),
                Duration::from_secs(1),
            )
            .await
            .expect_err("second replace cannot overtake the live timed-out call");
        assert_eq!(overtaking.code, CredentialErrorCode::OperationInProgress);
        assert_eq!(store.begin_mutation_count(), begin_count);

        control.release();
        let late_event = event_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("late committed change is emitted");
        assert_eq!(late_event.global_epoch, 1);
        assert!(!format!("{late_event:?}").contains("runtime-secret-canary"));
        assert_eq!(
            late_event.receipt.idempotency_token,
            idempotency("00000000-0000-0000-0000-000000000101")
        );
        assert_eq!(event_sink.emitted.load(Ordering::SeqCst), 1);
        assert!(
            event_rx.try_recv().is_err(),
            "late commit emits exactly once"
        );

        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let status = runtime.status();
                if status.lifecycle == CredentialRuntimeLifecycle::Ready
                    && status.service.global_epoch == 1
                    && status.service.worker.state == CredentialWorkerState::Idle
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("late finalizer advances the cache and releases admission");

        let third = runtime
            .replace_set(
                api_key_draft(
                    "00000000-0000-0000-0000-000000000103",
                    late_event.receipt.new_revision,
                ),
                Duration::from_secs(2),
            )
            .await
            .expect("new replace is admitted only after late finalization");
        assert_eq!(
            third.idempotency_token,
            idempotency("00000000-0000-0000-0000-000000000103")
        );
        let third_event = event_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("third committed change event");
        assert_eq!(third_event.global_epoch, 2);
        assert_eq!(event_sink.emitted.load(Ordering::SeqCst), 2);
        assert_eq!(open_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn timeout_losing_the_release_mutex_race_cannot_publish_a_stale_stall() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let (event_tx, event_rx) = mpsc::channel();
        let runtime = Arc::new(CredentialRuntime::dormant_with_token_source(
            CredentialBackendKind::InMemory,
            Arc::new(DeterministicTokenSource::default()),
        ));
        runtime
            .configure(
                Box::new(AlwaysReadyBackend { journal, store }),
                Arc::new(RecordingEventSink {
                    sender: Mutex::new(event_tx),
                    emitted: AtomicUsize::new(0),
                }),
            )
            .expect("configure runtime");
        let release_barrier = Arc::new(RuntimeReleaseBarrier::new());
        runtime.block_next_release(Arc::clone(&release_barrier));

        let replacing_runtime = Arc::clone(&runtime);
        let replacing = tokio::spawn(async move {
            replacing_runtime
                .replace_set(
                    api_key_draft("00000000-0000-0000-0000-000000000151", None),
                    Duration::from_millis(100),
                )
                .await
        });
        let entered_barrier = Arc::clone(&release_barrier);
        assert!(
            tokio::task::spawn_blocking(move || {
                entered_barrier.wait_until_entered(Duration::from_secs(2))
            })
            .await
            .expect("join release-barrier waiter"),
            "worker reaches release while holding the status mutex"
        );
        let timeout_barrier = Arc::clone(&release_barrier);
        let timeout_attempted = tokio::task::spawn_blocking(move || {
            timeout_barrier.wait_until_timeout_attempted(Duration::from_secs(2))
        })
        .await
        .expect("join timeout-attempt waiter");
        if !timeout_attempted {
            release_barrier.release();
        }
        assert!(
            timeout_attempted,
            "caller deadline reaches the serialized timeout transition"
        );

        release_barrier.release();
        let timeout = replacing
            .await
            .expect("join raced replace")
            .expect_err("caller deadline expires at release boundary");
        assert_eq!(timeout.code, CredentialErrorCode::OperationInProgress);
        let late_event = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("committed replace event precedes release");

        let coherent = runtime.status();
        assert_eq!(coherent.lifecycle, CredentialRuntimeLifecycle::Ready);
        assert_eq!(coherent.service.worker.state, CredentialWorkerState::Idle);
        assert_eq!(runtime.shared.admission(), AdmissionState::Idle);

        runtime
            .replace_set(
                api_key_draft(
                    "00000000-0000-0000-0000-000000000152",
                    late_event.receipt.new_revision,
                ),
                Duration::from_secs(1),
            )
            .await
            .expect("next request is admitted after coherent late release");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn initialize_opens_uninitialized_backend_on_named_worker_without_status_io() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let control = Arc::new(OpeningControl::new());
        let open_calls = Arc::new(AtomicUsize::new(0));
        let initialize_calls = Arc::new(AtomicUsize::new(0));
        let worker_names = Arc::new(Mutex::new(Vec::new()));
        let (event_tx, event_rx) = mpsc::channel();
        let event_sink = Arc::new(RecordingEventSink {
            sender: Mutex::new(event_tx),
            emitted: AtomicUsize::new(0),
        });
        let runtime = Arc::new(CredentialRuntime::dormant_with_token_source(
            CredentialBackendKind::InMemory,
            Arc::new(DeterministicTokenSource::default()),
        ));
        runtime
            .configure(
                Box::new(InitiallyUninitializedBackend {
                    control: Arc::clone(&control),
                    journal,
                    store,
                    open_calls: Arc::clone(&open_calls),
                    initialize_calls: Arc::clone(&initialize_calls),
                    worker_names: Arc::clone(&worker_names),
                }),
                event_sink,
            )
            .expect("configure dormant runtime");

        for _ in 0..32 {
            let status = runtime.status();
            assert_eq!(status.lifecycle, CredentialRuntimeLifecycle::Dormant);
            assert_eq!(status.service.worker.state, CredentialWorkerState::Idle);
            assert!(
                status
                    .service
                    .sets
                    .iter()
                    .all(|set| set.record_state == CredentialSetRecordState::Unknown)
            );
        }
        assert_eq!(open_calls.load(Ordering::SeqCst), 0);
        assert_eq!(initialize_calls.load(Ordering::SeqCst), 0);

        let initializing_runtime = Arc::clone(&runtime);
        let initializing = tokio::spawn(async move {
            initializing_runtime
                .initialize(Duration::from_secs(2))
                .await
        });
        let entered_control = Arc::clone(&control);
        assert!(
            tokio::task::spawn_blocking(move || {
                entered_control.wait_until_entered(Duration::from_secs(2))
            })
            .await
            .expect("join opening waiter")
        );
        let opening = runtime.status();
        assert_eq!(opening.lifecycle, CredentialRuntimeLifecycle::Opening);
        assert_eq!(opening.service.worker.state, CredentialWorkerState::Busy);
        control.release();

        let ready = initializing
            .await
            .expect("join initialize")
            .expect("initialize ready credential service");
        assert_eq!(ready.lifecycle, CredentialRuntimeLifecycle::Ready);
        assert_eq!(ready.service.worker.state, CredentialWorkerState::Idle);
        assert_eq!(open_calls.load(Ordering::SeqCst), 1);
        assert_eq!(initialize_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            worker_names
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .as_slice(),
            ["credential-v2-runtime", "credential-v2-runtime"]
        );
        assert!(event_rx.try_recv().is_err(), "initialize is eventless");
    }

    #[tokio::test]
    async fn explicit_diagnose_is_the_only_recovery_route_and_reopens_without_an_event() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let open_calls = Arc::new(AtomicUsize::new(0));
        let recovery_calls = Arc::new(AtomicUsize::new(0));
        let (event_tx, event_rx) = mpsc::channel();
        let runtime = CredentialRuntime::dormant_with_token_source(
            CredentialBackendKind::InMemory,
            Arc::new(DeterministicTokenSource::default()),
        );
        runtime
            .configure(
                Box::new(LockedThenRecoveredBackend {
                    journal,
                    store,
                    open_calls: Arc::clone(&open_calls),
                    recovery_calls: Arc::clone(&recovery_calls),
                }),
                Arc::new(RecordingEventSink {
                    sender: Mutex::new(event_tx),
                    emitted: AtomicUsize::new(0),
                }),
            )
            .expect("configure runtime");

        let locked = runtime
            .replace_set(
                api_key_draft("00000000-0000-0000-0000-000000000201", None),
                Duration::from_secs(1),
            )
            .await
            .expect_err("ordinary operation cannot unlock a locked backend");
        assert_eq!(locked.code, CredentialErrorCode::Locked);
        let unavailable = runtime.status();
        assert_eq!(
            unavailable.lifecycle,
            CredentialRuntimeLifecycle::Unavailable
        );
        assert_eq!(
            unavailable.service.backend.availability,
            CredentialBackendAvailability::Locked
        );
        assert_eq!(open_calls.load(Ordering::SeqCst), 1);
        assert_eq!(recovery_calls.load(Ordering::SeqCst), 0);

        for _ in 0..16 {
            let _ = runtime.status();
        }
        assert_eq!(open_calls.load(Ordering::SeqCst), 1);
        assert_eq!(recovery_calls.load(Ordering::SeqCst), 0);

        let still_locked = runtime
            .replace_set(
                api_key_draft("00000000-0000-0000-0000-000000000202", None),
                Duration::from_secs(1),
            )
            .await
            .expect_err("ordinary operations cannot reopen a recovery-gated backend");
        assert_eq!(still_locked.code, CredentialErrorCode::Locked);
        assert_eq!(open_calls.load(Ordering::SeqCst), 1);
        assert_eq!(recovery_calls.load(Ordering::SeqCst), 0);

        let ready = runtime
            .diagnose_or_unlock(Duration::from_secs(1))
            .await
            .expect("explicit payload-free recovery reopens the service");
        assert_eq!(ready.lifecycle, CredentialRuntimeLifecycle::Ready);
        assert_eq!(ready.service.worker.state, CredentialWorkerState::Idle);
        assert_eq!(open_calls.load(Ordering::SeqCst), 2);
        assert_eq!(recovery_calls.load(Ordering::SeqCst), 1);
        assert!(event_rx.try_recv().is_err(), "diagnosis is eventless");
    }

    #[tokio::test]
    async fn post_ready_closed_failures_require_diagnosis_without_further_ordinary_io() {
        let cases = [
            (
                "locked",
                Some(CredentialStoreFailure::Locked),
                CredentialErrorCode::Locked,
                CredentialRuntimeLifecycle::Unavailable,
                CredentialBackendAvailability::Locked,
            ),
            (
                "access_denied",
                Some(CredentialStoreFailure::AccessDenied),
                CredentialErrorCode::AccessDenied,
                CredentialRuntimeLifecycle::Unavailable,
                CredentialBackendAvailability::AccessDenied,
            ),
            (
                "store_unavailable",
                Some(CredentialStoreFailure::Unavailable),
                CredentialErrorCode::StoreUnavailable,
                CredentialRuntimeLifecycle::Unavailable,
                CredentialBackendAvailability::Unavailable,
            ),
            (
                "recovery_required",
                None,
                CredentialErrorCode::RecoveryRequired,
                CredentialRuntimeLifecycle::RecoveryRequired,
                CredentialBackendAvailability::RecoveryRequired,
            ),
            (
                "corrupt_record",
                Some(CredentialStoreFailure::CorruptRecord),
                CredentialErrorCode::CorruptRecord,
                CredentialRuntimeLifecycle::RecoveryRequired,
                CredentialBackendAvailability::RecoveryRequired,
            ),
            (
                "unsupported_schema",
                Some(CredentialStoreFailure::UnsupportedSchema),
                CredentialErrorCode::UnsupportedSchema,
                CredentialRuntimeLifecycle::RecoveryRequired,
                CredentialBackendAvailability::RecoveryRequired,
            ),
            (
                "ambiguous_match",
                Some(CredentialStoreFailure::AmbiguousMatch),
                CredentialErrorCode::AmbiguousMatch,
                CredentialRuntimeLifecycle::RecoveryRequired,
                CredentialBackendAvailability::RecoveryRequired,
            ),
        ];
        let mut observed = Vec::new();
        let mut expected_rows = Vec::new();

        for (name, store_failure, expected_code, expected_lifecycle, expected_availability) in cases
        {
            let mut journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
            if store_failure.is_none() {
                journal.sets.retain(|set| {
                    set.set_id != CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram)
                });
            }
            let store = Arc::new(FakeCredentialStore::new(journal.clone()));
            let open_calls = Arc::new(AtomicUsize::new(0));
            let recovery_calls = Arc::new(AtomicUsize::new(0));
            let (event_tx, event_rx) = mpsc::channel();
            let runtime = CredentialRuntime::dormant_with_token_source(
                CredentialBackendKind::InMemory,
                Arc::new(DeterministicTokenSource::default()),
            );
            runtime
                .configure(
                    Box::new(CountingReadyBackend {
                        journal,
                        store: Arc::clone(&store),
                        open_calls: Arc::clone(&open_calls),
                        recovery_calls: Arc::clone(&recovery_calls),
                    }),
                    Arc::new(RecordingEventSink {
                        sender: Mutex::new(event_tx),
                        emitted: AtomicUsize::new(0),
                    }),
                )
                .expect("configure counting runtime");
            runtime
                .initialize(Duration::from_secs(1))
                .await
                .expect("open genuinely ready runtime");
            assert_eq!(open_calls.load(Ordering::SeqCst), 1);

            if let Some(failure) = store_failure {
                store.fail_next(FakeStoreCall::ReadActive, failure);
            }
            let first = match runtime
                .resolve_for_use(deepgram_use_request(), Duration::from_secs(1))
                .await
            {
                Err(error) => error,
                Ok(_) => panic!("live ready service must report a closed backend failure"),
            };
            let closed = runtime.status();
            let calls_after_first = store.calls();
            let second = match runtime
                .resolve_for_use(deepgram_use_request(), Duration::from_secs(1))
                .await
            {
                Err(error) => error,
                Ok(_) => panic!("ordinary operation must remain behind the cached closed gate"),
            };
            observed.push((
                name,
                first.code,
                closed.lifecycle,
                closed.service.backend.availability,
                closed.service.worker.state,
                second.code,
                store.calls() == calls_after_first,
                open_calls.load(Ordering::SeqCst),
                recovery_calls.load(Ordering::SeqCst),
            ));

            let reopened = runtime
                .diagnose_or_unlock(Duration::from_secs(1))
                .await
                .expect("explicit payload-free diagnosis reopens the service");
            assert_eq!(reopened.lifecycle, CredentialRuntimeLifecycle::Ready);
            assert_eq!(open_calls.load(Ordering::SeqCst), 2);
            assert_eq!(recovery_calls.load(Ordering::SeqCst), 1);
            assert!(
                event_rx.try_recv().is_err(),
                "closed failures are eventless"
            );

            expected_rows.push((
                name,
                expected_code,
                expected_lifecycle,
                expected_availability,
                CredentialWorkerState::Idle,
                expected_code,
                true,
                1,
                0,
            ));
        }
        assert_eq!(observed, expected_rows);
    }

    #[tokio::test]
    async fn secret_schema_failures_are_pre_admission_and_all_artifacts_are_content_free() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let control = Arc::new(DelayedCommitControl::new());
        let store = Arc::new(DelayedCredentialStore::new(
            journal.clone(),
            Arc::clone(&control),
        ));
        let open_calls = Arc::new(AtomicUsize::new(0));
        let (event_tx, event_rx) = mpsc::channel();
        let runtime = CredentialRuntime::dormant_with_token_source(
            CredentialBackendKind::InMemory,
            Arc::new(DeterministicTokenSource::default()),
        );
        runtime
            .configure(
                Box::new(ReadyBackend {
                    store: Arc::clone(&store),
                    initial_journal: journal,
                    open_calls: Arc::clone(&open_calls),
                }),
                Arc::new(RecordingEventSink {
                    sender: Mutex::new(event_tx),
                    emitted: AtomicUsize::new(0),
                }),
            )
            .expect("configure runtime");

        let wrong_shape = CredentialReplaceDraft::aws_static(
            CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
            AuthMethodId::AwsStatic,
            "access-runtime-secret-canary".to_owned(),
            "secret-runtime-secret-canary".to_owned(),
            Some("session-runtime-secret-canary".to_owned()),
            None,
            idempotency("00000000-0000-0000-0000-000000000301"),
        );
        let wrong_shape_error = runtime
            .replace_set(wrong_shape, Duration::from_secs(1))
            .await
            .expect_err("AWS material cannot enter a non-AWS credential set");
        assert_eq!(
            wrong_shape_error.code,
            CredentialErrorCode::InvalidCredentialSet
        );

        let empty_error = runtime
            .replace_set(
                CredentialReplaceDraft::api_key(
                    CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                    AuthMethodId::ApiKey,
                    "   ".to_owned(),
                    None,
                    idempotency("00000000-0000-0000-0000-000000000302"),
                ),
                Duration::from_secs(1),
            )
            .await
            .expect_err("blank secret fails schema validation");
        assert_eq!(empty_error.code, CredentialErrorCode::InvalidCredentialSet);

        assert_eq!(open_calls.load(Ordering::SeqCst), 0);
        assert_eq!(store.begin_mutation_count(), 0);
        let cached = runtime.status();
        assert_eq!(cached.lifecycle, CredentialRuntimeLifecycle::Dormant);
        assert_eq!(cached.service.worker.state, CredentialWorkerState::Idle);
        for artifact in [
            format!("{wrong_shape_error:?}"),
            format!("{empty_error:?}"),
            format!("{cached:?}"),
            "credential-v2-runtime".to_owned(),
        ] {
            assert!(!artifact.contains("runtime-secret-canary"));
        }
        assert!(event_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn invalid_opaque_activation_shape_is_rejected_before_admission_or_open() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let open_calls = Arc::new(AtomicUsize::new(0));
        let recovery_calls = Arc::new(AtomicUsize::new(0));
        let (event_tx, event_rx) = mpsc::channel();
        let runtime = CredentialRuntime::dormant_with_token_source(
            CredentialBackendKind::InMemory,
            Arc::new(DeterministicTokenSource::default()),
        );
        runtime
            .configure(
                Box::new(CountingReadyBackend {
                    journal,
                    store: Arc::clone(&store),
                    open_calls: Arc::clone(&open_calls),
                    recovery_calls: Arc::clone(&recovery_calls),
                }),
                Arc::new(RecordingEventSink {
                    sender: Mutex::new(event_tx),
                    emitted: AtomicUsize::new(0),
                }),
            )
            .expect("configure dormant runtime");

        let invalid = PrepareCredentialActivation {
            set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
            auth_method_id: AuthMethodId::AwsStatic,
            material: StoredSecretBundle::aws_static(
                "activation-access-runtime-secret-canary",
                "activation-secret-runtime-secret-canary",
                Some("activation-session-runtime-secret-canary"),
            )
            .expect("valid AWS material shape"),
            expected_revision: None,
            expected_settings_revision: 4,
            proposed_settings_revision: 5,
            idempotency_token: idempotency("00000000-0000-0000-0000-000000000351"),
        };
        let error = runtime
            .prepare_settings_activation(invalid, Duration::from_secs(1))
            .await
            .expect_err("AWS material cannot activate the Deepgram set");
        let status = runtime.status();

        assert_eq!(
            (
                error.code,
                status.lifecycle,
                status.service.worker.state,
                open_calls.load(Ordering::SeqCst),
                recovery_calls.load(Ordering::SeqCst),
                store.calls(),
            ),
            (
                CredentialErrorCode::InvalidCredentialSet,
                CredentialRuntimeLifecycle::Dormant,
                CredentialWorkerState::Idle,
                0,
                0,
                Vec::new(),
            )
        );
        for artifact in [format!("{error:?}"), format!("{status:?}")] {
            assert!(!artifact.contains("runtime-secret-canary"));
        }
        assert!(
            event_rx.try_recv().is_err(),
            "preflight failure is eventless"
        );
    }

    #[tokio::test]
    async fn replace_then_delete_forwards_epoch_ordered_events_and_cache_once() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let (event_tx, event_rx) = mpsc::channel();
        let event_sink = Arc::new(RecordingEventSink {
            sender: Mutex::new(event_tx),
            emitted: AtomicUsize::new(0),
        });
        let runtime = CredentialRuntime::dormant_with_token_source(
            CredentialBackendKind::InMemory,
            Arc::new(DeterministicTokenSource::default()),
        );
        runtime
            .configure(
                Box::new(AlwaysReadyBackend { journal, store }),
                event_sink.clone(),
            )
            .expect("configure runtime");

        let replaced = runtime
            .replace_set(
                api_key_draft("00000000-0000-0000-0000-000000000401", None),
                Duration::from_secs(1),
            )
            .await
            .expect("replace through serialized runtime");
        let deleted = runtime
            .delete_set(
                DeleteCredentialSet {
                    set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                    expected_revision: replaced.new_revision,
                    idempotency_token: idempotency("00000000-0000-0000-0000-000000000402"),
                },
                Duration::from_secs(1),
            )
            .await
            .expect("delete through same serialized runtime");
        assert_eq!(
            deleted.idempotency_token,
            idempotency("00000000-0000-0000-0000-000000000402")
        );

        let first = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("replace event");
        let second = event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("delete event");
        assert_eq!((first.global_epoch, second.global_epoch), (1, 2));
        assert!(event_rx.try_recv().is_err());
        assert_eq!(event_sink.emitted.load(Ordering::SeqCst), 2);
        let cached = runtime.status();
        assert_eq!(cached.lifecycle, CredentialRuntimeLifecycle::Ready);
        assert_eq!(cached.service.global_epoch, 2);
        assert_eq!(cached.service.worker.state, CredentialWorkerState::Idle);
    }

    #[tokio::test]
    async fn resolve_for_use_stays_on_the_worker_and_returns_an_opaque_lease() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let (event_tx, event_rx) = mpsc::channel();
        let runtime = CredentialRuntime::dormant_with_token_source(
            CredentialBackendKind::InMemory,
            Arc::new(DeterministicTokenSource::default()),
        );
        runtime
            .configure(
                Box::new(AlwaysReadyBackend { journal, store }),
                Arc::new(RecordingEventSink {
                    sender: Mutex::new(event_tx),
                    emitted: AtomicUsize::new(0),
                }),
            )
            .expect("configure runtime");

        let replaced = runtime
            .replace_set(
                api_key_draft("00000000-0000-0000-0000-000000000451", None),
                Duration::from_secs(1),
            )
            .await
            .expect("replace through serialized runtime");
        event_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("replace event");

        let resolution = runtime
            .resolve_for_use(
                CredentialUseRequest {
                    set_id: CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                    consumer_id: "asr.deepgram",
                    auth_method_id: AuthMethodId::ApiKey,
                    purpose: CredentialPurpose::Asr,
                    audience: CredentialAudience::SecureNetworkOrigin {
                        scheme: SecureTransportScheme::Wss,
                        canonical_host: "api.deepgram.com".to_owned(),
                        effective_port: 443,
                    },
                },
                Duration::from_secs(1),
            )
            .await
            .expect("resolve credential through serialized runtime");

        let CredentialResolution::Stored(lease) = resolution else {
            panic!("Deepgram API key resolves to a stored lease");
        };
        assert_eq!(lease.set_id, replaced.set_id);
        assert_eq!(Some(lease.revision.clone()), replaced.new_revision);
        assert_eq!(
            lease.expose_api_key(str::to_owned),
            Some("runtime-secret-canary".to_owned())
        );
        assert!(event_rx.try_recv().is_err(), "resolve is eventless");
        assert_eq!(
            runtime.status().service.worker.state,
            CredentialWorkerState::Idle
        );
    }

    #[test]
    fn production_tokens_are_distinct_canonical_lowercase_rfc4122_v4() {
        let source = SecureUuidCredentialTokenSource;
        let mut seen = HashSet::new();
        for _ in 0..256 {
            for token in [
                source.next_operation_id().as_str().to_owned(),
                source.next_revision().as_str().to_owned(),
            ] {
                let parsed = Uuid::parse_str(&token).expect("UUID token parses");
                assert_eq!(parsed.get_version_num(), 4);
                assert_eq!(parsed.get_variant(), uuid::Variant::RFC4122);
                assert_eq!(token, parsed.hyphenated().to_string());
                assert!(
                    seen.insert(token),
                    "operation and revision ids are distinct"
                );
            }
        }
    }

    #[tokio::test]
    async fn closed_open_outcomes_have_exact_uninitialized_recovery_and_unavailable_projections() {
        let cases = [
            (
                FixedOpenOutcome::Uninitialized,
                CredentialErrorCode::MigrationRequired,
                CredentialRuntimeLifecycle::Uninitialized,
                CredentialBackendAvailability::Available,
            ),
            (
                FixedOpenOutcome::RecoveryRequired,
                CredentialErrorCode::RecoveryRequired,
                CredentialRuntimeLifecycle::RecoveryRequired,
                CredentialBackendAvailability::RecoveryRequired,
            ),
            (
                FixedOpenOutcome::Failure(CredentialStoreFailure::Unavailable),
                CredentialErrorCode::StoreUnavailable,
                CredentialRuntimeLifecycle::Unavailable,
                CredentialBackendAvailability::Unavailable,
            ),
        ];
        for (index, (outcome, error_code, lifecycle, availability)) in cases.into_iter().enumerate()
        {
            let runtime = CredentialRuntime::dormant_with_token_source(
                CredentialBackendKind::InMemory,
                Arc::new(DeterministicTokenSource::default()),
            );
            let (event_tx, _event_rx) = mpsc::channel();
            runtime
                .configure(
                    Box::new(FixedOpenBackend { outcome }),
                    Arc::new(RecordingEventSink {
                        sender: Mutex::new(event_tx),
                        emitted: AtomicUsize::new(0),
                    }),
                )
                .expect("configure fixed backend");
            let token = format!("00000000-0000-0000-0000-0000000005{index:02}");
            let error = runtime
                .replace_set(api_key_draft(&token, None), Duration::from_secs(1))
                .await
                .expect_err("closed open outcome prevents mutation");
            assert_eq!(error.code, error_code);
            let status = runtime.status();
            assert_eq!(status.lifecycle, lifecycle);
            assert_eq!(status.service.backend.availability, availability);
            assert_eq!(status.service.worker.state, CredentialWorkerState::Idle);
        }
    }

    #[tokio::test]
    async fn commit_unknown_is_terminal_before_retry_recovery_or_event() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        store.fail_next(
            FakeStoreCall::CommitJournal,
            CredentialStoreFailure::CommitUnknown,
        );
        let (event_tx, event_rx) = mpsc::channel();
        let runtime = CredentialRuntime::dormant_with_token_source(
            CredentialBackendKind::InMemory,
            Arc::new(DeterministicTokenSource::default()),
        );
        runtime
            .configure(
                Box::new(AlwaysReadyBackend {
                    journal,
                    store: Arc::clone(&store),
                }),
                Arc::new(RecordingEventSink {
                    sender: Mutex::new(event_tx),
                    emitted: AtomicUsize::new(0),
                }),
            )
            .expect("configure runtime");

        let unknown = runtime
            .replace_set(
                api_key_draft("00000000-0000-0000-0000-000000000601", None),
                Duration::from_secs(1),
            )
            .await
            .expect_err("uncertain commit fails closed");
        assert_eq!(unknown.code, CredentialErrorCode::CommitUnknown);
        let calls_after_unknown = store.calls();
        assert!(event_rx.try_recv().is_err());
        let status = runtime.status();
        assert_eq!(status.lifecycle, CredentialRuntimeLifecycle::Stalled);
        assert_eq!(status.service.worker.state, CredentialWorkerState::Stalled);

        let retry = runtime
            .replace_set(
                api_key_draft("00000000-0000-0000-0000-000000000602", None),
                Duration::from_secs(1),
            )
            .await
            .expect_err("terminal runtime rejects retry");
        assert_eq!(retry.code, CredentialErrorCode::StalledWorker);
        let diagnosis = runtime
            .diagnose_or_unlock(Duration::from_secs(1))
            .await
            .expect_err("terminal native uncertainty requires restart");
        assert_eq!(diagnosis.code, CredentialErrorCode::StalledWorker);
        assert_eq!(store.calls(), calls_after_unknown);
    }

    #[tokio::test]
    async fn event_sink_failure_is_terminal_and_never_retries_the_committed_event() {
        let journal = AuthorityJournal::new(CredentialBackendKind::InMemory);
        let store = Arc::new(FakeCredentialStore::new(journal.clone()));
        let emitted = Arc::new(AtomicUsize::new(0));
        let runtime = CredentialRuntime::dormant_with_token_source(
            CredentialBackendKind::InMemory,
            Arc::new(DeterministicTokenSource::default()),
        );
        runtime
            .configure(
                Box::new(AlwaysReadyBackend { journal, store }),
                Arc::new(RejectingEventSink {
                    emitted: Arc::clone(&emitted),
                }),
            )
            .expect("configure runtime");

        let failure = runtime
            .replace_set(
                api_key_draft("00000000-0000-0000-0000-000000000701", None),
                Duration::from_secs(1),
            )
            .await
            .expect_err("failed event publication stalls runtime");
        assert_eq!(failure.code, CredentialErrorCode::StalledWorker);
        assert_eq!(emitted.load(Ordering::SeqCst), 1);
        assert_eq!(
            runtime.status().lifecycle,
            CredentialRuntimeLifecycle::Stalled
        );
        let rejected = runtime
            .replace_set(
                api_key_draft("00000000-0000-0000-0000-000000000702", None),
                Duration::from_secs(1),
            )
            .await
            .expect_err("terminal event failure rejects later requests");
        assert_eq!(rejected.code, CredentialErrorCode::StalledWorker);
        assert_eq!(emitted.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn disconnected_channel_is_terminal_and_has_no_locator_or_secret_artifact() {
        let runtime = CredentialRuntime::dormant_with_token_source(
            CredentialBackendKind::InMemory,
            Arc::new(DeterministicTokenSource::default()),
        );
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        *runtime
            .shared
            .sender
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(sender);

        let error = runtime
            .replace_set(
                api_key_draft("00000000-0000-0000-0000-000000000801", None),
                Duration::from_secs(1),
            )
            .await
            .expect_err("disconnected worker channel fails closed");

        assert_eq!(error.code, CredentialErrorCode::StalledWorker);
        assert_eq!(error.set_id, None);
        assert!(!format!("{error:?}").contains("runtime-secret-canary"));
        let cached = runtime.status();
        assert_eq!(cached.lifecycle, CredentialRuntimeLifecycle::Stalled);
        assert_eq!(cached.service.worker.state, CredentialWorkerState::Stalled);
        assert_eq!(cached.service.worker.set_id, None);
    }
}
