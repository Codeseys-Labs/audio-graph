# audio-graph-68a1 — REPORT: provenance-to-durable-proof binding, then the B4 re-key

Seed `audio-graph-68a1`. Branch `work/audio-graph-68a1-proof-binding` in
`.worktrees/68a1-proof-binding`, base `27be43a`. Design: `plan.md` in this
directory (the brief as amended by the adversarial critique).

Footprint: one production file plus this run's documents.

* `src-tauri/src/persistence/session_artifact_manifest.rs` — binding, re-key,
  inverted pin, new tests.
* `docs/agentic-runs/2026-08-18-audio-graph-68a1/plan.md`, `report.md`,
  `check-anchors.py`.
* `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-68a1-report.md`
  — pointer at the wave-index path the brief named.
* `docs/commit-state-2026-08-16-session-control-contract-wave7c.md`,
  `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-3b53-report.md`
  — superseding blocks appended; no history rewritten.

`.seeds/` untouched. No ADR added or edited.

---

## 1. Anchor honesty

Every `path:line` anchor in `plan.md`, in this report, in the wave-path pointer,
and inside the `audio-graph-68a1 anchors` markers of the two superseded documents
is checked by `docs/agentic-runs/2026-08-18-audio-graph-68a1/check-anchors.py`.

What that script actually verifies, exactly:

1. it enumerates every `path:line` anchor in those scanned regions and asserts a
   recorded substring is present at the cited line of the cited file;
2. it FAILS on any anchor it finds that its table does not enumerate;
3. it FAILS on any table entry that no scanned region cites;
4. it FAILS on a bare line-number anchor anywhere in a scanned region — a
   colon followed by digits, naming no file — because such a number silently
   detaches from the file it was written against.

What it does not verify, deliberately and by design: fenced code blocks are
stripped before scanning, so the `src/persistence/...` locations printed by the
reproduced panic output below are NOT treated as claims about this tree — they are
evidence from the intermediate RED trees, and their line numbers refer to those
trees. The two older documents are scanned only between their markers, because
their earlier rounds carry anchors from earlier commits that those documents
already declare unmaintained.

Result on the shipped state, verbatim:

```
OK 41 anchors checked across 5 documents; 0 failures
```

No anchor in this run was hand-verified. Two bare line anchors that the first
draft of the new code comments inherited from the brief were removed rather than
renumbered: symbol names carry the reference instead.

---

## 2. What shipped

All symbols in `src-tauri/src/persistence/session_artifact_manifest.rs`.

### 2.1 The binding

`bind_v2_provenance_to_durable_proof`
(`src-tauri/src/persistence/session_artifact_manifest.rs:2499`) proves that a V2
candidate's `SessionProvenanceEvents` entry describes this Session's durable
v1-to-v2 transition-proof record. The record's path and identity are DERIVED from
the store's Session address and are never caller-supplied; the whole read happens
under the exclusive guard this transaction already holds, before anything is
staged. Order: entry identity, `symlink_metadata`, open, re-check of the opened
handle (`is_file` and length against the bound), bounded read, canonical decode,
Session match, digest-and-length comparison.

`V2ProvenanceProofBindingError`
(`src-tauri/src/persistence/session_artifact_manifest.rs:442`) classifies the
refusals separately and truthfully: `DurableProofAbsent`,
`NonRegularDurableProof`, `DurableProofExceedsCanonicalBound`,
`DurableProofUnreadable { kind, raw_os_error }`,
`NotCanonicalDurableProof(SessionSemanticsTransitionProofError)`,
`DurableProofSessionMismatch`, `ProvenanceIdentityMismatch`,
`ProvenanceContentMismatch`. `binding_io`
(`src-tauri/src/persistence/session_artifact_manifest.rs:2551`) is the single I/O
classifier: `NotFound` is the only condition that becomes `DurableProofAbsent`,
everything else is reported verbatim as `DurableProofUnreadable` and is never
widened into absence. An over-long record is refused *without* being decoded, so
this kernel never reports a "malformed proof" it only partially read. The bound
itself is `MAX_SESSION_SEMANTICS_TRANSITION_PROOF_BYTES`
(`src-tauri/src/persistence/session_artifact_manifest.rs:2481`), whose rustdoc
carries the arithmetic: 143-byte golden wire, 118-byte template, 255-byte id caps,
escaping at most doubling each id, 1138-byte true ceiling under a rounded 4096.

