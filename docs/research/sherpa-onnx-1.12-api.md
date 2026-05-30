# sherpa-onnx Rust crate API research (streaming ASR + offline diarization)

> Research brief output. **Research only — no source code was modified.**
> Workdir: `E:\CS\github\audio-graph`. Target binding file to fix:
> `src-tauri/src/asr/sherpa_streaming.rs`.

## 0. IMPORTANT version correction

The brief assumes the resolved crate is `1.12.x`. It is **not**.

- `src-tauri/Cargo.toml` requests `sherpa-onnx = { version = "1.12", optional = true }`.
- A Cargo `"1.12"` requirement means `>=1.12.0, <2.0.0`, so Cargo resolved the
  newest compatible release.
- `src-tauri/Cargo.lock` actually pins:
  - `sherpa-onnx 1.13.2` (checksum `f70620e4…`)
  - `sherpa-onnx-sys 1.13.2` (checksum `c7f3fe49…`)

Every API detail and snippet below is taken from the **1.13.2** crate source on
docs.rs (`online_asr.rs`, `offline_speaker_diarization.rs`, `speaker_embedding.rs`),
which is what actually compiles in this workspace. The 1.12 published source had
the same struct shapes for these modules, but the **linking model changed**
between 1.12 and 1.13 (see §4) — this is the single biggest gotcha, so treat the
binding as a 1.13 binding and (ideally) bump the `Cargo.toml` requirement to
`"1.13"` to make intent explicit.

The reason the current file does not compile is independent of the version skew:
it was written against an **imagined** API (`OnlineRecognizer::new`, `&str` config
fields, `i32` `enable_endpoint`, a `String`-returning `get_result`). The real API
never looked like that. Concrete fixes are in §1.

---

## 1. Online / streaming ASR — current API

Source of truth: `sherpa_onnx::online_asr` (docs.rs `online_asr.rs`, 1.13.2).

### 1.1 Types & constructors

| Item | Reality (1.13.2) | What the stale file assumed |
|---|---|---|
| Constructor | `OnlineRecognizer::create(&cfg) -> Option<Self>` | `OnlineRecognizer::new(&cfg)` (wrong name, wrong return) |
| Stream | `recognizer.create_stream() -> OnlineStream` | same (OK) |
| Result | `recognizer.get_result(&stream) -> Option<RecognizerResult>` | returned a `String`, called `.trim()` directly |
| `accept_waveform` | **method on `OnlineStream`**, `(sample_rate: i32, samples: &[f32])` | called on stream (OK) but note it's on the stream, not recognizer |
| `is_ready` / `decode` / `is_endpoint` / `reset` | methods on **`OnlineRecognizer`**, take `&OnlineStream` | OK |
| `input_finished` | method on `OnlineStream` (flush tail) | missing |

`OnlineRecognizer`, `OnlineStream`, `RecognizerResult` are all `Send + Sync`
(`unsafe impl` in the crate; the C lib is thread-safe for single-object usage).

### 1.2 Config structs (exact fields)

```rust
// All model-path fields are Option<String> (owned), NOT &str.
pub struct OnlineTransducerModelConfig {
    pub encoder: Option<String>,
    pub decoder: Option<String>,
    pub joiner:  Option<String>,
}

pub struct OnlineModelConfig {
    pub transducer:     OnlineTransducerModelConfig,
    pub paraformer:     OnlineParaformerModelConfig,
    pub zipformer2_ctc: OnlineZipformer2CtcModelConfig,
    pub nemo_ctc:       OnlineNemoCtcModelConfig,
    pub t_one_ctc:      OnlineToneCtcModelConfig,
    pub tokens:         Option<String>,
    pub num_threads:    i32,                 // default 1
    pub provider:       Option<String>,      // default Some("cpu")
    pub debug:          bool,                // NOT i32
    pub model_type:     Option<String>,
    pub modeling_unit:  Option<String>,      // cjkchar | bpe | cjkchar+bpe
    pub bpe_vocab:      Option<String>,
    pub tokens_buf:     Option<Vec<u8>>,
}

pub struct OnlineRecognizerConfig {
    pub feat_config:                sys::FeatureConfig, // { sample_rate: i32, feature_dim: i32 }
    pub model_config:               OnlineModelConfig,
    pub decoding_method:            Option<String>,     // "greedy_search" | "modified_beam_search"
    pub max_active_paths:           i32,
    pub enable_endpoint:            bool,               // NOT i32 (0/1)
    pub rule1_min_trailing_silence: f32,
    pub rule2_min_trailing_silence: f32,
    pub rule3_min_utterance_length: f32,
    pub hotwords_file:              Option<String>,
    pub hotwords_score:             f32,
    pub ctc_fst_decoder_config:     OnlineCtcFstDecoderConfig,
    pub rule_fsts:                  Option<String>,
    pub rule_fars:                  Option<String>,
    pub blank_penalty:              f32,
    pub hotwords_buf:               Option<Vec<u8>>,
    pub hr:                         HomophoneReplacerConfig,
}
```

