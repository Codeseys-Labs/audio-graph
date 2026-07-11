# Canonical log durability and OS-contract review

- Date: 2026-07-10
- Seed: `audio-graph-90f3`
- Candidate: `E:/CS/github/audio-graph-canonical-log/src-tauri/src/persistence/canonical_log.rs`
- Scope: file-opening and append semantics, cooperative locks, synchronization barriers, tail quarantine/truncation, uncertainty, EOF classification, reader/writer coordination, and crash/power-loss claims on Windows, macOS, and Linux.
- Review mode: read-only code audit plus primary platform/runtime documentation; no runtime integration is assumed.

## Executive verdict

**Verdict: bounded research kernel only.**

The candidate is a thoughtful, fail-closed kernel for framing, hash-chain validation, idempotency, single-appender coordination, short-write poisoning, and explicit recovery. Its normal append path correctly keeps an event pending until one complete frame write, flush, and `File::sync_all` succeed. It also explicitly says that parent-directory durability, quarantine lifecycle registration, runtime reader/writer coordination, and legacy-writer retirement are outside this module.

Those exclusions are load-bearing, not polish. A newly created stream can return an `Accepted` file-sync receipt without a durable directory entry; quarantine creation is not durably registered before destructive truncation; path-based recovery can read one file and truncate a replacement; readers do not participate in the lock protocol; and retry recovery checks the base length but not the expected base head or whether the tail belongs to the pending frame. The kernel must therefore remain non-runtime and non-authoritative until the P0 gaps below are closed and crash-tested.

## What the kernel gets right

1. **A successful new append is not committed in memory before the file barrier.** `attempt_pending` performs one frame write, treats a short write or write error as uncertain, then requires `flush` and `sync_all` before `commit_pending` advances the cached head (`canonical_log.rs:917-969`).
2. **The uncertain-event fence is explicit.** Once an append may have changed the file, the appender stores the pending event and rejects a different event until the identical commitment is reconciled (`canonical_log.rs:971-984`, `986-1137`).
3. **The commit marker is structural and fail-closed.** A framed record is accepted only when its final newline exists; a newline-terminated malformed record is never silently repaired (`canonical_log.rs:1276-1370`). This sharply bounds automatic recovery to an unterminated final suffix.
4. **Tail bytes are copied and file-synced before source truncation.** The quarantine helper uses `create_new`, `write_all`, `flush`, and `sync_all`; only then does the caller truncate and sync the source (`canonical_log.rs:394-435`, `373-390`, `692-703`, `1039-1068`). This is the right intra-file ordering, subject to the missing directory/manifest barriers below.
5. **Opening an existing stream validates structure and the requested payload schema before mutation.** Both public load recovery and the locked appender validate the retained prefix as `T` before truncation (`canonical_log.rs:478-504`, `672-715`).
6. **The code does not overstate the implemented boundary.** The module header explicitly limits locks to cooperating writers and names parent-directory durability and quarantine registration as blockers (`canonical_log.rs:14-23`); the durability enum also says a file-sync receipt is not a parent-directory guarantee (`canonical_log.rs:218-227`).

## Cross-platform contract actually provided

| Platform | File barrier reached by Rust `File::sync_all` | Lock behavior relevant here | Consequence for this candidate |
|---|---|---|---|
| Windows | Rust documents `sync_all` as an attempt to synchronize OS-internal file content and metadata; Rust 1.95 implements it with `FlushFileBuffers`. | Rust 1.95 uses `LockFileEx` for `try_lock`. Microsoft says an exclusive locked region denies other ordinary handles both read and write, including a second handle in the same process; mapped views are not blocked. | The writer lock is stronger than advisory I/O for ordinary handles, but the separate unlocked reader can fail while the appender is live. Neither the Rust nor Microsoft file-flush contract establishes durability of the *parent directory name*. |
| macOS | Rust 1.95's Unix implementation uses `fcntl(F_FULLFSYNC)` for `sync_all`, which is stronger than plain `fsync` and asks the drive to flush its cache. | Rust uses `flock`; the standard library expressly says lock interaction with non-lockholder reads/writes is platform-specific. | The file barrier is appropriate for the file, but external/legacy writers must still be retired and directory-entry/manifest ordering is still absent. |
| Linux | Rust uses `fsync`. Linux documents that this flushes file data and file metadata, but explicitly requires a separate `fsync` of the containing directory to persist the directory entry. | Rust uses advisory `flock`; a writer that ignores it can still modify the file. | File contents may be synced while the new stream or quarantine filename remains non-durable. The `seek(End) -> write` pair is not protected from a noncooperating writer by `O_APPEND`, because the file is opened read/write rather than append. |

