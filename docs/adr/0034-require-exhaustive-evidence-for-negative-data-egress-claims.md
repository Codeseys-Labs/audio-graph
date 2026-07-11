---
status: accepted
date: 2026-07-10
deciders: [AudioGraph maintainers]
---

# ADR-0034: Require Exhaustive Evidence for Negative Data-Egress Claims

## Context and Problem Statement

AudioGraph exposes a session data-route report from a redacted movement ledger.
The current production ledger records capture lifecycle boundaries and some
projection-provider activity, but it does not yet cover every ASR, LLM, TTS,
realtime, credential, artifact, export, delete, and promotion path. A valid
`CaptureStarted` / `CaptureStopped` pair proves that the capture lifecycle
closed; it does not prove that an uninstrumented producer sent nothing.

Positive evidence and negative evidence have different logic. One valid
off-device content row proves egress. The absence of such a row proves nothing
unless the producer set and session scope are known to be exhaustive.

## Decision Drivers

- Never turn missing telemetry into a privacy guarantee.
- Preserve immediate visibility of observed provider or export egress.
- Keep completeness authority in the backend that owns runtime producers.
- Make future coverage upgrades explicit, versioned, and regression-testable.
- Keep the movement ledger redacted and free of raw content or secrets.

## Considered Options

- Treat a closed capture lifecycle with no egress rows as local-only proof
- Always show Unknown, including when positive egress rows exist
- Report positive egress from partial evidence and require an explicit backend
  coverage marker before any negative claim

## Decision Outcome

Chosen option: "Report positive egress from partial evidence and require an
explicit backend coverage marker before any negative claim."

The UI may report that content left the device as soon as a valid redacted row
records content crossing a provider, organization, or export boundary. It may
report that no content left the device only when all of the following hold:

1. the backend emits a named, versioned exhaustive-runtime-coverage marker;
2. that marker covers every content-bearing producer enabled in the build;
3. the events share a valid session and schema scope;
4. the capture lifecycle is closed when capture occurred; and
5. no valid content-egress row exists in that complete scope.

Frontend code must not infer or manufacture the coverage marker. Until the
backend contract exists, overall movement completeness is false and every
negative egress claim renders Unknown, including sessions with a closed local
capture. Capture-lifecycle validity remains separately testable and useful for
diagnostics.

### Consequences

- **Positive**: Partial telemetry cannot produce a false green privacy claim.
- **Positive**: Known egress remains visible without waiting for full coverage.
- **Positive**: Enabling a provider cannot silently widen the proof boundary;
  its producers must join a versioned coverage matrix first.
- **Negative**: Local-only sessions remain Unknown until instrumentation is
  exhaustive, which is more conservative than the intended final experience.
- **Negative**: The backend must maintain a versioned producer inventory and
  tests for success, failure, blocked, and shutdown paths.
- **Neutral**: This decision does not change the redacted event payload schema
  or authorize raw content in the audit ledger.

## Pros and Cons of the Options

### Treat a closed capture lifecycle as local-only proof

- Good, because the UI can show a reassuring answer immediately.
- Bad, because capture closure says nothing about uninstrumented consumers.
- Bad, because adding a provider can invalidate old frontend assumptions.

### Always show Unknown

- Good, because it cannot overstate privacy.
- Bad, because it hides observed, actionable egress evidence.
- Bad, because it makes the audit view less useful during rollout.

### Positive evidence now; negative evidence after backend coverage proof

- Good, because positive evidence is monotonic and immediately useful.
- Good, because the component that owns producers owns completeness.
- Good, because coverage becomes a named compatibility contract.
- Bad, because users do not receive a green local-only result yet.

## More Information

Tracked by `audio-graph-70a3` and `audio-graph-51e0`. See the data-route section
of `docs/research/mvp-storage-audit-2026-07-09.md` and ADR-0032 for the evidence
levels required before a coverage version can be promoted.
