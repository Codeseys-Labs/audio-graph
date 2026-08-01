# Commit state: Windows credential filesystem detector

Date: 2026-08-01

Seed: `audio-graph-9a1e`

## Starting point

- Worktree: `.worktrees/9a1e-windows-filesystem-detector`
- Branch: `work/audio-graph-9a1e-windows-filesystem-detector`
- Base: `180f8595c48f4d8e2567bdeaafe644dd633b3e19`
- Initial status: clean
- Shared checkout caveat: Linux and macOS detector work proceeds in sibling
  worktrees. This slice owns only the Windows adapter, its test-only module
  declaration, the exact Windows binding features it uses, and this note.

## Test seams

- Host tests exercise a closed Windows metadata acquisition/classification
  seam with scripted outcomes and opaque identity tokens.
- Native Windows code owns handles, paths, identifiers, native result codes,
  and error values; none of those values enter the common observation or
  status contract.
- The production adapter is compiled only for Windows. Rust 1.95.0 has the
  `x86_64-pc-windows-msvc` standard library installed, and an isolated harness
  typechecked this exact module against `windows` 0.62.2. A full application
  cross-check stops in `ring` because the host lacks MSVC `lib.exe`; the native
  metadata smoke therefore remains an explicit Windows evidence gate.

## Invariants

- A single held directory handle is used for the complete inspection and final
  identity/volume recheck.
- Only NTFS with known writable, ACL, fixed, non-hotplug, non-remote, and exact
  Cloud Files not-under-sync-root observations becomes a candidate.
- ReFS and all unknown/error/race outcomes remain denied before any evidence
  profile could authorize them.
- Both compiled evidence-profile tables remain literal empty slices.

## Verification snapshot

- Focused Windows seam: 10 passed.
- Full credential suite: 119 passed, 1 pre-existing native-keychain smoke
  ignored.
- Locked Linux `cloud` check and strict all-target Clippy: passed.
- Credential contract, generator launcher, formatting, and fixture hygiene
  checks: passed.
- Repository hygiene scan still reports the same six findings in pre-existing
  Seeds/design-review material; none is in this slice.
- The full Windows application check remains blocked by missing MSVC `lib.exe`.
  The ignored native Windows smoke must be run on release hardware before this
  detector can contribute positive evidence.