`ManifestCasRejection::V2ProvenanceProofBinding`
(`src-tauri/src/persistence/session_artifact_manifest.rs:692`) is appended at the
end of the enum, off audio-graph-3cf2's rustdoc region. Nothing outside this file
matches `ManifestCasRejection` exhaustively; the sole external use wraps it
(`src-tauri/src/persistence/canonical_log.rs:349`).

### 2.2 The validator now hands over the validated pieces

`validate_v2_session_provenance`
(`src-tauri/src/persistence/session_artifact_manifest.rs:2436`) returns
`V2SessionProvenanceEntry`
(`src-tauri/src/persistence/session_artifact_manifest.rs:2431`) — the validated
provenance identity and content by reference — instead of the unit type. This is
the critique's required change: passing the whole entry would have forced the
binding to read `availability.content()` and classify an impossible `None` as
`ProvenanceContentMismatch`, which is a silent misclassification arm. With the
pieces extracted by the function that already proved them, that state is
unrepresentable and the binding has no such arm.

Its two self-consistency checks are unchanged, including
`manifest.transition.fingerprint != content.sha256`
(`src-tauri/src/persistence/session_artifact_manifest.rs:2464`). They are the
load-path and floor-admission checks, they still fire first, and
`session_semantics.rs`'s
`forged_already_completed_manifest_proof_cannot_preserve_the_logical_floor` still
depends on `TransitionFingerprintMismatch`.

### 2.3 The re-key

`refuse_unproven_v2_candidate`
(`src-tauri/src/persistence/session_artifact_manifest.rs:1650`), called from
`prepare_compare_and_swap` at
`src-tauri/src/persistence/session_artifact_manifest.rs:1716`, replaces the
floor-keyed gate. It is keyed on whether this call performs the transition:

* `proof_owned` calls are skipped, and must be — the advance calls
  `prepare_compare_and_swap` at
  `src-tauri/src/persistence/session_artifact_manifest.rs:1365`, BEFORE the record
  exists, and binds by full-byte equality afterwards at
  `src-tauri/src/persistence/session_artifact_manifest.rs:1375`;
* non-V2 candidates and unaddressed stores are skipped, unchanged behaviour;
* an absent head or a head below V2 is still `TransitionProofRequired`;
* an addressed store without a derived provenance path/identity is
  `SessionAddressRequired`;
* otherwise the candidate's entry must bind to the durable record.

Placement is unchanged — after `validate_candidate_and_normalize`, before the head
block — so floor regression, prepared/completed, idempotency, generation, and size
checks keep their existing order and classifications, and every refusal this gate
raises happens before anything is staged.

**Deviation from the brief, deliberate:** the brief wrote this as a nested `if`
block inline in `prepare_compare_and_swap`. It ships as one named method with
guard clauses. Reasons: `prepare_compare_and_swap`'s diff is seven lines instead
of thirty; the rationale sits in rustdoc next to the predicate it explains; and
the inline nested-`if` shape trips `clippy::collapsible_if` under `-D warnings` on
Rust 1.95.0. Semantics, ordering, and every classification are identical to the
brief's version.

### 2.4 The pinning test was inverted, not deleted

