# Canonical directory durability and cross-process exclusion

Date: 2026-08-14

Seed: `audio-graph-8e73`

Source base: `e09ccdbef4aa066a6686e10f1c3ab1a8c87434bc`

## Question and gated decision

What exact barriers and refusal semantics can a Rust 1.95 desktop application
truthfully implement for an existing-file append, a first-created directory
entry, an atomic quarantine rename, truncation through the verified source
handle, and shared/exclusive cross-process locks on Windows, macOS, and Linux?

This gates the narrow `audio-graph-8e73` decision: which operation may return
canonical `Accepted`, which must return a weaker or indeterminate result, and
which platform-specific implementation and CI evidence are prerequisites. It
does not decide session semantics, projections, adapters, UI, or providers.

## Recommendation

**Recommendation: implement `Accepted` as a successfully completed, named OS
barrier protocol, not as an absolute power-loss guarantee. Confidence: high for
existing-file append, locked-handle truncation, and Linux local-filesystem
namespace barriers; medium for macOS namespace barriers pending an APFS CI
probe; high that current public Windows APIs do not justify a general
parent-directory `Accepted` claim.**

The recommended AudioGraph policy is **[inferred]** and should:

1. serialize every canonical mutation with one stable coordination-file lock;
2. retain one open, exclusively locked source `File` from identity validation
   through any source truncation;
3. flush userspace buffering, then use `File::sync_all`, checking every result;
4. when a pathname is first created or changed, synchronize every changed
   parent directory on Linux and macOS;
5. durably register a completed quarantine before truncating the source; and
6. return `Accepted` only after every applicable barrier succeeds. A failure
   after a visible or possibly visible mutation is `DurabilityIndeterminate`
   and requires idempotent reconciliation; it is not a rollback and not
   `Accepted`.

**[inferred from verified implementation mappings]** Rust 1.95 std-only APIs
are sufficient for the regular-file barriers, locks, truncation, Unix rename,
and Linux directory sync. Rust 1.95 implements Apple `sync_all` with
`F_FULLFSYNC`, so no `libc` dependency is needed merely to obtain the stronger
Apple file barrier. [R1] [R3] [R4]

Windows is the exception for namespace claims. `File::sync_all` reaches
`FlushFileBuffers`, and `std::fs::rename` reaches `MoveFileExW` (with a
`SetFileInformationByHandle` fallback), but std does not request
`MOVEFILE_WRITE_THROUGH`; Microsoft does not list `FlushFileBuffers` among the
functions supported on directory handles; and `ReplaceFileW` explicitly says
its write-through flag is unsupported. Therefore first-create and quarantine
rename on Windows must remain non-`Accepted` under the currently documented
contract. [R2] [R5] [W1] [W3] [W4] [W6]

## The claim boundary

- **Visible** means the write/rename/truncate syscall returned success and a
  later lookup can observe the new state. It does not mean the state survived
  loss of the OS cache. **[documented]** Linux and Darwin provide a
  same-filesystem rename operation; Darwin's explicit through-crash
  destination-name continuity applies when `new` already exists, not when a
  fresh quarantine name is first published. Neither definition makes a
  completed Rust `rename` call by itself a cross-platform storage barrier.
  [L2] [A3] [R2]
- **Ordered** means the application does not begin the dependent step until the
  preceding barrier returned. Ordering is a protocol property; it does not
  upgrade an unsupported or ignored device flush. **[inferred]** Apple warns
  that even `F_FULLFSYNC` is best effort under sudden power loss, and its older
  manpage notes that some drives ignore flush requests. [A1] [A2] [A5]
- **OS-barrier durable** means the documented synchronization call returned
  success. Linux says `fsync` waits until the device reports completion;
  Microsoft says `FlushFileBuffers` writes buffered information to the device;
  Darwin `F_FULLFSYNC` asks the drive to flush buffered data to permanent
  storage. **[documented]** This is the strongest truthful portable product
  contract; hardware, firmware, remote servers, and filesystems can weaken it.
  [L1] [W1] [A1] [A2]
