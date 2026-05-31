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

## Run 2026-05-30 (later) — local-Rust unblock + verifiable backlog wave

Baseline: HEAD `2e18281`, clean. **Local Rust verification was blocked** in both
prior sessions (B-RSAC). This run's headline: that blocker is gone, so the whole
Rust backlog is now locally verifiable again (`cargo check`/`clippy -D warnings`/
`fmt` green on this Windows host).

### Landed (each CI-gate verified locally; commit)
| Item | Outcome | Commit |
|---|---|---|
| **B-RSAC** | wildcard `#[allow(unreachable_patterns)]` arms + `#[allow(deprecated)]` on `get_default_device()` — version-skew-safe under BOTH the CI-pinned and HEAD rsac. Unblocks ALL local Rust verification. | `e20f3f5` |
| **B02** | pruned dead config (`[graph]/[ui]/[pipeline]`, asr/audio extras, `[diarization]`) to the 3 keys actually read; forward-compat test. | `f39af51` |
| **B04** | native llama + mistral.rs extractors now use shared `ontology::extraction_system_prompt()` (ADR-0008 follow-up #1); LFM2 ChatML wrapper preserved; schema parity (regression low-risk; model-backed eval still advised). | `f39af51` |
| **B14 / N4** | confirmed already-documented Radix exception (ADR-0016); synced ADR-0017 README index status. | `4022411` |
| **B17** | ADR-0013 step 2 converse pipelined **front leg**: `useConverseFrontLeg` aggregates finalized transcripts into endpointed turns → `sendChatMessage` (graph-grounded streaming + speak-aloud). +12 tests. | `4022411`, `172edbf` |
| **B16 (partial)** | ADR-0017 live-diarization **stabilization core** (`diarization/stabilize.rs`: SpeakerRegistry cosine-centroid cross-window matching + greedy cannot-link + WindowSchedule; 11 tests) + verified model download refs. The ONNX-feature-gated worker/UI remain. | `f11e1dd`, `172edbf` |
| **B10 (partial)** | +50 vitest tests across 5 components (ControlBar/Notifications/AgentProposalsPanel/AudioSettings/PopoverOverlay). Suite 318→380. | `b8a38b2` |

Concurrent review (adversarial, read-only) ran each wave; its P1 (converse echo
loop via loopback TTS re-capture) + P2s (stabilizer unbounded-growth, sample_rate
guard) were reconciled into `172edbf`.

PHASE-3 research artifacts: `docs/research/openai-realtime-2026-05.md`,
`docs/research/sherpa-diarization-live-2026-05.md`.

### Genuinely remaining (review-confirmed verdicts)
- **B15 OpenAI Realtime (XL, multi-session):** research is implementation-ready
  (GA wire protocol, models, events captured); new WS client + provider wiring +
  reconnect/parser tests is net-new multi-session work.
- **B16 remainder (XL, hardware-gated):** the `diarization-clustering` worker
  (ring buffer → per-cluster embeddings → WindowSchedule/SpeakerRegistry), model
  downloader, and settings/UI selector — needs the ORT build + real models/audio
  to verify; the pure core + model refs are now in place.
- **B18 native S2S (XL, blocked):** depends on B15 + a barge-in turn orchestrator
  + Gemini audio-out.
- **B20 onboarding (M, closeable):** post-Express hand-off + pre-capture
  affordance — frontend, unblocked; deferred this run for budget.
- **B11 Rust tests (partial):** stabilize.rs added; executor/api_client/speak_aloud
  (async/network) remain.
- **B21 edition-2024 / B22 perf / B25 RTL / B26 signing-certs:** deferred —
  behavioral-risk / streaming-ASR-coupled / no-RTL-locale / external procurement
  (B26 cannot be closed by engineering).
- **B23 hygiene (cheap halves) / B24 CSS split (deliberately last):** unblocked,
  low priority.

**Converse half-duplex** is a new tracked hazard: the pipelined front-leg needs
pipeline-side self-capture/AEC suppression for true full-duplex; the frontend
echo guard (`172edbf`) is a coarse interim mitigation.

## Run 2026-05-30 (evening) — drive-to-zero loop (started at `3e955d2`)

### Phase 1 — Commit state
- Baseline HEAD `3e955d2`, working tree **clean**, 7 commits ahead of `origin/master`
  (unpushed, by design — no push requested).
- Frontend baseline **green**: `tsc --noEmit` ✓, `vitest` **380 passed / 37 files** ✓.
- Rust: B-RSAC unblock (`e20f3f5`) confirmed in place; local `cargo check` re-running
  this session for fresh evidence.
- Three stale worktrees (`agent-a6137…`, `agent-ad81…`, `agent-af5d…`) — all three
  branch heads are **already merged into master**; safe to prune (Phase 1 cleanup).

### Phase 2 — Backlog reconciliation (vs `docs/reviews/backlog-audit-2026-05-30.md`)
Ground-truth re-verification flipped several "remaining" items to **already-done**:
- **B23 bundle-analyze** — DONE (`rollup-plugin-visualizer` in `vite.config.ts`,
  `build:analyze` script, `ANALYZE=1` gate). Only `2.11`/`2.7` halves remain.
- **N3 coverage gate** — DONE: CI runs `bun run test:coverage` (`ci.yml:98`), not bare
  `test`; thresholds (60/50/55/60) are now enforced.
- **B28 light-theme literals** — DONE: 0 hardcoded `rgba(255…)`/`rgba(0…)` in the
  N1-flagged always-on components; tokenized via `--hover-overlay`/`--tint-*`/`--scrim-color`.
- **B26 signing plumbing** — present in `release.yml` (6 `APPLE_*` + `WINDOWS_*` secrets);
  only external cert *procurement* remains (uncloseable in code).
- **B09 i18n** — 15/30 components now use `useTranslation` (was 9/30); sweep continues.

True remaining set (this loop's target): **B15** OpenAI Realtime, **B16-rem**
diarization live-wiring, **B18** native S2S, **B20** onboarding hand-off, **B11**
Rust test backfill, **B09** i18n finish, **B24** CSS split, **B23-rem** (2.7/2.11),
**B21** edition-2024, **B22** perf, **B25** RTL, **B26** cert procurement (doc-only).

### Phases 3–7 — research → architect → execution waves (concurrent adversarial review)

PHASE 3 research (6 parallel agents, Tavily/Exa/DeepWiki/context7):
`docs/research/b{11,15,16,18,20,21}-*.md`. Caught two load-bearing corrections
before any code: the sherpa `SpeakerEmbeddingExtractor` **stream** API (the prior
doc cited a non-existent `compute_speaker_embedding`), and `ringbuf 0.4` is
already a dep (zero new deps for B16). Commit `b7a1823`.

PHASE 4 architect: **ADR-0018** (provider-agnostic converse turn-state FSM +
backend half-duplex/AEC, superseding the interim echo guard) authored + accepted
(`b699349`, `758ffef`). ADR-0002/0017 statuses promoted as work landed.

PHASES 5–7 — waves (each: worktree-isolated execution where Rust-heavy + a
concurrent adversarial reviewer fed only plan+ADR+diff; findings reconciled
into the backlog before commit):

| Wave | Items | Commits | Gate evidence |
|---|---|---|---|
| 1 | B20 onboarding, B09 i18n, B24 CSS, B11 Rust tests | `f1413cf`, `44cef09` | tsc✓ vitest 386✓ biome✓ parity✓; clippy --all-targets✓ |
| — | B26 signing runbook (doc; engineering-complete/procurement-pending) | `e00f482` | — |
| 2 | **B15** OpenAI Realtime STT, **B16** diarization engine+worker+downloads (worktrees) | `3004c6e`, `619af5f`, `ab23354` | cloud + diarization-clustering clippy --all-targets✓ |
| — | deferred-with-cause ledger | `d357afa` | — |
| 3a | **B18** native S2S (Gemini AUDIO + pure turn FSM), **B16-pipe** worker→pipeline wiring, B31 rust+css, B29/B30 i18n (worktrees + main) | `4cda1c2`, `c0eb93b`, `ebc32f9`, `75d8b5a`, `f243619` | cloud + diarization-clustering clippy --all-targets✓; tsc✓ vitest 387✓ parity 427/427✓ |

**Concurrent review caught real issues each wave** (reconciled, not deferred):
Wave 1 — a cross-agent **locale race** (B09 wrote en/pt from a pre-B20 snapshot,
dropping 34 keys → 15 tests red) + a CI-breaking `unnecessary_cast`; both fixed
before commit. Wave 2 — B16 worker correctly flagged **not pipeline-wired** (→
B16-pipe, done in 3a); B15 clean. Wave 3a — `cloud.rs` **E0428 dup-`tests`-module**
from cherry-pick stacking (B11 + B31 both added `mod tests`) fixed at integration
(`f243619`); B16-pipe time-offset precision flagged (→ B16-offset).

**Verification reality:** every Rust change is **compile + clippy `--all-targets
-D warnings` + fmt** verified locally; Rust **test execution** is blocked on this
Windows host by `STATUS_ENTRYPOINT_NOT_FOUND` (0xC0000139, MSVC CRT skew, ADR-0007)
— CI (Linux) is the authoritative test-exec gate. XL features (B15/B16/B18) are
built + CI-typed; their **runtime** (live key / real ONNX models / audio device /
real barge-in) is the documented gate — see `docs/reviews/deferred-ledger-2026-05-30.md`.

### New backlog surfaced by review (tracked, not skipped)
B16-pipe (done), B16-offset, B29 (switcher already existed — verified), B30 (done),
B31 (done), B32 (dep upgrade incl. rsac v0.3.0 + majors — user request, blocked-on
nothing now), B33 (B15 commit-cadence, runtime-gated), B34 (onboarding-key constant).

### Genuinely remaining after Wave 3a
- **B32** dep-upgrade sweep (rsac-hygiene + minors actionable now; majors CI-gated).
- **B21** edition-2024 (all-platform drop-order CI required — scaffold+gate-the-flip).
- **B33/B16-offset/B34** review-nit follow-ups (small).
- **Deferred-with-cause** (see ledger): B22 (Phase-0b-infeasible + streaming-ASR-
  coupled), B23/2.7 (Windows CRT env-fix), B23/2.11 (Tailwind theme trim, low ROI),
  B25 (no RTL locale), B26 (external cert procurement).

### Wave 3b + final reconciliation (after the Wave 3a checkpoint)

- **B32 (dep upgrade)** — safe half landed (`0a3f043`): npm minor/patch bumps
  (within caps; tsc+vitest green) + refreshed the stale rsac-0.2.0 Cargo.toml
  comments to v0.3.0 path-dep reality. GATED REMAINDER documented: the
  `capture.rs` v0.3.0 cleanup is **coupled to bumping CI's `RSAC_REPO_SHA`**
  (still pinned to the older exhaustive-enum SHA `bed2b99`) — doing the cleanup
  without the pin bump breaks the pinned CI, and the pin bump needs the
  all-platform matrix. Rust 0.x-majors (ringbuf/rubato/sysinfo) + framework
  majors (tauri/reqwest/ts6) also deferred (Phase C). Discovered via the worktree
  attempt — exactly the kind of coupling research-before-merge surfaces.
- **B21 (edition-2024)** — scaffolded as `docs/plans/b21-edition-2024-migration-plan.md`
  (22 sites → Pattern A–D fixes + the per-feature/per-OS procedure + the CI gate);
  flip NOT performed (cross-platform drop-order is the whole risk).
- **B33 / B34 / B16-offset** — review-nit follow-ups all LANDED + verified:
  B33 commit-on-utterance-cadence (`9b4219f`), B34 shared onboarding-key constant
  (`4727511`), B16-offset worker-stamped exact window timestamp removing the
  fed-sample reconstruction skew (`f2bcd95`, caught + fixed a feature-gated
  import-scope miss via the two-feature clippy gate).

### Phase 8 — final verification (BOTH sign-offs obtained)

Execution-team gate (fresh on integrated master, all green):
- Rust: `fmt --check` ✓; `clippy --all-targets -D warnings` on **cloud** ✓,
  **local-ml (default)** ✓, **diarization-clustering** ✓.
- Frontend: `tsc --noEmit` ✓; `biome check src/` (88 files) ✓; `vitest` **387
  passed / 38 files** ✓; `vite build` ✓; locale parity **427/427** ✓.
- Rust **test execution** remains CI-gated (Windows `STATUS_ENTRYPOINT_NOT_FOUND`
  CRT skew, ADR-0007); XL-feature **runtime** remains key/model/hardware-gated
  (per the deferred ledger). Honestly represented — not claimed verified.

Independent-review sign-off (fresh agent, read-only, adversarial): **"backlog
genuinely driven to zero — every original (B01–B27) + loop-surfaced (B28–B34)
item is DONE or deferred-with-real-cause; no silent gaps, no material
overclaims."** Spot-checked B15 (GA shape, no beta header, object audio format),
B18 (real FSM + 33 tests), B16-offset (worker-stamped, no reconstruction),
B16-pipe (SPEAKER_DETECTED reachable from two capture paths), B20 (aria-disabled
idiom). Flagged two LOW doc-residue items (ARCHITECTURE.md stale `gemini-3.1`
default) — **fixed** to the source-accurate `gemini-2.0-flash-live-001` in the
final commit. Noted B18's FSM is a pure landing not yet consumed by the live
`start_gemini` path — accurately scoped by ADR-0018 ("architecture before
implementation") + the runtime-gated ledger entry; tracked as the B18 orchestrator-
wiring remainder.

### Loop outcome
24 commits on local `master` (22 + 2 final), tree clean, all CI-faithful gates
green. Original ~14 genuinely-remaining items: **closed or deferred-with-cause to
zero.** Review-surfaced new items (B16-pipe, B16-offset, B29–B34): closed or
gated. The honest residual is entirely **runtime/CI-matrix/external-gated** work,
each with a documented unblock trigger in `docs/reviews/deferred-ledger-2026-05-30.md`.
Per the push policy, the loop ends by opening a PR (commits stay on `master`
locally; no force-push).


