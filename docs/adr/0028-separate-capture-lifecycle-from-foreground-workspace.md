---
status: accepted
date: 2026-07-09
deciders: [AudioGraph maintainers]
---

# ADR-0028: Separate Capture Lifecycle from Foreground Workspace

> **Note (2026-08-17):** The finalization arm of this record is **narrowed by
> [ADR-0035](0035-record-post-stop-finalization-failure-as-per-session-finalization-blocked.md)**.
> Post-stop finalization failure now records a per-Session `Finalization Blocked`
> state instead of entering app-modal `RecoveryRequired`. Read ADR-0035 as the
> current rule wherever this record says an incomplete drain "or finalization"
> enters `RecoveryRequired`. Everything else below — capture lifecycle, foreground
> workspace independence, passive readiness, egress scoping — remains in force,
> and `RecoveryRequired` retains its full undismissable strength for canonical
> writer and local drain failure.

## Context and Problem Statement

AudioGraph currently lets frontend view selection stand in for backend capture
truth. Capture, transcription, session writers, and projection consumers can
start through separate actions, and the frontend can infer Running before each
required stage has one authoritative acknowledgement. Historical session load
can also be confused with active session ownership.

The backend must own a safe session lifecycle independently from whatever the
user is viewing. A user may review historical session A while session B remains
live, with B's Stop, health, route, and durability controls still reachable.
Foreground navigation must never rotate B's writers or autosave target.

## Decision Drivers

- One action should start a useful note session or return one rollback result.
- Running must mean sources, consumers, provider, and durable session ownership
  are acknowledged.
- The first captured sample must have a consumer or an explicit discontinuity.
- Stop must drain and finalize in a bounded, observable order.
- Canonical persistence failure must never be dismissible into a healthy state.
- Historical review must be side-effect-free with respect to live capture.
- Fatal source exit and partial startup must reconcile one authoritative state.
- Backend lifecycle truth must remain visible from every foreground workspace.

## Considered Options

- Keep one coupled UI phase as both lifecycle and workspace state
- Use orthogonal backend lifecycle and foreground workspace state
- Run every active and historical session in an independent window process

## Decision Outcome

Chosen option: "Use orthogonal backend lifecycle and foreground workspace
state", because capture ownership and durability must remain authoritative while
the user navigates or reviews different content.

The backend session lifecycle is:

`Idle -> Starting -> Live -> Stopping -> Idle`

`RecoveryRequired` is reachable after canonical writer, drain, or
finalization failure. It retains exact residual and pending state plus explicit
retry, export, or safe-stop actions. It cannot be cosmetically dismissed into a
Saved or healthy state.

Foreground workspace state is independent. Selecting historical session A for
Review while capture B is Live does not mutate B's session id, writers,
ledgers, schedulers, buffers, graph states, or autosave target. A compact,
persistent active-session control keeps B's Stop, health, observed route, and
durability status reachable from every workspace.

Coordinated Start uses one backend command and this order:

1. allocate a draft generation, session id, audit scope, and canonical writers;
2. register bounded processing consumers;
3. connect and acknowledge the selected ASR provider;
4. register bounded projection consumers;
5. start and acknowledge rsac sources last.

The lifecycle emits Live only after required source subscriptions are active,
the first sample has an acknowledged consumer path, and rollback ownership is
established. A stage failure rolls back every completed stage in reverse order
and returns one typed actionable reason. Silent partial-source startup is not
part of the MVP.

Coordinated Stop first prevents new source production, then drains processing
consumers, provider completion, canonical writers, and derived
materialization in a bounded documented order. An incomplete canonical drain
or finalization enters RecoveryRequired rather than reporting Review or Saved.
Fatal source exit and last-active-source loss reconcile the same lifecycle.

Passive local readiness checks perform no provider egress. Any active provider
probe first allocates a draft generation and data-movement audit scope. A local
sample preview uses source permission but never reaches ASR, LLM, or canonical
session storage; teardown reports local drop state.

### Consequences

- **Positive**: Navigation can no longer steal capture ownership.
- **Positive**: Running, Stopping, Saved, and RecoveryRequired become
  backend-testable claims.
- **Positive**: One command owns startup ordering, rollback, and partial failure.
- **Positive**: Long capture can continue while the user reviews older sessions.
- **Negative**: Backend IPC and frontend state must migrate together.
- **Negative**: Orthogonal lifecycle and workspace selectors require more
  explicit tests than one global phase enum.
- **Negative**: Bounded drain and rollback add timeout and residual-state
  behavior that must be tested on all three operating systems.
- **Neutral**: Foreground workspace names and visual composition are governed by
  ADR-0030 rather than this lifecycle contract.

## Pros and Cons of the Options

### Keep one coupled UI phase as both lifecycle and workspace state

- Good, because it is the smallest state model.
- Good, because existing components already read a global phase.
- Bad, because opening history can imply capture stopped or rotate live state.
- Bad, because frontend navigation can make backend readiness claims.
- Bad, because rollback and RecoveryRequired have no durable owner.

### Use orthogonal backend lifecycle and foreground workspace state

- Good, because backend resources retain one owner independent of navigation.
- Good, because review A and capture B can safely coexist.
- Good, because start, stop, failure, and recovery share one command contract.
- Bad, because selectors, IPC events, and tests become more explicit.
- Bad, because every workspace must preserve active-session controls.

### Run every active and historical session in an independent window process

- Good, because process boundaries strongly isolate state.
- Good, because multiple sessions could be visible simultaneously.
- Bad, because capture device, tray, credential, and backend ownership become
  distributed coordination problems.
- Bad, because it adds window lifecycle and crash-recovery complexity before
  the MVP needs multiple active sessions.

## More Information

The shell information architecture is ADR-0030. Durable Accepted state is
ADR-0027, and the source timing boundary is ADR-0020.

Validation must cover no-key, rejected-key, unavailable-provider,
permission-denied, storage-full, source-start failure, provider-start failure,
fatal source exit, bounded-stop timeout, Review A while capture B remains Live,
passive versus active readiness egress, preview isolation, restart, and
packaged Windows/macOS/Linux capture-stop-recovery.

Research:

- `docs/research/mvp-ui-ux-2026-07-09.md`
- `docs/research/rsac-0.4.1-capture-audit-2026-07-09.md`

Mutable rollout evidence belongs in those audits and the current MVP handoff,
not in this accepted decision record. Implementation is tracked by
`audio-graph-b5ef`, `audio-graph-4521`, and `audio-graph-1f71`.
