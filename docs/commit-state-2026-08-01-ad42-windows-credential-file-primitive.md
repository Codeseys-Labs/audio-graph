# Commit state: Windows credential durable-replace primitive

Date: 2026-08-01

Seed: `audio-graph-ad42`

## Starting point

- Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/ad42-windows-credential-file-primitive`
- Branch: `work/audio-graph-ad42-windows-credential-file-primitive`
- Fixed base: `b9bdd48d11144edbee475237028a775f6e0ba0b6`
- Initial status: clean
- Shared-checkout caveat: the main checkout and every sibling worktree are
  custody-only. This slice does not edit, stage, reset, merge, or otherwise
  reconcile their work.

## Acceptance and confirmed seams

Implement the dark backend-private Windows file replacement primitive selected
by `audio-graph-b138`:

- accept only the already-held qualified Windows parent capability;
- create a same-directory `CREATE_NEW` candidate with a protected current-user
  owner/DACL and verify that security while its length is zero;
- write the bounded complete envelope, flush it, and retain candidate file,
  volume, operation, and revision identities;
- enter `commit_unknown` immediately before invoking
  `MoveFileExW(REPLACE_EXISTING | WRITE_THROUGH)` while the candidate handle
  remains open;
- reconcile both true and false native returns through exact final/temp
  envelope, metadata, file-id, and volume-id observations; and
- retry only a verified prior final plus the exact complete candidate, within a
  fixed bound.

The confirmed deterministic test seam is the crate-private stage-aware
replacement operation driven by a scripted platform boundary. The native seam
is an ignored, explicit-environment, dummy-only Windows smoke. Neither seam is
production wiring or a release grant.

## Ownership and exclusions

Owned paths are `credentials/adapters/**`, one dark declaration in
`credentials/mod.rs`, the backend-private Windows held-parent capability and
its nested file-replacement implementation under
`credentials/filesystem_policy/windows*`, tests inside those modules, and this
record. Cargo manifests, legacy filesystem utilities, service/domain behavior,
IPC, frontend, migration, profiles, native credential stores, workflows, and
release wiring are excluded.

Both compiled evidence-profile constants remain literal empty slices. Raw
paths, handles, SIDs, file/volume identities, native codes, and native prose
remain private and non-serializable.

## Verification state and sync policy

- TDD recorded four distinct RED results before their fixes: the initial
  parent-validation state-machine failure, a non-collision final create-attempt
  misclassification, encoded-metadata validation missing from the envelope,
  and missing fail-closed accounting for an absent pre-commit candidate path.
  The final platform-neutral suite passes all 16 deterministic tests.
- The locked cloud and default-feature credential suites each pass 164 tests
  with the existing opt-in OS-keychain smoke ignored. The accepted IPC
  credential contract passes 14 tests.
- Locked cloud `cargo check`, strict all-target Clippy with `-D warnings`, Rust
  formatting, diff checks, generated credential/runtime/endpoint contracts,
  generator-launcher checks, and the secret-hygiene fixture all pass. The
  normal hygiene scan retains exactly the accepted six pre-existing findings
  and adds no finding in an owned path.
- A temporary isolated crate compiled the exact production Windows modules and
  their target-only tests for `x86_64-pc-windows-msvc`, then passed strict
  target all-target Clippy. The temporary crate and its generated artifacts
  were removed before handoff.
- The full application Windows cross-check cannot reach AudioGraph source on
  this Linux host: native dependency build scripts require the absent MSVC
  `lib.exe`/toolchain and reject the host GNU compiler. This is an environment
  blocker, not native execution evidence.
- The ignored dummy-only smoke now covers absent-destination creation followed
  by existing-destination replacement, exact prior/new bytes, final owner-only
  metadata, and no new temp residue. It was source-checked but not run because
  no native Windows host is available. Packaged LocalAppData, permissive-old,
  persistent-reader/endpoint-security contention, and abrupt-VM NTFS
  durability remain the separate `audio-graph-4af0` evidence scope.
- Both compiled filesystem evidence tables remain literal empty slices, so
  this dark implementation grants neither journal nor file-v2 support.
- `sd sync` is intentionally not run: Seed mutations belong to the conductor,
  and syncing from this worktree could sweep unrelated shared queue changes.

## Post-review correction

A separate correction after implementation review tightened three native
boundaries without changing manifests, profiles, workflows, or production
wiring:

- Candidate, final, and temporary file identities now compare their full
  64-bit `FILE_ID_INFO.VolumeSerialNumber` with the qualified parent's full
  `DirectoryIdentity.volume_serial`. The 32-bit volume label returned by
  `GetVolumeInformationByHandleW` is no longer used for this exact-file
  identity comparison. A host regression proves equal low 32 bits with
  different high 32 bits do not match.
- Cleanup now opens the candidate with `DELETE` access, verifies the opened
  handle's exact file identity and owner-only DACL, and calls
  `SetFileInformationByHandle(FileDispositionInfo)` on that same live handle.
  It never closes the verified handle and re-resolves a pathname for deletion.
  The existing missing-path state classifier remains fail-closed before a
  verified commit.
- The generic raw `HANDLE`/`PCWSTR` callback escape hatches were removed. The
  native replacement module is now structurally nested below the Windows
  detector that owns the private qualified handle and child-path buffers; the
  adapter path is a narrow compatibility alias.

Correction verification passed the 16-test platform-neutral replacement suite,
the 11-test Windows policy suite, exact production-source Windows target
`check --tests`, strict Windows target Clippy, strict locked-cloud host Clippy,
Rust formatting, and diff checks. The temporary target harness and its module
shim were removed. Native runtime evidence remains unavailable, so
`audio-graph-ad42` must remain open.
