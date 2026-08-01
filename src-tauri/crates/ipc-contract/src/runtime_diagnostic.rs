//! Closed, content-free runtime diagnostics (Seed `audio-graph-ec13`).
//!
//! This module is the public boundary between provider/native failures and
//! IPC, logs, status events, and other exportable support surfaces. Raw
//! provider or native data may be inspected privately to choose a variant, but
//! it cannot be stored in these types: every exported value is a closed enum,
//! boolean, or the accepted credential-service error tuple.

use crate::credential_contract::CredentialError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Defines a closed wire vocabulary and an exhaustive exported slice from one
/// declaration. A new variant cannot be added without joining the slice used
/// by contract consumers and tests.
macro_rules! closed_vocabulary {
    ($(#[$meta:meta])* $vis:vis enum $name:ident => $values:ident {
        $($variant:ident => $wire:literal),+ $(,)?
    }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        $vis enum $name {
            $(#[serde(rename = $wire)] $variant),+
        }

        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $wire),+
                }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        pub const $values: &[$name] = &[$($name::$variant),+];
    };
}

closed_vocabulary! {
    /// Structural category for a non-credential runtime failure.
    pub enum RuntimeErrorCode => RUNTIME_ERROR_CODES {
        RateLimited => "rate_limited",
        NetworkUnreachable => "network_unreachable",
        Timeout => "timeout",
        ProviderUnavailable => "provider_unavailable",
        ProtocolError => "protocol_error",
        InvalidResponse => "invalid_response",
        RequestRejected => "request_rejected",
        CapacityExhausted => "capacity_exhausted",
        PolicyBlocked => "policy_blocked",
        Unsupported => "unsupported",
        Cancelled => "cancelled",
        Internal => "internal",
    }
}

closed_vocabulary! {
    /// Safe action a renderer or operator can offer for a runtime failure.
    pub enum RuntimeSafeRecoveryAction => RUNTIME_RECOVERY_ACTIONS {
        None => "none",
        Retry => "retry",
        RetryAfterDelay => "retry_after_delay",
        CheckNetwork => "check_network",
        ReviewConfiguration => "review_configuration",
        ChooseSupportedProvider => "choose_supported_provider",
        ReviewPolicy => "review_policy",
        RestartApplication => "restart_application",
    }
}

closed_vocabulary! {
    /// Closed operation identifier. It deliberately identifies a capability,
    /// not a provider, URL, route, model, filesystem location, or request.
    pub enum RuntimeOperation => RUNTIME_OPERATIONS {
        ProviderReadiness => "provider_readiness",
        ModelDiscovery => "model_discovery",
        Transcription => "transcription",
        SpeechSynthesis => "speech_synthesis",
        RealtimeConversation => "realtime_conversation",
        LanguageModelInference => "language_model_inference",
        ProviderRequest => "provider_request",
    }
}

closed_vocabulary! {
    /// Transport family without an endpoint, route, or native implementation
    /// name.
    pub enum RuntimeTransport => RUNTIME_TRANSPORTS {
        Native => "native",
        Http => "http",
        Websocket => "websocket",
        Sdk => "sdk",
    }
}

closed_vocabulary! {
    /// Coarse HTTP response class. Exact status values are intentionally not
    /// part of the public diagnostic context.
    pub enum RuntimeStatusClass => RUNTIME_STATUS_CLASSES {
        Redirect => "redirect",
        ClientError => "client_error",
        ServerError => "server_error",
    }
}

closed_vocabulary! {
    /// Saturating retry-delay bucket. Provider-supplied exact durations never
    /// cross the public boundary.
    pub enum RuntimeRetryDelayBucket => RUNTIME_RETRY_DELAY_BUCKETS {
        Immediate => "immediate",
        Short => "short",
        Medium => "medium",
        Long => "long",
    }
}

impl RuntimeRetryDelayBucket {
    /// Reduce an exact private retry delay to a stable, bounded wire value.
    pub const fn from_seconds(seconds: u64) -> Self {
        match seconds {
            0 => Self::Immediate,
            1..=5 => Self::Short,
            6..=30 => Self::Medium,
            _ => Self::Long,
        }
    }
}

/// Optional public context for a runtime failure. Every member is closed and
/// bounded; there is no escape hatch for provider/native prose, response
/// bodies, paths, request identifiers, fingerprints, or exact content sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeDiagnosticContext {
    pub operation: RuntimeOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<RuntimeTransport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_class: Option<RuntimeStatusClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_delay: Option<RuntimeRetryDelayBucket>,
}

impl RuntimeDiagnosticContext {
    pub const fn new(operation: RuntimeOperation) -> Self {
        Self {
            operation,
            transport: None,
            status_class: None,
            retry_delay: None,
        }
    }

