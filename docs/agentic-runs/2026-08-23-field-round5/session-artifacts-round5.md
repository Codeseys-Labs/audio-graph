# Field Round 5 — Session Storage Angle (build 376d3e5, 2026-08-23)

## Verdict

**The crash is explained by an already-open, already-diagnosed bug (`audio-graph-cfa1`, filed today 2026-08-23T18:02, P1, status `open`) tipping over into an actual app crash for the first time.** The non-live session the maintainer opened tonight is `d97bfcc3-c902-4e08-8119-6149f7ca6db6` — the exact session `cfa1` cites as its field evidence. Selecting that session row calls the frontend's `loadSession`, which invokes the backend `load_session` command (`src-tauri/src/commands.rs:7599`). That command deserializes `graphs/d97bfcc3….materialized.json` (**156,579,416 bytes / 156.6 MB**) and, because a canonical projection stream exists, also replays the **33,306,682-byte (33.3 MB)** `projections/d97bfcc3….events.jsonl` plus loads the **19,063,321-byte (19.1 MB)** `notes/d97bfcc3….json` — all synchronously, in one command call, before the timeline tab (`build_session_timeline_cmd`, which is comparatively light — 2.1 MB + 463 KB + 932 KB for this same session) ever gets a chance to render. `loadSession` fires `loadSessionTimeline` only after it resolves (`src/store/index.ts:3355`), so "opening a different session, then the timeline tab" is exactly the UI sequence that walks straight into the heavy `load_session` call first.

