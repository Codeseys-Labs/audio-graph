//! OpenAI Realtime **speech-to-speech** (S2S) voice-agent WebSocket client.
//!
//! Cloud-native voice agent over the OpenAI Realtime API (`gpt-realtime-2`),
//! the parallel of Gemini Live's native-audio (converse) path. This is the
//! S2S **sibling** of the STT [`crate::asr::openai_realtime`] transcription
//! client — a separate namespace with a separate event surface. The STT client
//! produces *user-speech text*; this one drives a full voice turn and emits the
//! assistant's spoken **audio** (PCM16-LE @ 24 kHz) plus the spoken-reply
//! transcript.
//!
//! # Protocol overview (GA — no `OpenAI-Beta` header)
//!
//! 1. Open WSS to `wss://api.openai.com/v1/realtime?model=<model>` with an
//!    `Authorization: Bearer {api_key}` header on the upgrade request.
//! 2. Immediately send a `session.update` configuring a **realtime** voice
//!    session: input + output audio as PCM16 @ 24 kHz, the prebuilt `voice`,
//!    `output_modalities: ["audio"]`, server VAD `turn_detection`, and
//!    `output_audio_transcription` so the graph still gets text for the spoken
//!    reply. Wait for the server `session.updated`/`session.created` to confirm.
//! 3. Stream user audio as `input_audio_buffer.append` text frames whose
//!    `audio` field is base64 of PCM16-LE 24 kHz mono.
//! 4. End a user turn with `input_audio_buffer.commit` + `response.create`
//!    ([`OpenAiRealtimeClient::end_user_turn`]) so the model generates a reply
//!    — server VAD also ends turns implicitly, this makes the FSM-driven
//!    boundary deterministic.
//! 5. Read assistant events:
//!    - `response.output_audio.delta` → [`OpenAiRealtimeEvent::Audio`]
//!      (base64 PCM16-LE 24 kHz, decoded at the point of use)
//!    - `response.output_audio_transcript.delta` → [`OpenAiRealtimeEvent::OutputTranscription`]
//!    - `input_audio_buffer.speech_started` → [`OpenAiRealtimeEvent::UserSpeechStarted`]
//!    - `input_audio_buffer.speech_stopped` → [`OpenAiRealtimeEvent::UserSpeechStopped`]
//!    - `response.done` → [`OpenAiRealtimeEvent::TurnComplete`]
//!    - top-level `error` → [`OpenAiRealtimeEvent::Error`]
//!
//! # Threading model
//!
//! Identical to [`crate::gemini`]: the public API is **synchronous** (called
//! from `std::thread` workers in `commands.rs`). Internally a dedicated tokio
//! runtime drives the WebSocket; audio is forwarded from the caller's thread to
//! the async writer via a `tokio::sync::mpsc` channel, and events flow back
//! through a `crossbeam_channel` that the command layer already expects.
//!
//! # Reconnect policy — NO server-side resume
//!
//! OpenAI Realtime sessions are capped at **60 minutes** and have **no resume**
//! (per the STT sibling's header). On any disconnect we open a fresh socket and
//! re-send `session.update` — "resumption" here is purely a fresh-socket
//! reconnect that re-applies the session config; there is no opaque handle to
//! restore prior conversation state. Reconnect uses the same 1s/2s/5s/10s
//! exponential backoff ladder as the other streaming clients.
//!
//! # Mockable tests
//!
//! Like the STT sibling, the reconnect path takes an injectable
//! [`ReconnectOpener`] (test-only) and writes go through an egress write-guard
//! so the full session lifecycle is exercised with an in-process WebSocket
//! fixture — **no live OpenAI key**.

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::Duration;
#[cfg(test)]
use std::{future::Future, pin::Pin};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{self, Message, client::IntoClientRequest},
};
// Closed, content-free runtime diagnostics (issue #145). Every error this
// module logs or emits carries one of these typed envelopes instead of raw
// provider / transport text.
use audio_graph_ipc_contract::runtime_diagnostic::{
    RuntimeDiagnostic, RuntimeDiagnosticContext, RuntimeErrorCode, RuntimeErrorDiagnostic,
    RuntimeOperation, RuntimeRetryDelayBucket, RuntimeSafeRecoveryAction, RuntimeStatusClass,
    RuntimeTransport,
};

// ---------------------------------------------------------------------------
// Public constants
// ---------------------------------------------------------------------------

/// Default OpenAI Realtime S2S voice model.
pub const DEFAULT_MODEL: &str = "gpt-realtime-2";
/// Default prebuilt voice when none is configured.
pub const DEFAULT_VOICE: &str = "alloy";
/// The only sample rate GA realtime audio accepts: 24 kHz mono, in and out.
pub const REALTIME_SAMPLE_RATE: u32 = 24_000;
/// Sample rate of the audio handed to [`OpenAiRealtimeClient::send_audio`] —
/// the speech pipeline's mixed mono output (see `audio::pipeline`).
const PIPELINE_SAMPLE_RATE: u32 = 16_000;
/// Provider id for content-egress diagnostics.
const PROVIDER: &str = "realtime_agent.openai_realtime";

/// Soft cap on the outbound audio queue (chunks). Mirrors the Gemini client's
/// bounded queue: buffers during transient reconnects, drops the newest chunk
/// past the cap rather than growing memory without bound.
const OPENAI_S2S_AUDIO_QUEUE_CAP: usize = 1000;

/// Count of audio chunks dropped due to a full outbound queue (log throttle).
static OPENAI_S2S_AUDIO_DROPS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Error category
// ---------------------------------------------------------------------------

/// Coarse category for an OpenAI Realtime S2S failure. Mirrors
/// [`crate::gemini::GeminiErrorCategory`] in shape so the converse layer can
/// normalize both engines through the same path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpenAiRealtimeErrorCategory {
    /// Invalid / missing API key — reauthentication required.
    Auth,
    /// The realtime session window ended (`session_expired`). Terminal
    /// session-end category; the diagnosis is a retryable `timeout`, not a
    /// credential problem.
    AuthExpired,
    /// Quota / rate-limit exceeded. `retry_delay` is the closed bucket a
    /// numeric `Retry-After` header reduces to (issue #145); exact seconds are
    /// never retained. Absent when no usable hint arrived.
    RateLimit {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_delay: Option<RuntimeRetryDelayBucket>,
    },
    /// Server-side failure (5xx / WS close 1011).
    Server,
    /// Transport-layer failure (TLS, TCP, DNS, socket reset).
    Network,
    /// Anything we could not positively classify.
    Unknown,
}

impl OpenAiRealtimeErrorCategory {
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
            // `AuthExpired` is the provider's session-window expiry
            // (`session_expired`), never a credential diagnosis: a retryable
            // timeout. Teardown keys off the category, not this diagnostic.
            Self::AuthExpired => realtime_diagnostic(
                RuntimeErrorCode::Timeout,
                true,
                RuntimeSafeRecoveryAction::Retry,
                None,
                None,
            ),
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

/// Fixed context for every OpenAI Realtime S2S diagnostic: the realtime S2S
/// operation over a WebSocket transport.
fn realtime_context() -> RuntimeDiagnosticContext {
    RuntimeDiagnosticContext::new(RuntimeOperation::RealtimeConversation)
        .with_transport(RuntimeTransport::Websocket)
}

/// Build the closed [`RuntimeDiagnostic`] every OpenAI Realtime S2S error
/// carries (issue #145). Context is fixed to the realtime S2S operation over
/// a WebSocket transport; only the closed code / retryability / recovery
/// action / optional coarse status class vary.
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

/// Closed site identifier carried on [`OpenAiRealtimeEvent::Error::message`]
/// (issue #145). The serialized `message` string is *only* one of these stable
/// tokens — never provider text, a WebSocket close reason, a request id, or
/// an exact payload length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenAiRealtimeErrorSite {
    /// Server sent a Close frame (classified close code).
    ServerClose,
    /// A top-level `error` frame arrived (provider-reported).
    ErrorFrame,
    /// A server JSON frame failed to parse.
    InvalidJson,
    /// The reconnect budget (1/2/5/10s ladder) is exhausted.
    ReconnectExhausted,
    /// A single reconnect attempt failed (session keeps retrying).
    ReconnectFailed,
    /// Runtime privacy policy blocked the session (steady-state).
    PolicyBlocked,
}

impl OpenAiRealtimeErrorSite {
    /// Stable snake_case token for the serialized event `message` field.
    pub const fn token(self) -> &'static str {
        match self {
            Self::ServerClose => "server_close",
            Self::ErrorFrame => "error_frame",
            Self::InvalidJson => "invalid_json",
            Self::ReconnectExhausted => "reconnect_exhausted",
            Self::ReconnectFailed => "reconnect_failed",
            Self::PolicyBlocked => "policy_blocked",
        }
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// Events emitted by the OpenAI Realtime S2S client to downstream consumers.
///
/// Serializable so Tauri can emit them directly to the frontend. The
/// load-bearing variant is [`Self::Audio`] — the assistant's spoken reply as
/// base64 PCM16-LE @ 24 kHz, mirroring [`crate::gemini::GeminiEvent::AudioChunk`]
/// (carried as a compact base64 string to avoid JSON int-array bloat over IPC;
/// consumers decode at the point of use).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OpenAiRealtimeEvent {
    /// A chunk of assistant **output audio** (PCM16-LE @ 24 kHz). Drives the
    /// converse FSM's `Thinking → Speaking` edge on the first chunk and feeds
    /// playback thereafter. `data_base64` is base64 of the raw PCM bytes.
    #[serde(rename = "audio")]
    Audio {
        data_base64: String,
        sample_rate: u32,
    },
    /// Streaming transcript of the assistant's spoken reply
    /// (`response.output_audio_transcript.delta`), routed to graph proposals.
    #[serde(rename = "output_transcription")]
    OutputTranscription { text: String },
    /// The server's VAD detected user speech began (the client-gated barge-in
    /// trigger for full-duplex). `input_audio_buffer.speech_started`.
    #[serde(rename = "user_speech_started")]
    UserSpeechStarted,
    /// The server's VAD detected the user stopped speaking
    /// (`input_audio_buffer.speech_stopped`).
    #[serde(rename = "user_speech_stopped")]
    UserSpeechStopped,
    /// The model finished its current turn (`response.done`). `usage` carries
    /// the token accounting when the server attaches it.
    #[serde(rename = "turn_complete")]
    TurnComplete { usage: Option<UsageMetadata> },
    /// A non-fatal error occurred (top-level `error` frame or a local parse
    /// failure). The socket stays open.
    ///
    /// `category` is the structured classification derived at the error
    /// site; `diagnostic` is the closed [`RuntimeDiagnostic`] envelope (issue
    /// #145) the frontend routes through `runtimeDiagnosticPresentation` for
    /// a localized, closed-key toast. `message` is a stable
    /// [`OpenAiRealtimeErrorSite::token`] site identifier — it never carries
    /// provider text, close reasons, request ids, or payload lengths.
    #[serde(rename = "error")]
    Error {
        category: OpenAiRealtimeErrorCategory,
        message: String,
        diagnostic: RuntimeDiagnostic,
    },
    /// The connection has been established and the session has been confirmed
    /// (`session.updated`/`session.created` received).
    #[serde(rename = "connected")]
    Connected,
    /// The WebSocket connection was closed.
    #[serde(rename = "disconnected")]
    Disconnected,
    /// The client detected a disconnect and is attempting to reconnect.
    ///
    /// `attempt` is 1-based: attempt 1 is the first retry after the initial
    /// loss.
    #[serde(rename = "reconnecting")]
    Reconnecting { attempt: u32, backoff_secs: u64 },
    /// The client successfully re-established the WebSocket after a disconnect
    /// (and re-sent `session.update`).
    ///
    /// `resumed` is **always `false`** for OpenAI Realtime: the API has no
    /// server-side resume, so a reconnect is always a fresh session that
    /// re-applies the config. The flag exists to mirror
    /// [`crate::gemini::GeminiEvent::Reconnected`] so the frontend can use one
    /// code path for both engines.
    #[serde(rename = "reconnected")]
    Reconnected { resumed: bool },
}

impl OpenAiRealtimeEvent {
    /// The only constructor for the [`OpenAiRealtimeEvent::Error`] payload
    /// (issue #145): the serialized `message` is pinned to the closed site
    /// token so raw provider / transport text can never reach logs, IPC, or
    /// the UI.
    fn error(
        site: OpenAiRealtimeErrorSite,
        category: OpenAiRealtimeErrorCategory,
        diagnostic: RuntimeDiagnostic,
    ) -> Self {
        Self::Error {
            category,
            message: site.token().to_string(),
            diagnostic,
        }
    }
}

