//! Minimal Server-Sent Events parser for OpenAI-compatible streaming chat.
//!
//! The OpenAI-compat streaming protocol (used by OpenAI, OpenRouter, Ollama,
//! vLLM, LM Studio, Anthropic-via-proxy, etc.) transports each token-delta
//! frame as a single SSE event of the form:
//!
//! ```text
//! data: {"choices":[{"delta":{"content":"Hel"}}]}
//!
//! data: {"choices":[{"delta":{"content":"lo"}}]}
//!
//! data: [DONE]
//!
//! ```
//!
//! Frames are separated by a blank line (i.e. two consecutive `\n` once
//! `\r` is stripped). Each frame may have multiple `field: value` lines; we
//! only care about the `data:` field for OpenAI-style streams. We tolerate
//! `\r\n` line endings, leading-space convention (`data: foo` and `data:foo`
//! are both valid), and comment lines starting with `:`.
//!
//! This is intentionally a tiny dependency-free parser (~80 LOC) so we don't
//! have to pull in `eventsource-stream` for one consumer. It is exercised by
//! tests at the bottom of the file and by `api_client::chat_completion_stream`
//! / `openrouter::chat_completion_stream`.
//!
//! Out of scope:
//! - `event:` field routing (OpenAI uses `data:`-only frames).
//! - `id:` / `retry:` fields (we don't need reconnection-with-id; if the
//!   upstream connection drops mid-stream we surface that as a streaming
//!   error and let the caller decide whether to retry from scratch).
//! - UTF-8 boundary handling within a single chunk: `Bytes::extend_from_slice`
//!   keeps us at byte level until we hit `\n\n`, at which point we decode
//!   the whole frame as UTF-8. JSON token deltas are always whole UTF-8
//!   strings so we don't split graphemes.

use audio_graph_ipc_contract::runtime_diagnostic::{
    RuntimeDiagnostic, RuntimeErrorCode as Code, RuntimeErrorDiagnostic,
};

use crate::llm::http_diag::{INFERENCE, diagnostic};
pub use crate::llm::stream_contract::StreamUsage;

/// Default maximum size for one buffered SSE frame before the decoder reports
/// an error and discards the frame. OpenAI-compatible token deltas are tiny;
/// 1 MiB leaves generous room for provider metadata while bounding hostile or
/// broken streams that never emit a blank-line terminator.
pub const DEFAULT_MAX_SSE_FRAME_BYTES: usize = 1024 * 1024;

/// Stateful incremental SSE frame parser.
///
/// Feed bytes via [`SseDecoder::feed`]; pull complete frames out via
/// [`SseDecoder::next_event`]. Any partial frame stays in the internal buffer
/// until enough bytes arrive to terminate it with a blank line.
pub struct SseDecoder {
    buf: Vec<u8>,
    max_frame_bytes: usize,
}

impl Default for SseDecoder {
    fn default() -> Self {
        Self::with_max_frame_bytes(DEFAULT_MAX_SSE_FRAME_BYTES)
    }
}

/// One decoded SSE frame.
///
/// `data` is the concatenation of every `data:` line in the frame (the SSE
/// spec says multiple `data:` lines are joined by `\n`, but OpenAI-style
/// streams always use a single line per frame). `[DONE]` is reported as a
/// terminator via [`SseEvent::Done`] so the consumer can stop the stream
/// loop cleanly.
#[derive(Debug, PartialEq, Eq)]
pub enum SseEvent {
    /// A `data:` frame whose body is not the literal `[DONE]` sentinel.
    Data(String),
    /// The `data: [DONE]` terminator. After this, no further frames will
    /// arrive on a well-behaved stream.
    Done,
    /// A provider fault or an invalid frame. Never carries provider content.
    Error(RuntimeDiagnostic),
}

impl SseDecoder {
    /// Construct an empty decoder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Construct an empty decoder with an explicit maximum frame size.
    pub fn with_max_frame_bytes(max_frame_bytes: usize) -> Self {
        Self {
            buf: Vec::new(),
            max_frame_bytes: max_frame_bytes.max(1),
        }
    }

