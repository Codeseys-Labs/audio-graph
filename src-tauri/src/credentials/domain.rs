use audio_graph_ipc_contract::credential_contract::{
    AuthMethodId, BUILT_IN_CREDENTIAL_SET_IDS, BuiltInCredentialSetId, CredentialActivationStage,
    CredentialActiveUseAction, CredentialBackendAvailability, CredentialBackendKind,
    CredentialBackendStatus, CredentialCleanupState, CredentialError, CredentialErrorCode,
    CredentialIdempotencyToken, CredentialMigrationState, CredentialMutationReceipt,
    CredentialOperationId, CredentialPendingActivationStatus, CredentialRevision,
    CredentialSafeRecoveryAction, CredentialServiceStatus, CredentialSetId,
    CredentialSetRecordState, CredentialSetRecoveryState, CredentialSetSource, CredentialSetStatus,
    CredentialWorkerStatus, PORTABLE_ENCODED_RECORD_MAX_BYTES,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use zeroize::Zeroizing;

const RECORD_SCHEMA_VERSION: u32 = 1;

pub(crate) struct SecretString(Zeroizing<String>);

impl SecretString {
    fn new(value: impl Into<String>) -> Result<Self, CredentialError> {
        let value = Zeroizing::new(value.into());
        if value.trim().is_empty() {
            return Err(content_free_error(
                CredentialErrorCode::InvalidCredentialSet,
                CredentialSafeRecoveryAction::ReenterCredential,
                None,
            ));
        }
        Ok(Self(value))
    }

    fn expose<R>(&self, use_secret: impl FnOnce(&str) -> R) -> R {
        use_secret(self.0.as_str())
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretString([REDACTED])")
    }
}

pub(crate) enum StoredSecretBundle {
    ApiKey {
        api_key: SecretString,
    },
    AwsStatic {
        access_key_id: SecretString,
        secret_access_key: SecretString,
        session_token: Option<SecretString>,
    },
}

impl StoredSecretBundle {
    pub(crate) fn api_key(value: impl Into<String>) -> Result<Self, CredentialError> {
        Ok(Self::ApiKey {
            api_key: SecretString::new(value)?,
        })
    }

    pub(crate) fn aws_static(
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        session_token: Option<impl Into<String>>,
    ) -> Result<Self, CredentialError> {
        Ok(Self::AwsStatic {
            access_key_id: SecretString::new(access_key_id)?,
            secret_access_key: SecretString::new(secret_access_key)?,
            session_token: session_token.map(SecretString::new).transpose()?,
        })
    }

    pub(crate) fn expose_api_key<R>(&self, use_secret: impl FnOnce(&str) -> R) -> Option<R> {
        match self {
            Self::ApiKey { api_key } => Some(api_key.expose(use_secret)),
            Self::AwsStatic { .. } => None,
        }
    }

    pub(crate) fn expose_aws_static<R>(
        &self,
        use_secret: impl FnOnce(&str, &str, Option<&str>) -> R,
    ) -> Option<R> {
        match self {
            Self::ApiKey { .. } => None,
            Self::AwsStatic {
                access_key_id,
                secret_access_key,
                session_token,
            } => Some(access_key_id.expose(|access| {
                secret_access_key.expose(|secret| {
                    use_secret(
                        access,
                        secret,
                        session_token.as_ref().map(|token| token.0.as_str()),
                    )
                })
            })),
        }
    }
}

impl fmt::Debug for StoredSecretBundle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApiKey { .. } => formatter
                .debug_struct("StoredSecretBundle::ApiKey")
                .field("api_key", &"[REDACTED]")
                .finish(),
            Self::AwsStatic { .. } => formatter
                .debug_struct("StoredSecretBundle::AwsStatic")
                .field("access_key_id", &"[REDACTED]")
                .field("secret_access_key", &"[REDACTED]")
                .field("session_token", &"[REDACTED]")
                .finish(),
        }
    }
}

pub(crate) struct EncodedCredentialRecord(Zeroizing<Vec<u8>>);

impl EncodedCredentialRecord {
    pub(crate) fn as_bytes(&self) -> &[u8] {
        self.0.as_slice()
    }

    pub(crate) fn copy_for_boundary(&self) -> Self {
        Self(Zeroizing::new(self.0.to_vec()))
    }

    pub(crate) fn from_boundary_bytes(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

impl fmt::Debug for EncodedCredentialRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EncodedCredentialRecord([REDACTED])")
    }
}

