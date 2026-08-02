# Handoff: credential v2 and local-build throughput

Date: 2026-08-02

Epic: `audio-graph-a0f6`

Canonical branch: `work/audio-graph-cred-v2-integration`

Implementation head before this handoff: `89b18d93ee7f18437b5ad3fe8bf1eb195e8ca3f6`

## Pause boundary

This session intentionally stops after the active settings-transaction,
macOS-boundary, and Cargo-lane slices are resolved or durably classified. It
does **not** begin the runtime correction, settings persistence, module
visibility, Tauri AppState/IPC, frontend, provider leases, migration, optional
fallback, packaged-platform proof, or v1 retirement.

The next session must resume from this repository record and the Seeds queue,
not from remembered chat.

## Resume in one minute

1. Enter the canonical integration worktree:

   ```bash
   cd /home/codeseys/DevBox/audio-graph/.worktrees/credential-v2-integration
   git status --short --branch
   git rev-parse HEAD
   git log --oneline --decorate -8
   ```

2. Confirm the integration branch is clean. Do not substitute dirty `master`
   as the credential-v2 baseline.
3. Read the queue from the custody checkout:

   ```bash
   cd /home/codeseys/DevBox/audio-graph
   sd ready --format json
   sd blocked --format json
   sd show audio-graph-a0f6 --format json
   sd show audio-graph-34c9 --format json
   sd show audio-graph-c2be --format json
   sd show audio-graph-79e7 --format json
   ```

4. Before coding, write a new `docs/commit-state-*.md` for the chosen slice and
   update its Seed. Use an existing clean worktree when its custody is known;
   any new linked worktree belongs under
   `/home/codeseys/DevBox/audio-graph/.worktrees/<seed-or-branch>`.

Closing queue snapshot: `sd ready` reports 50 issues and `sd blocked` reports
78. Credential runtime `34c9` and build throughput `79e7` are `in_progress`;
settings persistence `c2be` is ready; module exposure `4c64` waits on `34c9`;
and dark AppState composition `bb2d` waits on `34c9` plus `4c64`. Re-read the
queue because these counts are deliberately a point-in-time handoff, not a
replacement for Seeds.

## Repository custody

- The main checkout at `/home/codeseys/DevBox/audio-graph` is broadly dirty
  user custody. It is the Seeds/control checkout, not an implementation base.
- Do not reset, clean, stage, commit, or wholesale copy its unrelated changes.
- Do not run `sd sync`; unrelated staged/dirty work could be swept into a
  commit. Seed updates in `.seeds/issues.jsonl` remain intentionally unsynced.
- The canonical credential worktree is
  `.worktrees/credential-v2-integration` on
  `work/audio-graph-cred-v2-integration`.
- `/.worktrees/` is gitignored. All future AudioGraph worktrees stay there;
  do not create new AudioGraph worktrees under `/tmp` or an external project
  directory.
- Snapshot at handoff: 42 linked worktrees total, 30 project-local and 11
  pre-existing external non-main worktrees. Seed `audio-graph-5f2d` owns a
  later evidence-based cleanup. Delete or move none without explicit approval.
- There are 33 `/tmp/*audio-graph*target*` directories. The earlier review
  measured about 178 GiB across the fragmented target trees. Some are active
  or are the only warm evidence lane; no cache was deleted.

## Product naming decision

Keep **AudioGraph** as the product, repository, engine, and stable technical
identity. Do not rename the package, crate, binary, bundle id, filesystem
roots, repository, keychain service, credential accounts, protocol fields, or
telemetry during credential v2.

**Aria** can be tested later as display-only assistant copy, such as “Ask
Aria” or “Aria Live.” Do not expand A.R.I.A. into a technical identity.
“Adaptive Realtime Intelligent Assistant” is crowded, conflicts conceptually
with WAI-ARIA and existing assistants, and overstates the realtime sibling at
the expense of AudioGraph's durable transcript/notes/temporal-graph core.

The reviewed decision is Seed `audio-graph-16fc` and
`docs/designs/2026-07-31-product-naming-audio-graph-aria.md`. It is a product
recommendation, not trademark clearance.

## Why credential v2 is still dark

The July 31 plan explicitly kept Wave 3 dark until AppState/IPC work. It also
combined the production native adapter with optional plaintext file-v2,
filesystem qualification, ACL/durability labs, and three-platform prompt proof.
That made an optional fallback block the ordinary native-store path.

