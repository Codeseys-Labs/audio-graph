# audio-graph-8e73 Wave 7B durability plan

Date: 2026-08-14

Parent Seed: `audio-graph-8e73`

Implementation base: `c7d3fec2db8c60629e0b7c8b93e752c3aee85368`

Research source: commit `f44407ece305396ac91f65e4c277084603d16ddd`,
`docs/research/canonical-directory-durability-2026-08-14.md`

Governing decisions: [ADR-0027](../../adr/0027-file-canonical-durable-session-store.md)
and [ADR-0037](../../adr/0037-freeze-canonical-event-stream-registry.md)

## Outcome and done boundary

`audio-graph-8e73` is complete only when the dormant canonical durability stack
distinguishes existing-file from namespace-changing operations, registers
quarantine state before destructive recovery, owns recovery through one stable
lock and verified source handle, reconciles every subprocess crash cut
idempotently, and records platform-correct Linux, macOS, and Windows evidence.

The accepted platform-result boundary is deliberately asymmetric:

- a qualified Linux local filesystem may return `Accepted` for first-create or
  rename only after both the regular-file sync and every required parent
  directory sync succeed;
- macOS may return `Accepted` for namespace-changing operations only after an
  executed supported-host APFS probe proves the parent-directory barrier; and
- Windows first-create and rename return
  `NamespaceDurabilityUnsupported`. A successful file flush, ordinary restart,
  or rename does not upgrade that result.

The parent closes only after all seven children are integrated, the complete
three-platform result evidence is recorded, and no non-test production caller
exists. A subprocess kill/reopen matrix proves process-crash recovery and
completed OS barriers; it is never described as power-loss proof.

## Evidence and lineage boundary

The strict reader at `src-tauri/src/persistence/canonical_reader.rs` already
hard-codes ADR-0037's four stream identifiers and outer schema-v1 mappings and
cites ADR-0037 for the data-movement inner/outer version assertion. Before D0,
the accepted ADR file and its index entry were absent.

D0 restores ADR-0037 byte-for-byte from non-ancestor commit `5487c3b`; its
required Git blob is `6f94d5a9fb183afbef70826add08fc3c1f163f59`.
The ADR's historical references to ADR-0035 and ADR-0036 name non-ancestor
canonical-log framing and recovery decisions. They do not name the current
accepted speech-semantics decisions:

- current ADR-0035 blob:
  `91ffb0304c06323be6254889d716e639ebc4d79e`;
- current ADR-0036 blob:
  `3af2bcfeafe14d01544b4f122c10b8df78335fe2`.

Those current files remain byte-for-byte unchanged. This lineage clarification
belongs to this plan and the D0 report, not to the immutable historical ADR
blob. Discovery reports were message-only and read-only; the authoritative
planning evidence is the current code, exact Seed records, integration
checkpoint/report, and the reviewed primary-source research artifact.

## Dependency graph and concurrency

The exact child order is:

```text
D0  audio-graph-1189
         |
         +-------------------+
         v                   v
D1  audio-graph-c2e3    M0  audio-graph-661f
         +-------------------+
                   |
                   v
             M1  audio-graph-a596
                   |
                   v
             R1  audio-graph-3b8b
                   |
                   v
             T1  audio-graph-b77b
                   |
                   v
            CI1  audio-graph-2df3
                   |
                   v
             parent audio-graph-8e73
```

Only D1 and M0 run in parallel. The parent WIP cap is two and the independent
review/fix-round cap is two. Every implementation uses a dedicated clean
worktree, commits a stable tip and evidence report, and receives Standards and
Spec review before integrator-only fan-in. The integrator re-runs assembled
gates; a worktree-green result is not integration acceptance.

## D0 — `audio-graph-1189`: restore the accepted registry lineage

Ownership is limited to the exact ADR-0037 blob, the current-format README
index entry, this plan, and the D0 report. There is no product, Seeds,
workflow, package, generated-file, or other-ADR ownership.

The RED proof is absence of the ADR file and index row while the current strict
reader cites ADR-0037. GREEN requires the exact historical blob, a linked
accepted index row dated 2026-07-10, unchanged current ADR-0035/0036 blobs,
resolving relative links, and a base-range footprint limited to the four owned
files.

Stop if the source blob differs, current ADR-0035/0036 changes, any lineage
clarification would require editing the archival ADR, or any product/Seeds/
workflow mutation appears. Rollback is branch disposal or reversal of this
documentation-only commit before fan-in; it has no runtime or persisted-data
effect. D0 does not close a Seed, merge, push, dispatch a workflow, or start a
Blacksmith job.

