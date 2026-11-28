//! Automatic Speech Recognition (ASR) module.
//!
//! Uses whisper-rs to transcribe speech utterances into text segments.
//! The speech pipeline owns the Whisper state and calls
//! [`AsrWorker::transcribe_segment`] per `SpeechSegment`, producing
//! `TranscriptSegment`s.

use std::path::{Path, PathBuf};
use std::time::Duration;

use audio_graph_ipc_contract::credential_contract::{
    BuiltInCredentialSetId, CredentialError, CredentialErrorCode, CredentialSafeRecoveryAction,
    CredentialSetId,
};
use audio_graph_ipc_contract::runtime_diagnostic::{
    RuntimeDiagnostic, RuntimeDiagnosticContext, RuntimeErrorCode, RuntimeErrorDiagnostic,
    RuntimeOperation, RuntimeSafeRecoveryAction, RuntimeTransport,
};
use tokio_tungstenite::tungstenite::{
    self, protocol::CloseFrame, protocol::frame::coding::CloseCode,
};

#[cfg(feature = "asr-whisper")]
use log::debug;
#[cfg(feature = "asr-whisper")]
use uuid::Uuid;
#[cfg(feature = "asr-whisper")]
use whisper_rs::{FullParams, SamplingStrategy};

pub mod assemblyai;
pub mod aws_transcribe;
pub mod cloud;
pub mod deepgram;
#[cfg(test)]
mod event_fixtures;
#[cfg(test)]
mod fixtures;
pub mod gladia;
pub mod moonshine;
pub mod openai_realtime;
// The reconnect ladder now lives at crate root (`crate::reconnect`) so the
// Gemini + Aura transports can share it too (review n2). Re-exported here so the
// ASR clients' existing `super::reconnect::*` paths keep resolving.
pub(crate) use crate::reconnect;
pub mod revai;
#[cfg(feature = "sherpa-streaming")]
pub mod sherpa_streaming;
pub mod soniox;
pub mod speechmatics;
mod transport;
#[cfg(test)]
mod ws_fixture;

/// Convert a structural ASR failure before it can reach an event or log.
fn websocket_diagnostic(code: RuntimeErrorCode) -> RuntimeDiagnostic {
    asr_diagnostic(code, RuntimeTransport::Websocket)
}

fn asr_diagnostic(code: RuntimeErrorCode, transport: RuntimeTransport) -> RuntimeDiagnostic {
    use RuntimeErrorCode as Code;
    use RuntimeSafeRecoveryAction as Recovery;

    let context =
        RuntimeDiagnosticContext::new(RuntimeOperation::Transcription).with_transport(transport);
    let (retryable, recovery) = match code {
        Code::CredentialRejected => {
            return RuntimeErrorDiagnostic::credential_rejected(context).into();
        }
        Code::CredentialExpired => {
            return RuntimeErrorDiagnostic::credential_expired(context).into();
        }
        Code::PermissionDenied => return RuntimeErrorDiagnostic::permission_denied(context).into(),
        Code::InvalidConfiguration => {
            // OpenAI Realtime uses this when session.update itself is rejected.
            return RuntimeErrorDiagnostic::invalid_configuration(context).into();
        }
        Code::RateLimited => (true, Recovery::RetryAfterDelay),
        Code::NetworkUnreachable => (true, Recovery::CheckNetwork),
        Code::Timeout | Code::ProviderUnavailable => (true, Recovery::Retry),
        Code::RequestRejected | Code::CapacityExhausted => (false, Recovery::ReviewConfiguration),
        Code::PolicyBlocked => (false, Recovery::ReviewPolicy),
        _ => (false, Recovery::None),
    };
    RuntimeErrorDiagnostic::new(code, retryable, recovery, context).into()
}

fn required_api_key(
    api_key: &str,
    provider: BuiltInCredentialSetId,
) -> Result<&str, RuntimeDiagnostic> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        return Err(missing_provider_credential(provider));
    }
    Ok(api_key)
}

fn missing_provider_credential(provider: BuiltInCredentialSetId) -> RuntimeDiagnostic {
    CredentialError {
        code: CredentialErrorCode::Missing,
        retryable: false,
        recovery_action: CredentialSafeRecoveryAction::ReenterCredential,
        set_id: Some(CredentialSetId::BuiltIn(provider)),
    }
    .into()
}

fn is_terminal_reconnect_error(diagnostic: &RuntimeDiagnostic) -> bool {
    use RuntimeErrorCode as Code;

    match diagnostic {
        RuntimeDiagnostic::Credential(error) => error.code == CredentialErrorCode::Missing,
        // RequestRejected also covers ambiguous statuses such as 400. Keep
        // the existing reconnect budget for these coarse failures.
        RuntimeDiagnostic::Runtime(error) => matches!(
            error.code,
            Code::CredentialRejected
                | Code::CredentialExpired
                | Code::PermissionDenied
                | Code::InvalidConfiguration
        ),
    }
}

fn websocket_error(error: &tungstenite::Error) -> RuntimeDiagnostic {
    websocket_upgrade_error(error, false)
}

fn websocket_upgrade_error(error: &tungstenite::Error, credential_sent: bool) -> RuntimeDiagnostic {
    use RuntimeErrorCode as Code;
    use tungstenite::Error;

    let code = match error {
        Error::Http(response) => {
            return RuntimeErrorDiagnostic::from_http_status(
                response.status().as_u16(),
                credential_sent,
                RuntimeDiagnosticContext::new(RuntimeOperation::Transcription)
                    .with_transport(RuntimeTransport::Websocket),
            )
            .into();
        }
        Error::Io(error) if error.kind() == std::io::ErrorKind::TimedOut => Code::Timeout,
        Error::Io(_) | Error::ConnectionClosed | Error::AlreadyClosed => Code::NetworkUnreachable,
        // A failed TLS handshake can indicate a certificate or protocol problem.
        Error::Protocol(_) | Error::Utf8(_) | Error::Tls(_) => Code::ProtocolError,
        Error::Capacity(_) | Error::WriteBufferFull(_) => Code::CapacityExhausted,
        Error::Url(_) => Code::RequestRejected,
        _ => Code::Internal,
    };
    websocket_diagnostic(code)
}