The corrected sequence is recorded in
`docs/plans/2026-08-01-native-credential-vertical-resequence.md` and ADR-0036:

1. Finish and review the backend-owned service and native boundaries.
2. Compose the native service in Rust while keeping renderer responses
   redacted.
3. Cut provider consumers to scoped leases.
4. Migrate legacy keychain/YAML/inline inputs with exact readback and
   tombstones.
5. Prove packaged Windows, macOS, and authorized Linux behavior.
6. Retire v1 only after the v2 cutover is proven.
7. Decide Stronghold versus custom file-v2 separately, if a fallback is still
   needed.

Production still uses v1 today. `credentials/mod.rs` declares v2 dark; startup,
commands, readiness, and providers still call the flat v1 `CredentialStore`
and globally hydrate secret-bearing runtime settings. No AppState owns a
production `CredentialService`, and no v2 command is registered. This is the
next product gap, not evidence that the backend work is absent.

## Fixed credential architecture

These are settled guardrails unless a new ADR deliberately supersedes them:

- Rust owns long-lived credential state, provider sockets, policy, native
  calls, and secret use. React is configuration, control, and display.
- React may send an ephemeral replacement draft. Rust returns only typed
  status, revision, receipt, migration, and recovery metadata. Saved plaintext
  secrets never return through IPC.
- Provider runtimes obtain a scoped Rust-only `CredentialLease` for one closed
  set, consumer, purpose, and audience. No whole-store hydration remains after
  cutover.
- Passive status is a cached/journal projection with zero native, filesystem,
  unlock, or prompt I/O.
- One serialized backend worker owns credential calls. Timeout never cancels a
  started native call or permits overtaking; uncertainty remains stalled until
  exact completion, reconciliation, or restart.
- Background operations are `ForbidPrompt`. Only explicit user diagnosis or
  unlock can receive `AllowPrompt`, and it performs a separate no-prompt
  verification afterward.
- Native failure is typed and content-free. Native prose, locators, paths,
  secret bytes, and panic payloads do not enter errors, logs, status, events,
  crash reports, screenshots, docs, or Seeds.
- Secret bytes use the OS store; the non-secret authority journal is ordinary
  app-local atomic metadata. Ambiguous write/readback becomes
  `commit_unknown`/recovery-required, never fabricated success.
- A settings-plus-credential activation freezes the exact validated
  non-secret draft, operation, authority, set, credential revisions, and
  settings revisions into one closed transaction. Stale clones, swapped
  drafts, foreign authority, backward stages, and every uncertain cut fail
  closed.
- Legacy YAML is import-only on the native v2 path. Migration is explicit:
  inspect, plan, intent, active write, exact readback, journal commit, then
  quarantine/cleanup. Tombstones prevent resurrection.
- File-v2 is never auto-selected after native failure. Both compiled support
  tables remain empty, so it is not authorized for release.

Primary architecture records are ADR-0035, ADR-0036,
`docs/security/credential-service-threat-model.md`, and the two credential
plans under `docs/plans/`.

## Tauri and Rust library decisions

The research is durable in
`docs/research/2026-07-31-credential-service-library-evaluation.md`.

| Library or facility | Decision |
| --- | --- |
| `keyring` / `keyring-core` | Retain for the existing facade and typed entry seam. Use explicit platform boundaries where strict prompt/persistence behavior cannot be enforced by the generic facade. |
| Windows Credential Manager | Direct WinCred boundary with Local persistence, exact binary readback, typed numeric errors, and no CredUI. |
| macOS Keychain | Direct checked Security.framework boundary that saves, disables, and exactly restores the process interaction state; explicit unlock only. |
| Linux Secret Service | Stock `secret-service` 5.1.0 and `zbus-secret-service-keyring-store` 1.0.0 cannot satisfy deferred prompt ownership. Linux v2 stays dark until a user-approved immutable narrow fork revision exists. |
| Tauri managed state | Use later for one long-lived `Arc<CredentialService>`. Tauri command capabilities are not secret authorization. |
| Tauri filesystem/path APIs | Useful for portable app paths and scoped frontend file access, not as a credential backend. Rust-side service I/O uses Rust filesystem APIs. |
| Tauri Stronghold | Deferred optional fallback candidate. It changes the model to an encrypted vault whose password/master-key, recovery, backup, and unlock UX AudioGraph must own. |
| Tauri Store | Never for secrets or authority journal. It remains suitable only for ordinary non-secret UI preferences. |
| Renderer-facing keyring plugins | Rejected because caller-selected get/export/account APIs invert the backend-owned authority boundary. |
| `secrecy` and `zeroize` | Use for explicit exposure and short-lived redacted containers; do not claim erasure of every OS/TLS/provider copy. |
| File locking and atomic metadata | The later accepted decision supersedes the provisional `fs4`/`atomic-write-file` recommendation: use Rust 1.95 `File::try_lock()` behind a monotonic deadline and an AudioGraph-owned stage-aware same-directory replacement wrapper with platform proof. |
| `cap-std` | Optional path-hardening aid, not a credential backend and not needed on the native critical path. |

