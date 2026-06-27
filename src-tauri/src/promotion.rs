//! Local-to-organization knowledge promotion contracts.
//!
//! These types define the privacy boundary for future org knowledge sync. They
//! intentionally do not add repository methods or network commands: promotion is
//! explicit, redacted, revisioned, and auditable before any transport exists.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::projections::{
    ProjectionBasis, ProjectionBasisSpan, ProjectionBasisStaleness, ProjectionProvenance,
    TranscriptLedger,
};

pub const PROMOTION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromotionSourceObjectType {
    MaterializedNote,
    GraphNodeFact,
    GraphEdgeFact,
    LiveAssistCard,
    TranscriptSpan,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromotionStatus {
    Draft,
    RedactionRequired,
    ReadyToPromote,
    Rejected,
    Queued,
    Validated,
    BlockedByStaleSource,
    BlockedByRedaction,
    ApprovedLocal,
    QueuedSync,
    Synced,
    Failed,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrgKnowledgeKind {
    Note,
    GraphFact,
    LiveCard,
    Decision,
    Commitment,
    Question,
    Risk,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrgKnowledgeState {
    Active,
    Superseded,
    Retracted,
    Deleted,
    RetentionExpired,
    PurgePending,
    Purged,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromotionConflictState {
    None,
    RemoteNewer,
    LocalRedactionChanged,
    SourceSuperseded,
    AclConflict,
    RetentionConflict,
    TombstoneConflict,
    ManualResolutionRequired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromotionSyncTargetKind {
    SurrealdbRemote,
    ApiServer,
    FileExport,
    Disabled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromotionSyncStatus {
    NotConfigured,
    NotSynced,
    Queued,
    SyncPending,
    InFlight,
    Syncing,
    Synced,
    Conflict,
    PermissionDenied,
    RedactionRequired,
    RetryableError,
    PermanentError,
    AuthRequired,
    Failed,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AclVisibility {
    Private,
    Workspace,
    Org,
    Principals,
    PublicLink,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AclInheritanceMode {
    None,
    WorkspaceDefault,
    CollectionDefault,
    NarrowerOfSourceAndTarget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RetentionCategory {
    PersonalNote,
    MeetingMemory,
    OrgKnowledge,
    Regulated,
    Ephemeral,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeleteBehavior {
    Tombstone,
    RetractRemote,
    PurgeLocalAndRemote,
    PreserveApprovedSnapshot,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PromotionSourceSessionState {
    Active,
    SoftDeleted,
    RetentionExpired,
    ExplicitlyRestoredForReview,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromotionSourceReview {
    pub status: PromotionStatus,
    pub conflict_state: PromotionConflictState,
    pub basis_staleness: Option<ProjectionBasisStaleness>,
    pub reason_code: &'static str,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PromotionActor {
    pub actor_user_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_local_profile_id: Option<String>,
    pub actor_device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_service_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PromotionTarget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_workspace_id: Option<String>,
    pub target_org_id: String,
    pub target_workspace_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_collection_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PromotionSourceProvenance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    #[serde(default)]
    pub speaker_ids: Vec<String>,
    #[serde(default)]
    pub span_revisions: Vec<ProjectionBasisSpan>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<ProjectionProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PromotionSourceReference {
    pub source_object_type: PromotionSourceObjectType,
    pub source_object_id: String,
    pub source_object_version: String,
    pub source_session_id: String,
    #[serde(default)]
    pub source_span_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_projection_sequence: Option<u64>,
    pub source_basis_hash: String,
    pub source_hash: String,
    pub source_basis: ProjectionBasis,
    pub source_provenance: PromotionSourceProvenance,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RedactionDiffEntry {
    pub field: String,
    pub reason: String,
    pub before_hash: String,
    pub after_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PromotionRedactionSummary {
    pub redaction_policy_id: String,
    pub redaction_policy_version: String,
    pub redaction_snapshot_hash: String,
    #[serde(default)]
    pub redaction_diff: Vec<RedactionDiffEntry>,
    #[serde(default)]
    pub redacted_fields: Vec<String>,
    #[serde(default)]
    pub manual_redaction_overrides: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ApprovedOrgPayload {
    pub kind: OrgKnowledgeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default)]
    pub fields: BTreeMap<String, serde_json::Value>,
    pub approved_payload_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PromotionAcl {
    pub acl_policy_id: String,
    pub acl_visibility: AclVisibility,
    #[serde(default)]
    pub acl_principals: Vec<String>,
    pub acl_inheritance_mode: AclInheritanceMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PromotionRetention {
    pub retention_policy_id: String,
    pub retention_legal_basis: String,
    pub retention_category: RetentionCategory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    pub delete_behavior: DeleteBehavior,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PromotionLineage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_promotion_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_promotion_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conflict_group_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PromotionSyncSnapshot {
    pub target_kind: PromotionSyncTargetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_target_id: Option<String>,
    pub status: PromotionSyncStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_error_message_redacted: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PromotionEvent {
    pub id: String,
    pub schema_version: u32,
    pub created_at_ms: u64,
    pub actor: PromotionActor,
    pub target: PromotionTarget,
    pub source: PromotionSourceReference,
    pub redaction: PromotionRedactionSummary,
    pub reviewer_user_id: String,
    pub approved_payload_hash: String,
    pub payload_snapshot: ApprovedOrgPayload,
    pub acl: PromotionAcl,
    pub retention: PromotionRetention,
    pub sync: PromotionSyncSnapshot,
    pub lineage: PromotionLineage,
    pub conflict_state: PromotionConflictState,
    pub requested_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_at_ms: Option<u64>,
    pub status: PromotionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PromotionDraft {
    pub id: String,
    pub schema_version: u32,
    pub created_at_ms: u64,
    pub actor: PromotionActor,
    pub target: PromotionTarget,
    pub source: PromotionSourceReference,
    pub candidate_payload_hash: String,
    #[serde(default)]
    pub requested_redaction_fields: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reviewer_user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note_redacted: Option<String>,
    pub status: PromotionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PromotionRevocationRequest {
    pub id: String,
    pub schema_version: u32,
    pub promotion_event_id: String,
    pub org_knowledge_item_id: String,
    pub requested_by_user_id: String,
    pub requested_at_ms: u64,
    pub reason_code: String,
    pub reason_redacted: String,
    pub target_kind: PromotionSyncTargetKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RedactionSnapshot {
    pub id: String,
    pub schema_version: u32,
    pub promotion_event_id: String,
    pub source_object_type: PromotionSourceObjectType,
    pub source_object_id: String,
    pub policy_id: String,
    pub policy_version: String,
    #[serde(default)]
    pub redacted_fields: Vec<String>,
    #[serde(default)]
    pub removed_span_ids: Vec<String>,
    #[serde(default)]
    pub speaker_alias_map: BTreeMap<String, String>,
    #[serde(default)]
    pub entity_alias_map: BTreeMap<String, String>,
    #[serde(default)]
    pub manual_overrides: Vec<String>,
    pub payload_before_hash: String,
    pub payload_after_hash: String,
    pub approved_payload_hash: String,
    pub reviewed_by_user_id: String,
    pub reviewed_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OrgKnowledgeItem {
    pub id: String,
    pub schema_version: u32,
    pub org_id: String,
    pub workspace_id: String,
    pub kind: OrgKnowledgeKind,
    pub current_revision_id: String,
    pub revision_number: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub content_hash: String,
    pub redacted_payload: ApprovedOrgPayload,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_subject_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_object_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    pub source_promotion_event_id: String,
    pub promotion_event_ids: Vec<String>,
    pub source_local_object_fingerprint: String,
    pub source_session_fingerprint: String,
    pub provenance_summary: String,
    pub full_provenance_pointer: String,
    pub acl: PromotionAcl,
    pub retention: PromotionRetention,
    pub created_by_user_id: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub valid_from_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delete_reason: Option<String>,
    pub state: OrgKnowledgeState,
    pub conflict_state: PromotionConflictState,
    pub sync_state: PromotionSyncSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_revision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PromotionSyncState {
    pub promotion_event_id: String,
    pub target_kind: PromotionSyncTargetKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_revision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_etag: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_attempt_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_success_at_ms: Option<u64>,
    pub retry_count: u32,
    pub status: PromotionSyncStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error_message_redacted: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionSchemaError {
    UnsupportedSchemaVersion {
        record: &'static str,
        version: u32,
    },
    MissingRequiredField {
        record: &'static str,
        field: &'static str,
    },
    PrivatePayloadField {
        field: String,
    },
    UnsupportedStatusForRecord {
        record: &'static str,
        status: String,
    },
    BlockedSourceSessionState {
        state: PromotionSourceSessionState,
    },
    UnredactedErrorMessage {
        field: &'static str,
    },
}

impl PromotionConflictState {
    pub fn blocks_org_sync(&self) -> bool {
        !matches!(self, Self::None)
    }

    pub fn promotion_status(&self) -> PromotionStatus {
        match self {
            Self::None => PromotionStatus::Validated,
            Self::SourceSuperseded => PromotionStatus::BlockedByStaleSource,
            Self::LocalRedactionChanged => PromotionStatus::BlockedByRedaction,
            Self::RemoteNewer
            | Self::AclConflict
            | Self::RetentionConflict
            | Self::TombstoneConflict
            | Self::ManualResolutionRequired => PromotionStatus::Failed,
        }
    }

    pub fn sync_status(&self) -> PromotionSyncStatus {
        match self {
            Self::None => PromotionSyncStatus::NotSynced,
            Self::LocalRedactionChanged => PromotionSyncStatus::RedactionRequired,
            Self::RemoteNewer
            | Self::SourceSuperseded
            | Self::AclConflict
            | Self::RetentionConflict
            | Self::TombstoneConflict
            | Self::ManualResolutionRequired => PromotionSyncStatus::Conflict,
        }
    }
}

pub fn review_promotion_source_basis(
    source: &PromotionSourceReference,
    ledger: &TranscriptLedger,
    source_superseded: bool,
) -> PromotionSourceReview {
    if source_superseded {
        return PromotionSourceReview {
            status: PromotionStatus::BlockedByStaleSource,
            conflict_state: PromotionConflictState::SourceSuperseded,
            basis_staleness: None,
            reason_code: "source_superseded",
        };
    }

    if source.source_session_id != ledger.session_id {
        return PromotionSourceReview {
            status: PromotionStatus::BlockedByStaleSource,
            conflict_state: PromotionConflictState::None,
            basis_staleness: None,
            reason_code: "source_session_mismatch",
        };
    }

    match ledger.validate_basis(&source.source_basis) {
        Ok(()) => PromotionSourceReview {
            status: PromotionStatus::Validated,
            conflict_state: PromotionConflictState::None,
            basis_staleness: None,
            reason_code: "current",
        },
        Err(error) => PromotionSourceReview {
            status: PromotionStatus::BlockedByStaleSource,
            conflict_state: PromotionConflictState::None,
            basis_staleness: Some(error),
            reason_code: "stale_projection_basis",
        },
    }
}

pub fn validate_source_session_state_for_promotion(
    state: PromotionSourceSessionState,
) -> Result<(), PromotionSchemaError> {
    match state {
        PromotionSourceSessionState::Active
        | PromotionSourceSessionState::ExplicitlyRestoredForReview => Ok(()),
        PromotionSourceSessionState::SoftDeleted
        | PromotionSourceSessionState::RetentionExpired => {
            Err(PromotionSchemaError::BlockedSourceSessionState { state })
        }
    }
}

pub fn classify_source_session_state_for_promotion(
    deleted: bool,
    deleted_at_ms: Option<u64>,
    now_ms: u64,
    trash_retention_ms: u64,
    explicitly_restored_for_review: bool,
) -> PromotionSourceSessionState {
    if explicitly_restored_for_review {
        return PromotionSourceSessionState::ExplicitlyRestoredForReview;
    }
    if !deleted {
        return PromotionSourceSessionState::Active;
    }

    let Some(deleted_at_ms) = deleted_at_ms else {
        return PromotionSourceSessionState::SoftDeleted;
    };
    if now_ms >= deleted_at_ms.saturating_add(trash_retention_ms) {
        PromotionSourceSessionState::RetentionExpired
    } else {
        PromotionSourceSessionState::SoftDeleted
    }
}

impl PromotionEvent {
    pub fn validate(&self) -> Result<(), PromotionSchemaError> {
        require_schema("promotion_event", self.schema_version)?;
        require_non_empty("promotion_event", "id", &self.id)?;
        require_positive_ms("promotion_event", "created_at_ms", self.created_at_ms)?;
        self.actor.validate("promotion_event.actor")?;
        self.target.validate("promotion_event.target")?;
        self.source.validate("promotion_event.source")?;
        self.redaction.validate("promotion_event.redaction")?;
        require_non_empty(
            "promotion_event",
            "reviewer_user_id",
            &self.reviewer_user_id,
        )?;
        require_non_empty(
            "promotion_event",
            "approved_payload_hash",
            &self.approved_payload_hash,
        )?;
        self.payload_snapshot
            .validate("promotion_event.payload_snapshot")?;
        self.acl.validate("promotion_event.acl")?;
        self.retention.validate("promotion_event.retention")?;
        self.sync.validate("promotion_event.sync")?;
        require_positive_ms("promotion_event", "requested_at_ms", self.requested_at_ms)?;
        Ok(())
    }
}

impl PromotionDraft {
    pub fn validate(&self) -> Result<(), PromotionSchemaError> {
        require_schema("promotion_draft", self.schema_version)?;
        require_non_empty("promotion_draft", "id", &self.id)?;
        require_positive_ms("promotion_draft", "created_at_ms", self.created_at_ms)?;
        self.actor.validate("promotion_draft.actor")?;
        self.target.validate("promotion_draft.target")?;
        self.source.validate("promotion_draft.source")?;
        require_non_empty(
            "promotion_draft",
            "candidate_payload_hash",
            &self.candidate_payload_hash,
        )?;
        if self
            .requested_redaction_fields
            .iter()
            .any(|field| field.trim().is_empty())
        {
            return Err(PromotionSchemaError::MissingRequiredField {
                record: "promotion_draft",
                field: "requested_redaction_fields",
            });
        }
        if self
            .reviewer_user_id
            .as_deref()
            .is_some_and(|reviewer| reviewer.trim().is_empty())
        {
            return Err(PromotionSchemaError::MissingRequiredField {
                record: "promotion_draft",
                field: "reviewer_user_id",
            });
        }
        validate_redacted_error(
            "promotion_draft.note_redacted",
            self.note_redacted.as_deref(),
        )?;
        if !matches!(
            self.status,
            PromotionStatus::Draft
                | PromotionStatus::RedactionRequired
                | PromotionStatus::ReadyToPromote
                | PromotionStatus::Rejected
        ) {
            return Err(PromotionSchemaError::UnsupportedStatusForRecord {
                record: "promotion_draft",
                status: format!("{:?}", self.status),
            });
        }
        Ok(())
    }
}

impl PromotionRevocationRequest {
    pub fn validate(&self) -> Result<(), PromotionSchemaError> {
        require_schema("promotion_revocation_request", self.schema_version)?;
        require_non_empty("promotion_revocation_request", "id", &self.id)?;
        require_non_empty(
            "promotion_revocation_request",
            "promotion_event_id",
            &self.promotion_event_id,
        )?;
        require_non_empty(
            "promotion_revocation_request",
            "org_knowledge_item_id",
            &self.org_knowledge_item_id,
        )?;
        require_non_empty(
            "promotion_revocation_request",
            "requested_by_user_id",
            &self.requested_by_user_id,
        )?;
        require_positive_ms(
            "promotion_revocation_request",
            "requested_at_ms",
            self.requested_at_ms,
        )?;
        require_non_empty(
            "promotion_revocation_request",
            "reason_code",
            &self.reason_code,
        )?;
        require_non_empty(
            "promotion_revocation_request",
            "reason_redacted",
            &self.reason_redacted,
        )?;
        validate_redacted_error(
            "promotion_revocation_request.reason_redacted",
            Some(&self.reason_redacted),
        )?;
        Ok(())
    }
}

impl RedactionSnapshot {
    pub fn validate(&self) -> Result<(), PromotionSchemaError> {
        require_schema("redaction_snapshot", self.schema_version)?;
        require_non_empty("redaction_snapshot", "id", &self.id)?;
        require_non_empty(
            "redaction_snapshot",
            "promotion_event_id",
            &self.promotion_event_id,
        )?;
        require_non_empty(
            "redaction_snapshot",
            "source_object_id",
            &self.source_object_id,
        )?;
        require_non_empty("redaction_snapshot", "policy_id", &self.policy_id)?;
        require_non_empty("redaction_snapshot", "policy_version", &self.policy_version)?;
        require_non_empty(
            "redaction_snapshot",
            "payload_before_hash",
            &self.payload_before_hash,
        )?;
        require_non_empty(
            "redaction_snapshot",
            "payload_after_hash",
            &self.payload_after_hash,
        )?;
        require_non_empty(
            "redaction_snapshot",
            "approved_payload_hash",
            &self.approved_payload_hash,
        )?;
        require_non_empty(
            "redaction_snapshot",
            "reviewed_by_user_id",
            &self.reviewed_by_user_id,
        )?;
        require_positive_ms("redaction_snapshot", "reviewed_at_ms", self.reviewed_at_ms)?;
        Ok(())
    }
}

impl OrgKnowledgeItem {
    pub fn validate(&self) -> Result<(), PromotionSchemaError> {
        require_schema("org_knowledge_item", self.schema_version)?;
        require_non_empty("org_knowledge_item", "id", &self.id)?;
        require_non_empty("org_knowledge_item", "org_id", &self.org_id)?;
        require_non_empty("org_knowledge_item", "workspace_id", &self.workspace_id)?;
        require_non_empty(
            "org_knowledge_item",
            "current_revision_id",
            &self.current_revision_id,
        )?;
        require_non_empty("org_knowledge_item", "content_hash", &self.content_hash)?;
        self.redacted_payload
            .validate("org_knowledge_item.redacted_payload")?;
        require_non_empty(
            "org_knowledge_item",
            "source_promotion_event_id",
            &self.source_promotion_event_id,
        )?;
        require_non_empty(
            "org_knowledge_item",
            "source_local_object_fingerprint",
            &self.source_local_object_fingerprint,
        )?;
        require_non_empty(
            "org_knowledge_item",
            "source_session_fingerprint",
            &self.source_session_fingerprint,
        )?;
        require_non_empty(
            "org_knowledge_item",
            "provenance_summary",
            &self.provenance_summary,
        )?;
        require_non_empty(
            "org_knowledge_item",
            "full_provenance_pointer",
            &self.full_provenance_pointer,
        )?;
        self.acl.validate("org_knowledge_item.acl")?;
        self.retention.validate("org_knowledge_item.retention")?;
        require_non_empty(
            "org_knowledge_item",
            "created_by_user_id",
            &self.created_by_user_id,
        )?;
        require_positive_ms("org_knowledge_item", "created_at_ms", self.created_at_ms)?;
        require_positive_ms("org_knowledge_item", "updated_at_ms", self.updated_at_ms)?;
        require_positive_ms("org_knowledge_item", "valid_from_ms", self.valid_from_ms)?;
        self.sync_state.validate("org_knowledge_item.sync_state")?;
        Ok(())
    }
}

impl PromotionSyncState {
    pub fn validate(&self) -> Result<(), PromotionSchemaError> {
        require_non_empty(
            "promotion_sync_state",
            "promotion_event_id",
            &self.promotion_event_id,
        )?;
        validate_redacted_error(
            "promotion_sync_state.last_error_message_redacted",
            self.last_error_message_redacted.as_deref(),
        )?;
        Ok(())
    }
}

impl PromotionActor {
    fn validate(&self, record: &'static str) -> Result<(), PromotionSchemaError> {
        require_non_empty(record, "actor_user_id", &self.actor_user_id)?;
        require_non_empty(record, "actor_device_id", &self.actor_device_id)?;
        Ok(())
    }
}

impl PromotionTarget {
    fn validate(&self, record: &'static str) -> Result<(), PromotionSchemaError> {
        require_non_empty(record, "target_org_id", &self.target_org_id)?;
        require_non_empty(record, "target_workspace_id", &self.target_workspace_id)?;
        Ok(())
    }
}

impl PromotionSourceReference {
    fn validate(&self, record: &'static str) -> Result<(), PromotionSchemaError> {
        require_non_empty(record, "source_object_id", &self.source_object_id)?;
        require_non_empty(record, "source_object_version", &self.source_object_version)?;
        require_non_empty(record, "source_session_id", &self.source_session_id)?;
        require_non_empty(record, "source_basis_hash", &self.source_basis_hash)?;
        require_non_empty(record, "source_hash", &self.source_hash)?;
        require_non_empty(
            record,
            "source_basis.transcript_hash",
            &self.source_basis.transcript_hash,
        )?;
        self.source_provenance
            .validate("promotion_event.source.source_provenance")?;
        Ok(())
    }
}

impl PromotionSourceProvenance {
    fn validate(&self, record: &'static str) -> Result<(), PromotionSchemaError> {
        require_positive_ms(record, "created_at_ms", self.created_at_ms)?;
        require_positive_ms(record, "updated_at_ms", self.updated_at_ms)?;
        Ok(())
    }
}

impl PromotionRedactionSummary {
    fn validate(&self, record: &'static str) -> Result<(), PromotionSchemaError> {
        require_non_empty(record, "redaction_policy_id", &self.redaction_policy_id)?;
        require_non_empty(
            record,
            "redaction_policy_version",
            &self.redaction_policy_version,
        )?;
        require_non_empty(
            record,
            "redaction_snapshot_hash",
            &self.redaction_snapshot_hash,
        )?;
        Ok(())
    }
}

impl ApprovedOrgPayload {
    fn validate(&self, record: &'static str) -> Result<(), PromotionSchemaError> {
        require_non_empty(record, "approved_payload_hash", &self.approved_payload_hash)?;
        if self.title.as_deref().unwrap_or("").trim().is_empty()
            && self.body.as_deref().unwrap_or("").trim().is_empty()
            && self.fields.is_empty()
        {
            return Err(PromotionSchemaError::MissingRequiredField {
                record,
                field: "title_body_or_fields",
            });
        }
        for key in self.fields.keys() {
            if is_private_payload_key(key) {
                return Err(PromotionSchemaError::PrivatePayloadField { field: key.clone() });
            }
        }
        Ok(())
    }
}

impl PromotionAcl {
    fn validate(&self, record: &'static str) -> Result<(), PromotionSchemaError> {
        require_non_empty(record, "acl_policy_id", &self.acl_policy_id)?;
        Ok(())
    }
}

impl PromotionRetention {
    fn validate(&self, record: &'static str) -> Result<(), PromotionSchemaError> {
        require_non_empty(record, "retention_policy_id", &self.retention_policy_id)?;
        require_non_empty(record, "retention_legal_basis", &self.retention_legal_basis)?;
        Ok(())
    }
}

impl PromotionSyncSnapshot {
    fn validate(&self, record: &'static str) -> Result<(), PromotionSchemaError> {
        validate_redacted_error(
            "promotion_sync_snapshot.sync_error_message_redacted",
            self.sync_error_message_redacted.as_deref(),
        )?;
        if matches!(self.target_kind, PromotionSyncTargetKind::Disabled) {
            return Ok(());
        }
        require_non_empty(
            record,
            "sync_target_id",
            self.sync_target_id.as_deref().unwrap_or(""),
        )?;
        Ok(())
    }
}

fn require_schema(record: &'static str, version: u32) -> Result<(), PromotionSchemaError> {
    if version == PROMOTION_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(PromotionSchemaError::UnsupportedSchemaVersion { record, version })
    }
}

fn require_non_empty(
    record: &'static str,
    field: &'static str,
    value: &str,
) -> Result<(), PromotionSchemaError> {
    if value.trim().is_empty() {
        Err(PromotionSchemaError::MissingRequiredField { record, field })
    } else {
        Ok(())
    }
}

fn require_positive_ms(
    record: &'static str,
    field: &'static str,
    value: u64,
) -> Result<(), PromotionSchemaError> {
    if value == 0 {
        Err(PromotionSchemaError::MissingRequiredField { record, field })
    } else {
        Ok(())
    }
}

fn validate_redacted_error(
    field: &'static str,
    value: Option<&str>,
) -> Result<(), PromotionSchemaError> {
    let Some(value) = value else {
        return Ok(());
    };
    let lower = value.to_ascii_lowercase();
    if lower.contains("api_key")
        || lower.contains("authorization:")
        || lower.contains("bearer ")
        || lower.contains("sk-")
    {
        Err(PromotionSchemaError::UnredactedErrorMessage { field })
    } else {
        Ok(())
    }
}

fn is_private_payload_key(key: &str) -> bool {
    matches!(
        key.to_ascii_lowercase().as_str(),
        "raw_transcript"
            | "raw_transcript_text"
            | "raw_text"
            | "speaker_name"
            | "speaker_names"
            | "source_id"
            | "source_ids"
            | "provider_id"
            | "provider_ids"
            | "api_key"
            | "secret"
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use serde::Deserialize;
    use serde_json::json;

    use super::*;

    #[derive(Debug, Deserialize)]
    struct RedactionFixtureSet {
        schema_version: u32,
        cases: Vec<RedactionFixtureCase>,
    }

    #[derive(Debug, Deserialize)]
    struct RedactionFixtureCase {
        id: String,
        source_object_type: PromotionSourceObjectType,
        redaction_snapshot: RedactionSnapshot,
        org_knowledge_item: OrgKnowledgeItem,
        forbidden_org_visible_values: Vec<String>,
        forbidden_payload_fields: Vec<String>,
    }

    #[test]
    fn promotion_event_rejects_missing_required_actor() {
        let mut value = serde_json::to_value(sample_promotion_event()).unwrap();
        value.as_object_mut().unwrap().remove("actor");

        let error = serde_json::from_value::<PromotionEvent>(value).unwrap_err();
        assert!(
            error.to_string().contains("missing field `actor`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn promotion_event_validation_requires_target_and_redaction_audit_fields() {
        let mut event = sample_promotion_event();
        event.target.target_org_id.clear();
        assert_eq!(
            event.validate(),
            Err(PromotionSchemaError::MissingRequiredField {
                record: "promotion_event.target",
                field: "target_org_id",
            })
        );

        let mut event = sample_promotion_event();
        event.redaction.redaction_snapshot_hash.clear();
        assert_eq!(
            event.validate(),
            Err(PromotionSchemaError::MissingRequiredField {
                record: "promotion_event.redaction",
                field: "redaction_snapshot_hash",
            })
        );
    }

    #[test]
    fn promotion_event_rejects_unknown_raw_payload_field() {
        let mut value = serde_json::to_value(sample_promotion_event()).unwrap();
        value["payload_snapshot"]["raw_transcript_text"] = json!("private transcript");

        let error = serde_json::from_value::<PromotionEvent>(value).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("unknown field `raw_transcript_text`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn promotion_event_validation_rejects_private_payload_keys() {
        let mut event = sample_promotion_event();
        event.payload_snapshot.fields.insert(
            "speaker_names".to_string(),
            json!(["Alice", "private speaker"]),
        );

        assert_eq!(
            event.validate(),
            Err(PromotionSchemaError::PrivatePayloadField {
                field: "speaker_names".to_string(),
            })
        );
    }

    #[test]
    fn redaction_snapshot_requires_hashes_not_raw_payloads() {
        let snapshot = sample_redaction_snapshot();
        snapshot.validate().unwrap();

        let mut value = serde_json::to_value(snapshot).unwrap();
        value["payload_before"] = json!("private unredacted text");
        let error = serde_json::from_value::<RedactionSnapshot>(value).unwrap_err();
        assert!(
            error.to_string().contains("unknown field `payload_before`"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn org_knowledge_item_exposes_only_redacted_payload_and_state() {
        let item = sample_org_knowledge_item();
        item.validate().unwrap();

        let value = serde_json::to_value(item).unwrap();
        assert!(value.get("redacted_payload").is_some());
        assert!(value.get("state").is_some());
        assert!(value.get("raw_transcript_text").is_none());
        assert!(value.get("speaker_names").is_none());
        assert!(value.get("provider_ids").is_none());
    }

    #[test]
    fn sync_error_messages_must_be_redacted() {
        let mut state = sample_sync_state();
        state.last_error_message_redacted = Some("Authorization: Bearer sk-private".to_string());

        assert_eq!(
            state.validate(),
            Err(PromotionSchemaError::UnredactedErrorMessage {
                field: "promotion_sync_state.last_error_message_redacted",
            })
        );
    }

    #[test]
    fn promotion_draft_validation_allows_unapproved_input_record() {
        let draft = sample_promotion_draft();

        draft.validate().unwrap();
        let value = serde_json::to_value(draft).unwrap();
        assert!(value.get("payload_snapshot").is_none());
        assert!(value.get("approved_payload_hash").is_none());
    }

    #[test]
    fn promotion_draft_rejects_approved_or_synced_status() {
        let mut draft = sample_promotion_draft();
        draft.status = PromotionStatus::ApprovedLocal;

        assert_eq!(
            draft.validate(),
            Err(PromotionSchemaError::UnsupportedStatusForRecord {
                record: "promotion_draft",
                status: "ApprovedLocal".to_string(),
            })
        );
    }

    #[test]
    fn promotion_revocation_request_reason_must_be_redacted() {
        let mut request = sample_revocation_request();
        request.reason_redacted = "Authorization: Bearer sk-private".to_string();

        assert_eq!(
            request.validate(),
            Err(PromotionSchemaError::UnredactedErrorMessage {
                field: "promotion_revocation_request.reason_redacted",
            })
        );
    }

    #[test]
    fn promotion_source_review_accepts_current_projection_basis() {
        let event = sample_transcript_event("span-1", 2, 1_700_000_000_000);
        let ledger = TranscriptLedger::replay("session-1", vec![event]).unwrap();
        let mut source = sample_source_reference();
        source.source_session_id = ledger.session_id.clone();
        source.source_basis = ledger.current_basis();

        assert_eq!(
            review_promotion_source_basis(&source, &ledger, false),
            PromotionSourceReview {
                status: PromotionStatus::Validated,
                conflict_state: PromotionConflictState::None,
                basis_staleness: None,
                reason_code: "current",
            }
        );
    }

    #[test]
    fn stale_projection_basis_blocks_promotion_without_org_sync() {
        let stale_event = sample_transcript_event("span-1", 2, 1_700_000_000_000);
        let current_event = sample_transcript_event("span-1", 3, 1_700_000_000_500);
        let stale_ledger = TranscriptLedger::replay("session-1", vec![stale_event]).unwrap();
        let current_ledger = TranscriptLedger::replay("session-1", vec![current_event]).unwrap();
        let mut source = sample_source_reference();
        source.source_session_id = current_ledger.session_id.clone();
        source.source_basis = stale_ledger.current_basis();

        let review = review_promotion_source_basis(&source, &current_ledger, false);
        assert_eq!(review.status, PromotionStatus::BlockedByStaleSource);
        assert_eq!(review.conflict_state, PromotionConflictState::None);
        assert_eq!(review.reason_code, "stale_projection_basis");
        assert!(matches!(
            review.basis_staleness,
            Some(ProjectionBasisStaleness::StaleSpanRevision {
                span_id,
                current_revision: 3,
                basis_revision: 2,
            }) if span_id == "span-1"
        ));
    }

    #[test]
    fn promotion_source_review_reports_missing_current_span() {
        let first = sample_transcript_event("span-1", 1, 1_700_000_000_000);
        let second = sample_transcript_event("span-2", 1, 1_700_000_000_500);
        let ledger = TranscriptLedger::replay("session-1", vec![first.clone(), second]).unwrap();
        let mut source = sample_source_reference();
        source.source_session_id = ledger.session_id.clone();
        source.source_basis = ProjectionBasis::from_transcript_events(&[first]);

        let review = review_promotion_source_basis(&source, &ledger, false);
        assert_eq!(review.status, PromotionStatus::BlockedByStaleSource);
        assert_eq!(review.reason_code, "stale_projection_basis");
        assert!(matches!(
            review.basis_staleness,
            Some(ProjectionBasisStaleness::MissingCurrentSpan {
                span_id,
                current_revision: 1,
            }) if span_id == "span-2"
        ));
    }

    #[test]
    fn promotion_source_review_reports_unknown_basis_span() {
        let basis_event = sample_transcript_event("span-unknown", 1, 1_700_000_000_000);
        let ledger = TranscriptLedger::replay("session-1", Vec::new()).unwrap();
        let mut source = sample_source_reference();
        source.source_session_id = ledger.session_id.clone();
        source.source_basis = ProjectionBasis::from_transcript_events(&[basis_event]);

        let review = review_promotion_source_basis(&source, &ledger, false);
        assert_eq!(review.status, PromotionStatus::BlockedByStaleSource);
        assert_eq!(review.reason_code, "stale_projection_basis");
        assert!(matches!(
            review.basis_staleness,
            Some(ProjectionBasisStaleness::UnknownBasisSpan {
                span_id,
                basis_revision: 1,
            }) if span_id == "span-unknown"
        ));
    }

    #[test]
    fn promotion_source_review_reports_transcript_hash_mismatch() {
        let event = sample_transcript_event("span-1", 1, 1_700_000_000_000);
        let ledger = TranscriptLedger::replay("session-1", vec![event]).unwrap();
        let mut stale_basis = ledger.current_basis();
        stale_basis.transcript_hash = "sha256:not-current".to_string();
        let mut source = sample_source_reference();
        source.source_session_id = ledger.session_id.clone();
        source.source_basis = stale_basis;

        let review = review_promotion_source_basis(&source, &ledger, false);
        assert_eq!(review.status, PromotionStatus::BlockedByStaleSource);
        assert_eq!(review.reason_code, "stale_projection_basis");
        assert!(matches!(
            review.basis_staleness,
            Some(ProjectionBasisStaleness::TranscriptHashMismatch {
                basis_hash,
                ..
            }) if basis_hash == "sha256:not-current"
        ));
    }

    #[test]
    fn source_superseded_maps_to_blocked_status_and_conflict_state() {
        let event = sample_transcript_event("span-1", 2, 1_700_000_000_000);
        let ledger = TranscriptLedger::replay("session-1", vec![event]).unwrap();
        let mut source = sample_source_reference();
        source.source_session_id = ledger.session_id.clone();
        source.source_basis = ledger.current_basis();

        let review = review_promotion_source_basis(&source, &ledger, true);
        assert_eq!(review.status, PromotionStatus::BlockedByStaleSource);
        assert_eq!(
            review.conflict_state,
            PromotionConflictState::SourceSuperseded
        );
        assert_eq!(review.reason_code, "source_superseded");
        assert!(review.basis_staleness.is_none());
    }

    #[test]
    fn promotion_conflict_states_have_blocking_status_and_sync_mapping() {
        assert!(!PromotionConflictState::None.blocks_org_sync());
        assert_eq!(
            PromotionConflictState::None.promotion_status(),
            PromotionStatus::Validated
        );
        assert_eq!(
            PromotionConflictState::None.sync_status(),
            PromotionSyncStatus::NotSynced
        );

        let cases = [
            (
                PromotionConflictState::RemoteNewer,
                PromotionStatus::Failed,
                PromotionSyncStatus::Conflict,
            ),
            (
                PromotionConflictState::LocalRedactionChanged,
                PromotionStatus::BlockedByRedaction,
                PromotionSyncStatus::RedactionRequired,
            ),
            (
                PromotionConflictState::SourceSuperseded,
                PromotionStatus::BlockedByStaleSource,
                PromotionSyncStatus::Conflict,
            ),
            (
                PromotionConflictState::AclConflict,
                PromotionStatus::Failed,
                PromotionSyncStatus::Conflict,
            ),
            (
                PromotionConflictState::RetentionConflict,
                PromotionStatus::Failed,
                PromotionSyncStatus::Conflict,
            ),
            (
                PromotionConflictState::TombstoneConflict,
                PromotionStatus::Failed,
                PromotionSyncStatus::Conflict,
            ),
            (
                PromotionConflictState::ManualResolutionRequired,
                PromotionStatus::Failed,
                PromotionSyncStatus::Conflict,
            ),
        ];

        for (conflict, promotion_status, sync_status) in cases {
            assert!(conflict.blocks_org_sync(), "{conflict:?} must block sync");
            assert_eq!(conflict.promotion_status(), promotion_status);
            assert_eq!(conflict.sync_status(), sync_status);
        }
    }

    #[test]
    fn soft_deleted_or_retention_expired_source_sessions_block_new_promotions() {
        assert_eq!(
            validate_source_session_state_for_promotion(PromotionSourceSessionState::Active),
            Ok(())
        );
        assert_eq!(
            validate_source_session_state_for_promotion(
                PromotionSourceSessionState::ExplicitlyRestoredForReview,
            ),
            Ok(())
        );
        assert_eq!(
            validate_source_session_state_for_promotion(PromotionSourceSessionState::SoftDeleted),
            Err(PromotionSchemaError::BlockedSourceSessionState {
                state: PromotionSourceSessionState::SoftDeleted,
            })
        );
        assert_eq!(
            validate_source_session_state_for_promotion(
                PromotionSourceSessionState::RetentionExpired,
            ),
            Err(PromotionSchemaError::BlockedSourceSessionState {
                state: PromotionSourceSessionState::RetentionExpired,
            })
        );
    }

    #[test]
    fn source_session_state_classifier_marks_recent_trash_soft_deleted() {
        assert_eq!(
            classify_source_session_state_for_promotion(
                true,
                Some(1_700_000_000_000),
                1_700_000_500_000,
                1_000_000,
                false,
            ),
            PromotionSourceSessionState::SoftDeleted
        );
    }

    #[test]
    fn source_session_state_classifier_marks_old_trash_retention_expired() {
        assert_eq!(
            classify_source_session_state_for_promotion(
                true,
                Some(1_700_000_000_000),
                1_700_002_000_000,
                1_000_000,
                false,
            ),
            PromotionSourceSessionState::RetentionExpired
        );
    }

    #[test]
    fn source_session_state_classifier_allows_explicit_review_restore() {
        assert_eq!(
            classify_source_session_state_for_promotion(
                true,
                Some(1_700_000_000_000),
                1_700_002_000_000,
                1_000_000,
                true,
            ),
            PromotionSourceSessionState::ExplicitlyRestoredForReview
        );
    }

    #[test]
    fn redaction_policy_fixtures_omit_private_data_from_org_visible_items() {
        let fixture_set = load_redaction_fixture_set();
        assert_eq!(fixture_set.schema_version, PROMOTION_SCHEMA_VERSION);
        assert_eq!(fixture_set.cases.len(), 5);

        let mut covered_types = Vec::new();
        for case in fixture_set.cases {
            assert!(!case.id.trim().is_empty());
            assert_eq!(
                &case.redaction_snapshot.source_object_type, &case.source_object_type,
                "{} source type mismatch",
                case.id
            );
            covered_types.push(case.source_object_type.clone());

            case.redaction_snapshot.validate().unwrap_or_else(|error| {
                panic!("{} redaction snapshot invalid: {error:?}", case.id)
            });
            case.org_knowledge_item
                .validate()
                .unwrap_or_else(|error| panic!("{} org item invalid: {error:?}", case.id));

            assert_eq!(
                case.redaction_snapshot.approved_payload_hash,
                case.org_knowledge_item
                    .redacted_payload
                    .approved_payload_hash,
                "{} approved payload hash must link snapshot to org item",
                case.id
            );
            assert!(
                !case.redaction_snapshot.redacted_fields.is_empty(),
                "{} must list redacted fields",
                case.id
            );
            assert!(
                !case.redaction_snapshot.manual_overrides.is_empty(),
                "{} must record manual overrides",
                case.id
            );
            assert!(
                !case.redaction_snapshot.speaker_alias_map.is_empty()
                    || !case.redaction_snapshot.entity_alias_map.is_empty(),
                "{} must record speaker or entity aliasing",
                case.id
            );

            let org_visible =
                serde_json::to_string(&case.org_knowledge_item).unwrap_or_else(|error| {
                    panic!("{} org item serialization failed: {error}", case.id)
                });
            for forbidden in &case.forbidden_org_visible_values {
                assert!(
                    !org_visible.contains(forbidden),
                    "{} leaked forbidden org-visible value {forbidden:?}",
                    case.id
                );
            }
            for forbidden_field in &case.forbidden_payload_fields {
                assert!(
                    !case
                        .org_knowledge_item
                        .redacted_payload
                        .fields
                        .contains_key(forbidden_field),
                    "{} leaked forbidden payload field {forbidden_field}",
                    case.id
                );
            }
        }

        assert!(covered_types.contains(&PromotionSourceObjectType::MaterializedNote));
        assert!(covered_types.contains(&PromotionSourceObjectType::GraphNodeFact));
        assert!(covered_types.contains(&PromotionSourceObjectType::GraphEdgeFact));
        assert!(covered_types.contains(&PromotionSourceObjectType::TranscriptSpan));
        assert!(covered_types.contains(&PromotionSourceObjectType::LiveAssistCard));
    }

    fn sample_transcript_event(
        span_id: &str,
        revision_number: u64,
        received_at_ms: u64,
    ) -> crate::projections::TranscriptEvent {
        crate::projections::TranscriptEvent {
            span_id: span_id.to_string(),
            provider: "soniox".to_string(),
            source_id: "default-mic".to_string(),
            provider_item_id: None,
            transcript_segment_id: Some(format!("segment-{span_id}")),
            speaker_id: Some("speaker-local-1".to_string()),
            speaker_label: Some("Speaker 1".to_string()),
            channel: None,
            text: format!("Revision {revision_number} for {span_id}"),
            start_time: 1.0,
            end_time: 2.0,
            confidence: 0.94,
            is_final: true,
            stability: crate::projections::TranscriptEventStability::Final,
            revision_number,
            supersedes: None,
            turn_id: Some("turn-1".to_string()),
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: Some(10),
            asr_latency_ms: Some(80),
            received_at_ms,
        }
    }

    fn sample_promotion_draft() -> PromotionDraft {
        PromotionDraft {
            id: "promotion-draft-1".to_string(),
            schema_version: PROMOTION_SCHEMA_VERSION,
            created_at_ms: 1_700_000_000_000,
            actor: PromotionActor {
                actor_user_id: "user-1".to_string(),
                actor_local_profile_id: Some("profile-1".to_string()),
                actor_device_id: "device-1".to_string(),
                delegated_service_id: None,
            },
            target: PromotionTarget {
                source_workspace_id: Some("local-workspace".to_string()),
                target_org_id: "org-1".to_string(),
                target_workspace_id: "workspace-1".to_string(),
                target_collection_id: Some("collection-1".to_string()),
            },
            source: sample_source_reference(),
            candidate_payload_hash: "sha256:candidate".to_string(),
            requested_redaction_fields: vec!["speaker_name".to_string()],
            reviewer_user_id: Some("reviewer-1".to_string()),
            note_redacted: Some("Ready for reviewer redaction check".to_string()),
            status: PromotionStatus::Draft,
        }
    }

    fn sample_revocation_request() -> PromotionRevocationRequest {
        PromotionRevocationRequest {
            id: "revocation-request-1".to_string(),
            schema_version: PROMOTION_SCHEMA_VERSION,
            promotion_event_id: "promotion-1".to_string(),
            org_knowledge_item_id: "org-item-1".to_string(),
            requested_by_user_id: "reviewer-1".to_string(),
            requested_at_ms: 1_700_000_000_200,
            reason_code: "source_retracted".to_string(),
            reason_redacted: "Reviewer requested retraction after source review".to_string(),
            target_kind: PromotionSyncTargetKind::Disabled,
        }
    }

    fn sample_promotion_event() -> PromotionEvent {
        PromotionEvent {
            id: "promotion-1".to_string(),
            schema_version: PROMOTION_SCHEMA_VERSION,
            created_at_ms: 1_700_000_000_000,
            actor: PromotionActor {
                actor_user_id: "user-1".to_string(),
                actor_local_profile_id: Some("profile-1".to_string()),
                actor_device_id: "device-1".to_string(),
                delegated_service_id: None,
            },
            target: PromotionTarget {
                source_workspace_id: Some("local-workspace".to_string()),
                target_org_id: "org-1".to_string(),
                target_workspace_id: "workspace-1".to_string(),
                target_collection_id: Some("collection-1".to_string()),
            },
            source: sample_source_reference(),
            redaction: PromotionRedactionSummary {
                redaction_policy_id: "policy-1".to_string(),
                redaction_policy_version: "2026-06-26".to_string(),
                redaction_snapshot_hash: "sha256:redaction".to_string(),
                redaction_diff: vec![RedactionDiffEntry {
                    field: "body".to_string(),
                    reason: "speaker_name".to_string(),
                    before_hash: "sha256:before".to_string(),
                    after_hash: "sha256:after".to_string(),
                }],
                redacted_fields: vec!["speaker_name".to_string()],
                manual_redaction_overrides: vec!["alias-speaker-a".to_string()],
            },
            reviewer_user_id: "reviewer-1".to_string(),
            approved_payload_hash: "sha256:approved".to_string(),
            payload_snapshot: sample_payload(),
            acl: sample_acl(),
            retention: sample_retention(),
            sync: PromotionSyncSnapshot {
                target_kind: PromotionSyncTargetKind::Disabled,
                sync_target_id: None,
                status: PromotionSyncStatus::NotConfigured,
                remote_id: None,
                remote_revision: None,
                remote_etag: None,
                sync_error_code: None,
                sync_error_message_redacted: None,
            },
            lineage: PromotionLineage {
                parent_promotion_id: None,
                supersedes_promotion_id: None,
                conflict_group_id: Some("conflict-group-1".to_string()),
            },
            conflict_state: PromotionConflictState::None,
            requested_at_ms: 1_700_000_000_000,
            approved_at_ms: Some(1_700_000_000_100),
            status: PromotionStatus::ApprovedLocal,
        }
    }

    fn sample_source_reference() -> PromotionSourceReference {
        PromotionSourceReference {
            source_object_type: PromotionSourceObjectType::MaterializedNote,
            source_object_id: "note-1".to_string(),
            source_object_version: "sequence:7".to_string(),
            source_session_id: "session-1".to_string(),
            source_span_ids: vec!["span-1".to_string()],
            source_projection_sequence: Some(7),
            source_basis_hash: "sha256:basis".to_string(),
            source_hash: "sha256:source".to_string(),
            source_basis: ProjectionBasis {
                span_revisions: vec![ProjectionBasisSpan {
                    span_id: "span-1".to_string(),
                    revision_number: 2,
                }],
                diarization_span_revisions: Vec::new(),
                transcript_hash: "sha256:transcript".to_string(),
            },
            source_provenance: PromotionSourceProvenance {
                asr_provider: Some("soniox".to_string()),
                source_id: Some("default-mic".to_string()),
                speaker_ids: vec!["speaker-local-1".to_string()],
                span_revisions: vec![ProjectionBasisSpan {
                    span_id: "span-1".to_string(),
                    revision_number: 2,
                }],
                llm: Some(ProjectionProvenance {
                    provider: "openrouter".to_string(),
                    model: "test-model".to_string(),
                    prompt_id: "projection-v1".to_string(),
                }),
                confidence: Some(0.91),
                created_at_ms: 1_700_000_000_000,
                updated_at_ms: 1_700_000_000_100,
            },
        }
    }

    fn sample_payload() -> ApprovedOrgPayload {
        ApprovedOrgPayload {
            kind: OrgKnowledgeKind::Note,
            title: Some("Approved summary".to_string()),
            body: Some("Redacted approved body.".to_string()),
            fields: BTreeMap::from([("topic".to_string(), json!("roadmap"))]),
            approved_payload_hash: "sha256:approved".to_string(),
        }
    }

    fn sample_acl() -> PromotionAcl {
        PromotionAcl {
            acl_policy_id: "acl-1".to_string(),
            acl_visibility: AclVisibility::Workspace,
            acl_principals: vec!["workspace:workspace-1".to_string()],
            acl_inheritance_mode: AclInheritanceMode::NarrowerOfSourceAndTarget,
        }
    }

    fn sample_retention() -> PromotionRetention {
        PromotionRetention {
            retention_policy_id: "retention-1".to_string(),
            retention_legal_basis: "user_approved_org_memory".to_string(),
            retention_category: RetentionCategory::OrgKnowledge,
            expires_at_ms: None,
            delete_behavior: DeleteBehavior::RetractRemote,
        }
    }

    fn sample_redaction_snapshot() -> RedactionSnapshot {
        RedactionSnapshot {
            id: "redaction-1".to_string(),
            schema_version: PROMOTION_SCHEMA_VERSION,
            promotion_event_id: "promotion-1".to_string(),
            source_object_type: PromotionSourceObjectType::MaterializedNote,
            source_object_id: "note-1".to_string(),
            policy_id: "policy-1".to_string(),
            policy_version: "2026-06-26".to_string(),
            redacted_fields: vec!["speaker_name".to_string()],
            removed_span_ids: vec!["span-private".to_string()],
            speaker_alias_map: BTreeMap::from([(
                "speaker-local-1".to_string(),
                "Speaker A".to_string(),
            )]),
            entity_alias_map: BTreeMap::new(),
            manual_overrides: vec!["remove private name".to_string()],
            payload_before_hash: "sha256:before".to_string(),
            payload_after_hash: "sha256:after".to_string(),
            approved_payload_hash: "sha256:approved".to_string(),
            reviewed_by_user_id: "reviewer-1".to_string(),
            reviewed_at_ms: 1_700_000_000_100,
        }
    }

    fn sample_org_knowledge_item() -> OrgKnowledgeItem {
        OrgKnowledgeItem {
            id: "org-item-1".to_string(),
            schema_version: PROMOTION_SCHEMA_VERSION,
            org_id: "org-1".to_string(),
            workspace_id: "workspace-1".to_string(),
            kind: OrgKnowledgeKind::Note,
            current_revision_id: "org-item-1-r1".to_string(),
            revision_number: 1,
            title: Some("Approved summary".to_string()),
            body: Some("Redacted approved body.".to_string()),
            tags: vec!["roadmap".to_string()],
            content_hash: "sha256:content".to_string(),
            redacted_payload: sample_payload(),
            graph_subject_id: None,
            graph_object_id: None,
            relation_type: None,
            confidence: Some(0.91),
            source_promotion_event_id: "promotion-1".to_string(),
            promotion_event_ids: vec!["promotion-1".to_string()],
            source_local_object_fingerprint: "sha256:local-object".to_string(),
            source_session_fingerprint: "sha256:session".to_string(),
            provenance_summary: "Approved local note from session-1".to_string(),
            full_provenance_pointer: "promotion://promotion-1".to_string(),
            acl: sample_acl(),
            retention: sample_retention(),
            created_by_user_id: "reviewer-1".to_string(),
            created_at_ms: 1_700_000_000_100,
            updated_at_ms: 1_700_000_000_100,
            valid_from_ms: 1_700_000_000_100,
            valid_until_ms: None,
            deleted_at_ms: None,
            delete_reason: None,
            state: OrgKnowledgeState::Active,
            conflict_state: PromotionConflictState::None,
            sync_state: PromotionSyncSnapshot {
                target_kind: PromotionSyncTargetKind::Disabled,
                sync_target_id: None,
                status: PromotionSyncStatus::NotConfigured,
                remote_id: None,
                remote_revision: None,
                remote_etag: None,
                sync_error_code: None,
                sync_error_message_redacted: None,
            },
            remote_revision: None,
        }
    }

    fn sample_sync_state() -> PromotionSyncState {
        PromotionSyncState {
            promotion_event_id: "promotion-1".to_string(),
            target_kind: PromotionSyncTargetKind::ApiServer,
            remote_id: None,
            remote_revision: None,
            remote_etag: None,
            queued_at_ms: Some(1_700_000_000_100),
            last_attempt_at_ms: None,
            last_success_at_ms: None,
            retry_count: 0,
            status: PromotionSyncStatus::Queued,
            last_error_code: None,
            last_error_message_redacted: None,
        }
    }

    fn load_redaction_fixture_set() -> RedactionFixtureSet {
        let path = fixture_path("promotion/redaction_snapshots.json");
        let body = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        serde_json::from_str(&body)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
    }

    fn fixture_path(relative_path: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(relative_path)
    }
}
