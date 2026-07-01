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
    /// The decoder discarded an invalid frame or partial frame.
    Error(String),
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
                    return Some(SseEvent::Error(format!(
                        "SSE frame exceeded {} bytes without a terminator",
                        self.max_frame_bytes
                    )));
                }
                return None;
            }
        };

        if idx > self.max_frame_bytes {
            self.buf.drain(..idx + sep_len);
            return Some(SseEvent::Error(format!(
                "SSE frame exceeded {} bytes",
                self.max_frame_bytes
            )));
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
        let mut event_name: Option<String> = None;
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
                event_name = Some(rest.strip_prefix(' ').unwrap_or(rest).trim().to_string());
            }
            // Other field names (id:, retry:) are intentionally ignored.
        }

        // A named `error` event carries a fault that must reach the consumer as
        // an `SseEvent::Error` (which the caller redacts before display) rather
        // than being parsed as a token chunk and silently dropped.
        if event_name.as_deref() == Some("error") {
            return Some(SseEvent::Error(error_message_from_frame(
                have_data.then_some(data.as_str()),
            )));
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
        if let Some(message) = error_message_from_data(&data) {
            return Some(SseEvent::Error(message));
        }

        Some(SseEvent::Data(data))
    }
}

/// Build an error message for a named `event: error` frame, preferring a
/// structured `error.message` / `message` field from the `data:` payload and
/// falling back to the raw payload (or a generic note when the frame had none).
fn error_message_from_frame(data: Option<&str>) -> String {
    match data {
        Some(payload) => error_message_from_data(payload)
            .unwrap_or_else(|| format!("stream error event: {}", payload.trim())),
        None => "stream error event (no payload)".to_string(),
    }
}

/// If `data` is a JSON object carrying an error envelope, extract a concise
/// human-readable message. Returns `None` for any payload that is not an error
/// envelope (e.g. a normal token-delta chunk), so callers can fall through to
/// the regular `Data` path.
///
/// Recognized shapes (OpenAI-compatible providers vary):
/// - `{"error": {"message": "..."}}`
/// - `{"error": "..."}`
/// - `{"message": "...", "type": "error"}` (some gateways)
///
/// A `null`/`false` value at the `error` key (e.g. the healthy
/// `{"choices":[...],"error":null}` chunk many providers send) is treated as
/// "no error" and falls through to `None`.
fn error_message_from_data(data: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(data.trim()).ok()?;
    let obj = value.as_object()?;

    if let Some(error) = obj.get("error") {
        // A healthy chunk often carries `"error": null` (or `false`) as an
        // explicit "no error" signal: `{"choices":[...],"error":null}`. Such a
        // value is NOT an error — treat it as absent and fall through to the
        // regular `Data` path instead of fabricating `stream error: null`.
        if error.is_null() || error == &serde_json::Value::Bool(false) {
            // fall through
        } else if let Some(message) = error.get("message").and_then(|m| m.as_str()) {
            return Some(message.to_string());
        } else if let Some(message) = error.as_str() {
            return Some(message.to_string());
        } else {
            // An `error` field that isn't a string or message-bearing object is
            // still an error signal; surface its compact JSON form.
            return Some(format!("stream error: {error}"));
        }
    }

    // `{"type":"error", "message": "..."}` style without a nested `error`.
    if obj.get("type").and_then(|t| t.as_str()) == Some("error") {
        let message = obj
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("provider reported an error");
        return Some(message.to_string());
    }

    None
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

        match dec.next_event() {
            Some(SseEvent::Error(message)) => {
                assert!(message.contains("without a terminator"));
            }
            other => panic!("expected overflow error, got {other:?}"),
        }
        assert_eq!(dec.next_event(), None);

        dec.feed(b"data: ok\n\n");
        assert_eq!(dec.next_event(), Some(SseEvent::Data("ok".to_string())));
    }

    #[test]
    fn oversized_complete_frame_errors_and_preserves_following_frame() {
        let mut dec = SseDecoder::with_max_frame_bytes(8);
        dec.feed(b"data: 123456789\n\ndata: ok\n\n");

        match dec.next_event() {
            Some(SseEvent::Error(message)) => {
                assert!(message.contains("SSE frame exceeded"));
            }
            other => panic!("expected overflow error, got {other:?}"),
        }
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
    fn named_error_event_surfaces_as_error_with_message() {
        // BUG 4a49: a provider that reports a mid-stream fault via a named
        // `event: error` SSE frame must reach SseEvent::Error, not be dropped.
        let mut dec = SseDecoder::new();
        dec.feed(
            b"event: error\ndata: {\"error\":{\"message\":\"rate limit exceeded\",\"type\":\"rate_limit\"}}\n\n",
        );
        match dec.next_event() {
            Some(SseEvent::Error(message)) => {
                assert!(
                    message.contains("rate limit exceeded"),
                    "expected the provider message, got: {message}"
                );
            }
            other => panic!("expected SseEvent::Error, got {other:?}"),
        }
    }

    #[test]
    fn error_envelope_on_plain_data_frame_surfaces_as_error() {
        // Some gateways report the fault on an ordinary `data:` frame whose
        // body is an error envelope rather than using `event: error`. It must
        // still surface as an error instead of parsing to an empty chunk.
        let mut dec = SseDecoder::new();
        dec.feed(b"data: {\"error\":{\"message\":\"context length exceeded\"}}\n\n");
        match dec.next_event() {
            Some(SseEvent::Error(message)) => {
                assert!(message.contains("context length exceeded"));
            }
            other => panic!("expected SseEvent::Error, got {other:?}"),
        }
    }

    #[test]
    fn error_event_without_payload_surfaces_generic_error() {
        let mut dec = SseDecoder::new();
        dec.feed(b"event: error\n\n");
        match dec.next_event() {
            Some(SseEvent::Error(message)) => {
                assert!(message.contains("error event"));
            }
            other => panic!("expected SseEvent::Error, got {other:?}"),
        }
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
            error_message_from_data(r#"{"choices":[{"delta":{"content":"x"}}],"error":false}"#),
            None,
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
        match dec.next_event() {
            Some(SseEvent::Error(message)) => assert!(message.contains("upstream timeout")),
            other => panic!("expected mid-stream SseEvent::Error, got {other:?}"),
        }
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