`committed_v2_head_refuses_later_generations_until_proof_binding_exists` became
`committed_v2_head_records_later_generations_only_against_the_durable_proof`
(`src-tauri/src/persistence/session_artifact_manifest.rs:4317`), same position in
the file. The old name asserts "until proof binding exists", which became a false
statement about shipped code, and a test name that misdescribes the code is the
same defect class as a comment that does. The rustdoc now describes the shipped
state and carries one paragraph of history pointing at the two refusal tests. Its
V1-floor-regression assertion is kept verbatim, including its now-stale
`expected_generation` of 1 against a generation-2 head, because floor regression
is classified before the generation comparison and keeping it verbatim proves that
ordering; a comment in the test says exactly that.

---

## 3. RED evidence, verbatim

RED was captured in two phases because two of the tests name a type that did not
exist yet, and a compile error is not the seed's stated RED signal.

**Phase A** — T1 (the inverted pin), T5, T6 and the T4 case written against the
unchanged API. Verbatim, from `cargo +1.95.0 test --locked --lib
--no-default-features --features cloud persistence::session_artifact_manifest --
--test-threads=1`:

```
failures:

---- persistence::session_artifact_manifest::tests::advanced_head_conflicts_on_the_same_idempotency_id_with_changed_artifacts stdout ----

thread 'persistence::session_artifact_manifest::tests::advanced_head_conflicts_on_the_same_idempotency_id_with_changed_artifacts' (224494) panicked at src/persistence/session_artifact_manifest.rs:3413:9:
assertion `left == right` failed: the same transition id with changed artifacts conflicts
  left: Rejected(TransitionProofRequired)
 right: Rejected(TransitionConflict)
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- persistence::session_artifact_manifest::tests::committed_v2_head_records_later_generations_only_against_the_durable_proof stdout ----

thread 'persistence::session_artifact_manifest::tests::committed_v2_head_records_later_generations_only_against_the_durable_proof' (224499) panicked at src/persistence/session_artifact_manifest.rs:3344:22:
expected a genuine later generation to be accepted, got Rejected(TransitionProofRequired)

---- persistence::session_artifact_manifest::tests::quarantine_recovery_remains_closed_on_a_v2_session stdout ----

thread 'persistence::session_artifact_manifest::tests::quarantine_recovery_remains_closed_on_a_v2_session' (224527) panicked at src/persistence/session_artifact_manifest.rs:3500:9:
assertion `left == right` failed: a V2 quarantine completion has no prepared head to complete
  left: Rejected(TransitionProofRequired)
 right: Rejected(CompletionRequiresPrepared)


failures:
    persistence::session_artifact_manifest::tests::advanced_head_conflicts_on_the_same_idempotency_id_with_changed_artifacts
    persistence::session_artifact_manifest::tests::committed_v2_head_records_later_generations_only_against_the_durable_proof
    persistence::session_artifact_manifest::tests::quarantine_recovery_remains_closed_on_a_v2_session

test result: FAILED. 51 passed; 3 failed; 0 ignored; 0 measured; 1682 filtered out; finished in 9.09s

error: test failed, to rerun pass `--lib`
```

Two observations that matter. Every failure is
`Rejected(TransitionProofRequired)`: the refusal existed, for the wrong reason —
the wedge, not the binding. And T6's V2-**Prepared** shape passed already, because
`validate_candidate_and_normalize` refuses it with
`InvalidV2SessionProvenance(TransitionNotCompleted)` before the gate is reached,
which is exactly the claim the seed asked to be recorded. The T4 case added in this
pass also passed from the start — the pre-change gate already refused every V2
candidate whose call did not own proof bytes — so it appears in no failure list
above and is a green pin on behaviour the re-key had to preserve, not RED evidence.

**Phase B** — `V2ProvenanceProofBindingError` and the
`ManifestCasRejection::V2ProvenanceProofBinding` variant added as types only, with
no behaviour anywhere, so T2 and T3 could compile:

```
failures:

---- persistence::session_artifact_manifest::tests::advanced_head_conflicts_on_the_same_idempotency_id_with_changed_artifacts stdout ----

thread '...' (234629) panicked at src/persistence/session_artifact_manifest.rs:3477:9:
assertion `left == right` failed: the same transition id with changed artifacts conflicts
  left: Rejected(TransitionProofRequired)
 right: Rejected(TransitionConflict)

---- persistence::session_artifact_manifest::tests::advanced_head_refuses_later_generations_when_the_durable_proof_is_not_intact stdout ----

thread '...' (234635) panicked at src/persistence/session_artifact_manifest.rs:3782:13:
assertion `left == right` failed: durable proof shape absent must refuse a genuine later generation
  left: Rejected(TransitionProofRequired)
 right: Rejected(V2ProvenanceProofBinding(DurableProofAbsent))

---- persistence::session_artifact_manifest::tests::committed_v2_head_records_later_generations_only_against_the_durable_proof stdout ----

thread '...' (234640) panicked at src/persistence/session_artifact_manifest.rs:3408:22:
expected a genuine later generation to be accepted, got Rejected(TransitionProofRequired)

---- persistence::session_artifact_manifest::tests::forged_v2_provenance_is_refused_on_an_advanced_head stdout ----

thread '...' (234692) panicked at src/persistence/session_artifact_manifest.rs:3669:13:
assertion `left == right` failed: forged shape forge-identity-1 must be refused on an advanced head
  left: Rejected(TransitionProofRequired)
 right: Rejected(V2ProvenanceProofBinding(ProvenanceIdentityMismatch))

---- persistence::session_artifact_manifest::tests::quarantine_recovery_remains_closed_on_a_v2_session stdout ----

thread '...' (235855) panicked at src/persistence/session_artifact_manifest.rs:3564:9:
assertion `left == right` failed: a V2 quarantine completion has no prepared head to complete
  left: Rejected(TransitionProofRequired)
 right: Rejected(CompletionRequiresPrepared)


failures:
    persistence::session_artifact_manifest::tests::advanced_head_conflicts_on_the_same_idempotency_id_with_changed_artifacts
    persistence::session_artifact_manifest::tests::advanced_head_refuses_later_generations_when_the_durable_proof_is_not_intact
    persistence::session_artifact_manifest::tests::committed_v2_head_records_later_generations_only_against_the_durable_proof
    persistence::session_artifact_manifest::tests::forged_v2_provenance_is_refused_on_an_advanced_head
    persistence::session_artifact_manifest::tests::quarantine_recovery_remains_closed_on_a_v2_session

test result: FAILED. 51 passed; 5 failed; 0 ignored; 0 measured; 1682 filtered out; finished in 9.53s

error: test failed, to rerun pass `--lib`
```

The only edit to that transcript is the elision of the repeated thread name inside
each `thread '...' panicked` line; the `panicked at`, assertion, `left`, `right`,
and result lines are verbatim.

Neither phase committed a state in which the re-key existed without the binding.
Binding, re-key, inverted pin and all tests landed in one commit.

### 3.1 The forgery-acceptance probe (uncommitted, local only)

To prove the binding is load-bearing rather than merely present, the binding call
in `refuse_unproven_v2_candidate` was replaced locally with a success return — the
re-key without the binding, exactly the state audio-graph-3b53 reverted — and T2
was run. This probe was **never committed**; it exists only as the transcript
below and was reverted before the commit. Verbatim, elided only where marked:

```
running 1 test
test persistence::session_artifact_manifest::tests::forged_v2_provenance_is_refused_on_an_advanced_head ... FAILED
...
assertion `left == right` failed: forged shape forge-identity-1 must be refused on an advanced head
  left: Accepted { manifest: SessionArtifactManifestV1 { schema_version: 1, session_id: "session-1", session_semantics_version: SessionSemanticsVersion(2), generation: 2, transition: ManifestTransition { idempotency_id: "forge-identity-1", fingerprint: Sha256Digest("sha256:db17cad6d6d0d31fcbb82e83ff4f0dfdecd5d43dd67d4d0aec7327f6a011c3ae"), state: Completed }, artifacts: [SessionArtifactEntry { kind: SessionProvenanceEvents, privacy_class: CanonicalSessionMemory, managed_identity: ManagedArtifactIdentity("attacker/not-the-proof.jsonl"), availability: Present { content: ArtifactContentIdentity { sha256: Sha256Digest("sha256:db17cad6d6d0d31fcbb82e83ff4f0dfdecd5d43dd67d4d0aec7327f6a011c3ae"), byte_length: 146 } } }, <two entries elided> ], quarantine_transaction: None }, durability: CanonicalDurabilityReceipt { mutation: SnapshotReplacement, barrier: FileAndParentNamespace } }
 right: Rejected(V2ProvenanceProofBinding(ProvenanceIdentityMismatch))
```

