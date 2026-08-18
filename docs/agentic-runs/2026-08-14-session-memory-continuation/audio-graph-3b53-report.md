# audio-graph-3b53 Session Control Contract Report

Date: 2026-08-16

## Custody and result

- Seed: `audio-graph-3b53`.
- Exact base: `b5145b2b630a38df7065905263139575b44ead7e`.
- Branch: `work/audio-graph-3b53-session-control-contract-wave7c`.
- Worktree:
  `/home/codeseys/DevBox/audio-graph/.worktrees/3b53-session-control-contract-wave7c`.
- Initial implementation tip before review correction: `8586763`.
- Review-correction source tip before this report update: `b82380d`.
- Round-2 durable-handoff source tip: `3ddeb0afbb602a533295fcb06910d0bd53464ff1`.
- No Seed, workflow, frontend, dependency, lockfile, writer, projection,
  consumer, generated-contract, or integration-branch change was made.

The accepted ADR-0038 shared-persistence workstream is implemented but remains
runtime-dark. This work does not call `admitted_session_semantics_floor`, does
not authorize v2 content admission, and does not add a fifth stream.
The Standards/Spec BLOCK findings described below are corrected locally; this
report makes no SHIP or acceptance claim.

## What changed

### Production Session control addressing

`SessionArtifactManifestStore::for_session` validates through the existing
128-byte ASCII-safe Sessions validator before root conversion, path derivation,
qualification, or filesystem I/O. It derives a lowercase, unpadded RFC 4648
Base32 key and the exact flat-root identities for the Session manifest,
manifest temporary, and immutable v1-to-v2 provenance proof. Every Session
uses the same store-owned `.audio-graph-canonical.lock`.

The dormant manifest wire still accepts its broader 255-byte UTF-8 Session id.
That broader wire is not production-addressable: 129-byte, non-ASCII, and
broad-wire-only ids refuse before control-path I/O. A loaded manifest must
exactly match the requested validated Session id. The old explicit-root
constructors are now available only to crate tests; production mutation must
use the qualified per-Session constructor.

Tests cover the 128/129 boundary, broad-wire-only ids, case-distinct ids,
requested/manifest mismatch, two independent Session heads, exact identities,
and no-I/O refusal.

### Checked reads under one global guard

`checked_read` owns the coordination boundary through the caller's complete
snapshot callback.

- Qualified Linux/macOS establishes the qualified global coordination entry
  when absent, releases the establishment guard, acquires shared, then reloads
  and revalidates the selected Session manifest under that shared guard.
- A writer winning between establishment and shared acquisition is observed.
- Unqualified Windows/Other never creates state. When both the Session manifest
  and global lock are absent, exact transient detection is impossible through
  pathname observations alone, so the read returns typed
  `UncoordinatedAbsence` before invoking the callback. This also fails closed
  for an appearance-and-removal ABA that is no longer observable at entry.
- A present Windows/Other read uses the existing global shared guard.

No per-Session lock or directory exists.

### Digest-free immutable proof

`SessionSemanticsTransitionProofV1` is the versioned six-field proof defined by
the sibling plan. It rejects unknown members, including any attempted self-hash
or content-digest field. Canonical compact JSON is serialized first; SHA-256 is
computed from those complete bytes afterward.

The named golden (`session-1`, `advance-floor-v2`) is exactly 143 bytes and:

```text
sha256:1d796c12d556471fbfca4381957a80e82f3139259c6c974a190730fa69c6b0e6
```

Other valid idempotency-id lengths naturally produce their own canonical byte
length; the 143-byte assertion belongs to the exact golden fixture.

The exclusive durability guard now has crate-public immutable exact-create or
reconcile operations. A missing proof is created at its final identity with
owner-only protection, complete write, flush, file sync, and parent sync. An
exact regular proof only re-establishes those barriers. A recovered prefix is
completed through its retained handle after that handle is revalidated against
the pathname; only the missing expected suffix is written, and file plus parent
barriers are re-established. Longer, non-prefix, different, special, symlink, and
ASCII-case-alias collisions refuse without replacement. No proof temporary
identity was added.

Two strict-prefix recovery entry points exist, and only one of them carries the
round-2 contract:

- `create_or_reconcile_immutable_exact_with_authentication` is the primary
  install path. It hardcodes `minimum_recoverable_prefix_len = 0`
  (`canonical_durability.rs:1435`, declared at `:1423`), so any regular strict prefix, including the
  empty post-create final, is recoverable once the separately staged exact
  manifest temporary authenticates it under the same exclusive guard. The round-1
  rule — that a prefix was recoverable only once it reached the complete
  canonical proof-specific identity prefix through the idempotency field — is no
  longer live on this path. Earlier revisions of this report stated that round-1
  rule as the shipped contract; that statement was stale from round 2 onward.
