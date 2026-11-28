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
use audio_graph_ipc_contract::runtime_diagnostic::{
    RuntimeDiagnostic, RuntimeDiagnosticContext, RuntimeErrorCode, RuntimeErrorDiagnostic,
    RuntimeOperation, RuntimeSafeRecoveryAction, RuntimeTransport,
};
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
#[derive(Clone, Serialize, Deserialize)]
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
    Error {
        kind: TtsErrorKind,
        diagnostic: RuntimeDiagnostic,
    },
}

impl std::fmt::Debug for TtsEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AudioChunk { sample_rate, .. } => f
                .debug_struct("AudioChunk")
                .field("sample_rate", sample_rate)
                .finish_non_exhaustive(),
            Self::Status(status) => f.debug_tuple("Status").field(status).finish(),
            Self::Error { kind, diagnostic } => f
                .debug_struct("Error")
                .field("kind", kind)
                .field("diagnostic", diagnostic)
                .finish(),
        }
    }
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
    /// Server metadata arrived. Provider fields remain private.
    #[serde(rename = "metadata")]
    Metadata,
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

/// Content-free diagnostic returned by trait methods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsError {
    kind: TtsErrorKind,
    diagnostic: RuntimeDiagnostic,
}

impl TtsError {
    pub fn kind(&self) -> TtsErrorKind {
        self.kind
    }

    pub fn diagnostic(&self) -> &RuntimeDiagnostic {
        &self.diagnostic
    }

    pub(crate) fn new(kind: TtsErrorKind, diagnostic: impl Into<RuntimeDiagnostic>) -> Self {
        Self {
            kind,
            diagnostic: diagnostic.into(),
        }
    }

    pub(crate) fn runtime(
        kind: TtsErrorKind,
        code: RuntimeErrorCode,
        context: RuntimeDiagnosticContext,
    ) -> Self {
        use RuntimeErrorCode as Code;
        use RuntimeSafeRecoveryAction as Action;

        let (retryable, action) = match code {
            Code::CredentialRejected => {
                return Self::new(kind, RuntimeErrorDiagnostic::credential_rejected(context));
            }
            Code::PermissionDenied => {
                return Self::new(kind, RuntimeErrorDiagnostic::permission_denied(context));
            }
            Code::RateLimited => (true, Action::RetryAfterDelay),
            Code::NetworkUnreachable => (true, Action::CheckNetwork),
            Code::Timeout | Code::ProviderUnavailable | Code::ProtocolError => {
                (true, Action::Retry)
            }
            Code::PolicyBlocked => (false, Action::ReviewPolicy),
            Code::RequestRejected => (false, Action::ReviewConfiguration),
            _ => (false, Action::None),
        };
        Self::new(
            kind,
            RuntimeErrorDiagnostic::new(code, retryable, action, context),
        )
    }

    /// Classify a synthesis HTTP failure using only its status, including
    /// failures returned by a WebSocket upgrade.
    pub fn from_http_status(status: u16, credential_sent: bool) -> Self {
        let context = RuntimeDiagnosticContext::new(RuntimeOperation::SpeechSynthesis)
            .with_transport(RuntimeTransport::Http);
        let kind = match status {
            401 | 403 => TtsErrorKind::Auth,
            429 => TtsErrorKind::RateLimit,
            400..=499 => TtsErrorKind::BadRequest,
            500..=599 => TtsErrorKind::Server,
            _ => TtsErrorKind::Unknown,
        };
        Self::new(
            kind,
            RuntimeErrorDiagnostic::from_http_status(status, credential_sent, context),
        )
    }
}

impl From<TtsError> for TtsEvent {
    fn from(error: TtsError) -> Self {
        Self::Error {
            kind: error.kind,
            diagnostic: error.diagnostic,
        }
    }
}

impl std::fmt::Display for TtsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.diagnostic.fmt(f)
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
pub(crate) mod test_support {
    use crate::test_log_capture::ThreadLogCapture;
    use std::cell::RefCell;

    thread_local! {
        static TTS_LOGS: RefCell<Option<ThreadLogCapture>> = const { RefCell::new(None) };
    }

    /// Test tasks that produce logs must run on the capturing thread.
    pub(crate) fn capture_tts_logs() {
        TTS_LOGS.with_borrow_mut(|capture| {
            *capture = None;
            *capture = Some(ThreadLogCapture::start());
        });
    }

