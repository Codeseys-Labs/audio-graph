# Credential v2 supported-filesystem policy and detection seam

Date: 2026-08-01

Seed: `audio-graph-c7b2`

Gated workstream: `audio-graph-fb2b`

Related decisions:

- [ADR-0035: Backend-owned credential service](../adr/0035-backend-owned-credential-service.md)
- [Credential v2 lock and atomic-replacement primitives](2026-08-01-credential-lock-atomic-replace.md)
- [Credential service threat model](../security/credential-service-threat-model.md)

## Question and gated decision

How should CredentialService identify the filesystem and storage traits of the
**actual opened target directory**, and which filesystem families may host (a)
the non-secret credential-v2 authority journal and (b) the explicit plaintext
file-v2 secret backend on Windows, macOS, and Linux? [documented]

This gates the detector interface, dependency/API selection, closed status
contract, deterministic fixtures, and the release evidence that is required
before a platform/filesystem pair can be enabled. It does **not** implement the
detector or persistence backend. [documented]

Evidence labels in this note mean:

- **[verified]**: checked directly in this exact repository, locked source, SDK
  binding, or an executable dummy probe;
- **[documented]**: stated by an official OS, Rust, Tauri, or crate source; and
- **[inferred]**: the engineering conclusion drawn from that evidence.

## Recommendation

Adopt a two-stage, fail-closed policy: a target-native detector classifies a
held directory handle into closed traits, then a separate, versioned evidence
profile decides whether that exact `(target kind, platform, filesystem family,
detector schema)` is supported. A family name, mount flag, successful API call,
or local smoke test is only a **candidate classification**; none is packaged
durability or confidentiality proof. [inferred]

The initial accepted profile sets are deliberately empty:

```text
journal_supported_profiles = {}
file_v2_supported_profiles = {}
```

Therefore this research authorizes no filesystem today. It identifies the first
release-evidence candidates as Windows/local-fixed NTFS, macOS/local-internal
APFS, and Linux/local-fixed ext4, but each remains `durability_unproved` until
its target-native packaged evidence is accepted. ReFS, HFS+, btrfs, XFS, and
every other family require their own profile rather than inheriting a claim from
a similar filesystem. Confidence: **high** for the policy and Windows/Linux
detection seams; **medium** for the proposed macOS seam because no macOS target
or runner was available. [verified/inferred]

| Target | Runtime eligibility before profile lookup | Evidence profile required | Initial status |
| --- | --- | --- | --- |
| Non-secret authority journal | Held target is writable, stable, local, kernel/native, fixed/internal, outside an OS-declared cloud-sync root, in a closed candidate family, and enforces owner-only access controls. | Journal lock, replacement, visibility, restart/recovery, namespace-sync, abrupt-reset, and packaged actual-path evidence. | No profiles; fail with `durability_unproved`. |
| Explicit file-v2 | All journal eligibility, plus a separately selected path/format and a filesystem capable of enforceable owner-only metadata. | A **separate** file-v2 profile containing every journal gate plus permission-before-bytes, ACL/mode, residue, hard-link/reparse, and secret-file interruption evidence. | No profiles; fail with `durability_unproved`; after journal evidence exists, missing file-v2 evidence is `confidentiality_unproved`. |

[inferred]

