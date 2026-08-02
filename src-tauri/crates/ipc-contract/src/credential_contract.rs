//! Rust-owned credential-domain and IPC contract (Seed `audio-graph-e11c`).
//!
//! This module is deliberately dependency-light and behavior-free. Later
//! credential-service workstreams consume these closed identifiers and
//! content-free response DTOs; this slice does not read, write, migrate, or
//! resolve a credential.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::{fmt, str::FromStr};

/// Portable maximum for one final encoded native-store record.
pub const PORTABLE_ENCODED_RECORD_MAX_BYTES: usize = 2_560;
pub const CREDENTIAL_CONTRACT_SCHEMA_VERSION: u32 = 1;
pub const CUSTOM_CREDENTIAL_SET_ID_PREFIX: &str = "custom.";
pub const CUSTOM_CREDENTIAL_SET_ID_PATTERN: &str =
    r"^custom\.[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$";

/// Defines a closed wire vocabulary and its exhaustive exported slice from one
/// declaration. Adding a variant without adding it to the vocabulary is
/// therefore impossible.
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

        pub const $values: &[$name] = &[$($name::$variant),+];
    };
}

closed_vocabulary! {
    pub enum BuiltInCredentialSetId => BUILT_IN_CREDENTIAL_SET_IDS {
        Openai => "openai",
        Cerebras => "cerebras",
        Sambanova => "sambanova",
        Openrouter => "openrouter",
        Groq => "groq",
        Together => "together",
        Fireworks => "fireworks",
        Deepgram => "deepgram",
        Assemblyai => "assemblyai",
        Soniox => "soniox",
        Gladia => "gladia",
        Speechmatics => "speechmatics",
        Elevenlabs => "elevenlabs",
        Revai => "revai",
        AzureSpeech => "azure_speech",
        Gemini => "gemini",
        Aws => "aws",
    }
}

impl FromStr for BuiltInCredentialSetId {
    type Err = InvalidCredentialSetId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        BUILT_IN_CREDENTIAL_SET_IDS
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == value)
            .ok_or(InvalidCredentialSetId)
    }
}

/// Canonical backend-issued custom credential-set identifier.
///
/// The private field prevents renderers or downstream callers from constructing
/// an unchecked id. UUID generation belongs to the future service workstream;
/// this contract accepts only the lowercase canonical representation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CustomCredentialSetId(String);

impl CustomCredentialSetId {
    pub fn parse(value: impl Into<String>) -> Result<Self, InvalidCredentialSetId> {
        let value = value.into();
        if is_canonical_custom_credential_set_id(&value) {
            Ok(Self(value))
        } else {
            Err(InvalidCredentialSetId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CustomCredentialSetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for CustomCredentialSetId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CustomCredentialSetId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum CredentialSetId {
    BuiltIn(BuiltInCredentialSetId),
    Custom(CustomCredentialSetId),
}

impl CredentialSetId {
    pub fn as_str(&self) -> &str {
        match self {
            Self::BuiltIn(id) => id.as_str(),
            Self::Custom(id) => id.as_str(),
        }
    }
}

impl From<BuiltInCredentialSetId> for CredentialSetId {
    fn from(value: BuiltInCredentialSetId) -> Self {
        Self::BuiltIn(value)
    }
}

impl FromStr for CredentialSetId {
    type Err = InvalidCredentialSetId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        BuiltInCredentialSetId::from_str(value)
            .map(Self::BuiltIn)
            .or_else(|_| CustomCredentialSetId::parse(value).map(Self::Custom))
    }
}

impl fmt::Display for CredentialSetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for CredentialSetId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for CredentialSetId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::from_str(&value).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCredentialSetId;

impl fmt::Display for InvalidCredentialSetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid credential-set id")
    }
}

impl std::error::Error for InvalidCredentialSetId {}

pub fn is_canonical_custom_credential_set_id(value: &str) -> bool {
    let Some(uuid) = value.strip_prefix(CUSTOM_CREDENTIAL_SET_ID_PREFIX) else {
        return false;
    };
    if value.len() > 64 || uuid.len() != 36 {
        return false;
    }
    uuid.bytes().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) {
            byte == b'-'
        } else {
            byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
        }
    })
}

closed_vocabulary! {
    pub enum AuthMethodId => AUTH_METHOD_IDS {
        ApiKey => "api_key",
        GoogleServiceAccountFile => "google_service_account_file",
        AwsStatic => "aws_static",
        AwsProfile => "aws_profile",
        AwsDefaultChain => "aws_default_chain",
        CustomBearerApiKey => "custom_bearer_api_key",
    }
}

closed_vocabulary! {
    pub enum CredentialFieldClass => CREDENTIAL_FIELD_CLASSES {
        Secret => "secret",
        PrivateLocator => "private_locator",
        OrdinaryConfig => "ordinary_config",
    }
}

