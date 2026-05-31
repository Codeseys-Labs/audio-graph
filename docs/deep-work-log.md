# Deep Work Log

## Run 2026-05-30 13:35 — started at 794db72

Baseline: working tree clean, all CI green across Linux/macOS/Windows.

Goal: drive the backlog to zero via research → architect → execute → review loop.

### Phase 1 — Commit state
- HEAD `794db72` (docs(adr-0017): architect unbounded speaker diarization).
- Recent landed work (CI-green): persistent-context LLM actor (ADR-0012 Phase 0a),
  local extraction made functional (ChatML + generate-then-validate), Deepgram
  max_speakers UI, a11y + lint ratchets at `error`, clippy `-D` enforced, Radix
  Tooltip pilot.

### Phase 2 — Backlog (initial)
- B1 (XL): ADR-0017 unbounded diarization — fix stale `sherpa_streaming.rs`,
  feature-gate ORT conflict, sherpa-onnx clustering backend, model downloads,
  offline-window integration, UI selector.
- B2 (M): `src/asr/sherpa_streaming.rs` is broken vs sherpa-onnx 1.12 (10 errors)
  — prerequisite for B1; the `sherpa-streaming` feature won't compile.
- B3 (M): edition 2021→2024 (deferred — 22 `tail_expr_drop_order` sites need
  per-site review).
- B4 (S/M): broader Radix headless adoption (dropdown/popover) per
  `docs/reviews/tailwind-component-enhancement.md`.
### Phase 2 (revised) — full backlog from audit (`docs/reviews/backlog-audit-2026-05-30.md`)

Wave 1 (independent, verifiable now):
- Docs: ARCHITECTURE sherpa version (1.12→1.13), vLLM-sidecar claims (never built),
  stale `gemini-3.1-flash-live-preview` default, dedupe DATA_FLOW/DATAFLOW, remove
  stray `docs/adr/propmt.md`; ADR status promotions (0016 proposed→accepted).
- Dead config: trim `config/default.toml` (sidecar/graph/ui/pipeline/unused models
  not read by `config.rs`).
- UX: W3.1 loading states, W3.4 transcript empty-state, W3.7 stale artifacts.

Wave 2 (keystone, Rust+WSL):
- B01 (P0): fix `asr/sherpa_streaming.rs` vs sherpa-onnx 1.13.2; bump manifest to 1.13.
- ADR-0017: clustering diarization backend (ORT-exclusive w/ parakeet), models,
  offline-window integration, UI selector.

Wave 3 (medium features):
- ADR-0014 notes synthesis (`synthesize_notes` cmd + NotesPanel).
- ADR-0008 native/mistral ontology prompts. i18n completeness (W4.2). Light theme (W4.1).
- Test coverage (22/30 React components, several Rust modules untested).

Wave 4 (large/deferred): ADR-0002 OpenAI Realtime (XL); edition 2024 (drop-order review).

(Updated continuously as the review team feeds findings back.)

### Phases 5–8 — execution waves (all CI-green across Linux/macOS/Windows)

| Wave | Commit | Items |
|---|---|---|
| 1 | docs+ux | doc-drift (ARCHITECTURE/vLLM/DATA_FLOW dedupe/propmt.md), ADR-0016/0008 status, W3.1 loading states, W3.4 transcript empty-state, W3.7 stale artifacts |
| 2 | fix(rust) | B01 sherpa-onnx 1.13 binding rewrite (+manifest bump), B27 gemini default id, B02 dead `[sidecar]` config |
| — | feat(diarization) | ADR-0017 core: `diarization-clustering` feature + `ClusteringDiarizer` (sherpa OfflineSpeakerDiarization, unbounded via num_clusters=-1) + ORT mutual-exclusion `compile_error!` guard |
| 3 | feat(ui) | light theme (ADR-0009/W4.1), +94 component tests, fixed nested-`<button>` a11y bug (review-found) |
| 4 | feat(ui) | i18n completeness (W4.2, en/pt parity 357 keys + language switcher), +76 tests (coverage thresholds met) |
| 5 | fix(ui) | light-theme completeness sweep (~68 literals→tokens, graph theme-aware), CI coverage gate (N3), B27 ExpressSetup residue, B14 ADR-0016 dep note |
| 6 | feat(notes) | ADR-0014 on-demand notes synthesis (`synthesize_notes` command + NotesPanel action) |

Net: ~22 backlog items resolved + CI-verified. Test suite 148 → 318; coverage now
enforced in CI.

### Phase 8 — genuinely-remaining backlog (with justification)

Blocked / environmental:
- **B-RSAC** — local `rsac` path-dep checkout drifted to `#[non_exhaustive]` enums
  ahead of CI's pinned SHA; `capture.rs` matches need wildcard arms *when the pin
  bumps* (adding them now breaks CI's `-D warnings` on the older pinned rsac). Gates
  all LOCAL Rust verification (CI unaffected). Resolve in lockstep with an rsac pin bump.

Large net-new features (each warrants a focused, CI-iterated effort — not deferred
lightly, but not completable+verifiable to quality in one pass):
- **ADR-0002 — OpenAI Realtime provider** (XL, 0 src today).
- **ADR-0017 — diarization live integration** — model downloads (pyannote seg + 3D-Speaker
  embedding), rolling-window offline re-diarization + label stabilization, backend selector
  UI. Engine + feature + guard are DONE; this is the wiring (L/XL).
- **ADR-0013 — pipelined converse front-leg** (STT-final → chat → speak-aloud turn loop) (L).
- **ADR-0008 — unify native/mistral extraction prompts** — nuanced: the LFM2 native path
  needs its model-specific ChatML+schema prompt (just tuned), so a blind swap to the cloud
  `ontology::extraction_system_prompt()` would regress it; needs per-engine care (M).
- **edition 2021→2024** — 22 `tail_expr_drop_order` "changes-meaning" sites need per-site
  drop-order review + all-platform CI (behavioral risk) (M).
- **W5 CSS modularization** + further test coverage — incremental hygiene (L/S).

