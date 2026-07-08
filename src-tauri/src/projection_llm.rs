//! Structured LLM output contract for notes/graph projection patches.
//!
//! This module stops at prompt construction, model-output parsing, and trusted
//! patch construction. Runtime scheduler dispatch is intentionally wired in a
//! later slice so live ASR ingestion cannot start provider calls until the
//! apply path and telemetry are integrated together.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use schemars::JsonSchema;

use crate::llm::engine::ChatMessage;
use crate::projections::{
    GraphNodeDraft, ProjectionBasisStaleness, ProjectionJob, ProjectionKind, ProjectionOperation,
    ProjectionPatch, ProjectionProvenance, ROLLING_SUMMARY_HOT_WINDOW_TURNS, TranscriptEvent,
    TranscriptLedger, ordered_for_window,
};

pub const PROJECTION_PATCH_PROMPT_ID: &str = "projection_patch_v1";
pub const PROJECTION_PATCH_REPAIR_PROMPT_ID: &str = "projection_patch_repair_v1";

/// Number of leading messages in a projection prompt that form the byte-stable,
/// cache-eligible prefix (ADR-0025 §2d / seed audio-graph-d77e).
///
/// The prompt is ordered static→dynamic: message 0 is the system block
/// (instructions + operation guidance + output schema — identical every turn)
/// and message 1 is the append-only stable-context block (pinned facts +
/// rolling summary). The per-tick volatile metadata (basis hash, span count,
/// job id) lives in the *last* message so it never busts the cached prefix.
/// A provider cache breakpoint (`cache_control`) is placed after this many
/// leading messages for cache-capable providers.
pub const PROJECTION_STABLE_PREFIX_MESSAGE_COUNT: usize = 2;

/// Max characters of a single older turn kept in the rolling summary digest.
/// Bounds each folded turn's contribution so the summary stays far smaller than
/// the full transcript JSON it replaces.
const SUMMARY_TURN_DIGEST_MAX_CHARS: usize = 160;

/// Incremental extractive rolling summary of the transcript turns that have
/// left the verbatim hot window (ADR-0025 §2c / seed audio-graph-18ee).
///
/// Each older turn contributes exactly one bounded digest line, folded in when
/// the turn leaves the hot buffer. A line is **never rewritten** once folded —
/// there is no recursive "summarize the summary" step, which is what causes
/// the recursive-summarization ("Telephone") drift the research warns about.
/// Because a turn's digest depends only on that turn, folding turn-by-turn is
/// byte-identical to a single deterministic pass, so the summary can be
/// recomputed from the ledger on any call without ever re-summarizing a turn.
/// The serialized form is append-only, giving the stable-prefix cache (d77e) a
/// growing-but-stable prefix.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RollingSummary {
    lines: Vec<String>,
    summarized_through_revision: Option<u64>,
}

impl RollingSummary {
    /// Fold a single turn that has just left the hot window into the summary.
    ///
    /// Appends one bounded digest line and advances the summarized-through
    /// revision. Touches no previously folded line, so this is a true
    /// incremental fold (never a rebuild).
    pub fn fold_leaving_turn(&mut self, event: &TranscriptEvent) {
        self.lines.push(digest_line(event));
        self.summarized_through_revision = Some(match self.summarized_through_revision {
            Some(current) => current.max(event.revision_number),
            None => event.revision_number,
        });
    }

