# B36 — Contained Rust 0.x-major dep migrations

Research date: 2026-05-31. Sources are PRIMARY: docs.rs per-version pages, crate
GitHub release notes / migration guides, cross-checked against the actual pinned
versions in `src-tauri/Cargo.lock` and every in-repo call site.

## TL;DR verdict table

| Dep | Current (direct) | Target | In-repo call sites | API delta for us | Verdict |
|-----|------------------|--------|--------------------|------------------|---------|
| ringbuf | 0.4.8 | 0.5.0 | `playback/mod.rs`, `diarization/worker.rs` | **None** — identical re-exports/aliases/trait methods | **bump-now**, low-risk |
| rubato | 2.0.0 | 3.0.0 | `audio/pipeline.rs` | **None for us** — only `process_into_buffer` gained a 2nd lifetime; we use `process()` | **bump-now**, low-risk |
| sysinfo | 0.38.4 | 0.39.x | `commands.rs::list_running_processes` | **None** — breaking `refresh_processes`/`ProcessesToUpdate` changes already absorbed at 0.30→0.32 | **bump-now**, low-risk |

All three are contained, source-compatible bumps for *our* usage. None requires a
code change at the call sites. The only required edits are the three version
strings in `src-tauri/Cargo.toml`. Recommend doing all three together in one PR
with a normal `cargo build` + `cargo test` + a manual smoke of playback /
diarization / process-picker.

