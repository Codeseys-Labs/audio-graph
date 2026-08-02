# Commit state: audio-graph-79e7 build throughput

Date: 2026-08-02

## Starting point

- Seed: `audio-graph-79e7` — standardize reusable Cargo target lanes and
  coordinated local build jobs.
- Accepted base: `8be073fecd8db650548c3d28734ebdebae26e379`.
- Branch: `work/audio-graph-79e7-build-throughput`.
- Worktree: `.worktrees/79e7-build-throughput` (clean at handoff).
- Tracker state: the Seed is `in_progress`; this worktree must not mutate or
  sync the shared Seeds ledger.

## Bounded ownership

This workstream owns only a new Bun-facing Cargo lane facade and its tests,
`package.json` task exposure, build-lane guidance in `AGENTS.md` and
`docs/CONTRIBUTING.md`, and this commit-state note. It does not own credentials,
Cargo manifests or lockfiles, CI workflows, release behavior, cache cleanup, or
any unrelated checkout state.

## Acceptance contract

- Reuse a stable target identity that includes the worktree and explicit
  feature/profile lane.
- Run ordinary cloud-only checks/tests with Cargo `+1.95.0`, `--locked`, and a
  coordinated host-wide CPU budget. For simultaneously admitted default shared
  builds, the six-token operating points are one build at six jobs, two builds
  at three each, and three builds at two each.
- Never configure a budget above the CPUs detected on the host.
- Make default-feature/full gates exclusive with ordinary token-mode builds.
- Offer exactly one explicit fresh `mktemp` clean-room mode, report its target
  path, and never delete it automatically.
- Prevent Cargo from executing until its detached POSIX group is durable in the
  acquired leases. Never release or reclaim a lease while that group is known
  alive, including when cleanup cannot prove termination.
- Refuse coordinated Windows execution until descendant ownership and cleanup
  have both an auditable implementation and target-native evidence.
- Recover safely from interruption and dead stale leases; keep ordinary logs
  free of worktree paths and user/source content.
- Support explicit environment overrides without routing arguments through a
  shell.

## Review correction

The correction was driven by two formal BLOCK artifacts:

- specification review SHA-256
  `4285c06bdbc8e772e0d201a865a74f8a49c0b4c6be45348219a3078a2d7c57cb`;
- standards review SHA-256
  `5c59395d30951778f6eab2bf8bf57f48b2110772e2940eaf71edb7ad7dce148c`.

The corrected facade now:

- retains token owners when POSIX descendant cleanup remains uncertain, and
  permits stale recovery only after the recorded group is absent;
- launches a detached wrapper that waits on an IPC registration barrier, so a
  hard facade crash before the durable owner update cannot start Cargo;
- batches default shared arrivals for 100 ms and assigns an immutable share of
  the host budget before any Cargo process starts;
- rejects Windows before target creation, coordination, or Cargo spawn with
  `windows_descendant_ownership_unavailable`; and
- preserves fixed `AUDIO_GRAPH_CARGO_JOBS` overrides for deliberate runs while
  keeping default-feature and clean-room gates exclusive.

## Evidence boundary

The adoption review supplied two external timing observations: a cold build of
4m21 versus a warm RED rebuild of 12s, and a no-change service suite of 0.51s.
They motivate stable reuse but are not measurements of this facade.

Deterministic temporary fixtures exercise the public Bun CLI seam with a fake
Cargo executable. They prove stable worktree/feature target identity, exact
Cargo arguments, 6/3/2 admission, exclusive gates, interruption cleanup,
uncertain-cleanup retention and later reclaim, the pre-execution registration
barrier, content-free facade errors, and the fail-closed Windows policy.
The 6/3/2 cases use a guarded test-only detected-CPU seam, so they execute on
low-CPU test hosts instead of silently returning early; production CPU detection
is unchanged.

Real Cargo no-op timing, a controlled one-file rebuild, and compiled-artifact
disk measurements were not run in this correction worktree. A one-file rebuild
would require touching Rust source outside this workstream's ownership, and a
fresh or repeated compilation could consume substantial time and disk while
mutating the shared reusable targets. Follow-up evidence should be gathered on
a clean POSIX integration checkout by timing the same `bun run rust:check:cloud`
lane twice, applying one authorized Rust edit and timing it once, and recording
`du -sk` for that exact stable lane before and after. It must not delete or move
any target. Windows evidence remains blocked until the ownership strategy above
exists.

## Final verification

- `bun run test:cargo-lane`: 24 passed, 0 failed, 107 assertions.
- Focused Biome stdin checks for both owned scripts: passed.
- `bun run check`: 173 files checked, no fixes required.
- `bun run typecheck`: passed (`tsc --noEmit`).
- `git diff --check` against accepted base: passed.
- Forbidden-scope diff check against accepted base: empty; the candidate touches
  only the six assigned paths.

No Rust compile gate is assigned because this slice changes no Rust source or
Cargo metadata. Cargo argv, target environment, admission, and process behavior
are covered through the deterministic fake-Cargo seam; real performance and disk
evidence is explicitly deferred above.
