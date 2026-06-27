# Seed audio-graph-2b2c — Local Linux Evidence: SurrealDB File-Backed Engine Probe

**Date:** 2026-06-27
**Purpose:** Unblock ADR-0021's storage decision by measuring whether SurrealDB's
file-backed engines (`kv-surrealkv`, `kv-rocksdb`) build, link, fit, and persist
on Linux for the audio-graph Tauri app.
**Scope:** Linux leg ONLY. macOS + Windows are a later Blacksmith CI matrix.
**Isolation:** All builds ran in the throwaway worktree
`/mnt/e/cs/github/wt-ci-blacksmith` (branch `lane/wt-ci-blacksmith`, HEAD
`831cc30`) with a separate `CARGO_TARGET_DIR=/tmp/ag-2b2c-target`. The main
checkout was never touched. All Cargo.toml / lib.rs edits were throwaway and have
been reverted; the worktree is clean.

---

## IMPORTANT context discrepancy (read first)

The seed brief assumed the worktree already carried the `surrealdb` dependency at
`Cargo.toml:218` behind feature `surrealdb-embedded` with `kv-mem`. **It does
not.** At HEAD `831cc30` the worktree predates all SurrealDB wiring — its
`Cargo.toml` is 205 lines with no `surrealdb` dep and no `surrealdb-embedded`
feature, and there is no `src/persistence/surreal.rs`. The full SurrealDB wiring
the brief describes lives only in the **main checkout**
(`/mnt/e/CS/github/audio-graph`, `Cargo.toml:218`, `surrealdb-embedded` at line 40,
`src/persistence/surreal.rs`), which was added on a later commit than the
worktree's HEAD.

To run a faithful probe without touching the main checkout, I added the SurrealDB
dependency + `surrealdb-embedded` feature to the worktree's Cargo.toml as
throwaway edits (mirroring the main checkout's exact dep line, version `3.1.4`,
`default-features = false`), plus a tiny throwaway `surreal_probe_2b2c` module to
(a) force the crate to actually link into the lib and (b) host the durability
test. All of this was reverted at the end. The resolved crate version was
`surrealdb 3.1.5` (3.1.4 + a patch bump in the registry).

---

## Results table

| Engine | Compiles? | Links? | Build time (debug `--lib`, incremental on baseline) | Release binary size delta (stripped) | New native deps pulled | Durability |
|---|---|---|---|---|---|---|
| **baseline** (`cloud` only) | yes | yes | 1m 57s (clean) | — (37,154,664 B stripped) | — | n/a |
| **kv-surrealkv** | **yes** | **yes** | 2m 59s | **+1.17 MiB** (1,229,216 B) | `surrealkv 0.21.2`, `surrealmx 0.22.0`, `lz4` + `lz4-sys` (C, via `cc`) | **PASS** (cross-process restart) |
| **kv-rocksdb** | **yes** | **yes** | 5m 22s | **+6.68 MiB** (7,009,280 B) | `surrealdb-rocksdb 0.24.0-surreal.5` → `surrealdb-librocksdb-sys 0.18.3+11.0.0-4` (C++ RocksDB 11.0, via `cc`+`cmake`+`bindgen`), `bzip2-sys`, `zstd-sys`, `lz4-sys` | not separately tested (see notes) |

Notes on measurement method:
- **Build time** is wall-clock from `/usr/bin/time -v`. The two engine debug
  builds were *incremental* on top of the baseline's already-compiled shared
  crates (same target dir), so they reflect "cost added by the engine," not a
  cold build. RocksDB's 5m 22s is dominated by compiling the vendored C++ RocksDB
  source.
- **Size delta** is the honest, load-bearing number: it compares the **stripped
  release `audio-graph` binary** (which statically links the lib + all deps)
  built three ways. The `.rlib` delta was rejected as a proxy because release
  rlibs carry metadata (not the final linked engine code) and came out *smaller*
  for the engine builds than baseline — misleading. All three stripped binaries
  have distinct md5sums.

Raw (unstripped) release binary sizes for reference:
- baseline: 48,331,032 B
- kv-surrealkv: 50,529,512 B (+2,198,480 B)
- kv-rocksdb: 57,676,048 B (+9,345,016 B)

---

## Native-dependency findings (the subtle part)

