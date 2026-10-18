# Decision 3.0 native backend commit state — 2026-10-18

## Repository state

- Worktree: `C:\Users\bbala_n314ugx\.ao\data\worktrees\audio-graph\audio-graph-3`, branch `ao/audio-graph-3/root`.
- Base: `origin/master` `a6ea3f8`, plus the cherry-picked orchestrator Seeds commit `122fcd1` (as `f1d4910`).
- Merged `ao/audio-graph-2/root` `3c34cda` (decision-engine skeleton and Laya backend, still unmerged on master)
  as `f41abf6`, because the d3 backend plugs into that skeleton. The merge deduplicated eight Seed rows that
  both branches added (kept the newest row per id; `audio-graph-60c7` kept this branch's status and evidence).
- Commits: `158545f` (Phase A probe, fixtures, design note), `f734c42` (Seeds), `97bc66b` (fused Gated DeltaNet
  kernels), `f41abf6` (merge), `5245dfc` (D3Engine), `26a1e1d` (Seeds: 60c7 closed), `ad3b925` (this note),
  then the registry rework requested by the decision-engine owner: `lib.rs` is back to the skeleton except
  the feature-gated `pub mod d3`; `d3::register` adds the engines through `EngineRegistry::register`, called
  from `create_decision_engine`; the download table (URLs, files) moved into `src-tauri/src/models/mod.rs`.
- The tree is clean after these commits. Nothing is pushed and no PR is open: publishing is not authorized,
  and the orchestrator integrates this branch with the skeleton branch and the `decision-contract` crate
  (`ao/audio-graph/root`).
- The global `sd` 0.5.5 reads `E:\CS\github\audio-graph\.seeds`; every Seed mutation here mirrored this
  worktree's `issues.jsonl` to that checkout, ran `sd`, copied the result back, and restored the checkout.

## Active work

- Closed: `audio-graph-60c7` (S1-T4) with evidence in its `phaseA` / `phaseB` extensions.
- Follow-ups under epic `audio-graph-6dfb`: `audio-graph-1f5c` (CPU tuning), `audio-graph-377a` (Metal kernel and
  macOS parity), `audio-graph-26b4` (Vulkan-only GPUs).
- Documents: `docs/designs/d3-native-runtime-2026-10-18.md`, `docs/research/d3-native-latency-2026-10-18.md`.

## Verification

Run in `src-tauri` with Rust 1.95.0 (CUDA runs inside an MSVC developer shell with CUDA 13.2 on PATH and
`NVCC_APPEND_FLAGS=-Xcompiler /Zc:preprocessor`):

- `cargo test -p audio-graph-decision-engine --features decision-d3`: 8 passed.
- `cargo test -p audio-graph-decision-engine --features decision-d3,cuda --lib`: 9 passed.
- `cargo test --release -p audio-graph-decision-engine --features decision-d3 --test d3_parity -- --ignored`
  (CPU, `AUDIOGRAPH_MODELS_DIR` set): d3-edge and d3-nano passed.
- Same with `--features decision-d3,cuda` and `AUDIOGRAPH_D3_EP=cuda`: all four sizes passed on the RTX 5090.
- `cargo clippy -p audio-graph-decision-engine --all-targets -- -D warnings` for the default, `decision-d3`,
  `decision-d3,cuda` and `decision-laya,decision-d3` feature sets: clean.
- `cargo check --locked --no-default-features --features cloud` and the same with `decision-d3`: passed;
  `cargo clippy --locked --no-default-features --features cloud -- -D warnings`: clean; candle is absent from
  the cloud dependency tree.
- `AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1 cargo test --locked --no-default-features --features cloud --lib models::`:
  15 passed (without the env var the harness hits the documented `STATUS_ENTRYPOINT_NOT_FOUND`,
  `docs/ops/windows-rust-test-crt-skew.md`).

## Change boundary

Backend only: the `decision-engine` crate, four model-manager entries, `Cargo.lock` (candle and its optional
CUDA/Metal dependencies), CHANGELOG, docs and Seeds. No frontend, workflow, credential or default-feature
change; `decision-d3` stays opt-in.
