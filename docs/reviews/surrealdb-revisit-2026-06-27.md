# SurrealDB Revisit: Lean In, Keep Custom, or Hybrid?

Date: 2026-06-27
Audit: docs/reviews/_audit-2026-06-27
Author: storage/persistence decision review (subagent)

## TL;DR

**Recommendation: HYBRID, gated on evidence.** Keep `FileMemoryRepository` as the
shipped default. Keep the SurrealDB `kv-mem` adapter exactly where it is — a
feature-gated, non-default conformance target behind the `LocalMemoryRepository`
trait. Do **not** make SurrealDB selectable or default, and do **not** rip it out.
The decision to lean in is genuinely blocked on evidence that has **not been
gathered** (seed `audio-graph-2b2c`), so the honest verdict is "gated," not a
clean go/no-go. The cheapest unblock is a one-shot CI matrix that compiles the two
file-backed engines and measures build time, binary size, native deps, and
packaging on Linux/macOS/Windows. Until that exists, anyone claiming SurrealDB is
"safe to default" or "must be removed" is asserting past the evidence.

The architecture is already correct for keeping the option open at near-zero
carrying cost. The expensive question (file-backed engine packaging) is the only
one that matters now, and it is one CI run away from being answerable.

---

## 1. Current state of both paths

### 1.1 The custom path (`FileMemoryRepository`) — shipped, mature

The custom path is not a sketch; it is the product. The event-sourced model is
substantially built and the repository abstraction is real:

- `LocalMemoryRepository` trait (`src-tauri/src/persistence/mod.rs`) is the
  backend-owned boundary above file paths: session index, transcript events,
  projection patches, materialized notes/graph, live-assist cards, and the full
  promotion/redaction/org-knowledge surface.
- `FileMemoryRepository` implements all of it over the current artifact layout:
  `sessions.json`, `transcripts/<id>.events.jsonl`, `projections/<id>.events.jsonl`,
  `notes/<id>.json`, `graphs/<id>.materialized.json`, plus `live_assist/` and
  `promotions/` JSONL/JSON.
- Durability is hardened: per-session async writer threads, bounded
  `sync_channel` queues (capacity 2048, drop-new-on-full), `fsync` on the
  synchronous append path, owner-only file permissions, ENOSPC classification
  with a single debounced UI banner, corrupt-index backup-and-continue, and a
  poisoned-writer-lock fail-closed path (`audio-graph-93fc`) so the
  `TranscriptLedger` cannot advance without a proven durable append.
- Replay is the source of truth: `replay_transcript_ledger`,
  `replay_projection_state`, and historical basis validation (`audio-graph-ff70`)
  reconstruct canonical notes/graph from event logs; `load_session` repairs
  in-memory materialized state from replay when artifacts are missing/stale.

This path has shipped in release builds and installers (per recent deep-work-log
entries). It is the known-good baseline.

### 1.2 The SurrealDB path — a verified shape, an unproven engine

What actually exists in this checkout:

- `Cargo.toml`: `surrealdb = { version = "3.1.4", default-features = false,
  features = ["kv-mem"], optional = true }` behind feature `surrealdb-embedded`.
  Resolved stable 3.1.5. **Only the in-memory engine is compiled.**
- `src-tauri/src/persistence/surreal.rs`: `SurrealMemoryRepository` implements the
  full `LocalMemoryRepository` trait against schemaless SurrealDB tables, bridging
  the synchronous trait to the async SDK with a private current-thread Tokio
  runtime and a write mutex for deterministic sequence assignment.
- It passes the shared replay-parity conformance suite
  (`assert_repository_replay_parity_conformance`) and has artifact-descriptor and
  delete tests. It is provably a drop-in for the trait contract under `kv-mem`.

What does **not** exist:

- No file-backed engine anywhere. `grep` for `kv-rocksdb` / `kv-surrealkv` /
  `rocksdb` across `Cargo.toml` and `bun.lock` returns nothing.
- No CI job builds even the `kv-mem` `surrealdb-embedded` feature (`grep surrealdb
  .github/` is empty). The feature is exercised only by local `cargo test`.
- No runtime selectability: nothing in Settings/state lets a user choose SurrealDB.
- No migration/dual-read tooling from existing JSONL/JSON artifacts.

### 1.3 The seed dependency chain (intact, one gate left)

`5dde` (spike, closed) → `5679` (file adapter + trait, closed) → `965b`
(conformance suite, closed) → `48bb` (kv-mem adapter, **partial**, blocked on
`2b2c`) → `2b2c` (**open, unstarted** — the file-engine Blacksmith eval) → `ceda`
(memory-workspace UX, blocked on `48bb`). The supporting work — writer routing
(`f2b6`), DB artifact/delete semantics (`ff32`), bounded queues (`3a09`), UI
backpressure (`24dc`) — is all closed. **`2b2c` is the sole remaining gate, and it
has produced zero evidence.**

---

## 2. What each path buys

