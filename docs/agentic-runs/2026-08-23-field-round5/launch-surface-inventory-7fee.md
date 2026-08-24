# Launch-surface inventory for epic 7fee (home-first launch)

Read-only source trace, current `master`. No content quoted beyond field
names/counts/ids. **Overlap flag**: lane 4fa5 is actively changing
`SessionsBrowser.tsx`/session-loading in `store/index.ts` — this trace
touches both files (§3's `list_sessions`/session-index reads, §1's launch
default). Nothing here depends on 4fa5's specific in-flight edits; the facts
below are about the *index/list* path and the *destination-default* path,
not the `load_session` heavy-load path 4fa5 is understood to be fixing. Still
worth a fresh read against whatever 4fa5 lands before acting on this.

## 1. The current launch path

`src/main.tsx` mounts `<App />` inside an `ErrorBoundary`, no router, no
launch-time session/data fetch of any kind (src/main.tsx:60-67) — theme is
applied pre-paint (src/main.tsx:15), i18n is a side-effect import, and
that's the entire bootstrap.

`App.tsx`'s destination state is `nav.dest`, sourced from the `ShellNav`
store slice (src/store/shellNav.ts). The default is hardcoded:

```ts
export const DEFAULT_SHELL_NAV: ShellNav = {
  dest: "capture",
  sessionId: null,
  lens: "notes",
};
```
(src/store/shellNav.ts:123-127). **There is no persisted "last view," no
"open to sessions if you have any," no launch-time list_sessions call to
decide the default — the app always opens on `dest: "capture"`.**
`deriveWorkspaceView(nav)` (src/store/shellNav.ts:136-138) is a pure
passthrough (`nav.dest` literally *is* the workspace view since the
ADR-0046 destination collapse — see §4), so there is no hidden second layer
of routing logic to check.

**Capture does not auto-start.** What actually renders on the Capture
destination is a 3-way choice (App.tsx:1208-1223):
- `showGetStartedFallback` — `probeFailed && !isCapturing &&
  !samplePreviewActive && !loadedSessionId` — the credential-presence probe
  threw. Renders `GetStartedFallback`.
- `showPreflightCard` — same idle predicate, probe succeeded, plus
  `!hasAgentActivity` — renders `PreflightCard`
  (src/components/PreflightCard.tsx:1-40): a 3-row pass/fail checklist
  (Sources / Route / Storage) with fix actions, and **one "Start session"
  button** that calls the same `startCaptureAndTranscribe` action the
  NowStrip Start button uses. Nothing here starts recording automatically —
  a real user click is required.
- Otherwise — the live/reviewing bento workspace (App.tsx:617-684, the
  4-tile grid covered in the 83cc inventory).

**So "the app jumps straight into capture" is accurate as a *destination*
claim** (dest is always "capture" on launch, never "sessions", never a third
option) **but not as an "auto-recording" claim** — on a clean launch with no
active/sample/loaded session, what a user actually sees first is a
checklist-and-Start-button idle screen, not live transcription. Either way,
nothing about this screen shows past sessions or any cross-session content —
it's scoped entirely to *starting a new one*.

## 2. The navigation shell — no router, all store state

There is no client-side router (no `react-router`, no `TanStack Router`,
etc. — the Sessions destination gained a detail-lens URL-shaped structure
only as `nav.lens`/local `useState`, never a real route). Everything is
Zustand store state:
- `nav: ShellNav = { dest, sessionId, lens }` (src/store/shellNav.ts:99-106)
  is the ONE typed nav object. `dest` is `"capture" | "sessions"`
  (`ShellDest`, src/store/shellNav.ts:85) — frozen to exactly two values by
  the ADR-0046 shell collapse (see §4).
- `setWorkspaceView(view)` (src/store/shellNav.ts:170, 203-204) is what
  `App.tsx`'s destination-bar tab clicks and keyboard nav call
  (App.tsx:780+, `WORKSPACE_VIEWS = ["capture", "sessions"] as const`,
  App.tsx:220).
