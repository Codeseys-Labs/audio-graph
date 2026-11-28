//! Priority executor for LLM-backed work.
//!
//! Entity extraction is background work; chat/agent requests are interactive
//! work. Running both through this single executor prevents background
//! extraction jobs from monopolizing the shared LLM/API handles.

use std::collections::{BTreeSet, VecDeque};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, LazyLock, Mutex};

use audio_graph_ipc_contract::runtime_diagnostic::{
    RuntimeDiagnostic, RuntimeDiagnosticContext, RuntimeErrorCode as Code, RuntimeOperation,
};

use crate::crash_handler::{RedactedPanicOp, catch_redacted_panic};
use crate::error::AppError;
use crate::graph::entities::ExtractionResult;
use crate::llm::engine::{ChatMessage, ChatOutcome, engine_not_loaded};
use crate::llm::{ApiClient, LlmEngine, MistralRsEngine, OpenRouterClient};
use crate::projection_llm::{
    PROJECTION_PATCH_PROMPT_ID, PROJECTION_PATCH_REPAIR_PROMPT_ID, ProjectionContext,
    ProjectionPatchBuildContext, ProjectionPatchDraftError, ProjectionSnapshots,
    projection_patch_draft_json_schema, projection_patch_prompt_messages,
    projection_patch_repair_prompt_messages, projection_patch_strict_json_schema,
    trusted_projection_patch_from_model_json,
};
use crate::projections::{ProjectionJob, ProjectionKind, ProjectionPatch, TranscriptLedger};
use crate::settings::LlmProvider;

use super::http_diag::{SchemaRejection, diagnostic};
use super::route::{
    AttemptedRouteIdentity, AuthorizedRoute, BlockingBackend, ConstrainedDecodingGrade,
    ModelIdentitySource, RequestOutputForm, RouteRecord, TerminalStatus, WireOutcome,
    authorize_route_dispatch, owned_model_id_for_place, retry_class_for_terminal_status,
    route_for_api_endpoint, route_for_openrouter_policy,
};
use audio_graph_ipc_contract::model_id::ModelPlacement;

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

/// Back off background work only on typed provider rate-limit evidence.
fn note_extraction_error(diagnostic: &RuntimeDiagnostic) {
    if matches!(diagnostic, RuntimeDiagnostic::Runtime(error) if error.code == Code::RateLimited) {
        EXTRACTION_COOLDOWN_UNTIL_MS.store(now_ms() + EXTRACTION_COOLDOWN_MS, Ordering::Relaxed);
        log::warn!(
            "Extraction rate-limited — pausing background extraction for {}s to preserve \
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
    run: JobRunner,
}

/// Runs one job to its reply. Shared, so a replacement worker runs the same one.
type JobRunner = Arc<dyn Fn(LlmJob) + Send + Sync>;

struct QueueState {
    interactive: VecDeque<LlmJob>,
    background: VecDeque<LlmJob>,
    worker: Worker,
}

/// Who is taking jobs off the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Worker {
    /// A thread is taking jobs.
    Running,
    /// No thread is taking jobs: none has started yet, or the last one ended
    /// on a job's panic. The next job starts one.
    Retired,
    /// A thread could not be started. Nothing would take a job, so none is
    /// queued.
    Unavailable,
}

/// A queued unit of LLM work.
///
/// `provider` is the session's configured provider; the worker resolves it to
/// exactly ONE authorized route (`llm/route.rs`). There is no per-job
/// fallback-policy flag: ADR-0038 removed automatic cross-provider fallback, so
/// there is nothing for a flag to select.
// `ProjectionPatch` now carries an `Option<MaterializedNotes>` (seed
// audio-graph-253c part 2), widening the size gap against `Extract`/`Chat`;
// boxing it would ripple through every construction/match site for
// negligible benefit (same tradeoff `LlmJobResult` already accepted below).
#[allow(clippy::large_enum_variant)]
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
        /// Notes and speaker attribution captured for this session before enqueue.
        snapshots: ProjectionSnapshots,
        sequence: u64,
        created_at_ms: u64,
        provider: LlmProvider,
        response_tx: mpsc::Sender<LlmJobResult>,
    },
}

impl LlmJob {
    fn response_tx(&self) -> &mpsc::Sender<LlmJobResult> {
        match self {
            Self::Extract { response_tx, .. }
            | Self::Chat { response_tx, .. }
            | Self::ProjectionPatch { response_tx, .. } => response_tx,
        }
    }
}

// Channel result enum: boxing the large `ProjectionPatch` variant would
// ripple through every construction and match site for negligible benefit.
#[allow(clippy::large_enum_variant)]
enum LlmJobResult {
    Extraction(Option<ExtractionResult>),
    Chat(Result<ChatOutcome, RuntimeDiagnostic>),
    ProjectionPatch(ProjectionPatchAttempt),
    /// The worker ended on a job's panic before this job was answered.
    WorkerEnded,
}

#[derive(Debug, Clone)]
pub struct ProjectionPatchOutcome {
    pub patch: ProjectionPatch,
    pub tokens_used: u32,
    /// Count of `upsert_note` operations the ingest no-op filter dropped from
    /// `patch.operations` at admission (audio-graph-a6b5 W2 / design-b
    /// §1.5b) — see `projection_llm::TrustedProjectionPatch`. 0 for a
    /// Graph-kind patch, or when the caller had no live notes state to
    /// compare against.
    pub no_op_filtered_count: u32,
}

/// One projection-patch dispatch attempt, paired with the identity of the
/// route it actually reached — if resolution got that far before failing.
///
/// On success `outcome`'s patch provenance already carries this identity
/// (ADR-0038's route table stamps it into `ProjectionProvenance::provider`);
/// `attempted_route` exists so a FAILURE can ledger under that SAME identity
/// instead of falling back to the session-start `LlmProvider` snapshot, which
/// is exactly what seeds audio-graph-862c / audio-graph-7da4 ask for. `None`
/// only when the attempt failed before a live route was resolved — the client
/// was never configured, or `AuthorizedRoute::refine_within_authorization`
/// refused a cross-provider repoint — and in both of those cases nothing was
/// actually dialled, so the caller's snapshot-derived local fallback is
/// correct, not stale.
#[derive(Debug, Clone)]
pub struct ProjectionPatchAttempt {
    pub outcome: Result<ProjectionPatchOutcome, String>,
    pub attempted_route: Option<AttemptedRouteIdentity>,
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
        let handles = BackendHandles {
            llm_engine,
            api_client,
            openrouter_client,
            mistralrs_engine,
        };
        Self::with_runner(Arc::new(move |job| run_job(&handles, job)))
    }

    fn with_runner(run: JobRunner) -> Self {
        let executor = Self {
            queue: Arc::new((
                Mutex::new(QueueState {
                    interactive: VecDeque::new(),
                    background: VecDeque::new(),
                    worker: Worker::Retired,
                }),
                Condvar::new(),
            )),
            run,
        };
        executor.start_worker(&mut executor.queue.0.lock().unwrap_or_else(|e| e.into_inner()));
        executor
    }

    /// Start a worker thread. The caller holds the queue lock, so the queue
    /// and the record of who serves it change together.
    fn start_worker(&self, state: &mut QueueState) {
        let queue = self.queue.clone();
        let run = self.run.clone();
        // The spawn error is OS prose; the failure itself is the whole signal.
        let spawned = std::thread::Builder::new()
            .name("llm-executor".to_string())
            .spawn(move || worker_loop(queue, run));
        state.worker = if spawned.is_ok() {
            Worker::Running
        } else {
            log::error!("Failed to spawn LLM executor thread");
            Worker::Unavailable
        };
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
            Ok(LlmJobResult::WorkerEnded) => {
                log::warn!(
                    "LLM extraction falling back: the executor worker ended on a job's panic"
                );
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
    ) -> Result<ChatOutcome, RuntimeDiagnostic> {
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
            // The worker answered a chat job with another job's result.
            Ok(LlmJobResult::Extraction(_) | LlmJobResult::ProjectionPatch(_)) => {
                Err(executor_failure(Code::Internal))
            }
            Ok(LlmJobResult::WorkerEnded) | Err(_) => Err(no_worker_reply()),
        }
    }

    /// Generate a structured notes/graph projection patch from a basis-bound
    /// projection job.
    ///
    /// Runtime projection dispatch calls this from live ASR observation after
    /// the scheduler starts a basis-bound job. Callers must still validate and
    /// apply the returned patch through `AppState::apply_runtime_projection_patch`.
    ///
    /// `notes` is the live Notes-kind snapshot (seed audio-graph-253c part 2),
    /// cloned by the caller BEFORE this call — this executor never reaches
    /// back into `AppState` for it. `Some(&materialized)` renders the "Current
    /// notes state" prompt block via `projection_patch_prompt_messages`;
    /// `None` omits it rather than asserting a fabricated "no notes yet".
    pub fn generate_projection_patch(
        &self,
        job: ProjectionJob,
        ledger: TranscriptLedger,
        snapshots: impl Into<ProjectionSnapshots>,
        provider: LlmProvider,
        sequence: u64,
        created_at_ms: u64,
    ) -> ProjectionPatchAttempt {
        let (response_tx, response_rx) = mpsc::channel();
        self.enqueue(
            LlmPriority::Background,
            LlmJob::ProjectionPatch {
                job,
                ledger,
                snapshots: snapshots.into(),
                sequence,
                created_at_ms,
                provider,
                response_tx,
            },
        );

        // The two mismatch arms never reached a route. A job whose worker
        // ended may have been dispatched before the panic; that stamp is lost
        // with the reply.
        match response_rx.recv() {
            Ok(LlmJobResult::ProjectionPatch(attempt)) => attempt,
            Ok(LlmJobResult::Extraction(_)) => ProjectionPatchAttempt {
                outcome: Err(
                    "LLM executor returned extraction result for projection request".to_string(),
                ),
                attempted_route: None,
            },
            Ok(LlmJobResult::Chat(_)) => ProjectionPatchAttempt {
                outcome: Err("LLM executor returned chat result for projection request".to_string()),
                attempted_route: None,
            },
            Ok(LlmJobResult::WorkerEnded) | Err(_) => ProjectionPatchAttempt {
                outcome: Err(no_worker_reply().to_string()),
                attempted_route: None,
            },
        }
    }

    fn enqueue(&self, priority: LlmPriority, job: LlmJob) {
        let (lock, cvar) = &*self.queue;
        let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
        if state.worker == Worker::Retired {
            self.start_worker(&mut state);
        }
        if state.worker == Worker::Running {
            push_job(&mut state, priority, job);
            cvar.notify_one();
        }
        // Otherwise the job is dropped on return, and its reply sender with
        // it: the caller's `recv` fails at once instead of waiting on a queue
        // nobody drains.
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

/// Ends a worker's claim on the queue however its thread ends.
struct RetireOnExit<'a>(&'a (Mutex<QueueState>, Condvar));