    /// Build the summary for the "older" turns (everything outside the last
    /// [`ROLLING_SUMMARY_HOT_WINDOW_TURNS`] turns) by folding each older turn in
    /// canonical order exactly once.
    pub fn from_older_turns(older: &[&TranscriptEvent]) -> Self {
        let mut summary = Self::default();
        for event in older {
            summary.fold_leaving_turn(event);
        }
        summary
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn summarized_through_revision(&self) -> Option<u64> {
        self.summarized_through_revision
    }

    /// Render the summary as an append-only block for the prompt.
    pub fn render(&self) -> String {
        self.lines.join("\n")
    }
}

/// One bounded, deterministic digest line for a folded-out turn.
fn digest_line(event: &TranscriptEvent) -> String {
    let speaker = event.speaker_label.as_deref().unwrap_or("Unknown");
    let text: String = event
        .text
        .chars()
        .take(SUMMARY_TURN_DIGEST_MAX_CHARS)
        .collect();
    let text = text.trim();
    if event.text.chars().count() > SUMMARY_TURN_DIGEST_MAX_CHARS {
        format!("[{speaker}] {text}…")
    } else {
        format!("[{speaker}] {text}")
    }
}

/// Split the basis events into (older turns to summarize, hot-window turns to
/// feed verbatim), in canonical replay order.
fn split_summary_window(
    events: &[TranscriptEvent],
) -> (Vec<&TranscriptEvent>, Vec<&TranscriptEvent>) {
    let ordered = ordered_for_window(events);
    if ordered.len() <= ROLLING_SUMMARY_HOT_WINDOW_TURNS {
        return (Vec::new(), ordered);
    }
    let split = ordered.len() - ROLLING_SUMMARY_HOT_WINDOW_TURNS;
    let older = ordered[..split].to_vec();
    let hot = ordered[split..].to_vec();
    (older, hot)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProjectionPatchDraft {
    #[serde(default)]
    pub operations: Vec<ProjectionOperation>,
    #[serde(default)]
    pub confidence: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectionPatchBuildContext {
    pub sequence: u64,
    pub llm_request_id: String,
    pub provider: String,
    pub model: String,
    pub prompt_id: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ProjectionPatchDraftError {
    InvalidJson {
        error: String,
    },
    StaleBasis {
        staleness: ProjectionBasisStaleness,
    },
    MissingBasisSpan {
        span_id: String,
        revision_number: u64,
    },
    InvalidConfidence {
        confidence: f32,
    },
    WrongOperationKind {
        expected: ProjectionKind,
        operation: &'static str,
    },
    EmptyOperationField {
        operation: &'static str,
        field: &'static str,
    },
    DuplicateOperationId {
        operation: &'static str,
        id: String,
    },
    InvalidGraphEdgeWeight {
        id: String,
        weight: f32,
    },
    InvalidGraphEdgeWeightDelta {
        operation: &'static str,
        id: String,
        weight_delta: f32,
    },
    InvalidGraphSplitReplacementCount {
        id: String,
        count: usize,
    },
    DuplicateGraphSplitReplacementId {
        id: String,
        replacement_id: String,
    },
}

impl fmt::Display for ProjectionPatchDraftError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidJson { error } => write!(f, "invalid projection patch JSON: {error}"),
            Self::StaleBasis { staleness } => {
                write!(f, "projection job basis is stale: {staleness:?}")
            }
            Self::MissingBasisSpan {
                span_id,
                revision_number,
            } => write!(
                f,
                "projection job references missing transcript span {span_id}@{revision_number}"
            ),
            Self::InvalidConfidence { confidence } => write!(
                f,
                "projection patch confidence must be between 0.0 and 1.0, got {confidence}"
            ),
            Self::WrongOperationKind {
                expected,
                operation,
            } => write!(
                f,
                "projection patch for {expected:?} cannot contain {operation}"
            ),
            Self::EmptyOperationField { operation, field } => {
                write!(f, "{operation} requires non-empty field {field}")
            }
            Self::DuplicateOperationId { operation, id } => {
                write!(f, "{operation} repeats id {id} in one projection patch")
            }
            Self::InvalidGraphEdgeWeight { id, weight } => {
                write!(f, "graph edge {id} has invalid weight {weight}")
            }
            Self::InvalidGraphEdgeWeightDelta {
                operation,
                id,
                weight_delta,
            } => write!(
                f,
                "{operation} for graph edge {id} has invalid weight_delta {weight_delta}"
            ),
            Self::InvalidGraphSplitReplacementCount { id, count } => write!(
                f,
                "split_graph_node for {id} requires at least two replacement_nodes, got {count}"
            ),
            Self::DuplicateGraphSplitReplacementId { id, replacement_id } => write!(
                f,
                "split_graph_node for {id} repeats replacement node id {replacement_id}"
            ),
        }
    }
}

impl std::error::Error for ProjectionPatchDraftError {}

pub fn projection_patch_draft_json_schema() -> Result<serde_json::Value, String> {
    serde_json::to_value(schemars::schema_for!(ProjectionPatchDraft))
        .map_err(|e| format!("failed to build projection patch draft JSON schema: {e}"))
}

/// Human-authored, provider-strict JSON schema for a projection patch draft,
/// scoped to `kind` (seed audio-graph-a324).
///
/// This is the schema sent as OpenRouter structured-outputs
/// (`response_format: json_schema, strict: true`) so a schema-capable model is
/// constrained at generation time. It differs from
/// [`projection_patch_draft_json_schema`] (the `schemars`-derived shape the
/// vLLM/mistral.rs paths use) in three deliberate ways that make it a good fit
/// for OpenAI/OpenRouter strict mode and **at least as strict as the runtime
/// validator** ([`validate_projection_patch_draft`]):
///
/// 1. **Kind partitioning.** Only the operation variants that
///    [`operation_kind`] maps to `kind` are offered. The validator rejects a
///    graph op in a notes job (and vice-versa); the schema now forbids the
///    model from emitting one at all, so it is not looser than the validator on
///    the kind axis.
/// 2. **Every operation field is required.** The user's failures were patches
///    *missing* structural fields (`id` / `title` / `tags` for notes,
///    `relations`/`target`/`name` for graph edges). serde requires those
///    fields; the derived schema left them optional, so the model produced
///    field-incomplete patches that only failed at parse time. Here each
///    variant lists all of its serde fields in `required` with
///    `additionalProperties: false`, matching the internally-tagged wire shape
///    exactly. Rust `Option` fields (`description`, `after_id`, `label`) stay
///    required but nullable (`["string", "null"]`) so strict mode is satisfied
///    without forcing the model to invent a value.
/// 3. **No numeric range / non-empty keywords.** The validator additionally
///    enforces `weight`/`confidence` in `0.0..=1.0` and non-empty trimmed
///    strings. Those are intentionally NOT encoded here: several strict-mode
///    engines reject `minimum`/`maximum`/`minLength`, which would turn every
///    request into a 400. They stay the validator's job (and the repair path's).
///    That makes the schema marginally looser than the validator on ranges only
///    — never on structure or kind, which is where the failures were.
pub fn projection_patch_strict_json_schema(kind: &ProjectionKind) -> serde_json::Value {
    use serde_json::json;

    fn string() -> serde_json::Value {
        json!({ "type": "string" })
    }
    fn nullable_string() -> serde_json::Value {
        json!({ "type": ["string", "null"] })
    }
    fn string_array() -> serde_json::Value {
        json!({ "type": "array", "items": { "type": "string" } })
    }
    fn number() -> serde_json::Value {
        json!({ "type": "number" })
    }

    // One internally-tagged operation variant: a closed object whose `type` is
    // pinned to `type_const` and whose every field is required (strict mode).
    fn variant(type_const: &str, fields: &[(&str, serde_json::Value)]) -> serde_json::Value {
        let mut properties = serde_json::Map::new();
        properties.insert(
            "type".to_string(),
            json!({ "type": "string", "enum": [type_const] }),
        );
        let mut required = vec![json!("type")];
        for (name, schema) in fields {
            properties.insert((*name).to_string(), schema.clone());
            required.push(json!(name));
        }
        json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }

    let graph_node_draft = json!({
        "type": "object",
        "properties": {
            "id": string(),
            "name": string(),
            "entity_type": string(),
            "description": nullable_string(),
        },
        "required": ["id", "name", "entity_type", "description"],
        "additionalProperties": false,
    });

    let operation_variants = match kind {
        ProjectionKind::Notes => vec![
            variant(
                "upsert_note",
                &[
                    ("id", string()),
                    ("title", string()),
                    ("body", string()),
                    ("tags", string_array()),
                ],
            ),
            variant("delete_note", &[("id", string())]),
            variant(
                "reorder_note",
                &[("id", string()), ("after_id", nullable_string())],
            ),
        ],
        ProjectionKind::Graph => vec![
            variant(
                "upsert_graph_node",
                &[
                    ("id", string()),
                    ("name", string()),
                    ("entity_type", string()),
                    ("description", nullable_string()),
                ],
            ),
            variant("remove_graph_node", &[("id", string())]),
            variant("invalidate_graph_node", &[("id", string())]),
            variant(
                "upsert_graph_edge",
                &[
                    ("id", string()),
                    ("source", string()),
                    ("target", string()),
                    ("relation_type", string()),
                    ("label", nullable_string()),
                    ("weight", number()),
                ],
            ),
            variant("remove_graph_edge", &[("id", string())]),
            variant("invalidate_graph_edge", &[("id", string())]),
            variant(
                "strengthen_graph_edge",
                &[("id", string()), ("weight_delta", number())],
            ),
            variant(
                "weaken_graph_edge",
                &[("id", string()), ("weight_delta", number())],
            ),
            variant(
                "merge_graph_nodes",
                &[("source_id", string()), ("target_id", string())],
            ),
            variant(
                "split_graph_node",
                &[
                    ("id", string()),
                    (
                        "replacement_nodes",
                        json!({ "type": "array", "items": graph_node_draft }),
                    ),
                ],
            ),
        ],
    };

    json!({
        "type": "object",
        "properties": {
            "operations": {
                "type": "array",
                "items": { "anyOf": operation_variants },
            },
            "confidence": { "type": ["number", "null"] },
        },
        "required": ["operations", "confidence"],
        "additionalProperties": false,
    })
}

pub fn parse_projection_patch_draft(
    raw: &str,
    expected_kind: &ProjectionKind,
) -> Result<ProjectionPatchDraft, ProjectionPatchDraftError> {
    let draft: ProjectionPatchDraft =
        serde_json::from_str(raw).map_err(|error| ProjectionPatchDraftError::InvalidJson {
            error: error.to_string(),
        })?;
    validate_projection_patch_draft(&draft, expected_kind)?;
    Ok(draft)
}

pub fn trusted_projection_patch_from_model_json(
    raw: &str,
    job: &ProjectionJob,
    context: ProjectionPatchBuildContext,
) -> Result<ProjectionPatch, ProjectionPatchDraftError> {
    let draft = parse_projection_patch_draft(raw, &job.kind)?;
    Ok(ProjectionPatch {
        sequence: context.sequence,
        kind: job.kind.clone(),
        llm_request_id: context.llm_request_id,
        basis: job.basis.clone(),
        operations: draft.operations,
        confidence: draft.confidence.unwrap_or(1.0),
        provenance: ProjectionProvenance {
            provider: context.provider,
            model: context.model,
            prompt_id: context.prompt_id,
        },
        queued_at_ms: Some(job.queued_at_ms),
        generation_latency_ms: None,
        apply_latency_ms: None,
        created_at_ms: context.created_at_ms,
    })
}

/// Content-free description of what the projection prompt for `job` would
/// carry (ADR-0025 §2g / seed audio-graph-72d5). Used by the data-movement
/// ledger to record the new remote-LLM data flows without touching any
/// transcript/summary text.
///
/// Recomputed deterministically from the same window split the prompt builder
/// uses, so the ledger and the actual prompt never disagree about whether a
/// rolling summary / pinned-fact block was present.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectionPromptShape {
    /// A rolling summary of older turns was present (a new transcript-derived
    /// off-device artifact when the call is remote).
    pub has_rolling_summary: bool,
    /// Character count of the pinned typed-fact block (graph/transcript-derived
    /// context). 0 when absent.
    pub pinned_fact_chars: u64,
}

