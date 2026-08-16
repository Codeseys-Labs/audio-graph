# Backlog-Zero Plan — audio-graph

Date: 2026-07-03
Author: FRAME/scale-setter (backlog-zero mission)
Branch base: `fix/gtk-test-harness-65f0` (master ≈ e54422f)
Inputs: 6 audit batches (`/tmp/backlog-audit/audit-batch-0..5.md`) + merged machine classification (79 open seeds).

---

## Executive honesty statement

The backlog is **overwhelmingly non-code**. Of 79 open seeds:

- **13 are already implemented** on the current branch — they close immediately with a citation, no code.
- **1 is stale** (the dirty-tree the seed tracked is gone).
- **10 are guardrail-gated** — CI/release workflow edits, secrets provisioning, shared-history rewrites, or blocked on external factors (OS code-signing, third-party licensing, upstream crate releases). These get SURFACED for approval, not auto-run.
- **~5 need research** before any code can be written (local-TTS binding selection, source-separation bakeoff, multichannel contract, competitive benchmark methodology, upstream-cookie monitoring).
- **~21 are epics / architecture-session containers** — multi-sprint roadmap records. The mission literally CANNOT "close an 8-week epic" with a code wave; these stay open as containers.

That leaves a **genuinely-executable set of ~25 seeds**, and several of those are S-effort registry flips or test tightenings. A small executable set fully cleared + everything else correctly categorized IS mission success. We are not inventing code work for design-session seeds.

---

## 1. closeableNow — close immediately, no code wave

These are `already-done` or `stale-or-invalid`. Close with the cited evidence.

| Seed | Why closeable | Citation |
|------|---------------|----------|
| audio-graph-cbde | Readiness cache + credential_epoch + TTL + in-flight coalescing all shipped | commands.rs:171/255/7972/6411; SettingsPage.test.tsx:30 |
| audio-graph-0d66 | live-audio-smoke CI job (Linux+macOS legs, SHA-pinned); Windows leg held on d3d3 | ci.yml:1032 |
| audio-graph-afca | ProcessedAudioConsumerRegistry with bounded queues + per-consumer health | consumer.rs:410 |
| audio-graph-e864 | Per-consumer health payload + PipelineStatusBar aggregation + i18n tooltip | consumer.rs:322; PipelineStatusBar.tsx:143 |
| audio-graph-2586 | release.yml on Blacksmith all 3 OSes, all refs SHA-pinned, dry_run path | release.yml:51/134-140 |
| audio-graph-a2ff | ProviderSourcePolicy in registry + validation + UI | provider-registry lib.rs:111; commands.rs:657 |
| audio-graph-1e47 | Capability/permission-gated source rows + recovery hints | AudioSourceSelector.tsx:128-416 |
| audio-graph-f0a3 | AssemblyAI v3 endpoint/parser/auth; legacy v2 removed | assemblyai.rs:50/256-301 |
| audio-graph-bfcb | Full AudioChannelProvenanceKind type system + admission guards + fixtures | audioSource.ts:22; capture.rs:198-246 |
| audio-graph-c237 | LibriSpeech CC-BY overlap/turn-taking fixtures + manifest tests | source_separation_fixtures.rs:125/258 |
| audio-graph-84f4 | Full OpenRouter routing policy stack + catalog + UI + tests | openrouter.rs:370-436; commands.rs:8839 |
| audio-graph-88fc | Rich Sentry backend relay + all chokepoints + ErrorBoundary | analytics/mod.rs:402-648; sentry.ts |
| audio-graph-5641 | from_frontend_id already documents intentional collapse-to-Frontend | analytics/mod.rs:551-557 |
| audio-graph-bc1c | (stale) 198-row dirty tree committed green Wave 6; gone from git status | deep-work-log.md:640 |