/// Token usage metadata parsed from a `response.done` frame.
///
/// OpenAI reports usage under `response.usage` with `input_tokens`,
/// `output_tokens`, and `total_tokens`. Fields are optional because not every
/// frame carries them; a missing field serializes as `null` so the frontend
/// can distinguish "zero" from "not reported".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct UsageMetadata {
    // The OpenAI Realtime wire format uses snake_case for usage counters
    // (`input_tokens` / `output_tokens` / `total_tokens`), so these field names
    // map 1:1 — no `rename_all` (a camelCase rename would silently drop them).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u32>,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for an OpenAI Realtime S2S voice session.
#[derive(Clone)]
pub struct OpenAiRealtimeConfig {
    /// OpenAI API key (Bearer token). Hydrated at runtime from
    /// `credentials.yaml` (`openai_api_key`) — never persisted in settings.
    pub api_key: String,
    /// Realtime voice model id. Defaults to [`DEFAULT_MODEL`].
    pub model: String,
    /// Prebuilt voice name (e.g. `"alloy"`, `"marin"`). Empty falls back to
    /// [`DEFAULT_VOICE`].
    pub voice: String,
    /// Sample rate advertised to the provider. GA only supports 24 kHz; the
    /// client resamples the pipeline's 16 kHz audio up to this rate before
    /// sending.
    pub sample_rate: u32,
    /// Runtime privacy guard for session audio egress.
    pub content_egress_policy: crate::asr::ProviderContentEgressPolicy,
}

impl std::fmt::Debug for OpenAiRealtimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiRealtimeConfig")
            .field(
                "api_key",
                &crate::credentials::redacted_secret_presence(Some(&self.api_key)),
            )
            .field("model", &self.model)
            .field("voice", &self.voice)
            .field("sample_rate", &self.sample_rate)
            .field("content_egress_policy", &self.content_egress_policy)
            .finish()
    }
}

impl Default for OpenAiRealtimeConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: DEFAULT_MODEL.to_string(),
            voice: DEFAULT_VOICE.to_string(),
            sample_rate: REALTIME_SAMPLE_RATE,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::default(),
        }
    }
}

impl OpenAiRealtimeConfig {
    /// Construct an S2S **voice** config (the converse default). An empty
    /// `voice` falls back to [`DEFAULT_VOICE`]. The content-egress policy
    /// starts blocked (`explicit_policy_required`) — the command layer threads
    /// in the real policy from the user's privacy mode before connecting,
    /// mirroring [`crate::gemini::GeminiConfig::audio`].
    pub fn audio(
        api_key: impl Into<String>,
        model: impl Into<String>,
        voice: impl Into<String>,
    ) -> Self {
        let voice = voice.into();
        let voice = if voice.trim().is_empty() {
            DEFAULT_VOICE.to_string()
        } else {
            voice
        };
        Self {
            api_key: api_key.into(),
            model: model.into(),
            voice,
            sample_rate: REALTIME_SAMPLE_RATE,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::default(),
        }
    }

    /// Construct a **text-output** config (a degenerate S2S session that
    /// requests `output_modalities: ["text"]` instead of audio). Provided to
    /// mirror [`crate::gemini::GeminiConfig::text`]; the converse path uses
    /// [`Self::audio`].
    pub fn text(api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            model: model.into(),
            voice: DEFAULT_VOICE.to_string(),
            sample_rate: REALTIME_SAMPLE_RATE,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::default(),
        }
    }

    /// Set the runtime content-egress policy (builder-style).
    pub fn with_content_egress_policy(
        mut self,
        policy: crate::asr::ProviderContentEgressPolicy,
    ) -> Self {
        self.content_egress_policy = policy;
        self
    }
}

// ---------------------------------------------------------------------------
// Internal command passed from sync send_audio()/end_user_turn() → async writer
// ---------------------------------------------------------------------------

enum AudioCmd {
    /// Base64-encoded PCM16 24 kHz chunk ready to send as
    /// `input_audio_buffer.append`.
    Chunk(String),
    /// End the current user turn: commit the buffered audio and request a
    /// response, WITHOUT closing the socket. The S2S binding for ADR-0018
    /// `TurnAction::EndUserTurn`.
    EndTurn,
    /// Signal end of audio stream and close.
    Stop,
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

/// An OpenAI Realtime S2S voice-agent client.
///
/// The public methods (`connect`, `send_audio`, `end_user_turn`, `disconnect`,
/// `event_rx`, `is_connected`) are all **synchronous** — they block the
/// caller's thread just long enough to hand off work to the internal async
/// runtime. This matches the threading model used by `commands.rs` where worker
/// threads run in `std::thread`.
pub struct OpenAiRealtimeClient {
    config: OpenAiRealtimeConfig,
    /// crossbeam event channel — writer side (background reader task pushes here).
    event_tx: crossbeam_channel::Sender<OpenAiRealtimeEvent>,
    /// crossbeam event channel — reader side (command layer clones this).
    event_rx: crossbeam_channel::Receiver<OpenAiRealtimeEvent>,
    /// Whether the WebSocket is connected **and the session has been confirmed**
    /// (`session.updated`). Set to `true` only when the readiness frame is
    /// parsed — never merely on socket open — so it matches the contract of
    /// [`OpenAiRealtimeEvent::Connected`].
    connected: Arc<AtomicBool>,
    /// Set to `true` when the user has explicitly called `disconnect()`.
    user_disconnected: Arc<AtomicBool>,
    /// One-shot guard ensuring `Disconnected` is emitted **at most once** per
    /// teardown (mirrors the STT sibling's `emit_disconnected_once`).
    disconnected_emitted: Arc<AtomicBool>,
    /// Tokio runtime that owns the WebSocket tasks.
    rt: Option<tokio::runtime::Runtime>,
    /// Sender for audio commands → async writer task.
    audio_tx: Option<tokio_mpsc::Sender<AudioCmd>>,
    /// Handle to the session task (owns both halves + reconnect logic). Kept
    /// alive for as long as the client is connected; dropped on `Drop`.
    _session_handle: Option<tokio::task::JoinHandle<()>>,
}

impl OpenAiRealtimeClient {
    /// Create a new (disconnected) OpenAI Realtime S2S client.
    pub fn new(config: OpenAiRealtimeConfig) -> Self {
        let (event_tx, event_rx) = crossbeam_channel::bounded(256);
        Self {
            config,
            event_tx,
            event_rx,
            connected: Arc::new(AtomicBool::new(false)),
            user_disconnected: Arc::new(AtomicBool::new(false)),
            disconnected_emitted: Arc::new(AtomicBool::new(false)),
            rt: None,
            audio_tx: None,
            _session_handle: None,
        }
    }

    // ------------------------------------------------------------------
    // Connect
    // ------------------------------------------------------------------

