# Dirty-Tree Commit-Sequencing Plan — 2026-06-27

Status: PROPOSAL ONLY. Read-only synthesis of the 7 cluster digests under
`docs/reviews/_dirtytree-2026-06-27/digests/` plus live `git status` and
`.seeds/issues.jsonl` verification. Nothing here has been executed. Every commit
group below is a recommendation gated on the guardrails in
`docs/commit-state-2026-06-27-backlog-zero-wave-5.md`.

HEAD at synthesis: `831cc30`.

---

## 1. Executive summary

### Working-tree size (live `git status --short`, verified this pass)

| Category | Rows | Notes |
|---|---|---|
| Untracked (`??`) | 81 | Includes 17 new Rust files, 2 new crates, 2 generated TS files, ~33 docs, 5 scripts, and several **uncovered** dirs |
| Unstaged-modified (` M`) | 53 | Tracked files with working-tree edits only |
| Staged + unstaged (`MM`) | 49 | Both index and working-tree diffs — must reconcile both before commit |
| Staged-only (`M `) | 23 | Index ahead of working tree |
| Added (`A `) | 2 | `docs/ops/b18-converse-live-smoke.md`, `docs/plans/b18-native-s2s-runtime-driver-plan.md` (already `git add`ed) |
| Added + modified (`AM`) | 1 | `docs/ops/b18-converse-live-smoke.md` family |
| **Total status rows** | **209** | The "203" in the brief was the digest-time count; tree has grown to 209 |

Note: `git status --short` collapses untracked **directories** to a single row.
The expanded untracked file count is ~204 (`research/` alone is 60 files,
`docs/reviews/_audit-2026-06-27/` is 9, `_storage-megaloop` is 7, etc.).

### Complete vs WIP vs risky

- **Complete (commit-ready):** the overwhelming majority. All four backend
  clusters (asr, audio, core, llm) and the frontend cluster report **zero**
  incomplete code changes; every `readiness` field is `complete`,
  `docs-only`, or `generated-or-derived`. The code is finished and tested.
- **WIP (do NOT commit):** the `docs/designs/_storage-megaloop-2026-06-27/`
  design-loop artifacts (decision not ratified) and several **uncovered**
  untracked paths the digests never analyzed (`.hyperresearch/`, `research/`,
  `AGENTS.md`, `CLAUDE.md`, the `_audit` / `_dirtytree` review dirs).
- **Risky (approval-gated, isolate to late commits):** `src-tauri/Cargo.toml`
  (workspace + tauri bump + new deps), `bun.lock` + `package.json` (lock
  coupling), `.github/workflows/ci.yml`, `.github/workflows/release.yml`
  (CI surface, guardrail-flagged), and the two `src/generated/*` files
  (generated, must travel with their generator + source crate).

### The single biggest insight

**The digests over-count "done" seeds.** They union to **74 unique seed IDs**
labeled built. But cross-checking `.seeds/issues.jsonl`, **63 of those 74 are
already `closed`** — committing their code closes nothing. **Only 11 are still
`open`**, and of those 11, **only ~6 are genuinely closeable by committing**;
the other ~5 have code present but acceptance gated on external evidence
(Blacksmith CI runs, native-runtime linkage, or a blocked predecessor seed).

So the headline is the inverse of the naive reading: **this is not "30+ open
seeds are secretly built." It is "~6 open seeds can be closed by committing
finished code; the rest of the dirty tree is the already-committed-in-spirit
implementation of 63 already-closed seeds that simply never got staged."** The
biggest risk is therefore *not* lost work — it is committing a 200-file tree as
an undifferentiated blob and (a) breaking `cargo`/`bun` builds mid-sequence, and
(b) silently committing the guardrail-protected CI/manifest/lock/generated files
without a clean-worktree owner.

---

## 2. ALREADY-BUILT-BUT-UNCOMMITTED seeds

Union of every cluster's `done_seeds`, partitioned by **live seed status**. This
is the operative list — closing happens by *committing*, not building, but only
for seeds whose acceptance is code-based.

### 2a. Closeable by committing (code complete, acceptance is code/test-based)

These are `open` in the seeds file and their implementing files are present and
tested in the dirty tree. After the relevant commit groups land green, run
`sd doctor` then close.

