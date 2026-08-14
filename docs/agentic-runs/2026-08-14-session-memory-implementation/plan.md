# Session Memory Implementation Mission Plan

## Done condition

Complete the largest bounded set of decision-complete, active-milestone Seeds
that can be implemented and reviewed safely in isolated worktrees. Each accepted
workstream must have a stable commit, artifact report, focused gates, two-axis
review, integration re-gates, and honest Seeds evidence. Hitting the wave cap
with remaining recorded blockers is an allowed stop.

## Limits

- At most two simultaneous implementation workers in Wave 1.
- At most one integration worker.
- At most one review/fix round per workstream before backflow.
- No edits to the dirty custody checkout.
- No implementation of unresolved Wayfinder decision tickets.

## Classification

| Seed | Class | Rationale |
| --- | --- | --- |
| `audio-graph-fd9f` | `ACTIVE_MILESTONE` | P0 dependency authority; unblocks capture, projection, CI, and release evidence. |
| `audio-graph-e2be` | `ACTIVE_MILESTONE` | Exact local frontend gate currently fails on Node 26 without an undocumented environment override. |
| `audio-graph-9eee` | `ACTIVE_MILESTONE` | Decision-complete strict-reader consumer integration; Wave 2 due to size and shared Rust gate cost. |
| `audio-graph-edc8` | `BLOCKED_DEPENDENCY` | Speaker-aware replay requires the strict snapshot stack from `audio-graph-9eee`. |
| `audio-graph-8e73` | `BLOCKED_DESIGN` | Parent-directory durability and cross-process semantics are not yet decision-complete across three OSes. |
| Wayfinder grilling/prototype tickets | `BLOCKED_DESIGN` | Human or prototype decisions must close before implementation. |
| Realtime voice, provider expansion, Sentry webview | `POST_MILESTONE` | Explicitly outside the first trustworthy Session Memory slice. |

## Wave 1 workstreams

### rsac foundation

- Owner: one implementer worktree.
- Seed: `audio-graph-fd9f`.
- Scope: target-specific Cargo declarations, regenerated lockfile, approved CI
  and release resolution/attestation, dependency documentation, existing capture
  fixture preservation.
- Historical commits are references only; never merge their credential-v2
  ancestry.
- Gates: locked metadata/check/test/Clippy/format, Actionlint, semantic revision
  and feature assertions, diff check.
- Rollback: discard the isolated branch; no shared branch history is rewritten.

### Node 26 frontend test gate

- Owner: one implementer worktree.
- Seed: `audio-graph-e2be`.
- Scope: package test command, minimal cross-platform launcher/config and tests,
  contributing documentation.
- Required red seam: exact `bun run test:local` fails on this Node 26 host
  before the patch.
- Gates: launcher-focused tests, exact full local suite, typecheck, Biome,
  production build, diff check.
- Rollback: discard the isolated branch.

## Wave 2 candidate

### Strict snapshot consumers

- Seed: `audio-graph-9eee`.
- Scope and prerequisite footprint are defined by the read-only discovery
  report: transplant only the accepted canonical-log/reader source stack, then
  integrate one presence-bearing snapshot per Review/replay/timeline/export
  consumer while preserving current-master isolation and canonical-empty
  authority.
- Start only after Wave 1 fan-in and review.

## Review and fan-in

Each workstream receives independent Standards and Spec review against its
merge-base and Seed. Only a dedicated integrator may fan accepted branches into
this integration branch. The integrator validates merge-base footprints,
excludes placeholder or credential-v2 contamination, and re-runs affected gates
on the assembled snapshot.
