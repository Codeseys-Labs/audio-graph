//! Shared production WebSocket write guards for streaming ASR providers.
//!
//! Providers still own URL construction, handshake payload shape, parsers,
//! reconnect policy, and terminal semantics. This module centralizes the
//! content-egress check at the write primitive so provider session tasks cannot
//! accidentally bypass the runtime privacy guard.

use std::fmt;

use futures_util::SinkExt;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream,
    tungstenite::{self, Message},
};

use super::ProviderContentEgressPolicy;

pub(super) type AsrWsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;
pub(super) type AsrWsWriter = futures_util::stream::SplitSink<AsrWsStream, Message>;
pub(super) type AsrWsReader = futures_util::stream::SplitStream<AsrWsStream>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AsrTransportPayloadKind {
    SessionJson,
    Audio,
    Terminal,
}

impl AsrTransportPayloadKind {
    fn label(self) -> &'static str {
        match self {
            Self::SessionJson => "session_json",
            Self::Audio => "audio",
            Self::Terminal => "terminal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AsrTransportWriteError {
    Policy {
        message: String,
    },
    Write {
        provider: &'static str,
        payload_kind: AsrTransportPayloadKind,
        message: String,
    },
}

impl AsrTransportWriteError {
    pub(super) fn is_policy_blocked(&self) -> bool {
        matches!(self, Self::Policy { .. })
    }
}

impl fmt::Display for AsrTransportWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Policy { message, .. } => f.write_str(message),
            Self::Write {
                provider,
                payload_kind,
                message,
            } => write!(
                f,
                "WebSocket write failed for {provider} {}: {message}",
                payload_kind.label()
            ),
        }
    }
}

impl std::error::Error for AsrTransportWriteError {}

#[derive(Debug, Clone, Copy)]
pub(super) struct AsrWsWriteGuard {
    provider: &'static str,
    policy: ProviderContentEgressPolicy,
}

impl AsrWsWriteGuard {
    pub(super) const fn new(provider: &'static str, policy: ProviderContentEgressPolicy) -> Self {
        Self { provider, policy }
    }

    pub(super) async fn send_text(
        self,
        writer: &mut AsrWsWriter,
        payload_kind: AsrTransportPayloadKind,
        text: String,
    ) -> Result<(), AsrTransportWriteError> {
        self.check(payload_kind)?;
        writer
            .send(Message::Text(text.into()))
            .await
            .map_err(|error| self.write_error(payload_kind, error))
    }

    pub(super) async fn send_binary(
        self,
        writer: &mut AsrWsWriter,
        payload_kind: AsrTransportPayloadKind,
        bytes: Vec<u8>,
    ) -> Result<(), AsrTransportWriteError> {
        self.check(payload_kind)?;
        writer
            .send(Message::Binary(bytes.into()))
            .await
            .map_err(|error| self.write_error(payload_kind, error))
    }

    /// Send a WebSocket `Ping` control frame as an idle keepalive.
    ///
    /// Control frames carry no transcript/audio content, so — like the empty
    /// `Terminal` frame — they bypass the content-egress policy check. This is
    /// the provider-neutral keepalive fallback for streaming ASR clients whose
    /// protocols document no application-level idle no-op frame (M2 /
    /// audio-graph-63be).
    pub(super) async fn send_ping(
        self,
        writer: &mut AsrWsWriter,
        payload: Vec<u8>,
    ) -> Result<(), AsrTransportWriteError> {
        writer
            .send(Message::Ping(payload.into()))
            .await
            .map_err(|error| self.write_error(AsrTransportPayloadKind::Terminal, error))
    }

    fn check(self, payload_kind: AsrTransportPayloadKind) -> Result<(), AsrTransportWriteError> {
        let result = match payload_kind {
            AsrTransportPayloadKind::SessionJson => self.policy.check_json(self.provider),
            AsrTransportPayloadKind::Audio => self.policy.check_audio(self.provider),
            AsrTransportPayloadKind::Terminal => Ok(()),
        };

        result.map_err(|message| AsrTransportWriteError::Policy { message })
    }

