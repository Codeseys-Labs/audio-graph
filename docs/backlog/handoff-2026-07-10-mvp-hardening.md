# AudioGraph MVP hardening handoff

Date: 2026-07-10

Baseline: `master` at `f97e19c251e4c227aade1289b2aba56e0d40ffca`

Status: integrated hardening complete; final deterministic validation in progress

## 1. Executive MVP verdict

AudioGraph's intended product is a local-first desktop memory recorder:

`selected desktop audio -> rsac -> Deepgram -> revisioned transcript -> notes + temporal graph -> file-canonical Review/export/delete`

The current slice materially hardens that route and makes the product surface
more truthful, but the MVP must **not** yet be called ship-safe. The P0 blocker
is still `audio-graph-90f3`: projection/transcript queue admission can precede
the filesystem durability boundary, so a UI-visible change can still disappear
after process or power loss. A packaged live Windows/macOS/Linux capture round
and the full provider/data-movement coverage matrix also remain open.

The safe current product boundary is:

- Deepgram is the only ASR enabled for a new MVP content-bearing session.
- Registry-enabled LLMs remain selectable; deferred providers fail closed at
  the backend content-start boundary.
- JSON/JSONL files are the only canonical MVP authority.
- SurrealDB is not selectable and is not a hybrid co-authority.
- Review and Live are serialized until session-scoped stores/event envelopes
  support honest concurrent Review-while-Live.
- Positive egress evidence is visible; every negative "stayed local" claim is
  Unknown until the backend emits a versioned exhaustive-coverage marker.

## 2. Product route versus accepted target

The visible shell now uses Ready (changing to Live now while capturing), Review,
and Inspect. Internal view keys remain `during`, `after`, and `analysis`; Inspect
is still a peer tab. This is a low-risk vocabulary/composition checkpoint, not a
claim that ADR-0028's independent lifecycle/workspace model or all of ADR-0030
has landed.

Current capture and transcription starts are still separate commands. Review
Open is blocked while live, starting capture exits historical Review, and New
Session is blocked while live. The accepted target is one backend-owned Start /
Stop / Resume lifecycle with reverse-order rollback and concurrent foreground
Review that never steals live ownership.

## 3. Integrated UI, UX, accessibility, and provider truth

- Reworked onboarding and settings around the registry-backed MVP provider set,
  saved credential presence, explicit readiness, and deferred-provider labels.
- Enforced the registry at frontend actions and backend content-bearing start
  commands; saved deferred routes remain inspectable and stoppable.
- Made startup credential discovery passive so mounting onboarding cannot cause
  provider egress.
- Added actionable empty/error states, clearer session/live boundaries, locale-
  correct store errors, keyboard/focus behavior, and WCAG-oriented regression
  coverage.
- Made historical `load_session` a read-only artifact query and prevented stale
  historical responses from overwriting the active view.
- Made an existing empty canonical projection stream authoritative over orphan
  notes/graph caches; Review renders canonical empty state instead of reviving
  legacy cache state.
- Kept privacy reporting tri-state. A partial ledger may prove egress, but it
  cannot prove the absence of egress.

This is not a from-scratch frontend replacement. Existing typed Tauri/Zustand
contracts and proven data panels were retained while the highest-risk truth and
navigation seams were hardened.

## 4. rsac 0.4.1 and capture lifecycle

- Pinned rsac v0.4.1 to Git revision
  `7956e6ef24a44672d502e72b0500efb27530e3b9` for Windows, macOS, and Linux with
  target-specific features and default features disabled.
- Restored `src-tauri/Cargo.lock` as an application lockfile input. It is present
  and no longer ignored, but remains untracked in this dirty integration slice.
- Uses rsac source timestamps with a monotonic fallback and exposes subscriber
  drop deltas separately from ring/raw-channel pressure.
- Capture startup is two-phase: rsac build/start/subscribe yields Ready; audio
  remains gated; the first-source movement row is flushed/synchronized; Commit
  then releases the receive loop.
- Pipeline and dispatcher readiness use an ordered Reset barrier. Restart/New
  Session clears per-source resampler, accumulator, and timestamp state and
  waits until the dispatcher has handled the old processed prefix.
- Timed-out workers remain owned and fence restart/rotation until exit. Fatal
  callbacks reconcile by exact handle identity, cannot be replaced across a
  pending last-source boundary, sweep sibling dead handles, and emit one
  aggregate Stop.
