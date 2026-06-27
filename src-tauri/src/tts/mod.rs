//! Text-to-Speech (TTS) provider trait and shared types.
//!
//! See ADR-0004 (`docs/adr/0004-tts-provider-trait-and-deepgram-aura.md`)
//! for the architectural rationale. This module defines the cross-provider
//! surface; concrete implementations (`tts/deepgram_aura.rs` for now, later
//! `tts/kokoro.rs`, `tts/piper.rs`, etc.) live in sibling files so that one
//! provider's churn never touches another's tests.
//!
//! # Trait shape
//!
//! - [`TtsProvider`] is the entry point. Call [`TtsProvider::open`] to
//!   establish a streaming session against the cloud (or load a local model
//!   into memory for offline engines). Returns a boxed [`TtsSession`].
//! - [`TtsSession`] is the live handle. Producers call `speak(text)` /
//!   `flush()` / `clear()` / `close()` on it; consumers drain
//!   [`TtsEvent`]s from `events()`.
//! - [`TtsEvent`] is the normalized event stream. `AudioChunk { samples,
//!   sample_rate }` carries 16-bit mono PCM ready for playback. `Status`
//!   and `Error` are control-plane signals.
//!
//! # Why async-trait?
//!
//! `open()` performs network I/O for cloud providers, so it must be `async`.
//! Today's stable Rust still requires `#[async_trait]` for object-safe async
//! trait methods; the runtime cost (one box-allocation per call) is
//! negligible against the WebSocket handshake cost it accompanies.
//!
//! # Why split `TtsProvider` from `TtsSession`?
//!
//! A provider may be cheap to construct but each session is stateful (sample
//! rate, voice, in-flight Speak frames). Mirroring `asr/` we keep the
//! provider as a credentials/config holder and the session as the ephemeral
//! object users operate on. Tests construct sessions directly without going
//! through `open()`; production code always uses `open()`.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::pin::Pin;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Encoding format requested from the TTS server / produced by the engine.
///
/// Only the streaming-compatible set per ADR-0004 is exposed here. REST-only
/// formats (mp3, opus, flac, aac) are intentionally absent — they don't fit
/// the streaming AudioChunk model and would invite confused configurations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum TtsEncoding {
    /// 16-bit signed little-endian PCM. Default and the only encoding the
    /// audio playback subsystem (Wave B) understands without conversion.
    #[default]
    Linear16,
    /// G.711 µ-law (8-bit, 8 kHz canonical). Accepted by Aura but downstream
    /// playback would need a decoder; included for completeness.
    Mulaw,
    /// G.711 A-law. Same caveat as `Mulaw`.
    Alaw,
}

impl TtsEncoding {
    /// Wire-format name expected by the Deepgram Aura `?encoding=` query
    /// parameter. Stable; do not change without coordinating with the server.
    pub fn wire_name(&self) -> &'static str {
        match self {
            TtsEncoding::Linear16 => "linear16",
            TtsEncoding::Mulaw => "mulaw",
            TtsEncoding::Alaw => "alaw",
        }
    }
}

/// Configuration for a single TTS session.
///
/// `voice` is provider-specific (e.g. `aura-asteria-en` for Aura,
/// `en_US-amy-medium` for Piper). The trait does not enumerate voices —
/// that's an out-of-scope problem for v1, see plan A1.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    /// Provider-specific voice identifier.
    pub voice: String,
    /// PCM sample rate in Hz. Aura streaming default is 24000.
    pub sample_rate: u32,
    /// Wire encoding. Default `Linear16`.
    #[serde(default)]
    pub encoding: TtsEncoding,
    /// Speech rate multiplier. Aura accepts 0.7..=1.5 (default 1.0). Values
    /// outside that range are clamped at the call site.
    #[serde(default = "default_speed")]
    pub speed: f32,
}

fn default_speed() -> f32 {
    1.0
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            voice: "aura-asteria-en".to_string(),
            sample_rate: 24_000,
            encoding: TtsEncoding::Linear16,
            speed: 1.0,
        }
    }
}

