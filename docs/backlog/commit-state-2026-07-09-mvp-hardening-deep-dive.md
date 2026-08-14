# Commit State: MVP Hardening Deep Dive

Date: 2026-07-09

Branch: `master`

HEAD: `f97e19c` (`docs(handoff): checkpoint caad+10ac wave mid-flight`)

Active Seed: `audio-graph-99eb`

## Dirty Tree Caveats

Tracked product code was clean at the start of this run. The checkout already contained
untracked agent, preview-harness, backlog, and workflow-plan artifacts:

- `.agents/`
- `_preview-harness/`
- `docs/backlog/commit-state-2026-07-03-backlog-zero-mission.md`
- `docs/plans/2026-07-03-backlog-zero-plan.md`
- `docs/plans/2026-07-03-wave1-critique.md`
- `docs/plans/2026-07-03-wave23-critique.md`
- `docs/plans/2026-07-04-epic-1c2f-plan.md`
- `docs/plans/backlog-audit-workflow.js`
- `docs/plans/epic-1c2f-discover-workflow.js`
- `docs/plans/wave1-critique-workflow.js`
- `docs/plans/wave23-critique-workflow.js`

They are preserved as pre-existing user/session work and will not be staged, deleted, or
rewritten by this deep dive. `.seeds/issues.jsonl` is expected to change as findings are
recorded. No `sd sync` will run from this checkout while unrelated artifacts are present.

## Product Frame

The MVP is the durable desktop pipeline:

1. Discover a source and obtain explicit capture consent.
2. Capture source-local PCM through `rsac`.
3. Stream to Deepgram and normalize partial/final transcript revisions.
4. Project the revisioned transcript into useful notes and a temporal knowledge graph.
5. Persist event logs plus materialized views so sessions replay, export, delete, and
   recover deterministically.
6. Present the workflow through Ready, LiveNow, Review, and Inspect workspaces while
   backend capture lifecycle remains independently visible and controllable.

Native and composed speech-to-speech agents remain sibling modes. Deferred providers do
not return to the selector until the Deepgram-to-cloud-LLM path passes automated gates and
a clean manual test round.

## Owning Seeds

- `audio-graph-99eb`: coordinating MVP hardening epic.
- `audio-graph-caad`: notes apply policy during continuous speech.
- `audio-graph-10ac`: graph projection decode failures.
- `audio-graph-da33`: MVP selectability regression across onboarding and mode controls.
- `audio-graph-5c24`: UI/UX polish and native Tauri capture experience.
- `audio-graph-5c57`: hybrid JSONL/SurrealDB storage audit.
- `audio-graph-9c89`: session artifact lifecycle and migration.
- `audio-graph-fd9f`: reproducible `rsac` dependency/release strategy.
- `audio-graph-b9c7`: product and architecture documentation refresh.
- `audio-graph-efd3`: provider re-enable tracker after MVP stability.

## Scope

- Reconstruct product intent and current architecture from handoffs, ADRs, plans, code,
  tests, and current Seeds.
- Audit the complete capture, transcript, projection, persistence, replay, and UI path.
- Review the hybrid JSON/JSONL plus optional SurrealDB repository design and identify
  correctness, performance, migration, recovery, and product-operability gaps.
- Compare current `rsac` usage with the 0.4.1 release and produce a safe adoption plan.
- Define an opinionated, subject-specific UI direction and an execution plan that may
  replace the current shell while preserving backend-owned readiness and privacy rules.
- Verify the highest-value fixes in focused waves before widening provider selectability.

## Constraints

- Preserve the standing privacy, credential, and content-egress boundaries.
- Keep long-lived sockets, PCM, credentials, timing, and graph updates in Rust.
- The current request authorizes the rsac 0.4.1 Cargo pin and clean lockfile slice.
  CI/release workflow changes remain separately approval-gated.
- Do not commit or stage unrelated pre-existing files.
- Do not treat parser fixtures or registry status as proof that a provider is ready for
  user selection; runtime, readiness, content-egress, and cross-platform evidence remain
  required.

## Verification Plan

- Frontend: focused Vitest suites, typecheck, Biome, responsive screenshots, keyboard and
  reduced-motion checks.
- Backend: focused Rust tests for capture lifecycle, projection scheduling, persistence,
  replay, migration, and repository conformance; format and clippy with Rust 1.95.
- Cross-boundary: source descriptor round trips, capture start/stop/restart, continuous
  speech notes/graph updates, session reload/export/delete, and storage-pressure behavior.
- Release: a clean Windows manual round using the narrowed MVP provider set before any
  deferred provider is re-enabled.

## Progress Snapshot: 2026-07-09

### Research and design recorded