**Note on partial-completion flips:** `75a1` (onboarding) and `dbac` (diarization settings UX) and `1fbd` (diarization span revisions) are audited as "already-done substantially" with only S-size residuals (a live-sample onboarding step, a model-readiness badge, a `SpeakerLabelSource` enum discriminator). Machine classification tags `dbac`/`1fbd` already-done and `75a1` already-done. **Recommendation:** close the parent seeds citing the shipped core, and — if the residuals matter — file small child seeds rather than block closure. They are OPTIONALLY pulled into Wave 4 below as low-risk polish (see 4b). Do not treat them as blockers.

---

## 2. guardrailGated — surface for user approval, do NOT auto-execute

Each hits a guardrail rule (CI/release workflow edit, secrets provisioning, shared-history rewrite, or external blocker).

| Seed | Guardrail | Blocker detail |
|------|-----------|----------------|
| audio-graph-8eeb | CI workflow edit | ci.yml:73/160/1068 PPA replacement |
| audio-graph-b521 | CI workflow edit + external | Node 24 action SHA bumps; upstream must ship Node-24 releases; deadline 2026-09-16 |
| audio-graph-fd9f | CI workflow edit + external | rsac path→published dep; needs rsac crates.io publish |
| audio-graph-d806 | CI+release workflow edit | rsac path→git-pinned dep; ci.yml + release.yml both checkout rsac by SHA |
| audio-graph-c395 | CI+release workflow edit + Blacksmith | matrix expansion / runner changes |
| audio-graph-c335 | shared-history rewrite + secrets | GitGuardian PR-history findings; dashboard-dismiss or force-with-lease |
| audio-graph-d3d3 | external (third-party licensing) | VB-CABLE commercial license for CI |
| audio-graph-319c | secrets provisioning | inject live SONIOX_API_KEY via OS credential backend |
| audio-graph-0b93 | secrets (blocked by 319c) | run Soniox live smoke; needs provisioned key |
| audio-graph-fdaa | external (OS code-signing) | updater plugin needs signingIdentity (currently null) + notarization |

**Downstream note:** `be03` (promote Soniox to selectable) is CODE-executable (registry flip) but its definition-of-done requires live-smoke evidence from `0b93`, which is guardrail-blocked. It is placed in a gated wave (see Wave 5, conditional) — do not run it until 319c→0b93 clear, or split it to flip-with-recorded-evidence per maintainer call.

---

## 3. epicsAndDesign — roadmap containers, NOT closeable code units

The mission cannot "close" these in a code wave. They remain open as multi-sprint containers / design sessions. Where a container's architecture is substantially shipped, note it can close only once its tracked CHILD seeds close.

| Seed | Kind | Note |
|------|------|------|
| audio-graph-eee3 | Epic (XL) | Local S2S orchestrator — module does not exist; blocks 82b3, 7fcc |
| audio-graph-82b3 | Epic child | Flux eager-turn wiring; needs eee3 orchestrator |
| audio-graph-7fcc | Epic child | Barge-in across providers; needs eee3 turn protocol |
| audio-graph-ad1d | Epic (XL) | Provider-registry codegen + provider expansion roadmap |
| audio-graph-4673 | Epic (XL) | Immutable-JSONL/retcon/eval-CI spanning storage+frontend+eval |
| audio-graph-1c2f | Epic (L) | Credential health center; core shipped, auto-probe/debounce/recovery gaps |
| audio-graph-2044 | Epic (M) | Source descriptor/consumer bus; core shipped, capture_target + telemetry UI residual |
| audio-graph-3588 | Epic (XL) | Diarization architecture; shipped, closes when 5011 + f166 close |
| audio-graph-eebf | Design+feasibility (XL) | Channel-aware ASR projection bridge; needs feasibility study first |
| audio-graph-b153 | Epic (XL) | Competitive roadmap container (Granola/Cluely); stays P2 |
| audio-graph-53cf | Epic (XL) | Calendar pre-briefs; needs calendar-API arch decision |
| audio-graph-392b | Epic (XL) | Live-assist triggers; UI shell exists, backend logic + modes undesigned |
| audio-graph-ceda | Arch session (XL) | Cross-session memory workspace; pure design-first |
| audio-graph-1971 | Epic (XL) | Sharing/redaction/ACL; multi-system arch |
| audio-graph-8235 | Epic (XL) | Action-item lifecycle + integrations; per-integration API research |
| audio-graph-9284 | Epic (XL) | Domain-mode packs / playbooks; zero code exists |
| audio-graph-5b2a | Arch session (XL) | Team workspace governance; acceptance = arch doc |
| audio-graph-8055 | Arch session (XL) | Mobile companion; acceptance = build/buy/defer doc |
| audio-graph-058f | Arch session (XL) | Screen-context assist; governance gate |
| audio-graph-a32f | Design/docs (L) | SOC2/GDPR/DPIA checklist; needs 70a3+c282 to be accurate |
| audio-graph-d633 | Epic (XL) | Main workspace IA redesign; needs design session + shell rework |
| audio-graph-75a1 | (see §1) | Onboarding — close core; residual is S polish |