impl TtsConfig {
    /// Clamp `speed` to the provider-supported range (0.7..=1.5 per Aura
    /// docs). Returns a copy so callers can use it as a builder.
    pub fn with_clamped_speed(mut self) -> Self {
        if !self.speed.is_finite() {
            self.speed = 1.0;
        }
        self.speed = self.speed.clamp(0.7, 1.5);
        self
    }
}

/// A single normalized event emitted by a [`TtsSession`].
///
/// The `serde(tag = "type")` shape mirrors the existing ASR event shape
/// (see `crate::asr::deepgram::DeepgramEvent`) so the frontend doesn't have
/// to learn a different discriminator convention for TTS.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TtsEvent {
    /// A buffer of i16 PCM samples ready for playback at `sample_rate` Hz.
    /// Empty buffers are not emitted — consumers can rely on `samples.len()
    /// > 0`.
    #[serde(rename = "audio_chunk")]
    AudioChunk { samples: Vec<i16>, sample_rate: u32 },
    /// A control-plane status update (lifecycle, server acks).
    #[serde(rename = "status")]
    Status(TtsStatus),
    /// A non-fatal or fatal error. Producers should still drain remaining
    /// frames after emitting an error; the session task decides whether to
    /// reconnect.
    #[serde(rename = "error")]
    Error { kind: TtsErrorKind, message: String },
}

/// Lifecycle / acknowledgement signals from the provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum TtsStatus {
    /// Initial WebSocket connect succeeded.
    #[serde(rename = "connected")]
    Connected,
    /// Server acknowledged a `Flush` frame. `sequence` is a monotonic counter
    /// the client increments per Flush — useful for matching client-side
    /// barge-in actions to server-side flush points.
    #[serde(rename = "flushed")]
    Flushed { sequence: u64 },
    /// Server acknowledged a `Clear` frame. Consumers should drop any
    /// `AudioChunk` events they buffered between sending Clear and seeing
    /// this ack — those frames belong to the cancelled utterance.
    #[serde(rename = "cleared")]
    Cleared,
    /// Server-issued metadata summary. Aura emits `Metadata` frames with
    /// request/model identifiers; expose only bounded fields instead of the raw
    /// provider JSON frame.
    #[serde(rename = "metadata")]
    Metadata {
        request_id: Option<String>,
        model: Option<String>,
        field_count: usize,
    },
    /// WebSocket closed, either by the user or because the server hung up.
    #[serde(rename = "disconnected")]
    Disconnected,
    /// Client detected a disconnect and is attempting to reconnect with
    /// exponential backoff. Emitted at the start of each retry.
    #[serde(rename = "reconnecting")]
    Reconnecting { attempt: u32, backoff_secs: u64 },
    /// Successful re-establishment after a disconnect.
    #[serde(rename = "reconnected")]
    Reconnected,
}

/// Coarse error classification used in [`TtsEvent::Error`] and
/// [`TtsError`] alike. Frontends key UI behaviour off these — e.g.
/// `Auth` triggers a "go to settings" CTA, `RateLimit` triggers a
/// quieter retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TtsErrorKind {
    /// Bad / missing API key. (HTTP 401, 403.)
    Auth,
    /// HTTP 429 / quota exhaustion.
    RateLimit,
    /// HTTP 4xx (other) — request shape was wrong.
    BadRequest,
    /// HTTP 5xx — provider-side fault.
    Server,
    /// TLS, TCP, DNS, or socket-layer error.
    Network,
    /// Server returned malformed JSON / unexpected frame.
    Protocol,
    /// Reconnect ladder exhausted.
    Exhausted,
    /// Catch-all for not-yet-classified errors.
    Unknown,
}

/// Error returned by trait methods.
///
/// Variants closely match [`TtsErrorKind`] so callers can switch on the kind
/// without downcasting. `String` payloads carry the human-readable detail.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
pub enum TtsError {
    Auth(String),
    RateLimit(String),
    BadRequest(String),
    Server(String),
    Network(String),
    Protocol(String),
    Exhausted(String),
    Unknown(String),
}

/// Metadata-only detail for HTTP failures returned by TTS providers.
///
/// Do not put raw provider response bodies or URL query strings here. The
/// rendered error string is UI-visible in some call paths.
#[derive(Debug, Clone)]
pub(crate) struct TtsHttpErrorDiagnostic {
    provider: String,
    service: String,
    path: String,
    request_id: Option<String>,
    body_bytes: usize,
    body_chars: usize,
}

