# ADR-0017: Unbounded speaker diarization via sherpa-onnx embedding + clustering

## Status

Accepted (2026-05-30; originally recorded as Proposed the same day). Records the
architecture, backed by a feasibility investigation (real API + build probe).
**Engine + rolling-window worker + model downloads + pipeline wiring all landed
and model-validated**, and the live worker now normalizes its output into the
provider-neutral `SpeakerTimeline` revision ledger alongside provider labels
(Seed `audio-graph-eb6c`, merged 2026-06-28; see "Implementation status"). The one
remaining accuracy gate is multi-speaker verification on a curated clip. Scoped to
**speaker diarization** for the live transcript / knowledge-graph attribution path;
physical per-speaker audio-channel projection is explicitly research-gated (below).

## Implementation status (2026-05-30)

Landed:
- **B01 prerequisite done:** `src/asr/sherpa_streaming.rs` rewritten for the real
  sherpa-onnx **1.13** API; manifest bumped to `"1.13"`. The `sherpa-streaming`
  feature now compiles (verified on Linux/WSL — sherpa-onnx 1.13.2 static ORT).
- **`diarization-clustering` feature** added, pulling `sherpa-onnx`, **mutually
  exclusive** with the parakeet `diarization` feature via a `compile_error!`
  guard in `lib.rs` (verified: enabling both fails fast with the intended
  message).
- **Engine module** `src/diarization/clustering.rs`: `ClusteringDiarizer` wraps
  `OfflineSpeakerDiarization` (pyannote segmentation + 3D-Speaker embedding +
  `FastClusteringConfig { num_clusters: -1, threshold }`) → unbounded speaker
  count; `diarize(&[f32]) -> Vec<ClusterSegment>`. A `#[cfg(not(...))]` stub
  keeps the type referenceable in every build. An env-gated, model-backed test
  (`AG_DIAR_*`) is included (skipped in CI; no models there).

Landed (2026-05-30, B16 wave — clippy `--features diarization-clustering
--all-targets -D warnings` green; default build unaffected; tests compile,
execution CI-gated per ADR-0007 Windows CRT skew):
- **Model downloads DONE:** `models/mod.rs` generalized the archive downloader
  (`archive_required_files`) so Zipformer + the pyannote-segmentation-3.0
  `.tar.bz2` share one bzip2/tar extract path; registered pyannote-seg-3.0
  (archive → `model.int8.onnx`) + `nemo_en_titanet_small.onnx` (bare), URLs
  verified 200 (the "recongition" typo retained), `expected_size: None`.
- **Rolling-window worker DONE:** `diarization/worker.rs` `LiveDiarizationWorker`
  — one `ClusteringDiarizer` + one `SpeakerEmbeddingExtractor` (real 1.13.2
  **stream** API: `create_stream`/`accept_waveform`/`input_finished`/`is_ready`/
  `compute`) + `SpeakerRegistry` + `WindowSchedule`, fed by a lock-free `ringbuf`
  0.4 SPSC ring drained on a dedicated `std::thread`; per-cluster embed → assign
  → relabel → emit trailing-hop segments; rolling-buffer trim. Pure glue
  unit-tested (sample-slicing, trailing-hop filter, trim).
- **`DiarizationBackend::Clustering`** variant + `DiarizationConfig::clustering`
  constructor added.

Landed since (B16-pipe `c0eb93b`, B16-offset `f2bcd95`):
- **Pipeline wiring DONE.** `speech/mod.rs` spawns `LiveDiarizationWorker` for the
  `Clustering` backend (`maybe_spawn_clustering_diarization`), feeds it the 16 kHz
  mono tap via the lock-free SPSC `DiarizationFeed`, applies a worker-stamped
  absolute `window_start_sample` → exact session-time offset (robust under ring
  backpressure), maps transcript times by overlap (`overlap_speaker_for_segment`),
  and emits `DIARIZATION_SPAN_REVISION` from a dedicated
  `diarization-clustering-emit` consumer thread. (58 diarization unit-tests executed
  green in WSL, 2026-05-31.)