pub(crate) struct CredentialRecordEnvelope {
    pub(crate) set_id: CredentialSetId,
    pub(crate) revision: CredentialRevision,
    pub(crate) operation_id: CredentialOperationId,
    pub(crate) payload: CredentialRecordPayload,
}

pub(crate) enum CredentialRecordPayload {
    Present {
        auth_method_id: AuthMethodId,
        material: StoredSecretBundle,
    },
    Tombstone,
}

impl CredentialRecordEnvelope {
    pub(crate) fn present(
        set_id: CredentialSetId,
        auth_method_id: AuthMethodId,
        revision: CredentialRevision,
        operation_id: CredentialOperationId,
        material: StoredSecretBundle,
    ) -> Result<Self, CredentialError> {
        let valid_shape = match (&set_id, auth_method_id, &material) {
            (
                CredentialSetId::BuiltIn(BuiltInCredentialSetId::Aws),
                AuthMethodId::AwsStatic,
                StoredSecretBundle::AwsStatic { .. },
            ) => true,
            (
                CredentialSetId::BuiltIn(set_id),
                AuthMethodId::ApiKey,
                StoredSecretBundle::ApiKey { .. },
            ) => *set_id != BuiltInCredentialSetId::Aws,
            _ => false,
        };
        if !valid_shape {
            return Err(content_free_error(
                CredentialErrorCode::InvalidCredentialSet,
                CredentialSafeRecoveryAction::ReenterCredential,
                Some(set_id),
            ));
        }
        Ok(Self {
            set_id,
            revision,
            operation_id,
            payload: CredentialRecordPayload::Present {
                auth_method_id,
                material,
            },
        })
    }

    pub(crate) fn tombstone(
        set_id: CredentialSetId,
        revision: CredentialRevision,
        operation_id: CredentialOperationId,
    ) -> Self {
        Self {
            set_id,
            revision,
            operation_id,
            payload: CredentialRecordPayload::Tombstone,
        }
    }

    pub(crate) fn encode(&self) -> Result<EncodedCredentialRecord, CredentialError> {
        let payload = match &self.payload {
            CredentialRecordPayload::Present {
                auth_method_id,
                material: StoredSecretBundle::ApiKey { api_key },
            } => CredentialRecordPayloadWireRef::Present {
                auth_method_id: *auth_method_id,
                material: StoredSecretBundleWireRef::ApiKey {
                    api_key: api_key.0.as_str(),
                },
            },
            CredentialRecordPayload::Tombstone => CredentialRecordPayloadWireRef::Tombstone,
            CredentialRecordPayload::Present {
                auth_method_id,
                material:
                    StoredSecretBundle::AwsStatic {
                        access_key_id,
                        secret_access_key,
                        session_token,
                    },
            } => CredentialRecordPayloadWireRef::Present {
                auth_method_id: *auth_method_id,
                material: StoredSecretBundleWireRef::AwsStatic {
                    access_key_id: access_key_id.0.as_str(),
                    secret_access_key: secret_access_key.0.as_str(),
                    session_token: session_token.as_ref().map(|token| token.0.as_str()),
                },
            },
        };
        let wire = CredentialRecordWireRef {
            schema_version: RECORD_SCHEMA_VERSION,
            set_id: &self.set_id,
            revision: &self.revision,
            operation_id: &self.operation_id,
            payload,
        };
        let bytes = Zeroizing::new(serde_json::to_vec(&wire).map_err(|_| {
            content_free_error(
                CredentialErrorCode::Internal,
                CredentialSafeRecoveryAction::Retry,
                Some(self.set_id.clone()),
            )
        })?);
        if bytes.len() > PORTABLE_ENCODED_RECORD_MAX_BYTES {
            return Err(content_free_error(
                CredentialErrorCode::PayloadTooLarge,
                CredentialSafeRecoveryAction::ReenterCredential,
                Some(self.set_id.clone()),
            ));
        }
        Ok(EncodedCredentialRecord(bytes))
    }

