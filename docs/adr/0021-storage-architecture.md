# ADR-0021: Storage Architecture — File-Canonical Event Logs, DB Gated on Evidence

## Status

accepted (gated) — ratified 2026-06-27

This ADR records a **gated** decision, not a go/no-go one. The deciding
evidence for promoting any database to a default store does not exist yet
(seed `audio-graph-2b2c` has produced zero measurements). The honest verdict
is therefore to ratify the cheapest correct posture now and pre-commit the
conditions under which the dial moves — not to assert past the evidence.

## Context

AudioGraph captures transcripts, derives notes and extracted entities, and
maintains a temporal knowledge graph — a local-first Tauri desktop app shipping
Windows, macOS, and Linux. The storage question has four named positions, and a
2026-06-27 research megaloop (`docs/designs/_storage-megaloop-2026-06-27/`) was
run to decide among them:

- **A. SurrealDB-primary** — adopt embedded, file-backed SurrealDB as the
  primary off-the-shelf store.
- **B. Keep custom files** — keep the custom file-based event-sourced
  repository (`FileMemoryRepository`).
- **C. Hybrid** — human-readable files canonical for transparency, plus an
  embedded query database as a rebuildable index.
- **A'. Status-quo baseline** — keep the `LocalMemoryRepository` trait seam, the
  file adapter as default, and any database as a gated, opt-in adapter. This is
  the latent posture the 2026-06-26 spike recommended; the decision can ratify,
  extend, or overturn it.

The single most important architectural fact is that **the abstraction the whole
decision turns on already exists and is conformance-tested.**
`trait LocalMemoryRepository: Send + Sync`
(`src-tauri/src/persistence/mod.rs:424`) is a backend-owned boundary above file
paths. Two adapters implement it and both pass the shared replay-parity gate
`assert_repository_replay_parity_conformance`
(`src-tauri/src/persistence/mod.rs:3165`): the production `FileMemoryRepository`
(`mod.rs:593`) and a feature-gated `SurrealMemoryRepository`
(`surreal.rs:76`, behind `surrealdb-embedded`, off by default). The decision is
therefore **"which adapter is default and what evidence gates promotion,"** not
"whether to introduce a boundary."

Two storage generations coexist on disk and both are live (discover-persistence
§2; discover-graph §1):

- **gen-1 (legacy):** `transcripts/<id>.jsonl` (`TranscriptSegment`) and
  `graphs/<id>.json` (a petgraph snapshot autosaved every 30s, with destructive
  eviction and no event log — not replayable).
- **gen-2 (event-sourced):** append-only `transcripts/<id>.events.jsonl` and
  `projections/<id>.events.jsonl` logs, with materialized
  `notes/<id>.json` and `graphs/<id>.materialized.json` snapshots derived from
  them. On session load both generations are read and reconciled by
  `last_sequence` in `choose_materialized_graph` (`commands.rs:5206`).

Any storage swap must carry both generations.

Current external facts shaping this decision (from the research corpus in
`research/notes/`):

- The app's design is already the convergent local-first pattern that Obsidian,
  Anytype, Dendron, sandbar, sideshowdb, eventfold, and LiveStore independently
  arrive at: human-readable logs canonical, a rebuildable index derived from them
  (final report §4, §8).
- SurrealDB's durable single-node engine, RocksDB, is C++ (clang/libclang/bindgen;
  Windows `/MT`-vs-`/MD`; cross-compile failures) and lands in the same native-link
  minefield ADR-0007 documents (final report §2; sweep-2 §6).
- SurrealKV (pure-Rust) is beta with an unstable on-disk format, defaulted to
  no-fsync at the crate level (engine default repaired to `sync=every` in
  Feb 2026, but the safe mode is the slow mode), and carries a **back-in-time
  versioning caveat** that collides with this app's retroactive-revision model
  (final report §2).
- SurrealDB is the largest bundle of any candidate (~46 MB `.app` vs SQLite ~5 MB
  in a controlled Tauri comparison) and is single-vendor longevity risk; KuzuDB's
  Oct-2025 archival is the empirical proof of that risk class (final report §2, §6).
- The deciding cross-platform build/link/size/durability evidence
  (seed `audio-graph-2b2c`) is **ungathered**: the spike runs `kv-mem` only
  (`Cargo.toml:218`), which provides zero durability evidence (sweep-2 §5; final
  report §2).

