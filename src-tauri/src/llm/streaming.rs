//! Streaming chat dispatch for OpenAI-compatible providers (plan A3 / ADR-0006).
//!
//! Today this module covers the two providers whose wire format is OpenAI
//! Chat Completions over SSE:
//!
//! - [`LlmProvider::Api`]          — generic OpenAI-compatible endpoints
//!   (Ollama, LM Studio, vLLM, OpenAI itself when configured as a generic Api).
//! - [`LlmProvider::OpenRouter`]   — first-class OpenRouter (ADR-0005) with
//!   attribution headers + provider-routing passthrough.
//!
//! `LocalLlama`, `MistralRs`, and `AwsBedrock` streaming are deferred to a
//! follow-up issue. When the active provider falls into one of those
//! variants, [`stream_chat`] currently returns
//! `Err("streaming not yet supported for provider …")` so the caller can
//! decide whether to fall back to the legacy blocking executor or surface
//! the limitation to the user. Callers in this crate (the streaming-chat
//! Tauri command) treat that as a hard error today; the
//! `send_chat_message` shim degrades by short-circuiting to the blocking
//! executor.
//!
//! Wire shape: see `crate::llm::sse` for the SSE chunk parser and the
//! OpenAI-compat `StreamChunk` deserialization shape that both providers
//! emit.

use std::sync::Arc;

use bytes::Bytes;
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::llm::engine::ChatMessage;
use crate::llm::openrouter::{OpenRouterConfig, DEFAULT_APP_TITLE, DEFAULT_HTTP_REFERER};
use crate::llm::sse::{SseDecoder, SseEvent, StreamChunk, StreamUsage};
use crate::settings::LlmProvider;

/// One incremental update from the streaming-chat task.
///
/// Producer (this module): emits one `Delta` per `data:` chunk that contains
/// non-empty `choices[0].delta.content`, then exactly one terminal
/// `Done`/`Error`/`Cancelled`.
///
/// Consumer (the Tauri command in `commands.rs`): translates each variant
/// into a `chat-token-delta` or `chat-token-done` event for the frontend.
#[derive(Debug, Clone)]
pub enum TokenDelta {
    /// One token (or chunk of tokens) of generated content.
    Delta {
        content: String,
        finish_reason: Option<String>,
    },
    /// Final terminator on success. `usage` is populated when the provider
    /// honoured `stream_options.include_usage`.
    Done {
        full_text: String,
        usage: Option<StreamUsage>,
    },
    /// Stream errored mid-flight (network drop, HTTP non-2xx, malformed
    /// JSON, etc.). `full_text` is whatever was accumulated before the
    /// error so the caller can show a partial reply rather than nothing.
    Error { message: String, full_text: String },
    /// Caller invoked the cancel token. The stream task drops the HTTP
    /// connection and emits this as its terminal frame so the consumer
    /// can finalize the chat message with `finish_reason: "cancelled"`.
    Cancelled { full_text: String },
}

/// Configuration for a single streaming chat request.
///
/// The active provider is materialized into an HTTP request shape here
/// rather than passing the full `LlmProvider` enum down through the SSE
/// loop, so the loop itself stays provider-agnostic.
struct StreamRequest {
    url: String,
    headers: Vec<(String, String)>,
    body: serde_json::Value,
}

/// Build an OpenAI-style chat-completion `messages` array from the chat
/// history + a synthesized system prompt that injects the graph context.
///
/// Mirrors the prompt shape used by `ApiClient::chat_with_history` and
/// `OpenRouterClient::chat_with_history` so the streaming and blocking
/// paths produce comparable replies.
fn build_messages(history: &[ChatMessage], graph_context: &str) -> Vec<serde_json::Value> {
    let system_prompt = format!(
        "You are a knowledge graph assistant analyzing a live audio conversation. \
         Here is the current knowledge graph context:\n\n{}\n\n\
         Answer the user's question about the conversation, people, topics, or relationships discussed.",
        graph_context
    );

    let mut messages = Vec::with_capacity(history.len() + 1);
    messages.push(serde_json::json!({
        "role": "system",
        "content": system_prompt,
    }));
    for msg in history {
        messages.push(serde_json::json!({
            "role": msg.role,
            "content": msg.content,
        }));
    }
    messages
}