- `create_or_reconcile_immutable_exact_with_identity_prefix` remains a second
  recovery entry point and is still reached in production, not only in tests, on
  the `AlreadyCompleted` retry branch of
  `advance_session_semantics_v1_to_v2` (`session_artifact_manifest.rs:1233-1241`).
  That branch still passes
  `SessionSemanticsTransitionProofV1::recovery_identity_prefix_len` (`:379`),
  which is a byte-offset search for the `,"transition_kind":` marker — the
  round-1 prefix-length heuristic. The 3b53 plan's round-2 section states that
  the manifest temporary is "the durable authentication record, not a
  prefix-length heuristic", and this wave's commit-state states the same
  replacement; both are accurate for the install path only and overstate the
  replacement for the retry path. That second entry point carries a two-line
  rustdoc contract (`canonical_durability.rs:1398-1399`) but appears in no
  round-2 contract prose, so it is disclosed
  here rather than left implicit.

The cross-proof fixtures below enter at generation 0, so the preparation is
always `Install` and they exercise only the authenticated path. No fixture drives
the `AlreadyCompleted` branch with a partial proof, and this report does not
claim whether an installed head can coexist with an incomplete proof; the second
entry point's production reachability with a strict prefix is unestablished
either way and is left to review.

### Proof-before-manifest transition

`ManifestWriteTransaction::advance_session_semantics_v1_to_v2` binds the proof
Session and idempotency id to the production address and candidate. It replaces
the candidate's `SessionProvenanceEvents.managed_identity` with the exact
derived per-Session proof identity, installs the independently derived digest
and byte length into both manifest references. The acceptance/reopen test
asserts that the persisted manifest retains that identity.

The operation completes all pure candidate, current-head, generation, and
idempotency checks under the existing exclusive transaction before proof
mutation. Only a successful prepared transition durably ensures the proof and
commits manifest CAS. Generic CAS on an addressed store returns
`TransitionProofRequired` for v2, leaving the proof-owning operation as the only
addressed v1-to-v2 path. Dormant unaddressed manifest tests retain their broader
non-production CAS behavior.

The public result is the actual authoritative `ManifestCasOutcome`:

- new transition: `Accepted` with the installed manifest;
- exact completed retry: `AlreadyCompleted` at the existing generation;
- proof or CAS refusal: the original typed `Rejected` result; and
- any barrier uncertainty: the original `DurabilityIndeterminate` result.

A stale generation/head/idempotency/candidate rejection creates no proof,
manifest, or temporary state; the correct transition remains possible. Once
pure preflight succeeds, a proof orphan plus manifest temporary after manifest
file-sync uncertainty is intentionally retained and recoverable; the recovery
CAS completes on retry. That retention claim is established only for residues
whose bytes are exactly equal to, or a strict prefix of, the expected proof —
which is every residue the fault injector can leave, because it cuts before or
during the write.

One unexecuted case is not covered by that claim and is recorded here rather than
resolved. The proof write crosses no barrier between `write_all` and `sync_all`
(`canonical_durability.rs:1709-1740`): write, flush, owner-only protection, and
handle identity revalidation all precede the single `sync_all`. A reviewer
therefore raised the possibility of a host crash inside that window that persists
the final file length while leaving part of the tail unwritten, producing a
same-length residue with zero-filled trailing bytes. Both
`preflight_immutable_exact` and the create/reconcile path classify an existing
entry through `expected.starts_with(&observed)`, so a same-length non-equal
residue is `ImmutableExactConflict`; immutable exact never truncates or replaces a
conflicting entry, so that classification would repeat on every retry with no
reclaim path. This is a code-shape argument, rated PLAUSIBLE and not executed: no
fixture reproduces it, and whether a qualified ext4/APFS host can expose a
zero-filled same-length tail through this exact sequence is unresolved. Deciding
it needs a real crash or block-device fixture, not another injector cut, and the
retained-residue claim above should be read as bounded to prefix-shaped residues
until that evidence exists.

Proof conflict or proof indeterminate prevents manifest head and temporary
mutation. The operation preserves the candidate's validated
truthful Original Audio unavailable/present evidence; it does not invent
retained audio.

Windows/Other addressed mutation refuses at `begin_write` before manifest,
temporary, provenance, or global-lock creation.

## Serial RED to GREEN evidence

### Slice 1: addressing

The first public-constructor test failed to compile with E0599 because
`SessionArtifactManifestStore::for_session` did not exist. Separate public-seam
REDs then showed missing `ManifestLoadError::SessionMismatch` and missing
`qualified_for_test_session`. After the minimal implementation:

```text
production_session_store_derives_exact_portable_control_identities_without_io: 1 passed
production_session_load_refuses_requested_manifest_mismatch: 1 passed
two_qualified_session_stores_persist_independent_manifest_heads: 1 passed
session_artifact_manifest module at checkpoint: 31 passed, 0 failed
```

Commit: `d8030a4 feat(audio-graph-3b53): address Session control stores`.

### Slice 2: checked reads

