//! Deepgram Aura streaming TTS WebSocket client.
//!
//! Connects to `wss://api.deepgram.com/v1/speak` and streams server-rendered
//! PCM audio back as a stream of [`super::TtsEvent`]s. Implements the
//! [`super::TtsProvider`] / [`super::TtsSession`] traits.
//!
//! # Protocol summary (verified 2026-05-19)
//!
//! - WebSocket endpoint: `wss://api.deepgram.com/v1/speak`
//! - Auth: `Authorization: Token <api_key>` header on the upgrade.
//! - Default voice: `aura-asteria-en`. Default `encoding=linear16`,
//!   `sample_rate=24000`. Streaming-compatible encodings: linear16, mulaw,
//!   alaw.
//! - Client to server text frames:
//!   - `{"type":"Speak","text":"..."}` (max 2000 chars per frame)
//!   - `{"type":"Flush"}` (force-render anything buffered)
//!   - `{"type":"Clear"}` (cancel in-flight utterance)
//!   - `{"type":"Close"}` (graceful shutdown)
//!   - `{"type":"KeepAlive"}` (server idle-disconnects ~10s)
//! - Server to client:
//!   - Binary frames: raw PCM (i16 LE for `linear16`).
//!   - Text frames: JSON `Metadata`, `Flushed`, `Cleared`, `Warning`,
//!     `Error`.
//!
//! # Threading model
//!
//! Mirrors `crate::asr::deepgram` and `crate::gemini`: a dedicated tokio
//! runtime owns the socket; a session task drives reader+writer concurrently;
//! audio commands flow in through an `UnboundedSender`; events flow out
//! through a `tokio::sync::mpsc::UnboundedSender`-backed `futures_util::Stream`
//! the caller drains.
//!
//! # Reconnect with backoff
//!
//! Same ladder as the ASR / Gemini clients: 1s, 2s, 5s, 10s, then a fatal
//! error. Plus or minus 20% jitter is layered on each step using a low-quality
//! clock-derived pseudo-random source (no `rand` dependency). Audio commands
//! buffered while the socket is down are flushed once the new socket opens.

use crate::credentials::CredentialStore;
use async_trait::async_trait;
use audio_graph_ipc_contract::credential_contract::{
    CredentialError, CredentialErrorCode, CredentialSafeRecoveryAction,
};
use audio_graph_ipc_contract::runtime_diagnostic::{
    RuntimeDiagnosticContext, RuntimeErrorCode as Code, RuntimeOperation, RuntimeTransport,
};
use futures_util::{SinkExt, Stream, StreamExt};
use serde_json::Value;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::{self, Message};

use super::{
    TtsConfig, TtsError, TtsErrorKind as Kind, TtsEvent, TtsProvider, TtsSession, TtsStatus,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const AURA_POLICY_PROVIDER: &str = "tts.deepgram_aura";
const EXPLICIT_POLICY_REQUIRED: &str = "explicit_policy_required";

/// Aura idle-disconnect window is around 10s; the ADR mandates 8s for the
/// keepalive cadence -- we honour that exactly.
///
/// MARGIN RATIONALE (review n3): the Deepgram *listen* (ASR) socket uses a
/// tighter 4s cadence (`asr::deepgram::KEEPALIVE_INTERVAL_SECS`) against the
/// same vendor's ~10s idle window. The margins differ deliberately by
/// direction. On the ASR path we are the *sender*: any late/dropped keepalive
/// costs a reconnect that re-buffers live mic audio, so the 6s slack (10 − 4)
/// buys headroom against send-scheduling jitter under CPU load. On this TTS
/// path the socket is short-lived and driven by our own synthesis cadence — a
/// missed keepalive at worst forces a cheap re-open of an idle speaker socket
/// with no live audio to lose — so 8s (matching the ADR) is an acceptable
/// looser margin. Same vendor, same idle window, two directional choices.
const KEEPALIVE_INTERVAL_SECS: u64 = 8;
const KEEPALIVE_PAYLOAD: &str = r#"{"type":"KeepAlive"}"#;

/// Wire frames the client produces. JSON only -- never binary.
const FRAME_FLUSH: &str = r#"{"type":"Flush"}"#;
const FRAME_CLEAR: &str = r#"{"type":"Clear"}"#;
const FRAME_CLOSE: &str = r#"{"type":"Close"}"#;

/// Max chars per `Speak` frame. Aura-1 + Aura-2 cap is 2000.
const MAX_SPEAK_CHARS: usize = 2000;

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// Provider handle for Deepgram Aura. Holds the API key and any future
/// shared state. The same Deepgram key works for both STT (`asr/deepgram.rs`)
/// and TTS -- we share the credential slot rather than introducing a separate
/// `deepgram_tts_api_key`. See ADR-0004 reuse decision.
#[derive(Clone)]
pub struct DeepgramAuraProvider {
    api_key: String,
    content_egress_policy: crate::asr::ProviderContentEgressPolicy,
}

impl std::fmt::Debug for DeepgramAuraProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepgramAuraProvider")
            .field("api_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl DeepgramAuraProvider {
    /// Construct from a raw API key.
    pub fn new(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::block(
                EXPLICIT_POLICY_REQUIRED,
            ),
        }
    }

    /// Override the runtime content-egress policy.
    pub fn with_content_egress_policy(
        mut self,
        policy: crate::asr::ProviderContentEgressPolicy,
    ) -> Self {
        self.content_egress_policy = policy;
        self
    }

    /// Convenience: pull the Deepgram API key from `credentials.yaml` (the
    /// same slot used by `asr/deepgram.rs`). Returns `Auth` if absent.
    pub fn from_store(store: &CredentialStore) -> Result<Self, TtsError> {
        match store.deepgram_api_key.as_deref().map(str::trim) {
            Some(key) if !key.is_empty() => Ok(Self::new(key.to_string())),
            _ => Err(TtsError::new(
                Kind::Auth,
                CredentialError {
                    code: CredentialErrorCode::Missing,
                    retryable: false,
                    recovery_action: CredentialSafeRecoveryAction::ReenterCredential,
                    set_id: None,
                },
            )),
        }
    }
}

#[async_trait]
impl TtsProvider for DeepgramAuraProvider {
    async fn open(&self, voice: &str, config: TtsConfig) -> Result<Box<dyn TtsSession>, TtsError> {
        let mut effective = config.with_clamped_speed();
        if !voice.is_empty() {
            effective.voice = voice.to_string();
        }
        let session =
            AuraSession::open(self.api_key.clone(), effective, self.content_egress_policy).await?;
        Ok(Box::new(session))
    }
}

// ---------------------------------------------------------------------------
// Session command channel
// ---------------------------------------------------------------------------

/// Internal message passed from the sync `TtsSession` methods into the async
/// session task. Matches the audio command pattern from
/// `asr/deepgram.rs::AudioCmd` but specialized for TTS.
enum SessionCmd {
    /// `{"type":"Speak","text":"<text>"}`
    Speak(String),
    /// `{"type":"Flush"}` -- paired with the dispatch-time client sequence
    /// number so rapid consecutive flushes produce distinct ack events.
    Flush(u64),
    /// `{"type":"Clear"}`
    Clear,
    /// `{"type":"Close"}` -- graceful shutdown; exits the session task.
    Close,
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Live Aura session. Constructed via [`DeepgramAuraProvider::open`] in
/// production; tests construct via `AuraSession::start_with_url` (a
/// `#[cfg(test)]` entrypoint, hence not an intra-doc link) against a local mock
/// server.
pub struct AuraSession {
    cmd_tx: tokio_mpsc::UnboundedSender<SessionCmd>,
    events: Option<Pin<Box<dyn Stream<Item = TtsEvent> + Send>>>,
    closed: Arc<AtomicBool>,
    content_egress_policy: crate::asr::ProviderContentEgressPolicy,
    /// Increments per `flush()` call. Server-side `Flushed` JSON has no
    /// sequence number, so we attach our own monotonic counter to the
    /// outbound `Flushed` status event.
    flush_seq: Arc<AtomicU64>,
    /// Owned cancellation flag. Set to `true` on `close()` / `Drop` to break
    /// the session-task loop.
    user_closed: Arc<AtomicBool>,
}

impl AuraSession {
    /// Production entrypoint: connect and spawn the session task on the
    /// caller's tokio runtime.
    ///
    /// We deliberately do NOT create a per-session Runtime here. Owning one
    /// caused "Cannot drop a runtime in a context where blocking is not
    /// allowed" panics on Windows when an async test (or an async caller)
    /// dropped the AuraSession from inside the runtime's own context.
    /// Tauri commands always run under `tauri::async_runtime`, so a tokio
    /// reactor is in scope at every real call site.
    async fn open(
        api_key: String,
        config: TtsConfig,
        content_egress_policy: crate::asr::ProviderContentEgressPolicy,
    ) -> Result<Self, TtsError> {
        let url = build_aura_url(&config);
        let (writer, reader) = open_ws(&url, &api_key).await?;
        Ok(Self::spawn_session(
            writer,
            reader,
            url,
            api_key,
            config.sample_rate,
            content_egress_policy,
        ))
    }

    /// Test entrypoint: start a session against an arbitrary URL using
    /// the caller's tokio runtime.
    #[cfg(test)]
    pub(crate) async fn start_with_url(
        url: String,
        api_key: String,
        config: TtsConfig,
    ) -> Result<Self, TtsError> {
        // For tests we want both the connect and any reconnects to use the
        // explicit URL the caller passed (typically a localhost mock), not
        // the production Aura URL derived from config.
        let (writer, reader) = open_ws(&url, &api_key).await?;
        Ok(Self::spawn_session(
            writer,
            reader,
            url,
            api_key,
            config.sample_rate,
            crate::asr::ProviderContentEgressPolicy::block(EXPLICIT_POLICY_REQUIRED),
        ))
    }

