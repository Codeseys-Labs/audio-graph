# Seed 2b2c — SurrealKV append-throughput benchmark (ADR-0021 fit-criterion #1)

**Date:** 2026-06-27 · **Platform:** Linux (WSL2, kernel 6.18.33.1-microsoft-standard-WSL2), x86_64 · **Toolchain:** rustc/cargo 1.95.0 · **Mode:** in-process, single-threaded, release build (`opt-level=3`)

**Throwaway:** measured in `/tmp/ag-bench` (a scratch crate outside the repo). Nothing in `src-tauri/` was touched; nothing committed. This file is the only artifact left in the repo.

## What was measured

The load-bearing claim in ADR-0021 (seed 2b2c) is that the SurrealKV-backed repository does an **O(n) full-table scan on every append** (`surreal.rs::next_session_sequence` → `select_all` → filter → max → `create`), and that this regresses the streaming-transcript hot path versus the file repository's **O(1) JSONL append** (`mod.rs::append_jsonl` = `OpenOptions` append + `serde_json` + `\n` + `sync_all`).

Three append paths, each appending transcript-event-shaped records (field set + types mirror `projections::TranscriptEvent`; ~12-word transcript line per record):

1. **`file_jsonl`** — exact replica of `append_jsonl` (append-open, write line, flush, `sync_all` fsync per append). O(1).
2. **`surrealkv_onscan`** — file-backed SurrealKV (`Surreal::new::<SurrealKv>(path)`, surrealdb 3.1.5 / kv-surrealkv 0.21.2), replicating `append_session_value` exactly: full-table `select` → decode all rows → filter by session → max sequence → `create`. This is the adapter's actual behavior. O(n) per append.
3. **`surrealkv_keyed`** — same file-backed SurrealKV store, but the max sequence is held in memory (no per-append scan) before the `create`. This models the ADR's "Wave 3 indexed/keyed rewrite". O(1) scan cost; same `create` + durability cost as path 2.

## Results

| Path | N | Total time | Mean / append | p50 | p99 |
|------|---|-----------|---------------|-----|-----|
| `file_jsonl` (O(1)) | 1,000 | 3.19 s | 3.18 ms | 3.37 ms | 5.61 ms |
| `file_jsonl` (O(1)) | 10,000 | 32.65 s | 3.26 ms | 3.48 ms | 5.60 ms |
| `surrealkv_onscan` (O(n)) | 1,000 | 7.96 s | 7.74 ms | 7.40 ms | 14.00 ms |
| `surrealkv_onscan` (O(n)) | 10,000 | **728.33 s** | **66.78 ms** | 61.84 ms | **222.56 ms** |
| `surrealkv_keyed` (O(1) scan) | 1,000 | 3.38 s | 3.37 ms | 3.56 ms | 4.97 ms |
| `surrealkv_keyed` (O(1) scan) | 10,000 | 35.35 s | 3.53 ms | 3.62 ms | 5.72 ms |

### Slowdown ratio (O(n) SurrealKV vs file JSONL)

| N | onscan mean | file mean | **slowdown** |
|---|-------------|-----------|--------------|
| 1,000 | 7.74 ms | 3.18 ms | **2.4×** |
| 10,000 | 66.78 ms | 3.26 ms | **20.5×** |

The ratio is not a constant — it **grows with table size**, which is the signature of the O(n)-per-append scan: total work over N appends is O(n²). At N=1,000 the scan adds ~2.4×; at N=10,000 it adds ~20.5× (mean) and the p99 blows out to **222 ms** (40× the file p99 of 5.6 ms). The onscan p99 at 10k is the worst number here and the one that matters for a hot path — a single late append stalls a quarter of a second.

### What the keyed rewrite buys

`surrealkv_keyed` at N=10,000 lands at **3.53 ms mean / 5.72 ms p99** — statistically indistinguishable from the file path (3.26 ms / 5.60 ms), and flat from N=1k to N=10k (no growth). Dropping the full-table scan recovers essentially all of the regression: the slowdown vs file collapses from 20.5× back to ~1.08×.

## Caveats (read before trusting the absolute numbers)

- **fsync-bound floor.** The ~3.2 ms/append floor for `file_jsonl` is the cost of `sync_all` to the WSL2 ext4 VHD (`/dev/sdd`), not CPU. Both file and keyed paths pay this identical durability cost — that is *why* they match. On hardware with faster durable writes the absolute numbers shrink, but the **relative** story (onscan grows O(n²), keyed stays flat) is structural and platform-independent.
- **In-process, single-thread, no contention.** Real ASR streaming runs the adapter behind a `Mutex` with concurrent readers; that only makes the long onscan critical section worse, not better.
- **Linux-only.** No macOS/Windows leg here, as scoped. The O(n²) growth is an algorithmic property, not an OS property, so it generalizes; the fsync floor does not.
- **SurrealKV `create` cost is real but small.** Comparing keyed (3.53 ms) to file (3.26 ms) at 10k, SurrealKV's own write/index overhead beyond fsync is only ~0.3 ms — negligible next to the scan.

## Verdict

**CONFIRMED, with a rescue.** The O(n)-full-scan-per-append in `surreal.rs` does regress the streaming hot path — 2.4× at 1k rows growing to 20.5× mean / ~40× p99 at 10k, and worsening with session length (O(n²) total) — so the hybrid-gated fear in ADR-0021 is real for SurrealKV **as currently written**. But it is an adapter-algorithm defect, not a SurrealKV-engine defect: replacing the per-append full scan with an in-memory/keyed max sequence (the planned Wave-3 rewrite) brings SurrealKV to parity with the O(1) file append (within ~8%, and flat with table size). SurrealKV is **not** fundamentally too slow for the hot path — it is fine after the keyed rewrite, which makes "lean in" viable once that rewrite ships.