- **Canonical `Accepted`** should mean OS-barrier durable on an explicitly
  supported and tested filesystem, with all protocol barriers successful. It
  must not mean proof against arbitrary hardware failure. **[inferred]**

## Exact operation matrix

| Operation | Linux | macOS | Windows | May return `Accepted`? |
| --- | --- | --- | --- | --- |
| Existing-file append | Hold exclusive `File` lock; seek to EOF and write while locked; flush any `BufWriter`; `File::sync_all` (`fsync`). | Same; Rust 1.95 `sync_all` uses `F_FULLFSYNC`. | Same; use a read/write handle rather than append-only because Rust documents append-only lock failure; `sync_all` uses `FlushFileBuffers`. | Yes on an allowlisted/tested filesystem after every call succeeds. [R1] [R3] [R4] [R5] |
| First create | `create_new`; write; `sync_all` file; open parent directory and `sync_all` it. Linux explicitly requires parent-directory `fsync` for the entry. | `create_new`; write; `sync_all` file; open parent and `sync_all` it. Accept only after CI proves the latter succeeds on supported APFS hosts, because Rust calls filesystem-specific `F_FULLFSYNC`. | `create_new`; write; `sync_all` file establishes the documented file barrier, but no public parent-directory barrier was found. | Linux: yes. macOS: conditional on APFS probe. Windows: no; return `NamespaceDurabilityUnsupported`. [L1] [R4] [A2] [A6] [W1] [W6] |
| Publish quarantine temp by atomic rename | Create the temp in the final quarantine directory; `sync_all` it; same-filesystem `rename`; `sync_all` the quarantine parent. If parents differ, sync both changed parents. | Same. Darwin's through-crash destination-name continuity is documented for replacement of an existing destination, not fresh-name publication; a fresh quarantine name depends on rename success, the directory barrier, and recovery. Directory `F_FULLFSYNC` still needs the APFS probe. | `std::fs::rename` provides a namespace operation but requests no write-through. `MoveFileExW(MOVEFILE_WRITE_THROUGH)` is only the closest documented non-std request; its explicit flush guarantee discusses copy/delete, which is non-atomic across volumes. | Linux: yes after directory barriers. macOS: conditional. Windows: no general atomic-and-durable claim from cited public contracts. [L2] [L1] [A3] [R2] [R5] [W3] |
| Replace an existing quarantine name | Avoid this: allocate a collision-resistant unique final name and, while holding the coordination lock, refuse a destination already present before rename. Unix `rename` and Rust `rename` otherwise replace. | Same. | Windows replacement behavior also varies with filesystem/OS support and open handles; `ReplaceFileW` can leave several explicitly documented partial name/attribute states on failure and its write-through flag is unsupported. | Do not intentionally rely on replacement semantics; a detected collision is a typed refusal. Uncooperative pathname races remain outside the advisory-lock contract. [R2] [W4] |
| Truncate verified source | On the still-locked source `File`, `set_len(valid_prefix)` then `sync_all`; Linux maps to `ftruncate` and `fsync`. | Same; `sync_all` is `F_FULLFSYNC`. | Same; Rust uses `FILE_END_OF_FILE_INFO`, then `FlushFileBuffers`. | Yes only after quarantine and manifest prerequisites were already accepted and the truncate barrier succeeds. [R1] [R4] [R5] [L4] |
| Shared/exclusive coordination | Rust `File::{lock, lock_shared, try_lock, try_lock_shared}` maps to `flock`; locks are advisory and remote mounts vary. | Same `flock`; advisory, cooperating processes only. | Rust maps to whole-file `LockFileEx`; byte-range reads/writes are denied according to lock type, but mapped views bypass locks and delete/rename exclusion is controlled separately by sharing modes. | **[inferred AudioGraph policy]** Eligible as a cooperation precondition, never as proof against an uncooperative process. [R3] [R4] [R5] [L5] [A4] [W2] [W5] |

