# Windows credential-v2 authority metadata root

Date: 2026-08-01

Seed: `audio-graph-5195`

Status: recommended

## Question and gated decision

Should the secret-free Windows credential-v2 authority journal remain under
Tauri `app_config_dir()` in Roaming AppData, or use
`app_local_data_dir()` in Local AppData while explicit file-v2 remains a
separate storage decision?

This gates the stable Windows path and identity contract, the correction needed
to ADR-0035 before credential-v2 implementation, v1 import and downgrade
semantics, cross-machine restore behavior, packaged/dev isolation, and the
Windows release matrix. It does not select a non-Windows root, implement the
service, change the frozen Tauri identifier, or choose a file-v2 location.

## Recommendation

Use the following root for the **native Windows credential-v2 authority
journal**:

```text
app.path().app_local_data_dir()?.join("credential-v2")

Default Windows expansion:
%LOCALAPPDATA%\com.rsac.audiograph\credential-v2\
```

Keep `state.json`, `mutation.lock`, same-directory temporary files, and bounded
recovery residue inside that qualified child directory. Keep ordinary
non-secret `config.yaml` under `app_config_dir()`; this decision does not move
general configuration. Keep explicit file-v2 separately selected, separately
qualified, and in its own path and format. A native-store failure must never
relocate either the journal or secret bytes to Roaming AppData or file-v2.

The outer filesystem contract is **Windows-local-root-v1**: Tauri
`app_local_data_dir()` plus the frozen identifier `com.rsac.audiograph`, with
the existing `credential-v2` service-generation child. The journal's own
schema field, not another ad hoc path suffix, versions its format. An
unsupported schema is a typed recovery state, not permission to search or
invent another root.

Confidence is **high** for the root and identity decision. Microsoft defines
the selected native credential persistence as same-user/same-computer, and
defines Local AppData for machine-specific app state. The exact locked Tauri
and `dirs` implementations map `app_local_data_dir()` to that Windows known
folder plus the runtime identifier. Confidence is **medium** for preservation
through NSIS uninstall and real backup products because the repository has no
packaged lifecycle evidence; those claims remain release-gated experiments.

## Identity contract

The two stable technical identities are intentionally distinct:

| Surface | Frozen value | Rule |
| --- | --- | --- |
| Tauri bundle/filesystem identity | `com.rsac.audiograph` | Derives the `app_local_data_dir()` child in packaged and dev builds using the effective Tauri config. Do not replace it with the credential namespace. |
| Native credential service identity | `com.codeseys.audiograph.credentials`; Windows targets rooted at `Codeseys.AudioGraph.Credentials/` | Identifies exact native entries. It does not derive filesystem paths. |
| Display identity | AudioGraph today; possible Aria copy later | Display-only. `productName`, window title, assistant persona, installer copy, and icons must not change either technical identity. |

This recommendation does **not** propose changing `com.rsac.audiograph`.
`app_local_data_dir()` must continue to derive from that frozen Tauri
identifier. A rename to Aria must not create an `Aria` directory, change the
native target, or trigger migration.

## Evidence

### Repository and exact dependency behavior

- **[verified]** The live Tauri configuration fixes `productName` as
  `AudioGraph`, `identifier` as `com.rsac.audiograph`, and includes an NSIS
  target. It contains no portable-mode or installer-hook contract.
  [`src-tauri/tauri.conf.json`](../../src-tauri/tauri.conf.json)
