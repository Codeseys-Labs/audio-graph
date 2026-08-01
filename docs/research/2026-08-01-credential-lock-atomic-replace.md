# Credential v2 lock and atomic-replacement primitives

Date: 2026-08-01

Seed: `audio-graph-75ae`

Related decision: [ADR-0035](../adr/0035-backend-owned-credential-service.md)

## Question and gated decision

Are exact `fs4` 1.1.0 and `atomic-write-file` 0.3.0 suitable primitives for
AudioGraph credential-v2 cross-process mutation locking and owner-only durable
atomic replacement on Windows, macOS, and Linux?

This gates whether AudioGraph adopts or rejects each crate and defines the
narrow wrapper and runtime release-gate contract for the non-secret authority
journal and explicit file-v2 backend. [documented]

Evidence labels in this note mean: **[verified]** was checked directly in the
repository or exact published source; **[documented]** is stated by an official
OS or Rust source; **[inferred]** is the engineering conclusion drawn from that
evidence. [documented]

## Recommendation

| Candidate | Decision | Confidence | Reason |
| --- | --- | --- | --- |
| `fs4` 1.1.0 | **Reject for this use** | High | AudioGraph pins Rust 1.95.0, whose inherent `std::fs::File::{try_lock,lock,unlock}` API has been stable since Rust 1.89 and uses the same `flock`/`LockFileEx` primitives. `fs4` 1.1.0 deliberately mirrors that API; for `std::fs::File`, inherent method dispatch wins, so the dependency adds no needed lock capability. [verified] |
| `atomic-write-file` 0.3.0 | **Reject as the cross-platform replacement primitive** | High | Its Unix path is close to the ADR contract, but its non-Unix path uses only `File::sync_all()` followed by generic `std::fs::rename()`, exposes no parent-directory durability operation or Windows creation security descriptor, and contains an explicit TODO for `MOVEFILE_WRITE_THROUGH`. Its commit state also hides whether an error happened before or after namespace replacement. [verified] |
| Narrow AudioGraph lock wrapper over Rust 1.95 `std::fs::File` | **Adopt, release-gated** | High | A nonblocking `try_lock` loop can enforce a monotonic deadline without leaving an uncancellable blocking lock call, while an owned `File` guard gives OS release-on-close/process-exit semantics. [inferred] |
| Narrow AudioGraph atomic-replace wrapper | **Required; do not delegate its contract to the crate name** | High | The wrapper must own permission-before-bytes, stage-aware errors, same-directory replacement, file and namespace durability, readback/reconciliation, and platform gates. Windows needs a separately proved native replace implementation; Linux/macOS may use a small explicit Unix implementation with the sequence below. [inferred] |

This reverses only the provisional filesystem-library portion of
[the 2026-07-31 credential-service library evaluation](2026-07-31-credential-service-library-evaluation.md);
it does not change ADR-0035's higher-level lock, journal, recovery, or file-v2
requirements. [inferred]

This note is the superseding decision record for those two rows. The older
multi-library evaluation was not edited in this research branch because Seed
`audio-graph-75ae` owns only this output and requires a one-document commit. A
separate docs-hygiene Seed should add a forward pointer to the older evaluation
after this decision is accepted, so a reader cannot mistake its provisional
`fs4`/`atomic-write-file` recommendations for current guidance. [verified/inferred]

## Local constraints and source provenance

- `src-tauri/rust-toolchain.toml` pins `1.95.0`, and invoking `rustc` from
  `src-tauri` resolves to `rustc 1.95.0 (59807616e 2026-04-14)`. [verified]
- Neither crate is present in this worktree's `src-tauri/Cargo.toml` or
  `src-tauri/Cargo.lock`; this research is a dependency-selection gate, not a
  review of an already integrated dependency. [verified]
- The inspected crates.io archives were exactly `fs4-1.1.0.crate`
  (`sha256 7e72ed92b67c146290f88e9c89d60ca163ea417a446f61ffd7b72df3e7f1dfd5`)
  and `atomic-write-file-0.3.0.crate`
  (`sha256 84790c55b5704b0d35130bf16a4ce22a8e70eb0ea773522557524d9a4852663d`).
  Their embedded VCS commits are respectively
  `df476ee1de2926ae4599607c325a5aa1d334501d` and
  `4ec6203e19ca9ed92812822a630d7ce4dd502727`; sampled files matched those
  commits byte-for-byte. [verified]

## Executable proof on available hosts

The probe used only fixed dummy generations and lived outside the repository;
it did not modify AudioGraph dependencies or product code. Its manifest pinned
`fs4 = "=1.1.0"` with the `sync` feature and
`atomic-write-file = "=0.3.0"` with default features disabled. The generated
lockfile checksums matched the inspected crate archives. [verified]

### Linux / ext4

