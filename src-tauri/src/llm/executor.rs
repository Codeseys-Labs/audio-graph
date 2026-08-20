//! Priority executor for LLM-backed work.
//!
//! Entity extraction is background work; chat/agent requests are interactive
//! work. Running both through this single executor prevents background
//! extraction jobs from monopolizing the shared LLM/API handles.

use std::collections::{BTreeSet, VecDeque};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, LazyLock, Mutex};

use crate::error::AppError;
use crate::graph::entities::ExtractionResult;
use crate::llm::engine::{ChatMessage, ChatOutcome};
use crate::llm::{ApiClient, LlmEngine, MistralRsEngine, OpenRouterClient};
use crate::projection_llm::{
    PROJECTION_PATCH_PROMPT_ID, PROJECTION_PATCH_REPAIR_PROMPT_ID, ProjectionPatchBuildContext,
    ProjectionPatchDraftError, projection_patch_draft_json_schema,
    projection_patch_prompt_messages, projection_patch_repair_prompt_messages,
    projection_patch_strict_json_schema, trusted_projection_patch_from_model_json,
};
use crate::projections::{ProjectionJob, ProjectionKind, ProjectionPatch, TranscriptLedger};
use crate::settings::LlmProvider;

use super::route::{
    AuthorizedRoute, BlockingBackend, ConstrainedDecodingGrade, ModelIdentitySource,
    RequestOutputForm, RouteRecord, TerminalStatus, WireOutcome, authorize_route_dispatch,
    retry_class_for_terminal_status, route_for_api_endpoint, route_for_openrouter_policy,
};

/// Models where the structured-outputs (`response_format: json_schema`) request
/// is worth skipping for the rest of this process run because NO provider for
/// the model supports schema-constrained output. We send the schema request with
/// `provider.require_parameters=true` (see openrouter.rs), so a "no providers"
/// (404-class) response is genuine model-level evidence: no endpoint behind the
/// model slug can honor the schema. Caching that avoids paying the doomed
/// request + fallback on every projection tick (seed audio-graph-a324).
///
/// We do NOT cache a 400/422 here: with `require_parameters` set, a routed-to
/// provider that DOES advertise schema support can still 400/422 on a specific
/// schema (validation quirk), which is not evidence that the whole MODEL lacks
/// support — caching it would wrongly demote a schema-capable model to JSON mode
/// for the session. Session-scoped and best-effort — a stale entry only costs
/// one JSON-mode call, never correctness.
static OPENROUTER_SCHEMA_UNSUPPORTED: LazyLock<Mutex<BTreeSet<String>>> =
    LazyLock::new(|| Mutex::new(BTreeSet::new()));

/// Whether `model` has been recorded as having no schema-capable provider this run.
fn openrouter_schema_unsupported(model: &str) -> bool {
    OPENROUTER_SCHEMA_UNSUPPORTED
        .lock()
        .map(|set| set.contains(model))
        .unwrap_or(false)
}

/// Remember that `model` has no schema-capable provider this run.
fn note_openrouter_schema_unsupported(model: &str) {
    if let Ok(mut set) = OPENROUTER_SCHEMA_UNSUPPORTED.lock() {
        set.insert(model.to_string());
    }
}

/// Whether a provider error means the structured-outputs request should **retry
/// without the schema** — a 400/404/422-class rejection, as opposed to a
/// transient failure or an auth error (which must NOT trigger the schema-less
/// retry: a schema-less call would just fail the same way on auth, and transient
/// errors are already retried in-client). Both blocking error strings carry
/// `status=<code>` (`openrouter_http_error_message` / `api_error_message`).
///
/// The retry it authorizes is a MODE downgrade on the SAME authorized route —
/// json_schema to json_object — never a provider substitution (ADR-0038).
fn is_schema_rejection(err: &str) -> bool {
    err.contains("status=400") || err.contains("status=404") || err.contains("status=422")
}

/// Whether a schema rejection is **model-level** evidence worth caching (vs. a
/// single-provider validation quirk). With `require_parameters=true` on the
/// request, a 404-class response means "no endpoint for this model supports the
/// requested params" — that applies to the whole model slug, so it is
/// cache-worthy. A 400/422 came from a provider that WAS routed to (it advertised
/// support) and is schema-specific, not model-level, so it is not cached (seed
/// audio-graph-a324, Codex P2).
fn is_openrouter_schema_unsupported_by_model(err: &str) -> bool {
    err.contains("status=404")
}

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

/// A queued unit of LLM work.
///
/// `provider` is the session's configured provider; the worker resolves it to
/// exactly ONE authorized route (`llm/route.rs`). There is no per-job
/// fallback-policy flag: ADR-0038 removed automatic cross-provider fallback, so
/// there is nothing for a flag to select.
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
    ProjectionPatch {
        job: ProjectionJob,
        ledger: TranscriptLedger,
        sequence: u64,
        created_at_ms: u64,
        provider: LlmProvider,
        response_tx: mpsc::Sender<LlmJobResult>,
    },
}

// Channel result enum: boxing the large `ProjectionPatch` variant would
// ripple through every construction and match site for negligible benefit.
#[allow(clippy::large_enum_variant)]
enum LlmJobResult {
    Extraction(Option<ExtractionResult>),
    Chat(Result<ChatOutcome, String>),
    ProjectionPatch(Result<ProjectionPatchOutcome, String>),
}