- **[verified]** The lock resolves Tauri 2.11.5, `dirs` 6.0.0, and
  `dirs-sys` 0.5.0. The exact Tauri 2.11.5 source computes
  `app_config_dir()` as `dirs::config_dir() + config.identifier` and
  `app_local_data_dir()` as `dirs::data_local_dir() + config.identifier`.
  The exact `dirs` 6.0.0 Windows source maps those bases to
  `FOLDERID_RoamingAppData` and `FOLDERID_LocalAppData`, respectively.
  [`src-tauri/Cargo.lock`](../../src-tauri/Cargo.lock),
  [Tauri 2.11.5 path source](https://docs.rs/tauri/2.11.5/src/tauri/path/desktop.rs.html#235-260),
  [`dirs::config_dir`](https://docs.rs/dirs/6.0.0/dirs/fn.config_dir.html),
  [`dirs::data_local_dir`](https://docs.rs/dirs/6.0.0/dirs/fn.data_local_dir.html)
- **[verified]** Canonical settings already resolve through
  `app_config_dir()/config.yaml`. The current v1 credential files instead use
  `dirs::config_dir()/audio-graph/credentials.yaml` and
  `credentials-state.yaml`; the v1 keyring facade uses exact service
  `audio-graph` and account `provider:<key>`.
  [`settings/mod.rs`](../../src-tauri/src/settings/mod.rs),
  [`credentials/mod.rs`](../../src-tauri/src/credentials/mod.rs)
- **[verified]** No product code at the researched base implements
  `credential-v2/state.json`, `mutation.lock`, or `v2/_authority`. The Roaming
  v2 location in ADR-0035 is therefore an unshipped design contract in this
  checkout, not a live directory that this decision must move. The safe action
  is to correct the contract before implementation, not add a speculative
  broad root importer.
- **[documented]** ADR-0035 makes the active native present/tombstone record
  authoritative, reserves `v2/_authority` as a secret-free marker, requires an
  owner-only atomic journal and shared mutation lock, forbids automatic
  file-v2 fallback, and makes legacy migration explicit and idempotent.
  [ADR-0035](../adr/0035-backend-owned-credential-service.md)
- **[documented]** AudioGraph's accepted naming decision freezes
  `com.rsac.audiograph` and treats Aria as possible display copy only.
  [Product naming decision](../designs/2026-07-31-product-naming-audio-graph-aria.md)

### Windows platform semantics

- **[documented]** Microsoft defines `CRED_PERSIST_LOCAL_MACHINE` as visible
  across logon sessions for the same user on the same computer and not visible
  to that user on other computers. `CRED_PERSIST_ENTERPRISE`, by contrast, can
  be visible on other computers.
  [Microsoft `CREDENTIALW.Persist`](https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw#members)
- **[documented]** Microsoft recommends `FOLDERID_LocalAppData` for state
  specific to the current machine and warns that putting machine-specific data
  in folders that travel between machines can create conflicts.
  [Microsoft Windows app restore guidance](https://learn.microsoft.com/en-us/windows/apps/develop/windows-app-restore#machine-specific-app-data)
- **[documented]** The Windows known-folder table resolves
  `FOLDERID_LocalAppData` to `%LOCALAPPDATA%` and
  `FOLDERID_RoamingAppData` to `%APPDATA%`.
  [Microsoft known-folder identifiers](https://learn.microsoft.com/en-us/windows/win32/shell/knownfolderid)
- **[inferred]** The journal records the status and transaction history of a
  machine-local native authority. Putting that journal in the corresponding
  machine-local app-data root preserves one locality model. Roaming the journal
  while retaining a Local native marker creates predictable one-sided or stale
  authority views on another computer.
- **[inferred]** Local AppData reduces *intended Windows profile roaming*; it
  does not prove that backup software, redirection, junctions, enterprise
  agents, or a same-user copier cannot copy it. The held-target filesystem and
  permissions detector remains mandatory, and the recovery protocol must make
  copied metadata harmless.

## Required authority-instance binding

Root selection alone cannot distinguish a valid journal from a copied or
restored journal. Add one secret-free, opaque, randomly generated
`authority_instance_id` to both the local journal and the exact native
`v2/_authority` marker. It is an equality token, not a machine fingerprint:

- generate it only during explicit initialize/migrate;
- never derive it from hardware, username, SID, hostname, path, display brand,
  or a secret;
- never return the raw token through IPC, logs, analytics, docs, screenshots,
  or Seeds;
- compare it under `mutation.lock` before treating journal status as usable;
- never repair a mismatch by taking the greater epoch, merging journals,
  rewriting the marker from the journal, or importing legacy data.

This is a necessary binding, not device attestation. A backup system that
restores the journal, marker, and all exact native records as one internally
consistent authority may be indistinguishable from the original installation.
AudioGraph should claim consistency and recovery safety, not cryptographic
proof of physical-machine identity.

### Recovery matrix

| Local journal | Local `v2/_authority` marker | Exact active records | Required result |
| --- | --- | --- | --- |
| Absent | Absent | Not probed by passive status | `uninitialized`. Explicit initialization/migration must preflight only the closed set of exact known/referenced v2 locators; finding any v2 record changes the result to `recovery_required`. |
| Absent | Present | Any | `recovery_required`. Preserve the marker; exact reconciliation may reconstruct metadata. Never run legacy import as replacement authority. |
| Present | Absent | Any | `recovery_required`. Treat the journal as foreign/restored metadata. Do not create a marker from it and do not report its configured sets. |
| Present | Present, malformed/unsupported | Any | `recovery_required`; no fallback, root search, or automatic reinitialization. |
| Present | Present, instance ids differ | Any | `recovery_required`; do not merge, choose an epoch, or overwrite either side automatically. |
| Present | Present, ids match | Revisions/operation ids and present/tombstone records match | Normal operation after pending-intent reconciliation. |
| Present | Present, ids match | Missing, newer, older, or otherwise disagreeing records | `commit_unknown` or `recovery_required` according to the existing stage contract. Exact native present/tombstone records remain authoritative; the journal must not roll them back. |

**[inferred]** This matrix fails closed for the common restore failures: journal
only, marker only, stale journal, another machine's journal, partial uninstall,
and interrupted initialization. It also prevents Roaming ordinary settings from
conferring credential presence on a second computer. That computer may retain
provider choices, but it must initialize, explicitly import, or re-enter its own
local credentials.

A credential-set reference in Roaming `config.yaml` is only a request to resolve
that set against the **local** authority; it is never evidence of presence. Any
config-resident pending credential/settings activation marker must also be bound
to its authority instance. A second computer that receives a foreign pending
marker must not complete it, clear it against its own authority, create a native
marker from it, or run legacy import. The exact cross-machine settings UX is a
follow-up, but provider activation remains gated on successful local resolution.

## Migration, upgrade, and downgrade contract

### First credential-v2 activation

Use only the exact legacy inputs already named by the product contract:

- `%APPDATA%\audio-graph\credentials.yaml`;
- `%APPDATA%\audio-graph\credentials-state.yaml`;
- v1 keyring service `audio-graph`, accounts `provider:<key>`; and
- known inline-secret settings fields.

Do not scan neighboring profile directories, other Tauri identifiers, general
Credential Manager entries, or guessed Aria/AudioGraph variants. The current
locked Windows keyring store documents Enterprise persistence as its default,
and current v1 entry creation does not request Local persistence. A legacy
credential can therefore be a valid explicit import candidate on more than one
computer, but its presence cannot turn v2 into a roaming authority.
**[verified]** The first locator shape is present in
[`credentials/mod.rs`](../../src-tauri/src/credentials/mod.rs).
**[documented]** The locked adapter's default is stated in the
[`windows-native-keyring-store` persistence contract](https://docs.rs/windows-native-keyring-store/1.1.0/windows_native_keyring_store/#persistence-type).

The idempotent, locked sequence remains:

```text
inspect exact v1 candidates
  -> plan and resolve conflicts
  -> qualify/harden the Local AppData credential-v2 child
  -> persist local intent and paired authority-instance identity
  -> write Local-persistence native present/tombstone record
  -> exact readback of marker instance and active revision/operation
  -> atomic local journal commit
  -> optional verified legacy quarantine/cleanup
```

An interruption at any cut point must land in the recovery matrix. Normal v2
mutations are never dual-written to v1. Ordinary Roaming settings stay where
they are; only secret-bearing v1 fields are redacted after verified v2 commit.

### Upgrade and an already-running v2 design build

- **[verified]** There is no live v2 journal in the researched product code, so
  first release needs no Roaming-v2-to-Local-v2 data move.
- **[inferred]** Packaged upgrades that retain the identifier
  `com.rsac.audiograph` resolve the same Local root. Journal schema upgrades run
  in place under the same lock with old-or-new atomic replacement and explicit
  schema support.
- **[inferred]** Do not add a generic importer for a hypothetical Roaming v2
  directory. If release inventory later proves a specific experimental build
  wrote one, gate the importer to that exact build/channel and require a matching
  `authority_instance_id`. Acquire old and new locks in a fixed order, copy and
  readback-verify to Local, then archive/remove the old journal so an old
  design-build cannot keep mutating a split authority. Never merge mismatched
  journals. This contingent path needs its own ADR/Seed and fixtures.

### Downgrade

After verified v1 cleanup and any v2-exclusive rotation/delete, an old binary
may require credential re-entry. Do not copy the Local v2 journal to Roaming,
recreate v1 values, or dual-write to make downgrade appear seamless: that would
allow stale credentials or deleted values to resurrect. This is the same
forward-only rollback boundary already accepted by ADR-0035.

## Backup, restore, uninstall, and portable execution

### Backup and restore

- The journal is non-secret but integrity-sensitive. Exclude it from ordinary
  product export/portable-profile formats by default; it is not a credential
  backup and cannot establish configured state without the matching native
  marker and records.
- A journal-only restore, marker-only restore, cross-machine journal copy, or
  stale journal restore enters `recovery_required` through the matrix above.
  Recovery probes only exact closed locators and never resurrects legacy data.
- File-v2 export/backup is a separate, explicit, degraded-security workflow
  because it contains secret bytes. This root decision grants it no implied
  support.
- Local AppData is not a promise of “never backed up.” Release/support language
  must preserve that non-claim.

### Uninstall and reinstall

The repository targets NSIS but contains no uninstall hook or packaged evidence
for Local AppData and Credential Manager retention. Do not promise that ordinary
uninstall removes or preserves credentials until the packaged matrix is run.

The safe product contract is that **ordinary uninstall is not credential
deletion**: it should leave the paired local journal and exact native records
together so reinstall can validate them. “Remove all AudioGraph credentials and
local credential metadata” must be a separate, explicit service operation that
coordinates exact native entries, marker, journal, and interruption recovery
under the lock. A blind installer deletion of only the directory is forbidden;
if an external uninstaller nevertheless removes only one side, reinstall must
return `recovery_required`, never `uninitialized` or success.

### Portable and multiple copies

The current repository defines no portable distribution mode. Moving or
unpacking the executable does not relocate Tauri `app_local_data_dir()`; a copy
with the same identifier and native namespace shares the same per-user,
per-computer authority and lock. A true self-contained portable mode would need
a separate security/identity decision, and must not silently place this journal
or native credentials beside the executable. Explicit file-v2 may inform that
future design, but is not selected by this decision.

## Packaged and development behavior

Tauri's path resolver uses the effective runtime config identifier, so the same
frozen identifier gives packaged and ordinary `tauri dev` processes the same
Local root. That is stable, but sharing production credential authority with
routine development is unsafe.

Default unit, contract, CI, and headless development must use an injected
in-memory backend or an explicit file-v2/test root. A native Windows integration
harness must use a throwaway journal root **and** a matching throwaway native
target namespace, or require an unmistakable opt-in to exercise the production
identity. Changing only the dev Tauri identifier while retaining production
native targets is forbidden because it creates the same marker/journal split
this decision is designed to prevent.

## Permissions, filesystem qualification, and recovery operations

Changing the known-folder API does not relax the accepted persistence gates:

1. Resolve `app_local_data_dir()` and create/harden only its `credential-v2`
   child; do not rewrite permissions for the shared
   `%LOCALAPPDATA%\com.rsac.audiograph` parent that may contain unrelated app
   data.
2. Inspect the actual directory reached by a held handle. LocalAppData can still
   be redirected, reparse-backed, remote, removable, cloud-managed, or on an
   unproved filesystem.
3. Require the approved protected Windows DACL at directory and file creation,
   verify it from handles before content bytes, and retain same-directory
   old-or-new replacement and lock semantics.
4. Return the existing typed unsupported-target, durability, permission, or
   recovery status on any failed observation. Do not fall back to Roaming or
   file-v2.
5. Diagnose/reconcile only exact built-in locators and backend-issued custom ids
   referenced by current settings. Never enumerate the user's general
   Credential Manager or reveal paths/native error prose over IPC.

**[documented]** These requirements come from the accepted filesystem and
Windows replacement research; this note changes only the root whose actual
target those primitives must qualify.
[Supported-filesystem decision](2026-08-01-credential-supported-filesystems.md),
[Windows durable-replace decision](2026-08-01-windows-credential-durable-replace.md)

## Adversarial pass and rejected alternatives

| Alternative or failure mode | Disposition | Reason |
| --- | --- | --- |
| Keep the journal in `app_config_dir()` / Roaming AppData | Reject | It allows machine-local authority metadata to travel independently of a `CRED_PERSIST_LOCAL_MACHINE` marker/record, producing stale or one-sided state on another computer. |
| Hard-code `%LOCALAPPDATA%\AudioGraph`, `%LOCALAPPDATA%\Aria`, or the native service namespace | Reject | It bypasses Tauri's frozen filesystem identity, couples paths to display copy, or conflates two intentionally separate identities. |
| Treat LocalAppData as proof against copy/backup/redirect | Reject | Known-folder selection is intent, not attestation. Held-target qualification and mismatch recovery remain required. |
| Store a hardware id/SID/hostname to prove the machine | Reject | Those identifiers can be sensitive, mutable, cloneable, or unavailable and are unnecessary. A random paired authority-instance equality token addresses the split-state problem without a machine fingerprint. |
| If either side is missing, initialize or import v1 automatically | Reject | Partial uninstall, restore, and interrupted commit would silently replace or resurrect authority. One-sided state is recovery. |
| Search both Roaming and Local roots and choose the highest epoch | Reject | Epochs have meaning only inside one bound authority; this would merge copied or concurrent histories and enable rollback/resurrection. |
| Co-locate or auto-select file-v2 | Reject | File-v2 contains secrets and has separate path, permissions, evidence, and explicit degraded-security consent. Native failure never selects it. |
| Let routine dev builds share one half of production identity | Reject | A separate journal with production native targets, or the reverse, creates false status and unsafe mutation concurrency. Test identity must be end-to-end. |
| Assume uninstall or executable relocation defines storage semantics | Reject | No packaged evidence or portable-mode contract exists. The service root remains identity-derived and lifecycle behavior must be tested. |

Two more documentary sources would not change the root recommendation. The
remaining uncertainty is behavioral and requires packaged experiments.

## Cheapest decisive experiments and release gates

1. **Packaged path/identity proof:** On the minimum supported Windows release,
   install an NSIS artifact in a fresh user profile, initialize dummy v2 state,
   and verify by handle that the actual journal parent is the qualified
   `%LOCALAPPDATA%\com.rsac.audiograph\credential-v2` target. Upgrade with only
   display copy changed and prove the root, marker, revision, and native target
   remain unchanged.
2. **Mismatch/restore matrix:** With dummy entries, exercise journal-only copy,
   marker-only state, mismatched instance ids, stale journal, matched restore,
   a foreign Roaming config/pending-activation marker, corrupt/unsupported
   schema, and interruption at each initialization/migration cut. Assert no case
   auto-imports v1, overwrites the marker, reports configured falsely, or
   enumerates general credentials.
3. **Installer lifecycle:** Run install -> upgrade -> uninstall -> reinstall for
   ordinary uninstall and any explicit remove-data flow. Record what NSIS and
   Windows actually retain. A one-sided result must recover safely; a release
   claim of preservation/deletion requires the paired-state assertions.
4. **Development isolation:** Prove normal tests use memory/file-v2 test state,
   and prove any native integration profile changes both the filesystem root and
   native target namespace. Run two process copies to verify they share the
   expected lock only when both identities match.

No real provider credential is needed. Random dummy values and exact target
inspection make these the cheapest decisive experiments.

## Open risks

- NSIS uninstall/reinstall behavior for the selected Local directory and native
  credentials is unverified in this repository.
- A third-party backup that restores all paired components consistently is not
  distinguishable from the prior authority; the design makes no device-
  attestation claim.
- Enterprise redirection or an undeclared same-user copier can still observe or
  move Local AppData. The actual-target detector has a bounded assurance model.
- Exact initialization ordering and cleanup for an explicit remove-all operation
  remain implementation state machines and need interruption fixtures.
- A specific experimental build may exist outside this source snapshot. Do not
  implement a Roaming-v2 importer without release inventory proving its exact
  identifier, schema, and channel.

## Out-of-scope queue proposals

This research worktree is explicitly forbidden from editing Seeds. The main
checkout should preserve these findings in the queue:

- Update the ADR-0035/service-adapter work to replace only the **Windows**
  app-config journal root with the Windows-local-root-v1 contract and add the
  paired `authority_instance_id` recovery matrix.
- Extend the existing v1 migration work with exact Roaming-v1 -> Local-v2
  fixtures, legacy Enterprise-persistence cases, and forward-only downgrade
  messaging.
- Extend the packaged-platform proof Seed with the four experiments above,
  especially NSIS uninstall/reinstall and cross-machine restore mismatch.
- Create a separate Seed only if a supported portable mode or explicit
  remove-all/export workflow is desired; neither is implied by credential-v2.
- Create a gated Roaming-v2 migration Seed only if build/release inventory proves
  that an external experimental build wrote that design-only location.

## Decision consequence

Amend ADR-0035 before implementation so its Windows journal sentence says
`app_local_data_dir()/credential-v2`, while preserving its existing non-Windows
root decision until separately researched. Implementation and release work must
use the recovery and packaged-test contracts above; no filesystem move is
authorized by this research note itself.
