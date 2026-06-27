//! Optional SurrealDB-backed local memory repository.
//!
//! This adapter is intentionally feature-gated and non-default. The first
//! engine is `kv-mem` for conformance tests and schema-shape validation; file
//! backed SurrealKV/RocksDB engines need cross-platform release evidence before
//! this becomes selectable storage.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use surrealdb::Surreal;
use surrealdb::engine::local::{Db, Mem};

use super::{
    LocalMemoryRepository, SessionArtifactDeleteReport, SessionArtifactDescriptor,
    SessionArtifactKind, ensure_org_visible_record_is_safe, now_millis, validate_live_assist_card,
};
use crate::events::LiveAssistCardRecord;
use crate::projections::{MaterializedGraph, MaterializedNotes, ProjectionPatch, TranscriptEvent};
use crate::promotion::{
    OrgKnowledgeItem, OrgKnowledgeState, PromotionConflictState, PromotionDraft, PromotionEvent,
    PromotionRevocationRequest, PromotionSyncState, PromotionSyncStatus, RedactionSnapshot,
};
use crate::sessions::SessionMetadata;

const NS: &str = "audio_graph";
const DB: &str = "local_memory";

const SESSION_METADATA_TABLE: &str = "session_metadata_event";
const TRANSCRIPT_EVENT_TABLE: &str = "transcript_event";
const PROJECTION_PATCH_TABLE: &str = "projection_patch";
const MATERIALIZED_NOTES_TABLE: &str = "materialized_notes";
const MATERIALIZED_GRAPH_TABLE: &str = "materialized_graph";
const LIVE_ASSIST_CARD_TABLE: &str = "live_assist_card";
const PROMOTION_EVENT_TABLE: &str = "promotion_event";
const PROMOTION_DRAFT_TABLE: &str = "promotion_draft";
const PROMOTION_REVOCATION_TABLE: &str = "promotion_revocation_request";
const REDACTION_SNAPSHOT_TABLE: &str = "redaction_snapshot";
const ORG_KNOWLEDGE_ITEM_TABLE: &str = "org_knowledge_item";
const PROMOTION_SYNC_STATE_TABLE: &str = "promotion_sync_state";
const TABLES: &[&str] = &[
    SESSION_METADATA_TABLE,
    TRANSCRIPT_EVENT_TABLE,
    PROJECTION_PATCH_TABLE,
    MATERIALIZED_NOTES_TABLE,
    MATERIALIZED_GRAPH_TABLE,
    LIVE_ASSIST_CARD_TABLE,
    PROMOTION_EVENT_TABLE,
    PROMOTION_DRAFT_TABLE,
    PROMOTION_REVOCATION_TABLE,
    REDACTION_SNAPSHOT_TABLE,
    ORG_KNOWLEDGE_ITEM_TABLE,
    PROMOTION_SYNC_STATE_TABLE,
];

#[derive(Clone, Deserialize, Serialize)]
struct SessionRecord<T> {
    session_id: String,
    sequence: u64,
    value: T,
}

#[derive(Clone, Deserialize, Serialize)]
struct GlobalRecord<T> {
    sequence: u64,
    value: T,
}

/// Feature-gated embedded SurrealDB repository.
///
/// The synchronous [`LocalMemoryRepository`] contract is bridged to SurrealDB's
/// async SDK with a private Tokio runtime. A small mutex serializes append and
/// current-state operations so sequence assignment stays deterministic.
pub struct SurrealMemoryRepository {
    db: Surreal<Db>,
    runtime: tokio::runtime::Runtime,
    write_lock: Mutex<()>,
}

impl fmt::Debug for SurrealMemoryRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SurrealMemoryRepository")
            .field("engine", &"kv-mem")
            .field("namespace", &NS)
            .field("database", &DB)
            .finish_non_exhaustive()
    }
}