## Decision Drivers

The seven fit-criteria the megaloop frame derived from the current code
(frame §4), restated as drivers. A storage option fits only if it satisfies all
of them:

- **O(1)-ish, non-stalling append on the live-capture hot path.** Transcript
  events arrive many-per-second under streaming ASR; the file path is an O(1)
  JSONL append with per-append fsync (`append_jsonl`, `mod.rs:154`) behind a
  bounded, drop-new writer thread (cap 2048). The SurrealDB spike does an O(n)
  full-table scan per append to assign sequences (`next_session_sequence`
  `surreal.rs:153`, `next_global_sequence` `surreal.rs:167`) and would regress this.
- **Cheap whole-session load and in-memory replay.** Data is bounded by design:
  the sessions index caps at 100 (`mod.rs:872`), the graph caps at 1000 nodes /
  5000 edges (`temporal.rs:45,48`), so whole-session load-and-fold stays cheap.
- **Replay-parity with full-state equality**, including exact bitemporal
  `valid_until_ms` on invalidated facts. Replay lives in default trait methods
  over pure-Rust folds, so an adapter only has to make `append_*`/`load_*`
  byte- and order-correct (`mod.rs:3165`; discover-persistence §7).
- **Human-readable, portable transcripts and notes** — an explicit product value.
  An opaque DB file does not satisfy this; even Option A would need a file-export
  path, collapsing it toward C (frame §5; final report §7).
- **Crash, disk-full, and torn-tail recovery at least as good as today** — atomic
  fsync-before-rename (`save_json`, `mod.rs:2328`/`mod.rs:2358`), owner-only perms,
  a bespoke `CAPTURE_STORAGE_FULL` event with debounce (`io.rs:90`), corrupt-index
  quarantine to `*.json.corrupt-<ts>` (`mod.rs:213`), and loud-fail (not silent
  loss) on torn JSONL lines.
- **No cross-platform packaging regression** — no new native toolchain burden,
  acceptable binary-size delta, no per-OS link failures (cf. ADR-0007).
- **Earns its complexity** — adopting a query engine for queries that do not yet
  exist is cost without realized benefit. The only thing resembling a query today
  is hand-rolled top-k RAG over a fully-loaded snapshot
  (`build_graph_chat_context`, `entities.rs:222`).

## Considered Options

### Option A — SurrealDB-Primary (Embedded, File-Backed) As The Default Store

Adopt embedded SurrealDB as the canonical store, replacing the file repository
as the default adapter behind the trait.

**Pros.** One multi-model engine could unify document, graph, vector, and
time-series access. The trait boundary plus the existing `kv-mem` spike mean the
integration path is partly built. A remote/federated sync target is already
anticipated in the promotion subsystem
(`PromotionSyncTargetKind::SurrealdbRemote`, `promotion.rs:86`). The durability
default was repaired to the safest mode in Feb 2026, and embedding SurrealDB in
Tauri is a shipping pattern (final report §7).

**Cons.** As a *primary, canonical* store the case collapses. Durability is
weaker-by-default than the file adapter, and the only mature-durable engine
(RocksDB) is C++ and lands in the ADR-0007 native-link minefield — the `cloud`-only
build still aborts on Windows with `0xC0000139` from `aws-lc-sys`/`ring`, fixed by
a manifest embed rather than dep removal, and RocksDB compounds that class of
problem (sweep-2 §4, §6, §8). SurrealKV (the pure-Rust alternative) is beta with an
unstable on-disk format and a back-in-time versioning caveat that collides head-on
with this app's retroactive-revision model — transcript spans carry
`revision_number`/`supersedes`, and graph `invalidate_*` ops set `valid_until_ms`
*after the fact*; these are literally back-in-time inserts (final report §2, §3).
SurrealDB is the largest bundle of any candidate and is single-vendor longevity
risk. An opaque DB file fails the human-readability requirement, so even A needs a
file-export path, collapsing it toward C. And **none of SurrealDB's distinctive
value is exercised by current code** — the spike stores opaque `serde_json::Value`
in SCHEMALESS tables (`surreal.rs:104`), used as a KV blob bag, with an O(n)
full-table-scan sequence assigner (`surreal.rs:153`/`167`) that fails fit-criterion
#1. "Surreal works" is only true as "conforms under `kv-mem` with an O(n) assigner."

### Option B — Keep The Custom File-Based Event-Sourced Repository