## Wave 1, parallel after D0

### D1 — `audio-graph-c2e3`: named file and namespace durability substrate

Ownership is the new persistence `canonical_durability` module, its persistence
module declaration, the narrow `canonical_log` integration needed to consume
it, focused tests, and the workstream report. The module atomically
distinguishes existing and newly created files without a `Path::exists`
preflight; exposes content-free typed stages/outcomes and stable cooperative
coordination locks; flushes then file-syncs existing appends; requires a parent
barrier for a qualified Linux first-create; conditionally accepts macOS only
after APFS proof; refuses Windows first-create and rename as
`NamespaceDurabilityUnsupported`; and returns
`ParentProvisioningRequired` when the parent is absent.

Focused gates cover existing versus first-create races, every barrier failure,
typed/content-free diagnostics, lock contention/release, qualified and refused
filesystem outcomes, and unchanged strict non-mutating reads. Run the focused
Rust tests, locked cloud check, strict Clippy, rustfmt, contracts, pinned
`verify:fast`, secret hygiene, Betterleaks, and range diff.

Stop lines: no runtime caller, manifest or destructive recovery, Session
semantics floor, prompt, adapter, UI, or workflow edit. Because the substrate
is dormant, rollback removes its isolated module/integration commit without an
on-disk migration.

### M0 — `audio-graph-661f`: select and crash-model the manifest transaction

Ownership is a throwaway executable finite model plus its design/report
artifacts. Compare a versioned atomic snapshot, append-only manifest log, and
log-plus-materialized-view across prepare, quarantine publish, manifest
acceptance, source truncate, completion, restart, generation conflict, exact
residual state, and idempotent retry. The expected candidate is a versioned
atomic snapshot with generation compare-and-swap, but the executable evidence
selects the form.

Gates are syntax validation, the complete finite-model command with case,
transition, state, assertion, and invariant counts, explicit crash cuts at
every transition, deterministic restart/retry, exact residual-state checks,
secret hygiene, Betterleaks, and range diff.

Stop if the selected form is an event stream: create a new ADR proposal and
backflow an ADR-0037 registry update before any kernel implementation. Do not
silently add a fifth canonical stream. No production code or consumer
migration is allowed. Rollback discards the non-production model/design branch
and changes no runtime or persisted state.

## Wave 2 — `audio-graph-a596`: persisted typed Session Artifact manifest kernel

Starts only after D1 and M0 are integrated. Ownership is the dormant manifest
kernel and focused tests: versioned `SessionArtifactManifestV1`, typed artifact
entries, privacy and availability classes, stable relative managed identity,
hashes, lengths, source identity, generation-checked compare-and-swap, and
prepared/completed/unavailable/residual quarantine states. Manifest persistence
must consume D1 and cannot report `Accepted` when namespace durability is
unsupported.

Focused gates cover strict load, schema/type validation, generation conflict,
crash/reopen idempotence, prepared/completed/unavailable residual states,
unsupported namespace outcomes, and quarantine deletion parity. Run the
focused Rust tests and the common locked Rust, contracts, pinned
`verify:fast`, security, and diff gates.

Stop lines: no runtime consumer activation and no broad export, delete, purge,
backup, or recovery adoption owned by parent `audio-graph-be7c`. Rollback
removes the dormant kernel before adoption; no production manifest has been
written.

## Wave 3 — `audio-graph-3b8b`: locked manifest-transactional tail recovery

Starts only after D1 and M1 are integrated. Ownership is the dormant
`CanonicalRecoveryTransaction`, its focused recovery tests, and its report.
It owns one stable exclusive coordination guard and one identity-verified
source handle. The caller supplies the attempted-event recovery descriptor.
The enforced order is quarantine temp, file sync, rename, namespace barrier,
manifest prepare, same-handle source truncate and sync, manifest completion,
then acknowledgement. In-memory quarantine receipts are not authority.

Focused gates inject failure at every ordered stage, preserve each recoverable
copy, assert typed/content-free failures, refuse identity change and lock loss,
and prove strict readers remain non-mutating. Run the focused Rust tests and
the common locked Rust, contracts, pinned `verify:fast`, security, and diff
gates.

Stop lines: no command/state writer, prompt, provider adapter, UI, runtime
caller, Session semantics floor, or workflow edit. Runtime persistence remains
owned by `audio-graph-90f3` and `audio-graph-3b48`. Rollback removes the
dormant transaction before runtime adoption.

