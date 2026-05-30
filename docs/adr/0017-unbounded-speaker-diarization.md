# ADR-0017: Unbounded speaker diarization via sherpa-onnx embedding + clustering

## Status

Proposed (2026-05-30). Records the architecture ahead of code, backed by a
feasibility investigation (real API + build probe). Scoped to **speaker
diarization** for the live transcript / knowledge-graph attribution path.

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
   per-speaker embedding centroids. Emit `SPEAKER_DETECTED` as today.
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