**needs-research (not code-actionable now, route to `/hyperresearch`):** `1a8c` (local-TTS binding selection), `8181` (competitive benchmark methodology), `dd19` (source-separation bakeoff), `b5f3` (multichannel contract — depends on dd19), `2ef1` (monitor upstream cookie release; one-line change once unblocked).

---

## 4. waves[] — dependency-ordered execution of the EXECUTABLE set

Rules applied: (a) disjoint `touchesFiles` run parallel in the same wave; (b) shared-file or dependsOn edges push to later waves; (c) ≤6 items/wave (worktree fan-out bound); (d) foundational/blocking items first.

Key shared-file hazards driving sequencing:
- **`provider-registry/src/lib.rs` + `src/generated/providerRegistry.ts`**: touched by `e35f`, `eb2e`, `226e`, `14e0`, `be03`, `ff45` → these CANNOT run in one wave; spread across waves.
- **`llm/openrouter.rs`**: touched by `76bd`, `713c`, `8772` → `76bd` first (defines telemetry struct), then `713c`/`8772`.
- **`asr/cloud.rs`**: touched by `eb2e`, `226e` → different waves.
- **`persistence/mod.rs` + `commands.rs`**: touched by `70a3`, `9c89`, `c282`, `0c55` → group by disjointness carefully; `51e0` depends on `70a3`.

### Wave 1 — foundational / blocking (6 items)

Independent, high-leverage. `02da` is the blocking XL harness; `76bd` unblocks the OpenRouter telemetry chain; `70a3` unblocks the privacy-report chain. The rest are disjoint standalone units.

| id | title | effort | files |
|----|-------|--------|-------|
| audio-graph-02da | Reusable streaming WebSocket ASR session harness (WsAsrSession trait) | XL | asr/session.rs (new), asr/mod.rs, asr/deepgram.rs, asr/assemblyai.rs |
| audio-graph-76bd | OpenRouter routed-provider telemetry struct | M | llm/openrouter.rs, llm/streaming.rs, llm/stream_contract.rs |
| audio-graph-70a3 | Session data-movement ledger + audit event schema | L | persistence/mod.rs, commands.rs, asr/cloud.rs, llm/openrouter.rs, audio/capture.rs |
| audio-graph-8efa | Tighten flaky clear_drops_in_flight_audio_frames test | S | tts/deepgram_aura.rs |
| audio-graph-403d | SID-native Windows ACL hardening | S | fs_util/mod.rs |
| audio-graph-932b | Vite chunk-split analysis + additional splits | M | vite.config.ts, App.tsx |

Conflict watch: `70a3` and `76bd` both touch `llm/openrouter.rs`; `70a3` adds a ledger call-site (capture/ASR/LLM hooks), `76bd` adds a telemetry struct in the same file. Assign both to the SAME worktree operator OR serialize `76bd`→`70a3` inside the wave if collisions appear. Everything else is disjoint.