- Clean application shutdown stops capture handles and closes capture movement
  evidence before canonical writer shutdown.
- Background extraction/proposal work captures the submission session id and
  revalidates ownership immediately before graph, persistence, status, or UI
  commit.

Still open: source-rate discontinuity/provenance, coordinated provider startup
and rollback, and active mid-capture pipeline/dispatcher supervision
(`audio-graph-2e97`).

## 5. End-to-end data-path ownership

Rust owns long-lived sockets, credentials, rsac PCM, source timing, session
rotation, graph/projection updates, and canonical artifacts. React owns
configuration, explicit control, and display.

Implemented boundary order:

1. Preflight canonical writers and process-lifetime pipeline/dispatcher.
2. Send ordered audio Reset and wait for dispatcher acknowledgement.
3. Prepare rsac and wait for build/start/subscription Ready.
4. Flush/synchronize the first-source `CaptureStarted` row.
5. Commit capture and release PCM into the bounded raw pipeline.
6. Preserve source identity through per-source resampling and processed fanout.
7. Fence transcript extraction/proposals by active session ownership.
8. On last Stop/fatal/exit, reconcile consumers and append `CaptureStopped`.

This order is stronger than the discovery baseline. It does not make the
separately started ASR/provider route atomic and does not make transcript or
projection queue admission crash-durable Accepted.

## 6. Storage authority and SurrealDB verdict

Files remain the right MVP shape because they are inspectable, portable, and
already own replay. The implementation now:

- treats canonical projection-log existence as authority even when empty;
- fails closed on invalid canonical replay instead of falling back to cache;
- keeps historical loading side-effect-free;
- preflights writer open before publishing lifecycle boundaries;
- rejects active-session deletion and cross-session/self-index artifact paths;
- deletes managed artifacts before the session index and preserves the index as
  a retry anchor on any residual failure;
- excludes the active session from retention purge; and
- backs up and rejects malformed `sessions.json` for both production and
  explicit-root repositories instead of replacing it with an empty index.

Remaining storage work:

- durable Pending -> Accepted acknowledgement before live/materialized advance;
- framed tail recovery and subprocess crash-point tests;
- one typed artifact manifest for export/delete/purge/recovery/backup/accounting;
- typed residual IPC and no-follow/directory-handle deletion;
- session head vectors, snapshot basis hashes, and canonical provenance; and
- scheduler and broader movement durability.

The feature-gated SurrealDB Mem (`kv-mem`) code is a partial experiment only.
It has no runtime selector, stores schemaless rows, lacks diarization/movement
parity, and deletes metadata before children. It must remain non-selectable
until it is transactional, repository-conformance-complete, rebuildable, and
passes the same migration/backup/deletion/crash tests as files.

## 7. Isolated canonical-log branch

Worktree: `E:/CS/github/audio-graph-canonical-log`

Branch: `codex-audiograph-canonical-log`

Baseline: `f97e19c`

Scope is intentionally only:

- `src-tauri/src/persistence/mod.rs` (one module export)
- `src-tauri/src/persistence/canonical_log.rs` (new kernel and tests)

The branch explores a typed, locked, hash-linked appender with poison/recovery
semantics. It has no runtime callers. Twelve focused tests passed before the
final Windows read/write-handle correction; the post-correction 13-test run was
stopped during codegen before assertions. Do not integrate it until that suite
passes and parent-directory durability, reader coordination, quarantine
manifest integration, expected-head/suffix-truncation detection, migration
fixtures, and runtime Pending -> Accepted ordering are designed and tested.

## 8. Privacy and data-movement evidence

Capture Start/Stop and projection-provider rows are now real production
producers, and movement JSONL appends are process-serialized. Append order is
the lifecycle authority because independent producers can stamp wall-clock time
before contending for the append lock.

Coverage is still incomplete. ASR, TTS/realtime, credentials, artifact
load/export/delete, and promotion do not all emit success/failure/blocked rows.
ADR-0034 therefore requires a backend-owned, versioned exhaustive-coverage
marker before any negative egress claim. The frontend marker is currently
absent by design; closed local capture still renders Unknown. Known off-device
content rows remain visible immediately.

## 9. Validation ledger by claim

Final integrated rows will be filled only from commands executed after the last
code change.