The public checked-read RED failed with E0599 for missing `checked_read`.
Windows/Other race RED also lacked `for_test_session_platform` and
`CheckedManifestReadError::StateChanged`; the writer-wins RED lacked the
establishment hook. That first checked-read contract was subsequently
superseded by the correction-round fail-closed `UncoordinatedAbsence` behavior
documented below.

```text
checked_read_ filter: 5 passed, 0 failed
session_artifact_manifest: 36 passed, 0 failed, 8.89s
canonical_durability: 47 passed, 0 failed, 3.27s
```

Commit: `78a98b2 feat(audio-graph-3b53): guard Session control reads`.

### Slice 3: immutable proof

The true-partial RED failed with E0599 for both immutable-create methods and
the missing `ImmutableExactReconcile` mutation. GREEN left a real non-empty
strict prefix at the final proof identity with `DurabilityIndeterminate(Write)`,
dropped the guard, then used a fresh guard to complete exactly one record.

The wire RED failed with E0433 for the missing proof and proof-error types.
GREEN proved the exact 143-byte golden and digest plus unknown/self-hash
rejection.

```text
immutable_exact_create filter: 3 passed, 0 failed
golden proof fixture: 1 passed, 0 failed
session_artifact_manifest: 37 passed, 0 failed, 8.72s
canonical_durability: 50 passed, 0 failed, 3.44s
```

The immutable tests include create-new, Write, Flush, Protect, FileSync, and
ParentSync cuts; exact orphan reuse after every indeterminate cut; repeated
exact reconcile; non-prefix/longer conflicts; directory, FIFO, symlink, and
case-alias refusal.

Commit: `2004236 feat(audio-graph-3b53): persist immutable transition proof`.

### Slice 4: proof before CAS

The public transition RED failed twice with E0599 because
`advance_session_semantics_v1_to_v2` did not exist. The primary GREEN returned
actual `Accepted`, installed both proof digest references, persisted proof
first, and returned exact `AlreadyCompleted` on retry.

An initial follow-up assertion incorrectly assumed every proof was 143 bytes;
the test observed correct canonical lengths of 144 and 146 for different ids.
The test was corrected to compare each proof with its own canonical bytes while
retaining the exact 143-byte golden requirement.

```text
proof_ filter: 6 passed, 0 failed
session_artifact_manifest: 41 passed, 0 failed, 8.62s
canonical_durability: 50 passed, 0 failed, 3.24s
```

Commit: `866b03a feat(audio-graph-3b53): order proof before manifest CAS`.

### Production surface hardening

Gating all three legacy members initially exposed one compile RED:
`canonical_log` uses the inventory-only `internal_identities` accessor in
non-test code. That accessor was retained as crate-public; only the root-only
constructors became test-only. Production and test checks then passed.

Commit: `8586763 refactor(audio-graph-3b53): gate legacy root constructors`.

### Correction round 1: Standards/Spec BLOCK findings

Five public-seam REDs reproduced the review findings before the correction:

- exact provenance identity: the accepted/reopened manifest retained
  `streams/session-provenance.jsonl` instead of the derived
  `.audio-graph-session-onsxg43jn5xc2mi-v1-v2.provenance`;
- addressed generic CAS bypass: the RED did not compile because
  `ManifestCasRejection::TransitionProofRequired` did not exist;
- stale-generation preflight: the operation returned the expected generation
  conflict but had already created the provenance proof;
- cross-proof partial recovery: proof B returned `Accepted` by completing proof
  A's partial file because their early bytes matched; and
- unqualified absent ABA: the RED did not compile because
  `CheckedManifestReadError::UncoordinatedAbsence` did not exist.

GREEN binds and reopens the exact derived identity; refuses addressed v2 generic
CAS; prepares all pure CAS checks before proof mutation; requires a complete
proof-specific identity prefix before strict-prefix recovery; and refuses an
unqualified absent read before invoking its callback. The cross-proof test
drops the first transaction before attempting proof B, then drops that
transaction and proves a freshly opened exact proof A transaction converges.
The ABA regression constructs transient manifest and lock appearances, removes
them, and proves the otherwise-unobservable absent state cannot return success.

```text
exact identity: 1 passed, 0 failed
addressed CAS bypass: 1 passed, 0 failed
stale preflight/no mutation: 1 passed, 0 failed
cross-proof restart isolation: 1 passed, 0 failed
unqualified absent ABA refusal: 1 passed, 0 failed
session_artifact_manifest: 44 passed, 0 failed, 8.34s
canonical_durability: 50 passed, 0 failed, 3.18s
```

Commit: `b82380d fix(audio-graph-3b53): close Session proof bypasses`.

### Correction round 2: authenticated short-proof recovery handoff

Reviewers correctly BLOCKed the round-1 prefix threshold: a real crash after
`create_new` but before the first write leaves an empty fixed proof final, and a
real short write can leave fewer bytes than the idempotency boundary. The fault
injector had hidden that defect by forcing its partial write through the
threshold.

The round-2 order is now:

1. complete pure candidate/head/generation/idempotency preparation;
2. reject a provable proof collision through a read-only preflight;
3. durably stage the exact prepared manifest bytes at the existing per-Session
   manifest temporary, including file and parent barriers;
4. validate that exact temporary's bytes and open-handle identity under the
   same exclusive guard;
5. use it to authenticate recovery of the same immutable proof from empty or
   any regular strict prefix;
6. establish proof file and parent durability; and
7. have the existing recovery CAS consume that exact temporary, reopen the
   installed manifest, and only then return authoritative `Accepted` or exact
   `AlreadyCompleted`.

No new path, proof temporary, lock, directory, dependency, caller, or runtime
surface was introduced. A different proof cannot claim the first proof's
staged intent: empty, one-byte, sub-identity, and near-complete prefix fixtures
all preserve both first-proof residues unchanged before exact retry. The
unused `_after_snapshot` checked-read test callback was removed.

RED evidence:

```text
empty post-create proof: retry was not Accepted; 0 passed, 1 failed, exit 101
one-byte fixture: injector left 78 bytes instead of 1; 0 passed, 1 failed, exit 101
intent CreateNew cut: fault was ignored; 0 passed, 1 failed, exit 101
manifest ParentSync cut: fault was ignored; 0 passed, 1 failed, exit 101
```

GREEN evidence at source commit `3ddeb0a`:

```text
exact_transition_restarts_after_post_create_empty_proof_final: 1 passed
exact_transition_restarts_after_one_byte_proof_final: 1 passed
transition_intent_stage_faults_are_honest_and_exact_retry_converges: 1 passed
every_strict_proof_prefix_is_bound_to_its_exact_durable_intent: 1 passed
manifest_cas_consumes_staged_intent_and_restarts_every_install_cut: 1 passed
session_artifact_manifest: 49 passed, 0 failed, 8.99s
canonical_durability: 50 passed, 0 failed, 3.25s
rustfmt check: passed after formatting
git diff --check: passed
```

The manifest CAS cut test proves the first attempt remains indeterminate at
FileSync, Rename, and post-rename ParentSync cuts. Fresh retry returns
`Accepted` only for pre-install residue and exact `AlreadyCompleted` when the
head was installed but its parent barrier was uncertain. No first attempt is
converted to success.

Commit: `3ddeb0a fix(audio-graph-3b53): authenticate partial proof recovery`.

### Round-2 regate: strict Clippy on the round-2 source

The exact next command recorded in the round-2 handoff failed on its first run.
`create_or_reconcile_immutable_exact_inner` reached eight parameters when the
authentication pair was threaded through it, one over the lint's limit:

```text
error: this function has too many arguments (8/7)
    --> src/persistence/canonical_durability.rs:1496:5
error: could not compile `audio-graph` (lib) due to 1 previous error
error: could not compile `audio-graph` (lib test) due to 1 previous error
```

The fix is the `#[allow(clippy::too_many_arguments)]` idiom that seven sibling
functions in this same file already carry, including `install_snapshot_inner`. An
arity refactor was rejected: it would have rippled into
`session_artifact_manifest.rs` call sites for no durability benefit. The
misplaced rustdoc on `preflight_immutable_exact` was repaired in the same pass —
it described create/reconcile/complete behaviour that function never performs, so
that text moved to the inner function it actually documents and the preflight now
documents its own classification-only contract, including that it opens the
existing entry read/write for identity revalidation.

```text
cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 26.12s
  exit 0
cargo +1.95.0 test --locked --no-default-features --features cloud --lib persistence -- --test-threads=1
  233 passed; 0 failed; 0 ignored; 1498 filtered out; 19.93s; exit 0
cargo +1.95.0 fmt --all -- --check
  exit 0
```

Only those three gates ran in this pass. Every other broad gate listed below
remains outstanding for this source state.

### Correction round 3: a refused proof create after intent staging

The checklist item "proof CreateNew classification after durable intent staging"
was a real defect, not a review worry. `advance_session_semantics_v1_to_v2_inner`
durably stages the manifest intent temporary — file and parent barriers, continuing
only on `Accepted` (`session_artifact_manifest.rs:1175,1185`) — and then forwarded
any proof refusal as `Durability(...)`, whose enum-level contract at
`canonical_durability.rs:404` is that nothing was staged. One EMFILE, ENOSPC,
EDQUOT, or EACCES on the provenance `create_new` therefore told the caller
"nothing happened" while leaving a durable orphan temporary and returning no
recovery key. A caller that believed the outcome dropped the candidate, and every
later `compare_and_swap` (`:984`, `resume_temporary=false`) then refused with
`SnapshotTempAlreadyExists` forever, because `compare_and_swap_recovery` is
`pub(crate)` (`:1302`) and nothing in the durability substrate removes a staged
temporary outside test code.

No existing test could see it: `proof_fault` was only ever injected as `Write` or
`PostCreate`, while `intent_fault` swept all seven stages.

