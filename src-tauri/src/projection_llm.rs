//! Structured LLM output contract for notes/graph projection patches.
//!
//! This module owns prompt construction, model-output parsing, and trusted
//! patch construction. Runtime scheduler dispatch calls into this module from
//! live ASR ingestion (`llm/executor.rs`'s `run_projection_patch_dispatch`,
//! reached from `speech/mod.rs`'s `run_projection_job` on every basis-bound
//! projection tick). As of seed audio-graph-253c part 2, that live dispatch
//! path also threads the real `MaterializedNotes` state through: each Notes-
//! kind tick clones the current materialization under `AppState`'s
//! `materialized_projection_state` lock (`run_projection_job`), carries it
//! through `LlmJob::ProjectionPatch` -> `generate_projection_patch` ->
//! `run_projection_patch_dispatch`, and passes `Some(&snapshot)` into the
//! builders below. `None` still reaches these builders in two cases: a
//! Graph-kind job (which never renders the notes block regardless), or a
//! Notes-kind snapshot whose session id did not match the job's own (see
//! `ProjectionRuntimeHandle::materialized_notes_snapshot_for_session`) — both
//! omit the "Current notes state" block rather than asserting a fabricated
//! one.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use schemars::JsonSchema;

use crate::llm::engine::ChatMessage;
use crate::projections::{
    GraphNodeDraft, MaterializedNote, MaterializedNotes, ProjectionBasisStaleness, ProjectionJob,
    ProjectionKind, ProjectionOperation, ProjectionPatch, ProjectionProvenance,
    ROLLING_SUMMARY_HOT_WINDOW_TURNS, TranscriptEvent, TranscriptLedger, ordered_for_window,
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

/// ADR-0037: every content-creating operation's `evidence` field is judged
/// against this prompt's transcript window, never trusted
/// (`claim_evidence::judge_claim_evidence`). Before this text existed, the
/// system prompt never mentioned evidence, claim classes, or span ids at all
/// — the ONLY hint a schema-obeying model had was an optional field with a
/// `knowledge_gap` default (the class the judge always refuses), so a model
/// on any non-strict route either omitted `evidence` outright or fabricated
/// a class that happened to bypass the check. This is part of the system
/// message (the byte-stable prefix, `PROJECTION_STABLE_PREFIX_MESSAGE_COUNT`),
/// so the repair prompt (`projection_patch_repair_prompt_messages`, which
/// extends this same message list) inherits it without needing its own copy.
const EVIDENCE_GUIDANCE: &str = "Every upsert_note, upsert_graph_node, and upsert_graph_edge \
     operation requires an `evidence` object anchoring the claim to a span in this prompt's \
     transcript window (see `span_id` in the transcript JSON below). `claim_class` is one of: \
     `verified_quote` (set `span_id` to one of those span ids and `quote` to a literal, verbatim \
     substring of that span's text), `grounded_inference` (set `span_id` to one of those span \
     ids; no quote needed), or `unavailable_evidence` (set `span_id` to the closest relevant span \
     id AND `note` to a short explanation of what deeper evidence is missing, e.g. audio was not \
     retained for that span). Never use `knowledge_gap` — it is never admitted. A `span_id` \
     outside the transcript window below is refused.";

/// Max characters of a single older turn kept in the rolling summary digest.
/// Bounds each folded turn's contribution so the summary stays far smaller than
/// the full transcript JSON it replaces.
const SUMMARY_TURN_DIGEST_MAX_CHARS: usize = 160;

/// Max number of existing notes rendered into the Notes-kind prompt's live
/// notes-state snapshot (seed audio-graph-253c). Selection is by most-recently-
/// updated (`updated_by_sequence` descending); a session that has accumulated
/// more notes than this still gets a truncated block plus a count line, rather
/// than an unbounded prompt.
const NOTES_SNAPSHOT_MAX_ENTRIES: usize = 30;

/// Max characters of a single note's body kept in the one-line summary the
/// live notes-state snapshot renders per note. Mirrors
/// `SUMMARY_TURN_DIGEST_MAX_CHARS`'s bounding posture (applied to note bodies
/// instead of transcript turns) so one long note can never dominate the block.
const NOTES_SNAPSHOT_BODY_SUMMARY_MAX_CHARS: usize = 160;

/// Max characters of a single note's `id` or `title` kept in the live
/// notes-state snapshot. Unlike the body, `id` and `title` are model-authored
/// `String`s with no length validation anywhere on the apply path (`upsert_note`
/// accepts them verbatim), so without this cap one oversized title/id would
/// make the per-note line — and therefore the whole block — unbounded despite
/// [`NOTES_SNAPSHOT_MAX_ENTRIES`] and [`NOTES_SNAPSHOT_BODY_SUMMARY_MAX_CHARS`]
/// bounding the other two axes. Shorter than the body cap because an id/title
/// is expected to be short prose, not a paragraph.
const NOTES_SNAPSHOT_ID_TITLE_MAX_CHARS: usize = 80;

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
    /// The registry provider id the route was authorized against (ADR-0033).
    pub provider: String,
    pub model: String,
    /// Whether `model` is the served id or the requested one (ADR-0038 defect 3).
    pub model_source: crate::llm::route::ModelIdentitySource,
    /// The stamped route id, supplied by trusted code only.
    pub route_id: Option<String>,
    /// Content-free route evidence for this patch.
    pub route: Option<crate::llm::route::RouteRecord>,
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
    /// ADR-0037: `operation`'s `EvidenceAnchor` did not satisfy its declared
    /// `ClaimClass`'s evidence minimum. Fails the WHOLE patch, through the
    /// same all-or-nothing per-patch validation loop every other structural
    /// check here already uses.
    ClaimEvidenceRefused {
        operation: &'static str,
        id: String,
        deficiency: crate::claim_evidence::ClaimEvidenceDeficiency,
    },
    /// ADR-0037 part 4: "corrections and retractions are derived, not
    /// model-authored". `operation` is only ever legitimate coming from
    /// trusted, ADR-0031-derived code — never from a model-submitted draft.
    /// Fails the WHOLE patch, same as every other structural check here.
    DerivedOnlyOperation {
        operation: &'static str,
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
            Self::ClaimEvidenceRefused {
                operation,
                id,
                deficiency,
            } => write!(
                f,
                "{operation} {id} refused claim evidence admission: {deficiency}"
            ),
            Self::DerivedOnlyOperation { operation } => write!(
                f,
                "{operation} may only be derived by trusted code, never model-authored (ADR-0037 part 4)"
            ),
        }
    }
}

impl std::error::Error for ProjectionPatchDraftError {}

pub fn projection_patch_draft_json_schema() -> Result<serde_json::Value, String> {
    let mut schema = serde_json::to_value(schemars::schema_for!(ProjectionPatchDraft))
        .map_err(|e| format!("failed to build projection patch draft JSON schema: {e}"))?;
    require_evidence_on_content_creating_operation_variants(&mut schema);
    hide_heading_level_from_draft_schema(&mut schema);
    Ok(schema)
}

/// audio-graph-a6b5 W1 is a DARK ship: `heading_level` is declared on
/// [`ProjectionOperation::UpsertNote`] (so fresh-ingest and replay can carry
/// it end-to-end), but no model-facing surface may change until W2's
/// dedicated prompt/schema-exposure ticket. `schemars` derives this draft
/// schema straight from the Rust type, so without this post-process step
/// `heading_level` — AND the field's entire internal doc comment, verbatim,
/// as the property's JSON Schema `description` (ticket IDs, ADR numbers,
/// internal test-name cross-references included) — would be pasted into
/// every projection system prompt ([`projection_patch_prompt_messages`],
/// [`projection_patch_repair_prompt_messages`]) and offered as the
/// vLLM/mistral.rs structured-decoding grammar
/// (`llm::executor::projection_api`). That is exactly the coupling this
/// ticket was told to avoid rather than expose ("if adding the field to the
/// draft type inevitably exposes it in the wire schema W2 owns, STOP and
/// report the coupling rather than exposing it early"): the coupling is
/// avoidable here, so it is stripped, not shipped.
///
/// This is the same seam [`require_evidence_on_content_creating_operation_variants`]
/// already uses to tighten this schema post-generation, and it does not
/// weaken serde's tolerance for a model that emits the key anyway —
/// `#[serde(default)]` on the field means an unadvertised `heading_level` key
/// is still silently accepted (never rejected) on any route; this function
/// only controls what the schema *advertises*, not what deserialization
/// *tolerates*. W2 deletes this call (or narrows it) when it flips the field
/// model-visible.
fn hide_heading_level_from_draft_schema(schema: &mut serde_json::Value) {
    let Some(variants) = schema
        .get_mut("$defs")
        .and_then(|defs| defs.get_mut("ProjectionOperation"))
        .and_then(|operation| operation.get_mut("oneOf"))
        .and_then(|one_of| one_of.as_array_mut())
    else {
        return;
    };

    for variant in variants {
        let had_heading_level = variant
            .get_mut("properties")
            .and_then(|properties| properties.as_object_mut())
            .map(|properties| properties.remove("heading_level").is_some())
            .unwrap_or(false);
        if !had_heading_level {
            continue;
        }
        if let Some(required) = variant.get_mut("required").and_then(|r| r.as_array_mut()) {
            required.retain(|field| field != "heading_level");
        }
    }
}

/// `schemars` marks `evidence` OPTIONAL on the three content-creating
/// `ProjectionOperation` variants, with a `"default": {"claim_class":
/// "knowledge_gap"}` — exactly the one class `judge_claim_evidence` refuses
/// unconditionally (ADR-0037). This is `#[serde(default)]`'s doing
/// (schemars_derive-1.2.1 schema_exprs.rs:726-740: the DESERIALIZE-contract
/// `is_optional` expression ORs in `has_default` unconditionally, so even a
/// `#[schemars(required)]` override cannot win), and `#[serde(default)]`
/// itself cannot be removed from `ProjectionOperation` — it is the backward-
/// compat fallback `pre_contract_projection_patch_fixture_still_deserializes`
/// pins for a `ProjectionPatch` persisted before this contract, with no
/// `evidence` key at all. This schema, not the Rust type, is what actually
/// reaches the model — it is pasted into every projection prompt
/// (`projection_patch_prompt_messages`) and sent as the vLLM/mistral.rs
/// structured-decoding grammar (`llm::executor::projection_api`) — so
/// post-processing it here is the only seam that tightens the model-facing
/// contract without weakening historical-deserialization tolerance.
fn require_evidence_on_content_creating_operation_variants(schema: &mut serde_json::Value) {
    let Some(variants) = schema
        .get_mut("$defs")
        .and_then(|defs| defs.get_mut("ProjectionOperation"))
        .and_then(|operation| operation.get_mut("oneOf"))
        .and_then(|one_of| one_of.as_array_mut())
    else {
        return;
    };

    for variant in variants {
        let carries_evidence = variant
            .get("properties")
            .and_then(|properties| properties.get("evidence"))
            .is_some();
        if !carries_evidence {
            continue;
        }

        if let Some(evidence) = variant
            .get_mut("properties")
            .and_then(|properties| properties.get_mut("evidence"))
            .and_then(|evidence| evidence.as_object_mut())
        {
            // Advertising `knowledge_gap` as the field's default is exactly
            // the always-refused fallback a schema-obeying model should
            // never be nudged toward.
            evidence.remove("default");
        }

        if let Some(required) = variant.get_mut("required").and_then(|r| r.as_array_mut())
            && !required.iter().any(|field| field == "evidence")
        {
            required.push(serde_json::Value::String("evidence".to_string()));
        }
    }
}

/// Human-authored, provider-strict JSON schema for a projection patch draft,
/// scoped to `kind` (seed audio-graph-a324).
///
/// This is the schema sent as OpenRouter structured-outputs
/// (`response_format: json_schema, strict: true`) so a schema-capable model is
/// constrained at generation time. It differs from
/// [`projection_patch_draft_json_schema`] (the `schemars`-derived shape the
/// vLLM/mistral.rs paths use) in four deliberate ways that make it a good fit
/// for OpenAI/OpenRouter strict mode and **at least as strict as the runtime
/// validator** ([`validate_projection_patch_draft`]):
///
/// 1. **Kind partitioning.** Only the operation variants that
///    [`operation_kind`] maps to `kind` are offered. The validator rejects a
///    graph op in a notes job (and vice-versa); the schema now forbids the
///    model from emitting one at all, so it is not looser than the validator on
///    the kind axis.
/// 2. **Every operation field the model is meant to fill in is required.**
///    The user's failures were patches *missing* structural fields (`id` /
///    `title` / `tags` for notes, `relations`/`target`/`name` for graph
///    edges). serde requires those fields; the derived schema left them
///    optional, so the model produced field-incomplete patches that only
///    failed at parse time. Here each variant lists its serde fields in
///    `required` with `additionalProperties: false`, matching the
///    internally-tagged wire shape — with ONE deliberate exception:
///    `upsert_note`'s `heading_level` (audio-graph-a6b5 W1) is a real serde
///    field on `ProjectionOperation::UpsertNote` that this schema does NOT
///    list at all, dark-shipped until W2's prompt/schema-exposure ticket (see
///    the field's own doc comment in `projections.rs`). Rust `Option` fields
///    that ARE advertised (`description`, `after_id`, `label`) stay required
///    but nullable (`["string", "null"]`) so strict mode is satisfied without
///    forcing the model to invent a value.
/// 3. **No numeric range / non-empty keywords.** The validator additionally
///    enforces `weight`/`confidence` in `0.0..=1.0` and non-empty trimmed
///    strings. Those are intentionally NOT encoded here: several strict-mode
///    engines reject `minimum`/`maximum`/`minLength`, which would turn every
///    request into a 400. They stay the validator's job (and the repair path's).
///    That makes the schema marginally looser than the validator on ranges only
///    — never on structure or kind, which is where the failures were.
/// 4. **The claim-evidence class is an unconstrained string, not an inlined
///    `enum` (ADR-0037).** `upsert_note` / `upsert_graph_node` /
///    `upsert_graph_edge` each gain an `evidence` object whose `claim_class`
///    is typed `string`, not `{"enum": [...]}` — the same "ranges/kind-lists
///    stay the validator's job" posture as point 3, applied to a new field
///    instead of an existing one. `judge_claim_evidence` is the sole judge of
///    whether a class is recognized and satisfied; the schema only pins the
///    anchor's shape. This is scoped to the three content-creating Upsert*
///    variants ONLY — the seven delete/remove/invalidate/strengthen/weaken/
///    merge/split variants are untouched, so their budget is not spent on a
///    field they have no evidence requirement for.
pub fn projection_patch_strict_json_schema(kind: &ProjectionKind) -> serde_json::Value {
    use serde_json::json;

    fn string() -> serde_json::Value {
        json!({ "type": "string" })
    }
    fn nullable_string() -> serde_json::Value {
        json!({ "type": ["string", "null"] })
    }
    // seed audio-graph-e700 sub-fix 1 (SCHEMA BINDING): `entity_type` is
    // bound to `ontology::ENTITY_TYPES`'s closed ten-name enum at the
    // schema level, so a schema-capable/strict-mode provider cannot emit an
    // invented category at all — the field evidence behind this ticket
    // measured 31 invented entity types in one session's projection graph.
    // `relation_type` deliberately stays `string()` (see the plain
    // `string()` calls on the `upsert_graph_edge` variant below): this
    // ticket's own instruction was to investigate whether the ontology
    // defines a CLOSED relation set before binding it the same way, and
    // `RELATION_TYPES`'s own doc comment used to answer that with a
    // self-contradiction (it opened "Closed set of relation types" and then
    // immediately said the model "may emit another lowercase verb phrase
    // when none fit"). Resolved in favor of NOT closed, confirmed against
    // every actual caller (`extraction_system_prompt`'s prompt guidance and
    // the absence of any downstream rejection of a novel relation string),
    // and the doc comment on `RELATION_TYPES` has been corrected to match.
    // Binding `relation_type` to an enum here would fabricate a closed
    // relation ontology this ticket was explicitly told not to invent.
    // Anything that still slips through this enum on a non-strict
    // route (or a strict-mode provider that ignores it) is caught by the
    // deterministic ingest-side fallback,
    // `normalize_projection_patch_draft_ontology` below.
    fn entity_type_enum() -> serde_json::Value {
        json!({
            "type": "string",
            "enum": crate::ontology::ENTITY_TYPES
                .iter()
                .map(|t| t.name)
                .collect::<Vec<_>>(),
        })
    }
    fn string_array() -> serde_json::Value {
        json!({ "type": "array", "items": { "type": "string" } })
    }
    fn number() -> serde_json::Value {
        json!({ "type": "number" })
    }

    // Compact evidence-anchor object (ADR-0037), added ONLY to the three
    // content-creating Upsert* variants below — the seven delete/remove/
    // invalidate/strengthen/weaken/merge/split variants are untouched, so
    // their existing per-variant budget is not spent on a field they have no
    // evidence requirement for. Every `EvidenceAnchor` field is listed in
    // `required` (nullable where optional) to satisfy strict mode exactly
    // like every other variant field here; `claim_class` is NOT restricted
    // to an inlined `enum` list, matching this schema's existing posture of
    // staying "marginally looser than the validator … never on structure or
    // kind" (see the doc comment above) — `judge_claim_evidence` is what
    // actually rejects an unrecognized or unsatisfied class, not the schema.
    fn evidence() -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "claim_class": string(),
                "span_id": nullable_string(),
                "quote": nullable_string(),
                "note": nullable_string(),
            },
            "required": ["claim_class", "span_id", "quote", "note"],
            "additionalProperties": false,
        })
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
            "entity_type": entity_type_enum(),
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
                    ("evidence", evidence()),
                ],
            ),
            variant("delete_note", &[("id", string())]),
            // `invalidate_note` is deliberately NOT offered here (ADR-0037
            // part 4: corrections/retractions are derived, not
            // model-authored) — `validate_operation` refuses it wholesale if
            // a non-strict route's model emits it anyway; see
            // `ProjectionOperation::InvalidateNote`'s doc comment.
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
                    ("entity_type", entity_type_enum()),
                    ("description", nullable_string()),
                    ("evidence", evidence()),
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
                    ("evidence", evidence()),
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

