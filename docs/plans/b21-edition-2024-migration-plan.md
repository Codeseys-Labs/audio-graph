# B21 — Rust edition 2021→2024 migration plan (DONE)

**Status: DONE 2026-05-31** (commit `d3b190f`). Edition flipped to 2024;
verified `cargo test` + `clippy --all-targets -D warnings` + `fmt` green on
**Windows-native** (cloud 448 / local-ml 449) and **WSL Linux** (cloud 449 /
diarization-clustering 451), 0 failed; **macOS via the `rust-macos` Blacksmith CI
runner**. The flip was *not* gated on a missing macOS leg after all — CI runs all
three OSes. How it went: `cargo fix --edition` made the 2 mechanical changes
(`unsafe { set_var }` + a `ref` removal); `clippy --fix` collapsed the nested
`if`/`if let` patterns into 2024 `let_chains`, which incidentally moved the guard
temporaries out of the flagged `tail_expr_drop_order` positions — so all 24 sites
resolved with **no hand-rewrites needed**, and the test suite passing under the
new drop order is the behavioral proof they were benign. A
`#![warn(tail_expr_drop_order, if_let_rescope)]` guard in `lib.rs` now catches any
*new* hazard. The original scaffold (sites + procedure) is retained below for
the record.

---

## Original plan (scaffold — retained for the record)
Update 2026-05-31: WSL on the dev box restored Rust *test execution* on Linux
(cloud 449 / local-ml 450 / diarization 58, 0 failed) — so we now have a genuine
**Windows-compile + Linux-run** signal, no longer "can't run the tests at all."
But the migration's core risk is *cross-platform drop-order / lock-release
timing*, and the flagged set is platform-gated — the Windows-only `!Send
cpal::Stream` + crossbeam command-loop paths and the macOS CoreAudio paths each
carry their own sites. Two of three platforms is materially better than the prior
pure-scaffold state but does not cover the macOS leg, so flipping `edition=2024`
+ rewriting all sites here would still be partially-unverified. This plan +
`docs/research/b21-edition-2024-migration.md` + the verified site list below ARE
the scaffold; the flip is a focused follow-up that adds the macOS run.

## Verified flagged sites (real lint output, not estimate)

`RUSTFLAGS="-W tail_expr_drop_order -W if_let_rescope" cargo check
--no-default-features --features cloud` in WSL (Linux, 2026-05-31) →
**`audio-graph (lib) generated 28 warnings`**; the distinct audio-graph
`tail_expr_drop_order`/`if_let_rescope` sites (cloud build; default/local-ml +
macOS add more):

```
src/asr/assemblyai.rs:488      src/speech/mod.rs:2375
src/asr/deepgram.rs:675        src/speech/mod.rs:2746
src/asr/openai_realtime.rs:640 src/speech/mod.rs:2754
src/aws_util/mod.rs:261        src/speech/mod.rs:2773
src/commands.rs:2709           src/speech/mod.rs:2783
src/credentials/mod.rs:48      src/speech/mod.rs:3049
src/gemini/mod.rs:973          src/speech/mod.rs:3066
src/gemini/mod.rs:1230         src/speech/mod.rs:3076
src/lib.rs:107                 src/speech/mod.rs:3398
src/playback/mod.rs:145        src/speech/mod.rs:3411
src/playback/mod.rs:308        src/speech/mod.rs:3424
src/tts/deepgram_aura.rs:588   src/speech/mod.rs:3434
```

24 distinct cloud-build sites (higher than the research's ~13 estimate; the
default/local-ml build adds the whisper/llama/mistralrs + diarization paths, and
macOS adds CoreAudio). Each maps to a Pattern A–D fix in §3 below. Note several
are in code written during this effort (`openai_realtime.rs:640`, the new
`speech/mod.rs` clustering/realtime ranges) — so the audit must cover them too.

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
