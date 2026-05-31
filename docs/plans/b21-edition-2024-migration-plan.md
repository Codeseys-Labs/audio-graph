# B21 — Rust edition 2021→2024 migration plan (scaffolded; flip gated on CI matrix)

**Status:** scaffolded, **NOT executed** in the drive-to-zero loop. The migration
is **gated on an all-platform CI matrix** this single Windows host (which cannot
even *run* the Rust test binary — `STATUS_ENTRYPOINT_NOT_FOUND`, ADR-0007) cannot
provide. Producing the rewrites here without cross-platform test execution would
be unverifiable theater — the entire risk of this migration is *cross-platform
drop-order behavior*. This plan + `docs/research/b21-edition-2024-migration.md`
ARE the scaffold; execution is a focused CI-equipped follow-up.

## Why gated (not deferred-without-cause)

Rust 2024's `tail_expr_drop_order` change moves when a tail-expression temporary
with a *significant* `Drop` (MutexGuard / RwLockGuard / last `Sender` / pinned
future) is dropped — **before** the block's named locals instead of after. For
this codebase that means lock-release / channel-disconnect *timing* can reorder.
The lint is **allow-by-default with no machine-applicable fix** — `cargo fix
--edition` will NOT rewrite the 22 sites; they need manual, per-site review. And
the **flagged set is platform/feature-dependent** (22 on default-feature, ~13 on
cloud-only; Windows-only `!Send cpal::Stream` paths add their own), so a rewrite
verified on one OS can change behavior on another.

## Verified flagged sites (from research, default-feature build = 22)

The dominant shape is `if let Ok(mut guard) = lock.{read,write}() { … }` in tail/
match-arm position alongside another guard local. Per-site fix patterns (A–D) are
in `docs/research/b21-edition-2024-migration.md` §3:

| Site | Shape | Fix pattern |
|------|-------|-------------|
| `commands.rs:2677` | `if let Ok(mut gs) = graph_snapshot.write()` tail w/ `graph` MutexGuard local | C (`if let`→`match` or bind-first) |
| `speech/mod.rs:1992,2363,2371,2390,2400` | `if let Ok(mut status) = pipeline_status.write()` status updates | C |
| `gemini/mod.rs:830,1087` | awaited `open_ws(...)` reconnect tail w/ pinned `sleep` | D (bind the awaited result) |
| `tts/deepgram_aura.rs:588` | awaited `open_ws(...)` tail | D |
| `asr/assemblyai.rs:488`, `asr/deepgram.rs:675`, `aws_util/mod.rs:261`, `playback/mod.rs:308` | assorted significant-Drop tail temporaries | A (hoist to `let`) |
| (+ default-feature-only sites in whisper/llama/mistralrs/diarization paths) | — | per §3 |

Two auto-fixable non-drop changes also surface (handled by `cargo fix --edition`):
`ref` in implicitly-borrowing patterns (`speech/mod.rs:1093`) and now-`unsafe`
`std::env::set_var` (`gemini/mod.rs:791`).

## Execution procedure (for the CI-equipped session)

1. Branch; add `#![warn(tail_expr_drop_order, if_let_rescope)]` to the crate roots
   temporarily; run `cargo check` **per feature × per OS** to surface the full set
   (the count differs by combo).
2. `cargo fix --edition` per feature set + per target (it only sees code that
   compiles under the active cfg). Expect the `if_let_rescope` auto-rewrites in the
   diff; the `tail_expr_drop_order` warnings REMAIN and need the manual §3 fixes.
3. Apply Pattern A–D per site (hoist to `let` / explicit `drop(guard)` / `if let`→
   `match` / bind-awaited-future). Each rewrite is **edition-stable** (named locals
   drop in reverse-decl order on BOTH editions), so the diff is reviewable as
   behavior-preserving.
4. Flip `edition = "2021"` → `"2024"` in `src-tauri/Cargo.toml` (manual; `cargo
   fix` never does this). Toolchain is 1.95 — OK (2024 stable since 1.85).
5. **Gate:** `cargo build && cargo test` on the FULL matrix
   `{linux, windows, macos} × {default(local-ml), cloud, +diarization,
   +diarization-clustering}`. Windows runs the cloud-only `cargo test` + the
   `scripts/run-core-tests.ps1` subset (full ML test harness is the ADR-0007 /
   B23-2.7 CRT issue). Watch the two-guard blocks, last-`Sender`-drop disconnects,
   and the `!Send cpal::Stream` Windows path specifically.
6. Once green on all combos, add `#![deny(tail_expr_drop_order, if_let_rescope)]`
   to the crate roots as a permanent regression guard.

## Acceptance (for the eventual execution)

- All 22 (+ feature-specific) sites rewritten Pattern A–D, each diff reviewable as
  behavior-preserving; `edition = "2024"`; full CI matrix green; `#![deny(...)]`
  guard added. Until then this stays scaffolded — see
  `docs/reviews/deferred-ledger-2026-05-30.md`.