impl SurrealMemoryRepository {
    /// Create an embedded in-memory SurrealDB repository for tests/spikes.
    pub fn in_memory() -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("SurrealDB runtime init failed: {error}"))?;
        let db = runtime
            .block_on(async {
                let db = Surreal::new::<Mem>(()).await?;
                db.use_ns(NS).use_db(DB).await?;
                for table in TABLES {
                    db.query(format!("DEFINE TABLE IF NOT EXISTS {table} SCHEMALESS"))
                        .await?;
                }
                Ok::<_, surrealdb::Error>(db)
            })
            .map_err(|error| format!("SurrealDB in-memory init failed: {error}"))?;
        Ok(Self {
            db,
            runtime,
            write_lock: Mutex::new(()),
        })
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ()>, String> {
        self.write_lock
            .lock()
            .map_err(|_| "SurrealDB repository lock poisoned".to_string())
    }

    fn select_all<T>(&self, table: &str) -> Result<Vec<T>, String>
    where
        T: DeserializeOwned,
    {
        let values: Vec<serde_json::Value> = self
            .runtime
            .block_on(async { self.db.select(table).await })
            .map_err(|error| format!("SurrealDB select {table} failed: {error}"))?;
        values
            .into_iter()
            .map(|value| {
                serde_json::from_value(value)
                    .map_err(|error| format!("SurrealDB decode {table} failed: {error}"))
            })
            .collect()
    }

    fn create_record<T>(&self, table: &str, record: T) -> Result<(), String>
    where
        T: Serialize,
    {
        let record = serde_json::to_value(record)
            .map_err(|error| format!("SurrealDB encode {table} failed: {error}"))?;
        let _: Option<serde_json::Value> = self
            .runtime
            .block_on(async { self.db.create(table).content(record).await })
            .map_err(|error| format!("SurrealDB create {table} failed: {error}"))?;
        Ok(())
    }

    fn next_session_sequence<T>(&self, table: &str, session_id: &str) -> Result<u64, String>
    where
        T: DeserializeOwned,
    {
        let records: Vec<SessionRecord<T>> = self.select_all(table)?;
        Ok(records
            .iter()
            .filter(|record| record.session_id == session_id)
            .map(|record| record.sequence)
            .max()
            .unwrap_or(0)
            + 1)
    }

    fn next_global_sequence<T>(&self, table: &str) -> Result<u64, String>
    where
        T: DeserializeOwned,
    {
        let records: Vec<GlobalRecord<T>> = self.select_all(table)?;
        Ok(records
            .iter()
            .map(|record| record.sequence)
            .max()
            .unwrap_or(0)
            + 1)
    }

    fn append_session_value<T>(
        &self,
        table: &str,
        session_id: &str,
        value: &T,
    ) -> Result<(), String>
    where
        T: Clone + DeserializeOwned + Serialize,
    {
        crate::sessions::validate_session_id(session_id)?;
        let _guard = self.lock()?;
        let sequence = self.next_session_sequence::<T>(table, session_id)?;
        self.create_record(
            table,
            SessionRecord {
                session_id: session_id.to_string(),
                sequence,
                value: value.clone(),
            },
        )
    }

    fn append_global_value<T>(&self, table: &str, value: &T) -> Result<(), String>
    where
        T: Clone + DeserializeOwned + Serialize,
    {
        let _guard = self.lock()?;
        let sequence = self.next_global_sequence::<T>(table)?;
        self.create_record(
            table,
            GlobalRecord {
                sequence,
                value: value.clone(),
            },
        )
    }

    fn load_session_values<T>(&self, table: &str, session_id: &str) -> Result<Vec<T>, String>
    where
        T: DeserializeOwned,
    {
        crate::sessions::validate_session_id(session_id)?;
        let mut records: Vec<SessionRecord<T>> = self.select_all(table)?;
        records.retain(|record| record.session_id == session_id);
        records.sort_by_key(|record| record.sequence);
        Ok(records.into_iter().map(|record| record.value).collect())
    }

    fn load_global_values<T>(&self, table: &str) -> Result<Vec<T>, String>
    where
        T: DeserializeOwned,
    {
        let mut records: Vec<GlobalRecord<T>> = self.select_all(table)?;
        records.sort_by_key(|record| record.sequence);
        Ok(records.into_iter().map(|record| record.value).collect())
    }

    fn append_session_metadata(&self, metadata: &SessionMetadata) -> Result<(), String> {
        self.append_global_value(SESSION_METADATA_TABLE, metadata)
    }

    fn latest_sessions(&self) -> Result<Vec<SessionMetadata>, String> {
        let mut latest = BTreeMap::<String, SessionMetadata>::new();
        for metadata in self.load_global_values::<SessionMetadata>(SESSION_METADATA_TABLE)? {
            latest.insert(metadata.id.clone(), metadata);
        }
        let mut sessions = latest.into_values().collect::<Vec<_>>();
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at).then(a.id.cmp(&b.id)));
        if sessions.len() > 100 {
            sessions.truncate(100);
        }
        Ok(sessions)
    }

    fn latest_materialized<T>(&self, table: &str, session_id: &str) -> Result<Option<T>, String>
    where
        T: DeserializeOwned,
    {
        Ok(self
            .load_session_values(table, session_id)?
            .into_iter()
            .last())
    }

    fn delete_session_table_records(&self, table: &str, session_id: &str) -> Result<(), String> {
        self.runtime
            .block_on(async {
                self.db
                    .query(format!("DELETE {table} WHERE session_id = $session_id"))
                    .bind(("session_id", session_id.to_string()))
                    .await
            })
            .map_err(|error| format!("SurrealDB delete {table} failed: {error}"))?;
        Ok(())
    }

    fn delete_session_metadata_records(&self, session_id: &str) -> Result<(), String> {
        self.runtime
            .block_on(async {
                self.db
                    .query(format!(
                        "DELETE {SESSION_METADATA_TABLE} WHERE value.id = $session_id"
                    ))
                    .bind(("session_id", session_id.to_string()))
                    .await
            })
            .map_err(|error| {
                format!("SurrealDB delete {SESSION_METADATA_TABLE} failed: {error}")
            })?;
        Ok(())
    }
}

