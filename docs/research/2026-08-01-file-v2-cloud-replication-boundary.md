# File-v2 cloud-replication threat boundary

Date: 2026-08-01

Seed: `audio-graph-b686`

Gated workstream: `audio-graph-fb2b`

Related decisions:

- [ADR-0035: Backend-owned credential service](../adr/0035-backend-owned-credential-service.md)
- [Credential v2 supported-filesystem policy](2026-08-01-credential-supported-filesystems.md)
- [Credential service threat model](../security/credential-service-threat-model.md)
- [Credential service rebuild plan](../plans/2026-07-31-credential-service-rebuild.md)

## Question and gated decision

May AudioGraph support the explicitly selected plaintext file-v2 backend by
claiming only that a bounded detector excluded **recognized OS-declared managed
or cloud storage at its last qualification**, while saying that it cannot
detect or prevent arbitrary same-user backup, sync, indexing, endpoint-security,
administrative, or malicious software from copying the file? Or must file-v2
provide a stronger guarantee that the credential file cannot be replicated,
which would make file-v2 unsupported? [documented]

This gates the claim consumed by ADR-0035, the platform detector's meaning,
selection and recovery behavior, degraded-security Settings copy, redacted
status/telemetry, and the release evidence required before any file-v2 profile
can be populated. It does not implement a detector, enable a profile, select a
user path, or make plaintext file-v2 a secure-storage peer of the native
credential stores. [documented]

Evidence labels in this note mean:

- **[verified]**: checked directly in this repository, an exact installed SDK
  header, or an executable result already recorded in repository research;
- **[documented]**: stated by an official platform API, platform source, or
  repository decision; and
- **[inferred]**: the product or engineering conclusion drawn from that
  evidence.

## Recommendation

Choose **A: a bounded OS-declared-root exclusion**, with **high confidence**.
It is the strongest supportable claim for plaintext file-v2:

> At the last required qualification, AudioGraph did not recognize the opened
> target as remote, removable, userspace-backed, or inside a managed/cloud root
> exposed by the platform interfaces and release profile that AudioGraph
> supports. This is not a guarantee that the file stays on one device or that
> other software cannot read, copy, back up, or synchronize it.

[inferred]

