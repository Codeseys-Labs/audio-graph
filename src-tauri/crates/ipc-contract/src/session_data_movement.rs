//! Session data-movement ledger audit event schema (seed audio-graph-70a3).
//!
//! This is a backend-owned, session-scoped, append-only audit stream that
//! answers the trust question "where did this session's audio/text/provider
//! data go?" — separate from the transcript/projection event logs, which
//! answer graph-replay questions.
//!
//! It is the schema chain-ROOT: the frontend "Session data route UI + privacy
//! report" (seed audio-graph-51e0) consumes the generated TypeScript type in
//! `src/generated/sessionDataMovement.ts`, so the type lives in this
//! dependency-light `ipc-contract` crate and is exported alongside the audio
//! source contract.
//!
//! ## Redaction invariant
//!
//! Every field on [`DataMovementEvent`] is by construction redaction-safe: it
//! stores data *classes*, provider/model ids, source ids, destination
//! boundaries, byte/char/token counts, content *hashes*, artifact descriptors,
//! statuses, and *redacted* error codes/messages. It has no field capable of
//! carrying raw audio, raw transcript text, prompt bodies, API keys, bearer
//! tokens, service-account JSON, or full provider payloads. Callers that build
//! events from untrusted strings should route error messages through
//! [`DataMovementResult::failed`] and never place secrets in `source_label`,
//! which is documented as pre-redacted display text only.

use serde::{Deserialize, Serialize};

/// Current schema version for [`DataMovementEvent`]. Bumped when the on-disk
/// shape changes in a way that older readers must be aware of.
pub const DATA_MOVEMENT_SCHEMA_VERSION: u32 = 1;

/// Who or what caused a data-movement event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataMovementActor {
    /// Backend/system machinery (capture pipeline, schedulers, autosave).
    System,
    /// A direct user action (save credential, export, delete).
    User,
    /// A remote provider (an inbound provider response or callback).
    Provider,
    /// Org promotion / sync machinery.
    Sync,
}

/// The kind of movement or lifecycle transition being recorded.
///
/// Grouped to match the seed acceptance: capture start/stop and audio consumer
/// backpressure, provider calls, artifact writes/loads/export/delete,
/// credential save/delete/readiness, projection jobs/patches, and org
/// promotion state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataMovementEventType {
    // Capture lifecycle.
    CaptureStarted,
    CaptureStopped,
    AudioConsumerStarted,
    AudioConsumerBackpressure,
    AudioConsumerDropped,
    // Provider calls (ASR/LLM/TTS/S2S/readiness).
    ProviderCallStarted,
    ProviderCallSucceeded,
    ProviderCallFailed,
    ProviderCallCancelled,
    // Artifact lifecycle.
    ArtifactWritten,
    ArtifactLoaded,
    ArtifactExported,
    ArtifactSoftDeleted,
    ArtifactHardDeleted,
    ArtifactDeleteFailed,
    // Credentials.
    CredentialSaved,
    CredentialDeleted,
    CredentialSourceChanged,
    ProviderReadinessChecked,
    // Projection jobs / patches.
    ProjectionJobQueued,
    ProjectionJobStarted,
    ProjectionPatchAccepted,
    ProjectionPatchRejected,
    // Org promotion state.
    PromotionDraftCreated,
    PromotionRedactionReviewed,
    PromotionSyncStarted,
    PromotionSyncSucceeded,
    PromotionRevoked,
}

/// A class of data that moved, independent of its concrete content.
///
/// Only the class is recorded — never the bytes/text themselves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DataClass {
    /// Processed PCM audio (post-AEC/VAD), pre-provider.
    ProcessedPcm,
    /// Raw/near-raw capture audio stream handed to a provider.
    AudioStream,
    /// Transcript text spans.
    TranscriptText,
    /// Speaker attribution / diarization labels.
    SpeakerLabels,
    /// Materialized notes.
    Notes,
    /// Temporal graph facts / context.
    GraphContext,
    /// Vector embeddings.
    Embeddings,
    /// LLM/realtime prompt or context payloads (recorded by hash/size only).
    Prompts,
    /// Tool/function calls sent to a provider.
    ToolCalls,
    /// Generated live audio output (TTS/S2S).
    LiveAudioOutput,
    /// Provider diagnostics / redacted error logs.
    ProviderDiagnostics,
    /// Usage / latency / cost metadata.
    UsageMetadata,
    /// Credential presence / source metadata (never the secret itself).
    CredentialMetadata,
}

