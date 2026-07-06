//! Soniox realtime STT parser and WebSocket client.
//!
//! It normalizes Soniox WebSocket JSON responses into the app's ASR
//! span-revision contract and owns the provider-local live WebSocket runtime.
//! Provider selection remains gated elsewhere until live smoke/source-policy
//! evidence is strong enough to promote Soniox from planned to selectable.

#[cfg(test)]
use super::reconnect::backoff_for_attempt;
use super::reconnect::{ReconnectStep, next_reconnect_step};
use super::transport::{AsrTransportPayloadKind, AsrWsReader, AsrWsWriteGuard, AsrWsWriter};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
#[cfg(test)]
use std::{future::Future, pin::Pin};
use tokio::sync::mpsc as tokio_mpsc;
use tokio_tungstenite::{
    connect_async,
    tungstenite::{self, Message},
};

use crate::events::{AsrSpanRevisionPayload, AsrSpanStability};

const PROVIDER: &str = "soniox";
const WEBSOCKET_URL: &str = "wss://stt-rt.soniox.com/transcribe-websocket";
const AUDIO_BUFFER_MAX_CHUNKS: usize = 200;
pub const DEFAULT_MODEL: &str = "stt-rt-v5";
/// Idle keepalive cadence (M2 / audio-graph-63be). Soniox's realtime protocol
/// reserves the empty binary frame as the *finalize* signal, so it cannot be
/// reused as an idle no-op; we send a WebSocket `Ping` control frame during
/// quiet periods instead. The mixer normally keeps `last_outbound` warm with a
/// continuous silence-padded stream, so this only fires when the audio cadence
/// actually stalls.
const KEEPALIVE_INTERVAL_SECS: u64 = 8;

#[derive(Debug, Clone)]
pub struct SonioxParsedMessage {
    pub revisions: Vec<SonioxParsedRevision>,
    pub finished: bool,
    pub error: Option<SonioxProviderError>,
    pub final_audio_proc_ms: Option<u64>,
    pub total_audio_proc_ms: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SonioxParsedRevision {
    pub payload: AsrSpanRevisionPayload,
    pub language: Option<String>,
    pub source_language: Option<String>,
    pub final_audio_proc_ms: Option<u64>,
    pub total_audio_proc_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SonioxProviderError {
    pub code: Option<String>,
    pub error_type: Option<String>,
    pub message: String,
    pub request_id: Option<String>,
    pub more_info: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SonioxParseError {
    InvalidJson(String),
}

// Channel event enum: boxing the large `Revision` variant would ripple
// through every construction and match site for negligible benefit.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum SonioxEvent {
    Revision(SonioxParsedRevision),
    Finished,
    Error { message: String },
    Connected,
    Disconnected,
    Reconnecting { attempt: u32, backoff_secs: u64 },
    Reconnected,
}

#[derive(Clone)]
pub struct SonioxConfig {
    pub api_key: String,
    pub model: String,
    pub source_id: String,
    pub enable_diarization: bool,
    pub enable_language_identification: bool,
    pub language_hints: Vec<String>,
    pub content_egress_policy: crate::asr::ProviderContentEgressPolicy,
}

impl std::fmt::Debug for SonioxConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SonioxConfig")
            .field(
                "api_key",
                &crate::credentials::redacted_secret_presence(Some(&self.api_key)),
            )
            .field("model", &self.model)
            .field("source_id", &self.source_id)
            .field("enable_diarization", &self.enable_diarization)
            .field(
                "enable_language_identification",
                &self.enable_language_identification,
            )
            .field("language_hints", &self.language_hints)
            .field("content_egress_policy", &self.content_egress_policy)
            .finish()
    }
}

#[derive(Debug)]
pub struct SonioxRealtimeParser {
    source_id: String,
    turn_index: u64,
    response_sequence: u64,
    active_turn: Option<SonioxActiveTurn>,
}

#[derive(Debug, Clone)]
struct SonioxActiveTurn {
    span_id: String,
    provider_item_id: String,
    revision_number: u64,
    final_tokens: Vec<SonioxToken>,
    non_final_tokens: Vec<SonioxToken>,
}

#[derive(Debug, Deserialize)]
struct SonioxResponse {
    #[serde(default)]
    tokens: Vec<SonioxToken>,
    #[serde(default)]
    finished: bool,
    #[serde(default)]
    final_audio_proc_ms: Option<u64>,
    #[serde(default)]
    total_audio_proc_ms: Option<u64>,
    #[serde(default)]
    error_code: Option<serde_json::Value>,
    #[serde(default)]
    error_type: Option<String>,
    #[serde(default)]
    error_message: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    more_info: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SonioxToken {
    text: String,
    #[serde(default)]
    start_ms: Option<u64>,
    #[serde(default)]
    end_ms: Option<u64>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    is_final: bool,
    #[serde(default)]
    speaker: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    source_language: Option<String>,
}

impl SonioxRealtimeParser {
    pub fn new(source_id: impl Into<String>) -> Self {
        Self {
            source_id: source_id.into(),
            turn_index: 0,
            response_sequence: 0,
            active_turn: None,
        }
    }

    pub fn abandon_active_turn(&mut self) {
        self.active_turn = None;
    }

    /// Finalize the active turn on reconnect (M3 / audio-graph-c6de).
    ///
    /// A Soniox reconnect abandons the in-flight turn and resumes buffered audio
    /// under a fresh `turn-{n}` namespace. Without this, the pre-drop partial
    /// span (`soniox:{source}:turn-{n}`, `is_final=false`) is never superseded
    /// and lingers as an orphaned never-finalized partial in the transcript
    /// ledger. This emits a terminal (`is_final=true`) revision for that span —
    /// superseding the last emitted partial with the last-known text — mirroring
    /// how OpenAI-realtime finalizes across a reconnect. Returns `None` when
    /// there is no active turn or it carries no emitted text (nothing to
    /// orphan). Clears the active turn either way so post-call state matches
    /// [`Self::abandon_active_turn`].
    pub fn finalize_active_turn_for_reconnect(
        &mut self,
        received_at_ms: u64,
    ) -> Option<SonioxParsedRevision> {
        let active_turn = self.active_turn.as_mut()?;
        let tokens = active_turn.combined_tokens();
        let text = joined_text(&tokens);
        if text.is_empty() {
            // No partial was ever emitted downstream, so nothing is orphaned.
            self.active_turn = None;
            return None;
        }

        active_turn.revision_number += 1;
        let revision_number = active_turn.revision_number;
        // A finalize is only meaningful once a partial (rev >= 1) was emitted;
        // supersede it so the ledger retcons the provisional span to final.
        let supersedes =
            (revision_number > 1).then(|| revision_ref(&active_turn.span_id, revision_number - 1));
        let start_ms = min_start_ms(&tokens).unwrap_or(0);
        let end_ms = max_end_ms(&tokens).unwrap_or(start_ms);
        let language = consistent_token_field(&tokens, |token| token.language.as_deref());
        let source_language =
            consistent_token_field(&tokens, |token| token.source_language.as_deref());
        let speaker = consistent_token_field(&tokens, |token| token.speaker.as_deref());

        let revision = SonioxParsedRevision {
            payload: AsrSpanRevisionPayload {
                span_id: active_turn.span_id.clone(),
                provider: PROVIDER.to_string(),
                source_id: self.source_id.clone(),
                provider_item_id: Some(active_turn.provider_item_id.clone()),
                transcript_segment_id: None,
                speaker_id: speaker.clone(),
                speaker_label: speaker.as_ref().map(|speaker| format!("Speaker {speaker}")),
                channel: None,
                text,
                start_time: millis_to_secs(start_ms),
                end_time: millis_to_secs(end_ms),
                confidence: average_confidence(&tokens),
                is_final: true,
                stability: AsrSpanStability::Final,
                revision_number,
                supersedes,
                turn_id: Some(active_turn.provider_item_id.clone()),
                end_of_turn: true,
                raw_event_ref: Some(format!(
                    "soniox.reconnect.finalize.{}",
                    self.response_sequence
                )),
                capture_latency_ms: None,
                asr_latency_ms: None,
                received_at_ms,
            },
            language,
            source_language,
            final_audio_proc_ms: None,
            total_audio_proc_ms: None,
        };

        self.active_turn = None;
        Some(revision)
    }

