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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuiltInCredentialSetId {
    Openai,
    Cerebras,
    Sambanova,
    Openrouter,
    Groq,
    Together,
    Fireworks,
    Deepgram,
    Assemblyai,
    Soniox,
    Gladia,
    Speechmatics,
    Elevenlabs,
    Revai,
    AzureSpeech,
    Gemini,
    Aws,
}

impl BuiltInCredentialSetId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Openai => "openai",
            Self::Cerebras => "cerebras",
            Self::Sambanova => "sambanova",
            Self::Openrouter => "openrouter",
            Self::Groq => "groq",
            Self::Together => "together",
            Self::Fireworks => "fireworks",
            Self::Deepgram => "deepgram",
            Self::Assemblyai => "assemblyai",
            Self::Soniox => "soniox",
            Self::Gladia => "gladia",
            Self::Speechmatics => "speechmatics",
            Self::Elevenlabs => "elevenlabs",
            Self::Revai => "revai",
            Self::AzureSpeech => "azure_speech",
            Self::Gemini => "gemini",
            Self::Aws => "aws",
        }
    }
}

pub const BUILT_IN_CREDENTIAL_SET_IDS: &[BuiltInCredentialSetId] = &[
    BuiltInCredentialSetId::Openai,
    BuiltInCredentialSetId::Cerebras,
    BuiltInCredentialSetId::Sambanova,
    BuiltInCredentialSetId::Openrouter,
    BuiltInCredentialSetId::Groq,
    BuiltInCredentialSetId::Together,
    BuiltInCredentialSetId::Fireworks,
    BuiltInCredentialSetId::Deepgram,
    BuiltInCredentialSetId::Assemblyai,
    BuiltInCredentialSetId::Soniox,
    BuiltInCredentialSetId::Gladia,
    BuiltInCredentialSetId::Speechmatics,
    BuiltInCredentialSetId::Elevenlabs,
    BuiltInCredentialSetId::Revai,
    BuiltInCredentialSetId::AzureSpeech,
    BuiltInCredentialSetId::Gemini,
    BuiltInCredentialSetId::Aws,
];

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthMethodId {
    ApiKey,
    GoogleServiceAccountFile,
    AwsStatic,
    AwsProfile,
    AwsDefaultChain,
    CustomBearerApiKey,
}

