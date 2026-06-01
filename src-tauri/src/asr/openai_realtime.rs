//! OpenAI Realtime transcription (STT) WebSocket client.
//!
//! Connects to the OpenAI Realtime API via WebSocket and streams audio for
//! low-latency speech-to-text using the `gpt-realtime-whisper` transcription
//! session type (ADR-0002 Wave A — the transcription leg only; voice
//! speech-to-speech is a separate provider, B18).
//!
//! # Protocol overview (GA — no `OpenAI-Beta` header)
//!
//! 1. Open WSS connection to
//!    `wss://api.openai.com/v1/realtime?model=<model>` with an
//!    `Authorization: Bearer {api_key}` header on the upgrade request.
//! 2. Immediately send a `session.update` configuring a transcription session
//!    with an `audio.input.format` **object** `{"type":"audio/pcm","rate":24000}`
//!    plus `transcription.model` (+ optional `language`). Wait for the server
//!    `session.updated` before streaming.
//! 3. Stream audio as `input_audio_buffer.append` text frames whose `audio`
//!    field is base64 of PCM16 LE, 24 kHz mono.
//! 4. End an utterance with `input_audio_buffer.commit` (manual commit —
//!    `gpt-realtime-whisper` does not support server VAD / `turn_detection`).
//! 5. Read transcript events keyed by `item_id`:
//!    `conversation.item.input_audio_transcription.delta` (accumulate),
//!    `.completed` (replace with the final transcript), `.failed`, and a
//!    top-level `error` frame.
//!
//! # Threading model
//!
//! Identical to the Deepgram / AssemblyAI clients: the public API is
//! **synchronous** (called from `std::thread` workers in the speech
//! processor). Internally a dedicated tokio runtime drives the WebSocket;
//! audio is forwarded from the caller's thread to the async writer via an
//! unbounded `tokio::sync::mpsc` channel, and events flow back through a
//! `crossbeam_channel` that the speech processor consumes.
//!
//! # Reconnect policy
//!
//! Realtime sessions are capped at 60 minutes and have **no resume**: on a
//! drop we open a fresh socket, re-send `session.update`, and treat all
//! `item_id`s as a new namespace (the parser's accumulator is reset). Reconnect
//! uses the same 1s/2s/5s/10s exponential backoff ladder as the other
//! streaming clients (`backoff_for_attempt`).

use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{self, Message, client::IntoClientRequest},
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Default OpenAI Realtime transcription model (native streaming whisper).
pub const DEFAULT_MODEL: &str = "gpt-realtime-whisper";
/// The only sample rate the GA realtime audio input accepts: 24 kHz mono.
pub const REALTIME_SAMPLE_RATE: u32 = 24_000;

/// Events emitted by the OpenAI Realtime transcription client to downstream
/// consumers. Mirrors [`crate::asr::deepgram::DeepgramEvent`] in shape so the
/// speech processor can drive it with the same control flow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum OpenAiRealtimeEvent {
    /// A transcript result correlated to a provider `item_id`.
    ///
    /// `is_final` is `false` for accumulated `delta` events (interim display)
    /// and `true` for the `completed` event carrying the full transcript.
    #[serde(rename = "transcript")]
    Transcript {
        text: String,
        item_id: String,
        is_final: bool,
    },
    /// A non-fatal error occurred (top-level `error` frame, a transcription
    /// `failed` event, or a local parse failure). The socket stays open.
    #[serde(rename = "error")]
    Error { message: String },
    /// The connection has been established and the transcription session has
    /// been configured (`session.updated` received).
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
    #[serde(rename = "reconnected")]
    Reconnected,
}

/// Configuration for an OpenAI Realtime transcription session.
#[derive(Debug, Clone)]
pub struct OpenAiRealtimeConfig {
    /// OpenAI API key (Bearer token). Hydrated at runtime from
    /// `credentials.yaml` (`openai_api_key`) — never persisted in settings.
    pub api_key: String,
    /// Transcription model id. Defaults to [`DEFAULT_MODEL`].
    pub model: String,
    /// Optional BCP-47 language hint (e.g. `"en"`). `None` lets the model
    /// auto-detect.
    pub language: Option<String>,
    /// Sample rate advertised to the provider. GA only supports 24 kHz; the
    /// client resamples the pipeline's 16 kHz audio up to this rate before
    /// sending.
    pub sample_rate: u32,
}

impl Default for OpenAiRealtimeConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: DEFAULT_MODEL.to_string(),
            language: None,
            sample_rate: REALTIME_SAMPLE_RATE,
        }
    }
}

// ---------------------------------------------------------------------------
// Internal message passed from sync send_audio()/commit() -> async writer task
// ---------------------------------------------------------------------------

/// Hard cap on the audio-chunk backlog during a prolonged reconnect (see
/// `pending_chunks` on [`OpenAiRealtimeClient`]). ~10s worth of 50ms chunks —
/// mirrors the Deepgram / AssemblyAI clients.
const AUDIO_BUFFER_MAX_CHUNKS: usize = 200;

