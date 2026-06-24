# B16 / ADR-0017 — Exact Rust API for live sherpa-onnx diarization wiring

Verification pass for wiring the (already-built) pure core (`clustering.rs` +
`stabilize.rs`) into a live rolling-window worker. Everything below is verified
against `src-tauri/Cargo.lock`, docs.rs crate **source** (not rendered docs),
the upstream `rust-api-examples`, DeepWiki on `k2-fsa/sherpa-onnx`, and the
existing in-repo code that already uses these crates. Date: 2026-05-30.

**Pinned versions (Cargo.lock):** `sherpa-onnx 1.13.2`, `ringbuf 0.4.8`,
`rtrb 0.3.4`, `tar 0.4.45`, `bzip2 0.6.1`, `reqwest 0.12.28` **and** `0.13.3`
(Cargo.toml declares `reqwest = "0.13.2"`; the 0.12 is a transitive dup),
`flate2 1.1.9`.

---

## 0. CORRECTION to the prior doc (load-bearing)

`docs/research/sherpa-diarization-live-2026-05.md` line 50 states the embedding
API is `EmbeddingExtractor::compute_speaker_embedding(samples, sr)`. **That method
does not exist in `sherpa-onnx 1.13.2`.** The real type is
`sherpa_onnx::SpeakerEmbeddingExtractor` and it is **stream-based** (same shape as
the ASR `OnlineStream`): `create_stream()` → `accept_waveform(sr, &[f32])` →
`input_finished()` → `is_ready()` → `compute()`. The stabilizer's `LocalCluster`
still takes a raw `Vec<f32>` embedding, so the core is unaffected — only the
worker glue changes. Exact signatures in §1.5.

---

## 1. sherpa-onnx 1.13.2 — exact diarization API

Source: `docs.rs/sherpa-onnx/1.13.2/src/sherpa_onnx/offline_speaker_diarization.rs`
and `.../speaker_embedding.rs`. The existing `clustering.rs` wrapper matches the
real API exactly (verified field-by-field).

### 1.1 Config structs (all `#[derive(Clone, Debug)]`, all have `Default`)

```rust
pub struct OfflineSpeakerSegmentationPyannoteModelConfig {
    pub model: Option<String>,                 // path to model(.int8).onnx
}                                              // derives Default (None)

pub struct OfflineSpeakerSegmentationModelConfig {
    pub pyannote: OfflineSpeakerSegmentationPyannoteModelConfig,
    pub num_threads: i32,                      // Default = 1
    pub debug: bool,                           // Default = false
    pub provider: Option<String>,              // Default = Some("cpu")
}

pub struct SpeakerEmbeddingExtractorConfig {
    pub model: Option<String>,                 // Default = None
    pub num_threads: i32,                      // Default = 1
    pub debug: bool,                           // Default = false
    pub provider: Option<String>,              // Default = Some("cpu")
}

pub struct FastClusteringConfig {
    pub num_clusters: i32,                     // Default = -1  (unknown -> threshold)
    pub threshold: f32,                        // Default = 0.5
}

pub struct OfflineSpeakerDiarizationConfig {
    pub segmentation: OfflineSpeakerSegmentationModelConfig,
    pub embedding: SpeakerEmbeddingExtractorConfig,
    pub clustering: FastClusteringConfig,
    pub min_duration_on: f32,                  // Default = 0.3
    pub min_duration_off: f32,                 // Default = 0.5
}
```

All field names in the prior doc are correct. Note: `provider` defaults to
`Some("cpu")`, **not** `None` — leave it at default for CPU. `num_threads`
defaults to `1`; bump segmentation+embedding to 2 each at most (ORT has its own
pool — see §5).

### 1.2 `OfflineSpeakerDiarization` (the diarizer)

