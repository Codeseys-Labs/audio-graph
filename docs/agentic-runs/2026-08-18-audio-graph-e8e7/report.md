# audio-graph-e8e7 — guarded admission (report)

Branch `work/audio-graph-e8e7-guarded-admission`, worktree
`.worktrees/e8e7-guarded-admission`, base `222b2ad` on
`integration/session-memory-wave-20260814`.

The amended design is [`plan.md`](plan.md). This report records what shipped, the
verbatim gate result lines, and the residuals with owners. Every `file:line`
anchor in this document is machine-checked; see section 7.

---

## 1. Commits

| SHA | Subject |
| --- | --- |
| `1e8dfa9` | `docs(audio-graph-e8e7): record the amended guarded-admission design` |
| `b2b6675` | `feat(audio-graph-e8e7): add the historical-unknown wire reason and durable control-plane retirement` |
| `100a4c2` | `feat(audio-graph-e8e7): admit Session floors from the real outcome, under the guard, from observed bytes` |
| `41b3aa0` | `feat(audio-graph-e8e7): wire the guarded read seam and control-plane delete parity` |

`1e8dfa9` also carries the two acceptance-(3) RED tests, which fail at that commit
by design. Every later commit is gate-green. This report and its anchor checker
landed in one docs-only commit on top of `41b3aa0`, which is why that commit's own
SHA is not in the table above.

Two further **review-fix commits** follow, and are not in the table for the same
reason. Neither changes behaviour.

The first corrects report section 4's Windows paragraph (which rested on a false
premise), reconciles `plan.md` with the shipped code at four sites, adds residual
R8, sharpens R4, and extends `read_session_transcript_snapshot`'s rustdoc with the
ungated-reader constraint. Its only non-doc edit is that rustdoc.

The second answers a second review round: it replaces `session_semantics.rs`'s
`//! Dormant …` module header, which this seed itself falsified by putting
`open_session_for_content` and `retire_session_control_plane` on production paths;
retracts the same unmeasured Windows inference from `plan.md` section 7 that the
first commit had retracted only in `report.md`; corrects section 4's ungated-test
enumeration from 2 to the actual 10; and adds residual R9 for the two fence-blocked
headers this seed's wiring also falsified. Its only non-doc edit is the module
header, which shifted the seven `session_semantics.rs` anchors by +11 lines in
section 7's table and in the checker.

All four gates were re-run on each; section 4 reports the numbers from the last of
them.

A later commit also sits outside the table. `b238860` answered a review round that
returned SHIP on both axes: it put `projection_replay_report_for_session` and
`session_timeline` behind `open_session_for_content`, which is the MAJOR half of
residual R8, and added exactly one test
(`open_session_for_content_refuses_an_unsupported_reader_floor_on_a_bare_root`).
Section 4's original block therefore predates it by that one test; section 4.1
reconciles the counts.

**`098a674` closes residual R4** — it extends the control-identity reservation from
the historical bootstrap builder to the manifest validator, which is the only seam
that covers the `compare_and_swap` path. Section 3.(4) records what shipped,
section 2 the RED, and section 4 the re-run gates. It is the first commit in this
run to change `validate_and_normalize`'s behaviour, on the maintainer's explicit
2026-08-19 decision to lift the scope fence R4 was blocked by.

Files touched:

- `src-tauri/src/persistence/session_semantics.rs`
- `src-tauri/src/persistence/session_artifact_manifest.rs`
- `src-tauri/src/persistence/canonical_durability.rs` (one rustdoc sentence)
- `src-tauri/src/sessions/mod.rs`
- `src-tauri/src/commands.rs`
- `docs/agentic-runs/2026-08-18-audio-graph-e8e7/{plan.md,report.md,check-anchors.py}`

---

## 2. The RED, verbatim

Acceptance (3) is the only criterion with a behavioural RED. Captured at
`1e8dfa9` with
`cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud guarded_admission -- --test-threads=1`:

```text
running 2 tests
test persistence::session_semantics::guarded_admission_tests::accepted_later_generation_preserves_the_committed_floor ... FAILED
test persistence::session_semantics::guarded_admission_tests::accepted_v1_generation_preserves_the_v1_floor ... FAILED

---- persistence::session_semantics::guarded_admission_tests::accepted_later_generation_preserves_the_committed_floor stdout ----
assertion `left == right` failed: a durably Accepted later generation preserves the committed V2 floor
  left: Err(IllegalTransition { current: SessionSemanticsVersion(2), accepted: SessionSemanticsVersion(2) })
 right: Ok(SessionSemanticsVersion(2))

---- persistence::session_semantics::guarded_admission_tests::accepted_v1_generation_preserves_the_v1_floor stdout ----
assertion `left == right` failed
  left: Err(IllegalTransition { current: SessionSemanticsVersion(1), accepted: SessionSemanticsVersion(1) })
 right: Ok(SessionSemanticsVersion(1))

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 1745 filtered out; finished in 0.05s
```

That is exactly the stated reason: `exact_retry` was `true` only for
`AlreadyCompleted`, so an `Accepted` outcome that *preserves* the floor fell
through to `IllegalTransition`. audio-graph-68a1 made that arm reachable in
production.

The remaining new tests are compile REDs — the symbols they call did not exist at
the base — except the pins named as pins in `plan.md` section 7.

### 2.1 The R4 closure RED, verbatim

The control-identity reservation at the validator has a real behavioural RED on
both paths. Both were watched at `b238860` **before** the refusal existed, with the
assertions written as `is_err()` / `matches!(…, Rejected(_))` rather than against
the new variant, precisely so the failure would be about behaviour and not a
missing symbol.

Unit — the validator itself admitted the reserved name:

```text
$ CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked \
    --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
    --features cloud -- --test-threads=1 reserves
thread 'persistence::session_artifact_manifest::tests::candidate_inventory_reserves_this_sessions_control_identities_and_the_index' (935170) panicked at src/persistence/session_artifact_manifest.rs:6268:13:
.audio-graph-session-onsxg43jn5xc2mi-artifacts.v1.json was admitted as an ordinary artifact

test result: FAILED. 40 passed; 1 failed; 1 ignored; 0 measured; 1733 filtered out; finished in 0.18s
```

`onsxg43jn5xc2mi` is `encode_lowercase_base32(b"session-1")`, so the refused name is
this Session's own manifest head — the first of the four reserved classes the loop
reaches.

Integration — and this is the half that proves the enforcement point, because the
unit RED alone does not show the CAS path:

```text
$ … --features cloud -- --test-threads=1 the_generic_cas_refuses_an_inventory_entry_at_this_sessions_manifest_identity
thread 'persistence::session_artifact_manifest::tests::the_generic_cas_refuses_an_inventory_entry_at_this_sessions_manifest_identity' (935444) panicked at src/persistence/session_artifact_manifest.rs:6296:9:
the generic CAS admitted this Session's own manifest head as an ordinary artifact: Accepted { manifest: SessionArtifactManifestV1 { schema_version: 1, session_id: "session-1", session_semantics_version: SessionSemanticsVersion(1), generation: 1, transition: ManifestTransition { idempotency_id: "reserve-cas-1", fingerprint: Sha256Digest("sha256:eeee…eeee"), state: Completed }, artifacts: [SessionArtifactEntry { kind: SessionMetadata, privacy_class: OperationalMetadata, managed_identity: ManagedArtifactIdentity(".audio-graph-session-onsxg43jn5xc2mi-artifacts.v1.json"), availability: Present { content: ArtifactContentIdentity { sha256: Sha256Digest("sha256:ffff…ffff"), byte_length: 12 } } }, SessionArtifactEntry { kind: OriginalSessionAudio, privacy_class: OriginalEvidence, managed_identity: ManagedArtifactIdentity("audio/original.wav"), availability: Unavailable { reason: RetentionDisabled } }], quarantine_transaction: None }, durability: CanonicalDurabilityReceipt { mutation: InitialSnapshotInstall, barrier: FileAndParentNamespace } }

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 1774 filtered out; finished in 0.01s
```

The 64-character digest runs are elided to `eeee…eeee` / `ffff…ffff`; nothing else
is paraphrased. `Accepted` there means the head was installed **at the very
pathname the entry named** — the accepted manifest's own control identity is in its
own inventory. In GREEN both assertions were tightened to the typed variant, and
the integration test additionally asserts `!store.manifest_path().exists()`.

**Acceptance (5) has no RED and none is claimed.** The platform gate already
refuses: `namespace_supported_for` admits only Linux and macOS, and
`CanonicalFilesystemQualification::for_existing_managed_root` consults it before
any filesystem access. The mutation-refusal half is **already covered** by the
pre-existing `windows_other_session_transition_refuses_before_any_control_mutation`,
which this seed credits, did not write, and did not touch. This seed's
contribution to (5) is the read half plus the end-to-end refusal through the new
seam. No Windows durability was added.

---

## 3. What shipped, per acceptance criterion

### (1) Historical bootstrap records Present only from observed bytes

`ObservedManagedArtifact::observe` is the only constructor and the only path to
`ArtifactAvailability::Present` in the bootstrap. It calls `symlink_metadata`
first and refuses a non-regular entry, re-checks the opened handle, then streams
the file and derives `sha256` and `byte_length` from exactly those bytes.
`Ok(None)` means `NotFound`; every other `io::Error` is
`ObservedEntryUnreadable { kind, raw_os_error }`.

`HistoricalOriginalAudio::ObservedBytes` carries an `ObservedManagedArtifact`,
**not** a caller-supplied `ArtifactContentIdentity` as the brief specified. That
is a deliberate deviation: with the brief's shape the mandatory audio entry's
`Present` availability WOULD have come from a caller-supplied digest, which would
have made the brief's own contract 1 false, and a doc that misdescribes code is a
blocking defect here.

With no observable bytes the mandatory entry is
`Unavailable { reason: HistoricalUnknown }` at the builder-owned identity
`audio/original-session-audio` with `privacy_class: OriginalEvidence`. The live
inventory never yields audio bytes, so that is the normal case. No audio kind was
added to either enum.

Flat-root control and index identities are refused at the builder with distinct
classifications (`SessionControlIdentity`, `SessionIndexIdentity`,
`IdentityOutsideManagedArtifactTree`), because `is_internal_identity` reserves
only the three root-wide constants and
`.audio-graph-session-<key>-artifacts.v1.json` passes every other check in
`validate_managed_identity`.

That sentence is still exactly true after `098a674`: the R4 closure did **not**
touch `validate_managed_identity` or `is_internal_identity`, so those two symbols
keep their current meanings. The reservation the validator now performs lives in a
separate function, `refuse_reserved_control_identities`, called later in
`validate_and_normalize` — see section 3.(4).

**No production caller and no CAS.** The bootstrap returns a candidate.
Installing it is a `compare_and_swap`, which **stays uncalled in production** per
ADR-0038 section 11 and this seed's "no writer activation".

### (2) Guard-owned `checked_session_open` before canonical OR legacy reads

`guarded_session_open` supplies `checked_session_open` with the manifest that
`SessionArtifactManifestStore::checked_read` selected while holding the store's
shared coordination guard. `open_session_for_content` is the production policy
layer, and `commands.rs`'s `read_session_transcript_snapshot` — the shared
canonical-versus-legacy fork behind `load_session_impl`, `load_session_transcript`,
and `session_export_bundle` — now runs inside it at `maximum_supported = V1`. Both
branches of that fork are therefore behind one admitted floor.

**What that does NOT cover, stated here rather than left to be discovered.** The
seam gates the transcript fork, not every canonical read. `load_session_impl` and
`session_export_bundle` read the speaker-revision, projection-patch, materialized
notes/graph and live-assist streams *outside* the closure, and two whole commands
— `projection_replay_report_for_session` and `session_timeline` — call
`load_transcript_event_stream` with no floor check at all. Wiring them is outside
this seed's narrow-caller scope; residual **R8** carries them with an owner.

Both unguarded `V1` admissions carry the ADR-0038 section-5 pre/post absence
sandwich in `unguarded_absence_admission`: all three Session identities plus the
store-owned coordination identity are re-observed after the reader runs and before
its value escapes, and any appearance is
`ControlPlaneAppearedDuringUnguardedRead`. Observation uses `symlink_metadata`, so
a permission error is a classified refusal rather than "nothing is there".
`SessionFloorEvidence` was reconciled to three variants that state only what was
observed and under which guard (critique finding 7).

`SessionExportBundle` gained no field and `src/types/index.ts` is untouched.

### (3) The ACTUAL `ManifestCasOutcome` reaches the floor

The tail of `admitted_session_semantics_floor` now admits `current == accepted` as
preservation. `exact_retry` is gone. The retained `IllegalTransition` arm covers
exactly `accepted < current`, which the CAS already refuses upstream as
`SessionSemanticsFloorRegression`, and one comment says so. The V2 provenance
re-validation was not loosened; it still runs for every `accepted == V2`, including
the new preserve arm, which
`forged_accepted_manifest_proof_cannot_advance_the_logical_floor` and
`forged_already_completed_manifest_proof_cannot_preserve_the_logical_floor` still
pin.

