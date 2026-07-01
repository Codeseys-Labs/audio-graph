# Org Knowledge Promotion Schema

Date: 2026-06-26
Seed: audio-graph-d115

## Recommendation

Treat organization knowledge as an explicit promoted copy of selected local memory, never as an automatic sync mirror of a private session.

AudioGraph should keep local transcript, notes, graph facts, speaker timeline, and live-assist cards private by default. A user action can promote a specific object version into an org workspace after redaction review. The promoted record must carry provenance back to the source session/object/version, but the org copy must only contain the redacted payload that the user approved.

## Boundaries

Local private memory:

- transcript span revisions;
- materialized notes and note revisions;
- graph node/edge facts;
- speaker timeline segments;
- live-assist cards and action outcomes;
- provider/model/prompt provenance;
- unredacted local-only text.

Org knowledge:

- selected note/fact/card versions;
- approved redacted text;
- minimal source references for audit;
- ACL and retention metadata;
- conflict and sync state.

Non-goals for the first implementation:

- automatic full-session upload;
- automatic speaker identity sharing;
- org-wide raw transcript sync;
- hidden provider-side upload of local private memory;
- collaborative editing of private local sessions.

## Core Records

### `promotion_event`

Immutable local event recording the user action.

Fields:

- `id`
- `schema_version`
- `created_at_ms`
- `actor_user_id`
- `actor_local_profile_id`
- `actor_device_id`
- `delegated_service_id`
- `source_workspace_id`
- `target_org_id`
- `target_workspace_id`
- `target_collection_id`
- `source_object_type`: `materialized_note`, `graph_node_fact`, `graph_edge_fact`, `live_assist_card`, `transcript_span`
- `source_object_id`
- `source_object_version`
- `source_session_id`
- `source_span_ids`
- `source_projection_sequence`
- `source_basis_hash`
- `source_hash`
- `source_basis`: the `ProjectionBasis` or transcript span revision basis that was current at review time
- `source_provenance`: ASR provider/source/speaker/span revisions plus LLM provider, model, prompt id, confidence, created/updated timestamps
- `redaction_policy_id`
- `redaction_policy_version`
- `redaction_snapshot_hash`
- `redaction_diff`
- `redacted_fields`
- `manual_redaction_overrides`
- `reviewer_user_id`
- `approved_payload_hash`
- `payload_snapshot`: exact approved redacted title/body/fact/card content
- `acl_policy_id`
- `acl_visibility`
- `acl_principals`
- `acl_inheritance_mode`
- `retention_policy_id`
- `retention_legal_basis`
- `retention_category`
- `expires_at_ms`
- `delete_behavior`
- `parent_promotion_id`
- `supersedes_promotion_id`
- `conflict_group_id`
- `requested_at_ms`
- `approved_at_ms`
- `status`: `draft`, `redaction_required`, `ready_to_promote`, `rejected`, `queued`, `validated`, `blocked_by_stale_source`, `blocked_by_redaction`, `approved_local`, `queued_sync`, `synced`, `failed`, `revoked`
- `sync_target_id`
- `sync_error_code`
- `sync_error_message_redacted`

### `redaction_snapshot`

The exact user-approved payload boundary.

Fields:

- `id`
- `schema_version`
- `promotion_event_id`
- `source_object_type`
- `source_object_id`
- `policy_id`
- `policy_version`
- `redacted_fields`
- `removed_span_ids`
- `speaker_alias_map`
- `entity_alias_map`
- `manual_overrides`
- `payload_before_hash`
- `payload_after_hash`
- `reviewed_by_user_id`
- `reviewed_at_ms`

The pre-redaction payload itself should remain local unless the user explicitly exports it. Hashes prove what was reviewed without copying private raw text into org storage.

### `org_knowledge_item`

The org-visible copy.

Fields:

- `id`
- `schema_version`
- `org_id`
- `workspace_id`
- `kind`: `note`, `graph_fact`, `live_card`, `decision`, `commitment`, `question`, `risk`
- `current_revision_id`
- `revision_number`
- `title`
- `body`
- `tags`
- `content_hash`
- `redacted_payload`
- `graph_subject_id`
- `graph_object_id`
- `relation_type`
- `confidence`
- `source_promotion_event_id`
- `promotion_event_ids`
- `source_local_object_fingerprint`
- `source_session_fingerprint`
- `provenance_summary`
- `full_provenance_pointer`
- `acl_policy_id`
- `acl_effective`
- `retention_policy_id`
- `retention_effective`
- `created_by_user_id`
- `created_at_ms`
- `updated_at_ms`
- `valid_from_ms`
- `valid_until_ms`
- `deleted_at_ms`
- `delete_reason`
- `state`: `active`, `superseded`, `retracted`, `deleted`, `retention_expired`, `purge_pending`, `purged`
- `conflict_group_id`
- `conflict_state`
- `sync_state`
- `remote_revision`