The C/C++ toolchain (`cc`, `gcc`, `g++`, `cmake`, `clang`, `libclang-21`) was
**already present and already required by the baseline cloud build** — `cc`,
`cmake`, `bindgen`, and `clang-sys` all appear in the baseline tree, sourced from:
- `rsac`'s `pipewire` / `libspa-sys` (Linux audio) → `bindgen` + `clang-sys`
- `aws-lc-sys` (the AWS rustls crypto backend) → `cmake`

So **neither storage engine introduces a *new* toolchain class on Linux.** What
each engine adds:

- **kv-surrealkv:** pure-Rust engine crate (`surrealkv 0.21.2`) plus one C
  compression library, `lz4-sys` (compiled with the already-present `cc`). No
  cmake/bindgen demand of its own. This is the "pure Rust" engine the brief
  expected, and it lives up to it modulo lz4.
- **kv-rocksdb:** SurrealDB's vendored RocksDB fork
  (`surrealdb-rocksdb 0.24.0-surreal.5` → `surrealdb-librocksdb-sys
  0.18.3+11.0.0-4`, RocksDB 11.0). This compiles a large C++ codebase via
  `cc` + `cmake`, generates FFI bindings via `bindgen` (needs `libclang` at build
  time), and pulls three compression `-sys` crates: `bzip2-sys`, `zstd-sys`,
  `lz4-sys`. It uses SurrealDB's own `surrealdb-librocksdb-sys`, NOT the
  upstream `librocksdb-sys`.

---

## Durability probe (kv-surrealkv) — PASS

Because SurrealKV holds an exclusive on-disk `LOCK`, a same-process reopen is not
a valid durability test (the first attempt failed with "Database ... LOCK is
already locked by another process" — which itself confirms data hit disk). I
therefore drove the test across **two separate process invocations** of the test
binary (the real kill/restart proxy):

1. **Writer process:** opened a file-backed `SurrealKv` store at
   `/tmp/ag-2b2c-surrealkv-durability`, wrote 3 rows, exited. On-disk layout left
   behind: `LOCK`, `manifest/`, `sstables/`, `vlog/`, `wal/` (an LSM-tree store).
2. **Reader process (fresh invocation = "restart"):** reopened the same path and
   read back **all 3 rows**. Test asserted `rows.len() == 3` — **passed.**

This is genuine cross-process restart durability, not a fabricated claim. (I used
`serde_json::Value` for record content, mirroring the main checkout's
`surreal.rs` encode path, because SurrealDB 3.1.5 requires `SurrealValue` and
`serde_json::Value` satisfies it.)

**kv-rocksdb durability was NOT separately tested.** It compiled and linked; I did
not run a RocksDB restart test (the probe module's durability test targets the
SurrealKv type, and RocksDB's persistence is well-established upstream). This is an
honest gap for the Blacksmith matrix to close if RocksDB stays in contention.

---

## Verdict (Linux leg)

**Both engines are viable on Linux. SurrealKV is the clearly lighter, lower-risk
choice:** it builds ~2.4x faster, adds ~1.2 MiB stripped (vs RocksDB's ~6.7 MiB),
pulls only one C compression lib (lz4) with no new toolchain demand, and **passed
a real cross-process restart durability test.** RocksDB is viable but costs a
multi-minute C++ build, a `bindgen`/`libclang` build-time requirement, three
compression `-sys` crates, and ~5.5x the binary growth — justified only if its
write throughput / compaction story is needed.

---

## What the macOS / Windows Blacksmith matrix still must confirm

1. **kv-rocksdb build+link on macOS and (especially) Windows.** The C++ RocksDB
   compile + `bindgen`/`libclang` requirement is the highest cross-platform risk.
   Windows + bindgen + a vendored C++ tree is exactly where these probes fail;
   this Linux PASS does NOT predict Windows.
2. **kv-surrealkv on Windows** — verify lz4-sys (`cc`) compiles cleanly under MSVC
   and that the SurrealKV `LOCK`/`wal`/`sstables` on-disk layout behaves on NTFS
   (file-locking semantics differ).
3. **macOS** — confirm both engines build under the Xcode/Metal-enabled config
   already used for the local-ML deps, and that nothing conflicts with the
   existing `aws-lc-sys` cmake usage.
4. **kv-rocksdb durability** — run the cross-process restart test for RocksDB
   (skipped here) on at least one platform.
5. **Stripped-binary size deltas on macOS/Windows** — the +1.2 / +6.7 MiB figures
   are Linux-only; Windows PE / macOS Mach-O linking will differ.
