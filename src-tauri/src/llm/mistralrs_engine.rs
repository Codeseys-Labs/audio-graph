//! LLM inference engine backed by mistral.rs (Candle).
//!
//! Uses mistral.rs for in-process GGUF model inference with structured
//! generation via JSON Schema constraints.  Unlike llama-cpp-2, the
//! mistral.rs `Model` type is `Send + Sync`, so the engine can be shared
//! across threads without creating per-call contexts.
//!
//! Entity extraction uses [`Model::generate_structured`] which derives
//! a JSON Schema from [`ExtractionResult`]'s `schemars::JsonSchema`
//! implementation and constrains the model output automatically.

#[cfg(feature = "llm-mistralrs")]
use std::sync::Arc;

#[cfg(feature = "llm-mistralrs")]
use mistralrs::{GgufModelBuilder, Model, RequestBuilder, TextMessageRole, TextMessages};

use crate::graph::entities::ExtractionResult;
use crate::llm::engine::ChatMessage;
#[cfg(feature = "llm-mistralrs")]
use crate::llm::engine::{LlmChatParams, LlmStreamEvent};
#[cfg(feature = "llm-mistralrs")]
use crate::projection_llm::ProjectionPatchDraft;
#[cfg(feature = "llm-mistralrs")]
use mistralrs::Response;
#[cfg(feature = "llm-mistralrs")]
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Token-usage mapping (response seam, model-free + testable)
// ---------------------------------------------------------------------------

/// Map mistral.rs's `Usage.total_tokens` (`usize`) to the `u32` the chat
/// outcome carries. Realistic chat totals are far below `u32::MAX`; a
/// pathological value saturates rather than wrapping (so it never silently
/// reports a tiny count). Ungated so it can be unit-tested without compiling
/// the heavy `mistralrs` dependency or loading a model.
///
/// On the cloud-only build the `llm-mistralrs` impl is compiled out, so the
/// only caller is the unit test below; `allow(dead_code)` keeps that build
/// (which still exercises the mapping in tests) warning-free.
#[cfg_attr(not(feature = "llm-mistralrs"), allow(dead_code))]
fn usage_total_to_u32(total_tokens: usize) -> u32 {
    u32::try_from(total_tokens).unwrap_or(u32::MAX)
}

/// One decoded step from a streaming `Response::Chunk`.
///
/// mistral.rs 0.8 streams content as `Chunk` frames; the final chunk also sets
/// `choices[0].finish_reason` and the request `usage`. `content` is the delta to
/// forward (possibly empty); `terminal_usage` is `Some((prompt, completion,
/// total))` exactly on the terminal chunk (the token counts already cast to
/// `u32` via [`usage_total_to_u32`]), and `None` on intermediate chunks. Pure +
/// model-free so the drain logic is unit-tested without a loaded GGUF.
#[cfg(feature = "llm-mistralrs")]
#[derive(Debug, PartialEq, Eq)]
struct StreamChunkStep {
    content: String,
    terminal_usage: Option<(u32, u32, u32)>,
}

/// Decode a streaming `ChatCompletionChunkResponse` into a [`StreamChunkStep`].
///
/// A chunk is terminal when its first choice carries a non-null `finish_reason`
/// or the chunk carries `usage` (the final chunk sets both). Terminal usage is
/// reported as `Some((0, 0, 0))` when `finish_reason` is set but `usage` is
/// absent, so the loop still emits a single `Done` rather than waiting forever.
#[cfg(feature = "llm-mistralrs")]
fn interpret_stream_chunk(chunk: mistralrs::ChatCompletionChunkResponse) -> StreamChunkStep {
    let usage = chunk.usage;
    let choice = chunk.choices.into_iter().next();
    let finish_reason = choice.as_ref().and_then(|c| c.finish_reason.clone());
    let content = choice.and_then(|c| c.delta.content).unwrap_or_default();

    let terminal_usage = if finish_reason.is_some() || usage.is_some() {
        Some(usage.map_or((0, 0, 0), |u| {
            (
                usage_total_to_u32(u.prompt_tokens),
                usage_total_to_u32(u.completion_tokens),
                usage_total_to_u32(u.total_tokens),
            )
        }))
    } else {
        None
    };

    StreamChunkStep {
        content,
        terminal_usage,
    }
}