| Seed | Title | Implementing files | Confidence |
|---|---|---|---|
| `audio-graph-ad44` | Event-sourced transcript/notes/graph synthesis data model | `src-tauri/src/projections.rs`, `projection_scheduler.rs`, `projection_eval.rs`, `projection_llm.rs`, `persistence/mod.rs`, `events.rs` | **High** — full data model + replay harness + fixture (`two_span_repair.json`) present and unit-tested |
| `audio-graph-d042` | Reusable ASR provider transport + parser fixture harness | `src-tauri/src/asr/transport.rs`, `reconnect.rs`, `ws_fixture.rs`, `event_fixtures.rs`, `fixtures.rs` + `fixtures/asr/**` | **High** — acceptance is the fixture harness itself (not live sessions), and it is implemented + self-tested |
| `audio-graph-afca` | Dynamic processed-audio consumer registry | `src-tauri/src/audio/consumer.rs`, `audio/capture.rs`, `state.rs` | **Medium** — code + 16 tests complete, BUT seed is `blockedBy: audio-graph-1d59`; confirm 1d59 resolved before closing |
| `audio-graph-bfcb` | Source-channel provenance descriptors + guard fixtures | `src-tauri/crates/ipc-contract/**`, `audio/capture.rs` (provenance fields) | **High** — descriptors + guard fixtures present in ipc-contract crate |
| `audio-graph-a805` | Split provider registry exporter into lightweight codegen path | `scripts/generate-provider-registry.mjs`, `scripts/generate-audio-source-contract.mjs`, `crates/provider-registry/src/bin/export_provider_registry.rs` | **High** — thin delegator scripts + export binaries present |
| `audio-graph-a6d4` | Settings accessibility pass for provider configuration | `src/components/SettingsPage.tsx`, `ProviderReadinessPanel.tsx`, `ModelCatalogPicker.tsx`, `SecretCredentialControl.tsx`, `ConversationModeControl.tsx`, `DemoModeBanner.tsx`, `StorageBanner.tsx`, `styles.css` | **High** — A11Y fixes (WCAG 1.4.11, 4.1.2) implemented with test coverage |
| `audio-graph-70a3` | Session data movement ledger + audit event schema | `src-tauri/src/promotion.rs`, `persistence/mod.rs` (promotion artifacts), `events.rs` (`PRIVACY_POLICY_BLOCKED`) | **Medium** — types + persistence present; verify the "audit event schema" acceptance is types-only (it appears to be) |

### 2b. Code present but NOT closeable yet (acceptance needs external evidence)

`open`/blocked seeds whose code is in the tree but whose acceptance criteria
require CI/hardware/credential evidence the guardrails say we cannot fabricate.
**Commit the code; do NOT close the seed.**

| Seed | Title | Why not closeable | Files |
|---|---|---|---|
| `audio-graph-0117` | Moonshine streaming worker + span adapter | Native C-API runtime linkage + production speech routing still deferred; worker seam only | `src-tauri/src/asr/moonshine.rs`, `speech/mod.rs` |
| `audio-graph-9279` | Moonshine model downloader readiness + cross-platform validation | `blockedBy: 0117, 0d58`; cross-platform claim needs Blacksmith evidence (guardrail) | `src-tauri/src/models/mod.rs` |
| `audio-graph-e35f` | Implement Soniox realtime STT provider | `blockedBy: 0b93, be03` (selectability blocked); client built but not selectable | `src-tauri/src/asr/soniox.rs` |
| `audio-graph-2b2c` | Evaluate SurrealDB file-backed engines on Blacksmith | Acceptance is literally a Blacksmith compile-matrix evidence run we have not done | `src-tauri/src/persistence/surreal.rs`, `surrealdb-revisit-2026-06-27.md` |

### 2c. Already-closed seeds whose code is uncommitted (commit to make repo honest)

63 of the 74 union seeds are already `closed` in `.seeds/issues.jsonl`. Their
code/docs are the bulk of the dirty tree. Committing does not change seed state
but it makes the repository match the recorded backlog. No per-seed action
needed beyond landing the commit groups in §3. (Representative IDs: `b841`,
`7db3`, `3995`, `cf72`, `d598`, `bce5`, `814d`, `2e40`, `5958`, `1b50`, `d157`,
`b294`, `24dc`, `f673`, `abc1`, `903d`, `6381`, `f98e`, `0bc2`, `8c46`, `de28`,
`1322`, `3818`, `9580`, `257a`, `80ed`, ...)

---

## 3. Commit-sequencing plan (ordered)