    /// Append a chunk of bytes received from the HTTP stream.
    pub fn feed(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// Try to extract the next complete frame from the internal buffer.
    ///
    /// Returns `None` when the buffer doesn't yet contain a full frame
    /// (i.e. no blank-line terminator). Call again after [`SseDecoder::feed`]
    /// has appended more bytes.
    pub fn next_event(&mut self) -> Option<SseEvent> {
        // A frame ends at the first blank line. We accept both `\n\n`
        // (canonical) and `\r\n\r\n` (CRLF flavor) — pick whichever shows
        // up first.
        let nn = find_subseq(&self.buf, b"\n\n");
        let crnn = find_subseq(&self.buf, b"\r\n\r\n");
        let (idx, sep_len) = match (nn, crnn) {
            (Some(a), Some(b)) if a <= b => (a, 2),
            (Some(a), None) => (a, 2),
            (_, Some(b)) => (b, 4),
            (None, None) => {
                if self.buf.len() > self.max_frame_bytes {
                    self.buf.clear();
                    return Some(SseEvent::Error(diagnostic(Code::ProtocolError, INFERENCE)));
                }
                return None;
            }
        };

        if idx > self.max_frame_bytes {
            self.buf.drain(..idx + sep_len);
            return Some(SseEvent::Error(diagnostic(Code::ProtocolError, INFERENCE)));
        }

        let frame_bytes: Vec<u8> = self.buf.drain(..idx + sep_len).collect();
        // Strip the trailing terminator (we don't need it for parsing).
        let frame_end = frame_bytes.len().saturating_sub(sep_len);
        let frame = &frame_bytes[..frame_end];

        // Decode lossy: a malformed UTF-8 byte in a `data:` payload would
        // already be a server bug, but we don't want one bad byte to wedge
        // the entire stream loop.
        let frame_str = String::from_utf8_lossy(frame);

        let mut data = String::new();
        let mut have_data = false;
        let mut is_error_event = false;
        for line in frame_str.split('\n') {
            // SSE spec: lines starting with `:` are comments. OpenAI sends
            // `: OPENROUTER PROCESSING` / similar keepalives; ignore them.
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with(':') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("data:") {
                let value = rest.strip_prefix(' ').unwrap_or(rest);
                if have_data {
                    data.push('\n');
                }
                data.push_str(value);
                have_data = true;
            } else if let Some(rest) = line.strip_prefix("event:") {
                // Track the `event:` field so a provider's mid-stream
                // `event: error` frame is surfaced instead of dropped. OpenAI's
                // own `/chat/completions` stream is `data:`-only, but several
                // OpenAI-compatible gateways (and SSE in general) use a named
                // `error` event to report a fault that occurs after the 200
                // response headers were sent.
                is_error_event = rest.trim() == "error";
            }
            // Other field names (id:, retry:) are intentionally ignored.
        }

        if is_error_event {
            // The payload may be a bare error object or an envelope; either
            // way only its structured fields are read.
            let code = serde_json::from_str::<serde_json::Value>(&data)
                .map_or(Code::Internal, |frame| fault_code(&frame));
            return Some(SseEvent::Error(diagnostic(code, INFERENCE)));
        }

        if !have_data {
            // A frame with no `data:` line is a heartbeat / keepalive — try
            // the next one. Recurse-by-loop to avoid blowing the stack on
            // burst keepalives.
            return self.next_event();
        }

        if data.trim() == "[DONE]" {
            return Some(SseEvent::Done);
        }

        // Some providers report a mid-stream fault on a normal `data:` frame
        // whose JSON body is an error envelope (`{"error": {...}}`) instead of
        // the `event: error` mechanism above. Detect that shape and surface it
        // as an error so it isn't deserialized into an empty `StreamChunk` and
        // dropped. A normal token chunk has no top-level `error` field, so this
        // never misclassifies real deltas.
        if let Some(code) = fault_in_data(&data) {
            return Some(SseEvent::Error(diagnostic(code, INFERENCE)));
        }

        Some(SseEvent::Data(data))
    }
}

/// `Some` when a `data:` frame is a provider fault rather than a token chunk.
/// A healthy chunk may carry `"error": null` or `"error": false`.
fn fault_in_data(data: &str) -> Option<Code> {
    let frame: serde_json::Value = serde_json::from_str(data).ok()?;
    let has_error = frame
        .get("error")
        .is_some_and(|error| !error.is_null() && error.as_bool() != Some(false));
    let is_error = frame.get("type").and_then(|kind| kind.as_str()) == Some("error");
    (has_error || is_error).then(|| fault_code(&frame))
}

/// Classify a provider fault from its structured code fields. The message is
/// never read: a fault with no recognized code is `internal`, not a guess from
/// prose and not an outage worth retrying.
fn fault_code(frame: &serde_json::Value) -> Code {
    let error = frame.get("error").filter(|error| error.is_object());
    [
        error.and_then(|error| error.get("code")),
        error.and_then(|error| error.get("status")),
        error.and_then(|error| error.get("type")),
        frame.get("code"),
        frame.get("status"),
        frame.get("type"),
    ]
    .into_iter()
    .flatten()
    .find_map(structured_code)
    .unwrap_or(Code::Internal)
}

/// An HTTP status (number or numeric string) or an allowlisted provider token.
fn structured_code(value: &serde_json::Value) -> Option<Code> {
    if let Some(status) = value.as_u64() {
        return u16::try_from(status).ok().and_then(status_category);
    }
    let token = value.as_str()?;
    if let Ok(status) = token.parse() {
        return status_category(status);
    }
    Some(match token.to_ascii_lowercase().as_str() {
        "rate_limit"
        | "rate_limited"
        | "rate_limit_error"
        | "rate_limit_exceeded"
        | "too_many_requests"
        | "insufficient_quota" => Code::RateLimited,
        "overloaded"
        | "overloaded_error"
        | "server_error"
        | "api_error"
        | "internal_server_error"
        | "service_unavailable" => Code::ProviderUnavailable,
        "timeout" | "timeout_error" | "request_timeout" | "gateway_timeout" => Code::Timeout,
        "invalid_request"
        | "invalid_request_error"
        | "context_length_exceeded"
        | "model_not_found"
        | "not_found_error"
        | "request_too_large" => Code::RequestRejected,
        _ => return None,
    })
}

/// The contract's category for an HTTP failure status a provider reported in a
/// structured field. Nothing here says a credential was sent, so a 401 is an
/// ordinary rejection.
fn status_category(status: u16) -> Option<Code> {
    (400..=599)
        .contains(&status)
        .then(|| RuntimeErrorDiagnostic::from_http_status(status, false, INFERENCE).code)
}

/// Find the first occurrence of `needle` in `haystack`. Linear scan; for the
/// frame separators we look for (2- or 4-byte) the simple search beats
/// pulling in a string-search crate.
fn find_subseq(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

// ---------------------------------------------------------------------------
// OpenAI streaming chunk shape
// ---------------------------------------------------------------------------

/// A streaming chat-completion chunk. Mirrors the OpenAI-compat shape:
///
/// ```json
/// {
///   "choices": [
///     {
///       "delta": { "content": "Hello" },
///       "finish_reason": null
///     }
///   ],
///   "usage": null
/// }
/// ```
///
/// Only the fields the streaming chat path actually consumes are deserialized.
#[derive(Debug, serde::Deserialize)]
pub struct StreamChunk {
    #[serde(default)]
    pub choices: Vec<StreamChoice>,
    #[serde(default)]
    pub usage: Option<StreamUsage>,
}

/// One `choices[i]` slot inside a [`StreamChunk`].
#[derive(Debug, serde::Deserialize)]
pub struct StreamChoice {
    #[serde(default)]
    pub delta: StreamDelta,
    /// `"stop"`, `"length"`, `"content_filter"`, etc. Only populated on the
    /// final chunk before `data: [DONE]`.
    #[serde(default)]
    pub finish_reason: Option<String>,
}

/// The token delta carried by a [`StreamChoice`].
#[derive(Debug, Default, serde::Deserialize)]
pub struct StreamDelta {
    #[serde(default)]
    pub content: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_single_frame() {
        let mut dec = SseDecoder::new();
        dec.feed(b"data: hello\n\n");
        assert_eq!(dec.next_event(), Some(SseEvent::Data("hello".to_string())));
        assert_eq!(dec.next_event(), None);
    }

    #[test]
    fn decodes_multiple_frames_in_one_chunk() {
        let mut dec = SseDecoder::new();
        dec.feed(b"data: a\n\ndata: b\n\ndata: [DONE]\n\n");
        assert_eq!(dec.next_event(), Some(SseEvent::Data("a".to_string())));
        assert_eq!(dec.next_event(), Some(SseEvent::Data("b".to_string())));
        assert_eq!(dec.next_event(), Some(SseEvent::Done));
        assert_eq!(dec.next_event(), None);
    }

    #[test]
    fn handles_chunked_frame_split_across_feeds() {
        let mut dec = SseDecoder::new();
        dec.feed(b"data: par");
        assert_eq!(dec.next_event(), None);
        dec.feed(b"tial\n\n");
        assert_eq!(
            dec.next_event(),
            Some(SseEvent::Data("partial".to_string()))
        );
    }

    /// Worst-case streaming: every byte of a multi-frame stream arrives in
    /// its own `feed()` call. Flushes any false-positive in `find_subseq`
    /// where the `\n\n` terminator straddles a one-byte chunk boundary.
    /// See A3 reviewer finding (2026-05-20).
    #[test]
    fn handles_byte_by_byte_streaming() {
        let mut dec = SseDecoder::new();
        let stream = b"data: a\n\ndata: b\n\ndata: [DONE]\n\n";
        let mut events: Vec<SseEvent> = Vec::new();
        for byte in stream.iter() {
            dec.feed(&[*byte]);
            while let Some(evt) = dec.next_event() {
                events.push(evt);
            }
        }
        assert_eq!(events.len(), 3, "expected 3 events, got {events:?}");
        assert_eq!(events[0], SseEvent::Data("a".to_string()));
        assert_eq!(events[1], SseEvent::Data("b".to_string()));
        assert_eq!(events[2], SseEvent::Done);
    }

    #[test]
    fn skips_comment_keepalives() {
        let mut dec = SseDecoder::new();
        dec.feed(b": OPENROUTER PROCESSING\n\ndata: hi\n\n");
        assert_eq!(dec.next_event(), Some(SseEvent::Data("hi".to_string())));
    }

    #[test]
    fn handles_crlf_line_endings() {
        let mut dec = SseDecoder::new();
        dec.feed(b"data: hello\r\n\r\ndata: [DONE]\r\n\r\n");
        assert_eq!(dec.next_event(), Some(SseEvent::Data("hello".to_string())));
        assert_eq!(dec.next_event(), Some(SseEvent::Done));
    }

    #[test]
    fn unterminated_frame_over_cap_errors_and_recovers() {
        let mut dec = SseDecoder::with_max_frame_bytes(8);
        dec.feed(b"data: this frame never terminates");

        assert_eq!(
            dec.next_event(),
            Some(SseEvent::Error(diagnostic(Code::ProtocolError, INFERENCE)))
        );
        assert_eq!(dec.next_event(), None);

        dec.feed(b"data: ok\n\n");
        assert_eq!(dec.next_event(), Some(SseEvent::Data("ok".to_string())));
    }

    #[test]
    fn oversized_complete_frame_errors_and_preserves_following_frame() {
        let mut dec = SseDecoder::with_max_frame_bytes(8);
        dec.feed(b"data: 123456789\n\ndata: ok\n\n");

        assert_eq!(
            dec.next_event(),
            Some(SseEvent::Error(diagnostic(Code::ProtocolError, INFERENCE)))
        );
        assert_eq!(dec.next_event(), Some(SseEvent::Data("ok".to_string())));
    }

    #[test]
    fn parses_openai_stream_chunk_shape() {
        let raw = r#"{"choices":[{"delta":{"content":"Hel"},"finish_reason":null}],"usage":null}"#;
        let chunk: StreamChunk = serde_json::from_str(raw).expect("parse chunk");
        assert_eq!(chunk.choices.len(), 1);
        assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hel"));
        assert!(chunk.choices[0].finish_reason.is_none());
    }

    #[test]
    fn parses_terminal_chunk_with_usage() {
        let raw = r#"{"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":12,"completion_tokens":34,"total_tokens":46}}"#;
        let chunk: StreamChunk = serde_json::from_str(raw).expect("parse terminal chunk");
        assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("stop"));
        let usage = chunk.usage.expect("usage on terminal chunk");
        assert_eq!(usage.total_tokens, Some(46));
    }

    #[test]
    fn named_error_event_is_classified_by_structured_type_only() {
        // BUG 4a49: a provider that reports a mid-stream fault via a named
        // `event: error` SSE frame must reach SseEvent::Error, not be dropped.
        let mut dec = SseDecoder::new();
        dec.feed(
            b"event: error\ndata: {\"error\":{\"message\":\"sse-message-canary\",\"type\":\"rate_limit\"}}\n\n",
        );
        assert_eq!(
            dec.next_event(),
            Some(SseEvent::Error(diagnostic(Code::RateLimited, INFERENCE)))
        );
    }

    #[test]
    fn structured_fault_codes_classify_without_reading_the_message() {
        // Each message contradicts its code: only the code may decide.
        for (frame, code) in [
            (
                r#"{"error":{"code":429,"message":"server exploded"}}"#,
                Code::RateLimited,
            ),
            (
                r#"{"error":{"code":403,"message":"rate limit"}}"#,
                Code::PermissionDenied,
            ),
            (
                r#"{"error":{"code":401,"message":"rate limit"}}"#,
                Code::RequestRejected,
            ),
            (
                r#"{"error":{"code":"429","message":"timed out"}}"#,
                Code::RateLimited,
            ),
            (
                r#"{"error":{"status":503,"message":"rate limit"}}"#,
                Code::ProviderUnavailable,
            ),
            (
                r#"{"error":{"code":"rate_limit_exceeded","message":"x"}}"#,
                Code::RateLimited,
            ),
            (
                r#"{"type":"error","error":{"type":"overloaded_error","message":"rate limit"}}"#,
                Code::ProviderUnavailable,
            ),
            (
                r#"{"error":{"type":"invalid_request_error","message":"429"}}"#,
                Code::RequestRejected,
            ),
            (
                r#"{"type":"error","code":408,"message":"rate limit"}"#,
                Code::Timeout,
            ),
            (
                r#"{"error":{"code":"a-code-this-app-does-not-know","message":"rate limit 429"}}"#,
                Code::Internal,
            ),
            (
                r#"{"error":{"code":200,"message":"rate limit"}}"#,
                Code::Internal,
            ),
            (r#"{"error":"rate limit exceeded (429)"}"#, Code::Internal),
            (
                r#"{"type":"error","message":"too many requests"}"#,
                Code::Internal,
            ),
        ] {
            assert_eq!(fault_in_data(frame), Some(code), "{frame}");
        }
    }

    #[test]
    fn error_envelope_on_plain_data_frame_surfaces_as_error() {
        // Some gateways report the fault on an ordinary `data:` frame whose
        // body is an error envelope rather than using `event: error`. It must
        // still surface as an error instead of parsing to an empty chunk.
        let mut dec = SseDecoder::new();
        dec.feed(b"data: {\"error\":{\"message\":\"context length exceeded\"}}\n\n");
        assert_eq!(
            dec.next_event(),
            Some(SseEvent::Error(diagnostic(Code::Internal, INFERENCE)))
        );
    }

    #[test]
    fn error_event_without_payload_surfaces_generic_error() {
        let mut dec = SseDecoder::new();
        dec.feed(b"event: error\n\n");
        assert_eq!(
            dec.next_event(),
            Some(SseEvent::Error(diagnostic(Code::Internal, INFERENCE)))
        );
    }

    #[test]
    fn normal_delta_chunk_is_not_misclassified_as_error() {
        // Guard: a normal token-delta chunk has no top-level `error`/error-type
        // field and must remain a Data event so the fix can't break streaming.
        let mut dec = SseDecoder::new();
        dec.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}]}\n\n");
        match dec.next_event() {
            Some(SseEvent::Data(payload)) => {
                let chunk: StreamChunk = serde_json::from_str(&payload).expect("parse chunk");
                assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello"));
            }
            other => panic!("expected SseEvent::Data, got {other:?}"),
        }
    }

    #[test]
    fn explicit_null_error_field_is_not_misclassified() {
        // BUG 4a72: many OpenAI-compatible providers send a healthy chunk that
        // carries an explicit `"error": null` ("no error" signal) alongside the
        // real `choices`. The decoder must NOT treat the null `error` key as a
        // fault — doing so produced `stream error: null` and dropped the whole
        // partial response. The chunk must surface as Data and parse normally.
        let mut dec = SseDecoder::new();
        dec.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}],\"error\":null}\n\n");
        match dec.next_event() {
            Some(SseEvent::Data(payload)) => {
                let chunk: StreamChunk = serde_json::from_str(&payload).expect("parse chunk");
                assert_eq!(chunk.choices[0].delta.content.as_deref(), Some("Hello"));
            }
            other => panic!("expected SseEvent::Data, got {other:?}"),
        }
    }

    #[test]
    fn explicit_false_error_field_is_not_misclassified() {
        // Sibling of 4a72: some gateways use `"error": false` as the "no error"
        // sentinel. It must also fall through to Data, not fabricate an error.
        assert_eq!(
            fault_in_data(r#"{"choices":[{"delta":{"content":"x"}}],"error":false}"#),
            None
        );
    }

    #[test]
    fn error_after_deltas_reaches_error_event_mid_stream() {
        // Deltas flow, then a mid-stream error envelope must surface as Error
        // rather than being dropped — the core 4a49 regression.
        let mut dec = SseDecoder::new();
        dec.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n");
        assert!(matches!(dec.next_event(), Some(SseEvent::Data(_))));
        dec.feed(b"event: error\ndata: {\"error\":\"upstream timeout\"}\n\n");
        assert_eq!(
            dec.next_event(),
            Some(SseEvent::Error(diagnostic(Code::Internal, INFERENCE)))
        );
    }

    #[test]
    fn data_field_strips_optional_leading_space() {
        // SSE spec: `data: foo` and `data:foo` are equivalent.
        let mut dec = SseDecoder::new();
        dec.feed(b"data:no-space\n\ndata: with-space\n\n");
        assert_eq!(
            dec.next_event(),
            Some(SseEvent::Data("no-space".to_string()))
        );
        assert_eq!(
            dec.next_event(),
            Some(SseEvent::Data("with-space".to_string()))
        );
    }
}
