//! Deepgram Streaming ASR WebSocket client.
//!
//! Connects to the Deepgram real-time transcription API via WebSocket and
//! streams audio for low-latency speech-to-text with optional speaker
//! diarization.
//!
//! # Protocol overview
//!
//! 1. Open WSS connection to `wss://api.deepgram.com/v1/listen` with query
//!    parameters for encoding, sample rate, model, etc.
//! 2. Authenticate via `Authorization: Token {api_key}` header on upgrade.
//! 3. Stream binary frames of i16 LE PCM audio data.
//! 4. Receive JSON messages with transcript results (interim and final).
//! 5. Send text-frame `{"type":"KeepAlive"}` messages during idle periods.
//! 6. Send an empty binary frame `[]` to signal end of audio, then close.
//!
//! # Threading model
//!
//! The public API is **synchronous** (called from `std::thread` workers in
//! the speech processor). Internally, a dedicated tokio runtime drives the
//! WebSocket. Audio is forwarded from the caller's thread to the async writer
//! via an unbounded `tokio::sync::mpsc` channel, and events flow back through
//! a `crossbeam_channel` that the speech processor consumes.

#[cfg(test)]
use super::reconnect::backoff_for_attempt;
use super::reconnect::{ReconnectStep, next_reconnect_step};
use super::transport::{AsrTransportPayloadKind, AsrWsReader, AsrWsWriteGuard, AsrWsWriter};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

/// Events emitted by the Deepgram streaming client to downstream consumers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum DeepgramEvent {
    /// A transcript result from Deepgram.
    #[serde(rename = "transcript")]
    Transcript {
        text: String,
        confidence: f32,
        is_final: bool,
        speech_final: bool,
        start: f64,
        duration: f64,
        words: Vec<DeepgramWord>,
    },
    /// A non-fatal error occurred.
    #[serde(rename = "error")]
    Error { message: String },
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
    /// The client successfully re-established the WebSocket after a disconnect.
    #[serde(rename = "reconnected")]
    Reconnected,
    /// A provider-native turn lifecycle signal from Nova endpointing/VAD or
    /// Flux conversational turn detection.
    #[serde(rename = "turn")]
    Turn {
        kind: DeepgramTurnKind,
        text: Option<String>,
        start: Option<f64>,
        end: Option<f64>,
        confidence: Option<f32>,
        turn_index: Option<u64>,
    },
}

/// Deepgram-specific turn signals before they are normalized by the speech
/// processor into the app-wide `turn-event` IPC payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeepgramTurnKind {
    SpeechStarted,
    SpeechFinal,
    UtteranceEnd,
    StartOfTurn,
    EagerEndOfTurn,
    EndOfTurn,
    TurnResumed,
}

/// A single word from Deepgram's response, with timing and optional speaker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeepgramWord {
    pub word: String,
    pub start: f64,
    pub end: f64,
    pub confidence: f32,
    pub speaker: Option<u32>,
}

/// Configuration for a Deepgram streaming session.
#[derive(Clone)]
pub struct DeepgramConfig {
    /// Deepgram API key.
    pub api_key: String,
    /// Model name (e.g. `"nova-3"`).
    pub model: String,
    /// Whether to enable speaker diarization.
    pub enable_diarization: bool,
    /// Nova endpointing silence threshold in milliseconds. `None` leaves
    /// Deepgram's default behavior in place.
    pub endpointing_ms: Option<u32>,
    /// Nova UtteranceEnd gap threshold in milliseconds.
    pub utterance_end_ms: Option<u32>,
    /// Whether to request Deepgram VAD events such as `SpeechStarted`.
    pub vad_events: bool,
    /// Flux `eot_threshold` for reliable `EndOfTurn` events.
    pub eot_threshold: Option<f32>,
    /// Flux `eager_eot_threshold`; enables speculative `EagerEndOfTurn` and
    /// cancellation via `TurnResumed`.
    pub eager_eot_threshold: Option<f32>,
    /// Flux maximum silence before forcing `EndOfTurn`.
    pub eot_timeout_ms: Option<u32>,
    /// Runtime privacy guard for session audio egress.
    pub content_egress_policy: crate::asr::ProviderContentEgressPolicy,
}

impl std::fmt::Debug for DeepgramConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepgramConfig")
            .field(
                "api_key",
                &crate::credentials::redacted_secret_presence(Some(&self.api_key)),
            )
            .field("model", &self.model)
            .field("enable_diarization", &self.enable_diarization)
            .field("endpointing_ms", &self.endpointing_ms)
            .field("utterance_end_ms", &self.utterance_end_ms)
            .field("vad_events", &self.vad_events)
            .field("eot_threshold", &self.eot_threshold)
            .field("eager_eot_threshold", &self.eager_eot_threshold)
            .field("eot_timeout_ms", &self.eot_timeout_ms)
            .field("content_egress_policy", &self.content_egress_policy)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Internal message passed from sync send_audio() -> async writer task
// ---------------------------------------------------------------------------

/// Hard cap on the audio-chunk backlog (see `pending_chunks`). At roughly one
/// chunk per 50ms from the speech processor this corresponds to ~10s of
/// audio — well beyond any healthy reconnect window, so exceeding it signals
/// either a bug or a network catastrophe. New chunks are dropped after this
/// point and `user_disconnected` is flipped so the caller sees a clean error.
const AUDIO_BUFFER_MAX_CHUNKS: usize = 200;
/// Deepgram closes listen sockets after roughly 10 seconds without audio or a
/// KeepAlive message. Send KeepAlive conservatively before that window.
const KEEPALIVE_INTERVAL_SECS: u64 = 4;
const KEEPALIVE_PAYLOAD: &str = r#"{"type":"KeepAlive"}"#;

enum AudioCmd {
    /// Raw i16 LE PCM bytes ready to send as a binary frame.
    Chunk(Vec<u8>),
    /// Signal end of audio stream and close.
    Stop,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// A Deepgram real-time streaming ASR client.
///
/// The public methods (`connect`, `send_audio`, `disconnect`, `event_rx`) are
/// all **synchronous** -- they block the caller's thread just long enough to
/// hand off work to the internal async runtime. This matches the threading
/// model used by the speech processor where worker threads run in `std::thread`.
pub struct DeepgramStreamingClient {
    config: DeepgramConfig,
    /// crossbeam event channel -- writer side (background reader task pushes here).
    event_tx: crossbeam_channel::Sender<DeepgramEvent>,
    /// crossbeam event channel -- reader side (speech processor consumes this).
    event_rx: crossbeam_channel::Receiver<DeepgramEvent>,
    /// Whether the WebSocket is connected.
    connected: Arc<AtomicBool>,
    /// Set to `true` when the user has explicitly called `disconnect()`.
    ///
    /// Used by the reader loop to distinguish a user-initiated teardown
    /// (do not auto-reconnect) from a network error or server close
    /// (auto-reconnect with exponential backoff).
    user_disconnected: Arc<AtomicBool>,
    /// One-shot guard ensuring `Disconnected` is emitted at most once per
    /// session teardown across `disconnect()` and the session task.
    disconnected_emitted: Arc<AtomicBool>,
    /// Tokio runtime that owns the WebSocket tasks.
    rt: Option<tokio::runtime::Runtime>,
    /// Sender for audio commands -> async writer task.
    audio_tx: Option<tokio_mpsc::UnboundedSender<AudioCmd>>,
    /// Approximate count of audio chunks buffered in `audio_tx` awaiting
    /// transmission. Incremented by `send_audio`, decremented by the writer
    /// task. Used to bound memory during a prolonged reconnect cycle — we
    /// refuse to enqueue new chunks once the buffer exceeds
    /// [`AUDIO_BUFFER_MAX_CHUNKS`], which corresponds to roughly 10s of audio
    /// at the ~50ms chunk granularity the speech processor emits.
    pending_chunks: Arc<std::sync::atomic::AtomicUsize>,
    /// Handle to the reader task (for join on shutdown).
    _reader_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle to the writer task (for join on shutdown).
    _writer_handle: Option<tokio::task::JoinHandle<()>>,
}

impl DeepgramStreamingClient {
    /// Create a new (disconnected) Deepgram streaming client with the given config.
    pub fn new(config: DeepgramConfig) -> Self {
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
            _reader_handle: None,
            _writer_handle: None,
        }
    }

    // ------------------------------------------------------------------
    // Connect
    // ------------------------------------------------------------------

