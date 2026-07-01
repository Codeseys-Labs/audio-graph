# B35 — rsac 0.4.0 `backpressure_report()` API + audio-graph wiring

**Scope:** Exact `backpressure_report()` signature/return + minimal wiring into
audio-graph's existing capture-backpressure event → ControlBar pill.
**Primary sources (read directly):** rsac path dep at `/e/CS/github/rsac`
(v0.4.0, tag a2d3088) and audio-graph `src-tauri/` + `src/`.
**Version check:** audio-graph `src-tauri/Cargo.lock` already pins `rsac 0.4.0`
(line 6612-6613). NOTE: `src-tauri/Cargo.toml:55` comment still says "currently
at v0.3.0" — stale doc string only; the path dep resolves to the 0.4.0 checkout.
The `Cargo.toml` dep is `rsac = { path = "../../rsac", features = ["feat_*"] }`
per-OS — no version literal to bump.

---

## 1. Exact signature + return type, and where it lives

```rust
// rsac::api::AudioCapture  (inherent method — takes &self)
// src/api.rs:1999
pub fn backpressure_report(&self) -> BackpressureReport
```

- It is an **inherent method on the `AudioCapture` struct** (NOT on the
  `CapturingStream` trait). Re-exported via the crate prelude/`api` module as
  `rsac::AudioCapture::backpressure_report`. Takes `&self` (like
  `is_under_backpressure()`, `stream_stats()`, `format()`).
- Return type `BackpressureReport` lives in `src/core/introspection.rs:398-417`:

```rust
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct BackpressureReport {
    pub window: Duration,            // wall-clock span the tallies cover; ZERO if unattributable
    pub pushed: u64,                 // buffers successfully pushed within the window
    pub dropped: u64,                // buffers dropped (ring overflow) within the window
    pub drop_rate: f64,             // dropped / (pushed + dropped), in 0.0..=1.0, zero-div guarded
    pub is_under_backpressure: bool, // the legacy consecutive-drop bool, carried UNCHANGED
}
```

`#[non_exhaustive]` + `Default` ⇒ construct only via `..Default::default()` and
match with `..`. With no live stream, `backpressure_report()` returns
`BackpressureReport::default()` (all zero) — never panics (api.rs:2000-2003).

How the value is assembled (api.rs:1999-2035):
1. `let (pushed, dropped) = stream.drop_window_snapshot();`
2. `window = self.estimate_window_span(pushed + dropped)` —
   `buffers × buffer_size / sample_rate` seconds; `Duration::ZERO` if
   buffer_size/rate unknown or zero.
3. `BackpressureReport::from_counts(window, pushed, dropped, stream.is_under_backpressure())`
   computes `drop_rate` with a zero-denominator guard.

**Supporting trait method** (the data source), `CapturingStream` trait,
`src/core/interface.rs:332`:
```rust
fn drop_window_snapshot(&self) -> (u64, u64) { (0, 0) }   // default; bridge overrides
```
Overridden by `BridgeStream` (`src/bridge/stream.rs:305-311`) → delegates to
`BridgeShared::drop_window_snapshot()` (`src/bridge/ring_buffer.rs:495-504`).

---

## 2. Windowed report vs `is_under_backpressure` flag vs `stream_stats` counters

Three distinct diagnostic surfaces — do NOT conflate:

| Surface | Method | Semantics | Resets? |
|---|---|---|---|
| Legacy bool | `is_under_backpressure() -> bool` | `consecutive_drops >= backpressure_threshold`. All-or-nothing; trips only on a *run* of consecutive overflow drops. | Resets to false on ANY successful push. Misses sustained 1-in-N partial loss. |
| Lifetime counters | `stream_stats() -> StreamStats` | Cumulative-since-start: `buffers_pushed/captured/dropped`, `overruns`, `uptime`, `is_running`, `format_description`; `dropped_ratio()` helper. | Never resets → totals **dilute** over a long session ("how much overall?"). |
| **Windowed report** | `backpressure_report() -> BackpressureReport` | **Recent-window** `(pushed, dropped)` + `drop_rate` over a bounded sliding ring; carries the legacy bool too. | Slides — reflects "are we dropping *right now*?". Surfaces steady partial loss the bool misses. |

The window is a fixed alloc-free ring in `BridgeShared` (ring_buffer.rs:67-74):
`DROP_WINDOW_SLOTS = 8` slots × `DROP_WINDOW_SLOT_PUSHES = 128` pushes/slot ⇒
each slot ≈ 1.28 s, full ring ≈ **~10 s** of recent push history at typical
rates. Producer advances it on every push path via `record_drop_window()`
(reset-on-slot-advance = sliding, not cumulative).

Key fields: `drop_rate` is the new actionable signal (e.g. `0.33` = steady
1-in-3 loss with the bool still `false`). `window` is an honest estimate (may be
`Duration::ZERO` when span unattributable). See CHANGELOG `[0.4.0]` "Added" +
"Changed" (0.4.0 made the report *windowed* with a populated `window`; the
public struct shape was unchanged from 0.3.0).

