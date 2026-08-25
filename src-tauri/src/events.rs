//! Tauri event name constants and payload types.
//!
//! These constants define the event names emitted from the Rust backend
//! to the frontend. The frontend subscribes using `listen()` from `@tauri-apps/api`.

/// Event emitted when a new transcript segment is available.
pub const TRANSCRIPT_UPDATE: &str = "transcript-update";

/// Event emitted when a streaming ASR provider produces an interim hypothesis.
pub const ASR_PARTIAL: &str = "asr-partial";

/// Event emitted when a provider/local ASR path produces a transcript-span
/// revision. This is the normalized, provider-neutral event intended for the
/// event-sourced transcript/notes/graph pipeline. It is emitted alongside the
/// legacy `asr-partial` and `transcript-update` events while the UI migrates.
pub const ASR_SPAN_REVISION: &str = "asr-span-revision";

/// Event emitted when provider or local diarization revises a speaker timeline
/// span. This keeps speaker attribution diffable instead of forcing consumers
/// to infer timeline edits from append-only transcript rows.
pub const DIARIZATION_SPAN_REVISION: &str = "diarization-span-revision";

/// Event emitted when a provider or local fallback identifies speech turn
/// lifecycle boundaries. This is intentionally separate from transcript
/// events: graph/notes can use conservative final boundaries while the
/// speech-to-speech agent can react to eager/cancel/resume signals.
pub const TURN_EVENT: &str = "turn-event";

/// Event emitted when the knowledge graph changes (full snapshot).
///
/// Contract with [`GRAPH_DELTA`]: snapshots are authoritative resync points.
/// Within one backend graph mutation we emit the delta first, then the snapshot
/// if a full refresh is due. The frontend may apply deltas for low-latency
/// updates, but any snapshot replaces the current graph state and therefore
/// supersedes every delta produced by earlier graph mutations. Snapshot
/// receivers should not try to merge a snapshot into stale local graph data,
/// except for view-only fields such as force-layout node positions.
///
/// Emitted less frequently from streaming extraction and immediately after
/// explicit graph actions that need deterministic UI confirmation.
pub const GRAPH_UPDATE: &str = "graph-update";

/// Event emitted with incremental graph changes (delta updates).
///
/// Deltas are ordered best-effort updates between [`GRAPH_UPDATE`] snapshots.
/// They are generated from the graph's in-memory change buffer and cleared by
/// `take_delta()`. A receiver that misses a delta should rely on the next full
/// snapshot to recover; deltas must not be replayed after a newer snapshot has
/// been applied unless a future sequence/basis field proves they belong after
/// that snapshot.
///
/// Emitted on every graph mutation when the mutation changed nodes or edges.
pub const GRAPH_DELTA: &str = "graph-delta";

/// Event emitted after an accepted transcript-derived projection patch has
/// passed runtime validation, persistence, and materializer application.
pub const PROJECTION_PATCH: &str = "projection-patch";

/// Event emitted after a notes projection patch updates the materialized notes
/// artifact for the active session.
pub const MATERIALIZED_NOTES_UPDATE: &str = "materialized-notes-update";

/// Event emitted after a graph projection patch updates the materialized graph
/// artifact for the active session.
pub const MATERIALIZED_GRAPH_UPDATE: &str = "materialized-graph-update";

/// Event emitted periodically (every ~2s) or on status change.
pub const PIPELINE_STATUS_EVENT: &str = "pipeline-status";

/// Event emitted when a pipeline stage completes work and can report elapsed
/// wall-clock time. Kept separate from [`PIPELINE_STATUS_EVENT`] so latency
/// instrumentation can be added incrementally without changing the status
/// enum's serialization shape.
pub const PIPELINE_LATENCY: &str = "pipeline-latency";

/// Event emitted when the agent/react loop changes state.
pub const AGENT_STATUS: &str = "agent-status";

/// Event emitted when the agent/react loop proposes an action or note for
/// the user to inspect. Proposals stay advisory until the user approves them.
pub const AGENT_PROPOSAL: &str = "agent-proposal";

/// Event emitted when a new speaker is first identified.
pub const SPEAKER_DETECTED: &str = "speaker-detected";

/// Event emitted when a capture error occurs.
pub const CAPTURE_ERROR: &str = "capture-error";

/// Event emitted when a persistence write fails because the underlying storage
/// is full (ENOSPC / ERROR_DISK_FULL). The frontend should surface this as a
/// user-visible error so the operator can free disk space before more
/// transcript/graph data is lost.
pub const CAPTURE_STORAGE_FULL: &str = "capture-storage-full";

/// Event emitted when a persistence event-writer queue starts or stops dropping
/// events because the storage sink is not keeping up. Payload intentionally
/// omits session ids, file paths, transcript text, and provider payloads.
pub const PERSISTENCE_QUEUE_BACKPRESSURE: &str = "persistence-queue-backpressure";

/// Event emitted when the backpressure state of a capture source changes —
/// i.e. the rsac ring buffer has started or stopped dropping buffers because
/// the consumer (this app's pipeline) isn't keeping up. Edge-triggered: fires
/// only on transitions (false→true or true→false), not continuously.
pub const CAPTURE_BACKPRESSURE: &str = "capture-backpressure";

/// Event emitted by the processed-audio dispatcher with per-consumer queue
/// health and drop counters.
/// Payload: [`crate::audio::consumer::ProcessedAudioConsumerHealthPayload`].
pub const AUDIO_CONSUMER_HEALTH: &str = "audio-consumer-health";

/// Event emitted when Gemini Live produces a transcription.
pub const GEMINI_TRANSCRIPTION: &str = "gemini-transcription";

/// Event emitted when Gemini Live produces a model response.
pub const GEMINI_RESPONSE: &str = "gemini-response";

/// Event emitted when the Gemini Live connection status changes.
pub const GEMINI_STATUS: &str = "gemini-status";

