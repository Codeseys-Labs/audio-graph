# audio-graph-4249 implementation plan

Date: 2026-08-14

Accepted decisions: ADR-0041 and ADR-0042

Planning base: `142492261ecca4539d457bed0c3578869e75dd1f`

## Outcome and done boundary

`audio-graph-4249` is complete only when AudioGraph can durably advance one
Session semantics floor, replay one canonical legacy/v2 speech stream with
first-Accepted ordering, create and validate versioned Projection Bases, feed
the same normalized semantics to notes and graph prompts, load/export/recover
without fabricating evidence, and pass the predecessor refusal canary.

Adapter activation under `audio-graph-48de` remains blocked until that full
boundary is integrated. A green hash encoder alone does not close `4249`.

## Load-bearing discovery

- The current canonical appender file-syncs accepted frames, but explicitly
  does not prove parent-directory durability for a newly created stream or
  durably register quarantine artifacts. Existing Seeds `5e41` then `8e73`
  own those prerequisites.
- Production transcript and projection writers currently publish live state
  after queue admission, not after a canonical `Accepted` receipt.
- Strict compatible legacy/v2 decoding exists only as a reader-first seam.
- Canonical records retain sequence, but current consumers discard it before
  replay. The legacy ledger then sorts latest spans by mandatory scalar time.
- Current prompts serialize fields excluded from hash v2 and silently replace
  serialization failure with an empty transcript.
- Review, export, timeline, and recovery still require legacy scalar timing
  and confidence.

## Immediate Wave 7A

Two tracks run in parallel with exclusive ownership.

### Track A — `audio-graph-ab64`

Build the normalized projection-semantic view and exact hash-v2 conformance
kernel. Ownership is limited to Speech Span Revision access/normalization, a
new hash module, golden fixtures, and an independent Bun verifier.

The track must not edit or activate `TranscriptLedger`, `ProjectionBasis`,
prompts, persistence, writers, schedulers, or provider adapters.

Public TDD seams:

- validated read-only access to every projection-semantic v2 field;
- normalized legacy/v2 semantic records;
- positioned inputs carrying first canonical sequence for ordering only; and
- a fallible content-free hash-v2 encoder.

Required evidence includes all active design goldens, included/excluded field
mutation tests, negative-zero normalization, non-finite refusal, exact
supersession, missing/duplicate/reversed order failures, an independent Bun
encoder, and unchanged hash-v1 goldens.

### Track B — `audio-graph-5e41`

Build a non-production executable model for projection admission and Session
job fencing. It must exhaustively cover Pending, Accepted, AlreadyAccepted,
Rejected, and OutcomeUncertain; idempotent retry; session epoch replacement;
restart; detached completion; and deletion fencing.

Only Accepted or AlreadyAccepted may advance materialized state or become
basis-eligible. The prototype must not introduce a runtime
`SessionSemanticsVersion`, writer, or durability claim.

## Serial dependency order

1. `audio-graph-5e41` — accepted admission/fencing state model.
2. `audio-graph-8e73` — cross-platform directory-entry, quarantine-manifest,
   locked recovery, and subprocess durability.
3. `audio-graph-7e81` — canonical Session provenance and monotonic semantics
   floor; no speech writer activation.
4. `audio-graph-0baf` — Accepted-sequence-aware unified ledger and versioned
   Projection Basis.
5. `audio-graph-4c82` — prompt/hash order parity plus scheduler, currency, and
   patch validation dispatch.
6. `audio-graph-6b9d` — receipt-bearing mixed canonical transcript commit
   boundary with the production v2 caller unreachable.
7. `audio-graph-e969` — checked mixed Review/load/export/timeline/recovery and
   deletion without evidence fabrication.
8. `audio-graph-ddb3` — exact predecessor v1-open and v2-floor-refuse canary.
9. Close `4249`, then resume `audio-graph-48de` adapter activation.
10. Extend `audio-graph-2add` with the final fresh-process mixed v1/v2 golden.

## Guardrails

- The v2 Session floor is durably Accepted before any v2 transcript, basis,
  or patch can be created or applied.
- Guard-ahead is safe and idempotent. Artifact-ahead is typed corruption and
  never repairs or promotes the floor.
- Missing historical floors mean v1. Historical hash-v1 bases and patches
  validate with their frozen bytes forever.
- Unavailable evidence remains unavailable. No zero timing, default
  confidence, synthetic turn, or provider-specific downstream branch is
  permitted.
- The same normalized ordered semantics feed hash v2 and projection prompts.
- Production v2 writing remains unreachable until mixed consumers and the
  predecessor canary pass.
- Workflow, release, provider-selectability, credential, and deployment
  changes remain out of scope.

## Verification strategy

Each implementation branch captures RED then GREEN at a public seam, commits
an artifact report, and receives independent Standards and Spec review on a
stable tip. The integrator alone merges accepted tips and reruns focused gates,
locked cloud check/tests, strict Clippy, rustfmt, generated-contract drift,
frontend gates when touched, Seeds checks, secret hygiene, and range diff.

Cross-platform durability and predecessor claims require executed Windows,
macOS, and Linux evidence. Linux-only conformance may land dormant reader-first
code but cannot activate a v2 floor or writer.

## Human policy decisions still required

- Whether user-facing `Saved` is reserved for durable Accepted state.
- Whether restart may automatically reissue a remote projection request when
  duplicate cost or content egress is possible.
- Whether deletion always discards in-flight remote results or offers a
  wait-to-finish mode.
- How timing-unavailable spans appear in the ordered timeline.
- Which exact build is the minimum supported predecessor for the canary.

The `5e41` prototype should make the first three decisions concrete. Later
consumer and canary work must not guess the final two.

## Rollback

Wave 7A is dormant and reversible: discard the hash kernel or prototype
without changing runtime state. Reader-first later work remains reversible
until a Session durably accepts floor v2. After that transition, rollback must
remain v2-capable or restore a complete pre-transition Session; the floor is
never lowered.