    pub fn parse_message(
        &mut self,
        text: &str,
        received_at_ms: u64,
    ) -> Result<SonioxParsedMessage, SonioxParseError> {
        let response: SonioxResponse = serde_json::from_str(text)
            .map_err(|error| SonioxParseError::InvalidJson(error.to_string()))?;
        self.response_sequence += 1;

        let error = response
            .error_message
            .as_ref()
            .map(|message| SonioxProviderError {
                code: response.error_code.as_ref().map(json_value_to_string),
                error_type: response.error_type.clone(),
                message: message.clone(),
                request_id: response.request_id.clone(),
                more_info: response.more_info.clone(),
            });

        let mut revisions = Vec::new();
        let transcript_tokens = response
            .tokens
            .iter()
            .filter(|token| !is_marker_only(&token.text))
            .count();
        if transcript_tokens > 0
            && let Some(active_turn) = self.active_turn.as_mut()
        {
            active_turn.non_final_tokens.clear();
        }

        let mut updated_active_turn = false;
        for token in &response.tokens {
            let (clean_token, closes_turn) = token_without_markers(token);

            if let Some(clean_token) = clean_token {
                let active_turn = self.ensure_active_turn();
                if clean_token.is_final {
                    active_turn.final_tokens.push(clean_token);
                } else {
                    active_turn.non_final_tokens.push(clean_token);
                }
                updated_active_turn = true;
            }

            if closes_turn {
                if let Some(revision) = self.emit_active_revision(&response, received_at_ms, true) {
                    revisions.push(revision);
                }
                self.active_turn = None;
                updated_active_turn = false;
            }
        }

        if response.finished {
            if let Some(revision) = self.emit_active_revision(&response, received_at_ms, true) {
                revisions.push(revision);
            }
            self.active_turn = None;
        } else if updated_active_turn
            && let Some(revision) = self.emit_active_revision(&response, received_at_ms, false)
        {
            revisions.push(revision);
        }

        Ok(SonioxParsedMessage {
            revisions,
            finished: response.finished,
            error,
            final_audio_proc_ms: response.final_audio_proc_ms,
            total_audio_proc_ms: response.total_audio_proc_ms,
        })
    }

    fn ensure_active_turn(&mut self) -> &mut SonioxActiveTurn {
        if self.active_turn.is_none() {
            self.turn_index += 1;
            let provider_item_id = format!("turn-{}", self.turn_index);
            self.active_turn = Some(SonioxActiveTurn {
                span_id: soniox_span_id(&self.source_id, self.turn_index),
                provider_item_id,
                revision_number: 0,
                final_tokens: Vec::new(),
                non_final_tokens: Vec::new(),
            });
        }
        self.active_turn.as_mut().expect("active turn initialized")
    }