## Wave 4 — `audio-graph-b77b`: subprocess crash and exclusion proof

Starts only after R1 is integrated. Ownership is a non-production subprocess
and cross-process test harness, fixtures, and report. Child/parent handshakes
and parent kills cover before and after write, userspace flush, file sync,
new-entry sync, quarantine rename, manifest prepare sync, source truncate,
source sync, completion, and acknowledgement. Fresh-process reopen must
converge idempotently.

The exclusion matrix covers exclusive/exclusive, shared/exclusive,
release-on-process-death, readers, rename behavior, stable-lock semantics, and
the explicit uncooperative-process limitation. A qualified Linux local
filesystem passes locally or in a compatible disposable Linux Blacksmith
Testbox. The exact harness command and each process exit/status are recorded.

Stop lines: no production activation, power-loss claim, workflow edit, or
silent relaxation for an unsupported filesystem. Rollback removes only the
test harness/report. If a Testbox is used, its cleanup is part of the gate, not
a later courtesy.

## Wave 5 — `audio-graph-2df3`: macOS and Windows qualification

Starts only after T1 is integrated. Ownership is external evidence and its
report; it does not own workflow files. Run the accepted, non-ignored canonical
durability and subprocess matrix on macOS 15/APFS and Windows 2025/NTFS, while
keeping the Linux matrix green.

For every run, record runner OS, filesystem, workflow/job or Testbox ID, exact
command, status, exit, and durable log location. macOS must either prove the
APFS parent-directory `sync_all` barrier or retain the typed conditional
refusal. Windows must assert `NamespaceDurabilityUnsupported` for first-create
and rename while still exercising file barriers, locks, readers, renames, and
every crash cut. A passing restart test cannot change the Windows result.

Remain `BLOCKED_CI` if the evidence cannot be obtained. Do not weaken, skip, or
simulate the platform claim and do not mutate a workflow without separate
approval. Rollback is evidence-only: no product result changes until qualified
evidence is integrated.

## Blacksmith execution policy

- Use a disposable Testbox only when Blacksmith supports the requested OS and
  a compatible existing workflow/command shape.
- T1 may use a compatible Linux Testbox. CI1 may use Testboxes for macOS or
  Windows only when those requested OS/workflow combinations are supported.
- Otherwise, existing `blacksmith-6vcpu-macos-15` and
  `blacksmith-4vcpu-windows-2025` Actions jobs are the macOS and Windows
  platform authorities. Do not substitute Linux evidence.
- Capture every monitor/status command and output, Testbox or job ID, executed
  test command, status transition, exit, and log location.
- Explicitly stop every disposable Testbox, then run the provider's active-list
  command and record a clean list. A missing stop or list-clean result fails
  the evidence gate.
- Do not create, edit, or dispatch a workflow without separate authorization.

## Common branch and integration gates

Each child starts with a public-seam RED and ends with its focused GREEN. Every
code-bearing branch also runs Rust 1.95 with `--locked`, relevant cloud-only
focused tests, locked check, strict Clippy with warnings denied, and rustfmt.
Run all five generated-contract drift checks, or prove they ran inside the
pinned fast gate. Every branch runs:

```text
SEEDS_CLI_ROOT=$PWD/node_modules/@os-eco/seeds-cli bun run verify:fast
bun run verify:contracts
bun scripts/check-docs-secret-hygiene.mjs
betterleaks dir --no-banner --redact <owned files and report>
git diff --check <exact-base>...HEAD
```

The integrator validates the merge-base footprint, refuses unexpected Seeds,
workflow, dependency, generated, credential, build-output, or unrelated ADR
paths, reviews placeholder traps, and re-runs the assembled focused and common
gates. Seeds remain conductor-owned and close only after integrated acceptance
evidence. No child push, merge, workflow dispatch, deployment, or release is
authorized by this plan.

## Parent rollback and stop conditions

Before runtime adoption, every Wave 7B code slice is dormant and individually
reversible. Stop and backflow rather than guess when a platform barrier is
unsupported, APFS proof is absent, an event-stream manifest is selected, a
recovery state cannot reconcile idempotently, a lock loses object identity, a
workflow change appears necessary, or a child exceeds its ownership.

The parent must not introduce the Session semantics floor, receipt-bearing
runtime writer, projection prompt/scheduler behavior, adapter/UI changes, or a
non-test production caller. Those remain downstream of completed and
integrated `audio-graph-8e73` evidence.