#[derive(Debug)]
enum AudioCmd {
    /// Base64-encoded PCM16 24 kHz chunk ready to send as
    /// `input_audio_buffer.append`.
    Chunk(String),
    /// Commit the buffered audio as an utterance (`input_audio_buffer.commit`).
    Commit,
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

/// An OpenAI Realtime transcription (STT) client.
///
/// The public methods (`connect`, `send_audio`, `commit`, `disconnect`,
/// `event_rx`) are all **synchronous** — they block the caller's thread just
/// long enough to hand off work to the internal async runtime. This matches
/// the threading model used by the speech processor where worker threads run
/// in `std::thread`.
pub struct OpenAiRealtimeClient {
    config: OpenAiRealtimeConfig,
    /// crossbeam event channel — writer side (background reader task pushes here).
    event_tx: crossbeam_channel::Sender<OpenAiRealtimeEvent>,
    /// crossbeam event channel — reader side (speech processor consumes this).
    event_rx: crossbeam_channel::Receiver<OpenAiRealtimeEvent>,
    /// Whether the WebSocket is connected **and the transcription session has
    /// been confirmed** by the server (`session.updated`). Set to `true` only
    /// when the readiness frame is parsed — never merely on socket open — so it
    /// matches the contract of [`OpenAiRealtimeEvent::Connected`].
    connected: Arc<AtomicBool>,
    /// Set to `true` when the user has explicitly called `disconnect()`.
    ///
    /// Used by the session task to distinguish a user-initiated teardown (do
    /// not auto-reconnect) from a network error or server close (auto-reconnect
    /// with exponential backoff).
    user_disconnected: Arc<AtomicBool>,
    /// One-shot guard ensuring `Disconnected` is emitted **at most once** per
    /// teardown. `disconnect()`/`Drop` and the session task can both observe the
    /// same shutdown; routing every `Disconnected` emission through this guard
    /// (via [`emit_disconnected_once`]) collapses the duplicate so downstream
    /// state machines see a single teardown event.
    disconnected_emitted: Arc<AtomicBool>,
    /// Tokio runtime that owns the WebSocket tasks.
    rt: Option<tokio::runtime::Runtime>,
    /// Sender for audio commands -> async writer task.
    audio_tx: Option<tokio_mpsc::UnboundedSender<AudioCmd>>,
    /// Approximate count of audio chunks buffered in `audio_tx` awaiting
    /// transmission. See [`AUDIO_BUFFER_MAX_CHUNKS`] for the rationale; mirrors
    /// the Deepgram client's `pending_chunks` backlog cap.
    pending_chunks: Arc<std::sync::atomic::AtomicUsize>,
    /// Handle to the session task (for join on shutdown).
    _session_handle: Option<tokio::task::JoinHandle<()>>,
}

impl OpenAiRealtimeClient {
    /// Create a new (disconnected) OpenAI Realtime transcription client.
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
            pending_chunks: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            _session_handle: None,
        }
    }

    // ------------------------------------------------------------------
    // Connect
    // ------------------------------------------------------------------

    /// Connect to the OpenAI Realtime transcription API.
    ///
    /// Blocks the caller until the WebSocket is open and the transcription
    /// session has been configured (`session.update` sent), then spawns a
    /// background session task on an internal tokio runtime. The session task
    /// handles audio writing, server message reading, and automatic reconnect
    /// with exponential backoff (re-sending `session.update` on each reconnect).
    pub fn connect(&mut self) -> Result<(), String> {
        if self.config.api_key.trim().is_empty() {
            return Err("OpenAI API key is not configured".to_string());
        }

        // Build a dedicated multi-threaded (1 worker) tokio runtime for the WS.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("openai-realtime-ws-rt")
            .build()
            .map_err(|e| format!("Failed to create tokio runtime: {e}"))?;

        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let connected = Arc::clone(&self.connected);
        let user_disconnected = Arc::clone(&self.user_disconnected);
        let disconnected_emitted = Arc::clone(&self.disconnected_emitted);
        // Reset on (re)connect so any prior teardown flag does not poison a
        // fresh session.
        user_disconnected.store(false, Ordering::SeqCst);
        // A fresh session has not been confirmed yet — `connected` flips to
        // `true` only when the reader parses `session.updated`.
        connected.store(false, Ordering::SeqCst);
        // Re-arm the one-shot `Disconnected` guard for this session.
        disconnected_emitted.store(false, Ordering::SeqCst);
        self.pending_chunks
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let pending_chunks = Arc::clone(&self.pending_chunks);

        // Perform the blocking initial connect + session.update inside the
        // runtime so the caller sees auth / network errors immediately rather
        // than through the reconnect loop. NOTE: we deliberately do **not**
        // emit `Connected` here — the socket is merely open and the
        // `session.update` has been sent but not yet acknowledged. The session
        // task emits `Connected` once the server confirms with
        // `session.updated`, matching the event's documented contract.
        let (audio_tx, session_handle) = rt.block_on(async move {
            let (writer, reader) = open_ws(&config).await?;

            log::info!("OpenAI Realtime: WebSocket open; awaiting session.updated");

            let (atx, arx) = tokio_mpsc::unbounded_channel::<AudioCmd>();

            let session_handle = tokio::spawn(session_task(OpenAiRealtimeSessionCtx {
                writer,
                reader,
                audio_rx: arx,
                config,
                event_tx,
                connected,
                user_disconnected,
                disconnected_emitted,
                pending_chunks: Arc::clone(&pending_chunks),
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

    /// Send PCM audio data to OpenAI for transcription.
    ///
    /// The audio should be **f32 mono 16 kHz** (matching the pipeline output).
    /// The method resamples to the configured 24 kHz rate, converts to 16-bit
    /// LE PCM, base64-encodes, and queues an `input_audio_buffer.append`.
    /// Returns immediately (non-blocking).
    ///
    /// # Behaviour during auto-reconnect
    ///
    /// Only `user_disconnected` is checked — not the transient `connected`
    /// flag — so the caller can keep streaming audio during a reconnect cycle.
    /// Queued chunks flush as soon as the new socket is open.
    pub fn send_audio(&self, audio: &[f32]) -> Result<(), String> {
        if self.user_disconnected.load(Ordering::SeqCst) {
            return Err("OpenAI Realtime client has been disconnected".to_string());
        }

        if audio.is_empty() {
            return Ok(());
        }

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
                "OpenAI Realtime audio buffer full ({depth} chunks) — likely a stuck reconnect. Restart the session."
            ));
        }

        // f32 16 kHz -> 24 kHz -> i16 LE PCM -> base64.
        let resampled = resample_linear(audio, PIPELINE_SAMPLE_RATE, self.config.sample_rate);
        let pcm_bytes = f32_to_i16_le_bytes(&resampled);
        let b64 = base64::engine::general_purpose::STANDARD.encode(&pcm_bytes);

        self.pending_chunks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tx.send(AudioCmd::Chunk(b64)).map_err(|_| {
            self.pending_chunks
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            "Audio channel closed".to_string()
        })
    }

    /// Commit the buffered audio as an utterance boundary
    /// (`input_audio_buffer.commit`).
    ///
    /// `gpt-realtime-whisper` has no server VAD, so the caller drives turns by
    /// calling this at the end of each utterance. Non-blocking.
    pub fn commit(&self) -> Result<(), String> {
        if self.user_disconnected.load(Ordering::SeqCst) {
            return Err("OpenAI Realtime client has been disconnected".to_string());
        }
        let tx = self
            .audio_tx
            .as_ref()
            .ok_or_else(|| "Audio channel not initialized".to_string())?;
        tx.send(AudioCmd::Commit)
            .map_err(|_| "Audio channel closed".to_string())
    }

    // ------------------------------------------------------------------
    // Event receiver
    // ------------------------------------------------------------------

    /// Get a clone of the event receiver channel.
    ///
    /// The speech processor uses this to read `OpenAiRealtimeEvent`s.
    pub fn event_rx(&self) -> crossbeam_channel::Receiver<OpenAiRealtimeEvent> {
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

    /// Disconnect from OpenAI and clean up resources.
    ///
    /// Sends a close frame, waits for background tasks to finish, and shuts
    /// down the internal tokio runtime on Drop. Setting `user_disconnected`
    /// prevents the session task from attempting to auto-reconnect.
    pub fn disconnect(&self) {
        log::info!("OpenAiRealtimeClient: disconnecting (user-initiated)");

        self.user_disconnected.store(true, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);

        if let Some(ref tx) = self.audio_tx {
            let _ = tx.send(AudioCmd::Stop);
        }

        // Emit `Disconnected` through the one-shot guard so the session task —
        // which will independently observe this teardown via the `Stop` command
        // / `user_disconnected` flag — does not emit a second one.
        emit_disconnected_once(&self.event_tx, &self.disconnected_emitted);
    }
}

impl Drop for OpenAiRealtimeClient {
    fn drop(&mut self) {
        // Mark teardown as user-initiated so the session task exits cleanly
        // instead of trying to reconnect after we shut the runtime down.
        self.user_disconnected.store(true, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);

        if let Some(ref tx) = self.audio_tx {
            let _ = tx.send(AudioCmd::Stop);
        }
        self.audio_tx = None;

        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(std::time::Duration::from_secs(3));
        }

        log::info!("OpenAiRealtimeClient: dropped");
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
    UserRequested,
    WriterEnded,
}

/// Build the `session.update` client event that configures a transcription
/// session per the GA wire protocol (research §4.1).
///
/// The audio format is the **object** form `{"type":"audio/pcm","rate":24000}`
/// — sending the legacy string form yields
/// `expected an object, but got a string`. `turn_detection` is intentionally
/// omitted (`gpt-realtime-whisper` does not support server VAD; the caller
/// drives turns with manual `input_audio_buffer.commit`).
fn session_update_payload(config: &OpenAiRealtimeConfig) -> Value {
    let mut transcription = json!({ "model": config.model });
    if let Some(lang) = config
        .language
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        transcription["language"] = json!(lang);
    }

    json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {
                "input": {
                    "format": { "type": "audio/pcm", "rate": config.sample_rate },
                    "transcription": transcription
                }
            }
        }
    })
}

