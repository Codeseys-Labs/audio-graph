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
    ProjectionPatch, ProjectionProvenance, TranscriptEvent, TranscriptLedger,
};

pub const PROJECTION_PATCH_PROMPT_ID: &str = "projection_patch_v1";
pub const PROJECTION_PATCH_REPAIR_PROMPT_ID: &str = "projection_patch_repair_v1";

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

pub fn projection_patch_prompt_messages(
    job: &ProjectionJob,
    ledger: &TranscriptLedger,
) -> Result<Vec<ChatMessage>, ProjectionPatchDraftError> {
    ledger
        .validate_basis(&job.basis)
        .map_err(|staleness| ProjectionPatchDraftError::StaleBasis { staleness })?;

    let events = basis_events(job, ledger)?;
    let transcript = format_transcript_events_json(&events);
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

    Ok(vec![
        ChatMessage {
            role: "system".to_string(),
            content: format!(
                "You generate AudioGraph projection patch drafts. Return strict JSON only, with no markdown. \
                 Do not include trusted metadata such as sequence, basis, provenance, session_id, or llm_request_id; \
                 the backend stamps those fields. {operation_guidance}"
            ),
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
                 Current transcript basis:\n{transcript}\n\n\
                 Output JSON schema:\n{schema}\n\n\
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

        assert_eq!(messages.len(), 4);
        assert_eq!(messages[2].role, "assistant");
        assert!(messages[2].content.contains("upsert_graph_node"));
        assert_eq!(messages[3].role, "user");
        assert!(messages[3].content.contains("expected_kind: notes"));
        assert!(messages[3].content.contains("validation_error:"));
        assert!(messages[3].content.contains("upsert_graph_node"));
        assert!(messages[3].content.contains("Output JSON schema"));
        assert!(
            messages[3]
                .content
                .contains("Do not include trusted metadata")
        );
    }
}