/// `basis` is the basis-covered `TranscriptEvent` map (ADR-0037) each
/// content-creating operation's `EvidenceAnchor` is judged against — build it
/// once via [`basis_events`] and share it across the whole draft. Callers
/// with nothing evidence-anchored to validate (or no ledger available) may
/// pass an empty map.
pub fn parse_projection_patch_draft(
    raw: &str,
    expected_kind: &ProjectionKind,
    basis: &BTreeMap<&str, &TranscriptEvent>,
) -> Result<ProjectionPatchDraft, ProjectionPatchDraftError> {
    let mut draft: ProjectionPatchDraft =
        serde_json::from_str(raw).map_err(|error| ProjectionPatchDraftError::InvalidJson {
            error: error.to_string(),
        })?;
    validate_projection_patch_draft(&draft, expected_kind, basis)?;
    // seed audio-graph-e700: normalize AFTER structural validation (so
    // `require_non_empty`'s emptiness check still sees the model's raw
    // string, not a fallback that would mask a genuinely empty field) and
    // BEFORE the draft's operations are trusted into a persisted
    // `ProjectionPatch` — every FRESH-ingest event this backend writes from
    // here on carries a canonical `entity_type` and a soft-normalized
    // `relation_type`, regardless of which route produced it (strict-schema
    // enum binding above only covers the OpenRouter strict-mode schema; this
    // catches everything else, including anything that slips past that
    // enum). Never applied to replay — see `ontology::normalize_entity_type`
    // and `normalize_projection_patch_draft_ontology`'s doc comments.
    normalize_projection_patch_draft_ontology(&mut draft);
    // audio-graph-a6b5 W1: same fresh-ingest-only seam, same "clamp, never
    // refuse" posture, applied to the new `heading_level` field and the
    // note-body grammar. See `normalize_projection_patch_draft_doc_structure`'s
    // doc comment for why this can never move into `validate_operation` or
    // onto the replay path.
    normalize_projection_patch_draft_doc_structure(&mut draft);
    // `require_non_empty` (called by `validate_operation` above, BEFORE
    // normalization) guarantees every `body` was non-empty as the model
    // wrote it — but `normalize_doc_body` can reduce an all-markup body
    // (e.g. a lone "*", "`", "```", or "#") to the empty string, silently
    // undoing that guarantee. Re-check post-normalization and fail the same
    // way `require_non_empty` would have, rather than persisting a
    // content-free note into the canonical log.
    require_non_empty_body_survives_normalization(&draft)?;
    Ok(draft)
}

/// See [`parse_projection_patch_draft`]'s call site for why this exists: it
/// restores `require_non_empty`'s non-empty-`body` guarantee AFTER
/// [`normalize_projection_patch_draft_doc_structure`] has had a chance to
/// collapse an all-markup body down to nothing.
fn require_non_empty_body_survives_normalization(
    draft: &ProjectionPatchDraft,
) -> Result<(), ProjectionPatchDraftError> {
    for operation in &draft.operations {
        if let ProjectionOperation::UpsertNote { body, .. } = operation {
            require_non_empty(operation, "body", body)?;
        }
    }
    Ok(())
}

/// Ingest-time ontology normalization for a freshly-parsed, structurally
/// valid model draft (seed audio-graph-e700). See
/// [`parse_projection_patch_draft`]'s call site for exactly when this runs.
/// Deliberately NOT applied inside `MaterializedGraph::apply_patch` /
/// replay: a session's pre-e700 events keep whatever free-string
/// `entity_type`/`relation_type` values they were persisted with forever;
/// replay never re-validates or rewrites historical operations (ADR-0045:
/// "no accepted patch may be silently discarded"; "materialized state
/// rebuilt from the accepted patch log" — materialization is a pure
/// re-derivation from the accepted patch log, not a place that mutates that
/// log's content. ADR-0029 is about a DIFFERENT thing — gating optional,
/// separately-rebuildable query indexes on measured demand — and is not
/// cited here).
fn normalize_projection_patch_draft_ontology(draft: &mut ProjectionPatchDraft) {
    for operation in &mut draft.operations {
        match operation {
            ProjectionOperation::UpsertGraphNode { entity_type, .. } => {
                *entity_type = crate::ontology::normalize_entity_type(entity_type).to_string();
            }
            ProjectionOperation::UpsertGraphEdge { relation_type, .. } => {
                *relation_type = crate::ontology::normalize_relation_type(relation_type);
            }
            ProjectionOperation::SplitGraphNode {
                replacement_nodes, ..
            } => {
                for replacement in replacement_nodes {
                    replacement.entity_type =
                        crate::ontology::normalize_entity_type(&replacement.entity_type)
                            .to_string();
                }
            }
            _ => {}
        }
    }
}

/// Lower bound of the document heading-depth scale (audio-graph-a6b5 design
/// panel, design-b §1.2: 2 = top-level section, matching an `<h2>`).
const HEADING_LEVEL_MIN: u8 = 2;
/// Upper bound (4 = sub-subsection, matching an `<h4>`); design-b §1.2 caps
/// nesting at two levels below top-level rather than growing the scale
/// further.
const HEADING_LEVEL_MAX: u8 = 4;

/// Ingest-time doc-structure normalization for a freshly-parsed, structurally
/// valid model draft (audio-graph-a6b5 W1). Sibling of
/// [`normalize_projection_patch_draft_ontology`] at the exact same seam —
/// same call site, same "runs after structural validation, before the draft
/// is trusted into a persisted `ProjectionPatch`, and NEVER on replay"
/// contract, for the identical ADR-0045 reason: materialization is a pure
/// re-derivation from the accepted patch log, so rewriting a HISTORICAL
/// operation's `body`/`heading_level` here would make replay depend on
/// today's normalization rules instead of the rules in force when that
/// patch was accepted.
///
/// Two responsibilities, both **clamp, never refuse** (design-b §1.4: an
/// operation carrying an out-of-range heading or a stray markdown marker is
/// still real content — refusing the whole patch over formatting is exactly
/// the ADR-0045 trade this ticket forbids):
///
/// 1. Clamp `heading_level` into `2..=4` when present. `None` (no structure
///    asserted) passes through unchanged — clamping is not a license to
///    invent a depth for an operation that asserted none.
/// 2. Normalize `body` into the validated plain-line/bullet grammar
///    (design-b §1.3): `*`/`+` bullet markers become `-`; bullet indent
///    snaps to 0/2/4 spaces (two nesting levels); a leading run of `#`
///    markers is stripped (headings live in `title`, never in `body`);
///    inline emphasis/link/code-fence markers are stripped down to their
///    plain text. This is deliberately NOT an HTML sanitizer: `body` stays
///    an opaque `String` all the way through this backend, and the
///    XSS-safety property this grammar exists for comes from the FRONTEND
///    renderer parsing it into React text nodes with no HTML path at all
///    (design-b §1.3) — this function's job is only to keep the markup
///    *vocabulary* narrow, never to escape or interpret arbitrary text.
///    Every line survives: no line is ever dropped or truncated here,
///    regardless of length or content (a `<script>` tag or a markdown link
///    is rewritten/stripped of its markup exactly like any other line, never
///    discarded).
fn normalize_projection_patch_draft_doc_structure(draft: &mut ProjectionPatchDraft) {
    for operation in &mut draft.operations {
        if let ProjectionOperation::UpsertNote {
            heading_level,
            body,
            ..
        } = operation
        {
            *heading_level =
                heading_level.map(|level| level.clamp(HEADING_LEVEL_MIN, HEADING_LEVEL_MAX));
            *body = normalize_doc_body(body);
        }
    }
}

/// Rewrites `body` into design-b §1.3's validated grammar, one line at a
/// time. See [`normalize_projection_patch_draft_doc_structure`] for the
/// contract this implements (fresh-ingest-only, clamp-never-refuse,
/// no line ever dropped).
fn normalize_doc_body(body: &str) -> String {
    // `str::lines` drops a trailing newline distinction the grammar does not
    // care about (a body's line count, not its trailing-newline byte, is
    // what the renderer walks); rejoining with `\n` below reconstructs a
    // normal multi-line body.
    body.lines()
        .map(normalize_doc_body_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_doc_body_line(line: &str) -> String {
    // Leading tabs count as indentation too (a tab-indented bullet used to
    // fall through to the paragraph branch below, losing its bullet marker
    // entirely: `strip_bullet_marker` only ever saw `rest` starting with the
    // literal tab). Weight a tab as 2 columns — the same width as one
    // nesting level — so a single leading tab maps to depth 1 exactly like
    // two leading spaces do.
    let leading_ws_len = line.len() - line.trim_start_matches([' ', '\t']).len();
    let leading_ws = &line[..leading_ws_len];
    let indent_width: usize = leading_ws
        .chars()
        .map(|c| if c == '\t' { 2 } else { 1 })
        .sum();
    let rest = &line[leading_ws_len..];

    if let Some(text) = strip_bullet_marker(rest) {
        // Two nesting levels below the top, per design-b §1.3's grammar
        // (`indent ∈ {"", "  ", "    "} → depth 0/1/2`); anything deeper is
        // clamped to depth 2 rather than growing indentation further.
        let depth = (indent_width / 2).min(2);
        let indent = "  ".repeat(depth);
        return format!("{indent}- {}", strip_doc_body_markup(text.trim_start()));
    }

    // Headings live in `title`, never in `body` — strip a leading run of
    // `#` markers from a paragraph line without dropping the line itself.
    let without_heading_marker = rest.trim_start_matches('#');
    let text = if without_heading_marker.len() != rest.len() {
        without_heading_marker.trim_start()
    } else {
        rest
    };
    strip_doc_body_markup(text)
}

/// `- `/`* `/`+ ` at the start of a (whitespace-trimmed) line all mean
/// "bullet" under design-b §1.3's grammar; only `-` survives normalization.
fn strip_bullet_marker(rest: &str) -> Option<&str> {
    rest.strip_prefix("- ")
        .or_else(|| rest.strip_prefix("* "))
        .or_else(|| rest.strip_prefix("+ "))
}

/// Strips the grammar's disallowed inline markup down to its plain text,
/// never dropping the underlying content: bold/italic/code delimiters are
/// removed (the enclosed text survives); a markdown link/image keeps its
/// visible text and drops the `(url)` target; a code-fence delimiter line
/// (```` ``` ````, optionally followed by a language tag) collapses to just
/// that trailing text. Hostile input (a literal `<script>` tag, an
/// arbitrarily long line) is not markup this grammar recognizes, so it is
/// left exactly as-is — inert plain text, both here and at the frontend
/// text-node renderer this grammar is designed for (never interpreted as
/// HTML by either layer).
///
/// Runs in three ordered passes — code spans, then links/images, then
/// emphasis — each of which only removes a delimiter pair it can actually
/// PAIR UP; an unpaired/incidental marker is left exactly as-is rather than
/// deleted on sight. This matters because `*`, `_`, and `` ` `` show up
/// constantly in this app's own domain (meeting notes about software) as
/// plain content, not markup: `snake_case`, `my_function_name`,
/// `src/projection_llm.rs`, `2 * 3`, `a*b`. The prior character-by-character
/// version stripped every occurrence of these bytes unconditionally,
/// silently corrupting exactly that vocabulary in the canonical log.
fn strip_doc_body_markup(text: &str) -> String {
    let text = text.strip_prefix("```").unwrap_or(text);
    let text = strip_code_spans(text);
    let text = strip_links_and_images(&text);
    let text = strip_delimiter_pairs(&text, '*', false);
    strip_delimiter_pairs(&text, '_', true)
}

/// Backtick code spans: a run of N backticks pairs with the NEAREST later
/// run of exactly N backticks; everything between is code-span content and
/// is emitted verbatim (not reprocessed for other markup, matching how a
/// real Markdown parser treats code-span content as literal). A backtick
/// run with no matching same-length partner on the line is not a code span
/// at all — it is left as literal backtick characters rather than deleted.
fn strip_code_spans(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        if chars[i] != '`' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let run_start = i;
        let mut j = i;
        while j < n && chars[j] == '`' {
            j += 1;
        }
        let run_len = j - run_start;

        let mut k = j;
        let mut closing = None;
        while k < n {
            if chars[k] == '`' {
                let close_start = k;
                let mut m = k;
                while m < n && chars[m] == '`' {
                    m += 1;
                }
                if m - close_start == run_len {
                    closing = Some((close_start, m));
                    break;
                }
                k = m;
            } else {
                k += 1;
            }
        }

        match closing {
            Some((close_start, close_end)) => {
                out.extend(&chars[j..close_start]);
                i = close_end;
            }
            None => {
                // No matching run: literal backticks, not markup.
                out.extend(&chars[run_start..j]);
                i = j;
            }
        }
    }
    out
}

/// Markdown links/images: `[text](url)` / `![alt](url)` keep their visible
/// text and drop the `(url)` target. Anything that does not fully resolve to
/// that shape — an unmatched `[`, a `[text]` with no `(url)` target, or a
/// `(target` that never finds its closing `)` — is preserved VERBATIM
/// (including a leading `!`, which the prior implementation dropped even
/// when the following brackets turned out not to be a link at all) rather
/// than silently discarding any of the line.
fn strip_links_and_images(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        let bang = chars[i] == '!' && chars.get(i + 1) == Some(&'[');
        let bracket_start = if bang { i + 1 } else { i };
        if chars.get(bracket_start) != Some(&'[') {
            out.push(chars[i]);
            i += 1;
            continue;
        }

        let mut j = bracket_start + 1;
        let mut closed = false;
        while j < n {
            if chars[j] == ']' {
                closed = true;
                break;
            }
            j += 1;
        }

        if !closed {
            // Unmatched '[' (optionally preceded by '!'): preserve every
            // remaining byte of the line verbatim rather than dropping it.
            out.extend(&chars[i..]);
            break;
        }

        if chars.get(j + 1) != Some(&'(') {
            // "[text]" (or "![text]") with no "(url)" target immediately
            // after — not a link at all. Preserve the brackets (and any
            // leading '!') verbatim.
            out.extend(&chars[i..=j]);
            i = j + 1;
            continue;
        }

        let mut k = j + 2;
        let mut target_closed = false;
        while k < n {
            if chars[k] == ')' {
                target_closed = true;
                break;
            }
            k += 1;
        }

        if !target_closed {
            // Unterminated "(url" — the closing ')' never appears on this
            // line (e.g. a long URL wrapped mid-target). Preserve
            // everything from the opening bracket onward verbatim instead
            // of silently dropping the rest of the line, which is what an
            // unbounded "consume until ')' or EOL" scan used to do.
            out.extend(&chars[i..]);
            break;
        }

        // A real link/image: keep the visible text, drop the "(url)".
        out.extend(&chars[bracket_start + 1..j]);
        i = k + 1;
    }
    out
}

