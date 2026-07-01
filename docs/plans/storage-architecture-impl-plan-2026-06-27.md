# Storage Architecture — Implementation Plan (Dependency-Ordered Waves)

Date: 2026-06-27
Working dir of the deciding megaloop: `docs/designs/_storage-megaloop-2026-06-27/`
Companion artifacts (Wave 0 lands all three together):
- ADR: `docs/adr/0021-storage-architecture.md` (MADR 3.0, Status: proposed) — *to be authored in W0*
- Design doc: `docs/designs/storage-architecture-decision-2026-06-27.md` — *to be authored in W0*
- This plan: `docs/plans/storage-architecture-impl-plan-2026-06-27.md`

Scope of THIS document: the dependency-ordered work plan. **No wave writes production code as
part of the run that produces this plan** — the plan *describes* the work, its dependencies, its
acceptance criteria, and the seed IDs that carry it. The decision itself is recorded in the ADR;
this plan operationalizes it.

---

## 0. The decision this plan implements (one paragraph, for self-containment)

**Gated-on-evidence.** Ratify the latent **A'** posture — keep the existing
`LocalMemoryRepository` trait seam (`src-tauri/src/persistence/mod.rs:424`) with the file adapter
(`FileMemoryRepository`, `mod.rs:593`) as the only default and only selectable store — and **block**
any move to a database default on one named, cheap experiment (seed `audio-graph-2b2c`). Set the
dial to **B** (file-canonical event logs) now; hold **A'** as the formal posture; **reject A**
(SurrealDB-primary) as the default (not as a permanent foreclosure); pre-commit to a gated move
toward **file-canonical C** (a rebuildable embedded index, logs authoritative) on a *named demand
signal*, with the index engine left as a sub-decision (SurrealKV vs SQLite vs redb) that the corpus
does not resolve. The single thing that turns "gated" into "go/no-go" is `2b2c`, which has produced
zero evidence to date (`kv-mem` only — `Cargo.toml:218`; surreal spike never writes a file,
discover-persistence §10). See `plan-outline.md §0` and `final_report_storage-arch-tauri-temporal-graph-6415ba.md §10`.

This plan splits into **no-regret now** (W0–W1, independent of the gate) and **gated** (W2+, blocked
on `2b2c` and on a committed demand signal).

---

## 1. Evidence basis and how to read the citations

Two source classes are cited throughout, and both are required for a claim to be "load-bearing":

- **Corpus** — the deciding megaloop notes and the hyperresearch report, e.g.
  `final_report_storage-arch-tauri-temporal-graph-6415ba.md §N` (the synthesized report under
  `research/notes/`), `frame.md §N`, `plan-outline.md §N`, `discover-persistence §N`
  (`discover-src_tauri_src_persistence.md`), `discover-graph §N` (`discover-src_tauri_src_graph.md`),
  `discover-promotion §N` (`discover-src_tauri_src_promotion_rs.md`), `sweep-2 §N`.
- **Code** — `file:line` against the 2026-06-27 working tree, verified while authoring this plan.

Where evidence is absent, the gap is named as a gate, not papered over (frame §6; this is an
explicit honesty requirement of the deciding frame, §7 "Evidence honesty").

**Line-number note.** The `plan-outline.md` cited approximate call-site lines (e.g. `commands.rs:2570`).
The actual lines were re-verified for this plan and differ by a few lines; the **verified** values are
used below. Treat any line number as a pointer to re-confirm at implementation time, not a frozen
address (the tree is dirty — see the git status — and these files are under active edit).

---

## 2. Seed map (proposed IDs, with verified dependency edges from `.seeds/issues.jsonl`)

