# audio-graph-a596 typed Session Artifact manifest kernel report

Date: 2026-08-15

Seed: `audio-graph-a596`

Branch: `work/a596-artifact-manifest-kernel-wave7b`

Exact base: `5444b6630850302128196dcd8b7689afe3a30bb1`

Worktree:
`/home/codeseys/DevBox/audio-graph/.worktrees/a596-artifact-manifest-kernel-wave7b`

Implementation commit: `bec56b9f44aed27f113a143cec21b768778256ab`

## Outcome

Implemented one dormant, explicit-root deep module for the persisted typed
Session Artifact manifest. The module has no runtime caller and does not alter
repositories, sessions, commands, generated contracts, UI, dependencies,
workflows, Docker, Blacksmith, or the canonical durability substrate.

The public seam provides:

- strict `SessionArtifactManifestV1`, `SessionArtifactEntry`, typed artifact
  vocabulary, privacy classes, availability and content-free unavailable
  reasons;
- portable `ManagedArtifactIdentity`, `Sha256Digest`, content identity, and
  managed source identity types;
- optional `QuarantineTransaction` with only `Prepared` and `Completed`
  states plus exact source-full/source-truncated residual state;
- `SessionArtifactManifestStore`, `ManifestLoadOutcome`, and a
  guard-owning `ManifestWriteTransaction` with typed generation-CAS outcomes;
  and
- deterministic internal manifest, temp, and coordination identities kept
  outside the artifact inventory.

The artifact-kind vocabulary includes Original Session Audio, session
provenance, the four ADR-0037 streams, materialized notes/graph and snapshot
forms, scheduler, usage, live-assist current/audit, data movement, quarantine,
recovery, metadata, and legacy compatibility artifacts.

## Invariants and durability boundary

The module rejects absolute or prefixed identities, backslashes, NUL, empty,
dot, dot-dot, trailing dot/space, Windows-reserved path segments, internal
coordination aliases, and ASCII-case-equivalent duplicate identities. Hashes
must be exactly `sha256:` plus 64 lowercase hexadecimal digits. Persisted
source identity is managed identity plus content hash and length; no inode,
file index, or platform object identity enters the manifest wire.

Every accepted candidate has schema version 1, a non-empty safe Session and
idempotency identity, one stable Original Session Audio entry, a SHA-256
transition fingerprint, and a normalized deterministic inventory. Serde
structs deny unknown fields, scalar enums have no catch-all/default, the
schema is probed before V1 decoding, and duplicate JSON object members fail.

Quarantine validation requires:

- transition id, fingerprint, and phase match the manifest transition;
- source-before/source-after share one managed identity;
- the quarantine identity is distinct;
- target length is strictly shorter and the quarantine length is the exact
  difference;
- exactly one matching recovery artifact has the exact hash, length, and
  residual reason;
- exactly one source entry reflects the observed full or truncated state; and
- `Completed` is possible only with source-truncated residual state.

The store never resolves or creates a default product root. Missing load is
non-mutating. A present manifest without the canonical coordination entry
fails closed. A write transaction owns the exact
`CanonicalExclusiveGuard`, loads and validates the current head under it,
binds replacement to the exact open head file, assigns `expected + 1`, and
calls `CanonicalExclusiveGuard::install_snapshot`.

`NamespaceDurabilityUnsupported` stays a typed durability rejection and
`DurabilityIndeterminate` stays indeterminate. Neither can become manifest
`Accepted`. Stale and overflow generations do not create a manifest or temp.
Same id with another fingerprint is `IdempotencyConflict`; exact completed
retry is `AlreadyCompleted` without generation advancement; the same
transaction cannot regress from completed to prepared. Prepared-to-completed
advances exactly once.

## TDD evidence

The agreed seam was the explicit-root store plus its guard-owning write
transaction. Tests observe load, CAS results, and durable reopen through that
seam; no test qualification constructor exists in production.

### Public-seam RED

Command:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud public_manifest_store_seam_loads_an_absent_explicit_root_without_mutation -- --nocapture --test-threads=1
```

Real result: exit 101.

```text
error[E0433]: cannot find type `SessionArtifactManifestStore` in this scope
error[E0433]: cannot find type `ManifestLoadOutcome` in this scope
error: could not compile `audio-graph` (lib test) due to 2 previous errors
```

### First GREEN

The same command passed after the first vertical slice:

```text
running 1 test
test persistence::session_artifact_manifest::tests::public_manifest_store_seam_loads_an_absent_explicit_root_without_mutation ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 1626 filtered out
```

### Focused final GREEN

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud session_artifact_manifest -- --nocapture --test-threads=1
```

Final result:

```text
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 1626 filtered out; finished in 0.05s
```

The focused suite covers absent strict load without provisioning, present head
without lock, malformed/duplicate/wrong-schema/unknown wire data, portable and
case-equivalent identity rejection, the complete stable kind vocabulary,
golden deterministic V1 roundtrip, initial/replacement CAS, stale/overflow
generation, unqualified namespace refusal, Prepared-to-Completed, exact retry,
fingerprint conflict, completed regression, restart reopen, quarantine
reference/length/residual invariants, Original Session Audio unavailable
evidence, and deletion inventory/internal self-reference parity.

Two intermediate failures were fixture corrections rather than interface
changes: one test attempted a shared load while its own exclusive transaction
was still alive, and one assertion counted the manifest transition fingerprint
while checking that an unavailable artifact carried no content identity.

## Gates and real results

All Rust gates used Rust/Cargo 1.95.0 and the stable worktree-local target
`src-tauri/target`.

### Canonical durability regressions

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud canonical_durability -- --nocapture --test-threads=1
```

Result: `38 passed; 0 failed; 0 ignored; 1601 filtered out`.

### Locked check, strict Clippy, rustfmt, and diff

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
git diff --check
```

Results: locked check passed in 2m04s; strict Clippy passed in 40.74s after
boxing the large `Present` load payload; rustfmt and diff checks passed.

### One full serialized locked cloud library suite

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud -- --test-threads=1
```

Result:

```text
test result: ok. 1631 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 42.03s
```

### Windows actual-module compile evidence

The installed target is `x86_64-pc-windows-msvc`. A dependency-minimal Cargo
probe imported the actual production `canonical_durability.rs` and
`session_artifact_manifest.rs` modules and compiled them for Windows:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target/a596-windows-probe/target" cargo +1.95.0 check --manifest-path src-tauri/target/a596-windows-probe/Cargo.toml --target x86_64-pc-windows-msvc
```

Result: exit 0; `Finished dev profile`.

The same actual modules, including their `cfg(test)` code, compiled to a
Windows test object with pinned `rustc +1.95.0 --test --emit=obj`: exit 0 with
only expected dead-code/unused warnings from platform-gated inherited
canonical-durability test helpers.

A full application Windows Cargo check was also attempted. It stopped before
AudioGraph compilation in transitive `ring` because this Linux host has no
MSVC `lib.exe`. That host-toolchain stop is not reported as a product gate and
does not replace the passing actual-module production/test-object evidence.

### Contracts, pinned verify-fast, and secret gates

The first ordinary `bun run verify:fast` passed Biome, TypeScript, and all five
contract checks, then correctly exposed that this worktree has no local
`node_modules` and the fallback global Seeds CLI lacks the pipe-safe stdout
patch. It had already confirmed the repository-pinned CLI was patched.

The authoritative pinned rerun was:

```text
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/node_modules/@os-eco/seeds-cli bun run verify:fast
```

Result: exit 0. Biome checked 174 files; TypeScript passed; audio-source,
provider-registry, session-data-movement, endpoint-credential-routing, and
speech-span contracts were current; Seeds output stress parsed ready 50,
blocked 100, and list 50; docs/Seeds secret hygiene found 0 findings; diff
check passed.

```text
betterleaks dir --no-banner --redact src-tauri/src/persistence/session_artifact_manifest.rs src-tauri/src/persistence/mod.rs
```

Result: no leaks found.

## Exact footprint and non-goals

Implementation range `5444b6630850302128196dcd8b7689afe3a30bb1..bec56b9f44aed27f113a143cec21b768778256ab`
contains only:

```text
src-tauri/src/persistence/mod.rs
src-tauri/src/persistence/session_artifact_manifest.rs
```

No `.seeds` state was edited from this implementation worktree. No runtime
consumer was activated. Broad export/delete/purge/backup/recovery adoption
remains with `audio-graph-be7c`; locked recovery remains with the next Wave 7B
workstream. Production construction of namespace qualification and native
Windows/macOS filesystem execution evidence remain later platform work.

## Findings and open questions

- No accepted durability-interface backflow blocker remains. The c928
  guard-owned snapshot interface supports honest initial and exact-open-head
  replacement CAS.
- The manifest and coordination basenames are intentionally duplicated from
  the private durability implementation because that module does not expose
  forgeable namespace internals. The manifest tests expose deterministic
  internal identities and reserve every ASCII-case alias; later consumer
  integration should retain a drift assertion when it begins deletion
  ordering.
- Replacement temps are never promoted by filename inference. A later locked
  recovery workstream must reconcile an indeterminate retained temp under the
  same guard, per the selected 661f transaction design.