pub fn projection_prompt_shape(
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
) -> ProjectionPromptShape {
    let Ok(events) = basis_events(job, ledger) else {
        return ProjectionPromptShape::default();
    };
    let (older, _hot) = split_summary_window(&events);
    let pinned = pinned_typed_facts(&events);
    let pinned_chars: usize = pinned.iter().map(|line| line.chars().count()).sum();
    ProjectionPromptShape {
        has_rolling_summary: !older.is_empty(),
        pinned_fact_chars: pinned_chars as u64,
    }
}

pub fn projection_patch_prompt_messages(
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
) -> Result<Vec<ChatMessage>, ProjectionPatchDraftError> {
    ledger
        .validate_basis(&job.basis)
        .map_err(|staleness| ProjectionPatchDraftError::StaleBasis { staleness })?;

    let events = basis_events(job, ledger)?;
    // ADR-0025 §2c (seed audio-graph-18ee): feed a rolling summary of older
    // turns + the last K turns verbatim, NOT the whole transcript. The summary
    // is folded incrementally (one line per turn leaving the hot window) and
    // never re-summarized, so token cost is bounded per tick instead of O(n²).
    let (older, hot) = split_summary_window(&events);
    let summary = RollingSummary::from_older_turns(&older);
    let hot_events: Vec<TranscriptEvent> = hot.iter().map(|event| (*event).clone()).collect();
    let transcript = format_transcript_events_json(&hot_events);
    let pinned_facts = pinned_typed_facts(&events);

    let operation_guidance = match job.kind {
        ProjectionKind::Notes => {
            "Use only upsert_note, delete_note, and reorder_note operations. Keep stable note ids when refining earlier notes."
        }
        ProjectionKind::Graph => {
            "Use only graph operations: upsert_graph_node, remove_graph_node, invalidate_graph_node, upsert_graph_edge, remove_graph_edge, invalidate_graph_edge, strengthen_graph_edge, weaken_graph_edge, merge_graph_nodes, and split_graph_node. Upsert nodes before edges that reference them. Prefer retcon operations over duplicate nodes when later transcript context corrects earlier assumptions."
        }
    };

    let schema = projection_patch_draft_json_schema()
        .map(|value| value.to_string())
        .unwrap_or_else(|_| {
            r#"{"type":"object","required":["operations"],"properties":{"operations":{"type":"array"},"confidence":{"type":"number"}}"#.to_string()
        });

    // Prompt is ordered static→dynamic so the leading blocks form a byte-stable
    // prefix across submissions (ADR-0025 §2d / seed audio-graph-d77e):
    //   [0] system: instructions + operation guidance + output schema (immutable)
    //   [1] stable context: pinned facts + rolling summary (append-only)
    //   [.] append-only hot-buffer transcript (grows at the tail)
    //   [last] per-tick volatile metadata (basis hash / span count / job id)
    // Anything that changes every tick MUST stay at the tail or it busts the
    // cached prefix. See `PROJECTION_STABLE_PREFIX_MESSAGE_COUNT`.
    let summary_block = if summary.is_empty() {
        "(no earlier turns yet)".to_string()
    } else {
        summary.render()
    };
    let pinned_block = if pinned_facts.is_empty() {
        "(none)".to_string()
    } else {
        pinned_facts.join("\n")
    };

    Ok(vec![
        ChatMessage {
            role: "system".to_string(),
            content: format!(
                "You generate AudioGraph projection patch drafts. Return strict JSON only, with no markdown. \
                 Do not include trusted metadata such as sequence, basis, provenance, session_id, or llm_request_id; \
                 the backend stamps those fields. {operation_guidance}\n\n\
                 Output JSON schema:\n{schema}"
            ),
        },
        ChatMessage {
            role: "user".to_string(),
            content: format!(
                "Pinned facts (must-never-lose, structured):\n{pinned_block}\n\n\
                 Conversation summary (older turns, oldest first):\n{summary_block}"
            ),
        },
        ChatMessage {
            role: "user".to_string(),
            content: format!("Recent transcript (verbatim, most recent turns):\n{transcript}"),
        },
        ChatMessage {
            role: "user".to_string(),
            content: format!(
                "Projection job:\n\
                 id: {job_id}\n\
                 session_id: {session_id}\n\
                 kind: {kind}\n\
                 basis_hash: {basis_hash}\n\
                 span_count: {span_count}\n\n\
                 Return a compact patch draft as JSON: {{\"operations\": [...], \"confidence\": 0.0-1.0}}.",
                job_id = job.id,
                session_id = job.session_id,
                kind = projection_kind_key(&job.kind),
                basis_hash = job.basis.transcript_hash,
                span_count = job.basis.span_revisions.len(),
            ),
        },
    ])
}

/// Pinned must-never-lose facts, rendered as a deterministic structured block.
///
/// ADR-0025 §2c.3: the research shows a prose summarizer inverts negations and
/// drops rejection reasons, so the identity-bearing facts (which speaker said
/// something, in what span) are pinned as structured lines rather than trusted
/// to the summary. Derived deterministically from the basis events so the block
/// is byte-stable across turns (append-only), keeping the cache prefix intact.
/// This is transcript-derived (not a live graph snapshot) to keep the prompt
/// builder's `(job, ledger)` seam intact; the graph-snapshot source is a later
/// pillar (§2c graph feed).
fn pinned_typed_facts(events: &[TranscriptEvent]) -> Vec<String> {
    // First-appearance order (NOT sorted): a newly-seen speaker appends at the
    // tail, so the block stays append-only across turns and the stable-prefix
    // cache (d77e) keeps hitting. Sorting would let a new speaker reorder the
    // block and bust the cached prefix.
    let ordered = ordered_for_window(events);
    let mut seen = BTreeSet::new();
    let mut facts = Vec::new();
    for event in ordered {
        if let Some(speaker) = event.speaker_label.as_deref()
            && seen.insert(speaker.to_string())
        {
            facts.push(format!("speaker: {speaker}"));
        }
    }
    facts
}