- Within Sessions, `lens` is a plain local `useState<DetailLens>("notes")`
  in `SessionsBrowser`'s `SessionDetail` component
  (src/components/SessionsBrowser.tsx:662), NOT synced to `nav.lens` in the
  store — `nav.lens` is a vestigial field from the pre-R4 three-tab
  reconciliation era (see the shellNav.ts module doc's R1/R4 history) with
  a documented "KNOWN GAP" that nothing currently reads it as a live router
  (App.tsx's own graph-edge-focus-bridge comment, referenced in the SeekTimeline
  trace from the earlier 83cc inventory work).

**Net effect for 7fee**: there is no routing layer to extend — a home
surface has to be a third value of `ShellDest` (a real shell-level change,
see §4) or has to be squeezed into the existing "sessions" destination
somehow (e.g. as a new lens, or as the Sessions list's own empty/landing
state) rather than gaining its own URL or route.

## 3. What a home page would consume

### The sessions index — confirmed 4x undercount, and a fix that exists but isn't wired up

`list_sessions(limit: Option<usize>)` (src-tauri/src/commands.rs:7392-7398)
is a thin wrapper: `crate::sessions::load_index()`, optionally truncated —
`load_index()` (src-tauri/src/sessions/mod.rs:158-160) just reads
`sessions.json` and deserializes it (`load_index_checked`,
src-tauri/src/sessions/mod.rs:121-141); a missing file returns empty, a
malformed file gets backed up and returns an error (surfaced as
`Err(...).unwrap_or_default()` at the public `load_index()`, so a caller
sees an empty list either way, not a hard error). **There is no on-disk
reconciliation in this path at all** — it trusts the index file completely.

The round-5 session-artifacts trace (session-artifacts-round5.md,
"Round-4 `ab9d`-style index/on-disk divergence") measured this directly on
the current data: **`sessions.json` has 42 entries; scanning every artifact
directory for session-id-shaped filenames finds 209 unique session ids on
disk — 167 of them (mostly 0-byte transcript stubs) are not in the index at
all.** A home page built straight on `list_sessions` inherits this ~4x
undercount as-is.

**A fix already exists, unwired**: `recover_orphaned_sessions()`
(src-tauri/src/commands.rs:7930-7941) calls
`crate::sessions::rebuild_index_from_files()` — rescans transcript/graph
files under the data root and recovers missing index entries, returning a
`SessionRecoveryReport` (discovered/recovered/skipped/errors counts). **It
has zero frontend callers today** (no `recover_orphaned_sessions`/
`recoverOrphanedSessions` reference anywhere in `src/`). Any home-page
design that wants an honest session count should either call this on
launch/on-demand, or the panel should explicitly decide "42 as shown is
acceptable, orphans are abandoned stubs" — but that decision should be made
knowingly, not by omission.

### `SessionMetadata` shape (what each row actually carries)

`src-tauri/src/sessions/mod.rs:56-79`:
```
id, title: Option<String>, created_at: u64, ended_at: Option<u64>,
duration_seconds: Option<u64>, status: String ("active"|"complete"|"crashed"),
segment_count, speaker_count, entity_count, transcript_path, graph_path,
deleted: bool, deleted_at: Option<u64>
```
The frontend's `SessionRow` type (src/components/SessionsBrowser.tsx:161) is
just `SessionMetadata & { optimistic?: true }` — the `optimistic` flag
covers the one known race (`mergeSessionRows`,
src/components/SessionsBrowser.tsx:170-177: a `stopCapture()`-just-happened
row that hasn't round-tripped through `sessions.json` yet gets injected
client-side so the list doesn't show a gap for a few hundred ms). This is
already exactly the shape a home page's "recent sessions" list would want —
title, duration, status, three rough content counts (segments/speakers/
entities) — no new backend field is obviously missing for a *list* view.
What it does NOT carry: any excerpt/summary text, any thumbnail, any
per-session note/graph preview — a richer home-page card (vs. a plain list
row) would need a new read, not just this existing struct.

### Cross-session knowledge graph — does not exist

There is no cross-session graph aggregation anywhere in the codebase — no
command, no store action, no component. The literal string "knowledgebase"
*does* appear in this codebase already, but it means something narrower and
already-scoped: `ConversationModeControl.tsx`'s doc comment
(src/components/ConversationModeControl.tsx:7-8) and a matching comment in
`store/index.ts:2716-2717` use "knowledgebase" to mean **the current
session's own graph + notes** (the Notes-mode target that Converse-mode
talks to) — not a cross-session concept. **The epic 7fee ask ("the
cross-session knowledgebase") is a genuinely new capability, not an
existing feature that merely lacks a home-page entry point.** Every graph
surface in the app today — `KnowledgeGraphViewer`
(src/components/KnowledgeGraphViewer.tsx), the live bento graph tile
(`LiveGraphStrip`) — reads either the live session's `graphSnapshot`/
`materializedProjectionGraph` or one loaded session's `materialized_graph`
via `load_session`. Nothing merges, diffs, or links entities across two
different sessions' graphs. A "cross-session knowledgebase" view is new
backend work (at minimum: a way to enumerate/merge/dedupe entities across
N sessions' persisted graph files), not a frontend-only surfacing task.

## 4. Constraints a panel must know

- **`ShellDest` is frozen to exactly two values** (`"capture" | "sessions"`,
  src/store/shellNav.ts:85), and this is the *result* of a deliberate,
  recent collapse — ADR-0046 explicitly went from a three-tab shell
  (during/after/analysis) down to two destinations, and the shellNav.ts
  module doc narrates that collapse across four SHELL-R1→R4 tickets as
  settled, retired scaffolding, not an open question. **Adding a third
  "home" destination reopens that decision** — it is not a small addition
  alongside a stable two-destination shell, it is walking back a change
  this codebase just finished making deliberately. That doesn't make it
  wrong, but the panel should treat it as an ADR-0046-scope decision, not a
  routine feature add.
- **`WorkspaceTileId` is a frozen four-value union** (`"transcript" |
  "graph" | "document" | "agent"`, src/components/workspace/WorkspaceTile.tsx:14)
  — already documented in the 83cc chat-surface inventory. Not directly in
  7fee's way (a home surface isn't a bento tile), but relevant if the panel
  considers putting a "recent sessions" glance *inside* the capture
  workspace rather than as a separate destination — that would mean either
  reopening this contract too, or finding room in an existing tile's
  `headerSlot`/body, both of which are already tightly composed (see the
  83cc inventory's notes on the document/graph/agent tiles' header-slot
  dual-composition).
- **`SessionLens` is a frozen five-value union** (`"notes" | "transcript" |
  "timeline" | "graph" | "route"`, src/store/shellNav.ts:91-96) for the
  Sessions destination's detail view. If the panel's answer is "home lives
  as a new lens on Sessions" rather than a new destination, note that
  `askAvailable(lens)` (src/components/SessionsBrowser.tsx:225-227)
  currently hardcodes exactly which lenses get the Ask/chat aside (notes and
  graph only) — a new lens starts with no Ask access unless deliberately
  added.
- **`reviewLockedWhileLive`**: the entire Sessions destination's detail view
  (list browsing may still work — this specific lock is on `SessionDetail`,
  src/components/SessionsBrowser.tsx's `reviewLocked` render branch) is
  unreachable while `isCapturing`/`isTranscribing`, replaced with a
  lockout message. A home page proposed as living inside/near Sessions
  would inherit this: if the design wants a home surface visible *during* a
  live capture (e.g. a persistent way to jump to another session while
  recording), that is explicitly not how the Sessions destination behaves
  today — "concurrent Live+Review is not delivered" is a stated ADR-0046
  design decision, not an oversight.
- **The sessions-index undercount (§3) is a pre-existing, silent data-
  quality gap**, not something 7fee introduces — but a home page is the
  first UI surface that would make "42 vs 209" *visible and prominent*
  rather than buried in a list nobody scrolls to the bottom of. Worth
  deciding deliberately (wire up `recover_orphaned_sessions`, or explicitly
  scope orphans as out-of-scope) rather than shipping a home page that
  looks broken/incomplete on any real user's data directory.

---

## Conductor verification note (2026-08-24)

Claim 3 above is corrected: `recover_orphaned_sessions` is NOT unwired. It has a store action (`recoverOrphanedSessions`, src/store/index.ts:3419-3431) and a manual UI trigger (`handleRecover`, src/components/SessionsBrowser.tsx:340-350, surfacing a recovery summary via `sessions.recoverySummary`). The real gaps for 7fee are therefore: (a) recovery is MANUAL, not automatic at startup or before a home page renders; (b) orphan QUALITY is unaddressed — 199 of 209 on-disk transcript ids are 0-byte (seed ab9d), so blind adoption of 167 orphans may populate a home page with junk entries; rebuild_index_from_files needs an adopt-or-quarantine policy first. All other claims in this document were spot-checked and held (ShellDest frozen two-value union, DEFAULT_SHELL_NAV hardcoded to capture, list_sessions as thin index wrapper, no cross-session graph aggregation anywhere).