closed_vocabulary! {
    pub enum LegacyFieldDisposition => LEGACY_FIELD_DISPOSITIONS {
        Migrate => "migrate",
        Config => "config",
        Deprecate => "deprecate",
        Remove => "remove",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialFieldRequirement {
    Required,
    Optional,
    RequiredTogether { group_id: &'static str },
    Alternative { group_id: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CredentialFieldDefinition {
    pub legacy_key: &'static str,
    pub set_id: BuiltInCredentialSetId,
    pub auth_method_ids: &'static [AuthMethodId],
    pub class: CredentialFieldClass,
    pub legacy_disposition: LegacyFieldDisposition,
    pub requirement: CredentialFieldRequirement,
    /// Only stored secret fields can make a set `configured`.
    pub contributes_to_credential_presence: bool,
}

// One declaration expands to both the compatibility allowlist and the typed
// field metadata. This prevents a second Rust allowlist from drifting.
macro_rules! define_legacy_credential_fields {
    ($(($key:literal, $set:expr, $methods:expr, $class:expr, $disposition:expr, $requirement:expr, $presence:expr)),+ $(,)?) => {
        pub const ALLOWED_CREDENTIAL_KEYS: &[&str] = &[$($key),+];
        pub const CREDENTIAL_FIELDS: &[CredentialFieldDefinition] = &[
            $(CredentialFieldDefinition {
                legacy_key: $key,
                set_id: $set,
                auth_method_ids: $methods,
                class: $class,
                legacy_disposition: $disposition,
                requirement: $requirement,
                contributes_to_credential_presence: $presence,
            }),+
        ];
    };
}

use AuthMethodId as Auth;
use BuiltInCredentialSetId as Set;
use CredentialFieldClass as Class;
use CredentialFieldRequirement as Requirement;
use LegacyFieldDisposition as Disposition;

define_legacy_credential_fields!(
    (
        "openai_api_key",
        Set::Openai,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "cerebras_api_key",
        Set::Cerebras,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "sambanova_api_key",
        Set::Sambanova,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "openrouter_api_key",
        Set::Openrouter,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "groq_api_key",
        Set::Groq,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "together_api_key",
        Set::Together,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "fireworks_api_key",
        Set::Fireworks,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "deepgram_api_key",
        Set::Deepgram,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "assemblyai_api_key",
        Set::Assemblyai,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "soniox_api_key",
        Set::Soniox,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "gladia_api_key",
        Set::Gladia,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "speechmatics_api_key",
        Set::Speechmatics,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "elevenlabs_api_key",
        Set::Elevenlabs,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "revai_api_key",
        Set::Revai,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "azure_speech_key",
        Set::AzureSpeech,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Required,
        true
    ),
    (
        "gemini_api_key",
        Set::Gemini,
        &[Auth::ApiKey],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Alternative {
            group_id: "gemini.authentication"
        },
        true
    ),
    (
        "google_service_account_path",
        Set::Gemini,
        &[Auth::GoogleServiceAccountFile],
        Class::PrivateLocator,
        Disposition::Config,
        Requirement::Alternative {
            group_id: "gemini.authentication"
        },
        false
    ),
    (
        "aws_access_key",
        Set::Aws,
        &[Auth::AwsStatic],
        Class::Secret,
        Disposition::Migrate,
        Requirement::RequiredTogether {
            group_id: "aws.static_pair"
        },
        true
    ),
    (
        "aws_secret_key",
        Set::Aws,
        &[Auth::AwsStatic],
        Class::Secret,
        Disposition::Migrate,
        Requirement::RequiredTogether {
            group_id: "aws.static_pair"
        },
        true
    ),
    (
        "aws_session_token",
        Set::Aws,
        &[Auth::AwsStatic],
        Class::Secret,
        Disposition::Migrate,
        Requirement::Optional,
        false
    ),
    (
        "aws_profile",
        Set::Aws,
        &[Auth::AwsProfile],
        Class::OrdinaryConfig,
        Disposition::Config,
        Requirement::Optional,
        false
    ),
    (
        "aws_region",
        Set::Aws,
        &[Auth::AwsStatic, Auth::AwsProfile, Auth::AwsDefaultChain],
        Class::OrdinaryConfig,
        Disposition::Config,
        Requirement::Optional,
        false
    ),
);

pub fn credential_field_for_legacy_key(key: &str) -> Option<&'static CredentialFieldDefinition> {
    CREDENTIAL_FIELDS
        .iter()
        .find(|field| field.legacy_key == key)
}

closed_vocabulary! {
    pub enum CredentialPurpose => CREDENTIAL_PURPOSES {
        Asr => "asr",
        Llm => "llm",
        Tts => "tts",
        RealtimeAgent => "realtime_agent",
        ModelCatalog => "model_catalog",
        HealthCheck => "health_check",
        VertexAuthentication => "vertex_authentication",
    }
}

closed_vocabulary! {
    pub enum SecureTransportScheme => SECURE_TRANSPORT_SCHEMES {
        Https => "https",
        Wss => "wss",
    }
}

closed_vocabulary! {
    pub enum AwsPartition => AWS_PARTITIONS {
        Aws => "aws",
        AwsCn => "aws_cn",
        AwsUsGov => "aws_us_gov",
    }
}

closed_vocabulary! {
    pub enum AwsSdkService => AWS_SDK_SERVICES {
        TranscribeStreaming => "transcribe_streaming",
        BedrockRuntime => "bedrock_runtime",
        Sts => "sts",
    }
}

/// Runtime audience supplied to future `resolve_for_use` implementations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialAudience {
    SecureNetworkOrigin {
        scheme: SecureTransportScheme,
        canonical_host: String,
        effective_port: u16,
    },
    AwsSdk {
        partition: AwsPartition,
        service: AwsSdkService,
        region: String,
    },
}

/// Closed audience for one atomic credential-use relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialAudiencePolicyDefinition {
    ExactSecureOrigin {
        origin: &'static str,
    },
    BackendDerivedVertexOrigin {
        scheme: SecureTransportScheme,
        host_suffix: &'static str,
        effective_port: u16,
    },
    AwsSdk {
        partition: AwsPartition,
        service: AwsSdkService,
    },
}

closed_vocabulary! {
    pub enum CredentialActiveUseAction => ACTIVE_USE_ACTIONS {
        None => "none",
        RefreshBeforeNextUse => "refresh_before_next_use",
        Reauthenticate => "reauthenticate",
        Stop => "stop",
        RestartApplication => "restart_application",
    }
}

closed_vocabulary! {
    pub enum CredentialUsePolicyDisabledReason => USE_POLICY_DISABLED_REASONS {
        AudienceUnmodeled => "audience_unmodeled",
        UnsupportedCurrentRoute => "unsupported_current_route",
        ProviderPlanned => "provider_planned",
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CredentialUsePolicyDecisionDefinition {
    Authorized {
        audience: CredentialAudiencePolicyDefinition,
    },
    Disabled {
        reason: CredentialUsePolicyDisabledReason,
    },
}

/// Secret-field rule that establishes a set as configured. Supplemental
/// optional secrets and non-secret settings never satisfy this rule alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialSetCompleteness {
    AllRequiredSecretFields,
    RequiredTogether { group_id: &'static str },
    AnyStoredSecretAlternative { group_id: &'static str },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CredentialSetDefinition {
    pub id: BuiltInCredentialSetId,
    pub configured_when: CredentialSetCompleteness,
}

pub const CREDENTIAL_SET_DEFINITIONS: &[CredentialSetDefinition] = &[
    CredentialSetDefinition {
        id: Set::Openai,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Cerebras,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Sambanova,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Openrouter,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Groq,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Together,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Fireworks,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Deepgram,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Assemblyai,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Soniox,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Gladia,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Speechmatics,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Elevenlabs,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Revai,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::AzureSpeech,
        configured_when: CredentialSetCompleteness::AllRequiredSecretFields,
    },
    CredentialSetDefinition {
        id: Set::Gemini,
        configured_when: CredentialSetCompleteness::AnyStoredSecretAlternative {
            group_id: "gemini.authentication",
        },
    },
    CredentialSetDefinition {
        id: Set::Aws,
        configured_when: CredentialSetCompleteness::RequiredTogether {
            group_id: "aws.static_pair",
        },
    },
];

/// One row is the complete authorization decision. Callers must match every
/// field; no independent list may be combined into broader authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CredentialUsePolicyDefinition {
    pub set_id: BuiltInCredentialSetId,
    pub consumer_id: &'static str,
    pub auth_method_id: AuthMethodId,
    pub purpose: CredentialPurpose,
    pub decision: CredentialUsePolicyDecisionDefinition,
    pub active_use_action: CredentialActiveUseAction,
}

macro_rules! exact_use {
    ($set:expr, $consumer:literal, $auth:expr, $purpose:expr, $origin:literal, $action:expr) => {
        CredentialUsePolicyDefinition {
            set_id: $set,
            consumer_id: $consumer,
            auth_method_id: $auth,
            purpose: $purpose,
            decision: CredentialUsePolicyDecisionDefinition::Authorized {
                audience: CredentialAudiencePolicyDefinition::ExactSecureOrigin { origin: $origin },
            },
            active_use_action: $action,
        }
    };
}

macro_rules! disabled_use {
    ($set:expr, $consumer:literal, $auth:expr, $purpose:expr, $reason:expr) => {
        CredentialUsePolicyDefinition {
            set_id: $set,
            consumer_id: $consumer,
            auth_method_id: $auth,
            purpose: $purpose,
            decision: CredentialUsePolicyDecisionDefinition::Disabled { reason: $reason },
            active_use_action: CredentialActiveUseAction::Stop,
        }
    };
}

macro_rules! aws_use {
    ($consumer:literal, $auth:expr, $purpose:expr, $partition:expr, $service:expr, $action:expr) => {
        CredentialUsePolicyDefinition {
            set_id: Set::Aws,
            consumer_id: $consumer,
            auth_method_id: $auth,
            purpose: $purpose,
            decision: CredentialUsePolicyDecisionDefinition::Authorized {
                audience: CredentialAudiencePolicyDefinition::AwsSdk {
                    partition: $partition,
                    service: $service,
                },
            },
            active_use_action: $action,
        }
    };
}

use CredentialActiveUseAction as Action;
use CredentialPurpose as Purpose;
use CredentialUsePolicyDisabledReason as DisabledReason;

pub const CREDENTIAL_USE_POLICIES: &[CredentialUsePolicyDefinition] = &[
    exact_use!(
        Set::Openai,
        "asr.api",
        Auth::ApiKey,
        Purpose::Asr,
        "https://api.openai.com",
        Action::RefreshBeforeNextUse
    ),
    exact_use!(
        Set::Openai,
        "asr.openai_realtime",
        Auth::ApiKey,
        Purpose::Asr,
        "wss://api.openai.com",
        Action::Reauthenticate
    ),
    exact_use!(
        Set::Openai,
        "llm.api",
        Auth::ApiKey,
        Purpose::Llm,
        "https://api.openai.com",
        Action::RefreshBeforeNextUse
    ),
    exact_use!(
        Set::Openai,
        "realtime_agent.openai_realtime",
        Auth::ApiKey,
        Purpose::RealtimeAgent,
        "wss://api.openai.com",
        Action::Reauthenticate
    ),
    disabled_use!(
        Set::Cerebras,
        "asr.api",
        Auth::ApiKey,
        Purpose::Asr,
        DisabledReason::UnsupportedCurrentRoute
    ),
    exact_use!(
        Set::Cerebras,
        "llm.api",
        Auth::ApiKey,
        Purpose::Llm,
        "https://api.cerebras.ai",
        Action::RefreshBeforeNextUse
    ),
    exact_use!(
        Set::Cerebras,
        "llm.cerebras",
        Auth::ApiKey,
        Purpose::Llm,
        "https://api.cerebras.ai",
        Action::RefreshBeforeNextUse
    ),
    disabled_use!(
        Set::Sambanova,
        "asr.api",
        Auth::ApiKey,
        Purpose::Asr,
        DisabledReason::UnsupportedCurrentRoute
    ),
    exact_use!(
        Set::Sambanova,
        "llm.api",
        Auth::ApiKey,
        Purpose::Llm,
        "https://api.sambanova.ai",
        Action::RefreshBeforeNextUse
    ),
    exact_use!(
        Set::Sambanova,
        "llm.sambanova",
        Auth::ApiKey,
        Purpose::Llm,
        "https://api.sambanova.ai",
        Action::RefreshBeforeNextUse
    ),
    disabled_use!(
        Set::Openrouter,
        "asr.api",
        Auth::ApiKey,
        Purpose::Asr,
        DisabledReason::UnsupportedCurrentRoute
    ),
    exact_use!(
        Set::Openrouter,
        "llm.api",
        Auth::ApiKey,
        Purpose::Llm,
        "https://openrouter.ai",
        Action::RefreshBeforeNextUse
    ),
    exact_use!(
        Set::Openrouter,
        "llm.openrouter",
        Auth::ApiKey,
        Purpose::Llm,
        "https://openrouter.ai",
        Action::RefreshBeforeNextUse
    ),
    exact_use!(
        Set::Groq,
        "asr.api",
        Auth::ApiKey,
        Purpose::Asr,
        "https://api.groq.com",
        Action::RefreshBeforeNextUse
    ),
    exact_use!(
        Set::Groq,
        "llm.api",
        Auth::ApiKey,
        Purpose::Llm,
        "https://api.groq.com",
        Action::RefreshBeforeNextUse
    ),
    exact_use!(
        Set::Together,
        "asr.api",
        Auth::ApiKey,
        Purpose::Asr,
        "https://api.together.xyz",
        Action::RefreshBeforeNextUse
    ),
    exact_use!(
        Set::Together,
        "llm.api",
        Auth::ApiKey,
        Purpose::Llm,
        "https://api.together.xyz",
        Action::RefreshBeforeNextUse
    ),
    disabled_use!(
        Set::Fireworks,
        "asr.api",
        Auth::ApiKey,
        Purpose::Asr,
        DisabledReason::UnsupportedCurrentRoute
    ),
    exact_use!(
        Set::Fireworks,
        "llm.api",
        Auth::ApiKey,
        Purpose::Llm,
        "https://api.fireworks.ai",
        Action::RefreshBeforeNextUse
    ),
    exact_use!(
        Set::Deepgram,
        "asr.deepgram",
        Auth::ApiKey,
        Purpose::Asr,
        "wss://api.deepgram.com",
        Action::Reauthenticate
    ),
    exact_use!(
        Set::Deepgram,
        "tts.deepgram_aura",
        Auth::ApiKey,
        Purpose::Tts,
        "wss://api.deepgram.com",
        Action::Reauthenticate
    ),
    exact_use!(
        Set::Assemblyai,
        "asr.assemblyai",
        Auth::ApiKey,
        Purpose::Asr,
        "wss://streaming.assemblyai.com",
        Action::Reauthenticate
    ),
    exact_use!(
        Set::Soniox,
        "asr.soniox",
        Auth::ApiKey,
        Purpose::Asr,
        "wss://stt-rt.soniox.com",
        Action::Reauthenticate
    ),
    exact_use!(
        Set::Gladia,
        "asr.gladia",
        Auth::ApiKey,
        Purpose::Asr,
        "https://api.gladia.io",
        Action::RefreshBeforeNextUse
    ),
    exact_use!(
        Set::Speechmatics,
        "asr.speechmatics",
        Auth::ApiKey,
        Purpose::Asr,
        "wss://eu.rt.speechmatics.com",
        Action::Reauthenticate
    ),
    exact_use!(
        Set::Speechmatics,
        "asr.speechmatics",
        Auth::ApiKey,
        Purpose::Asr,
        "wss://us.rt.speechmatics.com",
        Action::Reauthenticate
    ),
    disabled_use!(
        Set::Elevenlabs,
        "asr.elevenlabs_scribe",
        Auth::ApiKey,
        Purpose::Asr,
        DisabledReason::AudienceUnmodeled
    ),
    exact_use!(
        Set::Revai,
        "asr.revai",
        Auth::ApiKey,
        Purpose::Asr,
        "wss://api.rev.ai",
        Action::Reauthenticate
    ),
    disabled_use!(
        Set::AzureSpeech,
        "asr.azure_speech",
        Auth::ApiKey,
        Purpose::Asr,
        DisabledReason::ProviderPlanned
    ),
    exact_use!(
        Set::Gemini,
        "realtime_agent.gemini_live",
        Auth::ApiKey,
        Purpose::RealtimeAgent,
        "wss://generativelanguage.googleapis.com",
        Action::Reauthenticate
    ),
    exact_use!(
        Set::Gemini,
        "realtime_agent.gemini_live",
        Auth::ApiKey,
        Purpose::HealthCheck,
        "https://generativelanguage.googleapis.com",
        Action::RefreshBeforeNextUse
    ),
    CredentialUsePolicyDefinition {
        set_id: Set::Gemini,
        consumer_id: "realtime_agent.gemini_live",
        auth_method_id: Auth::GoogleServiceAccountFile,
        purpose: Purpose::RealtimeAgent,
        decision: CredentialUsePolicyDecisionDefinition::Authorized {
            audience: CredentialAudiencePolicyDefinition::BackendDerivedVertexOrigin {
                scheme: SecureTransportScheme::Wss,
                host_suffix: "aiplatform.googleapis.com",
                effective_port: 443,
            },
        },
        active_use_action: Action::Reauthenticate,
    },
    aws_use!(
        "asr.aws_transcribe",
        Auth::AwsStatic,
        Purpose::Asr,
        AwsPartition::Aws,
        AwsSdkService::TranscribeStreaming,
        Action::Reauthenticate
    ),
    aws_use!(
        "asr.aws_transcribe",
        Auth::AwsStatic,
        Purpose::Asr,
        AwsPartition::AwsCn,
        AwsSdkService::TranscribeStreaming,
        Action::Reauthenticate
    ),
    aws_use!(
        "asr.aws_transcribe",
        Auth::AwsStatic,
        Purpose::Asr,
        AwsPartition::AwsUsGov,
        AwsSdkService::TranscribeStreaming,
        Action::Reauthenticate
    ),
    aws_use!(
        "asr.aws_transcribe",
        Auth::AwsProfile,
        Purpose::Asr,
        AwsPartition::Aws,
        AwsSdkService::TranscribeStreaming,
        Action::Reauthenticate
    ),
    aws_use!(
        "asr.aws_transcribe",
        Auth::AwsProfile,
        Purpose::Asr,
        AwsPartition::AwsCn,
        AwsSdkService::TranscribeStreaming,
        Action::Reauthenticate
    ),
    aws_use!(
        "asr.aws_transcribe",
        Auth::AwsProfile,
        Purpose::Asr,
        AwsPartition::AwsUsGov,
        AwsSdkService::TranscribeStreaming,
        Action::Reauthenticate
    ),
    aws_use!(
        "asr.aws_transcribe",
        Auth::AwsDefaultChain,
        Purpose::Asr,
        AwsPartition::Aws,
        AwsSdkService::TranscribeStreaming,
        Action::Reauthenticate
    ),
    aws_use!(
        "asr.aws_transcribe",
        Auth::AwsDefaultChain,
        Purpose::Asr,
        AwsPartition::AwsCn,
        AwsSdkService::TranscribeStreaming,
        Action::Reauthenticate
    ),
    aws_use!(
        "asr.aws_transcribe",
        Auth::AwsDefaultChain,
        Purpose::Asr,
        AwsPartition::AwsUsGov,
        AwsSdkService::TranscribeStreaming,
        Action::Reauthenticate
    ),
    aws_use!(
        "llm.aws_bedrock",
        Auth::AwsStatic,
        Purpose::Llm,
        AwsPartition::Aws,
        AwsSdkService::BedrockRuntime,
        Action::RefreshBeforeNextUse
    ),
    aws_use!(
        "llm.aws_bedrock",
        Auth::AwsStatic,
        Purpose::Llm,
        AwsPartition::AwsCn,
        AwsSdkService::BedrockRuntime,
        Action::RefreshBeforeNextUse
    ),
    aws_use!(
        "llm.aws_bedrock",
        Auth::AwsStatic,
        Purpose::Llm,
        AwsPartition::AwsUsGov,
        AwsSdkService::BedrockRuntime,
        Action::RefreshBeforeNextUse
    ),
    aws_use!(
        "llm.aws_bedrock",
        Auth::AwsProfile,
        Purpose::Llm,
        AwsPartition::Aws,
        AwsSdkService::BedrockRuntime,
        Action::RefreshBeforeNextUse
    ),
    aws_use!(
        "llm.aws_bedrock",
        Auth::AwsProfile,
        Purpose::Llm,
        AwsPartition::AwsCn,
        AwsSdkService::BedrockRuntime,
        Action::RefreshBeforeNextUse
    ),
    aws_use!(
        "llm.aws_bedrock",
        Auth::AwsProfile,
        Purpose::Llm,
        AwsPartition::AwsUsGov,
        AwsSdkService::BedrockRuntime,
        Action::RefreshBeforeNextUse
    ),
    aws_use!(
        "llm.aws_bedrock",
        Auth::AwsDefaultChain,
        Purpose::Llm,
        AwsPartition::Aws,
        AwsSdkService::BedrockRuntime,
        Action::RefreshBeforeNextUse
    ),
    aws_use!(
        "llm.aws_bedrock",
        Auth::AwsDefaultChain,
        Purpose::Llm,
        AwsPartition::AwsCn,
        AwsSdkService::BedrockRuntime,
        Action::RefreshBeforeNextUse
    ),
    aws_use!(
        "llm.aws_bedrock",
        Auth::AwsDefaultChain,
        Purpose::Llm,
        AwsPartition::AwsUsGov,
        AwsSdkService::BedrockRuntime,
        Action::RefreshBeforeNextUse
    ),
];

pub fn credential_set_definition(
    id: BuiltInCredentialSetId,
) -> Option<&'static CredentialSetDefinition> {
    CREDENTIAL_SET_DEFINITIONS.iter().find(|set| set.id == id)
}

pub fn credential_use_policies_for_consumer(
    consumer_id: &str,
) -> impl Iterator<Item = &'static CredentialUsePolicyDefinition> + '_ {
    CREDENTIAL_USE_POLICIES
        .iter()
        .filter(move |policy| policy.consumer_id == consumer_id)
}

/// Tests one already-normalized runtime audience against one atomic policy.
/// Disabled rows always deny and are retained only to make known gaps durable.
pub fn credential_use_policy_allows_audience(
    policy: &CredentialUsePolicyDefinition,
    audience: &CredentialAudience,
) -> bool {
    let CredentialUsePolicyDecisionDefinition::Authorized { audience: allowed } = policy.decision
    else {
        return false;
    };

    match (allowed, audience) {
        (
            CredentialAudiencePolicyDefinition::ExactSecureOrigin { origin },
            CredentialAudience::SecureNetworkOrigin {
                scheme,
                canonical_host,
                effective_port,
            },
        ) => url::Url::parse(origin).is_ok_and(|expected| {
            expected.scheme() == scheme.as_str()
                && expected.host_str() == Some(canonical_host.as_str())
                && expected.port_or_known_default() == Some(*effective_port)
                && expected.path() == "/"
                && expected.query().is_none()
                && expected.fragment().is_none()
        }),
        (
            CredentialAudiencePolicyDefinition::BackendDerivedVertexOrigin {
                scheme: allowed_scheme,
                host_suffix,
                effective_port: allowed_port,
            },
            CredentialAudience::SecureNetworkOrigin {
                scheme,
                canonical_host,
                effective_port,
            },
        ) => {
            let separator_and_suffix = format!("-{host_suffix}");
            let Some(location) = canonical_host.strip_suffix(&separator_and_suffix) else {
                return false;
            };
            scheme == &allowed_scheme
                && effective_port == &allowed_port
                && vertex_origin_host(location).as_deref() == Some(canonical_host.as_str())
        }
        (
            CredentialAudiencePolicyDefinition::AwsSdk { partition, service },
            CredentialAudience::AwsSdk {
                partition: actual_partition,
                service: actual_service,
                region,
            },
        ) => {
            partition == *actual_partition
                && service == *actual_service
                && !region.trim().is_empty()
        }
        _ => false,
    }
}

/// Vertex hosts are backend-derived from a validated location, never accepted
/// as an arbitrary caller-supplied origin.
pub fn is_valid_vertex_location(location: &str) -> bool {
    let bytes = location.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes.first().is_some_and(u8::is_ascii_lowercase)
        && bytes.last().is_some_and(u8::is_ascii_alphanumeric)
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-')
}

pub fn vertex_origin_host(location: &str) -> Option<String> {
    is_valid_vertex_location(location).then(|| format!("{location}-aiplatform.googleapis.com"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CustomCredentialSetPolicy {
    pub id_prefix: &'static str,
    pub canonical_pattern: &'static str,
    pub backend_issued: bool,
    pub immutable_origin_binding: bool,
    pub complete_secret_required_for_new_binding: bool,
    pub auth_method_id: AuthMethodId,
    pub allowed_schemes: &'static [SecureTransportScheme],
}

pub const CUSTOM_CREDENTIAL_SET_POLICY: CustomCredentialSetPolicy = CustomCredentialSetPolicy {
    id_prefix: CUSTOM_CREDENTIAL_SET_ID_PREFIX,
    canonical_pattern: CUSTOM_CREDENTIAL_SET_ID_PATTERN,
    backend_issued: true,
    immutable_origin_binding: true,
    complete_secret_required_for_new_binding: true,
    auth_method_id: AuthMethodId::CustomBearerApiKey,
    allowed_schemes: &[SecureTransportScheme::Https, SecureTransportScheme::Wss],
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidCredentialToken;

impl fmt::Display for InvalidCredentialToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid credential token")
    }
}

impl std::error::Error for InvalidCredentialToken {}

pub fn is_canonical_credential_token(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')
            }
        })
}

macro_rules! canonical_token {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, InvalidCredentialToken> {
                let value = value.into();
                if is_canonical_credential_token(&value) {
                    Ok(Self(value))
                } else {
                    Err(InvalidCredentialToken)
                }
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::parse(String::deserialize(deserializer)?).map_err(D::Error::custom)
            }
        }
    };
}