/// Map a single mistral.rs streaming [`Response`] frame to the engine-neutral
/// [`LlmStreamEvent`] the streaming bridge drains.
///
/// This is the model-free seam of the streaming path: it does no I/O, holds no
/// model borrow, and is a pure function over an in-memory `Response`, so the
/// unit tests below exercise the chunk / done / error mapping with synthetic
/// frames and **no loaded GGUF**.
///
/// In practice mistral.rs 0.8 streaming emits only `Response::Chunk` frames, and
/// the drain loop in [`MistralRsEngine::stream_chat`] handles those inline (via
/// [`interpret_stream_chunk`]) so it can detect the terminal chunk
/// (`finish_reason` / `usage`) and synthesize the `Done`. This helper is the
/// loop's fallback for the *non-chunk* variants and the canonical, independently
/// testable per-frame mapping:
///
/// - `Chunk` carries one streamed `choices[0].delta.content` fragment (empty /
///   absent content maps to an empty `Delta`).
/// - `Done` carries the final `Usage` (prompt / completion / total, the latter
///   via the shared [`usage_total_to_u32`] saturating cast).
/// - `ModelError` carries the partial assistant text emitted before the model
///   faulted, so the consumer can surface what was generated.
/// - `InternalError` / `ValidationError` (and the completion / image / speech /
///   raw / embedding variants that a chat stream never emits) map defensively to
///   `Error` with no partial text.
#[cfg(feature = "llm-mistralrs")]
fn mistralrs_response_to_event(resp: Response) -> LlmStreamEvent {
    match resp {
        Response::Chunk(chunk) => {
            let content = chunk
                .choices
                .into_iter()
                .next()
                .and_then(|choice| choice.delta.content)
                .unwrap_or_default();
            LlmStreamEvent::Delta { content }
        }
        Response::Done(response) => {
            let prompt_tokens = usage_total_to_u32(response.usage.prompt_tokens);
            let completion_tokens = usage_total_to_u32(response.usage.completion_tokens);
            let total_tokens = usage_total_to_u32(response.usage.total_tokens);
            let full_text = response
                .choices
                .into_iter()
                .next()
                .and_then(|choice| choice.message.content)
                .unwrap_or_default();
            LlmStreamEvent::Done {
                full_text,
                prompt_tokens,
                completion_tokens,
                total_tokens,
            }
        }
        Response::ModelError(message, partial) => {
            let full_text = partial
                .choices
                .into_iter()
                .next()
                .and_then(|choice| choice.message.content)
                .unwrap_or_default();
            LlmStreamEvent::Error {
                message: format!("mistral.rs model error: {}", message),
                full_text,
            }
        }
        Response::InternalError(err) => LlmStreamEvent::Error {
            message: format!("mistral.rs internal error: {}", err),
            full_text: String::new(),
        },
        Response::ValidationError(err) => LlmStreamEvent::Error {
            message: format!("mistral.rs validation error: {}", err),
            full_text: String::new(),
        },
        // A chat stream never yields the completion / image / speech / raw /
        // embedding variants; treat any of them as a protocol violation rather
        // than silently dropping the frame (which would hang the drain loop
        // waiting for a terminal that never arrives).
        _ => LlmStreamEvent::Error {
            message: "mistral.rs returned an unexpected non-chat stream response".to_string(),
            full_text: String::new(),
        },
    }
}

// ---------------------------------------------------------------------------
// MistralRsEngine
// ---------------------------------------------------------------------------