```rust
pub struct OfflineSpeakerDiarization { /* private */ }
unsafe impl Send for OfflineSpeakerDiarization {}   // confirmed in source
unsafe impl Sync for OfflineSpeakerDiarization {}

impl OfflineSpeakerDiarization {
    pub fn create(config: &OfflineSpeakerDiarizationConfig) -> Option<Self>;
    pub fn sample_rate(&self) -> i32;                         // 16000 for these models
    pub fn set_config(&self, config: &OfflineSpeakerDiarizationConfig); // &self, NOT &mut
    pub fn process(&self, samples: &[f32]) -> Option<OfflineSpeakerDiarizationResult>;
}
// Drop frees the C object. Reuse ONE instance for the whole session.
```

Confirmed: `create() -> Option`, `process(&[f32]) -> Option<Result>`,
`sample_rate() -> i32`, `set_config(&self, &cfg)`. **`set_config` and `process`
take `&self`** (interior C ptr), so the worker can hold the diarizer behind a
plain `&` and call `set_config` to retune `clustering.threshold` live without a
rebuild. `Send + Sync` → safe to `move` into a `std::thread`.

### 1.3 `OfflineSpeakerDiarizationResult`

```rust
pub struct OfflineSpeakerDiarizationResult { /* private */ }  // Send + Sync
impl OfflineSpeakerDiarizationResult {
    pub fn num_speakers(&self) -> i32;
    pub fn num_segments(&self) -> i32;
    pub fn sort_by_start_time(&self) -> Vec<OfflineSpeakerDiarizationSegment>;
}
```

### 1.4 `OfflineSpeakerDiarizationSegment`

```rust
#[derive(Clone, Debug)]
pub struct OfflineSpeakerDiarizationSegment {
    pub start: f32,   // seconds (window-local)
    pub end: f32,     // seconds
    pub speaker: i32, // 0-based, PERMUTATION-ARBITRARY per process() call
}
```

This is exactly what `clustering.rs::diarize` already maps into its own
`ClusterSegment`. No change needed there.

### 1.5 `SpeakerEmbeddingExtractor` — the per-cluster embedder (worker needs this)

The diarizer does NOT expose per-cluster embeddings. To get the one embedding
per local cluster that `stabilize.rs::SpeakerRegistry::assign` consumes, the
worker creates a **second** sherpa object pointed at the **same embedding
model** and runs it on each cluster's concatenated speech samples:

```rust
pub struct SpeakerEmbeddingExtractor { /* private */ }   // Send + Sync
impl SpeakerEmbeddingExtractor {
    pub fn create(config: &SpeakerEmbeddingExtractorConfig) -> Option<Self>;
    pub fn dim(&self) -> i32;                              // 192 (TitaNet) / 512 (ERes2Net)
    pub fn create_stream(&self) -> Option<OnlineStream>;
    pub fn is_ready(&self, stream: &OnlineStream) -> bool; // true once enough audio fed
    pub fn compute(&self, stream: &OnlineStream) -> Option<Vec<f32>>; // len == dim()
}
```

`OnlineStream` (shared with ASR module) feeding API:

```rust
impl OnlineStream {
    pub fn accept_waveform(&self, sample_rate: i32, samples: &[f32]);
    pub fn input_finished(&self);
}
```

**Canonical usage (from `rust-api-examples/.../speaker_embedding_extractor.rs`):**

```rust
let stream = extractor.create_stream().expect("stream");
stream.accept_waveform(16_000, &cluster_samples_f32);  // i32 sample rate
stream.input_finished();
if !extractor.is_ready(&stream) {
    // clip too short -> skip; stabilize.rs already tolerates empty embeddings
} else {
    let embedding: Vec<f32> = extractor.compute(&stream).expect("embed"); // == dim()
}
```

`extractor.dim()` gives the embedding dimension at runtime — feed that to nobody
(stabilize.rs auto-locks dim on first insert), but it confirms the prior doc's
"don't hardcode dim" advice. **`compute()` returns `Vec<f32>` of length `dim()`,
NOT L2-normalized** — `stabilize.rs::assign` normalizes internally, so pass the
raw vector straight into `LocalCluster.embedding`.

