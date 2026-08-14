# MVP validation and developer-experience audit

Date: 2026-07-09

Status: complete, implementation tracked in Seeds

Scope: local and CI validation claims, end-to-end MVP data-path evidence,
generated contracts, package/toolchain reproducibility, contributor guidance,
IPC compatibility, and packaged-artifact smoke coverage.

Source-anchor note: unqualified line references in discovery findings refer to
HEAD `f97e19c`. Symbols and dated implementation checkpoints are authoritative
for the current working slice.

## Executive finding

AudioGraph has useful unit, parser-fixture, replay, and frontend coverage, but it
does not yet have one authoritative test proving the MVP path:

`PCM + source clock -> processing -> Deepgram streaming contract -> canonical
transcript/speaker events -> notes/graph projection -> durable commit ->
restart/replay -> export`.

Several scripts currently make narrower claims than their names or docs imply.
They can pass while capture, streaming integration, persistence, projection, or
replay is broken. ADR-0032 therefore separates deterministic offline, live
device/provider, and packaged-release evidence and forbids a command from
claiming more than it asserts.

## Evidence already present

- `src-tauri/src/persistence/mod.rs` has valuable file-repository replay,
  conformance, and artifact-loss recovery tests.
- `src-tauri/src/asr/fixtures.rs` and `src-tauri/src/asr/event_fixtures.rs`
  cover provider-neutral ledger replay and Deepgram event parsing.
- Generated provider and endpoint-credential contracts are useful
  cross-language foundations.
- The frontend serial baseline completed 68 files and 908 tests successfully at
  discovery; the integrated provider/UI/a11y truth slice later completed 69
  files with 938 passing tests.
- Linux and macOS live enumeration exists and is useful as a capability smoke.

These are necessary layers. None alone proves the complete product path.

## P0 findings

### No authoritative deterministic MVP data-path fixture

`src-tauri/src/audio/live_audio_smoke.rs` treats real capture as best effort:
capture failures and zero buffers are logged rather than asserted. The suite
can pass after enumeration and format checks without receiving usable PCM.

`scripts/test-rsac-windows.ps1` warns rather than fails when the captured WAV is
too small to prove meaningful audio.

`scripts/test-cloud-pipeline.ps1` sends provider REST requests directly. It
does not exercise AudioGraph capture, processing, the production Deepgram
streaming adapter, canonical storage, projection, restart, or replay. It also
reads the legacy YAML path rather than the default OS keychain and can print
provider-returned content. It must be labeled a provider REST diagnostic and
must never be cited as MVP-path proof.

Speech integration tests commonly substitute `None` canonical writers and
in-memory projection state. They prove orchestration pieces, not durability.

The release-blocking offline fixture must use:

- synthetic PCM with source timestamps, a sample-rate transition, and a gap;
- production processing/framing code;
- a scripted local Deepgram WebSocket peer, without network or credentials;
- deterministic fake projection responses;
- real canonical writers under a temporary data root;
- explicit finalize/flush and a fresh repository/state instance;
- transcript, speaker, clock/discontinuity, projection, manifest, replay,
  timeline, and export assertions.

### Rust resolution was not reproducible at discovery HEAD

- `src-tauri/Cargo.lock` is ignored.
- rsac is a sibling path dependency.
- the local ignored lock records rsac 0.4.0 without a source.
- CI and release carry a separate older rsac SHA.
- representative Cargo commands do not use `--locked`.

The approved resolution is the full rsac v0.4.1 Git revision
`7956e6ef24a44672d502e72b0500efb27530e3b9`, an application lockfile that is
present and unignored in the implementation slice but still requires commit,
and locked canonical commands. A sibling checkout becomes an explicit,
untracked development override rather than implicit build input.

### Durability tests do not yet prove crash behavior

Canonical JSONL writers acknowledge queue admission before durable media
commit and primarily flush during graceful shutdown. Existing tests prove
graceful drain and in-process reconstruction, not process-kill boundaries.

The existing `audio-graph-90f3` acceptance remains required: subprocess
kill-points, torn-tail handling, ENOSPC, short write, flush/sync failure,
restart replay, and proof that Pending never becomes user-visible Accepted
state before canonical durability.

## P1 findings

### Generated contracts are not one enforced gate

Four generated checks exist, but `build`, `check`, and `test` do not compose
them. Ordinary workspace Cargo commands also do not necessarily execute both
contract crates. The provider registry and endpoint routing have useful drift
tests; audio-source and session-data generated artifacts need equivalent
committed-file equality enforcement.

A `verify:contracts` task should run all non-mutating drift checks, both Rust
contract-crate suites, frontend contract tests, and locked Cargo metadata.
The provider-registry generator must honor the same `CARGO` override as the
other generators.

### Default Vitest concurrency is not a trustworthy local authority

Vitest 4.1.7 defaults to fork workers and CPU-count-minus-one concurrency. On
this 16-core Windows host that means as many as 15 forks; the suite saw 35
worker-start timeouts even though a one-worker run passed all 908 assertions.
The worker-response timeout is not controlled by the existing hook timeout.

