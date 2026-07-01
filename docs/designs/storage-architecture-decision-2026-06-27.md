# Storage Architecture Decision — File-Canonical Event Logs, DB Gated on Evidence

Date: 2026-06-27
Working dir: `docs/designs/`
Companion to: `docs/designs/_storage-megaloop-2026-06-27/` (frame, plan-outline, discover-*, sweep-*)
Research corpus: `research/notes/` (hyperresearch report `final_report_storage-arch-tauri-temporal-graph-6415ba.md` + ~80 cited notes)
Decision record: `docs/adr/0021-storage-architecture.md` (the ADR; this design doc is its detailed companion and its References target)

---

## 0. Status and one-paragraph summary

**Status: proposed.** This document details the architecture the ADR ratifies. The decision is
deliberately *gated on evidence*, not a clean go/no-go, because the single experiment that would
discriminate between the two plausible database futures has produced zero evidence (`frame.md` §6;
`final_report` §10). The recommendation: **set the storage dial to B now** — the file-based
`FileMemoryRepository` is the only default and the only selectable store — while **holding A' as the
formal posture** (keep the `LocalMemoryRepository` trait seam, keep the `kv-mem` SurrealDB adapter as
an off-by-default conformance target), **rejecting A (SurrealDB-primary) as the default**, and
**pre-committing to move toward file-canonical C** (human-readable logs canonical + a rebuildable
embedded index) on a *named demand signal*, with the index engine itself left as a gated sub-decision
(SurrealKV vs SQLite vs redb) because the corpus does not resolve it. Every load-bearing claim below
cites either a research source (`[[note-id]]`) or a code location (`file:line`).

---

## 1. The dial model

