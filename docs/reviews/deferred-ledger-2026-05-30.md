# Deferred-with-cause ledger — 2026-05-30 drive-to-zero loop

Every backlog item NOT closed in this loop, with its **specific cause** and the
**trigger that would unblock it**. Per the deep-work-loop discipline: nothing is
silently skipped — each entry is deferred for a documented, verifiable reason.

This is the honest counterpart to the "completed" list in `deep-work-log.md`.

---

## Runtime-gated (built + CI-typed; need a key / model / hardware to validate)

These shipped as code (clippy + fmt + compile green; unit-tested where possible)
but their *runtime* behavior cannot be exercised in a headless, key-less,
model-less, audio-deviceless loop. The build+CI-verify-flag-runtime decision
(2026-05-30) governs them.

| Item | Built | Runtime gate (what's needed to validate) |
|------|-------|------------------------------------------|
| **B15** OpenAI Realtime STT | client + parser + reconnect + settings + dispatch; 8 verbatim-JSON parser tests | a live `OPENAI_API_KEY` + network — to validate the real WS handshake, transcript streaming, reconnect against a live socket |
| **B16** diarization engine+worker+downloads | `LiveDiarizationWorker`, `SpeakerEmbeddingExtractor` stream, model downloaders, pure glue tests | real pyannote-seg + TitaNet ONNX models + multi-speaker audio — to validate `num_speakers > 4` + tune `clustering.threshold`/`sim_threshold` |
| **B16-pipe** worker→pipeline wiring | (Wave 3a) spawn + 16k tap + time-offset + SPEAKER_DETECTED | same as B16 — real models + audio for end-to-end |
| **B18** native S2S (Gemini AUDIO + turn FSM) | (Wave 3a) AUDIO config + event decode + pure FSM with gating | a live Gemini key + audio playback device — real barge-in, AEC, audio-out |
| **B33** B15 commit-cadence / Connected semantics | n/a (tuning) | a live key — to measure per-chunk-vs-per-utterance commit cost / 429 behavior |

**Note:** every Rust test in this loop is *compile + clippy* verified locally but
its **execution** is blocked on this Windows host by `STATUS_ENTRYPOINT_NOT_FOUND`
(0xC0000139) — a pre-existing MSVC CRT / system-DLL version skew (System32 v14.51
vs VS-linked v14.50) that aborts the test binary at OS load before any Rust runs.
CI (Linux) is the authoritative test-execution gate (ADR-0007). This is **B23/2.7**
below — an environment fix, not a code fix.

---

## Genuinely deferred (a hard blocker or absent precondition)

| Item | Cause (why it cannot be done now) | Unblock trigger |
|------|-----------------------------------|-----------------|
| **B21** edition 2021→2024 | The 22 `tail_expr_drop_order` sites change lock-release / channel-disconnect *timing*, and the flagged set is **platform- and feature-dependent** (22 on default-feature, ~13 on cloud-only; Windows-only `!Send cpal::Stream` paths add their own). A single-host loop cannot prove the rewrite is behavior-preserving on Linux/macOS. Research (`docs/research/b21-edition-2024-migration.md`) confirms `cargo fix` does NOT auto-fix this lint. | The `{linux,windows,macos} × {default, cloud, +diarization}` CI matrix available to run `cargo test` per combo after the per-site Pattern A–D rewrites land. Wave 3b scaffolds the audited rewrites + gates the `edition = "2024"` flip behind that matrix. |
| **B22** ADR-0012 Phase 1/2 (streaming-partial prefill) | **Phase 0b proven infeasible 2026-05-30**: LFM2-350M-Extract is a hybrid *recurrent* model (`llama_memory_recurrent`); KV-sequence-removal ("drop the turn") is unsupported on recurrent memory, so warm-reuse decodes turn 1 but fails turn 2. Phase 1/2 (streaming-partial overlap + telemetry gating) is coupled to an **active streaming ASR**. | (a) A non-recurrent extraction GGUF replaces LFM2 → unblocks Phase 0b; AND/OR (b) a streaming ASR (B15/B16) is runtime-live → unblocks the Phase 1 overlap, which must still be telemetry-gated to prove it beats the simple full-finalized-transcript path. |
| **B25** UX W4.3 RTL groundwork | No RTL locale exists or is planned — only `en` + `pt`, both LTR. Logical-properties + `dir` wiring with no RTL locale to exercise it is untestable speculation. | A real RTL locale (e.g. `ar`/`he`) is added to `src/i18n/locales/`. Then: swap physical CSS props for logical (`margin-inline`, `padding-inline`, `inset-inline`), wire `dir` from the active locale, and verify against the RTL locale. |
| **B23 / 2.7** Windows full `cargo test` harness | `cargo test` aborts on Windows with `STATUS_ENTRYPOINT_NOT_FOUND` (0xC0000139) — native-ML link + MSVC CRT/system-DLL skew (ADR-0007). This is an **environment/toolchain** fix, not a source fix — no agent editing `src/` can resolve it. A subset runs via `scripts/run-core-tests.ps1`. | A matched MSVC CRT on the host (or a CI Windows runner with a clean toolchain), OR a cloud-only test job on Windows (which links clean). Gates local execution of ALL Rust tests written this loop. |

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