    /// Test entrypoint with an explicit content-egress policy.
    #[cfg(test)]
    pub(crate) async fn start_with_url_and_content_egress_policy(
        url: String,
        api_key: String,
        config: TtsConfig,
        content_egress_policy: crate::asr::ProviderContentEgressPolicy,
    ) -> Result<Self, TtsError> {
        let (writer, reader) = open_ws(&url, &api_key).await?;
        Ok(Self::spawn_session(
            writer,
            reader,
            url,
            api_key,
            config.sample_rate,
            content_egress_policy,
        ))
    }

    fn spawn_session(
        writer: WsWriter,
        reader: WsReader,
        url: String,
        api_key: String,
        sample_rate: u32,
        content_egress_policy: crate::asr::ProviderContentEgressPolicy,
    ) -> Self {
        let (cmd_tx, cmd_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, event_rx) = tokio_mpsc::unbounded_channel();
        let closed = Arc::new(AtomicBool::new(false));
        let user_closed = Arc::new(AtomicBool::new(false));
        let flush_seq = Arc::new(AtomicU64::new(0));
        let clearing = Arc::new(AtomicBool::new(false));

        // Emit an immediate Connected event so consumers don't have to race
        // against the first server frame to observe the connect.
        let _ = event_tx.send(TtsEvent::Status(TtsStatus::Connected));

        let ctx = SessionCtx {
            writer,
            reader,
            cmd_rx,
            event_tx,
            url,
            api_key,
            user_closed: user_closed.clone(),
            closed: closed.clone(),
            clearing,
            sample_rate,
            content_egress_policy,
        };
        tokio::spawn(session_task(ctx));

        let event_stream = UnboundedReceiverStream { inner: event_rx };

        Self {
            cmd_tx,
            events: Some(Box::pin(event_stream)),
            closed,
            content_egress_policy,
            flush_seq,
            user_closed,
        }
    }

    fn send_cmd(&self, cmd: SessionCmd) -> Result<(), TtsError> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(aura_error(Kind::Network, Code::NetworkUnreachable));
        }
        self.cmd_tx
            .send(cmd)
            .map_err(|_| aura_error(Kind::Network, Code::NetworkUnreachable))
    }
}

#[async_trait]
impl TtsSession for AuraSession {
    fn speak(&self, text: &str) -> Result<(), TtsError> {
        if text.is_empty() {
            return Ok(());
        }
        self.content_egress_policy
            .check_text(AURA_POLICY_PROVIDER)
            .map_err(|_| aura_error(Kind::BadRequest, Code::PolicyBlocked))?;
        if text.chars().take(MAX_SPEAK_CHARS + 1).count() > MAX_SPEAK_CHARS {
            return Err(aura_error(Kind::BadRequest, Code::RequestRejected));
        }
        self.send_cmd(SessionCmd::Speak(text.to_string()))
    }

    fn flush(&self) -> Result<(), TtsError> {
        // Pre-increment so the caller can correlate the seq with the next
        // expected `Flushed` ack. Carry the dispatch-time value in the command
        // itself so rapid consecutive flushes cannot all report the latest
        // global counter value.
        let sequence = self.flush_seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.send_cmd(SessionCmd::Flush(sequence))
    }

    fn clear(&self) -> Result<(), TtsError> {
        self.send_cmd(SessionCmd::Clear)
    }

    fn close(&self) -> Result<(), TtsError> {
        self.user_closed.store(true, Ordering::SeqCst);
        // Ignore send error: if the channel is already closed, the session
        // task has exited and the close has effectively already happened.
        let _ = self.cmd_tx.send(SessionCmd::Close);
        Ok(())
    }

    fn take_events(&mut self) -> Option<super::TtsEventStream> {
        self.events.take()
    }
}

impl Drop for AuraSession {
    /// Drop signals close + lets the spawned session task wind down on the
    /// caller's tokio runtime. We deliberately don't block on the task — the
    /// session task observes `user_closed` and exits cleanly via its existing
    /// `select!` loop. Blocking on shutdown here would re-introduce the
    /// "cannot drop a runtime in a context where blocking is not allowed"
    /// panic when AuraSession is dropped from inside an async test.
    fn drop(&mut self) {
        self.user_closed.store(true, Ordering::SeqCst);
        let _ = self.cmd_tx.send(SessionCmd::Close);
    }
}

// ---------------------------------------------------------------------------
// Stream adapter
// ---------------------------------------------------------------------------

/// Light wrapper over `tokio::sync::mpsc::UnboundedReceiver` implementing
/// [`futures_util::Stream`] without pulling `tokio-stream` in. Mirrors the
/// inline adapter pattern we use elsewhere in the workspace.
struct UnboundedReceiverStream {
    inner: tokio_mpsc::UnboundedReceiver<TtsEvent>,
}

impl Stream for UnboundedReceiverStream {
    type Item = TtsEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.poll_recv(cx)
    }
}

// ---------------------------------------------------------------------------
// URL + WebSocket open
// ---------------------------------------------------------------------------

type WsWriter = futures_util::stream::SplitSink<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    Message,
>;

type WsReader = futures_util::stream::SplitStream<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
>;

/// Construct the Aura WebSocket URL from a [`TtsConfig`].
pub(crate) fn build_aura_url(config: &TtsConfig) -> String {
    // Speed is a query parameter on Aura's wire surface. Always send the
    // post-clamp value to avoid 4xx from the server's own validator.
    let clamped_speed = config.clone().with_clamped_speed().speed;
    format!(
        "wss://api.deepgram.com/v1/speak?model={}&encoding={}&sample_rate={}&speed={:.2}",
        config.voice,
        config.encoding.wire_name(),
        config.sample_rate,
        clamped_speed,
    )
}

/// Open a fresh WebSocket against `url` with `Token <api_key>` auth.
/// Used both for the initial connect and for reconnect attempts.
pub(crate) async fn open_ws(url: &str, api_key: &str) -> Result<(WsWriter, WsReader), TtsError> {
    let api_key = api_key.trim();
    let credential_sent = !api_key.is_empty();
    let host = extract_host(url).unwrap_or_else(|| "api.deepgram.com".to_string());
    let mut request = tungstenite::http::Request::builder()
        .uri(url)
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .header("Sec-WebSocket-Version", "13")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Host", host)
        .body(())
        .map_err(|_| aura_error(Kind::BadRequest, Code::RequestRejected))?;
    if credential_sent {
        let auth = format!("Token {api_key}")
            .parse()
            .map_err(|_| aura_error(Kind::BadRequest, Code::RequestRejected))?;
        request.headers_mut().insert("Authorization", auth);
    }

    // Bounded connect so a stalled TLS/HTTP-upgrade handshake surfaces as an
    // ordinary transport error (routed through `classify_tungstenite_error`
    // below, driving the reconnect ladder) instead of hanging forever.
    match crate::ws_request::connect_async_bounded(request).await {
        Ok((ws_stream, response)) => {
            let status = response.status().as_u16();
            // Tungstenite only returns Ok on a 101 upgrade, but be defensive.
            if status != 101 {
                return Err(TtsError::from_http_status(status, credential_sent));
            }
            Ok(ws_stream.split())
        }
        Err(tungstenite::Error::Http(response)) => {
            if credential_sent
                && let Some(diagnostic) = crate::asr::deepgram::deepgram_http_auth_diagnostic(
                    &response,
                    RuntimeDiagnosticContext::new(RuntimeOperation::SpeechSynthesis)
                        .with_transport(RuntimeTransport::Http),
                )
            {
                return Err(TtsError::new(Kind::Auth, diagnostic));
            }
            Err(TtsError::from_http_status(
                response.status().as_u16(),
                credential_sent,
            ))
        }
        Err(e) => Err(classify_tungstenite_error(&e)),
    }
}

fn extract_host(url: &str) -> Option<String> {
    let stripped = url
        .strip_prefix("wss://")
        .or_else(|| url.strip_prefix("ws://"))?;
    let end = stripped.find(['/', '?']).unwrap_or(stripped.len());
    Some(stripped[..end].to_string())
}

fn aura_error(kind: Kind, code: Code) -> TtsError {
    TtsError::runtime(
        kind,
        code,
        RuntimeDiagnosticContext::new(RuntimeOperation::SpeechSynthesis)
            .with_transport(RuntimeTransport::Websocket),
    )
}

fn classify_tungstenite_error(error: &tungstenite::Error) -> TtsError {
    use tungstenite::Error;

    match error {
        Error::Io(io) if io.kind() == std::io::ErrorKind::TimedOut => {
            aura_error(Kind::Network, Code::Timeout)
        }
        Error::Io(_) | Error::Tls(_) | Error::ConnectionClosed | Error::AlreadyClosed => {
            aura_error(Kind::Network, Code::NetworkUnreachable)
        }
        Error::Url(_) | Error::HttpFormat(_) => aura_error(Kind::BadRequest, Code::RequestRejected),
        Error::Protocol(_) | Error::Capacity(_) => aura_error(Kind::Protocol, Code::ProtocolError),
        _ => aura_error(Kind::Unknown, Code::Internal),
    }
}

// ---------------------------------------------------------------------------
// Backoff
// ---------------------------------------------------------------------------

// The 1/2/5/10s backoff schedule + optional ±20% jitter now live in the shared
// crate-level reconnect ladder (review n2). Re-exported here under the names the
// Aura session task already uses.
pub(crate) use crate::reconnect::{backoff_for_attempt, jittered_backoff};

// ---------------------------------------------------------------------------
// Session task
// ---------------------------------------------------------------------------

