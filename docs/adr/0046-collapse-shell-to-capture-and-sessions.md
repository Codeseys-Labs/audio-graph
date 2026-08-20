---
status: accepted
date: 2026-08-20
deciders: [AudioGraph maintainers]
---

# ADR-0046: Collapse the Shell to Capture and Sessions with a Persistent Active-Session Strip (amends ADR-0030)

## Context and Problem Statement

ADR-0030 accepted Ready, LiveNow, Review, and Inspect as the MVP shell's
information architecture. Only the tab labels ever migrated: the shell still
exposes three peer tabs (`during`/`after`/`analysis`), Inspect is still a
sibling tab despite ADR-0030's own text ("not a peer primary product mode"),
and Ready/LiveNow are one tab whose label swaps on `isCapturing` — they were
never a navigable choice. The 2026-08-20 design panel verified the resulting
product gap: `stopCapture` never selects the finished session and the only
path to a recording is a modal (`store/index.ts:2157`, `App.tsx:441-445`) —
the product's noun (a session) has no destination. The maintainer has ratified
a full recomposition over the panel's graft-only recommendation.

This record amends ADR-0030's workspace *structure* while preserving its
decision drivers, lifecycle contract (ADR-0028), route-evidence rules
(ADR-0034), and visual direction.

## Decision Drivers

All of ADR-0030's drivers, plus two the label-only migration exposed:

- The finished recording must land somewhere the user can see.
- The IA must express list→detail with one primary action — the only shape a
  future mobile/wearable client (seed audio-graph-8055) can inherit.

## Considered Options

1. Keep the accepted four peer workspaces, implemented literally as four tabs.
2. Two destinations — Capture and Sessions — with a persistent NOW STRIP and
   contextual lenses/drawers.
3. Graft-only: Sessions rail+detail inside the existing Review tab, no
   structural change (the panel's recommendation).

## Decision Outcome

Chosen option: **two destinations + strip + lenses**, because it delivers what
ADR-0030 already argued: Ready/LiveNow are not a user choice (`isCapturing`
picks), Inspect "is not a peer primary product mode", and the lifecycle strip
is ADR-0028's "compact active-session control" made permanent.

ADR-0030's four names map, none are deleted:

| ADR-0030 workspace | Where it lives now |
|---|---|
| Ready   | Capture destination, idle state: preflight card (sources, planned route, storage) + one Start |
| LiveNow | Capture destination, capturing state: notes primary, transcript/assist aside; Stop/health/route/durability on the strip |
| Review  | Sessions destination: list rail → detail; Stop selects the just-ended session |
| Inspect | Contextual lenses on a session (Timeline/Graph/Route/Ask) + the System drawer (projection runtime, token usage, per-stage pipeline detail) |

Structural consequences, stated explicitly because they are contract changes:

- Workspace tab ids become `#workspace-tab-capture` / `#workspace-tab-sessions`
  (panels likewise); the `analysis` tab is removed. `e2e/specs/shell.e2e.ts`
  and `App.contract.test.tsx` are rewritten in the same landing as the rename,
  and in no other landing.
- Navigation state becomes one serializable object (store slice `shellNav`),
  not seven flags.
- The NOW STRIP owns Start/Stop, elapsed, durability, composite health, and
  the route chip. Ready labels the route **planned**; **observed** appears
  only when session-scoped data-movement evidence exists (ADR-0034) — until
  the audio-graph-70a3/51e0 ledger surfaces active-route state, the live chip
  stays planned-labeled.
- One Start on the strip composes the existing gated actions
  (`start_capture`, then transcribe only where ADR-0033's enablement gate
  already permits). It claims no atomicity; ADR-0028's coordinated Start
  (seed audio-graph-10ff) replaces the composition behind the same button
  when it lands.
- Validation matrix restated for the new shape: Capture-idle, Capture-live,
  Sessions-list, Sessions-detail(+each lens) at 1440/1024/768, light+dark,
  idle/loading/empty/degraded/recovery/error, keyboard-only, 200% zoom
  (== compact tier), forced colors, reduced motion, NVDA, VoiceOver,
  packaged three-OS smoke. Nothing below 768px is claimed.

### Consequences

- Positive: recordings become objects with a destination; Stop has a landing.
- Positive: the IA is expressible on a phone tab bar or a watch (shape only —
  no mobile client is claimed; ADR-0030's storage/lifecycle contracts are
  untouched).
- Positive: diagnostics stop competing with reading surfaces without losing
  reach (drawer + lenses).
- Negative: every E2E/contract id fact changes once, in one landing; saved
  workspace state and screenshots need one migration.
- Negative: two-destination chrome must carry the ADR-0028 active-session
  strip on every width tier or the lifecycle becomes invisible.
- Neutral: realtime speech-to-speech remains outside the shell (ADR-0013).

## Pros and Cons of the Options

### Four peer tabs, literal reading
- Good: no contract churn beyond labels (already paid).
- Bad: contradicts ADR-0030's own Inspect clause; Ready/LiveNow tabs are
  fake choices; the session still has no destination.

### Two destinations + strip + lenses (chosen)
- Good: aligns structure with the accepted product argument; one rename event.
- Good: list→detail + one action is the durable mobile-ready shape.
- Bad: one expensive, carefully-sequenced rename/E2E landing (mitigated: it
  is quarantined as pure deletion+rename after Sessions and the strip exist).

### Graft-only (panel recommendation)
- Good: ~70% of the product delight at ~30% of the risk; zero id churn.
- Bad: permanently entrenches the pipeline-stage tabs ADR-0030 diagnosed;
  ratification 2026-08-20 explicitly chose against it.

## More Information

Amends ADR-0030 (structure); lifecycle ADR-0028; evidence ADR-0034; provider
gates ADR-0033. Design basis: workflow wf_993ac5be-0aa Design 3 +
docs/agentic-runs/2026-08-20-ui-overhaul-design/{ui-design,recomposition-plan}.md.
Execution seed: audio-graph-19c7.
