---
status: accepted
date: 2026-08-19
deciders:
  - "AudioGraph maintainer (accepted 2026-08-19)"
drafter: "Claude agent (non-decider)"
narrows: ADR-0044
---

# ADR-0040: Select the Checked-Open Branch from Observed Control-Plane Absence

> **Provenance.** The maintainer decided this on 2026-08-19, choosing between
> accepting the deviation and implementing ADR-0044 item 5 literally, after the
> `audio-graph-e8e7` review escalated it as a conformance question no agent
> should settle. The reasoning below was drafted by an agent from that review's
> evidence; the maintainer reviewed the trade-off and the cost of the
> alternative, not every line of this record.

## Context and Problem Statement

[ADR-0044](0044-keep-session-control-plane-in-the-flat-artifact-root.md) item 5
fixes two checked-open branches **by capability** and closes with: "These
branches are fixed by capability; an implementer does not choose between them."

- On qualified Linux/ext4 or macOS/APFS with the store lock initially absent,
  checked open must establish the qualified global coordination entry, acquire
  its shared guard, and revalidate the Session manifest's absence *under that
  guard* before admitting v1.
- On the unqualified Windows/Other read-only path it creates nothing, and may
  admit an absent manifest as v1 "only because the production namespace policy
  makes manifest/proof mutation unavailable there," using a pre/post sandwich:
  check both identities, build the whole snapshot without releasing bytes, check
  both again before the snapshot escapes, and return a typed retry/refusal on any
  appearance or change.

`audio-graph-e8e7` shipped `open_session_for_content`, which selects between
these two shapes from an **unlocked** observation: if no control-plane entry is
present it takes the sandwich, and only if one is present does it construct the
store and take the shared guard. Because nothing writes a control plane at this
base, every Session today takes the sandwich, so neither the shared guard nor
`checked_session_open` runs on the live path.

The deviation is narrower and sharper than "the read is unguarded." It is that
the **Windows-shaped branch is applied on a platform where the Windows
justification does not hold**. On Windows the sandwich is licensed by a
structural fact — the namespace policy makes mutation unavailable, so there is
nothing to race. On qualified Linux and macOS mutation *is* available; the
sandwich is currently safe only because no code advances a Session to v2 yet.
That is a contingent property of this base, not a property of the platform.

Implementing item 5 literally has a real cost: the coordination entry would be
established on the **first read of any Session on any host**, so read-only
history browsing would begin writing lock state, and the unqualified platforms
would still need their own branch.

## Decision Drivers

- ADR-0044's guarantee exists to stop a stale absent result from being admitted
  as v1 while a concurrent mutator advances the Session.
- Reading history must not require creating coordination state on disk.
- A deviation that is safe only because a caller does not exist yet must be
  recorded as such, and must be re-examined when that caller appears.
- The repository treats an evidence artifact that overstates its guarantee as a
  defect; an ADR that quietly blesses a weaker property is the same defect.

## Considered Options

- **A. Select the branch from observed control-plane absence** (what shipped):
  keep the unguarded sandwich for the no-control-plane case on every platform,
  and require replacement by a single guarded call site before a v2 writer lands.
- **B. Implement ADR-0044 item 5 literally**: always construct the store and
  take the shared guard on qualified platforms, establishing the coordination
  entry on first read.
- **C. Block the read seam** until a writer exists, leaving canonical reads
  ungated in the meantime.

## Decision Outcome

Chosen option: **A**, because the guarantee item 5 protects cannot be violated
while no code can advance a Session to v2, and paying for it today means every
history browse creates coordination state on disk for a race that cannot occur.

This narrows item 5 as follows, and **only** as follows:

1. Checked open MAY select its branch from an unlocked observation of
   control-plane presence. When no control identity and no coordination entry are
   observed, it MAY admit historical v1 through the pre/post sandwich item 5
   already specifies for the unqualified path, on any platform.
2. When any control identity or the coordination entry IS observed, the
   capability rule stands unchanged: qualified platforms take the shared guard
   and revalidate under it.
