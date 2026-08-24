# Round 5 — crash code path: opening a non-live session + clicking the Timeline tab

**Angle:** code-path walk (read-only) of the UI → Tauri command chain the maintainer hit when the app
died on 2026-08-23 evening. Build 376d3e5. Companion lanes: logs investigator, session-artifacts
investigator.

---

## VERDICT

**The Timeline tab click is not the crash site. It cannot be.** The Timeline lens
(`SeekTimeline`) invokes **zero** Tauri commands on mount — it is a pure store reader
(`src/components/SeekTimeline.tsx:85-96`). The only command that produces its data,
`build_session_timeline_cmd`, is fired once from `loadSession`'s tail at
`src/store/index.ts:3355`, i.e. **at session-row-select time, not at tab-click time**. There is no
effect, no lazy fetch, and no `Suspense` boundary on the timeline lens
(`src/components/SessionsBrowser.tsx:815`).

So the crash is the **delayed / terminal consequence of `load_session`**, whose single synchronous
main-thread invoke response for the session the maintainer most likely opened is
**≈208 MB of JSON carrying ≈2.03 million artifact-basis entries**. The Timeline click is the last
allocation on an already-exhausted heap, or the last click on an already-hung message pump.

Three independent negative facts kill the "Rust panic" family of explanations outright:

| Fact | Where | Meaning |
|---|---|---|
| `~/.audiograph/crashes/` newest file is `1780279221824.log`, mtime **2026-05-31 19:00** | Windows disk | the panic hook (`src-tauri/src/crash_handler/mod.rs:28`) never fired tonight |
| `grep -c panic` on tonight's log = **0** | `AppData/Roaming/audio-graph/logs/audio-graph.log` | no unwind, no abort message |
| every coordination lock on the read path is `try_lock_*`, never blocking | `src-tauri/src/persistence/session_artifact_manifest.rs:866-891` | no lock deadlock; a contended control plane returns `Err`, not a hang |

And one positive fact that shapes everything: **tonight's log stops dead at
`2026-08-23 22:35:15.424` ("Stopped capture for source: app:51704") and never resumes**, while
`sessions.json` and `graphs/57cfc64e….json` were still written at 22:36. The app was alive after the
last log line and then died silently — which is *exactly* what this code predicts, because
`load_session`, `load_session_transcript`, `build_session_timeline_cmd`, `list_sessions` and
`export_session_bundle` contain **zero `log::` statements** (compare `delete_session`, which logs at
`src-tauri/src/commands.rs:7889`). The entire historical-read path is unobservable.

---

## 1. The code path, exactly

### 1.1 UI chain

| Step | File:line | Note |
|---|---|---|
| Sessions destination renders the rail + detail | `src/components/SessionsBrowser.tsx:229` | mounts, refetches 200 rows (`:270-272`) |
| Row click | `src/components/SessionsBrowser.tsx:317-322` | `setNavSessionId(row.id)` then `void loadSession(row.id)` — fire-and-forget, **not awaited** |
| Lens tabs (Notes / Transcript / **Timeline** / Graph / Route) | `src/components/SessionsBrowser.tsx:206-212, 779-802` | `lens` is component-local `useState`, default `"notes"` (`:661`) |
| Timeline panel | `src/components/SessionsBrowser.tsx:815` | `{lens === "timeline" && <SeekTimeline />}` — plain conditional mount |
| `SeekTimeline` | `src/components/SeekTimeline.tsx:85-179` | reads `sessionTimeline`, `sessionTimelineLoading`, `sessionTranscriptEvents`, `speakers`; **no `invoke`, no `useEffect`** |

`SeekTimeline` is also bounded on the render side: `entries = (timeline ?? []).slice(-MAX_BLOCKS)`
with `MAX_BLOCKS = TRANSCRIPT_WINDOW_SIZE = 200` (`src/components/SeekTimeline.tsx:73,120`;
`src/constants/transcript.ts:14`). Worst case 200 lanes × 200 blocks total. **It is not a DOM bomb.**

### 1.2 Backend chain (both commands **synchronous**)

`loadSession` (`src/store/index.ts:3294-3362`) →