    /// Connect to the Deepgram real-time transcription API.
    ///
    /// Blocks the caller until the WebSocket is open, then spawns a background
    /// session task on an internal tokio runtime. The session task handles
    /// audio writing, server message reading, and automatic reconnection with
    /// exponential backoff if the WebSocket drops mid-session.
    pub fn connect(&mut self) -> Result<(), String> {
        if self.config.api_key.is_empty() {
            return Err("Deepgram API key is not configured".to_string());
        }

        // Build a dedicated single-threaded tokio runtime for the WebSocket.
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("deepgram-ws-rt")
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
        disconnected_emitted.store(false, Ordering::SeqCst);
        // Reset any stale count from a prior session.
        self.pending_chunks
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let pending_chunks = Arc::clone(&self.pending_chunks);

        // Perform the blocking initial connect inside the runtime.
        let (audio_tx, session_handle) = rt.block_on(async move {
            // Initial connect — surfaced synchronously so the caller sees
            // auth / network errors immediately instead of through the
            // reconnect loop.
            let (writer, reader) = open_ws(&config).await?;

            log::info!("Deepgram: WebSocket connected");
            connected.store(true, Ordering::SeqCst);
            let _ = event_tx.send(DeepgramEvent::Connected);

            // Build the audio command channel the caller will push into.
            let (atx, arx) = tokio_mpsc::unbounded_channel::<AudioCmd>();

            // Spawn the session task, which owns both halves of the socket
            // and handles reconnects internally.
            let session_handle = tokio::spawn(session_task(DeepgramSessionCtx {
                writer,
                reader,
                audio_rx: arx,
                config,
                event_tx,
                connected,
                user_disconnected,
                disconnected_emitted,
                pending_chunks: Arc::clone(&pending_chunks),
                #[cfg(test)]
                reconnect_opener: None,
                #[cfg(test)]
                run_io_entries: None,
            }));

            Ok::<_, String>((atx, session_handle))
        })?;

        self.audio_tx = Some(audio_tx);
        self._reader_handle = Some(session_handle);
        self._writer_handle = None;
        self.rt = Some(rt);

        Ok(())
    }

    // ------------------------------------------------------------------
    // Send audio
    // ------------------------------------------------------------------

    /// Send PCM audio data to Deepgram for processing.
    ///
    /// The audio should be **f32 mono 16 kHz** (matching the pipeline output).
    /// The method converts to 16-bit LE PCM and queues for async sending.
    /// Returns immediately (non-blocking).
    ///
    /// # Behaviour during auto-reconnect
    ///
    /// This method *does not* check the `connected` flag — only
    /// `user_disconnected`. That way, if the session task is in the middle of
    /// a reconnect cycle, audio is still queued to the unbounded channel and
    /// will be flushed to Deepgram as soon as the new socket is open. The
    /// caller never sees a spurious "Not connected" error for a transient
    /// network hiccup.
    pub fn send_audio(&self, audio: &[f32]) -> Result<(), String> {
        if self.user_disconnected.load(Ordering::SeqCst) {
            return Err("Deepgram client has been disconnected".to_string());
        }

        if audio.is_empty() {
            return Ok(());
        }

        self.config
            .content_egress_policy
            .check_audio("asr.deepgram")?;

        let tx = self
            .audio_tx
            .as_ref()
            .ok_or_else(|| "Audio channel not initialized".to_string())?;

        // Drop chunks if the buffer has grown past the safety cap. This
        // protects against runaway memory usage when the WebSocket is stuck
        // in a long reconnect cycle (e.g. captive portal, network partition).
        // Flipping `user_disconnected` is deliberate: once we start losing
        // data the caller deserves to know the session is effectively dead
        // rather than silently seeing gaps in the transcript.
        let depth = self
            .pending_chunks
            .load(std::sync::atomic::Ordering::Relaxed);
        if depth >= AUDIO_BUFFER_MAX_CHUNKS {
            self.user_disconnected
                .store(true, std::sync::atomic::Ordering::SeqCst);
            return Err(format!(
                "Deepgram audio buffer full ({depth} chunks) — likely a stuck reconnect. Restart the session."
            ));
        }

        // f32 -> i16 LE PCM bytes
        let pcm_bytes = f32_to_i16_le_bytes(audio);

        self.pending_chunks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tx.send(AudioCmd::Chunk(pcm_bytes)).map_err(|_| {
            // Restore the counter on send failure so a permanently closed
            // channel doesn't permanently skew the cap.
            self.pending_chunks
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            "Audio channel closed".to_string()
        })
    }

    // ------------------------------------------------------------------
    // Event receiver
    // ------------------------------------------------------------------

    /// Get a clone of the event receiver channel.
    ///
    /// The speech processor uses this to read `DeepgramEvent`s.
    pub fn event_rx(&self) -> crossbeam_channel::Receiver<DeepgramEvent> {
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

    /// Disconnect from Deepgram and clean up resources.
    ///
    /// Sends a close frame, waits for background tasks to finish, and shuts
    /// down the internal tokio runtime. Setting `user_disconnected` prevents
    /// the session task from attempting to auto-reconnect.
    pub fn disconnect(&self) {
        log::info!("DeepgramStreamingClient: disconnecting (user-initiated)");

        // Mark this teardown as user-initiated so the session task does not
        // try to reconnect after the close frame is observed.
        self.user_disconnected.store(true, Ordering::SeqCst);

        // Signal not connected first (stops send_audio calls).
        self.connected.store(false, Ordering::SeqCst);

        // Tell the writer task to close.
        if let Some(ref tx) = self.audio_tx {
            let _ = tx.send(AudioCmd::Stop);
        }

        emit_disconnected_once(&self.event_tx, &self.disconnected_emitted);
    }
}

impl Drop for DeepgramStreamingClient {
    fn drop(&mut self) {
        // Mark teardown as user-initiated so the session task exits cleanly
        // instead of trying to reconnect after we shut the runtime down.
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

        log::info!("DeepgramStreamingClient: dropped");
    }
}

// ===========================================================================
// Free functions -- async building blocks
// ===========================================================================

/// Classifies *why* the session dropped so downstream logs / events can be
/// precise without the caller re-parsing error strings.
///
/// The inner `String` on the network variants carries the human-readable
/// reason for logging and telemetry. It is consumed through `Debug`
/// formatting on `{kind:?}`, which the dead-code lint does not track, hence
/// the allow.
#[derive(Debug)]
#[allow(dead_code)]
enum DisconnectKind {
    /// Remote server sent a Close frame. Typically a graceful server-side
    /// teardown (e.g. `GoAway`, idle timeout).
    ServerClose(String),
    /// Transport-level error (TLS, TCP reset, DNS flap, tungstenite I/O).
    NetworkError(String),
    /// Protocol violation — malformed frame, invalid sequence, etc.
    ProtocolError(String),
    /// Content-bearing send was blocked by the runtime privacy policy.
    PolicyBlocked(String),
    /// User called `disconnect()`. No reconnect attempt should be made.
    UserRequested,
    /// Writer task exhausted the audio command stream (caller dropped the
    /// sender). No reconnect — session is genuinely over.
    WriterEnded,
}

/// Open a fresh Deepgram WebSocket using the live [`DeepgramConfig`].
///
/// Used both for the initial connect and for each reconnect attempt. The
/// query-string-only "handshake" means a reconnect is just re-running this
/// function — no replay of a setup frame is required.
async fn open_ws(config: &DeepgramConfig) -> Result<(AsrWsWriter, AsrWsReader), String> {
    let url_str = deepgram_listen_url(config);

    let request = tungstenite::http::Request::builder()
        .uri(&url_str)
        .header("Authorization", format!("Token {}", config.api_key))
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .header("Sec-WebSocket-Version", "13")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Host", "api.deepgram.com")
        .body(())
        .map_err(|e| format!("Failed to build WebSocket request: {e}"))?;

    let (ws_stream, _response) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| {
            crate::error::redacted_provider_diagnostic(
                &format!("WebSocket connect failed: {e}"),
                [&config.api_key],
            )
        })?;

    Ok(ws_stream.split())
}

#[cfg(test)]
type ReconnectOpenFuture =
    Pin<Box<dyn Future<Output = Result<(AsrWsWriter, AsrWsReader), String>> + Send>>;

#[cfg(test)]
type ReconnectOpener = Arc<dyn Fn(DeepgramConfig) -> ReconnectOpenFuture + Send + Sync>;

#[cfg(test)]
async fn open_reconnect_ws(
    config: &DeepgramConfig,
    opener: Option<&ReconnectOpener>,
) -> Result<(AsrWsWriter, AsrWsReader), String> {
    if let Some(opener) = opener {
        opener(config.clone()).await
    } else {
        open_ws(config).await
    }
}

