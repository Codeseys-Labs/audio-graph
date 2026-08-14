# rsac 0.4.1 and AudioGraph capture audit

Date: 2026-07-09

Source-anchor note: unqualified line references in discovery findings refer to
HEAD `f97e19c`. The implementation record and Cargo-resolved revision are
authoritative for the current working slice.

Status: dependency pin, two-phase acknowledged startup, fatal-exit ownership,
rsac source timestamps, and subscriber-drop visibility implemented;
discontinuity, mid-run consumer supervision, and live three-platform gates
remain

Owning Seeds: `audio-graph-fd9f`, `audio-graph-b5ef`,
`audio-graph-b718`, `audio-graph-99ed`, `audio-graph-9daa`, and
`audio-graph-f166`.

Timing contract:
[ADR-0020](../adr/0020-processed-pcm-contract.md) accepts the source-aware
processed PCM, source/session clock mapping, and discontinuity requirements.

## Decision

Adopt rsac 0.4.1 before the MVP manual gate. Pin the full Git revision
`7956e6ef24a44672d502e72b0500efb27530e3b9`, the commit peeled from the
official annotated `v0.4.1` tag.

The dependency bump alone was not an MVP fix. This wave now waits for the real
rsac `build -> start -> subscribe` boundary before reporting capture success,
uses rsac source-position timestamps with a monotonic fallback, and surfaces
new subscriber-channel loss alongside ring backpressure. Rate-change tail
accounting, mid-capture pipeline/dispatcher supervision, and live platform
proof remain before the complete capture contract is validated; fatal-worker
ownership and aggregate reconciliation are implemented.

Do not enable rsac `compose`, mobile backends, or `macos-tcc-spi` for this
desktop MVP.

## Implementation record (2026-07-10)

- `src-tauri/Cargo.toml` now uses the same target-specific Git dependency at
  full revision `7956e6ef24a44672d502e72b0500efb27530e3b9` on Windows, macOS,
  and Linux, with default features disabled and only the target backend enabled.
- `src-tauri/Cargo.lock` was regenerated from a clean detached worktree and is
  now present and no longer ignored, but it remains unstaged/untracked in this
  working slice. Its rsac entry is version `0.4.1` at the exact revision above;
  the clean-worktree and main-worktree lockfiles had
  identical SHA-256
  `C1248BDE7D41EB60D6F88F727DD796CAF050A2E3D02DB8403932801BF49288E5`.
- `cargo +1.95.0 metadata --locked --no-deps` passed in both worktrees.
- `cargo +1.95.0 check --lib --tests --no-default-features --features cloud
  --locked` passed on Windows, including the 0.4.1
  `PlatformCapabilities::requires_user_consent` fixture migration.
- `AudioCaptureManager::start_capture` now performs a bounded rendezvous with
  the source worker after rsac build, start, and error-aware subscription. A
  failed, disconnected, or timed-out start is stop-signalled and cannot later
  acknowledge into the manager as an untracked active source.
- Capture chunks now prefer `AudioBuffer::timestamp()` and fall back to the
  prior monotonic elapsed clock only when the backend supplies no timestamp.
  New subscriber-drop deltas join the existing ring-loss signal and retain a
  layer-specific diagnostic.
- The hardware-free capture filter passes 28 tests, including six new v0.4.1
  contract tests for acknowledgement, failed/disconnected/timed-out start,
  timestamp preference, and drop-counter rebasing. Windows execution requires
  `AUDIOGRAPH_EMBED_WINDOWS_TEST_MANIFEST=1` for the test binary manifest.
- CI/release workflow deduplication remains approval-gated because this checkout
  contains broad unrelated work. Cross-platform resolution and live capture
  evidence remain open in `audio-graph-fd9f` and `audio-graph-8913`.

### Lifecycle hardening checkpoint (2026-07-10)

- Startup is now two-phase. The worker may build, start, and subscribe rsac,
  but it waits behind a one-shot Commit gate and cannot receive/forward a
  buffer until the first-source `CaptureStarted` audit row is synchronized.
- Pipeline and dispatcher readiness are proved with an ordered Reset barrier,
  rather than inferred from successful thread creation. The same barrier
  discards partial per-source resampler/accumulation state before a restarted
  source or New Session can reuse the source id.
- Startup and Stop timeouts retain their worker handles. A live retired worker
  fences capture restart and session rotation instead of becoming an untracked
  producer.
- Fatal exits reconcile through the shared session-lifecycle lock with exact
  handle identity. A dead last aggregate blocks both same-source and
  different-source restart until cleanup, and one cleanup sweeps sibling dead
  handles before emitting the aggregate Stop boundary.