1. `invoke("load_session")` → `load_session_impl` — `src-tauri/src/commands.rs:7599-7704`
2. on success, `void get().loadSessionTimeline(sessionId)` (`:3355`) →
   `invoke("build_session_timeline_cmd")` → `session_timeline` →
   `session_timeline_for_admitted_session` — `src-tauri/src/commands.rs:7808-7873`

What `load_session_impl` reads and **returns in one response** (`commands.rs:7694-7703`):

| Field | Reader | d97bfcc3 on-disk size |
|---|---|---|
| `transcript` (+ ledger replay) | `:7613` | 2.1 MB events → derived segments |
| `graph` (live snapshot) | `:7616-7621` | 932 KB |
| `diarization_events` | `:7627-7629` | 464 KB |
| `projection_events` | `:7630-7639` | **33.3 MB** |
| `live_assist_cards` | `:7640` | 55 KB |
| `notes` | `:7641` → replayed | **19.1 MB** |
| `materialized_graph` | `:7642` → replayed | **156.6 MB** |

`load_materialized_graph` is `load_json` → `fs::read_to_string` of the whole file, no size guard
(`src-tauri/src/persistence/mod.rs:1430-1440`, `:2929-2932`).
**There is no cap, truncation, pagination, or streaming anywhere on this path** (grep for
`MAX_*`/`truncate` between `commands.rs:7400-7900` → nothing).

### 1.3 Sync = main thread (verified against the macro, not from memory)

`tauri-macros-2.6.3/src/command/wrapper.rs:50` sets `execution_context: ExecutionContext::Blocking`
as the **default**; `:263-266` maps `Blocking` → command kind `"sync"`, resolved inline in the IPC
handler (only `#[tauri::command(async)]` or an `async fn` gets `sync_threadpool`/`async`).

In this repo: **98** `#[tauri::command]` attributes in `commands.rs`, **0** occurrences of
`tauri::command(async)`, and 55 of the 98 are `async fn`. `load_session`, `load_session_transcript`,
`build_session_timeline_cmd`, `list_sessions`, `export_session_bundle` are **all plain `pub fn`** →
all run on the Windows main thread, i.e. inside the win32 message pump that also delivers clicks to
the WebView2 host window.

---

## 2. Disk evidence gathered (structure/size/count only — no content read)

`/mnt/c/Users/bbala_n314ugx/.audiograph/` (the real data root; `AppData/Roaming/audio-graph/` holds
only credentials + `logs/`, and `AppData/Local/audio-graph` does not exist):

```
graphs/d97bfcc3-….materialized.json   156,579,416 B   Aug 23 10:19
graphs/d97bfcc3-….json                    932,849 B   Aug 23 16:03
notes/d97bfcc3-….json                  19,063,321 B   Aug 23 10:19
projections/d97bfcc3-….events.jsonl    33,306,682 B   Aug 23 16:03   (1,215 lines)
transcripts/d97bfcc3-….events.jsonl     2,178,713 B   Aug 23 16:03   (4,697 lines)
transcripts/d97bfcc3-….speaker.jsonl      463,743 B                  (  939 lines)
graphs/57cfc64e-….materialized.json     4,792,449 B   Aug 23 22:35   ← tonight's live session
```

Embedded basis entries (counted via `grep -o '"span_id"' | wc -l`, no values read):

| Artifact | `"span_id"` occurrences |
|---|---|
| `d97bfcc3` materialized graph (3,150 facts) | **1,364,045** |
| `d97bfcc3` projection patches | **495,114** |
| `d97bfcc3` notes | **165,916** |
| **total the invoke response must carry** | **≈2,025,075** |

That is the shape of cfa1: **3,150 facts embedding 1.36 M basis span-revisions ⇒ ~433 basis rows per
fact ⇒ O(facts × spans) on disk.** 156 MB is not a big graph, it is a *quadratic basis*.