#[derive(Debug, Clone)]
pub struct ProjectionPatchOutcome {
    pub patch: ProjectionPatch,
    pub tokens_used: u32,
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
            Ok(LlmJobResult::ProjectionPatch(_)) => {
                log::warn!("LLM executor returned projection result for extraction request");
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
            Ok(LlmJobResult::ProjectionPatch(_)) => {
                Err("LLM executor returned projection result for chat request".to_string())
            }
            Err(e) => Err(format!("LLM executor chat response failed: {}", e)),
        }
    }

    /// Generate a structured notes/graph projection patch from a basis-bound
    /// projection job.
    ///
    /// Runtime projection dispatch calls this from live ASR observation after
    /// the scheduler starts a basis-bound job. Callers must still validate and
    /// apply the returned patch through `AppState::apply_runtime_projection_patch`.
    pub fn generate_projection_patch(
        &self,
        job: ProjectionJob,
        ledger: TranscriptLedger,
        provider: LlmProvider,
        sequence: u64,
        created_at_ms: u64,
    ) -> Result<ProjectionPatchOutcome, String> {
        let (response_tx, response_rx) = mpsc::channel();
        self.enqueue(
            LlmPriority::Background,
            LlmJob::ProjectionPatch {
                job,
                ledger,
                sequence,
                created_at_ms,
                provider,
                response_tx,
            },
        );

        match response_rx.recv() {
            Ok(LlmJobResult::ProjectionPatch(result)) => result,
            Ok(LlmJobResult::Extraction(_)) => {
                Err("LLM executor returned extraction result for projection request".to_string())
            }
            Ok(LlmJobResult::Chat(_)) => {
                Err("LLM executor returned chat result for projection request".to_string())
            }
            Err(e) => Err(format!("LLM executor projection response failed: {}", e)),
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
            LlmJob::ProjectionPatch {
                job,
                ledger,
                sequence,
                created_at_ms,
                provider,
                response_tx,
            } => {
                let result = run_projection_patch(
                    &handles,
                    &provider,
                    &job,
                    &ledger,
                    sequence,
                    created_at_ms,
                );
                let _ = response_tx.send(LlmJobResult::ProjectionPatch(result));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Dispatch: exactly one authorized route per job (ADR-0038)
// ---------------------------------------------------------------------------
//
// There is no chain walker in this module and no per-provider attempt list.
// `authorize_route_dispatch` resolves the session's provider to ONE named route,
// runs the ADR-0033 start gate, and hands back an `AuthorizedRoute` token that
// every backend function below requires. Because that token has a private field
// and only one constructor, a dispatch that skipped the gate does not compile.

/// Peek the completion budget the dispatch will request, from the backend
/// handle the route will actually use — without holding the lock across the
/// blocking HTTP call that happens later in the same job.
///
/// `None` for `NativeLlama` / `MistralRs`: neither route row in
/// [`crate::llm::route::LLM_ROUTES`] documents a per-endpoint completion
/// maximum, so [`crate::llm::route::RouteDescriptor::clamp_completion_budget`]
/// would be a no-op for them regardless of what is passed in. `None` also when
/// the corresponding client is not yet configured; the dispatch fails with its
/// own "not configured" error immediately after, so there is nothing to clamp.
fn requested_completion_budget(handles: &BackendHandles, route: &AuthorizedRoute) -> Option<u32> {
    match route.descriptor().blocking_backend {
        Some(BlockingBackend::OpenAiCompatible) => handles
            .api_client
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|client| client.config().max_tokens)),
        Some(BlockingBackend::OpenRouter) => handles
            .openrouter_client
            .lock()
            .ok()
            .and_then(|guard| guard.as_ref().map(|client| client.config().max_tokens)),
        Some(BlockingBackend::NativeLlama) | Some(BlockingBackend::MistralRs) | None => None,
    }
}

/// Mint the route, then stamp it with the per-endpoint-clamped completion
/// budget this dispatch will request (ADR-0038's "clamped at mint time"): the
/// one gate every content-bearing dispatch below goes through, so
/// `RouteRecord.completion_budget_clamped` can only ever be `true` when a
/// route's documented maximum actually constrained this request.
fn authorize_and_budget_route_dispatch(
    handles: &BackendHandles,
    provider: &LlmProvider,
) -> Result<AuthorizedRoute, AppError> {
    let route = authorize_route_dispatch(provider)?;
    Ok(match requested_completion_budget(handles, &route) {
        Some(requested) => route.with_completion_budget(requested),
        None => route,
    })
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
    let route = match authorize_and_budget_route_dispatch(handles, provider) {
        Ok(route) => route,
        Err(error) => {
            // Content-free: registry identity only (ADR-0033). `None` here means
            // the caller falls back to the LOCAL rule-based extractor, which is
            // not a provider substitution.
            log::warn!("LLM extraction route refused: {error}");
            return None;
        }
    };
    match route.descriptor().blocking_backend {
        Some(BlockingBackend::NativeLlama) => extract_native(handles, text, speaker),
        Some(BlockingBackend::OpenAiCompatible) => {
            extract_api(handles, &route, text, speaker, context)
        }
        Some(BlockingBackend::OpenRouter) => {
            extract_openrouter(handles, &route, text, speaker, context)
        }
        Some(BlockingBackend::MistralRs) => extract_mistralrs(handles, text, speaker),
        None => {
            log::warn!(
                "{} has no blocking route; extraction falls back to the local rule-based extractor",
                route.id()
            );
            None
        }
    }
}

fn run_chat(
    handles: &BackendHandles,
    provider: &LlmProvider,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<ChatOutcome, String> {
    let route = authorize_and_budget_route_dispatch(handles, provider)
        .map_err(|error| error.to_string())?;
    match route.descriptor().blocking_backend {
        Some(BlockingBackend::NativeLlama) => chat_native(handles, messages, graph_context),
        Some(BlockingBackend::OpenAiCompatible) => {
            chat_api(handles, &route, messages, graph_context)
        }
        Some(BlockingBackend::OpenRouter) => {
            chat_openrouter(handles, &route, messages, graph_context)
        }
        Some(BlockingBackend::MistralRs) => chat_mistralrs(handles, messages, graph_context),
        None => Err(no_blocking_route_error(&route)),
    }
}

/// The honest terminal error for a route with no blocking implementation.
///
/// `route.aws_bedrock` used to be dispatched at `projection_api` / `chat_api`
/// while `api_config_from_runtime_settings` returns `None` for it, so the user saw
/// "API LLM client is not configured" — a diagnostic about the wrong client.
fn no_blocking_route_error(route: &AuthorizedRoute) -> String {
    format!(
        "{} has no blocking route; this provider serves streaming chat only",
        route.id()
    )
}

/// One route attempt's content-free result.
#[derive(Debug)]
struct ProjectionBackendOutput {
    raw_json: String,
    /// The route id stamped by trusted code, refined from the LIVE client config.
    route_id: &'static str,
    /// The registry provider id the route was authorized against (ADR-0033).
    provider_id: &'static str,
    wire_skin: &'static str,
    /// The SERVED model when the response reported one, else the requested id —
    /// which of those it is, is recorded in `model_source`, never guessed.
    model: String,
    model_source: ModelIdentitySource,
    tokens_used: u32,
    wire: WireOutcome,
    completion_budget_clamped: bool,
}

impl ProjectionBackendOutput {
    fn route_record(&self) -> RouteRecord {
        RouteRecord {
            route_id: self.route_id.to_string(),
            provider_id: self.provider_id.to_string(),
            wire_skin: self.wire_skin.to_string(),
            terminal_status: self.wire.terminal_status,
            retry_class: retry_class_for_terminal_status(self.wire.terminal_status),
            constrained_decoding: self.wire.constrained_decoding,
            completion_budget_clamped: self.completion_budget_clamped,
            served_upstream_provider: self.wire.served_upstream_provider.clone(),
        }
    }
}

/// Per-call context for stable-prefix prompt caching (ADR-0025 §2d / seed
/// audio-graph-d77e) plus the projection kind. Only cache-capable providers
/// (OpenRouter → Anthropic passthrough) act on the cache hint, and only the
/// schema-constrained paths read `kind` (to pick the strict schema — seed
/// audio-graph-a324).
#[derive(Clone)]
struct ProjectionCacheContext {
    session_id: String,
    /// Index of the last stable-prefix message the `cache_control` breakpoint
    /// rides on (immutable system + append-only stable-context blocks).
    cache_breakpoint_message_index: usize,
    /// Which projection kind this job is, so the constrained paths can request
    /// the kind-scoped strict output schema.
    kind: ProjectionKind,
}

impl ProjectionCacheContext {
    /// A (session, route)-scoped hint. Keyed on the stamped route id so a summary
    /// or prefix computed for one vendor's tokenizer is never reused for another.
    fn hint_for(&self, route_id: &str) -> crate::llm::openrouter::PromptCacheHint {
        crate::llm::openrouter::PromptCacheHint {
            cache_breakpoint_message_index: self.cache_breakpoint_message_index,
            cache_key: format!("{}::{}", self.session_id, route_id),
        }
    }
}

fn run_projection_patch(
    handles: &BackendHandles,
    provider: &LlmProvider,
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
    sequence: u64,
    created_at_ms: u64,
) -> Result<ProjectionPatchOutcome, String> {
    let route = authorize_and_budget_route_dispatch(handles, provider)
        .map_err(|error| error.to_string())?;
    let messages = projection_patch_prompt_messages(job, ledger).map_err(|e| e.to_string())?;
    let cache_context = ProjectionCacheContext {
        session_id: job.session_id.clone(),
        cache_breakpoint_message_index:
            crate::projection_llm::PROJECTION_STABLE_PREFIX_MESSAGE_COUNT.saturating_sub(1),
        kind: job.kind.clone(),
    };

    run_projection_patch_on_route(
        |messages| projection_on_route(handles, &route, messages, &cache_context),
        &messages,
        job,
        ledger,
        sequence,
        created_at_ms,
    )
}

/// Dispatch a projection prompt on one authorized route.
fn projection_on_route(
    handles: &BackendHandles,
    route: &AuthorizedRoute,
    messages: &[ChatMessage],
    cache: &ProjectionCacheContext,
) -> Result<ProjectionBackendOutput, String> {
    match route.descriptor().blocking_backend {
        Some(BlockingBackend::NativeLlama) => projection_native(handles, route, messages),
        Some(BlockingBackend::OpenAiCompatible) => projection_api(handles, route, messages, cache),
        Some(BlockingBackend::OpenRouter) => projection_openrouter(handles, route, messages, cache),
        Some(BlockingBackend::MistralRs) => projection_mistralrs(handles, route, messages),
        None => Err(no_blocking_route_error(route)),
    }
}

/// Draft → terminal-status check → validate → **same-route** repair → validate.
///
/// `run` is invoked with the draft prompt and, if the draft fails validation, with
/// the repair prompt. It closes over ONE authorized route, so the repair
/// structurally cannot reach a second provider — that is the removal ADR-0038
/// requires, expressed as the absence of a second candidate rather than as a rule.
///
/// The honest cost: seed audio-graph-a324 measured 6/6 same-model repair
/// double-failures, so when the route reproduces the failure the patch fails with
/// `UnusableCompletion` and does not hop.
fn run_projection_patch_on_route(
    mut run: impl FnMut(&[ChatMessage]) -> Result<ProjectionBackendOutput, String>,
    messages: &[ChatMessage],
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
    sequence: u64,
    created_at_ms: u64,
) -> Result<ProjectionPatchOutcome, String> {
    let output = run(messages)?;
    ensure_usable_completion(&output)?;

    match projection_outcome_from_output(
        &output,
        job,
        sequence,
        created_at_ms,
        PROJECTION_PATCH_PROMPT_ID,
        None,
    ) {
        Ok(mut outcome) => {
            outcome.tokens_used = output.tokens_used;
            Ok(outcome)
        }
        Err(first_error) => {
            let repair_messages = projection_patch_repair_prompt_messages(
                job,
                ledger,
                &output.raw_json,
                &first_error,
            )
            .map_err(|e| e.to_string())?;
            let repair_output = run(&repair_messages)?;
            ensure_usable_completion(&repair_output)?;
            let mut outcome = projection_outcome_from_output(
                &repair_output,
                job,
                sequence,
                created_at_ms,
                PROJECTION_PATCH_REPAIR_PROMPT_ID,
                Some("repair"),
            )
            .map_err(|repair_error| {
                format!(
                    "projection patch draft invalid and repair failed: {first_error}; repair: {repair_error}"
                )
            })?;
            outcome.tokens_used = output.tokens_used.saturating_add(repair_output.tokens_used);
            Ok(outcome)
        }
    }
}

/// Reject a non-`Completed` terminal status **before** the JSON parse.
///
/// This is what stops ADR-0038 defect 1 from re-manifesting: a `Truncated`
/// response used to fail as invalid JSON and enter the repair path, which embeds
/// the truncated draft in a new prompt. Per ADR-0038 sub-decision 4 a truncation
/// must not auto-spend a larger-budget attempt either — a larger declared
/// `max_completion_tokens` also raises the Cerebras pre-generation rate-limit
/// charge (ADR-0038:219-225), so the "safe" retry makes a 429 more likely.
///
/// Where a truncated INCREMENTAL patch finally rests is genuinely unresolved:
/// ADR-0035's `Finalization Blocked` is per-Session post-stop and does not exist
/// in this runtime, and the stalled-lane behaviour is `audio-graph-3b48`'s. This
/// function ships the classification and the no-larger-budget guarantee; it does
/// not invent a resting state.
fn ensure_usable_completion(output: &ProjectionBackendOutput) -> Result<(), String> {
    if output.wire.terminal_status == TerminalStatus::Completed {
        return Ok(());
    }
    let record = output.route_record();
    Err(format!(
        "projection route attempt is not a usable completion: route={} terminal_status={} \
         retry_class={}",
        record.route_id,
        output.wire.terminal_status.as_str(),
        record
            .retry_class
            .map(|class| format!("{class:?}"))
            .unwrap_or_else(|| "none".to_string())
    ))
}

fn projection_outcome_from_output(
    output: &ProjectionBackendOutput,
    job: &ProjectionJob,
    sequence: u64,
    created_at_ms: u64,
    prompt_id: &str,
    request_suffix: Option<&str>,
) -> Result<ProjectionPatchOutcome, ProjectionPatchDraftError> {
    log::debug!(
        "Projection patch route output: route={}, provider={}, model_source={:?}, \
         constrained_decoding={:?}",
        output.route_id,
        output.provider_id,
        output.model_source,
        output.wire.constrained_decoding
    );
    let route_key = output.route_id.replace('.', "_");
    let llm_request_id = match request_suffix {
        Some(suffix) => format!("{}:{}:{}:{}", job.id, route_key, sequence, suffix),
        None => format!("{}:{}:{}", job.id, route_key, sequence),
    };
    let patch = trusted_projection_patch_from_model_json(
        &output.raw_json,
        job,
        ProjectionPatchBuildContext {
            sequence,
            llm_request_id,
            provider: output.provider_id.to_string(),
            model: output.model.clone(),
            model_source: output.model_source,
            route_id: Some(output.route_id.to_string()),
            route: Some(output.route_record()),
            prompt_id: prompt_id.to_string(),
            created_at_ms,
        },
    )?;
    Ok(ProjectionPatchOutcome {
        patch,
        tokens_used: output.tokens_used,
    })
}

// ---------------------------------------------------------------------------
// Per-backend route attempts
// ---------------------------------------------------------------------------

/// Re-resolve and stamp the route from the LIVE `ApiClient` config.
///
/// `AuthorizedRoute` was minted from the job's `LlmProvider` **snapshot**, but the
/// client handle is the shared one that `sync_llm_api_client_from_settings_cache`
/// rebuilds on every settings save. Resolving again here is what makes the stamp
/// true and the gate current.
fn api_route_for_client(
    route: &AuthorizedRoute,
    client: &ApiClient,
) -> Result<&'static crate::llm::route::RouteDescriptor, String> {
    route.refine_within_authorization(route_for_api_endpoint(&client.config().endpoint))
}

/// Re-resolve and stamp the route from the LIVE `OpenRouterClient` config. This is
/// where `route.openrouter` sharpens into `route.cerebras_via_openrouter`: the
/// routing policy reaches this layer only through `OpenRouterConfig`, and both
/// routes gate on `llm.openrouter`, so the refinement cannot widen egress.
///
/// `pub(crate)` (not private) so `openrouter::tests::live_openrouter_routed_smoke`
/// can dispatch through the EXACT function production traffic uses
/// (audio-graph-8772, Wave 4) instead of re-deriving an equivalent call from
/// `route_for_openrouter_policy` alone — the live smoke exercises the same
/// `AuthorizedRoute` + refinement gate `executor.rs` itself runs, not a
/// parallel approximation of it.
pub(crate) fn openrouter_route_for_client(
    route: &AuthorizedRoute,
    client: &OpenRouterClient,
) -> Result<&'static crate::llm::route::RouteDescriptor, String> {
    let config = client.config();
    route.refine_within_authorization(route_for_openrouter_policy(
        config.routing_policy.as_ref(),
        config.provider_order.as_deref(),
    ))
}

