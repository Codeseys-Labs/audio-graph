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
//! `LocalLlama` and `MistralRs` use the provider-neutral request path plus an
//! explicit backend handle. Each local engine owns its own token loop and emits
//! engine-level stream events that this module bridges into `TokenDelta`
//! (`run_local_llama_stream` / `run_mistralrs_stream`). `AwsBedrock` is a
//! non-SSE adapter: it builds an `aws_sdk_bedrockruntime` client on demand and
//! drives the `ConverseStream` event stream into `TokenDelta` (see
//! [`crate::llm::bedrock`]).
//!
//! Wire shape: see `crate::llm::sse` for the SSE chunk parser and the
//! OpenAI-compat `StreamChunk` deserialization shape that both SSE providers
//! emit.

use std::sync::Arc;

use bytes::Bytes;
use futures_util::StreamExt;
use serde::Serialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::llm::engine::{ChatMessage, LlmChatParams, LlmStreamEvent};
use crate::llm::openrouter::{
    DEFAULT_APP_TITLE, DEFAULT_HTTP_REFERER, OpenRouterConfig, OpenRouterRoutingPolicy,
};
use crate::llm::sse::{SseDecoder, SseEvent, StreamChunk};
pub use crate::llm::stream_contract::{
    StreamBackendHandles, StreamChatRequest, StreamContextMetadata, StreamParams,
    StreamSourceMetadata, StreamTerminalEvent, StreamTerminalReason, StreamUsage,
};
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
    /// honoured `stream_options.include_usage`. `finish_reason` carries the
    /// last non-null `choices[0].finish_reason` from the SSE stream — usually
    /// `"stop"`, sometimes `"length"` (truncated by max_tokens), `"content_filter"`,
    /// or `"tool_calls"`. Defaults to `"stop"` if the stream ends without
    /// the provider declaring one (some providers omit it on `[DONE]`).
    Done {
        full_text: String,
        usage: Option<StreamUsage>,
        finish_reason: String,
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

impl TokenDelta {
    /// Adapt the provider-neutral terminal event into the legacy IPC-facing
    /// frame shape. Future adapters should build [`StreamTerminalEvent`] first
    /// so Done/Error/Cancelled semantics stay shared.
    pub fn from_terminal_event(event: StreamTerminalEvent) -> Self {
        match event.reason {
            StreamTerminalReason::Done { finish_reason } => Self::Done {
                full_text: event.full_text,
                usage: event.usage,
                finish_reason,
            },
            StreamTerminalReason::Error { message } => Self::Error {
                message,
                full_text: event.full_text,
            },
            StreamTerminalReason::Cancelled => Self::Cancelled {
                full_text: event.full_text,
            },
        }
    }
}

/// Configuration for a single streaming chat request.
///
/// The active provider is materialized into an HTTP request shape here
/// rather than passing the full `LlmProvider` enum down through the SSE
/// loop, so the loop itself stays provider-agnostic.
struct StreamRequest {
    provider: &'static str,
    url: String,
    headers: Vec<(String, String)>,
    body: serde_json::Value,
    secrets: Vec<String>,
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
    let secrets = (!api_key.is_empty())
        .then(|| api_key.to_string())
        .into_iter()
        .collect();
    StreamRequest {
        provider: "api",
        url,
        headers,
        body,
        secrets,
    }
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
    if let Some(provider) = config.provider_routing_value() {
        body["provider"] = provider;
    }
    StreamRequest {
        provider: "openrouter",
        url,
        headers,
        body,
        secrets: vec![config.api_key.clone()],
    }
}

fn openrouter_routing_policy_from_backend_handles(
    request: &StreamChatRequest,
) -> Option<OpenRouterRoutingPolicy> {
    let client = request.backend_handles.openrouter_client.as_ref()?;
    let guard = client.lock().ok()?;
    let client = guard.as_ref()?;
    client.config().routing_policy.clone()
}

/// Convert an [`LlmProvider`] enum value into a [`StreamRequest`], or `None`
/// if the variant doesn't have streaming support yet.
///
/// `max_tokens` / `temperature` come from the caller (the user-configured
/// `llm_api_config`, with the same fallback the blocking chat path uses).
/// They are NOT hardcoded here: a hardcode silently discards the user's
/// configured sampling settings, which the blocking executor honours — see
/// `commands::api_config_from_runtime_settings` /
/// `openrouter_config_from_runtime_settings`.
///
/// Variants returning `None`: `LocalLlama`, `MistralRs`, `AwsBedrock`.
/// `LocalLlama` and `MistralRs` are handled by `run_local_llama_stream` /
/// `run_mistralrs_stream` before this HTTP/SSE request builder is consulted, so
/// they never reach the `None` arm in practice. Bedrock needs a separate
/// `ConverseStream` adapter and remains deferred.
fn build_request_for_provider(
    request: &StreamChatRequest,
) -> Result<Option<StreamRequest>, String> {
    check_streaming_http_content_egress(request)?;

    let StreamParams {
        max_tokens,
        temperature,
    } = request.params;
    Ok(match &request.provider {
        LlmProvider::Api {
            endpoint,
            api_key,
            model,
        } => Some(build_api_request(
            endpoint,
            api_key,
            model,
            &request.history,
            &request.graph_context,
            max_tokens,
            temperature,
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
                routing_policy: openrouter_routing_policy_from_backend_handles(request),
                include_usage_in_stream: *include_usage_in_stream,
                http_referer: DEFAULT_HTTP_REFERER.to_string(),
                app_title: DEFAULT_APP_TITLE.to_string(),
                max_tokens,
                temperature,
            };
            Some(build_openrouter_request(
                &config,
                &request.history,
                &request.graph_context,
            ))
        }
        LlmProvider::LocalLlama
        | LlmProvider::MistralRs { .. }
        | LlmProvider::AwsBedrock { .. } => None,
    })
}

fn streaming_http_provider_policy_name(provider: &LlmProvider) -> Option<&'static str> {
    match provider {
        LlmProvider::Api { .. } => Some("llm.api"),
        LlmProvider::OpenRouter { .. } => Some("llm.openrouter"),
        LlmProvider::LocalLlama
        | LlmProvider::MistralRs { .. }
        | LlmProvider::AwsBedrock { .. } => None,
    }
}