Sort order matters for "which session did he click": `SessionsBrowser` defaults to `"newest"`
(`src/components/SessionsBrowser.tsx:89-99`). By artifact mtime the row **directly under tonight's
57cfc64e is d97bfcc3** — i.e. the single pathological session in the vault is the most likely first
"different session" click. The review guard passed cleanly: capture stopped 22:35:15, so
`loadSession`'s `isCapturing || isTranscribing` refusal (`src/store/index.ts:3295-3299`) did not fire.

---

## 3. Ranked root-cause hypotheses

### H1 — P1 · `load_session` returns every artifact uncapped in one synchronous main-thread response; the webview (or the allocator) dies materializing it

* **Where:** `src-tauri/src/commands.rs:7599` (`#[tauri::command] pub fn load_session`), `:7642`
  (`load_materialized_graph`), `:7694-7703` (`Ok(LoadedSession { … })`);
  `src/store/index.ts:3306` (`await invoke<LoadedSession>("load_session")`).
* **Artifact-shape trigger:** a session whose materialized graph + projection log + notes exceed the
  renderer's headroom. d97bfcc3 = 156.6 + 33.3 + 19.1 + 2.1 MB ⇒ ≈208 MB JSON / ≈2.03 M basis
  objects, which must be fully materialized **three times**: parsed structs in Rust, one serialized
  JSON `String` in Rust, then `JSON.parse` → ~1–2 M nested JS objects in the WebView2 renderer.
* **Kill mechanism:** WebView2 renderer OOM ⇒ blank/gray window that never returns (Tauri does not
  respawn a dead renderer), **or** Rust allocation failure ⇒ `alloc_error_handler` → `abort()`, which
  bypasses `std::panic::set_hook` entirely ⇒ process vanishes **with no crash report**. Both match
  the empty `crashes/` dir and the `panic: 0` log.
* **Confirming evidence the parallel lanes should find:**
  * logs lane: tonight's `audio-graph.log` ends at `22:35:15.424` with **no** subsequent line, and
    the whole file contains zero occurrences of `load_session` / `build_session_timeline` /
    `list_sessions` (already verified: all 0). Windows **Event Viewer → Application** should hold an
    `Application Error`/`AppHang` or `EdgeUpdate`/`msedgewebview2.exe` fault for ~22:36-22:45 —
    that entry is the discriminator between renderer-OOM and Rust-abort.
  * artifacts lane: confirm `sessions.json` lists `d97bfcc3` as the row immediately below
    `57cfc64e` under `newest` sort, and confirm no `.audio-graph-canonical.lock` / manifest exists in
    the data root (it does not — so the read takes the `unguarded_absence_admission` branch,
    `src-tauri/src/persistence/session_semantics.rs:544-548`, which is cheap and exonerates the
    guard layer).
* **Round-4 overlap:** this is **cfa1** (156 MB graph artifact) escalating from "wasteful" to
  "fatal", because `load_session` hands the artifact to the webview verbatim.

### H2 — P1 · `replay_accepted_patches_with_history` is O(patches × transcript_events) with a full clone per event, on the main thread, inside `load_session`

* **Where:** `src-tauri/src/projections.rs:2825-2862` — for **every** patch it builds a fresh
  `TranscriptLedger::new` and re-applies `event.clone()` for every transcript event with
  `received_at_ms <= patch.created_at_ms`, and (when speaker history is present) a fresh
  `SpeakerTimeline` replaying every speaker revision the same way. Called from
  `src-tauri/src/commands.rs:7643-7649`.
* **Artifact-shape trigger:** 1,215 patches × 4,697 transcript events ⇒ up to **5.7 M
  `TranscriptEvent` clones** (each cloning `text` plus ~8 `Option<String>` fields) and 1,215 × 939 ⇒
  **1.14 M `DiarizationSpanRevision` clones**. Tens of millions of heap allocations, single-threaded,
  on the UI message-pump thread.
* **Kill mechanism:** multi-second-to-minute main-thread freeze ⇒ Windows marks the window hung
  (ghost window / "not responding"); a click on a hung window is where a user gives up and force-closes,
  and the allocator churn is also the most likely proximate cause of H1's allocation failure. Note the
  work is thrown away twice over: the 156 MB on-disk materialized graph is loaded at `:7642` and then
  **discarded** by `choose_materialized_graph` (`:7582-7591, 7678`) in favour of the replayed copy,
  so both exist in memory simultaneously.