Notes:
- `feat_config` is the **sys** `FeatureConfig` struct directly:
  `sys::FeatureConfig { sample_rate: 16000, feature_dim: 80 }` (its `Default`).
- `OnlineRecognizerConfig` and `OnlineModelConfig` both implement `Default`.
  The idiomatic pattern is "default then assign fields" (see the official
  example), which avoids having to spell out every nested struct.

### 1.3 Result type — how to read text/tokens/timestamps

`get_result` parses the C-API JSON (`SherpaOnnxGetOnlineStreamResultAsJson`)
into:

```rust
#[derive(Clone, Debug, serde::Deserialize)]
pub struct RecognizerResult {
    pub text:       String,            // <- the recognized text
    pub tokens:     Vec<String>,
    pub timestamps: Option<Vec<f32>>,  // seconds per token (when available)
    pub segment:    Option<i32>,
    pub start_time: Option<f32>,
    pub is_final:   bool,
}
```

So the stale `result.trim()` (treating the result as a `String`) is replaced by:
`recognizer.get_result(&stream)` → `Option<RecognizerResult>` → read `.text`
(then `.trim()` the `String` field if you like). `is_endpoint` is a **separate**
recognizer call, not a field on the result.

### 1.4 Canonical official usage (from `rust-api-examples/examples/streaming_zipformer.rs`)

```rust
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig};

let mut cfg = OnlineRecognizerConfig::default();
cfg.model_config.transducer.encoder = Some(encoder_path);   // String
cfg.model_config.transducer.decoder = Some(decoder_path);
cfg.model_config.transducer.joiner  = Some(joiner_path);
cfg.model_config.tokens   = Some(tokens_path);
cfg.model_config.provider = Some("cpu".to_string());
cfg.model_config.num_threads = 2;
cfg.enable_endpoint  = true;
cfg.decoding_method  = Some("greedy_search".to_string());

let recognizer = OnlineRecognizer::create(&cfg).expect("create recognizer");
let stream = recognizer.create_stream();

for chunk in wave.samples().chunks(3200) {
    stream.accept_waveform(16000, chunk);
    while recognizer.is_ready(&stream) {
        recognizer.decode(&stream);
        if let Some(r) = recognizer.get_result(&stream) {
            if !r.text.is_empty() { /* partial */ }
        }
        if recognizer.is_endpoint(&stream) { recognizer.reset(&stream); }
    }
}
// tail padding + flush
stream.accept_waveform(16000, &vec![0.0f32; (16000.0 * 0.3) as usize]);
stream.input_finished();
while recognizer.is_ready(&stream) { recognizer.decode(&stream); /* read result */ }
```

### 1.5 Corrected `process_chunk` for `sherpa_streaming.rs` (copy-paste)

This is the minimal idiomatic rewrite of the broken `new()` + `process_chunk()`.
Field validation / path construction from the existing file is unchanged; only the
sherpa calls are corrected.