/// Event emitted with the OpenAI Realtime S2S assistant's spoken-reply
/// transcript (the S2S voice agent, parallel to `GEMINI_RESPONSE`).
pub const OPENAI_REALTIME_RESPONSE: &str = "openai-realtime-response";

/// Event emitted when the OpenAI Realtime S2S connection status changes
/// (connected / disconnected / reconnecting / reconnected / error — same
/// envelope shape the frontend already routes for `GEMINI_STATUS`).
pub const OPENAI_REALTIME_STATUS: &str = "openai-realtime-status";

/// Event emitted throughout a model download with elapsed + byte counters so
/// the frontend can compute an ETA. Throttled to roughly 1 Hz; also fires once
/// on completion or error.
pub const MODEL_DOWNLOAD_PROGRESS: &str = "model-download-progress";

/// Event emitted when an AWS call (Transcribe streaming, STS preflight) fails
/// with a credential- or region-class error that the frontend should surface
/// via a localized toast with recovery guidance (ag#13).
pub const AWS_ERROR: &str = "aws-error";

// The streaming-chat token deltas + terminal frame (previously the
// `chat-token-delta` / `chat-token-done` events, plan A3 / ADR-0006) moved off
// the event system onto a per-invocation `tauri::ipc::Channel<ChatStreamEvent>`
// returned by `start_streaming_chat` (audio-graph-1534) — the event system is
// not designed for the 20-100+/sec per-token throughput. See
// `crate::llm::streaming::ChatStreamEvent`.

/// Event emitted after a chat/LLM completion's provider-reported token usage
/// has been persisted to the session usage file.
/// Payload: [`LlmUsageUpdatePayload`].
pub const LLM_USAGE_UPDATE: &str = "llm-usage-update";

/// Event emitted when runtime privacy policy blocks a content-bearing provider
/// call before audio, transcript, graph context, prompts, or generated text can
/// leave the process.
pub const PRIVACY_POLICY_BLOCKED: &str = "privacy-policy-blocked";

/// Event emitted when the global capture shortcut (Cmd/Ctrl+Shift+R) fires,
/// even when the window is unfocused (audio-graph-f67e). The frontend listens
/// for this and routes it through the SAME store toggle path the UI Start/Stop
/// button uses (`startCapture` / `stopCapture`) — no parallel capture logic in
/// Rust — so the existing no-source-selected notification still surfaces.
pub const GLOBAL_SHORTCUT_TOGGLE_CAPTURE: &str = "global-shortcut-toggle-capture";

/// Event emitted when the tray *Stop capture* menu item is clicked
/// (audio-graph-a156). Single source of truth for the event name — the tray
/// menu handler in [`crate::tray`] emits this constant directly. Routed through
/// the store `stopCapture` path (same as the UI Stop button).
pub const TRAY_STOP_CAPTURE: &str = "tray-stop-capture";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LlmUsageUpdatePayload {
    pub session_id: String,
    pub total_tokens: u64,
    pub session_llm_total: u64,
    pub session_llm_turns: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PrivacyPolicyBlockedPayload {
    pub session_id: Option<String>,
    pub privacy_mode: String,
    pub action: String,
    pub provider: String,
    pub data_classes: Vec<String>,
    pub reason: String,
    pub timestamp_ms: u64,
}

/// Status of an individual pipeline stage.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum StageStatus {
    #[default]
    Idle,
    Running {
        processed_count: u64,
    },
    Error {
        message: String,
    },
    /// The stage is running, but not with the backend/fidelity the user's
    /// configuration asked for — e.g. diarization silently fell back from a
    /// missing/invalid neural model asset to the crude Simple heuristic
    /// (audio-graph-586b: `DiarizationSettings.mode` must never be silently
    /// overridden with no notice).
    ///
    /// `reason` is a stable, `snake_case` degradation-class code (e.g.
    /// `"asset_not_downloaded"` — see `speech::DiarizationDegradationReason`,
    /// the sole producer), NOT free-text prose: it is never transcript
    /// content, provider error bodies, or anything else session-content-
    /// bearing, and — review follow-up (audio-graph-586b) — it is also no
    /// longer hardcoded English composed in Rust. The frontend looks the code
    /// up against its OWN fully translated string table
    /// (`pipeline.diarizationDegradedReason.<code>`, matching this crate's
    /// existing typed+translated `SttFidelityDegradation` vocabulary
    /// pattern) rather than rendering Rust-composed prose verbatim, so a
    /// non-English UI gets a real translation instead of an English
    /// sentence bolted onto a translated title.
    Degraded {
        reason: String,
    },
}

/// Overall pipeline status, combining all stages.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PipelineStatus {
    pub capture: StageStatus,
    pub pipeline: StageStatus,
    pub asr: StageStatus,
    pub diarization: StageStatus,
    pub entity_extraction: StageStatus,
    pub graph: StageStatus,
}

/// Interim ASR hypothesis from streaming providers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AsrPartialPayload {
    pub provider: String,
    pub source_id: String,
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub confidence: f32,
    pub timestamp_ms: u64,
}

/// Stability/finality state for a normalized ASR span revision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AsrSpanStability {
    Partial,
    Final,
}

/// Provider-neutral transcript span revision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AsrSpanRevisionPayload {
    /// Stable span id within the provider/source stream when known. Legacy
    /// paths use a deterministic time-based id for partials and the transcript
    /// segment id for final-only paths until provider adapters can supply
    /// stronger identities.
    pub span_id: String,
    pub provider: String,
    pub source_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_segment_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub confidence: f32,
    pub is_final: bool,
    pub stability: AsrSpanStability,
    /// Monotonic within a span once provider adapters can preserve provider
    /// item identity. First additive slice uses 1 for the emitted revision.
    pub revision_number: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub end_of_turn: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_event_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_latency_ms: Option<u64>,
    pub received_at_ms: u64,
}

