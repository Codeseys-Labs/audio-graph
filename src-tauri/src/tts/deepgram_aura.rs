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
use futures_util::{SinkExt, Stream, StreamExt};
use serde_json::Value;
use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::tungstenite::{self, Message};

use super::{
    TtsConfig, TtsError, TtsErrorKind, TtsEvent, TtsHttpErrorDiagnostic, TtsProvider, TtsSession,
    TtsStatus, tts_http_diagnostic_path,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const AURA_POLICY_PROVIDER: &str = "tts.deepgram_aura";
const AURA_HTTP_PROVIDER: &str = "deepgram";
const AURA_HTTP_SERVICE: &str = "aura";
const EXPLICIT_POLICY_REQUIRED: &str = "explicit_policy_required";

/// Aura idle-disconnect window is around 10s; the ADR mandates 8s for the
/// keepalive cadence -- we honour that exactly.
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
            .field("content_egress_policy", &self.content_egress_policy)
            .finish()
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
            _ => Err(TtsError::Auth(
                "deepgram_api_key not configured (used for both STT and TTS)".into(),
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
#[derive(Debug)]
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
            return Err(TtsError::Network("session is closed".into()));
        }
        self.cmd_tx
            .send(cmd)
            .map_err(|_| TtsError::Network("session command channel closed".into()))
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
            .map_err(TtsError::BadRequest)?;
        if text.len() > MAX_SPEAK_CHARS {
            return Err(TtsError::BadRequest(format!(
                "Speak frame text length {} exceeds Aura cap of {}",
                text.len(),
                MAX_SPEAK_CHARS
            )));
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
    // For ws:// URLs (test mock server), we still set the Authorization
    // header but the server is free to ignore it.
    let host = extract_host(url).unwrap_or_else(|| "api.deepgram.com".to_string());
    let request = tungstenite::http::Request::builder()
        .uri(url)
        .header("Authorization", format!("Token {}", api_key))
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .header("Sec-WebSocket-Version", "13")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Host", host)
        .body(())
        .map_err(|e| TtsError::Unknown(format!("Failed to build WebSocket request: {e}")))?;

    match tokio_tungstenite::connect_async(request).await {
        Ok((ws_stream, response)) => {
            let status = response.status().as_u16();
            // Tungstenite only returns Ok on a 101 upgrade, but be defensive.
            if status >= 400 {
                return Err(TtsError::from_http_status_diagnostic(
                    status,
                    aura_http_error_diagnostic(url, response.headers(), None, api_key),
                ));
            }
            Ok(ws_stream.split())
        }
        Err(tungstenite::Error::Http(resp)) => {
            let status = resp.status().as_u16();
            let body = resp.body().as_deref();
            Err(TtsError::from_http_status_diagnostic(
                status,
                aura_http_error_diagnostic(url, resp.headers(), body, api_key),
            ))
        }
        Err(e) => Err(classify_tungstenite_error(&e, api_key)),
    }
}

fn extract_host(url: &str) -> Option<String> {
    let stripped = url
        .strip_prefix("wss://")
        .or_else(|| url.strip_prefix("ws://"))?;
    let end = stripped.find(['/', '?']).unwrap_or(stripped.len());
    Some(stripped[..end].to_string())
}

fn aura_http_error_diagnostic(
    url: &str,
    headers: &tungstenite::http::HeaderMap,
    body: Option<&[u8]>,
    api_key: &str,
) -> TtsHttpErrorDiagnostic {
    let request_id = response_request_id(headers)
        .or_else(|| body.and_then(response_body_request_id))
        .map(|id| crate::error::redacted_provider_diagnostic(&id, [api_key]));
    let diagnostic = TtsHttpErrorDiagnostic::new(
        AURA_HTTP_PROVIDER,
        AURA_HTTP_SERVICE,
        tts_http_diagnostic_path(url),
    )
    .with_request_id(request_id);

    match body {
        Some(body) => diagnostic.with_body_bytes(body),
        None => diagnostic,
    }
}

fn response_request_id(headers: &tungstenite::http::HeaderMap) -> Option<String> {
    for name in ["x-request-id", "request-id", "dg-request-id", "cf-ray"] {
        let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) else {
            continue;
        };
        if let Some(request_id) = sanitize_request_id(value) {
            return Some(request_id);
        }
    }
    None
}

fn response_body_request_id(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|parsed| string_field(&parsed, &["request_id", "requestId", "id"]))
        .and_then(|request_id| sanitize_request_id(&request_id))
}