impl LocalMemoryRepository for SurrealMemoryRepository {
    fn load_session_index(&self) -> Result<Vec<SessionMetadata>, String> {
        self.latest_sessions()
    }

    fn find_session(&self, session_id: &str) -> Result<Option<SessionMetadata>, String> {
        Ok(self
            .latest_sessions()?
            .into_iter()
            .find(|entry| entry.id == session_id))
    }

    fn register_session(&self, session_id: &str) -> Result<(), String> {
        crate::sessions::validate_session_id(session_id)?;
        let mut sessions = self.latest_sessions()?;
        let ended_at = now_millis();
        for entry in sessions
            .iter_mut()
            .filter(|entry| entry.status == "active" && entry.id != session_id)
        {
            entry.status = "crashed".into();
            entry.ended_at.get_or_insert(ended_at);
            self.append_session_metadata(entry)?;
        }
        let metadata = SessionMetadata {
            id: session_id.to_string(),
            title: None,
            created_at: now_millis(),
            ended_at: None,
            duration_seconds: None,
            status: "active".to_string(),
            segment_count: 0,
            speaker_count: 0,
            entity_count: 0,
            transcript_path: format!("surrealdb://{TRANSCRIPT_EVENT_TABLE}/{session_id}"),
            graph_path: format!("surrealdb://{MATERIALIZED_GRAPH_TABLE}/{session_id}"),
            deleted: false,
            deleted_at: None,
        };
        self.append_session_metadata(&metadata)
    }