/// Stability/finality state for a provider-neutral diarization span revision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiarizationSpanStability {
    /// A rolling/local or streaming-provider attribution that may be remapped.
    Provisional,
    /// The span has survived a stabilization window but can still be retconned
    /// by later full-session/provider revisions.
    Stable,
    /// The provider or offline reconciliation considers this span complete.
    Final,
}

/// Provider-neutral speaker-timeline span revision.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiarizationSpanRevisionPayload {
    /// Stable id for the logical speaker span being revised.
    pub span_id: String,
    /// Provider/engine that produced the attribution, e.g. `deepgram`,
    /// `aws_transcribe`, `soniox`, or `local_clustering`.
    pub provider: String,
    /// Logical timeline being revised. Provider diarization may use a source id;
    /// session-level local diarization can use `session`.
    pub timeline_id: String,
    /// Capture source when the attribution is source-local. Session-level local
    /// diarization may leave this unset until multichannel source attribution is
    /// wired.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    pub start_time: f64,
    pub end_time: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    pub is_final: bool,
    pub stability: DiarizationSpanStability,
    pub revision_number: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    pub basis_asr_span_ids: Vec<String>,
    pub basis_transcript_segment_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_event_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_latency_ms: Option<u64>,
    pub received_at_ms: u64,
}

/// Normalized turn lifecycle event kind shared by cloud and local providers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEventKind {
    SpeechStarted,
    SpeechFinal,
    UtteranceEnd,
    EagerEndOfTurn,
    EndOfTurn,
    TurnResumed,
    LocalWindow,
}

/// Provider-neutral turn lifecycle payload.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TurnEventPayload {
    pub provider: String,
    pub source_id: String,
    pub kind: TurnEventKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_time: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_index: Option<u64>,
    pub timestamp_ms: u64,
}

/// Per-stage latency sample emitted by backend workers.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PipelineLatencyPayload {
    /// Stage key matching the frontend `PipelineStatus` keys where possible:
    /// `capture`, `pipeline`, `asr`, `diarization`, `entity_extraction`,
    /// `graph`, or a future extension such as `agent`.
    pub stage: String,
    /// Optional source id when the timing belongs to a capture/source path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Optional transcript/audio segment id when the timing belongs to a
    /// logical speech segment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segment_id: Option<String>,
    /// Wall-clock duration for the just-completed stage.
    pub latency_ms: f64,
    /// Unix timestamp in milliseconds when the sample was emitted.
    pub timestamp_ms: u64,
}

/// Agent/react loop status state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatusState {
    Idle,
    Running,
    Error,
}

/// Status update for the agent/react loop.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AgentStatusPayload {
    pub state: AgentStatusState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_segment_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub timestamp_ms: u64,
}

/// Kind of advisory agent proposal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentProposalKind {
    Note,
    Question,
    GraphSuggestion,
}

/// Backend-computed quality grade for a mint-time [`AgentProposalKind::Question`]
/// proposal (audio-graph-83cc, T2). Ports ticket W9's shipped
/// `classifyQueueEntry` constants/logic
/// (`src/components/workspace/agentQueue.ts`) into Rust so the auto-answer
/// spend gate (T3) has a Rust-authoritative signal instead of trusting the
/// renderer's own W9 classification alone — the exact shape the ratified
/// agent-runtime verdict rejected in option D2 ("Rust validates what the
/// renderer claims").
///
/// `Strong` is W9's `"actionable"` outcome — either a non-question kind
/// (W9's quality rules never touch `note`/`graph_suggestion` at all) or a
/// question that clears the confidence floor and the content-shape check.
/// `Weak` and `Fragment` both correspond to W9's single, undifferentiated
/// `"fragment_suspect"` outcome, split here for observability into WHY:
/// `Weak` is the confidence-floor rule, `Fragment` is the content-shape rule
/// (104f's actual fragment-minting bug — a truncated utterance
/// misclassified as a question). This split does not exist on the
/// frontend, so the one invariant the mirrored fixtures pin is:
/// `grade == Strong` if and only if the frontend's `classifyQueueEntry(...,
/// true)` returns `"actionable"` for the same proposal. See
/// `agent_signal_grade` (`speech/mod.rs`) for the port and
/// `speech::mod::tests` for the 1:1 mirrored fixtures.
///
/// **Named, deliberate gap (fix-round finding, scope-honesty review):** this
/// grade mirrors `classifyQueueEntry` only, not the frontend's actual queue
/// *admission* path. `selectAgentQueue` (`agentQueue.ts`) runs a kind-agnostic
/// duplicate-collapse pass BEFORE `classifyQueueEntry` — a card whose
/// normalized `queueContentText` matches a newer surviving card is rejected
/// at admission regardless of what `classifyQueueEntry` would have said.
/// That collapse is NOT ported here: a verbatim-repeated question grades
/// `Strong` in Rust even though the frontend would refuse to admit it. This
/// is harmless while ratified gate Q2 requires BOTH the Rust grade AND the
/// frontend Signal admit (the frontend's rejection still wins), but it stops
/// being harmless if Q2 is ever collapsed to "Rust-alone" — at that point
/// each duplicate question would buy its own paid dispatch, bounded only by
/// the per-session auto-answer cap. Whoever performs that collapse MUST
/// either port duplicate-collapse into this grade first or add an equivalent
/// check elsewhere in the Rust spend path.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignalGrade {
    Strong,
    Weak,
    Fragment,
}

/// Advisory proposal emitted by the agent/react loop.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct AgentProposalPayload {
    pub id: String,
    pub source_segment_id: String,
    pub source_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_label: Option<String>,
    pub kind: AgentProposalKind,
    pub title: String,
    pub body: String,
    pub confidence: f32,
    pub created_at_ms: u64,
    /// Backend signal grade at mint (audio-graph-83cc, T2). `None` for
    /// every payload that predates this unit (nothing emitted one before
    /// it) and for any hand-built payload that never went through the one
    /// production mint site, `run_agent_proposal_task` (`speech/mod.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<SignalGrade>,
}