Ordering principle: foundations (new shared types/files with no upstream
dependents) first, integration surfaces (`commands.rs`, `lib.rs`, `App.tsx`)
last, and guardrail-protected files (Cargo.toml, locks, CI, generated) isolated
into clearly-flagged groups that **require explicit approval + a clean-worktree
owner**. Within Rust, the whole crate must compile after each group; the safest
real-world execution is to stage the listed files, run
`cargo check`/`cargo test` (or `bun run test` for frontend), and only commit on
green — but several mid-sequence groups will NOT compile alone because they share
the same crate as later integration files. Those are flagged "compiles only with
successor." Treat each numbered block as one commit unless noted.

Legend: 🟢 safe-now (low-risk, no guardrail) · 🟡 needs-owner/sequencing ·
🔴 approval-gated (CI/manifest/lock/generated)

### Phase A — Standalone low-risk foundations (🟢 commit immediately)

These have no cross-file dependents in the dirty tree and build/lint on their own.

1. 🟢 **fix(graph): stable edge ids via seq_id + aggregate diarization overlap**
   - `src-tauri/src/graph/temporal.rs`, `graph/entities.rs`, `diarization/mod.rs`
   - Two self-contained bug fixes; no deps. *(backend-core)*
2. 🟢 **fix(models): atomic downloads, HTTP-error rejection, Moonshine component downloader**
   - `src-tauri/src/models/mod.rs` — self-contained download hardening. *(backend-core)*
3. 🟢 **chore(infra): fs_util owner-only refactor, default.toml model_path fix, Info.plist audio permission**
   - `src-tauri/src/fs_util/mod.rs`, `src-tauri/config/default.toml`, `src-tauri/Info.plist`
4. 🟢 **chore(lint): Biome 2.5.1 schema + preset migration**
   - `biome.json` — pure config, no code impact. *(config-scripts-ci)*
5. 🟢 **chore(scripts): WSL test-runner pre-flight + xvfb fallback**
   - `scripts/run-rust-tests-wsl.sh` — standalone shell script.
6. 🟢 **docs: markdownlint/formatting fixes**
   - `docs/ops/windows-rust-test-crt-skew.md`, `docs/plans/b21-edition-2024-migration-plan.md`,
     `docs/research/b11-rust-async-testing.md`, `docs/research/b15-openai-realtime-rust-impl.md`,
     `docs/research/sherpa-diarization-live-2026-05.md`
7. 🟢 **docs(adr): ADR-0017→Accepted, ADR-0018 impl-status note**
   - `docs/adr/0017-unbounded-speaker-diarization.md`, `docs/adr/0018-converse-turn-state-machine-and-half-duplex.md`
8. 🟢 **docs(research): 2026-06-23..06-26 research notes** (11 new untracked notes)
   - all `docs/research/*-2026-06-2{3,5,6}.md` listed in the docs-seeds digest
9. 🟢 **docs(reviews+ops): supply-chain audit, deferred-ledger, vllm-backend, B18 plan+smoke, 06-27 audits**
   - `docs/reviews/supply-chain-audit-2026-06-02.md`, `docs/reviews/deferred-ledger-2026-05-30.md`,
     `docs/ops/vllm-backend.md`, `docs/plans/b18-native-s2s-runtime-driver-plan.md`,
     `docs/ops/b18-converse-live-smoke.md`, `docs/reviews/backlog-audit-2026-06-27-*.md`,
     `docs/reviews/dirty-worktree-ownership-2026-06-27-wave4.md`, `docs/reviews/seed-audit-2026-06-27.md`,
     `docs/reviews/subagent-integration-manifest-2026-06-27.md`, `docs/reviews/surrealdb-revisit-2026-06-27.md`
   - NOTE: `deferred-ledger` and several docs are `MM`/`A`+`M` — stage **both** index and working-tree diffs.

### Phase B — Backend shared seams (🟢/🟡; new files, land before importers)

10. 🟢 **feat(error): secret-redaction helpers + new AppError variants** — `src-tauri/src/error.rs`
    - *Compiles only with successor:* depended on by settings/gemini/commands. Land first.
11. 🟢 **feat(events): new event constants + payload types** — `src-tauri/src/events.rs`
    - depended on by projections, persistence, state, commands, frontend types.
12. 🟢 **feat(asr/mod): ProviderContentEgressPolicy + module wiring + AsrWorker run-loop removal** — `src-tauri/src/asr/mod.rs`
    - foundation type imported by every ASR provider; also FA-6b run-loop removal.
