//! Gemini Live API WebSocket client.
//!
//! Connects to the Gemini BidiGenerateContent streaming endpoint and exchanges
//! real-time audio (PCM → base64) for transcription + model text responses.
//!
//! # Protocol overview
//!
//! 1. Open WSS connection with API key in header (or Vertex bearer token).
//! 2. Send `BidiGenerateContentSetup` (model, generation config, system instruction).
//! 3. Wait for `setupComplete` server message.
//! 4. Stream audio as `realtimeInput.audio` (base64-encoded 16-bit LE PCM, 16 kHz mono).
//! 5. Receive `serverContent` messages containing:
//!    - `inputTranscription`  — what the user said
//!    - `modelTurn.parts[].text` — model reasoning / responses
//!    - `turnComplete` — end of a model turn
//!    - `goAway` — server requesting graceful shutdown
//! 6. Send `audioStreamEnd` to signal end of user input, then close.
//!
//! # Threading model
//!
//! The public API is **synchronous** (called from `std::thread` workers in
//! `commands.rs`). Internally, a dedicated tokio runtime drives the WebSocket.
//! Audio is forwarded from the caller's thread to the async writer via an
//! unbounded `tokio::sync::mpsc` channel, and events flow back through a
//! `crossbeam_channel` that the command layer already expects.
//!
//! # Auto-reconnect + session resumption
//!
//! The session is wrapped in a `session_task` that runs the reader + writer
//! concurrently via `tokio::select!` and, on any network-layer disconnect or
//! server-initiated `goAway`/Close, automatically reconnects with exponential
//! backoff (1 s / 2 s / 5 s / 10 s, then gives up). Mirrors the pattern used
//! in [`crate::asr::deepgram`] and [`crate::asr::assemblyai`], with one extra
//! Gemini-specific step on each reconnect: `open_ws` re-runs the full setup
//! handshake (send `BidiGenerateContentSetup` → await `setupComplete`) before
//! returning the fresh reader/writer halves. `Reconnecting` and `Reconnected`
//! events are emitted so consumers (see `commands.rs`) can surface the state.
//!
//! Session resumption is wired up so reconnects preserve model context when
//! the server is able to resume: the initial setup requests resumption by
//! sending `sessionResumption: {}`, and the server periodically pushes
//! `sessionResumptionUpdate { newHandle, resumable }` frames. The latest
//! `newHandle` (only captured while `resumable == true`) is threaded into
//! the next reconnect's setup payload as `sessionResumption.handle`, so the
//! server restores the prior session state instead of starting fresh. If no
//! handle is available yet, or the server rejects it, the client falls back
//! to a brand-new session transparently.

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use gcp_auth::TokenProvider;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::{self, Message};

// Shared reconnect ladder (review n2) — the 1/2/5/10s schedule now lives in one
// place for every transport.
use crate::reconnect::backoff_for_attempt;

// Closed, content-free runtime diagnostics (issue #145). Every error this
// module logs or emits carries one of these typed envelopes instead of raw
// provider / transport text.
use audio_graph_ipc_contract::runtime_diagnostic::{
    RuntimeDiagnostic, RuntimeDiagnosticContext, RuntimeErrorCode, RuntimeErrorDiagnostic,
    RuntimeOperation, RuntimeRetryDelayBucket, RuntimeSafeRecoveryAction, RuntimeStatusClass,
    RuntimeTransport,
};
// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Soft cap on the outbound audio queue (chunks). The audio channel buffers
/// during transient reconnects; an unbounded channel could grow without limit
/// if a reconnect stalls or the socket is persistently slow. At ~32 ms/chunk,
/// 1000 chunks ≈ 32 s of buffered audio — generous for reconnects, bounded for
/// memory. Beyond the cap, `send_audio` drops the newest chunk (and counts it).
///
/// OVERFLOW POLICY (deliberate; review m2) — this **lossy-drop-newest** soft cap
/// is the opposite of the ASR clients' 200-chunk **fail-fast** cap
/// (`asr::deepgram::AUDIO_BUFFER_MAX_CHUNKS`, which ends the session on
/// overflow). Gemini Live is an interactive S2S *conversation*: tearing the
/// session down on a transient buffer spike would drop the live dialog and lose
/// session-resumption continuity, which is worse for the user than shedding a
/// few tens of ms of the oldest-vs-newest audio. So Gemini keeps the session
/// alive and drops instead. The divergence is intentional, not copy-paste drift.
const GEMINI_AUDIO_QUEUE_CAP: usize = 1000;

/// Output audio sample rate for Gemini Live native audio. Always 24 kHz mono
/// PCM16 LE per the Live docs (research §1.3) — independent of the 16 kHz
/// *input* rate `send_audio` uses.
pub const GEMINI_OUTPUT_SAMPLE_RATE: u32 = 24_000;

/// Count of audio chunks dropped due to a full outbound queue (log throttle).
static GEMINI_AUDIO_DROPS: AtomicU64 = AtomicU64::new(0);

/// Coarse category for a Gemini-side failure.
///
/// Surfaced on every [`GeminiEvent::Error`] so the frontend can route to an
/// appropriate i18n key + toast severity without re-parsing error strings.
/// Every variant except `Unknown` corresponds to a *classified* failure the
/// backend has positively identified (close frame code + reason, or a
/// specific transport error). Issue #145 pairs every category with a closed
/// [`RuntimeDiagnostic`] — no raw provider or transport text is retained
/// anywhere on the event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum GeminiErrorCategory {
    /// Invalid / missing API key — reauthentication required.
    Auth,
    /// Credential expiry. No production path emits this today: the close
    /// reason is provider prose and the Vertex SDK offers no structural
    /// expiry signal, so it is retained only as part of the wire contract.
    /// Distinct from [`Self::Auth`] because the remediation differs.
    AuthExpired,
    /// Quota / rate-limit exceeded. `retry_delay` is the closed bucket a
    /// numeric `Retry-After` header reduces to (issue #145); the exact seconds
    /// are never retained. Absent when the server sent no usable hint.
    RateLimit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_delay: Option<RuntimeRetryDelayBucket>,
    },
    /// Server-side failure (5xx response or WS close code 1011).
    Server,
    /// Transport-layer failure — TLS, TCP, DNS, socket reset, etc. These are
    /// the ones our reconnect loop is expected to recover from.
    Network,
    /// Anything we could not positively classify.
    Unknown,
}

impl GeminiErrorCategory {
    /// Closed [`RuntimeDiagnostic`] envelope for this category (issue #145).
    /// Every branch discards the raw signal and keeps only closed values, so
    /// no provider text, close reason, status number, or exact retry-seconds
    /// survives onto logs, IPC events, or UI error state. `pub(crate)` so
    /// the converse layer's tests can build typed fixtures.
    pub(crate) fn runtime_diagnostic(&self) -> RuntimeDiagnostic {
        match self {
            Self::Auth => RuntimeDiagnostic::Runtime(RuntimeErrorDiagnostic::credential_rejected(
                realtime_context().with_status_class(RuntimeStatusClass::ClientError),
            )),
            Self::AuthExpired => {
                RuntimeDiagnostic::Runtime(RuntimeErrorDiagnostic::credential_expired(
                    realtime_context().with_status_class(RuntimeStatusClass::ClientError),
                ))
            }
            Self::RateLimit { retry_delay } => realtime_diagnostic(
                RuntimeErrorCode::RateLimited,
                true,
                RuntimeSafeRecoveryAction::RetryAfterDelay,
                Some(RuntimeStatusClass::ClientError),
                // Absence stays absent — never mapped to a bucket.
                *retry_delay,
            ),
            Self::Server => realtime_diagnostic(
                RuntimeErrorCode::ProviderUnavailable,
                true,
                RuntimeSafeRecoveryAction::Retry,
                Some(RuntimeStatusClass::ServerError),
                None,
            ),
            Self::Network => realtime_diagnostic(
                RuntimeErrorCode::NetworkUnreachable,
                true,
                RuntimeSafeRecoveryAction::CheckNetwork,
                None,
                None,
            ),
            Self::Unknown => realtime_diagnostic(
                RuntimeErrorCode::Internal,
                false,
                RuntimeSafeRecoveryAction::None,
                None,
                None,
            ),
        }
    }
}

/// Closed site identifier carried on [`GeminiEvent::Error::message`] (issue
/// #145). The serialized `message` string is *only* one of these stable
/// tokens — never provider text, a WebSocket close reason, a request id, or
/// an exact payload length. The structured classification lives in the
/// event's `diagnostic` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeminiErrorSite {
    /// Server sent a Close frame (classified close code).
    ServerClose,
    /// A server JSON frame failed to parse.
    InvalidJson,
    /// Server pushed `goAway` (graceful shutdown notice).
    GoAway,
    /// The reconnect budget (1/2/5/10s ladder) is exhausted.
    ReconnectExhausted,
    /// A single reconnect attempt failed (session keeps retrying).
    ReconnectFailed,
}

impl GeminiErrorSite {
    /// Stable snake_case token for the serialized event `message` field.
    pub const fn token(self) -> &'static str {
        match self {
            Self::ServerClose => "server_close",
            Self::InvalidJson => "invalid_json",
            Self::GoAway => "go_away",
            Self::ReconnectExhausted => "reconnect_exhausted",
            Self::ReconnectFailed => "reconnect_failed",
        }
    }
}

/// Fixed context for every Gemini realtime diagnostic: the realtime S2S
/// operation over a WebSocket transport.
fn realtime_context() -> RuntimeDiagnosticContext {
    RuntimeDiagnosticContext::new(RuntimeOperation::RealtimeConversation)
        .with_transport(RuntimeTransport::Websocket)
}

/// Build the closed [`RuntimeDiagnostic`] every Gemini error carries (issue
/// #145). The context is fixed to the realtime S2S operation over a
/// WebSocket transport; only the closed code / retryability / recovery
/// action / optional coarse status class + retry-delay bucket vary.
fn realtime_diagnostic(
    code: RuntimeErrorCode,
    retryable: bool,
    recovery_action: RuntimeSafeRecoveryAction,
    status_class: Option<RuntimeStatusClass>,
    retry_delay: Option<RuntimeRetryDelayBucket>,
) -> RuntimeDiagnostic {
    let mut context = realtime_context();
    if let Some(status_class) = status_class {
        context = context.with_status_class(status_class);
    }
    if let Some(retry_delay) = retry_delay {
        context = context.with_retry_delay(retry_delay);
    }
    RuntimeDiagnostic::Runtime(RuntimeErrorDiagnostic::new(
        code,
        retryable,
        recovery_action,
        context,
    ))
}

/// The closed `invalid_configuration` error for a realtime provider whose
/// local configuration is unusable (empty key, blank Vertex project or
/// location). The IPC commands' early guards and `connect()` return this same
/// typed error instead of legacy local prose errors.
pub(crate) fn invalid_configuration_error() -> crate::error::AppError {
    RuntimeDiagnostic::Runtime(RuntimeErrorDiagnostic::invalid_configuration(
        realtime_context(),
    ))
    .into()
}

/// Local empty-input guard for the Gemini auth mode (whitespace-only counts as
/// empty). Shared by `start_gemini`, `start_converse` and `connect()` so a
/// blank field reads the same typed `invalid_configuration` everywhere.
pub(crate) fn validate_auth_configuration(
    auth: &crate::settings::GeminiAuthMode,
) -> Result<(), crate::error::AppError> {
    let blank = match auth {
        crate::settings::GeminiAuthMode::ApiKey { api_key } => api_key.trim().is_empty(),
        crate::settings::GeminiAuthMode::VertexAI {
            project_id,
            location,
            ..
        } => project_id.trim().is_empty() || location.trim().is_empty(),
    };
    if blank {
        Err(invalid_configuration_error())
    } else {
        Ok(())
    }
}

/// Map the FSM's coarse [`crate::converse::TurnErrorCategory`] onto the
/// closed runtime diagnostic envelope (issue #145). This is only the FALLBACK
/// for engine signals that carry no diagnosis of their own (the STT adapter);
/// engine-supplied diagnostics are forwarded untouched. Delegates to
/// [`GeminiErrorCategory::runtime_diagnostic`] so the mapping lives in one
/// place.
fn turn_error_runtime_diagnostic(
    category: crate::converse::TurnErrorCategory,
) -> RuntimeDiagnostic {
    use crate::converse::TurnErrorCategory as C;
    match category {
        C::Auth => GeminiErrorCategory::Auth,
        C::Server => GeminiErrorCategory::Server,
        C::Network => GeminiErrorCategory::Network,
        C::Unknown => GeminiErrorCategory::Unknown,
    }
    .runtime_diagnostic()
}

/// The `GEMINI_STATUS` / `OPENAI_REALTIME_STATUS` payload both converse sinks
/// emit for an engine error (issue #145). It carries ONLY the closed
/// `diagnostic` — the engine's own diagnosis when it supplied one (so a rate
/// limit stays `rate_limited`, never an outage), else the fallback derived
/// from the coarse category. No message string, no coarse category: nothing
/// provider-controlled and nothing misleading crosses IPC. Pure so the sinks'
/// wire shape is unit-testable without an `AppHandle`.
pub(crate) fn converse_error_status_payload(
    category: crate::converse::TurnErrorCategory,
    diagnostic: Option<&RuntimeDiagnostic>,
) -> Value {
    let diagnostic = diagnostic
        .cloned()
        .unwrap_or_else(|| turn_error_runtime_diagnostic(category));
    json!({ "type": "error", "diagnostic": diagnostic })
}

/// Events emitted by the Gemini Live client to downstream consumers.
///
/// Serializable so Tauri can emit them directly to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum GeminiEvent {
    /// A transcription of the user's speech (input audio).
    #[serde(rename = "transcription")]
    Transcription { text: String, is_final: bool },
    /// A model-generated response to the audio input.
    #[serde(rename = "model_response")]
    ModelResponse { text: String },
    /// The model finished its current turn.
    ///
    /// `usage` is populated from the top-level `usageMetadata` field when the
    /// server attaches one to this frame (see [`UsageMetadata`]). Many
    /// turn-complete frames do not carry usage — it is typically bundled with
    /// the final model turn boundary. Callers that track cumulative usage
    /// should sum the values they see and ignore `None`.
    #[serde(rename = "turn_complete")]
    TurnComplete { usage: Option<UsageMetadata> },
    /// A non-fatal error occurred.
    ///
    /// `category` is the structured classification derived at the error site
    /// (close-frame code + reason, tungstenite error kind, HTTP status);
    /// `diagnostic` is the closed [`RuntimeDiagnostic`] envelope (issue #145)
    /// the frontend routes through `runtimeDiagnosticPresentation` for a
    /// localized, closed-key toast. `message` is a stable
    /// [`GeminiErrorSite::token`] site identifier — it never carries
    /// provider text, close reasons, request ids, or payload lengths.
    #[serde(rename = "error")]
    Error {
        category: GeminiErrorCategory,
        message: String,
        diagnostic: RuntimeDiagnostic,
    },

    /// The connection has been established.
    #[serde(rename = "connected")]
    Connected,
    /// The WebSocket connection was closed.
    #[serde(rename = "disconnected")]
    Disconnected,
    /// The client detected a disconnect and is attempting to reconnect.
    ///
    /// Emitted at the start of each reconnect attempt. `attempt` is 1-based:
    /// attempt 1 is the first retry after the initial loss.
    #[serde(rename = "reconnecting")]
    Reconnecting { attempt: u32, backoff_secs: u64 },
    /// The client successfully re-established the WebSocket (and re-ran the
    /// setup handshake) after a disconnect.
    ///
    /// `resumed` distinguishes the two outcomes operators care about:
    /// * `true` — the reconnect sent a cached `sessionResumption.handle` and
    ///   the server accepted it, so prior conversation state (in-flight turn,
    ///   input transcription context) is preserved across the outage.
    /// * `false` — the reconnect sent an empty `sessionResumption: {}`
    ///   because no handle was available (first outage before any
    ///   `sessionResumptionUpdate` arrived, or the last update reported
    ///   `resumable: false`). The new socket starts from a blank state.
    ///
    /// Note: we currently cannot detect server-side handle rejection from
    /// the `setupComplete` frame — the server silently falls back to a fresh
    /// session. So `resumed: true` means "we asked for resumption", not
    /// "the server confirmed resumption". The frontend should treat it as a
    /// best-effort hint.
    #[serde(rename = "reconnected")]
    Reconnected { resumed: bool },
    /// A chunk of native model audio output (converse-mode, AUDIO modality).
    ///
    /// Carries the **base64** payload from
    /// `serverContent.modelTurn.parts[].inlineData.data` verbatim (raw 16-bit
    /// PCM LE, mono, once decoded). `sample_rate` is the output rate, always
    /// **24000 Hz** for Gemini Live output audio (research §1.3). Consumers
    /// decode `data_base64` at the point of use (e.g. into the playback ring).
    ///
    /// We keep it as a base64 `String` rather than `Vec<u8>` because
    /// `GeminiEvent` is `Serialize` and may cross the Tauri IPC boundary; a
    /// `Vec<u8>` serializes to a JSON array of integers, which on a 24 kHz
    /// realtime stream balloons payload size + parse cost on the hottest path
    /// (CodeRabbit gemini/mod.rs:1469). A string stays compact.
    ///
    /// Only emitted when the session was set up with
    /// [`ResponseModality::Audio`]; notes-mode (TEXT) never produces this.
    #[serde(rename = "audio_chunk")]
    AudioChunk {
        data_base64: String,
        sample_rate: u32,
    },
    /// The server interrupted (barge-in): VAD detected user speech and the
    /// server canceled + discarded the in-flight generation
    /// (`serverContent.interrupted == true`, research §1.4). The client must
    /// flush any locally-buffered/unplayed audio. The cancel is automatic
    /// server-side for Gemini, so there is no client cancel to send.
    #[serde(rename = "interrupted")]
    Interrupted,
    /// Streaming transcript of the assistant's spoken reply
    /// (`serverContent.outputTranscription.text`, research §1.4). Present only
    /// in AUDIO mode (we request `outputAudioTranscription`) so the temporal
    /// graph still receives text proposals from the spoken reply.
    #[serde(rename = "output_transcription")]
    OutputTranscription { text: String },
    /// Model generation for the current turn is complete
    /// (`serverContent.generationComplete == true`). Precedes `turnComplete`
    /// (research §1.4); surfaced for turn bookkeeping (FSM `Speaking` →
    /// playback-drain → `Listening`).
    #[serde(rename = "generation_complete")]
    GenerationComplete,
}