Landed (SpeakerTimeline normalization seam, Seed `audio-graph-eb6c`, merged
2026-06-28):
- **Provider-neutral `SpeakerTimeline` ledger** (`src-tauri/src/projections.rs`):
  local clustering, the inline Simple/Sortformer per-utterance labels, and provider
  labels (Deepgram/AWS/AssemblyAI, via `diarization_span_revision_for_transcript` in
  `speech/mod.rs`) all normalize into `DiarizationSpanRevision`s keyed by a
  provider-neutral `span_id`. The raw `provider_speaker_id` is retained as
  provenance only and is **never** the durable identity. `apply_event` replaces a
  span's earlier revision (collapsing `Provisional` → `Stable`/`Final` remaps),
  rejects stale revisions, and rejects conflicting same-revision payloads —
  mirroring `TranscriptLedger`.
- **Durable JSONL replay + projection basis:** the speaker timeline persists and
  replays (`persistence::replay_speaker_timeline`), and `ProjectionBasis`'s
  `diarization_span_revisions` are now populated from the timeline and validated
  (`validate_diarization_basis`) instead of being rejected as unavailable. A
  notes/graph patch that cites speaker spans is staleness-gated against the ledger;
  one that cites none is unaffected. Replay fixtures prove a speaker remap *revises*
  the existing span rather than appending a duplicate.

**Model-validated 2026-05-31:** the real pyannote-segmentation-3.0 + TitaNet ONNX
models were downloaded into the app model cache (`app_data_dir()/models`, per
`models::get_models_dir`) and `ClusteringDiarizer::new()` constructs against them
with `sample_rate()==16000`; `diarize()` runs the full segmentation→embedding→
clustering ONNX pipeline end-to-end without error (new env-gated test
`constructs_and_runs_against_real_models`, run in WSL against the real cache).
So model-load + sherpa wiring + ONNX inference are proven real, not just compiled.