fn check_streaming_http_content_egress(request: &StreamChatRequest) -> Result<(), String> {
    let Some(provider) = streaming_http_provider_policy_name(&request.provider) else {
        return Ok(());
    };

    request.content_egress_policy.check_prompt(provider)?;
    request.content_egress_policy.check_json(provider)
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
///
/// `params` carries the user-configured sampling settings (`max_tokens` /
/// `temperature`). The caller must source them from the same config the
/// blocking chat path reads so both paths produce comparable replies.
pub fn stream_chat(
    provider: LlmProvider,
    history: Vec<ChatMessage>,
    graph_context: String,
    params: StreamParams,
) -> (mpsc::Receiver<TokenDelta>, CancellationToken) {
    stream_chat_with_request(StreamChatRequest::new(
        provider,
        history,
        graph_context,
        params,
    ))
}

/// Spawn a streaming chat task from an explicit provider-neutral request.
///
/// This is the entry point future LocalLlama, mistral.rs, and Bedrock adapters
/// should use when they need backend handles or source/context metadata.
pub fn stream_chat_with_request(
    request: StreamChatRequest,
) -> (mpsc::Receiver<TokenDelta>, CancellationToken) {
    let (tx, rx) = mpsc::channel(64);
    let cancel = CancellationToken::new();
    let cancel_for_task = cancel.clone();

    tokio::spawn(async move {
        let metadata = request.metadata.clone();
        if matches!(&request.provider, LlmProvider::LocalLlama) {
            run_local_llama_stream(request, tx, cancel_for_task, metadata).await;
            return;
        }
        if matches!(&request.provider, LlmProvider::MistralRs { .. }) {
            run_mistralrs_stream(request, tx, cancel_for_task, metadata).await;
            return;
        }

        // AwsBedrock is a non-SSE provider: it builds an aws_sdk_bedrockruntime
        // client on demand and drives the ConverseStream event stream into
        // TokenDelta. Branch before `build_request_for_provider` (it stays in
        // that builder's `=> None` arm because it does not use the SSE request
        // shape), mirroring the LocalLlama early-return above.
        if let LlmProvider::AwsBedrock {
            region,
            model_id,
            credential_source,
        } = request.provider.clone()
        {
            // Honor the same content-egress policy gate the SSE path applies
            // before any cloud request leaves the machine.
            if let Err(message) = request
                .content_egress_policy
                .check_prompt("llm.aws_bedrock")
                .and_then(|()| request.content_egress_policy.check_json("llm.aws_bedrock"))
            {
                send_terminal(
                    &tx,
                    StreamTerminalEvent::error(message, String::new(), metadata),
                )
                .await;
                return;
            }
            let adapter = crate::llm::bedrock::BedrockConverseStreamAdapter::new(
                region,
                model_id,
                credential_source,
                request.history,
                request.graph_context,
                request.params.max_tokens,
                request.params.temperature,
            )
            // Defense-in-depth: thread the same policy into the adapter so the
            // provider client carries a second egress gate even though the
            // router-level check above already gated this request.
            .with_content_egress_policy(request.content_egress_policy);
            adapter.run(tx, cancel_for_task, metadata).await;
            return;
        }

        let stream_request = match build_request_for_provider(&request) {
            Ok(Some(r)) => r,
            Ok(None) => {
                let _ = tx
                    .send(TokenDelta::from_terminal_event(StreamTerminalEvent::error(
                        format!(
                            "Streaming chat not supported for provider {}.",
                            provider_name(&request.provider)
                        ),
                        String::new(),
                        metadata,
                    )))
                    .await;
                return;
            }
            Err(message) => {
                send_terminal(
                    &tx,
                    StreamTerminalEvent::error(message, String::new(), metadata),
                )
                .await;
                return;
            }
        };
        run_sse_stream(stream_request, tx, cancel_for_task, metadata).await;
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

async fn send_terminal(tx: &mpsc::Sender<TokenDelta>, event: StreamTerminalEvent) {
    let _ = tx.send(TokenDelta::from_terminal_event(event)).await;
}

fn local_usage_from_done(
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
) -> Option<StreamUsage> {
    (total_tokens > 0).then_some(StreamUsage {
        prompt_tokens: Some(prompt_tokens),
        completion_tokens: Some(completion_tokens),
        total_tokens: Some(total_tokens),
    })
}

/// Local llama.cpp streaming adapter.
///
/// The persistent-context actor owns the model/context and emits one
/// [`LlmStreamEvent::Delta`] for each generated non-EOG token piece. Cancellation
/// is observed by that actor between token decodes; an in-progress llama.cpp
/// `ctx.decode` call cannot be interrupted through the safe API.
async fn run_local_llama_stream(
    request: StreamChatRequest,
    tx: mpsc::Sender<TokenDelta>,
    cancel: CancellationToken,
    metadata: StreamContextMetadata,
) {
    if cancel.is_cancelled() {
        send_terminal(&tx, StreamTerminalEvent::cancelled(String::new(), metadata)).await;
        return;
    }

    let Some(local_llama) = request.backend_handles.local_llama.clone() else {
        send_terminal(
            &tx,
            StreamTerminalEvent::error(
                "LocalLlama streaming requires StreamBackendHandles.local_llama; pass the explicit loaded local engine handle with StreamChatRequest instead of relying on AppState globals."
                    .to_string(),
                String::new(),
                metadata,
            ),
        )
        .await;
        return;
    };

    let engine = {
        match local_llama.lock() {
            Ok(guard) => guard.as_ref().cloned().ok_or_else(|| {
                "LocalLlama engine is not loaded; load a local LLM model before starting streaming chat."
                    .to_string()
            }),
            Err(e) => Err(format!("LocalLlama engine lock failed: {}", e)),
        }
    };

    let engine = match engine {
        Ok(engine) => engine,
        Err(message) => {
            send_terminal(
                &tx,
                StreamTerminalEvent::error(message, String::new(), metadata),
            )
            .await;
            return;
        }
    };

    let local_params = LlmChatParams {
        max_tokens: request.params.max_tokens,
        temperature: request.params.temperature,
    };
    let (event_tx, mut event_rx) = mpsc::channel(64);
    if let Err(message) = engine.stream_chat(
        request.history,
        request.graph_context,
        local_params,
        cancel.clone(),
        event_tx,
    ) {
        send_terminal(
            &tx,
            StreamTerminalEvent::error(message, String::new(), metadata),
        )
        .await;
        return;
    }

    while let Some(event) = event_rx.recv().await {
        match event {
            LlmStreamEvent::Delta { content } => {
                if tx
                    .send(TokenDelta::Delta {
                        content,
                        finish_reason: None,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            LlmStreamEvent::Done {
                full_text,
                prompt_tokens,
                completion_tokens,
                total_tokens,
            } => {
                let usage = local_usage_from_done(prompt_tokens, completion_tokens, total_tokens);
                send_terminal(
                    &tx,
                    StreamTerminalEvent::done(full_text, usage, "stop".to_string(), metadata),
                )
                .await;
                return;
            }
            LlmStreamEvent::Cancelled { full_text } => {
                send_terminal(&tx, StreamTerminalEvent::cancelled(full_text, metadata)).await;
                return;
            }
            LlmStreamEvent::Error { message, full_text } => {
                send_terminal(
                    &tx,
                    StreamTerminalEvent::error(message, full_text, metadata),
                )
                .await;
                return;
            }
        }
    }

    send_terminal(
        &tx,
        StreamTerminalEvent::error(
            "LocalLlama engine stream ended without a terminal frame".to_string(),
            String::new(),
            metadata,
        ),
    )
    .await;
}

/// mistral.rs (Candle GGUF) streaming adapter.
///
/// Mirrors [`run_local_llama_stream`]: the engine owns the async stream over its
/// dedicated tokio runtime and emits one [`LlmStreamEvent::Delta`] per streamed
/// content fragment, then exactly one terminal `Done` / `Cancelled` / `Error`.
/// This bridge pulls the explicit `mistralrs_engine` backend handle, locks it,
/// reports a single `Error` for a missing or unloaded engine, and otherwise
/// drains the engine's `LlmStreamEvent` channel into [`TokenDelta`].
async fn run_mistralrs_stream(
    request: StreamChatRequest,
    tx: mpsc::Sender<TokenDelta>,
    cancel: CancellationToken,
    metadata: StreamContextMetadata,
) {
    if cancel.is_cancelled() {
        send_terminal(&tx, StreamTerminalEvent::cancelled(String::new(), metadata)).await;
        return;
    }

    let Some(mistralrs) = request.backend_handles.mistralrs_engine.clone() else {
        send_terminal(
            &tx,
            StreamTerminalEvent::error(
                "MistralRs streaming requires StreamBackendHandles.mistralrs_engine; pass the explicit loaded mistral.rs engine handle with StreamChatRequest instead of relying on AppState globals."
                    .to_string(),
                String::new(),
                metadata,
            ),
        )
        .await;
        return;
    };

    let mistralrs_params = LlmChatParams {
        max_tokens: request.params.max_tokens,
        temperature: request.params.temperature,
    };
    let (event_tx, event_rx) = mpsc::channel(64);

    // Unlike the llama actor (whose `stream_chat` returns immediately after
    // handing the request to its actor thread), the mistral.rs engine is NOT
    // `Clone` and drives the whole stream synchronously on its own dedicated
    // tokio runtime via `block_on`. So we move the shared handle into a blocking
    // thread, lock it there (the lock is held for the generation, exactly as the
    // blocking chat path does), and call `stream_chat` on the borrowed engine.
    // Tokens arrive over `event_rx` as it generates; missing/unloaded engine and
    // request-rejection each surface as a single terminal Error on that channel.
    let cancel_for_engine = cancel.clone();
    let history = request.history;
    let graph_context = request.graph_context;
    let engine_tx = event_tx.clone();
    tokio::task::spawn_blocking(move || {
        let guard = match mistralrs.lock() {
            Ok(guard) => guard,
            Err(e) => {
                let _ = engine_tx.blocking_send(LlmStreamEvent::Error {
                    message: format!("MistralRs engine lock failed: {}", e),
                    full_text: String::new(),
                });
                return;
            }
        };
        let Some(engine) = guard.as_ref() else {
            let _ = engine_tx.blocking_send(LlmStreamEvent::Error {
                message:
                    "MistralRs engine is not loaded; load a local LLM model before starting streaming chat."
                        .to_string(),
                full_text: String::new(),
            });
            return;
        };
        if let Err(message) = engine.stream_chat(
            history,
            graph_context,
            mistralrs_params,
            cancel_for_engine,
            engine_tx.clone(),
        ) {
            let _ = engine_tx.blocking_send(LlmStreamEvent::Error {
                message,
                full_text: String::new(),
            });
        }
    });
    // The drain loop owns the only remaining receiver end; drop our extra sender
    // clone so the channel closes once the blocking task finishes.
    drop(event_tx);

    drain_engine_stream_events(
        event_rx,
        &tx,
        metadata,
        "MistralRs engine stream ended without a terminal frame",
    )
    .await;
}

/// Drain an engine-owned [`LlmStreamEvent`] channel into [`TokenDelta`] frames.
///
/// Shared by the local-engine adapters (mistral.rs today; the llama path keeps
/// its own inline copy for now). Each `Delta` forwards one content fragment;
/// the first `Done` / `Cancelled` / `Error` emits exactly one terminal frame and
/// returns. If the channel closes with no terminal frame (the engine dropped its
/// sender mid-stream), `ended_without_terminal` is surfaced as a single terminal
/// `Error` so the consumer never blocks waiting for a terminator that never
/// arrives. Sending stops early (without a terminal) only if the consumer has
/// already dropped its receiver — there is then no one to deliver to.
async fn drain_engine_stream_events(
    mut event_rx: mpsc::Receiver<LlmStreamEvent>,
    tx: &mpsc::Sender<TokenDelta>,
    metadata: StreamContextMetadata,
    ended_without_terminal: &str,
) {
    while let Some(event) = event_rx.recv().await {
        match event {
            LlmStreamEvent::Delta { content } => {
                if tx
                    .send(TokenDelta::Delta {
                        content,
                        finish_reason: None,
                    })
                    .await
                    .is_err()
                {
                    return;
                }
            }
            LlmStreamEvent::Done {
                full_text,
                prompt_tokens,
                completion_tokens,
                total_tokens,
            } => {
                let usage = local_usage_from_done(prompt_tokens, completion_tokens, total_tokens);
                send_terminal(
                    tx,
                    StreamTerminalEvent::done(full_text, usage, "stop".to_string(), metadata),
                )
                .await;
                return;
            }
            LlmStreamEvent::Cancelled { full_text } => {
                send_terminal(tx, StreamTerminalEvent::cancelled(full_text, metadata)).await;
                return;
            }
            LlmStreamEvent::Error { message, full_text } => {
                send_terminal(tx, StreamTerminalEvent::error(message, full_text, metadata)).await;
                return;
            }
        }
    }

    send_terminal(
        tx,
        StreamTerminalEvent::error(ended_without_terminal.to_string(), String::new(), metadata),
    )
    .await;
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
    metadata: StreamContextMetadata,
) {
    let client = match reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            send_terminal(
                &tx,
                StreamTerminalEvent::error(
                    format!("Failed to build HTTP client: {}", e),
                    String::new(),
                    metadata,
                ),
            )
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
            send_terminal(
                &tx,
                StreamTerminalEvent::cancelled(String::new(), metadata),
            ).await;
            return;
        }
        result = req.send() => match result {
            Ok(r) => r,
            Err(e) => {
                let message = crate::error::redacted_error_excerpt(
                    &format!("HTTP request failed: {}", e),
                    request.secrets.iter().map(String::as_str),
                    500,
                );
                send_terminal(
                    &tx,
                    StreamTerminalEvent::error(message, String::new(), metadata),
                ).await;
                return;
            }
        }
    };

    if !resp.status().is_success() {
        let status = resp.status();
        let request_id = response_request_id(resp.headers());
        let body = resp.text().await.unwrap_or_default();
        send_terminal(
            &tx,
            StreamTerminalEvent::error(
                streaming_http_error_message(
                    request.provider,
                    &request.url,
                    status,
                    &body,
                    request_id.as_deref(),
                ),
                String::new(),
                metadata,
            ),
        )
        .await;
        return;
    }

    let mut decoder = SseDecoder::new();
    let mut full_text = String::new();
    let mut usage: Option<StreamUsage> = None;
    let mut last_finish_reason: Option<String> = None;
    let response_request_id = response_request_id(resp.headers());
    let mut byte_stream = resp.bytes_stream();

    loop {
        let next_chunk = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                send_terminal(
                    &tx,
                    StreamTerminalEvent::cancelled(full_text.clone(), metadata),
                ).await;
                return;
            }
            chunk = byte_stream.next() => chunk,
        };

        let bytes: Bytes = match next_chunk {
            Some(Ok(b)) => b,
            Some(Err(e)) => {
                // Route the transport read error through the safe helper: a
                // reqwest stream error Displays the request URL, which can
                // embed userinfo / query credentials, and any registered
                // provider secret could otherwise surface verbatim here.
                let message = crate::error::redacted_error_excerpt(
                    &format!("Stream read error: {}", e),
                    request.secrets.iter().map(String::as_str),
                    500,
                );
                send_terminal(
                    &tx,
                    StreamTerminalEvent::error(message, full_text, metadata),
                )
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
                    send_terminal(
                        &tx,
                        StreamTerminalEvent::done(
                            std::mem::take(&mut full_text),
                            usage.take(),
                            last_finish_reason
                                .take()
                                .unwrap_or_else(|| "stop".to_string()),
                            metadata,
                        ),
                    )
                    .await;
                    return;
                }
                Some(SseEvent::Error(message)) => {
                    let message = crate::error::redacted_error_excerpt(
                        &message,
                        request.secrets.iter().map(String::as_str),
                        500,
                    );
                    send_terminal(
                        &tx,
                        StreamTerminalEvent::error(message, full_text, metadata),
                    )
                    .await;
                    return;
                }
                Some(SseEvent::Data(payload)) => {
                    match serde_json::from_str::<StreamChunk>(&payload) {
                        Ok(chunk) => {
                            // Keep the last usage block that actually carries a
                            // populated `total_tokens`. Some providers emit a
                            // trailing keepalive / `[DONE]`-adjacent chunk with
                            // `usage{}` (all-null); blindly overwriting would
                            // clobber a real earlier count down to 0.
                            if let Some(u) = chunk.usage
                                && u.has_reported_total()
                            {
                                usage = Some(u);
                            }
                            for choice in &chunk.choices {
                                // Capture the last non-null finish_reason — the
                                // provider sends it on the chunk that ends the
                                // generation (often the same chunk as the last
                                // delta content, but sometimes a separate trailer).
                                if let Some(reason) = choice.finish_reason.as_deref()
                                    && !reason.is_empty()
                                {
                                    last_finish_reason = Some(reason.to_string());
                                }
                                if let Some(content) = choice.delta.content.as_deref()
                                    && !content.is_empty()
                                {
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
                        Err(e) => {
                            log::warn!(
                                "{}",
                                streaming_parse_error_message(
                                    request.provider,
                                    &request.url,
                                    &e,
                                    &payload,
                                    response_request_id.as_deref(),
                                )
                            );
                        }
                    }
                }
            }
        }
    }

    // Stream ended without an explicit `[DONE]` (some providers do this):
    // emit a Done with whatever we accumulated.
    send_terminal(
        &tx,
        StreamTerminalEvent::done(
            full_text,
            usage,
            last_finish_reason.unwrap_or_else(|| "stop".to_string()),
            metadata,
        ),
    )
    .await;
}

fn streaming_http_error_message(
    provider: &str,
    url: &str,
    status: reqwest::StatusCode,
    body: &str,
    request_id: Option<&str>,
) -> String {
    format!(
        "Streaming chat HTTP error: provider={} path={} status={} body_bytes={} body_chars={}{}",
        provider,
        diagnostic_path(url),
        status.as_u16(),
        body.len(),
        body.chars().count(),
        request_id
            .map(|id| format!(" request_id={id}"))
            .unwrap_or_default()
    )
}

fn streaming_parse_error_message(
    provider: &str,
    url: &str,
    error: &serde_json::Error,
    payload: &str,
    request_id: Option<&str>,
) -> String {
    format!(
        "Failed to parse streaming chunk: provider={} path={} class={} detail={} payload_bytes={} payload_chars={}{}",
        provider,
        diagnostic_path(url),
        json_error_class(error),
        error,
        payload.len(),
        payload.chars().count(),
        request_id
            .map(|id| format!(" request_id={id}"))
            .unwrap_or_default()
    )
}

fn diagnostic_path(url: &str) -> String {
    reqwest::Url::parse(url)
        .map(|parsed| parsed.path().to_string())
        .unwrap_or_else(|_| "<unparseable>".to_string())
}

fn response_request_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    for name in [
        "x-request-id",
        "request-id",
        "x-openrouter-request-id",
        "cf-ray",
    ] {
        let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) else {
            continue;
        };
        let sanitized: String = value
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
            .take(128)
            .collect();
        if !sanitized.is_empty() {
            return Some(sanitized);
        }
    }
    None
}

fn json_error_class(error: &serde_json::Error) -> &'static str {
    match error.classify() {
        serde_json::error::Category::Io => "io",
        serde_json::error::Category::Syntax => "syntax",
        serde_json::error::Category::Data => "data",
        serde_json::error::Category::Eof => "eof",
    }
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

    /// Cancel the stream associated with `request_id`.
    ///
    /// Returns `true` when a token was present and cancellation was requested,
    /// `false` when no such stream was registered. This is a best-effort signal:
    /// the stream task may concurrently finish after the token is removed but
    /// before `cancel()` runs, so callers must not assume `true` guarantees the
    /// next terminal frame will be `Cancelled` rather than `Done`/`Error`.
    /// Idempotent.
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

    /// Cancel + drop every currently-registered stream, returning how many
    /// live streams were cancelled.
    ///
    /// Used to enforce "at most one active chat stream per session": the
    /// frontend tracks only a single `streamingChatRequestId`, so a second
    /// `start_streaming_chat` while the first still drains would leave the
    /// first consumer task running (burning tokens) and unreachable by
    /// `cancel_streaming_chat`. Cancelling priors before registering the new
    /// stream guarantees the registry never holds an orphaned entry the UI
    /// can no longer reach. Idempotent on an empty registry (returns 0).
    pub fn cancel_all(&self) -> usize {
        let tokens: Vec<CancellationToken> = match self.inner.lock() {
            Ok(mut g) => g.drain().map(|(_, token)| token).collect(),
            Err(_) => Vec::new(),
        };
        let count = tokens.len();
        for t in tokens {
            t.cancel();
        }
        count
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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
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

    async fn spawn_http_error_mock(status: u16, status_text: &'static str, body: String) -> String {
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
                let response = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nX-Request-Id: stream_req_123\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status,
                    status_text,
                    body.len(),
                    body,
                );
                let _ = stream.write_all(response.as_bytes()).await;
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

    async fn spawn_connection_probe() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let connections = Arc::new(AtomicUsize::new(0));
        let connections_for_task = connections.clone();
        let handle = tokio::spawn(async move {
            if let Ok(Ok((_stream, _))) =
                tokio::time::timeout(std::time::Duration::from_millis(300), listener.accept()).await
            {
                connections_for_task.fetch_add(1, Ordering::SeqCst);
            }
        });
        (format!("http://{}", addr), connections, handle)
    }

    fn api_provider(endpoint: String) -> LlmProvider {
        LlmProvider::Api {
            endpoint,
            api_key: "sk-test".to_string(),
            model: "test-model".to_string(),
        }
    }

    fn allowed_stream_request(
        provider: LlmProvider,
        history: Vec<ChatMessage>,
        graph_context: String,
        params: StreamParams,
    ) -> StreamChatRequest {
        StreamChatRequest::new(provider, history, graph_context, params)
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow())
    }

    #[tokio::test]
    async fn stream_chat_emits_deltas_and_done() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\"lo \"}}]}\n\n\
                    data: {\"choices\":[{\"delta\":{\"content\":\"world\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n\
                    data: [DONE]\n\n";
        let base = spawn_sse_mock(body).await;
        let provider = api_provider(base);

        let (mut rx, _cancel) = stream_chat_with_request(allowed_stream_request(
            provider,
            vec![],
            "graph context".to_string(),
            StreamParams::default(),
        ));

        let mut deltas: Vec<String> = Vec::new();
        let mut done_full: Option<String> = None;
        let mut done_usage: Option<StreamUsage> = None;
        while let Some(frame) = rx.recv().await {
            match frame {
                TokenDelta::Delta { content, .. } => deltas.push(content),
                TokenDelta::Done {
                    full_text,
                    usage,
                    finish_reason,
                } => {
                    done_full = Some(full_text);
                    done_usage = usage;
                    assert_eq!(
                        finish_reason, "stop",
                        "expected provider's finish_reason 'stop' to propagate"
                    );
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
    async fn stream_chat_http_error_uses_metadata_only_diagnostic() {
        let api_key = "sk-stream-chat-secret";
        let prompt_echo = "patient transcript and private graph context";
        let body = format!(
            r#"{{"error":"bad auth {api_key}; {prompt_echo}","authorization":"Bearer bearer-stream-secret-12345","aws":"AKIA1234567890ABCDEF"}}"#
        );
        let base = spawn_http_error_mock(401, "Unauthorized", body).await;
        let provider = LlmProvider::Api {
            endpoint: base,
            api_key: api_key.to_string(),
            model: "test-model".to_string(),
        };

        let (mut rx, _cancel) = stream_chat_with_request(allowed_stream_request(
            provider,
            vec![],
            "graph context".to_string(),
            StreamParams::default(),
        ));

        match rx.recv().await.expect("terminal frame") {
            TokenDelta::Error { message, .. } => {
                assert!(message.contains("Streaming chat HTTP error"));
                assert!(message.contains("provider=api"));
                assert!(message.contains("path=/chat/completions"));
                assert!(message.contains("status=401"));
                assert!(message.contains("request_id=stream_req_123"));
                assert!(
                    message.contains("body_bytes="),
                    "error must carry body byte length, got: {message}"
                );
                assert!(
                    message.contains("body_chars="),
                    "error must carry body char length, got: {message}"
                );
                assert!(
                    !message.contains("bad auth") && !message.contains(prompt_echo),
                    "streaming error must not echo provider body or prompt context: {message}"
                );
                for leaked in [
                    api_key,
                    "bearer-stream-secret-12345",
                    "AKIA1234567890ABCDEF",
                ] {
                    assert!(
                        !message.contains(leaked),
                        "streaming error leaked {leaked}: {message}"
                    );
                }
            }
            other => panic!("expected HTTP error terminal frame, got {other:?}"),
        }
    }

    /// The streaming HTTP-error diagnostic must be metadata-only: it emits only
    /// the URL *path* (never the query string or userinfo) plus body byte/char
    /// counts, so no credential shape — API-key-like, bearer-token-like,
    /// AWS-access-key-like, or URL userinfo/query credential — can reach the
    /// UI-visible `TokenDelta::Error`, even when the provider echoes them in the
    /// response body or they ride in the request URL.
    #[test]
    fn streaming_http_error_diagnostic_never_surfaces_any_credential_shape() {
        let body = concat!(
            r#"{"error":"echoed provider body; patient transcript","#,
            r#""api_key":"sk-stream-body-secret-12345","#,
            r#""authorization":"Bearer bearer-stream-body-secret-12345","#,
            r#""aws":"AKIA1234567890ABCDEF"}"#,
        );
        // Assemble the userinfo at runtime so the source carries no contiguous
        // scheme://user:pass@host literal (which secret scanners flag as a Basic
        // Auth String); the runtime URL is byte-identical, so the redaction
        // assertions below still exercise a real credential-bearing URL.
        let userinfo = format!("{}:{}", "svc-user", "svc-pass");
        let url = format!(
            "https://{userinfo}@provider.example/v1/chat/completions?api_key=stream-url-secret-12345&token=stream-url-token-12345"
        );
        let message = streaming_http_error_message(
            "api",
            &url,
            reqwest::StatusCode::UNAUTHORIZED,
            body,
            Some("stream_req_xyz"),
        );

        // Metadata context is preserved.
        assert!(message.contains("Streaming chat HTTP error"));
        assert!(message.contains("provider=api"));
        assert!(message.contains("path=/v1/chat/completions"));
        assert!(message.contains("status=401"));
        assert!(message.contains("request_id=stream_req_xyz"));
        assert!(message.contains(&format!("body_bytes={}", body.len())));
        assert!(message.contains(&format!("body_chars={}", body.chars().count())));

        // No credential shape, body content, or URL query/userinfo leaks.
        for leaked in [
            "sk-stream-body-secret-12345",
            "bearer-stream-body-secret-12345",
            "AKIA1234567890ABCDEF",
            "svc-user:svc-pass",
            "stream-url-secret-12345",
            "stream-url-token-12345",
            "echoed provider body",
            "patient transcript",
            "api_key=",
            "token=",
        ] {
            assert!(
                !message.contains(leaked),
                "streaming HTTP diagnostic leaked {leaked}: {message}"
            );
        }
    }

    #[test]
    fn streaming_parse_error_diagnostic_uses_payload_metadata_only() {
        let payload =
            r#"{"choices":[{"delta":{"content":"patient transcript and graph context"}}]"#;
        let error = serde_json::from_str::<StreamChunk>(payload)
            .expect_err("fixture must be malformed JSON");
        let message = streaming_parse_error_message(
            "api",
            "https://provider.example/v1/chat/completions",
            &error,
            payload,
            Some("parse_req_123"),
        );

        assert!(message.contains("Failed to parse streaming chunk"));
        assert!(message.contains("provider=api"));
        assert!(message.contains("path=/v1/chat/completions"));
        assert!(message.contains("class=eof"));
        assert!(message.contains("request_id=parse_req_123"));
        assert!(message.contains(&format!("payload_bytes={}", payload.len())));
        assert!(message.contains(&format!("payload_chars={}", payload.chars().count())));
        assert!(
            !message.contains("patient transcript")
                && !message.contains("graph context")
                && !message.contains(payload),
            "parse diagnostic must not echo malformed SSE payload: {message}"
        );
    }

    #[tokio::test]
    async fn blocked_content_egress_prevents_cloud_stream_request_send() {
        for provider_name in ["llm.api", "llm.openrouter"] {
            let (base, connections, probe) = spawn_connection_probe().await;
            let provider = match provider_name {
                "llm.api" => LlmProvider::Api {
                    endpoint: base,
                    api_key: "sk-test".to_string(),
                    model: "test-model".to_string(),
                },
                "llm.openrouter" => LlmProvider::OpenRouter {
                    api_key: "sk-or-test".to_string(),
                    model: "openrouter/test-model".to_string(),
                    base_url: base,
                    provider_order: None,
                    include_usage_in_stream: true,
                },
                _ => unreachable!("test provider list is exhaustive"),
            };
            let request = StreamChatRequest::new(
                provider,
                vec![ChatMessage {
                    role: "user".to_string(),
                    content: "sensitive question".to_string(),
                }],
                "sensitive graph context".to_string(),
                StreamParams::default(),
            )
            .with_content_egress_policy(
                crate::asr::ProviderContentEgressPolicy::block("local_only"),
            );

            let (mut rx, _cancel) = stream_chat_with_request(request);
            match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
                .await
                .expect("blocked policy terminal frame should arrive")
                .expect("terminal frame")
            {
                TokenDelta::Error { message, full_text } => {
                    assert!(full_text.is_empty());
                    assert!(
                        message.contains(&format!(
                            "Privacy policy blocked prompt egress to {provider_name}"
                        )),
                        "blocked egress error should name provider and prompt class, got: {message}"
                    );
                    assert!(
                        !message.contains("sensitive question")
                            && !message.contains("sensitive graph context"),
                        "blocked egress error must not echo prompt content: {message}"
                    );
                }
                other => panic!("expected blocked policy Error frame, got {other:?}"),
            }

            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                    .await
                    .expect("stream should close")
                    .is_none(),
                "blocked stream must end after exactly one terminal frame"
            );
            probe.await.expect("connection probe task should finish");
            assert_eq!(
                connections.load(Ordering::SeqCst),
                0,
                "blocked policy must not open a connection for {provider_name}"
            );
        }
    }

    #[test]
    fn default_content_egress_prevents_cloud_stream_request_build() {
        for provider_name in ["llm.api", "llm.openrouter"] {
            let provider = match provider_name {
                "llm.api" => LlmProvider::Api {
                    endpoint: "http://127.0.0.1:1/v1".to_string(),
                    api_key: "sk-test".to_string(),
                    model: "test-model".to_string(),
                },
                "llm.openrouter" => LlmProvider::OpenRouter {
                    api_key: "sk-or-test".to_string(),
                    model: "openrouter/test-model".to_string(),
                    base_url: "http://127.0.0.1:1/v1".to_string(),
                    provider_order: None,
                    include_usage_in_stream: true,
                },
                _ => unreachable!("test provider list is exhaustive"),
            };
            let request = StreamChatRequest::new(
                provider,
                vec![ChatMessage {
                    role: "user".to_string(),
                    content: "sensitive question".to_string(),
                }],
                "sensitive graph context".to_string(),
                StreamParams::default(),
            );

            let err = match build_request_for_provider(&request) {
                Err(err) => err,
                Ok(_) => panic!("default policy must reject before building a cloud request"),
            };
            assert!(
                err.contains(&format!(
                    "Privacy policy blocked prompt egress to {provider_name}"
                )),
                "default egress error should name provider and prompt class, got: {err}"
            );
            assert!(err.contains("explicit_policy_required"), "got: {err}");
            assert!(
                !err.contains("sensitive question") && !err.contains("sensitive graph context"),
                "default egress error must not echo prompt content: {err}"
            );
        }
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

        let (mut rx, cancel) = stream_chat_with_request(allowed_stream_request(
            provider,
            vec![],
            "ctx".to_string(),
            StreamParams::default(),
        ));

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
    async fn local_llama_stream_chat_requires_explicit_backend_handle() {
        let provider = LlmProvider::LocalLlama;
        let (mut rx, _cancel) =
            stream_chat(provider, vec![], String::new(), StreamParams::default());
        let frame = rx.recv().await.expect("at least one terminal frame");
        match frame {
            TokenDelta::Error { message, .. } => {
                assert!(
                    message.contains("StreamBackendHandles.local_llama"),
                    "error must name the missing explicit LocalLlama handle, got: {message}"
                );
            }
            other => panic!("expected Error for LocalLlama, got {other:?}"),
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("stream should close")
                .is_none(),
            "missing-handle stream must end after exactly one terminal frame"
        );
    }

    #[tokio::test]
    async fn local_llama_stream_chat_reports_unloaded_engine_from_handle() {
        let request = StreamChatRequest::new(
            LlmProvider::LocalLlama,
            vec![],
            String::new(),
            StreamParams::default(),
        )
        .with_backend_handles(StreamBackendHandles {
            local_llama: Some(Arc::new(Mutex::new(None))),
            ..StreamBackendHandles::empty()
        });

        let (mut rx, _cancel) = stream_chat_with_request(request);
        let frame = rx.recv().await.expect("at least one terminal frame");
        match frame {
            TokenDelta::Error { message, .. } => {
                assert!(
                    message.contains("LocalLlama engine is not loaded"),
                    "error must name the unloaded local engine, got: {message}"
                );
            }
            other => panic!("expected Error for unloaded LocalLlama, got {other:?}"),
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("stream should close")
                .is_none(),
            "unloaded-engine stream must end after exactly one terminal frame"
        );
    }

    #[cfg(feature = "llm-llama")]
    fn local_llama_request(
        handle: Arc<Mutex<Option<crate::llm::engine::LlmEngine>>>,
    ) -> StreamChatRequest {
        StreamChatRequest::new(
            LlmProvider::LocalLlama,
            vec![ChatMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            "graph context".to_string(),
            StreamParams::default(),
        )
        .with_backend_handles(StreamBackendHandles {
            local_llama: Some(handle),
            ..StreamBackendHandles::empty()
        })
    }

    #[cfg(feature = "llm-llama")]
    #[tokio::test]
    async fn local_llama_stream_chat_emits_multiple_deltas_and_done_from_engine_loop() {
        let engine = crate::llm::engine::LlmEngine::test_with_stream_pieces(
            vec!["local".to_string(), " answer".to_string(), ".".to_string()],
            std::time::Duration::ZERO,
            14,
        );
        let request = local_llama_request(Arc::new(Mutex::new(Some(engine))));

        let (mut rx, _cancel) = stream_chat_with_request(request);
        let mut deltas = Vec::new();
        let mut done = None;
        while let Some(frame) = rx.recv().await {
            match frame {
                TokenDelta::Delta {
                    content,
                    finish_reason,
                } => {
                    assert_eq!(finish_reason, None);
                    deltas.push(content);
                }
                TokenDelta::Done {
                    full_text,
                    usage,
                    finish_reason,
                } => {
                    done = Some((full_text, usage, finish_reason));
                    break;
                }
                TokenDelta::Error { message, .. } => panic!("unexpected local error: {message}"),
                TokenDelta::Cancelled { .. } => panic!("unexpected local cancel"),
            }
        }

        assert_eq!(deltas, vec!["local", " answer", "."]);
        let (full_text, usage, finish_reason) = done.expect("local stream done frame");
        assert_eq!(full_text, "local answer.");
        assert_eq!(finish_reason, "stop");
        let usage = usage.expect("local usage on done");
        assert_eq!(usage.prompt_tokens, Some(14));
        assert_eq!(usage.completion_tokens, Some(3));
        assert_eq!(usage.total_tokens, Some(17));
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("stream should close")
                .is_none(),
            "local stream must end after exactly one terminal frame"
        );
    }

    #[cfg(feature = "llm-llama")]
    #[tokio::test]
    async fn local_llama_stream_chat_cancel_before_first_token_returns_cancelled() {
        let engine = crate::llm::engine::LlmEngine::test_with_stream_pieces(
            vec!["late".to_string()],
            std::time::Duration::from_millis(50),
            4,
        );
        let request = local_llama_request(Arc::new(Mutex::new(Some(engine))));

        let (mut rx, cancel) = stream_chat_with_request(request);
        cancel.cancel();

        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("cancel frame should arrive")
            .expect("cancel frame")
        {
            TokenDelta::Cancelled { full_text } => assert!(full_text.is_empty()),
            other => panic!("expected cancel-before-token terminal frame, got {other:?}"),
        }
    }

    #[cfg(feature = "llm-llama")]
    #[tokio::test]
    async fn local_llama_stream_chat_cancel_between_tokens_frees_engine_for_next_request() {
        let engine = crate::llm::engine::LlmEngine::test_with_stream_pieces(
            vec!["one".to_string(), " two".to_string(), " three".to_string()],
            std::time::Duration::from_millis(100),
            8,
        );
        let handle = Arc::new(Mutex::new(Some(engine)));

        let (mut first_rx, first_cancel) =
            stream_chat_with_request(local_llama_request(handle.clone()));
        match tokio::time::timeout(std::time::Duration::from_secs(2), first_rx.recv())
            .await
            .expect("first delta should arrive")
            .expect("first delta")
        {
            TokenDelta::Delta { content, .. } => assert_eq!(content, "one"),
            other => panic!("expected first delta before cancellation, got {other:?}"),
        }
        first_cancel.cancel();

        let mut cancelled = None;
        while let Some(frame) =
            tokio::time::timeout(std::time::Duration::from_secs(2), first_rx.recv())
                .await
                .expect("cancelled terminal should arrive")
        {
            match frame {
                TokenDelta::Delta { .. } => continue,
                TokenDelta::Cancelled { full_text } => {
                    cancelled = Some(full_text);
                    break;
                }
                TokenDelta::Done { .. } => panic!("cancelled stream must not finish as done"),
                TokenDelta::Error { message, .. } => {
                    panic!("cancelled stream must not error: {message}")
                }
            }
        }
        assert_eq!(cancelled.as_deref(), Some("one"));

        let (mut second_rx, _second_cancel) = stream_chat_with_request(local_llama_request(handle));
        let mut second_deltas = Vec::new();
        while let Some(frame) =
            tokio::time::timeout(std::time::Duration::from_secs(2), second_rx.recv())
                .await
                .expect("second stream should not block behind cancelled request")
        {
            match frame {
                TokenDelta::Delta { content, .. } => second_deltas.push(content),
                TokenDelta::Done {
                    full_text, usage, ..
                } => {
                    assert_eq!(full_text, "one two three");
                    assert_eq!(usage.and_then(|u| u.total_tokens), Some(11));
                    break;
                }
                TokenDelta::Cancelled { .. } => panic!("second stream should complete"),
                TokenDelta::Error { message, .. } => {
                    panic!("second stream should not error: {message}")
                }
            }
        }
        assert_eq!(second_deltas, vec!["one", " two", " three"]);
    }

    #[cfg(feature = "llm-llama")]
    #[tokio::test]
    async fn local_llama_stream_chat_maps_engine_error_to_terminal_error() {
        let engine = crate::llm::engine::LlmEngine::test_with_stream_error("local stream failed");
        let request = local_llama_request(Arc::new(Mutex::new(Some(engine))));

        let (mut rx, _cancel) = stream_chat_with_request(request);
        match rx.recv().await.expect("error frame") {
            TokenDelta::Error { message, full_text } => {
                assert_eq!(message, "local stream failed");
                assert!(full_text.is_empty());
            }
            other => panic!("expected local Error frame, got {other:?}"),
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("stream should close")
                .is_none(),
            "local error stream must end after exactly one terminal frame"
        );
    }

    // ------------------------------------------------------------------
    // MistralRs streaming dispatch + drain
    //
    // The mistral.rs engine is model-backed and cannot generate without a loaded
    // GGUF, so these tests exercise the model-free seams: the dispatch guards
    // (missing handle / cancel-before-start) and the shared drain loop
    // (`drain_engine_stream_events`) fed synthetic `LlmStreamEvent`s. They run on
    // every build (no `llm-mistralrs` feature gate needed: a missing-or-unloaded
    // handle and a synthetic event feed touch no engine internals).
    // ------------------------------------------------------------------

    fn mistralrs_request(handles: StreamBackendHandles) -> StreamChatRequest {
        StreamChatRequest::new(
            LlmProvider::MistralRs {
                model_id: "mistralrs-test-model".to_string(),
            },
            vec![ChatMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            "graph context".to_string(),
            StreamParams::default(),
        )
        .with_backend_handles(handles)
    }

    #[tokio::test]
    async fn mistralrs_stream_requires_explicit_backend_handle() {
        // No `mistralrs_engine` handle on the request -> single terminal Error
        // naming the missing explicit handle.
        let request = mistralrs_request(StreamBackendHandles::empty());
        let (mut rx, _cancel) = stream_chat_with_request(request);
        match rx.recv().await.expect("at least one terminal frame") {
            TokenDelta::Error { message, .. } => assert!(
                message.contains("StreamBackendHandles.mistralrs_engine"),
                "error must name the missing explicit MistralRs handle, got: {message}"
            ),
            other => panic!("expected Error for MistralRs, got {other:?}"),
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("stream should close")
                .is_none(),
            "missing-handle stream must end after exactly one terminal frame"
        );
    }

    #[tokio::test]
    async fn mistralrs_stream_reports_unloaded_engine_from_handle() {
        // Handle present but `None` inside the mutex -> single terminal Error
        // naming the unloaded engine.
        let request = mistralrs_request(StreamBackendHandles {
            mistralrs_engine: Some(Arc::new(Mutex::new(None))),
            ..StreamBackendHandles::empty()
        });
        let (mut rx, _cancel) = stream_chat_with_request(request);
        match rx.recv().await.expect("at least one terminal frame") {
            TokenDelta::Error { message, .. } => assert!(
                message.contains("MistralRs engine is not loaded"),
                "error must name the unloaded MistralRs engine, got: {message}"
            ),
            other => panic!("expected Error for unloaded MistralRs, got {other:?}"),
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("stream should close")
                .is_none(),
            "unloaded-engine stream must end after exactly one terminal frame"
        );
    }

    #[tokio::test]
    async fn mistralrs_stream_cancel_before_start_returns_cancelled() {
        // A token cancelled before the stream starts short-circuits to a single
        // Cancelled terminal with no partial text (mirrors the local path).
        let request = mistralrs_request(StreamBackendHandles {
            mistralrs_engine: Some(Arc::new(Mutex::new(None))),
            ..StreamBackendHandles::empty()
        });
        let (mut rx, cancel) = stream_chat_with_request(request);
        cancel.cancel();
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("cancel frame should arrive")
            .expect("cancel frame")
        {
            TokenDelta::Cancelled { full_text } => assert!(full_text.is_empty()),
            // The cancel and dispatch race: an unloaded-engine Error is also an
            // acceptable single terminal here. Either way it must be terminal.
            TokenDelta::Error { .. } => {}
            other => panic!("expected cancel/error terminal, got {other:?}"),
        }
    }

    /// Drain helper: a run of `Delta`s followed by one `Done` maps to a run of
    /// `TokenDelta::Delta` then exactly one `TokenDelta::Done` with split usage.
    #[tokio::test]
    async fn drain_engine_stream_events_maps_deltas_then_single_done() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (tx, mut rx) = mpsc::channel(8);
        let metadata = StreamContextMetadata::from_provider(&LlmProvider::MistralRs {
            model_id: "m".to_string(),
        });

        let drain = tokio::spawn(async move {
            drain_engine_stream_events(event_rx, &tx, metadata, "ended without terminal").await;
        });

        event_tx
            .send(LlmStreamEvent::Delta {
                content: "mis".to_string(),
            })
            .await
            .unwrap();
        event_tx
            .send(LlmStreamEvent::Delta {
                content: "tral".to_string(),
            })
            .await
            .unwrap();
        event_tx
            .send(LlmStreamEvent::Done {
                full_text: "mistral".to_string(),
                prompt_tokens: 9,
                completion_tokens: 2,
                total_tokens: 11,
            })
            .await
            .unwrap();
        drop(event_tx);

        let mut deltas = Vec::new();
        let mut done = None;
        while let Some(frame) = rx.recv().await {
            match frame {
                TokenDelta::Delta { content, .. } => deltas.push(content),
                TokenDelta::Done {
                    full_text, usage, ..
                } => {
                    done = Some((full_text, usage));
                    break;
                }
                other => panic!("unexpected frame: {other:?}"),
            }
        }
        drain.await.unwrap();

        assert_eq!(deltas, vec!["mis", "tral"]);
        let (full_text, usage) = done.expect("done frame");
        assert_eq!(full_text, "mistral");
        let usage = usage.expect("usage on done");
        assert_eq!(usage.prompt_tokens, Some(9));
        assert_eq!(usage.completion_tokens, Some(2));
        assert_eq!(usage.total_tokens, Some(11));
        assert!(
            rx.recv().await.is_none(),
            "drain must end after exactly one terminal frame"
        );
    }

    /// Drain helper: a `Cancelled` engine event maps to a single
    /// `TokenDelta::Cancelled` carrying the partial text.
    #[tokio::test]
    async fn drain_engine_stream_events_maps_cancelled() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (tx, mut rx) = mpsc::channel(8);
        let metadata = StreamContextMetadata::from_provider(&LlmProvider::MistralRs {
            model_id: "m".to_string(),
        });
        let drain = tokio::spawn(async move {
            drain_engine_stream_events(event_rx, &tx, metadata, "ended without terminal").await;
        });

        event_tx
            .send(LlmStreamEvent::Delta {
                content: "part".to_string(),
            })
            .await
            .unwrap();
        event_tx
            .send(LlmStreamEvent::Cancelled {
                full_text: "part".to_string(),
            })
            .await
            .unwrap();
        drop(event_tx);

        // Skip the leading delta, assert the terminal is Cancelled.
        let mut cancelled = None;
        while let Some(frame) = rx.recv().await {
            match frame {
                TokenDelta::Delta { .. } => continue,
                TokenDelta::Cancelled { full_text } => {
                    cancelled = Some(full_text);
                    break;
                }
                other => panic!("expected Cancelled terminal, got {other:?}"),
            }
        }
        drain.await.unwrap();
        assert_eq!(cancelled.as_deref(), Some("part"));
        assert!(rx.recv().await.is_none(), "exactly one terminal frame");
    }

    /// Drain helper: an `Error` engine event maps to a single `TokenDelta::Error`
    /// carrying the message and partial text.
    #[tokio::test]
    async fn drain_engine_stream_events_maps_error() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (tx, mut rx) = mpsc::channel(8);
        let metadata = StreamContextMetadata::from_provider(&LlmProvider::MistralRs {
            model_id: "m".to_string(),
        });
        let drain = tokio::spawn(async move {
            drain_engine_stream_events(event_rx, &tx, metadata, "ended without terminal").await;
        });

        event_tx
            .send(LlmStreamEvent::Error {
                message: "mistral stream failed".to_string(),
                full_text: "so far".to_string(),
            })
            .await
            .unwrap();
        drop(event_tx);

        match rx.recv().await.expect("error frame") {
            TokenDelta::Error { message, full_text } => {
                assert_eq!(message, "mistral stream failed");
                assert_eq!(full_text, "so far");
            }
            other => panic!("expected Error terminal, got {other:?}"),
        }
        drain.await.unwrap();
        assert!(rx.recv().await.is_none(), "exactly one terminal frame");
    }

    /// Drain helper: a channel that closes with no terminal event surfaces a
    /// single defensive `Error` so the consumer never blocks forever.
    #[tokio::test]
    async fn drain_engine_stream_events_no_terminal_yields_defensive_error() {
        let (event_tx, event_rx) = mpsc::channel(8);
        let (tx, mut rx) = mpsc::channel(8);
        let metadata = StreamContextMetadata::from_provider(&LlmProvider::MistralRs {
            model_id: "m".to_string(),
        });
        let drain = tokio::spawn(async move {
            drain_engine_stream_events(event_rx, &tx, metadata, "engine dropped its sender").await;
        });

        // Close the channel immediately with no terminal frame.
        drop(event_tx);

        match rx.recv().await.expect("defensive error frame") {
            TokenDelta::Error { message, full_text } => {
                assert_eq!(message, "engine dropped its sender");
                assert!(full_text.is_empty());
            }
            other => panic!("expected defensive Error terminal, got {other:?}"),
        }
        drain.await.unwrap();
        assert!(rx.recv().await.is_none(), "exactly one terminal frame");
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

    /// AUD-STR1 P1: starting a new stream must cancel every prior live one so
    /// the registry never holds an orphaned entry the frontend can no longer
    /// reach (it tracks only one `streamingChatRequestId`).
    #[test]
    fn registry_cancel_all_fires_every_live_token() {
        let reg = StreamRegistry::new();
        let t1 = CancellationToken::new();
        let t2 = CancellationToken::new();
        reg.register("req-a".into(), t1.clone());
        reg.register("req-b".into(), t2.clone());

        assert_eq!(
            reg.cancel_all(),
            2,
            "must report every live stream cancelled"
        );
        assert!(t1.is_cancelled(), "prior stream a must be cancelled");
        assert!(t2.is_cancelled(), "prior stream b must be cancelled");

        // Registry is now empty: cancel_all is a no-op, and the old ids are
        // gone so a later targeted cancel finds nothing.
        assert_eq!(
            reg.cancel_all(),
            0,
            "cancel_all on empty registry is a no-op"
        );
        assert!(
            !reg.cancel("req-a"),
            "drained id must be unknown afterwards"
        );
    }

    #[test]
    fn stream_chat_request_carries_explicit_backend_handles_and_metadata() {
        let provider = LlmProvider::MistralRs {
            model_id: "mistralrs-test-model".to_string(),
        };
        let handles = StreamBackendHandles {
            mistralrs_engine: Some(Arc::new(Mutex::new(None))),
            ..StreamBackendHandles::empty()
        };

        let request = StreamChatRequest::new(
            provider.clone(),
            vec![ChatMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            "ctx".to_string(),
            StreamParams::default(),
        )
        .with_backend_handles(handles)
        .with_source_metadata(StreamSourceMetadata {
            session_id: Some("session-1".to_string()),
            source_id: Some("source-mic".to_string()),
            request_id: Some("request-1".to_string()),
        })
        .with_context_id("graph-context:session-1");

        assert!(request.backend_handles.has_handle_for(&provider));
        assert_eq!(request.metadata.backend.provider, "MistralRs");
        assert_eq!(
            request.metadata.backend.model.as_deref(),
            Some("mistralrs-test-model")
        );
        assert_eq!(
            request.metadata.source.session_id.as_deref(),
            Some("session-1")
        );
        assert_eq!(
            request.metadata.context_id.as_deref(),
            Some("graph-context:session-1")
        );
        // `build_request_for_provider` still returns `None` for MistralRs — but
        // NOT because streaming is deferred: MistralRs is dispatched by
        // `run_mistralrs_stream` via the early-return in
        // `stream_chat_with_request`, so the HTTP/SSE request builder is never
        // consulted for it. The `None` here just confirms the local engine path
        // owns this provider rather than the cloud egress path.
        assert!(
            build_request_for_provider(&request)
                .expect("local engine providers do not require cloud egress")
                .is_none(),
            "MistralRs is dispatched to run_mistralrs_stream before the HTTP/SSE builder, so the builder yields None"
        );
    }

    fn aws_bedrock_provider() -> LlmProvider {
        LlmProvider::AwsBedrock {
            region: "us-west-2".to_string(),
            model_id: "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
            credential_source: crate::settings::AwsCredentialSource::DefaultChain,
        }
    }

    /// AwsBedrock is a non-SSE adapter: it must NOT be routed through the SSE
    /// request builder. It stays in the `=> None` arm so the dispatch branches
    /// to the ConverseStream adapter before `build_request_for_provider`.
    #[test]
    fn aws_bedrock_is_not_an_sse_request_provider() {
        let request = StreamChatRequest::new(
            aws_bedrock_provider(),
            vec![ChatMessage {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            "ctx".to_string(),
            StreamParams::default(),
        )
        .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        assert!(
            build_request_for_provider(&request)
                .expect("AwsBedrock does not build an SSE request")
                .is_none(),
            "AwsBedrock must stay in the SSE builder's None arm — it uses the ConverseStream adapter"
        );
        assert_eq!(request.metadata.backend.provider, "AwsBedrock");
    }

    /// The Bedrock dispatch branch must enforce the content-egress policy before
    /// constructing the SDK client / making any cloud call. With the default
    /// (block) policy the stream must terminate with a single Error frame and
    /// never reach AWS. This mirrors the SSE providers' egress gate test and
    /// requires no live AWS credentials.
    #[tokio::test]
    async fn aws_bedrock_blocked_egress_rejects_before_cloud_call() {
        let request = StreamChatRequest::new(
            aws_bedrock_provider(),
            vec![ChatMessage {
                role: "user".to_string(),
                content: "sensitive question".to_string(),
            }],
            "sensitive graph context".to_string(),
            StreamParams::default(),
        )
        .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::block("local_only"));

        let (mut rx, _cancel) = stream_chat_with_request(request);
        match tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("blocked Bedrock terminal frame should arrive")
            .expect("terminal frame")
        {
            TokenDelta::Error { message, full_text } => {
                assert!(full_text.is_empty());
                assert!(
                    message.contains("Privacy policy blocked prompt egress to llm.aws_bedrock"),
                    "blocked Bedrock egress error should name the provider, got: {message}"
                );
                assert!(
                    !message.contains("sensitive question")
                        && !message.contains("sensitive graph context"),
                    "blocked egress error must not echo prompt content: {message}"
                );
            }
            other => panic!("expected blocked-egress Error frame, got {other:?}"),
        }
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
                .await
                .expect("stream should close")
                .is_none(),
            "blocked Bedrock stream must end after exactly one terminal frame"
        );
    }

    #[test]
    fn terminal_event_maps_to_legacy_token_delta() {
        let metadata = StreamContextMetadata::from_provider(&LlmProvider::Api {
            endpoint: "http://localhost:11434/v1".to_string(),
            api_key: String::new(),
            model: "llama3.2".to_string(),
        });
        let usage = Some(StreamUsage {
            prompt_tokens: Some(3),
            completion_tokens: Some(4),
            total_tokens: Some(7),
        });

        match TokenDelta::from_terminal_event(StreamTerminalEvent::done(
            "answer".to_string(),
            usage.clone(),
            "stop".to_string(),
            metadata.clone(),
        )) {
            TokenDelta::Done {
                full_text,
                usage: got_usage,
                finish_reason,
            } => {
                assert_eq!(full_text, "answer");
                assert_eq!(got_usage, usage);
                assert_eq!(finish_reason, "stop");
            }
            other => panic!("expected Done terminal mapping, got {other:?}"),
        }

        match TokenDelta::from_terminal_event(StreamTerminalEvent::error(
            "provider failed".to_string(),
            "partial".to_string(),
            metadata.clone(),
        )) {
            TokenDelta::Error { message, full_text } => {
                assert_eq!(message, "provider failed");
                assert_eq!(full_text, "partial");
            }
            other => panic!("expected Error terminal mapping, got {other:?}"),
        }

        match TokenDelta::from_terminal_event(StreamTerminalEvent::cancelled(
            "partial".to_string(),
            metadata,
        )) {
            TokenDelta::Cancelled { full_text } => assert_eq!(full_text, "partial"),
            other => panic!("expected Cancelled terminal mapping, got {other:?}"),
        }
    }

    /// AUD-STR1 P2: the user-configured `max_tokens` / `temperature` must flow
    /// into the wire request body, not the streaming path's own literals.
    #[test]
    fn build_request_threads_configured_sampling_params() {
        let provider = LlmProvider::Api {
            endpoint: "http://localhost:1234/v1".to_string(),
            api_key: "sk-test".to_string(),
            model: "test-model".to_string(),
        };
        let params = StreamParams {
            max_tokens: 4096,
            temperature: 0.9,
        };
        let request = allowed_stream_request(provider, vec![], "ctx".to_string(), params);
        let req = build_request_for_provider(&request)
            .expect("explicit allow permits request build")
            .expect("Api provider builds a request");
        assert_eq!(
            req.body["max_tokens"], 4096,
            "configured max_tokens must reach the request body, not the old 512 literal"
        );
        // f32 0.9 widens to ~0.8999999761 as JSON f64, so compare within an
        // epsilon rather than against the exact 0.9 literal.
        assert!(
            (req.body["temperature"]
                .as_f64()
                .expect("temperature is a number")
                - 0.9)
                .abs()
                < 1e-6,
            "configured temperature must reach the request body, not the old 0.7 literal"
        );

        // Same for OpenRouter.
        let or_provider = LlmProvider::OpenRouter {
            model: "anthropic/claude-sonnet-4.5".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            provider_order: None,
            include_usage_in_stream: true,
            api_key: "sk-or".to_string(),
        };
        let or_request = allowed_stream_request(or_provider, vec![], "ctx".to_string(), params);
        let or_req = build_request_for_provider(&or_request)
            .expect("explicit allow permits OpenRouter request build")
            .expect("OpenRouter provider builds a request");
        assert_eq!(or_req.body["max_tokens"], 4096);
        assert!(
            (or_req.body["temperature"]
                .as_f64()
                .expect("temperature is a number")
                - 0.9)
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn openrouter_streaming_provider_matches_blocking_provider_serializer() {
        let config = OpenRouterConfig {
            api_key: "sk-or".to_string(),
            model: "anthropic/claude-sonnet-4.5".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            provider_order: Some(vec!["anthropic".to_string(), "openai".to_string()]),
            routing_policy: None,
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 512,
            temperature: 0.1,
        };

        let streaming = build_openrouter_request(&config, &[], "ctx");
        let blocking_provider =
            crate::llm::openrouter::blocking_chat_provider_value_for_test(&config)
                .expect("blocking provider value");

        assert_eq!(
            streaming.body.get("provider"),
            Some(&blocking_provider),
            "streaming and blocking OpenRouter requests must share the provider object serializer"
        );

        let rich_config = OpenRouterConfig {
            provider_order: Some(vec!["legacy-provider".to_string()]),
            routing_policy: Some(OpenRouterRoutingPolicy {
                order: vec!["cerebras".to_string(), "groq".to_string()],
                only: vec!["cerebras".to_string(), "groq".to_string()],
                allow_fallbacks: Some(false),
                ..OpenRouterRoutingPolicy::default()
            }),
            ..config.clone()
        };
        let rich_streaming = build_openrouter_request(&rich_config, &[], "ctx");
        let rich_blocking_provider =
            crate::llm::openrouter::blocking_chat_provider_value_for_test(&rich_config)
                .expect("rich blocking provider value");
        let expected_rich = serde_json::json!({
            "order": ["cerebras", "groq"],
            "only": ["cerebras", "groq"],
            "allow_fallbacks": false
        });
        assert_eq!(rich_blocking_provider, expected_rich);
        assert_eq!(
            rich_streaming.body.get("provider"),
            Some(&rich_blocking_provider),
            "rich routing policy must serialize identically for streaming and blocking requests"
        );

        let empty_config = OpenRouterConfig {
            provider_order: None,
            routing_policy: None,
            ..config
        };
        let streaming = build_openrouter_request(&empty_config, &[], "ctx");
        assert!(
            streaming.body.get("provider").is_none(),
            "empty routing must omit provider in streaming requests"
        );
        assert_eq!(
            crate::llm::openrouter::blocking_chat_provider_value_for_test(&empty_config),
            None,
            "empty routing must omit provider in blocking requests"
        );
    }

    #[test]
    fn openrouter_stream_request_prefers_synced_rich_policy_over_provider_order() {
        let rich_config = OpenRouterConfig {
            api_key: "sk-or".to_string(),
            model: "anthropic/claude-sonnet-4.5".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            provider_order: Some(vec!["legacy-provider".to_string()]),
            routing_policy: Some(OpenRouterRoutingPolicy {
                order: vec!["cerebras".to_string(), "groq".to_string()],
                only: vec!["cerebras".to_string(), "groq".to_string()],
                allow_fallbacks: Some(false),
                ..OpenRouterRoutingPolicy::default()
            }),
            include_usage_in_stream: true,
            http_referer: DEFAULT_HTTP_REFERER.to_string(),
            app_title: DEFAULT_APP_TITLE.to_string(),
            max_tokens: 512,
            temperature: 0.1,
        };
        let openrouter_client = crate::llm::openrouter::OpenRouterClient::new(rich_config)
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        let handles = StreamBackendHandles {
            openrouter_client: Some(Arc::new(Mutex::new(Some(openrouter_client)))),
            ..StreamBackendHandles::empty()
        };
        let provider = LlmProvider::OpenRouter {
            api_key: "sk-or".to_string(),
            model: "anthropic/claude-sonnet-4.5".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            provider_order: Some(vec!["legacy-provider".to_string()]),
            include_usage_in_stream: true,
        };

        let request =
            StreamChatRequest::new(provider, vec![], "ctx".to_string(), StreamParams::default())
                .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow())
                .with_backend_handles(handles);
        let stream_request = build_request_for_provider(&request)
            .expect("request builds")
            .expect("OpenRouter provider builds a stream request");

        assert_eq!(
            stream_request.body.get("provider"),
            Some(&serde_json::json!({
                "order": ["cerebras", "groq"],
                "only": ["cerebras", "groq"],
                "allow_fallbacks": false
            })),
            "streaming request construction must prefer synced rich policy over legacy provider_order"
        );
    }

    /// AUD-STR1 P3: a real `usage` block earlier in the stream must NOT be
    /// clobbered by a trailing keepalive chunk that carries an all-null
    /// `usage{}` (some providers emit one right before `[DONE]`).
    #[tokio::test]
    async fn usage_not_clobbered_by_trailing_null_usage_chunk() {
        // Real usage on the content chunk, then a trailer with usage{} (all
        // null) — last-writer-wins would zero out the real count.
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":4,\"total_tokens\":11}}\n\n\
                    data: {\"choices\":[],\"usage\":{}}\n\n\
                    data: [DONE]\n\n";
        let base = spawn_sse_mock(body).await;
        let provider = api_provider(base);

        let (mut rx, _cancel) = stream_chat_with_request(allowed_stream_request(
            provider,
            vec![],
            "ctx".to_string(),
            StreamParams::default(),
        ));

        let mut done_usage: Option<StreamUsage> = None;
        while let Some(frame) = rx.recv().await {
            match frame {
                TokenDelta::Delta { .. } => continue,
                TokenDelta::Done { usage, .. } => {
                    done_usage = usage;
                    break;
                }
                TokenDelta::Error { message, .. } => panic!("unexpected error: {message}"),
                TokenDelta::Cancelled { .. } => panic!("unexpected cancel"),
            }
        }

        let u = done_usage.expect("real usage must survive the trailing null-usage chunk");
        assert_eq!(
            u.total_tokens,
            Some(11),
            "trailing usage{{}} must not clobber the real total_tokens"
        );
    }

    /// SSE mock whose body is owned (not `&'static`) so a test can stream an
    /// arbitrarily large frame. Mirrors [`spawn_sse_mock`] otherwise: drains
    /// the request headers, writes a 200 `text/event-stream` response with the
    /// supplied body, then closes.
    async fn spawn_sse_mock_owned(body: String) -> String {
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

    /// Drives the SSE `SseEvent::Error` arm (streaming.rs `run_sse_stream`,
    /// the `redacted_error_excerpt` call on the decoder error message) by
    /// streaming a single frame larger than the decoder's 1 MiB cap so the
    /// decoder reports an overflow. The frame body embeds an API key, a
    /// `Bearer` token, an AWS access key, and a `?token=` URL credential; the
    /// provider's `sk-` api_key is registered as a request secret. The
    /// terminal `TokenDelta::Error` must be the metadata-only overflow
    /// diagnostic with none of the injected credentials echoed back.
    #[tokio::test]
    async fn stream_chat_sse_error_event_redacts_provider_secrets() {
        let api_key = "sk-sse-error-provider-secret-12345";
        // One oversized `data:` frame: >1 MiB of filler with secrets sprinkled
        // in, and crucially NO blank-line (`\n\n`) terminator so the decoder
        // trips its frame-size cap and yields `SseEvent::Error`.
        let secrets_blob = concat!(
            "Bearer bearer-sse-secret-12345 ",
            "aws=AKIA1234567890ABCDEF ",
            "url=https://provider.example/v1?token=sse-query-secret-12345 "
        );
        let mut body = String::with_capacity(1_200_000);
        body.push_str("data: ");
        body.push_str(api_key);
        body.push(' ');
        body.push_str(secrets_blob);
        body.push_str(&"x".repeat(1_200_000));
        let base = spawn_sse_mock_owned(body).await;
        let provider = LlmProvider::Api {
            endpoint: base,
            api_key: api_key.to_string(),
            model: "test-model".to_string(),
        };

        let (mut rx, _cancel) = stream_chat_with_request(allowed_stream_request(
            provider,
            vec![],
            "graph context".to_string(),
            StreamParams::default(),
        ));

        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("SSE error terminal frame should arrive")
            .expect("terminal frame")
        {
            TokenDelta::Error { message, .. } => {
                assert!(
                    message.contains("SSE frame exceeded"),
                    "SSE error path must surface the decoder overflow diagnostic, got: {message}"
                );
                for leaked in [
                    api_key,
                    "bearer-sse-secret-12345",
                    "AKIA1234567890ABCDEF",
                    "sse-query-secret-12345",
                ] {
                    assert!(
                        !message.contains(leaked),
                        "SSE error frame leaked {leaked}: {message}"
                    );
                }
            }
            other => panic!("expected SSE error terminal frame, got {other:?}"),
        }
    }

    /// Drives the transport-error arm (streaming.rs `run_sse_stream`, the
    /// `req.send()` failure -> `redacted_error_excerpt` call) by pointing the
    /// request at an unroutable endpoint whose URL embeds userinfo and a
    /// `?api_key=` query credential, with the provider `sk-` api_key
    /// registered as a request secret. reqwest's transport error Displays the
    /// request URL, so the terminal `TokenDelta::Error` must redact every
    /// registered/pattern secret before it reaches UI/log surfaces.
    #[tokio::test]
    async fn stream_chat_transport_error_redacts_registered_secrets() {
        let api_key = "sk-transport-provider-secret-12345";
        // Port 1 is unroutable for an HTTP client; the connect attempt fails
        // fast. The endpoint embeds userinfo + a query credential so the URL
        // (echoed by reqwest's error Display) carries multiple secret shapes.
        let endpoint = format!(
            "http://svc-user:{api_key}@127.0.0.1:1/v1?api_key=transport-query-secret-12345"
        );
        let provider = LlmProvider::Api {
            endpoint,
            api_key: api_key.to_string(),
            model: "test-model".to_string(),
        };

        let (mut rx, _cancel) = stream_chat_with_request(allowed_stream_request(
            provider,
            vec![],
            "graph context".to_string(),
            StreamParams::default(),
        ));

        match tokio::time::timeout(std::time::Duration::from_secs(15), rx.recv())
            .await
            .expect("transport error terminal frame should arrive")
            .expect("terminal frame")
        {
            TokenDelta::Error { message, full_text } => {
                assert!(full_text.is_empty(), "transport error has no partial text");
                for leaked in [api_key, "svc-user", "transport-query-secret-12345"] {
                    assert!(
                        !message.contains(leaked),
                        "transport error leaked {leaked}: {message}"
                    );
                }
            }
            other => panic!("expected transport Error terminal frame, got {other:?}"),
        }
    }

    /// Drives the mid-stream read-error arm (streaming.rs `run_sse_stream`,
    /// the `byte_stream.next()` -> `Some(Err(e))` branch). The mock declares a
    /// large `Content-Length`, writes a partial SSE delta, then aborts the
    /// connection before satisfying it, forcing reqwest to surface a stream
    /// read error whose Display includes the request URL. The endpoint embeds
    /// userinfo + a query credential and the `sk-` api_key is registered, so
    /// the terminal `TokenDelta::Error` must carry none of them — this guards
    /// the production fix that routes this path through `redacted_error_excerpt`.
    #[tokio::test]
    async fn stream_chat_read_error_redacts_registered_secrets() {
        let api_key = "sk-readerr-provider-secret-12345";
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
                // Promise far more body than we deliver, emit one partial
                // delta, then drop the socket so the client's body read errors.
                let header = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 65536\r\nConnection: close\r\n\r\n";
                let _ = stream.write_all(header.as_bytes()).await;
                let _ = stream
                    .write_all(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n")
                    .await;
                // Abort without flushing the promised Content-Length.
                drop(stream);
            }
        });
        let endpoint = format!(
            "http://svc-user:{api_key}@127.0.0.1:{}/v1?api_key=readerr-query-secret-12345",
            addr.port()
        );
        let provider = LlmProvider::Api {
            endpoint,
            api_key: api_key.to_string(),
            model: "test-model".to_string(),
        };

        let (mut rx, _cancel) = stream_chat_with_request(allowed_stream_request(
            provider,
            vec![],
            "graph context".to_string(),
            StreamParams::default(),
        ));

        // Drain to the terminal frame. We may see the partial "hi" delta first;
        // the connection drop must then surface as a redacted Error (not Done).
        let mut saw_error = false;
        while let Some(frame) = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
            .await
            .expect("read-error stream must terminate")
        {
            match frame {
                TokenDelta::Delta { .. } => continue,
                TokenDelta::Error { message, .. } => {
                    for leaked in [api_key, "svc-user", "readerr-query-secret-12345"] {
                        assert!(
                            !message.contains(leaked),
                            "stream read error leaked {leaked}: {message}"
                        );
                    }
                    saw_error = true;
                    break;
                }
                TokenDelta::Done { .. } => break,
                TokenDelta::Cancelled { .. } => panic!("unexpected cancel"),
            }
        }
        // The under-delivered Content-Length must surface as a read error, so
        // the redacted Error frame is the branch under test. (If a future
        // platform delivered the truncated body as a clean EOF instead, this
        // assertion would flag that the read-error redaction went uncovered.)
        assert!(
            saw_error,
            "truncated Content-Length must surface as a redacted stream read Error"
        );
    }
}
