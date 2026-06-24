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
        reply: SyncSender<Result<String, String>>,
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
    pub fn chat(&self, messages: &[ChatMessage], graph_context: &str) -> Result<String, String> {
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

    let output = run_inference(model, ctx, &prompt, 512, sampler)?;

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
) -> Result<String, String> {
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
        LlamaSampler::temp(0.7),
        LlamaSampler::dist(seed),
    ]);

    run_inference(model, ctx, &prompt, 512, sampler)
}

/// Core inference loop shared by grammar-constrained and free-form generation.
///
/// Runs on the actor's **persistent** [`LlamaContext`]. The KV cache is fully
/// cleared first so each call is independent and deterministic — only the
/// per-call context allocation is eliminated (ADR-0012 Phase 0a).
#[cfg(feature = "llm-llama")]
fn run_inference(
    model: &LlamaModel,
    ctx: &mut LlamaContext,
    prompt: &str,
    max_tokens: u32,
    mut sampler: LlamaSampler,
) -> Result<String, String> {
    // Reset the persistent context to an empty-KV state (== fresh context).
    ctx.clear_kv_cache();

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

    let mut output = String::new();
    let mut decoder = encoding_rs::UTF_8.new_decoder();

    // The prompt occupies KV positions [0, prompt_len); each generated token
    // continues from there. Deriving the absolute position from the loop index
    // (rather than a stale/zero batch position) is required — appending at a
    // position that collides with the prompt's KV entries makes `llama_decode`
    // fail with ret=-1.
    let prompt_len = tokens.len() as i32;

    for i in 0..max_tokens as i32 {
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

        batch.clear();
        batch
            .add(new_token, prompt_len + i, &[0], true)
            .map_err(|e| format!("Failed to add token: {}", e))?;

        ctx.decode(&mut batch)
            .map_err(|e| format!("Decode failed: {}", e))?;
    }

    Ok(output.trim().to_string())
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
        assert!(!r1.trim().is_empty(), "chat reply should be non-empty");
        assert!(
            !r2.trim().is_empty(),
            "second chat reply on the reused context should be non-empty"
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
    pub fn chat(&self, _messages: &[ChatMessage], _graph_context: &str) -> Result<String, String> {
        Err(LLAMA_UNAVAILABLE.to_string())
    }
}
