//! LLM inference engine backed by llama-cpp-2.
//!
//! Wraps [`LlamaBackend`] and [`LlamaModel`] for in-process GGUF model
//! inference.  Supports grammar-constrained entity extraction (via GBNF) and
//! free-form chat generation.
//!
//! ## Persistent-context actor (ADR-0012, Phase 0a)
//!
//! `LlamaContext` is **not** `Send`, so it cannot live in the
//! `Arc<Mutex<Option<LlmEngine>>>` shared application state. Previously each
//! inference call created — and threw away — a fresh `LlamaContext`, paying the
//! KV-cache allocation cost every time. Phase 0a moves the model **and** a
//! single long-lived `LlamaContext` onto a dedicated thread (an actor); calls
//! arrive over a channel and replies go back over a one-shot channel, so the
//! public `&self` API (`extract_entities`, `chat`) is unchanged and still
//! synchronous from the caller's perspective.
//!
//! Each call resets the persistent context's KV cache (`clear_kv_cache`) before
//! running the tokenize → decode → sample sequence, so calls are independent
//! and deterministic (extraction is seeded with 42). The wins are eliminating
//! per-call context creation and establishing the warm-context foundation that
//! the instruction-prefix-reuse optimization (Phase 0b) and the real-time S2S
//! agent will build on. This work also fixed a latent bug in the generation
//! loop (tokens were appended at a stale KV position, which `llama_decode`
//! rejects) — previously unhit because the local extraction model's download
//! URL 404'd, so this inference path had never actually executed end-to-end.
//! Because one context now serves every request, calls serialize through the
//! actor (extraction is `Background` priority, so this is acceptable for the
//! notes pipeline).

#[cfg(feature = "llm-llama")]
use std::num::NonZeroU32;
#[cfg(feature = "llm-llama")]
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};

#[cfg(feature = "llm-llama")]
use llama_cpp_2::context::LlamaContext;
#[cfg(feature = "llm-llama")]
use llama_cpp_2::context::params::LlamaContextParams;
#[cfg(feature = "llm-llama")]
use llama_cpp_2::llama_backend::LlamaBackend;
#[cfg(feature = "llm-llama")]
use llama_cpp_2::llama_batch::LlamaBatch;
#[cfg(feature = "llm-llama")]
use llama_cpp_2::model::params::LlamaModelParams;
#[cfg(feature = "llm-llama")]
use llama_cpp_2::model::{AddBos, LlamaModel};
#[cfg(feature = "llm-llama")]
use llama_cpp_2::sampling::LlamaSampler;
use tokio_util::sync::CancellationToken;

use crate::graph::entities::ExtractionResult;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A chat message with role and content.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatMessage {
    pub role: String, // "user", "assistant", "system"
    pub content: String,
}

/// Response from the chat endpoint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChatResponse {
    pub message: ChatMessage,
    pub tokens_used: u32,
}

/// A completed chat generation plus the token usage the backend reported.
///
/// Internal carrier so the blocking chat path (`LlmExecutor::chat_with_history`
/// → `commands::send_chat_message`) can surface a real `tokens_used` count
/// instead of a hard-coded 0. `tokens_used` is the total (prompt + completion)
/// when the backend exposes it; backends that genuinely do not report usage
/// set it to 0 (never fabricated).
#[derive(Debug, Clone)]
pub struct ChatOutcome {
    pub text: String,
    pub tokens_used: u32,
}

/// Sampling settings used by the local llama.cpp chat loop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LlmChatParams {
    pub max_tokens: u32,
    pub temperature: f32,
}

impl Default for LlmChatParams {
    fn default() -> Self {
        Self {
            max_tokens: 512,
            temperature: 0.7,
        }
    }
}

/// Engine-owned stream events emitted by the local llama.cpp actor.
///
/// This is deliberately separate from `streaming::TokenDelta`: the actor owns
/// llama.cpp generation and usage accounting, while `streaming.rs` owns the
/// provider-neutral IPC-facing bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LlmStreamEvent {
    Delta {
        content: String,
    },
    Done {
        full_text: String,
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
    },
    Cancelled {
        full_text: String,
    },
    Error {
        message: String,
        full_text: String,
    },
}