13. 🟢 **feat(audio/pcm): shared PCM conversion helpers** — `src-tauri/src/audio/pcm.rs` (new, pure utility)
14. 🟢 **feat(asr/transport+reconnect): shared WS write guard + reconnect ladder** — `src-tauri/src/asr/transport.rs`, `asr/reconnect.rs` (new)
    - depends on group 12.
15. 🟢 **test(asr/ws_fixture): fake WebSocket server infra** — `src-tauri/src/asr/ws_fixture.rs` (new, `#[cfg(test)]`)

### Phase C — Backend provider + subsystem implementations (🟡 share crate; sequence carefully)

16. 🟡 **feat(asr): deepgram + openai_realtime transport/policy/reconnect refactor**
    - `src-tauri/src/asr/deepgram.rs`, `asr/openai_realtime.rs` — depend on 12/14/15.
17. 🟡 **feat(asr): assemblyai v2→v3 protocol upgrade** — `src-tauri/src/asr/assemblyai.rs`
    - **protocol-breaking**; callers in `speech/mod.rs` + `commands.rs` must handle the new `ServerMessage` variant (those land later in the same sequence). Depends on 14/15.
18. 🟡 **feat(asr): parser-only providers + soniox client**
    - `src-tauri/src/asr/gladia.rs`, `moonshine.rs`, `revai.rs`, `speechmatics.rs`, `soniox.rs` — depend on 12/14/15.
19. 🟡 **feat(asr): aws_transcribe + cloud egress guards** — `src-tauri/src/asr/aws_transcribe.rs`, `asr/cloud.rs` — depend on 12.
20. 🔴 **test(fixtures): ASR provider fake-server JSON fixtures** — `src-tauri/fixtures/asr/**` (14 files)
    - generated-or-derived test data; travels with groups 16-18 + `event_fixtures.rs`/`fixtures.rs`.
21. 🟡 **test(asr): event_fixtures + fixtures harness** — `src-tauri/src/asr/event_fixtures.rs`, `asr/fixtures.rs` — depend on 16-20.
22. 🟢 **feat(credentials): CredentialBackend facade + OS keychain + new provider keys** — `src-tauri/src/credentials/mod.rs` — depends on 11.
23. 🟡 **feat(aws-util): BackendRefreshingCredentialsProvider + draft creds** — `src-tauri/src/aws_util/mod.rs` — depends on 22.
24. 🟢 **feat(sessions): usage tracking, session metadata, validate_session_id** — `src-tauri/src/sessions/mod.rs`, `sessions/usage.rs` (MM — stage both diffs).
25. 🟢 **feat(projections): event-sourced data model + promotion types**
    - `src-tauri/src/projections.rs`, `projection_scheduler.rs`, `projection_llm.rs`, `projection_eval.rs`, `promotion.rs`,
      `fixtures/projection_eval/two_span_repair.json`, `fixtures/promotion/redaction_snapshots.json` — depends on 11. **Closes ad44 candidate.**
26. 🟡 **feat(persistence): repository trait, file-backed impl, event writers, SurrealDB adapter**
    - `src-tauri/src/persistence/mod.rs`, `persistence/surreal.rs` — depends on 25, 11, 24. (surreal.rs feature-gated, not default.)
27. 🟡 **feat(audio): capture descriptor v2 + consumer registry + pipeline/mixer Arc<str> contract**
    - `src-tauri/src/audio/capture.rs`, `audio/consumer.rs`, `audio/pipeline.rs`, `audio/mixer.rs` — depend on 13.
28. 🟢 **feat(playback): producer-side resampling + flush_samples + cancel reset** — `src-tauri/src/playback/mod.rs`, `playback/tests.rs` (tests are ` M`).
29. 🟡 **feat(tts): Deepgram Aura egress policy, flush-sequence fix, redaction** — `src-tauri/src/tts/deepgram_aura.rs`, `tts/mod.rs`.
30. 🟡 **feat(speak_aloud): flush-on-end + non-fatal TTS handling + egress pass-through** — `src-tauri/src/speak_aloud.rs` — depends on 28, 29.
31. 🟡 **feat(converse): FlushPlayback action + state-sensitive reset + ConverseDriver + OpenAI adapter** — `src-tauri/src/converse/mod.rs` — depends on 28.

### Phase D — LLM cluster (🟡; internal ordering matters)