    fn update_session_stats(
        &self,
        session_id: &str,
        segment_count: u64,
        speaker_count: u64,
        entity_count: u64,
    ) -> Result<(), String> {
        if let Some(mut metadata) = self.find_session(session_id)? {
            metadata.segment_count = segment_count;
            metadata.speaker_count = speaker_count;
            metadata.entity_count = entity_count;
            self.append_session_metadata(&metadata)?;
        }
        Ok(())
    }

    fn finalize_session(&self, session_id: &str) -> Result<(), String> {
        if let Some(mut metadata) = self.find_session(session_id)? {
            metadata.status = "complete".into();
            let end = now_millis();
            metadata.ended_at = Some(end);
            metadata.duration_seconds = Some(end.saturating_sub(metadata.created_at) / 1000);
            self.append_session_metadata(&metadata)?;
        }
        Ok(())
    }

    fn session_artifacts(
        &self,
        session_id: &str,
    ) -> Result<Vec<SessionArtifactDescriptor>, String> {
        crate::sessions::validate_session_id(session_id)?;
        Ok(vec![
            SessionArtifactDescriptor::repository_record(
                SessionArtifactKind::SessionMetadata,
                "Session metadata event records",
                format!("surrealdb://{SESSION_METADATA_TABLE}/{session_id}"),
            ),
            SessionArtifactDescriptor::repository_record(
                SessionArtifactKind::TranscriptEvents,
                "Transcript revision event records",
                format!("surrealdb://{TRANSCRIPT_EVENT_TABLE}/{session_id}"),
            ),
            SessionArtifactDescriptor::repository_record(
                SessionArtifactKind::ProjectionEvents,
                "Projection patch event records",
                format!("surrealdb://{PROJECTION_PATCH_TABLE}/{session_id}"),
            ),
            SessionArtifactDescriptor::repository_record(
                SessionArtifactKind::MaterializedNotes,
                "Materialized notes records",
                format!("surrealdb://{MATERIALIZED_NOTES_TABLE}/{session_id}"),
            ),
            SessionArtifactDescriptor::repository_record(
                SessionArtifactKind::MaterializedGraph,
                "Materialized graph records",
                format!("surrealdb://{MATERIALIZED_GRAPH_TABLE}/{session_id}"),
            ),
            SessionArtifactDescriptor::repository_record(
                SessionArtifactKind::LiveAssistAudit,
                "Live assist card records",
                format!("surrealdb://{LIVE_ASSIST_CARD_TABLE}/{session_id}"),
            ),
        ])
    }

    fn delete_session_artifacts(
        &self,
        session_id: &str,
    ) -> Result<SessionArtifactDeleteReport, String> {
        crate::sessions::validate_session_id(session_id)?;
        let _guard = self.lock()?;
        self.delete_session_metadata_records(session_id)?;
        let session_tables = [
            TRANSCRIPT_EVENT_TABLE,
            PROJECTION_PATCH_TABLE,
            MATERIALIZED_NOTES_TABLE,
            MATERIALIZED_GRAPH_TABLE,
            LIVE_ASSIST_CARD_TABLE,
        ];
        for table in session_tables {
            self.delete_session_table_records(table, session_id)?;
        }
        let mut report = SessionArtifactDeleteReport::new(session_id);
        report
            .deleted_repository_records
            .push(format!("surrealdb://{SESSION_METADATA_TABLE}/{session_id}"));
        for table in session_tables {
            report
                .deleted_repository_records
                .push(format!("surrealdb://{table}/{session_id}"));
        }
        Ok(report)
    }

    fn append_transcript_event(
        &self,
        session_id: &str,
        event: &TranscriptEvent,
    ) -> Result<(), String> {
        self.append_session_value(TRANSCRIPT_EVENT_TABLE, session_id, event)
    }

    fn load_transcript_events(&self, session_id: &str) -> Result<Vec<TranscriptEvent>, String> {
        self.load_session_values(TRANSCRIPT_EVENT_TABLE, session_id)
    }

