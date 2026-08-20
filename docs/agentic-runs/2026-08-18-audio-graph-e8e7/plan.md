# audio-graph-e8e7 — guarded admission (amended design)

Base `222b2ad` on `integration/session-memory-wave-20260814`. Branch
`work/audio-graph-e8e7-guarded-admission`, worktree
`.worktrees/e8e7-guarded-admission`.

This plan is the design brief **as amended by every required change from the
adversarial critique**. Where the brief and the critique disagree, the critique
wins and the divergence is stated. Symbol names, not line numbers, are the
anchors; the report machine-checks every `file:line` anchor it cites.

**Post-implementation reconciliation.** Sections 3.2 and 4 were amended *after* the
code landed, at four places where the pre-implementation design did not match what
shipped: the `HistoricalOriginalAudio::ObservedBytes` payload, the `audio/`
allow-list argument (which was inverted — the design said a collision was
structurally impossible; shipped code relies on the manifest validator refusing it),
the removed `StoreCoordinationIdentity` classification, and the claim that
`open_session_for_content` is the only production read seam. Each amendment names
the report section that owns the deviation. Everything else in this file is
pre-implementation design and is the design of record only where the report does not
contradict it — the report wins on what shipped.

---

## 0. Corrected premises (carried from the brief, re-verified at this base)

1. **Two enums share the name `SessionArtifactKind`.** The live consumer enum in
   `persistence/mod.rs` has exactly twelve members and no audio member. The
   dormant manifest wire enum in `persistence/session_artifact_manifest.rs` has
   twenty-one members whose first is `OriginalSessionAudio`, and
   `validate_and_normalize` requires exactly one such entry with
   `privacy_class == OriginalEvidence`. Section-0 answer Q0.3 binds on the live
   enum: **no audio kind is added anywhere**, and the mandatory wire entry must
   still exist in every bootstrap manifest.
2. **The acceptance-(3) defect is the tail of `admitted_session_semantics_floor`,
   not its signature.** It already takes `&ManifestCasOutcome`. `exact_retry` is
   `true` only for `AlreadyCompleted`, so an `Accepted` outcome that *preserves*
   the floor (`current == accepted`) falls through to
   `IllegalTransition { V2, V2 }`. audio-graph-68a1 made that arm reachable in
   production. This is the one criterion with a clean behavioural RED, captured
   verbatim in the report.
3. **The kernel is dormant *at the base*.** `checked_session_open`,
   `admitted_session_semantics_floor`, and `validate_artifact_semantics` have no
   callers outside their own module; `qualified_existing_session` has none
   anywhere; there is no genesis path for a first manifest. **This premise is a
   statement about `222b2ad`, and this seed ends it:** `open_session_for_content`
   and `retire_session_control_plane` ship with production callers, and
   `qualified_existing_session` is called from both. `session_semantics.rs`'s
   module header records the post-seed truth; report R9 carries the two headers
   the scope fence blocked this seed from correcting.

---

## 1. Scope amendments requiring explicit human authorization

The seed fences `session_artifact_manifest.rs` to consumer-only use and declares
`canonical_durability.rs` out of scope. Three edits exceed that fence. They are
listed here so the reviewer authorizes or refuses each one; the report repeats
them verbatim under *Residuals and deviations*.

| # | Edit | Authority | If refused |
| --- | --- | --- | --- |
| A1 | `ArtifactUnavailableReason::HistoricalUnknown` appended to the wire enum | ADR-0044 §8 and its Consequences bullet: *"If the v1 manifest wire cannot represent that distinction, implementation stops for a reviewed schema refinement rather than choosing a plausible reason"* | **STOP on acceptance (1).** Do not substitute `NeverCaptured`/`Inaccessible` (both are fabricated histories ADR §8 names) and do not omit the entry (`MissingOriginalSessionAudio`). |
| A2 | New public `SessionArtifactManifestStore::retire_owned_control_plane` (plus a non-panicking `addressed_control_identities` accessor, a `pub` on the already-existing proof-size ceiling, and a new recovery-key domain) inside `session_artifact_manifest.rs` | Critique finding 1: raw `std::fs::remove_file` retirement is unshippable — no parent-directory barrier, no reserved-name/regular-file/identity fences, no recovery key. `unlink_canonical_entry` is `pub(crate)` on the guard and the transaction's guard field is module-private, so the only correct home is that module. | **STOP on acceptance (4)'s delete leg** for a scope amendment. Inventory/export/recovery parity still ships. |
| A3 | One sentence of `CanonicalExclusiveGuard::unlink_canonical_entry`'s TRUST BOUNDARY rustdoc in `canonical_durability.rs`, which today asserts *"Its only production caller is `ManifestWriteTransaction::abandon_staged_transition`"* | A2 adds a second production caller. Leaving the sentence would ship a doc that misdescribes the code, which this repo treats as a blocking defect. | Refuse A2 as well; the two stand or fall together. |

