# audio-graph-cc9a production qualification plan

## Acceptance and custody

Seed `audio-graph-cc9a` authorizes one production path from an existing exact
managed Session artifact root to an opaque filesystem qualification paired
with `CanonicalDurability`. The qualification must bind the canonical root,
live mount, filesystem volume, and directory object; it must not trust a
caller-provided boolean or filesystem string. Qualified guard acquisition must
revalidate that binding before it opens or creates the coordination entry.

Execution is fixed to base `d31b5f9695164452a6c353b8230097fd8f661119`
on branch `work/audio-graph-cc9a-production-qualification-wave7c` in the clean
worktree
`/home/codeseys/DevBox/audio-graph/.worktrees/cc9a-production-qualification-wave7c`.
The parent `audio-graph-7e81` stays open and runtime-dark; cc9a supplies only
its missing production qualification prerequisite.

## Vertical RED/GREEN slices

1. Add deterministic public-policy tests around
   `CanonicalFilesystemQualification::for_existing_managed_root` (or an
   equivalently narrow honest seam): longest matching mount, Linux ext4 and
   macOS APFS admission, and typed refusal for unsupported filesystem class,
   read-only, removable, no matching mount, unavailable identity, Windows,
   and Other. Capture compile/behavior RED before the minimum GREEN.
2. Add public guard-acquisition tests proving the qualified pair refuses a
   foreign root, ancestor/nested-root mismatch, and moved/recreated root before
   coordination-file mutation. Capture RED before binding validation moves in
   front of coordination-file open/create.
3. Add `SessionArtifactManifestStore::qualified_existing_root` tests proving a
   qualified initial CAS reports `Accepted` only with the exact
   `FileAndParentNamespace` barrier while unqualified production storage stays
   refusal-only. Capture RED before construction and initial CAS GREEN.
4. Add qualified replacement tests proving generation replacement retains the
   exact validated open head and returns `Accepted` only after file and parent
   barriers. Preserve existing replacement-race refusal. Capture RED before
   the final GREEN.
5. Audit `Debug` and all new errors with sentinel root, mount, source, id, and
   byte content. Only a small stable filesystem-class enum may cross the error
   boundary; raw filesystem strings and paths may not.

The deterministic inventory is a private test injection into the same policy
function production uses. Production construction always obtains a fresh
`sysinfo 0.39` `Disks` list, chooses the longest path-prefix mount containing
the canonical root, and independently binds native volume/object identity. No
test/synthetic qualification or algorithm-environment constructor is callable
from production.

## Platform policy

- Linux: admit only `ext4`, writable, and non-removable.
- macOS: admit only `APFS`, writable, and non-removable.
- Windows and Other: typed namespace unsupported before any coordination or
  manifest mutation.
- Unknown/local-unclassified, network/remote, FUSE, tmpfs, other filesystem,
  read-only, removable, no-match, and unavailable-identity observations:
  typed refusal before mutation.
- Longest matching live mount wins; a parent/ancestor observation cannot
  override a more specific nested mount.

## Ownership and hard stops

Implementation owns only the two persistence modules and the three cc9a
documents named in the commit-state. It does not change Seeds, workflows,
dependencies/lockfiles, frontend, generated files, canonical log/crash code,
Session writers/consumers, or any other path. It does not add unsafe, a fifth
stream, custom-root expansion, a caller-authority boolean/string, synthetic
production qualification, or b887 activation.

Stop if sysinfo cannot support a safe longest-mount allowlist that remains
bound to the exact canonical root plus native volume/object identity without a
dependency, workflow, unsafe, or out-of-scope change.

## Gates and evidence limits

During TDD, run focused `canonical_durability`,
`session_artifact_manifest`, and `session_semantics` cloud library tests plus
locked cloud check as needed. Final candidate gates are:

- locked cloud `cargo check`;
- one serialized full cloud library suite;
- strict cloud Clippy with `-D warnings` and rustfmt `--check`;
- frontend typecheck;
- repository-authoritative `verify:fast` and all five contracts;
- Betterleaks and docs/Seeds secret hygiene;
- `git diff --check`, exact base footprint, forbidden-path, and runtime-dark
  searches.

The current fixture resolves to writable ext4 and can run the native-Linux
live qualification test. The prior Linux/ext4 and macOS/APFS protocol runs are
accepted context, not new-constructor evidence. No native macOS or Windows
execution is available locally, and the report will not claim either.

## Rollback and handoff

Commit planning, implementation, and final evidence separately with
`audio-graph-cc9a` in each message. The dedicated branch is the rollback unit;
the conductor owns review, integration, Seed reconciliation, push/merge, and
worktree cleanup.