### Wave 2 — registry-flip batch A + telemetry consumers + independent providers (6 items)

`14e0` and `e35f` both touch registry+generated-TS but are the two safest (Moonshine is feature-gated local; Soniox flip). Put ONE registry-touching provider here (`e35f`) and keep `14e0` in Wave 3 to avoid registry-file collision — actually `14e0` primarily touches `asr/moonshine.rs` + `Cargo.toml` + registry descriptor; `e35f` touches registry status + generated TS. To be safe they go in separate waves.

| id | title | effort | files |
|----|-------|--------|-------|
| audio-graph-e35f | Promote Soniox realtime STT to selectable (registry flip) | S | provider-registry lib.rs, providerRegistry.ts |
| audio-graph-713c | Wire full_usage triple into OpenRouterRoutingTelemetry | S | llm/openrouter.rs |
| audio-graph-51e0 | Session data-route UI + privacy report | M | src/components, commands.rs, src/types/index.ts |
| audio-graph-ff45 | SambaNova credential slot + allowlist + routing | M | credentials/mod.rs, llm/, provider_registry.rs, providerRegistry.ts, types/index.ts |
| audio-graph-9c89 | Session artifact migration (schema_version + dispatch) | M | sessions/mod.rs, projections.rs, persistence/mod.rs, commands.rs |
| audio-graph-5011 | Local diarization worker rolling-window stability tests | M | diarization/worker.rs, diarization/stabilize.rs, commands.rs |

Deps satisfied: `713c`→`76bd` (W1 done), `51e0`→`70a3` (W1 done).
Conflict watch: `e35f` and `ff45` both touch `providerRegistry.ts` + a registry lib — SERIALIZE these two (same operator or e35f before ff45). `9c89` and `51e0` both touch `commands.rs`/`persistence`; keep to non-overlapping functions or serialize. If the worktree fan-out shows collisions, split this wave into 2a (e35f, 713c, 51e0) and 2b (ff45, 9c89, 5011).

### Wave 3 — Moonshine + cloud-provider runtimes + doc/test units (6 items)

`14e0` (Moonshine) now runs alone re: registry. `eb2e` and `226e` both touch `asr/cloud.rs` + `provider_registry.rs` + `providerRegistry.ts` → they MUST be serialized (different waves ideally). Put `eb2e` here, `226e` in Wave 4.

| id | title | effort | files |
|----|-------|--------|-------|
| audio-graph-14e0 | Moonshine streaming STT provider (ONNX binding under feature) | L | asr/moonshine.rs, Cargo.toml, provider-registry lib.rs, scripts/download-models.ps1 |
| audio-graph-eb2e | Speechmatics live realtime runtime + readiness | L | asr/speechmatics.rs, asr/cloud.rs, provider_registry.rs, providerRegistry.ts, SttPanel.tsx |
| audio-graph-1534 | Migrate streaming hot paths to ipc::Channel<T> | L | commands.rs, events.rs, speech/mod.rs, ipc-contract/, useTauriEvents.ts |
| audio-graph-c282 | Retention policy matrix for artifacts + diagnostics | M | promotion.rs, persistence/mod.rs, sessions/mod.rs, docs |
| audio-graph-09a7 | Cross-platform release smoke runbooks (3 docs) | M | docs/ops/smoke-runbook-{windows,macos,linux}.md |
| audio-graph-f3e3 | Wire secret-hygiene scanner into routed-smoke evidence path | S | scripts/check-docs-secret-hygiene.mjs, package.json, docs/ops/ |

Conflict watch: `14e0` and `eb2e` both touch a provider-registry file (`14e0`→crate `lib.rs` descriptor; `eb2e`→`provider_registry.rs` shim + `providerRegistry.ts`). Verify these are distinct files; if `eb2e` regenerates `providerRegistry.ts` and `14e0` also edits it, serialize. `1534` and `c282`/`eb2e` all touch `commands.rs`/`speech/mod.rs` — keep functions disjoint or serialize `1534` last.