/// Result returned after the user approves an agent proposal.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct AgentActionResult {
    pub proposal_id: String,
    pub action: String,
    pub message: String,
    pub graph_updated: bool,
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LiveAssistCardStatus {
    Pending,
    Approved,
    Dismissed,
}

/// Who authored a live-assist card's underlying question (audio-graph-83cc,
/// T1). `Transcript` is a question detected from the meeting itself — the
/// only origin that has ever existed in production
/// (`run_agent_proposal_task`, `speech/mod.rs`) — and is therefore the
/// `#[serde(default)]` value every pre-83cc record deserializes to. `User`
/// is a free-form chatbox question the user typed (`ask_question_card`,
/// T3's command — not introduced by this unit; this enum and the
/// validator's Q4 narrow widening below exist now so the schema is built
/// and pinned ahead of it).
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CardOrigin {
    Transcript,
    User,
}

/// Terminal (or interrupted) state of a [`CardAnswer`] (audio-graph-83cc,
/// T1). `Answered` and `Failed` are self-explanatory; `Interrupted` is a
/// user-preempted auto-answer (T3's single-flight gate cancels the
/// in-progress stream of a *user* action, never the reverse) whose partial
/// text is still recorded, not discarded.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CardAnswerStatus {
    Answered,
    Failed,
    Interrupted,
}

/// Hard cap on [`CardAnswer::text`]'s length, enforced by
/// `persistence::validate_live_assist_card` (audio-graph-83cc T1,
/// size-margin gate — see `commands::tests` for the worst-case-card ×
/// worst-case-count computation against `MAX_LIVE_ASSIST_CARDS_BYTES`).
///
/// Angle-a's design deliberately leaves `CardAnswer.text` uncapped (it is
/// the FULL answer rendered under the card, not a preview — a rejected
/// sibling design's separate `answer_preview` field, capped at 240 chars,
/// is a different field for a different storage shape and does not apply
/// here). Left fully uncapped, though, this field's worst case is bounded
/// only by whatever `max_tokens` the active LLM provider config allows —
/// which this crate's own settings permit configuring up to 262,144 for at
/// least one provider (`llm/api_client.rs`'s Cerebras clamp test) — so a
/// misconfigured or adversarial provider response could still make this
/// field the dominant term in the live-assist ceiling math. 4,000 UTF-8
/// characters is generous for a live-assist answer (several paragraphs)
/// while keeping the worst-case card × worst-case count total a small,
/// provable multiple of `MAX_LIVE_ASSIST_CARDS_BYTES`'s 8 MiB — see
/// `commands::tests`' size-margin gate for the measured figure. (Fix-round
/// correction: an earlier draft of this comment attributed "55 KB" to this
/// field's own observed pathological size. That number is actually the
/// *whole pre-answer-field* `live_assist` artifact's observed pathological
/// size, per the ratified synthesis's deletion-test
/// (`docs/agentic-runs/2026-08-24-83cc-design-panel/synthesis-ticket-cut.md`
/// §1) — this field has never existed in production and so has no observed
/// size of its own.)
///
/// A validator rejection at this cap (`persistence::validate_live_assist_card`)
/// is a TERMINAL failure for whoever calls `upsert_live_assist_card` — it
/// does not truncate, so a caller that lets a raw, uncapped answer hit the
/// validator risks losing the whole answer, which is exactly the field
/// failure this epic exists to kill (see the synthesis's "the field failure
/// is answers being lost" framing, same section). `CardAnswer::cap_text`
/// exists so a future writer (T3) can truncate BEFORE validation instead of
/// discovering the cap as a write failure.
pub const MAX_CARD_ANSWER_TEXT_CHARS: usize = 4_000;

/// The threaded answer to a live-assist card's question (audio-graph-83cc,
/// T1, angle-a §2.1). Written ONCE, on the answer stream's terminal frame
/// (T3's job — this unit only defines the shape and its
/// persistence/validation contract). A Grounded Inference in CONTEXT.md
/// terms: `evidence_span_ids` / `evidence_graph_ids` / `notes_last_sequence`
/// are its Inference Chain, `route_id` is the trusted-code-resolved route
/// that served it, and it lives on this Session Artifact, never in the
/// temporal graph — ADR-0013: no new graph write path is introduced by this
/// type.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct CardAnswer {
    pub status: CardAnswerStatus,
    /// Full answer text from the stream's terminal frame. Non-empty unless
    /// `status == Failed`, and capped at [`MAX_CARD_ANSWER_TEXT_CHARS`]
    /// (both validator-enforced, `persistence::validate_live_assist_card`).
    /// Prefer constructing via [`CardAnswer::cap_text`] over building this
    /// field directly so an over-cap answer is truncated-and-marked instead
    /// of rejected-and-lost at persistence time.
    pub text: String,
    /// `true` when [`CardAnswer::cap_text`] cut `text` down to
    /// [`MAX_CARD_ANSWER_TEXT_CHARS`] characters. `#[serde(default)]` (not
    /// `Option`) because "not truncated" is itself meaningful information,
    /// not an absence — every pre-truncation-support answer defaults to
    /// `false`, which is correct (nothing truncated it).
    #[serde(default)]
    pub truncated: bool,
    /// Transcript-window span ids that fed retrieval.
    #[serde(default)]
    pub evidence_span_ids: Vec<String>,
    /// `build_graph_chat_context` node ids that fed retrieval.
    #[serde(default)]
    pub evidence_graph_ids: Vec<String>,
    /// Notes basis sequence, when notes were used to answer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes_last_sequence: Option<u64>,
    /// ADR-0038: resolved by trusted code (`resolve_route`), never model- or
    /// config-echoed. Required non-empty (validator-enforced) regardless of
    /// `status` — route resolution happens before dispatch, so even a
    /// `Failed` answer carries the route that was attempted.
    pub route_id: String,
    pub requested_by: CardOrigin,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// `None` when the provider omitted usage (the existing
    /// `persist_llm_usage_for_session` early-return, `commands.rs`) — never
    /// a fabricated zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_tokens: Option<u64>,
    pub answered_at_ms: u64,
}

