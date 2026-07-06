//! AssemblyAI real-time streaming ASR client.
//!
//! Connects to the AssemblyAI real-time transcription WebSocket endpoint
//! and streams audio for live speech-to-text.
//!
//! # Protocol overview
//!
//! 1. Open WSS connection to `wss://streaming.assemblyai.com/v3/ws` with
//!    `Authorization: {api_key}` on the WebSocket upgrade request and v3 query
//!    configuration (`speech_model`, `sample_rate`, `encoding`, etc.).
//! 2. Stream audio as binary PCM s16le frames.
//! 3. Receive v3 JSON events (`Begin`, `SpeechStarted`, `Turn`,
//!    `SpeakerRevision`, `Termination`, `Error`).
//! 4. Close session by sending `{ "type": "Terminate" }`, then the close frame.
//!
//! # Threading model
//!
//! Same as the Gemini client: the public API is **synchronous** (called from
//! `std::thread` workers). Internally a dedicated tokio runtime drives the
//! WebSocket, with audio forwarded via `tokio::sync::mpsc` and events
//! delivered back through `crossbeam_channel`.

#[cfg(test)]
use super::reconnect::backoff_for_attempt;
use super::reconnect::{ReconnectStep, next_reconnect_step};
use super::transport::{AsrTransportPayloadKind, AsrWsWriteGuard};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
#[cfg(test)]
use std::{future::Future, pin::Pin};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::{self, Message};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

const ASSEMBLYAI_PROVIDER: &str = "assemblyai";
/// Streaming `speech_model` sent on the v3 upgrade query.
///
/// PINNED, not configurable (review m5) — this is deliberate, not an oversight.
/// Unlike Deepgram (`model` threaded from settings across many tiers) and Soniox
/// (`config.model`), AssemblyAI's v3 Universal-Streaming endpoint exposes a
/// SINGLE realtime tier, so there is no alternate value for a user to pick; a
/// `model` field on `AssemblyAIConfig` + a settings/UI control would be a no-op
/// picker. If AssemblyAI later ships a second streaming tier, thread a `model`
/// through exactly like Soniox: add `model` to `AsrProvider::AssemblyAI`
/// (settings/mod.rs) with a `default_assemblyai_model` serde default, carry it
/// into `AssemblyAIConfig` at speech/mod.rs, use it here instead of this const,
/// and add en+pt picker strings.
pub const DEFAULT_MODEL: &str = "universal-3-5-pro";
const ASSEMBLYAI_V3_WS_ENDPOINT: &str = "wss://streaming.assemblyai.com/v3/ws";
/// Idle keepalive cadence (M2 / audio-graph-63be). AssemblyAI's v3 streaming
/// protocol documents no application-level idle no-op frame, so we send a
/// WebSocket `Ping` control frame during quiet periods. The mixer normally
/// feeds a continuous silence-padded stream (audio keeps `last_outbound`
/// warm), so this only fires when the audio cadence actually stalls. 8s is a
/// conservative margin under typical ~30-60s server idle windows.
const KEEPALIVE_INTERVAL_SECS: u64 = 8;

/// Events emitted by the AssemblyAI streaming client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AssemblyAIEvent {
    /// V3 server JSON message for source-aware parsing. Serialization and Debug
    /// expose only bounded metadata; the raw frame is retained only inside the
    /// process so the speech receiver can parse it with the current source id.
    #[serde(rename = "server_message")]
    ServerMessage {
        frame: AssemblyAiServerMessageFrame,
        received_at_ms: u64,
    },
    /// The session has been terminated. Emitted by the session task on
    /// user-initiated teardown, reconnect-budget exhaustion, or a policy
    /// block — see `session_task`. (A v3 `Termination` server message is
    /// surfaced as a `ServerMessage` and handled by the event consumer, which
    /// stops its loop directly rather than round-tripping this variant.)
    #[serde(rename = "session_terminated")]
    SessionTerminated,
    /// A non-fatal error occurred.
    #[serde(rename = "error")]
    Error { message: String },
    /// The client detected a disconnect and is attempting to reconnect.
    #[serde(rename = "reconnecting")]
    Reconnecting { attempt: u32, backoff_secs: u64 },
    /// The client successfully re-established the WebSocket after a disconnect.
    #[serde(rename = "reconnected")]
    Reconnected,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AssemblyAiServerMessageFrame {
    #[serde(skip_serializing, skip_deserializing, default)]
    raw_text: String,
    pub message_type: String,
    pub request_id: Option<String>,
    pub field_count: usize,
}

impl std::fmt::Debug for AssemblyAiServerMessageFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssemblyAiServerMessageFrame")
            .field("message_type", &self.message_type)
            .field("request_id", &self.request_id)
            .field("field_count", &self.field_count)
            .field("raw_text", &"<redacted>")
            .finish()
    }
}

impl AssemblyAiServerMessageFrame {
    fn new(raw_text: &str, parsed: &Value) -> Self {
        Self {
            raw_text: raw_text.to_string(),
            message_type: json_string_field(parsed, &["type"])
                .unwrap_or_else(|| "unknown".to_string()),
            request_id: json_string_field(parsed, &["request_id", "requestId", "id"]),
            field_count: json_field_count(parsed),
        }
    }

    pub fn as_str(&self) -> &str {
        &self.raw_text
    }
}

/// Configuration for an AssemblyAI streaming session.
#[derive(Clone)]
pub struct AssemblyAIConfig {
    /// AssemblyAI API key.
    pub api_key: String,
    /// Whether to enable speaker diarization.
    pub enable_diarization: bool,
    /// Runtime privacy guard for session audio egress.
    pub content_egress_policy: crate::asr::ProviderContentEgressPolicy,
}

#[derive(Debug, Clone)]
pub struct AssemblyAiV3ParsedMessage {
    pub session_id: Option<String>,
    pub revisions: Vec<AssemblyAiV3ParsedRevision>,
    pub speaker_revisions: Vec<AssemblyAiV3SpeakerRevision>,
    pub terminated: bool,
    pub error: Option<AssemblyAiV3ProviderError>,
}