// ---------------------------------------------------------------------------
// LlmEngine — persistent-context actor (ADR-0012 Phase 0a)
// ---------------------------------------------------------------------------

/// Request sent to the engine actor thread. Each variant carries a one-shot
/// reply channel so the calling thread can block for the result, preserving the
/// previous synchronous `&self` API.
#[cfg(feature = "llm-llama")]
enum EngineReq {
    Extract {
        text: String,
        speaker: String,
        reply: SyncSender<Result<ExtractionResult, String>>,
    },
    Chat {
        messages: Vec<ChatMessage>,
        graph_context: String,
        reply: SyncSender<Result<ChatOutcome, String>>,
    },
    StreamChat {
        messages: Vec<ChatMessage>,
        graph_context: String,
        params: LlmChatParams,
        cancel: CancellationToken,
        events: tokio::sync::mpsc::Sender<LlmStreamEvent>,
    },
}

/// Native LLM engine using llama.cpp via llama-cpp-2 bindings.
///
/// A handle to a dedicated actor thread that owns the `LlamaModel` and a
/// long-lived `LlamaContext` (neither of which needs to be `Send` to cross the
/// channel — only the request/reply messages do). The handle itself is
/// `Send + Sync`, so it still lives inside `Arc<Mutex<Option<LlmEngine>>>` in
/// application state exactly as before.
#[cfg(feature = "llm-llama")]
pub struct LlmEngine {
    tx: Sender<EngineReq>,
}

#[cfg(feature = "llm-llama")]
impl Clone for LlmEngine {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

#[cfg(feature = "llm-llama")]
impl LlmEngine {
    /// Load a GGUF model from disk and start the engine actor thread.
    ///
    /// Blocks until the thread has loaded the model + created the persistent
    /// context, so load failures surface synchronously (as before).
    pub fn new(model_path: &str) -> Result<Self, String> {
        let (tx, rx) = channel::<EngineReq>();
        let (ready_tx, ready_rx) = sync_channel::<Result<(), String>>(1);
        let path = model_path.to_string();

        std::thread::Builder::new()
            .name("llm-engine".to_string())
            .spawn(move || engine_loop(&path, rx, ready_tx))
            .map_err(|e| format!("Failed to spawn LLM engine thread: {}", e))?;

        // Wait for the actor to report model-load + context-creation status.
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Self { tx }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("LLM engine thread exited before signaling readiness".to_string()),
        }
        // On error the `tx` we hold is dropped here, closing the channel so the
        // (already-exited) thread leaves nothing dangling.
    }

    /// Check if model is loaded and ready.
    pub fn is_loaded(&self) -> bool {
        true // If we constructed successfully, the actor loaded the model.
    }

    /// Extract entities and relations from text using grammar-constrained
    /// generation. The output is forced to match the JSON schema expected by
    /// [`ExtractionResult`].
    pub fn extract_entities(&self, text: &str, speaker: &str) -> Result<ExtractionResult, String> {
        let (reply, reply_rx) = sync_channel(1);
        self.tx
            .send(EngineReq::Extract {
                text: text.to_string(),
                speaker: speaker.to_string(),
                reply,
            })
            .map_err(|_| "LLM engine thread is not running".to_string())?;
        reply_rx
            .recv()
            .map_err(|_| "LLM engine thread dropped the extraction request".to_string())?
    }

    /// Chat with the LLM, providing graph context in the system prompt.
    ///
    /// Returns the generated text plus the real token usage (prompt +
    /// completion) the local inference loop counted, so the blocking chat path
    /// can report accurate telemetry.
    pub fn chat(
        &self,
        messages: &[ChatMessage],
        graph_context: &str,
    ) -> Result<ChatOutcome, String> {
        let (reply, reply_rx) = sync_channel(1);
        self.tx
            .send(EngineReq::Chat {
                messages: messages.to_vec(),
                graph_context: graph_context.to_string(),
                reply,
            })
            .map_err(|_| "LLM engine thread is not running".to_string())?;
        reply_rx
            .recv()
            .map_err(|_| "LLM engine thread dropped the chat request".to_string())?
    }