impl TtsHttpErrorDiagnostic {
    pub(crate) fn new(
        provider: impl Into<String>,
        service: impl Into<String>,
        path: impl Into<String>,
    ) -> Self {
        Self {
            provider: provider.into(),
            service: service.into(),
            path: path.into(),
            request_id: None,
            body_bytes: 0,
            body_chars: 0,
        }
    }

    pub(crate) fn with_request_id(mut self, request_id: Option<String>) -> Self {
        self.request_id = request_id;
        self
    }

    pub(crate) fn with_body_str(mut self, body: &str) -> Self {
        self.body_bytes = body.len();
        self.body_chars = body.chars().count();
        self
    }

    pub(crate) fn with_body_bytes(mut self, body: &[u8]) -> Self {
        self.body_bytes = body.len();
        self.body_chars = String::from_utf8_lossy(body).chars().count();
        self
    }
}

pub(crate) fn tts_http_diagnostic_path(url: &str) -> String {
    if let Ok(parsed) = url::Url::parse(url) {
        let path = parsed.path();
        return if path.is_empty() { "/" } else { path }.to_string();
    }

    if url.starts_with('/') {
        return url
            .split(['?', '#'])
            .next()
            .filter(|path| !path.is_empty())
            .unwrap_or("/")
            .to_string();
    }

    "<unparseable>".to_string()
}

impl TtsError {
    /// Map this error to its [`TtsErrorKind`] discriminator.
    pub fn kind(&self) -> TtsErrorKind {
        match self {
            TtsError::Auth(_) => TtsErrorKind::Auth,
            TtsError::RateLimit(_) => TtsErrorKind::RateLimit,
            TtsError::BadRequest(_) => TtsErrorKind::BadRequest,
            TtsError::Server(_) => TtsErrorKind::Server,
            TtsError::Network(_) => TtsErrorKind::Network,
            TtsError::Protocol(_) => TtsErrorKind::Protocol,
            TtsError::Exhausted(_) => TtsErrorKind::Exhausted,
            TtsError::Unknown(_) => TtsErrorKind::Unknown,
        }
    }

    /// Borrow the human-readable message.
    pub fn message(&self) -> &str {
        match self {
            TtsError::Auth(m)
            | TtsError::RateLimit(m)
            | TtsError::BadRequest(m)
            | TtsError::Server(m)
            | TtsError::Network(m)
            | TtsError::Protocol(m)
            | TtsError::Exhausted(m)
            | TtsError::Unknown(m) => m,
        }
    }

    /// Build a [`TtsError`] from an HTTP status code returned during the
    /// initial handshake. Used by every provider impl, hence the shared spot.
    pub fn from_http_status(status: u16, body: &str) -> Self {
        Self::from_http_status_diagnostic(
            status,
            TtsHttpErrorDiagnostic::new("tts", "unknown", "<unknown>").with_body_str(body),
        )
    }

    /// Compatibility wrapper for older call sites. The body is measured for
    /// byte/char counts only; no response text is echoed.
    pub fn from_http_status_redacted<I, S>(status: u16, body: &str, _secrets: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::from_http_status(status, body)
    }

    pub(crate) fn from_http_status_diagnostic(
        status: u16,
        diagnostic: TtsHttpErrorDiagnostic,
    ) -> Self {
        let request_id = diagnostic
            .request_id
            .as_deref()
            .map(|id| format!(" request_id={id}"))
            .unwrap_or_default();
        let msg = format!(
            "HTTP status={status} provider={} service={} path={}{} body_bytes={} body_chars={}",
            diagnostic.provider,
            diagnostic.service,
            diagnostic.path,
            request_id,
            diagnostic.body_bytes,
            diagnostic.body_chars
        );
        match status {
            401 | 403 => TtsError::Auth(msg),
            429 => TtsError::RateLimit(msg),
            400..=499 => TtsError::BadRequest(msg),
            500..=599 => TtsError::Server(msg),
            _ => TtsError::Unknown(msg),
        }
    }
}

impl std::fmt::Display for TtsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind(), self.message())
    }
}

impl std::error::Error for TtsError {}

// ---------------------------------------------------------------------------
// Trait
// ---------------------------------------------------------------------------