```rust
use sherpa_onnx::{OnlineRecognizer, OnlineRecognizerConfig, OnlineStream};

pub fn new(config: &SherpaStreamingConfig) -> Result<Self, String> {
    // ... build encoder_path/decoder_path/joiner_path/tokens_path Strings &
    //     existence-check them exactly as before ...

    let mut rec_config = OnlineRecognizerConfig::default();
    rec_config.model_config.transducer.encoder = Some(encoder_path);
    rec_config.model_config.transducer.decoder = Some(decoder_path);
    rec_config.model_config.transducer.joiner  = Some(joiner_path);
    rec_config.model_config.tokens      = Some(tokens_path);
    rec_config.model_config.num_threads = 2;
    rec_config.model_config.provider    = Some("cpu".to_string());
    rec_config.model_config.debug       = false;
    rec_config.decoding_method = Some("greedy_search".to_string());
    rec_config.max_active_paths = 4;
    rec_config.enable_endpoint = config.enable_endpoint_detection;
    rec_config.rule1_min_trailing_silence = 2.4;
    rec_config.rule2_min_trailing_silence = 1.2;
    rec_config.rule3_min_utterance_length = 20.0;

    let recognizer = OnlineRecognizer::create(&rec_config)
        .ok_or_else(|| "Failed to create sherpa-onnx OnlineRecognizer".to_string())?;
    let stream = recognizer.create_stream();

    Ok(Self { recognizer, stream })
}

/// Feed a 16 kHz mono f32 chunk; returns Some((text, is_endpoint)).
pub fn process_chunk(&mut self, samples: &[f32]) -> Option<(String, bool)> {
    self.stream.accept_waveform(16000, samples);

    while self.recognizer.is_ready(&self.stream) {
        self.recognizer.decode(&self.stream);
    }

    let text = self
        .recognizer
        .get_result(&self.stream)            // Option<RecognizerResult>
        .map(|r| r.text.trim().to_string())  // read the .text field
        .unwrap_or_default();

    let is_endpoint = self.recognizer.is_endpoint(&self.stream);
    if is_endpoint {
        self.recognizer.reset(&self.stream);
    }

    if text.is_empty() { None } else { Some((text, is_endpoint)) }
}

pub fn reset(&mut self) {
    self.recognizer.reset(&self.stream);
}
```

Key diffs vs. the broken file:
- `OnlineRecognizer::new` → `OnlineRecognizer::create(...).ok_or_else(...)?`
  (returns `Option`, not the value).
- Config built via `..default()` + field assignment; all path fields are
  `Some(String)`, not `&str`.
- `enable_endpoint` is `bool`, not `0`/`1` `i32`.
- `get_result` returns `Option<RecognizerResult>`; read `.text` (then `.trim()`).
- Optionally call `self.stream.input_finished()` when the audio source ends, to
  flush the trailing context.

---

## 2. Offline speaker diarization — current API

Source of truth: `sherpa_onnx::offline_speaker_diarization` +
`sherpa_onnx::speaker_embedding` (docs.rs, 1.13.2).

### 2.1 Config structs (exact fields + defaults)

```rust
pub struct OfflineSpeakerSegmentationPyannoteModelConfig {
    pub model: Option<String>,           // path to pyannote model.onnx
}

pub struct OfflineSpeakerSegmentationModelConfig {
    pub pyannote:    OfflineSpeakerSegmentationPyannoteModelConfig,
    pub num_threads: i32,                // default 1
    pub debug:       bool,               // default false
    pub provider:    Option<String>,     // default Some("cpu")
}

// from speaker_embedding.rs
pub struct SpeakerEmbeddingExtractorConfig {
    pub model:       Option<String>,     // path to 3D-Speaker / WeSpeaker .onnx
    pub num_threads: i32,                // default 1
    pub debug:       bool,               // default false
    pub provider:    Option<String>,     // default Some("cpu")
}

pub struct FastClusteringConfig {
    pub num_clusters: i32,               // default -1
    pub threshold:    f32,               // default 0.5
}

pub struct OfflineSpeakerDiarizationConfig {
    pub segmentation:     OfflineSpeakerSegmentationModelConfig,
    pub embedding:        SpeakerEmbeddingExtractorConfig,
    pub clustering:       FastClusteringConfig,
    pub min_duration_on:  f32,           // default 0.3 (s)  drop shorter speech
    pub min_duration_off: f32,           // default 0.5 (s)  merge gaps shorter than this
}
```

All five config structs derive `Default`.

#### `FastClusteringConfig` semantics for unknown speaker count

- `num_clusters > 0` → **known** speaker count; clustering forces exactly that
  many clusters and `threshold` is **ignored**.
- `num_clusters = -1` (default) → **unknown** speaker count; clustering uses
  `threshold` (cosine distance). **Smaller threshold ⇒ more clusters (more
  speakers); larger threshold ⇒ fewer clusters (fewer speakers).** Default
  `0.5` is a reasonable starting point; tune per embedding model.

For your "unbounded speakers" requirement: set `num_clusters = -1` and pick a
`threshold` (start ~0.5, raise to merge over-split speakers, lower to split
under-segmented ones).

### 2.2 `OfflineSpeakerDiarization` API

