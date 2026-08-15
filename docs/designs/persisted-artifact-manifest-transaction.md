# Persisted Session Artifact manifest transaction

Date: 2026-08-15

Seed: `audio-graph-661f`

Status: selected design; production implementation remains owned by
`audio-graph-a596`

Prototype branch: `prototype/audio-graph-661f-manifest-model-wave7b`

Corrected prototype tip: `88849b89cea3aaf476ffcf5fdd98029a4f095822`

Governing decisions: [ADR-0027](../adr/0027-file-canonical-durable-session-store.md)
and [ADR-0037](../adr/0037-freeze-canonical-event-stream-registry.md)

Platform boundary: [canonical directory durability research](../research/canonical-directory-durability-2026-08-14.md)
and the [Wave 7B plan](../agentic-runs/2026-08-14-session-memory-continuation/audio-graph-8e73-wave7b-plan.md)

## Verdict

Persist the typed Session Artifact manifest as **one versioned atomic snapshot
with generation compare-and-swap (CAS)** under the stable session coordination
lock.

The manifest is current typed state, not an append-only event domain. Its two
transaction states are `Prepared` and `Completed`; exact residual information
is data in that state, not a second authority. Each successful state change
increments `generation` exactly once. A candidate snapshot is synchronized,
its expected generation is checked while the coordination lock is held, it is
atomically installed in the manifest directory, and the qualified parent
namespace barrier completes before that manifest transition returns
`Accepted`.

This selection does not add a fifth canonical stream and therefore does not
trigger ADR-0037 backflow. Selecting either log candidate would require a new
ADR and registry update before implementation.

## Question and evidence

The throwaway model is preserved outside integration history on branch
`prototype/audio-graph-661f-manifest-model-wave7b` at corrected tip
`88849b89cea3aaf476ffcf5fdd98029a4f095822`. Reproduce the exact evidence from
that branch with:

```text
git switch prototype/audio-graph-661f-manifest-model-wave7b
test "$(git rev-parse HEAD)" = 88849b89cea3aaf476ffcf5fdd98029a4f095822
node --check scripts/prototypes/manifest-transaction-crash-model.mjs
bun scripts/prototypes/manifest-transaction-crash-model.mjs
```

The integration-ready branch starts directly at the assigned base and never
contains either prototype commit in its ancestry. The runnable script is
therefore durable through its dedicated branch ref without shipping throwaway
code in integration history. The exact source remains inspectable from any
branch with:

```text
git show 88849b89cea3aaf476ffcf5fdd98029a4f095822:scripts/prototypes/manifest-transaction-crash-model.mjs
```

The finite model compared the snapshot, append-only log, and
log-plus-materialized-view forms under the same successful transaction,
ten crash cuts, visible or possibly visible faults, unsupported namespace
capability, restart, exact retry, generation conflict, idempotency conflict,
and cooperating concurrent-writer cases. All three candidates can be made
crash-correct under the modeled protocol; physical simplicity and the accepted
canonical-stream boundary select the snapshot.

The corrected run explored 124 cases and 1,158 reducer transitions, observed
411 unique full states, and passed 22,121 assertions across 48 invariant
families. The exact case breakdown was:

| Model group | Cases |
| --- | ---: |
| Unexpected completion-generation regression | 3 |
| Successful form | 3 |
| Crash cut | 46 |
| Visible or possibly visible failure | 54 |
| Namespace refusal / late discovery | 6 |
| Concurrent writer | 12 |
| **Total** | **124** |

Representation-specific physical outcomes make the log and hybrid case counts
slightly larger; every candidate still traverses the same ten logical cuts and
safety assertions.

## Selected physical contract

### Snapshot identity and CAS

The durable manifest root must carry at least:

- a manifest schema version;
- a monotonically increasing `generation`;
- the typed artifact inventory required by ADR-0027;
- a stable transaction/idempotency identity and fingerprint;
- a `Prepared` or `Completed` transaction phase;
- a stable relative quarantine identity;
- the verified source identity and original/target lengths needed to classify
  residual state; and
- typed privacy, availability, and residual information owned by
  `audio-graph-a596`.