Seed `audio-graph-1500` owns the later Stronghold-versus-custom-fallback
decision. The large filesystem/durability Seed lane is preserved at P3 and
must not block native v2.

## What is built and integrated

The integration history below is authoritative. All entries are ancestors of
the pre-handoff implementation head.

| Commit | Work | State |
| --- | --- | --- |
| `b0d22b5`, `6e2ed0f` | Pin all target-specific `rsac` entries and lockfile to v0.4.4 full SHA `ea2019bba217cab695d45696bc2ca25430b23dc2`; fix `requires_user_consent`; make locked commands canonical | Locally green; Seed `audio-graph-fd9f` stays open for Windows/macOS capture CI and release attestation |
| `42ea2b6` and later adapter foundations through `0a931be` | Typed contract, service core/fake, origin policy, non-secret journal, native-keyring authority, filesystem policy/detectors, diagnostics foundations | Integrated and still dark |
| `8be073f` | Sealed native interaction gate and explicit recovery port | Reviewed, integrated; Seed `audio-graph-0350` closed |
| `3d8a617` | Direct noninteractive Windows Credential Manager boundary | Reviewed dormant source integrated; assembly/package evidence remains under `12c4`/`2aa8`/`c4c5` |
| `78c4973` | Redacted credential-boundary panic containment | Reviewed integrated foundation; whole-runtime/package evidence keeps `e004` open |
| `54d3eab` | Guarded macOS Keychain boundary | Reviewed dormant source integrated; Apple compile/package evidence keeps `07cb` open |
| `89b18d9` | Closed settings authority transaction plus systemic stalled-worker observation | Two independent reviews shipped; integration service 64/64 and credential 246 pass + 1 ignored; Seeds `8849` and `7cc6` closed |

The current service includes typed status/revisions, mutation receipts,
idempotency, present/tombstone envelopes, exact readback, non-secret authority
journal, events, scoped leases, settings activation/recovery, failure injection,
and deterministic concurrency tests. Windows and macOS boundary source exists
but is deliberately not declared/selected until the assembly slice.

The `rsac` v0.4.4 upgrade is worth retaining independently of credentials. It
includes upstream macOS input-only microphone and Process Tap fixes,
Linux/Wine matching fixes, removal of package-manager execution from
`build.rs`, Windows application-capture silence repair, macOS watcher leak/race
repair, and removal of the unused experimental SampleRing feature. It does not
itself fix the old `requires_user_consent` fixture skew; that correction landed
with the baseline.

## What is not built yet

- No production runtime worker is accepted; candidate `96e8141` for
  `audio-graph-34c9` was blocked on panic-hook containment, timeout/release
  coherence, post-ready failure gating, and pre-admission activation
  validation.
- No filesystem-backed revisioned settings activation port (`c2be`).
- No committed runtime module declaration (`4c64`) or dark AppState
  composition (`bb2d`).
- No native factory/diagnosis assembly (`2aa8`). Windows/macOS source is
  dormant; Linux lacks an authorized implementation.
- No v2 status/replace/delete/migration/recovery IPC (`f107`) or new Settings
  and onboarding UX (`2c33`, `cae3`).
- No provider consumer uses scoped v2 leases (`0c3d`, `0fc2`, `84be`, `7dfb`).
- No legacy v1/YAML/inline migration (`86e9`, `b14d`, `f70b`).
- No semantic cutover (`54e7`, `c826`), v1 retirement (`5f75`), packaged
  three-platform proof (`c4c5`), or approval-gated CI enforcement (`0ff1`).
- No optional fallback has been selected or authorized.

