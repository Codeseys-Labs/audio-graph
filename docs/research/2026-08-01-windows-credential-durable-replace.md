# Windows credential-v2 owner-only durable replacement

Date: 2026-08-01

Seed: `audio-graph-b138`

Related decision: [ADR-0035](../adr/0035-backend-owned-credential-service.md)

Prerequisite:
[credential-v2 lock and atomic-replacement primitives](2026-08-01-credential-lock-atomic-replace.md)

## Question and gated decision

What exact Windows primitive sequence should AudioGraph use to replace the
credential-v2 authority journal and explicit file-v2 backend with owner-only,
complete old-or-new visibility and the strongest non-administrative namespace
durability that Win32 documents?

This gates the Windows implementation boundary and release tests. It does not
gate the native Windows Credential Manager adapter, and it does not make the
explicit degraded-security file-v2 backend an automatic fallback. [documented]

Evidence labels in this note mean: **[verified]** was checked in this repository,
exact source, or the native dummy-byte prototype; **[documented]** is stated by
Microsoft or exact Rust source; **[inferred]** is the engineering conclusion
drawn from that evidence. [documented]

## Recommendation

**Select a narrow direct-Win32 wrapper around a protected `CreateFileW`
temporary file, `FlushFileBuffers`, and same-directory
`MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)`, followed by
mandatory metadata and envelope readback.** The sequence is selected with high
confidence for owner-only creation, ACL outcome, error staging, and complete
old-or-new process-crash behavior on the tested Windows/NTFS host. It is selected
with medium confidence for namespace durability because Microsoft documents the
write-through flag but explicitly describes its flush guarantee for copy/delete,
not a same-volume NTFS rename, and no abrupt-reset experiment ran here.
[documented/verified/inferred]

The Windows wrapper is **release-gated**, not fully release-proven by this note.
Do not claim durable file-v2 support until the packaged-path abrupt-VM NTFS gate
below passes. Process termination, exact readback, and a successful
`MOVEFILE_WRITE_THROUGH` return cannot prove persistence across loss of volatile
OS, hypervisor, controller, or drive caches. [inferred]

| Requirement | Decision | Confidence |
| --- | --- | --- |
| Permission before bytes | Explicit self-relative security descriptor with current process-token user as owner and the only allow ACE; protected DACL; verify on the returned handle before writing | High [documented/verified] |
| File-content durability | Complete buffered write, then successful `FlushFileBuffers(temp)` | High for documented API behavior [documented] |
| Atomic visibility | Same-directory `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)`; native readers observed only complete old/new generations | Medium-high pending packaged/minimum-OS matrix [verified/inferred] |
| Final ACL | Keep the protected temp object as the replacement; same-volume move retained its owner-only DACL even over a permissive old destination | High on local NTFS [documented/verified] |
| Namespace durability | `MOVEFILE_WRITE_THROUGH` is the only selected non-administrative documented namespace flag; abrupt-reset proof remains mandatory | Medium, release-blocked [documented/inferred] |
| Replace-call errors | From invocation until exact readback, status is `commit_unknown`, including a false return | High [verified/inferred] |
| Supported storage | Normalized, fixed, local NTFS with persistent ACLs only; no UNC/SMB, ReFS, removable, reparse/cloud-placeholder, or otherwise unproved volume | High as a fail-closed initial scope [documented/inferred] |

## Exact wrapper contract

The wrapper must keep this contract private to the Rust backend. It must not
expose raw handles, private paths, SDDL, or native error text through IPC, logs,
analytics, docs generated at runtime, or Seeds. [documented/inferred]

1. **Hold the stable mutation lock.** Resolve the Tauri app-config
   `credential-v2` directory, acquire `mutation.lock`, and keep it across
   journal load, expected-revision check, temp creation, replacement, readback,
   journal commit, and recovery classification. [documented]
2. **Validate the parent and volume.** Open the final parent, obtain its
   normalized handle path and volume identity, and require fixed local NTFS plus
   `FILE_PERSISTENT_ACLS`. Require the parent itself to have the approved
   protected user-only DACL. Reject UNC syntax, SMB/remote results, ReFS,
   removable media, reparse/cloud-placeholder paths, and any source/final
   volume mismatch. `GetFinalPathNameByHandleW` resolves the handle path,
   `GetDriveTypeW` distinguishes fixed from remote/removable media, and
   `GetVolumeInformationByHandleW` reports filesystem name and ACL support;
   SMB does not support that volume-management call. See
   [final path](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew),
   [drive type](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getdrivetypew),
   and [volume information](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationbyhandlew).
   [documented]
