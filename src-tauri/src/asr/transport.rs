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
}
