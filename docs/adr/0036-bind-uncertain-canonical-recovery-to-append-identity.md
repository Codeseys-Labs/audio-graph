---
status: accepted
date: 2026-07-10
deciders: [AudioGraph maintainers]
---

# ADR-0036: Bind Uncertain Canonical Recovery to the Expected Append Identity

## Context and Problem Statement

The canonical-log v1 appender poisons itself when a write, flush, or file-sync
outcome is uncertain. The prototype originally reconciled an identical retry
primarily by comparing the observed file length with the byte length cached
before the append. Length does not prove that the retained prefix is the same
stream head or that the unterminated suffix was produced by the pending frame.

This admits two unsafe recovery decisions. A same-length replacement can cause
the appender to retry a frame whose `previous_hash` belongs to the old base, and
an unrelated unterminated suffix can be quarantined as though it were the
appender's partial write. A valid legacy final row without a newline also needs
a separator before the framed suffix, so length-only tail offsets can strand a
legitimate short-write recovery.

## Decision Drivers

- Never mutate bytes that are not proven to belong to the uncertain append.
- Preserve exact-once idempotency for an identical retry.
- Detect same-length stream replacement before returning `Accepted`.
- Recover legacy-prefix separator short writes deterministically.
- Keep uncertainty recovery exceptional and fail closed.

## Considered Options

- Continue recovery from byte length alone.
- Compare the expected base head and require an exact pending-frame suffix.
- Disable in-process uncertainty repair and require every caller to reopen.
- Journal every append as a separate multi-file transaction.

## Decision Outcome

Chosen option: "Compare the expected base head and require an exact
pending-frame suffix", because it binds a recovery mutation to the semantic
stream state and the immutable bytes of the attempted event while preserving a
bounded single-file recovery path.

Each pending append records its original byte length, semantic stream head,
newline state, immutable frame bytes, event commitment, and target sequence.
Recovery reparses the original prefix at the recorded byte boundary and must
match the expected head and newline state. Any observed suffix must be an exact
prefix of the immutable pending frame. A mismatch returns content-redacted
`RecoveryRequired` and leaves the source byte-for-byte unchanged. Every
successful recovery is followed by a strict full-stream reopen assertion in
the fault test matrix.

A complete pending event is accepted semantically only when its event ID,
sequence, record hash, commitment, and position as the current stream head all
match. It then crosses a fresh flush and file-sync barrier before
`AlreadyAccepted` is returned. A zero-byte uncertain write may retry from the
exact captured base without mutation. A legacy base that lacked a final
newline treats the inserted separator as the first byte of the immutable
pending frame. In `Strict` mode, partial and separator-only suffixes are never
quarantined or truncated; only the zero-byte retry and complete-event
reconciliation rules remain available.

The appender remains poisoned after every uncertain, conflicting, malformed,
or failed-repair outcome, rejects a different event, and permits only an
identical commitment retry. Poison clears only after the identical event
returns `Accepted` following a successful retry or `AlreadyAccepted` following
the fresh recovery barrier.

This decision does not claim path or filesystem identity, make ignored OS locks
safe, or replace the directory/manifest transaction required by ADR-0027.
Those remain separate runtime-adoption gates.

### Consequences

- **Positive**: Same-length replacement and foreign suffixes cannot be silently
  truncated or accepted as the pending event.
- **Positive**: The legacy-no-newline separator case has one deterministic
  exact-once recovery rule.
- **Positive**: Recovery failure remains explicit and content-redacted.
- **Negative**: Exceptional recovery reparses the retained prefix and compares
  additional state before retrying.
- **Negative**: A mismatch leaves the appender poisoned and requires operator or
  startup reconciliation rather than automatic progress.
- **Negative**: File identity, reader locking, directory durability, and
  manifest-owned quarantine still require additional implementation and tests.
- **Neutral**: The normal successful append path remains one write, flush, and
  file synchronization before its bounded receipt.

## Pros and Cons of the Options

### Continue recovery from byte length alone

- Good, because it stores and compares minimal state.
- Good, because the happy-path retry is simple.
- Bad, because equal lengths do not prove equal stream heads.
- Bad, because unrelated suffix bytes can be destroyed during repair.

### Compare the expected base head and require an exact pending-frame suffix

- Good, because the decision is tied to both prior semantic state and attempted
  bytes.
- Good, because it preserves exact-once retry without a second transaction log.
- Bad, because recovery performs a full validation scan of the base.
- Bad, because it cannot by itself prove that a pathname still names the same
  file object.

### Disable in-process uncertainty repair and require every caller to reopen

- Good, because the live appender never performs a destructive recovery step.
- Good, because startup could centralize all reconciliation.
- Bad, because every transient uncertain write forces stream teardown and
  session-level recovery.
- Bad, because reopen still needs the same base/suffix proof to distinguish an
  accepted event from a torn or foreign tail.

### Journal every append as a separate multi-file transaction

- Good, because a durable intent record could support richer crash recovery.
- Good, because recovery decisions would have an independent append identity.
- Bad, because every event would add another file/directory transaction to the
  latency-sensitive path.
- Bad, because the manifest and journal would become another authority that
  needs atomic lifecycle, migration, and deletion semantics.

## More Information

- Governing storage decision: ADR-0027.
- Implementation Seed: `audio-graph-b481`, child of `audio-graph-90f3`.
- Directory, one-handle repair, and crash-matrix follow-up:
  `audio-graph-8e73`.