```rust
impl OfflineSpeakerDiarization {
    pub fn create(config: &OfflineSpeakerDiarizationConfig) -> Option<Self>;
    pub fn sample_rate(&self) -> i32;                  // expected input rate (16000)
    pub fn set_config(&self, config: &OfflineSpeakerDiarizationConfig);
    pub fn process(&self, samples: &[f32]) -> Option<OfflineSpeakerDiarizationResult>;
}

pub struct OfflineSpeakerDiarizationSegment {
    pub start:   f32,   // seconds
    pub end:     f32,   // seconds
    pub speaker: i32,   // cluster id (speaker_00, speaker_01, ...)
}

impl OfflineSpeakerDiarizationResult {
    pub fn num_speakers(&self) -> i32;
    pub fn num_segments(&self) -> i32;
    pub fn sort_by_start_time(&self) -> Vec<OfflineSpeakerDiarizationSegment>;
}
```

- `create` returns `Option<Self>` (`None` on failure — bad/missing model files).
- `process` returns `Option<OfflineSpeakerDiarizationResult>`. It is a **whole-file**
  call — feed the entire mono waveform at once, not chunk-by-chunk.
- **Both `OfflineSpeakerDiarization` and `OfflineSpeakerDiarizationResult` are
  `Send + Sync`** (`unsafe impl` in the crate). Safe to hold inside a worker /
  move across threads for single-object usage. There is no public progress
  callback in the Rust wrapper (the C API has one; not surfaced here).

### 2.3 Canonical official usage (from `rust-api-examples/examples/offline_speaker_diarization.rs`)

```rust
use sherpa_onnx::{
    FastClusteringConfig, OfflineSpeakerDiarization, OfflineSpeakerDiarizationConfig,
    OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    SpeakerEmbeddingExtractorConfig,
};

let config = OfflineSpeakerDiarizationConfig {
    segmentation: OfflineSpeakerSegmentationModelConfig {
        pyannote: OfflineSpeakerSegmentationPyannoteModelConfig {
            model: Some("./sherpa-onnx-pyannote-segmentation-3-0/model.onnx".into()),
        },
        ..Default::default()
    },
    embedding: SpeakerEmbeddingExtractorConfig {
        model: Some("./3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx".into()),
        ..Default::default()
    },
    clustering: FastClusteringConfig {
        num_clusters: -1,   // unknown number of speakers
        threshold: 0.5,     // tune for over/under-splitting
    },
    ..Default::default()
};

let sd = OfflineSpeakerDiarization::create(&config)
    .expect("Failed to initialize offline speaker diarization");

// samples must be mono f32 at sd.sample_rate() (16000). Resample/downmix first.
assert_eq!(sd.sample_rate(), 16000);

let result = sd.process(&samples).expect("diarization failed");
println!("speakers={}, segments={}", result.num_speakers(), result.num_segments());
for s in result.sort_by_start_time() {
    println!("{:.3}--{:.3} speaker_{:02}", s.start, s.end, s.speaker);
}
```

### 2.4 (Optional) manual embedding + clustering path

If you ever need to bypass `OfflineSpeakerDiarization` and do embeddings yourself,
`sherpa_onnx::speaker_embedding` exposes `SpeakerEmbeddingExtractor`
(`create`, `create_stream`, `is_ready`, `compute -> Option<Vec<f32>>`, `dim`) and
`SpeakerEmbeddingManager` (`add`/`search`/`get_best_matches`/`verify`). For the
brief's "embedding + clustering" goal, the bundled `OfflineSpeakerDiarization`
already wires pyannote-segmentation + 3D-Speaker embedding + FastClustering
together, so prefer it.

---

## 3. Pretrained models to download

Both releases are reachable from
<https://k2-fsa.github.io/sherpa/onnx/speaker-diarization/index.html>.

### 3.1 Segmentation (pyannote segmentation-3.0)

Release tag: `speaker-segmentation-models`
(<https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-segmentation-models>).

| Asset | Size | Notes |
|---|---|---|
| `sherpa-onnx-pyannote-segmentation-3-0.tar.bz2` | **6.6 MB** | the one you want |
| `sherpa-onnx-reverb-diarization-v1.tar.bz2` | 10.4 MB | alt (Revai) |
| `sherpa-onnx-reverb-diarization-v2.tar.bz2` | 242.3 MB | alt, large |

