# Session projection admission and fencing prototype

> **THROWAWAY LOGIC PROTOTYPE — `audio-graph-5e41`.** This is executable
> design evidence, not production code, a production durability claim, or an
> implementation of `SessionSemanticsVersion`.

## Question and assumptions

Can one receipt-bearing projection admission model prevent queue admission from
appearing durable, prevent detached Session workers from publishing or
recreating artifacts, and still make exact retry, restart, and independent
Notes/Graph progress deterministic?

The model assumes:

- one synthetic projection event per lane is enough to test the finite state
  boundary; its attempted per-stream sequence is `1`;
- the idempotency identity is the exact event id plus exact attempted-byte
  digest, and `AlreadyAccepted` must return the original committed sequence;
- a durable scheduler-queue record and a durable canonical projection event
  are different records and different transitions;
- an existing canonical stream needs completed write, flush, and file-sync
  barriers, while a newly created stream also needs a parent-directory barrier;
- the barrier flags are abstract inputs. This prototype does not prove that
  Windows, macOS, or Linux supplied them; that proof remains `audio-graph-8e73`;
- loss of an in-flight remote request produces `ExternalEffectUnknown`; the
  process cannot infer whether cost, egress, or a provider-side result occurred;
- a deletion or Session replacement raises its fence before waiting for or
  discarding remote results; and
- all state and diagnostics are synthetic metadata held only in memory. The
  prototype opens no files, network connections, or production modules.

Run the complete model in one command:

```text
bun scripts/prototype-session-projection-admission.mjs
```

The command prints a representative full state after every action, then runs
the exhaustive bounded model. There is intentionally no package-script entry,
fixture directory, persistence layer, or separate test framework.

## Result

The model supports the proposed shape, with one non-negotiable boundary:
materialized state and Projection Basis eligibility advance atomically from a
canonical `Accepted` or exact-retry `AlreadyAccepted` receipt, never from
`Pending`, `Rejected`, `OutcomeUncertain`, queue admission, a remote result, or
a snapshot write.

An accepted event owns its per-stream committed sequence forever. An exact
retry returns `AlreadyAccepted` with that sequence. Reusing its idempotency id
with different attempted bytes returns `Rejected` and does not alter the
commit. A materialized snapshot failure after acceptance is cache lag; it does
not roll back the accepted logical state or free the sequence for reuse.

The model also supports one Session ownership token:

```text
(session key, session epoch, process lease, projection lane, exact job id)
```

Restart replaces the process lease. Session rotation replaces both the epoch
and lease. Deletion raises a fence and replaces the lease before artifact
removal or waiting. Results and writer calls bearing any retired token are
observable refusals; success and failure have the same fence behavior.

## Admission state and crash reconciliation

| State returned to caller | Canonical meaning | Materialized/basis eligible? | Retry rule |
| --- | --- | --- | --- |
| `Pending` | Bounded admission began; no durable claim | No | Reopen/reconcile, then retry exact bytes if absent |
| `Accepted` | Exact canonical bytes crossed every required barrier and acknowledgement returned | Yes, atomically at the committed sequence | Exact retry becomes `AlreadyAccepted` |
| `AlreadyAccepted` | Reopen/retry found the exact id and bytes already committed | Yes, at the original sequence | Return the same sequence; never append again |
| `Rejected` | Definite refusal or exact id with different bytes | No new advancement | Correct the request; an idempotency conflict never overwrites |
| `OutcomeUncertain` | The caller lost the result after an effect may have begun | No | Reconcile exact bytes before any append or visible advancement |

The executable matrix explores both existing and newly created streams at each
cut. The allowed restart observations are deliberately conservative:

| Crash cut | Caller state | Restart observations in the bounded model | Required convergence |
| --- | --- | --- | --- |
| before enqueue | `Rejected` | absent | exact new attempt may commit sequence 1 |
| after enqueue | `Pending` | absent | exact retry may commit sequence 1 |
| after write | `OutcomeUncertain` | absent, torn tail, or durable exact bytes | typed quarantine/absence permits exact append; exact bytes return `AlreadyAccepted(1)` |
| after flush | `OutcomeUncertain` | absent, torn tail, or durable exact bytes | same reconciliation rule |
| after file sync, existing stream | `OutcomeUncertain` | durable exact bytes | return `AlreadyAccepted(1)` |
| after file sync, new stream | `OutcomeUncertain` | absent or durable exact bytes because directory-entry durability was not acknowledged | complete recovery barriers, then exact append or `AlreadyAccepted(1)` |
| after directory sync | `OutcomeUncertain` | durable exact bytes | return `AlreadyAccepted(1)` |
| after acknowledgement | `Accepted` | durable exact bytes | exact retry returns `AlreadyAccepted(1)` |

The `TornTailRequiresTypedQuarantine` diagnostic is only a pointer to later
work. This prototype does not choose a quarantine transaction or pretend that
the current implementation can durably register one.

## Scheduler restart and lane independence

A job may cross the remote boundary only after its scheduler record is
`DurableQueued`.

- Restart from `DurableQueued` remains safe to dispatch because no remote
  attempt began.
- Restart from `RemoteInFlight` becomes `ExternalEffectUnknown`; it never
  fabricates success, failure, or safe-to-retry.
- Late success and late failure from the retired lease are both refused.
- Notes and Graph own separate scheduler states, canonical sequences,
  materialized heads, and basis-eligible heads. The checker runs all 25 receipt
  pairs in both lane orders for every policy profile and requires identical
  lane state. One lane may therefore be Accepted while the other is Pending,
  Rejected, or uncertain.

The model does not choose backlog coalescing, priority, concurrency, or final
refinement semantics. Those remain in the scheduler design under
`audio-graph-3b48`.