impl GeminiEvent {
    /// The only constructor for the [`GeminiEvent::Error`] payload (issue
    /// #145): the serialized `message` is pinned to the closed site token so
    /// raw provider / transport text can never reach logs, IPC, or the UI.
    fn error(
        site: GeminiErrorSite,
        category: GeminiErrorCategory,
        diagnostic: RuntimeDiagnostic,
    ) -> Self {
        Self::Error {
            category,
            message: site.token().to_string(),
            diagnostic,
        }
    }
}

/// Token usage metadata parsed from a Gemini Live server message.
///
/// Mirrors the `usageMetadata` block documented at
/// <https://ai.google.dev/api/live#usage-metadata>. Fields are optional because
/// the server only populates counters that are meaningful for the current
/// frame (e.g. `cachedContentTokenCount` is omitted when no prompt cache was
/// hit). A missing field is serialized as `null` so the frontend can
/// distinguish "zero" from "not reported".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UsageMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_token_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_content_token_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_token_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_use_prompt_token_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thoughts_token_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_token_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompt_tokens_details: Vec<ModalityTokenCount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cache_tokens_details: Vec<ModalityTokenCount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub response_tokens_details: Vec<ModalityTokenCount>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_use_prompt_tokens_details: Vec<ModalityTokenCount>,
}

/// Per-modality token count (TEXT, AUDIO, IMAGE, VIDEO …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModalityTokenCount {
    pub modality: String,
    pub token_count: u32,
}

/// Which output modality the model should generate for a session.
///
/// Gemini Live accepts **exactly one** modality in `responseModalities` per
/// session — it is AUDIO **XOR** TEXT, never both (research §1.2). This enum
/// encodes that constraint so callers cannot request the (rejected) combined
/// form.
///
/// * [`Text`](Self::Text) — the historical notes/graph default. The model
///   returns `modelTurn.parts[].text`; only `inputAudioTranscription` is
///   requested. Unchanged behaviour for notes-mode.
/// * [`Audio`](Self::Audio) — native speech-out for converse-mode. The model
///   returns base64 PCM16 LE @ 24 kHz audio in `modelTurn.parts[].inlineData`;
///   the setup additionally requests `speechConfig.voiceConfig` and
///   `outputAudioTranscription` (so the graph still gets text alongside the
///   spoken reply, per research §1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseModality {
    /// Text output (notes-mode default).
    #[default]
    Text,
    /// Native audio output (converse-mode).
    Audio,
}

impl ResponseModality {
    /// The string Gemini expects in `generationConfig.responseModalities`.
    fn as_setup_str(self) -> &'static str {
        match self {
            ResponseModality::Text => "TEXT",
            ResponseModality::Audio => "AUDIO",
        }
    }
}

/// Default Gemini prebuilt voice for AUDIO sessions when none is configured.
///
/// `Kore` is one of Gemini's prebuilt voices (others: `Puck`, `Charon`,
/// `Aoede`, `Fenrir`); chosen as a neutral default per research §1.2.
pub const DEFAULT_GEMINI_VOICE: &str = "Kore";
const EXPLICIT_POLICY_REQUIRED: &str = "explicit_policy_required";

/// Configuration for a Gemini Live session.
#[derive(Clone)]
pub struct GeminiConfig {
    /// Authentication mode (API key or Vertex AI with bearer token).
    pub auth: crate::settings::GeminiAuthMode,
    /// Model name (e.g. `"gemini-2.0-flash-live-001"`).
    pub model: String,
    /// Output modality for this session (research §1.2 / ADR-0018).
    ///
    /// Defaults to [`ResponseModality::Text`] so notes-mode is unchanged;
    /// converse-mode sets [`ResponseModality::Audio`] to enable native
    /// speech-out. AUDIO XOR TEXT — never both.
    pub response_modality: ResponseModality,
    /// Prebuilt voice name for AUDIO sessions
    /// (`speechConfig.voiceConfig.prebuiltVoiceConfig.voiceName`). Ignored when
    /// [`Self::response_modality`] is [`ResponseModality::Text`]. Empty falls
    /// back to [`DEFAULT_GEMINI_VOICE`].
    pub voice_name: String,
    /// Runtime privacy guard for session audio egress.
    pub content_egress_policy: crate::asr::ProviderContentEgressPolicy,
}

impl std::fmt::Debug for GeminiConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeminiConfig")
            .field("auth", &self.auth)
            .field("model", &self.model)
            .field("response_modality", &self.response_modality)
            .field("voice_name", &self.voice_name)
            .field("content_egress_policy", &self.content_egress_policy)
            .finish()
    }
}

impl GeminiConfig {
    /// Construct a TEXT-output config (the notes/graph default). Keeps the
    /// historical two-argument call shape so existing callers stay terse.
    pub fn text(auth: crate::settings::GeminiAuthMode, model: impl Into<String>) -> Self {
        Self {
            auth,
            model: model.into(),
            response_modality: ResponseModality::Text,
            voice_name: DEFAULT_GEMINI_VOICE.to_string(),
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::block(
                EXPLICIT_POLICY_REQUIRED,
            ),
        }
    }