fn deepgram_listen_url(config: &DeepgramConfig) -> String {
    let is_flux = config.model.starts_with("flux-");
    let mut url = if is_flux {
        format!(
            "wss://api.deepgram.com/v2/listen?encoding=linear16&sample_rate=16000&channels=1&model={}",
            config.model
        )
    } else {
        format!(
            "wss://api.deepgram.com/v1/listen?encoding=linear16&sample_rate=16000&channels=1&model={}&interim_results=true&diarize={}&punctuate=true",
            config.model, config.enable_diarization
        )
    };

    if is_flux {
        if let Some(threshold) = config.eot_threshold {
            url.push_str(&format!("&eot_threshold={threshold}"));
        }
        if let Some(threshold) = config.eager_eot_threshold {
            url.push_str(&format!("&eager_eot_threshold={threshold}"));
        }
        if let Some(ms) = config.eot_timeout_ms {
            url.push_str(&format!("&eot_timeout_ms={ms}"));
        }
    } else {
        if let Some(ms) = config.endpointing_ms {
            url.push_str(&format!("&endpointing={ms}"));
        }
        if let Some(ms) = config.utterance_end_ms {
            url.push_str(&format!("&utterance_end_ms={ms}"));
        }
        if config.vad_events {
            url.push_str("&vad_events=true");
        }
    }

    url
}

/// Bundles everything `session_task` owns for a single Deepgram session:
/// the split WebSocket halves, the audio command receiver, live config,
/// the outbound event channel, and the three shared atomics. Collapses an
/// 8-arg function signature to one — see `speech/context.rs` for the same
/// pattern applied to the speech workers.
struct DeepgramSessionCtx {
    writer: AsrWsWriter,
    reader: AsrWsReader,
    audio_rx: tokio_mpsc::UnboundedReceiver<AudioCmd>,
    config: DeepgramConfig,
    event_tx: crossbeam_channel::Sender<DeepgramEvent>,
    connected: Arc<AtomicBool>,
    user_disconnected: Arc<AtomicBool>,
    disconnected_emitted: Arc<AtomicBool>,
    pending_chunks: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    reconnect_opener: Option<ReconnectOpener>,
    #[cfg(test)]
    run_io_entries: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

fn emit_disconnected_once(
    event_tx: &crossbeam_channel::Sender<DeepgramEvent>,
    disconnected_emitted: &Arc<AtomicBool>,
) -> bool {
    if disconnected_emitted.swap(true, Ordering::SeqCst) {
        return false;
    }
    let _ = event_tx.send(DeepgramEvent::Disconnected);
    true
}

/// Background task owning a single Deepgram WebSocket session, including
/// reconnect logic.
///
/// Runs the reader and writer concurrently via `tokio::select!`. When either
/// half reports a disconnect (server Close frame, tungstenite error, etc.),
/// the task:
///
/// 1. Checks the `user_disconnected` flag — if set, exits silently.
/// 2. Emits `Disconnected` + a fresh `Reconnecting { attempt }` event.
/// 3. Sleeps for the exponential backoff period (1s/2s/5s/10s).
/// 4. Calls [`open_ws`] to re-establish the socket.
/// 5. On success, emits `Reconnected` and resumes the read/write loop. The
///    audio channel (`arx`) is preserved across reconnects so the caller's
///    in-flight audio is not lost — it just buffers until the writer side
///    comes back.
/// 6. On failure, loops back to step 2 with the incremented attempt count.
/// 7. After 4 failed attempts, emits a fatal `Error` event and exits.
async fn session_task(ctx: DeepgramSessionCtx) {
    let DeepgramSessionCtx {
        writer: initial_writer,
        reader: initial_reader,
        mut audio_rx,
        config,
        event_tx,
        connected,
        user_disconnected,
        disconnected_emitted,
        pending_chunks,
        #[cfg(test)]
        reconnect_opener,
        #[cfg(test)]
        run_io_entries,
    } = ctx;

    let mut writer = initial_writer;
    let mut reader = initial_reader;
    let mut reconnect_attempts: u32 = 0;
    let write_guard = AsrWsWriteGuard::new("asr.deepgram", config.content_egress_policy);

    loop {
        #[cfg(test)]
        if let Some(entries) = &run_io_entries {
            entries.fetch_add(1, Ordering::SeqCst);
        }

        // Drive reader + writer concurrently until one side signals we are
        // done. `run_io` is responsible for pumping audio out and transcripts
        // back until the socket breaks or the caller sends `AudioCmd::Stop`.
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

        // Any fresh disconnect resets to the "actively down" state so
        // `send_audio()` correctly starts rejecting while we recover.
        connected.store(false, Ordering::SeqCst);

        match disconnect {
            DisconnectKind::UserRequested | DisconnectKind::WriterEnded => {
                // Clean end — the user asked to stop, or we ran out of audio
                // commands because the client was dropped. Do not reconnect.
                log::info!("Deepgram session: ending ({disconnect:?})");
                emit_disconnected_once(&event_tx, &disconnected_emitted);
                break;
            }
            DisconnectKind::PolicyBlocked(message) => {
                log::warn!("Deepgram session: content egress blocked: {message}");
                let _ = event_tx.send(DeepgramEvent::Error { message });
                emit_disconnected_once(&event_tx, &disconnected_emitted);
                break;
            }
            _ => {
                // Network-ish failure. If the user *also* asked to disconnect
                // (e.g. they hit stop just as the socket was dying), honour
                // that and skip the reconnect dance.
                if user_disconnected.load(Ordering::SeqCst) {
                    emit_disconnected_once(&event_tx, &disconnected_emitted);
                    break;
                }

                log::warn!("Deepgram session: disconnected — {disconnect:?}");
                emit_disconnected_once(&event_tx, &disconnected_emitted);

                // Drive the reconnect ladder entirely inline. Each open_ws
                // failure advances to the *next* attempt right here (increment
                // + Reconnecting + backoff sleep) rather than looping back
                // through `run_io` with a dead socket — that path would have
                // immediately re-disconnected and double-counted the attempt,
                // double-firing Disconnected/Reconnecting and confusing the
                // UI attempt counter (FA-2).
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
                            // Budget exhausted — surface a fatal error and stop.
                            log::error!(
                                "Deepgram session: reconnect budget exhausted after {attempted} attempts"
                            );
                            let _ = event_tx.send(DeepgramEvent::Error {
                                message: "Deepgram reconnect attempts exhausted".into(),
                            });
                            break false;
                        }
                    };

                    log::info!(
                        "Deepgram session: reconnecting (attempt {attempt}, backoff {backoff}s)"
                    );
                    let _ = event_tx.send(DeepgramEvent::Reconnecting {
                        attempt,
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
                                    log::info!("Deepgram session: user cancelled during backoff");
                                    emit_disconnected_once(&event_tx, &disconnected_emitted);
                                    return;
                                }
                            }
                        }
                    }

                    // Attempt the reconnect. Deepgram has no setup handshake —
                    // the query parameters on the URL *are* the handshake — so
                    // `open_ws` is all we need.
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
                            disconnected_emitted.store(false, Ordering::SeqCst);
                            log::info!("Deepgram session: reconnected on attempt {attempt}");
                            let _ = event_tx.send(DeepgramEvent::Reconnected);
                            reconnect_attempts = 0;
                            break true;
                        }
                        Err(e) => {
                            // Redact: a reconnect error can embed the upgrade
                            // request (api_key header/query) or URL userinfo, so
                            // scrub the key before it reaches logs or the UI.
                            let diag = crate::error::redacted_provider_diagnostic(
                                &format!("Reconnect attempt {attempt} failed: {e}"),
                                [&config.api_key],
                            );
                            log::warn!("Deepgram session: {diag}");
                            let _ = event_tx.send(DeepgramEvent::Error { message: diag });
                            // Stay in this inner loop: the next iteration drives
                            // the following attempt inline (no run_io detour with
                            // a dead socket), preserving the backoff ladder.
                            continue;
                        }
                    }
                };

                if reconnected {
                    // Resume run_io with the fresh socket halves.
                    continue;
                }
                // Budget exhausted: stop the session task.
                break;
            }
        }
    }

    connected.store(false, Ordering::SeqCst);
    log::info!("Deepgram: session task exited");
}