On Rust 1.95.0, the exact dependency graph passed an offline, locked Cargo
check, and an executable probe ran on WSL2 Linux 6.18.33.2 with `/tmp` on local
ext4. This is supplementary primitive evidence, not the packaged-path or
power-loss release gate. [verified]

- While one process held the lock, both `fs4::FileExt::try_lock` and Rust 1.95
  `File::try_lock` reported contention. A nonblocking retry loop timed out in
  126-128 ms against a 125 ms deadline instead of hanging. [verified]
- A duplicated descriptor kept the lock after the original descriptor closed;
  the next contender acquired only after the last duplicate closed. After the
  holder was killed with `SIGKILL`, a fresh process acquired the same lock.
  [verified]
- `strace` showed `flock(LOCK_EX|LOCK_NB)`, repeated `EAGAIN` while contended,
  and success after descriptor/process release. It also showed std/fs4
  interoperability on the same file object. [verified]
- With the crate's Unix defaults and the host `0022` umask, new and
  existing-destination temporary files were `0644`. With explicit
  `mode(0600)`, `preserve_mode(false)`, and `preserve_owner(false)`, the temp was
  `0600` before dummy bytes, and both new and replacement destinations ended
  `0600`. Two simultaneous opens produced two distinct same-directory names.
  [verified]
- The syscall trace showed secure creation as
  `openat(..., O_CREAT|O_EXCL|O_CLOEXEC, 0600)`, then
  `fsync(temp) -> renameat(same directory) -> fsync(parent)`. A forced rename
  error over a directory occurred only after `fsync(temp)` and left one named
  `0600` temporary file, confirming the crate's finalized/drop failure mode.
  [verified]

### Native Windows / NTFS

The same exact manifest compiled with native Rust 1.95.0 MSVC on Windows and
the executable ran from a native NTFS temporary directory. The lock probe
observed a 125 ms timeout, last-duplicated-handle release, std/fs4 contention,
and successful acquisition after the holder was terminated. Microsoft still
allows termination release to be delayed, so this smoke result does not remove
the measured packaged-path deadline gate. [verified/documented]

The generic atomic-write probe created two distinct same-directory temps,
replaced an existing destination, and left one temp after a deliberately failed
rename. However, native ACL inspection showed the probe directory and leftover
temp inherited access rules (`AreAccessRulesProtected = false`), including a
non-owner sandbox group with modify access. This is a concrete demonstration
that default `File::create_new` cannot itself establish the required owner-only
temp; the exact rules are environment-specific, while the inheritance mechanism
is the documented Windows default. The successful replace smoke test says
nothing about namespace durability because the source omits
`MOVEFILE_WRITE_THROUGH`.
[verified/documented/inferred]

### macOS limitation

No macOS compiler target or native APFS runner was available. The macOS result
is therefore limited to exact Rust/crate source and Apple documentation; no
compile or runtime claim is made. Target-native permission, directory-sync,
process-release, and abrupt-reset tests below remain release blockers rather
than paperwork to waive. [verified/inferred]

## `fs4` 1.1.0 assessment

### It is redundant with pinned Rust 1.95