    /// Connect to the OpenAI Realtime API and configure an S2S voice session.
    ///
    /// Blocks the caller until the WebSocket is open and the `session.update`
    /// has been sent, then spawns a background session task on an internal
    /// tokio runtime. The session task handles audio writing, server message
    /// reading, and automatic reconnect with exponential backoff (re-sending
    /// `session.update` on each reconnect — there is no server-side resume).
    /// Issue #145: `connect` returns a closed [`AppError`] (typed
    /// `RuntimeDiagnostic` variant) — never a raw provider or transport
    /// string.
    pub fn connect(&mut self) -> Result<(), crate::error::AppError> {
        if self.config.api_key.trim().is_empty() {
            // Issue #145: closed `invalid_configuration` diagnostic, no
            // prose.
            return Err(
                RuntimeDiagnostic::Runtime(RuntimeErrorDiagnostic::invalid_configuration(
                    realtime_context(),
                ))
                .into(),
            );
        }

        // Build a dedicated multi-threaded (1 worker) tokio runtime for the WS.
        // Issue #145: unclassified native failure → closed `internal`
        // diagnostic; the source error is dropped, never logged.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("openai-realtime-s2s-rt")
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
        let disconnected_emitted = Arc::clone(&self.disconnected_emitted);
        // Reset on (re)connect so a prior teardown flag doesn't poison a fresh
        // session. `connected` flips true only when the reader parses
        // `session.updated`.
        user_disconnected.store(false, Ordering::SeqCst);
        connected.store(false, Ordering::SeqCst);
        disconnected_emitted.store(false, Ordering::SeqCst);

        // Perform the blocking initial connect + session.update inside the
        // runtime so the caller sees auth / network errors immediately rather
        // than through the reconnect loop. We deliberately do NOT emit
        // `Connected` here — the socket is merely open and `session.update`
        // sent-but-not-acked; the session task emits `Connected` once the
        // server confirms with `session.updated`.
        let (audio_tx, session_handle) = rt.block_on(async move {
            let (writer, reader) = open_ws(&config).await.map_err(|e| {
                // Issue #145: log only the closed category and return the
                // closed diagnostic envelope; the connect error's raw source
                // never crossed this boundary.
                log::warn!("OpenAI Realtime S2S: connect failed ({:?})", e.category);
                crate::error::AppError::from(e.diagnostic)
            })?;

            log::info!("OpenAI Realtime S2S: WebSocket open; awaiting session.updated");

            let (atx, arx) = tokio_mpsc::channel::<AudioCmd>(OPENAI_S2S_AUDIO_QUEUE_CAP);

            let session_handle = tokio::spawn(session_task(OpenAiRealtimeSessionCtx {
                writer,
                reader,
                audio_rx: arx,
                config,
                event_tx,
                connected,
                user_disconnected,
                disconnected_emitted,
                #[cfg(test)]
                reconnect_opener: None,
                #[cfg(test)]
                run_io_entries: None,
            }));

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

    /// Send PCM audio data to OpenAI for the live voice turn.
    ///
    /// The audio should be **f32 mono 16 kHz** (matching the pipeline output).
    /// The method resamples to the configured 24 kHz rate, converts to 16-bit
    /// LE PCM, base64-encodes, and queues an `input_audio_buffer.append`.
    /// Returns immediately (non-blocking).
    ///
    /// Only `user_disconnected` is checked — not the transient `connected` flag
    /// — so the caller can keep streaming audio during a reconnect cycle; queued
    /// chunks flush as soon as the new socket is open.
    pub fn send_audio(&self, audio: &[f32]) -> Result<(), String> {
        if self.user_disconnected.load(Ordering::SeqCst) {
            return Err("OpenAI Realtime S2S client has been disconnected".to_string());
        }

        if audio.is_empty() {
            return Ok(());
        }

        self.config.content_egress_policy.check_audio(PROVIDER)?;

        let tx = self
            .audio_tx
            .as_ref()
            .ok_or_else(|| "Audio channel not initialized".to_string())?;

        // f32 16 kHz → 24 kHz → i16 LE PCM → base64.
        let resampled = resample_linear(audio, PIPELINE_SAMPLE_RATE, self.config.sample_rate);
        let pcm_bytes = f32_to_i16_le_bytes(&resampled);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&pcm_bytes);

        // Non-blocking, bounded send: drop the newest chunk past the cap rather
        // than growing memory without bound (mirrors the Gemini client). A
        // closed channel is a real error the caller should see.
        match tx.try_send(AudioCmd::Chunk(b64)) {
            Ok(()) => Ok(()),
            Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                let n = OPENAI_S2S_AUDIO_DROPS.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 50 == 1 {
                    log::warn!(
                        "OpenAI Realtime S2S outbound audio queue full ({} chunks); dropping audio \
                         (total dropped: {}). The socket or reconnect is falling behind.",
                        OPENAI_S2S_AUDIO_QUEUE_CAP,
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

    /// Signal end-of-user-turn to the engine (commit the buffered audio +
    /// `response.create`) so it starts generating, **without** closing the
    /// socket. The S2S binding for ADR-0018 `TurnAction::EndUserTurn`.
    ///
    /// Best-effort, like [`Self::send_audio`]: a full queue drops the signal
    /// (server VAD still ends the turn); a closed channel is a real error.
    pub fn end_user_turn(&self) -> Result<(), String> {
        if self.user_disconnected.load(Ordering::SeqCst) {
            return Err("OpenAI Realtime S2S client has been disconnected".to_string());
        }
        let tx = self
            .audio_tx
            .as_ref()
            .ok_or_else(|| "Audio channel not initialized".to_string())?;
        match tx.try_send(AudioCmd::EndTurn) {
            Ok(()) => Ok(()),
            Err(tokio_mpsc::error::TrySendError::Full(_)) => {
                log::warn!("OpenAI Realtime S2S: end_user_turn dropped (outbound queue full)");
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

    /// Get a clone of the event receiver channel. The command layer uses this
    /// to read `OpenAiRealtimeEvent`s from a worker thread.
    pub fn event_rx(&self) -> crossbeam_channel::Receiver<OpenAiRealtimeEvent> {
        self.event_rx.clone()
    }

    // ------------------------------------------------------------------
    // Status
    // ------------------------------------------------------------------

    /// Check if the client is currently connected (session confirmed).
    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    // ------------------------------------------------------------------
    // Disconnect
    // ------------------------------------------------------------------

    /// Disconnect from OpenAI and clean up resources.
    ///
    /// Sends a close frame, marks `user_disconnected` so the session task does
    /// not auto-reconnect, and the runtime is torn down on `Drop`.
    pub fn disconnect(&self) {
        log::info!("OpenAiRealtimeClient (S2S): disconnecting (user-initiated)");

        self.user_disconnected.store(true, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);

        if let Some(ref tx) = self.audio_tx {
            let _ = tx.try_send(AudioCmd::Stop);
        }

        // Emit `Disconnected` through the one-shot guard so the session task —
        // which will independently observe this teardown — does not emit a
        // second one.
        emit_disconnected_once(&self.event_tx, &self.disconnected_emitted);
    }
}

impl Drop for OpenAiRealtimeClient {
    fn drop(&mut self) {
        self.user_disconnected.store(true, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);

        if let Some(ref tx) = self.audio_tx {
            let _ = tx.try_send(AudioCmd::Stop);
        }
        self.audio_tx = None;

        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(std::time::Duration::from_secs(3));
        }

        log::info!("OpenAiRealtimeClient (S2S): dropped");
    }
}

// ===========================================================================
// Free functions — async building blocks
// ===========================================================================

/// Classifies *why* the session dropped so downstream logs / events can be
/// precise. Every payload-carrying variant holds a closed
/// [`RuntimeDiagnostic`] (issue #145) — never a raw reason string. The
/// `Debug` render of this enum therefore only ever shows closed enum tokens.
#[derive(Debug)]
#[allow(dead_code)]
enum DisconnectKind {
    /// Remote server sent a Close frame. `category` is `None` for normal /
    /// unclassified closes (which do not surface an error event).
    ServerClose {
        category: Option<OpenAiRealtimeErrorCategory>,
        diagnostic: RuntimeDiagnostic,
    },
    /// Transport-level error (TLS, TCP reset, DNS flap, tungstenite I/O).
    NetworkError(RuntimeDiagnostic),
    /// Protocol violation — malformed frame, invalid sequence, etc.
    ProtocolError(RuntimeDiagnostic),
    /// Runtime privacy policy blocked the session (steady-state, no
    /// reconnect).
    PolicyBlocked(RuntimeDiagnostic),
    /// User called `disconnect()`. No reconnect attempt should be made.
    UserRequested,
    /// Writer task exhausted the audio command stream (caller dropped the
    /// sender). No reconnect — session is genuinely over.
    WriterEnded,
}

/// Closed diagnostic for a content-egress policy block (issue #145).
fn policy_blocked_diagnostic() -> RuntimeDiagnostic {
    realtime_diagnostic(
        RuntimeErrorCode::PolicyBlocked,
        false,
        RuntimeSafeRecoveryAction::ReviewPolicy,
        None,
        None,
    )
}

/// Classify a `tungstenite::Error` into the closed
/// [`OpenAiRealtimeErrorCategory`] + [`RuntimeDiagnostic`] pair (issue #145 —
/// sibling of `crate::gemini::classify_tungstenite_error`). The raw error
/// display text (which can echo the request, including the auth header) is
/// dropped at the call site.
fn classify_tungstenite_error(
    err: &tokio_tungstenite::tungstenite::Error,
) -> OpenAiRealtimeErrorCategory {
    use tokio_tungstenite::tungstenite::Error as E;
    match err {
        E::Http(response) => {
            let status = response.status().as_u16();
            if status == 429 {
                // Issue #145: numeric `Retry-After` → closed bucket; exact
                // seconds dropped, absence stays absent.
                OpenAiRealtimeErrorCategory::RateLimit {
                    retry_delay: crate::gemini::retry_delay_bucket_from_headers(response.headers()),
                }
            } else if (500..600).contains(&status) {
                OpenAiRealtimeErrorCategory::Server
            } else if status == 401 || status == 403 {
                OpenAiRealtimeErrorCategory::Auth
            } else {
                OpenAiRealtimeErrorCategory::Unknown
            }
        }
        E::Io(_) | E::Tls(_) | E::ConnectionClosed | E::AlreadyClosed | E::Url(_) => {
            OpenAiRealtimeErrorCategory::Network
        }
        _ => OpenAiRealtimeErrorCategory::Unknown,
    }
}

/// A classified connect failure surfaced from [`open_ws`] (issue #145). The
/// raw source (HTTP status, transport error) is inspected at the
/// construction site to pick `category` and then **discarded** — it is never
/// retained, logged, or serialized. The closed [`RuntimeDiagnostic`] is the
/// only thing that crosses to events and the returned
/// [`AppError`](crate::error::AppError).
#[derive(Debug, Clone)]
struct OpenAiRealtimeConnectError {
    category: OpenAiRealtimeErrorCategory,
    diagnostic: RuntimeDiagnostic,
}

impl OpenAiRealtimeConnectError {
    fn new(category: OpenAiRealtimeErrorCategory) -> Self {
        Self {
            diagnostic: category.runtime_diagnostic(),
            category,
        }
    }

    /// Connect failure from a tungstenite error. Category comes from
    /// `classify_tungstenite_error`. An HTTP 401/403 handshake rejection is
    /// typed through `from_http_status` (the bearer credential was sent), so
    /// a 401 is `credential_rejected` and a 403 is `permission_denied`. Only
    /// the status class survives; the response is dropped.
    fn from_tungstenite(err: &tokio_tungstenite::tungstenite::Error) -> Self {
        let category = classify_tungstenite_error(err);
        let diagnostic = match err {
            tokio_tungstenite::tungstenite::Error::Http(response)
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

    /// `Auth` failure caused by local configuration: the saved key cannot be
    /// encoded as an `Authorization` header value.
    fn invalid_configuration() -> Self {
        Self {
            category: OpenAiRealtimeErrorCategory::Auth,
            diagnostic: RuntimeDiagnostic::Runtime(RuntimeErrorDiagnostic::invalid_configuration(
                realtime_context(),
            )),
        }
    }
}

/// Build the `session.update` client event that configures a **realtime voice
/// (S2S)** session for `gpt-realtime-2`.
///
/// Requests PCM16 @ 24 kHz input + output audio, the prebuilt `voice`,
/// `output_modalities: ["audio"]`, server VAD `turn_detection` (so the model
/// can end turns on its own), and `output_audio_transcription` so the graph
/// still receives the spoken reply as text. Pure — unit-testable without a
/// socket.
fn session_update_payload(config: &OpenAiRealtimeConfig) -> Value {
    let voice = if config.voice.trim().is_empty() {
        DEFAULT_VOICE
    } else {
        config.voice.trim()
    };

    json!({
        "type": "session.update",
        "session": {
            "type": "realtime",
            "model": config.model,
            "output_modalities": ["audio"],
            "audio": {
                "input": {
                    "format": { "type": "audio/pcm", "rate": config.sample_rate },
                    "turn_detection": { "type": "server_vad" }
                },
                "output": {
                    "format": { "type": "audio/pcm", "rate": config.sample_rate },
                    "voice": voice
                }
            }
        }
    })
}

/// The realtime WebSocket URL for the given model.
fn realtime_url(model: &str) -> String {
    format!("wss://api.openai.com/v1/realtime?model={model}")
}

/// Open a fresh OpenAI Realtime WebSocket and send the initial
/// `session.update`. Used both for the initial connect and for each reconnect
/// attempt — realtime sessions cannot resume, so the voice config must be
/// re-sent on every (re)connect.
async fn open_ws(
    config: &OpenAiRealtimeConfig,
) -> Result<(WsWriter, WsReader), OpenAiRealtimeConnectError> {
    let url = realtime_url(&config.model);
    open_ws_url(config, &url).await
}

async fn open_ws_url(
    config: &OpenAiRealtimeConfig,
    url_str: &str,
) -> Result<(WsWriter, WsReader), OpenAiRealtimeConnectError> {
    // `IntoClientRequest` fills in the mandatory WebSocket upgrade headers; we
    // only layer `Authorization` on top. NO `OpenAI-Beta` header (GA only).
    // Issue #145: every failure below classifies structurally into the
    // closed category + diagnostic; the raw source text is dropped at the
    // site and never logged or serialized.
    let mut request = url_str
        .into_client_request()
        .map_err(|_| OpenAiRealtimeConnectError::new(OpenAiRealtimeErrorCategory::Unknown))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", config.api_key)
            .parse()
            .map_err(|_| OpenAiRealtimeConnectError::invalid_configuration())?,
    );

    let (ws_stream, _response) = connect_async(request)
        .await
        .map_err(|e| OpenAiRealtimeConnectError::from_tungstenite(&e))?;

    let (mut writer, reader) = ws_stream.split();

    // Configure the voice session immediately after connect, behind the egress
    // guard so a blocked privacy policy cannot leak the session config.
    // Issue #145: the policy reason text is dropped; only the closed
    // `policy_blocked` diagnostic crosses.
    config
        .content_egress_policy
        .check_json(PROVIDER)
        .map_err(|_| OpenAiRealtimeConnectError {
            category: OpenAiRealtimeErrorCategory::Unknown,
            diagnostic: policy_blocked_diagnostic(),
        })?;
    let update = session_update_payload(config).to_string();
    writer
        .send(Message::Text(update.into()))
        .await
        .map_err(|e| OpenAiRealtimeConnectError::new(classify_tungstenite_error(&e)))?;

    Ok((writer, reader))
}

#[cfg(test)]
type ReconnectOpenFuture =
    Pin<Box<dyn Future<Output = Result<(WsWriter, WsReader), OpenAiRealtimeConnectError>> + Send>>;

#[cfg(test)]
type ReconnectOpener = Arc<dyn Fn(OpenAiRealtimeConfig) -> ReconnectOpenFuture + Send + Sync>;

#[cfg(test)]
async fn open_reconnect_ws(
    config: &OpenAiRealtimeConfig,
    opener: Option<&ReconnectOpener>,
) -> Result<(WsWriter, WsReader), OpenAiRealtimeConnectError> {
    if let Some(opener) = opener {
        opener(config.clone()).await
    } else {
        open_ws(config).await
    }
}

/// Backoff schedule per the resilience spec: 1s, 2s, 5s, 10s, then give up.
/// `attempt` is 1-based; returns `None` once the budget is exhausted.
fn backoff_for_attempt(attempt: u32) -> Option<u64> {
    match attempt {
        1 => Some(1),
        2 => Some(2),
        3 => Some(5),
        4 => Some(10),
        _ => None,
    }
}

/// Emit [`OpenAiRealtimeEvent::Disconnected`] exactly once across all the
/// places that can observe a single teardown. Returns `true` if this call was
/// the one that actually emitted.
fn emit_disconnected_once(
    event_tx: &crossbeam_channel::Sender<OpenAiRealtimeEvent>,
    disconnected_emitted: &Arc<AtomicBool>,
) -> bool {
    if disconnected_emitted.swap(true, Ordering::SeqCst) {
        return false;
    }
    let _ = event_tx.send(OpenAiRealtimeEvent::Disconnected);
    true
}

/// Bundles everything `session_task` owns for a single S2S session. Collapses a
/// long function signature to one — mirrors the STT sibling's
/// `OpenAiRealtimeSessionCtx`.
struct OpenAiRealtimeSessionCtx {
    writer: WsWriter,
    reader: WsReader,
    audio_rx: tokio_mpsc::Receiver<AudioCmd>,
    config: OpenAiRealtimeConfig,
    event_tx: crossbeam_channel::Sender<OpenAiRealtimeEvent>,
    connected: Arc<AtomicBool>,
    user_disconnected: Arc<AtomicBool>,
    disconnected_emitted: Arc<AtomicBool>,
    #[cfg(test)]
    reconnect_opener: Option<ReconnectOpener>,
    #[cfg(test)]
    run_io_entries: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

/// Background task owning a single OpenAI Realtime S2S WebSocket session,
/// including reconnect logic. Mirrors the Gemini `session_task` structure. The
/// one OpenAI-specific detail is that `open_ws` re-sends `session.update` on
/// each reconnect (NO resume): every reconnect emits `Reconnected { resumed:
/// false }`.
async fn session_task(ctx: OpenAiRealtimeSessionCtx) {
    let mut writer = ctx.writer;
    let mut reader = ctx.reader;
    let mut audio_rx = ctx.audio_rx;
    let config = ctx.config;
    let event_tx = ctx.event_tx;
    let connected = ctx.connected;
    let user_disconnected = ctx.user_disconnected;
    let disconnected_emitted = ctx.disconnected_emitted;
    #[cfg(test)]
    let reconnect_opener = ctx.reconnect_opener;
    #[cfg(test)]
    let run_io_entries = ctx.run_io_entries;
    let mut reconnect_attempts: u32 = 0;
    // The readiness event `run_io` should emit when the server confirms the
    // session: `Connected` for the first session, then `Reconnected` after each
    // successful reconnect.
    let mut ready_event = OpenAiRealtimeEvent::Connected;

    loop {
        #[cfg(test)]
        if let Some(entries) = &run_io_entries {
            entries.fetch_add(1, Ordering::SeqCst);
        }

        let disconnect = run_io(RunIoCtx {
            writer: &mut writer,
            reader: &mut reader,
            audio_rx: &mut audio_rx,
            event_tx: &event_tx,
            connected: &connected,
            user_disconnected: &user_disconnected,
            ready_event: &ready_event,
            disconnected_emitted: &disconnected_emitted,
        })
        .await;

        connected.store(false, Ordering::SeqCst);

        match disconnect {
            DisconnectKind::UserRequested | DisconnectKind::WriterEnded => {
                log::info!("OpenAI Realtime S2S session: ending ({disconnect:?})");
                emit_disconnected_once(&event_tx, &disconnected_emitted);
                break;
            }
            DisconnectKind::PolicyBlocked(diagnostic) => {
                // Issue #145: log + emit only the closed diagnostic envelope;
                // the policy reason text never crossed `run_io`.
                log::warn!("OpenAI Realtime S2S session: content egress blocked");
                let _ = event_tx.send(OpenAiRealtimeEvent::error(
                    OpenAiRealtimeErrorSite::PolicyBlocked,
                    OpenAiRealtimeErrorCategory::Unknown,
                    diagnostic,
                ));
                emit_disconnected_once(&event_tx, &disconnected_emitted);
                break;
            }
            _ => {
                if user_disconnected.load(Ordering::SeqCst) {
                    emit_disconnected_once(&event_tx, &disconnected_emitted);
                    break;
                }

                log::warn!("OpenAI Realtime S2S session: disconnected — {disconnect:?}");
                emit_disconnected_once(&event_tx, &disconnected_emitted);

                let reconnected = loop {
                    reconnect_attempts += 1;
                    let Some(backoff) = backoff_for_attempt(reconnect_attempts) else {
                        // Issue #145: closed fatal event — site token +
                        // diagnostic envelope, no reconnect prose.
                        log::warn!(
                            "OpenAI Realtime S2S session: reconnect budget exhausted ({reconnect_attempts} attempts)"
                        );
                        let _ = event_tx.send(OpenAiRealtimeEvent::error(
                            OpenAiRealtimeErrorSite::ReconnectExhausted,
                            OpenAiRealtimeErrorCategory::Network,
                            OpenAiRealtimeErrorCategory::Network.runtime_diagnostic(),
                        ));
                        break false;
                    };

                    log::info!(
                        "OpenAI Realtime S2S session: reconnecting (attempt {reconnect_attempts}, backoff {backoff}s)"
                    );
                    let _ = event_tx.send(OpenAiRealtimeEvent::Reconnecting {
                        attempt: reconnect_attempts,
                        backoff_secs: backoff,
                    });

                    // Sleep for the backoff window, bailing out early on user
                    // cancellation so shutdown doesn't wait up to 10s.
                    let sleep = tokio::time::sleep(Duration::from_secs(backoff));
                    tokio::pin!(sleep);
                    loop {
                        tokio::select! {
                            _ = &mut sleep => break,
                            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                                if user_disconnected.load(Ordering::SeqCst) {
                                    log::info!("OpenAI Realtime S2S session: user cancelled during backoff");
                                    emit_disconnected_once(&event_tx, &disconnected_emitted);
                                    return;
                                }
                            }
                        }
                    }

                    if user_disconnected.load(Ordering::SeqCst) {
                        log::info!(
                            "OpenAI Realtime S2S session: user cancelled before reconnect open"
                        );
                        emit_disconnected_once(&event_tx, &disconnected_emitted);
                        return;
                    }

                    // Reconnect: `open_ws` re-sends `session.update` (no resume).
                    #[cfg(test)]
                    let reconnect_result =
                        open_reconnect_ws(&config, reconnect_opener.as_ref()).await;
                    #[cfg(not(test))]
                    let reconnect_result = open_ws(&config).await;

                    match reconnect_result {
                        Ok((new_writer, new_reader)) => {
                            writer = new_writer;
                            reader = new_reader;
                            // Do NOT flip `connected` / emit `Reconnected` here:
                            // the socket is open but the session is not yet
                            // confirmed. `run_io` emits `ready_event` on
                            // `session.updated`. NO resume → resumed=false.
                            ready_event = OpenAiRealtimeEvent::Reconnected { resumed: false };
                            log::info!(
                                "OpenAI Realtime S2S session: socket reopened on attempt {reconnect_attempts}; awaiting session.updated"
                            );
                            reconnect_attempts = 0;
                            break true;
                        }
                        Err(e) => {
                            // Issue #145: log + emit only the closed category
                            // and diagnostic envelope; the event `message` is
                            // a stable site token, never the connect error
                            // text.
                            log::warn!(
                                "OpenAI Realtime S2S session: reconnect attempt {reconnect_attempts} failed ({:?})",
                                e.category,
                            );
                            let _ = event_tx.send(OpenAiRealtimeEvent::error(
                                OpenAiRealtimeErrorSite::ReconnectFailed,
                                e.category.clone(),
                                e.diagnostic.clone(),
                            ));
                            // Stay in the reconnect ladder; do not re-enter
                            // run_io with the previous closed socket.
                            continue;
                        }
                    }
                };

                if reconnected {
                    continue;
                }
                break;
            }
        }
    }

    connected.store(false, Ordering::SeqCst);
    log::info!("OpenAI Realtime S2S: session task exited");
}

/// Everything a single [`run_io`] invocation borrows from its owning
/// [`session_task`].
struct RunIoCtx<'a> {
    writer: &'a mut WsWriter,
    reader: &'a mut WsReader,
    audio_rx: &'a mut tokio_mpsc::Receiver<AudioCmd>,
    event_tx: &'a crossbeam_channel::Sender<OpenAiRealtimeEvent>,
    /// Flipped to `true` when this socket's `session.updated` is parsed.
    connected: &'a Arc<AtomicBool>,
    user_disconnected: &'a Arc<AtomicBool>,
    /// The event to emit once the server confirms the session (`Connected` on
    /// the first socket, `Reconnected` after a reconnect).
    ready_event: &'a OpenAiRealtimeEvent,
    /// Re-armed when readiness is confirmed so a later terminal stop after a
    /// successful reconnect can emit a fresh `Disconnected`.
    disconnected_emitted: &'a Arc<AtomicBool>,
}

/// Pumps audio out and assistant events back for a single WebSocket instance.
///
/// Returns the classified [`DisconnectKind`] when the socket breaks or the
/// caller asks to stop.
async fn run_io(ctx: RunIoCtx<'_>) -> DisconnectKind {
    let RunIoCtx {
        writer,
        reader,
        audio_rx,
        event_tx,
        connected,
        user_disconnected,
        ready_event,
        disconnected_emitted,
    } = ctx;

    // Tracks whether this socket's `session.updated` has been seen so the
    // readiness event and the `connected` flag are raised exactly once.
    let mut session_confirmed = false;

    loop {
        tokio::select! {
            cmd = audio_rx.recv() => {
                match cmd {
                    Some(AudioCmd::Chunk(b64)) => {
                        let payload = json!({ "type": "input_audio_buffer.append", "audio": b64 });
                        if let Err(e) = writer.send(Message::Text(payload.to_string().into())).await {
                            // Issue #145: classify structurally; drop the
                            // transport text from logs + the payload.
                            let category = classify_tungstenite_error(&e);
                            log::warn!("OpenAI Realtime S2S: audio send failed ({category:?})");
                            return DisconnectKind::NetworkError(category.runtime_diagnostic());
                        }
                    }
                    Some(AudioCmd::EndTurn) => {
                        // Commit the buffered audio + ask the model to respond,
                        // WITHOUT closing the socket (the session stays open for
                        // the assistant reply + the next turn).
                        let commit = json!({ "type": "input_audio_buffer.commit" });
                        if let Err(e) = writer.send(Message::Text(commit.to_string().into())).await {
                            let category = classify_tungstenite_error(&e);
                            log::warn!("OpenAI Realtime S2S: commit send failed ({category:?})");
                            return DisconnectKind::NetworkError(category.runtime_diagnostic());
                        }
                        let create = json!({ "type": "response.create" });
                        if let Err(e) = writer.send(Message::Text(create.to_string().into())).await {
                            let category = classify_tungstenite_error(&e);
                            log::warn!("OpenAI Realtime S2S: response.create send failed ({category:?})");
                            return DisconnectKind::NetworkError(category.runtime_diagnostic());
                        }
                    }
                    Some(AudioCmd::Stop) => {
                        // Graceful user-initiated close: commit any buffered
                        // audio so the trailing utterance still completes, then
                        // close.
                        let commit = json!({ "type": "input_audio_buffer.commit" });
                        let _ = writer.send(Message::Text(commit.to_string().into())).await;
                        let _ = writer.close().await;
                        return DisconnectKind::UserRequested;
                    }
                    None => {
                        // Caller dropped the sender — end without reconnecting.
                        let _ = writer.close().await;
                        return DisconnectKind::WriterEnded;
                    }
                }
            }

            result = reader.next() => {
                let Some(result) = result else {
                    return DisconnectKind::NetworkError(
                        OpenAiRealtimeErrorCategory::Network.runtime_diagnostic(),
                    );
                };

                match result {
                    Ok(Message::Text(text)) => {
                        if handle_server_message(&text, event_tx) && !session_confirmed {
                            session_confirmed = true;
                            connected.store(true, Ordering::SeqCst);
                            disconnected_emitted.store(false, Ordering::SeqCst);
                            let _ = event_tx.send(ready_event.clone());
                        }
                    }
                    Ok(Message::Close(_)) => {
                        // Issue #145: the close frame is not logged or
                        // retained — only the closed Network diagnostic
                        // crosses (uncategorized close; no error event).
                        log::info!("OpenAI Realtime S2S: server closed connection");
                        if user_disconnected.load(Ordering::SeqCst) {
                            return DisconnectKind::UserRequested;
                        }
                        return DisconnectKind::ServerClose {
                            category: None,
                            diagnostic: OpenAiRealtimeErrorCategory::Network.runtime_diagnostic(),
                        };
                    }
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {
                        // Protocol-level frames; nothing to do.
                    }
                    Ok(Message::Binary(_)) => {
                        log::debug!("OpenAI Realtime S2S: unexpected binary message");
                    }
                    Err(tungstenite::Error::ConnectionClosed)
                    | Err(tungstenite::Error::AlreadyClosed) => {
                        return DisconnectKind::NetworkError(
                            OpenAiRealtimeErrorCategory::Network.runtime_diagnostic(),
                        );
                    }
                    Err(tungstenite::Error::Protocol(_)) => {
                        return DisconnectKind::ProtocolError(
                            OpenAiRealtimeErrorCategory::Unknown.runtime_diagnostic(),
                        );
                    }
                    Err(e) => {
                        // Issue #145: classify structurally; drop the
                        // transport error text from logs + the payload.
                        let category = classify_tungstenite_error(&e);
                        log::warn!("OpenAI Realtime S2S: WebSocket read error ({category:?})");
                        return DisconnectKind::NetworkError(category.runtime_diagnostic());
                    }
                }
            }
        }
    }
}

/// Parse a single OpenAI Realtime S2S server JSON message and emit appropriate
/// events.
///
/// Returns `true` iff the message is a session-readiness frame
/// (`session.updated` / `session.created`), signalling the caller that the
/// session is now configured and it may emit `Connected`/`Reconnected`. The
/// readiness *event* is emitted by the caller (so the once-only gating lives
/// next to the `connected` flag), keeping this parser free of cross-message
/// state.
pub(crate) fn handle_server_message(
    text: &str,
    tx: &crossbeam_channel::Sender<OpenAiRealtimeEvent>,
) -> bool {
    let parsed: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            // Issue #145: classify structurally (unparseable frame → closed
            // internal code); the serde error text (which quotes the raw
            // frame) is dropped here and never logged.
            log::warn!("OpenAI Realtime S2S: invalid server JSON");
            let _ = tx.send(OpenAiRealtimeEvent::error(
                OpenAiRealtimeErrorSite::InvalidJson,
                OpenAiRealtimeErrorCategory::Unknown,
                OpenAiRealtimeErrorCategory::Unknown.runtime_diagnostic(),
            ));
            return false;
        }
    };

    let msg_type = parsed.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match msg_type {
        "response.output_audio.delta" | "response.audio.delta" => {
            // Assistant output audio: base64 PCM16-LE @ 24 kHz. Carry the
            // base64 verbatim; the converse adapter decodes at the point of use
            // (mirrors GeminiEvent::AudioChunk).
            if let Some(delta) = parsed.get("delta").and_then(|d| d.as_str())
                && !delta.is_empty()
            {
                let _ = tx.send(OpenAiRealtimeEvent::Audio {
                    data_base64: delta.to_string(),
                    sample_rate: REALTIME_SAMPLE_RATE,
                });
            }
            false
        }
        "response.output_audio_transcript.delta" | "response.audio_transcript.delta" => {
            if let Some(delta) = parsed.get("delta").and_then(|d| d.as_str())
                && !delta.is_empty()
            {
                let _ = tx.send(OpenAiRealtimeEvent::OutputTranscription {
                    text: delta.to_string(),
                });
            }
            false
        }
        "input_audio_buffer.speech_started" => {
            let _ = tx.send(OpenAiRealtimeEvent::UserSpeechStarted);
            false
        }
        "input_audio_buffer.speech_stopped" => {
            let _ = tx.send(OpenAiRealtimeEvent::UserSpeechStopped);
            false
        }
        "response.done" => {
            let usage = parsed
                .get("response")
                .and_then(|r| r.get("usage"))
                .and_then(|u| serde_json::from_value::<UsageMetadata>(u.clone()).ok());
            let _ = tx.send(OpenAiRealtimeEvent::TurnComplete { usage });
            false
        }
        "error" => {
            // Issue #145: classify structurally; the event message is the
            // closed site token and the provider body is never echoed.
            let (category, diagnostic) = classify_error_frame(parsed.get("error"));
            let _ = tx.send(OpenAiRealtimeEvent::error(
                OpenAiRealtimeErrorSite::ErrorFrame,
                category,
                diagnostic,
            ));
            false
        }
        "session.updated" | "session.created" => {
            log::debug!("OpenAI Realtime S2S: {msg_type} (session configured)");
            true
        }
        _ => {
            // Many informational events (response.created, output_item.added,
            // rate_limits.updated, etc.) are expected and not actionable here.
            // Issue #145: fixed label — the provider-controlled `type` string
            // is never logged.
            log::debug!("OpenAI Realtime S2S: unhandled message type unknown");
            false
        }
    }
}

/// Classify a top-level `error` frame's `error` object into the closed
/// category + [`RuntimeDiagnostic`] pair (issue #145). The provider `type` /
/// `code` / `message` fields are inspected **only** to pick the category and
/// are then discarded — never echoed, length-reported, or retained, so no
/// provider error body, request id, or exact length can reach logs, IPC
/// events, or UI error state.
fn classify_error_frame(error: Option<&Value>) -> (OpenAiRealtimeErrorCategory, RuntimeDiagnostic) {
    let error_type = error.and_then(|e| e.get("type")).and_then(Value::as_str);
    let code = error.and_then(|e| e.get("code")).and_then(Value::as_str);

    // Exact structured codes only — never substring or provider prose.
    // `session_expired` is the realtime session-duration cap (master's STT
    // classifier maps it to `timeout` too); it keeps the terminal
    // `AuthExpired` session-end category but is NOT a credential problem.
    let category = match (error_type, code) {
        (_, Some("rate_limit_exceeded")) => {
            OpenAiRealtimeErrorCategory::RateLimit { retry_delay: None }
        }
        (_, Some("session_expired")) => OpenAiRealtimeErrorCategory::AuthExpired,
        (_, Some("invalid_api_key")) => OpenAiRealtimeErrorCategory::Auth,
        (Some("server_error"), _) => OpenAiRealtimeErrorCategory::Server,
        _ => OpenAiRealtimeErrorCategory::Unknown,
    };

    (category.clone(), category.runtime_diagnostic())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert f32 PCM samples (range −1.0 … +1.0) to little-endian i16 bytes.
fn f32_to_i16_le_bytes(samples: &[f32]) -> Vec<u8> {
    crate::audio::pcm::f32_mono_to_pcm_s16le_bytes(samples)
}

/// Linear-interpolation resampler from `from_rate` to `to_rate` (mono f32).
/// OpenAI Realtime accepts only 24 kHz; the pipeline tap is 16 kHz, so we
/// upsample each chunk. Stateless per-chunk; returns the input unchanged when
/// the rates are equal or either rate is zero. Mirrors the STT sibling's
/// resampler.
fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || from_rate == 0 || to_rate == 0 || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let out_len = ((samples.len() as f64) * ratio).round() as usize;
    if out_len == 0 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(out_len);
    let step = (from_rate as f64) / (to_rate as f64);
    for i in 0..out_len {
        let src_pos = i as f64 * step;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;
        let a = samples.get(idx).copied().unwrap_or(0.0);
        let b = samples.get(idx + 1).copied().unwrap_or(a);
        out.push(a + (b - a) * frac);
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Test-only in-process WebSocket fixture. The STT sibling reuses
/// `crate::asr::ws_fixture`, but that module is private to the `asr` module and
/// this is a top-level sibling crate module, so we keep a tiny local copy of
/// just the two helpers our tests need (`spawn_server` + `connect_client`).
#[cfg(test)]
mod ws_fixture {
    use futures_util::StreamExt;
    use std::future::Future;
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, accept_async, connect_async};

    pub(super) type ServerSocket = WebSocketStream<TcpStream>;
    pub(super) type ClientSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

    pub(super) async fn spawn_server<F, Fut, T>(handler: F) -> (String, JoinHandle<T>)
    where
        F: FnOnce(ServerSocket) -> Fut + Send + 'static,
        Fut: Future<Output = T> + Send + 'static,
        T: Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local websocket server");
        let addr = listener.local_addr().expect("local websocket addr");

        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept websocket");
            let websocket = accept_async(stream)
                .await
                .expect("server websocket handshake");
            handler(websocket).await
        });

        (format!("ws://{addr}"), handle)
    }

    pub(super) async fn connect_client(url: &str) -> ClientSocket {
        let (socket, _) = connect_async(url).await.expect("client websocket connect");
        socket
    }

    #[tokio::test(flavor = "current_thread")]
    async fn fixture_round_trips() {
        use futures_util::SinkExt as _;
        use tokio_tungstenite::tungstenite::Message;
        let (url, server) = spawn_server(|mut ws| async move {
            ws.send(Message::Text("ready".into()))
                .await
                .expect("server sends ready");
        })
        .await;
        let mut client = connect_client(&url).await;
        match client.next().await.expect("client receives ready") {
            Ok(Message::Text(text)) => assert_eq!(text, "ready"),
            other => panic!("expected ready text, got {other:?}"),
        }
        let _ = tokio::time::timeout(std::time::Duration::from_secs(1), server).await;
    }
}

#[cfg(test)]
mod tests {
    use super::ws_fixture;
    use super::*;

    fn test_config() -> OpenAiRealtimeConfig {
        OpenAiRealtimeConfig {
            api_key: "openai-test-key-sentinel".into(),
            model: DEFAULT_MODEL.into(),
            voice: DEFAULT_VOICE.into(),
            sample_rate: REALTIME_SAMPLE_RATE,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        }
    }

    fn with_blocked_content_egress(mut config: OpenAiRealtimeConfig) -> OpenAiRealtimeConfig {
        config.api_key = "openai-private-s2s-key-sentinel".into();
        config.content_egress_policy = crate::asr::ProviderContentEgressPolicy::block("local_only");
        config
    }

    #[derive(Debug, PartialEq, Eq)]
    enum ClientContentFrame {
        Text,
        Binary { byte_len: usize },
    }

    async fn first_client_content_frame(
        mut websocket: ws_fixture::ServerSocket,
    ) -> Option<ClientContentFrame> {
        match tokio::time::timeout(Duration::from_millis(250), websocket.next()).await {
            Ok(Some(Ok(Message::Text(_)))) => Some(ClientContentFrame::Text),
            Ok(Some(Ok(Message::Binary(bytes)))) => Some(ClientContentFrame::Binary {
                byte_len: bytes.len(),
            }),
            Ok(Some(Ok(Message::Close(_))))
            | Ok(Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))))
            | Ok(Some(Err(_)))
            | Ok(None)
            | Err(_) => None,
        }
    }

    async fn recv_event(
        rx: &crossbeam_channel::Receiver<OpenAiRealtimeEvent>,
        timeout: Duration,
    ) -> OpenAiRealtimeEvent {
        tokio::time::timeout(timeout, async {
            loop {
                if let Ok(event) = rx.try_recv() {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for OpenAI Realtime S2S event")
    }

    // -- config + defaults --------------------------------------------------

    #[test]
    fn defaults_match_s2s_protocol() {
        let cfg = OpenAiRealtimeConfig::default();
        assert_eq!(cfg.model, "gpt-realtime-2");
        assert_eq!(cfg.voice, "alloy");
        assert_eq!(cfg.sample_rate, 24_000);
        // Default policy must require an explicit allow (defense in depth).
        let error = cfg
            .content_egress_policy
            .check_audio(PROVIDER)
            .expect_err("default config must require explicit content-egress allow");
        assert!(error.contains("explicit_policy_required"));
    }

    #[test]
    fn audio_constructor_falls_back_to_default_voice() {
        let cfg =
            OpenAiRealtimeConfig::audio("openai-config-key-sentinel", "gpt-realtime-2", "   ");
        assert_eq!(cfg.voice, DEFAULT_VOICE);
        let cfg =
            OpenAiRealtimeConfig::audio("openai-config-key-sentinel", "gpt-realtime-2", "marin");
        assert_eq!(cfg.voice, "marin");
    }

    #[test]
    fn text_constructor_sets_model_and_key() {
        let cfg = OpenAiRealtimeConfig::text("openai-config-key-sentinel", "gpt-realtime-2");
        assert_eq!(cfg.model, "gpt-realtime-2");
        assert_eq!(cfg.api_key, "openai-config-key-sentinel");
    }

    #[test]
    fn with_content_egress_policy_overrides() {
        let cfg = OpenAiRealtimeConfig::audio("openai-config-key-sentinel", "m", "v")
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        assert!(cfg.content_egress_policy.check_audio(PROVIDER).is_ok());
    }

    #[test]
    fn config_debug_redacts_api_key() {
        let mut config = test_config();
        config.api_key = "openai-debug-secret-sentinel".into();
        let debug = format!("{config:?}");
        assert!(!debug.contains("openai-debug-secret-sentinel"));
        assert!(debug.contains("<present>"));
        assert!(debug.contains(DEFAULT_MODEL));
        assert!(debug.contains("sample_rate"));
    }

    // -- session.update payload ---------------------------------------------

    #[test]
    fn session_update_payload_configures_s2s_voice() {
        let cfg = test_config();
        let payload = session_update_payload(&cfg);
        let session = &payload["session"];
        assert_eq!(payload["type"], "session.update");
        assert_eq!(session["type"], "realtime");
        assert_eq!(session["model"], "gpt-realtime-2");
        assert_eq!(session["output_modalities"][0], "audio");
        // Input + output audio are the object PCM form at 24 kHz.
        assert_eq!(session["audio"]["input"]["format"]["type"], "audio/pcm");
        assert_eq!(session["audio"]["input"]["format"]["rate"], 24_000);
        assert_eq!(session["audio"]["output"]["format"]["rate"], 24_000);
        assert_eq!(session["audio"]["output"]["voice"], "alloy");
        // Server VAD drives turn boundaries.
        assert_eq!(
            session["audio"]["input"]["turn_detection"]["type"],
            "server_vad"
        );
    }

    #[test]
    fn session_update_uses_configured_voice() {
        let mut cfg = test_config();
        cfg.voice = "marin".into();
        let payload = session_update_payload(&cfg);
        assert_eq!(payload["session"]["audio"]["output"]["voice"], "marin");
    }

    #[test]
    fn realtime_url_carries_model() {
        assert_eq!(
            realtime_url("gpt-realtime-2"),
            "wss://api.openai.com/v1/realtime?model=gpt-realtime-2"
        );
    }

    // -- client lifecycle (no socket) ---------------------------------------

    #[test]
    fn client_new_is_disconnected() {
        let client = OpenAiRealtimeClient::new(test_config());
        assert!(!client.is_connected());
    }

    #[test]
    fn connect_fails_without_api_key() {
        let mut config = test_config();
        config.api_key.clear();
        let mut client = OpenAiRealtimeClient::new(config);
        let result = client.connect();
        assert!(result.is_err());
        // Issue #145: closed `invalid_configuration` diagnostic, never prose.
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

    /// Provider-auth typing (#518): closed code + recovery action per auth
    /// path, with the category (terminal-auth teardown) unchanged.
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
        fn http(status: u16) -> tokio_tungstenite::tungstenite::Error {
            tokio_tungstenite::tungstenite::Error::Http(Box::new(
                tokio_tungstenite::tungstenite::http::Response::builder()
                    .status(status)
                    .body(None)
                    .expect("response builds"),
            ))
        }

        assert_eq!(
            tuple(&OpenAiRealtimeErrorCategory::Auth.runtime_diagnostic()),
            (Code::CredentialRejected, false, Action::ReenterCredential)
        );
        assert_eq!(
            tuple(&OpenAiRealtimeErrorCategory::AuthExpired.runtime_diagnostic()),
            // Session-window expiry, not a credential failure.
            (Code::Timeout, true, Action::Retry)
        );
        // Error-frame auth codes route through the category.
        let (category, diagnostic) = classify_error_frame(Some(&json!({
            "type": "invalid_request_error", "code": "invalid_api_key"
        })));
        assert_eq!(category, OpenAiRealtimeErrorCategory::Auth);
        assert_eq!(
            tuple(&diagnostic),
            (Code::CredentialRejected, false, Action::ReenterCredential)
        );

        // Handshake 401 rejects the credential; 403 is a permission failure.
        let unauthorized = OpenAiRealtimeConnectError::from_tungstenite(&http(401));
        assert_eq!(unauthorized.category, OpenAiRealtimeErrorCategory::Auth);
        assert_eq!(
            tuple(&unauthorized.diagnostic),
            (Code::CredentialRejected, false, Action::ReenterCredential)
        );
        let forbidden = OpenAiRealtimeConnectError::from_tungstenite(&http(403));
        assert_eq!(forbidden.category, OpenAiRealtimeErrorCategory::Auth);
        assert_eq!(
            tuple(&forbidden.diagnostic),
            (Code::PermissionDenied, false, Action::ReviewConfiguration)
        );

        // A key that cannot be sent as a header is a configuration failure.
        let malformed = OpenAiRealtimeConnectError::invalid_configuration();
        assert_eq!(malformed.category, OpenAiRealtimeErrorCategory::Auth);
        assert_eq!(
            tuple(&malformed.diagnostic),
            (
                Code::InvalidConfiguration,
                false,
                Action::ReviewConfiguration
            )
        );

        // Non-auth handshake statuses keep their category mapping.
        assert_eq!(
            tuple(&OpenAiRealtimeConnectError::from_tungstenite(&http(503)).diagnostic),
            (Code::ProviderUnavailable, true, Action::Retry)
        );
    }

    #[test]
    fn connect_fails_with_whitespace_api_key() {
        let mut config = test_config();
        config.api_key = "   ".into();
        let mut client = OpenAiRealtimeClient::new(config);
        assert!(client.connect().is_err());
    }

    #[test]
    fn send_audio_fails_when_disconnected() {
        let client = OpenAiRealtimeClient::new(test_config());
        assert!(client.send_audio(&[0.5, -0.3]).is_err());
    }

    #[test]
    fn blocked_policy_rejects_non_empty_audio_before_channel_initialization() {
        let client = OpenAiRealtimeClient::new(with_blocked_content_egress(test_config()));
        let error = client.send_audio(&[0.5, -0.3]).unwrap_err();
        assert!(error.contains("Privacy policy blocked"));
        assert!(error.contains(PROVIDER));
        assert!(error.contains("local_only"));
        assert!(!error.contains("Audio channel not initialized"));
    }

    #[test]
    fn blocked_policy_allows_empty_audio_without_channel_initialization() {
        let client = OpenAiRealtimeClient::new(with_blocked_content_egress(test_config()));
        assert!(client.send_audio(&[]).is_ok());
    }

    #[test]
    fn blocked_policy_error_redacts_secret_values() {
        let client = OpenAiRealtimeClient::new(with_blocked_content_egress(test_config()));
        let error = client.send_audio(&[0.5, -0.3]).unwrap_err();
        for forbidden in ["openai-private-s2s-key-sentinel", "0.5", "-0.3"] {
            assert!(
                !error.contains(forbidden),
                "privacy error leaked {forbidden}: {error}"
            );
        }
    }

    #[test]
    fn end_user_turn_fails_when_channel_uninitialized() {
        let client = OpenAiRealtimeClient::new(test_config());
        assert!(client.end_user_turn().is_err());
    }

    // -- pcm helpers --------------------------------------------------------

    #[test]
    fn f32_to_i16_conversion_silence() {
        let silence = [0.0f32; 4];
        let bytes = f32_to_i16_le_bytes(&silence);
        assert_eq!(bytes.len(), 8);
        assert!(bytes.iter().all(|&b| b == 0));
    }

    #[test]
    fn f32_to_i16_conversion_full_scale() {
        let samples = [1.0f32, -1.0];
        let bytes = f32_to_i16_le_bytes(&samples);
        assert_eq!(&bytes[0..2], &i16::MAX.to_le_bytes());
        assert_eq!(&bytes[2..4], &i16::MIN.to_le_bytes());
    }

    #[test]
    fn resample_identity_when_rates_match() {
        let samples = [0.1, 0.2, -0.3, 0.4];
        assert_eq!(resample_linear(&samples, 24_000, 24_000), samples);
    }

    #[test]
    fn resample_16k_to_24k_lengthens_by_ratio() {
        let samples = vec![0.0f32; 160]; // 10 ms @ 16 kHz
        assert_eq!(resample_linear(&samples, 16_000, 24_000).len(), 240);
    }

    #[test]
    fn resample_empty_is_empty() {
        assert!(resample_linear(&[], 16_000, 24_000).is_empty());
    }

    // -- backoff ladder -----------------------------------------------------

    #[test]
    fn backoff_schedule_matches_spec() {
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(2), Some(2));
        assert_eq!(backoff_for_attempt(3), Some(5));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), None);
        assert_eq!(backoff_for_attempt(99), None);
    }

    // -- event serialization ------------------------------------------------

    #[test]
    fn event_serialization_roundtrip() {
        let events = vec![
            OpenAiRealtimeEvent::Audio {
                data_base64: "AQID".into(),
                sample_rate: 24_000,
            },
            OpenAiRealtimeEvent::OutputTranscription { text: "hi".into() },
            OpenAiRealtimeEvent::UserSpeechStarted,
            OpenAiRealtimeEvent::UserSpeechStopped,
            OpenAiRealtimeEvent::TurnComplete { usage: None },
            OpenAiRealtimeEvent::Error {
                category: OpenAiRealtimeErrorCategory::Unknown,
                message: "invalid_json".into(),
                diagnostic: OpenAiRealtimeErrorCategory::Unknown.runtime_diagnostic(),
            },
            OpenAiRealtimeEvent::Error {
                category: OpenAiRealtimeErrorCategory::RateLimit {
                    retry_delay: Some(RuntimeRetryDelayBucket::Medium),
                },
                message: "error_frame".into(),
                diagnostic: OpenAiRealtimeErrorCategory::RateLimit {
                    retry_delay: Some(RuntimeRetryDelayBucket::Medium),
                }
                .runtime_diagnostic(),
            },
            OpenAiRealtimeEvent::Connected,
            OpenAiRealtimeEvent::Disconnected,
            OpenAiRealtimeEvent::Reconnecting {
                attempt: 2,
                backoff_secs: 2,
            },
            OpenAiRealtimeEvent::Reconnected { resumed: false },
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let parsed: Value = serde_json::from_str(&json).unwrap();
            assert!(parsed.get("type").is_some(), "tagged on type: {json}");
            let _back: OpenAiRealtimeEvent = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn audio_event_tag_and_payload() {
        let json = serde_json::to_value(OpenAiRealtimeEvent::Audio {
            data_base64: "AQID".into(),
            sample_rate: 24_000,
        })
        .unwrap();
        assert_eq!(json["type"], "audio");
        assert_eq!(json["data_base64"], "AQID");
        assert_eq!(json["sample_rate"], 24_000);
    }

    #[test]
    fn error_category_serializes_with_kind_tag() {
        let rl = serde_json::to_value(OpenAiRealtimeErrorCategory::RateLimit { retry_delay: None })
            .unwrap();
        assert_eq!(rl["kind"], "rate_limit");
        assert!(rl.get("retry_delay").is_none());
        let auth = serde_json::to_value(OpenAiRealtimeErrorCategory::AuthExpired).unwrap();
        assert_eq!(auth["kind"], "auth_expired");
    }

    // -- server message parsing ---------------------------------------------

    #[test]
    fn parses_output_audio_delta() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let confirmed = handle_server_message(
            r#"{"type":"response.output_audio.delta","delta":"AQID"}"#,
            &tx,
        );
        assert!(!confirmed);
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::Audio {
                data_base64,
                sample_rate,
            } => {
                assert_eq!(data_base64, "AQID");
                assert_eq!(sample_rate, 24_000);
            }
            other => panic!("expected Audio, got {other:?}"),
        }
    }

    #[test]
    fn parses_legacy_audio_delta_alias() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message(r#"{"type":"response.audio.delta","delta":"AQID"}"#, &tx);
        assert!(matches!(
            rx.try_recv().unwrap(),
            OpenAiRealtimeEvent::Audio { .. }
        ));
    }

    #[test]
    fn empty_audio_delta_not_emitted() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message(r#"{"type":"response.output_audio.delta","delta":""}"#, &tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn parses_output_transcript_delta() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message(
            r#"{"type":"response.output_audio_transcript.delta","delta":"hello"}"#,
            &tx,
        );
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::OutputTranscription { text } => assert_eq!(text, "hello"),
            other => panic!("expected OutputTranscription, got {other:?}"),
        }
    }