| Seed | Title (verbatim) | Status | Edge | Wave |
|---|---|---|---|---|
| `audio-graph-5dde` | Architecture spike: SurrealDB 3.x embedded memory and federated org knowledge | closed | — | (history) |
| `audio-graph-5679` | LocalMemoryRepository trait and file-backed adapter | closed | blocks 965b | (history; W1 extends) |
| `audio-graph-965b` | Repository replay parity tests for transcript and projection state | closed | blockedBy 5679 | W1c extends |
| `audio-graph-2b2c` | Evaluate SurrealDB file-backed engines on Blacksmith before storage selectability | open | blocks 48bb | **W2 (THE GATE)** |
| `audio-graph-48bb` | SurrealDB embedded local memory adapter spike | open | blockedBy 2b2c; blocks ceda | **W3** |
| `audio-graph-ceda` | Architecture session: cross-session meeting memory workspace and recall UX | open | blockedBy 48bb | **W4 (demand signal)** |
| `audio-graph-9c89` | Session artifact migration for transcript and projection events | open | blockedBy 4da5, ad44 | **W4 (migration)** |
| `audio-graph-ad44` | Event-sourced transcript/notes/graph synthesis data model | open | blocks 4da5, 9c89, 0d1c, … | schema freeze (guardrail) |
| `audio-graph-4da5` | Transcript revision ledger and canonical span projection | open | blockedBy ad44; blocks 9c89 | schema freeze (guardrail) |
| `audio-graph-0d1c` | Supersede ADR-0014 with event-sourced notes and graph projection architecture | open | blockedBy ad44 | ADR-0014 supersession (W0 note) |

**New seeds proposed by this plan** (no existing ID — to be filed in W0 alongside the artifacts):
- `storage-decision` (W0) — supersede-aware ratification seed that lands the three artifacts.
- `repo-di-threading` (W1a) — thread `Arc<dyn LocalMemoryRepository>` through call sites; extends the
  `5679` lineage.
- `repo-shared-gates` (W1b) — hoist validation/privacy gates into trait defaults.
- `conformance-promotion` (W1c) — extend the shared gate to promotion/privacy; extends `965b` lineage.
- `release-size-baseline` (W1d, optional) — `[profile.release]` baseline + per-OS size tracking.

> The verified seed-graph fact that matters most: **`2b2c` blocks `48bb` blocks `ceda`.** That is the
> exact W2 → W3 → W4 spine this plan follows. The schema chain `ad44 → 4da5 → 9c89` is the freeze
> guardrail that must precede any W3/W4 migration (sequencing guardrails, §8).

---

## 3. The acceptance substrate every wave references

Two fixed targets recur in the acceptance criteria below. Stating them once:

### 3.1 The conformance gate — `assert_repository_replay_parity_conformance` (`mod.rs:3165`)
The shared replay-parity gate any adapter must pass (verified at `persistence/mod.rs:3165`; re-exported
at `mod.rs:4760`; invoked at `mod.rs:4134`). It:
- appends two transcript revisions of one span → replays the ledger, asserts latest-wins;
- appends interleaved notes + graph patches (upsert/delete/reorder notes; upsert nodes/edges, then
  invalidate edge + node) → replays materialized state, asserts note ordering, deletion, and **exact
  `valid_until_ms`** on the invalidated edge/node;
- saves replayed snapshots → reloads `load_materialized_projection_state` → asserts **full-state
  equality** with the replay (not a subset);
- a stale-session sub-case asserts `StaleTranscriptRevision` surfaces.

Both adapters pass it today: `FileMemoryRepository` (`mod.rs:4130` region) and the `kv-mem`
`SurrealMemoryRepository` (`surreal.rs:689`). Because replay lives in default trait methods over pure
Rust folds (`replay_transcript_ledger` `mod.rs:553`, `replay_projection_state` `mod.rs:559`,
`load_materialized_projection_state` `mod.rs:573`), an adapter only has to make `append_*` + `load_*`
byte/order-correct to pass parity (discover-persistence §7). **This is the migration safety net and the
acceptance bar.**

### 3.2 The named evidence gates (frame §6; plan-outline §0 "What evidence is missing")
1. **`2b2c`** — cross-platform build/link/size/packaging + durability evidence for the file-backed
   engines. **Unmeasured; one CI run from existing** (sweep-2 §6,§10; `surreal.rs:5-6` records the
   constraint in code).
2. **Streaming-frequency throughput** for any indexed adapter vs the O(1) JSONL append. Conformance
   proves correctness, not throughput (frame §4.1).
3. **Demand signal** for DB queries — present need or speculative (frame §6 Q5). The in-Rust
   chat-context retrieval (`build_graph_chat_context`, `entities.rs:222`) is the leading indicator, not
   yet a strain (discover-graph §5,§8).
4. **Engine choice for the index slot** — SurrealKV vs SQLite vs redb is unresolved by the corpus (the
   two reviews disagree: report → SQLite for the index slot, §6; revisit note → SurrealKV/RocksDB).
   Resolving it depends partly on (1).
5. **Schema stability** — event-sourced core `ad44`/`4da5`/`9c89` open; schema still moving (frame §2.2).

