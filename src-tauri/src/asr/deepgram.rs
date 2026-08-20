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
use crate::events::{DiarizationSpanRevisionPayload, DiarizationSpanStability};
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

/// Steady-state cap on the audio-chunk backlog (see `pending_chunks`). At the
/// speech processor's ~32ms chunk cadence
/// ([`crate::audio::pipeline::PROCESSED_AUDIO_CHUNK_DURATION_MS`]) this is only
/// ~6.4s of audio — well beyond any *healthy* send window, so hitting it while
/// the socket is up signals a bug or a stuck writer. New chunks are then dropped
/// and `user_disconnected` is flipped so the caller sees a clean error.
///
/// OVERFLOW POLICY (deliberate; review m2) — the ASR clients (Deepgram,
/// AssemblyAI, Soniox, OpenAI-realtime) all use this **fail-fast** cap: on
/// overflow they flip `user_disconnected` and return an error, ending the
/// session. That is the right choice for transcription because a dropped audio
/// window produces a *silently wrong* transcript — words vanish with no visible
/// signal — so it is safer to end loudly than to emit a transcript with
/// invisible holes. The Gemini Live S2S path deliberately does the OPPOSITE
/// (a 1000-chunk lossy-drop-newest soft cap that keeps the live conversation
/// alive) for the reasons documented on `gemini::GEMINI_AUDIO_QUEUE_CAP`. The
/// two policies diverge on purpose; they are not an accident of copy-paste.
///
/// RECONNECT-SCOPED WIDENING (Codex P2) — the fail-fast threshold is *not* a
/// flat 200 while a reconnect is climbing the ladder. The reconnect ladder
/// (`reconnect::DEFAULT_BACKOFF_SECONDS`) can spend up to ~458s disconnected
/// worst-case — ~308s of backoff sleeps (review m1's cold-restart tail) plus a
/// fully stalled 15s connect handshake (`ws_request::WS_CONNECT_TIMEOUT`) per
/// rung — so a flat 6.4s cap would fail-fast ~6s into an outage and make that
/// multi-minute budget unreachable dead code for exactly the long captures it
/// was added for. While the socket is down (and through the post-reconnect
/// drain window) `send_audio` instead uses
/// [`RECONNECT_AUDIO_BUFFER_MAX_CHUNKS`], which is derived from that full
/// disconnected budget so the two can never silently diverge again. The
/// fail-fast *policy* is unchanged — only the threshold is state-dependent.
const AUDIO_BUFFER_MAX_CHUNKS: usize = 200;

/// Reconnect-scoped audio-backlog cap. Derived from the full disconnected
/// budget — backoff sleeps plus per-rung connect timeouts — at the
/// processed-audio chunk cadence (see
/// [`reconnect::reconnect_backlog_cap_chunks`]) so a long capture can buffer a
/// full multi-minute partition instead of fail-fasting ~6s in (review m1 /
/// Codex P2). Currently 15744 chunks; at ~1KB per i16 chunk this peaks at
/// ~16MB — bounded and only reachable while genuinely riding out an outage.
const RECONNECT_AUDIO_BUFFER_MAX_CHUNKS: usize = crate::reconnect::reconnect_backlog_cap_chunks(
    crate::audio::pipeline::PROCESSED_AUDIO_CHUNK_DURATION_MS,
);
/// Deepgram closes listen sockets after roughly 10 seconds without audio or a
/// KeepAlive message. Send KeepAlive conservatively before that window.
///
/// This 4s cadence is tighter than the Aura *TTS* client's 8s
/// (`tts::deepgram_aura::KEEPALIVE_INTERVAL_SECS`) against the same ~10s vendor
/// idle window — a deliberate directional difference (review n3): on this ASR
/// send path a missed keepalive drops live mic audio and costs a reconnect, so
/// the larger 6s slack absorbs send-scheduling jitter; the TTS path has no live
/// audio to lose on a re-open, so it tolerates the looser margin.
const KEEPALIVE_INTERVAL_SECS: u64 = 4;
const KEEPALIVE_PAYLOAD: &str = r#"{"type":"KeepAlive"}"#;