/// The realtime transcription WebSocket URL for the given model.
fn realtime_url(model: &str) -> String {
    format!("wss://api.openai.com/v1/realtime?model={model}")
}

/// Open a fresh OpenAI Realtime WebSocket and send the initial
/// `session.update`.
///
/// Used both for the initial connect and for each reconnect attempt. Realtime
/// sessions cannot resume, so the transcription config must be re-sent on every
/// (re)connect — hence `session.update` lives here rather than only at connect
/// time. We do **not** wait synchronously for `session.updated`; the server
/// buffers `input_audio_buffer.append` frames sent right after, and the reader
/// loop surfaces `session.updated`/`error` as they arrive.
async fn open_ws(config: &OpenAiRealtimeConfig) -> Result<(WsWriter, WsReader), String> {
    let url = realtime_url(&config.model);

    // `IntoClientRequest` fills in the mandatory WebSocket upgrade headers; we
    // only layer `Authorization` on top. NO `OpenAI-Beta` header (GA only).
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| format!("Failed to build WebSocket request: {e}"))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", config.api_key)
            .parse()
            .map_err(|e| format!("Invalid Authorization header: {e}"))?,
    );

    let (ws_stream, _response) = connect_async(request)
        .await
        .map_err(|e| format!("WebSocket connect failed: {e}"))?;

    let (mut writer, reader) = ws_stream.split();

    // Configure the transcription session immediately after connect.
    let update = session_update_payload(config).to_string();
    writer
        .send(Message::Text(update.into()))
        .await
        .map_err(|e| format!("Failed to send session.update: {e}"))?;

    Ok((writer, reader))
}

/// Backoff schedule per the resilience spec: 1 s, 2 s, 5 s, 10 s, then give up.
///
/// `attempt` is 1-based: 1 is the first retry after the initial disconnect.
/// Returns `None` once the budget is exhausted, which signals the session task
/// to emit a fatal error and exit.
fn backoff_for_attempt(attempt: u32) -> Option<u64> {
    match attempt {
        1 => Some(1),
        2 => Some(2),
        3 => Some(5),
        4 => Some(10),
        _ => None,
    }
}

/// Bundles everything `session_task` owns for a single OpenAI Realtime session.
/// Collapses a long function signature to one — mirrors Deepgram's
/// `DeepgramSessionCtx`.
struct OpenAiRealtimeSessionCtx {
    writer: WsWriter,
    reader: WsReader,
    audio_rx: tokio_mpsc::UnboundedReceiver<AudioCmd>,
    config: OpenAiRealtimeConfig,
    event_tx: crossbeam_channel::Sender<OpenAiRealtimeEvent>,
    connected: Arc<AtomicBool>,
    user_disconnected: Arc<AtomicBool>,
    /// One-shot guard shared with the client; see [`emit_disconnected_once`].
    disconnected_emitted: Arc<AtomicBool>,
    pending_chunks: Arc<std::sync::atomic::AtomicUsize>,
}

/// Emit [`OpenAiRealtimeEvent::Disconnected`] exactly once across all the
/// places that can observe a single teardown (`disconnect()`/`Drop` and the
/// session task's exit/reconnect paths). Returns `true` if this call was the
/// one that actually emitted, `false` if a previous call already did.
fn emit_disconnected_once(
    event_tx: &crossbeam_channel::Sender<OpenAiRealtimeEvent>,
    disconnected_emitted: &Arc<AtomicBool>,
) -> bool {
    // `swap` makes the check-and-set atomic so concurrent observers race to a
    // single winner.
    if disconnected_emitted.swap(true, Ordering::SeqCst) {
        return false;
    }
    let _ = event_tx.send(OpenAiRealtimeEvent::Disconnected);
    true
}

