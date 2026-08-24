# Field Round 5 — Triage Synthesis (build 376d3e5, tested 2026-08-23 evening)

Inputs: `logs-round5.md`, `crash-code-path-round5.md`, `session-artifacts-round5.md` (all three lanes
reported; the session-artifacts lane, initially flagged dead, recovered and delivered a complete
artifact). All evidence below is cited to those artifacts' file:line/log-line pointers or verified
directly against the repo (read-only). Metrics/ids/error classes only — no transcript content.

---

## 1. CRASH VERDICT

**Root cause (HIGH confidence, all three lanes independently convergent): the crash is seed
`audio-graph-cfa1` escalating from "bloat" to "fatal" — clicking the session row for
`d97bfcc3-c902-4e08-8119-6149f7ca6db6` fired the synchronous, uncapped `load_session` command,
which must materialize a ≈208 MB JSON response (156.6 MB materialized graph + 33.3 MB projection
events + 19.1 MB notes + 2.1 MB transcript ≈ 2.03 M embedded basis objects) three times over
(Rust structs → Rust JSON String → JS parse) while also running an O(patches × events) replay
(1,215 × 4,697 ⇒ ~5.7 M `TranscriptEvent` clones + 1.14 M speaker-revision clones) on the Windows
main thread. The Timeline tab click is NOT the crash site — it is the last click on an
already-dying process.**

### Why the timeline tab is exonerated

- `SeekTimeline` invokes zero Tauri commands on mount — pure store reader, no `useEffect`, no
  `invoke` (`src/components/SeekTimeline.tsx:85-179`), render-capped at 200 blocks (`:73,120`).
- Its data command `build_session_timeline_cmd` fires from `loadSession`'s tail at
  `src/store/index.ts:3355` — i.e., at **session-row-select** time. The maintainer's sequence
  ("opened a different session, then the timeline tab") walks into the heavy `load_session` call
  at the first click; the tab click lands on a heap/message-pump already exhausted.

### Which session was opened

`SessionsBrowser` defaults to "newest" sort (`src/components/SessionsBrowser.tsx:89-99`); by
artifact mtime, the row directly under tonight's live session `57cfc64e` is `d97bfcc3` — the one
pathological session in the vault, and the exact session `cfa1` cites as evidence
(`session-artifacts-round5.md` §1). The `isCapturing || isTranscribing` refusal
(`src/store/index.ts:3295-3299`) did not fire because capture stopped at 22:35:15.

### Evidence chain (positive + negative)

| Fact | Source |
|---|---|
| `graphs/d97bfcc3….materialized.json` = 156,579,416 B; 1,336 nodes + 1,814 edges (= the 3,150 "facts") carrying **1,360,895** `basis.span_revisions` entries (~433/fact); one node at 933 entries despite `summarized_through_revision: 38` | session-artifacts §2; crash-code-path §2 (counts agree) |
| `load_session_impl` loads/replays all artifacts unconditionally in one response, no size guard anywhere (`src-tauri/src/commands.rs:7599, 7642, 7694-7703`; `persistence/mod.rs:1430-1440`) | crash-code-path §1.2 |
| `load_session` is a plain `pub fn` ⇒ `"sync"` ⇒ main-thread (verified against `tauri-macros-2.6.3/src/command/wrapper.rs:50, 263-266`; 98 `#[tauri::command]`, 0 `(async)` in commands.rs) | crash-code-path §1.3 |
| `replay_accepted_patches_with_history` rebuilds a fresh ledger per patch with `event.clone()` per event (`src-tauri/src/projections.rs:2825-2862`, called at `commands.rs:7643-7649`); the 156 MB on-disk graph is loaded then **discarded** at `:7678`, so both copies are resident | crash-code-path §1.2, H2 |
| Log ends cleanly at line 3490 / **22:35:15.424** (post-`stop_capture`, no shutdown cascade, clean `\n`), yet `sessions.json` + `graphs/57cfc64e….json` were written at ~22:35-22:36 — the app was alive after the last log line, then died silently | logs §1; crash-code-path VERDICT |
| **Rust panic family ruled out**: `~/.audiograph/crashes/` newest = 2026-06-01 (stale tokio panic, 0.1.0-rc.1); `panic` count 0 in tonight's log; no `panic="abort"` profile; all coordination locks `try_lock_*` | crash-code-path H7; session-artifacts §4 |
| Crashpad reports dir empty; zero log lines possible for this path (`load_session` etc. contain zero `log::` calls, `commands.rs:7478-7873`) | logs §1; crash-code-path H6 |