/// Native LLM engine using mistral.rs (Candle) for GGUF model inference.
///
/// `Model` is `Send + Sync` so this engine can live in shared state without
/// per-call context creation (unlike `LlmEngine` which wraps llama-cpp-2).
///
/// A dedicated tokio runtime is stored alongside the model to bridge
/// async mistral.rs calls into the synchronous speech-processor threads.
#[cfg(feature = "llm-mistralrs")]
pub struct MistralRsEngine {
    model: Model,
    rt: Arc<tokio::runtime::Runtime>,
}

#[cfg(feature = "llm-mistralrs")]
impl MistralRsEngine {
    /// Load a GGUF model from disk (blocking).
    ///
    /// Creates a dedicated tokio runtime for async model loading and
    /// subsequent inference calls.  Use this when calling from
    /// synchronous code (e.g., speech processor initialization threads).
    ///
    /// `model_dir` is the directory containing the model file(s).
    /// `model_filename` is the GGUF file name within that directory.
    pub fn new(model_dir: &str, model_filename: &str) -> Result<Self, String> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .thread_name("mistralrs-rt")
            .build()
            .map_err(|e| format!("Failed to create tokio runtime for mistral.rs: {}", e))?;

        let model = rt
            .block_on(GgufModelBuilder::new(model_dir, vec![model_filename.to_string()]).build())
            .map_err(|e| format!("Failed to build mistral.rs model: {}", e))?;

        log::info!(
            "mistral.rs model loaded from: {}/{}",
            model_dir,
            model_filename
        );