`admit_session_semantics_v1_to_v2` makes "the actual outcome" structural: the
`ManifestCasOutcome` never leaves the function, so no boolean, receipt, head
re-read, or caller-asserted success can substitute for it. `Rejected` and
`DurabilityIndeterminate` are forwarded verbatim with their recovery keys.
`SessionSemanticsAdvanceError::ManifestCasNotAccepted` is now documented as
unreachable through the wrapper and reachable for direct callers.

### (4) Manifest, proof, and temp parity; lock and other Sessions excluded

- **Inventory.** `SessionControlPlanePaths` holds exactly the three Session-owned
  paths; the store-owned lock is unrepresentable in it and is reachable only
  through the separately named `store_coordination_path`.
- **Export.** `export_session_control_plane` reads the manifest, the proof bytes,
  and the temporary's residue inside one `checked_read` closure. The proof is
  **authenticated** before inclusion — regular file, within the canonical proof
  ceiling, exactly one canonical proof for this Session, and for a V2 manifest a
  digest and length equal to the manifest provenance entry's. A V2 manifest with
  an absent proof is `V2ProofMissing`, never a partial success (critique
  finding 5).
- **Recovery.** The temporary is retired through the same unlink of the same
  pathname under the same recovery-key domain as
  `ManifestWriteTransaction::abandon_staged_transition`, so an exact rerun of
  either reconciles the other. That is audio-graph-3cf2's substrate and its
  recovery-key semantics rather than a parallel temp path.
  `recovery_scan_ignores_session_control_residue` pins that control residue cannot
  resurrect or rename a Session.
- **Delete residual/retry.** `retire_owned_control_plane` routes the temporary, the
  manifest head, and the proof through the substrate's `unlink_canonical_entry`
  under the exclusive guard, in that fixed order, stopping at the first step that
  does not reach durable absence. **No raw `std::fs::remove_file`** (critique
  finding 1). It deliberately does not go through `begin_write`, which re-loads and
  re-validates the head, so a truncated or session-mismatched head cannot make a
  Session undeletable forever (critique finding 2);
  `control_plane_retirement_ignores_an_unreadable_head` pins that. The policy layer
  observes **before** it constructs a store, so a data root the substrate cannot
  qualify keeps today's behaviour when there is no residue (critique finding 4).
- **Exclusion.** The lock is unrepresentable twice over: once in
  `SessionControlPlanePaths`, once at the substrate, which refuses the coordination
  name under every ASCII-case spelling. Another Session's entries are unreachable
  because every pathname is derived from this store's validated address.
  `control_plane_retirement_removes_all_three_and_nothing_else` asserts the lock,
  `sessions.json`, and a second Session's three entries all survive byte-identical.

#### The carried B2 reservation, now enforced at the validator (`098a674`, closes R4)

audio-graph-3b53 finding B2 was carried into this acceptance as "per-Session control
identities are reserved nowhere today, so the parity work includes establishing that
reservation." This run first established it at the **historical bootstrap builder**
(`refuse_reserved_observed_identity`), which is not on any write path — residual R4
recorded that gap. `098a674` extends it to the validator.

**Enforcement point:** `refuse_reserved_control_identities`, called from
`validate_and_normalize` immediately after the artifact loop's
`original_audio_count` match and **before** `validate_quarantine_transaction`.

Why that point and not another, by symbol:

- It is **context-independent** — not gated on `ManifestValidationContext` — and it
  has to be. `prepare_compare_and_swap` validates every candidate and re-validates
  the generation-stamped result, so the generic `compare_and_swap` path is covered;
  `advance_session_semantics_v1_to_v2_inner` validates directly, so the transition
  path is covered; `SessionArtifactManifestV1::candidate` and
  `load_manifest_file_with_after_open` are covered too. Covering the load path is
  mandatory, not incidental: a `Candidate`-only reservation would have left an
  advanced V2 head **unloadable**, because that head legitimately carries its
  provenance entry at the control identity.
- It is **not** `validate_managed_identity` / `is_internal_identity`, deliberately.
  Tightening `ManagedArtifactIdentity::new` would refuse the very identities
  `session_control_address` derives for its own callers, and would break
  `session_semantics`'s bootstrap tests, which construct those identities through
  `new`. Both symbols keep their exact current meanings.
- Placement **before** `validate_quarantine_transaction` makes the reservation the
  last identity-level gate, so it wins over the coarser quarantine-shape
  mismatches; placement **before** `artifacts.sort_by` makes the refusal
  independent of inventory order on the wire.
- The session id it derives from is `manifest.session_id`, and that is already
  bound to the address wherever it matters: `prepare_compare_and_swap` and
  `advance_..._inner` both refuse `SessionMismatch` before validating, and
  `validate_requested_session` refuses `ManifestLoadError::SessionMismatch` for a
  head whose id differs from the address.

**Reserved set, and who owns each name:**

| Name | Owner | Refused as |
| --- | --- | --- |
| `.audio-graph-session-<key>-artifacts.v1.json` | per-Session | `ReservedSessionControlIdentity(SessionManifest)` |
| `.audio-graph-session-<key>-artifacts.v1.tmp` | per-Session | `ReservedSessionControlIdentity(SessionTemporary)` |
| `.audio-graph-session-<key>-v1-v2.provenance` | per-Session | `ReservedSessionControlIdentity(SessionProvenance)`, with one exemption below |
| `sessions.json` | store-wide, written by `sessions` / `user_data` | `ReservedSessionControlIdentity(SessionIndex)` |
| `.audio-graph-canonical.lock` | store-owned | `ReservedInternalIdentity` — **unchanged**, and by a different symbol |
| `.audio-graph-session-artifacts.v1.json` / `.tmp` | store-owned | `ReservedInternalIdentity` — unchanged |

The three store-owned names need no arm in the new enum, and one was deliberately
not added: they are the three root-wide constants `is_internal_identity` already
compares, so `validate_managed_identity` refuses them **earlier in the same loop**,
including on a deserialized entry that bypassed `new` (`ManagedArtifactIdentity` is
`#[serde(transparent)]`). `the_store_owned_coordination_lock_is_still_reserved_as_an_internal_identity`
pins that at the validator, in both ASCII-case spellings, so the claim is asserted
rather than asserted-by-reading. This is the same precedent as section 8 item 3: no
dead variant for an unreachable class.

**Case sensitivity: ASCII-case-insensitive, and yes deliberately.** The three
derived identities are matched with `ManagedArtifactIdentity::ascii_case_equivalent`
and the index name with `eq_ignore_ascii_case`. That matches all three of the
surrounding precedents — `is_internal_identity`'s `eq_ignore_ascii_case`, the
`CaseEquivalentManagedIdentity` `to_ascii_lowercase` fold in the same loop, and
`canonical_log::validate_recovery_identity_reservations` — because a case-variant
spelling names the same file on a case-insensitive filesystem. Both case variants
are asserted in
`candidate_inventory_reserves_this_sessions_control_identities_and_the_index`.