32. 🟡 **feat(llm): provider-neutral streaming contract** — `src-tauri/src/llm/stream_contract.rs` (new), `llm/sse.rs`, `llm/mod.rs`.
33. 🟡 **feat(llm): token-usage surfacing across backends + OpenRouter routing/catalog**
    - `src-tauri/src/llm/api_client.rs`, `llm/engine.rs`, `llm/mistralrs_engine.rs`, `llm/openrouter.rs` — references `asr::ProviderContentEgressPolicy` (group 12) and `credentials::redacted_secret_presence` (22).
34. 🟡 **feat(llm): LocalLlama streaming adapter + streaming refactor** — `src-tauri/src/llm/streaming.rs` — depends on 32, 33.
35. 🟡 **feat(llm): executor projection-patch generation + allow_cloud_fallbacks** — `src-tauri/src/llm/executor.rs` — depends on 33 and the projection crate (group 25).

### Phase E — Settings + provider registry app glue (🟡)

36. 🟢 **feat(settings): YAML config, PrivacyMode, Soniox/Moonshine ASR variants, ConfigCodec harness**
    - `src-tauri/src/settings/mod.rs` + `src-tauri/fixtures/settings/*.yaml|json` (10 fixtures) — depends on 10, 22.
37. 🟡 **feat(gemini): content egress policy, EndTurn cmd, Vertex AI auth fix, redacted debug** — `src-tauri/src/gemini/mod.rs` — depends on 10, 12, 36.
38. 🟡 **feat(provider-registry-app): descriptor mapping commands** — `src-tauri/src/provider_registry.rs` — depends on the registry crate (group 40 🔴) and 36.
39. 🟢 **test(fixtures): source-separation LibriSpeech audio + loader** — `src-tauri/fixtures/source_separation/**`, `src-tauri/src/source_separation_fixtures.rs`.

### Phase F — APPROVAL-GATED: crates, manifest, lock, CI, generated (🔴 clean-worktree owner required)

These are the guardrail-protected files. **Do not stage without a clean-worktree
owner + explicit merge sequencing per wave-5 guardrails.** Each is its own commit.

40. 🔴 **feat(crates): ipc-contract + provider-registry standalone crates**
    - `src-tauri/crates/ipc-contract/**`, `src-tauri/crates/provider-registry/**` — new workspace members; must land with group 41.
41. 🔴 **chore(manifest): Cargo.toml workspace + new deps (keyring, surrealdb, serde-saphyr) + tauri 2.10.3→2.11.0**
    - `src-tauri/Cargo.toml` — **manifest, guardrail-flagged.** Couples with 40 (workspace members) and 36 (serde-saphyr dev-dep). High collision; must be reconciled by clean-worktree owner. *(NOTE: no `Cargo.lock` appears dirty in the slice — confirm the lock is regenerated/committed deterministically by the owner.)*
42. 🔴 **chore(deps): package.json + bun.lock**
    - `package.json` (adds `@os-eco/seeds-cli` devDep + 7 scripts), `bun.lock` (vitest bump + dedup). **Manifest+lock coupling — must commit together.** Guardrail-flagged.
43. 🔴 **feat(scripts): codegen delegates + Seeds tooling** (depends on 42)
    - `scripts/check-docs-secret-hygiene.mjs`, `scripts/ensure-seeds-json-output.mjs`,
      `scripts/generate-audio-source-contract.mjs`, `scripts/generate-provider-registry.mjs`, `scripts/sd-issues.mjs`
44. 🔴 **feat(generated): audioSource.ts + providerRegistry.ts (+ test)**
    - `src/generated/audioSource.ts`, `src/generated/providerRegistry.ts`, `src/generated/providerRegistry.test.ts`
    - **Generated files.** MUST be byte-identical to the output of group 43's generators run against the group 40 crates. The clean-worktree owner must run `bun run check:provider-registry` + `check:audio-source-contract` and confirm no drift before committing. *(biome.json group 4 already excludes `providerRegistry.ts` from rewrite.)*
45. 🔴 **ci: Blacksmith smoke matrix + schedule** — `.github/workflows/ci.yml`, `.github/actionlint.yaml`
    - **CI surface — approval-gated.** ci.yml is unstaged-only; actionlint.yaml registers Blacksmith runner labels (must travel together). Cross-platform claims unverified (no Blacksmith run evidence) — see risk register.
46. 🔴 **ci(release): rsac SHA pin, Blacksmith runners, dry-run, standalone artifacts** — `.github/workflows/release.yml`
    - **CI surface, `MM` — reconcile staged + unstaged.** Approval-gated.