        Ok(Self {
            model,
            rt: Arc::new(rt),
        })
    }

    /// Check if model is loaded and ready.
    pub fn is_loaded(&self) -> bool {
        true // If we constructed successfully, model is loaded
    }

    // ------------------------------------------------------------------
    // Entity extraction (JSON Schema-constrained structured generation)
    // ------------------------------------------------------------------

    /// Extract entities and relations from text using JSON Schema-constrained
    /// structured generation.
    ///
    /// Uses [`Model::generate_structured`] which automatically derives the
    /// JSON Schema from [`ExtractionResult`]'s `schemars::JsonSchema`
    /// implementation, constraining the model to produce valid JSON that
    /// deserializes without error.
    pub fn extract_entities(&self, text: &str, speaker: &str) -> Result<ExtractionResult, String> {
        // ADR-0008 follow-up #1: adopt the shared conversation ontology as the
        // system prompt so the typed vocabulary (entity + relation types,
        // conservative-extraction rules) matches every other backend. The JSON
        // shape is still enforced structurally by `generate_structured`; the
        // ontology guidance steers *which* types/relations the model picks.
        let system_prompt = crate::ontology::extraction_system_prompt();
        let user_prompt = format!(
            r#"Speaker: {}
Text: {}

If no entities are found, return {{"entities": [], "relations": []}}.
Output JSON:"#,
            speaker, text
        );

        let messages = TextMessages::new()
            .add_message(TextMessageRole::System, &system_prompt)
            .add_message(TextMessageRole::User, &user_prompt);

        let result: ExtractionResult = self
            .rt
            .block_on(self.model.generate_structured::<ExtractionResult>(messages))
            .map_err(|e| format!("mistral.rs structured extraction failed: {}", e))?;

        log::debug!(
            "mistral.rs extraction: {} entities, {} relations",
            result.entities.len(),
            result.relations.len()
        );

        Ok(result)
    }

    // ------------------------------------------------------------------
    // Chat
    // ------------------------------------------------------------------

    /// Chat with the LLM, providing graph context in the system prompt.
    ///
    /// Returns the reply text only; use [`Self::chat_with_usage`] when the
    /// caller needs the token count too.
    pub fn chat(&self, messages: &[ChatMessage], graph_context: &str) -> Result<String, String> {
        self.chat_with_usage(messages, graph_context)
            .map(|(text, _tokens)| text)
    }

    /// Chat with the LLM, returning the reply text **and** the token usage the
    /// backend reported.
    ///
    /// `tokens_used` is `ChatCompletionResponse.usage.total_tokens` (prompt +
    /// completion), which mistral.rs always populates (the field is
    /// non-optional in `mistralrs-core`'s `Usage`). It is cast `usize -> u32`;
    /// realistic chat totals are far below `u32::MAX`, and `saturating` keeps a
    /// pathological value from wrapping.
    pub fn chat_with_usage(
        &self,
        messages: &[ChatMessage],
        graph_context: &str,
    ) -> Result<(String, u32), String> {
        let system_prompt = format!(
            "You are a helpful assistant that answers questions about an \
             audio conversation and its knowledge graph. Use the following context from \
             the knowledge graph and recent transcript to answer questions.\n\n\
             Knowledge Graph Context:\n{}",
            graph_context
        );

        let mut text_messages =
            TextMessages::new().add_message(TextMessageRole::System, &system_prompt);

        for msg in messages {
            let role = match msg.role.as_str() {
                "user" => TextMessageRole::User,
                "assistant" => TextMessageRole::Assistant,
                "system" => TextMessageRole::System,
                _ => TextMessageRole::User,
            };
            text_messages = text_messages.add_message(role, &msg.content);
        }

        let response = self
            .rt
            .block_on(self.model.send_chat_request(text_messages))
            .map_err(|e| format!("mistral.rs chat request failed: {}", e))?;

        let tokens_used = usage_total_to_u32(response.usage.total_tokens);

        let text = response
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .ok_or_else(|| "No response content from mistral.rs".to_string())?;

        Ok((text, tokens_used))
    }

    /// Chat with full message history and knowledge graph context.
    pub fn chat_with_history(
        &self,
        messages: &[ChatMessage],
        graph_context: &str,
    ) -> Result<String, String> {
        self.chat(messages, graph_context)
    }

    /// Chat with full message history, surfacing the backend's token usage.
    ///
    /// Internal carrier for the blocking chat path (`executor::chat_mistralrs`)
    /// so it can report a real `tokens_used` instead of a hard-coded 0.
    pub fn chat_with_history_usage(
        &self,
        messages: &[ChatMessage],
        graph_context: &str,
    ) -> Result<(String, u32), String> {
        self.chat_with_usage(messages, graph_context)
    }

    /// Generate a ProjectionPatchDraft with mistral.rs JSON Schema constraints.
    ///
    /// `generate_structured` returns the parsed value but not token usage, so
    /// the usage count is reported as 0 rather than fabricated.
    pub fn projection_patch_draft_with_usage(
        &self,
        messages: &[ChatMessage],
    ) -> Result<(String, u32), String> {
        let mut text_messages = TextMessages::new();
        for msg in messages {
            let role = match msg.role.as_str() {
                "user" => TextMessageRole::User,
                "assistant" => TextMessageRole::Assistant,
                "system" => TextMessageRole::System,
                _ => TextMessageRole::User,
            };
            text_messages = text_messages.add_message(role, &msg.content);
        }

        let draft: ProjectionPatchDraft = self
            .rt
            .block_on(
                self.model
                    .generate_structured::<ProjectionPatchDraft>(text_messages),
            )
            .map_err(|e| format!("mistral.rs structured projection failed: {}", e))?;
        let raw_json = serde_json::to_string(&draft)
            .map_err(|e| format!("failed to serialize structured projection draft: {}", e))?;

        Ok((raw_json, 0))
    }

    // ------------------------------------------------------------------
    // Streaming chat
    // ------------------------------------------------------------------

    /// Start a true token-streaming chat request.
    ///
    /// Mirrors [`crate::llm::engine::LlmEngine::stream_chat`]: the returned
    /// `Result` only reports whether the request was accepted; generated tokens
    /// and the single terminal frame (`Done` / `Cancelled` / `Error`) are
    /// delivered through `events`. The system-prompt + history assembly matches
    /// [`Self::chat_with_usage`] so the streamed reply tracks the blocking one.
    ///
    /// Because `Stream<'a>` borrows `&'a self.model`, the whole drain loop runs
    /// inside `self.rt.block_on(...)` (the same `block_on` bridge the blocking
    /// chat path uses). The loop is a `biased` `tokio::select!` between
    /// cancellation and the next stream frame: on cancel we emit `Cancelled`
    /// and drop the `Stream` immediately so the model borrow and the upstream
    /// `Receiver` are released promptly. Exactly one terminal frame is emitted.
    pub fn stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        graph_context: String,
        params: LlmChatParams,
        cancel: CancellationToken,
        events: tokio::sync::mpsc::Sender<LlmStreamEvent>,
    ) -> Result<(), String> {
        let system_prompt = format!(
            "You are a helpful assistant that answers questions about an \
             audio conversation and its knowledge graph. Use the following context from \
             the knowledge graph and recent transcript to answer questions.\n\n\
             Knowledge Graph Context:\n{}",
            graph_context
        );

        let mut request =
            RequestBuilder::new().add_message(TextMessageRole::System, &system_prompt);
        for msg in &messages {
            let role = match msg.role.as_str() {
                "user" => TextMessageRole::User,
                "assistant" => TextMessageRole::Assistant,
                "system" => TextMessageRole::System,
                _ => TextMessageRole::User,
            };
            request = request.add_message(role, &msg.content);
        }
        let request = request
            .set_sampler_temperature(params.temperature as f64)
            .set_sampler_max_len(params.max_tokens as usize);

        self.rt.block_on(async move {
            // `Stream<'a>` borrows `&'a self.model`, so it must be created and
            // fully drained inside this async block (it cannot escape `block_on`).
            let mut stream = match self.model.stream_chat_request(request).await {
                Ok(stream) => stream,
                Err(e) => {
                    let _ = events
                        .send(LlmStreamEvent::Error {
                            message: format!("mistral.rs stream request failed: {}", e),
                            full_text: String::new(),
                        })
                        .await;
                    return;
                }
            };

            // Accumulate streamed content so the terminal `Done` carries the full
            // reply and a mid-stream cancellation carries the partial text.
            //
            // mistral.rs 0.8 streaming emits ONLY `Response::Chunk` frames (never
            // `Response::Done`): each chunk carries a content delta, and the final
            // chunk additionally carries a non-null `choices[0].finish_reason` and
            // the request `usage`. So the chunk arm both forwards the delta AND
            // detects the terminal chunk to synthesize the `Done`. The
            // non-chunk arms (`Done` / `ModelError` / errors) are mapped
            // defensively by `mistralrs_response_to_event` in case a future /
            // non-streaming code path ever yields them.
            let mut full_text = String::new();

            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => {
                        let _ = events
                            .send(LlmStreamEvent::Cancelled { full_text })
                            .await;
                        // Drop the Stream now to release the &model borrow and the
                        // upstream Receiver before returning.
                        drop(stream);
                        return;
                    }
                    frame = stream.next() => {
                        match frame {
                            Some(Response::Chunk(chunk)) => {
                                let StreamChunkStep {
                                    content,
                                    terminal_usage,
                                } = interpret_stream_chunk(chunk);
                                if !content.is_empty() {
                                    full_text.push_str(&content);
                                    if events
                                        .send(LlmStreamEvent::Delta { content })
                                        .await
                                        .is_err()
                                    {
                                        // Consumer hung up; release the borrow and stop.
                                        return;
                                    }
                                }
                                // The final chunk carries a finish_reason (and the
                                // request usage). Emit the single terminal `Done`.
                                if let Some((prompt_tokens, completion_tokens, total_tokens)) =
                                    terminal_usage
                                {
                                    let _ = events
                                        .send(LlmStreamEvent::Done {
                                            full_text,
                                            prompt_tokens,
                                            completion_tokens,
                                            total_tokens,
                                        })
                                        .await;
                                    return;
                                }
                            }
                            Some(resp) => {
                                // Non-chunk frame (Done / ModelError / Internal /
                                // Validation / unexpected). Map defensively; all of
                                // these are terminal.
                                let _ = events.send(mistralrs_response_to_event(resp)).await;
                                return;
                            }
                            None => {
                                // Stream ended without a terminal (finish_reason)
                                // chunk: surface a single defensive Error carrying
                                // whatever text was generated so far.
                                let _ = events
                                    .send(LlmStreamEvent::Error {
                                        message: "mistral.rs stream ended without a terminal chunk"
                                            .to_string(),
                                        full_text,
                                    })
                                    .await;
                                return;
                            }
                        }
                    }
                }
            }
        });

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Stub when mistral.rs is not compiled in (cloud-only build). Same API,
// reports "not in this build" so call sites + state plumbing are unchanged.
// ---------------------------------------------------------------------------

