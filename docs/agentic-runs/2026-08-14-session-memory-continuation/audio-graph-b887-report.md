# audio-graph-b887 implementation report

## Assignment and custody

- Seed: `audio-graph-b887` — implement the dormant Session semantics floor
  kernel and checked-open contract.
- Acceptance: missing historical floor is v1; only the v1-to-v2 floor change
  is legal; logical advancement requires actual `ManifestCasOutcome::Accepted`
  or exact `AlreadyCompleted` evidence; guard-ahead retry is idempotent;
  transcript/basis/patch-ahead under v1 is distinct typed content-free
  corruption; checked open refuses unsupported floors before content read; the
  manifest wire carries the floor and a v2 manifest inventories Session
  provenance; unsupported values and regression fail closed; unqualified
  production persistence cannot claim `Accepted`.
- Exact execution base: `967cb4837b58592d180a3cdb22675d28e6101c36`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/b887-session-semantics-kernel-wave7c`.
- Branch: `work/audio-graph-b887-session-semantics-kernel-wave7c`.
- Initial state: clean worktree at the exact execution base.

## Outcome

The new deep module defines a closed numeric v1/v2 Session floor with a
historical-v1 default. Its advancement seam consumes the manifest CAS outcome
itself, binds the evidence to the expected Session id, accepts only durable
`Accepted` or exact `AlreadyCompleted`, permits only v1-to-v2 advancement, and
returns exact v2 on a guard-ahead retry. Rejected or indeterminate manifest
CAS cannot advance the logical floor; no generic receipt, boolean, setter, or
production filesystem qualification was introduced.

The artifact guard exposes separate content-free corruption variants for a v2
transcript revision, Projection Basis, and projection patch observed under
floor v1. The checked-open seam validates both the persisted Session floor and
reader capability before invoking its supplied canonical-or-legacy content
reader closure.

`SessionArtifactManifestV1` now serializes
`session_semantics_version` explicitly for every new candidate while serde
defaults historical missing wire to v1. Manifest validation rejects unknown
floor values, requires one exact `SessionProvenanceEvents` proof at v2, and
manifest CAS rejects floor regression. That proof must be
`CanonicalSessionMemory`, `Present`, attached to a `Completed` transition, and
bind the transition fingerprint to the provenance content SHA-256. The
existing unqualified and Windows policy refusal paths remain unchanged and
passing.

The design is dormant. There is no fifth ADR-0037 stream, production v2
artifact append, basis or patch activation, broad consumer migration,
predecessor canary, dependency/workflow/generated/frontend change, or new
durability claim.

## Stable-snapshot review correction

The Wave 7C stable-snapshot reviews disagreed: Standards returned **BLOCK**
with one P1 finding, while Spec returned **SHIP**. The Standards finding was
accepted as load-bearing. The prior v2 validator only counted provenance-kind
entries, and `admitted_session_semantics_floor` trusted the manifest carried by
an `Accepted` or `AlreadyCompleted` enum without independently revalidating its
proof.

The correction adds the content-free typed `V2SessionProvenanceError` family
for missing, duplicate, privacy-mismatched, unavailable, residual,
non-completed, and transition-fingerprint-mismatched proofs. One shared proof
validator now runs both during v2 candidate/persisted validation and again at
the logical-floor admission boundary. Consequently, a caller-mutated or forged
`Accepted` / `AlreadyCompleted` payload cannot promote or preserve v2. The
historical v1 wire, v1 validation order, qualified v1-to-v2 CAS, exact retry,
floor-regression stop, and unqualified production refusal remain intact.

### Correction RED / GREEN evidence

Initial RED command:

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud session_semantics -- --nocapture
```

RED result: compile failed with two `E0432` unresolved imports for
`V2SessionProvenanceError`, one `E0599` for the missing manifest-validation
variant, and two `E0599` failures for the missing admission-error variant.
After the validator existed, the retained old test fixture also demonstrated
the binding requirement: four qualified v2 CAS assertions failed until their
transition fingerprint matched the exact provenance content digest. The
non-completed manifest case initially returned `PreparedWithoutQuarantine`
until the v2 proof check was placed ahead of that v1-compatible generic stop.

Final GREEN results:

- `session_semantics`: 11 passed, 0 failed, 1,686 filtered out. This includes
  actual qualified v1-to-v2 acceptance, exact `AlreadyCompleted` retry, forged
  `Accepted` privacy rejection, and forged retry fingerprint rejection.
- `session_artifact_manifest`: 23 passed, 0 failed, 1,674 filtered out. The
  table test covers all seven content-free proof errors, including a duplicate
  with the exact same managed identity.
- locked cloud `cargo check`: pass; dev profile completed.
- strict cloud Clippy with `-D warnings`: pass; dev profile completed.
- rustfmt `--check`: pass with no diff.

Correction footprint is exactly the two owned Rust modules plus this owned
report. It does not alter the generic v1 manifest contract, production
qualification, module registration, dependencies, workflows, generated files,
frontend code, Seeds, or any other module.

## TDD evidence

The agreed public seams were implemented as vertical red/green slices.

1. Historical default:
   - RED command:
     `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud session_semantics -- --nocapture`
   - RED: `error[E0432]`, unresolved public
     `SessionSemanticsVersion` at `session_semantics.rs:5`.
   - GREEN: `historical_missing_floor_resolves_to_v1 ... ok`.
2. Historical/new manifest wire:
   - RED command targeted
     `historical_wire_defaults_floor_to_v1_and_new_candidate_wire_is_explicit`.
   - RED: `error[E0609]`, no field `session_semantics_version` on
     `SessionArtifactManifestV1` at `session_artifact_manifest.rs:1617`.
   - GREEN: historical missing field decoded as v1 and candidate JSON emitted
     numeric `1`.