* **Confirming evidence:** artifacts lane — `projections/<id>.events.jsonl` line count × transcript
  events line count for the opened session (1,215 × 4,697 here). logs lane — nothing will be logged
  (that is the point, see H6); the only positive signal available is an `AppHang` event or a
  perfmon/RSS trace.
* **Round-4 overlap:** amplifies **cfa1**; independent of ab9d/67cd.

### H3 — P1 · Any render error under the Timeline lens blanks the entire app permanently: the root `ErrorBoundary`'s fallback is `null`

* **Where:** `src/main.tsx:60-64` mounts `<ErrorBoundary><App /></ErrorBoundary>` **with no
  `fallback` prop**; `src/analytics/ErrorBoundary.tsx:44-46` returns
  `this.props.fallback ?? null`. The module doc at `ErrorBoundary.tsx:1-11` claims it "renders a
  minimal fallback so a render crash does not leave a blank window" — the mount site contradicts the
  doc.
* **Artifact-shape trigger:** any throw in the `SessionsBrowser`/`SeekTimeline` subtree. I could not
  find a *specific* throw in `SeekTimeline` — `formatTime` is NaN-safe (`src/utils/format.ts:11-18`),
  the `timeline` icon exists (`src/components/Icon.tsx:96`), all `seekTimeline.*` i18n keys exist,
  and `related_edge_ids` is a non-`Option` `Vec<String>` in the Rust type
  (`src-tauri/src/timeline.rs:78`) so `entry.related_edge_ids.length`
  (`src/components/SeekTimeline.tsx:173,293`) cannot see `undefined` from a real session. But a V8
  `RangeError`/OOM thrown *during* the lens swap lands here too.
* **Kill mechanism:** window renders nothing, forever, with no error text and no reload affordance —
  indistinguishable from "the app crashed" to a field tester, while the process stays alive.
* **Confirming evidence:** logs lane — if analytics was enabled, a `frontend.react.render` diagnostic
  with `component: root-boundary` (`ErrorBoundary.tsx:38-41`) or a `frontend.window.error` /
  `frontend.unhandledrejection` (`src/main.tsx:45-56`) at ~22:36+; if the maintainer reports the
  window was still *present* (title bar alive, could be moved) rather than gone, H3 outranks H1's
  abort variant.
* **Round-4 overlap:** none — this is a new, independent defect that makes every other frontend
  failure look like a process crash. Worth filing whether or not it is tonight's proximate cause.

### H4 — P2 · Every historical-session read is a `"sync"` (main-thread) command; the app has no async escape hatch for disk work

* **Where:** `src-tauri/src/commands.rs:7534` (`load_session_transcript`), `:7599` (`load_session`),
  `:7781` (`export_session_bundle`), `:7869` (`build_session_timeline_cmd`) — all `pub fn`, none
  `#[tauri::command(async)]` (0 occurrences repo-wide). Semantics verified at
  `tauri-macros-2.6.3/src/command/wrapper.rs:50,263-266`.
* **Trigger / mechanism:** any session large enough to make a read take >2 s freezes input delivery
  to the webview host window; the user's next click (here: the Timeline tab) is swallowed, then the
  window is declared hung. This is the structural precondition for H1 and H2 both, and the cheapest
  thing to fix (`#[tauri::command(async)]` on the four read commands moves them to the threadpool
  without changing signatures).

### H5 — P2 · The 1,000-node cap protects only the live graph; the path `load_session` actually prefers has no cap at all

* **Where:** `MAX_NODES = 1000` / `MAX_EDGES = 5000` at
  `src-tauri/src/graph/temporal.rs:45,53` are enforced only on the live insert path.
  `TemporalKnowledgeGraph::load_from_file` (`temporal.rs:1011-1048`) re-creates **every** persisted
  node/edge without re-applying either cap, and `snapshot()` (`temporal.rs:778-836`) emits everything
  it finds. Meanwhile `choose_materialized_graph` (`commands.rs:7582-7591, 7678`) makes the
  **materialized** projection graph authoritative whenever a canonical projection log exists — and
  that structure has no eviction cap anywhere.