---

## 3. How audio-graph surfaces backpressure today + MINIMAL wiring

### Current path (bool only)
- **Capture thread** `src-tauri/src/audio/capture.rs:509-564`: owns a local
  `rsac::AudioCapture` (built at :444, `!Sync`, owned for the thread's life).
  Every 10th received buffer (~50 ms) it polls `capture.is_under_backpressure()`
  and, **edge-triggered** (only on transition), emits `CAPTURE_BACKPRESSURE`.
- **Event/payload** `src-tauri/src/events.rs:58, 263-271`:
  `pub const CAPTURE_BACKPRESSURE = "capture-backpressure"` carrying
  `CaptureBackpressurePayload { source_id: String, is_backpressured: bool }`.
- **Frontend** `src/hooks/useTauriEvents.ts:339-343` listens, calls
  `setSourceBackpressure(source_id, is_backpressured)` → store
  `src/store/index.ts:461-475` maintains `backpressuredSources: string[]`.
- **Pill** `src/components/ControlBar.tsx:363-372`: when
  `backpressuredSources.length > 0`, renders a pulsing warning pill
  (`pulse-backpressure` keyframe, `src/styles/keyframes.css:45`), i18n keys
  `controlBar.backpressure` / `backpressureHint` (en.json:39-40, pt.json:39-40).
  TS type `src/types/index.ts:273-276`.

### Minimal wiring to feed the windowed report in
The existing bool→pill plumbing is the right surface to keep. Two minimal-effort
options (no architectural change):

**Option A (smallest — keep bool gate, enrich the trip threshold).**
In capture.rs replace the bool poll with the windowed report and trip on
sustained partial loss the bool misses:
```rust
let report = capture.backpressure_report();   // &self, cheap; replaces is_under_backpressure()
let now_backpressured = report.is_under_backpressure || report.drop_rate >= DROP_RATE_TRIP; // e.g. 0.05
```
Keep the same edge-triggered `CaptureBackpressurePayload { is_backpressured }`
emit. ZERO frontend/store/type changes. This alone closes the "steady 1-in-N
loss never lights the pill" gap.

**Option B (richer pill — surface `drop_rate`).**
Add an optional `drop_rate: f64` (and/or `window_secs: f64`) field to
`CaptureBackpressurePayload` (events.rs) + the TS `CaptureBackpressurePayload`
(types/index.ts) + store + the `backpressureHint` i18n string so the pill can
show e.g. "dropping 33%". Slightly more surface area; only do if product wants a
percentage in the UI. The trip logic is still Option A's.

Recommend **Option A** for B35's minimal scope: one-line change at
capture.rs:542, a new `DROP_RATE_TRIP` const, no IPC/contract churn. Note the
report's `is_under_backpressure` field already equals the old call, so behaviour
is a strict superset (never regresses the existing trip).

---

## 4. Threading / lifetime / cost caveats

- **Called off the RT thread, fine on the capture thread.** `backpressure_report()`
  / `drop_window_snapshot()` are **consumer-side reads**. In audio-graph they'd
  run on the same capture/forwarder thread that already calls
  `is_under_backpressure()` (capture.rs:542) — NOT the OS audio callback. The
  producer only ever *writes* the window (`record_drop_window`, RT-safe).
- **Cheap, lock-free, alloc-free.** `drop_window_snapshot()` is a single
  `Relaxed` pass over 8 `AtomicU64` slots (ring_buffer.rs:495-504); the wrapper
  adds `estimate_window_span` (a couple loads + one f64 divide) + `from_counts`
  (one divide). No `Mutex`, no heap. rsac docs (`CROSS_LANGUAGE_BINDINGS.md:52`,
  `PERFORMANCE.md`) call it "a second cheap, non-locking consumer-side read";
  `benches/observability.rs` covers it for RT-safety regression. Safe to call at
  the existing every-10-buffers (~50 ms) cadence; no need to throttle harder.
- **`&self`, but `AudioCapture` is `!Sync`.** The method takes `&self` (no `&mut`),
  but audio-graph already pins each `AudioCapture` to one thread
  (capture.rs:150/388 note "`rsac::AudioCapture` is `!Sync`") — so call it from
  that thread; do not share the handle across threads.
- **Eventually-consistent snapshot.** Counts are `Relaxed` loads → a torn/slightly
  stale view is possible but harmless for a UI drop-rate signal. `window` may be
  `Duration::ZERO` if buffer_size/rate is unknown — guard any UI math that
  divides by it.
- **No `start_stream`/version-break risk.** `BackpressureReport` shape is
  identical 0.3.0→0.4.0 (only the *windowing* semantics + populated `window`
  changed — CHANGELOG `[0.4.0]` "Changed"). The C ABI added a new symbol
  (MINOR, backward compatible). Pure Rust consumption via the path dep needs no
  migration.