/// Background task owning a single OpenAI Realtime WebSocket session, including
/// reconnect logic. Mirrors the Deepgram `session_task` structure — see the
/// comments there for the full design rationale. The one OpenAI-specific
/// detail is that `open_ws` re-sends `session.update` on each reconnect (no
/// resume), and the per-session transcript accumulator (in `run_io`) starts
/// fresh because `item_id`s are a new namespace after a reconnect.
async fn session_task(ctx: OpenAiRealtimeSessionCtx) {
    let OpenAiRealtimeSessionCtx {
        writer: initial_writer,
        reader: initial_reader,
        mut audio_rx,
        config,
        event_tx,
        connected,
        user_disconnected,
        disconnected_emitted,
        pending_chunks,
    } = ctx;

    let mut writer = initial_writer;
    let mut reader = initial_reader;
    let mut reconnect_attempts: u32 = 0;
    // The readiness event `run_io` should emit when the server confirms the
    // session (`session.updated`): `Connected` for the first session, then
    // `Reconnected` after each successful reconnect.
    let mut ready_event = OpenAiRealtimeEvent::Connected;
    // A command popped from `audio_rx` whose WebSocket write failed mid-flight.
    // It is replayed on the next (reconnected) socket so a transient send error
    // never silently drops an audio chunk or — worse — an utterance commit.
    let mut pending_cmd: Option<AudioCmd> = None;

    loop {
        let disconnect = run_io(RunIoCtx {
            writer: &mut writer,
            reader: &mut reader,
            audio_rx: &mut audio_rx,
            event_tx: &event_tx,
            connected: &connected,
            user_disconnected: &user_disconnected,
            pending_chunks: &pending_chunks,
            ready_event: &ready_event,
            pending_cmd: &mut pending_cmd,
        })
        .await;

        connected.store(false, Ordering::SeqCst);

        match disconnect {
            DisconnectKind::UserRequested | DisconnectKind::WriterEnded => {
                log::info!("OpenAI Realtime session: ending ({disconnect:?})");
                emit_disconnected_once(&event_tx, &disconnected_emitted);
                break;
            }
            _ => {
                if user_disconnected.load(Ordering::SeqCst) {
                    emit_disconnected_once(&event_tx, &disconnected_emitted);
                    break;
                }

                log::warn!("OpenAI Realtime session: disconnected — {disconnect:?}");
                emit_disconnected_once(&event_tx, &disconnected_emitted);

                reconnect_attempts += 1;
                let Some(backoff) = backoff_for_attempt(reconnect_attempts) else {
                    log::error!(
                        "OpenAI Realtime session: reconnect budget exhausted after {} attempts",
                        reconnect_attempts - 1
                    );
                    let _ = event_tx.send(OpenAiRealtimeEvent::Error {
                        message: "OpenAI Realtime reconnect attempts exhausted".into(),
                    });
                    break;
                };

                log::info!(
                    "OpenAI Realtime session: reconnecting (attempt {reconnect_attempts}, backoff {backoff}s)"
                );
                let _ = event_tx.send(OpenAiRealtimeEvent::Reconnecting {
                    attempt: reconnect_attempts,
                    backoff_secs: backoff,
                });

                // Sleep for the backoff window, but bail out early on user
                // cancellation so shutdown doesn't wait up to 10s.
                let sleep = tokio::time::sleep(Duration::from_secs(backoff));
                tokio::pin!(sleep);
                loop {
                    tokio::select! {
                        _ = &mut sleep => break,
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {
                            if user_disconnected.load(Ordering::SeqCst) {
                                log::info!("OpenAI Realtime session: user cancelled during backoff");
                                emit_disconnected_once(&event_tx, &disconnected_emitted);
                                return;
                            }
                        }
                    }
                }

                // Reconnect: `open_ws` re-sends `session.update` (no resume).
                match open_ws(&config).await {
                    Ok((new_writer, new_reader)) => {
                        writer = new_writer;
                        reader = new_reader;
                        // Do NOT flip `connected` / emit `Reconnected` here: the
                        // socket is open but the session is not yet confirmed.
                        // `run_io` emits `ready_event` on `session.updated`.
                        ready_event = OpenAiRealtimeEvent::Reconnected;
                        log::info!(
                            "OpenAI Realtime session: socket reopened on attempt {reconnect_attempts}; awaiting session.updated"
                        );
                        reconnect_attempts = 0;
                    }
                    Err(e) => {
                        log::warn!(
                            "OpenAI Realtime session: reconnect attempt {reconnect_attempts} failed: {e}"
                        );
                        let _ = event_tx.send(OpenAiRealtimeEvent::Error {
                            message: format!("Reconnect attempt {reconnect_attempts} failed: {e}"),
                        });
                        // Skip run_io next iteration — just try the next backoff
                        // step.
                        continue;
                    }
                }
            }
        }
    }

    connected.store(false, Ordering::SeqCst);
    log::info!("OpenAI Realtime: session task exited");
}

/// Everything a single [`run_io`] invocation borrows from its owning
/// [`session_task`]. Bundled into one struct to keep the signature readable and
/// to thread the cross-`run_io` state (readiness event, in-flight command).
struct RunIoCtx<'a> {
    writer: &'a mut WsWriter,
    reader: &'a mut WsReader,
    audio_rx: &'a mut tokio_mpsc::UnboundedReceiver<AudioCmd>,
    event_tx: &'a crossbeam_channel::Sender<OpenAiRealtimeEvent>,
    /// Flipped to `true` when this socket's `session.updated` is parsed.
    connected: &'a Arc<AtomicBool>,
    user_disconnected: &'a Arc<AtomicBool>,
    pending_chunks: &'a Arc<std::sync::atomic::AtomicUsize>,
    /// The event to emit once the server confirms the session (`Connected` on
    /// the first socket, `Reconnected` after a reconnect).
    ready_event: &'a OpenAiRealtimeEvent,
    /// A command popped from `audio_rx` on a *previous* socket whose write
    /// failed. It is replayed first on this socket so reconnects never drop an
    /// audio chunk or utterance commit. Holds the surviving command back out
    /// again if this socket's replay also fails.
    pending_cmd: &'a mut Option<AudioCmd>,
}