fn websocket_write_error(error: &transport::AsrTransportWriteError) -> RuntimeDiagnostic {
    match error {
        transport::AsrTransportWriteError::Policy => {
            websocket_diagnostic(RuntimeErrorCode::PolicyBlocked)
        }
        transport::AsrTransportWriteError::Write { diagnostic, .. } => diagnostic.clone(),
    }
}

/// Classify a peer close. Local cancellation is decided by the adapter.
fn websocket_close(frame: Option<&CloseFrame>) -> RuntimeDiagnostic {
    use RuntimeErrorCode as Code;

    let code = match frame.map(|frame| frame.code) {
        Some(
            CloseCode::Normal
            | CloseCode::Away
            | CloseCode::Abnormal
            | CloseCode::Error
            | CloseCode::Restart,
        ) => Code::ProviderUnavailable,
        Some(CloseCode::Protocol | CloseCode::Unsupported | CloseCode::Invalid) => {
            Code::ProtocolError
        }
        Some(CloseCode::Policy) => Code::PolicyBlocked,
        Some(CloseCode::Size) => Code::CapacityExhausted,
        Some(CloseCode::Again) => Code::RateLimited,
        _ => Code::Internal,
    };
    websocket_diagnostic(code)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsrParseError {
    InvalidJson,
    UnsupportedMessageType,
}

impl From<AsrParseError> for RuntimeDiagnostic {
    fn from(error: AsrParseError) -> Self {
        websocket_diagnostic(match error {
            AsrParseError::InvalidJson => RuntimeErrorCode::InvalidResponse,
            AsrParseError::UnsupportedMessageType => RuntimeErrorCode::Unsupported,
        })
    }
}

impl std::fmt::Display for AsrParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&RuntimeDiagnostic::from(*self), f)
    }
}