impl CardAnswer {
    /// Truncates `text` to [`MAX_CARD_ANSWER_TEXT_CHARS`] Unicode scalar
    /// values (matching the validator's `chars().count()` measure exactly)
    /// and sets `truncated = true` when truncation actually happened.
    /// Returns whether it truncated.
    ///
    /// Fix-round addition (scope-honesty review, major): the validator
    /// rejects any answer over the cap outright, with no truncation seam —
    /// a caller that skips this method and writes a raw, uncapped answer
    /// straight to `upsert_live_assist_card` loses the whole answer on a
    /// validator error, not just the excess. Call this BEFORE persisting an
    /// answer built from unbounded model output.
    ///
    /// Truncates at a `char` boundary (never splits a multi-byte UTF-8
    /// sequence or a surrogate pair), so the result is always valid UTF-8.
    pub fn cap_text(&mut self) -> bool {
        if self.text.chars().count() <= MAX_CARD_ANSWER_TEXT_CHARS {
            return false;
        }
        self.text = self.text.chars().take(MAX_CARD_ANSWER_TEXT_CHARS).collect();
        self.truncated = true;
        true
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct LiveAssistCardRecord {
    pub session_id: String,
    pub proposal: AgentProposalPayload,
    pub status: LiveAssistCardStatus,
    #[serde(default)]
    pub source_span_ids: Vec<String>,
    #[serde(default)]
    pub graph_context_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<AgentActionResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection_patch_sequence: Option<u64>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    /// Who authored the underlying question. Absent ⇒ `Transcript` (every
    /// pre-83cc record) — audio-graph-83cc T1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<CardOrigin>,
    /// The threaded answer, written once on terminal (T3). Absent for
    /// every unanswered card and for every pre-83cc record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<CardAnswer>,
    /// Backend signal grade at mint (T2), mirrored from `proposal.signal`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signal: Option<SignalGrade>,
}

/// Payload for capture error events.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaptureErrorPayload {
    pub source_id: String,
    pub error: String,
    pub recoverable: bool,
}

/// Payload for `CAPTURE_STORAGE_FULL` events.
///
/// Emitted when a persistence write fails because the underlying storage is
/// full (ENOSPC / ERROR_DISK_FULL). Use the `bytes_lost` field to tell the
/// user how much data failed to hit disk on this attempt; `bytes_written`
/// is best-effort and is `0` when the error happens on the initial open.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaptureStorageFullPayload {
    /// Absolute path the app tried to write to.
    pub path: String,
    /// Bytes successfully written before the error (best-effort).
    pub bytes_written: u64,
    /// Bytes the app was trying to write when the error occurred (best-effort:
    /// the size of the buffer we were attempting to persist).
    pub bytes_lost: u64,
}

/// Payload for persistence queue pressure transitions.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PersistenceQueueBackpressurePayload {
    /// Stable writer identifier, e.g. `transcript_event` or `projection_event`.
    pub writer: String,
    /// `true` after the writer starts dropping new events because the queue is
    /// full; `false` after a later enqueue succeeds.
    pub is_backpressured: bool,
    /// Configured queue capacity for this writer.
    pub queue_capacity: usize,
    /// Cumulative count of dropped events in this process for the writer handle.
    pub dropped_count: u64,
}

/// Payload for capture-backpressure state-change events.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CaptureBackpressurePayload {
    pub source_id: String,
    /// `true` when the ring buffer has started dropping; `false` when recovery
    /// is detected. The frontend should surface this as a transient warning
    /// (e.g. a pill badge) rather than a fatal error.
    pub is_backpressured: bool,
}

/// Payload for `AWS_ERROR` events (ag#13).
///
/// `error` carries the structured classification (a [`crate::aws_util::UiAwsError`]
/// serialized with `category` / payload fields). `raw_message` is the original aws-sdk
/// error string, kept so the frontend can log or disclose details when the
/// category alone isn't enough (e.g. unexpected `Unknown` bucket).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AwsErrorPayload {
    pub error: crate::aws_util::UiAwsError,
    pub raw_message: String,
}

/// Emit a Tauri event and log any emission failure at `error` level.
///
/// The default `let _ = app.emit(...)` pattern silently swallows emission
/// errors, which makes failed frontend notifications undebuggable. Use this
/// helper instead so failures surface in logs.
pub fn emit_or_log<P>(app: &tauri::AppHandle, event: &str, payload: P)
where
    P: serde::Serialize + Clone,
{
    use tauri::Emitter;
    if let Err(e) = app.emit(event, payload) {
        log::error!("Failed to emit event '{}': {}", event, e);
    }
}

/// Heuristic classifier for capture errors into recoverable vs fatal.
///
/// Used at capture-error emit sites to populate `CaptureErrorPayload.recoverable`.
/// Fatal errors indicate the source cannot be used again without user action
/// (permission, device disconnection). Recoverable errors may succeed on retry.
pub fn classify_capture_error(err: &str) -> bool {
    let lower = err.to_lowercase();
    let fatal_markers = [
        "permission denied",
        "not permitted",
        "unauthorized",
        "disconnected",
        "device not found",
        "no such device",
        "device removed",
        "access denied",
        "not supported",
        "unsupported",
    ];
    if fatal_markers.iter().any(|m| lower.contains(m)) {
        return false;
    }
    // Default to recoverable for unclassified errors — user can retry.
    true
}

