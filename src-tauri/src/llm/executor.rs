//! Priority executor for LLM-backed work.
//!
//! Entity extraction is background work; chat/agent requests are interactive
//! work. Running both through this single executor prevents background
//! extraction jobs from monopolizing the shared LLM/API handles.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};

use crate::graph::entities::ExtractionResult;
use crate::llm::engine::{ChatMessage, ChatOutcome};
use crate::llm::{ApiClient, LlmEngine, MistralRsEngine, OpenRouterClient};
use crate::settings::LlmProvider;

// ---------------------------------------------------------------------------
// Extraction rate-limit backoff
// ---------------------------------------------------------------------------
//
// Background extraction fires once per transcript segment (~every 2s). On a
// rate-limited endpoint (e.g. an OpenRouter `:free` model capped at 16/min)
// this both burns the quota the interactive chat needs and floods the logs
// with 429s. When we see a 429 we pause ALL background extraction for a
// cooldown window so the user's quota is preserved for chat.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static EXTRACTION_COOLDOWN_UNTIL_MS: AtomicU64 = AtomicU64::new(0);
const EXTRACTION_COOLDOWN_MS: u64 = 60_000;

// ---------------------------------------------------------------------------
// Background queue bound
// ---------------------------------------------------------------------------
//
// Background extraction is submitted once per transcript segment and blocks on
// the single executor worker. If extraction is slower than ingest (slow/remote
// LLM, long prompts), the background queue can grow without bound and OOM a
// long session. We cap it and drop the OLDEST pending background job when full
// — its caller's `recv()` then returns `Err` and falls back to rule-based
// extraction, exactly like the lossy `try_send` audio path. Interactive (chat)
// work is user-paced and stays unbounded.
const MAX_BACKGROUND_QUEUE: usize = 32;

/// Count of background jobs dropped due to a full queue (for log throttling).
static DROPPED_BACKGROUND_JOBS: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// True while background extraction is paused after a recent rate-limit.
pub fn extraction_in_cooldown() -> bool {
    now_ms() < EXTRACTION_COOLDOWN_UNTIL_MS.load(Ordering::Relaxed)
}

fn is_rate_limited(err: &str) -> bool {
    err.contains("429")
        || err.contains("Too Many Requests")
        || err.to_ascii_lowercase().contains("rate limit")
}