That reproduces audio-graph-3b53's demonstrated consequence exactly: generation 2
installed with `managed_identity = "attacker/not-the-proof.jsonl"` while the
genuine durable proof stayed untouched at the control identity. The binding, not
the re-key, is what refuses it.

---

## 4. Gates, verbatim

All four gates run from the worktree root on the shipped state
(`CARGO_TARGET_DIR="$PWD/src-tauri/target"`, Rust 1.95.0, `--features cloud`).
The transcripts below come from the final run of all four, against the source tree
this branch ends on; only the elapsed-time lines vary between runs of it.

Strict Clippy — `cargo +1.95.0 clippy --locked --manifest-path
src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud --
-D warnings`:

```
    Checking audio-graph v0.1.0-rc.1 (/home/codeseys/DevBox/audio-graph/.worktrees/68a1-proof-binding/src-tauri)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 16.15s
exit=0
```

Authoritative serial persistence suite — `cargo +1.95.0 test --locked
--manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud
persistence -- --test-threads=1`:

```
running 240 tests
test result: ok. 240 passed; 0 failed; 0 ignored; 0 measured; 1498 filtered out; finished in 19.69s
exit=0
```

236 passed at the base `27be43a`; 240 now. Net plus four tests: T2, T3, T5, T6 are
new, T1 replaced the pinning test in place, and T4's new case joined an existing
test.

Formatting — `cargo +1.95.0 fmt --all -- --check` from `src-tauri`:

```
exit=0
```

Whitespace — `git diff --check` from the worktree root:

```
exit=0
```

Named regression set, each run by name on the shipped state:

```
test persistence::session_artifact_manifest::tests::proof_before_manifest_transition_returns_actual_accepted_and_already_completed ... ok
test persistence::session_artifact_manifest::tests::proof_conflict_and_indeterminate_prevent_manifest_mutation_then_retry_converges ... ok
test persistence::session_artifact_manifest::tests::stale_existing_head_rejects_different_transition_before_proof_mutation ... ok
test persistence::session_artifact_manifest::tests::v2_candidate_requires_exact_bound_session_provenance_proof ... ok
test persistence::session_artifact_manifest::tests::accepted_v2_manifest_cannot_regress_to_v1 ... ok
test persistence::session_artifact_manifest::tests::addressed_generic_cas_cannot_install_v2_without_owning_proof_bytes ... ok
```

and all of `persistence::session_semantics::tests`:

```
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 1728 filtered out; finished in 0.04s
```

Anchor check — `python3 docs/agentic-runs/2026-08-18-audio-graph-68a1/check-anchors.py`:

```
OK 41 anchors checked across 5 documents; 0 failures
```

Gates NOT run, stated plainly: no Windows or macOS execution, no `--all-features`
build, no integration or UI suite, no benchmark. This change is confined to one
dormant persistence module; the four gate commands the seed names are the
authoritative set and all four are green.

---

## 5. Every property the gate protected, and where it is now proved