Root cause of the bloat (confirmed independently on-disk, matching `cfa1`'s own numbers almost exactly): every graph node/edge's `basis.span_revisions` list is never pruned or deduplicated across the session — it just keeps re-embedding the growing history of ASR/diarization revisions. `d97bfcc3` has 1,336 nodes + 1,814 edges carrying **1,360,895 total `span_revisions` entries** between them (605,713 on nodes, 755,182 on edges); one node alone carries 933 entries despite a `summarized_through_revision: 38` marker that is tracked but never used to actually truncate the vector. All of this accumulated in under **59 minutes of active recording** (09:20:39–10:19:23 local) — after that, live capture itself went idle for the rest of the 6h43m session (per the sibling logs-round5.md finding), so the bloat is not a multi-hour marathon artifact, it's a fast, easily-reproduced-in-one-sitting growth curve.

**No diagnostic trail exists for tonight's crash anywhere in session storage.** `~/.audiograph/crashes/` contains only two crash reports, both from 2026-06-01 (a `tokio` nested-runtime panic in an `0.1.0-rc.1` build — unrelated, stale). Nothing was written tonight. Combined with the sibling agent's finding that `audio-graph.log` ends cleanly with no panic/ERROR/shutdown cascade, this means whatever killed the process on `load_session` bypassed the Rust panic hook entirely — consistent with an OOM kill or an unresponsive-process termination while deserializing/replaying ~50 MB of JSON synchronously, not a caught panic.

**This is not a historical-only risk — the live session already shows the identical curve.** The currently-active session `57cfc64e-adc2-4031-a2da-4d1d8ad686e4` (22:25:38–22:35:15, ~9.5 minutes, 158 ASR segments) already has a **4,792,449-byte (4.79 MB)** materialized graph, and its single most-revised node already carries `span_revisions: 158` (i.e., essentially every revision the session has seen so far), with `summarized_through_revision: 12` — same untruncated-marker pattern as `d97bfcc3`. Any session run for an hour or more is on track to reproduce tonight's crash.

## Evidence

### 0. Where session storage actually lives (correction to the assigned starting paths)

Neither `AppData/Roaming/audio-graph/` nor `AppData/Local/` (Roaming or Local `com.rsac.audiograph`) hold session artifacts:
- `AppData/Roaming/audio-graph/` holds only `credentials.yaml`/`credentials-state.yaml` (untouched per hard rule) and `logs/`.
- `AppData/Roaming/com.rsac.audiograph/` holds `config.yaml`, `settings.json`, and the local Whisper model.
- `AppData/Local/com.rsac.audiograph/` is entirely WebView2/EBWebView browser-engine cache (no app data).

Per `src-tauri/src/user_data.rs:14-16`, the data root defaults to `dirs::home_dir().join(".audiograph")` unless `AUDIOGRAPH_DATA_DIR` is set — on this Windows box that resolves to `C:\Users\bbala_n314ugx\.audiograph`, i.e. **`/mnt/c/Users/bbala_n314ugx/.audiograph/`**, confirmed by every log line's `transcript_writer.final_flush file="C:\\Users\\bbala_n314ugx\\.audiograph\\transcripts\\..."`. Subdirs: `sessions.json` (index), `transcripts/`, `projections/`, `graphs/`, `notes/`, `usage/`, `ledgers/`, `live_assist/`, `crashes/`.

### 1. Session identification

- **Live session:** `57cfc64e-adc2-4031-a2da-4d1d8ad686e4`, started 22:25:38 local (`audio-graph.log:2`), `sessions.json` status `active`, `created_at` 1787549138564 ms.
- **Crash-target (prior) session:** `d97bfcc3-c902-4e08-8119-6149f7ca6db6`, started 09:20:39 local that morning, gracefully shut down 16:03:52 (`audio-graph-20260823-222538.log` last lines), restored on relaunch as "prior run … 0 turns, 0 total tokens" (`audio-graph.log:5`). This is also the exact session id named in the already-filed seed `audio-graph-cfa1`'s description and in the `heading_level` doc comment at `src-tauri/src/projection_llm.rs:126` ("field session d97bfcc3's 'D6' finding") — i.e. this session has been reused as field-test evidence across multiple rounds and has simply grown too large.

### 2. Per-session artifact inventory

`d97bfcc3` (crash target):

| Artifact | Path (relative to `~/.audiograph`) | Size | Last write |
|---|---|---|---|
| Transcript snapshot | `transcripts/d97bfcc3….jsonl` | 253,973 B | 16:03 (session end) |
| Transcript events | `transcripts/d97bfcc3….events.jsonl` | 2,178,713 B | 16:03 |
| Speaker/diarization log | `transcripts/d97bfcc3….speaker.jsonl` | 463,743 B (939 lines, 0 JSON errors) | **10:19** |
| Projection events | `projections/d97bfcc3….events.jsonl` | **33,306,682 B** (1,215 lines, 0 JSON errors, max single line 61,813 B) | 16:03 |
| Scheduler queue | `projections/d97bfcc3….scheduler_queue.json` | 194 B | **10:19** |
| Raw graph snapshot | `graphs/d97bfcc3….json` | 932,849 B | 16:03 |
| **Materialized graph** | `graphs/d97bfcc3….materialized.json` | **156,579,416 B (156.6 MB)**, valid JSON, 1,336 nodes / 1,814 edges / `last_sequence: 610` | **10:19** |
| Materialized notes | `notes/d97bfcc3….json` | **19,063,321 B (19.1 MB)**, valid JSON, 371 notes | **10:19** |
| Data-movement ledger | `ledgers/d97bfcc3….movements.jsonl` | 1,681,995 B (2,480 lines, 0 JSON errors) | **10:19** |
| Live-assist cards | `live_assist/d97bfcc3….{current.json,jsonl}` | 55,006 B / 46,635 B | **10:17** |
| Usage | `usage/d97bfcc3….json` | 237 B | 09:23 |

The **10:17–10:19 freeze** across five independent artifact families (speaker log, scheduler queue, materialized graph, notes, movements ledger, live-assist) while transcript/projection *event* logs kept growing until 16:03 is fully explained (not a stall bug) by the sibling `logs-round5.md` finding that live audio capture itself went idle at 10:19:23 (Deepgram chunk counter frozen at 109,900 chunks) — the rest of the session was idle app-browsing with no new ASR revisions to materialize. The important fact for this angle is that **all 156.6 MB / 33.3 MB / 19.1 MB of bloat accumulated in the first ~59 minutes of active recording**, not over the full 6h43m wall-clock session.

`57cfc64e` (live, for comparison — same bug, smaller scale so far):

| Artifact | Size | Notes |
|---|---|---|
| Materialized graph | 4,792,449 B (4.79 MB) | 180 nodes / 303 edges, `last_sequence: 111`, total `span_revisions` across nodes+edges = 38,143; one node already at 158/158 revisions (`summarized_through_revision: 12`, unused for truncation) |
| Materialized notes | 913,199 B | — |
| Projection events | 1,438,794 B | — |
| Transcript/speaker/ledger/live-assist | 197–302 KB range | — |

### 3. Why `load_session` (not the timeline command) is the crash suspect

- `src-tauri/src/commands.rs:7599-7704` (`load_session_impl`): loads `graph_path` (`graphs/<id>.materialized.json` via `load_materialized_graph`), `notes/<id>.json`, and — since `canonical_projection_stream_exists` for `d97bfcc3` — runs `MaterializedProjectionState::replay_accepted_patches_with_history` over the full transcript-event + speaker + **33.3 MB projection-event** streams, synchronously, in one call.
- `src-tauri/src/commands.rs:7830-7855` (`session_timeline_for_admitted_session`, backing `build_session_timeline_cmd`): only replays `transcripts/<id>.events.jsonl` (2.1 MB), `transcripts/<id>.speaker.jsonl` (463 KB), and the **raw** `graphs/<id>.json` (932 KB) — explicitly *not* the materialized graph (`src-tauri/src/timeline.rs:18-25` documents this as deliberate). In isolation this command is not expensive for this session.
- `src/store/index.ts:3294-3355` (`loadSession`): calls backend `load_session`, and only on success fires `void get().loadSessionTimeline(sessionId)` (which calls `build_session_timeline_cmd`). `src/components/SessionsBrowser.tsx:321` calls `loadSession(row.id)` on session-row selection — i.e., simply clicking the non-live session row (before or regardless of clicking the "Timeline" tab specifically) already triggers the heavy `load_session` path.

### 4. No crash artifact from tonight

`ls ~/.audiograph/crashes/`: only `1780279221824.log` and `1780279224249.log`, both timestamped `2026-06-01T02:00:2{1,4}Z`, both a `tokio` "Cannot start a runtime from within a runtime" panic against `App version: 0.1.0-rc.1` — a stale, unrelated bug from an old build. Nothing dated 2026-08-23. This corroborates the sibling logs-round5.md finding that the Rust panic hook never fired tonight.

### 5. Round-4 `ab9d`-style index/on-disk divergence — recounted on current data

`sessions.json` has **42 entries** (33 `complete`, 8 `crashed`, 1 `active`), all 42 resolvable to on-disk files (0 index entries missing from disk). But scanning every artifact directory for distinct session-id-shaped filenames turns up **209 unique session ids** on disk — **167 of them are not in the index at all**. Of the 209 ids' main `transcripts/<id>.jsonl` files, **199 are 0 bytes**; 176 of those 199 empty ones are also the un-indexed orphans (consistent with aborted/never-started sessions that got a stub transcript file but never made it into the index). The remaining 23 empty-transcript files **are** indexed. Of those, **3 indexed sessions have `segment_count > 0` in the index metadata (13, 23, 14) while their legacy `.jsonl` transcript is permanently 0 bytes** (their `graphs/<id>.json` does have real content) — `27486481…` (complete), `353982de…` (status `crashed` in the index itself), `5b4fd2e2…` (complete). All three predate this build by months (May 29 – Jun 20), so this specific legacy-empty-transcript mismatch is pre-existing and not attributable to the 376d3e5 build under test — it recurs identically to the round-4 `ab9d` signature simply because nothing has cleaned it up.

### 6. Schema drift check (new-build-only fields)

- `heading_level`: present in exactly 3 of 10 `notes/*.json` files, and only in sessions from **today** (`1784ad2a` — 6 occurrences, `57cfc64e` — 78, `d97bfcc3` — 370); zero occurrences in Aug 20/21/22 or Jul sessions. Confirmed backward-compatible: declared `Option<u8>` everywhere it's constructed (`projection_scheduler.rs:3246,3727`, `projection_eval.rs:869,1533`), so its absence on older sessions is not a parse hazard.
- `basis_currency_at_apply`: zero occurrences anywhere in `src-tauri/src/` or in any on-disk session artifact — not a real field in this codebase; no drift to report.

## Seed-shaped findings

1. **Title:** `load_session` crashes on session open because it synchronously deserializes+replays the exact 156 MB/33 MB/19 MB artifact set `audio-graph-cfa1` already diagnosed as unbounded growth.
   **Severity:** P1
   **Evidence:** `graphs/d97bfcc3….materialized.json` = 156,579,416 B (1,336 nodes/1,814 edges, 1,360,895 total `basis.span_revisions` entries); `projections/d97bfcc3….events.jsonl` = 33,306,682 B; `notes/d97bfcc3….json` = 19,063,321 B; `src-tauri/src/commands.rs:7599-7704` (`load_session_impl`) loads/replays all three unconditionally; `src/store/index.ts:3294-3355` + `src/components/SessionsBrowser.tsx:321` confirm session-row selection triggers this path before the timeline tab's own (lighter) loader.
   **Suspected root cause:** Same as `cfa1` — `ProjectionBasis.span_revisions` is embedded and cloned into every touched node/edge on every projection cycle (`projection_eval.rs:885`) with no pruning; `summarized_through_revision` is tracked but never used to truncate the vector, so `load_session`'s cost is `O(items × accumulated revisions)` and now large enough to crash the app outright, not just bloat disk. This finding should be attached to `cfa1` as new severity-escalating evidence (a concrete crash reproduction) rather than filed as a duplicate.

2. **Title:** Tonight's crash left zero forensic trail — the Rust panic-hook crash reporter never fired.
   **Severity:** P1
   **Evidence:** `~/.audiograph/crashes/` contains only two 2026-06-01 reports (unrelated `tokio` runtime panic, `0.1.0-rc.1`); nothing dated 2026-08-23. Cross-confirmed by sibling `logs-round5.md`: `audio-graph.log` ends cleanly mid-session with no panic/ERROR/shutdown cascade after the last `stop_capture` line.
   **Suspected root cause:** Whatever kills the process on a `load_session` call against an oversized artifact set (most likely an OS-level OOM kill, or a Windows "unresponsive process" termination from the long synchronous parse/replay blocking the command thread) does not go through `std::panic::set_hook`, so the existing crash-report writer never gets a chance to run. A fix for finding 1 should also close this gap (e.g., a size/row-count guard before attempting the load, or moving the replay off the path that can be killed silently).

3. **Title:** The live session already exhibits the same unbounded `basis.span_revisions` growth at a smaller scale — this is an active, worsening condition, not a one-off historical artifact.
   **Severity:** P2
   **Evidence:** `graphs/57cfc64e….materialized.json` = 4,792,449 B after only ~9.5 minutes / 158 ASR segments; one node's `basis.span_revisions` already has 158 entries (matching total segment count) despite `summarized_through_revision: 12`.
   **Suspected root cause:** Same as finding 1; flagged separately because it shows the bug reproduces fast (under 10 minutes) and will affect *every* session that runs long enough, not just the specific field-test session already known to `cfa1`.

4. **Title:** Session index (`sessions.json`) undercounts on-disk sessions by ~4x; round-4 `ab9d` empty-transcript-with-nonzero-segment-count mismatch still present on 3 legacy sessions.
   **Severity:** P3
   **Evidence:** 42 index entries vs. 209 unique session ids attested across on-disk artifact directories (167 orphans, 176 of which are 0-byte-transcript stubs); 3 indexed sessions (`27486481…`, `353982de…` [status `crashed`], `5b4fd2e2…`) have `segment_count` 13/23/14 in the index but a permanently 0-byte legacy `.jsonl` transcript, all dated May–Jun 2026 (pre-existing, not caused by build 376d3e5).
   **Suspected root cause:** Orphans are most likely sessions created (stub file touched) then abandoned before ever reaching the point where `sessions.json` gets an entry written; the 3 legacy mismatches predate the current transcript-event-log architecture and were never backfilled.

5. **Title:** `heading_level` schema drift confirmed but backward-compatible; no malformed-JSON risk found for the timeline loader.
   **Severity:** P3
   **Evidence:** `heading_level` present only in today's 3 sessions' `notes/*.json` (6/78/370 occurrences), absent from all older sessions; declared `Option<u8>` at every construction site in `src-tauri/src/projection_scheduler.rs` and `projection_eval.rs`. `basis_currency_at_apply` does not exist anywhere in the codebase or on disk.
   **Suspected root cause:** N/A — this is a clean, intentionally-nullable schema addition from the 626c notes-outline work; included here only because the brief asked for the check, and to close out the "does the timeline loader choke on schema drift" question with a negative result.