fn sanitize_request_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
    {
        return None;
    }

    let sanitized: String = trimmed.chars().take(128).collect();
    Some(sanitized)
}

fn classify_tungstenite_error(e: &tungstenite::Error, api_key: &str) -> TtsError {
    let safe = |message: String| crate::error::redacted_provider_diagnostic(&message, [api_key]);
    match e {
        tungstenite::Error::Io(io) => TtsError::Network(safe(format!("io: {io}"))),
        tungstenite::Error::Tls(tls) => TtsError::Network(safe(format!("tls: {tls}"))),
        tungstenite::Error::Url(u) => TtsError::BadRequest(safe(format!("url: {u}"))),
        tungstenite::Error::Protocol(p) => TtsError::Protocol(safe(format!("protocol: {p}"))),
        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed => {
            TtsError::Network("connection closed".into())
        }
        _ => TtsError::Unknown(safe(format!("websocket error: {e}"))),
    }
}

// ---------------------------------------------------------------------------
// Backoff
// ---------------------------------------------------------------------------

/// Backoff schedule per the resilience spec: 1 s, 2 s, 5 s, 10 s, then give
/// up. Matches `crate::asr::deepgram::backoff_for_attempt` and
/// `crate::gemini::backoff_for_attempt` for consistency.
pub(crate) fn backoff_for_attempt(attempt: u32) -> Option<u64> {
    match attempt {
        1 => Some(1),
        2 => Some(2),
        3 => Some(5),
        4 => Some(10),
        _ => None,
    }
}