    pub(crate) fn decode(encoded: &EncodedCredentialRecord) -> Result<Self, CredentialError> {
        if encoded.as_bytes().len() > PORTABLE_ENCODED_RECORD_MAX_BYTES {
            return Err(content_free_error(
                CredentialErrorCode::PayloadTooLarge,
                CredentialSafeRecoveryAction::Reconcile,
                None,
            ));
        }
        let wire: CredentialRecordWire =
            serde_json::from_slice(encoded.as_bytes()).map_err(|_| {
                content_free_error(
                    CredentialErrorCode::CorruptRecord,
                    CredentialSafeRecoveryAction::Reconcile,
                    None,
                )
            })?;
        if wire.schema_version != RECORD_SCHEMA_VERSION {
            return Err(content_free_error(
                CredentialErrorCode::UnsupportedSchema,
                CredentialSafeRecoveryAction::Reconcile,
                Some(wire.set_id),
            ));
        }
        let CredentialRecordWire {
            schema_version: _,
            set_id,
            revision,
            operation_id,
            payload,
        } = wire;
        let (auth_method_id, material) = match payload {
            CredentialRecordPayloadWire::Present {
                auth_method_id,
                material: StoredSecretBundleWire::ApiKey { api_key },
            } => (
                auth_method_id,
                StoredSecretBundle::api_key(api_key).map_err(|_| {
                    content_free_error(
                        CredentialErrorCode::CorruptRecord,
                        CredentialSafeRecoveryAction::Reconcile,
                        Some(set_id.clone()),
                    )
                })?,
            ),
            CredentialRecordPayloadWire::Present {
                auth_method_id,
                material:
                    StoredSecretBundleWire::AwsStatic {
                        access_key_id,
                        secret_access_key,
                        session_token,
                    },
            } => (
                auth_method_id,
                StoredSecretBundle::aws_static(access_key_id, secret_access_key, session_token)
                    .map_err(|_| {
                        content_free_error(
                            CredentialErrorCode::CorruptRecord,
                            CredentialSafeRecoveryAction::Reconcile,
                            Some(set_id.clone()),
                        )
                    })?,
            ),
            CredentialRecordPayloadWire::Tombstone => {
                return Ok(Self::tombstone(set_id, revision, operation_id));
            }
        };
        Self::present(
            set_id.clone(),
            auth_method_id,
            revision,
            operation_id,
            material,
        )
        .map_err(|_| {
            content_free_error(
                CredentialErrorCode::CorruptRecord,
                CredentialSafeRecoveryAction::Reconcile,
                Some(set_id),
            )
        })
    }

    pub(crate) fn expose_api_key<R>(&self, use_secret: impl FnOnce(&str) -> R) -> Option<R> {
        match &self.payload {
            CredentialRecordPayload::Present { material, .. } => {
                material.expose_api_key(use_secret)
            }
            CredentialRecordPayload::Tombstone => None,
        }
    }

    pub(crate) fn expose_aws_static<R>(
        &self,
        use_secret: impl FnOnce(&str, &str, Option<&str>) -> R,
    ) -> Option<R> {
        match &self.payload {
            CredentialRecordPayload::Present { material, .. } => {
                material.expose_aws_static(use_secret)
            }
            CredentialRecordPayload::Tombstone => None,
        }
    }

    pub(crate) fn is_tombstone(&self) -> bool {
        matches!(self.payload, CredentialRecordPayload::Tombstone)
    }

    pub(crate) fn into_present(self) -> Option<(AuthMethodId, StoredSecretBundle)> {
        match self.payload {
            CredentialRecordPayload::Present {
                auth_method_id,
                material,
            } => Some((auth_method_id, material)),
            CredentialRecordPayload::Tombstone => None,
        }
    }
}

impl fmt::Debug for CredentialRecordEnvelope {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CredentialRecordEnvelope")
            .field("set_id", &self.set_id)
            .field("revision", &self.revision)
            .field("operation_id", &self.operation_id)
            .field("payload", &"[REDACTED]")
            .finish()
    }
}

#[derive(Serialize)]
struct CredentialRecordWireRef<'a> {
    schema_version: u32,
    set_id: &'a CredentialSetId,
    revision: &'a CredentialRevision,
    operation_id: &'a CredentialOperationId,
    payload: CredentialRecordPayloadWireRef<'a>,
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum CredentialRecordPayloadWireRef<'a> {
    Present {
        auth_method_id: AuthMethodId,
        material: StoredSecretBundleWireRef<'a>,
    },
    Tombstone,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredSecretBundleWireRef<'a> {
    ApiKey {
        api_key: &'a str,
    },
    AwsStatic {
        access_key_id: &'a str,
        secret_access_key: &'a str,
        session_token: Option<&'a str>,
    },
}