3. **Build the creation descriptor.** Read the process token's user SID. Build
   a self-relative descriptor equivalent to
   `O:<user-sid>D:P(A;;FA;;;<user-sid>)`: the user is owner, the DACL is
   protected from inheritance, and its sole ACE grants that user file full
   access. `P` is the SDDL protected-DACL flag and `FA` is file full access.
   [Security-descriptor string
   format](https://learn.microsoft.com/en-us/windows/win32/secauthz/security-descriptor-string-format)
   and [ACE strings](https://learn.microsoft.com/en-us/windows/win32/secauthz/ace-strings)
   define those tokens. [documented]
4. **Create one unique same-directory temp.** Use a long random name from a
   bounded retry loop and `CreateFileW` with `CREATE_NEW`, desired access
   `GENERIC_READ | GENERIC_WRITE | DELETE | READ_CONTROL | WRITE_DAC |
   SYNCHRONIZE`, share mode `FILE_SHARE_READ | FILE_SHARE_DELETE`,
   `FILE_ATTRIBUTE_NORMAL`, the explicit descriptor, and
   `bInheritHandle = FALSE`. Do not request `FILE_ATTRIBUTE_TEMPORARY`,
   `FILE_FLAG_DELETE_ON_CLOSE`, or `MOVEFILE_COPY_ALLOWED`. `CREATE_NEW`
   refuses an existing name, and delete sharing permits a later rename while
   the creator's handle remains open. [`CreateFileW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew)
   [documented]
5. **Verify permission on the handle before bytes.** Call `GetSecurityInfo` on
   the returned handle with owner and DACL information. Parse rather than merely
   string-compare: owner equals the process-token user SID, DACL is present and
   protected, exactly one non-inherited allow ACE names that SID with the
   approved rights, and no other allow/deny/object/callback ACE exists. Any
   mismatch is `permission_hardening_failure`; close and remove the still-empty
   temp, recording cleanup failure separately. [`GetSecurityInfo`](https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-getsecurityinfo)
   [documented]
6. **Write and flush completely.** Encode the whole envelope in bounded memory,
   write until every byte is accepted, and call `FlushFileBuffers` on the temp
   handle. Any create, permission, encode, write, or file-flush error is
   definitely pre-replace: the old destination remains authoritative and no
   replacement call is allowed. An owner-only leftover may require later
   locked cleanup. [`FlushFileBuffers`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers)
   [documented]
7. **Record reconciliation identity.** Before replacement, retain the expected
   operation id, revision, content schema, temp file id, and volume serial in
   memory. The latter pair uniquely identifies a file on one computer and can
   confirm that the final handle is the moved temp object.
   [`FILE_ID_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info)
   [documented]
8. **Invoke one same-volume replacement.** While the temp handle remains open,
   call `MoveFileExW(temp, final, MOVEFILE_REPLACE_EXISTING |
   MOVEFILE_WRITE_THROUGH)`. Omitting `MOVEFILE_COPY_ALLOWED` makes a volume
   mismatch fail instead of degrading to non-atomic copy/delete. The same call
   works when the destination is absent. Microsoft says `WRITE_THROUGH` does
   not return until the file is moved on disk, but its explicit guarantee text
   is about copy/delete; same-volume durability is therefore still a runtime
   gate. [`MoveFileExW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw)
   [documented/inferred]
9. **Treat the call boundary as uncertain.** Immediately before invoking
   `MoveFileExW`, transition the in-memory result stage to `commit_unknown`.
   A true return does not become application success before readback. A false
   return—including `ERROR_ACCESS_DENIED` or `ERROR_SHARING_VIOLATION`—also
   remains `commit_unknown`; capture `GetLastError` immediately, then inspect
   the final name and bounded temp name under the lock. Never fall back to
   in-place write, generic `std::fs::rename`, `ReplaceFileW`, or a rename
   lacking write-through. [verified/inferred]
10. **Reconcile and bounded-retry contention.** If the final envelope is the
    prior generation and the exact owner-only temp is still the complete
    candidate, classify that attempt `not_committed`; a short bounded retry of
    the same temp is allowed while still holding the lock. If the final envelope
    is the candidate and the temp name is absent, continue as possibly
    committed. Any other combination is `recovery_required`. Exhausted
    access/sharing contention is a typed operation-in-progress failure, never a
    durability downgrade. [verified/inferred]
11. **Read back through the final name.** Open with read/control access and
    `FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE`, so the still-open
    temp/final write handle and future replacements remain compatible. Verify
    exact schema, operation id, revision, complete bytes, final file/volume id,
    and the same protected user-only DACL. Readers must close file handles
    promptly; native evidence below shows that delete sharing alone did not
    make `MoveFileExW` replace a destination with a currently open reader.
    [verified/inferred]
12. **Publish only after readback.** Close handles, delete only a verified
    wrapper-owned leftover when safe without claiming secure media erasure,
    commit the authority journal, release the mutation lock, and then publish
    the event. A readback, permission, identity, or cleanup ambiguity after
    possible replacement never becomes success. [documented/inferred]

## Why `MoveFileExW` is selected

### It retains the protected temp instead of merging old access

Microsoft documents that cross-volume `MoveFileExW` does not carry the old
security descriptor, while same-volume moves preserve the moved object's ACL in
normal Windows behavior. Microsoft also advises protecting an ACL when it must
remain unchanged across a same-volume move. The native proof replaced an
existing destination granting `Everyone` read with a protected temp granting
only the current user full access; the final name had exactly the temp's
protected one-ACE DACL. See Microsoft's
[same-volume ACL behavior](https://learn.microsoft.com/en-us/troubleshoot/windows-server/windows-security/inherited-permissions-not-automatically-update)
and [`MoveFileExW` remarks](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw).
[documented/verified]

This property is necessary but does not remove the parent-directory check. A
different principal with delete-child rights on the parent may still remove or
rename an owner-only file without being able to read its content. [inferred]

### It has the only viable documented non-admin write-through flag

`FlushFileBuffers(temp)` addresses buffered file information. Flushing every
open file through a volume handle requires administrator privilege and is not a
viable desktop application contract. `MOVEFILE_WRITE_THROUGH` is therefore the
narrowest documented non-administrative namespace primitive among the compared
APIs. It is necessary, but abrupt-reset evidence is still required because the
documentation's strongest explicit flush sentence addresses copy/delete and
the product deliberately forbids cross-volume copy/delete. See
[`FlushFileBuffers` volume remarks](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers)
and [`MoveFileExW` flags](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw).
[documented/inferred]

### Rust 1.95 `std::fs::rename` is not equivalent

Exact Rust 1.95 source first calls `MoveFileExW` with only
`MOVEFILE_REPLACE_EXISTING`. On `ERROR_ACCESS_DENIED` it may fall back to
`SetFileInformationByHandle(FileRenameInfoEx)` with replace and POSIX flags.
Neither path requests `MOVEFILE_WRITE_THROUGH`, and the fallback exposes no
subsequent namespace flush. AudioGraph therefore needs an explicit Windows
wrapper and must not substitute `std::fs::rename`. [Rust 1.95 Windows
filesystem source](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/windows.rs#L1311-L1379)
[verified/inferred]

The POSIX rename flag is useful prior art for visibility with open destination
handles: Microsoft states that it allows replacement while existing handles
remain valid and makes subsequent opens use the new file. It is rejected here
as a fallback because neither `FILE_RENAME_INFO_EX` nor
`SetFileInformationByHandle` supplies a documented write-through namespace
operation. See Microsoft
[`FILE_RENAME_INFORMATION` flags](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information)
and [`SetFileInformationByHandle`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle).
[documented/inferred]

## Why `ReplaceFileW` is rejected

`ReplaceFileW` has attractive open-reader behavior and a documented multi-step
replacement operation, but it violates two load-bearing requirements:

- `REPLACEFILE_WRITE_THROUGH` is explicitly “not supported.” A native call on
  the tested Windows build even returned success when that flag was passed;
  that result demonstrates why an accepted/ignored flag cannot be treated as a
  persistence signal. [`ReplaceFileW` flags](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew)
  [documented/verified]
- The function deliberately preserves/merges the replaced file's DACL and other
  metadata into the replacement. Native replacement of an `Everyone`-readable
  old file produced a final protected DACL that still contained the
  `Everyone`-read ACE. Hardening after replacement creates a content-exposure
  window, while ignoring ACL-merge errors makes security outcome conditional.
  [`ReplaceFileW` preserved
  attributes](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew#remarks)
  [documented/verified]

Pre-hardening every old destination would narrow the ACL issue but would not
create write-through semantics, would not handle the absent-destination case,
and would add another externally visible mutation before candidate bytes are
committed. It is therefore rejected rather than used as a split path. [inferred]

`ReplaceFileW` also documents partial-state error codes where the old file can
be gone or renamed and the replacement can inherit streams/attributes despite a
false return. Those cases reinforce the common `commit_unknown` rule, but they
do not justify selecting the API. [`ReplaceFileW` return
states](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew#return-value)
[documented]

## Native Windows/NTFS dummy proof

### Environment and limits

The out-of-repository probe was a .NET 9 console harness using direct P/Invoke
for the exact Win32 calls. It compiled without warnings and ran natively as the
ordinary logged-in user on Windows 11 build `10.0.22631.6199`. The handle-level
volume query reported local `NTFS` and `FILE_PERSISTENT_ACLS`. Every payload was
a fixed dummy `AUDIOGRAPH-DUMMY` generation; no real credential or private
AudioGraph path was used. [verified]

This was a process/API proof in the Windows temporary directory, not the
packaged Tauri app-config path and not an abrupt-reset durability proof.
[verified]

### Permission and metadata outcome

- An explicitly supplied descriptor created a protected DACL with the current
  token user as owner and sole full-access ACE. `GetSecurityInfo` verified it on
  the returned handle before the first byte. A deliberately permissive
  two-ACE temp was detected and rejected at length zero. [verified]
- `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` succeeded while the source
  temp handle remained open, both with an existing destination and an absent
  destination. Exact readback was the complete new generation. [verified]
- Replacing an old destination that granted `Everyone` read produced a final
  path with only the protected current-user ACE; the temp name disappeared.
  `ReplaceFileW`, by contrast, produced a final DACL retaining the
  `Everyone`-read ACE. [verified]

### Concurrent visibility and sharing

- Across completed stress runs configured for 120 replacements and four
  readers, readers repeatedly reopened the destination with read/write/delete
  sharing. Every successful read was exactly one 64-KiB old or new dummy
  generation; invalid, missing, and reader-sharing outcomes were zero. In a
  representative final run there were 45 old and 51 new reads. [verified]
- Writer calls intermittently returned Win32 error 5 (`ACCESS_DENIED`) while
  readers were active—between zero and four calls in completed runs. Readback
  found the old destination plus the complete temp, so each attempt was safely
  classified not committed and retried. No false return was observed to have
  committed, but the wrapper must not generalize that observation. [verified/inferred]
- Holding one otherwise compatible destination reader open caused
  `MoveFileExW` to return error 5 even though that handle included delete
  sharing; its old handle still read the complete old generation. Closing it
  allowed the exact same temp to replace successfully. `ReplaceFileW` succeeded
  while a compatible reader remained open, which is useful comparison evidence
  but does not cure its ACL/durability failures. [verified]
- A reader opened without delete sharing also caused error 5, not necessarily
  `ERROR_SHARING_VIOLATION`. Reconciliation again found the complete old
  destination and complete candidate temp; closing the blocker allowed a
  successful retry. Error handling must therefore use stage and readback, not a
  single expected Win32 code. [verified/inferred]
- An initial continuous-reader stress variant provided no intentional reopen
  gap and did not complete within 30 seconds before being stopped. This is an
  availability warning: persistent readers can starve this selected durable
  primitive, so readers must close promptly and writer retry must be bounded.
  [verified/inferred]

### Failure stages and forced process termination

- `CREATE_NEW` collision returned error 80 with the old destination unchanged.
  Calling `FlushFileBuffers` on an invalidated handle returned error 6 before
  any replace attempt. Replacing a directory returned error 5; reconciliation
  found the destination directory unchanged and the complete owner-only temp.
  [verified]
- Forced process termination at `temp-created`, `partial-written`, and
  `file-flushed` left the complete old destination and an owner-only named
  temp. Forced termination immediately after `MoveFileExW` returned left the
  complete new destination, preserved its protected DACL, and left no temp
  name. [verified]
- These termination points did not interrupt inside the kernel call and did not
  cut VM/storage power. They validate restart reconciliation shape, not
  same-volume write-through persistence. [verified/inferred]

## Error-stage contract

| Failure point | Immediate classification | Required locked action |
| --- | --- | --- |
| Parent/volume/path validation | Definitely not committed | Return unsupported or permission failure; no temp |
| Unique `CREATE_NEW` | Definitely not committed | Bound collision retries; preserve old destination |
| Handle DACL verification | Definitely not committed, no content bytes | Close/remove empty temp; report hardening failure |
| Encode/write | Definitely not committed | Close; delete only verified residue when safe or record owner-only residue |
| `FlushFileBuffers(temp)` | Definitely not committed | Never invoke replace; old remains authority |
| Immediately before or during `MoveFileExW` | `commit_unknown` | Inspect exact final and temp under the mutation lock |
| False replace return, including error 5/32 | `commit_unknown` | Readback; retry only if old is exact and complete candidate temp remains |
| True replace return | `commit_unknown` until verified | Reopen final; verify envelope, DACL, volume/file id |
| Final readback/parse/identity/DACL failure | `commit_unknown` or `recovery_required` | Do not journal/event success; retain recovery evidence |
| Cleanup failure after verified final | Commit state unchanged; cleanup pending | Never roll back or overwrite verified final blindly |

The wrapper may expose typed, content-free stage codes internally, but raw
Win32 messages and paths remain backend-private. [documented/inferred]

## Adversarial pass and failure modes

- A true return from an unsupported `ReplaceFileW` flag is worse than a clean
  error because it can tempt code to infer durability that Microsoft explicitly
  does not promise. [documented/verified]
- Delete sharing did not guarantee that `MoveFileExW` would replace an open
  destination on the tested NTFS host. Antivirus, indexers, backup agents, and
  other filters can widen that contention window. Bounded retry plus exact
  reconciliation is part of correctness, not an optional polish. [verified/inferred]
- Retrying a false replace call without readback can overwrite a generation
  that actually became visible. A false return is never globally equivalent to
  “old file remains.” [inferred]
- Owner-only file DACL does not compensate for a permissive parent with
  delete-child access, nor does it protect against administrators who exercise
  ownership/backup privileges. The contract is confidentiality and integrity
  among ordinary principals and cooperating AudioGraph processes, not defense
  from the OS administrator. [documented/inferred]
- A same-user process can ignore the mutation lock and race names/handles. The
  wrapper narrows this with `CREATE_NEW`, retained handles, file ids, and exact
  readback but does not claim hostile same-user isolation. [inferred]
- Local NTFS under OneDrive or another cloud-files provider can still have
  placeholder/filter semantics. Merely seeing the string `NTFS` is insufficient;
  the final support detector must reject cloud-placeholder/reparse paths.
  [`CfGetPlaceholderStateFromAttributeTag`](https://learn.microsoft.com/en-us/windows/win32/api/cfapi/nf-cfapi-cfgetplaceholderstatefromattributetag)
  [documented/inferred]
- `DRIVE_FIXED` is not proof that media is physically internal or non-USB;
  Microsoft directs callers to device removal policy for USB classification.
  If the support contract excludes physically removable storage, the detector
  must map the volume to that device policy or fail closed rather than treating
  `GetDriveTypeW` as sufficient. [documented/inferred]
- Process kill closes handles and lets Windows finish normal storage-stack
  cleanup. It is strictly weaker than an abrupt VM reset or power cut. [inferred]
- Even an abrupt VM test proves only the tested Windows build, NTFS version,
  virtualization/storage stack, and cache settings. Release wording must remain
  bounded to the supported matrix. [inferred]

## Rejected alternatives

- **`ReplaceFileW`:** rejected for unsupported write-through and mandatory old
  DACL/metadata merging. [documented/verified]
- **Rust 1.95 `std::fs::rename`:** rejected because it omits
  `MOVEFILE_WRITE_THROUGH` and can silently take a non-write-through
  `FileRenameInfoEx` fallback. [verified]
- **`SetFileInformationByHandle(FileRenameInfoEx)` as fallback:** rejected even
  though POSIX semantics handle open readers; it lacks documented namespace
  write-through. [documented/inferred]
- **Post-create or post-replace ACL hardening:** rejected because secret bytes
  would exist during a permissive exposure window. [inferred]
- **Default/inherited DACL:** rejected because the parent may grant non-owner
  access before bytes. [documented/verified]
- **Closing the temp before the replace:** rejected because retaining a
  non-inheritable, non-write-shared handle narrows path/object substitution and
  was compatible with `MoveFileExW` in native proof. [verified/inferred]
- **`MoveFileExW` without `WRITE_THROUGH`:** rejected because it discards the
  only applicable documented namespace durability request. [documented]
- **`MOVEFILE_COPY_ALLOWED`:** rejected because cross-volume copy/delete is not
  atomic and assigns a destination-default security descriptor. [documented]
- **In-place truncate/write:** rejected because readers can observe missing or
  partial generations and a crash can destroy the old authority. [inferred]
- **Administrative volume flush:** rejected as a desktop runtime requirement;
  Microsoft requires administrative privilege to flush all volume files.
  [documented]

## Remaining release gates and cheapest decisive experiments

1. **Abrupt-reset namespace durability — blocking.** The cheapest decisive
   experiment is a disposable Windows VM on local NTFS with a host-controlled
   serial/named-pipe barrier. Repeat absent/existing replacements and hard-reset
   the VM at temp-created, partial-write, file-flush-returned,
   replace-invoked, replace-returned, and readback-returned barriers. On reboot,
   accept only a complete old or new envelope with the protected DACL and
   consistent journal recovery. Record hypervisor disk-cache mode and run enough
   cycles to make the supported claim explicit. [inferred]
2. **Packaged app-config path — blocking.** Run the exact wrapper from the
   packaged Tauri application on the minimum supported Windows release and
   actual bundle-derived app-config directory. Prove non-administrator use,
   parent/temp/final DACLs, normalized local NTFS identity, no reparse/cloud
   placeholder, absent/existing destinations, endpoint-security interference,
   restart recovery, and content-free telemetry. [inferred]
3. **Reader/contention contract — blocking.** Run short-lived cooperative
   readers plus a deliberately persistent compatible reader. Prove all
   successful reads are old/new complete, access errors stay typed, retry ends
   at its deadline without fallback, and readback never reports a false success.
   [inferred]
4. **Fault-injection matrix.** Inject every table stage, including cleanup and
   final metadata/file-id queries. This can be deterministic unit/contract work
   once the wrapper seam exists; only the native sharing and durability rows
   require Windows hosts. [inferred]

Until gates 1–3 pass, journal/file-v2 support on Windows remains development or
recovery-only and must not be described as crash-durable production storage.
[inferred]

## Out-of-scope Seed proposals

These are proposals only; this assignment forbids Seed edits. [verified]

- Child of `audio-graph-fb2b`: **Implement the stage-aware Windows credential
  file primitive** with the exact sequence and deterministic fault seams in this
  note; acceptance includes no generic rename/ReplaceFile fallback and complete
  content-free error mapping. [inferred]
- Child of `audio-graph-fb2b`: **Run the packaged Windows NTFS abrupt-reset
  release lab**; acceptance includes host-controlled cut points, cache-mode
  record, absent/existing cases, DACL verification, and a bounded release claim.
  [inferred]
- Update the existing supported-filesystem work rather than duplicating it:
  include fixed local NTFS, normalized/reparse/cloud-placeholder detection, and
  fail-closed UNC/SMB/ReFS/removable behavior. [inferred]

## Primary sources

- Microsoft Win32 file operations:
  [`CreateFileW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-createfilew),
  [`FlushFileBuffers`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-flushfilebuffers),
  [`MoveFileExW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-movefileexw), and
  [`ReplaceFileW`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-replacefilew).
  [documented]
- Microsoft security and volume contracts:
  [security-descriptor string format](https://learn.microsoft.com/en-us/windows/win32/secauthz/security-descriptor-string-format),
  [ACE strings](https://learn.microsoft.com/en-us/windows/win32/secauthz/ace-strings),
  [`ConvertStringSecurityDescriptorToSecurityDescriptorW`](https://learn.microsoft.com/en-us/windows/win32/api/sddl/nf-sddl-convertstringsecuritydescriptortosecuritydescriptorw),
  [`GetSecurityInfo`](https://learn.microsoft.com/en-us/windows/win32/api/aclapi/nf-aclapi-getsecurityinfo),
  [`GetVolumeInformationByHandleW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationbyhandlew),
  [`GetFinalPathNameByHandleW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getfinalpathnamebyhandlew), and
  [`GetDriveTypeW`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getdrivetypew).
  [documented]
- Microsoft rename/identity detail:
  [`SetFileInformationByHandle`](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-setfileinformationbyhandle),
  [`FILE_RENAME_INFORMATION`](https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information),
  [`FILE_ID_INFO`](https://learn.microsoft.com/en-us/windows/win32/api/winbase/ns-winbase-file_id_info), and
  [same-volume ACL behavior](https://learn.microsoft.com/en-us/troubleshoot/windows-server/windows-security/inherited-permissions-not-automatically-update).
  [documented]
- Exact [Rust 1.95 Windows filesystem
  source](https://github.com/rust-lang/rust/blob/1.95.0/library/std/src/sys/fs/windows.rs#L1311-L1379).
  [verified]
