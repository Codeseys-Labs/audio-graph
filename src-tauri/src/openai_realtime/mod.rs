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
    /// Token / session credential expired (distinct remediation from `Auth`).
    AuthExpired,
    /// Quota / rate-limit exceeded.
    RateLimit {
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_after_secs: Option<u64>,
    },
    /// Server-side failure (5xx / WS close 1011).
    Server,
    /// Transport-layer failure (TLS, TCP, DNS, socket reset).
    Network,
    /// Anything we could not positively classify.
    Unknown,
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
    #[serde(rename = "error")]
    Error {
        category: OpenAiRealtimeErrorCategory,
        message: String,
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
    pub fn connect(&mut self) -> Result<(), String> {
        if self.config.api_key.trim().is_empty() {
            return Err("OpenAI API key is not configured".to_string());
        }

        // Build a dedicated multi-threaded (1 worker) tokio runtime for the WS.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("openai-realtime-s2s-rt")
            .build()
            .map_err(|e| format!("Failed to create tokio runtime: {e}"))?;

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
            let (writer, reader) = open_ws(&config).await?;

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

            Ok::<_, String>((atx, session_handle))
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
/// precise. The inner `String` is consumed through `Debug` formatting, which
/// the dead-code lint doesn't track, hence the allow.
#[derive(Debug)]
#[allow(dead_code)]
enum DisconnectKind {
    ServerClose(String),
    NetworkError(String),
    ProtocolError(String),
    PolicyBlocked(String),
    UserRequested,
    WriterEnded,
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
async fn open_ws(config: &OpenAiRealtimeConfig) -> Result<(WsWriter, WsReader), String> {
    let url = realtime_url(&config.model);
    open_ws_url(config, &url).await
}

async fn open_ws_url(
    config: &OpenAiRealtimeConfig,
    url_str: &str,
) -> Result<(WsWriter, WsReader), String> {
    // `IntoClientRequest` fills in the mandatory WebSocket upgrade headers; we
    // only layer `Authorization` on top. NO `OpenAI-Beta` header (GA only).
    let mut request = url_str
        .into_client_request()
        .map_err(|e| format!("Failed to build WebSocket request: {e}"))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", config.api_key)
            .parse()
            .map_err(|e| format!("Invalid Authorization header: {e}"))?,
    );

    let (ws_stream, _response) = connect_async(request).await.map_err(|e| {
        crate::error::redacted_provider_diagnostic(
            &format!("WebSocket connect failed: {e}"),
            [&config.api_key],
        )
    })?;

    let (mut writer, reader) = ws_stream.split();

    // Configure the voice session immediately after connect, behind the egress
    // guard so a blocked privacy policy cannot leak the session config.
    config
        .content_egress_policy
        .check_json(PROVIDER)
        .map_err(|e| crate::error::redacted_provider_diagnostic(&e, [&config.api_key]))?;
    let update = session_update_payload(config).to_string();
    writer
        .send(Message::Text(update.into()))
        .await
        .map_err(|e| {
            crate::error::redacted_provider_diagnostic(
                &format!("Failed to send session.update: {e}"),
                [&config.api_key],
            )
        })?;

    Ok((writer, reader))
}

#[cfg(test)]
type ReconnectOpenFuture =
    Pin<Box<dyn Future<Output = Result<(WsWriter, WsReader), String>> + Send>>;

#[cfg(test)]
type ReconnectOpener = Arc<dyn Fn(OpenAiRealtimeConfig) -> ReconnectOpenFuture + Send + Sync>;

#[cfg(test)]
async fn open_reconnect_ws(
    config: &OpenAiRealtimeConfig,
    opener: Option<&ReconnectOpener>,
) -> Result<(WsWriter, WsReader), String> {
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
            DisconnectKind::PolicyBlocked(message) => {
                log::warn!("OpenAI Realtime S2S session: content egress blocked: {message}");
                let _ = event_tx.send(OpenAiRealtimeEvent::Error {
                    category: OpenAiRealtimeErrorCategory::Unknown,
                    message,
                });
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
                        log::error!(
                            "OpenAI Realtime S2S session: reconnect budget exhausted after {} attempts",
                            reconnect_attempts - 1
                        );
                        let _ = event_tx.send(OpenAiRealtimeEvent::Error {
                            category: OpenAiRealtimeErrorCategory::Network,
                            message: "OpenAI Realtime S2S reconnect attempts exhausted".into(),
                        });
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
                            log::warn!(
                                "OpenAI Realtime S2S session: reconnect attempt {reconnect_attempts} failed: {e}"
                            );
                            let _ = event_tx.send(OpenAiRealtimeEvent::Error {
                                category: OpenAiRealtimeErrorCategory::Network,
                                message: format!(
                                    "Reconnect attempt {reconnect_attempts} failed: {e}"
                                ),
                            });
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
                            log::error!("OpenAI Realtime S2S: failed to send audio: {e}");
                            return DisconnectKind::NetworkError(format!("send failed: {e}"));
                        }
                    }
                    Some(AudioCmd::EndTurn) => {
                        // Commit the buffered audio + ask the model to respond,
                        // WITHOUT closing the socket (the session stays open for
                        // the assistant reply + the next turn).
                        let commit = json!({ "type": "input_audio_buffer.commit" });
                        if let Err(e) = writer.send(Message::Text(commit.to_string().into())).await {
                            return DisconnectKind::NetworkError(format!("commit failed: {e}"));
                        }
                        let create = json!({ "type": "response.create" });
                        if let Err(e) = writer.send(Message::Text(create.to_string().into())).await {
                            return DisconnectKind::NetworkError(format!("response.create failed: {e}"));
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
                    return DisconnectKind::NetworkError("reader stream ended".into());
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
                    Ok(Message::Close(frame)) => {
                        let diagnostic = close_frame_diagnostic(frame.as_ref());
                        log::info!("OpenAI Realtime S2S: server closed connection {diagnostic}");
                        if user_disconnected.load(Ordering::SeqCst) {
                            return DisconnectKind::UserRequested;
                        }
                        return DisconnectKind::ServerClose(diagnostic);
                    }
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {
                        // Protocol-level frames; nothing to do.
                    }
                    Ok(Message::Binary(_)) => {
                        log::debug!("OpenAI Realtime S2S: unexpected binary message");
                    }
                    Err(tungstenite::Error::ConnectionClosed)
                    | Err(tungstenite::Error::AlreadyClosed) => {
                        return DisconnectKind::NetworkError("connection closed".into());
                    }
                    Err(tungstenite::Error::Protocol(e)) => {
                        return DisconnectKind::ProtocolError(e.to_string());
                    }
                    Err(e) => {
                        log::error!("OpenAI Realtime S2S: WebSocket read error: {e}");
                        return DisconnectKind::NetworkError(format!("{e}"));
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
fn handle_server_message(text: &str, tx: &crossbeam_channel::Sender<OpenAiRealtimeEvent>) -> bool {
    let parsed: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("OpenAI Realtime S2S: invalid JSON: {e}");
            let _ = tx.send(OpenAiRealtimeEvent::Error {
                category: OpenAiRealtimeErrorCategory::Unknown,
                message: format!("Invalid server JSON: {e}"),
            });
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
            let (category, message) = classify_error_frame(parsed.get("error"));
            let _ = tx.send(OpenAiRealtimeEvent::Error { category, message });
            false
        }
        "session.updated" | "session.created" => {
            log::debug!("OpenAI Realtime S2S: {msg_type} (session configured)");
            true
        }
        other => {
            // Many informational events (response.created, output_item.added,
            // rate_limits.updated, etc.) are expected and not actionable here.
            log::debug!("OpenAI Realtime S2S: unhandled message type '{other}'");
            false
        }
    }
}

/// Classify a top-level `error` frame's `error` object into a category +
/// redacted diagnostic message. Never echoes the provider message verbatim
/// (which may contain user content / secrets); reports `type`/`code`/length.
fn classify_error_frame(error: Option<&Value>) -> (OpenAiRealtimeErrorCategory, String) {
    let error_type = error.and_then(|e| e.get("type")).and_then(Value::as_str);
    let code = error.and_then(|e| e.get("code")).and_then(Value::as_str);
    let message_len = error
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .map(|m| m.chars().count())
        .map(|len| len.to_string())
        .unwrap_or_else(|| "none".to_string());

    let category = match (error_type, code) {
        (_, Some(c)) if c.contains("rate_limit") => OpenAiRealtimeErrorCategory::RateLimit {
            retry_after_secs: None,
        },
        (_, Some(c)) if c.contains("expired") => OpenAiRealtimeErrorCategory::AuthExpired,
        (Some(t), _) if t.contains("authentication") || t.contains("invalid_request_error") => {
            // invalid_request_error covers bad keys / params; treat unqualified
            // ones as Auth-adjacent only when the code also signals it.
            if code
                .map(|c| c.contains("auth") || c.contains("api_key"))
                .unwrap_or(false)
            {
                OpenAiRealtimeErrorCategory::Auth
            } else {
                OpenAiRealtimeErrorCategory::Unknown
            }
        }
        (Some(t), _) if t.contains("server_error") => OpenAiRealtimeErrorCategory::Server,
        _ => OpenAiRealtimeErrorCategory::Unknown,
    };

    let message = format!(
        "OpenAI Realtime S2S provider_error type={} code={} message_len={message_len}",
        safe_diagnostic_token(error_type),
        safe_diagnostic_token(code),
    );
    (category, message)
}

fn close_frame_diagnostic(
    frame: Option<&tokio_tungstenite::tungstenite::protocol::CloseFrame>,
) -> String {
    let Some(frame) = frame else {
        return "code=none reason_len=0".to_string();
    };
    let code: u16 = frame.code.into();
    format!("code={code} reason_len={}", frame.reason.chars().count())
}

fn safe_diagnostic_token(value: Option<&str>) -> String {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return "none".to_string();
    };
    if value.len() <= 128
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':'))
    {
        value.to_string()
    } else {
        format!("present_len_{}", value.chars().count())
    }
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
        assert!(result.unwrap_err().contains("API key"));
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
                message: "oops".into(),
            },
            OpenAiRealtimeEvent::Error {
                category: OpenAiRealtimeErrorCategory::RateLimit {
                    retry_after_secs: Some(30),
                },
                message: "429".into(),
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
        let rl = serde_json::to_value(OpenAiRealtimeErrorCategory::RateLimit {
            retry_after_secs: None,
        })
        .unwrap();
        assert_eq!(rl["kind"], "rate_limit");
        assert!(rl.get("retry_after_secs").is_none());
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
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message(
            r#"{"type":"error","error":{"type":"invalid_request_error","code":"rate_limit_exceeded","message":"Rate limit reached for key openai-leak-sentinel"}}"#,
            &tx,
        );
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::Error { category, message } => {
                assert_eq!(
                    category,
                    OpenAiRealtimeErrorCategory::RateLimit {
                        retry_after_secs: None
                    }
                );
                assert!(!message.contains("Rate limit reached"));
                assert!(!message.contains("openai-leak-sentinel"));
                assert!(message.contains("OpenAI Realtime S2S provider_error"));
                assert!(message.contains("type=invalid_request_error"));
                assert!(message.contains("code=rate_limit_exceeded"));
                assert!(message.contains("message_len="));
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[test]
    fn invalid_json_emits_error() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message("not json", &tx);
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::Error { message, .. } => {
                assert!(message.contains("Invalid server JSON"))
            }
            other => panic!("expected Error, got {other:?}"),
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
            OpenAiRealtimeErrorCategory::RateLimit {
                retry_after_secs: None
            }
        );

        let (cat, _) = classify_error_frame(Some(&json!({
            "type": "invalid_request_error", "code": "session_expired"
        })));
        assert_eq!(cat, OpenAiRealtimeErrorCategory::AuthExpired);

        let (cat, _) = classify_error_frame(Some(&json!({
            "type": "invalid_request_error", "code": "invalid_api_key"
        })));
        assert_eq!(cat, OpenAiRealtimeErrorCategory::Auth);

        let (cat, _) = classify_error_frame(Some(&json!({ "type": "server_error" })));
        assert_eq!(cat, OpenAiRealtimeErrorCategory::Server);

        let (cat, _) = classify_error_frame(None);
        assert_eq!(cat, OpenAiRealtimeErrorCategory::Unknown);
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
        let (frame_tx, frame_rx) = tokio::sync::oneshot::channel();
        let (url, server) = ws_fixture::spawn_server(move |websocket| async move {
            let _ = frame_tx.send(first_client_content_frame(websocket).await);
        })
        .await;

        let config = with_blocked_content_egress(test_config());
        let error = open_ws_url(&config, &url)
            .await
            .expect_err("blocked policy should reject S2S session.update write");

        assert!(error.contains("Privacy policy blocked"));
        assert!(error.contains(PROVIDER));
        assert!(error.contains("local_only"));
        assert!(!error.contains("openai-private-s2s-key-sentinel"));

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
                    Err("fake reconnect failure".to_string())
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
            OpenAiRealtimeEvent::Error { message, .. } => {
                assert!(message.contains("Reconnect attempt 1 failed"))
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
                        .map_err(|e| {
                            crate::error::redacted_provider_diagnostic(&e, [&config.api_key])
                        })?;
                    let update = session_update_payload(&config).to_string();
                    writer
                        .send(Message::Text(update.into()))
                        .await
                        .map_err(|e| {
                            crate::error::redacted_provider_diagnostic(
                                &format!("fake session.update failed: {e}"),
                                [&config.api_key],
                            )
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
