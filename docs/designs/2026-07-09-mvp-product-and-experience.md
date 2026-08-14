# AudioGraph MVP product and experience design

Date: 2026-07-09

Status: accepted direction; implementation in waves

Owning Seeds: `audio-graph-99eb`, `audio-graph-5c24`,
`audio-graph-10ff`, and `audio-graph-19c7`

Research basis:

- `docs/research/mvp-ui-ux-2026-07-09.md`
- `docs/research/mvp-projection-correctness-2026-07-09.md`
- `docs/research/mvp-storage-audit-2026-07-09.md`
- `docs/research/rsac-0.4.1-capture-audit-2026-07-09.md`
- `docs/research/mvp-validation-devex-audit-2026-07-09.md`
- `docs/backlog/handoff-2026-07-08-caad-10ac-wave.md`
- ADR-0020, ADR-0024, ADR-0027, ADR-0028, ADR-0029, ADR-0030, ADR-0031,
  ADR-0032, ADR-0033, and ADR-0034
- proposed ADR-0025 and ADR-0026 for broader follow-on projection/timeline work

## Product definition

AudioGraph is a local-first desktop memory recorder for meetings, calls,
videos, and other computer audio.

Its MVP promise is:

> Select what to hear, start once, receive trustworthy live notes plus a
> transcript, then review, export, and inspect exactly where the memory came
> from.

The durable path is the product:

`source -> rsac capture -> Deepgram ASR -> revisioned transcript -> automatic
notes and temporal graph -> durable session replay`.

Realtime speech-to-speech agents remain a sibling product mode. They must not
shape first run, primary navigation, or MVP readiness until their capture,
egress, response, and playback paths satisfy the same evidence bar.

## MVP scope boundary

Selectable now:

- Deepgram streaming ASR
- the LLM providers already marked selectable by the generated registry
- no TTS or Deepgram Aura where the selected workflow does not speak
- local session files as the canonical durable store

Deferred:

- native Gemini and OpenAI realtime modes
- local and other ASR providers without complete runtime evidence
- additional TTS providers
- a runtime-selectable SurrealDB store
- agent-oriented controls in the primary recording flow

Deferred providers may appear in a clearly labelled roadmap or capability
inventory. Existing saved routes remain inspectable, credentials remain
maintainable, and stop or cleanup actions remain available, but no
content-bearing start may route into a deferred provider. The Rust command
boundary is authoritative for registry `ui_selectable`; the UI mirrors that
gate and readiness without substituting a different provider silently.

## Orthogonal experience model

The shell presents Ready, LiveNow, Review, and Inspect workspaces, but the
backend capture lifecycle is independent:

- lifecycle: Idle, Starting, Live, Stopping, RecoveryRequired
- workspace: Ready, LiveNow, Review(session id), Inspect(scope)

This separation lets a user review historical session A while session B remains
live. Review never steals writer, autosave, or stop ownership; the shell keeps
the active session's Stop, health, and durability controls visible.

### Ready

Ready owns:

- source discovery and selection
- capture capability and permission
- Deepgram credential and runtime readiness
- selected LLM credential, model, and runtime readiness
- local storage health and available-space status
- exact content-free data route and boundary disclosure
- sample preview
- one primary action: **Start note session**

Ready is not an empty Live screen. It is a compact preflight that tells the
truth about what will happen before audio leaves a source.

Ready shows a **planned route** derived from saved configuration, registry
selectability, and passive local checks. It does not call that route observed.
Any active provider probe first creates a draft session generation and
session-scoped audit owner.

Sample preview is local and transient. It uses the same source permission, never
enters ASR/LLM or canonical session storage, and tears down with explicit local
drop accounting.

### Starting

Starting is an explicit transient state. The backend owns a session generation
and returns progress across:

1. storage and session allocation
2. bounded pipeline consumer registration
3. Deepgram connection and readiness
4. projection worker registration
5. rsac source construction, start, and subscription acknowledgement

The shell exposes one deterministic progress message and Cancel. It never shows
Live or Running before the required startup quorum is acknowledged.

Sources start last so no producer can outrun its consumer. A first-sample
fixture proves every captured sample is processed or represented by an explicit
discontinuity.