### Phase G — Backend integration surfaces (🟡; land after all of B–F)

47. 🟡 **feat(state): projection runtime, converse-mode fields, ipc-contract AudioSourceInfo** — `src-tauri/src/state.rs`
    - depends on 40 (ipc-contract), 25 (projections), 26 (persistence), 24 (sessions).
48. 🟡 **feat(speech): projection wiring, span-revision events, Soniox/Moonshine dispatch, FA-1/FA-5**
    - `src-tauri/src/speech/mod.rs` (3000+ lines, collision-prone — needs a clean-worktree slot), `speech/context.rs`, `speech/tests_audio_accumulator.rs`, `speech/tests_integration.rs`
    - depends on 27 (audio), 13 (pcm), 31 (converse), 17/18 (asr providers), 25 (projections).
49. 🟡 **feat(commands+lib): converse/projection/readiness commands, privacy enforcement, provider wiring**
    - `src-tauri/src/commands.rs`, `src-tauri/src/lib.rs` — **highest-collision files; depend on nearly everything above.** Must be the final backend commit. Includes the AssemblyAI v3 `ServerMessage` consumer updates that group 17 requires.

### Phase H — Frontend (🟡; ordered by import graph) — most depend on group 44 (generated, 🔴)

50. 🟢 **feat(types): expand type system (span revisions, projection, promotion, provider registry, credential presence)** — `src/types/index.ts`, `src/types/promotionSchema.test.ts`
51. 🟡 **feat(store): projection patch apply, ASR revision, sample preview isolation, backpressure, Gemini routing** — `src/store/index.ts`, `store/index.test.ts`, `store/captureSelection.test.ts`, `store/slices.test.ts` — depends on 50.
52. 🟡 **feat(events): 7 new Tauri event routes + Gemini stale-banner clear** — `src/hooks/useTauriEvents.ts`(+test) — depends on 51.
53. 🟡 **fix(converse): stream watchdog for lost chat-token-done** — `src/hooks/useConverseFrontLeg.ts`(+test) — depends on 51.
54. 🟡 **feat(ui): provider registry helpers, setup-modes view model, credential controls** — `src/components/providerRegistryHelpers.ts`(+test), `providerSetupModes.ts`(+test), `SecretCredentialControl.tsx`(+test), `AdvancedSettingsDisclosure.tsx`, `settingsTypes.ts` — depends on 44 + 50.
55. 🟡 **feat(ui): ProviderReadinessPanel + ModelCatalogPicker** — `src/components/ProviderReadinessPanel.tsx`(+test), `ModelCatalogPicker.tsx`(+test) — depends on 54.
56. 🟡 **feat(ui): ProjectionRuntimeStatusPanel** — `src/components/ProjectionRuntimeStatusPanel.tsx`(+test) — depends on 51.
57. 🟡 **feat(pipeline): audio-consumer health + persistence-queue pressure in PipelineStatusBar** — `src/components/PipelineStatusBar.tsx`(+test) — depends on 52.
58. 🟢 **fix(a11y): focus ring on accent banners + ConversationModeControl aria-label** — `src/styles.css`, `DemoModeBanner.tsx`, `StorageBanner.tsx`, `ConversationModeControl.tsx`(+test). **Closes a6d4 candidate (with 59).**
59. 🟡 **feat(settings): rebuild SettingsPage (mode cards, readiness dashboard, credential controls, model picker)** — `src/components/SettingsPage.tsx`(+test), `src/styles/settings.css` — depends on 54, 55.
60. 🟡 **feat(onboarding): rebuild ExpressSetup** — `src/components/ExpressSetup.tsx` — depends on 54, 55.
61. 🟡 **feat(shell): App.tsx credential-presence gate, sample-preview handoff, panel mount** — `src/App.tsx`(+test) — depends on 60, 56, 51.
62. 🟢 **feat(i18n): new strings (credential controls, readiness, model picker, backpressure, audio consumers)** — `src/i18n/locales/en.json`, `pt.json`.
63. 🟢 **feat(utils): captureTarget helpers + errorToMessage new error codes** — `src/utils/captureTarget.ts`(+test), `errorToMessage.ts`(+test) — depends on 50.
64. 🟢 **test(panels): AgentProposalsPanel + ControlBar test updates** — `src/components/AgentProposalsPanel.test.tsx`, `ControlBar.test.tsx` — depends on 51.

