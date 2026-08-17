# audio-graph-67a1 Session Control Plane Plan

Date: 2026-08-16

## Decision and custody boundary

Seed `audio-graph-67a1` is the P0 architectural prerequisite for the bounded
`audio-graph-7e81` Session-semantics activation. Execution is fixed to base
`e64aa4a3aedb7e8839e1cb1e0e4cd01bd4e3de25` on branch
`work/audio-graph-67a1-session-control-plane-wave7c` in the clean worktree
`/home/codeseys/DevBox/audio-graph/.worktrees/67a1-session-control-plane-wave7c`.

[ADR-0038](../../adr/0038-keep-session-control-plane-in-the-flat-artifact-root.md)
is `proposed`. This plan records a future implementation shape; it authorizes
no production code, Seed mutation, workflow action, push, merge, release, or
deployment. A human decider must first accept ADR-0038. The conductor then
assigns a focused child Seed and clean worktree for each serial workstream.
Acceptance itself lands separately as one docs-only commit that atomically
changes the ADR status, its actual acceptance date, and the README row status
and date. It changes no production code or Seed and grants no implementation
authority.

## Fixed implementation contract

The accepted implementation must preserve these decisions without reopening
them inside a code workstream:

- exact validated Session-id bytes map through lowercase unpadded RFC 4648
  Base32 to a bounded, injective key, but only after the production constructor
  applies `sessions::session_id_is_valid`;
- the narrower 128-byte ASCII-safe Sessions validator is the production
  addressability seam; the dormant manifest's broader 255-byte UTF-8 wire
  validity remains unchanged and grants no production path eligibility;
- addressability refusal is content-free and precedes path derivation or I/O,
  and a manifest loaded from a derived path must exactly match the requested
  validated Session id;
- manifest, immutable v1-to-v2 proof, and manifest temporary use the ADR-0038
  role-specific basenames at the existing qualified flat root;
- those three identities are Session-owned while
  `.audio-graph-canonical.lock` is store-owned and global;
- one shared guard linearizes checked open, and an absent manifest receives a
  capability-fixed revalidation rather than unlocked v1 admission: qualified
  Linux/macOS establishes/acquires the global lock and revalidates under its
  shared guard, while unqualified Windows/Other creates nothing and requires
  exact pre/post absence around an unreleased content snapshot;
- one immutable exact proof, not manifest metadata and not a fifth canonical
  stream, is the complete v1-to-v2 provenance scope; its canonical bytes carry
  no digest-derived field, are hashed only after serialization, and supply the
  same digest to the manifest transition fingerprint and proof artifact hash;
- historical Original Session Audio is Present when observed or explicitly
  historical-unknown when not observed, never assigned an invented reason;
- Windows may read compatible state but refuses v2 mutation before any
  coordination, proof, temporary, or manifest side effect; and
- richer lifecycle provenance remains deferred.

The dormant manifest and Session-semantics kernels have no production caller at
this base. No Session has a persisted control-plane migration under this plan,
and neither workstream may describe a compatibility migration as pre-existing
state.

## Versioned transition proof wire specification

This section, rather than ADR-0038, owns the bounded proof wire. The proof is
compact UTF-8 JSON with fields in the fixed order `schema_version`,
`session_id`, `from`, `to`, `idempotency_id`, and `transition_kind`. The floor
values are exactly 1 and 2, the transition kind is exactly
`session_semantics_advance`, and the canonical bytes end at the closing object
delimiter without a trailing newline. No field contains a hash, digest,
fingerprint, content identity, or another value derived from those bytes.

The serializer emits the complete canonical proof before hashing. SHA-256 over
those exact bytes then supplies both the manifest transition fingerprint and
the proof artifact's content SHA-256. Strict decoding rejects unknown members,
including a self-hash member, before persistence.

The required golden fixture uses Session `session-1`, idempotency id
`advance-floor-v2`, and transition kind `session_semantics_advance`. Its exact
143 bytes are:

```json
{"schema_version":1,"session_id":"session-1","from":1,"to":2,"idempotency_id":"advance-floor-v2","transition_kind":"session_semantics_advance"}
```

Their expected digest is
`sha256:1d796c12d556471fbfca4381957a80e82f3139259c6c974a190730fa69c6b0e6`.
The fixture asserts exact bytes, byte length, digest, and both manifest digest
references. Negative variants adding `fingerprint`, `sha256`, `digest`,
`content_sha256`, or any other self-hash member fail strict proof decoding
before persistence.