## Platform truth table

| Platform | Current honest state | Remaining proof |
| --- | --- | --- |
| Windows | Reviewed direct WinCred source integrated and dormant | `2aa8` assembly, target-native compile, persistence/locked-store/package matrix |
| macOS | Reviewed guarded Keychain source integrated and dormant | `2aa8` assembly, Apple compile/link, 20-test assembled filter, signed/package prompt/unlock matrix |
| Linux | Generic v1 facade exists; strict v2 implementation is unsupported/dark | User-approved immutable deferred-prompt fork, scripted order tests, GNOME Keyring/KWallet package evidence |
| file-v2 | Detectors/policy core exist, but both release support tables are empty | Decide need/architecture under `1500`; do not enable based on detector code alone |

`audio-graph-8171` records the exact Linux adapter contract. Research proved no
maintained official release exposes both deferred prompt ownership and per-call
`NoAutoStart`. Do not invent or publish a remote. Without an approved fork
revision, report Linux v2 unsupported and make no native/file fallback switch.

Before implementing `2aa8`, reconcile its combined dependency shape: Windows
and macOS composition should be able to proceed under target `cfg` while Linux
explicitly stays dark, instead of letting fork authorization repeat the old
file-v2 critical-path mistake. The recommendation is recorded in the Seed; do
not silently rewrite its acceptance graph.

## Local Rust build throughput

The measured problem is target fragmentation and uncoordinated concurrent
Cargo processes, not a lack of Cargo parallelism:

- Host: six logical CPUs.
- No prior Cargo `build.jobs` or `CARGO_BUILD_JOBS` override; three builds could
  collectively request about 18 rustc jobs.
- Rust incremental/source/dependency caches work inside a target directory.
- The final monolithic `audio_graph` test-crate rustc was observed around
  3.7 GiB RAM and 111% CPU, so more Cargo jobs do not speed that phase.
- `--test-threads=1` limits test execution, not compilation.
- Many correction/review cycles created fresh 8–9 GiB target trees instead of
  reusing the same worktree/feature/profile lane.

Manual operating policy now, until the repository facade is corrected and
accepted:

- One stable target directory per canonical worktree + feature set + profile.
- Reuse it through implementation, correction, and ordinary review.
- One active build may use the six-job host budget; two builds target roughly
  three jobs each; three builds target two each.
- Serialize full/default-feature and final clean-room gates.
- Run the cloud fast gate first for provider/settings work:

  ```bash
  cargo check --locked -p audio-graph --lib --no-default-features --features cloud
  ```

- Create exactly one fresh `mktemp` target only when independent clean-room
  evidence is an explicit acceptance requirement.
- Never delete caches merely because they are large; first prove the owning
  lane/process is inactive and obtain explicit authorization.

Neither `audio-graph-79e7` candidate is accepted; do not integrate either one.
The first candidate `fdb4923` failed because it could release or stale-reclaim
CPU tokens while compiler descendants survived, could spawn before durable
PID/process-group registration, and lacked auditable Windows descendant
ownership.

Replacement candidate `d33cc4b8c88ec346161eb8703973a7ddc7b659b2`
(tree `9c4d0d7c5bfb835e2fb5aaa34912b90fa53b70cc`) closes those lifecycle
defects, implements and tests the six-CPU 6/3/2 allocation, preserves exclusive
modes and stable lanes, and fails closed on Windows before side effects. Its
final specification review returned SHIP. Its final standards review returned
**BLOCK** for two remaining defects:

1. NUL-bearing prefix arguments can make the registered-child spawn throw
   synchronously outside the sanitization boundary, leaking caller-controlled
   text and an absolute source/worktree path. Reject those arguments before
   side effects, contain every registered-child top-level failure, and prove
   canary/path redaction plus safe lease release.
2. Several tests enter their cleanup `try/finally` only after waiting for a
   startup marker. A setup timeout can therefore orphan the facade, wrapper,
   or fake Cargo process. Establish process ownership immediately after spawn
   and terminate/await all outstanding children from teardown.

After that correction, rerun both immutable review axes. Keep Seed `79e7` open
even if the source lands: Windows needs a Job Object or equivalent auditable
descendant owner, and real Cargo no-op, authorized one-file rebuild, and exact
lane disk-growth evidence are still absent.

Follow-up build Seeds:

- `audio-graph-e7ee`: trial a safe shared compiler cache such as sccache after
  the lane coordinator is accepted; do not commit a wrapper unless bootstrap
  guarantees it exists.
- `audio-graph-245d`: benchmark `debug = "line-tables-only"`, Cargo timings,
  and the 130k-line root-crate critical path before splitting crates.
- `audio-graph-dc39`: approval-gated compatible CI/release cache sharing; no
  workflow mutation from the dirty checkout.
- `audio-graph-5f2d`: later worktree/target cleanup with explicit custody and
  recovery evidence.

## Required working method

- Seeds are the durable queue. Create or update a focused Seed before
  non-trivial implementation, research, CI, or UX work. Every meaningful
  finding becomes an extension, a child Seed, or a justified closure.
- Begin broad work with `sd ready --format json` and
  `sd blocked --format json`; parse `.issues`. Use the repo helper if output is
  capped or malformed.
- Create a commit-state document before a large slice: exact HEAD/branch,
  dirty-tree caveats, active Seeds, known gates, scope, and exclusions.
- Research load-bearing provider, platform, CI, packaging, and architecture
  unknowns from primary sources before coding; record source/date and update
  Seeds when the direction changes.
- Parallelize file-disjoint work only. Every worker owns named files or one
  bounded responsibility and knows other work is present. Reviewers are
  immutable/read-only; the conductor owns fan-in, conflicts, and queue hygiene.
- Use TDD for semantic changes: deterministic RED at the actual seam, bounded
  GREEN, focused suite, then broader gates. A compile success is not runtime or
  packaged proof.
- Review every candidate against a fixed base and immutable commit/tree, along
  both specification and repository-standard lenses. A green suite does not
  override a semantic BLOCK.
- Integrate only accepted candidates, verify the merge-base footprint and blob
  provenance, preserve unrelated accepted landings, and rerun gates on the
  assembled branch.
- Keep workflow/release edits approval-gated. Do not hide unfinished work in
  prose or close a Seed because only local source evidence exists.
- Do not expose stored secrets to React, log native errors, enable plaintext
  file-v2, bypass prompt policy, or silently use v1 as fallback for a selected
  v2 path.

## Recommended next waves

### Wave A: finish the backend vertical

1. Correct `audio-graph-34c9` from the current integration baseline. The
   existing blocked worktree is `.worktrees/34c9-credential-runtime` at
   candidate `96e8141`; its preservation tag is recorded in the Seed. Required
   correction:
   - wrap the entire worker dispatch in the integrated
     `catch_redacted_credential_panic` boundary;
   - serialize timeout marking and worker release under one coherent state
     transition so `Idle` cannot coexist with stale `Stalled`;
   - make post-ready Locked/AccessDenied/RecoveryRequired failures retain a
     cached recovery gate and reject ordinary retries with zero I/O;
   - validate activation shape before admission/backend open; and
   - preserve late-result/event exactly-once behavior.
2. In parallel only if file ownership remains disjoint, implement `c2be` from
   the reviewed `8849` transaction. This is non-secret settings persistence,
   not credential file-v2. It must use revision fencing, atomic pending marker,
   exact readback, rollback, restart recovery, and no ambient draft registry.
3. Independently review and integrate both candidates. Re-run runtime/settings
   interactions after fan-in.
4. Land `4c64`, the minimal module declaration that makes the reviewed runtime
   reachable without native I/O, then `bb2d` for one dormant AppState-owned
   service. Keep commands dark until composition is proven.
5. Reconcile/split `2aa8` so Windows/macOS target assembly can advance while
   Linux remains explicitly unsupported pending `8171` authorization.

### Wave B: expose a redacted product path

1. `f107`: versioned status, replace, delete, migration, and explicit recovery
   commands. No saved-secret get/export or renderer-selected locator/destination.
2. `2c33`/`cae3`: Settings presence/save/delete and startup/onboarding use
   coherent revisioned status. React holds replacement text only until invoke
   settles, then clears it.
3. `0c3d`, `0fc2`, `84be`, then `7dfb`: move ASR/TTS, LLM/realtime, AWS, and
   readiness/catalog/probe consumers from hydrated settings to scoped leases.
4. `54e7`/`c826`: prove the full lease cutover and rollback boundaries.

### Wave C: migrate and retire

1. `86e9` plus `b14d`/`f70b`: import exact legacy native/YAML/inline values,
   verify readback, commit tombstones, and redact old settings only after
   verified import. Never merge partial AWS fields from different snapshots.
