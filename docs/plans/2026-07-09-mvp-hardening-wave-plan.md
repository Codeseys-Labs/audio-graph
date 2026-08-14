# AudioGraph MVP hardening wave plan

Date: 2026-07-09

Status: active

Coordinating Seed: `audio-graph-99eb`

## Goal

Ship the durable desktop path before enabling deferred providers:

`source -> acknowledged rsac capture -> Deepgram ASR -> revisioned transcript
-> automatic notes and temporal graph -> crash-safe session replay -> calm
Ready/LiveNow/Review UX`.

Realtime speech-to-speech and additional providers remain gated until capture,
processing, storage, privacy-route, and replay evidence is green.

## Inputs

- `docs/backlog/handoff-2026-07-08-caad-10ac-wave.md`
- `docs/research/mvp-ui-ux-2026-07-09.md`
- `docs/research/mvp-projection-correctness-2026-07-09.md`
- `docs/research/rsac-0.4.1-capture-audit-2026-07-09.md`
- `docs/research/mvp-storage-audit-2026-07-09.md`
- `docs/designs/2026-07-09-mvp-product-and-experience.md`
- accepted ADR-0020, ADR-0027, ADR-0028, ADR-0029, ADR-0030, ADR-0031,
  ADR-0032, and ADR-0033

## Current baseline

- Branch: `master`
- HEAD at discovery: `f97e19c`
- Product code was clean at discovery.
- Pre-existing untracked agent and planning artifacts are out of scope and must
  remain untouched.
- New research, ADRs, plan, and Seeds changes are intentionally uncommitted.
- `bun run typecheck`: pass.
- `bun run check`: pass after repairing the local frozen Bun install.
- Discovery serial frontend baseline: 68 files, 908 tests, pass.
- Integrated UI/provider/a11y truth baseline: 69 files, 938 tests, pass through the
  new `bun run test:local` facade.
- Default parallel `bun run test`: assertions pass, but 35 fork workers time
  out during startup; tracked by `audio-graph-e2be`.
- Local Rust verification now resolves the present, unignored working-tree lock
  to rsac v0.4.1 at full
  Git revision `7956e6ef24a44672d502e72b0500efb27530e3b9`. Locked metadata,
  cloud library/test check, format, and the 125-test projection slice pass.
  Cross-platform resolution and CI/release input deduplication remain open in
  `audio-graph-fd9f`.

Do not run `sd sync` in this broadly dirty checkout.

## Gates

### Accepted architecture

- ADR-0020: source-clock/session-clock/discontinuity contract
- ADR-0027: file-canonical durable session storage
- ADR-0028: orthogonal capture lifecycle and foreground workspace
- ADR-0029: measured-demand gate for rebuildable query indexes
- ADR-0030: Ready, LiveNow, Review, and Inspect shell
- ADR-0031: Current, AppendOnly, and Revised projection-basis policy
- ADR-0032: layered evidence whose deterministic golden MVP fixture is release-blocking
- ADR-0033: registry-backed enablement at frontend and backend content-start boundaries

ADR-0025 remains proposed for the broader context-efficiency and notes/graph
retcon design. It no longer governs or blocks the focused `caad` classifier.

### Workflow approval gate

Changes to `.github/workflows/*`, release inputs, or generated provider
artifacts require a clean worktree and explicit approval under the repository
workflow. The user's explicit rsac 0.4.1 request authorizes the Cargo source and
lockfile slice, but it still runs first in an isolated clean worktree.

### Provider gate

No deferred provider becomes actionable until:

- its content-egress checklist is complete
- credential and model readiness is proven from saved credentials
- source/capture compatibility is proven
- parser fixtures and live runtime behavior pass
- failure, cancellation, and recovery are tested
- privacy route and data-movement evidence are complete

ADR-0033 makes the backend command boundary authoritative. Saved deferred
settings, credential maintenance, and scoped diagnostics remain available, but
new content-bearing starts fail before transport/audio subscription; stop and
cleanup always remain available.

## Wave 0 — Reproducibility and validation authority

### 0A. Pin the Rust dependency before Rust evidence

Seed: `audio-graph-fd9f`

Use a clean, flat-named worktree:

- pin rsac to full Git revision
  `7956e6ef24a44672d502e72b0500efb27530e3b9`
- regenerate and track a clean `src-tauri/Cargo.lock`
- make Cargo metadata/check/test use `--locked`
- document an explicit local sibling `[patch]` override

Do not edit CI or release workflows in this slice. Their duplicate revision
inputs are removed only after separate workflow approval.

No Rust result closes a Seed until it runs against this Cargo-resolved revision.