    /// Construct an AUDIO-output config (converse-mode native speech-out).
    /// An empty `voice_name` falls back to [`DEFAULT_GEMINI_VOICE`].
    pub fn audio(
        auth: crate::settings::GeminiAuthMode,
        model: impl Into<String>,
        voice_name: impl Into<String>,
    ) -> Self {
        let voice_name = voice_name.into();
        let voice_name = if voice_name.trim().is_empty() {
            DEFAULT_GEMINI_VOICE.to_string()
        } else {
            voice_name
        };
        Self {
            auth,
            model: model.into(),
            response_modality: ResponseModality::Audio,
            voice_name,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::block(
                EXPLICIT_POLICY_REQUIRED,
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Internal message passed from sync send_audio() → async writer task
// ---------------------------------------------------------------------------

enum AudioCmd {
    /// Base64-encoded PCM chunk ready to send.
    Chunk(String),
    /// Signal end of the current user turn (`audioStreamEnd`) WITHOUT closing
    /// the socket, so the model starts generating its reply and the session
    /// stays open for the assistant audio + the next turn (B18 / ADR-0018
    /// `TurnAction::EndUserTurn`). Distinct from [`AudioCmd::Stop`], which also
    /// sends `audioStreamEnd` but then tears the socket down.
    EndTurn,
    /// Signal end of audio stream and close.
    Stop,
}

/// The `realtimeInput.audioStreamEnd` frame Gemini Live expects to mark the end
/// of a user turn. Shared by the per-turn [`AudioCmd::EndTurn`] (socket stays
/// open) and the teardown [`AudioCmd::Stop`] (socket closes after) so the two
/// paths can never drift. Pure — unit-testable without a socket.
fn audio_stream_end_frame() -> String {
    json!({ "realtimeInput": { "audioStreamEnd": true } }).to_string()
}

// ---------------------------------------------------------------------------
// Type aliases for the split WebSocket halves
// ---------------------------------------------------------------------------

type WsWriter = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

type WsReader = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A Gemini Live bidirectional streaming client.
///
/// The public methods (`connect`, `send_audio`, `disconnect`, `event_rx`) are
/// all **synchronous** — they block the caller's thread just long enough to
/// hand off work to the internal async runtime. This matches the threading
/// model used by `commands.rs` where worker threads run in `std::thread`.
pub struct GeminiLiveClient {
    config: GeminiConfig,
    /// crossbeam event channel — writer side (background reader task pushes here).
    event_tx: crossbeam_channel::Sender<GeminiEvent>,
    /// crossbeam event channel — reader side (command layer clones this).
    event_rx: crossbeam_channel::Receiver<GeminiEvent>,
    /// Whether the WebSocket is connected.
    connected: Arc<AtomicBool>,
    /// Set to `true` when the user has explicitly called `disconnect()`.
    ///
    /// Used by the session task to distinguish a user-initiated teardown
    /// (do not auto-reconnect) from a network error or server close
    /// (auto-reconnect with exponential backoff).
    user_disconnected: Arc<AtomicBool>,
    /// Tokio runtime that owns the WebSocket tasks.
    rt: Option<tokio::runtime::Runtime>,
    /// Sender for audio commands → async writer task.
    audio_tx: Option<tokio_mpsc::Sender<AudioCmd>>,
    /// Handle to the session task (owns both halves + reconnect logic).
    ///
    /// Kept alive for as long as the client is connected; dropped when the
    /// client is dropped (the runtime shutdown in `Drop` joins it). Never
    /// read directly — leading underscore mirrors `crate::asr::deepgram`.
    _session_handle: Option<tokio::task::JoinHandle<()>>,
    /// Latest session-resumption handle received from the server.
    ///
    /// Updated from `sessionResumptionUpdate` frames whenever the server
    /// reports `resumable: true`. On reconnect, the current value (if any)
    /// is sent back in the `BidiGenerateContentSetup.sessionResumption.handle`
    /// field so the server restores the prior conversation state instead of
    /// starting a fresh one. `None` means either (a) the initial session
    /// hasn't run long enough to receive an update yet, or (b) the last
    /// update reported `resumable: false` (e.g. mid-generation).
    resumption_handle: Arc<std::sync::Mutex<Option<String>>>,
}

impl GeminiLiveClient {
    /// Create a new (disconnected) Gemini Live client with the given config.
    pub fn new(config: GeminiConfig) -> Self {
        let (event_tx, event_rx) = crossbeam_channel::bounded(128);
        Self {
            config,
            event_tx,
            event_rx,
            connected: Arc::new(AtomicBool::new(false)),
            user_disconnected: Arc::new(AtomicBool::new(false)),
            rt: None,
            audio_tx: None,
            _session_handle: None,
            resumption_handle: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    // ------------------------------------------------------------------
    // Connect
    // ------------------------------------------------------------------

    /// Connect to the Gemini Live API.
    ///
    /// Blocks the caller until the WebSocket is open and `setupComplete` has
    /// been received, then spawns a background session task on an internal
    /// tokio runtime. The session task handles audio writing, server message
    /// reading, and automatic reconnection with exponential backoff if the
    /// WebSocket drops mid-session (see `session_task` in this module).
    /// Issue #145: `connect` returns a closed [`AppError`] (typed
    /// `RuntimeDiagnostic` variant) — never a raw provider or transport
    /// string. The command layer forwards it to the frontend, which routes
    /// it through `runtimeDiagnosticPresentation` for localized copy.
    pub fn connect(&mut self) -> Result<(), crate::error::AppError> {
        // Validate auth configuration before proceeding. Issue #145: closed
        // `invalid_configuration` diagnostic, no prose.
        validate_auth_configuration(&self.config.auth)?;

        // Build a dedicated single-threaded tokio runtime for the WebSocket.
        // Issue #145: unclassified native failure → closed `internal`
        // diagnostic; the source error is dropped, never logged.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("gemini-ws-rt")
            .build()
            .map_err(|e| {
                crate::error::AppError::from_unclassified_runtime_error(
                    &e,
                    RuntimeDiagnosticContext::new(RuntimeOperation::RealtimeConversation)
                        .with_transport(RuntimeTransport::Native),
                )
            })?;

        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let connected = Arc::clone(&self.connected);
        let user_disconnected = Arc::clone(&self.user_disconnected);
        let resumption_handle = Arc::clone(&self.resumption_handle);
        // Reset on (re)connect so a prior teardown flag doesn't poison a
        // fresh session.
        user_disconnected.store(false, Ordering::SeqCst);
        // Fresh client = no prior handle. Any stale value from a previous
        // connect cycle on the same struct is cleared so we don't try to
        // resume a session that belongs to a different runtime.
        if let Ok(mut guard) = resumption_handle.lock() {
            *guard = None;
        }

        // Perform the blocking initial connect + setup handshake inside the
        // runtime. Surfaced synchronously so the caller sees auth / network
        // errors immediately instead of through the reconnect loop.
        let (audio_tx, session_handle) = rt.block_on(async move {
            // No handle on the very first connect — request resumption so
            // the server will start sending `sessionResumptionUpdate` frames.
            let (writer, reader) = open_ws(&config, None).await.map_err(|e| {
                // Issue #145: log only the closed category; the connect
                // error's raw source never crossed this boundary (it was
                // dropped inside `open_ws`). Return the closed diagnostic
                // envelope so the command layer / frontend classify
                // structurally.
                log::warn!("Gemini Live: connect failed ({:?})", e.category);
                crate::error::AppError::from(e.diagnostic)
            })?;

            log::info!("Gemini Live: setup complete");
            connected.store(true, Ordering::SeqCst);

            let _ = event_tx.send(GeminiEvent::Connected);

            // Build the audio command channel the caller will push into.
            let (atx, arx) = tokio_mpsc::channel::<AudioCmd>(GEMINI_AUDIO_QUEUE_CAP);

            // Spawn the session task, which owns both halves of the socket
            // and handles reconnects (including full setup-handshake replay).
            let session_handle = tokio::spawn(session_task(
                writer,
                reader,
                arx,
                config,
                event_tx,
                connected,
                user_disconnected,
                resumption_handle,
            ));

            Ok::<_, crate::error::AppError>((atx, session_handle))
        })?;

        self.audio_tx = Some(audio_tx);
        self._session_handle = Some(session_handle);
        self.rt = Some(rt);

        Ok(())
    }

    // ------------------------------------------------------------------
    // Send audio
    // ------------------------------------------------------------------

    /// Send PCM audio data to Gemini for processing.
    ///
    /// The audio should be **f32 mono 16 kHz** (matching the pipeline output).
    /// The method converts to 16-bit LE PCM, base64-encodes, and queues for
    /// async sending. Returns immediately (non-blocking).
    ///
    /// # Behaviour during auto-reconnect
    ///
    /// This method *does not* check the `connected` flag — only
    /// `user_disconnected`. That way, if the session task is in the middle of
    /// a reconnect cycle, audio is still queued to the unbounded channel and
    /// will be flushed as soon as the new socket finishes its setup handshake.
    /// Callers never see a spurious "Not connected" error for a transient
    /// network hiccup. Note: the receiving model is a fresh instance, so any
    /// in-flight turn from the old socket is lost (see module-level docs).
    pub fn send_audio(&self, audio: &[f32]) -> Result<(), String> {
        if self.user_disconnected.load(Ordering::SeqCst) {
            return Err("Gemini client has been disconnected".to_string());
        }

        if audio.is_empty() {
            return Ok(());
        }

        self.config
            .content_egress_policy
            .check_audio("gemini.live")?;

        let tx = self
            .audio_tx
            .as_ref()
            .ok_or_else(|| "Audio channel not initialized".to_string())?;

        // f32 → i16 LE PCM → base64
        let pcm_bytes = f32_to_i16_le_bytes(audio);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&pcm_bytes);

        // Non-blocking, bounded send: if the queue is full (e.g. a stalled
        // reconnect), drop the newest chunk rather than growing memory without
        // bound. A `Closed` channel is a real error the caller should see.
        match tx.try_send(AudioCmd::Chunk(b64)) {
            Ok(()) => Ok(()),
            Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                let n = GEMINI_AUDIO_DROPS.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 50 == 1 {
                    log::warn!(
                        "Gemini outbound audio queue full ({} chunks); dropping audio \
                         (total dropped: {}). The socket or reconnect is falling behind.",
                        GEMINI_AUDIO_QUEUE_CAP,
                        n
                    );
                }
                Ok(())
            }
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                Err("Audio channel closed".to_string())
            }
        }
    }

    /// Signal end-of-user-turn to the engine (`audioStreamEnd`) so it starts
    /// generating, **without** closing the socket — the session stays open for
    /// the assistant reply and the next turn. This is the engine binding for
    /// ADR-0018 `TurnAction::EndUserTurn` (B18 native S2S). Contrast with
    /// [`Self::disconnect`], which sends the same frame but then tears down.
    ///
    /// With Gemini server-VAD the model may also end the turn implicitly; this
    /// makes the explicit FSM-driven boundary deterministic. Best-effort, like
    /// [`Self::send_audio`]: a full queue drops the signal (the next turn's VAD
    /// still ends it); a closed channel is a real error.
    pub fn end_user_turn(&self) -> Result<(), String> {
        if self.user_disconnected.load(Ordering::SeqCst) {
            return Err("Gemini client has been disconnected".to_string());
        }
        let tx = self
            .audio_tx
            .as_ref()
            .ok_or_else(|| "Audio channel not initialized".to_string())?;
        match tx.try_send(AudioCmd::EndTurn) {
            Ok(()) => Ok(()),
            Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                log::warn!("Gemini: end_user_turn dropped (outbound queue full)");
                Ok(())
            }
            Err(tokio_mpsc::error::TrySendError::Closed(_)) => {
                Err("Audio channel closed".to_string())
            }
        }
    }

    // ------------------------------------------------------------------
    // Event receiver
    // ------------------------------------------------------------------

    /// Get a clone of the event receiver channel.
    ///
    /// The command layer uses this to read `GeminiEvent`s from a worker thread.
    pub fn event_rx(&self) -> crossbeam_channel::Receiver<GeminiEvent> {
        self.event_rx.clone()
    }

    // ------------------------------------------------------------------
    // Status
    // ------------------------------------------------------------------

    /// Check if the client is currently connected.
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    // ------------------------------------------------------------------
    // Disconnect
    // ------------------------------------------------------------------

    /// Disconnect from the Gemini Live API and clean up resources.
    ///
    /// Sends `audioStreamEnd`, closes the WebSocket, waits for background
    /// tasks to finish, and shuts down the internal tokio runtime. Setting
    /// `user_disconnected` prevents the session task from attempting to
    /// auto-reconnect after the close frame is observed.
    pub fn disconnect(&self) {
        log::info!("GeminiLiveClient: disconnecting (user-initiated)");

        // Mark this teardown as user-initiated so the session task does not
        // try to reconnect after the close frame is observed.
        self.user_disconnected.store(true, Ordering::SeqCst);

        // Signal not connected first (stops send_audio calls).
        self.connected.store(false, Ordering::SeqCst);

        // Tell the writer task to send audioStreamEnd + close.
        if let Some(ref tx) = self.audio_tx {
            let _ = tx.try_send(AudioCmd::Stop);
        }

        let _ = self.event_tx.send(GeminiEvent::Disconnected);

        // The runtime and task handles are cleaned up on Drop. We don't
        // block here because disconnect() is called from a Mutex guard in
        // stop_gemini and we want to avoid deadlock with the rt shutdown.
    }
}

impl Drop for GeminiLiveClient {
    fn drop(&mut self) {
        // Mark teardown as user-initiated so the session task exits cleanly
        // instead of trying to reconnect after we shut the runtime down.
        self.user_disconnected.store(true, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);

        // Signal writer to stop.
        if let Some(ref tx) = self.audio_tx {
            let _ = tx.try_send(AudioCmd::Stop);
        }
        self.audio_tx = None;

        // Shut down the tokio runtime (this joins background tasks).
        if let Some(rt) = self.rt.take() {
            // Give tasks a moment to finish cleanly.
            rt.shutdown_timeout(std::time::Duration::from_secs(3));
        }

        log::info!("GeminiLiveClient: dropped");
    }
}

// ===========================================================================
// Free functions — async building blocks
// ===========================================================================

/// Classifies *why* the session dropped so downstream logs / events can be
/// precise without the caller re-parsing error strings.
///
/// Every payload-carrying variant holds a closed [`RuntimeDiagnostic`] (issue
/// #145) — never a raw reason string. The `Debug` render of this enum (which
/// is what `log::info!("… ({disconnect:?})")` prints) therefore only ever
/// shows closed enum tokens.
#[derive(Debug)]
#[allow(dead_code)]
enum DisconnectKind {
    /// Remote server sent a Close frame. Typically a graceful server-side
    /// teardown (e.g. `goAway`, idle timeout). Carries the closed
    /// classification for the frame (issue #145) — `category` is `None` for
    /// normal / unclassified closes, which do not surface an error event.
    ServerClose {
        category: Option<GeminiErrorCategory>,
        diagnostic: RuntimeDiagnostic,
    },
    /// Transport-level error (TLS, TCP reset, DNS flap, tungstenite I/O).
    NetworkError(RuntimeDiagnostic),
    /// Protocol violation — malformed frame, invalid sequence, etc.
    ProtocolError(RuntimeDiagnostic),
    /// User called `disconnect()`. No reconnect attempt should be made.
    UserRequested,
    /// Writer task exhausted the audio command stream (caller dropped the
    /// sender). No reconnect — session is genuinely over.
    WriterEnded,
}

/// Build the `BidiGenerateContentSetup` JSON message.
///
/// Called once per (re)connect so reconnects see fresh `generationConfig` +
/// `system_instruction` values even if the config struct were mutated
/// between attempts.
///
/// `resumption_handle` semantics (per
/// <https://ai.google.dev/api/live#session-management>):
/// * `None` — first connect or post-outage with no usable handle. Sends an
///   empty `sessionResumption: {}` so the server enables resumption updates
///   and starts pushing `sessionResumptionUpdate` frames.
/// * `Some(h)` — reconnect with a live handle. Sends
///   `sessionResumption: { handle: h }` so the server restores the prior
///   session state. If the server rejects the handle it falls back to a
///   fresh session transparently (still returns `setupComplete`).
fn build_setup_message(config: &GeminiConfig, resumption_handle: Option<&str>) -> Value {
    let model_path = match &config.auth {
        crate::settings::GeminiAuthMode::ApiKey { .. } => {
            format!("models/{}", config.model)
        }
        crate::settings::GeminiAuthMode::VertexAI {
            project_id,
            location,
            ..
        } => {
            format!(
                "projects/{}/locations/{}/publishers/google/models/{}",
                project_id, location, config.model,
            )
        }
    };

    let session_resumption = match resumption_handle {
        Some(handle) => json!({ "handle": handle }),
        None => json!({}),
    };

    // `responseModalities` accepts exactly one modality (AUDIO XOR TEXT;
    // research §1.2). `inputAudioTranscription` stays in both modes so user
    // speech keeps feeding the graph (unchanged from notes-mode). For AUDIO
    // sessions we additionally request a prebuilt voice and
    // `outputAudioTranscription` so the graph still receives the spoken reply
    // as text alongside the audio.
    let mut generation_config = json!({
        "responseModalities": [config.response_modality.as_setup_str()],
        "inputAudioTranscription": {},
    });

    if config.response_modality == ResponseModality::Audio {
        let voice = if config.voice_name.trim().is_empty() {
            DEFAULT_GEMINI_VOICE
        } else {
            config.voice_name.as_str()
        };
        generation_config["speechConfig"] = json!({
            "voiceConfig": {
                "prebuiltVoiceConfig": { "voiceName": voice }
            }
        });
        // Spoken reply also surfaced as text for graph proposals.
        generation_config["outputAudioTranscription"] = json!({});
    }

    json!({
        "setup": {
            "model": model_path,
            "generationConfig": generation_config,
            "sessionResumption": session_resumption
        }
    })
}

/// Classify a WebSocket close frame into a [`GeminiErrorCategory`].
///
/// The Gemini Live service signals auth, quota, and server errors through
/// `CloseFrame.code` + `CloseFrame.reason`. We key off the numeric code
/// (1008 = policy violation, 1011 = server error) and then scan the reason
/// string (lowercased) for the signal words that tell the Auth vs.
/// RateLimit variants apart. The reason is provider prose, so it is never
/// taken as evidence of credential EXPIRY: an "expired" reason is just an
/// unrecognized 1008 and falls through to [`GeminiErrorCategory::Unknown`],
/// like any bare 1008 with none of the known markers, so we don't lie about
/// remediation.
///
/// Returns `None` if the frame is a normal close (code 1000) or any code
/// we don't want to surface as an error (transient server restarts do not
/// warrant a user-visible toast — the reconnect loop handles them).
fn classify_close_frame(code: u16, reason: &str) -> Option<GeminiErrorCategory> {
    let r = reason.to_lowercase();

    match code {
        1000 => None, // normal closure — not an error
        1008 => {
            // Policy violation — auth / quota family.
            if r.contains("api key") {
                Some(GeminiErrorCategory::Auth)
            } else if r.contains("quota") {
                Some(GeminiErrorCategory::RateLimit { retry_delay: None })
            } else {
                Some(GeminiErrorCategory::Unknown)
            }
        }
        1011 => Some(GeminiErrorCategory::Server),
        _ => None,
    }
}

/// Closed diagnostic for a category derived from a WebSocket close frame.
/// The close REASON is provider prose, so a prose-derived `Auth` /
/// `AuthExpired` category must not manufacture a credential diagnosis
/// (`credential_rejected` / `credential_expired`): a 1008 close is the
/// generic closed `request_rejected` / `review_configuration` code. The
/// category itself is unchanged, so terminal-auth teardown still fires.
/// Credential codes come only from structured signals (an HTTP 401 on the
/// handshake, an OpenAI `invalid_api_key` error code).
fn close_frame_diagnostic(category: &GeminiErrorCategory) -> RuntimeDiagnostic {
    match category {
        GeminiErrorCategory::Auth | GeminiErrorCategory::AuthExpired => realtime_diagnostic(
            RuntimeErrorCode::RequestRejected,
            false,
            RuntimeSafeRecoveryAction::ReviewConfiguration,
            Some(RuntimeStatusClass::ClientError),
            None,
        ),
        other => other.runtime_diagnostic(),
    }
}

/// Reduce a numeric `Retry-After` header to a closed
/// [`RuntimeRetryDelayBucket`] (issue #145). The exact seconds are dropped;
/// an absent, non-UTF-8, or non-numeric (HTTP-date) header yields `None` —
/// absence is never mapped to a bucket. Shared with the OpenAI Realtime S2S
/// classifier.
pub(crate) fn retry_delay_bucket_from_headers(
    headers: &tungstenite::http::HeaderMap,
) -> Option<RuntimeRetryDelayBucket> {
    headers
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(RuntimeRetryDelayBucket::from_seconds)
}

/// Classify a `tungstenite::Error` encountered while connecting or reading
/// from the socket into a [`GeminiErrorCategory`].
///
/// Priority order:
/// 1. `Http(response)` — inspect the status code. 429 → RateLimit (parsing
///    `Retry-After` if the server included one), 5xx → Server, 401/403 →
///    Auth, anything else → Unknown (we've seen enough of the response to
///    know it isn't network, but not enough to name it).
/// 2. `Io(_)`, `Tls(_)`, `ConnectionClosed`, `AlreadyClosed`, `Url(_)` —
///    transport-layer. Map to `Network`.
/// 3. Everything else (protocol violations, capacity limits, attack
///    attempts) → `Unknown`.
fn classify_tungstenite_error(err: &tungstenite::Error) -> GeminiErrorCategory {
    match err {
        tungstenite::Error::Http(response) => {
            let status = response.status().as_u16();
            if status == 429 {
                // Issue #145: the numeric `Retry-After` value is reduced to a
                // closed delay bucket here and the exact seconds are dropped —
                // they never reach the category, logs, or the wire. An absent
                // or non-numeric header leaves the bucket absent.
                GeminiErrorCategory::RateLimit {
                    retry_delay: retry_delay_bucket_from_headers(response.headers()),
                }
            } else if (500..600).contains(&status) {
                GeminiErrorCategory::Server
            } else if status == 401 || status == 403 {
                GeminiErrorCategory::Auth
            } else {
                GeminiErrorCategory::Unknown
            }
        }
        tungstenite::Error::Io(_)
        | tungstenite::Error::Tls(_)
        | tungstenite::Error::ConnectionClosed
        | tungstenite::Error::AlreadyClosed
        | tungstenite::Error::Url(_) => GeminiErrorCategory::Network,
        _ => GeminiErrorCategory::Unknown,
    }
}

/// A classified failure surfaced from [`open_ws`] to its caller so the
/// session task can emit a correctly-categorized `GeminiEvent::Error`
/// without string-parsing the display form.
///
/// Issue #145: the raw source (HTTP status, close reason, tungstenite /
/// `gcp_auth` error) is inspected at the construction site to pick
/// `category` and then **discarded** — it is never retained, logged, or
/// serialized. The closed [`RuntimeDiagnostic`] is the only thing that
/// crosses to events, the returned [`AppError`](crate::error::AppError),
/// and the frontend.
#[derive(Debug, Clone)]
struct GeminiConnectError {
    category: GeminiErrorCategory,
    diagnostic: RuntimeDiagnostic,
}

impl GeminiConnectError {
    /// Build a classified connect failure from a positively-identified
    /// category. The raw source error stays at the call site.
    fn new(category: GeminiErrorCategory) -> Self {
        Self {
            diagnostic: category.runtime_diagnostic(),
            category,
        }
    }

    /// Connect failure from a tungstenite error. Category comes from
    /// `classify_tungstenite_error`. An HTTP 401/403 handshake rejection is
    /// typed through `from_http_status` (the credential was sent), so a 401
    /// is `credential_rejected` and a 403 is `permission_denied`. Only the
    /// status class survives; the response is dropped.
    fn from_tungstenite(err: &tungstenite::Error) -> Self {
        let category = classify_tungstenite_error(err);
        let diagnostic = match err {
            tungstenite::Error::Http(response)
                if matches!(response.status().as_u16(), 401 | 403) =>
            {
                RuntimeDiagnostic::Runtime(RuntimeErrorDiagnostic::from_http_status(
                    response.status().as_u16(),
                    true,
                    realtime_context(),
                ))
            }
            _ => category.runtime_diagnostic(),
        };
        Self {
            category,
            diagnostic,
        }
    }

    /// `Auth` failure caused by local credential configuration (unreadable
    /// service-account file, no application default credentials).
    fn auth_configuration() -> Self {
        Self {
            category: GeminiErrorCategory::Auth,
            diagnostic: RuntimeDiagnostic::Runtime(RuntimeErrorDiagnostic::invalid_configuration(
                realtime_context(),
            )),
        }
    }

    /// The saved API key cannot be encoded as a header value: a local
    /// configuration problem. The category stays `Unknown` (unchanged
    /// teardown behavior); only the diagnostic is the honest code.
    fn unencodable_api_key() -> Self {
        Self {
            category: GeminiErrorCategory::Unknown,
            diagnostic: RuntimeDiagnostic::Runtime(RuntimeErrorDiagnostic::invalid_configuration(
                realtime_context(),
            )),
        }
    }

    /// Vertex token acquisition failure. The SDK exposes no structural
    /// "credential rejected" signal (a non-success token response is a
    /// static message regardless of status), so this never emits
    /// `credential_rejected`. A transport-level error is a `Network`
    /// failure; anything else keeps the terminal `Auth` session-end category
    /// with the generic closed `internal` diagnostic.
    fn from_gcp_token_error(err: &gcp_auth::Error) -> Self {
        match err {
            gcp_auth::Error::Http(..) => Self::new(GeminiErrorCategory::Network),
            _ => Self {
                category: GeminiErrorCategory::Auth,
                diagnostic: GeminiErrorCategory::Unknown.runtime_diagnostic(),
            },
        }
    }
}

/// Open a fresh Gemini Live WebSocket using the live [`GeminiConfig`].
///
/// Unlike the Deepgram / AssemblyAI equivalents (whose handshake is entirely
/// in the upgrade request), Gemini requires a stateful setup message exchange
/// *after* the socket is open:
///
/// 1. Build URL + upgrade request based on auth mode (API key header vs.
///    Vertex bearer token).
/// 2. `connect_async` to establish the WebSocket.
/// 3. Split into reader + writer.
/// 4. Send the `BidiGenerateContentSetup` frame, including a
///    `sessionResumption` block. If `resumption_handle` is `Some`, the
///    server will attempt to restore the prior session identified by that
///    opaque token; otherwise a brand-new session is created.
/// 5. Await `setupComplete` on the reader.
/// 6. Return `(writer, reader)`. The resumption handle is *not* returned
///    here — it arrives asynchronously as `sessionResumptionUpdate` frames
///    later in the session.
///
/// Used for the initial connect *and* every reconnect attempt, so the full
/// handshake is replayed on reconnect. Callers pass the latest handle they
/// have seen so far so the server can stitch turns across the outage.
/// The auth/content header pairs the ApiKey branch of [`open_ws`] layers onto
/// the upgrade request. Extracted so the production-path regression test
/// exercises the REAL header assembly — a typo'd or dropped header here fails
/// the test, which is exactly how the original B2 defect would have been
/// caught. Security: the key rides in the `x-goog-api-key` header, never the
/// URL query string.
fn gemini_api_key_connect_headers(
    api_key: &str,
) -> Result<
    Vec<(
        tungstenite::http::HeaderName,
        tungstenite::http::HeaderValue,
    )>,
    GeminiConnectError,
> {
    let api_key_value = tungstenite::http::HeaderValue::from_str(api_key)
        .map_err(|_| GeminiConnectError::unencodable_api_key())?;
    Ok(vec![
        (
            tungstenite::http::HeaderName::from_static("x-goog-api-key"),
            api_key_value,
        ),
        (
            tungstenite::http::header::CONTENT_TYPE,
            tungstenite::http::HeaderValue::from_static("application/json"),
        ),
    ])
}

/// The auth/content header pairs the VertexAI branch of [`open_ws`] layers
/// onto the upgrade request. Same test-seam rationale as
/// [`gemini_api_key_connect_headers`].
fn gemini_vertex_connect_headers(
    bearer_token: &str,
) -> Result<
    Vec<(
        tungstenite::http::HeaderName,
        tungstenite::http::HeaderValue,
    )>,
    GeminiConnectError,
> {
    let bearer = tungstenite::http::HeaderValue::from_str(&format!("Bearer {bearer_token}"))
        .map_err(|_| {
            // Issue #145: classify structurally; drop the unencodable source.
            GeminiConnectError::new(GeminiErrorCategory::Unknown)
        })?;
    Ok(vec![
        (tungstenite::http::header::AUTHORIZATION, bearer),
        (
            tungstenite::http::header::CONTENT_TYPE,
            tungstenite::http::HeaderValue::from_static("application/json"),
        ),
    ])
}

/// Build the WebSocket upgrade request (via the shared header-injecting helper)
/// for `url_str` with the provider `headers`, connect, and return the split
/// halves.
///
/// Both Gemini auth branches funnel through here so the mandatory-WS-header fix
/// (audio-graph-7086 / review B2) lives in exactly one place and a test can
/// exercise the *production* request shape against a local fixture. Issue
/// #145: the tungstenite error is inspected only to classify; its display text
/// (which can echo the full request, including the auth header) is dropped
/// here and never logged or serialized.
async fn connect_gemini_ws(
    url_str: &str,
    headers: Vec<(
        tungstenite::http::HeaderName,
        tungstenite::http::HeaderValue,
    )>,
) -> Result<(WsWriter, WsReader), GeminiConnectError> {
    let request = crate::ws_request::build_ws_upgrade_request(url_str, headers)
        .map_err(|_| GeminiConnectError::new(GeminiErrorCategory::Unknown))?;

    // Bounded connect so a stalled TLS/HTTP-upgrade handshake surfaces as an
    // ordinary connect error (classified as a transport error, driving the
    // reconnect ladder) instead of hanging this future forever — this is the
    // single seam for both the ApiKey and Vertex auth modes (PR #79 note).
    // Issue #145: the tungstenite error is inspected only to classify; its
    // display text (which can echo the full request, including the auth
    // header) is dropped here.
    let (ws_stream, _response) = crate::ws_request::connect_async_bounded(request)
        .await
        .map_err(|e| GeminiConnectError::from_tungstenite(&e))?;

    Ok(ws_stream.split())
}

async fn open_ws(
    config: &GeminiConfig,
    resumption_handle: Option<&str>,
) -> Result<(WsWriter, WsReader), GeminiConnectError> {
    // ── Open WebSocket ─────────────────────────────────────────────────
    let (mut writer, reader) = match &config.auth {
        crate::settings::GeminiAuthMode::ApiKey { api_key } => {
            // Security: pass API key in header (not URL query string).
            // URLs get logged by DNS, proxies, firewalls, cert monitoring —
            // defeating TLS protection. Headers are not logged by default.
            let url_str = "wss://generativelanguage.googleapis.com/ws/\
                 google.ai.generativelanguage.v1beta.\
                 GenerativeService.BidiGenerateContent";

            // Build via the shared helper so the five mandatory WS upgrade
            // headers are injected; the old hand-built request set only the
            // API-key + content-type headers and never handshook (see
            // audio-graph-7086 / review B2). Key stays in the header, per the
            // security comment above; issue #145 drops the transport-error
            // display text entirely at the classify site. The header assembly
            // lives in `gemini_api_key_connect_headers` so the
            // production-path test exercises the exact same construction.
            let headers = gemini_api_key_connect_headers(api_key)?;
            connect_gemini_ws(url_str, headers).await?
        }
        crate::settings::GeminiAuthMode::VertexAI {
            project_id,
            location,
            service_account_path,
        } => {
            let scopes = &["https://www.googleapis.com/auth/cloud-platform"];
            let token = if let Some(sa_path) = service_account_path.as_deref().map(str::trim)
                && !sa_path.is_empty()
            {
                // Issue #145: `gcp_auth` failures classify structurally and the
                // provider's own error text is dropped at the site. An
                // unreadable service-account file or missing application
                // default credentials is `invalid_configuration`. A failed
                // token request is classified by error KIND only
                // (`from_gcp_token_error`): the SDK cannot tell a rejected
                // credential from any other non-success token response, so it
                // never claims `credential_rejected`.
                let service_account = gcp_auth::CustomServiceAccount::from_file(sa_path)
                    .map_err(|_| GeminiConnectError::auth_configuration())?;
                service_account
                    .token(scopes)
                    .await
                    .map_err(|e| GeminiConnectError::from_gcp_token_error(&e))?
            } else {
                let provider = gcp_auth::provider()
                    .await
                    .map_err(|_| GeminiConnectError::auth_configuration())?;
                provider
                    .token(scopes)
                    .await
                    .map_err(|e| GeminiConnectError::from_gcp_token_error(&e))?
            };

            let url_str = format!(
                "wss://{location}-aiplatform.googleapis.com/ws/\
                 google.cloud.aiplatform.v1beta1.\
                 LlmBidiService/BidiGenerateContent?\
                 alt=proto&key={project_id}",
            );

            // Build via the shared helper so the five mandatory WS upgrade
            // headers are injected; the old hand-built request set only the
            // bearer + content-type headers and never handshook (see
            // audio-graph-7086 / review B2, Vertex path). The bearer token
            // stays in the header; issue #145 drops the transport-error
            // display text entirely at the classify site. The header assembly
            // lives in `gemini_vertex_connect_headers` so the production-path
            // test exercises the exact same construction.
            let headers = gemini_vertex_connect_headers(token.as_str())?;
            connect_gemini_ws(&url_str, headers).await?
        }
    };

    // ── Send setup message ─────────────────────────────────────────────
    let setup_msg = build_setup_message(config, resumption_handle);
    writer
        .send(Message::Text(setup_msg.to_string().into()))
        .await
        .map_err(|e| {
            // Issue #145: classify from the tungstenite error kind only; the
            // display text (which may echo auth-header shapes) is dropped.
            GeminiConnectError::new(classify_tungstenite_error(&e))
        })?;

    // ── Wait for setupComplete ─────────────────────────────────────────
    let reader = wait_for_setup_complete(reader).await?;

    Ok((writer, reader))
}

/// Wait for `setupComplete` from the server.
///
/// Returns the reader half back so its ownership can be threaded onwards.
/// Note that the `setupComplete` frame itself does not contain a resumption
/// handle — those arrive later as separate `sessionResumptionUpdate` frames
/// (see [`handle_server_message`]).
async fn wait_for_setup_complete(mut reader: WsReader) -> Result<WsReader, GeminiConnectError> {
    let timeout = tokio::time::Duration::from_secs(15);

    loop {
        let frame = tokio::time::timeout(timeout, reader.next())
            .await
            .map_err(|_| GeminiConnectError::new(GeminiErrorCategory::Network))?
            .ok_or_else(|| GeminiConnectError::new(GeminiErrorCategory::Network))?;

        let msg = match frame {
            Ok(m) => m,
            Err(e) => {
                // Issue #145: classify structurally; drop the transport text.
                return Err(GeminiConnectError::from_tungstenite(&e));
            }
        };

        // If the server rejects the setup with a close frame, surface it
        // through the close-frame classifier so auth / quota / server-error
        // signals land on the right category even pre-handshake. Issue #145:
        // the close *reason string* is inspected for classification only —
        // it is never retained, logged, or serialized.
        if let Message::Close(frame) = &msg {
            if let Some(f) = frame {
                let code: u16 = f.code.into();
                let category = classify_close_frame(code, f.reason.as_ref())
                    .unwrap_or(GeminiErrorCategory::Unknown);
                return Err(GeminiConnectError {
                    diagnostic: close_frame_diagnostic(&category),
                    category,
                });
            }
            return Err(GeminiConnectError::new(GeminiErrorCategory::Network));
        }

        if let Message::Text(text) = msg {
            let parsed: Value = serde_json::from_str(&text)
                .map_err(|_| GeminiConnectError::new(GeminiErrorCategory::Unknown))?;

            if parsed.get("setupComplete").is_some() {
                return Ok(reader);
            }

            log::debug!(
                "Gemini Live: pre-setup message {}",
                gemini_frame_diagnostic(&parsed)
            );
        }
    }
}

/// Background task owning a single Gemini Live WebSocket session, including
/// reconnect logic.
///
/// Runs the reader and writer concurrently via `tokio::select!` in [`run_io`].
/// When either half reports a disconnect (server Close frame, tungstenite
/// error, etc.), the task:
///
/// 1. Checks the `user_disconnected` flag — if set, exits silently.
/// 2. Emits `Disconnected` + a fresh `Reconnecting { attempt }` event.
/// 3. Sleeps for the exponential backoff period (1s/2s/5s/10s), polling
///    `user_disconnected` every 100 ms so shutdown doesn't wait up to 10 s.
/// 4. Calls [`open_ws`] to re-establish the socket *including the full
///    setup-handshake replay* (send `BidiGenerateContentSetup` → await
///    `setupComplete`). This is the Gemini-specific bit that distinguishes
///    this reconnect path from Deepgram / AssemblyAI.
/// 5. On success, emits `Reconnected` and resumes the read/write loop. The
///    audio channel (`arx`) is preserved across reconnects so the caller's
///    in-flight audio is not lost — it just buffers until the new socket is
///    open.
/// 6. On failure, loops back to step 2 with the incremented attempt count.
/// 7. After 4 failed attempts, emits a fatal `Error` event and exits.
///
/// **Caveat**: any in-flight model turn on the dead socket is LOST. The fresh
/// socket starts from a blank `turnComplete` state and treats queued audio as
/// a brand-new utterance. Callers that care about turn boundaries should
/// handle the `Reconnecting`/`Reconnected` event pair as an implicit
/// `TurnComplete` barrier.
#[allow(clippy::too_many_arguments)]
async fn session_task(
    initial_writer: WsWriter,
    initial_reader: WsReader,
    mut audio_rx: tokio_mpsc::Receiver<AudioCmd>,
    config: GeminiConfig,
    event_tx: crossbeam_channel::Sender<GeminiEvent>,
    connected: Arc<AtomicBool>,
    user_disconnected: Arc<AtomicBool>,
    resumption_handle: Arc<std::sync::Mutex<Option<String>>>,
) {
    let mut writer = initial_writer;
    let mut reader = initial_reader;
    let mut reconnect_attempts: u32 = 0;

    loop {
        // Drive reader + writer concurrently until one side signals we are
        // done. `run_io` is responsible for pumping audio out and transcripts
        // back until the socket breaks or the caller sends `AudioCmd::Stop`.
        let disconnect = run_io(
            &mut writer,
            &mut reader,
            &mut audio_rx,
            &event_tx,
            &resumption_handle,
            &user_disconnected,
            config.content_egress_policy,
        )
        .await;

        // Any fresh disconnect resets the "connected" flag so the rest of
        // the app knows we're recovering. `send_audio` tolerates this.
        connected.store(false, Ordering::SeqCst);

        match disconnect {
            DisconnectKind::UserRequested | DisconnectKind::WriterEnded => {
                // Clean end — the user asked to stop, or we ran out of audio
                // commands because the client was dropped. Do not reconnect.
                log::info!("Gemini session: ending ({disconnect:?})");
                let _ = event_tx.send(GeminiEvent::Disconnected);
                break;
            }
            _ => {
                // Network-ish failure. If the user *also* asked to disconnect
                // (e.g. they hit stop just as the socket was dying), honour
                // that and skip the reconnect dance.
                if user_disconnected.load(Ordering::SeqCst) {
                    let _ = event_tx.send(GeminiEvent::Disconnected);
                    break;
                }

                log::warn!("Gemini session: disconnected — {disconnect:?}");
                let _ = event_tx.send(GeminiEvent::Disconnected);

                reconnect_attempts += 1;
                let Some(backoff) = backoff_for_attempt(reconnect_attempts) else {
                    // Budget exhausted — surface a closed fatal error and stop.
                    // Issue #145: the event `message` is a stable site token,
                    // and the diagnostic envelope carries the closed
                    // classification; no reconnect prose crosses.
                    log::warn!(
                        "Gemini session: reconnect budget exhausted ({reconnect_attempts} attempts)"
                    );
                    let _ = event_tx.send(GeminiEvent::error(
                        GeminiErrorSite::ReconnectExhausted,
                        GeminiErrorCategory::Network,
                        GeminiErrorCategory::Network.runtime_diagnostic(),
                    ));
                    break;
                };

                log::info!(
                    "Gemini session: reconnecting (attempt {reconnect_attempts}, backoff {backoff}s)"
                );
                let _ = event_tx.send(GeminiEvent::Reconnecting {
                    attempt: reconnect_attempts,
                    backoff_secs: backoff,
                });

                // Sleep for the backoff window, but bail out early if the
                // user cancels during the wait.
                let sleep = tokio::time::sleep(Duration::from_secs(backoff));
                tokio::pin!(sleep);
                loop {
                    tokio::select! {
                        _ = &mut sleep => break,
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {
                            if user_disconnected.load(Ordering::SeqCst) {
                                log::info!("Gemini session: user cancelled during backoff");
                                let _ = event_tx.send(GeminiEvent::Disconnected);
                                return;
                            }
                        }
                    }
                }

                // Snapshot the latest resumption handle under lock, then
                // drop the guard before awaiting the (potentially slow)
                // reconnect — the reader side may need to grab the same
                // mutex if a `sessionResumptionUpdate` arrives mid-flight
                // on a future socket.
                let handle_snapshot = resumption_handle.lock().ok().and_then(|g| g.clone());
                if handle_snapshot.is_some() {
                    log::info!("Gemini session: reconnecting with resumption handle");
                } else {
                    log::info!(
                        "Gemini session: reconnecting without resumption handle (new session)"
                    );
                }

                // Attempt the reconnect. Unlike Deepgram, this also replays
                // the `BidiGenerateContentSetup` frame and waits for a fresh
                // `setupComplete` — all hidden inside `open_ws`. If a
                // resumption handle is available, it is sent in the setup
                // payload so the server restores the prior session state.
                let resumed = handle_snapshot.is_some();
                match open_ws(&config, handle_snapshot.as_deref()).await {
                    Ok((new_writer, new_reader)) => {
                        writer = new_writer;
                        reader = new_reader;
                        connected.store(true, Ordering::SeqCst);
                        log::info!(
                            "Gemini session: reconnected on attempt {reconnect_attempts} (resumed={resumed})"
                        );
                        let _ = event_tx.send(GeminiEvent::Reconnected { resumed });
                        reconnect_attempts = 0;
                        // Loop around to resume run_io with the new halves.
                    }
                    Err(e) => {
                        // Issue #145: log + emit only the closed category and
                        // diagnostic envelope; the event `message` is a
                        // stable site token, never the connect error text.
                        log::warn!(
                            "Gemini session: reconnect attempt {reconnect_attempts} failed ({:?})",
                            e.category,
                        );
                        let _ = event_tx.send(GeminiEvent::error(
                            GeminiErrorSite::ReconnectFailed,
                            e.category.clone(),
                            e.diagnostic.clone(),
                        ));
                        // Skip run_io next iteration — just try the next
                        // backoff step directly.
                        continue;
                    }
                }
            }
        }
    }

    connected.store(false, Ordering::SeqCst);
    log::info!("Gemini: session task exited");
}

/// Pumps audio out and server events back for a single WebSocket instance.
///
/// Returns the classified [`DisconnectKind`] when the socket breaks or the
/// caller asks to stop. The [`session_task`] above turns that into either a
/// reconnect or a clean exit.
#[allow(clippy::too_many_arguments)]
async fn run_io(
    writer: &mut WsWriter,
    reader: &mut WsReader,
    audio_rx: &mut tokio_mpsc::Receiver<AudioCmd>,
    event_tx: &crossbeam_channel::Sender<GeminiEvent>,
    resumption_handle: &Arc<std::sync::Mutex<Option<String>>>,
    user_disconnected: &Arc<AtomicBool>,
    content_egress_policy: crate::asr::ProviderContentEgressPolicy,
) -> DisconnectKind {
    loop {
        tokio::select! {
            // Writer side: audio command from the caller.
            cmd = audio_rx.recv() => {
                match cmd {
                    Some(AudioCmd::Chunk(b64)) => {
                        // Defense-in-depth content-egress gate (second layer).
                        // `send_audio` already refuses to enqueue a Chunk in a
                        // blocked privacy mode; re-checking here means a direct
                        // caller that drives `run_io` (or feeds `audio_rx`)
                        // bypassing `send_audio` still cannot ship audio bytes
                        // to Gemini. The policy error is redacted (no audio
                        // payload). We drop the frame WITHOUT tearing down the
                        // socket — a blocked policy is a steady-state condition,
                        // not a transport failure to reconnect around.
                        if content_egress_policy.check_audio("gemini.live").is_err() {
                            continue;
                        }
                        let payload = json!({
                            "realtimeInput": {
                                "audio": {
                                    "data": b64,
                                    "encoding": "LINEAR16",
                                    "sampleRateHertz": 16000
                                }
                            }
                        });

                        if let Err(e) = writer
                            .send(Message::Text(payload.to_string().into()))
                            .await
                        {
                            // Issue #145: classify structurally; drop the
                            // transport text from logs and the disconnect
                            // payload.
                            let category = classify_tungstenite_error(&e);
                            log::warn!("Gemini: audio send failed ({category:?})");
                            return DisconnectKind::NetworkError(category.runtime_diagnostic());
                        }
                    }
                    Some(AudioCmd::EndTurn) => {
                        // Per-turn end-of-user-input (B18): send audioStreamEnd
                        // so the model starts generating, but KEEP the socket
                        // open for the assistant reply + the next turn.
                        if let Err(e) = writer
                            .send(Message::Text(audio_stream_end_frame().into()))
                            .await
                        {
                            // Issue #145: classify structurally; drop the text.
                            let category = classify_tungstenite_error(&e);
                            log::warn!("Gemini: end-turn send failed ({category:?})");
                            return DisconnectKind::NetworkError(category.runtime_diagnostic());
                        }
                    }
                    Some(AudioCmd::Stop) => {
                        // Graceful user-initiated close.
                        let _ = writer
                            .send(Message::Text(audio_stream_end_frame().into()))
                            .await;
                        let _ = writer.close().await;
                        return DisconnectKind::UserRequested;
                    }
                    None => {
                        // Caller dropped the sender. No more audio will ever
                        // arrive — end without reconnecting.
                        let _ = writer.close().await;
                        return DisconnectKind::WriterEnded;
                    }
                }
            }

            // Reader side: inbound frame from Gemini.
            result = reader.next() => {
                let Some(result) = result else {
                    // Reader stream ended without a Close frame — treat as a
                    // network-layer drop.
                    return DisconnectKind::NetworkError(
                        GeminiErrorCategory::Network.runtime_diagnostic(),
                    );
                };

                match result {
                    Ok(Message::Text(text)) => {
                        handle_server_message(&text, event_tx, resumption_handle);
                    }
                    Ok(Message::Close(frame)) => {
                        if user_disconnected.load(Ordering::SeqCst) {
                            return DisconnectKind::UserRequested;
                        }
                        // Classify the close frame (if any) and emit a
                        // categorized error event so the frontend can show
                        // an auth / quota / server toast before the
                        // reconnect loop kicks in. `classify_close_frame`
                        // returns None for code 1000 / unclassified codes —
                        // in that case the generic Disconnected signal
                        // carries no toast. Issue #145: the close *reason
                        // string* is inspected for classification only — it
                        // is never retained, logged, or serialized; the
                        // emitted event carries the closed site token +
                        // diagnostic envelope instead.
                        let category = frame.as_ref().and_then(|f| {
                            let code: u16 = f.code.into();
                            classify_close_frame(code, f.reason.as_ref())
                        });
                        match category {
                            Some(category) => {
                                log::warn!("Gemini: server closed connection ({category:?})");
                                let _ = event_tx.send(GeminiEvent::error(
                                    GeminiErrorSite::ServerClose,
                                    category.clone(),
                                    close_frame_diagnostic(&category),
                                ));
                                return DisconnectKind::ServerClose {
                                    category: Some(category),
                                    diagnostic: GeminiErrorCategory::Network.runtime_diagnostic(),
                                };
                            }
                            None => {
                                log::info!("Gemini: server closed connection (uncategorized)");
                                return DisconnectKind::ServerClose {
                                    category: None,
                                    diagnostic: GeminiErrorCategory::Network.runtime_diagnostic(),
                                };
                            }
                        }
                    }
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {
                        // Protocol-level frames; nothing to do.
                    }
                    Ok(Message::Binary(_)) => {
                        // TEXT modality only; binary is unexpected.
                        log::warn!("Gemini: unexpected binary message");
                    }
                    Err(tungstenite::Error::ConnectionClosed)
                    | Err(tungstenite::Error::AlreadyClosed) => {
                        return DisconnectKind::NetworkError(
                            GeminiErrorCategory::Network.runtime_diagnostic(),
                        );
                    }
                    Err(tungstenite::Error::Protocol(_)) => {
                        return DisconnectKind::ProtocolError(
                            GeminiErrorCategory::Unknown.runtime_diagnostic(),
                        );
                    }
                    Err(e) => {
                        // Issue #145: classify structurally; drop the
                        // transport error text from logs + the payload.
                        let category = classify_tungstenite_error(&e);
                        log::warn!("Gemini: WebSocket read error ({category:?})");
                        return DisconnectKind::NetworkError(category.runtime_diagnostic());
                    }
                }
            }
        }
    }
}

/// Parse a single server JSON message and emit appropriate events.
fn handle_server_message(
    text: &str,
    tx: &crossbeam_channel::Sender<GeminiEvent>,
    resumption_handle: &Arc<std::sync::Mutex<Option<String>>>,
) {
    let parsed: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            // Issue #145: classify structurally (unparseable frame → closed
            // internal code); the serde error text (which quotes the raw
            // frame) is dropped here and never logged.
            log::warn!("Gemini Live: invalid server JSON");
            let _ = tx.send(GeminiEvent::error(
                GeminiErrorSite::InvalidJson,
                GeminiErrorCategory::Unknown,
                GeminiErrorCategory::Unknown.runtime_diagnostic(),
            ));
            return;
        }
    };