### Existing-file append

**[verified]** Rust documents `sync_all` as attempting to synchronize file
content and metadata and warns that close/drop errors are ignored. In 1.95 the
implementation is Linux `fsync`, Apple `F_FULLFSYNC`, and Windows
`FlushFileBuffers`. [R1] [R3] [R4] [R5]

Use a read/write handle, take the exclusive lock, seek to EOF, and write the
complete framed record under that lock. This avoids depending on Windows'
append-only handle, which Rust says cannot be locked. **[inferred]** Kernel
`O_APPEND` is atomic for each Linux `write`, but NFS must simulate append and
can corrupt concurrent appends; the application lock and local-filesystem
allowlist remain necessary. [R3] [L3]

After any `BufWriter::flush`, call `sync_all`. A write, flush, or file-sync
error can follow a partial or visible mutation and therefore means
`DurabilityIndeterminate { stage, recovery_key }` unless the implementation can
prove that no bytes or metadata became visible. Retry must inspect
framed-record identity rather than blindly append. **[inferred]**

### First-created entry

**[documented]** Linux `fsync(file)` does not necessarily persist the link in
its containing directory; Linux explicitly requires `fsync` of a directory
descriptor. The barrier is therefore `create_new -> write -> file.sync_all ->
parent.sync_all`. [L1]

**[verified]** On Apple targets Rust 1.95 sends `F_FULLFSYNC` for `sync_all`.
XNU routes `F_FULLFSYNC` to the filesystem implementation; the public Apple
material describes a best-effort persistence request but does not promise its
success for an APFS directory descriptor. The same sequence is the correct
attempt, but `Accepted` is conditional on a real APFS runner demonstrating
that opening the parent and `sync_all` both succeed. [R4] [A2] [A5] [A6]

**[documented gap]** Windows documents opening a directory with
`FILE_FLAG_BACKUP_SEMANTICS`, but its directory-handle page enumerates supported
operations without `FlushFileBuffers`. `FlushFileBuffers` itself requires a
write-capable file handle and promises to flush the specified file; it does not
name the containing directory entry. The first-created entry therefore cannot
receive `Accepted` solely because the new file's `sync_all` succeeded. [W1]
[W6]

### Quarantine publication and source truncation

The safe sequence is:

1. acquire the stable coordination lock and exclusively lock/open the source;
2. validate identity and read/copy through that same source handle;
3. `create_new` a temp inside the final quarantine directory, write it, and
   `sync_all` the temp;
4. rename the temp to a unique final name on the same filesystem/volume;
5. synchronize every parent directory changed by the rename;
6. persist a typed `quarantine_prepared` transition through the authoritative
   manifest abstraction and complete every file/namespace barrier required by
   its selected physical form;
7. only then call `set_len(valid_prefix)` on the original locked source handle
   and `sync_all` it;
8. persist the typed manifest-completion transition and complete its selected
   barriers; and
9. only then publish acknowledgement.

This protocol deliberately does **not** choose an append-only event log, an
atomic snapshot replacement, or another manifest representation.
`audio-graph-661f` owns that physical-form decision and its corresponding
barriers. Choosing an event stream requires backflow into ADR-0037 before
implementation. **[documented project dependency: `audio-graph-661f`]**

This ordering prevents source destruction before there is a synchronized,
named, durably registered recovery copy. **[inferred]** A crash or error after
steps 4 through 8 can legitimately leave temp/final quarantine, manifest, and
source in overlapping states. Recovery must use identities and state
transitions to complete or retain them; it must not infer failure from duplicate
bytes or attempt an eager rollback.

**[documented]** Linux and Darwin provide same-filesystem rename and report a
cross-device error; Darwin's explicit through-crash continuity concerns an
existing destination name. [L2] [A3] [R2]

**[inferred AudioGraph policy]** Treat successful rename as atomic visibility,
refuse cross-device moves rather than degrading to copy/delete, and do not
extend Darwin's replacement-only crash wording to a fresh quarantine name.