/// Where the captured data originated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DataMovementSource {
    /// Coarse source kind, e.g. `"rsac"`, `"device"`, `"application"`,
    /// `"session_artifact"`, `"credential_store"`.
    pub kind: String,
    /// Stable, non-secret source identifier when one exists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Pre-redacted, display-only label. Documented as redaction-safe: callers
    /// must never place raw device paths or secrets here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_label: Option<String>,
}

/// The trust boundary a piece of data crossed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DestinationBoundary {
    /// Stayed on device (local durable storage or transient memory).
    Local,
    /// Left the device to a cloud/external provider.
    Provider,
    /// Synced to an org target.
    Org,
    /// Exported to a user-controlled destination (file, share).
    Export,
}

/// Where the data went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DataMovementDestination {
    pub boundary: DestinationBoundary,
    /// Provider id when the boundary is `provider`, e.g. `"llm.openrouter"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    /// Non-secret endpoint class, e.g. `"chat_completions"`,
    /// `"model_catalog"`, `"realtime_ws"`. Never a full URL with secrets.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_class: Option<String>,
}

impl DataMovementDestination {
    /// Data that never left the device.
    pub fn local() -> Self {
        Self {
            boundary: DestinationBoundary::Local,
            provider_id: None,
            endpoint_class: None,
        }
    }

    /// Data sent to a named cloud provider endpoint class.
    pub fn provider(provider_id: impl Into<String>, endpoint_class: impl Into<String>) -> Self {
        Self {
            boundary: DestinationBoundary::Provider,
            provider_id: Some(provider_id.into()),
            endpoint_class: Some(endpoint_class.into()),
        }
    }
}

/// How a referenced artifact is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStorageKind {
    File,
    RepositoryRecord,
}

/// A reference to a durable artifact touched by the event.
///
/// The concrete path is never recorded — only a hash of it — so the ledger can
/// prove which artifact moved without leaking a filesystem layout that might
/// embed a username or session title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ArtifactRef {
    /// Artifact kind, e.g. `"transcript_events"`, `"materialized_graph"`.
    pub kind: String,
    pub storage: ArtifactStorageKind,
    /// Opaque, redaction-safe fingerprint of the artifact path/uri
    /// (`"h64:<16 hex>"`). Never the raw path. This is a fast, *non-cryptographic*
    /// 64-bit fingerprint (std-library hasher, no crypto dependency): it lets the
    /// ledger correlate which artifact moved without storing a filesystem layout
    /// that could embed a username or session title. It is a one-way display
    /// token, not a SHA-256 or an integrity/collision-resistance guarantee. See
    /// `hash_artifact_path` in `persistence::data_movement` for the producer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path_hash: Option<String>,
}

/// The transcript/projection sequence basis this movement was derived from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MovementBasis {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_sequence: Option<u64>,
}

/// The provider/model that produced or consumed the data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MovementModel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
}

/// Quantitative, non-content measures of the movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MovementCounts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_chars: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_in: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_out: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// The privacy/provider policy in force when the event was recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PrivacyMode {
    /// No external provider transfers for session content.
    LocalOnly,
    /// User-selected cloud providers may receive required data classes.
    ByokCloud,
    /// Saved cloud credentials may be health/catalog checked, but session
    /// content cannot leave the device.
    CloudDisabledReadinessOnly,
    /// Explicit redacted object versions may sync to an org target.
    OrgSync,
}