/// Serialize and write a single [`AudioCmd`] to the socket.
///
/// On success a `Chunk` decrements the `pending_chunks` backlog counter (the
/// chunk is no longer awaiting transmission). On a write failure the command is
/// returned to the caller as `Err(cmd)` so it can be preserved and replayed on
/// the reconnected socket — the `pending_chunks` decrement is intentionally
/// *not* applied so the held chunk still counts against the backlog cap.
///
/// `Stop` / `None` are terminal and handled by the caller, never passed here.
async fn write_audio_cmd(
    writer: &mut WsWriter,
    pending_chunks: &Arc<std::sync::atomic::AtomicUsize>,
    cmd: AudioCmd,
) -> Result<(), AudioCmd> {
    let (payload, is_chunk) = match &cmd {
        AudioCmd::Chunk(b64) => (
            json!({ "type": "input_audio_buffer.append", "audio": b64 }),
            true,
        ),
        AudioCmd::Commit => (json!({ "type": "input_audio_buffer.commit" }), false),
        // Terminal commands are handled inline by `run_io`; never reach here.
        AudioCmd::Stop => return Ok(()),
    };

    match writer.send(Message::Text(payload.to_string().into())).await {
        Ok(()) => {
            if is_chunk {
                pending_chunks.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
            Ok(())
        }
        Err(e) => {
            if is_chunk {
                log::error!("OpenAI Realtime: failed to send audio: {e}");
            } else {
                log::error!("OpenAI Realtime: failed to send commit: {e}");
            }
            // Preserve the unsent command for replay on the next socket.
            Err(cmd)
        }
    }
}

/// Pumps audio out and transcripts back for a single WebSocket instance.
///
/// Owns the per-session transcript accumulator (delta text keyed by `item_id`),
/// which is intentionally local: after a reconnect a fresh `run_io` starts with
/// an empty accumulator because realtime sessions don't resume and `item_id`s
/// are a new namespace.
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
        pending_chunks,
        ready_event,
        pending_cmd,
    } = ctx;

    let mut accumulator: HashMap<String, String> = HashMap::new();
    // Tracks whether this socket's `session.updated` has been seen so the
    // readiness event (`Connected`/`Reconnected`) and the `connected` flag are
    // raised exactly once — and only after the server confirms the config.
    let mut session_confirmed = false;

    // Replay any command whose write failed on the previous socket *before*
    // pulling new work, preserving ordering. If the replay also fails the
    // command is put back into `pending_cmd` for the next reconnect.
    if let Some(cmd) = pending_cmd.take()
        && let Err(unsent) = write_audio_cmd(writer, pending_chunks, cmd).await
    {
        *pending_cmd = Some(unsent);
        return DisconnectKind::NetworkError("replay of in-flight command failed".into());
    }

    loop {
        tokio::select! {
            cmd = audio_rx.recv() => {
                match cmd {
                    Some(cmd @ (AudioCmd::Chunk(_) | AudioCmd::Commit)) => {
                        if let Err(unsent) = write_audio_cmd(writer, pending_chunks, cmd).await {
                            // Hold the unsent command so the reconnected socket
                            // replays it instead of silently dropping audio or
                            // the utterance commit.
                            *pending_cmd = Some(unsent);
                            return DisconnectKind::NetworkError("send failed".into());
                        }
                    }
                    Some(AudioCmd::Stop) => {
                        // Graceful user-initiated close: commit any buffered
                        // audio so the trailing utterance still transcribes,
                        // then close.
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
                        if handle_server_message(&text, event_tx, &mut accumulator)
                            && !session_confirmed
                        {
                            // The server has applied our `session.update`: the
                            // session is now configured per the `Connected`
                            // contract, so raise readiness exactly once.
                            session_confirmed = true;
                            connected.store(true, Ordering::SeqCst);
                            let _ = event_tx.send(ready_event.clone());
                        }
                    }
                    Ok(Message::Close(frame)) => {
                        log::info!("OpenAI Realtime: server closed connection: {frame:?}");
                        if user_disconnected.load(Ordering::SeqCst) {
                            return DisconnectKind::UserRequested;
                        }
                        let reason = frame
                            .map(|f| format!("{} {}", f.code, f.reason))
                            .unwrap_or_else(|| "no frame".into());
                        return DisconnectKind::ServerClose(reason);
                    }
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {
                        // Protocol-level frames; nothing to do.
                    }
                    Ok(Message::Binary(_)) => {
                        log::debug!("OpenAI Realtime: unexpected binary message");
                    }
                    Err(tungstenite::Error::ConnectionClosed)
                    | Err(tungstenite::Error::AlreadyClosed) => {
                        return DisconnectKind::NetworkError("connection closed".into());
                    }
                    Err(tungstenite::Error::Protocol(e)) => {
                        return DisconnectKind::ProtocolError(e.to_string());
                    }
                    Err(e) => {
                        log::error!("OpenAI Realtime: WebSocket read error: {e}");
                        return DisconnectKind::NetworkError(format!("{e}"));
                    }
                }
            }
        }
    }
}