    #[test]
    fn parses_speech_started_and_stopped() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message(r#"{"type":"input_audio_buffer.speech_started"}"#, &tx);
        assert!(matches!(
            rx.try_recv().unwrap(),
            OpenAiRealtimeEvent::UserSpeechStarted
        ));
        handle_server_message(r#"{"type":"input_audio_buffer.speech_stopped"}"#, &tx);
        assert!(matches!(
            rx.try_recv().unwrap(),
            OpenAiRealtimeEvent::UserSpeechStopped
        ));
    }

    #[test]
    fn parses_response_done_with_usage() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message(
            r#"{"type":"response.done","response":{"usage":{"input_tokens":10,"output_tokens":20,"total_tokens":30}}}"#,
            &tx,
        );
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::TurnComplete { usage } => {
                let u = usage.expect("usage parsed");
                assert_eq!(u.input_tokens, Some(10));
                assert_eq!(u.output_tokens, Some(20));
                assert_eq!(u.total_tokens, Some(30));
            }
            other => panic!("expected TurnComplete, got {other:?}"),
        }
    }

    #[test]
    fn parses_response_done_without_usage() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message(r#"{"type":"response.done","response":{}}"#, &tx);
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::TurnComplete { usage } => assert!(usage.is_none()),
            other => panic!("expected TurnComplete, got {other:?}"),
        }
    }

    #[test]
    fn error_frame_redacts_message_and_classifies() {
        crate::test_log_capture::install();
        const KEY_CANARY: &str = "canary-credential-value";
        const BODY: &str = "canary-provider-body-text";
        const REQUEST_ID: &str = "canary-request-identifier-openai";
        const EXACT_LEN: &str = "48213";

        let frame = json!({
            "type": "error",
            "event_id": REQUEST_ID,
            "error": {
                "type": "invalid_request_error",
                "code": "rate_limit_exceeded",
                "event_id": REQUEST_ID,
                "message": format!(
                    "{BODY} for key {KEY_CANARY} (request {REQUEST_ID}), {EXACT_LEN} tokens"
                ),
            }
        })
        .to_string();
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message(&frame, &tx);
        let event = rx.try_recv().unwrap();

        // Issue #145: the FULL wire shape — closed site token, structural
        // category, closed diagnostic, and nothing else.
        let wire = serde_json::to_value(&event).unwrap();
        assert_eq!(
            wire,
            json!({
                "type": "error",
                "category": { "kind": "rate_limit" },
                "message": "error_frame",
                "diagnostic": {
                    "kind": "runtime",
                    "detail": {
                        "code": "rate_limited",
                        "retryable": true,
                        "recovery_action": "retry_after_delay",
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
            KEY_CANARY,
            BODY,
            REQUEST_ID,
            EXACT_LEN,
            "invalid_request_error",
            "rate_limit_exceeded",
            "_len",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "serialized error event leaked `{forbidden}`: {serialized}"
            );
        }
        // The planted values must not have reached any log line either.
        crate::test_log_capture::assert_process_logs_exclude(&[
            KEY_CANARY, BODY, REQUEST_ID, EXACT_LEN,
        ]);
    }

    /// Issue #145: a valid JSON frame whose top-level `type` is unrecognized
    /// is provider-controlled text; it must never be logged verbatim.
    #[test]
    fn unknown_event_type_is_logged_with_a_fixed_label_only() {
        const SENTINEL: &str = "canary-unknown-event-type";
        let capture = crate::test_log_capture::ThreadLogCapture::start();
        let frame = json!({ "type": SENTINEL, "delta": "x" }).to_string();
        let (tx, rx) = crossbeam_channel::bounded(16);
        assert!(!handle_server_message(&frame, &tx));
        assert!(rx.try_recv().is_err(), "unknown frames emit no event");

        let logs = crate::test_log_capture::render(&capture.finish());
        assert!(
            logs.contains("unhandled message type unknown"),
            "the fixed label must be logged so the canary is not vacuous: {logs}"
        );
        assert!(!logs.contains(SENTINEL), "{logs}");
        crate::test_log_capture::assert_process_logs_exclude(&[SENTINEL]);
    }

    #[test]
    fn invalid_json_emits_error() {
        const SENTINEL: &str = "canary-not-json-body";
        let capture = crate::test_log_capture::ThreadLogCapture::start();
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message(&format!("not json {SENTINEL}"), &tx);
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::Error {
                message,
                diagnostic,
                ..
            } => {
                // Issue #145: closed site token; the serde error text (which
                // quotes the raw frame) is never surfaced.
                assert_eq!(message, "invalid_json");
                let serialized = serde_json::to_string(&diagnostic).unwrap();
                assert!(serialized.contains("internal"));
                assert!(!serialized.contains("not json"));
                assert!(!serialized.contains(SENTINEL));
            }
            other => panic!("expected Error, got {other:?}"),
        }
        let logs = crate::test_log_capture::render(&capture.finish());
        assert!(logs.contains("invalid server JSON"), "{logs}");
        assert!(!logs.contains(SENTINEL), "{logs}");
        crate::test_log_capture::assert_process_logs_exclude(&[SENTINEL]);
    }

    fn http_429(retry_after: Option<&str>) -> tokio_tungstenite::tungstenite::Error {
        let mut builder = tokio_tungstenite::tungstenite::http::Response::builder().status(429);
        if let Some(value) = retry_after {
            builder = builder.header("retry-after", value);
        }
        tokio_tungstenite::tungstenite::Error::Http(Box::new(
            builder.body(None).expect("build 429 response"),
        ))
    }

    /// Issue #145 canary (sibling of the Gemini test): a numeric
    /// `Retry-After` is reduced to a closed bucket at classification and the
    /// exact seconds never reach the category (wire or the `Debug` form the
    /// connect/reconnect logs print), the connect error, or the diagnostic.
    #[test]
    fn numeric_retry_after_is_bucketed_and_the_exact_value_never_survives() {
        const EXACT: &str = "7391";
        let category = classify_tungstenite_error(&http_429(Some(EXACT)));
        assert_eq!(
            category,
            OpenAiRealtimeErrorCategory::RateLimit {
                retry_delay: Some(RuntimeRetryDelayBucket::Long)
            }
        );
        let surfaces = [
            format!("{category:?}"),
            serde_json::to_string(&category).unwrap(),
            serde_json::to_string(&category.runtime_diagnostic()).unwrap(),
            format!("{:?}", OpenAiRealtimeConnectError::new(category.clone())),
        ];
        for surface in &surfaces {
            assert!(
                !surface.contains(EXACT),
                "exact Retry-After leaked into {surface}"
            );
        }
        assert_eq!(
            serde_json::to_value(&category).unwrap(),
            json!({ "kind": "rate_limit", "retry_delay": "long" })
        );
    }

    /// Absent or non-numeric `Retry-After` leaves the bucket absent.
    #[test]
    fn absent_or_unparseable_retry_after_leaves_the_bucket_absent() {
        for header in [None, Some("Wed, 21 Oct 2026 07:28:00 GMT")] {
            let category = classify_tungstenite_error(&http_429(header));
            assert_eq!(
                category,
                OpenAiRealtimeErrorCategory::RateLimit { retry_delay: None }
            );
            let diagnostic = serde_json::to_value(category.runtime_diagnostic()).unwrap();
            assert!(
                diagnostic["detail"]["context"].get("retry_delay").is_none(),
                "absence must not map to a bucket: {diagnostic}"
            );
        }
    }

    #[test]
    fn informational_events_are_ignored() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message(r#"{"type":"response.created"}"#, &tx);
        handle_server_message(r#"{"type":"rate_limits.updated"}"#, &tx);
        handle_server_message(r#"{"type":"conversation.item.added"}"#, &tx);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn session_updated_and_created_signal_readiness() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        assert!(handle_server_message(
            r#"{"type":"session.updated","session":{}}"#,
            &tx
        ));
        assert!(handle_server_message(
            r#"{"type":"session.created","session":{}}"#,
            &tx
        ));
        // The parser itself emits no event for readiness.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn non_session_messages_do_not_signal_readiness() {
        let (tx, _rx) = crossbeam_channel::bounded(16);
        assert!(!handle_server_message(
            r#"{"type":"response.output_audio.delta","delta":"AQID"}"#,
            &tx
        ));
        assert!(!handle_server_message(
            r#"{"type":"error","error":{"message":"boom"}}"#,
            &tx
        ));
        assert!(!handle_server_message("not json", &tx));
    }

    // -- error classification matrix ----------------------------------------

    #[test]
    fn classify_error_frame_matrix() {
        let (cat, _) = classify_error_frame(Some(&json!({
            "type": "invalid_request_error", "code": "rate_limit_exceeded"
        })));
        assert_eq!(
            cat,
            OpenAiRealtimeErrorCategory::RateLimit { retry_delay: None }
        );

        // `session_expired` is the realtime session-DURATION cap, not a
        // credential problem: the diagnosis is a retryable timeout while the
        // terminal session-end category is unchanged.
        let (cat, diagnostic) = classify_error_frame(Some(&json!({
            "type": "invalid_request_error", "code": "session_expired"
        })));
        assert_eq!(cat, OpenAiRealtimeErrorCategory::AuthExpired);
        match diagnostic {
            RuntimeDiagnostic::Runtime(e) => {
                assert_eq!(e.code, RuntimeErrorCode::Timeout);
                assert!(e.retryable);
                assert_eq!(e.recovery_action, RuntimeSafeRecoveryAction::Retry);
            }
            other => panic!("expected runtime diagnostic, got {other:?}"),
        }

        let (cat, _) = classify_error_frame(Some(&json!({
            "type": "invalid_request_error", "code": "invalid_api_key"
        })));
        assert_eq!(cat, OpenAiRealtimeErrorCategory::Auth);

        let (cat, _) = classify_error_frame(Some(&json!({ "type": "server_error" })));
        assert_eq!(cat, OpenAiRealtimeErrorCategory::Server);

        let (cat, _) = classify_error_frame(None);
        assert_eq!(cat, OpenAiRealtimeErrorCategory::Unknown);

        // Exact structured codes only: look-alike codes and types never
        // classify by substring.
        for look_alike in [
            json!({ "type": "invalid_request_error", "code": "my_rate_limit_check" }),
            json!({ "type": "invalid_request_error", "code": "token_expired" }),
            json!({ "type": "invalid_request_error", "code": "expired_signature" }),
            json!({ "type": "invalid_request_error", "code": "no_api_key_here" }),
            json!({ "type": "authentication_failed" }),
            json!({ "type": "not_a_server_error" }),
        ] {
            let (cat, _) = classify_error_frame(Some(&look_alike));
            assert_eq!(cat, OpenAiRealtimeErrorCategory::Unknown, "{look_alike}");
        }
    }

    // -- emit_disconnected_once ---------------------------------------------

    #[test]
    fn emit_disconnected_once_dedupes() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let guard = Arc::new(AtomicBool::new(false));
        assert!(emit_disconnected_once(&tx, &guard));
        assert!(!emit_disconnected_once(&tx, &guard));
        assert!(!emit_disconnected_once(&tx, &guard));
        assert!(matches!(
            rx.try_recv().unwrap(),
            OpenAiRealtimeEvent::Disconnected
        ));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn emit_disconnected_once_re_arms_per_session() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let guard = Arc::new(AtomicBool::new(false));
        assert!(emit_disconnected_once(&tx, &guard));
        assert!(matches!(
            rx.try_recv().unwrap(),
            OpenAiRealtimeEvent::Disconnected
        ));
        // run_io re-arms the guard for a fresh session after readiness.
        guard.store(false, Ordering::SeqCst);
        assert!(emit_disconnected_once(&tx, &guard));
        assert!(matches!(
            rx.try_recv().unwrap(),
            OpenAiRealtimeEvent::Disconnected
        ));
    }

    // -- open_ws egress guard (in-process socket) ---------------------------

    #[tokio::test(flavor = "current_thread")]
    async fn open_ws_blocked_policy_writes_no_session_update_frame() {
        let capture = crate::test_log_capture::ThreadLogCapture::start();
        let (frame_tx, frame_rx) = tokio::sync::oneshot::channel();
        let (url, server) = ws_fixture::spawn_server(move |websocket| async move {
            let _ = frame_tx.send(first_client_content_frame(websocket).await);
        })
        .await;

        // Key unique to this test (the shared fixture's key is also planted by
        // other tests, which would make a process-wide exclusion ambiguous).
        const BLOCKED_KEY: &str = "canary-blocked-policy-credential";
        let mut config = with_blocked_content_egress(test_config());
        config.api_key = BLOCKED_KEY.into();
        let error = open_ws_url(&config, &url)
            .await
            .expect_err("blocked policy should reject S2S session.update write");

        // Issue #145: the failure is the closed `policy_blocked` diagnostic
        // — the raw policy reason (which quotes the provider and mode) is
        // dropped at the classify site, and the sentinel key cannot leak.
        // Assert the FULL wire shape of the `AppError` the command layer
        // returns (`connect()` converts the connect error exactly this way).
        // (the log capture was started before the action, above)
        let wire = serde_json::to_value(crate::error::AppError::from(error.diagnostic.clone()))
            .expect("AppError serializes");
        assert_eq!(
            wire,
            json!({
                "code": "runtime_diagnostic",
                "diagnostic": {
                    "kind": "runtime",
                    "detail": {
                        "code": "policy_blocked",
                        "retryable": false,
                        "recovery_action": "review_policy",
                        "context": {
                            "operation": "realtime_conversation",
                            "transport": "websocket"
                        }
                    }
                }
            })
        );
        let serialized = wire.to_string();
        for forbidden in [
            BLOCKED_KEY,
            PROVIDER,
            "local_only",
            "Privacy policy blocked",
        ] {
            assert!(
                !serialized.contains(forbidden),
                "returned error leaked `{forbidden}`: {serialized}"
            );
        }
        // The connect error's `Debug` form (what any `{:?}` log would print)
        // is closed too.
        let debug = format!("{error:?}");
        for forbidden in [BLOCKED_KEY, "local_only"] {
            assert!(
                !debug.contains(forbidden),
                "connect error Debug leaked `{forbidden}`: {debug}"
            );
        }
        // The thread-scoped capture was started BEFORE the action; the
        // process-wide exclusion uses a key unique to this test.
        let logs = crate::test_log_capture::render(&capture.finish());
        assert!(!logs.contains(BLOCKED_KEY), "{logs}");
        crate::test_log_capture::assert_process_logs_exclude(&[BLOCKED_KEY]);

        let observed = tokio::time::timeout(Duration::from_secs(1), frame_rx)
            .await
            .expect("server should report whether a content frame arrived")
            .expect("server frame channel should not drop");
        assert_eq!(
            observed, None,
            "blocked session.update must not write a content frame"
        );

        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_ws_sends_session_update_on_open() {
        let (text_tx, text_rx) = tokio::sync::oneshot::channel();
        let (url, server) = ws_fixture::spawn_server(move |mut websocket| async move {
            let first = match tokio::time::timeout(Duration::from_secs(1), websocket.next()).await {
                Ok(Some(Ok(Message::Text(t)))) => Some(t.to_string()),
                _ => None,
            };
            let _ = text_tx.send(first);
        })
        .await;

        let config = test_config();
        let (_writer, _reader) = open_ws_url(&config, &url)
            .await
            .expect("open + session.update should succeed");

        let first = tokio::time::timeout(Duration::from_secs(1), text_rx)
            .await
            .expect("server reports first frame")
            .expect("server frame channel should not drop")
            .expect("first frame must be the session.update");
        assert!(first.contains(r#""type":"session.update""#));
        assert!(first.contains(r#""type":"realtime""#));
        assert!(first.contains(r#""output_modalities":["audio"]"#));

        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
    }

    // -- session_task: reconnect ladder (mockable, NO live key) -------------

    /// A reconnect open failure must NOT re-enter `run_io` with the stale
    /// (closed) socket halves — it stays in the reconnect ladder. Reuses the
    /// injectable `ReconnectOpener` + in-process fixture pattern from the STT
    /// sibling so no live OpenAI key is needed.
    #[tokio::test(flavor = "current_thread")]
    async fn reconnect_open_failure_does_not_reenter_run_io_on_stale_socket() {
        let (url, server) = ws_fixture::spawn_server(|mut websocket| async move {
            let _ = websocket.close(None).await;
        })
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (writer, reader) = client_socket.split();
        let (_audio_tx, audio_rx) = tokio_mpsc::channel(8);
        let (event_tx, event_rx) = crossbeam_channel::bounded(16);
        let connected = Arc::new(AtomicBool::new(true));
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let disconnected_emitted = Arc::new(AtomicBool::new(false));
        let run_io_entries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let opener_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let opener: ReconnectOpener = {
            let opener_calls = Arc::clone(&opener_calls);
            Arc::new(move |_config| {
                let opener_calls = Arc::clone(&opener_calls);
                Box::pin(async move {
                    opener_calls.fetch_add(1, Ordering::SeqCst);
                    Err(OpenAiRealtimeConnectError::new(
                        OpenAiRealtimeErrorCategory::Network,
                    ))
                })
            })
        };

        let handle = tokio::spawn(session_task(OpenAiRealtimeSessionCtx {
            writer,
            reader,
            audio_rx,
            config: test_config(),
            event_tx,
            connected: Arc::clone(&connected),
            user_disconnected: Arc::clone(&user_disconnected),
            disconnected_emitted,
            reconnect_opener: Some(opener),
            run_io_entries: Some(Arc::clone(&run_io_entries)),
        }));

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            OpenAiRealtimeEvent::Disconnected => {}
            other => panic!("expected initial Disconnected, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            OpenAiRealtimeEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                assert_eq!(attempt, 1);
                assert_eq!(backoff_secs, 1);
            }
            other => panic!("expected first Reconnecting, got {other:?}"),
        }
        assert_eq!(run_io_entries.load(Ordering::SeqCst), 1);

        match recv_event(&event_rx, Duration::from_secs(2)).await {
            OpenAiRealtimeEvent::Error {
                category, message, ..
            } => {
                // Issue #145: closed site token + structural category; the
                // connect error text never crosses.
                assert_eq!(message, "reconnect_failed");
                assert_eq!(category, OpenAiRealtimeErrorCategory::Network);
            }
            other => panic!("expected reconnect failure error, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            OpenAiRealtimeEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                assert_eq!(attempt, 2);
                assert_eq!(backoff_secs, 2);
            }
            other => panic!("expected second Reconnecting, got {other:?}"),
        }
        assert_eq!(opener_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            run_io_entries.load(Ordering::SeqCst),
            1,
            "failed reconnect must not re-enter run_io with stale socket halves"
        );

        user_disconnected.store(true, Ordering::SeqCst);
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("session task should exit during reconnect backoff")
            .expect("session task panicked");
        assert!(!connected.load(Ordering::SeqCst));
        assert_eq!(opener_calls.load(Ordering::SeqCst), 1);
        assert!(
            event_rx
                .try_iter()
                .all(|event| !matches!(event, OpenAiRealtimeEvent::Reconnected { .. })),
            "cancel during backoff must not emit Reconnected"
        );

        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;
    }

    /// A successful reconnect (fresh socket re-sending `session.update`) emits
    /// `Reconnected { resumed: false }` only after readiness, re-arms the
    /// `Disconnected` guard, and forwards assistant audio from the fresh socket.
    #[tokio::test(flavor = "current_thread")]
    async fn session_task_successful_reconnect_emits_reconnected_after_readiness() {
        let (initial_url, initial_server) = ws_fixture::spawn_server(|mut websocket| async move {
            let _ = websocket.close(None).await;
        })
        .await;

        let client_socket = ws_fixture::connect_client(&initial_url).await;
        let (writer, reader) = client_socket.split();
        let (audio_tx, audio_rx) = tokio_mpsc::channel(8);
        let (event_tx, event_rx) = crossbeam_channel::bounded(32);
        let connected = Arc::new(AtomicBool::new(true));
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let disconnected_emitted = Arc::new(AtomicBool::new(false));
        let run_io_entries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let opener_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (text_frames_tx, mut text_frames_rx) =
            tokio::sync::mpsc::unbounded_channel::<Vec<String>>();

        let opener: ReconnectOpener = {
            let opener_calls = Arc::clone(&opener_calls);
            Arc::new(move |config| {
                let opener_calls = Arc::clone(&opener_calls);
                let text_frames_tx = text_frames_tx.clone();
                Box::pin(async move {
                    opener_calls.fetch_add(1, Ordering::SeqCst);
                    let (url, _server) =
                        ws_fixture::spawn_server(move |mut websocket| async move {
                            let mut text_frames = Vec::new();
                            while let Some(frame) = websocket.next().await {
                                match frame.expect("reconnected S2S server frame") {
                                    Message::Text(text) => {
                                        let text = text.to_string();
                                        let msg_type = serde_json::from_str::<Value>(&text)
                                            .ok()
                                            .and_then(|v| {
                                                v.get("type")
                                                    .and_then(Value::as_str)
                                                    .map(str::to_string)
                                            });
                                        text_frames.push(text);
                                        match msg_type.as_deref() {
                                            Some("session.update") => {
                                                websocket
                                                    .send(Message::Text(
                                                        r#"{"type":"session.updated","session":{}}"#
                                                            .into(),
                                                    ))
                                                    .await
                                                    .expect("send session.updated");
                                                websocket
                                                    .send(Message::Text(
                                                        r#"{"type":"response.output_audio.delta","delta":"AQID"}"#
                                                            .into(),
                                                    ))
                                                    .await
                                                    .expect("send audio delta");
                                            }
                                            Some("input_audio_buffer.commit") => break,
                                            _ => {}
                                        }
                                    }
                                    Message::Close(_) => break,
                                    _ => {}
                                }
                            }
                            let _ = text_frames_tx.send(text_frames);
                        })
                        .await;

                    let socket = ws_fixture::connect_client(&url).await;
                    let (mut writer, reader) = socket.split();
                    config
                        .content_egress_policy
                        .check_json(PROVIDER)
                        .map_err(|_| OpenAiRealtimeConnectError {
                            category: OpenAiRealtimeErrorCategory::Unknown,
                            diagnostic: policy_blocked_diagnostic(),
                        })?;
                    let update = session_update_payload(&config).to_string();
                    writer
                        .send(Message::Text(update.into()))
                        .await
                        .map_err(|e| {
                            OpenAiRealtimeConnectError::new(classify_tungstenite_error(&e))
                        })?;
                    Ok((writer, reader))
                })
            })
        };

        let handle = tokio::spawn(session_task(OpenAiRealtimeSessionCtx {
            writer,
            reader,
            audio_rx,
            config: test_config(),
            event_tx,
            connected: Arc::clone(&connected),
            user_disconnected: Arc::clone(&user_disconnected),
            disconnected_emitted,
            reconnect_opener: Some(opener),
            run_io_entries: Some(Arc::clone(&run_io_entries)),
        }));

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            OpenAiRealtimeEvent::Disconnected => {}
            other => panic!("expected initial Disconnected, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            OpenAiRealtimeEvent::Reconnecting { attempt, .. } => assert_eq!(attempt, 1),
            other => panic!("expected first Reconnecting, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(3)).await {
            OpenAiRealtimeEvent::Reconnected { resumed } => assert!(
                !resumed,
                "OpenAI Realtime has no resume — reconnect must report resumed=false"
            ),
            other => panic!("expected readiness-gated Reconnected, got {other:?}"),
        }
        assert!(connected.load(Ordering::SeqCst));

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            OpenAiRealtimeEvent::Audio { data_base64, .. } => assert_eq!(data_base64, "AQID"),
            other => panic!("expected assistant audio after reconnect, got {other:?}"),
        }

        audio_tx.send(AudioCmd::Stop).await.expect("queue stop");

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("session task should exit after stop")
            .expect("session task panicked");
        assert!(!connected.load(Ordering::SeqCst));
        assert_eq!(opener_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            run_io_entries.load(Ordering::SeqCst),
            2,
            "session task must resume run_io with the fresh socket after reconnect"
        );
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            OpenAiRealtimeEvent::Disconnected => {}
            other => panic!("expected final Disconnected after clean stop, got {other:?}"),
        }

        let text_frames = tokio::time::timeout(Duration::from_secs(1), text_frames_rx.recv())
            .await
            .expect("reconnected server reports frames")
            .expect("reconnected server sender dropped");
        assert!(
            text_frames
                .iter()
                .any(|f| f.contains(r#""type":"session.update""#)),
            "fake reconnect opener must send session.update before readiness"
        );
        assert!(
            text_frames
                .iter()
                .any(|f| f.contains(r#""type":"input_audio_buffer.commit""#)),
            "stop should commit the trailing utterance on the reconnected socket"
        );

        tokio::time::timeout(Duration::from_secs(1), initial_server)
            .await
            .expect("initial server task should finish")
            .expect("initial server task panicked");
    }

    /// `end_user_turn` on a live socket commits the buffer and asks the model to
    /// respond WITHOUT closing the socket — exercised through `run_io` with an
    /// in-process server that records the frames it sees.
    #[tokio::test(flavor = "current_thread")]
    async fn end_user_turn_commits_and_requests_response_without_closing() {
        let (frames_tx, mut frames_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        let (url, server) = ws_fixture::spawn_server(move |mut websocket| async move {
            while let Some(frame) = websocket.next().await {
                match frame {
                    Ok(Message::Text(t)) => {
                        let _ = frames_tx.send(t.to_string());
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    _ => {}
                }
            }
        })
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (mut writer, mut reader) = client_socket.split();
        let (audio_tx, mut audio_rx) = tokio_mpsc::channel(8);
        let (event_tx, _event_rx) = crossbeam_channel::bounded(16);
        let connected = Arc::new(AtomicBool::new(false));
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let disconnected_emitted = Arc::new(AtomicBool::new(false));
        let ready_event = OpenAiRealtimeEvent::Connected;

        audio_tx
            .send(AudioCmd::EndTurn)
            .await
            .expect("queue endturn");
        audio_tx.send(AudioCmd::Stop).await.expect("queue stop");

        let disconnect = tokio::time::timeout(
            Duration::from_secs(2),
            run_io(RunIoCtx {
                writer: &mut writer,
                reader: &mut reader,
                audio_rx: &mut audio_rx,
                event_tx: &event_tx,
                connected: &connected,
                user_disconnected: &user_disconnected,
                ready_event: &ready_event,
                disconnected_emitted: &disconnected_emitted,
            }),
        )
        .await
        .expect("run_io should exit after stop");
        assert!(matches!(disconnect, DisconnectKind::UserRequested));

        let mut seen = Vec::new();
        while let Ok(Some(f)) =
            tokio::time::timeout(Duration::from_millis(200), frames_rx.recv()).await
        {
            seen.push(f);
        }
        // EndTurn → commit + response.create; Stop → a second commit then close.
        assert!(
            seen.iter()
                .any(|f| f.contains(r#""type":"input_audio_buffer.commit""#)),
            "end_user_turn must commit the buffer; frames={seen:?}"
        );
        assert!(
            seen.iter()
                .any(|f| f.contains(r#""type":"response.create""#)),
            "end_user_turn must request a response; frames={seen:?}"
        );

        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;
    }
}
