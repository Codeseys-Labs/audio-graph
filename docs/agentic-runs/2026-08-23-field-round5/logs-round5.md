# Field Round 5 — Logs Angle (build 376d3e5, 2026-08-23)

Files analyzed:
- `audio-graph.log` (active, 3,490 lines / 2.2MB) — starts 22:25:38, ends 22:35:15.424, session `57cfc64e`
- `audio-graph-20260823-222538.log` (archived, 14,508 lines / 46MB) — spans 09:20:39–16:03:52, session `d97bfcc3` (this is the same session ID cited in seed `104f`'s round-4 baseline)

## Verdict

**The crash left no diagnostic trail in the Rust backend log.** The active log ends at `22:35:15.424` immediately after a clean, successful `stop_capture` sequence (device + app audio sources both stopped, Deepgram closed gracefully) — there is no panic, no ERROR line, no partial/truncated final line (file ends on a clean `\n`), and critically **no "Graceful shutdown" cascade** like the one that closes out the archived 46MB log. The absence of any shutdown sequence after a normal-looking stop is itself the crash signature: this app logs an explicit multi-line graceful-shutdown sequence on exit, and that sequence never ran.

More importantly: **the code path the maintainer says triggered the crash (opening a different session, then the timeline tab) has zero log instrumentation.** `load_session_impl` (src-tauri/src/commands.rs:7604-7690) contains no `log::info!/debug!/warn!` calls anywhere in its body — not on entry, not on any of its branches, not on return. So whether that call even started, got partway through, or crashed the frontend without ever reaching Rust, is unknowable from this log. This is a genuine blind spot, not an absence-of-evidence-means-no-crash situation.

Secondary corroboration: `AppData/Local/com.rsac.audiograph/EBWebView/Crashpad/reports/` is empty — no WebView2/Chromium crash dump was captured either, which is consistent with either a Rust-side hard abort (SIGABRT/access violation that Crashpad wasn't hooked to catch) or a crash in the Tauri/Rust process itself outside the webview renderer.

Both 586b (diarization honesty) and 626c (notes-outline projection) machinery are confirmed live and firing normally throughout the active session, right up until the crash. WARN-level noise exists in both files but is entirely non-fatal (0 ERROR lines in either file) and matches previously-known projection-repair classes. The question/chat pipeline (items 5–6) is **completely unobserved by the backend logger** — zero log lines for agent proposals, live-assist cards, or chat in either file — so round-4-vs-round-5 fragment-rate comparison cannot be done from logs.

## 1. CRASH — last structured events before the log goes silent

Active log, lines 3455–3490 (last 20 structured events, 22:35:09.403 → 22:35:15.424):

| Line | Time | Event class |
|---|---|---|
| 3456 | 22:35:09.403 | Projection job applied (Notes) |
| 3457 | 22:35:09.414 | Projection scheduler completion (Notes) |
| — | 22:35:09.795 | Deepgram streaming: chunk-count heartbeat |
| — | 22:35:09.890 | Graph delta emitted |
| — | 22:35:12.996 | Deepgram streaming: chunk-count heartbeat |
| 3480 | 22:35:15.092 | `stop_capture` called (device source) |
| 3481 | 22:35:15.096 | Capture stopping (device) |
| 3482 | 22:35:15.096 | Capture thread exiting (device) |
| — | 22:35:15.105–.106 | WASAPI thread exit (device) |
| 3486 | 22:35:15.142 | Capture stopped confirmation (device) x2 |
| 3487 | 22:35:15.144 | `stop_capture` called (app source) |
| 3488 | 22:35:15.149 | Capture stopping (app) |
| — | 22:35:15.149–.160 | Capture thread exit + WASAPI exit (app) |
| — | 22:35:15.195 | Capture stopped confirmation (app) |
| — | 22:35:15.219 | Audio mixer stopped; Deepgram channel disconnect (user-initiated) |
| — | 22:35:15.333–.334 | Deepgram close-drain, session end (`UserRequested`), receiver exit — ASR segments=158, diarized=158 |
| — | 22:35:15.355 | Deepgram audio sender exiting, chunks sent=17,366 |
| 3489 | 22:35:15.418 | `projection_job.flush` elapsed_ms=60 |
| **3490 (last line)** | **22:35:15.424** | `Stopped capture for source: app:51704` |

**File ends exactly here.** No mid-line truncation (confirmed via hex dump — final byte is a clean `\n`), no next-launch log file exists in the logs directory (no `audio-graph-*.log` newer than this active file), and `EBWebView/Crashpad/reports/` is empty. The 8-second window from `stop_capture` to end-of-log is a clean recording-stop, not a crash in progress — the crash (per maintainer report: opening a different session + timeline tab) happened *after* this point and produced not one single log line, consistent with the Rust-side instrumentation gap in `load_session_impl` noted above, or a crash that occurred in a code path with no logging at all.

## 2. WARN/ERROR sweep

**ERROR count: 0 in both files.** All abnormal signal is WARN-level.

**Active log (12 WARN, lines: 100, 731, 865, 887, 905, 2186, 2198, 2308, 2586, 2900, 2919, 2925):**
- 1x credential shadowing (line 100, `openai_api_key` plaintext file entry shadows keychain value — pre-existing, unrelated to this round's fixes)
- 4x `StaleBasis`/`MissingCurrentSpan` projection apply failures (lines 731, 2308, 2919, 2925)
- 2x `StaleSequence` projection apply failures (lines 2186, 2198)
- 2x `MissingGraphNode` — an edge referenced a `Speaker 1` node id that did not exist in the graph at apply time (lines 2586, 2900)
- 3x `Notes` generation failure, all the same self-perpetuating bug: the LLM's projection patch duplicates a `reorder_note` id, and the automated repair attempt duplicates a *different* id in the same patch, so repair never converges (lines 865, 887, 905 — ids `sec-12`→`sec-13`, escalating across 3 consecutive job attempts ~11s apart)

**Archived 46MB log, session `d97bfcc3` (29 WARN):**
- 1x same credential-shadowing WARN (line 100)
- 17x `route attempt is not a usable completion ... retry_class=UnusableCompletion` against `route.openrouter` — dominant class, all clustered in the first hour (09:29:03–10:19:32); zero after that because live capture itself stopped sending audio at ~10:19:23 (Deepgram chunk counter last increments at 109,900 chunks, line ~14073) — the rest of the 6.5h file is idle-app browsing, not active transcription
- 3x `OpenRouter extraction failed` — JSON schema misses on `extract_entities` (missing `source`/`target` fields), lines 10236, 13692, 14122
- 3x same duplicate-id `reorder_note`/`upsert_graph_edge` repair-loop-doesn't-converge bug, different ids each time (`note-6007`, `note-90016`, `note-21099newZ2/Z3`) — same signature as the active log's 3 instances, confirming this is a standing/reproducible bug class, not a one-off
- 1x `MissingGraphNode` (line 2090, edge→node dangling reference, different node than the active log's)
- 1x invalid graph-edge weight (out-of-range numeric value) rejected, repair also duplicated an id (line 6236)
- 1x `upsert_graph_node` rejected because cited evidence span fell outside the patch's basis-covered set (grounding-invariant enforcement working as designed, line 10114)
- 1x plain `StaleSequence` (line 905)

No new WARN/ERROR class exists in round 5 that wasn't already present in round 4's known signature set; the duplicate-id repair-loop-doesn't-converge pattern is now confirmed present in **both** sessions (3 instances each), which elevates it from "seen once" to "reproducible."

## 3. 586b (diarization honesty) verification

Confirmed firing correctly in the active log:
- Line 412 (22:25:59.772, INFO): `Neural diarization engine (`diarization` feature) not compiled into this build — using Simple backend.`
- Line 413 (22:25:59.772, INFO): `DiarizationWorker created (backend=Simple, threshold=0.7, max_speakers=10, gap=2s)`

This is exactly the 586b honesty behavior working as intended — the build states plainly it's running the Simple backend rather than silently degrading.

The separate `Degraded` pipeline-status WARN path (`log::warn!("Diarization degraded ({}): {}", ...)` at src-tauri/src/speech/mod.rs:804, wired to wire codes `engine_not_compiled` / `clustering_assets_not_downloaded`, src-tauri/src/speech/mod.rs:1282-1283) **never fired in either log file** — grep for `Diarization degraded`, `engine_not_compiled`, `clustering_assets_not_downloaded` returns 0 hits in both files. The 8 call sites for `apply_diarization_degradation` (src-tauri/src/speech/mod.rs:5206, 5496, 5910, 6766, 8177, 8431, 8694) sit behind a local-ASR-worker code path (`make_diarization_config` at the whisper-worker construction site) that this session's cloud-Deepgram-based transcription flow does not appear to traverse — the INFO honesty line (item above) and the WARN degradation line are two different emission sites, and only the former fired this round.

Speaker-label stats: only **one distinct speaker label** appears anywhere in the active log's graph references — `"Speaker 1"` (from the two `MissingGraphNode` WARNs at lines 2586/2900, where an edge tried to reference that node id and it wasn't present in the graph yet). No `"Speaker 2"` or higher ever appears. Worth flagging to whoever owns diarization QA — can't tell from logs alone whether that reflects a single-speaker source, a Simple-backend clustering limitation, or a labeling bug, since the log has no per-utterance speaker-assignment trace.

## 4. 626c (notes-outline projection) verification

`Projection job movement counts` fires reliably every job cycle in both files (212 lines in the active log alone) with the `no_op_filtered` / `notes_outline_chars` / `notes_outline_entries` triple the round-5 brief asked about. Active-session end state (last applied jobs, lines 3450/3456):
- Graph: `node_count=180`, `edge_count=303`, `last_sequence=111`
- Notes: `note_count=79`, outline `entries=74` / `chars=5,066`, `last_sequence=103`

Aggregate for the active session: 343 total `Projection job applied` lines (Notes+Graph combined), `no_op_filtered` sums to 24 across the 212 movement-count lines (~11% of cycles filtered at least one no-op movement), 8 apply failures (the WARN set above) and 3 generation failures (the duplicate-id repair loop). No sign of outline-tail regression — outline chars/entries grow monotonically alongside note_count with no resets or negative deltas observed.

## 5. Question pipeline / fragment-rate comparison

**Cannot be measured from either log file — the round-5 brief's ask is a blind spot for the logs angle.** Grep for `question`, `AgentProposal`, `live_assist`/`LiveAssist` across both files returns **zero hits** in every case. Cross-checking against source (src-tauri/src/commands.rs:3793-4160, src-tauri/src/events.rs:427-470) confirms why: the entire live-assist-card / agent-proposal creation, approval, and dismissal code path has no `log::info!/debug!/warn!` calls anywhere near it. Round 4's cited numbers (84.5% fragment rate on a 110-item population, 74 pending cards piled up in session `d97bfcc3` — per seed `audio-graph-104f`/`audio-graph-83cc`) must have been derived from session-artifact/DB introspection, not from these log files, since this session's own log (the same `d97bfcc3` session, archived as the 46MB file analyzed here) contains no trace of card creation at all. **Hand-off note:** whoever verifies fragment-rate change for round 5 needs the session-artifact/materialized-notes data (or a DB query), not the log files — flagging this so the task isn't silently marked "unverified" without explanation.

## 6. Chat / Ask-AI

Zero literal `chat` mentions in either log file (0/0). This is consistent with — but does not independently confirm beyond source-reading — the maintainer's complaint and epic `83cc`'s own characterization that "Ask AI" today dismisses the card and dumps text into the ordinary streaming chat with no dedicated backend wiring: there is genuinely no backend log signal for any chat-adjacent action, so the backend logger cannot distinguish "chat was never touched" from "chat was touched but nothing logs it." Same instrumentation gap as items 5 and the crash in item 1 — this is a recurring theme across the round-5 ask, not three separate coincidences.

## Seed-shaped findings

1. **Crash produces zero backend log evidence because the triggering command has no instrumentation** (P1). Evidence: `load_session_impl`, src-tauri/src/commands.rs:7604-7690, contains no log macro calls; active log ends cleanly at line 3490 (22:35:15.424) with no shutdown cascade and no subsequent log file created. Suspected root cause: `load_session` and its downstream frontend render path (SeekTimeline/timeline tab) are the least-observed command in the app relative to their blast radius — a targeted `log::debug!` at entry/exit of `load_session_impl` plus a frontend error boundary around the timeline tab would turn this into a diagnosable crash next time instead of a silent one.

2. **Duplicate-ID projection-repair loop that cannot converge is now confirmed reproducible across two independent sessions** (P2). Evidence: active log lines 865/887/905 (ids `sec-12`→`sec-13` across 3 consecutive ~11s-apart attempts) and archived log (ids `note-6007`, `note-90016`, `note-21099newZ2/Z3`, 3 separate instances over the session). Suspected root cause: the automated patch-repair step that fires after an invalid `reorder_note`/`upsert_graph_edge` patch reuses/re-derives an id in a way that collides with itself, so repair regenerates the same class of error it was trying to fix rather than converging — this looks like a bug in the repair heuristic itself, not the original generation.

3. **The question/live-assist-card and chat subsystems are entirely unobserved by the backend logger** (P2, cross-cutting). Evidence: 0 hits for `question`/`AgentProposal`/`live_assist`/`chat` across 3,490 + 14,508 log lines; confirmed by source inspection that src-tauri/src/commands.rs:3793-4160 has no logging. Suspected root cause: this subsystem was built without telemetry from the start, which is exactly why round 4's fragment-rate number had to come from a non-log source and why round 5 can't verify movement on it from logs either — any fix to 104f or build-out of epic 83cc should add basic creation/dismissal counters so future rounds don't hit this same wall.

4. **Diarization `Degraded` status path is dead code in cloud-ASR sessions** (P3). Evidence: wire codes `engine_not_compiled`/`clustering_assets_not_downloaded` (src-tauri/src/speech/mod.rs:1282-1283) never appear in either log despite the build honestly running Simple-backend the whole time (line 412-413, active log); the 8 `apply_diarization_degradation` call sites (src-tauri/src/speech/mod.rs:5206 etc.) sit behind local-Whisper-worker construction, not the Deepgram-streaming path this session used. Suspected root cause: the 586b degradation signal was designed/tested against the local-ASR path and may never surface to the frontend at all for the more common cloud-Deepgram configuration — worth confirming whether that's intentional (Simple-on-cloud is considered "not degraded") or a coverage gap.

5. **Only one speaker label (`Speaker 1`) ever appears in the active session's graph** (P3). Evidence: active log lines 2586 and 2900, both `MissingGraphNode` WARNs referencing node id `"Speaker 1"`; no higher-numbered speaker label appears anywhere in either log. Suspected root cause: unknown from logs alone (could be a single-speaker recording, a Simple-backend clustering ceiling, or a labeling bug) — needs cross-checking against the session's persisted speaker-revision stream, which is outside the logs angle.