- Clean application exit calls the capture manager's per-handle Stop path and
  closes the movement ledger before shutting canonical writers down.

The post-checkpoint Rust 1.95 cloud library suite passes 1,498 tests with zero
failures and eight explicitly ignored live/HOME/torture gates across 1,506
tests, serialized with `--test-threads=1`. A local `lld-link` override made the
Windows test-binary link tractable; it is a DevEx workaround, not release or
packaging evidence. Rust 1.95 `fmt --check` and strict cloud/all-target Clippy
also pass on the final integrated Rust sources. Live permission, unplug,
source-loss, and three-OS packaged
evidence remain required; mid-capture pipeline/dispatcher panic or stall also
lacks an active supervisor.

## Primary sources

- [v0.4.1 release](https://github.com/Codeseys-Labs/rust-crossplat-audio-capture/releases/tag/v0.4.1)
- [release commit](https://github.com/Codeseys-Labs/rust-crossplat-audio-capture/commit/7956e6ef24a44672d502e72b0500efb27530e3b9)
- [v0.4.0 to v0.4.1 comparison](https://github.com/Codeseys-Labs/rust-crossplat-audio-capture/compare/v0.4.0...v0.4.1)
- [v0.4.1 manifest](https://github.com/Codeseys-Labs/rust-crossplat-audio-capture/blob/v0.4.1/Cargo.toml)
- [consumer guide](https://github.com/Codeseys-Labs/rust-crossplat-audio-capture/blob/v0.4.1/docs/CONSUMING_RSAC.md)

The official crates.io API returned no `rsac` crate on 2026-07-09, so a
published version pin is not currently available.

## Dependency drift before this wave

Before this wave, AudioGraph had no single authoritative rsac revision:

1. `src-tauri/Cargo.toml` used target-specific sibling path dependencies.
2. `.github/workflows/ci.yml` separately pinned the older 0.4.0-era revision
   `a2d3088...`.
3. `.github/workflows/release.yml` accepted another revision input and cloned
   rsac independently.
4. `src-tauri/Cargo.lock` contained path-style 0.4.0 metadata, while the
   repository ignored that lockfile.
5. A developer's sibling checkout could therefore build different code from CI
   and release.

The manifest and lockfile portion of that target is now implemented and proven
in a clean worktree. The duplicate CI and release revision inputs still need an
approval-gated workflow change, and project validation commands still need a
single `--locked` facade so local, CI, and release claims cannot drift.

## Relevant 0.4.1 changes

### Source time and discontinuities

Every desktop backend now populates `AudioBuffer::timestamp()` with source
stream position. Producer loss appears as a timestamp gap, and rate
renegotiation does not retroactively rescale earlier audio.

AudioGraph now carries rsac's source position into each raw `AudioChunk`, with
`start_time.elapsed()` retained only as a compatibility fallback. The remaining
gap is explicit discontinuity propagation through resampling and downstream
session provenance; timestamp gaps are observable but are not yet first-class
canonical events.

### Bounded subscriptions and drop accounting

rsac subscriptions are bounded to 128 buffers, drop instead of blocking, and
expose `subscriber_dropped_count()`. Recoverable errors are coalesced and the
fatal terminal remains the final subscription item.

AudioGraph now polls both rsac ring backpressure and the cumulative subscriber
drop counter, logging subscriber deltas and folding them into the public
capture-loss transition. Raw-channel timeout loss and all-layer canonical
provenance remain incomplete. The MVP still needs separate content-free
counters for:

- rsac producer or ring loss
- rsac subscriber loss
- AudioGraph raw and processing queue loss
- downstream consumer loss

### Stop and backend robustness

The release strengthens drain and terminal behavior, contains WASAPI panics,
uses realtime-safe PipeWire diagnostics, and fixes a reported PipeWire stop
wedge. AudioGraph now has per-source bounded stop/join, retained timed-out
ownership, aggregate fatal reconciliation, and clean-exit capture closure.
Coordinated provider drain/finalization, mid-run spine supervision, and live
permission/source-loss tests remain open.

### macOS permission semantics

Process Tap gains an all-zero diagnostic. Honest public preflight remains
`NotDetermined`; stronger preflight is behind private `macos-tcc-spi` and
should remain disabled.

AudioGraph projects system-default and device capture as `NotRequired` too
broadly. Ordinary device capture must remain distinct from system,
application, and process-tree Process Tap readiness. The UI needs actionable
no-signal guidance without claiming success.

### Capability and target contracts

`PlatformCapabilities` adds `requires_user_consent`. The known AudioGraph
compile migration is a test literal missing that field. It models mobile
configuration artifacts and is false for desktop backends; it must not be
mistaken for desktop runtime permission.

Upstream now states explicitly that `ApplicationId` is a numeric PID,
`ApplicationByName` is a process name, and `ProcessTree` is distinct.
AudioGraph's canonical target mapping agrees with that contract.

## Current AudioGraph capture spine

The runtime structure is suitable for the product:

1. Source discovery maps rsac kinds into typed descriptors and canonical
   targets.
2. Start parses selected targets and launches one worker per source.
3. Each worker negotiates format, builds and starts rsac, and subscribes with
   error events.
4. Interleaved `f32` buffers become source-scoped raw chunks.
5. `AudioPipeline` independently downmixes and resamples each source to
   16 kHz mono in roughly 32 ms chunks.
6. A dispatcher fans processed chunks into bounded consumer queues.
7. Speech consumers preserve per-source routing and a declared drop policy.
8. Batch ASR builds approximately two-second segments; streaming providers
   consume processed chunks directly.

Keep this backend-owned per-source spine. rsac `compose` would merge sources
before AudioGraph can preserve source identity and speaker provenance.

## MVP defects

### Capture startup acknowledgement — implemented boundary

The manager now returns success only after its worker has built and started rsac
and created the error-aware subscription. Build/start/subscribe errors propagate
to the command, and timeout/disconnect cannot publish a source handle.

Implemented contract:

- bounded startup acknowledgement per selected source
- rsac Ready only after build/start/subscribe; audio publication only after
  writer readiness, pipeline/dispatcher reset acknowledgement, synchronized
  first-source `CaptureStarted`, and Commit
- failed startup is stop-signalled and cannot later publish an untracked source

This proves the capture-source boundary, not whole-session Running. ASR/provider
startup is still a separate command path until ADR-0028's coordinated Start
and reverse-order rollback land.

Remaining lifecycle contract:

- explicit multi-source partial-success policy or complete rollback
- transcription tied to an acknowledged capture generation at every provider
  boundary
- mid-capture pipeline/dispatcher heartbeat and fatal reconciliation
- fault-injected permission, disappearing-device, consumer-death, and rollback
  coverage in addition to the current hardware-free ownership tests

### Source loss needs canonical discontinuities

rsac source timestamps are now the capture timeline basis, with a monotonic
fallback for backends that omit them. Preserve source-local gaps through
resampling and materialize them as discontinuities rather than shifting later
speech earlier. Capture health is an input to notes, the temporal graph, speaker
joins, and provenance, not merely observability.

### Rate changes discard pending audio

When a source rate changes, AudioGraph replaces its resampler and clears pending
input. Flush the old resampler or explicitly record the discarded tail as a
discontinuity before accepting the new rate. Keep multiple sources independently
clocked.

## Dependency shape

```toml
rsac = {
  git = "https://github.com/Codeseys-Labs/rust-crossplat-audio-capture.git",
  rev = "7956e6ef24a44672d502e72b0500efb27530e3b9",
  default-features = false,
  features = ["feat_windows"] # corresponding desktop feature per target
}
```

Sibling development should use an explicit local Cargo `[patch]` override,
never the repository default. Move to an exact published version only after
crates.io publication is independently confirmed.

## Verification gate

Hardware-free:

- target parse and round trip for system, device, PID, name, and tree
- failed startup never reaches Running
- success acknowledges only after subscription is live
- fatal exit reconciles capture and transcription
- timestamps remain monotonic through resampling
- induced loss produces explicit gaps and layer-specific counters
- rate transitions preserve or account for pending tails

Live:

- Linux known-tone default-monitor capture and stop under three seconds
- separate Linux minimum-PipeWire test because 0.4.1 enables the
  `v0_3_65` API gate while documenting an older baseline
- macOS virtual-device PCM test and structured Process Tap TCC/no-signal result
- Windows live virtual-endpoint test with distinct PID and process-tree cases
- every leg logs only descriptor kind, a redacted stable source id,
  capabilities, permission outcome, negotiated format, source timestamps, and
  all four drop layers. Raw process names, PIDs, window titles, device labels,
  and target paths never enter logs or analytics.

CI and release:

- Linux, macOS, and Windows resolve the same Cargo-locked SHA
- build with `--locked`
- release dry run reports the resolved revision

## Sequence

1. Pin 0.4.1 and regenerate the lock in a clean worktree so every later Rust
   result targets the same revision. CI/release input cleanup remains a
   separately approval-gated follow-up.
2. Implement startup acknowledgement and fatal-exit reconciliation.
3. Preserve source-position time, discontinuities, and layered drop telemetry.
4. Make rate changes loss-aware.
5. Correct macOS Process Tap readiness.
6. Run hardware-free and three-OS gates.
7. Only then call the dependency upgrade MVP-ready.