---

## 4. Wave 0 — Ratify + record (no code). Seed: `storage-decision` (new, supersede-aware)

**Work.** Land the three artifacts together:
1. `docs/adr/0021-storage-architecture.md` — MADR 3.0, Status `proposed`, matching the house format
   (Status / Context / Decision Drivers / Considered Options ≥2 / Decision Outcome / Consequences /
   Implementation Outline / Rollback / Acceptance Criteria / References — the section set used by
   ADR-0019). Options: **A** SurrealDB-primary, **B** keep custom files, **C** file-canonical hybrid,
   **A'** status-quo baseline. Outcome: ratify A' / B-default / reject-A-as-default / C-on-signal /
   engine gated on `2b2c` — stated plainly as *gated*, not go/no-go.
2. `docs/designs/storage-architecture-decision-2026-06-27.md` — the design doc (data shapes, query
   patterns, two-generation reality, readability preservation, migration/dual-read with rollback,
   conformance-parity strategy, packaging/size impact, engine comparison table).
3. This plan (already on disk).
4. Update `docs/adr/README.md`: add the `0021` row + link reference; note **ADR-0014
   supersession-pending** (tracked by seed `0d1c`, "Supersede ADR-0014 with event-sourced notes…") and
   the **ADR-0019 relation** (the `serde_yaml`-archived longevity precedent and the codec-boundary
   migration pattern, ADR-0019 Context + Rollback).

**Depends on:** nothing.

**Acceptance criteria:**
- **A0.1 Three artifacts agree** on (a) the recommended posture (A' formal / B default / reject A as
  default / C-on-signal / engine gated on `2b2c`), (b) the five gates (§3.2), and (c) the data model
  (the §6 byte-for-byte type list, two generations carried, conformance = full-state equality incl.
  bitemporal `valid_until_ms`). This is the frame §7 / plan-outline §3 consistency contract.
- **A0.2 ADR is MADR 3.0** with ≥2 considered options, honest pros/cons per option, and an
  evidence-backed, honesty-gated Decision Outcome. Per the adr-methodology: do **not** rationalize
  retrospectively — the outcome must state that the deciding `2b2c` evidence does not yet exist.
- **A0.3 Every load-bearing claim cites** either a corpus source or a `file:line` code location
  (frame §7 "Evidence honesty").
- **A0.4 README index updated**: `0021` row present, status `proposed`, ADR-0014-supersession-pending
  and ADR-0019-relation noted.

**Definition of done:** the three files exist, are internally consistent (A0.1), and nothing is
committed beyond the docs (the deciding frame mandates "nothing committed, no production code",
frame §7).

---

## 5. Wave 1 — No-regret refactors. B stays default; A' enforced. Parallel; gate-independent.

These four sub-waves are correct regardless of how `2b2c` resolves. They reduce per-adapter migration
risk and are the prerequisites a *future* non-default adapter needs. They can run in parallel with each
other and with W2.

### W1a — Thread `Arc<dyn LocalMemoryRepository>` through the hard-wired call sites. Seed: `repo-di-threading` (extends `5679` lineage).

**Why.** `FileMemoryRepository::user_data()` is constructed directly at the call sites below — the
boundary exists (`mod.rs:424`) but the default is hard-wired (discover-persistence §1; frame §2.1). DI
through these sites is the prerequisite for **any** non-default adapter, independent of the gate.

**Verified call sites (2026-06-27 tree):**
- `commands.rs:2582`, `2828`, `2922`, `2943`, `4989`, `5236`, `8583`, `8752`, `9209`
  (nine `FileMemoryRepository::user_data()` constructions).
- `speech/mod.rs:731` (`upsert_live_assist_card` via a direct construction).
- `state.rs` — the plan-outline named `403/712/1224/1423/1488`; **re-verify at implementation time**
  (the file is under active edit per git status; the exact lines were not re-confirmed for this plan).

> The runtime writer threads already accept `Arc<dyn LocalMemoryRepository>`
> (`TranscriptEventWriter::repository` `mod.rs:1918`; `ProjectionEventWriter::repository` `mod.rs:2175`;
> exercised at `mod.rs:4070`/`4102`), so the hot-path write contract is *already* adapter-pluggable. W1a
> only closes the gap at the synchronous command/state call sites (discover-persistence §1).