fn projection_api(
    handles: &BackendHandles,
    route: &AuthorizedRoute,
    messages: &[ChatMessage],
    cache: &ProjectionCacheContext,
) -> Result<ProjectionBackendOutput, String> {
    let client = {
        let guard = handles.api_client.lock().map_err(|e| e.to_string())?;
        guard
            .as_ref()
            .ok_or_else(|| "API LLM client is not configured".to_string())?
            .clone()
    };
    let live = api_route_for_client(route, &client)?;
    let requested_model = client.config().model.clone();

    // Route-driven, not host-substring-driven: an endpoint whose row declares
    // `GuaranteedConstrained` (direct Cerebras) requests its documented strict
    // schema. `prefers_vllm_structured_outputs()` stays the ONE endpoint
    // heuristic, and it selects only the vLLM-specific body field.
    let constrained_form = match live.capability.constrained_decoding {
        ConstrainedDecodingGrade::GuaranteedConstrained => {
            Some(RequestOutputForm::StrictJsonSchema {
                name: "projection_patch".to_string(),
                schema: projection_patch_strict_json_schema(&cache.kind),
            })
        }
        _ if client.prefers_vllm_structured_outputs() => {
            Some(RequestOutputForm::VllmStructuredOutputs {
                schema: projection_patch_draft_json_schema()?,
            })
        }
        _ => None,
    };

    let (raw_json, tokens_used, wire) = match constrained_form {
        Some(form) => match client.chat_completion_with_wire_outcome(prompt_tuples(messages), form)
        {
            Ok(result) => result,
            Err(error) if is_schema_rejection(&error) => {
                // A MODE downgrade on the SAME route, recorded rather than only
                // logged: the returned `WireOutcome` carries `Unconstrained`.
                log::warn!(
                    "constrained projection output rejected on {} (falling back to JSON mode on \
                     the same route): {error}",
                    live.id
                );
                client.chat_completion_with_wire_outcome(
                    prompt_tuples(messages),
                    RequestOutputForm::JsonObject,
                )?
            }
            Err(error) => return Err(error),
        },
        None => client.chat_completion_with_wire_outcome(
            prompt_tuples(messages),
            RequestOutputForm::JsonObject,
        )?,
    };

    Ok(projection_output_from_wire(
        route,
        live,
        raw_json,
        requested_model,
        tokens_used,
        wire,
    ))
}

