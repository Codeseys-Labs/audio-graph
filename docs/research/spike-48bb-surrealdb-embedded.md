# Spike 48bb — SurrealDB Embedded as Local Memory/Graph Adapter

**Date:** 2026-06-30
**Seed:** `audio-graph-48bb` (named in ADR-0021 as "the indexed rewrite")
**Question:** Should AudioGraph adopt embedded, file-backed SurrealDB (RocksDB / SurrealKV)
as the local memory + temporal-graph store, versus the current file-based event-sourced
repository?
**Verdict:** **spike-further** — advance the gated indexed rewrite as a *selectable,
non-default* adapter; do **not** make any database the default store. Rationale below.

> **The decision is the user's.** This doc is the grounded recommendation. It does not
> commit code. ADR-0021 (accepted-gated, 2026-06-27) already ratified the *posture*; this
> spike re-tests it against (a) the now-collected `2b2c` gate evidence and (b) fresh
> upstream facts (SurrealDB 3.1.5 / 3.2.0-beta, SurrealKV 0.21.2, the Dec-2025 durability
> default flip), and confirms the posture holds with one engine-choice refinement.

---

## TL;DR

- The architectural seam this turns on **already exists and is conformance-tested**:
  `trait LocalMemoryRepository` (`src-tauri/src/persistence/mod.rs:431`) with two passing
  adapters — production `FileMemoryRepository` and a feature-gated `SurrealMemoryRepository`
  (`src-tauri/src/persistence/surreal.rs:76`, behind `surrealdb-embedded`, **off by default**:
  `Cargo.toml:40`, `default = ["local-ml"]` at `Cargo.toml:33`). The decision is "which adapter
  is default + what gates promotion," not "introduce a boundary."
