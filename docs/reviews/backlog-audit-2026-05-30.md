# Backlog Audit — 2026-05-30

**Team:** Review / Audit (read-only)
**Scope:** Full enumeration of outstanding work across `src/`, `src-tauri/src/`,
`docs/`, ADRs, config, tests, deps. Drives the deep-work loop to zero.
**Method:** `rg` marker sweeps, ADR ↔ code cross-checks, doc spot-checks,
dependency/lockfile + CI-audit inspection, real sherpa-onnx 1.13 API verification
(docs.rs). **No source files were modified;** this is the only doc created.

---

## TL;DR posture

The codebase is unusually clean: **zero** `TODO`/`FIXME`/`HACK`/`XXX`,
**zero** `todo!`/`unimplemented!`/`dbg!`, no stub `unreachable!`, no
commented-out code blocks. Every non-test `unwrap`/`expect`/`panic` is guarded
or in `#[cfg(test)]`. `cargo audit` is a hard CI gate with a categorized,
justified ignore file. The outstanding work is therefore **feature/decision
debt**, not rot:

1. One genuinely broken module: `asr/sherpa_streaming.rs` vs sherpa-onnx 1.13 API.
2. Four ADRs whose implementations are pending or partial (0002, 0008, 0017, 0014).
3. Doc/reality drift (notes synthesis, dead config, stale model id, sidecar fiction).
4. UI/UX Waves 3–5 mostly open; no light theme; i18n ~30% of components.
5. Frontend test coverage thin (22/30 components untested); a few Rust modules untested.

---

## Evidence highlights (category 1 — code markers)

- `rg -i "todo|fixme|hack|xxx"` over `src/` + `src-tauri/src/` → **1 hit**, a
  string literal in `speech/mod.rs:201` (`lower.contains("todo")`), not a marker.
- `rg "unimplemented!|todo!|dbg!"` → **0**. `unreachable!` → **0**.
- TS/TSX: no `console.log`, `debugger`, `@ts-ignore`; one justified
  `@ts-expect-error` in `utils/format.test.ts:16`.
- No commented-out code blocks (heuristic scan returned only prose comments).
- `#[allow(dead_code)]` occurrences are all feature-gated or future-introspection
  justified (e.g. `diarization/mod.rs:439,472`, `audio/capture.rs:54`).
- 146 `.unwrap()` + ~215 `expect`/`panic` are **all** in tests or guarded
  (`diarization/mod.rs:336` behind `is_sortformer_active()`, `:620` behind a
  non-empty check). **No code-marker backlog items.**

---

## Prioritized backlog

Priority: **P0** = broken/blocking or correctness; **P1** = high value / committed
ADR not yet built; **P2** = polish / hygiene / deferred-with-evidence.
Complexity: **S** <0.5d · **M** ~1d · **L** ~2–3d · **XL** >3d.

### READY NOW (no blocking dependency)