    // ── usageMetadata ───────────────────────────────────────────────────
    // Per the spec, `usageMetadata` is a top-level sibling of `serverContent`.
    // It typically travels alongside the frame that ends a turn, so parse it
    // up front and thread it into any `TurnComplete` emitted below.
    let usage = parsed
        .get("usageMetadata")
        .and_then(|v| serde_json::from_value::<UsageMetadata>(v.clone()).ok());

    // ── serverContent envelope ──────────────────────────────────────────
    if let Some(server_content) = parsed.get("serverContent") {
        // --- inputTranscription ────────────────────────────────────────
        if let Some(transcript) = server_content.get("inputTranscription")
            && let Some(text_val) = transcript.get("text").and_then(|t| t.as_str())
            && !text_val.is_empty()
        {
            let is_final = transcript
                .get("completed")
                .and_then(|c| c.as_bool())
                .unwrap_or(false);
            let _ = tx.send(GeminiEvent::Transcription {
                text: text_val.to_string(),
                is_final,
            });
        }

        // --- outputTranscription ───────────────────────────────────────
        // AUDIO mode (research §1.4): the spoken reply mirrored as text so the
        // graph still gets proposals. Emitted alongside the audio chunks.
        if let Some(out) = server_content.get("outputTranscription")
            && let Some(text_val) = out.get("text").and_then(|t| t.as_str())
            && !text_val.is_empty()
        {
            let _ = tx.send(GeminiEvent::OutputTranscription {
                text: text_val.to_string(),
            });
        }

        // --- modelTurn ─────────────────────────────────────────────────
        if let Some(model_turn) = server_content.get("modelTurn")
            && let Some(parts) = model_turn.get("parts").and_then(|p| p.as_array())
        {
            for part in parts {
                if let Some(text_val) = part.get("text").and_then(|t| t.as_str())
                    && !text_val.is_empty()
                {
                    let _ = tx.send(GeminiEvent::ModelResponse {
                        text: text_val.to_string(),
                    });
                }
                // AUDIO mode (research §1.3): native speech arrives as
                // base64 PCM16 LE @ 24 kHz in `inlineData.data`. Forward the
                // base64 string verbatim (the consumer decodes at the point of
                // use) — see the AudioChunk doc for why we don't carry Vec<u8>.
                if let Some(inline) = part.get("inlineData")
                    && let Some(b64) = inline.get("data").and_then(|d| d.as_str())
                    && !b64.is_empty()
                {
                    let _ = tx.send(GeminiEvent::AudioChunk {
                        data_base64: b64.to_string(),
                        sample_rate: GEMINI_OUTPUT_SAMPLE_RATE,
                    });
                }
            }
        }

        // --- interrupted (barge-in) ────────────────────────────────────
        // Server auto-fires on VAD; it has already canceled the in-flight
        // generation server-side. The client must flush locally-buffered
        // audio (research §1.4). Surface as a distinct event for the FSM.
        if server_content
            .get("interrupted")
            .and_then(|i| i.as_bool())
            .unwrap_or(false)
        {
            let _ = tx.send(GeminiEvent::Interrupted);
        }

        // --- generationComplete ────────────────────────────────────────
        // Precedes `turnComplete` (research §1.4). Turn bookkeeping only.
        if server_content
            .get("generationComplete")
            .and_then(|g| g.as_bool())
            .unwrap_or(false)
        {
            let _ = tx.send(GeminiEvent::GenerationComplete);
        }

        // --- turnComplete ──────────────────────────────────────────────
        if server_content.get("turnComplete").is_some() {
            // Issue #145: no token counts in diagnostic logs; usage rides the
            // `TurnComplete` data event below.
            let _ = tx.send(GeminiEvent::TurnComplete {
                usage: usage.clone(),
            });
        }

        return;
    }

