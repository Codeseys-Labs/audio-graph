# Linux Credential Filesystem Detector Worktree State

Date: 2026-08-01

Seed: `audio-graph-e917`

Parent Seed: `audio-graph-e241`

Branch: `work/audio-graph-e917-linux-filesystem-detector`

Base: `180f8595c48f4d8e2567bdeaafe644dd633b3e19`

Worktree:
`/home/codeseys/DevBox/audio-graph/.worktrees/e917-linux-filesystem-detector`

## Custody

- This clean worktree is the only write surface for this Seed.
- Owned production scope is the Linux adapter, its Linux-only module
  declaration, the exact target-Linux `rustix` dependency and necessary
  lockfile delta, focused tests, and this record.
- Windows and macOS adapters belong to sibling worktrees. Their module and
  manifest assembly must remain independently mergeable.
- No persistence primitive, profile grant, IPC, frontend, workflow, main
  checkout, Seed, remote, or integration change belongs to this branch.

## Accepted contract

Implement the Linux target-native seam accepted by ADR-0035 and research Seeds
`audio-graph-c7b2` and `audio-graph-b686`:

- open and retain one directory fd with `openat` and `NOFOLLOW`;
- bind `fstat`, `fstatfs`, and `statx(AT_EMPTY_PATH, STATX_MNT_ID)` to that fd;
- join exactly one bounded `/proc/self/mountinfo` row by mount id and require
  its major/minor identity to agree with the fd;
- classify exact ext-family magic plus exact `ext4` type as the first Linux
  candidate, while recognizing btrfs and XFS without authorizing them;
- traverse a bounded, read-only kernel block topology keyed by device number;
- require the generic driver-core `removable` attribute to be present with the
  exact value `fixed` on the discovered PCI ancestry; the block layer's
  numeric `removable=0` is necessary but cannot provide that proof;
- re-sample fd and topology identity after all secondary queries; and
- reduce every result to the common closed observation or detector fault.

Exact ext4 on a writable, local, kernel-native, recognized fixed topology is
candidate-only. Both compiled evidence tables remain literal empty slices, so
even a favorable observation remains `DurabilityUnproved`.

## Confirmed seams

- `parse_mountinfo` consumes bounded untrusted bytes and returns only closed
  mount identity, writability, and filesystem class.
- `classify_topology` consumes a bounded graph of closed node facts plus exact
  adapter-private identity and returns fixed, denied, or unknown. Raw sysfs
  identity participates only in equality and never enters the observation.
- `classify_facts` combines initial/final handle identity, mount facts,
  filesystem magic, and topology into `FilesystemObservation`.
- `LinuxHeldTarget` owns the descriptor; `LinuxFilesystemDetector` is the
  native acquisition adapter behind the existing `FilesystemDetector` trait.
- The native probe opens only the current test directory and reads metadata;
  it never creates product content or exports raw native values.

## Bounds and denial posture

- Mountinfo: 1 MiB total, 4,096 records, 16 KiB per record, 128 fields.
- Kernel topology: 32 nodes, depth 16, 64 directory entries per enumeration,
  4 KiB per attribute/path value, and 256 KiB total retained private identity.
- Zero/duplicate mount rows, malformed or oversized input, mount/device
  disagreement, unknown fields required for a favorable result, cycles,
  multiple backers, missing ancestry, topology change, and all bound overruns
  fail closed.
- Remote filesystems, FUSE, overlay, read-only views, removable/hot-plug
  media, USB, FireWire, Thunderbolt, MMC/SD, virtual/network block transports,
  and loop devices never become candidates.
- A missing, `unknown`, `removable`, malformed, or oversized generic device
  removability attribute fails closed. A later USB/Thunderbolt/hot-plug
  ancestor overrides any `fixed` PCI descendant, and both topology snapshots
  must retain the same closed fixed result.
- No longest-path matching, shell-out, `sysinfo` authority, vendor/path
  heuristic, raw path/source/device/native prose export, or favorable missing
  data is permitted.

## Review correction: exact topology identity

The first implementation reduced each block node to device number, backers,
block removability, and a closed ancestry class. That was insufficient for the
identity-stability claim: a different PCI/sysfs chain reusing the same device
number and the same `FixedPciNvme` class could compare equal.

Each topology node now retains the exact bounded canonical sysfs path bytes and
exact stat device/inode/ctime metadata. Physical leaves also retain the ordered
exact identity sequence for every visited ancestor. These values are
adapter-private, equality-bearing, non-serializable, and use custom Debug that
emits only `<redacted>`. `HandleIdentity` likewise has a fully redacted custom
Debug implementation.