    fn write_error(
        self,
        payload_kind: AsrTransportPayloadKind,
        error: tungstenite::Error,
    ) -> AsrTransportWriteError {
        AsrTransportWriteError::Write {
            provider: self.provider,
            payload_kind,
            message: error.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::ws_fixture::{self, ClientFrame, ServerStep};
    use futures_util::StreamExt;

    #[test]
    fn content_bearing_payloads_require_policy_allow() {
        let guard =
            AsrWsWriteGuard::new("asr.test", ProviderContentEgressPolicy::block("local_only"));

        for payload_kind in [
            AsrTransportPayloadKind::SessionJson,
            AsrTransportPayloadKind::Audio,
        ] {
            let error = guard
                .check(payload_kind)
                .expect_err("content-bearing payload should be blocked");
            assert!(error.is_policy_blocked());
            assert!(error.to_string().contains("Privacy policy blocked"));
            assert!(!error.to_string().contains("secret-session-json"));
        }
    }

    #[test]
    fn terminal_payload_does_not_require_content_transfer_policy() {
        let guard =
            AsrWsWriteGuard::new("asr.test", ProviderContentEgressPolicy::block("local_only"));

        guard
            .check(AsrTransportPayloadKind::Terminal)
            .expect("terminal control frame should not be content-bearing");
    }

    /// Direct write-primitive block test for the shared ASR WebSocket write
    /// guard (2e39). Every streaming ASR sender (deepgram, assemblyai,
    /// openai_realtime, soniox) ships its content frames through THIS exact
    /// `send_text` / `send_binary` primitive. The per-sender `run_io` tests
    /// drive the production loop; this drives the primitive itself against a
    /// live socket so the refusal is proven at the lowest reachable write layer
    /// regardless of which sender routes through it.
    ///
    /// Non-vacuous: the fake server is scripted to expect ONLY the empty
    /// terminal control frame followed by the client close — NO content frame.
    /// If the guard ever stopped calling `check()` inside `send_text` /
    /// `send_binary`, the refused audio/session-json content would reach the
    /// socket and the scripted server's first `expect_binary(vec![])` would
    /// observe the leaked content frame (e.g. `Binary(SECRET_AUDIO...)` or a
    /// `Text` frame) instead of the empty terminator and panic.
    #[tokio::test(flavor = "current_thread")]
    async fn send_primitives_refuse_content_under_blocked_policy_and_write_nothing() {
        // The scripted server expects EXACTLY the empty terminal control frame
        // (allowed even when blocked) and then the close — and NO content
        // frame. A blocked guard must put no audio/session-json bytes on the
        // wire, so the first thing the server ever sees is the empty terminator.
        let (url, server) = ws_fixture::spawn_scripted_server(vec![
            ServerStep::expect_binary(Vec::new()),
            ServerStep::expect_close(),
        ])
        .await;

        let client = ws_fixture::connect_client(&url).await;
        let (mut writer, _reader) = client.split();

        let guard =
            AsrWsWriteGuard::new("asr.test", ProviderContentEgressPolicy::block("local_only"));

        // Audio payload (binary) — payload-like bytes that must never reach the
        // socket and must never appear in the redacted error.
        let secret_audio = b"SECRET_AUDIO_PCM_FRAME".to_vec();
        let binary_error = guard
            .send_binary(&mut writer, AsrTransportPayloadKind::Audio, secret_audio)
            .await
            .expect_err("blocked policy must refuse the audio write primitive");
        assert!(
            binary_error.is_policy_blocked(),
            "audio refusal must be a policy block, not a transport error: {binary_error:?}"
        );
        let binary_message = binary_error.to_string();
        assert!(binary_message.contains("Privacy policy blocked"));
        assert!(binary_message.contains("audio"));
        assert!(binary_message.contains("local_only"));
        assert!(
            !binary_message.contains("SECRET_AUDIO_PCM_FRAME"),
            "policy error must not echo audio bytes: {binary_message}"
        );

        // Session-config payload (text) — carries a JSON handshake the guard
        // classifies as content. Must be refused identically.
        let secret_session_json = r#"{"api_key":"SECRET_SESSION_JSON","sample_rate":16000}"#;
        let text_error = guard
            .send_text(
                &mut writer,
                AsrTransportPayloadKind::SessionJson,
                secret_session_json.to_string(),
            )
            .await
            .expect_err("blocked policy must refuse the session-json write primitive");
        assert!(
            text_error.is_policy_blocked(),
            "session-json refusal must be a policy block: {text_error:?}"
        );
        let text_message = text_error.to_string();
        assert!(text_message.contains("Privacy policy blocked"));
        assert!(text_message.contains("json"));
        assert!(text_message.contains("local_only"));
        assert!(
            !text_message.contains("SECRET_SESSION_JSON"),
            "policy error must not echo session-config payload: {text_message}"
        );

        // Terminal control frames are NOT content-bearing; the guard must still
        // let a close/terminator through even under a blocked policy.
        guard
            .send_binary(&mut writer, AsrTransportPayloadKind::Terminal, Vec::new())
            .await
            .expect("terminal control frame must pass the guard even when blocked");

        // Close the client socket; the scripted server asserts it observed a
        // single close frame and zero content frames before this resolves.
        writer.close().await.expect("client closes socket");

        tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("scripted server task finishes")
            .expect("scripted server task panicked");
    }

    /// Companion to the blocked-policy test above: prove the SAME write
    /// primitive actually puts content on the wire under an allow policy. This
    /// is what makes the blocked test non-vacuous — the guard is the only
    /// difference between "frame reaches the socket" and "frame is refused".
    #[tokio::test(flavor = "current_thread")]
    async fn send_primitives_emit_content_under_allow_policy() {
        let (url, server) = ws_fixture::spawn_scripted_server(vec![
            ServerStep::expect_binary(vec![1, 2, 3, 4]),
            ServerStep::expect_text(r#"{"type":"Start"}"#),
        ])
        .await;

        let client = ws_fixture::connect_client(&url).await;
        let (mut writer, _reader) = client.split();

        let guard = AsrWsWriteGuard::new("asr.test", ProviderContentEgressPolicy::allow());

        guard
            .send_binary(
                &mut writer,
                AsrTransportPayloadKind::Audio,
                vec![1, 2, 3, 4],
            )
            .await
            .expect("allow policy must send the audio frame");
        guard
            .send_text(
                &mut writer,
                AsrTransportPayloadKind::SessionJson,
                r#"{"type":"Start"}"#.to_string(),
            )
            .await
            .expect("allow policy must send the session-json frame");

        let received = tokio::time::timeout(std::time::Duration::from_secs(1), server)
            .await
            .expect("scripted server task finishes")
            .expect("scripted server task panicked");
        assert_eq!(
            received,
            vec![
                ClientFrame::Binary(vec![1, 2, 3, 4]),
                ClientFrame::Text(r#"{"type":"Start"}"#.into()),
            ]
        );
    }
}