/// Stream of [`TtsEvent`]s exposed by [`TtsSession::take_events`].
///
/// Boxed + pinned so the trait remains object-safe across providers that may
/// use very different internal stream types (tokio mpsc, futures channel,
/// in-memory iterator, etc.). The `Send` bound mirrors `Stream` consumers in
/// the rest of the codebase that move the receiver across threads.
pub type TtsEventStream = Pin<Box<dyn futures_util::Stream<Item = TtsEvent> + Send>>;

/// A live TTS session. Drop the value to terminate the session.
///
/// Methods are sync because the underlying transports send on
/// `tokio::sync::mpsc::UnboundedSender` (lock-free, never blocks) — the
/// async happens off-thread in the session task. This matches the
/// ergonomics of `crate::asr::deepgram::DeepgramStreamingClient::send_audio`.
#[async_trait]
pub trait TtsSession: Send {
    /// Queue text for synthesis. The text is sent verbatim — line splitting,
    /// punctuation insertion, SSML handling, etc. are caller responsibilities.
    fn speak(&self, text: &str) -> Result<(), TtsError>;
    /// Tell the server "I'm done with this utterance, render anything
    /// buffered and emit a [`TtsStatus::Flushed`] ack". Optional — long-form
    /// streamed text doesn't need it; barge-in / turn boundaries do.
    fn flush(&self) -> Result<(), TtsError>;
    /// Cancel the in-flight utterance. Audio frames received from the server
    /// after this point and before the [`TtsStatus::Cleared`] ack belong to
    /// the cancelled utterance and should be dropped by the consumer.
    fn clear(&self) -> Result<(), TtsError>;
    /// Send a Close frame and tear down the session. Subsequent calls are
    /// no-ops; the session object should be dropped after this.
    fn close(&self) -> Result<(), TtsError>;
    /// Take ownership of the event stream. Calling this more than once on
    /// the same session returns `None` for subsequent calls — there is only
    /// one stream per session.
    fn take_events(&mut self) -> Option<TtsEventStream>;
}

/// Top-level provider trait. Implementations live in sibling modules
/// (`deepgram_aura.rs`, future: `kokoro.rs`, etc.).
#[async_trait]
pub trait TtsProvider: Send + Sync {
    /// Open a fresh streaming session.
    ///
    /// Cloud providers perform a WebSocket handshake here. Local providers
    /// (later) load model weights or warm up an inference engine.
    async fn open(&self, voice: &str, config: TtsConfig) -> Result<Box<dyn TtsSession>, TtsError>;
}

// ---------------------------------------------------------------------------
// Concrete provider modules
// ---------------------------------------------------------------------------