impl std::error::Error for AsrParseError {}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;
    use crate::error::AppError;
    use crate::test_log_capture::ThreadLogCapture;
    use audio_graph_ipc_contract::runtime_diagnostic::RuntimeStatusClass;
    use serde_json::json;

    pub(super) const SECRET: &str = "asr-secret-canary";
    pub(super) const REQUEST_ID: &str = "asr-request-id-canary";
    const PROSE: &str = "asr-provider-prose-canary";
    pub(super) const UNKNOWN: &str = "asr-unknown-code-canary";
    pub(super) const CLOSE_REASON: &str = "asr-close-reason-canary";
    const NATIVE_PROSE: &str = "asr-native-transport-canary";
    const BODY_CHARS: usize = 379;
    const BODY_BYTES: usize = 691;

    pub(super) struct LogCapture(ThreadLogCapture);

    impl LogCapture {
        pub(super) fn start() -> Self {
            Self(ThreadLogCapture::start())
        }

        pub(super) fn finish(self) -> Vec<String> {
            let asr_module = module_path!().strip_suffix("::diagnostic_tests").unwrap();
            self.0
                .finish()
                .into_iter()
                .filter(|record| record.target.starts_with(asr_module))
                .map(|record| record.message)
                .collect()
        }
    }

    pub(super) fn wire_prose() -> String {
        let canaries = format!("{PROSE}{SECRET}");
        // Give both the exact character count and the exact byte count their own canary.
        let padding = "é".repeat(BODY_BYTES - BODY_CHARS);
        let filler = "x".repeat(BODY_CHARS - canaries.len() - padding.chars().count());
        format!("{canaries}{filler}{padding}")
    }

    pub(super) fn assert_safe(value: &str) {
        for private in [
            SECRET,
            REQUEST_ID,
            PROSE,
            UNKNOWN,
            CLOSE_REASON,
            NATIVE_PROSE,
        ] {
            assert!(!value.contains(private), "diagnostic exposed {private}");
        }
        for exact_length in [BODY_CHARS, BODY_BYTES] {
            assert!(
                !value.contains(&exact_length.to_string()),
                "diagnostic exposed an exact length"
            );
        }
    }

    fn assert_diagnostic(diagnostic: &RuntimeDiagnostic, code: RuntimeErrorCode) {
        let RuntimeDiagnostic::Runtime(detail) = diagnostic else {
            panic!("expected a runtime diagnostic");
        };
        assert_eq!(detail.code, code);
        assert_eq!(detail.context.operation, RuntimeOperation::Transcription);
        assert_eq!(detail.context.transport, Some(RuntimeTransport::Websocket));
        let app_error = AppError::from(diagnostic.clone());
        assert!(matches!(app_error, AppError::RuntimeDiagnostic(_)));
        assert_safe(&format!(
            "{diagnostic:?} {diagnostic} {app_error:?} {app_error}"
        ));
        assert_safe(&serde_json::to_string(&app_error).unwrap());
    }

    pub(super) fn assert_outcome(
        diagnostic: &RuntimeDiagnostic,
        expected: (
            RuntimeErrorCode,
            bool,
            RuntimeSafeRecoveryAction,
            Option<RuntimeStatusClass>,
        ),
    ) {
        assert_diagnostic(diagnostic, expected.0);
        let RuntimeDiagnostic::Runtime(detail) = diagnostic else {
            unreachable!();
        };
        assert_eq!(
            (
                detail.code,
                detail.retryable,
                detail.recovery_action,
                detail.context.status_class
            ),
            expected
        );
    }

    pub(super) async fn reject_upgrade(
        status: u16,
        credential_sent: bool,
    ) -> (String, tokio::task::JoinHandle<()>) {
        reject_upgrades(vec![(status, Some(wire_prose()))], credential_sent).await
    }

    #[allow(
        clippy::result_large_err,
        reason = "tungstenite's upgrade callback requires an HTTP response as its error"
    )]
    pub(super) async fn reject_upgrades(
        responses: Vec<(u16, Option<String>)>,
        credential_sent: bool,
    ) -> (String, tokio::task::JoinHandle<()>) {
        use tungstenite::handshake::server::{Request, Response};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/asr", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for (status, body) in responses {
                let (stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
                    .await
                    .expect("timed out waiting for the test upgrade")
                    .unwrap();
                let result = tokio_tungstenite::accept_hdr_async(
                    stream,
                    |request: &Request, _: Response| {
                        assert_eq!(
                            request.headers().contains_key("authorization"),
                            credential_sent
                        );
                        Err(tungstenite::http::Response::builder()
                            .status(status)
                            .header("x-request-id", REQUEST_ID)
                            .body(body)
                            .unwrap())
                    },
                )
                .await;
                assert!(result.is_err(), "server must reject the upgrade");
            }
        });
        (url, server)
    }

    fn assert_parse_error(error: AsrParseError, expected: AsrParseError) {
        assert_eq!(error, expected);
        assert_safe(&format!("{error:?} {error}"));
    }

    pub(super) async fn assert_loopback_io(
        error_frame: String,
        run: impl AsyncFn(
            ws_fixture::ClientSocket,
            Option<String>,
            Duration,
        ) -> (RuntimeDiagnostic, Vec<RuntimeDiagnostic>),
    ) {
        use RuntimeErrorCode as Code;
        use futures_util::SinkExt;
        use tungstenite::{
            Message,
            protocol::{
                WebSocketConfig,
                frame::{
                    Frame,
                    coding::{Data, OpCode},
                },
            },
        };

        // Exercise each exit on a fresh socket so the preceding close or read
        // error cannot mask a write or keepalive failure.
        for (server_frames, audio, max_write, keepalive, code, log_line) in [
            (
                vec![
                    Message::Text(error_frame.into()),
                    Message::Close(Some(CloseFrame {
                        code: CloseCode::Normal,
                        reason: CLOSE_REASON.into(),
                    })),
                ],
                None,
                4096,
                Duration::from_secs(10),
                Code::ProviderUnavailable,
                "server closed connection runtime/provider_unavailable",
            ),
            (
                vec![Message::Frame(Frame::message(
                    [vec![0xff], wire_prose().into_bytes()].concat(),
                    OpCode::Data(Data::Text),
                    true,
                ))],
                None,
                4096,
                Duration::from_secs(10),
                Code::ProtocolError,
                "WebSocket read error: runtime/protocol_error",
            ),
            (
                Vec::new(),
                Some(wire_prose()),
                64,
                Duration::from_secs(10),
                Code::CapacityExhausted,
                "failed to send audio: runtime/capacity_exhausted",
            ),
            (
                Vec::new(),
                None,
                1,
                Duration::from_millis(5),
                Code::CapacityExhausted,
                "failed to send keepalive: runtime/capacity_exhausted",
            ),
        ] {
            let (release, wait) = tokio::sync::oneshot::channel::<()>();
            let (url, server) = ws_fixture::spawn_server(move |mut socket| async move {
                for frame in server_frames {
                    socket.send(frame).await.unwrap();
                }
                let _ = wait.await;
            })
            .await;
            // Set the cap below the size of the selected outbound frame to
            // force WriteBufferFull without racing a remote socket shutdown.
            let config = WebSocketConfig::default()
                .write_buffer_size(0)
                .max_write_buffer_size(max_write);
            let (socket, _) =
                tokio_tungstenite::connect_async_with_config(&url, Some(config), false)
                    .await
                    .unwrap();
            let capture = LogCapture::start();
            let (disconnect, events) =
                tokio::time::timeout(Duration::from_secs(2), run(socket, audio, keepalive))
                    .await
                    .expect("adapter IO canary did not exit");
            assert_diagnostic(&disconnect, code);
            if code == Code::ProviderUnavailable {
                let [error] = events.as_slice() else {
                    panic!("expected one provider error before close");
                };
                assert_diagnostic(error, Code::Internal);
            } else {
                assert!(events.is_empty());
            }
            let logs = capture.finish().join("\n");
            assert!(logs.contains(log_line), "missing {log_line:?} in {logs:?}");
            assert_safe(&logs);
            let _ = release.send(());
            server.await.unwrap();
        }
    }

    pub(super) fn collect_io_errors<E: std::fmt::Debug + serde::Serialize>(
        events: crossbeam_channel::Receiver<E>,
        unwrap_error: impl Fn(E) -> RuntimeDiagnostic,
    ) -> Vec<RuntimeDiagnostic> {
        events
            .try_iter()
            .map(|event| {
                assert_safe(&format!("{event:?}"));
                assert_safe(&serde_json::to_string(&event).unwrap());
                unwrap_error(event)
            })
            .collect()
    }

    #[test]
    fn asr_logs_discard_private_wire_details() {
        // Logger and level are process-global. Isolate them from the rest of
        // the test suite, which also tests changes to the global log level.
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "diagnostic_log_canary"])
            .output()
            .expect("run the ASR log-capture subprocess");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success() && stdout.contains("11 passed; 0 failed"),
            "ASR log canaries failed\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn websocket_handshake_transport_and_close_discard_private_inputs() {
        use RuntimeErrorCode as Code;
        use tungstenite::{
            Error,
            http::{Response, StatusCode},
        };

        for (status, code, class) in [
            (
                StatusCode::TEMPORARY_REDIRECT,
                Code::RequestRejected,
                RuntimeStatusClass::Redirect,
            ),
            (
                StatusCode::BAD_REQUEST,
                Code::RequestRejected,
                RuntimeStatusClass::ClientError,
            ),
            (
                StatusCode::UNAUTHORIZED,
                Code::RequestRejected,
                RuntimeStatusClass::ClientError,
            ),
            (
                StatusCode::TOO_MANY_REQUESTS,
                Code::RateLimited,
                RuntimeStatusClass::ClientError,
            ),
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Code::ProviderUnavailable,
                RuntimeStatusClass::ServerError,
            ),
            (
                StatusCode::GATEWAY_TIMEOUT,
                Code::ProviderUnavailable,
                RuntimeStatusClass::ServerError,
            ),
        ] {
            let error = Error::Http(Box::new(
                Response::builder()
                    .status(status)
                    .header("x-request-id", REQUEST_ID)
                    .header("x-provider-info", SECRET)
                    .body(Some(wire_prose().into_bytes()))
                    .unwrap(),
            ));
            let diagnostic = websocket_error(&error);
            assert_diagnostic(&diagnostic, code);
            let RuntimeDiagnostic::Runtime(detail) = diagnostic else {
                unreachable!();
            };
            assert_eq!(detail.context.status_class, Some(class));
            assert!(!format!("{detail:?}").contains(&status.as_u16().to_string()));
        }

        for (error, code) in [
            (
                Error::Io(std::io::Error::other(NATIVE_PROSE)),
                Code::NetworkUnreachable,
            ),
            (
                Error::Io(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    NATIVE_PROSE,
                )),
                Code::Timeout,
            ),
            (Error::Utf8(NATIVE_PROSE.into()), Code::ProtocolError),
        ] {
            assert_diagnostic(&websocket_error(&error), code);
        }
        let close = CloseFrame {
            code: CloseCode::Library(4763),
            reason: format!("{CLOSE_REASON} {REQUEST_ID} {SECRET}").into(),
        };
        let diagnostic = websocket_close(Some(&close));
        assert_diagnostic(&diagnostic, Code::Internal);
        assert!(!format!("{diagnostic:?}").contains("4763"));
        assert_diagnostic(&websocket_close(None), Code::Internal);
    }

    #[test]
    #[ignore = "runs in the isolated ASR log-capture subprocess"]
    fn deepgram_diagnostic_log_canary() {
        let capture = LogCapture::start();
        let (tx, rx) = crossbeam_channel::unbounded();
        for payload in [
            json!({"type": "Metadata", "request_id": REQUEST_ID}),
            json!({"type": UNKNOWN, "request_id": REQUEST_ID}),
            json!({"type": "TurnInfo", "event": UNKNOWN, "request_id": REQUEST_ID}),
        ] {
            deepgram::handle_server_message(&payload.to_string(), &tx);
        }
        assert!(rx.try_recv().is_err());
        let payload = json!({
            "type": "Error", "err_code": UNKNOWN, "description": wire_prose(),
            "request_id": REQUEST_ID,
        });
        deepgram::handle_server_message(&payload.to_string(), &tx);
        let event = rx.try_recv().unwrap();
        assert_safe(&format!("{event:?}"));
        assert_safe(&serde_json::to_string(&event).unwrap());
        let deepgram::DeepgramEvent::Error { message } = event else {
            panic!("expected Deepgram error event");
        };
        assert_diagnostic(&message, RuntimeErrorCode::Internal);
        let logs = capture.finish().join("\n");
        assert!(logs.contains("received metadata"));
        assert!(logs.contains("unhandled Flux TurnInfo event"));
        assert_safe(&logs);
    }

    #[test]
    #[ignore = "runs in the isolated ASR log-capture subprocess"]
    fn assemblyai_diagnostic_log_canary() {
        let capture = LogCapture::start();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut parser = assemblyai::AssemblyAiV3Parser::new("source");
        let payload = json!({
            "type": "Error", "error_code": UNKNOWN, "error": wire_prose(), "request_id": REQUEST_ID,
            "token": SECRET,
        });
        let payload = payload.to_string();
        let parsed = parser.parse_message(&payload, 0).unwrap();
        assert_safe(&format!("{parsed:?}"));
        assert_diagnostic(&parsed.error.unwrap().message, RuntimeErrorCode::Internal);
        assemblyai::handle_server_message(&payload, &tx);
        let event = rx.try_recv().unwrap();
        assert_safe(&format!("{event:?}"));
        assert_safe(&serde_json::to_string(&event).unwrap());
        let assemblyai::AssemblyAIEvent::Error { message } = event else {
            panic!("expected AssemblyAI error event");
        };
        assert_diagnostic(&message, RuntimeErrorCode::Internal);
        for (wire_code, code) in [
            (1008, RuntimeErrorCode::RequestRejected),
            (1011, RuntimeErrorCode::ProviderUnavailable),
            (3005, RuntimeErrorCode::Internal),
            (3006, RuntimeErrorCode::ProtocolError),
            (3007, RuntimeErrorCode::RequestRejected),
            (3008, RuntimeErrorCode::Timeout),
        ] {
            let payload = json!({"type": "Error", "error_code": wire_code, "error": wire_prose()});
            let payload = payload.to_string();
            let parsed = parser.parse_message(&payload, 0).unwrap();
            assert_diagnostic(&parsed.error.unwrap().message, code);
            assemblyai::handle_server_message(&payload, &tx);
            let assemblyai::AssemblyAIEvent::Error { message } = rx.try_recv().unwrap() else {
                panic!("expected AssemblyAI error event");
            };
            assert_diagnostic(&message, code);
        }
        let begin = parser
            .parse_message(&json!({"type": "Begin", "id": REQUEST_ID}).to_string(), 0)
            .unwrap();
        assert_eq!(begin.session_id.as_deref(), Some(REQUEST_ID));
        assert!(format!("{begin:?}").contains("session_id_present: true"));
        assert_safe(&format!("{begin:?}"));
        let unknown = json!({"type": UNKNOWN, "request_id": REQUEST_ID}).to_string();
        assemblyai::handle_server_message(&unknown, &tx);
        assert!(rx.try_recv().is_err());
        assert_parse_error(
            parser.parse_message(&unknown, 0).unwrap_err(),
            AsrParseError::UnsupportedMessageType,
        );
        let turn = json!({
            "type": "Turn", "turn_order": 1, "transcript": "hello",
            "request_id": REQUEST_ID, "id": REQUEST_ID, "token": SECRET,
            "error": wire_prose(), "words": [{"text": "hello", "more_info": SECRET}],
        });
        assemblyai::handle_server_message(&turn.to_string(), &tx);
        let assemblyai::AssemblyAIEvent::ServerMessage { frame, .. } = rx.try_recv().unwrap()
        else {
            panic!("expected AssemblyAI transcript frame");
        };
        assert_safe(frame.as_str());
        let parsed = parser.parse_message(frame.as_str(), 0).unwrap();
        assert_eq!(parsed.revisions[0].payload.text, "hello");
        let logs = capture.finish().join("\n");
        assert!(logs.contains("server error"));
        assert!(logs.contains("unsupported server message"));
        assert_safe(&logs);
    }

    #[test]
    #[ignore = "runs in the isolated ASR log-capture subprocess"]
    fn openai_diagnostic_log_canary() {
        let capture = LogCapture::start();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut accumulator = std::collections::HashMap::new();
        let payload = json!({
            "type": "conversation.item.input_audio_transcription.failed", "item_id": REQUEST_ID,
            "error": {"type": UNKNOWN, "code": UNKNOWN, "message": wire_prose(),
                "param": SECRET, "event_id": REQUEST_ID},
        });
        openai_realtime::handle_server_message(&payload.to_string(), &tx, &mut accumulator);
        let event = rx.try_recv().unwrap();
        assert_safe(&format!("{event:?}"));
        assert_safe(&serde_json::to_string(&event).unwrap());
        let openai_realtime::OpenAiRealtimeEvent::Error { message } = event else {
            panic!("expected OpenAI error event");
        };
        assert_diagnostic(&message, RuntimeErrorCode::Internal);
        for (wire_code, code) in [
            ("invalid_api_key", RuntimeErrorCode::Internal),
            ("session_expired", RuntimeErrorCode::Timeout),
        ] {
            let payload =
                json!({"type": "error", "error": {"code": wire_code, "message": wire_prose()}});
            openai_realtime::handle_server_message(&payload.to_string(), &tx, &mut accumulator);
            let openai_realtime::OpenAiRealtimeEvent::Error { message } = rx.try_recv().unwrap()
            else {
                panic!("expected OpenAI error event");
            };
            assert_diagnostic(&message, code);
        }
        let unknown = json!({"type": UNKNOWN, "request_id": REQUEST_ID});
        openai_realtime::handle_server_message(&unknown.to_string(), &tx, &mut accumulator);
        assert!(rx.try_recv().is_err());
        let logs = capture.finish().join("\n");
        assert!(logs.contains("unhandled message type"));
        assert_safe(&logs);
    }

    #[test]
    #[ignore = "runs in the isolated ASR log-capture subprocess"]
    fn soniox_diagnostic_log_canary() {
        let capture = LogCapture::start();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut parser = soniox::SonioxRealtimeParser::new("source");
        let payload = json!({
            "error_type": UNKNOWN, "error_code": {"detail": UNKNOWN}, "error_message": wire_prose(),
            "request_id": REQUEST_ID, "more_info": SECRET, "total_audio_proc_ms": BODY_BYTES,
        });
        let payload = payload.to_string();
        let parsed = parser.parse_message(&payload, 0).unwrap();
        assert_safe(&format!("{parsed:?}"));
        assert_diagnostic(&parsed.error.unwrap(), RuntimeErrorCode::Internal);
        soniox::handle_server_message(&payload, &tx, &mut parser).unwrap();
        let event = rx.try_recv().unwrap();
        assert_safe(&format!("{event:?}"));
        let soniox::SonioxEvent::Error { message } = event else {
            panic!("expected Soniox error event");
        };
        assert_diagnostic(&message, RuntimeErrorCode::Internal);
        for (wire_code, code) in [
            (408, RuntimeErrorCode::Timeout),
            (429, RuntimeErrorCode::RateLimited),
            (504, RuntimeErrorCode::Timeout),
        ] {
            let payload = json!({"error_code": wire_code, "error_message": wire_prose()});
            soniox::handle_server_message(&payload.to_string(), &tx, &mut parser).unwrap();
            let soniox::SonioxEvent::Error { message } = rx.try_recv().unwrap() else {
                panic!("expected Soniox error event");
            };
            assert_diagnostic(&message, code);
        }
        assert_safe(&capture.finish().join("\n"));
    }

    #[test]
    #[ignore = "runs in the isolated ASR log-capture subprocess"]
    fn gladia_diagnostic_log_canary() {
        let capture = LogCapture::start();
        let mut parser = gladia::GladiaLiveParser::new("source");
        let payload = json!({
            "type": UNKNOWN, "session_id": REQUEST_ID, "acknowledged": false,
            "error": {"code": UNKNOWN, "message": wire_prose(), "token": SECRET},
            "data": {"byte_range": [0, BODY_BYTES]},
        });
        let parsed = parser.parse_message(&payload.to_string(), 0).unwrap();
        assert_safe(&format!("{parsed:?}"));
        assert_diagnostic(&parsed.error.unwrap(), RuntimeErrorCode::Internal);
        let begin = json!({"type": "start_session", "session_id": REQUEST_ID});
        let begin = parser.parse_message(&begin.to_string(), 0).unwrap();
        assert_eq!(begin.session_id.as_deref(), Some(REQUEST_ID));
        assert!(format!("{begin:?}").contains("session_id_present: true"));
        assert_safe(&format!("{begin:?}"));
        let unknown = json!({"type": UNKNOWN, "request_id": REQUEST_ID});
        assert_parse_error(
            parser.parse_message(&unknown.to_string(), 0).unwrap_err(),
            AsrParseError::UnsupportedMessageType,
        );
        assert_safe(&capture.finish().join("\n"));
    }

    #[test]
    #[ignore = "runs in the isolated ASR log-capture subprocess"]
    fn speechmatics_diagnostic_log_canary() {
        let capture = LogCapture::start();
        let mut parser = speechmatics::SpeechmaticsRealtimeParser::new("source");
        let payload = json!({
            "message": "Error", "type": UNKNOWN, "reason": wire_prose(), "code": BODY_CHARS,
            "seq_no": BODY_BYTES, "id": REQUEST_ID, "token": SECRET,
        });
        let parsed = parser.parse_message(&payload.to_string(), 0).unwrap();
        assert_safe(&format!("{parsed:?}"));
        assert_diagnostic(&parsed.error.unwrap(), RuntimeErrorCode::Internal);
        let begin = json!({"message": "RecognitionStarted", "id": REQUEST_ID});
        let begin = parser.parse_message(&begin.to_string(), 0).unwrap();
        assert_eq!(begin.recognition_id.as_deref(), Some(REQUEST_ID));
        assert!(format!("{begin:?}").contains("recognition_id_present: true"));
        assert_safe(&format!("{begin:?}"));
        let unknown = json!({"message": UNKNOWN, "id": REQUEST_ID});
        assert_parse_error(
            parser.parse_message(&unknown.to_string(), 0).unwrap_err(),
            AsrParseError::UnsupportedMessageType,
        );
        assert_diagnostic(
            &speechmatics::classify_close_code(4763),
            RuntimeErrorCode::Internal,
        );
        assert_safe(&capture.finish().join("\n"));
    }

    #[test]
    #[ignore = "runs in the isolated ASR log-capture subprocess"]
    fn revai_diagnostic_log_canary() {
        let capture = LogCapture::start();
        let mut parser = revai::RevAiStreamingParser::new("source");
        let payload = json!({
            "type": UNKNOWN, "id": REQUEST_ID, "reason": wire_prose(), "token": SECRET,
        });
        assert_parse_error(
            parser.parse_message(&payload.to_string(), 0).unwrap_err(),
            AsrParseError::UnsupportedMessageType,
        );
        let malformed = json!({"type": wire_prose(), "elements": SECRET, "id": REQUEST_ID});
        assert_parse_error(
            parser.parse_message(&malformed.to_string(), 0).unwrap_err(),
            AsrParseError::InvalidJson,
        );
        let begin = json!({"type": "connected", "id": REQUEST_ID});
        let begin = parser.parse_message(&begin.to_string(), 0).unwrap();
        assert_eq!(begin.connected_id.as_deref(), Some(REQUEST_ID));
        assert!(format!("{begin:?}").contains("connected_id_present: true"));
        assert_safe(&format!("{begin:?}"));
        assert_diagnostic(
            &revai::classify_close_code(4763),
            RuntimeErrorCode::Internal,
        );
        assert_safe(&capture.finish().join("\n"));
    }
}