- `docs/research/mvp-ui-ux-2026-07-09.md`
- `docs/research/mvp-projection-correctness-2026-07-09.md`
- `docs/research/mvp-storage-audit-2026-07-09.md`
- `docs/research/rsac-0.4.1-capture-audit-2026-07-09.md`
- `docs/research/mvp-validation-devex-audit-2026-07-09.md`
- `docs/a11y/mvp-provider-truth-ui-2026-07-09.md`
- `docs/designs/2026-07-09-mvp-product-and-experience.md`
- `docs/plans/2026-07-09-mvp-hardening-wave-plan.md`

Storage verdict: keep versioned files as the only MVP canonical store, but do not call
the current runtime crash-safe. `audio-graph-1f71`'s side-effect-free historical
Review path is implemented and focused-tested in this working slice;
`audio-graph-90f3`'s authoritative durable commit remains the P0 integrity blocker.
SurrealKV remains one candidate for a future disposable index, with no engine
preselected before measured cross-session query demand.

Review/Live migration checkpoint: backend historical reads are pure, the shared
frontend serializes Review and Live, and starting capture clears historical view
projections. Capture/transcription start/stop and New Session now share a backend
lifecycle mutex; rotation clears session-scoped transcript/graph/chat/proposal
state and swaps writers before publishing the new ID. This is a bounded P0
hardening slice, not the full coordinated Start/Stop/Resume state machine.

rsac verdict: adopt v0.4.1 at full Git revision
`7956e6ef24a44672d502e72b0500efb27530e3b9`. Establish that Cargo pin and a clean,
committed application lock before treating repository or release Rust evidence as
reproducible. Do not enable compose,
mobile, or private macOS TCC SPI features for MVP.

UI verdict: preserve the tested data panels and typed Tauri/store contracts, but rewrite
shell composition around orthogonal backend lifecycle and foreground workspace. The
visible Ready/Live now/Review/Inspect vocabulary is now present, but Review and Live
remain serialized over one shared store; concurrent review of historical session A
while capture B remains live is still target work. Ready should show a planned route;
Live/Review should show an observed audited route once coverage is exhaustive.

### Accepted decisions

- ADR-0020: source clock, session mapping, discontinuities, and multi-source timing.
- ADR-0027: file-canonical durable commit, per-stream causal head vectors, complete
  provenance, and artifact lifecycle.
- ADR-0028: orthogonal lifecycle/workspace ownership, producer-last startup, bounded
  stop, and RecoveryRequired.
- ADR-0029: rebuildable query indexes only after measured product demand.
- ADR-0030: Ready, LiveNow, Review, and Inspect shell composition.
- ADR-0031: Current, AppendOnly, and Revised projection-basis classification.
- ADR-0032: layered validation evidence with deterministic offline MVP proof.
- ADR-0033: registry-backed MVP enablement at every content-bearing provider start.
- ADR-0034: positive egress evidence is usable from a partial ledger, but negative
  privacy claims require an explicit versioned exhaustive backend coverage marker.

ADR-0025 remains proposed for broader context efficiency and notes/graph retcon work;
its former `caad` basis-policy scope moved to ADR-0031. The accepted decisions above
are in force under the user's explicit instruction to ADRify the changes and continue.

### Architecture and design corrections recorded

- require capture lifecycle to remain separate from Ready/LiveNow/Review/Inspect state
- require RecoveryRequired for canonical writer/drain/finalization failure
- require start order writers -> consumers -> Deepgram -> projection workers -> rsac
  sources
- require the first sample to be processed or explicitly accounted as a gap
- require canonical session provenance plus per-stream head vectors and causal ids
- distinguish planned Ready route from observed audited route
- require preview to remain local, transient, non-provider, and non-canonical
- require rsac diagnostics to redact process, PID, window, device, and target identifiers
- moved rsac pin and Vitest reliability into Wave 0
- restored an intermediate Windows Round 4A build copied to Downloads
- split pure caad policy from its durability-gated runtime fold

### Implementation state

- `src-tauri/src/audio/capture.rs`: added the rsac 0.4.1
  `requires_user_consent` test-literal field, bounded startup acknowledgement
  after real build/start/subscription, rsac source-position timestamps with a
  monotonic fallback, and subscriber-drop observation alongside ring
  backpressure. The hardware-free capture filter passes 28 tests, including six
  new v0.4.1 contract tests. Two-phase audit/Commit startup, fatal-generation
  reconciliation, sibling sweeping, retained timed-out ownership, and clean
  shutdown are now integrated. Rate-change discontinuities, mid-capture spine
  supervision, and live three-platform capture remain open gates.
- `.gitignore`, `src-tauri/Cargo.toml`, and `src-tauri/Cargo.lock`: rsac now resolves
  from the official v0.4.1 Git commit
  `7956e6ef24a44672d502e72b0500efb27530e3b9`; the application lockfile is present
  and no longer ignored, but remains unstaged/untracked in this working slice.
  An isolated clean-worktree lock has the same SHA-256 as the main-worktree lock and
  `cargo metadata --locked` passes. The clean-worktree cloud library/test check also
  passes with Rust 1.95 after exposing and repairing the expected
  `requires_user_consent` compile delta (`9m 02s`).