canonical_token!(CredentialRevision);
canonical_token!(CredentialOperationId);
canonical_token!(CredentialIdempotencyToken);

closed_vocabulary! {
    pub enum CredentialBackendKind => BACKEND_KINDS {
        Native => "native",
        FileV2 => "file_v2",
        InMemory => "in_memory",
    }
}

closed_vocabulary! {
    pub enum CredentialBackendAvailability => BACKEND_AVAILABILITIES {
        Unknown => "unknown",
        Available => "available",
        Locked => "locked",
        AccessDenied => "access_denied",
        Unavailable => "unavailable",
        Unsupported => "unsupported",
        RecoveryRequired => "recovery_required",
    }
}

closed_vocabulary! {
    pub enum CredentialMigrationState => MIGRATION_STATES {
        Uninitialized => "uninitialized",
        NotRequired => "not_required",
        InventoryRequired => "inventory_required",
        Ready => "ready",
        InProgress => "in_progress",
        Conflict => "conflict",
        Completed => "completed",
        RecoveryRequired => "recovery_required",
    }
}

closed_vocabulary! {
    pub enum CredentialCleanupState => CLEANUP_STATES {
        NotApplicable => "not_applicable",
        Pending => "pending",
        InProgress => "in_progress",
        Completed => "completed",
        Blocked => "blocked",
    }
}