/// Provider-client defense-in-depth guard for session content egress.
///
/// Command handlers still enforce the user-facing privacy mode before a
/// session starts. This value is threaded into long-lived provider clients so
/// direct lower-level calls cannot send content if a caller bypasses the
/// command/session gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderContentEgressPolicy {
    allows_content: bool,
    privacy_mode: &'static str,
}

impl ProviderContentEgressPolicy {
    pub const fn allow() -> Self {
        Self {
            allows_content: true,
            privacy_mode: "byok_cloud",
        }
    }

    pub const fn block(privacy_mode: &'static str) -> Self {
        Self {
            allows_content: false,
            privacy_mode,
        }
    }

    pub fn from_privacy_mode(mode: crate::settings::PrivacyMode) -> Self {
        if mode.allows_session_cloud_content_transfer() {
            Self::allow()
        } else {
            Self::block(mode.as_str())
        }
    }

    pub fn from_privacy_mode_and_transfer_requirement(
        mode: crate::settings::PrivacyMode,
        requires_cloud_content_transfer: bool,
    ) -> Self {
        if !requires_cloud_content_transfer || mode.allows_session_cloud_content_transfer() {
            Self::allow()
        } else {
            Self::block(mode.as_str())
        }
    }

    pub fn check_audio(self, provider: &str) -> Result<(), String> {
        self.check_content(provider, "audio")
    }