struct SessionCtx {
    writer: WsWriter,
    reader: WsReader,
    cmd_rx: tokio_mpsc::UnboundedReceiver<SessionCmd>,
    event_tx: tokio_mpsc::UnboundedSender<TtsEvent>,
    url: String,
    api_key: String,
    user_closed: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    /// Set to `true` when SessionCmd::Clear is dispatched; cleared when the
    /// server's `Cleared` ack arrives. While set, AudioChunk frames received
    /// from the server are suppressed at the session layer — they belong to
    /// the cancelled utterance and would cause audible "tail" artifacts on
    /// barge-in if forwarded. See ADR-0004 + audio-graph-7107.
    clearing: Arc<AtomicBool>,
    /// Sample rate the AudioChunk events advertise. Sourced from
    /// `TtsConfig::sample_rate` at session-open; matches the WS URL query
    /// param (`?sample_rate=...`) so consumers see consistent values.
    sample_rate: u32,
    /// Defense-in-depth content-egress guard. `AuraSession::speak` already
    /// refuses to enqueue a `Speak` command in a blocked privacy mode; carrying
    /// the policy into the session task gives a SECOND layer so a direct caller
    /// that feeds a `Speak` command bypassing `speak` still cannot ship
    /// synthesis text to Deepgram. Defaults to fail-closed via the surrounding
    /// session struct's policy.
    content_egress_policy: crate::asr::ProviderContentEgressPolicy,
}

#[derive(Debug)]
enum DisconnectKind {
    /// User asked to close. No reconnect.
    UserRequested,
    /// Provider close or transport failure. May reconnect unless user closed.
    Disconnected(TtsError),
    /// Command channel ran dry -- the session struct was dropped.
    CmdChannelEnded,
}

/// Background task driving the WebSocket reader and writer. Handles
/// reconnect with exponential backoff and plus-or-minus 20% jitter. See
/// module docs for the protocol surface this task speaks.
async fn session_task(ctx: SessionCtx) {
    let SessionCtx {
        writer: initial_writer,
        reader: initial_reader,
        mut cmd_rx,
        event_tx,
        url,
        api_key,
        user_closed,
        closed,
        clearing,
        sample_rate,
        content_egress_policy,
    } = ctx;

    let mut writer = initial_writer;
    let mut reader = initial_reader;
    let mut reconnect_attempts: u32 = 0;

    loop {
        let disconnect = run_io(
            &mut writer,
            &mut reader,
            &mut cmd_rx,
            &event_tx,
            &user_closed,
            &clearing,
            sample_rate,
            content_egress_policy,
        )
        .await;

        match disconnect {
            DisconnectKind::UserRequested | DisconnectKind::CmdChannelEnded => {
                let _ = event_tx.send(TtsEvent::Status(TtsStatus::Disconnected));
                break;
            }
            DisconnectKind::Disconnected(error) => {
                if user_closed.load(Ordering::SeqCst) {
                    let _ = event_tx.send(TtsEvent::Status(TtsStatus::Disconnected));
                    break;
                }
                log::warn!("Aura session disconnected: {error}");
                let _ = event_tx.send(TtsEvent::Status(TtsStatus::Disconnected));

                reconnect_attempts += 1;
                let Some(backoff_secs) = backoff_for_attempt(reconnect_attempts) else {
                    let _ = event_tx
                        .send(aura_error(Kind::Exhausted, Code::ProviderUnavailable).into());
                    break;
                };

                let _ = event_tx.send(TtsEvent::Status(TtsStatus::Reconnecting {
                    attempt: reconnect_attempts,
                    backoff_secs,
                }));

                let backoff = jittered_backoff(backoff_secs);
                // Sleep with cancellation: poll user_closed every 100ms so a
                // close() during backoff exits quickly instead of waiting up
                // to 12s.
                let sleep = tokio::time::sleep(backoff);
                tokio::pin!(sleep);
                let cancelled = loop {
                    tokio::select! {
                        _ = &mut sleep => break false,
                        _ = tokio::time::sleep(Duration::from_millis(100)) => {
                            if user_closed.load(Ordering::SeqCst) {
                                break true;
                            }
                        }
                    }
                };
                if cancelled {
                    let _ = event_tx.send(TtsEvent::Status(TtsStatus::Disconnected));
                    break;
                }

                match open_ws(&url, &api_key).await {
                    Ok((new_writer, new_reader)) => {
                        writer = new_writer;
                        reader = new_reader;
                        // Reset the Clear suppression flag on every successful
                        // reconnect. If the socket dropped after a Clear was
                        // sent but before the server's `Cleared` ack arrived,
                        // `clearing` would otherwise stay `true` forever — and
                        // since the new socket starts a fresh utterance with no
                        // pending Clear, every Binary frame would be suppressed
                        // permanently (silent audio after a barge-in-driven
                        // Clear + reconnect). A new socket has no in-flight
                        // cancelled utterance, so clearing must start `false`.
                        clearing.store(false, Ordering::SeqCst);
                        let _ = event_tx.send(TtsEvent::Status(TtsStatus::Reconnected));
                        reconnect_attempts = 0;
                    }
                    Err(e) => {
                        let _ = event_tx.send(e.into());
                        // Don't run_io with broken halves; loop directly to
                        // increment the backoff ladder.
                        continue;
                    }
                }
            }
        }
    }

    closed.store(true, Ordering::SeqCst);
    log::info!("Aura: session task exited");
}

/// Drive the WebSocket reader and writer concurrently for a single
/// connected socket. Returns the [`DisconnectKind`] when the loop ends.
#[allow(clippy::too_many_arguments)]
async fn run_io(
    writer: &mut WsWriter,
    reader: &mut WsReader,
    cmd_rx: &mut tokio_mpsc::UnboundedReceiver<SessionCmd>,
    event_tx: &tokio_mpsc::UnboundedSender<TtsEvent>,
    user_closed: &Arc<AtomicBool>,
    clearing: &Arc<AtomicBool>,
    sample_rate: u32,
    content_egress_policy: crate::asr::ProviderContentEgressPolicy,
) -> DisconnectKind {
    let mut keep_alive = tokio::time::interval(Duration::from_secs(KEEPALIVE_INTERVAL_SECS));
    keep_alive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Skip the immediate first tick -- Tokio interval fires once at t=0.
    keep_alive.tick().await;
    let mut last_outbound = tokio::time::Instant::now();
    let mut pending_flushes: VecDeque<u64> = VecDeque::new();

    loop {
        tokio::select! {
            _ = keep_alive.tick() => {
                if last_outbound.elapsed() >= Duration::from_secs(KEEPALIVE_INTERVAL_SECS) {
                    if let Err(e) = writer.send(Message::Text(KEEPALIVE_PAYLOAD.into())).await {
                        return DisconnectKind::Disconnected(classify_tungstenite_error(&e));
                    }
                    last_outbound = tokio::time::Instant::now();
                }
            }

            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(SessionCmd::Speak(text)) => {
                        // Defense-in-depth content-egress gate (second layer).
                        // `AuraSession::speak` already refuses to enqueue a
                        // Speak command in a blocked privacy mode; re-checking
                        // here means a direct caller that pushes a Speak command
                        // bypassing `speak` still cannot ship synthesis text to
                        // Deepgram. The payload `text` is NEVER interpolated into
                        // the error; we drop the frame WITHOUT tearing down the
                        // socket — a blocked policy is steady-state, not a
                        // transport failure to reconnect around.
                        if content_egress_policy
                            .check_text(AURA_POLICY_PROVIDER)
                            .is_err()
                        {
                            continue;
                        }
                        let payload = serde_json::json!({"type": "Speak", "text": text}).to_string();
                        if let Err(e) = writer.send(Message::Text(payload.into())).await {
                            return DisconnectKind::Disconnected(classify_tungstenite_error(&e));
                        }
                        last_outbound = tokio::time::Instant::now();
                    }
                    Some(SessionCmd::Flush(sequence)) => {
                        if let Err(e) = writer.send(Message::Text(FRAME_FLUSH.into())).await {
                            return DisconnectKind::Disconnected(classify_tungstenite_error(&e));
                        }
                        pending_flushes.push_back(sequence);
                        last_outbound = tokio::time::Instant::now();
                    }
                    Some(SessionCmd::Clear) => {
                        // Set the clearing flag BEFORE sending the Clear frame
                        // so any AudioChunk that arrives in the brief window
                        // before the server processes the Clear is suppressed.
                        clearing.store(true, Ordering::SeqCst);
                        if let Err(e) = writer.send(Message::Text(FRAME_CLEAR.into())).await {
                            return DisconnectKind::Disconnected(classify_tungstenite_error(&e));
                        }
                        last_outbound = tokio::time::Instant::now();
                    }
                    Some(SessionCmd::Close) => {
                        // Send Close, then DRAIN the server-rendered tail before
                        // tearing down the socket. `finish()` calls
                        // speak(tail) + flush() immediately before close(); if we
                        // closed the socket synchronously here the server would
                        // not have finished rendering the just-flushed clause and
                        // the last fragment would be truncated. Wait for the
                        // `Flushed` ack (the render of the final flush completed)
                        // or a short drain timeout, forwarding any audio that
                        // arrives in the meantime.
                        let _ = writer.send(Message::Text(FRAME_CLOSE.into())).await;
                        drain_until_flushed(
                            reader,
                            event_tx,
                            &mut pending_flushes,
                            clearing,
                            sample_rate,
                        )
                        .await;
                        let _ = writer.close().await;
                        return DisconnectKind::UserRequested;
                    }
                    None => {
                        let _ = writer.close().await;
                        return DisconnectKind::CmdChannelEnded;
                    }
                }
            }

            frame = reader.next() => {
                let Some(frame) = frame else {
                    return DisconnectKind::Disconnected(aura_error(
                        Kind::Network, Code::NetworkUnreachable,
                    ));
                };
                match frame {
                    Ok(Message::Binary(bytes)) => {
                        // Aura linear16: raw i16 LE PCM. Suppress emission
                        // while a Clear is in flight — frames received between
                        // the client-side Clear send and the server's Cleared
                        // ack belong to the cancelled utterance.
                        if clearing.load(Ordering::SeqCst) {
                            continue;
                        }
                        let samples = i16_le_bytes_to_samples(&bytes);
                        if !samples.is_empty() {
                            let _ = event_tx.send(TtsEvent::AudioChunk {
                                samples,
                                sample_rate,
                            });
                        }
                    }
                    Ok(Message::Text(text)) => {
                        handle_server_text(&text, event_tx, &mut pending_flushes, clearing);
                    }
                    Ok(Message::Close(frame)) => {
                        if user_closed.load(Ordering::SeqCst) {
                            return DisconnectKind::UserRequested;
                        }
                        let code = frame.map(|frame| u16::from(frame.code));
                        return DisconnectKind::Disconnected(classify_close(code));
                    }
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                    Err(e) => return DisconnectKind::Disconnected(classify_tungstenite_error(&e)),
                }
            }
        }
    }
}

