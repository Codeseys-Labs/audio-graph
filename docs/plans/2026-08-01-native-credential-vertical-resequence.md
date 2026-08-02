# Native credential vertical resequence

Date: 2026-08-01

Seed: `audio-graph-fb2b`

Decision: [ADR-0036](../adr/0036-native-keyring-vertical-and-optional-fallback.md)

## Why this plan exists

The July 31 credential rebuild plan combined the production native adapter,
custom plaintext file-v2, filesystem qualification, explicit platform stores,
prompt control, and shared durability primitives in one WS3B. That shape made
the optional fallback architecture a prerequisite for the production native
path.

This plan supersedes only that WS3B implementation sequence. It does not edit
or erase the old plan, and it does not replace ADR-0035's service and security
model.

## Native-first vertical

Implement one behavior at a time through `CredentialEntryStore` and the
production open result:

1. Derive the frozen service and exact active/staging/marker accounts entirely
   in Rust, then prove binary set/get and exact readback through an injected
   entry boundary.
2. Use `keyring` 4.1.6 as the unified production facade. On Windows, initialize
   the facade store and construct entries with the underlying `keyring-core`
   `persistence=local` modifier; never write the legacy derived Enterprise
   target. Keep exact literals under contract tests.
3. Persist the secret-free authority journal with an ordinary app-local atomic
   metadata writer. Enforce a byte cap before strict deserialization, validate
   bounded state and all owned cross-references, and pair the envelope with the
   `v2/_authority` marker using a new random `authority_instance_id` created
   only by explicit initialization.
4. Classify open as `uninitialized`, `ready`, or `recovery_required`:
   absent+absent is uninitialized; an exact valid id pair is ready; every
   one-sided, malformed, unsupported, or mismatched pair requires recovery.
   Never auto-repair, import, or select a fallback. Lock acquisition may create
   the app-local directory and stable empty lock file, but an uninitialized
   open writes neither authority marker nor journal state. Before first
   publication, reject orphan built-in active v2 entries without enumerating
   staging or custom namespaces, and preserve typed read failures.
5. Serialize all entry access in process and retain one stable Rust 1.95 file
   lock from mutation-session creation through journal commit. Preserve
   `commit_unknown` after any write whose effect cannot be proven. Treat
   missing staging deletion as success, and verify absence after deletion
   because the Apple facade can discard a native delete result. Fail a poisoned
   process entry gate as `stalled_worker`/restart rather than recovering its
   guard. Exercise contention and killed-owner release in a child process.
6. Keep the ordinary journal metadata owner-only on Unix (`0700` root, `0600`
   state and lock, including repair of broader existing modes). Preserve
   definite errors for atomic open/temp-write failures; classify commit and
   exact-readback uncertainty as `commit_unknown`. Suppress the exact
   `keyring_core` log namespace because upstream debug records carry locators.
7. Run focused adapter/service contracts, all credential tests, formatting,
   host check and strict Clippy. Record cross-target compile blockers rather
   than substituting compile-only claims for packaged runtime evidence.

## Risk register and downstream evidence

- `keyring-core` exposes a generic, non-exhaustive error taxonomy. Variant-only
  mapping can preserve missing, unavailable, unsupported, corrupt, oversized,
  and internal outcomes, but generic platform failures cannot honestly prove
  a platform-specific locked/denied/cancelled distinction. Do not inspect or
  format associated platform errors or byte-bearing variants to guess.
- The stock Linux zbus store may activate or unlock a Secret Service session.
  This slice does not fork upstream or claim background no-prompt behavior.
- macOS remains on the legacy Keychain selected by the facade. Packaged
  prompt/unlock behavior and delete verification need target-native proof.
- Windows Local persistence is structurally selected for new v2 entries;
  target-native persistence and locked-store behavior still need packaged
  proof. The ordinary metadata path has source-level atomic replacement and
  exact readback, but Windows inherited-ACL and namespace write-through
  durability remain target-native evidence rather than source claims.
- Seeds `audio-graph-3098` and `audio-graph-c4c5` retain the downstream
  packaged/no-prompt acceptance work. Migration of legacy v1 locators remains
  with `audio-graph-86e9`.
- The adapter remains synchronous. The dedicated serialized blocking worker
  and runtime composition remain downstream under `audio-graph-f107`; the
  in-process mutex in this slice does not satisfy that Seed.

## Optional fallback isolation

File-v2 or another explicit degraded backend may be designed later behind the
same service interface. It gets its own threat model, consent, paths, format,
permissions, detectors, profiles, recovery, and release evidence. None of
those mechanisms are imported into this native vertical, and a native failure
never activates them.

## Stop conditions

Stop this slice if implementation requires renderer-visible raw locators,
secret readback, file-v2 coupling, migration behavior, or widening the service
interface beyond crate-private composition.