Pending:
- **P2 — live retune:** expose `clustering.threshold` + registry `sim_threshold`
  setters via `set_config` (the diarizer's `set_config(&self,…)` supports it).
- **P2 — UI:** a backend selector (Simple / Sortformer-4 / Clustering-∞) + a
  clustering-threshold control (the SpeakerPanel list is already dynamic).
- **Research-gated — physical multi-channel projection:** speaker-separated PCM
  lanes (one audio channel per diarized speaker) are **deliberately not built**. The
  processed pipeline is mono (ADR-0020) and diarization is a metadata join over it;
  `DiarizationSpanRevision.channel` exists so channel-aware providers can be
  normalized later, but `supports_multichannel` stays `false` until both a capture
  source descriptor and a provider adapter prove real, ordered, stable channels and
  the session artifact stores the channel map. See
  [`docs/research/speaker-channel-routing-2026-06-26.md`](../research/speaker-channel-routing-2026-06-26.md).
- **Accuracy verification (the one remaining gate):** the WAV-gated
  `diarizes_a_clip_into_speaker_segments` test currently asserts only `>= 1`
  distinct speaker (it runs against whatever clip the `AG_DIAR_*` env vars point
  at, so it can't hardcode an expected count). Proving the *unbounded* (>4)
  behavior needs a **curated/labeled multi-speaker 16 kHz clip** (a
  data-collection task, not a code/env one) before the assertion can be tightened
  to the known speaker count. Construction + pipeline execution are now
  validated; only the speaker-count accuracy on real multi-speaker audio remains.

## Status (original proposal below)

## Context

Speaker detection today is **bounded and, in practice, hardcoded**
(investigation 2026-05-30):

- The active neural backend is **NVIDIA Sortformer** via `parakeet-rs`
  (`diar_streaming_sortformer_4spk-v2.onnx`), behind the optional `diarization`
  feature. It is **fixed at 4 speakers** — a hard constraint of the model's
  4-channel output (`SORTFORMER_MAX_SPEAKERS = 4`, `[0u64; 4]` accumulators in
  `diarization/mod.rs`). It is *streaming* (`feed`/`flush`).
- The default build (no `diarization` feature) falls back to a **Simple**
  signal-based backend (RMS/ZCR features, nearest-neighbour), dynamic up to a
  hardcoded 10 but not reliable speaker ID.
- The cloud **Deepgram** path supports a `max_speakers` cap (now exposed in the
  UI, 2026-05-30) but is a cloud dependency.

We want **unbounded / unknown-count** local speaker detection (the user's ask:
"embedding + clustering by default, or pyannote").

A feasibility probe established the key facts:

- The project **already depends on `sherpa-onnx` 1.12** (optional, behind
  `sherpa-streaming`; kept optional precisely "to avoid ONNX Runtime linker
  conflicts with parakeet-rs"). That crate exposes
  **`OfflineSpeakerDiarization`** = pyannote **segmentation** +
  **speaker-embedding** extraction + **clustering**, with
  `FastClusteringConfig { num_clusters, threshold }`. Setting `num_clusters = -1`
  with a distance `threshold` yields an **unknown/unbounded** speaker count. Its
  result exposes `num_speakers()` and segments `{ start, end, speaker }`. The
  type is `Send + Sync` (no `!Send` actor needed, unlike the llama engine).
- **`OfflineSpeakerDiarization::process(&[f32])` is offline** — it diarizes a
  *complete* waveform, not an incremental stream. So it cannot drop into the
  current per-segment streaming path unchanged.
- **Two real blockers surfaced:**
  1. The existing `src/asr/sherpa_streaming.rs` no longer compiles against
     sherpa-onnx 1.12 (10 API-drift errors, e.g. `process()` now returns
     `Option<RecognizerResult>`); the `sherpa-streaming` feature is broken
     (unnoticed because it's off by default).
  2. **ONNX Runtime linker conflict**: `sherpa-onnx` and `parakeet-rs` both link
     onnxruntime and can't co-link, so a sherpa-based diarizer must be
     **mutually exclusive** with the parakeet Sortformer backend.

## Decision Drivers

- Support an **unknown, unbounded** number of speakers locally and offline.
- Reuse the already-present `sherpa-onnx` dependency and proven pyannote +
  embedding + clustering pipeline rather than hand-rolling clustering.
- Respect the ORT single-link constraint (no parakeet + sherpa together).
- Keep the real-time UX acceptable despite the diarizer being offline/batch.
- Keep the default build unchanged (feature-gated, opt-in).

## Considered Options

- **Option A — `sherpa-onnx` `OfflineSpeakerDiarization`, run on a rolling
  window (chosen).** New `diarization-clustering` feature pulling `sherpa-onnx`;
  a `DiarizationBackend::Clustering` that buffers session audio and re-diarizes a
  rolling window (e.g. on each finalized utterance or every N seconds) with
  `num_clusters = -1` + threshold; map the offline `{start,end,speaker}`
  segments back onto transcript segments by time overlap. Mutually exclusive
  with `diarization` (parakeet) to dodge the ORT conflict.
- **Option B — Keep Sortformer (4-cap) streaming + offline clustering "refine"
  pass.** Live labels from Sortformer (≤4), then a full-session offline
  re-diarization (sherpa) to relabel with the true count. Blocked by the ORT
  conflict (can't link both); would need the diarizer in a separate process.
- **Option C — Embedding + online (incremental) clustering, hand-rolled.** Run a
  segmentation + embedding model per chunk and maintain an online leader/
  agglomerative clusterer ourselves. True streaming + unbounded, but we
  re-implement what sherpa already does, with more risk.
- **Option D — Port to a non-conflicting embedding/cluster stack** (e.g. ONNX
  via `ort` directly with pyannote + a Rust clusterer). Most control, most work.
- **Option E — pyannote.audio `speaker-diarization-community-1` /
  `speech-separation-ami` directly.** These are PyTorch pipelines; using them in
  this Rust/offline app means ONNX-exported segmentation + embedding components —
  which is exactly what sherpa-onnx already packages. So "use pyannote" is
  realized *through* Option A's sherpa pipeline (pyannote segmentation-3.0 ONNX +
  a 3D-Speaker / wespeaker embedding model), not a separate integration.

## Decision Outcome

Proposed: **Option A** — a feature-gated `Clustering` backend built on
`sherpa-onnx`'s `OfflineSpeakerDiarization` (pyannote segmentation + 3D-Speaker
embedding + fast clustering with `num_clusters = -1`), run on a rolling window,
**mutually exclusive** with the parakeet Sortformer backend. It directly
delivers unbounded speakers, reuses an existing dependency and a battle-tested
pipeline, and keeps the default build untouched. Option C/D are more work for
no near-term gain; Option B is blocked by the ORT conflict without process
isolation; Option E reduces to Option A in this stack.

This is **not yet implemented** — it is a multi-part feature with prerequisites
(below). The ADR records the design + the validated API so implementation is a
focused, de-risked effort.

### Consequences

- **Positive:** Unknown/unbounded local speaker count; reuses sherpa-onnx;
  `Send + Sync` (no actor); "pyannote" satisfied via ONNX segmentation.
- **Positive:** Better accuracy than the Simple backend; offline clustering can
  relabel the whole session consistently.
- **Negative / cost:** Offline → **latency**. Speaker labels for the latest
  audio lag until the next window re-diarization; labels can also **re-map**
  between windows (speaker 2 ↔ 3) and need stabilization (anchor by embedding
  centroid across windows). The UI must tolerate label churn.
- **Negative:** **ORT linker conflict** forces `diarization-clustering` XOR
  `diarization` (parakeet) at build time; pick a default and document it.
- **Negative:** **Prerequisite work** — `src/asr/sherpa_streaming.rs` must be
  updated to the sherpa-onnx 1.12 API before (or alongside) enabling any
  sherpa-based build that includes it; new model download entries (pyannote
  segmentation-3.0 ONNX + a 3D-Speaker/wespeaker embedding ONNX) in
  `models/mod.rs`; CPU cost of running two extra ONNX models.
- **Neutral:** Compute budget — re-diarizing a growing window is O(window); cap
  the window (e.g. last N minutes) to bound cost.

## Implementation outline (informational, non-binding)

1. **Unblock sherpa:** fix `src/asr/sherpa_streaming.rs` for the sherpa-onnx 1.12
   API (`process()` → `Option<RecognizerResult>`, etc.). Prereq for any sherpa
   build that compiles that module.
2. **Feature:** add `diarization-clustering = ["dep:sherpa-onnx"]`; make it and
   `diarization` (parakeet) mutually exclusive (compile_error! if both, or a
   documented precedence). Keep both off by default.
3. **Models:** add `models/mod.rs` entries for sherpa pyannote
   segmentation-3.0 ONNX + a 3D-Speaker (`eres2net`) or wespeaker embedding ONNX,
   with download + verification (mirror the Sherpa Zipformer downloader).
4. **Backend:** `diarization/clustering.rs` (`#[cfg(feature =
   "diarization-clustering")]`) wrapping `OfflineSpeakerDiarization::create`
   with `FastClusteringConfig { num_clusters: -1, threshold }`; a
   `DiarizationBackend::Clustering` variant in `diarization/mod.rs`.
5. **Streaming integration:** buffer mono f32 @ the model sample rate; on each
   finalized utterance (or every N s), `process()` the rolling window, then map
   segments to transcript times by overlap; stabilize labels across windows via
   per-speaker embedding centroids. (As implemented, the live worker emits
   `DIARIZATION_SPAN_REVISION` into the provider-neutral `SpeakerTimeline` rather
   than the legacy `SPEAKER_DETECTED`-only path; see "Implementation status".)
6. **Settings/UI:** a backend selector (Simple / Sortformer-4 / Clustering-∞) +
   a clustering threshold; the SpeakerPanel already renders a dynamic list.
7. **Verify:** WSL build of `--features diarization-clustering` (sherpa-onnx +
   ORT), a model-backed env-gated test diarizing a known multi-speaker clip and
   asserting `num_speakers > 4` is achievable.

## References

- Feasibility: `sherpa-onnx` `OfflineSpeakerDiarization` /
  `OfflineSpeakerDiarizationConfig` / `FastClusteringConfig`
  (docs.rs + k2-fsa/sherpa-onnx `rust-api-examples/offline_speaker_diarization.rs`).
- sherpa-onnx speaker-diarization models (pyannote segmentation, 3D-Speaker /
  wespeaker embeddings): k2-fsa/sherpa-onnx releases.
- Current diarization: `src-tauri/src/diarization/mod.rs`,
  `src-tauri/src/models/mod.rs`, `src-tauri/src/asr/sherpa_streaming.rs`.
- Related: ADR-0007 (feature-gate local ML), ADR-0008 (ontology / speaker
  attribution), the 2026-05-30 speaker-detection investigation.