### `promotion_sync_state`

Per-target transport state.

Fields:

- `promotion_event_id`
- `target_kind`: `surrealdb_remote`, `api_server`, `file_export`, `disabled`
- `remote_id`
- `remote_revision`
- `remote_etag`
- `queued_at_ms`
- `last_attempt_at_ms`
- `last_success_at_ms`
- `retry_count`
- `status`: `not_configured`, `not_synced`, `queued`, `sync_pending`, `in_flight`, `syncing`, `synced`, `conflict`, `permission_denied`, `redaction_required`, `retryable_error`, `permanent_error`, `auth_required`, `failed`, `revoked`
- `last_error_code`
- `last_error_message_redacted`

## Conflict Model

Conflicts should be object-version conflicts, not whole-session conflicts.

Required cases:

- Local note/fact changes after promotion: create a new promotion candidate, do not mutate the org item silently.
- Org item edited remotely: preserve remote revision and show local update as a proposed superseding version.
- User revokes a promotion: mark org item `retracted` or `deleted` according to target policy and keep a local audit event.
- Redaction policy changes: require re-review before syncing a previously approved local object version.
- Source local object deleted: preserve org item only if policy allows it; otherwise queue revocation.
- Stale projection basis: if a promoted item's `ProjectionBasis` is stale against the current transcript ledger, mark `blocked_by_stale_source` or `source_superseded` and require a new promotion event or explicit repair.
- ACL or retention conflict: do not sync until manual resolution chooses the narrower policy or a reviewer approves the broader policy.
- Remote tombstone conflict: preserve local audit state and require explicit restore/re-promote rather than recreating the org item automatically.

Minimum conflict states:

- `none`
- `remote_newer`
- `local_redaction_changed`
- `source_superseded`
- `acl_conflict`
- `retention_conflict`
- `tombstone_conflict`
- `manual_resolution_required`

## UX Rules

- Promotion begins from a concrete local object version.
- UI shows a preview of the org-visible payload before sync.
- Raw transcript spans and speaker names are opt-in, not default.
- The user can promote a summary without promoting underlying transcript text.
- Shared items show provenance in human terms: source meeting, approximate time, generated/provider status, and user approval status.
- Failed sync does not corrupt local notes/graph state.

## Repository Contract

Extend the eventual `LocalMemoryRepository` with promotion methods after the base repository exists:

```rust
trait PromotionRepository {
    fn create_promotion_draft(&self, draft: PromotionDraft) -> Result<PromotionEvent>;
    fn save_redaction_snapshot(&self, snapshot: RedactionSnapshot) -> Result<()>;
    fn approve_promotion(&self, id: &str, approved_payload: ApprovedOrgPayload) -> Result<PromotionEvent>;
    fn upsert_org_knowledge_item(&self, item: OrgKnowledgeItem) -> Result<()>;
    fn update_sync_state(&self, state: PromotionSyncState) -> Result<()>;
    fn revoke_promotion(&self, id: &str, reason: RedactedReason) -> Result<PromotionEvent>;
}
```

Keep this trait backend-owned. React should see previews, state, and actions, but not direct database credentials or unreviewed secret/private payloads.

## Tests

Minimum tests before any cloud/federated sync:

- schema validation rejects missing actor, target, source object version, redaction snapshot, ACL, retention, and sync state;
- promotion from `MaterializedNote` preserves session id, `updated_by_sequence`, basis, provenance, and redacted payload hash;
- promotion from graph node/edge facts preserves validity intervals, confidence, basis, and LLM provenance;
- stale `ProjectionBasis` maps to `blocked_by_stale_source` or `source_superseded`;
- redaction preview omits raw transcript text and speaker names by default;
- approving a promotion stores payload hashes and approved redacted body;
- changing the local source object creates a new draft instead of mutating synced org item;
- revocation writes a local audit event and sync-state transition;
- sync errors are redacted and do not include provider credentials or raw transcript excerpts;
- import/export round-trip preserves provenance and retention state;
- local deletion policy is explicit for promoted items.
- soft-deleted or retention-expired local sessions cannot create new promotions without explicit restore/review;
- live-assist cards cannot be promoted until represented as durable `live_assist_card` revisions.

## No-Cloud-Sync Guard

Until this schema, redaction snapshot model, repository methods, and tests exist, cloud/federated sync commands should remain absent or feature-blocked. This prevents a partial implementation from accidentally uploading local private sessions or raw provider output before the privacy boundary is enforceable.

## Follow-Up Seeds

- Build promotion repository records after `audio-graph-5679` lands.
- Add promotion preview UI and redaction review flow.
- Add org sync transport only after local repository, redaction, and audit behavior are tested.
- Add retention/deletion policy tests for promoted items.