fn projection_openrouter(
    handles: &BackendHandles,
    route: &AuthorizedRoute,
    messages: &[ChatMessage],
    cache: &ProjectionCacheContext,
) -> Result<ProjectionBackendOutput, String> {
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
    let live = openrouter_route_for_client(route, &client)?;
    let requested_model = client.config().model.clone();
    // Stable-prefix prompt caching (ADR-0025 §2d / seed audio-graph-d77e): mark
    // the cache breakpoint on the stable prefix and route this session's turns to
    // the same cache-warm machine via a (session, route) key. The request output
    // form is orthogonal to the cache hint, so both paths below route identically.
    let hint = cache.hint_for(live.id);

    // Prefer OpenRouter structured outputs (with provider.require_parameters=true
    // so routing only picks a schema-capable provider) so the model is constrained
    // to the projection-patch schema at generation time (seed audio-graph-a324).
    //
    // On a schema rejection (400/404/422-class) we downgrade to JSON mode for THIS
    // call, on the SAME route. We only cache the model as schema-unsupported on a
    // model-level signal (404 = require_parameters found no provider for the model
    // that supports the schema); a 400/422 came from a routed-to provider that DID
    // advertise support and is schema-specific, so caching it would wrongly demote
    // a schema-capable model to JSON mode for the whole session (Codex P2).
    if !openrouter_schema_unsupported(&requested_model) {
        let form = RequestOutputForm::StrictJsonSchema {
            name: "projection_patch".to_string(),
            schema: projection_patch_strict_json_schema(&cache.kind),
        };
        match client.chat_completion_with_wire_outcome(
            prompt_tuples(messages),
            &form,
            Some(hint.clone()),
        ) {
            Ok((raw_json, tokens_used, wire)) => {
                return Ok(projection_output_from_wire(
                    route,
                    live,
                    raw_json,
                    requested_model,
                    tokens_used,
                    wire,
                ));
            }
            Err(e) if is_schema_rejection(&e) => {
                if is_openrouter_schema_unsupported_by_model(&e) {
                    log::warn!(
                        "OpenRouter structured projection output: no schema-capable provider for \
                         model (caching + falling back to JSON mode for the session): {e}"
                    );
                    note_openrouter_schema_unsupported(&requested_model);
                } else {
                    log::warn!(
                        "OpenRouter structured projection output rejected for this call (falling \
                         back to JSON mode; not caching — a schema-capable provider may serve the \
                         next call): {e}"
                    );
                }
                // fall through to JSON mode below, on the SAME route
            }
            Err(e) => return Err(e),
        }
    }

    let (raw_json, tokens_used, wire) = client.chat_completion_with_wire_outcome(
        prompt_tuples(messages),
        &RequestOutputForm::JsonObject,
        Some(hint),
    )?;
    Ok(projection_output_from_wire(
        route,
        live,
        raw_json,
        requested_model,
        tokens_used,
        wire,
    ))
}

/// Assemble the route-stamped output. The route id and provider id come from
/// trusted code; `model` is the SERVED id when the response reported one, and
/// `model_source` records which it is so no reader can mistake a config echo for
/// served identity (ADR-0038 defect 3).
fn projection_output_from_wire(
    route: &AuthorizedRoute,
    live: &'static crate::llm::route::RouteDescriptor,
    raw_json: String,
    requested_model: String,
    tokens_used: u32,
    wire: WireOutcome,
) -> ProjectionBackendOutput {
    let (model, model_source) = match wire.served_model.clone() {
        Some(served) => (served, ModelIdentitySource::Served),
        None => (requested_model, ModelIdentitySource::Requested),
    };
    ProjectionBackendOutput {
        raw_json,
        route_id: live.id,
        provider_id: live.provider_id,
        wire_skin: route.skin().as_str(),
        model,
        model_source,
        tokens_used,
        wire,
        completion_budget_clamped: route.completion_budget_clamped(),
    }
}

fn projection_native(
    handles: &BackendHandles,
    route: &AuthorizedRoute,
    messages: &[ChatMessage],
) -> Result<ProjectionBackendOutput, String> {
    let guard = handles.llm_engine.lock().map_err(|e| e.to_string())?;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Native LLM is not loaded".to_string())?;
    let outcome = engine.chat(messages, "")?;
    Ok(ProjectionBackendOutput {
        raw_json: outcome.text,
        route_id: route.id(),
        provider_id: route.provider_id(),
        wire_skin: route.skin().as_str(),
        // The in-process engine has no wire, so there is nothing to echo: the
        // loaded model IS the requested one, recorded as such.
        model: "loaded_local_llama".to_string(),
        model_source: ModelIdentitySource::Requested,
        tokens_used: outcome.tokens_used,
        wire: WireOutcome::plain(
            TerminalStatus::Completed,
            ConstrainedDecodingGrade::Unconstrained,
        ),
        completion_budget_clamped: route.completion_budget_clamped(),
    })
}

fn projection_mistralrs(
    handles: &BackendHandles,
    route: &AuthorizedRoute,
    messages: &[ChatMessage],
) -> Result<ProjectionBackendOutput, String> {
    let guard = handles.mistralrs_engine.lock().map_err(|e| e.to_string())?;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "mistral.rs LLM is not loaded".to_string())?;
    let (raw_json, tokens_used, grade) = match engine.projection_patch_draft_with_usage(messages) {
        Ok((raw_json, tokens_used)) => (
            raw_json,
            tokens_used,
            ConstrainedDecodingGrade::AdvertisedHint,
        ),
        Err(e) => {
            // A MODE downgrade inside one local engine, recorded in the route
            // record rather than only logged.
            log::warn!(
                "mistral.rs structured projection output failed, falling back to chat JSON mode: {e}"
            );
            let (raw_json, tokens_used) = engine.chat_with_history_usage(messages, "")?;
            (
                raw_json,
                tokens_used,
                ConstrainedDecodingGrade::Unconstrained,
            )
        }
    };
    Ok(ProjectionBackendOutput {
        raw_json,
        route_id: route.id(),
        provider_id: route.provider_id(),
        wire_skin: route.skin().as_str(),
        model: "loaded_mistralrs".to_string(),
        model_source: ModelIdentitySource::Requested,
        tokens_used,
        wire: WireOutcome::plain(TerminalStatus::Completed, grade),
        completion_budget_clamped: route.completion_budget_clamped(),
    })
}

fn prompt_tuples(messages: &[ChatMessage]) -> Vec<(String, String)> {
    messages
        .iter()
        .map(|message| (message.role.clone(), message.content.clone()))
        .collect()
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
    route: &AuthorizedRoute,
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
    if let Err(error) = api_route_for_client(route, &client) {
        log::warn!("LLM extraction route refused: {error}");
        return None;
    }
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
    route: &AuthorizedRoute,
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
    if let Err(error) = openrouter_route_for_client(route, &client) {
        log::warn!("LLM extraction route refused: {error}");
        return None;
    }
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
    route: &AuthorizedRoute,
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
    let live = api_route_for_client(route, &client)?;
    // `ApiClient::chat_with_history_with_usage` (and the Bedrock requests routed
    // through it) surfaces the OpenAI `usage.total_tokens` from the response.
    // A provider that omits the `usage` block reports 0 — never fabricated
    // (FA-7c).
    let (text, tokens_used, wire) =
        client.chat_with_history_with_wire_outcome(messages, graph_context)?;
    log_non_completed_chat_status(live.id, &wire);
    Ok(ChatOutcome { text, tokens_used })
}

fn chat_openrouter(
    handles: &BackendHandles,
    route: &AuthorizedRoute,
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
    let live = openrouter_route_for_client(route, &client)?;
    // OpenRouter is OpenAI-compatible: the non-streaming response carries
    // `usage.total_tokens`. It is 0 only when the upstream provider omits the
    // usage block — never fabricated (FA-7c).
    let (text, tokens_used, wire) =
        client.chat_with_history_with_wire_outcome(messages, graph_context)?;
    log_non_completed_chat_status(live.id, &wire);
    Ok(ChatOutcome { text, tokens_used })
}

