# Deferred-with-cause ledger — 2026-05-30 drive-to-zero loop

Every backlog item NOT closed in this loop, with its **specific cause** and the
**trigger that would unblock it**. Per the deep-work-loop discipline: nothing is
silently skipped — each entry is deferred for a documented, verifiable reason.

This is the honest counterpart to the "completed" list in `deep-work-log.md`.

## Update 2026-05-31 — rsac v0.4.0 unblocked the rsac-pin item

- **B32-rsac-pin — RESOLVED** (`bc41a39`). rsac cut **v0.4.0** (tag `a2d3088`),
  which formally `#[deprecated]`s `get_default_device()` for `default_device()`
  (both exist) and keeps the capture enums `#[non_exhaustive]`. `capture.rs` was
  migrated to the clean form (dropped all 3 version-skew `#[allow]`s) and CI's
  `RSAC_REPO_SHA` bumped to the v0.4.0 tag, in one atomic commit. Local gate green
  (fmt + clippy `--all-targets -D warnings` on cloud/local-ml/diarization-clustering
  against the real 0.4.0 checkout); the multi-OS CI run of the new pin is the only
  residual, and it's a tagged release so it's stable.
- **Was a "repeat the deep-dive" needed?** No. rsac is the *capture* layer only —
  it does not touch the diarization (B16) or realtime (B15) research, so a 0.4.0
  bump cannot invalidate those. 0.4.0 is an rsac-*documented* coordinated migration,
  not an API surprise.
- **Newly surfaced (now-actionable, tracked):** **B35** (wire rsac 0.4.0's new
  `backpressure_report()` windowed-drop view into the UI signal) and **B36** (the
  contained Rust 0.x-major bumps — ringbuf 0.5 / rubato 3 / sysinfo 0.39 — split
  out of B32 Phase C as locally-attemptable, unlike the framework majors).

---

## Runtime-gated (built + CI-typed; need a key / model / hardware to validate)

These shipped as code (clippy + fmt + compile green; unit-tested where possible)
but their *runtime* behavior cannot be exercised in a headless, key-less,
model-less, audio-deviceless loop. The build+CI-verify-flag-runtime decision
(2026-05-30) governs them.