The stronger B claim—no replication by any process—cannot be established by a
filesystem detector. A process running with the same user's file authority can
copy an owner-readable file without changing the target filesystem, mount,
Cloud Files registration, iCloud resource value, or File Provider identity.
Linux's official `fanotify` documentation even names virus scanning as a
filesystem-monitoring use case and permits monitoring an entire mounted
filesystem; that is an example of an observer, not an exhaustive registry of
all observers. [documented: [Linux `fanotify(7)`](https://man7.org/linux/man-pages/man7/fanotify.7.html);
inferred]

Therefore:

- if maintainers accept A, file-v2 may become a separately evidenced,
  explicit, persistently degraded backend after all platform/profile gates
  pass;
- if product or compliance policy requires B, keep file-v2 unsupported and use
  the native credential stores; no additional metadata source or user consent
  can turn B into a proved claim; and
- choosing A **does not enable file-v2 now**. The accepted journal and file-v2
  evidence-profile tables remain empty, so current release status remains
  unsupported on Windows, macOS, and Linux. [verified/inferred]

Confidence is **high** that B is unprovable and that A is the honest product
boundary; **medium** that the proposed macOS negative can eventually qualify a
profile, because Apple documents useful positive signals but AudioGraph still
needs the signed packaged cross-provider experiment described below. [inferred]

## Exact supported claim and non-claims

### What a supported profile may claim

A file-v2 profile may authorize only this conjunction, evaluated against the
actual opened target and the detector/profile versions compiled into the
release: [inferred]

1. the target was writable and its identity stayed stable through the
   qualification window;
2. the platform reported a supported local, fixed/internal,
   kernel-filesystem topology rather than a recognized network, removable,
   hot-plug, FUSE/userspace, or unknown topology;
3. Windows Cloud Files, macOS iCloud plus any separately proved File Provider
   query, or the Linux mount/topology seam did not positively identify the
   target as managed under that platform's bounded interface;
4. every required observation produced an exact favorable result—an error,
   timeout, missing value, unproved negative, or identity disagreement denied
   the target; and
5. an exact, separately reviewed file-v2 evidence profile covered the detector,
   owner-only permission-before-bytes, persistence wrapper, recovery protocol,
   and release artifact.

The claim is an observation at a defined qualification time, not attestation of
future state. UI and support material must use “did not recognize” or “last
checked,” never “cannot sync,” “never leaves this device,” “cloud-proof,” or
“secure because local.” [inferred]

### What it never proves

Even a matched profile does not prove: [documented/inferred]

- absence of a same-user backup/sync/indexing/EDR process, an administrator,
  malware, a compromised AudioGraph process, VM or volume snapshots, or later
  software installation;
- absence of a provider that does not use the queried OS registration/API;
- future absence of Cloud Files registration, iCloud/File Provider takeover,
  mount or namespace changes, redirection, or path replacement;
- deletion of copies made before a target was denied or requalified;
- confidentiality merely because the filesystem is NTFS, APFS, or ext4, or
  because the file has owner-only metadata; or
- native-store-equivalent at-rest protection. File-v2 stores the credential
  envelope as plaintext bytes readable to the selected user authority.

Owner-only ACL/mode checks remain mandatory because they reduce exposure to
other ordinary accounts in the tested OS boundary. They do not contradict the
same-user non-claim: on Linux, effective credentials determine file access,
and the owner permission bits grant the owning user read/write access.
[documented: [Linux `credentials(7)`](https://man7.org/linux/man-pages/man7/credentials.7.html),
[`open(2)` permission bits](https://man7.org/linux/man-pages/man2/open.2.html)]

## Repository facts that constrain the decision

- **[verified]** ADR-0035 already makes native storage the only automatic
  production backend, requires explicit file-v2 selection before service
  initialization, prohibits native-failure fallback, and requires persistent
  degraded-security status.
- **[verified]** The accepted filesystem research uses separate versioned
  `journal_supported_profiles` and `file_v2_supported_profiles`; both are
  empty. Journal evidence cannot authorize file-v2, and a local filesystem
  family is only a candidate until packaged evidence is accepted.
- **[verified]** The generated v2 contract already has closed `native`,
  `file_v2`, and `in_memory` backend kinds and a closed `file_v2` set source.
  It does not expose a path in those types.
- **[verified]** The current product implementation is still the legacy v1
  surface. `AUDIO_GRAPH_CREDENTIAL_BACKEND` can explicitly select
  `credentials.yaml`, and the current Credentials panel renders scalar source
  rows and the unconditional sentence “Saved keys stay local.” There is no
  implemented file-v2 selector, acknowledgement, requalification, or recovery
  UI in this base. The future v2 Settings work must not reuse that sentence for
  file-v2.
- **[verified]** `audio-graph-fb2b` owns native/file-v2 adapters,
  `audio-graph-55df` owns the dummy-only conformance manifest, and
  `audio-graph-2c33` owns the revisioned Settings credential UX. This decision
  supplies requirements to those Seeds; it does not edit them.

## Platform evidence and bounded detector meaning

### Windows: CfAPI covers registered Cloud Files trees

Microsoft documents `CfRegisterSyncRoot` as a one-time registration that lets a
sync provider claim the entire directory tree rooted at `SyncRootPath`; the
platform persistently tracks registered roots on a volume and forbids
overlapping registered trees. [documented: Microsoft
[`CfRegisterSyncRoot`](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfregistersyncroot)]

Microsoft documents `CfGetSyncRootInfoByHandle` as returning characteristics of
the sync root containing the object named by a handle. It succeeds for a file
under a Cloud Files sync root, needs only `READ_ATTRIBUTES`, and fails when the
file is not underneath one. [documented: Microsoft
[`CfGetSyncRootInfoByHandle`](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfgetsyncrootinfobyhandle)]

The exact favorable negative is important: [verified/documented]

- installed Windows SDK 10.0.22621.0 and 10.0.26100.0 `winerror.h` both define
  `ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT` as Win32 error **390**;
- `HRESULT_FROM_WIN32(390)` is **`0x80070186`**; and
- this must not be confused with `ERROR_NOT_A_CLOUD_SYNC_ROOT` **405**, whose
  meaning is that the queried object is not itself a cloud sync root.

Microsoft's Cloud Files API documentation uses
`HRESULT(ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT)` when an object is not contained
in a registered sync-root tree, and Microsoft documents the Win32-to-HRESULT
mapping. [documented: Microsoft
[`CfUpdatePlaceholder` containment rule](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfupdateplaceholder),
[`HRESULT_FROM_WIN32`](https://learn.microsoft.com/en-us/windows/win32/api/winerror/nf-winerror-hresult_from_win32)]

The Windows adapter may therefore map only these outcomes: [inferred]

| `CfGetSyncRootInfoByHandle` outcome | Closed observation |
| --- | --- |
| `S_OK` | `os_managed_cloud_root = Yes`; deny and discard provider/file identities. |
| `HRESULT_FROM_WIN32(ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT)` (`0x80070186`) | `os_managed_cloud_root = No` for the registered-CfAPI scope at that instant. |
| Any other HRESULT, panic, unsupported API, or deadline failure | `Unknown`; deny. |

This detects roots registered through Cloud Files. It does not enumerate or
exclude ordinary applications that copy local files, non-CfAPI sync products,
backup software, profile-management tools, filters, or a provider installed
after the observation. The exact HRESULT stays adapter-private and never
crosses IPC or telemetry. [inferred]

The query must use the already opened final target rather than a drive letter
or configured prefix. Windows documents that a directory handle is obtained
with `CreateFile` plus `FILE_FLAG_BACKUP_SEMANTICS`, and that file/volume
identity can compare whether two paths or handles reach the same object.
[documented: Microsoft
[`CreateFileW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
[`GetFileInformationByHandle`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandle)]

### macOS: iCloud is direct; File Provider needs a proved bounded query

Apple documents `URLResourceKey.isUbiquitousItemKey` as a Boolean indicating
whether an item is in iCloud storage. A true value is a direct deny signal. A
false value means “local” for this iCloud trait; it does not say that no
third-party File Provider or ordinary process will replicate the item.
[documented: Apple
[`isUbiquitousItemKey`](https://developer.apple.com/documentation/foundation/urlresourcekey/isubiquitousitemkey)]

Foundation separately exposes volume-local and volume-internal traits.
`volumeIsInternalKey` may be `nil` when the system cannot determine the answer,
which confirms that missing resource values cannot be treated as favorable.
[documented: Apple
[`volumeIsLocalKey`](https://developer.apple.com/documentation/foundation/urlresourcekey/volumeislocalkey),
[`volumeIsInternalKey`](https://developer.apple.com/documentation/foundation/urlresourcekey/volumeisinternalkey)]

Apple's `NSFileProviderManager.getIdentifierForUserVisibleFile` returns an item
identifier and domain for a user-visible provider URL. Apple says a URL not
managed by “your File Provider extension” returns `NSFileNoSuchFileError`.
Apple also describes `getDomainsWithCompletionHandler` as returning all of
**the File Provider extension's** domains, not a public inventory of every
installed provider. [documented: Apple
[`getIdentifierForUserVisibleFile`](https://developer.apple.com/documentation/fileprovider/nsfileprovidermanager/getidentifierforuservisiblefile%28at%3Acompletionhandler%3A%29),
[`getDomainsWithCompletionHandler`](https://developer.apple.com/documentation/fileprovider/nsfileprovidermanager/getdomainswithcompletionhandler%28_%3A%29)]

Those docs support a positive deny on successful identification, but they do
not establish that an unrelated AudioGraph process receives a universal
negative across iCloud and every third-party provider. The query is also
asynchronous. AudioGraph may use it as a favorable negative only after a signed,
packaged experiment proves the exact calling context on minimum/current macOS;
until then, or on unexpected error/timeout, the result is `Unknown` and macOS
file-v2 profiles stay empty. A completion arriving after the deadline must be
discarded and cannot upgrade a returned denial. Provider/domain identifiers
remain transient and private. [inferred]

### Linux: kernel-visible storage layers, not ordinary copiers

Linux exposes the mount containing an opened object: `statx(...,
STATX_MNT_ID)` returns the mount ID corresponding to field 1 in
`/proc/self/mountinfo`, and mountinfo describes mounts in the reading process's
mount namespace, including per-mount versus per-superblock options and the
filesystem type. [documented: Linux
[`statx(2)`](https://man7.org/linux/man-pages/man2/statx.2.html),
[`proc_pid_mountinfo(5)`](https://man7.org/linux/man-pages/man5/proc_pid_mountinfo.5.html)]

That permits a bounded adapter to reject NFS/CIFS/SMB, 9p/virtiofs, FUSE,
overlay/userspace, read-only mount views, removable/hot-plug or unresolved
block topology, and unproved filesystem types. The kernel defines FUSE as a
userspace filesystem in which an ordinary userspace process supplies data and
metadata, so rejecting `fuse`/`fuseblk` is a meaningful layer exclusion.
[documented: Linux kernel
[`FUSE overview`](https://docs.kernel.org/filesystems/fuse/fuse.html);
inferred]

The same APIs contain no registry of processes that read a normal local ext4
file and copy it elsewhere. A Dropbox-like watcher, backup job, indexer, EDR
agent, or user script can operate above an unchanged local ext4 mount. Linux
`inotify` and `fanotify` document mechanisms by which userspace can monitor file
events; a detector cannot infer the absence of other mechanisms by enumerating
those APIs or current processes. [documented: Linux
[`inotify(7)`](https://man7.org/linux/man-pages/man7/inotify.7.html),
[`fanotify(7)`](https://man7.org/linux/man-pages/man7/fanotify.7.html);
inferred]

Linux support under A may therefore claim only that no **kernel-visible denied
storage layer or topology** was observed at qualification time. It must not
claim that an ext4 location is “not cloud-synced.” [inferred]

## Held-target, redirection, namespace, and time boundaries

### What held-target qualification establishes

The detector must qualify the directory object that would actually contain the
file-v2 lock, temporary objects, recovery residue, and active secret envelope.
It must not qualify a configured string, a path prefix, the user's home
directory, or the boot volume. [inferred]

On Linux, the `openat` family exists specifically to avoid races in which a
directory-path component changes between a check and a later operation. The
Linux man page says a directory file descriptor remains a stable reference even
if the directory is renamed and prevents the underlying filesystem from being
unmounted. Relative `openat`/`renameat` operations can therefore keep the
persistence wrapper on the qualified object. [documented: Linux
[`open(2)`, rationale for `openat`](https://man7.org/linux/man-pages/man2/open.2.html)]

The same design principle applies to Windows directory handles and macOS file
descriptors: resolve redirection/reparse/symlink behavior according to the
closed platform policy, hold the final target, query that target, re-read its
identity, and make the persistence operation relative to or otherwise prove it
still reaches that target. [documented/inferred]

This bounds these races: [inferred]

- a configured symlink, Windows junction/reparse point, bind mount, relocated
  known folder, or redirected configuration root is classified by where it
  actually leads, not by its friendly path;
- Linux mount flags and filesystem type come from the process's own mount
  namespace, the same view in which AudioGraph will access the target; and
- an identity change between the primary and supplemental platform queries, or
  between qualification and an unavoidable path-based write, becomes
  `target_changed`/`inspection_unavailable`, never support.

Linux explicitly documents that mount namespaces isolate the mount list seen by
processes, so a result from the host, a helper in another namespace, or a
longest-prefix scan is not authority for AudioGraph's target. [documented:
Linux [`mount_namespaces(7)`](https://man7.org/linux/man-pages/man7/mount_namespaces.7.html)]

### What a held target does not establish

A held directory handle or descriptor does not freeze all relevant world
state. It does not: [inferred]

- prevent another process from reading the file;
- prove that an ancestor will not later become a Cloud Files sync root, that a
  File Provider domain will not claim a location, or that a same-user agent
  will not begin copying it;
- prove that a future process reopening the persisted locator reaches the same
  directory or mount;
- undo a copy already made; or
- make path-based Foundation/File Provider or Windows supplemental results
  race-free without reopening and comparing identity to the held object.

Even repeated polling supplies only additional observation points. It does not
turn A into B. The product must expose a persistent degraded posture and a last
qualification state rather than implying continuous enforcement. [inferred]

## Selection, requalification, and recovery contract

### Explicit selection before bytes

File-v2 selection is one backend-owned, user-initiated transition, not a
fallback flag inferred from a native-store error: [documented/inferred]

1. The user chooses **Plaintext file (reduced protection)** and acknowledges
   the versioned bounded claim before service initialization or migration.
2. A backend-owned locator flow resolves and opens the chosen parent. No
   journal, lock, temporary object, secret envelope, or recovery residue is
   created until the detector returns `Supported` for an exact compiled
   file-v2 profile.
3. The backend records only the selected backend, acknowledgement version,
   profile/detector versions, and private locator in backend-private state. A
   user override, environment variable, remote response, or renderer-provided
   “trust this location” bit cannot manufacture support.
4. A native-store `locked`, `denied`, `unavailable`, `cancelled`, `unsupported`,
   or internal result leaves the native backend selected and returns its typed
   recovery action. It never selects file-v2, legacy YAML, or another path.

Headless development may make the same explicit selection through a documented
local launch contract, but it receives the same denial rules, persistent
degraded posture, and empty-profile gate. A test-only injected in-memory backend
is separate and must not be labeled file-v2. [documented/inferred]

### Minimum requalification points

Requalification must use the held-target detector and exact compiled profile:
[inferred]

- on every process/service initialization, before opening an existing file-v2
  envelope for normal use;
- after explicit path/backend selection or change;
- after any restart, recovery, reconcile, target reopen, or loss of the held
  handle;
- immediately before each mutation, after acquiring the cooperating lock and
  before creating or writing the first new secret byte; and
- whenever a target/provider/mount change notification is available, while
  treating notifications only as prompts to requalify, never as proof that no
  unreported change occurred.

The mutation must use the same qualified held target. If an implementation has
to re-resolve a string for any object-creation, replacement, or namespace-sync
step, it must reopen, compare identity, and requalify before that step. A check
performed in a different process, namespace, helper, or earlier boot does not
carry forward. [inferred]

Requalifying before mutation reduces additional exposure after a recognized
state change; it cannot guarantee that no copy occurred between checks. The UI
must keep the “last checked, not guaranteed” copy even if every requalification
passes. [inferred]

### Fail-closed change and explicit recovery

If any required observation becomes unknown/negative, the target identity
changes, the profile no longer matches, or detector/persistence protocol
versions change: [inferred]

1. latch file-v2 into a typed `recovery_required` state with a closed reason
   such as `cloud_managed`, `remote`, `userspace_filesystem`,
   `target_changed`, `inspection_unavailable`, or
   `confidentiality_unproved`;
2. stop normal credential resolution and mutation; do not create, replace,
   delete, quarantine, or search for another file automatically;
3. do not resume automatically if a later observation happens to pass—the
   denied interval may already have produced a copy;
4. offer explicit choices to move to the native credential store, select and
   qualify a new file-v2 location, attempt backend-owned recovery from the exact
   known old authority, or remove the old authority; and
5. require the destination to qualify before bytes, commit/readback-verify the
   new authority before cleanup, and state plainly that relocation/deletion
   cannot revoke copies made previously.

If the old target is unavailable or cannot be safely opened, recovery may
require credential re-entry. The service must not scan neighboring directories,
try platform-default paths, import legacy YAML, choose the highest epoch, or
fall back to stale native entries. [documented/inferred]

The detailed authority-transfer transaction remains implementation work under
the existing adapter/migration Seeds. This decision fixes its product
boundary: every recovery is explicit and fail-closed, and none can claim to
reverse external replication. [inferred]

## Honest degraded-security UI copy

The exact localized wording may change for readability, but every locale must
preserve these propositions and the acceptance tests must pin their meaning.
Recommended English source copy: [inferred]

**Selector title**

> Plaintext credential file (reduced protection)

**Pre-selection warning**

> Credentials will be stored unencrypted in an owner-only file at the location
> you choose. AudioGraph blocks locations it recognizes as network, removable,
> userspace-backed, or OS-managed cloud storage when checked. It cannot detect
> or prevent backup, sync, security, admin, malware, or other software running
> as your account from copying the file. Use the system credential store for
> stronger protection.

**Required acknowledgement**

> I understand that this plaintext file can be copied by software running as
> my account.

**Persistent status badge and detail**

> Plaintext file · reduced protection

> The chosen location passed AudioGraph's last supported-location check. This
> is not a no-backup or no-sync guarantee.

**Selection denial**

> This location is managed, remote, removable, userspace-backed, or could not
> be verified. Choose another location or use the system credential store.

**Recovery state**

> The saved file location no longer qualifies. AudioGraph will not choose a
> different backend automatically. Choose a new location, move to the system
> credential store, remove the file, or re-enter credentials if recovery is
> unavailable. Copies made earlier cannot be revoked by AudioGraph.

The warning and badge are persistent product state, not a one-time modal that
disappears after consent. The current generic Settings sentence “Saved keys
stay local” must not render for file-v2; “local” is ambiguous and would
contradict this boundary. Native-store copy should also avoid claiming absolute
same-user isolation beyond the native-store threat model. [verified/inferred]

The UI must not say that a selected location is secure merely because it is
local, internal, NTFS, APFS, ext4, owner-only, or profile-matched. It may show a
closed status and safe actions, but not a private path, provider/domain name,
device/volume identity, or native error. [inferred]

## Telemetry, logs, diagnostics, and IPC redaction

File-v2 status may expose only closed, content-free fields already needed for
safe action selection: [inferred]

- `backend_kind = file_v2` and `security_posture = degraded_plaintext`;
- closed qualification/recovery code;
- closed platform/filesystem family when the accepted profile permits it;
- detector schema, cloud-boundary policy version, persistence protocol version,
  and evidence-profile identifier/digest; and
- whether acknowledgement is current and which safe recovery actions are
  available.

These fields may cross IPC and, subject to the user's analytics choice, be
counted in aggregate telemetry. They contain no location or secret-derived
material. [inferred]

The following remain backend-private and must not appear in serialized status,
mutation receipts, events, logs, analytics, crash breadcrumbs, screenshots,
support bundles, docs, or Seeds: [documented/inferred]

- configured/canonical path, basename, mount point/root/source, symlink or
  reparse target, username, home/profile path, drive letter, URL, or private
  locator;
- volume serial/GUID, filesystem/device/file/mount identity, SID, machine id,
  File Provider item/domain/provider id, Cloud Files identity, or installed
  provider inventory;
- raw filesystem/driver strings, native error prose, HRESULT/Win32/OSStatus/
  errno values—including the Windows `0x80070186` negative sentinel;
- secret bytes, exact encoded length, hashes/fingerprints, filenames derived
  from a credential, temporary/recovery content, or content canaries; and
- a list of other processes, services, agents, or applications that might be
  observing the file.

An internal detector can retain transient native data long enough to evaluate
and recheck identity, then reduce it to the closed observation. Errors map at
the boundary; formatting a native error first and attempting to redact it later
is not acceptable. A backend-owned “choose location” or “open recovery
location” action must not return the private locator as ordinary renderer data.
[inferred]

## Empty profiles and exact release-proof consequence

Choosing A changes the **permitted claim**, not the current support state. The
existing build-time tables remain: [verified]

```text
journal_supported_profiles = {}
file_v2_supported_profiles = {}
```

No implementation, unit fixture, user acknowledgement, successful metadata
call, or journal profile may populate the file-v2 table. Each platform profile
must bind the release/artifact digest, minimum/current OS, detector schema,
`cloud-boundary-v1`, persistence-wrapper protocol, closed storage traits, and a
reviewed dummy-only manifest. [inferred]

In addition to the accepted permission/durability gates, the cloud-boundary
matrix must prove: [inferred]

### Windows profile

- a held local fixed NTFS target returns the exact
  `ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT` 390 HRESULT and remains only a
  candidate until every other gate passes;
- a temporary directory tree registered with `CfRegisterSyncRoot` produces a
  successful containment query and denial for the root and descendants;
- every other HRESULT and injected timeout is unknown/denied;
- junction/reparse/redirection into a registered root is denied by the final
  target, not allowed by the configured path; and
- registration/unregistration or identity change after initial selection is
  caught at a required requalification point and latches explicit recovery
  without automatic resume.

### macOS profile

- iCloud true denies and iCloud false alone does not grant;
- a signed packaged ordinary AudioGraph process gets a positive File Provider
  identification for representative third-party domains and a proved exact
  negative for ordinary internal APFS on minimum/current macOS;
- error, missing/wrong-typed value, callback timeout, and late completion deny;
- SMB, external/removable APFS, provider takeover/known-folder changes, and fd
  versus transient-URL identity disagreement deny; and
- no APFS file-v2 profile is accepted if the File Provider experiment cannot
  establish a reliable cross-provider negative.

### Linux profile

- exact fd mount ID/mountinfo and bounded block topology deny real NFS/CIFS,
  FUSE/fuseblk, 9p/virtiofs, overlay/userspace, read-only bind, removable/USB,
  loop, unknown, and inconsistent targets in the AudioGraph process's mount
  namespace;
- a path/bind/namespace substitution either leaves operations on the held
  qualified target or denies with target change before bytes;
- native ext4 fixed topology remains only a candidate until all file-v2
  permission, replacement, recovery, and abrupt-reset gates pass; and
- a dummy same-user watcher/copier exercise confirms the contract's limitation:
  it must not cause the detector or UI to claim prevention, and the persistent
  degraded/non-replication copy remains visible. This is a non-claim test, not
  a requirement to discover the copier.

All three profiles must run redaction canaries through status, IPC, logs,
telemetry, errors, crash/support output, and the manifest. No real credential is
needed. A missing or failing case leaves the corresponding profile absent and
file-v2 unsupported on that platform; evidence from another OS or filesystem
cannot substitute. [documented/inferred]

## Adversarial pass and failure modes

- **A same-user copier defeats any “no replication” interpretation.** It can
  read a file using ordinary allowed file access and write a copy without
  changing the source target's filesystem classification. Enumerating known
  processes, services, filesystem watchers, extensions, or installed products
  would be incomplete, racy, privacy-sensitive, and still miss one-shot or
  future actors. This is why the limitation must be in the supported claim,
  not hidden as an implementation footnote. [documented/inferred]
- **A provider can appear after a passing check.** CfAPI registration,
  File Provider/known-folder state, mounts, namespaces, redirection, and
  software installation can change. Required requalification reduces future
  writes after a recognized change but cannot close the observation interval
  or revoke an earlier copy. [inferred]
- **“Local” is not synonymous with “unreplicated.”** A fixed NTFS/APFS/ext4
  object may be read by a local agent that uploads it. Conversely, a local
  cache of cloud-managed content may live on an internal filesystem. Storage
  topology and replication policy are different facts. [inferred]
- **The Windows negative is easy to miscode.** Mapping all failures to “not a
  root,” or using `ERROR_NOT_A_CLOUD_SYNC_ROOT` 405 rather than
  `ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT` 390, creates a fail-open detector.
  Fixtures must pin the exact symbolic/numeric distinction while keeping it
  out of public status. [verified/inferred]
- **The macOS negative is not yet proved.** Apple's “your File Provider
  extension” wording and extension-scoped domain enumeration do not support a
  universal inventory claim. Shipping an APFS profile on iCloud false alone
  would miss third-party File Provider roots. [documented/inferred]
- **Linux namespace answers are process-relative.** A host helper can observe a
  different mount tree from the packaged app. The app's held fd plus
  `/proc/self/mountinfo` is the relevant view, and an unrecognized stacked
  topology must deny. [documented/inferred]
- **Handle qualification can be discarded accidentally.** If the persistence
  wrapper later opens the configured absolute path, a junction/bind/symlink
  swap can redirect bytes after the detector passed. The held target or a full
  identity requalification must reach the actual write. [documented/inferred]
- **Automatic recovery can resurrect or disclose.** Falling from an
  unavailable file target to stale native records or legacy YAML can restore a
  deleted generation; choosing a new default path can create another plaintext
  copy. A later passing observation cannot prove that the denied interval was
  harmless. Recovery and any authority transfer remain explicit. [documented/inferred]
- **Permission hardening is necessary but easily overmarketed.** `0600` or an
  owner-only DACL is not encryption, malware defense, backup exclusion, or
  same-user process isolation. Degraded copy must survive every successful
  permission/profile check. [documented/inferred]
- **Evidence can drift.** OS releases, Cloud Files/File Provider behavior,
  filesystem drivers, detector schemas, and persistence protocols can change.
  Profiles are release data bound to versions; runtime inference cannot extend
  an old profile. [inferred]

The adversarial pass does not overturn A. It narrows A to a useful detector
claim and confirms that any stronger claim must keep file-v2 unsupported.
[inferred]

## Rejected alternatives

- **B: require proof that no process will replicate the file.** Rejected as
  undecidable from filesystem/provider metadata. If this remains a policy
  requirement, the resulting product action is to disable file-v2, not to
  weaken the evidence standard. [inferred]
- **Call a local filesystem or owner-only file “secure/local-only.”** Rejected
  because these facts do not exclude same-user readers or later replication.
  [documented/inferred]
- **Vendor folder-name or path-prefix denylist.** Rejected as incomplete,
  user-configurable/localization-sensitive, racy under redirection, and a leak
  of private path/provider information. [inferred]
- **Enumerate installed sync/backup/security processes and services.** Rejected
  because absence at one instant is not proof of future absence, process names
  are not authority, privileged/system agents may be opaque, and the inventory
  itself is sensitive. [inferred]
- **Treat any native query failure as “not managed.”** Rejected. Only the exact
  proved negative may be favorable; errors and timeouts are unknown. [inferred]
- **One-time qualification at selection.** Rejected because process restarts,
  path redirection, provider registration, mounts, and detector/profile versions
  change. Required requalification and explicit recovery are part of A.
  [inferred]
- **User consent or an environment override that bypasses the detector/profile.**
  Rejected. Consent accepts the residual risk; it does not make an unsupported
  target supported. [documented/inferred]
- **Automatic native-to-file, file-to-native, legacy-YAML, or alternate-path
  fallback.** Rejected because it creates unreviewed plaintext copies and can
  resurrect stale authority. [documented/inferred]
- **Add encryption and continue calling it plaintext file-v2.** Rejected as a
  category error. An encrypted vault needs a separate key-custody, unlock,
  backup, migration, and threat decision; ciphertext replication would still
  occur even if its confidentiality consequence differed. [documented/inferred]

## Open risks and cheapest decisive experiments

These risks do not block the A-versus-B decision. They block implementation or
an individual release profile. [inferred]

1. **Windows CfAPI packaged behavior.** Run one non-admin packaged harness on
   minimum/current Windows. Query a held ordinary local target, then register a
   throwaway dummy directory tree with `CfRegisterSyncRoot`, query root and
   descendant handles, unregister, and test requalification transitions. Pin
   `ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT` 390 versus every other HRESULT. This
   is the cheapest decisive check of the actual Rust/SDK seam. [inferred]
2. **macOS cross-provider negative.** Run a small signed packaged AudioGraph
   harness on minimum/current macOS and Apple silicon against ordinary internal
   APFS, iCloud Drive, and at least one third-party replicated File Provider.
   Bound the `getIdentifierForUserVisibleFile` callback; record only closed
   result classes. If the ordinary app cannot obtain a reliable negative for
   every required class, no macOS file-v2 profile is possible with this seam.
   [inferred]
3. **Linux process-view topology.** Run the packaged detector on native Linux
   across direct fixed ext4 plus actual NFS/CIFS, FUSE, read-only bind, USB,
   loop, virtual, and representative stacked topologies in the app's mount
   namespace. Use a dummy same-user watcher/copier only to prove the UI/non-claim
   remains honest; do not try to detect it. [inferred]
4. **Post-selection change cuts.** On every platform, pass initial selection,
   then alter the provider/mount/redirection state before each mutation barrier.
   Prove either handle-bound operation on the original qualified target or a
   latched closed recovery result before bytes, with no automatic resume or
   fallback. [inferred]

No real secret, provider account, private path in artifacts, or production
credential namespace is required for these experiments. [inferred]

## Concrete dependent-Seed proposals

This worktree is forbidden from editing Seeds. The root/conductor should apply
the following narrowly scoped updates; no new broad epic is needed.
[verified/inferred]

### `audio-graph-fb2b` — native/file-v2 adapters

Add acceptance that file-v2 implements `cloud-boundary-v1`, is selected only by
an explicit versioned acknowledgement before service initialization, and never
by native-store failure. Requalify the same held target at process/service
initialization, after reopen/recovery/path change, and under the mutation lock
immediately before bytes. Any target/profile change latches typed recovery and
cannot automatically resume. The private locator and native detector values do
not cross IPC/logging. Keep file-v2 disabled while its exact profile table is
empty. [inferred]

### `audio-graph-e241` — filesystem detector/policy fan-in

Pin the Windows favorable negative to exact
`ERROR_CLOUD_FILE_NOT_UNDER_SYNC_ROOT` 390 / `0x80070186`; distinguish 405 and
all other failures. Model cloud result time and requalification without
promising continuous enforcement. On macOS, treat File Provider success as
deny and do not admit its `NSFileNoSuchFileError` as a universal favorable
negative until `audio-graph-0e08` proves the packaged ordinary-app context.
Linux status copy must describe kernel-visible storage topology, not “not
synced.” [inferred]

### `audio-graph-55df` — conformance harness and manifest

Add the exact Windows, macOS, Linux, post-selection-change, and redaction
matrices from the release-proof section. Bind each file-v2 manifest to
`cloud-boundary-v1`, detector schema, persistence protocol, and packaged digest.
Include a dummy same-user copier as an honest-non-claim/UI fixture, not a
detector expectation. Profiles remain absent until reviewed evidence passes.
[inferred]

### `audio-graph-2c33` — Settings credential UX

Replace the unconditional “Saved keys stay local” copy for file-v2 with the
warning, acknowledgement, persistent badge/detail, denial, and recovery
propositions above. Never show a green “secure/local-only” state, never hide the
degraded badge after consent, and never offer “continue anyway.” Recovery
offers explicit native-store move, new qualified file-v2 selection, exact-old-
authority recovery, removal, or re-entry; no automatic fallback. Assert that
no private locator, provider/domain identity, or native error is requested over
IPC or logged from rejection objects. [inferred]

### Platform evidence children

- Update `audio-graph-4af0` with CfAPI registered-root/descendant, exact 390
  negative, other-error, reparse/redirection, and post-selection registration
  cases for the separate Windows file-v2 profile. [inferred]
- Keep `audio-graph-0e08` as the decisive signed cross-provider macOS query;
  explicitly withhold the APFS file-v2 profile if a universal bounded negative
  cannot be proved. [inferred]
- Update `audio-graph-ed26` with the dummy same-user copier non-claim and
  process-mount-namespace substitution cases while retaining its real
  remote/FUSE/topology negative matrix. [inferred]

Finally, attach this decision to closed `audio-graph-c7b2` as the resolution of
its bounded cloud-threat follow-up, then close `audio-graph-b686` only after the
research and ADR amendment integrate and the dependent Seed extensions are
recorded. [inferred]

## Decision summary

AudioGraph may design explicit plaintext file-v2 around **A**, but only as a
persistently degraded, profile-gated backend with a bounded “recognized
OS-declared managed root not observed at the last required check” claim.
AudioGraph cannot establish **B**. If “no replication” is required, file-v2 is
unsupported. Empty evidence profiles mean file-v2 remains unsupported today,
and no sentence in this decision should be read as release authorization.
[inferred]

Two more documentary sources would not change this recommendation. The
remaining uncertainties are behavioral and have the bounded, cheapest decisive
experiments above. The research stop condition is met. [inferred]

## Primary sources

Sources were checked on 2026-08-01.

### Repository

- [ADR-0035: Backend-owned credential service](../adr/0035-backend-owned-credential-service.md).
  [verified]
- [Credential v2 supported-filesystem policy](2026-08-01-credential-supported-filesystems.md).
  [verified]
- [Credential service threat model](../security/credential-service-threat-model.md).
  [verified]
- [Credential service rebuild plan](../plans/2026-07-31-credential-service-rebuild.md).
  [verified]
- `src-tauri/crates/ipc-contract/src/credential_contract.rs`,
  `src-tauri/src/credentials/mod.rs`,
  `src/components/settings/CredentialsPanel.tsx`, and
  `src/i18n/locales/en.json` at base `0a3d754`. [verified]
- Installed Windows SDK 10.0.22621.0 and 10.0.26100.0 `winerror.h` and
  `cfapi.h`; both define the 390/405 distinction recorded above. [verified]

### Windows

- Microsoft [`CfRegisterSyncRoot`](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfregistersyncroot).
  [documented]
- Microsoft [`CfGetSyncRootInfoByHandle`](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfgetsyncrootinfobyhandle).
  [documented]
- Microsoft [`CfUpdatePlaceholder`](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfupdateplaceholder)
  and [`HRESULT_FROM_WIN32`](https://learn.microsoft.com/en-us/windows/win32/api/winerror/nf-winerror-hresult_from_win32).
  [documented]
- Microsoft [`CreateFileW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)
  and [`GetFileInformationByHandle`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandle).
  [documented]

### macOS

- Apple [`isUbiquitousItemKey`](https://developer.apple.com/documentation/foundation/urlresourcekey/isubiquitousitemkey),
  [`volumeIsLocalKey`](https://developer.apple.com/documentation/foundation/urlresourcekey/volumeislocalkey),
  and [`volumeIsInternalKey`](https://developer.apple.com/documentation/foundation/urlresourcekey/volumeisinternalkey).
  [documented]
- Apple [`NSFileProviderManager.getIdentifierForUserVisibleFile`](https://developer.apple.com/documentation/fileprovider/nsfileprovidermanager/getidentifierforuservisiblefile%28at%3Acompletionhandler%3A%29),
  [`getDomainsWithCompletionHandler`](https://developer.apple.com/documentation/fileprovider/nsfileprovidermanager/getdomainswithcompletionhandler%28_%3A%29),
  and [File Provider overview](https://developer.apple.com/documentation/fileprovider).
  [documented]

### Linux

- Linux [`statx(2)`](https://man7.org/linux/man-pages/man2/statx.2.html),
  [`proc_pid_mountinfo(5)`](https://man7.org/linux/man-pages/man5/proc_pid_mountinfo.5.html),
  [`mount_namespaces(7)`](https://man7.org/linux/man-pages/man7/mount_namespaces.7.html),
  and [`open(2)` / `openat` rationale](https://man7.org/linux/man-pages/man2/open.2.html).
  [documented]
- Linux kernel [FUSE overview](https://docs.kernel.org/filesystems/fuse/fuse.html).
  [documented]
- Linux [`credentials(7)`](https://man7.org/linux/man-pages/man7/credentials.7.html),
  [`inotify(7)`](https://man7.org/linux/man-pages/man7/inotify.7.html), and
  [`fanotify(7)`](https://man7.org/linux/man-pages/man7/fanotify.7.html).
  [documented]