    fn append_projection_patch(
        &self,
        session_id: &str,
        patch: &ProjectionPatch,
    ) -> Result<(), String> {
        self.append_session_value(PROJECTION_PATCH_TABLE, session_id, patch)
    }

    fn load_projection_patches(&self, session_id: &str) -> Result<Vec<ProjectionPatch>, String> {
        self.load_session_values(PROJECTION_PATCH_TABLE, session_id)
    }

    fn save_materialized_notes(
        &self,
        session_id: &str,
        notes: &MaterializedNotes,
    ) -> Result<(), String> {
        self.append_session_value(MATERIALIZED_NOTES_TABLE, session_id, notes)
    }

    fn load_materialized_notes(
        &self,
        session_id: &str,
    ) -> Result<Option<MaterializedNotes>, String> {
        self.latest_materialized(MATERIALIZED_NOTES_TABLE, session_id)
    }

    fn save_materialized_graph(
        &self,
        session_id: &str,
        graph: &MaterializedGraph,
    ) -> Result<(), String> {
        self.append_session_value(MATERIALIZED_GRAPH_TABLE, session_id, graph)
    }

    fn load_materialized_graph(
        &self,
        session_id: &str,
    ) -> Result<Option<MaterializedGraph>, String> {
        self.latest_materialized(MATERIALIZED_GRAPH_TABLE, session_id)
    }

    fn upsert_live_assist_card(
        &self,
        session_id: &str,
        card: &LiveAssistCardRecord,
    ) -> Result<(), String> {
        validate_live_assist_card(session_id, card)?;
        self.append_session_value(LIVE_ASSIST_CARD_TABLE, session_id, card)
    }

    fn load_live_assist_card_audit(
        &self,
        session_id: &str,
    ) -> Result<Vec<LiveAssistCardRecord>, String> {
        self.load_session_values(LIVE_ASSIST_CARD_TABLE, session_id)
    }

    fn load_live_assist_cards(
        &self,
        session_id: &str,
    ) -> Result<Vec<LiveAssistCardRecord>, String> {
        let mut current = BTreeMap::<String, LiveAssistCardRecord>::new();
        for card in self.load_live_assist_card_audit(session_id)? {
            current.insert(card.proposal.id.clone(), card);
        }
        let mut cards = current.into_values().collect::<Vec<_>>();
        cards.sort_by(|a, b| {
            a.proposal
                .created_at_ms
                .cmp(&b.proposal.created_at_ms)
                .then(a.proposal.id.cmp(&b.proposal.id))
        });
        Ok(cards)
    }

    fn append_promotion_event(&self, event: &PromotionEvent) -> Result<(), String> {
        event
            .validate()
            .map_err(|error| format!("Invalid promotion event {}: {error:?}", event.id))?;
        self.append_global_value(PROMOTION_EVENT_TABLE, event)
    }

    fn load_promotion_events(&self) -> Result<Vec<PromotionEvent>, String> {
        self.load_global_values(PROMOTION_EVENT_TABLE)
    }

    fn append_promotion_draft(&self, draft: &PromotionDraft) -> Result<(), String> {
        draft
            .validate()
            .map_err(|error| format!("Invalid promotion draft {}: {error:?}", draft.id))?;
        self.append_global_value(PROMOTION_DRAFT_TABLE, draft)
    }

    fn load_promotion_drafts(&self) -> Result<Vec<PromotionDraft>, String> {
        self.load_global_values(PROMOTION_DRAFT_TABLE)
    }

    fn append_promotion_revocation_request(
        &self,
        request: &PromotionRevocationRequest,
    ) -> Result<(), String> {
        request.validate().map_err(|error| {
            format!(
                "Invalid promotion revocation request {}: {error:?}",
                request.id
            )
        })?;
        self.append_global_value(PROMOTION_REVOCATION_TABLE, request)
    }