**Building the per-cluster samples:** after `diarize()` returns segments, for
each `speaker` id collect that cluster's `[start,end)` spans, slice them out of
the window's 16 kHz mono buffer (`samples[(start*16000) as usize .. (end*16000)]`),
concatenate, and feed as one `accept_waveform`. `duration_secs` for the
`LocalCluster` = sum of (end-start) per the cluster's segments.

---

## 2. Ring buffer: use `ringbuf 0.4` (NOT rtrb) — already a dependency

**Decision: `ringbuf 0.4.8`.** Rationale beyond API: the repo **already depends
on `ringbuf = "0.4"`** (Cargo.toml line 118) and already uses it for the cpal
playback SPSC handoff in `src-tauri/src/playback/mod.rs`. Adding `rtrb` would be
a redundant second SPSC crate. `rtrb 0.3.4` is present only transitively. Mirror
the established in-repo pattern.

### 2.1 Established in-repo pattern (playback/mod.rs:41-42, 216-217, 268, 287)

```rust
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

let rb = HeapRb::<f32>::new(capacity);     // capacity in SAMPLES
let (prod, cons): (HeapProd<f32>, HeapCons<f32>) = rb.split();
// prod  -> audio/capture side
// cons  -> diarization worker thread (move it in)
```

`HeapProd`/`HeapCons` are `Send` (inner storage is `Arc<SharedRb<...>>`), so the
consumer moves cleanly into a dedicated `std::thread`.

### 2.2 Exact 0.4.x methods used (all confirmed in playback/mod.rs)

```rust
// Producer (capture thread / pipeline callback):
prod.push_slice(&[f32]) -> usize     // count actually written (<= len if full)
prod.vacant_len() -> usize           // free slots (back-pressure check)
prod.try_push(f32) -> Result<(), f32>

// Consumer (worker):
cons.pop_slice(&mut [f32]) -> usize  // count popped
cons.occupied_len() -> usize         // samples available
cons.try_pop() -> Option<f32>
```

These come from the `Producer` / `Consumer` / `Observer` traits respectively;
`Split` provides `.split()`. **The `traits::*` import is mandatory** — the
methods are trait methods, not inherent, so the file won't compile without the
`use ringbuf::traits::{...}` line.

### 2.3 rtrb comparison (for the record — not chosen)

`rtrb 0.3.4`: constructor is `RingBuffer::<f32>::new(capacity) -> (Producer<f32>,
Consumer<f32>)` (no `.split()`). Slice ops: `push_partial_slice(&[T]) -> usize` /
`push_entire_slice(&[T]) -> Result<(),_>` (producer), `pop_partial_slice(&mut
[T]) -> usize` / `pop_entire_slice` (consumer), plus `slots()` for free/used
count. Genuinely wait-free and slightly leaner, but (a) it's not the repo's
existing choice and (b) `ringbuf` 0.4 is equally lock-free/realtime-safe for this
SPSC pattern. **No technical reason to introduce rtrb.**

### 2.4 Worker drain pattern (matches stabilize.rs::WindowSchedule)

The worker is a dedicated `std::thread`. It does NOT need to run ORT in the
audio callback (which is the whole point). Pattern:

- The capture/pipeline side pushes 16 kHz mono f32 into `prod` via `push_slice`
  (drop on full — `push_slice` returns < len; log + count drops, never block the
  audio thread).
- Worker keeps its **own** rolling `Vec<f32>` buffer (the ring is just the
  thread handoff; `WindowSchedule` tracks sample counts, not bytes). Loop:
  `pop_slice` into a scratch buffer, append to the rolling buffer + call
  `schedule.ingest(n)`; when `schedule.poll()` returns `Some(take)`, run
  `diarize` on the trailing `take` samples, embed each cluster, call
  `registry.assign(...)`, emit only the trailing-`hop` relabeled segments.