#[cfg(not(feature = "llm-mistralrs"))]
const MISTRALRS_UNAVAILABLE: &str = "Local mistral.rs LLM is not included in this build (cloud-only). Use a cloud \
     LLM provider, or rebuild with the `local-ml` / `llm-mistralrs` feature.";

#[cfg(not(feature = "llm-mistralrs"))]
pub struct MistralRsEngine;

#[cfg(not(feature = "llm-mistralrs"))]
impl MistralRsEngine {
    pub fn new(_model_dir: &str, _model_filename: &str) -> Result<Self, String> {
        Err(MISTRALRS_UNAVAILABLE.to_string())
    }
    pub fn is_loaded(&self) -> bool {
        false
    }
    pub fn extract_entities(
        &self,
        _text: &str,
        _speaker: &str,
    ) -> Result<ExtractionResult, String> {
        Err(MISTRALRS_UNAVAILABLE.to_string())
    }
    pub fn chat(&self, _messages: &[ChatMessage], _graph_context: &str) -> Result<String, String> {
        Err(MISTRALRS_UNAVAILABLE.to_string())
    }
    pub fn chat_with_usage(
        &self,
        _messages: &[ChatMessage],
        _graph_context: &str,
    ) -> Result<(String, u32), String> {
        Err(MISTRALRS_UNAVAILABLE.to_string())
    }
    pub fn chat_with_history(
        &self,
        _messages: &[ChatMessage],
        _graph_context: &str,
    ) -> Result<String, String> {
        Err(MISTRALRS_UNAVAILABLE.to_string())
    }
    pub fn chat_with_history_usage(
        &self,
        _messages: &[ChatMessage],
        _graph_context: &str,
    ) -> Result<(String, u32), String> {
        Err(MISTRALRS_UNAVAILABLE.to_string())
    }
    pub fn projection_patch_draft_with_usage(
        &self,
        _messages: &[ChatMessage],
    ) -> Result<(String, u32), String> {
        Err(MISTRALRS_UNAVAILABLE.to_string())
    }
    pub fn stream_chat(
        &self,
        _messages: Vec<ChatMessage>,
        _graph_context: String,
        _params: crate::llm::engine::LlmChatParams,
        _cancel: tokio_util::sync::CancellationToken,
        _events: tokio::sync::mpsc::Sender<crate::llm::engine::LlmStreamEvent>,
    ) -> Result<(), String> {
        Err(MISTRALRS_UNAVAILABLE.to_string())
    }
}