- Rust stabilized `File::lock`, `lock_shared`, `try_lock`,
  `try_lock_shared`, `unlock`, and `TryLockError` in 1.89. The API explicitly
  separates `WouldBlock` from real I/O failure and says a lock is released
  when the file and every duplicated/inherited handle are closed or on
  `unlock`. [documented: [Rust 1.95 `std::fs::File`
  source](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/fs.rs#L815-L1084)]
- `fs4` 1.1.0's own changelog says it renamed its methods and error shape to
  match stable `std`, and says the inherent `std::fs::File` methods win method
  dispatch on recent Rust. [verified: [`fs4` changelog at the published
  commit](https://github.com/al8n/fs4/blob/df476ee1de2926ae4599607c325a5aa1d334501d/CHANGELOG.md#L83-L116)]
- On Unix, `fs4` calls `rustix::fs::flock` with shared/exclusive/nonblocking/
  unlock flags. Rust 1.95 calls libc `flock` with the corresponding flags on
  Linux and Apple targets. [verified: [`fs4` Unix
  implementation](https://github.com/al8n/fs4/blob/df476ee1de2926ae4599607c325a5aa1d334501d/src/unix.rs#L6-L58),
  [Rust 1.95 Unix
  implementation](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/unix.rs#L1434-L1685)]
- On Windows, both use `LockFileEx` over the range starting at zero with both
  length halves set to `u32::MAX`, and both map
  `ERROR_LOCK_VIOLATION` to `WouldBlock`. Rust's blocking path additionally
  handles an overlapped `ERROR_IO_PENDING` completion and its explicit unlock
  path accounts for layered shared/exclusive locks; the standard implementation
  is not a weaker substitute for the one-lock AudioGraph use case. [verified:
  [`fs4` Windows
  implementation](https://github.com/al8n/fs4/blob/df476ee1de2926ae4599607c325a5aa1d334501d/src/windows.rs#L13-L86),
  [Rust 1.95 Windows
  implementation](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/windows.rs#L409-L515)]

### The wrapper contract, not the crate, supplies deadline semantics

- Neither `fs4::lock` nor `std::fs::File::lock` accepts a deadline. A blocking
  call can wait until success or error, so AudioGraph must not use it for the
  bounded mutation-acquisition path. [verified]
- The wrapper should open `mutation.lock` read/write without truncation, call
  `File::try_lock()` in a monotonic-deadline loop, retry only
  `TryLockError::WouldBlock` with bounded backoff, surface other errors, and
  return a typed `operation_in_progress`/timeout at the deadline. [inferred]
- The guard must exclusively own one `File`, must never clone or inherit the
  handle, must be non-reentrant, and should release by dropping the file after
  the mutation critical section. Recursive acquisition or lock upgrades are
  forbidden because Rust documents same-handle relocking as platform-dependent
  and potentially deadlocking. [documented/inferred]
- `mutation.lock` is a permanent inode/file object: cooperating code must never
  unlink, rename, or atomically replace it. Replacing a locked pathname can let
  later openers lock a different object. [inferred]
- On Unix, create the lock file with mode `0600`, harden and validate an existing
  file as owner-only, and keep the enclosing directory owner-only. On Windows,
  apply and validate the approved user-only DACL and open read/write while
  excluding `FILE_SHARE_DELETE`; Microsoft documents that delete sharing also
  permits rename. [inferred/documented: [Microsoft `CreateFileW` sharing
  modes](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)]

### What the OS guarantees, and does not

- Linux `flock` locks are associated with the open file description and are
  released by `LOCK_UN` or when every reference is closed. They are advisory on
  ordinary local filesystems; NFS and SMB emulation can change interoperability
  and even mandatory-I/O behavior. [documented: [Linux
  `flock(2)`](https://man7.org/linux/man-pages/man2/flock.2.html)]
- Apple's `flock` is also explicitly advisory, uses `LOCK_NB`/
  `EWOULDBLOCK` for nonblocking acquisition, and duplicated/forked descriptors
  refer to one lock. [documented: [Apple
  `flock(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/flock.2.html)]
- Windows `LockFileEx` releases outstanding locks when the process terminates or
  the file closes, but Microsoft warns that release after termination may be
  delayed depending on system resources. It also does not prevent access through
  mapped file views. [documented: [Microsoft
  `LockFileEx`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex)]
- Therefore the lock is a cooperating-AudioGraph-v2 serialization mechanism,
  not protection against old binaries, same-user hostile processes, mapped
  views, or native credential-store editors. Remote/network app-config volumes
  must be unsupported unless separately proved. [inferred]

## `atomic-write-file` 0.3.0 assessment

### Common commit and error semantics

- `AtomicWriteFile::_commit` sets `finalized = true` **before** calling
  `sync_all()` and then `rename_file()`. `Drop::_discard` returns immediately
  once `finalized` is true, and ordinary `Drop` ignores discard errors.
  Consequently a sync or rename error can leave a temporary file, and the
  consumed value cannot retry cleanup. [verified: [`AtomicWriteFile` commit and
  drop](https://github.com/andreacorbellini/rust-atomic-write-file/blob/4ec6203e19ca9ed92812822a630d7ce4dd502727/src/lib.rs#L578-L665)]
- On Unix, `rename_file()` performs `renameat` and then `fsync` on the already
  opened parent directory. Reaching the following directory `fsync` proves the
  destination has already become the new file, so an error there leaves only
  its crash durability uncertain. More conservatively, once `renameat` has
  been invoked, AudioGraph cannot classify an error as definitely
  pre-replacement across every supported filesystem; Linux explicitly calls
  out surprising failure-after-success behavior for NFS. The public
  `commit()` error does not reveal which stage failed. [verified/inferred:
  [`atomic-write-file`
  Unix implementation](https://github.com/andreacorbellini/rust-atomic-write-file/blob/4ec6203e19ca9ed92812822a630d7ce4dd502727/src/imp/unix/mod.rs#L153-L190),
  [Linux `rename(2)`](https://man7.org/linux/man-pages/man2/rename.2.html),
  [Apple `rename(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/rename.2.html)]
- A wrapper that used this crate would therefore have to treat **every**
  `commit()` error as `commit_unknown`, inspect and parse the final path under
  the mutation lock, compare its operation id/revision, and securely sweep
  bounded same-prefix leftovers. Treating `Err` as “old file definitely
  remains” is unsafe. [inferred]
- `AtomicWriteFile` exposes its inner `File` and permits cloning it; a clone can
  continue writing after commit and those writes are no longer atomic. A narrow
  credential wrapper must not expose either the crate object or a cloned file.
  [verified/inferred]
- Both implementations name temps `.<destination>.<suffix>` with exactly six
  random ASCII-alphanumeric characters. They retry indefinitely after an
  `EEXIST`/`AlreadyExists` collision, while `O_EXCL`/`create_new(true)` prevents
  overwriting the colliding path. Exclusive creation makes a collision safe,
  but the unbounded retry is an avoidable availability ambiguity; the
  AudioGraph wrapper should use a longer random component, a bounded retry
  count, and a typed fail-closed exhaustion error. [verified/inferred:
  [Unix name/open loop](https://github.com/andreacorbellini/rust-atomic-write-file/blob/4ec6203e19ca9ed92812822a630d7ce4dd502727/src/imp/unix/mod.rs#L120-L176),
  [generic name/open loop](https://github.com/andreacorbellini/rust-atomic-write-file/blob/4ec6203e19ca9ed92812822a630d7ce4dd502727/src/imp/generic.rs#L45-L64)]

### Linux and macOS: close, but only under a strict Unix wrapper

- Unix temporary files are created in the destination directory with
  `openat(O_CREAT | O_EXCL | O_CLOEXEC, mode)`, then committed by same-directory
  `renameat`; keeping the directory descriptor makes the target directory
  stable across rename/remount races. [verified: [`atomic-write-file` Unix
  source](https://github.com/andreacorbellini/rust-atomic-write-file/blob/4ec6203e19ca9ed92812822a630d7ce4dd502727/src/imp/unix/mod.rs#L88-L190)]
- The Unix defaults are **not secure defaults for credential bytes**:
  `mode = 0666`, `preserve_mode = true`, and best-effort owner preservation.
  Replacing a legacy `0644` file therefore preserves its permissive mode, while
  a new file depends on the process umask. [verified: [`atomic-write-file`
  Unix defaults](https://github.com/andreacorbellini/rust-atomic-write-file/blob/4ec6203e19ca9ed92812822a630d7ce4dd502727/src/imp/unix/mod.rs#L60-L80)]
- A minimally safe Unix call would have to set standard Unix
  `OpenOptionsExt::mode(0o600)`, crate
  `OpenOptionsExt::preserve_mode(false)`, and
  `OpenOptionsExt::preserve_owner(false)`, then validate current UID, exact
  `0600` effective mode, and absence of access-granting ACL entries on the
  temporary file **before writing the first byte**. Passing `0600` is not a
  substitute for the post-open validation on ACL-capable or unusual
  filesystems. [inferred]
- Linux documents that a new file's effective mode is derived from the supplied
  mode and umask and that `O_CREAT|O_EXCL` refuses an existing pathname and does
  not follow a final symlink. [documented: [Linux
  `open(2)`](https://man7.org/linux/man-pages/man2/open.2.html)]
- `commit()` invokes `std::fs::File::sync_all()` on the temporary file before
  rename. On Linux Rust 1.95 maps that to `fsync`; Linux requires a separate
  directory `fsync` to make the directory entry durable, which the crate's Unix
  path performs. [verified/documented: [Rust 1.95 Unix
  `sync_all`](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/unix.rs#L1381-L1392),
  [Linux `fsync(2)`](https://man7.org/linux/man-pages/man2/fsync.2.html)]
- On Apple targets, Rust 1.95 maps **both** `File::sync_all()` and
  `File::sync_data()` to `fcntl(F_FULLFSYNC)`, so the crate's temporary-file
  sync is stronger than a plain macOS `fsync`. Apple says `F_FULLFSYNC` performs
  `fsync` and also asks the drive to flush its buffered data. [verified/documented:
  [Rust 1.95 Apple sync
  implementation](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/unix.rs#L1381-L1402),
  [Apple `fcntl(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html)]
- The crate's parent-directory sync is a direct `nix::unistd::fsync`, not Rust
  `File::sync_all`, so on macOS it is ordinary `fsync`, **not**
  `F_FULLFSYNC`. Apple warns that ordinary `fsync` may leave drive-cache and
  ordering exposure, while `F_FULLFSYNC` is the stronger request; whether a
  directory `fsync` succeeds and supplies the required APFS namespace barrier
  must be an explicit target-native runtime gate. [verified/documented/inferred:
  [crate Unix source](https://github.com/andreacorbellini/rust-atomic-write-file/blob/4ec6203e19ca9ed92812822a630d7ce4dd502727/src/imp/unix/mod.rs#L183-L190),
  [Apple `fsync(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html)]
- These Unix strengths do not rescue the crate as the shared cross-platform
  primitive, and its opaque commit staging is avoidable in a small explicit
  Unix wrapper. [inferred]

### Windows: fails the source-level contract

- All non-Unix targets use the generic implementation. It creates a named
  same-directory temporary file with `std::fs::File::options().create_new(true)`
  and commits with `std::fs::rename`; `directory()` always returns `None`.
  [verified: [`atomic-write-file` generic
  implementation](https://github.com/andreacorbellini/rust-atomic-write-file/blob/4ec6203e19ca9ed92812822a630d7ce4dd502727/src/imp/generic.rs#L35-L89)]
- The source itself has a Windows TODO to use `CreateFileW` with hidden and
  delete-on-close flags and `MoveFileEx` with
  `MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH`. Version 0.3.0 does not
  implement that TODO. [verified: [`atomic-write-file` platform selection and
  TODO](https://github.com/andreacorbellini/rust-atomic-write-file/blob/4ec6203e19ca9ed92812822a630d7ce4dd502727/src/imp/mod.rs#L1-L14)]
- Rust 1.95's Windows `File::sync_all` calls `FlushFileBuffers`, so temporary
  **file content** is flushed before rename. [verified/documented: [Rust 1.95
  Windows `fsync`](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/windows.rs#L400-L407),
  [Microsoft `FlushFileBuffers`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers)]
- Rust 1.95's `std::fs::rename` first calls `MoveFileExW` with only
  `MOVEFILE_REPLACE_EXISTING`, and on a specific access-denied path may fall
  back to `SetFileInformationByHandle(FileRenameInfoEx)`. Neither path requests
  `MOVEFILE_WRITE_THROUGH`, and the crate provides no later parent/volume flush.
  [verified: [Rust 1.95 Windows
  rename](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/windows.rs#L1311-L1379)]
- Microsoft documents `MOVEFILE_WRITE_THROUGH` as the flag that prevents
  `MoveFileExW` returning until the move is performed on disk. Because the crate
  omits it and exposes no namespace flush, successful `commit()` does not supply
  primary-source proof of the ADR's durable-replacement contract on Windows.
  [documented/inferred: [Microsoft
  `MoveFileExW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw)]
- The generic `OpenOptions` exposes no Windows security-descriptor control and
  the crate documents no non-Unix permission/ownership preservation. Windows
  assigns a default descriptor on creation whose ACL is inherited from the
  parent directory; therefore a permissive parent can expose the temporary file
  before any wrapper writes bytes. [verified/documented: [Microsoft file
  security](https://learn.microsoft.com/en-us/windows/win32/fileio/file-security-and-access-rights),
  [Microsoft `CreateFileW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)]
- A wrapper could require and validate an owner-only directory before opening,
  but 0.3.0 still cannot express an explicit creation descriptor or the missing
  durable rename. This is a source-level rejection; a process-kill smoke test
  cannot cure it. [inferred]

### Upstream tests do not close the release-evidence gap

- The published source contains ordinary Linux, macOS, and Windows CI, but its
  abrupt-crash workflow is Linux-only (btrfs, ext3, ext4, and XFS). There is no
  corresponding Windows or macOS crash/power-loss matrix in the published 0.3.0
  source. [verified: [cross-platform CI](https://github.com/andreacorbellini/rust-atomic-write-file/blob/4ec6203e19ca9ed92812822a630d7ce4dd502727/.github/workflows/ci.yml),
  [Linux-only crash workflow](https://github.com/andreacorbellini/rust-atomic-write-file/blob/4ec6203e19ca9ed92812822a630d7ce4dd502727/.github/workflows/crash-tests.yml)]
- Upstream functional tests show useful prior coverage, but they do not prove
  AudioGraph's owner-only-before-bytes, stage-specific errors, packaged app
  path, or three-platform crash-durability contract. [inferred]

## Required narrow wrapper contract

### Lock wrapper

The accepted wrapper contract is all of the following; omitting any item fails
the release gate. [inferred]

1. Resolve one stable app-config `credential-v2/mutation.lock`; create its
   owner-only parent and file without truncating or replacing the file.
   [inferred]
2. Open read/write. On Windows, use an explicit owner-only DACL and share read
   and write but not delete; on Unix, request/harden `0600` and validate owner,
   mode, and ACL. [inferred]
3. Acquire only with `std::fs::File::try_lock()` plus a monotonic deadline.
   Retry only `WouldBlock`; classify every other error distinctly. [inferred]
4. Return a non-cloneable RAII guard owning the sole file handle. Do not pass it
   to child processes, upgrade locks, or recursively acquire it. [inferred]
5. Hold it across journal load, expected-revision check, intent persistence,
   native mutation/readback, journal commit, and reconciliation decision; drop
   it before publishing the event. [documented]
6. Never promise exclusion against non-cooperators, and reject unproved remote
   filesystems. [inferred]

### Atomic replacement wrapper

The accepted wrapper must expose explicit stages rather than a single opaque
`commit` result. [inferred]

1. Under the mutation lock, open the already owner-only parent directory and
   create a unique same-directory temporary file with exclusive creation and
   close-on-exec/non-inheritable semantics. [inferred]
2. Establish and verify owner-only access **before the first content byte**.
   Unix means process-owned, effective mode exactly `0600`, and no granting ACL;
   Windows means an explicit approved DACL supplied at create time and verified
   on the handle. Failure closes/removes the empty temp and returns
   `permission_hardening_failure`. [inferred]
3. Write the complete encoded bytes; flush the temporary file (`fsync` on
   Linux, Rust 1.95 `F_FULLFSYNC` on macOS, `FlushFileBuffers` on Windows).
   A failure here is definitely pre-replace and must not rename. [inferred]
4. Replace the destination atomically within that directory. Linux/macOS may
   use `renameat`; Windows must use a separately selected and proved native
   replacement path with a write-through/durability primitive rather than
   version 0.3.0's generic `std::fs::rename`. [inferred]
5. Sync the namespace: parent-directory `fsync` on Linux/macOS, with APFS
   success gated at runtime; use the selected Windows primitive's documented
   namespace durability operation. [inferred]
6. Return `not_committed` only for a proved pre-replace failure. Once replacement
   may have happened, any error is `commit_unknown`; while still holding the
   lock, reopen and validate the final envelope's operation id/revision before
   deciding recovery. [inferred]
7. On success, reopen and parse/readback the final file and verify expected
   operation id/revision and owner-only metadata before journal/event success.
   [documented/inferred]
8. Sweep only bounded, wrapper-named leftovers while holding the lock; verify
   ownership/type/no-link policy before deletion. Never log file-v2 content or
   private paths. [inferred]

## Exact runtime release gates

These gates use dummy, non-secret bytes and must run natively on the packaged
application's actual app-config filesystem. A test in a generic temporary
directory is supplementary, because filesystem and ACL behavior are part of
the contract. [inferred]

### Gates common to Windows, macOS, and Linux

1. **Contention and deadline:** process A acquires the stable lock and reports
   ready; process B observes only `WouldBlock`, reaches the configured deadline
   without hanging a worker, returns the typed timeout, and performs no journal
   or native mutation. [inferred]
2. **Normal and forced release:** after ordinary exit without explicit unlock,
   and separately after forced termination, B eventually acquires the same lock
   within the configured deadline. Use `TerminateProcess` on Windows and
   `SIGKILL` on macOS/Linux. Record release latency; Windows may not assume
   immediate release. [inferred]
3. **Stable object:** compare file identity before/after contention and prove no
   cooperating path unlinks, truncates, or replaces `mutation.lock`. [inferred]
4. **Advisory-limit proof:** a deliberately lock-ignoring helper can still alter
   dummy state where the OS permits it; the test documents rather than widens
   the guarantee. [inferred]
5. **Permission before bytes:** pause immediately after temp creation and prove
   the temp is owner-only before allowing a first write. Start with an absent
   destination and with a deliberately permissive existing destination; both
   final files must be owner-only. [inferred]
6. **Atomic visibility:** with concurrent readers and repeated old/new payloads,
   every read is exactly one complete valid generation; no missing, empty,
   prefix, mixed, or malformed generation is observable. [inferred]
7. **Interruption matrix:** terminate at temp-created, partially-written,
   file-synced, replace-started, replace-returned, namespace-sync-started, and
   namespace-sync-returned barriers. Restart reconciliation must see old or new
   complete bytes, never partial bytes, and must classify uncertainty instead
   of publishing success. [inferred]
8. **Error matrix:** inject create, hardening, write, file-sync, replace,
   namespace-sync, readback, and cleanup failures. Pre-replace errors leave the
   old destination authoritative; post/possibly-post-replace errors become
   `commit_unknown`. [inferred]
9. **Leftovers:** every forced-exit residue is owner-only, bounded by the wrapper
   naming scheme, ignored by readers, and safely removable during locked
   recovery. [inferred]
10. **Actual durability:** forced process exit proves process-crash recovery but
    not power-loss persistence. A durable release claim additionally requires
    an abrupt VM/power-cut harness on each supported filesystem, or the product
    must label that platform/filesystem unsupported for file-v2. [inferred]

### Linux gate

- Run the matrix on every supported local filesystem family (at minimum the CI
  filesystem plus the release image's expected ext4/btrfs/XFS family); record
  `statfs` identity and reject NFS/CIFS/other unproved remote mounts. [inferred]
- Run with `umask 000` and an existing `0644` destination; temp and final must
  remain current-UID `0600` with no effective granting ACL for group, other, or
  named non-owner principals. [inferred]
- Prove `fsync(temp) -> renameat -> fsync(parent)` all return success and inject
  each failure separately. [inferred]
- Keep Linux anonymous `O_TMPFILE` support out of the initial contract; named,
  exclusively created owner-only temps give deterministic cleanup evidence and
  avoid making `/proc` support part of credential persistence. [inferred]

### macOS gate

- Run on the minimum supported macOS release on APFS from a packaged, signed
  application path; include lock contention and `SIGKILL` release. [inferred]
- Verify UID, mode `0600`, and ACL before bytes and after replacement, including
  an existing permissive file and a parent with an unexpected inherited ACL.
  The latter must fail closed rather than create a readable temp. [inferred]
- Prove the temporary-file `F_FULLFSYNC` succeeds and the parent-directory
  ordinary `fsync` succeeds on the actual app-config directory. A directory
  `EINVAL`/`ENOTSUP` is a release blocker, not a warning after success.
  [inferred]
- Run the interruption matrix and an abrupt virtual-machine/power-loss APFS
  harness. Apple notes that even `F_FULLFSYNC` is a request to the storage stack,
  so the claim remains bounded to tested hardware/filesystem behavior.
  [documented/inferred]

### Windows gate

- Run on the minimum supported Windows release on local NTFS; add ReFS only if
  it is explicitly claimed. Reject UNC/SMB and other unproved locations.
  [inferred]
- Verify the parent, temp, final journal, final file-v2 file, and lock DACLs
  before bytes/after replacement. The temp must be created with the explicit
  descriptor; inheritance from a permissive parent is a hard failure.
  [inferred]
- Prove two-process contention, deadline behavior, normal exit, and
  `TerminateProcess` release while measuring the documented delayed-release
  window. [inferred]
- For the future native replace wrapper, prove
  `FlushFileBuffers(temp) -> native replace with the selected write-through
  semantics -> readback`, concurrent old/new visibility, sharing-violation
  behavior when the destination is held without delete sharing, and every
  interruption/error barrier. [inferred]
- Run abrupt VM reset tests on NTFS. `atomic-write-file` 0.3.0 is barred from
  satisfying this gate because its source omits the required Windows security
  creation and namespace-durability primitives. [inferred]

## Adversarial pass and failure modes

- A lock-ignoring process can bypass Unix advisory locking, and a same-user
  process can attempt to unlink/replace the lock object. The contract is only
  for cooperating v2 processes; journal/native disagreement remains
  `commit_unknown` or `recovery_required`. [documented/inferred]
- Network filesystems alter `flock` and rename/durability semantics. “Works on
  my local CI runner” cannot be generalized to NFS, SMB, cloud-synced, FUSE, or
  removable app-config locations. [documented/inferred]
- A killed Windows holder may not release immediately; a single immediate retry
  is flaky and unsafe. The contender must keep its deadline loop and never
  assume a stale lock can be deleted. [documented/inferred]
- `atomic-write-file`'s `finalized` ordering can leak a named temp even on a
  handled sync error, not only on abrupt process death. File-v2 cleanup is a
  security concern because the residue contains secret bytes. [verified/inferred]
- Its six-character temp namespace retries collisions without a bound. Normal
  random collision is improbable in the 62^6 namespace, but a fault-injection
  or adversarial filesystem that keeps returning `EEXIST` can make the open
  loop unbounded. [verified/inferred]
- Preserving the old Unix mode is actively dangerous when migrating an insecure
  `0644` legacy file. Explicit `preserve_mode(false)` is necessary if the crate
  is ever prototyped, and the final wrapper should create fresh secure metadata
  instead of inheriting it. [verified/inferred]
- Windows inherited ACLs and Unix/macOS ACLs can defeat a simplistic
  “requested 0600/default DACL” check. Permission verification must happen on
  the actual created object before content. [documented/inferred]
- A `commit()` error after Unix rename but during directory sync means the new
  bytes may already be visible. Returning ordinary failure and retrying blindly
  could overwrite or misreport a committed revision. [verified/inferred]
- Process-kill tests exercise atomic visibility and recovery but do not emulate
  loss of volatile controller/drive caches. Power-loss durability needs an
  abrupt reset harness. [inferred]
- Antivirus, indexers, backup tools, and other open handles can cause Windows
  sharing violations around replacement. The wrapper must preserve typed
  failure/uncertainty and never fall back to in-place writes. [inferred]

## Rejected alternatives

- **`fs4` for `std::fs::File`:** rejected because Rust 1.95 provides the same
  required API and platform primitives, while `fs4`'s own changelog confirms
  inherent std methods win. [verified/inferred]
- **Blocking `lock()`:** rejected for the bounded acquisition path because it
  has no deadline/cancellation contract. [inferred]
- **Create/delete sentinel lockfiles:** rejected because a crash leaves stale
  state and deleting/recreating the path can split contenders across objects.
  [inferred]
- **`atomic-write-file` 0.3.0 as one three-platform implementation:** rejected
  for missing Windows creation-security and durable-namespace controls plus
  opaque commit staging. [verified/inferred]
- **Preserve existing Unix permissions/owner:** rejected because an insecure
  legacy mode would be copied onto the new inode. [verified/inferred]
- **Rely on umask or inherited Windows ACL:** rejected because ambient process
  and parent-directory policy is not the credential service's fail-closed
  contract. [documented/inferred]
- **Treat any commit error as definitely uncommitted:** rejected because Unix
  parent `fsync` occurs after rename. [verified]
- **In-place truncate/write or a rename without file and namespace sync:**
  rejected because it cannot meet atomic visibility and crash-durability
  together. [documented/inferred]

## Open risks and cheapest decisive experiments

1. **Windows primitive remains undecided.** The cheapest decisive experiment is
   a tiny target-native harness comparing the chosen explicit
   `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` or alternative native replace
   sequence under concurrent readers, DACL inspection, sharing violations,
   stage injection, and abrupt VM reset on NTFS. Do not guess from
   `std::fs::rename`. [inferred]
2. **macOS directory durability is not established by source alone.** The
   cheapest decisive experiment is a minimal APFS harness that performs
   `F_FULLFSYNC(temp)`, `renameat`, and `fsync(parent)` with barriers, then
   repeatedly hard-resets a disposable VM and verifies old-or-new complete state.
   [inferred]
3. **Supported filesystem scope is undefined.** Detect and record the app-config
   volume/filesystem in the conformance test; start with local NTFS/APFS and the
   explicitly tested Linux filesystems, failing closed elsewhere. [inferred]
4. **ACL validation policy needs exact platform fixtures.** The harness must
   include inherited/named ACL cases and encode which system principals, if any,
   are allowed in addition to the logged-in user. [inferred]

## Out-of-scope Seed proposals

These are proposals only and were not filed or investigated beyond what was
needed for this decision. [verified]

- Child of `audio-graph-fb2b`: **Select and prove the Windows credential-v2
  owner-only durable replace primitive**; acceptance includes explicit create
  DACL, write-through namespace replacement, stage-aware errors, NTFS abrupt-VM
  evidence, and packaged-path tests. [inferred]
- Child of `audio-graph-fb2b`: **Define credential file-backend supported
  filesystem policy and detection**; acceptance names supported local
  filesystems and fail-closed behavior for remote/FUSE/cloud-synced paths.
  [inferred]
- Child of `audio-graph-fb2b`: **Prove APFS parent-directory durability for the
  credential journal/file backend**; acceptance includes target-native
  `F_FULLFSYNC`/directory-`fsync` stage evidence and abrupt-VM recovery.
  [inferred]
- Docs hygiene: **Mark the 2026-07-31 filesystem-library recommendations as
  superseded by this decision**; acceptance adds a forward link without
  changing the earlier evaluation's historical evidence. This proposal is
  separate because `audio-graph-75ae` requires a one-document commit and this
  assignment forbids edits to Seeds or the older note. [verified/inferred]

## Primary sources

- Exact [`fs4` 1.1.0 source archive](https://crates.io/api/v1/crates/fs4/1.1.0/download)
  and [published VCS commit](https://github.com/al8n/fs4/tree/df476ee1de2926ae4599607c325a5aa1d334501d).
  [verified]
- Exact [`atomic-write-file` 0.3.0 source
  archive](https://crates.io/api/v1/crates/atomic-write-file/0.3.0/download)
  and [published VCS commit](https://github.com/andreacorbellini/rust-atomic-write-file/tree/4ec6203e19ca9ed92812822a630d7ce4dd502727).
  [verified]
- [Rust 1.95 `std::fs` public API](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/fs.rs)
  and exact [Unix](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/unix.rs) /
  [Windows](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/windows.rs)
  implementations. [verified]
- Linux man-pages project:
  [`flock(2)`](https://man7.org/linux/man-pages/man2/flock.2.html),
  [`open(2)`](https://man7.org/linux/man-pages/man2/open.2.html),
  [`rename(2)`](https://man7.org/linux/man-pages/man2/rename.2.html), and
  [`fsync(2)`](https://man7.org/linux/man-pages/man2/fsync.2.html).
  [documented]
- Apple system documentation:
  [`flock(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/flock.2.html),
  [`rename(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/rename.2.html),
  [`fsync(2)`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html), and
  [`fcntl(2)` / `F_FULLFSYNC`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html).
  [documented]
- Microsoft system documentation:
  [`LockFileEx`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex),
  [`CreateFileW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
  [`FlushFileBuffers`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers),
  [`MoveFileExW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw),
  and [file security and access
  rights](https://learn.microsoft.com/en-us/windows/win32/fileio/file-security-and-access-rights).
  [documented]