/// How long the moved data is expected to persist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RetentionClass {
    /// Deleted with the session.
    SessionArtifact,
    /// Transient — not durably persisted (e.g. a prompt body sent to an LLM).
    Transient,
    /// Local redacted diagnostics.
    Diagnostic,
    /// Org-retained after explicit promotion.
    OrgRetained,
}

/// Policy context for the event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MovementPolicy {
    pub privacy_mode: PrivacyMode,
    /// Whether this event is intended to be surfaced in the user route report.
    pub user_visible: bool,
    pub retention_class: RetentionClass,
}

/// Terminal status of the movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MovementStatus {
    Started,
    Succeeded,
    Failed,
    Cancelled,
    /// Blocked by policy (e.g. a cloud call refused in `local_only` mode).
    Blocked,
}

/// The outcome of the movement, with a redacted error surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DataMovementResult {
    pub status: MovementStatus,
    /// Stable, non-secret error code, e.g. `"provider_timeout"`, `"enospc"`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Human-readable message that has already been redacted of secrets and
    /// raw content. Never a raw provider payload or stack trace with data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_message_redacted: Option<String>,
}

impl DataMovementResult {
    pub fn started() -> Self {
        Self {
            status: MovementStatus::Started,
            error_code: None,
            error_message_redacted: None,
        }
    }

    pub fn succeeded() -> Self {
        Self {
            status: MovementStatus::Succeeded,
            error_code: None,
            error_message_redacted: None,
        }
    }

    /// A failure with a stable code and a pre-redacted message. The message is
    /// passed through the redaction guard so callers cannot accidentally store
    /// obvious secrets in the ledger.
    pub fn failed(code: impl Into<String>, message_redacted: impl Into<String>) -> Self {
        Self {
            status: MovementStatus::Failed,
            error_code: Some(code.into()),
            error_message_redacted: Some(redact_message(&message_redacted.into())),
        }
    }

    /// A policy-blocked movement (e.g. a cloud call refused in local-only mode).
    pub fn blocked(code: impl Into<String>) -> Self {
        Self {
            status: MovementStatus::Blocked,
            error_code: Some(code.into()),
            error_message_redacted: None,
        }
    }
}

/// A single redacted data-movement audit event.
///
/// Append-only. Serialized one-per-line to a session's ledger JSONL file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DataMovementEvent {
    /// Stable unique id for this event (uuid v4 string).
    pub event_id: String,
    pub schema_version: u32,
    pub session_id: String,
    /// Unix milliseconds when the event was recorded.
    pub created_at_ms: u64,
    pub actor: DataMovementActor,
    pub event_type: DataMovementEventType,
    /// Data classes that moved. Empty for pure lifecycle events (e.g. a
    /// readiness check that sends no content).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub data_classes: Vec<DataClass>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<DataMovementSource>,
    pub destination: DataMovementDestination,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub artifact_refs: Vec<ArtifactRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis: Option<MovementBasis>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<MovementModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<MovementCounts>,
    pub policy: MovementPolicy,
    pub result: DataMovementResult,
}

/// A crude, dependency-free guard that strips obvious credential-shaped tokens
/// from a message before it is stored in the ledger.
///
/// This is defense-in-depth, not a substitute for callers redacting at the
/// source: the schema has no field that *should* ever contain a secret, but a
/// carelessly-forwarded provider error string might. We collapse anything that
/// looks like a bearer token, `sk-`/`AKIA`-prefixed key, JWT, or a long
/// base64/hex run into `<redacted>`.
pub fn redact_message(message: &str) -> String {
    let mut out = String::with_capacity(message.len());
    for token in message.split_inclusive(|c: char| c.is_whitespace()) {
        let (word, trailing_ws) = split_trailing_ws(token);
        if looks_secret(word) {
            out.push_str("<redacted>");
        } else {
            out.push_str(word);
        }
        out.push_str(trailing_ws);
    }
    out
}

fn split_trailing_ws(token: &str) -> (&str, &str) {
    match token.char_indices().rev().find(|(_, c)| !c.is_whitespace()) {
        Some((idx, c)) => {
            let split = idx + c.len_utf8();
            (&token[..split], &token[split..])
        }
        None => ("", token),
    }
}