### Kill mechanism — top-2 with the discriminating probe

The proximate command is settled; the terminal mechanism is a two-way split (a hung-window
force-close by the user is a benign variant of either):

1. **WebView2 renderer OOM** materializing ~2 M nested JS objects → blank window that never
   returns (Tauri does not respawn a dead renderer). Pairs with the null-fallback ErrorBoundary
   (below) so even a caught V8 RangeError presents identically.
2. **Rust allocation-failure `abort()`** → bypasses `std::panic::set_hook` entirely → process
   vanishes with no crash report — exactly matching the empty `crashes/` dir.

**Discriminating probe:** Windows Event Viewer → Application log, 2026-08-23 ~22:35–22:50 — an
`Application Error`/`AppHang` for `audio-graph.exe` ⇒ Rust abort; a fault for
`msedgewebview2.exe` ⇒ renderer OOM. Secondary: ask the maintainer whether the window stayed
present-but-blank (renderer/boundary) or disappeared outright (process abort).

### Fix shape (layered — each layer is a filed seed below)

1. Compact the basis (cfa1 proper): store `(first_seq, last_seq)` range or hash pointer, actually
   truncate at `summarized_through_revision`.
2. Guard the load path: per-lens lazy fetch, hard byte ceiling with a typed refuse-error the UI
   renders. (Independent of 1 — even fixed writers leave 156 MB legacy artifacts on disk.)
3. Make the replay O(patches + events) with one forward cursor; move read commands off the main
   thread (`#[tauri::command(async)]` is signature-compatible).
4. Give the root ErrorBoundary a real fallback + instrument the read path so the next occurrence
   is diagnosable.

**Urgency multiplier:** this is not a legacy-artifact one-off. Tonight's 9.5-minute live session
already shows the identical growth curve — 4.79 MB materialized graph, one node at 158/158
span revisions with `summarized_through_revision: 12` unused (session-artifacts §2). Any ≥1-hour
session reproduces the crash: d97bfcc3's entire 156 MB accumulated in **59 minutes** of active
recording (09:20:39–10:19:23), not 6.5 hours.

---

## 2. FIX VERDICTS on build 376d3e5

### 626c (notes-outline projection) — HELD on the core; the id-repair sub-path remains broken

- Movement-count logging fired every cycle: 212 lines in the 9.5-min active session;
  `no_op_filtered` summed to 24 (~11% of cycles filtered ≥1 no-op). End state monotonic, no
  resets/negative deltas: notes=79, outline entries=74, outline_chars=5,066, graph 180 nodes/303
  edges, `last_sequence` 103/111. 343 applied jobs vs 8 apply failures + 3 generation failures
  (logs §4).
- Schema addition is clean: `heading_level` present only in today's 3 sessions (6/78/370
  occurrences), declared `Option<u8>` at every construction site — no parse hazard for older
  sessions (session-artifacts §6).
- **BUT id re-minting still fails to converge:** the duplicate-ID `reorder_note`/`upsert_graph_edge`
  repair loop reproduced 3× in EACH session — active log lines 865/887/905 (ids `sec-12`→`sec-13`,
  3 attempts ~11 s apart) and archived log (`note-6007`, `note-90016`, `note-21099newZ2/Z3`). The
  repair step re-derives an id that collides again, regenerating the error class it was fixing
  (logs §2). Verdict: outline projection held in the field; the repair heuristic needs its own seed.

### 586b (diarization honesty) — FIRED HONESTLY; the Degraded wire channel is dead code on the cloud path

- Honesty lines fired at session start: active log lines 412-413 (22:25:59.772, INFO) — "Neural
  diarization engine … not compiled … using Simple backend" + "DiarizationWorker created
  (backend=Simple, threshold=0.7, max_speakers=10, gap=2s)". No false "neural" banner anywhere;
  Deepgram close reported segments=158, diarized=158 (logs §1, §3).