impl Drop for RetireOnExit<'_> {
    fn drop(&mut self) {
        let queued = {
            let mut state = self.0.0.lock().unwrap_or_else(|e| e.into_inner());
            state.worker = Worker::Retired;
            (
                std::mem::take(&mut state.interactive),
                std::mem::take(&mut state.background),
            )
        };
        // Outside the lock: a job can hold a whole ledger. Each caller is told
        // why no result is coming, then its job is dropped.
        for job in queued.0.into_iter().chain(queued.1) {
            let _ = job.response_tx().send(LlmJobResult::WorkerEnded);
        }
    }
}

fn worker_loop(queue: Arc<(Mutex<QueueState>, Condvar)>, run: JobRunner) {
    let _retire = RetireOnExit(&queue);
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

        // A job that panics would otherwise end this thread with jobs still
        // queued, and every caller behind it would wait forever. The unwind
        // drops the job, so `reply` is kept outside it to tell its caller. The
        // scope keeps the payload out of crash reports and stderr.
        let reply = job.response_tx().clone();
        if catch_redacted_panic(RedactedPanicOp::LlmExecutorJob, || run(job)).is_err() {
            log::error!("LLM executor job panicked; retiring the worker and failing queued jobs");
            let _ = reply.send(LlmJobResult::WorkerEnded);
            // `_retire` tells the queued jobs. The next job starts a new worker.
            return;
        }
    }
}