- Trim the rolling buffer to `window_samples()` (drop the front) so it stays
  bounded for a long session. Sleep ~`hop/4` between drains, or block on a
  `crossbeam_channel` "samples available" nudge.

Capacity sizing: ring must hold ≥ one hop plus jitter; `HeapRb::<f32>::new(16000
* 4)` (≈4 s) is comfortable for H≤3 s.

---

## 3. Download + extract `.tar.bz2` at runtime — pattern already exists

`src-tauri/src/models/mod.rs` **already does exactly this** for the Zipformer
ASR archive and for the bare-`.onnx` Sortformer model. Reuse verbatim; the
constants for the two diarization models are **already declared** in that file
(lines 79-95) as `#[allow(dead_code)]` awaiting this wiring:

- `DIAR_SEG_PYANNOTE_URL` / `DIAR_SEG_PYANNOTE_DIR` /
  `DIAR_SEG_PYANNOTE_FILE = "model.int8.onnx"` /
  `DIAR_SEG_PYANNOTE_REQUIRED_FILES = ["model.onnx","model.int8.onnx"]`
- `DIAR_EMB_TITANET_URL` / `DIAR_EMB_TITANET_FILENAME = "nemo_en_titanet_small.onnx"`

### 3.1 Crates (all pinned, no new deps)

- **Download:** `reqwest::blocking::Client` (already used; the file pulls
  `reqwest::blocking::Client::new().get(url).send()` then reads the response as
  `std::io::Read` in an 8 KiB loop — see `download_model`). The `stream` feature
  is on but the existing code uses the **blocking** reader, not async streaming.
- **bzip2 decompress:** `bzip2::read::BzDecoder::new(File)` (bzip2 0.6.1).
- **tar extract:** `tar::Archive::new(decoder).unpack(&dir)` (tar 0.4.45).
- `flate2` (gzip) is present but irrelevant — the diarization archives are
  bzip2, not gzip.

### 3.2 Exact extract code to mirror (models/mod.rs:606-612)

```rust
let archive_file = std::fs::File::open(archive_path)?;
let decoder = bzip2::read::BzDecoder::new(archive_file);
let mut archive = tar::Archive::new(decoder);
archive.unpack(&extract_dir)?;          // then find_*_root + fs::rename into place
```

### 3.3 Wiring the two models into `MODELS` / status

1. Add a `ModelDef` for the segmentation archive routed through a
   `download_*_archive` branch (clone `download_sherpa_zipformer_model` +
   `extract_sherpa_zipformer_archive`, swap `SHERPA_ZIPFORMER_REQUIRED_FILES`
   for `DIAR_SEG_PYANNOTE_REQUIRED_FILES`, target dir `DIAR_SEG_PYANNOTE_DIR`).
   The downloader generalizes cleanly — consider a `RequiredFiles(&[&str])`
   field on `ModelDef` instead of the `if filename == SHERPA_ZIPFORMER_20M`
   special-case, since there are now two archive models.
2. Add a `ModelDef` for `nemo_en_titanet_small.onnx` as a **bare download**
   (the default `download_model` path — like Sortformer). No `expected_size` is
   published; set `expected_size: None` (the verifier then only checks
   non-empty, see `verify_model_file`). Same for the segmentation archive's
   `expected_size` — prior doc says "No published SHA"; size-tolerance check is
   the only gate.
3. Resolve final paths for `ClusteringDiarizer::new(seg, emb, threshold)`:
   `models_dir/DIAR_SEG_PYANNOTE_DIR/DIAR_SEG_PYANNOTE_FILE` and
   `models_dir/DIAR_EMB_TITANET_FILENAME`.

---

## 4. Model release URLs — all resolve (HTTP 200, verified 2026-05-30)

`curl -sIL` (HEAD, follows redirect to GitHub `release-assets` CDN), all returned
`200`:

| Asset | URL | Status |
|---|---|---|
| Segmentation (MIT) | `github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2` | **200** |
| TitaNet-small (dim 192, en) | `.../releases/download/speaker-recongition-models/nemo_en_titanet_small.onnx` | **200** |
| 3D-Speaker ERes2Net (dim 512, multiling) | `.../releases/download/speaker-recongition-models/3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx` | **200** |

The release **tag literally spells "recongition"** (typo upstream) — keep it; the
correctly-spelled URL 404s. Confirmed still true. Segmentation archive extracts
to `sherpa-onnx-pyannote-segmentation-3-0/{model.onnx, model.int8.onnx}`.

DeepWiki confirms the recommended pairing: pyannote-segmentation-3.0 +
(TitaNet-small for English / ERes2Net for multilingual), all **16 kHz**. The
diarizer's `sample_rate()` returns `16000`; `clustering.rs` already hard-asserts
`CLUSTERING_SAMPLE_RATE == 16_000` against it.

---

## 5. ORT / parakeet build constraint — confirmed enforced

- The repo enforces mutual exclusion in `src-tauri/src/lib.rs:20-22`:
  `#[cfg(all(feature = "diarization", feature = "diarization-clustering"))]
  compile_error!(...)`. Verified present.
- `Cargo.toml`: `diarization = ["dep:parakeet-rs"]` (line 44),
  `diarization-clustering = ["dep:sherpa-onnx"]` (line 53),
  `sherpa-streaming = ["dep:sherpa-onnx"]` (line 47). The Sortformer path
  (`parakeet-rs 0.3`, `sortformer` feature) and the sherpa-onnx path each link
  their **own ONNX Runtime**, which collide at link time — hence the exclusion.
- DeepWiki: `sherpa-onnx-sys 1.13.x` `build.rs` **statically** links a prebuilt
  ORT it **auto-downloads from GitHub releases** at build time when
  `SHERPA_ONNX_LIB_DIR` is unset. Opt into shared linkage with
  `sherpa-onnx = { version = "1.13.2", default-features = false,
  features = ["shared"] }` (build.rs then copies the DLL/.so next to the binary).
  For B16, keep the **default static** ORT (no Cargo change) since the feature is
  already wired; just ensure CI builds the `diarization-clustering` feature in a
  job that does NOT also enable `diarization`.
- `num_threads`: ORT manages its own intra-op pool, so keep
  segmentation/embedding `num_threads` at 1–2; do not multiply by core count.

---

## 6. Net wiring checklist (no core changes needed)

1. New feature-gated module (e.g. `diarization/worker.rs`, `cfg(feature =
   "diarization-clustering")`): owns one `ClusteringDiarizer` + one
   `SpeakerEmbeddingExtractor` (same embedding-model path) + one
   `SpeakerRegistry::with_defaults()` + one `WindowSchedule`.
2. `HeapRb::<f32>::new(16000*4).split()`; producer handed to the capture/pipeline
   16 kHz-mono tap, consumer moved into a `std::thread`.
3. Worker loop: `pop_slice` → append to rolling buf → `schedule.ingest` →
   `schedule.poll()` → `diarize` trailing window → per-cluster
   `accept_waveform/input_finished/is_ready/compute` → `registry.assign` →
   relabel + emit trailing-hop segments via existing `crossbeam`/Tauri event.
4. Download wiring: two `ModelDef`s + reuse the existing bzip2/tar archive path;
   constants already declared in `models/mod.rs`.
5. Live retune: expose `clustering.threshold` + registry `sim_threshold`; call
   `OfflineSpeakerDiarization::set_config(&new_cfg)` to retune without rebuild.

**Open items to validate on real audio (unchanged from prior doc):** tune
`clustering.threshold` (within-window) and `sim_threshold` (cross-window) on a
real multi-speaker clip; confirm `is_ready` fires for ≥ ~1.5 s clusters.