| Property | Where |
| --- | --- |
| A V1 candidate against a V2 head is a floor regression | `accepted_v2_manifest_cannot_regress_to_v1` (`src-tauri/src/persistence/session_artifact_manifest.rs:4037`), plus T1's third assertion kept verbatim |
| A real transition still requires its proof | `addressed_generic_cas_cannot_install_v2_without_owning_proof_bytes` (`src-tauri/src/persistence/session_artifact_manifest.rs:4138`): absent head AND, new in this seed, a committed V1 head, both `TransitionProofRequired` |
| Same idempotency id with changed artifacts conflicts | T5 (`src-tauri/src/persistence/session_artifact_manifest.rs:4387`) — `TransitionConflict`, and see R4 for why not `IdempotencyConflict` |
| Byte-identical replay is idempotent | T5 — `AlreadyCompleted` |
| The advance path's refusal classifications are untouched | `proof_before_manifest_transition_returns_actual_accepted_and_already_completed`, `proof_conflict_and_indeterminate_prevent_manifest_mutation_then_retry_converges`, `stale_existing_head_rejects_different_transition_before_proof_mutation`, all green with no edits |
| A forged provenance entry cannot install on an advanced head | T2 (`src-tauri/src/persistence/session_artifact_manifest.rs:4527`), four shapes, each mutating nothing |
| A damaged durable record cannot admit a later generation | T3 (`src-tauri/src/persistence/session_artifact_manifest.rs:4636`), six record shapes |
| Quarantine recovery is still closed on a V2 Session | T6 (`src-tauri/src/persistence/session_artifact_manifest.rs:4444`), four candidate shapes |

The seed's "also record while here" closure, now executable: quarantine recovery
is unreachable on a V2 Session for every candidate shape. V1 prepared and V1
completed candidates are `SessionSemanticsFloorRegression`; a V2 prepared candidate
is `InvalidV2SessionProvenance(TransitionNotCompleted)`; a V2 completed candidate
is `CompletionRequiresPrepared`
(`src-tauri/src/persistence/session_artifact_manifest.rs:1768`). The named cause is
`SessionArtifactManifestV1::candidate`
(`src-tauri/src/persistence/session_artifact_manifest.rs:256`) hardcoding
`SessionSemanticsVersion::V1`, and the production quarantine candidate is built
through exactly that constructor
(`src-tauri/src/persistence/canonical_log.rs:973` to
`src-tauri/src/persistence/canonical_log.rs:1032`), reaching this CAS through
`compare_and_swap_recovery`
(`src-tauri/src/persistence/canonical_log.rs:1107`) with `proof_owned = false`. The
re-key did not open that path, and T6 keeps that statement honest.

---

## 6. What this binding does NOT prove

Stated here and in the enum's rustdoc, because overclaiming it is the failure mode
that produced the reverted round:

* It proves the Session's **floor** is backed by a real, canonical,
  session-matching proof record whose bytes are exactly what the candidate's
  inventory claims. It proves **nothing** about the later generation's own
  transition.
* It deliberately does **not** compare the record's `idempotency_id` to the
  candidate's transition. Every generation after the advance legitimately carries a
  different one; comparing them would refuse every legitimate later generation.
* It closes the **API-level** forgery — a caller that controls the candidate's own
  fields — for callers cooperating with the canonical exclusive guard. It does not
  close an adversary with raw write access to the managed root, and nothing in this
  kernel can.
* The load path is unchanged and still accepts a persisted V2 manifest whose
  durable proof is absent or altered. That is deliberate (R2).

---

## 7. Residuals, with owners