/// Strips a paired run of `delim` (`'*'` or `'_'`) that plausibly delimits
/// emphasis, leaving the enclosed text untouched; an unpaired or
/// non-delimiting run of the same character is left exactly as-is. A run
/// "can open" only if it is not followed by whitespace, and "can close"
/// only if it is not preceded by whitespace — an isolated marker surrounded
/// by spaces on both sides (`2 * 3`) or with nothing to pair against
/// (`a*b`, alone) is therefore never touched.
///
/// `intraword_forbidden` additionally applies Markdown's underscore-specific
/// rule: an underscore run flanked by an alphanumeric character on its inner
/// side can neither open nor close, so `_` never fires inside an identifier
/// like `snake_case`, `my_function_name`, or a `_`-bearing URL/path segment.
/// This restriction does not apply to `*`, matching the existing
/// `a*b`-stays-literal behavior above (which falls out of the plain flanking
/// rule, not an intraword one) and the already-covered `**bold**` case.
fn strip_delimiter_pairs(text: &str, delim: char, intraword_forbidden: bool) -> String {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let is_word = |c: char| c.is_alphanumeric();

    let mut runs: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < n {
        if chars[i] == delim {
            let start = i;
            while i < n && chars[i] == delim {
                i += 1;
            }
            runs.push((start, i));
        } else {
            i += 1;
        }
    }

    let mut remove = vec![false; runs.len()];
    let mut open_idx: Option<usize> = None;
    for (idx, &(start, end)) in runs.iter().enumerate() {
        let prev = if start > 0 {
            Some(chars[start - 1])
        } else {
            None
        };
        let next = chars.get(end).copied();
        let mut can_open = next.is_some_and(|c| !c.is_whitespace());
        let mut can_close = prev.is_some_and(|c| !c.is_whitespace());
        if intraword_forbidden {
            if prev.is_some_and(is_word) {
                can_open = false;
            }
            if next.is_some_and(is_word) {
                can_close = false;
            }
        }

        if let Some(open) = open_idx
            && can_close
        {
            remove[open] = true;
            remove[idx] = true;
            open_idx = None;
            continue;
        }
        if can_open {
            open_idx = Some(idx);
        }
    }

    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (idx, &(start, end)) in runs.iter().enumerate() {
        if remove[idx] {
            out.extend(&chars[last..start]);
            last = end;
        }
    }
    out.extend(&chars[last..]);
    out
}

pub fn trusted_projection_patch_from_model_json(
    raw: &str,
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
    context: ProjectionPatchBuildContext,
) -> Result<ProjectionPatch, ProjectionPatchDraftError> {
    // The SAME basis-covered set the repair prompt (`projection_patch_repair_prompt_messages`)
    // and the original draft prompt (`projection_patch_prompt_messages`) were
    // built from — resolving evidence against anything else would let a
    // `Revised` span (ADR-0031) launder a stale claim into a proof.
    let events = basis_events(job, ledger)?;
    let basis: BTreeMap<&str, &TranscriptEvent> = events
        .iter()
        .map(|event| (event.span_id.as_str(), event))
        .collect();
    let draft = parse_projection_patch_draft(raw, &job.kind, &basis)?;
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
            route_id: context.route_id,
            model_source: context.model_source,
        },
        route: context.route,
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
    /// Character count of the Notes-kind live notes-state snapshot block
    /// (seed audio-graph-253c), content-free like the other fields here. 0
    /// when the caller passed `notes: None` (no block rendered at all — see
    /// [`projection_patch_prompt_messages`]) or for a Graph-kind job.
    pub notes_snapshot_chars: u64,
    /// Number of existing notes rendered into the snapshot block (capped at
    /// [`NOTES_SNAPSHOT_MAX_ENTRIES`]; the true total may be larger — see the
    /// snapshot's own trailing count line). 0 under the same conditions as
    /// `notes_snapshot_chars`.
    pub notes_snapshot_entries: u32,
}

pub fn projection_prompt_shape(
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
) -> ProjectionPromptShape {
    projection_prompt_shape_with_notes(job, ledger, None)
}

/// Widened form of [`projection_prompt_shape`] that also accounts for the
/// Notes-kind live notes-state snapshot (seed audio-graph-253c). Kept as a
/// separate function, additive to the existing `projection_prompt_shape`
/// (which has exactly one caller left at HEAD — a test pinning the
/// notes-blind shape below — and zero production callers), rather than
/// changing that function's signature in place. As of seed audio-graph-253c
/// part 2, `speech/mod.rs`'s `projection_movement_facts` — the data-movement
/// ledger's one production caller of this shape — calls THIS function with
/// the real live-dispatch snapshot, so
/// `notes_snapshot_chars`/`notes_snapshot_entries` reach the ledger.
pub fn projection_prompt_shape_with_notes(
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
    notes: Option<&MaterializedNotes>,
) -> ProjectionPromptShape {
    let Ok(events) = basis_events(job, ledger) else {
        return ProjectionPromptShape::default();
    };
    let (older, _hot) = split_summary_window(&events);
    let pinned = pinned_typed_facts(&events);
    let pinned_chars: usize = pinned.iter().map(|line| line.chars().count()).sum();
    let (notes_snapshot_chars, notes_snapshot_entries) = match notes {
        Some(notes) if job.kind == ProjectionKind::Notes && !notes.notes.is_empty() => (
            render_notes_snapshot(notes).chars().count() as u64,
            notes.notes.len().min(NOTES_SNAPSHOT_MAX_ENTRIES) as u32,
        ),
        _ => (0, 0),
    };
    ProjectionPromptShape {
        has_rolling_summary: !older.is_empty(),
        pinned_fact_chars: pinned_chars as u64,
        notes_snapshot_chars,
        notes_snapshot_entries,
    }
}

