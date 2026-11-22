# Diarization clustering bench

Research only; no product dependency or model change. Results and interpretation:
[2026-11-22 report](../../../docs/research/diarization-clustering-bench-2026-11-22.md).

## Reproduce

From the repository root, with [uv](https://docs.astral.sh/uv/):

```sh
uv run --python 3.12 scripts/research/diarization-clustering-bench/bench.py all
```

The inline dependency block pins direct dependencies; uv manages the interpreter and isolated environment. No global install is required. Alternatively run stages in order:

```sh
uv run --python 3.12 scripts/research/diarization-clustering-bench/bench.py prepare
uv run --python 3.12 scripts/research/diarization-clustering-bench/bench.py quality
uv run --python 3.12 scripts/research/diarization-clustering-bench/bench.py ami
uv run --python 3.12 scripts/research/diarization-clustering-bench/bench.py timing
```

Use `--models titanet` or `--models campp` to restrict a stage. `prepare` caches embeddings; delete the particular `.cache/*npz` to extract them again. `quality` and `ami` overwrite their own result JSON files; `timing` resumes successful rows and retries failed/timed-out rows. Use `timing --models titanet --methods ahc_tuned complete_tuned registry_tuned --fresh` to replace only selected timing methods; delete `.cache/timing-<model>.json` for an entirely fresh speed run. Refresh timing after changing any source/model/protocol or relevant calibrated settings. These commands download public data/models to `data/` and write generated results to `.cache/`, both ignored. HF audio URLs are signed and expire: if an uncached download fails because an old manifest expired, remove `data/librispeech-rows.json` and rerun `prepare` to get fresh URLs. No authentication or credentials are needed.

## Protocol

- Embeddings: sherpa-onnx CPU, one thread, L2 normalization; TitaNet-small (192 dimensions) and English VoxCeleb CAM++ from the [k2-fsa release](https://github.com/k2-fsa/sherpa-onnx/releases/tag/speaker-recongition-models). Each embedding covers 1.5 s of audio with a 0.75 s hop. Model SHA-256 and extraction time are recorded.
- LibriSpeech: enumerate test-clean metadata through the [HF rows API](https://huggingface.co/docs/dataset-viewer/rows), download the first 12 utterances for each of the first/last 12 sorted speaker IDs, extending each speaker's prefix until it supplies at least 100 windows. The first 12 speakers form dev; the last 12 form test. No speaker/utterance crosses this split. Sessions have 160 embeddings, 2/3/4/6/8/10 speakers: the two-speaker case uses 60/40 talk time; with three or more, one speaker contributes 50% and the others share the rest. Four-window turns are randomly interleaved. Windows are sampled **without replacement within a session**; sessions can reuse windows. Dev seeds 101/202; test seeds 11/22/33.
- AMI: ES2004a and TS3003a, Mix-Headset, [pyannote only-words test RTTMs](https://github.com/pyannote/AMI-diarization-setup). An event sweep excludes every reference overlap region. At most 400 equally spaced candidate windows across each entire meeting, retaining time order; no test-meeting tuning. Three cluster initialization seeds, not three independent meetings.
- Average AHC thresholds: 0.20–0.85 by 0.05. Complete AHC: 0.20–1.20 by 0.05. Registry similarity: 0.30–0.85 by 0.05. Genie distance cuts: 0.10–0.70 by 0.05, Gini 0.1/0.3/0.5/0.7/1.0. Select minimum mean dev Hungarian confusion, break ties toward the lower parameter. All test settings are then frozen per embedder.
- **Complete**, not average, is the actual [sherpa 1.13.4 FastClustering linkage](https://github.com/k2-fsa/sherpa-onnx/blob/v1.13.4/sherpa-onnx/csrc/fast-clustering.cc). Both are evaluated; they are not interchangeable.
- NME-SC-style auto-k: binary top-p cosine-neighbor graph, symmetrized, unnormalized Laplacian, minimize `(p/n)/(largest eigengap / largest eigenvalue)`; k=1…12, 30 p candidates up to 25% of n. NME uses at most 512 uniformly spaced points, then scales p to the complete session for spectral assignment. This is a compact NumPy/SciPy implementation of the [Park/NeMo mechanism](https://github.com/NVIDIA/NeMo/blob/main/nemo/collections/asr/parts/utils/offline_clustering.py), not a bit-for-bit NeMo port; no multiscale fusion, short-session enhancement, majority vote, or connectivity repair. Spectral uses the lowest-k eigenvectors of the full sparse Laplacian plus Lloyd.
- k-means: sklearn Lloyd, k-means++, 10 restarts. Silhouette selects k=2…12 on up to 512 seeded sample points, using cosine silhouette on Lloyd partitions. e-means receives **the same selected k**, and uses the actual Faiss 1.16 NumPy reference implementation, five epochs; final labels are reassigned to final centroids. Both operate on normalized embeddings with squared Euclidean objectives, not spherical k-means. Oracle-k is diagnostic, not a deployable unknown-count solution and not a mathematical upper bound on all clustering methods.
- Genie: actual `metric="cosine"`, fixed k (oracle and NME estimate), and externally chosen hierarchy distance cut. Gini is not a speaker-count estimator. The five oracle Gini values are all reported, rather than selecting a test-set winner.
- HDBSCAN: sklearn precomputed cosine distance, minimum cluster size 5, minimum samples 3, allow one cluster; other settings are defaults. No special noise reassignment or oracle-k repair.
- Registry: greedy one-to-one assignment of all qualifying local/global cosine pairs in descending score, deterministic `(local, global)` tie-breaking; unmatched locals mint a speaker; matched locals with at least 1.5 s active duration update the count-weighted, renormalized running centroid. Mirrors `stabilize.rs` for valid finite embeddings. Local clusters come from complete-linkage AHC over successive eight-embedding batches, **not reference speaker labels**. Their embeddings are means of segment embeddings; duration is the union on a synthetic 0.75 s-hop grid. AMI embeddings retain chronology, but sampling/gaps mean each batch does not represent a fixed six-second real-audio window. This isolates the registry without pretending to reproduce neural segmentation/worker concatenated-audio embeddings. A second default-threshold path supplies one 1.5 s local at a time. Invalid-input sentinel behavior is outside this data experiment.
- Metrics: ARI; purity = sum of maximum reference-speaker count per predicted cluster / n; coverage = sum of maximum predicted-cluster count per reference speaker / n; speaker-count error = predicted non-noise k minus true k; confusion = 1 minus maximum one-to-one Hungarian matched segments / n. Unmatched clusters and HDBSCAN noise are wrong; no many-to-one mapping that hides fragmentation. Purity/coverage exclude noise, ARI treats `-1` as a partition label. These are **embedding-level**, not time-weighted DER.
- Timing: 100/1,000/10,000 embeddings bootstrapped from real eight-speaker test embeddings, three seeds; this duplicates vectors **only for scaling**, not quality. Each method runs in its own subprocess, 180 s cap. Timers exclude interpreter/import and data load; include pairwise distances, k-selection and final fit. One native thread. Spectral uses a block-built sparse graph for the full-session eigensolve. GPU, optimized Faiss tensor kernels, and cross-platform speed are not measured.

VBx is deliberately not mislabeled as an arbitrary smoothing pass: the [BUT recipe](https://github.com/BUTSpeechFIT/VBx/blob/master/VBx/vbhmm.py) needs an embedding-compatible LDA/centering transform and PLDA parameters. Neither TitaNet-small nor CAM++ supplies those parameters in the downloaded release. The benchmark therefore skips VBx rather than using incompatible x-vector calibration or training on test labels.

## Artifacts and licenses

`.cache/environment.json`, `extraction-*.json`, `settings-*.json`, `quality-*.json`, `ami-quality-*.json`, `timing-*.json` hold machine-readable evidence. `.npz` banks retain public-data embeddings/labels and AMI timestamps. Audio is never added to source control.

LibriSpeech: Vassil Panayotov, Guoguo Chen, Daniel Povey, Sanjeev Khudanpur, [OpenSLR 12](https://www.openslr.org/12/), [HF card](https://huggingface.co/datasets/openslr/librispeech_asr), CC BY 4.0. AMI Meeting Corpus: Jean Carletta et al., [corpus](https://groups.inf.ed.ac.uk/ami/corpus/), [CC BY 4.0 license](https://groups.inf.ed.ac.uk/ami/corpus/license.shtml). Audio is converted to overlapping windows for research; none redistributed here.

Faiss/e-means is [MIT](https://github.com/facebookresearch/faiss/blob/main/LICENSE). The installed Genie 1.3.0 is **GNU AGPL v3**, confirmed from package metadata and [upstream](https://github.com/gagolews/genieclust). It is a research-environment dependency only. Shipping its implementation requires a license review; alternatively evaluate a clean implementation of the paper, with legal review. The TitaNet-small release's checkpoint license is not established by this experiment; a toolkit license must not be silently substituted for checkpoint terms.

NME and the registry are independently written from the cited algorithm descriptions and project rules, not copied implementations. Project XERJ registry was checked; this checkout has no registered corpus. No shared index/global configuration was changed.