    /// Start a true token-streaming chat request on the actor thread.
    ///
    /// The returned result only indicates whether the actor accepted the request.
    /// Generated tokens and terminal state are delivered through `events`. The
    /// actor checks `cancel` between generated tokens; the safe llama-cpp-2 API
    /// cannot interrupt an in-progress `ctx.decode` call.
    pub fn stream_chat(
        &self,
        messages: Vec<ChatMessage>,
        graph_context: String,
        params: LlmChatParams,
        cancel: CancellationToken,
        events: tokio::sync::mpsc::Sender<LlmStreamEvent>,
    ) -> Result<(), String> {
        self.tx
            .send(EngineReq::StreamChat {
                messages,
                graph_context,
                params,
                cancel,
                events,
            })
            .map_err(|_| "LLM engine thread is not running".to_string())
    }

    #[cfg(test)]
    pub(crate) fn test_with_stream_pieces(
        pieces: Vec<String>,
        delay_between_pieces: std::time::Duration,
        prompt_tokens: u32,
    ) -> Self {
        let (tx, rx) = channel::<EngineReq>();
        std::thread::Builder::new()
            .name("llm-engine-stream-test".to_string())
            .spawn(move || {
                for req in rx {
                    match req {
                        EngineReq::Extract { reply, .. } => {
                            let _ = reply.send(Err(
                                "test LLM engine does not implement extraction".to_string(),
                            ));
                        }
                        EngineReq::Chat { reply, .. } => {
                            let text = pieces.concat();
                            let tokens_used = prompt_tokens.saturating_add(pieces.len() as u32);
                            let _ = reply.send(Ok(ChatOutcome { text, tokens_used }));
                        }
                        EngineReq::StreamChat { events, cancel, .. } => {
                            let mut full_text = String::new();
                            for piece in &pieces {
                                if delay_between_pieces > std::time::Duration::ZERO {
                                    std::thread::sleep(delay_between_pieces);
                                }
                                if cancel.is_cancelled() {
                                    let _ = events.blocking_send(LlmStreamEvent::Cancelled {
                                        full_text: full_text.clone(),
                                    });
                                    break;
                                }
                                full_text.push_str(piece);
                                if events
                                    .blocking_send(LlmStreamEvent::Delta {
                                        content: piece.clone(),
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                                if cancel.is_cancelled() {
                                    let _ = events.blocking_send(LlmStreamEvent::Cancelled {
                                        full_text: full_text.clone(),
                                    });
                                    break;
                                }
                            }
                            if !cancel.is_cancelled() {
                                let completion_tokens = pieces.len() as u32;
                                let total_tokens = prompt_tokens.saturating_add(completion_tokens);
                                let _ = events.blocking_send(LlmStreamEvent::Done {
                                    full_text,
                                    prompt_tokens,
                                    completion_tokens,
                                    total_tokens,
                                });
                            }
                        }
                    }
                }
            })
            .expect("spawn stream test LLM engine thread");
        Self { tx }
    }

    #[cfg(test)]
    pub(crate) fn test_with_stream_error(message: impl Into<String>) -> Self {
        let message = message.into();
        let (tx, rx) = channel::<EngineReq>();
        std::thread::Builder::new()
            .name("llm-engine-stream-error-test".to_string())
            .spawn(move || {
                for req in rx {
                    match req {
                        EngineReq::Extract { reply, .. } => {
                            let _ = reply.send(Err(
                                "test LLM engine does not implement extraction".to_string(),
                            ));
                        }
                        EngineReq::Chat { reply, .. } => {
                            let _ = reply.send(Err(message.clone()));
                        }
                        EngineReq::StreamChat { events, .. } => {
                            let _ = events.blocking_send(LlmStreamEvent::Error {
                                message: message.clone(),
                                full_text: String::new(),
                            });
                        }
                    }
                }
            })
            .expect("spawn stream error test LLM engine thread");
        Self { tx }
    }
}

// Dropping `LlmEngine` drops `tx`, which closes the request channel; the actor
// loop's `for req in rx` then ends and the thread unwinds, freeing the model
// and context. The thread is detached (we don't join) so drop never blocks.

/// Process-global llama backend.
///
/// `LlamaBackend::init()` is a one-shot process-global initialization — calling
/// it a second time returns `BackendAlreadyInitialized`. Since multiple engine
/// instances (e.g. across a model reload) each need a backend reference, we
/// initialize once and hand out a `'static` borrow. `LlamaBackend` is a ZST, so
/// sharing it across threads is sound. The init result is cached: a failure is
/// permanent for the process.
#[cfg(feature = "llm-llama")]
fn global_backend() -> Result<&'static LlamaBackend, String> {
    use std::sync::OnceLock;
    static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
    BACKEND
        .get_or_init(|| {
            LlamaBackend::init().map_err(|e| format!("Failed to init llama backend: {}", e))
        })
        .as_ref()
        .map_err(|e| e.clone())
}

/// Actor thread entry point: owns the model and one persistent `LlamaContext`
/// (borrowing the process-global backend), then services requests until the
/// channel closes.
///
/// `model` and `ctx` are declared in dependency order so that `ctx` (which
/// borrows the model + the `'static` backend) drops first — satisfying the
/// borrow checker for the `!Send` context held across the loop.
#[cfg(feature = "llm-llama")]
fn engine_loop(model_path: &str, rx: Receiver<EngineReq>, ready: SyncSender<Result<(), String>>) {
    let backend = match global_backend() {
        Ok(b) => b,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };

    let model = match LlamaModel::load_from_file(backend, model_path, &LlamaModelParams::default())
    {
        Ok(m) => m,
        Err(e) => {
            let _ = ready.send(Err(format!("Failed to load model '{}': {}", model_path, e)));
            return;
        }
    };

    let ctx_params = LlamaContextParams::default().with_n_ctx(Some(NonZeroU32::new(2048).unwrap()));
    let mut ctx = match model.new_context(backend, ctx_params) {
        Ok(c) => c,
        Err(e) => {
            let _ = ready.send(Err(format!("Failed to create context: {}", e)));
            return;
        }
    };

    log::info!(
        "LLM model loaded from: {} (persistent-context actor)",
        model_path
    );
    if ready.send(Ok(())).is_err() {
        return; // Caller gave up; nothing to serve.
    }

    for req in rx {
        match req {
            EngineReq::Extract {
                text,
                speaker,
                reply,
            } => {
                let _ = reply.send(do_extract(&model, &mut ctx, &text, &speaker));
            }
            EngineReq::Chat {
                messages,
                graph_context,
                reply,
            } => {
                let _ = reply.send(do_chat(&model, &mut ctx, &messages, &graph_context));
            }
            EngineReq::StreamChat {
                messages,
                graph_context,
                params,
                cancel,
                events,
            } => {
                do_stream_chat(
                    &model,
                    &mut ctx,
                    &messages,
                    &graph_context,
                    params,
                    cancel,
                    events,
                );
            }
        }
    }
}

/// Entity extraction on the persistent context (generate-then-validate).
///
/// Prompts the model with its native **ChatML** template and a system prompt
/// that pins the exact JSON schema, then decodes **greedily** (the LFM2-Extract
/// model card recommends temperature=0) and parses the result. We do *not* use
/// a GBNF grammar sampler: it aborts inside llama.cpp on this model/version
/// (`llama-grammar.cpp: GGML_ASSERT(!stacks.empty())`, an uncatchable C++
/// `abort()`), and `llama-cpp-2` 0.1.146 is already the latest release.
/// LFM2-350M-**Extract** is fine-tuned to emit schema-conformant JSON when
/// prompted in its ChatML format with a schema system prompt; if parsing still
/// fails, the executor falls back to other providers / rule-based extraction.
///
/// `str_to_token` tokenizes with `parse_special=true`, so the `<|im_start|>` /
/// `<|im_end|>` control tokens are recognized; `AddBos::Always` supplies the
/// leading `<|startoftext|>` BOS, and `<|im_end|>` is the EOG that stops decode.
#[cfg(feature = "llm-llama")]
fn do_extract(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    text: &str,
    speaker: &str,
) -> Result<ExtractionResult, String> {
    // ADR-0008 follow-up #1: use the shared conversation ontology as the single
    // source of truth for the extraction vocabulary/schema (kills the prompt
    // drift that came from a hard-coded type list here). The LFM2-specific
    // ChatML wrapper is preserved — `extraction_system_prompt()` only supplies
    // the system *content*; the `<|im_start|>` framing remains the model's
    // template. Schema is identical to the previous inline prompt
    // (entities[name,entity_type,description] + relations[source,target,
    // relation_type,detail]), so the downstream `ExtractionResult` parse is
    // unchanged.
    let system_prompt = crate::ontology::extraction_system_prompt();

    // LFM2 ChatML template. BOS (<|startoftext|>) is added by AddBos::Always.
    let prompt = format!(
        "<|im_start|>system\n{system}<|im_end|>\n\
         <|im_start|>user\n{speaker}: {text}<|im_end|>\n\
         <|im_start|>assistant\n",
        system = system_prompt,
        speaker = speaker,
        text = text,
    );

    // Greedy decoding for deterministic, schema-faithful extraction (temp=0,
    // per the model card), with a repetition penalty in front: pure greedy on
    // this 350M model degenerates into repeating the same relation object until
    // the token cap truncates the JSON. The penalty breaks that loop while
    // staying fully deterministic (no RNG), so extraction remains reproducible.
    let sampler = LlamaSampler::chain_simple([
        LlamaSampler::penalties(256, 1.3, 0.0, 0.0),
        LlamaSampler::greedy(),
    ]);

    // Extraction discards the token count: it flows back to the caller as an
    // `ExtractionResult`, not a `ChatResponse`, so usage telemetry is N/A here.
    let output = run_inference(model, ctx, &prompt, 512, sampler)?
        .text
        .trim()
        .to_string();

    // The fine-tuned model emits JSON directly, but be defensive: slice out the
    // outermost { .. } object in case the model wraps it in prose.
    let json = extract_json_object(&output).unwrap_or(output.as_str());

    serde_json::from_str(json)
        .map_err(|e| format!("Failed to parse extraction JSON: {} — raw: {}", e, output))
}

/// Return the substring spanning the first `{` to the last `}` (inclusive), or
/// `None` if no balanced-looking object is present.
#[cfg(feature = "llm-llama")]
fn extract_json_object(s: &str) -> Option<&str> {
    let start = s.find('{')?;
    let end = s.rfind('}')?;
    if end > start {
        Some(&s[start..=end])
    } else {
        None
    }
}

/// Free-form chat generation on the persistent context.
#[cfg(feature = "llm-llama")]
fn do_chat(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<ChatOutcome, String> {
    let generation = build_chat_generation(messages, graph_context, LlmChatParams::default());
    let outcome = run_inference(
        model,
        ctx,
        &generation.prompt,
        generation.params.max_tokens,
        generation.sampler,
    )?;
    Ok(ChatOutcome {
        text: outcome.text.trim().to_string(),
        tokens_used: outcome.total_tokens,
    })
}

#[cfg(feature = "llm-llama")]
fn do_stream_chat(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    messages: &[ChatMessage],
    graph_context: &str,
    params: LlmChatParams,
    cancel: CancellationToken,
    events: tokio::sync::mpsc::Sender<LlmStreamEvent>,
) {
    let generation = build_chat_generation(messages, graph_context, params);
    let result = run_streaming_inference(
        model,
        ctx,
        &generation.prompt,
        generation.params.max_tokens,
        generation.sampler,
        &cancel,
        &events,
    );

    match result {
        Ok(InferenceRun::Done(outcome)) => {
            let _ = events.blocking_send(LlmStreamEvent::Done {
                full_text: outcome.text,
                prompt_tokens: outcome.prompt_tokens,
                completion_tokens: outcome.completion_tokens,
                total_tokens: outcome.total_tokens,
            });
        }
        Ok(InferenceRun::Cancelled { full_text }) => {
            let _ = events.blocking_send(LlmStreamEvent::Cancelled { full_text });
        }
        Err(message) => {
            let _ = events.blocking_send(LlmStreamEvent::Error {
                message,
                full_text: String::new(),
            });
        }
    }
}

#[cfg(feature = "llm-llama")]
struct ChatGeneration {
    prompt: String,
    params: LlmChatParams,
    sampler: LlamaSampler,
}

#[cfg(feature = "llm-llama")]
fn build_chat_generation(
    messages: &[ChatMessage],
    graph_context: &str,
    params: LlmChatParams,
) -> ChatGeneration {
    let mut prompt = String::new();
    prompt.push_str(
        "<|system|>\nYou are a helpful assistant that answers questions about an \
         audio conversation and its knowledge graph. Use the following context from \
         the knowledge graph and recent transcript to answer questions.\n\n",
    );
    prompt.push_str("Knowledge Graph Context:\n");
    prompt.push_str(graph_context);
    prompt.push_str("\n</s>\n");

    for msg in messages {
        match msg.role.as_str() {
            "user" => {
                prompt.push_str("<|user|>\n");
                prompt.push_str(&msg.content);
                prompt.push_str("\n</s>\n");
            }
            "assistant" => {
                prompt.push_str("<|assistant|>\n");
                prompt.push_str(&msg.content);
                prompt.push_str("\n</s>\n");
            }
            _ => {}
        }
    }
    prompt.push_str("<|assistant|>\n");

    // Non-deterministic seed for chat (unlike seed-42 extraction).
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    let sampler = LlamaSampler::chain_simple([
        LlamaSampler::top_k(40),
        LlamaSampler::top_p(0.95, 1),
        LlamaSampler::temp(params.temperature),
        LlamaSampler::dist(seed),
    ]);

    ChatGeneration {
        prompt,
        params,
        sampler,
    }
}

#[cfg(feature = "llm-llama")]
struct InferenceOutcome {
    text: String,
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
}

#[cfg(feature = "llm-llama")]
enum InferenceRun {
    Done(InferenceOutcome),
    Cancelled { full_text: String },
}

/// Core inference loop shared by grammar-constrained and free-form generation.
///
/// Runs on the actor's **persistent** [`LlamaContext`]. The KV cache is fully
/// cleared first so each call is independent and deterministic — only the
/// per-call context allocation is eliminated (ADR-0012 Phase 0a).
///
/// Returns `(output_text, total_tokens)` where `total_tokens` is the prompt
/// token count plus the number of tokens actually generated (excluding the EOG
/// stop token, which is sampled but never appended). Callers that report chat
/// telemetry use this; extraction discards it.
#[cfg(feature = "llm-llama")]
fn run_inference(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    prompt: &str,
    max_tokens: u32,
    mut sampler: LlamaSampler,
) -> Result<InferenceOutcome, String> {
    match run_inference_inner(
        model,
        ctx,
        prompt,
        max_tokens,
        &mut sampler,
        None,
        None::<&mut dyn FnMut(&str) -> Result<(), String>>,
    )? {
        InferenceRun::Done(outcome) => Ok(outcome),
        InferenceRun::Cancelled { .. } => {
            Err("Inference was cancelled without a cancellation token".to_string())
        }
    }
}

#[cfg(feature = "llm-llama")]
fn run_streaming_inference(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    prompt: &str,
    max_tokens: u32,
    mut sampler: LlamaSampler,
    cancel: &CancellationToken,
    events: &tokio::sync::mpsc::Sender<LlmStreamEvent>,
) -> Result<InferenceRun, String> {
    let mut emit_piece = |piece: &str| {
        if piece.is_empty() {
            return Ok(());
        }
        events
            .blocking_send(LlmStreamEvent::Delta {
                content: piece.to_string(),
            })
            .map_err(|_| "LocalLlama stream receiver dropped".to_string())
    };

    run_inference_inner(
        model,
        ctx,
        prompt,
        max_tokens,
        &mut sampler,
        Some(cancel),
        Some(&mut emit_piece),
    )
}

#[cfg(feature = "llm-llama")]
#[allow(clippy::type_complexity)]
fn run_inference_inner(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    prompt: &str,
    max_tokens: u32,
    sampler: &mut LlamaSampler,
    cancel: Option<&CancellationToken>,
    mut on_piece: Option<&mut dyn FnMut(&str) -> Result<(), String>>,
) -> Result<InferenceRun, String> {
    // Reset the persistent context to an empty-KV state (== fresh context).
    ctx.clear_kv_cache();

    if cancel.is_some_and(|token| token.is_cancelled()) {
        return Ok(InferenceRun::Cancelled {
            full_text: String::new(),
        });
    }

    let tokens = model
        .str_to_token(prompt, AddBos::Always)
        .map_err(|e| format!("Tokenization failed: {}", e))?;

    let mut batch = LlamaBatch::new(2048, 1);
    for (i, token) in tokens.iter().enumerate() {
        let is_last = i == tokens.len() - 1;
        batch
            .add(*token, i as i32, &[0], is_last)
            .map_err(|e| format!("Failed to add token to batch: {}", e))?;
    }

    ctx.decode(&mut batch)
        .map_err(|e| format!("Failed to decode prompt: {}", e))?;

    if cancel.is_some_and(|token| token.is_cancelled()) {
        return Ok(InferenceRun::Cancelled {
            full_text: String::new(),
        });
    }

    let mut output = String::new();
    let mut decoder = encoding_rs::UTF_8.new_decoder();

    // The prompt occupies KV positions [0, prompt_len); each generated token
    // continues from there. Deriving the absolute position from the loop index
    // (rather than a stale/zero batch position) is required — appending at a
    // position that collides with the prompt's KV entries makes `llama_decode`
    // fail with ret=-1.
    let prompt_len = tokens.len() as i32;
    // Count tokens we actually generate (the EOG stop token is sampled but
    // never appended, so it is excluded). total = prompt + completion, matching
    // the OpenAI-style `total_tokens` the streaming path reports.
    let mut completion_tokens: u32 = 0;

    for i in 0..max_tokens as i32 {
        if cancel.is_some_and(|token| token.is_cancelled()) {
            return Ok(InferenceRun::Cancelled { full_text: output });
        }

        // Sample from the logits of the last decoded token in the current
        // batch (prompt batch on the first pass, single-token batch after).
        let new_token = sampler.sample(ctx, batch.n_tokens() - 1);
        sampler.accept(new_token);

        if model.is_eog_token(new_token) {
            break;
        }

        let piece = model
            .token_to_piece(new_token, &mut decoder, false, None)
            .map_err(|e| format!("Token decode failed: {}", e))?;
        output.push_str(&piece);
        completion_tokens += 1;

        if let Some(on_piece) = on_piece.as_deref_mut() {
            on_piece(&piece)?;
        }

        if cancel.is_some_and(|token| token.is_cancelled()) {
            return Ok(InferenceRun::Cancelled { full_text: output });
        }

        batch.clear();
        batch
            .add(new_token, prompt_len + i, &[0], true)
            .map_err(|e| format!("Failed to add token: {}", e))?;

        ctx.decode(&mut batch)
            .map_err(|e| format!("Decode failed: {}", e))?;
    }

    let total_tokens = (prompt_len as u32).saturating_add(completion_tokens);
    Ok(InferenceRun::Done(InferenceOutcome {
        text: output,
        prompt_tokens: prompt_len as u32,
        completion_tokens,
        total_tokens,
    }))
}

// ---------------------------------------------------------------------------
// Model-backed tests (ADR-0012 Phase 0a)
//
// These exercise the real llama.cpp inference path, so they need a GGUF model
// on disk. They are gated on the `AG_LLM_TEST_MODEL` env var (path to a GGUF)
// and no-op when it's unset — CI has no model, so it skips them. Run locally:
//   AG_LLM_TEST_MODEL=/path/LFM2-350M-Extract-Q4_K_M.gguf \
//     cargo test --lib model_backed_tests -- --nocapture --test-threads=1
// ---------------------------------------------------------------------------
#[cfg(all(test, feature = "llm-llama"))]
mod model_backed_tests {
    use super::*;

    fn test_model_path() -> Option<String> {
        std::env::var("AG_LLM_TEST_MODEL")
            .ok()
            .filter(|p| !p.is_empty())
    }

    /// Free-form (non-grammar) generation exercises the shared `run_inference`
    /// path — persistent-context reset (`clear_kv_cache`), prompt decode, and
    /// the corrected generation-loop position tracking. Two successful,
    /// non-empty replies prove the Phase 0a persistent-context mechanism is
    /// correct (no cross-call contamination, no KV-position collision).
    #[test]
    fn chat_free_form_generation_runs_on_persistent_context() {
        let Some(path) = test_model_path() else {
            eprintln!(
                "skipping chat_free_form_generation_runs_on_persistent_context: \
                 set AG_LLM_TEST_MODEL to a GGUF path to run"
            );
            return;
        };
        let engine = LlmEngine::new(&path).expect("engine should load the test model");
        let msgs = [ChatMessage {
            role: "user".to_string(),
            content: "Say hello in one short sentence.".to_string(),
        }];
        let r1 = engine.chat(&msgs, "").expect("first chat should succeed");
        let r2 = engine.chat(&msgs, "").expect("second chat should succeed");
        assert!(!r1.text.trim().is_empty(), "chat reply should be non-empty");
        assert!(
            !r2.text.trim().is_empty(),
            "second chat reply on the reused context should be non-empty"
        );
        // A non-empty reply means at least one prompt + one completion token,
        // so the usage count surfaced to the blocking path must be non-zero.
        assert!(
            r1.tokens_used > 0,
            "a non-empty local chat reply must report a non-zero token count"
        );
    }

    /// Entity extraction (generate-then-validate) must (a) run without aborting
    /// — proving the grammar-sampler crash is gone — and (b) be deterministic
    /// and isolated across calls on the reused context: identical inputs yield
    /// identical results (seed 42), and an interleaved different extraction must
    /// not contaminate a later identical one (proves `clear_kv_cache` fully
    /// resets the persistent context per call).
    #[test]
    fn extraction_is_deterministic_and_isolated() {
        let Some(path) = test_model_path() else {
            eprintln!("skipping extraction_is_deterministic_and_isolated: set AG_LLM_TEST_MODEL");
            return;
        };
        let engine = LlmEngine::new(&path).expect("engine should load");
        let input = ("Alice met Bob at Acme Corp in Paris.", "Speaker 1");

        let a = engine
            .extract_entities(input.0, input.1)
            .expect("extraction should succeed (no grammar abort)");
        let b = engine
            .extract_entities(input.0, input.1)
            .expect("repeat extraction should succeed");
        let _c = engine
            .extract_entities(
                "The quarterly budget was approved by the board.",
                "Speaker 2",
            )
            .expect("interleaved extraction should succeed");
        let d = engine
            .extract_entities(input.0, input.1)
            .expect("post-interleave extraction should succeed");

        assert_eq!(
            format!("{a:?}"),
            format!("{b:?}"),
            "repeated identical extraction must be deterministic (seed 42)"
        );
        assert_eq!(
            format!("{a:?}"),
            format!("{d:?}"),
            "identical extraction after an interleaved call must be unchanged \
             — proves the persistent context's KV cache is reset per call"
        );
    }
}

// ---------------------------------------------------------------------------
// Stub when local llama is not compiled in (cloud-only build).
// Keeps the type + API so the Arc<Mutex<Option<LlmEngine>>> plumbing and all
// call sites compile unchanged; every operation reports "not in this build".
// ---------------------------------------------------------------------------

#[cfg(not(feature = "llm-llama"))]
const LLAMA_UNAVAILABLE: &str = "Local llama.cpp LLM is not included in this build (cloud-only). Use a cloud \
     LLM provider, or rebuild with the `local-ml` / `llm-llama` feature.";

#[cfg(not(feature = "llm-llama"))]
#[derive(Clone)]
pub struct LlmEngine;

#[cfg(not(feature = "llm-llama"))]
impl LlmEngine {
    pub fn new(_model_path: &str) -> Result<Self, String> {
        Err(LLAMA_UNAVAILABLE.to_string())
    }
    pub fn is_loaded(&self) -> bool {
        false
    }
    pub fn extract_entities(
        &self,
        _text: &str,
        _speaker: &str,
    ) -> Result<ExtractionResult, String> {
        Err(LLAMA_UNAVAILABLE.to_string())
    }
    pub fn chat(
        &self,
        _messages: &[ChatMessage],
        _graph_context: &str,
    ) -> Result<ChatOutcome, String> {
        Err(LLAMA_UNAVAILABLE.to_string())
    }
    pub fn stream_chat(
        &self,
        _messages: Vec<ChatMessage>,
        _graph_context: String,
        _params: LlmChatParams,
        _cancel: CancellationToken,
        _events: tokio::sync::mpsc::Sender<LlmStreamEvent>,
    ) -> Result<(), String> {
        Err(LLAMA_UNAVAILABLE.to_string())
    }
}