On Windows, do not substitute `ReplaceFileW`: Microsoft lists failure results
where the replaced file disappears, the replacement keeps its old name, or
attributes/streams have already transferred, and `REPLACEFILE_WRITE_THROUGH`
is unsupported. **[documented]** [W4]

### Cross-process lock contract

**[documented]** Rust's lock family is stable in 1.95. `try_lock` distinctly
reports `WouldBlock`, while other failures are I/O errors. Re-locking the same
handle or clone is platform-dependent and can deadlock. [R3]

Unix `flock` is advisory: nonparticipants can still read, write, rename, or
unlink. Linux NFS and CIFS behavior changes with kernel, mount, server, and
protocol; CIFS can make locks mandatory, while NFS emulates whole-file locks.
**[documented]** [L5] [A4]

Windows `LockFileEx` locks the range used by Rust (effectively the whole file)
and denies normal range I/O according to shared/exclusive mode, but memory maps
bypass it. Rename/delete access is controlled by `CreateFile` share flags;
Rust's default Windows share mode allows read, write, and delete/rename.
**[verified]** [R5] [W2] [W5]

**[inferred AudioGraph policy]** Use a stable coordination file that is never
renamed as the transaction lock; keep the source data handle exclusively
locked through validation and truncation; and require readers needing a
canonical snapshot to take a shared lock on the same coordination file. One
transaction object owns the lock once. The Rust standard library suffices for
this cooperative policy, but the lock is not a namespace lease. Remote mounts
are ineligible for `Accepted` until explicitly qualified.

## Required result and refusal semantics

The storage layer needs typed outcomes at the exact barrier boundary:

- `Accepted { barrier: FileAndNamespace, filesystem }`: all required file,
  directory, manifest, and source barriers succeeded on a supported
  filesystem.
- `Contended`: `try_lock` returned `WouldBlock`; no mutation begins.
- `NamespaceDurabilityUnsupported { platform, filesystem, operation }`: the
  platform/filesystem has no qualified namespace barrier (the required Windows
  result today, and the macOS result if directory `sync_all` is rejected).
  This refusal is valid only when capability is established before mutation;
  discovering it after a possibly visible mutation is
  `DurabilityIndeterminate`.
- `IoFailedBeforeAcceptance { stage, source }`: reserved for a failure proven
  to occur before any visible mutation, such as failure to open an existing
  coordination/source file or failure of pre-mutation validation.
- `DurabilityIndeterminate { stage, recovery_key }`: any visible or possibly
  visible partial write, userspace flush, file sync, rename, directory sync,
  manifest-transition persistence, truncation, or source sync failed or has an
  uncertain result. Recovery owns the state; callers must not repeat a
  destructive step blindly.
- `IdentityChanged` or `LockLost`: only when detected before mutation, refuse
  because the open source identity no longer matches or the transaction does
  not own its lock. Detection after any possibly visible mutation is
  `DurabilityIndeterminate`.
- `CrossDeviceRenameRefused`: only when a preflight proves the mismatch before
  creating a visible temp, refuse and use an explicit copy-to-final-directory
  protocol. If rename returns `EXDEV` after the temp exists, return
  `DurabilityIndeterminate` and preserve the temp/source; never call a
  convenience move that silently becomes copy/delete.

**[inferred]** Raw OS error codes should remain attached for diagnostics, but
product correctness must branch on the typed stage/outcome, not on a brittle
enumeration of errno or Win32 values. Linux can report delayed `EIO`, `ENOSPC`,
or quota failure at `fsync`; Windows APIs require `GetLastError`; Darwin can
return `EINVAL`/`EIO` for unsupported or failed synchronization. [L1] [W1]
[W2] [A1]

## Std-only and platform-specific requirements

- **Linux — inferred implementation conclusion:** std-only suffices for the
  selected contract: `File::lock`, `File::set_len`, `File::sync_all`,
  `File::open(parent)`, and `std::fs::rename`. No unsafe code or locking crate
  is justified. [R1] [R2] [R4]
