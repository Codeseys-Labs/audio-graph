---
status: accepted
date: 2026-08-01
deciders: [AudioGraph maintainers]
---

# ADR-0036: Build the Native Keyring Vertical Before Any Optional Fallback

## Context and Problem Statement

ADR-0035 correctly made credentials a backend-owned typed service and selected
native desktop stores as the only automatic production backend. Its initial
implementation plan nevertheless bundled the native adapter with explicit
platform stores, prompt suppression, custom plaintext file-v2, filesystem
qualification, and shared replacement primitives. The optional fallback thus
blocked the shortest production-critical credential path.

The dark service core now supplies a private `CredentialEntryStore` and an
opaque mutation-session seam. We can implement and verify native entry storage,
authority pairing, and mutation locking without exposing secrets or locators to
the renderer and without designing a plaintext fallback.

## Decision Drivers

- Ship the production native credential boundary independently of an optional
  degraded backend.
- Preserve ADR-0035's authority, lifecycle, audience, tombstone, and exact
  readback invariants.
- Minimize platform-specific code while retaining Windows Local persistence.
- Keep the authority journal secret-free and operationally ordinary.
- Represent platform uncertainty honestly instead of inferring details from
  native error prose or claiming unproved no-prompt behavior.

## Decision Outcome

Chosen option: **implement the unified native keyring vertical first and
isolate every optional fallback behind a later decision and evidence track**.

This ADR amends only ADR-0035's combined adapter sequencing and its explicit
platform-adapter implementation choice. It does not supersede ADR-0035 as a
whole and does not rewrite its history.

### Native entry adapter

Production uses `keyring` 4.1.6 behind AudioGraph's private entry interface.
The immutable service is `com.codeseys.audiograph.credentials`; the adapter
alone derives `v2/<set-id>`, `v2-staging/<operation-id>/<set-id>`, and the
reserved `v2/_authority` marker. Raw locators are never accepted over IPC.

All records use binary `set_secret`/`get_secret`. The domain envelope retains
the 2,560-byte portable ceiling and service-level exact readback. Active delete
continues to write an authoritative tombstone. Staging delete is idempotent and
must read back absence rather than trusting the facade's convenience result.

On Windows, the adapter initializes the facade store and constructs v2 entries
through `keyring-core` with the creation modifier `persistence=local`, selecting
Local rather than the facade's default Enterprise persistence. Old derived
Enterprise targets are migration inputs, never v2 targets. macOS retains the
facade's legacy Keychain store. Linux retains the stock zbus Secret Service
store; AudioGraph does not fork it in this vertical.

All entry access is serialized in process. One stable Rust 1.95 file lock is
held for the entire opaque mutation session, including journal load, native
write/readback, and journal commit. A poisoned process-global entry gate fails
closed as `stalled_worker` with a restart action; native access never continues
through a recovered poisoned mutex guard.

The application logger denies the exact `keyring_core` target namespace at
every level because its entry debug records may contain private locators and
platform entry representations. AudioGraph Debug and Trace logs remain
available outside that namespace.

### Authority composition

The native authority marker and a non-secret app-local journal envelope share
one opaque random `authority_instance_id`. Absent marker plus absent journal is
`uninitialized`; an exact well-formed pair is `ready`; one-sided, malformed,
unsupported, or mismatched state is `recovery_required`. Initialization is
explicit. Open never repairs, imports, or chooses another backend.

Opening may create the app-local metadata directory and its stable empty lock
file while acquiring the mutation lock. An uninitialized open performs no
authority marker or journal-state write; the lock file is coordination
infrastructure, not authority state.

Before first publication, initialization checks the exact built-in active v2
accounts while holding both locks. An existing built-in entry without an
authority pair fails closed as corrupt state. A typed keyring read failure is
preserved, and staging or custom namespaces are not enumerated.

The journal is ordinary application metadata, not file-v2 and not a secret
container. It may use `atomic-write-file` 0.3.0 for same-directory old-or-new
replacement. Its envelope remains strictly secret-free, rejects unknown wire
fields, is capped before deserialization, and is accepted only after bounded
set/intent/history and cross-reference validation. Unix creates and repairs
the metadata directory as `0700` and the journal and stable lock as `0600`;
permission repair failures fail closed.

Atomic open and temporary-file write failures occur before publication and
remain definite unavailable failures. Rename/commit, permission repair after
publication, and exact final-path readback uncertainty map to
`commit_unknown`; startup pairing and reconciliation determine the next safe
action. A true child-process contention/crash-release contract covers the
stable file lock.

On Windows, this source slice accepts the crate's ordinary old-or-new metadata
replacement plus exact final-path and marker/journal readback. It does not
claim target-native inherited-ACL or namespace write-through durability proof;
those packaged checks remain with `audio-graph-c4c5`. This choice does not
approve `atomic-write-file` for plaintext secret storage and does not weaken
the separate file-v2 requirements recorded in ADR-0035.

### Error and platform evidence boundary

Keyring errors are matched by structural variant only. Platform error objects,
bad-encoding bytes, bad-format bytes, store strings, secret material, and
locators are never formatted, logged, serialized, or returned. The generic
taxonomy cannot always distinguish locked, denied, and cancelled outcomes;
unknown platform failures therefore map conservatively to a closed failure.
Discarded post-delete failures pass through the same owned byte/string
scrubber before being collapsed to `commit_unknown`.

The facade selection is an implementation decision, not packaged behavior
proof. Linux zbus may activate or unlock a service session. Windows Local
persistence and macOS/Linux prompt behavior remain target-native acceptance
gates under Seeds `audio-graph-3098` and `audio-graph-c4c5`.

This synchronous adapter and its process mutex do not satisfy the dedicated
serialized blocking-worker requirement. That composition work remains
downstream under `audio-graph-f107`.

### Optional fallback

No custom plaintext file-v2, Stronghold, filesystem detector, DACL qualifier,
support profile, or fallback selector is part of this vertical. A later
explicit fallback may implement the same service interface only after its own
threat model, consent, recovery, and platform evidence are accepted. Native
missing, locked, denied, cancelled, unavailable, or failed results never select
it automatically.

## Consequences

- The native critical path becomes independently testable through one narrow
  injected entry/filesystem boundary.
- Windows needs a small direct `keyring-core` dependency solely for the Local
  creation modifier; other platforms keep the unified facade.
- Generic error classification and packaged no-prompt behavior remain visible
  release risks rather than being hidden behind adapter abstraction.
- Optional fallback work can evolve without coupling plaintext storage or
  filesystem policy into native credential access.

## Retained ADR-0035 Invariants

Backend ownership, Rust-only scoped leases, closed audience authorization,
fixed technical identities, one logical bundle per entry, retained tombstones,
bounded encoded records, exact readback, marker/journal agreement,
content-free errors, no secret readback over IPC, and no automatic plaintext
fallback remain in force.