A3 touches an existing rustdoc block in a file a concurrent workstream owns. It
is one sentence, deliberately minimal, and flagged as a rebase-conflict site.

---

## 2. Acceptance (3) — the actual CAS outcome

### 2.1 Floor preservation

`admitted_session_semantics_floor`'s tail becomes:

```rust
if current == accepted {
    return Ok(current);
}
if current == SessionSemanticsVersion::V1 && accepted == SessionSemanticsVersion::V2 {
    return Ok(SessionSemanticsVersion::V2);
}
Err(SessionSemanticsAdvanceError::IllegalTransition { current, accepted })
```

`exact_retry` disappears; the destructuring binds only `manifest`. The retained
`Err` arm now covers exactly `accepted < current`, which the CAS already refuses
upstream as `SessionSemanticsFloorRegression`; it is kept as defence and one
comment line says so. The V2 provenance re-validation is **not** loosened — it
still runs for every `accepted == V2`, including the new preserve arm.

### 2.2 The admission wrapper

```rust
pub enum SessionSemanticsAdmissionError {
    Refused(ManifestCasRejection),
    DurabilityIndeterminate(CanonicalDurabilityIndeterminate),
    Floor(SessionSemanticsAdvanceError),
}

pub fn admit_session_semantics_v1_to_v2(
    transaction: &mut ManifestWriteTransaction<'_>,
    expected_session_id: &str,
    current: SessionSemanticsVersion,
    expected_generation: u64,
    candidate: SessionArtifactManifestV1,
    proof: SessionSemanticsTransitionProofV1,
) -> Result<SessionSemanticsVersion, SessionSemanticsAdmissionError>
```

The body calls `transaction.advance_session_semantics_v1_to_v2(..)` and passes
**that returned value by reference** to `admitted_session_semantics_floor`. The
outcome never leaves the function, so no caller can synthesise one. `Rejected` /
`DurabilityIndeterminate` are forwarded verbatim, carrying
`TransitionProofRefusedAfterIntentStaged` /
`ManifestInstallRefusedAfterProofAndIntentDurable` and their recovery keys.
`SessionSemanticsAdvanceError` is `Copy`; `ManifestCasRejection` is not, so the
new enum is `Clone` only. `SessionSemanticsAdvanceError::ManifestCasNotAccepted`
becomes unreachable *through this wrapper* but stays reachable for direct
callers — its rustdoc says so rather than the variant being deleted.

---

## 3. Acceptance (2) — guard-owned open

`checked_session_open` stays the inner floor primitive; its
`UnsupportedReaderFloor` / `InvalidSessionFloor` / `UnsupportedSessionFloor`
classification is already correct and is the acceptance criterion's named symbol.

```rust
pub struct AdmittedSessionFloor {
    pub floor: SessionSemanticsVersion,
    pub evidence: SessionFloorEvidence,
}

pub enum SessionFloorEvidence {
    GuardedManifest,
    GuardedAbsence,
    UnguardedObservedAbsence,
}
```

**Critique finding 7 amendment.** The brief's variant set was internally
inconsistent (`NoSessionControlPlane` did not exist; `GuardedHistoricalAbsence`
claimed "namespace-mutating platform" although a coordinated read-only store
reaches the same `Absent` arm; `ReadOnlyPlatformAbsence` claimed no v2 floor can
ever have been Accepted, which a torn copy falsifies). The three variants above
state only what was observed and under which guard:

- `GuardedManifest` — a manifest was selected and floor-validated while this call
  held the store's shared coordination guard.
- `GuardedAbsence` — no manifest was selected under that shared guard. It makes
  no platform claim.
- `UnguardedObservedAbsence` — the Session's manifest, temporary, and provenance
  identities and the store-owned coordination identity were all observed absent
  immediately before the content reader ran **and again before its result
  escaped**, with no guard held.

