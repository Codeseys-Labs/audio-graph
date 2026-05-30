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