Primary references:

- [Rust `File`: `sync_all`, locks, and platform-specific lock behavior](https://doc.rust-lang.org/std/fs/struct.File.html)
- [Rust 1.95 Unix filesystem implementation (`fsync`, including Apple `F_FULLFSYNC`)](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/unix.rs)
- [Rust 1.95 Windows filesystem implementation (`FlushFileBuffers`, `LockFileEx`)](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/windows.rs)
- [Microsoft `FlushFileBuffers`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers)
- [Microsoft `LockFileEx`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-lockfileex)
- [Microsoft `WriteFile` synchronization and file-position rules](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-writefile)
- [Apple `fsync(2)` and the `F_FULLFSYNC` distinction](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fsync.2.html)
- [Apple `fcntl(2)` definition of `F_FULLFSYNC`](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man2/fcntl.2.html)
- [Linux `fsync(2)`, including the separate directory-sync requirement](https://www.man7.org/linux/man-pages/man2/fsync.2.html)
- [POSIX `write`, including the atomic end-position step provided by `O_APPEND`](https://pubs.opengroup.org/onlinepubs/9699919799/functions/write.html)

## Gaps, priorities, and acceptance tests

### DUR-P0-1 — New stream and quarantine directory entries are not durable

- **Priority:** P0 before any runtime path may treat `Accepted` as authoritative.
- **Evidence:** `open_locked_appender` may create parent directories and the log, but only the file handle is later synced (`canonical_log.rs:557-575`, `717-727`). `create_quarantine_file` syncs the new quarantine file but never its parent directory (`canonical_log.rs:394-435`). The source file is then truncated and synced (`canonical_log.rs:373-390`, `692-703`, `1055-1068`). The enum caveat at `canonical_log.rs:218-227` accurately admits this limitation.
- **Impact:** On Linux in particular, a successful file `fsync` does not necessarily persist the containing directory entry. After power loss, an `Accepted` event's newly created log can disappear, or a source truncation can survive while the quarantine filename does not. The latter defeats the stated evidence-preservation intent.
- **Seed proposal:** Under `audio-graph-90f3`, add **P0: cross-platform directory-entry barrier and receipt semantics**. The implementation must distinguish created-vs-existing log state, establish an explicit Windows/macOS/Linux directory-entry durability strategy, and withhold `Accepted`/repair success until every required barrier succeeds. If a platform cannot provide the claimed barrier, narrow the product claim and fail closed rather than silently promoting the event.
- **Required tests:**
  1. Extend the file-ops abstraction with parent-directory sync failpoints. A newly created stream must not return `Accepted` when the directory barrier fails, even when the file `sync_all` succeeds.
  2. A repair must not truncate the source until quarantine-file sync, quarantine directory-entry sync, and durable registration all succeed.
  3. Run a process-crash matrix at each barrier on Windows/NTFS, macOS/APFS, and Linux/ext4 (plus any other supported filesystem). Add a VM power-cut test for the Linux reference path; process kill alone does not test cache-loss durability.

### DUR-P0-2 — Poisoned recovery proves length, not ownership of the base or tail

- **Priority:** P0.
- **Evidence:** `PendingAppend` retains `base_byte_len` but no expected base head/identity (`canonical_log.rs:603-611`). The normal preflight checks only current length (`canonical_log.rs:917-930`). During uncertain recovery, an unterminated tail is accepted for repair when `valid_up_to == base_byte_len`; the retained prefix is schema-validated but never compared with the appender's cached head, and the tail is never checked against the attempted frame (`canonical_log.rs:1008-1069`). In the no-tail branch, `None if cache.byte_len == pending.base_byte_len` retries the stale pending frame without verifying the recovered head (`canonical_log.rs:1079-1136`).
- **Impact:** A same-length replacement of the valid prefix, or arbitrary bytes appended by a noncooperating writer, can be mistaken for this appender's uncertainty. The code can quarantine/truncate unrelated bytes and then append a frame whose `previous_hash` was computed from the old head. Because the successful retry does not rescan after sync, it can return `Accepted` for a stream that will fail the next structural load.
- **Concrete legacy-prefix defect:** when a valid legacy prefix lacks a newline, the pending frame deliberately starts with a separator newline (`canonical_log.rs:889-904`). If a short write persists that separator plus only part of the frame, parsing advances `valid_up_to` to `base_byte_len + 1`, but recovery requires equality with `base_byte_len` (`canonical_log.rs:1012-1017`). The identical retry is therefore stranded as `RecoveryRequired` even though the actual suffix is provably this pending frame. The current short-write test starts from an empty file and does not exercise this case (`canonical_log.rs:2152-2185`).
- **Seed proposal:** Under `audio-graph-90f3`, add **P0: expected-head and expected-suffix recovery proof**. Persist in `PendingAppend` the base stream head/identity and the exact attempted frame. Permit destructive repair only when the current prefix exactly matches the expected base and the bytes after the original base are an exact prefix of the attempted frame. Define how the optional legacy separator is quarantined/truncated so retry is deterministic.
- **Required tests:**
  1. Replace the valid base with a different same-length, structurally valid prefix after an injected uncertain write; recovery must return `RecoveryRequired` without truncating or appending.
  2. Put arbitrary unterminated bytes after the expected base; identical retry must not classify them as the pending frame.
  3. For an unterminated legacy prefix, inject short writes at every byte boundary, including after the separator newline. Every exact pending-frame prefix must reconcile to exactly one event; unrelated suffixes must remain untouched.
  4. After every recovery path that returns `Accepted` or `AlreadyAccepted`, reopen in strict mode and prove the whole chain parses and the head equals the receipt.

### DUR-P0-3 — Public readers and path-based repair do not share the lock/identity protocol

- **Priority:** P0.
- **Evidence:** the public loader reads by pathname with `fs::read` and no shared lock (`canonical_log.rs:364-370`, `465-478`). Its recovery helper then opens the pathname again for truncation (`canonical_log.rs:373-390`, `484-497`). The documentation asks callers to quiesce writers only when selecting repair mode (`canonical_log.rs:444-447`), while the writer's lifetime lock is acquired on a different handle (`canonical_log.rs:557-584`).
- **Impact:** On Unix, advisory `flock` does not stop this reader from observing a partial in-flight frame; strict load can report transient corruption, while a mis-coordinated repair can truncate a live writer. Between path read and path reopen, a rename/replacement can cause the loader to quarantine bytes from one file and truncate another. On Windows, the exclusive `LockFileEx` range can instead make the separate reader fail while the appender is live. The same API therefore has incompatible live-read behavior across platforms.
- **Seed proposal:** Under `audio-graph-90f3`, add **P0: one-handle shared-reader and recovery identity protocol**. Reads must either go through the backend's serialized stream owner or acquire a shared lock on the same handle used for the entire read. Mutating repair must upgrade/exclusively lock and truncate that same verified file identity; a pathname must not be re-resolved between evidence capture and mutation.
- **Required tests:**
  1. Cross-process writer/reader tests on Windows, macOS, and Linux must produce either a coherent snapshot or a typed `Busy`/lock-contended result, never transient corruption from an in-flight frame.
  2. Hold a read snapshot, replace/rename the path, and attempt repair. The replacement must remain byte-for-byte unchanged.
  3. A public repair request while an appender owns the stream must fail before creating a quarantine or changing either file.

### DUR-P0-4 — Quarantine is not durably registered, and destructive-repair uncertainty loses the receipt

- **Priority:** P0.
- **Evidence:** quarantine receipts exist only in the returned snapshot or the appender's in-memory vector, whose own comment says typed-manifest integration is absent (`canonical_log.rs:106-118`, `631`, `759-764`). During appender open, quarantine creation can succeed and source truncation or truncate-sync can fail, but the function returns a plain `CanonicalLogError::Io` without the quarantine path or a typed uncertain-repair state (`canonical_log.rs:166-183`, `692-712`). The public loader has the same loss of context through `quarantine_and_truncate` (`canonical_log.rs:354-390`, `495-504`). Poisoned recovery pushes an in-memory receipt before truncate/sync, but a process crash can erase the receipt (`canonical_log.rs:1039-1068`).
- **Impact:** A destructive step can be completed or outcome-uncertain while the only discoverable pointer to the preserved bytes is lost. Reopening can see a cleanly truncated source and never know a quarantine exists. Repeated failures can also create duplicate or partial orphan artifacts. This is recoverable by filesystem forensics, not by the product's storage contract.
- **Seed proposal:** Under `audio-graph-90f3`, add **P0: manifest-first tail-quarantine transaction and startup reconciliation**. Before truncation, durably register the quarantine artifact, original stream identity/head, retained range, byte count/hash, and recovery state in the typed artifact manifest; sync the relevant file and directory entries; then truncate and transition the manifest record to complete. Error types must carry enough redacted state to distinguish `source unchanged`, `truncate outcome uncertain`, and `repair complete but acknowledgement lost`.
- **Required tests:**
  1. Inject failure after quarantine create, write, flush, file sync, directory sync, manifest prepare, source truncate, source sync, and manifest completion. For every cut point, restart reconciliation must deterministically preserve/locate the bytes and either finish or stop without a second destructive action.
  2. Verify session deletion includes prepared, completed, duplicate, and orphan quarantine artifacts.
  3. Verify diagnostics and manifest metadata never embed payload bytes while still retaining artifact hash, length, stream identity, and state.

### DUR-P0-5 — Exclusive ownership is a migration assumption, not an enforced runtime invariant

- **Priority:** P0 integration gate.
- **Evidence:** the module explicitly says the lock cannot stop a legacy/external writer that ignores it and requires old writers to be atomically quiesced/replaced (`canonical_log.rs:14-19`). The candidate opens read/write, not append (`canonical_log.rs:567-575`), and implements append as `seek(End)` followed by `write` (`canonical_log.rs:521-536`). The only ordinary guard before writing is file length (`canonical_log.rs:917-930`).
- **Impact:** The lifetime lock correctly excludes another `CanonicalAppender`, but on Unix it is advisory. A legacy or external writer can change the file between the length check, seek, write, and sync. Without `O_APPEND`, POSIX does not provide the atomic "move to current EOF and write" operation that append mode would provide. On Windows, `LockFileEx` blocks normal reads/writes to the locked range but not mapped-file writes. This does not make the kernel invalid; it makes legacy-writer retirement a hard prerequisite rather than a documentation note.
- **Seed proposal:** Keep **P0 writer retirement and ownership proof** under `audio-graph-90f3`: inventory every current writer, route them through one backend-owned stream service, remove or hard-disable legacy paths before selection, and add an explicit runtime ownership/capability check. ADR the final append/truncate handle strategy: retaining seek/write is acceptable only with proven single ownership; adopting OS append mode requires a separately proven Windows-compatible locked truncation/recovery design.
- **Required tests:**
  1. Repository contract test: no production module other than the canonical stream owner opens a canonical path for write/append/truncate.
  2. Cross-process test: a second cooperating appender is rejected on all three OSes (the existing same-process test at `canonical_log.rs:2187-2205` is insufficient).
  3. Adversarial test: a deliberately noncooperating writer must either be prevented by the integration boundary or cause a typed ownership violation before `Accepted`; document the expected Unix and Windows difference.
  4. Test path rename/replacement while the appender is live; the owner must detect loss of path identity before acknowledging subsequent events.

### DUR-P1-1 — File barriers are implemented, but the supported crash/power-loss claim is unproven

- **Priority:** P1 evidence program after the P0 ordering gaps are fixed; a release must not make a stronger claim before this evidence exists.
- **Evidence:** new writes use `flush` then `sync_all` before `Accepted` (`canonical_log.rs:932-969`), recovery uses a fresh barrier before `AlreadyAccepted` (`canonical_log.rs:1089-1126`), and open barriers an already readable stream (`canonical_log.rs:717-727`). The receipt intentionally names `FileDataAndMetadataSynced`, not total power-loss proof (`canonical_log.rs:218-227`).
- **Impact:** These are appropriate OS calls, but no portable API can guarantee honest hardware, firmware, network storage, or every filesystem. Linux documents older/less-used filesystem cache-flush limitations; Apple calls `F_FULLFSYNC` a stronger request rather than magic; Windows documents the file buffer operation, not an application-wide transaction. A unit mock that returns `Ok(())` cannot establish the real crash boundary.
- **Seed proposal:** Add **P1: canonical durability platform matrix and bounded product claim**. ADR the supported storage classes (at minimum local NTFS, APFS, and selected Linux filesystems), unsupported redirected/network/removable cases, exact meaning of each receipt, and what the UI may say. Treat any broader wording as unknown, consistent with the app's evidence-first privacy posture.
- **Required tests:**
  1. Subprocess crash harness at byte-write completion, flush completion, file sync completion, directory sync completion, manifest prepare, source truncate, and acknowledgement. Restart must classify every event as accepted, absent/retryable, or recovery-required without duplicate semantic events.
  2. VM power-cut harness on the reference Linux filesystem and manual/platform lab runs for NTFS and APFS. Record filesystem, mount flags, storage type, Rust version, and pass/fail evidence.
  3. Exercise disk-full, quota, permission, read-only, removed-device, and delayed writeback errors. `Accepted` must never be returned after a reported failed required barrier.

### DUR-P1-2 — Generic EOF recovery policy is broader than "our torn append"

- **Priority:** P1 once manifest-first preservation exists.
- **Evidence:** a missing framed terminator is repairable (`canonical_log.rs:1324-1333`), and any final parse failure is marked repairable solely when it lacks a newline (`canonical_log.rs:1335-1358`). The public/open recovery modes then quarantine and truncate such a suffix after validating the prefix, even when there is no `PendingAppend` to tie the bytes to a known attempt (`canonical_log.rs:478-504`, `679-710`).
- **Impact:** The rule is conservative about committed/newline-terminated data, which is good, but it intentionally classifies arbitrary unterminated suffix bytes as recoverable damage. That can be a valid product policy only if the original bytes are durably registered, visible to inspection/deletion, and never silently discarded. It should not be described as proof that the suffix was produced by AudioGraph.
- **Seed proposal:** Add **P1: tail classification and operator-visible recovery policy**. ADR the distinction between `known pending-frame prefix`, `unknown unterminated suffix`, and `committed/newline-terminated corruption`; expose the latter two as different typed states.
- **Required tests:** cover a valid framed record missing only its newline, every truncated framed header/length/JSON boundary, random binary suffix, duplicate ID suffix, a newline-terminated malformed last frame, middle-record corruption, and a valid unterminated legacy JSON row. Only explicitly approved final-suffix classes may mutate, and every mutation must yield a durable manifest artifact.

### DUR-P1-3 — The fault model and tests do not cover all side-effect/error combinations

- **Priority:** P1, with the destructive-repair cases promoted by DUR-P0-4.
- **Evidence:** `write_once` correctly treats `Ok(short)` and `Err` as uncertain (`canonical_log.rs:932-947`), but the memory fault model can only short-write or fail flush/sync and its errors have no partial side effect (`canonical_log.rs:2014-2077`). Existing uncertainty tests cover sync failure and one empty-stream short write (`canonical_log.rs:2102-2185`). There are no tests for read failure, write error after partial mutation, quarantine write/flush/sync failure, truncate failure, truncate-sync failure, or acknowledgement loss after successful sync.
- **Impact:** Real OS errors may be reported after partial state changes, and recovery code is defined precisely around that uncertainty. Untested failpoints are where duplicate events, lost evidence, or a permanently poisoned stream are most likely.
- **Seed proposal:** Add **P1: exhaustive canonical I/O failpoint model**. Make every side-effecting boundary injectable with independent `before`, `partial`, `after-success-but-report-error`, and `acknowledgement-lost` modes where the OS contract permits.
- **Required tests:** table-drive every phase in `CanonicalAppendPhase` and `CanonicalIoOperation`; after each injected result, retry identical and different commitments, reopen strict, verify the complete hash chain, verify receipt durability type, verify all quarantine artifacts, and prove no payload content appears in diagnostics.

### DUR-P2-1 — Preflight I/O failure is misclassified as concurrent modification

- **Priority:** P2 DevEx/observability hardening.
- **Evidence:** failure to obtain file metadata/length is converted to `Rejected(ConcurrentModification)` with the I/O kind discarded (`canonical_log.rs:917-924`). By contrast, later write/flush/sync errors retain the `io::ErrorKind` inside uncertainty (`canonical_log.rs:932-961`).
- **Impact:** Operators and recovery policy cannot distinguish an actual competing writer from a permissions/device/filesystem error. No write has been attempted, so rejection is safe, but the diagnosis and retry guidance are wrong.
- **Seed proposal:** Add **P2: typed preflight I/O rejection** with redacted operation and `io::ErrorKind`, distinct from observed-length mismatch.
- **Required test:** inject `len()` failures for `PermissionDenied`, `NotFound`, `Other`, and device-style errors; assert no write occurs and the typed outcome preserves the operation/kind without path, event ID, or payload content.

### DUR-P2-2 — Failed quarantine creation can leave untracked partial artifacts

- **Priority:** P2 after manifest-first P0 work, or fold into DUR-P0-4.
- **Evidence:** after `create_new`, a quarantine write/flush/sync error returns only operation/kind; the candidate path is not returned and no cleanup/reconciliation marker is written (`canonical_log.rs:408-435`). `set_owner_only` is also best-effort and its result is ignored (`canonical_log.rs:425`).
- **Impact:** The source is not truncated on these failures, so canonical data is safe, but partial sensitive artifacts can accumulate outside the manifest and deletion lifecycle. Repeated recovery attempts produce more files.
- **Seed proposal:** Add **P2: quarantine staging cleanup and permission verification**, preferably as part of the manifest transaction. A failed staging artifact must be atomically discoverable for reconciliation or securely removed; permission-setting failure must be typed according to the storage security policy.
- **Required tests:** inject partial write, flush, sync, and permission failures; restart; prove every created candidate is either registered and retained or removed, and session deletion leaves no candidate behind.

## Important non-gaps and claim boundaries

- **One `write` call is not a power-failure transaction.** The one-call design is useful because it avoids application-level interleaving and exposes short writes, but neither POSIX regular-file write atomicity nor Windows `WriteFile` makes a multi-sector frame failure-atomic across power loss. The trailing newline plus declared length, JSON decoding, payload hash, record hash, and previous hash are therefore detection mechanisms. The parser correctly fails closed when a newline survives but any earlier frame byte does not (`canonical_log.rs:1454-1547`). Do not rewrite this into a claim that the commit newline alone proves durability.
- **`flush` before `sync_all` is harmless for `std::fs::File`.** Rust `File` itself is unbuffered in user space, so the durability-bearing call is `sync_all`; retaining `flush` keeps the trait contract explicit and permits buffered test/alternate implementations.
- **Short writes are not silently completed.** Returning uncertainty instead of looping is the safer event-boundary behavior (`canonical_log.rs:932-947`). The needed fix is better ownership/recovery proof, not an unconditional `write_all`, which could obscure how much of a frame became visible between writes.
- **Committed corruption remains fail-closed.** Restricting automatic tail repair to an unterminated last suffix means a newline-terminated bad frame, a middle-record error, a chain mismatch, or duplicate ID is rejected (`canonical_log.rs:1276-1547`). Keep this conservative rule.
- **`sync_all` success is a bounded receipt, not an absolute hardware promise.** The current variant name `FileDataAndMetadataSynced` and its parent-directory caveat are materially more accurate than a generic `Durable` boolean (`canonical_log.rs:218-227`). Preserve that specificity when the platform/directory receipt is expanded.

## Minimum promotion gate from research kernel to MVP runtime

All of the following are required before wiring `Pending -> Accepted` product state to this appender:

1. DUR-P0-1 through DUR-P0-5 are implemented or superseded by an ADR with an equally strong fail-closed design.
2. The append receipt represents every required file, directory-entry, and manifest barrier for the actual create/existing/repair path; no caller advances live state on a weaker receipt.
3. One backend-owned service is the sole canonical writer; every reader and repair operation uses its serialized/locked identity protocol; legacy writers are proven unreachable.
4. Startup reconciliation covers outcome-uncertain appends, prepared/completed quarantine entries, orphan staging files, and acknowledgement loss.
5. The exhaustive failpoint suite passes, including the unterminated-legacy separator case and same-length base replacement.
6. Cross-process tests pass on Windows, macOS, and Linux. Process-crash evidence exists for every phase, and bounded power-cut evidence exists for the reference filesystem path.
7. A strict reopen validates the complete chain and receipt head after every successful normal append and every successful recovery in the test matrix.
8. The storage/UX documentation states the actual supported filesystem boundary and shows recovery-required states without claiming data is safe when evidence is incomplete.

## Final verdict

**Bounded research kernel only.** The intra-file append state machine is a strong foundation and is substantially safer than ad hoc JSONL append. It is not yet an adequate production durability boundary because the transaction spans more than the log file: parent directory entries, the typed quarantine manifest, file identity, all readers/writers, and restart reconciliation are part of the same correctness claim. Promoting it before the five P0 contracts are proven would allow `Accepted` to outrun recoverable evidence.