    pub fn check_text(self, provider: &str) -> Result<(), String> {
        self.check_content(provider, "text")
    }

    pub fn check_prompt(self, provider: &str) -> Result<(), String> {
        self.check_content(provider, "prompt")
    }

    pub fn check_json(self, provider: &str) -> Result<(), String> {
        self.check_content(provider, "json")
    }

    pub fn check_content(self, provider: &str, data_class: &str) -> Result<(), String> {
        if self.allows_content {
            return Ok(());
        }

        Err(format!(
            "Privacy policy blocked {data_class} egress to {provider} in mode {}",
            self.privacy_mode
        ))
    }
}

impl Default for ProviderContentEgressPolicy {
    fn default() -> Self {
        Self::block("explicit_policy_required")
    }
}

#[cfg(test)]
mod provider_content_egress_policy_tests {
    use super::ProviderContentEgressPolicy;

    #[test]
    fn provider_content_egress_policy_requires_explicit_allow_by_default() {
        let policy = ProviderContentEgressPolicy::default();

        for result in [
            policy.check_audio("asr.deepgram"),
            policy.check_text("tts.deepgram_aura"),
            policy.check_prompt("llm.openrouter"),
            policy.check_json("llm.api"),
        ] {
            let error = result.expect_err("default policy should require an explicit allow");
            assert!(error.contains("Privacy policy blocked"));
            assert!(error.contains("explicit_policy_required"));
        }
    }