**Acceptance criteria:**
- **A1a.1** No behavior change: all existing tests green (`cargo test` on default + `cloud` features —
  see ADR-0007 / sweep-2 §4 for the two-path matrix).
- **A1a.2** `FileMemoryRepository` is constructed once and injected as `Arc<dyn LocalMemoryRepository>`;
  **zero** direct `FileMemoryRepository::user_data()` calls remain at the listed command/state/speech
  call sites (a grep for the construction returns only the single injection root + tests).
- **A1a.3** The conformance gate (`mod.rs:3165`) still passes for `FileMemoryRepository` unchanged.

### W1b — Hoist shared validation/privacy gates into trait default methods. Seed: `repo-shared-gates` (new).

**Why.** `ensure_org_visible_record_is_safe` (`mod.rs:240`) and `validate_live_assist_card`
(`mod.rs:292`), plus each type's `.validate()`, are duplicated between the file and surreal adapters
rather than shared on the trait — a third adapter would re-duplicate them and could bypass a privacy
floor (discover-persistence §10; discover-promotion §7 item 4). Hoisting them into trait default
methods makes them adapter-agnostic.

**Acceptance criteria:**
- **A1b.1** Identical reject/accept behavior: the existing file-adapter privacy/validation tests
  (`mod.rs:4344-4699` region — e.g. `file_memory_repository_rejects_private_org_visible_payload_fields`,
  `file_memory_repository_rejects_unredacted_revocation_requests`) pass unchanged.
- **A1b.2** A hypothetical third adapter cannot bypass the gates: the validation is invoked from the
  default trait method, not from each adapter's `impl` (verified by moving the call into the default and
  confirming an adapter that does *not* re-implement it still rejects a forbidden-key payload).
- **A1b.3** The privacy floor list is unchanged (still rejects `api_key`, `secret`,
  `raw_transcript_text`, `speaker_names`, `source_ids`, `provider_ids`, … — discover-promotion §7 item 4).

### W1c — Extend the shared conformance gate to cover promotion/privacy. Seed: `conformance-promotion` (extends `965b` lineage).

**Why — the single biggest correctness gap.** Today the shared gate (`mod.rs:3165`) covers only
transcript + notes + graph replay. The promotion / redaction / org-knowledge audit-vs-current / revoke
/ silent-source-mutation / privacy-floor invariants are pinned by **file-adapter-only `#[test]`s**
(`mod.rs:4344-4699`). The surreal adapter runs only the shared suite (`surreal.rs:689`). **Therefore
promotion parity is UNVERIFIED for any non-file adapter** (discover-promotion §7 item 8). Lifting these
into the shared gate is the precondition for trusting any future adapter on the promotion subsystem.

**What to lift (discover-promotion §7):** audit immutability + completeness (rejected records never
enter the log); audit-vs-current divergence (`load_*_audit` returns all revisions, `load_*` returns
one-per-key last-write-wins with stable sort — `id`; then `(promotion_event_id, target_kind)`); the two
write guards (silent-source-mutation rejection `mod.rs:1215-1230`; terminal-state/`deleted_at_ms`/
`delete_reason`/`Revoked`-sync requirements in `revoke_org_knowledge_item` `mod.rs:1294`); the privacy
floor; bitemporal round-trip; transcript-ledger cross-link integrity (`source_basis`).

**Acceptance criteria:**
- **A1c.1** The **extended** `assert_repository_replay_parity_conformance` passes for
  `FileMemoryRepository` (no regression of the existing assertions; the lifted invariants now run inside
  the shared gate).
- **A1c.2** The `kv-mem` `SurrealMemoryRepository` is run against the **extended** gate. **Expected
  outcome: it surfaces the read-time-vs-write-time fold divergence** — the file adapter materializes
  `.current.json` at write time, the surreal adapter folds at read time (discover-promotion §3). The
  acceptance is that the gate *detects* any observable divergence, not that surreal passes; if it
  passes, the parity claim is genuinely stronger; if it fails, the failure is a real finding the design
  doc must record.
- **A1c.3** No new privacy bypass: a forbidden-key org-visible payload is rejected through the gate for
  every adapter the gate is run against.

### W1d (optional) — `[profile.release]` baseline + size tracking. Seed: `release-size-baseline` (new).

**Why.** There is **no `[profile.release]` override today** (no LTO/strip/opt-level — sweep-2 §9) and
**no measured release-build binary size** is tracked or asserted (sweep-2 §10 item 5). Without a
baseline, the `2b2c` size-delta evidence has nothing to compare against.