/// Pumps audio out and transcripts back for a single WebSocket instance.
///
/// Returns the classified [`DisconnectKind`] when the socket breaks or the
/// caller asks to stop. The session task above turns that into either a
/// reconnect or a clean exit.
#[allow(clippy::too_many_arguments)]
async fn run_io(
    writer: &mut AsrWsWriter,
    reader: &mut AsrWsReader,
    audio_rx: &mut tokio_mpsc::UnboundedReceiver<AudioCmd>,
    event_tx: &crossbeam_channel::Sender<DeepgramEvent>,
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
    writer: &mut AsrWsWriter,
    reader: &mut AsrWsReader,
    audio_rx: &mut tokio_mpsc::UnboundedReceiver<AudioCmd>,
    event_tx: &crossbeam_channel::Sender<DeepgramEvent>,
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
            // Provider keepalive: Deepgram expects this as a text frame during
            // idle periods. It should not be sent as binary audio.
            _ = keep_alive.tick() => {
                if last_outbound.elapsed() >= keepalive_interval {
                    if let Err(e) = write_guard
                        .send_text(
                            writer,
                            AsrTransportPayloadKind::Terminal,
                            KEEPALIVE_PAYLOAD.to_string(),
                        )
                        .await
                    {
                        let policy_blocked = e.is_policy_blocked();
                        let message = crate::error::redacted_provider_diagnostic(
                            &format!("keepalive failed: {e}"),
                            [api_key],
                        );
                        log::error!("Deepgram: failed to send keepalive: {message}");
                        return if policy_blocked {
                            DisconnectKind::PolicyBlocked(message)
                        } else {
                            DisconnectKind::NetworkError(message)
                        };
                    }
                    last_outbound = tokio::time::Instant::now();
                }
            }

            // Writer side: audio command from the caller.
            cmd = audio_rx.recv() => {
                match cmd {
                    Some(AudioCmd::Chunk(pcm_bytes)) => {
                        // Decrement on consumption. Keep this symmetric with
                        // the increment in `send_audio` so the backlog metric
                        // stays accurate whether the frame sends or errors out.
                        pending_chunks.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                        if let Err(e) = write_guard
                            .send_binary(writer, AsrTransportPayloadKind::Audio, pcm_bytes)
                            .await
                        {
                            let policy_blocked = e.is_policy_blocked();
                            let message = crate::error::redacted_provider_diagnostic(
                                &format!("send failed: {e}"),
                                [api_key],
                            );
                            log::error!("Deepgram: failed to send audio: {message}");
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
                        let _ = write_guard
                            .send_binary(writer, AsrTransportPayloadKind::Terminal, Vec::new())
                            .await;
                        let _ = writer.close().await;
                        return DisconnectKind::UserRequested;
                    }
                    None => {
                        // Caller dropped the sender. No more audio will ever
                        // arrive — end the session without reconnecting.
                        let _ = write_guard
                            .send_binary(writer, AsrTransportPayloadKind::Terminal, Vec::new())
                            .await;
                        let _ = writer.close().await;
                        return DisconnectKind::WriterEnded;
                    }
                }
            }

            // Reader side: inbound frame from Deepgram.
            result = reader.next() => {
                let Some(result) = result else {
                    // Reader stream ended without a Close frame — treat as a
                    // network-layer drop.
                    return DisconnectKind::NetworkError("reader stream ended".into());
                };

                match result {
                    Ok(Message::Text(text)) => {
                        handle_server_message_with_key(&text, event_tx, api_key);
                    }
                    Ok(Message::Close(frame)) => {
                        // If the user was the one asking to close, honour that;
                        // otherwise classify as a server-initiated close that
                        // should trigger reconnect.
                        if user_disconnected.load(Ordering::SeqCst) {
                            return DisconnectKind::UserRequested;
                        }
                        let reason = frame
                            .map(|f| {
                                let code: u16 = f.code.into();
                                close_frame_diagnostic(code, f.reason.as_ref())
                            })
                            .unwrap_or_else(|| "no_frame".into());
                        log::info!("Deepgram: server closed connection {reason}");
                        return DisconnectKind::ServerClose(reason);
                    }
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {
                        // Protocol-level frames; nothing to do.
                    }
                    Ok(Message::Binary(_)) => {
                        // Unexpected for Deepgram text-mode responses.
                        log::debug!("Deepgram: unexpected binary message");
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
                        log::error!("Deepgram: WebSocket read error: {message}");
                        return DisconnectKind::NetworkError(message);
                    }
                }
            }
        }
    }
}

/// Parse a single Deepgram server JSON message and emit appropriate events.
#[cfg(test)]
pub(super) fn handle_server_message(text: &str, tx: &crossbeam_channel::Sender<DeepgramEvent>) {
    handle_server_message_with_key(text, tx, "");
}

