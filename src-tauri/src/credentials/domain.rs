use audio_graph_ipc_contract::credential_contract::{
    AuthMethodId, BUILT_IN_CREDENTIAL_SET_IDS, BuiltInCredentialSetId, CredentialActivationStage,
    CredentialActiveUseAction, CredentialBackendAvailability, CredentialBackendKind,
    CredentialBackendStatus, CredentialCleanupState, CredentialError, CredentialErrorCode,
    CredentialIdempotencyToken, CredentialMigrationState, CredentialMutationReceipt,
    CredentialMutationResultCode, CredentialOperationId, CredentialPendingActivationStatus,
    CredentialRevision, CredentialSafeRecoveryAction, CredentialServiceStatus, CredentialSetId,
    CredentialSetRecordState, CredentialSetRecoveryState, CredentialSetSource, CredentialSetStatus,
    CredentialWorkerStatus, PORTABLE_ENCODED_RECORD_MAX_BYTES,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;
use zeroize::Zeroizing;

const RECORD_SCHEMA_VERSION: u32 = 1;
const MAX_PERSISTED_CREDENTIAL_SETS: usize = 128;
const MAX_PENDING_CREDENTIAL_INTENTS: usize = 64;
const MAX_IDEMPOTENCY_HISTORY: usize = 128;

fn stored_auth_method_matches_set(set_id: &CredentialSetId, auth_method_id: AuthMethodId) -> bool {
    match set_id {
        CredentialSetId::BuiltIn(BuiltInCredentialSetId::Aws) => {
            auth_method_id == AuthMethodId::AwsStatic
        }
        CredentialSetId::BuiltIn(_) => auth_method_id == AuthMethodId::ApiKey,
        CredentialSetId::Custom(_) => false,
    }
}

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

    pub(crate) fn from_zeroizing_boundary_bytes(bytes: Zeroizing<Vec<u8>>) -> Self {
        Self(bytes)
    }
}

impl fmt::Debug for EncodedCredentialRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("EncodedCredentialRecord([REDACTED])")
    }
}

/// Opaque identity for one initialized credential authority.
///
/// The wire UUID remains private to the authority-journal adapter. Core code
/// can clone and compare this value, but cannot format, serialize, or recover
/// the underlying authority token.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct CredentialAuthorityInstanceId([u8; 16]);

impl CredentialAuthorityInstanceId {
    pub(super) fn from_validated_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    #[cfg(test)]
    pub(crate) fn from_test_bytes(bytes: [u8; 16]) -> Self {
        Self::from_validated_bytes(bytes)
    }
}

impl fmt::Debug for CredentialAuthorityInstanceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialAuthorityInstanceId([OPAQUE])")
    }
}

/// Canonical bytes for a settings draft that has already passed the settings
/// schema's non-secret validation boundary.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ValidatedNonSecretSettingsDraft(Box<[u8]>);

impl ValidatedNonSecretSettingsDraft {
    pub(crate) fn from_validated_bytes(bytes: Vec<u8>) -> Self {
        Self(bytes.into_boxed_slice())
    }

    pub(crate) fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for ValidatedNonSecretSettingsDraft {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ValidatedNonSecretSettingsDraft([REDACTED])")
    }
}

/// One authority identity and the journal validated from that same atomic
/// adapter load. There is deliberately no later identity accessor on a
/// mutation session: consumers must receive both values together.
pub(crate) struct LoadedAuthorityJournal {
    authority_instance_id: CredentialAuthorityInstanceId,
    journal: AuthorityJournal,
}

impl LoadedAuthorityJournal {
    pub(super) fn new(
        authority_instance_id: CredentialAuthorityInstanceId,
        journal: AuthorityJournal,
    ) -> Self {
        Self {
            authority_instance_id,
            journal,
        }
    }

    pub(crate) fn into_parts(self) -> (CredentialAuthorityInstanceId, AuthorityJournal) {
        (self.authority_instance_id, self.journal)
    }
}