    // ── standalone usageMetadata frame ─────────────────────────────────
    // The server occasionally ships usage without a `serverContent` envelope
    // (e.g. a billing roll-up at the end of a long turn). Surface it as a
    // `TurnComplete` with usage populated — same path the frontend already
    // listens on for per-turn accounting.
    if let Some(u) = usage {
        // Issue #145: no token counts in diagnostic logs; usage rides the
        // `TurnComplete` data event below.
        log::debug!("Gemini Live: standalone usage frame");
        let _ = tx.send(GeminiEvent::TurnComplete { usage: Some(u) });
        return;
    }

    // ── goAway ─────────────────────────────────────────────────────────
    if parsed.get("goAway").is_some() {
        log::warn!("Gemini Live: received goAway — server is shutting down");
        // Issue #145: site token + closed diagnostic, no reconnect prose.
        let _ = tx.send(GeminiEvent::error(
            GeminiErrorSite::GoAway,
            GeminiErrorCategory::Server,
            GeminiErrorCategory::Server.runtime_diagnostic(),
        ));
        return;
    }

    // ── sessionResumptionUpdate ────────────────────────────────────────
    // Per <https://ai.google.dev/api/live#session-management>: the server
    // sends these periodically once resumption is enabled in setup. We only
    // cache `newHandle` when `resumable == true`; otherwise the handle is
    // not valid for reconnect (e.g. mid-generation or during a function
    // call) and keeping a stale value would trigger a server-side reject
    // on the next reconnect.
    if let Some(update) = parsed.get("sessionResumptionUpdate") {
        let resumable = update
            .get("resumable")
            .and_then(|r| r.as_bool())
            .unwrap_or(false);
        let new_handle = update
            .get("newHandle")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty());

        if resumable {
            if let Some(handle) = new_handle {
                if let Ok(mut guard) = resumption_handle.lock() {
                    *guard = Some(handle.to_string());
                }
                log::debug!("Gemini Live: session resumption handle refreshed");
            }
        } else {
            log::debug!("Gemini Live: sessionResumptionUpdate with resumable=false");
        }
        return;
    }

    // ── Unknown ────────────────────────────────────────────────────────
    log::debug!(
        "Gemini Live: unhandled message {}",
        gemini_frame_diagnostic(&parsed)
    );
}

/// Closed frame summary for debug logs (issue #145): only the closed
/// top-level frame type token and a bounded field-count bucket — no field
/// values, request ids, or payload lengths.
fn gemini_frame_diagnostic(parsed: &Value) -> String {
    let top_level_type = if parsed.get("serverContent").is_some() {
        "serverContent"
    } else if parsed.get("usageMetadata").is_some() {
        "usageMetadata"
    } else if parsed.get("goAway").is_some() {
        "goAway"
    } else if parsed.get("sessionResumptionUpdate").is_some() {
        "sessionResumptionUpdate"
    } else if parsed.get("setupComplete").is_some() {
        "setupComplete"
    } else {
        "unknown"
    };
    let fields_bucket = match json_field_count(parsed) {
        0 => "zero",
        1..=8 => "few",
        _ => "many",
    };
    format!("type={top_level_type} fields={fields_bucket}")
}