/// Upper bound on how long [`drain_until_flushed`] waits for the final
/// `Flushed` ack after a `Close` is requested. Keeps a graceful close from
/// hanging if the server never acks (e.g. it already closed the socket).
const CLOSE_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

/// After a graceful `Close`, keep reading the socket until the server acks the
/// final `Flush` (so the last clause's rendered audio is forwarded) or a short
/// timeout elapses. Forwards `AudioChunk`s and processes text frames exactly
/// like [`run_io`] so the trailing audio isn't lost. Returns once a `Flushed`
/// text frame is seen, the socket ends, or [`CLOSE_DRAIN_TIMEOUT`] elapses.
async fn drain_until_flushed(
    reader: &mut WsReader,
    event_tx: &tokio_mpsc::UnboundedSender<TtsEvent>,
    pending_flushes: &mut VecDeque<u64>,
    clearing: &Arc<AtomicBool>,
    sample_rate: u32,
) {
    let deadline = tokio::time::Instant::now() + CLOSE_DRAIN_TIMEOUT;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        match tokio::time::timeout(remaining, reader.next()).await {
            // Timed out waiting for the next frame: stop draining.
            Err(_) => return,
            // Socket ended.
            Ok(None) => return,
            Ok(Some(Ok(Message::Binary(bytes)))) => {
                if clearing.load(Ordering::SeqCst) {
                    continue;
                }
                let samples = i16_le_bytes_to_samples(&bytes);
                if !samples.is_empty() {
                    let _ = event_tx.send(TtsEvent::AudioChunk {
                        samples,
                        sample_rate,
                    });
                }
            }
            Ok(Some(Ok(Message::Text(text)))) => {
                let is_flushed = is_flushed_frame(&text);
                handle_server_text(&text, event_tx, pending_flushes, clearing);
                if is_flushed {
                    // Final flush rendered; the tail audio has been forwarded.
                    return;
                }
            }
            // Any close / error / other frame: stop draining and let the
            // caller tear the socket down.
            Ok(Some(Ok(Message::Close(_)))) => return,
            Ok(Some(Err(_))) => return,
            Ok(Some(Ok(_))) => {}
        }
    }
}