The classification is now truthful. `ManifestCasRejection` gained one variant
(`:538`) that reports the inner refusal verbatim and carries the staged intent's
`CanonicalRecoveryKey`; the proof arm selects it only when this transaction
actually accepted a staged intent, so the `AlreadyCompleted` preparation path and
every pre-staging refusal keep plain `Durability` (`:1287`). `Durability` itself
now documents that nothing survives it (`:518`), and the durability enum's own
doc scopes its pre-mutation claim to the single requested operation and names the
composed caller's obligation (`canonical_durability.rs:404`). Shape (a) was chosen
over reusing `DurabilityIndeterminate`: the proof create definitively did not
happen, and calling a proven refusal indeterminate would replace one lie with a
milder one.

RED evidence, with the variant declared but unwired so the failure is behavioural
rather than a compile error:

```text
thread '...refused_proof_create_after_durable_intent_is_never_reported_as_unstaged'
panicked at src/persistence/session_artifact_manifest.rs:3575:22:
post-staging proof refusal misclassified as
Rejected(Durability(IoFailedBeforeMutation { stage: CreateNew, kind: Other, raw_os_error: None }))

thread '...transition_proof_stage_faults_are_honest_and_exact_retry_converges'
panicked at src/persistence/session_artifact_manifest.rs:3657:30:
refused proof create misclassified as
Rejected(Durability(IoFailedBeforeMutation { stage: CreateNew, kind: Other, raw_os_error: None }))

test result: FAILED. 0 passed; 2 failed; 1731 filtered out; 0.02s
```

Those two panic locations are verbatim from the pre-fix source and do not resolve
against the current file. Do not treat any line number in this report as
load-bearing without opening it: review of `58b1e3a` found that an earlier draft
of this paragraph claimed every other citation had been re-derived, and that claim
was FALSE — seven anchors were stale, because each round of edits shifts every
citation below its insertion point. Round-5 review of `9e8b9a9` then found the
scoped claim that replaced it false as well, for one anchor inside its own set,
so hand verification is retired: every line anchor in the round-3, round-4, and
round-5 sections is enumerated in the round-5 table with the symbol or code
expected at that line, and each expectation was machine-checked against the
post-edit sources. The only exclusions are the two disclaimed panic locations
above and the anchors the round-5 narrative quotes as the stale values being
corrected. Anchors elsewhere in this report carry no such guarantee. Prefer the
symbol name over the number wherever both appear. The
staged-temporary and absent-proof assertions preceding those panics already held,
so the RED is the classification alone.

Two tests were added. `refused_proof_create_after_durable_intent_is_never_reported_as_unstaged`
(`:3640`) pins the truthful variant, asserts the carried key equals
`recovery_key(&proof_digest)`, then proves the consequence end to end: the
temporary survives, a plain `compare_and_swap` of an unrelated v1 candidate is
refused with `SnapshotTempAlreadyExists`, and the one reachable escape — a
byte-exact replay of the named candidate and proof through the public
`advance_session_semantics_v1_to_v2` — installs and clears the temporary.
`transition_proof_stage_faults_are_honest_and_exact_retry_converges` (`:3721`)
adds the previously empty proof-stage fault table: `CreateNew` is the new
post-staging refusal with the proof absent, and `Flush`, `ProtectTemp`,
`FileSync`, and `ParentSync` are each indeterminate at their own stage with the
complete unsynced proof on disk and the same recovery key, with exact retry
converging in all five cases.

```text
cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.48s; exit 0
cargo +1.95.0 test --locked --no-default-features --features cloud --lib persistence -- --test-threads=1
  235 passed; 0 failed; 0 ignored; 1498 filtered out; 18.99s; exit 0
git diff --name-only b5145b2b630a38df7065905263139575b44ead7e | wc -l
  5
```

Two residues are named, not fixed. First, abandoning the orphaned temporary is
still unreachable: resume is public, but no public path removes a staged
temporary, and the durability substrate has no removal primitive at all, so a
caller that wants to give up on the transition entirely still cannot use
`compare_and_swap`. Adding a durable abandon means a new namespace-mutating
operation and a public entry point, which is a separate Seed. Second, the same
lie survives one step later in a different shape: a `manifest_fault` of
`CreateNew` returns `Rejected(Durability(IoFailedBeforeMutation))` from the
install after both the intent temporary and the proof are durable, and that stage
is uncovered too — the existing install-cut table only exercises `FileSync`,
`Rename`, and `ParentSync`. That reclassification was left out of this pass as
scope this correction was not asked to take.

### Correction round 4: B4 re-key attempted, then REVERTED as unsafe

B4 is real and remains OPEN. The transition-proof gate in
`prepare_compare_and_swap` keys on the candidate's floor rather than on whether
the call performs the advance, so an addressed Session that reaches V2 can never
record a later generation. That is a liveness wedge.

Re-keying the gate on "this call advances the head" was implemented, reviewed,
and **reverted**. Adversarial review demonstrated that relaxing the gate for an
already-advanced head opens a forgery path that was unreachable before it:

- With the relaxed gate, the generic `compare_and_swap` accepted a V2 candidate
  carrying a **forged** `SessionProvenanceEvents` entry — generation 2 installed
  with `transition.fingerprint = sha256:ffff...` and
  `managed_identity = "attacker/not-the-proof.jsonl"`, while the real durable
  proof stayed untouched at the control identity. The forged manifest reloaded
  cleanly through `load_manifest_file`.
- Pre-fix that path was unreachable, because every V2 candidate through the
  generic CAS was refused by the gate being relaxed.

An earlier draft of this section justified the re-key by asserting that
`validate_v2_session_provenance` forces a V2 candidate's `transition.fingerprint`
to equal the durable proof digest. **That assertion was false.** The function
(`session_artifact_manifest.rs:1840`, comparison at `:1868`) only compares the candidate's own
`transition.fingerprint` against its own provenance entry's `content.sha256` —
internal self-consistency with no reference to any durable proof record. A caller
controlling both fields satisfies it trivially. The false claim was the entire
stated basis for believing the re-key safe, and the forgery probe is its
counterexample.

**Re-keying is therefore blocked on a binding that does not exist yet**, owned by
seed `audio-graph-68a1` (and the B3 substrate residuals by `audio-graph-3cf2`): the
candidate's provenance entry must be provably tied to the durable proof record.
Until that binding lands, relaxing this gate trades a liveness wedge for a
forgery hole, which is strictly worse. The wedge stays, and
`committed_v2_head_refuses_later_generations_until_proof_binding_exists`
(`session_artifact_manifest.rs:3209`) pins it as a deliberate refusal with the
reason recorded in the test's own contract. When the binding exists, that test
should be inverted rather than deleted.

Two further corrections to earlier drafts of this section, both from the same
review:

- It claimed "lock-owned quarantine recovery on an advanced Session is unwedged
  too for a V2-floor candidate." Disproved for every shape against a V2 head: a
  V1 prepared or completed candidate is a floor regression; a V2 prepared
  candidate fails `InvalidV2SessionProvenance(TransitionNotCompleted)`; a V2
  completed candidate fails `CompletionRequiresPrepared`. V2 validation demands
  `Completed` while quarantine prepare demands `Prepared`, so the shapes are
  mutually exclusive.
- **Unrecorded residue, now recorded:** quarantine recovery is therefore entirely
  unreachable on a V2 Session, before and after this round. The
  `SessionArtifactManifestV1::candidate` V1 hardcode (`:252`) is genuinely left in
  place and was already named; the consequence — that the path is fully closed on
  an advanced Session — was not, and is the part that matters.

### Correction round 5: the anchor guarantee itself was stale

Round-5 review of `9e8b9a9` (fresh Standards and Spec reviewers plus a dedicated
B4-revert auditor, every blocking finding independently adversarially verified)
confirmed the production behaviour, the gates, and the B4 revert clean, and
blocked on one defect: the scoped verification guarantee written in round 4 was
itself false for one anchor in its own set. Removing the three orphaned comment
lines in round 4 shifted everything below them up by three lines, so the
fingerprint comparison cited as `:1871` sits at `:1868`; the sibling anchors in
the same sentence were corrected, this one was missed. The same shift staled the
B4 gate comment's `(:1871)` and the `Durability` rustdoc's `(:1547)` (the return
site is `:1544`), and review also caught that rustdoc routing B3's install-stage
residual to `audio-graph-7e18` — the round-3 review ticket — instead of
`audio-graph-3cf2`, which owns it. A mechanical sweep during the fix then caught
three more stale anchors in the round-3 section that five review rounds had not:
`:529` (mid-rustdoc; the variant it names is declared at `:538`), `:1271` (a
closing brace; the pre-staging `Durability` arm is `:1287`), and `:1130,1176`
(the production intent staging call and its `Accepted` continuation are
`:1175,1185`).

All source-file fixes were intra-line, so no line in either source file moved.
Every line anchor in the round-3, round-4, and round-5 sections is enumerated
below with the content expected at that line; each row was machine-checked
against the current sources, and the sweep fails on any anchor in those sections
that the table does not enumerate. Excluded: the two panic locations disclaimed
inline in round 3, and the stale values this section quotes while correcting
them.