The four "options" in the goal (A SurrealDB-primary, B keep-files, C hybrid, A' status-quo baseline)
are not four discrete forks. They are **four positions on a single dial**, and the mechanism that lets
the app slide along that dial cheaply is the abstraction that *already exists*: `trait
LocalMemoryRepository: Send + Sync` (`persistence/mod.rs:424`). The decision is therefore not "introduce
a storage boundary" — that is built and conformance-tested — but "which adapter is the default, and what
evidence moves the dial off it" (`frame.md` §2.1; `discover-src_tauri_src_persistence.md` §1).

```
  A'  ──────────  B  ──────────────  C (file-canonical)  ──────────  A (DB-canonical)
 (seam +        (file default,      (logs canonical +              (DB primary;
  file default,  the only            rebuildable embedded           opaque file;
  DB gated        selectable          index; logs                    needs a file-
  opt-in)         store)              authoritative)                 export path → C)

  ← cheapest, reversible                                  most expensive, riskiest →

  position TODAY:  A' (posture)  +  B (active default)
  pre-committed next step, ON SIGNAL:  C (file-canonical)
  rejected as default (retained as gated opt-in adapter):  A
```

Each position is correct under a nameable boundary condition (`final_report` §10):

| Dial position | What it means | Correct when |
|---|---|---|
| **A'** | Keep the trait seam; file adapter is default; any DB is a gated, off-by-default opt-in adapter | Always — it is the cheapest correct posture and the substrate every other position builds on |
| **B** | `FileMemoryRepository` is the *only* default and only selectable store | The current workload: bounded data, no indexed-query demand, human-readable transcripts are a hard product requirement |
| **C (file-canonical)** | Human-readable event logs stay the source of truth; an embedded DB is a *derived, rebuildable* index | A concrete indexed-query need materializes (cross-session recall, large-graph traversal beyond caps, vector search) |
| **A (DB-canonical)** | A database is the primary, canonical store | Only on the full conjunction of green cross-platform packaging evidence, a stabilized durable engine, *and* a committed multi-model/sync future — and even then as a remote/federated adapter, never the local default |

The honest verdict is **gated-on-evidence** because two independent decision documents converge on the
*posture* but **diverge on the eventual engine** — `surrealdb-revisit-2026-06-27.md` lands on SurrealKV/
RocksDB; the hyperresearch `final_report` lands on SQLite — and the deciding evidence for both is absent
(`plan-outline.md` §0). The disciplined move is to ratify the low-cost posture both agree on and gate the
engine choice on the cheap experiment that would discriminate between them, not to pick SurrealKV or
SQLite blind.

### 1.1 Why B serves today and the DB upside is deferred

- **B fully serves the current workload.** Hot-path append is O(1) JSONL with per-append fsync
  (`append_jsonl`, `persistence/mod.rs:154`; backs `append_transcript_event` and `append_projection_patch`)
  behind bounded, drop-new writer threads (`TranscriptEventWriter`, cap 2048,
  `discover-src_tauri_src_persistence.md` §3.2). Data is bounded — sessions index capped at 100
  (`persistence/mod.rs:872`), graph capped at 1000 nodes / 5000 edges (`graph/temporal.rs:45,48`) — so
  whole-session load-and-replay is cheap and the "need a query engine for scale" argument does not bite
  today (`frame.md` §5).
- **The DB upside is unproven and partly mis-built.** The spike runs `kv-mem` only — zero durability
  evidence (`Cargo.toml:218`; `sweep-2.md` §5). It stores opaque `serde_json::Value` in `SCHEMALESS`
  tables (`surreal.rs:104`), exercising none of SurrealDB's graph/vector/live-query value, and it does an
  **O(n) full-table scan per append** to assign sequences (`next_session_sequence`, `surreal.rs:153`;
  `next_global_sequence`, `surreal.rs:167` — both call `select_all` then `max(sequence)+1`), which fails
  fit-criterion #1 and would regress the O(1) JSONL append at streaming frequency
  (`discover-src_tauri_src_persistence.md` §8).
- **The cost of holding A' is near-zero; the cost of jumping to A is asymmetric.** B pays nothing. A pays
  the full migration plus a data-loss risk on a bad bet against a still-moving schema. C pays a small,
  reversible cost because a derived index is rebuildable from the authoritative logs (`final_report` §10).

---

## 2. Concrete data shapes the store must preserve byte-for-byte

A storage swap conforms only if it round-trips every persisted type with serde fidelity intact —
including the absent-vs-null distinctions that `skip_serializing_if` / `#[serde(default)]` encode, and the
pinned `SCHEMA_VERSION = 1`. These are not cosmetic: basis-validated replay is order- and field-sensitive,
so a KV/blob adapter that loses a field or reorders events breaks replay
(`discover-src_tauri_src_persistence.md` §5, §6).

### 2.1 Transcript events — `TranscriptEvent` (`projections.rs:31`)

Immutable span-revision event. Fields:
`span_id, provider, source_id, provider_item_id?, transcript_segment_id?, speaker_id?, speaker_label?,
channel?, text, start_time/end_time (f64 secs), confidence (f32), is_final, stability (Partial|Final,
snake_case), revision_number (u64), supersedes?, turn_id?, end_of_turn, raw_event_ref?,
capture_latency_ms?, asr_latency_ms?, received_at_ms (u64)`.

- `Option` fields use `skip_serializing_if`; the two `*_latency_ms` fields use `#[serde(default)]` for
  back-compat. **A swap must preserve absent-vs-null:** an absent latency field is not the same byte
  sequence as `null`, and the replay fold tie-breaks on `received_at_ms`, so dropping or defaulting these
  silently corrupts ledger ordering.
- WRITE: many per second under streaming ASR (one event per partial + final per span). Append must be
  O(1) and non-stalling (`frame.md` §3.1).
- READ: load-all-by-session, then replay into a `TranscriptLedger` (latest revision per `span_id` wins by
  `revision_number`, tie-break `received_at_ms`; `projections.rs:127`).

### 2.2 Notes projection — `ProjectionPatch` / `ProjectionOperation` / `ProjectionBasis`

- **`ProjectionPatch`** (`projections.rs:387`): `sequence (u64), kind (Notes|Graph), llm_request_id,
  basis (ProjectionBasis), operations (Vec<ProjectionOperation>), confidence (f32), provenance
  {provider, model, prompt_id}, queued_at_ms?, generation_latency_ms?, apply_latency_ms?, created_at_ms`.
- **`ProjectionOperation`** (`projections.rs:564`, internally tagged `type`, snake_case): note ops
  (`upsert_note`/`delete_note`/`reorder_note`) plus graph ops (`upsert_graph_node`, `remove_graph_node`,
  `invalidate_graph_node`, `upsert_graph_edge`, `remove_graph_edge`, `invalidate_graph_edge`,
  `strengthen_graph_edge`, `weaken_graph_edge`, `merge_graph_nodes`, `split_graph_node`). This is a
  CRDT-ish diff stream; **invalidate-vs-remove is the bitemporal lever** — invalidate sets `valid_until_ms`,
  remove deletes.
- **`ProjectionBasis`** (`projections.rs:156`): `span_revisions (Vec<{span_id, revision_number}>),
  diarization_span_revisions, transcript_hash`. This is the provenance contract that ties a patch to the
  exact transcript revisions it was built from — the spine of basis-validated replay
  (`projections.rs:1411`; `discover-src_tauri_src_persistence.md` §6.4).

### 2.3 Materialized snapshots — `MaterializedNotes` and `MaterializedGraph`

- **`MaterializedNotes`** (`projections.rs:650`): `schema_version (=1, projections.rs:671), session_id,
  last_sequence, notes (Vec<MaterializedNote{id, title, body, tags, updated_by_sequence, updated_at_ms,
  basis, provenance}>)`. The notes vector is **ordered** — reorder ops are load-bearing.
- **`MaterializedGraph`** (`projections.rs:870`): `schema_version (=1, projections.rs:894), session_id,
  last_sequence, nodes, edges`.
  - **`MaterializedGraphNode`** (`projections.rs:791`): `id, name, entity_type, description?,
    confidence (f32, default fn), valid_from_ms (u64, default 0), valid_until_ms (Option<u64>),
    updated_by_sequence, updated_at_ms, basis, provenance`.
  - **`MaterializedGraphEdge`** (`projections.rs:830`): adds `source, target, relation_type, label?,
    weight (f32)` to the same bitemporal fields.
- The conformance suite asserts **exact `valid_until_ms`** on an invalidated node and edge after replay
  (`persistence/mod.rs:3329, 3335`). A store that loses or coarsens this value fails the gate.

### 2.4 Session index — `SessionMetadata` (`sessions/mod.rs:56`)

`id, title?, created_at, ended_at?, duration_seconds?, status ("active"|"complete"|"crashed"),
segment_count, speaker_count, entity_count, transcript_path, graph_path, deleted (default),
deleted_at? (default)`. The two `*_path` strings are file-system paths under the file adapter; the surreal
spike stuffs `surrealdb://...` URIs into the same fields (`surreal.rs:327-328`). **A swap must decide what
these path fields mean** for a non-file backend — they are part of the serialized shape, and
`SessionArtifactStorage::File{path}` vs `RepositoryRecord{uri}` (`persistence/mod.rs:347`) is the abstract
hook for it.

### 2.5 Promotion / org-knowledge (`promotion.rs`, ~30 types, `PROMOTION_SCHEMA_VERSION = 1` at `promotion.rs:16`)

Six record families split into two storage strategies (`discover-src_tauri_src_promotion_rs.md` §1):

- **Append-only immutable audit logs** (no "current" view): `PromotionEvent` (`promotion.rs:312`),
  `PromotionDraft`, `PromotionRevocationRequest`, `RedactionSnapshot`.
- **Audit-log + derived "current" snapshot pairs** (the only stateful ones): `OrgKnowledgeItem`
  (`promotion.rs:396`; state machine Active → Retracted/Deleted/RetentionExpired/PurgePending/Purged) and
  `PromotionSyncState`. Each upsert appends the full record to the `.jsonl` audit log *and* rewrites the
  `.current.json` deduped-latest array.

Byte-for-byte requirements: every struct uses `serde(deny_unknown_fields)` (so an unknown raw field fails
deserialization), and `OrgKnowledgeItem` carries bitemporal validity (`valid_from_ms` required-positive,
`valid_until_ms?`) plus `created/updated_at_ms`, `deleted_at_ms?`, `revision_number`, `current_revision_id`,
`source_basis: ProjectionBasis` (the cross-link into the transcript ledger). `ApprovedOrgPayload`
(`promotion.rs:248`) stores free-form data as a `BTreeMap<String, serde_json::Value>` — **BTreeMap gives
deterministic key order on serialize**, which any adapter must preserve to keep byte-equivalent output
(`discover-src_tauri_src_promotion_rs.md` §4).

### 2.6 Legacy gen-1 graph — `SerializableGraph` (`graph/temporal.rs`)

`{nodes: Vec<GraphEntity>, edges: Vec<SerializableEdge{source_name, target_name, edge: TemporalEdge}>,
event_counter}`. Edges are stored **by lowercased node name**, re-attached through `name_index` on load,
and dropped if an endpoint is missing (`graph/temporal.rs:729-755`; `discover-src_tauri_src_graph.md`
§1.1, §2). `TemporalEdge.seq_id` is `#[serde(default)]` and **re-derived 0..n on load** so older saves
missing it do not all collapse to `edge-0` (`graph/temporal.rs:744`). Time fields here are `f64`
seconds-since-capture-start — a **different time unit** from the materialized graph's `u64` epoch-ms
(`discover-src_tauri_src_graph.md` §2). A swap must keep both unit conventions and the re-derivation
behavior.

### 2.7 The schema-version invariant

All snapshot types pin `SCHEMA_VERSION = 1` (`projections.rs:189, 671, 894`; `PROMOTION_SCHEMA_VERSION`
at `promotion.rs:16`). There is a version field but **no migration code yet** — a swap cannot silently
change the value, and the version field is the explicit seam a future migration would key off
(`discover-src_tauri_src_persistence.md` §5).

---

## 3. Query patterns the store actually serves

There is **no query engine and no index** today. Every read is load-all-then-fold-in-memory
(`discover-src_tauri_src_persistence.md` §4; `discover-src_tauri_src_graph.md` §5):

- **Transcript / notes / graph:** load the whole JSONL or JSON, then fold in pure Rust via the
  trait's *default methods* — `replay_transcript_ledger` (`persistence/mod.rs:553`),
  `replay_projection_state` (`:559`), `load_materialized_projection_state` (`:573`). Because replay lives
  in default methods over pure-Rust folds, **replay is identical across adapters** — an adapter only has
  to make `append_*` and `load_*` byte- and order-correct.
- **The only thing resembling a "query"** is in-Rust top-k RAG: `build_graph_chat_context(snapshot, query,
  max_nodes)` (`graph/entities.rs:222`) scores nodes by query-token overlap plus a `mention_count`
  centrality tiebreak over a *fully-loaded* `GraphSnapshot`, keeping only edges whose both endpoints
  survive. Hand-rolled, not a DB query (`discover-src_tauri_src_graph.md` §5).
- **No traversal, no as-of, no vector search exists anywhere.** Active-fact filtering is a linear
  `valid_until_ms.is_none()` scan; entity resolution is exact `name_index` lookup with a
  `jaro_winkler` fuzzy fallback that has no caller yet (`graph/temporal.rs:278`). As-of queries are
  possible only *by replay*, not by a stored index.

This matters for the decision in two ways. First, **the workloads that would justify a query engine are
future, not present** — adopting one now is cost without realized benefit (`frame.md` §4 criterion #7).
Second, the existing in-Rust chat-context retrieval is the **leading indicator**: it is the most plausible
near-term trigger for building the file-canonical index, which is why the recommendation is gated-but-
imminent rather than indefinite deferral (`final_report` §9-10).

---

## 4. The two-generation reality (a swap must carry both)

Two storage generations coexist on disk and **both are live** (`discover-src_tauri_src_persistence.md` §2;
`discover-src_tauri_src_graph.md` §1):

| | Gen-1 (legacy) | Gen-2 (event-sourced) |
|---|---|---|
| Transcript | `transcripts/<id>.jsonl` (`TranscriptSegment`, appended by `TranscriptWriter`) | `transcripts/<id>.events.jsonl` (`TranscriptEvent`, append-only hot path) |
| Notes | — | `projections/<id>.events.jsonl` (`ProjectionPatch` log) + `notes/<id>.json` snapshot |
| Graph | `graphs/<id>.json` (petgraph `SerializableGraph`, 30 s autosave, destructive eviction) | `graphs/<id>.materialized.json` (folded `MaterializedGraph`) |
| Durability | whole-file snapshot every 30 s; **NOT replayable** | append-only log is the source of truth; snapshot is a rebuildable cache |
| Time unit | `f64` seconds | `u64` epoch-ms |

The reconciliation point is `load_session` → `choose_materialized_graph` (`commands.rs:5206`; the discover
docs cite the pre-shift line `5184`). It loads the petgraph snapshot for the UI, loads the materialized
snapshot, replays the projection patch log, then **prefers whichever materialized graph has the higher
`last_sequence`** (`materialized_graph_has_content` at `commands.rs:5186`). So `last_sequence` is the merge
key between the snapshot fast-path cache and the authoritative event log; the petgraph file has no such
authority — it is purely the legacy / live-UI view (`discover-src_tauri_src_graph.md` §4).

**Implication for any swap:** the two generations use different time units, different id schemes
(uuid + `edge-{seq_id}` vs patch-assigned ids), and different durability models. A storage swap must carry
both and preserve the `last_sequence` reconciliation, or it is incomplete. Collapsing the two graph
representations into one is a *data-model redesign* (which would itself need the conformance gate extended),
not a storage-engine choice (`discover-src_tauri_src_graph.md` §8). This is the single most under-counted
cost in any naive "swap files for a DB" proposal.

---

## 5. How human-readability is preserved

Human-readable, portable transcripts and notes are an **explicit product requirement** — a user can open
`transcripts/<id>.jsonl` in any editor, `grep` it, back it up with `cp`, and recover it by deleting a torn
last line (`frame.md` §5). The architecture preserves this by keeping **files canonical and any index
derived**:

- The event logs (`*.events.jsonl`) and snapshots (`notes/<id>.json`, `graphs/*.json`) are the source of
  truth in every position on the dial up to and including file-canonical C. An embedded DB, if added, is a
  *projected view* of the logs — exactly the Obsidian/Anytype/Dendron/sandbar pattern where the database
  is "a background-updated eventually-consistent index" rebuilt from the canonical text
  (`final_report` §4; `[[understanding-obsidian-and-how-it-works-meta-obsidian-forum]]`,
  `[[sandbardocconceptsmarkdown-as-canonicalmd-at-master-danlentzsandbar-github]]`).
- **An opaque DB file does not satisfy readability on its own.** A `kv-surrealkv`/RocksDB file is a binary
  blob; even Option A would need a file-export path, which collapses A toward C in practice
  (`frame.md` §5; `final_report` §7).
- The codebase already encodes a file-export contract: `export_graph` (`commands.rs:4577`; discover docs
  cite the pre-shift line `4555`) serializes the graph snapshot to a JSON string for clipboard/download,
  and `PromotionSyncTargetKind::FileExport` (`promotion.rs:88`) is a first-class sync target alongside the
  anticipated `SurrealdbRemote` (`discover-src_tauri_src_graph.md` §7.8;
  `discover-src_tauri_src_promotion_rs.md` §8). These reinforce that file export is a designed transport,
  not an afterthought.

The one precision the design must state honestly (the discover trace corrected the frame here): "JSONL =
truncate-tail recovery" is *not* automatic. `load_jsonl` returns `Err` on any malformed line, including a
torn last line (`persistence/mod.rs:2430`). The real properties are (a) **loud-fail, no silent data loss**
and (b) **trivial manual repair** because the file is plain text. A DB must not be unfairly credited or
discredited on "recovery"; the bar is loud-fail parity plus a repair/export path
(`discover-src_tauri_src_persistence.md` §10).

---

## 6. Migration / dual-read with rollback

The migration design is the reason C is cheap and A is expensive. Because the materialized snapshots are
*derived caches* rebuilt from the append-only logs (every durability test deletes the snapshot and rebuilds
it from the log, `persistence/mod.rs:3951-3967`), **a derived index is rebuildable — so rollback is "drop
the index," not a data-loss event** (`final_report` §10).

**Dual-read / backfill (for C):**

1. Logs remain authoritative and untouched. Stand up the embedded index as a *derived projection* of the
   canonical logs.
2. Backfill the index from both generations (gen-1 legacy transcript + petgraph; gen-2 event logs +
   materialized snapshots — §4). Backfill must be **idempotent and non-destructive**.
3. Read path is dual: serve from the index where it answers a query, fall back to the load-all-then-fold
   path otherwise; assert parity between the two during a bake-in window.
4. **Rollback = drop the index** and revert to the file-fold read path. No log data is ever at risk because
   the logs were never the thing being migrated.

**Sequencing guardrail (schema-freeze-first).** The event-sourced core is still moving (seeds `ad44`
data-model, `4da5` revision ledger, `9c89` artifact migration are open). Migrating onto an index against a
moving schema means migrating twice. **Land or freeze that schema before any index migration (W3/W4 in the
impl plan)** so it migrates once (`plan-outline.md` §0 gate 5, §2 sequencing guardrails;
`final_report` §10). The `SCHEMA_VERSION` / `PROMOTION_SCHEMA_VERSION` fields are the explicit seam for a
versioned migration (§2.7).

**Why A's migration is asymmetric.** A (DB-canonical) must additionally: re-implement the bespoke disk-full
UX the DB adapter loses (§7), carry both generations through a dual-read backfill with rollback against an
*opaque* store, lift the promotion/privacy invariants into the shared gate (§8), and thread the trait
through the remaining hard-wired call sites — all of it against a store from which a bad bet is a data-loss
event, not a rebuild (`final_report` §10). C pays only the last two costs, reversibly.

---

## 7. Durability and disk-full UX parity (a real, often-overlooked cost)

The file adapter's durability and disk-full handling are bespoke and product-visible. Any DB adapter loses
all of it unless it re-implements equivalents (`discover-src_tauri_src_persistence.md` §3.3;
`discover-src_tauri_src_graph.md` §3):

- **Per-append fsync** on every JSONL append (`append_jsonl` → `sync_all`, `persistence/mod.rs:154`).
- **Atomic fsync-before-rename** for snapshot writes (`save_json`, `persistence/mod.rs:2328`): write
  `*.json.tmp` → flush → `sync_all` → owner-only chmod → `fs::rename` → re-chmod. The fsync-before-rename is
  a deliberate crash-consistency guarantee against a zero-length file replacing a known-good snapshot.
- **Owner-only permissions** (`fs_util::set_owner_only`) on files containing transcribed speech.
- **`CAPTURE_STORAGE_FULL` disk-full UX** (`persistence/io.rs:90`): classify ENOSPC, debounce via a
  process-wide atomic, emit a Tauri event once, probe writability on the user's "Resume" retry.
- **Corrupt-index quarantine** (`persistence/mod.rs:213`): a malformed session index is backed up to
  `*.json.corrupt-<ts>` and an empty index returned so a read-modify-write caller can rewrite cleanly.
- **Bounded, non-blocking writer threads** with drop-new-on-full and a backpressure Tauri event
  (`TranscriptEventWriter`, cap 2048; `discover-src_tauri_src_persistence.md` §3.2).

The surreal spike has **none** of this disk-full surfacing (`discover-src_tauri_src_persistence.md` §8). A
swap that wants parity must either keep text logs or re-implement equivalent disk-full classification,
backup/export, and corruption recovery — a concrete, named migration cost.

---

## 8. Conformance-parity strategy

The shared gate `assert_repository_replay_parity_conformance(repo, session_id)`
(`persistence/mod.rs:3165`) is the migration safety net and the acceptance bar. Today it covers transcript
revisions (latest-wins replay), interleaved notes + graph patches (note ordering, deletion, and **exact
`valid_until_ms`** on an invalidated node and edge), save→reload equality with the replay, and a
stale-revision sub-case (`discover-src_tauri_src_persistence.md` §7). Both `FileMemoryRepository`
(`persistence/mod.rs` test at `:4130`) and the gated `SurrealMemoryRepository` (`surreal.rs:689`) pass it.

**The gap, and the strategy.** The promotion / redaction / org-knowledge / privacy invariants are pinned
**only by file-adapter-`#[test]`s** (`persistence/mod.rs:4344-4699`), *not* by the shared conformance fn.
The surreal adapter runs only the shared suite, so **promotion round-trip, audit-vs-current divergence,
revoke guards, the silent-source-mutation guard, and the privacy floor are currently UNVERIFIED for any
non-file adapter** (`discover-src_tauri_src_promotion_rs.md` §7 item 8). The strategy is to **lift those
file-only invariants into the shared gate** so any third adapter is checked against them, specifically:

- **Audit immutability + completeness:** every accepted record appended, never mutated; rejected records
  (validation / privacy / redaction failures) never appear in the audit log.
- **Audit-vs-current divergence:** `load_*_audit` returns all revisions; `load_*` (current) returns one per
  key, last-write-wins, with stable sort order (`id`; `(promotion_event_id, target_kind)`). Note the
  file/surreal fold-time divergence: the file adapter folds at *write* time into `.current.json`; the
  surreal adapter folds at *read* time via `BTreeMap`. Both converge on last-write-wins-per-key, but a new
  adapter must reproduce the same key definitions and sort order or parity drifts
  (`discover-src_tauri_src_promotion_rs.md` §3).
- **The two write guards:** silent-source-mutation rejection (`persistence/mod.rs:1215-1230`) and the
  terminal-state / `deleted_at_ms` / `delete_reason` / `Revoked`-sync requirements in
  `revoke_org_knowledge_item`.
- **The privacy floor:** `serde(deny_unknown_fields)` + each type's `.validate()` +
  `ensure_org_visible_record_is_safe` (`persistence/mod.rs:240`) + `validate_redacted_error`. Org-visible
  serialization must never contain `raw_transcript_text`, `speaker_names`, `source_ids`, `provider_ids`, or
  credentials.

A no-regret refactor supports this: hoist the shared validation/privacy gates
(`ensure_org_visible_record_is_safe` `:240`, `validate_live_assist_card` `:292`, per-type `.validate()`)
into trait default methods so they are not duplicated per adapter and a third adapter cannot bypass them
(`discover-src_tauri_src_persistence.md` §10; impl plan W1b/W1c). The acceptance bar is exact: equality is
checked on full `MaterializedProjectionState`, not a subset.

---

## 9. Packaging and binary-size impact

Tauri ships Win/mac/Linux (`tauri.conf.json` targets `app, dmg, nsis, appimage, deb`; macOS universal fat
binary). The toolchain is pinned to Rust 1.95.0, above SurrealDB's MSRV 1.89+ floor — no toolchain conflict
today (`sweep-2.md` §2, §7).

| Storage dep / feature | Native toolchain | Cross-platform risk | Status |
|---|---|---|---|
| `kv-mem` (current spike) | pure-Rust; no C++ | none new | compiled, off by default (`Cargo.toml:218`) |
| `kv-surrealkv` (pure-Rust file engine) | pure-Rust | unknown — never compiled in this repo | **untested** (`sweep-2.md` §6) |
| `kv-rocksdb` (C++ file engine) | C++ / cmake / clang (Windows `/MT`-vs-`/MD`; cross-compile failures) | high on Windows | **untested; likely risky** (`sweep-2.md` §6, §11) |
| `ring` (transitive) | C build | none new — **already transitive** via `aws-sdk-transcribestreaming` and now surrealdb 3.1.4 | present (`sweep-2.md` §4) |

Key facts (`sweep-2.md` §3-§6, §9-§11):

- `surrealdb-embedded` is **off by default in all builds** (`Cargo.toml:40`); the default `local-ml` path
  and the `cloud` fast path both omit it.
- The Windows test-harness abort (`0xC0000139`) was traced to `aws-lc-sys`/`ring`, **not** the ML libs, and
  fixed by an embedded SxS manifest, not dep removal (ADR-0007 correction). So a storage dep that also pulls
  `ring` adds no *new* native burden — `ring` is already an always-on transitive dep.
- The `kv-mem` SurrealDB dep tree is large (8 surrealdb crates; `surrealdb-core` pulls `diskann`,
  `object_store`, `ndarray`, crypto crates) but pure-Rust. **Its binary-size delta is not yet benchmarked.**
- There is **no `[profile.release]` override** (no LTO, no `strip`) in `Cargo.toml`, so there is no
  repository-level size mitigation in effect — any large storage dep contributes directly to bundle size.
  A measured baseline is a no-regret prerequisite (impl plan W1d).
- A controlled Tauri comparison measured SurrealDB at roughly **46 MB `.app` (16 MB compressed `.dmg`) and
  ~70 MB RAM vs SQLite's ~5 MB and ~30 MB** — the largest footprint of any candidate, though feasible
  (`final_report` §2; `[[tauri-demoexamplessurrealdb-at-master-huakunshentauri-demo-github]]`).

**The blocking gate (`audio-graph-2b2c`).** Build/link time, stripped binary-size delta, native-dep
inventory, corruption/backup behavior, and Tauri packaging for `kv-surrealkv` *and* `kv-rocksdb` on
Blacksmith Linux/macOS/Windows are **all unmeasured**. This is one CI run from existing (`frame.md` §6
Q1-Q3; `sweep-2.md` §6, §10). Until it runs, "lean in" or "remove it" both assert past the evidence.

---

## 10. Engine comparison

Two axes decide each candidate: **query-model fit** for a bitemporal temporal-graph + event-sourced
workload, and **longevity/governance** (the bus-factor on betting durable storage on it). There is no
single best engine — only a best engine per boundary condition (`final_report` §2, §6). Note the data model
(valid-time intervals + transaction-time markers, corrections invalidate rather than delete) is **portable
across all of these** — the engine is downstream of the model, so the real question is "at what scale does
an index beat the loaded-snapshot approach," which is bounded by design today (`final_report` §3).

| Engine | Temporal-graph fit | Longevity / governance | Packaging (Tauri) | Verdict for this app |
|---|---|---|---|---|
| **In-memory** (`Mem`/SurrealMX) | n/a (lost on close) | stable | pure-Rust | tests only; **zero durability evidence** — the current spike runs here only |
| **RocksDB** (SurrealDB durable) | good (LSM, write-optimized) | mature | **C++/clang/bindgen; Windows `/MT`; cross-compile failures; largest** | durable but compounds the ADR-0007 native-link minefield |
| **SurrealKV** (SurrealDB pure-Rust) | good, **but back-in-time versioning caveat** collides with retroactive revision | **beta; unstable on-disk format**; single-vendor | pure-Rust (good); ~46 MB `.app` (largest) | avoids C++ but beta + the caveat hits this app's exact data shape |
| **SQLite** (+FTS5, recursive CTE) | strong (recursive CTE traversal + bitemporal columns + FTS5; production-proven for this exact agent-memory model) | **highest** (public domain, ubiquitous, not single-vendor) | Tauri-blessed, single-file, ~5 MB | **first choice for the index slot when one is warranted** |
| **redb** | none (KV only; hand-roll indexing) | high (stable, stable file format; the sled successor) | pure-Rust, single-file | pure-Rust KV fallback; no query power over files |
| **KuzuDB / LadybugDB** | **best raw** (native Cypher, columnar, vector + FTS) | **low** — archived 10 Oct 2025; fork governance unproven | embeddable | only on a Cypher-heavy need with proven governance; the empirical proof of single-vendor bus-factor risk |
| **DuckDB** | wrong shape (OLAP/columnar; bad for single-row OLTP appends) | high | embeddable | analytical sidecar only, never the primary index |
| **libSQL / Turso** | SQLite-superset + vector | medium (active; **server-primary** sync model) | SQLite-compatible | only if cross-device sync becomes a goal |
| **sled** | none (KV only) | low (beta; unstable on-disk format; warns "use SQLite if reliability matters") | pure-Rust | avoid |

Engine-level conclusions (`final_report` §2, §6, §7):

- **SurrealDB-primary is rejected as the default**, not foreclosed. As a primary store the case collapses:
  durability weaker-by-default (`Eventual` at the crate level; `sync=every` repaired at the engine level in
  Feb 2026, but the safe mode is the slow mode), the only mature-durable engine (RocksDB) is a C++ burden,
  SurrealKV is beta with a back-in-time versioning caveat that collides with the app's retroactive-revision
  model (transcript `supersedes`; graph `invalidate_*` setting `valid_until_ms` after the fact), the bundle
  is the largest of any candidate, and none of its distinctive value is exercised today.
- **SQLite is the index engine on both axes** if and when an index is warranted — strong temporal-graph fit
  via recursive CTEs + bitemporal columns + FTS5, lowest longevity risk, Tauri-blessed, backup is a file
  copy.
- **KuzuDB's Oct-2025 archival and sled's instability are the empirical proof** of the bus-factor risk that
  also attaches to single-vendor SurrealDB — the boring public-domain option outranks the more capable
  single-vendor ones for *durable* storage.
- **The engine sub-decision (SurrealKV vs SQLite vs redb) is left gated** on `2b2c` plus the demand-signal
  shape: graph/vector/sync demand + green SurrealKV → SurrealKV is the candidate; FTS / recursive-CTE
  traversal at bounded scale → SQLite is the lighter candidate. Pick on measured numbers, not blind
  (`plan-outline.md` §0 gate 4, §2 Wave 2).

---

## 11. Evidence gates (what turns "gated" into go/no-go)

The verdict is gated because these are unmeasured (`frame.md` §6; `plan-outline.md` §0):

1. **`2b2c` — the blocking gate.** Build/link time, stripped size delta, native-dep inventory,
   corruption/backup behavior, Tauri packaging for `kv-surrealkv` *and* `kv-rocksdb` on Blacksmith
   Linux/macOS/Windows. Unmeasured; schema-independent; one CI run from existing.
2. **Streaming-frequency throughput** for any indexed adapter vs the O(1) JSONL append. Conformance proves
   correctness, not throughput.
3. **Demand signal** for DB queries — the in-Rust chat-context retrieval (`graph/entities.rs:222`) is the
   leading indicator, not yet a strain.
4. **Engine choice for the index slot** — unresolved by the corpus; resolution depends partly on (1).
5. **Schema stability** — the event-sourced core (`ad44`/`4da5`/`9c89`) is open; freeze before migrating.

Decision rule (from `plan-outline.md` §2 Wave 2): SurrealKV green on all three OSes → advance to the
indexed-adapter rewrite, keeping RocksDB off unless SurrealKV fails and RocksDB clearly justifies the
Windows native-link cost; both engines red on any OS → close `2b2c`, keep the file default, keep `kv-mem`
test-only, and revisit only when cross-session recall / team-graph become committed roadmap.

---

## 12. Honest uncertainties and concerns

- **The deciding evidence does not exist.** `2b2c` has produced zero numbers (`Cargo.toml:218` shows
  `kv-mem` only; `sweep-2.md` §5-§6). Any stronger verdict than "gated" over-claims.
- **The two reviews diverge on the eventual engine** (SurrealKV/RocksDB vs SQLite). This design ratifies the
  shared posture and defers the engine to `2b2c` + demand-signal shape rather than manufacturing a pick
  (`plan-outline.md` §0).
- **The surreal spike's O(n)-scan-per-append (`surreal.rs:153-178`) is the single most misleading
  artifact** — it makes Surreal look functionally complete while hiding that no realistic indexed write path
  exists. Any "Surreal works" claim must be qualified to "conforms under `kv-mem` with an O(n) sequence
  assigner."
- **Line-number drift.** The discover docs were written against a slightly earlier tree; this doc cites the
  current-tree locations for the shifted commands.rs items (`choose_materialized_graph` `commands.rs:5206`,
  `materialized_graph_has_content` `:5186`, `export_graph` `:4577`, `save_graph` `:4520`, `load_session`
  `:5224`), verified 2026-06-27. All `projections.rs`, `persistence/mod.rs`, `surreal.rs`, `promotion.rs`,
  `sessions/mod.rs`, `graph/temporal.rs`, and `graph/entities.rs` line numbers were re-verified against the
  working tree.
- **Promotion is contract-only and dormant** — not wired to any Tauri command yet (`commands.rs` has
  negative tests asserting the absence of promotion IPC commands;
  `discover-src_tauri_src_promotion_rs.md` §8). A storage change here has no UI blast radius today, which
  *lowers* the risk of lifting its invariants into the shared gate now (impl plan W1c).
- **Collapsing the two graph generations is a data-model redesign, not a storage choice** — out of scope
  for this decision and would itself require extending the conformance gate
  (`discover-src_tauri_src_graph.md` §8).

---

## 13. References

- ADR: `docs/adr/0021-storage-architecture.md`
- Implementation plan: `docs/plans/storage-architecture-impl-plan-2026-06-27.md`
- Megaloop working set: `docs/designs/_storage-megaloop-2026-06-27/{frame.md, plan-outline.md,
  discover-src_tauri_src_persistence.md, discover-src_tauri_src_graph.md, discover-src_tauri_src_promotion_rs.md,
  sweep-0.md, sweep-1.md, sweep-2.md}`
- Research report: `research/notes/final_report_storage-arch-tauri-temporal-graph-6415ba.md` (+ the cited
  corpus under `research/notes/`)
- Prior spike: `docs/research/surrealdb-embedded-memory-2026-06-26.md`
- Related ADRs: ADR-0007 (feature-gate local ML + Windows native-link precedent), ADR-0014 (on-demand notes
  synthesis — supersession-pending when `0d1c` lands), ADR-0019 (credential/config storage — `serde_yaml`
  archival longevity precedent)
- Key code anchors: `src-tauri/src/persistence/mod.rs` (trait `:424`, `FileMemoryRepository` `:593`,
  `append_jsonl` `:154`, `save_json` `:2328`, conformance `:3165`, promotion file-only tests
  `:4344-4699`), `src-tauri/src/persistence/surreal.rs` (`:76`, `:104`, `:153`, `:167`, `:689`),
  `src-tauri/src/persistence/io.rs` (`:90`), `src-tauri/src/projections.rs`
  (`:31`, `:156`, `:387`, `:564`, `:650`, `:791`, `:830`, `:870`),
  `src-tauri/src/promotion.rs` (`:16`, `:85`, `:248`, `:312`, `:396`),
  `src-tauri/src/sessions/mod.rs` (`:56`), `src-tauri/src/graph/temporal.rs` (`:45`, `:48`),
  `src-tauri/src/graph/entities.rs` (`:222`), `src-tauri/src/commands.rs`
  (`:4520`, `:4577`, `:5186`, `:5206`, `:5224`), `src-tauri/Cargo.toml` (`:40`, `:218`)