Keep `FileMemoryRepository` as the only default and only selectable store. It is
shipped, hardened, and passes the conformance gate.

**Pros.** It already implements the convergent local-first pattern (event logs
canonical, rebuildable projections). Append is O(1) JSONL on the hot path
(`append_jsonl`, `mod.rs:154`); durability is by-default (per-append fsync, atomic
fsync-before-rename, `mod.rs:2358`); the disk-full UX is bespoke and product-visible
(`io.rs:90`); transcripts and notes are human-readable, greppable, and recoverable
by hand; it passes the shared conformance gate (`mod.rs:3165`); and bounded data
(1000/5000 caps, 100-session index) keeps whole-session load-and-replay cheap,
removing the scale argument for a query engine *for present workloads*.

**Cons.** It offers no indexed queries — all traversal and relevance retrieval is
hand-rolled Rust over fully-loaded snapshots (`build_graph_chat_context`,
`entities.rs:222`). The graph caps that keep replay cheap are also a ceiling. The
agent-era critique applies: as people try to make files agent-native, they usually
reinvent a worse database (final report §8, citing "don't build a second brain,
build a database") — so deferring a database postpones the query work, it does not
abolish it.

### Option C — Hybrid: File-Canonical Logs + Rebuildable Embedded Query Index

Keep the human-readable event logs as the source of truth and add an embedded
database as a *derived, rebuildable* index behind the same trait, with the logs
authoritative.

**Pros.** Indexed traversal, full-text search, and vector recall without
sacrificing readability. Because the index is rebuildable from the logs, a bad
engine choice is a rebuild, not a data-loss event — which is precisely why the
switching cost is small and reversible (final report §9, §10). It fits behind the
existing trait. The strongest pro-database argument in the corpus, followed
honestly, lands *here* (file-canonical C with the DB underneath) rather than on A.

**Cons.** Dual-write / dual-maintenance complexity that is unjustified until a
concrete demand signal appears. The engine for the index slot is itself unresolved
by the corpus — the two decision documents disagree (SurrealKV/RocksDB vs SQLite),
so committing to one blind would be guessing.

### Option A' — Status-Quo Baseline (Trait Seam + File Default + DB As Gated Opt-In)

Keep the `LocalMemoryRepository` trait seam, keep `FileMemoryRepository` as the
default, and hold the `kv-mem` `SurrealMemoryRepository` exactly where it is —
a feature-gated conformance target, off by default, out of the default CI path.
Do not add runtime selectability, a file-backed engine, or migration tooling.

**Pros.** The cheapest correct posture available today. The abstraction already
exists and is conformance-tested, so this posture costs nothing new while
preserving full optionality — the dial can move later without a rewrite. It keeps
the event log as the friendliest substrate for any future sync.

**Cons.** It is a holding posture, not a destination: the DB upside stays deferred
and the query work stays postponed-not-avoided. The gate must actually be run for
the dial to advance; "gated forever" would be indistinguishable from never deciding.

## Decision Outcome

**Set the dial to B now; hold A' as the formal posture; reject A as the default;
pre-commit to a gated move toward file-canonical C on a named demand signal; and
gate the index-engine choice on seed `audio-graph-2b2c`.**

Concretely:

- **B is the active default.** `FileMemoryRepository` is the only default and only
  selectable store. It is shipped, hardened, and passes the conformance gate.
- **A' is the standing posture.** Keep the trait seam; keep the `kv-mem`
  `SurrealMemoryRepository` as a feature-gated conformance target, off by default.
  Do not add runtime selectability, a file-backed engine, or migration tooling
  outside the gate.
- **A is rejected as the default store today** — not as a permanent foreclosure,
  but as the default-store choice. SurrealDB is retained only as a gated, opt-in
  adapter for a future remote/federated path.
- **C (file-canonical) is the pre-committed next step**, gated on a named demand
  signal, with the **index engine itself left as a gated sub-decision** because the
  corpus does not resolve SurrealKV vs SQLite vs redb.

**The verdict is gated, not go/no-go, because the deciding evidence is absent.**
The single thing that turns "gated" into a clean direction is `audio-graph-2b2c`:
cross-platform build/link/size/packaging plus durability evidence for the
file-backed engines (`kv-surrealkv` and `kv-rocksdb`) on Linux, macOS, and Windows.
It has produced zero evidence and is one CI run from existing. Until it runs, a
clean "lean in" or "remove it" verdict asserts past the evidence (review §4; final
report §10).

### Why "gated-on-evidence" is the honest verdict

The task mandated honesty over a manufactured pick. Two independent decision
documents in the corpus converge on the *posture* but **diverge on the eventual
DB engine**, and the deciding evidence for both is missing:

- `surrealdb-revisit-2026-06-27.md` → "HYBRID, gated on `2b2c`," with
  SurrealKV/RocksDB as the future engine.
- `final_report_storage-arch-tauri-temporal-graph-6415ba.md` → "B-default +
  A'-posture + gated move to file-canonical C with SQLite," explicitly rejecting
  SurrealDB-primary and naming KuzuDB's Oct-2025 archival and sled's instability as
  proof of the single-vendor bus-factor risk that also attaches to SurrealDB.

That divergence is unresolved by current evidence: no compiled file engine, no
binary-size numbers, no streaming-frequency throughput benchmark, and no demand
signal beyond the existing in-Rust chat-context retrieval. The disciplined move is
to ratify the low-cost posture both agree on and gate the engine choice on the
cheap experiment that would discriminate between them — not to pick SurrealKV or
SQLite blind.

### Decision rationale, evidence-backed

- **The option's carrying cost is near-zero; the upside is real but deferred.**
  The trait the whole decision turns on already exists (`mod.rs:424`) with two
  conformance-passing adapters (`mod.rs:3165`). The decision is which adapter is
  default and what gates promotion.
- **B fully serves today's workload.** O(1) JSONL append on the hot path
  (`append_jsonl`, `mod.rs:154`); bounded data (`mod.rs:872`; `temporal.rs:45,48`)
  keeps load-and-replay cheap; human-readable/portable transcripts and notes are an
  explicit product requirement files satisfy for free; durability and the disk-full
  UX are bespoke and product-visible (`save_json` fsync-before-rename
  `mod.rs:2358`; `CAPTURE_STORAGE_FULL` `io.rs:90`; corrupt-index quarantine
  `mod.rs:213`).
- **The DB upside is unproven and partly mis-built.** The spike runs `kv-mem` only
  (zero durability evidence, `Cargo.toml:218`); stores opaque `serde_json::Value`
  in SCHEMALESS tables (`surreal.rs:104`), exercising none of SurrealDB's
  graph/vector/live-query value; and does an O(n) full-table scan per append
  (`surreal.rs:153`/`167`) that fails fit-criterion #1. Every DB-justifying workload
  (cross-session recall, promotion-lineage, team graph, vector recall) is
  future/aspirational; the only thing resembling a query today is in-Rust top-k RAG
  (`entities.rs:222`) — the leading indicator, not yet a strain.
- **Engine-level facts argue against SurrealDB as primary.** RocksDB is C++ and
  lands in the ADR-0007 minefield; SurrealKV is beta with an unstable on-disk format
  and a back-in-time caveat that collides with retroactive revision; SurrealDB is the
  largest bundle and single-vendor risk (final report §2, §6; sweep-2 §6).
- **Migration economics are asymmetric.** B pays nothing. A pays the full migration
  plus a data-loss risk on a bad bet against a still-moving schema. C pays a small,
  reversible cost because a derived index is rebuildable from the authoritative logs
  (final report §10). Migrating now means migrating twice.

### Relation to other ADRs

- **ADR-0014 (on-demand notes synthesis)** is **supersession-pending**: the notes it
  describes are now an event-sourced projection. The supersession lands with seed
  `0d1c`; this ADR records the relation but does not itself supersede ADR-0014.
- **ADR-0019 (credential and config storage)** establishes the same storage posture
  this ADR extends — a backend-owned facade/trait, a default backend, and gated/opt-in
  alternatives behind it — and provides the **`serde_yaml`-archived longevity
  precedent** that justifies weighting single-vendor bus-factor risk (KuzuDB's
  archival, SurrealDB's single-vendor exposure) as a first-class driver here.
- **ADR-0007 (feature-gate local ML)** provides both the **feature-gate pattern**
  (`surrealdb-embedded` is off by default, exactly as `local-ml` deps are gated) and
  the **Windows native-link `0xC0000139` precedent**: the test-harness abort comes
  from `aws-lc-sys`/`ring`, not the ML libs, and is fixed by a manifest embed, not
  dep removal. RocksDB would compound that class of problem.

## Consequences

Positive:

- Zero new carrying cost — the default, the trait, and the spike are unchanged.
- Optionality is preserved; the dial can move to C without a rewrite because the
  index is a rebuildable projection of canonical logs.
- The append-only event log is the friendliest substrate for any future
  multi-device sync — change streams merge cleanly where snapshots do not.
- Human-readable, portable transcripts and notes are retained as a hard product
  guarantee.

Negative:

- The DB upside (indexed traversal, FTS, vector recall) stays deferred.
- The query work is postponed, not avoided — when chat-context retrieval strains the
  loaded-snapshot approach, the index must be built.
- The decision only advances if the gate (`2b2c`) is actually run; a holding posture
  that is never re-evaluated would be a failure mode in itself.

Neutral:

- The `kv-mem` spike remains in-tree as a conformance target, documenting the seam
  without claiming a production path.

## Implementation Outline

The work splits into **no-regret now** (independent of the gate) and **gated**
(blocked on `2b2c` plus a demand signal). No wave in this outline writes production
code as part of ratifying this ADR.

1. **Wave 0 — Ratify and record (no code).** Land this ADR, the design doc
   (`docs/designs/storage-architecture-decision-2026-06-27.md`), and the impl plan
   (`docs/plans/storage-architecture-impl-plan-2026-06-27.md`); update the ADR README;
   note the ADR-0014 supersession-pending and the ADR-0019 relation.

2. **Wave 1 — No-regret refactors (B stays default, A' enforced; gate-independent).**
   - Thread `Arc<dyn LocalMemoryRepository>` through the ~15 hard-wired call sites
     (`commands.rs` 2582/2828/2922/2943/4989/5236/8583/8752/9209; `state.rs`
     403/712/1224/1423/1488; `speech/mod.rs:731`) — a DI prerequisite for any
     non-default adapter, with no behavior change.
   - Hoist shared validation/privacy gates into trait default methods
     (`ensure_org_visible_record_is_safe` `mod.rs:240`, `validate_live_assist_card`
     `mod.rs:292`, per-type `.validate()`), removing the file/surreal duplication so a
     third adapter cannot bypass them.
   - Extend the shared conformance gate to cover promotion/redaction/org-knowledge,
     revoke guards, silent-source-mutation guard, and the privacy floor (today
     file-adapter-only `#[test]`s) so promotion parity is adapter-checked.
   - (Optional) Add a `[profile.release]` baseline and size tracking so any future
     storage-dep delta is measurable (there is no LTO/strip mitigation today,
     sweep-2 §9).

3. **Wave 2 — The unblocking experiment, `audio-graph-2b2c` (THE GATE).** On a
   throwaway branch wired into nothing: add `kv-surrealkv` and `kv-rocksdb` as alt
   features; build `--release` on Blacksmith Linux+macOS+Windows; record build/link
   time, stripped binary-size delta vs baseline, native-dep inventory; `tauri build`
   per OS per engine (installer size + does it link/launch); run a short durability
   probe (write N sessions, `kill -9` mid-write, reopen, confirm recovery; corrupt the
   store, confirm failure mode). Schema-independent — can run today.

4. **Wave 3 — Indexed adapter rewrite + benchmark (GATED on Wave 2 green + a demand
   signal).** Rewrite the chosen adapter to use indexed/keyed access (drop the O(n)
   full-table-scan sequence assigner, `surreal.rs:153`–`178`); benchmark against the
   O(1) JSONL append at streaming transcript frequency; make it
   selectable-but-not-default.

5. **Wave 4 — File-canonical hybrid index + migration/dual-read (GATED).** Stand up
   the rebuildable index as a *derived* projection of the canonical logs (logs
   authoritative); dual-read/backfill from both generations with rollback; wire the
   cross-session recall workload and/or `build_graph_chat_context` onto it.

Sequencing guardrails: freeze the event-sourced core schema before any DB migration
so it migrates once, not twice; `2b2c` is schema-independent and can run in parallel
with Wave 1; never make a DB default until `2b2c` is green **and** a demand signal is
committed.

## Rollback

Rollback is trivial because B is never modified. The default file adapter is the
status quo and stays in force throughout. All gated work is opt-in behind the
`surrealdb-embedded` feature, off by default, and reversible: because any future
index is a *rebuildable* projection of the canonical logs, backing it out is "drop
the index," not a data-loss migration. If the `2b2c` gate comes back red on any OS
(Windows link failure, large size regression, weak recovery), close `2b2c`, keep the
`kv-mem` adapter test-only, and the file default is unchanged.

## Acceptance Criteria

- The three megaloop artifacts (this ADR, the design doc, the impl plan) agree on the
  recommended posture, the gates, and the data model.
- Every load-bearing claim in this ADR cites either a code location or a corpus source;
  unknowns are named as gates, not papered over.
- The default build, the existing test suite, and the conformance gate
  (`mod.rs:3165`) are unchanged by ratification (Wave 0 writes no production code).
- **`2b2c` decision rule:** if `kv-surrealkv` is green on all three OSes (clean
  pure-Rust link, acceptable size, sane recovery), advance Wave 3 with an indexed
  rewrite; if both engines are red on any OS, close `2b2c` and keep the file default.
  Resolve the engine sub-decision here on measured numbers: SurrealKV if the demand is
  graph/vector/sync and it is green; SQLite (final report §6) if the demand is
  FTS/recursive-CTE traversal at bounded scale.
- Any future non-default adapter must pass the *extended* conformance gate with
  full-state equality including exact `valid_until_ms`, demonstrate streaming-frequency
  append throughput no worse than the file path, re-implement disk-full surfacing /
  backup / export parity, and prove back-in-time/retroactive-revision correctness
  before it can be made selectable.

## References

- Design doc (detailed companion to this ADR):
  `docs/designs/storage-architecture-decision-2026-06-27.md`
- Implementation plan: `docs/plans/storage-architecture-impl-plan-2026-06-27.md`
- Megaloop frame and plan: `docs/designs/_storage-megaloop-2026-06-27/frame.md`,
  `docs/designs/_storage-megaloop-2026-06-27/plan-outline.md`
- Code discovery: `docs/designs/_storage-megaloop-2026-06-27/discover-src_tauri_src_persistence.md`,
  `docs/designs/_storage-megaloop-2026-06-27/discover-src_tauri_src_graph.md`,
  `docs/designs/_storage-megaloop-2026-06-27/discover-src_tauri_src_promotion_rs.md`,
  `docs/designs/_storage-megaloop-2026-06-27/sweep-2.md`
- Research report: `research/notes/final_report_storage-arch-tauri-temporal-graph-6415ba.md`
- Trait + adapters: `src-tauri/src/persistence/mod.rs:424` (trait),
  `mod.rs:593` (file adapter), `mod.rs:3165` (conformance gate),
  `src-tauri/src/persistence/surreal.rs:76` (spike adapter)
- Hot-path / durability: `src-tauri/src/persistence/mod.rs:154` (`append_jsonl`),
  `mod.rs:2328`/`mod.rs:2358` (`save_json` fsync-before-rename),
  `mod.rs:213` (corrupt-index quarantine),
  `src-tauri/src/persistence/io.rs:90` (disk-full surfacing)
- Spike red flags: `src-tauri/src/persistence/surreal.rs:104` (SCHEMALESS blob),
  `surreal.rs:153`/`surreal.rs:167` (O(n) per-append sequence scan)
- Bounded data: `src-tauri/src/persistence/mod.rs:872` (sessions cap 100),
  `src-tauri/src/graph/temporal.rs:45`/`temporal.rs:48` (graph caps)
- Query surface: `src-tauri/src/graph/entities.rs:222` (`build_graph_chat_context`),
  `src-tauri/src/commands.rs:4577` (`export_graph`)
- Sync/export targets: `src-tauri/src/promotion.rs:86` (`SurrealdbRemote`),
  `promotion.rs:88` (`FileExport`)
- Feature gate / dep: `src-tauri/Cargo.toml:40` (`surrealdb-embedded`),
  `Cargo.toml:218` (`surrealdb 3.1.4`, `kv-mem` only)
- Related ADRs: ADR-0007 (feature-gate + Windows native-link `0xC0000139` precedent),
  ADR-0014 (notes synthesis; supersession-pending via `0d1c`),
  ADR-0019 (storage posture; `serde_yaml`-archived longevity precedent)
- Seeds: `audio-graph-2b2c` (the blocking gate), `48bb` (indexed rewrite),
  `9c89`/`ceda` (migration + recall), `0d1c` (ADR-0014 supersession)