pub fn projection_patch_repair_prompt_messages(
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
    invalid_model_output: &str,
    error: &ProjectionPatchDraftError,
) -> Result<Vec<ChatMessage>, ProjectionPatchDraftError> {
    let mut messages = projection_patch_prompt_messages(job, ledger)?;
    let schema = projection_patch_draft_json_schema()
        .map(|value| value.to_string())
        .unwrap_or_else(|_| {
            r#"{"type":"object","required":["operations"],"properties":{"operations":{"type":"array"},"confidence":{"type":"number"}}"#.to_string()
        });

    messages.push(ChatMessage {
        role: "assistant".to_string(),
        content: compact_model_output(invalid_model_output),
    });
    messages.push(ChatMessage {
        role: "user".to_string(),
        content: format!(
            "The previous projection patch draft was invalid.\n\
             expected_kind: {kind}\n\
             validation_error: {error}\n\n\
             Output JSON schema:\n{schema}\n\n\
             Return only one corrected compact JSON patch draft. Do not include trusted metadata \
             such as sequence, basis, provenance, session_id, or llm_request_id. Prefer the \
             smallest operation set needed to repair the draft.",
            kind = projection_kind_key(&job.kind),
        ),
    });

    Ok(messages)
}

fn validate_projection_patch_draft(
    draft: &ProjectionPatchDraft,
    expected_kind: &ProjectionKind,
) -> Result<(), ProjectionPatchDraftError> {
    if let Some(confidence) = draft.confidence
        && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
    {
        return Err(ProjectionPatchDraftError::InvalidConfidence { confidence });
    }

    let mut seen_ids = BTreeSet::new();
    for operation in &draft.operations {
        validate_operation(operation, expected_kind)?;
        let (namespace, id) = operation_identity(operation);
        if !seen_ids.insert((namespace, id)) {
            return Err(ProjectionPatchDraftError::DuplicateOperationId {
                operation: operation_name(operation),
                id: id.to_string(),
            });
        }
    }

    Ok(())
}

fn validate_operation(
    operation: &ProjectionOperation,
    expected_kind: &ProjectionKind,
) -> Result<(), ProjectionPatchDraftError> {
    let actual_kind = operation_kind(operation);
    if &actual_kind != expected_kind {
        return Err(ProjectionPatchDraftError::WrongOperationKind {
            expected: expected_kind.clone(),
            operation: operation_name(operation),
        });
    }

    match operation {
        ProjectionOperation::UpsertNote {
            id, title, body, ..
        } => {
            require_non_empty(operation, "id", id)?;
            require_non_empty(operation, "title", title)?;
            require_non_empty(operation, "body", body)
        }
        ProjectionOperation::DeleteNote { id } => require_non_empty(operation, "id", id),
        ProjectionOperation::ReorderNote { id, .. } => require_non_empty(operation, "id", id),
        ProjectionOperation::UpsertGraphNode {
            id,
            name,
            entity_type,
            ..
        } => {
            require_non_empty(operation, "id", id)?;
            require_non_empty(operation, "name", name)?;
            require_non_empty(operation, "entity_type", entity_type)
        }
        ProjectionOperation::RemoveGraphNode { id } => require_non_empty(operation, "id", id),
        ProjectionOperation::InvalidateGraphNode { id } => require_non_empty(operation, "id", id),
        ProjectionOperation::UpsertGraphEdge {
            id,
            source,
            target,
            relation_type,
            weight,
            ..
        } => {
            require_non_empty(operation, "id", id)?;
            require_non_empty(operation, "source", source)?;
            require_non_empty(operation, "target", target)?;
            require_non_empty(operation, "relation_type", relation_type)?;
            if !weight.is_finite() || !(0.0..=1.0).contains(weight) {
                return Err(ProjectionPatchDraftError::InvalidGraphEdgeWeight {
                    id: id.clone(),
                    weight: *weight,
                });
            }
            Ok(())
        }
        ProjectionOperation::RemoveGraphEdge { id } => require_non_empty(operation, "id", id),
        ProjectionOperation::InvalidateGraphEdge { id } => require_non_empty(operation, "id", id),
        ProjectionOperation::StrengthenGraphEdge { id, weight_delta }
        | ProjectionOperation::WeakenGraphEdge { id, weight_delta } => {
            require_non_empty(operation, "id", id)?;
            validate_weight_delta(operation, id, *weight_delta)
        }
        ProjectionOperation::MergeGraphNodes {
            source_id,
            target_id,
        } => {
            require_non_empty(operation, "source_id", source_id)?;
            require_non_empty(operation, "target_id", target_id)
        }
        ProjectionOperation::SplitGraphNode {
            id,
            replacement_nodes,
        } => {
            require_non_empty(operation, "id", id)?;
            validate_graph_split_replacements(id, replacement_nodes)
        }
    }
}

fn validate_weight_delta(
    operation: &ProjectionOperation,
    id: &str,
    weight_delta: f32,
) -> Result<(), ProjectionPatchDraftError> {
    if !weight_delta.is_finite() || !(0.0..=1.0).contains(&weight_delta) {
        return Err(ProjectionPatchDraftError::InvalidGraphEdgeWeightDelta {
            operation: operation_name(operation),
            id: id.to_string(),
            weight_delta,
        });
    }
    Ok(())
}

fn validate_graph_split_replacements(
    id: &str,
    replacement_nodes: &[GraphNodeDraft],
) -> Result<(), ProjectionPatchDraftError> {
    if replacement_nodes.len() < 2 {
        return Err(
            ProjectionPatchDraftError::InvalidGraphSplitReplacementCount {
                id: id.to_string(),
                count: replacement_nodes.len(),
            },
        );
    }

    let mut replacement_ids = BTreeSet::new();
    for replacement in replacement_nodes {
        if replacement.id.trim().is_empty() {
            return Err(ProjectionPatchDraftError::EmptyOperationField {
                operation: "split_graph_node",
                field: "replacement_nodes.id",
            });
        }
        if replacement.name.trim().is_empty() {
            return Err(ProjectionPatchDraftError::EmptyOperationField {
                operation: "split_graph_node",
                field: "replacement_nodes.name",
            });
        }
        if replacement.entity_type.trim().is_empty() {
            return Err(ProjectionPatchDraftError::EmptyOperationField {
                operation: "split_graph_node",
                field: "replacement_nodes.entity_type",
            });
        }
        if !replacement_ids.insert(replacement.id.as_str()) {
            return Err(
                ProjectionPatchDraftError::DuplicateGraphSplitReplacementId {
                    id: id.to_string(),
                    replacement_id: replacement.id.clone(),
                },
            );
        }
    }

    Ok(())
}

fn require_non_empty(
    operation: &ProjectionOperation,
    field: &'static str,
    value: &str,
) -> Result<(), ProjectionPatchDraftError> {
    if value.trim().is_empty() {
        return Err(ProjectionPatchDraftError::EmptyOperationField {
            operation: operation_name(operation),
            field,
        });
    }
    Ok(())
}