/// `notes` is the current Notes projection materialization (seed
/// audio-graph-253c), when the caller actually has it. `Some(&materialized)`
/// renders the live "Current notes state" block (via
/// [`render_notes_snapshot`]) for `ProjectionKind::Notes` jobs — an empty
/// `MaterializedNotes` there is a truthful "no notes yet" (the caller
/// affirmatively knows the session has none). `None` means the caller cannot
/// currently supply the real notes state — the live production call site
/// (`llm/executor.rs`'s `run_projection_patch_dispatch`) passes `None` only
/// for a Graph-kind job or a Notes-kind snapshot that failed the session-
/// identity check (see this module's top-of-file doc and
/// `ProjectionRuntimeHandle::materialized_notes_snapshot_for_session`) — and
/// OMITS the block entirely rather than rendering a fabricated "(no notes
/// yet)" that would be indistinguishable from a real empty session and would
/// license the model to mint ids as if none existed. Graph kind never renders
/// this block regardless of `notes` (seed e700's lane).
pub fn projection_patch_prompt_messages(
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
    notes: Option<&MaterializedNotes>,
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
        // NOTE: this text lives in the byte-stable prefix (`[0]` above), so it
        // must stay the SAME string regardless of this tick's runtime
        // `notes.is_some()` — branching on that here would leak a per-tick
        // variable into the theoretically-stable system message and bust the
        // cache prefix (ADR-0025 §2d). It is phrased to stay honest either
        // way instead: a conditional "if a block appears" claim, not an
        // unconditional "this prompt includes" claim, since the "Current
        // notes state" block is only rendered below when `notes` is `Some`
        // (see the doc comment above and this function's `notes == None`
        // comment further down).
        ProjectionKind::Notes => {
            "Use only upsert_note, delete_note, and reorder_note operations. If a \"Current \
             notes state\" block appears below, it lists every existing note's id, title, and \
             a one-line body summary — check it before minting an id: if a note there already \
             covers this topic, upsert_note with THAT SAME id to refine it in place; only use \
             an id absent from that block when the note is genuinely new. If no such block \
             appears, you have no visibility into already-existing note ids this tick, so only \
             mint a new id when you are confident this is a genuinely new topic. Keep stable \
             note ids when refining earlier notes."
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
    //   [.] Notes-kind AND `notes.is_some()` ONLY: live notes-state snapshot
    //       (audio-graph-253c) — NOT append-only (an existing note can be
    //       rewritten in place), so it lives here in the variable region,
    //       never in the [0]/[1] prefix. Omitted (not rendered as an empty
    //       block) when the caller passed `None`, i.e. does not actually
    //       have the live notes state — see this function's doc comment.
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

    let mut messages = vec![
        ChatMessage {
            role: "system".to_string(),
            content: format!(
                "You generate AudioGraph projection patch drafts. Return strict JSON only, with no markdown. \
                 Do not include trusted metadata such as sequence, basis, provenance, session_id, or llm_request_id; \
                 the backend stamps those fields. {operation_guidance} {EVIDENCE_GUIDANCE}\n\n\
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
    ];

    // `notes == None`: the caller does not have the live notes state (see
    // the doc comment above) — the message is omitted rather than rendered
    // with a fabricated "(no notes yet)" body, since that would be
    // indistinguishable from a real empty session and would license the
    // model to mint ids as if none existed.
    if job.kind == ProjectionKind::Notes
        && let Some(notes) = notes
    {
        messages.push(ChatMessage {
            role: "user".to_string(),
            content: format!(
                "Current notes state (existing note ids — reuse one of these exactly when \
                 refining or extending that SAME note; only mint a new id for a genuinely new \
                 note):\n{notes_block}",
                notes_block = render_notes_snapshot(notes),
            ),
        });
    }

    messages.push(ChatMessage {
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
    });

    Ok(messages)
}

/// Pinned must-never-lose facts, rendered as a deterministic structured block.
///
/// ADR-0025 §2c.3: the research shows a prose summarizer inverts negations and
/// drops rejection reasons, so the identity-bearing facts (which speaker said
/// something, in what span) are pinned as structured lines rather than trusted
/// to the summary. Derived deterministically from the basis events so the block
/// is byte-stable across turns (append-only), keeping the cache prefix intact.
/// This is transcript-derived (not a live notes/graph snapshot) to keep this
/// helper's `&[TranscriptEvent]`-only seam intact. The Notes-kind live notes
/// snapshot (seed audio-graph-253c) is a SEPARATE block, rendered by
/// [`render_notes_snapshot`] from the `notes: Option<&MaterializedNotes>`
/// parameter [`projection_patch_prompt_messages`] takes. As of seed
/// audio-graph-253c part 2 the live production dispatch path passes
/// `Some(&snapshot)` for every Notes-kind tick that can confirm its own
/// session identity (see this module's top-of-file doc), closing the seam
/// this doc comment used to describe as open in practice. A live graph
/// snapshot for the Graph kind remains a later pillar (seed e700 owns that
/// lane's problems).
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

/// Render the live Notes-kind notes state into a bounded, deterministic block
/// (seed audio-graph-253c): every upsert the model emits full-replaces a note
/// by id (`MaterializedNotes::upsert_note`), but the prompt never showed which
/// ids already existed, so a model that could not see id `note-1` re-minted it
/// blind on every tick — measured field impact: 83 patches / 293 upserts over
/// only 23 final ids in one 16m41s session, 94% of upserts landing on six id
/// slots, note-1 rewritten 77 times. This block is what makes "keep stable
/// note ids" (the existing prompt instruction) actually followable.
///
/// Lives in the prompt's per-job VARIABLE region (after the cache-stable
/// prefix, `PROJECTION_STABLE_PREFIX_MESSAGE_COUNT`), because unlike
/// `pinned_typed_facts` this is NOT append-only — an existing note's title/body
/// can be rewritten in place, so folding it into the byte-stable prefix would
/// bust the prompt cache on every note edit.
///
/// Selection is most-recently-updated first (`updated_by_sequence` descending,
/// ties broken by id for determinism), capped at
/// [`NOTES_SNAPSHOT_MAX_ENTRIES`]; a session with more notes than the cap
/// still gets every note ID **shown or counted**, never silently dropped: the
/// cut is one deterministic total-order truncation, and the trailing count
/// line reports the true total so the model is never told a capped session is
/// complete.
///
/// NOTE on ordering vs. `reorder_note`: this list is recency-ordered, which is
/// NOT necessarily the notes' actual display order (`MaterializedNotes.notes`
/// is itself an ordered `Vec` that `reorder_note` maintains by index). A
/// `reorder_note { id, after_id }` operation the model emits from reading this
/// block targets that real Vec order, not the recency order shown here.
fn render_notes_snapshot(notes: &MaterializedNotes) -> String {
    if notes.notes.is_empty() {
        return "(no notes yet)".to_string();
    }

    let total = notes.notes.len();
    let mut ordered: Vec<&MaterializedNote> = notes.notes.iter().collect();
    ordered.sort_by(|a, b| {
        b.updated_by_sequence
            .cmp(&a.updated_by_sequence)
            .then_with(|| a.id.cmp(&b.id))
    });
    let shown = ordered.len().min(NOTES_SNAPSHOT_MAX_ENTRIES);

    let mut lines: Vec<String> = ordered[..shown]
        .iter()
        .map(|note| {
            format!(
                "- id: {} | title: {} | body: {}",
                one_line_bounded(&note.id, NOTES_SNAPSHOT_ID_TITLE_MAX_CHARS),
                one_line_bounded(&note.title, NOTES_SNAPSHOT_ID_TITLE_MAX_CHARS),
                one_line_bounded(&note.body, NOTES_SNAPSHOT_BODY_SUMMARY_MAX_CHARS)
            )
        })
        .collect();

    if total > shown {
        lines.push(format!(
            "(+{} more existing note(s) not shown; {total} total)",
            total - shown
        ));
    }

    lines.join("\n")
}

/// One bounded, single-line rendering of a model-authored field (note id,
/// title, or body) for [`render_notes_snapshot`]. Collapses embedded
/// newlines/whitespace and truncates to `max_chars`.
///
/// `id` and `title` are `String`s with no length or newline validation
/// anywhere on the apply path (`upsert_note` accepts them verbatim, and there
/// is no `maxLength`/schema constraint on either field) — without running them
/// through this same collapse-and-truncate step as the body, a single
/// oversized or newline-containing id/title would make one snapshot line (and
/// therefore the whole block) unbounded, and an embedded newline would break
/// the one-line-per-note invariant the snapshot's line-count bound relies on.
fn one_line_bounded(text: &str, max_chars: usize) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated: String = collapsed.chars().take(max_chars).collect();
    if collapsed.chars().count() > max_chars {
        format!("{truncated}…")
    } else {
        truncated
    }
}

pub fn projection_patch_repair_prompt_messages(
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
    notes: Option<&MaterializedNotes>,
    invalid_model_output: &str,
    error: &ProjectionPatchDraftError,
) -> Result<Vec<ChatMessage>, ProjectionPatchDraftError> {
    let mut messages = projection_patch_prompt_messages(job, ledger, notes)?;
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
    basis: &BTreeMap<&str, &TranscriptEvent>,
) -> Result<(), ProjectionPatchDraftError> {
    if let Some(confidence) = draft.confidence
        && (!confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
    {
        return Err(ProjectionPatchDraftError::InvalidConfidence { confidence });
    }

    let mut seen_ids = BTreeSet::new();
    for operation in &draft.operations {
        validate_operation(operation, expected_kind, basis)?;
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
    basis: &BTreeMap<&str, &TranscriptEvent>,
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
            id,
            title,
            body,
            evidence,
            ..
        } => {
            require_non_empty(operation, "id", id)?;
            require_non_empty(operation, "title", title)?;
            require_non_empty(operation, "body", body)?;
            judge_operation_evidence(operation, id, evidence, basis)
        }
        ProjectionOperation::DeleteNote { id } => require_non_empty(operation, "id", id),
        // ADR-0037 part 4 (see `ProjectionOperation::InvalidateNote`'s doc):
        // never admissible from a model-submitted draft, regardless of `id`.
        ProjectionOperation::InvalidateNote { .. } => {
            Err(ProjectionPatchDraftError::DerivedOnlyOperation {
                operation: operation_name(operation),
            })
        }
        ProjectionOperation::ReorderNote { id, .. } => require_non_empty(operation, "id", id),
        ProjectionOperation::UpsertGraphNode {
            id,
            name,
            entity_type,
            evidence,
            ..
        } => {
            require_non_empty(operation, "id", id)?;
            require_non_empty(operation, "name", name)?;
            require_non_empty(operation, "entity_type", entity_type)?;
            judge_operation_evidence(operation, id, evidence, basis)
        }
        ProjectionOperation::RemoveGraphNode { id } => require_non_empty(operation, "id", id),
        ProjectionOperation::InvalidateGraphNode { id } => require_non_empty(operation, "id", id),
        ProjectionOperation::UpsertGraphEdge {
            id,
            source,
            target,
            relation_type,
            weight,
            evidence,
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
            judge_operation_evidence(operation, id, evidence, basis)
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

/// The claim-evidence admission call (ADR-0037) for the three content-
/// creating Upsert* operations: resolves `evidence` against `basis` — the
/// SAME basis-covered `TranscriptEvent` set [`basis_events`] built for this
/// job — and turns a [`ClaimAdmission::Refused`] into the SAME
/// all-or-nothing per-patch validation failure every other structural check
/// in [`validate_operation`] already produces (one bad operation already
/// fails the whole patch today; this is additive to that, not a new
/// asymmetry).
fn judge_operation_evidence(
    operation: &ProjectionOperation,
    id: &str,
    evidence: &crate::claim_evidence::EvidenceAnchor,
    basis: &BTreeMap<&str, &TranscriptEvent>,
) -> Result<(), ProjectionPatchDraftError> {
    match crate::claim_evidence::judge_claim_evidence(evidence, basis) {
        crate::claim_evidence::ClaimAdmission::Admitted(_) => Ok(()),
        crate::claim_evidence::ClaimAdmission::Refused(deficiency) => {
            Err(ProjectionPatchDraftError::ClaimEvidenceRefused {
                operation: operation_name(operation),
                id: id.to_string(),
                deficiency,
            })
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
        | ProjectionOperation::InvalidateNote { .. }
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
        ProjectionOperation::InvalidateNote { .. } => "invalidate_note",
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
        | ProjectionOperation::InvalidateNote { id }
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

    fn empty_notes() -> MaterializedNotes {
        MaterializedNotes::new("session-1")
    }

    fn context() -> ProjectionPatchBuildContext {
        ProjectionPatchBuildContext {
            sequence: 7,
            llm_request_id: "llm-request-7".to_string(),
            provider: "llm.api".to_string(),
            model: "test-model".to_string(),
            model_source: crate::llm::route::ModelIdentitySource::Served,
            route_id: Some("route.openai_compatible".to_string()),
            route: None,
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
                "tags": ["topic"],
                "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
            }],
            "confidence": 0.82
        })
        .to_string();

        let patch = trusted_projection_patch_from_model_json(&raw, &job, &ledger, context())
            .expect("valid patch");

        assert_eq!(patch.sequence, 7);
        assert_eq!(patch.kind, ProjectionKind::Notes);
        assert_eq!(patch.basis, job.basis);
        assert_eq!(patch.confidence, 0.82);
        // Post-ADR-0038 provenance names the REGISTRY provider id, and records the
        // stamped route id plus whether the model id is the served one.
        assert_eq!(patch.provenance.provider, "llm.api");
        assert_eq!(patch.provenance.model, "test-model");
        assert_eq!(
            patch.provenance.route_id.as_deref(),
            Some("route.openai_compatible")
        );
        assert_eq!(
            patch.provenance.model_source,
            crate::llm::route::ModelIdentitySource::Served
        );
        assert_eq!(patch.provenance.prompt_id, PROJECTION_PATCH_PROMPT_ID);
        assert_eq!(patch.llm_request_id, "llm-request-7");
        assert_eq!(patch.created_at_ms, 20);
    }

    /// Measure the serialized size of the strict output schema per projection kind.
    ///
    /// This exists to put a NUMBER in the repository rather than reason about one.
    /// Cerebras's documented strict-schema ceiling is cited in off-branch research
    /// that does not exist in this worktree, so ADR-0038's contract deliberately
    /// does NOT hardcode it: the 4xx → `json_object` downgrade on the same route
    /// makes behaviour independent of the exact limit. What is worth recording is
    /// how large the schema we actually send is, so a future reader can compare it
    /// against whatever ceiling they can cite. The bound below is a loose
    /// regression guard, not a provider claim.
    #[test]
    fn projection_patch_strict_schema_serialized_length_is_recorded() {
        for kind in [ProjectionKind::Notes, ProjectionKind::Graph] {
            let schema = projection_patch_strict_json_schema(&kind);
            let len = serde_json::to_string(&schema)
                .expect("serialize schema")
                .len();
            assert!(
                (200..8_000).contains(&len),
                "{kind:?} strict schema serializes to {len} bytes, outside the recorded \
                 200..8000 envelope — re-measure before widening this bound"
            );
        }
    }

    /// The trusted-stamp boundary (ADR-0024 §3): model JSON supplies only
    /// `operations` + `confidence`. A model that tries to dictate its own route id,
    /// provider, or served-model identity is ignored — those come from
    /// [`ProjectionPatchBuildContext`], which only trusted code constructs.
    #[test]
    fn model_json_cannot_dictate_route_provenance() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice met Bob."))
            .unwrap();
        let job = job(ProjectionKind::Notes, &ledger);
        let raw = serde_json::json!({
            "operations": [{
                "type": "upsert_note",
                "id": "note:alice-bob",
                "title": "Alice and Bob",
                "body": "Alice met Bob.",
                "tags": [],
                "evidence": {"claim_class": "unavailable_evidence", "note": "fixture"}
            }],
            "confidence": 0.5,
            "provenance": {
                "provider": "llm.attacker",
                "model": "attacker-model",
                "prompt_id": "attacker",
                "route_id": "route.attacker",
                "model_source": "served"
            },
            "route": { "route_id": "route.attacker" }
        })
        .to_string();

        // `ProjectionPatchDraft` is `deny_unknown_fields`, so the draft is rejected
        // outright rather than partially honored — the new `route_id` / `route`
        // fields inherit that boundary instead of widening it.
        let error = trusted_projection_patch_from_model_json(&raw, &job, &ledger, context())
            .expect_err("a draft carrying trusted route metadata must be rejected");
        assert!(
            matches!(
                &error,
                ProjectionPatchDraftError::InvalidJson { error } if error.contains("unknown field")
            ),
            "got: {error:?}"
        );
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

        let err = trusted_projection_patch_from_model_json(&raw, &job, &ledger, context())
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

        let draft = parse_projection_patch_draft(&raw, &ProjectionKind::Notes, &BTreeMap::new())
            .expect("reorder note");

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
                    "description": null,
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                },
                {
                    "type": "upsert_graph_node",
                    "id": "person:bob",
                    "name": "Bob",
                    "entity_type": "person",
                    "description": null,
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                },
                {
                    "type": "upsert_graph_edge",
                    "id": "edge:alice:bob:works_with",
                    "source": "person:alice",
                    "target": "person:bob",
                    "relation_type": "works_with",
                    "label": "works with",
                    "weight": 0.7,
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                }
            ],
            "confidence": 0.76
        })
        .to_string();

        let patch = trusted_projection_patch_from_model_json(&raw, &job, &ledger, context())
            .expect("graph patch");

        assert_eq!(patch.kind, ProjectionKind::Graph);
        assert_eq!(patch.operations.len(), 3);
        assert_eq!(patch.confidence, 0.76);
    }

    /// seed audio-graph-e700 sub-fix 1 (INGEST FALLBACK): a non-strict route
    /// (or a strict-mode provider that ignored the wire enum) can still hand
    /// `trusted_projection_patch_from_model_json` an invented `entity_type`.
    /// The deterministic fallback (`ontology::normalize_entity_type`) must
    /// rewrite it to a canonical ontology name in the TRUSTED patch that
    /// actually gets persisted — never reject the whole patch over it, and
    /// never leave the invented string riding through.
    #[test]
    fn ingest_normalizes_invented_entity_type_to_closed_ontology() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "The team evaluated Postgres."))
            .unwrap();
        let job = job(ProjectionKind::Graph, &ledger);
        let raw = serde_json::json!({
            "operations": [{
                "type": "upsert_graph_node",
                "id": "node1",
                "name": "Postgres",
                "entity_type": "Provider",
                "description": null,
                "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
            }]
        })
        .to_string();

        let patch = trusted_projection_patch_from_model_json(&raw, &job, &ledger, context())
            .expect("invented entity_type must not be rejected outright");

        assert!(matches!(
            patch.operations.first(),
            Some(ProjectionOperation::UpsertGraphNode { entity_type, .. })
                if entity_type == "Product"
        ));
    }

    /// Companion negative case: an entity_type that already matches the
    /// closed ontology (any case) must pass through unchanged in shape (only
    /// case-canonicalized), proving the fallback does not perturb already-
    /// valid model output.
    #[test]
    fn ingest_preserves_already_canonical_entity_type() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice is a person."))
            .unwrap();
        let job = job(ProjectionKind::Graph, &ledger);
        let raw = serde_json::json!({
            "operations": [{
                "type": "upsert_graph_node",
                "id": "person:alice",
                "name": "Alice",
                "entity_type": "person",
                "description": null,
                "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
            }]
        })
        .to_string();

        let patch = trusted_projection_patch_from_model_json(&raw, &job, &ledger, context())
            .expect("valid patch");

        assert!(matches!(
            patch.operations.first(),
            Some(ProjectionOperation::UpsertGraphNode { entity_type, .. })
                if entity_type == "Person"
        ));
    }

    /// seed audio-graph-e700 (RELATION TYPES): `relation_type` is NOT bound
    /// to a closed enum (ontology.rs's `RELATION_TYPES` is explicitly open),
    /// but surface-form variance (case/punctuation/whitespace) must still
    /// collapse at ingest so `"Works At"` and `"works_at"` land on the SAME
    /// persisted string.
    #[test]
    fn ingest_soft_normalizes_relation_type_surface_form() {
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
                    "description": null,
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                },
                {
                    "type": "upsert_graph_node",
                    "id": "person:bob",
                    "name": "Bob",
                    "entity_type": "person",
                    "description": null,
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                },
                {
                    "type": "upsert_graph_edge",
                    "id": "edge:alice:bob",
                    "source": "person:alice",
                    "target": "person:bob",
                    "relation_type": "Works With!!",
                    "label": null,
                    "weight": 0.5,
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                }
            ]
        })
        .to_string();

        let patch = trusted_projection_patch_from_model_json(&raw, &job, &ledger, context())
            .expect("valid patch");

        assert!(matches!(
            patch.operations.last(),
            Some(ProjectionOperation::UpsertGraphEdge { relation_type, .. })
                if relation_type == "works_with"
        ));
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

        let draft = parse_projection_patch_draft(&raw, &ProjectionKind::Graph, &BTreeMap::new())
            .expect("retcon draft");

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
            &BTreeMap::new(),
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

        let err = parse_projection_patch_draft(&raw, &ProjectionKind::Notes, &BTreeMap::new())
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

        let err = parse_projection_patch_draft(&raw, &ProjectionKind::Graph, &BTreeMap::new())
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
        let err =
            parse_projection_patch_draft(&invalid_delta, &ProjectionKind::Graph, &BTreeMap::new())
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
        let err = parse_projection_patch_draft(
            &underspecified_split,
            &ProjectionKind::Graph,
            &BTreeMap::new(),
        )
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
        let fixture_event = event("span-1", 1, "Alice met Bob.");
        let basis: BTreeMap<&str, &TranscriptEvent> =
            [("span-1", &fixture_event)].into_iter().collect();
        let raw = serde_json::json!({
            "operations": [
                {
                    "type": "upsert_graph_node",
                    "id": "person:alice",
                    "name": "Alice",
                    "entity_type": "person",
                    "description": null,
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                },
                {
                    "type": "upsert_graph_node",
                    "id": "person:alice",
                    "name": "Alice A.",
                    "entity_type": "person",
                    "description": "duplicate in same patch",
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                }
            ]
        })
        .to_string();

        let err = parse_projection_patch_draft(&raw, &ProjectionKind::Graph, &basis)
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
                "tags": ["provider"],
                "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
            }]
        })
        .to_string();
        let first_patch =
            trusted_projection_patch_from_model_json(&first_raw, &first_job, &ledger, context())
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
                "tags": ["provider", "correction"],
                "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
            }],
            "confidence": 0.78
        })
        .to_string();
        let second_patch = trusted_projection_patch_from_model_json(
            &second_raw,
            &second_job,
            &ledger,
            second_context,
        )
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
                "description": "Candidate provider.",
                "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
            }]
        })
        .to_string();
        let first_patch =
            trusted_projection_patch_from_model_json(&first_raw, &first_job, &ledger, context())
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
                "description": "Realtime STT candidate with speaker labels.",
                "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
            }],
            "confidence": 0.83
        })
        .to_string();
        let second_patch = trusted_projection_patch_from_model_json(
            &second_raw,
            &second_job,
            &ledger,
            second_context,
        )
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

        let err = projection_patch_prompt_messages(&job, &ledger, Some(&empty_notes()))
            .expect_err("stale basis");

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

        let messages = projection_patch_repair_prompt_messages(
            &job,
            &ledger,
            Some(&empty_notes()),
            &invalid,
            &error,
        )
        .expect("repair prompt");

        // Static→dynamic base prompt is 5 messages for a Notes-kind job
        // (system, stable-context, hot-buffer transcript, notes-state
        // snapshot, per-tick metadata); the repair pass appends the invalid
        // assistant turn + the correction user turn.
        assert_eq!(messages.len(), 7);
        assert_eq!(messages[5].role, "assistant");
        assert!(messages[5].content.contains("upsert_graph_node"));
        assert_eq!(messages[6].role, "user");
        assert!(messages[6].content.contains("expected_kind: notes"));
        assert!(messages[6].content.contains("validation_error:"));
        assert!(messages[6].content.contains("upsert_graph_node"));
        assert!(messages[6].content.contains("Output JSON schema"));
        assert!(
            messages[6]
                .content
                .contains("Do not include trusted metadata")
        );
    }

    /// ADR-0037: the system prompt must actually teach a model what
    /// `evidence` is, or a schema-obeying model on any non-strict route has
    /// no way to learn the contract (finding: prompt text never mentioned
    /// evidence, claim classes, or that `span_id` must come from the
    /// transcript window). Repair prompts extend this same base prompt, so
    /// they inherit the guidance without a separate copy.
    #[test]
    fn system_prompt_teaches_the_evidence_contract() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice chose Soniox."))
            .unwrap();
        let job = job(ProjectionKind::Notes, &ledger);
        let messages =
            projection_patch_prompt_messages(&job, &ledger, Some(&empty_notes())).expect("prompt");

        let system = &messages[0].content;
        for term in [
            "evidence",
            "claim_class",
            "verified_quote",
            "grounded_inference",
            "unavailable_evidence",
            "knowledge_gap",
            "span_id",
            "quote",
        ] {
            assert!(
                system.contains(term),
                "system prompt must teach `{term}`, got: {system}"
            );
        }
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
        let messages = projection_patch_prompt_messages(&job, &ledger, Some(&empty_notes()))
            .expect("windowed prompt");

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
        let first = projection_patch_prompt_messages(&first_job, &ledger, Some(&empty_notes()))
            .expect("first prompt");

        // Append a brand-new turn. Because the new turn enters the hot buffer
        // and pushes the oldest one into the (append-only) summary, the stable
        // prefix (system block) must stay byte-identical.
        ledger
            .apply_event(event("span-new", 1, "A fresh turn arrives"))
            .unwrap();
        let second_job = job(ProjectionKind::Notes, &ledger);
        let second = projection_patch_prompt_messages(&second_job, &ledger, Some(&empty_notes()))
            .expect("second prompt");

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

    /// Seam-safety proof (audio-graph-253c): a note being REWRITTEN (not
    /// appended) between two calls against the SAME ledger state must still
    /// leave the cache-stable prefix (messages 0 and 1) byte-identical. This
    /// is the load-bearing reason the notes snapshot was placed in the
    /// per-job variable region (message 3) instead of the pinned-facts block
    /// (message 1): a snapshot that changes shape when a note's body is
    /// edited in place would bust the cache if it lived in the append-only
    /// prefix.
    #[test]
    fn notes_snapshot_placement_never_busts_the_cache_stable_prefix() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice chose Soniox."))
            .unwrap();
        let job = job(ProjectionKind::Notes, &ledger);

        let mut notes_v1 = empty_notes();
        notes_v1
            .apply_patch(
                &notes_patch(1, "note:1", "Choice", "Alice chose Soniox."),
                None,
            )
            .expect("apply first note version");
        let before = projection_patch_prompt_messages(&job, &ledger, Some(&notes_v1))
            .expect("prompt with first note version");

        // Rewrite the SAME note id in place (the exact overwrite shape the
        // field measurement showed happening 77 times to `note-1`) — the
        // ledger/job/basis are untouched, only the notes materialization
        // changed shape.
        let mut notes_v2 = notes_v1.clone();
        notes_v2
            .apply_patch(
                &notes_patch(
                    2,
                    "note:1",
                    "Choice",
                    "Alice chose Soniox for the realtime pilot, not production.",
                ),
                None,
            )
            .expect("apply rewritten note version");
        let after = projection_patch_prompt_messages(&job, &ledger, Some(&notes_v2))
            .expect("prompt with rewritten note version");

        assert_eq!(
            before[0].content, after[0].content,
            "system block (message 0) must stay byte-identical when only notes content changes"
        );
        assert_eq!(
            before[1].content, after[1].content,
            "pinned-facts/summary block (message 1) must stay byte-identical when only notes content changes"
        );
        // The notes-snapshot message (message 3, Notes-kind only) DOES change —
        // proving the rewrite is actually visible where it is supposed to be.
        assert_ne!(
            before[3].content, after[3].content,
            "notes-snapshot message must reflect the rewritten note body"
        );
        assert!(after[3].content.contains("not production"));
    }

    /// Helper: a one-operation Notes patch, used to build a
    /// `MaterializedNotes` fixture by direct `apply_patch` calls (no ledger,
    /// no LLM route — pure, synchronous, no cross-thread state).
    fn notes_patch(sequence: u64, id: &str, title: &str, body: &str) -> ProjectionPatch {
        ProjectionPatch {
            route: None,
            sequence,
            kind: ProjectionKind::Notes,
            llm_request_id: format!("test-request-{sequence}"),
            basis: ProjectionBasis {
                span_revisions: vec![ProjectionBasisSpan {
                    span_id: "span-1".to_string(),
                    revision_number: 1,
                }],
                diarization_span_revisions: Vec::new(),
                transcript_hash: format!("fnv1a64:{sequence:016x}"),
                summarized_through_revision: None,
            },
            operations: vec![ProjectionOperation::UpsertNote {
                id: id.to_string(),
                title: title.to_string(),
                body: body.to_string(),
                tags: Vec::new(),
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
                heading_level: None,
            }],
            confidence: 1.0,
            provenance: ProjectionProvenance {
                provider: "test".to_string(),
                model: "test-model".to_string(),
                prompt_id: PROJECTION_PATCH_PROMPT_ID.to_string(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: sequence,
        }
    }

    // ----- audio-graph-253c: live notes-state snapshot ----------------------

    /// The snapshot block is present (with every note's id) for Notes-kind
    /// jobs and ABSENT (not merely empty) for Graph-kind jobs — Graph-kind
    /// prompt shape is seed e700's lane, unchanged here.
    #[test]
    fn notes_snapshot_present_for_notes_kind_and_absent_for_graph_kind() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice chose Soniox."))
            .unwrap();
        let mut notes = empty_notes();
        notes
            .apply_patch(
                &notes_patch(
                    1,
                    "note:decision",
                    "Provider decision",
                    "Alice chose Soniox.",
                ),
                None,
            )
            .expect("apply note");

        let notes_job = job(ProjectionKind::Notes, &ledger);
        let notes_messages = projection_patch_prompt_messages(&notes_job, &ledger, Some(&notes))
            .expect("notes prompt");
        assert_eq!(
            notes_messages.len(),
            5,
            "Notes-kind prompt carries the extra notes-snapshot message"
        );
        assert!(
            notes_messages
                .iter()
                .any(|m| m.content.contains("note:decision")),
            "Notes-kind prompt must show the existing note's id somewhere"
        );
        assert!(notes_messages[3].content.contains("Current notes state"));
        assert!(notes_messages[3].content.contains("note:decision"));
        assert!(notes_messages[3].content.contains("Provider decision"));

        let graph_job = job(ProjectionKind::Graph, &ledger);
        let graph_messages = projection_patch_prompt_messages(&graph_job, &ledger, Some(&notes))
            .expect("graph prompt");
        assert_eq!(
            graph_messages.len(),
            4,
            "Graph-kind prompt has no notes-snapshot message at all"
        );
        assert!(
            graph_messages
                .iter()
                .all(|m| !m.content.contains("note:decision")),
            "Graph-kind prompt must never leak notes content (seed e700 owns that lane)"
        );
        assert!(
            graph_messages
                .iter()
                .all(|m| !m.content.contains("Current notes state")),
            "the snapshot block itself must be absent, not merely empty, for Graph kind"
        );
    }

    /// `notes: None` (the caller does not actually have the live notes state
    /// for this tick — e.g. a Graph-kind job, or a Notes-kind snapshot that
    /// failed the session-identity check in `llm/executor.rs`'s live
    /// dispatch path) must OMIT the notes-state message
    /// entirely for a Notes-kind job, never render a fabricated "(no notes
    /// yet)" body. A fabricated empty block is indistinguishable from a real
    /// empty session and would license the model to mint ids as if none
    /// existed — the exact overwrite-storm mechanism this seed exists to fix,
    /// just re-triggered via a false premise instead of silence.
    #[test]
    fn notes_snapshot_omitted_not_fabricated_when_caller_has_no_notes_state() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice chose Soniox."))
            .unwrap();

        let notes_job = job(ProjectionKind::Notes, &ledger);
        let messages =
            projection_patch_prompt_messages(&notes_job, &ledger, None).expect("notes prompt");

        assert_eq!(
            messages.len(),
            4,
            "with notes=None the notes-snapshot message must be absent, same shape as Graph-kind"
        );
        // The byte-stable system message (message 0) legitimately references
        // the block by name as part of the id-stability instruction text —
        // that guidance text is unconditional (see the doc comment on
        // `projection_patch_prompt_messages`). What must never happen is a
        // per-tick message claiming notes state, true or false.
        assert!(
            messages[1..]
                .iter()
                .all(|m| !m.content.contains("Current notes state")),
            "notes=None must render no per-tick notes-state message, got: {messages:?}"
        );
        assert!(
            messages.iter().all(|m| !m.content.contains("no notes yet")),
            "notes=None must never fabricate a \"(no notes yet)\" claim, got: {messages:?}"
        );
    }

    /// ADR-0025 §2g / seed audio-graph-72d5: the data-movement ledger must be
    /// able to see the notes-snapshot block's content-free shape once a
    /// caller has real notes state, and must report zero when it does not —
    /// so the ledger and the actual prompt never disagree about whether note
    /// content left the device on a given tick.
    #[test]
    fn prompt_shape_with_notes_reports_notes_snapshot_shape_content_free() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice chose Soniox."))
            .unwrap();
        let mut notes = empty_notes();
        notes
            .apply_patch(
                &notes_patch(
                    1,
                    "note:decision",
                    "Provider decision",
                    "Alice chose Soniox.",
                ),
                None,
            )
            .expect("apply note");

        let notes_job = job(ProjectionKind::Notes, &ledger);
        let with_notes = projection_prompt_shape_with_notes(&notes_job, &ledger, Some(&notes));
        assert_eq!(with_notes.notes_snapshot_entries, 1);
        assert!(with_notes.notes_snapshot_chars > 0);

        // `None` (unwired caller) and the plain, non-widened
        // `projection_prompt_shape` must both report zero — no phantom
        // off-device notes content recorded when none was actually sent.
        let unwired = projection_prompt_shape_with_notes(&notes_job, &ledger, None);
        assert_eq!(unwired.notes_snapshot_entries, 0);
        assert_eq!(unwired.notes_snapshot_chars, 0);
        let plain = projection_prompt_shape(&notes_job, &ledger);
        assert_eq!(plain.notes_snapshot_entries, 0);
        assert_eq!(plain.notes_snapshot_chars, 0);

        // Graph-kind never reports notes-snapshot shape, even with notes
        // handy (seed e700's lane; Graph prompts never render this block).
        let graph_job = job(ProjectionKind::Graph, &ledger);
        let graph_shape = projection_prompt_shape_with_notes(&graph_job, &ledger, Some(&notes));
        assert_eq!(graph_shape.notes_snapshot_entries, 0);
        assert_eq!(graph_shape.notes_snapshot_chars, 0);
    }

    /// A session with more notes than [`NOTES_SNAPSHOT_MAX_ENTRIES`] still
    /// gets a bounded block: every note beyond the cap is folded into one
    /// trailing count line rather than growing the prompt unbounded.
    #[test]
    fn notes_snapshot_is_bounded_at_the_cap_with_a_count_line() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Many topics discussed."))
            .unwrap();
        let mut notes = empty_notes();
        let total = NOTES_SNAPSHOT_MAX_ENTRIES + 5;
        for i in 0..total {
            notes
                .apply_patch(
                    &notes_patch(
                        (i + 1) as u64,
                        &format!("note:{i}"),
                        &format!("Topic {i}"),
                        &format!("Body of note {i}."),
                    ),
                    None,
                )
                .expect("apply note");
        }

        let block = render_notes_snapshot(&notes);
        let shown_lines = block.lines().count();
        // NOTES_SNAPSHOT_MAX_ENTRIES note lines + one trailing count line.
        assert_eq!(shown_lines, NOTES_SNAPSHOT_MAX_ENTRIES + 1);
        assert!(
            block.contains(&format!(
                "+5 more existing note(s) not shown; {total} total"
            )),
            "got: {block}"
        );
        // The most-recently-applied notes (highest sequence) must be the ones
        // actually shown, never silently dropped in favor of older ones.
        assert!(block.contains(&format!("note:{}", total - 1)));
        assert!(
            !block.contains("note:0 |"),
            "the oldest note past the cap must not be individually listed"
        );
    }

    /// Prompt-size bound, demonstrated in bytes rather than asserted in the
    /// abstract. Independent growth axes, each checked against a fixed byte
    /// ceiling derived from the documented caps — this is what makes the
    /// per-tick token cost bounded, not O(session length), the same growth
    /// failure mode ADR-0025 already fixed for the transcript feed.
    #[test]
    fn notes_snapshot_prompt_size_is_bounded_independent_of_session_growth() {
        fn snapshot_len(note_count: usize, body: &str) -> usize {
            let mut notes = empty_notes();
            for i in 0..note_count {
                notes
                    .apply_patch(
                        &notes_patch(
                            (i + 1) as u64,
                            &format!("note:{i}"),
                            &format!("Topic {i}"),
                            body,
                        ),
                        None,
                    )
                    .expect("apply note");
            }
            render_notes_snapshot(&notes).len()
        }

        // Generous fixed ceiling derived from the documented caps: at most
        // NOTES_SNAPSHOT_MAX_ENTRIES lines, each well under 400 bytes once
        // the per-body cap is applied, plus a short trailing count line.
        const CEILING_BYTES: usize = NOTES_SNAPSHOT_MAX_ENTRIES * 400 + 200;

        // Axis 1 — note COUNT: a 200x increase (10 -> 2,000 notes, same short
        // body each) must stay under the ceiling and must not grow anywhere
        // close to proportionally, because the cap stops adding lines past
        // NOTES_SNAPSHOT_MAX_ENTRIES.
        let few_notes = snapshot_len(10, "A short note body.");
        let many_notes = snapshot_len(2_000, "A short note body.");
        assert!(
            many_notes < CEILING_BYTES,
            "2,000-note snapshot is {many_notes} bytes, over the {CEILING_BYTES}-byte ceiling — \
             the cap is not actually bounding note-count growth"
        );
        assert!(
            many_notes < few_notes * 4,
            "snapshot size grew from {few_notes} to {many_notes} bytes across a 200x note-count \
             increase (same body each) — this is not the roughly-flat growth the cap is supposed \
             to give past NOTES_SNAPSHOT_MAX_ENTRIES"
        );

        // Axis 2 — per-note BODY LENGTH: a ~260x increase in one note's body
        // (5,000 chars vs. 19) must stay under the ceiling and must not grow
        // anywhere close to proportionally, because `one_line_bounded`
        // truncates every body to `NOTES_SNAPSHOT_BODY_SUMMARY_MAX_CHARS`
        // regardless of input length.
        let short_bodies = snapshot_len(10, "A short note body.");
        let long_bodies = snapshot_len(10, &"x".repeat(5_000));
        assert!(
            long_bodies < CEILING_BYTES,
            "long-body snapshot is {long_bodies} bytes, over the {CEILING_BYTES}-byte ceiling — \
             the per-body cap is not actually bounding body-length growth"
        );
        assert!(
            long_bodies < short_bodies * 4,
            "snapshot size grew from {short_bodies} to {long_bodies} bytes across a ~260x \
             per-note body-length increase (same note count) — the per-body cap is not bounding"
        );

        // Axis 3 — per-note ID/TITLE LENGTH: unlike the body, `id` and `title`
        // are model-authored `String`s with no length validation anywhere on
        // the apply path (`upsert_note` accepts them verbatim). A single
        // oversized id or title must not make the block unbounded either.
        fn snapshot_len_with_id_and_title(id: &str, title: &str) -> usize {
            let mut notes = empty_notes();
            notes
                .apply_patch(&notes_patch(1, id, title, "A short note body."), None)
                .expect("apply note");
            render_notes_snapshot(&notes).len()
        }
        let short_id_title = snapshot_len_with_id_and_title("n", "Topic");
        let long_id_title = snapshot_len_with_id_and_title(&"i".repeat(5_000), &"t".repeat(5_000));
        const ID_TITLE_CEILING_BYTES: usize = 2 * NOTES_SNAPSHOT_ID_TITLE_MAX_CHARS * 4 + 200;
        assert!(
            long_id_title < ID_TITLE_CEILING_BYTES,
            "single-note snapshot with a 5,000-char id AND title is {long_id_title} bytes, over \
             the {ID_TITLE_CEILING_BYTES}-byte ceiling — id/title are not actually bounded"
        );
        assert!(
            long_id_title < short_id_title * 20,
            "snapshot size grew from {short_id_title} to {long_id_title} bytes across a 5,000x \
             id/title-length increase (same note count) — id/title are not bounded independent \
             of session growth"
        );
    }

    /// A note title containing an embedded newline must not break the
    /// one-line-per-note invariant [`notes_snapshot_is_bounded_at_the_cap_with_a_count_line`]
    /// relies on (`block.lines().count()`), and must not let note content be
    /// misparsed as a separate snapshot entry.
    #[test]
    fn notes_snapshot_collapses_newlines_in_title_and_id() {
        let mut notes = empty_notes();
        notes
            .apply_patch(
                &notes_patch(
                    1,
                    "note:1",
                    "Multi-line\ntitle\nwith\nembedded\nnewlines",
                    "A short note body.",
                ),
                None,
            )
            .expect("apply note");

        let block = render_notes_snapshot(&notes);
        assert_eq!(
            block.lines().count(),
            1,
            "an embedded newline in the title must not add extra lines to the block, got: {block}"
        );
        assert!(block.contains("Multi-line title with embedded newlines"));
    }

    /// Determinism: the same `MaterializedNotes` renders byte-identically on
    /// every call (no `HashMap` iteration, no wall-clock/random tie-break).
    #[test]
    fn notes_snapshot_has_stable_deterministic_ordering() {
        let mut notes = empty_notes();
        for (i, id) in ["note:c", "note:a", "note:b"].iter().enumerate() {
            notes
                .apply_patch(&notes_patch((i + 1) as u64, id, "Title", "Body."), None)
                .expect("apply note");
        }

        let first = render_notes_snapshot(&notes);
        let second = render_notes_snapshot(&notes);
        assert_eq!(
            first, second,
            "rendering must be a pure, deterministic function of the notes state"
        );

        // Most-recently-updated first: note:b (sequence 3) precedes note:a
        // (sequence 2) precedes note:c (sequence 1).
        let b_index = first.find("note:b").expect("note:b present");
        let a_index = first.find("note:a").expect("note:a present");
        let c_index = first.find("note:c").expect("note:c present");
        assert!(b_index < a_index && a_index < c_index, "got: {first}");
    }

    /// Regression proof for the measured overwrite storm (session ae528252):
    /// job N's APPLIED patch introduces a note id; job N+1's prompt (built
    /// from the resulting `MaterializedNotes`, exactly as a real scheduler
    /// tick would once it can supply `Some(&materialized)`) must show that
    /// same id, so the model has a followable "keep stable note ids"
    /// instruction instead of re-minting blind.
    #[test]
    fn overwrite_storm_regression_next_job_prompt_contains_ids_from_previous_applied_patch() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice chose Soniox for the pilot."))
            .unwrap();

        // Job N's patch is APPLIED to the materialization, exactly as the
        // scheduler's apply path would do after a successful generation.
        let mut notes = empty_notes();
        notes
            .apply_patch(
                &notes_patch(
                    1,
                    "note:decision",
                    "Provider decision",
                    "Alice chose Soniox for the pilot.",
                ),
                None,
            )
            .expect("apply job N's patch");

        // Job N+1's prompt is built against the SAME resulting notes state —
        // this is the exact seam the field measurement showed as blind.
        let job_n_plus_1 = job(ProjectionKind::Notes, &ledger);
        let messages = projection_patch_prompt_messages(&job_n_plus_1, &ledger, Some(&notes))
            .expect("job N+1 prompt");

        let snapshot_message = &messages[3].content;
        assert!(
            snapshot_message.contains("note:decision"),
            "job N+1's prompt must show the note id job N's applied patch introduced, got: {snapshot_message}"
        );
        // The id-stability instruction must actually point at this block, not
        // just assert stability in the abstract.
        assert!(messages[0].content.contains("Current notes state"));
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
        for field in ["type", "id", "title", "body", "tags", "evidence"] {
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

    /// The `schemars`-derived DRAFT schema (vLLM/mistral.rs/generic
    /// OpenAI-compatible routes, and the fallback every strict-schema
    /// rejection downgrades to) must require `evidence` on every
    /// content-creating variant too — not just the hand-authored strict
    /// schema `strict_schema_requires_every_operation_field` already pins.
    /// Before `require_evidence_on_content_creating_operation_variants`,
    /// `#[serde(default)]`'s backward-compat fallback made schemars omit
    /// `evidence` from `required` and advertise `knowledge_gap` — the one
    /// class `judge_claim_evidence` always refuses — as its default.
    #[test]
    fn draft_schema_requires_evidence_on_content_creating_operations() {
        let schema = projection_patch_draft_json_schema().expect("draft schema builds");
        let variants = schema["$defs"]["ProjectionOperation"]["oneOf"]
            .as_array()
            .expect("ProjectionOperation is a oneOf");

        let mut checked_evidence_bearing_variants = 0;
        for variant in variants {
            let Some(evidence) = variant.get("properties").and_then(|p| p.get("evidence")) else {
                continue;
            };
            checked_evidence_bearing_variants += 1;
            let type_const = variant["properties"]["type"]["const"]
                .as_str()
                .expect("variant pins its type const");
            let required: Vec<&str> = variant["required"]
                .as_array()
                .expect("variant lists required fields")
                .iter()
                .map(|v| v.as_str().unwrap())
                .collect();
            assert!(
                required.contains(&"evidence"),
                "{type_const} must require `evidence`, required = {required:?}"
            );
            assert!(
                evidence.get("default").is_none(),
                "{type_const}'s evidence field must not advertise a default \
                 (the always-refused knowledge_gap fallback)"
            );
        }
        assert_eq!(
            checked_evidence_bearing_variants, 3,
            "expected exactly the three content-creating variants \
             (upsert_note / upsert_graph_node / upsert_graph_edge) to carry evidence"
        );
    }

    /// The load-bearing strictness claim: a patch that OBEYS the strict schema
    /// (all required fields present, correct kind) also PASSES the runtime
    /// validator. If the schema were looser than the validator on structure or
    /// kind, a schema-valid fixture could still fail validation — this asserts it
    /// does not, for a representative notes and graph patch (audio-graph-a324).
    #[test]
    fn schema_valid_fixture_passes_the_runtime_validator() {
        let fixture_event = event("span-1", 1, "Alice met Bob.");
        let basis: BTreeMap<&str, &TranscriptEvent> =
            [("span-1", &fixture_event)].into_iter().collect();

        // Notes: every upsert_note field present.
        let notes_fixture = serde_json::json!({
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
        parse_projection_patch_draft(&notes_fixture, &ProjectionKind::Notes, &basis)
            .expect("a schema-obeying notes patch must pass the validator");

        // Graph: node + edge, every field present, weight in range.
        let graph_fixture = serde_json::json!({
            "operations": [
                {
                    "type": "upsert_graph_node",
                    "id": "person:alice",
                    "name": "Alice",
                    "entity_type": "person",
                    "description": null,
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                },
                {
                    "type": "upsert_graph_edge",
                    "id": "edge:alice-bob",
                    "source": "person:alice",
                    "target": "person:bob",
                    "relation_type": "met",
                    "label": null,
                    "weight": 0.5,
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                }
            ],
            "confidence": null
        })
        .to_string();
        parse_projection_patch_draft(&graph_fixture, &ProjectionKind::Graph, &basis)
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

    // ----- audio-graph-2cf9 / ADR-0037: claim-class evidence admission ------

    /// A schema-obeying `UpsertNote` / `UpsertGraphNode` / `UpsertGraphEdge`
    /// WITH a satisfying evidence anchor (`VerifiedQuote`, containment-
    /// checked against a real basis span) passes end to end — the positive
    /// mirror of `schema_valid_fixture_passes_the_runtime_validator`, which
    /// deliberately used the evidence-light `UnavailableEvidence` class so it
    /// stayed focused on structural strictness.
    #[test]
    fn evidence_annotated_upsert_operations_pass_end_to_end() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice chose Soniox for the pilot."))
            .unwrap();
        let notes_job = job(ProjectionKind::Notes, &ledger);
        let notes_raw = serde_json::json!({
            "operations": [{
                "type": "upsert_note",
                "id": "note:decision",
                "title": "Provider decision",
                "body": "Alice chose Soniox for the pilot.",
                "tags": ["decision"],
                "evidence": {
                    "claim_class": "verified_quote",
                    "span_id": "span-1",
                    "quote": "chose Soniox"
                }
            }],
            "confidence": 0.9
        })
        .to_string();
        let notes_patch =
            trusted_projection_patch_from_model_json(&notes_raw, &notes_job, &ledger, context())
                .expect("verified-quote note admits");
        assert_eq!(notes_patch.operations.len(), 1);

        let graph_job = job(ProjectionKind::Graph, &ledger);
        let graph_raw = serde_json::json!({
            "operations": [
                {
                    "type": "upsert_graph_node",
                    "id": "provider:soniox",
                    "name": "Soniox",
                    "entity_type": "provider",
                    "description": null,
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                },
                {
                    "type": "upsert_graph_edge",
                    "id": "edge:alice-soniox",
                    "source": "provider:soniox",
                    "target": "provider:soniox",
                    "relation_type": "self",
                    "label": null,
                    "weight": 0.5,
                    "evidence": {
                        "claim_class": "verified_quote",
                        "span_id": "span-1",
                        "quote": "Alice chose Soniox"
                    }
                }
            ],
            "confidence": 0.85
        })
        .to_string();
        let graph_patch =
            trusted_projection_patch_from_model_json(&graph_raw, &graph_job, &ledger, context())
                .expect("evidence-annotated graph ops admit");
        assert_eq!(graph_patch.operations.len(), 2);
    }

    /// NEGATIVE: an `UpsertNote` whose claim class requires a `span_id`
    /// (`grounded_inference`) but whose anchor is empty fails the WHOLE
    /// patch, surfaced as `ProjectionPatchDraftError::ClaimEvidenceRefused`
    /// (all-or-nothing, the same posture every other structural check here
    /// already has).
    #[test]
    fn upsert_note_missing_required_evidence_refuses_the_whole_patch() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice chose Soniox."))
            .unwrap();
        let job = job(ProjectionKind::Notes, &ledger);
        let raw = serde_json::json!({
            "operations": [{
                "type": "upsert_note",
                "id": "note:decision",
                "title": "Provider decision",
                "body": "Alice chose Soniox.",
                "tags": ["decision"],
                "evidence": {"claim_class": "grounded_inference"}
            }],
            "confidence": 0.9
        })
        .to_string();

        let err = trusted_projection_patch_from_model_json(&raw, &job, &ledger, context())
            .expect_err("missing span_id must refuse the whole patch");

        assert_eq!(
            err,
            ProjectionPatchDraftError::ClaimEvidenceRefused {
                operation: "upsert_note",
                id: "note:decision".to_string(),
                deficiency: crate::claim_evidence::ClaimEvidenceDeficiency::MissingSpanId {
                    class: crate::claim_evidence::ClaimClass::GroundedInference,
                },
            }
        );
    }

    /// NEGATIVE: an `UpsertGraphEdge`'s evidence anchor points at a span that
    /// is real (it is in the ledger) but is NOT part of THIS job's pinned
    /// basis — e.g. it arrived after this job was queued, or belongs to a
    /// different job's window. This proves cross-basis laundering is
    /// impossible: `basis_events` only resolves spans in `job.basis`, so a
    /// span outside it is invisible to `judge_claim_evidence` regardless of
    /// whether it exists elsewhere in the ledger (ADR-0031/ADR-0037).
    #[test]
    fn upsert_graph_edge_evidence_anchored_outside_the_job_basis_is_rejected() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(event("span-1", 1, "Alice chose Soniox."))
            .unwrap();
        // The job's basis is pinned BEFORE span-2 exists.
        let job = job(ProjectionKind::Graph, &ledger);
        ledger
            .apply_event(event("span-2", 1, "Bob prefers Deepgram."))
            .unwrap();

        let raw = serde_json::json!({
            "operations": [
                {
                    "type": "upsert_graph_node",
                    "id": "provider:soniox",
                    "name": "Soniox",
                    "entity_type": "provider",
                    "description": null,
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                },
                {
                    "type": "upsert_graph_node",
                    "id": "provider:deepgram",
                    "name": "Deepgram",
                    "entity_type": "provider",
                    "description": null,
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-1"}
                },
                {
                    "type": "upsert_graph_edge",
                    "id": "edge:soniox-deepgram",
                    "source": "provider:soniox",
                    "target": "provider:deepgram",
                    "relation_type": "competes_with",
                    "label": null,
                    "weight": 0.4,
                    // span-2 is real (just applied to the ledger above) but is
                    // NOT in `job.basis` — this is the laundering attempt.
                    "evidence": {"claim_class": "grounded_inference", "span_id": "span-2"}
                }
            ]
        })
        .to_string();

        let err = trusted_projection_patch_from_model_json(&raw, &job, &ledger, context())
            .expect_err("a span outside the job's basis must be rejected");

        assert_eq!(
            err,
            ProjectionPatchDraftError::ClaimEvidenceRefused {
                operation: "upsert_graph_edge",
                id: "edge:soniox-deepgram".to_string(),
                deficiency: crate::claim_evidence::ClaimEvidenceDeficiency::SpanNotInBasis {
                    span_id: "span-2".to_string(),
                },
            }
        );
    }

    /// `InvalidateNote` is REFUSED from a model-submitted draft (ADR-0037
    /// part 4: "corrections and retractions are derived, not
    /// model-authored") — unlike `InvalidateGraphNode`/`Edge`, its
    /// materialization is a hard delete with no `valid_until_ms` trace, so a
    /// model-authored one would be an unrecoverable hallucination rather
    /// than an auditable soft-invalidate. Deliberately NOT parity with
    /// `invalidate_graph_node`, which the strict schema still offers.
    #[test]
    fn invalidate_note_is_refused_as_a_derived_only_operation() {
        let raw = serde_json::json!({
            "operations": [{
                "type": "invalidate_note",
                "id": "note:retracted"
            }]
        })
        .to_string();

        let error = parse_projection_patch_draft(&raw, &ProjectionKind::Notes, &BTreeMap::new())
            .expect_err("invalidate_note must never be admitted from a model draft");
        assert_eq!(
            error,
            ProjectionPatchDraftError::DerivedOnlyOperation {
                operation: "invalidate_note",
            }
        );

        // The strict schema does not even offer the variant, so a
        // schema-obeying model cannot emit it at all.
        let schema = projection_patch_strict_json_schema(&ProjectionKind::Notes);
        let variant_types: Vec<&str> = schema["properties"]["operations"]["items"]["anyOf"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["properties"]["type"]["enum"][0].as_str().unwrap())
            .collect();
        assert!(!variant_types.contains(&"invalidate_note"));
    }

    /// Permanent regression: the strict schema (both kinds) stays under the
    /// hard 5,000-character strict-mode ceiling after the evidence field was
    /// added, with the per-variant char count LOGGED so a future variant
    /// addition sees the actual remaining headroom instead of the ADR's now-
    /// stale 237-char/variant estimate (ADR-0037's own probe was a throwaway
    /// that got reverted — this test is what replaces re-losing that number).
    #[test]
    fn strict_schema_char_budget_regression_with_per_variant_measurement() {
        const HARD_CEILING: usize = 5_000;

        for kind in [ProjectionKind::Notes, ProjectionKind::Graph] {
            let schema = projection_patch_strict_json_schema(&kind);
            let total_len = schema.to_string().len();
            assert!(
                total_len < HARD_CEILING,
                "{kind:?} strict schema is {total_len} chars, at or over the hard \
                 {HARD_CEILING}-char strict-mode ceiling"
            );

            let variants = schema["properties"]["operations"]["items"]["anyOf"]
                .as_array()
                .expect("operation variants are an anyOf array");
            let mut per_variant = Vec::new();
            for variant in variants {
                let type_const = variant["properties"]["type"]["enum"][0]
                    .as_str()
                    .expect("each variant pins its type const")
                    .to_string();
                let variant_len = variant.to_string().len();
                per_variant.push((type_const, variant_len));
            }
            // A number in the repository, not a claim reasoned about: log every
            // variant's actual serialized size so a future variant addition can
            // read the real remaining headroom off this test's output.
            eprintln!(
                "strict schema char budget — {kind:?}: total={total_len}/{HARD_CEILING}, \
                 per-variant={per_variant:?}"
            );
            assert!(
                !per_variant.is_empty(),
                "{kind:?} strict schema offers no operation variants"
            );
        }
    }

    /// Backward-compat: a `ProjectionPatch` JSON fixture written before this
    /// contract (no `evidence` key on its `upsert_note` operation) still
    /// deserializes — `EvidenceAnchor`'s `Default` fills the gap rather than
    /// failing the whole record, and that default is never re-validated for
    /// an already-materialized historical patch.
    #[test]
    fn pre_contract_projection_patch_fixture_still_deserializes() {
        let legacy_patch_json = serde_json::json!({
            "sequence": 1,
            "kind": "notes",
            "llm_request_id": "req-legacy-1",
            "basis": {
                "span_revisions": [],
                "diarization_span_revisions": [],
                "transcript_hash": "fnv1a64:0000000000000000"
            },
            "operations": [{
                "type": "upsert_note",
                "id": "note:legacy",
                "title": "Legacy note",
                "body": "Written before ADR-0037.",
                "tags": []
            }],
            "confidence": 0.5,
            "provenance": {
                "provider": "llm.api",
                "model": "legacy-model",
                "prompt_id": "legacy-v1"
            },
            "created_at_ms": 1_000
        })
        .to_string();

        let patch: ProjectionPatch =
            serde_json::from_str(&legacy_patch_json).expect("pre-contract patch deserializes");
        assert!(matches!(
            patch.operations.first(),
            Some(ProjectionOperation::UpsertNote { evidence, .. })
                if *evidence == crate::claim_evidence::EvidenceAnchor::default()
        ));
    }

    /// Backward-compat: a `MaterializedNote` JSON fixture written before this
    /// contract (no `evidence` field at all) still deserializes, with
    /// `evidence: None` — never confused with a class-satisfying absence.
    #[test]
    fn pre_contract_materialized_note_fixture_deserializes_with_no_evidence() {
        let legacy_note_json = serde_json::json!({
            "id": "note:legacy",
            "title": "Legacy note",
            "body": "Written before ADR-0037.",
            "tags": [],
            "updated_by_sequence": 1,
            "updated_at_ms": 1_000,
            "basis": {
                "span_revisions": [],
                "diarization_span_revisions": [],
                "transcript_hash": "fnv1a64:0000000000000000"
            },
            "provenance": {
                "provider": "llm.api",
                "model": "legacy-model",
                "prompt_id": "legacy-v1"
            }
        })
        .to_string();

        let note: crate::projections::MaterializedNote =
            serde_json::from_str(&legacy_note_json).expect("pre-contract note deserializes");
        assert!(note.evidence.is_none());
    }

    // ----- audio-graph-a6b5 W1: `heading_level` contract field ------------

    /// Backward-compat, per the `pre_contract_projection_patch_fixture_still_deserializes`
    /// precedent just above: a `ProjectionPatch` JSON fixture written before
    /// W1 (no `heading_level` key at all on its `upsert_note` operation)
    /// still deserializes, with `heading_level: None` — never confused with
    /// a class-satisfying "level 2" default.
    #[test]
    fn pre_w1_projection_patch_fixture_without_heading_level_still_deserializes() {
        let pre_w1_patch_json = serde_json::json!({
            "sequence": 1,
            "kind": "notes",
            "llm_request_id": "req-pre-w1",
            "basis": {
                "span_revisions": [],
                "diarization_span_revisions": [],
                "transcript_hash": "fnv1a64:0000000000000000"
            },
            "operations": [{
                "type": "upsert_note",
                "id": "note:pre-w1",
                "title": "Pre-W1 note",
                "body": "Written before audio-graph-a6b5 W1.",
                "tags": [],
                "evidence": {
                    "claim_class": "verified_quote",
                    "span_id": "span-1",
                    "quote": "written before",
                    "note": null
                }
            }],
            "confidence": 0.5,
            "provenance": {
                "provider": "llm.api",
                "model": "legacy-model",
                "prompt_id": "pre-w1-v1"
            },
            "created_at_ms": 1_000
        })
        .to_string();

        let patch: ProjectionPatch = serde_json::from_str(&pre_w1_patch_json)
            .expect("pre-W1 patch (no heading_level key) deserializes");
        assert!(matches!(
            patch.operations.first(),
            Some(ProjectionOperation::UpsertNote {
                heading_level: None,
                ..
            })
        ));

        // Re-serializing must not resurrect the field: `skip_serializing_if`
        // keeps a `None` invisible, so a session that never saw W1 stays
        // byte-shape-identical on the wire, not just equal-after-parse.
        let round_tripped = serde_json::to_string(&patch).expect("re-serialize");
        assert!(
            !round_tripped.contains("heading_level"),
            "a None heading_level must not appear on the wire: {round_tripped}"
        );
    }

    /// Forward-compat, the general property the whole W1 contract rests on
    /// (design-b §0): `ProjectionOperation` carries no
    /// `#[serde(deny_unknown_fields)]`, so a record carrying a field NEITHER
    /// this build's `upsert_note` NOR its `heading_level` addition knows
    /// about (simulating some future additive field landing after W1) still
    /// deserializes today, exactly like `heading_level` itself would have
    /// deserialized against a pre-W1 build. Losing this property silently
    /// would make the NEXT additive field a strict-reader break instead of a
    /// degrade.
    #[test]
    fn upsert_note_json_with_unrecognized_future_field_still_deserializes() {
        let json_with_unknown_field = serde_json::json!({
            "type": "upsert_note",
            "id": "note:future",
            "title": "Future note",
            "body": "From a build newer than this one.",
            "tags": [],
            "evidence": {
                "claim_class": "grounded_inference",
                "span_id": null,
                "quote": null,
                "note": null
            },
            "heading_level": 3,
            "some_field_this_build_has_never_heard_of": {"nested": ["anything"]}
        })
        .to_string();

        let operation: ProjectionOperation = serde_json::from_str(&json_with_unknown_field)
            .expect("an operation with an unrecognized extra field must still deserialize");
        assert!(matches!(
            operation,
            ProjectionOperation::UpsertNote {
                heading_level: Some(3),
                ..
            }
        ));
    }

    /// design-b §1.2/§1.8, corrected: W1 is a DARK ship on BOTH model-facing
    /// schemas, not just the hand-authored strict one. `heading_level` is a
    /// real serde field on `ProjectionOperation::UpsertNote` (so fresh-ingest
    /// and replay both carry it), but `schemars` derives the non-strict draft
    /// schema straight from that same Rust type — so without
    /// `hide_heading_level_from_draft_schema` post-processing it out, the
    /// draft schema would advertise `heading_level` (plus the field's entire
    /// internal doc comment as the property's `description`) on every
    /// non-strict/vLLM/mistral.rs route, contradicting the "no prompt/schema
    /// surface change" premise this ticket is supposed to hold to and
    /// creating exactly the silent regression design-b §1.8 item 2 names:
    /// the prompt would tell a model a field exists that the strict schema
    /// simultaneously forbids. This test pins BOTH schemas dark and pins the
    /// stronger, model-facing-observable property that actually matters: the
    /// rendered prompt messages contain no trace of `heading_level` at all,
    /// so ADR-0025 §2d's byte-stable cached prefix is untouched by this
    /// ticket. W2 flipping either schema is a deliberate, reviewed edit to
    /// THIS test, not a silent one.
    #[test]
    fn heading_level_is_dark_on_both_draft_and_strict_schemas_and_absent_from_prompt() {
        let draft_schema = projection_patch_draft_json_schema().expect("draft schema builds");
        let draft_upsert_note = draft_schema["$defs"]["ProjectionOperation"]["oneOf"]
            .as_array()
            .expect("draft schema enumerates operation variants")
            .iter()
            .find(|variant| {
                variant["properties"]["type"]["enum"][0] == "upsert_note"
                    || variant["properties"]["type"]["const"] == "upsert_note"
            })
            .expect("draft schema offers upsert_note");
        assert!(
            draft_upsert_note["properties"]
                .get("heading_level")
                .is_none(),
            "W1 ships dark: the schemars-derived draft schema must NOT advertise \
             heading_level yet — it is pasted verbatim into every projection system \
             prompt and sent as the vLLM/mistral.rs decoding grammar, so advertising \
             it here IS a prompt/schema surface change (that is W2's ticket), got: \
             {draft_upsert_note}"
        );
        assert!(
            !draft_schema.to_string().contains("heading_level"),
            "heading_level must not appear ANYWHERE in the draft schema (including \
             nested under a different variant or as a stray description fragment)"
        );

        let strict_schema = projection_patch_strict_json_schema(&ProjectionKind::Notes);
        let strict_upsert_note = strict_schema["properties"]["operations"]["items"]["anyOf"]
            .as_array()
            .expect("strict schema enumerates operation variants")
            .iter()
            .find(|variant| variant["properties"]["type"]["enum"][0] == "upsert_note")
            .expect("strict schema offers upsert_note");
        assert!(
            strict_upsert_note["properties"]
                .get("heading_level")
                .is_none(),
            "W1 ships dark: the strict schema must NOT advertise heading_level yet \
             (that is W2's prompt/schema-exposure ticket), got: {strict_upsert_note}"
        );

        // The stronger, end-to-end assertion: the ACTUAL rendered system
        // prompt (what a model on any route receives) never mentions the
        // field, byte-stable prefix included.
        let mut ledger = TranscriptLedger::new("session-heading-level-dark");
        ledger
            .apply_event(event("span-1", 1, "Alice chose Soniox."))
            .unwrap();
        let job = job(ProjectionKind::Notes, &ledger);
        let messages = projection_patch_prompt_messages(&job, &ledger, Some(&empty_notes()))
            .expect("prompt messages build");
        for message in &messages {
            assert!(
                !message.content.contains("heading_level"),
                "heading_level leaked into a projection prompt message ({}): {}",
                message.role,
                message.content
            );
        }
    }

    /// design-b §1.4/§1.2: an out-of-range `heading_level` is a cosmetic
    /// defect, never a reason to refuse real content — the ingest
    /// normalizer clamps into `2..=4` rather than failing the whole patch.
    /// `None` (no structure asserted) must pass through untouched: clamping
    /// is not a license to invent a depth for an operation that asserted
    /// none at all.
    #[test]
    fn normalize_projection_patch_draft_doc_structure_clamps_heading_level_into_2_to_4() {
        fn upsert_note_with_heading(heading_level: Option<u8>) -> ProjectionOperation {
            ProjectionOperation::UpsertNote {
                id: "note:clamp".to_string(),
                title: "Clamp fixture".to_string(),
                body: "plain body".to_string(),
                tags: Vec::new(),
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
                heading_level,
            }
        }

        let mut draft = ProjectionPatchDraft {
            operations: vec![
                upsert_note_with_heading(Some(0)),
                upsert_note_with_heading(Some(1)),
                upsert_note_with_heading(Some(2)),
                upsert_note_with_heading(Some(3)),
                upsert_note_with_heading(Some(4)),
                upsert_note_with_heading(Some(99)),
                upsert_note_with_heading(Some(u8::MAX)),
                upsert_note_with_heading(None),
            ],
            confidence: None,
        };

        normalize_projection_patch_draft_doc_structure(&mut draft);

        let heading_levels: Vec<Option<u8>> = draft
            .operations
            .iter()
            .map(|operation| match operation {
                ProjectionOperation::UpsertNote { heading_level, .. } => *heading_level,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            heading_levels,
            vec![
                Some(2), // 0 clamps up to the floor
                Some(2), // 1 clamps up to the floor
                Some(2), // already in range
                Some(3), // already in range
                Some(4), // already in range
                Some(4), // 99 clamps down to the ceiling
                Some(4), // u8::MAX clamps down to the ceiling
                None,    // no structure asserted — never fabricated into a depth
            ]
        );
    }

    /// design-b §1.3's validated body grammar: bullet markers normalize to
    /// `-`, indentation snaps to the two-level 0/2/4-space scale, a leading
    /// heading marker is stripped (headings live in `title`, never `body`),
    /// and inline emphasis/link/image/code-fence markup collapses to its
    /// plain text — with the link/image URL discarded, never the visible
    /// text.
    #[test]
    fn normalize_projection_patch_draft_doc_structure_rewrites_body_grammar_markers() {
        let raw_body_lines = [
            "* top bullet",
            "+ also a bullet",
            "  * nested one level",
            "# Stray heading marker",
            "**bold** and _italic_ and `code`",
            "[click here](https://evil.example/track)",
            "![alt text](https://evil.example/pixel.png)",
            "```rust",
            "fn normalize() {}",
            "```",
        ];
        let mut draft = ProjectionPatchDraft {
            operations: vec![ProjectionOperation::UpsertNote {
                id: "note:grammar".to_string(),
                title: "Grammar fixture".to_string(),
                body: raw_body_lines.join("\n"),
                tags: Vec::new(),
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
                heading_level: Some(2),
            }],
            confidence: None,
        };

        normalize_projection_patch_draft_doc_structure(&mut draft);

        let body = match draft.operations.first() {
            Some(ProjectionOperation::UpsertNote { body, .. }) => body.clone(),
            _ => unreachable!(),
        };
        // `split('\n')`, not `.lines()`, so a fence line that normalizes to
        // empty (the last one here) still shows up as its own element
        // instead of being absorbed into "no trailing newline" ambiguity —
        // the normalizer maps 10 input lines to 10 output lines 1:1.
        let lines: Vec<&str> = body.split('\n').collect();
        assert_eq!(
            lines,
            vec![
                "- top bullet",
                "- also a bullet",
                "  - nested one level",
                "Stray heading marker",
                "bold and italic and code",
                "click here",
                "alt text",
                "rust",
                "fn normalize() {}",
                "",
            ],
            "normalized body did not match the expected grammar, got: {body:?}"
        );
    }

    /// design-b §1.3/§1.4: this grammar is XSS-safe because the FRONTEND
    /// never opens an HTML path (React text nodes only) — this normalizer's
    /// job is narrowing the markup vocabulary, never sanitizing or dropping
    /// content. A line this normalizer recognizes no markup in (a raw
    /// `<script>` tag; an arbitrarily long line) must survive completely
    /// unchanged: not stripped, not escaped, not truncated, and — the
    /// ADR-0045 property every clamp-not-refuse claim in this ticket rests
    /// on — never dropped, regardless of how hostile or how large it is.
    #[test]
    fn normalize_projection_patch_draft_doc_structure_never_drops_hostile_or_huge_lines() {
        let huge_line = "x".repeat(50_000);
        let hostile_body = format!(
            "<script>alert(document.cookie)</script>\n{huge_line}\n\
             <img src=x onerror=alert(1)>"
        );
        let mut draft = ProjectionPatchDraft {
            operations: vec![ProjectionOperation::UpsertNote {
                id: "note:hostile".to_string(),
                title: "Hostile fixture".to_string(),
                body: hostile_body.clone(),
                tags: Vec::new(),
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
                heading_level: None,
            }],
            confidence: None,
        };

        let original_line_count = hostile_body.lines().count();
        normalize_projection_patch_draft_doc_structure(&mut draft);

        let body = match draft.operations.first() {
            Some(ProjectionOperation::UpsertNote { body, .. }) => body.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            body.lines().count(),
            original_line_count,
            "no line may be dropped by the normalizer, got: {body:?}"
        );
        assert!(
            body.contains("<script>alert(document.cookie)</script>"),
            "a script tag is not markup this grammar recognizes — it must survive \
             byte-for-byte as inert plain text, got: {body:?}"
        );
        assert!(
            body.contains(&huge_line),
            "a huge line must never be truncated"
        );
        assert!(
            body.contains("<img src=x onerror=alert(1)>"),
            "an inline event-handler attribute is likewise inert plain text here, \
             got: {body:?}"
        );
    }

    /// `*`, `_`, and `` ` `` are constant in this app's own domain (meeting
    /// notes about software): identifiers, math, and file paths. Only a
    /// PAIRED, plausibly-delimiting run may be stripped — an isolated or
    /// intraword occurrence of these bytes is content, not markup, and must
    /// survive normalization byte-for-byte.
    #[test]
    fn normalize_projection_patch_draft_doc_structure_preserves_incidental_marker_characters() {
        let raw_body_lines = [
            "call my_function_name with snake_case",
            "src-tauri/src/projection_llm.rs",
            "2 * 3 = 6 and a*b",
            "the __init__ dunder is a real Python identifier",
        ];
        let mut draft = ProjectionPatchDraft {
            operations: vec![ProjectionOperation::UpsertNote {
                id: "note:incidental-markers".to_string(),
                title: "Incidental markers fixture".to_string(),
                body: raw_body_lines.join("\n"),
                tags: Vec::new(),
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
                heading_level: None,
            }],
            confidence: None,
        };

        normalize_projection_patch_draft_doc_structure(&mut draft);

        let body = match draft.operations.first() {
            Some(ProjectionOperation::UpsertNote { body, .. }) => body.clone(),
            _ => unreachable!(),
        };
        let lines: Vec<&str> = body.split('\n').collect();
        assert_eq!(
            lines,
            vec![
                "call my_function_name with snake_case",
                "src-tauri/src/projection_llm.rs",
                "2 * 3 = 6 and a*b",
                // `__init__` is left to the same fate a real Markdown
                // renderer gives it (both leading and trailing `__` sit at a
                // word boundary on the outer side, exactly like `_italic_`
                // above — there is no local, structural way to tell a
                // dunder identifier from deliberate bold apart without a
                // vocabulary-specific heuristic, which is out of scope for
                // this formatting-grammar normalizer).
                "the init dunder is a real Python identifier",
            ],
            "an incidental/intraword marker must not be treated as emphasis, got: {body:?}"
        );
    }

    /// design-b §1.3: a markdown link whose `(url` target never finds its
    /// closing `)` on the line (e.g. a long URL wrapped across a line break)
    /// must not silently drop the rest of the line — only the recognized
    /// `[text](url)` shape may be rewritten; anything that does not fully
    /// resolve to that shape is preserved verbatim, mirroring the existing
    /// unmatched-`[` fallback.
    #[test]
    fn normalize_projection_patch_draft_doc_structure_preserves_unterminated_link_target() {
        let raw_body = "[see notes](incomplete url then MORE TEXT";
        let mut draft = ProjectionPatchDraft {
            operations: vec![ProjectionOperation::UpsertNote {
                id: "note:unterminated-link".to_string(),
                title: "Unterminated link fixture".to_string(),
                body: raw_body.to_string(),
                tags: Vec::new(),
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
                heading_level: None,
            }],
            confidence: None,
        };

        normalize_projection_patch_draft_doc_structure(&mut draft);

        let body = match draft.operations.first() {
            Some(ProjectionOperation::UpsertNote { body, .. }) => body.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            body, raw_body,
            "an unterminated link target must preserve the entire line, not silently \
             drop everything after '(': got {body:?}"
        );
    }

    /// A tab-indented bullet used to fall through to the paragraph branch
    /// (only ASCII space was ever counted as indent), so the `* `/`- ` prefix
    /// was left in place for `strip_doc_body_markup` to eat as an emphasis
    /// run — the bullet lost its marker AND its semantics. A leading tab now
    /// counts as indentation (weighted like two spaces) before bullet
    /// detection runs.
    #[test]
    fn normalize_projection_patch_draft_doc_structure_keeps_tab_indented_bullet_marker() {
        let raw_body = "\t* tab-indented bullet";
        let mut draft = ProjectionPatchDraft {
            operations: vec![ProjectionOperation::UpsertNote {
                id: "note:tab-bullet".to_string(),
                title: "Tab bullet fixture".to_string(),
                body: raw_body.to_string(),
                tags: Vec::new(),
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
                heading_level: None,
            }],
            confidence: None,
        };

        normalize_projection_patch_draft_doc_structure(&mut draft);

        let body = match draft.operations.first() {
            Some(ProjectionOperation::UpsertNote { body, .. }) => body.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            body, "  - tab-indented bullet",
            "a tab-indented bullet must keep its bullet marker, got: {body:?}"
        );
    }

    /// `require_non_empty` (called by `validate_projection_patch_draft`
    /// BEFORE normalization) only ever sees the model's raw string — it
    /// cannot see that `normalize_doc_body` is about to reduce an all-markup
    /// body to nothing. Without a post-normalization re-check, a
    /// content-free note (raw body `` "```" `` or `"#"`, a bare code-fence
    /// delimiter or heading marker with nothing else on the line) would sail
    /// past the non-empty guarantee and land in the canonical log.
    ///
    /// A bare `"**"` or `` "`" `` (no enclosed text to strip, just one
    /// isolated delimiter run) is deliberately NOT in this list:
    /// [`strip_delimiter_pairs`]/[`strip_code_spans`] only remove a marker
    /// that pairs with a LATER marker of the same kind — an unpaired run has
    /// nothing to strip, so it is preserved as literal, non-empty text
    /// instead of vanishing. That is the fix for the "every `*`/`_`/`` ` ``
    /// deleted unconditionally" corruption bug, not a gap in this guarantee.
    #[test]
    fn parse_projection_patch_draft_rejects_a_body_that_normalizes_to_empty() {
        let fixture_event = event("span-1", 1, "Alice met Bob.");
        let basis: BTreeMap<&str, &TranscriptEvent> =
            [("span-1", &fixture_event)].into_iter().collect();

        for all_markup_body in ["```", "#"] {
            let raw = serde_json::json!({
                "operations": [{
                    "type": "upsert_note",
                    "id": "note:all-markup",
                    "title": "All-markup fixture",
                    "body": all_markup_body,
                    "tags": [],
                    "evidence": {
                        "claim_class": "grounded_inference",
                        "span_id": "span-1",
                        "quote": null,
                        "note": null
                    }
                }]
            })
            .to_string();

            let result = parse_projection_patch_draft(&raw, &ProjectionKind::Notes, &basis);
            assert!(
                matches!(
                    result,
                    Err(ProjectionPatchDraftError::EmptyOperationField { field: "body", .. })
                ),
                "a body that normalizes to empty ({all_markup_body:?}) must be rejected \
                 post-normalization, got: {result:?}"
            );
        }
    }

    /// THE fresh-ingest-vs-replay seam, pinned explicitly (ADR-0045 / the
    /// e700 precedent design-b §1.4 cites): a hostile/unnormalized body
    /// already sitting in an OLD accepted `projection_patches` log — the
    /// exact shape a session recorded before this normalizer existed — must
    /// replay UNTOUCHED. Deserializing a `ProjectionPatch` directly (what
    /// replay does) never calls `normalize_projection_patch_draft_doc_structure`;
    /// only `parse_projection_patch_draft` (the fresh-ingest path a freshly
    /// generated model draft goes through) does. Rewriting a historical
    /// operation's `body` at replay time would make replay depend on
    /// whichever normalization rules are in force today instead of the rules
    /// in force when the patch was accepted — exactly what ADR-0045 forbids.
    #[test]
    fn hostile_body_in_an_old_accepted_log_replays_untouched_never_normalized() {
        let raw_hostile_body = "* unnormalized bullet\n<script>alert(1)</script>\n#stray heading";
        let old_accepted_patch_json = serde_json::json!({
            "sequence": 1,
            "kind": "notes",
            "llm_request_id": "req-old-hostile",
            "basis": {
                "span_revisions": [],
                "diarization_span_revisions": [],
                "transcript_hash": "fnv1a64:0000000000000000"
            },
            "operations": [{
                "type": "upsert_note",
                "id": "note:old-hostile",
                "title": "Old accepted note",
                "body": raw_hostile_body,
                "tags": [],
                "evidence": {
                    "claim_class": "grounded_inference",
                    "span_id": "span-1",
                    "quote": null,
                    "note": null
                }
            }],
            "confidence": 0.5,
            "provenance": {
                "provider": "llm.api",
                "model": "old-model",
                "prompt_id": "old-v1"
            },
            "created_at_ms": 1_000
        })
        .to_string();

        // Replay's actual path: a straight `ProjectionPatch` deserialize,
        // never through `parse_projection_patch_draft`.
        let replayed_patch: ProjectionPatch = serde_json::from_str(&old_accepted_patch_json)
            .expect("old accepted patch deserializes for replay");
        let replayed_body = match replayed_patch.operations.first() {
            Some(ProjectionOperation::UpsertNote { body, .. }) => body.clone(),
            _ => unreachable!(),
        };
        assert_eq!(
            replayed_body, raw_hostile_body,
            "replay must never rewrite a historical body — the log content is the \
             source of truth, got: {replayed_body:?}"
        );

        // Contrast: the SAME raw operations JSON, if it arrived TODAY through
        // the REAL fresh-ingest production seam (`parse_projection_patch_draft`,
        // not a direct call to the normalizer), WOULD be rewritten — proving
        // the seam is actually wired up, not just that the normalizer function
        // does something when called directly.
        let fixture_event = event("span-1", 1, "Alice met Bob.");
        let basis: BTreeMap<&str, &TranscriptEvent> =
            [("span-1", &fixture_event)].into_iter().collect();
        let raw_operations_only = serde_json::json!({ "operations": old_accepted_patch_json_operations(&old_accepted_patch_json) })
            .to_string();
        let fresh_draft =
            parse_projection_patch_draft(&raw_operations_only, &ProjectionKind::Notes, &basis)
                .expect("fresh-ingest draft parses");
        let fresh_body = match fresh_draft.operations.first() {
            Some(ProjectionOperation::UpsertNote { body, .. }) => body.clone(),
            _ => unreachable!(),
        };
        assert_ne!(
            fresh_body, replayed_body,
            "the fresh-ingest seam must actually rewrite this input, or this test would \
             not be distinguishing fresh-ingest from replay at all"
        );
        assert!(fresh_body.starts_with("- unnormalized bullet\n"));
    }

    /// Pulls `operations` back out of the JSON built above so the SAME
    /// operation payload is reused for both the replay-path deserialize and
    /// the fresh-ingest-path `parse_projection_patch_draft` call, rather than
    /// hand-duplicating the fixture and risking the two drifting apart.
    fn old_accepted_patch_json_operations(patch_json: &str) -> serde_json::Value {
        let parsed: serde_json::Value = serde_json::from_str(patch_json).unwrap();
        parsed["operations"].clone()
    }
}