Download URL:
```
https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2
```
After extraction the directory layout is:
```
sherpa-onnx-pyannote-segmentation-3-0/
├── model.onnx          # <- segmentation.pyannote.model points here
├── model.int8.onnx     # optional quantized variant
├── LICENSE
├── README.md
└── ... (test wavs)
```

### 3.2 Speaker embedding (3D-Speaker eres2net / WeSpeaker)

Release tag: `speaker-recongition-models` (note the upstream spelling "recongition")
(<https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models>).
These are **single `.onnx` files**, not tarballs — point `embedding.model` straight at the file.

| Asset (recommended) | Size | Lang |
|---|---|---|
| `3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx` | 37.8 MB | zh (used in official example) |
| `3dspeaker_speech_eres2netv2_sv_zh-cn_16k-common.onnx` | 68.1 MB | zh, newer |
| `3dspeaker_speech_eres2net_sv_en_voxceleb_16k.onnx` | 25.3 MB | **en** (good default for English) |
| `3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx` | 28.2 MB | en, CAM++ |
| `wespeaker_en_voxceleb_CAM++.onnx` | 27.9 MB | en (WeSpeaker alt) |
| `wespeaker_en_voxceleb_resnet152_LM.onnx` | 75.5 MB | en, high accuracy |

Download URL pattern (file is the asset name directly):
```
https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/<asset-name>.onnx
```
e.g. English default:
```
https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/3dspeaker_speech_eres2net_sv_en_voxceleb_16k.onnx
```

Recommended on-disk layout for this project (mirrors the streaming-ASR
`model_dir` convention):
```
<models>/diarization/
├── sherpa-onnx-pyannote-segmentation-3-0/model.onnx
└── 3dspeaker_speech_eres2net_sv_en_voxceleb_16k.onnx
```

All embedding + segmentation models here are **16 kHz**.

---

## 4. ONNX Runtime linking — and the parakeet-rs conflict

### 4.1 How sherpa-onnx links ORT (changed across the version range!)

**This is the highest-risk area.** The linking model differs between the
requested `1.12` and the resolved `1.13.2`:

- **`sherpa-onnx-sys` 1.12.x** (`build.rs`): links **dynamic** libs
  (`cargo:rustc-link-lib=dylib=sherpa-onnx-c-api` and
  `dylib=onnxruntime`) and expects you to set **`SHERPA_ONNX_LIB_DIR`** to a
  folder containing `libsherpa-onnx-c-api` + `libonnxruntime`. If unset it just
  warns and link fails. So the 1.12 binding required shipping/locating ORT
  shared libs yourself.
- **`sherpa-onnx` 1.13.2** (what is actually locked here): **links statically by
  default** and, if `SHERPA_ONNX_LIB_DIR` is **not** set, the build script
  **auto-downloads** a matching prebuilt `…-static-lib` archive from GitHub
  releases and links it. The bundled archive includes ONNX Runtime, so you get a
  self-contained static binary with no extra env vars. Per-platform default
  archives:
  - Linux x86_64: `sherpa-onnx-v1.13.2-linux-x64-static-lib.tar.bz2`
  - Linux aarch64: `sherpa-onnx-v1.13.2-linux-aarch64-static-lib.tar.bz2`
  - macOS x86_64: `sherpa-onnx-v1.13.2-osx-x64-static-lib.tar.bz2`
  - macOS arm64: `sherpa-onnx-v1.13.2-osx-arm64-static-lib.tar.bz2`
  - Windows x64: `sherpa-onnx-v1.13.2-win-x64-static-MT-Release-lib.tar.bz2`
    (note: built with the static **MT** CRT).

  Opt into shared libs with
  `sherpa-onnx = { version = "1.13", default-features = false, features = ["shared"] }`;
  override either mode with `SHERPA_ONNX_LIB_DIR=/path/to/lib`.