    pub const fn with_transport(mut self, transport: RuntimeTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    pub const fn with_status_class(mut self, status_class: RuntimeStatusClass) -> Self {
        self.status_class = Some(status_class);
        self
    }

    pub const fn with_retry_delay(mut self, retry_delay: RuntimeRetryDelayBucket) -> Self {
        self.retry_delay = Some(retry_delay);
        self
    }
}

/// Content-free tuple for a non-credential runtime failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeErrorDiagnostic {
    pub code: RuntimeErrorCode,
    pub retryable: bool,
    pub recovery_action: RuntimeSafeRecoveryAction,
    pub context: RuntimeDiagnosticContext,
}

impl RuntimeErrorDiagnostic {
    pub const fn new(
        code: RuntimeErrorCode,
        retryable: bool,
        recovery_action: RuntimeSafeRecoveryAction,
        context: RuntimeDiagnosticContext,
    ) -> Self {
        Self {
            code,
            retryable,
            recovery_action,
            context,
        }
    }

    /// Safe default for a native/provider failure that has no recognized
    /// structural classification. The private source is discarded by the
    /// caller; this tuple has nowhere to retain it.
    pub const fn internal(context: RuntimeDiagnosticContext) -> Self {
        Self::new(
            RuntimeErrorCode::Internal,
            false,
            RuntimeSafeRecoveryAction::None,
            context,
        )
    }
}

/// One public diagnostic envelope for credential and non-credential runtime
/// failures. Credential failures embed the accepted e11c tuple unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum RuntimeDiagnostic {
    Credential(CredentialError),
    Runtime(RuntimeErrorDiagnostic),
}

impl RuntimeDiagnostic {
    pub const fn internal(context: RuntimeDiagnosticContext) -> Self {
        Self::Runtime(RuntimeErrorDiagnostic::internal(context))
    }
}

impl From<CredentialError> for RuntimeDiagnostic {
    fn from(error: CredentialError) -> Self {
        Self::Credential(error)
    }
}

impl From<RuntimeErrorDiagnostic> for RuntimeDiagnostic {
    fn from(error: RuntimeErrorDiagnostic) -> Self {
        Self::Runtime(error)
    }
}

impl fmt::Display for RuntimeDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credential(error) => write!(formatter, "credential/{}", error.code.as_str()),
            Self::Runtime(error) => write!(formatter, "runtime/{}", error.code),
        }
    }
}

#[derive(Serialize)]
struct RuntimeDiagnosticContractDefinition {
    schema_version: u32,
    error_codes: &'static [RuntimeErrorCode],
    recovery_actions: &'static [RuntimeSafeRecoveryAction],
    operations: &'static [RuntimeOperation],
    transports: &'static [RuntimeTransport],
    status_classes: &'static [RuntimeStatusClass],
    retry_delay_buckets: &'static [RuntimeRetryDelayBucket],
}

const RUNTIME_DIAGNOSTIC_CONTRACT: RuntimeDiagnosticContractDefinition =
    RuntimeDiagnosticContractDefinition {
        schema_version: 1,
        error_codes: RUNTIME_ERROR_CODES,
        recovery_actions: RUNTIME_RECOVERY_ACTIONS,
        operations: RUNTIME_OPERATIONS,
        transports: RUNTIME_TRANSPORTS,
        status_classes: RUNTIME_STATUS_CLASSES,
        retry_delay_buckets: RUNTIME_RETRY_DELAY_BUCKETS,
    };