pub mod deepgram_aura;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn config_default_matches_aura_streaming_spec() {
        let cfg = TtsConfig::default();
        assert_eq!(cfg.voice, "aura-asteria-en");
        assert_eq!(cfg.sample_rate, 24_000);
        assert_eq!(cfg.encoding, TtsEncoding::Linear16);
        assert!((cfg.speed - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn encoding_wire_name_matches_aura_query_param() {
        assert_eq!(TtsEncoding::Linear16.wire_name(), "linear16");
        assert_eq!(TtsEncoding::Mulaw.wire_name(), "mulaw");
        assert_eq!(TtsEncoding::Alaw.wire_name(), "alaw");
    }

    #[test]
    fn speed_clamp_keeps_in_supported_range() {
        let too_slow = TtsConfig {
            speed: 0.1,
            ..TtsConfig::default()
        }
        .with_clamped_speed();
        assert!((too_slow.speed - 0.7).abs() < f32::EPSILON);

        let too_fast = TtsConfig {
            speed: 9.0,
            ..TtsConfig::default()
        }
        .with_clamped_speed();
        assert!((too_fast.speed - 1.5).abs() < f32::EPSILON);

        let nan = TtsConfig {
            speed: f32::NAN,
            ..TtsConfig::default()
        }
        .with_clamped_speed();
        assert!((nan.speed - 1.0).abs() < f32::EPSILON);

        let in_range = TtsConfig {
            speed: 1.2,
            ..TtsConfig::default()
        }
        .with_clamped_speed();
        assert!((in_range.speed - 1.2).abs() < f32::EPSILON);
    }

    #[test]
    fn error_classification_maps_http_status_to_kind() {
        assert!(matches!(
            TtsError::from_http_status(401, "bad key"),
            TtsError::Auth(_)
        ));
        assert!(matches!(
            TtsError::from_http_status(403, "forbidden"),
            TtsError::Auth(_)
        ));
        assert!(matches!(
            TtsError::from_http_status(429, "slow down"),
            TtsError::RateLimit(_)
        ));
        assert!(matches!(
            TtsError::from_http_status(400, "bad json"),
            TtsError::BadRequest(_)
        ));
        assert!(matches!(
            TtsError::from_http_status(500, "oops"),
            TtsError::Server(_)
        ));
        assert!(matches!(
            TtsError::from_http_status(502, "bad gateway"),
            TtsError::Server(_)
        ));
    }

    #[test]
    fn http_status_reports_metadata_without_body_excerpt() {
        let body = r#"{"error":"provider body text","text":"generated speech text","api_key":"tts-secret-12345"}"#;
        let err = TtsError::from_http_status_redacted(401, body, ["tts-secret-12345"]);
        let msg = err.message();

        assert!(msg.contains("status=401"));
        assert!(msg.contains("provider=tts"));
        assert!(msg.contains("service=unknown"));
        assert!(msg.contains("path=<unknown>"));
        assert!(msg.contains(&format!("body_bytes={}", body.len())));
        assert!(msg.contains(&format!("body_chars={}", body.chars().count())));
        assert!(!msg.contains("provider body text"));
        assert!(!msg.contains("generated speech text"));
        assert!(!msg.contains("tts-secret-12345"));
        assert!(matches!(err, TtsError::Auth(_)));
    }

    #[test]
    fn http_status_diagnostic_includes_path_only_and_request_id() {
        let body = r#"{"error":"provider body text","text":"generated speech text","api_key":"tts-secret-12345"}"#;
        let err = TtsError::from_http_status_diagnostic(
            429,
            TtsHttpErrorDiagnostic::new(
                "deepgram",
                "aura",
                tts_http_diagnostic_path(
                    "wss://api.deepgram.com/v1/speak?api_key=tts-secret-12345&model=aura",
                ),
            )
            .with_request_id(Some("dg-req_123".to_string()))
            .with_body_str(body),
        );
        let msg = err.message();

        assert!(msg.contains("status=429"));
        assert!(msg.contains("provider=deepgram"));
        assert!(msg.contains("service=aura"));
        assert!(msg.contains("path=/v1/speak"));
        assert!(msg.contains("request_id=dg-req_123"));
        assert!(msg.contains(&format!("body_bytes={}", body.len())));
        assert!(msg.contains(&format!("body_chars={}", body.chars().count())));
        assert!(!msg.contains("api_key="));
        assert!(!msg.contains("model=aura"));
        assert!(!msg.contains("provider body text"));
        assert!(!msg.contains("generated speech text"));
        assert!(!msg.contains("tts-secret-12345"));
        assert!(matches!(err, TtsError::RateLimit(_)));
    }

    #[test]
    fn event_serialization_roundtrips() {
        let events = vec![
            TtsEvent::AudioChunk {
                samples: vec![1, -1, 2, -2],
                sample_rate: 24_000,
            },
            TtsEvent::Status(TtsStatus::Connected),
            TtsEvent::Status(TtsStatus::Flushed { sequence: 7 }),
            TtsEvent::Status(TtsStatus::Cleared),
            TtsEvent::Status(TtsStatus::Metadata {
                request_id: Some("abc".into()),
                model: Some("aura-asteria-en".into()),
                field_count: 2,
            }),
            TtsEvent::Status(TtsStatus::Disconnected),
            TtsEvent::Status(TtsStatus::Reconnecting {
                attempt: 2,
                backoff_secs: 2,
            }),
            TtsEvent::Status(TtsStatus::Reconnected),
            TtsEvent::Error {
                kind: TtsErrorKind::Auth,
                message: "bad token".into(),
            },
        ];

        for event in &events {
            let json = serde_json::to_string(event).expect("serialize");
            let _back: Value = serde_json::from_str(&json).expect("parse");
        }
    }
}