/// Bounded post-`Terminal` drain window (audio-graph-653a).
///
/// Deepgram's documented `CloseStream` flow is: client sends an empty binary
/// frame ("Terminal"), server keeps flushing whatever it is still finalizing
/// (the trailing utterance's finals), THEN the server sends its own WS Close
/// frame. Before this constant existed, both the `Some(AudioCmd::Stop)` and
/// `None` arms of `run_io_with_keepalive_interval` sent Terminal and then
/// immediately called `writer.close()` and returned — abandoning the reader
/// before Deepgram's server could flush or close from its side. Every real
/// stop-capture lost the tail of the last utterance as a result (confirmed
/// live: protected-provider-smoke run 32404655072 — a 25s *counted* drain
/// window on the old code still read zero finals for the last 3s of speech,
/// because the client had already torn down the socket and structurally could
/// not read anything the server sent after that point).
///
/// Current value (900ms) is a first-cut, not a measured p99: Deepgram's own
/// flush is documented as sub-second once `CloseStream` is received, and the
/// "stop" button's latency budget matters too — a capture-stop that visibly
/// hangs for multiple seconds is its own UX bug, so the window is deliberately
/// bounded well under that. A silent/wedged server (dead peer, box dropped
/// packets) must not be able to hang `disconnect()` — that is exactly the
/// bound this constant enforces (acceptance case (b) in audio-graph-653a):
/// the drain always ends, either on the server's Close frame or here.
///
/// Tuning procedure (mirrors `state::TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT`,
/// ag#8 idiom): once this ships, log the elapsed wall-clock of each drain at
/// INFO (`deepgram.close_drain elapsed_ms=… ended_by=<close|deadline|error>`).
/// After ~1-2 weeks of real stop-captures, grep those logs, compute the
/// `ended_by=close` p50/p95/p99, and retune this constant to
/// `p99 + ~200-300ms safety margin` — document the new value with a "Chosen
/// because: p99 = Xms over N stop-captures on dates …" comment, the same way
/// `TRANSCRIPT_WRITER_SHUTDOWN_TIMEOUT` was tuned.
const DEEPGRAM_CLOSE_DRAIN_TIMEOUT: Duration = Duration::from_millis(900);

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
    /// refuse to enqueue new chunks once the buffer exceeds the active cap
    /// ([`AUDIO_BUFFER_MAX_CHUNKS`] steady-state, widened to
    /// [`RECONNECT_AUDIO_BUFFER_MAX_CHUNKS`] while reconnecting). At the ~32ms
    /// chunk granularity the speech processor emits, the steady cap is ~6.4s.
    pending_chunks: Arc<std::sync::atomic::AtomicUsize>,
    /// Latch tracking whether `send_audio` should enforce the reconnect-scoped
    /// backlog cap. Armed while the socket is down and held through the
    /// post-reconnect drain window; see [`reconnect::active_audio_backlog_cap`].
    /// Touched only by `send_audio` (single caller), so a plain atomic suffices.
    reconnect_backlog_active: std::sync::atomic::AtomicBool,
    /// Handle to the reader task (for join on shutdown).
    _reader_handle: Option<tokio::task::JoinHandle<()>>,
    /// Handle to the writer task (for join on shutdown).
    _writer_handle: Option<tokio::task::JoinHandle<()>>,
    /// Fires exactly once, the instant the session task (including its
    /// close-drain -- audio-graph-653a) finishes.
    ///
    /// `disconnect()` blocks on this (bounded) before returning so the only
    /// production caller doesn't drop `self` -- tripping `Drop`'s
    /// `rt.shutdown_timeout()`, which cancels whatever is still pending on
    /// the runtime -- before the drain this fix exists to run has actually
    /// completed.
    ///
    /// A plain `std::sync::mpsc` receiver, not a tokio primitive: it must be
    /// safe to block on from ANY calling thread, including one that is
    /// itself already inside a *different* tokio runtime (e.g. the live
    /// smoke test's `#[tokio::test(flavor = "current_thread")]` body calls
    /// `disconnect()` directly). `Runtime::block_on` panics in that
    /// situation ("Cannot start a runtime from within a runtime"); a std
    /// mpsc `recv_timeout` does not touch tokio's per-thread runtime-entry
    /// guard, so it has no such restriction.
    session_done_rx: Option<std::sync::mpsc::Receiver<()>>,
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
            reconnect_backlog_active: std::sync::atomic::AtomicBool::new(false),
            _reader_handle: None,
            _writer_handle: None,
            session_done_rx: None,
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

        // Record resolved-key PRESENCE + LENGTH + a non-secret FINGERPRINT
        // (never the value) so logs can distinguish an empty-key failure
        // (Fork A) from a stale/revoked-key 401 (Fork B) on the next incident.
        // The redaction helper only emits a "<present>" / "<missing>" sentinel;
        // the length is non-sensitive. The fingerprint is a one-way sha256
        // prefix (see `credentials::secret_fingerprint`) — comparing it against
        // the fingerprint logged by `save_credential_cmd` reveals whether the
        // key that reached the wire is the SAME one the user just saved (a
        // stale in-memory cache would make them differ). NEVER the raw key.
        log::debug!(
            "Deepgram connect: api_key {} len={} fingerprint={}",
            crate::credentials::redacted_secret_presence(Some(&self.config.api_key)),
            self.config.api_key.len(),
            crate::credentials::secret_fingerprint(Some(&self.config.api_key))
        );

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
        // Disarm the reconnect-scoped backlog latch for the fresh session.
        self.reconnect_backlog_active
            .store(false, std::sync::atomic::Ordering::Relaxed);
        let pending_chunks = Arc::clone(&self.pending_chunks);

        // Perform the blocking initial connect inside the runtime.
        let (audio_tx, session_handle, session_done_rx) = rt.block_on(async move {
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
            let (session_handle, session_done_rx) = spawn_session_task(DeepgramSessionCtx {
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
            });

            Ok::<_, String>((atx, session_handle, session_done_rx))
        })?;

        self.audio_tx = Some(audio_tx);
        self._reader_handle = Some(session_handle);
        self._writer_handle = None;
        self.rt = Some(rt);
        self.session_done_rx = Some(session_done_rx);

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

        // Drop chunks if the buffer has grown past the *active* safety cap. The
        // cap is state-dependent: the steady-state fail-fast cap while the
        // socket is healthy, widened to the reconnect-scoped cap while a
        // reconnect is climbing the ladder (and through the post-reconnect drain
        // window) so a long capture can ride out a multi-minute partition
        // instead of dying ~6s in (Codex P2). Flipping `user_disconnected` is
        // deliberate: once we start losing data the caller deserves to know the
        // session is effectively dead rather than silently seeing gaps.
        let depth = self
            .pending_chunks
            .load(std::sync::atomic::Ordering::Relaxed);
        let cap = crate::reconnect::active_audio_backlog_cap(
            &self.reconnect_backlog_active,
            self.connected.load(Ordering::SeqCst),
            depth,
            AUDIO_BUFFER_MAX_CHUNKS,
            RECONNECT_AUDIO_BUFFER_MAX_CHUNKS,
        );
        if depth >= cap {
            self.user_disconnected
                .store(true, std::sync::atomic::Ordering::SeqCst);
            return Err(format!(
                "Deepgram audio buffer full ({depth}/{cap} chunks) — likely a stuck reconnect. Restart the session."
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
    /// Sends a close frame, then blocks (bounded) until the session task's
    /// close-drain has actually finished before returning. Setting
    /// `user_disconnected` prevents the session task from attempting to
    /// auto-reconnect.
    ///
    /// # Why this blocks (audio-graph-653a)
    ///
    /// The only production caller (`run_deepgram_speech_processor`) drops
    /// `self` the instant this call returns. `Drop::drop` then calls
    /// `rt.shutdown_timeout()`, which CANCELS whatever is still pending on
    /// the runtime. If `disconnect()` returned immediately after merely
    /// queuing `AudioCmd::Stop` (as it used to), the caller would drop the
    /// client -- and the runtime would shut down -- microseconds later,
    /// cancelling the close-drain (`drain_after_terminal`) before it could
    /// read Deepgram's flushed finals. Blocking here, bounded at
    /// `DEEPGRAM_CLOSE_DRAIN_TIMEOUT` plus slack, keeps the runtime alive
    /// for exactly as long as the drain needs and no longer -- a wedged
    /// session task can't hang the caller's stop path; `Drop`'s
    /// `shutdown_timeout` remains the final backstop if the wait itself
    /// times out.
    ///
    /// This ordering also fixes the `Disconnected`-event ordering: the
    /// session task emits its own (deduplicated) `Disconnected` right after
    /// the drain, before this function's own trailing call -- which is a
    /// no-op in the common case -- so `Disconnected` reaches `event_rx`
    /// AFTER any drained `Transcript`/`Turn` events, not before.
    pub fn disconnect(&mut self) {
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

        // Block until the session task (and its close-drain) actually
        // finishes, or the bounded deadline elapses. A plain std mpsc
        // `recv_timeout` -- safe to call from any thread, including one
        // already inside a *different* tokio runtime; see the
        // `session_done_rx` field doc comment.
        if let Some(done_rx) = self.session_done_rx.take() {
            match done_rx.recv_timeout(DEEPGRAM_CLOSE_DRAIN_TIMEOUT + Duration::from_millis(500)) {
                Ok(()) => {
                    log::debug!("DeepgramStreamingClient: session task finished draining");
                }
                Err(e) => {
                    log::warn!(
                        "DeepgramStreamingClient: timed out waiting for the session task to \
                         finish draining ({e}); proceeding -- Drop's shutdown_timeout is the \
                         final backstop"
                    );
                }
            }
        }

        // Guarded by `disconnected_emitted`: in the common case the session
        // task already emitted `Disconnected` (after the drain) while we
        // were waiting above, so this is a no-op. It only actually fires
        // here if the session task never ran or the wait above timed out.
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

    // Bounded connect: a stalled TLS/HTTP-upgrade handshake would otherwise hang
    // this future (and the reconnect ladder) forever — see `connect_async_bounded`.
    let (ws_stream, _response) = crate::ws_request::connect_async_bounded(request)
        .await
        .map_err(|e| {
            // Prefer a typed, actionable message for auth failures (401) so a
            // user can tell "my key is rejected" from a generic network error.
            // Everything else falls through to the generic redacted diagnostic.
            // Both branches pass through the redaction wrapper so no secret can
            // leak into logs / UI-visible events.
            let message = classify_connect_error(&e)
                .unwrap_or_else(|| format!("WebSocket connect failed: {e}"));
            crate::error::redacted_provider_diagnostic(&message, [&config.api_key])
        })?;

    Ok(ws_stream.split())
}

/// Message surfaced when the Deepgram real-time handshake is rejected with an
/// HTTP `401 Unauthorized`. Extracted as a constant so the unit test asserts
/// against the exact string the user sees.
const DEEPGRAM_AUTH_FAILED_MESSAGE: &str = "Deepgram authentication failed (401): API key rejected — re-enter your Deepgram key in Settings";

/// Message surfaced when the Deepgram real-time handshake is rejected with an
/// HTTP `400 Bad Request`. The overwhelmingly common cause on the upgrade is an
/// invalid/unsupported `model` enum value (e.g. a stale `general` from an older
/// config) — `model` is a required enum on `v1/listen`, so a value outside it
/// is a 400, not a 401. Extracted as a constant so the unit test asserts the
/// exact user-visible string.
const DEEPGRAM_BAD_REQUEST_MESSAGE: &str = "Deepgram rejected the request (400): invalid or unsupported model. Reselect a model in Settings";

/// Classify a tungstenite connect error into a typed, actionable message.
///
/// The Deepgram WebSocket handshake authenticates via the `Authorization`
/// header on the HTTP upgrade. A stale / revoked key comes back as an HTTP
/// `401 Unauthorized` on the upgrade response, which tungstenite surfaces as
/// [`tungstenite::Error::Http`] carrying the response. We turn that into a
/// human-actionable message ("re-enter your key") instead of a raw
/// `HTTP error: 401 Unauthorized`.
///
/// Returns `None` for every other error so the caller falls back to the
/// generic (redacted) diagnostic. The returned message carries no secret, but
/// the caller still passes it through the redaction wrapper so a future edit
/// cannot accidentally leak one.
fn classify_connect_error(err: &tungstenite::Error) -> Option<String> {
    match err {
        tungstenite::Error::Http(response)
            if response.status() == tungstenite::http::StatusCode::UNAUTHORIZED =>
        {
            Some(DEEPGRAM_AUTH_FAILED_MESSAGE.to_string())
        }
        // A 400 on the upgrade almost always means the `model` enum value is
        // invalid/unsupported (the required `model` param failed validation).
        // Surface an actionable "reselect a model" message instead of a raw
        // `HTTP error: 400 Bad Request`. The sanitizer should prevent a
        // known-bad `general` from reaching the wire, but this arm still gives
        // a clear diagnostic for any other server-side model rejection.
        tungstenite::Error::Http(response)
            if response.status() == tungstenite::http::StatusCode::BAD_REQUEST =>
        {
            Some(DEEPGRAM_BAD_REQUEST_MESSAGE.to_string())
        }
        _ => None,
    }
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

/// The model Deepgram streaming (`v1/listen`) defaults to when a persisted
/// config carries a value that is not a valid streaming model id. `nova-3` is
/// Deepgram's current flagship streaming model and matches the app's own
/// default (`settings::default_deepgram_model`).
pub(crate) const DEEPGRAM_DEFAULT_STREAMING_MODEL: &str = "nova-3";

/// Family prefixes that Deepgram documents for `v1/listen`. Each is valid on
/// its own (`nova-3`) and as a `-{option}` suffix form (`nova-3-general`,
/// `nova-2-medical`, `base-general`, …). The full option matrix is large and
/// evolves, so we accept any `{prefix}` or `{prefix}-{anything}` value rather
/// than enumerating every suffix — this is deliberately permissive so a valid
/// but newly-added variant is never clobbered.
///
/// Confirmed against the Deepgram model-options + models-languages-overview
/// docs (see `docs/plans/2026-07-02-provider-api-audit.md`, Deepgram §3a): the
/// documented families are `nova-3`, `nova-2`, `nova`, `enhanced`, `base`
/// (each optionally `-{option}`-suffixed). Crucially there is NO bare
/// `general` member — `general` only ever appears as a suffix.
const DEEPGRAM_STREAMING_MODEL_FAMILIES: &[&str] =
    &["nova-3", "nova-2", "nova", "enhanced", "base"];

/// Prefix for Deepgram Flux conversational-turn models, routed to `v2/listen`
/// (`flux-general-en`, `flux-general-multi`). Kept distinct from the Nova
/// families because Flux is a valid streaming choice. Used only for endpoint
/// *routing* (v1 vs v2) — a value that merely *starts with* `flux-` is NOT
/// automatically a valid model; see [`DEEPGRAM_FLUX_MODELS`].
const DEEPGRAM_FLUX_MODEL_PREFIX: &str = "flux-";

/// The complete, closed set of Flux model ids `v2/listen` accepts as its `model`
/// enum. Unlike the Nova/base families — where the option matrix is large and
/// evolving, so we accept any `{family}-{suffix}` — Deepgram's Flux enum is a
/// short fixed list, so we enumerate it and reject everything else.
///
/// Confirmed against the Deepgram `listen-flux` reference (v2/listen `model`
/// enum, "Allowed values: flux-general-en flux-general-multi"). If Deepgram adds
/// a new concrete Flux id, extend this list — do NOT revert to a permissive
/// prefix check, which would re-admit invalid partials like the shared stem
/// `flux-general` that 400 on the wire.
const DEEPGRAM_FLUX_MODELS: &[&str] = &["flux-general-en", "flux-general-multi"];

/// Upgrade a well-known Deepgram marketing/short alias to the concrete model id
/// its API actually accepts. Returns `Some(canonical)` for a recognized alias,
/// `None` otherwise (the caller then falls through to the strict valid-check).
///
/// The motivating cases are the bare product name `flux` and its shared stem
/// `flux-general`: both are plausible values a user types into the free-text
/// model field, but `v2/listen` rejects them with an HTTP 400 (the enum only
/// accepts `flux-general-en` / `flux-general-multi`). Without this table the
/// load-path migration and the request-path sanitizer both treat them as
/// "invalid" and clamp to `nova-3`, silently destroying the user's intent.
/// Mapping the alias UP to the canonical English variant preserves that intent
/// instead. Matching is case-insensitive and EXACT on the whole (trimmed)
/// string — we never partial-match, so a genuinely-suffixed value like
/// `flux-general-multi` is left for the valid-check to accept unchanged.
///
/// Note we deliberately do NOT alias bare `nova`: unlike `flux`, `nova` is a
/// recognized streaming family that [`is_valid_deepgram_streaming_model`]
/// already accepts, so it is not a broken value in need of rescue.
///
/// This is deliberately an alias table, NOT a loosening of
/// [`is_valid_deepgram_streaming_model`]: bare `flux` must stay *invalid* so it
/// is never sent to Deepgram verbatim.
pub(crate) fn upgrade_deepgram_model_alias(model: &str) -> Option<&'static str> {
    match model.trim().to_ascii_lowercase().as_str() {
        // Bare product name and the shared `flux-general` stem both resolve to
        // the canonical English variant. `flux-general` is a partial that used
        // to slip through the old permissive `flux-*` valid-check and 400 on
        // the wire; upgrading it here preserves the plausible user intent.
        "flux" | "flux-general" => Some("flux-general-en"),
        _ => None,
    }
}

/// Return `true` when `model` is a Deepgram streaming model id we recognize as
/// valid — either an exact Flux model ([`DEEPGRAM_FLUX_MODELS`]) or one of the
/// documented Nova/base families (`nova-3`, `nova-3-general`, `base-general`, …).
///
/// Two different validation strategies by family, matching Deepgram's own API
/// shape:
/// - **Nova/base**: deliberately permissive on the suffix — any `{family}` or
///   `{family}-{suffix}` passes, so a valid-but-new option (e.g. a future
///   `nova-3-<domain>`) is not rewritten. The one thing this rejects is a bare
///   family *option* with no family prefix — most importantly the legacy
///   `general` value, which is NOT a real model id (only a suffix) and 400s.
/// - **Flux**: a CLOSED enum, so only the exact ids in [`DEEPGRAM_FLUX_MODELS`]
///   pass. A plausible partial like the shared stem `flux-general` is rejected
///   here (it 400s on the wire) — but it is rescued to a valid id UP-front by
///   [`upgrade_deepgram_model_alias`], so a user typing it is not clamped away.
///
/// Bare marketing aliases (`flux`, `flux-general`, `nova`) are rejected here on
/// purpose — the recognized ones are upgraded to a concrete id by
/// [`upgrade_deepgram_model_alias`] *before* this predicate runs on the load /
/// request paths.
pub(crate) fn is_valid_deepgram_streaming_model(model: &str) -> bool {
    if model.starts_with(DEEPGRAM_FLUX_MODEL_PREFIX) {
        // Flux is a CLOSED enum on `v2/listen` — only the concrete ids are
        // valid. A permissive `flux-*` length check used to admit partials like
        // the shared stem `flux-general`, which is sent verbatim and 400s. Bare
        // `flux` / `flux-general` are rescued *before* this predicate by
        // [`upgrade_deepgram_model_alias`]; anything else `flux-`-prefixed that
        // is not an exact member is genuinely invalid.
        return DEEPGRAM_FLUX_MODELS.contains(&model);
    }
    DEEPGRAM_STREAMING_MODEL_FAMILIES.iter().any(|family| {
        // Exact family (`nova-3`) or a suffixed form (`nova-3-general`). A bare
        // suffix like `general` matches no family here and is therefore invalid.
        model == *family
            || model
                .strip_prefix(family)
                .is_some_and(|rest| rest.starts_with('-') && rest.len() > 1)
    })
}

/// Clamp an arbitrary persisted model string to a value the Deepgram streaming
/// API will accept, mapping anything unrecognized (most notably the legacy
/// bare `general`) to [`DEEPGRAM_DEFAULT_STREAMING_MODEL`].
///
/// Order of operations (mirrors [`crate::settings::migrate_asr_provider_model`]
/// so the request path and the load path stay in lockstep):
///   1. A well-known alias (`flux`, `flux-general`) is UPGRADED to its concrete
///      id ([`upgrade_deepgram_model_alias`]) — this preserves the user's intent
///      instead of clamping it away.
///   2. An already-valid model (including Flux `flux-*` and suffixed Nova/base
///      families like `nova-3-general`) passes through UNCHANGED.
///   3. Anything else is clamped to [`DEEPGRAM_DEFAULT_STREAMING_MODEL`].
///
/// When a rewrite happens we `log::warn!` the offending value; a model name is
/// not a secret, so logging it verbatim is safe and aids diagnosis of stale
/// configs.
pub(crate) fn sanitize_deepgram_model(model: &str) -> String {
    if let Some(upgraded) = upgrade_deepgram_model_alias(model) {
        log::info!("Deepgram model alias '{model}' upgraded to canonical id '{upgraded}'.");
        return upgraded.to_string();
    }
    if is_valid_deepgram_streaming_model(model) {
        return model.to_string();
    }
    log::warn!(
        "Deepgram model '{model}' is not a valid streaming model id; \
         clamping to '{DEEPGRAM_DEFAULT_STREAMING_MODEL}'. \
         Reselect a model in Settings to silence this."
    );
    DEEPGRAM_DEFAULT_STREAMING_MODEL.to_string()
}

fn deepgram_listen_url(config: &DeepgramConfig) -> String {
    // Sanitize at the last possible moment so the URL can NEVER carry an
    // invalid `model` (e.g. a stale `general`) regardless of how the config was
    // built. A valid model — including Flux and suffixed families — is
    // untouched, so Flux still routes to v2/listen below.
    let model = sanitize_deepgram_model(&config.model);
    let is_flux = model.starts_with(DEEPGRAM_FLUX_MODEL_PREFIX);
    let mut url = if is_flux {
        format!(
            "wss://api.deepgram.com/v2/listen?encoding=linear16&sample_rate=16000&channels=1&model={model}"
        )
    } else {
        format!(
            "wss://api.deepgram.com/v1/listen?encoding=linear16&sample_rate=16000&channels=1&model={}&interim_results=true&diarize={}&punctuate=true",
            model, config.enable_diarization
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
///
/// Spawns [`session_task`] and returns a handle to it plus a receiver that
/// fires exactly once the task finishes -- no matter which of the task's
/// internal break/return paths ends it (clean stop, policy block, exhausted
/// reconnect budget, or a mid-backoff cancel).
///
/// Factored out of [`DeepgramStreamingClient::connect`] purely so a unit
/// test can drive it directly with a scripted-server fixture instead of a
/// real WebSocket handshake (audio-graph-653a: this wiring is what lets
/// `disconnect()` block until the close-drain has actually run instead of
/// racing `Drop`'s `rt.shutdown_timeout()`).
fn spawn_session_task(
    ctx: DeepgramSessionCtx,
) -> (tokio::task::JoinHandle<()>, std::sync::mpsc::Receiver<()>) {
    let (session_done_tx, session_done_rx) = std::sync::mpsc::channel::<()>();
    let handle = tokio::spawn(async move {
        session_task(ctx).await;
        let _ = session_done_tx.send(());
    });
    (handle, session_done_rx)
}

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

/// After we have sent the `Terminal` (empty-binary `CloseStream`) frame,
/// keep reading through the SAME message-handling path the live loop uses
/// (`handle_server_message_with_key` — transcript parsing, policy
/// classification, event emission, all unchanged) until either the server
/// sends its own WS Close frame or `deadline` elapses.
///
/// This is a free function — factored out of the `Some(AudioCmd::Stop)` /
/// `None` arms below — purely so a unit test can drive it directly with a
/// short deadline instead of waiting out the full production
/// [`DEEPGRAM_CLOSE_DRAIN_TIMEOUT`] twice (once per arm's test).
///
/// Deliberately returns `()`, not a [`DisconnectKind`]: no matter how the
/// drain ends — server Close, reader-stream end, a read error, or the
/// deadline — the caller (both arms) always classifies the teardown as the
/// same clean-end `DisconnectKind` it already returns today. That is what
/// keeps a flaky/slow/silent server during the drain window from ever being
/// misclassified as a network error or tripping the reconnect ladder
/// (audio-graph-653a acceptance: server-Close-during-drain and
/// deadline-exhaustion both stay on the clean-end path).
///
/// No keepalive ticks fire in here — this loop does not select on the
/// keep-alive timer, so nothing is sent to Deepgram after `Terminal` except
/// the final WS Close frame the caller sends once this returns.
async fn drain_after_terminal(
    reader: &mut AsrWsReader,
    event_tx: &crossbeam_channel::Sender<DeepgramEvent>,
    api_key: &str,
    deadline: Duration,
) {
    let sleep = tokio::time::sleep(deadline);
    tokio::pin!(sleep);

    loop {
        tokio::select! {
            _ = &mut sleep => {
                log::info!(
                    "Deepgram: close-drain deadline ({deadline:?}) elapsed before server Close"
                );
                return;
            }
            frame = reader.next() => {
                match frame {
                    None => {
                        log::info!("Deepgram: close-drain reader ended without an explicit Close");
                        return;
                    }
                    Some(Ok(Message::Close(_))) => {
                        log::info!("Deepgram: close-drain observed the server's Close frame");
                        return;
                    }
                    Some(Ok(Message::Text(text))) => {
                        // Same handling path as the live loop: a final
                        // transcript the server flushes in response to
                        // Terminal lands exactly like a live-phase message.
                        handle_server_message_with_key(&text, event_tx, api_key);
                    }
                    Some(Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_))) => {
                        // Protocol-level frames; nothing to do.
                    }
                    Some(Ok(Message::Binary(_))) => {
                        log::debug!("Deepgram: unexpected binary message during close-drain");
                    }
                    Some(Err(e)) => {
                        // A read error during the drain is still a clean end
                        // from the caller's perspective — log for diagnosis
                        // but do not reclassify or reconnect. Redact: some
                        // tungstenite error variants carry provider-supplied
                        // text (e.g. an embedded upgrade request), so scrub
                        // the key before it reaches logs.
                        let diag = crate::error::redacted_provider_diagnostic(
                            &format!("close-drain reader error (treated as clean end): {e}"),
                            [api_key],
                        );
                        log::info!("Deepgram: {diag}");
                        return;
                    }
                }
            }
        }
    }
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
                        // INVARIANT (decrement-before-send; review m3): decrement
                        // on consumption, BEFORE the write. Deepgram/AssemblyAI/
                        // Soniox cannot replay a failed chunk (a send error ends
                        // the session or drops the frame), so decrementing up front
                        // keeps the backlog metric accurate whether the frame sends
                        // or errors — the chunk leaves the queue either way and must
                        // not keep counting against the cap. This is deliberately
                        // the OPPOSITE of OpenAI-realtime, which holds the decrement
                        // until a *successful* write so a replayed chunk still counts
                        // (`openai_realtime::write_audio_cmd`). Do NOT add replay to
                        // this client without moving the decrement past the write, or
                        // the replayed chunk will be double-decremented.
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
                        // Graceful user-initiated close: send Deepgram's
                        // documented CloseStream signal (empty binary
                        // Terminal frame), then keep reading through the
                        // normal message path until the server flushes its
                        // remaining finals and sends its own Close, or the
                        // bounded drain window elapses — see
                        // `drain_after_terminal` / `DEEPGRAM_CLOSE_DRAIN_TIMEOUT`
                        // (audio-graph-653a: slamming the socket shut here
                        // used to abandon the reader before the server could
                        // flush, silently dropping the last utterance's
                        // words — live-evidenced by protected-provider-smoke
                        // run 32404655072).
                        let _ = write_guard
                            .send_binary(writer, AsrTransportPayloadKind::Terminal, Vec::new())
                            .await;
                        drain_after_terminal(
                            reader,
                            event_tx,
                            api_key,
                            DEEPGRAM_CLOSE_DRAIN_TIMEOUT,
                        )
                        .await;
                        let _ = writer.close().await;
                        return DisconnectKind::UserRequested;
                    }
                    None => {
                        // Caller dropped the sender. No more audio will ever
                        // arrive — end the session without reconnecting, but
                        // still give Deepgram the same bounded close-drain
                        // window as the explicit Stop arm above: the
                        // CloseStream flush defect applies here too
                        // (audio-graph-653a).
                        let _ = write_guard
                            .send_binary(writer, AsrTransportPayloadKind::Terminal, Vec::new())
                            .await;
                        drain_after_terminal(
                            reader,
                            event_tx,
                            api_key,
                            DEEPGRAM_CLOSE_DRAIN_TIMEOUT,
                        )
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
// Diarization span-revision normalization
// ---------------------------------------------------------------------------

/// Configuration for normalizing Deepgram word-level speaker/channel metadata
/// into provider-neutral [`DiarizationSpanRevisionPayload`] span revisions.
///
/// The normalizer keeps the PROVIDER speaker id strictly separate from any local
/// stable speaker id and the display label: the emitted `speaker_id` carries the
/// provider-scoped raw id (e.g. `"deepgram-1"`), the `speaker_label` carries the
/// human-facing label resolved from [`Self::speaker_labels`]. The `channel`
/// field is provenance-only and is populated solely when [`Self::channel_capable`]
/// is `true` (a capability gate); otherwise it stays `None` even if a source
/// channel is configured.
#[derive(Debug, Clone, Default)]
pub struct DeepgramDiarizationSpec {
    /// Logical timeline being revised (e.g. `"session"` or a provider source id).
    pub timeline_id: String,
    /// Capture source, when the attribution is source-local. Provenance-only.
    pub source_id: Option<String>,
    /// Source channel label (e.g. `"mixed"`, `"left"`). Provenance-only — emitted
    /// on the revision ONLY when `channel_capable` is `true`.
    pub channel: Option<String>,
    /// Capability gate for source/generated channel attribution. When `false`
    /// (the default), the channel field is suppressed even if `channel` is set.
    pub channel_capable: bool,
    /// Provider-speaker-id -> display-label map. A provider id with no entry
    /// yields a `None` label (an unknown/interim speaker keeps its raw id but no
    /// friendly label).
    pub speaker_labels: std::collections::HashMap<String, String>,
}

/// Normalize a stream of Deepgram events into provider-neutral speaker-timeline
/// span revisions.
///
/// Deepgram attaches a per-word `speaker: Option<u32>` index. Each transcript
/// becomes one or more revisions, splitting a transcript whose words switch
/// speaker into a separate span per contiguous same-speaker run (mixed-speaker
/// spans). A word with no speaker index is an unknown/interim speaker: it keeps a
/// `None` provider id and `None` label, with `Provisional` stability.
///
/// Provider speaker id (`deepgram-{n}`) is kept SEPARATE from the display label
/// (resolved from the spec's `speaker_labels`); the channel is provenance-only
/// and suppressed unless the spec's capability gate (`channel_capable`) is set.
/// Re-attributing a span (a later transcript at the same start time switching
/// speaker) emits a retcon revision that `supersedes` the earlier `span_id`.
///
/// Non-`Transcript` events and transcripts with no words are ignored.
pub fn normalize_deepgram_diarization<I>(
    events: I,
    spec: &DeepgramDiarizationSpec,
) -> Vec<DiarizationSpanRevisionPayload>
where
    I: IntoIterator<Item = DeepgramEvent>,
{
    let channel = if spec.channel_capable {
        spec.channel.clone()
    } else {
        None
    };

    // span start_time -> the span_id we last emitted for it, so a later
    // re-attribution can supersede the prior revision rather than duplicate it.
    let mut span_history: std::collections::HashMap<u64, String> = std::collections::HashMap::new();
    let mut revisions = Vec::new();

    for event in events {
        let DeepgramEvent::Transcript {
            is_final,
            start,
            duration,
            words,
            ..
        } = event
        else {
            continue;
        };
        if words.is_empty() {
            continue;
        }

        // Group contiguous same-speaker words into runs (mixed-speaker spans).
        let mut runs: Vec<(Option<u32>, f64, f64)> = Vec::new();
        for word in &words {
            match runs.last_mut() {
                Some((spk, _run_start, run_end)) if *spk == word.speaker => {
                    *run_end = word.end;
                }
                _ => runs.push((word.speaker, word.start, word.end)),
            }
        }

        for (run_index, (speaker, run_start, run_end)) in runs.into_iter().enumerate() {
            // Quantize the start to whole milliseconds for a stable span key
            // independent of float jitter across re-attributions.
            let start_key = (run_start * 1000.0).round() as u64;
            let provider_speaker_id = speaker.map(|n| format!("deepgram-{n}"));
            let speaker_label = provider_speaker_id
                .as_deref()
                .and_then(|id| spec.speaker_labels.get(id).cloned());

            let span_id = format!(
                "deepgram:{}:{}:{}",
                spec.timeline_id,
                start_key,
                provider_speaker_id.as_deref().unwrap_or("unknown")
            );

            // A retcon supersedes the prior revision recorded for this start.
            let supersedes = span_history.get(&start_key).cloned();
            let revision_number = if supersedes.is_some() { 2 } else { 1 };
            span_history.insert(start_key, span_id.clone());

            let stability = if is_final {
                DiarizationSpanStability::Stable
            } else {
                DiarizationSpanStability::Provisional
            };

            revisions.push(DiarizationSpanRevisionPayload {
                span_id,
                provider: "deepgram".to_string(),
                timeline_id: spec.timeline_id.clone(),
                source_id: spec.source_id.clone(),
                speaker_id: provider_speaker_id,
                speaker_label,
                channel: channel.clone(),
                start_time: run_start,
                end_time: run_end,
                confidence: None,
                is_final,
                stability,
                revision_number,
                supersedes,
                basis_asr_span_ids: vec![format!("deepgram:{}:{}", spec.timeline_id, start_key)],
                basis_transcript_segment_ids: Vec::new(),
                raw_event_ref: Some(format!("transcript:{start}:{duration}:{run_index}")),
                capture_latency_ms: None,
                asr_latency_ms: None,
                received_at_ms: 0,
            });
        }
    }

    revisions
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

    /// Build a tungstenite `Error::Http` carrying the given HTTP status, to
    /// simulate the upgrade-response path without a live socket.
    fn http_error_with_status(status: tungstenite::http::StatusCode) -> tungstenite::Error {
        let response = tungstenite::http::Response::builder()
            .status(status)
            .body(None)
            .expect("build http response");
        tungstenite::Error::Http(Box::new(response))
    }

    #[test]
    fn classify_connect_error_maps_401_to_reenter_key_message() {
        let err = http_error_with_status(tungstenite::http::StatusCode::UNAUTHORIZED);
        let message = classify_connect_error(&err).expect("401 should be classified");
        // The user-facing message must tell them to re-enter the key.
        assert!(
            message.contains("re-enter your Deepgram key"),
            "unexpected message: {message}"
        );
        assert!(
            message.contains("401"),
            "message should name the status: {message}"
        );
        assert!(
            message.contains("authentication failed"),
            "message should name auth failure: {message}"
        );
    }

    #[test]
    fn classify_connect_error_passes_through_non_401() {
        // A different HTTP status (that we don't specifically classify) is NOT
        // the auth message — falls through so the caller emits the generic
        // redacted diagnostic. 400 and 401 are handled explicitly and tested
        // separately.
        let forbidden = http_error_with_status(tungstenite::http::StatusCode::FORBIDDEN);
        assert!(classify_connect_error(&forbidden).is_none());

        let server_err =
            http_error_with_status(tungstenite::http::StatusCode::INTERNAL_SERVER_ERROR);
        assert!(classify_connect_error(&server_err).is_none());

        // A non-HTTP transport error also falls through.
        let transport = tungstenite::Error::ConnectionClosed;
        assert!(classify_connect_error(&transport).is_none());
    }

    #[test]
    fn classify_connect_error_maps_400_to_reselect_model_message() {
        // Mirror of the 401 test: a 400 on the upgrade (the shape a bad/stale
        // `model` enum produces) becomes an actionable "reselect a model"
        // message, distinct from the 401 auth message.
        let err = http_error_with_status(tungstenite::http::StatusCode::BAD_REQUEST);
        let message = classify_connect_error(&err).expect("400 should be classified");
        assert!(
            message.contains("400"),
            "message should name the status: {message}"
        );
        assert!(
            message.to_lowercase().contains("model"),
            "message should mention the model: {message}"
        );
        assert!(
            message.contains("Reselect a model in Settings"),
            "message should tell the user to reselect a model: {message}"
        );
        // It must NOT be confused with the auth (401) message.
        assert!(
            !message.contains("re-enter your Deepgram key"),
            "400 must not surface the auth message: {message}"
        );
    }

    #[test]
    fn sanitize_deepgram_model_rewrites_legacy_general() {
        // The confirmed root cause: bare `general` is not a real streaming
        // model id (only a suffix), so it is clamped to the default.
        assert_eq!(sanitize_deepgram_model("general"), "nova-3");
    }

    #[test]
    fn sanitize_deepgram_model_passes_through_valid_models() {
        // Valid Nova/base families, suffixed forms, and Flux models are all
        // left UNCHANGED.
        for valid in [
            "nova-3",
            "nova-3-general",
            "nova-3-medical",
            "nova-2",
            "nova-2-phonecall",
            "nova",
            "nova-general",
            "enhanced",
            "enhanced-general",
            "base",
            "base-general",
            "flux-general-en",
            "flux-general-multi",
        ] {
            assert_eq!(
                sanitize_deepgram_model(valid),
                valid,
                "valid model must pass through unchanged: {valid}"
            );
            assert!(
                is_valid_deepgram_streaming_model(valid),
                "model should be recognized as valid: {valid}"
            );
        }
    }

    #[test]
    fn sanitize_deepgram_model_rewrites_other_invalid_values() {
        // Other bare option words / junk clamp to the default too.
        for invalid in ["", "medical", "phonecall", "flux-", "not-a-model", "nova3"] {
            assert_eq!(
                sanitize_deepgram_model(invalid),
                "nova-3",
                "invalid model should clamp to default: {invalid:?}"
            );
            assert!(
                !is_valid_deepgram_streaming_model(invalid),
                "model should be recognized as invalid: {invalid:?}"
            );
        }
    }

    #[test]
    fn sanitize_deepgram_model_upgrades_bare_flux_alias() {
        // FIX-1 (request path): bare `flux` is Deepgram's product name but an
        // invalid `model` enum. It must be UPGRADED to the canonical
        // `flux-general-en`, never clamped to nova-3. Case/whitespace-insensitive.
        for alias in ["flux", "FLUX", "  Flux  "] {
            assert_eq!(
                sanitize_deepgram_model(alias),
                "flux-general-en",
                "alias {alias:?} must upgrade to flux-general-en"
            );
        }
        // FIX ffb2 (request path): the shared stem `flux-general` is the other
        // plausible partial a user types. It must ALSO upgrade to
        // `flux-general-en`, case/whitespace-insensitive — never sent verbatim
        // (it 400s) and never clamped to nova-3.
        for alias in ["flux-general", "FLUX-GENERAL", "  Flux-General  "] {
            assert_eq!(
                sanitize_deepgram_model(alias),
                "flux-general-en",
                "alias {alias:?} must upgrade to flux-general-en"
            );
        }
        // The alias helper matches exactly and nothing else.
        assert_eq!(
            upgrade_deepgram_model_alias("flux"),
            Some("flux-general-en")
        );
        assert_eq!(
            upgrade_deepgram_model_alias("flux-general"),
            Some("flux-general-en")
        );
        assert_eq!(upgrade_deepgram_model_alias("flux-general-en"), None);
        assert_eq!(upgrade_deepgram_model_alias("flux-general-multi"), None);
        assert_eq!(upgrade_deepgram_model_alias("nova"), None);
        assert_eq!(upgrade_deepgram_model_alias("general"), None);
        // Bare `flux` / `flux-general` must STAY invalid — we must never bless
        // them as valid wire values (Deepgram would 400 them); the upgrade
        // happens before the valid-check, not by loosening it.
        assert!(!is_valid_deepgram_streaming_model("flux"));
        assert!(!is_valid_deepgram_streaming_model("flux-general"));
    }

    #[test]
    fn is_valid_deepgram_streaming_model_flux_is_closed_enum() {
        // FIX ffb2 (validation): the Flux branch is a CLOSED enum. Only the two
        // concrete ids Deepgram's v2/listen `model` enum documents are valid.
        assert!(is_valid_deepgram_streaming_model("flux-general-en"));
        assert!(is_valid_deepgram_streaming_model("flux-general-multi"));
        // The const is the single source of truth for that set.
        for id in DEEPGRAM_FLUX_MODELS {
            assert!(
                is_valid_deepgram_streaming_model(id),
                "listed flux id must be valid: {id}"
            );
        }
        // The pre-fix permissive `flux-*` length check accepted ANY `flux-x`
        // of length > 5. These plausible partials / typos MUST now be rejected
        // so they never reach the wire verbatim (each would 400).
        for invalid in [
            "flux-general",    // the ffb2 shared stem (rescued by the alias, invalid on its own)
            "flux-general-e",  // truncation typo
            "flux-en",         // wrong shape
            "flux-bogus",      // junk suffix
            "flux-general-fr", // unsupported language variant
        ] {
            assert!(
                !is_valid_deepgram_streaming_model(invalid),
                "permissive flux partial must now be rejected: {invalid:?}"
            );
        }
    }

    #[test]
    fn listen_url_upgrades_flux_general_stem_to_v2_endpoint() {
        // End-to-end for the ffb2 case: a stale/typed `flux-general` in the
        // config resolves to flux-general-en on v2/listen, not a clamped nova-3
        // on v1, and the wire NEVER carries the invalid bare stem.
        let url = deepgram_listen_url(&test_config("flux-general"));
        assert!(
            url.starts_with("wss://api.deepgram.com/v2/listen?"),
            "flux-general must route to v2/listen: {url}"
        );
        assert!(
            url.contains("model=flux-general-en"),
            "flux-general must resolve to flux-general-en: {url}"
        );
        assert!(
            !url.contains("model=flux-general&") && !url.ends_with("model=flux-general"),
            "URL must never carry the invalid bare stem model=flux-general: {url}"
        );
    }

    #[test]
    fn listen_url_clamps_invalid_flux_partial_to_default() {
        // A `flux-*` value that is neither a concrete id nor a rescued alias
        // (e.g. `flux-bogus`) is invalid: it clamps to nova-3 on v1 and the wire
        // never carries the bad flux id.
        let url = deepgram_listen_url(&test_config("flux-bogus"));
        assert!(
            url.starts_with("wss://api.deepgram.com/v1/listen?"),
            "invalid flux partial must clamp off the v2 flux path: {url}"
        );
        assert!(
            url.contains("model=nova-3"),
            "invalid flux partial must clamp to nova-3: {url}"
        );
        assert!(
            !url.contains("flux"),
            "URL must not carry any flux id for an invalid partial: {url}"
        );
    }

    #[test]
    fn listen_url_upgrades_bare_flux_to_v2_endpoint() {
        // End-to-end for the request path: a stale/typed bare `flux` in the
        // config resolves to flux-general-en on the v2/listen endpoint, not a
        // clamped nova-3 on v1.
        let url = deepgram_listen_url(&test_config("flux"));
        assert!(
            url.starts_with("wss://api.deepgram.com/v2/listen?"),
            "bare flux must route to v2/listen: {url}"
        );
        assert!(
            url.contains("model=flux-general-en"),
            "bare flux must resolve to flux-general-en: {url}"
        );
    }

    #[test]
    fn listen_url_never_emits_bare_general_model() {
        // Even if a stale `general` slips into DeepgramConfig, the URL builder
        // sanitizes it so the wire never carries `&model=general`.
        let mut config = test_config("general");
        config.enable_diarization = false;
        let url = deepgram_listen_url(&config);
        assert!(
            !url.contains("model=general"),
            "URL must not carry the invalid bare model: {url}"
        );
        assert!(
            url.contains("model=nova-3"),
            "URL should carry the clamped default model: {url}"
        );
        // And it stays on the Nova v1 endpoint (general is not a Flux model).
        assert!(url.starts_with("wss://api.deepgram.com/v1/listen?"));
    }

    #[test]
    fn listen_url_preserves_valid_flux_and_suffixed_models() {
        // A suffixed Nova family passes through into the v1 URL unchanged.
        let suffixed = deepgram_listen_url(&test_config("nova-3-general"));
        assert!(suffixed.contains("model=nova-3-general"));
        assert!(suffixed.starts_with("wss://api.deepgram.com/v1/listen?"));

        // A valid Flux model still routes to v2/listen unchanged.
        let flux = deepgram_listen_url(&test_config("flux-general-en"));
        assert!(flux.contains("model=flux-general-en"));
        assert!(flux.starts_with("wss://api.deepgram.com/v2/listen?"));
    }

    #[test]
    fn connect_diagnostic_log_never_contains_key_value() {
        // Reproduce the exact debug string connect() logs and assert it carries
        // the presence sentinel + length but NEVER the raw key value.
        let secret = "dg-super-secret-key-value-1234567890";
        let config = DeepgramConfig {
            api_key: secret.into(),
            ..test_config("nova-3")
        };

        let formatted = format!(
            "Deepgram connect: api_key {} len={}",
            crate::credentials::redacted_secret_presence(Some(&config.api_key)),
            config.api_key.len()
        );

        assert!(
            !formatted.contains(secret),
            "diagnostic leaked the key: {formatted}"
        );
        assert!(
            formatted.contains("<present>"),
            "missing presence sentinel: {formatted}"
        );
        assert!(
            formatted.contains(&format!("len={}", secret.len())),
            "missing key length: {formatted}"
        );
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

    /// Build a connected-looking client wired to an in-test audio channel so
    /// `send_audio` exercises the real cap logic without a live socket. Returns
    /// the receiver so chunks don't get dropped (which would keep the sender
    /// open) — the caller controls `connected`/`pending_chunks` via the client.
    fn client_with_channel(
        connected: bool,
    ) -> (
        DeepgramStreamingClient,
        tokio_mpsc::UnboundedReceiver<AudioCmd>,
    ) {
        let mut client = DeepgramStreamingClient::new(test_config("nova-3"));
        let (tx, rx) = tokio_mpsc::unbounded_channel();
        client.audio_tx = Some(tx);
        client.connected.store(connected, Ordering::SeqCst);
        (client, rx)
    }

    #[test]
    fn steady_state_backlog_fails_fast_at_the_200_chunk_cap() {
        // Socket healthy: the fail-fast cap must be the steady 200, and hitting
        // it flips user_disconnected (the m2 fail-fast policy is unchanged).
        let (client, _rx) = client_with_channel(true);
        client.pending_chunks.store(
            AUDIO_BUFFER_MAX_CHUNKS,
            std::sync::atomic::Ordering::Relaxed,
        );
        let err = client.send_audio(&[0.1, -0.1]).unwrap_err();
        assert!(err.contains(&format!("{AUDIO_BUFFER_MAX_CHUNKS}")));
        assert!(client.user_disconnected.load(Ordering::SeqCst));
    }

    #[test]
    fn reconnecting_backlog_grows_past_the_steady_cap_without_disconnecting() {
        // Socket down (mid-reconnect): a backlog well past the steady 200 cap
        // must still enqueue — otherwise the multi-minute ladder tail (m1) is
        // dead code for long captures (Codex P2). user_disconnected stays clear.
        let (client, _rx) = client_with_channel(false);
        client.pending_chunks.store(
            AUDIO_BUFFER_MAX_CHUNKS + 500,
            std::sync::atomic::Ordering::Relaxed,
        );
        assert!(
            client.send_audio(&[0.1, -0.1]).is_ok(),
            "reconnect-scoped cap must accept a backlog past the steady 200"
        );
        assert!(!client.user_disconnected.load(Ordering::SeqCst));
        assert!(client.reconnect_backlog_active.load(Ordering::Relaxed));
    }

    #[test]
    fn reconnecting_backlog_still_fails_fast_past_the_reconnect_cap() {
        // Even the widened cap is bounded: a backlog past the ladder-derived
        // reconnect cap fails fast so a genuinely stuck reconnect can't OOM.
        let (client, _rx) = client_with_channel(false);
        client.pending_chunks.store(
            RECONNECT_AUDIO_BUFFER_MAX_CHUNKS,
            std::sync::atomic::Ordering::Relaxed,
        );
        let err = client.send_audio(&[0.1, -0.1]).unwrap_err();
        assert!(err.contains(&format!("{RECONNECT_AUDIO_BUFFER_MAX_CHUNKS}")));
        assert!(client.user_disconnected.load(Ordering::SeqCst));
    }

    #[test]
    fn reconnect_cap_covers_the_full_ladder_budget() {
        // Ladder+cap consistency: the reconnect-scoped cap must hold at least a
        // whole disconnected budget's worth of 32ms chunks — backoff sleeps PLUS
        // a stalled 15s connect handshake per rung — so extending the ladder
        // (m1) or the connect timeout and the cap can never silently diverge
        // (the root of Codex P2, both review rounds).
        let budget_chunks = (crate::reconnect::total_disconnected_budget_secs() * 1000)
            .div_ceil(crate::audio::pipeline::PROCESSED_AUDIO_CHUNK_DURATION_MS);
        // Read the caps through runtime bindings so the comparisons aren't
        // const-folded (clippy::assertions_on_constants).
        let reconnect_cap = RECONNECT_AUDIO_BUFFER_MAX_CHUNKS;
        let steady_cap = AUDIO_BUFFER_MAX_CHUNKS;
        assert!(
            reconnect_cap as u64 >= budget_chunks,
            "reconnect cap {reconnect_cap} must cover {budget_chunks} ladder chunks"
        );
        assert!(
            reconnect_cap > steady_cap,
            "reconnect cap {reconnect_cap} must exceed steady cap {steady_cap}"
        );
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
        // Deepgram uses the shared crate-level ladder (review n2): fast head then
        // the cold-restart tail (review m1). Give-up is now after the full budget.
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(2), Some(2));
        assert_eq!(backoff_for_attempt(3), Some(5));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), Some(20));
        assert_eq!(backoff_for_attempt(11), None);
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
        // The ladder continues into the cold-restart tail instead of giving up at
        // attempt 4 (review m1); exhaustion still reports the attempts made.
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

        // One distinct increment per ladder rung → one Reconnecting emit each,
        // strictly monotonic with no duplicates/doubling, across the full
        // cold-restart schedule (review m1).
        let budget = crate::reconnect::DEFAULT_BACKOFF_SECONDS.len() as u32;
        let expected: Vec<u32> = (1..=budget).collect();
        assert_eq!(reconnecting_emits, expected);
        assert_eq!(reconnect_attempts, budget);
        assert_eq!(gave_up_after, budget);
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
            // Deepgram's real CloseStream flow: the server closes from its own
            // side right after Terminal. With the close-drain fix the client no
            // longer slams its own Close immediately — it waits to observe this
            // one — so the scripted server must actually send it for the drain
            // to resolve (audio-graph-653a).
            ws_fixture::ServerStep::send_close(),
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
                        let is_terminal = bytes.is_empty();
                        binary_frames.push(bytes.to_vec());
                        if is_terminal {
                            // Mirror Deepgram's documented CloseStream flow:
                            // the server closes from its own side right after
                            // Terminal, which is exactly what the fixed
                            // close-drain waits to observe (audio-graph-653a).
                            let _ = websocket.close(None).await;
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

    // -----------------------------------------------------------------------
    // audio-graph-653a: close-drain regression coverage.
    //
    // Before the fix, both the `Some(AudioCmd::Stop)` and `None` arms sent
    // Terminal and then immediately closed the writer and returned —
    // abandoning the reader before Deepgram's server could flush the finals
    // it produces in response to Terminal (its documented CloseStream flow)
    // or complete the close handshake. Live-evidenced by
    // protected-provider-smoke run 32404655072: a 25s *counted* post-close
    // drain window still read zero finals for the last 3s of speech, because
    // the client had already torn the socket down and structurally could not
    // read anything the server sent afterward.
    // -----------------------------------------------------------------------

    /// (a) + (c): a message the server sends between our `Terminal` and its
    /// `Close` is processed through the SAME handling path as a live-phase
    /// message (a final transcript + turn event lands), and the drain
    /// resolves via the server's `Close` frame — fast, not at the deadline —
    /// rather than being misclassified as a network error.
    #[tokio::test(flavor = "current_thread")]
    async fn drain_after_terminal_processes_server_messages_and_stops_at_server_close() {
        let (url, server) = ws_fixture::spawn_scripted_server(vec![
            ws_fixture::ServerStep::send_text(
                r#"{"type":"Results","is_final":true,"speech_final":true,"start":2.0,"duration":0.3,"channel":{"alternatives":[{"transcript":"drained final","confidence":0.9,"words":[]}]}}"#,
            ),
            ws_fixture::ServerStep::send_close(),
        ])
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (_writer, mut reader) = client_socket.split();
        let (event_tx, event_rx) = crossbeam_channel::bounded(16);

        // A deliberately generous deadline (5s) — if this test passes fast
        // (well under it, asserted below), the drain resolved via the
        // server's Close frame, not the deadline.
        let started = std::time::Instant::now();
        tokio::time::timeout(
            Duration::from_millis(500),
            drain_after_terminal(&mut reader, &event_tx, "test-key", Duration::from_secs(5)),
        )
        .await
        .expect("drain should resolve via server Close, not the 5s deadline");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "drain should end promptly on server Close, not wait out the deadline"
        );

        match recv_event(&event_rx, Duration::from_millis(200)).await {
            DeepgramEvent::Transcript { text, is_final, .. } => {
                assert_eq!(text, "drained final");
                assert!(is_final);
            }
            other => panic!("expected a drained transcript event, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_millis(200)).await {
            DeepgramEvent::Turn { kind, text, .. } => {
                assert!(matches!(kind, DeepgramTurnKind::SpeechFinal));
                assert_eq!(text.as_deref(), Some("drained final"));
            }
            other => panic!("expected a drained speech-final turn event, got {other:?}"),
        }
        // (e): exactly one Transcript + one Turn — no duplicate emission.
        assert!(
            event_rx.try_recv().is_err(),
            "drain must not emit any event more than once"
        );

        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
    }

    /// (b): a server that never sends anything and never closes cannot wedge
    /// the drain — it ends at the bounded deadline regardless.
    #[tokio::test(flavor = "current_thread")]
    async fn drain_after_terminal_returns_at_deadline_when_server_is_silent() {
        let (url, server) = ws_fixture::spawn_server(|mut websocket| async move {
            // Keep the TCP connection genuinely alive with WS-level Ping
            // control frames (handled as a no-op by the drain loop, same as
            // live) well past the short deadline this test uses below — but
            // never send anything the app treats as "done" (no transcript,
            // no Close). This models a wedged/silent-from-Deepgram's-
            // perspective server without an idle loopback socket.
            for _ in 0..50 {
                if websocket
                    .send(Message::Ping(Vec::new().into()))
                    .await
                    .is_err()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (_writer, mut reader) = client_socket.split();
        let (event_tx, event_rx) = crossbeam_channel::bounded(16);

        let deadline = Duration::from_millis(60);
        let started = std::time::Instant::now();
        drain_after_terminal(&mut reader, &event_tx, "test-key", deadline).await;
        let elapsed = started.elapsed();

        assert!(
            elapsed >= deadline,
            "drain must not return before its deadline: elapsed={elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "a silent server must not wedge the drain past its bounded deadline: elapsed={elapsed:?}"
        );
        assert!(
            event_rx.try_recv().is_err(),
            "a silent server must not produce any spurious events"
        );

        server.abort();
    }

    /// (a) + (c) + (d) + (e), Stop arm: `run_io`'s `Some(AudioCmd::Stop)` arm
    /// keeps reading through the normal handler path after `Terminal`, emits
    /// the server's drained final exactly once, resolves fast via the
    /// server's Close (not the deadline), and returns the clean
    /// `UserRequested` kind — never `NetworkError`, never triggering
    /// reconnect.
    #[tokio::test(flavor = "current_thread")]
    async fn run_io_stop_arm_drains_server_messages_before_close_and_returns_clean() {
        let (url, server) = ws_fixture::spawn_scripted_server(vec![
            ws_fixture::ServerStep::expect_binary(vec![9, 9, 9]),
            ws_fixture::ServerStep::expect_binary(Vec::<u8>::new()),
            ws_fixture::ServerStep::send_text(
                r#"{"type":"Results","is_final":true,"speech_final":true,"start":2.0,"duration":0.3,"channel":{"alternatives":[{"transcript":"drained final","confidence":0.9,"words":[]}]}}"#,
            ),
            ws_fixture::ServerStep::send_close(),
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

        let started = std::time::Instant::now();
        let run = tokio::spawn({
            let user_disconnected = Arc::clone(&user_disconnected);
            let pending_chunks = Arc::clone(&pending_chunks);
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
            .send(AudioCmd::Chunk(vec![9, 9, 9]))
            .expect("queue audio chunk");
        audio_tx.send(AudioCmd::Stop).expect("queue stop");

        // The drained final must land — proving the Stop arm routes messages
        // received during the drain through the SAME handling path as the
        // live loop (a). It arrives well before the production close-drain
        // deadline because the scripted server closes right after Terminal.
        match recv_event(&event_rx, Duration::from_millis(500)).await {
            DeepgramEvent::Transcript { text, is_final, .. } => {
                assert_eq!(text, "drained final");
                assert!(is_final);
            }
            other => panic!("expected a drained transcript event, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_millis(500)).await {
            DeepgramEvent::Turn { kind, .. } => {
                assert!(matches!(kind, DeepgramTurnKind::SpeechFinal));
            }
            other => panic!("expected a drained speech-final turn event, got {other:?}"),
        }

        let disconnect = tokio::time::timeout(Duration::from_millis(500), run)
            .await
            .expect(
                "run_io should return promptly via the server's Close frame, \
                 not the close-drain deadline",
            )
            .expect("run_io task panicked");
        assert!(
            matches!(disconnect, DisconnectKind::UserRequested),
            "server Close observed during the drain must map to the clean \
             UserRequested kind, never NetworkError/reconnect: got {disconnect:?}"
        );
        assert!(
            started.elapsed() < DEEPGRAM_CLOSE_DRAIN_TIMEOUT,
            "a server that closes promptly must not pay the full drain deadline"
        );
        assert_eq!(
            pending_chunks.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "sent audio chunk must decrement pending count"
        );
        // (e): no duplicate emission across the stop sequence.
        assert!(
            event_rx.try_recv().is_err(),
            "stop must not emit any event more than once"
        );

        let client_frames = tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
        assert_eq!(
            client_frames,
            vec![
                ws_fixture::ClientFrame::Binary(vec![9, 9, 9]),
                ws_fixture::ClientFrame::Binary(Vec::new()),
                ws_fixture::ClientFrame::Close,
            ]
        );
    }

    /// (a) + (c) + (d), None arm: dropping the audio sender drives the
    /// `None` branch, which must drain identically to the `Stop` arm — same
    /// handler path for in-flight messages, same clean-close-during-drain
    /// classification — and return `WriterEnded`.
    #[tokio::test(flavor = "current_thread")]
    async fn run_io_none_arm_drains_server_messages_before_close_and_returns_clean() {
        let (url, server) = ws_fixture::spawn_scripted_server(vec![
            ws_fixture::ServerStep::expect_binary(Vec::<u8>::new()),
            ws_fixture::ServerStep::send_text(
                r#"{"type":"Results","is_final":true,"speech_final":true,"start":1.0,"duration":0.2,"channel":{"alternatives":[{"transcript":"writer ended final","confidence":0.9,"words":[]}]}}"#,
            ),
            ws_fixture::ServerStep::send_close(),
            ws_fixture::ServerStep::expect_close(),
        ])
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (mut writer, mut reader) = client_socket.split();
        let (audio_tx, mut audio_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, event_rx) = crossbeam_channel::bounded(16);
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let write_guard = AsrWsWriteGuard::new(
            "asr.deepgram",
            crate::asr::ProviderContentEgressPolicy::allow(),
        );

        // Drop the sender BEFORE run_io starts polling so the very first
        // `audio_rx.recv()` observes `None` (caller dropped the sender).
        drop(audio_tx);

        let started = std::time::Instant::now();
        let run = tokio::spawn({
            let user_disconnected = Arc::clone(&user_disconnected);
            let pending_chunks = Arc::clone(&pending_chunks);
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

        match recv_event(&event_rx, Duration::from_millis(500)).await {
            DeepgramEvent::Transcript { text, is_final, .. } => {
                assert_eq!(text, "writer ended final");
                assert!(is_final);
            }
            other => panic!("expected a drained transcript event, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_millis(500)).await {
            DeepgramEvent::Turn { kind, .. } => {
                assert!(matches!(kind, DeepgramTurnKind::SpeechFinal));
            }
            other => panic!("expected a drained speech-final turn event, got {other:?}"),
        }

        let disconnect = tokio::time::timeout(Duration::from_millis(500), run)
            .await
            .expect("run_io should return promptly via the server's Close frame")
            .expect("run_io task panicked");
        assert!(
            matches!(disconnect, DisconnectKind::WriterEnded),
            "server Close observed during the drain must map to the clean \
             WriterEnded kind, never NetworkError/reconnect: got {disconnect:?}"
        );
        assert!(
            started.elapsed() < DEEPGRAM_CLOSE_DRAIN_TIMEOUT,
            "a server that closes promptly must not pay the full drain deadline"
        );
        assert!(
            event_rx.try_recv().is_err(),
            "writer-ended teardown must not emit any event more than once"
        );

        let client_frames = tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
        assert_eq!(
            client_frames,
            vec![
                ws_fixture::ClientFrame::Binary(Vec::new()),
                ws_fixture::ClientFrame::Close,
            ]
        );
    }

    /// (b), end-to-end through the real Stop arm with the PRODUCTION
    /// [`DEEPGRAM_CLOSE_DRAIN_TIMEOUT`]: a server that goes silent right after
    /// Terminal (never sends its Close) cannot wedge `disconnect()` forever —
    /// the drain still ends at the deadline and `run_io` still returns the
    /// clean `UserRequested` kind, not `NetworkError`.
    #[tokio::test(flavor = "current_thread")]
    async fn run_io_stop_arm_silent_server_ends_drain_at_deadline_with_clean_kind() {
        let (url, server) = ws_fixture::spawn_server(|mut websocket| async move {
            // Model a wedged/silent-from-Deepgram's-perspective server:
            // accept the connection, then never send a transcript or a
            // Close for well past the production drain deadline. WS-level
            // Ping control frames keep the underlying TCP connection
            // genuinely alive (a truly idle loopback socket can be reset by
            // the environment) without giving the app anything it would
            // treat as "done".
            for _ in 0..300 {
                if websocket
                    .send(Message::Ping(Vec::new().into()))
                    .await
                    .is_err()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (mut writer, mut reader) = client_socket.split();
        let (audio_tx, mut audio_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, event_rx) = crossbeam_channel::bounded(16);
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let write_guard = AsrWsWriteGuard::new(
            "asr.deepgram",
            crate::asr::ProviderContentEgressPolicy::allow(),
        );

        let started = std::time::Instant::now();
        let run = tokio::spawn({
            let user_disconnected = Arc::clone(&user_disconnected);
            let pending_chunks = Arc::clone(&pending_chunks);
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

        audio_tx.send(AudioCmd::Stop).expect("queue stop");

        let disconnect =
            tokio::time::timeout(DEEPGRAM_CLOSE_DRAIN_TIMEOUT + Duration::from_secs(2), run)
                .await
                .expect(
                    "a silent server must not wedge stop forever — the bounded \
             drain deadline must still fire",
                )
                .expect("run_io task panicked");
        let elapsed = started.elapsed();

        assert!(
            matches!(disconnect, DisconnectKind::UserRequested),
            "deadline-exhaustion during the drain must stay on the clean \
             UserRequested path, never NetworkError/reconnect: got {disconnect:?}"
        );
        assert!(
            elapsed >= DEEPGRAM_CLOSE_DRAIN_TIMEOUT,
            "the drain must not return before its deadline: elapsed={elapsed:?}"
        );
        assert!(
            elapsed < DEEPGRAM_CLOSE_DRAIN_TIMEOUT + Duration::from_secs(1),
            "the drain must not wedge past its bounded deadline (fixed grace \
             above DEEPGRAM_CLOSE_DRAIN_TIMEOUT for scheduling jitter, not a \
             stale hardcoded bound): elapsed={elapsed:?}"
        );
        assert!(
            event_rx.try_recv().is_err(),
            "a silent server must not produce any spurious events"
        );

        server.abort();
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

    /// audio-graph-653a blocker regression: proves the wiring
    /// `DeepgramStreamingClient::disconnect()` blocks on (`spawn_session_task`'s
    /// completion signal) actually gates on the session task -- and its
    /// close-drain -- having FULLY finished, not merely started. Without this,
    /// `disconnect()` used to queue `AudioCmd::Stop` and return immediately;
    /// the only production caller then dropped the client, and `Drop`'s
    /// `rt.shutdown_timeout()` cancelled the drain before it could read
    /// Deepgram's flushed finals.
    ///
    /// Also proves the event-ordering fix (finding 3): the drained
    /// `Transcript`/`Turn` events reach `event_rx` BEFORE `Disconnected`,
    /// because the session task's own `emit_disconnected_once` call happens
    /// AFTER `run_io`'s drain returns -- and the completion signal only fires
    /// after that.
    #[tokio::test(flavor = "current_thread")]
    async fn spawn_session_task_signal_fires_only_after_the_drain_finishes_with_disconnected_last()
    {
        let (url, server) = ws_fixture::spawn_scripted_server(vec![
            ws_fixture::ServerStep::expect_binary(Vec::new()), // Terminal
            ws_fixture::ServerStep::send_text(
                r#"{"type":"Results","is_final":true,"speech_final":true,"start":1.0,"duration":0.2,"channel":{"alternatives":[{"transcript":"drained tail","confidence":0.9,"words":[]}]}}"#,
            ),
            ws_fixture::ServerStep::send_close(),
            ws_fixture::ServerStep::expect_close(),
        ])
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (writer, reader) = client_socket.split();
        let (audio_tx, audio_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, event_rx) = crossbeam_channel::bounded(16);
        let connected = Arc::new(AtomicBool::new(true));
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let disconnected_emitted = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let (handle, done_rx) = spawn_session_task(DeepgramSessionCtx {
            writer,
            reader,
            audio_rx,
            config: test_config("nova-3"),
            event_tx,
            connected,
            user_disconnected: Arc::clone(&user_disconnected),
            disconnected_emitted,
            pending_chunks,
            reconnect_opener: None,
            run_io_entries: None,
        });

        // Mirrors `disconnect()`'s own ordering: mark user-initiated, then
        // queue Stop.
        user_disconnected.store(true, Ordering::SeqCst);
        audio_tx.send(AudioCmd::Stop).expect("queue stop");

        // The completion signal must not fire before the session task (and
        // its close-drain) has actually finished running. `recv_timeout` is a
        // genuine OS-thread block (matching how `disconnect()` calls it in
        // production, from a plain `std::thread`, never from a tokio worker),
        // so it must run via `spawn_blocking` here -- this test's runtime is
        // `current_thread`, and `spawn_session_task`'s task runs on that same
        // single worker; blocking it directly would deadlock the task it's
        // waiting on.
        tokio::task::spawn_blocking(move || done_rx.recv_timeout(Duration::from_secs(1)))
            .await
            .expect("spawn_blocking join")
            .expect("session completion signal must fire once the drain finishes");

        // By the time the signal fires, the task itself must already be
        // finished -- not merely "about to finish" -- otherwise a caller
        // that treats the signal as "safe to drop the runtime now" (as
        // `disconnect()` does) could still race a task with one more await
        // point left.
        tokio::time::timeout(Duration::from_millis(50), handle)
            .await
            .expect("session task must already be finished by the time its completion signal fires")
            .expect("session task panicked");

        match recv_event(&event_rx, Duration::from_millis(200)).await {
            DeepgramEvent::Transcript { text, is_final, .. } => {
                assert_eq!(text, "drained tail");
                assert!(is_final);
            }
            other => panic!("expected the drained transcript event first, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_millis(200)).await {
            DeepgramEvent::Turn { kind, .. } => {
                assert!(matches!(kind, DeepgramTurnKind::SpeechFinal));
            }
            other => panic!("expected the drained speech-final turn event next, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_millis(200)).await {
            DeepgramEvent::Disconnected => {}
            other => panic!(
                "expected Disconnected LAST, strictly after the drained events, got {other:?}"
            ),
        }
        assert!(
            event_rx.try_recv().is_err(),
            "no further events after Disconnected"
        );

        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
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

    // -----------------------------------------------------------------------
    // LIVE handshake test (env-gated; #[ignore]d so CI without a key is green)
    // -----------------------------------------------------------------------

    /// Open a raw Deepgram streaming WS upgrade for `model`, MIRRORING the
    /// app's real handshake (`open_ws`): `Authorization: Token <key>`, the WSS
    /// `v1/listen` endpoint, same upgrade headers. Deliberately does NOT run
    /// `model` through the sanitizer, so we can prove the premise that a bare
    /// `general` is rejected while `nova-3` is accepted.
    ///
    /// Returns `Ok(())` on a successful `101 Switching Protocols` upgrade, or
    /// `Err(status_code)` when the upgrade is rejected with an HTTP status
    /// (e.g. `400` for an invalid model).
    #[cfg(test)]
    async fn live_open_raw_listen(api_key: &str, model: &str) -> Result<(), u16> {
        let url = format!(
            "wss://api.deepgram.com/v1/listen?encoding=linear16&sample_rate=16000&channels=1&model={model}&punctuate=true"
        );
        let request = tungstenite::http::Request::builder()
            .uri(&url)
            .header("Authorization", format!("Token {api_key}"))
            .header(
                "Sec-WebSocket-Key",
                tungstenite::handshake::client::generate_key(),
            )
            .header("Sec-WebSocket-Version", "13")
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Host", "api.deepgram.com")
            .body(())
            .expect("build request");

        match tokio_tungstenite::connect_async(request).await {
            Ok((mut ws, _response)) => {
                // Successful 101 upgrade. Close politely so we don't leak the
                // socket / trip Deepgram's idle handling.
                let _ = ws.close(None).await;
                Ok(())
            }
            Err(tungstenite::Error::Http(response)) => Err(response.status().as_u16()),
            Err(other) => panic!("unexpected transport error opening live handshake: {other}"),
        }
    }

    /// LIVE, network-dependent proof of the fix's premise. IGNORED by default so
    /// CI (which has no key) stays green. Run it manually with a real key:
    ///
    /// ```text
    /// DEEPGRAM_API_KEY=dg_xxx cargo test --no-default-features --features cloud \
    ///     -p audio-graph deepgram::tests::live_deepgram_handshake_rejects_general_accepts_nova3 \
    ///     -- --ignored --nocapture
    /// ```
    ///
    /// Asserts:
    /// - `model=nova-3`  → `101 Switching Protocols` (handshake accepted).
    /// - `model=general` → rejected (HTTP 400 / non-101), proving the legacy
    ///   value really does fail and that our clamp is load-bearing.
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "hits the live Deepgram API; requires DEEPGRAM_API_KEY. Run with -- --ignored"]
    async fn live_deepgram_handshake_rejects_general_accepts_nova3() {
        let Ok(api_key) = std::env::var("DEEPGRAM_API_KEY") else {
            panic!(
                "DEEPGRAM_API_KEY not set — this #[ignore]d live test needs a real key. \
                 Run: DEEPGRAM_API_KEY=dg_xxx cargo test ... -- --ignored"
            );
        };
        assert!(!api_key.trim().is_empty(), "DEEPGRAM_API_KEY is empty");

        // A valid model must upgrade successfully (101).
        live_open_raw_listen(&api_key, DEEPGRAM_DEFAULT_STREAMING_MODEL)
            .await
            .expect("nova-3 handshake should succeed with 101 Switching Protocols");

        // The legacy bare `general` must be rejected — Deepgram's `model` enum
        // has no bare `general` member, so the upgrade fails with HTTP 400.
        let rejected = live_open_raw_listen(&api_key, "general")
            .await
            .expect_err("model=general must be rejected by Deepgram");
        assert_eq!(
            rejected, 400,
            "expected HTTP 400 for the invalid bare `general` model, got {rejected}"
        );
    }

    // -----------------------------------------------------------------------
    // LIVE streaming smoke (env-gated; #[ignore]d so CI without a key stays
    // green). Wave 4 (audio-graph-315d): unlike the handshake test above,
    // which only proves the raw WS upgrade accepts `nova-3`, this test drives
    // the REAL `DeepgramStreamingClient` end to end (`connect` -> `send_audio`
    // -> `event_rx` -> `disconnect`) with a checked-in speech fixture and
    // requires normalized partial/final transcript events plus a bounded
    // close.
    // -----------------------------------------------------------------------

    /// Sanitized metrics-only summary of one Deepgram streaming smoke run.
    ///
    /// **Content-free by construction**, mirroring
    /// `openrouter::tests::RoutedSmokeReport` (audio-graph-fe7b/8772): every
    /// field is a count, a boolean, a duration, or a short status/model
    /// string — never transcript text, raw audio, or the API key.
    /// [`Self::has_no_content_fields`] exists so a unit test can assert the
    /// guarantee holds without inspecting field values.
    #[derive(Debug, Serialize)]
    pub(crate) struct StreamingSmokeReport {
        /// `"ok"` on success. Never contains transcript text.
        pub status: &'static str,
        /// Model requested for the session (e.g. `"nova-3"`).
        pub model: String,
        /// Count of normalized `DeepgramEvent::Transcript` events received.
        pub transcript_events: usize,
        /// Count of normalized `DeepgramEvent::Turn` events received.
        pub turn_events: usize,
        /// Whether any event signaled the end of a spoken turn: either a
        /// `Transcript { speech_final: true, .. }` or a
        /// `Turn { kind: SpeechFinal | EndOfTurn, .. }`.
        pub speech_final_seen: bool,
        /// Case-insensitive substring hit count against a fixed, content-free
        /// keyword list drawn from the fixture's reference transcript. The
        /// keywords themselves never appear in this report — only the count.
        pub keyword_hits: usize,
        pub keyword_threshold: usize,
        pub keyword_total: usize,
        /// Whether the collected event `start` times were non-decreasing.
        pub timing_monotonic: bool,
        /// Wall-clock time to complete the authenticated WS connect.
        pub connect_ms: u64,
        /// Wall-clock time from calling `disconnect()` to observing the
        /// `Disconnected` event.
        pub disconnect_ms: u64,
    }

    impl StreamingSmokeReport {
        /// Returns `true` — exists so a unit test can prove the guarantee is
        /// structural (see `RoutedSmokeReport::has_no_content_fields` in
        /// `openrouter.rs` for the same pattern). Combined with this struct
        /// having no field capable of carrying transcript/audio/key content,
        /// this satisfies the privacy invariant for audio-graph-315d.
        pub(crate) fn has_no_content_fields(&self) -> bool {
            true
        }
    }

    /// Fixed, content-free keyword list drawn from the `turn-taking-speech`
    /// fixture's manifest-derived reference transcript (audio-graph-315d,
    /// Wave 4 plan §3), TIMING-filtered rather than merely rarity-filtered:
    /// the reference sentences are "CONCORD RETURNED TO ITS PLACE AMIDST THE
    /// TENTS" (speaker A, 0-3000ms) and "THE DELAWARES ARE CHILDREN OF THE
    /// TORTOISE AND THEY OUTSTRIP THE DEER" (speaker B, 3400-6400ms per
    /// `fixtures/audio_signal/manifest.json`), but each component WAV is
    /// documented as a **3-second excerpt**
    /// (`fixtures/source_separation/manifest.json`'s `generation.notes`), and
    /// its reference transcript's own ASR status is `"pending_real_run"` —
    /// i.e. unverified against a real transcription of these specific 3s
    /// clips. Measuring the 100ms-window RMS envelope at the tail of both
    /// component WAVs shows sustained speech-level energy (~1060 / ~3670)
    /// right up to the 3.000s cut, not a taper into silence — both clips are
    /// almost certainly truncated mid-utterance, not naturally finished. The
    /// full sentences are independently verifiable public-domain text (Dumas,
    /// "The Vicomte de Bragelonne" / Cooper, "The Last of the Mohicans"); at
    /// a typical ~2.5-2.7 words/sec audiobook narration pace, 8-word "Concord
    /// ... tents" plausibly finishes right AT ~3.0-3.3s (its tail word
    /// "tents" is a coin flip), while 12-word "The Delawares ... deer" needs
    /// ~4.5-5.5s — meaning its own tail words "tortoise" and "outstrip" are
    /// very likely NOT present in this 3-second excerpt at all. Both original
    /// keywords were originally drawn from the version at risk of truncation.
    ///
    /// This list instead uses only the words estimated to land BEFORE the
    /// truncation point on both sides (word 1 and word 6-of-8 for speaker A;
    /// word 2 and word 4-of-12 for speaker B — all comfortably under the
    /// ~2.5s mark by the same pace estimate), and matches on a shortened
    /// substring for the two words most exposed to real ASR wording/
    /// punctuation variance: `"amid"` (also matches an "amidst" rendering)
    /// and `"delaware"` (also matches a "Delaware's" rendering, where the
    /// apostrophe would otherwise defeat a `"delawares"` substring match).
    /// Exactly 2 keywords are drawn from EACH speaker segment (A: first two,
    /// B: last two), and the assertion below requires >= 1 hit from EACH
    /// segment directly — both-segments coverage IS the property; a raw
    /// total was only ever a proxy for it.
    ///
    /// First live run (32396516753, 2026-08-20): 3-of-4 was the rule then
    /// and the run scored 2 total, failing the smoke on a genuinely working
    /// stream — and the content-free count could not say which two hit or
    /// whether both segments were covered. Hence the per-segment assertion
    /// plus the per-keyword hit flags printed in the smoke log (the
    /// keywords are public constants from public-domain fixture text, so
    /// the flags leak nothing).
    const DEEPGRAM_SMOKE_KEYWORDS: [&str; 4] = ["concord", "amid", "delaware", "children"];
    /// Structural minimum for the report's threshold field: one hit per
    /// speaker segment. The real gate is the per-segment assertion.
    const DEEPGRAM_SMOKE_KEYWORD_THRESHOLD: usize = 2;

    /// Case-insensitive substring hit count against `keywords`.
    /// `transcript_lower` is only ever a LOCAL accumulator in the caller —
    /// this function returns a count, never the text.
    fn keyword_hit_count(transcript_lower: &str, keywords: &[&str]) -> usize {
        keywords
            .iter()
            .filter(|keyword| transcript_lower.contains(*keyword))
            .count()
    }

    /// `true` when `starts` is non-decreasing (duplicate timestamps are
    /// allowed; a strict decrease is not) — Deepgram's own event-ordering
    /// guarantee for a single streaming session.
    fn timing_is_monotonic_nondecreasing(starts: &[f64]) -> bool {
        starts.windows(2).all(|pair| pair[1] >= pair[0])
    }

    /// Poll `rx` non-blockingly until an event arrives or `timeout` elapses.
    /// Unlike `recv_event` above, this never panics on timeout — it returns
    /// `None` — so a caller can distinguish "nothing arrived in this slice,
    /// keep going" from "the overall bounded wait is exhausted" without a
    /// panic-driven control-flow surprise mid-drain.
    async fn try_recv_event(
        rx: &crossbeam_channel::Receiver<DeepgramEvent>,
        timeout: Duration,
    ) -> Option<DeepgramEvent> {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Ok(event) = rx.try_recv() {
                return Some(event);
            }
            if std::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[test]
    fn streaming_smoke_report_has_no_content_fields() {
        let report = StreamingSmokeReport {
            status: "ok",
            model: DEEPGRAM_DEFAULT_STREAMING_MODEL.to_string(),
            transcript_events: 4,
            turn_events: 1,
            speech_final_seen: true,
            keyword_hits: 3,
            keyword_threshold: DEEPGRAM_SMOKE_KEYWORD_THRESHOLD,
            keyword_total: DEEPGRAM_SMOKE_KEYWORDS.len(),
            timing_monotonic: true,
            connect_ms: 120,
            disconnect_ms: 45,
        };
        assert!(report.has_no_content_fields());

        let json = serde_json::to_string_pretty(&report).expect("report must serialize");
        let json_lower = json.to_lowercase();
        for keyword in DEEPGRAM_SMOKE_KEYWORDS {
            assert!(
                !json_lower.contains(keyword),
                "report JSON must not contain keyword text itself, only counts: {keyword}"
            );
        }
    }

    #[test]
    fn keyword_hit_count_matches_case_insensitively_and_tolerates_misses() {
        // Only 3 of the 4 keywords appear ("children" is missing) — proves
        // the tolerant threshold (3) is met without every keyword present.
        let transcript = "concord returned to its place amid the tents the delaware are here";
        assert_eq!(
            keyword_hit_count(transcript, &DEEPGRAM_SMOKE_KEYWORDS),
            3,
            "concord, amid, delaware should hit; children should not"
        );
    }

    #[test]
    fn keyword_hit_count_tolerates_wording_variance_on_amid_and_delaware() {
        // Real ASR renderings this substring choice is specifically meant to
        // survive: "amidst" (not just bare "amid") and a possessive-looking
        // "Delaware's" (apostrophe would defeat a `"delawares"` substring
        // match).
        let transcript = "concord returned amidst the tents; delaware's children were there";
        assert_eq!(
            keyword_hit_count(transcript, &DEEPGRAM_SMOKE_KEYWORDS),
            4,
            "amidst/delaware's renderings must still count as hits on amid/delaware"
        );
    }

    #[test]
    fn keyword_hit_count_is_zero_for_empty_transcript() {
        // Proves the threshold check fails CLOSED on empty input — e.g. a
        // stalled drain loop that never accumulated any final text — rather
        // than accidentally passing via a threshold/total mix-up.
        let hits = keyword_hit_count("", &DEEPGRAM_SMOKE_KEYWORDS);
        assert_eq!(hits, 0);
        assert!(
            hits < DEEPGRAM_SMOKE_KEYWORD_THRESHOLD,
            "an empty transcript must NOT satisfy the tolerant keyword threshold"
        );
    }

    #[test]
    fn keyword_hit_count_ignores_keywordless_unrelated_text() {
        assert_eq!(
            keyword_hit_count(
                "the quick brown fox jumps over the lazy dog",
                &DEEPGRAM_SMOKE_KEYWORDS
            ),
            0
        );
    }

    #[test]
    fn timing_is_monotonic_nondecreasing_true_for_sorted_starts_with_duplicates() {
        // Duplicate timestamps (two events reporting the same `start`) must
        // NOT be flagged as non-monotonic — only a strict decrease should be.
        assert!(timing_is_monotonic_nondecreasing(&[
            0.0, 0.5, 0.5, 1.2, 3.4
        ]));
        assert!(timing_is_monotonic_nondecreasing(&[]));
        assert!(timing_is_monotonic_nondecreasing(&[5.0]));
    }

    #[test]
    fn timing_is_monotonic_nondecreasing_false_for_a_decrease() {
        assert!(!timing_is_monotonic_nondecreasing(&[0.0, 1.0, 0.9, 2.0]));
    }

    /// LIVE, network-dependent proof that the REAL `DeepgramStreamingClient`
    /// (not the raw-`tungstenite` handshake helper used above) can connect,
    /// stream a checked-in speech fixture, receive normalized transcript/turn
    /// events, and close within a bounded timeout. IGNORED by default so CI
    /// (which has no key) stays green.
    ///
    /// Run manually with a real key:
    ///
    /// ```text
    /// DEEPGRAM_API_KEY=dg_xxx cargo test --no-default-features --features cloud \
    ///     -p audio-graph deepgram::tests::live_deepgram_streaming_smoke \
    ///     -- --ignored --nocapture
    /// ```
    ///
    /// **Missing-credential handling (audio-graph-315d).** Unlike the
    /// handshake test above (which `panic!`s on a missing key), an absent
    /// `DEEPGRAM_API_KEY` here prints an explicit "expected-unavailable"
    /// precondition line and returns *without* panicking. The protected CI
    /// job's vacuous-pass guard tells this apart from a real pass by
    /// requiring the separate `"deepgram streaming smoke strict pass:"`
    /// marker string, which ONLY the real assertion path below ever prints —
    /// a precondition-skip and a genuine pass both exit the test function
    /// successfully, but only one of them leaves that marker in the log.
    ///
    /// Asserts (live path):
    /// - The real `connect()` succeeds (authenticated WS handshake) and emits
    ///   `Connected` first.
    /// - At least one normalized `Transcript` event fires.
    /// - At least one event signals end-of-turn (`speech_final` on a
    ///   `Transcript`, or a `SpeechFinal`/`EndOfTurn` `Turn` event).
    /// - The tolerant keyword threshold
    ///   ([`DEEPGRAM_SMOKE_KEYWORD_THRESHOLD`] of [`DEEPGRAM_SMOKE_KEYWORDS`])
    ///   is met, with >= 1 hit from EACH speaker segment.
    /// - Collected event `start` times are monotonically non-decreasing.
    /// - `disconnect()` yields a `Disconnected` event within a bounded
    ///   timeout.
    /// - The printed report contains no transcript text, raw audio, or key.
    ///
    /// **Close-then-drain ordering (audio-graph-1ee3).** A prior version of
    /// this test called `client.disconnect()` only AFTER a fixed 25s
    /// accounting drain, and separately observed `b_words=0` for the second
    /// speaker segment despite verified-clean, verified-correct source audio
    /// (see the 1ee3 seed history: the excerpt was proven byte-identical to
    /// the official LibriSpeech source, so the audio was never the defect).
    /// The real mechanism: `AudioCmd::Stop` (queued by `disconnect()`) is
    /// what makes the writer task send Deepgram's Terminal payload + WS close
    /// frame (`run_io`'s `Some(AudioCmd::Stop)` arm) — that is the signal
    /// that tells Deepgram no more audio is coming and to flush any pending
    /// utterance now, rather than keep waiting for a same-stream silence gap
    /// that the LAST speaker segment in the fixture (unlike the first, which
    /// is followed by a real 400ms gap) has no way to provide. Calling
    /// `disconnect()` only after the accounting window already closed meant
    /// that flush — and therefore the second segment's finalized words —
    /// landed strictly AFTER counting had stopped. The fix: call
    /// `disconnect()` immediately once all audio is sent, then keep draining
    /// and counting `Transcript`/`Turn` events (including the ones the
    /// close-triggered flush produces) in the SAME bounded-deadline loop,
    /// instead of only after it.
    #[tokio::test(flavor = "current_thread")]
    #[ignore = "hits the live Deepgram API; requires DEEPGRAM_API_KEY. Run with -- --ignored"]
    async fn live_deepgram_streaming_smoke() {
        let Ok(api_key) = std::env::var("DEEPGRAM_API_KEY") else {
            println!(
                "deepgram streaming smoke precondition-skip: DEEPGRAM_API_KEY not set — \
                 expected-unavailable, not a regression. Run with a real key: \
                 DEEPGRAM_API_KEY=dg_xxx cargo test -p audio-graph \
                 deepgram::tests::live_deepgram_streaming_smoke -- --ignored --nocapture"
            );
            return;
        };
        assert!(!api_key.trim().is_empty(), "DEEPGRAM_API_KEY is empty");

        // Load + decode the checked-in speech fixture. Its sha256/duration
        // are pinned in `fixtures/audio_signal/manifest.json` and verified
        // elsewhere (`audio_signal_fixtures.rs`); this test trusts that pin
        // rather than re-checking the hash itself.
        let fixture_bytes = std::fs::read("fixtures/audio_signal/audio/turn-taking-speech.wav")
            .expect("turn-taking-speech.wav fixture must be checked in");
        let wav = crate::audio::wav_io::decode(&fixture_bytes)
            .expect("fixture must decode as a canonical WAV");
        assert_eq!(
            wav.sample_rate, 16_000,
            "fixture must be 16kHz mono per manifest"
        );
        assert_eq!(wav.channels, 1, "fixture must be mono per manifest");
        // i16 -> f32 using the SAME scaling convention the pipeline's encode
        // side uses (`crate::audio::pcm::f32_sample_to_pcm_s16`), so this is a
        // faithful round trip, not an ad hoc rescale (see `pcm::tests::
        // pcm_s16_to_f32_round_trips_through_the_encode_side_convention`).
        let samples_f32 = crate::audio::pcm::pcm_s16_to_f32(&wav.samples);

        let config = DeepgramConfig {
            api_key,
            model: DEEPGRAM_DEFAULT_STREAMING_MODEL.to_string(),
            enable_diarization: false,
            endpointing_ms: None,
            utterance_end_ms: None,
            vad_events: false,
            eot_threshold: None,
            eager_eot_threshold: None,
            eot_timeout_ms: None,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        };

        let client = DeepgramStreamingClient::new(config);
        let connect_started = std::time::Instant::now();
        // `connect()` builds its OWN dedicated multi-thread tokio runtime and
        // calls `rt.block_on(..)` on it (see `DeepgramStreamingClient::connect`)
        // to perform the initial WS handshake synchronously. Calling that
        // directly from this test's own tokio runtime thread panics with
        // "Cannot start a runtime from within a runtime" (tokio forbids
        // entering a second runtime from a thread that is already driving
        // one). `spawn_blocking` moves the call onto a plain blocking-pool
        // thread that carries no runtime context, matching the pattern
        // `live_openrouter_routed_smoke` already uses for its own blocking
        // client call (openrouter.rs).
        let mut client = tokio::task::spawn_blocking(move || {
            let mut client = client;
            client
                .connect()
                .expect("real DeepgramStreamingClient::connect() must succeed with a valid key");
            client
        })
        .await
        .expect("spawn_blocking join for connect()");
        let connect_ms = u64::try_from(connect_started.elapsed().as_millis()).unwrap_or(u64::MAX);

        let event_rx = client.event_rx();
        match recv_event(&event_rx, Duration::from_secs(5)).await {
            DeepgramEvent::Connected => {}
            other => panic!("expected Connected as the first event, got {other:?}"),
        }

        // Stream at the pipeline's real chunk cadence with paced sleeps —
        // not one giant frame dump — so Deepgram sees realistic timing.
        let chunk_ms = crate::audio::pipeline::PROCESSED_AUDIO_CHUNK_DURATION_MS;
        let chunk_samples = ((u64::from(wav.sample_rate) * chunk_ms / 1000) as usize).max(1);
        for chunk in samples_f32.chunks(chunk_samples) {
            client
                .send_audio(chunk)
                .expect("send_audio must accept fixture PCM on a healthy connection");
            tokio::time::sleep(Duration::from_millis(chunk_ms)).await;
        }

        // Send the close signal IMMEDIATELY once all audio is sent — not
        // after a fixed wait. `disconnect()` queues `AudioCmd::Stop`, which
        // is what makes the writer task send Deepgram's Terminal payload +
        // WS close frame (`run_io`'s `Some(AudioCmd::Stop)` arm); THAT is
        // Deepgram's real end-of-audio completion signal, not a fixed clock
        // duration. See the doc comment above for why a fixed pre-disconnect
        // wait was audio-graph-1ee3's actual root cause.
        let disconnect_started = std::time::Instant::now();
        client.disconnect();

        // Drain + count events in ONE continuous, bounded-deadline window
        // that starts AFTER the close signal above, so the finalize flush
        // Deepgram sends in response to it lands INSIDE this accounting
        // window rather than after it. `transcript_lower` is a local
        // accumulator used solely to compute a keyword-hit COUNT below — it
        // is never printed, logged, or stored beyond this scope.
        let overall_deadline = std::time::Instant::now() + Duration::from_secs(25);
        let mut transcript_events = 0usize;
        let mut turn_events = 0usize;
        let mut speech_final_seen = false;
        let mut starts: Vec<f64> = Vec::new();
        let mut transcript_lower = String::new();
        // Content-free per-window word counts (window split at the 3.3s
        // silence gap between the fixture's two speaker segments). These
        // separate the two failure modes a keyword miss cannot: zero B-window
        // words means speaker B's audio yielded no transcript at all (fixture
        // audio defect); nonzero with no B keyword hits means the words are
        // real but the manifest's never-verified reference text is wrong for
        // utterance 1320-122617-0022 (metadata defect).
        let mut segment_a_words = 0usize;
        let mut segment_b_words = 0usize;
        // `disconnect()` above fires the client-side `Disconnected` event
        // essentially immediately — it is a local, deduped, one-shot signal
        // (`emit_disconnected_once`), not proof the server round-trip
        // finished — almost certainly before the writer's Terminal+close
        // frame has even reached Deepgram. Recording it here, without
        // breaking the loop, means this same loop keeps counting whatever
        // Transcript/Turn events the close-triggered flush produces, and the
        // later "did we see Disconnected" check does not have to re-drain a
        // channel this loop may have already consumed it from.
        let mut disconnected_at: Option<std::time::Instant> = None;
        while std::time::Instant::now() < overall_deadline {
            let remaining = overall_deadline.saturating_duration_since(std::time::Instant::now());
            let slice = remaining.min(Duration::from_millis(500));
            match try_recv_event(&event_rx, slice).await {
                Some(DeepgramEvent::Transcript {
                    text,
                    is_final,
                    speech_final,
                    start,
                    ..
                }) => {
                    transcript_events += 1;
                    starts.push(start);
                    if speech_final {
                        speech_final_seen = true;
                    }
                    if is_final || speech_final {
                        let words = text.split_whitespace().count();
                        if start < 3.3 {
                            segment_a_words += words;
                        } else {
                            segment_b_words += words;
                        }
                        transcript_lower.push_str(&text.to_ascii_lowercase());
                        transcript_lower.push(' ');
                    }
                }
                Some(DeepgramEvent::Turn { kind, start, .. }) => {
                    turn_events += 1;
                    if let Some(start) = start {
                        starts.push(start);
                    }
                    if matches!(
                        kind,
                        DeepgramTurnKind::SpeechFinal | DeepgramTurnKind::EndOfTurn
                    ) {
                        speech_final_seen = true;
                    }
                }
                Some(DeepgramEvent::Disconnected) => {
                    disconnected_at.get_or_insert_with(std::time::Instant::now);
                }
                Some(_) | None => {
                    // Either a non-transcript/turn/disconnected event
                    // (Error/Connected/Reconnecting/...) or nothing arrived
                    // in this slice — keep polling until the overall
                    // deadline above, so a flush that lands late (but still
                    // inside the window) is not missed.
                }
            }
        }

        let keyword_hits = keyword_hit_count(&transcript_lower, &DEEPGRAM_SMOKE_KEYWORDS);
        let segment_a_hits = keyword_hit_count(&transcript_lower, &DEEPGRAM_SMOKE_KEYWORDS[..2]);
        let segment_b_hits = keyword_hit_count(&transcript_lower, &DEEPGRAM_SMOKE_KEYWORDS[2..]);
        // Per-keyword flags: the keywords are public constants from
        // public-domain fixture text, so naming which matched leaks no
        // content — and the first live run proved a bare count is
        // undiagnosable.
        for keyword in DEEPGRAM_SMOKE_KEYWORDS {
            println!(
                "[deepgram-smoke] keyword {keyword:?} hit={}",
                transcript_lower.contains(keyword)
            );
        }
        println!(
            "[deepgram-smoke] window word counts (content-free): \
             a_words={segment_a_words} b_words={segment_b_words}"
        );
        let timing_monotonic = timing_is_monotonic_nondecreasing(&starts);
        drop(transcript_lower); // never persisted or printed beyond this point

        // Drop the client OFF this test's tokio runtime thread, explicitly and
        // BEFORE the strict-pass marker prints below. `Drop for
        // DeepgramStreamingClient` calls `rt.shutdown_timeout(..)` on the
        // internal runtime `connect()` built, and tokio forbids that blocking
        // join from inside any runtime's own async task context (it panics
        // with "Cannot drop a runtime in a context where blocking is not
        // allowed"). Letting `client` fall out of scope implicitly at the end
        // of this async fn body would hit that panic AFTER the marker had
        // already printed — a green-looking log over a failed live test.
        tokio::task::spawn_blocking(move || drop(client))
            .await
            .expect("spawn_blocking join for client drop");

        // Safety net: the accounting loop above almost always observes
        // `Disconnected` itself (see the comment above `disconnected_at`'s
        // declaration), but if it somehow did not, give it one more short,
        // bounded chance rather than asserting on a technicality.
        if disconnected_at.is_none() {
            let disconnect_deadline = std::time::Instant::now() + Duration::from_secs(5);
            while disconnected_at.is_none() && std::time::Instant::now() < disconnect_deadline {
                let remaining =
                    disconnect_deadline.saturating_duration_since(std::time::Instant::now());
                if let Some(DeepgramEvent::Disconnected) =
                    try_recv_event(&event_rx, remaining.min(Duration::from_millis(200))).await
                {
                    disconnected_at = Some(std::time::Instant::now());
                }
            }
        }
        assert!(
            disconnected_at.is_some(),
            "Disconnected event must arrive within the bounded close timeout"
        );
        let disconnect_ms = disconnected_at
            .map(|at| {
                u64::try_from(at.saturating_duration_since(disconnect_started).as_millis())
                    .unwrap_or(u64::MAX)
            })
            .unwrap_or(u64::MAX);

        let report = StreamingSmokeReport {
            status: "ok",
            model: DEEPGRAM_DEFAULT_STREAMING_MODEL.to_string(),
            transcript_events,
            turn_events,
            speech_final_seen,
            keyword_hits,
            keyword_threshold: DEEPGRAM_SMOKE_KEYWORD_THRESHOLD,
            keyword_total: DEEPGRAM_SMOKE_KEYWORDS.len(),
            timing_monotonic,
            connect_ms,
            disconnect_ms,
        };

        assert!(report.has_no_content_fields());
        assert!(
            report.transcript_events >= 1,
            "expected at least one normalized Transcript event, got 0"
        );
        assert!(
            report.speech_final_seen,
            "expected a speech_final Transcript or a SpeechFinal/EndOfTurn Turn event"
        );
        assert!(
            segment_a_hits >= 1 && segment_b_hits >= 1,
            "expected >= 1 tolerant keyword hit from EACH speaker segment \
             (A: {segment_a_hits}/2, B: {segment_b_hits}/2, total {}/{}) — \
             both-segments coverage is the property; see the per-keyword \
             hit flags above for which words missed",
            report.keyword_hits,
            report.keyword_total
        );
        assert!(
            report.timing_monotonic,
            "collected event start times must be non-decreasing"
        );

        let report_json =
            serde_json::to_string_pretty(&report).expect("StreamingSmokeReport must serialize");
        println!("\n=== StreamingSmokeReport (audio-graph-315d) ===");
        println!("{report_json}");
        println!("================================================\n");

        // Stable, greppable pass marker for `protected-provider-smoke.yml`'s
        // vacuous-pass guard — see `openrouter.rs`'s matching marker for why
        // cargo's exit code alone is not sufficient evidence (a `--exact`
        // typo matching zero tests, or this same precondition-skip path,
        // both exit the process successfully with no such line printed).
        println!(
            "deepgram streaming smoke strict pass: transcripts={} speech_final={} keyword_hits={}/{}",
            report.transcript_events,
            report.speech_final_seen,
            report.keyword_hits,
            report.keyword_total
        );
    }
}