One transaction object owns the stable coordination lock once. It loads
generation `g`, and a `Prepared` replacement may install only if the current
generation is still `g`. The `Completed` replacement may install only if the
current snapshot is the exact same transaction at generation `g + 1` in
`Prepared`. Same transaction id and fingerprint at any other generation is
still `GenerationConflict`, and completion performs no mutation. Each install
increments exactly once. A stale different writer
receives `GenerationConflict { expected, actual }`; the snapshot is not
changed. The coordination lock serializes cooperating writers, while CAS also
detects a stale caller or an unexpected head. Neither mechanism claims safety
against an uncooperative process outside the lock contract.

### Ordered transaction

The physical order is:

1. Preflight platform/filesystem namespace capability before any mutation,
   acquire the stable coordination lock, verify the source handle/identity,
   and validate the expected manifest generation and exact retry identity.
2. Prepare the quarantine temp in the final quarantine directory, write the
   exact recovery bytes, and synchronize that file.
3. Publish it to a collision-refusing unique final quarantine name by an
   atomic same-filesystem rename.
4. Complete the quarantine parent-directory barrier. Until this succeeds, the
   name is visible or possibly visible but not durably registered.
5. Persist snapshot generation `g + 1` with the transaction in `Prepared`.
   Only the synchronized replacement plus qualified manifest-directory barrier
   makes this manifest transition `Accepted`.
6. Truncate the still-locked, identity-verified source handle to the validated
   prefix.
7. Synchronize that same source handle.
8. Persist snapshot generation `g + 2` with the exact transaction in
   `Completed`, again including the replacement and manifest-directory
   barriers.
9. Establish the pre-acknowledgement state only after `Completed` is durable,
   then publish acknowledgement.

The direct invariant is:

```text
durable quarantine namespace
  -> durable Prepared snapshot
  -> source truncate and source sync
  -> durable Completed snapshot
  -> acknowledgement
```

No source truncate call is legal merely because quarantine bytes or a rename
are visible. No acknowledgement is legal merely because source truncation is
visible.

### Platform refusal

`NamespaceDurabilityUnsupported` is a valid no-mutation result only when the
capability preflight completes before preparing a temp. If missing capability
is discovered after any visible or possibly visible create, write, rename, or
replacement, the result is `DurabilityIndeterminate`, not the safe preflight
refusal.

The selected snapshot requires a qualified parent-directory barrier for both
the quarantine publication and each manifest replacement. Therefore it cannot
return canonical `Accepted` on the current Windows namespace contract. macOS
remains conditional on the APFS directory probe. A qualified supported Linux
local filesystem may accept only after all file and directory barriers pass.
The snapshot form does not weaken the platform outcome defined by the
durability research.

A prepare failure proven to occur before any file or namespace can be visible
is `IoFailedBeforeAcceptance` and creates no recovery uncertainty. A partial or
exact temp, a possibly visible publish, or any later mutation failure remains
`DurabilityIndeterminate`. The typed result boundary depends on proof of
visibility, not merely on the operation name.

## Restart, residuals, and exact retry

Restart treats the installed snapshot as the only manifest authority. An
uncommitted replacement temp is never promoted by filename inference: preserve
or quarantine its content-free identity, load either the old or the new valid
snapshot, and reconcile from the exact installed generation.

The modeled residual states are:

| Installed manifest | Source after reopen | Quarantine observation | Exact state / next action |
| --- | ---: | --- | --- |
| `Absent` | full | missing | `CleanSourceFull`; exact retry may prepare again |
| `Absent` | full | exact temp | `TempOnlySourceFull`; publish and barrier it |
| `Absent` | full | partial temp | `PartialTempSourceFull`; preserve/remove under lock, then recreate exact temp |
| `Absent` | full | final, namespace unproven | `PublishedNamespaceUncertainSourceFull`; repeat a qualified barrier before manifest acceptance |
| `Absent` | full | final and namespace durable | `QuarantineOnlySourceFull`; persist `Prepared` |
| `Prepared` | full | final and namespace durable | `PreparedSourceFull`; truncate and sync the source |
| `Prepared` | target | final and namespace durable | `PreparedSourceTruncated`; persist `Completed` without truncating again |
| `Completed` | target | final and namespace durable | `CompletedSourceTruncated`; return `AlreadyCompleted` for the exact retry |