closed_vocabulary! {
    /// Redaction-safe authority source for a set. This never contains a native
    /// account locator, filesystem path, or provider response.
    pub enum CredentialSetSource => SET_SOURCES {
        None => "none",
        NativeV2 => "native_v2",
        FileV2 => "file_v2",
        LegacyKeychain => "legacy_keychain",
        LegacyYaml => "legacy_yaml",
        LegacyInlineSettings => "legacy_inline_settings",
        AmbientProviderChain => "ambient_provider_chain",
        PrivateSettingsLocator => "private_settings_locator",
    }
}

closed_vocabulary! {
    /// Redacted runtime projection of one credential-set record state.
    ///
    /// `Unknown` is reserved for pre-authority, opening, locked, or unavailable
    /// service states. It is not a valid persisted record state in a ready
    /// authority journal.
    pub enum CredentialSetRecordState => SET_RECORD_STATES {
        Unknown => "unknown",
        Missing => "missing",
        Configured => "configured",
        Tombstoned => "tombstoned",
        RecoveryRequired => "recovery_required",
    }
}

closed_vocabulary! {
    pub enum CredentialSetRecoveryState => SET_RECOVERY_STATES {
        None => "none",
        PendingIntent => "pending_intent",
        RecordJournalMismatch => "record_journal_mismatch",
        CommitUnknown => "commit_unknown",
    }
}

closed_vocabulary! {
    pub enum CredentialWorkerState => WORKER_STATES {
        Idle => "idle",
        Busy => "busy",
        Stalled => "stalled",
    }
}

closed_vocabulary! {
    pub enum CredentialActivationStage => ACTIVATION_STAGES {
        Staged => "staged",
        SettingsPending => "settings_pending",
        CredentialPending => "credential_pending",
        CleanupPending => "cleanup_pending",
        RecoveryRequired => "recovery_required",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialBackendStatus {
    pub kind: CredentialBackendKind,
    pub availability: CredentialBackendAvailability,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialWorkerStatus {
    pub state: CredentialWorkerState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<CredentialOperationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_id: Option<CredentialSetId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialPendingActivationStatus {
    pub operation_id: CredentialOperationId,
    pub set_id: CredentialSetId,
    pub stage: CredentialActivationStage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialSetStatus {
    pub set_id: CredentialSetId,
    pub record_state: CredentialSetRecordState,
    pub source: CredentialSetSource,
    pub cleanup_state: CredentialCleanupState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<CredentialRevision>,
    pub recovery_state: CredentialSetRecoveryState,
    pub pending_activation: bool,
    pub active_use_action: CredentialActiveUseAction,
}

/// Side-effect-free, journal-backed public status snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialServiceStatus {
    pub global_epoch: u64,
    pub backend: CredentialBackendStatus,
    pub migration_state: CredentialMigrationState,
    pub cleanup_state: CredentialCleanupState,
    pub worker: CredentialWorkerStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_activation: Option<CredentialPendingActivationStatus>,
    pub sets: Vec<CredentialSetStatus>,
}

closed_vocabulary! {
    pub enum CredentialErrorCode => ERROR_CODES {
        Missing => "missing",
        Locked => "locked",
        AccessDenied => "access_denied",
        Cancelled => "cancelled",
        StoreUnavailable => "store_unavailable",
        StoreUnsupported => "store_unsupported",
        CorruptRecord => "corrupt_record",
        UnsupportedSchema => "unsupported_schema",
        PayloadTooLarge => "payload_too_large",
        AmbiguousMatch => "ambiguous_match",
        Conflict => "conflict",
        MigrationRequired => "migration_required",
        MigrationConflict => "migration_conflict",
        RecoveryRequired => "recovery_required",
        LegacyCleanupRequired => "legacy_cleanup_required",
        PermissionHardeningFailed => "permission_hardening_failed",
        InvalidCredentialSet => "invalid_credential_set",
        AudienceNotAllowed => "audience_not_allowed",
        InsecureTransport => "insecure_transport",
        RevisionConflict => "revision_conflict",
        OperationInProgress => "operation_in_progress",
        StalledWorker => "stalled_worker",
        CommitUnknown => "commit_unknown",
        Internal => "internal",
    }
}

closed_vocabulary! {
    pub enum CredentialSafeRecoveryAction => RECOVERY_ACTIONS {
        None => "none",
        Retry => "retry",
        InitializeStore => "initialize_store",
        UnlockStore => "unlock_store",
        ReenterCredential => "reenter_credential",
        SelectMigrationSource => "select_migration_source",
        RunMigration => "run_migration",
        RunCleanup => "run_cleanup",
        Reconcile => "reconcile",
        RepairPermissions => "repair_permissions",
        ChooseSupportedBackend => "choose_supported_backend",
        RestartApplication => "restart_application",
    }
}

/// Content-free public error; native causes remain behind the backend boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialError {
    pub code: CredentialErrorCode,
    pub retryable: bool,
    pub recovery_action: CredentialSafeRecoveryAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set_id: Option<CredentialSetId>,
}

closed_vocabulary! {
    pub enum CredentialMutationResultCode => MUTATION_RESULT_CODES {
        Created => "created",
        Replaced => "replaced",
        Tombstoned => "tombstoned",
        AlreadyApplied => "already_applied",
        Recovered => "recovered",
        NoChange => "no_change",
    }
}

/// Safe receipt for an idempotent replace/delete/recovery operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialMutationReceipt {
    pub operation_id: CredentialOperationId,
    pub idempotency_token: CredentialIdempotencyToken,
    pub set_id: CredentialSetId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_revision: Option<CredentialRevision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_revision: Option<CredentialRevision>,
    pub result_code: CredentialMutationResultCode,
    pub recovery_action: CredentialSafeRecoveryAction,
}

/// Versioned response for a side-effect-free credential status snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialStatusEnvelope {
    pub schema_version: u32,
    pub status: CredentialServiceStatus,
}

impl CredentialStatusEnvelope {
    pub fn new(status: CredentialServiceStatus) -> Self {
        Self {
            schema_version: CREDENTIAL_CONTRACT_SCHEMA_VERSION,
            status,
        }
    }
}

/// Versioned response for a committed credential mutation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialMutationEnvelope {
    pub schema_version: u32,
    pub global_epoch: u64,
    pub receipt: CredentialMutationReceipt,
}

impl CredentialMutationEnvelope {
    pub fn new(global_epoch: u64, receipt: CredentialMutationReceipt) -> Self {
        Self {
            schema_version: CREDENTIAL_CONTRACT_SCHEMA_VERSION,
            global_epoch,
            receipt,
        }
    }
}

/// Versioned successful result of an explicit diagnose or unlock interaction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialDiagnosisEnvelope {
    pub schema_version: u32,
    pub global_epoch: u64,
    pub status: CredentialServiceStatus,
}

impl CredentialDiagnosisEnvelope {
    pub fn new(status: CredentialServiceStatus) -> Self {
        Self {
            schema_version: CREDENTIAL_CONTRACT_SCHEMA_VERSION,
            global_epoch: status.global_epoch,
            status,
        }
    }
}

/// Minimal notification published only after a credential mutation commits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialChangeNotification {
    pub schema_version: u32,
    pub global_epoch: u64,
    pub receipt: CredentialMutationReceipt,
}

