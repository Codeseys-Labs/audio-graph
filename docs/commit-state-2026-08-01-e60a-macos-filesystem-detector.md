# Commit state: macOS credential filesystem detector

Date: 2026-08-01

Seed: `audio-graph-e60a`

## Starting state

- Worktree: `.worktrees/e60a-macos-filesystem-detector`
- Branch: `work/audio-graph-e60a-macos-filesystem-detector`
- Base/starting HEAD: `180f8595c48f4d8e2567bdeaafe644dd633b3e19`
- Starting worktree: clean
- Queue state: the root conductor marked `audio-graph-e60a` in progress; this
  worker does not mutate shared Seeds state.

## Scope and custody

This slice owns only the macOS detector module, its macOS-only declaration,
the exact target-macOS direct dependencies and necessary lockfile delta, and
focused deterministic tests. Windows and Linux detector slices are being built
in sibling repo-local worktrees; this branch must keep the shared declaration
and manifest delta mechanically mergeable.

The accepted evidence tables remain literal empty slices. This slice does not
enable a filesystem profile, persistence backend, IPC surface, renderer flow,
workflow, or release claim.

## Known verification boundary

No Apple Rust target is installed in this environment. At gate time,
`rustup target list --installed` reported the Linux and Windows x86-64 targets
only. The pure Foundation/File Provider result seam is host-tested, and exact
locked macOS dependency source was inspected. A macOS target compile and the
signed packaged APFS/File Provider matrix remain required under
`audio-graph-0e08`; this worktree did not install or alter global toolchains.

## Implemented state

- The adapter retains one `NOFOLLOW` directory descriptor, takes initial and
  final `fstat`/`fstatfs` snapshots, and compares them with a descriptor
  reopened from the transient `F_GETPATH`/`NSURL` bridge.
- Darwin mount flags and Foundation URL resource values are immediately
  reduced to closed enums and ternaries. Paths, native errors, filesystem
  strings, provider identifiers, and native numeric codes are not returned.
- The production adapter can emit only `UnprovedNotManaged` for the File
  Provider boundary. The favorable `ProvedNotManaged` fixture variant is
  compiled only in tests, so `isUbiquitous=false` cannot create a favorable
  cloud-root result at runtime before `audio-graph-0e08`.
- Both compiled evidence profile slices remain literal empty slices. This
  branch grants no journal or file-v2 support profile.

## Verification snapshot

- 22 focused filesystem-policy tests passed in the locked cloud build.
- Locked Linux cloud `cargo check`, library Clippy, test Clippy with warnings
  denied, and Rust formatting passed.
- Credential/runtime generated-contract drift checks and generator-launcher
  checks passed. The docs/Seeds scanner fixture passed; the normal scan retained
  its pre-existing six findings and this branch introduced none.
- The macOS target command was attempted and stopped at `E0463` because the
  target standard library is absent. This is not macOS compile evidence.
