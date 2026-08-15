# audio-graph-a596 typed Session Artifact manifest kernel report

Date: 2026-08-15

Seed: `audio-graph-a596`

Branch: `work/a596-artifact-manifest-kernel-wave7b`

Exact base: `5444b6630850302128196dcd8b7689afe3a30bb1`

Worktree:
`/home/codeseys/DevBox/audio-graph/.worktrees/a596-artifact-manifest-kernel-wave7b`

Implementation commit: `bec56b9f44aed27f113a143cec21b768778256ab`

Review correction round 1 commit: `0dfb6e7f20931e33cb36557c4a12d7baf326a566`

Review correction round 2 commit: `ded9230df0b91b0aaaa627e3ee43e9d484990bc5`

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

Final corrected result:

```text
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 1626 filtered out; finished in 8.12s
```

The focused suite covers absent strict load without provisioning, present head
without lock, malformed/duplicate/wrong-schema/unknown wire data, portable and
case-equivalent identity rejection, the complete stable kind vocabulary,
golden deterministic V1 roundtrip, initial/replacement CAS, stale/overflow
generation, unqualified namespace refusal, Prepared-to-Completed, exact retry,
fingerprint conflict, completed regression, restart reopen, quarantine
reference/length/residual invariants, Original Session Audio unavailable
evidence, and deletion inventory/internal self-reference parity.

### Independent review correction RED/GREEN

Review round 1 identified five admission/read strictness gaps. Each correction
started with a focused RED against the public seam or one deterministic private
read-race seam, then passed alone before the consolidated run.

#### Exact Prepared-to-Completed transaction

Initial RED exited 101 because the typed refusal classes did not exist:

```text
error[E0599]: no variant or associated item named `CompletionRequiresPrepared`
error[E0599]: no variant or associated item named `PreparedCompletionConflict`
```

After the first correction, a second RED proved that an unrelated already
Completed head could still admit a direct Completed quarantine snapshot:

```text
assertion failed: matches!(..., CompletionRequiresPrepared)
test result: FAILED. 0 passed; 1 failed
```

GREEN requires a current durable `Prepared` head for the exact same immutable
transaction. The only Prepared-to-Completed changes permitted are:

- the manifest generation and manifest/quarantine phase;
- `SourceFull` to `SourceTruncated` (or retaining an already observed
  `SourceTruncated` prepared residual);
- the one source artifact moving from its recorded before content to its
  recorded target content; and
- the one matching quarantine entry moving to the corresponding
  source-truncated residual reason.

Session, id/fingerprint, source-before/source-after/quarantine identities,
hashes, lengths, kinds, privacy classes, and every unrelated inventory entry
must remain byte-for-byte equivalent after normalizing those allowed fields.
Tests independently alter source identity, source-before hash, lengths,
expected target, quarantine identity/reference, and inventory; each returns
`PreparedCompletionConflict`. Direct Completed from Absent or from an
unrelated Completed head returns `CompletionRequiresPrepared`. Exact Completed
retry still returns `AlreadyCompleted` at the existing generation.

#### Candidate size preflight

RED exited 101:

```text
error[E0599]: no variant named `ManifestTooLarge` found for enum `ManifestCasRejection`
```

GREEN serializes the fully normalized generation-assigned candidate before
calling `install_snapshot`. An exact 16 MiB candidate reaches the typed
unqualified-namespace refusal; a 16 MiB + 1 byte candidate returns
`ManifestTooLarge { byte_length: 16777217 }`. Neither case creates a manifest
or temp, and the oversized case never calls the durability install seam.

#### Bounded, revalidated strict load

RED exited 101:

```text
error[E0425]: cannot find function `load_manifest_file_with_after_open`
error[E0599]: no variant or associated item named `ChangedDuringRead`
```

GREEN rejects symlinks/non-regular pathname entries, opens the head, validates
the open handle's regular-file type and length, reads through a `MAX + 1`
`Take` bound, then revalidates handle type and exact length before schema
probing or decoding. A deterministic test changes the open file length at the
read seam and receives `ChangedDuringRead`. Same-length mutation by an
uncooperative process remains outside the documented cooperative-lock threat
boundary; no unsafe code or dependency was added.

#### Portable control and component floor

RED showed that a newline-bearing identity was admitted:

```text
control must be rejected: "line\\nbreak"
test result: FAILED. 0 passed; 1 failed
```