- **macOS — inferred implementation conclusion:** std-only suffices for the
  intended file and directory attempts in Rust 1.95 because `sync_all` already
  calls `F_FULLFSYNC`. Do not add `libc` merely to call the same command. A
  `cfg(target_os = "macos")` branch is still useful for qualifying/reporting
  APFS directory refusal. [R4]
- **Windows — inferred implementation conclusion:** std-only suffices for file
  sync, cooperative locks, and locked-handle truncation, but not for a
  requested write-through rename or a strict handle-based rename/share-mode
  protocol. The closest experiment would add a **direct**, cfg-Windows
  `windows-sys` dependency and a very small, reviewed unsafe wrapper for
  `MoveFileExW`/`SetFileInformationByHandle` and exact flags.
  `OpenOptionsExt::{access_mode, share_mode, custom_flags}` is stable, but
  Win32 constants and handle operations still need an owned binding. Even that
  experiment must not upgrade Windows namespace operations to `Accepted`
  without an authoritative contract or an explicitly narrower product
  definition. [R5] [R6] [W3] [W7] [W8]

## Minimum evidence before returning canonical `Accepted`

1. **Contract gate:** the result type exposes the barrier stage and has no path
   from write/rename/truncate success directly to `Accepted`.
2. **Rust 1.95 gate:** compile and run with Rust 1.95; verify the target-specific
   source mappings used above have not been replaced by a weaker call.
3. **Filesystem gate:** record filesystem type at runtime or conservatively
   allowlist qualified local filesystems. Initial CI scope should be Linux
   ext4 plus one of XFS/Btrfs, macOS APFS, and Windows NTFS; tmpfs, network
   mounts, FUSE, CIFS/SMB, NFS, cloud placeholders, FAT/exFAT, and removable
   media remain non-`Accepted` until separately qualified.
4. **Directory probe:** Linux and macOS runners must create a new file, sync
   the file, open its parent, and successfully `sync_all` the parent. The macOS
   probe is the cheapest decisive experiment for the APFS uncertainty. Windows
   must assert typed refusal rather than skip the check.
5. **Cross-process lock matrix:** on all three OSes, prove exclusive-vs-exclusive,
   shared-vs-exclusive, release-on-process-death, second-handle behavior, and
   the Windows append-only refusal. Also prove an uncooperative process is
   outside the lock contract rather than silently considered safe.
6. **Rename matrix:** same-directory and cross-directory same-filesystem rename,
   destination collision, open readers/writers, permission denial, and
   cross-device refusal. Windows tests must cover sharing violations and
   partial/error reconciliation without labeling success durable.
7. **Crash-cut subprocess matrix:** kill before and after write, userspace
   flush, file sync, quarantine rename, directory sync, manifest-prepare
   persistence, source truncate, source sync, persisted manifest completion,
   pre-acknowledgement, and post-acknowledgement. The manifest-completion cut is
   explicitly between source sync and acknowledgement. On restart, every state
   must reconcile idempotently and no cut may report an unbarriered event as
   `Accepted`.
8. **Fault gate:** inject short/partial write, `ENOSPC`/disk-full, sync failure,
   rename failure, directory-sync failure, manifest-transition persistence
   failure, and truncate/source-sync failure. Assert
   `DurabilityIndeterminate` for every visible or possibly visible mutation
   failure, plus the typed stage and preservation of every recoverable copy.
9. **Claim gate:** subprocess kill/reopen proves process-crash recovery, not
   power-loss durability. The product and tests must describe the result as a
   completed OS barrier; real power-cut testing is optional additional evidence,
   not a license to claim more than the platform contract.

Until gates 1-9 pass, canonical `Accepted` remains unavailable for the affected
operation. In particular, the Windows first-create and rename paths stay at a
weaker level even if ordinary restart tests pass.

## Rejected shortcuts

- **`flush()` or close is durable.** Rejected: `flush` drains userspace
  buffering, while Rust says drop ignores close errors and names `sync_all` for
  synchronization. [R1]