Within one node snapshot, the adapter independently acquires two complete
samples around a second `/sys/dev/block/<device>` canonicalization. Both
samples contain the node identity, device number, block-removable value,
sorted backers, loop state, and full physical-ancestor result/identity. Any
change fails closed as ambiguous. The outer initial/final topology snapshots
then compare the retained private identities as part of `TopologyGraph`
equality.

The TDD regression began RED with missing private identity types/field. It is
GREEN with identical device/backer/removable/`FixedPciNvme` closed facts but
different private node/ancestor canaries: `identity_stable = No`, and the
evaluator returns `TargetChanged`. Separate deterministic coverage changes
removable, backers, loop state, node identity, and ancestor identity one at a
time; tests also enforce the identity budget and prove path/stat/handle
canaries cannot escape through Debug or serialized status.

## Kernel evidence and availability limit

Sources checked 2026-08-01:

- Linux's [device-driver infrastructure documentation](https://docs.kernel.org/driver-api/infrastructure.html#c.device_removable)
  defines `DEVICE_FIXED` as a device not removable by the user, while the
  default is `DEVICE_REMOVABLE_NOT_SUPPORTED`.
- Driver core [emits only `fixed`, `removable`, or `unknown`](https://github.com/torvalds/linux/blob/master/drivers/base/core.c)
  and creates the generic sysfs attribute only when
  `dev_removable_is_valid()` is true.
- Upstream commit
  [`70f400d4d957c2453c8689552ff212bc59f88938`](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/commit/?id=70f400d4d957c2453c8689552ff212bc59f88938)
  made the attribute generic and explicitly opt-in. It landed in Linux 5.14;
  ordinary PCI storage drivers may still omit it.

Therefore this first detector deliberately has sparse positive host support.
Absence never falls back to PCI path shape, a vendor heuristic, or numeric
block `removable=0`; it returns an unproved inspection result. This branch does
not claim that common PCI NVMe/SATA/SCSI hosts already expose `DEVICE_FIXED`.

## Linux cloud-boundary-v1 meaning

For a positive ext4 candidate, `os_managed_cloud_root = No` means only that the
bounded Linux allowlist observed no kernel-visible remote, userspace,
overlay/provider, removable, hot-plug, or otherwise denied storage layer at
qualification time. This is the accepted cloud-boundary-v1 statement; it is
not proof that the target is unreplicated or that Dropbox-like watchers,
backup tools, indexers, EDR agents, administrative software, user scripts, or
ordinary copiers are absent or unable to read/copy a later file.

`FilesystemObservation` is backend-private and is not serializable. The common
evaluator consumes the cloud-boundary field only as one conservative denial
gate; it does not copy the field into `FilesystemStatus`. The serializable
status contains only target, closed status code, optional family, and detector
schema. The Linux favorable fixture asserts that exact shape and still returns
`DurabilityUnproved`; both evidence tables are empty. Consequently this branch
has no serialized status or support result that can claim "not cloud-synced",
"no backup", "no watcher", or native-store equivalence.

## Verification

Cargo output uses the shared `/tmp/audio-graph-target-dccd` cache and Rust
1.95.0. The final worktree state passed:

- focused Linux detector/native suite: 21 passed;
- locked cloud credential suite: 130 passed, 1 ignored;
- locked cloud `cargo check`;
- strict locked cloud Clippy with all targets and `-D warnings`;
- final default-feature credential suite: 130 passed, 1 ignored;
- Rust formatting and generated credential-contract drift checks; and
- secret-hygiene fixture self-test.

One local, closed-value-only diagnostic run (temporary instrumentation removed
before the final gates) observed the current WSL worktree as Linux 6.18.33,
ext4, writable/local/kernel-native, identity-stable, and
`internal_fixed = No`. The independently observed device ancestry contains
VMBus, so that exact result is expected. It is local evidence, not a portable
test expectation: the committed native smoke remains host-agnostic, while the
deterministic VMBus, missing-attribute, hot-plug-rescue, and two-snapshot
fixtures carry the fail-closed contract.

The repository hygiene scan retained exactly the accepted six pre-existing
findings and added no owned-file finding. Exact commands and outputs are
recorded in
`/tmp/audio-graph-artifacts/2026-08-01/audio-graph-e917-implementation.md`.