File-v2 evidence may imply the journal evidence only when the evidence manifest
explicitly includes every journal gate. Journal evidence must never imply
file-v2 support. This preserves ADR-0035's distinction: the journal is
secret-free and recoverable against a native authority marker, whereas file-v2
is an explicit degraded-security backend whose file itself contains credential
bytes. [documented: [ADR-0035 journal contract](../adr/0035-backend-owned-credential-service.md#status-errors-and-concurrency),
[file backend contract](../adr/0035-backend-owned-credential-service.md#file-and-test-backends)]

## Policy scope and non-claims

### Inspect the opened target, not a configured string

Tauri's `app_config_dir()` is the platform config directory plus the bundle
identifier. Its platform bases are XDG config on Linux, Application Support on
macOS, and Roaming AppData on Windows. Environment, account, mount namespace,
redirection, junction, bind mount, enterprise profile, or filesystem changes
can therefore make a compile-time platform assumption wrong. The detector must
resolve the backend's real target, open the directory, retain that handle, and
classify the object reached by the handle. It must not classify a path prefix or
the boot volume. [verified: `src-tauri/tauri.conf.json` fixes the current bundle
identifier; documented: [Tauri `appConfigDir`](https://v2.tauri.app/reference/javascript/api/namespacepath/#appconfigdir),
[Tauri `configDir`](https://v2.tauri.app/reference/javascript/api/namespacepath/#configdir)]

For the journal, the inspected target is the actual parent of
`credential-v2/state.json` and `mutation.lock`. For file-v2, it is the separately
configured file-v2 parent, not automatically the journal parent. Native-store
failure cannot select or relocate file-v2. [documented: [ADR-0035](../adr/0035-backend-owned-credential-service.md#file-and-test-backends)]

### Meaning of network, cloud, removable, and unknown

The runtime policy denies every target the OS or filesystem identifies as
remote/network, FUSE/userspace, cloud/provider managed, removable/ejectable/
hot-pluggable, read-only, unknown, or internally inconsistent. A failed
required observation is `Unknown`, never the favorable value. [inferred]

No desktop OS exposes a proof that an arbitrary same-user backup, antivirus,
indexer, enterprise roaming agent, or synchronization program is not observing
an otherwise ordinary local directory. “Cloud managed” in this policy therefore
means a root or filesystem the OS exposes through its supported cloud/provider
or filesystem interfaces. An undeclared same-user copier remains outside the
detector's assurance, just as a same-user process that ignores the mutation lock
is outside the lock guarantee. Path-substring blocklists for vendor folder names
would not close that gap and are forbidden. [inferred]

That limitation is especially material for Windows Roaming AppData and Linux
`XDG_CONFIG_HOME`: a local NTFS/ext4 classification does not mean that account or
enterprise policy will never replicate the directory. The release security
statement must retain this bounded non-claim. If product policy requires proof
against arbitrary synchronization agents, the question is undecidable from
filesystem metadata and both profile tables must remain empty. [inferred]

## Repository and dependency facts

- ADR-0035 places the secret-free journal at the Tauri app-config path under
  `credential-v2/state.json`, requires owner-only metadata, same-directory temp,
  file sync, atomic rename, and parent sync where supported, and prohibits
  private paths or raw native errors in status surfaces. [verified:
  [ADR-0035](../adr/0035-backend-owned-credential-service.md#status-errors-and-concurrency)]
- The explicit file-v2 backend has a separate path and format, must establish
  owner-only storage before bytes, fails closed, synchronizes file and parent,
  and reports persistent degraded security. [verified:
  [ADR-0035](../adr/0035-backend-owned-credential-service.md#file-and-test-backends)]
- The accepted primitive research requires tests on the packaged application's
  actual target filesystem and says generic temporary-directory smokes are only
  supplementary. It already rejects unproved remote filesystems and requires
  abrupt-reset evidence for a durable claim. [verified:
  [runtime release gates](2026-08-01-credential-lock-atomic-replace.md#exact-runtime-release-gates)]
- The manifest directly uses `windows = 0.62.2` on Windows, but its current
  features cover security/threading rather than filesystem, Cloud Files, I/O,
  and storage-ioctl APIs. It directly uses `sysinfo = "0.39"`. [verified:
  `src-tauri/Cargo.toml`]
- The exact lock currently resolves `windows 0.62.2`, `sysinfo 0.39.6`,
  `rustix 1.1.4`, `libc 0.2.189`, and target-macOS
  `objc2-foundation 0.3.2`. `rustix` and `objc2-foundation` are transitive; Rust
  code must add direct target-specific dependencies before naming them. No
  `objc2-file-provider`, `objc2-disk-arbitration`, `udev`, or `libudev-sys`
  package is currently locked. [verified: `src-tauri/Cargo.lock`; locked
  `cargo tree` on Linux and `--target x86_64-apple-darwin`]

## Normative policy model

### Closed inputs and outputs

The product interface should be equivalent to the following pseudocode. Names
are illustrative, but the separation and closed fields are normative. [inferred]

```rust
enum PersistenceTarget { Journal, FileV2 }

enum Platform { Windows, MacOs, Linux }

enum FilesystemFamily {
    WindowsNtfs,
    WindowsRefs,
    MacApfs,
    MacHfsPlus,
    LinuxExt4,
    LinuxBtrfs,
    LinuxXfs,
    Other,
}

enum Ternary { Yes, No, Unknown }

struct FilesystemObservation {
    platform: Platform,
    family: FilesystemFamily,
    writable: Ternary,
    local: Ternary,
    kernel_native: Ternary,
    internal_fixed: Ternary,
    os_managed_cloud_root: Ternary,
    access_controls_enforced: Ternary,
    identity_stable: Ternary,
    detector_schema: u16,
}

trait FilesystemDetector {
    fn inspect(&self, target: &OpenedTarget)
        -> Result<FilesystemObservation, DetectorFault>;
}

enum FilesystemStatusCode {
    Supported,
    TargetUnavailable,
    InspectionUnavailable,
    TargetChanged,
    ReadOnly,
    Remote,
    UserspaceFilesystem,
    CloudManaged,
    RemovableOrHotplug,
    FilesystemUnproved,
    AccessControlUnproved,
    DurabilityUnproved,
    ConfidentialityUnproved,
}

struct FilesystemStatus {
    target: PersistenceTarget,
    code: FilesystemStatusCode,
    // Optional closed family; no path, volume/device/provider id, or OS prose.
    family: Option<FilesystemFamily>,
    detector_schema: u16,
}
```

`DetectorFault` is internal and immediately maps to a closed status. Neither it
nor the observation may implement a free-form serialized display that includes
native errors. Raw paths, mount points, mount sources, drive/volume labels,
serial numbers, file IDs, provider/domain identifiers, device names, symlink
targets, native codes, and native error prose are transient detector inputs only
and must not reach IPC, logs, analytics, crash breadcrumbs, docs, or Seeds.
[inferred]

Tests may use an opaque fake identity token solely to simulate identity changes;
production status does not serialize that token. The handle and transient
identity remain backend-owned and must not be accepted from IPC. [inferred]

### Pure evaluation and deterministic precedence

`evaluate(target, observation, profiles)` must be a pure function. Unknown is
never coerced to a favorable value, and the result precedence is fixed so the
same observation cannot produce platform-dependent user status. [inferred]

1. target missing/unopenable -> `TargetUnavailable`;
2. detector/API/parse/permission failure or any required `Unknown` ->
   `InspectionUnavailable`;
3. initial/final identity disagreement -> `TargetChanged`;
4. `writable != Yes` -> `ReadOnly`;
5. `local != Yes` -> `Remote`;
6. `kernel_native != Yes` -> `UserspaceFilesystem`;
7. `os_managed_cloud_root != No` -> `CloudManaged`;
8. `internal_fixed != Yes` -> `RemovableOrHotplug`;
9. family is `Other` or absent from the target's candidate set ->
   `FilesystemUnproved`;
10. `access_controls_enforced != Yes` -> `AccessControlUnproved` for either
    target; file-v2's profile then requires the stronger permission-before-bytes
    and secret-residue evidence;
11. no journal-equivalent evidence profile -> `DurabilityUnproved`;
12. file-v2 has journal-equivalent evidence but no exact file-v2 profile ->
    `ConfidentialityUnproved`; and
13. otherwise -> `Supported`.

An implementation may preserve the specific negative reason before the first
unknown, but it may not treat a later success as curing an earlier unknown. A
single final identity recheck is required after all secondary queries; a writer
must revalidate again at first use when it cannot operate relative to the held
directory handle. [inferred]

### Evidence profiles are release data, not runtime inference

An accepted profile is a reviewed build-time record keyed at minimum by target,
platform, closed filesystem family, detector schema, minimum OS release, and
the persistence-wrapper protocol version. It references a content-free test
artifact manifest and its digest. It never keys on or ships a user's volume ID,
device ID, path, provider ID, or machine identifier. [inferred]

A runtime observation can match only a profile compiled into that release.
Remote configuration, environment variables, a server response, or user consent
cannot add a profile. Updating the detector schema invalidates older profiles
unless a review explicitly proves backward equivalence. [inferred]

## Target-native detection seams

All three adapters must begin with an open directory handle and must verify that
the handle denotes a directory. They inspect metadata only; they never read a
journal or secret file. They retain only closed observations after evaluation.
[inferred]

### Windows: handle + volume GUID + Cloud Files + storage hotplug

Use the existing exact `windows = 0.62.2` dependency with additional direct
features `Win32_Storage_FileSystem`, `Win32_Storage_CloudFilters`,
`Win32_System_IO`, `Win32_System_Ioctl`, `Win32_System_SystemServices`, and
`Win32_System_WindowsProgramming`. The last two expose the filesystem capability
and drive-type constants used by the policy. The generated 0.62.2 source
contains the APIs and structures below. No WMI, PowerShell, registry scan,
drive-letter enumeration, or shell command is required. [verified: locally inspected
`windows 0.62.2` generated source; documented:
[`windows` feature list](https://docs.rs/crate/windows/0.62.2/features)]

1. Open the resolved target directory with `CreateFileW`,
   `FILE_READ_ATTRIBUTES`, read/write/delete sharing, `OPEN_EXISTING`, and
   `FILE_FLAG_BACKUP_SEMANTICS`. Follow the final junction/reparse target so the
   handle reaches the storage that would hold bytes. Confirm directory type and
   capture `FILE_ID_INFO` for race detection. Do not request or retain its path
   for status. [documented: [Microsoft `CreateFileW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
   [`GetFileInformationByHandleEx`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandleex)]
2. Call `GetVolumeInformationByHandleW`. Internally map the exact filesystem
   name to a closed family and inspect flags; `FILE_READ_ONLY_VOLUME` denies the
   target and both targets require `FILE_PERSISTENT_ACLS`. Do not request the volume
   label or serial. The API documents that SMB does not support volume-management
   functions; any failure is inconclusive/denied rather than a guessed family.
   A returned name or ACL flag is a capability observation, not durability
   proof. [documented: [Microsoft
   `GetVolumeInformationByHandleW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationbyhandlew)]
3. Query `GetFileInformationByHandleEx(FileRemoteProtocolInfo)`. A successful
   `FILE_REMOTE_PROTOCOL_INFO` result is a remote deny signal. Failure is **not**
   a documented portable “local” result and may be neutral only if the later
   positive GUID-volume/fixed-media checks all succeed; otherwise it is unknown.
   [documented: [Microsoft
   `FILE_REMOTE_PROTOCOL_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_remote_protocol_info);
   verified: the native local probe returned a generic invalid-parameter class]
4. Ask `GetFinalPathNameByHandleW(..., VOLUME_NAME_GUID)` only inside the adapter,
   derive the volume GUID root, and call `GetDriveTypeW`. Require `DRIVE_FIXED`.
   A missing GUID form, unsupported third-party driver, `DRIVE_REMOTE`,
   `DRIVE_REMOVABLE`, or any other result is denied. Never export the returned
   path. [documented: [Microsoft
   `GetFinalPathNameByHandleW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew),
   [`GetDriveTypeW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getdrivetypew)]
5. Open the volume device internally and call `DeviceIoControl` with
   `IOCTL_STORAGE_GET_HOTPLUG_INFO`. Require `MediaRemovable == false`,
   `MediaHotplug == false`, and `DeviceHotplug == false`. `DRIVE_FIXED` alone is
   insufficient because Microsoft directs USB-device callers to storage removal
   policy; a fixed-type USB disk may still be hot-pluggable. Failure is unknown.
   A future profile may add `StorageDeviceProperty` /
   `STORAGE_DEVICE_DESCRIPTOR.RemovableMedia` as a corroborating deny signal,
   but it cannot replace the hotplug query. [documented: [Microsoft
   `IOCTL_STORAGE_GET_HOTPLUG_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_storage_get_hotplug_info),
   [`STORAGE_HOTPLUG_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-storage_hotplug_info),
   [`STORAGE_DEVICE_DESCRIPTOR`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddstor/ns-ntddstor-_storage_device_descriptor)]
6. Call `CfGetSyncRootInfoByHandle` with `CF_SYNC_ROOT_INFO_BASIC`. Success means
   the object is underneath a registered Cloud Files sync root and is denied;
   the returned provider/file identifiers are discarded. Microsoft documents
   that the read-only query needs only `READ_ATTRIBUTES` and fails outside a
   sync root. Only the exact SDK “not under sync root” HRESULT may map to
   `os_managed_cloud_root = No`; every other failure is unknown. The SDK header
   used by the probe defines that Win32 sentinel as 390, and
   `HRESULT_FROM_WIN32` maps it to the observed `0x80070186`; this numeric detail
   must remain inside the adapter. [verified/documented: Windows SDK header and
   native probe; [Microsoft
   `CfGetSyncRootInfoByHandle`](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfgetsyncrootinfobyhandle),
   [`HRESULT_FROM_WIN32`](https://learn.microsoft.com/en-us/windows/win32/api/winerror/nf-winerror-hresult_from_win32)]
7. Re-read `FILE_ID_INFO` and volume information. Any file identity, volume
   serial (internal only), family, or flags disagreement maps to `TargetChanged`.
   [inferred]

The first candidate is only `WindowsNtfs` with ACL support on a fixed,
non-hotplug, non-remote, non-Cloud-Files volume. ReFS is already understood by
the volume API but must remain `FilesystemUnproved` until a distinct ReFS
profile exists. CSVFS, FAT/exFAT, UNC/SMB, WebDAV, removable media, cloud roots,
and unknown third-party filesystems are denied. [documented/inferred]

### Linux: fd + `statx` mount identity + mountinfo + bounded block topology

Add an exact direct target-Linux dependency on
`rustix = { version = "=1.1.4", features = ["fs"] }`. Version 1.1.4 is already
locked transitively, but transitive presence is not a public dependency. The
crate exposes `openat`, `fstat`, `fstatfs`, and Linux `statx`, including
`stx_mnt_id`; no direct `libc` struct layout is needed for these calls.
[verified: locked `rustix 1.1.4` source and `cargo tree`; documented:
[`rustix::fs`](https://docs.rs/rustix/1.1.4/rustix/fs/)]

1. Open the resolved target using `rustix::fs::openat` with
   `OFlags::RDONLY | DIRECTORY | CLOEXEC | NOFOLLOW`. Retain the descriptor and
   capture `fstat` type/device/inode. If the product intentionally supports a
   final symlink, open the resolved target separately and prove the writer uses
   the same object; silently weakening `NOFOLLOW` is not allowed. [documented:
   [Linux `openat(2)`](https://man7.org/linux/man-pages/man2/openat.2.html)]
2. Call `fstatfs(fd)` and `statx(fd, "", AT_EMPTY_PATH,
   STATX_BASIC_STATS | STATX_MNT_ID)`. `fstatfs` binds the filesystem magic to
   the handle; `statx` supplies a mount ID that corresponds to mountinfo field 1
   since Linux 5.8. If `STATX_MNT_ID` is unavailable, the initial policy denies
   rather than falling back to longest path-prefix matching. [documented:
   [Linux `fstatfs(2)`](https://man7.org/linux/man-pages/man2/fstatfs.2.html),
   [`statx(2)`](https://man7.org/linux/man-pages/man2/statx.2.html)]
3. Parse a bounded `/proc/self/mountinfo` snapshot from the **same mount
   namespace**, select exactly one row by mount ID, locate the `-` separator,
   and read only the closed filesystem type plus mount/superblock flags. Ignore
   unknown optional fields as the format requires. Cross-check the row's
   major:minor with `st_dev`. Never retain or export mount point, root, or mount
   source. Require writable mount and superblock. [documented:
   [Linux `proc_pid_mountinfo(5)`](https://man7.org/linux/man-pages/man5/proc_pid_mountinfo.5.html)]
4. Apply a positive family map, not a denylist. The first candidate requires
   both ext-family magic `0xEF53` and mountinfo type exactly `ext4`. The magic is
   shared by ext2/ext3/ext4, so it cannot distinguish ext4 alone. A mismatch is
   unknown. Every NFS, CIFS/SMB, FUSE/fuseblk, overlay, tmpfs, eCryptfs,
   9p/virtiofs, or unknown type fails before profile lookup. Btrfs and XFS are
   recognizable by their documented magic values but remain unproved
   candidates until their own profiles exist. [documented: filesystem magic
   values in [Linux `fstatfs(2)`](https://man7.org/linux/man-pages/man2/fstatfs.2.html);
   inferred]
5. For a block-backed candidate, resolve the exact `st_dev` major/minor through
   the kernel device model with a bounded, read-only topology adapter. Walk all
   stacked-device parents/slaves, not only the leaf. Deny if any layer is loop,
   network, userspace, removable, USB, FireWire, Thunderbolt, MMC/SD, hotplug, or
   unresolved; cycles, missing attributes, device-mapper ambiguity, multiple
   backers, and non-block-backed filesystems are unknown. Re-read identity after
   the query. [inferred]

Linux does not provide one generic “internal fixed disk” bit. The kernel's
`GENHD_FL_REMOVABLE` specifically means a device whose **media** can be removed
while the device remains, and says it must not be set for a device that itself
disappears. Therefore a zero `removable`/capability bit does not prove an
internal or non-hotplug device. [documented: [Linux generic block-device
capability](https://docs.kernel.org/block/capability.html)]

The kernel also warns applications not to depend on internal sysfs symlink
layout and to handle `/sys/subsystem`, `/sys/class`, and `/sys/block` evolution.
A small topology reader is acceptable only behind the detector trait, with
strict bounds, closed property parsing, fake fixtures, and a release profile for
each accepted topology. [documented: [Linux sysfs access
rules](https://docs.kernel.org/admin-guide/sysfs-rules.html); inferred]

`udev 0.9.3` offers `Device::from_devnum`, parent walking, attributes, and
properties, but it introduces `libudev-sys`/a target system-library build
dependency absent from the lock and current environment. `sd-device` similarly
requires `libsystemd`. Neither library supplies a universal “internal fixed”
proof, so this decision does not add it merely to move the same classification
policy behind another API. If direct sysfs support proves too brittle, the
cheapest follow-up is a target-Linux compile/package experiment comparing a
minimal `udev = "=0.9.3"` adapter with the current packaging image; until that
passes, an unrecognized topology is denied. [verified/documented:
[`udev::Device`](https://docs.rs/udev/0.9.3/udev/struct.Device.html),
[`sd-device`](https://www.freedesktop.org/software/systemd/man/latest/sd-device.html);
inferred]

### macOS: fd + `fstatfs` + Foundation volume traits, with no runtime claim

Add exact direct target-macOS dependencies on
`rustix = { version = "=1.1.4", features = ["fs"] }` and the already locked
`objc2-foundation = "=0.3.2"` with defaults disabled and only the `std`,
`NSArray`, `NSDictionary`, `NSError`, `NSString`, `NSURL`, and `NSValue`
features needed for URL resource values. This selection is source-level only;
it must pass a target-native compile before adoption. [verified: locked source
and target-macOS `cargo tree`; documented:
[`objc2-foundation 0.3.2`](https://docs.rs/objc2-foundation/0.3.2/objc2_foundation/)]

1. Open and retain the target directory fd with `openat`/`NOFOLLOW`; capture
   `fstat` device/inode. Call `fstatfs(fd)` and map only exact `f_fstypename =
   "apfs"` as the first candidate. Require `MNT_LOCAL`, writable state, and no
   `MNT_UNKNOWNPERMISSIONS`. Do not retain `f_mntonname` or `f_mntfromname`.
   [documented: [Apple `statfs(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/statfs.2.html)]
2. Use `rustix::fs::getpath(fd)` (`fcntl(F_GETPATH)`) only transiently to create
   an `NSURL`. Ask for `NSURLVolumeIsLocalKey`, `NSURLVolumeIsInternalKey`,
   `NSURLVolumeIsRemovableKey`, `NSURLVolumeIsEjectableKey`,
   `NSURLVolumeIsReadOnlyKey`, `NSURLVolumeSupportsAccessPermissionsKey`,
   `NSURLVolumeTypeNameKey`, and `NSURLIsUbiquitousItemKey`. Require known true
   for local/internal/access permissions, known false for removable/ejectable/
   read-only/iCloud, and a type consistent with APFS. Any missing, wrong-typed,
   or error result is unknown. [documented: [Apple URL resource
   keys](https://developer.apple.com/documentation/foundation/urlresourcekey),
   [`isUbiquitousItemKey`](https://developer.apple.com/documentation/foundation/urlresourcekey/isubiquitousitemkey),
   [Apple `fcntl(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html)]
3. Reopen the NSURL result and compare `st_dev`/`st_ino` and `f_fsid` with the
   held fd, then repeat `fstatfs`. A path/URL race or disagreement between
   `MNT_LOCAL`, Foundation locality, filesystem type, and identity is
   `TargetChanged`/unknown. The writer should use the held fd and `openat` where
   possible. [inferred]

Apple's Disk Arbitration description dictionary exposes device-internal,
media-removable/ejectable, volume-network, and volume-kind keys. The maintained
`objc2-disk-arbitration 0.3.2` crate binds them, but the relevant synchronous
`DADiskCreateFromVolumePath`/description surface is marked deprecated in the
generated bindings and is not locked. It is suitable as a conformance-harness
cross-check, not a required initial runtime dependency. Disagreement with the
Foundation/fd result must deny. [documented: [Apple Disk Arbitration
constants](https://developer.apple.com/documentation/diskarbitration/diskarbitration-constants),
[`objc2-disk-arbitration 0.3.2`](https://docs.rs/objc2-disk-arbitration/0.3.2/objc2_disk_arbitration/);
inferred]

`NSFileProviderManager.getIdentifierForUserVisibleFile(at:)` returns a provider
item/domain for a user-visible provider URL, and `objc2-file-provider 0.3.2`
binds that manager behind its `Extension` feature. Apple describes the manager
as communication from a File Provider extension or related process and says a
non-managed URL returns a no-such-file error. Without a macOS experiment proving
what an unrelated ordinary app observes for iCloud and third-party File Provider
roots, this API cannot establish a universal negative. If the experiment proves
it, add it only as a supplemental bounded-time query: success denies and any
unexpected error/timeout is unknown; never export the provider/domain values.
[documented: [Apple
`getIdentifierForUserVisibleFile`](https://developer.apple.com/documentation/fileprovider/nsfileprovidermanager/getidentifierforuservisiblefile%28at%3Acompletionhandler%3A%29),
[`objc2-file-provider 0.3.2`](https://docs.rs/objc2-file-provider/0.3.2/objc2_file_provider/);
inferred]

Because no macOS compiler target or runner was available, APFS is only a
candidate family and both macOS profile sets remain empty. There is no macOS
compile, File Provider, removable-media, or runtime support claim in this note.
[verified]

## Why the existing `sysinfo` dependency is not the authority detector

`sysinfo 0.39.6` enumerates disks/mount points and exposes filesystem bytes plus
`is_removable()`, which looks attractive but does not bind the answer to the
already opened credential target. Its public `Disk` also carries names and mount
paths that must not leak into status. [verified: exact locked `sysinfo 0.39.6`
source; documented: [`sysinfo::Disk`](https://docs.rs/sysinfo/0.39.6/sysinfo/struct.Disk.html)]

The exact Linux source parses the mount table and recognizes removable media
through a narrow USB by-id convention. That misses USB devices presented as
fixed media, non-USB hotplug, stacked block devices, and namespace/path races.
The exact Windows source uses volume enumeration/`GetDriveTypeW` and treats only
`DRIVE_REMOVABLE` as removable, while Microsoft's own drive-type documentation
requires a different storage removal-policy query for USB. [verified/documented]

Therefore `sysinfo` may remain a non-authoritative diagnostics dependency, but
CredentialService must not use `Disks` or mount-point prefix matching to grant a
profile. Reusing an already compiled crate is not evidence that its abstraction
matches this security decision. [inferred]

## Bounded executable probes

The probes used fixed metadata-only operations and lived under `/tmp`; neither
was added to the repository or dependency graph. They opened only directories,
did not create a product file, and did not read or write a credential value.
Outputs below are deliberately reduced to closed traits and contain no private
path, label, serial, device name, provider id, or native prose. [verified]

### Current Linux process: actual app-config directory

A small C probe opened the resolved existing AudioGraph app-config directory
with `O_RDONLY | O_DIRECTORY | O_CLOEXEC | O_NOFOLLOW`, then called `fstat`,
`fstatfs`, and `statx(AT_EMPTY_PATH, STATX_MNT_ID)`. It joined the returned mount
ID to `/proc/self/mountinfo` by field 1. [verified]

```text
open = yes
kind = directory
filesystem_candidate = ext-family magic + ext4 mount type
mount_view_writable = no
identity_join = exact mount id and device agreement
host = WSL2 Linux 6.18.33.2
```

The current restricted research mount namespace exposes that actual app-config
target as a read-only mount view of a writable ext4 superblock. The proposed detector
would correctly return `ReadOnly`, even though the same underlying filesystem is
writable in another view. This demonstrates why target handle + mount ID beats
“Linux normally uses ext4” or a boot-volume lookup. It is not native packaged
Linux, removable-media, lock/replace, or durability proof. [verified/inferred]

For comparison only, the same probe opened the clean worktree directory and
reported a writable ext4 view with a different mount ID. The differing result
for two directories on the same underlying device is expected and confirms
that mount-view flags are part of the target decision. [verified/inferred]

### Native Windows: actual app-config directory

A metadata-only PowerShell/C# P/Invoke probe executed under native Windows
10.0.22631 and opened the existing AudioGraph directory beneath native Roaming
AppData. It held the directory handle while invoking the exact APIs selected
above. [verified]

```text
open = yes
filesystem_candidate = NTFS
persistent_acls = yes
guid_volume_drive_type = fixed
cloud_files_sync_root = no (exact not-under-root sentinel)
storage_media_removable = no
storage_media_hotplug = no
storage_device_hotplug = no
remote_protocol_query = inconclusive on this local handle
```

The remote-protocol query's failure was not promoted to “local”; the positive
GUID/fixed/storage observations provided the candidate classification. This is
an important fixture requirement because treating any native error as the
favorable result would make the policy fail open. [verified/inferred]

The Windows result proves that the chosen metadata seam can classify this one
target on this one host without reading content. It does not prove the packaged
Rust binding/features, owner-only DACLs, a relocation, SMB/UNC, Cloud Files,
USB/removable media, atomic replacement, namespace durability, or abrupt reset.
NTFS therefore remains `DurabilityUnproved`. [verified/inferred]

### Probe conclusion

Two additional sources would not change the recommendation: the load-bearing
gap is no longer API discoverability but target-native conformance evidence.
The stop condition is met. The next useful evidence is an executable packaged
matrix, not more filesystem marketing or API summaries. [inferred]

## Deterministic detector and policy fixtures

Keep native acquisition/parsing behind three target adapters and test policy
independently through a fake `FilesystemDetector`. Fake inputs use only the
closed fields above plus a test-only opaque identity token. No fixture contains
a real home directory, drive letter, volume/device/provider ID, native message,
or secret-like string. [inferred]

The common policy suite must include at least: [inferred]

| Fixture | Journal result | File-v2 result |
| --- | --- | --- |
| All positive NTFS traits, no profiles | `DurabilityUnproved` | `DurabilityUnproved` |
| Accepted journal NTFS profile only | `Supported` | `ConfidentialityUnproved` |
| Accepted journal + distinct file-v2 NTFS profiles | `Supported` | `Supported` |
| Local APFS candidate, macOS profile absent | `DurabilityUnproved` | `DurabilityUnproved` |
| Local ext4 candidate, journal profile only | `Supported` | `ConfidentialityUnproved` |
| Remote/network yes or local unknown | `Remote` or `InspectionUnavailable` | same denial |
| FUSE/userspace yes or native unknown | `UserspaceFilesystem` or `InspectionUnavailable` | same denial |
| OS-managed cloud yes or cloud query unknown | `CloudManaged` or `InspectionUnavailable` | same denial |
| Removable/hotplug yes or fixed unknown | `RemovableOrHotplug` or `InspectionUnavailable` | same denial |
| Read-only mount/volume | `ReadOnly` | `ReadOnly` |
| Known family but filesystem/profile mismatch | `FilesystemUnproved` | `FilesystemUnproved` |
| ACL capability absent/unknown on otherwise supported family | `AccessControlUnproved` | `AccessControlUnproved` |
| Identity changes between first and final sample | `TargetChanged` | `TargetChanged` |
| Any adapter call/parse failure | `InspectionUnavailable` | `InspectionUnavailable` |

Windows adapter fixtures must additionally cover: [inferred]

- `FILE_REMOTE_PROTOCOL_INFO` success (deny), local-probe-style neutral failure
  with every positive local check (continue), and neutral failure plus any
  missing local check (unknown);
- `CfGetSyncRootInfoByHandle` success, exact not-under-root failure, and every
  other HRESULT class;
- fixed drive with any hotplug bit set, removable/remote/unknown drive type,
  volume GUID conversion failure, `FILE_READ_ONLY_VOLUME`, missing
  `FILE_PERSISTENT_ACLS`, NTFS/ReFS/other mapping, and `FILE_ID_INFO` change; and
- serialization/property tests that forbid extra strings and numeric native
  codes in `FilesystemStatus`.

Linux adapter fixtures must additionally cover: [inferred]

- a mountinfo parser with zero, one, duplicate, malformed, oversized, and
  unknown-optional-field records;
- mount-ID and major:minor mismatches, bind-mounted read-only view over writable
  superblock, and identity change during the snapshot;
- ext magic + `ext4`, ext magic + `ext3`, Btrfs/XFS recognized-but-unproved,
  NFS/CIFS/FUSE/overlay/9p/virtiofs/tmpfs/unknown, and filesystem string/magic
  disagreement; and
- direct fixed block, removable media, USB fixed media, Thunderbolt, loop,
  device-mapper/LUKS, LVM/RAID with multiple leaves, missing udev/sysfs data,
  cycles, excessive depth/count, and device topology change.

macOS adapter fixtures must additionally cover: [inferred]

- `apfs` plus all positive Foundation values, non-APFS, non-local, removable,
  ejectable, read-only, unknown permissions, and missing/wrong-typed resource
  values;
- iCloud true/false/query failure, File Provider success/not-managed/error/
  timeout if that supplemental query is adopted, and Disk Arbitration
  disagreement if the conformance cross-check is adopted; and
- fd -> transient NSURL -> reopened-fd identity match and every mismatch/race.

Every table-driven test must run each negative fixture with an artificially
present evidence profile to prove that runtime denial has precedence over
profile membership. Profile lookup is authorization only after detection; it is
never a bypass. [inferred]

## Exact release evidence required to add a profile

API availability, crate compilation, a detector smoke, filesystem
documentation, and an upstream library test suite are necessary inputs but are
not sufficient to add a profile. The release artifact must exercise dummy,
non-secret generations through the same packaged binary, Tauri path resolver,
detector, lock wrapper, replacement wrapper, recovery logic, and permission
adapter that production uses. [documented/inferred]

The evidence manifest must identify, without user-private locators: [inferred]

- release commit and packaged artifact digest/signature;
- detector schema, persistence-wrapper protocol, evidence-harness version, and
  target (`Journal` or `FileV2`);
- OS edition/build and minimum supported release, CPU architecture, closed
  filesystem family, filesystem/driver version where the OS exposes a safe
  closed value, and enabled filesystem feature flags that affect the protocol;
- storage class as closed local/internal/fixed or a negative test class, plus VM
  and cache/power-cut harness versions; and
- each test case's closed pass/fail/unsupported code and artifact digest, never
  a mount source, volume/device/provider ID, username, private path, or payload.

### Gates common to journal and file-v2 profiles

1. **Packaged actual target:** resolve the packaged target exactly as production,
   hold the directory handle, and prove detector observations and final identity
   recheck. Relocate/redirection fixtures must include local supported,
   remote/network, OS-cloud, removable/hotplug, FUSE/userspace where applicable,
   read-only, and unknown. Every negative case must deny before creating content.
   [inferred]
2. **Detector fault closure:** deny injected access errors, missing APIs,
   malformed/oversized mount/device data, timeouts, inconsistent sources, and
   target changes. Prove no raw input or native error reaches logs, IPC,
   analytics, crash breadcrumbs, or screenshots. [inferred]
3. **Lock protocol:** two packaged processes contend on the stable lock; the
   loser reaches a monotonic deadline without mutation. Prove normal and forced
   process release, stable lock identity, and lock-ignoring-adversary non-claim.
   [documented: [accepted lock gates](2026-08-01-credential-lock-atomic-replace.md#exact-runtime-release-gates)]
4. **Permissions and object type:** establish and verify the owner-only parent,
   lock, temp, and final journal metadata. Deny links/reparse surprises, wrong
   owner/type, granting ACLs, and insecure inherited metadata. Journal bytes are
   non-secret, but the directory/lock still guard authority and concurrency.
   [documented]
5. **Atomic visibility:** concurrent readers observe exactly one complete old or
   new dummy envelope, never missing/empty/prefix/mixed/malformed content, across
   many replacements. [inferred]
6. **Stage failures:** inject create, hardening, partial write, file sync,
   replace, namespace sync, readback, and cleanup failures. Only proved
   pre-replace failures may be `not_committed`; possibly post-replace failures
   reconcile under lock and return `commit_unknown`/`recovery_required` rather
   than success. [documented]
7. **Process interruption:** terminate at every persistence barrier and restart.
   Reconciliation sees old/new complete state or a closed recovery condition;
   no case silently initializes, imports legacy data, or publishes an unverified
   revision. [documented]
8. **Abrupt VM/power interruption:** hard reset a disposable native/virtual
   target repeatedly at the file-sync, replace, and namespace-sync barriers.
   Verify the exact promised outcome after reboot. Process kill is not storage
   cache/power-loss evidence. [inferred]
9. **Minimum and current OS:** run the full profile on the minimum supported OS
   and a current release for each claimed architecture/package format. A failing
   or unavailable required runner blocks the profile; another OS/filesystem
   cannot substitute. [inferred]
10. **Negative storage matrix:** run actual SMB/UNC or NFS/CIFS, FUSE/userspace,
    registered cloud/provider root, removable/hotplug storage, and unknown family
    examples and prove deterministic denial. A fake-only negative matrix is not
    enough to validate the native adapter. [inferred]

The journal abrupt-reset acceptance may include a recognized missing/corrupt
journal that deterministically returns `recovery_required` and can reconstruct
safe metadata from the retained native authority marker. It may not include
silent reinitialization or legacy resurrection. This is the journal's narrower
confidentiality consequence, not permission to skip atomicity/durability
testing. [documented/inferred]

### Additional gates for a file-v2 profile

File-v2 gets its own evidence manifest even on a filesystem with an accepted
journal profile. It must additionally prove: [inferred]

1. the explicit file-v2 target is separate from legacy `credentials.yaml` and
   selection happens before service initialization, never after a native-store
   error; [documented]
2. parent, lock, every same-directory temp, final file, backup/recovery object,
   and forced-exit residue are owner-only, with security established and
   inspected **before the first dummy secret byte**; [documented]
3. Unix effective mode is exactly `0600`, current owner, and no granting POSIX/
   NFSv4 ACL entry for group, other, or named non-owner principals; Windows uses
   the approved owner/System-only DACL policy at object creation and verifies it
   from the handle; macOS likewise validates mode, owner, and ACL rather than
   trusting umask; [inferred]
4. a permissive existing file and parent, umask `000`, inherited Windows ACL,
   named ACLs, symlink/junction/reparse substitution, hard links, sparse/clone
   behavior where relevant, and a concurrent lock-ignoring actor all fail closed
   without exposing a byte; [inferred]
5. interruption after any byte can leave only owner-only, bounded, ignored
   residue; recovery validates owner/type/no-link before opening or deleting it,
   and content canaries never appear in logs, errors, status, telemetry, crash
   diagnostics, docs, or filenames; [documented/inferred]
6. file and namespace durability plus exact readback of one complete secret
   generation under abrupt reset; loss or ambiguity is an explicit persistent
   degraded/recovery status, never empty/missing/success; and [inferred]
7. settings/UI/diagnostics always retain the prominent degraded-security status
   and never serialize the private file-v2 locator. [documented]

An evidence failure may still leave a journal profile valid if the failing case
is strictly file-v2-only. The manifest and review must make that separation
explicit rather than using one broad “filesystem supported” checkbox. [inferred]

### Candidate-specific first experiments

- **Windows NTFS:** packaged Windows on the minimum release; actual app-config
  NTFS plus relocated UNC/SMB, Cloud Files, and USB-fixed cases; exact
  `FlushFileBuffers`/native write-through replacement, DACL, contention,
  interruption, and abrupt-VM-reset matrix. ReFS is a separate experiment.
  [inferred]
- **Linux ext4:** native Linux rather than WSL; actual XDG app-config target on a
  recognized fixed block topology; ext4 magic/type cross-check; read-only bind,
  NFS/CIFS/FUSE, USB-fixed/removable, and device-mapper topology cases; exact
  `fsync(file) -> renameat -> fsync(parent)` and abrupt-reset matrix. Btrfs and
  XFS are separate experiments. [inferred]
- **macOS APFS:** signed packaged app on the minimum macOS release and current
  Apple silicon; actual Application Support target on internal APFS; Foundation
  resource values, iCloud, third-party File Provider, SMB, external APFS, and
  removable cases; `F_FULLFSYNC`, rename, directory `fsync`, ACL, restart, and
  abrupt-reset matrix. This is also the cheapest decisive test of whether the
  supplemental File Provider query gives a trustworthy negative to an unrelated
  ordinary app. [inferred]

## Adversarial pass and failure modes

- **TOCTOU after a good classification:** a symlink, junction, bind mount, or
  provider state can change after inspection. Holding the directory handle and
  comparing identity limits this; it does not help code that later re-resolves a
  string without revalidation. The persistence wrapper must consume the held
  target or prove identity again. [inferred]
- **Mount-view flags differ from superblock flags:** the Linux probe observed a
  read-only mount view of a writable ext4 superblock. Checking only `fstatfs`
  family or underlying device would grant incorrectly. Mount-ID-specific flags
  are mandatory. [verified/inferred]
- **Filesystem identity is not semantics:** NTFS/APFS/ext4 labels do not prove
  the selected lock/rename/sync implementation, hardware-cache behavior, ACL
  inheritance, File Provider exclusion, or abrupt-reset outcome. Profiles—not
  names—carry those claims. [inferred]
- **“Not removable” is weaker than “internal fixed”:** Linux explicitly scopes
  its removable capability to removable media, Windows documents USB removal
  policy separately from drive type, and virtual/stacked devices obscure backing
  topology. Missing ancestry information must deny. [documented/inferred]
- **Cloud negatives are bounded:** Windows CfAPI detects registered Cloud Files
  roots and Foundation detects iCloud; no selected API proves absence of an
  arbitrary same-user sync agent. A vendor-folder substring list would be both
  incomplete and a private-path leak. [documented/inferred]
- **Windows Roaming AppData:** the native probe's fixed local NTFS answer does
  not negate account/domain roaming semantics. Do not describe `local = Yes` as
  “never copied off machine.” [verified/inferred]
- **Async/provider query hangs:** if a future macOS File Provider call is used,
  it needs a bounded worker/deadline and timeout -> unknown. A delayed callback
  must not mutate an already returned status. [inferred]
- **Mount/device parsing as an attack surface:** mountinfo, sysfs, udev, and
  driver-returned names are untrusted metadata. Bound bytes, records, nesting,
  and retries; parse closed ASCII constants; never interpolate raw values into
  errors. [inferred]
- **Evidence drift:** OS updates, filesystem drivers, crate/API changes, wrapper
  changes, and detector-schema changes can invalidate a profile. Minimum/current
  release CI plus explicit profile-version review is required; runtime detection
  cannot infer continued durability. [inferred]
- **False sense of file-v2 security:** owner-only metadata protects against other
  ordinary users within the tested OS boundary, not malware/root/admin, the
  logged-in user, memory disclosure, arbitrary backup agents, or a compromised
  AudioGraph process. Persistent degraded-security status remains necessary.
  [documented/inferred]

## Rejected alternatives

- **Allow NTFS/APFS/ext4 immediately from documentation or these probes:**
  rejected because the release guarantee is about packaged lock, replacement,
  recovery, permissions, and abrupt-reset behavior, not type detection.
  [verified/inferred]
- **One supported-filesystem set for journal and file-v2:** rejected because it
  lets non-secret journal evidence authorize plaintext-secret storage without
  permission-before-bytes and residue proof. [documented/inferred]
- **Filesystem-name denylist:** rejected because new, aliased, stacked, or
  userspace filesystems would fall through as allowed. Use a positive family map
  and exact evidence profiles. [inferred]
- **`sysinfo::Disks` / longest mount-point prefix:** rejected because enumeration
  is not bound to the held target and its removable abstraction is insufficient
  for this security decision. [verified/inferred]
- **Drive type, `statfs` flags, or `removable=0` alone:** rejected because each is
  only one trait and has documented blind spots. [documented]
- **Path or vendor-name heuristics for network/cloud/removable:** rejected as
  racy, incomplete, localization/configuration dependent, and incompatible with
  the private-path status contract. [inferred]
- **Treat native query failure as the favorable result:** rejected. Only a
  documented exact negative sentinel may produce `No`; all other failures are
  unknown. [inferred]
- **WMI, PowerShell, `diskutil`, `mount`, `findmnt`, `udevadm`, or shell-outs in
  product detection:** rejected because they add executable availability,
  output parsing/localization, process, and private-path surfaces when direct
  APIs exist. The PowerShell used here was only a native dummy-probe harness.
  [inferred]
- **Add `udev`, Disk Arbitration, or File Provider crates solely because an API
  exists:** rejected until the exact target compile/package and behavioral gap
  they close is proved. Dependency presence is not a policy. [inferred]
- **User override to force support:** rejected. An explicit choice may select
  file-v2, but it cannot turn an unknown/unproved filesystem into supported or
  suppress the degraded-security status. [documented/inferred]

## Open risks and cheapest decisive experiments

1. **macOS is unverified.** The cheapest decisive experiment is one small,
   signed, packaged APFS harness using the exact proposed direct dependencies.
   It opens the actual target, cross-checks fd/Foundation identity and traits,
   then runs internal APFS, external/removable APFS, SMB, iCloud, and third-party
   File Provider targets. Until it passes, macOS profiles stay empty. [inferred]
2. **Linux internal/fixed classification has no one-bit API.** The cheapest
   decisive experiment is a read-only topology harness on the release image
   across direct NVMe/SATA, USB-fixed, removable media, LUKS/LVM/RAID, loop,
   virtual disks, and a read-only bind. If a strict sysfs adapter cannot cover
   the supported topology without unstable assumptions, compare a minimally
   pinned `udev 0.9.3` build/package; do not guess. [inferred]
3. **Cloud-sync absence is inherently bounded.** Decide explicitly whether
   “OS-declared managed root” is the accepted product threat boundary. If the
   requirement is instead “no process can replicate this directory,” no
   filesystem detector can decide it; keep file-v2 unsupported and rely on the
   native credential stores. [inferred]
4. **Windows profile roaming is distinct from filesystem locality.** A product
   decision may prefer a non-roaming local-data root for the journal or file-v2,
   but changing ADR-0035's Tauri app-config location requires a separate path/
   migration decision. This research does not silently move it. [inferred]

## Out-of-scope Seed proposals

These are narrow proposals only. Assignment scope forbids Seeds edits, so they
were not filed or investigated beyond what was necessary for this decision.
[verified]

- Child of `audio-graph-fb2b`: **Implement closed credential filesystem detector
  seam and pure policy evaluator**; own target-specific adapters, fake fixtures,
  status redaction tests, and empty evidence tables. Do not own persistence
  primitives. [inferred]
- Child of `audio-graph-fb2b`: **Build credential filesystem conformance harness
  and evidence-manifest schema**; use dummy generations, packaged actual paths,
  negative relocation matrix, abrupt-reset automation, and content-free
  artifacts. [inferred]
- Child of that conformance Seed: **Qualify Windows NTFS journal, then file-v2**;
  keep two independently reviewable profiles and leave ReFS separate. [inferred]
- Child of that conformance Seed: **Qualify native Linux ext4 journal, then
  file-v2 and select the fixed-device topology adapter**; leave WSL, btrfs, and
  XFS separate. [inferred]
- Child of that conformance Seed: **Compile/probe macOS APFS detection and File
  Provider behavior before any APFS profile**; include signed packaged and
  external/provider negative targets. [inferred]
- Architecture follow-up: **Decide whether Windows credential journal storage
  remains under Roaming AppData or migrates to local app data**; include upgrade,
  downgrade, native marker, and recovery consequences. [inferred]

## Primary sources

### Repository

- [ADR-0035: Backend-owned credential service](../adr/0035-backend-owned-credential-service.md).
  [verified]
- [Credential service rebuild plan](../plans/2026-07-31-credential-service-rebuild.md).
  [verified]
- [Credential service threat model](../security/credential-service-threat-model.md).
  [verified]
- [Credential v2 lock and atomic-replacement primitives](2026-08-01-credential-lock-atomic-replace.md).
  [verified]
- Exact manifest, lock, Tauri identifier, and locked `cargo tree` for Linux and
  target macOS in this worktree. [verified]

### Windows

- Microsoft [`CreateFileW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
  [`GetFileInformationByHandleEx`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfileinformationbyhandleex),
  and [`FILE_REMOTE_PROTOCOL_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_remote_protocol_info).
  [documented]
- Microsoft [`GetVolumeInformationByHandleW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationbyhandlew),
  [`GetFinalPathNameByHandleW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew),
  and [`GetDriveTypeW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getdrivetypew).
  [documented]
- Microsoft [`CfGetSyncRootInfoByHandle`](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfgetsyncrootinfobyhandle)
  and [`CF_SYNC_ROOT_INFO_BASIC`](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/ns-cfapi-cf_sync_root_basic_info).
  [documented]
- Microsoft [`IOCTL_STORAGE_GET_HOTPLUG_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-ioctl_storage_get_hotplug_info),
  [`STORAGE_HOTPLUG_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ns-winioctl-storage_hotplug_info),
  and [`STORAGE_DEVICE_DESCRIPTOR`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntddstor/ns-ntddstor-_storage_device_descriptor).
  [documented]
- Exact [`windows 0.62.2` feature list](https://docs.rs/crate/windows/0.62.2/features)
  and generated bindings. [verified]

### Linux and Rust

- Linux [`fstatfs(2)`](https://man7.org/linux/man-pages/man2/fstatfs.2.html),
  [`statx(2)`](https://man7.org/linux/man-pages/man2/statx.2.html), and
  [`proc_pid_mountinfo(5)`](https://man7.org/linux/man-pages/man5/proc_pid_mountinfo.5.html).
  [documented]
- Linux [generic block-device capability](https://docs.kernel.org/block/capability.html)
  and [sysfs access rules](https://docs.kernel.org/admin-guide/sysfs-rules.html).
  [documented]
- Exact [`rustix 1.1.4` filesystem API](https://docs.rs/rustix/1.1.4/rustix/fs/)
  and source; [`udev 0.9.3 Device`](https://docs.rs/udev/0.9.3/udev/struct.Device.html)
  as the evaluated but unselected alternative. [verified]

### macOS

- Apple [`statfs(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/statfs.2.html)
  and [`fcntl(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html).
  [documented]
- Apple [`URLResourceKey`](https://developer.apple.com/documentation/foundation/urlresourcekey),
  including [`isUbiquitousItemKey`](https://developer.apple.com/documentation/foundation/urlresourcekey/isubiquitousitemkey),
  and [Disk Arbitration constants](https://developer.apple.com/documentation/diskarbitration/diskarbitration-constants).
  [documented]
- Apple [`NSFileProviderManager.getIdentifierForUserVisibleFile`](https://developer.apple.com/documentation/fileprovider/nsfileprovidermanager/getidentifierforuservisiblefile%28at%3Acompletionhandler%3A%29).
  [documented]
- Exact [`objc2-foundation 0.3.2`](https://docs.rs/objc2-foundation/0.3.2/objc2_foundation/),
  [`objc2-disk-arbitration 0.3.2`](https://docs.rs/objc2-disk-arbitration/0.3.2/objc2_disk_arbitration/),
  and [`objc2-file-provider 0.3.2`](https://docs.rs/objc2-file-provider/0.3.2/objc2_file_provider/)
  bindings evaluated above. [verified]