## Serial TDD workstream 1: shared persistence contract

This workstream must complete and integrate before workstream 2 starts.

### Owned production seams

- `src-tauri/src/persistence/session_artifact_manifest.rs`
- `src-tauri/src/persistence/canonical_durability.rs` only if the existing
  guard surface cannot express race-safe absent-state coordination
- `src-tauri/src/persistence/mod.rs` only for a narrow module/export change
- focused inline tests in those owned modules
- one workstream plan/report pair assigned by the conductor

It does not own Session consumers, `sessions/mod.rs`, canonical stream writers,
Projection Basis/patch activation, frontend, workflows, dependencies, or
Seeds.

### Public-seam RED/GREEN order

1. Start with failing addressability table tests. GREEN makes the production
   control-address constructor call `sessions::session_id_is_valid` before any
   path derivation. It accepts a 128-byte ASCII-safe id and rejects a 129-byte
   ASCII id; non-ASCII and 255-byte ids that remain structurally valid under the
   dormant manifest wire are ineligible for production addressing. Empty,
   case-distinct, hyphen/underscore, and collision-adversarial cases remain
   covered. Every refusal is content-free and proves no path lookup, directory
   read, lock open/create, or control-file I/O occurred.

   A manifest loaded from a derived address must carry the exact requested
   validated Session id. A deterministic requested/manifest mismatch fails
   before proof or artifact content read. GREEN then exposes one opaque control
   key and the three exact identities; the 128-byte maximum basenames fit the
   255-byte component ceiling and ASCII-case aliases fail before I/O.
2. Start with a failing two-Session store test at one root. GREEN selects and
   reopens independent manifests and temporaries while both stores contend on
   the same global coordination entry. Internal ownership classifies
   manifest/proof/temp as Session-owned and the lock as store-owned.
3. Start with failing shared-reader/exclusive-writer race tests for present and
   absent manifests. GREEN exposes one guard-owning checked-read transaction
   that revalidates the selected manifest after shared acquisition and holds
   the guard through a supplied admitted-snapshot closure. Late manifest/lock
   appearance must either be observed under the guard or force typed retry;
   unlocked preflight never returns an admitted floor. For an absent qualified
   Linux/ext4 or macOS/APFS lock, the test requires qualified coordination-entry
   establishment followed by shared acquisition and guarded Session-manifest
   revalidation. A deterministic mutator win between establishment and shared
   acquisition must be observed after the guard is acquired.