```rust
pub enum GuardedSessionOpenError<E> {
    UnsupportedReaderFloor { actual: u32 },
    InvalidSessionFloor { actual: u32 },
    UnsupportedSessionFloor { required: SessionSemanticsVersion, maximum_supported: SessionSemanticsVersion },
    ControlPlaneUnreadable(ManifestLoadError),
    ControlPlaneQualificationRequired,
    ControlPlaneStore(ManifestStoreError),
    ControlPlaneObservation(SessionControlPlaneObservationError),
    ControlPlaneAppearedDuringUnguardedRead,
    ContentReader(E),
}

pub fn guarded_session_open<T, E>(
    store: &SessionArtifactManifestStore,
    maximum_supported: SessionSemanticsVersion,
    content_reader: impl FnOnce(AdmittedSessionFloor) -> Result<T, E>,
) -> Result<T, GuardedSessionOpenError<E>>
```

Implementation is a pure consumer of `store.checked_read`. Inside that closure,
which runs while the shared guard is alive:

- `Present(m)` → `checked_session_open(&m, maximum_supported, ..)` then the
  reader with `GuardedManifest`;
- `Absent` → the reader with `GuardedAbsence` at `V1`;
- `CheckedManifestReadError::Load` → `ControlPlaneUnreadable`;
  `NamespaceQualificationRequired` → `ControlPlaneQualificationRequired`;
  `UncoordinatedAbsence` → the §3.1 unguarded sandwich.

Taking `&SessionArtifactManifestStore` is load-bearing: it lets tests inject
`qualified_for_test_session` and `for_test_session_platform(.., Windows)` stores
and exercise both ADR-0044 §5 branches on Linux.

**Rustdoc honesty requirement.** On the qualified branch `checked_read`
*establishes* the coordination entry when it is missing (it drops an exclusive
guard to create the lock, then re-acquires shared). The wrapper's doc says the
read path may create the store-owned lock file on Linux/macOS.

### 3.1 The ADR-0044 §5 sandwich (critique finding 3)

ADR-0044 §5 fixes two branches by capability and states *"An unlocked preflight
result alone is never an admitted floor"* and, for the read-only branch, that the
implementation *"checks the Session manifest and global coordination entry before
reading, builds the complete content snapshot without releasing bytes to the
caller, then checks both identities again immediately before that snapshot
escapes. Any appearance or change returns typed retry/refusal rather than v1."*

One private helper implements exactly that and is the only path to
`UnguardedObservedAbsence`:

1. observe the three Session-owned control identities plus the store-owned
   coordination identity; any one present → refuse (`ControlPlaneAppeared…`);
2. run the content reader and **hold** its value;
3. re-observe all four; any appearance, or any classification error, → typed
   refusal and the held value is dropped;
4. only then return it.

Observation uses `std::fs::symlink_metadata`. `NotFound` is the only condition
that means absence; every other `io::Error` is a classified
`ControlPlaneObservation` refusal. `Path::exists()` is deliberately not used
because it folds a permission error into "no residue → admit".

**Residual §5 deviation, surfaced for explicit reviewer decision.** The
`open_session_for_content` preflight in §3.2 decides *which branch to take* from
an unlocked observation. That is not an admitted floor — the floor is admitted
either under the shared guard or by the pre/post sandwich above — but it is a
capability decision made from unlocked bytes, where ADR §5 says the branch is
fixed by capability alone. It is admissible only while no production mutator and
no v2 artifact writer exists: at this base `advance_session_semantics_v1_to_v2`
has zero callers and `validate_artifact_semantics` has zero callers outside its
own module. A code comment states that constraint and names
`guarded_session_open` as the one-call-site replacement when either is activated,
and the report records it as a residual with an owner. It is **not** described as
race-safe anywhere.

### 3.2 `open_session_for_content` — the one production read seam this seed wires

```rust
pub fn open_session_for_content<T, E>(
    data_root: &Path,
    session_id: &str,
    maximum_supported: SessionSemanticsVersion,
    content_reader: impl FnOnce(AdmittedSessionFloor) -> Result<T, E>,
) -> Result<T, GuardedSessionOpenError<E>>
```

Lives in `session_semantics.rs` (so its tests run under the authoritative
`persistence` filter) and is called from `commands.rs`. Policy, in order:

1. Derive the three Session-owned identities and, **separately**, the store-owned
   coordination identity. No I/O, no store.
2. Observe all four. **All absent** → the §3.1 sandwich admits `V1` with
   `UnguardedObservedAbsence`. Zero store construction, zero mount scan, zero
   lock creation — byte-identical filesystem effect to today on every host, which
   is what keeps every existing `sessions`/`commands` test green.