## Deletion model

Both deletion policies start with the same mandatory action: raise the Session
deletion fence, retire every writer lease, and refuse all later results and
writes.

- `DiscardImmediately` removes managed artifact state without waiting for a
  provider terminal. A later success or failure is counted and discarded.
- `WaitForRemote` retains artifacts while it observes terminals for the set of
  requests already in flight. Results are still discarded and cannot advance
  canonical or materialized state. Artifact removal occurs only after that set
  is empty.

Neither policy cancels the fence, accepts a late result, or lets a detached
writer recreate an artifact. Actual artifact inventory, locked writer
quiescence, directory barriers, residual manifests, and purge behavior remain
production work.

## Human policy decisions

All eight combinations of the three axes satisfy the safety invariants. The
prototype therefore exposes tradeoffs; it does not convert recommendations
into product decisions.

| Policy | Candidate A | Candidate B | Prototype observation | Recommendation | Human decision |
| --- | --- | --- | --- | --- | --- |
| User-facing persistence wording | `Saved` only after durable `Accepted`/`AlreadyAccepted` | `Durably saved` only after the same receipts | Both are safe if `Pending` says `Saving`, uncertainty says `Recovery required`, and rejection says `Not saved` | Prefer the shorter `Saved`, but make its durable meaning a UI contract; never reuse it for queue admission or a weaker platform level | **Open: accept, revise, or choose the explicit wording** |
| Remote request after `ExternalEffectUnknown` | Require reconciliation/user decision | Reissue automatically with a visible duplicate cost/egress risk | Both can protect local state; automatic reissue can still duplicate provider cost and content egress | Do not automatically reissue unless that route has independently proven provider idempotency for the exact request | **Open: accept, revise, or authorize at-risk automatic reissue** |
| Deletion with remote work in flight | Fence and discard immediately | Fence, wait for terminals, discard results, then delete | Both prevent resurrection; waiting delays deletion without making an unknown remote effect reversible | Default to immediate fenced discard; a future explicit wait mode must never relax the fence | **Open: accept, revise, or require wait mode** |

## Transition ownership for later work

This is the handoff from the prototype transition to the work that must make it
real. No row is implemented here.

| Prototype transition | Required production owner | Contract carried forward |
| --- | --- | --- |
| `Idle -> DurableQueued` before remote dispatch | Projection Backlog/scheduler persistence (`audio-graph-3b48`) | A restart can distinguish never-dispatched durable work from external-effect-unknown work; Notes and Graph queue independently |
| `DurableQueued -> RemoteInFlight -> ExternalEffectUnknown` across restart | Projection scheduler persistence (`audio-graph-3b48`) | Persist exact job/attempt ownership; do not infer provider outcome; apply the accepted reissue policy |
| canonical enqueue -> `Pending` | canonical commit boundary (`audio-graph-90f3`, then mixed transcript follow-through in `audio-graph-6b9d`) | Queue admission, writer send, and snapshot write expose no durable state and no Projection Basis eligibility |
| write -> flush -> file sync | canonical durability (`audio-graph-90f3`) | Any pre-ack lost response is reconciled by exact id and attempted bytes; sequence cannot be reused |
| new-file directory sync, typed quarantine registration, destructive recovery | cross-platform durability (`audio-graph-8e73`) | Do not produce `Accepted` or user-facing Saved until the platform-specific file/directory/manifest transaction is proven |
| durable exact commit -> `Accepted(sequence)` | canonical commit integration (`audio-graph-90f3`) | Atomically advance live materialized state and basis eligibility; snapshot failure is rebuildable-cache lag |
| exact reopen/retry -> `AlreadyAccepted(original sequence)` | canonical durability/recovery (`audio-graph-90f3` plus `audio-graph-8e73`) | Reconcile before append; exact retry cannot duplicate or renumber the event |
| `Accepted`/`AlreadyAccepted` provenance guard -> v2 Session floor | Session-provenance floor (`audio-graph-7e81`) | The monotonic floor advances only from these receipts; guard-ahead retry is idempotent |
| accepted sequence -> first-position ledger/basis | unified ledger (`audio-graph-0baf`) and scheduler/currency (`audio-graph-4c82`) | Only accepted canonical order may become a Projection Basis position; Pending and uncertain work remain ineligible |
| restart lease replacement and Session epoch rotation | scheduler persistence (`audio-graph-3b48`) plus Session lifecycle/load (`audio-graph-9c89`, later `audio-graph-e969`) | Match exact Session, epoch, lease, lane, and job id before any terminal mutation |
| deletion fence -> discard or fenced wait -> artifact removal | Session deletion (`audio-graph-9c89`, later checked mixed deletion in `audio-graph-e969`) plus `audio-graph-8e73` durability | Revoke writers before removal/wait; late results cannot publish; typed inventory and residual reporting determine completion |
| deterministic failure code/counter | trustworthy acceptance evidence (`audio-graph-44c1`) | Diagnostics contain only state metadata, never transcript, prompt, provider payload, credentials, or other user content |

## Executable evidence boundary

The successful run explores eight policy profiles and 744 exhaustive bounded
cases: 368 admission/crash cases, 80 receipt cases, 32 scheduler-restart cases,
200 two-lane commutativity cases, and 64 rotation/deletion cases. It evaluates
7,434 reducer transitions, observes 936 unique full states, performs 55,638
invariant assertions across 23 named invariant families, and observes all five
receipt states.

Those counts describe this finite prototype, not the production state space.
They do not prove filesystem, subprocess, operating-system, provider, or UI
behavior. Production evidence must come from the later Seeds mapped above.