- `src-tauri/src/projections.rs`, `projection_scheduler.rs`, and `speech/mod.rs`:
  the pure ADR-0031 slice is integrated. It classifies exact covered-subset
  currency, ignores appended partials, schedules one Background follow-up for
  append-only finals, reserves Replay for revisions, and rejects superseded
  completion by job/session/ledger identity. Per-kind job counters survive
  in-process scheduler reset, and same-session replacement workers cannot consume
  replacement metrics or materialize output. AppendOnly completions coalesce a
  current-basis Background follow-up instead of being misclassified as Revised.
  The broad projection filter passes 125 tests. Crash-durable visibility still
  waits for ADR-0027's Accepted event implementation in `audio-graph-90f3`.
- `src-tauri/src/commands.rs`: historical `load_session` is now a pure Review read
  with no live `AppState` argument. It replay-validates returned artifacts without
  rebinding the active graph, transcript ledger, projection materializer, or
  schedulers. Canonical transcript revisions now produce Review/export rows before
  the legacy segment file is considered, and replayed notes/graph state overrides
  ahead or divergent derived caches. Focused tests cover missing-artifact replay, active-state
  preservation, sequential A/B isolation, and diarization present/absent cases.
  Transactional Resume remains separate open lifecycle work.
- `src-tauri/src/state.rs`, `commands.rs`, and `TokenUsagePanel.tsx`: content
  start/stop, deferred-provider transitions, foreground chat work, and session
  rotation share one lifecycle lock. Stop bounded-joins ASR/provider workers;
  a timed-out handle is retained and fences producer restart/rotation until it
  actually exits. Rotation preflights every canonical writer, resets transcript/
  graph/projection/chat/proposal/audio aggregates, and publishes its new ID last;
  only a successful rotation finalizes the previous session. New Session is
  disabled while live, a failed backend rotation preserves the visible session,
  and success resets the frontend aggregates. Historical-load generations reject
  stale A-over-B and load-over-live responses. Full epoch/barrier fault injection
  and a coordinated one-command Start remain open in `audio-graph-4521` /
  ADR-0028.
- `audio-graph-da33`: provider-selectability truth is implemented at the Rust
  content-start boundary and mirrored in the UI/store. ASR, both chat paths,
  on-demand notes, and fixed realtime routes fail closed before runtime setup;
  stop/cleanup remains available. Startup hydrates settings passively without
  readiness egress, selected deferred routes remain inspectable, and mode cards
  cannot fall back to a blocked provider. Executed IPC-entrypoint coverage remains
  owned by `audio-graph-f567`. AWS named-profile onboarding now passively checks
  `list_aws_profiles` and accepts the route only when the configured profile exists;
  it does not resolve credentials or contact AWS.
- Provider mode cards now preserve a deferred provider only on the actually selected
  saved route; alternative cards derive from selectable candidates. Saved deferred
  rows retain settings/credential/model recovery navigation without becoming
  actionable.
- ADR-0032 created release-blocking Seeds for the deterministic golden data path,
  canonical task facade, executed Tauri IPC contract, packaged artifact smoke,
  hermetic Rust roots, toolchain doctor, and coverage ratchet.
- `package.json` now exposes `test:local` as the proven one-worker frontend gate.
  The generated-contract launchers honor `CARGO` and use `cargo.exe` on Windows;
  all four drift checks pass. A full Rust run also exposed that HOME isolation did
  not isolate the host OS keychain; the OpenRouter catalog test now injects an
  explicit empty store, and `audio-graph-1b91` was raised to P1 for complete
  data-root plus credential-backend hermeticity.

### Frontend verification baseline

- `bun run typecheck`: pass.
- `bun run check`: pass after a frozen forced Bun install repaired the stale native
  Biome package.
- Discovery `bun run test -- --maxWorkers=1`: pass, 68 files / 908 tests.
- default parallel `bun run test`: exit 1 after 35 worker startup timeouts with no
  assertion failures in the 301 tests that ran. `audio-graph-e2be` owns the fix;
  serial Vitest is the temporary authoritative command.
- Integrated UI/provider/a11y truth: `bun run test:local` passes 69 files /
  938 tests in 192.87 seconds; TypeScript and full-workspace Biome pass.
- `bun run build`: production Vite build passes after transforming 2,938 modules
  in 13m 06s.
- Generated audio-source, provider-registry, session-data-movement, and
  endpoint-credential-routing drift checks pass after the Windows launcher fix.