### Phase I — Seeds + docs that reference shipped state (🟡 orchestrator-owned)

65. 🟡 **docs(adr): ADR-0019 (cred/config migration) + ADR-0020 (PCM contract) + README index** — `docs/adr/0019-*.md`, `docs/adr/0020-*.md`, `docs/adr/README.md` (MM).
66. 🟡 **docs(core): ARCHITECTURE, SETTINGS_DESIGN, CONTRIBUTING, deep-work-log, provider-architecture, README** — credential/settings/B18 updates (several MM — stage both diffs).
67. 🔴 **docs: RELEASE.md** — Blacksmith/NSIS/dry-run rewrite (MM); pairs thematically with the CI groups 45/46 — land in the same approval window.
68. 🟡 **docs(commit-state): 06-23..06-27 session snapshots** (10 untracked journals).
69. 🟡 **chore(seeds): `.seeds/issues.jsonl`** — **orchestrator-owned per guardrails.** Land LAST, after all seed-ID-referencing docs and after the seeds whose code shipped are verified. Run `sd doctor` after.

---

## 4. WIP / abandoned / needs-owner — DO NOT COMMIT YET

| Path(s) | Why hold |
|---|---|
| `docs/designs/_storage-megaloop-2026-06-27/` (7 files: frame.md, sweep-0, sweep-2, discover-*) | **WIP design loop, decision not ratified** (digest marks `wip`). Confirm the megaloop is not mid-session before landing; otherwise hold. |
| `docs/reviews/_dirtytree-2026-06-27/` (this analysis's input dir + this plan) | Operational scratch for the current task. Owner decides whether to keep or discard; not a code change. |
| `docs/reviews/_audit-2026-06-27/` (9 files) | Audit scratch referenced by `seed-audit-2026-06-27.md`. **Uncovered by any digest** — owner must classify before committing. |
| `research/` (60 files) | **Uncovered by any digest.** Large untracked dir at repo root (note: distinct from `docs/research/`). Likely hyperresearch scratch — needs-owner triage; do not blind-commit. |
| `.hyperresearch/` (3 files) | **Uncovered.** Tooling state dir — almost certainly belongs in `.gitignore`, not a commit. |
| `AGENTS.md`, `CLAUDE.md` | **Uncovered by any digest.** Untracked agent-instruction files at repo root. Owner must decide if these are intended project files or local-only; do not commit without confirmation. |
| `src-tauri/src/speech/mod.rs` | Not abandoned — complete, but 3000+ lines and collision-prone. Needs a dedicated clean-worktree slot (group 48). Hold until its dependencies have landed. |

---

## 5. Risk register

| # | Risk | Detail / affected files | Mitigation |
|---|---|---|---|
| R1 | **High-collision integration files** | `src-tauri/src/commands.rs` + `lib.rs` (group 49), `src-tauri/src/speech/mod.rs` (group 48), `src-tauri/src/state.rs` (group 47), `src/App.tsx` (61) depend on nearly everything | Sequence them LAST in their cluster; clean-worktree owner; `cargo check` / `bun test` gate before each. |
| R2 | **Manifest + lock couplings** (guardrail) | `Cargo.toml` (41) ↔ new crates (40) ↔ serde-saphyr dev-dep (36); `package.json` ↔ `bun.lock` (42). Cargo.lock not in the dirty slice — must be regenerated deterministically | Commit each manifest with its coupled members in one approval window; owner regenerates + verifies lock. Never commit lock alone or manifest alone. |
| R3 | **Generated-file / generator pairings** (guardrail) | `src/generated/providerRegistry.ts` + `audioSource.ts` (44) are derived from crates (40) via scripts (43). Risk: committing stale generated output that drifts from source | Owner runs `bun run check:provider-registry` + `check:audio-source-contract` and confirms zero drift before committing 44. Land 40→43→44 in order. biome.json (4) already excludes the file from rewrite. |
| R4 | **CI surface, approval-gated** (guardrail) | `.github/workflows/ci.yml` (45, unstaged-only), `release.yml` (46, MM), `.github/actionlint.yaml` (45) | Approval-gated; clean-worktree owner only. ci.yml must be reviewed to confirm it integrates with the staged-at-HEAD state. release.yml needs staged+unstaged reconciliation. |
| R5 | **Cross-platform claims without evidence** (guardrail) | Seeds `9279`, `2b2c`, `fbf6`, `0d58`, `b05b`, `f53b` and the ci.yml matrix claim Linux/macOS/Windows behavior with no Blacksmith run logs | Do NOT close these seeds on commit. Commit the code; keep seeds open with the missing-evidence note. |
| R6 | **AssemblyAI v2→v3 protocol break** | `asr/assemblyai.rs` (17) changes endpoint + event types; consumers in `speech/mod.rs` (48) and `commands.rs` (49) must handle new `ServerMessage` variant | Land 17 only when 48/49 are in the same staged sequence, or the crate won't compile/behave. Do not commit 17 in isolation and stop. |
| R7 | **MM files: dropped working-tree diffs** | 49 `MM` files have BOTH staged and unstaged diffs (e.g. `release.yml`, `deferred-ledger`, `sessions/usage.rs`, `RELEASE.md`, `SETTINGS_DESIGN.md`, ADR `README.md`) | For every MM file, `git add` the working-tree diff (or review `git diff` AND `git diff --cached`) before commit so the unstaged portion isn't silently left behind. |
| R8 | **Seeds file is orchestrator-owned** (guardrail) | `.seeds/issues.jsonl` (69) | Land last; orchestrator only; run `sd doctor` after; never `sd sync` from this checkout. |
| R9 | **Uncovered untracked paths** | `research/` (60 files), `.hyperresearch/`, `AGENTS.md`, `CLAUDE.md`, `_audit-2026-06-27/` — none analyzed by a digest | Triage before any `git add -A`. A bulk add would sweep these in. Prefer explicit per-group staging. |
| R10 | **Mid-sequence non-compiling commits** | Several Phase B/C groups reference symbols defined in later integration groups within the same crate | Where green-on-its-own is impossible, document the dependency in the commit body and verify the *cumulative* sequence compiles; do not claim each isolated commit builds. |

---

## 6. Recommended next actions

### Safe to commit immediately (no guardrail, builds/lints standalone)

Phase A groups **1–9** (graph/diarization fix, models hardening, fs_util/config/Info.plist,
biome, WSL script, and all the docs/research/reviews batches). These are pure
bug-fixes, infra, and documentation with zero dependents and no guardrail
exposure. Recommend landing these first to shrink the tree from 209 rows before
tackling the coupled code. **~9 safe-now commit groups.**

Also safe-now within their phase once §A lands: the standalone-foundation Rust
groups **10, 11, 12, 13** (error, events, asr/mod, pcm) — new/additive symbols,
no guardrail, though they "compile only with successors."

### Need a clean-worktree owner + sequencing (per guardrails)

- **All of Phase F (40–46):** crates, `Cargo.toml`, `package.json`+`bun.lock`,
  CI workflows, generated TS. Approval-gated. Owner regenerates locks + generated
  files and confirms no drift.
- **Integration files (R1):** `commands.rs`/`lib.rs` (49), `speech/mod.rs` (48),
  `state.rs` (47), `App.tsx` (61) — large, collision-prone, depend on everything.
- **`.seeds/issues.jsonl` (69)** and `RELEASE.md` (67) — orchestrator-owned.

### Seeds to close AFTER the relevant code commits land green

Close ONLY these (acceptance is code/test-based), and only after `sd doctor`:

- `audio-graph-ad44` (after group 25 + 26 + 47/48)
- `audio-graph-d042` (after groups 14, 15, 20, 21)
- `audio-graph-bfcb` (after groups 40, 27)
- `audio-graph-a805` (after groups 40, 43)
- `audio-graph-a6d4` (after groups 58, 59)
- `audio-graph-70a3` (after group 25/26 — verify acceptance is types/schema only)
- `audio-graph-afca` (after group 27 — **only if `audio-graph-1d59` is resolved**)

Do **NOT** close (code shipped, evidence missing): `audio-graph-0117`,
`audio-graph-9279`, `audio-graph-e35f`, `audio-graph-2b2c`, plus all CI/cross-
platform seeds (`fbf6`, `0d58`, `b05b`, `f53b`). Keep them open with the
missing-evidence note.

### Triage before any bulk staging

Classify the uncovered untracked paths (R9): `research/`, `.hyperresearch/`,
`AGENTS.md`, `CLAUDE.md`, `_audit-2026-06-27/`, `_storage-megaloop/`. Add the
tooling scratch (`.hyperresearch/`, possibly `research/`) to `.gitignore` rather
than committing. Never run `git add -A` against this tree.