#[derive(Debug, Clone)]
pub struct AssemblyAiV3ParsedRevision {
    pub payload: crate::events::AsrSpanRevisionPayload,
    pub turn_is_formatted: bool,
    pub end_of_turn_confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssemblyAiV3SpeakerRevision {
    pub turn_order: u64,
    pub span_id: String,
    pub provider_item_id: String,
    pub speaker_id: Option<String>,
    pub speaker_label: Option<String>,
    pub words: Vec<AssemblyAiV3SpeakerRevisionWord>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssemblyAiV3SpeakerRevisionWord {
    pub text: String,
    pub speaker_id: Option<String>,
    pub start_time: Option<f64>,
    pub end_time: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyAiV3ProviderError {
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssemblyAiV3ParseError {
    InvalidJson(String),
    UnsupportedMessageType(String),
}

#[derive(Debug)]
pub struct AssemblyAiV3Parser {
    source_id: String,
    response_sequence: u64,
    revision_numbers_by_turn: HashMap<u64, u64>,
}

#[derive(Debug, Deserialize)]
struct AssemblyAiV3Response {
    #[serde(rename = "type")]
    message_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    turn_order: Option<u64>,
    #[serde(default)]
    end_of_turn: Option<bool>,
    #[serde(default)]
    turn_is_formatted: Option<bool>,
    #[serde(default)]
    end_of_turn_confidence: Option<f32>,
    #[serde(default)]
    transcript: Option<String>,
    #[serde(default)]
    words: Vec<AssemblyAiV3Word>,
    #[serde(default)]
    speaker_label: Option<String>,
    #[serde(default)]
    revisions: Vec<AssemblyAiV3SpeakerRevisionResponse>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct AssemblyAiV3Word {
    text: String,
    #[serde(default)]
    start: Option<u64>,
    #[serde(default)]
    end: Option<u64>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    speaker: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AssemblyAiV3SpeakerRevisionResponse {
    turn_order: u64,
    #[serde(default)]
    speaker_label: Option<String>,
    #[serde(default)]
    words: Vec<AssemblyAiV3Word>,
}

impl AssemblyAiV3Parser {
    pub fn new(source_id: impl Into<String>) -> Self {
        Self {
            source_id: source_id.into(),
            response_sequence: 0,
            revision_numbers_by_turn: HashMap::new(),
        }
    }

    pub fn set_source_id_if_no_turns(&mut self, source_id: impl Into<String>) {
        if self.revision_numbers_by_turn.is_empty() {
            self.source_id = source_id.into();
        }
    }

    pub fn parse_message(
        &mut self,
        text: &str,
        received_at_ms: u64,
    ) -> Result<AssemblyAiV3ParsedMessage, AssemblyAiV3ParseError> {
        let response: AssemblyAiV3Response = serde_json::from_str(text)
            .map_err(|error| AssemblyAiV3ParseError::InvalidJson(error.to_string()))?;
        self.response_sequence += 1;

        match response.message_type.as_str() {
            "Begin" => Ok(AssemblyAiV3ParsedMessage {
                session_id: response.id,
                revisions: Vec::new(),
                speaker_revisions: Vec::new(),
                terminated: false,
                error: None,
            }),
            "SpeechStarted" => Ok(AssemblyAiV3ParsedMessage {
                session_id: None,
                revisions: Vec::new(),
                speaker_revisions: Vec::new(),
                terminated: false,
                error: None,
            }),
            "Turn" => Ok(AssemblyAiV3ParsedMessage {
                session_id: None,
                revisions: self
                    .emit_turn_revision(&response, received_at_ms)
                    .into_iter()
                    .collect(),
                speaker_revisions: Vec::new(),
                terminated: false,
                error: None,
            }),
            "SpeakerRevision" => Ok(AssemblyAiV3ParsedMessage {
                session_id: None,
                revisions: Vec::new(),
                speaker_revisions: self.parse_speaker_revisions(response.revisions),
                terminated: false,
                error: None,
            }),
            "Termination" => Ok(AssemblyAiV3ParsedMessage {
                session_id: None,
                revisions: Vec::new(),
                speaker_revisions: Vec::new(),
                terminated: true,
                error: None,
            }),
            "Error" => Ok(AssemblyAiV3ParsedMessage {
                session_id: None,
                revisions: Vec::new(),
                speaker_revisions: Vec::new(),
                terminated: false,
                error: Some(AssemblyAiV3ProviderError {
                    message: format!(
                        "AssemblyAI v3 streaming error message_len={}",
                        response
                            .error
                            .or(response.message)
                            .map(|message| message.chars().count())
                            .unwrap_or(0)
                    ),
                }),
            }),
            other => Err(AssemblyAiV3ParseError::UnsupportedMessageType(
                other.to_string(),
            )),
        }
    }

    fn emit_turn_revision(
        &mut self,
        response: &AssemblyAiV3Response,
        received_at_ms: u64,
    ) -> Option<AssemblyAiV3ParsedRevision> {
        let text = response.transcript.as_deref().unwrap_or("").trim();
        if text.is_empty() {
            return None;
        }

        let turn_order = response.turn_order.unwrap_or(self.response_sequence - 1);
        let provider_item_id = assemblyai_v3_provider_item_id(turn_order);
        let span_id = assemblyai_v3_span_id(&self.source_id, turn_order);
        let revision_number = self
            .revision_numbers_by_turn
            .entry(turn_order)
            .and_modify(|revision| *revision += 1)
            .or_insert(1);
        let supersedes =
            (*revision_number > 1).then(|| revision_ref(&span_id, *revision_number - 1));
        let is_final = response.end_of_turn.unwrap_or(false);
        let speaker_id = response.speaker_label.clone();

        Some(AssemblyAiV3ParsedRevision {
            payload: crate::events::AsrSpanRevisionPayload {
                span_id,
                provider: ASSEMBLYAI_PROVIDER.to_string(),
                source_id: self.source_id.clone(),
                provider_item_id: Some(provider_item_id.clone()),
                transcript_segment_id: is_final.then(|| format!("{provider_item_id}@final")),
                speaker_id: speaker_id.clone(),
                speaker_label: speaker_id
                    .as_ref()
                    .map(|speaker| format!("Speaker {speaker}")),
                channel: None,
                text: text.to_string(),
                start_time: min_start_ms(&response.words).map_or(0.0, millis_to_secs),
                end_time: max_end_ms(&response.words).map_or(0.0, millis_to_secs),
                confidence: average_confidence(&response.words)
                    .or(response.end_of_turn_confidence)
                    .unwrap_or(0.0),
                is_final,
                stability: if is_final {
                    crate::events::AsrSpanStability::Final
                } else {
                    crate::events::AsrSpanStability::Partial
                },
                revision_number: *revision_number,
                supersedes,
                turn_id: Some(provider_item_id),
                end_of_turn: is_final,
                raw_event_ref: Some(format!("assemblyai.v3.turn.{}", self.response_sequence)),
                capture_latency_ms: None,
                asr_latency_ms: None,
                received_at_ms,
            },
            turn_is_formatted: response.turn_is_formatted.unwrap_or(false),
            end_of_turn_confidence: response.end_of_turn_confidence,
        })
    }

    fn parse_speaker_revisions(
        &self,
        revisions: Vec<AssemblyAiV3SpeakerRevisionResponse>,
    ) -> Vec<AssemblyAiV3SpeakerRevision> {
        revisions
            .into_iter()
            .map(|revision| {
                let provider_item_id = assemblyai_v3_provider_item_id(revision.turn_order);
                let speaker_id = revision.speaker_label.clone();
                AssemblyAiV3SpeakerRevision {
                    turn_order: revision.turn_order,
                    span_id: assemblyai_v3_span_id(&self.source_id, revision.turn_order),
                    provider_item_id,
                    speaker_id: speaker_id.clone(),
                    speaker_label: speaker_id
                        .as_ref()
                        .map(|speaker| format!("Speaker {speaker}")),
                    words: revision
                        .words
                        .into_iter()
                        .map(|word| AssemblyAiV3SpeakerRevisionWord {
                            text: word.text,
                            speaker_id: word.speaker,
                            start_time: word.start.map(millis_to_secs),
                            end_time: word.end.map(millis_to_secs),
                        })
                        .collect(),
                }
            })
            .collect()
    }
}

fn assemblyai_v3_provider_item_id(turn_order: u64) -> String {
    format!("turn-{turn_order}")
}

fn assemblyai_v3_span_id(source_id: &str, turn_order: u64) -> String {
    format!("{ASSEMBLYAI_PROVIDER}:{source_id}:turn-{turn_order}")
}

fn revision_ref(span_id: &str, revision_number: u64) -> String {
    format!("{span_id}@rev{revision_number}")
}

fn min_start_ms(words: &[AssemblyAiV3Word]) -> Option<u64> {
    words.iter().filter_map(|word| word.start).min()
}

fn max_end_ms(words: &[AssemblyAiV3Word]) -> Option<u64> {
    words.iter().filter_map(|word| word.end).max()
}

fn average_confidence(words: &[AssemblyAiV3Word]) -> Option<f32> {
    let mut total = 0.0;
    let mut count = 0usize;
    for confidence in words.iter().filter_map(|word| word.confidence) {
        total += confidence;
        count += 1;
    }
    (count > 0).then(|| total / count as f32)
}

fn millis_to_secs(ms: u64) -> f64 {
    ms as f64 / 1000.0
}

impl std::fmt::Debug for AssemblyAIConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssemblyAIConfig")
            .field(
                "api_key",
                &crate::credentials::redacted_secret_presence(Some(&self.api_key)),
            )
            .field("enable_diarization", &self.enable_diarization)
            .field("content_egress_policy", &self.content_egress_policy)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Internal message passed from sync send_audio() -> async writer task
// ---------------------------------------------------------------------------

/// Hard cap on the audio-chunk backlog during a prolonged reconnect (see
/// `pending_chunks` on `AssemblyAIClient`). ~10s worth of 50ms chunks. Overflow
/// is **fail-fast** (flip `user_disconnected`, end the session) — the shared ASR
/// overflow policy documented on `asr::deepgram::AUDIO_BUFFER_MAX_CHUNKS`
/// (deliberately the opposite of Gemini's lossy-drop; review m2).
const AUDIO_BUFFER_MAX_CHUNKS: usize = 200;

enum AudioCmd {
    /// PCM s16le bytes ready to send as a binary frame.
    Chunk(Vec<u8>),
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

/// An AssemblyAI real-time streaming ASR client.
///
/// The public methods (`connect`, `send_audio`, `disconnect`, `event_rx`) are
/// all **synchronous** — they block the caller's thread just long enough to
/// hand off work to the internal async runtime. This matches the threading
/// model used by `commands.rs` where worker threads run in `std::thread`.
pub struct AssemblyAIClient {
    config: AssemblyAIConfig,
    /// crossbeam event channel — writer side (background reader task pushes here).
    event_tx: crossbeam_channel::Sender<AssemblyAIEvent>,
    /// crossbeam event channel — reader side (command layer clones this).
    event_rx: crossbeam_channel::Receiver<AssemblyAIEvent>,
    /// Whether the WebSocket is connected.
    connected: Arc<AtomicBool>,
    /// Set to `true` when the user has explicitly called `disconnect()`.
    /// Suppresses auto-reconnect on teardown.
    user_disconnected: Arc<AtomicBool>,
    /// Tokio runtime that owns the WebSocket tasks.
    rt: Option<tokio::runtime::Runtime>,
    /// Sender for audio commands -> async writer task.
    audio_tx: Option<tokio_mpsc::UnboundedSender<AudioCmd>>,
    /// Approximate backlog of unsent audio chunks. Bounded by
    /// `AUDIO_BUFFER_MAX_CHUNKS` — see the Deepgram client for the full
    /// reconnect-memory rationale.
    pending_chunks: Arc<std::sync::atomic::AtomicUsize>,
    /// Handle to the session task (owns both halves and reconnect logic).
    #[allow(dead_code)]
    session_handle: Option<tokio::task::JoinHandle<()>>,
}

impl AssemblyAIClient {
    /// Create a new (disconnected) AssemblyAI streaming client.
    pub fn new(config: AssemblyAIConfig) -> Self {
        let (event_tx, event_rx) = crossbeam_channel::bounded(128);
        Self {
            config,
            event_tx,
            event_rx,
            connected: Arc::new(AtomicBool::new(false)),
            user_disconnected: Arc::new(AtomicBool::new(false)),
            rt: None,
            audio_tx: None,
            pending_chunks: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            session_handle: None,
        }
    }

    // ------------------------------------------------------------------
    // Connect
    // ------------------------------------------------------------------

    /// Connect to the AssemblyAI real-time transcription API.
    ///
    /// Blocks the caller until the WebSocket is open, then spawns a background
    /// session task on an internal tokio runtime. The session task handles
    /// audio writing, server message reading, and automatic reconnection with
    /// exponential backoff if the WebSocket drops mid-session.
    pub fn connect(&mut self) -> Result<(), String> {
        if self.config.api_key.is_empty() {
            return Err("AssemblyAI API key is not configured".to_string());
        }

        // Build a dedicated single-threaded tokio runtime for the WebSocket.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("assemblyai-ws-rt")
            .build()
            .map_err(|e| format!("Failed to create tokio runtime: {e}"))?;

        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let connected = Arc::clone(&self.connected);
        let user_disconnected = Arc::clone(&self.user_disconnected);
        // Reset on (re)connect so a prior teardown flag does not poison a
        // fresh session.
        user_disconnected.store(false, Ordering::SeqCst);
        self.pending_chunks
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let pending_chunks = Arc::clone(&self.pending_chunks);

        // Perform the blocking initial connect inside the runtime.
        let (audio_tx, session_handle) = rt.block_on(async move {
            let (writer, reader) = open_ws(&config).await?;

            log::info!("AssemblyAI: WebSocket connected");
            connected.store(true, Ordering::SeqCst);

            let (atx, arx) = tokio_mpsc::unbounded_channel::<AudioCmd>();

            let session_handle = tokio::spawn(session_task(AssemblyAISessionCtx {
                writer,
                reader,
                audio_rx: arx,
                config,
                event_tx,
                connected,
                user_disconnected,
                pending_chunks,
                #[cfg(test)]
                reconnect_opener: None,
                #[cfg(test)]
                run_io_entries: None,
            }));

            Ok::<_, String>((atx, session_handle))
        })?;

        self.audio_tx = Some(audio_tx);
        self.session_handle = Some(session_handle);
        self.rt = Some(rt);

        Ok(())
    }

    // ------------------------------------------------------------------
    // Send audio
    // ------------------------------------------------------------------

    /// Send PCM audio data to AssemblyAI for transcription.
    ///
    /// The audio should be **f32 mono 16 kHz** (matching the pipeline output).
    /// The method converts to 16-bit LE PCM and queues a binary frame for async
    /// sending. Returns immediately (non-blocking).
    ///
    /// # Behaviour during auto-reconnect
    ///
    /// Only `user_disconnected` is checked — not the transient `connected`
    /// flag — so the caller can keep streaming audio during a reconnect
    /// cycle. Queued chunks flush as soon as the new socket is open.
    pub fn send_audio(&self, audio: &[f32]) -> Result<(), String> {
        if self.user_disconnected.load(Ordering::SeqCst) {
            return Err("AssemblyAI client has been disconnected".to_string());
        }

        if audio.is_empty() {
            return Ok(());
        }

        self.config
            .content_egress_policy
            .check_audio("asr.assemblyai")?;

        let tx = self
            .audio_tx
            .as_ref()
            .ok_or_else(|| "Audio channel not initialized".to_string())?;

        // Bail when the backlog is past the safety cap — mirrors the Deepgram
        // client; see its comment for rationale.
        let depth = self
            .pending_chunks
            .load(std::sync::atomic::Ordering::Relaxed);
        if depth >= AUDIO_BUFFER_MAX_CHUNKS {
            self.user_disconnected
                .store(true, std::sync::atomic::Ordering::SeqCst);
            return Err(format!(
                "AssemblyAI audio buffer full ({depth} chunks) — likely a stuck reconnect. Restart the session."
            ));
        }

        let pcm_bytes = f32_to_i16_le_bytes(audio);

        self.pending_chunks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tx.send(AudioCmd::Chunk(pcm_bytes)).map_err(|_| {
            self.pending_chunks
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            "Audio channel closed".to_string()
        })
    }

    // ------------------------------------------------------------------
    // Event receiver
    // ------------------------------------------------------------------

    /// Get a clone of the event receiver channel.
    pub fn event_rx(&self) -> crossbeam_channel::Receiver<AssemblyAIEvent> {
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

    /// Disconnect from the AssemblyAI API and clean up resources.
    ///
    /// Sends `Terminate`, closes the WebSocket, and shuts down
    /// the internal tokio runtime on Drop. Setting `user_disconnected`
    /// prevents the session task from attempting to auto-reconnect.
    pub fn disconnect(&self) {
        log::info!("AssemblyAIClient: disconnecting (user-initiated)");

        // Mark this teardown as user-initiated so the session task does not
        // try to reconnect after the close frame is observed.
        self.user_disconnected.store(true, Ordering::SeqCst);

        // Signal not connected first (stops send_audio calls).
        self.connected.store(false, Ordering::SeqCst);

        // Tell the writer task to send the v3 Terminate message + close.
        if let Some(ref tx) = self.audio_tx {
            let _ = tx.send(AudioCmd::Stop);
        }
    }
}

impl Drop for AssemblyAIClient {
    fn drop(&mut self) {
        // Mark teardown as user-initiated so the session task exits cleanly.
        self.user_disconnected.store(true, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);

        // Signal writer to stop.
        if let Some(ref tx) = self.audio_tx {
            let _ = tx.send(AudioCmd::Stop);
        }
        self.audio_tx = None;

        // Shut down the tokio runtime (this joins background tasks).
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(std::time::Duration::from_secs(3));
        }

        log::info!("AssemblyAIClient: dropped");
    }
}

// ===========================================================================
// Free functions — async building blocks
// ===========================================================================

/// Classifies *why* the session dropped so downstream logs / events can be
/// precise without the caller re-parsing error strings. See the matching
/// comment on Deepgram's `DisconnectKind` — the inner String is consumed
/// through `Debug` formatting, which the dead-code lint doesn't track.
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

/// Open a fresh AssemblyAI WebSocket using the live [`AssemblyAIConfig`].
///
/// Used for the initial connect and for each reconnect attempt. AssemblyAI's
/// v3 endpoint has no separate setup frame — the `Authorization`
/// header and query params on the upgrade request are the full handshake —
/// so a reconnect is just re-running this function.
async fn open_ws(config: &AssemblyAIConfig) -> Result<(WsWriter, WsReader), String> {
    let url = assemblyai_v3_websocket_url(config)?;
    open_ws_url(config, url.as_str()).await
}

/// Connect the AssemblyAI upgrade request against an explicit URL.
///
/// Split out from [`open_ws`] so tests can exercise the *production* request
/// shape against a local `ws_fixture` server (the fast tests connect via
/// `ws_fixture::connect_client`, which auto-injects the upgrade headers and so
/// never covered the hand-built request that omitted them — see
/// audio-graph-7086 / review B1).
async fn open_ws_url(
    config: &AssemblyAIConfig,
    url_str: &str,
) -> Result<(WsWriter, WsReader), String> {
    // Build via the shared helper so the five mandatory WS upgrade headers are
    // injected (the old hand-built `http::Request` set only `Authorization` and
    // never handshook). The key stays in the header, never the URL query string.
    let auth = tungstenite::http::HeaderValue::from_str(&config.api_key).map_err(|e| {
        crate::error::redacted_provider_diagnostic(
            &format!("Invalid AssemblyAI Authorization header: {e}"),
            [&config.api_key],
        )
    })?;
    let request = crate::ws_request::build_ws_upgrade_request(
        url_str,
        [(tungstenite::http::header::AUTHORIZATION, auth)],
    )
    .map_err(|e| crate::error::redacted_provider_diagnostic(&e, [config.api_key.as_str()]))?;

    // Bounded connect so a stalled TLS/HTTP-upgrade handshake surfaces as an
    // ordinary connect error instead of hanging the reconnect ladder forever.
    let (ws_stream, _response) = crate::ws_request::connect_async_bounded(request)
        .await
        .map_err(|e| {
            crate::error::redacted_provider_diagnostic(
                &format!("WebSocket connect failed: {e}"),
                [&config.api_key],
            )
        })?;

    Ok(ws_stream.split())
}

fn assemblyai_v3_websocket_url(config: &AssemblyAIConfig) -> Result<url::Url, String> {
    let mut url = url::Url::parse(ASSEMBLYAI_V3_WS_ENDPOINT)
        .map_err(|e| format!("Invalid AssemblyAI v3 WebSocket URL: {e}"))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("speech_model", DEFAULT_MODEL);
        query.append_pair("sample_rate", "16000");
        query.append_pair("encoding", "pcm_s16le");
        if config.enable_diarization {
            query.append_pair("speaker_labels", "true");
        }
    }
    Ok(url)
}

#[cfg(test)]
type ReconnectOpenFuture =
    Pin<Box<dyn Future<Output = Result<(WsWriter, WsReader), String>> + Send>>;

#[cfg(test)]
type ReconnectOpener = Arc<dyn Fn(AssemblyAIConfig) -> ReconnectOpenFuture + Send + Sync>;

#[cfg(test)]
async fn open_reconnect_ws(
    config: &AssemblyAIConfig,
    opener: Option<&ReconnectOpener>,
) -> Result<(WsWriter, WsReader), String> {
    if let Some(opener) = opener {
        opener(config.clone()).await
    } else {
        open_ws(config).await
    }
}

/// Bundles everything `session_task` owns for a single AssemblyAI session:
/// the split WebSocket halves, the audio command receiver, live config,
/// the outbound event channel, and the three shared atomics. Collapses an
/// 8-arg function signature to one — see `speech/context.rs` for the same
/// pattern applied to the speech workers.
struct AssemblyAISessionCtx {
    writer: WsWriter,
    reader: WsReader,
    audio_rx: tokio_mpsc::UnboundedReceiver<AudioCmd>,
    config: AssemblyAIConfig,
    event_tx: crossbeam_channel::Sender<AssemblyAIEvent>,
    connected: Arc<AtomicBool>,
    user_disconnected: Arc<AtomicBool>,
    pending_chunks: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    reconnect_opener: Option<ReconnectOpener>,
    #[cfg(test)]
    run_io_entries: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

/// Emit a single terminal [`AssemblyAIEvent::SessionTerminated`], guarded by a
/// one-shot atomic so a teardown that reaches this from more than one arm
/// (e.g. a user-cancel racing the session task's own drop path) never
/// double-emits (review n4 — mirrors `deepgram::emit_disconnected_once` /
/// `openai_realtime::emit_disconnected_once`). Returns `true` if this call was
/// the one that emitted. Re-armed (`store(false)`) on a successful reconnect so
/// a later teardown on the fresh session still fires exactly once.
fn emit_session_terminated_once(
    event_tx: &crossbeam_channel::Sender<AssemblyAIEvent>,
    terminated_emitted: &Arc<AtomicBool>,
) -> bool {
    if terminated_emitted.swap(true, Ordering::SeqCst) {
        return false;
    }
    let _ = event_tx.send(AssemblyAIEvent::SessionTerminated);
    true
}

/// Background task owning a single AssemblyAI WebSocket session, including
/// reconnect logic. Mirrors the Deepgram `session_task` structure — see
/// comments there for full design rationale.
async fn session_task(ctx: AssemblyAISessionCtx) {
    let mut writer = ctx.writer;
    let mut reader = ctx.reader;
    let mut audio_rx = ctx.audio_rx;
    let config = ctx.config;
    let event_tx = ctx.event_tx;
    let connected = ctx.connected;
    let user_disconnected = ctx.user_disconnected;
    let pending_chunks = ctx.pending_chunks;
    #[cfg(test)]
    let reconnect_opener = ctx.reconnect_opener;
    #[cfg(test)]
    let run_io_entries = ctx.run_io_entries;
    // One-shot terminal-event guard, fresh per session task (re-armed on each
    // successful reconnect below). See `emit_session_terminated_once`.
    let terminated_emitted = Arc::new(AtomicBool::new(false));
    let mut reconnect_attempts: u32 = 0;
    let write_guard = AsrWsWriteGuard::new("asr.assemblyai", config.content_egress_policy);

    loop {
        #[cfg(test)]
        if let Some(entries) = &run_io_entries {
            entries.fetch_add(1, Ordering::SeqCst);
        }

        let disconnect = run_io(
            &mut writer,
            &mut reader,
            &mut audio_rx,
            &event_tx,
            &user_disconnected,
            &pending_chunks,
            &write_guard,
            &config.api_key,
        )
        .await;

        connected.store(false, Ordering::SeqCst);

        match disconnect {
            DisconnectKind::UserRequested | DisconnectKind::WriterEnded => {
                log::info!("AssemblyAI session: ending ({disconnect:?})");
                emit_session_terminated_once(&event_tx, &terminated_emitted);
                break;
            }
            DisconnectKind::PolicyBlocked(message) => {
                log::warn!("AssemblyAI session: content egress blocked: {message}");
                let _ = event_tx.send(AssemblyAIEvent::Error { message });
                emit_session_terminated_once(&event_tx, &terminated_emitted);
                break;
            }
            _ => {
                if user_disconnected.load(Ordering::SeqCst) {
                    emit_session_terminated_once(&event_tx, &terminated_emitted);
                    break;
                }

                log::warn!("AssemblyAI session: disconnected — {disconnect:?}");

                let reconnected = loop {
                    let (backoff, attempt) = match next_reconnect_step(reconnect_attempts) {
                        ReconnectStep::Retry {
                            attempt,
                            backoff_secs,
                        } => {
                            reconnect_attempts = attempt;
                            (backoff_secs, attempt)
                        }
                        ReconnectStep::GiveUp { attempted } => {
                            log::error!(
                                "AssemblyAI session: reconnect budget exhausted after {attempted} attempts"
                            );
                            let _ = event_tx.send(AssemblyAIEvent::Error {
                                message: "AssemblyAI reconnect attempts exhausted".into(),
                            });
                            emit_session_terminated_once(&event_tx, &terminated_emitted);
                            break false;
                        }
                    };

                    log::info!(
                        "AssemblyAI session: reconnecting (attempt {attempt}, backoff {backoff}s)"
                    );
                    let _ = event_tx.send(AssemblyAIEvent::Reconnecting {
                        attempt,
                        backoff_secs: backoff,
                    });

                    // Sleep for the backoff window but bail out early on user
                    // cancellation so shutdown doesn't wait up to 10s.
                    let sleep = tokio::time::sleep(Duration::from_secs(backoff));
                    tokio::pin!(sleep);
                    loop {
                        tokio::select! {
                            _ = &mut sleep => break,
                            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                                if user_disconnected.load(Ordering::SeqCst) {
                                    log::info!("AssemblyAI session: user cancelled during backoff");
                                    emit_session_terminated_once(&event_tx, &terminated_emitted);
                                    return;
                                }
                            }
                        }
                    }

                    if user_disconnected.load(Ordering::SeqCst) {
                        log::info!("AssemblyAI session: user cancelled before reconnect open");
                        emit_session_terminated_once(&event_tx, &terminated_emitted);
                        return;
                    }

                    #[cfg(test)]
                    let reconnect_result =
                        open_reconnect_ws(&config, reconnect_opener.as_ref()).await;
                    #[cfg(not(test))]
                    let reconnect_result = open_ws(&config).await;

                    match reconnect_result {
                        Ok((new_writer, new_reader)) => {
                            writer = new_writer;
                            reader = new_reader;
                            connected.store(true, Ordering::SeqCst);
                            // Re-arm the terminal-event guard so a teardown on the
                            // fresh session emits exactly once (review n4).
                            terminated_emitted.store(false, Ordering::SeqCst);
                            log::info!("AssemblyAI session: reconnected on attempt {attempt}");
                            let _ = event_tx.send(AssemblyAIEvent::Reconnected);
                            reconnect_attempts = 0;
                            break true;
                        }
                        Err(e) => {
                            // Redact: a reconnect error can embed the upgrade
                            // request (Authorization header) or URL userinfo, so
                            // scrub the api_key before it reaches logs or the UI.
                            let diag = crate::error::redacted_provider_diagnostic(
                                &format!("Reconnect attempt {attempt} failed: {e}"),
                                [&config.api_key],
                            );
                            log::warn!("AssemblyAI session: {diag}");
                            let _ = event_tx.send(AssemblyAIEvent::Error { message: diag });
                            // Stay in the reconnect ladder. Do not loop back
                            // through run_io with the previous closed socket.
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
    log::info!("AssemblyAI: session task exited");
}

/// Pumps audio out and transcripts back for a single WebSocket instance.
///
/// Returns the classified [`DisconnectKind`] when the socket breaks or the
/// caller asks to stop. The session task turns that into either a reconnect
/// or a clean exit.
#[allow(clippy::too_many_arguments)]
async fn run_io(
    writer: &mut WsWriter,
    reader: &mut WsReader,
    audio_rx: &mut tokio_mpsc::UnboundedReceiver<AudioCmd>,
    event_tx: &crossbeam_channel::Sender<AssemblyAIEvent>,
    user_disconnected: &Arc<AtomicBool>,
    pending_chunks: &Arc<std::sync::atomic::AtomicUsize>,
    write_guard: &AsrWsWriteGuard,
    api_key: &str,
) -> DisconnectKind {
    run_io_with_keepalive_interval(
        writer,
        reader,
        audio_rx,
        event_tx,
        user_disconnected,
        pending_chunks,
        write_guard,
        api_key,
        Duration::from_secs(KEEPALIVE_INTERVAL_SECS),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_io_with_keepalive_interval(
    writer: &mut WsWriter,
    reader: &mut WsReader,
    audio_rx: &mut tokio_mpsc::UnboundedReceiver<AudioCmd>,
    event_tx: &crossbeam_channel::Sender<AssemblyAIEvent>,
    user_disconnected: &Arc<AtomicBool>,
    pending_chunks: &Arc<std::sync::atomic::AtomicUsize>,
    write_guard: &AsrWsWriteGuard,
    api_key: &str,
    keepalive_interval: Duration,
) -> DisconnectKind {
    let mut keep_alive = tokio::time::interval(keepalive_interval);
    keep_alive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_outbound = tokio::time::Instant::now();

    loop {
        tokio::select! {
            // Idle keepalive: a WS Ping control frame during quiet periods keeps
            // the AssemblyAI session socket warm when the audio cadence stalls
            // (M2 / audio-graph-63be). Guarded by `last_outbound` so it never
            // fires while audio is actively flowing.
            _ = keep_alive.tick() => {
                if last_outbound.elapsed() >= keepalive_interval {
                    if let Err(e) = write_guard
                        .send_ping(writer, Vec::new())
                        .await
                    {
                        let message = crate::error::redacted_provider_diagnostic(
                            &format!("keepalive failed: {e}"),
                            [api_key],
                        );
                        log::error!("AssemblyAI: failed to send keepalive: {message}");
                        return DisconnectKind::NetworkError(message);
                    }
                    last_outbound = tokio::time::Instant::now();
                }
            }

            cmd = audio_rx.recv() => {
                match cmd {
                    Some(AudioCmd::Chunk(bytes)) => {
                        // INVARIANT (decrement-before-send; review m3): this
                        // client cannot replay a failed chunk, so the decrement
                        // happens up front — the chunk leaves the queue whether the
                        // write succeeds or errors, and must not keep counting
                        // against the cap. Opposite of OpenAI-realtime, which holds
                        // the decrement for replay; see the Deepgram Chunk arm for
                        // the full cross-client note. Adding replay here without
                        // moving the decrement past the write would double-count.
                        pending_chunks.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                        if let Err(e) = write_guard
                            .send_binary(writer, AsrTransportPayloadKind::Audio, bytes)
                            .await
                        {
                            let policy_blocked = e.is_policy_blocked();
                            let message = crate::error::redacted_provider_diagnostic(
                                &format!("send failed: {e}"),
                                [api_key],
                            );
                            log::error!("AssemblyAI: failed to send audio: {message}");
                            return if policy_blocked {
                                DisconnectKind::PolicyBlocked(message)
                            } else {
                                DisconnectKind::NetworkError(message)
                            };
                        }
                        last_outbound = tokio::time::Instant::now();
                    }
                    Some(AudioCmd::Stop) => {
                        // Graceful user-initiated close.
                        let terminate_msg = json!({ "type": "Terminate" });
                        let _ = writer
                            .send(Message::Text(terminate_msg.to_string().into()))
                            .await;
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
                        handle_server_message_with_key(&text, event_tx, api_key);
                    }
                    Ok(Message::Close(frame)) => {
                        if user_disconnected.load(Ordering::SeqCst) {
                            return DisconnectKind::UserRequested;
                        }
                        let reason = frame
                            .map(|f| {
                                let code: u16 = f.code.into();
                                close_frame_diagnostic(code, f.reason.as_ref())
                            })
                            .unwrap_or_else(|| "no_frame".into());
                        log::info!("AssemblyAI: server closed connection {reason}");
                        return DisconnectKind::ServerClose(reason);
                    }
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {
                        // Protocol-level frames; nothing to do.
                    }
                    Ok(Message::Binary(_)) => {
                        log::warn!("AssemblyAI: unexpected binary message");
                    }
                    Err(tungstenite::Error::ConnectionClosed)
                    | Err(tungstenite::Error::AlreadyClosed) => {
                        return DisconnectKind::NetworkError("connection closed".into());
                    }
                    Err(tungstenite::Error::Protocol(e)) => {
                        let message =
                            crate::error::redacted_provider_diagnostic(&e.to_string(), [api_key]);
                        return DisconnectKind::ProtocolError(message);
                    }
                    Err(e) => {
                        let message =
                            crate::error::redacted_provider_diagnostic(&e.to_string(), [api_key]);
                        log::error!("AssemblyAI: WebSocket read error: {message}");
                        return DisconnectKind::NetworkError(message);
                    }
                }
            }
        }
    }
}

/// Parse a single server JSON message and emit appropriate events.
#[cfg(test)]
pub(super) fn handle_server_message(text: &str, tx: &crossbeam_channel::Sender<AssemblyAIEvent>) {
    handle_server_message_with_key(text, tx, "");
}

fn handle_server_message_with_key(
    text: &str,
    tx: &crossbeam_channel::Sender<AssemblyAIEvent>,
    api_key: &str,
) {
    let parsed: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("AssemblyAI: invalid JSON: {e}");
            let _ = tx.send(AssemblyAIEvent::Error {
                message: format!("Invalid server JSON: {e}"),
            });
            return;
        }
    };

    // The v3 Universal-Streaming endpoint keys every message on a top-level
    // `type` field (`Begin`, `SpeechStarted`, `Turn`, `SpeakerRevision`,
    // `Termination`, `Error`). Errors carry bounded diagnostics; every other
    // typed frame is forwarded verbatim as a `ServerMessage` for the
    // source-aware v3 parser in `speech/mod.rs` to decode.
    if let Some(message_type) = parsed.get("type").and_then(|v| v.as_str()) {
        if message_type == "Error" {
            let message = assemblyai_error_diagnostic(&parsed);
            let message = crate::error::redacted_provider_diagnostic(&message, [api_key]);
            log::error!("AssemblyAI: server error: {message}");
            let _ = tx.send(AssemblyAIEvent::Error { message });
            return;
        }

        let _ = tx.send(AssemblyAIEvent::ServerMessage {
            frame: AssemblyAiServerMessageFrame::new(text, &parsed),
            received_at_ms: current_unix_millis(),
        });
        return;
    }

    // The v3 endpoint never sends a message without a `type` field. A frame
    // that reaches here is malformed or from an unexpected source; log a
    // bounded diagnostic and drop it rather than fabricate a transcript event.
    log::debug!(
        "AssemblyAI: server message missing v3 `type` field fields={}",
        json_field_count(&parsed)
    );
}

fn assemblyai_error_diagnostic(parsed: &Value) -> String {
    let message_len = parsed
        .get("error")
        .or_else(|| parsed.get("message"))
        .and_then(|value| value.as_str())
        .map(|value| value.chars().count());
    let code = json_string_field(parsed, &["code", "error_code", "status"]);
    let request_id = json_string_field(parsed, &["request_id", "requestId", "id"]);

    match (code, request_id, message_len) {
        (Some(code), Some(request_id), Some(message_len)) => {
            format!(
                "AssemblyAI error code={code} request_id={request_id} message_len={message_len}"
            )
        }
        (Some(code), None, Some(message_len)) => {
            format!("AssemblyAI error code={code} message_len={message_len}")
        }
        (None, Some(request_id), Some(message_len)) => {
            format!("AssemblyAI error request_id={request_id} message_len={message_len}")
        }
        (Some(code), Some(request_id), None) => {
            format!("AssemblyAI error code={code} request_id={request_id}")
        }
        (Some(code), None, None) => format!("AssemblyAI error code={code}"),
        (None, Some(request_id), None) => format!("AssemblyAI error request_id={request_id}"),
        (None, None, Some(message_len)) => format!("AssemblyAI error message_len={message_len}"),
        (None, None, None) => format!(
            "AssemblyAI error type={} fields={}",
            json_string_field(parsed, &["type"]).unwrap_or_else(|| "unknown".into()),
            json_field_count(parsed)
        ),
    }
}

fn json_string_field(parsed: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| parsed.get(*key).and_then(|value| value.as_str()))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn json_field_count(parsed: &Value) -> usize {
    parsed.as_object().map_or(0, serde_json::Map::len)
}

fn close_frame_diagnostic(code: u16, reason: &str) -> String {
    format!("code={code} reason_len={}", reason.chars().count())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert f32 PCM samples (range -1.0 ... +1.0) to little-endian i16 bytes.
fn f32_to_i16_le_bytes(samples: &[f32]) -> Vec<u8> {
    crate::audio::pcm::f32_mono_to_pcm_s16le_bytes(samples)
}

fn current_unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::ws_fixture;

    #[test]
    fn emit_session_terminated_once_dedupes_and_re_arms() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let guard = Arc::new(AtomicBool::new(false));

        // First call emits; subsequent calls on the same (un-re-armed) guard do
        // not — so a teardown reaching this from multiple arms emits once.
        assert!(emit_session_terminated_once(&tx, &guard));
        assert!(!emit_session_terminated_once(&tx, &guard));
        assert!(!emit_session_terminated_once(&tx, &guard));
        assert!(matches!(
            rx.try_recv(),
            Ok(AssemblyAIEvent::SessionTerminated)
        ));
        assert!(
            rx.try_recv().is_err(),
            "only one SessionTerminated must be sent"
        );

        // Re-arming (as the reconnect path does) lets the next session emit once.
        guard.store(false, Ordering::SeqCst);
        assert!(emit_session_terminated_once(&tx, &guard));
        assert!(matches!(
            rx.try_recv(),
            Ok(AssemblyAIEvent::SessionTerminated)
        ));
        assert!(rx.try_recv().is_err());
    }

    fn test_config() -> AssemblyAIConfig {
        AssemblyAIConfig {
            api_key: "assemblyai-test-key".into(),
            enable_diarization: false,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        }
    }

    fn with_blocked_content_egress(mut config: AssemblyAIConfig) -> AssemblyAIConfig {
        config.api_key = "aai-private-api-key".into();
        config.content_egress_policy = crate::asr::ProviderContentEgressPolicy::block("local_only");
        config
    }

    #[derive(Debug)]
    struct CapturedClientFrames {
        binary_frames: Vec<Vec<u8>>,
        text_frames: Vec<Value>,
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
        rx: &crossbeam_channel::Receiver<AssemblyAIEvent>,
        timeout: Duration,
    ) -> AssemblyAIEvent {
        tokio::time::timeout(timeout, async {
            loop {
                if let Ok(event) = rx.try_recv() {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for AssemblyAI event")
    }

    #[test]
    fn assemblyai_config_debug_redacts_api_key() {
        let config = AssemblyAIConfig {
            api_key: "aai-debug-secret".into(),
            enable_diarization: true,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        };

        let debug = format!("{config:?}");

        assert!(!debug.contains("aai-debug-secret"));
        assert!(debug.contains("<present>"));
        assert!(debug.contains("enable_diarization"));
    }

    #[test]
    fn v3_websocket_url_uses_universal_35_binary_params() {
        let mut config = test_config();
        config.enable_diarization = true;

        let url = assemblyai_v3_websocket_url(&config).expect("v3 websocket url");
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();

        assert_eq!(
            url.as_str().split('?').next(),
            Some(ASSEMBLYAI_V3_WS_ENDPOINT)
        );
        assert_eq!(
            query.get("speech_model").map(String::as_str),
            Some(DEFAULT_MODEL)
        );
        assert_eq!(query.get("sample_rate").map(String::as_str), Some("16000"));
        assert_eq!(query.get("encoding").map(String::as_str), Some("pcm_s16le"));
        assert_eq!(
            query.get("speaker_labels").map(String::as_str),
            Some("true")
        );
        assert!(!url.as_str().contains(&config.api_key));
    }

    #[test]
    fn v3_server_error_message_redacts_provider_credentials() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let api_key = "aai-v3-server-secret";
        let raw_transcript = "patient said private diagnosis";
        handle_server_message_with_key(
            &format!(
                r#"{{"type":"Error","error":"bad key {api_key} token bearer-aai-secret","transcript":"{raw_transcript}","api_key":"{api_key}"}}"#
            ),
            &tx,
            api_key,
        );

        match rx.recv().expect("error event") {
            AssemblyAIEvent::Error { message } => {
                for leaked in [
                    api_key,
                    "bearer-aai-secret",
                    "bad key",
                    "token",
                    raw_transcript,
                    "transcript",
                    "api_key",
                ] {
                    assert!(
                        !message.contains(leaked),
                        "AssemblyAI v3 server error diagnostic leaked {leaked}: {message}"
                    );
                }
                assert!(message.contains("message_len="));
            }
            other => panic!("expected error event, got {other:?}"),
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
    fn client_new_is_disconnected() {
        let client = AssemblyAIClient::new(AssemblyAIConfig {
            api_key: "key".into(),
            enable_diarization: false,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        });
        assert!(!client.is_connected());
    }

    #[test]
    fn connect_fails_without_api_key() {
        let mut client = AssemblyAIClient::new(AssemblyAIConfig {
            api_key: String::new(),
            enable_diarization: false,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        });
        let result = client.connect();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
    }

    #[test]
    fn send_audio_fails_when_disconnected() {
        let client = AssemblyAIClient::new(AssemblyAIConfig {
            api_key: "key".into(),
            enable_diarization: false,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        });
        let result = client.send_audio(&[0.5, -0.3]);
        assert!(result.is_err());
    }

    #[test]
    fn blocked_policy_rejects_non_empty_audio_before_channel_initialization() {
        let client = AssemblyAIClient::new(with_blocked_content_egress(test_config()));

        let error = client.send_audio(&[0.5, -0.3]).unwrap_err();

        assert!(error.contains("Privacy policy blocked"));
        assert!(error.contains("asr.assemblyai"));
        assert!(error.contains("local_only"));
        assert!(!error.contains("Audio channel not initialized"));
    }

    #[test]
    fn blocked_policy_allows_empty_audio_without_channel_initialization() {
        let client = AssemblyAIClient::new(with_blocked_content_egress(test_config()));

        assert!(client.send_audio(&[]).is_ok());
    }

    #[test]
    fn blocked_policy_error_redacts_secret_audio_and_transcript_like_values() {
        let client = AssemblyAIClient::new(with_blocked_content_egress(test_config()));

        let error = client.send_audio(&[0.5, -0.3]).unwrap_err();

        for forbidden in [
            "aai-private-api-key",
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
    fn v3_error_message_missing_type_is_dropped_without_event() {
        // The v3 endpoint keys every message on `type`. A frame with neither a
        // `type` nor the legacy v2 `message_type` field is not a v3 message and
        // must be dropped (logged), never surfaced as a transcript/error event.
        let (tx, rx) = crossbeam_channel::bounded(16);

        let msg = r#"{ "error": "some untyped payload", "detail": "no type field" }"#;
        handle_server_message(msg, &tx);

        assert!(
            rx.try_recv().is_err(),
            "an untyped (non-v3) frame must not emit any event"
        );
    }

    #[test]
    fn event_serialization_roundtrip() {
        let events = vec![
            AssemblyAIEvent::ServerMessage {
                frame: AssemblyAiServerMessageFrame::new(
                    r#"{"type":"Begin","id":"session"}"#,
                    &serde_json::json!({"type":"Begin","id":"session"}),
                ),
                received_at_ms: 1700000000000,
            },
            AssemblyAIEvent::SessionTerminated,
            AssemblyAIEvent::Error {
                message: "oops".into(),
            },
            AssemblyAIEvent::Reconnecting {
                attempt: 3,
                backoff_secs: 5,
            },
            AssemblyAIEvent::Reconnected,
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let _parsed: Value = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn server_message_serialization_and_debug_redact_raw_provider_frame() {
        let raw = r#"{"type":"Turn","transcript":"patient said private diagnosis","words":[{"text":"patient"}]}"#;
        let parsed: Value = serde_json::from_str(raw).unwrap();
        let event = AssemblyAIEvent::ServerMessage {
            frame: AssemblyAiServerMessageFrame::new(raw, &parsed),
            received_at_ms: 1700000000000,
        };

        let serialized = serde_json::to_string(&event).unwrap();
        let debug = format!("{event:?}");

        for surface in [serialized.as_str(), debug.as_str()] {
            assert!(!surface.contains("patient said private diagnosis"));
            assert!(!surface.contains("\"words\""));
            assert!(surface.contains("Turn"));
        }
    }

    /// Regression guard for the v2-branch removal: after dropping the dead
    /// `message_type`-keyed handling, the live v3 `type`-keyed messages must
    /// still surface as raw `ServerMessage` frames for the source-aware parser.
    #[test]
    fn v3_type_keyed_messages_surface_as_server_message_frames() {
        let (tx, rx) = crossbeam_channel::bounded(16);

        handle_server_message(
            r#"{"type":"Turn","turn_order":0,"end_of_turn":true,"transcript":"hello world"}"#,
            &tx,
        );
        handle_server_message(
            r#"{"type":"Termination","audio_duration_seconds":4.7}"#,
            &tx,
        );

        match rx.try_recv().expect("v3 Turn should emit a ServerMessage") {
            AssemblyAIEvent::ServerMessage { frame, .. } => {
                assert_eq!(frame.message_type, "Turn");
                let parsed: Value =
                    serde_json::from_str(frame.as_str()).expect("frame retains raw v3 JSON");
                assert_eq!(
                    parsed.get("transcript").and_then(Value::as_str),
                    Some("hello world")
                );
            }
            other => panic!("expected ServerMessage for v3 Turn, got {other:?}"),
        }

        match rx
            .try_recv()
            .expect("v3 Termination should emit a ServerMessage")
        {
            AssemblyAIEvent::ServerMessage { frame, .. } => {
                assert_eq!(frame.message_type, "Termination");
            }
            other => panic!("expected ServerMessage for v3 Termination, got {other:?}"),
        }

        assert!(
            rx.try_recv().is_err(),
            "no further events expected after the two v3 frames"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_io_fake_server_writes_audio_reads_final_and_stops() {
        let (url, server) = ws_fixture::spawn_scripted_server(vec![
            ws_fixture::ServerStep::send_text(
                r#"{"type":"Turn","turn_order":0,"end_of_turn":true,"turn_is_formatted":true,"end_of_turn_confidence":0.91,"transcript":"fake assembly result","words":[{"text":"fake","start":0,"end":300,"confidence":0.91}]}"#,
            ),
            ws_fixture::ServerStep::expect_binary(vec![0x61, 0x62, 0x63]),
            ws_fixture::ServerStep::expect_any_text(),
            ws_fixture::ServerStep::expect_close(),
        ])
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (mut writer, mut reader) = client_socket.split();
        let (audio_tx, mut audio_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, event_rx) = crossbeam_channel::bounded(16);
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let write_guard = AsrWsWriteGuard::new(
            "asr.assemblyai",
            crate::asr::ProviderContentEgressPolicy::allow(),
        );

        let run = tokio::spawn({
            let user_disconnected = Arc::clone(&user_disconnected);
            let pending_chunks = Arc::clone(&pending_chunks);
            async move {
                run_io(
                    &mut writer,
                    &mut reader,
                    &mut audio_rx,
                    &event_tx,
                    &user_disconnected,
                    &pending_chunks,
                    &write_guard,
                    "assemblyai-test-key",
                )
                .await
            }
        });

        audio_tx
            .send(AudioCmd::Chunk(vec![0x61, 0x62, 0x63]))
            .expect("queue binary audio");

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            AssemblyAIEvent::ServerMessage {
                frame,
                received_at_ms,
            } => {
                assert_eq!(frame.message_type, "Turn");
                let turn: Value = serde_json::from_str(frame.as_str()).expect("parse fake Turn");
                assert_eq!(
                    turn.get("transcript").and_then(Value::as_str),
                    Some("fake assembly result")
                );
                assert_eq!(turn.get("end_of_turn").and_then(Value::as_bool), Some(true));
                assert_eq!(
                    turn.get("turn_is_formatted").and_then(Value::as_bool),
                    Some(true)
                );
                assert_eq!(
                    turn.get("end_of_turn_confidence").and_then(Value::as_f64),
                    Some(0.91)
                );
                assert!(received_at_ms > 0);
            }
            other => panic!("expected v3 server message from fake server, got {other:?}"),
        }

        audio_tx.send(AudioCmd::Stop).expect("queue stop");

        let disconnect = tokio::time::timeout(Duration::from_secs(2), run)
            .await
            .expect("run_io should exit after stop")
            .expect("run_io task panicked");
        assert!(
            matches!(disconnect, DisconnectKind::UserRequested),
            "stop command should be classified as user-requested, got {disconnect:?}"
        );
        assert_eq!(
            pending_chunks.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "sent audio chunk must decrement pending count"
        );

        let client_frames = tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
        assert_eq!(
            client_frames.first(),
            Some(&ws_fixture::ClientFrame::Binary(vec![0x61, 0x62, 0x63]))
        );
        let Some(ws_fixture::ClientFrame::Text(terminate_frame)) = client_frames.get(1) else {
            panic!("stop command should send v3 Terminate text frame, got {client_frames:?}");
        };
        let terminate: Value =
            serde_json::from_str(terminate_frame).expect("client terminate frame json");
        assert_eq!(
            terminate.get("type").and_then(Value::as_str),
            Some("Terminate"),
            "stop command should send v3 Terminate"
        );
        assert_eq!(client_frames.get(2), Some(&ws_fixture::ClientFrame::Close));
    }

    /// M2 / audio-graph-63be: after a quiet period (no audio) the AssemblyAI
    /// `run_io` loop must emit a WS `Ping` keepalive so the server socket does
    /// not idle-close. Uses a short injected interval so the test does not wait
    /// the production 8s.
    #[tokio::test(flavor = "current_thread")]
    async fn run_io_sends_ping_keepalive_after_quiet_period() {
        let (ping_tx, ping_rx) = tokio::sync::oneshot::channel();
        let (url, server) = ws_fixture::spawn_server(move |mut websocket| async move {
            // The client sends no audio, so the first frame the server sees must
            // be the keepalive Ping.
            let saw_ping = loop {
                match tokio::time::timeout(Duration::from_secs(2), websocket.next()).await {
                    Ok(Some(Ok(Message::Ping(_)))) => break true,
                    Ok(Some(Ok(_))) => continue,
                    Ok(Some(Err(_))) | Ok(None) | Err(_) => break false,
                }
            };
            let _ = ping_tx.send(saw_ping);
            let _ = websocket.close(None).await;
        })
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (mut writer, mut reader) = client_socket.split();
        let (_audio_tx, mut audio_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, _event_rx) = crossbeam_channel::bounded(16);
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let write_guard = AsrWsWriteGuard::new(
            "asr.assemblyai",
            crate::asr::ProviderContentEgressPolicy::allow(),
        );

        let run = tokio::spawn(async move {
            run_io_with_keepalive_interval(
                &mut writer,
                &mut reader,
                &mut audio_rx,
                &event_tx,
                &user_disconnected,
                &pending_chunks,
                &write_guard,
                "assemblyai-test-key",
                Duration::from_millis(50),
            )
            .await
        });

        let saw_ping = tokio::time::timeout(Duration::from_secs(3), ping_rx)
            .await
            .expect("server should observe a frame before timeout")
            .expect("ping channel should not drop");
        assert!(
            saw_ping,
            "quiet AssemblyAI run_io must send a Ping keepalive frame"
        );

        run.abort();
        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;
    }

    /// Regression for audio-graph-7086 / review B1: drive the PRODUCTION
    /// `open_ws_url` request-builder path (NOT `ws_fixture::connect_client`,
    /// which auto-injects the upgrade headers) against a fixture listener with
    /// an obviously-fake key. Before the fix, the hand-built `http::Request`
    /// carried only `Authorization`, so `generate_request` failed with
    /// `Protocol(InvalidHeader("sec-websocket-key"))` before any TCP and
    /// AssemblyAI could never connect. The handshake succeeding here proves the
    /// five mandatory WS headers now reach the wire in production, and the
    /// captured request confirms the auth header is present and the key never
    /// appears in the URL.
    #[tokio::test(flavor = "current_thread")]
    async fn open_ws_production_path_handshakes_with_mandatory_headers() {
        // If the client request were missing mandatory upgrade headers this
        // handshake would never complete — the test would fail at connect.
        let (addr, server) =
            crate::ws_request::test_support::spawn_header_capturing_ws_server().await;

        let config = AssemblyAIConfig {
            api_key: "test-key-not-real".into(),
            enable_diarization: false,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        };
        let url = format!("ws://{addr}/v3/ws?speech_model=universal-3-5-pro");

        let (mut writer, _reader) = open_ws_url(&config, &url)
            .await
            .expect("production open_ws_url must handshake against the fixture");
        // Cleanly close so the server task finishes.
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
        assert!(
            captured_headers
                .iter()
                .any(|(name, value)| name == "authorization" && value == "test-key-not-real"),
            "production handshake must carry the Authorization header: {captured_headers:?}"
        );
        assert!(
            !captured_uri.contains("test-key-not-real"),
            "the API key must never appear in the request URI: {captured_uri}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_io_blocked_policy_writes_no_audio_frame() {
        let (frame_tx, frame_rx) = tokio::sync::oneshot::channel();
        let (url, server) = ws_fixture::spawn_server(move |websocket| async move {
            let _ = frame_tx.send(first_client_content_frame(websocket).await);
        })
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (mut writer, mut reader) = client_socket.split();
        let (audio_tx, mut audio_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, _event_rx) = crossbeam_channel::bounded(16);
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let config = with_blocked_content_egress(test_config());
        let write_guard = AsrWsWriteGuard::new("asr.assemblyai", config.content_egress_policy);
        let api_key = config.api_key.clone();

        let run = tokio::spawn({
            let user_disconnected = Arc::clone(&user_disconnected);
            let pending_chunks = Arc::clone(&pending_chunks);
            async move {
                run_io(
                    &mut writer,
                    &mut reader,
                    &mut audio_rx,
                    &event_tx,
                    &user_disconnected,
                    &pending_chunks,
                    &write_guard,
                    &api_key,
                )
                .await
            }
        });

        audio_tx
            .send(AudioCmd::Chunk(vec![1, 2, 3, 4]))
            .expect("queue binary audio");

        let disconnect = tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .expect("run_io should exit after policy block")
            .expect("run_io task panicked");
        match disconnect {
            DisconnectKind::PolicyBlocked(message) => {
                assert!(message.contains("Privacy policy blocked"));
                assert!(message.contains("asr.assemblyai"));
                assert!(message.contains("local_only"));
                for forbidden in ["aai-private-api-key", "1, 2, 3, 4"] {
                    assert!(
                        !message.contains(forbidden),
                        "policy error leaked {forbidden}: {message}"
                    );
                }
            }
            other => panic!("expected policy-blocked disconnect, got {other:?}"),
        }
        assert_eq!(
            pending_chunks.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "blocked writer still consumed the queued chunk from the local buffer"
        );

        let observed = tokio::time::timeout(Duration::from_secs(1), frame_rx)
            .await
            .expect("server should report whether a content frame arrived")
            .expect("server frame channel should not drop");
        assert_eq!(
            observed, None,
            "blocked audio must not write a binary content frame"
        );

        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
    }

    #[tokio::test(flavor = "current_thread")]
    #[ignore = "requires ASSEMBLYAI_API_KEY and live AssemblyAI v3 network access"]
    async fn live_smoke_assemblyai_v3_websocket_accepts_binary_audio_and_terminates() {
        let api_key = std::env::var("ASSEMBLYAI_API_KEY")
            .expect("set ASSEMBLYAI_API_KEY to run the ignored AssemblyAI live smoke");
        let api_key = api_key.trim().to_string();
        assert!(
            !api_key.is_empty(),
            "ASSEMBLYAI_API_KEY must not be empty for live smoke"
        );

        let config = AssemblyAIConfig {
            api_key: api_key.clone(),
            enable_diarization: false,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        };
        let (mut writer, mut reader) = open_ws(&config).await.unwrap_or_else(|error| {
            panic!(
                "AssemblyAI live smoke connect failed: {}",
                crate::error::redacted_provider_diagnostic(&error, [&api_key])
            )
        });

        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut parser = AssemblyAiV3Parser::new("live-smoke");
        let mut saw_begin = false;
        let mut saw_requested_model = false;
        let mut saw_termination = false;

        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let Some(message) = tokio::time::timeout(remaining, reader.next())
                .await
                .expect("timed out waiting for AssemblyAI live smoke response")
            else {
                break;
            };

            match message.unwrap_or_else(|error| {
                panic!(
                    "AssemblyAI live smoke read failed: {}",
                    crate::error::redacted_provider_diagnostic(&error.to_string(), [&api_key])
                )
            }) {
                Message::Text(text) => {
                    let value: Value = serde_json::from_str(&text).unwrap_or_else(|error| {
                        panic!(
                            "AssemblyAI live smoke invalid JSON response: {}",
                            crate::error::redacted_provider_diagnostic(
                                &format!("{error}: {text}"),
                                [&api_key],
                            )
                        )
                    });
                    if value.get("type").and_then(Value::as_str) == Some("Error") {
                        panic!(
                            "AssemblyAI live smoke provider error: {}",
                            crate::error::redacted_provider_diagnostic(&text, [&api_key])
                        );
                    }

                    let parsed = parser
                        .parse_message(&text, current_unix_millis())
                        .unwrap_or_else(|error| {
                            panic!(
                                "AssemblyAI live smoke parser rejected response: {}",
                                crate::error::redacted_provider_diagnostic(
                                    &format!("{error:?}: {text}"),
                                    [&api_key],
                                )
                            )
                        });

                    if parsed.session_id.is_some() {
                        saw_begin = true;
                        saw_requested_model = value
                            .get("configuration")
                            .and_then(|configuration| configuration.get("model"))
                            .and_then(Value::as_str)
                            .is_some_and(|model| model == DEFAULT_MODEL);

                        let one_frame_silence_pcm16 = vec![0_u8; 800 * 2];
                        writer
                            .send(Message::Binary(one_frame_silence_pcm16.into()))
                            .await
                            .unwrap_or_else(|error| {
                                panic!(
                                    "AssemblyAI live smoke audio send failed: {}",
                                    crate::error::redacted_provider_diagnostic(
                                        &error.to_string(),
                                        [&api_key],
                                    )
                                )
                            });
                        writer
                            .send(Message::Text(
                                json!({ "type": "Terminate" }).to_string().into(),
                            ))
                            .await
                            .unwrap_or_else(|error| {
                                panic!(
                                    "AssemblyAI live smoke terminate send failed: {}",
                                    crate::error::redacted_provider_diagnostic(
                                        &error.to_string(),
                                        [&api_key],
                                    )
                                )
                            });
                    }

                    if parsed.terminated {
                        saw_termination = true;
                        break;
                    }
                }
                Message::Close(frame) => {
                    if !saw_termination {
                        panic!(
                            "AssemblyAI live smoke closed before Termination: {}",
                            crate::error::redacted_provider_diagnostic(
                                &format!("{frame:?}"),
                                [&api_key],
                            )
                        );
                    }
                    break;
                }
                Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
            }
        }

        let _ = writer.close().await;
        assert!(saw_begin, "AssemblyAI live smoke returned no Begin message");
        assert!(
            saw_requested_model,
            "AssemblyAI live smoke Begin response did not echo requested {DEFAULT_MODEL} model"
        );
        assert!(
            saw_termination,
            "AssemblyAI live smoke returned no Termination message"
        );
    }

    #[test]
    fn backoff_schedule_matches_spec() {
        // Shared crate-level ladder (review n2): fast head + cold-restart tail
        // (review m1).
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), Some(20));
        assert_eq!(backoff_for_attempt(11), None);
    }

    #[test]
    fn next_reconnect_step_increments_exactly_once_per_attempt() {
        assert_eq!(
            next_reconnect_step(0),
            ReconnectStep::Retry {
                attempt: 1,
                backoff_secs: 1
            }
        );
        assert_eq!(
            next_reconnect_step(1),
            ReconnectStep::Retry {
                attempt: 2,
                backoff_secs: 2
            }
        );
        // Continues into the cold-restart tail past attempt 4 (review m1).
        assert_eq!(
            next_reconnect_step(4),
            ReconnectStep::Retry {
                attempt: 5,
                backoff_secs: 20
            }
        );
        assert_eq!(
            next_reconnect_step(10),
            ReconnectStep::GiveUp { attempted: 10 }
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reconnect_open_failure_does_not_reenter_run_io_on_stale_socket() {
        let (url, server) = ws_fixture::spawn_server(|mut websocket| async move {
            let _ = websocket.close(None).await;
        })
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (writer, reader) = client_socket.split();
        let (_audio_tx, audio_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, event_rx) = crossbeam_channel::bounded(16);
        let connected = Arc::new(AtomicBool::new(true));
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
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

        let handle = tokio::spawn(session_task(AssemblyAISessionCtx {
            writer,
            reader,
            audio_rx,
            config: test_config(),
            event_tx,
            connected: Arc::clone(&connected),
            user_disconnected: Arc::clone(&user_disconnected),
            pending_chunks,
            reconnect_opener: Some(opener),
            run_io_entries: Some(Arc::clone(&run_io_entries)),
        }));

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            AssemblyAIEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                assert_eq!(attempt, 1);
                assert_eq!(backoff_secs, 1);
            }
            other => panic!("expected first Reconnecting event, got {other:?}"),
        }
        assert_eq!(
            run_io_entries.load(Ordering::SeqCst),
            1,
            "initial disconnect should have entered run_io once"
        );

        match recv_event(&event_rx, Duration::from_secs(2)).await {
            AssemblyAIEvent::Error { message } => {
                assert!(message.contains("Reconnect attempt 1 failed"));
            }
            other => panic!("expected reconnect failure error, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            AssemblyAIEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                assert_eq!(attempt, 2);
                assert_eq!(backoff_secs, 2);
            }
            other => panic!("expected second Reconnecting event, got {other:?}"),
        }
        assert_eq!(opener_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            run_io_entries.load(Ordering::SeqCst),
            1,
            "failed reconnect must stay in the reconnect ladder, not re-enter run_io with stale socket halves"
        );

        user_disconnected.store(true, Ordering::SeqCst);
        match recv_event(&event_rx, Duration::from_secs(2)).await {
            AssemblyAIEvent::SessionTerminated => {}
            other => panic!("expected cancellation termination, got {other:?}"),
        }
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("session task should exit during reconnect backoff")
            .expect("session task panicked");
        assert!(
            !connected.load(Ordering::SeqCst),
            "cancelled reconnect must leave connected=false"
        );
        assert_eq!(
            opener_calls.load(Ordering::SeqCst),
            1,
            "cancel during backoff must not start another reconnect open"
        );
        assert!(
            event_rx
                .try_iter()
                .all(|event| !matches!(event, AssemblyAIEvent::Reconnected)),
            "cancel during backoff must not emit Reconnected"
        );

        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_task_successful_reconnect_resumes_run_io_on_fresh_socket() {
        let (initial_url, initial_server) = ws_fixture::spawn_server(|mut websocket| async move {
            let _ = websocket.close(None).await;
        })
        .await;

        let client_socket = ws_fixture::connect_client(&initial_url).await;
        let (writer, reader) = client_socket.split();
        let (audio_tx, audio_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, event_rx) = crossbeam_channel::bounded(32);
        let connected = Arc::new(AtomicBool::new(true));
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let run_io_entries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let opener_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (reconnected_messages_tx, mut reconnected_messages_rx) =
            tokio::sync::mpsc::unbounded_channel::<CapturedClientFrames>();

        let opener: ReconnectOpener = {
            let opener_calls = Arc::clone(&opener_calls);
            Arc::new(move |_config| {
                let opener_calls = Arc::clone(&opener_calls);
                let reconnected_messages_tx = reconnected_messages_tx.clone();
                Box::pin(async move {
                    opener_calls.fetch_add(1, Ordering::SeqCst);
                    let (url, _server) = ws_fixture::spawn_server(move |mut websocket| async move {
                        websocket
                            .send(Message::Text(
                                r#"{"type":"Turn","turn_order":1,"end_of_turn":true,"turn_is_formatted":true,"end_of_turn_confidence":0.87,"transcript":"assembly after reconnect","words":[{"text":"assembly","start":0,"end":300,"confidence":0.87}]}"#
                                    .into(),
                            ))
                            .await
                            .expect("send fake final transcript");

                        let mut client_frames = CapturedClientFrames {
                            binary_frames: Vec::new(),
                            text_frames: Vec::new(),
                        };
                        while let Some(frame) = websocket.next().await {
                            match frame.expect("reconnected AssemblyAI server frame") {
                                Message::Text(text) => {
                                    let parsed: Value = serde_json::from_str(&text)
                                        .expect("client text frame json");
                                    let is_terminate = parsed
                                        .get("type")
                                        .and_then(Value::as_str)
                                        .is_some_and(|value| value == "Terminate");
                                    client_frames.text_frames.push(parsed);
                                    if is_terminate {
                                        break;
                                    }
                                }
                                Message::Binary(bytes) => {
                                    client_frames.binary_frames.push(bytes.to_vec());
                                }
                                Message::Close(_) => break,
                                _ => {}
                            }
                        }
                        let _ = reconnected_messages_tx.send(client_frames);
                    })
                    .await;

                    let socket = ws_fixture::connect_client(&url).await;
                    Ok(socket.split())
                })
            })
        };

        let handle = tokio::spawn(session_task(AssemblyAISessionCtx {
            writer,
            reader,
            audio_rx,
            config: test_config(),
            event_tx,
            connected: Arc::clone(&connected),
            user_disconnected: Arc::clone(&user_disconnected),
            pending_chunks: Arc::clone(&pending_chunks),
            reconnect_opener: Some(opener),
            run_io_entries: Some(Arc::clone(&run_io_entries)),
        }));

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            AssemblyAIEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                assert_eq!(attempt, 1);
                assert_eq!(backoff_secs, 1);
            }
            other => panic!("expected first Reconnecting event, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(3)).await {
            AssemblyAIEvent::Reconnected => {}
            other => panic!("expected Reconnected event, got {other:?}"),
        }
        assert!(
            connected.load(Ordering::SeqCst),
            "successful reconnect must mark the session connected"
        );

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            AssemblyAIEvent::ServerMessage { frame, .. } => {
                assert_eq!(frame.message_type, "Turn");
                assert!(frame.as_str().contains("assembly after reconnect"));
            }
            other => panic!("expected v3 server message after reconnect, got {other:?}"),
        }

        pending_chunks.store(1, Ordering::SeqCst);
        audio_tx
            .send(AudioCmd::Chunk(vec![0x61, 0x62, 0x63]))
            .expect("queue binary audio after reconnect");
        audio_tx.send(AudioCmd::Stop).expect("queue stop");

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("session task should exit after stop")
            .expect("session task panicked");
        assert!(
            !connected.load(Ordering::SeqCst),
            "stopped session must leave connected=false"
        );
        assert_eq!(
            opener_calls.load(Ordering::SeqCst),
            1,
            "successful reconnect should use exactly one reconnect opener call"
        );
        assert_eq!(
            run_io_entries.load(Ordering::SeqCst),
            2,
            "session task must resume run_io with the fresh socket after reconnect"
        );
        assert_eq!(
            pending_chunks.load(Ordering::SeqCst),
            0,
            "audio sent on the reconnected socket must decrement pending count"
        );
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            AssemblyAIEvent::SessionTerminated => {}
            other => panic!("expected SessionTerminated after clean stop, got {other:?}"),
        }

        let client_frames =
            tokio::time::timeout(Duration::from_secs(1), reconnected_messages_rx.recv())
                .await
                .expect("reconnected server should report client messages")
                .expect("reconnected server sender dropped");
        assert_eq!(
            client_frames.binary_frames.first().map(Vec::as_slice),
            Some(&[0x61, 0x62, 0x63][..])
        );
        assert!(
            client_frames.text_frames.iter().any(|value| value
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|value| value == "Terminate")),
            "stop command should send v3 Terminate on the reconnected socket"
        );

        tokio::time::timeout(Duration::from_secs(1), initial_server)
            .await
            .expect("initial server task should finish")
            .expect("initial server task panicked");
    }
}