- **The blocking gate (`2b2c`) has now produced evidence.** SurrealKV builds clean on Linux
  (+1.17 MiB stripped), cross-compiles + links clean for `x86_64-pc-windows-msvc` (including
  RocksDB's vendored C++ + bindgen, the top risk ADR-0021 feared), and **passed a real
  cross-process restart durability test** on Linux. (`docs/reviews/2b2c-local-linux-evidence-2026-06-27.md`)
- **The make-or-break perf question is answered.** The current adapter's O(n)-full-scan-per-append
  regresses the streaming hot path **20.5×** at 10k rows (p99 blows to 222 ms). But the *keyed
  rewrite* — which is exactly what seed `48bb` is — recovers full O(1) parity with the file path
  (3.53 ms vs 3.26 ms mean, flat with table size).
  (`docs/reviews/2b2c-throughput-benchmark-2026-06-27.md`)
- **As a *default canonical* store SurrealDB still loses**: human-readable transcripts are a hard
  product requirement (an opaque LSM blob fails it), SurrealKV is still officially **beta**, and
  the migration is asymmetric vs near-zero for the file path.
- **As a *selectable, derived index* (the Option-C / Wave-3 path) it is now viable** and the
  evidence to advance it exists. That is the "spike-further" recommendation: do the keyed indexed
  rewrite behind the existing feature gate, keep files canonical and default.

---

## 1. Options compared

The four "options" are positions on one dial (per ADR-0021). The table below scores the realistic
forks against the seven fit-criteria the codebase imposes.

| Option | What it is | Hot-path append | Durability (default) | Human-readable | Cross-platform bundling | Migration cost | License | Fit verdict |
|---|---|---|---|---|---|---|---|---|
| **B — Keep file event logs (current default)** | `FileMemoryRepository`: append-only JSONL + materialized JSON snapshots | **O(1)** `append_jsonl`+`sync_all` (`mod.rs:160`); ~3.26 ms/append (fsync floor) [bench] | by-default per-append fsync + atomic fsync-before-rename (`save_json` `mod.rs:2422`) | **yes** (plain JSONL/JSON, greppable, `cp`-backupable) | **none new** — pure Rust, no storage dep | **zero** | project's own | **Serves today's workload fully** |
| **A — SurrealDB-primary (RocksDB or SurrealKV) as canonical default** | DB is the source of truth; files become an export | O(1) *after* keyed rewrite [bench]; current adapter O(n²) | SurrealKV/RocksDB now default `sync=every` (most durable) since Dec-2025 [PR #6614 / #6882], but was *not* crash-safe by default before that [cf8.gg] | **no** — opaque LSM blob; needs a file-export path, which collapses A→C | RocksDB = C++/cmake/bindgen (largest, +6.68 MiB); SurrealKV = pure-Rust +1 C lib (lz4), +1.17 MiB [2b2c] | **high** — carry 2 on-disk generations through dual-read against an opaque store; re-implement disk-full UX; data-loss risk on a bad bet | SurrealDB **BSL 1.1** (permissive Additional Use Grant; embedding allowed; →Apache-2.0 after 4 yrs) [surrealdb.com/license] | **Rejected as default** |
| **C — File-canonical + rebuildable embedded index (SurrealKV *or* SQLite)** | Logs stay canonical; DB is a *derived, rebuildable* index for traversal/FTS/vector | O(1) on canonical log; index updated async | logs keep file durability; index is rebuildable so its durability is non-critical | **yes** (files canonical) | SurrealKV pure-Rust (+1.17 MiB) **or** SQLite (~5 MB, Tauri-blessed) | **small + reversible** — rollback = "drop the index" | SurrealKV **Apache-2.0** [crates.io]; SQLite public domain | **Pre-committed next step, on demand signal** |
| **A' — Status-quo posture (trait seam + file default + DB gated opt-in)** | Keep seam; file default; `kv-mem` adapter as off-by-default conformance target | n/a (file path used) | n/a | yes | none | n/a | n/a | **Standing posture (always correct)** |

Sources for the table: code anchors verified against the working tree (line numbers below);
`[bench]` = `docs/reviews/2b2c-throughput-benchmark-2026-06-27.md`;
`[2b2c]` = `docs/reviews/2b2c-local-linux-evidence-2026-06-27.md`;
`[PR #6614]` = https://github.com/surrealdb/surrealdb/pull/6614;
`[PR #6882]` = https://github.com/surrealdb/surrealdb/pull/6882;
`[cf8.gg]` = https://blog.cf8.gg/surrealdbs-ch/;
license = https://surrealdb.com/license + https://github.com/surrealdb/surrealdb/blob/HEAD/LICENSE.

### Engine sub-comparison (for the index slot, if/when C is built)

| Engine | crates.io status | Temporal-graph fit | Longevity / bus-factor | Tauri bundling | Verdict |
|---|---|---|---|---|---|
| **SurrealKV** (pure-Rust LSM) | `0.21.2`, Apache-2.0, **officially beta** [surrealdb.com/docs/build/deployment] | good; native time-travel (`VERSION`); MVCC; **back-in-time caveat** (mitigable, see §2.3) | single-vendor (SurrealDB Ltd); co-developed with the DB | pure-Rust, +1.17 MiB stripped, +1 C lib (lz4) [2b2c] | candidate if demand = graph/vector/sync **and** it's green |
| **RocksDB** (SurrealDB durable fork) | `surrealdb-rocksdb 0.24.0-surreal.5`, RocksDB 11.0 | good (mature LSM, write-optimized) | mature engine | **C++/cmake/bindgen/libclang; +6.68 MiB; ~5.5× growth** [2b2c] | only if SurrealKV fails and write throughput justifies the native-link cost |
| **SQLite** (+FTS5, recursive CTE) | ubiquitous, public domain | strong (recursive CTE traversal + bitemporal columns + FTS5) | **highest** (not single-vendor) | Tauri-blessed, single-file, ~5 MB | first choice if demand = FTS / recursive-CTE traversal at bounded scale |
| **redb** | stable file format | none (KV only; hand-roll indexing) | high | pure-Rust, single-file | pure-Rust KV fallback; no query power |
| **KuzuDB** | **archived 2025-10-10**, v0.11.3 final, forked as "bighorn" [theregister.com; github.com/kuzudb/kuzu] | best raw (native Cypher, vector+FTS) | **low — the empirical proof of single-vendor risk** | embeddable | avoid; cautionary tale, not a candidate |

---

## 2. Key trade-offs

### 2.1 Latency / perf — the decisive number, now measured

The current `SurrealMemoryRepository` assigns sequences with an O(n) full-table scan on **every**
append (`next_session_sequence` `surreal.rs:153`, `next_global_sequence` `surreal.rs:167` → `select_all` →
filter → `max`+1). Total work over a session is O(n²). The 2b2c benchmark confirms this is a real
hot-path regression for streaming ASR (which writes many events/sec):

| Path | N=10,000 mean | p99 | vs file |
|---|---|---|---|
| `file_jsonl` (current default, O(1)) | 3.26 ms | 5.60 ms | 1× |
| `surrealkv_onscan` (current adapter, O(n)) | **66.78 ms** | **222.56 ms** | **20.5×** |
| `surrealkv_keyed` (the `48bb` rewrite, O(1) scan) | 3.53 ms | 5.72 ms | ~1.08× |

**Conclusion:** SurrealKV is *not* fundamentally too slow. The regression is an adapter-algorithm
defect, not an engine defect. The keyed rewrite (hold max-sequence in memory; the planned `48bb`
work) brings it to within ~8% of the file path and flat with table size. This converts "lean in"
from speculative to evidence-backed — but *only after the rewrite ships*. The current in-tree
adapter must **not** be promoted as-is.

### 2.2 Durability — materially improved, but recently, and with a sharp edge

- The Rust SDK exposes `.sync(SyncMode::Every)` for SurrealKV/RocksDB
  (https://docs.rs/surrealdb/latest/surrealdb/struct.Connect.html). SurrealKV's file engine now
  **defaults to `sync=every` (most durable)** after PR #6882 (Feb 2026); RocksDB's `SURREAL_SYNC_DATA`
  flipped to `true` by default in Dec 2025 (PR #6614).
- **Adversarial finding:** before that flip, both backends were *not* crash-safe by default and
  there were corruption reports (https://blog.cf8.gg/surrealdbs-ch/, Aug 2025: "your instance is NOT
  crash safe and can very easily corrupt"). The safe behavior is recent (≤ ~7 months old) and any
  adapter MUST set `SyncMode::Every` explicitly and pin it in a test — do not rely on a default that
  changed twice in six months.
- Net: a correctly-configured SurrealKV adapter can match the file path's per-append durability, but
  this is a configuration responsibility the adapter must own, not a free property.

### 2.3 Schema / graph fit — good, with one caveat the app must dodge

AudioGraph's model is **bitemporal with retroactive revision**: transcript spans carry
`revision_number`/`supersedes`; graph `invalidate_*` ops set `valid_until_ms` *after the fact*
(`projections.rs`, materialized graph types). These are literally "back-in-time" writes.

- SurrealKV's documented caveat: *"When versioning is enabled without the B+tree index, timestamps
  inserted 'back in time' will not be read correctly"* — its LSM orders by key asc / sequence desc,
  not by timestamp. **Mitigation exists:** `with_versioned_index(true)` enables a B+tree that
  "correctly handles out-of-order timestamp inserts"
  (https://github.com/surrealdb/surrealkv README/ARCHITECTURE).
- **The clean dodge:** the adapter does *not* need SurrealKV's time-travel `VERSION` feature at all.
  Bitemporality is modeled in the app's *own* fields (`valid_from_ms`/`valid_until_ms`) and replayed
  in pure Rust over default trait methods. Store records as plain rows (versioning disabled) and the
  back-in-time caveat never fires. This is exactly what the current spike does (plain records, no
  `with_versioning`), so the caveat is real but **not a blocker** for this data model — a refinement
  on ADR-0021's framing of it as a head-on collision.
- The flip side: none of SurrealDB's *distinctive* value (native graph edges, live queries, vector
  index) is exercised by storing opaque `serde_json::Value` in `SCHEMALESS` tables (`surreal.rs:104`).
  Adopting it without using its graph/query model is paying the cost without the benefit.

### 2.4 On-disk size / bundling — SurrealKV is cheap, RocksDB is not

- SurrealKV: **+1.17 MiB** stripped on Linux; pure-Rust + one C compression lib (lz4); **no new
  toolchain class** (cc/cmake/bindgen already present via pipewire + aws-lc-sys). Cross-compiles +
  links clean for Windows-MSVC. [2b2c]
- RocksDB: **+6.68 MiB** stripped, ~5.5× growth, vendored C++ compiled via cc+cmake+bindgen+libclang,
  three `-sys` compression crates. Cross-compiled clean for Windows-MSVC under cargo-xwin (the feared
  bindgen-against-MSVC-headers step succeeded), but it remains the heaviest option. [2b2c]
- For reference, a controlled Tauri comparison put SurrealDB at ~46 MB `.app` vs SQLite ~5 MB — the
  largest of any candidate (research corpus). The 2b2c deltas are the load-bearing, repo-specific
  numbers.
- **Still unmeasured:** native Windows *runtime* (cross-compile ≠ run), RocksDB durability on any
  platform, macOS leg, and stripped PE/Mach-O size deltas. These are the remaining `2b2c` gaps.

### 2.5 License — clear, permissive enough, no blocker

- **SurrealDB core:** Business Source License 1.1 with a permissive Additional Use Grant. Embedding
  SurrealDB in your application (including apps shipped to customers) is explicitly allowed; the only
  restriction is offering it commercially as a DBaaS. Converts to Apache-2.0 four years after each
  release. SurrealDB's maintainers confirmed single-instance-embedded-in-app deployments are fine
  (github.com/surrealdb/docs.surrealdb.com issue #1259, 2025). For a local-first desktop app this is
  **not a constraint**.
- **SurrealKV** (the pure-Rust engine crate): **Apache-2.0** (crates.io). The SDKs are Apache-2.0/MIT.
- Caveat for honesty: BSL is *not* OSI-approved during the 4-year window. If the project has a hard
  "OSI-approved dependencies only" policy this matters; for a shipped desktop binary it does not.
  SQLite (public domain) is the cleaner license if license purity is weighted heavily.

### 2.6 Maintenance / longevity — single-vendor risk is real and recently demonstrated

- SurrealKV is co-developed with and **single-vendor** to SurrealDB Ltd, and is **officially beta**
  (SurrealDB's own deployment docs, 2026: *"For conservative production on-disk server deployments
  today, prefer RocksDB"*).
- The bus-factor risk is not theoretical: **KuzuDB was archived 2025-10-10** (v0.11.3 final, "working
  on something new," community forked it as "bighorn") — a well-liked, MIT, actively-developed
  embedded graph DB that vanished. (https://www.theregister.com/software/2025/10/14/kuzudb_graph_database_abandoned/;
  https://github.com/kuzudb/kuzu archived=true). This is the empirical reason to keep files canonical
  and any DB *rebuildable* — so a vendor disappearing is a rebuild, not a data-loss event.

### 2.7 Effort — asymmetric, which is the whole argument

- **B (stay):** zero.
- **C / `48bb` keyed indexed adapter (recommended path):** moderate and *reversible* — the index is a
  derived projection of canonical logs; rollback is "drop the index."
- **A (DB-canonical default):** large and *irreversible-ish* — full migration of two on-disk
  generations through dual-read against an opaque store, re-implementing the bespoke disk-full UX,
  with data-loss risk on a bad engine bet against a still-moving schema (`SCHEMA_VERSION = 1`, no
  migration code yet). Migrating now = migrating twice.

---

## 3. RECOMMENDATION

**spike-further.** Concretely:

1. **Keep `FileMemoryRepository` the default and the only *canonical* store.** It satisfies all seven
   fit-criteria for today's workload: O(1) non-stalling append on the streaming hot path, bounded data
   (sessions cap 100; graph caps 1000 nodes / 5000 edges, `temporal.rs:45,48`) keeping load-and-replay
   cheap, by-default durability, and human-readable/portable transcripts (a hard product requirement an
   opaque DB cannot meet). **Reject SurrealDB as the default canonical store.**

2. **Advance the gated `48bb` indexed rewrite as a *selectable, non-default* adapter**, because the gate
   evidence that was missing when ADR-0021 was written now exists and is green on the legs run:
   - The keyed rewrite reaches O(1) append parity with the file path (the perf blocker is resolved in
     principle — `2b2c` throughput bench).
   - SurrealKV builds + links clean on Linux and cross-compiles clean for Windows-MSVC, adds only
     +1.17 MiB, and passed a real cross-process durability test (`2b2c` local-linux evidence).
   The rewrite must: drop the O(n) full-table-scan sequence assigner (`surreal.rs:153`–`178`), set
   `SyncMode::Every` explicitly, pass the **extended** conformance gate (promotion/redaction/org-knowledge
   + exact `valid_until_ms`), and stay behind `surrealdb-embedded` (off by default).

3. **When a real query demand signal lands** (the in-Rust top-k RAG in `build_graph_chat_context`,
   `entities.rs:222`, is the leading indicator — not yet a strain), build it as **file-canonical
   Option C**: logs authoritative, DB a rebuildable index. **Pick the engine on the demand shape, not
   blind:** SurrealKV if the demand is graph/vector/sync and it stays green; **SQLite** if the demand is
   FTS / recursive-CTE traversal at bounded scale (lighter, public-domain, lower bus-factor).

4. **Do not promote any DB to default** until: (a) the remaining `2b2c` gaps close (native Windows
   runtime + durability, macOS leg, RocksDB durability), (b) the event-sourced schema is frozen, and
   (c) a committed demand signal exists.

**Why "spike-further" and not "adopt" or "reject":** "adopt" overclaims — the gate is green only on the
legs run (Linux + Windows-cross-compile); native Windows runtime, durability, and macOS are still open,
SurrealKV is still beta, and no query demand yet justifies the index. "reject" underclaims — the perf
blocker is solved by the keyed rewrite, the license/bundling/build risks came back acceptable for
SurrealKV, and the back-in-time caveat is dodgeable. The disciplined move is to do the indexed rewrite
behind the gate and keep the file default — exactly ADR-0021's pre-committed direction, now unblocked by
evidence.

---

## 4. Integration risks + rough effort estimate

**Risks (highest first):**

1. **Promotion/privacy invariants are unverified for non-file adapters.** The shared conformance gate
   (`mod.rs` `assert_repository_replay_parity_conformance`) covers transcript/notes/graph replay incl.
   exact `valid_until_ms`, but promotion round-trip, audit-vs-current divergence, revoke guards, the
   silent-source-mutation guard, and the privacy floor are pinned by **file-adapter-only `#[test]`s**.
   Any DB adapter could silently violate the privacy floor (org-visible records must never carry
   `raw_transcript_text`, `speaker_names`, etc.). **Mitigation:** lift these into the shared gate first
   (the "no-regret refactor"). *High severity, pre-work.*
2. **Durability default churn.** SurrealKV/RocksDB sync defaults changed twice in ~6 months and were
   historically unsafe. **Mitigation:** set `SyncMode::Every` explicitly + a crash-recovery test in CI.
3. **Two on-disk generations.** gen-1 (legacy `.jsonl` + petgraph snapshot, `f64` seconds) and gen-2
   (event logs + materialized snapshots, `u64` ms) coexist and are reconciled by `last_sequence`. A swap
   must carry both. **Mitigation:** the file-canonical C model sidesteps this — the index is rebuilt from
   both, files stay authoritative.
4. **Disk-full UX parity loss.** The bespoke `CAPTURE_STORAGE_FULL` event + debounce + probe + corrupt-
   index quarantine (`io.rs:90`; `mod.rs` quarantine) is product-visible and a DB adapter loses it.
   **Mitigation:** C keeps the file write path (and its UX) authoritative.
5. **Remaining cross-platform unknowns:** native Windows runtime/durability, macOS build, RocksDB
   durability, stripped PE/Mach-O size — un-run. **Mitigation:** finish the `2b2c` Blacksmith matrix
   before any default change (not before the gated selectable rewrite).
6. **Beta engine / single-vendor longevity** (KuzuDB precedent). **Mitigation:** rebuildable-index
   architecture; prefer SQLite if the demand shape allows.

**Rough effort estimate (the recommended path — `48bb` keyed adapter as selectable, behind the gate):**

| Work item | Estimate |
|---|---|
| No-regret pre-work: hoist validation/privacy gates into trait defaults; extend shared conformance gate to promotion/redaction/org-knowledge | ~2–3 dev-days |
| Keyed-sequence rewrite of `surreal.rs` (drop O(n) scan; in-memory/keyed max; explicit `SyncMode::Every`) | ~2–4 dev-days |
| Streaming-frequency append benchmark in-repo (confirm parity holds in the real Mutex/writer-thread path) | ~1 dev-day |
| Finish `2b2c` Blacksmith matrix (native Win runtime+durability, macOS, RocksDB durability, PE/Mach-O size) | ~1–2 dev-days CI wiring |
| **Subtotal to a credible, conformance-passing, selectable (non-default) SurrealKV adapter** | **~6–10 dev-days** |
| *Full Option-C file-canonical index + dual-read + wire `build_graph_chat_context` onto it (deferred until demand signal)* | *~2–4 dev-weeks (out of scope now)* |

Promoting any DB to **default** is explicitly *not* in this estimate — it requires the schema freeze,
the closed gate, and a committed demand signal first.

---

## 5. Sources

**Internal (verified against the working tree, 2026-06-30):**
- ADR-0021 storage architecture (accepted-gated): `docs/adr/0021-storage-architecture.md`
- Design companion: `docs/designs/storage-architecture-decision-2026-06-27.md`
- `2b2c` Linux + Windows-cross-compile build/durability evidence: `docs/reviews/2b2c-local-linux-evidence-2026-06-27.md`
- `2b2c` append-throughput benchmark: `docs/reviews/2b2c-throughput-benchmark-2026-06-27.md`
- Trait + adapters: `src-tauri/src/persistence/mod.rs:431` (trait), `:160` (`append_jsonl`, O(1) hot path),
  `:2422` (`save_json` atomic fsync-before-rename); `src-tauri/src/persistence/surreal.rs:76` (spike adapter),
  `:104` (SCHEMALESS blob), `:153`/`:167` (O(n) per-append scan)
- Disk-full UX: `src-tauri/src/persistence/io.rs:90`
- Graph caps: `src-tauri/src/graph/temporal.rs:45` (1000 nodes), `:48` (5000 edges)
- Query surface: `src-tauri/src/graph/entities.rs:222` (`build_graph_chat_context`)
- Feature gate: `src-tauri/Cargo.toml:33` (`default = ["local-ml"]`), `:40` (`surrealdb-embedded`),
  `:225` (`surrealdb 3.1.4`, `kv-mem` only, optional)

**External (grounded, 2026-06-30):**
- SurrealDB license FAQ — https://surrealdb.com/license ; LICENSE (BSL 1.1, →Apache-2.0) —
  https://github.com/surrealdb/surrealdb/blob/HEAD/LICENSE ; embedded-deployment confirmation —
  https://github.com/surrealdb/docs.surrealdb.com/issues/1259
- SurrealKV beta status — https://surrealdb.com/docs/build/deployment ; engine + back-in-time caveat +
  Apache-2.0 — https://github.com/surrealdb/surrealkv (README + docs/ARCHITECTURE.md),
  https://crates.io/crates/surrealkv (0.21.2, Apache-2.0)
- Durability defaults: RocksDB sync-by-default — https://github.com/surrealdb/surrealdb/pull/6614 ;
  query-param + SurrealKV `sync=every` default — https://github.com/surrealdb/surrealdb/pull/6882 ;
  Rust SDK `.sync(SyncMode::Every)` — https://docs.rs/surrealdb/latest/surrealdb/struct.Connect.html
- Adversarial — durability footgun / corruption reports —
  https://blog.cf8.gg/surrealdbs-ch/ ("your instance is NOT crash safe")
- Single-vendor bus-factor (KuzuDB archived 2025-10-10) —
  https://www.theregister.com/software/2025/10/14/kuzudb_graph_database_abandoned/ ;
  https://github.com/kuzudb/kuzu (archived=true, v0.11.3 final)
- Crate versions (crates.io API, 2026-06-30): `surrealdb` max-stable **3.1.5** (newest 3.2.0-beta.2,
  ~1.05M downloads); `surrealkv` **0.21.2** (Apache-2.0)