| ID | Item | Category | Pri | Cx | Deps | Evidence |
|----|------|----------|-----|----|------|----------|
| B01 | Fix `asr/sherpa_streaming.rs` for sherpa-onnx **1.13** API drift (lockfile resolved `"1.12"`→1.13.2). Drift confirmed vs docs.rs: (a) ctor is `OnlineRecognizer::create(&cfg) -> Option<_>` not `::new()`; (b) `OnlineTransducerModelConfig{encoder/decoder/joiner}` are `Option<String>` not `&str`; (c) `OnlineModelConfig.tokens/provider/model_type…` are `Option<String>`, struct has 13 fields incl. `paraformer/zipformer2_ctc/nemo_ctc/t_one_ctc`; (d) `OnlineRecognizerConfig.decoding_method` is `Option<String>`, `enable_endpoint` is `bool` not `i32`, has `feat_config`, `hr`, `ctc_fst_decoder_config`; (e) `get_result` returns `Option<RecognizerResult>` (code does `result.trim()` on a `String`); (f) `accept_waveform`/`is_endpoint`/`is_ready` signatures must be re-checked. Module won't compile under `--features sherpa-streaming`. | known-broken | **P0** | M | none (prereq for B16) | `asr/sherpa_streaming.rs:11-14,82-122,130-150`; `Cargo.toml:47,162`; `Cargo.lock:7365` (1.13.2); docs.rs sherpa_onnx 1.13; ADR-0017 §Context blocker 1 |
| B02 | Prune / wire **dead config** in `config/default.toml`. Only `audio.sample_rate`, `audio.channels`, `asr.model_path` are read (`settings/mod.rs:20,25,30`). Unused: `[diarization] segmentation_model="pyannote-segmentation-3.0.onnx" / embedding_model="wespeaker-…onnx"` (no code loads these — diarization uses parakeet Sortformer), `speaker_similarity_threshold`, `max_speakers`; entire `[sidecar]` block (`port=8081`, `lfm2-…gguf`, `health_check_interval_ms` — **there is no sidecar process**, LLM is in-process llama.cpp); `[pipeline].segment_duration_ms`; `asr.beam_size/temperature`; all of `[graph]` and `[ui]`. Decide: delete, or wire each. | stale config | P1 | S | none | `config/default.toml:17-40`; consumers `settings/mod.rs:20-30`; `config.rs:54-55` parse-only |
| B03 | Implement ADR-0014 `synthesize_notes` backend command. ADR is **accepted** (Option A chosen) but **not built**: `rg synthesize_notes\|summarize` → no command; `NotesPanel.tsx` is still the client-only categorized chip dump (the rejected Option C). Doc/reality drift: NotesPanel header comment claims "needs no backend call" while ADR-0014 mandates one. | ADR pending / drift | P1 | M | reuses `build_graph_chat_context` (`graph/entities.rs:219`), `executor.chat_with_history` | ADR-0014 §Decision/§Implementation; `NotesPanel.tsx:1-9,23-48`; `commands.rs` (no cmd) |
| B04 | ADR-0008 follow-up: adopt the shared `ontology::extraction_system_prompt()` in the **native llama + mistral.rs** extractors. Cloud paths use it (`api_client.rs:224`, `openrouter.rs:399`) but `llm/engine.rs:283` hard-codes its own type list ("Person, Organization, Location, Event, Topic…") and `mistralrs_engine.rs:123` builds its own `system_prompt` — the ADR's stated "Follow-ups" #1 and the prompt-drift it was written to kill. | ADR pending | P1 | M | ADR-0008 substrate exists | ADR-0008 §Follow-ups; `engine.rs:283`; `mistralrs_engine.rs:123`; `ontology.rs:149` |
| B05 | Promote ADR statuses to match reality (immutable-ADR discipline: amend index + add a dated status note). **ADR-0016** is "proposed" but its §Implementation is fully landed (Tailwind v4 in `vite.config.ts`, `@theme inline` in styles.css, 13 modules migrated) → should be **accepted**. **ADR-0008** is "proposed" but cloud impl shipped (`ontology.rs`) → accepted-with-follow-ups. | ADR hygiene | P2 | S | none | `adr/README.md:17,25`; `adr/0016…md:4-5,93-104`; `ontology.rs` |
| B06 | UX **W3.1** — apply the existing `<Button loading>` spinner to the primary controls. ControlBar Start/Stop/Transcribe/Gemini are raw `<button>` with only `disabled` (no in-flight feedback / double-click guard); the primitive already exists. | UI/UX | P1 | M | `Button.tsx` loading prop (done) | `ControlBar.tsx:162-229`; `Button.tsx:6-44` |
| B07 | UX **W3.4** — `LiveTranscript` must distinguish "not started" vs "no speech yet". Still shows bare "Waiting for speech…" implying it's listening when Transcribe was never started. | UI/UX | P2 | S | none | UX deep-dive §2.4 / W3.4; `LiveTranscript.tsx:207` |
| B08 | UX **W3.7** — remove stale artifacts. The Gemini default model `gemini-3.1-flash-live-preview` (flagged "non-existent model id" in the deep-dive) is now hard-coded as the default everywhere; confirm it's a real GA/preview id or replace. | UI/UX / drift | P1 | S | none | UX deep-dive §2.4 ("shipped placeholders … `gemini-3.1-flash-live-preview`"); `settings/mod.rs:432-433`; `gemini/mod.rs:224,621,1440`; `GeminiSettings.tsx:190`; `ARCHITECTURE.md:998,1391` |
| B09 | UX **W4.2** — i18n sweep. Only **9 of 30** components call `useTranslation`; always-on chrome (transcript, pipeline status, chat, toasts, most settings panels) is hardcoded English → `pt` users see mixed UI. Add language switcher in Settings (auto-detect only today). | i18n | P1 | L | none | `rg useTranslation src/components` → 9/30; UX deep-dive §2.3; locales `en.json`(322)/`pt.json`(316) |
| B10 | Frontend test coverage — **22/30 components have no test** (incl. ControlBar, ChatSidebar, LiveTranscript, KnowledgeGraphViewer, NotesPanel, PipelineStatusBar, SpeakerPanel, ConversationModeControl, Notifications, all provider-settings panels). Confirm vitest thresholds (60/50/55/60) actually hold once these render-heavy files count, or raise the floor. | test gap | P1 | L | none | `vitest.config.ts:33-38`; component vs `*.test.tsx` diff (8 tested) |
| B11 | Rust tests for untested non-trivial modules: `llm/executor.rs` (440 ln — priority scheduling / fallback chain), `llm/api_client.rs` (277 ln — OpenAI-compat client), `speak_aloud.rs` (222 ln — TTS pipe + barge-in), `asr/cloud.rs` (190 ln), `asr/mod.rs` (284 ln), `speech/context.rs` (76 ln). `mistralrs_engine.rs` is feature-gated (model-backed, env-gate like the llama path). | test gap | P2 | M | none | `rg #[test]` coverage map; line counts measured |
| B12 | Doc drift sweep on `docs/ARCHITECTURE.md` / `DATA_FLOW.md` / `README.md`: (a) `ARCHITECTURE.md:1099` pins sherpa-onnx "1.12" (lock is 1.13.2); (b) `ARCHITECTURE.md:517` describes a "Python sidecar using vLLM StreamingInput" — never built (ADR-0012 settled it locally); (c) `gemini-3.1-flash-live-preview` echoed in `ARCHITECTURE.md:998,1391,1412` (see B08); (d) `DATA_FLOW.md:317` points at `sherpa_streaming.rs:130` whose API is broken (B01). Also two near-duplicate dataflow docs: `DATA_FLOW.md` and `DATAFLOW.md`. | doc drift | P2 | S | overlaps B01/B08 | cited lines |
| B13 | Resolve `propmt.md` in `docs/adr/` — misspelled, not an ADR, not in the index. Move/rename/delete. | doc hygiene | P2 | S | none | `docs/adr/propmt.md`; absent from `adr/README.md` index |
| B14 | Note runtime-dep posture: ADR-0016/deep-dive say "no UI runtime deps beyond React", but `@radix-ui/react-tooltip ^1.2.8` is now a runtime dep (used by `Tooltip.tsx`). Either document the exception or fold the tooltip into the hand-built primitives to keep the stated invariant. (lucide-react `^1.17.0` verified legit/installed — modernization 2.9 closed.) | dep hygiene | P2 | S | none | `package.json` deps; `Tooltip.tsx`; modernization-audit 2.9 |
| B27 | **SOURCE fix (not doc):** the Gemini default model id `gemini-3.1-flash-live-preview` is hard-coded as the default across **source** and must be verified against the live Gemini model catalog / replaced with a valid current id. Confirmed during the 2026-05-30 doc-drift pass to live in source, not just docs, so the doc-only sweep (B12c) cannot fix it. Exact hits: `src-tauri/src/settings/mod.rs:433` (`default_gemini_model()`), `src-tauri/src/gemini/mod.rs:224` (doc comment), `:1440`, `:1446`, `:1467`, `:1473`, `:1483`, `:1629`, `:1901` (mostly tests/examples), `src/components/settingsTypes.ts:282`, `src/components/GeminiSettings.tsx:190` (placeholder), `src/components/ExpressSetup.tsx:239,244,248`. Docs (`ARCHITECTURE.md`, `GEMINI_LANGUAGES.md`, `SETTINGS_DESIGN.md`) intentionally left matching source until the source default is corrected. | drift / source | P1 | S | none (supersedes the doc half of B08/B12c) | enumerated file:line above |