Until bounded parallelism is proven per runner:

- the local authority is serial;
- CI parallelism is explicit and bounded;
- retries cannot establish correctness;
- very large Settings and store test files should be split for diagnosis and
  focused-run speed.

### Contributor documentation describes stale commands and architecture

- `docs/CONTRIBUTING.md` says local gates match CI and says Clippy is not
  gated, while CI does gate Clippy.
- It documents four jobs although the workflow now has twelve and names the
  old rsac input.
- Its raw Windows Cargo command omits
  `AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1`, despite the solved CRT-skew
  runbook and CI use.
- `README.md` still discusses a future rsac 0.2.0.
- `docs/WINDOWS_QUICKSTART.md` says credentials are written to YAML although
  the OS keychain is the default, and it describes a stale capture flow.
- `scripts/run-core-tests.ps1` presents a narrow generated-harness workaround
  as the Windows test path.

`audio-graph-b9c7` owns the product/architecture/documentation correction.

### Frontend tests mock IPC rather than executing the Tauri contract

The global test setup replaces `invoke`, so frontend tests do not prove command
registration, casing, required/optional arguments, Rust deserialization, or
response serialization. Source-grep tests of the hand-maintained handler list
cannot replace an executed contract.

A typed command/argument manifest plus representative `tauri::test`
`InvokeRequest` coverage should include capture start/stop, settings,
credentials, readiness, session review/export/delete, timeline, and projection
status.

### Packaged artifacts compile but are not started under observation

Compile smokes do not launch the artifact. Release upload currently warns when
artifacts are absent, and the standalone script launches detached then declares
success without a readiness observation or controlled exit.

A packaged smoke must isolate the data/config root, launch the exact binary,
wait for a content-free startup-ready marker, observe bounded health, exercise
a self-check or command initialization, terminate the exact child, and record
version, rsac revision, hash, and absence of writes outside the isolated root.

### The tracked npm lock is stale and unsafe

`package-lock.json` describes React 18 and an older dependency set while the
project is Bun/React 19. It is an attractive but incorrect install path. Remove
it, guard the Bun-only workflow, pin the tested Node range, and provide a
toolchain doctor.

## P2 findings

- Rust tests mutate process-global HOME/data-root state behind a global mutex.
  Introduce injectable `DataPaths`/test-workspace ownership so repository tests
  can eventually run in parallel.
- Coverage thresholds trail the measured frontend baseline and Rust coverage
  is not measured. Add a ratchet rather than a one-time target.
- Optional Surreal adapter tests are experimental conformance evidence, not
  canonical MVP proof.
- The docs secret-hygiene check is not composed into canonical gates.
- There is no single task facade for toolchain and feature combinations.
- `src-tauri/tauri.conf.json` references a non-official schema URL.
- CI typechecks twice because build already invokes TypeScript.

## Accepted validation matrix

ADR-0032 records the decision. The target task facade is:

| Tier | Canonical task | Claim |
| --- | --- | --- |
| Fast | `bun run verify:fast` | formatting/lint, types, generated drift; no broad test claim |
| Focused frontend | `bun run test:focused -- <files>` | explicit serial Vitest files |
| Focused Rust | `bun run test:rust -- <filter>` | pinned feature/toolchain/platform wrapper and explicit filter |
| Full offline | `bun run verify:full` | frozen install, contracts, serial frontend, locked Rust, golden replay, docs/Seeds/diff hygiene |
| Live | `bun run verify:live` | strict correlated PCM and production-path provider/device smokes; no silent skip |
| Release | `bun run verify:release` | full gate plus packaged startup, manifest/revision, bundle inspection, and hashes |

The temporary authoritative frontend command is now exposed directly as:

```powershell
bun run test:local
```

It is exactly `vitest run --maxWorkers=1`; default unbounded local parallelism
is still tracked by `audio-graph-e2be` and is not silently retried.

The first claim-bounded façade slice is also implemented:

- `bun run verify:contracts` executes all four Rust-generated TypeScript drift
  checks under the pinned Rust toolchain and lock;
- `bun run verify:fast` composes Biome, TypeScript, generated contracts, Seeds
  JSON stress, docs secret hygiene, and `git diff --check`; and
- `bun run test:focused -- <files>` preserves the reliable one-worker contract
  for an explicit frontend slice.

These tasks make only their named claims. `verify:full`, `verify:live`, and
`verify:release` remain intentionally absent until the golden data path,
executed IPC contract, and packaged observation gates exist.

The temporary focused Windows Rust form is:

```powershell
$env:AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST = "1"
cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml `
  -p audio-graph --lib --no-default-features --features cloud `
  --locked `
  <filter> -- --nocapture --test-threads=1
