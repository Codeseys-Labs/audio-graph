# Wave 7 — P6 + Residual Audit Adjudication (2026-07-04)

Adjudicator pass over two lanes: P6 adversarial review (`/tmp/wave7-audit/p6-critique.md`)
and the 7-open-seed residual audit (`/tmp/wave7-audit/residual-audit.md`). All claims below
were re-verified against the working tree at HEAD (branch `fix/gtk-test-harness-65f0`).
Read-only for me; the orchestrator applies sd/gh mutations.

---

## Lane 1 — P6 adversarial review (#56 / #57 / #58 / #59)

Verdict: **NO REAL FINDINGS.** Every probe resolved to intended, tested behavior. Nothing
to file.

- **#56 redactedErrors cap/collapse** — grouping key `providerId|errorCode` with most-recent
  message + `count`. The only "distinct failure hidden" shape (two same-provider failures,
  both `error_code==null`, different redacted messages) collapses in the *summary panel only*;
  the append-only ledger JSONL retains every event. errorCode is the documented dedup axis and
  real provider failures carry one. Count is exact (insert=1, +1 per match, distinct keys stay
  separate; test asserts 25+1 → 2 rows). No finding.
- **#57 SambaNova provider** — every credential-recovery switch with a `cerebras` arm has the
  matching `sambanova` arm with `default: return null` (cred route, cred-key route, model
  routes, catalog args). Save/load round-trips losslessly via
  `endpointCredentialKey(SAMBANOVA_BASE_URL)=='sambanova_api_key'`. No runtime
  non-exhaustiveness (if/switch chains with explicit defaults). No key leak. No finding.
- **#58 Soniox Planned-with-catalog doc lock** — comment-only across provider-registry/lib.rs +
  commands.rs + registry_tests; no descriptor value changed; invariant test still asserts
  `status==Planned && model_catalog==RemoteCommand`. Nothing to break.
- **#59 ipc::Channel chat-streaming migration** — no terminal-frame latch hang (onmessage armed
  before invoke; `start_streaming_chat` returns id synchronously so invoke always resolves;
  done-before-arm holds then applies on resolve; test covers it). No registry leak (all four
  terminal branches call `registry.finish` then break; delta-send-fail does
  `pipe.cancel(); finish; break`). Cancel path correct (stale request_id dropped by
  finalizeChatStream/appendChatTokenDelta guards). No new privacy surface. No finding.

---

## Lane 2 — Residual seed audit (7 seeds)

### CLOSE (acceptance provably met against code)

**audio-graph-713c** — chat_completion_with_full_usage triple → runtime accounting.
Verified: `OpenRouterRuntimeAccounting` struct folds the full prompt/completion/total triple
(openrouter.rs:826, saturating adds); the blocking chat path calls
`OpenRouterRuntimeAccounting::record_global(&telemetry)` at **openrouter.rs:1457** after every
successful completion; `chat_completion_with_full_usage` (:1357) delegates to it. Test
`accounting_sums_the_full_usage_triple_across_records` (:3259). Seed acceptance = in-memory
accounting wiring, which is done. Per-session disk-persist / Settings surfacing (snapshot_global
has zero consumers outside openrouter.rs) is a NEW capability → filed as a fresh seed, not held
against 713c.

**audio-graph-d042** — reusable ASR transport + parser fixture harness.
Verified: `src-tauri/src/asr/transport.rs` (311 lines) with `AsrWsWriteGuard` (:78) enforcing
content-egress policy at the write primitive; `fixtures.rs` (38KB) + `event_fixtures.rs` +
`reconnect.rs` + `ws_fixture.rs` present. Child **b841 CLOSED** with Soniox(new)+Deepgram(existing)
both on the shared boundary. Seed's own `alreadybuilt_residual` says do NOT re-derive
transport.rs/fixtures.rs; live-runtime consolidation (Gladia/Speechmatics/Soniox) is owned by
**epic ad1d** and its STT children, not d042. Only leftover under d042's labels is cross-platform
Blacksmith CI validation → split to a CI seed if closure needs it.

**audio-graph-bc1c** — map dirty-worktree ownership before broad merges.
Verified obsolete: the 198-row dirty tree the seed coordinated is gone (current
`git status --short` ≈ 12 rows, all untracked docs/plans + .seeds/issues.jsonl + one test); the
six lane worktrees (wt-ci-blacksmith, wt-settings-creds, …) are absent from `git worktree list`;
the ownership-map artifact `docs/reviews/dirty-worktree-ownership-2026-06-27-wave4.md` was
delivered and the Wave 4/5 merges it gated have landed. One-time coordination guardrail whose
window has passed. Close as superseded/obsolete.

