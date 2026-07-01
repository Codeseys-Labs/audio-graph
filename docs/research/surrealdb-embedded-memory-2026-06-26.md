# SurrealDB Embedded Memory Spike

Date: 2026-06-26
Seed: audio-graph-5dde

## Recommendation

Spike SurrealDB behind a repository boundary, but do not make it the default session store yet.

AudioGraph already has the right event-sourced domain model for this: immutable transcript span revisions, projection patches, materialized notes, and materialized graph facts. The next architecture move should be a `LocalMemoryRepository` trait with the current JSON/JSONL store as the baseline implementation. A SurrealDB adapter can then be proven by replay parity, migration, and cross-platform packaging tests before it becomes a product dependency.

This is a good fit for future local-first meeting memory and explicit promotion into organization knowledge, but it should not replace current artifact persistence in one step.

## Current Storage

Current durable storage is file-backed under `AUDIOGRAPH_DATA_DIR` or legacy `~/.audiograph`:

- `sessions.json`: session index and counters.
- `transcripts/<session_id>.jsonl`: legacy finalized transcript segments.
- `transcripts/<session_id>.events.jsonl`: immutable transcript span revision events.
- `projections/<session_id>.events.jsonl`: notes/graph projection patches.
- `notes/<session_id>.json`: materialized notes.
- `graphs/<session_id>.json`: legacy `TemporalKnowledgeGraph` snapshot.
- `graphs/<session_id>.materialized.json`: materialized projection graph.
- `usage/<session_id>.json`: token/turn usage.

The repo now has two storage generations: legacy transcript + petgraph snapshot, and newer event-sourced transcript/projection artifacts. Any SurrealDB work must preserve both during migration.

## Repository Boundary

Introduce a backend-owned repository trait below `TranscriptLedger`, materialized projection state, and session browsing, but above file paths:

```rust
trait LocalMemoryRepository {
    fn register_session(&self, metadata: SessionMetadata) -> Result<()>;
    fn append_transcript_event(&self, session_id: &str, event: TranscriptEvent) -> Result<()>;
    fn append_projection_patch(&self, session_id: &str, patch: ProjectionPatch) -> Result<()>;
    fn load_transcript_events(&self, session_id: &str) -> Result<Vec<TranscriptEvent>>;
    fn load_projection_patches(&self, session_id: &str) -> Result<Vec<ProjectionPatch>>;
    fn save_materialized_notes(&self, session_id: &str, notes: &MaterializedNotes) -> Result<()>;
    fn save_materialized_graph(&self, session_id: &str, graph: &MaterializedGraph) -> Result<()>;
    fn load_materialized_state(&self, session_id: &str) -> Result<MaterializedProjectionState>;
    fn list_sessions(&self, query: SessionQuery) -> Result<Vec<SessionMetadata>>;
}
```

Best insertion points:

- `ProjectionRuntimeHandle`: replace direct file writers for projection patches/materialized notes/materialized graph with a repository handle.
- ASR ingestion: append transcript events through the repository after `TranscriptLedger` accepts them.
- Session replay/load: use repository event loads instead of direct `persistence::load_*` calls.
- Keep `TemporalKnowledgeGraph` as an in-memory live/UI structure initially; adapt materialized graph facts into repository records instead of making petgraph write to SurrealDB directly.

Avoid putting SurrealDB in `user_data.rs` or low-level `persistence::io`; those modules should remain path/file utilities and storage-error plumbing.

## SurrealDB Fit

Useful SurrealDB properties:

- Rust embedded operation supports in-memory and file-backed modes, with remote/distributed options later.
- The current SDK supports live queries, which could eventually power local UI subscriptions or cross-process observers.
- SurrealDB's document/graph/vector shape maps naturally to transcript revisions, notes, graph facts, speaker timeline, and future semantic recall.
- Rust SDK docs currently state Rust 1.89+ and compatibility through SurrealDB 3.1.5; AudioGraph already builds with a newer toolchain in current validation.

Risks:

- `kv-rocksdb` pulls in non-Rust dependencies and has known Windows/dev-environment friction; this matters for Tauri release packaging.
- SurrealDB's flexible graph/document modeling can become hard to maintain if every relation has multiple possible shapes. AudioGraph should define strict typed records and repository methods rather than letting arbitrary SurrealQL leak through the app.
- The existing JSON/JSONL artifacts are easy to inspect and recover. Any database adapter needs export, backup, corruption recovery, and replay parity before it replaces them.

Preferred spike engines:

- Start with `kv-mem` for tests and adapter shape.
- Evaluate file-backed `kv-surrealkv` and `kv-rocksdb` separately for packaging, durability, and performance.
- Keep remote/distributed SurrealDB out of the MVP adapter; model org promotion as an explicit sync/promotion layer over local records.

## Schema Objects

Minimum local schema:

- `session`: lifecycle, title, counts, source settings fingerprint, schema/artifact versions.
- `transcript_span_revision`: current `TranscriptEvent` fields, including provider/source/speaker/channel/revision/supersedes/latency.
- `transcript_span_current`: latest accepted revision per span.
- `speaker`: speaker identity, display label, provider/local source, display metadata.
- `speaker_timeline_segment`: speaker, session, span, time range, stability, source, revision/provenance.
- `projection_job`: job id, kind, basis, priority, queued time.
- `projection_patch`: sequence, kind, operations, confidence, prompt/model/provider provenance, basis, latency.
- `note_revision` and `note_current`: revisioned notes and latest materialized notes.
- `graph_node_fact` and `graph_edge_fact`: revisioned graph facts with valid_from/valid_until, confidence, basis, provenance.
- `live_assist_card`: durable live cards, status, source spans, approval/dismissal result.
- `promotion_event`: explicit promotion of a local object/version to an org/workspace.
- `org_knowledge_item`: promoted, redacted, ACL-bound org memory with provenance, conflict lineage, deletion/retention markers.

## Org Promotion Model

Local session memory remains private by default. Promotion is an explicit event that records:

- source object type/id/version and source session/span/note/fact provenance;
- redaction snapshot and policy version;
- actor/user/workspace/org target;
- ACL hooks and retention/deletion markers;
- remote id and sync status.

The promoted org item should never be a hidden mirror of the whole local session. It should be a selected, redacted, provenance-preserving copy of a note/fact/card version.

## Follow-Up Work

The implementation queue should be:

1. Define `LocalMemoryRepository` plus `FileMemoryRepository` preserving current JSON/JSONL behavior.
2. Add replay parity tests for transcript ledger and materialized projection state against the file adapter.
3. Add an opt-in SurrealDB adapter spike using `kv-mem` first.
4. Evaluate file-backed SurrealDB engines on Linux, macOS, and Windows in CI before defaulting.
5. Design migration/dual-read from existing artifacts with backup and rollback.
6. Promote speaker timeline and live assist cards into durable revisioned objects.
7. Design org promotion/redaction/ACL/sync records before any cloud or federated store.

## Sources

- SurrealDB Rust embedding docs: https://surrealdb.com/docs/languages/rust/embedding
- SurrealDB Rust SDK overview: https://surrealdb.com/docs/languages/rust/overview
- SurrealDB Rust local engine docs: https://docs.rs/surrealdb/latest/surrealdb/engine/local/index.html
- SurrealDB Rust live queries: https://surrealdb.com/docs/languages/rust/concepts/live
- SurrealDB vector search model: https://surrealdb.com/docs/learn/data-models/vector-search/overview
- SurrealKV repository: https://github.com/surrealdb/surrealkv