    fn load_promotion_revocation_requests(
        &self,
    ) -> Result<Vec<PromotionRevocationRequest>, String> {
        self.load_global_values(PROMOTION_REVOCATION_TABLE)
    }

    fn append_redaction_snapshot(&self, snapshot: &RedactionSnapshot) -> Result<(), String> {
        snapshot
            .validate()
            .map_err(|error| format!("Invalid redaction snapshot {}: {error:?}", snapshot.id))?;
        self.append_global_value(REDACTION_SNAPSHOT_TABLE, snapshot)
    }

    fn load_redaction_snapshots(&self) -> Result<Vec<RedactionSnapshot>, String> {
        self.load_global_values(REDACTION_SNAPSHOT_TABLE)
    }

    fn upsert_org_knowledge_item(&self, item: &OrgKnowledgeItem) -> Result<(), String> {
        item.validate()
            .map_err(|error| format!("Invalid org knowledge item {}: {error:?}", item.id))?;
        ensure_org_visible_record_is_safe(item)?;
        if let Some(existing) = self
            .load_org_knowledge_items()?
            .into_iter()
            .find(|existing| existing.id == item.id)
        {
            let source_changed = existing.source_local_object_fingerprint
                != item.source_local_object_fingerprint
                || existing.source_promotion_event_id != item.source_promotion_event_id;
            if matches!(
                (&existing.state, &item.state),
                (OrgKnowledgeState::Active, OrgKnowledgeState::Active)
            ) && source_changed
                && matches!(&item.conflict_state, PromotionConflictState::None)
            {
                return Err(format!(
                    "Org knowledge item {} source changed without conflict/review state; create a new promotion draft",
                    item.id
                ));
            }
        }
        self.append_global_value(ORG_KNOWLEDGE_ITEM_TABLE, item)
    }

    fn load_org_knowledge_item_audit(&self) -> Result<Vec<OrgKnowledgeItem>, String> {
        self.load_global_values(ORG_KNOWLEDGE_ITEM_TABLE)
    }

    fn load_org_knowledge_items(&self) -> Result<Vec<OrgKnowledgeItem>, String> {
        let mut current = BTreeMap::<String, OrgKnowledgeItem>::new();
        for item in self.load_org_knowledge_item_audit()? {
            current.insert(item.id.clone(), item);
        }
        Ok(current.into_values().collect())
    }

    fn upsert_promotion_sync_state(&self, state: &PromotionSyncState) -> Result<(), String> {
        state.validate().map_err(|error| {
            format!(
                "Invalid promotion sync state {}: {error:?}",
                state.promotion_event_id
            )
        })?;
        self.append_global_value(PROMOTION_SYNC_STATE_TABLE, state)
    }

    fn load_promotion_sync_state_audit(&self) -> Result<Vec<PromotionSyncState>, String> {
        self.load_global_values(PROMOTION_SYNC_STATE_TABLE)
    }

    fn load_promotion_sync_states(&self) -> Result<Vec<PromotionSyncState>, String> {
        let mut current = BTreeMap::<String, PromotionSyncState>::new();
        for state in self.load_promotion_sync_state_audit()? {
            current.insert(
                format!("{}::{:?}", state.promotion_event_id, state.target_kind),
                state,
            );
        }
        Ok(current.into_values().collect())
    }