### Custom (FileMemoryRepository)
- Zero new native dependencies; nothing added to packaging or attack surface.
- Human-inspectable, append-only artifacts — trivial to recover, diff, back up
  (copy the directory), and reason about for privacy/redaction audits.
- Already shipped, hardened, and tested across the providers and replay fixtures.
- Per-file blast radius: a corrupt `notes/<id>.json` loses one session's notes,
  not the database.

### SurrealDB (if file-backed engine proves out)
- One queryable store for the entities the signals repeatedly describe as
  graph-shaped: speaker↔span joins (`asr-streaming` signals 1fbd/5011/56da),
  promotion lineage/conflict/ACL graphs (`security-privacy` signals e793/053f),
  cross-session recall (`ceda`), and team/shared-graph governance (`5b2a`).
- Native indexed queries replace the current `SELECT *`-then-filter-in-Rust loads,
  which would matter once cross-session memory and recall (the `ceda` workspace)
  arrive and the working set exceeds one session.
- Live queries could eventually back UI subscriptions.
- Optional vector search for semantic recall without a second dependency.

The catch: **every one of these benefits is a future/aspirational feature
(`ceda`, `5b2a`, semantic recall), not a current product requirement.** Today's
shipping workload — single-session, append-then-replay, file-per-session — is
served fully by the file path. SurrealDB's leverage is real but deferred.

---

## 3. The real costs

### 3.1 Binary size, build/link time, native deps — UNMEASURED
This is the crux and it is **not yet measured for the engines that matter.**
- `kv-mem` is pure-Rust and compiles, but it is never the production engine — it
  has no durability.
- `kv-surrealkv` (pure-Rust LSM) vs `kv-rocksdb` (C++ RocksDB, bindgen/clang,
  known Windows/dev friction per the 5dde research and the spike's own remaining
  notes) have materially different cost profiles, and **neither has been compiled
  in this repo.** SurrealDB pulls a large dependency tree even before the engine;
  the spike itself flagged "a heavy compile/link surface." The seed authors were
  right to gate on this.
- Past CI pain is on record (`docs/ops/windows-rust-test-crt-skew.md`, the
  `local-ml` native-link aborts that forced the cloud-only test build per
  ADR-0007). RocksDB would land in exactly this minefield on Windows.

### 3.2 Packaging on Win/mac/Linux — UNTESTED
No Tauri bundle has ever been produced with the feature on. Whether the
file-backed engine links cleanly in the release bundler on all three OSes, and how
much it adds to the installer, is unknown. This is precisely `2b2c`'s acceptance.

### 3.3 Corruption / backup
- File path: per-session blast radius; backup is a directory copy; corrupt files
  are backed up and skipped. Excellent for a local-first desktop app.
- SurrealDB file engine: single-store blast radius unless carefully partitioned;
  backup/restore and corruption-recovery semantics are **unverified** and are an
  explicit `2b2c` acceptance item. For a desktop app where the user *is* the DBA,
  this is a genuine risk, not a checkbox.

### 3.4 Query-model fit for the temporal graph — good in theory, mis-implemented today
The temporal graph (valid_from/valid_until facts, retcon merge/split/invalidate,
provenance edges) maps cleanly onto SurrealDB's document+graph+`RELATE` model.
**But the current `kv-mem` adapter does not use any of it.** `surreal.rs` stores
typed JSON envelopes in `SCHEMALESS` tables and, on every append and load, does
`db.select(table)` (full-table scan) then filters by `session_id` in Rust
(`select_all` → `.retain()` → `.sort_by_key()`), recomputing the next sequence by
scanning all records. That is O(n) per write at streaming transcript frequency —
the exact `4da5` workload the signals call "the primary read/write workload any
SurrealDB adapter must handle at streaming frequency." As written, the adapter
would **regress** write performance versus an `O(1)` JSONL append, and it uses
SurrealDB as a slow document bag, not as a graph engine. The graph-query upside is
real only after a schema/indexing rewrite that does not exist yet. This is a
strong reason **not** to lean in on the current implementation, independent of the
packaging gate.

### 3.5 Toolchain / version exposure
SurrealDB 3.x requires Rust 1.89+ (the repo already builds newer). The crate moves
fast (3.1.5 stable, 3.2 beta); pinning and churn become a maintenance tax once it
is on the critical path. Low but non-zero.

---

## 4. What evidence is still missing (the honest gaps)

1. **`2b2c` has produced nothing.** Build/link time, binary size delta, native-dep
   inventory, corruption/backup behavior, and Tauri packaging for `kv-surrealkv`
   **and** `kv-rocksdb` on Blacksmith Linux/macOS/Windows are all unmeasured. This
   is the blocking evidence and it does not exist.
2. **No streaming-frequency performance data** for either repository under a
   realistic transcript-revision write rate. The conformance suite proves
   correctness, not throughput; the O(n) adapter pattern is unbenchmarked.
3. **No migration/dual-read design** for existing artifacts (`9c89` owns this and
   is itself blocked on `ad44`/`4da5`).