- **BUT** the `Degraded` pipeline-status path (wire codes `engine_not_compiled` /
  `clustering_assets_not_downloaded`, `src-tauri/src/speech/mod.rs:1282-1283`) fired zero times in
  18k log lines — its 8 `apply_diarization_degradation` call sites sit behind local-Whisper-worker
  construction, which the cloud-Deepgram flow never traverses. The frontend degradation signal
  appears unreachable for the most common configuration (logs §3). Open question for the 586b
  owner: intentional ("Simple-on-cloud ≠ degraded") or coverage gap.
- Open observation: only one speaker label (`Speaker 1`) appears in the active session's graph
  references (log lines 2586/2900); needs a distinct-speaker count over
  `transcripts/<id>.speaker.jsonl` (939 lines exist for d97bfcc3) to classify.

### 104f (question fragments) — UNMEASURABLE this round; no movement can be claimed either way

- Round-5 fragment rate vs round-4's 84.5%: **cannot be computed.** The question/live-assist
  pipeline has zero backend telemetry — 0 hits for `question`/`AgentProposal`/`live_assist`/`chat`
  across 3,490 + 14,508 log lines, confirmed structural by source inspection
  (`src-tauri/src/commands.rs:3793-4160`, `events.rs:427-470` — no log calls near card
  creation/approval/dismissal) (logs §5). The artifacts lane inventoried `live_assist/` files
  (d97bfcc3: 55,006 B `current.json` / 46,635 B `.jsonl`, frozen 10:17) but performed no
  fragment-shape analysis, so the artifact-based measurement round 4 used was not reproduced.
- Nothing in 376d3e5 touches the question path (626c/586b are unrelated subsystems); the
  maintainer's "still incomplete fragments" report is consistent with no change shipped.
- **Where the fix must sit** (per the 2026-08-23 live-workspace design synthesis,
  `docs/agentic-runs/2026-08-23-live-workspace-design/synthesis.md` — W8/W9/R6): the agent tile's
  `admitToQueue` predicate is 104f's designated slot. Interim: the W9 `classifyQueueEntry`
  heuristic (confidence <0.5, normalized-title dupe collapse, locale-safe sentence-shape test — no
  English word lists) behind the Signal/All toggle; durable: a **backend quality field** on the
  proposal that replaces that function body ("one line, no tile change"). Neither has shipped.
- Precondition for round 6: creation/approval/dismissal counters (seed below) or a scripted
  `live_assist/*.jsonl` shape analysis, otherwise this stays unmeasurable.

---

## 3. CHATBOX (83cc) — gap CONFIRMED, with the exact mechanism

The maintainer's "the agent chatbox cannot be interacted with to ask questions" is the current
design, not a regression. Verified directly in source this round:

- **Live capture: the agent tile has no free-text input at all.** `AgentProposalsPanel` renders
  only per-card buttons — `agent.askAi` (`src/components/AgentProposalsPanel.tsx:369`) and
  `agent.dismiss` (`:376,396`); the feed section is explicitly read-only (`:421-427`).
- **"Ask AI" does not open a chat** — `askAgentProposal` (`src/store/index.ts:1810-1823`) strips
  the "Consider answering or linking this question:" prefix, **dismisses the card**, then routes
  the text through `sendChatMessage` — exactly epic 83cc's own characterization.
- **The only free-text chat input in the app is unreachable in practice.** `ChatSidebar` mounts
  solely inside `SessionsBrowser` (`src/components/SessionsBrowser.tsx:827`, doc comment `:223`:
  available as an "Ask" aside only there), and its input + send button are hard-disabled whenever
  `historicalReview = loadedSessionId !== null` (`src/components/ChatSidebar.tsx:34, 52, 192, 199`)
  — i.e., the moment a session row is clicked (which is what puts content on screen), typing is
  disabled. So: live view → no input exists; review view → input exists but is disabled.
- Backend logs corroborate the absence of any chat wiring: 0 literal `chat` mentions across both
  log files (logs §6).

Conclusion: 83cc's "unbuilt" statement stands; the new detail worth folding into the epic is the
`historicalReview` disable, which makes the chat input dead in effectively every state a field
tester encounters, and the `askAgentProposal` dismiss-then-pipe behavior, which destroys the card
the user wanted to discuss.

---

## 4. SEEDS TO FILE (deduplicated across all three lanes, most severe first)