* **Evidence:** `d97bfcc3….json` (live, capped) = 932 KB / at-cap; `d97bfcc3….materialized.json`
  (uncapped) = 156.6 MB. Same session, 168× apart.
* **Round-4 overlap:** this is why **67cd**'s 1,000-node cap did not prevent **cfa1**. State it
  explicitly in the cfa1 fix: capping the live graph is not a mitigation for the load path.

### H6 — P2 · The whole historical-read path is unobservable, so this class of crash can never be diagnosed from a field log

* **Where:** zero `log::` calls in `load_session_impl`, `session_timeline`,
  `session_timeline_for_admitted_session`, `load_session_transcript`, `list_sessions`,
  `session_export_bundle` (`src-tauri/src/commands.rs:7478-7873`). The nearest neighbours *do* log
  (`delete_session` `:7889`, `restore_session` `:7898`, `recover_orphaned_sessions` `:7933`).
* **Evidence:** tonight's log's last line is `22:35:15.424`; `grep -c` for `load_session`,
  `build_session_timeline`, `list_sessions` and `Session files not found` in that file = **0, 0, 0, 0**.
  The maintainer's entire post-capture interaction is invisible.
* **Fix shape:** one `log::info!` at entry/exit of each read command carrying `session_id`, per-artifact
  byte counts, response byte count and `elapsed_ms` (the `projection_job.flush elapsed_ms=…` line
  already in the log is the house style). Add a warn threshold on response size.

### H7 — P3 · De-ranked and explicitly ruled out

* **Rust panic → `extern "C"` abort.** Mechanically plausible (a panic in a `"sync"` command unwinds
  through the WebView2 COM callback, which aborts under edition 2024), but **no crash report since
  2026-05-31** and **`panic: 0`** in tonight's log. There is also no `panic = "abort"` profile in
  `src-tauri/Cargo.toml`. Ruled out unless the artifacts lane finds a `crashes/17*.log` newer than
  May 31 (it does not — the dir mtime is May 31 19:00).
* **Old-schema deserialization panic.** Ruled out: the reads are `serde_json::from_str` returning
  `Err` (`persistence/mod.rs:2758-2765`, `:2931`), `seq_id` carries `#[serde(default)]`
  (`temporal.rs:32`), and the pre-586b/pre-626c fields are `Option`. A schema mismatch surfaces as
  `AppError`, i.e. a red banner, not a crash.
* **Integer/index panics on the fold.** Ruled out: `millis()` is `is_finite()`-guarded and uses
  saturating `as i64` (`projections.rs:3360-3366`); `timeline.rs` has no non-test `unwrap`/`expect`;
  `graph/temporal.rs:355`'s `best_match.unwrap()` is short-circuit-guarded by `is_none()`.
* **Coordination-lock hang.** Ruled out: `try_lock_shared*` / `try_lock_exclusive*` only
  (`session_artifact_manifest.rs:809-814, 866-891`), and no control plane exists at this data root,
  so `open_session_for_content` takes the metadata-only absence branch
  (`session_semantics.rs:544-548`).
* **ab9d (41-vs-218 index divergence, 366 empty transcript files).** Does **not** support the crash.
  An indexed session whose artifacts are empty passes `session_has_any_artifact`
  (`commands.rs:7522-7528`, `path.exists()`), reads zero events, and folds an **empty** timeline →
  `SeekTimeline`'s empty state (`SeekTimeline.tsx:201-227`). ab9d predicts "Timeline says nothing
  happened", a *different* defect from tonight's.
* **Event flood.** Ruled out for this path: the Timeline lens registers no listeners and
  `load_session`/`session_timeline` emit no events.

---

## 4. Secondary finding on the timeline fold itself (not the crash)