2. Run `c4c5` packaged Windows/macOS/Linux evidence for every claim actually
   enabled. Unsupported Linux remains an honest result until its adapter is
   authorized and proven.
3. Retire v1 only through `5f75` after `c826` and platform evidence.
4. Apply `0ff1` CI/release enforcement only with explicit workflow approval.

## Verification and artifact index

Important local artifacts from this run are supplementary; the handoff and
committed sidecars carry the durable decisions even if `/tmp` is later cleared.

The handoff itself passes `git diff --check` and was not named by the docs/Seeds
secret-hygiene scanner. The repository-wide scanner still exits 1 on six
pre-existing unrelated findings: `.seeds/issues.jsonl` lines 377, 378, and 462;
`docs/plans/2026-07-03-provider-arch-plan.md` line 44; and
`docs/reviews/2026-07-05-credential-mechanism-review.md` lines 211 and 386.
Existing Seeds `cd02`/`c335` carry that history; do not misreport this baseline
failure as a handoff or credential-v2 regression.

- `audio-graph-0350` integration:
  `/tmp/audio-graph-artifacts/2026-08-02/audio-graph-0350-integration.md`,
  SHA-256 `7334722b8b9c96e8fd0596b18dcd1710b11154220960f364fda514ae18b1b29f`.
- Windows integration:
  `/tmp/audio-graph-artifacts/2026-08-02/audio-graph-12c4-windows-integration.md`,
  SHA-256 `5b38be1cb8136c3825a222ec5b2b3ccd0469c38b5137cf3e2a9521f2c5caa6d8`.
- Panic-containment integration:
  `/tmp/audio-graph-artifacts/2026-08-02/audio-graph-e004-panic-integration.md`,
  SHA-256 `33dedaa507916106d4909acc0d0ad9164fb0256240b563c2b655dcd695ceffae`.
- macOS implementation/review/integration:
  `/tmp/audio-graph-artifacts/2026-08-02/audio-graph-07cb-implementation.md`,
  `audio-graph-07cb-review.md`, and `audio-graph-07cb-integration.md`; final
  integration artifact SHA-256
  `2f7b117c0706ee11bdaf2c3bff6d511af63c835cff0c6bb823c9192923ea8c29`.
- Settings sixth correction and two reviews:
  `/tmp/audio-graph-artifacts/2026-08-02/audio-graph-8849-sixth-*.md`.
- Settings integration:
  `/tmp/audio-graph-artifacts/2026-08-02/audio-graph-8849-integration.md`,
  SHA-256 `e1d0582c155ffbbd1c72adc076bc5963779f9e703077bf1d9cd95b6729207e8f`.
- Blocked first throughput candidate and formal reviews:
  `/tmp/audio-graph-artifacts/2026-08-02/audio-graph-79e7-build-throughput.md`,
  `audio-graph-79e7-spec-review.md`, and
  `/tmp/audio-graph-reviews/2026-08-02/79e7-standards-review.md`.
- Blocked replacement throughput candidate and final reviews:
  `/tmp/audio-graph-artifacts/2026-08-02/audio-graph-79e7-correction.md`,
  `audio-graph-79e7-final-spec-review.md`, and
  `audio-graph-79e7-final-standards-review.md`; correction artifact SHA-256
  `7bfa1246fb06685514ab773056d735a7947ebe7558a586f879c502854a6d7bc5`,
  final Spec SHA-256
  `fc77b918cd77ec467147620b8dcb8b1e87d70a8961a6ad02fab753ce66142e48`,
  and final Standards SHA-256
  `7886fe79da78fb4f9ab64549eb54ec9377d910441fd87c0b87286215cd48a66d`.

## Stop conditions

Stop and update the relevant Seed instead of improvising if work requires:

- saved-secret readback or arbitrary locator/destination IPC;
- a generic native library path that cannot enforce the prompt contract;
- an unapproved Linux fork or remote publication;
- enabling file-v2 with empty evidence tables;
- secret-bearing logs, panic payloads, docs, fixtures, screenshots, or Seeds;
- broad edits in dirty `master`, `sd sync`, cache deletion, or workflow/release
  mutation without authorization; or
- closing a platform Seed on Linux-host source tests when target-native/package
  evidence is an explicit acceptance criterion.