Duplicates merged: logs#1 + crash-code-path#7 (read-path instrumentation); crash-code-path#1 +
session-artifacts#1/#2 (load_session crash, filed as the cfa1 update + one new load-path seed);
crash-code-path#6 + session-artifacts#3 (basis growth, folded into the cfa1 update);
session-artifacts#5 (heading_level) is a negative result — reported in §2, not seeded.

### S1 · P1 · UPDATE to `cfa1` — unbounded per-fact basis growth now crashes the app on session open (severity escalation + mechanism + live reproduction)
Evidence: `graphs/d97bfcc3….materialized.json` = 156,579,416 B = 1,336 nodes + 1,814 edges carrying
1,360,895 `basis.span_revisions` entries (~433/fact; one node at 933 despite
`summarized_through_revision: 38` — the marker is tracked but never truncates the vector); same
pattern in projections (495,114 `"span_id"`) and notes (165,916). All accumulated in 59 min of
active recording. Tonight's 9.5-min live session already at 4,792,449 B with one node at 158/158
revisions (`summarized_through_revision: 12` unused). Concrete crash reproduction 2026-08-23
~22:36 via `load_session` (see §1). Suspected root cause: `ProjectionBasis.span_revisions` embedded
and re-cloned into every touched node/edge each projection cycle (`projection_eval.rs:885`) with no
pruning ⇒ artifacts O(facts × spans). Remedy: store basis as `(first_seq, last_seq)` range or
content-hash pointer into the transcript log; actually truncate at `summarized_through_revision`;
add a persisted-artifact size regression gate. **Attach to cfa1 — do not file a duplicate.**

### S2 · P1 · NEW — `load_session` returns every session artifact in one uncapped synchronous invoke response (~208 MB for d97bfcc3)
Evidence: `src-tauri/src/commands.rs:7599` (`#[tauri::command] pub fn load_session`), `:7642`
(`load_materialized_graph`), `:7694-7703` (`Ok(LoadedSession{…})`); `src/store/index.ts:3306`;
`load_json` = `fs::read_to_string`, no size guard (`persistence/mod.rs:1430-1440, 2929-2932`); no
`MAX_*`/truncate in `commands.rs:7400-7900`. Payload materialized 3× (Rust structs, Rust JSON
String, JS parse ⇒ ~2 M JS objects). Negative evidence set: no crash report since 2026-06-01,
`panic`=0, log dead after 22:35:15.424 while disk writes continue to ~22:36. Root cause:
`LoadedSession` is an all-artifacts bundle with no cap/pagination/streaming/per-lens fetch. Remedy:
return metadata + transcript only; fetch notes/materialized-graph/projection-events per lens; hard
byte ceiling that refuses with a typed error the UI can render. Overlaps cfa1 (consumes its
artifact) but is independently necessary: legacy oversized artifacts remain on disk after S1 lands.

### S3 · P1 · NEW — `replay_accepted_patches_with_history` is O(patches × events) with a clone per event, on the main thread, inside `load_session`
Evidence: `src-tauri/src/projections.rs:2825-2862` (fresh `TranscriptLedger` per patch,
`event.clone()` per event with `received_at_ms <= patch.created_at_ms`, fresh `SpeakerTimeline`
likewise), called at `commands.rs:7643-7649`. For d97bfcc3: 1,215 × 4,697 ⇒ ~5.7 M
`TranscriptEvent` clones + 1,215 × 939 ⇒ 1.14 M speaker-revision clones per load. The 156 MB
on-disk graph loaded at `:7642` is discarded by `choose_materialized_graph` (`:7582-7591, 7678`) —
both copies resident simultaneously. Root cause: per-patch ledger rebuilt from scratch instead of
advanced incrementally over already-ordered patches. Remedy: one forward-cursor ledger + speaker
timeline ⇒ O(patches + events); references over clones; run off the main thread.