3. **Any present** → build the store and route through `guarded_session_open`
   (critique finding 3: *route through `guarded_session_open` whenever a
   qualified store can be built*):
   - `qualified_existing_session` → `Ok` → `guarded_session_open`;
   - `Err(Qualification(NamespaceDurabilityUnsupported { .. }))` → `for_session`
     → `guarded_session_open`, i.e. the ADR §5 read-only branch;
   - any other refusal → `ControlPlaneStore(..)`, never an admitted floor.

`commands.rs`'s `read_session_transcript_snapshot` — the exact canonical-vs-legacy
fork shared by `load_session_impl`, `load_session_transcript`, and
`session_export_bundle` — has its body wrapped in this call with
`maximum_supported = V1`. `SessionExportBundle` gains no field; its hand-maintained
TS mirror stays untouched.

It is **not** the only production reader of canonical Session content, and this
seed does not make it so: `projection_replay_report_for_session` and
`session_timeline` read `load_transcript_event_stream` with no floor check, and the
non-transcript streams inside `load_session_impl` / `session_export_bundle` are read
outside the admitted closure. Narrow-caller scope leaves them where they are;
residual R8 in the report carries them with an owner.

Known user-visible consequence (brief failure mode 4): a data root carried from an
ext4/APFS host onto btrfs/xfs/zfs/overlayfs/tmpfs **while carrying real control
residue** now refuses the read instead of admitting V1. That is the fail-closed
direction. With no residue — every host today — behaviour is unchanged.

---

## 4. Acceptance (1) — historical bootstrap

```rust
pub struct ObservedManagedArtifact { /* private fields */ }
impl ObservedManagedArtifact {
    pub fn observe(kind, privacy_class, managed_identity, path: &Path)
        -> Result<Option<Self>, HistoricalBootstrapError>;
}

// SHIPPED AS `ObservedBytes(ObservedManagedArtifact)`, not the brief's
// `ArtifactContentIdentity` — see report section 8 item 1 for why.
pub enum HistoricalOriginalAudio { ObservedBytes(ObservedManagedArtifact), NoObservableBytes }

pub fn historical_session_bootstrap_candidate(
    session_id: &str, idempotency_id: &str,
    observed: Vec<ObservedManagedArtifact>, original_audio: HistoricalOriginalAudio,
) -> Result<SessionArtifactManifestV1, HistoricalBootstrapError>;

pub fn historical_session_bootstrap_from_live_inventory(
    data_root: &Path, session_id: &str, idempotency_id: &str,
) -> Result<SessionArtifactManifestV1, HistoricalBootstrapError>;
```

1. **`Present` is unforgeable.** `ArtifactAvailability::Present` is produced only
   from an `ObservedManagedArtifact`, whose only constructor streams the file and
   computes `sha256` / `byte_length` from exactly those bytes. There is no code
   path from a caller-supplied digest to `Present`.
2. **Regular-file fence (critique finding 6b).** `observe` calls
   `symlink_metadata` first and refuses a non-regular entry with
   `NonRegularObservedEntry`. Without it a symlink at `transcripts/<id>.jsonl`
   pointing anywhere would be hashed and recorded `Present` under a managed
   identity — an identity forgery the rest of the codebase fences.
   `Ok(None)` is returned for `NotFound` only; every other `io::Error` is
   `ObservedEntryUnreadable { kind, raw_os_error }`.
3. **Audio (critique finding 6a).** The live inventory contains no audio path, so
   `historical_session_bootstrap_from_live_inventory` always yields
   `NoObservableBytes` → the mandatory entry is
   `Unavailable { reason: HistoricalUnknown }` with
   `privacy_class: OriginalEvidence`. The builder owns exactly one audio
   identity, `audio/original-session-audio`: no extension, because no bytes were
   observed and claiming a container format would fabricate evidence.
   **RECONCILED WITH SHIPPED CODE (report section 8 item 2): the brief's argument
   below was inverted and is superseded.** `audio` IS the first entry of
   `MANAGED_ARTIFACT_ROOTS`, because excluding it made the builder refuse its own
   mandatory entry once that entry came from a real observation (critique finding
   6a). So an observed identity CAN start with `audio/`, and a collision with the
   builder-owned constant is **not** structurally impossible — it is refused by
   the manifest validator as `CaseEquivalentManagedIdentity`, which is the fence a
   later producer must rely on. Superseded text: *"the caller/observed allow-list
   in (4) applies only to caller-supplied observed identities, never to this
   builder-owned constant — which is why `audio/` is not in the allow-list and the
   builder can still emit its own mandatory entry. Because no observed identity may
   start with `audio/`, the builder identity can never collide with one, so
   `validate_and_normalize`'s case-fold uniqueness check cannot fire on it."*
   The `ObservedBytes` arm exists so the guarantee is
   structural rather than a comment, and a test hands the builder a real observed
   file.