/// True when `text` is an Aura `Flushed` ack frame.
fn is_flushed_frame(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(|t| t.as_str())
                .map(|s| s == "Flushed")
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Server message parsing
// ---------------------------------------------------------------------------

/// Parse a single server JSON text frame into a closed control-plane event.
pub(crate) fn handle_server_text(
    text: &str,
    event_tx: &tokio_mpsc::UnboundedSender<TtsEvent>,
    pending_flushes: &mut VecDeque<u64>,
    clearing: &Arc<AtomicBool>,
) {
    let Ok(parsed) = serde_json::from_str::<Value>(text) else {
        let _ = event_tx.send(aura_error(Kind::Protocol, Code::InvalidResponse).into());
        return;
    };

    let event = match parsed.get("type").and_then(Value::as_str) {
        Some("Metadata") => TtsEvent::Status(TtsStatus::Metadata),
        Some("Flushed") => {
            // The server leaves the sequence untagged. Zero marks an unsolicited ack.
            let sequence = pending_flushes.pop_front().unwrap_or(0);
            TtsEvent::Status(TtsStatus::Flushed { sequence })
        }
        Some("Cleared") => {
            clearing.store(false, Ordering::SeqCst);
            TtsEvent::Status(TtsStatus::Cleared)
        }
        Some("Warning") => {
            let (_, code) = classify_aura_code(&parsed);
            aura_error(Kind::Unknown, code).into()
        }
        Some("Error") => {
            let (kind, code) = classify_aura_code(&parsed);
            aura_error(kind, code).into()
        }
        _ => {
            log::debug!("Aura unhandled text frame");
            return;
        }
    };
    let _ = event_tx.send(event);
}

fn classify_aura_code(frame: &Value) -> (Kind, Code) {
    let code = ["code", "error_code", "err_code", "warn_code"]
        .into_iter()
        .find_map(|key| frame.get(key).and_then(Value::as_str));
    // Exact tokens from Deepgram's API and TTS WebSocket references.
    // Descriptions and unknown code strings never participate in diagnostics.
    match code {
        // https://developers.deepgram.com/docs/errors
        Some("INVALID_AUTH") => (Kind::Auth, Code::CredentialRejected),
        Some("INSUFFICIENT_PERMISSIONS") => (Kind::Auth, Code::PermissionDenied),
        Some("MESSAGE-0000" | "DATA-0000" | "BIG-0000" | "BIG-0001" | "TEXT_LENGTH_WARNING") => {
            (Kind::BadRequest, Code::RequestRejected)
        }
        Some("DATA-0001" | "TOO_MANY_REQUESTS") => (Kind::RateLimit, Code::RateLimited),
        Some("NET-0000") => (Kind::Server, Code::ProviderUnavailable),
        Some("NET-0001" | "NET-0002") => (Kind::Network, Code::NetworkUnreachable),
        Some("NET-0003") => (Kind::Network, Code::Timeout),
        // An unclassified Error still has the terminal behavior of a server error.
        _ => (Kind::Server, Code::Internal),
    }
}

fn classify_close(code: Option<u16>) -> TtsError {
    let (kind, code) = match code {
        None | Some(1000 | 1001 | 1006) => (Kind::Network, Code::NetworkUnreachable),
        Some(1002 | 1007 | 1010) => (Kind::Protocol, Code::ProtocolError),
        Some(1003 | 1008 | 1009) => (Kind::BadRequest, Code::RequestRejected),
        Some(1011 | 1012) => (Kind::Server, Code::ProviderUnavailable),
        Some(1013) => (Kind::RateLimit, Code::RateLimited),
        _ => (Kind::Unknown, Code::Internal),
    };
    aura_error(kind, code)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Convert a byte buffer of i16 LE PCM into a `Vec<i16>` ready for playback.
/// Drops any trailing odd byte (the server should never produce one, but be
/// robust against truncation).
fn i16_le_bytes_to_samples(bytes: &[u8]) -> Vec<i16> {
    let n = bytes.len() / 2;
    let mut out = Vec::with_capacity(n);
    for chunk in bytes.chunks_exact(2) {
        out.push(i16::from_le_bytes([chunk[0], chunk[1]]));
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::test_support::{assert_no_canaries, capture_tts_logs, take_tts_logs};
    use super::*;
    use audio_graph_ipc_contract::runtime_diagnostic::RuntimeDiagnostic;
    use tungstenite::protocol::{CloseFrame, frame::coding::CloseCode};
    // SinkExt / StreamExt imports are required for `.send()` / `.next()` method
    // resolution on tokio_tungstenite WebSocketStream halves; rustc's
    // `unused_imports` lint flags them under the `as _` alias even though they
    // ARE used at call sites. Allow + named imports to silence cleanly.
    #[allow(unused_imports)]
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Bind a listener and return both the listener and url so the test can
    /// accept connections.
    async fn bind_keep() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let url = format!("ws://{addr}/v1/speak");
        (listener, url)
    }

    async fn start_allowed_session(url: String) -> Result<AuraSession, TtsError> {
        AuraSession::start_with_url_and_content_egress_policy(
            url,
            "test-key".into(),
            TtsConfig::default(),
            crate::asr::ProviderContentEgressPolicy::allow(),
        )
        .await
    }

    #[test]
    fn build_aura_url_matches_verified_protocol() {
        let cfg = TtsConfig::default();
        let url = build_aura_url(&cfg);
        assert!(url.starts_with("wss://api.deepgram.com/v1/speak?"));
        assert!(url.contains("model=aura-asteria-en"));
        assert!(url.contains("encoding=linear16"));
        assert!(url.contains("sample_rate=24000"));
        assert!(url.contains("speed=1.00"));
    }

    #[test]
    fn build_aura_url_uses_clamped_speed() {
        let cfg = TtsConfig {
            speed: 5.0,
            ..TtsConfig::default()
        };
        let url = build_aura_url(&cfg);
        assert!(url.contains("speed=1.50"), "url: {url}");
    }

    #[test]
    fn backoff_schedule_matches_spec() {
        // Shared crate-level ladder (review n2): fast head + cold-restart tail
        // (review m1). Aura wraps each rung through `jittered_backoff`.
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), Some(20));
        assert_eq!(backoff_for_attempt(11), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_ws_http_rejection_discards_route_headers_and_body() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        capture_tts_logs();
        let route = "/v1/speak?api_key=query-secret-12345&model=aura-generated-test";
        let url = format!("ws://{addr}{route}");
        let api_key = "example credential";
        let body = r#"{"err_code":"INSUFFICIENT_PERMISSIONS","error":"provider body text","description":"generated speech text 音声","api_key":"body-secret-12345","request_id":"dg-body-req"}"#;
        let body_for_server = body.to_string();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut request = Vec::new();
            let mut buf = [0u8; 512];
            loop {
                let n = stream.read(&mut buf).await.expect("read request");
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 401 Unauthorized\r\n\
                 Content-Type: application/json\r\n\
                 Content-Length: {}\r\n\
                 X-Request-Id: dg-header-req_7\r\n\
                 Dg-Request-Id: dg-header-req_9\r\n\
                 Connection: close\r\n\
                 \r\n\
                 {}",
                body_for_server.len(),
                body_for_server
            );
            stream
                .write_all(response.as_bytes())
                .await
                .expect("write response");
        });

        let err = match open_ws(&url, api_key).await {
            Ok(_) => panic!("handshake must reject"),
            Err(err) => err,
        };
        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server timeout")
            .expect("server task");
        assert_eq!(err.kind(), Kind::Auth);
        assert_eq!(
            err.diagnostic(),
            TtsError::from_http_status(403, true).diagnostic()
        );
        assert_eq!(err.to_string(), "runtime/permission_denied");
        let event: TtsEvent = err.clone().into();
        let exported = format!(
            "{err} {err:?} {event:?} {} {} {:?}",
            serde_json::to_string(&err).unwrap(),
            serde_json::to_string(&event).unwrap(),
            take_tts_logs(),
        );
        assert_no_canaries(
            &exported,
            &[
                route,
                "/v1/speak",
                "401",
                "dg-header-req_7",
                "dg-header-req_9",
                "dg-body-req",
                "query-secret-12345",
                "aura-generated-test",
                "provider body text",
                "generated speech text",
                "body-secret-12345",
                api_key,
                &body.len().to_string(),
                &body.chars().count().to_string(),
            ],
        );
    }

    #[test]
    fn tungstenite_error_classifier_discards_native_prose() {
        let native = "native-websocket-error-canary /v1/speak?token=native-secret";
        let err = tungstenite::Error::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, native));
        let classified = classify_tungstenite_error(&err);

        assert_eq!(classified.kind(), Kind::Network);
        assert_eq!(classified.to_string(), "runtime/timeout");
        assert_no_canaries(
            &format!(
                "{classified} {classified:?} {}",
                serde_json::to_string(&classified).unwrap()
            ),
            &[
                native,
                "native-websocket-error-canary",
                "/v1/speak",
                "native-secret",
            ],
        );
    }

    #[test]
    fn websocket_and_text_limits_report_transport_and_request_failures() {
        use tungstenite::error::CapacityError;

        let error = tungstenite::Error::Capacity(CapacityError::MessageTooLong {
            size: 691,
            max_size: 379,
        });
        let diagnostic = classify_tungstenite_error(&error);
        assert_eq!(diagnostic.kind(), Kind::Protocol);
        assert_eq!(diagnostic.to_string(), "runtime/protocol_error");
        assert_no_canaries(&format!("{diagnostic:?} {diagnostic}"), &["691", "379"]);

        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        handle_server_text(
            r#"{"type":"Warning","warn_code":"TEXT_LENGTH_WARNING","description":"private text note"}"#,
            &tx,
            &mut VecDeque::new(),
            &Arc::new(AtomicBool::new(false)),
        );
        let event = rx.try_recv().expect("warning diagnostic");
        assert_error_event(&event, Kind::Unknown, Code::RequestRejected);
        assert_no_canaries(&format!("{event:?}"), &["private text note"]);
    }

    #[test]
    fn server_frames_discard_provider_content() {
        capture_tts_logs();
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let mut pending_flushes = VecDeque::new();
        let clearing = Arc::new(AtomicBool::new(false));
        let unknown_type = "aura-unknown-frame-canary";
        let unknown_code = "aura-unknown-code-canary";
        let request_id = "aura-frame-request-id-canary";
        let secret = "aura-frame-secret-canary";
        let description =
            format!("authentication rate limit text 音声 {secret} /v1/speak?test=private")
                .repeat(11);
        let malformed = format!(r#"{{"secret":"{secret}",{description}"#);

        for (frame, kind, code) in [
            ("Warning", Kind::Unknown, Code::ProviderUnavailable),
            ("Error", Kind::Server, Code::Internal),
            (unknown_type, Kind::Unknown, Code::Internal),
        ] {
            let wire_code = if frame == "Warning" {
                "NET-0000"
            } else {
                unknown_code
            };
            handle_server_text(
                &serde_json::json!({
                    "type": frame,
                    "code": wire_code,
                    "request_id": request_id,
                    "description": description,
                    "message": description,
                })
                .to_string(),
                &tx,
                &mut pending_flushes,
                &clearing,
            );
            if frame == unknown_type {
                assert!(rx.try_recv().is_err());
            } else {
                let event = rx.try_recv().expect("diagnostic event");
                assert_error_event(&event, kind, code);
                assert_no_canaries(
                    &format!("{event:?} {}", serde_json::to_string(&event).unwrap()),
                    &[
                        secret,
                        request_id,
                        unknown_code,
                        "/v1/speak",
                        &description.len().to_string(),
                        &description.chars().count().to_string(),
                    ],
                );
            }
        }

        handle_server_text(&malformed, &tx, &mut pending_flushes, &clearing);
        let event = rx.try_recv().expect("malformed frame diagnostic");
        assert_error_event(&event, Kind::Protocol, Code::InvalidResponse);
        assert_no_canaries(
            &format!("{event:?} {}", serde_json::to_string(&event).unwrap()),
            &[
                secret,
                "/v1/speak",
                &description.len().to_string(),
                "column",
            ],
        );

        let logs = take_tts_logs().join("\n");
        assert!(
            logs.lines()
                .any(|line| line.ends_with("Aura unhandled text frame"))
        );
        assert_no_canaries(
            &logs,
            &[unknown_type, unknown_code, request_id, secret, "/v1/speak"],
        );
    }

    fn assert_error_event(event: &TtsEvent, expected_kind: Kind, expected_code: Code) {
        let TtsEvent::Error {
            kind,
            diagnostic: RuntimeDiagnostic::Runtime(diagnostic),
        } = event
        else {
            panic!("expected a runtime error event");
        };
        assert_eq!(*kind, expected_kind);
        assert_eq!(diagnostic.code, expected_code);
        assert_eq!(
            diagnostic.context.operation,
            RuntimeOperation::SpeechSynthesis
        );
        assert_eq!(
            diagnostic.context.transport,
            Some(RuntimeTransport::Websocket)
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn close_and_audio_diagnostics_discard_reason_and_exact_lengths() {
        capture_tts_logs();
        let (listener, url) = bind_keep().await;
        let reason = "close-reason-canary 音声 /v1/speak?key=close-secret";
        const SAMPLE_COUNT: usize = 1317;
        const SAMPLE_VALUE: i16 = -26173;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let bytes = SAMPLE_VALUE.to_le_bytes().repeat(SAMPLE_COUNT);
            ws.send(Message::Binary(bytes.into())).await.unwrap();
            ws.close(Some(CloseFrame {
                code: CloseCode::Error,
                reason: reason.into(),
            }))
            .await
            .unwrap();
        });

        let mut session = start_allowed_session(url).await.unwrap();
        let mut events = session.take_events().unwrap();
        let audio_debug = tokio::time::timeout(Duration::from_secs(2), async {
            let mut audio_debug = None;
            while let Some(event) = events.next().await {
                match event {
                    TtsEvent::AudioChunk { ref samples, .. } => {
                        assert_eq!(samples.len(), SAMPLE_COUNT);
                        assert!(samples.iter().all(|sample| *sample == SAMPLE_VALUE));
                        audio_debug = Some(format!("{event:?}"));
                    }
                    TtsEvent::Status(TtsStatus::Disconnected) => break,
                    _ => {}
                }
            }
            audio_debug.expect("audio before close")
        })
        .await
        .expect("disconnect before timeout");
        session.close().unwrap();
        server.await.unwrap();

        let logs = take_tts_logs().join("\n");
        assert!(logs.contains("Aura session disconnected: runtime/provider_unavailable"));
        assert!(audio_debug.contains("AudioChunk"));
        assert_no_canaries(
            &format!("{logs} {audio_debug}"),
            &[
                reason,
                "close-reason-canary",
                "close-secret",
                "/v1/speak",
                "reason_len",
                &reason.len().to_string(),
                &reason.chars().count().to_string(),
                &SAMPLE_COUNT.to_string(),
                &(SAMPLE_COUNT * 2).to_string(),
                &SAMPLE_VALUE.to_string(),
            ],
        );
    }

    #[test]
    fn websocket_close_classification_uses_closed_codes() {
        for (code, kind, token) in [
            (None, Kind::Network, "runtime/network_unreachable"),
            (Some(1000), Kind::Network, "runtime/network_unreachable"),
            (Some(1002), Kind::Protocol, "runtime/protocol_error"),
            (Some(1008), Kind::BadRequest, "runtime/request_rejected"),
            (Some(1011), Kind::Server, "runtime/provider_unavailable"),
            (Some(1013), Kind::RateLimit, "runtime/rate_limited"),
            (Some(4337), Kind::Unknown, "runtime/internal"),
        ] {
            let error = classify_close(code);
            assert_eq!(error.kind(), kind);
            assert_eq!(error.to_string(), token);
        }
    }

    #[test]
    fn jitter_stays_within_plus_minus_20_percent() {
        // Sample many times to cover the clock-derived randomness range.
        for _ in 0..200 {
            let d = jittered_backoff(10);
            // +/- 20% of 10s = 8s..=12s. Allow tiny rounding slop on the upper.
            assert!(
                d >= Duration::from_secs(8) && d <= Duration::from_millis(12_010),
                "jittered backoff outside +/- 20% range: {d:?}"
            );
        }
    }

    #[test]
    fn flush_command_carries_dispatch_time_sequence() {
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::unbounded_channel();
        let session = AuraSession {
            cmd_tx,
            events: None,
            closed: Arc::new(AtomicBool::new(false)),
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
            flush_seq: Arc::new(AtomicU64::new(0)),
            user_closed: Arc::new(AtomicBool::new(false)),
        };

        session.flush().expect("first flush");
        session.flush().expect("second flush");

        match cmd_rx.try_recv().expect("first command") {
            SessionCmd::Flush(sequence) => assert_eq!(sequence, 1),
            _ => panic!("expected first flush command"),
        }
        match cmd_rx.try_recv().expect("second command") {
            SessionCmd::Flush(sequence) => assert_eq!(sequence, 2),
            _ => panic!("expected second flush command"),
        }
    }

    #[test]
    fn speak_queues_text_when_content_egress_policy_allows() {
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::unbounded_channel();
        let session = AuraSession {
            cmd_tx,
            events: None,
            closed: Arc::new(AtomicBool::new(false)),
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
            flush_seq: Arc::new(AtomicU64::new(0)),
            user_closed: Arc::new(AtomicBool::new(false)),
        };

        session.speak("hello world").expect("allowed speak");

        match cmd_rx.try_recv().expect("speak command") {
            SessionCmd::Speak(text) => assert_eq!(text, "hello world"),
            _ => panic!("expected speak command"),
        }
    }

    #[test]
    fn speak_rejects_blocked_policy_without_queueing_or_leaking_content() {
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::unbounded_channel();
        let api_key = "dg-aura-secret-api-key";
        let generated_text = format!("generated patient text with secret {api_key}");
        let session = AuraSession {
            cmd_tx,
            events: None,
            closed: Arc::new(AtomicBool::new(false)),
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::block("local_only"),
            flush_seq: Arc::new(AtomicU64::new(0)),
            user_closed: Arc::new(AtomicBool::new(false)),
        };

        let err = session
            .speak(&generated_text)
            .expect_err("blocked policy rejects speak");
        assert_eq!(err.kind(), Kind::BadRequest);
        assert_eq!(err.to_string(), "runtime/policy_blocked");
        assert_no_canaries(
            &format!("{err} {err:?}"),
            &[&generated_text, api_key, AURA_POLICY_PROVIDER, "local_only"],
        );
        assert!(
            cmd_rx.try_recv().is_err(),
            "blocked speak must not queue SessionCmd::Speak"
        );
    }

    #[test]
    fn empty_speak_is_noop_even_when_policy_blocks_text() {
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::unbounded_channel();
        let session = AuraSession {
            cmd_tx,
            events: None,
            closed: Arc::new(AtomicBool::new(false)),
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::block("local_only"),
            flush_seq: Arc::new(AtomicU64::new(0)),
            user_closed: Arc::new(AtomicBool::new(false)),
        };

        session.speak("").expect("empty speak is a no-op");

        assert!(
            cmd_rx.try_recv().is_err(),
            "empty speak should not queue a provider frame"
        );
    }

    #[test]
    fn handle_server_text_emits_metadata_without_provider_fields() {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        handle_server_text(
            r#"{"type":"Metadata","request_id":"metadata-request-canary","model":"metadata-model-canary"}"#,
            &tx,
            &mut VecDeque::new(),
            &Arc::new(AtomicBool::new(false)),
        );
        let event = rx.try_recv().expect("metadata event");
        assert!(matches!(event, TtsEvent::Status(TtsStatus::Metadata)));
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "type": "status",
                "kind": "metadata",
            })
        );
    }

    #[test]
    fn handle_server_text_emits_flushed_with_client_sequence() {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let mut pending_flushes = VecDeque::from([3]);
        let clearing = Arc::new(AtomicBool::new(false));
        handle_server_text(
            r#"{"type":"Flushed"}"#,
            &tx,
            &mut pending_flushes,
            &clearing,
        );
        match rx.try_recv().expect("event") {
            TtsEvent::Status(TtsStatus::Flushed { sequence }) => assert_eq!(sequence, 3),
            _ => panic!("expected flushed"),
        }
    }

    #[test]
    fn handle_server_text_flushed_pops_dispatch_time_sequences_in_order() {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let mut pending_flushes = VecDeque::from([1, 2]);
        let clearing = Arc::new(AtomicBool::new(false));

        handle_server_text(
            r#"{"type":"Flushed"}"#,
            &tx,
            &mut pending_flushes,
            &clearing,
        );
        handle_server_text(
            r#"{"type":"Flushed"}"#,
            &tx,
            &mut pending_flushes,
            &clearing,
        );

        match rx.try_recv().expect("first event") {
            TtsEvent::Status(TtsStatus::Flushed { sequence }) => assert_eq!(sequence, 1),
            _ => panic!("expected first flushed"),
        }
        match rx.try_recv().expect("second event") {
            TtsEvent::Status(TtsStatus::Flushed { sequence }) => assert_eq!(sequence, 2),
            _ => panic!("expected second flushed"),
        }
        assert!(pending_flushes.is_empty());
    }

    #[test]
    fn handle_server_text_emits_cleared() {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let mut pending_flushes = VecDeque::new();
        let clearing = Arc::new(AtomicBool::new(true)); // start "clearing" so we can assert it resets
        handle_server_text(
            r#"{"type":"Cleared"}"#,
            &tx,
            &mut pending_flushes,
            &clearing,
        );
        match rx.try_recv().expect("event") {
            TtsEvent::Status(TtsStatus::Cleared) => {}
            _ => panic!("expected cleared"),
        }
        assert!(
            !clearing.load(Ordering::SeqCst),
            "Cleared ack must reset the clearing flag so subsequent AudioChunks flow"
        );
    }

    #[test]
    fn handle_server_text_classifies_allowlisted_codes_without_prose() {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        for (wire_code, kind, code) in [
            ("INVALID_AUTH", Kind::Auth, Code::CredentialRejected),
            (
                "INSUFFICIENT_PERMISSIONS",
                Kind::Auth,
                Code::PermissionDenied,
            ),
            ("DATA-0001", Kind::RateLimit, Code::RateLimited),
            ("BIG-0001", Kind::BadRequest, Code::RequestRejected),
            ("NET-0000", Kind::Server, Code::ProviderUnavailable),
            ("NET-0003", Kind::Network, Code::Timeout),
            ("DATA-0001-extra", Kind::Server, Code::Internal),
        ] {
            handle_server_text(
                &serde_json::json!({
                    "type": "Error",
                    "code": wire_code,
                    "description": "authentication failed",
                })
                .to_string(),
                &tx,
                &mut VecDeque::new(),
                &Arc::new(AtomicBool::new(false)),
            );
            assert_error_event(&rx.try_recv().unwrap(), kind, code);
        }
    }

    #[test]
    fn i16_decode_round_trips_through_le_bytes() {
        let samples = vec![0i16, 1, -1, i16::MAX, i16::MIN, 12345, -12345];
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let decoded = i16_le_bytes_to_samples(&bytes);
        assert_eq!(samples, decoded);
    }

    #[test]
    fn i16_decode_drops_trailing_odd_byte() {
        let bytes = vec![0u8, 1, 2, 3, 4]; // last byte ignored
        let decoded = i16_le_bytes_to_samples(&bytes);
        assert_eq!(decoded.len(), 2);
    }

    #[test]
    fn extract_host_strips_scheme_and_path() {
        assert_eq!(
            extract_host("wss://api.deepgram.com/v1/speak?x=1"),
            Some("api.deepgram.com".to_string())
        );
        assert_eq!(
            extract_host("ws://127.0.0.1:1234/v1/speak"),
            Some("127.0.0.1:1234".to_string())
        );
        assert_eq!(extract_host("http://wrong"), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn provider_reports_auth_when_credential_store_empty() {
        let store = CredentialStore::default();
        let err = DeepgramAuraProvider::from_store(&store).expect_err("must error");
        assert_eq!(err.kind(), Kind::Auth);
    }

    #[test]
    fn provider_content_policy_defaults_to_explicit_policy_required() {
        let provider = DeepgramAuraProvider::new("dg-private-test-key");

        let error = provider
            .content_egress_policy
            .check_text(AURA_POLICY_PROVIDER)
            .unwrap_err();

        assert!(error.contains("Privacy policy blocked text egress"));
        assert!(error.contains(AURA_POLICY_PROVIDER));
        assert!(error.contains(EXPLICIT_POLICY_REQUIRED));
        assert!(!error.contains("dg-private-test-key"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn default_session_policy_rejects_speak_without_queueing_text() {
        let (listener, url) = bind_keep().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("server handshake");
            let saw_speak = tokio::time::timeout(Duration::from_millis(200), ws.next())
                .await
                .ok()
                .flatten()
                .is_some_and(
                    |frame| matches!(frame, Ok(Message::Text(text)) if text.contains("\"Speak\"")),
                );
            let _ = ws.close(None).await;
            saw_speak
        });
        let session = AuraSession::start_with_url(url, "test-key".into(), TtsConfig::default())
            .await
            .expect("connect");

        let err = session
            .speak("private synthesis text")
            .expect_err("default policy rejects speak");
        assert_eq!(err.kind(), Kind::BadRequest);
        assert_eq!(err.to_string(), "runtime/policy_blocked");
        assert_no_canaries(
            &format!("{err} {err:?}"),
            &[
                AURA_POLICY_PROVIDER,
                EXPLICIT_POLICY_REQUIRED,
                "private synthesis text",
            ],
        );
        drop(session);
        let saw_speak = tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server timeout")
            .expect("server task");
        assert!(!saw_speak, "blocked speak must not send a Speak frame");
    }

    /// Defense-in-depth: drive `run_io` directly with a blocked content-egress
    /// policy and a pre-queued `Speak` command. The writer half must refuse to
    /// ship the Speak frame even though the command reached `run_io` WITHOUT
    /// passing through `AuraSession::speak` (which already gates enqueue). The
    /// server socket records whether it ever saw a `Speak` frame.
    #[tokio::test(flavor = "current_thread")]
    async fn run_io_blocked_policy_writes_no_speak_frame() {
        let (listener, url) = bind_keep().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("server handshake");
            let saw_speak = tokio::time::timeout(Duration::from_millis(200), ws.next())
                .await
                .ok()
                .flatten()
                .is_some_and(
                    |frame| matches!(frame, Ok(Message::Text(text)) if text.contains("\"Speak\"")),
                );
            let _ = ws.close(None).await;
            saw_speak
        });

        let (client_socket, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("client connect");
        let (mut writer, mut reader) = client_socket.split();
        let (cmd_tx, mut cmd_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, _event_rx) = tokio_mpsc::unbounded_channel();
        let user_closed = Arc::new(AtomicBool::new(false));
        let clearing = Arc::new(AtomicBool::new(false));

        // Push a Speak command carrying payload-like text directly (bypassing
        // `speak`), then a Close so `run_io` returns deterministically after
        // handling the (blocked) Speak.
        cmd_tx
            .send(SessionCmd::Speak("SECRET_SYNTHESIS_TEXT".into()))
            .expect("queue speak");
        cmd_tx.send(SessionCmd::Close).expect("queue close");
        drop(cmd_tx);

        let disconnect = run_io(
            &mut writer,
            &mut reader,
            &mut cmd_rx,
            &event_tx,
            &user_closed,
            &clearing,
            24000,
            crate::asr::ProviderContentEgressPolicy::block("local_only"),
        )
        .await;
        assert!(
            matches!(disconnect, DisconnectKind::UserRequested),
            "run_io should end via the Close command, got {disconnect:?}"
        );

        let saw_speak = tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server timeout")
            .expect("server task");
        assert!(
            !saw_speak,
            "blocked policy must not write a Speak frame to the socket"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn oversized_speak_discards_utterance_and_exact_length() {
        let (listener, url) = bind_keep().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("server handshake");
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = ws.close(None).await;
        });
        let session = start_allowed_session(url).await.expect("connect");
        let too_long = "utterance-secret-canary".repeat(137);
        let err = session.speak(&too_long).expect_err("must reject");
        assert_eq!(err.kind(), Kind::BadRequest);
        assert_eq!(err.to_string(), "runtime/request_rejected");
        assert_no_canaries(
            &format!("{err} {err:?} {}", serde_json::to_string(&err).unwrap()),
            &["utterance-secret-canary", &too_long.len().to_string()],
        );
        drop(session);
        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn speak_accepts_cjk_text_at_character_limit() {
        let (listener, url) = bind_keep().await;
        let text = "漢".repeat(MAX_SPEAK_CHARS);
        let expected = text.clone();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            let Some(Ok(Message::Text(frame))) = ws.next().await else {
                panic!("expected Speak frame");
            };
            assert_eq!(
                serde_json::from_str::<Value>(&frame).unwrap(),
                serde_json::json!({"type": "Speak", "text": expected})
            );
        });
        let session = start_allowed_session(url).await.unwrap();
        assert_eq!(
            session.speak(&format!("{text}漢")).unwrap_err().kind(),
            Kind::BadRequest,
        );
        session.speak(&text).unwrap();
        tokio::time::timeout(Duration::from_secs(2), server)
            .await
            .expect("server must receive the Speak frame")
            .unwrap();
    }

    /// Server stub that echoes the protocol skeleton: accepts a Speak frame,
    /// sends a Metadata text frame, then a binary audio frame.
    #[tokio::test(flavor = "current_thread")]
    async fn connect_emits_connected_status_then_audio_chunk() {
        let (listener, url) = bind_keep().await;

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("server handshake");
            // Read the client's first Speak frame.
            let _ = ws.next().await;
            // Send a Metadata text frame.
            ws.send(Message::Text(
                r#"{"type":"Metadata","request_id":"r-1"}"#.into(),
            ))
            .await
            .ok();
            // Send a binary audio frame: two samples, 0x1234 + 0x5678.
            let bytes = vec![0x34u8, 0x12, 0x78, 0x56];
            ws.send(Message::Binary(bytes.into())).await.ok();
            // Keep open briefly so the client can drain.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = ws.close(None).await;
        });

        let mut session = start_allowed_session(url).await.expect("client connect");

        session.speak("hello world").expect("speak");

        let mut events = session.take_events().expect("events");
        let mut got_connected = false;
        let mut got_audio = false;
        let mut got_metadata = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            let next = tokio::time::timeout(Duration::from_millis(200), events.next()).await;
            match next {
                Ok(Some(TtsEvent::Status(TtsStatus::Connected))) => got_connected = true,
                Ok(Some(TtsEvent::Status(TtsStatus::Metadata))) => got_metadata = true,
                Ok(Some(TtsEvent::AudioChunk {
                    samples,
                    sample_rate,
                })) => {
                    assert_eq!(sample_rate, 24_000);
                    assert_eq!(samples, vec![0x1234i16, 0x5678]);
                    got_audio = true;
                }
                Ok(Some(_)) => {}
                Ok(None) => break,
                Err(_) => continue,
            }
            if got_connected && got_audio && got_metadata {
                break;
            }
        }
        drop(session);
        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;

        assert!(got_connected, "must emit Connected");
        assert!(got_audio, "must emit AudioChunk");
        assert!(got_metadata, "must emit Metadata status");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clear_drops_in_flight_audio_frames() {
        let (listener, url) = bind_keep().await;

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("server handshake");

            // Phase 1: deliver a few "in-flight" audio frames before the
            // client sends Clear.
            for byte in 0..3u8 {
                let bytes = vec![byte, 0x00];
                ws.send(Message::Binary(bytes.into())).await.ok();
            }

            // Phase 2: wait for the Clear frame from the client.
            let mut got_clear = false;
            while !got_clear {
                if let Some(Ok(Message::Text(t))) = ws.next().await {
                    if t.contains("\"Clear\"") {
                        got_clear = true;
                    }
                } else {
                    break;
                }
            }

            // Phase 3: emit one more "trailing" audio frame that *should* be
            // dropped by the consumer (it arrived after Clear was sent but
            // before the server saw it -- the protocol allows this race).
            let trailing = vec![0xFFu8, 0x00];
            ws.send(Message::Binary(trailing.into())).await.ok();

            // Phase 4: emit the Cleared ack.
            ws.send(Message::Text(r#"{"type":"Cleared"}"#.into()))
                .await
                .ok();

            tokio::time::sleep(Duration::from_millis(200)).await;
            let _ = ws.close(None).await;
        });

        let mut session = AuraSession::start_with_url(url, "test-key".into(), TtsConfig::default())
            .await
            .expect("connect");

        // Drain ALL three pre-clear frames before sending Clear.
        //
        // Determinism note: the server sends exactly 3 pre-clear AudioChunks
        // (Phase 1) and the client MUST observe all 3 before it calls
        // `clear()`. The previous version bounded this loop by both
        // `pre_clear.len() < 3` AND a 500ms deadline, but only asserted
        // `> 0` — so under `multi_thread` scheduling it could exit early with
        // 1 or 2 frames drained. The remaining legitimate pre-clear frame(s)
        // then arrived inside the Clear->Cleared window and were miscounted as
        // trailing audio, tripping the final `trailing_audio_count == 0`
        // assertion (~1/25-50 flake). We now block on `events.next()` until
        // exactly 3 AudioChunks are observed — no timing-based early exit — so
        // the pre-clear/trailing boundary is deterministic. A generous safety
        // timeout guards against a hang if the fixture ever stops delivering,
        // failing loudly rather than silently under-collecting.
        const PRE_CLEAR_FRAMES: usize = 3;
        let mut events = session.take_events().expect("events");
        let mut pre_clear: Vec<TtsEvent> = Vec::new();
        while pre_clear.len() < PRE_CLEAR_FRAMES {
            let next = tokio::time::timeout(Duration::from_secs(5), events.next())
                .await
                .expect("timed out waiting for pre-Clear AudioChunks");
            match next {
                Some(ev @ TtsEvent::AudioChunk { .. }) => pre_clear.push(ev),
                // Non-audio events (e.g. Connected status) are expected before
                // the audio frames; ignore them and keep draining.
                Some(_) => {}
                None => panic!(
                    "event stream closed after {} of {PRE_CLEAR_FRAMES} pre-Clear AudioChunks",
                    pre_clear.len()
                ),
            }
        }
        let pre_audio_count = pre_clear.len();
        assert_eq!(
            pre_audio_count, PRE_CLEAR_FRAMES,
            "must observe all pre-Clear AudioChunks before sending Clear"
        );

        // Send Clear and consume events until we see Cleared. Track every
        // AudioChunk that arrives between Clear and Cleared -- those are the
        // frames a real consumer is required to drop.
        session.clear().expect("clear");
        let mut trailing_audio_count = 0usize;
        let cleared = loop {
            let next = tokio::time::timeout(Duration::from_millis(500), events.next()).await;
            match next {
                Ok(Some(TtsEvent::AudioChunk { .. })) => trailing_audio_count += 1,
                Ok(Some(TtsEvent::Status(TtsStatus::Cleared))) => break true,
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break false,
            }
        };

        drop(session);
        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;

        assert!(cleared, "must observe Cleared status");
        // Post-7107 contract: AudioChunks received between client-side Clear
        // and server's Cleared ack are SUPPRESSED at the session layer (see
        // `clearing` AtomicBool in run_io). Consumers must observe zero
        // trailing AudioChunk events.
        assert_eq!(
            trailing_audio_count, 0,
            "session layer must suppress trailing AudioChunks during Clear window"
        );
    }

    /// The session task fires KeepAlive every KEEPALIVE_INTERVAL_SECS (8)
    /// after the first idle window. We use a multi-thread runtime so the
    /// session task and the test driver each get a worker — `current_thread`
    /// would starve the spawned session task because the test spends most
    /// of its wall-clock time inside `frame_rx.recv()` and that yields to
    /// the runtime, but on a single thread the session task's 8-second
    /// interval timer can be starved if other I/O is queued. Multi-thread
    /// makes this deterministic.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn keepalive_sent_after_idle_timeout() {
        let (listener, url) = bind_keep().await;

        // Channel the server uses to report received frames back to the test.
        let (frame_tx, mut frame_rx) = tokio_mpsc::unbounded_channel::<String>();

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("server handshake");
            // Observe up to 5 client frames, forwarding their text payloads.
            for _ in 0..5 {
                match ws.next().await {
                    Some(Ok(Message::Text(t))) => {
                        let _ = frame_tx.send(t.to_string());
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = ws.close(None).await;
        });

        let session = AuraSession::start_with_url(url, "test-key".into(), TtsConfig::default())
            .await
            .expect("connect");

        // Wait for the first KeepAlive frame with a 20s ceiling. recv() yields
        // properly to the runtime so the session task gets scheduled to fire
        // its 8s interval — try_recv() in a sleep loop doesn't. The original
        // 12s budget proved too tight on slower runners (macOS Blacksmith
        // ate ~4s on session setup, leaving ~4s after keepalive fires —
        // racy). 20s gives ~10s of headroom after the 8s keepalive timer.
        let mut found = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            match tokio::time::timeout(remaining, frame_rx.recv()).await {
                Ok(Some(text)) if text.contains("KeepAlive") => {
                    found = true;
                    break;
                }
                Ok(Some(_)) => continue, // some other frame, keep waiting
                Ok(None) => break,       // sender dropped
                Err(_) => break,         // deadline elapsed
            }
        }

        drop(session);
        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;

        assert!(
            found,
            "expected at least one KeepAlive frame within idle window"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn reconnect_after_disconnect_with_backoff() {
        // First server: accept, then immediately close to force a reconnect.
        let (listener_a, url) = bind_keep().await;
        let port = listener_a.local_addr().unwrap().port();

        let server_a = tokio::spawn(async move {
            let (stream, _) = listener_a.accept().await.expect("accept-a");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("handshake-a");
            // Drop the socket immediately to simulate a network blip.
            let _ = ws.close(None).await;
        });

        let mut session =
            AuraSession::start_with_url(url.clone(), "test-key".into(), TtsConfig::default())
                .await
                .expect("connect-a");

        let mut events = session.take_events().expect("events");

        // Wait for server_a to drain so we can re-bind the port for the
        // reconnect target.
        let _ = tokio::time::timeout(Duration::from_secs(2), server_a).await;

        // Re-bind on the same port. A small race here is acceptable; the
        // test asserts only on Disconnected + Reconnecting, both of which
        // fire regardless of whether the second bind succeeds.
        let listener_b = TcpListener::bind(format!("127.0.0.1:{port}")).await;
        let server_b = if let Ok(listener_b) = listener_b {
            Some(tokio::spawn(async move {
                if let Ok((stream, _)) = listener_b.accept().await
                    && let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await
                {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    let _ = ws.close(None).await;
                }
            }))
        } else {
            None
        };

        let mut got_disconnected = false;
        let mut got_reconnecting = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            let next = tokio::time::timeout(Duration::from_millis(500), events.next()).await;
            match next {
                Ok(Some(TtsEvent::Status(TtsStatus::Disconnected))) => got_disconnected = true,
                Ok(Some(TtsEvent::Status(TtsStatus::Reconnecting { attempt, .. }))) => {
                    assert!(attempt >= 1);
                    got_reconnecting = true;
                    break;
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break,
            }
        }

        drop(session);
        if let Some(handle) = server_b {
            let _ = tokio::time::timeout(Duration::from_secs(1), handle).await;
        }

        assert!(got_disconnected, "must observe Disconnected on socket loss");
        assert!(got_reconnecting, "must enter Reconnecting state");
    }

    #[test]
    fn is_flushed_frame_detects_only_flushed_type() {
        assert!(is_flushed_frame(r#"{"type":"Flushed"}"#));
        assert!(is_flushed_frame(r#"{"type":"Flushed","extra":1}"#));
        assert!(!is_flushed_frame(r#"{"type":"Cleared"}"#));
        assert!(!is_flushed_frame(r#"{"type":"Metadata"}"#));
        assert!(!is_flushed_frame("not-json"));
        assert!(!is_flushed_frame(r#"{"no_type":true}"#));
    }

    /// Regression for the P1 wedge: if the socket drops AFTER a Clear is sent
    /// but BEFORE the server's `Cleared` ack, the `clearing` flag must NOT
    /// stay latched across the reconnect — otherwise every Binary frame on the
    /// fresh socket is suppressed forever (permanent silence after a
    /// barge-in-driven Clear). The session task must reset `clearing` on a
    /// successful reconnect so post-reconnect audio flows again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clearing_resets_on_reconnect_so_audio_flows_again() {
        let (listener_a, url) = bind_keep().await;
        let port = listener_a.local_addr().unwrap().port();

        // Server A: accept, wait for the client's Clear frame, then DROP the
        // socket WITHOUT sending a `Cleared` ack — leaving `clearing` latched.
        let server_a = tokio::spawn(async move {
            let (stream, _) = listener_a.accept().await.expect("accept-a");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("handshake-a");
            // Read frames until we see the Clear, then drop the socket.
            loop {
                match ws.next().await {
                    Some(Ok(Message::Text(t))) if t.contains("\"Clear\"") => break,
                    Some(Ok(_)) => continue,
                    _ => break,
                }
            }
            // Drop without a Cleared ack to simulate a mid-clear network blip.
            let _ = ws.close(None).await;
        });

        let mut session =
            AuraSession::start_with_url(url.clone(), "test-key".into(), TtsConfig::default())
                .await
                .expect("connect-a");
        let mut events = session.take_events().expect("events");

        // Trigger the Clear (sets the clearing flag in the session task).
        session.clear().expect("clear");

        // Wait for server_a to finish (it drops after seeing Clear).
        let _ = tokio::time::timeout(Duration::from_secs(3), server_a).await;

        // Server B: re-bind the same port for the reconnect. On connect, send
        // an audio frame. If the wedge is fixed, this frame is FORWARDED;
        // if `clearing` stayed latched it would be silently suppressed.
        let listener_b = TcpListener::bind(format!("127.0.0.1:{port}"))
            .await
            .expect("rebind-b");
        let server_b = tokio::spawn(async move {
            if let Ok((stream, _)) = listener_b.accept().await
                && let Ok(mut ws) = tokio_tungstenite::accept_async(stream).await
            {
                // 0x0102 LE = one sample. This is the post-reconnect audio
                // that MUST reach the consumer.
                ws.send(Message::Binary(vec![0x02u8, 0x01].into()))
                    .await
                    .ok();
                tokio::time::sleep(Duration::from_millis(300)).await;
                let _ = ws.close(None).await;
            }
        });

        // Drain events: expect Reconnected, then an AudioChunk (the proof that
        // `clearing` was reset). The reconnect backoff is ~1s (+/-20%).
        let mut got_reconnected = false;
        let mut got_post_reconnect_audio = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        while tokio::time::Instant::now() < deadline {
            let next = tokio::time::timeout(Duration::from_millis(500), events.next()).await;
            match next {
                Ok(Some(TtsEvent::Status(TtsStatus::Reconnected))) => got_reconnected = true,
                Ok(Some(TtsEvent::AudioChunk { samples, .. })) => {
                    // Only count audio observed after the reconnect.
                    if got_reconnected {
                        assert_eq!(samples, vec![0x0102i16]);
                        got_post_reconnect_audio = true;
                        break;
                    }
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => {}
            }
        }

        drop(session);
        let _ = tokio::time::timeout(Duration::from_secs(1), server_b).await;

        assert!(
            got_reconnected,
            "must observe Reconnected after socket loss"
        );
        assert!(
            got_post_reconnect_audio,
            "post-reconnect AudioChunk must be forwarded — `clearing` must reset on reconnect"
        );
    }

    /// Regression for the P2 tail-truncation: a graceful Close must DRAIN the
    /// server-rendered tail (forwarding the audio that the final Flush
    /// produces) before tearing the socket down, instead of closing
    /// synchronously and dropping the last clause.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_drains_flushed_tail_before_teardown() {
        let (listener, url) = bind_keep().await;

        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept");
            let mut ws = tokio_tungstenite::accept_async(stream)
                .await
                .expect("handshake");
            // Wait until we receive the client's Close frame, THEN render the
            // tail (audio) and the Flushed ack — mimicking a server that only
            // finishes rendering the final flush slightly after the client
            // asked to close.
            loop {
                match ws.next().await {
                    Some(Ok(Message::Text(t))) if t.contains("\"Close\"") => break,
                    Some(Ok(_)) => continue,
                    _ => break,
                }
            }
            // Tail audio for the just-flushed clause: 0x0A0B LE.
            ws.send(Message::Binary(vec![0x0Bu8, 0x0A].into()))
                .await
                .ok();
            ws.send(Message::Text(r#"{"type":"Flushed"}"#.into()))
                .await
                .ok();
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = ws.close(None).await;
        });

        let mut session = start_allowed_session(url).await.expect("connect");
        let mut events = session.take_events().expect("events");

        // finish()-style sequence: speak tail, flush, then close.
        session.speak("final clause.").expect("speak");
        session.flush().expect("flush");
        session.close().expect("close");

        let mut got_tail_audio = false;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
        while tokio::time::Instant::now() < deadline {
            let next = tokio::time::timeout(Duration::from_millis(300), events.next()).await;
            match next {
                Ok(Some(TtsEvent::AudioChunk { samples, .. })) => {
                    assert_eq!(samples, vec![0x0A0Bi16]);
                    got_tail_audio = true;
                    break;
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => {}
            }
        }

        drop(session);
        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;

        assert!(
            got_tail_audio,
            "Close must drain the Flushed tail audio before teardown"
        );
    }
}
