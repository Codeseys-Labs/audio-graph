import { describe, expect, it } from "vitest";
import type {
  OrgKnowledgeItem,
  PromotionAcl,
  PromotionEvent,
  PromotionRetention,
  PromotionSyncSnapshot,
  RedactionSnapshot,
} from "./index";

describe("promotion schema types", () => {
  it("models approved org payloads without raw transcript or private source fields", () => {
    const promotion = samplePromotionEvent();
    const item = sampleOrgKnowledgeItem();
    const snapshot = sampleRedactionSnapshot();

    expect(promotion.payload_snapshot.fields).not.toHaveProperty(
      "raw_transcript_text",
    );
    expect(item.redacted_payload.fields).not.toHaveProperty("speaker_names");
    expect(item.redacted_payload.fields).not.toHaveProperty("provider_ids");
    expect(snapshot).not.toHaveProperty("payload_before");
    expect(item.state).toBe("active");
    expect(item.sync_state.status).toBe("not_configured");
  });
});

function samplePromotionEvent(): PromotionEvent {
  return {
    id: "promotion-1",
    schema_version: 1,
    created_at_ms: 1_700_000_000_000,
    actor: {
      actor_user_id: "user-1",
      actor_local_profile_id: "profile-1",
      actor_device_id: "device-1",
    },
    target: {
      source_workspace_id: "local-workspace",
      target_org_id: "org-1",
      target_workspace_id: "workspace-1",
      target_collection_id: "collection-1",
    },
    source: {
      source_object_type: "materialized_note",
      source_object_id: "note-1",
      source_object_version: "sequence:7",
      source_session_id: "session-1",
      source_span_ids: ["span-1"],
      source_projection_sequence: 7,
      source_basis_hash: "sha256:basis",
      source_hash: "sha256:source",
      source_basis: {
        transcript_hash: "sha256:transcript",
      },
      source_provenance: {
        asr_provider: "soniox",
        source_id: "default-mic",
        speaker_ids: ["speaker-local-1"],
        span_revisions: [{ span_id: "span-1", revision_number: 2 }],
        llm: {
          provider: "openrouter",
          model: "test-model",
          prompt_id: "projection-v1",
        },
        confidence: 0.91,
        created_at_ms: 1_700_000_000_000,
        updated_at_ms: 1_700_000_000_100,
      },
    },
    redaction: {
      redaction_policy_id: "policy-1",
      redaction_policy_version: "2026-06-26",
      redaction_snapshot_hash: "sha256:redaction",
      redaction_diff: [
        {
          field: "body",
          reason: "speaker_name",
          before_hash: "sha256:before",
          after_hash: "sha256:after",
        },
      ],
      redacted_fields: ["speaker_name"],
      manual_redaction_overrides: ["alias-speaker-a"],
    },
    reviewer_user_id: "reviewer-1",
    approved_payload_hash: "sha256:approved",
    payload_snapshot: {
      kind: "note",
      title: "Approved summary",
      body: "Redacted approved body.",
      fields: { topic: "roadmap" },
      approved_payload_hash: "sha256:approved",
    },
    acl: sampleAcl(),
    retention: sampleRetention(),
    sync: disabledSync(),
    lineage: {
      conflict_group_id: "conflict-group-1",
    },
    conflict_state: "none",
    requested_at_ms: 1_700_000_000_000,
    approved_at_ms: 1_700_000_000_100,
    status: "approved_local",
  };
}

function sampleOrgKnowledgeItem(): OrgKnowledgeItem {
  return {
    id: "org-item-1",
    schema_version: 1,
    org_id: "org-1",
    workspace_id: "workspace-1",
    kind: "note",
    current_revision_id: "org-item-1-r1",
    revision_number: 1,
    title: "Approved summary",
    body: "Redacted approved body.",
    tags: ["roadmap"],
    content_hash: "sha256:content",
    redacted_payload: {
      kind: "note",
      title: "Approved summary",
      body: "Redacted approved body.",
      fields: { topic: "roadmap" },
      approved_payload_hash: "sha256:approved",
    },
    confidence: 0.91,
    source_promotion_event_id: "promotion-1",
    promotion_event_ids: ["promotion-1"],
    source_local_object_fingerprint: "sha256:local-object",
    source_session_fingerprint: "sha256:session",
    provenance_summary: "Approved local note from session-1",
    full_provenance_pointer: "promotion://promotion-1",
    acl: sampleAcl(),
    retention: sampleRetention(),
    created_by_user_id: "reviewer-1",
    created_at_ms: 1_700_000_000_100,
    updated_at_ms: 1_700_000_000_100,
    valid_from_ms: 1_700_000_000_100,
    state: "active",
    conflict_state: "none",
    sync_state: disabledSync(),
  };
}

function sampleRedactionSnapshot(): RedactionSnapshot {
  return {
    id: "redaction-1",
    schema_version: 1,
    promotion_event_id: "promotion-1",
    source_object_type: "materialized_note",
    source_object_id: "note-1",
    policy_id: "policy-1",
    policy_version: "2026-06-26",
    redacted_fields: ["speaker_name"],
    removed_span_ids: ["span-private"],
    speaker_alias_map: {
      "speaker-local-1": "Speaker A",
    },
    entity_alias_map: {},
    manual_overrides: ["remove private name"],
    payload_before_hash: "sha256:before",
    payload_after_hash: "sha256:after",
    approved_payload_hash: "sha256:approved",
    reviewed_by_user_id: "reviewer-1",
    reviewed_at_ms: 1_700_000_000_100,
  };
}

function sampleAcl(): PromotionAcl {
  return {
    acl_policy_id: "acl-1",
    acl_visibility: "workspace",
    acl_principals: ["workspace:workspace-1"],
    acl_inheritance_mode: "narrower_of_source_and_target",
  };
}

function sampleRetention(): PromotionRetention {
  return {
    retention_policy_id: "retention-1",
    retention_legal_basis: "user_approved_org_memory",
    retention_category: "org_knowledge",
    delete_behavior: "retract_remote",
  };
}

function disabledSync(): PromotionSyncSnapshot {
  return {
    target_kind: "disabled",
    status: "not_configured",
  };
}