### LEAVE OPEN — blocked / needs-research / owned elsewhere (no close, no executable wave slice)

**audio-graph-84f4** — OpenRouter accelerator routing + API surface.
Own acceptance is met (routing-policy schema openrouter.rs:381; catalog commands
list_openrouter_providers_cmd/list_openrouter_model_endpoints_cmd registered lib.rs:512-513;
`OpenRouterAcceleratorDiscovery` rendered at LlmProviderSettings.tsx:791). But the seed is
**Blocked-by 8772 + f3e3** (both testing/CI-secret tasks). No executable code residual on 84f4
itself. Recommendation: close once 8772/f3e3 close, OR narrow 84f4's acceptance to the shipped
routing surface and let 8772/f3e3 own the smoke concern. Orchestrator's call — not auto-closing.

**audio-graph-c237** — ground-truth overlapping-speech fixtures for separation bakeoffs.
Fixtures (4 WAVs + manifest + README) and the offline validator (source_separation_fixtures.rs)
are landed. Remaining acceptance is DATA/MEASUREMENT: manifest marks mono_asr + diarization
baselines `pending_real_run` with `required_before_close:true` (manifest.json:88-141), asserted
at source_separation_fixtures.rs:213-216, and `generated_speaker_lane_selectable_without_baseline`
stays false until measured. Provider-diarization arm is credential-gated. Needs-research: define
+ run the baseline protocol (mono-ASR WER/fragmentation + local diarization offline; provider
diarization credential-gated), then flip pending_real_run→measured. Not a clean code slice.

### PARTIAL — executable residual (this wave)

**audio-graph-9c89** — session artifact export + scheduler queue persistence.
Backend is strong (load_session_impl commands.rs:6049 replays ledger+projections+resets
schedulers; export_session_bundle registered lib.rs:473). Two verified executable gaps, both
secrets/CI-free:
1. Frontend never invokes `export_session_bundle` — only reference in src/ is a comment at
   types/index.ts:2184; store invokes only in-memory export_transcript/export_graph. The
   full-artifact bundle is unreachable from the UI.
2. `projection_scheduler.rs` is in-memory only — `in_flight`/`pending_basis` (:145-146) are never
   persisted; `reset()` (:493) re-creates a fresh scheduler on load. A crash mid-projection drops
   the queued job (ledger survives; queue does not).
→ Executable wave slice below.

**audio-graph-8772** — OpenRouter routed smoke harness.
The live RUN is secrets/live-gated (real OpenRouter key + f3e3 scanner path) → blocked. But the
offline scaffolding is executable NOW: an `#[ignore]`/env-gated test + sanitized metrics-only
report struct that compiles and whose sanitization asserts pass offline with the network step
skipped when the env flag is unset (mirrors projection_eval.rs:724
`provider_projection_smoke_config` but adds routing-policy input + upstream metadata). The live
evidence run stays blocked.
→ Executable wave slice below (scaffolding only).

---

## Orchestrator action summary

- **Close (3):** 713c, d042, bc1c.
- **File as seeds (2):** 713c disk-persist follow-up; d042 Blacksmith cross-platform CI split
  (optional, only if closing d042 needs a home for the CI item).
- **Executable wave (2):** 9c89 export-UI + scheduler-persist; 8772 offline smoke scaffolding.
- **Leave open, no wave (2):** 84f4 (blocked-by 8772/f3e3), c237 (needs baseline measurement).
- **P6 lane:** empty — no findings to file.

### Backlog health
Backlog is healthy and converging. Of 7 open seeds, 3 provably close against merged code
(713c/d042/bc1c), 2 carry genuinely autonomous next-wave slices (9c89 export+persist,
8772 offline scaffolding), and 2 are legitimately held: 84f4 by its 8772/f3e3 blockers and
c237 on real baseline measurements. The P6 review of the four Wave-7 changes is clean. After
this wave lands, the residual is: one epic-owned live-runtime consolidation (ad1d), two
secret/CI-gated live-smoke concerns (8772 run + f3e3 scanner, which also unblock 84f4), and one
needs-data eval task (c237 baselines) — no correctness debt.