Current evidence: an isolated worktree resolves rsac 0.4.1 at full commit
`7956e6ef24a44672d502e72b0500efb27530e3b9`; its generated lock is byte-identical
to the main-worktree lock and `cargo metadata --locked` passes. The first cloud
test compile exposed the expected new `requires_user_consent` fixture field and
passed after the repair (`cargo +1.95.0 check --lib --tests
--no-default-features --features cloud --locked`, 9m 02s).

### 0B. Establish the frontend test authority

Seed: `audio-graph-e2be`

- diagnose and fix default Vitest worker startup, or
- explicitly use `bun run test -- --maxWorkers=1` as the temporary
  authoritative command in every UI Seed

The current serial baseline is 68 files and 908 passing tests. Wave 4 does not
claim broad UI verification from the failing default parallel command.

Implementation checkpoint: `package.json` now exposes `bun run test:local` and
the integrated suite passes 69 files / 938 tests. `audio-graph-e2be` remains open
for the default worker-pool root cause and bounded CI parallelism.

### 0C. Establish claim-bounded validation

Seeds: `audio-graph-2add`, `audio-graph-9e23`, `audio-graph-f567`, and
`audio-graph-211f`

Implement ADR-0032 in this order:

1. a deterministic timed-PCM through restart/replay/export golden fixture;
2. one local task facade for fast, focused, contracts, full, live, and release;
3. an executed frontend-to-Tauri IPC manifest/contract;
4. exact packaged-artifact startup under an isolated data root.

No live-device or direct provider REST script substitutes for the golden
fixture. A skipped required leg is a failure; retries do not establish correctness.

Implementation checkpoint: all four generated-contract wrappers now honor the
`CARGO` override and resolve `cargo.exe` on Windows; every current generated
artifact drift check passes. The broader task facade, executed IPC manifest,
golden fixture, and packaged smoke remain open.

## Wave 1 — Projection correctness

OpenRouter response handling and job/session correlation do not depend on the
storage or shell work. The `caad` policy is governed by accepted ADR-0031.

### 1A. Repair and test the pure `audio-graph-caad` policy

Source implementation commits for reference:

- `b5130fb` — basis currency classifier
- `ef66099` — append-only scheduler follow-up policy
- `d963d18` — speech/runtime tests

Do not apply `200a87a` wholesale: it edits accepted ADR-0024. Implement the
focused policy from accepted ADR-0031 without changing accepted ADR bodies.

Files:

- `src-tauri/src/projections.rs`
- `src-tauri/src/projection_scheduler.rs`

Implementation:

1. Introduce one basis currency classifier.
2. Hash the current-ledger subset covered by the basis before checking for
   appended spans.
3. Classify same-span hash mismatch as
   `Revised(TranscriptHashMismatch)`.
4. Classify append-only stale completion for Background follow-up.
5. Classify revised completion for Replay repair.
6. Make `validate_basis` delegate to or exactly match the classifier.

Do not yet change runtime apply/persistence or write a test that equates queue
enqueue with durable persistence. That fold is Step 1D after Wave 3A provides
the authoritative commit API.

Acceptance:

- wrong hash is always Revised with TranscriptHashMismatch
- appended-only spans are classified independently from revision
- scheduler chooses Background follow-up for append-only and Replay repair for
  revision
- no regression to scheduler failure/cancellation behavior

Verification:

- `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check`
- focused projections and scheduler suites
- full Rust library suite after the rsac compile delta is resolved
- independent review of the final diff

Rollback:

- keep changes unstaged
- reverse only the focused apply-patch hunks if a gate fails
- preserve the reference branch and Seeds evidence

Implementation checkpoint: Steps 1-6 above are integrated. The current basis is
projection-eligible finals only, stale job/session/ledger completion is ignored
content-free, and the broad projection filter passes 125 tests. Step 1D remains
blocked on the canonical Accepted acknowledgement; no enqueue-only patch is
being represented as durable or visible.

### 1B. Correlate projection completion

Seed: `audio-graph-ab10`

Files:

- `src-tauri/src/projection_scheduler.rs`
- `src-tauri/src/speech/mod.rs`
- `src-tauri/src/state.rs`
- scheduler persistence fixtures as required

Implementation:

- carry `job_id` and `session_id` through start, success, failure,
  cancellation, and telemetry
- ignore stale completion after rotation
- persist correlation in queue state

Acceptance:

- old A completion cannot consume same-kind B job after rotation
- same guarantee for failure and cancellation
- recovery preserves identity

### 1C. Repair OpenRouter graph completion handling

Seed: `audio-graph-10ac`

Files:

- `src-tauri/src/llm/openrouter.rs`
- projection OpenRouter call sites and private output structs
- focused response, retry, telemetry, and projection fixtures

Implementation:

- per-request Graph token floor of 2048
- nullable content and typed embedded-error support
- response text capture with content-free shape telemetry
- bounded retry classification
- selected-provider, served-model, finish-reason, and reasoning-token
  diagnostics

Acceptance:

- graph schema and fallback share the override
- other consumers retain configured budgets
- null/empty completion is a clean terminal error
- malformed/truncated body retains bounded retry
- no content, reasoning, provider error text, or secret reaches logs

### 1D. Fold progressive notes into the durable runtime

Executes only after Wave 3A exposes the ADR-0027 authoritative event commit API.
ADR-0031 governs semantic applicability.

Files:

- `src-tauri/src/speech/mod.rs`
- `src-tauri/src/state.rs`
- durable projection writer/materializer integration

Acceptance:

- append-only patch becomes visible only after its canonical event is Accepted
- revised patch never commits live or derived state
- snapshot lag is recoverable and never turns a committed event into Failed
- accepted append-only replay reproduces identical materialized notes
- the state regression test proves durable acknowledgement, not queue enqueue

Immediately after 1D and 1C pass, run the intermediate Round 4A Windows
continuous-speech checkpoint described below.

## Wave 2 — Capture truth and rsac 0.4.1

Seeds: `audio-graph-b5ef`, `audio-graph-b718`,
`audio-graph-99ed`, `audio-graph-9daa`, and `audio-graph-fd9f`

### 2A. Make capture lifecycle authoritative

- acknowledge build, start, and subscription before Running
- tie transcription to the acknowledged capture generation
- reconcile fatal exit and last-active-source state
- make partial-source policy explicit; MVP default is rollback

### 2B. Preserve source time and loss

Governed by accepted ADR-0020.

- use rsac source-position timestamps
- preserve gaps through downmix/resampling
- expose ring, subscriber, raw/processing, and consumer drops separately
- flush or record pending-tail loss at sample-rate changes
- diagnostics use descriptor kind and redacted stable source id only; never raw
  PID, process/window name, device label, or target path

### 2C. Correct macOS Process Tap readiness

- distinguish ordinary device capture from Process Tap
- keep private `macos-tcc-spi` disabled
- represent unknown/TCC/no-signal outcomes honestly

### 2D. Deduplicate CI and release revision inputs

Workflow-approval gated after Wave 0A proves the Cargo pin:

- remove separate CI/release rsac clone and revision inputs
- derive release metadata from the Cargo-resolved lock
- require `--locked` in approved workflows

Verification:

- hardware-free startup, timing, drop, and rate-transition tests
- Linux known-tone/stop test plus minimum PipeWire runtime
- macOS virtual-device and Process Tap outcome
- Windows virtual-endpoint PID and tree tests
- three-OS Cargo-resolved revision equality

## Wave 3 — Storage integrity

Governed by accepted ADR-0027.

Execute in a clean, flat-named worktree; do not layer the persistence rewrite
over the broad research/Seeds checkout.

### 3A. Authoritative durable commit

Seed: `audio-graph-90f3`

- framed versioned events and idempotency keys
- compatibility reader for the current unversioned event rows
- per-stream monotonic heads, stable event/causal ids, and complete session
  head vector
- canonical session provenance for lifecycle, redacted source identity/format,
  clock mappings, discontinuities, drop summaries, and route transitions
- Pending versus Accepted state
- canonical event durability before live commit
- snapshot head/hash basis and log-first authority
- torn-final-row recovery
- ENOSPC, short-write, flush/sync, rotation, and shutdown fault injection

### 3B. Side-effect-free historical review

Seed: `audio-graph-1f71`

- isolate loaded review state from the active-session aggregate
- do not rotate writers or autosave targets for Review
- reserve Resume for a later transactional command

### 3C. Complete session lifecycle

Seeds: `audio-graph-be7c`, `audio-graph-617e`,
`audio-graph-34be`, `audio-graph-70a3`, and
`audio-graph-51e0`

- one typed artifact manifest
- persistent scheduler mutations
- broader artifact migrations and rollback after the Wave 3A event reader is
  compatible
- complete content-free data-movement events
- exact export/delete/purge/recovery parity

Session import is not an MVP acceptance item. A separate Seed owns any future
bundle-import UI and migration conformance.

### 3D. Operability

Seed: `audio-graph-2d22`

- consistent pagination
- storage size and pressure
- retention, pinning, archive/export, and compaction policy