### Wave 4 — remaining runtimes + smoke harness + polish (5 items)

`226e` (Gladia) now runs after `eb2e`. `8772` (routed smoke) after `76bd`+`f3e3`. `0c55` is standalone. `f166` after `0d66` (done).

| id | title | effort | files |
|----|-------|--------|-------|
| audio-graph-226e | Gladia Solaria live runtime + registry readiness | L | asr/gladia.rs, asr/cloud.rs, provider_registry.rs, providerRegistry.ts |
| audio-graph-8772 | OpenRouter routed smoke harness (env-gated) | M | projection_eval.rs, llm/openrouter.rs |
| audio-graph-0c55 | Resumable-vs-readonly loaded live-assist pending cards decision+impl | M | commands.rs, state.rs, AgentProposalsPanel.tsx |
| audio-graph-f166 | Capture source round-trip tests (Win/mac/Linux) | M | audio/live_audio_smoke.rs, ci.yml (test extension only) |
| audio-graph-2d77 | @sentry/browser webview capture gated by analytics toggle (CSP decision first) | M | analytics/sentry.ts, tauri.conf.json, adr/0023, package.json |

Conflict watch: `226e` touches `asr/cloud.rs`/`providerRegistry.ts` (now free after eb2e in W3). `8772` and W1's `70a3`/`76bd` touched `openrouter.rs` but those waves are done. `f166`'s ci.yml touch is a test-job extension, not a workflow-guardrail structural change — but if it materially alters the workflow, treat it as guardrail-gated and surface instead.

#### 4b — OPTIONAL low-risk polish (only if maintainer wants residuals closed, not blockers)
- `dbac` residual: model-readiness badge in diarization section (S) — SttPanel.tsx
- `1fbd` residual: `SpeakerLabelSource` enum discriminator on revision payload (S) — events.rs, projections.rs
- `75a1` residual: live "short sample" step in ExpressSetup (S) — ExpressSetup.tsx

These share `SttPanel.tsx`/`events.rs` with earlier waves; run last, serialized. Prefer filing as child seeds and closing parents on shipped core.

### Wave 5 — CONDITIONAL / gated (do not auto-run)

| id | title | effort | gate |
|----|-------|--------|------|
| audio-graph-be03 | Promote Soniox to selectable ASR provider | M | Code-ready (registry flip) but DoD needs live-smoke from 0b93 (guardrail 319c→0b93). Surface for maintainer: flip-with-recorded-evidence, or hold. |

---

## Wave ordering rationale (summary)

1. **Wave 1** lands the two chain-roots (`76bd` telemetry struct, `70a3` ledger) plus the blocking XL harness (`02da`) and three fully-disjoint standalone units (`8efa`, `403d`, `932b`). Nothing here depends on anything else executable.
2. **Wave 2** consumes W1 outputs (`713c`←`76bd`, `51e0`←`70a3`), does the safest registry flip (`e35f`), and runs disjoint units (`ff45`, `9c89`, `5011`). Registry-TS collisions between `e35f`/`ff45` are the one thing to serialize.
3. **Wave 3** takes the first cloud runtime (`eb2e`), Moonshine (`14e0`), and file-disjoint doc/infra work. Cloud-runtime pairs (`eb2e`/`226e`) are split across W3/W4 because they share `asr/cloud.rs` + generated TS.
4. **Wave 4** finishes the second cloud runtime (`226e`), the routed-smoke harness (`8772`←`76bd`+`f3e3`), and independent UI/test units.
5. **Wave 5** is the single gated code unit (`be03`) held on secrets provisioning.

Total executable seeds routed: 24 across Waves 1-4 (plus `be03` gated). Guardrail (10) surfaced. Epics/design (21) + needs-research (5) left as containers. Already-done/stale (14) close on citation.