    #[test]
    fn provider_content_egress_policy_blocks_each_content_class_without_payload() {
        let policy = ProviderContentEgressPolicy::block("local_only");

        for (data_class, result) in [
            ("audio", policy.check_audio("asr.deepgram")),
            ("text", policy.check_text("tts.deepgram_aura")),
            ("prompt", policy.check_prompt("llm.openrouter")),
            ("json", policy.check_json("llm.api")),
        ] {
            let error = result.expect_err("blocked policy should reject content");
            assert!(error.contains("Privacy policy blocked"));
            assert!(error.contains(data_class));
            assert!(error.contains("local_only"));
            assert!(
                !error.contains("patient said private diagnosis"),
                "policy error must not echo payload text: {error}"
            );
        }
    }

    #[test]
    fn provider_content_egress_policy_allows_local_transfer_requirement_in_local_only_mode() {
        let local_loopback =
            ProviderContentEgressPolicy::from_privacy_mode_and_transfer_requirement(
                crate::settings::PrivacyMode::LocalOnly,
                false,
            );
        assert!(
            local_loopback.check_prompt("llm.api").is_ok(),
            "local-only mode must still allow loopback/local providers"
        );

        let remote = ProviderContentEgressPolicy::from_privacy_mode_and_transfer_requirement(
            crate::settings::PrivacyMode::LocalOnly,
            true,
        );
        assert!(remote.check_prompt("llm.api").is_err());
    }
}