/// If `err` looks like a rate-limit, start/extend the extraction cooldown.
fn note_extraction_error(err: &str) {
    if is_rate_limited(err) {
        EXTRACTION_COOLDOWN_UNTIL_MS.store(now_ms() + EXTRACTION_COOLDOWN_MS, Ordering::Relaxed);
        log::warn!(
            "Extraction rate-limited (429) — pausing background extraction for {}s to preserve \
             quota for chat. Consider a non-`:free` OpenRouter model or adding credits.",
            EXTRACTION_COOLDOWN_MS / 1000
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmPriority {
    Interactive,
    Background,
}

#[derive(Clone)]
pub struct LlmExecutor {
    queue: Arc<(Mutex<QueueState>, Condvar)>,
}

struct QueueState {
    interactive: VecDeque<LlmJob>,
    background: VecDeque<LlmJob>,
}

enum LlmJob {
    Extract {
        text: String,
        speaker: String,
        context: String,
        provider: LlmProvider,
        response_tx: mpsc::Sender<LlmJobResult>,
    },
    Chat {
        messages: Vec<ChatMessage>,
        graph_context: String,
        provider: LlmProvider,
        response_tx: mpsc::Sender<LlmJobResult>,
    },
}

enum LlmJobResult {
    Extraction(Option<ExtractionResult>),
    Chat(Result<ChatOutcome, String>),
}

struct BackendHandles {
    llm_engine: Arc<Mutex<Option<LlmEngine>>>,
    api_client: Arc<Mutex<Option<ApiClient>>>,
    openrouter_client: Arc<Mutex<Option<OpenRouterClient>>>,
    mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>>,
}

impl LlmExecutor {
    pub fn new(
        llm_engine: Arc<Mutex<Option<LlmEngine>>>,
        api_client: Arc<Mutex<Option<ApiClient>>>,
        openrouter_client: Arc<Mutex<Option<OpenRouterClient>>>,
        mistralrs_engine: Arc<Mutex<Option<MistralRsEngine>>>,
    ) -> Self {
        let queue = Arc::new((
            Mutex::new(QueueState {
                interactive: VecDeque::new(),
                background: VecDeque::new(),
            }),
            Condvar::new(),
        ));
        let worker_queue = queue.clone();
        let handles = BackendHandles {
            llm_engine,
            api_client,
            openrouter_client,
            mistralrs_engine,
        };

        let _ = std::thread::Builder::new()
            .name("llm-executor".to_string())
            .spawn(move || worker_loop(worker_queue, handles))
            .map_err(|e| log::error!("Failed to spawn LLM executor thread: {}", e));

        Self { queue }
    }

    pub fn extract_entities(
        &self,
        text: String,
        speaker: String,
        context: String,
        provider: LlmProvider,
        priority: LlmPriority,
    ) -> Option<ExtractionResult> {
        let (response_tx, response_rx) = mpsc::channel();
        self.enqueue(
            priority,
            LlmJob::Extract {
                text,
                speaker,
                context,
                provider,
                response_tx,
            },
        );

        match response_rx.recv() {
            Ok(LlmJobResult::Extraction(result)) => result,
            Ok(LlmJobResult::Chat(_)) => {
                log::warn!("LLM executor returned chat result for extraction request");
                None
            }
            Err(e) => {
                log::warn!("LLM executor extraction response failed: {}", e);
                None
            }
        }
    }

    /// Run an interactive chat through the executor and return the generated
    /// text plus the token usage the backend reported. Backends that surface a
    /// real count (the native `LlmEngine`) populate `tokens_used`; the others
    /// report 0 (see the `chat_*` attempt fns) — never fabricated.
    pub fn chat_with_history(
        &self,
        messages: Vec<ChatMessage>,
        graph_context: String,
        provider: LlmProvider,
    ) -> Result<ChatOutcome, String> {
        let (response_tx, response_rx) = mpsc::channel();
        self.enqueue(
            LlmPriority::Interactive,
            LlmJob::Chat {
                messages,
                graph_context,
                provider,
                response_tx,
            },
        );

        match response_rx.recv() {
            Ok(LlmJobResult::Chat(result)) => result,
            Ok(LlmJobResult::Extraction(_)) => {
                Err("LLM executor returned extraction result for chat request".to_string())
            }
            Err(e) => Err(format!("LLM executor chat response failed: {}", e)),
        }
    }

    fn enqueue(&self, priority: LlmPriority, job: LlmJob) {
        let (lock, cvar) = &*self.queue;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        push_job(&mut state, priority, job);
        cvar.notify_one();
    }
}

/// Push a job onto the appropriate priority queue, applying the drop-oldest
/// bound to background work.
///
/// Pure data-structure logic, lifted out of `enqueue` so the bound +
/// drop-oldest ordering can be unit-tested without spawning the worker
/// thread. Behaviour is identical to the prior inline body.
fn push_job(state: &mut QueueState, priority: LlmPriority, job: LlmJob) {
    match priority {
        LlmPriority::Interactive => state.interactive.push_back(job),
        LlmPriority::Background => {
            // Bound the background queue (drop-oldest). Dropping the front
            // job drops its `response_tx`, so the blocked caller's `recv()`
            // returns Err → None → rule-based fallback.
            while state.background.len() >= MAX_BACKGROUND_QUEUE {
                state.background.pop_front();
                let n = DROPPED_BACKGROUND_JOBS.fetch_add(1, Ordering::Relaxed) + 1;
                if n % 10 == 1 {
                    log::warn!(
                        "LLM executor background queue full ({} jobs); dropping oldest \
                         extraction job (total dropped: {}). Extraction is falling behind \
                         ingest — consider a faster LLM provider.",
                        MAX_BACKGROUND_QUEUE,
                        n
                    );
                }
            }
            state.background.push_back(job);
        }
    }
}

/// Pop the next job to run: interactive work is drained before background
/// work. Lifted out of `worker_loop`'s pop expression so the
/// interactive-before-background ordering can be unit-tested.
fn pop_next_job(state: &mut QueueState) -> Option<LlmJob> {
    state
        .interactive
        .pop_front()
        .or_else(|| state.background.pop_front())
}

fn worker_loop(queue: Arc<(Mutex<QueueState>, Condvar)>, handles: BackendHandles) {
    loop {
        let job = {
            let (lock, cvar) = &*queue;
            let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
            while state.interactive.is_empty() && state.background.is_empty() {
                state = cvar.wait(state).unwrap_or_else(|e| e.into_inner());
            }
            pop_next_job(&mut state)
        };

        let Some(job) = job else {
            continue;
        };

        match job {
            LlmJob::Extract {
                text,
                speaker,
                context,
                provider,
                response_tx,
            } => {
                let result = run_extraction(&handles, &provider, &text, &speaker, &context);
                let _ = response_tx.send(LlmJobResult::Extraction(result));
            }
            LlmJob::Chat {
                messages,
                graph_context,
                provider,
                response_tx,
            } => {
                let result = run_chat(&handles, &provider, &messages, &graph_context);
                let _ = response_tx.send(LlmJobResult::Chat(result));
            }
        }
    }
}

fn run_extraction(
    handles: &BackendHandles,
    provider: &LlmProvider,
    text: &str,
    speaker: &str,
    context: &str,
) -> Option<ExtractionResult> {
    // Skip background extraction entirely while cooling down from a 429 so we
    // don't keep hammering a rate-limited endpoint.
    if extraction_in_cooldown() {
        return None;
    }
    match provider {
        LlmProvider::LocalLlama => extract_native(handles, text, speaker)
            .or_else(|| extract_openrouter(handles, text, speaker, context))
            .or_else(|| extract_api(handles, text, speaker, context))
            .or_else(|| extract_mistralrs(handles, text, speaker)),
        LlmProvider::OpenRouter { .. } => extract_openrouter(handles, text, speaker, context)
            .or_else(|| extract_api(handles, text, speaker, context))
            .or_else(|| extract_native(handles, text, speaker))
            .or_else(|| extract_mistralrs(handles, text, speaker)),
        LlmProvider::Api { .. } | LlmProvider::AwsBedrock { .. } => {
            extract_api(handles, text, speaker, context)
                .or_else(|| extract_openrouter(handles, text, speaker, context))
                .or_else(|| extract_native(handles, text, speaker))
                .or_else(|| extract_mistralrs(handles, text, speaker))
        }
        LlmProvider::MistralRs { .. } => extract_mistralrs(handles, text, speaker)
            .or_else(|| extract_native(handles, text, speaker))
            .or_else(|| extract_openrouter(handles, text, speaker, context))
            .or_else(|| extract_api(handles, text, speaker, context)),
    }
}

/// A single chat backend attempt: same signature for every provider so the
/// fallback chain can be expressed as a slice.
type ChatAttemptFn = fn(&BackendHandles, &[ChatMessage], &str) -> Result<ChatOutcome, String>;

fn run_chat(
    handles: &BackendHandles,
    provider: &LlmProvider,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<ChatOutcome, String> {
    let attempts: &[ChatAttemptFn] = match provider {
        LlmProvider::LocalLlama => &[chat_native, chat_openrouter, chat_api, chat_mistralrs],
        LlmProvider::OpenRouter { .. } => &[chat_openrouter, chat_api, chat_native, chat_mistralrs],
        LlmProvider::Api { .. } | LlmProvider::AwsBedrock { .. } => {
            &[chat_api, chat_openrouter, chat_native, chat_mistralrs]
        }
        LlmProvider::MistralRs { .. } => &[chat_mistralrs, chat_native, chat_openrouter, chat_api],
    };

    run_attempts(attempts, |attempt| {
        attempt(handles, messages, graph_context)
    })
}

/// Walk a fallback chain: invoke each attempt via `run` in order, return the
/// first `Ok`, or the **last** `Err` if every attempt fails (a default
/// message if the chain is empty).
///
/// Generic over the attempt type, the success type, and the invoker closure so
/// it can be unit-tested with recorder closures (no backends, no network).
/// Behaviour is identical to the prior inline loop in `run_chat`.
fn run_attempts<A, T>(
    attempts: &[A],
    mut run: impl FnMut(&A) -> Result<T, String>,
) -> Result<T, String> {
    let mut last_error = None;
    for attempt in attempts {
        match run(attempt) {
            Ok(value) => return Ok(value),
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| "No LLM backend configured".to_string()))
}

fn extract_native(handles: &BackendHandles, text: &str, speaker: &str) -> Option<ExtractionResult> {
    let guard = handles.llm_engine.lock().unwrap_or_else(|e| e.into_inner());
    let engine = guard.as_ref()?;
    match engine.extract_entities(text, speaker) {
        Ok(result) => Some(result),
        Err(e) => {
            log::warn!("Native LLM extraction failed: {}", e);
            None
        }
    }
}

fn extract_api(
    handles: &BackendHandles,
    text: &str,
    speaker: &str,
    context: &str,
) -> Option<ExtractionResult> {
    // Clone the client and release the mutex BEFORE the blocking HTTP call, so
    // a long-running extraction request never blocks interactive chat (which
    // needs the same client lock). See executor.rs lock-scope note.
    let client = {
        let guard = handles.api_client.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref()?.clone()
    };
    match client.extract_entities(text, speaker, context) {
        Ok(result) => Some(result),
        Err(e) => {
            log::warn!("API extraction failed: {}", e);
            note_extraction_error(&e);
            None
        }
    }
}

fn extract_openrouter(
    handles: &BackendHandles,
    text: &str,
    speaker: &str,
    context: &str,
) -> Option<ExtractionResult> {
    // Clone + drop the guard before the blocking HTTP request (see extract_api).
    let client = {
        let guard = handles
            .openrouter_client
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        guard.as_ref()?.clone()
    };
    match client.extract_entities(text, speaker, context) {
        Ok(result) => Some(result),
        Err(e) => {
            log::warn!("OpenRouter extraction failed: {}", e);
            note_extraction_error(&e);
            None
        }
    }
}

fn extract_mistralrs(
    handles: &BackendHandles,
    text: &str,
    speaker: &str,
) -> Option<ExtractionResult> {
    let guard = handles
        .mistralrs_engine
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let engine = guard.as_ref()?;
    match engine.extract_entities(text, speaker) {
        Ok(result) => Some(result),
        Err(e) => {
            log::warn!("mistral.rs extraction failed: {}", e);
            None
        }
    }
}

fn chat_native(
    handles: &BackendHandles,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<ChatOutcome, String> {
    let guard = handles.llm_engine.lock().map_err(|e| e.to_string())?;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Native LLM is not loaded".to_string())?;
    // The native engine's inference loop counts prompt + completion tokens, so
    // this is the one blocking backend that surfaces a real `tokens_used`.
    engine.chat(messages, graph_context)
}

fn chat_api(
    handles: &BackendHandles,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<ChatOutcome, String> {
    // Clone + drop the guard before the blocking HTTP request (see extract_api).
    let client = {
        let guard = handles.api_client.lock().map_err(|e| e.to_string())?;
        guard
            .as_ref()
            .ok_or_else(|| "API LLM client is not configured".to_string())?
            .clone()
    };
    // `ApiClient::chat_with_history_with_usage` (and the Bedrock requests routed
    // through it) surfaces the OpenAI `usage.total_tokens` from the response.
    // A provider that omits the `usage` block reports 0 — never fabricated
    // (FA-7c).
    client
        .chat_with_history_with_usage(messages, graph_context)
        .map(|(text, tokens_used)| ChatOutcome { text, tokens_used })
}

fn chat_openrouter(
    handles: &BackendHandles,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<ChatOutcome, String> {
    // Clone + drop the guard before the blocking HTTP request (see extract_api).
    let client = {
        let guard = handles
            .openrouter_client
            .lock()
            .map_err(|e| e.to_string())?;
        guard
            .as_ref()
            .ok_or_else(|| "OpenRouter client is not configured".to_string())?
            .clone()
    };
    // `OpenRouterClient::chat_with_history` (blocking) returns only the reply
    // text; usage is reported on the streaming path, not this one. tokens_used
    // is 0 here, not fabricated — wiring it needs an openrouter.rs change (out of
    // this file set; NEW BACKLOG FA-7c).
    client
        .chat_with_history(messages, graph_context)
        .map(|text| ChatOutcome {
            text,
            tokens_used: 0,
        })
}

fn chat_mistralrs(
    handles: &BackendHandles,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<ChatOutcome, String> {
    let guard = handles.mistralrs_engine.lock().map_err(|e| e.to_string())?;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "mistral.rs LLM is not loaded".to_string())?;
    // `MistralRsEngine::chat_with_history` returns only the reply text; the
    // mistral.rs `ChatCompletionResponse.usage` is not surfaced through that
    // signature. tokens_used is 0 here, not fabricated — wiring it needs a
    // mistralrs_engine.rs change (out of this file set; NEW BACKLOG FA-7c).
    engine
        .chat_with_history(messages, graph_context)
        .map(|text| ChatOutcome {
            text,
            tokens_used: 0,
        })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    /// Serialize the cooldown tests: they read/mutate the process-wide
    /// `EXTRACTION_COOLDOWN_UNTIL_MS` atomic, so two running concurrently
    /// would race. A plain `Mutex` guard around the body keeps them ordered.
    static COOLDOWN_TEST_LOCK: StdMutex<()> = StdMutex::new(());

    fn empty_handles() -> BackendHandles {
        BackendHandles {
            llm_engine: Arc::new(Mutex::new(None)),
            api_client: Arc::new(Mutex::new(None)),
            openrouter_client: Arc::new(Mutex::new(None)),
            mistralrs_engine: Arc::new(Mutex::new(None)),
        }
    }

    // ----- is_rate_limited --------------------------------------------------

    #[test]
    fn is_rate_limited_matches_known_signals() {
        assert!(is_rate_limited("API error 429 from endpoint"));
        assert!(is_rate_limited("Too Many Requests"));
        assert!(is_rate_limited("rate limit exceeded"));
        // case-insensitive on the "rate limit" phrase
        assert!(is_rate_limited("RATE LIMIT reached"));
        assert!(is_rate_limited("Provider says: Rate Limit hit"));
    }

    #[test]
    fn is_rate_limited_rejects_plain_errors() {
        assert!(!is_rate_limited("connection refused"));
        assert!(!is_rate_limited("500 Internal Server Error"));
        assert!(!is_rate_limited("No LLM backend configured"));
        assert!(!is_rate_limited(""));
    }

    // ----- cooldown set / observe ------------------------------------------

    #[test]
    fn note_extraction_error_sets_cooldown_for_rate_limit() {
        let _guard = COOLDOWN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Reset to a clean state (no cooldown).
        EXTRACTION_COOLDOWN_UNTIL_MS.store(0, Ordering::Relaxed);
        assert!(!extraction_in_cooldown());

        note_extraction_error("HTTP 429 Too Many Requests");
        assert!(
            extraction_in_cooldown(),
            "a 429 error must start the cooldown window"
        );

        // Restore so other tests / the real app aren't affected.
        EXTRACTION_COOLDOWN_UNTIL_MS.store(0, Ordering::Relaxed);
    }

    #[test]
    fn note_extraction_error_ignores_plain_errors() {
        let _guard = COOLDOWN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        EXTRACTION_COOLDOWN_UNTIL_MS.store(0, Ordering::Relaxed);

        note_extraction_error("connection refused");
        assert!(
            !extraction_in_cooldown(),
            "a non-rate-limit error must NOT start the cooldown"
        );

        EXTRACTION_COOLDOWN_UNTIL_MS.store(0, Ordering::Relaxed);
    }

    // ----- run_attempts fallback loop --------------------------------------

    #[test]
    fn run_attempts_returns_first_ok_and_stops() {
        let calls = Arc::new(Mutex::new(Vec::<usize>::new()));
        // Three attempts: first fails, second succeeds, third must NOT run.
        let attempts: Vec<Box<dyn Fn() -> Result<String, String>>> = vec![
            {
                let calls = calls.clone();
                Box::new(move || {
                    calls.lock().unwrap().push(0);
                    Err("first failed".to_string())
                })
            },
            {
                let calls = calls.clone();
                Box::new(move || {
                    calls.lock().unwrap().push(1);
                    Ok("second ok".to_string())
                })
            },
            {
                let calls = calls.clone();
                Box::new(move || {
                    calls.lock().unwrap().push(2);
                    Ok("third ok".to_string())
                })
            },
        ];

        let result = run_attempts(&attempts, |a| a());
        assert_eq!(result, Ok("second ok".to_string()));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![0, 1],
            "attempts run in order and stop at the first Ok"
        );
    }

    #[test]
    fn run_attempts_returns_last_error_when_all_fail() {
        let attempts: Vec<&str> = vec!["a", "b", "c"];
        // Every attempt errors, so the Ok type is unconstrained — pin it.
        let result: Result<String, String> =
            run_attempts(&attempts, |&name| Err(format!("{name} failed")));
        assert_eq!(
            result,
            Err("c failed".to_string()),
            "the LAST error surfaces when every attempt fails"
        );
    }

    #[test]
    fn run_attempts_empty_chain_returns_default() {
        let attempts: Vec<&str> = vec![];
        let result: Result<String, String> = run_attempts(&attempts, |_| Ok("never".to_string()));
        assert_eq!(result, Err("No LLM backend configured".to_string()));
    }

    // ----- chat token usage flows through the fallback chain ----------------

    /// The real seam the blocking chat path uses: `run_chat` → `run_attempts`
    /// over `ChatAttemptFn`s, each returning a `ChatOutcome`. This asserts that
    /// when an attempt reports a non-zero `tokens_used`, that count is preserved
    /// (not dropped or zeroed) on the way back to `chat_with_history` /
    /// `send_chat_message`. Uses recorder closures so no backend/model/network
    /// is needed.
    #[test]
    fn run_attempts_preserves_chat_token_count() {
        // First attempt fails; second succeeds with a real (non-zero) count.
        let attempts: Vec<&str> = vec!["fail", "ok"];
        let result = run_attempts(&attempts, |&name| {
            if name == "ok" {
                Ok(ChatOutcome {
                    text: "hi".to_string(),
                    tokens_used: 42,
                })
            } else {
                Err(format!("{name} failed"))
            }
        });
        let outcome = result.expect("the second attempt succeeds");
        assert_eq!(outcome.text, "hi");
        assert_eq!(
            outcome.tokens_used, 42,
            "a non-zero token count from a backend must flow through unchanged"
        );
    }

    // ----- push_job + pop_next_job queue semantics -------------------------

    fn new_state() -> QueueState {
        QueueState {
            interactive: VecDeque::new(),
            background: VecDeque::new(),
        }
    }

    /// Build a `Chat` job tagged with `graph_context` so we can identify which
    /// job survived the queue, plus the receiver to assert drop semantics.
    fn chat_job(tag: &str) -> (LlmJob, mpsc::Receiver<LlmJobResult>) {
        let (tx, rx) = mpsc::channel();
        let job = LlmJob::Chat {
            messages: Vec::new(),
            graph_context: tag.to_string(),
            provider: LlmProvider::LocalLlama,
            response_tx: tx,
        };
        (job, rx)
    }

    fn job_tag(job: &LlmJob) -> String {
        match job {
            LlmJob::Chat { graph_context, .. } => graph_context.clone(),
            LlmJob::Extract { context, .. } => context.clone(),
        }
    }

    #[test]
    fn push_job_drops_oldest_background_when_full() {
        let mut state = new_state();
        // Push MAX_BACKGROUND_QUEUE + 1 background jobs; keep the first
        // receiver so we can assert its sender was dropped.
        let (first_job, first_rx) = chat_job("bg-0");
        push_job(&mut state, LlmPriority::Background, first_job);
        for i in 1..MAX_BACKGROUND_QUEUE {
            let (job, _rx) = chat_job(&format!("bg-{i}"));
            // Leak the receiver so it stays alive (sender not dropped) and the
            // queue stays bounded purely by the drop-oldest logic.
            std::mem::forget(_rx);
            push_job(&mut state, LlmPriority::Background, job);
        }
        assert_eq!(state.background.len(), MAX_BACKGROUND_QUEUE);

        // One more overflows → oldest (bg-0) is dropped.
        let (overflow_job, overflow_rx) = chat_job("bg-overflow");
        std::mem::forget(overflow_rx);
        push_job(&mut state, LlmPriority::Background, overflow_job);

        assert_eq!(
            state.background.len(),
            MAX_BACKGROUND_QUEUE,
            "queue stays bounded at MAX_BACKGROUND_QUEUE"
        );
        // The dropped front job's response_tx is gone → the caller's recv()
        // returns Err (the key correctness property → rule-based fallback).
        assert!(
            first_rx.recv().is_err(),
            "dropping the oldest background job must drop its response_tx so the \
             caller's recv() returns Err"
        );
        // The oldest tag should no longer be present.
        assert!(
            !state.background.iter().any(|j| job_tag(j) == "bg-0"),
            "oldest background job (bg-0) must have been dropped"
        );
        // The newest tag should be present.
        assert!(
            state.background.iter().any(|j| job_tag(j) == "bg-overflow"),
            "newest background job must be retained"
        );
    }

    #[test]
    fn pop_next_job_drains_interactive_before_background() {
        let mut state = new_state();
        let (bg_job, _bg_rx) = chat_job("background");
        let (int_job, _int_rx) = chat_job("interactive");
        // Background enqueued first, interactive second.
        push_job(&mut state, LlmPriority::Background, bg_job);
        push_job(&mut state, LlmPriority::Interactive, int_job);

        // Despite arriving second, interactive must pop first.
        let first = pop_next_job(&mut state).expect("a job is available");
        assert_eq!(job_tag(&first), "interactive");
        let second = pop_next_job(&mut state).expect("background remains");
        assert_eq!(job_tag(&second), "background");
        assert!(pop_next_job(&mut state).is_none(), "queue is now empty");
    }

    #[test]
    fn interactive_queue_is_unbounded() {
        let mut state = new_state();
        for i in 0..(MAX_BACKGROUND_QUEUE * 2) {
            let (job, _rx) = chat_job(&format!("int-{i}"));
            std::mem::forget(_rx);
            push_job(&mut state, LlmPriority::Interactive, job);
        }
        assert_eq!(
            state.interactive.len(),
            MAX_BACKGROUND_QUEUE * 2,
            "interactive work is user-paced and never drops"
        );
    }

    // ----- run_chat / run_extraction with no backends ----------------------

    #[test]
    fn run_chat_with_no_backends_reports_first_attempt_error() {
        let handles = empty_handles();
        // LocalLlama order = [native, openrouter, api, mistralrs]; all None.
        // Every attempt errors → the LAST attempt's error surfaces.
        let err = run_chat(&handles, &LlmProvider::LocalLlama, &[], "ctx")
            .expect_err("no backends → Err");
        assert!(
            err.contains("mistral.rs LLM is not loaded"),
            "last attempt (mistralrs) error should surface, got: {err}"
        );
    }

    #[test]
    fn run_extraction_with_no_backends_returns_none() {
        let _guard = COOLDOWN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        EXTRACTION_COOLDOWN_UNTIL_MS.store(0, Ordering::Relaxed);
        let handles = empty_handles();
        let result = run_extraction(
            &handles,
            &LlmProvider::OpenRouter {
                model: String::new(),
                base_url: String::new(),
                provider_order: None,
                include_usage_in_stream: false,
                api_key: String::new(),
            },
            "Alice met Bob",
            "Alice",
            "",
        );
        assert!(
            result.is_none(),
            "with no backends configured, extraction yields None (→ rule-based fallback)"
        );
    }

    // ----- end-to-end through the live worker thread -----------------------

    #[test]
    fn executor_chat_with_no_backends_returns_err_not_panic() {
        let exec = LlmExecutor::new(
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
        );
        // Drives enqueue → worker_loop → run_chat → response over the channel.
        let result = exec.chat_with_history(Vec::new(), String::new(), LlmProvider::LocalLlama);
        assert!(result.is_err(), "chat with no backends resolves to an Err");
    }

    #[test]
    fn executor_background_extraction_with_no_backends_returns_none() {
        let _guard = COOLDOWN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        EXTRACTION_COOLDOWN_UNTIL_MS.store(0, Ordering::Relaxed);
        let exec = LlmExecutor::new(
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
        );
        let result = exec.extract_entities(
            "text".to_string(),
            "speaker".to_string(),
            String::new(),
            LlmProvider::LocalLlama,
            LlmPriority::Background,
        );
        assert!(result.is_none());
    }
}