An exact retry must match both transaction id and fingerprint. The same id with
a different fingerprint is `IdempotencyConflict`. Once `Completed` is
installed, the same exact retry returns the original completion without a
third generation advancement. A crash before acknowledgement and a crash after
acknowledgement reopen to the same durable `Completed` state; the caller-facing
difference is whether the earlier acknowledgement may have been observed.
`AlreadyCompleted` is the safe response in both cases.

Every failure after a visible or possibly visible mutation remains
`DurabilityIndeterminate` at the failing boundary, even when restart later
observes an exact candidate state. Restart inspection and exact retry may
converge it; the original failure is never relabeled `Accepted`, and recovery
does not attempt eager rollback.

## Crash cuts

| Cut | Reopen observations modeled | Required convergence |
| --- | --- | --- |
| Before prepare | clean source | prepare normally |
| After prepare | temp missing or exact temp | recreate or reuse exact temp |
| After quarantine publish | missing, temp, or final name | preserve source; establish qualified final namespace before manifest acceptance |
| After quarantine namespace durability | durable final quarantine only | persist `Prepared` |
| After manifest acceptance | `Prepared`, source full | truncate and sync once |
| After source truncate | `Prepared`, source full or target length | truncate only if full; otherwise continue to completion |
| After source sync | `Prepared`, source target length | persist `Completed` |
| After completion persistence | `Completed`, source target length | exact retry returns `AlreadyCompleted` |
| Pre-acknowledgement | `Completed`, acknowledgement not observed | exact retry returns `AlreadyCompleted` |
| Post-acknowledgement | `Completed`, acknowledgement observed before crash | exact retry still returns `AlreadyCompleted` |

The fault matrix additionally exercised snapshot replacement observed as old
head plus temp or exact new head; log append observed as absent, torn, or
exact; and hybrid view observed as lagging or exact. Torn log tails and hybrid
view repair are representation-specific work that the snapshot avoids.

## Candidate comparison

The prototype's small complexity score adds canonical authorities, authority
writes, auxiliary view writes, namespace replacements, distinct restart-skew
domains, torn-tail handling, and replay. It is a design-complexity comparison,
not a performance benchmark.

| Candidate | Authority | Manifest writes | Namespace replacements | Additional recovery | ADR-0037 | Score |
| --- | --- | ---: | ---: | --- | --- | ---: |
| Versioned atomic snapshot + generation CAS | one current snapshot | 2 | 2 | old/new head plus replacement temp; no replay | no backflow | **6** |
| Append-only manifest log | one new event stream | 2 | 0 after creation | torn-tail quarantine and replay | new fifth stream; stop/backflow required | 7 |
| Log + materialized view | log authority plus rebuildable view | 2 + 2 view writes | 0 after creation | torn-tail replay plus log/view skew repair | new fifth stream; stop/backflow required | 10 |

The log's lower steady-state namespace replacement count does not offset its
new canonical event domain, framing/torn-tail protocol, replay, and migration
surface. The hybrid retains all log costs and adds a second physical structure
whose lag must be detected and repaired. The snapshot wins the comparison even
before applying the ADR-0037 stop rule.

## Production handoff and non-goals

`audio-graph-a596` may implement the dormant typed manifest kernel from this
contract only after consuming the named durability substrate. It must keep the
generation comparison and replacement inside the stable coordination lock,
strictly validate schema/type/relative identities, persist exact residuals,
and inherit typed platform outcomes. Later locked recovery may consume the
kernel; no runtime writer or consumer is activated by this decision.

This prototype does not prove OS barriers, atomic replacement on a particular
filesystem, process-crash behavior, power-loss behavior, production schema
compatibility, migration, performance, export/delete parity, or cross-platform
qualification. Those remain with the Wave 7B implementation and evidence
Seeds. No product code, package, workflow, generated file, or Seeds state is
changed here.