/// Build the wire-shape request for a generic OpenAI-compatible provider
/// (`LlmProvider::Api`). The endpoint URL must already be validated upstream
/// (see `commands::validate_endpoint_url`).
fn build_api_request(
    endpoint: &str,
    api_key: &str,
    model: &str,
    history: &[ChatMessage],
    graph_context: &str,
    max_tokens: u32,
    temperature: f32,
) -> StreamRequest {
    let url = format!("{}/chat/completions", endpoint.trim_end_matches('/'));
    let mut headers = Vec::with_capacity(2);
    headers.push(("Content-Type".to_string(), "application/json".to_string()));
    if !api_key.is_empty() {
        headers.push(("Authorization".to_string(), format!("Bearer {}", api_key)));
    }
    let body = serde_json::json!({
        "model": model,
        "messages": build_messages(history, graph_context),
        "max_tokens": max_tokens,
        "temperature": temperature,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    StreamRequest { url, headers, body }
}

/// Build the wire-shape request for the first-class OpenRouter provider.
/// Always sends attribution headers (`HTTP-Referer`, `X-OpenRouter-Title`)
/// and forwards `provider.order` when configured.
fn build_openrouter_request(
    config: &OpenRouterConfig,
    history: &[ChatMessage],
    graph_context: &str,
) -> StreamRequest {
    let url = format!("{}/chat/completions", config.base_url.trim_end_matches('/'));
    let headers = vec![
        ("Content-Type".to_string(), "application/json".to_string()),
        (
            "Authorization".to_string(),
            format!("Bearer {}", config.api_key),
        ),
        ("HTTP-Referer".to_string(), config.http_referer.clone()),
        ("X-OpenRouter-Title".to_string(), config.app_title.clone()),
    ];

    let mut body = serde_json::json!({
        "model": config.model,
        "messages": build_messages(history, graph_context),
        "max_tokens": config.max_tokens,
        "temperature": config.temperature,
        "stream": true,
    });
    if config.include_usage_in_stream {
        body["stream_options"] = serde_json::json!({ "include_usage": true });
    }
    if let Some(order) = config.provider_order.as_ref().filter(|o| !o.is_empty()) {
        body["provider"] = serde_json::json!({ "order": order });
    }
    StreamRequest { url, headers, body }
}

/// Convert an [`LlmProvider`] enum value into a [`StreamRequest`], or `None`
/// if the variant doesn't have streaming support yet.
///
/// Variants returning `None`: `LocalLlama`, `MistralRs`, `AwsBedrock`.
/// Those need engine-specific token-callback wiring (LocalLlama / MistralRs)
/// or a separate Bedrock `ConverseStream` adapter (AwsBedrock).
fn build_request_for_provider(
    provider: &LlmProvider,
    history: &[ChatMessage],
    graph_context: &str,
) -> Option<StreamRequest> {
    match provider {
        LlmProvider::Api {
            endpoint,
            api_key,
            model,
        } => Some(build_api_request(
            endpoint,
            api_key,
            model,
            history,
            graph_context,
            512,
            0.7,
        )),
        LlmProvider::OpenRouter {
            api_key,
            model,
            base_url,
            provider_order,
            include_usage_in_stream,
            ..
        } => {
            let config = OpenRouterConfig {
                api_key: api_key.clone(),
                model: model.clone(),
                base_url: base_url.clone(),
                provider_order: provider_order.clone(),
                include_usage_in_stream: *include_usage_in_stream,
                http_referer: DEFAULT_HTTP_REFERER.to_string(),
                app_title: DEFAULT_APP_TITLE.to_string(),
                max_tokens: 512,
                temperature: 0.7,
            };
            Some(build_openrouter_request(&config, history, graph_context))
        }
        LlmProvider::LocalLlama
        | LlmProvider::MistralRs { .. }
        | LlmProvider::AwsBedrock { .. } => None,
    }
}

/// Spawn a background tokio task that streams chat tokens for `provider`
/// and emits them as [`TokenDelta`] messages on the returned receiver.
///
/// The task terminates after exactly one of `Done` / `Error` / `Cancelled`
/// has been sent, then drops `tx`. Callers should consume the channel to
/// completion; the dispatcher in `commands.rs` does this in a tokio task.
///
/// Cancellation: triggering `cancel` mid-stream causes the task to drop
/// the HTTP response (closing the upstream connection) and emit
/// [`TokenDelta::Cancelled`].
///
/// Returns a stream-handle pair: the [`mpsc::Receiver`] that produces
/// [`TokenDelta`] frames, and the [`CancellationToken`] the caller owns
/// to abort the stream. Cloning the cancel token (cheap) and storing the
/// clone in `AppState` is what `cancel_streaming_chat` uses to abort.
pub fn stream_chat(
    provider: LlmProvider,
    history: Vec<ChatMessage>,
    graph_context: String,
) -> (mpsc::Receiver<TokenDelta>, CancellationToken) {
    let (tx, rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();

    tokio::spawn(async move {
        let request = match build_request_for_provider(&provider, &history, &graph_context) {
            Some(r) => r,
            None => {
                let _ = tx
                    .send(TokenDelta::Error {
                        message: format!(
                            "Streaming chat not yet supported for provider {}; \
                             scoped follow-up issue (LocalLlama/MistralRs/Bedrock).",
                            provider_name(&provider)
                        ),
                        full_text: String::new(),
                    })
                    .await;
                return;
            }
        };
        run_sse_stream(request, tx, cancel_for_task).await;
    });

    (rx, cancel)
}

/// Short string identifier for a provider, used only in error messages.
fn provider_name(p: &LlmProvider) -> &'static str {
    match p {
        LlmProvider::Api { .. } => "Api",
        LlmProvider::OpenRouter { .. } => "OpenRouter",
        LlmProvider::LocalLlama => "LocalLlama",
        LlmProvider::MistralRs { .. } => "MistralRs",
        LlmProvider::AwsBedrock { .. } => "AwsBedrock",
    }
}

/// Drive the OpenAI-compatible SSE stream loop:
///
/// 1. POST `request.body` to `request.url` with attribution headers.
/// 2. Accumulate the response body via `bytes_stream()`.
/// 3. Push each chunk into [`SseDecoder`]; for every emitted `data:` frame,
///    parse it as a [`StreamChunk`] and forward each non-empty
///    `choices[0].delta.content` as a [`TokenDelta::Delta`].
/// 4. Terminate on `data: [DONE]` or `finish_reason` ≠ `null` with
///    [`TokenDelta::Done`].
///
/// All early exits (HTTP non-2xx, network error, malformed JSON, cancel
/// token tripped) emit exactly one terminal frame, so the consumer side
/// always sees one terminator regardless of the failure mode.
async fn run_sse_stream(
    request: StreamRequest,
    tx: mpsc::Sender<TokenDelta>,
    cancel: CancellationToken,
) {
    let client = match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx
                .send(TokenDelta::Error {
                    message: format!("Failed to build HTTP client: {}", e),
                    full_text: String::new(),
                })
                .await;
            return;
        }
    };

    let mut req = client.post(&request.url).json(&request.body);
    for (k, v) in &request.headers {
        // We pre-set Content-Type via .json(); skip duplicating.
        if k.eq_ignore_ascii_case("content-type") {
            continue;
        }
        req = req.header(k, v);
    }

    // Issue the request, racing against cancellation. If the user cancels
    // before the headers come back we abort cleanly with no partial text.
    let resp = tokio::select! {
        biased;
        _ = cancel.cancelled() => {
            let _ = tx.send(TokenDelta::Cancelled { full_text: String::new() }).await;
            return;
        }
        result = req.send() => match result {
            Ok(r) => r,
            Err(e) => {
                let _ = tx.send(TokenDelta::Error {
                    message: format!("HTTP request failed: {}", e),
                    full_text: String::new(),
                }).await;
                return;
            }
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        let _ = tx
            .send(TokenDelta::Error {
                message: format!(
                    "Streaming chat HTTP {}: {}",
                    status,
                    body.chars().take(500).collect::<String>()
                ),
                full_text: String::new(),
            })
            .await;
        return;
    }

    let mut decoder = SseDecoder::new();
    let mut full_text = String::new();
    let mut usage: Option<StreamUsage> = None;
    let mut byte_stream = resp.bytes_stream();

    loop {
        let next_chunk = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                let _ = tx.send(TokenDelta::Cancelled { full_text: full_text.clone() }).await;
                return;
            }
            chunk = byte_stream.next() => chunk,
        };

        let bytes: Bytes = match next_chunk {
            Some(Ok(b)) => b,
            Some(Err(e)) => {
                let _ = tx
                    .send(TokenDelta::Error {
                        message: format!("Stream read error: {}", e),
                        full_text,
                    })
                    .await;
                return;
            }
            None => break, // server closed the connection cleanly
        };
        decoder.feed(&bytes);

        loop {
            match decoder.next_event() {
                None => break,
                Some(SseEvent::Done) => {
                    let _ = tx
                        .send(TokenDelta::Done {
                            full_text: std::mem::take(&mut full_text),
                            usage: usage.take(),
                        })
                        .await;
                    return;
                }
                Some(SseEvent::Data(payload)) => {
                    match serde_json::from_str::<StreamChunk>(&payload) {
                        Ok(chunk) => {
                            if let Some(u) = chunk.usage {
                                usage = Some(u);
                            }
                            for choice in &chunk.choices {
                                if let Some(content) = choice.delta.content.as_deref() {
                                    if !content.is_empty() {
                                        full_text.push_str(content);
                                        if tx
                                            .send(TokenDelta::Delta {
                                                content: content.to_string(),
                                                finish_reason: choice.finish_reason.clone(),
                                            })
                                            .await
                                            .is_err()
                                        {
                                            // Receiver dropped; abandon.
                                            return;
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            log::warn!(
                                "Failed to parse streaming chunk: {} (payload: {})",
                                e,
                                payload.chars().take(200).collect::<String>()
                            );
                        }
                    }
                }
            }
        }
    }

    // Stream ended without an explicit `[DONE]` (some providers do this):
    // emit a Done with whatever we accumulated.
    let _ = tx.send(TokenDelta::Done { full_text, usage }).await;
}

/// In-memory request_id → cancel token registry.
///
/// Lives on `AppState`; lookup-on-cancel is O(1). The registry is the
/// single source of truth for "is this stream still active?" — it removes
/// the entry when the stream task signals terminal, and `cancel_streaming_chat`
/// removes-and-cancels in one swap.
#[derive(Clone, Default)]
pub struct StreamRegistry {
    inner: Arc<std::sync::Mutex<std::collections::HashMap<String, CancellationToken>>>,
}

impl StreamRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a `(request_id, cancel)` pair so [`StreamRegistry::cancel`]
    /// can find it later. Overwrites any prior token registered under the
    /// same id (which would only happen if the caller reuses request_ids,
    /// which it shouldn't).
    pub fn register(&self, request_id: String, cancel: CancellationToken) {
        if let Ok(mut g) = self.inner.lock() {
            g.insert(request_id, cancel);
        }
    }

    /// Mark a stream complete: remove it from the registry without firing
    /// cancel. Called from the stream task on its own terminal frame.
    pub fn finish(&self, request_id: &str) {
        if let Ok(mut g) = self.inner.lock() {
            g.remove(request_id);
        }
    }

    /// Cancel the stream associated with `request_id`. Returns `true` if a
    /// stream was found + cancelled, `false` if no such stream exists
    /// (already done / unknown id). Idempotent.
    pub fn cancel(&self, request_id: &str) -> bool {
        let token = {
            match self.inner.lock() {
                Ok(mut g) => g.remove(request_id),
                Err(_) => None,
            }
        };
        if let Some(t) = token {
            t.cancel();
            true
        } else {
            false
        }
    }
}

// `Serialize` impls used for the IPC payloads — defined here so they live
// next to the producer.

/// IPC payload for the `chat-token-delta` event.
#[derive(Debug, Clone, Serialize)]
pub struct ChatTokenDeltaPayload {
    pub request_id: String,
    pub delta: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
}

/// IPC payload for the `chat-token-done` event.
#[derive(Debug, Clone, Serialize)]
pub struct ChatTokenDonePayload {
    pub request_id: String,
    pub full_text: String,
    pub finish_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<StreamUsage>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Tiny SSE mock server. Reads (and discards) the request, then writes
    /// an HTTP/1.1 200 response with `Transfer-Encoding: chunked` and the
    /// supplied SSE body. Single-shot — closes after responding.
    async fn spawn_sse_mock(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let mut total = String::new();
                loop {
                    let n = match stream.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    total.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if total.contains("\r\n\r\n") {
                        break;
                    }
                }
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(header.as_bytes()).await;
                let _ = stream.write_all(body.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        format!("http://{}", addr)
    }

    /// Mock that streams the payload byte-by-byte with delays so the cancel
    /// token has a window to fire mid-stream.
    async fn spawn_slow_sse_mock(body: &'static str) -> (String, Arc<tokio::sync::Notify>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let started = Arc::new(tokio::sync::Notify::new());
        let started_for_task = started.clone();
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                // Drain headers
                let mut buf = vec![0u8; 4096];
                let mut total = String::new();
                loop {
                    let n = match stream.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    total.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if total.contains("\r\n\r\n") {
                        break;
                    }
                }
                let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(header.as_bytes()).await;
                started_for_task.notify_waiters();
                // Send each byte as its own chunked frame, with a small delay,
                // so the consumer has time to invoke cancel between bytes.
                for byte in body.bytes() {
                    let chunk = format!("1\r\n{}\r\n", byte as char);
                    if stream.write_all(chunk.as_bytes()).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                let _ = stream.write_all(b"0\r\n\r\n").await;
                let _ = stream.shutdown().await;
            }
        });
        (format!("http://{}", addr), started)
    }

    fn api_provider(endpoint: String) -> LlmProvider {
        LlmProvider::Api {
            endpoint,
            api_key: "sk-test".to_string(),
            model: "test-model".to_string(),
        }
    }

    #[tokio::test]
    async fn stream_chat_emits_deltas_and_done() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\"lo \"}}]}\n\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\"world\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n\
                    data: [DONE]\n\n";
        let base = spawn_sse_mock(body).await;
        let provider = api_provider(base);

        let (mut rx, _cancel) = stream_chat(provider, vec![], "graph context".to_string());

        let mut deltas: Vec<String> = Vec::new();
        let mut done_full: Option<String> = None;
        let mut done_usage: Option<StreamUsage> = None;
        while let Some(frame) = rx.recv().await {
            match frame {
                TokenDelta::Delta { content, .. } => deltas.push(content),
                TokenDelta::Done { full_text, usage } => {
                    done_full = Some(full_text);
                    done_usage = usage;
                    break;
                }
                TokenDelta::Error { message, .. } => {
                    panic!("unexpected error: {message}");
                }
                TokenDelta::Cancelled { .. } => panic!("unexpected cancel"),
            }
        }

        assert_eq!(deltas, vec!["Hel", "lo ", "world"]);
        assert_eq!(done_full.as_deref(), Some("Hello world"));
        let u = done_usage.expect("usage on terminal chunk");
        assert_eq!(u.total_tokens, Some(5));
    }

    #[tokio::test]
    async fn cancel_aborts_in_flight_stream() {
        // 100 byte-sized chunks so the producer takes ~2s — plenty of time
        // for our cancel to land mid-stream. The body itself is a single
        // SSE frame; the consumer should never observe a Done.
        let body =
            "data: {\"choices\":[{\"delta\":{\"content\":\"abcdefghijklmnopqrstuvwxyz\"}}]}\n\n";
        let (base, started) = spawn_slow_sse_mock(body).await;
        let provider = api_provider(base);

        let (mut rx, cancel) = stream_chat(provider, vec![], "ctx".to_string());

        // Wait until the server has started writing, then cancel.
        started.notified().await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        cancel.cancel();

        // Drain until terminal. We expect a Cancelled, possibly preceded by
        // partial Deltas.
        let mut saw_cancelled = false;
        let mut saw_done = false;
        while let Some(frame) = rx.recv().await {
            match frame {
                TokenDelta::Delta { .. } => continue,
                TokenDelta::Cancelled { .. } => {
                    saw_cancelled = true;
                    break;
                }
                TokenDelta::Done { .. } => {
                    saw_done = true;
                    break;
                }
                TokenDelta::Error { .. } => {
                    // network error from dropping the connection counts as a
                    // valid cancel outcome from the consumer's perspective —
                    // partials may have been received but no Done.
                    saw_cancelled = true;
                    break;
                }
            }
        }
        assert!(
            saw_cancelled,
            "expected a Cancelled (or Error) terminator after cancel, got Done={}",
            saw_done
        );
    }

    #[tokio::test]
    async fn stream_chat_reports_unsupported_provider() {
        let provider = LlmProvider::LocalLlama;
        let (mut rx, _cancel) = stream_chat(provider, vec![], String::new());
        let frame = rx.recv().await.expect("at least one terminal frame");
        match frame {
            TokenDelta::Error { message, .. } => {
                assert!(
                    message.contains("LocalLlama"),
                    "error must name the unsupported provider, got: {message}"
                );
            }
            other => panic!("expected Error for LocalLlama, got {other:?}"),
        }
    }

    #[test]
    fn registry_cancel_finds_and_fires_token() {
        let reg = StreamRegistry::new();
        let token = CancellationToken::new();
        reg.register("req-1".into(), token.clone());

        assert!(reg.cancel("req-1"), "first cancel must report success");
        assert!(token.is_cancelled());
        assert!(
            !reg.cancel("req-1"),
            "second cancel of the same id must be a no-op"
        );
    }

    #[test]
    fn registry_finish_removes_without_cancel() {
        let reg = StreamRegistry::new();
        let token = CancellationToken::new();
        reg.register("req-fin".into(), token.clone());

        reg.finish("req-fin");
        assert!(
            !token.is_cancelled(),
            "finish() must NOT fire cancel — the stream completed normally"
        );
        assert!(
            !reg.cancel("req-fin"),
            "cancel after finish must observe the entry is gone"
        );
    }
}