**Build-env requirements (1.13 default):** network access at build time for the
auto-download (or pre-set `SHERPA_ONNX_LIB_DIR` for offline/CI builds); a working
C/C++ toolchain. On Windows the bundled static archive uses the **/MT** runtime —
mixing `/MT` (sherpa) with `/MD` (other crates' C deps) in the same binary can
itself cause CRT conflicts. macOS/Linux add an rpath only in the
`SHERPA_ONNX_LIB_DIR` shared path.

> Recommendation: bump the manifest to `sherpa-onnx = "1.13"` so the static
> auto-download behavior is intentional, and so future `cargo update` can't
> silently swing linking behavior.

### 4.2 Confirmed conflict with `parakeet-rs`

Locked: `parakeet-rs 0.3.5`. Its `Cargo.toml` depends on the **`ort`** crate
(`ort = "2.0.0-rc.12"`, features `std`, `ndarray`, `api-24`; its `default`
feature set enables `cpu` + `ort-defaults` = `ort/default`). `ort` provides its
**own** ONNX Runtime (downloaded/linked by `ort`'s build).

So in one binary you would have **two** independent copies of ONNX Runtime:
1. `parakeet-rs` → `ort` → ONNX Runtime, and
2. `sherpa-onnx` → `sherpa-onnx-sys` → bundled (static) ONNX Runtime.

Both export the same `OrtGetApiBase`/ORT C symbols. Linking both into one
artifact causes **duplicate-symbol / multiply-defined ORT** link errors (or, with
dynamic loading, runtime "two ORT versions loaded" instability). This is exactly
why `src-tauri/Cargo.toml` puts them behind **mutually-exclusive optional
features**:
- `diarization = ["dep:parakeet-rs"]` (Sortformer streaming diarization via `ort`)
- `sherpa-streaming = ["dep:sherpa-onnx"]`

and the comments there explicitly say sherpa is "Optional to avoid ONNX Runtime
linker conflicts with parakeet-rs." **Do not enable both features at once.**
The new offline-diarization work (this brief) rides on the `sherpa-streaming`
feature/ORT, so it composes with sherpa-streaming ASR but remains mutually
exclusive with the `parakeet-rs`-based `diarization` feature.

---

## 5. Sample-rate / mono requirements for the diarizer input

- **Sample rate: 16 kHz.** `OfflineSpeakerDiarization::sample_rate()` reports the
  rate the segmentation model expects (16000 for pyannote-segmentation-3.0 and
  all listed 3D-Speaker/WeSpeaker `…_16k` embeddings). The official example
  `assert_eq!(sd.sample_rate(), wave.sample_rate())` — feeding a different rate is
  a usage error; **resample to 16 kHz first**.
- **Mono, single channel.** `process(&[f32])` takes a flat slice of mono samples
  in `[-1.0, 1.0]`. Down-mix stereo/multi-channel to mono before calling.
- **Whole utterance.** Offline diarization is a one-shot, whole-file call — pass
  the complete recording, not streaming chunks.
- The streaming ASR path has the same 16 kHz mono f32 expectation
  (`feat_config.sample_rate = 16000`, `accept_waveform(16000, &[f32])`).

---

## Appendix — primary sources

- docs.rs source, `online_asr.rs` (1.13.2):
  <https://docs.rs/sherpa-onnx/latest/src/sherpa_onnx/online_asr.rs.html>
- docs.rs source, `offline_speaker_diarization.rs` (1.13.2):
  <https://docs.rs/sherpa-onnx/latest/src/sherpa_onnx/offline_speaker_diarization.rs.html>
- docs.rs source, `speaker_embedding.rs` (1.13.2):
  <https://docs.rs/sherpa-onnx/latest/src/sherpa_onnx/speaker_embedding.rs.html>
- `rust-api-examples/examples/streaming_zipformer.rs` and
  `…/offline_speaker_diarization.rs` (k2-fsa/sherpa-onnx, master).
- `sherpa-onnx-sys` 1.12.31 `build.rs` (dynamic + `SHERPA_ONNX_LIB_DIR`):
  <https://docs.rs/crate/sherpa-onnx-sys/latest/source/build.rs>
- crate docs §Setup (1.13.2 static-by-default + auto-download archive names):
  <https://docs.rs/sherpa-onnx>
- Model releases: `speaker-segmentation-models`, `speaker-recongition-models`
  (asset names/sizes pulled live via GitHub API).
- `parakeet-rs` 0.3.5 `Cargo.toml` (`ort = "2.0.0-rc.12"`):
  <https://docs.rs/crate/parakeet-rs/0.3.5/source/Cargo.toml>
- DeepWiki MCP `k2-fsa/sherpa-onnx` (Rust API Q&A) — cross-checked struct fields,
  defaults, and `Send + Sync`.
- Local: `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock` (resolved 1.13.2),
  `src-tauri/src/asr/sherpa_streaming.rs` (the file to fix).
```