### S4 · P1 · NEW — root `ErrorBoundary` has no fallback: any render error blanks the entire app permanently, indistinguishable from a process crash
Evidence: `src/main.tsx:60-64` mounts `<ErrorBoundary><App /></ErrorBoundary>` with no `fallback`
prop; `src/analytics/ErrorBoundary.tsx:44-46` returns `this.props.fallback ?? null`; module doc at
`:1-11` claims the opposite ("renders a minimal fallback so a render crash does not leave a blank
window"). No specific throw found in `SeekTimeline` (formatTime NaN-safe, icon + i18n keys present,
`related_edge_ids` non-Option), but a V8 RangeError/OOM during the lens swap lands here. Root
cause: boundary wired for telemetry (`captureFrontendError`) not recovery. Remedy: real fallback
(error text + Reload via `window.location.reload()` + Back-to-Capture nav reset), reset `hasError`
on nav change. Independent of tonight's root cause — masks every future frontend fault in the field.

### S5 · P2 · NEW — every historical-session read is a main-thread `"sync"` Tauri command; no async escape hatch for disk work
Evidence: `src-tauri/src/commands.rs:7534` (`load_session_transcript`), `:7599` (`load_session`),
`:7781` (`export_session_bundle`), `:7869` (`build_session_timeline_cmd`) all plain `pub fn`; 98
`#[tauri::command]` / 0 `#[tauri::command(async)]` in commands.rs; semantics verified at
`tauri-macros-2.6.3/src/command/wrapper.rs:50, 263-266` (Blocking default ⇒ kind "sync" ⇒ inline in
the IPC handler on the win32 message pump). Root cause: heavy disk+CPU work shares the thread that
delivers input to the WebView2 host window ⇒ any >2 s read swallows the next click and gets the
window declared hung — the structural precondition for S2/S3. Remedy: `#[tauri::command(async)]`
on the four read commands (signature-compatible) + a lint asserting no sync command does
full-artifact reads.

### S6 · P2 · UPDATE to `67cd` — the 1,000-node cap protects only the live graph; the path `load_session` prefers is uncapped
Evidence: `MAX_NODES=1000`/`MAX_EDGES=5000` (`src-tauri/src/graph/temporal.rs:45,53`) enforced on
insert only; `load_from_file` (`:1011-1048`) re-applies neither; `snapshot()` (`:778-836`) emits
everything; `choose_materialized_graph` (`commands.rs:7582-7591, 7678`) makes the uncapped
materialized graph authoritative whenever a canonical projection log exists. Same session: live
graph 932,849 B (at cap) vs materialized 156,579,416 B (168×). Root cause: one eviction cap between
two representations, applied to the non-authoritative one. Remedy: cap/window the materialized
graph at materialization time; load path refuses above a ceiling with a typed error. **State in
cfa1/67cd explicitly: capping the live graph does not mitigate the load path.**

### S7 · P2 · NEW — duplicate-ID projection-repair loop cannot converge; reproducible 3× in each of two independent sessions (626c-adjacent)
Evidence: active log lines 865/887/905 (ids `sec-12`→`sec-13`, 3 consecutive attempts ~11 s apart);
archived log 3 separate instances (`note-6007`, `note-90016`, `note-21099newZ2/Z3`) — same
signature, different ids, both sessions. Root cause: the automated repair step for invalid
`reorder_note`/`upsert_graph_edge` patches re-derives an id that collides within the same patch,
regenerating the error class it was repairing. Remedy: make repair id re-minting globally unique
across the whole patch (not per-op), and cap repair attempts with a terminal skip + WARN so the
loop cannot ratchet. This is the surviving defect from the 626c verdict (§2).

### S8 · P2 · NEW — the historical-read path has zero instrumentation, so this crash class leaves no field trace (merged: logs#1 + crash-code-path#7)
Evidence: zero `log::` calls in `load_session_impl`, `session_timeline`,
`session_timeline_for_admitted_session`, `load_session_transcript`, `list_sessions`,
`session_export_bundle` (`commands.rs:7478-7873`) while neighbouring mutating commands log
(`delete_session :7889`, `restore_session :7898`, `recover_orphaned_sessions :7933`); tonight's log
counts for `load_session`/`build_session_timeline`/`list_sessions`/`Session files not found` =
0/0/0/0; last line 22:35:15.424. Remedy: entry/exit `log::info!` per read command with
`session_id`, per-artifact byte counts, response bytes, `elapsed_ms` (house style:
`projection_job.flush elapsed_ms=`), plus `log::warn!` above a response-size threshold.