| anchor | expected at that line |
| --- | --- |
| `session_artifact_manifest.rs:252` | `session_semantics_version: SessionSemanticsVersion::V1,` |
| `session_artifact_manifest.rs:518` | `Durability` rustdoc: `For every refusal raised BEFORE this transaction staged anything, nothing` |
| `session_artifact_manifest.rs:538` | `TransitionProofRefusedAfterIntentStaged {` |
| `session_artifact_manifest.rs:984` | `pub fn compare_and_swap(` |
| `session_artifact_manifest.rs:1175` | `self.guard.stage_snapshot_temporary(` under `#[cfg(not(test))]` |
| `session_artifact_manifest.rs:1185` | `CanonicalDurabilityOutcome::Accepted(_) => Some(*recovery_key),` |
| `session_artifact_manifest.rs:1287` | `None => ManifestCasRejection::Durability(rejection),` |
| `session_artifact_manifest.rs:1302` | `pub(crate) fn compare_and_swap_recovery(` |
| `session_artifact_manifest.rs:1544` | `ManifestCasOutcome::Rejected(ManifestCasRejection::Durability(rejection))` |
| `session_artifact_manifest.rs:1840` | `pub(crate) fn validate_v2_session_provenance(` |
| `session_artifact_manifest.rs:1868` | `if manifest.transition.fingerprint != content.sha256 {` |
| `session_artifact_manifest.rs:3209` | `fn committed_v2_head_refuses_later_generations_until_proof_binding_exists()` |
| `session_artifact_manifest.rs:3640` | `fn refused_proof_create_after_durable_intent_is_never_reported_as_unstaged()` |
| `session_artifact_manifest.rs:3721` | `fn transition_proof_stage_faults_are_honest_and_exact_retry_converges()` |
| `canonical_durability.rs:404` | `A refusal proven before the one requested operation mutated canonical bytes` |

## Pre-round-2 broad gates

The following broad results were recorded before round 2. They are retained as
history and are not evidence for source commit `3ddeb0a`; every broad gate must
be rerun before review can clear the round-2 checkpoint.

### Rust

- Locked production cloud library check: exit 0, 14.87s.
- Locked cloud library plus tests check: exit 0, 25.78s.
- Correction locked cloud library plus tests check: exit 0, 8.88s.
- Strict Clippy (`--lib --tests ... -- -D warnings`): the first run found three
  redundant checked-read test closures; after replacing them with the result
  constructor, exit 0 in 32.93s.
- Rustfmt `--all -- --check`: exit 0.
- Correction strict Clippy (`--lib --tests ... -- -D warnings`): exit 0,
  16.53s after boxing the prepared CAS value and test-gating the legacy
  unbound immutable helper.
- Full serialized cloud library after correction: 1,718 passed, 0 failed,
  8 ignored, 55.95s.
- Focused manifest and durability suites after correction: 44/44 and 50/50
  passed.

### Windows object probes

The whole-repository Windows production and test checks both stopped before
AudioGraph compilation in the transitive `ring` build script because this
Linux host has no MSVC `lib.exe`; both returned 101. No native-Windows or full
repository cross-link claim is made.

An ignored worktree-local minimal Cargo probe then imported the actual changed
`canonical_durability.rs`, `session_artifact_manifest.rs`, and sibling
`session_semantics.rs` with their direct dependencies. Both pinned offline
checks passed for `x86_64-pc-windows-msvc`:

```text
production --lib: exit 0, 23.54s
cfg(test) --tests: exit 0, 0.80s
correction production --lib: exit 0, 32.15s
correction cfg(test) --tests: exit 0, 1.02s
```

The correction probe suppressed only expected unused/dead-code warnings from
isolating the modules. The ignored probe source and its 296.8 MiB generated
build target were removed afterward. Runtime tests also prove simulated
Windows/Other absent reads refuse before their callback and create nothing,
while mutation refuses before control creation.

### Repository and security

- `bun run verify:contracts`: exit 0; all five generated contracts are current.
- `bun run verify:fast`: Biome checked 174 files, TypeScript passed, all five
  contracts passed, and native-action checks passed. The command then stopped
  at `check:seeds-json-output`: the repo-pinned CLI parsed ready 50, blocked 96,
  and list 50. That repo-pinned stress result is the repository-authoritative
  evidence; the external global CLI lacks the pipe-safe stdout patch. The
  global installation is outside this worktree and was not modified.
- Docs/Seeds secret hygiene: 0 findings.
- Final Betterleaks over the complete implementation, planning, and report
  footprint after correction: approximately 415.13 KB, no leaks.
- Exact-base diff hygiene and rustfmt: exit 0.

## Footprint and runtime-dark proof

Expected final tracked footprint is exactly:

```text
docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-3b53-plan.md
docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-3b53-report.md
docs/commit-state-2026-08-16-session-control-contract-wave7c.md
src-tauri/src/persistence/canonical_durability.rs
src-tauri/src/persistence/session_artifact_manifest.rs
```

Static assertions passed for:

- no production caller outside the manifest module for the new Session store
  constructors or transition operation;
- no diff in Seeds, workflows, dependencies/locks, generated contracts,
  persistence module registration, commands, audio consumers, or projections;
- no production directory provisioning or per-Session lock;
- no added `admitted_session_semantics_floor` call; and
- only the three Session-owned control identities plus the one store-owned
  global coordination identity.

## Findings, open questions, and rollback

No narrow ADR or owned-interface blocker was found in the bounded prototype:
the existing manifest temporary can be staged exactly and consumed by recovery
CAS without another identity. Standards and Spec reviewers have not reviewed
round 2, so both approvals remain pending and no SHIP or integration claim is
made.

### Open cross-file gap: addressed control identities are reserved nowhere