`build_session_timeline_cmd` returns **one entry per surviving span, with full `text`**, uncapped
(`src-tauri/src/timeline.rs:119-186`), and the frontend then discards all but the last 200
(`src/components/SeekTimeline.tsx:120`). For d97bfcc3 that is 4,697 entries serialized to render 200,
and it re-reads and re-parses the live graph file that `load_session` had just parsed seconds earlier
(`commands.rs:7616` then `:7850`). Pure waste on the main thread — a P3 efficiency seed, and a cheap
one (fold, then `entries.split_off(len - 200)` behind a `limit` arg).

---

## Seed-shaped findings

### 1. P1 — `load_session` serializes every session artifact into one uncapped synchronous invoke response
* **Evidence:** `src-tauri/src/commands.rs:7599, 7642, 7694-7703`; `src/store/index.ts:3306`;
  disk: `graphs/d97bfcc3….materialized.json` = 156,579,416 B + `projections/…` 33.3 MB + `notes/…`
  19.1 MB ⇒ ≈208 MB / ≈2.03 M basis entries in one response; no crash report since 2026-05-31;
  `panic: 0`; log dead after `22:35:15.424` while disk writes continue to 22:36.
* **Root cause:** no size cap, no pagination, no streaming, and no lazy per-lens fetch —
  `LoadedSession` is an all-artifacts bundle, and the renderer must materialize the whole thing to
  show the Notes lens. Renderer OOM (blank window) or Rust `abort()` on allocation failure (silent
  process death); both bypass the panic hook.
* **Fix shape:** make `load_session` return metadata + transcript only; fetch notes / materialized
  graph / projection events per lens; hard-cap and refuse-with-error above a byte threshold.

### 2. P1 — `replay_accepted_patches_with_history` is O(patches × events) with a clone per event, on the main thread
* **Evidence:** `src-tauri/src/projections.rs:2825-2862`, called at
  `src-tauri/src/commands.rs:7643-7649`; 1,215 projection patches × 4,697 transcript events ⇒ ~5.7 M
  `TranscriptEvent` clones + 1.14 M speaker-revision clones per `load_session` of d97bfcc3; the
  156 MB on-disk graph loaded at `:7642` is then discarded at `:7678`.
* **Root cause:** the per-patch evidence ledger is rebuilt from scratch instead of advanced
  incrementally (the patches are already in `created_at_ms` order, so one forward cursor over
  `transcript_events` would make this O(patches + events)); plus `.clone()` where `&` would do.
* **Fix shape:** single incrementally-advanced ledger + speaker timeline; move the command off the
  main thread.

### 3. P1 — Root `ErrorBoundary` has no fallback: any render error renders the entire app as a blank window, forever
* **Evidence:** `src/main.tsx:60-64` (no `fallback` prop) + `src/analytics/ErrorBoundary.tsx:44-46`
  (`this.props.fallback ?? null`), contradicting the module doc at `ErrorBoundary.tsx:1-11`.
* **Root cause:** the boundary was wired for telemetry, not recovery; there is no error UI and no
  reload affordance, so every frontend fault is reported by users as "the app crashed".
* **Fix shape:** pass a real fallback (message + "Reload" that calls `window.location.reload()` +
  "Back to Capture" that resets `nav`), and reset `hasError` on nav change.

### 4. P2 — Every historical-session read command is a main-thread `"sync"` Tauri command
* **Evidence:** `src-tauri/src/commands.rs:7534, 7599, 7781, 7869` are plain `pub fn`; 98
  `#[tauri::command]` attributes and **0** `#[tauri::command(async)]` in `commands.rs`; semantics at
  `tauri-macros-2.6.3/src/command/wrapper.rs:50, 263-266` (`ExecutionContext::Blocking` default →
  kind `"sync"`).
* **Root cause:** heavy disk+CPU work runs inside the win32 message pump, so a slow read freezes
  input delivery to the webview and the OS declares the window hung.
* **Fix shape:** `#[tauri::command(async)]` on the four read commands (signature-compatible), plus a
  lint/test asserting no `pub fn` command performs a full-artifact read.