/// Parse a single OpenAI Realtime server JSON message and emit appropriate
/// events.
///
/// Correlates transcription events by `item_id`:
/// - `...transcription.delta` accumulates `delta` text per `item_id` and emits
///   a non-final `Transcript` carrying the accumulated text so far.
/// - `...transcription.completed` replaces the accumulated text with the
///   provider's full `transcript`, emits a final `Transcript`, and clears the
///   accumulator entry.
/// - `...transcription.failed` and the top-level `error` frame emit `Error`.
///
/// `accumulator` holds in-progress delta text keyed by `item_id`. Cross-turn
/// `completed` ordering is not guaranteed, so each `item_id` is reconciled
/// independently.
///
/// Returns `true` iff the message is a session-readiness frame
/// (`session.updated` / `session.created`), signalling the caller that the
/// transcription session is now configured and it may emit
/// `Connected`/`Reconnected`. The readiness *event* is emitted by the caller
/// (so the once-only gating lives next to the `connected` flag), keeping this
/// parser free of cross-message state.
fn handle_server_message(
    text: &str,
    tx: &crossbeam_channel::Sender<OpenAiRealtimeEvent>,
    accumulator: &mut HashMap<String, String>,
) -> bool {
    let parsed: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("OpenAI Realtime: invalid JSON: {e}");
            let _ = tx.send(OpenAiRealtimeEvent::Error {
                message: format!("Invalid server JSON: {e}"),
            });
            return false;
        }
    };

    let msg_type = parsed.get("type").and_then(|t| t.as_str()).unwrap_or("");

    match msg_type {
        "conversation.item.input_audio_transcription.delta" => {
            let item_id = parsed
                .get("item_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let delta = parsed.get("delta").and_then(|v| v.as_str()).unwrap_or("");
            if item_id.is_empty() || delta.is_empty() {
                return false;
            }
            let acc = accumulator.entry(item_id.clone()).or_default();
            acc.push_str(delta);
            let _ = tx.send(OpenAiRealtimeEvent::Transcript {
                text: acc.clone(),
                item_id,
                is_final: false,
            });
            false
        }
        "conversation.item.input_audio_transcription.completed" => {
            let item_id = parsed
                .get("item_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            // Prefer the authoritative full transcript; fall back to whatever
            // we accumulated if the field is absent.
            let transcript = parsed
                .get("transcript")
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .or_else(|| accumulator.get(&item_id).cloned())
                .unwrap_or_default();
            accumulator.remove(&item_id);
            if transcript.is_empty() {
                return false;
            }
            let _ = tx.send(OpenAiRealtimeEvent::Transcript {
                text: transcript,
                item_id,
                is_final: true,
            });
            false
        }
        "conversation.item.input_audio_transcription.failed" => {
            let item_id = parsed.get("item_id").and_then(|v| v.as_str()).unwrap_or("");
            if !item_id.is_empty() {
                accumulator.remove(item_id);
            }
            let message = error_message(parsed.get("error"))
                .unwrap_or_else(|| "transcription failed".to_string());
            let _ = tx.send(OpenAiRealtimeEvent::Error {
                message: format!("transcription failed (item {item_id}): {message}"),
            });
            false
        }
        "error" => {
            let message =
                error_message(parsed.get("error")).unwrap_or_else(|| "unknown error".to_string());
            let _ = tx.send(OpenAiRealtimeEvent::Error { message });
            false
        }
        "session.updated" | "session.created" => {
            log::debug!("OpenAI Realtime: {msg_type} (session configured)");
            true
        }
        other => {
            // Many informational events (speech_started/stopped, item.added,
            // rate_limits.updated, etc.) are expected and not actionable on the
            // STT-only path.
            log::debug!("OpenAI Realtime: unhandled message type '{other}'");
            false
        }
    }
}

/// Extract a human-readable message from an OpenAI `error` object
/// (`{type,code,message,param}`), preferring `message`.
fn error_message(error: Option<&Value>) -> Option<String> {
    let error = error?;
    if let Some(msg) = error.get("message").and_then(|v| v.as_str()) {
        return Some(msg.to_string());
    }
    if let Some(code) = error.get("code").and_then(|v| v.as_str()) {
        return Some(code.to_string());
    }
    None
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Sample rate of the audio handed to [`OpenAiRealtimeClient::send_audio`] —
/// the speech pipeline's mixed mono output (see `audio::pipeline`).
const PIPELINE_SAMPLE_RATE: u32 = 16_000;

/// Convert f32 PCM samples (range -1.0 ... +1.0) to little-endian i16 bytes.
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

/// Linear-interpolation resampler from `from_rate` to `to_rate` (mono f32).
///
/// OpenAI Realtime accepts only 24 kHz, while the pipeline tap is 16 kHz, so we
/// upsample each chunk. A linear resampler is intentionally chosen over the
/// pipeline's heavier rubato sinc resampler: the audio is already
/// ASR-conditioned 16 kHz mono, the cloud model is robust to the mild aliasing
/// from linear interpolation, and a stateless per-chunk transform keeps the
/// hot path simple and unit-testable. Returns the input unchanged when the
/// rates are equal or either rate is zero.
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
    // Map each output index back to a fractional source position and linearly
    // interpolate between the two neighbouring input samples.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> OpenAiRealtimeConfig {
        OpenAiRealtimeConfig {
            api_key: "sk-test".into(),
            model: DEFAULT_MODEL.into(),
            language: Some("en".into()),
            sample_rate: REALTIME_SAMPLE_RATE,
        }
    }

    #[test]
    fn defaults_match_ga_protocol() {
        let cfg = OpenAiRealtimeConfig::default();
        assert_eq!(cfg.model, "gpt-realtime-whisper");
        assert_eq!(cfg.sample_rate, 24_000);
        assert!(cfg.language.is_none());
    }

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
        let result = client.send_audio(&[0.5, -0.3]);
        assert!(result.is_err());
    }

    #[test]
    fn commit_fails_when_channel_uninitialized() {
        let client = OpenAiRealtimeClient::new(test_config());
        // Not connected -> no audio channel.
        assert!(client.commit().is_err());
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
    fn resample_identity_when_rates_match() {
        let samples = [0.1, 0.2, -0.3, 0.4];
        let out = resample_linear(&samples, 24_000, 24_000);
        assert_eq!(out, samples);
    }

    #[test]
    fn resample_16k_to_24k_lengthens_by_ratio() {
        // 16 kHz -> 24 kHz is a 1.5x upsample.
        let samples = vec![0.0f32; 160]; // 10 ms @ 16 kHz
        let out = resample_linear(&samples, 16_000, 24_000);
        assert_eq!(out.len(), 240); // 10 ms @ 24 kHz
    }

    #[test]
    fn resample_interpolates_between_samples() {
        // Two samples upsampled 2x: midpoint should be the average.
        let samples = [0.0f32, 1.0];
        let out = resample_linear(&samples, 1, 2);
        assert_eq!(out.len(), 4);
        assert!((out[0] - 0.0).abs() < 1e-6);
        assert!((out[1] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn resample_empty_is_empty() {
        assert!(resample_linear(&[], 16_000, 24_000).is_empty());
    }

    #[test]
    fn session_update_payload_matches_research_verbatim() {
        // Verbatim shape from research §4.1.
        let cfg = test_config();
        let payload = session_update_payload(&cfg);
        let session = &payload["session"];
        assert_eq!(payload["type"], "session.update");
        assert_eq!(session["type"], "transcription");
        // Format MUST be the object form, not a string.
        assert!(session["audio"]["input"]["format"].is_object());
        assert_eq!(session["audio"]["input"]["format"]["type"], "audio/pcm");
        assert_eq!(session["audio"]["input"]["format"]["rate"], 24_000);
        assert_eq!(
            session["audio"]["input"]["transcription"]["model"],
            "gpt-realtime-whisper"
        );
        assert_eq!(session["audio"]["input"]["transcription"]["language"], "en");
        // No turn_detection for whisper (manual commit).
        assert!(session["audio"]["input"].get("turn_detection").is_none());
    }

    #[test]
    fn session_update_omits_language_when_none() {
        let mut cfg = test_config();
        cfg.language = None;
        let payload = session_update_payload(&cfg);
        assert!(
            payload["session"]["audio"]["input"]["transcription"]
                .get("language")
                .is_none()
        );
    }

    #[test]
    fn realtime_url_carries_model() {
        let url = realtime_url("gpt-realtime-whisper");
        assert_eq!(
            url,
            "wss://api.openai.com/v1/realtime?model=gpt-realtime-whisper"
        );
    }

    #[test]
    fn handle_delta_accumulates_per_item() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();

        // Verbatim from research §4 (delta).
        handle_server_message(
            r#"{ "type": "conversation.item.input_audio_transcription.delta",
                 "item_id": "item_003", "content_index": 0, "delta": "Hello," }"#,
            &tx,
            &mut acc,
        );
        handle_server_message(
            r#"{ "type": "conversation.item.input_audio_transcription.delta",
                 "item_id": "item_003", "content_index": 0, "delta": " how" }"#,
            &tx,
            &mut acc,
        );

        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::Transcript {
                text,
                item_id,
                is_final,
            } => {
                assert_eq!(text, "Hello,");
                assert_eq!(item_id, "item_003");
                assert!(!is_final);
            }
            other => panic!("Expected interim Transcript, got {other:?}"),
        }
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::Transcript { text, is_final, .. } => {
                assert_eq!(text, "Hello, how");
                assert!(!is_final);
            }
            other => panic!("Expected accumulated interim Transcript, got {other:?}"),
        }
        assert_eq!(acc.get("item_003").map(String::as_str), Some("Hello, how"));
    }

    #[test]
    fn handle_completed_emits_final_and_clears_accumulator() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();

        handle_server_message(
            r#"{ "type": "conversation.item.input_audio_transcription.delta",
                 "item_id": "item_003", "content_index": 0, "delta": "Hello," }"#,
            &tx,
            &mut acc,
        );
        let _interim = rx.try_recv().unwrap();

        // Verbatim from research §4 (completed) — full transcript replaces deltas.
        handle_server_message(
            r#"{ "type": "conversation.item.input_audio_transcription.completed",
                 "item_id": "item_003", "content_index": 0,
                 "transcript": "Hello, how are you?" }"#,
            &tx,
            &mut acc,
        );

        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::Transcript {
                text,
                item_id,
                is_final,
            } => {
                assert_eq!(text, "Hello, how are you?");
                assert_eq!(item_id, "item_003");
                assert!(is_final);
            }
            other => panic!("Expected final Transcript, got {other:?}"),
        }
        assert!(
            !acc.contains_key("item_003"),
            "accumulator entry should be cleared on completed"
        );
    }

    #[test]
    fn completed_without_transcript_falls_back_to_accumulated() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();
        handle_server_message(
            r#"{ "type": "conversation.item.input_audio_transcription.delta",
                 "item_id": "x", "delta": "partial text" }"#,
            &tx,
            &mut acc,
        );
        let _interim = rx.try_recv().unwrap();
        handle_server_message(
            r#"{ "type": "conversation.item.input_audio_transcription.completed",
                 "item_id": "x" }"#,
            &tx,
            &mut acc,
        );
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::Transcript { text, is_final, .. } => {
                assert_eq!(text, "partial text");
                assert!(is_final);
            }
            other => panic!("Expected fallback final Transcript, got {other:?}"),
        }
    }

    #[test]
    fn out_of_order_items_are_reconciled_independently() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();
        // Two interleaved items; completion arrives out of start order.
        handle_server_message(
            r#"{ "type": "conversation.item.input_audio_transcription.delta",
                 "item_id": "a", "delta": "one" }"#,
            &tx,
            &mut acc,
        );
        handle_server_message(
            r#"{ "type": "conversation.item.input_audio_transcription.delta",
                 "item_id": "b", "delta": "two" }"#,
            &tx,
            &mut acc,
        );
        // Drain the two interim events.
        let _ = rx.try_recv().unwrap();
        let _ = rx.try_recv().unwrap();

        handle_server_message(
            r#"{ "type": "conversation.item.input_audio_transcription.completed",
                 "item_id": "b", "transcript": "two done" }"#,
            &tx,
            &mut acc,
        );
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::Transcript {
                text,
                item_id,
                is_final,
            } => {
                assert_eq!(item_id, "b");
                assert_eq!(text, "two done");
                assert!(is_final);
            }
            other => panic!("Expected item b final, got {other:?}"),
        }
        // Item a is untouched and still accumulating.
        assert_eq!(acc.get("a").map(String::as_str), Some("one"));
        assert!(!acc.contains_key("b"));
    }

    #[test]
    fn handle_failed_emits_error_and_clears_item() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();
        acc.insert("item_003".to_string(), "partial".to_string());
        // Verbatim failure shape from research §4.
        handle_server_message(
            r#"{ "type": "conversation.item.input_audio_transcription.failed",
                 "item_id": "item_003", "content_index": 0,
                 "error": { "type": "invalid_request_error", "code": "rate_limit_exceeded",
                            "message": "Rate limit reached", "param": null } }"#,
            &tx,
            &mut acc,
        );
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::Error { message } => {
                assert!(message.contains("Rate limit reached"));
                assert!(message.contains("item_003"));
            }
            other => panic!("Expected Error, got {other:?}"),
        }
        assert!(!acc.contains_key("item_003"));
    }

    #[test]
    fn handle_top_level_error_frame() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();
        handle_server_message(
            r#"{ "type": "error",
                 "error": { "type": "server_error", "code": "internal",
                            "message": "Something went wrong", "param": null,
                            "event_id": "evt_1" } }"#,
            &tx,
            &mut acc,
        );
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::Error { message } => {
                assert_eq!(message, "Something went wrong");
            }
            other => panic!("Expected Error, got {other:?}"),
        }
    }

    #[test]
    fn invalid_json_emits_error() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();
        handle_server_message("not json", &tx, &mut acc);
        match rx.try_recv().unwrap() {
            OpenAiRealtimeEvent::Error { message } => {
                assert!(message.contains("Invalid server JSON"));
            }
            other => panic!("Expected Error, got {other:?}"),
        }
    }

    #[test]
    fn informational_events_are_ignored() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();
        handle_server_message(r#"{"type":"session.updated","session":{}}"#, &tx, &mut acc);
        handle_server_message(
            r#"{"type":"input_audio_buffer.committed","item_id":"item_1"}"#,
            &tx,
            &mut acc,
        );
        handle_server_message(r#"{"type":"rate_limits.updated"}"#, &tx, &mut acc);
        assert!(
            rx.try_recv().is_err(),
            "informational events should not emit transcript/error events"
        );
    }

    #[test]
    fn empty_delta_not_emitted() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();
        handle_server_message(
            r#"{"type":"conversation.item.input_audio_transcription.delta",
                "item_id":"item_1","delta":""}"#,
            &tx,
            &mut acc,
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn backoff_schedule_matches_spec() {
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(2), Some(2));
        assert_eq!(backoff_for_attempt(3), Some(5));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), None);
        assert_eq!(backoff_for_attempt(99), None);
    }

    #[test]
    fn event_serialization_roundtrip() {
        let events = vec![
            OpenAiRealtimeEvent::Transcript {
                text: "hello".into(),
                item_id: "item_1".into(),
                is_final: true,
            },
            OpenAiRealtimeEvent::Error {
                message: "oops".into(),
            },
            OpenAiRealtimeEvent::Connected,
            OpenAiRealtimeEvent::Disconnected,
            OpenAiRealtimeEvent::Reconnecting {
                attempt: 2,
                backoff_secs: 2,
            },
            OpenAiRealtimeEvent::Reconnected,
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let parsed: Value = serde_json::from_str(&json).unwrap();
            assert!(parsed.get("type").is_some(), "tagged on type: {json}");
            // Round-trip back into the enum.
            let _back: OpenAiRealtimeEvent = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn transcript_event_tag_is_type() {
        let json = serde_json::to_value(OpenAiRealtimeEvent::Transcript {
            text: "hi".into(),
            item_id: "i".into(),
            is_final: false,
        })
        .unwrap();
        assert_eq!(json["type"], "transcript");
        assert_eq!(json["text"], "hi");
        assert_eq!(json["item_id"], "i");
        assert_eq!(json["is_final"], false);
    }

    // --- Finding openai_realtime.rs:265 — Connected only after session.updated ---

    #[test]
    fn session_updated_signals_readiness() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();
        let confirmed =
            handle_server_message(r#"{"type":"session.updated","session":{}}"#, &tx, &mut acc);
        assert!(confirmed, "session.updated must signal session-confirmed");
        // The parser itself emits no event — the readiness event is the caller's
        // responsibility (so the once-only gating lives next to `connected`).
        assert!(
            rx.try_recv().is_err(),
            "parser must not emit a readiness event itself"
        );
    }

    #[test]
    fn session_created_signals_readiness() {
        let (tx, _rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();
        let confirmed =
            handle_server_message(r#"{"type":"session.created","session":{}}"#, &tx, &mut acc);
        assert!(confirmed, "session.created must signal session-confirmed");
    }

    #[test]
    fn non_session_messages_do_not_signal_readiness() {
        let (tx, _rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();
        // Transcript delta, error, informational, and unknown frames must all
        // report `false` so the caller never raises Connected prematurely.
        assert!(!handle_server_message(
            r#"{"type":"conversation.item.input_audio_transcription.delta",
                "item_id":"i","delta":"hi"}"#,
            &tx,
            &mut acc,
        ));
        assert!(!handle_server_message(
            r#"{"type":"error","error":{"message":"boom"}}"#,
            &tx,
            &mut acc,
        ));
        assert!(!handle_server_message(
            r#"{"type":"rate_limits.updated"}"#,
            &tx,
            &mut acc,
        ));
        assert!(!handle_server_message("not json", &tx, &mut acc));
    }

    /// Reproduces the `run_io` readiness gating in isolation: the readiness
    /// event (`Connected`/`Reconnected`) is emitted exactly once, only after a
    /// `session.updated`, and the `connected` flag flips at the same moment.
    #[test]
    fn readiness_event_emitted_once_after_session_updated() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut acc = HashMap::new();
        let connected = Arc::new(AtomicBool::new(false));
        let ready_event = OpenAiRealtimeEvent::Connected;
        let mut session_confirmed = false;

        // Mirror the run_io reader-arm gating logic.
        let mut feed = |text: &str| {
            if handle_server_message(text, &tx, &mut acc) && !session_confirmed {
                session_confirmed = true;
                connected.store(true, Ordering::SeqCst);
                let _ = tx.send(ready_event.clone());
            }
        };

        // A transcript before session.updated must NOT mark connected.
        feed(
            r#"{"type":"conversation.item.input_audio_transcription.delta",
                 "item_id":"i","delta":"hi"}"#,
        );
        assert!(
            !connected.load(Ordering::SeqCst),
            "must not be connected before session.updated"
        );
        let _ = rx.try_recv().expect("interim transcript"); // drain the delta

        // First session.updated -> Connected emitted, connected flips true.
        feed(r#"{"type":"session.updated","session":{}}"#);
        assert!(connected.load(Ordering::SeqCst));
        assert!(matches!(
            rx.try_recv().unwrap(),
            OpenAiRealtimeEvent::Connected
        ));

        // A second session.updated must NOT emit a duplicate Connected.
        feed(r#"{"type":"session.updated","session":{}}"#);
        assert!(
            rx.try_recv().is_err(),
            "Connected must be emitted at most once per session"
        );
    }

    // --- Finding openai_realtime.rs:403 — Disconnected emitted at most once ---

    #[test]
    fn emit_disconnected_once_dedupes() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let guard = Arc::new(AtomicBool::new(false));

        // First caller wins and emits.
        assert!(emit_disconnected_once(&tx, &guard));
        // All subsequent callers (disconnect() + session task) are no-ops.
        assert!(!emit_disconnected_once(&tx, &guard));
        assert!(!emit_disconnected_once(&tx, &guard));

        assert!(matches!(
            rx.try_recv().unwrap(),
            OpenAiRealtimeEvent::Disconnected
        ));
        assert!(
            rx.try_recv().is_err(),
            "exactly one Disconnected event must be sent"
        );
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

        // connect() re-arms the guard for a fresh session.
        guard.store(false, Ordering::SeqCst);
        assert!(emit_disconnected_once(&tx, &guard));
        assert!(matches!(
            rx.try_recv().unwrap(),
            OpenAiRealtimeEvent::Disconnected
        ));
    }

    // --- Finding openai_realtime.rs:713 — in-flight command preserved ---

    /// Open a real in-process WebSocket and hand back the split writer (the
    /// exact `WsWriter` type `write_audio_cmd` takes) plus a handle to close the
    /// server side so writes fail deterministically.
    async fn connect_local_ws() -> (WsWriter, tokio::task::JoinHandle<()>) {
        use tokio::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Server: accept one upgrade, then immediately close the connection so
        // the client's subsequent writes error out.
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            // Close the server end right away.
            let (mut w, _r) = ws.split();
            let _ = w.close().await;
        });

        let url = format!("ws://{addr}/");
        let (ws_stream, _resp) = connect_async(url).await.unwrap();
        let (writer, _reader) = ws_stream.split();
        (writer, server)
    }

    /// A successful write consumes the command and decrements the chunk backlog
    /// counter so `send_audio`'s cap stays accurate.
    #[tokio::test]
    async fn write_audio_cmd_decrements_chunk_on_success() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Server stays open and drains frames so the write succeeds.
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let (_w, mut r) = ws.split();
            // Read until the client closes.
            while let Some(Ok(msg)) = r.next().await {
                if msg.is_close() {
                    break;
                }
            }
        });
        let (ws_stream, _resp) = connect_async(format!("ws://{addr}/")).await.unwrap();
        let (mut writer, _reader) = ws_stream.split();

        let pending = Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let res = write_audio_cmd(&mut writer, &pending, AudioCmd::Chunk("YWJj".into())).await;
        assert!(res.is_ok(), "write to an open socket must succeed");
        assert_eq!(
            pending.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "a successfully sent chunk must leave the backlog counter"
        );
        let _ = writer.close().await;
        let _ = server.await;
    }

    /// On a write failure the command is returned intact (`Err(cmd)`) so the
    /// session task can replay it on the reconnected socket — and the chunk
    /// counter is *not* decremented, so the held chunk still counts against the
    /// backlog cap. This is the core of the in-flight-loss finding.
    #[tokio::test]
    async fn write_audio_cmd_preserves_command_on_failure() {
        use std::sync::atomic::Ordering::Relaxed;
        let (mut writer, server) = connect_local_ws().await;
        // Ensure the server has closed before we attempt to write.
        let _ = server.await;
        // Seed the backlog high enough that no early *successful* write (each of
        // which legitimately decrements) underflows it before the failure path
        // is exercised. The first write may buffer locally; flush-on-next forces
        // the error, so loop until the closed-socket error is observed.
        let pending = Arc::new(std::sync::atomic::AtomicUsize::new(100));
        let mut last = Ok(());
        let mut count_before_fail = 0usize;
        for _ in 0..50 {
            // Snapshot the counter immediately *before* each call so we can
            // assert the failing call specifically leaves it untouched.
            count_before_fail = pending.load(Relaxed);
            last = write_audio_cmd(&mut writer, &pending, AudioCmd::Chunk("YWJj".into())).await;
            if last.is_err() {
                break;
            }
        }
        let returned = last.expect_err("write to a closed socket must eventually fail");
        // The exact command is handed back for replay.
        match returned {
            AudioCmd::Chunk(b64) => assert_eq!(b64, "YWJj"),
            other => panic!("expected the chunk to be preserved, got {other:?}"),
        }
        // The *failing* call must not have decremented the counter: a held chunk
        // still counts toward the backlog cap and is never under-counted.
        assert_eq!(
            pending.load(Relaxed),
            count_before_fail,
            "a failed chunk write must not decrement the backlog counter"
        );
    }

    /// A failed `Commit` is likewise preserved — losing an utterance commit is
    /// the worst case the finding calls out (the trailing turn never finalizes).
    #[tokio::test]
    async fn write_audio_cmd_preserves_commit_on_failure() {
        let (mut writer, server) = connect_local_ws().await;
        let _ = server.await;
        let pending = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut last = Ok(());
        for _ in 0..50 {
            last = write_audio_cmd(&mut writer, &pending, AudioCmd::Commit).await;
            if last.is_err() {
                break;
            }
        }
        let returned = last.expect_err("commit to a closed socket must eventually fail");
        assert!(
            matches!(returned, AudioCmd::Commit),
            "the utterance commit must be preserved for replay"
        );
    }
}