/// Apply plus-or-minus 20% jitter to a backoff value in seconds, returning
/// the jittered duration. Uses a low-quality clock-derived pseudo-random
/// multiplier -- we only need enough variance to de-synchronize concurrent
/// reconnects across clients, not crypto-quality randomness.
pub(crate) fn jittered_backoff(base_secs: u64) -> Duration {
    if base_secs == 0 {
        return Duration::ZERO;
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    // Map nanos in [0, 1_000_000_000) to a multiplier in [0.8, 1.2].
    let frac = (nanos as f64) / 1_000_000_000_f64;
    let multiplier = 0.8 + 0.4 * frac;
    let scaled = (base_secs as f64) * multiplier;
    let millis = (scaled * 1000.0).round().max(1.0) as u64;
    Duration::from_millis(millis)
}

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
// The `String` payloads are diagnostic detail surfaced only through the
// derived `Debug` impl (reconnect logging). Dead-code analysis ignores
// `Debug` usage, so allow the otherwise-"unread" fields rather than
// dropping the diagnostic context.
#[allow(dead_code)]
enum DisconnectKind {
    /// User asked to close. No reconnect.
    UserRequested,
    /// Server sent a Close frame. May reconnect unless user closed.
    ServerClose(String),
    /// Transport-level error (TLS, TCP reset, DNS flap).
    NetworkError(String),
    /// Protocol violation -- malformed frame, invalid sequence.
    ProtocolError(String),
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
            &api_key,
            content_egress_policy,
        )
        .await;

        match disconnect {
            DisconnectKind::UserRequested | DisconnectKind::CmdChannelEnded => {
                let _ = event_tx.send(TtsEvent::Status(TtsStatus::Disconnected));
                break;
            }
            other => {
                if user_closed.load(Ordering::SeqCst) {
                    let _ = event_tx.send(TtsEvent::Status(TtsStatus::Disconnected));
                    break;
                }
                log::warn!("Aura session disconnected: {other:?}");
                let _ = event_tx.send(TtsEvent::Status(TtsStatus::Disconnected));

                reconnect_attempts += 1;
                let Some(backoff_secs) = backoff_for_attempt(reconnect_attempts) else {
                    let _ = event_tx.send(TtsEvent::Error {
                        kind: TtsErrorKind::Exhausted,
                        message: "Aura reconnect attempts exhausted".into(),
                    });
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
                        let _ = event_tx.send(TtsEvent::Error {
                            kind: e.kind(),
                            message: format!(
                                "Reconnect attempt {reconnect_attempts} failed: {}",
                                e.message()
                            ),
                        });
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
    api_key: &str,
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
                        let message = crate::error::redacted_provider_diagnostic(
                            &format!("keepalive failed: {e}"),
                            [api_key],
                        );
                        return DisconnectKind::NetworkError(message);
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
                            let message = crate::error::redacted_provider_diagnostic(
                                &format!("speak send failed: {e}"),
                                [api_key],
                            );
                            return DisconnectKind::NetworkError(message);
                        }
                        last_outbound = tokio::time::Instant::now();
                    }
                    Some(SessionCmd::Flush(sequence)) => {
                        if let Err(e) = writer.send(Message::Text(FRAME_FLUSH.into())).await {
                            let message = crate::error::redacted_provider_diagnostic(
                                &format!("flush send failed: {e}"),
                                [api_key],
                            );
                            return DisconnectKind::NetworkError(message);
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
                            let message = crate::error::redacted_provider_diagnostic(
                                &format!("clear send failed: {e}"),
                                [api_key],
                            );
                            return DisconnectKind::NetworkError(message);
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
                            api_key,
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
                    return DisconnectKind::NetworkError("reader stream ended".into());
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
                        handle_server_text_with_key(
                            &text,
                            event_tx,
                            &mut pending_flushes,
                            clearing,
                            api_key,
                        );
                    }
                    Ok(Message::Close(frame)) => {
                        if user_closed.load(Ordering::SeqCst) {
                            return DisconnectKind::UserRequested;
                        }
                        let reason = frame
                            .map(|f| {
                                let code: u16 = f.code.into();
                                close_frame_diagnostic(code, f.reason.as_ref())
                            })
                            .unwrap_or_else(|| "no_frame".into());
                        return DisconnectKind::ServerClose(reason);
                    }
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                    Err(tungstenite::Error::ConnectionClosed)
                    | Err(tungstenite::Error::AlreadyClosed) => {
                        return DisconnectKind::NetworkError("connection closed".into());
                    }
                    Err(tungstenite::Error::Protocol(p)) => {
                        let message =
                            crate::error::redacted_provider_diagnostic(&p.to_string(), [api_key]);
                        return DisconnectKind::ProtocolError(message);
                    }
                    Err(e) => {
                        let message = crate::error::redacted_provider_diagnostic(
                            &format!("read error: {e}"),
                            [api_key],
                        );
                        return DisconnectKind::NetworkError(message);
                    }
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
    api_key: &str,
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
                handle_server_text_with_key(&text, event_tx, pending_flushes, clearing, api_key);
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

/// Parse a single server JSON text frame and emit the corresponding
/// [`TtsEvent`]. Public for unit tests.
#[cfg(test)]
pub(crate) fn handle_server_text(
    text: &str,
    event_tx: &tokio_mpsc::UnboundedSender<TtsEvent>,
    pending_flushes: &mut VecDeque<u64>,
    clearing: &Arc<AtomicBool>,
) {
    handle_server_text_with_key(text, event_tx, pending_flushes, clearing, "");
}

fn handle_server_text_with_key(
    text: &str,
    event_tx: &tokio_mpsc::UnboundedSender<TtsEvent>,
    pending_flushes: &mut VecDeque<u64>,
    clearing: &Arc<AtomicBool>,
    api_key: &str,
) {
    let parsed: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            // Defense-in-depth on the UI-visible `TtsEvent::Error`:
            // `serde_json::Error`'s `Display` reports only the failure position
            // (line/column), not the frame bytes, so `{e}` does not itself echo
            // the provider text frame. Still route the detail through the shared
            // redaction/safe-excerpt helper (api_key registered as a known
            // secret, pattern scrub for the rest) so this branch stays leak-safe
            // if it is ever changed to interpolate the raw `text` frame.
            let detail = crate::error::redacted_error_excerpt(&e.to_string(), [api_key], 200);
            let _ = event_tx.send(TtsEvent::Error {
                kind: TtsErrorKind::Protocol,
                message: format!("Invalid JSON from Aura: {detail}"),
            });
            return;
        }
    };

    let msg_type = parsed
        .get("type")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();

    match msg_type.as_str() {
        "Metadata" => {
            let _ = event_tx.send(TtsEvent::Status(TtsStatus::Metadata {
                request_id: string_field(&parsed, &["request_id", "requestId"]),
                model: string_field(&parsed, &["model", "model_name", "modelName"]),
                field_count: object_field_count(&parsed),
            }));
        }
        "Flushed" => {
            // Server doesn't tag the flush with a sequence; pop the oldest
            // dispatch-time sequence from the commands this socket sent.
            // `0` means the server produced an unsolicited ack.
            let sequence = pending_flushes.pop_front().unwrap_or(0);
            let _ = event_tx.send(TtsEvent::Status(TtsStatus::Flushed { sequence }));
        }
        "Cleared" => {
            // Reset the suppression flag so subsequent AudioChunks (from a
            // new utterance after barge-in) flow through to consumers.
            clearing.store(false, Ordering::SeqCst);
            let _ = event_tx.send(TtsEvent::Status(TtsStatus::Cleared));
        }
        "Warning" => {
            let message = aura_error_diagnostic(&parsed);
            let message = crate::error::redacted_provider_diagnostic(&message, [api_key]);
            let _ = event_tx.send(TtsEvent::Error {
                kind: TtsErrorKind::Unknown,
                message: format!("Aura warning: {message}"),
            });
        }
        "Error" => {
            let raw_message = parsed
                .get("description")
                .and_then(|v| v.as_str())
                .or_else(|| parsed.get("message").and_then(|v| v.as_str()))
                .unwrap_or("");
            let message = aura_error_diagnostic(&parsed);
            let message = crate::error::redacted_provider_diagnostic(&message, [api_key]);
            // Server-classified errors land here regardless of HTTP status --
            // map common phrases to the right `TtsErrorKind` for UI surfaces.
            let kind = if raw_message.to_ascii_lowercase().contains("auth") {
                TtsErrorKind::Auth
            } else if raw_message.to_ascii_lowercase().contains("rate") {
                TtsErrorKind::RateLimit
            } else {
                TtsErrorKind::Server
            };
            let _ = event_tx.send(TtsEvent::Error { kind, message });
        }
        _ => {
            let request_id = string_field(&parsed, &["request_id", "requestId"])
                .and_then(|value| sanitize_request_id(&value))
                .unwrap_or_else(|| "none".to_string());
            let safe_msg_type =
                sanitize_request_id(&msg_type).unwrap_or_else(|| "unknown".to_string());
            log::debug!(
                "Aura: unhandled message type='{safe_msg_type}' request_id={request_id} fields={}",
                object_field_count(&parsed)
            );
        }
    }
}

fn string_field(parsed: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| parsed.get(*key).and_then(|value| value.as_str()))
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn object_field_count(parsed: &Value) -> usize {
    parsed.as_object().map_or(0, serde_json::Map::len)
}

fn close_frame_diagnostic(code: u16, reason: &str) -> String {
    format!("code={code} reason_len={}", reason.chars().count())
}

fn aura_error_diagnostic(parsed: &Value) -> String {
    let message_len = parsed
        .get("description")
        .or_else(|| parsed.get("message"))
        .and_then(|value| value.as_str())
        .map(|value| value.chars().count());
    let code = diagnostic_token_field(parsed, &["code", "error_code", "status"]);
    let request_id = diagnostic_token_field(parsed, &["request_id", "requestId"]);

    match (code, request_id, message_len) {
        (Some(code), Some(request_id), Some(message_len)) => {
            format!("Aura error code={code} request_id={request_id} message_len={message_len}")
        }
        (Some(code), None, Some(message_len)) => {
            format!("Aura error code={code} message_len={message_len}")
        }
        (None, Some(request_id), Some(message_len)) => {
            format!("Aura error request_id={request_id} message_len={message_len}")
        }
        (Some(code), Some(request_id), None) => {
            format!("Aura error code={code} request_id={request_id}")
        }
        (Some(code), None, None) => format!("Aura error code={code}"),
        (None, Some(request_id), None) => format!("Aura error request_id={request_id}"),
        (None, None, Some(message_len)) => format!("Aura error message_len={message_len}"),
        (None, None, None) => format!(
            "Aura error type={} fields={}",
            diagnostic_token_field(parsed, &["type"]).unwrap_or_else(|| "unknown".into()),
            object_field_count(parsed)
        ),
    }
}

fn diagnostic_token_field(parsed: &Value, keys: &[&str]) -> Option<String> {
    string_field(parsed, keys).and_then(|value| sanitize_request_id(&value))
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
    use super::*;
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
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(2), Some(2));
        assert_eq!(backoff_for_attempt(3), Some(5));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_ws_http_rejection_uses_metadata_only_diagnostic() {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local addr");
        let url =
            format!("ws://{addr}/v1/speak?api_key=query-secret-12345&model=aura-generated-test");
        let api_key = "dg-http-test-key";
        let body = r#"{"error":"provider body text","description":"generated speech text","api_key":"body-secret-12345","request_id":"dg-body-req"}"#;
        let expected_body_bytes = body.len();
        let expected_body_chars = body.chars().count();
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
        let message = err.message();

        assert_eq!(err.kind(), TtsErrorKind::Auth);
        assert!(message.contains("status=401"));
        assert!(message.contains("provider=deepgram"));
        assert!(message.contains("service=aura"));
        assert!(message.contains("path=/v1/speak"));
        assert!(message.contains("request_id=dg-header-req_7"));
        assert!(message.contains(&format!("body_bytes={expected_body_bytes}")));
        assert!(message.contains(&format!("body_chars={expected_body_chars}")));
        for leaked in [
            "query-secret-12345",
            "aura-generated-test",
            "provider body text",
            "generated speech text",
            "body-secret-12345",
            api_key,
            "api_key=",
        ] {
            assert!(
                !message.contains(leaked),
                "Aura HTTP diagnostic leaked {leaked}: {message}"
            );
        }
    }

    #[test]
    fn tungstenite_error_classifier_redacts_provider_credentials() {
        let api_key = "dg-aura-websocket-secret";
        let err = tungstenite::Error::Io(std::io::Error::other(format!(
            "bad token {api_key} Authorization: Bearer bearer-aura-secret-12345 wss://user:pass@example.com/v1?api_key=url-aura-secret-12345 AKIA1234567890ABCDEF"
        )));

        let classified = classify_tungstenite_error(&err, api_key);
        let message = classified.message();

        for leaked in [
            api_key,
            "bearer-aura-secret-12345",
            "user:pass",
            "url-aura-secret-12345",
            "AKIA1234567890ABCDEF",
        ] {
            assert!(
                !message.contains(leaked),
                "classified Aura WebSocket error leaked {leaked}: {message}"
            );
        }
        assert!(message.contains("<redacted>"));
    }

    #[test]
    fn server_error_message_uses_metadata_only_diagnostic() {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let mut pending_flushes = VecDeque::new();
        let clearing = Arc::new(AtomicBool::new(false));
        let api_key = "dg-aura-server-secret";

        handle_server_text_with_key(
            &format!(
                r#"{{"type":"Error","description":"provider body text generated speech text auth failed {api_key} Authorization: Bearer bearer-aura-server-secret-12345 wss://user:pass@example.com?api_key=url-aura-server-secret-12345 AKIA1234567890ABCDEF","request_id":"dg-runtime-req_1"}}"#
            ),
            &tx,
            &mut pending_flushes,
            &clearing,
            api_key,
        );

        match rx.try_recv().expect("error event") {
            TtsEvent::Error { message, .. } => {
                for leaked in [
                    api_key,
                    "bearer-aura-server-secret-12345",
                    "user:pass",
                    "url-aura-server-secret-12345",
                    "AKIA1234567890ABCDEF",
                    "provider body text",
                    "generated speech text",
                    "auth failed",
                    "Authorization",
                ] {
                    assert!(
                        !message.contains(leaked),
                        "Aura server error leaked {leaked}: {message}"
                    );
                }
                assert!(message.contains("request_id=dg-runtime-req_1"));
                assert!(message.contains("message_len="));
            }
            other => panic!("expected error event, got {other:?}"),
        }
    }

    /// Contract guard for the invalid-JSON branch: the UI-visible
    /// `TtsEvent::Error` must never carry a credential shape. `serde_json::Error`
    /// Displays only line/column today (so `{e}` does not echo the frame), but
    /// this locks the redaction path in so a future change interpolating the raw
    /// `text` frame — which may embed an echoed api_key / bearer token / AWS key /
    /// URL credential — stays scrubbed before surfacing.
    #[test]
    fn invalid_json_text_frame_redacts_before_surfacing() {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let mut pending_flushes = VecDeque::new();
        let clearing = Arc::new(AtomicBool::new(false));
        let api_key = "dg-aura-parse-secret-12345";

        // NOT valid JSON (leading garbage) but crammed with credential shapes so
        // the serde error's echoed snippet must be scrubbed before surfacing.
        let bad_frame = format!(
            concat!(
                "not-json {api_key} ",
                "Authorization: Bearer bearer-aura-parse-secret-12345 ",
                "wss://user:pass@example.com?api_key=url-aura-parse-secret-12345 ",
                "AKIA1234567890ABCDEF }}",
            ),
            api_key = api_key,
        );

        handle_server_text_with_key(&bad_frame, &tx, &mut pending_flushes, &clearing, api_key);

        match rx.try_recv().expect("error event") {
            TtsEvent::Error { kind, message } => {
                assert_eq!(kind, TtsErrorKind::Protocol);
                assert!(
                    message.contains("Invalid JSON from Aura"),
                    "parse error must name the invalid-JSON failure, got: {message}"
                );
                for leaked in [
                    api_key,
                    "bearer-aura-parse-secret-12345",
                    "user:pass",
                    "url-aura-parse-secret-12345",
                    "AKIA1234567890ABCDEF",
                ] {
                    assert!(
                        !message.contains(leaked),
                        "Aura invalid-JSON error leaked {leaked}: {message}"
                    );
                }
            }
            other => panic!("expected protocol error event, got {other:?}"),
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
            other => panic!("expected first flush command, got {other:?}"),
        }
        match cmd_rx.try_recv().expect("second command") {
            SessionCmd::Flush(sequence) => assert_eq!(sequence, 2),
            other => panic!("expected second flush command, got {other:?}"),
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
            other => panic!("expected speak command, got {other:?}"),
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
        let message = err.message();

        assert_eq!(err.kind(), TtsErrorKind::BadRequest);
        assert!(message.contains("Privacy policy blocked text egress"));
        assert!(message.contains(AURA_POLICY_PROVIDER));
        assert!(message.contains("local_only"));
        assert!(
            !message.contains(&generated_text),
            "policy error must not echo generated text: {message}"
        );
        assert!(
            !message.contains(api_key),
            "policy error must not leak API key-like text: {message}"
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
    fn handle_server_text_emits_metadata_status() {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let mut pending_flushes = VecDeque::new();
        let clearing = Arc::new(AtomicBool::new(false));
        handle_server_text(
            r#"{"type":"Metadata","request_id":"abc"}"#,
            &tx,
            &mut pending_flushes,
            &clearing,
        );
        match rx.try_recv().expect("event") {
            TtsEvent::Status(TtsStatus::Metadata {
                request_id,
                field_count,
                ..
            }) => {
                assert_eq!(request_id.as_deref(), Some("abc"));
                assert_eq!(field_count, 2);
            }
            other => panic!("expected metadata, got {other:?}"),
        }
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
            other => panic!("expected flushed, got {other:?}"),
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
            other => panic!("expected first flushed, got {other:?}"),
        }
        match rx.try_recv().expect("second event") {
            TtsEvent::Status(TtsStatus::Flushed { sequence }) => assert_eq!(sequence, 2),
            other => panic!("expected second flushed, got {other:?}"),
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
            other => panic!("expected cleared, got {other:?}"),
        }
        assert!(
            !clearing.load(Ordering::SeqCst),
            "Cleared ack must reset the clearing flag so subsequent AudioChunks flow"
        );
    }

    #[test]
    fn handle_server_text_classifies_auth_error() {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let mut pending_flushes = VecDeque::new();
        let clearing = Arc::new(AtomicBool::new(false));
        handle_server_text(
            r#"{"type":"Error","description":"authentication failed"}"#,
            &tx,
            &mut pending_flushes,
            &clearing,
        );
        match rx.try_recv().expect("event") {
            TtsEvent::Error {
                kind: TtsErrorKind::Auth,
                ..
            } => {}
            other => panic!("expected auth error, got {other:?}"),
        }
    }

    #[test]
    fn handle_server_text_classifies_rate_limit_error() {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let mut pending_flushes = VecDeque::new();
        let clearing = Arc::new(AtomicBool::new(false));
        handle_server_text(
            r#"{"type":"Error","description":"rate limit exceeded"}"#,
            &tx,
            &mut pending_flushes,
            &clearing,
        );
        match rx.try_recv().expect("event") {
            TtsEvent::Error {
                kind: TtsErrorKind::RateLimit,
                ..
            } => {}
            other => panic!("expected rate-limit error, got {other:?}"),
        }
    }

    #[test]
    fn handle_server_text_classifies_generic_error_as_server() {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let mut pending_flushes = VecDeque::new();
        let clearing = Arc::new(AtomicBool::new(false));
        handle_server_text(
            r#"{"type":"Error","description":"internal pipeline failure"}"#,
            &tx,
            &mut pending_flushes,
            &clearing,
        );
        match rx.try_recv().expect("event") {
            TtsEvent::Error {
                kind: TtsErrorKind::Server,
                ..
            } => {}
            other => panic!("expected server error, got {other:?}"),
        }
    }

    #[test]
    fn handle_server_text_invalid_json_emits_protocol_error() {
        let (tx, mut rx) = tokio_mpsc::unbounded_channel();
        let mut pending_flushes = VecDeque::new();
        let clearing = Arc::new(AtomicBool::new(false));
        handle_server_text("not-json", &tx, &mut pending_flushes, &clearing);
        match rx.try_recv().expect("event") {
            TtsEvent::Error {
                kind: TtsErrorKind::Protocol,
                ..
            } => {}
            other => panic!("expected protocol error, got {other:?}"),
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
        assert_eq!(err.kind(), TtsErrorKind::Auth);
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
        let message = err.message();

        assert_eq!(err.kind(), TtsErrorKind::BadRequest);
        assert!(message.contains("Privacy policy blocked text egress"));
        assert!(message.contains(AURA_POLICY_PROVIDER));
        assert!(message.contains(EXPLICIT_POLICY_REQUIRED));
        assert!(!message.contains("private synthesis text"));
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
            "test-key",
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
    async fn speak_rejects_text_over_2000_chars() {
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
        let too_long = "x".repeat(MAX_SPEAK_CHARS + 1);
        let err = session.speak(&too_long).expect_err("must reject");
        assert_eq!(err.kind(), TtsErrorKind::BadRequest);
        drop(session);
        let _ = tokio::time::timeout(Duration::from_secs(1), server).await;
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
                Ok(Some(TtsEvent::Status(TtsStatus::Metadata { .. }))) => got_metadata = true,
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

        // Drain a few pre-clear frames.
        let mut events = session.take_events().expect("events");
        let mut pre_clear: Vec<TtsEvent> = Vec::new();
        let pre_deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while tokio::time::Instant::now() < pre_deadline && pre_clear.len() < 3 {
            if let Ok(Some(ev)) =
                tokio::time::timeout(Duration::from_millis(100), events.next()).await
                && matches!(ev, TtsEvent::AudioChunk { .. })
            {
                pre_clear.push(ev);
            }
        }
        let pre_audio_count = pre_clear.len();
        assert!(
            pre_audio_count > 0,
            "must observe at least one pre-Clear AudioChunk"
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

    #[tokio::test(flavor = "current_thread")]
    async fn error_classification_for_4xx_vs_5xx() {
        // Tokio-tungstenite's local mock can't easily produce HTTP errors;
        // exercise the classifier path the WS connect uses on real 4xx/5xx
        // responses. This is the same code path open_ws hits on rejection.
        let auth = TtsError::from_http_status(401, "{\"error\":\"unauthorized\"}");
        assert_eq!(auth.kind(), TtsErrorKind::Auth);

        let rate = TtsError::from_http_status(429, "Too Many Requests");
        assert_eq!(rate.kind(), TtsErrorKind::RateLimit);

        let bad = TtsError::from_http_status(400, "bad request body");
        assert_eq!(bad.kind(), TtsErrorKind::BadRequest);

        let server = TtsError::from_http_status(503, "service unavailable");
        assert_eq!(server.kind(), TtsErrorKind::Server);
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