pub const AUTH_METHOD_IDS: &[AuthMethodId] = &[
    AuthMethodId::ApiKey,
    AuthMethodId::GoogleServiceAccountFile,
    AuthMethodId::AwsStatic,
    AuthMethodId::AwsProfile,
    AuthMethodId::AwsDefaultChain,
    AuthMethodId::CustomBearerApiKey,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialFieldClass {
    Secret,
    PrivateLocator,
    OrdinaryConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyFieldDisposition {
    Migrate,
    Config,
    Deprecate,
    Remove,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialPurpose {
    Asr,
    Llm,
    Tts,
    RealtimeAgent,
    ModelCatalog,
    HealthCheck,
    VertexAuthentication,
}

pub const CREDENTIAL_PURPOSES: &[CredentialPurpose] = &[
    CredentialPurpose::Asr,
    CredentialPurpose::Llm,
    CredentialPurpose::Tts,
    CredentialPurpose::RealtimeAgent,
    CredentialPurpose::ModelCatalog,
    CredentialPurpose::HealthCheck,
    CredentialPurpose::VertexAuthentication,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecureTransportScheme {
    Https,
    Wss,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AwsPartition {
    Aws,
    AwsCn,
    AwsUsGov,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AwsSdkService {
    TranscribeStreaming,
    BedrockRuntime,
    Sts,
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

/// Closed policy declaration attached to each built-in set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CredentialAudiencePolicyDefinition {
    ExactSecureOrigin {
        origin: &'static str,
        purposes: &'static [CredentialPurpose],
    },
    AwsSdk {
        partitions: &'static [AwsPartition],
        services: &'static [AwsSdkService],
        purposes: &'static [CredentialPurpose],
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialActiveUseAction {
    None,
    RefreshBeforeNextUse,
    Reauthenticate,
    Stop,
    RestartApplication,
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
    pub auth_method_ids: &'static [AuthMethodId],
    pub allowed_consumers: &'static [&'static str],
    pub purposes: &'static [CredentialPurpose],
    pub audience_policies: &'static [CredentialAudiencePolicyDefinition],
    pub configured_when: CredentialSetCompleteness,
    pub active_use_action: CredentialActiveUseAction,
}

const ASR_LLM: &[CredentialPurpose] = &[CredentialPurpose::Asr, CredentialPurpose::Llm];
const LLM_CATALOG_HEALTH: &[CredentialPurpose] = &[
    CredentialPurpose::Llm,
    CredentialPurpose::ModelCatalog,
    CredentialPurpose::HealthCheck,
];
const ASR_HEALTH: &[CredentialPurpose] = &[CredentialPurpose::Asr, CredentialPurpose::HealthCheck];
const ASR_TTS_HEALTH: &[CredentialPurpose] = &[
    CredentialPurpose::Asr,
    CredentialPurpose::Tts,
    CredentialPurpose::HealthCheck,
];
const OPENAI_PURPOSES: &[CredentialPurpose] = &[
    CredentialPurpose::Asr,
    CredentialPurpose::Llm,
    CredentialPurpose::Tts,
    CredentialPurpose::RealtimeAgent,
    CredentialPurpose::ModelCatalog,
    CredentialPurpose::HealthCheck,
];
const GEMINI_PURPOSES: &[CredentialPurpose] = &[
    CredentialPurpose::Asr,
    CredentialPurpose::Llm,
    CredentialPurpose::RealtimeAgent,
    CredentialPurpose::ModelCatalog,
    CredentialPurpose::HealthCheck,
    CredentialPurpose::VertexAuthentication,
];
const AWS_PURPOSES: &[CredentialPurpose] = &[
    CredentialPurpose::Asr,
    CredentialPurpose::Llm,
    CredentialPurpose::ModelCatalog,
    CredentialPurpose::HealthCheck,
];

const OPENAI_AUDIENCES: &[CredentialAudiencePolicyDefinition] = &[
    CredentialAudiencePolicyDefinition::ExactSecureOrigin {
        origin: "https://api.openai.com",
        purposes: OPENAI_PURPOSES,
    },
    CredentialAudiencePolicyDefinition::ExactSecureOrigin {
        origin: "wss://api.openai.com",
        purposes: OPENAI_PURPOSES,
    },
];

macro_rules! exact_audiences {
    ($name:ident, $purposes:expr, $($origin:literal),+ $(,)?) => {
        const $name: &[CredentialAudiencePolicyDefinition] = &[
            $(CredentialAudiencePolicyDefinition::ExactSecureOrigin {
                origin: $origin,
                purposes: $purposes,
            }),+
        ];
    };
}

exact_audiences!(
    CEREBRAS_AUDIENCES,
    LLM_CATALOG_HEALTH,
    "https://api.cerebras.ai"
);
exact_audiences!(
    SAMBANOVA_AUDIENCES,
    LLM_CATALOG_HEALTH,
    "https://api.sambanova.ai"
);
exact_audiences!(
    OPENROUTER_AUDIENCES,
    LLM_CATALOG_HEALTH,
    "https://openrouter.ai"
);
exact_audiences!(GROQ_AUDIENCES, ASR_LLM, "https://api.groq.com");
exact_audiences!(TOGETHER_AUDIENCES, ASR_LLM, "https://api.together.xyz");
exact_audiences!(
    FIREWORKS_AUDIENCES,
    LLM_CATALOG_HEALTH,
    "https://api.fireworks.ai"
);
exact_audiences!(
    DEEPGRAM_AUDIENCES,
    ASR_TTS_HEALTH,
    "https://api.deepgram.com",
    "wss://api.deepgram.com"
);
exact_audiences!(
    ASSEMBLYAI_AUDIENCES,
    ASR_HEALTH,
    "https://api.assemblyai.com",
    "wss://streaming.assemblyai.com"
);
exact_audiences!(
    SONIOX_AUDIENCES,
    ASR_HEALTH,
    "https://api.soniox.com",
    "wss://stt-rt.soniox.com"
);
exact_audiences!(GLADIA_AUDIENCES, ASR_HEALTH, "https://api.gladia.io");
exact_audiences!(
    SPEECHMATICS_AUDIENCES,
    ASR_HEALTH,
    "wss://eu.rt.speechmatics.com",
    "wss://us.rt.speechmatics.com"
);
exact_audiences!(REVAI_AUDIENCES, ASR_HEALTH, "wss://api.rev.ai");
exact_audiences!(
    GEMINI_AUDIENCES,
    GEMINI_PURPOSES,
    "https://generativelanguage.googleapis.com",
    "wss://generativelanguage.googleapis.com"
);

const AWS_AUDIENCES: &[CredentialAudiencePolicyDefinition] =
    &[CredentialAudiencePolicyDefinition::AwsSdk {
        partitions: &[
            AwsPartition::Aws,
            AwsPartition::AwsCn,
            AwsPartition::AwsUsGov,
        ],
        services: &[
            AwsSdkService::TranscribeStreaming,
            AwsSdkService::BedrockRuntime,
            AwsSdkService::Sts,
        ],
        purposes: AWS_PURPOSES,
    }];

macro_rules! set_definition {
    ($id:expr, $methods:expr, $consumers:expr, $purposes:expr, $audiences:expr, $action:expr) => {
        set_definition!(
            $id,
            $methods,
            $consumers,
            $purposes,
            $audiences,
            CredentialSetCompleteness::AllRequiredSecretFields,
            $action
        )
    };
    ($id:expr, $methods:expr, $consumers:expr, $purposes:expr, $audiences:expr, $configured_when:expr, $action:expr) => {
        CredentialSetDefinition {
            id: $id,
            auth_method_ids: $methods,
            allowed_consumers: $consumers,
            purposes: $purposes,
            audience_policies: $audiences,
            configured_when: $configured_when,
            active_use_action: $action,
        }
    };
}

pub const CREDENTIAL_SET_DEFINITIONS: &[CredentialSetDefinition] = &[
    set_definition!(
        Set::Openai,
        &[Auth::ApiKey],
        &[
            "asr.api",
            "asr.openai_realtime",
            "llm.api",
            "realtime_agent.openai_realtime"
        ],
        OPENAI_PURPOSES,
        OPENAI_AUDIENCES,
        CredentialActiveUseAction::Reauthenticate
    ),
    set_definition!(
        Set::Cerebras,
        &[Auth::ApiKey],
        &["asr.api", "llm.api", "llm.cerebras"],
        LLM_CATALOG_HEALTH,
        CEREBRAS_AUDIENCES,
        CredentialActiveUseAction::RefreshBeforeNextUse
    ),
    set_definition!(
        Set::Sambanova,
        &[Auth::ApiKey],
        &["asr.api", "llm.api", "llm.sambanova"],
        LLM_CATALOG_HEALTH,
        SAMBANOVA_AUDIENCES,
        CredentialActiveUseAction::RefreshBeforeNextUse
    ),
    set_definition!(
        Set::Openrouter,
        &[Auth::ApiKey],
        &["asr.api", "llm.api", "llm.openrouter"],
        LLM_CATALOG_HEALTH,
        OPENROUTER_AUDIENCES,
        CredentialActiveUseAction::RefreshBeforeNextUse
    ),
    set_definition!(
        Set::Groq,
        &[Auth::ApiKey],
        &["asr.api", "llm.api"],
        ASR_LLM,
        GROQ_AUDIENCES,
        CredentialActiveUseAction::RefreshBeforeNextUse
    ),
    set_definition!(
        Set::Together,
        &[Auth::ApiKey],
        &["asr.api", "llm.api"],
        ASR_LLM,
        TOGETHER_AUDIENCES,
        CredentialActiveUseAction::RefreshBeforeNextUse
    ),
    set_definition!(
        Set::Fireworks,
        &[Auth::ApiKey],
        &["asr.api", "llm.api"],
        LLM_CATALOG_HEALTH,
        FIREWORKS_AUDIENCES,
        CredentialActiveUseAction::RefreshBeforeNextUse
    ),
    set_definition!(
        Set::Deepgram,
        &[Auth::ApiKey],
        &["asr.deepgram", "tts.deepgram_aura"],
        ASR_TTS_HEALTH,
        DEEPGRAM_AUDIENCES,
        CredentialActiveUseAction::Reauthenticate
    ),
    set_definition!(
        Set::Assemblyai,
        &[Auth::ApiKey],
        &["asr.assemblyai"],
        ASR_HEALTH,
        ASSEMBLYAI_AUDIENCES,
        CredentialActiveUseAction::Reauthenticate
    ),
    set_definition!(
        Set::Soniox,
        &[Auth::ApiKey],
        &["asr.soniox"],
        ASR_HEALTH,
        SONIOX_AUDIENCES,
        CredentialActiveUseAction::Reauthenticate
    ),
    set_definition!(
        Set::Gladia,
        &[Auth::ApiKey],
        &["asr.gladia"],
        ASR_HEALTH,
        GLADIA_AUDIENCES,
        CredentialActiveUseAction::Reauthenticate
    ),
    set_definition!(
        Set::Speechmatics,
        &[Auth::ApiKey],
        &["asr.speechmatics"],
        ASR_HEALTH,
        SPEECHMATICS_AUDIENCES,
        CredentialActiveUseAction::Reauthenticate
    ),
    set_definition!(
        Set::Elevenlabs,
        &[Auth::ApiKey],
        &["asr.elevenlabs_scribe"],
        ASR_HEALTH,
        &[],
        CredentialActiveUseAction::Reauthenticate
    ),
    set_definition!(
        Set::Revai,
        &[Auth::ApiKey],
        &["asr.revai"],
        ASR_HEALTH,
        REVAI_AUDIENCES,
        CredentialActiveUseAction::Reauthenticate
    ),
    set_definition!(
        Set::AzureSpeech,
        &[Auth::ApiKey],
        &["asr.azure_speech"],
        ASR_HEALTH,
        &[],
        CredentialActiveUseAction::Reauthenticate
    ),
    set_definition!(
        Set::Gemini,
        &[Auth::ApiKey, Auth::GoogleServiceAccountFile],
        &["llm.api", "realtime_agent.gemini_live"],
        GEMINI_PURPOSES,
        GEMINI_AUDIENCES,
        CredentialSetCompleteness::AnyStoredSecretAlternative {
            group_id: "gemini.authentication"
        },
        CredentialActiveUseAction::Reauthenticate
    ),
    set_definition!(
        Set::Aws,
        &[Auth::AwsStatic, Auth::AwsProfile, Auth::AwsDefaultChain],
        &["asr.aws_transcribe", "llm.aws_bedrock"],
        AWS_PURPOSES,
        AWS_AUDIENCES,
        CredentialSetCompleteness::RequiredTogether {
            group_id: "aws.static_pair"
        },
        CredentialActiveUseAction::Reauthenticate
    ),
];

pub fn credential_set_definition(
    id: BuiltInCredentialSetId,
) -> Option<&'static CredentialSetDefinition> {
    CREDENTIAL_SET_DEFINITIONS.iter().find(|set| set.id == id)
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

macro_rules! string_token {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
    };
}

string_token!(CredentialRevision);
string_token!(CredentialOperationId);
string_token!(CredentialIdempotencyToken);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialBackendKind {
    Native,
    FileV2,
    InMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialBackendAvailability {
    Unknown,
    Available,
    Locked,
    AccessDenied,
    Unavailable,
    Unsupported,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMigrationState {
    Uninitialized,
    NotRequired,
    InventoryRequired,
    Ready,
    InProgress,
    Conflict,
    Completed,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialCleanupState {
    NotApplicable,
    Pending,
    InProgress,
    Completed,
    Blocked,
}

/// Redaction-safe authority source for a set. This never contains a native
/// account locator, filesystem path, or provider response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSetSource {
    None,
    NativeV2,
    FileV2,
    LegacyKeychain,
    LegacyYaml,
    LegacyInlineSettings,
    AmbientProviderChain,
    PrivateSettingsLocator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSetRecordState {
    Missing,
    Configured,
    Tombstoned,
    RecoveryRequired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSetRecoveryState {
    None,
    PendingIntent,
    RecordJournalMismatch,
    CommitUnknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialWorkerState {
    Idle,
    Busy,
    Stalled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialActivationStage {
    Staged,
    SettingsPending,
    CredentialPending,
    CleanupPending,
    RecoveryRequired,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialErrorCode {
    Missing,
    Locked,
    AccessDenied,
    Cancelled,
    StoreUnavailable,
    StoreUnsupported,
    CorruptRecord,
    UnsupportedSchema,
    PayloadTooLarge,
    AmbiguousMatch,
    Conflict,
    MigrationRequired,
    MigrationConflict,
    RecoveryRequired,
    LegacyCleanupRequired,
    PermissionHardeningFailed,
    InvalidCredentialSet,
    AudienceNotAllowed,
    InsecureTransport,
    RevisionConflict,
    OperationInProgress,
    StalledWorker,
    CommitUnknown,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialSafeRecoveryAction {
    None,
    Retry,
    UnlockStore,
    ReenterCredential,
    SelectMigrationSource,
    RunMigration,
    RunCleanup,
    Reconcile,
    RepairPermissions,
    ChooseSupportedBackend,
    RestartApplication,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialMutationResultCode {
    Created,
    Replaced,
    Tombstoned,
    AlreadyApplied,
    Recovered,
    NoChange,
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

const BACKEND_KINDS: &[CredentialBackendKind] = &[
    CredentialBackendKind::Native,
    CredentialBackendKind::FileV2,
    CredentialBackendKind::InMemory,
];
const BACKEND_AVAILABILITIES: &[CredentialBackendAvailability] = &[
    CredentialBackendAvailability::Unknown,
    CredentialBackendAvailability::Available,
    CredentialBackendAvailability::Locked,
    CredentialBackendAvailability::AccessDenied,
    CredentialBackendAvailability::Unavailable,
    CredentialBackendAvailability::Unsupported,
    CredentialBackendAvailability::RecoveryRequired,
];
const MIGRATION_STATES: &[CredentialMigrationState] = &[
    CredentialMigrationState::Uninitialized,
    CredentialMigrationState::NotRequired,
    CredentialMigrationState::InventoryRequired,
    CredentialMigrationState::Ready,
    CredentialMigrationState::InProgress,
    CredentialMigrationState::Conflict,
    CredentialMigrationState::Completed,
    CredentialMigrationState::RecoveryRequired,
];
const CLEANUP_STATES: &[CredentialCleanupState] = &[
    CredentialCleanupState::NotApplicable,
    CredentialCleanupState::Pending,
    CredentialCleanupState::InProgress,
    CredentialCleanupState::Completed,
    CredentialCleanupState::Blocked,
];
const SET_SOURCES: &[CredentialSetSource] = &[
    CredentialSetSource::None,
    CredentialSetSource::NativeV2,
    CredentialSetSource::FileV2,
    CredentialSetSource::LegacyKeychain,
    CredentialSetSource::LegacyYaml,
    CredentialSetSource::LegacyInlineSettings,
    CredentialSetSource::AmbientProviderChain,
    CredentialSetSource::PrivateSettingsLocator,
];
const SET_RECORD_STATES: &[CredentialSetRecordState] = &[
    CredentialSetRecordState::Missing,
    CredentialSetRecordState::Configured,
    CredentialSetRecordState::Tombstoned,
    CredentialSetRecordState::RecoveryRequired,
];
const SET_RECOVERY_STATES: &[CredentialSetRecoveryState] = &[
    CredentialSetRecoveryState::None,
    CredentialSetRecoveryState::PendingIntent,
    CredentialSetRecoveryState::RecordJournalMismatch,
    CredentialSetRecoveryState::CommitUnknown,
];
const WORKER_STATES: &[CredentialWorkerState] = &[
    CredentialWorkerState::Idle,
    CredentialWorkerState::Busy,
    CredentialWorkerState::Stalled,
];
const ACTIVATION_STAGES: &[CredentialActivationStage] = &[
    CredentialActivationStage::Staged,
    CredentialActivationStage::SettingsPending,
    CredentialActivationStage::CredentialPending,
    CredentialActivationStage::CleanupPending,
    CredentialActivationStage::RecoveryRequired,
];
const ACTIVE_USE_ACTIONS: &[CredentialActiveUseAction] = &[
    CredentialActiveUseAction::None,
    CredentialActiveUseAction::RefreshBeforeNextUse,
    CredentialActiveUseAction::Reauthenticate,
    CredentialActiveUseAction::Stop,
    CredentialActiveUseAction::RestartApplication,
];
const ERROR_CODES: &[CredentialErrorCode] = &[
    CredentialErrorCode::Missing,
    CredentialErrorCode::Locked,
    CredentialErrorCode::AccessDenied,
    CredentialErrorCode::Cancelled,
    CredentialErrorCode::StoreUnavailable,
    CredentialErrorCode::StoreUnsupported,
    CredentialErrorCode::CorruptRecord,
    CredentialErrorCode::UnsupportedSchema,
    CredentialErrorCode::PayloadTooLarge,
    CredentialErrorCode::AmbiguousMatch,
    CredentialErrorCode::Conflict,
    CredentialErrorCode::MigrationRequired,
    CredentialErrorCode::MigrationConflict,
    CredentialErrorCode::RecoveryRequired,
    CredentialErrorCode::LegacyCleanupRequired,
    CredentialErrorCode::PermissionHardeningFailed,
    CredentialErrorCode::InvalidCredentialSet,
    CredentialErrorCode::AudienceNotAllowed,
    CredentialErrorCode::InsecureTransport,
    CredentialErrorCode::RevisionConflict,
    CredentialErrorCode::OperationInProgress,
    CredentialErrorCode::StalledWorker,
    CredentialErrorCode::CommitUnknown,
    CredentialErrorCode::Internal,
];
const RECOVERY_ACTIONS: &[CredentialSafeRecoveryAction] = &[
    CredentialSafeRecoveryAction::None,
    CredentialSafeRecoveryAction::Retry,
    CredentialSafeRecoveryAction::UnlockStore,
    CredentialSafeRecoveryAction::ReenterCredential,
    CredentialSafeRecoveryAction::SelectMigrationSource,
    CredentialSafeRecoveryAction::RunMigration,
    CredentialSafeRecoveryAction::RunCleanup,
    CredentialSafeRecoveryAction::Reconcile,
    CredentialSafeRecoveryAction::RepairPermissions,
    CredentialSafeRecoveryAction::ChooseSupportedBackend,
    CredentialSafeRecoveryAction::RestartApplication,
];
const MUTATION_RESULT_CODES: &[CredentialMutationResultCode] = &[
    CredentialMutationResultCode::Created,
    CredentialMutationResultCode::Replaced,
    CredentialMutationResultCode::Tombstoned,
    CredentialMutationResultCode::AlreadyApplied,
    CredentialMutationResultCode::Recovered,
    CredentialMutationResultCode::NoChange,
];

#[derive(Debug, Serialize)]
pub struct CredentialContractVocabulary {
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
    custom_set_policy: CUSTOM_CREDENTIAL_SET_POLICY,
    vocabulary: CredentialContractVocabulary {
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
  (typeof CREDENTIAL_CONTRACT.fields)[number]["class"];
export type LegacyFieldDisposition =
  (typeof CREDENTIAL_CONTRACT.fields)[number]["legacy_disposition"];
export type CredentialFieldDefinition =
  (typeof CREDENTIAL_CONTRACT.fields)[number];
export type CredentialSetDefinition = (typeof CREDENTIAL_CONTRACT.sets)[number];
export type CredentialSetCompleteness =
  (typeof CREDENTIAL_CONTRACT.sets)[number]["configured_when"];

export type CredentialRevision = string;
export type CredentialOperationId = string;
export type CredentialIdempotencyToken = string;
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
export type CredentialErrorCode =
  (typeof CREDENTIAL_CONTRACT.vocabulary.error_codes)[number];
export type CredentialSafeRecoveryAction =
  (typeof CREDENTIAL_CONTRACT.vocabulary.recovery_actions)[number];
export type CredentialMutationResultCode =
  (typeof CREDENTIAL_CONTRACT.vocabulary.mutation_result_codes)[number];

export type SecureTransportScheme = "https" | "wss";
export type AwsPartition = "aws" | "aws_cn" | "aws_us_gov";
export type AwsSdkService = "transcribe_streaming" | "bedrock_runtime" | "sts";
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
            let set = credential_set_definition(field.set_id)
                .unwrap_or_else(|| panic!("missing set for {}", field.legacy_key));
            assert!(!field.auth_method_ids.is_empty());
            for auth_method in field.auth_method_ids {
                assert!(AUTH_METHOD_IDS.contains(auth_method));
                assert!(set.auth_method_ids.contains(auth_method));
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
        for set in CREDENTIAL_SET_DEFINITIONS {
            for policy in set.audience_policies {
                let CredentialAudiencePolicyDefinition::ExactSecureOrigin { origin, .. } = policy
                else {
                    continue;
                };
                let parsed = url::Url::parse(origin).unwrap_or_else(|error| {
                    panic!(
                        "set {} has invalid origin {origin}: {error}",
                        set.id.as_str()
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
                revision: Some(CredentialRevision("opaque-revision".into())),
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
            operation_id: CredentialOperationId("operation".into()),
            idempotency_token: CredentialIdempotencyToken("idempotency".into()),
            set_id,
            previous_revision: None,
            new_revision: Some(CredentialRevision("new-revision".into())),
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
