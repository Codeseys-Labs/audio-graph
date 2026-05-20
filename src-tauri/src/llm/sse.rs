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

/// Stateful incremental SSE frame parser.
///
/// Feed bytes via [`SseDecoder::feed`]; pull complete frames out via
/// [`SseDecoder::next_event`]. Any partial frame stays in the internal buffer
/// until enough bytes arrive to terminate it with a blank line.
#[derive(Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
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
}

impl SseDecoder {
    /// Construct an empty decoder.
    pub fn new() -> Self {
        Self::default()
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
            (None, None) => return None,
        };

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
            }
            // Other field names (event:, id:, retry:) are intentionally
            // ignored. OpenAI-style streams never emit them.
        }

        if !have_data {
            // A frame with no `data:` line is a heartbeat / keepalive — try
            // the next one. Recurse-by-loop to avoid blowing the stack on
            // burst keepalives.
            return self.next_event();
        }

        if data.trim() == "[DONE]" {
            Some(SseEvent::Done)
        } else {
            Some(SseEvent::Data(data))
        }
    }
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

/// Token-usage block sent on the terminal chunk when
/// `stream_options.include_usage` is `true`.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct StreamUsage {
    #[serde(default)]
    pub prompt_tokens: Option<u32>,
    #[serde(default)]
    pub completion_tokens: Option<u32>,
    #[serde(default)]
    pub total_tokens: Option<u32>,
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