/// Returns `true` if this I/O error indicates the underlying storage is full
/// (ENOSPC on Unix, ERROR_DISK_FULL on Windows).
///
/// `std::io::ErrorKind::StorageFull` was stabilised relatively recently and
/// the mapping from raw OS codes into that kind still varies across Rust
/// versions and platforms, so we check both the kind and the `raw_os_error`
/// signatures defensively — whichever trips first wins.
pub fn is_storage_full(err: &std::io::Error) -> bool {
    // Prefer the symbolic kind when available; fall through to raw_os_error
    // if the current toolchain doesn't map the error to `StorageFull` yet.
    if err.kind() == std::io::ErrorKind::StorageFull {
        return true;
    }
    matches!(err.raw_os_error(), Some(28) | Some(112))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_storage_full_detects_enospc() {
        // ENOSPC is 28 on Linux and macOS.
        let err = std::io::Error::from_raw_os_error(28);
        assert!(is_storage_full(&err));
    }

    #[test]
    fn is_storage_full_detects_windows_disk_full() {
        // ERROR_DISK_FULL is 112 on Windows.
        let err = std::io::Error::from_raw_os_error(112);
        assert!(is_storage_full(&err));
    }

    #[test]
    fn is_storage_full_ignores_unrelated_errors() {
        // EACCES / generic errors must not be misclassified as storage-full.
        let err = std::io::Error::from_raw_os_error(13);
        assert!(!is_storage_full(&err));

        let other = std::io::Error::other("boom");
        assert!(!is_storage_full(&other));
    }

    #[test]
    fn asr_span_revision_serializes_snake_case_contract() {
        let payload = AsrSpanRevisionPayload {
            span_id: "deepgram:system:1000-2000".to_string(),
            provider: "deepgram".to_string(),
            source_id: "system".to_string(),
            provider_item_id: Some("provider-item-1".to_string()),
            transcript_segment_id: Some("segment-1".to_string()),
            speaker_id: Some("speaker-0".to_string()),
            speaker_label: Some("Speaker 0".to_string()),
            channel: Some("left".to_string()),
            text: "hello".to_string(),
            start_time: 1.0,
            end_time: 2.0,
            confidence: 0.9,
            is_final: true,
            stability: AsrSpanStability::Final,
            revision_number: 2,
            supersedes: Some("rev-1".to_string()),
            turn_id: Some("turn-1".to_string()),
            end_of_turn: true,
            raw_event_ref: Some("deepgram.results[0]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000,
        };

        let json = serde_json::to_value(payload).expect("serialize payload");
        assert_eq!(json["stability"], "final");
        assert_eq!(json["span_id"], "deepgram:system:1000-2000");
        assert_eq!(json["provider_item_id"], "provider-item-1");
        assert_eq!(json["transcript_segment_id"], "segment-1");
        assert_eq!(json["revision_number"], 2);
        assert_eq!(json["end_of_turn"], true);
        assert_eq!(json["received_at_ms"], 1_700_000_000_000u64);
    }

    #[test]
    fn live_assist_card_record_serializes_status_and_outcome_contract() {
        let card = LiveAssistCardRecord {
            session_id: "session-1".to_string(),
            proposal: AgentProposalPayload {
                id: "card-1".to_string(),
                source_segment_id: "segment-1".to_string(),
                source_id: "default-mic".to_string(),
                speaker_label: Some("Speaker 1".to_string()),
                kind: AgentProposalKind::Question,
                title: "Question from Speaker 1".to_string(),
                body: "Consider answering or linking this question: What changed?".to_string(),
                confidence: 0.92,
                created_at_ms: 1_700_000_000_000,
                signal: None,
            },
            status: LiveAssistCardStatus::Approved,
            source_span_ids: vec!["span-1".to_string()],
            graph_context_ids: vec!["node-1".to_string()],
            outcome: Some(AgentActionResult {
                proposal_id: "card-1".to_string(),
                action: "graph_update".to_string(),
                message: "Approved live assist card".to_string(),
                graph_updated: true,
                timestamp_ms: 1_700_000_000_100,
            }),
            projection_patch_sequence: Some(7),
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_100,
            origin: None,
            answer: None,
            signal: None,
        };

        let json = serde_json::to_value(card).expect("serialize live assist card");
        assert_eq!(json["status"], "approved");
        assert_eq!(json["proposal"]["kind"], "question");
        assert_eq!(json["source_span_ids"][0], "span-1");
        assert_eq!(json["outcome"]["action"], "graph_update");
        assert_eq!(json["projection_patch_sequence"], 7);
        // A `None` origin/answer/signal/proposal.signal must not appear in
        // the wire JSON at all (skip_serializing_if) — the exact shape
        // every pre-83cc consumer already tolerates.
        assert!(!json.as_object().unwrap().contains_key("origin"));
        assert!(!json.as_object().unwrap().contains_key("answer"));
        assert!(!json.as_object().unwrap().contains_key("signal"));
        assert!(!json["proposal"].as_object().unwrap().contains_key("signal"));
    }

    /// audio-graph-83cc T1: additive-fields-only contract for
    /// `LiveAssistCardRecord.origin`/`.answer`/`.signal` and
    /// `AgentProposalPayload.signal`. A pre-83cc record (no `origin`,
    /// `answer`, or `signal` key at all) must deserialize with every new
    /// field defaulting to `None` — this is C12's whole point: additive
    /// *fields* are safe for an older reader (serde ignores unknown keys),
    /// additive *variants* are not, so this design adds fields only.
    #[test]
    fn live_assist_card_pre_83cc_json_deserializes_with_every_new_field_defaulting_to_none() {
        let legacy_json = serde_json::json!({
            "session_id": "session-legacy",
            "proposal": {
                "id": "card-legacy",
                "source_segment_id": "segment-legacy",
                "source_id": "default-mic",
                "kind": "question",
                "title": "Question from Speaker 1",
                "body": "Consider answering or linking this question: What changed?",
                "confidence": 0.9,
                "created_at_ms": 1_700_000_000_000u64
            },
            "status": "pending",
            "source_span_ids": ["segment-legacy"],
            "graph_context_ids": [],
            "created_at_ms": 1_700_000_000_000u64,
            "updated_at_ms": 1_700_000_000_000u64
        });

        let card: LiveAssistCardRecord =
            serde_json::from_value(legacy_json).expect("pre-83cc record must still deserialize");
        assert_eq!(card.origin, None);
        assert_eq!(card.answer, None);
        assert_eq!(card.signal, None);
        assert_eq!(card.proposal.signal, None);

        // Re-serializing a defaulted legacy record must not corrupt it or
        // introduce the new keys (skip_serializing_if) — byte-identical
        // shape to what was read, modulo whitespace.
        let round_tripped = serde_json::to_value(&card).expect("re-serialize legacy record");
        assert!(!round_tripped.as_object().unwrap().contains_key("origin"));
        assert!(!round_tripped.as_object().unwrap().contains_key("answer"));
        assert!(!round_tripped.as_object().unwrap().contains_key("signal"));
    }

    /// The inverse of the test above (A1's other gate): a NEW-shape record
    /// — written by this build, carrying real `origin`/`answer`/`signal`
    /// values — must still deserialize in an OLDER reader that has never
    /// heard of those fields. Modeled with a locally-defined struct that is
    /// exactly the pre-83cc `LiveAssistCardRecord` shape (no
    /// `deny_unknown_fields` anywhere on the real type, so this is the
    /// real behavior, not an assumption about it).
    #[test]
    fn live_assist_card_new_shape_json_deserializes_in_a_pre_83cc_reader_that_ignores_unknown_fields()
     {
        #[derive(serde::Deserialize)]
        struct PreCard83ccShape {
            #[allow(dead_code)]
            session_id: String,
            status: LiveAssistCardStatus,
            #[allow(dead_code)]
            source_span_ids: Vec<String>,
            #[allow(dead_code)]
            graph_context_ids: Vec<String>,
            #[allow(dead_code)]
            created_at_ms: u64,
            #[allow(dead_code)]
            updated_at_ms: u64,
            // Deliberately NO `origin`/`answer`/`signal` field, and no
            // `proposal` field either (proposal.signal is nested) — this
            // struct models a reader from before ANY of this unit's fields
            // existed.
        }

        let new_shape = LiveAssistCardRecord {
            session_id: "session-new".to_string(),
            proposal: AgentProposalPayload {
                id: "card-new".to_string(),
                source_segment_id: "segment-new".to_string(),
                source_id: "default-mic".to_string(),
                speaker_label: None,
                kind: AgentProposalKind::Question,
                title: "Question from Speaker 1".to_string(),
                body: "Consider answering or linking this question: What changed?".to_string(),
                confidence: 0.9,
                created_at_ms: 1_700_000_000_000,
                signal: Some(SignalGrade::Strong),
            },
            status: LiveAssistCardStatus::Pending,
            source_span_ids: vec!["segment-new".to_string()],
            graph_context_ids: vec![],
            outcome: None,
            projection_patch_sequence: None,
            created_at_ms: 1_700_000_000_000,
            updated_at_ms: 1_700_000_000_000,
            origin: Some(CardOrigin::User),
            answer: Some(CardAnswer {
                status: CardAnswerStatus::Answered,
                text: "The deadline moved to Friday.".to_string(),
                truncated: false,
                evidence_span_ids: vec!["segment-new".to_string()],
                evidence_graph_ids: vec![],
                notes_last_sequence: None,
                route_id: "route.openrouter".to_string(),
                requested_by: CardOrigin::User,
                finish_reason: Some("stop".to_string()),
                total_tokens: Some(42),
                answered_at_ms: 1_700_000_001_000,
            }),
            signal: Some(SignalGrade::Strong),
        };

        let json = serde_json::to_string(&new_shape).expect("serialize new-shape record");
        let old_reader: PreCard83ccShape =
            serde_json::from_str(&json).expect("an older reader must tolerate unknown fields");
        assert_eq!(old_reader.status, LiveAssistCardStatus::Pending);
    }

    /// Pins `CardAnswer`'s own wire shape (field names, snake_case enum
    /// values, which optionals are omitted vs `null` vs present) —
    /// independent of the whole-record tests above, so a future field
    /// rename or a dropped `skip_serializing_if` is caught at this type's
    /// own boundary.
    #[test]
    fn card_answer_serializes_snake_case_contract_and_omits_absent_optionals() {
        let minimal = CardAnswer {
            status: CardAnswerStatus::Failed,
            text: String::new(),
            truncated: false,
            evidence_span_ids: vec![],
            evidence_graph_ids: vec![],
            notes_last_sequence: None,
            route_id: "route.openrouter".to_string(),
            requested_by: CardOrigin::Transcript,
            finish_reason: None,
            total_tokens: None,
            answered_at_ms: 1_700_000_000_000,
        };
        let json = serde_json::to_value(&minimal).expect("serialize minimal CardAnswer");
        assert_eq!(json["status"], "failed");
        assert_eq!(json["requested_by"], "transcript");
        assert_eq!(json["route_id"], "route.openrouter");
        assert_eq!(json["evidence_span_ids"], serde_json::json!([]));
        // `truncated` is a plain bool (no `skip_serializing_if`) — `false`
        // is meaningful information ("not truncated"), not absence, so it
        // is always present on the wire, unlike the Option fields below.
        assert_eq!(json["truncated"], false);
        let obj = json.as_object().unwrap();
        assert!(!obj.contains_key("notes_last_sequence"));
        assert!(!obj.contains_key("finish_reason"));
        assert!(!obj.contains_key("total_tokens"));

        let full = CardAnswer {
            status: CardAnswerStatus::Answered,
            text: "42".to_string(),
            truncated: true,
            evidence_span_ids: vec!["span-1".to_string()],
            evidence_graph_ids: vec!["node-1".to_string()],
            notes_last_sequence: Some(9),
            route_id: "route.openrouter".to_string(),
            requested_by: CardOrigin::User,
            finish_reason: Some("stop".to_string()),
            total_tokens: Some(128),
            answered_at_ms: 1_700_000_001_000,
        };
        let json = serde_json::to_value(&full).expect("serialize full CardAnswer");
        assert_eq!(json["status"], "answered");
        assert_eq!(json["requested_by"], "user");
        assert_eq!(json["notes_last_sequence"], 9);
        assert_eq!(json["finish_reason"], "stop");
        assert_eq!(json["total_tokens"], 128);
        assert_eq!(json["truncated"], true);
    }

    /// `CardAnswer::cap_text` (fix-round addition): truncates to
    /// `MAX_CARD_ANSWER_TEXT_CHARS` chars and marks `truncated = true` only
    /// when the text actually exceeded the cap. Uses a multi-byte string so
    /// truncation exercises the `chars()`-boundary path, not just ASCII
    /// byte-slicing (which would panic or corrupt UTF-8 on a wrong boundary).
    #[test]
    fn card_answer_cap_text_truncates_and_marks_truncated_when_over_the_cap() {
        let over = "é".repeat(MAX_CARD_ANSWER_TEXT_CHARS + 5);
        let mut answer = CardAnswer {
            status: CardAnswerStatus::Answered,
            text: over,
            truncated: false,
            evidence_span_ids: vec![],
            evidence_graph_ids: vec![],
            notes_last_sequence: None,
            route_id: "route.openrouter".to_string(),
            requested_by: CardOrigin::Transcript,
            finish_reason: None,
            total_tokens: None,
            answered_at_ms: 1_700_000_000_000,
        };
        let did_truncate = answer.cap_text();
        assert!(did_truncate);
        assert!(answer.truncated);
        assert_eq!(answer.text.chars().count(), MAX_CARD_ANSWER_TEXT_CHARS);
        assert!(
            answer.text.chars().all(|c| c == 'é'),
            "truncation must land on a char boundary and not corrupt the text"
        );
    }

    /// The inverse: text at or under the cap is untouched and `truncated`
    /// stays `false` — `cap_text` must not be a no-op-that-lies (e.g.
    /// flipping the flag when nothing was cut).
    #[test]
    fn card_answer_cap_text_is_a_no_op_at_or_under_the_cap() {
        for len in [0, 1, MAX_CARD_ANSWER_TEXT_CHARS] {
            let text = "a".repeat(len);
            let mut answer = CardAnswer {
                status: if len == 0 {
                    CardAnswerStatus::Failed
                } else {
                    CardAnswerStatus::Answered
                },
                text: text.clone(),
                truncated: false,
                evidence_span_ids: vec![],
                evidence_graph_ids: vec![],
                notes_last_sequence: None,
                route_id: "route.openrouter".to_string(),
                requested_by: CardOrigin::Transcript,
                finish_reason: None,
                total_tokens: None,
                answered_at_ms: 1_700_000_000_000,
            };
            let did_truncate = answer.cap_text();
            assert!(!did_truncate, "length {len} must not be truncated");
            assert!(!answer.truncated, "length {len} must not mark truncated");
            assert_eq!(answer.text, text);
        }
    }

    #[test]
    fn diarization_span_revision_serializes_snake_case_contract() {
        let payload = DiarizationSpanRevisionPayload {
            span_id: "local_clustering:session:1000-2000:speaker-c-0".to_string(),
            provider: "local_clustering".to_string(),
            timeline_id: "session".to_string(),
            source_id: None,
            speaker_id: Some("speaker-c-0".to_string()),
            speaker_label: Some("Speaker 1".to_string()),
            channel: Some("mixed".to_string()),
            start_time: 1.0,
            end_time: 2.0,
            confidence: None,
            is_final: false,
            stability: DiarizationSpanStability::Provisional,
            revision_number: 1,
            supersedes: None,
            basis_asr_span_ids: vec!["asr:1".to_string()],
            basis_transcript_segment_ids: vec!["segment-1".to_string()],
            raw_event_ref: Some("window_start_sample:16000".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000,
        };

        let json = serde_json::to_value(payload).expect("serialize payload");
        assert_eq!(json["stability"], "provisional");
        assert_eq!(json["timeline_id"], "session");
        assert_eq!(
            json["span_id"],
            "local_clustering:session:1000-2000:speaker-c-0"
        );
        assert_eq!(json["speaker_id"], "speaker-c-0");
        assert_eq!(json["basis_asr_span_ids"][0], "asr:1");
        assert!(
            json.get("source_id").is_none(),
            "session-level local timelines should not invent a source id"
        );
        assert!(
            json.get("confidence").is_none(),
            "uncalibrated local confidence should be omitted"
        );
        assert!(
            json.get("capture_latency_ms").is_none(),
            "missing capture latency should be omitted"
        );
        assert!(
            json.get("asr_latency_ms").is_none(),
            "missing ASR latency should be omitted"
        );
    }

    #[test]
    fn persistence_queue_backpressure_payload_is_redacted() {
        let payload = PersistenceQueueBackpressurePayload {
            writer: "transcript_event".to_string(),
            is_backpressured: true,
            queue_capacity: 2048,
            dropped_count: 3,
        };

        let json = serde_json::to_value(payload).expect("serialize payload");
        assert_eq!(json["writer"], "transcript_event");
        assert_eq!(json["is_backpressured"], true);
        assert_eq!(json["queue_capacity"], 2048);
        assert_eq!(json["dropped_count"], 3);
        assert!(
            json.get("session_id").is_none(),
            "queue diagnostics must not expose session ids"
        );
        assert!(
            json.get("path").is_none(),
            "queue diagnostics must not expose local file paths"
        );
        assert!(
            json.get("text").is_none(),
            "queue diagnostics must not expose transcript text"
        );
    }
}
