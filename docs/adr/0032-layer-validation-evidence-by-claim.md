---
status: accepted
date: 2026-07-09
deciders: [AudioGraph maintainers]
---

# ADR-0032: Layer Validation Evidence by Claim

## Context and Problem Statement

AudioGraph has strong unit, fixture, frontend, and replay coverage, but no one
test proves the complete MVP path from timed PCM through durable restart and
export. Existing live-device and cloud-provider scripts can pass after a
best-effort capture failure or by bypassing AudioGraph's production transport,
storage, and projection paths. Contributors also face divergent commands,
unbounded local test concurrency, unlocked Rust resolution, and documentation
that overstates what individual checks prove.

The project needs a validation architecture in which every command has a
bounded claim, deterministic offline evidence is release-blocking, and live or
packaged checks complement rather than substitute for that evidence.

## Decision Drivers

- Prove the complete durable MVP path without credentials, network, or hardware.
- Make command names and documentation match the assertions they execute.
- Keep fast feedback fast without presenting it as full correctness evidence.
- Separate deterministic regressions from platform/provider availability.
- Require reproducible toolchains, generated contracts, and locked dependencies.
- Make failures diagnosable without retry-based masking.
- Reuse the same task facade locally and in approved CI/release workflows.

## Considered Options

- Continue with ad hoc commands and prose release checklists
- Build one monolithic end-to-end validation command
- Adopt layered fast, focused, full-offline, live, and release evidence

## Decision Outcome

Chosen option: "Adopt layered fast, focused, full-offline, live, and release
evidence", because different feedback loops have different cost and
environment requirements, but each can still make a precise, enforceable
claim.

AudioGraph defines five validation tiers:

1. **Fast** runs formatting/lint, TypeScript, and non-mutating generated-contract
   drift checks. It does not claim broad behavioral correctness.
2. **Focused** runs explicit frontend files serially or an explicit Rust filter
   through platform/toolchain wrappers. Its report names the selected scope.
3. **Full offline** runs frozen dependency checks, all generated contracts,
   serial frontend tests and build, locked Rust format/lint/tests, documentation
   and Seed hygiene, and a deterministic golden MVP fixture from timed PCM
   through canonical durable restart/replay/export.
4. **Live** proves strict device and provider behavior on supported platforms.
   Required legs fail on zero PCM, missing timestamps, silent skip, or an
   unclassified permission/capability outcome.
5. **Release** includes Full offline, launches the exact packaged artifact in
   an isolated data root, verifies startup/readiness and controlled exit, and
   records version, Cargo-resolved rsac revision, bundle contents, and hashes.

A command may claim only what it asserts. Live hardware/provider evidence
cannot replace the deterministic golden fixture. A skipped required leg is a
failure. Retries may diagnose nondeterminism but cannot establish correctness.
Generated-file equality and locked dependency resolution are part of the Full
offline contract.

The canonical task facade will expose `verify:fast`, `test:focused`,
`test:rust`, `verify:contracts`, `verify:full`, `verify:live`, and
`verify:release`. CI and release workflows may call the same tasks only after
the tasks are stable and the workflow slice receives separate approval.

### Consequences

- **Positive**: Release evidence covers the real product path rather than a set
  of adjacent components.
- **Positive**: Fast and focused feedback remains available with honest scope.
- **Positive**: Live-device failures are separated from deterministic logic
  regressions.
- **Positive**: Local, CI, and release instructions converge on one facade.
- **Negative**: The golden fixture, scripted peers, and packaged harness add
  maintained test infrastructure.
- **Negative**: Full and release validation are intentionally slower than the
  current ad hoc commands.
- **Negative**: Live and packaged checks require ongoing three-platform upkeep.
- **Negative**: Some production paths must gain injectable clocks, data roots,
  transports, and process lifecycle seams.
- **Neutral**: This decision refines the validation-command guidance associated
  with ADR-0007; it does not change ADR-0007's local-ML feature-gating decision.

## Pros and Cons of the Options

### Continue with ad hoc commands and prose release checklists

- Good, because it requires little immediate harness work.
- Good, because contributors can choose any narrow command.
- Bad, because a passing script can overstate product correctness.
- Bad, because local, CI, and release behavior continues to drift.
- Bad, because manual checklists do not provide deterministic regression proof.

### Build one monolithic end-to-end validation command

- Good, because there is one apparent pass/fail result.
- Good, because the full route can be exercised together.
- Bad, because hardware, credentials, packaging, and deterministic logic become
  coupled in one slow and flaky environment.
- Bad, because failures are difficult to localize.
- Bad, because contributors lose a trustworthy fast feedback loop.

### Adopt layered fast, focused, full-offline, live, and release evidence

- Good, because each tier has a precise claim and environment contract.
- Good, because deterministic offline proof is always available and blocking.
- Good, because live and packaged behavior still receive explicit evidence.
- Bad, because task composition and fixtures require ongoing maintenance.
- Bad, because reviewers must understand which tier is required for a change.

## More Information

The evidence and proposed command matrix are recorded in
`docs/research/mvp-validation-devex-audit-2026-07-09.md`.

The deterministic fixture is subordinate to ADR-0020 for source clocks and
loss, ADR-0027 for canonical Accepted durability, ADR-0028 for lifecycle
ownership, and ADR-0031 for projection basis currency. Seeds created from this
audit track the fixture, task facade, executed IPC contract, packaged smoke,
hermetic test roots, toolchain doctor, and coverage ratchet.