Any failure rolls back all completed stages, joins workers, preserves the prior
idle session, and presents one actionable reason. Partial-source capture is
unsupported in the initial MVP unless a later accepted decision defines it.

### Live

Live begins automatically after successful start.

Primary surface:

- automatic notes, including decisions, actions, questions, and evolving
  context

Secondary surface:

- transcript with speaker, source, confidence, revision, and timing provenance

Persistent compact controls:

- source summary
- elapsed time
- Stop
- exact data route
- one composite health signal

LiveNow and Review show an **observed route** backed by session-scoped
data-movement evidence, not merely configured credentials.

Detailed pipeline stages, queue counters, token usage, agent controls, and
projection diagnostics are disclosed only when health degrades or the user
opens Inspect.

### Stopping

Stopping is explicit. Stop drains capture and processing, finalizes durable
events, materializes review caches, and only then enters Review. The UI must
distinguish:

- audio stopped, processing finishing
- canonical data durable
- derived views ready
- failure requiring recovery

Drain order and timeout are bounded and documented. A canonical writer, drain,
or finalization failure enters RecoveryRequired and cannot be dismissed into
Review/Saved.

### Review

Review opens automatically after a successful stop or when a historical session
is selected.

It owns:

- durable automatic notes
- decisions, actions, questions, and named entities
- transcript and speaker timeline
- seekable temporal spine
- source, provider, and boundary metadata
- export, rename, pin, and delete
- overflow action **Generate prose summary**

Historical review is read-only with respect to the active capture session.
Loading a session must not swap live backend writers or autosave ownership.
If another session is Live, its compact Stop, health, route, and durability
controls remain persistent while the historical Review workspace is open.

### Inspect

Inspect is nested under Review and available as a diagnostic drawer during
Live. It owns:

- temporal graph
- source and event provenance
- data-movement ledger
- projection basis, sequence, and status
- queue, drop, and backpressure details
- storage artifact manifest and recovery state
- redacted provider diagnostics

Inspect is not a peer navigation destination competing with the user's notes.

## Atomic start contract

The primary action coordinates one backend lifecycle command rather than a
frontend sequence of loosely related commands.

Passive preflight returns typed local checks:

- `ready`
- `needs_action`
- `unavailable`
- `blocked`

Each check includes a stable code, short user explanation, next action, and
content-free diagnostic context.

Start first allocates a draft generation and audit scope for any active provider
probe, then returns only after:

- the session id and canonical writers are allocated
- bounded processing and projection consumers are registered
- Deepgram is connected and ready
- selected sources start last and are subscribed and acknowledged
- rollback ownership is established

The frontend renders lifecycle state; it does not infer success from scattered
booleans.

## Information hierarchy

### Global chrome

- wordmark and current session name
- foreground Ready, LiveNow, Review, or Inspect label
- independent active-session lifecycle and persistent Stop control
- compact storage and route trust indicators
- Settings and session library

### Main canvas

- Ready: centered preflight deck and route spine
- Live: notes dominant, transcript adjacent or collapsible
- Review: notes and transcript with temporal navigation
- Inspect: contextual drawer or lower workbench

### Temporal spine

The signature interaction extends the existing `SeekTimeline`.

Ready:

- source -> capture -> provider -> local-memory route

Live:

- capture continuity
- final transcript events
- note and graph projection commits
- discontinuities and health degradation

Review:

- seek navigation
- speaker and source regions
- note/projection provenance
- revision and retcon markers

It is not a decorative waveform. Every mark corresponds to meaningful backend
state.

## Visual direction

Concept: **studio tape meets temporal memory**.

The app should feel like a precise field recorder that turns sound into
traceable memory, not a generic SaaS dashboard.

Core palette:

- Console `#0C171A`
- Deck `#15272B`
- Fog `#DCE8E5`
- Phosphor `#75D8C4`
- Record `#FF6F61`
- Provenance `#91A8FF`

Typography:

- Bricolage Grotesque for restrained headings and the wordmark
- Atkinson Hyperlegible Next for body, notes, and transcript
- Spline Sans Mono for time, source ids, sequences, and provenance