    pub(crate) fn take_tts_logs() -> Vec<String> {
        TTS_LOGS
            .with_borrow_mut(|capture| capture.take().expect("start TTS log capture"))
            .finish()
            .into_iter()
            .filter(|record| {
                record
                    .target
                    .starts_with(module_path!().trim_end_matches("::test_support"))
                    || record.target.ends_with("::speak_aloud")
            })
            .map(|record| record.message)
            .collect()
    }

    pub(crate) fn assert_no_canaries(output: &str, canaries: &[&str]) {
        for canary in canaries {
            assert!(!output.contains(canary), "TTS diagnostic leaked {canary:?}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn http_status_classification_exports_only_closed_context() {
        use RuntimeErrorCode as Code;
        use RuntimeSafeRecoveryAction as Action;
        use audio_graph_ipc_contract::runtime_diagnostic::RuntimeStatusClass as Status;

        for (status, credential_sent, kind, code, retryable, action, status_class) in [
            (
                307,
                true,
                TtsErrorKind::Unknown,
                Code::RequestRejected,
                false,
                Action::ReviewConfiguration,
                Some(Status::Redirect),
            ),
            (
                401,
                true,
                TtsErrorKind::Auth,
                Code::CredentialRejected,
                false,
                Action::ReenterCredential,
                Some(Status::ClientError),
            ),
            (
                401,
                false,
                TtsErrorKind::Auth,
                Code::RequestRejected,
                false,
                Action::ReviewConfiguration,
                Some(Status::ClientError),
            ),
            (
                403,
                true,
                TtsErrorKind::Auth,
                Code::PermissionDenied,
                false,
                Action::ReviewConfiguration,
                Some(Status::ClientError),
            ),
            (
                408,
                true,
                TtsErrorKind::BadRequest,
                Code::Timeout,
                true,
                Action::Retry,
                Some(Status::ClientError),
            ),
            (
                429,
                true,
                TtsErrorKind::RateLimit,
                Code::RateLimited,
                true,
                Action::RetryAfterDelay,
                Some(Status::ClientError),
            ),
            (
                422,
                true,
                TtsErrorKind::BadRequest,
                Code::RequestRejected,
                false,
                Action::ReviewConfiguration,
                Some(Status::ClientError),
            ),
            (
                503,
                true,
                TtsErrorKind::Server,
                Code::ProviderUnavailable,
                true,
                Action::Retry,
                Some(Status::ServerError),
            ),
            (
                504,
                true,
                TtsErrorKind::Server,
                Code::ProviderUnavailable,
                true,
                Action::Retry,
                Some(Status::ServerError),
            ),
            (
                700,
                true,
                TtsErrorKind::Unknown,
                Code::Internal,
                false,
                Action::None,
                None,
            ),
        ] {
            let error = TtsError::from_http_status(status, credential_sent);
            assert_eq!(error.kind(), kind);
            let RuntimeDiagnostic::Runtime(diagnostic) = error.diagnostic() else {
                panic!("expected a runtime diagnostic");
            };
            assert_eq!(diagnostic.code, code);
            assert_eq!(diagnostic.retryable, retryable);
            assert_eq!(diagnostic.recovery_action, action);
            assert_eq!(
                diagnostic.context.operation,
                RuntimeOperation::SpeechSynthesis
            );
            assert_eq!(diagnostic.context.transport, Some(RuntimeTransport::Http));
            assert_eq!(diagnostic.context.status_class, status_class);
            let serialized = serde_json::to_string(&error).unwrap();
            assert!(!serialized.contains(&status.to_string()));
        }
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
            TtsEvent::Status(TtsStatus::Metadata),
            TtsEvent::Status(TtsStatus::Disconnected),
            TtsEvent::Status(TtsStatus::Reconnecting {
                attempt: 2,
                backoff_secs: 2,
            }),
            TtsEvent::Status(TtsStatus::Reconnected),
            TtsError::from_http_status(403, true).into(),
        ];

        for event in &events {
            let json = serde_json::to_string(event).expect("serialize");
            let back: TtsEvent = serde_json::from_str(&json).expect("parse");
            assert_eq!(serde_json::to_string(&back).unwrap(), json);
        }
    }
}