/// Record a non-`Completed` terminal status on the interactive chat path.
///
/// Scope statement, because the code cannot show it: interactive chat REPORTS the
/// normalized terminal status and still returns the partial reply. Acting on a
/// truncated chat reply changes the frontend terminal-frame contract, which is not
/// this contract's surface — the projection path is where an unread stop signal
/// caused the cross-provider escalation ADR-0038 removes.
fn log_non_completed_chat_status(route_id: &str, wire: &WireOutcome) {
    if wire.terminal_status != TerminalStatus::Completed {
        log::warn!(
            "chat reply on {route_id} ended with terminal_status={} — the reply may be incomplete",
            wire.terminal_status.as_str()
        );
    }
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
    // `MistralRsEngine::chat_with_history_usage` surfaces the real token count
    // from mistral.rs's `ChatCompletionResponse.usage.total_tokens` (FA-7c).
    engine
        .chat_with_history_usage(messages, graph_context)
        .map(|(text, tokens_used)| ChatOutcome { text, tokens_used })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projections::{
        ProjectionKind, ProjectionOperation, ProjectionPriority, TranscriptEvent,
        TranscriptEventStability,
    };
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

    // ----- test fixtures ----------------------------------------------------

    fn projection_test_event(span_id: &str, text: &str) -> TranscriptEvent {
        TranscriptEvent {
            span_id: span_id.to_string(),
            provider: "test".to_string(),
            source_id: "source-1".to_string(),
            provider_item_id: Some(span_id.to_string()),
            transcript_segment_id: None,
            speaker_id: Some("speaker-1".to_string()),
            speaker_label: Some("Speaker 1".to_string()),
            channel: None,
            text: text.to_string(),
            start_time: 1.0,
            end_time: 2.0,
            confidence: 0.9,
            is_final: true,
            stability: TranscriptEventStability::Final,
            revision_number: 1,
            supersedes: None,
            turn_id: None,
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000,
        }
    }

    fn projection_test_job(kind: ProjectionKind) -> (ProjectionJob, TranscriptLedger) {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(projection_test_event("span-1", "Alice met Bob."))
            .expect("seed transcript ledger");
        let job = ProjectionJob {
            id: "projection:session-1:notes:1".to_string(),
            session_id: "session-1".to_string(),
            kind,
            basis: ledger.current_basis(),
            priority: ProjectionPriority::Realtime,
            queued_at_ms: 10,
        };
        (job, ledger)
    }

    /// A completed route output stamped with a fixed test route id. The recorder
    /// tests below assert on `route_id`, which is how "the repair never left the
    /// authorized route" is observable at all.
    fn projection_output(raw_json: String, tokens_used: u32) -> ProjectionBackendOutput {
        route_projection_output("route.openrouter", raw_json, tokens_used)
    }

    fn route_projection_output(
        route_id: &'static str,
        raw_json: String,
        tokens_used: u32,
    ) -> ProjectionBackendOutput {
        ProjectionBackendOutput {
            raw_json,
            route_id,
            provider_id: "llm.openrouter",
            wire_skin: "chat_completions",
            model: "test-model".to_string(),
            model_source: ModelIdentitySource::Served,
            tokens_used,
            wire: WireOutcome::plain(
                TerminalStatus::Completed,
                ConstrainedDecodingGrade::AdvertisedHint,
            ),
            completion_budget_clamped: false,
        }
    }

    fn graded_projection_output(
        raw_json: String,
        tokens_used: u32,
        grade: ConstrainedDecodingGrade,
    ) -> ProjectionBackendOutput {
        let mut output = projection_output(raw_json, tokens_used);
        output.wire.constrained_decoding = grade;
        output
    }

    // ----- token usage flows through the single-route dispatch --------------

    #[test]
    fn route_dispatch_preserves_chat_token_count() {
        // The real seam the blocking chat path uses: `run_chat` resolves one
        // route, dispatches through `chat_api`, and returns that backend's
        // `ChatOutcome` unchanged. Drives the REAL wire path against a mock
        // endpoint reporting `usage.total_tokens`, so a regression that
        // discards the tuple element from `chat_with_history_with_wire_outcome`
        // (e.g. `chat_api`/`chat_openrouter` returning `tokens_used: 0`) would
        // fail this test — a self-asserting `ChatOutcome` literal could not.
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{
                "message": { "content": "hi there" },
                "finish_reason": "stop"
            }],
            "usage": { "total_tokens": 77 }
        })
        .to_string();
        let (base, request_count) = rt.block_on(spawn_counting_mock(vec![body]));

        let handles = handles_with_only_api_client(&base);
        let provider = LlmProvider::Api {
            endpoint: base.clone(),
            api_key: "sk-route-removal-probe".to_string(),
            model: "probe-model".to_string(),
        };
        let outcome =
            std::thread::spawn(move || run_chat(&handles, &provider, &[], "graph context"))
                .join()
                .expect("worker thread panic")
                .expect("a completed chat reply");

        assert_eq!(
            request_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "exactly one request reaches the wire"
        );
        assert_eq!(outcome.text, "hi there");
        assert_eq!(
            outcome.tokens_used, 77,
            "the backend-reported usage.total_tokens must reach the caller unchanged"
        );

        let route =
            authorize_route_dispatch(&LlmProvider::LocalLlama).expect("local_llama is selectable");
        assert_eq!(route.id(), "route.local_llama");
    }

    // ----- repair: same route, one route only -------------------------------

    #[test]
    fn projection_patch_retries_once_with_repair_prompt() {
        let (job, ledger) = projection_test_job(ProjectionKind::Notes);
        let call_count = Arc::new(Mutex::new(0usize));
        let seen_messages = Arc::new(Mutex::new(Vec::<Vec<ChatMessage>>::new()));
        let invalid_first = serde_json::json!({
            "operations": [{
                "type": "upsert_graph_node",
                "id": "person:alice",
                "name": "Alice",
                "entity_type": "person",
                "description": null
            }]
        })
        .to_string();
        let repaired = serde_json::json!({
            "operations": [{
                "type": "upsert_note",
                "id": "note:alice-bob",
                "title": "Alice and Bob",
                "body": "Alice met Bob.",
                "tags": ["people"]
            }],
            "confidence": 0.8
        })
        .to_string();

        let outcome = run_projection_patch_on_route(
            {
                let call_count = call_count.clone();
                let seen_messages = seen_messages.clone();
                move |messages| {
                    let mut count = call_count.lock().unwrap_or_else(|e| e.into_inner());
                    *count += 1;
                    seen_messages
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(messages.to_vec());
                    if *count == 1 {
                        Ok(projection_output(invalid_first.clone(), 11))
                    } else {
                        Ok(projection_output(repaired.clone(), 13))
                    }
                }
            },
            &projection_patch_prompt_messages(&job, &ledger).expect("initial prompt"),
            &job,
            &ledger,
            4,
            123,
        )
        .expect("repaired projection patch");

        assert_eq!(*call_count.lock().unwrap_or_else(|e| e.into_inner()), 2);
        assert_eq!(outcome.tokens_used, 24);
        assert_eq!(
            outcome.patch.provenance.prompt_id,
            PROJECTION_PATCH_REPAIR_PROMPT_ID
        );
        // The request id is keyed on the stamped ROUTE id, not on a fourth ad-hoc
        // provider naming scheme.
        assert_eq!(
            outcome.patch.llm_request_id,
            "projection:session-1:notes:1:route_openrouter:4:repair"
        );
        assert_eq!(
            outcome.patch.provenance.route_id.as_deref(),
            Some("route.openrouter")
        );
        assert_eq!(outcome.patch.provenance.provider, "llm.openrouter");
        assert!(matches!(
            outcome.patch.operations.first(),
            Some(ProjectionOperation::UpsertNote { id, .. }) if id == "note:alice-bob"
        ));

        let seen = seen_messages.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(seen.len(), 2);
        let repair_instruction = seen[1].last().expect("repair instruction");
        assert!(repair_instruction.content.contains("validation_error:"));
        assert!(repair_instruction.content.contains("upsert_graph_node"));
    }

    #[test]
    fn projection_patch_fails_after_one_repair_attempt() {
        let (job, ledger) = projection_test_job(ProjectionKind::Notes);
        let call_count = Arc::new(Mutex::new(0usize));
        let invalid = serde_json::json!({
            "operations": [{
                "type": "upsert_graph_node",
                "id": "person:alice",
                "name": "Alice",
                "entity_type": "person",
                "description": null
            }]
        })
        .to_string();

        let err = run_projection_patch_on_route(
            {
                let call_count = call_count.clone();
                move |_messages| {
                    let mut count = call_count.lock().unwrap_or_else(|e| e.into_inner());
                    *count += 1;
                    Ok(projection_output(invalid.clone(), 3))
                }
            },
            &projection_patch_prompt_messages(&job, &ledger).expect("initial prompt"),
            &job,
            &ledger,
            4,
            123,
        )
        .expect_err("repair remains invalid");

        assert_eq!(*call_count.lock().unwrap_or_else(|e| e.into_inner()), 2);
        assert!(err.contains("projection patch draft invalid and repair failed"));
        assert!(err.contains("upsert_graph_node"));
    }

    #[test]
    fn schema_constrained_projection_output_still_uses_repair_fallback() {
        let (job, ledger) = projection_test_job(ProjectionKind::Graph);
        let call_count = Arc::new(Mutex::new(0usize));
        let invalid_first = serde_json::json!({
            "operations": [{
                "type": "upsert_note",
                "id": "note:wrong-kind",
                "title": "Wrong kind",
                "body": "Schema-valid JSON can still be semantically wrong."
            }]
        })
        .to_string();
        let repaired = serde_json::json!({
            "operations": [{
                "type": "upsert_graph_node",
                "id": "person:alice",
                "name": "Alice",
                "entity_type": "person",
                "description": "Met Bob."
            }],
            "confidence": 0.9
        })
        .to_string();

        let outcome = run_projection_patch_on_route(
            {
                let call_count = call_count.clone();
                move |_messages| {
                    let mut count = call_count.lock().unwrap_or_else(|e| e.into_inner());
                    *count += 1;
                    if *count == 1 {
                        Ok(graded_projection_output(
                            invalid_first.clone(),
                            7,
                            ConstrainedDecodingGrade::GuaranteedConstrained,
                        ))
                    } else {
                        Ok(graded_projection_output(
                            repaired.clone(),
                            8,
                            ConstrainedDecodingGrade::GuaranteedConstrained,
                        ))
                    }
                }
            },
            &projection_patch_prompt_messages(&job, &ledger).expect("initial prompt"),
            &job,
            &ledger,
            9,
            456,
        )
        .expect("repaired schema-constrained output");

        assert_eq!(*call_count.lock().unwrap_or_else(|e| e.into_inner()), 2);
        assert_eq!(outcome.tokens_used, 15);
        assert_eq!(
            outcome.patch.provenance.prompt_id, PROJECTION_PATCH_REPAIR_PROMPT_ID,
            "schema-constrained JSON still goes through semantic validation and repair"
        );
        // ADR-0038 sub-decision 5: the grade is RECORDED, and it never gated
        // dispatch — a `GuaranteedConstrained` route still had to be validated.
        let record = outcome.patch.route.as_ref().expect("route record");
        assert_eq!(
            record.constrained_decoding,
            ConstrainedDecodingGrade::GuaranteedConstrained
        );
        assert_eq!(record.terminal_status, TerminalStatus::Completed);
        assert_eq!(record.retry_class, None);
        assert!(matches!(
            outcome.patch.operations.first(),
            Some(ProjectionOperation::UpsertGraphNode { id, .. }) if id == "person:alice"
        ));
    }

    // ----- ADR-0038 defect (d): no dispatch without an authorized route -----

    #[test]
    fn dispatch_requires_an_authorized_route_token() {
        use crate::llm::route::{BlockingBackend, authorize_route_dispatch};
        let authorized = authorize_route_dispatch(&LlmProvider::LocalLlama)
            .expect("llm.local_llama is MVP-selectable");
        assert_eq!(authorized.descriptor().id, "route.local_llama");
        assert!(matches!(
            authorized.descriptor().blocking_backend,
            Some(BlockingBackend::NativeLlama)
        ));
    }

    // ----- ADR-0038 defect (a): Truncated never enters the repair path -------

    #[test]
    fn truncated_draft_never_reaches_repair_and_never_raises_the_budget() {
        let (job, ledger) = projection_test_job(ProjectionKind::Notes);
        let calls = Arc::new(Mutex::new(0usize));
        let err = run_projection_patch_on_route(
            {
                let calls = calls.clone();
                move |_messages| {
                    *calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                    let mut output = projection_output("{\"operations\":[".to_string(), 40_960);
                    output.wire.terminal_status = TerminalStatus::Truncated;
                    Ok(output)
                }
            },
            &projection_patch_prompt_messages(&job, &ledger).expect("initial prompt"),
            &job,
            &ledger,
            4,
            123,
        )
        .expect_err("a truncated draft is not a usable completion");

        assert_eq!(
            *calls.lock().unwrap_or_else(|e| e.into_inner()),
            1,
            "Truncated short-circuits before the parse: no repair, no second call"
        );
        assert!(
            err.contains("Truncated"),
            "the error must name the normalized terminal status, got: {err}"
        );
        assert!(
            err.contains("UnusableCompletion"),
            "and its retry class, got: {err}"
        );
    }

    #[test]
    fn refused_draft_never_reaches_repair() {
        let (job, ledger) = projection_test_job(ProjectionKind::Notes);
        let calls = Arc::new(Mutex::new(0usize));
        let err = run_projection_patch_on_route(
            {
                let calls = calls.clone();
                move |_messages| {
                    *calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                    let mut output = projection_output("{}".to_string(), 3);
                    output.wire.terminal_status = TerminalStatus::Refused;
                    Ok(output)
                }
            },
            &projection_patch_prompt_messages(&job, &ledger).expect("initial prompt"),
            &job,
            &ledger,
            4,
            123,
        )
        .expect_err("a refusal is not a usable completion");
        assert_eq!(*calls.lock().unwrap_or_else(|e| e.into_inner()), 1);
        assert!(err.contains("Refused"), "got: {err}");
    }

    // ----- ADR-0038: exactly one route, no cross-provider hop ---------------

    #[test]
    fn projection_dispatch_invokes_exactly_one_route_and_surfaces_its_error() {
        // ADR-0038: automatic cross-provider fallback is removed. A projection
        // dispatch invokes exactly ONE authorized route; when that route fails,
        // its own error surfaces verbatim and no second provider is touched.
        //
        // This is the inversion of the deleted `run_projection_attempts` walker
        // (whose job was to pick which of four backends' errors to surface) and
        // the promotion of its "No projection LLM backend configured" default:
        // with one route there is no chain, so the route's real error is the only
        // error there is.
        let calls = Arc::new(Mutex::new(0usize));
        let err = run_projection_patch_on_route(
            {
                let calls = calls.clone();
                move |_messages| {
                    *calls.lock().unwrap_or_else(|e| e.into_inner()) += 1;
                    Err("OpenRouter client is not configured".to_string())
                }
            },
            &[],
            &projection_test_job(ProjectionKind::Notes).0,
            &projection_test_job(ProjectionKind::Notes).1,
            4,
            123,
        )
        .expect_err("the single authorized route fails");

        assert_eq!(
            *calls.lock().unwrap_or_else(|e| e.into_inner()),
            1,
            "exactly one route must be invoked — no fallback hop"
        );
        assert_eq!(
            err, "OpenRouter client is not configured",
            "the authorized route's own error must surface verbatim"
        );
        assert!(
            !err.contains("mistral.rs LLM is not loaded")
                && !err.contains("No projection LLM backend configured"),
            "no downstream boilerplate and no generic chain default, got: {err}"
        );
    }

    #[test]
    fn repair_never_leaves_the_authorized_route() {
        // ADR-0038 / seed audio-graph-3624: the repair prompt embeds the invalid
        // draft plus transcript-derived context. It must re-run on the SAME
        // authorized route; escalating it to the next provider is unauthorized
        // egress caused purely by a validator rejection.
        //
        // This is the direct inversion of the deleted
        // `repair_escalates_to_next_backend_when_available`, whose
        // `vec!["backend-a", "backend-b"]` fixture asserted exactly what ADR-0038
        // forbids.
        let (job, ledger) = projection_test_job(ProjectionKind::Notes);
        let route_ids = Arc::new(Mutex::new(Vec::<&'static str>::new()));
        let invalid_first = serde_json::json!({
            "operations": [{
                "type": "upsert_graph_node",
                "id": "person:alice",
                "name": "Alice",
                "entity_type": "person",
                "description": null
            }]
        })
        .to_string();
        let repaired = serde_json::json!({
            "operations": [{
                "type": "upsert_note",
                "id": "note:alice-bob",
                "title": "Alice and Bob",
                "body": "Alice met Bob.",
                "tags": ["people"]
            }],
            "confidence": 0.8
        })
        .to_string();

        let outcome = run_projection_patch_on_route(
            {
                let route_ids = route_ids.clone();
                let mut seen = 0usize;
                move |_messages| {
                    seen += 1;
                    let output = if seen == 1 {
                        route_projection_output("route.openrouter", invalid_first.clone(), 5)
                    } else {
                        route_projection_output("route.openrouter", repaired.clone(), 7)
                    };
                    route_ids
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(output.route_id);
                    Ok(output)
                }
            },
            &projection_patch_prompt_messages(&job, &ledger).expect("initial prompt"),
            &job,
            &ledger,
            4,
            123,
        )
        .expect("same-route repair succeeds");

        assert_eq!(
            *route_ids.lock().unwrap_or_else(|e| e.into_inner()),
            vec!["route.openrouter", "route.openrouter"],
            "the repair must stay on the route that produced the draft"
        );
        assert_eq!(
            outcome.patch.provenance.prompt_id,
            PROJECTION_PATCH_REPAIR_PROMPT_ID
        );
        assert_eq!(
            outcome.patch.provenance.route_id.as_deref(),
            Some("route.openrouter")
        );
    }

    #[test]
    fn same_route_repair_error_surfaces_and_no_second_route_appears() {
        // The a324 finding is real: a same-route repair can reproduce the failure
        // (6/6 same-model repair double-failures in user data). When it does, the
        // route's own repair error surfaces and the patch fails — it does NOT hop.
        // This replaces
        // `repair_escalation_surfaces_producing_backend_error_not_next_backend_boilerplate`,
        // whose masking concern cannot arise once there is only one candidate.
        let (job, ledger) = projection_test_job(ProjectionKind::Notes);
        let invalid_first = serde_json::json!({
            "operations": [{
                "type": "upsert_graph_node",
                "id": "person:alice",
                "name": "Alice",
                "entity_type": "person",
                "description": null
            }]
        })
        .to_string();
        let mut seen = 0usize;

        let err = run_projection_patch_on_route(
            move |_messages| {
                seen += 1;
                if seen == 1 {
                    Ok(projection_output(invalid_first.clone(), 5))
                } else {
                    Err("OpenRouter HTTP error: status=502 (repair failed)".to_string())
                }
            },
            &projection_patch_prompt_messages(&job, &ledger).expect("initial prompt"),
            &job,
            &ledger,
            4,
            123,
        )
        .expect_err("the repair fails on the same route");

        assert!(
            err.contains("status=502 (repair failed)"),
            "the authorized route's own repair error must surface, got: {err}"
        );
        assert!(
            !err.contains("mistral.rs LLM is not loaded"),
            "there is no next backend whose boilerplate could mask it, got: {err}"
        );
    }

    // ----- a324: OpenRouter structured-output rejection classification ------

    #[test]
    fn schema_rejection_matches_4xx_class_only() {
        // A model that lacks structured-output support returns a 400/404/422;
        // those trigger the schema-less retry.
        assert!(is_schema_rejection(
            "OpenRouter HTTP error: provider=openrouter path=/chat/completions status=400 body_bytes=12"
        ));
        assert!(is_schema_rejection("... status=404 ..."));
        assert!(is_schema_rejection("... status=422 ..."));
        // Auth and transient failures must NOT be treated as schema rejections —
        // a schema-less retry would fail the same way (401/403) or should have
        // been retried in-client (429/5xx).
        assert!(!is_schema_rejection("... status=401 ..."));
        assert!(!is_schema_rejection("... status=403 ..."));
        assert!(!is_schema_rejection("... status=429 ..."));
        assert!(!is_schema_rejection("... status=502 ..."));
        assert!(!is_schema_rejection(
            "OpenRouter chat completion request failed: timed out"
        ));
    }

    #[test]
    fn only_model_level_404_is_cacheable_as_schema_unsupported() {
        // With require_parameters=true, a 404 (no provider for the model supports
        // the schema) is model-level → cache-worthy. A 400/422 came from a
        // routed-to provider that advertised support and is schema-specific → NOT
        // model-level, must not demote the whole model for the session (Codex P2).
        assert!(is_openrouter_schema_unsupported_by_model(
            "OpenRouter HTTP error: provider=openrouter path=/chat/completions status=404 body_bytes=30"
        ));
        assert!(
            !is_openrouter_schema_unsupported_by_model("... status=400 ..."),
            "a 400 is a per-provider validation quirk, not model-level evidence"
        );
        assert!(
            !is_openrouter_schema_unsupported_by_model("... status=422 ..."),
            "a 422 is a per-provider validation quirk, not model-level evidence"
        );
        // Sanity: every model-level signal is also a (broader) schema rejection.
        let no_providers =
            "OpenRouter HTTP error: provider=openrouter path=/chat/completions status=404";
        assert!(is_schema_rejection(no_providers));
        assert!(is_openrouter_schema_unsupported_by_model(no_providers));
    }

    #[test]
    fn openrouter_schema_unsupported_cache_round_trips() {
        let model = "test/schema-cache-probe-model";
        assert!(
            !openrouter_schema_unsupported(model),
            "a fresh model is assumed schema-capable"
        );
        note_openrouter_schema_unsupported(model);
        assert!(
            openrouter_schema_unsupported(model),
            "once recorded, the model is skipped for structured outputs this session"
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
            LlmJob::ProjectionPatch { job, .. } => job.id.clone(),
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

    // ----- one route, no backends: the route's own error, nothing else -------

    #[test]
    fn run_chat_on_a_route_with_no_backend_reports_only_that_route_error() {
        let handles = empty_handles();
        // LocalLlama resolves to `route.local_llama` and nothing else, so the only
        // error that can surface is the native engine's.
        let err =
            run_chat(&handles, &LlmProvider::LocalLlama, &[], "ctx").expect_err("no backend → Err");
        assert_eq!(err, "Native LLM is not loaded");
        assert!(
            !err.contains("mistral.rs") && !err.contains("OpenRouter") && !err.contains("API LLM"),
            "a single-route dispatch must not mention any other provider, got: {err}"
        );
    }

    #[test]
    fn run_chat_on_the_openrouter_route_never_falls_back_to_a_local_engine() {
        // This replaces `run_chat_restricted_policy_omits_cloud_attempts`, whose
        // premise was the deleted privacy-boolean local-only chain. A cloud route
        // under a non-ByokCloud privacy mode is now refused by the CLIENT's
        // content-egress policy (`ApiClient::new` / `OpenRouterClient::new` default
        // to `block("explicit_policy_required")`), not silently downgraded to a
        // local model — so what must be proven here is that no local engine is
        // reached at all.
        let handles = empty_handles();
        let err = run_chat(
            &handles,
            &LlmProvider::OpenRouter {
                model: "openai/gpt-5.2".into(),
                base_url: crate::llm::openrouter::DEFAULT_BASE_URL.into(),
                provider_order: None,
                include_usage_in_stream: true,
                api_key: String::new(),
            },
            &[],
            "ctx",
        )
        .expect_err("no OpenRouter client → Err");
        assert_eq!(err, "OpenRouter client is not configured");
        assert!(
            !err.contains("mistral.rs") && !err.contains("Native LLM"),
            "the OpenRouter route must never reach a local engine, got: {err}"
        );
    }

    #[test]
    fn bedrock_route_reports_no_blocking_route_not_a_missing_api_client() {
        // Bedrock used to be dispatched at `chat_api`, so the user saw "API LLM
        // client is not configured" — a diagnostic about the wrong client.
        let handles = empty_handles();
        let err = run_chat(
            &handles,
            &LlmProvider::AwsBedrock {
                region: "us-east-1".into(),
                model_id: "anthropic.claude-3-5-sonnet-20241022-v2:0".into(),
                credential_source: Default::default(),
            },
            &[],
            "ctx",
        )
        .expect_err("bedrock has no blocking route");
        assert!(err.contains("route.aws_bedrock"), "got: {err}");
        assert!(err.contains("streaming chat only"), "got: {err}");
        assert!(
            !err.contains("API LLM client is not configured"),
            "the old misleading diagnostic must be gone, got: {err}"
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
            "with no backend on the authorized route, extraction yields None → the \
             LOCAL rule-based extractor, which is not a provider substitution"
        );
    }

    // ----- ADR-0038: the removal, proven against a real wire ----------------

    /// Serve `responses` (one per incoming connection, in order) and report how
    /// many requests actually arrived. The count is the proof: a fallback hop
    /// would have to open a second connection to a second backend, and with only
    /// one backend configured the surfaced error names it and nothing else.
    async fn spawn_counting_mock(
        responses: Vec<String>,
    ) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock");
        let addr = listener.local_addr().expect("local addr");
        let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count_for_task = count.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            for body in responses {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 8192];
                let mut total = String::new();
                let mut content_len: Option<usize> = None;
                loop {
                    let n = match stream.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    total.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if content_len.is_none()
                        && let Some(hdr_end) = total.find("\r\n\r\n")
                    {
                        content_len = total[..hdr_end]
                            .to_ascii_lowercase()
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok());
                    }
                    match (content_len, total.find("\r\n\r\n")) {
                        (Some(cl), Some(hdr_end)) if total.len() - (hdr_end + 4) >= cl => break,
                        (None, Some(_)) => break,
                        _ => {}
                    }
                }
                count_for_task.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{}",
                    body.len(),
                    body,
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        (format!("http://{}", addr), count)
    }

    fn handles_with_only_api_client(endpoint: &str) -> BackendHandles {
        let client = ApiClient::new(crate::llm::ApiConfig {
            endpoint: endpoint.to_string(),
            api_key: Some("sk-route-removal-probe".to_string()),
            model: "probe-model".to_string(),
            max_tokens: 64,
            temperature: 0.1,
        })
        .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
        BackendHandles {
            // Every OTHER backend handle is deliberately loaded-and-available-free:
            // if any fallback path survived, its "not loaded / not configured"
            // error would appear in the surfaced message.
            llm_engine: Arc::new(Mutex::new(None)),
            api_client: Arc::new(Mutex::new(Some(client))),
            openrouter_client: Arc::new(Mutex::new(None)),
            mistralrs_engine: Arc::new(Mutex::new(None)),
        }
    }

    #[test]
    fn a_refused_response_does_not_reach_a_second_provider() {
        // THE removal proof (seed audio-graph-3624). The one configured route
        // returns HTTP 200 with finish_reason "content_filter" — a provider
        // refusal. Before ADR-0038 an unusable draft escalated the repair prompt
        // (which embeds the draft plus transcript-derived context) to the NEXT
        // provider in a hardcoded chain, authorized only by a privacy boolean.
        //
        // Now: exactly ONE request reaches the wire, the error names the
        // normalized terminal status, and no other provider is named or dialled.
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let refusal = serde_json::json!({
            "choices": [{
                "message": { "content": "{\"operations\":[]}" },
                "finish_reason": "content_filter"
            }]
        })
        .to_string();
        // Three responses are QUEUED so a surviving retry or repair would be
        // served rather than merely failing to connect — the assertion is on the
        // observed request count, not on the mock running out.
        let (base, request_count) = rt.block_on(spawn_counting_mock(vec![
            refusal.clone(),
            refusal.clone(),
            refusal,
        ]));

        let (job, ledger) = projection_test_job(ProjectionKind::Notes);
        let handles = handles_with_only_api_client(&base);
        let provider = LlmProvider::Api {
            endpoint: base.clone(),
            api_key: "sk-route-removal-probe".to_string(),
            model: "probe-model".to_string(),
        };
        let err = std::thread::spawn(move || {
            run_projection_patch(&handles, &provider, &job, &ledger, 1, 100)
        })
        .join()
        .expect("worker thread panic")
        .expect_err("a refusal is not a usable completion");

        assert_eq!(
            request_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a refused request must reach exactly one provider — no repair \
             escalation, no fallback hop"
        );
        assert!(err.contains("Refused"), "got: {err}");
        assert!(err.contains("route.openai_compatible"), "got: {err}");
        assert!(
            !err.contains("mistral.rs")
                && !err.contains("Native LLM")
                && !err.contains("OpenRouter"),
            "no other provider may be named, got: {err}"
        );
    }

    #[test]
    fn a_validator_rejected_draft_repairs_on_the_same_provider_only() {
        // The rejected-draft half of the same proof: the draft is transport-OK but
        // the WRONG KIND, so the validator rejects it and the repair prompt runs.
        // It must run against the SAME endpoint — two requests to one provider, not
        // one request each to two.
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let wrong_kind = serde_json::json!({
            "choices": [{
                "message": { "content": "{\"operations\":[{\"type\":\"upsert_graph_node\",\
\"id\":\"person:alice\",\"name\":\"Alice\",\"entity_type\":\"person\",\"description\":null}]}" },
                "finish_reason": "stop"
            }]
        })
        .to_string();
        let (base, request_count) = rt.block_on(spawn_counting_mock(vec![
            wrong_kind.clone(),
            wrong_kind.clone(),
            wrong_kind,
        ]));

        let (job, ledger) = projection_test_job(ProjectionKind::Notes);
        let handles = handles_with_only_api_client(&base);
        let provider = LlmProvider::Api {
            endpoint: base.clone(),
            api_key: "sk-route-removal-probe".to_string(),
            model: "probe-model".to_string(),
        };
        let err = std::thread::spawn(move || {
            run_projection_patch(&handles, &provider, &job, &ledger, 1, 100)
        })
        .join()
        .expect("worker thread panic")
        .expect_err("the same-route repair reproduces the wrong kind");

        assert_eq!(
            request_count.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "one draft + one SAME-route repair, and no third attempt"
        );
        assert!(
            err.contains("projection patch draft invalid and repair failed"),
            "got: {err}"
        );
    }

    #[test]
    fn served_model_echo_is_recorded_as_served_not_requested() {
        // ADR-0038 defect 3, end to end on the generic blocking path: the request
        // asks for `probe-model`, the response echoes a different served id, and
        // provenance must record the SERVED one with `model_source: Served`.
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{
                "message": { "content": "{\"operations\":[{\"type\":\"upsert_note\",\
        \"id\":\"note:a\",\"title\":\"T\",\"body\":\"B\",\"tags\":[]}],\"confidence\":0.7}" },
                "finish_reason": "stop"
            }],
            "model": "probe-model-turbo"
        })
        .to_string();
        let (base, _count) = rt.block_on(spawn_counting_mock(vec![body]));

        let (job, ledger) = projection_test_job(ProjectionKind::Notes);
        let handles = handles_with_only_api_client(&base);
        let provider = LlmProvider::Api {
            endpoint: base.clone(),
            api_key: "sk-route-removal-probe".to_string(),
            model: "probe-model".to_string(),
        };
        let outcome = std::thread::spawn(move || {
            run_projection_patch(&handles, &provider, &job, &ledger, 1, 100)
        })
        .join()
        .expect("worker thread panic")
        .expect("a valid notes draft");

        assert_eq!(outcome.patch.provenance.model, "probe-model-turbo");
        assert_eq!(
            outcome.patch.provenance.model_source,
            ModelIdentitySource::Served
        );
        assert_eq!(
            outcome.patch.provenance.route_id.as_deref(),
            Some("route.openai_compatible")
        );
        assert_eq!(outcome.patch.provenance.provider, "llm.api");
        let record = outcome.patch.route.as_ref().expect("route record");
        assert_eq!(record.terminal_status, TerminalStatus::Completed);
        assert_eq!(record.wire_skin, "chat_completions");
        // No prompt, reply, or credential text may ride in the route record.
        let json = serde_json::to_string(record).expect("serialize");
        assert!(!json.contains("sk-route-removal-probe"), "got: {json}");
        assert!(!json.contains("Alice met Bob"), "got: {json}");
        assert!(!json.contains("note:a"), "got: {json}");
    }

    #[test]
    fn a_mid_session_endpoint_repoint_fails_closed_instead_of_stamping_the_old_route() {
        // Critique finding 1: the job's `LlmProvider` is a session-start SNAPSHOT,
        // while egress goes through the shared client handle that
        // `sync_llm_api_client_from_settings_cache` rebuilds on every settings
        // save. A snapshot authorized as Cerebras must not egress to a re-pointed
        // endpoint under the Cerebras route id and capability row.
        let handles = handles_with_only_api_client("https://api.openai.com/v1");
        let (job, ledger) = projection_test_job(ProjectionKind::Notes);
        let stale_snapshot = LlmProvider::Api {
            endpoint: crate::settings::CEREBRAS_BASE_URL.to_string(),
            api_key: "sk-cerebras".to_string(),
            model: "gpt-oss-120b".to_string(),
        };

        let err = run_projection_patch(&handles, &stale_snapshot, &job, &ledger, 1, 100)
            .expect_err("a re-pointed client must fail closed");
        assert!(err.contains("route.cerebras_direct"), "got: {err}");
        assert!(err.contains("route.openai_compatible"), "got: {err}");
        assert!(err.contains("re-authorization required"), "got: {err}");
        assert!(
            !err.contains("api.openai.com") && !err.contains("sk-cerebras"),
            "the refusal must stay content-free, got: {err}"
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