4. **The event-sourced core (`ad44`) is not closed.** Durable speaker/diarization
   basis, ahead-of-log materialized-artifact reconciliation, and export bundling
   are open. The schema SurrealDB would store is **still moving**; storing it now
   means migrating it twice.

Because (1) is entirely absent, a clean "lean in" verdict is unsupported by
evidence, and a "keep custom forever / remove SurrealDB" verdict is unsupported by
evidence too — the carrying cost of the current option is low and the upside is
plausible. The defensible position is HYBRID-gated.

---

## 5. Recommendation and next steps

### Decision: HYBRID, gated on `2b2c`
- **Ship:** `FileMemoryRepository` stays the only default and selectable store.
- **Keep:** the `kv-mem` `SurrealMemoryRepository` as a feature-gated conformance
  target. It is cheap to carry (opt-in feature, not in the default build, not in
  CI's default path) and it keeps the trait honest. Removing it would discard the
  proven shape for no benefit; promoting it would be reckless without `2b2c`.
- **Do not** add Settings/runtime selectability, file-backed engines, or migration
  tooling until the gate is green.

### Cheapest experiment to unblock (do this first — it is one CI run)
Add a throwaway CI matrix job (Blacksmith, Linux+macOS+Windows) that, **without
wiring anything into the product**:
1. Adds `kv-surrealkv` and `kv-rocksdb` as alternative features in a scratch
   branch and runs `cargo build --release --features surrealdb-embedded,<engine>`.
2. Records: wall-clock build/link time delta, stripped binary size delta vs
   baseline, and the native toolchain each engine demands (clang/bindgen/libc++
   for RocksDB; pure-Rust for SurrealKV).
3. Runs `tauri build` once per OS per engine and records installer size + whether
   the bundle links and launches.
4. A 10-minute durability probe: write N sessions, `kill -9` mid-write, reopen,
   confirm recovery; then corrupt the store and confirm the failure mode.

Expected outcome and the actual decision rule:
- If **SurrealKV** is green on all three OSes (clean pure-Rust link, acceptable
  size, sane recovery): then **and only then** advance `48bb` to (a) rewrite the
  adapter to use indexed/keyed access instead of full-table scans, (b) benchmark
  it against the file path at streaming frequency, and (c) make it
  *selectable-but-not-default* for the multi-session recall feature (`ceda`). Keep
  RocksDB off unless SurrealKV fails and RocksDB's size/perf clearly justifies the
  Windows native-link cost.
- If **both engines** are red on any OS (Windows link failure, large size
  regression, or weak recovery): close `2b2c` as "file path stays default," keep
  `kv-mem` as a test-only target, and revisit only if/when `ceda`/`5b2a`
  cross-session/team features become committed roadmap rather than design seeds.

### Sequencing guardrail
Land the event-sourced core first. `ad44`/`4da5`/`9c89` should close (or at least
freeze the transcript/projection/diarization schema) **before** any SurrealDB
migration work, so the schema is migrated once, not twice. The packaging
experiment (`2b2c`) is schema-independent and can run in parallel today — it only
touches build/link/packaging, not the data model.

---

## 6. Why not the other two verdicts

- **Lean in now:** unsupported. The production engine (`kv-rocksdb`/`kv-surrealkv`)
  has never been compiled or packaged here; the current adapter is O(n)-per-write
  and uses none of SurrealDB's graph strengths; the schema it would store is still
  in flux. Leaning in trades a hardened, shipped store for unmeasured packaging
  risk and a guaranteed near-term migration.
- **Keep custom / remove SurrealDB:** also unsupported as a *permanent* call. The
  cross-session recall (`ceda`), promotion-lineage (`053f`/`e793`), and team-graph
  (`5b2a`) workloads are genuinely graph-shaped, and the option is carried at
  near-zero cost behind a clean trait. Deleting it now would only have to be
  rebuilt later. Keeping custom *as default* is correct; foreclosing SurrealDB is
  not.

## References
- `src-tauri/src/persistence/mod.rs` (trait + `FileMemoryRepository`)
- `src-tauri/src/persistence/surreal.rs` (kv-mem adapter; O(n) scan pattern)
- `src-tauri/Cargo.toml` lines 40, 216-218 (feature + kv-mem-only dep)
- `docs/research/surrealdb-embedded-memory-2026-06-26.md` (prior spike)
- `docs/adr/0014-notes-synthesis.md` (to be superseded by `0d1c`)
- `docs/adr/0019-credential-and-config-storage.md` (storage-decision posture)
- Seeds: `audio-graph-2b2c` (the gate), `audio-graph-48bb`, `audio-graph-ad44`,
  `audio-graph-4da5`, `audio-graph-9c89`, `audio-graph-0d1c`, `audio-graph-ceda`,
  `audio-graph-5b2a`
- Cross-cluster `surrealdb_signals`: data-architecture, security-privacy,
  asr-streaming, ux-settings, ci-testing digests in `_audit-2026-06-27/`