4. Start with the failing exact and negative fixtures in
   [Versioned transition proof wire specification](#versioned-transition-proof-wire-specification)
   plus every crash cut. GREEN serializes the fixed digest-free field set,
   hashes only the complete exact bytes, writes that same digest into the
   manifest transition fingerprint and proof artifact content identity, then
   durably installs the proof before manifest v2 CAS. It binds exact length,
   preserves exact retry bytes, and rejects altered, duplicate, unavailable,
   residual, mismatched, or self-hashing proof. No append API exists.
5. Start with failing Windows/Other read-only policy tests. GREEN creates no
   coordination or Session control identity, checks manifest and lock absence,
   constructs the complete content snapshot without returning bytes, then
   rechecks both immediately before return. Deterministic manifest or lock
   appearance at either cut returns typed retry/refusal. The same tests permit
   strict v1/present-compatible reads and refuse proof or manifest v2 mutation
   before global-lock, proof, temp, or manifest creation/change.

Focused commands begin with individual named RED tests and end with:

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud \
  session_artifact_manifest -- --nocapture --test-threads=1
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud \
  canonical_durability -- --nocapture --test-threads=1
```

Stop rather than widen scope if a shared checked-open boundary requires a new
lock identity, a Session capsule, a production qualification shortcut, or a
Windows namespace mutation.

## Serial TDD workstream 2: admission and lifecycle parity

This workstream starts only after workstream 1 is reviewed and integrated.

### Owned production seams

- `src-tauri/src/persistence/session_semantics.rs`
- `src-tauri/src/persistence/session_artifact_manifest.rs` only for consuming
  the integrated contract
- `src-tauri/src/sessions/mod.rs` for bounded inventory/export/delete ownership
  and historical bootstrap
- the narrow existing export/recovery consumer modules discovered and fixed in
  the workstream plan; any broader consumer expansion requires backflow
- focused inline tests in owned modules
- one workstream plan/report pair assigned by the conductor

It does not own a new canonical stream, runtime v2 transcript/basis/patch
writer activation, frontend, workflow, dependency, generated contract, or
Seed changes.

### Public-seam RED/GREEN order

1. Start with failing historical-bootstrap fixtures for present audio, absent
   audio, ambiguous metadata, and a racing manifest. GREEN derives Present
   audio from exact observed bytes and otherwise emits the explicit
   historical-unknown state. If that state requires a manifest wire change,
   add only the reviewed backward-compatible representation and strict tests;
   do not reuse an existing false reason.
2. Start with a failing checked-open integration test. GREEN consumes only the
   guard-owned manifest/proof snapshot from workstream 1, validates reader
   support before content read, treats guarded historical absence as v1, and
   rejects every v2 proof/floor mismatch without consulting canonical or
   legacy content.
3. Start with failing v1-to-v2 admission and crash/retry fixtures. GREEN passes
   the actual `ManifestCasOutcome` to `admitted_session_semantics_floor` and
   accepts only authoritative `ManifestCasOutcome::Accepted` or exact
   `ManifestCasOutcome::AlreadyCompleted`. Both outcomes must carry the strict
   manifest/proof binding from workstream 1. Rejected or durability-indeterminate
   outcomes, a generic durability receipt, proof-only evidence, a caller
   boolean, or a detached manifest cannot admit or preserve v2. Exact
   `AlreadyCompleted` preserves guard-ahead idempotence; no path lowers or
   infers the floor from a v2 artifact.
4. Start with a failing ownership matrix covering inventory, export, permanent
   delete, partial delete retry, recovery, and two co-resident Sessions. GREEN
   includes manifest and available proof in export; deletes manifest/proof/temp
   for only the target Session; reports exact residuals; and never exports or
   deletes the global lock or another Session's controls.
5. Start with failing Windows end-to-end policy fixtures. GREEN reads guarded
   v1 and compatible present state but returns the typed v2 mutation refusal
   before any side effect or content writer activation.

Focused commands begin with individual named RED tests and end with:

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud \
  session_semantics -- --nocapture --test-threads=1
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud \
  session_artifact_manifest -- --nocapture --test-threads=1
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud \
  sessions::tests -- --nocapture --test-threads=1
```

Stop and return to ADR review if parity requires a fifth stream, a per-Session
directory or lock, fabricated historical evidence, a weakened Windows result,
generic receipt or proof-only floor admission, or lifecycle provenance beyond
the one floor transition.

## Common gates and review

Each code workstream records exact RED and GREEN output, receives independent
Standards and Spec review on a stable commit, and runs:

```text
cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml \
  --lib --tests --no-default-features --features cloud
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud -- --test-threads=1
cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml \
  --lib --tests --no-default-features --features cloud -- -D warnings
cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check
bun run verify:fast
bun run verify:contracts
bun scripts/check-docs-secret-hygiene.mjs
git diff --check <exact-base>...HEAD
```

Betterleaks scans the exact owned footprint with redaction. The report includes
an exact base-range name/status footprint and runtime-activation search. The
integrator re-runs gates after serial fan-in; a branch-green result alone is not
acceptance.

## Rollback, findings, and open questions

Before runtime activation, each dedicated branch is independently reversible.
Workstream 1 may be omitted or reverted as one dormant persistence-contract
unit; workstream 2 cannot land without it. No migration or v2 write begins in
either plan merely because its tests pass.

Because the current kernels are dormant, rollback has no persisted migration
to reverse and no production caller to detach. If discovery finds either claim
false at an implementation base, planning stops and backflows the new evidence
before code changes.

Current authoring finding: the existing historical unavailable-reason enum has
no truthful historical-unknown Original Audio value. The second workstream must
prove the smallest backward-compatible representation before bootstrap.

Open questions for ADR authoring: none. Human acceptance remains deliberately
open, and the conductor must assign the two child Seed ids and exact clean
worktrees only after acceptance.

## ADR authoring artifact report

### Changed documentation

- Added proposed Extended-tier MADR 3.0 ADR-0038 with four real options,
  explicit negative consequences, typed relationships, compliance assertions,
  confirmation, reversal condition, and a human acceptance gate.
- Rebuilt the ADR index view for 38 on-disk ADRs by adding the sorted ADR-0038
  row and reference link.
- Added the exact-base custody record and this two-workstream serial TDD plan.
- Changed no production code, test, Seed, workflow, dependency, generated
  contract, or other path.

### Authoring gates and real results

- `git diff --check`: exit 0; no output.
- ADR index and relative-link Bun check: exit 0;
  `ADR_INDEX_OK files=38 rows=38 refs=38` and
  `RELATIVE_LINKS_OK checked=5`.
- MADR structure and required-decision-term check: exit 0; proposed status,
  title, all Extended-tier headings, Confirmation, and 36 required-term
  matches were present.
- `bun run verify:contracts`: exit 0; audio source, provider registry, Session
  data movement, endpoint credential routing, and Speech Span Revision
  generated contracts all reported current.
- Final Markdown structure check: exit 0;
  `MARKDOWN_STRUCTURE_OK files=4`.
- `bun scripts/check-docs-secret-hygiene.mjs`: exit 0;
  `docs/Seeds secret hygiene scan passed: 0 findings`.
- Betterleaks over the exact four owned files: exit 0; approximately 40.17 KB
  scanned and `no leaks found`.

Correction round 1 reran the same authoring gates after resolving the review
findings:

- Markdown/index/link checks: exit 0;
  `MARKDOWN_STRUCTURE_OK files=4`,
  `ADR_INDEX_OK files=38 rows=38 refs=38`, and
  `RELATIVE_LINKS_OK checked=5`.
- Independent golden-proof check: exit 0; the documented canonical fixture is
  exactly 143 bytes and hashes to
  `1d796c12d556471fbfca4381957a80e82f3139259c6c974a190730fa69c6b0e6`.
- `bun run verify:contracts`: exit 0; all five generated contracts again
  reported current.
- `bun scripts/check-docs-secret-hygiene.mjs`: exit 0 with 0 findings.
- Betterleaks: exit 0; approximately 47.10 KB scanned and `no leaks found`.
- `git diff --check`: exit 0; no output.

Correction round 2 reran the authoring gates after moving the wire detail and
reconciling Session-id validation:

- Markdown/index/link-and-anchor checks: exit 0;
  `MARKDOWN_STRUCTURE_OK files=4`,
  `ADR_INDEX_OK files=38 rows=38 refs=38`, and
  `RELATIVE_LINKS_OK checked=7`.
- ADR wire-boundary search: exit 0 with
  `ADR_WIRE_SPEC_BOUNDARY_OK`; detailed proof field, byte, delimiter, and
  golden literals occur only in this plan.
- Independent plan-golden check: exit 0; 143 bytes and SHA-256
  `1d796c12d556471fbfca4381957a80e82f3139259c6c974a190730fa69c6b0e6`.
- `bun run verify:contracts`: exit 0; all five generated contracts reported
  current.
- Docs/Seeds secret hygiene: exit 0 with 0 findings.
- Betterleaks: exit 0; approximately 53.44 KB scanned and `no leaks found`.
- `git diff --check`: exit 0; no output.

The exact staged footprint and base-range diff checks run after this evidence
section is added and are reported in the handoff.

### Findings and open questions

The only implementation finding is the missing truthful historical-unknown
Original Audio wire state recorded above. It is intentionally assigned to
workstream 2 and is not guessed in this decision-only branch.

Correction-round review also found and resolved two constructibility defects
in the proposed documentation. The proof wire is now explicitly digest-free
and acyclic, with exact golden bytes/hash plus negative self-hash fixtures.
Admission now names the actual authoritative `ManifestCasOutcome::Accepted`
and exact `ManifestCasOutcome::AlreadyCompleted` inputs to
`admitted_session_semantics_floor` and excludes generic or proof-only evidence.

The same review fixed two evidence/lifecycle ambiguities. Absent-store handling
is now deterministic by capability rather than an implementer choice, with
guarded qualified admission and non-mutating pre/post read-only admission. A
future acceptance must atomically update status and actual date in both ADR and
index in a separate docs-only, non-authorizing commit.

Correction round 2 removes proof-wire schema from ADR-0038 and makes this plan's
wire-specification section its sole detailed authority. It also reconciles the
narrow production Sessions addressability contract with the broader dormant
manifest wire validator: broad wire validity remains intact, while path
derivation refuses ineligible ids content-free before I/O and exact loaded-id
matching prevents cross-Session selection. The current dormant/no-migration
evidence boundary is now explicit.

No ADR-authoring question remains open. Human acceptance of ADR-0038 and later
assignment of two implementation child Seeds remain required external actions.