### 5. P2 — The materialized projection graph has no node/edge cap, so 67cd's 1,000-node cap does not protect the path `load_session` prefers
* **Evidence:** `src-tauri/src/graph/temporal.rs:45,53` (caps) vs `:1011-1048`
  (`load_from_file` re-applies neither) and `:778-836` (`snapshot()` emits all);
  `src-tauri/src/commands.rs:7582-7591, 7678` (`choose_materialized_graph` prefers the uncapped
  structure); disk: live graph 932 KB vs materialized 156.6 MB for the same session.
* **Root cause:** two graph representations with one cap between them, applied to the one that is no
  longer authoritative.
* **Fix shape:** cap/window the materialized graph at materialization time, and make the load path
  refuse (with a typed error the UI can render) above a ceiling instead of returning the artifact.

### 6. P2 — cfa1's real shape: fact basis is embedded per fact, so artifacts grow O(facts × spans)
* **Evidence:** `d97bfcc3….materialized.json` = 3,150 facts embedding **1,364,045** `"span_id"`
  basis entries (~433 per fact); the same pattern in `projections/…events.jsonl` (**495,114**) and
  `notes/….json` (**165,916**). Structural keys observed: `basis` → `span_revisions[{span_id,
  revision_number}]`, `valid_from_ms`, `last_sequence`.
* **Root cause:** every materialized fact carries a full span-revision basis list rather than a
  reference/range into the transcript log, so artifact size is quadratic in session length.
* **Fix shape:** store the basis as a `(first_seq, last_seq)` range or a content hash + pointer, and
  add a persisted-artifact size regression gate.

### 7. P3 — The historical-read path is uninstrumented; the crash left no trace
* **Evidence:** zero `log::` calls in `commands.rs:7478-7873`'s read functions; tonight's
  `audio-graph.log` contains 0 occurrences of `load_session`, `build_session_timeline`,
  `list_sessions`, `Session files not found`; last line `2026-08-23 22:35:15.424`.
* **Root cause:** logging was added to mutating session commands only.
* **Fix shape:** entry/exit `log::info!` with `session_id`, per-artifact bytes, response bytes,
  `elapsed_ms`; `log::warn!` above a response-size threshold.

### 8. P3 — `build_session_timeline_cmd` serializes every span to render 200, and re-parses the graph file `load_session` just read
* **Evidence:** `src-tauri/src/timeline.rs:119-186` (uncapped `Vec<TimelineEntry>` incl. full
  `text`); `src/components/SeekTimeline.tsx:120` (`slice(-200)`); duplicate live-graph parse at
  `src-tauri/src/commands.rs:7616` then `:7850`.
* **Root cause:** no `limit` parameter on the fold; no reuse of the graph already parsed by the
  sibling command in the same click.
* **Fix shape:** `limit`/`before_ms` argument on the command, returning the tail the UI actually
  renders.

---

## What each parallel lane should find if H1/H2 are right

* **Logs lane:** `audio-graph.log` ends `2026-08-23 22:35:15.424`, nothing after; no `panic`, no
  `load_session`/`build_session_timeline` string anywhere in the file; the previous rotated log
  (`audio-graph-20260823-222538.log`, 46 MB, mtime 16:03) is the *earlier* run and should show
  d97bfcc3's capture, not tonight's crash. The decisive artifact is **outside** the app's own log:
  Windows Event Viewer → Application, ~22:36-22:50, an `Application Error` for `audio-graph.exe`
  (⇒ Rust abort / allocation failure) **or** a fault/hang for `msedgewebview2.exe` (⇒ renderer OOM,
  H1's blank-window variant, which pairs with H3).
* **Session-artifacts lane:** confirm from `sessions.json` that `d97bfcc3-c902-4e08-8119-6149f7ca6db6`
  is the row immediately below tonight's `57cfc64e-adc2-4031-a2da-4d1d8ad686e4` under `newest` sort
  (that is the click the maintainer described as "a different session"); confirm the data root has
  **no** `.audio-graph-canonical.lock`/manifest (so the guard layer is exonerated); confirm no file
  in `~/.audiograph/crashes/` is newer than 2026-05-31; and record `wc -l` of
  `projections/<id>.events.jsonl` × `transcripts/<id>.events.jsonl` for every session in the list, so
  the O(patches × events) blast radius (H2) is quantified per session rather than for d97bfcc3 alone.
