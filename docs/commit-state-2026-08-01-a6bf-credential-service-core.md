# CredentialService Core Worktree State

Date: 2026-08-01

Seed: `audio-graph-a6bf`

Branch: `work/audio-graph-a6bf-credential-service-core`

Base: `b59c8c8ca0f931510642049d60f25ba4d6d36103`

Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/a6bf-credential-service-core`

## Custody

- This clean worktree is the only write surface for the Seed.
- The main checkout and other credential-v2 worktrees are custody-only.
- No adapter, migration, AppState, IPC, settings, runtime, frontend, generated, dependency, lockfile, workflow, or Seed file belongs to this branch.
- Accepted WS2 history is immutable; review fixes, if required, will use a separate commit.

## Acceptance

Implement the dark backend-owned credential service core against injected ports:

- journal-only zero-I/O status projection;
- authorization before any entry read;
- zeroizing stored-secret leases and explicit non-store authority outcomes;
- one-record present/retained-tombstone envelopes with the 2,560-byte portable limit;
- expected-revision CAS, bounded secret-free idempotency, exact readback, and post-commit events;
- an opaque mutation session spanning journal/CAS through entry readback and journal commit;
- worker serialization/stalled admission behavior; and
- deterministic fake, failure, concurrency, activation, property-style, and redaction tests.

The confirmed test seams are the crate-internal `CredentialService` API, the opaque entry-store/mutation-session boundary, the worker-admission state, the settings-activation port, and the deterministic fake's safe observations. Tests do not inspect native stores, filesystems, Tauri state, or implementation-private secret bytes.

## Ownership

- `src-tauri/src/credentials/domain.rs`
- `src-tauri/src/credentials/service.rs`
- `src-tauri/src/credentials/fake.rs`
- `src-tauri/src/credentials/test_support.rs`
- declarations/re-exports only in `src-tauri/src/credentials/mod.rs`
- this commit-state record

## Stop conditions

- Stop if the atomic mutation-session contract requires a concrete WS3B adapter or dependency/lockfile change.
- Stop rather than add migration, production wiring, settings persistence, or real keychain/filesystem access.
- Do not turn a non-store authentication method or backend failure into credential absence.
- Do not persist secret-derived fingerprints, lengths, paths, or native error prose in journal/status/error/event artifacts.

## Planned gates

Use `/tmp/audio-graph-target-a6bf` as the isolated Cargo target. Run focused credential tests, accepted IPC-contract tests, cloud check/clippy, Rust formatting, generated-contract drift, documentation/Seeds hygiene with baseline accounting, scoped diff checks, and clean status. Full command output is recorded in the implementation artifact.