3. V2 inventory, unsupported value, and regression:
   - REDs were missing typed variants
     `MissingSessionProvenanceEvents`,
     `UnsupportedSessionSemanticsVersion`, and
     `SessionSemanticsFloorRegression` (`E0599`) at their focused public CAS or
     load seams.
   - GREEN: v2 without provenance rejects, persisted floor `3` rejects, and an
     Accepted v2 head refuses a v1 replacement. The deterministic wire suite
     then reported the expected old-golden mismatch with 22 passing / 1
     failing before the explicit-floor golden was updated to 23 passing.
4. Receipt-gated advance and exact retry:
   - RED: `error[E0432]`, missing
     `admitted_session_semantics_floor` at the actual qualified CAS seam.
   - GREEN: actual `Accepted` advanced v1 to v2.
   - Retry RED: assertion returned `Err(ManifestCasNotAccepted)` instead of
     `Ok(SessionSemanticsVersion(2))` for the actual
     `AlreadyCompleted` retry.
   - Retry GREEN: the exact guard-ahead retry preserved v2.
5. Artifact-ahead classification:
   - RED: `error[E0432]` for the missing artifact, corruption, and validation
     public seam.
   - GREEN: transcript, basis, and patch each returned its own typed
     content-free corruption under v1 and were admitted under v2.
6. Checked open:
   - RED: `error[E0432]` for missing `CheckedSessionOpenError` and
     `checked_session_open`.
   - GREEN: a v1-capable reader refused a v2-floor Session without invoking the
     supplied closure; a supported floor invoked it exactly once.

Two initial regression fixtures used non-hex digest characters and therefore
correctly tripped the existing `InvalidSha256` validator. The fixture literals
were corrected to valid lowercase hexadecimal before GREEN; production
validation was not weakened.

## Gates and real results

### Focused kernel and manifest

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud session_semantics -- --nocapture
```

Result after correction: pass; 11 passed, 0 failed, 1,686 filtered out.

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud session_artifact_manifest -- --nocapture
```

Result after correction: pass; 23 passed, 0 failed, 1,674 filtered out.

### Locked check and frontend typecheck

```text
cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud
bun run typecheck
```

Result: pass. Cargo finished the dev profile; TypeScript emitted no diagnostic.

### Serialized full cloud library

```text
cargo +1.95.0 test --quiet --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud -- --test-threads=1
```

Result: pass, exit 0; 1,687 passed, 0 failed, 8 ignored, 0 measured,
0 filtered out; 57.14 seconds. PipeWire/ALSA emitted the expected host-without-
audio-device diagnostics without a test failure.

### Strict Clippy and rustfmt

```text
cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml -- --check
```

Result: pass; Clippy finished with no warning and rustfmt reported no diff.

### Pinned fast gate and all five contracts

```text
SEEDS_CLI_ROOT=/home/codeseys/DevBox/audio-graph/.worktrees/b887-session-semantics-kernel-wave7c/node_modules/@os-eco/seeds-cli \
  bun run verify:fast
bun run verify:contracts
```

Result: pass, exit 0. `verify:fast` checked Biome over 174 files, TypeScript,
all five generated contracts, the direct LABSN action contract and 18
mutations, Seeds JSON output (`ready` 50, `blocked` 93, `list` 50), docs/Seeds
secret hygiene with 0 findings, and diff hygiene. The explicit contract rerun
confirmed current audio-source, provider-registry, session-data-movement,
endpoint-credential-routing, and speech-span generated artifacts.

### Final hygiene, footprint, and runtime dormancy

```text
betterleaks dir --no-banner --redact \
  src-tauri/src/persistence/session_semantics.rs \
  src-tauri/src/persistence/mod.rs \
  src-tauri/src/persistence/session_artifact_manifest.rs \
  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-7e81-wave7c-plan.md \
  docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-b887-report.md
```

Result: pass; approximately 358,674 bytes scanned in 290 ms, no leaks found.

`git diff --check` against the exact base passed. The forbidden-path diff over
`.seeds`, workflows, dependency manifests/lockfile, generated contracts,
`commands.rs`, `sessions/mod.rs`, `projections.rs`, `canonical_log.rs`, and
`canonical_durability.rs` was empty. Runtime symbol search for the new advance,
artifact-check, and checked-open seams returned only
`src-tauri/src/persistence/session_semantics.rs`; there is no production call
site. The pre-report tracked range contained exactly the plan and three owned
Rust paths, with this report as the sole untracked path. The post-report commit
rerun must therefore contain exactly the five assigned paths.

## Commits

- `a7accbd` — `docs(audio-graph-b887): record Wave 7C kernel plan`
- `48001ca` — `feat(audio-graph-b887): add dormant session semantics kernel`
- `8ec57c5` — `docs(audio-graph-b887): record Wave 7C evidence`
- The bounded Standards correction is committed separately with this updated
  report; its exact hash is returned to the conductor after commit.

## Findings, caveats, and open questions

- No implementation blocker or unrelated defect was found.
- Linux algorithm-qualified tests establish the in-process CAS semantics only.
  They do not claim Windows namespace durability or create a production
  qualification path.
- The full suite retains 8 intentional live/network ignored tests.
- Actual v2 artifact writers, broad Session consumers, production filesystem
  qualification, and the predecessor-binary canary remain owned by their
  separate Seeds and were not activated here.
- Seeds were read for acceptance but not edited, closed, synced, or committed,
  per conductor ownership.