// ---------------------------------------------------------------------------
// Tests
//
// The chat path itself (`send_chat_request` / `stream_chat_request`) is
// model-backed and cannot run without a loaded GGUF, so the unit tests target
// the model-free response seams: the `usize -> u32` usage mapping
// (`usage_total_to_u32`, ungated so it covers cloud-only builds too) and the
// streaming `Response -> LlmStreamEvent` mapping (`mistralrs_response_to_event`,
// gated on `llm-mistralrs` because it references mistral.rs `Response` values,
// but still driven entirely by synthetic in-memory frames — no native lib).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::usage_total_to_u32;

    #[test]
    fn usage_total_to_u32_passes_through_typical_counts() {
        // A normal chat total (prompt + completion) maps unchanged.
        assert_eq!(usage_total_to_u32(0), 0);
        assert_eq!(usage_total_to_u32(1), 1);
        assert_eq!(usage_total_to_u32(2048), 2048);
        assert_eq!(usage_total_to_u32(u32::MAX as usize), u32::MAX);
    }

    #[test]
    fn usage_total_to_u32_saturates_instead_of_wrapping() {
        // A pathological count above u32::MAX must clamp to u32::MAX, never
        // wrap to a tiny value (which would silently under-report usage).
        let over = u32::MAX as usize + 1;
        assert_eq!(usage_total_to_u32(over), u32::MAX);
        assert_eq!(usage_total_to_u32(usize::MAX), u32::MAX);
    }

    // ------------------------------------------------------------------
    // Streaming response-mapping seam (model-free)
    //
    // `mistralrs_response_to_event` references mistral.rs's `Response` type, so
    // these tests only compile under `llm-mistralrs`. They construct synthetic
    // `Chunk` / `Done` / `ModelError` (and the defensive `InternalError` /
    // unexpected-variant) frames in memory and assert the mapping with **no
    // loaded GGUF and no native inference** — the same intent as the
    // `usage_total_to_u32` tests above, one layer up.
    // ------------------------------------------------------------------
    #[cfg(feature = "llm-mistralrs")]
    mod stream_mapping {
        use super::super::{StreamChunkStep, interpret_stream_chunk, mistralrs_response_to_event};
        use crate::llm::engine::LlmStreamEvent;
        use mistralrs::{
            ChatCompletionChunkResponse, ChatCompletionResponse, Choice, ChunkChoice, Delta,
            Response, ResponseMessage, Usage,
        };

        fn usage(prompt_tokens: usize, completion_tokens: usize, total_tokens: usize) -> Usage {
            Usage {
                completion_tokens,
                prompt_tokens,
                total_tokens,
                avg_tok_per_sec: 0.0,
                avg_prompt_tok_per_sec: 0.0,
                avg_compl_tok_per_sec: 0.0,
                total_time_sec: 0.0,
                total_prompt_time_sec: 0.0,
                total_completion_time_sec: 0.0,
            }
        }

        /// Build a raw streaming chunk with optional content, finish_reason, and
        /// usage (mirroring how mistral.rs populates intermediate vs final chunks).
        fn chunk_response(
            content: Option<&str>,
            finish_reason: Option<&str>,
            usage: Option<Usage>,
        ) -> ChatCompletionChunkResponse {
            ChatCompletionChunkResponse {
                id: "chunk-id".to_string(),
                choices: vec![ChunkChoice {
                    finish_reason: finish_reason.map(str::to_string),
                    index: 0,
                    delta: Delta {
                        content: content.map(str::to_string),
                        role: "assistant".to_string(),
                        tool_calls: None,
                        reasoning_content: None,
                    },
                    logprobs: None,
                }],
                created: 0,
                model: "test".to_string(),
                system_fingerprint: "local".to_string(),
                object: "chat.completion.chunk".to_string(),
                usage,
            }
        }

        fn chunk(content: Option<&str>) -> Response {
            Response::Chunk(chunk_response(content, None, None))
        }

        fn completion(content: Option<&str>, usage: Usage) -> ChatCompletionResponse {
            ChatCompletionResponse {
                id: "done-id".to_string(),
                choices: vec![Choice {
                    finish_reason: "stop".to_string(),
                    index: 0,
                    message: ResponseMessage {
                        content: content.map(str::to_string),
                        role: "assistant".to_string(),
                        tool_calls: None,
                        reasoning_content: None,
                    },
                    logprobs: None,
                }],
                created: 0,
                model: "test".to_string(),
                system_fingerprint: "local".to_string(),
                object: "chat.completion".to_string(),
                usage,
            }
        }

        #[test]
        fn chunk_maps_to_delta_with_choice_content() {
            assert_eq!(
                mistralrs_response_to_event(chunk(Some("tok"))),
                LlmStreamEvent::Delta {
                    content: "tok".to_string()
                }
            );
        }

        #[test]
        fn chunk_with_absent_content_maps_to_empty_delta() {
            assert_eq!(
                mistralrs_response_to_event(chunk(None)),
                LlmStreamEvent::Delta {
                    content: String::new()
                }
            );
        }

        #[test]
        fn done_maps_to_done_with_split_usage() {
            let resp = Response::Done(completion(Some("full answer"), usage(11, 4, 15)));
            assert_eq!(
                mistralrs_response_to_event(resp),
                LlmStreamEvent::Done {
                    full_text: "full answer".to_string(),
                    prompt_tokens: 11,
                    completion_tokens: 4,
                    total_tokens: 15,
                }
            );
        }

        #[test]
        fn model_error_carries_partial_text() {
            let resp = Response::ModelError(
                "boom".to_string(),
                completion(Some("partial"), usage(2, 1, 3)),
            );
            match mistralrs_response_to_event(resp) {
                LlmStreamEvent::Error { message, full_text } => {
                    assert!(
                        message.contains("boom"),
                        "message should carry cause: {message}"
                    );
                    assert_eq!(full_text, "partial");
                }
                other => panic!("expected Error, got {other:?}"),
            }
        }

        #[test]
        fn internal_error_maps_to_error_without_partial_text() {
            let resp = Response::InternalError("io".to_string().into());
            match mistralrs_response_to_event(resp) {
                LlmStreamEvent::Error { message, full_text } => {
                    assert!(
                        message.contains("io"),
                        "message should carry cause: {message}"
                    );
                    assert!(full_text.is_empty());
                }
                other => panic!("expected Error, got {other:?}"),
            }
        }

        #[test]
        fn unexpected_non_chat_variant_maps_to_defensive_error() {
            // A chat stream never yields embeddings, but the mapper must still
            // emit exactly one terminal rather than dropping the frame.
            let resp = Response::Embeddings {
                embeddings: vec![0.0],
                prompt_tokens: 1,
                total_tokens: 1,
            };
            match mistralrs_response_to_event(resp) {
                LlmStreamEvent::Error { full_text, .. } => assert!(full_text.is_empty()),
                other => panic!("expected defensive Error, got {other:?}"),
            }
        }

        // --- interpret_stream_chunk: the real 0.8 streaming terminal path ---

        #[test]
        fn intermediate_chunk_is_content_only_no_terminal() {
            // A chunk with content but no finish_reason / usage is non-terminal:
            // forward the delta, do NOT synthesize Done yet.
            let step = interpret_stream_chunk(chunk_response(Some("Hel"), None, None));
            assert_eq!(
                step,
                StreamChunkStep {
                    content: "Hel".to_string(),
                    terminal_usage: None,
                }
            );
        }

        #[test]
        fn final_chunk_with_finish_reason_and_usage_is_terminal() {
            // The final chunk sets finish_reason AND carries the request usage:
            // its trailing content is forwarded and the terminal usage is split.
            let step = interpret_stream_chunk(chunk_response(
                Some("lo"),
                Some("stop"),
                Some(usage(8, 5, 13)),
            ));
            assert_eq!(
                step,
                StreamChunkStep {
                    content: "lo".to_string(),
                    terminal_usage: Some((8, 5, 13)),
                }
            );
        }

        #[test]
        fn final_chunk_finish_reason_without_usage_still_terminates_with_zero_usage() {
            // Defensive: a terminal finish_reason but no usage block still
            // terminates (usage zeroed) so the drain loop never hangs.
            let step = interpret_stream_chunk(chunk_response(None, Some("stop"), None));
            assert_eq!(
                step,
                StreamChunkStep {
                    content: String::new(),
                    terminal_usage: Some((0, 0, 0)),
                }
            );
        }

        #[test]
        fn chunk_carrying_usage_without_finish_reason_is_treated_as_terminal() {
            // Some builds attach usage to the last chunk without a finish_reason;
            // treat usage presence alone as terminal.
            let step = interpret_stream_chunk(chunk_response(None, None, Some(usage(3, 2, 5))));
            assert_eq!(step.terminal_usage, Some((3, 2, 5)));
        }
    }
}