3. The sandwich's typed refusal is mandatory, not optional. A control identity
   appearing before or after the snapshot must return
   `ControlPlaneAppearedDuringUnguardedRead` rather than v1.
4. **This narrowing expires when a v2 writer is activated.** The seed that
   activates `advance_session_semantics_v1_to_v2`, or any other durable v2
   producer, must first replace the branch selection with a single guarded call
   site, and must not treat this record as authorization to keep the sandwich.

Everything else in ADR-0044 — the flat control plane, the store-owned lock, the
mutation contract, the proof shape, and item 5's guarded branch for a Session
that has a control plane — remains in force.

### Consequences

- **Positive**: reading a historical Session creates nothing on disk, which keeps
  history browsing side-effect-free and matches ADR-0028's requirement that
  passive readiness perform no writes.
- **Positive**: the guarded branch still exists and is exercised the moment a
  control plane exists, so the machinery is not removed, only deferred.
- **Negative, and the reason this needed a human**: the sandwich on a qualified
  platform rests on a **contingent** fact (no v2 writer exists) where Windows
  rests on a **structural** one (mutation unavailable). This is a strictly weaker
  guarantee than item 5 as written, and nothing in the type system enforces the
  expiry in decision item 4 — only this record and the residual it points at.
- **Negative**: for the window between now and writer activation, a reader that
  observes no control plane and then races a hypothetical mutator relies on the
  post-check to catch it. That check is sound, but the branch *decision* was
  still made from unlocked bytes, which is what item 5 forbade.
- **Negative**: narrows an accepted ADR, so a reader of ADR-0044 item 5 must now
  read this record to know the current rule.
- **Neutral**: no behaviour changes for a Session that already has a control
  plane, and none on Windows/Other, whose branch this record does not touch.

## Pros and Cons of the Options

### A. Select the branch from observed control-plane absence

- Good, because history reads stay side-effect-free.
- Good, because the guarded path is retained for every Session that has a
  control plane, so conformance returns automatically as the wave progresses.
- Good, because the failure mode is a typed refusal, never a wrongly admitted
  floor.
- Bad, because it substitutes a contingent safety argument for a structural one.
- Bad, because the expiry condition is documentary, not mechanical.

### B. Implement ADR-0044 item 5 literally

- Good, because it is race-safe by construction and needs no expiry clause.
- Good, because it removes an ADR deviation instead of recording one.
- Bad, because opening any Session for reading would establish the coordination
  entry, so read-only history browsing writes lock state on every host.
- Bad, because it puts lock establishment on the path of the most common
  operation in the app to protect against a mutator that does not exist yet.

### C. Block the read seam until a writer exists

- Good, because it defers the question without weakening any rule.
- Bad, because canonical reads then stay entirely ungated, which is the gap
  `audio-graph-e8e7` acceptance (2) exists to close — strictly worse than either
  branch shape.

## More Information

- **Relationship to ADR-0044**: narrows item 5's branch-selection rule only.
  ADR-0044 keeps its `accepted` status and carries a pointer here, following this
  repository's partial-supersession convention (ADR-0003 / ADR-0006, and
  ADR-0028 / ADR-0041).
- **Where the deviation is disclosed in code**: `open_session_for_content`'s
  rustdoc states it is not race-safe and names the two symbols whose activation
  requires replacing it; the `audio-graph-e8e7` report records it as residual R1.
- **How to reverse**: implement option B. Cheap today — the guarded path already
  exists and the change is the branch condition plus a Windows/Other arm. The
  cost rises only if callers come to depend on reads creating nothing.
- **Numbering note**: `0040` was chosen after enumerating `docs/adr` across every
  local and remote ref, not just the checked-out branch, because seed
  `audio-graph-c306` records that master and this integration branch already
  carry four different ADRs under numbers 0035 through 0038. Seed
  `audio-graph-c306` has since resolved that collision on this branch by
  renumbering this branch's four records to 0041-0044, per the convention that
  the later-merged branch renumbers; `0040` and ADR-0039 never collided and kept
  their numbers. See the renumbering note in `README.md` for the mapping.