GREEN rejects ASCII controls including newline, tab, and DEL; Unicode control
characters; conservative Unicode format/bidirectional controls; and any path
component longer than 255 UTF-8 bytes.

#### Persisted generation floor

RED exited 101:

```text
error[E0599]: no variant named `InvalidGeneration` found for enum `ManifestValidationError`
```

GREEN separates uncommitted candidate validation from persisted-head
validation. Initial CAS still uses expected coordinate 0 and assigns durable
generation 1, while strict load and the golden persisted V1 validation reject
generation 0 as `InvalidGeneration { actual: 0 }`.

Two intermediate failures were fixture corrections rather than interface
changes: one test attempted a shared load while its own exclusive transaction
was still alive, and one assertion counted the manifest transition fingerprint
while checking that an unavailable artifact carried no content identity.

### Independent review correction round 2 RED/GREEN

The final-cap review identified one state-machine bypass and one missing
portable-identity bound. Both corrections remained inside the manifest module;
no successor Seed or scope expansion was needed.

#### Durable Prepared-head transition dispatch

RED proved that a candidate could remove the quarantine transaction from a
durable Prepared head and be admitted as an ordinary Completed transition:

```text
assertion failed: matches!(transaction.compare_and_swap(1, removed),
    ManifestCasOutcome::Rejected(ManifestCasRejection::PreparedCompletionConflict))
test result: FAILED. 0 passed; 1 failed
```

GREEN drives transition validation from the current durable head. Once that
head contains a Prepared quarantine transaction, every candidate must retain
the same transaction in Completed phase and satisfy the exact immutable
predecessor comparison documented above. Removing the transaction, presenting
an ordinary Completed manifest, retaining Prepared, or altering transaction or
inventory fields is rejected without durability mutation. The regression
reopens generation 1 and confirms that its Prepared transaction and inventory
remain intact and no temp was created. Exact Completed retry remains
`AlreadyCompleted` without generation advancement.

#### Total managed-identity byte ceiling

RED failed to compile because the durable V1 total-path ceiling did not exist:

```text
error[E0425]: cannot find value `MAX_MANAGED_ARTIFACT_IDENTITY_BYTES` in this scope
```

GREEN freezes `MAX_MANAGED_ARTIFACT_IDENTITY_BYTES` at 1023 UTF-8 bytes, in
addition to the existing 255-byte component ceiling. This is a conservative
cross-platform manifest-wire bound; platform integrations must still validate
the resolved root plus relative identity. Tests accept exactly 1023 bytes and
reject 1024 while keeping every component valid. The 16 MiB serialization
fixture now reaches its boundary using many unique, valid, bounded identities
instead of one intentionally oversized path.

## Gates and real results

All Rust gates used Rust/Cargo 1.95.0 and the stable worktree-local target
`src-tauri/target`.

### Canonical durability regressions

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud canonical_durability -- --nocapture --test-threads=1
```

Result: `38 passed; 0 failed; 0 ignored; 1606 filtered out`.

### Locked check, strict Clippy, rustfmt, and diff

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
git diff --check
```

Final corrected results: locked check passed in 8.68s; strict Clippy passed in
18.31s; rustfmt and diff checks passed. The initial implementation also
corrected Clippy's large-enum finding by boxing the `Present` load payload.

### One full serialized locked cloud library suite

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud -- --test-threads=1
```

Result:

```text
test result: ok. 1636 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 50.23s
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

The review correction range
`80b97aedc01adc846afedf9734d5909a479a3d89..0dfb6e7f20931e33cb36557c4a12d7baf326a566`
contains only:

```text
src-tauri/src/persistence/session_artifact_manifest.rs
```

The final-cap correction range
`e12e2aca7af82deac13b2b330e87d21d143e16a1..ded9230df0b91b0aaaa627e3ee43e9d484990bc5`
also contains only:

```text
src-tauri/src/persistence/session_artifact_manifest.rs
```

The complete implementation footprint remains:

```text
src-tauri/src/persistence/mod.rs
src-tauri/src/persistence/session_artifact_manifest.rs
```

No `.seeds` state was edited from this implementation worktree. No runtime
consumer was activated. Broad export/delete/purge/backup/recovery adoption
remains with `audio-graph-be7c`; locked recovery remains with the next Wave 7B
workstream. Production construction of namespace qualification and native
Windows/macOS filesystem execution evidence remain later platform work.

The final-cap correction is complete with no successor blocker: neither
finding required a durability-interface change, consumer migration, dependency,
workflow, or platform-runtime edit.

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
