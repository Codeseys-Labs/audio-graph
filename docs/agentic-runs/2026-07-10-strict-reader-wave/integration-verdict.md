# B481 integration verdict and cleanup proof

Date: 2026-07-10

Seed: `audio-graph-b481`

## Verdict

The accepted ADR slice and reviewed canonical-log kernel are integrated in one
linear history and satisfy b481's bounded acceptance criteria. B481 may close;
this verdict does not authorize a runtime writer or claim the separate
directory/manifest/subprocess guarantees owned by `audio-graph-8e73`.

## Integrated history

```text
7b0e5d003dcc23c971561c85fe5a5a57dc6920ed feat(persistence): harden canonical log v1 kernel
1d1c7cc157a3b4bd250119be6344ace29fca662e docs(adr): record MVP decisions through ADR-0036
f97e19c251e4c227aade1289b2aba56e0d40ffca baseline
```

The history has two commits beyond the baseline and zero merge commits. The
ADR commit is an ancestor of the kernel commit.

## Integrated-tree gates

| Gate | Result |
|---|---|
| `cargo +1.95.0 fmt --all -- --check` | passed |
| `cargo +1.95.0 metadata --locked --format-version 1 --no-deps` | passed |
| focused locked canonical-log test suite | 23 passed, 0 failed, 1,451 filtered; 0.25 s assertions after 2 m 43 s build |
| strict locked library Clippy with `-D warnings` | passed in 36.23 s |
| `git diff --check` and clean worktree | passed |

Rust gates used one build job, disabled incremental compilation, embedded the
Windows test manifest, and used `lld-link`, matching the reviewed local gate.

## Cleanup proof

- Correctness, durability, and integration-scout branches have no commits
  beyond the common base.
- Their five substantive reports compare byte-for-byte after line-ending
  normalization with the copies committed under the canonical durability run.
- The detached rsac worktree's lockfile is byte-identical. Its manifest pin,
  capture capability field, and lockfile are all present in the kernel commit;
  remaining differences are only the canonical test dependency, formatting,
  and stronger lockfile comment.
- The ADR branch commit is retained as an ancestor of the kernel branch, so its
  separate worktree and branch name are no longer required.
- Missing historical worktree registrations were pruned before live worktree
  removal.

## Successor boundary

`audio-graph-6896` may now begin with strict, non-mutating mixed-format readers.
`audio-graph-8e73` remains open for directory-entry barriers, one-handle repair,
manifest-first quarantine, file identity, and subprocess crash proof. Neither
successor may add a runtime canonical writer during the reader wave.