SurrealDB is not part of Wave 3. Under ADR-0029, `audio-graph-21c4` remains a
post-MVP, measured-demand, rebuildable-index task.

## Wave 4 — Provider truth and shell rewrite

Keep the focused provider-truth fix in the current worktree. Execute the
ADR-0028 lifecycle and ADR-0030 shell work in their own clean, flat-named
worktree.

### 4A. Enforce current MVP selectability

Seed: `audio-graph-da33`

This truth fix is independent of the lifecycle and shell rewrite:

- every actionable provider/mode surface obeys generated
  `ui_selectable`
- no fallback selects a non-selectable provider
- readiness requires the Deepgram plus selectable-LLM route, not credential
  presence alone
- deferred modes are non-actionable and labelled Planned or Not in MVP

Implementation checkpoint: the backend is authoritative for transcription,
chat, notes synthesis, and fixed realtime starts; UI/store guards provide early
feedback; teardown is ungated; and saved deferred routes remain inspectable.
The full serial frontend suite passes 938 tests. Actual Tauri invoke-contract
tests remain in `audio-graph-f567`. AWS named-profile onboarding now performs a
passive local profile-list existence check without resolving the credential
chain or contacting AWS. This Seed is not closed from helper and component
evidence alone.

### 4B. Atomic Ready and start

Seed: `audio-graph-10ff`; governed by ADR-0028 and blocked on Wave 2A capture
acknowledgement plus Wave 3A durable acceptance.

- typed preflight
- one Start note session command
- explicit Starting and rollback
- passive Ready shows planned route without egress
- active provider probes have a draft session/audit owner
- start order is writers -> bounded consumers -> Deepgram ready -> projection
  workers -> rsac sources last
- first captured sample is processed or explicitly accounted as a discontinuity
- source, provider, storage, and observed-route truth
- bounded Stop/drain enters RecoveryRequired on incomplete durable finalization

### 4C. Rewrite shell composition

Seed: `audio-graph-19c7`; governed by ADR-0030 and dependent on the ADR-0028
lifecycle contract.

- Ready, LiveNow, Review
- orthogonal backend lifecycle and foreground workspace so Review A can coexist
  with Live B
- contextual Inspect and RecoveryRequired
- automatic notes primary
- transcript and temporal spine secondary
- composite health, exact route, and non-dismissible storage recovery
- local/transient sample preview that never reaches provider or canonical store

Supporting Seeds:

- `audio-graph-50e3`
- `audio-graph-8d18`
- `audio-graph-a339`
- `audio-graph-8e51`

Verification:

- component and state-machine fixtures
- screenshots at 1440, 1024, and 768 pixels, dark/light, idle/loading/error
- keyboard, 200 percent zoom, forced colors, reduced motion
- NVDA and VoiceOver
- packaged three-OS smoke

## Intermediate Round 4A — Projection handoff checkpoint

Run immediately after the reproducible rsac baseline, `10ac`, and the
durability-gated `caad` runtime fold are integrated. Do not wait for the shell
rewrite; this keeps failures attributable to the handoff lanes.

- produce a Windows dry-run build
- copy the exact executable/bundle under test into the user's Downloads folder
  with a revision-bearing name
- record build path, hash, rsac revision, settings route, and commands in the
  owning Seeds and commit-state handoff
- run continuous speech long enough to keep transcript append ahead of notes
  generation
- require notes before pause, clean graph decoding, content-free route/finish
  telemetry, and deterministic event/snapshot reload
- keep `caad` and `10ac` open if any manual or replay cell fails

## Wave 5 — MVP proof and provider re-enablement

### Automated

- frontend typecheck, Biome, serial and repaired parallel Vitest
- full Rust format, clippy, unit, integration, replay, and fault-injection
- same Cargo-locked rsac revision on all targets
- packaged capture/start/stop/restart/export

### Final end-to-end manual gate

- notes visible before a pause
- no graph response-decode or missing-discriminator failures
- content-free route, finish-reason, and reasoning-token telemetry
- capture discontinuities and drops represented honestly
- stop finalizes canonical events before Review reports Saved
- restart restores transcript, speakers, notes, graph, route, and timeline
  deterministically

### Deferred providers

Only after MVP proof, re-read `sd ready` and advance the highest-value
provider Seed whose runtime and content-egress gates are complete. Do not
re-enable providers as a batch.

## Session close

- update or close only Seeds whose acceptance evidence is complete
- run `sd ready --format json`, `sd blocked --format json`, and
  `sd doctor --json`
- record commands and failures in Seed extensions
- do not `sd sync` while unrelated or broad changes remain
- do not commit without explicit user confirmation
