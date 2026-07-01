# Research: sherpa-onnx live speaker diarization (B16 / ADR-0017) — 2026-05-30

Live wiring of `OfflineSpeakerDiarization` over a rolling window with cross-window
label stabilization, for the official `sherpa-onnx` Rust crate (already wrapped by
`ClusteringDiarizer`). Sources: k2-fsa.github.io/sherpa, docs.rs/sherpa-onnx,
k2-fsa/sherpa-onnx releases, Coria et al. 2021 (arXiv 2109.06483), diart.

## Key architectural fact
`OfflineSpeakerDiarization::process()` re-runs segmentation→embedding→**fresh
FastClustering** every call. Cluster indices are permutation-arbitrary: speaker_00 in
window N != speaker_00 in window N+1. **Cross-window stabilization is mandatory.**

## Models (verified URLs)
Segmentation (MIT, commercial-ok), tag `speaker-segmentation-models`:
`https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2`
  → extracts `sherpa-onnx-pyannote-segmentation-3-0/model.onnx` (5.7MB) + `model.int8.onnx` (1.5MB).

Embedding, tag `speaker-recongition-models` (sic — real typo in URL):
- `nemo_en_titanet_small.onnx` (fast, dim=192) — best for live/English. RTF combo ~0.11.
  `https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_small.onnx`
- `3dspeaker_speech_eres2net_base_sv_zh-cn_3dspeaker_16k.onnx` (~37MB, multilingual). RTF ~0.24-0.30.
No published SHA: verify by `create()->Some` and `sample_rate()==16000`. Don't hardcode
embedding dim (TitaNet=192, others 256/512) — query at runtime.
Recommended live combo: `model.int8.onnx` + `nemo_en_titanet_small.onnx` (RTF ~0.11).

## Config structs (official sherpa-onnx crate)
```
OfflineSpeakerDiarizationConfig { segmentation, embedding, clustering,
  min_duration_on: f32 /*0.3*/, min_duration_off: f32 /*0.5*/ }
OfflineSpeakerSegmentationModelConfig { pyannote{model:Option<String>}, num_threads, debug, provider }
SpeakerEmbeddingExtractorConfig { model:Option<String>, num_threads, debug, provider }
FastClusteringConfig { num_clusters:i32 /* -1 = unknown -> use threshold */, threshold:f32 /*0.5*/ }
```
Methods: `create(&cfg)->Option`, `sample_rate()->i32`, `set_config()` (retune w/o rebuild),
`process(&[f32])->Option<Result>`; Result `num_speakers()`, `sort_by_start_time()->Vec<Segment{start,end,speaker:i32}>`.
`num_clusters=-1` → threshold (cosine-distance cut): higher=fewer speakers. Start 0.7, tune.

## Rolling window
- Window W = 10-15s; hop H = 2-5s; overlap = W-H (e.g. 12s/3s → 9s overlap).
- Only emit/commit trailing H seconds (older already emitted); overwrite trailing H on
  re-run, freeze earlier. Don't run until buffer >= W_min (~6-8s).
- abs_time = buffer_start_abs + local_seconds.

## Label stabilization (core)
Global registry of L2-normalized embedding centroids; per window:
1. One embedding per local cluster (concat its in-buffer segments;
   `EmbeddingExtractor::compute_speaker_embedding(samples,sr)`), L2-normalize; track duration Δ_l.
2. `S[l][g]` = cosine(e_l, centroid_g).
3. Assign with cannot-link (two locals in same window != same global): Hungarian on 1-S
   (crate `pathfinding`/`hungarian`) or greedy by descending S, accept if S>=sim_threshold (~0.55-0.70).
4. Unmatched local → new global id seeded with e_l.
5. Centroid update only for Δ_l>=~1.5s: centroid_g = normalize((centroid_g*count+e_l)/(count+1)); count++.
6. Relabel window segments local→global; emit. Optional: require new global in 2 consecutive
   windows before exposing (suppress spurious). Cap registry for long sessions.
sim_threshold (cross-window new-speaker cut) is SEPARATE from clustering.threshold (within-window).

## Perf / threading
- RTF ~0.11 (int8 seg + TitaNet) → ~1.3s per 12s window; with H=3s sustainable real-time.
- Audio thread: only copy samples into lock-free SPSC ring (rtrb/ringbuf), no ONNX/locks.
- Dedicated diarization WORKER thread drains ring + runs process() every H; emits via mpsc/crossbeam.
- num_threads 1-2 (ORT has own pool). Build diarizer ONCE; reuse; set_config() to retune live.
- Latency ~H + compute (~3-5s to stable label); emit provisional + correct next window if needed.

## To verify locally before building
1. embedding dim of chosen model (runtime). 2. tune clustering.threshold + sim_threshold on real audio.
