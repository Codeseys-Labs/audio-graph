---
status: accepted
date: 2026-11-27
deciders: [AudioGraph maintainers]
---

# ADR-0052: Serialize Credential v2 Operations on One Backend Worker

## Context and Problem Statement

Credential storage and provider use belong to Rust, but native store calls can
block longer than a UI deadline and cannot reliably be cancelled. A replacement
that times out may still commit. Releasing admission at the deadline would let
another operation overtake the first; querying the store for passive status
could block or prompt. A panic caught after the global hook runs can still leak
private data unless the worker marks the redaction boundary before dispatch.

[ADR-0019](0019-credential-and-config-storage.md) established the settings and
credential split. This record narrows its credential runtime, fallback, and
migration decisions for v2 only. The proposed `ConfigCodec` is unchanged.

## Decision Drivers

- Passive status never touches disk, the native store, or a prompt path.
- A caller deadline does not weaken revision checks or change write ordering.
- Unknown commits, store failures, and missing credentials are distinct.
- Saved secret values, private locators, and native diagnostics never cross the
  UI or diagnostic boundary.
- The backend can land and be verified independently of Tauri composition.

## Considered Options

- **Execute directly on the async executor.** A blocking store can occupy an
  executor thread; a dropped future does not make the native call cancellable.
- **Use a blocking-pool job per request.** This still needs separate admission,
  cache reconciliation, and event ordering; a post-timeout job can outlive its
  caller and must not race a successor.
- **Use a dedicated worker and a cached status projection.** One owner keeps
  store access, cache publication, and event delivery in the same order.

## Decision Outcome

Use one backend-owned `CredentialRuntime`. Dormant construction performs no
I/O and starts no thread. Configuration installs the backend and sink and
starts one named `std::thread` with a bounded request channel; it does not open
an authority. The worker alone runs the synchronous service and its store
operations. Production operation IDs and revisions are opaque UUID v4 tokens,
not counters or fingerprints.

Expose `dormant`, `opening`, `uninitialized`, `ready`, `recovery_required`,
`unavailable`, and `stalled` from a cached, content-free projection. Status
never enqueues work. Ordinary operations must not prompt. Explicit diagnosis
or unlock is the only interactive path; successful recovery is followed by a
noninteractive reopen. A recovery-required or unavailable authority does not
fall back to v1 or to a plaintext store. One explicit, noninteractive repair
request takes ownership of refused authority state where the platform allows
it and removes native records the authority journal does not track; it
reopens only after it changed an owner, and no ordinary operation runs any
part of it. It changes only objects whose ownership refusal this process
recorded, while their paths still reach the same objects. Every reopen,
whichever route starts it, keeps what the runtime adopted: a service bound
to another authority, or over a journal older than the highest epoch the
runtime observed, is refused as recovery-required. The floor lives only in
memory, so a restarted process accepts an older valid journal of the same
authority; durable ordering evidence outside the journal is owed before any
consumer cutover (#521).

Every recovery-class refusal (`recovery_required`, `corrupt_record`,
`unsupported_schema`, `ambiguous_match`) carries a closed scope, decided
where the service produces it. A store-scoped refusal concerns the authority
itself or cannot be attributed to one set: a rolled-back or overflowing
journal, a failed authority read. It closes the runtime, and explicit
diagnosis is the only route back. A set-scoped refusal is decided from one
set's own state, in a journal adopted from the authority under the lock, or
from that set's own records. It leaves the runtime ready for every other set,
and the next request for that set reads the authoritative journal and record
under the lock again, so it sees another process's recovery without a
reopen. A refusal by another program holding the journal open, before any
native change, is neither: it is a retryable `operation_in_progress` that
closes nothing.

The store's file state is built against one adversary: another local
principal who can create, rename or replace entries in the store's
directories or in one of their ancestors while the app runs, trying to make
the app change, delete or publish over an object it did not qualify. The
rule that follows: every mutation acts on an object qualified through a
handle, and nothing can change which object that is between the
qualification and the act. The one act by path is resolving a root's path
when the store opens; what it reaches is qualified on its handle before
anything changes. Inside a held root, which is owner-only once held, names
are resolved relative to its handle, and only this user can change what
they name. Two gaps in that argument stay named and undecided, owed under
#521 before the store is enabled:

- The ancestor gap: an ancestor writable by another principal can choose
  which of this user's directories the path resolution reaches.
- The in-place gap (Windows): a handle keeps the access it was opened with,
  so a principal who owned a root before an ownership repair, or whom the
  root's old DACL granted `WRITE_DAC` or `WRITE_OWNER`, can keep a handle
  that later makes the root writable again, and sharing modes cannot detect
  it. The owner-only argument holds for a root the store created, owner-only
  from the moment it exists, but not for one repaired or hardened in place.
  The options are to have the repair build a fresh root and move the
  qualified entries into it, to refuse a root whose previous owner or DACL
  was untrusted, or to accept the residual here; none is chosen yet.

Atomic admission allows one live operation, not a backlog of mutations. An
async deadline limits only the caller's wait: it neither cancels nor retries
the worker call, and it does not release admission. The worker reconciles a
late definite commit into the cache and emits its one content-free event before
releasing admission. An epoch another process committed, which this process
only observed in the authority journal, emits one content-free signal with no
receipt and no set, telling consumers to take a fresh snapshot. An unverified
commit is not success and emits no commit event. An unhandled dispatch panic
latches a terminal stall; dispatch enters the crash handler's credential
redaction scope before `catch_unwind` so the panic hook sees the same boundary
as the caller.

## Cutover and Consequences

- [#149](https://github.com/Codeseys-Labs/audio-graph/issues/149) and
  [#156](https://github.com/Codeseys-Labs/audio-graph/issues/156) land the
  dormant core and crate-private declarations, ported from reviewed commit
  `8fd7132` onto `d325001` with the existing IPC contract and redacted panic
  hook. They do not select a platform backend or change live v1 behavior.
- Dark Tauri composition is tracked in
  [#157](https://github.com/Codeseys-Labs/audio-graph/issues/157). The consumer
  cutover and rollback proof in
  [#209](https://github.com/Codeseys-Labs/audio-graph/issues/209) are separate.
  Backend consumers obtain scoped zeroizing leases; IPC exposes typed status
  and receipts, not saved secret readback or renderer-selected audiences.
- V2 migration is explicit. Legacy YAML is import-only: no live override,
  automatic fallback, or v1 dual-write. Verify exact native readback before
  legacy cleanup, keep tombstones authoritative, and gate v1 retirement on
  migration, rollback, and packaged platform evidence. Optional file-v2 is a
  separate decision, not a prerequisite for native rollout.
- A wedged native call leaves the runtime stalled rather than pretending it
  can safely proceed. Deterministic delayed-call and panic-canary tests prove
  the worker contract; they do not prove native prompt, durability, migration,
  or packaged behavior on any platform.