    fn emit_active_revision(
        &mut self,
        response: &SonioxResponse,
        received_at_ms: u64,
        is_final: bool,
    ) -> Option<SonioxParsedRevision> {
        let active_turn = self.active_turn.as_mut()?;
        let tokens = active_turn.combined_tokens();
        let text = joined_text(&tokens);
        if text.is_empty() {
            return None;
        }

        active_turn.revision_number += 1;
        let revision_number = active_turn.revision_number;
        let supersedes =
            (revision_number > 1).then(|| revision_ref(&active_turn.span_id, revision_number - 1));
        let start_ms = min_start_ms(&tokens).unwrap_or_else(|| {
            response
                .final_audio_proc_ms
                .or(response.total_audio_proc_ms)
                .unwrap_or(0)
        });
        let end_ms = max_end_ms(&tokens).unwrap_or(start_ms);
        let language = consistent_token_field(&tokens, |token| token.language.as_deref());
        let source_language =
            consistent_token_field(&tokens, |token| token.source_language.as_deref());
        let speaker = consistent_token_field(&tokens, |token| token.speaker.as_deref());

        Some(SonioxParsedRevision {
            payload: AsrSpanRevisionPayload {
                span_id: active_turn.span_id.clone(),
                provider: PROVIDER.to_string(),
                source_id: self.source_id.clone(),
                provider_item_id: Some(active_turn.provider_item_id.clone()),
                transcript_segment_id: None,
                speaker_id: speaker.clone(),
                speaker_label: speaker.as_ref().map(|speaker| format!("Speaker {speaker}")),
                channel: None,
                text,
                start_time: millis_to_secs(start_ms),
                end_time: millis_to_secs(end_ms),
                confidence: average_confidence(&tokens),
                is_final,
                stability: if is_final {
                    AsrSpanStability::Final
                } else {
                    AsrSpanStability::Partial
                },
                revision_number,
                supersedes,
                turn_id: Some(active_turn.provider_item_id.clone()),
                end_of_turn: is_final,
                raw_event_ref: Some(format!("soniox.response.{}", self.response_sequence)),
                capture_latency_ms: None,
                asr_latency_ms: None,
                received_at_ms,
            },
            language,
            source_language,
            final_audio_proc_ms: response.final_audio_proc_ms,
            total_audio_proc_ms: response.total_audio_proc_ms,
        })
    }
}

enum AudioCmd {
    Chunk(Vec<u8>),
    Stop,
}

pub struct SonioxClient {
    config: SonioxConfig,
    event_tx: crossbeam_channel::Sender<SonioxEvent>,
    event_rx: crossbeam_channel::Receiver<SonioxEvent>,
    connected: Arc<AtomicBool>,
    user_disconnected: Arc<AtomicBool>,
    rt: Option<tokio::runtime::Runtime>,
    audio_tx: Option<tokio_mpsc::UnboundedSender<AudioCmd>>,
    pending_chunks: Arc<std::sync::atomic::AtomicUsize>,
    #[allow(dead_code)]
    session_handle: Option<tokio::task::JoinHandle<()>>,
}

impl SonioxClient {
    pub fn new(config: SonioxConfig) -> Self {
        let (event_tx, event_rx) = crossbeam_channel::bounded(256);
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

    pub fn connect(&mut self) -> Result<(), String> {
        if self.config.api_key.trim().is_empty() {
            return Err("Soniox API key is not configured".to_string());
        }
        if self.config.model.trim().is_empty() {
            return Err("Soniox model is not configured".to_string());
        }

        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("soniox-ws-rt")
            .build()
            .map_err(|e| format!("Failed to create tokio runtime: {e}"))?;

        let config = self.config.clone();
        let event_tx = self.event_tx.clone();
        let connected = Arc::clone(&self.connected);
        let user_disconnected = Arc::clone(&self.user_disconnected);
        user_disconnected.store(false, Ordering::SeqCst);
        self.pending_chunks
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let pending_chunks = Arc::clone(&self.pending_chunks);

        let (audio_tx, session_handle) = rt.block_on(async move {
            let (writer, reader) = open_ws(&config).await?;
            connected.store(true, Ordering::SeqCst);
            let _ = event_tx.send(SonioxEvent::Connected);
            let (atx, arx) = tokio_mpsc::unbounded_channel::<AudioCmd>();
            let session_handle = tokio::spawn(session_task(SonioxSessionCtx {
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

    pub fn send_audio(&self, audio: &[f32]) -> Result<(), String> {
        if self.user_disconnected.load(Ordering::SeqCst) {
            return Err("Soniox client has been disconnected".to_string());
        }
        if audio.is_empty() {
            return Ok(());
        }

        self.config
            .content_egress_policy
            .check_audio("asr.soniox")?;

        let tx = self
            .audio_tx
            .as_ref()
            .ok_or_else(|| "Audio channel not initialized".to_string())?;
        let depth = self
            .pending_chunks
            .load(std::sync::atomic::Ordering::Relaxed);
        if depth >= AUDIO_BUFFER_MAX_CHUNKS {
            self.user_disconnected
                .store(true, std::sync::atomic::Ordering::SeqCst);
            return Err(format!(
                "Soniox audio buffer full ({depth} chunks) — likely a stuck reconnect. Restart the session."
            ));
        }

        let pcm_bytes = crate::audio::pcm::f32_mono_to_pcm_s16le_bytes(audio);
        self.pending_chunks
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        tx.send(AudioCmd::Chunk(pcm_bytes)).map_err(|_| {
            self.pending_chunks
                .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            "Audio channel closed".to_string()
        })
    }

    pub fn event_rx(&self) -> crossbeam_channel::Receiver<SonioxEvent> {
        self.event_rx.clone()
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn disconnect(&self) {
        log::info!("SonioxClient: disconnecting (user-initiated)");
        self.user_disconnected.store(true, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);
        if let Some(ref tx) = self.audio_tx {
            let _ = tx.send(AudioCmd::Stop);
        }
    }
}

impl Drop for SonioxClient {
    fn drop(&mut self) {
        self.user_disconnected.store(true, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);
        if let Some(ref tx) = self.audio_tx {
            let _ = tx.send(AudioCmd::Stop);
        }
        self.audio_tx = None;
        if let Some(rt) = self.rt.take() {
            rt.shutdown_timeout(Duration::from_secs(3));
        }
        log::info!("SonioxClient: dropped");
    }
}

#[derive(Debug)]
#[allow(dead_code)]
enum DisconnectKind {
    ServerClose(String),
    NetworkError(String),
    PolicyBlocked(String),
    ProtocolError(String),
    UserRequested,
    WriterEnded,
}

async fn open_ws(config: &SonioxConfig) -> Result<(AsrWsWriter, AsrWsReader), String> {
    open_ws_url(config, WEBSOCKET_URL).await
}

async fn open_ws_url(
    config: &SonioxConfig,
    url_str: &str,
) -> Result<(AsrWsWriter, AsrWsReader), String> {
    let (ws_stream, _response) = connect_async(url_str).await.map_err(|e| {
        crate::error::redacted_provider_diagnostic(
            &format!("WebSocket connect failed: {e}"),
            [&config.api_key],
        )
    })?;
    let (mut writer, reader) = ws_stream.split();
    let payload = soniox_session_config_payload(config);
    AsrWsWriteGuard::new("asr.soniox", config.content_egress_policy)
        .send_text(
            &mut writer,
            AsrTransportPayloadKind::SessionJson,
            payload.to_string(),
        )
        .await
        .map_err(|e| {
            crate::error::redacted_provider_diagnostic(
                &format!("Soniox config send failed: {e}"),
                [&config.api_key],
            )
        })?;
    Ok((writer, reader))
}

#[cfg(test)]
type ReconnectOpenFuture =
    Pin<Box<dyn Future<Output = Result<(AsrWsWriter, AsrWsReader), String>> + Send>>;

#[cfg(test)]
type ReconnectOpener = Arc<dyn Fn(SonioxConfig) -> ReconnectOpenFuture + Send + Sync>;

#[cfg(test)]
async fn open_reconnect_ws(
    config: &SonioxConfig,
    opener: Option<&ReconnectOpener>,
) -> Result<(AsrWsWriter, AsrWsReader), String> {
    if let Some(opener) = opener {
        opener(config.clone()).await
    } else {
        open_ws(config).await
    }
}

struct SonioxSessionCtx {
    writer: AsrWsWriter,
    reader: AsrWsReader,
    audio_rx: tokio_mpsc::UnboundedReceiver<AudioCmd>,
    config: SonioxConfig,
    event_tx: crossbeam_channel::Sender<SonioxEvent>,
    connected: Arc<AtomicBool>,
    user_disconnected: Arc<AtomicBool>,
    pending_chunks: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    reconnect_opener: Option<ReconnectOpener>,
    #[cfg(test)]
    run_io_entries: Option<Arc<std::sync::atomic::AtomicUsize>>,
}

async fn session_task(ctx: SonioxSessionCtx) {
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
    let mut reconnect_attempts: u32 = 0;
    let mut parser = SonioxRealtimeParser::new(config.source_id.clone());
    let write_guard = AsrWsWriteGuard::new("asr.soniox", config.content_egress_policy);

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
            &mut parser,
            &write_guard,
            &config.api_key,
        )
        .await;

        connected.store(false, Ordering::SeqCst);

        match disconnect {
            DisconnectKind::UserRequested | DisconnectKind::WriterEnded => {
                log::info!("Soniox session: ending ({disconnect:?})");
                let _ = event_tx.send(SonioxEvent::Disconnected);
                break;
            }
            DisconnectKind::PolicyBlocked(message) => {
                log::warn!("Soniox session: content egress blocked: {message}");
                let _ = event_tx.send(SonioxEvent::Error { message });
                let _ = event_tx.send(SonioxEvent::Disconnected);
                break;
            }
            _ => {
                if user_disconnected.load(Ordering::SeqCst) {
                    let _ = event_tx.send(SonioxEvent::Disconnected);
                    break;
                }

                log::warn!("Soniox session: disconnected — {disconnect:?}");
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
                                "Soniox session: reconnect budget exhausted after {attempted} attempts"
                            );
                            let _ = event_tx.send(SonioxEvent::Error {
                                message: "Soniox reconnect attempts exhausted".into(),
                            });
                            let _ = event_tx.send(SonioxEvent::Disconnected);
                            break false;
                        }
                    };

                    log::info!(
                        "Soniox session: reconnecting (attempt {attempt}, backoff {backoff}s)"
                    );
                    let _ = event_tx.send(SonioxEvent::Reconnecting {
                        attempt,
                        backoff_secs: backoff,
                    });

                    let sleep = tokio::time::sleep(Duration::from_secs(backoff));
                    tokio::pin!(sleep);
                    loop {
                        tokio::select! {
                            _ = &mut sleep => break,
                            _ = tokio::time::sleep(Duration::from_millis(100)) => {
                                if user_disconnected.load(Ordering::SeqCst) {
                                    log::info!("Soniox session: user cancelled during backoff");
                                    let _ = event_tx.send(SonioxEvent::Disconnected);
                                    return;
                                }
                            }
                        }
                    }

                    if user_disconnected.load(Ordering::SeqCst) {
                        let _ = event_tx.send(SonioxEvent::Disconnected);
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
                            // Finalize (not silently abandon) the pre-drop turn so
                            // its provisional span is superseded rather than left
                            // orphaned as a never-finalized partial in the ledger
                            // (M3 / audio-graph-c6de). Falls back to a plain
                            // abandon when the turn carried no emitted text.
                            if let Some(revision) =
                                parser.finalize_active_turn_for_reconnect(current_unix_millis())
                            {
                                let _ = event_tx.send(SonioxEvent::Revision(revision));
                            }
                            connected.store(true, Ordering::SeqCst);
                            log::info!("Soniox session: reconnected on attempt {attempt}");
                            let _ = event_tx.send(SonioxEvent::Reconnected);
                            reconnect_attempts = 0;
                            break true;
                        }
                        Err(e) => {
                            // Redact: a reconnect error can embed the upgrade
                            // request (api_key) or URL userinfo, so scrub the key
                            // before it reaches logs or the UI.
                            let diag = crate::error::redacted_provider_diagnostic(
                                &format!("Reconnect attempt {attempt} failed: {e}"),
                                [&config.api_key],
                            );
                            log::warn!("Soniox session: {diag}");
                            let _ = event_tx.send(SonioxEvent::Error { message: diag });
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
    log::info!("Soniox: session task exited");
}

#[allow(clippy::too_many_arguments)]
async fn run_io(
    writer: &mut AsrWsWriter,
    reader: &mut AsrWsReader,
    audio_rx: &mut tokio_mpsc::UnboundedReceiver<AudioCmd>,
    event_tx: &crossbeam_channel::Sender<SonioxEvent>,
    user_disconnected: &Arc<AtomicBool>,
    pending_chunks: &Arc<std::sync::atomic::AtomicUsize>,
    parser: &mut SonioxRealtimeParser,
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
        parser,
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
    event_tx: &crossbeam_channel::Sender<SonioxEvent>,
    user_disconnected: &Arc<AtomicBool>,
    pending_chunks: &Arc<std::sync::atomic::AtomicUsize>,
    parser: &mut SonioxRealtimeParser,
    write_guard: &AsrWsWriteGuard,
    api_key: &str,
    keepalive_interval: Duration,
) -> DisconnectKind {
    let mut finishing = false;
    let mut keep_alive = tokio::time::interval(keepalive_interval);
    keep_alive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_outbound = tokio::time::Instant::now();

    loop {
        tokio::select! {
            // Idle keepalive: a WS Ping control frame during quiet periods keeps
            // the Soniox socket warm when the audio cadence stalls (M2 /
            // audio-graph-63be). Suppressed once `finishing` so it never races
            // the finalize/close handshake, and guarded by `last_outbound` so it
            // never fires while audio is actively flowing.
            _ = keep_alive.tick(), if !finishing => {
                if last_outbound.elapsed() >= keepalive_interval {
                    if let Err(e) = write_guard.send_ping(writer, Vec::new()).await {
                        let message = crate::error::redacted_provider_diagnostic(
                            &format!("keepalive failed: {e}"),
                            [api_key],
                        );
                        log::error!("Soniox: failed to send keepalive: {message}");
                        return DisconnectKind::NetworkError(message);
                    }
                    last_outbound = tokio::time::Instant::now();
                }
            }

            cmd = audio_rx.recv(), if !finishing => {
                match cmd {
                    Some(AudioCmd::Chunk(bytes)) => {
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
                            log::error!("Soniox: failed to send audio: {message}");
                            return if policy_blocked {
                                DisconnectKind::PolicyBlocked(message)
                            } else {
                                DisconnectKind::NetworkError(message)
                            };
                        }
                        last_outbound = tokio::time::Instant::now();
                    }
                    Some(AudioCmd::Stop) => {
                        finishing = true;
                        let _ = write_guard
                            .send_binary(writer, AsrTransportPayloadKind::Terminal, Vec::new())
                            .await;
                    }
                    None => {
                        let _ = write_guard
                            .send_binary(writer, AsrTransportPayloadKind::Terminal, Vec::new())
                            .await;
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
                        let finished =
                            handle_server_message_with_key(&text, event_tx, parser, api_key);
                        if finished && finishing {
                            let _ = writer.close().await;
                            return DisconnectKind::UserRequested;
                        }
                    }
                    Ok(Message::Close(frame)) => {
                        let safe_frame = crate::error::redacted_provider_diagnostic(
                            &format!("{frame:?}"),
                            [api_key],
                        );
                        log::info!("Soniox: server closed connection: {safe_frame}");
                        if user_disconnected.load(Ordering::SeqCst) || finishing {
                            return DisconnectKind::UserRequested;
                        }
                        let reason = frame
                            .map(|f| {
                                crate::error::redacted_provider_diagnostic(
                                    &format!("{} {}", f.code, f.reason),
                                    [api_key],
                                )
                            })
                            .unwrap_or_else(|| "no frame".into());
                        return DisconnectKind::ServerClose(reason);
                    }
                    Ok(Message::Ping(_) | Message::Pong(_) | Message::Frame(_)) => {}
                    Ok(Message::Binary(_)) => {
                        log::debug!("Soniox: unexpected binary message from server");
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
                        log::error!("Soniox: WebSocket read error: {message}");
                        return DisconnectKind::NetworkError(message);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
fn handle_server_message(
    text: &str,
    tx: &crossbeam_channel::Sender<SonioxEvent>,
    parser: &mut SonioxRealtimeParser,
) -> bool {
    handle_server_message_with_key(text, tx, parser, "")
}

fn handle_server_message_with_key(
    text: &str,
    tx: &crossbeam_channel::Sender<SonioxEvent>,
    parser: &mut SonioxRealtimeParser,
    api_key: &str,
) -> bool {
    let parsed = match parser.parse_message(text, current_unix_millis()) {
        Ok(parsed) => parsed,
        Err(SonioxParseError::InvalidJson(error)) => {
            let _ = tx.send(SonioxEvent::Error {
                message: format!("Invalid server JSON: {error}"),
            });
            return false;
        }
    };

    if let Some(error) = parsed.error {
        let message =
            crate::error::redacted_provider_diagnostic(&format_soniox_error(&error), [api_key]);
        let _ = tx.send(SonioxEvent::Error { message });
    }

    for revision in parsed.revisions {
        let _ = tx.send(SonioxEvent::Revision(revision));
    }

    if parsed.finished {
        let _ = tx.send(SonioxEvent::Finished);
    }

    parsed.finished
}

fn soniox_session_config_payload(config: &SonioxConfig) -> Value {
    let mut payload = json!({
        "api_key": config.api_key,
        "model": config.model,
        "audio_format": "pcm_s16le",
        "sample_rate": 16_000,
        "num_channels": 1,
        "enable_speaker_diarization": config.enable_diarization,
        "enable_language_identification": config.enable_language_identification,
        "enable_endpoint_detection": true,
    });
    if !config.language_hints.is_empty() {
        payload["language_hints"] = json!(config.language_hints);
    }
    payload
}

fn format_soniox_error(error: &SonioxProviderError) -> String {
    let mut message = String::from("Soniox error");
    if let Some(code) = &error.code {
        message.push_str(&format!(" code={code}"));
    }
    if let Some(error_type) = &error.error_type {
        message.push_str(&format!(" type={error_type}"));
    }
    message.push_str(&format!(": {}", error.message));
    if let Some(request_id) = &error.request_id {
        message.push_str(&format!(" request_id={request_id}"));
    }
    message
}

fn current_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl SonioxActiveTurn {
    fn combined_tokens(&self) -> Vec<SonioxToken> {
        self.final_tokens
            .iter()
            .chain(self.non_final_tokens.iter())
            .cloned()
            .collect()
    }
}

fn token_without_markers(token: &SonioxToken) -> (Option<SonioxToken>, bool) {
    let mut text = token.text.clone();
    let had_marker = contains_turn_marker(&text);
    text = text.replace("<end>", "").replace("<fin>", "");
    let mut cleaned = token.clone();
    cleaned.text = text;

    if cleaned.text.is_empty() || (had_marker && cleaned.text.trim().is_empty()) {
        (None, had_marker)
    } else {
        (Some(cleaned), had_marker)
    }
}

fn joined_text(tokens: &[SonioxToken]) -> String {
    tokens
        .iter()
        .map(|token| token.text.as_str())
        .collect::<String>()
        .trim()
        .to_string()
}

fn min_start_ms(tokens: &[SonioxToken]) -> Option<u64> {
    tokens.iter().filter_map(|token| token.start_ms).min()
}

fn max_end_ms(tokens: &[SonioxToken]) -> Option<u64> {
    tokens.iter().filter_map(|token| token.end_ms).max()
}

fn average_confidence(tokens: &[SonioxToken]) -> f32 {
    let mut total = 0.0;
    let mut count = 0usize;
    for confidence in tokens.iter().filter_map(|token| token.confidence) {
        total += confidence;
        count += 1;
    }

    if count == 0 {
        0.0
    } else {
        total / count as f32
    }
}

fn consistent_token_field(
    tokens: &[SonioxToken],
    field: impl Fn(&SonioxToken) -> Option<&str>,
) -> Option<String> {
    let mut value = None::<&str>;
    for token in tokens.iter().filter(|token| !token.text.trim().is_empty()) {
        let token_value = field(token)?;
        match value {
            Some(current) if current != token_value => return None,
            Some(_) => {}
            None => value = Some(token_value),
        }
    }
    value.map(str::to_string)
}

fn revision_ref(span_id: &str, revision_number: u64) -> String {
    format!("{span_id}@rev{revision_number}")
}

fn soniox_span_id(source_id: &str, turn_index: u64) -> String {
    format!("{PROVIDER}:{source_id}:turn-{turn_index}")
}

fn millis_to_secs(ms: u64) -> f64 {
    ms as f64 / 1000.0
}

fn contains_turn_marker(text: &str) -> bool {
    text.contains("<end>") || text.contains("<fin>")
}

fn is_marker_only(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed == "<end>" || trimmed == "<fin>"
}

fn json_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::ws_fixture;
    use crate::projections::{TranscriptEvent, TranscriptLedger};

    fn test_config() -> SonioxConfig {
        SonioxConfig {
            api_key: "soniox-test-key".into(),
            model: DEFAULT_MODEL.into(),
            source_id: "mixed".into(),
            enable_diarization: true,
            enable_language_identification: true,
            language_hints: vec!["en".into(), "es".into()],
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        }
    }

    fn with_blocked_content_egress(mut config: SonioxConfig) -> SonioxConfig {
        config.api_key = "soniox-private-api-key".into();
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
        rx: &crossbeam_channel::Receiver<SonioxEvent>,
        timeout: Duration,
    ) -> SonioxEvent {
        tokio::time::timeout(timeout, async {
            loop {
                if let Ok(event) = rx.try_recv() {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("timed out waiting for Soniox event")
    }

    #[test]
    fn soniox_config_debug_redacts_api_key() {
        let config = SonioxConfig {
            api_key: "soniox-debug-secret".into(),
            model: DEFAULT_MODEL.into(),
            source_id: "mixed".into(),
            enable_diarization: true,
            enable_language_identification: true,
            language_hints: vec!["en".into()],
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        };

        let debug = format!("{config:?}");

        assert!(!debug.contains("soniox-debug-secret"));
        assert!(debug.contains("<present>"));
        assert!(debug.contains(DEFAULT_MODEL));
    }

    #[test]
    fn blocked_policy_rejects_non_empty_audio_before_channel_initialization() {
        let client = SonioxClient::new(with_blocked_content_egress(test_config()));

        let error = client.send_audio(&[0.5, -0.3]).unwrap_err();

        assert!(error.contains("Privacy policy blocked"));
        assert!(error.contains("asr.soniox"));
        assert!(error.contains("local_only"));
        assert!(!error.contains("Audio channel not initialized"));
    }

    #[test]
    fn blocked_policy_allows_empty_audio_without_channel_initialization() {
        let client = SonioxClient::new(with_blocked_content_egress(test_config()));

        assert!(client.send_audio(&[]).is_ok());
    }

    #[test]
    fn blocked_policy_error_redacts_secret_audio_and_transcript_like_values() {
        let client = SonioxClient::new(with_blocked_content_egress(test_config()));

        let error = client.send_audio(&[0.5, -0.3]).unwrap_err();

        for forbidden in [
            "soniox-private-api-key",
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

    #[tokio::test(flavor = "current_thread")]
    async fn open_ws_sends_config_as_first_frame() {
        let (config_tx, config_rx) = tokio::sync::oneshot::channel();
        let (url, server) = ws_fixture::spawn_server(move |mut websocket| async move {
            let first = websocket
                .next()
                .await
                .expect("first Soniox client frame")
                .expect("first frame ok");
            let Message::Text(text) = first else {
                panic!("first Soniox frame should be text config, got {first:?}");
            };
            let parsed: Value = serde_json::from_str(&text).expect("config json");
            let _ = config_tx.send(parsed);
            let _ = websocket.close(None).await;
        })
        .await;

        let (mut writer, _reader) = open_ws_url(&test_config(), &url)
            .await
            .expect("connect to fake Soniox server");
        let _ = writer.close().await;

        let config = tokio::time::timeout(Duration::from_secs(1), config_rx)
            .await
            .expect("server should receive config")
            .expect("config channel should not drop");
        assert_eq!(
            config.get("api_key").and_then(Value::as_str),
            Some("soniox-test-key")
        );
        assert_eq!(
            config.get("model").and_then(Value::as_str),
            Some(DEFAULT_MODEL)
        );
        assert_eq!(
            config.get("audio_format").and_then(Value::as_str),
            Some("pcm_s16le")
        );
        assert_eq!(
            config.get("sample_rate").and_then(Value::as_u64),
            Some(16_000)
        );
        assert_eq!(config.get("num_channels").and_then(Value::as_u64), Some(1));
        assert_eq!(
            config
                .get("enable_speaker_diarization")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            config
                .get("enable_language_identification")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            config
                .get("enable_endpoint_detection")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            config
                .get("language_hints")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );

        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn open_ws_blocked_policy_writes_no_session_config_frame() {
        let (frame_tx, frame_rx) = tokio::sync::oneshot::channel();
        let (url, server) = ws_fixture::spawn_server(move |websocket| async move {
            let _ = frame_tx.send(first_client_content_frame(websocket).await);
        })
        .await;

        let config = with_blocked_content_egress(test_config());
        let error = open_ws_url(&config, &url)
            .await
            .expect_err("blocked policy should reject Soniox session config write");

        assert!(error.contains("Privacy policy blocked"));
        assert!(error.contains("asr.soniox"));
        assert!(error.contains("local_only"));
        assert!(!error.contains("soniox-private-api-key"));

        let observed = tokio::time::timeout(Duration::from_secs(1), frame_rx)
            .await
            .expect("server should report whether a content frame arrived")
            .expect("server frame channel should not drop");
        assert_eq!(
            observed, None,
            "blocked session config must not write a text or binary content frame"
        );

        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
    }

    #[tokio::test(flavor = "current_thread")]
    async fn run_io_writes_binary_audio_reads_revision_and_finalizes_with_empty_frame() {
        let (client_frames_tx, client_frames_rx) = tokio::sync::oneshot::channel();
        let (url, server) = ws_fixture::spawn_server(move |mut websocket| async move {
            let mut frames = Vec::new();
            while let Some(frame) = websocket.next().await {
                match frame.expect("server frame") {
                    Message::Binary(bytes) => {
                        let is_empty = bytes.is_empty();
                        frames.push(bytes.to_vec());
                        if is_empty {
                            websocket
                                .send(Message::Text(
                                    r#"{"tokens":[],"final_audio_proc_ms":220,"total_audio_proc_ms":240,"finished":true}"#
                                        .into(),
                                ))
                                .await
                                .expect("send finished response");
                            break;
                        }
                        websocket
                            .send(Message::Text(
                                r#"{"tokens":[{"text":"hello","start_ms":100,"end_ms":220,"confidence":0.91,"is_final":false,"speaker":"1","language":"en"}],"total_audio_proc_ms":240}"#
                                    .into(),
                            ))
                            .await
                            .expect("send partial response");
                    }
                    Message::Close(_) => break,
                    other => panic!("unexpected client frame: {other:?}"),
                }
            }
            let _ = client_frames_tx.send(frames);
        })
        .await;

        let client_socket = ws_fixture::connect_client(&url).await;
        let (mut writer, mut reader) = client_socket.split();
        let (audio_tx, mut audio_rx) = tokio_mpsc::unbounded_channel();
        let (event_tx, event_rx) = crossbeam_channel::bounded(16);
        let user_disconnected = Arc::new(AtomicBool::new(false));
        let pending_chunks = Arc::new(std::sync::atomic::AtomicUsize::new(1));
        let mut parser = SonioxRealtimeParser::new("mixed");
        let write_guard = AsrWsWriteGuard::new(
            "asr.soniox",
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
                    &mut parser,
                    &write_guard,
                    "soniox-test-key",
                )
                .await
            }
        });

        audio_tx
            .send(AudioCmd::Chunk(vec![1, 2, 3, 4]))
            .expect("queue binary audio");
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            SonioxEvent::Revision(revision) => {
                assert_eq!(revision.payload.provider, "soniox");
                assert_eq!(revision.payload.source_id, "mixed");
                assert_eq!(revision.payload.text, "hello");
                assert!(!revision.payload.is_final);
                assert_eq!(revision.language.as_deref(), Some("en"));
            }
            other => panic!("expected Soniox revision, got {other:?}"),
        }

        audio_tx.send(AudioCmd::Stop).expect("queue stop");
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            SonioxEvent::Revision(revision) => {
                assert_eq!(revision.payload.text, "hello");
                assert!(revision.payload.is_final);
                assert_eq!(revision.payload.revision_number, 2);
                assert_eq!(
                    revision.payload.supersedes.as_deref(),
                    Some("soniox:mixed:turn-1@rev1")
                );
            }
            other => panic!("expected Soniox final revision, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            SonioxEvent::Finished => {}
            other => panic!("expected Soniox finished event, got {other:?}"),
        }

        let disconnect = tokio::time::timeout(Duration::from_secs(2), run)
            .await
            .expect("run_io should exit after finished")
            .expect("run_io task panicked");
        assert!(
            matches!(disconnect, DisconnectKind::UserRequested),
            "stop + finished should be user-requested, got {disconnect:?}"
        );
        assert_eq!(
            pending_chunks.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "sent audio chunk must decrement pending count"
        );
        let frames = tokio::time::timeout(Duration::from_secs(1), client_frames_rx)
            .await
            .expect("server should report client frames")
            .expect("server oneshot dropped");
        assert_eq!(frames.first().map(Vec::as_slice), Some(&[1, 2, 3, 4][..]));
        assert_eq!(frames.last().map(Vec::as_slice), Some(&[][..]));

        tokio::time::timeout(Duration::from_secs(1), server)
            .await
            .expect("server task should finish")
            .expect("server task panicked");
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
        let mut parser = SonioxRealtimeParser::new("mixed");
        let write_guard = AsrWsWriteGuard::new(
            "asr.soniox",
            crate::asr::ProviderContentEgressPolicy::block("local_only"),
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
                    &mut parser,
                    &write_guard,
                    "soniox-private-api-key",
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
                assert!(message.contains("asr.soniox"));
                assert!(message.contains("local_only"));
                assert!(!message.contains("soniox-private-api-key"));
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
                    Err("fake Soniox reconnect failure".to_string())
                })
            })
        };

        let handle = tokio::spawn(session_task(SonioxSessionCtx {
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
            SonioxEvent::Reconnecting {
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
            SonioxEvent::Error { message } => {
                assert!(message.contains("Reconnect attempt 1 failed"));
            }
            other => panic!("expected reconnect failure error, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            SonioxEvent::Reconnecting {
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
            "failed reconnect must not re-enter run_io with stale socket halves"
        );

        user_disconnected.store(true, Ordering::SeqCst);
        match recv_event(&event_rx, Duration::from_secs(2)).await {
            SonioxEvent::Disconnected => {}
            other => panic!("expected cancellation disconnect, got {other:?}"),
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
                .all(|event| !matches!(event, SonioxEvent::Reconnected)),
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
        let (reconnected_frames_tx, mut reconnected_frames_rx) =
            tokio::sync::mpsc::unbounded_channel::<(Value, Vec<Vec<u8>>)>();

        let opener: ReconnectOpener = {
            let opener_calls = Arc::clone(&opener_calls);
            Arc::new(move |config| {
                let opener_calls = Arc::clone(&opener_calls);
                let reconnected_frames_tx = reconnected_frames_tx.clone();
                Box::pin(async move {
                    opener_calls.fetch_add(1, Ordering::SeqCst);
                    let (url, _server) = ws_fixture::spawn_server(move |mut websocket| async move {
                        let config_frame = websocket
                            .next()
                            .await
                            .expect("reconnected Soniox config frame")
                            .expect("reconnected config frame ok");
                        let Message::Text(config_text) = config_frame else {
                            panic!(
                                "reconnected Soniox first frame should be config text, got {config_frame:?}"
                            );
                        };
                        let config_json: Value =
                            serde_json::from_str(&config_text).expect("Soniox config json");

                        websocket
                            .send(Message::Text(
                                r#"{"tokens":[{"text":"after","start_ms":100,"end_ms":180,"confidence":0.91,"is_final":true,"speaker":"1","language":"en"},{"text":" reconnect<end>","start_ms":180,"end_ms":360,"confidence":0.9,"is_final":true,"speaker":"1","language":"en"}],"final_audio_proc_ms":360,"total_audio_proc_ms":390}"#
                                    .into(),
                            ))
                            .await
                            .expect("send fake reconnected transcript");

                        let mut binary_frames = Vec::new();
                        while let Some(frame) = websocket.next().await {
                            match frame.expect("reconnected Soniox server frame") {
                                Message::Binary(bytes) => {
                                    let is_empty = bytes.is_empty();
                                    binary_frames.push(bytes.to_vec());
                                    if is_empty {
                                        websocket
                                            .send(Message::Text(
                                                r#"{"tokens":[],"finished":true}"#.into(),
                                            ))
                                            .await
                                            .expect("send fake finished response");
                                        break;
                                    }
                                }
                                Message::Close(_) => break,
                                _ => {}
                            }
                        }
                        let _ = reconnected_frames_tx.send((config_json, binary_frames));
                    })
                    .await;

                    open_ws_url(&config, &url).await
                })
            })
        };

        let handle = tokio::spawn(session_task(SonioxSessionCtx {
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
            SonioxEvent::Reconnecting {
                attempt,
                backoff_secs,
            } => {
                assert_eq!(attempt, 1);
                assert_eq!(backoff_secs, 1);
            }
            other => panic!("expected first Reconnecting event, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(3)).await {
            SonioxEvent::Reconnected => {}
            other => panic!("expected Reconnected event, got {other:?}"),
        }
        assert!(
            connected.load(Ordering::SeqCst),
            "successful reconnect must mark the session connected"
        );

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            SonioxEvent::Revision(revision) => {
                assert_eq!(revision.payload.text, "after reconnect");
                assert!(revision.payload.is_final);
                assert!(revision.payload.end_of_turn);
                assert_eq!(revision.language.as_deref(), Some("en"));
            }
            other => panic!("expected revision from reconnected socket, got {other:?}"),
        }

        pending_chunks.store(1, Ordering::SeqCst);
        audio_tx
            .send(AudioCmd::Chunk(vec![0x73, 0x6f, 0x6e]))
            .expect("queue audio after reconnect");
        audio_tx.send(AudioCmd::Stop).expect("queue stop");

        match recv_event(&event_rx, Duration::from_secs(1)).await {
            SonioxEvent::Finished => {}
            other => panic!("expected Soniox finished event after stop, got {other:?}"),
        }
        match recv_event(&event_rx, Duration::from_secs(1)).await {
            SonioxEvent::Disconnected => {}
            other => panic!("expected final Disconnected after clean stop, got {other:?}"),
        }
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

        let (config_json, binary_frames) =
            tokio::time::timeout(Duration::from_secs(1), reconnected_frames_rx.recv())
                .await
                .expect("reconnected server should report frames")
                .expect("reconnected server sender dropped");
        assert_eq!(
            config_json.get("model").and_then(Value::as_str),
            Some(DEFAULT_MODEL)
        );
        assert_eq!(
            config_json
                .get("enable_endpoint_detection")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            binary_frames.first().map(Vec::as_slice),
            Some(&[0x73, 0x6f, 0x6e][..])
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
    #[ignore = "requires SONIOX_API_KEY and live Soniox network access"]
    async fn live_smoke_soniox_websocket_accepts_config_audio_and_finish() {
        let api_key = std::env::var("SONIOX_API_KEY")
            .expect("set SONIOX_API_KEY to run the ignored Soniox live smoke");
        let api_key = api_key.trim().to_string();
        assert!(
            !api_key.is_empty(),
            "SONIOX_API_KEY must not be empty for live smoke"
        );

        let config = SonioxConfig {
            api_key: api_key.clone(),
            model: DEFAULT_MODEL.to_string(),
            source_id: "live-smoke".to_string(),
            enable_diarization: false,
            enable_language_identification: true,
            language_hints: vec!["en".to_string()],
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::allow(),
        };
        let (mut writer, mut reader) = open_ws(&config).await.unwrap_or_else(|error| {
            panic!(
                "Soniox live smoke connect/config failed: {}",
                crate::error::redacted_provider_diagnostic(&error, [&api_key])
            )
        });

        let one_second_silence_pcm16 = vec![0_u8; 16_000 * 2];
        writer
            .send(Message::Binary(one_second_silence_pcm16.into()))
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "Soniox live smoke audio send failed: {}",
                    crate::error::redacted_provider_diagnostic(&error.to_string(), [&api_key])
                )
            });
        writer
            .send(Message::Binary(Vec::new().into()))
            .await
            .unwrap_or_else(|error| {
                panic!(
                    "Soniox live smoke finalization send failed: {}",
                    crate::error::redacted_provider_diagnostic(&error.to_string(), [&api_key])
                )
            });

        let (event_tx, event_rx) = crossbeam_channel::bounded(32);
        let mut parser = SonioxRealtimeParser::new("live-smoke");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let mut saw_text_response = false;
        let mut saw_finished = false;

        while tokio::time::Instant::now() < deadline {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let Some(message) = tokio::time::timeout(remaining, reader.next())
                .await
                .expect("timed out waiting for Soniox live smoke response")
            else {
                break;
            };

            match message.unwrap_or_else(|error| {
                panic!(
                    "Soniox live smoke read failed: {}",
                    crate::error::redacted_provider_diagnostic(&error.to_string(), [&api_key])
                )
            }) {
                Message::Text(text) => {
                    saw_text_response = true;
                    if handle_server_message_with_key(&text, &event_tx, &mut parser, &api_key) {
                        saw_finished = true;
                    }
                    while let Ok(event) = event_rx.try_recv() {
                        match event {
                            SonioxEvent::Finished => saw_finished = true,
                            SonioxEvent::Error { message } => panic!(
                                "Soniox live smoke provider error: {}",
                                crate::error::redacted_provider_diagnostic(&message, [&api_key])
                            ),
                            _ => {}
                        }
                    }
                    if saw_finished {
                        break;
                    }
                }
                Message::Close(frame) => {
                    if !saw_finished {
                        panic!(
                            "Soniox live smoke closed before finished response: {}",
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
        assert!(
            saw_text_response,
            "Soniox live smoke returned no text frames"
        );
        assert!(
            saw_finished,
            "Soniox live smoke returned no finished response"
        );
    }

    #[test]
    fn server_error_message_redacts_provider_credentials() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let api_key = "soniox-server-secret";
        let mut parser = SonioxRealtimeParser::new("mixed");

        handle_server_message_with_key(
            &format!(
                r#"{{"tokens":[],"error_code":401,"error_type":"unauthorized","error_message":"bad key {api_key} Authorization: Bearer bearer-soniox-secret wss://user:pass@example.com?api_key=url-soniox-secret"}}"#
            ),
            &tx,
            &mut parser,
            api_key,
        );

        match rx.recv().expect("error event") {
            SonioxEvent::Error { message } => {
                for leaked in [
                    api_key,
                    "bearer-soniox-secret",
                    "user:pass",
                    "url-soniox-secret",
                ] {
                    assert!(
                        !message.contains(leaked),
                        "Soniox server error leaked {leaked}: {message}"
                    );
                }
                assert!(message.contains("<redacted>"));
            }
            other => panic!("expected error event, got {other:?}"),
        }
    }

    #[test]
    fn handle_invalid_json_emits_error_event() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        let mut parser = SonioxRealtimeParser::new("mixed");

        assert!(!handle_server_message("{", &tx, &mut parser));

        match rx.recv().expect("error event") {
            SonioxEvent::Error { message } => {
                assert!(message.contains("Invalid server JSON"));
            }
            other => panic!("expected error event, got {other:?}"),
        }
    }

    #[test]
    fn backoff_schedule_matches_streaming_provider_spec() {
        assert_eq!(backoff_for_attempt(1), Some(1));
        assert_eq!(backoff_for_attempt(2), Some(2));
        assert_eq!(backoff_for_attempt(3), Some(5));
        assert_eq!(backoff_for_attempt(4), Some(10));
        assert_eq!(backoff_for_attempt(5), None);
    }

    #[test]
    fn partial_then_final_revisions_share_turn_span_and_replay_without_duplicates() {
        let mut parser = SonioxRealtimeParser::new("mic-1");

        let partial = parser
            .parse_message(
                r#"{
                    "tokens": [
                        {
                            "text": "hello",
                            "start_ms": 600,
                            "end_ms": 760,
                            "confidence": 0.92,
                            "is_final": false,
                            "speaker": "1",
                            "language": "en"
                        }
                    ],
                    "total_audio_proc_ms": 820
                }"#,
                1_700_000_000_001,
            )
            .unwrap();

        let final_message = parser
            .parse_message(
                r#"{
                    "tokens": [
                        {
                            "text": "hello",
                            "start_ms": 600,
                            "end_ms": 760,
                            "confidence": 0.96,
                            "is_final": true,
                            "speaker": "1",
                            "language": "en"
                        },
                        { "text": "<end>", "is_final": true }
                    ],
                    "final_audio_proc_ms": 1040,
                    "total_audio_proc_ms": 1100
                }"#,
                1_700_000_000_002,
            )
            .unwrap();

        assert_eq!(partial.revisions.len(), 1);
        assert_eq!(final_message.revisions.len(), 1);

        let partial_revision = &partial.revisions[0];
        let final_revision = &final_message.revisions[0];
        let span_id = "soniox:mic-1:turn-1";

        assert_eq!(partial_revision.payload.span_id, span_id);
        assert_eq!(
            partial_revision.payload.provider_item_id.as_deref(),
            Some("turn-1")
        );
        assert_eq!(partial_revision.payload.revision_number, 1);
        assert_eq!(partial_revision.payload.supersedes, None);
        assert!(!partial_revision.payload.is_final);
        assert_eq!(partial_revision.payload.speaker_id.as_deref(), Some("1"));
        assert_eq!(partial_revision.language.as_deref(), Some("en"));
        assert_eq!(partial_revision.total_audio_proc_ms, Some(820));

        assert_eq!(final_revision.payload.span_id, span_id);
        assert_eq!(
            final_revision.payload.provider_item_id.as_deref(),
            Some("turn-1")
        );
        assert_eq!(final_revision.payload.revision_number, 2);
        assert_eq!(
            final_revision.payload.supersedes.as_deref(),
            Some("soniox:mic-1:turn-1@rev1")
        );
        assert!(final_revision.payload.is_final);
        assert!(final_revision.payload.end_of_turn);
        assert_eq!(final_revision.final_audio_proc_ms, Some(1040));

        let ledger = TranscriptLedger::replay(
            "session-1",
            [
                TranscriptEvent::from(partial_revision.payload.clone()),
                TranscriptEvent::from(final_revision.payload.clone()),
            ],
        )
        .unwrap();

        assert_eq!(ledger.accepted_event_count, 2);
        assert_eq!(ledger.latest_spans.len(), 1);
        assert_eq!(ledger.latest_spans[0].span_id, span_id);
        assert_eq!(ledger.latest_spans[0].revision_number, 2);
        assert!(ledger.latest_spans[0].is_final);
    }

    #[test]
    fn mixed_final_and_non_final_tokens_revise_one_active_turn() {
        let mut parser = SonioxRealtimeParser::new("system");

        let first = parser
            .parse_message(
                r#"{
                    "tokens": [
                        { "text": "How", "start_ms": 100, "end_ms": 240, "confidence": 0.72, "is_final": false },
                        { "text": "'re", "start_ms": 240, "end_ms": 360, "confidence": 0.65, "is_final": false }
                    ],
                    "total_audio_proc_ms": 380
                }"#,
                1_700_000_000_010,
            )
            .unwrap();
        let second = parser
            .parse_message(
                r#"{
                    "tokens": [
                        { "text": "How", "start_ms": 100, "end_ms": 240, "confidence": 0.95, "is_final": true },
                        { "text": " ", "start_ms": 240, "end_ms": 260, "confidence": 0.95, "is_final": true },
                        { "text": "are", "start_ms": 260, "end_ms": 440, "confidence": 0.85, "is_final": false }
                    ],
                    "final_audio_proc_ms": 260,
                    "total_audio_proc_ms": 460
                }"#,
                1_700_000_000_020,
            )
            .unwrap();
        let third = parser
            .parse_message(
                r#"{
                    "tokens": [
                        { "text": "are", "start_ms": 260, "end_ms": 440, "confidence": 0.93, "is_final": true },
                        { "text": " ", "start_ms": 440, "end_ms": 460, "confidence": 0.94, "is_final": true },
                        { "text": "you", "start_ms": 460, "end_ms": 640, "confidence": 0.86, "is_final": false }
                    ],
                    "final_audio_proc_ms": 460,
                    "total_audio_proc_ms": 660
                }"#,
                1_700_000_000_030,
            )
            .unwrap();
        let endpoint = parser
            .parse_message(
                r#"{
                    "tokens": [
                        { "text": "you", "start_ms": 460, "end_ms": 640, "confidence": 0.97, "is_final": true },
                        { "text": "?<end>", "start_ms": 640, "end_ms": 660, "confidence": 0.96, "is_final": true }
                    ],
                    "final_audio_proc_ms": 660,
                    "total_audio_proc_ms": 660
                }"#,
                1_700_000_000_040,
            )
            .unwrap();

        assert_eq!(first.revisions[0].payload.text, "How're");
        assert_eq!(first.revisions[0].payload.revision_number, 1);
        assert_eq!(second.revisions[0].payload.text, "How are");
        assert_eq!(second.revisions[0].payload.revision_number, 2);
        assert_eq!(third.revisions[0].payload.text, "How are you");
        assert_eq!(third.revisions[0].payload.revision_number, 3);
        assert_eq!(endpoint.revisions[0].payload.text, "How are you?");
        assert_eq!(endpoint.revisions[0].payload.revision_number, 4);
        assert!(endpoint.revisions[0].payload.is_final);
        assert_eq!(
            endpoint.revisions[0].payload.supersedes.as_deref(),
            Some("soniox:system:turn-1@rev3")
        );
    }

    #[test]
    fn starts_new_turn_after_endpoint_marker() {
        let mut parser = SonioxRealtimeParser::new("system");
        let parsed = parser
            .parse_message(
                r#"{
                    "tokens": [
                        { "text": "done", "start_ms": 100, "end_ms": 280, "confidence": 0.91, "is_final": true, "speaker": "1", "language": "en" },
                        { "text": "<end>", "is_final": true },
                        { "text": "next", "start_ms": 420, "end_ms": 600, "confidence": 0.81, "is_final": false, "speaker": "2", "language": "en" }
                    ]
                }"#,
                1_700_000_000_050,
            )
            .unwrap();

        assert_eq!(parsed.revisions.len(), 2);
        assert_eq!(parsed.revisions[0].payload.text, "done");
        assert!(parsed.revisions[0].payload.is_final);
        assert_eq!(parsed.revisions[0].payload.span_id, "soniox:system:turn-1");
        assert_eq!(parsed.revisions[1].payload.text, "next");
        assert!(!parsed.revisions[1].payload.is_final);
        assert_eq!(parsed.revisions[1].payload.span_id, "soniox:system:turn-2");
        assert_eq!(parsed.revisions[1].payload.speaker_id.as_deref(), Some("2"));
    }

    #[test]
    fn reconnect_finalize_supersedes_orphaned_partial() {
        // A partial was emitted downstream (is_final=false, rev 1) for turn-1.
        let mut parser = SonioxRealtimeParser::new("mic-1");
        let partial = parser
            .parse_message(
                r#"{
                    "tokens": [
                        { "text": "hello", "start_ms": 100, "end_ms": 300, "confidence": 0.9, "is_final": false, "speaker": "1", "language": "en" }
                    ],
                    "total_audio_proc_ms": 320
                }"#,
                1_700_000_000_200,
            )
            .unwrap();
        assert_eq!(partial.revisions.len(), 1);
        assert!(!partial.revisions[0].payload.is_final);
        let span_id = partial.revisions[0].payload.span_id.clone();
        assert_eq!(span_id, "soniox:mic-1:turn-1");

        // A reconnect finalizes that turn: emit a terminal revision that
        // supersedes the partial so the ledger doesn't strand it (M3).
        let finalize = parser
            .finalize_active_turn_for_reconnect(1_700_000_000_300)
            .expect("active turn with emitted text finalizes on reconnect");

        assert_eq!(finalize.payload.span_id, span_id);
        assert!(finalize.payload.is_final);
        assert!(finalize.payload.end_of_turn);
        assert_eq!(finalize.payload.revision_number, 2);
        assert_eq!(
            finalize.payload.supersedes.as_deref(),
            Some("soniox:mic-1:turn-1@rev1")
        );
        assert_eq!(finalize.payload.text, "hello");

        // A subsequent finalize is a no-op — the active turn is cleared, and a
        // fresh turn resumes under a new namespace.
        assert!(
            parser
                .finalize_active_turn_for_reconnect(1_700_000_000_400)
                .is_none()
        );

        // Replaying [partial, finalize] through the ledger leaves exactly one
        // span, now final — no orphaned never-finalized partial.
        let ledger = TranscriptLedger::replay(
            "session-reconnect",
            [
                TranscriptEvent::from(partial.revisions[0].payload.clone()),
                TranscriptEvent::from(finalize.payload.clone()),
            ],
        )
        .unwrap();
        assert_eq!(ledger.latest_spans.len(), 1);
        assert_eq!(ledger.latest_spans[0].span_id, span_id);
        assert!(ledger.latest_spans[0].is_final);
        assert_eq!(ledger.latest_spans[0].revision_number, 2);
    }

    #[test]
    fn reconnect_finalize_without_active_turn_is_noop() {
        let mut parser = SonioxRealtimeParser::new("mic-1");
        assert!(
            parser
                .finalize_active_turn_for_reconnect(1_700_000_000_500)
                .is_none()
        );
    }

    #[test]
    fn mixed_speaker_tokens_leave_span_speaker_unset() {
        let mut parser = SonioxRealtimeParser::new("mic-1");
        let parsed = parser
            .parse_message(
                r#"{
                    "tokens": [
                        { "text": "hello", "start_ms": 100, "end_ms": 300, "confidence": 0.91, "is_final": false, "speaker": "1" },
                        { "text": " there", "start_ms": 300, "end_ms": 520, "confidence": 0.9, "is_final": false, "speaker": "2" }
                    ]
                }"#,
                1_700_000_000_060,
            )
            .unwrap();

        assert_eq!(parsed.revisions.len(), 1);
        assert_eq!(parsed.revisions[0].payload.speaker_id, None);
        assert_eq!(parsed.revisions[0].payload.speaker_label, None);
    }

    #[test]
    fn finished_response_closes_active_turn() {
        let mut parser = SonioxRealtimeParser::new("mic-1");
        parser
            .parse_message(
                r#"{
                    "tokens": [
                        { "text": "bye", "start_ms": 100, "end_ms": 260, "confidence": 0.88, "is_final": false }
                    ]
                }"#,
                1_700_000_000_070,
            )
            .unwrap();

        let finished = parser
            .parse_message(
                r#"{
                    "tokens": [],
                    "final_audio_proc_ms": 300,
                    "total_audio_proc_ms": 320,
                    "finished": true
                }"#,
                1_700_000_000_080,
            )
            .unwrap();

        assert!(finished.finished);
        assert_eq!(finished.revisions.len(), 1);
        assert_eq!(finished.revisions[0].payload.text, "bye");
        assert!(finished.revisions[0].payload.is_final);
        assert!(finished.revisions[0].payload.end_of_turn);
    }

    #[test]
    fn preserves_provider_error_without_emitting_empty_transcript() {
        let mut parser = SonioxRealtimeParser::new("mic-1");
        let parsed = parser
            .parse_message(
                r#"{
                    "tokens": [],
                    "error_code": 503,
                    "error_type": "service_unavailable",
                    "error_message": "Cannot continue request",
                    "request_id": "req-123",
                    "more_info": "https://soniox.com/docs/api-reference/errors#service-unavailable"
                }"#,
                1_700_000_000_090,
            )
            .unwrap();

        assert!(parsed.revisions.is_empty());
        assert_eq!(
            parsed.error,
            Some(SonioxProviderError {
                code: Some("503".to_string()),
                error_type: Some("service_unavailable".to_string()),
                message: "Cannot continue request".to_string(),
                request_id: Some("req-123".to_string()),
                more_info: Some(
                    "https://soniox.com/docs/api-reference/errors#service-unavailable".to_string()
                ),
            })
        );
    }

    #[test]
    fn empty_tokens_without_finished_are_ignored() {
        let mut parser = SonioxRealtimeParser::new("mic-1");
        let parsed = parser
            .parse_message(r#"{ "tokens": [] }"#, 1_700_000_000_100)
            .unwrap();

        assert!(parsed.revisions.is_empty());
        assert!(!parsed.finished);
    }

    #[test]
    fn invalid_json_is_reported_as_parse_error() {
        let mut parser = SonioxRealtimeParser::new("mic-1");
        let error = parser.parse_message("{", 1).unwrap_err();
        assert!(matches!(error, SonioxParseError::InvalidJson(_)));
    }
}