This gap is real at this source checkpoint, is outside this wave's authorized
footprint, and is deliberately not fixed here. Its fix seam is
`canonical_log.rs`, which this wave does not own.

`manifest_path` and `temporary_path` (`session_artifact_manifest.rs:785,792`)
became address-aware in this diff: an addressed store reads and writes
`.audio-graph-session-<key>-artifacts.v1.json` and its `.tmp` sibling.
`internal_identities` (`:614`) did not. It still returns the three root-wide
legacy constants `MANIFEST_FILE_NAME`, `MANIFEST_TEMP_FILE_NAME`, and
`COORDINATION_FILE_NAME` regardless of the bound address, and the module-level
`is_internal_identity` (`:2096`) compares candidate managed identities against
those same three statics. Nothing reserves an addressed store's own manifest,
temporary, or proof identity, and `ManifestInternalIdentities` (`:273`) has no
provenance member at all.

Two consequences follow:

- a manifest candidate can inventory the addressed store's own live manifest or
  temporary as an ordinary managed artifact.
  `validate_managed_identity` (`:2006`) refuses only the legacy names, so
  ADR-0038's "no manifest inventories itself" rule is unenforced for every
  addressed Session. The proof is a different case: the manifest is required to
  inventory it as `SessionProvenanceEvents`.
- `validate_recovery_identity_reservations` (`canonical_log.rs:1205`) builds its
  reserved set from `store.internal_identities()`, so against an addressed store
  it protects three filenames that store never touches while leaving the three
  it owns unreserved. A recovery descriptor naming the live addressed manifest
  as its source therefore passes reservation and reaches
  `self.source.set_len(self.source_after.content.byte_length)`
  (`canonical_log.rs:706`), truncating the authoritative manifest.

ADR-0038's Compliance section requires that inventory, export, delete, and
recovery "treat all three per-Session control identities consistently and exclude
the global lock". Address-blind reservation does not satisfy that bullet: it is
consistent only in the legacy unaddressed shape this wave is superseding.

The gap is latent rather than live. `SessionArtifactManifestStore::for_session`
still has no production caller — every addressed store in the tree is built by a
crate test — so no production recovery runs against an addressed root today. That
is why this wave leaves it open rather than treating it as an immediate
regression, and it is also why the candidate stays NO_SHIP.

Follow-up, needing its own Seed and footprint over `canonical_log.rs` plus
`session_artifact_manifest.rs`: derive `internal_identities` and
`is_internal_identity` from the bound `SessionControlAddress` when one exists,
carry the provenance identity in that set, keep the store-owned global lock
excluded, and add a recovery fixture that refuses a descriptor naming an addressed
store's manifest, temporary, or proof. Close it before any production caller
adopts the addressed constructor.

Immutable remaining checklist:

- run strict Clippy and locked production/test checks on `3ddeb0a`;
- run the full serialized cloud library suite;
- rebuild the actual-module Windows production and cfg(test) probes;
- rerun contracts and `verify:fast`, recording the repo-pinned Seeds fallback
  separately from the known external global-CLI limitation;
- rerun Betterleaks, docs/Seeds secret hygiene, exact footprint, diff, and
  runtime-dark searches;
- adversarially review partial manifest-intent recovery, special-entry
  authentication, and the new canonical public surface; and
- obtain fresh Standards and Spec review. Do not integrate before both clear.

The proof `CreateNew` classification bullet is discharged by correction round 3
above: the misclassification is fixed under TDD, the proof-stage fault table now
covers all five previously uncovered stages, and the two residues that pass names
(no reachable abandon path, and the same lie one step later at the manifest
install's `CreateNew`) are recorded there rather than closed.

Checklist state after this pass: strict Clippy is now green, the focused
`persistence` library suite and rustfmt are green, and their real output is
recorded above. Every other bullet is untouched. Two items are added rather than
substituted — close the addressed-identity reservation gap above, and settle the
same-length zero-filled proof residue question with a crash or block-device
fixture. The `AlreadyCompleted` retry path's surviving prefix-length threshold
also needs a Spec decision: either remove that second entry point or document it
as contract. The 3b53 plan's round-2 wording that denies a prefix-length heuristic
was left unedited in this pass by instruction; it still overstates the
replacement and needs a correction of its own.

Exact next command. The previously recorded strict-Clippy command has run and is
green, so the next outstanding broad gate is the full serialized cloud library
suite:

```text
CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud -- --test-threads=1
```

Two inherited environment limitations remain: the external global Seeds CLI
lacks the pipe-safe stdout patch, and this Linux host lacks the MSVC librarian
needed for a full-repository Windows cross-build. Worktree-local actual-module
Windows probes remain the bounded compile evidence.

The dormant kernel has no production caller or persisted migration to unwind.
Rollback reverts, in order, constructor hardening, proof-before-CAS, immutable
proof, checked-read, addressing, and finally the planning/report commits. For
round 2 specifically, revert `3ddeb0a` and its following docs-only handoff
commit.