### Lockfile note (important context)
`Cargo.lock` also lists `rubato 0.16.2` and `sysinfo 0.36.1`. Those are
**transitive deps of `mistralrs-core`**, NOT our code, and are unaffected by
bumping our direct deps. Our direct deps are `rubato = "2.0"` (→ 2.0.0) and
`sysinfo = "0.38.4"`. After our bump the graph will simply carry two majors of
each (ours + mistralrs's) until mistralrs upgrades — this already happens today
and is harmless.

---

## 1. ringbuf 0.4.8 → 0.5.0

### What we use (verified)
- `playback/mod.rs:41-42`: `use ringbuf::traits::{Consumer, Observer, Producer, Split};`
  + `use ringbuf::{HeapCons, HeapProd, HeapRb};`
- `playback/mod.rs`: `HeapRb::<i16>::new(cap)`, `rb.split()`, `prod.push_slice(samples)`,
  `prod.vacant_len()`, `cons.pop_slice(buf)`, `cons.occupied_len()`.
- `diarization/worker.rs:216-217`: `use ringbuf::traits::{Consumer, Producer, Split};`
  + `use ringbuf::{HeapCons, HeapProd, HeapRb};`
- `diarization/worker.rs`: `HeapRb::<f32>::new(cap)`, `rb.split()`, `prod.push_slice`,
  `cons.pop_slice`.

### Breaking-change delta (0.4 → 0.5)
Cross-checked docs.rs/ringbuf/0.4.8 vs docs.rs/ringbuf/0.5.0 crate root and trait
pages — they are **identical for everything we touch**:
- The trait module is still `ringbuf::traits`; `Consumer`, `Producer`, `Observer`,
  `Split` keep the same names and locations (re-exported via `pub use traits::consumer;`
  / `pub use traits::producer;` in both versions).
- Crate-root **type aliases unchanged**: `HeapRb` (= `SharedRb<Heap<T>>`),
  `HeapProd` (= `CachingProd<Arc<HeapRb<T>>>`), `HeapCons` (= `CachingCons<Arc<HeapRb<T>>>`).
- Method names/signatures unchanged:
  - `Split::split(self) -> (Prod, Cons)` — same.
  - `Producer::push_slice(&mut self, &[T]) -> usize where T: Copy` — same.
  - `Consumer::pop_slice(&mut self, &mut [T]) -> usize where T: Copy` — same.
  - `Observer::vacant_len(&self) -> usize`, `Observer::occupied_len(&self) -> usize` — same.
  - `HeapRb::<T>::new(usize)` — same.

There is **no trait reorg, no method rename, and no HeapRb API change** affecting
our SPSC push_slice/pop_slice/vacant_len/occupied_len usage. The 0.5 release is a
major bump for internal/edge-API reasons (the public docs show no diff in the
surface we depend on; 0.4.9 was published after 0.5.0, i.e. 0.4.x is still
maintained in parallel, which is itself a signal the break is narrow).

### Before / after for the methods we use
No change. Example (unchanged across both):
```rust
use ringbuf::traits::{Consumer, Producer, Observer, Split};
use ringbuf::{HeapRb, HeapProd, HeapCons};
let rb = HeapRb::<i16>::new(cap);
let (mut prod, mut cons) = rb.split();
let wrote = prod.push_slice(samples);   // -> usize
let free  = prod.vacant_len();          // -> usize
let got   = cons.pop_slice(&mut buf);   // -> usize
let used  = cons.occupied_len();        // -> usize
```

### Risk class
LOW. Pure dependency bump; no cross-platform behavior change (it is a portable
in-memory lock-free buffer — no OS-specific code paths). No multi-OS validation
needed beyond the normal CI build/test.

**Recommendation: bump-now.** Change `ringbuf = "0.4"` → `ringbuf = "0.5"` in
`src-tauri/Cargo.toml`. Expect zero source edits; rely on the compiler to confirm.

---

## 2. rubato 2.0.0 → 3.0.0

### What we use (verified — `audio/pipeline.rs`)
```rust
use rubato::{Async, FixedAsync, Resampler, SincInterpolationParameters,
             SincInterpolationType, WindowFunction};
use audioadapter_buffers::direct::SequentialSliceOfVecs;

// create_resampler():
Async::<f32>::new_sinc(
    ratio, 2.0, &params, RESAMPLER_CHUNK_SIZE, 1 /*mono*/, FixedAsync::Input,
)?;

// drain_resampler():
let needed = resampler.input_frames_next();
let input_adapter = SequentialSliceOfVecs::new(&waves_in, 1, needed)?;
let interleaved_out = resampler.process(&input_adapter, 0, None)?; // ResampleResult<InterleavedOwned<f32>>
let resampled = interleaved_out.take_data();
```
We call only: `Async::new_sinc`, `FixedAsync::Input`, `Resampler::input_frames_next`,
`Resampler::process(&buffer_in, input_offset, active_channels_mask)`, and
`InterleavedOwned::take_data`. We do **not** call `process_into_buffer` (verified
via grep — zero callers).

### Breaking-change delta (2.0 → 3.0)
The big audioadapter redesign (merged FixedIn/FixedOut/FixedInOut into single
`Async`/`Fft` types, switch to the `audioadapter` crate) landed at **1.0.0**, which
we are already past. The v3.0.0 GitHub release (signed, 2026-05-20) lists only:
- doc updates (#125),
- **"Separate lifetimes for input and output" (#126)** — the sole API-signature change,
- performance/dot-product internals (#130, #131).

Concretely, comparing docs.rs/rubato/2.0.0 vs 3.0.0:
- `Async::new_sinc(resample_ratio: f64, max_resample_ratio_relative: f64,
  parameters: &SincInterpolationParameters, chunk_size: usize, nbr_channels: usize,
  fixed: FixedAsync) -> Result<Self, ResamplerConstructionError>` — **byte-identical**.
- `FixedAsync` enum — **unchanged** (`Input` / `Output`).
- `SincInterpolationParameters`, `SincInterpolationType`, `WindowFunction` — unchanged.
- `Resampler::process(&dyn Adapter, input_offset: usize, active_channels_mask:
  Option<&[bool]>) -> ResampleResult<InterleavedOwned<T>>` — **identical** in both;
  still the heap-allocating convenience wrapper returning `InterleavedOwned`.
- The #126 change: `process_into_buffer` went from one lifetime `<'a>` to two
  `<'a, 'b>` (separate input/output buffer lifetimes). This is the only signature
  delta — and it is **non-breaking for almost all callers** (lifetimes are
  inferred), AND we don't call it at all.

### Before / after for our call sites
No change required. `new_sinc(...)`, `FixedAsync::Input`, `input_frames_next()`,
`process(&adapter, 0, None)?.take_data()` all compile unchanged under 3.0.0.

### Caveat to validate at build time
`process()` quality/length output is governed by the same sinc params; the #130/#131
dot-product changes are performance-only and must not change output framing, but
since resampled output length is variable and we already drain it into an
accumulation buffer (`accumulation_buffer`), there is no fixed-length assumption to
break. Our `input_frames_next()`-gated feeding remains correct.

### Risk class
LOW. The resampler is portable numeric code (realfft + audioadapter); no
OS-specific paths, so no multi-OS validation is required. The only thing worth a
quick check is that resampled audio still sounds correct — covered by running the
ASR pipeline once on 44.1k and 48k input (the existing `create_resampler_48k` /
`create_resampler_44k` unit tests already exercise construction).

**Recommendation: bump-now.** Change `rubato = "2.0"` → `rubato = "3"` (or `"3.0"`)
in `src-tauri/Cargo.toml`. Keep `audioadapter-buffers` aligned — verify the
`audioadapter`/`audioadapter-buffers` versions rubato 3.0.0 pulls match the
`SequentialSliceOfVecs`/`take_data` API we use (the 3.0.0 lock entry depends on
`audioadapter` + `audioadapter-buffers`, same crates we already pull). Run
`cargo build` + the two resampler unit tests + one live pipeline smoke.

---

## 3. sysinfo 0.38.4 → 0.39.x

### What we use (verified — `commands.rs:2619-2641`)
```rust
use sysinfo::System;
let mut sys = System::new();
sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
let processes = sys.processes().iter()
    .filter(|(_, p)| !p.name().to_string_lossy().is_empty())
    .map(|(pid, p)| ProcessInfo {
        pid: pid.as_u32(),
        name: p.name().to_string_lossy().to_string(),
        exe_path: p.exe().map(|e| e.to_string_lossy().to_string()),
    });
```
Surface used: `System::new`, `System::refresh_processes(ProcessesToUpdate, bool)`,
`System::processes`, `Pid::as_u32`, `Process::name` (`-> &OsStr`), `Process::exe`
(`-> Option<&Path>`).

### Breaking-change delta (0.38 → 0.39)
The official `migration_guide.md` (GuillaumeGomez/sysinfo, main) has **no
"0.38 to 0.39" section** — meaning 0.39 shipped no breaking API renames. The
breaking changes that touch *our* surface all predate us and are already absorbed:
- `refresh_processes`/`ProcessesToUpdate` were introduced at **0.30→0.31**.
- The extra `remove_dead_processes: bool` arg (our `true`) was added at **0.31→0.32**.
- `Process::name()` becoming `&OsStr` and `exe()` returning `Option<&Path>` predate 0.38.

The CHANGELOG entries for the 0.39 line are platform-level fixes only, none API-breaking:
- 0.39.3: improve `Networks::refresh` perf (non-Windows); fix a user-retrieval
  soundness issue; Linux cgroup parent memory limits; Linux ESXi process panic fix;
  FreeBSD zfs disk naming. None of these touch process listing on our targets in a
  source-incompatible way.

Cross-checked docs.rs/sysinfo/0.39.0: `ProcessesToUpdate { All, Some(&[Pid]) }`,
`System`, `refresh_processes`, `processes()`, `Pid::as_u32`, `Process::name`,
`Process::exe` are all present with the same signatures we call.

### Before / after for our call site
No change. The exact line
`sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);` is already in the
post-0.32 two-argument form and remains valid in 0.39.

### Risk class
LOW for the *API*. BUT sysinfo is inherently per-OS (Windows/macOS/Linux all have
separate backends), and 0.39.x carries platform-behavior fixes (Linux cgroup/ESXi,
non-Windows networks/users). Our usage is just "list all processes with
name + exe path," which is the most stable, widely-tested code path, and the 0.39
fixes are improvements, not regressions, for that path. A light multi-OS sanity
check of the process-picker (Windows + at least one of macOS/Linux) is prudent
because this crate is the one with genuine cross-platform surface — but no code
change is anticipated.

**Recommendation: bump-now.** Change `sysinfo = "0.38.4"` → `sysinfo = "0.39"` in
`src-tauri/Cargo.toml`. Zero source edits expected. Smoke `list_running_processes`
on the primary dev OS; if CI runs multi-OS, let it confirm the others.

---

## Combined rollout recommendation
One PR, three one-line `Cargo.toml` edits, `cargo update -p ringbuf -p rubato -p
sysinfo` (or full `cargo build`). Expected source diff: **zero lines**. Verify:
1. `cargo build --manifest-path src-tauri/Cargo.toml` (catches any surface drift).
2. `cargo test` in `src-tauri/` (covers `create_resampler_*`, playback ringbuf tests).
3. Manual smoke: TTS playback (ringbuf i16), live diarization feed (ringbuf f32 +
   resampler), and the process-picker UI (`list_running_processes`).
4. If the sysinfo process-picker must work on macOS/Linux too, run that smoke per OS.

Confidence: HIGH that all three are source-compatible for our call sites (verified
against per-version docs.rs pages, the rubato v3.0.0 signed release notes, and the
sysinfo migration guide). The single residual unknown is the rubato #126 lifetime
change — irrelevant to us because we never call `process_into_buffer`.