/// Generate the renderer's closed diagnostic vocabulary and wire DTOs from the
/// exact Rust-owned contract. These types deliberately have no free-form text
/// field; untrusted payload validation remains fail-closed in the frontend.
pub fn runtime_diagnostic_typescript_module() -> String {
    let contract = serde_json::to_string_pretty(&RUNTIME_DIAGNOSTIC_CONTRACT)
        .expect("runtime diagnostic contract should serialize");
    format!(
        r#"// @generated by src-tauri/crates/ipc-contract/src/runtime_diagnostic.rs. Do not edit manually.

import type {{ CredentialError }} from "./credentialContract";

// biome-ignore format: preserve the deterministic serde projection from Rust
export const RUNTIME_DIAGNOSTIC_CONTRACT = {contract} as const;

export type RuntimeErrorCode =
  (typeof RUNTIME_DIAGNOSTIC_CONTRACT.error_codes)[number];
export type RuntimeSafeRecoveryAction =
  (typeof RUNTIME_DIAGNOSTIC_CONTRACT.recovery_actions)[number];
export type RuntimeOperation =
  (typeof RUNTIME_DIAGNOSTIC_CONTRACT.operations)[number];
export type RuntimeTransport =
  (typeof RUNTIME_DIAGNOSTIC_CONTRACT.transports)[number];
export type RuntimeStatusClass =
  (typeof RUNTIME_DIAGNOSTIC_CONTRACT.status_classes)[number];
export type RuntimeRetryDelayBucket =
  (typeof RUNTIME_DIAGNOSTIC_CONTRACT.retry_delay_buckets)[number];

export interface RuntimeDiagnosticContext {{
  operation: RuntimeOperation;
  transport?: RuntimeTransport | null;
  status_class?: RuntimeStatusClass | null;
  retry_delay?: RuntimeRetryDelayBucket | null;
}}

export interface RuntimeErrorDiagnostic {{
  code: RuntimeErrorCode;
  retryable: boolean;
  recovery_action: RuntimeSafeRecoveryAction;
  context: RuntimeDiagnosticContext;
}}

export type RuntimeDiagnostic =
  | {{ kind: "credential"; detail: CredentialError }}
  | {{ kind: "runtime"; detail: RuntimeErrorDiagnostic }};

export interface RuntimeDiagnosticAppError {{
  code: "runtime_diagnostic";
  diagnostic: RuntimeDiagnostic;
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_contract::{
        BuiltInCredentialSetId, CredentialErrorCode, CredentialSafeRecoveryAction, CredentialSetId,
    };
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use std::{collections::HashSet, fmt::Debug};

    fn assert_vocabulary<T>(label: &str, values: &[T], expected: &[&str])
    where
        T: Copy + Debug + Eq + Serialize + DeserializeOwned,
    {
        assert_eq!(values.len(), expected.len(), "{label} cardinality drifted");
        let serialized = serde_json::to_value(values).expect("vocabulary JSON");
        assert_eq!(serialized, json!(expected), "{label} wire values drifted");

        let unique: HashSet<_> = expected.iter().copied().collect();
        assert_eq!(unique.len(), expected.len(), "{label} contains duplicates");
        for value in values {
            let encoded = serde_json::to_string(value).expect("wire value JSON");
            let decoded: T = serde_json::from_str(&encoded).expect("wire value round trip");
            assert_eq!(*value, decoded, "{label} failed to round trip");
        }
    }

    #[test]
    fn vocabularies_are_exhaustive_unique_and_stable() {
        assert_vocabulary(
            "runtime error codes",
            RUNTIME_ERROR_CODES,
            &[
                "rate_limited",
                "network_unreachable",
                "timeout",
                "provider_unavailable",
                "protocol_error",
                "invalid_response",
                "request_rejected",
                "capacity_exhausted",
                "policy_blocked",
                "unsupported",
                "cancelled",
                "internal",
            ],
        );
        assert_vocabulary(
            "runtime recovery actions",
            RUNTIME_RECOVERY_ACTIONS,
            &[
                "none",
                "retry",
                "retry_after_delay",
                "check_network",
                "review_configuration",
                "choose_supported_provider",
                "review_policy",
                "restart_application",
            ],
        );
        assert_vocabulary(
            "runtime operations",
            RUNTIME_OPERATIONS,
            &[
                "provider_readiness",
                "model_discovery",
                "transcription",
                "speech_synthesis",
                "realtime_conversation",
                "language_model_inference",
                "provider_request",
            ],
        );
        assert_vocabulary(
            "runtime transports",
            RUNTIME_TRANSPORTS,
            &["native", "http", "websocket", "sdk"],
        );
        assert_vocabulary(
            "runtime status classes",
            RUNTIME_STATUS_CLASSES,
            &["redirect", "client_error", "server_error"],
        );
        assert_vocabulary(
            "runtime retry-delay buckets",
            RUNTIME_RETRY_DELAY_BUCKETS,
            &["immediate", "short", "medium", "long"],
        );
    }

    #[test]
    fn generated_runtime_diagnostic_ts_is_content_free_and_current() {
        let module = runtime_diagnostic_typescript_module();
        for forbidden in [
            "message:",
            "body:",
            "path:",
            "request_id:",
            "fingerprint:",
            "exact_length:",
        ] {
            assert!(
                !module.contains(forbidden),
                "generated diagnostic DTO contains {forbidden}"
            );
        }

        let generated = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../src/generated/runtimeDiagnostic.ts");
        let actual = std::fs::read_to_string(&generated).unwrap_or_else(|error| {
            panic!(
                "failed to read generated runtime diagnostic contract {}: {error}",
                generated.display()
            )
        });
        assert_eq!(
            actual, module,
            "generated runtime diagnostic contract drifted; run `bun run generate:runtime-diagnostic`"
        );
    }

    #[test]
    fn unknown_wire_values_are_rejected_by_every_closed_vocabulary() {
        let unknown = r#""provider_native_unknown_canary""#;
        assert!(serde_json::from_str::<RuntimeErrorCode>(unknown).is_err());
        assert!(serde_json::from_str::<RuntimeSafeRecoveryAction>(unknown).is_err());
        assert!(serde_json::from_str::<RuntimeOperation>(unknown).is_err());
        assert!(serde_json::from_str::<RuntimeTransport>(unknown).is_err());
        assert!(serde_json::from_str::<RuntimeStatusClass>(unknown).is_err());
        assert!(serde_json::from_str::<RuntimeRetryDelayBucket>(unknown).is_err());
    }

    #[test]
    fn credential_envelope_preserves_the_accepted_e11c_tuple() {
        let credential = CredentialError {
            code: CredentialErrorCode::AccessDenied,
            retryable: false,
            recovery_action: CredentialSafeRecoveryAction::ReenterCredential,
            set_id: Some(CredentialSetId::BuiltIn(BuiltInCredentialSetId::Openai)),
        };
        let diagnostic = RuntimeDiagnostic::from(credential.clone());

        assert_eq!(
            serde_json::to_value(&diagnostic).expect("credential diagnostic JSON"),
            json!({
                "kind": "credential",
                "detail": {
                    "code": "access_denied",
                    "retryable": false,
                    "recovery_action": "reenter_credential",
                    "set_id": "openai",
                },
            })
        );
        assert_eq!(diagnostic, RuntimeDiagnostic::Credential(credential));
    }

    #[test]
    fn runtime_envelope_contains_only_closed_bounded_context() {
        let diagnostic = RuntimeDiagnostic::Runtime(RuntimeErrorDiagnostic::new(
            RuntimeErrorCode::RateLimited,
            true,
            RuntimeSafeRecoveryAction::RetryAfterDelay,
            RuntimeDiagnosticContext::new(RuntimeOperation::Transcription)
                .with_transport(RuntimeTransport::Websocket)
                .with_status_class(RuntimeStatusClass::ClientError)
                .with_retry_delay(RuntimeRetryDelayBucket::Long),
        ));

        assert_eq!(
            serde_json::to_value(diagnostic).expect("runtime diagnostic JSON"),
            json!({
                "kind": "runtime",
                "detail": {
                    "code": "rate_limited",
                    "retryable": true,
                    "recovery_action": "retry_after_delay",
                    "context": {
                        "operation": "transcription",
                        "transport": "websocket",
                        "status_class": "client_error",
                        "retry_delay": "long",
                    },
                },
            })
        );
    }

    fn assert_no_forbidden_keys(value: &Value) {
        const FORBIDDEN_PARTS: &[&str] = &[
            "message",
            "reason",
            "secret",
            "body",
            "path",
            "request_id",
            "fingerprint",
            "length",
            "bytes",
            "chars",
        ];
        match value {
            Value::Object(object) => {
                for (key, nested) in object {
                    assert!(
                        FORBIDDEN_PARTS.iter().all(|part| !key.contains(part)),
                        "forbidden public diagnostic key {key}"
                    );
                    assert_no_forbidden_keys(nested);
                }
            }
            Value::Array(values) => values.iter().for_each(assert_no_forbidden_keys),
            _ => {}
        }
    }

    #[test]
    fn public_envelopes_have_no_prose_or_sensitive_dimension_fields() {
        let credential = RuntimeDiagnostic::Credential(CredentialError {
            code: CredentialErrorCode::Locked,
            retryable: true,
            recovery_action: CredentialSafeRecoveryAction::UnlockStore,
            set_id: Some(CredentialSetId::BuiltIn(BuiltInCredentialSetId::Aws)),
        });
        let runtime = RuntimeDiagnostic::internal(
            RuntimeDiagnosticContext::new(RuntimeOperation::ProviderRequest)
                .with_transport(RuntimeTransport::Native),
        );

        for value in [
            serde_json::to_value(credential).expect("credential JSON"),
            serde_json::to_value(runtime).expect("runtime JSON"),
        ] {
            assert_no_forbidden_keys(&value);
        }
    }

    #[test]
    fn retry_delay_is_saturated_before_serialization() {
        for (seconds, expected) in [
            (0, RuntimeRetryDelayBucket::Immediate),
            (1, RuntimeRetryDelayBucket::Short),
            (5, RuntimeRetryDelayBucket::Short),
            (6, RuntimeRetryDelayBucket::Medium),
            (30, RuntimeRetryDelayBucket::Medium),
            (31, RuntimeRetryDelayBucket::Long),
            (u64::MAX, RuntimeRetryDelayBucket::Long),
        ] {
            assert_eq!(RuntimeRetryDelayBucket::from_seconds(seconds), expected);
        }

        let serialized = serde_json::to_string(&RuntimeRetryDelayBucket::from_seconds(7_919_977))
            .expect("retry bucket JSON");
        assert_eq!(serialized, r#""long""#);
        assert!(!serialized.contains("7919977"));
    }
}