| # | Residual | Owner |
| --- | --- | --- |
| R1 | Install-stage `Durability` classification in `commit_prepared_compare_and_swap` (`src-tauri/src/persistence/session_artifact_manifest.rs:1808`) and `install_snapshot_inner`, and the missing removal/abandon primitive for an orphaned intent temporary. Untouched here by scope fence. | audio-graph-3cf2 — lands first; this branch rebases onto it |
| R2 | `load`, `load_selected_manifest`, and `load_manifest_file` (`src-tauri/src/persistence/session_artifact_manifest.rs:2008`) still accept a persisted V2 manifest whose durable proof is absent or altered: they run only the structural validator. Deliberate — a damaged Session must stay loadable for diagnosis, and floor admission is CAS-gated through `admitted_session_semantics_floor` (`src-tauri/src/persistence/session_semantics.rs:167`). | unowned; needs a seed |
| R3 | `checked_session_open` (`src-tauri/src/persistence/session_semantics.rs:113`) admits a V2 floor from manifest bytes alone, with no reference to a durable record. | unowned; follow-up seed |
| R4 | `IdempotencyConflict` (`src-tauri/src/persistence/session_artifact_manifest.rs:1749`) is now unreachable for a bound V2 candidate: head and candidate both carry the durable proof digest as their fingerprint, so the same-id arm's comparison cannot fire and the divergence falls through to `TransitionConflict` (`src-tauri/src/persistence/session_artifact_manifest.rs:1762`). The refusal holds; the classification shape differs from V1's. Recorded, not fixed. | unowned |
| R5 | `recovery_key(&candidate.transition.fingerprint)` (`src-tauri/src/persistence/session_artifact_manifest.rs:1800`) is identical for every V2 generation of a Session, since all of them carry the proof digest. `CanonicalRecoveryKey` is an opaque correlation token, never a path component or comparison key, so this costs diagnosability, not correctness, and it was already the status quo on the transition path. | unowned |
| R6 | `SessionArtifactManifestV1::candidate`'s V1 hardcode (`src-tauri/src/persistence/session_artifact_manifest.rs:256`) and therefore the quarantine-recovery closure on a V2 Session. Fixing it means letting recovery build V2 candidates, which collides with `TransitionNotCompleted` versus `CompletionRequiresPrepared` — a design question about quarantine-under-V2, not a binding question. | unowned; design decision needed |
| R7 | The binding, like the gate, applies only to addressed stores. Sound only because every unaddressed constructor is `#[cfg(test)]` while `for_session` and `qualified_existing_session` both derive an address. If a future production caller constructs an unaddressed store, the binding silently does not apply. Standing constraint. | whoever adds such a caller |
| R8 | The kernel is still dormant: `grep` finds no caller of `SessionArtifactManifestStore::for_session` or `::qualified_existing_session` outside this module. Everything here is proved by tests, not by production traffic. | wave-level |
| R9 | `DurableProofUnreadable` has no automated test. Forcing a read error at that exact path needs root-fragile permission manipulation or a fault seam this module does not have. Precedent: `ManifestLoadError::Io` is constructed only by `load_io` and is asserted by no test in this module either. Coverage was not faked. | unowned; would need a fault seam |
| R10 | The binding does not re-check metadata *after* its read, unlike `load_manifest_file` (`src-tauri/src/persistence/session_artifact_manifest.rs:2024` is the pre-read re-check this binding does mirror). It does not need to: the record is accepted only if its bytes hash and measure to exactly what the candidate claims, so a torn or swapped read is refused as `ProvenanceContentMismatch` or `NotCanonicalDurableProof`. A `ChangedDuringRead` analogue would only rename an already-refused state. | closed by design; recorded |

---

## 8. Rebase note for the integrator

Production edits are confined to three regions, all away from audio-graph-3cf2's:
the error enums near
`src-tauri/src/persistence/session_artifact_manifest.rs:442` and
`src-tauri/src/persistence/session_artifact_manifest.rs:692` (appended at the end
of `ManifestCasRejection`, not inside the `Durability` rustdoc); the gate method
plus its seven-line call site immediately before `prepare_compare_and_swap`; and
the validator/binding block after `validate_v2_session_provenance`. The
`commit_prepared_compare_and_swap` body and the install stage are untouched. Test
additions are one contiguous block from T1 through T3, plus one added case inside
`addressed_generic_cas_cannot_install_v2_without_owning_proof_bytes`.

After the rebase, re-run `check-anchors.py`: every anchor in this report and in
`plan.md` needs its line numbers refreshed if audio-graph-3cf2's edits shift them,
and the script fails loudly rather than letting a stale anchor through.