// Only the whisper-gated `transcribe_segment` returns `TranscriptSegment`s now
// that the vestigial `Sender<TranscriptSegment>` field is gone (FA-6b).
#[cfg(feature = "asr-whisper")]
use crate::state::TranscriptSegment;

/// A segment of speech audio ready for ASR transcription.
///
/// This is the ASR module's input type — it represents a contiguous chunk
/// of speech audio (typically ~2 seconds) accumulated from the pipeline.
#[derive(Debug, Clone)]
pub struct SpeechSegment {
    /// Identifier of the audio source that produced this segment.
    pub source_id: String,
    /// 16kHz mono f32 audio data for the speech segment.
    pub audio: Vec<f32>,
    /// Start time relative to stream start.
    pub start_time: Duration,
    /// End time relative to stream start.
    pub end_time: Duration,
    /// Number of audio frames (equal to `audio.len()`).
    pub num_frames: usize,
}

/// Configuration for the ASR worker.
pub struct AsrConfig {
    /// Path to the Whisper GGML model file (e.g. `models/ggml-small.en.bin`).
    pub model_path: PathBuf,
    /// Language code for transcription (e.g. `"en"`).
    pub language: String,
    /// Number of threads for whisper inference. Default: 4.
    pub n_threads: i32,
    /// Sampling temperature. 0.0 = greedy. Default: 0.0.
    pub temperature: f32,
    /// Beam size (only used with beam-search strategy). Default: 5.
    pub beam_size: i32,
}

impl AsrConfig {
    /// Create an `AsrConfig` with the model path resolved under the given
    /// models directory.
    pub fn with_models_dir(models_dir: &Path) -> Self {
        Self {
            model_path: models_dir.join("ggml-small.en.bin"),
            language: "en".to_string(),
            n_threads: 4,
            temperature: 0.0,
            beam_size: 5,
        }
    }

    pub fn with_models_dir_and_model(models_dir: &Path, model_filename: &str) -> Self {
        Self {
            model_path: models_dir.join(model_filename),
            language: "en".to_string(),
            n_threads: 4,
            temperature: 0.0,
            beam_size: 5,
        }
    }
}

impl Default for AsrConfig {
    fn default() -> Self {
        Self {
            model_path: PathBuf::from("models/ggml-small.en.bin"),
            language: "en".to_string(),
            n_threads: 4,
            temperature: 0.0,
            beam_size: 5,
        }
    }
}

/// ASR worker that processes speech segments into transcript segments.
///
/// The live entrypoint is [`AsrWorker::transcribe_segment`]: the speech
/// pipeline (`speech/mod.rs`) owns the Whisper state and drives transcription
/// segment-by-segment, interleaving diarization and downstream emission. There
/// is intentionally no self-owned receive loop — `transcribe_segment` is called
/// directly from the pipeline's own loop.
// `config` is only read by the whisper-gated transcribe_segment.
#[cfg_attr(not(feature = "asr-whisper"), allow(dead_code))]
pub struct AsrWorker {
    config: AsrConfig,
    segments_processed: u64,
}