**Acceptance criteria:**
- **A1d.1** A recorded baseline stripped binary size per OS (Linux/macOS/Windows), captured from the
  existing default-feature Tauri smoke artifacts (`ci.yml:526-536` region uploads them).
- **A1d.2** No functional change; if a `[profile.release]` section is added, both feature paths
  (default + `cloud`) still build and the test matrix stays green.

> W1d is "helpful but not required" for W2 (plan-outline §2 Wave 2 "Depends on"). If skipped, W2 must
> capture its own baseline first.

---

## 6. Wave 2 — THE GATE. The unblocking experiment. Seed: `audio-graph-2b2c` (open, unstarted).

This is the one wave that turns "gated" into "go/no-go." It is **throwaway** — wired into nothing,
schema-independent, and **can run today, in parallel with W1** (plan-outline §2; sequencing
guardrails §8).

**Work (scratch branch, never merged):**
- Add `kv-surrealkv` and `kv-rocksdb` as alternative features behind `surrealdb-embedded` (the existing
  opt-in pattern — `Cargo.toml:40`, `:218`; ADR-0007 precedent, sweep-2 §8). Neither feature is in the
  current `Cargo.lock` (sweep-2 §6) — this is the first time either is compiled.
- `cargo build --release --features surrealdb-embedded,<engine>` on **Blacksmith Linux + macOS +
  Windows**. Record: build/link **time** delta, **stripped size** delta vs the W1d baseline, **native-dep
  inventory** (does `kv-rocksdb` pull C++/cmake/clang beyond the already-required `aws-lc-sys` cmake?
  sweep-2 §3,§7).
- `tauri build` per OS per engine (installer/`.app`/`.dmg`/`.nsis`/`.appimage`/`.deb` size — targets at
  `tauri.conf.json:27`; does it link and **launch**?).
- A **10-minute durability probe**: write N sessions through the file-backed engine, `kill -9`
  mid-write, reopen, confirm recovery; then corrupt the store and confirm the failure mode. This is the
  durability evidence the `kv-mem` spike has **never** produced (discover-persistence §10; the spike
  never writes a file).

**Depends on:** W1d (baseline) helpful but not required; schema-independent.

**Acceptance / decision rule (plan-outline §2 Wave 2; final report §2,§6,§10):**
- **A2.1 — green on all three OSes** (clean link, acceptable size delta, sane crash + corruption
  recovery) for the chosen engine → **advance to W3**.
- **A2.2 — red on any OS** (Windows link failure à la the ADR-0007 `0xC0000139` class, large size
  regression, or weak recovery) for **both** engines → **close `2b2c`** "file path stays default";
  keep `kv-mem` test-only; revisit only when `ceda`/cross-session-recall becomes committed roadmap.