**The legitimate v2 provenance entry still passes, and that is the crux.**
`advance_session_semantics_v1_to_v2_inner` overwrites the candidate's provenance
entry identity with the store-derived one **before** it validates, and
`bind_v2_provenance_to_durable_proof` then requires that exact identity on every
later `proof_owned == false` V2 CAS. The entry is therefore mandatory at the control
identity for the whole life of an advanced Session, on the generic path too. Three
things narrow the exemption to it:

1. `kind == SessionArtifactKind::SessionProvenanceEvents`.
2. A **V2** floor — at which point `validate_v2_session_provenance` has already run
   earlier in the same function and proved the entry unique, `CanonicalSessionMemory`,
   `Present`, `transition.state == Completed`, and `transition.fingerprint ==
   content.sha256`. At a V1 floor nothing proves that label, so it is not trusted;
   `a_v1_candidate_cannot_claim_the_provenance_exemption_by_self_labelling` pins the
   refusal.
3. **Exact** equality, not case equivalence — the same carve-out shape
   `validate_recovery_identity_reservations` uses for its retry quarantine, because a
   case variant collides with the real durable proof on a case-insensitive
   filesystem.

`advanced_v2_head_keeps_its_provenance_entry_at_the_control_identity` proves the
legitimate case on all three surfaces: the accepted head's entry equals
`store.control_identities().provenance`, `store.load()` returns `Present` (the
**persisted** context accepts it), and `compare_and_swap(1, later_generation(…))` is
`Accepted` at generation 2 (the **generic** path accepts it). It then uppercases
that entry's identity and asserts
`Rejected(Validation(ReservedSessionControlIdentity(SessionProvenance)))`, which
also pins the ordering: validation runs before `refuse_unproven_v2_candidate`, so a
case-variant provenance identity reports the reservation, not
`V2ProvenanceProofBindingError::ProvenanceIdentityMismatch`.

**The strongest evidence that the exemption is right is the tests nobody had to
write.** Had the enforcement point or the exemption been wrong, the pre-existing V2
witnesses would have failed: the `advanced_v2_session(…)` tests in
`session_artifact_manifest.rs` and the five in `session_semantics.rs`, plus
`committed_v2_head_records_later_generations_only_against_the_durable_proof`,
`forged_v2_provenance_is_refused_on_an_advanced_head`,
`v2_candidate_requires_exact_bound_session_provenance_proof`, and
`quarantine_recovery_remains_closed_on_a_v2_session`. All stayed green untouched.

**One reclassification, disclosed and pinned.**
`canonical_log::RecoveryTransaction::manifest_candidate` rebuilds a candidate from
`head.artifacts` through `SessionArtifactManifestV1::candidate`, which hardcodes
floor V1. On a V2 head that rebuild inherits the provenance entry at the control
identity into a V1-floor candidate, which loses the exemption, so it now fails at
**construction** as `Validation(ReservedSessionControlIdentity(SessionProvenance))`
instead of reaching the CAS's `SessionSemanticsFloorRegression`. The path was
already closed either way — that is what
`quarantine_recovery_remains_closed_on_a_v2_session` documents — and no existing
test observed the old classification (`grep -n
"session_semantics_version\|SessionSemanticsVersion::V2\|advance_session_semantics"`
over `canonical_log.rs` returns zero hits). Two edits follow from it: that test's
rustdoc now states the production rebuild no longer reaches the CAS at all, and
`rebuilding_a_v1_floor_candidate_from_a_v2_head_is_refused_at_construction` pins the
new classification. That pin is scoped to the **constructor**, which is the step
that was reclassified; an end-to-end `canonical_log` recovery on a V2 head is not
constructible, because a recovery store comes from `qualified_for_algorithm_test`
with `address: None` and an unaddressed store cannot advance to V2 at all.

**Reuse, not a third literal.** `SESSIONS_INDEX_IDENTITY` moved from `const` to
`pub(crate) const` in place, so the manifest module reserves the same copy
`refuse_reserved_observed_identity` uses. Its rustdoc now names the caveat: this is
the one reserved name **not** derived from its writer — `user_data.rs` and
`persistence/mod.rs` each join the literal onto the data root and neither reads the
const, so a rename there silently unreserves it. Every other reserved name comes
from `session_control_address` or from the manifest module's own root-wide
constants, which *are* their writers' source.

The three control identities were kept **out** of `default_session_artifact_paths`,
so `default_artifact_inventory_covers_every_managed_session_file_and_temp` (18
paths), `purge_removes_all_expired_session_artifacts`, and
`session_has_any_artifact` keep their exact current authority and stayed green
untouched. That is itself the evidence that no control identity was smuggled into
the content path.

### (5) Windows reads compatible state and refuses v2 mutation

Verified and pinned, not built.
`windows_platform_reads_compatible_state_through_the_guarded_open` admits a
persisted V1 manifest under the shared guard on a Windows-forced store and refuses
a V2 one with `UnsupportedSessionFloor { V2, V1 }` before the reader runs.
`windows_platform_refuses_v2_mutation_before_any_side_effect` asserts
`begin_write()` is `Err(NamespaceQualificationRequired)` and the root gains no
entry, and that the same store still admits an entirely absent control plane as
`V1` while creating nothing.

---

## 4. Gates — verbatim result lines

Run from the worktree root, Rust 1.95.0, `--features cloud`, on Linux. First
captured at `41b3aa0`, then **re-run on both review-fix commits**. The counts below
were identical on all three runs — 270 / 1764 passing + 8 ignored / clean — and
only the wall-clock durations differed (the second review-fix run reported
`20.17s` and `56.87s` for the two test gates). The blocks below are the `41b3aa0`
capture; the durations in them are that run's.

```text
$ CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked \
    --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features \
    --features cloud -- -D warnings
    Checking audio-graph v0.1.0-rc.1 (/home/codeseys/DevBox/audio-graph/.worktrees/e8e7-guarded-admission/src-tauri)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.44s
```

```text
$ cd src-tauri && cargo +1.95.0 fmt --all -- --check && echo FMT_CLEAN
FMT_CLEAN
```

`fmt --check` prints nothing on success; `FMT_CLEAN` is the echo that proves the
command exited 0.

```text
$ CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked \
    --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
    --features cloud persistence -- --test-threads=1
test result: ok. 270 passed; 0 failed; 0 ignored; 0 measured; 1502 filtered out; finished in 20.38s
```

```text
$ CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked \
    --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
    --features cloud -- --test-threads=1
test result: ok. 1764 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 56.34s
```