fn handle_server_message_with_key(
    text: &str,
    tx: &crossbeam_channel::Sender<DeepgramEvent>,
    api_key: &str,
) {
    let parsed: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("Deepgram: invalid JSON: {e}");
            let _ = tx.send(DeepgramEvent::Error {
                message: format!("Invalid server JSON: {e}"),
            });
            return;
        }
    };

    // Deepgram Nova uses `type`; Flux turn messages may carry the provider
    // event name under `event`.
    let msg_type = parsed
        .get("type")
        .or_else(|| parsed.get("event"))
        .and_then(|t| t.as_str())
        .unwrap_or("");

    if msg_type == "Error" || looks_like_deepgram_error_object(&parsed) {
        let _ = tx.send(DeepgramEvent::Error {
            message: deepgram_error_message(&parsed, text, api_key),
        });
        return;
    }

    match msg_type {
        "Results" => {
            // Extract transcript data from the Deepgram response.
            let is_final = parsed
                .get("is_final")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let speech_final = parsed
                .get("speech_final")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let start = parsed.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let duration = parsed
                .get("duration")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            // Navigate: channel -> alternatives[0]
            let alternative = parsed
                .get("channel")
                .and_then(|ch| ch.get("alternatives"))
                .and_then(|alts| alts.as_array())
                .and_then(|alts| alts.first());

            if let Some(alt) = alternative {
                let transcript_text = alt
                    .get("transcript")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .to_string();

                let confidence = alt
                    .get("confidence")
                    .and_then(|c| c.as_f64())
                    .unwrap_or(0.0) as f32;

                // Parse words array
                let words: Vec<DeepgramWord> = alt
                    .get("words")
                    .and_then(|w| w.as_array())
                    .map(|words_arr| {
                        words_arr
                            .iter()
                            .filter_map(|w| {
                                let word = w.get("word")?.as_str()?.to_string();
                                let word_start =
                                    w.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let end = w.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0);
                                let conf =
                                    w.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0)
                                        as f32;
                                let speaker =
                                    w.get("speaker").and_then(|v| v.as_u64()).map(|s| s as u32);
                                Some(DeepgramWord {
                                    word,
                                    start: word_start,
                                    end,
                                    confidence: conf,
                                    speaker,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                // Only emit if there's actual transcript text
                if !transcript_text.is_empty() {
                    let _ = tx.send(DeepgramEvent::Transcript {
                        text: transcript_text.clone(),
                        confidence,
                        is_final,
                        speech_final,
                        start,
                        duration,
                        words,
                    });
                }

                if speech_final {
                    let _ = tx.send(DeepgramEvent::Turn {
                        kind: DeepgramTurnKind::SpeechFinal,
                        text: (!transcript_text.is_empty()).then_some(transcript_text),
                        start: Some(start),
                        end: Some(start + duration),
                        confidence: Some(confidence),
                        turn_index: parsed
                            .get("turn_index")
                            .and_then(|v| v.as_u64())
                            .or_else(|| parsed.get("turnIndex").and_then(|v| v.as_u64())),
                    });
                }
            }
        }
        "TurnInfo" => {
            handle_flux_turn_info(&parsed, tx, api_key);
        }
        "StartOfTurn" => {
            emit_simple_deepgram_turn(&parsed, tx, DeepgramTurnKind::StartOfTurn);
        }
        "EagerEndOfTurn" => {
            emit_simple_deepgram_turn(&parsed, tx, DeepgramTurnKind::EagerEndOfTurn);
        }
        "EndOfTurn" => {
            emit_simple_deepgram_turn(&parsed, tx, DeepgramTurnKind::EndOfTurn);
        }
        "TurnResumed" => {
            emit_simple_deepgram_turn(&parsed, tx, DeepgramTurnKind::TurnResumed);
        }
        "Metadata" => {
            log::debug!(
                "Deepgram: received metadata request_id={} fields={}",
                json_string_field(&parsed, &["request_id", "requestId"])
                    .unwrap_or_else(|| "none".to_string()),
                json_field_count(&parsed)
            );
        }
        "UtteranceEnd" => {
            let last_word_end = parsed
                .get("last_word_end")
                .and_then(|v| v.as_f64())
                .or_else(|| parsed.get("lastWordEnd").and_then(|v| v.as_f64()));
            if matches!(last_word_end, Some(value) if value < 0.0) {
                log::debug!("Deepgram: ignoring UtteranceEnd with last_word_end=-1");
                return;
            }
            let _ = tx.send(DeepgramEvent::Turn {
                kind: DeepgramTurnKind::UtteranceEnd,
                text: None,
                start: None,
                end: last_word_end,
                confidence: None,
                turn_index: parsed
                    .get("turn_index")
                    .and_then(|v| v.as_u64())
                    .or_else(|| parsed.get("turnIndex").and_then(|v| v.as_u64())),
            });
        }
        "SpeechStarted" => {
            let timestamp = parsed
                .get("timestamp")
                .and_then(|v| v.as_f64())
                .or_else(|| parsed.get("start").and_then(|v| v.as_f64()));
            let _ = tx.send(DeepgramEvent::Turn {
                kind: DeepgramTurnKind::SpeechStarted,
                text: None,
                start: timestamp,
                end: None,
                confidence: None,
                turn_index: parsed
                    .get("turn_index")
                    .and_then(|v| v.as_u64())
                    .or_else(|| parsed.get("turnIndex").and_then(|v| v.as_u64())),
            });
        }
        _ => {
            log::debug!(
                "Deepgram: unhandled message type='{msg_type}' request_id={} fields={}",
                json_string_field(&parsed, &["request_id", "requestId"])
                    .unwrap_or_else(|| "none".to_string()),
                json_field_count(&parsed)
            );
        }
    }
}

fn looks_like_deepgram_error_object(parsed: &Value) -> bool {
    parsed.get("err_code").is_some()
        || parsed.get("err_msg").is_some()
        || parsed.get("category").is_some()
        || parsed.get("error").is_some()
}

fn deepgram_error_message(parsed: &Value, _raw_text: &str, api_key: &str) -> String {
    let code = parsed
        .get("code")
        .or_else(|| parsed.get("err_code"))
        .or_else(|| parsed.get("category"))
        .and_then(|value| value.as_str());
    let description_len = parsed
        .get("description")
        .or_else(|| parsed.get("message"))
        .or_else(|| parsed.get("err_msg"))
        .or_else(|| parsed.get("details"))
        .or_else(|| parsed.get("error"))
        .and_then(|value| value.as_str())
        .map(|value| value.chars().count());
    let request_id = parsed.get("request_id").and_then(|value| value.as_str());

    let message = match (code, request_id, description_len) {
        (Some(code), Some(request_id), Some(description_len)) => {
            format!(
                "Deepgram error code={code} request_id={request_id} description_len={description_len}"
            )
        }
        (Some(code), None, Some(description_len)) => {
            format!("Deepgram error code={code} description_len={description_len}")
        }
        (Some(code), Some(request_id), None) => {
            format!("Deepgram error code={code} request_id={request_id}")
        }
        (Some(code), None, None) => format!("Deepgram error code={code}"),
        (None, Some(request_id), Some(description_len)) => {
            format!("Deepgram error request_id={request_id} description_len={description_len}")
        }
        (None, None, Some(description_len)) => {
            format!("Deepgram error description_len={description_len}")
        }
        (None, Some(request_id), None) => format!("Deepgram error request_id={request_id}"),
        (None, None, None) => format!(
            "Deepgram error frame type={} fields={}",
            json_string_field(parsed, &["type", "event"]).unwrap_or_else(|| "unknown".to_string()),
            json_field_count(parsed)
        ),
    };

    crate::error::redacted_provider_diagnostic(&message, [api_key])
}

fn handle_flux_turn_info(
    parsed: &Value,
    tx: &crossbeam_channel::Sender<DeepgramEvent>,
    _api_key: &str,
) {
    let event_name = parsed
        .get("event")
        .or_else(|| parsed.get("turn_event"))
        .or_else(|| parsed.get("state"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    match event_name {
        "StartOfTurn" => emit_simple_deepgram_turn(parsed, tx, DeepgramTurnKind::StartOfTurn),
        "EagerEndOfTurn" => emit_simple_deepgram_turn(parsed, tx, DeepgramTurnKind::EagerEndOfTurn),
        "EndOfTurn" => emit_simple_deepgram_turn(parsed, tx, DeepgramTurnKind::EndOfTurn),
        "TurnResumed" => emit_simple_deepgram_turn(parsed, tx, DeepgramTurnKind::TurnResumed),
        _ => {
            log::debug!(
                "Deepgram: unhandled Flux TurnInfo event='{event_name}' request_id={} fields={}",
                json_string_field(parsed, &["request_id", "requestId"])
                    .unwrap_or_else(|| "none".to_string()),
                json_field_count(parsed)
            );
        }
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

fn emit_simple_deepgram_turn(
    parsed: &Value,
    tx: &crossbeam_channel::Sender<DeepgramEvent>,
    kind: DeepgramTurnKind,
) {
    let start = parsed
        .get("start")
        .or_else(|| parsed.get("start_time"))
        .or_else(|| parsed.get("startTime"))
        .and_then(|v| v.as_f64());
    let end = parsed
        .get("end")
        .or_else(|| parsed.get("end_time"))
        .or_else(|| parsed.get("endTime"))
        .and_then(|v| v.as_f64());
    let confidence = parsed
        .get("confidence")
        .and_then(|v| v.as_f64())
        .map(|v| v as f32);
    let text = parsed
        .get("transcript")
        .or_else(|| parsed.get("text"))
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.trim().is_empty());
    let turn_index = parsed
        .get("turn_index")
        .and_then(|v| v.as_u64())
        .or_else(|| parsed.get("turnIndex").and_then(|v| v.as_u64()));

    let _ = tx.send(DeepgramEvent::Turn {
        kind,
        text,
        start,
        end,
        confidence,
        turn_index,
    });
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert f32 PCM samples (range -1.0 ... +1.0) to little-endian i16 bytes.
fn f32_to_i16_le_bytes(samples: &[f32]) -> Vec<u8> {
    crate::audio::pcm::f32_mono_to_pcm_s16le_bytes(samples)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::ws_fixture;

    fn test_config(model: &str) -> DeepgramConfig {
        DeepgramConfig {
            api_key: "key".into(),
            model: model.into(),
            enable_diarization: true,
            endpointing_ms: Some(300),
            utterance_end_ms: Some(1000),
            vad_events: true,
            eot_threshold: Some(0.5),
            eager_eot_threshold: None,
            eot_timeout_ms: None,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        }
    }

    fn with_blocked_content_egress(mut config: DeepgramConfig) -> DeepgramConfig {
        config.api_key = "dg-private-api-key".into();
        config.content_egress_policy = crate::asr::ProviderContentEgressPolicy::block("local_only");
        config
    }

    #[test]
    fn deepgram_config_debug_redacts_api_key() {
        let mut config = test_config("nova-3");
        config.api_key = "dg-debug-secret".into();

        let debug = format!("{config:?}");

        assert!(!debug.contains("dg-debug-secret"));
        assert!(debug.contains("<present>"));
        assert!(debug.contains("nova-3"));
        assert!(debug.contains("endpointing_ms"));
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
    fn client_new_is_disconnected() {
        let client = DeepgramStreamingClient::new(test_config("nova-3"));
        assert!(!client.is_connected());
    }

    #[test]
    fn connect_fails_without_api_key() {
        let mut config = test_config("nova-3");
        config.api_key.clear();
        config.enable_diarization = false;
        let mut client = DeepgramStreamingClient::new(config);
        let result = client.connect();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("API key"));
    }

    #[test]
    fn send_audio_fails_when_disconnected() {
        let mut config = test_config("nova-3");
        config.enable_diarization = false;
        let client = DeepgramStreamingClient::new(config);
        let result = client.send_audio(&[0.5, -0.3]);
        assert!(result.is_err());
    }

    #[test]
    fn blocked_policy_rejects_non_empty_audio_before_channel_initialization() {
        let client =
            DeepgramStreamingClient::new(with_blocked_content_egress(test_config("nova-3")));

        let error = client.send_audio(&[0.5, -0.3]).unwrap_err();

        assert!(error.contains("Privacy policy blocked"));
        assert!(error.contains("asr.deepgram"));
        assert!(error.contains("local_only"));
        assert!(!error.contains("Audio channel not initialized"));
    }

    #[test]
    fn blocked_policy_allows_empty_audio_without_channel_initialization() {
        let client =
            DeepgramStreamingClient::new(with_blocked_content_egress(test_config("nova-3")));

        assert!(client.send_audio(&[]).is_ok());
    }

    #[test]
    fn blocked_policy_error_redacts_secret_audio_and_transcript_like_values() {
        let client =
            DeepgramStreamingClient::new(with_blocked_content_egress(test_config("nova-3")));

        let error = client.send_audio(&[0.5, -0.3]).unwrap_err();

        for forbidden in [
            "dg-private-api-key",
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
    fn handle_deepgram_transcript_result() {
        let (tx, rx) = crossbeam_channel::bounded(16);

        let msg = r#"{
            "type": "Results",
            "channel_index": [0, 1],
            "duration": 1.5,
            "start": 0.0,
            "is_final": true,
            "speech_final": true,
            "channel": {
                "alternatives": [{
                    "transcript": "hello world",
                    "confidence": 0.98,
                    "words": [
                        {"word": "hello", "start": 0.1, "end": 0.4, "confidence": 0.99, "speaker": 0},
                        {"word": "world", "start": 0.5, "end": 0.9, "confidence": 0.97, "speaker": 0}
                    ]
                }]
            }
        }"#;

        handle_server_message(msg, &tx);

        let event = rx.try_recv().unwrap();
        match event {
            DeepgramEvent::Transcript {
                text,
                confidence,
                is_final,
                speech_final,
                words,
                ..
            } => {
                assert_eq!(text, "hello world");
                assert!((confidence - 0.98).abs() < 0.01);
                assert!(is_final);
                assert!(speech_final);
                assert_eq!(words.len(), 2);
                assert_eq!(words[0].word, "hello");
                assert_eq!(words[0].speaker, Some(0));
                assert_eq!(words[1].word, "world");
            }
            _ => panic!("Expected Transcript event"),
        }
    }

    #[test]
    fn speech_final_result_emits_turn_event() {
        let (tx, rx) = crossbeam_channel::bounded(16);

        let msg = r#"{
            "type": "Results",
            "duration": 0.8,
            "start": 2.0,
            "is_final": true,
            "speech_final": true,
            "channel": {
                "alternatives": [{
                    "transcript": "done now",
                    "confidence": 0.91,
                    "words": []
                }]
            }
        }"#;

        handle_server_message(msg, &tx);
        let _transcript = rx.try_recv().unwrap();
        let event = rx.try_recv().unwrap();
        match event {
            DeepgramEvent::Turn {
                kind,
                text,
                start,
                end,
                confidence,
                ..
            } => {
                assert!(matches!(kind, DeepgramTurnKind::SpeechFinal));
                assert_eq!(text.as_deref(), Some("done now"));
                assert_eq!(start, Some(2.0));
                assert_eq!(end, Some(2.8));
                assert_eq!(confidence, Some(0.91));
            }
            other => panic!("Expected turn event, got {other:?}"),
        }
    }

    #[test]
    fn utterance_end_with_negative_last_word_end_is_ignored() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message(
            r#"{"type":"UtteranceEnd","channel":[0,1],"last_word_end":-1}"#,
            &tx,
        );
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn speech_started_and_utterance_end_emit_turn_events() {
        let (tx, rx) = crossbeam_channel::bounded(16);

        handle_server_message(r#"{"type":"SpeechStarted","timestamp":1.25}"#, &tx);
        handle_server_message(r#"{"type":"UtteranceEnd","last_word_end":3.5}"#, &tx);

        match rx.try_recv().unwrap() {
            DeepgramEvent::Turn { kind, start, .. } => {
                assert!(matches!(kind, DeepgramTurnKind::SpeechStarted));
                assert_eq!(start, Some(1.25));
            }
            other => panic!("Expected SpeechStarted turn, got {other:?}"),
        }
        match rx.try_recv().unwrap() {
            DeepgramEvent::Turn { kind, end, .. } => {
                assert!(matches!(kind, DeepgramTurnKind::UtteranceEnd));
                assert_eq!(end, Some(3.5));
            }
            other => panic!("Expected UtteranceEnd turn, got {other:?}"),
        }
    }

    #[test]
    fn flux_turn_info_events_are_parsed() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        handle_server_message(
            r#"{"type":"TurnInfo","event":"EagerEndOfTurn","turn_index":7,"transcript":"maybe done","confidence":0.82}"#,
            &tx,
        );
        handle_server_message(
            r#"{"type":"TurnInfo","event":"TurnResumed","turn_index":7}"#,
            &tx,
        );

        match rx.try_recv().unwrap() {
            DeepgramEvent::Turn {
                kind,
                text,
                turn_index,
                ..
            } => {
                assert!(matches!(kind, DeepgramTurnKind::EagerEndOfTurn));
                assert_eq!(text.as_deref(), Some("maybe done"));
                assert_eq!(turn_index, Some(7));
            }
            other => panic!("Expected eager turn event, got {other:?}"),
        }
        match rx.try_recv().unwrap() {
            DeepgramEvent::Turn {
                kind, turn_index, ..
            } => {
                assert!(matches!(kind, DeepgramTurnKind::TurnResumed));
                assert_eq!(turn_index, Some(7));
            }
            other => panic!("Expected resumed turn event, got {other:?}"),
        }
    }

    #[test]
    fn listen_url_routes_nova_and_flux_parameters() {
        let nova_url = deepgram_listen_url(&test_config("nova-3"));
        assert!(nova_url.starts_with("wss://api.deepgram.com/v1/listen?"));
        assert!(nova_url.contains("&endpointing=300"));
        assert!(nova_url.contains("&utterance_end_ms=1000"));
        assert!(nova_url.contains("&vad_events=true"));
        assert!(!nova_url.contains("eot_threshold"));

        let mut flux = test_config("flux-general-en");
        flux.eager_eot_threshold = Some(0.35);
        flux.eot_timeout_ms = Some(1500);
        let flux_url = deepgram_listen_url(&flux);
        assert!(flux_url.starts_with("wss://api.deepgram.com/v2/listen?"));
        assert!(flux_url.contains("&eot_threshold=0.5"));
        assert!(flux_url.contains("&eager_eot_threshold=0.35"));
        assert!(flux_url.contains("&eot_timeout_ms=1500"));
        assert!(!flux_url.contains("utterance_end_ms"));
    }

    #[test]
    fn handle_empty_transcript_not_emitted() {
        let (tx, rx) = crossbeam_channel::bounded(16);

        let msg = r#"{
            "type": "Results",
            "channel_index": [0, 1],
            "duration": 0.5,
            "start": 0.0,
            "is_final": false,
            "speech_final": false,
            "channel": {
                "alternatives": [{
                    "transcript": "",
                    "confidence": 0.0,
                    "words": []
                }]
            }
        }"#;

        handle_server_message(msg, &tx);

        assert!(
            rx.try_recv().is_err(),
            "Empty transcript should not emit event"
        );
    }

    #[test]
    fn event_serialization_roundtrip() {
        let events = vec![
            DeepgramEvent::Transcript {
                text: "hello".into(),
                confidence: 0.95,
                is_final: true,
                speech_final: true,
                start: 0.0,
                duration: 1.0,
                words: vec![DeepgramWord {
                    word: "hello".into(),
                    start: 0.0,
                    end: 0.5,
                    confidence: 0.95,
                    speaker: Some(0),
                }],
            },
            DeepgramEvent::Error {
                message: "oops".into(),
            },
            DeepgramEvent::Connected,
            DeepgramEvent::Disconnected,
            DeepgramEvent::Reconnecting {
                attempt: 2,
                backoff_secs: 2,
            },
            DeepgramEvent::Reconnected,
            DeepgramEvent::Turn {
                kind: DeepgramTurnKind::EndOfTurn,
                text: Some("done".into()),
                start: Some(0.0),
                end: Some(1.0),
                confidence: Some(0.9),
                turn_index: Some(1),
            },
        ];

        for event in &events {
            let json = serde_json::to_string(event).unwrap();
            let _parsed: Value = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn emit_disconnected_once_dedupes() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let guard = Arc::new(AtomicBool::new(false));

        assert!(emit_disconnected_once(&tx, &guard));
        assert!(!emit_disconnected_once(&tx, &guard));
        assert!(!emit_disconnected_once(&tx, &guard));

        assert!(matches!(rx.try_recv(), Ok(DeepgramEvent::Disconnected)));
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
        assert!(matches!(rx.try_recv(), Ok(DeepgramEvent::Disconnected)));

        guard.store(false, Ordering::SeqCst);
        assert!(emit_disconnected_once(&tx, &guard));
        assert!(matches!(rx.try_recv(), Ok(DeepgramEvent::Disconnected)));
    }

    #[test]
    fn backoff_schedule_matches_spec() {
        // 1s, 2s, 5s, 10s, then give up.
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(2), Some(2));
        assert_eq!(backoff_for_attempt(3), Some(5));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), None);
        assert_eq!(backoff_for_attempt(99), None);
    }

    #[test]
    fn next_reconnect_step_increments_exactly_once_per_attempt() {
        // The first disconnect leaves prior_attempts == 0; each call advances
        // the ladder by exactly one attempt with the matching backoff.
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
        assert_eq!(
            next_reconnect_step(2),
            ReconnectStep::Retry {
                attempt: 3,
                backoff_secs: 5
            }
        );
        assert_eq!(
            next_reconnect_step(3),
            ReconnectStep::Retry {
                attempt: 4,
                backoff_secs: 10
            }
        );
        // Fifth call exhausts the budget — give up, reporting the 4 attempts
        // already made (never a fifth phantom attempt).
        assert_eq!(
            next_reconnect_step(4),
            ReconnectStep::GiveUp { attempted: 4 }
        );
    }

    /// FA-2 regression: a single `open_ws` failure must advance the ladder by
    /// exactly ONE attempt and emit exactly ONE `Reconnecting` — never two.
    /// Before the fix, an `open_ws` Err `continue`d back through `run_io` with a
    /// dead socket, which re-disconnected and re-ran the backoff branch, so one
    /// failed reconnect double-counted the attempt and double-fired events. Here
    /// we model the session loop's ladder stepping (the part the bug lived in):
    /// drive N consecutive failures and assert the counter and emit log match
    /// the attempt count one-to-one.
    #[test]
    fn single_open_ws_failure_counts_one_attempt_one_reconnecting() {
        // Mirror the production loop: `reconnect_attempts` starts at 0 after the
        // first disconnect. Each iteration represents one open_ws call; we make
        // every call "fail" (continue) and record the emitted Reconnecting.
        let mut reconnect_attempts: u32 = 0;
        let mut reconnecting_emits: Vec<u32> = Vec::new();

        // Simulate the inner reconnect loop with all open_ws attempts failing.
        let gave_up_after = loop {
            match next_reconnect_step(reconnect_attempts) {
                ReconnectStep::Retry {
                    attempt,
                    backoff_secs,
                } => {
                    reconnect_attempts = attempt;
                    // Exactly one Reconnecting emit per ladder step.
                    reconnecting_emits.push(attempt);
                    // Backoff must match the published schedule.
                    assert_eq!(backoff_for_attempt(attempt), Some(backoff_secs));
                    // open_ws "fails" → loop continues to the *next* attempt
                    // inline, without any run_io detour.
                    continue;
                }
                ReconnectStep::GiveUp { attempted } => {
                    break attempted;
                }
            }
        };

        // Four attempts → four distinct increments → four Reconnecting emits,
        // strictly monotonic with no duplicates/doubling.
        assert_eq!(reconnecting_emits, vec![1, 2, 3, 4]);
        assert_eq!(reconnect_attempts, 4);
        assert_eq!(gave_up_after, 4);
    }

    async fn recv_event(
        rx: &crossbeam_channel::Receiver<DeepgramEvent>,
        timeout: Duration,
    ) -> DeepgramEvent {
        tokio::time::timeout(timeout, async {
            loop {
                if let Ok(event) = rx.try_recv() {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for Deepgram event")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_io_fake_server_writes_audio_reads_results_and_stops() {
        let (url, server) = ws_fixture::spawn_scripted_server(vec![
            ws_fixture::ServerStep::send_text(
                r#"{"type":"Results","is_final":true,"speech_final":true,"start":0.0,"duration":0.5,"channel":{"alternatives":[{"transcript":"fake result","confidence":0.77,"words":[]}]}}"#,
            ),
            ws_fixture::ServerStep::expect_binary(vec![1, 2, 3, 4]),
            ws_fixture::ServerStep::expect_binary(Vec::<u8>::new()),
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
            "asr.deepgram",
            crate::asr::ProviderContentEgressPolicy::allow(),
        );

        let run = tokio::spawn({
            let user_disconnected = Arc::clone(&user_disconnected);
            let pending_chunks = Arc::clone(&pending_chunks);
            // Move the write guard into the spawned block, mirroring the
            // `Arc::clone` re-bindings above so `async move` captures it here.
            #[allow(clippy::redundant_locals)]
            let write_guard = write_guard;
            async move {
                run_io(
                    &mut writer,
                    &mut reader,
                    &mut audio_rx,
                    &event_tx,
                    &user_disconnected,
                    &pending_chunks,
                    &write_guard,
                    "test-key",
                )
                .await
            }
        });

        audio_tx
            .send(AudioCmd::Chunk(vec![1, 2, 3, 4]))
            .expect("queue audio chunk");

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            DeepgramEvent::Transcript { text, is_final, .. } => {
                assert_eq!(text, "fake result");
                assert!(is_final);
            }
            other => panic!("expected transcript from fake server, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            DeepgramEvent::Turn {
                kind, text, end, ..
            } => {
                assert!(matches!(kind, DeepgramTurnKind::SpeechFinal));
                assert_eq!(text.as_deref(), Some("fake result"));
                assert_eq!(end, Some(0.5));
            }
            other => panic!("expected speech-final turn from fake server, got {other:?}"),
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
            Some(&ws_fixture::ClientFrame::Binary(vec![1, 2, 3, 4]))
        );
        assert!(
            client_frames.iter().any(
                |frame| matches!(frame, ws_fixture::ClientFrame::Binary(bytes) if bytes.is_empty())
            ),
            "stop command should send the terminal empty binary frame"
        );
        assert_eq!(client_frames.get(2), Some(&ws_fixture::ClientFrame::Close));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_io_fake_server_sends_idle_keepalive_text_then_stops_cleanly() {
        let (keepalive_tx, keepalive_rx) = tokio::sync::oneshot::channel();
        let (server_tx, server_rx) = tokio::sync::oneshot::channel();

        let (url, server) = ws_fixture::spawn_server(move |mut websocket| async move {
            let mut keepalive_tx = Some(keepalive_tx);
            let mut text_frames = Vec::new();
            let mut binary_frames = Vec::new();

            while let Some(frame) = websocket.next().await {
                match frame.expect("server frame") {
                    Message::Text(text) => {
                        let text = text.to_string();
                        if text == KEEPALIVE_PAYLOAD
                            && let Some(tx) = keepalive_tx.take()
                        {
                            let _ = tx.send(binary_frames.len());
                        }
                        text_frames.push(text);
                    }
                    Message::Binary(bytes) => {
                        binary_frames.push(bytes.to_vec());
                        if binary_frames.last().is_some_and(Vec::is_empty) {
                            break;
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }

            let _ = server_tx.send((text_frames, binary_frames));
        })
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (mut writer, mut reader) = client_socket.split();
        let (audio_tx, mut audio_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, _event_rx) = crossbeam_channel::bounded(16);
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let write_guard = AsrWsWriteGuard::new(
            "asr.deepgram",
            crate::asr::ProviderContentEgressPolicy::allow(),
        );

        let run = tokio::spawn({
            let user_disconnected = Arc::clone(&user_disconnected);
            let pending_chunks = Arc::clone(&pending_chunks);
            // Move the write guard into the spawned block, mirroring the
            // `Arc::clone` re-bindings above so `async move` captures it here.
            #[allow(clippy::redundant_locals)]
            let write_guard = write_guard;
            async move {
                run_io_with_keepalive_interval(
                    &mut writer,
                    &mut reader,
                    &mut audio_rx,
                    &event_tx,
                    &user_disconnected,
                    &pending_chunks,
                    &write_guard,
                    "test-key",
                    Duration::from_millis(20),
                )
                .await
            }
        });

        let binary_count_when_keepalive_arrived =
            tokio::time::timeout(Duration::from_secs(1), keepalive_rx)
                .await
                .expect("idle socket should send keepalive text")
                .expect("keepalive sender dropped");
        assert_eq!(
            binary_count_when_keepalive_arrived, 0,
            "idle keepalive must not be sent as binary audio"
        );
        assert_eq!(
            pending_chunks.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "idle keepalive must not change pending audio count"
        );

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
            "stop after idle keepalive must leave pending audio count unchanged"
        );

        let (text_frames, binary_frames) = tokio::time::timeout(Duration::from_secs(1), server_rx)
            .await
            .expect("server should report observed frames")
            .expect("server oneshot dropped");
        assert!(
            text_frames.iter().any(|frame| frame == KEEPALIVE_PAYLOAD),
            "server should observe Deepgram keepalive text frame"
        );
        assert_eq!(
            binary_frames,
            vec![Vec::<u8>::new()],
            "idle session should send only the terminal empty binary frame"
        );

        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_io_blocked_policy_sends_no_audio_content_frame() {
        let (server_tx, server_rx) = tokio::sync::oneshot::channel();

        let (url, server) = ws_fixture::spawn_server(move |mut websocket| async move {
            let mut text_frames = Vec::new();
            let mut binary_frames = Vec::new();

            while let Some(frame) = websocket.next().await {
                match frame {
                    Ok(Message::Text(text)) => text_frames.push(text.to_string()),
                    Ok(Message::Binary(bytes)) => {
                        binary_frames.push(bytes.to_vec());
                        if binary_frames.last().is_some_and(Vec::is_empty) {
                            break;
                        }
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => {}
                }
            }

            let _ = server_tx.send((text_frames, binary_frames));
        })
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (mut writer, mut reader) = client_socket.split();
        let (audio_tx, mut audio_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, _event_rx) = crossbeam_channel::bounded(16);
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let write_guard = AsrWsWriteGuard::new(
            "asr.deepgram",
            crate::asr::ProviderContentEgressPolicy::block("local_only"),
        );

        let run = tokio::spawn({
            let user_disconnected = Arc::clone(&user_disconnected);
            let pending_chunks = Arc::clone(&pending_chunks);
            // Move the write guard into the spawned block, mirroring the
            // `Arc::clone` re-bindings above so `async move` captures it here.
            #[allow(clippy::redundant_locals)]
            let write_guard = write_guard;
            async move {
                run_io(
                    &mut writer,
                    &mut reader,
                    &mut audio_rx,
                    &event_tx,
                    &user_disconnected,
                    &pending_chunks,
                    &write_guard,
                    "dg-private-api-key",
                )
                .await
            }
        });

        audio_tx
            .send(AudioCmd::Chunk(vec![1, 2, 3, 4]))
            .expect("queue audio chunk");

        let disconnect = tokio::time::timeout(Duration::from_secs(1), run)
            .await
            .expect("run_io should exit after policy block")
            .expect("run_io task panicked");
        match disconnect {
            DisconnectKind::PolicyBlocked(message) => {
                assert!(message.contains("Privacy policy blocked"));
                assert!(message.contains("asr.deepgram"));
                assert!(message.contains("local_only"));
                assert!(!message.contains("dg-private-api-key"));
            }
            other => panic!("expected policy-blocked disconnect, got {other:?}"),
        }
        assert_eq!(
            pending_chunks.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "blocked audio send should still decrement pending count"
        );

        drop(audio_tx);

        let (text_frames, binary_frames) = tokio::time::timeout(Duration::from_secs(1), server_rx)
            .await
            .expect("server should report observed frames")
            .expect("server oneshot dropped");
        assert!(
            text_frames.is_empty(),
            "blocked audio send should not require keepalive/control traffic"
        );
        assert!(
            binary_frames.is_empty(),
            "blocked policy must prevent Deepgram audio content from reaching the socket"
        );

        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn session_task_cancels_during_reconnect_backoff() {
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
        let disconnected_emitted = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let handle = tokio::spawn(session_task(DeepgramSessionCtx {
            writer,
            reader,
            audio_rx,
            config: test_config("nova-3"),
            event_tx,
            connected: Arc::clone(&connected),
            user_disconnected: Arc::clone(&user_disconnected),
            disconnected_emitted: Arc::clone(&disconnected_emitted),
            pending_chunks,
            reconnect_opener: None,
            run_io_entries: None,
        }));

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            DeepgramEvent::Disconnected => {}
            other => panic!("expected initial Disconnected event, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            DeepgramEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                assert_eq!(attempt, 1);
                assert_eq!(backoff_secs, 1);
            }
            other => panic!("expected Reconnecting event, got {other:?}"),
        }

        user_disconnected.store(true, Ordering::SeqCst);

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("session task should exit before reconnect backoff completes")
            .expect("session task panicked");
        assert!(
            !connected.load(Ordering::SeqCst),
            "cancelled reconnect must leave connected=false"
        );
        assert!(
            event_rx.try_iter().all(|event| !matches!(
                event,
                DeepgramEvent::Disconnected | DeepgramEvent::Reconnected
            )),
            "cancel during backoff must not emit duplicate Disconnected or Reconnected"
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
        let disconnected_emitted = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let run_io_entries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let opener_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (reconnected_frames_tx, mut reconnected_frames_rx) =
            tokio::sync::mpsc::unbounded_channel::<Vec<Vec<u8>>>();

        let opener: ReconnectOpener = {
            let opener_calls = Arc::clone(&opener_calls);
            Arc::new(move |_config| {
                let opener_calls = Arc::clone(&opener_calls);
                let reconnected_frames_tx = reconnected_frames_tx.clone();
                Box::pin(async move {
                    opener_calls.fetch_add(1, Ordering::SeqCst);
                    let (url, _server) = ws_fixture::spawn_server(move |mut websocket| async move {
                        websocket
                            .send(Message::Text(
                                r#"{"type":"Results","is_final":true,"speech_final":true,"start":1.0,"duration":0.25,"channel":{"alternatives":[{"transcript":"after reconnect","confidence":0.88,"words":[]}]}}"#
                                    .into(),
                            ))
                            .await
                            .expect("send reconnected result");

                        let mut binary_frames = Vec::new();
                        while let Some(frame) = websocket.next().await {
                            match frame.expect("reconnected server frame") {
                                Message::Binary(bytes) => {
                                    binary_frames.push(bytes.to_vec());
                                    if binary_frames.last().is_some_and(Vec::is_empty) {
                                        break;
                                    }
                                }
                                Message::Close(_) => break,
                                _ => {}
                            }
                        }
                        let _ = reconnected_frames_tx.send(binary_frames);
                    })
                    .await;

                    let socket = ws_fixture::connect_client(&url).await;
                    Ok(socket.split())
                })
            })
        };

        let handle = tokio::spawn(session_task(DeepgramSessionCtx {
            writer,
            reader,
            audio_rx,
            config: test_config("nova-3"),
            event_tx,
            connected: Arc::clone(&connected),
            user_disconnected: Arc::clone(&user_disconnected),
            disconnected_emitted,
            pending_chunks: Arc::clone(&pending_chunks),
            reconnect_opener: Some(opener),
            run_io_entries: Some(Arc::clone(&run_io_entries)),
        }));

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            DeepgramEvent::Disconnected => {}
            other => panic!("expected initial Disconnected event, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            DeepgramEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                assert_eq!(attempt, 1);
                assert_eq!(backoff_secs, 1);
            }
            other => panic!("expected first Reconnecting event, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(3)).await {
            DeepgramEvent::Reconnected => {}
            other => panic!("expected Reconnected event, got {other:?}"),
        }
        assert!(
            connected.load(Ordering::SeqCst),
            "successful reconnect must mark the session connected"
        );

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            DeepgramEvent::Transcript { text, is_final, .. } => {
                assert_eq!(text, "after reconnect");
                assert!(is_final);
            }
            other => panic!("expected transcript from reconnected socket, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            DeepgramEvent::Turn { kind, text, .. } => {
                assert!(matches!(kind, DeepgramTurnKind::SpeechFinal));
                assert_eq!(text.as_deref(), Some("after reconnect"));
            }
            other => panic!("expected turn from reconnected socket, got {other:?}"),
        }

        pending_chunks.store(1, Ordering::SeqCst);
        audio_tx
            .send(AudioCmd::Chunk(vec![9, 8, 7]))
            .expect("queue audio after reconnect");
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
            DeepgramEvent::Disconnected => {}
            other => panic!("expected final Disconnected after clean stop, got {other:?}"),
        }

        let binary_frames =
            tokio::time::timeout(Duration::from_secs(1), reconnected_frames_rx.recv())
                .await
                .expect("reconnected server should report binary frames")
                .expect("reconnected server sender dropped");
        assert_eq!(
            binary_frames.first().map(Vec::as_slice),
            Some(&[9, 8, 7][..])
        );
        assert!(
            binary_frames.iter().any(Vec::is_empty),
            "stop command should send the terminal empty binary frame on the reconnected socket"
        );

        tokio::time::timeout(Duration::from_secs(1), initial_server)
            .await
            .expect("initial server task should finish")
            .expect("initial server task panicked");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn user_stop_after_client_disconnected_emit_does_not_duplicate_event() {
        let (url, server) = ws_fixture::spawn_server(|mut websocket| async move {
            while let Some(frame) = websocket.next().await {
                match frame.expect("server frame") {
                    Message::Binary(bytes) if bytes.is_empty() => break,
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        })
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (writer, reader) = client_socket.split();
        let (audio_tx, audio_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, event_rx) = crossbeam_channel::bounded(16);
        let connected = Arc::new(AtomicBool::new(true));
        let user_disconnected = Arc::new(AtomicBool::new(true));
        let disconnected_emitted = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        assert!(emit_disconnected_once(&event_tx, &disconnected_emitted));

        let handle = tokio::spawn(session_task(DeepgramSessionCtx {
            writer,
            reader,
            audio_rx,
            config: test_config("nova-3"),
            event_tx,
            connected,
            user_disconnected,
            disconnected_emitted,
            pending_chunks,
            reconnect_opener: None,
            run_io_entries: None,
        }));

        audio_tx.send(AudioCmd::Stop).expect("queue stop");

        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("session task should exit after stop")
            .expect("session task panicked");
        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");

        let events: Vec<_> = event_rx.try_iter().collect();
        let disconnected_count = events
            .iter()
            .filter(|event| matches!(event, DeepgramEvent::Disconnected))
            .count();
        assert_eq!(
            disconnected_count, 1,
            "client-side disconnect emit plus session task stop must collapse to one event: {events:?}"
        );
    }
}