### S9 · P2 · NEW — question/live-assist/chat subsystem is entirely unobserved by the backend logger; blocks 104f measurement
Evidence: 0 hits for `question`/`AgentProposal`/`live_assist`/`chat` across 3,490 + 14,508 log
lines; source-confirmed no logging near card creation/approval/dismissal
(`commands.rs:3793-4160`, `events.rs:427-470`). Round 4's 84.5% fragment rate came from a non-log
source; round 5 could not reproduce the measurement (live_assist artifacts exist —
d97bfcc3: 55,006 B — but no lane analyzed shape). Remedy: creation/approval/dismissal counters +
per-card kind/confidence log line; prerequisite for evaluating any 104f fix (whose designated slot
is the `admitToQueue` predicate per the 2026-08-23 live-workspace design, W8/W9/R6).

### S10 · P3 · NEW — diarization `Degraded` status path is dead code for cloud-ASR (Deepgram) sessions (586b follow-up)
Evidence: wire codes `engine_not_compiled`/`clustering_assets_not_downloaded`
(`src-tauri/src/speech/mod.rs:1282-1283`) never appear in either log despite the build running
Simple-backend throughout (honesty INFO at active log 412-413); the 8
`apply_diarization_degradation` call sites (`speech/mod.rs:5206, 5496, 5910, 6766, 8177, 8431,
8694`) sit behind local-Whisper-worker construction. Remedy: confirm intent — if Simple-on-cloud
should surface as Degraded, wire the status into the Deepgram session path; if not, document it in
586b and close.

### S11 · P3 · NEW — only one speaker label (`Speaker 1`) ever appears in the active session's graph
Evidence: active log lines 2586/2900 (`MissingGraphNode` WARNs referencing node id `Speaker 1`);
no higher-numbered label in 18k log lines. Open probe: count distinct `speaker` values in
`transcripts/57cfc64e….speaker.jsonl` and `…d97bfcc3….speaker.jsonl` (939 lines) to discriminate
single-speaker source vs Simple-backend clustering ceiling vs labeling bug. File only after the
probe, or file as a question-seed.

### S12 · P3 · UPDATE to `ab9d` — index/on-disk divergence recounted on current data; legacy mismatch persists; explicitly NOT the crash cause
Evidence: `sessions.json` = 42 entries (33 complete / 8 crashed / 1 active), all resolvable; disk
attests 209 unique session ids ⇒ 167 un-indexed orphans; 199/209 main transcripts are 0 bytes; 3
indexed sessions (`27486481…`, `353982de…` [crashed], `5b4fd2e2…`) carry `segment_count` 13/23/14
with permanently 0-byte legacy `.jsonl` (May–Jun 2026, pre-existing, not build 376d3e5). Also
record in ab9d: an empty-artifact session passes `session_has_any_artifact`
(`commands.rs:7522-7528`), folds an empty timeline and renders `SeekTimeline`'s empty state
(`:201-227`) — ab9d predicts "timeline shows nothing", not tonight's crash. Remedy: orphan-stub
cleanup on startup + index backfill or tombstone for the 3 legacy mismatches.

### S13 · P3 · NEW — `build_session_timeline_cmd` serializes every span to render 200 and re-parses the graph file `load_session` just read
Evidence: `src-tauri/src/timeline.rs:119-186` (uncapped `Vec<TimelineEntry>` with full text ⇒
4,697 entries for d97bfcc3); `src/components/SeekTimeline.tsx:120` discards all but the last 200
(`MAX_BLOCKS = TRANSCRIPT_WINDOW_SIZE = 200`, `src/constants/transcript.ts:14`); duplicate
live-graph parse at `commands.rs:7616` then `:7850` in the same click. Remedy: `limit`/`before_ms`
parameter on the fold command; reuse the sibling command's parsed graph.

---

## Round-6 verification checklist (so next round isn't blind)

1. Pull Windows Event Viewer Application entries for 2026-08-23 22:35–22:50 (crash-mechanism
   discriminator, §1).
2. Land S8/S9 telemetry before the next field build, or script a `live_assist/*.jsonl`
   fragment-shape analysis — otherwise 104f stays unmeasurable a third round.
3. After any S1/S2 fix: re-open d97bfcc3 from the Sessions browser as the regression test — it is
   the canonical pathological artifact set and reproduces the crash deterministically by size.