- A later one-file Vitest attempt timed out before starting its sole threads worker
  while concurrent Cargo/worktree I/O was active. This is recorded as additional
  `audio-graph-e2be` evidence, not treated as an assertion failure or retried as
  correctness proof.

### Backend verification

- Pre-read-only-Review baseline: `cargo +1.95.0 clippy --lib --tests
  --no-default-features --features cloud --locked -- -D warnings` passes.
- Pre-read-only-Review baseline from the rebuilt locked Rust library binary:
  1,464 passed, 7 intentionally ignored, 0 failed across 1,471 tests. Ignored
  cells are explicit live-provider, HOME-mutating, or torture gates; they are
  not counted as MVP-path proof. The focused `load_session` tests pass after the
  Review isolation change; a full post-change Rust rerun is not claimed here.
- Projection filter: 125 passed. Final scheduler refactor: 14 passed.
- The first full run exposed one non-hermetic OpenRouter test that read the host
  keychain and attempted a real request. After explicit credential-store injection,
  the focused test and the rebuilt full binary both pass without provider egress.

### Queue and hygiene

All material findings have been added to or reopened as Seeds. The storage audit Seed
`audio-graph-5c57` is closed because its report, alternatives, negative consequences,
test matrix, accepted ADRs, and focused follow-ups are complete. No product-correctness
Seed has been closed from research alone. No `sd sync`, staging, or commit has run.

### Final adversarial remediation checkpoint

- Quick Setup mount/focus readiness is passive (`refresh: false`) and scoped to
  the draft ASR/LLM/TTS/realtime provider ids. It cannot contact a persisted
  provider merely because onboarding opened; active probes remain owned by the
  future explicit ADR-0028 preflight.
- AWS named-profile discovery follows the SDK file overrides
  `AWS_CONFIG_FILE` and `AWS_SHARED_CREDENTIALS_FILE` without resolving the
  credential chain or making network calls.
- Projection job counters now survive in-process reset, and generated patches
  are ownership-checked before and atomically across the apply decision. A stale
  same-session worker cannot materialize or consume replacement metrics, while
  offline replay identifiers remain deterministic.
- Store-level settings-hydration failures now use the active locale instead of
  hardcoded English.
- Every generated Rust contract launcher now uses Rust 1.95 and `--locked`.
- The storage crash-consistency implementation has been reduced to a clean,
  independently mergeable canonical-log kernel/compatibility-reader slice. No
  storage code was layered into this broad dirty checkout.

### Integrated hardening and final local evidence (2026-07-10)

- Visible workspace vocabulary now reads Ready (Live now during capture),
  Review, and Inspect while the internal view keys and shared store remain.
  Review and Live are still serialized; concurrent Review-while-Live is not
  claimed.
- Data-route reporting now follows ADR-0034: a partial ledger may prove positive
  egress, but a closed capture cannot prove content stayed local. No exhaustive
  backend coverage version exists, so every negative claim remains Unknown.
- Capture uses Ready -> synchronized `CaptureStarted` -> Commit ordering, an
  ordered raw/processed Reset barrier, exact fatal-handle ownership, sibling
  reconciliation, retirement fencing, and clean-exit `CaptureStopped`.
- Duplicate/unreconciled capture generations now reject before writer or
  process-lifetime worker side effects and are revalidated at final manager
  ownership.
- Historical canonical projection streams remain authoritative even when empty;
  invalid replay fails closed and cannot promote an orphan materialized cache.
- Destructive session paths use target-session containment, artifact-first /
  index-last retry behavior, active-session exclusion, and malformed-index
  fail-closed behavior for both production and explicit-root repositories.
- The isolated canonical-log kernel remains unintegrated: 12 tests passed before
  its final Windows handle correction, while the post-correction 13-test run did
  not reach assertions. P0 `audio-graph-90f3` stays open.

Current integrated evidence:

- `bun run check`: pass, 171 files.
- `bun run typecheck`: pass.
- focused shell/privacy/canonical-view Vitest: 4 files / 91 tests pass.
- `bun run test:local`: 70 files / 962 tests pass.
- all four generated-contract drift checks pass.
- `cargo +1.95.0 fmt --all -- --check`: pass.
- strict Rust 1.95 cloud/all-target Clippy: pass.
- serial Rust 1.95 cloud library suite: 1,498 passed, 0 failed, 8 explicitly
  ignored across 1,506 tests.
- final `verify:fast`: pass, including four generated contracts, Seeds JSON
  stress, zero docs/Seeds hygiene findings, and `git diff --check`.
- `bun run build`: pass; Vite transformed 2,940 modules and produced the
  frontend bundle in 15m 17s. This is frontend-bundle proof, not a packaged
  Tauri desktop build.

No workflow, staging, commit, `sd sync`, or push operation is authorized by or
has run from this checkpoint. The detailed successor handoff is
`docs/backlog/handoff-2026-07-10-mvp-hardening.md`.
