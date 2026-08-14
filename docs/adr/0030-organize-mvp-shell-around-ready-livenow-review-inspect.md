---
status: accepted
date: 2026-07-09
deciders: [AudioGraph maintainers]
---

# ADR-0030: Organize the MVP Shell Around Ready, LiveNow, Review, and Inspect

## Context and Problem Statement

AudioGraph's MVP job is to select audio, start once, receive trustworthy live
notes and transcript, then review, export, and inspect provenance. The current
During, After, and Analysis shell exposes implementation stages as peer
destinations, opens into an empty live cockpit, and gives graph diagnostics
visual weight comparable to notes. Replacing all existing frontend contracts
would discard tested transcript, notes, timeline, graph, settings, tray,
shortcut, and accessibility work.

The shell needs a product-shaped information architecture while preserving the
orthogonal backend lifecycle established by ADR-0028.

## Decision Drivers

- Explain readiness and planned data movement before capture.
- Make automatic notes the primary live value.
- Keep transcript, timing, route, and graph evidence close enough to build
  trust without dominating the task.
- Keep active capture controls visible while reviewing history.
- Enforce generated provider selectability on every actionable surface.
- Reuse proven typed Tauri/store contracts and data panels.
- Support desktop widths, 200 percent zoom, keyboard-only operation, forced
  colors, reduced motion, and assistive technology.
- Give storage pressure and RecoveryRequired non-dismissible recovery paths.

## Considered Options

- Incrementally relabel During, After, and Analysis
- Recompose the shell around Ready, LiveNow, Review, and Inspect
- Replace the whole frontend, store, and Tauri integration

## Decision Outcome

Chosen option: "Recompose the shell around Ready, LiveNow, Review, and
Inspect", because it aligns navigation with the user's job while preserving
tested backend and panel contracts.

The foreground workspaces are:

- **Ready**: select sources, show passive local preflight, display the planned
  provider and storage route, and offer one Start note session action.
- **LiveNow**: show automatic notes first, transcript and speaker/timing
  provenance second, plus persistent Stop, durability, health, and observed
  route.
- **Review**: browse, search, export, delete, and inspect a historical session
  without mutating the active capture lifecycle.
- **Inspect**: a contextual Review workspace or Live drawer for graph,
  projection, data-movement, and diagnostic evidence; it is not a peer primary
  product mode.

The shell always exposes the independent ADR-0028 lifecycle. Review session A
while capture B is live retains a compact active-session strip with B's Stop,
health, observed route, and durability state.

Ready labels configuration as a **planned route** and performs no provider
egress. LiveNow and Review label a route **observed** only when session-scoped
data-movement evidence proves it. Credential presence alone never means Ready.
Generated `ui_selectable` and runtime readiness are universal action gates.
Deferred providers appear only as non-actionable Planned or Not in MVP
information.

The rewrite changes `App.tsx`, control chrome, workspace composition, Express
Setup, and Settings overview. It retains existing store and Tauri contracts,
notes, transcript, timeline, graph, session, settings, tray, shortcut, and
accessibility primitives wherever their contracts remain valid.

The visual direction is "studio tape meets temporal memory": a calm recording
deck, high-legibility type, restrained semantic color, and a temporal spine
showing meaningful capture, transcript, projection, health, and provenance
events. The spine is not a decorative fake waveform. Existing ADR-0009 design
tokens and the ADR-0016 Tailwind bridge remain authoritative.

Storage-full and RecoveryRequired states interrupt optimistic navigation and
provide explicit retry, export, safe-stop, or cleanup actions. No close button
converts them to healthy.

### Consequences

- **Positive**: First run explains what will happen before capture.
- **Positive**: Live emphasizes durable notes while keeping provenance nearby.
- **Positive**: Diagnostics remain available without competing with the primary
  workflow.
- **Positive**: Existing tested panels and accessibility primitives remain
  valuable.
- **Negative**: Shell composition, screenshots, interaction tests, and saved
  view compatibility require broad updates.
- **Negative**: Some advanced graph and pipeline controls move one interaction
  deeper.
- **Negative**: Responsive, zoom, forced-color, and assistive-technology
  validation becomes a release gate across four workspaces.
- **Neutral**: Realtime speech-to-speech remains a sibling mode under ADR-0013
  and stays out of the MVP shell until its runtime gates pass.

## Pros and Cons of the Options

### Incrementally relabel During, After, and Analysis

- Good, because it touches the fewest composition files.
- Good, because existing navigation tests need smaller changes.
- Bad, because there is still no true Ready workspace.
- Bad, because Analysis continues to compete with notes and historical review.
- Bad, because active lifecycle and foreground navigation remain easy to
  conflate.

### Recompose the shell around Ready, LiveNow, Review, and Inspect

- Good, because the information architecture follows the user's end-to-end job.
- Good, because notes become primary and evidence becomes contextual.
- Good, because existing panels and typed contracts can be reused.
- Bad, because high-conflict shell and styling files require coordinated review.
- Bad, because visual and accessibility regression coverage expands.

### Replace the whole frontend, store, and Tauri integration

- Good, because it provides maximum structural and visual freedom.
- Good, because no compatibility layer is required for old composition.
- Bad, because it discards proven session, settings, tray, shortcut, panel, and
  accessibility work.
- Bad, because it multiplies backend/frontend contract risk and delays storage
  and capture correctness.

## More Information

Backend lifecycle is ADR-0028. Durable storage and route evidence are ADR-0027.
The product design is
`docs/designs/2026-07-09-mvp-product-and-experience.md`, and the research basis
is `docs/research/mvp-ui-ux-2026-07-09.md`.

Validation includes Ready/LiveNow/Review/Inspect screenshots at 1440, 1024, and
768 pixels in light and dark themes; idle, loading, empty, degraded, recovery,
and error states; keyboard-only navigation; 200 percent zoom; forced colors;
reduced motion; NVDA on Windows; VoiceOver on macOS; and packaged three-OS
smoke.