4. **Identity reservation (feeds acceptance 4).** Every observed managed identity
   is a path relative to the data root and is refused unless its first segment is
   one of `audio/ transcripts/ projections/ notes/ graphs/ ledgers/ usage/
   live_assist/` (eight roots as shipped; `audio` was added per report section 8
   item 2). That rejects, by construction, every Session's flat-root control
   identity, `sessions.json`, and `.audio-graph-canonical.lock`. It matters
   because `validate_managed_identity` does **not** reserve them:
   `is_internal_identity` compares only the three root-wide constants, and
   `.audio-graph-session-<key>-artifacts.v1.json` passes every other check.
   Refusals are classified against **derived public** values, not against the
   module-private `SESSION_CONTROL_PREFIX`:
   `SessionIndexIdentity` (`sessions.json`), `SessionControlIdentity` (equal to one
   of *this* Session's three derived control identities), and
   `IdentityOutsideManagedArtifactTree` for everything else — which is where
   another Session's control identity lands. The brief's fourth classification,
   `StoreCoordinationIdentity`, is **not** in the shipped
   `HistoricalBootstrapError`: `ManagedArtifactIdentity::new` already refuses the
   coordination name as `ReservedInternalIdentity`, so the variant was unreachable
   (report section 8 item 3). Note that this reservation lives at the bootstrap
   builder, not in `validate_managed_identity`.

   *Forward state:* this plan is the design as authorized, and it is left as
   written. The gap that sentence describes was residual R4, and R4 is now
   **closed** — the maintainer lifted the scope fence on 2026-08-19 and the
   reservation was extended to the manifest validator, as
   `refuse_reserved_control_identities` inside `validate_and_normalize`. The two
   narrower residuals that survived R4, R10 and R11, are now closed as well, by
   audio-graph-f629; report section 9 carries the current state and supersedes
   section 3.(4) where they differ. One consequence for the paragraph above: the
   validator now also refuses ANOTHER Session's control identity, by an
   address-independent namespace ban, so `IdentityOutsideManagedArtifactTree` is
   no longer the only seam that catches it. This builder's classification is
   unchanged, and its allow-list is still the stronger rule for its own inputs.
   `validate_managed_identity` is still not the enforcement point, and
   deliberately so, so the sentence above remains literally true.
5. **Transition.** `state: Completed`, `quarantine_transaction: None`,
   `fingerprint = sha256` of a domain-tagged canonicalisation of the Session id
   plus the sorted observed inventory, so an unchanged rerun is byte-identical
   and therefore `AlreadyCompleted`. Floor is `V1`; the bootstrap never writes V2.
6. **No production caller, no CAS.** The bootstrap returns a candidate.
   Installing it is a `compare_and_swap`, which **stays uncalled in production**
   per ADR-0044 §11 and the seed's "no writer activation".
7. **Live inventory scope.** The bootstrap inventory covers the twelve durable
   per-Session artifacts with an explicit kind and privacy class each. The six
   `*.json.tmp` write sidecars in `default_session_artifact_paths` are interrupted-write
   residue with no kind in the manifest vocabulary and are deliberately excluded.
   A pinning test in `sessions/mod.rs` asserts that the bootstrap identities are
   exactly `default_session_artifact_paths` minus precisely those six sidecars, so
   either side drifting fails the gate.

---

## 5. Acceptance (4) — per-Session control-plane parity

`SessionControlIdentities` bundles the store-owned lock with the three
Session-owned identities, so no consumer may iterate it. The consumer surface
cannot express the lock:

```rust
pub struct SessionControlPlanePaths { pub manifest: PathBuf, pub temporary: PathBuf, pub provenance: PathBuf }
impl SessionControlPlanePaths { pub fn all(&self) -> [&Path; 3]; }

pub fn session_control_plane_paths(data_root: &Path, session_id: &str)
    -> Result<SessionControlPlanePaths, ManifestStoreError>;   // no I/O
pub fn store_coordination_path(data_root: &Path, session_id: &str)
    -> Result<PathBuf, ManifestStoreError>;                    // returned SEPARATELY, on purpose

pub struct SessionControlPlaneResidue { pub manifest: bool, pub temporary: bool, pub provenance: bool }
pub fn observe_session_control_plane(paths: &SessionControlPlanePaths)
    -> Result<SessionControlPlaneResidue, SessionControlPlaneObservationError>;
```

Both derive from the manifest module's new `session_control_identities_for`,
which wraps the existing private `session_control_address`; the Base32 encoding
is never duplicated and `sessions::session_id_is_valid` still gates it before any
path is derived.

### 5.1 Export (critique finding 5)

```rust
pub struct SessionControlPlaneExport {
    pub manifest: Option<SessionArtifactManifestV1>,
    pub proof: Option<Vec<u8>>,
    pub temporary_residual: bool,
}
pub fn export_session_control_plane(store: &SessionArtifactManifestStore)
    -> Result<SessionControlPlaneExport, SessionControlPlaneExportError>;
```

The manifest, the proof bytes, and the temporary's residue are all observed
inside one `store.checked_read` closure, i.e. under one shared guard. The lock is
never read, never exported, never touched. The temporary is reported as residue
and never exported.

Amendments over the brief:

- A **V2** manifest with an absent proof is `Err(V2ProofMissing)`, never
  `Some(manifest) + None`. ADR-0044 §3: *"A successful export contains the
  manifest and an available proof."*
- The proof bytes are **authenticated** before they are included: the file must
  be regular, within the canonical proof ceiling, exactly one canonical
  `SessionSemanticsTransitionProofV1` for this Session, and — for a V2 manifest —
  its digest and length must equal the manifest provenance entry's
  `sha256`/`byte_length`. Mismatch is `V2ProofContentMismatch`, not a silent
  export of unverified bytes. A V1 manifest with a proof file present must still
  authenticate as a canonical proof for this Session.

### 5.2 Retirement (critique findings 1, 2, 4)

Two layers:

**Durable primitive, inside the manifest module (scope amendment A2).**

```rust
pub struct SessionControlPlaneRetirementReport {
    pub temporary: CanonicalUnlinkOutcome,
    pub manifest: Option<CanonicalUnlinkOutcome>,   // None = not attempted
    pub provenance: Option<CanonicalUnlinkOutcome>, // None = not attempted
}
impl SessionArtifactManifestStore {
    pub fn retire_owned_control_plane(&self)
        -> Result<SessionControlPlaneRetirementReport, ManifestStoreError>;
}
```

- It acquires the store's **exclusive** guard directly and **never loads or
  validates the manifest head**. That is the reviewed escape critique finding 2
  demands: `begin_write` re-loads and re-validates the head, so a corrupt head or
  a head whose `session_id` mismatches the derived identity (a cross-session copy)
  would fail `ManifestStoreError::Load` on **every** retry and wedge
  `permanently_delete_session` forever. Retirement needs no head: authorization is
  the exclusive guard plus the three identities derived from the validated Session
  id.
- Each removal goes through `CanonicalExclusiveGuard::unlink_canonical_entry`,
  which crosses the qualified parent-directory barrier and applies the
  substrate's reserved-coordination-name, regular-file, canonical-parent, and
  open-handle identity fences. **No raw `std::fs::remove_file`.** The lock is
  refused by the substrate under every ASCII-case spelling, so it is
  unrepresentable twice over: once in `SessionControlPlanePaths`, once at the
  substrate.
- Recovery keys are derived, never caller-supplied. The temporary uses
  `temporary_abandon_recovery_key` — the **same** unlink of the same pathname
  under the same key domain as `abandon_staged_transition`, so an exact rerun of
  either reconciles the other. That is audio-graph-3cf2's substrate and its
  recovery-key semantics, as the seed requires, rather than a parallel temp path.
  The head and the proof use a new domain-separated
  `control_plane_retirement_recovery_key`.
- **Fixed order: temporary → manifest head → provenance proof**, stopping at the
  first step that does not reach durable absence (`Unlinked` or `AlreadyAbsent`).
  Named crash-intermediate states:
  - *after the temporary*: head and proof intact; the Session still reads and
    still advances; a rerun continues.
  - *after the head*: an **orphan immutable proof** with no head. The Session
    reads as absence (`V1`). A rerun retires the proof. The reverse order is
    strictly worse: a surviving V2 head with an absent proof is exactly the state
    `bind_v2_provenance_to_durable_proof` classifies as `DurableProofAbsent`, so
    every later CAS on that Session would be refused. The orphan proof only
    obstructs a *future* advance of a *re-created* Session with the same id and a
    different transition id (`ImmutableExactConflict`), which the same retirement
    rerun clears.
  - *after the proof*: nothing Session-owned remains; the lock and every other
    Session are untouched.

**Policy layer, in `session_semantics.rs`.**

```rust
pub enum SessionControlPlaneRetirement { Nothing, Retired, Residual { failures: Vec<String> } }
pub fn retire_session_control_plane(data_root: &Path, session_id: &str) -> SessionControlPlaneRetirement;
pub fn retire_observed_session_control_plane(store: &SessionArtifactManifestStore) -> SessionControlPlaneRetirement;
```

**Critique finding 4 amendment:** the production entry takes `data_root` +
`session_id`, **not** an already-built store. It observes first with **no store**
and constructs one lazily only when residue exists. The brief's store-injected
signature would have made every delete/purge on an unsupported filesystem
construct a store and fail `FilesystemUnsupported`, turning today's succeeding
delete into `Err` with **zero** residue present — the exact opposite of the "zero
behaviour change" the brief claimed. The store-injected leg survives as the
second function for tests, which pass `qualified_for_test_session` and
`for_test_session_platform(.., Windows)` stores.

Failure classification is exhaustive and truthful. `Residual { failures }` names
the reason for every reachable class: `NamespaceQualificationRequired` (no
qualification, e.g. Windows or an unsupported filesystem), `Qualification(..)`,
`Coordination(..)` (including `Contended` and `Missing`),
`InvalidSessionAddress`, and each per-entry `CanonicalUnlinkOutcome::Rejected` /
`DurabilityIndeterminate`. Messages carry the outcome's `Debug`, whose
`CanonicalRecoveryKey` renders `[REDACTED]`, so the fact of a recovery key is
reported and never its bytes. There is **never** an unguarded fallback unlink and
never a silent skip.

Known user-visible consequence for **delete**, stated here because the brief
stated it only for reads: on a data root the substrate cannot qualify
(btrfs/xfs/zfs/overlayfs/tmpfs, or Windows) **and with control residue actually
present**, retirement is a permanent `Residual`, so
`permanently_delete_session` returns `Err` and preserves the index entry with no
retry that can succeed at that root. Production cannot create that residue there
— the only writer is `begin_write`, which requires the same qualification — so
the state requires a copied data root. It is recorded as a residual with an owner
rather than resolved by a non-durable removal.

### 5.3 Narrow `sessions/mod.rs` wiring

The three control paths are **not** added to `default_session_artifact_paths`.
ADR-0044 §3 makes per-Session control lifecycle a separate authority *"in addition
to the manifest's managed artifact inventory"*. Keeping them out means the content
inventory and its delete allow-list keep their exact current authority,
`default_artifact_inventory_covers_every_managed_session_file_and_temp` (18 paths)
and `purge_removes_all_expired_session_artifacts` stay green **untouched** — which
is itself evidence no control identity was smuggled into the content path — and
`session_has_any_artifact` keeps its meaning, so control residue alone never makes
a Session "loadable".

- `permanently_delete_session`: after `remove_artifact_paths` and **before** the
  index removal, retire the control plane and fold `Residual { failures }` into the
  existing residual error, so the documented "index entry preserved for retry"
  contract covers control residue unchanged. Retirement is attempted only when
  content removal fully succeeded, so a content residue can never leave the head
  removed while content survives.
- `purge_expired_sessions_excluding`: the same fold into `residual_failures`; a
  Session with control residue is not added to `purged`.
- Nothing else. `collect_recovery_candidates` reads only `transcripts/`,
  `projections/`, `notes/`, `graphs/`, `usage/` and never the flat root, so
  control residue cannot resurrect or rename a Session. That becomes a pinning
  test, not a change.
- An unresolvable data root is recorded as a residual failure, never a silent skip.

---

## 6. Acceptance (5) — Windows

**No RED exists and none is claimed.** The platform gate already refuses:
`CanonicalFilesystemQualification::for_existing_managed_root` checks the platform
*before any filesystem access* and `namespace_supported_for` admits only Linux and
macOS, and `begin_write` refuses on missing qualification before acquiring the
lock. The mutation-refusal half is **already covered** by the existing
`windows_other_session_transition_refuses_before_any_control_mutation` in
`session_artifact_manifest.rs`; that test is credited, not claimed, and not
touched. This seed's contribution is the **read** half plus the end-to-end
assertion through the new seam. **No Windows durability is added** — the substrate
is Linux/macOS-only by design, proven by the 2026-08-18 real-hardware probes, and
production Windows correctly reports `NamespaceDurabilityUnsupported`.

---

## 7. Test plan

Gate-filter facts the report must repeat: the authoritative filter `persistence`
does **not** match `crate::sessions::tests` and does **not** match `commands`
tests. Every gate is therefore reported for the **full `--lib` suite** as well.
Every test needing real filesystem identity is `#[cfg(unix)]`-gated, and so are
both platform-forced Windows tests. Whether the audio-graph-a58b Windows failure
set grew is **unmeasured** — no Windows run exists anywhere in this seed, so this
section asserts no result about it. `report.md` section 4 owns the measured
position and enumerates every ungated test.

| Criterion | Test | RED |
| --- | --- | --- |
| 3 | `accepted_later_generation_preserves_the_committed_floor`, `accepted_v1_generation_preserves_the_v1_floor` | **behavioural RED**, verbatim in the report |
| 3 | `admission_passes_only_the_real_cas_outcome` | compile RED |
| 2 | `v2_session_bytes_never_reach_a_v1_only_content_reader` (plus the live contrast that bare `checked_read` *does* invoke its closure for that manifest) | compile RED + live contrast |
| 2 | `guarded_open_admits_historical_absence_only_under_the_guard` | compile RED |
| 2 | `legacy_and_canonical_readers_are_both_gated` | compile RED |
| 2 | `open_session_for_content_admits_a_bare_root_without_touching_it` (finding 8b: zero-residue byte-identical behaviour) | compile RED |
| 2 | `open_session_for_content_refuses_residue_on_an_unqualifiable_root` (finding 8b) | compile RED |
| 1 | `historical_bootstrap_records_present_only_from_observed_bytes` | compile RED |
| 1 | `historical_bootstrap_refuses_non_regular_observed_entries` (finding 6b) | compile RED |
| 1 | `historical_bootstrap_original_audio_is_unknown_not_fabricated` + `"historical_unknown"` wire round-trip | compile RED |
| 1 | `historical_bootstrap_refuses_control_and_index_identities` | compile RED |
| 1 | `historical_bootstrap_candidate_is_accepted_by_the_manifest_validator` (installs at V1, unchanged rerun is `AlreadyCompleted`) | compile RED |
| 1 | `bootstrap_inventory_matches_the_live_session_artifact_inventory` (`sessions/mod.rs`) | pin |
| 4 | `session_control_plane_paths_exclude_the_store_owned_lock` | pin |
| 4 | `control_plane_retirement_removes_all_three_and_nothing_else` (second Session + lock + `sessions.json` survive) | RED |
| 4 | `control_plane_temporary_is_retired_through_abandon` | RED |
| 4 | `control_plane_retirement_without_a_guard_is_a_residual_not_a_silent_skip` (Windows-platform store) | RED |
| 4 | `control_plane_retirement_ignores_an_unreadable_head` (finding 2's escape) | RED |
| 4 | `control_plane_export_carries_manifest_and_proof_but_never_the_lock` | RED |
| 4 | `control_plane_export_refuses_a_v2_manifest_whose_proof_is_missing` (finding 5) | RED |
| 4 | `permanent_delete_preserves_the_index_on_control_plane_residue` (`sessions/mod.rs`) | RED |
| 4 | `recovery_scan_ignores_session_control_residue` (`sessions/mod.rs`) | pin |
| 5 | `windows_platform_reads_compatible_state_through_the_guarded_open` | pin |
| 5 | `windows_platform_refuses_v2_mutation_before_any_side_effect` | pin |

**Critique finding 8a amendment.** `permanent_delete_preserves_the_index_on_control_plane_residue`
cannot assert a retry that succeeds *through a qualified retirement*: production
`permanently_delete_session` resolves its own data root, and a `/tmp` fixture root
is typically tmpfs, which never qualifies, and no injection seam into
`sessions/mod.rs` is in scope. The test therefore asserts the half that is real —
residue → `Err`, index entry preserved — and then clears the obstruction by
removing the residue directly and asserts the retry succeeds and the index entry
is gone. The report says exactly that rather than claiming a qualified retry.

---

## 8. Out of scope

Fifth canonical stream; any `SessionArtifactKind` member in either enum; broad
e969 consumer migration; writer/projection/provider/frontend/workflow/dependency
activation; `SessionExportBundle` fields and `src/types/index.ts`; the manifest
module's CAS, validation, gate, and classification internals and its existing
rustdoc/tests (the only edits there are the additive `HistoricalUnknown` variant
and the additive retirement/accessor block of amendment A2); Windows durability;
the audio-graph-a58b test-cfg gating; the audio-graph-0641 HomeGuard gap;
installing a bootstrap manifest in production; ADR changes.