fn operation_kind(operation: &ProjectionOperation) -> ProjectionKind {
    match operation {
        ProjectionOperation::UpsertNote { .. }
        | ProjectionOperation::DeleteNote { .. }
        | ProjectionOperation::ReorderNote { .. } => ProjectionKind::Notes,
        ProjectionOperation::UpsertGraphNode { .. }
        | ProjectionOperation::RemoveGraphNode { .. }
        | ProjectionOperation::InvalidateGraphNode { .. }
        | ProjectionOperation::UpsertGraphEdge { .. }
        | ProjectionOperation::RemoveGraphEdge { .. }
        | ProjectionOperation::InvalidateGraphEdge { .. }
        | ProjectionOperation::StrengthenGraphEdge { .. }
        | ProjectionOperation::WeakenGraphEdge { .. }
        | ProjectionOperation::MergeGraphNodes { .. }
        | ProjectionOperation::SplitGraphNode { .. } => ProjectionKind::Graph,
    }
}

fn operation_name(operation: &ProjectionOperation) -> &'static str {
    match operation {
        ProjectionOperation::UpsertNote { .. } => "upsert_note",
        ProjectionOperation::DeleteNote { .. } => "delete_note",
        ProjectionOperation::ReorderNote { .. } => "reorder_note",
        ProjectionOperation::UpsertGraphNode { .. } => "upsert_graph_node",
        ProjectionOperation::RemoveGraphNode { .. } => "remove_graph_node",
        ProjectionOperation::InvalidateGraphNode { .. } => "invalidate_graph_node",
        ProjectionOperation::UpsertGraphEdge { .. } => "upsert_graph_edge",
        ProjectionOperation::RemoveGraphEdge { .. } => "remove_graph_edge",
        ProjectionOperation::InvalidateGraphEdge { .. } => "invalidate_graph_edge",
        ProjectionOperation::StrengthenGraphEdge { .. } => "strengthen_graph_edge",
        ProjectionOperation::WeakenGraphEdge { .. } => "weaken_graph_edge",
        ProjectionOperation::MergeGraphNodes { .. } => "merge_graph_nodes",
        ProjectionOperation::SplitGraphNode { .. } => "split_graph_node",
    }
}

fn operation_identity(operation: &ProjectionOperation) -> (&'static str, &str) {
    match operation {
        ProjectionOperation::UpsertNote { id, .. }
        | ProjectionOperation::DeleteNote { id }
        | ProjectionOperation::ReorderNote { id, .. } => ("note", id.as_str()),
        ProjectionOperation::UpsertGraphNode { id, .. }
        | ProjectionOperation::RemoveGraphNode { id }
        | ProjectionOperation::InvalidateGraphNode { id }
        | ProjectionOperation::SplitGraphNode { id, .. } => ("graph_node", id.as_str()),
        ProjectionOperation::UpsertGraphEdge { id, .. }
        | ProjectionOperation::RemoveGraphEdge { id }
        | ProjectionOperation::InvalidateGraphEdge { id }
        | ProjectionOperation::StrengthenGraphEdge { id, .. }
        | ProjectionOperation::WeakenGraphEdge { id, .. } => ("graph_edge", id.as_str()),
        ProjectionOperation::MergeGraphNodes { source_id, .. } => {
            ("graph_node", source_id.as_str())
        }
    }
}

fn basis_events(
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
) -> Result<Vec<TranscriptEvent>, ProjectionPatchDraftError> {
    let latest_by_span: BTreeMap<(&str, u64), &TranscriptEvent> = ledger
        .latest_spans
        .iter()
        .map(|event| ((event.span_id.as_str(), event.revision_number), event))
        .collect();

    job.basis
        .span_revisions
        .iter()
        .map(|span| {
            latest_by_span
                .get(&(span.span_id.as_str(), span.revision_number))
                .map(|event| (*event).clone())
                .ok_or_else(|| ProjectionPatchDraftError::MissingBasisSpan {
                    span_id: span.span_id.clone(),
                    revision_number: span.revision_number,
                })
        })
        .collect()
}

fn format_transcript_events_json(events: &[TranscriptEvent]) -> String {
    if events.is_empty() {
        return "[]".to_string();
    }

    serde_json::to_string_pretty(events).unwrap_or_else(|_| "[]".to_string())
}

fn compact_model_output(raw: &str) -> String {
    const MAX_CHARS: usize = 2_000;
    let mut compact = raw.chars().take(MAX_CHARS).collect::<String>();
    if raw.chars().count() > MAX_CHARS {
        compact.push_str("\n...[truncated]");
    }
    compact
}