fn json_field_count(parsed: &Value) -> usize {
    parsed.as_object().map_or(0, serde_json::Map::len)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert f32 PCM samples (range −1.0 … +1.0) to little-endian i16 bytes.
fn f32_to_i16_le_bytes(samples: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(samples.len() * 2);
    for &s in samples {
        let clamped = s.clamp(-1.0, 1.0);
        let val = if clamped >= 0.0 {
            (clamped * i16::MAX as f32) as i16
        } else {
            (clamped * -(i16::MIN as f32)) as i16
        };
        bytes.extend_from_slice(&val.to_le_bytes());
    }
    bytes
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn blocked_gemini_config() -> GeminiConfig {
        let mut config = GeminiConfig::text(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "gemini-private-api-key".into(),
            },
            "model",
        );
        config.content_egress_policy = crate::asr::ProviderContentEgressPolicy::block("local_only");
        config
    }

    fn allowed_gemini_config() -> GeminiConfig {
        let mut config = GeminiConfig::text(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "key".into(),
            },
            "model",
        );
        config.content_egress_policy = crate::asr::ProviderContentEgressPolicy::allow();
        config
    }

    /// Regression for audio-graph-7086 / review B2: drive the PRODUCTION
    /// connect path — the REAL per-auth-branch header assembly
    /// (`gemini_api_key_connect_headers` / `gemini_vertex_connect_headers`,
    /// the exact functions `open_ws` consumes) fed through `connect_gemini_ws`
    /// — against a fixture listener with an obviously-fake credential. Before
    /// the fix, both branches hand-built an `http::Request` carrying only
    /// their auth + content-type headers, so `generate_request` failed with
    /// `Protocol(InvalidHeader("sec-websocket-key"))` before any TCP and
    /// Gemini Live could never connect. The handshake succeeding here proves
    /// the five mandatory WS headers now reach the wire, the captured request
    /// confirms the auth header is present with the exact production value,
    /// and the credential never appears in the URL (the header-not-query
    /// security invariant at the connect site). Because the test consumes the
    /// same header builders as `open_ws`, a typo'd or dropped header in the
    /// production assembly fails this test — which is exactly how the
    /// original B2 defect escaped: the old test duplicated the headers by
    /// hand instead of exercising them.
    async fn assert_gemini_connect_handshake(
        headers: Vec<(
            tungstenite::http::HeaderName,
            tungstenite::http::HeaderValue,
        )>,
        secret: &str,
        expected_auth: (&str, &str),
    ) {
        // Missing mandatory upgrade headers would abort this handshake; the
        // test would then fail at the client `connect_gemini_ws` call.
        let (addr, server) =
            crate::ws_request::test_support::spawn_header_capturing_ws_server().await;

        let url = format!("ws://{addr}/ws/gemini.BidiGenerateContent");
        let (mut writer, _reader) = connect_gemini_ws(&url, headers)
            .await
            .expect("production Gemini connect must handshake against the fixture");
        writer.close().await.expect("close client socket");

        let (captured_uri, captured_headers) = tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("server task finishes")
            .expect("server task panicked");

        for mandatory in [
            "host",
            "connection",
            "upgrade",
            "sec-websocket-version",
            "sec-websocket-key",
        ] {
            assert!(
                captured_headers.iter().any(|(name, _)| name == mandatory),
                "production handshake missing mandatory `{mandatory}` header: {captured_headers:?}"
            );
        }
        let (auth_name, auth_value) = expected_auth;
        assert!(
            captured_headers
                .iter()
                .any(|(name, value)| name == auth_name && value == auth_value),
            "production handshake must carry the `{auth_name}` header: {captured_headers:?}"
        );
        assert!(
            captured_headers
                .iter()
                .any(|(name, value)| name == "content-type" && value == "application/json"),
            "production handshake must carry the content-type header: {captured_headers:?}"
        );
        assert!(
            !captured_uri.contains(secret),
            "the credential must never appear in the request URI: {captured_uri}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_ws_api_key_production_headers_handshake_with_mandatory_headers() {
        let api_key = "test-key-not-real";
        let headers = gemini_api_key_connect_headers(api_key)
            .expect("production ApiKey header assembly builds");
        assert_gemini_connect_handshake(headers, api_key, ("x-goog-api-key", api_key)).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_ws_vertex_production_headers_handshake_with_mandatory_headers() {
        let token = "test-token-not-real";
        let headers =
            gemini_vertex_connect_headers(token).expect("production Vertex header assembly builds");
        assert_gemini_connect_handshake(
            headers,
            token,
            ("authorization", "Bearer test-token-not-real"),
        )
        .await;
    }

    #[test]
    fn gemini_config_debug_redacts_auth_secret() {
        let config = GeminiConfig::audio(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "gemini-debug-secret-sentinel".into(),
            },
            "gemini-live",
            "Kore",
        );

        let debug = format!("{config:?}");

        assert!(!debug.contains("gemini-debug-secret-sentinel"));
        assert!(debug.contains("<present>"));
        assert!(debug.contains("gemini-live"));
        assert!(debug.contains("Kore"));
    }

    #[test]
    fn gemini_content_policy_defaults_to_explicit_policy_required() {
        let text_config = GeminiConfig::text(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "key".into(),
            },
            "model",
        );
        let audio_config = GeminiConfig::audio(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "key".into(),
            },
            "model",
            "Kore",
        );

        for config in [text_config, audio_config] {
            let error = config
                .content_egress_policy
                .check_audio("gemini.live")
                .unwrap_err();
            assert!(error.contains("Privacy policy blocked audio egress"));
            assert!(error.contains("gemini.live"));
            assert!(error.contains(EXPLICIT_POLICY_REQUIRED));
        }
    }

    #[test]
    fn f32_to_i16_conversion_silence() {
        let silence = [0.0f32; 4];
        let bytes = f32_to_i16_le_bytes(&silence);
        assert_eq!(bytes.len(), 8);
        assert!(bytes.iter().all(|&b| b == 0));
    }

    #[test]
    fn audio_stream_end_frame_is_realtime_input_stream_end() {
        // B18: EndUserTurn + teardown both send exactly this frame. Lock its
        // shape so a refactor can't silently change the per-turn boundary the
        // server keys generation off of.
        let v: Value = serde_json::from_str(&audio_stream_end_frame()).unwrap();
        assert_eq!(
            v["realtimeInput"]["audioStreamEnd"],
            serde_json::json!(true)
        );
        // It must NOT carry an audio payload (that would be a chunk, not an end).
        assert!(v["realtimeInput"]["audio"].is_null());
    }

    #[test]
    fn f32_to_i16_conversion_full_scale() {
        let samples = [1.0f32, -1.0];
        let bytes = f32_to_i16_le_bytes(&samples);
        assert_eq!(&bytes[0..2], &i16::MAX.to_le_bytes());
        assert_eq!(&bytes[2..4], &i16::MIN.to_le_bytes());
    }

    #[test]
    fn f32_to_i16_clamps() {
        let samples = [2.0f32, -3.0];
        let bytes = f32_to_i16_le_bytes(&samples);
        assert_eq!(&bytes[0..2], &i16::MAX.to_le_bytes());
        assert_eq!(&bytes[2..4], &i16::MIN.to_le_bytes());
    }

    #[test]
    fn setup_message_structure_api_key() {
        let config = GeminiConfig::text(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "test-key".into(),
            },
            "gemini-3.1-flash-live-preview",
        );
        let msg = build_setup_message(&config, None);

        assert_eq!(
            msg["setup"]["model"],
            "models/gemini-3.1-flash-live-preview"
        );
        assert_eq!(
            msg["setup"]["generationConfig"]["responseModalities"][0],
            "TEXT"
        );
        assert!(msg["setup"]["generationConfig"]["inputAudioTranscription"].is_object());
        // TEXT (notes) mode must NOT request audio voice / output transcription.
        assert!(msg["setup"]["generationConfig"]["speechConfig"].is_null());
        assert!(msg["setup"]["generationConfig"]["outputAudioTranscription"].is_null());
        // First connect sends empty sessionResumption so the server enables
        // updates.
        assert!(msg["setup"]["sessionResumption"].is_object());
        assert!(msg["setup"]["sessionResumption"]["handle"].is_null());
    }

    #[test]
    fn setup_message_structure_vertex_ai() {
        let config = GeminiConfig::text(
            crate::settings::GeminiAuthMode::VertexAI {
                project_id: "my-project".into(),
                location: "us-central1".into(),
                service_account_path: None,
            },
            "gemini-3.1-flash-live-preview",
        );
        let msg = build_setup_message(&config, None);

        assert_eq!(
            msg["setup"]["model"],
            "projects/my-project/locations/us-central1/publishers/google/models/gemini-3.1-flash-live-preview"
        );
    }

    #[test]
    fn setup_message_audio_modality_emits_voice_and_output_transcription() {
        // Converse-mode (ADR-0018): AUDIO XOR TEXT, plus a prebuilt voice and
        // outputAudioTranscription so the graph still gets text. Default voice.
        let config = GeminiConfig::audio(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "k".into(),
            },
            "gemini-3.1-flash-live-preview",
            "", // empty → DEFAULT_GEMINI_VOICE
        );
        let msg = build_setup_message(&config, None);
        let gc = &msg["setup"]["generationConfig"];

        assert_eq!(gc["responseModalities"][0], "AUDIO");
        // Exactly one modality — never both.
        assert!(gc["responseModalities"][1].is_null());
        assert_eq!(
            gc["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]["voiceName"],
            DEFAULT_GEMINI_VOICE
        );
        // Graph still gets text in AUDIO mode.
        assert!(gc["outputAudioTranscription"].is_object());
        // User speech still feeds the graph.
        assert!(gc["inputAudioTranscription"].is_object());
    }

    #[test]
    fn setup_message_audio_respects_custom_voice() {
        let config = GeminiConfig::audio(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "k".into(),
            },
            "gemini-3.1-flash-live-preview",
            "Puck",
        );
        let msg = build_setup_message(&config, None);
        assert_eq!(
            msg["setup"]["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]
                ["voiceName"],
            "Puck"
        );
    }

    #[test]
    fn audio_config_blank_voice_falls_back_to_default() {
        let cfg = GeminiConfig::audio(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "k".into(),
            },
            "m",
            "   ",
        );
        assert_eq!(cfg.voice_name, DEFAULT_GEMINI_VOICE);
        assert_eq!(cfg.response_modality, ResponseModality::Audio);
    }

    #[test]
    fn text_config_is_default_modality() {
        let cfg = GeminiConfig::text(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "k".into(),
            },
            "m",
        );
        assert_eq!(cfg.response_modality, ResponseModality::Text);
        assert_eq!(ResponseModality::default(), ResponseModality::Text);
    }

    #[test]
    fn setup_message_includes_resumption_handle_on_reconnect() {
        let config = GeminiConfig::text(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "k".into(),
            },
            "gemini-3.1-flash-live-preview",
        );
        let msg = build_setup_message(&config, Some("opaque-handle-xyz"));

        assert_eq!(
            msg["setup"]["sessionResumption"]["handle"],
            "opaque-handle-xyz"
        );
    }

    #[test]
    fn setup_message_omits_handle_when_none() {
        let config = GeminiConfig::text(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "k".into(),
            },
            "m",
        );
        let msg = build_setup_message(&config, None);

        // Must still include sessionResumption so server enables updates,
        // but the `handle` key itself must be absent (server treats "handle
        // present but empty" as invalid).
        let sr = &msg["setup"]["sessionResumption"];
        assert!(sr.is_object(), "sessionResumption must be present");
        assert!(
            sr.get("handle").is_none(),
            "handle must be absent for a fresh session, got {sr:?}"
        );
    }

    #[test]
    fn event_serialization_roundtrip() {
        let events = vec![
            GeminiEvent::Transcription {
                text: "hello".into(),
                is_final: true,
            },
            GeminiEvent::ModelResponse {
                text: "world".into(),
            },
            GeminiEvent::TurnComplete { usage: None },
            GeminiEvent::Error {
                category: GeminiErrorCategory::Unknown,
                message: "invalid_json".into(),
                diagnostic: GeminiErrorCategory::Unknown.runtime_diagnostic(),
            },
            GeminiEvent::Error {
                category: GeminiErrorCategory::Auth,
                message: "server_close".into(),
                diagnostic: GeminiErrorCategory::Auth.runtime_diagnostic(),
            },
            GeminiEvent::Error {
                category: GeminiErrorCategory::AuthExpired,
                message: "server_close".into(),
                diagnostic: GeminiErrorCategory::AuthExpired.runtime_diagnostic(),
            },
            GeminiEvent::Error {
                category: GeminiErrorCategory::Network,
                message: "reconnect_failed".into(),
                diagnostic: GeminiErrorCategory::Network.runtime_diagnostic(),
            },
            GeminiEvent::Error {
                category: GeminiErrorCategory::RateLimit {
                    retry_delay: Some(RuntimeRetryDelayBucket::Medium),
                },
                message: "server_close".into(),
                diagnostic: GeminiErrorCategory::RateLimit {
                    retry_delay: Some(RuntimeRetryDelayBucket::Medium),
                }
                .runtime_diagnostic(),
            },
            GeminiEvent::Error {
                category: GeminiErrorCategory::Server,
                message: "go_away".into(),
                diagnostic: GeminiErrorCategory::Server.runtime_diagnostic(),
            },
            GeminiEvent::Connected,
            GeminiEvent::Disconnected,
            GeminiEvent::Reconnecting {
                attempt: 2,
                backoff_secs: 2,
            },
            GeminiEvent::Reconnected { resumed: true },
            GeminiEvent::Reconnected { resumed: false },
            GeminiEvent::AudioChunk {
                data_base64: "AAH/fw==".into(), // base64 of [0x00,0x01,0xff,0x7f]
                sample_rate: GEMINI_OUTPUT_SAMPLE_RATE,
            },
            GeminiEvent::Interrupted,
            GeminiEvent::OutputTranscription {
                text: "spoken reply".into(),
            },
            GeminiEvent::GenerationComplete,
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let reparsed: GeminiEvent = serde_json::from_str(&json).unwrap();
            // Lossless round-trip (catches a malformed tag / field rename).
            assert_eq!(
                serde_json::to_string(&reparsed).unwrap(),
                json,
                "event did not round-trip: {json}"
            );
        }
    }

    #[test]
    fn backoff_schedule_matches_spec() {
        // Gemini shares the crate-level reconnect ladder (review n2); this pins
        // that it sees the fast head + cold-restart tail (review m1) the session
        // task relies on.
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), Some(20));
        assert_eq!(backoff_for_attempt(11), None);
    }

    async fn recv_event(
        rx: &crossbeam_channel::Receiver<GeminiEvent>,
        timeout: Duration,
    ) -> GeminiEvent {
        tokio::time::timeout(timeout, async {
            loop {
                if let Ok(event) = rx.try_recv() {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for Gemini event")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_task_cancels_during_reconnect_backoff() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local websocket server");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept websocket");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("server websocket handshake");
            let _ = websocket.close(None).await;
        });

        let (client_socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .expect("client websocket connect");
        let (writer, reader) = client_socket.split();
        let (_audio_tx, audio_rx) = tokio_mpsc::channel(8);
        let (event_tx, event_rx) = crossbeam_channel::bounded(16);
        let connected = Arc::new(AtomicBool::new(true));
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let resumption_handle = Arc::new(std::sync::Mutex::new(None));

        let task_connected = connected.clone();
        let task_user_disconnected = user_disconnected.clone();
        let task_resumption_handle = resumption_handle.clone();
        let handle = tokio::spawn(session_task(
            writer,
            reader,
            audio_rx,
            GeminiConfig::text(
                crate::settings::GeminiAuthMode::ApiKey {
                    api_key: "test-key".into(),
                },
                "gemini-3.1-flash-live-preview",
            ),
            event_tx,
            task_connected,
            task_user_disconnected,
            task_resumption_handle,
        ));

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            GeminiEvent::Disconnected => {}
            other => panic!("expected initial Disconnected event, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            GeminiEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                assert_eq!(attempt, 1);
                assert_eq!(backoff_secs, 1);
            }
            other => panic!("expected Reconnecting event, got {other:?}"),
        }

        user_disconnected.store(true, Ordering::SeqCst);

        match recv_event(&event_rx, Duration::from_secs(2)).await {
            GeminiEvent::Disconnected => {}
            other => panic!("expected cancel Disconnected event, got {other:?}"),
        }
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("session task should exit before reconnect backoff completes")
            .expect("session task panicked");
        assert!(
            !connected.load(Ordering::SeqCst),
            "cancelled reconnect must leave connected=false"
        );
        assert!(
            event_rx
                .try_iter()
                .all(|event| !matches!(event, GeminiEvent::Reconnected { .. })),
            "cancel during backoff must not emit Reconnected"
        );

        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;
    }

    /// Defense-in-depth: drive `run_io` directly with a blocked content-egress
    /// policy and feed an audio Chunk through the channel. The writer half must
    /// refuse to ship the audio data frame even though the Chunk reached
    /// `run_io` without passing through `send_audio` (which already gates
    /// enqueue). The terminal `audioStreamEnd` control frame (sent on Stop)
    /// carries no audio content and is allowed — we assert specifically that
    /// the audio DATA payload (and the secret bytes) never reach the socket.
    #[tokio::test(flavor = "current_thread")]
    async fn run_io_blocked_policy_writes_no_audio_frame() {
        const SECRET_AUDIO_B64: &str = "U0VDUkVUX0FVRElPX0JZVEVT";

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local websocket server");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept websocket");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("server websocket handshake");
            // Collect every text frame the writer emits in the window so we can
            // distinguish an audio DATA frame from the legitimate terminal
            // `audioStreamEnd` control frame.
            let mut frames: Vec<String> = Vec::new();
            while let Ok(Some(Ok(message))) =
                tokio::time::timeout(Duration::from_millis(200), websocket.next()).await
            {
                if let Message::Text(text) = message {
                    frames.push(text.to_string());
                }
            }
            let _ = websocket.close(None).await;
            frames
        });

        let (client_socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .expect("client websocket connect");
        let (mut writer, mut reader) = client_socket.split();
        let (audio_tx, mut audio_rx) = tokio_mpsc::channel(8);
        let (event_tx, _event_rx) = crossbeam_channel::bounded(16);
        let resumption_handle = Arc::new(std::sync::Mutex::new(None));
        let user_disconnected = Arc::new(AtomicBool::new(false));

        // Pre-load a Chunk carrying payload-like base64 so we can assert the
        // bytes never reach the socket under a blocked policy.
        audio_tx
            .send(AudioCmd::Chunk(SECRET_AUDIO_B64.into()))
            .await
            .expect("queue chunk");
        // Then a Stop so `run_io` returns deterministically after handling the
        // (blocked) Chunk.
        audio_tx.send(AudioCmd::Stop).await.expect("queue stop");
        drop(audio_tx);

        let disconnect = run_io(
            &mut writer,
            &mut reader,
            &mut audio_rx,
            &event_tx,
            &resumption_handle,
            &user_disconnected,
            crate::asr::ProviderContentEgressPolicy::block("local_only"),
        )
        .await;

        assert!(
            matches!(disconnect, DisconnectKind::UserRequested),
            "run_io should end via the Stop command, got {disconnect:?}"
        );

        let frames = tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server timeout")
            .expect("server task");

        // The blocked Chunk must NOT have produced an audio data frame, and the
        // secret audio bytes must never appear on the wire. The only frame the
        // server may legitimately see is the terminal `audioStreamEnd`.
        for frame in &frames {
            assert!(
                !frame.contains(SECRET_AUDIO_B64),
                "blocked policy leaked audio bytes to the socket: {frame}"
            );
            assert!(
                !frame.contains("\"data\""),
                "blocked policy wrote an audio data frame to the socket: {frame}"
            );
            assert!(
                frame.contains("audioStreamEnd"),
                "only the terminal audioStreamEnd control frame is allowed, got: {frame}"
            );
        }
    }

    #[test]
    fn client_new_is_disconnected() {
        let client = GeminiLiveClient::new(GeminiConfig::text(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "key".into(),
            },
            "model",
        ));
        assert!(!client.is_connected());
    }

    #[test]
    fn connect_fails_without_api_key() {
        let mut client = GeminiLiveClient::new(GeminiConfig::text(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: String::new(),
            },
            "model",
        ));
        let result = client.connect();
        assert!(result.is_err());
        // Issue #145: the failure is the closed `invalid_configuration`
        // diagnostic — never a prose string.
        assert!(matches!(
            result.unwrap_err(),
            crate::error::AppError::RuntimeDiagnostic(
                audio_graph_ipc_contract::runtime_diagnostic::RuntimeDiagnostic::Runtime(
                    audio_graph_ipc_contract::runtime_diagnostic::RuntimeErrorDiagnostic {
                        code: audio_graph_ipc_contract::runtime_diagnostic::RuntimeErrorCode::InvalidConfiguration,
                        ..
                    }
                )
            )
        ));
    }

    #[test]
    fn connect_fails_without_vertex_config() {
        let mut client = GeminiLiveClient::new(GeminiConfig::text(
            crate::settings::GeminiAuthMode::VertexAI {
                project_id: String::new(),
                location: String::new(),
                service_account_path: None,
            },
            "model",
        ));
        let result = client.connect();
        assert!(result.is_err());
        // Issue #145: same closed `invalid_configuration` diagnostic.
        assert!(matches!(
            result.unwrap_err(),
            crate::error::AppError::RuntimeDiagnostic(
                audio_graph_ipc_contract::runtime_diagnostic::RuntimeDiagnostic::Runtime(
                    audio_graph_ipc_contract::runtime_diagnostic::RuntimeErrorDiagnostic {
                        code: audio_graph_ipc_contract::runtime_diagnostic::RuntimeErrorCode::InvalidConfiguration,
                        ..
                    }
                )
            )
        ));
    }

    /// The shared local guard the IPC commands also call: blank (including
    /// whitespace-only) fields are the typed `invalid_configuration`, and a
    /// filled configuration passes.
    #[test]
    fn validate_auth_configuration_trims_blank_fields() {
        use crate::settings::GeminiAuthMode as Mode;
        let invalid = |auth: Mode| {
            matches!(
                validate_auth_configuration(&auth),
                Err(crate::error::AppError::RuntimeDiagnostic(
                    audio_graph_ipc_contract::runtime_diagnostic::RuntimeDiagnostic::Runtime(
                        audio_graph_ipc_contract::runtime_diagnostic::RuntimeErrorDiagnostic {
                            code:
                                audio_graph_ipc_contract::runtime_diagnostic::RuntimeErrorCode::InvalidConfiguration,
                            ..
                        }
                    )
                ))
            )
        };
        let vertex = |project: &str, location: &str| Mode::VertexAI {
            project_id: project.into(),
            location: location.into(),
            service_account_path: None,
        };
        assert!(invalid(Mode::ApiKey {
            api_key: "   ".into()
        }));
        assert!(invalid(vertex("  ", "us-central1")));
        assert!(invalid(vertex("project", "\t")));
        assert!(!invalid(Mode::ApiKey {
            api_key: "key".into()
        }));
        assert!(validate_auth_configuration(&vertex("project", "us-central1")).is_ok());
    }

    /// Sibling of the OpenAI Realtime
    /// `error_frame_redacts_message_and_classifies` test: a transport-layer
    /// error `Display` can echo the failing request back, including the
    /// `x-goog-api-key` header, the `Authorization: Bearer …` header, and the
    /// `?…&key={project_id}` query string. The connect sites in `open_ws` wrap
    /// every such diagnostic in `crate::error::redacted_provider_diagnostic`
    /// with the live auth secrets, so the secret must never survive into the
    /// `GeminiConnectError::message` that becomes a `GeminiEvent::Error`.
    #[test]
    fn error_frame_redacts_message_and_classifies() {
        // ── ApiKey site: secret is the api key (x-goog-api-key header). ──────
        let api_key = "gemini-live-key-sentinel";
        let secrets = [api_key];
        let synthetic = format!(
            "WebSocket connect failed: HTTP error 401 \
             (request had header x-goog-api-key: {api_key})"
        );
        let diag = crate::error::redacted_provider_diagnostic(&synthetic, secrets.iter().copied());
        assert!(
            !diag.contains(api_key),
            "api key must be scrubbed from the diagnostic: {diag}"
        );
        assert!(
            diag.contains("<redacted>"),
            "expected redaction marker: {diag}"
        );
        assert!(
            diag.contains("WebSocket connect failed"),
            "non-secret context must be preserved: {diag}"
        );

        // Classification is independent of the message and survives redaction:
        // the connect site pairs the redacted message with the tungstenite
        // category. Spot-check a representative category mapping here — a 1008
        // close with an "api key" reason classifies as Auth.
        let category = classify_close_frame(1008, "invalid api key").unwrap();
        assert_eq!(category, GeminiErrorCategory::Auth);

        // ── Vertex site: secrets are the bearer token + project id. ──────────
        let token = "ya29.FAKE-vertex-bearer-token-aabbccddeeff";
        let project = "my-prod-project-007";
        let secrets = [token, project];
        let synthetic = format!(
            "WebSocket connect failed: 403 GET \
             wss://us-central1-aiplatform.googleapis.com/...?alt=proto&key={project} \
             (Authorization: Bearer {token})"
        );
        let diag = crate::error::redacted_provider_diagnostic(&synthetic, secrets.iter().copied());
        assert!(!diag.contains(token), "bearer token survived: {diag}");
        assert!(!diag.contains(project), "project id survived: {diag}");

        // ── Empty-secrets pattern pass (setup-send / read-loop sites). ───────
        // Even without the literal secret in scope, the pattern pass scrubs the
        // bearer/key-query shapes a transport error may echo.
        let synthetic =
            "Failed to send setup: broken pipe after Authorization: Bearer ya29.LEAKED999";
        let diag =
            crate::error::redacted_provider_diagnostic(synthetic, std::iter::empty::<&str>());
        assert!(
            !diag.contains("ya29.LEAKED999"),
            "pattern pass must scrub bearer even with empty explicit secrets: {diag}"
        );
    }

    /// Mock-WS harness (modelled on `session_task_cancels_during_reconnect_backoff`):
    /// drives a real client socket against a local server that closes with a
    /// 1008 auth code whose REASON carries the configured api key, a request
    /// id, and a sentinel. `user_disconnected` starts `false` so `run_io`
    /// really classifies the close frame (setting it `true` would skip
    /// classification and make this canary vacuous). The test requires the
    /// full serialized error event, then cancels the reconnect ladder.
    #[tokio::test(flavor = "current_thread")]
    async fn server_close_error_event_does_not_leak_api_key() {
        let capture = crate::test_log_capture::ThreadLogCapture::start();
        // Unique to this test: the error-frame test plants a different key.
        const API_KEY: &str = "canary-close-test-credential";
        const SENTINEL: &str = "canary-close-reason-text";
        const REQUEST_ID: &str = "canary-request-identifier-gemini";

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local websocket server");
        let addr = listener.local_addr().expect("local addr");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept websocket");
            let mut websocket = tokio_tungstenite::accept_async(stream)
                .await
                .expect("server websocket handshake");
            // Close with an auth code whose reason echoes every leak class,
            // so the run_io close-frame path emits a classified error.
            let close = tungstenite::protocol::CloseFrame {
                code: tungstenite::protocol::frame::coding::CloseCode::Library(1008),
                reason: format!("invalid api key {API_KEY} {SENTINEL} {REQUEST_ID}").into(),
            };
            let _ = websocket.send(Message::Close(Some(close))).await;
            let _ = websocket.next().await;
        });

        let (client_socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}"))
            .await
            .expect("client websocket connect");
        let (writer, reader) = client_socket.split();
        let (_audio_tx, audio_rx) = tokio_mpsc::channel(8);
        let (event_tx, event_rx) = crossbeam_channel::bounded(16);
        let connected = Arc::new(AtomicBool::new(true));
        // `false`: the close MUST be classified. Flipped to `true` below once
        // the error event is captured, to cancel the reconnect ladder.
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let resumption_handle = Arc::new(std::sync::Mutex::new(None));

        let handle = tokio::spawn(session_task(
            writer,
            reader,
            audio_rx,
            GeminiConfig::text(
                crate::settings::GeminiAuthMode::ApiKey {
                    api_key: API_KEY.into(),
                },
                "gemini-3.1-flash-live-preview",
            ),
            event_tx,
            connected.clone(),
            user_disconnected.clone(),
            resumption_handle.clone(),
        ));

        // Require the full error event before cancelling.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        let mut error_event = None;
        while tokio::time::Instant::now() < deadline {
            match event_rx.try_recv() {
                Ok(event @ GeminiEvent::Error { .. }) => {
                    error_event = Some(event);
                    break;
                }
                Ok(_) => {}
                Err(_) => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
        user_disconnected.store(true, Ordering::SeqCst);
        let error_event = error_event.expect("close frame must surface a classified error event");

        // Full wire assertion: closed site token, structural category,
        // closed diagnostic — nothing derived from the close reason.
        let wire = serde_json::to_value(&error_event).expect("event serializes");
        assert_eq!(
            wire,
            json!({
                "type": "error",
                "category": { "kind": "auth" },
                "message": "server_close",
                "diagnostic": {
                    "kind": "runtime",
                    "detail": {
                        "code": "request_rejected",
                        "retryable": false,
                        "recovery_action": "review_configuration",
                        "context": {
                            "operation": "realtime_conversation",
                            "transport": "websocket",
                            "status_class": "client_error"
                        }
                    }
                }
            })
        );
        let serialized = wire.to_string();
        for forbidden in [
            API_KEY,
            SENTINEL,
            REQUEST_ID,
            "invalid api key",
            "reason_len",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "serialized error event leaked `{forbidden}`: {serialized}"
            );
        }

        let _ = tokio::time::timeout(Duration::from_secs(2), handle).await;
        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;

        // The close path really logged — asserted on the capture started
        // BEFORE the action, on this (current-thread) runtime, so no other
        // test's log can satisfy it — and none of the planted values (a key
        // and sentinels unique to this test) reached a log line.
        let logs = crate::test_log_capture::render(&capture.finish());
        assert!(
            logs.contains("Gemini: server closed connection"),
            "the close-frame log site must have fired: {logs}"
        );
        for planted in [API_KEY, SENTINEL, REQUEST_ID] {
            assert!(!logs.contains(planted), "`{planted}` was logged: {logs}");
        }
        crate::test_log_capture::assert_process_logs_exclude(&[API_KEY, SENTINEL, REQUEST_ID]);
    }

    #[test]
    fn send_audio_fails_when_disconnected() {
        let client = GeminiLiveClient::new(allowed_gemini_config());
        let result = client.send_audio(&[0.5, -0.3]);
        assert!(result.is_err());
    }

    #[test]
    fn default_policy_rejects_non_empty_audio_before_channel_initialization() {
        let client = GeminiLiveClient::new(GeminiConfig::text(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "key".into(),
            },
            "model",
        ));

        let error = client.send_audio(&[0.5, -0.3]).unwrap_err();

        assert!(error.contains("Privacy policy blocked"));
        assert!(error.contains("gemini.live"));
        assert!(error.contains(EXPLICIT_POLICY_REQUIRED));
        assert!(!error.contains("Audio channel not initialized"));
    }

    #[test]
    fn blocked_policy_rejects_non_empty_audio_before_channel_initialization() {
        let client = GeminiLiveClient::new(blocked_gemini_config());

        let error = client.send_audio(&[0.5, -0.3]).unwrap_err();

        assert!(error.contains("Privacy policy blocked"));
        assert!(error.contains("gemini.live"));
        assert!(error.contains("local_only"));
        assert!(!error.contains("Audio channel not initialized"));
    }

    #[test]
    fn blocked_policy_allows_empty_audio_without_channel_initialization() {
        let client = GeminiLiveClient::new(blocked_gemini_config());

        assert!(client.send_audio(&[]).is_ok());
    }

    #[test]
    fn blocked_policy_error_redacts_secret_audio_and_transcript_like_values() {
        let client = GeminiLiveClient::new(blocked_gemini_config());

        let error = client.send_audio(&[0.5, -0.3]).unwrap_err();

        for forbidden in [
            "gemini-private-api-key",
            "0.5",
            "-0.3",
            "patient said private diagnosis",
        ] {
            assert!(
                !error.contains(forbidden),
                "privacy error leaked {forbidden}: {error}"
            );
        }
    }

    #[test]
    fn handle_server_transcription() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{
            "serverContent": {
                "inputTranscription": {
                    "text": "hello world",
                    "completed": true
                }
            }
        }"#;

        handle_server_message(msg, &tx, &handle);

        let event = rx.try_recv().unwrap();
        match event {
            GeminiEvent::Transcription { text, is_final } => {
                assert_eq!(text, "hello world");
                assert!(is_final);
            }
            _ => panic!("Expected Transcription event"),
        }
    }

    #[test]
    fn handle_server_model_turn() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{
            "serverContent": {
                "modelTurn": {
                    "parts": [
                        { "text": "The user said hello" }
                    ]
                }
            }
        }"#;

        handle_server_message(msg, &tx, &handle);

        let event = rx.try_recv().unwrap();
        match event {
            GeminiEvent::ModelResponse { text } => {
                assert_eq!(text, "The user said hello");
            }
            _ => panic!("Expected ModelResponse event"),
        }
    }

    #[test]
    fn handle_server_turn_complete() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{ "serverContent": { "turnComplete": true } }"#;
        handle_server_message(msg, &tx, &handle);

        match rx.try_recv().unwrap() {
            GeminiEvent::TurnComplete { usage } => {
                assert!(usage.is_none(), "no usageMetadata in this frame");
            }
            _ => panic!("Expected TurnComplete event"),
        }
    }

    #[test]
    fn handle_server_audio_chunk_decodes_inline_data() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        // base64 of bytes [0x00, 0x01, 0x02, 0x03] = "AAECAw=="
        let msg = r#"{
            "serverContent": {
                "modelTurn": {
                    "parts": [
                        { "inlineData": { "mimeType": "audio/pcm;rate=24000", "data": "AAECAw==" } }
                    ]
                }
            }
        }"#;

        handle_server_message(msg, &tx, &handle);

        match rx.try_recv().unwrap() {
            GeminiEvent::AudioChunk {
                data_base64,
                sample_rate,
            } => {
                // Forwarded verbatim from inlineData.data (decoded at point of use).
                assert_eq!(data_base64, "AAECAw==");
                assert_eq!(sample_rate, GEMINI_OUTPUT_SAMPLE_RATE);
                assert_eq!(sample_rate, 24_000);
            }
            other => panic!("Expected AudioChunk, got {other:?}"),
        }
    }

    #[test]
    fn handle_server_audio_and_text_part_both_emit() {
        // A modelTurn can mix a text part and an inlineData audio part; both
        // must surface (text → ModelResponse, audio → AudioChunk).
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{
            "serverContent": {
                "modelTurn": {
                    "parts": [
                        { "text": "hi" },
                        { "inlineData": { "data": "AAECAw==" } }
                    ]
                }
            }
        }"#;
        handle_server_message(msg, &tx, &handle);

        let mut saw_text = false;
        let mut saw_audio = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                GeminiEvent::ModelResponse { text } => {
                    assert_eq!(text, "hi");
                    saw_text = true;
                }
                GeminiEvent::AudioChunk { data_base64, .. } => {
                    assert_eq!(data_base64, "AAECAw==");
                    saw_audio = true;
                }
                other => panic!("unexpected event {other:?}"),
            }
        }
        assert!(saw_text && saw_audio, "expected both text and audio events");
    }

    #[test]
    fn handle_server_audio_forwards_base64_verbatim_without_decoding() {
        // The handler no longer decodes (decode is deferred to the consumer, see
        // GeminiEvent::AudioChunk). It must forward the inlineData string verbatim
        // and never panic — including on a string that won't base64-decode; the
        // decode-and-drop happens downstream in `gemini_event_to_signal` (see
        // `gemini_invalid_base64_audio_maps_to_none` in the converse module).
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{
            "serverContent": {
                "modelTurn": { "parts": [ { "inlineData": { "data": "!!!notb64!!!" } } ] }
            }
        }"#;
        handle_server_message(msg, &tx, &handle);

        match rx.try_recv().unwrap() {
            GeminiEvent::AudioChunk { data_base64, .. } => {
                assert_eq!(data_base64, "!!!notb64!!!");
            }
            other => panic!("Expected AudioChunk (verbatim), got {other:?}"),
        }
        // Only that one chunk; an empty inlineData string is the sole skip case.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn handle_server_interrupted_barge_in() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{ "serverContent": { "interrupted": true } }"#;
        handle_server_message(msg, &tx, &handle);

        match rx.try_recv().unwrap() {
            GeminiEvent::Interrupted => {}
            other => panic!("Expected Interrupted, got {other:?}"),
        }
    }

    #[test]
    fn handle_server_interrupted_false_is_noop() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{ "serverContent": { "interrupted": false } }"#;
        handle_server_message(msg, &tx, &handle);
        assert!(rx.try_recv().is_err(), "interrupted:false must not emit");
    }

    #[test]
    fn handle_server_output_transcription() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{
            "serverContent": { "outputTranscription": { "text": "the spoken reply" } }
        }"#;
        handle_server_message(msg, &tx, &handle);

        match rx.try_recv().unwrap() {
            GeminiEvent::OutputTranscription { text } => {
                assert_eq!(text, "the spoken reply");
            }
            other => panic!("Expected OutputTranscription, got {other:?}"),
        }
    }

    #[test]
    fn handle_server_generation_complete() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        // generationComplete precedes turnComplete; both can appear in one frame.
        let msg = r#"{
            "serverContent": { "generationComplete": true, "turnComplete": true }
        }"#;
        handle_server_message(msg, &tx, &handle);

        let mut saw_gen = false;
        let mut saw_turn = false;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                GeminiEvent::GenerationComplete => saw_gen = true,
                GeminiEvent::TurnComplete { .. } => saw_turn = true,
                other => panic!("unexpected event {other:?}"),
            }
        }
        assert!(saw_gen, "expected GenerationComplete");
        assert!(saw_turn, "expected TurnComplete");
    }

    #[test]
    fn handle_server_go_away() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{ "goAway": {} }"#;
        handle_server_message(msg, &tx, &handle);

        match rx.try_recv().unwrap() {
            GeminiEvent::Error {
                category, message, ..
            } => {
                // Issue #145: message is the closed site token.
                assert_eq!(message, "go_away");
                assert_eq!(category, GeminiErrorCategory::Server);
            }
            _ => panic!("Expected Error event for goAway"),
        }
    }

    /// Issue #145: usage accounting is product DATA (the `turn_complete`
    /// event carries it and the command layer persists it), but diagnostic
    /// logs must not print exact token counts. Covers both usage log sites
    /// (turn-complete and standalone usage frame).
    #[test]
    fn usage_token_counts_ride_the_event_but_never_the_logs() {
        let capture = crate::test_log_capture::ThreadLogCapture::start();
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        // Sentinel counts, deliberately unlike any other test's numbers.
        let turn_frame = r#"{
            "serverContent": { "turnComplete": true },
            "usageMetadata": {
                "promptTokenCount": 918273,
                "responseTokenCount": 645192,
                "totalTokenCount": 1563465
            }
        }"#;
        handle_server_message(turn_frame, &tx, &handle);
        let standalone_frame = r#"{ "usageMetadata": { "totalTokenCount": 7340211 } }"#;
        handle_server_message(standalone_frame, &tx, &handle);

        // The DATA path still carries the exact usage.
        let mut totals = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let GeminiEvent::TurnComplete { usage: Some(u) } = event {
                totals.push(u.total_token_count);
            }
        }
        assert_eq!(totals, vec![Some(1_563_465), Some(7_340_211)]);

        // …while the diagnostic logs (captured on THIS thread, started before
        // the action) carry neither the counts nor their parts. The positive
        // assertion can only be satisfied by this test's own frames.
        let logs = crate::test_log_capture::render(&capture.finish());
        assert!(
            logs.contains("Gemini Live: standalone usage frame"),
            "the standalone usage log site must have fired: {logs}"
        );
        for count in ["918273", "645192", "1563465", "7340211"] {
            assert!(
                !logs.contains(count),
                "usage count `{count}` logged: {logs}"
            );
        }
        crate::test_log_capture::assert_process_logs_exclude(&[
            "918273", "645192", "1563465", "7340211",
        ]);
    }

    #[test]
    fn resumption_update_captures_handle_when_resumable() {
        // The canonical happy-path: server says "here's a fresh handle you
        // can use to resume, and yes it's valid right now".
        let (tx, _rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{
            "sessionResumptionUpdate": {
                "newHandle": "opaque-handle-abc",
                "resumable": true
            }
        }"#;
        handle_server_message(msg, &tx, &handle);

        assert_eq!(
            handle.lock().unwrap().as_deref(),
            Some("opaque-handle-abc"),
            "a resumable update must populate the handle slot"
        );
    }

    #[test]
    fn resumption_update_ignores_non_resumable() {
        // Server sends an update mid-generation or during a function call
        // where resumption is temporarily unavailable. We must *not*
        // overwrite the last known-good handle — otherwise a reconnect in
        // that window would fall back to a fresh session unnecessarily.
        let (tx, _rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(Some("prev-good".to_string())));

        let msg = r#"{
            "sessionResumptionUpdate": {
                "newHandle": "",
                "resumable": false
            }
        }"#;
        handle_server_message(msg, &tx, &handle);

        assert_eq!(
            handle.lock().unwrap().as_deref(),
            Some("prev-good"),
            "non-resumable update must preserve prior handle"
        );
    }

    #[test]
    fn resumption_update_missing_new_handle_is_noop() {
        // Defensive: some update frames may carry only `resumable: true`
        // without a fresh handle (re-affirming the current one). Treat as
        // a no-op rather than clobbering the cache.
        let (tx, _rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(Some("keep-me".to_string())));

        let msg = r#"{
            "sessionResumptionUpdate": {
                "resumable": true
            }
        }"#;
        handle_server_message(msg, &tx, &handle);

        assert_eq!(handle.lock().unwrap().as_deref(), Some("keep-me"));
    }

    /// End-to-end state-machine check: feed a `sessionResumptionUpdate` into
    /// the message handler, then build a reconnect setup payload off the
    /// captured handle and verify it flows through to
    /// `setup.sessionResumption.handle`. This is the behavioural guarantee
    /// the feature is supposed to provide.
    #[test]
    fn resumption_handle_threads_into_reconnect_setup() {
        let (tx, _rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let update = r#"{
            "sessionResumptionUpdate": {
                "newHandle": "srh-42",
                "resumable": true
            }
        }"#;
        handle_server_message(update, &tx, &handle);

        let captured = handle.lock().unwrap().clone();
        assert_eq!(captured.as_deref(), Some("srh-42"));

        let config = GeminiConfig::text(
            crate::settings::GeminiAuthMode::ApiKey {
                api_key: "k".into(),
            },
            "gemini-3.1-flash-live-preview",
        );
        let reconnect_setup = build_setup_message(&config, captured.as_deref());

        assert_eq!(
            reconnect_setup["setup"]["sessionResumption"]["handle"], "srh-42",
            "captured handle must appear in next setup payload"
        );
    }

    // ── usageMetadata parsing ──────────────────────────────────────────

    /// Full-fat happy path: the server attaches `usageMetadata` to the same
    /// frame as `turnComplete`. Verifies every documented counter and the
    /// per-modality detail arrays are parsed and propagated.
    #[test]
    fn usage_metadata_parsed_on_turn_complete() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{
            "serverContent": { "turnComplete": true },
            "usageMetadata": {
                "promptTokenCount": 120,
                "cachedContentTokenCount": 32,
                "responseTokenCount": 45,
                "toolUsePromptTokenCount": 10,
                "thoughtsTokenCount": 5,
                "totalTokenCount": 212,
                "promptTokensDetails": [{ "modality": "TEXT", "tokenCount": 120 }],
                "responseTokensDetails": [{ "modality": "AUDIO", "tokenCount": 45 }]
            }
        }"#;
        handle_server_message(msg, &tx, &handle);

        match rx.try_recv().unwrap() {
            GeminiEvent::TurnComplete { usage } => {
                let u = usage.expect("usageMetadata must be parsed");
                assert_eq!(u.prompt_token_count, Some(120));
                assert_eq!(u.cached_content_token_count, Some(32));
                assert_eq!(u.response_token_count, Some(45));
                assert_eq!(u.tool_use_prompt_token_count, Some(10));
                assert_eq!(u.thoughts_token_count, Some(5));
                assert_eq!(u.total_token_count, Some(212));
                assert_eq!(u.prompt_tokens_details.len(), 1);
                assert_eq!(u.prompt_tokens_details[0].modality, "TEXT");
                assert_eq!(u.prompt_tokens_details[0].token_count, 120);
                assert_eq!(u.response_tokens_details.len(), 1);
                assert_eq!(u.response_tokens_details[0].modality, "AUDIO");
            }
            other => panic!("Expected TurnComplete with usage, got {other:?}"),
        }
    }

    /// Minimal usage frame — only totals reported. Optional counters must
    /// stay `None` so the UI can tell "zero" from "not reported".
    #[test]
    fn usage_metadata_optional_fields_stay_none() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{
            "serverContent": { "turnComplete": true },
            "usageMetadata": {
                "promptTokenCount": 10,
                "responseTokenCount": 20,
                "totalTokenCount": 30
            }
        }"#;
        handle_server_message(msg, &tx, &handle);

        match rx.try_recv().unwrap() {
            GeminiEvent::TurnComplete { usage } => {
                let u = usage.unwrap();
                assert_eq!(u.prompt_token_count, Some(10));
                assert_eq!(u.response_token_count, Some(20));
                assert_eq!(u.total_token_count, Some(30));
                assert!(u.cached_content_token_count.is_none());
                assert!(u.thoughts_token_count.is_none());
                assert!(u.prompt_tokens_details.is_empty());
            }
            other => panic!("Expected TurnComplete, got {other:?}"),
        }
    }

    /// The server sometimes sends `usageMetadata` without `serverContent`
    /// (billing roll-up). The handler should still surface it as a
    /// `TurnComplete` so downstream accounting stays consistent.
    #[test]
    fn usage_metadata_standalone_emits_turn_complete() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let handle = Arc::new(std::sync::Mutex::new(None));

        let msg = r#"{
            "usageMetadata": {
                "promptTokenCount": 1,
                "responseTokenCount": 2,
                "totalTokenCount": 3
            }
        }"#;
        handle_server_message(msg, &tx, &handle);

        match rx.try_recv().unwrap() {
            GeminiEvent::TurnComplete { usage } => {
                let u = usage.expect("standalone usage must be surfaced");
                assert_eq!(u.total_token_count, Some(3));
            }
            other => panic!("Expected TurnComplete, got {other:?}"),
        }
    }

    /// The `Reconnected` event carries a `resumed` flag so the frontend can
    /// distinguish "server restored prior context" from "fresh session after
    /// outage". Both paths must serialize with the flag visible in the JSON
    /// envelope, under the same `#[serde(tag = "type")]` the other variants
    /// use. The flag mirrors `handle_snapshot.is_some()` at the emit site:
    /// truthy iff a cached resumption handle was threaded into the setup.
    #[test]
    fn reconnected_event_serializes_with_resumed_flag() {
        // Resumed path: reconnect used a cached handle.
        let resumed = serde_json::to_value(GeminiEvent::Reconnected { resumed: true }).unwrap();
        assert_eq!(resumed["type"], "reconnected");
        assert_eq!(
            resumed["resumed"], true,
            "resumed=true path must surface the flag so UI can show 'session restored'"
        );

        // Fresh path: first outage or no resumable handle.
        let fresh = serde_json::to_value(GeminiEvent::Reconnected { resumed: false }).unwrap();
        assert_eq!(fresh["type"], "reconnected");
        assert_eq!(
            fresh["resumed"], false,
            "resumed=false path must surface the flag so UI can warn 'fresh session'"
        );
    }

    // ── Error categorization ───────────────────────────────────────────
    //
    // Coverage matrix:
    //   close-frame 1008 + "API key"       → Auth
    //   close-frame 1008 + "token expired" → Unknown (prose is never a signal)
    //   close-frame 1008 + "quota"         → RateLimit (no retry-after hint)
    //   close-frame 1011                   → Server
    //   close-frame 1000                   → None (normal closure)
    //   close-frame 1008 + unknown         → Unknown
    //   tungstenite::Error::Io             → Network
    //   tungstenite::Error::ConnectionClosed → Network

    #[test]
    fn close_frame_1008_api_key_maps_to_auth() {
        let cat = classify_close_frame(1008, "Invalid API key: bad signature");
        assert_eq!(cat, Some(GeminiErrorCategory::Auth));
    }

    #[test]
    fn close_frame_1008_token_expired_prose_is_not_credential_expiry() {
        // The close REASON is provider prose, never evidence of credential
        // expiry: an "expired" reason on a 1008 close is the generic closed
        // category (internal), not `credential_expired`.
        let cat = classify_close_frame(1008, "token expired, please refresh");
        assert_eq!(cat, Some(GeminiErrorCategory::Unknown));
    }

    /// A prose-derived close-frame category never yields a credential
    /// diagnosis: both `Auth` and `AuthExpired` close categories are the
    /// generic closed `request_rejected` / `review_configuration`, while the
    /// category (teardown) is unchanged and other categories keep their own
    /// mapping.
    #[test]
    fn close_frame_diagnostic_never_claims_a_credential_code() {
        use audio_graph_ipc_contract::runtime_diagnostic::RuntimeErrorCode as Code;
        let code_of = |d: RuntimeDiagnostic| match d {
            RuntimeDiagnostic::Runtime(e) => (e.code, e.recovery_action),
            other => panic!("expected runtime diagnostic, got {other:?}"),
        };
        for category in [GeminiErrorCategory::Auth, GeminiErrorCategory::AuthExpired] {
            assert_eq!(
                code_of(close_frame_diagnostic(&category)),
                (
                    Code::RequestRejected,
                    RuntimeSafeRecoveryAction::ReviewConfiguration
                )
            );
        }
        assert_eq!(
            code_of(close_frame_diagnostic(&GeminiErrorCategory::Server)),
            (Code::ProviderUnavailable, RuntimeSafeRecoveryAction::Retry)
        );
    }

    /// Vertex token acquisition (#145): only a transport-level `Http` error
    /// reads as network failure; everything else is the generic closed
    /// diagnostic. No `gcp_auth` error ever claims `credential_rejected`
    /// because the SDK does not structurally distinguish a rejected
    /// credential from any other non-success token response.
    #[test]
    fn vertex_token_failures_never_claim_credential_rejection() {
        use audio_graph_ipc_contract::runtime_diagnostic::RuntimeErrorCode as Code;
        for err in [
            gcp_auth::Error::Str("token request failed"),
            gcp_auth::Error::Io(
                "failed to open application credentials file",
                std::io::Error::other("x"),
            ),
        ] {
            let failure = GeminiConnectError::from_gcp_token_error(&err);
            // Terminal session-end category is kept; the diagnosis is generic.
            assert_eq!(failure.category, GeminiErrorCategory::Auth);
            match failure.diagnostic {
                RuntimeDiagnostic::Runtime(e) => assert_eq!(e.code, Code::Internal),
                other => panic!("expected runtime diagnostic, got {other:?}"),
            }
        }
    }

    #[test]
    fn close_frame_1008_quota_maps_to_rate_limit() {
        let cat = classify_close_frame(1008, "Quota exceeded for project");
        assert_eq!(
            cat,
            Some(GeminiErrorCategory::RateLimit { retry_delay: None })
        );
    }

    #[test]
    fn close_frame_1011_maps_to_server() {
        let cat = classify_close_frame(1011, "internal error");
        assert_eq!(cat, Some(GeminiErrorCategory::Server));
    }

    #[test]
    fn close_frame_1000_is_not_an_error() {
        // Normal closure must not trigger a toast.
        assert_eq!(classify_close_frame(1000, "bye"), None);
    }

    #[test]
    fn close_frame_1008_unknown_reason_falls_through_to_unknown() {
        // Policy violation with a reason we don't recognize — we still
        // signal *an* error, but the category is Unknown so the UI
        // doesn't lie about remediation.
        let cat = classify_close_frame(1008, "something else entirely");
        assert_eq!(cat, Some(GeminiErrorCategory::Unknown));
    }

    /// Provider-auth typing (#518): the closed code and recovery action per
    /// auth path, with the category (terminal-auth teardown) unchanged.
    #[test]
    fn auth_paths_use_provider_auth_codes() {
        use audio_graph_ipc_contract::runtime_diagnostic::{
            RuntimeDiagnostic as D, RuntimeErrorCode as Code, RuntimeSafeRecoveryAction as Action,
        };
        fn tuple(d: &RuntimeDiagnostic) -> (Code, bool, Action) {
            match d {
                D::Runtime(e) => (e.code, e.retryable, e.recovery_action),
                other => panic!("expected runtime diagnostic, got {other:?}"),
            }
        }
        fn http(status: u16) -> tungstenite::Error {
            tungstenite::Error::Http(Box::new(
                tungstenite::http::Response::builder()
                    .status(status)
                    .body(None)
                    .expect("response builds"),
            ))
        }

        // Close-frame categories.
        assert_eq!(
            tuple(&GeminiErrorCategory::Auth.runtime_diagnostic()),
            (Code::CredentialRejected, false, Action::ReenterCredential)
        );
        assert_eq!(
            tuple(&GeminiErrorCategory::AuthExpired.runtime_diagnostic()),
            (Code::CredentialExpired, false, Action::RefreshCredential)
        );

        // Handshake 401 rejects the credential; 403 is a permission failure.
        // Both stay in the `Auth` category.
        let unauthorized = GeminiConnectError::from_tungstenite(&http(401));
        assert_eq!(unauthorized.category, GeminiErrorCategory::Auth);
        assert_eq!(
            tuple(&unauthorized.diagnostic),
            (Code::CredentialRejected, false, Action::ReenterCredential)
        );
        let forbidden = GeminiConnectError::from_tungstenite(&http(403));
        assert_eq!(forbidden.category, GeminiErrorCategory::Auth);
        assert_eq!(
            tuple(&forbidden.diagnostic),
            (Code::PermissionDenied, false, Action::ReviewConfiguration)
        );

        // Local credential configuration failure (gcp_auth setup).
        let misconfigured = GeminiConnectError::auth_configuration();
        assert_eq!(misconfigured.category, GeminiErrorCategory::Auth);
        assert_eq!(
            tuple(&misconfigured.diagnostic),
            (
                Code::InvalidConfiguration,
                false,
                Action::ReviewConfiguration
            )
        );

        // A key that cannot be encoded as a header is a local configuration
        // failure (not a rejected credential); the category is unchanged.
        let unencodable = gemini_api_key_connect_headers("bad\nkey")
            .expect_err("a newline cannot be a header value");
        assert_eq!(unencodable.category, GeminiErrorCategory::Unknown);
        assert_eq!(
            tuple(&unencodable.diagnostic),
            (
                Code::InvalidConfiguration,
                false,
                Action::ReviewConfiguration
            )
        );

        // Non-auth handshake statuses keep their category mapping.
        assert_eq!(
            tuple(&GeminiConnectError::from_tungstenite(&http(503)).diagnostic),
            (Code::ProviderUnavailable, true, Action::Retry)
        );
    }

    #[test]
    fn tungstenite_io_maps_to_network() {
        let err = tungstenite::Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "x",
        ));
        assert_eq!(
            classify_tungstenite_error(&err),
            GeminiErrorCategory::Network
        );
    }

    #[test]
    fn tungstenite_connection_closed_maps_to_network() {
        assert_eq!(
            classify_tungstenite_error(&tungstenite::Error::ConnectionClosed),
            GeminiErrorCategory::Network,
        );
    }

    #[test]
    fn gemini_error_category_serializes_with_kind_tag() {
        // The frontend branches on `category.kind`, so the tag must be
        // stable and snake_cased. Rate-limit's closed retry bucket is
        // optional and must be omitted when absent.
        let rl_none =
            serde_json::to_value(GeminiErrorCategory::RateLimit { retry_delay: None }).unwrap();
        assert_eq!(rl_none["kind"], "rate_limit");
        assert!(
            rl_none.get("retry_delay").is_none(),
            "absent retry-after must not serialize as null — got {rl_none:?}"
        );

        let rl_some = serde_json::to_value(GeminiErrorCategory::RateLimit {
            retry_delay: Some(RuntimeRetryDelayBucket::Medium),
        })
        .unwrap();
        assert_eq!(rl_some["kind"], "rate_limit");
        assert_eq!(rl_some["retry_delay"], "medium");

        let auth = serde_json::to_value(GeminiErrorCategory::AuthExpired).unwrap();
        assert_eq!(auth["kind"], "auth_expired");
    }

    fn http_429(retry_after: Option<&str>) -> tungstenite::Error {
        let mut builder = tungstenite::http::Response::builder().status(429);
        if let Some(value) = retry_after {
            builder = builder.header("retry-after", value);
        }
        tungstenite::Error::Http(Box::new(builder.body(None).expect("build 429 response")))
    }

    /// Issue #145 canary: a numeric `Retry-After` is reduced to a closed
    /// bucket at classification; the exact seconds never reach the category
    /// (wire or `Debug`, which the connect/reconnect logs print) nor the
    /// diagnostic envelope.
    #[test]
    fn numeric_retry_after_is_bucketed_and_the_exact_value_never_survives() {
        const EXACT: &str = "7391";
        let category = classify_tungstenite_error(&http_429(Some(EXACT)));
        assert_eq!(
            category,
            GeminiErrorCategory::RateLimit {
                retry_delay: Some(RuntimeRetryDelayBucket::Long)
            }
        );
        let surfaces = [
            format!("{category:?}"),
            serde_json::to_string(&category).unwrap(),
            serde_json::to_string(&category.runtime_diagnostic()).unwrap(),
            format!("{:?}", GeminiConnectError::new(category.clone())),
        ];
        for surface in &surfaces {
            assert!(
                !surface.contains(EXACT),
                "exact Retry-After leaked into {surface}"
            );
        }
        assert!(surfaces[2].contains("\"retry_delay\":\"long\""));
    }

    /// An absent, or non-numeric (HTTP-date), `Retry-After` must leave the
    /// bucket absent — never `immediate`.
    #[test]
    fn absent_or_unparseable_retry_after_leaves_the_bucket_absent() {
        for header in [None, Some("Wed, 21 Oct 2026 07:28:00 GMT")] {
            let category = classify_tungstenite_error(&http_429(header));
            assert_eq!(
                category,
                GeminiErrorCategory::RateLimit { retry_delay: None }
            );
            let diagnostic = serde_json::to_value(category.runtime_diagnostic()).unwrap();
            assert!(
                diagnostic["detail"]["context"].get("retry_delay").is_none(),
                "absence must not map to a bucket: {diagnostic}"
            );
        }
    }

    /// `TurnComplete { usage: Some(..) }` must round-trip through the
    /// `#[serde(tag = "type")]` envelope used to emit to the frontend.
    /// Asserts `tokens_used` is no longer `0`: the event now carries the
    /// actual `usage` sub-object with non-zero counters.
    #[test]
    fn turn_complete_with_usage_serializes_cleanly() {
        let event = GeminiEvent::TurnComplete {
            usage: Some(UsageMetadata {
                prompt_token_count: Some(7),
                response_token_count: Some(13),
                total_token_count: Some(20),
                ..Default::default()
            }),
        };
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "turn_complete");
        assert_eq!(json["usage"]["promptTokenCount"], 7);
        assert_eq!(json["usage"]["responseTokenCount"], 13);
        assert_eq!(json["usage"]["totalTokenCount"], 20);
        // Unreported counters must be absent (not serialized as 0).
        assert!(
            json["usage"].get("cachedContentTokenCount").is_none(),
            "optional counters not reported by the server must be omitted"
        );
    }
}