```

The lockfile is present and no longer ignored in the current implementation
slice, so focused and full Rust commands now require `--locked`. Repository and
release reproducibility still require that file to be committed.

## Implementation checkpoint: 2026-07-09

- `package.json` now exposes the proven `test:local` command. The integrated
  frontend run passed 69 files and 938 tests in 192.87 seconds.
- All four generated-contract launchers now honor `CARGO` and use `cargo.exe`
  on Windows instead of the nonexistent `cargo.cmd`. Audio source, provider
  registry, session data movement, and endpoint credential routing drift checks
  all pass after the repair.
- A final reproducibility review found that the launchers could still select an
  ambient toolchain or update dependency resolution. All four now invoke
  `cargo +1.95.0 run --locked`; the provider-registry launcher was the remaining
  script missing both controls.
- The first complete current-feature Rust binary run executed 1,468 tests:
  1,460 passed and 7 were intentionally ignored, but one test failed because
  HOME isolation did not isolate the OS keychain. The test read a host
  OpenRouter credential and attempted a real request.
- The OpenRouter catalog commands now delegate to helpers accepting an explicit
  credential store. The offline test passes an empty store, so host credentials
  can no longer turn that unit test into provider egress. This is a concrete
  example of why `audio-graph-1b91` must cover credentials as well as file data
  roots.
- The rebuilt locked Rust library binary passes 1,464 tests with 7 explicitly
  ignored and 0 failures across 1,471 tests. The suite first caught and then
  verified the deterministic monotonic projection-id repair; strict Rust 1.95
  Clippy passes with warnings denied.
- The Seeds stdout stress gate initially found the repo-pinned 0.4.5 patch
  missing, then the repair task failed because it also tried to patch an
  unrelated global 0.5.5 install. `ensure-seeds-json-output.mjs` now treats an
  explicit `SEEDS_CLI_ROOT` or the repo-pinned dependency as authoritative and
  consults the global install only as fallback. The repaired local patch passes
  ready/blocked/list JSON stress parsing; `sd doctor` reports 12 passing checks.
- The rsac v0.4.1 Git pin, present unignored lockfile, locked metadata/check, and focused
  projection tests now pass locally. Linux/macOS resolution and approval-gated
  CI/release input deduplication remain open.

The automatic Quick Setup readiness path is also an offline-test boundary:
mount and focus now request `refresh: false` and carry the exact draft provider
ids. Active vendor readiness remains reserved for an explicit, audit-owned
preflight rather than being triggered by rendering a dialog. AWS named-profile
discovery now honors `AWS_CONFIG_FILE` and `AWS_SHARED_CREDENTIALS_FILE` while
remaining a local file existence check.

## Final integrated checkpoint: 2026-07-10

- Biome passes across 171 files and TypeScript passes on the final frontend
  sources.
- A focused Ready/Live now/Review/Inspect, privacy-route, and canonical-empty-
  Review slice passes 91 tests across four files.
- `bun run test:local` passes 962 tests across 70 files in 176.80 seconds.
- The four generated TypeScript contracts are current under the pinned Rust 1.95
  and `--locked` launchers.
- Rust formatting and strict cloud/all-target Clippy pass on the final integrated
  sources.
- The serial Rust cloud library suite executes 1,506 tests: 1,498 pass, zero
  fail, and eight explicit live/HOME/torture gates are ignored.
- That full Rust run first found two command-lifecycle regressions. One was a
  production ordering defect—duplicate Start spawned process-lifetime workers
  before rejection—and now uses early validation repeated at final ownership.
  The other was a stale synthetic Stop fixture that omitted the required
  pipeline/dispatcher Reset acknowledgement. Both focused regressions and the
  rebuilt full suite pass.
- On this Windows host, `CARGO_INCREMENTAL=0`, `CARGO_BUILD_JOBS=1`, and serial
  tests were deterministic. A local `lld-link` override reduced the final test
  link enough to execute the suite; this is a developer workaround, not release
  evidence. Distinct linker/Clippy fingerprints still caused expensive native
  recompilation, reinforcing the need for a checked-in Rust task facade and
  better isolated target/cache strategy.

`verify:fast` passes on the final tree, including all four contracts, Seeds JSON
stress, docs/Seeds secret hygiene with zero findings, and `git diff --check`.
`bun run build` also passes after transforming 2,940 modules in 15m 17s. That
proves the TypeScript/Vite frontend bundle only; it is not packaged Tauri proof.

## Queue consequences

- Raise `audio-graph-e2be` and `audio-graph-f166` to P1 MVP prerequisites.
- Expand `audio-graph-fd9f`, `audio-graph-8913`, `audio-graph-90f3`,
  `audio-graph-be7c`, `audio-graph-34be`, `audio-graph-b718`, and
  `audio-graph-b9c7` with the evidence above.
- Create focused Seeds for the deterministic golden fixture, canonical
  validation facade, executed IPC contract, packaged smoke, hermetic Rust data
  roots, toolchain doctor/stale lock removal, and coverage ratchet.

No workflow change is authorized by this audit. Canonical tasks should first
stabilize locally; CI/release adoption remains a clean-worktree,
approval-gated slice.
