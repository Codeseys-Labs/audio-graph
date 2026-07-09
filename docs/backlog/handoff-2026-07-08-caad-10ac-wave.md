# Handoff — 2026-07-08 checkpoint: caad+10ac wave mid-flight

Session checkpoint written at user request. Two-lane fix wave (caad P1 + 10ac P2) was
stopped cleanly mid-flight; everything needed to resume is below. **No work was lost:**
all completed stages are journal-cached, caad's implementation is pushed to origin, and
10ac's worktree is clean (its implement stage never reached the edit phase).

---

## 1. Mission context (the standing frame)

- **Backlog-zero mission**: drive `sd` seeds to zero via tiered workflows. Backlog lives
  in git-tracked `.seeds/issues.jsonl` (never `git reset --hard` without committing it).
- **Manual-test loop**: user tests Windows dry-run builds from
  `/mnt/c/Users/bbala_n314ugx/Downloads/`; logs at
  `C:\Users\bbala_n314ugx\AppData\Roaming\audio-graph\logs`; Sentry org `codeseys-labs`
  (region https://us.sentry.io). Round-3 build was `audio-graph-dryrun-d5eaa91.exe`,
  session `a26e85c0` (2026-07-07 23:06–23:10). **Round 4 is owed to the user** once this
  wave folds (build + move to Downloads).
- **MVP scoping (shipped)**: only Deepgram ASR + all llm.* + tts.none/deepgram_aura are
  UI-selectable; everything else deferred behind the `ui_selectable` axis (PR #97).
- **Model routing**: route by *what gate catches a wrong answer*, not by importance
  (memory: `route-by-gates-not-importance`). User ratified a refinement this session —
  see §6. Multi-lane work runs as Workflow tool invocations with explicit per-stage
  `model:` tiers (user directive "use workflows").

## 2. Where we are — completed this session

| Item | State | Evidence |
|---|---|---|
| PR #97 (MVP provider scoping, ad56+e153) | **MERGED** squash `ad1e863`; seeds closed; master at `8989667` | Adoption metric: 2 outer fix rounds (Express P1 review catch; aws-smithy CI incident) |
| aws-smithy-http CI incident | **RESOLVED** — root fix in `347ccfc` | See §5.1; do NOT re-add a direct aws-smithy-http dep |
| caad design (Opus) | **DONE**, journal-cached | `docs/plans/2026-07-08-caad-apply-policy-design.md` — chose **option (c)** |
| caad implement (Sonnet) | **DONE + PUSHED**: 4 commits on `origin/fix/notes-apply-policy-caad`, head `200a87a` | All gates green: fmt, clippy -D warnings, 40/40 projections, 11/11 scheduler, 54/54 speech tests |
| caad review (Opus) | **IN FLIGHT when stopped** (attempt 2, ~116 transcript lines) | Must re-run on resume — no verdict was returned |
| 10ac design/investigation (Opus) | **DONE**, journal-cached | `docs/plans/2026-07-08-10ac-graph-decode-findings.md` |
| 10ac implement (Sonnet) | **NOT STARTED (edits)** — 7 attempts died pre-edit; worktree clean | See §5.2 for why and what to do differently |
| 10ac review / both fix+PR stages | Not reached | — |

**Open PRs from this wave: none.** (Only unrelated old PR #21 is open.)

## 3. The two lanes — technical substance

### 3.1 caad (P1) — notes patches discarded as stale during continuous speech

**Problem** (round-3 finding): generation is fixed (0 notes failures, OpenRouterJsonSchema
active) but 22/23 completed notes patches were `DiscardedStaleAndStartedRepair`
(staleness=MissingCurrentSpan); the During view shows nothing until the speaker pauses.
New transcript spans land faster (~2s) than generation completes (~10–20s), so every
patch is stale-by-arrival. **The apply policy is the bottleneck, not generation.**

**Design decision (option c — append-vs-revise discrimination)**: a patch whose basis is
stale only because the transcript *grew* (append-only) is still correct for what it
covers; discard only when covered spans were *revised*. Doc:
`docs/plans/2026-07-08-caad-apply-policy-design.md`.

**Implementation (pushed, head `200a87a`)**:
- `projections.rs`: new pure classifier `TranscriptLedger::classify_basis_currency` →
  `Current | AppendOnlyStale | Revised(ProjectionBasisStaleness)` (replaces binary
  validate_basis at the two gates; `validate_basis` itself unchanged as a thin wrapper so
  promotion.rs/replay are untouched).
- Gate 1 `apply_validated_patch_with_speaker_timeline_opt`: applies on
  `Current|AppendOnlyStale`, rejects only `Revised`.
- Gate 2 `ProjectionScheduler::complete_in_flight`/`fail_in_flight`
  (projection_scheduler.rs): AppendOnlyStale routes through the existing
  `CompletedAndStartedFollowUp` path (new `FailedAndStartedFollowUp` variant for the
  failure side) — no stale_discards bump, no Replay-priority repair.
- `speech/mod.rs`: `dispatch_projection_decision` handles the new variant.
- Tests: classifier units, apply-gate units, scheduler append-vs-revise contrast pairs,
  and an integration regression proving the During view populates before a pause.
- ADR-0025 addendum + ADR-0024 §2 pointer committed (`200a87a`).
- Cross-lane touches: **none** (stayed off executor/openrouter/projection_llm).

**What remains**: Opus integration-surface review (callers of every changed function;
symmetric writers — the complete/fail pair was the historic bug class; double-apply /
out-of-order / repair-starvation traces; ADR-0023; hygiene), then fix+PR. **The PR body
must contain the design-decision paragraph from the design doc — merge-blocking if missing.**

### 3.2 10ac (P2) — graph projection ~50% decode failures + missing-'type' escapes

**Root causes** (Opus investigation, high confidence, all code-verified):
1. **max_tokens starvation**: projection requests fall back to `max_tokens=512`
   (`commands.rs:901-905`; llm_api_config defaults None). gpt-oss-120b is a reasoning
   model — reasoning tokens eat the budget before the large graph patch completes, so
   tails truncate. The "missing field type" failures at deep columns (172–417) are
   truncated tails, **not** a schema bug (schema is correct: type required per-variant,
   `projection_llm.rs:305-322`).
2. **Envelope shape mismatch misclassified as retryable**: `ChoiceMessage.content` is
   required `String` (`openrouter.rs:761`) but reasoning models can emit envelopes that
   don't match; the deterministic mismatch is misclassified as a transient decode error
   (retries finish in ~2.5s, nowhere near timeout — it's not network truncation).
   Classification point: `is_retryable_chat_decode_error` (`openrouter.rs:~1725`).
3. **Observability gap**: `chat_completion_with_schema_cached` (`openrouter.rs:1607`)
   discards routing telemetry, so the routed upstream provider is unlogged — can't
   distinguish provider-ignored-schema from truncation in the field.

**Fix set (designed, unimplemented)**: kind-aware max_tokens (graph ≥ 2048); make
`ChoiceMessage.content` `Option` + tolerate reasoning envelopes; decode via `text()` to
log body-len + routed provider (metadata only — ADR-0023: never log response bodies);
stop misclassifying shape-mismatch as retryable. Unit-testable with the existing
request-capture harness; no live API calls.

**Review must check** (baked into the relaunch script): making content Optional changes
the deserialization contract for **ALL** chat-completion consumers (chat, extraction),
not just projections — enumerate every caller; verify the retry-classification change
doesn't swallow genuinely transient network errors.

## 4. How to resume (exact commands)

Worktrees still standing, both based on master `8989667`:
- `.claude/worktrees/wave-caad` → `fix/notes-apply-policy-caad` (= origin, `200a87a`, clean)
- `.claude/worktrees/wave-10ac` → `fix/graph-decode-10ac` (at base, clean, unpushed)

Workflow scripts + journals (session
`/home/codeseys/.claude/projects/-mnt-e-CS-github-audio-graph/e8c99489-58e9-4eff-8601-536ac38c6e4e/`):
- Original two-lane run: `workflows/scripts/caad-10ac-wave-wf_fd8ea3c1-b9a.js`, run id
  `wf_fd8ea3c1-b9a` (journal: `subagents/workflows/wf_fd8ea3c1-b9a/journal.jsonl`)
- 10ac relaunch (hardened prompts): `workflows/scripts/wave-10ac-lane-wf_5eede861-a2c.js`,
  run id `wf_5eede861-a2c`

**Note**: resume caching keys on (prompt, opts) — the original script's design stages
would return cached results, but both lanes' remaining stages are cheaper to relaunch
fresh given §5.2. Recommended resume in a NEW session:

1. **caad**: run review→fix+PR only. Either resume `wf_fd8ea3c1-b9a` (design+implement
   cached; review re-runs) or extract the review/fixpr prompts from the original script
   into a small single-lane workflow (cleaner). Review stage = Opus; fix+PR = Sonnet.
2. **10ac**: relaunch `wave-10ac-lane-wf_5eede861-a2c.js` as-is (its implement prompt
   already carries the timeout-wrapped-grep hardening and the design is inlined), **but
   only after reading §5.2** — consider main-loop implementation instead.
3. Re-create the wave-fold cron (the previous one, 6158d76f, is deleted; text below).

### Wave-fold cron text (was 23,53 * * * *, session-only)

> When a lane's PR exists: watch CI from main loop (backgrounded polling, ≤90s sleeps).
> MANDATORY merge gate per PR: FRESH full sweep (reviews + issue comments + inline +
> reactions, fetched AFTER CI green, immediately before merge); verify Majors against
> the diff (check review anchor commit vs fix-push time — a stale review may re-flag
> fixed code, as happened on #97); real CodeRabbit review ≠ rate-limit comment; record
> adoption metric (outer fix rounds) in a PR comment; nonblocking findings → sd seeds.
> caad PR body must contain its design-decision paragraph (blocking gap otherwise).
> Merge order: caad reported zero cross-lane touches — if 10ac's PR is also disjoint,
> merge either order; if overlap, caad (P1) first, rebase 10ac.
> After each merge: close the seed with evidence; commit .seeds on master
> (`timeout 150 git fetch origin master` then `timeout 150 git rebase origin/master`,
> stepped — plain pull times out); delete remote branch; prune the lane worktree.
> When both folded: build Windows dry-run exe → `/mnt/c/Users/bbala_n314ugx/Downloads/`
> (round-4 manual test), notify user + remind about seed 8913 decision; delete the cron.

## 5. Operational lessons from this session (read before resuming)

### 5.1 aws-smithy-http split-graph (CI incident, resolved — don't regress it)

`Cargo.lock` is gitignored (`.gitignore:11`) → CI re-resolves deps every run. The AWS
SDK crates enable smithy-http's `event-stream` feature; a fresh resolve can land the SDK
on a different semver-incompatible 0.x than any direct dep, splitting the graph into two
crate instances — the feature attaches to the SDK's instance, so a direct
`aws_smithy_http::` import fails E0433 **regardless of pinning** (an exact `=0.63.6` pin
failed identically). Durable fix shipped in `347ccfc`: import via the SDK re-export
`aws_sdk_transcribestreaming::primitives::event_stream::EventStreamSender` and no direct
dep — version-split-proof by construction. `Cargo.toml` carries a warning comment.
**Never re-add a direct aws-smithy-http dependency.** Systemic fix is seed 8913 (§7).

### 5.2 Subagent watchdog deaths on /mnt/e (the tax that stalled 10ac)

Workflow subagents died mid-flight with "[Request interrupted by user]" **10 times** this
session (caad design ×3, 10ac implement ×7 across both runs, caad review ×1). Pattern:
deaths cluster during long silent greps/reads on `/mnt/e` while another lane compiles —
D-state I/O waits trip the silence watchdog. caad's implementer survived 90+ minutes by
polling with explicit `sleep 150; tail` loops (audible, not silent).

Mitigations that helped: wrapping every grep/find in `timeout 120` (10ac attempt
lifespan went from ~17 to ~41 min); flock-serializing all cargo invocations
(`/tmp/audio-graph-cargo.lock`); backgrounding compiles via nohup + short polls.
Not sufficient alone: 10ac still died pre-edit every time.

**Recommendation for 10ac resume**: either (a) run the 10ac lane while nothing else
compiles (it was always the victim of caad's cargo activity — solo it should survive), or
(b) implement 10ac from the main loop directly (no watchdog) using the findings doc,
then spawn only the Opus review as a subagent. (b) is the certain path; (a) preserves
the delegation pattern. The harness retry loop does converge, but each retry re-reads
everything — expensive re-orientation.

### 5.3 Merge-gate sweeps catch stale bot reviews

On #97 the post-CI-green sweep surfaced a Codex P1 that **re-flagged already-fixed
code** — the review anchored to the original commit and posted after the fix was pushed.
Always compare a review's anchor commit against the fix-push timeline before treating a
Major as live (cron text encodes this).

## 6. Model-routing policy (proposed this session — awaiting explicit user sign-off)

The user asked whether design=Fable / implement=Opus / everything-else=Sonnet was right;
I pushed back with the table below. The user did not reply before the checkpoint, so
confirm it (or take their edits) before folding it into the standing memory. Proposed:

| Work | Tier |
|---|---|
| Direction-setting decision with NO downstream gate | **Fable** — via main-loop ratification between workflow phases (not a Fable subagent) |
| Trust-boundary review (security, credentials, ADR-0023 privacy) | **Fable** |
| Investigation / root-cause; design-option analysis | Opus |
| Review — the gate — always | Opus |
| Weakly-gated implement (concurrency, staleness policy, ordering) | Opus |
| Gated implement (compiler + tests + Opus review behind it) | Sonnet |
| Babysitting, CI-watch, log grep, mechanical fix+PR | Sonnet |

Key principles: pay for the gate, not the doer; review never drops below Opus ("Sonnet
for everything else" would defund the one stage that catches shipped bugs); split future
Design phases into their own workflow so a Fable (main-loop) ratification point exists
between design and implement. Memory `route-by-gates-not-importance` holds the fuller
rationale — worth folding this table into it.

## 7. Open decisions & queue after this wave

- **Seed 8913 (P1, approval-gated, AWAITING USER)**: commit `Cargo.lock` + add
  `--locked` to CI. Recommended — the §5.1 fix removed the *current* vector, but any of
  ~400 unlocked deps can break or poison CI the same way. CI-policy change = user
  approval required. Ask at the round-4 checkpoint.
- **Round-4 manual test**: owed once both lanes merge (build → Downloads → notify user).
- Remaining round-3 seeds: `e37b` (extraction schema), `208c` (Sentry residual info
  event). Follow-ups: `2b54` (hybrid-card test await nit), `efd3` (GA re-enable
  tracker), `bfa8` (rsac lock drift), `c335` (GitGuardian history findings), Soniox
  chain (319c→be03→0b93 — be03 is now a one-entry `MVP_SELECTABLE_PROVIDERS` flip after
  PR #97).

## 8. Standing security constraints (verbatim, always in force)

- Never `git add -A` / `git add .`; never stage `.seeds/`, `.claude/`, `Cargo.lock`.
- No API-key-shaped strings in commits, even rotated; defanged sentinels only
  ("test-key-not-real"). GitGuardian scans ALL PR-history commits — amend, don't stack.
- The rsac `requires_user_consent` capture.rs workaround is LOCAL-ONLY, never committed
  (`git checkout -- src-tauri/src/audio/capture.rs` before commit if touched).
- ADR-0023: analytics carry event names/categories only — never argument content.
- CI-policy changes are approval-gated (seed 8913 awaits the user).
- Don't force-push shared branches / rewrite shared history / touch secrets / alter CI
  autonomously — surface for approval.