- **`sync_data()` is enough everywhere.** Rejected: its metadata guarantee is
  intentionally weaker, even though Apple and Windows currently map it to the
  stronger primitive. The protocol requires a stable cross-target contract,
  not an implementation accident. [R1] [R4] [R5]
- **File sync persists a first-created name on Linux.** Rejected explicitly by
  the Linux `fsync` manpage. [L1]
- **A successful rename is durable.** Rejected: atomic visibility and storage
  persistence are different claims; Rust's Windows rename does not request
  write-through. [R2] [R5] [L2] [W3]
- **`ReplaceFileW` supplies atomic durable replacement.** Rejected: its
  write-through flag is unsupported and documented failures can leave partial
  namespace/attribute states. [W4]
- **A lock prevents every competing access.** Rejected: Unix locks are
  advisory, Windows mapped views bypass byte-range locks, and Windows
  delete/rename sharing is a separate open-handle policy. [L5] [A4] [W2] [W5]
- **Use a cross-platform locking crate.** **[inferred]** Rejected for the
  selected Rust 1.95 cooperative policy because the standard library exposes
  the needed shared/exclusive/try lock family and its documented platform
  mappings. [R3] [R4] [R5]
- **Treat a finite kill/reopen test as power-loss proof.** Rejected: the OS page
  cache survives process death, and Apple explicitly frames even full sync as
  best effort against sudden power loss. [A5]

## Open risks and bounded unknowns

- Whether Rust 1.95 `File::sync_all` succeeds on a directory on every supported
  macOS APFS configuration is not established by the cited public contract.
  Run the directory probe; refusal is the safe result on error.
- Windows has no cited public parent-directory flush contract and no cited
  atomic-plus-durable same-volume replacement contract. A Win32 probe can find
  runtime incompatibilities but cannot by itself prove a durability guarantee.
- Virtualized CI cannot emulate sudden device power loss faithfully. It can
  validate error propagation, ordering, locks, and recovery; filesystem/hardware
  guarantees remain bounded by the named OS contract.
- Network, removable, layered, encrypted, compressed, and user-space
  filesystems may change append, flush, rename, or lock semantics. They require
  separate qualification and are out of the initial `Accepted` allowlist.

Two additional general sources would not change the recommendation. The only
decision-changing evidence is a platform/filesystem-specific contract for
Windows namespace durability or the bounded APFS directory-sync experiment.

## Primary source ledger

Every material claim above cites one or more entries below. All sources were
accessed 2026-08-14.

### Rust 1.95