    fn revoke_org_knowledge_item(
        &self,
        item: &OrgKnowledgeItem,
        sync_state: Option<&PromotionSyncState>,
    ) -> Result<(), String> {
        if !matches!(
            &item.state,
            OrgKnowledgeState::Retracted
                | OrgKnowledgeState::Deleted
                | OrgKnowledgeState::RetentionExpired
                | OrgKnowledgeState::PurgePending
                | OrgKnowledgeState::Purged
        ) {
            return Err(format!(
                "Org knowledge item {} revocation must use a terminal/retracted state",
                item.id
            ));
        }
        if item.deleted_at_ms.is_none() {
            return Err(format!(
                "Org knowledge item {} revocation requires deleted_at_ms",
                item.id
            ));
        }
        if item
            .delete_reason
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return Err(format!(
                "Org knowledge item {} revocation requires delete_reason",
                item.id
            ));
        }
        self.upsert_org_knowledge_item(item)?;
        if let Some(sync_state) = sync_state {
            if !matches!(&sync_state.status, PromotionSyncStatus::Revoked) {
                return Err(format!(
                    "Promotion sync state {} revocation must use revoked status",
                    sync_state.promotion_event_id
                ));
            }
            self.upsert_promotion_sync_state(sync_state)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surreal_memory_repository_passes_replay_parity_conformance_suite() {
        let repo = SurrealMemoryRepository::in_memory().expect("create in-memory repository");

        crate::persistence::repository_conformance::assert_repository_replay_parity_conformance(
            &repo,
            "surreal-replay-parity-session",
        );
    }

    #[test]
    fn surreal_memory_repository_exposes_repository_artifact_descriptors() {
        let repo = SurrealMemoryRepository::in_memory().expect("create in-memory repository");
        let artifacts = repo
            .session_artifacts("surreal-artifact-session")
            .expect("session artifact descriptors");

        assert!(
            repo.session_artifact_paths("surreal-artifact-session")
                .expect("file artifact paths")
                .is_empty()
        );
        assert!(
            artifacts
                .iter()
                .all(|artifact| artifact.file_path().is_none())
        );
        assert!(artifacts.iter().any(|artifact| matches!(
            (&artifact.kind, &artifact.storage),
            (
                SessionArtifactKind::TranscriptEvents,
                crate::persistence::SessionArtifactStorage::RepositoryRecord { uri },
            ) if uri == "surrealdb://transcript_event/surreal-artifact-session"
        )));
        assert!(artifacts.iter().any(|artifact| matches!(
            (&artifact.kind, &artifact.storage),
            (
                SessionArtifactKind::MaterializedGraph,
                crate::persistence::SessionArtifactStorage::RepositoryRecord { uri },
            ) if uri == "surrealdb://materialized_graph/surreal-artifact-session"
        )));
    }

    #[test]
    fn surreal_memory_repository_delete_session_artifacts_deletes_repository_records() {
        let repo = SurrealMemoryRepository::in_memory().expect("create in-memory repository");
        let session_id = "surreal-delete-session";
        crate::persistence::repository_conformance::assert_repository_replay_parity_conformance(
            &repo, session_id,
        );
        assert!(
            !repo
                .load_transcript_events(session_id)
                .expect("transcript events before delete")
                .is_empty()
        );
        assert!(
            !repo
                .load_projection_patches(session_id)
                .expect("projection patches before delete")
                .is_empty()
        );

        let report = repo
            .delete_session_artifacts(session_id)
            .expect("delete repository artifacts");
        assert!(report.deleted_files.is_empty());
        assert!(report.missing_files.is_empty());
        assert!(!report.has_failures(), "unexpected failures: {report:?}");
        assert!(
            report
                .deleted_repository_records
                .iter()
                .any(|uri| { uri == "surrealdb://transcript_event/surreal-delete-session" })
        );
        assert!(
            report
                .deleted_repository_records
                .iter()
                .any(|uri| { uri == "surrealdb://materialized_graph/surreal-delete-session" })
        );
        assert!(
            repo.load_transcript_events(session_id)
                .expect("transcript events after delete")
                .is_empty()
        );
        assert!(
            repo.load_projection_patches(session_id)
                .expect("projection patches after delete")
                .is_empty()
        );
        assert!(
            repo.load_materialized_notes(session_id)
                .expect("notes after delete")
                .is_none()
        );
        assert!(
            repo.load_materialized_graph(session_id)
                .expect("graph after delete")
                .is_none()
        );
    }
}