impl AsrWorker {
    /// Create a new ASR worker with the given config. The pipeline
    /// (`speech/mod.rs`) drives transcription by calling
    /// [`Self::transcribe_segment`] directly and routes the returned segments
    /// itself — there is no self-owned output channel (FA-6b dropped the
    /// vestigial one left over from the removed `run()` loop).
    pub fn new(config: AsrConfig) -> Self {
        Self {
            config,
            segments_processed: 0,
        }
    }

    /// Transcribe a single speech segment into zero or more transcript segments.
    ///
    /// Configures Whisper parameters, runs inference, then extracts and filters
    /// the resulting segments. Whisper timestamps (in centiseconds) are converted
    /// to absolute seconds by adding the speech segment's `start_time` offset.
    #[cfg(feature = "asr-whisper")]
    pub fn transcribe_segment(
        &mut self,
        state: &mut whisper_rs::WhisperState,
        segment: &SpeechSegment,
    ) -> Result<Vec<TranscriptSegment>, String> {
        // ── Configure Whisper params ────────────────────────────────────
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(&self.config.language));
        params.set_print_progress(false);
        params.set_print_timestamps(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_single_segment(false);
        params.set_no_context(true);
        params.set_n_threads(self.config.n_threads);
        params.set_temperature(self.config.temperature);

        // ── Run inference ───────────────────────────────────────────────
        state
            .full(params, &segment.audio)
            .map_err(|e| format!("Whisper inference failed: {}", e))?;

        // ── Extract results ─────────────────────────────────────────────
        let num_segments = state.full_n_segments();

        let mut transcripts = Vec::new();

        for i in 0..num_segments {
            let whisper_seg = match state.get_segment(i) {
                Some(s) => s,
                None => continue,
            };

            let text = whisper_seg
                .to_str()
                .map_err(|e| format!("Failed to get segment text: {}", e))?;

            let text = text.trim().to_string();
            if text.is_empty() {
                continue;
            }

            // Whisper returns timestamps in centiseconds (1/100th of a second)
            let t0 = whisper_seg.start_timestamp();
            let t1 = whisper_seg.end_timestamp();

            // Convert whisper timestamps (centiseconds) to absolute seconds
            // by adding the speech segment's start-time offset.
            let segment_start_secs = segment.start_time.as_secs_f64();
            let start_time = segment_start_secs + (t0 as f64 / 100.0);
            let end_time = segment_start_secs + (t1 as f64 / 100.0);

            // Use (1.0 - no_speech_probability) as a rough confidence proxy
            let confidence = 1.0 - whisper_seg.no_speech_probability();

            self.segments_processed += 1;

            let transcript = TranscriptSegment {
                id: Uuid::new_v4().to_string(),
                source_id: segment.source_id.clone(),
                speaker_id: None,    // filled by diarization later
                speaker_label: None, // filled by diarization later
                text: text.clone(),
                start_time,
                end_time,
                confidence,
            };

            debug!(
                "ASR segment metadata count={} source_id={} start={:.2}s end={:.2}s conf={:.2} text_len={}",
                self.segments_processed,
                segment.source_id,
                start_time,
                end_time,
                confidence,
                text.chars().count()
            );

            transcripts.push(transcript);
        }

        Ok(transcripts)
    }

    /// Returns the total number of transcript segments produced so far.
    pub fn segments_processed(&self) -> u64 {
        self.segments_processed
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_models_dir_joins_default_small_en_model() {
        let cfg = AsrConfig::with_models_dir(Path::new("/opt/models"));
        assert_eq!(
            cfg.model_path,
            PathBuf::from("/opt/models").join("ggml-small.en.bin")
        );
        assert_eq!(cfg.language, "en");
        assert_eq!(cfg.n_threads, 4);
        assert!((cfg.temperature - 0.0).abs() < f32::EPSILON);
        assert_eq!(cfg.beam_size, 5);
    }

    #[test]
    fn with_models_dir_and_model_joins_given_filename() {
        let cfg =
            AsrConfig::with_models_dir_and_model(Path::new("/opt/models"), "ggml-medium.en.bin");
        assert_eq!(
            cfg.model_path,
            PathBuf::from("/opt/models").join("ggml-medium.en.bin")
        );
        // Other fields keep their defaults.
        assert_eq!(cfg.language, "en");
        assert_eq!(cfg.n_threads, 4);
        assert_eq!(cfg.beam_size, 5);
    }

    #[test]
    fn default_config_matches_documented_values() {
        let cfg = AsrConfig::default();
        assert_eq!(cfg.model_path, PathBuf::from("models/ggml-small.en.bin"));
        assert_eq!(cfg.language, "en");
        assert_eq!(cfg.n_threads, 4);
        assert!((cfg.temperature - 0.0).abs() < f32::EPSILON);
        assert_eq!(cfg.beam_size, 5);
    }

    #[test]
    fn speech_segment_num_frames_equals_audio_len_invariant() {
        let audio = vec![0.0_f32; 32_000]; // 2s @ 16kHz
        let seg = SpeechSegment {
            source_id: "src-1".to_string(),
            audio: audio.clone(),
            start_time: Duration::from_secs(0),
            end_time: Duration::from_secs(2),
            num_frames: audio.len(),
        };
        // The documented invariant: num_frames == audio.len().
        assert_eq!(seg.num_frames, seg.audio.len());
    }

    #[test]
    fn new_worker_starts_with_zero_segments_processed() {
        let worker = AsrWorker::new(AsrConfig::default());
        assert_eq!(worker.segments_processed(), 0);
    }
}