- **[R1]** Rust 1.95, [`std::fs::File`](https://doc.rust-lang.org/1.95.0/std/fs/struct.File.html) — `sync_all`, `sync_data`, `set_len`, and lock contracts. **[documented; verified against 1.95 source]**
- **[R2]** Rust 1.95, [`std::fs::rename`](https://doc.rust-lang.org/1.95.0/std/fs/fn.rename.html) — same-mount requirement and platform mappings. **[documented; verified]**
- **[R3]** Rust 1.95 source, [`library/std/src/fs.rs`](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/fs.rs#L758-L1058) — synchronization and lock API implementation boundary. **[verified]**
- **[R4]** Rust 1.95 source, [Unix filesystem implementation](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/unix.rs#L1381-L1655) — Apple `F_FULLFSYNC`, other Unix `fsync`/`fdatasync`, and `flock`. **[verified]**
- **[R5]** Rust 1.95 source, [Windows filesystem implementation](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/windows.rs#L400-L520) and [rename implementation](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/windows.rs#L1311-L1378) — `FlushFileBuffers`, whole-file `LockFileEx`, handle truncation, and rename flags. **[verified]**
- **[R6]** Rust 1.95 source, [Windows `OpenOptionsExt`](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/os/windows/fs.rs#L140-L225) — access, share-mode, and custom-flag controls. **[documented; verified]**

### Microsoft Windows

- **[W1]** Microsoft, [`FlushFileBuffers`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers) — file-handle access, behavior, errors, and supported storage technologies. **[documented]**
- **[W2]** Microsoft, [`LockFileEx`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex) and [byte-range locking overview](https://learn.microsoft.com/en-us/windows/win32/fileio/locking-and-unlocking-byte-ranges-in-files) — shared/exclusive behavior, nonblocking mode, mapped-view exception, and release. **[documented]**
- **[W3]** Microsoft, [`MoveFileExW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw) — replace, copy-across-volume, write-through, and failure behavior. **[documented]**
- **[W4]** Microsoft, [`ReplaceFileW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew) — unsupported write-through flag and partial failure states. **[documented]**
- **[W5]** Microsoft, [`CreateFile`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew) — read/write/delete sharing, directory-open flag, and write-through flag. **[documented]**
- **[W6]** Microsoft, [Directory Handles](https://learn.microsoft.com/en-us/windows/win32/fileio/obtaining-a-handle-to-a-directory) — directory handle acquisition and enumerated supported functions. **[documented]**
- **[W7]** Microsoft, [`SetFileInformationByHandle`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle) and [`FILE_RENAME_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_rename_info) — handle permissions and rename information. **[documented]**
- **[W8]** Microsoft `windows-sys` 0.61.2, [`MoveFileExW` binding](https://docs.rs/windows-sys/0.61.2/windows_sys/Win32/Storage/FileSystem/fn.MoveFileExW.html) — authoritative binding surface for the bounded Windows experiment. **[verified]**

### Apple/Darwin

- **[A1]** Apple, [`fsync(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html) — host-to-drive scope, reordering/power caveat, and errors. **[documented]**
- **[A2]** Apple, [`fcntl(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html) — `F_FULLFSYNC` semantics and device/filesystem caveats. **[documented]**
- **[A3]** Apple XNU, [`rename(2)`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/man/man2/rename.2) — same-filesystem rule, destination-name continuity when replacing, and errors. **[documented; verified]**
- **[A4]** Apple XNU, [`flock(2)`](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/man/man2/flock.2) — advisory shared/exclusive locks and `EWOULDBLOCK`. **[documented; verified]**
- **[A5]** Apple, [Reducing disk writes](https://developer.apple.com/documentation/xcode/reducing-disk-writes) — `F_BARRIERFSYNC` versus `F_FULLFSYNC` and best-effort power-loss caveat. **[documented]**
- **[A6]** Apple XNU, [`F_FULLFSYNC`/`F_BARRIERFSYNC` constants](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/sys/fcntl.h#L304-L351) and [filesystem dispatch](https://github.com/apple-oss-distributions/xnu/blob/f6217f891ac0bb64f3d375211650a4c1ff8ca1ea/bsd/vfs/vfs_syscalls.c#L13449-L13485) — full sync is dispatched to the filesystem vnode implementation. **[verified]**

### Linux

- **[L1]** Linux man-pages, [`fsync(2)`](https://man7.org/linux/man-pages/man2/fsync.2.html) — device completion, metadata, parent-directory requirement, and delayed errors. **[documented]**
- **[L2]** Linux man-pages, [`rename(2)`](https://man7.org/linux/man-pages/man2/rename.2.html) — atomic replacement visibility and rename errors. **[documented]**
- **[L3]** Linux man-pages, [`open(2)`](https://man7.org/linux/man-pages/man2/open.2.html) — `O_APPEND` atomic step, NFS caveat, `O_EXCL`, and `O_SYNC`. **[documented]**
- **[L4]** Linux man-pages, [`truncate(2)`/`ftruncate(2)`](https://man7.org/linux/man-pages/man2/truncate.2.html) — handle-based length change and errors. **[documented]**
- **[L5]** Linux man-pages, [`flock(2)`](https://man7.org/linux/man-pages/man2/flock.2.html) — open-file-description locks, nonblocking error, and NFS/CIFS variance. **[documented]**