```text
$ git diff --check && echo "diff-check-clean (exit 0)"
diff-check-clean (exit 0)
```

**Gate-filter blind spot, stated because it matters.** The authoritative filter
`persistence` matches neither `crate::sessions::tests` nor
`crate::commands::tests`. Four of this seed's new tests live there — three in
`sessions::tests` (`bootstrap_inventory_matches_the_live_session_artifact_inventory`,
`recovery_scan_ignores_session_control_residue`,
`permanent_delete_preserves_the_index_on_control_plane_residue`) and one in
`commands::tests` (`guarded_open_refuses_a_session_whose_control_plane_cannot_be_read`).
The **full `--lib` run above is the gate that covers them**, and it is reported
alongside the filtered one for exactly that reason. The 23 new `session_semantics`
tests do run under the `persistence` filter.

Test counts, derived rather than measured at the base: this seed adds 27 tests —
23 in `persistence::session_semantics::guarded_admission_tests` (the run above
reports `running 23 tests` for that module's filter), 3 in `sessions::tests`, 1 in
`commands::tests`. So `persistence` went 247 -> 270 and the full `--lib` suite went
1745 -> 1772 total (1764 passing + 8 ignored). The RED run at `1e8dfa9` corroborates
the full-suite figure: it reports `1745 filtered out` while running its 2 new tests,
i.e. 1747 total with 2 of the 27 present.

**Windows — unmeasured, and stated as such.** All four gates above ran on Linux;
no Windows run exists anywhere in this seed, so no claim about the
audio-graph-a58b Windows failure set is verified here. What *is* true by
inspection: **17 of the 27** new tests are `#[cfg(unix)]`-gated — every test that
needs real filesystem **identity** (a data root the durability substrate can
qualify), plus the two platform-forced Windows tests, which force
`CanonicalPlatform::Windows` on a Linux host because that is where the behaviour
under test is reachable.

The other **10 are ungated**, each because its contract is host-independent *by
intent*: 6 in `session_semantics::guarded_admission_tests` need only a plain temp
directory and no substrate identity
(`open_session_for_content_admits_a_bare_root_without_touching_it`,
`session_control_plane_paths_exclude_the_store_owned_lock`, and the four
`historical_bootstrap_*` builder tests); the remaining 4 —
`sessions::tests::bootstrap_inventory_matches_the_live_session_artifact_inventory`,
`sessions::tests::recovery_scan_ignores_session_control_residue`,
`sessions::tests::permanent_delete_preserves_the_index_on_control_plane_residue`,
and `commands::tests::guarded_open_refuses_a_session_whose_control_plane_cannot_be_read`
— use `HomeGuard` + `TEST_HOME_LOCK`, which is the audio-graph-0641 gap. Two of
those four obstruct a control path with a **directory at a control identity**, a
non-regular entry on every host.

Host-independent *by intent* is not the same as measured. The Windows behaviour of
all 10, and therefore whether the a58b failure set grew, is **untested**.

### 4.1 Gates at `098a674` (the R4 closure), verbatim

Same commands, same worktree, Rust 1.95.0, `--features cloud`, Linux. All six ran;
nothing below is inferred.

```text
$ CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 clippy --locked \
    --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features \
    --features cloud -- -D warnings
    Checking audio-graph v0.1.0-rc.1 (/home/codeseys/DevBox/audio-graph/.worktrees/e8e7-guarded-admission/src-tauri)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.29s
```

```text
$ cd src-tauri && cargo +1.95.0 fmt --all -- --check && echo FMT_CLEAN
FMT_CLEAN
```

```text
$ CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked \
    --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
    --features cloud persistence -- --test-threads=1
test result: ok. 278 passed; 0 failed; 0 ignored; 0 measured; 1502 filtered out; finished in 22.24s
```

```text
$ CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked \
    --manifest-path src-tauri/Cargo.toml --lib --no-default-features \
    --features cloud -- --test-threads=1
test result: ok. 1772 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 61.26s
```

```text
$ git diff --check && echo "diff-check-clean (exit 0)"
diff-check-clean (exit 0)
```

```text
$ python3 docs/agentic-runs/2026-08-18-audio-graph-e8e7/check-anchors.py
17 anchors enumerated, 17 extracted from prose, 17 verified
ANCHORS OK
```

**The deltas, reconciled rather than asserted.** The 270 / 1764 in the block above
was captured before `b238860`, which added one test, so the immediate baseline for
this commit is 271 / 1765. `098a674` adds **seven** tests, all in
`session_artifact_manifest::tests`: 271 + 7 = 278 and 1765 + 7 = 1772, which is
exactly what the two runs report. The seven are
`candidate_inventory_reserves_this_sessions_control_identities_and_the_index`,
`a_v1_candidate_cannot_claim_the_provenance_exemption_by_self_labelling`,
`the_store_owned_coordination_lock_is_still_reserved_as_an_internal_identity`,
`an_unaddressable_session_id_reserves_the_index_but_derives_no_control_identities`,
`the_generic_cas_refuses_an_inventory_entry_at_this_sessions_manifest_identity`,
`advanced_v2_head_keeps_its_provenance_entry_at_the_control_identity`, and
`rebuilding_a_v1_floor_candidate_from_a_v2_head_is_refused_at_construction`. Three
of the seven are `#[cfg(unix)]`, matching their neighbours, because they need a data
root the durability substrate can qualify. **No Windows run exists for any of
them**, exactly as for everything else in this seed.

The anchor checker earned its keep a third time. `098a674` inserted the
`ReservedControlIdentity` enum above `ManifestValidationError`, one refusal branch
inside `validate_and_normalize`, `refuse_reserved_control_identities` after it, and
seven tests at the end of the module — which shifted five of the seven
`session_artifact_manifest.rs` anchors. The pre-shift checker reported them as five
`expected … found …` failures and `12 verified`, exit 1 — the `is_internal_identity`
row, for one, reported `expected 'fn is_internal_identity(identity: &str) -> bool
{', found ''`, because its old line number now falls on the blank separator above
`validate_session_id`. Each was re-derived by symbol and the table and the checker were updated
together: 1478 -> 1508, 2077 -> 2107, 2451 -> 2608, 2665 -> 2822, 5098 -> 5264 (the
stale numbers are written bare here, without their paths, so this paragraph does not
itself become an unenumerated anchor). `152` did not move (it is above the inserted enum) and none of the
seven `session_semantics.rs` anchors moved, because `SESSIONS_INDEX_IDENTITY`'s
visibility changed in place and every other edit in that file is below line 996.

---

## 5. Scope amendments — reviewer authorization required

These three edits exceed the seed's fence and are stated plainly rather than
buried. `plan.md` section 1 carries the full authority argument and the STOP
posture for each.

| # | Edit | Consequence if refused |
| --- | --- | --- |
| A1 | `ArtifactUnavailableReason::HistoricalUnknown` appended in `session_artifact_manifest.rs`, on ADR-0038 section 8's explicit instruction | **STOP on acceptance (1).** No substitution of `NeverCaptured`/`Inaccessible`; no omission of the entry. |
| A2 | New public `SessionArtifactManifestStore::retire_owned_control_plane`, plus `addressed_control_identities`, `session_control_identities_for`, a `pub` on the already-existing proof-size ceiling, and one new recovery-key domain — all in one appended, clearly separated block | **STOP on acceptance (4)'s delete leg.** Inventory, export, and recovery parity still ship. Do NOT fall back to raw-unlink retirement. |
| A3 | One sentence of `unlink_canonical_entry`'s TRUST BOUNDARY rustdoc in `canonical_durability.rs`, which asserted it had exactly one production caller | Refuse A2 too; they stand or fall together. |

Nothing in A1–A3 changes the manifest module's CAS, validation, gate, or
classification logic. There is no `match` on `ArtifactUnavailableReason` anywhere
in the crate and no test pins its variant count; `validate_and_normalize` inspects
only `availability.content()` and the audio entry's `privacy_class`. An older
binary reading a `historical_unknown` value would return
`ManifestLoadError::Malformed`; no field is populated anywhere in production
today, so no downgrade population exists.

**Rebase-conflict sites** for the concurrent workstream that owns test cfg-gating
and rustdoc precision in `session_artifact_manifest.rs` and
`canonical_durability.rs`:

- `canonical_durability.rs` — the A3 rustdoc sentence. One sentence, inside an
  existing rustdoc block. This is the only place this seed edits existing prose in
  either file.
- `session_artifact_manifest.rs` — one variant appended to
  `ArtifactUnavailableReason`, one `pub` keyword on an existing `const`, and one
  appended block after `encode_lowercase_base32`. No existing rustdoc block and no
  existing test function was modified.

---

## 6. Residuals, with owners

| # | Residual | Owner |
| --- | --- | --- |
| R1 | **The `open_session_for_content` branch choice is made from an unlocked observation.** ADR-0038 section 5 fixes the branch by capability, not by observation. The floor itself is always admitted either under the shared guard or through the pre/post sandwich, so no unlocked look is an admitted floor — but the choice of branch is one. It is admissible only while nothing can concurrently create a control plane or a v2 artifact: at this base `advance_session_semantics_v1_to_v2` has no caller and `validate_artifact_semantics` has none outside its own module. A code comment on `open_session_for_content` states the constraint and names `guarded_session_open` as the single-call-site replacement. It is described as race-safe nowhere. **This is an explicit ADR-deviation decision for the human reviewer.** | whichever seed activates a v2 writer or a v2 artifact reader |
| R2 | **A data root carried onto a filesystem the substrate cannot qualify, WITH control residue present, becomes undeletable.** `retire_owned_control_plane` needs a qualified exclusive guard for a durable unlink; without one it reports `Residual`, and `permanently_delete_session` returns `Err` with the index entry preserved and no retry that can succeed at that root. Production cannot create that residue there — the only writer is `begin_write`, which needs the same qualification — so it requires a copied root. The fail-closed direction was chosen over a non-durable removal, whose resurrection of a manifest head would make it assert `Present` for artifacts `remove_artifact_paths` had already destroyed. | the platform-probe / filesystem-policy workstream |
| R3 | **The same root, WITH control residue, now refuses the READ** instead of admitting historical v1. `load_session` errors. Known user-visible consequence, fail-closed, documented in `plan.md` section 3.2. With no residue — every host today — behaviour is unchanged. | same as R2 |
| R4 | ~~**`validate_managed_identity` still does not reserve per-Session control identities.**~~ **CLOSED at `098a674`**, on the maintainer's 2026-08-19 decision to lift the scope fence that made it partial by construction. The reservation is now enforced at the manifest validator — `refuse_reserved_control_identities`, called from `validate_and_normalize` — which covers the generic `compare_and_swap` path, the transition path, `SessionArtifactManifestV1::candidate`, and the load path, and refuses this Session's manifest, temporary, and provenance identities plus `sessions.json` as `ReservedSessionControlIdentity`. `validate_managed_identity` and `is_internal_identity` were deliberately **not** changed and keep their exact prior meanings; the store-owned coordination lock is still refused by them, as `ReservedInternalIdentity`. Section 3.(4) records the enforcement point, the reserved set, the V2 provenance exemption, and the one disclosed reclassification. Two narrower residuals survive it, R10 and R11. | closed |
| R5 | **The production-qualification leg of `retire_session_control_plane` is not asserted end-to-end.** `permanent_delete_preserves_the_index_on_control_plane_residue` asserts the classified-refusal contract with a host-independent non-regular obstruction; asserting the *durable removal* through production `permanently_delete_session` would require the fixture data root to qualify, which is host-dependent, and no injection seam into `sessions/mod.rs` is in scope. The durable-removal contract is asserted against a test-qualified store in `session_semantics`. | whichever seed adds a data-root injection seam to `sessions/mod.rs` |
| R6 | **An orphan immutable proof is a reachable crash intermediate of retirement.** Crashing between the head unlink and the proof unlink leaves a proof with no head. It obstructs only a *future* advance of a *re-created* Session with the same id and a different transition id, and a retirement rerun clears it. The reverse order was rejected because a surviving V2 head with an absent proof is refused by every later CAS as `DurableProofAbsent`. | accepted by design; revisit with the writer-activation seed |
| R7 | **If a later reviewer prefers the control identities inside `default_session_artifact_paths`**, then `default_artifact_inventory_covers_every_managed_session_file_and_temp` (18 paths) and `purge_removes_all_expired_session_artifacts` must both change, and the interaction between an unqualifiable root and `begin_write` must be re-analysed. Flagged, not chosen. | reviewer decision |
| R8 | **Acceptance (2) is partial in production: canonical Session content is still reachable without the guarded floor.** `projection_replay_report_for_session` and `session_timeline` each call `repository.load_transcript_event_stream(session_id)` directly and are both reachable from `#[tauri::command]` entry points (`get_projection_replay_report_cmd` and `build_session_timeline_cmd`), through neither `checked_session_open`, `guarded_session_open`, nor `open_session_for_content`. Separately, `load_session_impl` and `session_export_bundle` are gated on their *transcript* fork only: their speaker-revision, projection-patch, materialized notes/graph and live-assist reads happen outside the admitted closure. Harmless at this base, because nothing writes v2; once a v2 writer is activated, a Session at floor V2 opened through `build_session_timeline_cmd` or `get_projection_replay_report_cmd` replays v2 revisions through v1-only `TranscriptLedger` / `MaterializedProjectionState` logic with no floor check — exactly the corruption acceptance (2) exists to prevent. Not wired here because the seed's scope is narrow callers; disclosed so it is not rediscovered as a surprise. | whichever seed activates a v2 writer, jointly with R1 |

| R9 | **The unqualified word "dormant" in two fence-blocked module headers is now false, and this seed could not fix it.** `session_semantics.rs`'s own header was corrected in this run's review-fix round, because this seed put `open_session_for_content` and `retire_session_control_plane` on production paths. The same wiring falsifies `session_artifact_manifest.rs`'s "This dormant deep module" and `canonical_durability.rs`'s "This dormant module": `open_session_for_content` calls `session_control_identities_for` **unconditionally** on every production transcript read, so the manifest module now executes in production; and `retire_session_control_plane` reaches `retire_owned_control_plane`, hence `unlink_canonical_entry`, so both modules now hold production call sites that perform filesystem unlinks — reached only when Session control residue exists, which no production writer creates at this base (see R2). Rustdoc in both files belongs to the concurrent workstream that owns rustdoc precision there, and the scope fence forbids editing existing rustdoc blocks in them, so the correction is named here instead of made silently. The repo's established meaning of "dormant" is "no production callers", and where it means something narrower it qualifies the word explicitly — `canonical_log.rs` says "deliberately does not replace any runtime writer yet" and scopes itself to "Dormant tail repair". Neither of these two headers carries such a qualification, which is why the unqualified word now misleads. | the concurrent workstream that owns rustdoc precision in `session_artifact_manifest.rs` and `canonical_durability.rs` |

| R10 | **The validator reserves only THIS Session's control identities, not another Session's.** `refuse_reserved_control_identities` derives the reserved set from `manifest.session_id`, so `.audio-graph-session-<other-key>-artifacts.v1.json` remains an admissible ordinary artifact at the validator. The bootstrap builder does refuse it, as `IdentityOutsideManagedArtifactTree`, and `control_plane_retirement_removes_all_three_and_nothing_else` still proves a second Session's entries survive retirement byte-identical, so nothing *destroys* them — the gap is that a manifest may name them. Closing it needs an address-independent `SESSION_CONTROL_PREFIX` ban, which must carry the same kind / V2 / exact-equality exemption or it will refuse the legitimate provenance entry. Deliberately not done here: the maintainer's task scoped the work to "this Session's own control identities", and the prefix ban is a strictly larger change with its own false-refusal risk. | whichever seed activates a cross-Session manifest producer |
| R11 | **A V2 `SessionProvenanceEvents` entry at the control identity could in principle be named as a quarantine SOURCE.** The exemption admits the entry; `validate_quarantine_transaction` does not additionally refuse `source_before.managed_identity == identities.provenance`, so a quarantine transaction naming the durable proof as its source would let recovery truncate the proof. Unreachable today, and doubly so: quarantine recovery is closed on V2 Sessions, and since `098a674` the production rebuild does not even construct (see section 3.(4)'s reclassification). A future quarantine-on-V2 activation must add that check to `validate_quarantine_transaction`. | whichever seed activates quarantine recovery on a V2 Session |

Explicitly out of scope and not touched: a fifth canonical stream; any
`SessionArtifactKind` member in either enum; broad e969 consumer migration;
writer/projection/provider/frontend/workflow/dependency activation;
`SessionExportBundle` fields and `src/types/index.ts`; Windows durability; the
audio-graph-a58b test-cfg gating; the audio-graph-0641 HomeGuard gap; installing a
bootstrap manifest in production; ADR changes.

---

## 7. Anchor verification

Every `path:line` anchor in this run's documents is enumerated in the table below
with the exact substring expected at that line. The checker
`docs/agentic-runs/2026-08-18-audio-graph-e8e7/check-anchors.py`:

1. extracts **every** `src-tauri/src/**.rs:line` occurrence in `report.md` and
   `plan.md` by regex, including ones outside the table;
2. fails if any extracted anchor is absent from the table, so an unenumerated
   anchor cannot slip through;
3. fails if the file's 1-indexed line does not contain the expected substring;
4. fails if any table row names a nonexistent file or an out-of-range line.

What that establishes and nothing more: that each cited line currently contains
the named symbol or code. It does not establish that the surrounding prose is a
correct reading of that code.

Result at the review-fix commit that carries this revision of the report:

```text
$ python3 docs/agentic-runs/2026-08-18-audio-graph-e8e7/check-anchors.py
17 anchors enumerated, 17 extracted from prose, 17 verified
ANCHORS OK
```

The checker earned its keep during the review-fix round rather than only in a
fixture: adding the R8 constraint note to `read_session_transcript_snapshot`'s
rustdoc pushed that anchor down 8 lines, and the checker failed with
`expected 'fn read_session_transcript_snapshot(', found '/// speaker-revision /
projection-patch / materialized / live-assist reads inside'` until the row and the
table were both moved from 6871 to 6879.

It earned it again in the second review-fix round, and this was observed rather
than assumed: replacing the `session_semantics.rs` module header pushed all seven
of that file's anchors down 11 lines, and running the **pre-shift** checker against
the edited source reported 14 failures — 7 stale enumerated rows (the
`admitted_session_semantics_floor` row, for one, reported
`expected 'pub fn admitted_session_semantics_floor(', found 'unsupported =>
Err(SessionSemanticsCorruption::UnsupportedSessionFloor {'` at its old line) plus 7
`cites unenumerated anchor` failures for the shifted prose rows, `10 verified`,
exit 1. The table and the checker were then updated together.

| Anchor | Expected substring |
| --- | --- |
| `src-tauri/src/persistence/session_semantics.rs:167` | `pub fn admitted_session_semantics_floor(` |
| `src-tauri/src/persistence/session_semantics.rs:200` | `if current == accepted {` |
| `src-tauri/src/persistence/session_semantics.rs:249` | `pub fn admit_session_semantics_v1_to_v2(` |
| `src-tauri/src/persistence/session_semantics.rs:402` | `pub fn guarded_session_open<T, E>(` |
| `src-tauri/src/persistence/session_semantics.rs:486` | `fn unguarded_absence_admission<T, E>(` |
| `src-tauri/src/persistence/session_semantics.rs:526` | `pub fn open_session_for_content<T, E>(` |
| `src-tauri/src/persistence/session_semantics.rs:996` | the `HISTORICAL_ORIGINAL_AUDIO_IDENTITY` constant and its value |
| `src-tauri/src/persistence/session_artifact_manifest.rs:152` | `HistoricalUnknown,` |
| `src-tauri/src/persistence/session_artifact_manifest.rs:1576` | `pub fn abandon_staged_transition(&self)` |
| `src-tauri/src/persistence/session_artifact_manifest.rs:2175` | `MissingOriginalSessionAudio` |
| `src-tauri/src/persistence/session_artifact_manifest.rs:2676` | `fn is_internal_identity(identity: &str) -> bool {` |
| `src-tauri/src/persistence/session_artifact_manifest.rs:2890` | `pub fn retire_owned_control_plane(` |
| `src-tauri/src/persistence/session_artifact_manifest.rs:5350` | `fn windows_other_session_transition_refuses_before_any_control_mutation()` |
| `src-tauri/src/persistence/canonical_durability.rs:1428` | `pub(crate) fn unlink_canonical_entry(` |
| `src-tauri/src/persistence/canonical_durability.rs:3780` | `const fn namespace_supported_for(platform: CanonicalPlatform) -> bool {` |
| `src-tauri/src/sessions/mod.rs:1679` | `assert_eq!(actual.len(), 18` |
| `src-tauri/src/commands.rs:6911` | `fn read_session_transcript_snapshot(` |

The checker was negative-tested during this run, not just run green. Two fixtures
were injected and reverted. (1) Appending an anchor for `commands.rs` line 1 to the
prose produced `ANCHOR FAIL: report.md cites unenumerated anchor <that anchor>` and
exit 1. (2) Shifting the `read_session_transcript_snapshot` table row by one line
produced `ANCHOR FAIL: <that anchor> expected 'fn read_session_transcript_snapshot(',
found 'repository: &FileMemoryRepository,'` and exit 1. The literal anchors are
paraphrased here so the fixtures do not become permanent findings of the checker
itself.

Line numbers drift on rebase. Prefer the symbol names, which are stable; the table
exists so a stale anchor fails loudly instead of silently misleading.

---

## 8. Deliberate deviations from the brief

1. **`HistoricalOriginalAudio::ObservedBytes` carries an `ObservedManagedArtifact`,
   not an `ArtifactContentIdentity`** (section 3.(1) above). The brief's shape
   would have made its own "no code path from a caller-supplied digest to
   `Present`" contract false for the mandatory entry.
2. **`audio` is in the observed-identity allow-list.** The brief's seven-entry list
   excluded it, which — as critique finding 6a noted — made the builder refuse its
   own mandatory entry once that entry came from a real observation. A collision
   between an observed `audio/...` identity and the builder-owned constant is
   refused by the manifest validator as `CaseEquivalentManagedIdentity`.
3. **No `StoreCoordinationIdentity` bootstrap error variant.**
   `ManagedArtifactIdentity::new` already refuses the coordination name as
   `ReservedInternalIdentity`, so the variant would have been unreachable. The test
   asserts that existing reservation instead, and a comment records that if it ever
   changed the name would land in `IdentityOutsideManagedArtifactTree`.
4. **`retire_session_control_plane` takes `data_root` + `session_id`, not a store**
   (critique finding 4). The brief's store-injected signature would have turned
   today's succeeding delete into `Err` on every unsupported filesystem with
   **zero** residue present. The store-injected leg survives as
   `retire_observed_session_control_plane` for tests.
5. **Retirement lives in the manifest module and acquires the exclusive guard
   directly rather than through `begin_write`** (critique findings 1 and 2). The
   brief's `store.begin_write()` step would have wedged delete forever on a
   truncated or session-mismatched head.
6. **`SessionFloorEvidence` has three variants, differently named and differently
   documented** than the brief's (critique finding 7).
7. **`permanent_delete_preserves_the_index_on_control_plane_residue` obstructs with
   a non-regular entry, not with an unqualifiable filesystem** (critique finding
   8a, residual R5). The brief's version was host-dependent: it passes on a tmpfs
   `/tmp` and fails on an ext4 `/tmp`, where retirement actually succeeds. Observed
   empirically during this run.
8. **`canonical_durability.rs` was edited** (one rustdoc sentence, amendment A3),
   which the brief's section 3 declared out of scope. Shipping the false "only
   production caller" sentence was the worse option.

### 8.1 Deviations in the R4-closure design brief (`098a674`)

The R4 brief was followed as written on every load-bearing point — enforcement
point, exemption shape, typed-error shape, RED plan — with three amendments, all of
which its own critique required or its own text offered as options.

1. **The reclassification pin is scoped to the constructor, not to an end-to-end
   `canonical_log` recovery.** The brief chose option (b), "pin it with a new test",
   without saying where. An end-to-end pin is not constructible: a recovery store
   comes from `qualified_for_algorithm_test`, which sets `address: None`, and an
   unaddressed store cannot advance to V2, so no V2 head can exist under one.
   `rebuilding_a_v1_floor_candidate_from_a_v2_head_is_refused_at_construction`
   pins the step that actually changed — the `SessionArtifactManifestV1::candidate`
   rebuild from `head.artifacts` that `manifest_candidate` performs — and its
   rustdoc states that scope rather than implying more.
2. **`quarantine_recovery_remains_closed_on_a_v2_session`'s rustdoc was rewritten**
   (critique finding 1). The brief's section 7 omitted it from the
   mandatory-rewrite list while its own section 4 disclosed the reclassification
   that falsified it. Its claim that the production rebuild "reaches this CAS
   through `compare_and_swap_recovery`" is no longer true on a V2 head, and a doc
   that misdescribes code is a blocking defect here. No assertion in that test
   changed; its V1 arms use `quarantine_candidate`, which carries no provenance
   entry, and its V2 arms are refused at `TransitionNotCompleted` /
   `CompletionRequiresPrepared`, both classified before or independently of the new
   check. It stayed green untouched.
3. **`plan.md`'s R4 cross-reference was updated rather than left or rewritten**
   (critique finding 2). `plan.md` is the design as authorized and its sentence —
   that the reservation lives at the bootstrap builder and not in
   `validate_managed_identity` — is still literally true, because the closure
   deliberately avoided that symbol. A short forward-state note was added under it
   pointing at report section 3.(4), so the pointer to a now-closed residual cannot
   read as the current state.

Also recorded, not a deviation: the brief's `SESSIONS_INDEX_IDENTITY` const gained
a rustdoc caveat naming `user_data.rs` as the coupled writer that does not read it
(critique finding 3). Two of the brief's navigation enumerations were off — its
`session_semantics.rs` anchor list omitted the `open_session_for_content` anchor,
and it counted six `advanced_v2_session` tests in the manifest module where there
were five — but neither changed any conclusion, and every anchor was re-derived by
symbol against the source rather than taken from the brief.