### BLOCKED-BY (has a prerequisite)

| ID | Item | Category | Pri | Cx | Blocked by | Evidence |
|----|------|----------|-----|----|------------|----------|
| B15 | Implement **ADR-0002 OpenAI Realtime** provider family — STT-only `gpt-realtime-whisper` (ASR provider, Wave A) then `gpt-realtime-2` voice agent (Wave B). Not started: `rg RealtimeProvider\|gpt-realtime-whisper` → 0 in src. Large: new WS client (model on `asr/deepgram.rs`), settings enums, item-id correlation, audio-format conversion, reconnect/cancel/parser tests, proposal routing. | ADR pending | P1 | XL | (independent of B01; gated only on owner sign-off — ADR is "proposed for implementation") | ADR-0002 (whole); no impl in `src-tauri/src` |
| B16 | Implement **ADR-0017 unbounded diarization** (`OfflineSpeakerDiarization` clustering, `num_clusters=-1`, rolling window, mutually exclusive with parakeet). Multi-part: new `diarization-clustering` feature, `models/mod.rs` entries (pyannote seg-3.0 + 3D-Speaker/wespeaker embedding ONNX downloads), `diarization/clustering.rs`, label-stabilization across windows, settings backend selector, env-gated model-backed test asserting `num_speakers>4`. | ADR pending | P1 | XL | **B01** (sherpa module must compile first — ADR-0017 prereq #1) + ORT XOR-parakeet build constraint | ADR-0017 §Implementation 1-7, §Consequences |
| B17 | Implement **ADR-0013 step 2** — pipelined-converse **front leg**: STT-final → `start_streaming_chat` behind push-to-talk / endpointed turn (LLM→TTS leg already works). The mode selector UI + store (step 1) is done (`ConversationModeControl.tsx`, store `conversationMode/converseEngine`); the speech→chat trigger is not (`rg push.to.talk\|front.leg` → none). | ADR pending | P1 | L | streaming chat exists (`commands.rs:1402`); needs turn/endpoint loop (best paired with a streaming ASR, see B01) | ADR-0013 §Rollout step 2; store `index.ts:663-709`; `commands.rs:1402` |
| B18 | Implement **ADR-0013 step 3** / native S2S grounding: Gemini `responseModalities` is hard-coded `["TEXT"]` — no audio-out, no graph-grounded reply; OpenAI Realtime native not present (→B15). Full barge-in turn orchestrator (ADR-0003 turn protocol) + local TTS. | ADR pending | P2 | XL | B15, B17 | ADR-0013 §Rollout step 3; `gemini/mod.rs:621` |
| B19 | UX **W4.1** — ship **light theme**. No `data-theme`, `prefers-color-scheme`, `color-scheme`, or toggle anywhere (`rg` → 0). Needs semantic-token swap + system-pref detection + Settings toggle; ADR-0009 mandates dark+light. | UI/UX / theming | P2 | L | W1 token layer (done) — ready, but large; sits behind higher-pri flow fixes | UX deep-dive §2.1/W4.1; ADR-0009; `rg data-theme src` → 0 |
| B20 | UX **W3.2/W3.3** onboarding hand-off — after Express "Save & Start", guide source→Start; surface pipeline controls pre-capture (disabled+hint); make converse/Gemini discoverable when a key is present. (Partly addressed by ADR-0013 mode selector; the post-Express flow + pre-capture affordance remain.) | UI/UX | P2 | M | benefits from B06 (loading states) | UX deep-dive §2.4/W3.2-3.3; ExpressSetup flow |

### DEFERRED-WITH-EVIDENCE (tracked, intentionally not-now)

| ID | Item | Category | Pri | Cx | Note | Evidence |
|----|------|----------|-----|----|------|----------|
| B21 | Rust **edition 2021→2024**. `cargo fix --edition` auto-edits 2 files but raises **22 `tail_expr_drop_order` "changes meaning in Rust 2024"** warnings (lock-release / channel-send timing in Mutex/channel-heavy code). Needs per-site drop-order review + macOS/Windows CI confirmation. | modernization | P2 | M | deliberately deferred with rationale | modernization-audit 2.5 (lines 35,78-86) |
| B22 | **ADR-0012 Phase 1/2** — streaming-partial overlap + telemetry gating (prefill stable ASR partials, LCP-invalidate, decode at turn-final). Phase 0a done; Phase 0b proven infeasible on the recurrent LFM2 model (only pays off with a non-recurrent GGUF). Phase 1 needs a streaming ASR active (→ couples to B01/B15/B17). | perf | P2 | L | gated on streaming ASR + telemetry | ADR-0012 §Phase 0a/0b/Phase 1; lines 52-66 |
| B23 | Modernization P3 hygiene: **2.11** trim Tailwind default theme (`@theme {--*: initial;}` reclaims ~7KB, risks theme-derived utilities — deferred); **2.7** Windows full `cargo test` harness (`STATUS_ENTRYPOINT_NOT_FOUND`, ADR-0007; subset runs via `scripts/run-core-tests.ps1`); **2.12** bundle-analyze. | hygiene | P2 | S–M | low ROI / tooling | modernization-audit 2.7,2.11,2.12,2.13 |
| B24 | UX **W5.1/W5.2** — split `App.css` per component + dead-rule audit. Lowest urgency; deliberately last so churn doesn't fight earlier waves. (Note: the migrated 13 modules already moved to utilities; this is the residual shared component-layer CSS.) | CSS hygiene | P2 | L | after Waves 3–4 | UX deep-dive §Wave 5 |
| B25 | UX **W4.3** RTL groundwork — logical properties + `dir` wiring. Explicitly deferred until an RTL locale is planned. | i18n | P2 | M | deferred | UX deep-dive W4.3 |
| B26 | Release **signing certs** — procure Apple Developer ID + Windows Authenticode; populate `APPLE_*`/`WINDOWS_*` GitHub secrets. Plumbing is in `release.yml`, waiting on credentials (external/ops, not code). | release/ops | P1 | S | external procurement | gap-analysis Phase 1 #1; `docs/RELEASE.md` |

---

## Security / dependency findings (category 8)

**Posture: good.** No backlog item except the doc/dep-hygiene notes already
listed (B14).

- **Secrets:** `credentials/mod.rs` defines a `Zeroize`/`ZeroizeOnDrop` secret
  struct (`openai/openrouter/groq/together/fireworks/deepgram/assemblyai/gemini
  _api_key`, `aws_secret_key`, `aws_session_token`); keys are kept in
  `credentials.yaml`, **never in `settings.json`** (ADR-0002 acceptance criterion
  already honored by existing code).
- **`unwrap` on external input in hot paths:** none found — audio/gemini/diar
  non-test `unwrap`/`expect` are all guarded (see Evidence highlights).
- **`cargo audit`:** hard CI gate (`ci.yml:103-125`, `cargo audit` in
  `src-tauri/`), with a categorized, justified ignore list at
  `src-tauri/.cargo/audit.toml` (rustls 0.21/webpki 0.101.7 transitive via AWS
  SDK with documented nil exposure + unblock triggers; unmaintained gtk-rs/GTK3
  via Tauri v2; misc unmaintained build helpers). rustls 0.23.40 is used on the
  modern paths; only the AWS chain pins 0.21.12. **No unreviewed advisories.**
- **Frontend deps:** current (React 19.2, Vite 6, Vitest 4.1.4, Tailwind 4.3,
  Biome 2.4, bun pinned `1.3.14`). No `bun audit` step in CI — optional add
  (low pri; npm-side surface is tiny: 11 runtime deps).
- **Rust deps:** modernization audit confirms current (tauri 2.10, tokio 1.50,
  serde 1.0.228, thiserror 2, reqwest, mistralrs 0.8); no broad bump warranted.

---

## Cross-check: ADR ↔ code reality

| ADR | Index status | Code reality | Action |
|-----|--------------|--------------|--------|
| 0002 OpenAI Realtime | proposed | **not implemented** | B15 |
| 0008 Ontology | proposed | shipped for cloud (`ontology.rs`); native/mistral follow-up open | B04, B05 |
| 0012 Prefill | accepted (0a done) | 0a shipped+validated; 0b infeasible (recurrent model); 1/2 open | B22 |
| 0013 Conversation modes | accepted | step 1 (UI/store) done; steps 2 & 3 open | B17, B18 |
| 0014 Notes synthesis | accepted | **not implemented** (NotesPanel still client-only) | B03 |
| 0016 Tailwind v4 | proposed | **fully implemented** (status drift) | B05 |
| 0017 Unbounded diar | proposed | not implemented; blocked on B01 + ORT XOR | B16 |
| 0009/0010/0011 | accepted | implemented (tokens, lucide Icon/IconButton, Notifications) | — |
| 0015 | superseded by 0016 | n/a | — |

Other ADRs (0001/0003/0004/0005/0006/0007) are accepted and reflected in code
(parallel pipeline, TtsProvider/Aura, OpenRouter, streaming chat, feature gates).

---

## Wave-status snapshot (UX deep-dive)

- **Wave 1** (tokens, focus ring, reduced-motion, contrast) — ✅ done.
- **Wave 2** (lucide icons, `<Button>`, notifications, emoji sweep) — ✅ done.
- **Wave 3** — partial: W3.5 (unsaved guard) ✅, W3.6 (graph click-to-inspect,
  `KnowledgeGraphViewer.tsx:170,433`) ✅; **W3.1 partial** (Button has spinner,
  ControlBar doesn't use it → B06); **W3.2/3.3** onboarding → B20; **W3.4** → B07;
  **W3.7** stale artifacts → B08.
- **Wave 4** — open: W4.1 light theme → B19; W4.2 i18n → B09; W4.3 RTL → B25.
- **Wave 5** — open (deferred-by-design): W5.1/W5.2 → B24.

---

## Suggested execution order for the loop

1. **B01** (unblocks B16; one isolated module).
2. **Parallel ready-now batch:** B02, B03, B04, B06, B08 (independent, S–M).
3. **B05, B12, B13, B14** doc/ADR hygiene (cheap, do alongside #2).
4. **B17** (pipelined converse front-leg) — highest user-visible converse win.
5. **B09** i18n sweep, **B10/B11** test backfill (parallelizable, large).
6. **B16, B15** (XL features) once B01/sign-off land; **B18** after B15/B17.
7. **B19** light theme; then deferred **B21–B26** as capacity allows.

## Appended during deep-work loop (2026-05-30)

| ID | item | category | priority | complexity | evidence |
|----|------|----------|----------|------------|----------|
| B-RSAC | Local `rsac` path-dep checkout (HEAD 22ea1a1, dirty) is far ahead of CI's pinned SHA (bed2b99) and marks `AudioSourceKind`/`CaptureTarget` `#[non_exhaustive]`; `src-tauri/src/audio/capture.rs:190,246` match them without a wildcard arm, so local builds against current rsac fail E0004. CI is unaffected (pinned SHA). When the rsac pin is bumped, add wildcard arms (can't add now — they'd be "unreachable" under the older pinned rsac and fail `-D warnings`). | tech-debt / version-skew | P1 (becomes P0 when rsac pin bumps) | S | rsac drift |
| B-ADR17-ENGINE | ADR-0017 diarization: engine + feature + guard DONE; model downloads + live rolling-window integration + UI selector PENDING. | feature | P1 | L | ADR-0017 "Implementation status" |