- **A2.3 — engine sub-decision resolved here, on the measured numbers** (gate #4, §3.2):
  - SurrealKV is the candidate **iff** it is green on all three OSes (pure-Rust avoids the C++
    toolchain — sweep-2 §3) **and** the demand is graph/vector/sync-shaped. But weigh the corpus
    caveats: SurrealKV is **beta with an unstable on-disk format**, its safe `sync=every` mode is the
    slow mode, and it carries a **back-in-time versioning caveat** that collides with this app's
    retroactive-revision model (transcript `supersedes`; graph `invalidate_*` setting `valid_until_ms`
    after the fact) — final report §2,§3.
  - SQLite is the lighter candidate **iff** the demand is FTS / recursive-CTE traversal at bounded
    scale; it is the lowest-longevity-risk option (public domain, Tauri-blessed) and is production-proven
    for this exact bitemporal model (final report §3,§6).
  - redb is the pure-Rust KV fallback (stable file format) **iff** a pure-Rust KV is preferred over SQL
    and no query power is needed (final report §6).
  - RocksDB stays **off** unless SurrealKV fails *and* RocksDB clearly justifies the Windows native-link
    cost (it is C++; ADR-0007 minefield — final report §2; sweep-2 §3,§7).

**Honesty note.** Do **not** read "the spike conforms" as "SurrealDB performs adequately." The spike's
O(n) full-table-scan-per-append sequence assigner (`next_session_sequence` `surreal.rs:153`,
`next_global_sequence` `surreal.rs:167`) fails fit-criterion #1 and is fatal as a real file-backed write
path (frame §2.4; discover-persistence §8). The only honest reading is "Surreal conforms under `kv-mem`
with an O(n) assigner." W2 measures packaging/durability; **throughput is a W3 acceptance item, not a
W2 one.**

---

## 7. Wave 3 (GATED) — Indexed adapter rewrite + benchmark. Seed: `audio-graph-48bb` (blockedBy `2b2c`).

**Gated on:** W2 green (A2.1) **and** a named demand signal (gate #3) **and** W1a (DI) + W1c (promotion
conformance) landed.

**Work.** Rewrite the chosen adapter (W2's A2.3 winner) to use indexed/keyed access — **drop the O(n)
full-table-scan sequence assigner** (`surreal.rs:153-178`). Benchmark its append throughput against the
O(1) JSONL append at **streaming transcript frequency** (one event per partial + final per span, many
per second — discover-persistence §3). Make it **selectable-but-not-default**.

**Acceptance criteria:**
- **A3.1** Passes the **extended** conformance gate (§3.1 + W1c), full-state equality incl. exact
  `valid_until_ms`.
- **A3.2 — streaming-frequency throughput ≥ file path** (no regression vs O(1) JSONL append). This is
  gate #2; it is **not** proven by conformance and is the specific failure mode the spike's O(n)
  assigner would exhibit.
- **A3.3** Re-implements the bespoke product-visible durability surfaces the file adapter has and a DB
  adapter loses by default: disk-full surfacing (`CAPTURE_STORAGE_FULL` via `io.rs:90`, debounced),
  backup/export parity, owner-only perms (discover-persistence §3.3; discover-graph §3 item 6).
- **A3.4 — back-in-time / retroactive-revision correctness proven** for the chosen engine: transcript
  `supersedes` and graph `invalidate_*` (setting `valid_until_ms` after the fact) read back correctly.
  This is the SurrealKV back-in-time landmine made into a pass/fail test (final report §2,§3). If the
  engine is SurrealKV, this requires the versioned B+tree index enabled and validated.
- **A3.5** Default unchanged: `FileMemoryRepository` remains the default; the new adapter is opt-in.

---

## 8. Wave 4 (GATED) — File-canonical hybrid index + migration/dual-read. Seeds: `audio-graph-9c89` (migration), `audio-graph-ceda` (recall UX).

**Gated on:** W3 green; **schema frozen** (`ad44`/`4da5`/`9c89` — guardrail §9); and the demand signal
is **committed roadmap**, not a seed (frame §6 Q5,Q7). `9c89` is itself `blockedBy 4da5, ad44` — i.e.
the seed graph already encodes the schema-freeze precondition.

**Work.** Stand up the rebuildable index as a **derived projection** of the canonical logs (logs
authoritative — the file-canonical reading of Option C, final report §9). Dual-read/backfill from **both
storage generations** (gen-1 legacy transcript `.jsonl` + petgraph `graphs/<id>.json`; gen-2 event logs
+ materialized snapshots — discover-graph §1,§4) with rollback. Wire the cross-session recall workload
(`ceda`) and/or `build_graph_chat_context` (`entities.rs:222`) onto it.

**Acceptance criteria:**
- **A4.1 — index is rebuildable from logs**: rollback = drop the index and rebuild (the index is a
  cache, the logs are the source of truth — final report §9,§10; discover-graph §7 item 2).
- **A4.2 — migration is idempotent + non-destructive + parity-gated**: re-running the backfill produces
  the same state; the legacy artifacts are not deleted (mirrors the ADR-0019 non-destructive-migration
  pattern); the extended conformance gate (§3.1 + W1c) passes against the migrated state.
- **A4.3 — both generations carried**: the petgraph snapshot and the materialized/event-sourced graph
  are both readable and reconciled (the `choose_materialized_graph` `last_sequence` merge,
  `commands.rs:5206` — discover-graph §4). Legacy artifacts are NOT replayable from the event log
  (discover-graph §1), so the migration must carry them, not derive them.
- **A4.4 — human-readable transcripts/notes unchanged**: files remain canonical; the index is derived;
  `export_graph` (`commands.rs:4577`) and `PromotionSyncTargetKind::FileExport` (promotion.rs) keep
  working (discover-graph §7 item 8; discover-promotion §8).
- **A4.5 — no packaging regression vs the W2 baseline** (size, link, launch on all three OSes).

---

## 9. Sequencing guardrails (the hard ordering constraints)

1. **Freeze the event-sourced core schema before any DB migration.** Land or freeze `ad44`
   (data model) → `4da5` (revision ledger) → `9c89` (artifact migration) **before** W3/W4 so the schema
   migrates once, not twice (frame §2.2; final report §10; plan-outline §2 sequencing). The seed graph
   already enforces this: `9c89` is `blockedBy 4da5, ad44`. Migrating now means migrating twice — B pays
   nothing, A pays the full migration plus a data-loss risk on a still-moving schema (final report §10
   migration economics).
2. **`2b2c` (W2) is schema-independent and runs in parallel with W1 today.** It builds throwaway
   branches and is wired into nothing, so it does not wait on the schema freeze (plan-outline §2;
   sweep-2 §6 — the seed is "open, unblocked").
3. **Never make a DB the default until W2 is green (A2.1) AND a demand signal is committed.** Both
   conditions, conjunctively. The "lean in" and "remove it" verdicts both assert past the evidence until
   `2b2c` runs (plan-outline §0; final report §10).
4. **W0 before everything; W1 sub-waves are mutually parallel and gate-independent; W2 parallel with
   W1; W3 after W2-green + demand + W1a/W1c; W4 after W3 + schema-freeze + committed roadmap.**

```
W0 (ratify) ──► W1a ─┐
                W1b ─┤ (all parallel, gate-independent)
                W1c ─┤                                 ┌─► W3 (48bb) ──► W4 (9c89 + ceda)
                W1d ─┘                                 │   [GATED: W2-green + demand     [GATED: W3 + schema-freeze
                                                       │    + W1a + W1c]                  (ad44→4da5→9c89) + committed roadmap]
W2 (2b2c, THE GATE) ───────────────────────────────────┤
  (parallel with W1, schema-independent)               │
  ├─ green-all-three  ──────────────────────────────────┘
  └─ red-any ──► CLOSE 2b2c; file default stays; kv-mem test-only
```

---

## 10. What this plan does NOT do (explicit non-goals, for honesty)

- It does **not** pick SurrealKV vs SQLite vs redb. That is the W2 A2.3 sub-decision, resolved on
  measured numbers, because the corpus genuinely disagrees (final report §6 → SQLite for the index slot;
  the revisit note → SurrealKV/RocksDB) and the deciding evidence is absent (gate #4).
- It does **not** add runtime selectability, a file-backed engine, or migration tooling now. A' holds
  the seam; B is the only default and only selectable store (plan-outline §0).
- It does **not** treat the surreal spike's conformance pass as performance evidence (the O(n) assigner
  caveat, §6 honesty note).
- It does **not** write production code in the run that produces it (frame §7; plan-outline §1.3).

---

## 11. Risks and uncertainties (named, not papered over)

| Risk | Likelihood | Impact | Where addressed | Confidence |
|---|---|---|---|---|
| `2b2c` never gets run, so the dial stays frozen indefinitely | Medium | Medium (optionality unrealized, not a regression) | W2 is "one CI run from existing" (sweep-2 §6); guardrail §9.3 | High that it's cheap; uncertain it'll be prioritized |
| `kv-rocksdb` Windows link failure (ADR-0007 `0xC0000139` class) | Medium-High | Closes the RocksDB path | W2 A2.2; sweep-2 §3,§7,§8 | Medium-High (untested in-repo) |
| SurrealKV back-in-time caveat breaks retroactive revision | Medium | High if SurrealKV chosen | W3 A3.4 makes it a pass/fail test; final report §2,§3 | Medium (documented caveat; repair landed Feb 2026 but beta) |
| W1c surfaces a real read-time-vs-write-time fold divergence in surreal | Likely | Informational (a finding, not a blocker) | W1c A1c.2; discover-promotion §3 | High (the mechanisms differ by design) |
| Schema (`ad44`/`4da5`/`9c89`) moves during W3/W4, forcing a second migration | Medium | High | Guardrail §9.1; seed graph `9c89 blockedBy 4da5,ad44` | High that the guardrail prevents it if followed |
| `state.rs` call-site line numbers stale (file under active edit) | High | Low (re-verify at impl time) | W1a note; §1 line-number note | High |
| No demand signal ever materializes (DB upside is speculative) | Medium | Low (B fully serves today's bounded workload) | Gate #3; final report §8; frame §5 | Medium (chat-context retrieval is the leading indicator, §3.2) |

---

## 12. Consistency contract (this plan ↔ the ADR ↔ the design doc)

All three artifacts must agree on:
- **Posture:** A' formal / B default / reject A as default / C-on-signal / engine gated on `2b2c`.
- **Gates:** the five named gates (§3.2) — `2b2c` (the blocker), streaming-frequency benchmark, demand
  signal, engine sub-decision, schema freeze.
- **Data model:** the byte-for-byte type list, two generations carried, conformance = full-state
  equality incl. bitemporal `valid_until_ms` (design doc §"data shapes"; discover-persistence §5).
- **Honesty posture:** the verdict is *gated*, not go/no-go; unknowns are named gates, not papered over
  (frame §7; plan-outline §3).

If any artifact diverges from this contract, the divergence is a W0 defect (A0.1) and must be resolved
before the three are considered ratifiable.

---

## 13. References

Corpus (deciding megaloop, `docs/designs/_storage-megaloop-2026-06-27/` and `research/notes/`):
- `frame.md` — the decision frame, fit-criteria (§4), the human-readability tension (§5), open questions (§6), doneMeans (§7).
- `plan-outline.md` — the scale-setter outline this plan expands (§0 decision, §2 waves, §3 consistency contract).
- `research/notes/final_report_storage-arch-tauri-temporal-graph-6415ba.md` — the hyperresearch report (§2 engine tradeoffs, §3 query-model fit, §6 embedded alternatives, §7–§10 option evaluations + recommendation).
- `discover-src_tauri_src_persistence.md` — persistence trace (§1 trait/call-sites, §3 write path, §7 conformance, §8 spike red flags, §10 open flags).
- `discover-src_tauri_src_graph.md` — graph trace (§1 two representations, §4 reconcile, §5 query patterns, §7 acceptance floor).
- `discover-src_tauri_src_promotion_rs.md` — promotion trace (§3 fold divergence, §7 acceptance checklist incl. item 8 the unverified-for-non-file gap).
- `sweep-2.md` — packaging/binary-size constraints (§3 native deps, §6 the `2b2c` gate, §9 no release profile, §10–§11 open questions + verdict map).

Code (`src-tauri/src/`, 2026-06-27 tree; re-verify at implementation time):
- `persistence/mod.rs:424` (trait), `:593` (`FileMemoryRepository`), `:599` (`user_data`),
  `:240`/`:292` (shared gates), `:553`/`:559`/`:573` (default-method replay), `:1918`/`:2175` (writer
  thread `Arc<dyn>` plumbing), `:1215-1230` (silent-source-mutation guard), `:1294` (`revoke_org_knowledge_item`),
  `:3165` (conformance gate), `:4344-4699` (file-only promotion/privacy tests).
- `persistence/surreal.rs:5-6` (the file-engine constraint comment), `:153`/`:167` (O(n) sequence
  assigners), `:689` (surreal conformance entry).
- `commands.rs:2582,2828,2922,2943,4989,5236,8583,8752,9209` (W1a call sites), `:4577` (`export_graph`),
  `:5206` (`choose_materialized_graph`).
- `speech/mod.rs:731` (W1a call site).
- `graph/entities.rs:222` (`build_graph_chat_context`, the leading-indicator query).
- `src-tauri/Cargo.toml:40` (`surrealdb-embedded` feature), `:218` (`kv-mem`-only dep).

Related ADRs:
- ADR-0007 (`docs/adr/0007-feature-gate-local-ml.md`) — feature-gate + Windows native-link precedent.
- ADR-0014 (`docs/adr/0014-notes-synthesis.md`) — supersession-pending (seed `0d1c`).
- ADR-0019 (`docs/adr/0019-credential-and-config-storage.md`) — `serde_yaml`-archived longevity
  precedent + codec-boundary non-destructive-migration pattern.
- ADR-0021 (`docs/adr/0021-storage-architecture.md`) — the decision this plan implements (W0).

Seeds (`.seeds/issues.jsonl`, verified 2026-06-27): `2b2c` (THE GATE), `48bb` (W3, blockedBy 2b2c),
`ceda` (W4, blockedBy 48bb), `9c89`/`4da5`/`ad44` (schema chain), `0d1c` (ADR-0014 supersession),
`5679`/`965b` (closed — the trait + parity-test history W1 extends), `5dde` (closed — the original spike).