impl CredentialChangeNotification {
    pub fn new(global_epoch: u64, receipt: CredentialMutationReceipt) -> Self {
        Self {
            schema_version: CREDENTIAL_CONTRACT_SCHEMA_VERSION,
            global_epoch,
            receipt,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CredentialContractVocabulary {
    pub field_classes: &'static [CredentialFieldClass],
    pub legacy_field_dispositions: &'static [LegacyFieldDisposition],
    pub secure_transport_schemes: &'static [SecureTransportScheme],
    pub aws_partitions: &'static [AwsPartition],
    pub aws_sdk_services: &'static [AwsSdkService],
    pub backend_kinds: &'static [CredentialBackendKind],
    pub backend_availabilities: &'static [CredentialBackendAvailability],
    pub migration_states: &'static [CredentialMigrationState],
    pub cleanup_states: &'static [CredentialCleanupState],
    pub set_sources: &'static [CredentialSetSource],
    pub set_record_states: &'static [CredentialSetRecordState],
    pub set_recovery_states: &'static [CredentialSetRecoveryState],
    pub worker_states: &'static [CredentialWorkerState],
    pub activation_stages: &'static [CredentialActivationStage],
    pub active_use_actions: &'static [CredentialActiveUseAction],
    pub use_policy_disabled_reasons: &'static [CredentialUsePolicyDisabledReason],
    pub error_codes: &'static [CredentialErrorCode],
    pub recovery_actions: &'static [CredentialSafeRecoveryAction],
    pub mutation_result_codes: &'static [CredentialMutationResultCode],
}

#[derive(Debug, Serialize)]
pub struct CredentialContractDefinition {
    pub schema_version: u32,
    pub portable_encoded_record_max_bytes: usize,
    pub built_in_set_ids: &'static [BuiltInCredentialSetId],
    pub auth_method_ids: &'static [AuthMethodId],
    pub purposes: &'static [CredentialPurpose],
    pub fields: &'static [CredentialFieldDefinition],
    pub sets: &'static [CredentialSetDefinition],
    pub use_policies: &'static [CredentialUsePolicyDefinition],
    pub custom_set_policy: CustomCredentialSetPolicy,
    pub vocabulary: CredentialContractVocabulary,
}

pub const CREDENTIAL_CONTRACT: CredentialContractDefinition = CredentialContractDefinition {
    schema_version: CREDENTIAL_CONTRACT_SCHEMA_VERSION,
    portable_encoded_record_max_bytes: PORTABLE_ENCODED_RECORD_MAX_BYTES,
    built_in_set_ids: BUILT_IN_CREDENTIAL_SET_IDS,
    auth_method_ids: AUTH_METHOD_IDS,
    purposes: CREDENTIAL_PURPOSES,
    fields: CREDENTIAL_FIELDS,
    sets: CREDENTIAL_SET_DEFINITIONS,
    use_policies: CREDENTIAL_USE_POLICIES,
    custom_set_policy: CUSTOM_CREDENTIAL_SET_POLICY,
    vocabulary: CredentialContractVocabulary {
        field_classes: CREDENTIAL_FIELD_CLASSES,
        legacy_field_dispositions: LEGACY_FIELD_DISPOSITIONS,
        secure_transport_schemes: SECURE_TRANSPORT_SCHEMES,
        aws_partitions: AWS_PARTITIONS,
        aws_sdk_services: AWS_SDK_SERVICES,
        backend_kinds: BACKEND_KINDS,
        backend_availabilities: BACKEND_AVAILABILITIES,
        migration_states: MIGRATION_STATES,
        cleanup_states: CLEANUP_STATES,
        set_sources: SET_SOURCES,
        set_record_states: SET_RECORD_STATES,
        set_recovery_states: SET_RECOVERY_STATES,
        worker_states: WORKER_STATES,
        activation_stages: ACTIVATION_STAGES,
        active_use_actions: ACTIVE_USE_ACTIONS,
        use_policy_disabled_reasons: USE_POLICY_DISABLED_REASONS,
        error_codes: ERROR_CODES,
        recovery_actions: RECOVERY_ACTIONS,
        mutation_result_codes: MUTATION_RESULT_CODES,
    },
};

pub fn credential_contract_typescript_module() -> String {
    let contract = serde_json::to_string_pretty(&CREDENTIAL_CONTRACT)
        .expect("credential contract should serialize");
    format!(
        r#"// @generated by src-tauri/crates/ipc-contract/src/credential_contract.rs. Do not edit manually.

// biome-ignore format: preserve the deterministic serde projection from Rust
export const CREDENTIAL_CONTRACT = {contract} as const;

export const PORTABLE_ENCODED_RECORD_MAX_BYTES =
  CREDENTIAL_CONTRACT.portable_encoded_record_max_bytes;
export const ALLOWED_CREDENTIAL_KEYS: readonly string[] =
  CREDENTIAL_CONTRACT.fields.map((field) => field.legacy_key);

export type LegacyCredentialKey =
  (typeof CREDENTIAL_CONTRACT.fields)[number]["legacy_key"];
export type BuiltInCredentialSetId =
  (typeof CREDENTIAL_CONTRACT.built_in_set_ids)[number];
/** Validated and issued by the backend; never synthesize this in the renderer. */
export type CustomCredentialSetId = `custom.${{string}}`;
export type CredentialSetId = BuiltInCredentialSetId | CustomCredentialSetId;
export type AuthMethodId = (typeof CREDENTIAL_CONTRACT.auth_method_ids)[number];
export type CredentialPurpose = (typeof CREDENTIAL_CONTRACT.purposes)[number];
export type CredentialFieldClass =
  (typeof CREDENTIAL_CONTRACT.vocabulary.field_classes)[number];
export type LegacyFieldDisposition =
  (typeof CREDENTIAL_CONTRACT.vocabulary.legacy_field_dispositions)[number];
export type CredentialFieldDefinition =
  (typeof CREDENTIAL_CONTRACT.fields)[number];
export type CredentialSetDefinition = (typeof CREDENTIAL_CONTRACT.sets)[number];
export type CredentialSetCompleteness =
  (typeof CREDENTIAL_CONTRACT.sets)[number]["configured_when"];
export type CredentialUsePolicyDefinition =
  (typeof CREDENTIAL_CONTRACT.use_policies)[number];

declare const credentialRevisionBrand: unique symbol;
declare const credentialOperationIdBrand: unique symbol;
declare const credentialIdempotencyTokenBrand: unique symbol;
/** Canonical lowercase UUID issued and validated by the backend. */
export type CredentialRevision = string & {{
  readonly [credentialRevisionBrand]: true;
}};
/** Canonical lowercase UUID issued and validated by the backend. */
export type CredentialOperationId = string & {{
  readonly [credentialOperationIdBrand]: true;
}};
/** Canonical lowercase UUID validated by the backend before it can be echoed. */
export type CredentialIdempotencyToken = string & {{
  readonly [credentialIdempotencyTokenBrand]: true;
}};
export type CredentialBackendKind =
  (typeof CREDENTIAL_CONTRACT.vocabulary.backend_kinds)[number];
export type CredentialBackendAvailability =
  (typeof CREDENTIAL_CONTRACT.vocabulary.backend_availabilities)[number];
export type CredentialMigrationState =
  (typeof CREDENTIAL_CONTRACT.vocabulary.migration_states)[number];
export type CredentialCleanupState =
  (typeof CREDENTIAL_CONTRACT.vocabulary.cleanup_states)[number];
export type CredentialSetSource =
  (typeof CREDENTIAL_CONTRACT.vocabulary.set_sources)[number];
/**
 * `unknown` is a runtime projection for pre-authority, opening, locked, or
 * unavailable states. It must not be persisted as a ready authority row.
 */
export type CredentialSetRecordState =
  (typeof CREDENTIAL_CONTRACT.vocabulary.set_record_states)[number];
export type CredentialSetRecoveryState =
  (typeof CREDENTIAL_CONTRACT.vocabulary.set_recovery_states)[number];
export type CredentialWorkerState =
  (typeof CREDENTIAL_CONTRACT.vocabulary.worker_states)[number];
export type CredentialActivationStage =
  (typeof CREDENTIAL_CONTRACT.vocabulary.activation_stages)[number];
export type CredentialActiveUseAction =
  (typeof CREDENTIAL_CONTRACT.vocabulary.active_use_actions)[number];
export type CredentialUsePolicyDisabledReason =
  (typeof CREDENTIAL_CONTRACT.vocabulary.use_policy_disabled_reasons)[number];
export type CredentialErrorCode =
  (typeof CREDENTIAL_CONTRACT.vocabulary.error_codes)[number];
export type CredentialSafeRecoveryAction =
  (typeof CREDENTIAL_CONTRACT.vocabulary.recovery_actions)[number];
export type CredentialMutationResultCode =
  (typeof CREDENTIAL_CONTRACT.vocabulary.mutation_result_codes)[number];

export type SecureTransportScheme =
  (typeof CREDENTIAL_CONTRACT.vocabulary.secure_transport_schemes)[number];
export type AwsPartition =
  (typeof CREDENTIAL_CONTRACT.vocabulary.aws_partitions)[number];
export type AwsSdkService =
  (typeof CREDENTIAL_CONTRACT.vocabulary.aws_sdk_services)[number];
export type CredentialAudience =
  | {{
      kind: "secure_network_origin";
      scheme: SecureTransportScheme;
      canonical_host: string;
      effective_port: number;
    }}
  | {{
      kind: "aws_sdk";
      partition: AwsPartition;
      service: AwsSdkService;
      region: string;
    }};

// Public passive/mutation DTOs. These shapes are intentionally content-free.
export interface CredentialBackendStatus {{
  kind: CredentialBackendKind;
  availability: CredentialBackendAvailability;
}}

export interface CredentialWorkerStatus {{
  state: CredentialWorkerState;
  operation_id?: CredentialOperationId | null;
  set_id?: CredentialSetId | null;
}}

export interface CredentialPendingActivationStatus {{
  operation_id: CredentialOperationId;
  set_id: CredentialSetId;
  stage: CredentialActivationStage;
}}

export interface CredentialSetStatus {{
  set_id: CredentialSetId;
  record_state: CredentialSetRecordState;
  source: CredentialSetSource;
  cleanup_state: CredentialCleanupState;
  revision?: CredentialRevision | null;
  recovery_state: CredentialSetRecoveryState;
  pending_activation: boolean;
  active_use_action: CredentialActiveUseAction;
}}

export interface CredentialServiceStatus {{
  global_epoch: number;
  backend: CredentialBackendStatus;
  migration_state: CredentialMigrationState;
  cleanup_state: CredentialCleanupState;
  worker: CredentialWorkerStatus;
  pending_activation?: CredentialPendingActivationStatus | null;
  sets: CredentialSetStatus[];
}}

export interface CredentialError {{
  code: CredentialErrorCode;
  retryable: boolean;
  recovery_action: CredentialSafeRecoveryAction;
  set_id?: CredentialSetId | null;
}}

export interface CredentialMutationReceipt {{
  operation_id: CredentialOperationId;
  idempotency_token: CredentialIdempotencyToken;
  set_id: CredentialSetId;
  previous_revision?: CredentialRevision | null;
  new_revision?: CredentialRevision | null;
  result_code: CredentialMutationResultCode;
  recovery_action: CredentialSafeRecoveryAction;
}}

export interface CredentialStatusEnvelope {{
  schema_version: typeof CREDENTIAL_CONTRACT.schema_version;
  status: CredentialServiceStatus;
}}

export interface CredentialMutationEnvelope {{
  schema_version: typeof CREDENTIAL_CONTRACT.schema_version;
  global_epoch: number;
  receipt: CredentialMutationReceipt;
}}

export interface CredentialDiagnosisEnvelope {{
  schema_version: typeof CREDENTIAL_CONTRACT.schema_version;
  global_epoch: number;
  status: CredentialServiceStatus;
}}

export interface CredentialChangeNotification {{
  schema_version: typeof CREDENTIAL_CONTRACT.schema_version;
  global_epoch: number;
  receipt: CredentialMutationReceipt;
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint_credential_routing::{
        DEFAULT_ENDPOINT_CREDENTIAL_KEY, ENDPOINT_CREDENTIAL_ROUTING, SAVED_ENDPOINT_AUDIENCES,
    };
    use serde_json::Value;
    use std::collections::HashSet;

    #[test]
    fn all_twenty_two_v1_keys_have_one_explicit_disposition() {
        assert_eq!(ALLOWED_CREDENTIAL_KEYS.len(), 22);
        assert_eq!(CREDENTIAL_FIELDS.len(), 22);
        let keys: HashSet<_> = ALLOWED_CREDENTIAL_KEYS.iter().copied().collect();
        assert_eq!(keys.len(), 22);
        assert_eq!(
            keys,
            CREDENTIAL_FIELDS
                .iter()
                .map(|field| field.legacy_key)
                .collect()
        );
        assert_eq!(
            CREDENTIAL_FIELDS
                .iter()
                .filter(|field| field.legacy_disposition == Disposition::Migrate)
                .count(),
            19
        );
        assert_eq!(
            CREDENTIAL_FIELDS
                .iter()
                .filter(|field| field.legacy_disposition == Disposition::Config)
                .count(),
            3
        );
    }

    #[test]
    fn every_field_maps_to_a_declared_set_and_auth_method() {
        for field in CREDENTIAL_FIELDS {
            credential_set_definition(field.set_id)
                .unwrap_or_else(|| panic!("missing set for {}", field.legacy_key));
            assert!(!field.auth_method_ids.is_empty());
            for auth_method in field.auth_method_ids {
                assert!(AUTH_METHOD_IDS.contains(auth_method));
                assert!(
                    CREDENTIAL_USE_POLICIES.iter().any(|policy| {
                        policy.set_id == field.set_id && policy.auth_method_id == *auth_method
                    }),
                    "{} has no explicit authorized or disabled use relation for {:?}",
                    field.legacy_key,
                    auth_method
                );
            }
            if field.contributes_to_credential_presence {
                assert_eq!(
                    field.class,
                    Class::Secret,
                    "only stored secret fields can establish credential presence: {}",
                    field.legacy_key
                );
            }
            if field.class != Class::Secret {
                assert!(!field.contributes_to_credential_presence);
            }
        }
    }

    #[test]
    fn endpoint_routing_saved_keys_map_to_the_contract() {
        let keys = SAVED_ENDPOINT_AUDIENCES
            .iter()
            .map(|audience| audience.credential_key)
            .chain(
                ENDPOINT_CREDENTIAL_ROUTING
                    .iter()
                    .map(|route| route.credential_key),
            )
            .chain(std::iter::once(DEFAULT_ENDPOINT_CREDENTIAL_KEY));
        for key in keys {
            let field = credential_field_for_legacy_key(key)
                .unwrap_or_else(|| panic!("endpoint routing key {key} is unmapped"));
            assert!(credential_set_definition(field.set_id).is_some());
        }
    }

    #[test]
    fn exact_secure_origin_policies_are_canonical_origins() {
        for policy in CREDENTIAL_USE_POLICIES {
            let CredentialUsePolicyDecisionDefinition::Authorized {
                audience: CredentialAudiencePolicyDefinition::ExactSecureOrigin { origin },
            } = policy.decision
            else {
                continue;
            };
            let parsed = url::Url::parse(origin).unwrap_or_else(|error| {
                panic!(
                    "set {} consumer {} has invalid origin {origin}: {error}",
                    policy.set_id.as_str(),
                    policy.consumer_id
                )
            });
            assert!(matches!(parsed.scheme(), "https" | "wss"));
            assert!(parsed.host().is_some());
            assert!(parsed.username().is_empty());
            assert!(parsed.password().is_none());
            assert_eq!(parsed.path(), "/");
            assert!(parsed.query().is_none());
            assert!(parsed.fragment().is_none());
            assert_eq!(
                parsed.origin().ascii_serialization(),
                *origin,
                "origin must already be canonical"
            );
        }
    }

    #[test]
    fn use_policy_rows_are_atomic_unique_and_closed() {
        let rows: HashSet<_> = CREDENTIAL_USE_POLICIES
            .iter()
            .map(|policy| serde_json::to_string(policy).expect("policy JSON"))
            .collect();
        assert_eq!(rows.len(), CREDENTIAL_USE_POLICIES.len());

        for policy in CREDENTIAL_USE_POLICIES {
            assert!(credential_set_definition(policy.set_id).is_some());
            assert!(AUTH_METHOD_IDS.contains(&policy.auth_method_id));
            assert!(CREDENTIAL_PURPOSES.contains(&policy.purpose));
            assert!(ACTIVE_USE_ACTIONS.contains(&policy.active_use_action));
            if matches!(
                policy.decision,
                CredentialUsePolicyDecisionDefinition::Disabled { .. }
            ) {
                assert_eq!(policy.active_use_action, Action::Stop);
            }
        }

        let openai_per_request: Vec<_> = CREDENTIAL_USE_POLICIES
            .iter()
            .filter(|policy| {
                policy.set_id == Set::Openai && matches!(policy.consumer_id, "asr.api" | "llm.api")
            })
            .collect();
        assert!(!openai_per_request.is_empty());
        assert!(
            openai_per_request
                .iter()
                .all(|policy| policy.active_use_action == Action::RefreshBeforeNextUse)
        );

        let openai_realtime: Vec<_> = CREDENTIAL_USE_POLICIES
            .iter()
            .filter(|policy| {
                policy.set_id == Set::Openai && policy.consumer_id.contains("realtime")
            })
            .collect();
        assert!(!openai_realtime.is_empty());
        assert!(
            openai_realtime
                .iter()
                .all(|policy| policy.active_use_action == Action::Reauthenticate)
        );

        let consumers: HashSet<_> = CREDENTIAL_USE_POLICIES
            .iter()
            .map(|policy| policy.consumer_id)
            .collect();
        assert_eq!(
            consumers,
            HashSet::from([
                "asr.api",
                "asr.assemblyai",
                "asr.aws_transcribe",
                "asr.azure_speech",
                "asr.deepgram",
                "asr.elevenlabs_scribe",
                "asr.gladia",
                "asr.openai_realtime",
                "asr.revai",
                "asr.soniox",
                "asr.speechmatics",
                "llm.api",
                "llm.aws_bedrock",
                "llm.cerebras",
                "llm.openrouter",
                "llm.sambanova",
                "realtime_agent.gemini_live",
                "realtime_agent.openai_realtime",
                "tts.deepgram_aura",
            ])
        );
        for policy in CREDENTIAL_USE_POLICIES {
            let expected = if matches!(
                policy.decision,
                CredentialUsePolicyDecisionDefinition::Disabled { .. }
            ) {
                Action::Stop
            } else if policy.purpose == Purpose::HealthCheck
                || matches!(
                    policy.consumer_id,
                    "asr.api"
                        | "asr.gladia"
                        | "llm.api"
                        | "llm.aws_bedrock"
                        | "llm.cerebras"
                        | "llm.openrouter"
                        | "llm.sambanova"
                )
            {
                Action::RefreshBeforeNextUse
            } else {
                Action::Reauthenticate
            };
            assert_eq!(
                policy.active_use_action,
                expected,
                "unexpected action for {} / {} / {:?}",
                policy.set_id.as_str(),
                policy.consumer_id,
                policy.purpose
            );
        }
    }

    #[test]
    fn gemini_auth_modes_have_disjoint_runtime_audiences() {
        for policy in CREDENTIAL_USE_POLICIES
            .iter()
            .filter(|policy| policy.set_id == Set::Gemini)
        {
            match (policy.auth_method_id, policy.decision) {
                (
                    Auth::ApiKey,
                    CredentialUsePolicyDecisionDefinition::Authorized {
                        audience: CredentialAudiencePolicyDefinition::ExactSecureOrigin { origin },
                    },
                ) => assert!(matches!(
                    origin,
                    "https://generativelanguage.googleapis.com"
                        | "wss://generativelanguage.googleapis.com"
                )),
                (
                    Auth::GoogleServiceAccountFile,
                    CredentialUsePolicyDecisionDefinition::Authorized {
                        audience:
                            CredentialAudiencePolicyDefinition::BackendDerivedVertexOrigin {
                                scheme: SecureTransportScheme::Wss,
                                host_suffix: "aiplatform.googleapis.com",
                                effective_port: 443,
                            },
                    },
                ) => {}
                _ => panic!("Gemini auth mode escaped its closed audience policy"),
            }
        }

        let api_policy = CREDENTIAL_USE_POLICIES
            .iter()
            .find(|policy| {
                policy.set_id == Set::Gemini
                    && policy.auth_method_id == Auth::ApiKey
                    && policy.purpose == Purpose::RealtimeAgent
            })
            .expect("Gemini API-key realtime policy");
        let vertex_policy = CREDENTIAL_USE_POLICIES
            .iter()
            .find(|policy| {
                policy.set_id == Set::Gemini
                    && policy.auth_method_id == Auth::GoogleServiceAccountFile
                    && policy.purpose == Purpose::RealtimeAgent
            })
            .expect("Gemini Vertex realtime policy");

        let generative_language = CredentialAudience::SecureNetworkOrigin {
            scheme: SecureTransportScheme::Wss,
            canonical_host: "generativelanguage.googleapis.com".into(),
            effective_port: 443,
        };
        let vertex = CredentialAudience::SecureNetworkOrigin {
            scheme: SecureTransportScheme::Wss,
            canonical_host: "us-central1-aiplatform.googleapis.com".into(),
            effective_port: 443,
        };
        assert!(credential_use_policy_allows_audience(
            api_policy,
            &generative_language
        ));
        assert!(!credential_use_policy_allows_audience(api_policy, &vertex));
        assert!(credential_use_policy_allows_audience(
            vertex_policy,
            &vertex
        ));
        assert!(!credential_use_policy_allows_audience(
            vertex_policy,
            &CredentialAudience::SecureNetworkOrigin {
                scheme: SecureTransportScheme::Https,
                canonical_host: "us-central1-aiplatform.googleapis.com".into(),
                effective_port: 443,
            }
        ));
        assert!(!credential_use_policy_allows_audience(
            vertex_policy,
            &generative_language
        ));

        for invalid_host in [
            "US-central1-aiplatform.googleapis.com",
            "-aiplatform.googleapis.com",
            "us-central1.aiplatform.googleapis.com",
            "us-central1-aiplatform.googleapis.com.evil.test",
        ] {
            assert!(!credential_use_policy_allows_audience(
                vertex_policy,
                &CredentialAudience::SecureNetworkOrigin {
                    scheme: SecureTransportScheme::Wss,
                    canonical_host: invalid_host.into(),
                    effective_port: 443,
                }
            ));
        }
        assert!(is_valid_vertex_location("us-central1"));
        assert!(!is_valid_vertex_location("us-central1/evil"));
        assert_eq!(
            vertex_origin_host("us-central1").as_deref(),
            Some("us-central1-aiplatform.googleapis.com")
        );
        assert!(
            !CREDENTIAL_USE_POLICIES
                .iter()
                .any(|policy| { policy.set_id == Set::Gemini && policy.consumer_id == "llm.api" })
        );
    }

    #[test]
    fn aws_static_fields_form_one_atomic_generation() {
        let aws_fields: Vec<_> = CREDENTIAL_FIELDS
            .iter()
            .filter(|field| field.set_id == Set::Aws)
            .collect();
        let required_pair: HashSet<_> = aws_fields
            .iter()
            .filter(|field| {
                field.requirement
                    == Requirement::RequiredTogether {
                        group_id: "aws.static_pair",
                    }
            })
            .map(|field| field.legacy_key)
            .collect();
        assert_eq!(
            required_pair,
            HashSet::from(["aws_access_key", "aws_secret_key"])
        );
        let session = credential_field_for_legacy_key("aws_session_token").expect("session token");
        assert_eq!(session.requirement, Requirement::Optional);
        assert!(!session.contributes_to_credential_presence);
        assert_eq!(
            credential_set_definition(Set::Aws)
                .expect("AWS set")
                .configured_when,
            CredentialSetCompleteness::RequiredTogether {
                group_id: "aws.static_pair"
            }
        );
        for key in ["aws_profile", "aws_region"] {
            let field = credential_field_for_legacy_key(key).expect("AWS config field");
            assert_eq!(field.class, Class::OrdinaryConfig);
            assert!(!field.contributes_to_credential_presence);
        }
    }

    #[test]
    fn gemini_authentication_is_an_explicit_secret_or_locator_alternative() {
        let alternatives: Vec<_> = CREDENTIAL_FIELDS
            .iter()
            .filter(|field| {
                field.requirement
                    == Requirement::Alternative {
                        group_id: "gemini.authentication",
                    }
            })
            .collect();
        assert_eq!(alternatives.len(), 2);
        let api_key = credential_field_for_legacy_key("gemini_api_key").expect("Gemini key");
        assert_eq!(api_key.class, Class::Secret);
        assert!(api_key.contributes_to_credential_presence);
        let locator = credential_field_for_legacy_key("google_service_account_path")
            .expect("service account locator");
        assert_eq!(locator.class, Class::PrivateLocator);
        assert!(!locator.contributes_to_credential_presence);
        assert_eq!(locator.legacy_disposition, Disposition::Config);
        assert_eq!(
            credential_set_definition(Set::Gemini)
                .expect("Gemini set")
                .configured_when,
            CredentialSetCompleteness::AnyStoredSecretAlternative {
                group_id: "gemini.authentication"
            }
        );
    }

    #[test]
    fn ids_are_unique_stable_and_custom_ids_are_canonical() {
        let built_in: HashSet<_> = BUILT_IN_CREDENTIAL_SET_IDS
            .iter()
            .map(|id| id.as_str())
            .collect();
        assert_eq!(built_in.len(), BUILT_IN_CREDENTIAL_SET_IDS.len());
        assert_eq!(
            CREDENTIAL_SET_DEFINITIONS
                .iter()
                .map(|definition| definition.id.as_str())
                .collect::<HashSet<_>>(),
            built_in
        );
        for id in built_in {
            assert!(id.len() <= 64);
            assert!(id.bytes().enumerate().all(|(index, byte)| {
                (index != 0 || byte.is_ascii_alphanumeric())
                    && (byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'.' | b'_' | b'-'))
            }));
        }

        let auth_method_ids: HashSet<_> = AUTH_METHOD_IDS
            .iter()
            .map(|id| serde_json::to_string(id).expect("auth method id should serialize"))
            .collect();
        assert_eq!(auth_method_ids.len(), AUTH_METHOD_IDS.len());
        for id in BUILT_IN_CREDENTIAL_SET_IDS {
            assert_eq!(
                serde_json::to_value(id).expect("set id should serialize"),
                Value::String(id.as_str().to_owned())
            );
        }

        let valid = "custom.123e4567-e89b-12d3-a456-426614174000";
        assert!(is_canonical_custom_credential_set_id(valid));
        assert_eq!(
            valid
                .parse::<CredentialSetId>()
                .expect("canonical custom id")
                .as_str(),
            valid
        );
        for invalid in [
            "custom.123E4567-e89b-12d3-a456-426614174000",
            "custom.123e4567e89b12d3a456426614174000",
            "custom.not-a-uuid",
            "CUSTOM.123e4567-e89b-12d3-a456-426614174000",
            "openai.extra",
        ] {
            assert!(!is_canonical_custom_credential_set_id(invalid));
            assert!(invalid.parse::<CredentialSetId>().is_err());
        }
    }

    #[test]
    fn all_macro_derived_vocabularies_are_unique_on_the_wire() {
        fn assert_unique<T: Serialize>(label: &str, values: &[T]) {
            assert!(!values.is_empty(), "{label} vocabulary is empty");
            let encoded: HashSet<_> = values
                .iter()
                .map(|value| serde_json::to_string(value).expect("vocabulary JSON"))
                .collect();
            assert_eq!(encoded.len(), values.len(), "duplicate {label} wire value");
        }

        for (label, values) in [
            (
                "built-in sets",
                serde_json::to_value(BUILT_IN_CREDENTIAL_SET_IDS).unwrap(),
            ),
            (
                "auth methods",
                serde_json::to_value(AUTH_METHOD_IDS).unwrap(),
            ),
            (
                "field classes",
                serde_json::to_value(CREDENTIAL_FIELD_CLASSES).unwrap(),
            ),
            (
                "legacy dispositions",
                serde_json::to_value(LEGACY_FIELD_DISPOSITIONS).unwrap(),
            ),
            (
                "purposes",
                serde_json::to_value(CREDENTIAL_PURPOSES).unwrap(),
            ),
            (
                "secure schemes",
                serde_json::to_value(SECURE_TRANSPORT_SCHEMES).unwrap(),
            ),
            (
                "AWS partitions",
                serde_json::to_value(AWS_PARTITIONS).unwrap(),
            ),
            (
                "AWS services",
                serde_json::to_value(AWS_SDK_SERVICES).unwrap(),
            ),
            (
                "active-use actions",
                serde_json::to_value(ACTIVE_USE_ACTIONS).unwrap(),
            ),
            (
                "disabled reasons",
                serde_json::to_value(USE_POLICY_DISABLED_REASONS).unwrap(),
            ),
            (
                "backend kinds",
                serde_json::to_value(BACKEND_KINDS).unwrap(),
            ),
            (
                "backend availability",
                serde_json::to_value(BACKEND_AVAILABILITIES).unwrap(),
            ),
            (
                "migration states",
                serde_json::to_value(MIGRATION_STATES).unwrap(),
            ),
            (
                "cleanup states",
                serde_json::to_value(CLEANUP_STATES).unwrap(),
            ),
            ("set sources", serde_json::to_value(SET_SOURCES).unwrap()),
            (
                "set record states",
                serde_json::to_value(SET_RECORD_STATES).unwrap(),
            ),
            (
                "set recovery states",
                serde_json::to_value(SET_RECOVERY_STATES).unwrap(),
            ),
            (
                "worker states",
                serde_json::to_value(WORKER_STATES).unwrap(),
            ),
            (
                "activation stages",
                serde_json::to_value(ACTIVATION_STAGES).unwrap(),
            ),
            ("error codes", serde_json::to_value(ERROR_CODES).unwrap()),
            (
                "recovery actions",
                serde_json::to_value(RECOVERY_ACTIONS).unwrap(),
            ),
            (
                "mutation results",
                serde_json::to_value(MUTATION_RESULT_CODES).unwrap(),
            ),
        ] {
            let Value::Array(values) = values else {
                panic!("{label} did not serialize as an array");
            };
            assert_unique(label, &values);
        }
    }

    #[test]
    fn lifecycle_projection_vocabularies_include_unknown_and_initialization() {
        assert_eq!(
            serde_json::to_value(CredentialSetRecordState::Unknown).unwrap(),
            Value::String("unknown".into())
        );
        assert!(SET_RECORD_STATES.contains(&CredentialSetRecordState::Unknown));
        assert_eq!(
            serde_json::to_value(CredentialSafeRecoveryAction::InitializeStore).unwrap(),
            Value::String("initialize_store".into())
        );
        assert!(RECOVERY_ACTIONS.contains(&CredentialSafeRecoveryAction::InitializeStore));
    }

    #[test]
    fn public_tokens_accept_only_canonical_128_bit_values() {
        let valid = "123e4567-e89b-12d3-a456-426614174000";
        assert_eq!(CredentialRevision::parse(valid).unwrap().as_str(), valid);
        assert_eq!(CredentialOperationId::parse(valid).unwrap().as_str(), valid);
        assert_eq!(
            CredentialIdempotencyToken::parse(valid).unwrap().as_str(),
            valid
        );

        for invalid in [
            "sk-live-super-secret",
            "/home/alice/.config/provider.json",
            "native keychain access denied for account alice@example.test",
            "123E4567-e89b-12d3-a456-426614174000",
            "123e4567e89b12d3a456426614174000",
            "",
        ] {
            assert!(CredentialRevision::parse(invalid).is_err());
            assert!(CredentialOperationId::parse(invalid).is_err());
            assert!(CredentialIdempotencyToken::parse(invalid).is_err());
            assert!(
                serde_json::from_value::<CredentialRevision>(Value::String(invalid.into()))
                    .is_err()
            );
        }
    }

    fn assert_content_free_keys(value: &Value) {
        const FORBIDDEN: &[&str] = &[
            "secret",
            "value",
            "private_locator",
            "length",
            "fingerprint",
            "native_error",
            "message",
        ];
        match value {
            Value::Object(object) => {
                for (key, nested) in object {
                    assert!(
                        !FORBIDDEN.contains(&key.as_str()),
                        "forbidden public key {key}"
                    );
                    assert_content_free_keys(nested);
                }
            }
            Value::Array(values) => values.iter().for_each(assert_content_free_keys),
            _ => {}
        }
    }

    #[test]
    fn public_envelopes_are_versioned_content_free_and_notification_is_minimal() {
        let status = CredentialServiceStatus {
            global_epoch: 11,
            backend: CredentialBackendStatus {
                kind: CredentialBackendKind::Native,
                availability: CredentialBackendAvailability::Locked,
            },
            migration_state: CredentialMigrationState::Uninitialized,
            cleanup_state: CredentialCleanupState::NotApplicable,
            worker: CredentialWorkerStatus {
                state: CredentialWorkerState::Idle,
                operation_id: None,
                set_id: None,
            },
            pending_activation: None,
            sets: Vec::new(),
        };
        let receipt = CredentialMutationReceipt {
            operation_id: CredentialOperationId::parse("123e4567-e89b-12d3-a456-426614174000")
                .expect("valid operation id"),
            idempotency_token: CredentialIdempotencyToken::parse(
                "223e4567-e89b-12d3-a456-426614174000",
            )
            .expect("valid idempotency token"),
            set_id: CredentialSetId::BuiltIn(Set::Openai),
            previous_revision: None,
            new_revision: None,
            result_code: CredentialMutationResultCode::NoChange,
            recovery_action: CredentialSafeRecoveryAction::InitializeStore,
        };

        let status_envelope = CredentialStatusEnvelope::new(status.clone());
        let mutation_envelope = CredentialMutationEnvelope::new(11, receipt.clone());
        let diagnosis_envelope = CredentialDiagnosisEnvelope::new(status);
        let notification = CredentialChangeNotification::new(11, receipt);

        assert_eq!(
            status_envelope.schema_version,
            CREDENTIAL_CONTRACT_SCHEMA_VERSION
        );
        assert_eq!(
            mutation_envelope.schema_version,
            CREDENTIAL_CONTRACT_SCHEMA_VERSION
        );
        assert_eq!(
            diagnosis_envelope.schema_version,
            CREDENTIAL_CONTRACT_SCHEMA_VERSION
        );
        assert_eq!(diagnosis_envelope.global_epoch, 11);
        assert_eq!(
            notification.schema_version,
            CREDENTIAL_CONTRACT_SCHEMA_VERSION
        );

        for value in [
            serde_json::to_value(status_envelope).expect("status envelope JSON"),
            serde_json::to_value(mutation_envelope).expect("mutation envelope JSON"),
            serde_json::to_value(diagnosis_envelope).expect("diagnosis envelope JSON"),
        ] {
            assert_content_free_keys(&value);
        }

        let Value::Object(notification_fields) =
            serde_json::to_value(notification).expect("change notification JSON")
        else {
            panic!("change notification did not serialize as an object");
        };
        assert_eq!(
            notification_fields
                .keys()
                .map(String::as_str)
                .collect::<HashSet<_>>(),
            HashSet::from(["schema_version", "global_epoch", "receipt"])
        );
        assert_content_free_keys(&Value::Object(notification_fields));
    }

    #[test]
    fn public_response_dtos_have_no_plaintext_bearing_fields() {
        let set_id = CredentialSetId::BuiltIn(Set::Openai);
        let status = CredentialServiceStatus {
            global_epoch: 7,
            backend: CredentialBackendStatus {
                kind: CredentialBackendKind::Native,
                availability: CredentialBackendAvailability::Available,
            },
            migration_state: CredentialMigrationState::Ready,
            cleanup_state: CredentialCleanupState::Pending,
            worker: CredentialWorkerStatus {
                state: CredentialWorkerState::Idle,
                operation_id: None,
                set_id: None,
            },
            pending_activation: None,
            sets: vec![CredentialSetStatus {
                set_id: set_id.clone(),
                record_state: CredentialSetRecordState::Configured,
                source: CredentialSetSource::NativeV2,
                cleanup_state: CredentialCleanupState::Completed,
                revision: Some(
                    CredentialRevision::parse("123e4567-e89b-12d3-a456-426614174000")
                        .expect("valid revision"),
                ),
                recovery_state: CredentialSetRecoveryState::None,
                pending_activation: false,
                active_use_action: CredentialActiveUseAction::None,
            }],
        };
        let error = CredentialError {
            code: CredentialErrorCode::Locked,
            retryable: true,
            recovery_action: CredentialSafeRecoveryAction::UnlockStore,
            set_id: Some(set_id.clone()),
        };
        let receipt = CredentialMutationReceipt {
            operation_id: CredentialOperationId::parse("223e4567-e89b-12d3-a456-426614174000")
                .expect("valid operation id"),
            idempotency_token: CredentialIdempotencyToken::parse(
                "323e4567-e89b-12d3-a456-426614174000",
            )
            .expect("valid idempotency token"),
            set_id,
            previous_revision: None,
            new_revision: Some(
                CredentialRevision::parse("423e4567-e89b-12d3-a456-426614174000")
                    .expect("valid revision"),
            ),
            result_code: CredentialMutationResultCode::Created,
            recovery_action: CredentialSafeRecoveryAction::None,
        };
        for value in [
            serde_json::to_value(status).expect("status JSON"),
            serde_json::to_value(error).expect("error JSON"),
            serde_json::to_value(receipt).expect("receipt JSON"),
        ] {
            assert_content_free_keys(&value);
        }

        let module = credential_contract_typescript_module();
        let public_dtos = module
            .split_once("// Public passive/mutation DTOs.")
            .expect("public DTO marker")
            .1;
        for forbidden in [
            "secret:",
            "value:",
            "private_locator:",
            "length:",
            "fingerprint:",
            "native_error:",
            "message:",
        ] {
            assert!(
                !public_dtos.contains(forbidden),
                "generated DTO contains {forbidden}"
            );
        }
        assert!(
            !public_dtos.contains("CredentialReplacementDraft"),
            "secret-bearing replacement drafts must remain backend-only"
        );
        for legacy_key in ALLOWED_CREDENTIAL_KEYS {
            assert!(
                !public_dtos.contains(legacy_key),
                "generated public DTO exposes legacy plaintext field {legacy_key}"
            );
        }
    }

    #[test]
    fn portable_record_ceiling_is_exact() {
        assert_eq!(PORTABLE_ENCODED_RECORD_MAX_BYTES, 2_560);
    }

    #[test]
    fn generated_credential_contract_ts_is_current() {
        let generated = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../src/generated/credentialContract.ts");
        let actual = std::fs::read_to_string(&generated).unwrap_or_else(|error| {
            panic!(
                "failed to read generated credential contract {}: {error}",
                generated.display()
            )
        });
        assert_eq!(
            actual,
            credential_contract_typescript_module(),
            "generated credential contract drifted; run `bun run generate:credential-contract`"
        );
    }
}