| Item | Built | Runtime gate (what's needed to validate) |
|------|-------|------------------------------------------|
| **B15** OpenAI Realtime STT | client + parser + reconnect + settings + dispatch; 8 verbatim-JSON parser tests | a live `OPENAI_API_KEY` + network — to validate the real WS handshake, transcript streaming, reconnect against a live socket |
| **B16** diarization engine+worker+downloads | `LiveDiarizationWorker`, `SpeakerEmbeddingExtractor` stream, model downloaders, pure glue tests **+ model-load/pipeline VALIDATED 2026-05-31** (real pyannote-seg + TitaNet ONNX downloaded into the app cache; `ClusteringDiarizer::new()` loads them, `sample_rate()==16000`, `diarize()` runs the full ONNX pipeline — test `constructs_and_runs_against_real_models`, executed in WSL) | **Only `num_speakers > 4` ACCURACY remains** — needs a *curated/labeled* multi-speaker 16 kHz clip (a data-collection task, not code/env). Construction + inference are now proven on real models. Threshold tuning (`clustering.threshold`/`sim_threshold`) wants the same clip. |
| **B16-pipe** worker→pipeline wiring | (Wave 3a) spawn + 16k tap + time-offset + SPEAKER_DETECTED — unit-tests executed green in WSL | accuracy only (same labeled-clip gate as B16) |
| **B18** native S2S (Gemini AUDIO + turn FSM) | (Wave 3a) AUDIO config + event decode + pure FSM with gating | a live Gemini key + audio playback device — real barge-in, AEC, audio-out |
| **B33** B15 commit-cadence / Connected semantics | n/a (tuning) | a live key — to measure per-chunk-vs-per-utterance commit cost / 429 behavior |

**Note (UPDATED 2026-05-31):** the Rust tests are no longer execution-gated. The
Windows `STATUS_ENTRYPOINT_NOT_FOUND` (MSVC 14.50↔14.51 CRT skew) is real but
**worked around via WSL Ubuntu on the same box** — the full suite now *executes*
locally on Linux: **cloud 449 ✓ · local-ml(default) 450 ✓ · diarization-clustering
58 ✓ · 0 failed** (`scripts/run-rust-tests-wsl.sh`, `docs/ops/windows-rust-test-crt-skew.md`,
B23/2.7 below). So the B15/B16/B18 "Built" columns above are now **built + unit-tests
executed (Linux)**; only their *true runtime* (live keys / real models+audio) remains
gated — that's the narrower, honest residual.

---

## Genuinely deferred (a hard blocker or absent precondition)

| Item | Cause (why it cannot be done now) | Unblock trigger |
|------|-----------------------------------|-----------------|
| **B21** edition 2021→2024 | The 22 `tail_expr_drop_order` sites change lock-release / channel-disconnect *timing*, and the flagged set is **platform- and feature-dependent** (22 on default-feature, ~13 on cloud-only; Windows-only `!Send cpal::Stream` paths add their own). A single-host loop cannot prove the rewrite is behavior-preserving on Linux/macOS. Research (`docs/research/b21-edition-2024-migration.md`) confirms `cargo fix` does NOT auto-fix this lint. | The `{linux,windows,macos} × {default, cloud, +diarization}` CI matrix available to run `cargo test` per combo after the per-site Pattern A–D rewrites land. Wave 3b scaffolds the audited rewrites + gates the `edition = "2024"` flip behind that matrix. |
| **B22** ADR-0012 Phase 1/2 (streaming-partial prefill) | **Phase 0b proven infeasible 2026-05-30**: LFM2-350M-Extract is a hybrid *recurrent* model (`llama_memory_recurrent`); KV-sequence-removal ("drop the turn") is unsupported on recurrent memory, so warm-reuse decodes turn 1 but fails turn 2. Phase 1/2 (streaming-partial overlap + telemetry gating) is coupled to an **active streaming ASR**. | (a) A non-recurrent extraction GGUF replaces LFM2 → unblocks Phase 0b; AND/OR (b) a streaming ASR (B15/B16) is runtime-live → unblocks the Phase 1 overlap, which must still be telemetry-gated to prove it beats the simple full-finalized-transcript path. |
| **B25** UX W4.3 RTL groundwork | No RTL locale exists or is planned — only `en` + `pt`, both LTR. Logical-properties + `dir` wiring with no RTL locale to exercise it is untestable speculation. | A real RTL locale (e.g. `ar`/`he`) is added to `src/i18n/locales/`. Then: swap physical CSS props for logical (`margin-inline`, `padding-inline`, `inset-inline`), wire `dir` from the active locale, and verify against the RTL locale. |
| **B23 / 2.7** Windows full `cargo test` harness | **DIAGNOSED + WORKED AROUND 2026-05-31.** The `STATUS_ENTRYPOINT_NOT_FOUND` (0xC0000139) is a VC++ toolset/CRT skew: the harness is linked by MSVC **14.50** (a preview toolset) but System32 ships **14.51** vcruntime/ucrtbase — an ABI gap in the vcruntime→ucrtbase chain that app-local CRT copying can't bridge (full diagnosis: `docs/ops/windows-rust-test-crt-skew.md`). **Local test EXECUTION is now restored via WSL Ubuntu on the same box** (`scripts/run-rust-tests-wsl.sh`): cloud **449 passed**, diarization-clustering **58 passed**, 0 failed (2026-05-31) — so every Rust test written across this effort is now genuinely *executed*, not just clippy/compiled, AND we have a two-platform signal (Windows-compile + Linux-run). | The remaining *Windows-native* `cargo test` needs a host toolchain repair (link with the stable 14.44 BuildTools toolset, or install the matching 14.50 VC++ redist, or remove the 14.50 preview) — a system-config action, not a code fix. No longer blocks local verification: WSL covers it. |

---

## Deferred-by-design (low ROI / deliberately last)

| Item | Cause | Unblock trigger |
|------|-------|-----------------|
| **B23 / 2.11** Trim Tailwind default theme | `@import "tailwindcss/theme.css"` emits ~7 KB of unused default vars; `@theme { --*: initial; }` reclaims it but **risks silently dropping theme-derived utilities** (spacing/color scales some components rely on). Net 7 KB on a desktop app is low ROI vs the regression surface. | A bundle-size pass where 7 KB matters, with a full visual-regression sweep to confirm no utility breaks. (`build:analyze` already wired for measurement.) |
| **B32 majors** (tauri, reqwest, …) | Each major-version bump needs its own migration + the all-platform CI matrix (same constraint as B21). Genuinely multi-session. The rsac-hygiene + safe-minors halves (B32 Phase A/B) ARE actionable in Wave 3b. | The CI matrix + per-major migration review. Wave 3b lands rsac-hygiene + minors; majors are scaffolded/deferred-with-cause. |

---

## Cannot be closed by engineering (external)

| Item | Cause | Who closes it |
|------|-------|---------------|
| **B26** release signing certs | The CI plumbing is **complete** (`release.yml` forwards all 10 secrets; every secret's generation is documented in `docs/RELEASE.md`). The residual is *procurement*: enroll in the Apple Developer Program ($99/yr), buy an Authenticode cert (~$300–500/yr), paste the secrets. No code change exists to make. | A human with a budget + the two developer accounts. Then artifacts sign automatically on the next tagged release. |