impl fmt::Debug for LoadedAuthorityJournal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("LoadedAuthorityJournal([OPAQUE])")
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
        let material_matches_auth_method = matches!(
            (auth_method_id, &material),
            (AuthMethodId::ApiKey, StoredSecretBundle::ApiKey { .. })
                | (
                    AuthMethodId::AwsStatic,
                    StoredSecretBundle::AwsStatic { .. }
                )
        );
        let valid_shape =
            stored_auth_method_matches_set(&set_id, auth_method_id) && material_matches_auth_method;
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
    PermissionHardeningFailed,
    Unsupported,
    CorruptRecord,
    UnsupportedSchema,
    PayloadTooLarge,
    AmbiguousMatch,
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
            Self::PermissionHardeningFailed => (
                CredentialErrorCode::PermissionHardeningFailed,
                CredentialSafeRecoveryAction::RepairPermissions,
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
            Self::AmbiguousMatch => (
                CredentialErrorCode::AmbiguousMatch,
                CredentialSafeRecoveryAction::Reconcile,
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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub(crate) struct PendingCredentialIntent {
    pub(crate) operation_id: CredentialOperationId,
    pub(crate) idempotency_token: CredentialIdempotencyToken,
    pub(crate) set_id: CredentialSetId,
    pub(crate) mutation_kind: CredentialMutationKind,
    pub(crate) expected_revision: Option<CredentialRevision>,
    pub(crate) proposed_revision: CredentialRevision,
    pub(crate) recovery_state: CredentialSetRecoveryState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct IdempotencyJournalEntry {
    pub(crate) idempotency_token: CredentialIdempotencyToken,
    pub(crate) set_id: CredentialSetId,
    pub(crate) mutation_kind: CredentialMutationKind,
    pub(crate) expected_revision: Option<CredentialRevision>,
    pub(crate) receipt: CredentialMutationReceipt,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdempotencyJournalEntryWire {
    idempotency_token: CredentialIdempotencyToken,
    set_id: CredentialSetId,
    mutation_kind: CredentialMutationKind,
    expected_revision: Option<CredentialRevision>,
    receipt: CredentialMutationReceiptWire,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialMutationReceiptWire {
    operation_id: CredentialOperationId,
    idempotency_token: CredentialIdempotencyToken,
    set_id: CredentialSetId,
    previous_revision: Option<CredentialRevision>,
    new_revision: Option<CredentialRevision>,
    result_code: CredentialMutationResultCode,
    recovery_action: CredentialSafeRecoveryAction,
}

impl<'de> Deserialize<'de> for IdempotencyJournalEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = IdempotencyJournalEntryWire::deserialize(deserializer)?;
        Ok(Self {
            idempotency_token: wire.idempotency_token,
            set_id: wire.set_id,
            mutation_kind: wire.mutation_kind,
            expected_revision: wire.expected_revision,
            receipt: CredentialMutationReceipt {
                operation_id: wire.receipt.operation_id,
                idempotency_token: wire.receipt.idempotency_token,
                set_id: wire.receipt.set_id,
                previous_revision: wire.receipt.previous_revision,
                new_revision: wire.receipt.new_revision,
                result_code: wire.receipt.result_code,
                recovery_action: wire.receipt.recovery_action,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingSettingsActivation {
    pub(crate) operation_id: CredentialOperationId,
    pub(crate) idempotency_token: CredentialIdempotencyToken,
    pub(crate) set_id: CredentialSetId,
    pub(crate) auth_method_id: AuthMethodId,
    pub(crate) expected_revision: Option<CredentialRevision>,
    pub(crate) proposed_revision: CredentialRevision,
    pub(crate) expected_settings_revision: u64,
    pub(crate) proposed_settings_revision: u64,
    pub(crate) expected_global_epoch: u64,
    pub(crate) proposed_global_epoch: u64,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthorityJournalWire {
    schema_version: u32,
    global_epoch: u64,
    backend: CredentialBackendStatusWire,
    migration_state: CredentialMigrationState,
    cleanup_state: CredentialCleanupState,
    pending_activation: Option<PendingSettingsActivation>,
    sets: Vec<JournalSetState>,
    pending_intents: Vec<PendingCredentialIntent>,
    idempotency_history: Vec<IdempotencyJournalEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialBackendStatusWire {
    kind: CredentialBackendKind,
    availability: CredentialBackendAvailability,
}

impl<'de> Deserialize<'de> for AuthorityJournal {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = AuthorityJournalWire::deserialize(deserializer)?;
        Ok(Self {
            schema_version: wire.schema_version,
            global_epoch: wire.global_epoch,
            backend: CredentialBackendStatus {
                kind: wire.backend.kind,
                availability: wire.backend.availability,
            },
            migration_state: wire.migration_state,
            cleanup_state: wire.cleanup_state,
            pending_activation: wire.pending_activation,
            sets: wire.sets,
            pending_intents: wire.pending_intents,
            idempotency_history: wire.idempotency_history,
        })
    }
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

    /// Validate the non-secret persistence envelope before composition uses it
    /// as authority. Serde proves only that individual fields have valid wire
    /// shapes; it does not prove their cross-field authority invariants.
    pub(crate) fn validate_persisted_for_backend(
        &self,
        backend_kind: CredentialBackendKind,
    ) -> Result<(), CredentialStoreFailure> {
        if self.schema_version != RECORD_SCHEMA_VERSION
            || self.backend.kind != backend_kind
            || self.sets.len() > MAX_PERSISTED_CREDENTIAL_SETS
            || self.pending_intents.len() > MAX_PENDING_CREDENTIAL_INTENTS
            || self.idempotency_history.len() > MAX_IDEMPOTENCY_HISTORY
        {
            return Err(CredentialStoreFailure::CorruptRecord);
        }

        let unique_set_ids: HashSet<&CredentialSetId> =
            self.sets.iter().map(|set| &set.set_id).collect();
        if unique_set_ids.len() != self.sets.len()
            || !BUILT_IN_CREDENTIAL_SET_IDS
                .iter()
                .all(|built_in| unique_set_ids.contains(&CredentialSetId::BuiltIn(*built_in)))
            || self.sets.iter().any(|set| !set.has_coherent_record_shape())
            || !self.pending_intents_are_coherent()
            || !self.pending_activation_is_coherent()
            || !self.idempotency_history_is_coherent()
        {
            return Err(CredentialStoreFailure::CorruptRecord);
        }

        Ok(())
    }

    fn pending_intents_are_coherent(&self) -> bool {
        if self.pending_intents.len() > self.sets.len() {
            return false;
        }

        let mut operation_ids = HashSet::with_capacity(self.pending_intents.len());
        let mut idempotency_tokens = HashSet::with_capacity(self.pending_intents.len());
        let mut set_ids = HashSet::with_capacity(self.pending_intents.len());
        for intent in &self.pending_intents {
            let Some(set) = self.set_state(&intent.set_id) else {
                return false;
            };
            if intent.recovery_state != CredentialSetRecoveryState::PendingIntent
                || set.recovery_state == CredentialSetRecoveryState::None
                || intent.expected_revision.as_ref() == Some(&intent.proposed_revision)
                || !operation_ids.insert(&intent.operation_id)
                || !idempotency_tokens.insert(&intent.idempotency_token)
                || !set_ids.insert(&intent.set_id)
            {
                return false;
            }

            match intent.mutation_kind {
                CredentialMutationKind::Replace => {
                    if set.pending_activation
                        || set.recovery_state != CredentialSetRecoveryState::PendingIntent
                        || set.revision != intent.expected_revision
                    {
                        return false;
                    }
                }
                CredentialMutationKind::Delete => {
                    if set.pending_activation
                        || set.recovery_state != CredentialSetRecoveryState::PendingIntent
                        || intent.expected_revision.is_none()
                        || set.revision != intent.expected_revision
                    {
                        return false;
                    }
                }
                CredentialMutationKind::Activate => {}
            }
        }

        self.sets.iter().all(|set| {
            set.recovery_state != CredentialSetRecoveryState::PendingIntent
                || set_ids.contains(&set.set_id)
        })
    }

    fn pending_activation_is_coherent(&self) -> bool {
        let activation_intents: Vec<&PendingCredentialIntent> = self
            .pending_intents
            .iter()
            .filter(|intent| intent.mutation_kind == CredentialMutationKind::Activate)
            .collect();
        let flagged_sets: Vec<&JournalSetState> = self
            .sets
            .iter()
            .filter(|set| set.pending_activation)
            .collect();

        let Some(pending) = self.pending_activation.as_ref() else {
            return activation_intents.is_empty() && flagged_sets.is_empty();
        };
        let [intent] = activation_intents.as_slice() else {
            return false;
        };
        if self.pending_intents.len() != 1 {
            return false;
        }
        let [flagged_set] = flagged_sets.as_slice() else {
            return false;
        };
        if intent.operation_id != pending.operation_id
            || intent.idempotency_token != pending.idempotency_token
            || intent.set_id != pending.set_id
            || intent.expected_revision != pending.expected_revision
            || intent.proposed_revision != pending.proposed_revision
            || flagged_set.set_id != pending.set_id
            || !stored_auth_method_matches_set(&pending.set_id, pending.auth_method_id)
            || pending.proposed_settings_revision <= pending.expected_settings_revision
            || pending.expected_global_epoch != self.global_epoch
            || pending.expected_global_epoch.checked_add(1) != Some(pending.proposed_global_epoch)
        {
            return false;
        }

        match pending.stage {
            CredentialActivationStage::CleanupPending => {
                flagged_set.recovery_state == CredentialSetRecoveryState::PendingIntent
                    && flagged_set.record_state == CredentialSetRecordState::Configured
                    && flagged_set.source == CredentialSetSource::NativeV2
                    && flagged_set.revision.as_ref() == Some(&pending.proposed_revision)
            }
            CredentialActivationStage::RecoveryRequired => {
                flagged_set.recovery_state == CredentialSetRecoveryState::CommitUnknown
                    && (flagged_set.revision == pending.expected_revision
                        || flagged_set.revision.as_ref() == Some(&pending.proposed_revision))
            }
            CredentialActivationStage::Staged
            | CredentialActivationStage::SettingsPending
            | CredentialActivationStage::CredentialPending => {
                flagged_set.recovery_state == CredentialSetRecoveryState::PendingIntent
                    && flagged_set.revision == pending.expected_revision
            }
        }
    }

    fn idempotency_history_is_coherent(&self) -> bool {
        let pending_tokens: HashSet<&CredentialIdempotencyToken> = self
            .pending_intents
            .iter()
            .map(|intent| &intent.idempotency_token)
            .collect();
        let pending_operations: HashSet<&CredentialOperationId> = self
            .pending_intents
            .iter()
            .map(|intent| &intent.operation_id)
            .collect();
        let mut history_tokens = HashSet::with_capacity(self.idempotency_history.len());
        let mut history_operations = HashSet::with_capacity(self.idempotency_history.len());

        self.idempotency_history.iter().all(|entry| {
            let receipt = &entry.receipt;
            self.set_state(&entry.set_id).is_some()
                && history_tokens.insert(&entry.idempotency_token)
                && history_operations.insert(&receipt.operation_id)
                && !pending_tokens.contains(&entry.idempotency_token)
                && !pending_operations.contains(&receipt.operation_id)
                && receipt.idempotency_token == entry.idempotency_token
                && receipt.set_id == entry.set_id
                && receipt.previous_revision == entry.expected_revision
                && receipt.recovery_action == CredentialSafeRecoveryAction::None
                && entry.receipt_shape_matches_kind()
        })
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

impl JournalSetState {
    fn has_coherent_record_shape(&self) -> bool {
        #[allow(unreachable_patterns)]
        match self.record_state {
            CredentialSetRecordState::Missing => {
                self.revision.is_none() && self.source == CredentialSetSource::None
            }
            CredentialSetRecordState::Configured | CredentialSetRecordState::Tombstoned => {
                self.revision.is_some() && self.source != CredentialSetSource::None
            }
            CredentialSetRecordState::RecoveryRequired => {
                self.recovery_state != CredentialSetRecoveryState::None
            }
            // Runtime-only/pre-authority states (including the parallel
            // `Unknown` addition) must never authorize a persisted journal.
            _ => false,
        }
    }
}

impl IdempotencyJournalEntry {
    fn receipt_shape_matches_kind(&self) -> bool {
        let receipt = &self.receipt;
        let result_matches_kind = match self.mutation_kind {
            CredentialMutationKind::Replace => matches!(
                receipt.result_code,
                CredentialMutationResultCode::Created | CredentialMutationResultCode::Replaced
            ),
            CredentialMutationKind::Delete => {
                receipt.result_code == CredentialMutationResultCode::Tombstoned
            }
            CredentialMutationKind::Activate => matches!(
                receipt.result_code,
                CredentialMutationResultCode::Created
                    | CredentialMutationResultCode::Replaced
                    | CredentialMutationResultCode::Recovered
                    | CredentialMutationResultCode::NoChange
            ),
        };
        let revisions_match_result = match receipt.result_code {
            CredentialMutationResultCode::Created => {
                receipt.previous_revision.is_none() && receipt.new_revision.is_some()
            }
            CredentialMutationResultCode::Replaced | CredentialMutationResultCode::Tombstoned => {
                receipt.previous_revision.is_some()
                    && receipt.new_revision.is_some()
                    && receipt.new_revision != receipt.previous_revision
            }
            CredentialMutationResultCode::Recovered => {
                receipt.new_revision.is_some() && receipt.new_revision != receipt.previous_revision
            }
            CredentialMutationResultCode::NoChange => {
                receipt.new_revision == receipt.previous_revision
            }
            CredentialMutationResultCode::AlreadyApplied => false,
        };
        result_matches_kind && revisions_match_result
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthorityJournal, CredentialMutationKind, CredentialRecordEnvelope, CredentialStoreFailure,
        EncodedCredentialRecord, IdempotencyJournalEntry, JournalSetState, PendingCredentialIntent,
        PendingSettingsActivation, StoredSecretBundle,
    };
    use audio_graph_ipc_contract::credential_contract::{
        AuthMethodId, BuiltInCredentialSetId, CredentialActivationStage, CredentialBackendKind,
        CredentialErrorCode, CredentialIdempotencyToken, CredentialMutationReceipt,
        CredentialMutationResultCode, CredentialOperationId, CredentialRevision,
        CredentialSafeRecoveryAction, CredentialSetId, CredentialSetRecordState,
        CredentialSetRecoveryState, CredentialSetSource, PORTABLE_ENCODED_RECORD_MAX_BYTES,
    };

    fn revision(value: &str) -> CredentialRevision {
        CredentialRevision::parse(value).expect("canonical revision")
    }

    fn operation(value: &str) -> CredentialOperationId {
        CredentialOperationId::parse(value).expect("canonical operation id")
    }

    fn idempotency(value: &str) -> CredentialIdempotencyToken {
        CredentialIdempotencyToken::parse(value).expect("canonical idempotency token")
    }

    #[test]
    fn permission_hardening_failure_maps_to_repair_permissions_without_retry() {
        let error = CredentialStoreFailure::PermissionHardeningFailed.into_public(None);

        assert_eq!(error.code, CredentialErrorCode::PermissionHardeningFailed);
        assert_eq!(
            error.recovery_action,
            CredentialSafeRecoveryAction::RepairPermissions
        );
        assert!(!error.retryable);
        assert!(error.set_id.is_none());
    }

    #[test]
    fn unavailable_store_failure_remains_retryable_without_permission_remediation() {
        let error = CredentialStoreFailure::Unavailable.into_public(None);

        assert_eq!(error.code, CredentialErrorCode::StoreUnavailable);
        assert_eq!(error.recovery_action, CredentialSafeRecoveryAction::Retry);
        assert!(error.retryable);
        assert!(error.set_id.is_none());
    }

    fn pending_replace_journal() -> AuthorityJournal {
        let mut journal = AuthorityJournal::new(CredentialBackendKind::Native);
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        journal.pending_intents.push(PendingCredentialIntent {
            operation_id: operation("aaaaaaaa-0000-4000-8000-000000000001"),
            idempotency_token: idempotency("bbbbbbbb-0000-4000-8000-000000000001"),
            set_id: set_id.clone(),
            mutation_kind: CredentialMutationKind::Replace,
            expected_revision: None,
            proposed_revision: revision("cccccccc-0000-4000-8000-000000000001"),
            recovery_state: CredentialSetRecoveryState::PendingIntent,
        });
        journal
            .set_state_mut(&set_id)
            .expect("built-in set state")
            .recovery_state = CredentialSetRecoveryState::PendingIntent;
        journal
    }

    fn pending_activation_journal(
        stage: CredentialActivationStage,
        proposed_is_committed: bool,
    ) -> AuthorityJournal {
        let mut journal = AuthorityJournal::new(CredentialBackendKind::Native);
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        let operation_id = operation("dddddddd-0000-4000-8000-000000000001");
        let idempotency_token = idempotency("eeeeeeee-0000-4000-8000-000000000001");
        let proposed_revision = revision("ffffffff-0000-4000-8000-000000000001");
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
            auth_method_id: AuthMethodId::ApiKey,
            expected_revision: None,
            proposed_revision: proposed_revision.clone(),
            expected_settings_revision: 10,
            proposed_settings_revision: 11,
            expected_global_epoch: 0,
            proposed_global_epoch: 1,
            stage,
        });
        let set = journal.set_state_mut(&set_id).expect("built-in set state");
        set.pending_activation = true;
        set.recovery_state = if stage == CredentialActivationStage::RecoveryRequired {
            CredentialSetRecoveryState::CommitUnknown
        } else {
            CredentialSetRecoveryState::PendingIntent
        };
        if proposed_is_committed {
            set.record_state = CredentialSetRecordState::Configured;
            set.source = CredentialSetSource::NativeV2;
            set.revision = Some(proposed_revision);
        }
        journal
    }

    fn completed_replace_journal() -> AuthorityJournal {
        let mut journal = AuthorityJournal::new(CredentialBackendKind::Native);
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Openai);
        let operation_id = operation("40000000-0000-4000-8000-000000000001");
        let idempotency_token = idempotency("50000000-0000-4000-8000-000000000001");
        let new_revision = revision("60000000-0000-4000-8000-000000000001");
        let receipt = CredentialMutationReceipt {
            operation_id,
            idempotency_token: idempotency_token.clone(),
            set_id: set_id.clone(),
            previous_revision: None,
            new_revision: Some(new_revision.clone()),
            result_code: CredentialMutationResultCode::Created,
            recovery_action: CredentialSafeRecoveryAction::None,
        };
        journal.idempotency_history.push(IdempotencyJournalEntry {
            idempotency_token,
            set_id: set_id.clone(),
            mutation_kind: CredentialMutationKind::Replace,
            expected_revision: None,
            receipt,
        });
        let set = journal.set_state_mut(&set_id).expect("built-in set state");
        set.record_state = CredentialSetRecordState::Configured;
        set.source = CredentialSetSource::NativeV2;
        set.revision = Some(new_revision);
        journal.global_epoch = 1;
        journal
    }

    #[test]
    fn persisted_journal_rejects_incoherent_state_revision_and_source_rows() {
        let committed_revision = revision("10101010-2020-3030-4040-505050505050");
        let rows = [
            (
                "missing_with_revision",
                CredentialSetRecordState::Missing,
                CredentialSetSource::None,
                Some(committed_revision.clone()),
                CredentialSetRecoveryState::None,
            ),
            (
                "missing_with_source",
                CredentialSetRecordState::Missing,
                CredentialSetSource::NativeV2,
                None,
                CredentialSetRecoveryState::None,
            ),
            (
                "configured_without_revision",
                CredentialSetRecordState::Configured,
                CredentialSetSource::NativeV2,
                None,
                CredentialSetRecoveryState::None,
            ),
            (
                "configured_without_source",
                CredentialSetRecordState::Configured,
                CredentialSetSource::None,
                Some(committed_revision.clone()),
                CredentialSetRecoveryState::None,
            ),
            (
                "tombstoned_without_revision",
                CredentialSetRecordState::Tombstoned,
                CredentialSetSource::NativeV2,
                None,
                CredentialSetRecoveryState::None,
            ),
            (
                "recovery_record_without_recovery_state",
                CredentialSetRecordState::RecoveryRequired,
                CredentialSetSource::None,
                None,
                CredentialSetRecoveryState::None,
            ),
        ];

        for (name, record_state, source, revision, recovery_state) in rows {
            let mut journal = AuthorityJournal::new(CredentialBackendKind::Native);
            let set = &mut journal.sets[0];
            set.record_state = record_state;
            set.source = source;
            set.revision = revision;
            set.recovery_state = recovery_state;
            assert!(
                journal
                    .validate_persisted_for_backend(CredentialBackendKind::Native)
                    .is_err(),
                "accepted incoherent row {name}"
            );
        }
    }

    #[test]
    fn persisted_journal_bounds_and_cross_checks_pending_intents() {
        let valid = pending_replace_journal();
        assert!(
            valid
                .validate_persisted_for_backend(CredentialBackendKind::Native)
                .is_ok(),
            "one coherent pending replacement is a legitimate recovery state"
        );

        fn duplicate_operation(journal: &mut AuthorityJournal) {
            let mut duplicate = journal.pending_intents[0].clone();
            duplicate.set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
            duplicate.idempotency_token = idempotency("bbbbbbbb-0000-4000-8000-000000000002");
            duplicate.proposed_revision = revision("cccccccc-0000-4000-8000-000000000002");
            journal
                .set_state_mut(&duplicate.set_id)
                .expect("built-in set state")
                .recovery_state = CredentialSetRecoveryState::PendingIntent;
            journal.pending_intents.push(duplicate);
        }

        fn duplicate_set(journal: &mut AuthorityJournal) {
            let mut duplicate = journal.pending_intents[0].clone();
            duplicate.operation_id = operation("aaaaaaaa-0000-4000-8000-000000000002");
            duplicate.idempotency_token = idempotency("bbbbbbbb-0000-4000-8000-000000000002");
            duplicate.proposed_revision = revision("cccccccc-0000-4000-8000-000000000002");
            journal.pending_intents.push(duplicate);
        }

        fn duplicate_token(journal: &mut AuthorityJournal) {
            let mut duplicate = journal.pending_intents[0].clone();
            duplicate.operation_id = operation("aaaaaaaa-0000-4000-8000-000000000002");
            duplicate.set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
            duplicate.proposed_revision = revision("cccccccc-0000-4000-8000-000000000002");
            journal
                .set_state_mut(&duplicate.set_id)
                .expect("built-in set state")
                .recovery_state = CredentialSetRecoveryState::PendingIntent;
            journal.pending_intents.push(duplicate);
        }

        fn unknown_set(journal: &mut AuthorityJournal) {
            journal.pending_intents[0].set_id = "custom.00000000-0000-0000-0000-000000000001"
                .parse()
                .expect("canonical custom set id");
        }

        fn replace_with_commit_unknown(journal: &mut AuthorityJournal) {
            journal.sets[0].recovery_state = CredentialSetRecoveryState::CommitUnknown;
        }

        fn unbounded(journal: &mut AuthorityJournal) {
            *journal = AuthorityJournal::new(CredentialBackendKind::Native);
            for index in 1_u64..=65 {
                let set_id: CredentialSetId =
                    format!("custom.00000000-0000-0000-0000-{index:012x}")
                        .parse()
                        .expect("canonical custom set id");
                let mut set_state: JournalSetState = journal.sets[0].clone();
                set_state.set_id = set_id.clone();
                set_state.recovery_state = CredentialSetRecoveryState::PendingIntent;
                journal.sets.push(set_state);
                journal.pending_intents.push(PendingCredentialIntent {
                    operation_id: operation(&format!("10000000-0000-4000-8000-{index:012x}")),
                    idempotency_token: idempotency(&format!(
                        "20000000-0000-4000-8000-{index:012x}"
                    )),
                    set_id,
                    mutation_kind: CredentialMutationKind::Replace,
                    expected_revision: None,
                    proposed_revision: revision(&format!("30000000-0000-4000-8000-{index:012x}")),
                    recovery_state: CredentialSetRecoveryState::PendingIntent,
                });
            }
        }

        for (name, mutate) in [
            (
                "duplicate_operation",
                duplicate_operation as fn(&mut AuthorityJournal),
            ),
            ("duplicate_set", duplicate_set),
            ("duplicate_token", duplicate_token),
            ("unknown_set", unknown_set),
            ("replace_with_commit_unknown", replace_with_commit_unknown),
            ("unbounded", unbounded),
        ] {
            let mut journal = valid.clone();
            mutate(&mut journal);
            assert!(
                journal
                    .validate_persisted_for_backend(CredentialBackendKind::Native)
                    .is_err(),
                "accepted invalid pending-intent case {name}"
            );
        }
    }

    #[test]
    fn activation_absent_allows_coherent_multi_set_replace_and_delete_intents() {
        let mut journal = pending_replace_journal();
        let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        let current_revision = revision("71717171-8282-4393-84a4-b5b5b5b5b5b5");
        let proposed_revision = revision("72727272-8383-44a4-85b5-c6c6c6c6c6c6");
        journal.pending_intents.push(PendingCredentialIntent {
            operation_id: operation("73737373-8484-45b5-86c6-d7d7d7d7d7d7"),
            idempotency_token: idempotency("74747474-8585-46c6-87d7-e8e8e8e8e8e8"),
            set_id: set_id.clone(),
            mutation_kind: CredentialMutationKind::Delete,
            expected_revision: Some(current_revision.clone()),
            proposed_revision,
            recovery_state: CredentialSetRecoveryState::PendingIntent,
        });
        let set = journal.set_state_mut(&set_id).expect("built-in set state");
        set.record_state = CredentialSetRecordState::Configured;
        set.source = CredentialSetSource::NativeV2;
        set.revision = Some(current_revision);
        set.recovery_state = CredentialSetRecoveryState::PendingIntent;

        assert!(journal.pending_activation.is_none());
        assert!(
            journal
                .validate_persisted_for_backend(CredentialBackendKind::Native)
                .is_ok(),
            "activation-absent replace/delete recovery may span distinct sets"
        );
    }

    #[test]
    fn persisted_pending_activation_excludes_unrelated_pending_mutations_globally() {
        for (name, mutation_kind, expected_revision) in [
            ("replace", CredentialMutationKind::Replace, None),
            (
                "delete",
                CredentialMutationKind::Delete,
                Some(revision("75757575-8686-47d7-88e8-f9f9f9f9f9f9")),
            ),
        ] {
            let mut journal = pending_activation_journal(CredentialActivationStage::Staged, false);
            let set_id = CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
            journal.pending_intents.push(PendingCredentialIntent {
                operation_id: operation("76767676-8787-48e8-89f9-a0a0a0a0a0a0"),
                idempotency_token: idempotency("77777777-8888-49f9-8a0a-b1b1b1b1b1b1"),
                set_id: set_id.clone(),
                mutation_kind,
                expected_revision: expected_revision.clone(),
                proposed_revision: revision("78787878-8989-4a0a-8b1b-c2c2c2c2c2c2"),
                recovery_state: CredentialSetRecoveryState::PendingIntent,
            });
            let set = journal.set_state_mut(&set_id).expect("built-in set state");
            if let Some(current_revision) = expected_revision {
                set.record_state = CredentialSetRecordState::Configured;
                set.source = CredentialSetSource::NativeV2;
                set.revision = Some(current_revision);
            }
            set.recovery_state = CredentialSetRecoveryState::PendingIntent;

            assert!(
                journal
                    .validate_persisted_for_backend(CredentialBackendKind::Native)
                    .is_err(),
                "accepted pending activation plus unrelated {name} intent"
            );
        }
    }

    #[test]
    fn persisted_journal_cross_checks_pending_activation_and_legitimate_recovery_stages() {
        for (stage, proposed_is_committed) in [
            (CredentialActivationStage::Staged, false),
            (CredentialActivationStage::SettingsPending, false),
            (CredentialActivationStage::CredentialPending, false),
            (CredentialActivationStage::CleanupPending, true),
            (CredentialActivationStage::RecoveryRequired, false),
            (CredentialActivationStage::RecoveryRequired, true),
        ] {
            let journal = pending_activation_journal(stage, proposed_is_committed);
            assert!(
                journal
                    .validate_persisted_for_backend(CredentialBackendKind::Native)
                    .is_ok(),
                "rejected legitimate activation state {stage:?}, proposed={proposed_is_committed}"
            );
        }

        fn activation_intent_without_global(journal: &mut AuthorityJournal) {
            journal.pending_activation = None;
            let target = journal.pending_intents[0].set_id.clone();
            let set = journal.set_state_mut(&target).expect("target set");
            set.pending_activation = false;
            set.recovery_state = CredentialSetRecoveryState::RecordJournalMismatch;
        }

        fn operation_mismatch(journal: &mut AuthorityJournal) {
            journal
                .pending_activation
                .as_mut()
                .expect("pending activation")
                .operation_id = operation("dddddddd-0000-4000-8000-000000000002");
        }

        fn token_mismatch(journal: &mut AuthorityJournal) {
            journal
                .pending_activation
                .as_mut()
                .expect("pending activation")
                .idempotency_token = idempotency("eeeeeeee-0000-4000-8000-000000000002");
        }

        fn settings_revision_not_advanced(journal: &mut AuthorityJournal) {
            journal
                .pending_activation
                .as_mut()
                .expect("pending activation")
                .proposed_settings_revision = 10;
        }

        fn auth_method_does_not_match_built_in_set(journal: &mut AuthorityJournal) {
            journal
                .pending_activation
                .as_mut()
                .expect("pending activation")
                .auth_method_id = AuthMethodId::AwsStatic;
        }

        fn global_epoch_drifted_past_reservation(journal: &mut AuthorityJournal) {
            journal.global_epoch = 1;
        }

        fn second_pending_flag(journal: &mut AuthorityJournal) {
            journal.sets[1].pending_activation = true;
        }

        fn staged_with_commit_unknown(journal: &mut AuthorityJournal) {
            journal.sets[0].recovery_state = CredentialSetRecoveryState::CommitUnknown;
        }

        fn cleanup_before_proposed_revision(journal: &mut AuthorityJournal) {
            journal
                .pending_activation
                .as_mut()
                .expect("pending activation")
                .stage = CredentialActivationStage::CleanupPending;
        }

        for (name, mutate) in [
            (
                "activation_intent_without_global",
                activation_intent_without_global as fn(&mut AuthorityJournal),
            ),
            ("operation_mismatch", operation_mismatch),
            ("token_mismatch", token_mismatch),
            (
                "settings_revision_not_advanced",
                settings_revision_not_advanced,
            ),
            (
                "auth_method_does_not_match_built_in_set",
                auth_method_does_not_match_built_in_set,
            ),
            (
                "global_epoch_drifted_past_reservation",
                global_epoch_drifted_past_reservation,
            ),
            ("second_pending_flag", second_pending_flag),
            ("staged_with_commit_unknown", staged_with_commit_unknown),
            (
                "cleanup_before_proposed_revision",
                cleanup_before_proposed_revision,
            ),
        ] {
            let mut journal = pending_activation_journal(CredentialActivationStage::Staged, false);
            mutate(&mut journal);
            assert!(
                journal
                    .validate_persisted_for_backend(CredentialBackendKind::Native)
                    .is_err(),
                "accepted invalid pending-activation case {name}"
            );
        }
    }

    #[test]
    fn persisted_journal_cross_checks_idempotency_receipts_and_keys() {
        let valid = completed_replace_journal();
        assert!(
            valid
                .validate_persisted_for_backend(CredentialBackendKind::Native)
                .is_ok()
        );

        fn duplicate_token(journal: &mut AuthorityJournal) {
            let mut duplicate = journal.idempotency_history[0].clone();
            duplicate.receipt.operation_id = operation("40000000-0000-4000-8000-000000000002");
            journal.idempotency_history.push(duplicate);
        }

        fn duplicate_operation(journal: &mut AuthorityJournal) {
            let mut duplicate = journal.idempotency_history[0].clone();
            duplicate.idempotency_token = idempotency("50000000-0000-4000-8000-000000000002");
            duplicate.receipt.idempotency_token = duplicate.idempotency_token.clone();
            journal.idempotency_history.push(duplicate);
        }

        fn receipt_token_mismatch(journal: &mut AuthorityJournal) {
            journal.idempotency_history[0].receipt.idempotency_token =
                idempotency("50000000-0000-4000-8000-000000000002");
        }

        fn receipt_set_mismatch(journal: &mut AuthorityJournal) {
            journal.idempotency_history[0].receipt.set_id =
                CredentialSetId::from(BuiltInCredentialSetId::Deepgram);
        }

        fn expected_revision_mismatch(journal: &mut AuthorityJournal) {
            journal.idempotency_history[0].expected_revision =
                Some(revision("60000000-0000-4000-8000-000000000002"));
        }

        fn kind_result_mismatch(journal: &mut AuthorityJournal) {
            journal.idempotency_history[0].mutation_kind = CredentialMutationKind::Delete;
        }

        fn created_without_new_revision(journal: &mut AuthorityJournal) {
            journal.idempotency_history[0].receipt.new_revision = None;
        }

        fn unsafe_recovery_action(journal: &mut AuthorityJournal) {
            journal.idempotency_history[0].receipt.recovery_action =
                CredentialSafeRecoveryAction::Reconcile;
        }

        for (name, mutate) in [
            (
                "duplicate_token",
                duplicate_token as fn(&mut AuthorityJournal),
            ),
            ("duplicate_operation", duplicate_operation),
            ("receipt_token_mismatch", receipt_token_mismatch),
            ("receipt_set_mismatch", receipt_set_mismatch),
            ("expected_revision_mismatch", expected_revision_mismatch),
            ("kind_result_mismatch", kind_result_mismatch),
            ("created_without_new_revision", created_without_new_revision),
            ("unsafe_recovery_action", unsafe_recovery_action),
        ] {
            let mut journal = valid.clone();
            mutate(&mut journal);
            assert!(
                journal
                    .validate_persisted_for_backend(CredentialBackendKind::Native)
                    .is_err(),
                "accepted incoherent idempotency case {name}"
            );
        }
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