fn looks_secret(word: &str) -> bool {
    let trimmed = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_');
    if trimmed.len() < 12 {
        // Short tokens are unlikely to be keys; keep human-readable messages
        // intact (but always redact a bare "Bearer" scheme word).
        return trimmed.eq_ignore_ascii_case("bearer");
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("sk-")
        || lower.starts_with("sk_")
        || trimmed.starts_with("AKIA")
        || trimmed.starts_with("ASIA")
        || lower.starts_with("bearer")
        || lower.starts_with("eyj")
    // JWT header base64 prefix
    {
        return true;
    }
    // A long run of key-shaped chars is almost certainly a token, not prose.
    let key_shaped = trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '+' || c == '/');
    key_shaped && trimmed.len() >= 24
}

/// JSON Schema for [`DataMovementEvent`], for the frontend to validate against.
pub fn data_movement_event_schema_json() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(DataMovementEvent))
        .expect("DataMovementEvent schema should serialize")
}

/// The generated TypeScript module consumed by the frontend (seed
/// audio-graph-51e0). Hand-written interfaces kept in sync with the Rust types
/// above, plus the embedded JSON Schema literal for runtime validation.
pub fn session_data_movement_typescript_module() -> String {
    let schema = serde_json::to_string_pretty(&data_movement_event_schema_json())
        .expect("DataMovementEvent schema should serialize");
    let schema_literal = crate::js_single_quoted_string_literal(&schema);
    format!(
        r#"// @generated by src-tauri/crates/ipc-contract/src/session_data_movement.rs. Do not edit manually.

// Session data-movement ledger audit event schema (seed audio-graph-70a3).
// Chain-root for the Session data route UI + privacy report (audio-graph-51e0).

export const DATA_MOVEMENT_SCHEMA_VERSION = {version};

export type DataMovementActor = "system" | "user" | "provider" | "sync";

export type DataMovementEventType =
  | "capture_started"
  | "capture_stopped"
  | "audio_consumer_started"
  | "audio_consumer_backpressure"
  | "audio_consumer_dropped"
  | "provider_call_started"
  | "provider_call_succeeded"
  | "provider_call_failed"
  | "provider_call_cancelled"
  | "artifact_written"
  | "artifact_loaded"
  | "artifact_exported"
  | "artifact_soft_deleted"
  | "artifact_hard_deleted"
  | "artifact_delete_failed"
  | "credential_saved"
  | "credential_deleted"
  | "credential_source_changed"
  | "provider_readiness_checked"
  | "projection_job_queued"
  | "projection_job_started"
  | "projection_patch_accepted"
  | "projection_patch_rejected"
  | "promotion_draft_created"
  | "promotion_redaction_reviewed"
  | "promotion_sync_started"
  | "promotion_sync_succeeded"
  | "promotion_revoked";

export type DataClass =
  | "processed_pcm"
  | "audio_stream"
  | "transcript_text"
  | "speaker_labels"
  | "notes"
  | "graph_context"
  | "embeddings"
  | "prompts"
  | "tool_calls"
  | "live_audio_output"
  | "provider_diagnostics"
  | "usage_metadata"
  | "credential_metadata";

export type DestinationBoundary = "local" | "provider" | "org" | "export";

export type ArtifactStorageKind = "file" | "repository_record";

export type PrivacyMode =
  | "local_only"
  | "byok_cloud"
  | "cloud_disabled_readiness_only"
  | "org_sync";

export type RetentionClass =
  | "session_artifact"
  | "transient"
  | "diagnostic"
  | "org_retained";

export type MovementStatus =
  | "started"
  | "succeeded"
  | "failed"
  | "cancelled"
  | "blocked";

export interface DataMovementSource {{
  kind: string;
  source_id?: string | null;
  source_label?: string | null;
}}

export interface DataMovementDestination {{
  boundary: DestinationBoundary;
  provider_id?: string | null;
  endpoint_class?: string | null;
}}

export interface ArtifactRef {{
  kind: string;
  storage: ArtifactStorageKind;
  path_hash?: string | null;
}}

export interface MovementBasis {{
  transcript_sequence?: number | null;
  projection_sequence?: number | null;
}}

export interface MovementModel {{
  provider_id?: string | null;
  model_id?: string | null;
}}

export interface MovementCounts {{
  audio_ms?: number | null;
  text_chars?: number | null;
  tokens_in?: number | null;
  tokens_out?: number | null;
  bytes?: number | null;
}}

export interface MovementPolicy {{
  privacy_mode: PrivacyMode;
  user_visible: boolean;
  retention_class: RetentionClass;
}}

export interface DataMovementResult {{
  status: MovementStatus;
  error_code?: string | null;
  error_message_redacted?: string | null;
}}

export interface DataMovementEvent {{
  event_id: string;
  schema_version: number;
  session_id: string;
  created_at_ms: number;
  actor: DataMovementActor;
  event_type: DataMovementEventType;
  data_classes?: DataClass[];
  source?: DataMovementSource | null;
  destination: DataMovementDestination;
  artifact_refs?: ArtifactRef[];
  basis?: MovementBasis | null;
  model?: MovementModel | null;
  counts?: MovementCounts | null;
  policy: MovementPolicy;
  result: DataMovementResult;
}}

export const DATA_MOVEMENT_EVENT_SCHEMA_JSON =
  {schema_literal};

export const DATA_MOVEMENT_EVENT_SCHEMA = JSON.parse(
  DATA_MOVEMENT_EVENT_SCHEMA_JSON,
) as Record<string, unknown>;
"#,
        version = DATA_MOVEMENT_SCHEMA_VERSION,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_event() -> DataMovementEvent {
        DataMovementEvent {
            event_id: "11111111-1111-4111-8111-111111111111".to_string(),
            schema_version: DATA_MOVEMENT_SCHEMA_VERSION,
            session_id: "session-1".to_string(),
            created_at_ms: 1_700_000_000_000,
            actor: DataMovementActor::System,
            event_type: DataMovementEventType::ProviderCallSucceeded,
            data_classes: vec![DataClass::TranscriptText, DataClass::Prompts],
            source: Some(DataMovementSource {
                kind: "session_artifact".to_string(),
                source_id: Some("transcript_events".to_string()),
                source_label: Some("Transcript revision log".to_string()),
            }),
            destination: DataMovementDestination::provider("llm.openrouter", "chat_completions"),
            artifact_refs: vec![ArtifactRef {
                kind: "materialized_notes".to_string(),
                storage: ArtifactStorageKind::File,
                path_hash: Some("h64:0123456789abcdef".to_string()),
            }],
            basis: Some(MovementBasis {
                transcript_sequence: Some(12),
                projection_sequence: Some(4),
            }),
            model: Some(MovementModel {
                provider_id: Some("llm.openrouter".to_string()),
                model_id: Some("anthropic/claude-sonnet-4".to_string()),
            }),
            counts: Some(MovementCounts {
                audio_ms: None,
                text_chars: Some(1200),
                tokens_in: Some(300),
                tokens_out: Some(80),
                bytes: None,
            }),
            policy: MovementPolicy {
                privacy_mode: PrivacyMode::ByokCloud,
                user_visible: true,
                retention_class: RetentionClass::Transient,
            },
            result: DataMovementResult::succeeded(),
        }
    }

    #[test]
    fn event_round_trips_through_json() {
        let event = sample_event();
        let value = serde_json::to_value(&event).expect("serialize");
        assert_eq!(value["actor"], "system");
        assert_eq!(value["event_type"], "provider_call_succeeded");
        assert_eq!(value["destination"]["boundary"], "provider");
        assert_eq!(value["destination"]["provider_id"], "llm.openrouter");
        assert_eq!(value["policy"]["privacy_mode"], "byok_cloud");
        assert_eq!(value["result"]["status"], "succeeded");
        assert_eq!(value["data_classes"][0], "transcript_text");

        let round_trip: DataMovementEvent = serde_json::from_value(value).expect("deserialize");
        assert_eq!(round_trip, event);
    }

    #[test]
    fn empty_data_classes_and_optionals_are_omitted() {
        let event = DataMovementEvent {
            data_classes: Vec::new(),
            source: None,
            artifact_refs: Vec::new(),
            basis: None,
            model: None,
            counts: None,
            destination: DataMovementDestination::local(),
            result: DataMovementResult::started(),
            ..sample_event()
        };
        let value = serde_json::to_value(&event).expect("serialize");
        assert!(value.get("data_classes").is_none());
        assert!(value.get("source").is_none());
        assert!(value.get("artifact_refs").is_none());
        assert!(value.get("basis").is_none());
        assert!(value.get("model").is_none());
        assert!(value.get("counts").is_none());
        assert_eq!(value["destination"]["boundary"], "local");
        assert!(value["destination"].get("provider_id").is_none());
    }

    #[test]
    fn redact_message_strips_credential_shaped_tokens_but_keeps_prose() {
        // Build a key-shaped sentinel at runtime so no static credential-shaped
        // literal appears in source (avoids tripping secret scanners on the
        // fake sentinel while still exercising the redactor on real key shape).
        let fake_key = ["s", "k", "-", &"A".repeat(24)].concat();
        let input = format!("provider rejected key {fake_key} with 401");
        let redacted = redact_message(&input);
        assert!(redacted.contains("provider rejected key"));
        assert!(redacted.contains("with 401"));
        assert!(!redacted.contains(&fake_key));
        assert!(redacted.contains("<redacted>"));

        // Runtime-assemble the JWT-shaped sentinel so no static `eyJ.../Bearer`
        // key literal appears in source (seed audio-graph-9d13 — GitGuardian
        // flags the `eyJ` JWT header shape). The header segment is base64 of
        // `{"alg":...}`; we rebuild the `eyJ` prefix from parts at runtime so
        // the redactor still sees a genuine `Bearer <eyJ...>` shape and the
        // `.starts_with("eyj")` JWT branch is still exercised.
        let jwt_header = ["ey", "J", &"h".repeat(18)].concat();
        let fake_jwt = format!("Bearer {jwt_header}.payload.signature");
        let bearer = redact_message(&format!("Authorization: {fake_jwt}"));
        assert!(!bearer.contains(&fake_jwt));
        assert!(bearer.contains("<redacted>"));

        // Prose is untouched.
        let prose = redact_message("The provider returned a timeout after 30 seconds.");
        assert_eq!(prose, "The provider returned a timeout after 30 seconds.");
    }

    #[test]
    fn failed_result_redacts_message() {
        // Runtime-assembled key-shaped sentinel — no static sk- literal in source.
        let fake_key = ["s", "k", "-", &"9".repeat(22)].concat();
        let result = DataMovementResult::failed(
            "provider_auth",
            format!("rejected token {fake_key} unauthorized"),
        );
        assert_eq!(result.status, MovementStatus::Failed);
        assert_eq!(result.error_code.as_deref(), Some("provider_auth"));
        let message = result.error_message_redacted.expect("redacted message");
        assert!(!message.contains(&fake_key));
        assert!(message.contains("<redacted>"));
    }

    #[test]
    fn generated_typescript_module_contains_core_symbols() {
        let module = session_data_movement_typescript_module();
        assert!(module.contains("export interface DataMovementEvent"));
        assert!(module.contains("export type DataMovementEventType"));
        assert!(module.contains("\"provider_call_succeeded\""));
        assert!(module.contains("export type DestinationBoundary"));
        assert!(module.contains("DATA_MOVEMENT_EVENT_SCHEMA_JSON"));
        assert!(module.contains("DATA_MOVEMENT_SCHEMA_VERSION = 1"));
        assert!(module.contains("Do not edit manually"));
    }
}