fn projection_kind_key(kind: &ProjectionKind) -> &'static str {
    match kind {
        ProjectionKind::Notes => "notes",
        ProjectionKind::Graph => "graph",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projections::{
        ProjectionBasis, ProjectionBasisSpan, ProjectionPriority, TranscriptEventStability,
    };

    fn event(span_id: &str, revision_number: u64, text: &str) -> TranscriptEvent {
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
            revision_number,
            supersedes: None,
            turn_id: None,
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000 + revision_number,
        }
    }

    fn job(kind: ProjectionKind, ledger: &TranscriptLedger) -> ProjectionJob {
        ProjectionJob {
            id: "projection:session-1:notes:1".to_string(),
            session_id: "session-1".to_string(),
            kind,
            basis: ledger.current_basis(),
            priority: ProjectionPriority::Realtime,
            queued_at_ms: 10,
        }
    }

    fn context() -> ProjectionPatchBuildContext {
        ProjectionPatchBuildContext {
            sequence: 7,
            llm_request_id: "llm-request-7".to_string(),
            provider: "api".to_string(),
            model: "test-model".to_string(),
            prompt_id: PROJECTION_PATCH_PROMPT_ID.to_string(),
            created_at_ms: 20,
        }
    }

    #[test]
    fn trusted_patch_stamps_runtime_metadata_for_notes() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice mentioned GraphQL."))
            .unwrap();
        let job = job(ProjectionKind::Notes, &ledger);
        let raw = serde_json::json!({
            "operations": [{
                "type": "upsert_note",
                "id": "note:graphql",
                "title": "GraphQL",
                "body": "Alice mentioned GraphQL.",
                "tags": ["topic"]
            }],
            "confidence": 0.82
        })
        .to_string();

        let patch =
            trusted_projection_patch_from_model_json(&raw, &job, context()).expect("valid patch");

        assert_eq!(patch.sequence, 7);
        assert_eq!(patch.kind, ProjectionKind::Notes);
        assert_eq!(patch.basis, job.basis);
        assert_eq!(patch.confidence, 0.82);
        assert_eq!(patch.provenance.provider, "api");
        assert_eq!(patch.provenance.model, "test-model");
        assert_eq!(patch.provenance.prompt_id, PROJECTION_PATCH_PROMPT_ID);
        assert_eq!(patch.llm_request_id, "llm-request-7");
        assert_eq!(patch.created_at_ms, 20);
    }

    #[test]
    fn notes_patch_rejects_graph_operations_before_materialization() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice met Bob."))
            .unwrap();
        let job = job(ProjectionKind::Notes, &ledger);
        let raw = serde_json::json!({
            "operations": [{
                "type": "upsert_graph_node",
                "id": "person:alice",
                "name": "Alice",
                "entity_type": "person",
                "description": null
            }]
        })
        .to_string();

        let err = trusted_projection_patch_from_model_json(&raw, &job, context())
            .expect_err("wrong operation kind");

        assert_eq!(
            err,
            ProjectionPatchDraftError::WrongOperationKind {
                expected: ProjectionKind::Notes,
                operation: "upsert_graph_node",
            }
        );
    }

    #[test]
    fn notes_patch_accepts_reorder_operations() {
        let raw = serde_json::json!({
            "operations": [{
                "type": "reorder_note",
                "id": "note:decision",
                "after_id": null
            }]
        })
        .to_string();

        let draft =
            parse_projection_patch_draft(&raw, &ProjectionKind::Notes).expect("reorder note");

        assert!(matches!(
            draft.operations.first(),
            Some(ProjectionOperation::ReorderNote { id, after_id })
                if id == "note:decision" && after_id.is_none()
        ));
    }

    #[test]
    fn graph_patch_accepts_nodes_and_edges() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice works with Bob."))
            .unwrap();
        let job = job(ProjectionKind::Graph, &ledger);
        let raw = serde_json::json!({
            "operations": [
                {
                    "type": "upsert_graph_node",
                    "id": "person:alice",
                    "name": "Alice",
                    "entity_type": "person",
                    "description": null
                },
                {
                    "type": "upsert_graph_node",
                    "id": "person:bob",
                    "name": "Bob",
                    "entity_type": "person",
                    "description": null
                },
                {
                    "type": "upsert_graph_edge",
                    "id": "edge:alice:bob:works_with",
                    "source": "person:alice",
                    "target": "person:bob",
                    "relation_type": "works_with",
                    "label": "works with",
                    "weight": 0.7
                }
            ],
            "confidence": 0.76
        })
        .to_string();

        let patch =
            trusted_projection_patch_from_model_json(&raw, &job, context()).expect("graph patch");

        assert_eq!(patch.kind, ProjectionKind::Graph);
        assert_eq!(patch.operations.len(), 3);
        assert_eq!(patch.confidence, 0.76);
    }

    #[test]
    fn graph_patch_accepts_retcon_operations() {
        let raw = serde_json::json!({
            "operations": [
                {
                    "type": "invalidate_graph_node",
                    "id": "person:stale"
                },
                {
                    "type": "invalidate_graph_edge",
                    "id": "edge:stale"
                },
                {
                    "type": "strengthen_graph_edge",
                    "id": "edge:alice:bob:works_with",
                    "weight_delta": 0.2
                },
                {
                    "type": "weaken_graph_edge",
                    "id": "edge:alice:bob:reports_to",
                    "weight_delta": 0.4
                },
                {
                    "type": "merge_graph_nodes",
                    "source_id": "person:alias",
                    "target_id": "person:alice"
                },
                {
                    "type": "split_graph_node",
                    "id": "topic:providers",
                    "replacement_nodes": [
                        {
                            "id": "topic:provider-research",
                            "name": "Provider research",
                            "entity_type": "topic",
                            "description": null
                        },
                        {
                            "id": "topic:provider-implementation",
                            "name": "Provider implementation",
                            "entity_type": "topic",
                            "description": "Implementation work"
                        }
                    ]
                }
            ]
        })
        .to_string();

        let draft =
            parse_projection_patch_draft(&raw, &ProjectionKind::Graph).expect("retcon draft");

        assert_eq!(draft.operations.len(), 6);
        assert!(matches!(
            draft.operations.last(),
            Some(ProjectionOperation::SplitGraphNode { id, replacement_nodes })
                if id == "topic:providers" && replacement_nodes.len() == 2
        ));
    }

    #[test]
    fn malformed_json_is_rejected_as_replacement_prose() {
        let err = parse_projection_patch_draft(
            "Alice and Bob are connected. This should be a note.",
            &ProjectionKind::Notes,
        )
        .expect_err("replacement prose is not JSON");

        assert!(matches!(err, ProjectionPatchDraftError::InvalidJson { .. }));
    }

    #[test]
    fn model_supplied_trusted_fields_are_rejected() {
        let raw = serde_json::json!({
            "sequence": 99,
            "basis": {"transcript_hash": "model-owned"},
            "operations": []
        })
        .to_string();

        let err = parse_projection_patch_draft(&raw, &ProjectionKind::Notes)
            .expect_err("model cannot supply trusted metadata");

        assert!(matches!(err, ProjectionPatchDraftError::InvalidJson { .. }));
    }

    #[test]
    fn invalid_graph_edge_weight_is_rejected() {
        let raw = serde_json::json!({
            "operations": [{
                "type": "upsert_graph_edge",
                "id": "edge:bad",
                "source": "person:alice",
                "target": "person:bob",
                "relation_type": "knows",
                "label": null,
                "weight": 1.5
            }]
        })
        .to_string();

        let err = parse_projection_patch_draft(&raw, &ProjectionKind::Graph)
            .expect_err("edge weight must be bounded");

        assert_eq!(
            err,
            ProjectionPatchDraftError::InvalidGraphEdgeWeight {
                id: "edge:bad".to_string(),
                weight: 1.5,
            }
        );
    }

    #[test]
    fn invalid_graph_retcon_drafts_are_rejected() {
        let invalid_delta = serde_json::json!({
            "operations": [{
                "type": "strengthen_graph_edge",
                "id": "edge:bad",
                "weight_delta": 2.0
            }]
        })
        .to_string();
        let err = parse_projection_patch_draft(&invalid_delta, &ProjectionKind::Graph)
            .expect_err("weight delta must be bounded");
        assert_eq!(
            err,
            ProjectionPatchDraftError::InvalidGraphEdgeWeightDelta {
                operation: "strengthen_graph_edge",
                id: "edge:bad".to_string(),
                weight_delta: 2.0,
            }
        );

        let underspecified_split = serde_json::json!({
            "operations": [{
                "type": "split_graph_node",
                "id": "topic:providers",
                "replacement_nodes": [{
                    "id": "topic:provider-research",
                    "name": "Provider research",
                    "entity_type": "topic",
                    "description": null
                }]
            }]
        })
        .to_string();
        let err = parse_projection_patch_draft(&underspecified_split, &ProjectionKind::Graph)
            .expect_err("split needs at least two replacements");
        assert_eq!(
            err,
            ProjectionPatchDraftError::InvalidGraphSplitReplacementCount {
                id: "topic:providers".to_string(),
                count: 1,
            }
        );
    }

    #[test]
    fn duplicate_graph_node_identity_is_rejected_before_materialization() {
        let raw = serde_json::json!({
            "operations": [
                {
                    "type": "upsert_graph_node",
                    "id": "person:alice",
                    "name": "Alice",
                    "entity_type": "person",
                    "description": null
                },
                {
                    "type": "upsert_graph_node",
                    "id": "person:alice",
                    "name": "Alice A.",
                    "entity_type": "person",
                    "description": "duplicate in same patch"
                }
            ]
        })
        .to_string();

        let err = parse_projection_patch_draft(&raw, &ProjectionKind::Graph)
            .expect_err("duplicate graph node id");

        assert_eq!(
            err,
            ProjectionPatchDraftError::DuplicateOperationId {
                operation: "upsert_graph_node",
                id: "person:alice".to_string(),
            }
        );
    }

    #[test]
    fn later_note_context_can_retcon_stable_note_id_without_replacement_prose() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice chose Soniox."))
            .unwrap();
        let first_job = job(ProjectionKind::Notes, &ledger);
        let first_raw = serde_json::json!({
            "operations": [{
                "type": "upsert_note",
                "id": "note:provider-choice",
                "title": "Provider choice",
                "body": "Alice chose Soniox.",
                "tags": ["provider"]
            }]
        })
        .to_string();
        let first_patch =
            trusted_projection_patch_from_model_json(&first_raw, &first_job, context())
                .expect("first note patch");

        ledger
            .apply_event(event(
                "span-1",
                2,
                "Alice chose Soniox for realtime tests, not production.",
            ))
            .unwrap();
        let second_job = job(ProjectionKind::Notes, &ledger);
        let mut second_context = context();
        second_context.sequence = 8;
        let second_raw = serde_json::json!({
            "operations": [{
                "type": "upsert_note",
                "id": "note:provider-choice",
                "title": "Provider choice",
                "body": "Alice chose Soniox for realtime tests, not production.",
                "tags": ["provider", "correction"]
            }],
            "confidence": 0.78
        })
        .to_string();
        let second_patch =
            trusted_projection_patch_from_model_json(&second_raw, &second_job, second_context)
                .expect("retcon note patch");

        assert!(matches!(
            first_patch.operations.first(),
            Some(ProjectionOperation::UpsertNote { id, .. }) if id == "note:provider-choice"
        ));
        assert!(matches!(
            second_patch.operations.first(),
            Some(ProjectionOperation::UpsertNote { id, body, .. })
                if id == "note:provider-choice" && body.contains("not production")
        ));
        assert_ne!(
            first_patch.basis.transcript_hash,
            second_patch.basis.transcript_hash
        );
    }

    #[test]
    fn later_graph_context_can_update_stable_node_id() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Soniox is a candidate."))
            .unwrap();
        let first_job = job(ProjectionKind::Graph, &ledger);
        let first_raw = serde_json::json!({
            "operations": [{
                "type": "upsert_graph_node",
                "id": "provider:soniox",
                "name": "Soniox",
                "entity_type": "provider",
                "description": "Candidate provider."
            }]
        })
        .to_string();
        let first_patch =
            trusted_projection_patch_from_model_json(&first_raw, &first_job, context())
                .expect("first graph patch");

        ledger
            .apply_event(event(
                "span-1",
                2,
                "Soniox is a realtime STT candidate with speaker labels.",
            ))
            .unwrap();
        let second_job = job(ProjectionKind::Graph, &ledger);
        let mut second_context = context();
        second_context.sequence = 8;
        let second_raw = serde_json::json!({
            "operations": [{
                "type": "upsert_graph_node",
                "id": "provider:soniox",
                "name": "Soniox",
                "entity_type": "provider",
                "description": "Realtime STT candidate with speaker labels."
            }],
            "confidence": 0.83
        })
        .to_string();
        let second_patch =
            trusted_projection_patch_from_model_json(&second_raw, &second_job, second_context)
                .expect("updated graph patch");

        assert!(matches!(
            first_patch.operations.first(),
            Some(ProjectionOperation::UpsertGraphNode { id, .. }) if id == "provider:soniox"
        ));
        assert!(matches!(
            second_patch.operations.first(),
            Some(ProjectionOperation::UpsertGraphNode { id, description, .. })
                if id == "provider:soniox"
                    && description
                        .as_deref()
                        .is_some_and(|value| value.contains("speaker labels"))
        ));
        assert_ne!(
            first_patch.basis.transcript_hash,
            second_patch.basis.transcript_hash
        );
    }

    #[test]
    fn prompt_builder_rejects_stale_job_basis() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger.apply_event(event("span-1", 2, "new text")).unwrap();
        let job = ProjectionJob {
            id: "projection:session-1:notes:1".to_string(),
            session_id: "session-1".to_string(),
            kind: ProjectionKind::Notes,
            basis: ProjectionBasis {
                span_revisions: vec![ProjectionBasisSpan {
                    span_id: "span-1".to_string(),
                    revision_number: 1,
                }],
                diarization_span_revisions: Vec::new(),
                transcript_hash: "stale".to_string(),
                summarized_through_revision: None,
            },
            priority: ProjectionPriority::Realtime,
            queued_at_ms: 10,
        };

        let err = projection_patch_prompt_messages(&job, &ledger).expect_err("stale basis");

        assert!(matches!(err, ProjectionPatchDraftError::StaleBasis { .. }));
    }

    #[test]
    fn repair_prompt_includes_validation_error_schema_and_compact_invalid_output() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice met Bob."))
            .unwrap();
        let job = job(ProjectionKind::Notes, &ledger);
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
        let error = ProjectionPatchDraftError::WrongOperationKind {
            expected: ProjectionKind::Notes,
            operation: "upsert_graph_node",
        };

        let messages = projection_patch_repair_prompt_messages(&job, &ledger, &invalid, &error)
            .expect("repair prompt");

        // Static→dynamic base prompt is 4 messages (system, stable-context,
        // hot-buffer transcript, per-tick metadata); the repair pass appends the
        // invalid assistant turn + the correction user turn.
        assert_eq!(messages.len(), 6);
        assert_eq!(messages[4].role, "assistant");
        assert!(messages[4].content.contains("upsert_graph_node"));
        assert_eq!(messages[5].role, "user");
        assert!(messages[5].content.contains("expected_kind: notes"));
        assert!(messages[5].content.contains("validation_error:"));
        assert!(messages[5].content.contains("upsert_graph_node"));
        assert!(messages[5].content.contains("Output JSON schema"));
        assert!(
            messages[5]
                .content
                .contains("Do not include trusted metadata")
        );
    }

    /// The rolling-summary window feeds only the last K turns verbatim; older
    /// turns collapse into the summary block, and the summary is never rebuilt
    /// from scratch (ADR-0025 §2c / seed audio-graph-18ee).
    #[test]
    fn windowed_prompt_summarizes_older_turns_and_feeds_hot_buffer_verbatim() {
        let mut ledger = TranscriptLedger::new("session-1");
        let total = ROLLING_SUMMARY_HOT_WINDOW_TURNS + 4;
        for i in 0..total {
            ledger
                .apply_event(event(
                    &format!("span-{i}"),
                    1,
                    &format!("Turn {i} content about topic {i}"),
                ))
                .unwrap();
        }
        let job = job(ProjectionKind::Notes, &ledger);
        let messages = projection_patch_prompt_messages(&job, &ledger).expect("windowed prompt");

        // The hot-buffer transcript block (message 2) carries only the last K
        // turns verbatim, not all of them.
        let transcript_block = &messages[2].content;
        assert!(transcript_block.contains(&format!("Turn {} content", total - 1)));
        assert!(
            !transcript_block.contains("Turn 0 content"),
            "oldest turn must not be fed verbatim once it leaves the hot window"
        );

        // The summary block (message 1) covers the oldest turn.
        let summary_block = &messages[1].content;
        assert!(summary_block.contains("Turn 0 content"));

        // The basis records the summarized-through boundary and the ledger
        // still validates the current basis (windowing stays sound).
        assert!(job.basis.summarized_through_revision.is_some());
        assert!(ledger.validate_basis(&job.basis).is_ok());
    }

    /// Incremental fold (turn-by-turn) is byte-identical to a single pass — the
    /// summary is a pure function of the older turns, so it is never rebuilt.
    #[test]
    fn rolling_summary_incremental_fold_matches_single_pass() {
        let mut ledger = TranscriptLedger::new("session-1");
        for i in 0..(ROLLING_SUMMARY_HOT_WINDOW_TURNS + 3) {
            ledger
                .apply_event(event(&format!("span-{i}"), 1, &format!("Utterance {i}")))
                .unwrap();
        }
        let events = ledger.latest_spans.clone();
        let (older, _hot) = split_summary_window(&events);

        // Single pass over all older turns.
        let single = RollingSummary::from_older_turns(&older);

        // Incremental fold, one turn at a time (never touches prior lines).
        let mut incremental = RollingSummary::default();
        for turn in &older {
            incremental.fold_leaving_turn(turn);
        }

        assert_eq!(single, incremental);
        assert_eq!(single.render(), incremental.render());
        assert_eq!(
            single.summarized_through_revision(),
            job(ProjectionKind::Notes, &ledger)
                .basis
                .summarized_through_revision
        );
    }

    /// The leading blocks form a byte-stable prefix across submissions — the
    /// prompt-cache win only materializes if the prefix is byte-identical
    /// (ADR-0025 §2d / seed audio-graph-d77e).
    #[test]
    fn stable_prefix_is_byte_identical_across_appended_turns() {
        let mut ledger = TranscriptLedger::new("session-1");
        for i in 0..(ROLLING_SUMMARY_HOT_WINDOW_TURNS + 2) {
            ledger
                .apply_event(event(&format!("span-{i}"), 1, &format!("Utterance {i}")))
                .unwrap();
        }
        let first_job = job(ProjectionKind::Notes, &ledger);
        let first = projection_patch_prompt_messages(&first_job, &ledger).expect("first prompt");

        // Append a brand-new turn. Because the new turn enters the hot buffer
        // and pushes the oldest one into the (append-only) summary, the stable
        // prefix (system block) must stay byte-identical.
        ledger
            .apply_event(event("span-new", 1, "A fresh turn arrives"))
            .unwrap();
        let second_job = job(ProjectionKind::Notes, &ledger);
        let second = projection_patch_prompt_messages(&second_job, &ledger).expect("second prompt");

        assert_eq!(PROJECTION_STABLE_PREFIX_MESSAGE_COUNT, 2);
        // Message 0 (system block: instructions + guidance + schema) is the
        // cache anchor — it must be byte-identical across turns.
        assert_eq!(
            first[0].content, second[0].content,
            "system block must be byte-identical across turns (cache anchor)"
        );
        // Message 1 (pinned facts + rolling summary) is append-only: the earlier
        // turn's block is a byte-prefix of the later one, so the longest-common-
        // prefix cache still hits up to the breakpoint.
        assert!(
            second[1].content.starts_with(&first[1].content),
            "stable-context block must grow append-only, never rewrite prior bytes"
        );
        // The per-tick metadata (last message) is expected to differ (basis hash).
        assert_ne!(
            first.last().map(|m| &m.content),
            second.last().map(|m| &m.content)
        );
    }

    // ----- a324: provider-strict structured-output schema -------------------

    /// Collect the `type` const of every operation variant offered in a strict
    /// schema for `kind`. Used to assert kind-partitioning.
    fn strict_schema_operation_types(kind: &ProjectionKind) -> Vec<String> {
        let schema = projection_patch_strict_json_schema(kind);
        schema["properties"]["operations"]["items"]["anyOf"]
            .as_array()
            .expect("operation variants are an anyOf array")
            .iter()
            .map(|variant| {
                variant["properties"]["type"]["enum"][0]
                    .as_str()
                    .expect("each variant pins its type const")
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn strict_schema_partitions_operations_by_kind() {
        // The validator rejects a graph op in a notes job (and vice-versa). The
        // strict schema must forbid the model from emitting one at all, so it is
        // not looser than the validator on the kind axis (audio-graph-a324).
        let notes_ops = strict_schema_operation_types(&ProjectionKind::Notes);
        assert!(notes_ops.contains(&"upsert_note".to_string()));
        assert!(notes_ops.contains(&"delete_note".to_string()));
        assert!(notes_ops.contains(&"reorder_note".to_string()));
        assert!(
            notes_ops.iter().all(|op| !op.contains("graph")),
            "notes schema must not offer any graph op, got: {notes_ops:?}"
        );

        let graph_ops = strict_schema_operation_types(&ProjectionKind::Graph);
        assert!(graph_ops.contains(&"upsert_graph_node".to_string()));
        assert!(graph_ops.contains(&"upsert_graph_edge".to_string()));
        assert!(graph_ops.contains(&"split_graph_node".to_string()));
        assert!(
            graph_ops.iter().all(|op| !op.ends_with("_note")),
            "graph schema must not offer any note op, got: {graph_ops:?}"
        );
    }

    #[test]
    fn strict_schema_requires_every_operation_field() {
        // The user's failures were patches MISSING structural fields (id/title/
        // tags for notes). The schema must list all serde fields of a variant in
        // `required` with additionalProperties:false, so a schema-obeying model
        // cannot omit them (audio-graph-a324).
        let schema = projection_patch_strict_json_schema(&ProjectionKind::Notes);
        let upsert = schema["properties"]["operations"]["items"]["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["properties"]["type"]["enum"][0] == "upsert_note")
            .expect("upsert_note variant present");

        let required: BTreeSet<&str> = upsert["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for field in ["type", "id", "title", "body", "tags"] {
            assert!(
                required.contains(field),
                "upsert_note must require `{field}`, required = {required:?}"
            );
        }
        assert_eq!(
            upsert["additionalProperties"].as_bool(),
            Some(false),
            "variants must be closed objects for strict mode"
        );
    }

    /// The load-bearing strictness claim: a patch that OBEYS the strict schema
    /// (all required fields present, correct kind) also PASSES the runtime
    /// validator. If the schema were looser than the validator on structure or
    /// kind, a schema-valid fixture could still fail validation — this asserts it
    /// does not, for a representative notes and graph patch (audio-graph-a324).
    #[test]
    fn schema_valid_fixture_passes_the_runtime_validator() {
        // Notes: every upsert_note field present.
        let notes_fixture = serde_json::json!({
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
        parse_projection_patch_draft(&notes_fixture, &ProjectionKind::Notes)
            .expect("a schema-obeying notes patch must pass the validator");

        // Graph: node + edge, every field present, weight in range.
        let graph_fixture = serde_json::json!({
            "operations": [
                {
                    "type": "upsert_graph_node",
                    "id": "person:alice",
                    "name": "Alice",
                    "entity_type": "person",
                    "description": null
                },
                {
                    "type": "upsert_graph_edge",
                    "id": "edge:alice-bob",
                    "source": "person:alice",
                    "target": "person:bob",
                    "relation_type": "met",
                    "label": null,
                    "weight": 0.5
                }
            ],
            "confidence": null
        })
        .to_string();
        parse_projection_patch_draft(&graph_fixture, &ProjectionKind::Graph)
            .expect("a schema-obeying graph patch must pass the validator");
    }

    #[test]
    fn strict_schema_serializes_as_a_closed_object_with_required_operations() {
        let schema = projection_patch_strict_json_schema(&ProjectionKind::Graph);
        assert_eq!(schema["type"].as_str(), Some("object"));
        assert_eq!(schema["additionalProperties"].as_bool(), Some(false));
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(required.contains(&"operations"));
        assert!(
            required.contains(&"confidence"),
            "strict mode requires every top-level property to be listed, including the nullable confidence"
        );
    }
}