fn run_job(handles: &BackendHandles, job: LlmJob) {
    match job {
        LlmJob::Extract {
            text,
            speaker,
            context,
            provider,
            response_tx,
        } => {
            let result = run_extraction(handles, &provider, &text, &speaker, &context);
            let _ = response_tx.send(LlmJobResult::Extraction(result));
        }
        LlmJob::Chat {
            messages,
            graph_context,
            provider,
            response_tx,
        } => {
            let result = run_chat(handles, &provider, &messages, &graph_context);
            let _ = response_tx.send(LlmJobResult::Chat(result));
        }
        LlmJob::ProjectionPatch {
            job,
            ledger,
            snapshots,
            sequence,
            created_at_ms,
            provider,
            response_tx,
        } => {
            let result = run_projection_patch(
                handles,
                &provider,
                &job,
                &ledger,
                snapshots.view(),
                sequence,
                created_at_ms,
            );
            let _ = response_tx.send(LlmJobResult::ProjectionPatch(result));
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
) -> Result<ChatOutcome, RuntimeDiagnostic> {
    let route = authorize_and_budget_route_dispatch(handles, provider).map_err(route_refusal)?;
    match route.descriptor().blocking_backend {
        Some(BlockingBackend::NativeLlama) => chat_native(handles, messages, graph_context),
        Some(BlockingBackend::OpenAiCompatible) => {
            chat_api(handles, &route, messages, graph_context)
        }
        Some(BlockingBackend::OpenRouter) => {
            chat_openrouter(handles, &route, messages, graph_context)
        }
        Some(BlockingBackend::MistralRs) => chat_mistralrs(handles, messages, graph_context),
        // This provider serves streaming chat only.
        None => Err(executor_failure(Code::Unsupported)),
    }
}

/// A closed diagnostic for a failure the executor detects itself, outside any
/// backend call.
fn executor_failure(code: Code) -> RuntimeDiagnostic {
    diagnostic(
        code,
        RuntimeDiagnosticContext::new(RuntimeOperation::LanguageModelInference),
    )
}

/// The closed diagnostic for a chat dispatch the route layer refused before
/// any backend was reached.
fn route_refusal(error: AppError) -> RuntimeDiagnostic {
    match error {
        AppError::RuntimeDiagnostic(diagnostic) => diagnostic,
        // The provider is compiled out of this build.
        AppError::ProviderUnavailable { .. } => executor_failure(Code::Unsupported),
        // The start gate, the privacy mode, or a wire skin not admitted.
        _ => executor_failure(Code::PolicyBlocked),
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

/// Terminal for a model reply that never became a valid patch, on any route.
fn unusable_reply() -> String {
    executor_failure(Code::InvalidResponse).to_string()
}

/// Terminal for a job no worker answered: the worker thread never started, or
/// it dropped the reply channel.
fn no_worker_reply() -> RuntimeDiagnostic {
    executor_failure(Code::Internal)
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
    /// Where the model ran: the endpoint the client dialled decides.
    model_placement: ModelPlacement,
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

/// Dispatch one projection-patch job and report the route it actually reached
/// alongside the outcome (seeds audio-graph-862c / audio-graph-7da4: the
/// failure ledger must be able to name that route, not the session-start
/// snapshot). `run_projection_patch_dispatch` does the real work and takes an
/// out-param for the attempted identity so `?` still works inside it; this
/// wrapper is what every caller (and every existing test) sees.
fn run_projection_patch(
    handles: &BackendHandles,
    provider: &LlmProvider,
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
    snapshot: ProjectionContext<'_>,
    sequence: u64,
    created_at_ms: u64,
) -> ProjectionPatchAttempt {
    let mut attempted_route = None;
    let outcome = run_projection_patch_dispatch(
        handles,
        provider,
        job,
        ledger,
        snapshot,
        sequence,
        created_at_ms,
        &mut attempted_route,
    );
    ProjectionPatchAttempt {
        outcome,
        attempted_route,
    }
}

// 8 params since the notes-snapshot wiring (seed audio-graph-253c part 2)
// pushed this one over clippy's default threshold of 7; bundling would
// obscure the out-param contract documented above.
#[allow(clippy::too_many_arguments)]
fn run_projection_patch_dispatch(
    handles: &BackendHandles,
    provider: &LlmProvider,
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
    snapshot: ProjectionContext<'_>,
    sequence: u64,
    created_at_ms: u64,
    attempted_route: &mut Option<AttemptedRouteIdentity>,
) -> Result<ProjectionPatchOutcome, String> {
    let route = authorize_and_budget_route_dispatch(handles, provider)
        .map_err(|error| error.to_string())?;
    // Live-notes snapshot wiring (seed audio-graph-253c part 2): `notes` is
    // whatever the caller cloned under `AppState`'s
    // `materialized_projection_state` lock at job-spawn time
    // (`ProjectionRuntimeHandle::materialized_notes_snapshot_for_session`),
    // threaded all the way down through `LlmJob::ProjectionPatch` ->
    // `generate_projection_patch` -> here. `None` means either a Graph-kind
    // job (which never renders the notes block) or a Notes-kind caller that
    // could not confirm the snapshot belonged to this job's own session — in
    // both cases the Notes-kind prompt OMITS the "Document outline" block
    // entirely instead of asserting a false "(no sections yet)": an empty
    // snapshot is indistinguishable from a real empty session and would
    // license the model to mint ids as if none existed, which is the same
    // overwrite-storm failure mode this seed exists to fix.
    let messages =
        projection_patch_prompt_messages(job, ledger, snapshot).map_err(|e| e.to_string())?;
    let cache_context = ProjectionCacheContext {
        session_id: job.session_id.clone(),
        cache_breakpoint_message_index:
            crate::projection_llm::PROJECTION_STABLE_PREFIX_MESSAGE_COUNT.saturating_sub(1),
        kind: job.kind.clone(),
    };

    run_projection_patch_on_route(
        |messages| projection_on_route(handles, &route, messages, &cache_context, attempted_route),
        &messages,
        job,
        ledger,
        snapshot,
        sequence,
        created_at_ms,
    )
}

/// Dispatch a projection prompt on one authorized route.
///
/// `attempted_route` is stamped by whichever backend function below resolves
/// the LIVE route, before that function's fallible network/engine call — so a
/// subsequent `Err` still leaves the caller able to name the route that was
/// actually reached. Left `None` when this returns before a live route
/// resolves (client not configured, or a cross-provider repoint refused by
/// `AuthorizedRoute::refine_within_authorization`); in both cases nothing was
/// dialled, so the caller's snapshot-derived fallback is correct there.
///
/// This function runs twice per patch attempt (draft, then same-route repair
/// — see `run_projection_patch_on_route` below), each time against the SAME
/// `attempted_route` out-param. Every backend function stamps via
/// `stamp_attempted_route`, which FOLDS the new resolution with whatever the
/// first call already stamped rather than overwriting it, so a settings save
/// between the two calls cannot make the first dial's egress disappear.
fn projection_on_route(
    handles: &BackendHandles,
    route: &AuthorizedRoute,
    messages: &[ChatMessage],
    cache: &ProjectionCacheContext,
    attempted_route: &mut Option<AttemptedRouteIdentity>,
) -> Result<ProjectionBackendOutput, String> {
    match route.descriptor().blocking_backend {
        Some(BlockingBackend::NativeLlama) => {
            projection_native(handles, route, messages, attempted_route)
        }
        Some(BlockingBackend::OpenAiCompatible) => {
            projection_api(handles, route, messages, cache, attempted_route)
        }
        Some(BlockingBackend::OpenRouter) => {
            projection_openrouter(handles, route, messages, cache, attempted_route)
        }
        Some(BlockingBackend::MistralRs) => {
            projection_mistralrs(handles, route, messages, attempted_route)
        }
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
    snapshot: ProjectionContext<'_>,
    sequence: u64,
    created_at_ms: u64,
) -> Result<ProjectionPatchOutcome, String> {
    let output = run(messages)?;
    ensure_usable_completion(&output)?;

    match projection_outcome_from_output(
        &output,
        job,
        ledger,
        snapshot,
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
                snapshot,
                &output.raw_json,
                &first_error,
            )
            .map_err(|e| e.to_string())?;
            let repair_output = run(&repair_messages)?;
            ensure_usable_completion(&repair_output)?;
            let mut outcome = projection_outcome_from_output(
                &repair_output,
                job,
                ledger,
                snapshot,
                sequence,
                created_at_ms,
                PROJECTION_PATCH_REPAIR_PROMPT_ID,
                Some("repair"),
            )
            // The validator's errors served the repair prompt and stop here:
            // a parse error can quote the reply and a validation error can
            // carry a model-chosen operation id.
            .map_err(|_| unusable_reply())?;
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

// 8 params, same clippy threshold reason as `run_projection_patch_dispatch`
// above: the notes-snapshot wiring (seed audio-graph-253c part 2, extended by
// audio-graph-a6b5 W2 to reach the no-op filter here) pushed this one over
// clippy's default threshold of 7; bundling would obscure the arguments.
#[allow(clippy::too_many_arguments)]
fn projection_outcome_from_output(
    output: &ProjectionBackendOutput,
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
    snapshot: ProjectionContext<'_>,
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
    let trusted = trusted_projection_patch_from_model_json(
        &output.raw_json,
        job,
        ledger,
        snapshot,
        ProjectionPatchBuildContext {
            sequence,
            llm_request_id,
            provider: output.provider_id.to_string(),
            model: output.model.clone(),
            model_source: output.model_source,
            model_placement: Some(output.model_placement),
            route_id: Some(output.route_id.to_string()),
            route: Some(output.route_record()),
            prompt_id: prompt_id.to_string(),
            created_at_ms,
        },
    )?;
    Ok(ProjectionPatchOutcome {
        patch: trusted.patch,
        tokens_used: output.tokens_used,
        no_op_filtered_count: trusted.no_op_filtered_count,
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

/// Stamp a newly-resolved live attempt into the shared out-param, folding it
/// with whatever the FIRST dial already stamped rather than overwriting it.
///
/// `run_projection_patch_on_route` invokes the backend function below twice
/// on the SAME `attempted_route` — once for the draft, once for the
/// same-route repair — and each invocation re-resolves the live client, so a
/// mid-job settings save can make the repair dial's identity disagree with
/// the draft dial's (seed audio-graph-862c / audio-graph-7da4 follow-up,
/// "two-dial projection job whose live client is repointed... between the
/// initial call and the repair call"). `AttemptedRouteIdentity::fold` keeps
/// the max-egress identity across both dials instead of letting whichever
/// dial happens second silently erase the other's egress.
fn stamp_attempted_route(
    attempted_route: &mut Option<AttemptedRouteIdentity>,
    identity: AttemptedRouteIdentity,
) {
    *attempted_route = Some(match attempted_route.take() {
        Some(previous) => previous.fold(identity),
        None => identity,
    });
}

fn projection_api(
    handles: &BackendHandles,
    route: &AuthorizedRoute,
    messages: &[ChatMessage],
    cache: &ProjectionCacheContext,
    attempted_route: &mut Option<AttemptedRouteIdentity>,
) -> Result<ProjectionBackendOutput, String> {
    let client = {
        let guard = handles.api_client.lock().map_err(|e| e.to_string())?;
        guard
            .as_ref()
            .ok_or_else(|| "API LLM client is not configured".to_string())?
            .clone()
    };
    let live = api_route_for_client(route, &client)?;
    // Stamped from the LIVE client's endpoint, BEFORE the fallible network
    // call below, so a failure still names the route actually dialled rather
    // than the (possibly stale) session-start snapshot `route` was minted
    // from (seeds audio-graph-862c / audio-graph-7da4). Folded, not
    // overwritten, so a repair dial that lands on a re-pointed client cannot
    // erase the draft dial's egress (see `stamp_attempted_route`).
    let identity = AttemptedRouteIdentity::for_route(live, Some(&client.config().endpoint));
    stamp_attempted_route(attempted_route, identity);
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
            // A missing model (404) is not a refused schema; only OpenRouter
            // reads a 404 as "no provider for this model takes the schema".
            // Any 400 or 422 still resends once, for the reason given at the
            // extraction downgrade in `api_client.rs`.
            Err(error) if error.schema_rejection == Some(SchemaRejection::Request) => {
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
            Err(error) => return Err(error.into()),
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
        &client.config().endpoint,
    ))
}

fn projection_openrouter(
    handles: &BackendHandles,
    route: &AuthorizedRoute,
    messages: &[ChatMessage],
    cache: &ProjectionCacheContext,
    attempted_route: &mut Option<AttemptedRouteIdentity>,
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
    // OpenRouter has no loopback deployment, so unlike the API route above no
    // endpoint string is needed to know this is remote. Folded, not
    // overwritten (see `stamp_attempted_route`) — always a no-op fold here
    // since every OpenRouter row is cloud, but kept consistent with the other
    // three backend functions rather than a special-cased overwrite.
    stamp_attempted_route(
        attempted_route,
        AttemptedRouteIdentity::for_route(live, None),
    );
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
        match client.chat_completion_diagnostic(prompt_tuples(messages), &form, Some(hint.clone()))
        {
            Ok((raw_json, tokens_used, wire)) => {
                return Ok(projection_output_from_wire(
                    route,
                    live,
                    raw_json,
                    requested_model,
                    tokens_used,
                    wire,
                    &client.config().base_url,
                ));
            }
            Err(e) if e.schema_rejection.is_some() => {
                if e.schema_rejection == Some(SchemaRejection::Model) {
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
            Err(e) => return Err(e.into()),
        }
    }

    let (raw_json, tokens_used, wire) = client.chat_completion_diagnostic(
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
        &client.config().base_url,
    ))
}

/// Assemble the route-stamped output. The route id and provider id come from
/// trusted code; `model` is the SERVED id when the response reported one, and
/// `model_source` records which it is so no reader can mistake a config echo for
/// served identity (ADR-0038 defect 3).
///
/// This is the one place a model id enters provenance, so it is where the rule
/// in `ipc_contract::model_id` is applied, by the endpoint the client dialled:
/// a model at an endpoint that cannot be on the user's own machine or network
/// keeps its hosted identifier, and any other shows a display name. A served
/// id was already handled where it was read. The requested id is the user's
/// own configuration, which for a local server is often the path of the model
/// file.
fn projection_output_from_wire(
    route: &AuthorizedRoute,
    live: &'static crate::llm::route::RouteDescriptor,
    raw_json: String,
    requested_model: String,
    tokens_used: u32,
    wire: WireOutcome,
    endpoint: &str,
) -> ProjectionBackendOutput {
    let (model, model_source) = match wire.served_model.clone() {
        Some(served) => (served, ModelIdentitySource::Served),
        None => (requested_model, ModelIdentitySource::Requested),
    };
    let model_placement = ModelPlacement::at(endpoint);
    let model = owned_model_id_for_place(model, model_placement.ran_remotely());
    ProjectionBackendOutput {
        raw_json,
        route_id: live.id,
        provider_id: live.provider_id,
        wire_skin: route.skin().as_str(),
        model,
        model_source,
        model_placement,
        tokens_used,
        wire,
        completion_budget_clamped: route.completion_budget_clamped(),
    }
}

fn projection_native(
    handles: &BackendHandles,
    route: &AuthorizedRoute,
    messages: &[ChatMessage],
    attempted_route: &mut Option<AttemptedRouteIdentity>,
) -> Result<ProjectionBackendOutput, String> {
    // No live client to diverge from a snapshot here — the route is the
    // identity, known before any fallible call. Folded, not overwritten (see
    // `stamp_attempted_route`) — always a no-op fold since this route is
    // fixed for the whole job, but kept consistent with the other three
    // backend functions rather than a special-cased overwrite.
    stamp_attempted_route(
        attempted_route,
        AttemptedRouteIdentity::for_route(route.descriptor(), None),
    );
    let guard = handles.llm_engine.lock().map_err(|e| e.to_string())?;
    let engine = guard
        .as_ref()
        .ok_or_else(|| "Native LLM is not loaded".to_string())?;
    let outcome = engine.chat(messages, "").map_err(|e| e.to_string())?;
    Ok(ProjectionBackendOutput {
        raw_json: outcome.text,
        route_id: route.id(),
        provider_id: route.provider_id(),
        wire_skin: route.skin().as_str(),
        // The in-process engine has no wire, so there is nothing to echo: the
        // loaded model IS the requested one, recorded as such.
        model: "loaded_local_llama".to_string(),
        model_source: ModelIdentitySource::Requested,
        model_placement: ModelPlacement::Local,
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
    attempted_route: &mut Option<AttemptedRouteIdentity>,
) -> Result<ProjectionBackendOutput, String> {
    // Folded, not overwritten (see `stamp_attempted_route`) — always a no-op
    // fold since this route is fixed for the whole job, but kept consistent
    // with the other three backend functions rather than a special-cased
    // overwrite.
    stamp_attempted_route(
        attempted_route,
        AttemptedRouteIdentity::for_route(route.descriptor(), None),
    );
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
            let (raw_json, tokens_used) = engine
                .chat_with_history_usage(messages, "")
                .map_err(|e| e.to_string())?;
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
        model_placement: ModelPlacement::Local,
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

/// Lock a backend slot for extraction, refusing a poisoned one.
///
/// A panic that unwound through this lock left the backend in a state nobody
/// inspected. Chat and projection refuse such a slot. Extraction falls back to
/// the rule-based extractor rather than call a model in that state.
fn extraction_slot<T>(slot: &Mutex<Option<T>>) -> Option<std::sync::MutexGuard<'_, Option<T>>> {
    match slot.lock() {
        Ok(guard) => Some(guard),
        Err(_) => {
            log::warn!("LLM extraction falling back: the backend slot is poisoned");
            None
        }
    }
}

fn extract_native(handles: &BackendHandles, text: &str, speaker: &str) -> Option<ExtractionResult> {
    let guard = extraction_slot(&handles.llm_engine)?;
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
        let guard = extraction_slot(&handles.api_client)?;
        guard.as_ref()?.clone()
    };
    if let Err(error) = api_route_for_client(route, &client) {
        log::warn!("LLM extraction route refused: {error}");
        return None;
    }
    match client.extract_entities_diagnostic(text, speaker, context) {
        Ok(result) => Some(result),
        Err(e) => {
            log::warn!("API extraction failed: {}", e);
            note_extraction_error(&e.diagnostic);
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
        let guard = extraction_slot(&handles.openrouter_client)?;
        guard.as_ref()?.clone()
    };
    if let Err(error) = openrouter_route_for_client(route, &client) {
        log::warn!("LLM extraction route refused: {error}");
        return None;
    }
    match client.extract_entities_diagnostic(text, speaker, context) {
        Ok(result) => Some(result),
        Err(e) => {
            log::warn!("OpenRouter extraction failed: {}", e);
            note_extraction_error(&e.diagnostic);
            None
        }
    }
}

fn extract_mistralrs(
    handles: &BackendHandles,
    text: &str,
    speaker: &str,
) -> Option<ExtractionResult> {
    let guard = extraction_slot(&handles.mistralrs_engine)?;
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
) -> Result<ChatOutcome, RuntimeDiagnostic> {
    let guard = handles
        .llm_engine
        .lock()
        .map_err(|_| executor_failure(Code::Internal))?;
    let engine = guard.as_ref().ok_or_else(engine_not_loaded)?;
    // The native engine's inference loop counts prompt + completion tokens, so
    // this is the one blocking backend that surfaces a real `tokens_used`.
    engine.chat(messages, graph_context)
}

fn chat_api(
    handles: &BackendHandles,
    route: &AuthorizedRoute,
    messages: &[ChatMessage],
    graph_context: &str,
) -> Result<ChatOutcome, RuntimeDiagnostic> {
    // Clone + drop the guard before the blocking HTTP request (see extract_api).
    let client = {
        let guard = handles
            .api_client
            .lock()
            .map_err(|_| executor_failure(Code::Internal))?;
        guard
            .as_ref()
            .ok_or_else(|| executor_failure(Code::InvalidConfiguration))?
            .clone()
    };
    // A client repointed outside the authorized provider is refused.
    let live =
        api_route_for_client(route, &client).map_err(|_| executor_failure(Code::PolicyBlocked))?;
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
) -> Result<ChatOutcome, RuntimeDiagnostic> {
    // Clone + drop the guard before the blocking HTTP request (see extract_api).
    let client = {
        let guard = handles
            .openrouter_client
            .lock()
            .map_err(|_| executor_failure(Code::Internal))?;
        guard
            .as_ref()
            .ok_or_else(|| executor_failure(Code::InvalidConfiguration))?
            .clone()
    };
    let live = openrouter_route_for_client(route, &client)
        .map_err(|_| executor_failure(Code::PolicyBlocked))?;
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
) -> Result<ChatOutcome, RuntimeDiagnostic> {
    let guard = handles
        .mistralrs_engine
        .lock()
        .map_err(|_| executor_failure(Code::Internal))?;
    let engine = guard.as_ref().ok_or_else(engine_not_loaded)?;
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
    use crate::llm::http_diag::canary::assert_absent;
    use crate::projections::{
        ProjectionKind, ProjectionOperation, ProjectionPriority, TranscriptEvent,
        TranscriptEventStability,
    };
    use crate::test_log_capture::{ThreadLogCapture, render};
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

    // ----- cooldown set / observe ------------------------------------------

    #[test]
    fn note_extraction_error_sets_cooldown_for_rate_limit() {
        let _guard = COOLDOWN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Reset to a clean state (no cooldown).
        EXTRACTION_COOLDOWN_UNTIL_MS.store(0, Ordering::Relaxed);
        assert!(!extraction_in_cooldown());

        note_extraction_error(&super::super::http_diag::diagnostic(
            Code::RateLimited,
            super::super::http_diag::INFERENCE,
        ));
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

        note_extraction_error(&super::super::http_diag::diagnostic(
            Code::NetworkUnreachable,
            super::super::http_diag::INFERENCE,
        ));
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
            model_placement: ModelPlacement::Remote,
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
                "tags": ["people"],
                "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
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
            &projection_patch_prompt_messages(&job, &ledger, None).expect("initial prompt"),
            &job,
            &ledger,
            None.into(),
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
        // Each reply fails to parse, and the parser's error quotes the value.
        let reply = |value: &str| {
            serde_json::json!({
                "operations": [{ "type": value, "id": "validation-id-canary" }]
            })
            .to_string()
        };
        let draft = reply("validation-value-canary");
        let repair = reply("repair-value-canary");
        let private_detail = projection_outcome_from_output(
            &projection_output(draft.clone(), 3),
            &job,
            &ledger,
            None.into(),
            4,
            123,
            PROJECTION_PATCH_PROMPT_ID,
            None,
        )
        .expect_err("the draft is invalid")
        .to_string();
        assert!(
            private_detail.contains("validation-value-canary"),
            "the validator error must carry the value for this to be a canary: {private_detail}"
        );

        let capture = ThreadLogCapture::start();
        let err = run_projection_patch_on_route(
            {
                let call_count = call_count.clone();
                move |_messages| {
                    let mut count = call_count.lock().unwrap_or_else(|e| e.into_inner());
                    *count += 1;
                    let raw = if *count == 1 { &draft } else { &repair };
                    Ok(projection_output(raw.clone(), 3))
                }
            },
            &projection_patch_prompt_messages(&job, &ledger, None).expect("initial prompt"),
            &job,
            &ledger,
            None.into(),
            4,
            123,
        )
        .expect_err("repair remains invalid");
        let logs = render(&capture.finish());

        assert_eq!(*call_count.lock().unwrap_or_else(|e| e.into_inner()), 2);
        assert_eq!(err, "runtime/invalid_response");
        // The draft and the repair were each logged, by route identity only.
        assert_eq!(
            logs.matches("Projection patch route output").count(),
            2,
            "{logs}"
        );
        assert_absent(
            &format!("{err}\n{logs}"),
            &[
                "validation-value-canary",
                "repair-value-canary",
                "validation-id-canary",
            ],
        );
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
                "description": "Met Bob.",
                "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
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
            &projection_patch_prompt_messages(&job, &ledger, None).expect("initial prompt"),
            &job,
            &ledger,
            None.into(),
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
            &projection_patch_prompt_messages(&job, &ledger, None).expect("initial prompt"),
            &job,
            &ledger,
            None.into(),
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
            &projection_patch_prompt_messages(&job, &ledger, None).expect("initial prompt"),
            &job,
            &ledger,
            None.into(),
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
            None.into(),
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
                "tags": ["people"],
                "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
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
            &projection_patch_prompt_messages(&job, &ledger, None).expect("initial prompt"),
            &job,
            &ledger,
            None.into(),
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
            &projection_patch_prompt_messages(&job, &ledger, None).expect("initial prompt"),
            &job,
            &ledger,
            None.into(),
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
    fn only_schema_related_statuses_authorize_a_downgrade() {
        use super::super::http_diag::{HttpFailure, INFERENCE};

        for (status, expected) in [
            (400, Some(SchemaRejection::Request)),
            (404, Some(SchemaRejection::Model)),
            (422, Some(SchemaRejection::Request)),
            (401, None),
            (403, None),
            (429, None),
            (502, None),
        ] {
            let failure = HttpFailure::status(
                reqwest::StatusCode::from_u16(status).unwrap(),
                INFERENCE,
                true,
            );
            assert_eq!(failure.schema_rejection, expected, "status {status}");
        }
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
            worker: Worker::Running,
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
        // Only the native engine's own diagnostic: no other route was tried.
        assert_eq!(err, engine_not_loaded());
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
        // The missing client, not a local engine's "not loaded".
        assert_eq!(err, executor_failure(Code::InvalidConfiguration));
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
        // Not "client is not configured", which named the wrong client.
        assert_eq!(err, executor_failure(Code::Unsupported));
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

    // The cloud build's engine stub fails every call, which is enough to reach
    // the log site; a real engine needs a model file.
    #[cfg(not(feature = "llm-llama"))]
    #[test]
    fn native_extraction_failure_logs_only_the_diagnostic() {
        let handles = BackendHandles {
            llm_engine: Arc::new(Mutex::new(Some(LlmEngine))),
            ..empty_handles()
        };
        let capture = ThreadLogCapture::start();
        let result = extract_native(&handles, "heron transcript", "osprey");
        let logs = render(&capture.finish());

        assert!(result.is_none());
        assert!(
            logs.contains("Native LLM extraction failed: runtime/unsupported"),
            "{logs}"
        );
        assert_absent(&logs, &["heron", "osprey"]);
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

    /// Like [`spawn_counting_mock`], but also captures each request's raw
    /// JSON body (seed audio-graph-253c part 2's live-dispatch-path proof
    /// needs to inspect what a REAL wire call actually carried, not just how
    /// many calls were made). Bodies are captured in arrival order, so
    /// `captured[0]` is the first request's body, `captured[1]` the second's,
    /// etc.
    async fn spawn_capturing_mock(responses: Vec<String>) -> (String, Arc<StdMutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock");
        let addr = listener.local_addr().expect("local addr");
        let captured = Arc::new(StdMutex::new(Vec::new()));
        let captured_for_task = captured.clone();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            for body in responses {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 8192];
                let mut total = String::new();
                let mut content_len: Option<usize> = None;
                let mut header_end: Option<usize> = None;
                loop {
                    let n = match stream.read(&mut buf).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    total.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if header_end.is_none()
                        && let Some(hdr_end) = total.find("\r\n\r\n")
                    {
                        header_end = Some(hdr_end);
                        content_len = total[..hdr_end]
                            .to_ascii_lowercase()
                            .lines()
                            .find_map(|l| l.strip_prefix("content-length:"))
                            .and_then(|v| v.trim().parse::<usize>().ok());
                    }
                    match (content_len, header_end) {
                        (Some(cl), Some(hdr_end)) if total.len() - (hdr_end + 4) >= cl => break,
                        (None, Some(_)) => break,
                        _ => {}
                    }
                }
                if let Some(hdr_end) = header_end {
                    let request_body = total[hdr_end + 4..].to_string();
                    captured_for_task
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(request_body);
                }
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
        (format!("http://{}", addr), captured)
    }

    fn handles_with_only_api_client(endpoint: &str) -> BackendHandles {
        handles_with_api_model(endpoint, "probe-model")
    }

    fn handles_with_api_model(endpoint: &str, model: &str) -> BackendHandles {
        let client = ApiClient::new(crate::llm::ApiConfig {
            endpoint: endpoint.to_string(),
            api_key: Some("sk-route-removal-probe".to_string()),
            model: model.to_string(),
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
    fn api_extraction_failure_logs_only_the_diagnostic() {
        let _guard = COOLDOWN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        EXTRACTION_COOLDOWN_UNTIL_MS.store(0, Ordering::Relaxed);
        let rt = tokio::runtime::Runtime::new().expect("rt");
        // A 200 whose body is not a chat completion.
        let body = format!("walrus reply {}", "w".repeat(4000));
        let body_length = body.len().to_string();
        let (base, request_count) = rt.block_on(spawn_counting_mock(vec![body]));
        let handles = handles_with_only_api_client(&base);
        let provider = LlmProvider::Api {
            endpoint: base,
            api_key: "sk-route-removal-probe".to_string(),
            model: "probe-model".to_string(),
        };

        let (result, logs) = std::thread::spawn(move || {
            let capture = ThreadLogCapture::start();
            let result = run_extraction(&handles, &provider, "heron transcript", "osprey", "");
            (result, render(&capture.finish()))
        })
        .join()
        .expect("worker thread panic");

        assert!(result.is_none());
        assert_eq!(request_count.load(Ordering::SeqCst), 1);
        assert!(
            logs.contains("API extraction failed: runtime/invalid_response"),
            "{logs}"
        );
        assert_absent(
            &logs,
            &[
                "walrus",
                "heron",
                "osprey",
                "sk-route-removal-probe",
                "127.0.0.1",
                &body_length,
            ],
        );
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
        let attempt = std::thread::spawn(move || {
            run_projection_patch(&handles, &provider, &job, &ledger, None.into(), 1, 100)
        })
        .join()
        .expect("worker thread panic");
        let err = attempt
            .outcome
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
        // The request reached the wire (and was refused there), so the
        // attempted route must still be reported for the failure ledger.
        assert_eq!(
            attempt.attempted_route.map(|route| route.provider_id),
            Some("llm.api")
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
            run_projection_patch(&handles, &provider, &job, &ledger, None.into(), 1, 100)
        })
        .join()
        .expect("worker thread panic")
        .outcome
        .expect_err("the same-route repair reproduces the wrong kind");

        assert_eq!(
            request_count.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "one draft + one SAME-route repair, and no third attempt"
        );
        assert_eq!(err, "runtime/invalid_response");
    }

    /// ADR-0037 x ADR-0038: a draft rejected for MISSING CLAIM EVIDENCE
    /// repairs on the SAME route and never appears as a second-provider
    /// dispatch — the evidence-specific instance of the same-route repair
    /// proof above (`a_validator_rejected_draft_repairs_on_the_same_provider_only`
    /// covers `WrongOperationKind`; this covers `ClaimEvidenceRefused`). Before
    /// `audio-graph-3624` removed automatic cross-provider fallback, every
    /// validator rejection escalated to the next provider in the chain — the
    /// exact amplification ADR-0037's sequencing precondition exists to rule
    /// out for evidence rejections specifically.
    #[test]
    fn a_missing_evidence_rejection_repairs_on_the_same_provider_only() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let missing_evidence = serde_json::json!({
            "choices": [{
                "message": { "content": "{\"operations\":[{\"type\":\"upsert_note\",\
        \"id\":\"note:decision\",\"title\":\"Decision\",\"body\":\"Alice met Bob.\",\
        \"tags\":[]}],\"confidence\":0.8}" },
                "finish_reason": "stop"
            }]
        })
        .to_string();
        let with_evidence = serde_json::json!({
            "choices": [{
                "message": { "content": "{\"operations\":[{\"type\":\"upsert_note\",\
        \"id\":\"note:decision\",\"title\":\"Decision\",\"body\":\"Alice met Bob.\",\
        \"tags\":[],\"evidence\":{\"claim_class\":\"grounded_inference\",\
        \"span_id\":\"span-1\"}}],\"confidence\":0.8}" },
                "finish_reason": "stop"
            }]
        })
        .to_string();
        let (base, request_count) =
            rt.block_on(spawn_counting_mock(vec![missing_evidence, with_evidence]));

        let (job, ledger) = projection_test_job(ProjectionKind::Notes);
        let handles = handles_with_only_api_client(&base);
        let provider = LlmProvider::Api {
            endpoint: base.clone(),
            api_key: "sk-route-removal-probe".to_string(),
            model: "probe-model".to_string(),
        };
        let outcome = std::thread::spawn(move || {
            run_projection_patch(&handles, &provider, &job, &ledger, None.into(), 1, 100)
        })
        .join()
        .expect("worker thread panic")
        .outcome
        .expect("the same-route repair supplies the missing evidence");

        assert_eq!(
            request_count.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "one draft + one SAME-route repair, and no third attempt"
        );
        assert_eq!(
            outcome.patch.provenance.route_id.as_deref(),
            Some("route.openai_compatible")
        );
        assert!(matches!(
            outcome.patch.operations.first(),
            Some(ProjectionOperation::UpsertNote { id, .. }) if id == "note:decision"
        ));
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
        \"id\":\"note:a\",\"title\":\"T\",\"body\":\"B\",\"tags\":[],\
        \"evidence\":{\"claim_class\":\"grounded_inference\",\"span_id\":\"span-1\"}}],\
        \"confidence\":0.7}" },
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
            run_projection_patch(&handles, &provider, &job, &ledger, None.into(), 1, 100)
        })
        .join()
        .expect("worker thread panic")
        .outcome
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
    fn a_model_id_enters_provenance_by_the_endpoint_its_client_dialled() {
        use crate::llm::route::model_id_fixtures::{PATH_CANARY, SECRET_CANARY, STORED};
        use crate::llm::route::{LOCAL_MODEL_PLACEHOLDER, WITHHELD_MODEL_PLACEHOLDER};
        use ModelIdentitySource::{Requested, Served};

        let rt = tokio::runtime::Runtime::new().expect("rt");
        let reply = |served: Option<&str>| {
            let mut body = serde_json::json!({
                "choices": [{
                    "message": { "content": "{\"operations\":[{\"type\":\"upsert_note\",\
            \"id\":\"note:a\",\"title\":\"T\",\"body\":\"B\",\"tags\":[],\
            \"evidence\":{\"claim_class\":\"grounded_inference\",\"span_id\":\"span-1\"}}],\
            \"confidence\":0.7}" },
                    "finish_reason": "stop"
                }]
            });
            if let Some(served) = served {
                body["model"] = served.into();
            }
            body.to_string()
        };
        // Both clients dial a loopback server, so both models ran on the
        // user's machine: OpenRouter at a custom base URL is placed by that
        // URL, not by its name.
        let provenance = |openrouter: bool, served: Option<&str>, configured: &str| {
            let (base, _count) = rt.block_on(spawn_counting_mock(vec![reply(served)]));
            let (handles, provider) = if openrouter {
                let client = OpenRouterClient::new(crate::llm::openrouter::OpenRouterConfig {
                    base_url: base.clone(),
                    ..crate::llm::openrouter::OpenRouterConfig::with_defaults(
                        "probe-key".to_string(),
                        configured.to_string(),
                    )
                })
                .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow());
                let handles = BackendHandles {
                    llm_engine: Arc::new(Mutex::new(None)),
                    api_client: Arc::new(Mutex::new(None)),
                    openrouter_client: Arc::new(Mutex::new(Some(client))),
                    mistralrs_engine: Arc::new(Mutex::new(None)),
                };
                let provider = LlmProvider::OpenRouter {
                    model: configured.to_string(),
                    base_url: base,
                    provider_order: None,
                    include_usage_in_stream: true,
                    api_key: "probe-key".to_string(),
                };
                (handles, provider)
            } else {
                let handles = handles_with_api_model(&base, configured);
                let provider = LlmProvider::Api {
                    endpoint: base,
                    api_key: "probe-key".to_string(),
                    model: configured.to_string(),
                };
                (handles, provider)
            };
            let (job, ledger) = projection_test_job(ProjectionKind::Notes);
            std::thread::spawn(move || {
                run_projection_patch(&handles, &provider, &job, &ledger, None.into(), 1, 100)
            })
            .join()
            .expect("worker thread panic")
            .outcome
            .expect("a valid notes draft")
            .patch
        };

        for (id, local, _) in STORED {
            for openrouter in [false, true] {
                // The server reports `id` as the model it served.
                let (served_model, served_source) =
                    if local == LOCAL_MODEL_PLACEHOLDER || local == WITHHELD_MODEL_PLACEHOLDER {
                        // No name may be kept, so the requested id is recorded.
                        ("probe-model".to_string(), Requested)
                    } else {
                        (
                            crate::llm::route::sanitize_route_metadata(local).expect("a name"),
                            Served,
                        )
                    };
                let patch = provenance(openrouter, Some(id), "probe-model");
                assert_eq!(
                    patch.provenance.model, served_model,
                    "served {id} {openrouter}"
                );
                assert_eq!(patch.provenance.model_source, served_source);
                assert_eq!(
                    patch.provenance.model_placement,
                    Some(ModelPlacement::Local)
                );
                let persisted = serde_json::to_string(&patch).expect("serialize the patch");
                assert!(!persisted.contains(PATH_CANARY), "{persisted}");
                assert!(!persisted.contains(SECRET_CANARY), "{persisted}");

                // The server reports nothing, so the configured id is
                // recorded: for a local server, often its model's path.
                let patch = provenance(openrouter, None, id);
                assert_eq!(
                    patch.provenance.model, local,
                    "configured {id} {openrouter}"
                );
                assert_eq!(patch.provenance.model_source, Requested);
                let persisted = serde_json::to_string(&patch).expect("serialize the patch");
                assert!(!persisted.contains(PATH_CANARY), "{persisted}");
                assert!(!persisted.contains(SECRET_CANARY), "{persisted}");
            }
        }
    }

    #[test]
    fn a_model_at_openrouters_host_keeps_its_namespace_through_a_reload() {
        // The endpoint decides, before anything is sent: no request is made.
        for (base_url, placement, expected) in [
            (
                crate::llm::openrouter::DEFAULT_BASE_URL,
                ModelPlacement::Remote,
                "anthropic/claude-sonnet-4",
            ),
            (
                "http://127.0.0.1:9/api/v1",
                ModelPlacement::Local,
                "claude-sonnet-4",
            ),
        ] {
            let route = authorize_route_dispatch(&LlmProvider::OpenRouter {
                model: "anthropic/claude-sonnet-4".to_string(),
                base_url: base_url.to_string(),
                provider_order: None,
                include_usage_in_stream: true,
                api_key: "probe-key".to_string(),
            })
            .expect("OpenRouter is authorized");
            let mut wire = WireOutcome::plain(
                TerminalStatus::Completed,
                ConstrainedDecodingGrade::Unconstrained,
            );
            wire.served_model = Some("anthropic/claude-sonnet-4".to_string());
            let output = projection_output_from_wire(
                &route,
                route.descriptor(),
                String::new(),
                "anthropic/claude-sonnet-4".to_string(),
                0,
                wire,
                base_url,
            );
            assert_eq!(output.model, expected, "{base_url}");
            assert_eq!(output.model_placement, placement);

            // The provenance written from it reads back the same.
            let provenance = crate::projections::ProjectionProvenance {
                provider: output.provider_id.to_string(),
                model: output.model.clone(),
                prompt_id: "notes-v1".to_string(),
                route_id: Some(output.route_id.to_string()),
                model_source: output.model_source,
                model_placement: Some(output.model_placement),
            };
            let stored = serde_json::to_value(&provenance).expect("serialize provenance");
            assert_eq!(
                stored["model_placement"],
                serde_json::to_value(placement).unwrap()
            );
            let read: crate::projections::ProjectionProvenance =
                serde_json::from_value(stored).expect("read provenance");
            assert_eq!(read, provenance, "{base_url}");
        }
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

        let attempt = run_projection_patch(
            &handles,
            &stale_snapshot,
            &job,
            &ledger,
            None.into(),
            1,
            100,
        );
        let err = attempt
            .outcome
            .expect_err("a re-pointed client must fail closed");
        assert!(err.contains("route.cerebras_direct"), "got: {err}");
        assert!(err.contains("route.openai_compatible"), "got: {err}");
        assert!(err.contains("re-authorization required"), "got: {err}");
        assert!(
            !err.contains("api.openai.com") && !err.contains("sk-cerebras"),
            "the refusal must stay content-free, got: {err}"
        );
        // The refusal fires BEFORE the live route resolves, so nothing was
        // dialled and there is no attempted-route identity to report — the
        // caller correctly falls back to the (here, still-honest) snapshot.
        assert!(
            attempt.attempted_route.is_none(),
            "a refused, never-dialled dispatch must not report an attempted route"
        );
    }

    #[test]
    fn projection_api_repair_dial_does_not_erase_an_earlier_cloud_stamp() {
        // Finding: `run_projection_patch_on_route` dials `projection_api`
        // TWICE (draft, then same-route repair) against the SAME
        // `attempted_route` out-param, and `projection_api` re-resolves the
        // LIVE client on each call. Before `stamp_attempted_route` folded
        // instead of overwrote, a draft dial that reached a cloud endpoint
        // followed by a repair dial against a client re-pointed to loopback
        // would have its cloud egress silently erased by the second stamp —
        // exactly the case a mid-job settings save produces. This drives a
        // REAL wire call for the second dial (against a loopback mock server)
        // with the first dial's stamp pre-seeded, proving the fold survives
        // through `projection_api` itself, not just the pure `fold` unit
        // tests in `llm::route`.
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let body = serde_json::json!({
            "choices": [{
                "message": { "content": "{}" },
                "finish_reason": "stop"
            }]
        })
        .to_string();
        let (base, _count) = rt.block_on(spawn_counting_mock(vec![body]));

        let handles = handles_with_only_api_client(&base);
        let provider = LlmProvider::Api {
            endpoint: base.clone(),
            api_key: "sk-route-removal-probe".to_string(),
            model: "probe-model".to_string(),
        };
        let (job, ledger) = projection_test_job(ProjectionKind::Notes);
        let messages = projection_patch_prompt_messages(&job, &ledger, None).expect("prompt");
        let cache = ProjectionCacheContext {
            session_id: job.session_id.clone(),
            cache_breakpoint_message_index: 0,
            kind: job.kind.clone(),
        };

        std::thread::spawn(move || {
            let route = authorize_and_budget_route_dispatch(&handles, &provider)
                .expect("the loopback endpoint authorizes");
            // Seed the out-param as if a first (cloud) dial already ran and
            // stamped it — the exact state a repair dial inherits mid-job.
            let mut attempted_route = Some(AttemptedRouteIdentity {
                provider_id: "llm.api",
                requires_cloud_transfer: true,
            });

            projection_api(&handles, &route, &messages, &cache, &mut attempted_route)
                .expect("the loopback mock responds");

            assert_eq!(
                attempted_route,
                Some(AttemptedRouteIdentity {
                    provider_id: "llm.api",
                    requires_cloud_transfer: true,
                }),
                "a real second dial against a loopback client must not erase \
                 the first dial's cloud stamp"
            );
        })
        .join()
        .expect("worker thread panic");
    }

    // ----- end-to-end through the live worker thread -----------------------

    /// The field-bug kill shot at the REAL executor dispatch path (seed
    /// audio-graph-253c part 2). Drives two Notes-kind projection jobs through
    /// the ACTUAL `LlmExecutor` — `generate_projection_patch` -> `enqueue` ->
    /// the live `worker_loop` thread -> `run_projection_patch` ->
    /// `run_projection_patch_dispatch` -> `projection_patch_prompt_messages`
    /// -> a REAL wire call against a loopback mock — with job 1's outcome
    /// APPLIED to a `MaterializedNotes` in between, exactly what the
    /// scheduler's apply path does in production. Asserts job 2's ACTUAL WIRE
    /// REQUEST BODY (not just the pure prompt builder — part 1's
    /// `overwrite_storm_regression_next_job_prompt_contains_ids_from_previous_applied_patch`
    /// in `projection_llm.rs` already proved that layer) carries job 1's
    /// applied note id.
    ///
    /// Revert proof: hardcoding `run_projection_patch_dispatch`'s `notes`
    /// back to `let notes: Option<&MaterializedNotes> = None;` (the pre-part-2
    /// production behavior) makes this test fail, because job 2's prompt then
    /// omits the "Current notes state" block entirely and the wire request
    /// never carries `note:decision`.
    #[test]
    fn live_executor_dispatch_carries_job_ones_applied_note_id_into_job_twos_wire_request() {
        let rt = tokio::runtime::Runtime::new().expect("rt");

        let mut ledger = TranscriptLedger::new("session-livewire");
        ledger
            .apply_event(projection_test_event(
                "span-1",
                "Alice chose Soniox for the pilot.",
            ))
            .expect("seed ledger");

        // ----- Job 1: no notes exist yet (a real, empty MaterializedNotes for
        // this session — what `materialized_notes_snapshot_for_session`
        // returns on the first tick, NOT `None`).
        let job_one_response = serde_json::json!({
            "choices": [{
                "message": { "content": "{\"operations\":[{\"type\":\"upsert_note\",\
        \"id\":\"note:decision\",\"title\":\"Provider decision\",\"body\":\"Alice chose Soniox for the pilot.\",\
        \"tags\":[],\"evidence\":{\"claim_class\":\"grounded_inference\",\"span_id\":\"span-1\"}}],\
        \"confidence\":0.9}" },
                "finish_reason": "stop"
            }]
        })
        .to_string();
        let (base_one, captured_one) = rt.block_on(spawn_capturing_mock(vec![job_one_response]));

        let make_client = |base: &str| {
            ApiClient::new(crate::llm::ApiConfig {
                endpoint: base.to_string(),
                api_key: Some("sk-livewire-probe".to_string()),
                model: "probe-model".to_string(),
                max_tokens: 64,
                temperature: 0.1,
            })
            .with_content_egress_policy(crate::asr::ProviderContentEgressPolicy::allow())
        };
        let provider_for = |base: &str| LlmProvider::Api {
            endpoint: base.to_string(),
            api_key: "sk-livewire-probe".to_string(),
            model: "probe-model".to_string(),
        };

        let executor_one = LlmExecutor::new(
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(Some(make_client(&base_one)))),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
        );
        let job_one = ProjectionJob {
            id: "projection:session-livewire:notes:1".to_string(),
            session_id: "session-livewire".to_string(),
            kind: ProjectionKind::Notes,
            basis: ledger.current_basis(),
            priority: ProjectionPriority::Realtime,
            queued_at_ms: 10,
        };
        let empty_notes = crate::projections::MaterializedNotes::new("session-livewire");

        let attempt_one = executor_one.generate_projection_patch(
            job_one,
            ledger.clone(),
            Some(empty_notes),
            provider_for(&base_one),
            1,
            100,
        );
        let outcome_one = attempt_one.outcome.expect("job 1's draft succeeds");
        assert_eq!(
            captured_one.lock().unwrap_or_else(|e| e.into_inner()).len(),
            1,
            "job 1 must reach the wire exactly once"
        );

        // Apply job 1's patch — the SAME effect the scheduler's apply path
        // has on `AppState::materialized_projection_state` in production.
        let mut notes_after_job_one =
            crate::projections::MaterializedNotes::new("session-livewire");
        notes_after_job_one
            .apply_patch(&outcome_one.patch, None)
            .expect("apply job 1's patch");
        assert!(
            notes_after_job_one
                .notes
                .iter()
                .any(|note| note.id == "note:decision"),
            "job 1's patch must have introduced note:decision"
        );

        // ----- Job 2: dispatched with job 1's resulting notes snapshot.
        let job_two_response = serde_json::json!({
            "choices": [{
                "message": { "content": "{\"operations\":[{\"type\":\"upsert_note\",\
        \"id\":\"note:decision\",\"title\":\"Provider decision\",\"body\":\"Alice chose Soniox for the pilot, confirmed.\",\
        \"tags\":[],\"evidence\":{\"claim_class\":\"grounded_inference\",\"span_id\":\"span-1\"}}],\
        \"confidence\":0.9}" },
                "finish_reason": "stop"
            }]
        })
        .to_string();
        let (base_two, captured_two) = rt.block_on(spawn_capturing_mock(vec![job_two_response]));

        let executor_two = LlmExecutor::new(
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(Some(make_client(&base_two)))),
            Arc::new(Mutex::new(None)),
            Arc::new(Mutex::new(None)),
        );
        let job_two = ProjectionJob {
            id: "projection:session-livewire:notes:2".to_string(),
            session_id: "session-livewire".to_string(),
            kind: ProjectionKind::Notes,
            basis: ledger.current_basis(),
            priority: ProjectionPriority::Realtime,
            queued_at_ms: 20,
        };

        let attempt_two = executor_two.generate_projection_patch(
            job_two,
            ledger,
            Some(notes_after_job_one),
            provider_for(&base_two),
            2,
            200,
        );
        attempt_two.outcome.expect("job 2's draft succeeds");

        let requests_two = captured_two.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(
            requests_two.len(),
            1,
            "job 2 must reach the wire exactly once"
        );
        assert!(
            requests_two[0].contains("note:decision"),
            "job 2's ACTUAL WIRE REQUEST must carry job 1's applied note id — got: {}",
            requests_two[0]
        );
    }

    #[test]
    fn executor_thread_logs_no_reply_content_when_draft_and_repair_both_fail() {
        use crate::test_log_capture::{assert_process_logs_exclude, install, process_records};

        install();
        let rt = tokio::runtime::Runtime::new().expect("rt");
        // Two completed replies that are not valid patches: each carries an
        // unknown operation type and a model-chosen operation id.
        let reply = |value: &str| {
            let patch = serde_json::json!({
                "operations": [{ "type": value, "id": "quokka-operation" }]
            });
            serde_json::json!({
                "choices": [{
                    "message": { "content": patch.to_string() },
                    "finish_reason": "stop"
                }]
            })
            .to_string()
        };
        let (base, requests) = rt.block_on(spawn_capturing_mock(vec![
            reply("numbat-draft"),
            reply("bilby-repair"),
        ]));
        let BackendHandles {
            llm_engine,
            api_client,
            openrouter_client,
            mistralrs_engine,
        } = handles_with_only_api_client(&base);
        let executor =
            LlmExecutor::new(llm_engine, api_client, openrouter_client, mistralrs_engine);
        let (job, ledger) = projection_test_job(ProjectionKind::Notes);

        let logged_before = process_records().len();
        let attempt = executor.generate_projection_patch(
            job,
            ledger,
            None::<crate::projections::MaterializedNotes>,
            LlmProvider::Api {
                endpoint: base,
                api_key: "sk-route-removal-probe".to_string(),
                model: "probe-model".to_string(),
            },
            1,
            100,
        );

        assert_eq!(
            requests.lock().unwrap_or_else(|e| e.into_inner()).len(),
            2,
            "the draft and the repair must both reach the wire"
        );
        assert_eq!(
            attempt.outcome.expect_err("the repair is still invalid"),
            "runtime/invalid_response"
        );
        // The worker thread is not this one, so its records are only in the
        // process log. Those logged since the job started must include the
        // path's own line.
        assert!(
            process_records()[logged_before..]
                .iter()
                .any(|record| record.message.contains("Projection patch route output"))
        );
        assert_process_logs_exclude(&["numbat-draft", "bilby-repair", "quokka-operation"]);
    }

    #[test]
    fn an_executor_whose_worker_could_not_start_fails_at_once() {
        let executor = LlmExecutor {
            queue: Arc::new((
                Mutex::new(QueueState {
                    worker: Worker::Unavailable,
                    ..new_state()
                }),
                Condvar::new(),
            )),
            run: Arc::new(|_| unreachable!("no worker runs a job")),
        };
        // On its own thread: a regression would block here forever.
        let (done, outcome) = mpsc::channel();
        std::thread::spawn(move || {
            let chat =
                executor.chat_with_history(Vec::new(), String::new(), LlmProvider::LocalLlama);
            let (job, ledger) = projection_test_job(ProjectionKind::Notes);
            let patch = executor.generate_projection_patch(
                job,
                ledger,
                None::<crate::projections::MaterializedNotes>,
                LlmProvider::LocalLlama,
                1,
                100,
            );
            let _ = done.send((chat.map(|_| ()), patch.outcome.map(|_| ())));
        });
        let (chat, patch) = outcome
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("a job nobody will run must not wait for a reply");
        assert_eq!(chat, Err(executor_failure(Code::Internal)));
        assert_eq!(patch, Err("runtime/internal".to_string()));
    }

    /// What a native binding's panic can say.
    const JOB_PANIC_CANARY: &str = "wombat-private-model-dir";

    /// A stand-in for the backends. A chat whose graph context is `"panic"`
    /// reports that it has started, waits to be released, then panics. Every
    /// other job is answered as served.
    fn panicking_runner(started: mpsc::Sender<()>, release: mpsc::Receiver<()>) -> JobRunner {
        let started = StdMutex::new(started);
        let release = StdMutex::new(release);
        Arc::new(move |job| match job {
            LlmJob::Chat {
                graph_context,
                response_tx,
                ..
            } if graph_context == "panic" => {
                let _ = started.lock().unwrap().send(());
                let _ = release.lock().unwrap().recv();
                // Still owned here when the unwind starts, as in a real job.
                let _response_tx = response_tx;
                panic!("\"/home/{JOB_PANIC_CANARY}/model.gguf\" could not be read");
            }
            LlmJob::Chat { response_tx, .. } => {
                let _ = response_tx.send(LlmJobResult::Chat(Ok(ChatOutcome {
                    text: "served".to_string(),
                    tokens_used: 0,
                })));
            }
            LlmJob::Extract { response_tx, .. } => {
                let _ = response_tx.send(LlmJobResult::Extraction(Some(ExtractionResult {
                    entities: Vec::new(),
                    relations: Vec::new(),
                })));
            }
            LlmJob::ProjectionPatch { response_tx, .. } => {
                let _ = response_tx.send(LlmJobResult::ProjectionPatch(ProjectionPatchAttempt {
                    outcome: Err("served".to_string()),
                    attempted_route: None,
                }));
            }
        })
    }

    /// Run `call` on its own thread: a regression would block it forever.
    fn spawn_caller<T: Send + 'static>(
        call: impl FnOnce() -> T + Send + 'static,
    ) -> mpsc::Receiver<T> {
        let (done, outcome) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = done.send(call());
        });
        outcome
    }

    fn chat_caller(
        executor: &LlmExecutor,
        tag: &str,
    ) -> mpsc::Receiver<Result<String, RuntimeDiagnostic>> {
        let (executor, tag) = (executor.clone(), tag.to_string());
        spawn_caller(move || {
            executor
                .chat_with_history(Vec::new(), tag, LlmProvider::LocalLlama)
                .map(|outcome| outcome.text)
        })
    }

    /// Run `work` on its own thread and fail if it has not returned in `limit`.
    fn within<T: Send + 'static>(
        limit: std::time::Duration,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> T {
        let (done, outcome) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = done.send(work());
        });
        match outcome.recv_timeout(limit) {
            Ok(value) => value,
            Err(mpsc::RecvTimeoutError::Timeout) => panic!("no result within {limit:?}"),
            // The work panicked: surface that panic, not a closed channel.
            Err(mpsc::RecvTimeoutError::Disconnected) => match worker.join() {
                Err(panic) => std::panic::resume_unwind(panic),
                Ok(()) => unreachable!("the work returned without a result"),
            },
        }
    }

    /// Each wait inside the scenario is bounded on its own; this bounds it whole.
    const SCENARIO_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

    /// One job panics with a chat, an extraction and a projection queued
    /// behind it, and another chat arrives afterwards.
    fn run_job_panic_scenario() {
        let wait = std::time::Duration::from_secs(10);
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let executor = LlmExecutor::with_runner(panicking_runner(started_tx, release_rx));

        let in_flight = chat_caller(&executor, "panic");
        started
            .recv_timeout(wait)
            .expect("the job that will panic starts");

        let queued_chat = chat_caller(&executor, "queued");
        let queued_extraction = spawn_caller({
            let executor = executor.clone();
            move || {
                executor
                    .extract_entities(
                        "text".to_string(),
                        "speaker".to_string(),
                        String::new(),
                        LlmProvider::LocalLlama,
                        LlmPriority::Background,
                    )
                    .is_some()
            }
        });
        let queued_patch = spawn_caller({
            let executor = executor.clone();
            move || {
                let (job, ledger) = projection_test_job(ProjectionKind::Notes);
                executor
                    .generate_projection_patch(
                        job,
                        ledger,
                        None::<crate::projections::MaterializedNotes>,
                        LlmProvider::LocalLlama,
                        1,
                        100,
                    )
                    .outcome
                    .map(|_| ())
            }
        });
        let deadline = std::time::Instant::now() + wait;
        loop {
            let queued = {
                let state = executor.queue.0.lock().unwrap();
                state.interactive.len() + state.background.len()
            };
            if queued == 3 {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the three callers must be queued behind the running job"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }

        release.send(()).expect("release the job into its panic");

        let internal = Err(executor_failure(Code::Internal));
        assert_eq!(
            in_flight.recv_timeout(wait).expect("the in-flight caller"),
            internal
        );
        assert_eq!(
            queued_chat.recv_timeout(wait).expect("the queued chat"),
            internal
        );
        assert!(
            !queued_extraction
                .recv_timeout(wait)
                .expect("the queued extraction"),
            "a queued extraction must fall back, not be served"
        );
        assert_eq!(
            queued_patch
                .recv_timeout(wait)
                .expect("the queued projection"),
            Err("runtime/internal".to_string())
        );

        // The worker that ran the panicking job is gone. This one is served,
        // so the call started another.
        assert_eq!(
            chat_caller(&executor, "later")
                .recv_timeout(wait)
                .expect("a later caller"),
            Ok("served".to_string())
        );
        assert_eq!(executor.queue.0.lock().unwrap().worker, Worker::Running);
    }

    #[test]
    fn a_job_panic_fails_its_caller_and_the_queue_and_the_next_job_is_served() {
        within(SCENARIO_LIMIT, run_job_panic_scenario);
    }

    const JOB_PANIC_CHILD: &str = "AUDIOGRAPH_TEST_LLM_EXECUTOR_PANIC";

    #[test]
    #[ignore = "invoked in isolation by a_job_panic_payload_reaches_no_log_or_crash_report"]
    fn job_panic_child() {
        use crate::test_log_capture::{assert_process_logs_exclude, install, process_records};

        if std::env::var_os(JOB_PANIC_CHILD).is_none() {
            return;
        }
        crate::crash_handler::install();
        install();
        within(SCENARIO_LIMIT, run_job_panic_scenario);

        // This process ran nothing else, so the whole log is this scenario's.
        let logged: Vec<String> = process_records()
            .into_iter()
            .map(|record| record.message)
            .collect();
        assert!(
            logged.iter().any(|line| line
                == "LLM executor job panicked; retiring the worker and failing queued jobs"),
            "{logged:?}"
        );
        // The queued extraction says why it fell back, in its own words: not
        // the line a job dropped for any other reason logs.
        assert!(
            logged.iter().any(|line| line
                == "LLM extraction falling back: the executor worker ended on a job's panic"),
            "{logged:?}"
        );
        assert!(
            !logged
                .iter()
                .any(|line| line.starts_with("LLM executor extraction response failed")),
            "{logged:?}"
        );
        assert_process_logs_exclude(&[JOB_PANIC_CANARY]);
    }

    /// Run `command` to its end and collect its output, killing it at `limit`.
    fn output_within(
        mut command: std::process::Command,
        limit: std::time::Duration,
    ) -> std::process::Output {
        use std::io::Read;
        use std::process::Stdio;

        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the child test");
        // Drain both pipes while waiting: a full pipe would stall the child.
        let mut stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let stdout = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.read_to_end(&mut bytes);
            bytes
        });
        let stderr = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });
        let deadline = std::time::Instant::now() + limit;
        let status = loop {
            if let Some(status) = child.try_wait().expect("poll the child test") {
                break status;
            }
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("the child test did not finish within {limit:?}");
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        std::process::Output {
            status,
            stdout: stdout.join().expect("read the child's stdout"),
            stderr: stderr.join().expect("read the child's stderr"),
        }
    }

    #[test]
    fn a_job_panic_payload_reaches_no_log_or_crash_report() {
        let root =
            std::env::temp_dir().join(format!("audiograph-llm-executor-{}", uuid::Uuid::new_v4()));
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "llm::executor::tests::job_panic_child",
                "--ignored",
                "--nocapture",
            ])
            .env(crate::user_data::DATA_DIR_ENV, &root)
            .env(JOB_PANIC_CHILD, "1")
            .env("RUST_BACKTRACE", "0");
        // Twice the scenario's own bound, so a stuck scenario is reported by
        // the child and only a stuck child is killed here.
        let output = output_within(child, SCENARIO_LIMIT * 2);
        assert!(output.status.success(), "child status {}", output.status);
        let reports: Vec<_> = std::fs::read_dir(root.join("crashes"))
            .unwrap()
            .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
            .collect();
        std::fs::remove_dir_all(root).unwrap();

        // The hook ran once, and recorded the operation and nothing else.
        assert_eq!(
            reports,
            ["AudioGraph crash report\n\nPayload:\n[redacted panic payload: llm.executor_job]\n"]
        );
        let printed = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !printed.contains(JOB_PANIC_CANARY),
            "panic payload in child output"
        );
    }

    /// Poison `slot` the way a backend call that panics under its lock does.
    fn poison<T: Send + 'static>(slot: &Arc<Mutex<T>>) {
        let held = slot.clone();
        let _ = std::thread::spawn(move || {
            let _guard = held.lock().unwrap();
            panic!("poison the slot");
        })
        .join();
        assert!(slot.is_poisoned());
    }

    #[test]
    fn extraction_refuses_a_poisoned_backend_slot() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        // An endpoint that would answer, so a request would be counted.
        let (base, requests) = rt.block_on(spawn_counting_mock(vec![
            "{}".to_string(),
            "{}".to_string(),
        ]));
        let allow = crate::asr::ProviderContentEgressPolicy::allow();

        // Without the native engines the slots can hold their stand-ins, whose
        // calls would log a failure line of their own.
        #[cfg(not(feature = "llm-llama"))]
        let llm_engine = Some(LlmEngine);
        #[cfg(feature = "llm-llama")]
        let llm_engine = None;
        #[cfg(not(feature = "llm-mistralrs"))]
        let mistralrs_engine = Some(MistralRsEngine);
        #[cfg(feature = "llm-mistralrs")]
        let mistralrs_engine = None;

        let handles = BackendHandles {
            llm_engine: Arc::new(Mutex::new(llm_engine)),
            api_client: Arc::new(Mutex::new(Some(
                ApiClient::new(crate::llm::ApiConfig {
                    endpoint: base.clone(),
                    api_key: Some("sk-route-removal-probe".to_string()),
                    model: "probe-model".to_string(),
                    max_tokens: 64,
                    temperature: 0.1,
                })
                .with_content_egress_policy(allow),
            ))),
            openrouter_client: Arc::new(Mutex::new(Some(
                OpenRouterClient::new(crate::llm::openrouter::OpenRouterConfig {
                    base_url: base.clone(),
                    ..crate::llm::openrouter::OpenRouterConfig::with_defaults(
                        "sk-route-removal-probe".to_string(),
                        "probe-model".to_string(),
                    )
                })
                .with_content_egress_policy(allow),
            ))),
            mistralrs_engine: Arc::new(Mutex::new(mistralrs_engine)),
        };
        poison(&handles.llm_engine);
        poison(&handles.api_client);
        poison(&handles.openrouter_client);
        poison(&handles.mistralrs_engine);

        let api_route = authorize_route_dispatch(&LlmProvider::Api {
            endpoint: base.clone(),
            api_key: "sk-route-removal-probe".to_string(),
            model: "probe-model".to_string(),
        })
        .expect("the loopback endpoint is authorized");
        let openrouter_route = authorize_route_dispatch(&LlmProvider::OpenRouter {
            model: "probe-model".to_string(),
            base_url: base,
            provider_order: None,
            include_usage_in_stream: true,
            api_key: "sk-route-removal-probe".to_string(),
        })
        .expect("OpenRouter is authorized");

        let capture = ThreadLogCapture::start();
        assert!(extract_native(&handles, "text", "speaker").is_none());
        assert!(extract_api(&handles, &api_route, "text", "speaker", "").is_none());
        assert!(extract_openrouter(&handles, &openrouter_route, "text", "speaker", "").is_none());
        assert!(extract_mistralrs(&handles, "text", "speaker").is_none());
        let logs = render(&capture.finish());

        assert_eq!(
            logs.matches("LLM extraction falling back: the backend slot is poisoned")
                .count(),
            4,
            "{logs}"
        );
        // No backend was called: none logged a failure of its own, and nothing
        // reached the endpoint.
        assert!(!logs.contains("extraction failed"), "{logs}");
        assert_eq!(requests.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

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