#[derive(Deserialize)]
struct CredentialRecordWire {
    schema_version: u32,
    set_id: CredentialSetId,
    revision: CredentialRevision,
    operation_id: CredentialOperationId,
    payload: CredentialRecordPayloadWire,
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
enum CredentialRecordPayloadWire {
    Present {
        auth_method_id: AuthMethodId,
        material: StoredSecretBundleWire,
    },
    Tombstone,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum StoredSecretBundleWire {
    ApiKey {
        api_key: String,
    },
    AwsStatic {
        access_key_id: String,
        secret_access_key: String,
        session_token: Option<String>,
    },
}

pub(crate) fn content_free_error(
    code: CredentialErrorCode,
    recovery_action: CredentialSafeRecoveryAction,
    set_id: Option<CredentialSetId>,
) -> CredentialError {
    CredentialError {
        code,
        retryable: matches!(
            recovery_action,
            CredentialSafeRecoveryAction::Retry | CredentialSafeRecoveryAction::UnlockStore
        ),
        recovery_action,
        set_id,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialStoreFailure {
    Missing,
    Locked,
    AccessDenied,
    Cancelled,
    Unavailable,
    Unsupported,
    CorruptRecord,
    UnsupportedSchema,
    PayloadTooLarge,
    RevisionConflict,
    OperationInProgress,
    StalledWorker,
    CommitUnknown,
    Internal,
}

impl CredentialStoreFailure {
    pub(crate) fn into_public(self, set_id: Option<CredentialSetId>) -> CredentialError {
        let (code, action) = match self {
            Self::Missing => (
                CredentialErrorCode::Missing,
                CredentialSafeRecoveryAction::ReenterCredential,
            ),
            Self::Locked => (
                CredentialErrorCode::Locked,
                CredentialSafeRecoveryAction::UnlockStore,
            ),
            Self::AccessDenied => (
                CredentialErrorCode::AccessDenied,
                CredentialSafeRecoveryAction::UnlockStore,
            ),
            Self::Cancelled => (
                CredentialErrorCode::Cancelled,
                CredentialSafeRecoveryAction::Retry,
            ),
            Self::Unavailable => (
                CredentialErrorCode::StoreUnavailable,
                CredentialSafeRecoveryAction::Retry,
            ),
            Self::Unsupported => (
                CredentialErrorCode::StoreUnsupported,
                CredentialSafeRecoveryAction::ChooseSupportedBackend,
            ),
            Self::CorruptRecord => (
                CredentialErrorCode::CorruptRecord,
                CredentialSafeRecoveryAction::Reconcile,
            ),
            Self::UnsupportedSchema => (
                CredentialErrorCode::UnsupportedSchema,
                CredentialSafeRecoveryAction::Reconcile,
            ),
            Self::PayloadTooLarge => (
                CredentialErrorCode::PayloadTooLarge,
                CredentialSafeRecoveryAction::ReenterCredential,
            ),
            Self::RevisionConflict => (
                CredentialErrorCode::RevisionConflict,
                CredentialSafeRecoveryAction::Retry,
            ),
            Self::OperationInProgress => (
                CredentialErrorCode::OperationInProgress,
                CredentialSafeRecoveryAction::Retry,
            ),
            Self::StalledWorker => (
                CredentialErrorCode::StalledWorker,
                CredentialSafeRecoveryAction::RestartApplication,
            ),
            Self::CommitUnknown => (
                CredentialErrorCode::CommitUnknown,
                CredentialSafeRecoveryAction::Reconcile,
            ),
            Self::Internal => (
                CredentialErrorCode::Internal,
                CredentialSafeRecoveryAction::Retry,
            ),
        };
        content_free_error(code, action, set_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct JournalSetState {
    pub(crate) set_id: CredentialSetId,
    pub(crate) record_state: CredentialSetRecordState,
    pub(crate) source: CredentialSetSource,
    pub(crate) cleanup_state: CredentialCleanupState,
    pub(crate) revision: Option<CredentialRevision>,
    pub(crate) recovery_state: CredentialSetRecoveryState,
    pub(crate) pending_activation: bool,
    pub(crate) active_use_action: CredentialActiveUseAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CredentialMutationKind {
    Replace,
    Delete,
    Activate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingCredentialIntent {
    pub(crate) operation_id: CredentialOperationId,
    pub(crate) idempotency_token: CredentialIdempotencyToken,
    pub(crate) set_id: CredentialSetId,
    pub(crate) mutation_kind: CredentialMutationKind,
    pub(crate) expected_revision: Option<CredentialRevision>,
    pub(crate) proposed_revision: CredentialRevision,
    pub(crate) recovery_state: CredentialSetRecoveryState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct IdempotencyJournalEntry {
    pub(crate) idempotency_token: CredentialIdempotencyToken,
    pub(crate) set_id: CredentialSetId,
    pub(crate) mutation_kind: CredentialMutationKind,
    pub(crate) expected_revision: Option<CredentialRevision>,
    pub(crate) receipt: CredentialMutationReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PendingSettingsActivation {
    pub(crate) operation_id: CredentialOperationId,
    pub(crate) idempotency_token: CredentialIdempotencyToken,
    pub(crate) set_id: CredentialSetId,
    pub(crate) auth_method_id: AuthMethodId,
    pub(crate) expected_revision: Option<CredentialRevision>,
    pub(crate) proposed_revision: CredentialRevision,
    pub(crate) expected_settings_revision: u64,
    pub(crate) proposed_settings_revision: u64,
    pub(crate) stage: CredentialActivationStage,
}

impl PendingSettingsActivation {
    fn status(&self) -> CredentialPendingActivationStatus {
        CredentialPendingActivationStatus {
            operation_id: self.operation_id.clone(),
            set_id: self.set_id.clone(),
            stage: self.stage,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AuthorityJournal {
    pub(crate) schema_version: u32,
    pub(crate) global_epoch: u64,
    pub(crate) backend: CredentialBackendStatus,
    pub(crate) migration_state: CredentialMigrationState,
    pub(crate) cleanup_state: CredentialCleanupState,
    pub(crate) pending_activation: Option<PendingSettingsActivation>,
    pub(crate) sets: Vec<JournalSetState>,
    pub(crate) pending_intents: Vec<PendingCredentialIntent>,
    pub(crate) idempotency_history: Vec<IdempotencyJournalEntry>,
}

impl AuthorityJournal {
    pub(crate) fn new(backend_kind: CredentialBackendKind) -> Self {
        Self {
            schema_version: RECORD_SCHEMA_VERSION,
            global_epoch: 0,
            backend: CredentialBackendStatus {
                kind: backend_kind,
                availability: CredentialBackendAvailability::Unknown,
            },
            migration_state: CredentialMigrationState::Uninitialized,
            cleanup_state: CredentialCleanupState::NotApplicable,
            pending_activation: None,
            sets: BUILT_IN_CREDENTIAL_SET_IDS
                .iter()
                .copied()
                .map(|set_id| JournalSetState {
                    set_id: CredentialSetId::BuiltIn(set_id),
                    record_state: CredentialSetRecordState::Missing,
                    source: CredentialSetSource::None,
                    cleanup_state: CredentialCleanupState::NotApplicable,
                    revision: None,
                    recovery_state: CredentialSetRecoveryState::None,
                    pending_activation: false,
                    active_use_action: CredentialActiveUseAction::None,
                })
                .collect(),
            pending_intents: Vec::new(),
            idempotency_history: Vec::new(),
        }
    }

    pub(crate) fn snapshot(&self, worker: CredentialWorkerStatus) -> CredentialServiceStatus {
        CredentialServiceStatus {
            global_epoch: self.global_epoch,
            backend: self.backend.clone(),
            migration_state: self.migration_state,
            cleanup_state: self.cleanup_state,
            worker,
            pending_activation: self
                .pending_activation
                .as_ref()
                .map(PendingSettingsActivation::status),
            sets: self
                .sets
                .iter()
                .map(|set| CredentialSetStatus {
                    set_id: set.set_id.clone(),
                    record_state: set.record_state,
                    source: set.source,
                    cleanup_state: set.cleanup_state,
                    revision: set.revision.clone(),
                    recovery_state: set.recovery_state,
                    pending_activation: set.pending_activation,
                    active_use_action: set.active_use_action,
                })
                .collect(),
        }
    }

    pub(crate) fn set_state(&self, set_id: &CredentialSetId) -> Option<&JournalSetState> {
        self.sets.iter().find(|set| &set.set_id == set_id)
    }

    pub(crate) fn set_state_mut(
        &mut self,
        set_id: &CredentialSetId,
    ) -> Option<&mut JournalSetState> {
        self.sets.iter_mut().find(|set| &set.set_id == set_id)
    }

    pub(crate) fn idempotency_entry(
        &self,
        token: &CredentialIdempotencyToken,
    ) -> Option<&IdempotencyJournalEntry> {
        self.idempotency_history
            .iter()
            .find(|entry| &entry.idempotency_token == token)
    }

    pub(crate) fn record_idempotency(&mut self, entry: IdempotencyJournalEntry) {
        const MAX_HISTORY: usize = 128;
        self.idempotency_history.push(entry);
        if self.idempotency_history.len() > MAX_HISTORY {
            self.idempotency_history.remove(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CredentialRecordEnvelope, EncodedCredentialRecord, StoredSecretBundle};
    use audio_graph_ipc_contract::credential_contract::{
        AuthMethodId, BuiltInCredentialSetId, CredentialErrorCode, CredentialOperationId,
        CredentialRevision, CredentialSetId, PORTABLE_ENCODED_RECORD_MAX_BYTES,
    };

    fn revision(value: &str) -> CredentialRevision {
        CredentialRevision::parse(value).expect("canonical revision")
    }

    fn operation(value: &str) -> CredentialOperationId {
        CredentialOperationId::parse(value).expect("canonical operation id")
    }

    #[test]
    fn api_key_record_round_trips_as_one_logical_generation() {
        let record = CredentialRecordEnvelope::present(
            CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
            AuthMethodId::ApiKey,
            revision("11111111-1111-1111-1111-111111111111"),
            operation("22222222-2222-2222-2222-222222222222"),
            StoredSecretBundle::api_key("unit-test-secret").expect("complete API key"),
        )
        .expect("valid record");

        let decoded =
            CredentialRecordEnvelope::decode(&record.encode().expect("encode")).expect("decode");

        assert_eq!(
            decoded.expose_api_key(|secret| secret.to_owned()),
            Some("unit-test-secret".to_string())
        );
    }

    #[test]
    fn aws_static_record_round_trips_all_fields_as_one_generation() {
        let record = CredentialRecordEnvelope::present(
            CredentialSetId::BuiltIn(BuiltInCredentialSetId::Aws),
            AuthMethodId::AwsStatic,
            revision("33333333-3333-3333-3333-333333333333"),
            operation("44444444-4444-4444-4444-444444444444"),
            StoredSecretBundle::aws_static("access-id", "secret-key", Some("session-token"))
                .expect("complete AWS bundle"),
        )
        .expect("valid record");

        let decoded =
            CredentialRecordEnvelope::decode(&record.encode().expect("encode")).expect("decode");

        assert_eq!(
            decoded.expose_aws_static(|access, secret, session| {
                (
                    access.to_owned(),
                    secret.to_owned(),
                    session.map(str::to_owned),
                )
            }),
            Some((
                "access-id".to_string(),
                "secret-key".to_string(),
                Some("session-token".to_string())
            ))
        );
    }

    #[test]
    fn incomplete_or_mismatched_aws_material_is_rejected_before_encoding() {
        let incomplete = StoredSecretBundle::aws_static("", "secret-key", Option::<String>::None)
            .expect_err("empty access id is not a complete bundle");
        assert_eq!(incomplete.code, CredentialErrorCode::InvalidCredentialSet);

        let mismatch = CredentialRecordEnvelope::present(
            CredentialSetId::BuiltIn(BuiltInCredentialSetId::Aws),
            AuthMethodId::ApiKey,
            revision("33333333-aaaa-bbbb-cccc-555555555555"),
            operation("44444444-aaaa-bbbb-cccc-555555555555"),
            StoredSecretBundle::api_key("not-an-aws-bundle").expect("API key shape"),
        )
        .expect_err("AWS never accepts an API-key-shaped generation");
        assert_eq!(mismatch.code, CredentialErrorCode::InvalidCredentialSet);
    }

    #[test]
    fn decode_rejects_wire_material_that_violates_set_and_auth_invariants() {
        let malformed_records = [
            serde_json::json!({
                "schema_version": 1,
                "set_id": "aws",
                "revision": "33333333-aaaa-bbbb-cccc-666666666666",
                "operation_id": "44444444-aaaa-bbbb-cccc-666666666666",
                "payload": {
                    "state": "present",
                    "auth_method_id": "api_key",
                    "material": { "kind": "api_key", "api_key": "wrong-shape" }
                }
            }),
            serde_json::json!({
                "schema_version": 1,
                "set_id": "deepgram",
                "revision": "33333333-aaaa-bbbb-cccc-777777777777",
                "operation_id": "44444444-aaaa-bbbb-cccc-777777777777",
                "payload": {
                    "state": "present",
                    "auth_method_id": "aws_static",
                    "material": {
                        "kind": "aws_static",
                        "access_key_id": "access",
                        "secret_access_key": "secret",
                        "session_token": null
                    }
                }
            }),
        ];

        for malformed in malformed_records {
            let encoded = EncodedCredentialRecord::from_boundary_bytes(
                serde_json::to_vec(&malformed).expect("test JSON"),
            );
            let error = CredentialRecordEnvelope::decode(&encoded)
                .expect_err("wire shape must be revalidated");
            assert_eq!(error.code, CredentialErrorCode::CorruptRecord);
        }
    }

    #[test]
    fn tombstone_round_trip_retains_authoritative_revision_without_material() {
        let tombstone = CredentialRecordEnvelope::tombstone(
            CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
            revision("55555555-5555-5555-5555-555555555555"),
            operation("66666666-6666-6666-6666-666666666666"),
        );

        let decoded =
            CredentialRecordEnvelope::decode(&tombstone.encode().expect("encode")).expect("decode");

        assert!(decoded.is_tombstone());
    }

    #[test]
    fn encoded_record_accepts_exact_portable_limit_and_rejects_the_next_byte() {
        let mut last_success = None;
        let mut first_failure = None;

        for secret_length in 1..=PORTABLE_ENCODED_RECORD_MAX_BYTES + 1 {
            let record = CredentialRecordEnvelope::present(
                CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
                AuthMethodId::ApiKey,
                revision("77777777-7777-7777-7777-777777777777"),
                operation("88888888-8888-8888-8888-888888888888"),
                StoredSecretBundle::api_key("x".repeat(secret_length)).expect("API key"),
            )
            .expect("valid record");

            match record.encode() {
                Ok(encoded) => last_success = Some((secret_length, encoded.as_bytes().len())),
                Err(error) => {
                    first_failure = Some((secret_length, error.code));
                    break;
                }
            }
        }

        let (largest_secret, encoded_length) = last_success.expect("one record must fit");
        let (first_rejected_secret, error_code) = first_failure.expect("oversize rejection");
        assert_eq!(encoded_length, PORTABLE_ENCODED_RECORD_MAX_BYTES);
        assert_eq!(first_rejected_secret, largest_secret + 1);
        assert_eq!(error_code, CredentialErrorCode::PayloadTooLarge);
    }

    #[test]
    fn decode_rejects_an_oversized_boundary_payload_before_parsing() {
        let encoded = EncodedCredentialRecord::from_boundary_bytes(vec![
            b'x';
            PORTABLE_ENCODED_RECORD_MAX_BYTES
                + 1
        ]);

        let error = CredentialRecordEnvelope::decode(&encoded)
            .expect_err("portable read boundary must match the write boundary");

        assert_eq!(error.code, CredentialErrorCode::PayloadTooLarge);
        assert!(error.set_id.is_none());
    }

    #[test]
    fn secret_bearing_domain_debug_and_decode_errors_are_content_free() {
        let canary = "A6BF_DOMAIN_SECRET_CANARY";
        let bundle = StoredSecretBundle::api_key(canary).expect("API key");
        assert!(!format!("{bundle:?}").contains(canary));

        let record = CredentialRecordEnvelope::present(
            CredentialSetId::BuiltIn(BuiltInCredentialSetId::Deepgram),
            AuthMethodId::ApiKey,
            revision("99999999-1111-2222-3333-444444444444"),
            operation("aaaaaaaa-1111-2222-3333-444444444444"),
            StoredSecretBundle::api_key(canary).expect("API key"),
        )
        .expect("record");
        assert!(!format!("{record:?}").contains(canary));
        let encoded = record.encode().expect("encode");
        assert!(!format!("{encoded:?}").contains(canary));

        let malformed =
            EncodedCredentialRecord::from_boundary_bytes(format!("not-json-{canary}").into_bytes());
        let error = CredentialRecordEnvelope::decode(&malformed).expect_err("malformed");
        assert!(!format!("{error:?}").contains(canary));
        assert_eq!(error.code, CredentialErrorCode::CorruptRecord);
    }
}