| Command | Current result | What it proves |
|---|---|---|
| `bun run typecheck` | pass | Type checking only |
| `bun run check` | pass, 171 files | Biome lint/format only |
| `bun run test:local` | pass, 70 files / 962 tests | Executed frontend assertions, serial |
| `bun run build` | pass, 2,940 modules in 15m 17s | TypeScript + Vite frontend bundle only |
| `bun run verify:contracts` | pass, all four generated contracts current | Generated-contract drift only |
| `bun run verify:fast` | pass; Seeds stress and docs hygiene report zero findings | Static/contracts/docs/Seed/diff hygiene; not Rust tests |
| `cargo check --tests` | superseded by the executed full test build | Test targets compile; assertions do not execute |
| `cargo fmt --all -- --check` | pass | Rust formatting only |
| strict `cargo clippy ... -D warnings` | pass with Rust 1.95, cloud, all targets | Rust lint compilation |
| serial `cargo test --lib ...` | pass, 1,498 passed / 0 failed / 8 ignored | Executed Rust assertions |
| `bun run tauri build` | not run | Packaged desktop build |
| live provider/device gate | not run | Real transport/hardware evidence |

## 10. Dirty-worktree and git boundaries

This is a broad, already-dirty integration checkout. At handoff drafting it has
110 modified/untracked status entries and includes pre-existing `.agents/`,
`_preview-harness/`, and July 3/4 planning artifacts. No staging, commit, push,
workflow edit, or `sd sync` has run. Do not sweep these changes into one commit.
Split reviewed slices in clean branches/worktrees after the final diff is
reconciled.

## 11. Open P0/P1 Seeds

- P0 `audio-graph-90f3`: durable canonical commit before live/materialized
  acceptance; isolated kernel is not runtime-ready.
- P1 `audio-graph-2e97`: active pipeline/dispatcher supervision during capture.
- P1 `audio-graph-be7c`: one typed artifact manifest and deletion/export parity.
- P1 `audio-graph-70a3`: exhaustive production movement coverage.
- P1 `audio-graph-51e0`: truthful route UI, blocked on exhaustive coverage.
- P1 `audio-graph-1f71`: session-scoped Review/Live state and event envelopes.
- P1 `audio-graph-1b91`: hermetic data-root/credential test isolation.
- Capture/live/platform work remains under `audio-graph-b5ef`,
  `audio-graph-b718`, `audio-graph-fd9f`, and `audio-graph-8913`.

## 12. Exact next commands

```powershell
# Frontend truth and bundle
bun run check
bun run typecheck
bun run test:local
bun run verify:contracts
bun run verify:fast
bun run build

# Rust deterministic local gates
cd src-tauri
$env:CARGO_BUILD_JOBS='1'
$env:CARGO_INCREMENTAL='0'
$env:AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST='1'
cargo +1.95.0 fmt --all -- --check
cargo +1.95.0 clippy --locked --no-default-features --features cloud --all-targets -- -D warnings
cargo +1.95.0 test --lib --locked --no-default-features --features cloud -- --test-threads=1
```

On this Windows host, default parallel Vitest has produced worker-startup
timeouts, and incremental Rust workers have previously gone nearly idle while
holding build state. Single-worker Vitest plus `CARGO_INCREMENTAL=0` and one
Cargo job are the current reproducible local defaults. A linker override is a
local DevEx workaround, not release evidence.

## 13. Provider re-enable gates

Do not enable another ASR, TTS, or realtime provider merely because its adapter
compiles or parser fixtures pass. Promotion requires:

1. registry descriptor and generated frontend parity;
2. credential/readiness behavior with no passive egress;
3. source/capture-format compatibility and bounded cancellation;
4. production success/failure/blocked movement rows with redaction tests;
5. offline fixture/replay evidence and content-start fail-closed tests;
6. live credentialed provider/device evidence on supported platforms;
7. packaged restart, stop, recovery, export, and deletion evidence; and
8. explicit addition to `MVP_SELECTABLE_PROVIDERS` only after the above passes.

## Decision and research index

- `docs/designs/2026-07-09-mvp-product-and-experience.md`
- `docs/research/mvp-ui-ux-2026-07-09.md`
- `docs/research/rsac-0.4.1-capture-audit-2026-07-09.md`
- `docs/research/mvp-storage-audit-2026-07-09.md`
- ADR-0027, ADR-0028, ADR-0030, ADR-0032, ADR-0033, and ADR-0034