Fonts must be bundled locally. Existing token and Tailwind bridges from
ADR-0009 and ADR-0016 remain authoritative; the palette is expressed as
semantic tokens, not component literals.

Visual behavior:

- quiet surfaces with one strong recording accent
- border and tonal hierarchy before shadows
- motion only for state transition, continuity, and spatial orientation
- no gradient-heavy dashboard chrome
- no fake waveform or decorative pipeline animation
- no all-card layout; use cards only for bounded choices and recovery actions

## Rewrite boundary

Preserve:

- Zustand state and typed Tauri commands/events
- `NotesPanel`, `LiveTranscript`, `SeekTimeline`,
  `KnowledgeGraphViewer`, sessions, and settings controllers
- focus traps, skip link, roving tabs, phase announcements, reduced motion,
  tray behavior, and global shortcut support
- semantic token and Tailwind infrastructure

Rewrite or recombine:

- `App.tsx` shell and phase composition
- `ControlBar`
- workspace layout styles
- Express Setup and Settings overview
- health, route, and storage recovery placement

The rewrite is a composition and state-contract change, not a discard of
working data components.

## Content rules

- Use user goals: **Start note session**, **Stop and save**, **Review memory**.
- Avoid exposing internal stage names until Inspect.
- Say where data goes using provider and boundary names, not vague privacy
  claims.
- Never show Ready when storage cannot durably accept canonical events.
- Never dismiss a stopped-writer or storage-full state back to healthy.
- Never label a provider available from credential presence alone.
- Call Ready-time configuration a planned route; reserve observed route for
  audited session evidence.
- Deferred providers say **Planned** or **Not in MVP**, never **Use this mode**.

## Accessibility and responsive behavior

Required:

- complete keyboard flow and visible focus
- 200 percent zoom without two-dimensional scrolling for primary actions
- forced-colors support for status and temporal marks
- reduced-motion equivalents
- one coherent live announcement stream, not nested or per-second chatter
- NVDA validation on Windows and VoiceOver validation on macOS
- phase and durability changes announced with stable, humanized text

At narrower desktop widths:

- Ready remains one column
- Live prioritizes notes; transcript becomes a toggleable secondary pane
- Review uses one content column plus a collapsible timeline/details region
- Inspect becomes a full-height drawer

## Failure and recovery

The shell needs first-class states for:

- source removed or permission denied
- rsac startup timeout or fatal exit
- Deepgram rejection or disconnect
- LLM unavailable while transcript remains healthy
- projection backlog or decode failure
- raw, subscriber, processing, or consumer drop
- storage low, full, writer failed, or snapshot lag
- partial session recovery

Capture may continue during a non-essential LLM outage if transcript events stay
durable and the UI states that notes are delayed. Capture must stop or visibly
enter a blocked recovery state when canonical transcript persistence is lost.
RecoveryRequired retains exact residual and pending state plus explicit retry,
export, or safe-stop actions; it never offers a cosmetic dismiss-to-healthy.

## Acceptance evidence

- no-key, partial-key, rejected-key, provider-down, permission-denied,
  storage-full, and happy-path fixtures
- one action starts capture and Deepgram; any second-stage failure rolls back
- the first captured sample is consumed or an explicit discontinuity accounts
  for it
- Review A while capture B remains live keeps B's Stop, health, and durability
  controls available and never mutates A or B ownership
- passive Ready performs no egress; active probes have a draft audit owner
- sample preview remains local, transient, and absent from session artifacts
- continuous speech yields useful automatic notes before a pause
- stop and restart restore transcript, notes, graph, route, speakers, and
  timeline deterministically
- deferred providers cannot be selected or started from any UI or IPC route;
  inspection, credential maintenance, diagnostics, stop, and cleanup remain
  available
- Ready, LiveNow, Review, and Inspect screenshots at 1440, 1024, and 768 pixels in
  light/dark and idle/loading/error states
- keyboard, zoom, forced-colors, reduced-motion, NVDA, and VoiceOver passes
- packaged Tauri smoke on Windows, macOS, and Linux for permission, capture,
  tray, shortcut, export, and restart
