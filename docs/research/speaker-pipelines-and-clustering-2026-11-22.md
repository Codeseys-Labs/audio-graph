# Speaker pipelines and clustering: what AudioGraph should reuse

Research date: **2026-11-22**. Status: research and recommendations; no product-code or tracker changes.

Question: Which diarization, separation, and identification pipelines used in 2023–2026 offer reusable patterns for AudioGraph's live worker, post-session refinement, and late naming? In particular, are e-means or Genie appropriate speaker clusterers?

This extends, rather than replaces, [the 2026-11-21 identification research](speaker-identification-2026-11-21.md) and [the lazy-identification design](../designs/lazy-speaker-identification.md). Those documents own the product's identity ledger, consent, relabel/replay contracts, and competitor naming survey. Older foundational papers are included where they explain algorithms still used by the 2023–2026 pipelines. Material external claims link to primary sources; **[INFERENCE]** distinguishes analysis and recommendations from published measurements.

## Direct answer

1. **Live:** keep the current small, incremental, overlap-aware clustering architecture in `stabilize.rs`. Improve the speech admitted to centroids before replacing the clusterer. Coria/diart's reusable ingredients are local multi-speaker segmentation, overlap-aware embedding extraction, one-to-one local/global assignment, a new-speaker threshold, and duration-gated updates. AudioGraph uses a greedy assignment, whereas the paper uses Hungarian assignment; they are related designs, not identical implementations. [Coria paper][C1], [diart][C2], [local implementation](../../src-tauri/src/diarization/stabilize.rs).
2. **Post-session:** compare quality-filtered AHC, spectral clustering with estimated speaker count, and constrained VBx. The most directly relevant recent result is **filter short/unreliable embeddings, cluster the good ones, then reassign all local tracks subject to cannot-link constraints**, not faster k-means. The 2025 EEND-VC/VBx paper tests precisely this pattern. Its GMM-VBx variant needs a PLDA backend trained for the embedding space; a PLDA trained for WeSpeaker cannot simply be applied to the current TitaNet-small vectors. [EEND-VC/VBx paper][V2], [pyannote implementation][P5], [INFERENCE: compatibility consequence].
3. **Naming:** leave anonymous acoustic clusters separate from people. Start with model-pinned cosine verification against a small, consented candidate roster, with an explicit unknown outcome. Calibrate acceptance on held-out meeting conditions; add PLDA or AS-norm only if they improve those trials. Preserve the existing manual/source/roster/text-evidence ladder and revision spine. [verification study][I1], [WeSpeaker][W1], [lazy-ID design](../designs/lazy-speaker-identification.md).
4. **e-means:** a useful new optimizer for a **fixed-k k-means objective**, not a diarization pipeline, auto-k estimator, overlap model, or named-speaker recognizer. It is worth a research comparison, but importing Faiss/PyTorch or SuperEMeans into the live worker is not justified by the large-scale vector-index benchmarks. [paper][E1], [Faiss 1.16 guide][E3], [SuperEMeans author benchmark][E5], **[INFERENCE: AudioGraph recommendation]**.
5. **Genie/genieclust:** a legitimate alternative hierarchical partitioner with cosine support, but its Gini correction discourages very unequal cluster sizes, which can represent *real* unequal speaking time. It still needs k or an external tree-cut/count rule. The current upstream implementation is **AGPL-3.0**, including its header-only C++ core: recommend **research-only benchmarking, not shipping unless license review passes**. A separately reviewed, independent implementation from the published algorithm is an option if measurements justify it; translating the AGPL code into Rust is not that option. [author manual][G1], [Python API][G2], [license][G4], **[INFERENCE: adoption/legal-review recommendation, not legal advice]**.
6. **Separation:** keep it off the live path. However, do not claim that meeting separation has no evidence: **PixIT demonstrates offline AMI/AliMeeting transcription gains**, with diarization-driven leakage removal essential to its pipeline. That is evidence for an offline experiment, not proof of local CPU latency or a reason to feed generated sources into live providers. [PixIT][P4], [existing separation decision](source-separation-bakeoff-2026-06-26.md).

**Empirical companion:** [`docs/research/diarization-clustering-bench-2026-11-22.md`](diarization-clustering-bench-2026-11-22.md) measures the clustering options on real TitaNet-small and CAM++ embeddings, using 36 synthetic LibriSpeech sessions and two AMI meetings. It isolates clustering from segmentation, so treat its numbers as clustering-only evidence, not DER. On AMI with unknown k, it found:

- **Spectral NME-SC** was best: 5.75% Hungarian confusion on TitaNet and 3.38% on CAM++.
- **Tuned average-linkage AHC** was close behind: 7.38% on TitaNet and 5.25% on CAM++.
- **Complete-linkage AHC**, which sherpa's FastClustering uses, was far worse even when tuned: 34.38% on TitaNet.
- **e-means** ran (Faiss 1.16 on Windows) but gave no consistent speaker-recovery gain.
- **Genie** with a dev-tuned hierarchy cut scored 18.25% on TitaNet and tied spectral at 3.38% on CAM++.

At 10,000 embeddings, average AHC took 4.32 s and spectral 5.89 s on one CPU thread. Speed is not the deciding constraint. Synthetic embedding partitions, oracle-k results, full diarization DER, and end-to-end ASR quality are different evidence levels.

## 1. What is being clustered, and where AudioGraph already fits

There are three different outputs, which should not be conflated:

- **Diarization:** time intervals of anonymous speaker activity, potentially multiple active speakers per frame. A segmentation model may distinguish local speakers only within a short window; vector clustering supplies recording-wide labels. [pyannote pipeline][P5], [EEND-VC/VBx][V2].
- **Separation:** estimated waveforms, one source per speaker or local output slot. Clustering can align those slots across windows, but clustering itself does not generate a waveform. [PixIT][P4], [separation implementation][P6].
- **Identification/verification:** compare a voice representation with an enrolled person, or combine non-acoustic evidence to name an anonymous cluster. An acoustic cluster index has no inherent human name. [Eagle enrollment/recognition][F2], [pyannoteAI identification capabilities][P3].

**Local evidence.** [`clustering.rs`](../../src-tauri/src/diarization/clustering.rs) wraps sherpa's offline segmentation → embedding → FastClustering pipeline with `num_clusters = -1`; the cosine-**distance** threshold controls count without supplying k. [`stabilize.rs`](../../src-tauri/src/diarization/stabilize.rs) compares normalized local embeddings to session centroids using cosine **similarity**, greedy one-to-one assignment, and count-weighted running updates. Duration gates admission; it does not duration-weight contributions. The two thresholds therefore have opposite directions: a larger offline distance threshold permits more merging; a larger live similarity threshold demands stronger matches. Neither is a calibrated probability. The supplied project context and [prior research §E](speaker-identification-2026-11-21.md#e-speaker-embedding-models-identification-and-verification) identify the active model as `nemo_en_titanet_small.onnx`, 192 dimensions, not the 3D-Speaker model mentioned in some wrapper comments. This report does not change those comments or infer the small checkpoint's license from TitaNet-Large.

**[INFERENCE]** The useful seam is not “replace everything with a clusterer.” It is: retain segment/activity/overlap and duration provenance alongside embeddings; compare offline clusterers on the *same* embedding set; then evaluate any complete replacement pipeline separately. Replacing only clustering can reduce speaker confusion, but cannot recover speech the segmenter never detected. This distinction is also explicit in the fixed-segmentation experiments in [V2].

## 2. pyannote: four different products, not one clustering recipe

| Pipeline | How it clusters or avoids clustering | Output and constraints | License, gate, and deployment |
|---|---|---|---|
| **speaker-diarization-3.1** (2023 generation) | Local powerset segmentation → per-local-speaker embeddings → AHC → global reassignment/reconstruction. The release is **not** clustering-free. The published pyannote baseline uses centroid linkage; average and complete linkage are alternatives, not names for that same rule. | 16 kHz mono; ordinary overlap-aware annotation; optional `num_speakers` or min/max bounds. “Unbounded global count” does not remove the segmentation model's local capacity. | Pipeline MIT; gated HF access, including acceptance of segmentation/pipeline conditions. Segmentation and embedding run in PyTorch in 3.1, replacing 3.0's ONNX Runtime path. Local after authorized download. [card][P1], [baseline analysis][V2]. |
| **speaker-diarization-community-1** (2025 generation, current 2026) | Segmentation → WeSpeaker embedding → AHC initialization → **VBx** → constrained local/global reassignment. Important qualification: the current pyannote VBx utility **drops HMM support and uses GMM updates**; do not attribute temporal HMM smoothing to this implementation merely because its cited paper is named Bayesian HMM clustering. | Ordinary diarization **and a separate exclusive diarization** for easier STT reconciliation; count may be inferred or bounded/forced. Exclusive diarization is an attribution view, not separated audio. | Pipeline **CC-BY-4.0**, gated contact/use-case conditions; pyannote.audio code MIT and the VBx utility Apache-2.0. Authorized download can be cloned for offline operation. No built-in voiceprint identification. [card][P2], [clustering code][P5], [GMM utility][V3], [capabilities][P3]. |
| **precision-2** (hosted) | Proprietary implementation: the public model documentation does not disclose enough to assign a specific clustering algorithm. Do **not** assume it is just Community-1 with a different threshold. | Offline diarization, exclusive view, count controls, confidence-related output and optional voiceprint identification. | Commercial API, processing on pyannoteAI servers; Enterprise self-hosting is separately offered. It is not an openly redistributable desktop checkpoint. Current docs also offer Precision-3 and warn the default will change; pin the model for reproducible research. [official model docs][P3]. |
| **speech-separation-ami-1.0 / PixIT** (2024) | Joint local separation/activity network → speaker embeddings from activity-selected **original mixture** regions → AHC → global source/track stitching. It still clusters. | Both an `Annotation` and separated `SlidingWindowFeature` waveforms; local neural slots are aligned with local activity tracks, then mapped into global speakers. | Pipeline and underlying `separation-ami-1.0` card are MIT; both are gated. The pipeline card requires acceptance for both repositories and specifies `pyannote.audio[separation]==3.3.2`. Its ECAPA dependency has its own checkpoint terms. Local Python reference, not a verified single ONNX export. [pipeline card][P7], [underlying model][P8], [paper][P4], [source][P6]. |

**Gating is not the license.** A publicly readable model card is not authorization to accept its terms, retrieve gated files, or redistribute every component. The Community-1 gate states contact/use-case sharing and CC-BY-4.0; 3.1/PixIT gates and component terms also need a release inventory. No gate was accepted for this research. **[INFERENCE]** Treat model digest, attribution, exact gate/terms version, segmentation/embedding/PLDA dependencies, and redistribution permission as separate release records. Sources: [P1], [P2], [P7], [P8].

### 2.1 What PixIT actually does

The authors' **ToTaToNet** network combines a learned time-domain encoder/decoder and separator with WavLM features and an activity head for each separated source. **PixIT is the training procedure:** permutation-invariant diarization loss (PIT) plus mixture-invariant separation loss (MixIT). It trains on real mixture recordings with diarization labels, constructing mixtures of mixtures; it does not require clean isolated reference waveforms for each meeting speaker. Diarization supervision and limiting the mixture's speaker count address MixIT over-separation. [paper §§2–3][P4].

Inference, separating neural and clustering responsibilities:

1. Run the joint network on **5 s windows, 0.5 s hop**, producing up to **three local speaker activity tracks and three aligned waveform estimates** in the published setup. Three is a local capacity, not a whole-meeting speaker limit. [P4], [underlying card][P8].
2. Threshold the activity estimates. For each active local track, concatenate corresponding regions of the **original audio** and extract a pretrained ECAPA-TDNN embedding. The paper does not claim embeddings are normally extracted from already-separated sources. The reference pipeline has an `embedding_exclude_overlap` option, with documented fallback when clean speech is too short; its constructor default is false. [Paper §2.2][P4], [P6].
3. AHC with a tuned threshold connects local tracks across windows. Use those same global assignments to reorder/reconstruct the local waveform outputs; overlap-add aggregation stitches them into long-form sources. Embeddings resolve permutations across long pauses that adjacency-only source correlation cannot reliably bridge. [Paper §§1–2.2][P4], [P6].
4. **Leakage removal:** zero a global source while its speaker is inactive, optionally retaining an ASR collar around activity. The reference also peak-normalizes sources. These are generated waveform estimates, not clean source-native recordings. [P4], [P6].
5. ASR on each stitched source yields attributed text; joining all sources by word time yields a combined transcript. Training/tuning for lowest cpWER is not identical to tuning for lowest DER. [Paper §§3.2–3.4][P4].

Concrete evidence, **authors' offline measurements, not AudioGraph results**: on AMI-SDM, Whisper large-v2 cpWER is **38.3%** with pyannote 3.1 attribution versus **31.7%** with PixIT separation; un-attributed utterance WER is **35.4% → 33.8%**. In a separate Whisper medium.en ablation, a model without WavLM and without leakage removal produces **50.1% cpWER**, worse than the **38.8%** pyannote baseline; leakage removal reduces that configuration to **36.2%**. The paper flags differing cpWER definitions concerning redundant output channels and supplies an appendix using MeetEval's penalized definition. [Tables 2, 3, 5 and Appendix A][P4].

**[INFERENCE]** Reusable pattern: jointly align activity and separation outputs, retain global identity stitching, and suppress inactive-lane leakage. Do not copy its meeting ASR gains into a live CPU or streaming-latency claim. This corrects an overly broad reading of the earlier research's “no candidate proven on meeting speech” caution: **offline meeting evidence exists; AudioGraph integration and latency evidence do not**.

## 3. Clustering families: what their controls really mean

Let **n** be the number of valid segment/track embeddings, **d** their dimension, **k** the final number of speakers, **s** a candidate/initial speaker count, **I** an iteration count, and **r** the number of affinity-pruning settings searched. Costs below exclude neural embedding extraction. Where not an implementation guarantee, they are explicitly **[INFERENCE: algorithmic cost model]**; dense versus sparse implementations matter.

| Family | Needs / count selection | Cost and memory | Unequal talk time and short segments | Overlap and temporal constraints |
|---|---|---|---|---|
| **AHC, average linkage** | Pairwise cosine/PLDA-derived dissimilarities and a merge threshold, or cut at k. Average linkage averages **all cross-cluster pair distances**. | Dense distances `O(n²d)`; optimized linkage `O(n²)` time and `O(n²)` memory. [SciPy][A1]. | No equal-size requirement. Average linkage is less dictated by one extreme pair than complete linkage, but contaminated embeddings/minimum-size absorption can still erase a rare speaker **[INFERENCE]**. | One label per embedding; no waveform separation. Preserve local activity tracks and add cannot-links or constrained reassignment. [V2]. |
| **AHC, complete linkage** | Same controls; distance is the **maximum** cross-cluster distance. | Optimized dense linkage `O(n²)` time/memory after distances. [A1]. | Compact clusters; a single noisy/short embedding can prevent a correct merge and fragment one speaker **[INFERENCE from max rule]**. | No native overlap model. Same track/constraint requirement. |
| **AHC, centroid linkage** | Threshold or k; distance between evolving centroids. This is distinct from average linkage. | SciPy's generic centroid linkage is `O(n³)` time and `O(n²)` memory; other implementations can differ. [A1]. | Means can be perturbed by low-quality tracks; filtering and reassignment are specifically studied in pyannote/DiariZen. [V2]. | Local overlap comes from segmentation, not the tree. Use Euclidean geometry where required; do not silently feed arbitrary cosine distances to Ward/centroid APIs that assume Euclidean input. [A1], [P5]. |
| **Spectral, including NeMo NME-SC** | Cosine affinity graph; plain spectral needs k/eigengap rule. **NME-SC jointly selects row-neighbor pruning p and k using normalized maximum eigengap**, then k-means on spectral coordinates. Practical implementations still bound count/search. [A2], [N2]. | Dense affinity `O(n²d)` and `O(n²)` storage; full dense eigensolve up to `O(n³)` per searched setting, hence `O(rn³)` naive search. Partial eigensolvers/subsampling reduce actual cost **[INFERENCE]**. | Graph structure can help non-spherical groups; weak/rare groups and unreliable affinity edges can distort eigengaps. Auto-k is not parameter-free truth. [A2]. Short-recording counting has special treatment in NeMo config. [N2]. | Basic graph clustering gives one label per input vector. Needs overlap-aware tracks or later neural overlap/resegmentation. |
| **VBx: Bayesian HMM + PLDA** | An ordered x-vector sequence, matching PLDA/transform, initialization (usually AHC), acoustic/regularization scales and state self-loop. Redundant speaker priors can shrink to zero; estimated count depends on initialization/prior, not a guaranteed discovery oracle. [V1], [V4]. | Following diagonalizing transform, dominant emissions/statistic updates approximately `O(Insd)`, plus HMM inference; generic dense forward-backward is `O(Ins²)`, while the structured transition can reduce it **[INFERENCE]**. Responsibility storage `O(ns)`; AHC initialization adds its cost. | Probabilistic pooling/prior can refine sparse speakers; domain/PLDA mismatch and overly strong smoothing can still merge them or miss short turns **[INFERENCE]**. [V4]. | Standard single-state VBx does **not** represent simultaneous speakers. Multi-stream VBx or local EEND plus constrained reassignment is needed. Temporal smoothing requires a meaningful ordered sequence. [V4], [V5]. |
| **GMM-VBx / cVBx for EEND-VC** | Matching PLDA; AHC over-initialization, regularization, quality filtering, then optional constrained reassignment. No HMM chronology assumption. [V2], [V3]. | Approximately `O(Insd)` iterative operations and `O(ns)` responsibilities after transforms, plus initial AHC and reassignment **[INFERENCE from implementation]**. | Explicit evidence for excluding short tracks from centroid training and re-inserting them afterwards; filtering too aggressively can exclude rare speakers entirely. [V2]. | Local EEND provides overlap. One-to-one reassignment preserves different local speakers; clustering alone still does not separate mixtures. [V2]. |
| **k-means / spherical / mini-batch / e-means** | **k supplied**; initialization and iteration budget. Spherical normalizes centroids for cosine; mini-batch/e-means change optimization, not count estimation. [E1], [E3]. | Exact assignments across I passes `O(Inkd)`; centroid/state `O(kd)` plus vectors/labels; batch scores `O(Bk)` if materialized **[INFERENCE]**. Optimized kernels/pruning change constants. | SSE is sample-weighted: many embeddings from one talker can dominate. It does **not** impose equal sizes, but can split a broad dominant speaker while losing a sparse speaker **[INFERENCE from objective]**. Short/mixed vectors remain noisy whichever optimizer is used. | No temporal/overlap constraints by default; add a separate assignment/constraint stage. |
| **UMAP → HDBSCAN, optionally post-AHC** | No k, but UMAP neighbors/dimension/metric/random seed and HDBSCAN minimum cluster size/min-samples/selection policy. Can label points as noise. WeSpeaker ships this pipeline. [W1], [W3]. | Low-dimensional tree-based HDBSCAN can be near `O(n log n)`; high-dimensional/generic distance work can approach quadratic. UMAP adds nearest-neighbor graph and optimization cost. No universal linear-time guarantee. [H1], [U1], **[INFERENCE: complexity characterization]**. | Useful noise rejection, but a genuine brief speaker may look like sparse noise. UMAP can distort density and create false tears; never assume its 2-D visualization preserves speaker geometry. [U1]. | No direct overlap model; reassigning noise must respect cannot-links and allow unknown. WeSpeaker's code explicitly postprocesses HDBSCAN labels. [W3]. |
| **Constrained clustering / assignment** | Must-link and cannot-link evidence in addition to the base algorithm's k/threshold. Local distinct speaker slots supply cannot-links; human-confirmed same-person decisions can supply must-links. [C1], [V2]; **[INFERENCE: user evidence integration]**. | Depends on base method. Rectangular local/global assignment is small; a padded Hungarian solve is `O(max(local,global)³)`. Current greedy sorts at most local×global scores. **[INFERENCE]**. | Protects brief simultaneous speakers from collapse, but erroneous constraints can force false splits/merges. Exclude inactive local slots from assignment. [V2]. | Different speakers within a local window are constrained even if they do not overlap at every frame; simultaneous same-source speech is a particularly strong cannot-link. Echo across sources is not proof of different people **[INFERENCE]**. |
| **Online/incremental (diart/Coria)** | Activity threshold, new-speaker distance threshold, minimum duration for updates; k grows as needed. [C1]. | For L active local speakers and s registered speakers: scoring `O(Lsd)`, centroids `O(sd)`; assignment as above, plus bounded rolling neural context **[INFERENCE]**. | Duration/overlap-aware updates limit contamination; early mistakes and long-term changes can persist. More observed speech can improve centroids, but short backchannels remain hard. [C1]. | Overlap-aware segmentation + weighted embedding pooling + cannot-links. No waveform separation required. |
| **Two-stage over-cluster → merge/refine; under-cluster → split** | Conservative seed groups, then threshold/PLDA/neural resegmentation; multiple stages have multiple controls. AHC→VBx, UMAP/HDBSCAN→post-AHC, and 3D-Speaker cosine merging are concrete examples. [V2], [W3], [D2]. | Sum of stages. If m reliable seed centroids replace n vectors for a later dense stage, its matrix becomes `O(m²)`, but building/refining seeds still costs work **[INFERENCE]**. | Over-clustering can preserve rare speakers until evidence accumulates; blindly absorbing all small groups destroys that benefit. An **under-clustered** mixture cannot be repaired by another merge-only pass: revisit embeddings/audio and split or resegment **[INFERENCE]**. | Keep cannot-link constraints during merge and retain multi-speaker activity during resegmentation; TS-VAD/MSDD can refine overlap given seed profiles. [I2], [N1]. |
| **Genie** | MST + Gini threshold, and k or an external cut rule. The Gini threshold is **not** a distance cutoff or automatic count estimator. [G1], [G2]. | Generic MST `O(n²)` distance evaluations (each generally `O(d)`); Genie after MST `O(n√n)` time, `O(n)` memory. Fast low-dimensional Euclidean MST is a special path. [G1], [G2]. | Designed to avoid single-linkage's undesirable imbalance, but may absorb a *legitimate* tiny speaker group. Tune against unequal-speaking-time fixtures, not balanced blobs only **[INFERENCE]**. | No native diarization, time, or overlap logic; would need the same local-track reassignment/constraints. |

### 3.1 Robust patterns are more portable than a clusterer's name

- **Separate centroid training from final labeling.** Fit on enough clean speech, then attach short segments by constrained evidence. Do not drop their timeline speech merely because they were excluded from fitting. This is the explicit filter/cluster/reassign pattern in [V2] and `filter_embeddings` in [P5].
- **Protect rare-speaker recall.** Minimum cluster size and Gini guards operate on **embedding counts**, not a semantic judgment that a short speaker is an outlier. A person with one brief intervention is still a person. **[INFERENCE from G1, W3, D2]**.
- **Keep uncertainty visible.** Noise/low-score tracks may remain unknown; count diagnostics should not force them to the nearest named person. **[INFERENCE]** This connects HDBSCAN's abstention capability [U1] with the lazy-ID design's existing unknown/suggestion states.
- **Use chronology only when valid.** A concatenation of scattered clean frames selected for one local track is not a chronological single-speaker x-vector sequence. That is why [V2] uses GMM-VBx and why current pyannote [V3] removes HMM support.
- **Distinguish constraints from identities.** A cannot-link protects two local speaker tracks from merging. It does not name either; must-links from a wrong self-introduction can poison a whole session. **[INFERENCE]** Keep language evidence in the identity binder rather than making an LLM reshape acoustic clusters directly.

## 4. e-means and SuperEMeans: real advance, different bottleneck

### Objective and update

Bessa et al.'s November 2026 paper optimizes the ordinary k-means mean squared quantization error:

\[
\min_{c_1,\ldots,c_k}\frac{1}{n}\sum_i\min_j\|x_i-c_j\|_2^2.
\]

It is a stochastic optimizer for this non-convex objective, **not an exact global optimum solver** and not a new semantic notion of a speaker. e-means maintains exponentially averaged cluster sums and counts, `S ← (1−α)S + α·batch_sum`, `N ← (1−α)N + α·batch_count`, with centroid `S/N`; fitted scaling laws select batch/step sizes from n, k and budget. Initialization, dead-cluster revival, cosine scheduling and finalization also matter. Its EMA is used to optimize a **static, revisited dataset**; this is not the same contract as AudioGraph's open-ended, once-arriving live registry. [paper][E1], [author explanation][E2], [versioned guide][E3].

Faiss **1.16.0** includes it in `faiss.contrib.e_means`. The CPU NumPy path calls Faiss nearest-centroid primitives; GPU training uses PyTorch/Triton/CuTe rather than implying a standalone Rust-callable C++ e-means class is included merely because Faiss is a C++ library. The guide supplies `spherical=True`, so cosine-oriented experiments need not pretend Euclidean input normalization alone implements spherical updates. Both still require k. [release][E4], [guide][E3].

For normalized data and normalized centers, squared Euclidean distance is `2(1−cosine)`. **[INFERENCE: algebra]** Ordinary k-means centers are generally not unit length, so “normalize input, then any Euclidean k-means is identical to spherical k-means” is false. Record center normalization and final assignment conventions in a benchmark.

### Scale and relevance

Published evidence ranges from author visualizations at **100,000 points** through large embedding experiments and a **10 billion-vector / 16 million-cluster** example. The paper reports matching Lloyd's 25-epoch loss in **1.4–2.6 epochs** and **10–45× GPU** speedups on its tested configurations. These are k-means loss/runtime results, **not diarization DER or speaker-count accuracy**. [E1], [E2].

**SuperEMeans** combines e-means' fewer epochs with SuperKMeans' dimension-pruning assignment acceleration. The maintainers publish a MIT C++/Python library. Its November 17 benchmark uses **5 million 1,536-D OpenAI** and **10 million 1,024-D Cohere** vectors, with roughly **20,000/40,000 centroids**. The reported gain is up to **1.8×** over their e-means at two epochs and roughly **3×** over the authors' e-means implementation at five; these are different comparisons, not one universal “3× faster” constant. [maintainer benchmark][E5], [library][E6].

**[INFERENCE: AudioGraph scale analysis]** Hundreds to roughly 10,000 embeddings per session and a handful of actual speakers have a small n×k relative to index-building workloads. At n=10,000, d=192, k=8, an exact assignment pass evaluates about **15.36 million coordinate-distance terms**, while the raw float32 embedding matrix is **7.68 MB**. These are arithmetic estimates, not measured time. By contrast, a dense n×n float32 affinity matrix is **400 MB** before eigensolver workspace. Thus dense auto-k and embedding extraction may be more consequential than speeding up a small fixed-k assignment loop; profile the complete pipeline rather than applying the headline speedup.

Other limitations for speaker discovery:

- k must come from a roster-derived hypothesis, count estimator, or candidate search. Invitees are not a guaranteed count of actual audible voices. [E3]; **[INFERENCE: roster caution]**.
- Lower quantization MSE need not preserve a rare speaker or improve temporal speaker confusion; e-means' dead-cluster revival can replace low-use centroids by splitting larger clusters. Its objective and revival were not designed to protect a brief meeting participant. [E2], [E3]; **[INFERENCE: diarization risk]**.
- Returned training labels are last-pass labels and are not necessarily nearest-final-center labels; the guide explicitly recommends `assign` when exact final assignments are needed. Count fair compute budgets and include the final assignment in downstream speaker evaluation. [E3].
- “Streaming batches from SSD” means feeding a known static dataset across epochs. It does not discover new live speakers automatically. [E3].

**Recommendation [INFERENCE]:** no production adoption now. Reconsider if fixed-k clustering is a measured bottleneck in large post-session archives, embedding-model training/data curation, or opt-in library indexing at much larger scale. For session-level comparison, use spherical mode, document how k was obtained, compare against a small CPU baseline, retain rare-speaker tests, and separate oracle-k from estimated-k results. SuperEMeans offers a potential C++ FFI route if that later need is demonstrated; an ONNX graph is unnecessary for the clustering arithmetic. Faiss and SuperKMeans are MIT [E6], [E7], but packaging and backend costs remain.

## 5. Genie / genieclust: mechanism, fit, and implementation boundary

### Algorithm

Genie starts with singleton clusters and a minimum spanning tree of the pairwise-distance graph. Like single linkage, it normally consumes increasing-weight MST edges. When the **Gini index of the current cluster-size distribution exceeds `gini_threshold`**, it enforces a merge involving a smallest cluster instead of allowing arbitrary continued growth of a large one. A threshold of **1** disables the correction; smaller thresholds penalize small clusters more strongly. This is an inequality guard, not speaker verification, not an outlier probability and not hard equal-sized clustering. [author R manual][G1], [Python API][G2], [original paper][G5].

**Does it estimate k?** The standard Python API takes `n_clusters`; R `genie(..., k=...)` produces the requested partition and `gclust` produces a hierarchy to cut. The phrase “any number of clusters” means arbitrary **requested** counts, not automatic speaker discovery. The experimental GIc API also exposes `n_clusters`; its name is not proof of an automatic-k interface. An external cut/count estimator is additional design and must be evaluated separately. [G1], [G2].

### Complexity and cosine

The author documents `O(n√n)` clustering time and `O(n)` memory **after the MST exists**. Generic/non-Euclidean MST construction is `O(n²)` time in distance evaluations; an individual dense-vector distance also depends on d. Modern `quitefastmst` accelerates low-dimensional Euclidean input, but that does not establish the same scaling for 192-D cosine speaker vectors. Supplying a full distance matrix consumes `O(n²)` storage even if the subsequent tree operations do not. [G1], [G2]; **[INFERENCE: distance/storage qualification]**.

Cosine is directly exposed by the R interface. For L2-normalized vectors, Euclidean and cosine distances have the same ordering (`L2² = 2−2cos`), so an exact MST-based implementation can exploit normalized Euclidean geometry subject to tie handling. **[INFERENCE: mathematical equivalence for MST edge order, not a guarantee of floating-point-identical library output]**. Preprocessing, zero vectors, approximate MSTs and version-specific APIs must be recorded. [G1], [G2].

### Speaker use and adoption

The upstream project is a general-purpose clustering package with general clustering benchmarks, not a speaker-diarization recipe. Focused searches for `genieclust speaker diarization` and `Genie speaker diarization` on this research date did **not identify a primary-source production recipe or controlled speaker benchmark using it**. This is a bounded negative search finding, not proof nobody has tried it. Embedding compatibility is plausible because the API accepts vector/distance data and cosine; published success on generic vectors would still not establish overlap handling or unequal-talk-time robustness. [upstream][G3], [G1], **[INFERENCE: domain-transfer caution]**.

The current repo supplies Python/R wrappers and a **header-only C++ core**, but no official Rust binding was identified in its documented interfaces. More importantly, upstream `LICENSE` explicitly says **GNU Affero GPL Version 3**. A C++ or Rust FFI boundary does not by itself change that license. [G3], [G4].

**Recommendation [INFERENCE]:** include Genie as a research-only counterexample to AHC/spectral on the sibling benchmark, using both balanced and very unequal speaker frequencies and reporting which k was provided. Do not package current genieclust in AudioGraph without license review. If it materially improves *real embedding* speaker confusion/count behavior, consider an independent implementation from [the paper][G5] and mathematical specification, with provenance and legal review; do not copy or mechanically translate the AGPL implementation. Algorithmic simplicity makes that technically plausible, not automatically legally cleared. Its Gini correction is exactly the aspect that requires ablation on brief participants before adoption.

## 6. End-to-end and hybrid diarizers: where clustering moves, not just disappears

| System / reusable pattern | Clustering or alternative mechanism | What to reuse for AudioGraph; deployment qualification |
|---|---|---|
| **EEND-EDA** | Frame embeddings plus an encoder/decoder generate attractors until an existence stopping condition; frame/attractor dot products yield multi-label activity. No external vector clustering is required within that inference. Training-count generalization remains a limitation; the authors also propose iterative inference. | Pattern: direct overlap-aware activity and learned count, not a voiceprint store. Long-form/global identity still needs a deliberate strategy. Foundational 2020–2022 algorithm used by later hybrids; checkpoint/export/license must be chosen separately. [paper][Q1]. |
| **EEND-vector-clustering / EEND-VC** | Local EEND predicts overlapping activities; cluster per-track vectors across chunks, then aggregate global activity. Constrained assignment prevents distinct local tracks collapsing. MS-VBx extends VBx to multiple streams with same-chunk constraints. | Closest conceptual match to the existing worker. Reuse clean-track filtering, global reassignment and activity reconstruction rather than assuming every end-to-end local segmenter removes the need for clustering. [V2], [V5]. |
| **Sortformer** | Arrival-time ordering/Sort Loss resolves output permutation; directly emits speaker activity, not AHC/spectral clusters. | Capped output slots; no named-person recognition. Offline v1 weights are CC-BY-NC-4.0, unlike later streaming checkpoints; do not transfer licenses between versions. [NeMo architecture][N1], [v1 card][N3]. |
| **Streaming Sortformer v2/v2.1** | Chunked model plus **Arrival-Order Speaker Cache (AOSC)** and recent FIFO context maintain session speaker slots without an external embedding-clustering pass. | Native streaming pattern; four-speaker cap. v2 is CC-BY-4.0; v2.1 has the NVIDIA Open Model License. The runtime path already explored by AudioGraph should be compared with newer models, not treated as unbounded. [N1], [v2 card][N4], [v2.1 card][N5]. |
| **Nemotron-3-Diarization** | Sortformer arrival ordering plus AOSC/FIFO; up to **eight** speakers, streaming and offline-style configurations. | Existing Seed `audio-graph-e9bd` owns the spike. Card says OpenMDW-1.1, commercial/non-commercial use, and local NeMo-Speech.cpp support. Recommended buffer configurations are 0.32/0.64/1.04 s; these **exclude compute time**. Native C SDK/ggml/GGUF is a Rust FFI candidate, not an already verified AudioGraph backend. Session slots still need a naming layer. [model card][N6], [runtime][N7]. |
| **NeMo clustering + MSDD** | VAD → multiscale TitaNet vectors → clustering initialization → neural Multi-Scale Diarization Decoder comparing local vectors to cluster-average profiles, with learned scale weighting and per-speaker probabilities. | Pattern: use longer context for reliable speaker identity and shorter scales for precise turns/backchannels. MSDD refines seed clusters; it does not make their initialization unnecessary. NeMo toolkit Apache-2.0; each VAD/embedding/decoder checkpoint has separate terms. Python reference; a complete Rust/ONNX pipeline is not verified. [N1], [N2], [NeMo repo][N8]. |
| **DiariZen** | WavLM + Conformer powerset local segmentation, WeSpeaker embeddings, quality filtering and VBx/global reconstruction in its recent configuration. | Strong offline research comparator; do not ship published weights: **code MIT, weights CC-BY-NC-4.0**. Reuse modular pattern and evaluate shippable components independently. [authors' repo][Z1], [V2]. |
| **3D-Speaker** | VAD/segmentation → CAM++/other embeddings → spectral clustering by default; alternatives include average-link AHC and UMAP/HDBSCAN. Current wrapper uses AHC for small n, absorbs minor clusters and optionally merges similar centroids. Optional pyannote overlap refinement is separate. | Existing sherpa-compatible embedding candidates and useful short-recording/multistage comparisons. Do not copy unconditional small-cluster absorption as if it were rare-speaker-safe. Toolkit Apache-2.0; per-checkpoint and optional gated pyannote terms remain. Face/video identity fusion exists but is outside this product design's non-goals. [recipe][D1], [code][D2], [config][D3], [toolkit][D4]. |
| **WeSpeaker** | Silero VAD → sliding-window ResNet embeddings → spectral clustering, or cosine UMAP → HDBSCAN → **post-AHC** for noise/short groups. It is more than an embedding-model zoo. | Research comparator and source of actual ONNX embedding runtime/recipes, cosine/PLDA/AS-norm and calibration tooling. Apache-2.0 code does not settle each checkpoint's rights. UMAP/HDBSCAN adds geometry/parameter dependencies. [W1], [tutorial][W2], [clusterer][W3]. |
| **WhisperX** (2023–2026) | Whisper/faster-whisper ASR + VAD/phoneme forced alignment, plus **a separate pyannote diarization pipeline**, followed by assigning word speakers. Clustering belongs to pyannote; alignment is not speaker discovery. | Reuse independent word-timing / speaker-timeline reconciliation. It neither names people nor recovers the second overlapped transcript merely by enabling diarization. Current repo code BSD-2-Clause; alignment/ASR/pyannote model terms separate. [repo][X1]. |
| **SpeechBrain recipes** | AMI recipe: ECAPA/x-vectors + spectral clustering, with oracle or estimated count. Recognition recipes support embedding scoring, including PLDA. | Good controlled baselines and training recipes, not a complete low-latency Rust service. Its AMI table uses **0.25 s collar and ignores overlap**; not comparable directly with no-collar/overlap-scored pyannote DER. Toolkit Apache-2.0, model card terms separate. [AMI recipe][B1], [toolkit][B2]. |
| **Picovoice Falcon** | Proprietary on-device diarization engine; internal clusterer/count algorithm is not disclosed in cited public docs. | File/buffer SDK and C API could support offline evaluation. SDK repository Apache-2.0 does **not** make the proprietary engine/model unrestricted. AccessKey licensing/account limits and internet validation are required despite local audio inference. Not a default no-account local-first dependency. [Falcon docs][F1], [repo][F3]. |
| **Picovoice Eagle** | Enroll voiceprints into profiles; compare incoming frames against enrolled profiles in real time. This is speaker recognition, not anonymous speaker-count discovery. | Reuse the enrollment/recognition split as a pattern, not necessarily the SDK. C FFI possible; AccessKey and license-server validation apply. No official Rust binding was identified in the documented SDK list. [Eagle docs][F2], [repo][F4]. |
| **sherpa-onnx** | Local segmentation + embedding + FastClustering for offline diarization; separate embedding-manager enrollment/search/verify APIs. Rolling live behavior is AudioGraph's worker around that offline engine. | Lowest-integration-cost baseline: Rust/C/C++ and ONNX already fit. The toolkit is Apache-2.0, while each model has separate terms; do not infer TitaNet-small's terms from the toolkit. No need to replace runtime merely to add cosine verification. [diarization docs][S1], [repo][S2], [prior verified Rust APIs](speaker-identification-2026-11-21.md#f-sherpa-onnx-apis-available-to-this-repo). |

**[INFERENCE]** An end-to-end streaming model can replace the *live diarization engine* without replacing the *identity binder*. Arrival-order speaker 2 is still not “Priya,” and resetting a stream/cache needs a new anonymous-slot correspondence rather than silently reusing person identity. Keep the unbounded clustering path for sessions exceeding an end-to-end model's cap.

## 7. Identification patterns: score evidence, not cluster indices

### 7.1 Enrollment, scoring, normalization, calibration

**Enrollment → verification → open-set identification** is the reusable split. Eagle explicitly models enrollment and recognition as separate steps; sherpa exposes embedding-manager add/search/verify. The lazy-ID design already supplies consented, local, model-pinned storage and late binding; a different clusterer is not a prerequisite. [F4], [S2], [design](../designs/lazy-speaker-identification.md#34-cross-session-voiceprint-store-opt-in-phase-3).

| Mechanism | What it needs / produces | Recommendation [INFERENCE] |
|---|---|---|
| **Cosine** | L2-normalized same-model enrollment/test vectors; a similarity score, not probability. Angular-margin speaker models commonly use it. | First baseline for current sherpa embeddings: transparent, cheap `O(candidate_count × d)`, no learned backend. Calibrate separately from the live clustering threshold. [I1], [W1]. |
| **PLDA scoring** | Trained between/within-speaker distributions, appropriate transforms and domain coverage; likelihood-ratio-style same/different evidence. | Useful when validated, and required by the chosen VBx backend; not automatically better for naming. In the cited 2023 verification study, cosine beats the tested PLDA configuration. Do not reuse its exact scores across models. [V4], [I1]. |
| **AS-norm** | Score enrollment and test against selected **impostor cohort** representations, normalize each side and combine. Cohort choice/size and model space matter. | Evaluate only after raw cosine baseline. A tiny personal voiceprint library is not automatically a representative impostor cohort; storing unrelated named voices to create one is not justified by this research. Extra cohort work/privacy/license cost needs measurement. [I1], [WeSpeaker normalization tooling][W1]. |
| **Calibration** | Labeled target/non-target trials separate from evaluation; optionally duration/quality/channel features, affine/logistic score mapping and a chosen operating prior/cost. | Report false accepts, false rejects and unknown coverage at a fixed risk, not EER alone. Normalization is not calibration; similarity softmax is not automatically a posterior. [I1], **[INFERENCE: evaluation contract]**. |

The 2023 verification study explicitly uses enrollment/test duration, embedding magnitude and cohort statistics as quality measures; its calibration improves some metrics but is omitted for one challenge because minDCF deteriorated. This is a concrete warning against assuming a more elaborate backend always helps. Its dataset/EER improvements are not an AudioGraph false-bind guarantee. [Study §§2.3, 3.2, 4.3][I1].

### 7.2 Closed roster, open-set rejection, drift

- **Closed candidate roster, open-set decision.** Compare against invitees/self/consented people, but include **unknown/not present**. A pure argmax must name an outsider as someone; a roster is only a prior. Zoom and Teams restrict candidate matching to invitees, and pyannoteAI warns a supplied voiceprint can match even when the person is absent. [Zoom official conditions][I3], [Teams][I4], [voiceprint caveat][I5].
- **[INFERENCE]** Require an independently calibrated absolute acceptance threshold, sufficient clean speech, and separation from the runner-up. Tune per model pin and relevant duration/channel conditions; test unseen impostors, similar voices and roster-size changes. Pairwise verification FAR does not equal false-identification risk after searching many candidates.
- **[INFERENCE]** Keep a small set of consented, confirmed exemplars for different microphones/sessions rather than continuously auto-updating a named profile from the system's own guesses. Otherwise one false bind can contaminate the library and reinforce future false binds. Preserve model digest/dimension and feature preprocessing; on model changes re-enroll or explicitly migrate with evidence, never compare unlike spaces. The model-specific-identifier warning is explicit in [Speechmatics][I6]; the bounded exemplar/local purge policy already lives in [the design](../designs/lazy-speaker-identification.md#34-cross-session-voiceprint-store-opt-in-phase-3).
- **[INFERENCE]** Treat illness, codec, room and microphone changes as possible score drift, not evidence a new person has appeared. Monitor false-bind/unknown rates and request fresh consented enrollment when matching degrades; do not silently lower a threshold to maintain coverage. Quality-aware scoring evidence in [I1] motivates evaluation, not a universal drift detector.

### 7.3 TS-VAD and personal VAD

**TS-VAD** takes target speaker profiles—often obtained from an initial clustering pass—and estimates each target's activity over time, including overlap. Original fixed-profile-count architectures have limits; transformer speaker-axis designs support variable profile sets and order invariance. It can refine *where* known session speakers talk, but cannot discover a new human name from acoustics. Profiles can be session-anonymous, so TS-VAD need not imply biometric enrollment. [TS-VAD paper][I2].

**Personal VAD** asks whether one enrolled target is speaking rather than whether anyone is speaking. The 2023 SVVAD work uses speaker-conditioned feature weighting to improve verification in multi-speaker conditions; this is not the same as producing separated audio or diarizing all unknown participants. [SVVAD][I7].

**[INFERENCE]** TS-VAD is a second-tier offline-refinement candidate after reliable seed clusters exist, not the first live-worker replacement. Personal VAD could later strengthen an opt-in “me” signal or enrollment-quality check; do not discard non-self speech from meeting ASR. Neither approach removes the unknown-person path, and each new checkpoint/runtime needs its own license and native/export spike.

### 7.4 Text names and platform metadata

These patterns are already designed in the [lazy-ID binder](../designs/lazy-speaker-identification.md#4-evidence-sources-and-fusion); the additions here are boundaries:

- Content-based name inference is real: AssemblyAI documents inference from conversation content without enrollment. **[INFERENCE]** It supplies a candidate/evidence item, not proof of acoustic sameness; distinguish self-introduction, addressee and someone absent being mentioned. A constrained LLM decision must abstain and never override a user-confirmed binding. [official feature][I8].
- Platform attribution is often stronger than a voice similarity: Zoom RTMS exposes per-participant identity/audio; Granola documents reading participant names and active-speaker UI through Accessibility or its Meet extension. These are different capture surfaces, not evidence every desktop app exposes the same fields. [RTMS][I9], [Granola][I10].
- **[INFERENCE]** Fuse active-speaker intervals with acoustic activity over time, accounting for UI lag, layout changes, shared-room devices, echo and multiple simultaneous speakers. One active-speaker badge is not a timestamp-perfect label for every audio frame. Preserve evidence provenance and use capability probes on Windows/macOS/Linux rather than a platform-specific default. Existing Seed `audio-graph-918c` owns this work; no second roster integration is proposed.

## 8. Separation for overlap: useful offline, not a free improvement

Three arrangements are documented in [PixIT's primary paper][P4]: **separation → diarization**, **diarization → guided separation**, and **joint local separation/activity training → global clustering**. Each cascaded arrangement can amplify its first stage's errors; joint alignment removes some permutation/leakage ambiguity, not all artifacts.

| Situation | Why it may help | Why it may hurt / costs | AudioGraph decision [INFERENCE] |
|---|---|---|---|
| Genuine overlapping voices in a room microphone | Recover quiet/interrupted words that mono ASR deletes; source-specific ASR also supplies attribution. PixIT measures this on real meetings. [P4]. | Leakage duplicates words; suppression removes quiet speech; generated artifacts alter phonetics; wrong stitching swaps identities. The paper's leakage ablation demonstrates harm can exceed benefit. [P4]. | Offline oracle/bakeoff under existing `dd19` and `c237`; report cpWER/tcpWER where appropriate, overall WER, deletion/insertion/duplicate words and speaker consistency, not SI-SDR alone. |
| Long mostly single-speaker stretches | Little overlap to recover. | Running full separation and per-lane ASR adds unnecessary compute and silent-lane hallucination opportunities **[INFERENCE]**. | Prefer original mono ASR; evaluate overlap-triggered processing with context, not unconditional all-session live separation. |
| Speaker embedding purity | Overlap-free estimates could help when no clean speech exists. | Separated timbre/artifacts can bias verification, especially against enrollment from original audio **[INFERENCE]**. | Exclude overlap first (`faaa`); test separation-derived identity evidence only after `dd19` results. Do not train named profiles on generated lanes automatically. |
| True per-participant capture channels available | Already have source/identity evidence. [I9]. | Mixing then re-separating throws away that information **[INFERENCE]**. | Preserve source-native lanes/provenance; do not substitute a separator. |
| PixIT-style full-session output | Global clustering stitches local slots through pauses; activity masks suppress leakage. [P4], [P6]. | 90%-overlap neural windows, WavLM/separator inference, source storage and multiple ASR passes; exact desktop CPU/RTF remains unmeasured here. | Research oracle, not a single exported model promised to ship. |

**[INFERENCE: memory arithmetic]** One hour of 16 kHz float32 mono is about **230.4 MB**; four full-length separated lanes alone are about **921.6 MB**, excluding original PCM, model weights and intermediate tensors. Processing in chunks can reduce working memory, but changes stitching/caching design and must preserve source-time mapping. These are size estimates, not observed RSS.

The lazy-ID design explicitly excludes separated PCM as ASR input. This report **does not silently widen it**: PixIT's ASR evidence belongs in the existing separation bakeoff, where a future policy change would need explicit design approval, quality evidence, generated-artifact provenance and provider capability/egress review. Until then separation can only inform the existing optional offline identity/refinement experiment, not live routing. [design §7](../designs/lazy-speaker-identification.md#7-separation-when-to-run-it-and-when-to-skip-it), [existing decision](source-separation-bakeoff-2026-06-26.md).

## 9. Pattern-to-AudioGraph synthesis

Expected gains below are **[INFERENCE: qualitative hypotheses]**, not promised DER, latency or identity accuracy. License entries summarize cited upstream terms, not a release approval.

| Pattern | Exact AudioGraph seam | License / shippability | Rust / ONNX path | Expected gain | Main risk / gate |
|---|---|---|---|---|---|
| **Overlap-aware clean embedding selection + duration-gated updates** | Live `worker.rs` → `stabilize.rs`; identity snapshots, existing `faaa` | Existing sherpa Apache-2.0; model terms still separate. [S2]. | Rust range selection + existing ONNX extractor; no new inference runtime. | Less centroid pollution and fewer false identities. | Can starve brief speakers of clean evidence; preserve their spans/unknown state. [C1], [V2]. |
| **One-to-one assignment; greedy vs Hungarian** | `stabilize.rs`, separately offline local/global reassignment | Coria pattern; diart MIT; choose independently licensed/native assignment code. [C1], [C2]. | Small Rust numeric routine, no ONNX. | Avoid locally distinct speakers collapsing; compare total-score assignment to greedy. | No assumption Hungarian automatically improves DER; inactive slots and threshold/dummy-new semantics matter. [V2]. |
| **Quality-filter → AHC → constrained reassignment** | Offline `clustering.rs` experiment; `4379` emits revisioned Merge/Split proposals | Shippable algorithm; library license and neural models reviewed separately. [A1], [P5]. | Rust AHC/assignment or current sherpa baseline; ONNX embeddings reused. | Better count/rare-speaker behavior without full neural replacement. | Thresholds and minimum-size absorption need meeting fixtures, not per-test tuning. [V2]. |
| **NME-SC auto-k, bounded/sparse when justified** | Alternative offline clustering under `4379`, not rolling live re-eigensolve | NeMo code Apache-2.0; reproduce algorithm with attribution. [A2], [N8]. | Rust linear algebra/partial eigensolver candidate; no new neural model. | A count estimator and non-centroid graph partitioning. | Dense quadratic memory/eigensolve costs; weak eigengap on short/imbalanced sessions. |
| **PLDA + GMM-VBx + constraints** | Offline refinement `4379`; not an HMM pasted onto unordered live centroids | pyannote pipeline MIT, VBx utility Apache-2.0; matching transforms/PLDA weights have separate terms. [P5], [V3]. | Rust matrix arithmetic + existing embedding ONNX; backend training/artifact pinning required. | Probabilistic refinement/count pruning grounded in diarization research. | Current 192-D TitaNet-small has no compatible PLDA artifact established here. [V2]. |
| **UMAP/HDBSCAN + explicit noise reattachment** | Offline research comparator for `4379` | WeSpeaker Apache-2.0; audit transitive libs before shipment. [W1], [W3]. | Python reference first; no verified complete Rust-equivalent pipeline in this report. | Outlier rejection, automatic candidate count. | UMAP geometry/seed sensitivity, rare-speaker noise and silent forced absorption. [U1]. |
| **e-means / SuperEMeans** | Offline fixed-k experiment only; later large-library/index tooling if profiled | Faiss/SuperKMeans MIT. [E6], [E7]. | Faiss contrib reference is Python/NumPy/PyTorch; SuperKMeans C++ FFI candidate; no ONNX needed. | Faster fixed-k quantization at genuinely large scale. | No auto-k/overlap/identity improvement supplied; packaging can outweigh session-scale benefit. [E1], [E3]. |
| **Genie MST + Gini guard** | Sibling benchmark; possible independent offline algorithm experiment | **Current genieclust AGPL-3.0: research-only unless license review passes**. [G4]. | Header-only C++ exists; no official Rust wrapper found. Paper-based independent Rust implementation is only a reviewed option. | Test whether hierarchy resists embedding outliers. | Guard can absorb true brief speakers; external k and exact-MST cost remain. [G1]. |
| **Streaming arrival-order model + persistent cache** | Alternative live backend, existing `e9bd`; identity binder stays separate | Nemotron OpenMDW-1.1; NeMo-Speech.cpp Apache-2.0. [N6], [N7]. | C SDK/ggml/GGUF FFI; do not promise unverified ONNX/native integration. | Native streaming and overlap activity, avoiding full reclustering per window. | Eight-speaker cap, cache resets, CPU latency/RTF and three-OS packaging. |
| **TS-VAD / MSDD refinement from session profiles** | Later `4379` research after robust seed clusters | Algorithm is not a checkpoint license; choose/pin permissive weights. [I2], [N1]. | Separate export/native feasibility experiment; none established here. | Finer short-turn/overlap boundaries given good profiles. | Wrong seed profiles propagate; extra model and training-domain mismatch. |
| **Cosine enrollment + calibrated open-set rejection** | `identity::binder` / voiceprints in lazy-ID design; existing `437e`, `c44d` | Existing sherpa embedding-manager path; enrollment consent/model terms still mandatory. [S2]. | Existing Rust APIs or normalized dot products; optional numeric calibration. | Names without replacing the diarizer; controlled unknown outcome. | False binds, stale models, cross-session drift and circular self-training. [I1], [I6]. |
| **Source/roster/active-speaker/text evidence with provenance** | Lazy-ID binder; `9ae7`, `918c`, `636a` | Platform APIs/permissions/terms reviewed per surface; no new neural checkpoint for metadata. [I9], [I10]. | Backend adapters per OS + existing decision contract. | Reduce biometric search and improve naming coverage. | Shared-room devices, lag, absent-name mentions, incorrect confidence fusion. |
| **PixIT-style aligned separation + leakage mask** | Offline oracle under existing `dd19`/`c237`; evidence-only refinement in `4379` | MIT gated pipeline/core model, separate ECAPA/WavLM dependency review. [P7], [P8]. | Python oracle first; complete Rust/ONNX export not established. | Recover overlap words offline; more defensible separation than synthetic-mixture results alone. | CPU/memory/ASR multiplier, leakage and generated-lane provenance; no live ASR cutover. [P4]. |
| **WhisperX-style timing/attribution separation** | Speaker timeline join / post-session word alignment, not identity storage | BSD-2-Clause code plus model licenses. [X1]. | Reuse available backend word times; alignment model integration evaluated separately. | More precise speaker attribution at word boundaries. | Forced alignment cannot recover words absent from original ASR or infer human names. |

### Recommended adoption order [INFERENCE]

1. **Finish the existing foundations, do not create replacements for their Seeds:** release neural-engine availability (`8848`), basis anchors (`ca06`), correct label consumption (`1586`), source-local timelines (`9ae7`), overlap-exclusive identity embeddings (`faaa`), lazy binding/relabel semantics and fixtures (`437e`, `c44d`). These remain prerequisites regardless of clustering algorithm.
2. **Live:** retain the small incremental registry; independently test assignment quality/clean-speech admission and the already-tracked Nemotron spike (`e9bd`). Do not add session-wide e-means, Genie or repeated dense spectral solves to the hop loop.
3. **Offline:** extend `4379` with a controlled clusterer/quality-policy experiment. Begin with unchanged segmentation/embeddings and quality-filtered AHC, then NME-SC and cVBx only when its matching PLDA exists. Report both oracle-k and unknown-k, per-speaker recall for brief talkers, speaker-count error, confusion, DER with overlap, latency and memory. Preserve user-confirmed names and revise the timeline rather than rewriting source ASR.
4. **Naming:** separately calibrate cosine open-set matching and fusion thresholds using cross-session/channel-disjoint trials. AS-norm/PLDA are candidates, not mandatory complexity. Keep manual confirmation authoritative and opt-in voiceprints deletable as the existing design requires.
5. **Separation:** use the real-meeting PixIT result to improve the existing `dd19` experiment's evidence, not to bypass `c237` baseline/quality gates. No new separation ticket is needed.

## 10. Evidence limits and backlog deduplication

- No model was downloaded behind a gate, no terms were accepted, no private meeting audio was used, and no model-backed inference, timing, code tests, builds, formatters or linters were run for this research slice. The sibling benchmark owns its empirical results; this document contains published measurements and explicitly marked arithmetic estimates only.
- Sources were read on the research date. Several URLs track upstream `main`/`master`; an implementation experiment must pin commits/models/configs. Faiss's e-means guide/license are linked at **v1.16.0**. Future upstream changes are not evidence for the release described here.
- Hosted Precision-2 internals and Picovoice clustering internals are not disclosed by the cited public documentation. No internal algorithm is guessed.
- A native/ONNX path for an individual embedding model does not prove an entire diarization/separation pipeline exports. No claim of Rust parity or desktop CPU speed is made for untested pipelines.
- Current TitaNet-small checkpoint redistribution terms remain unresolved in the previous research; do not borrow the license of TitaNet-Large, NeMo code, or a different sherpa release asset.
- Genie prior-speaker-use and official-Rust-binding findings are bounded searches. The license claim is grounded directly in upstream `LICENSE`, not GitHub's generic “Other” summary.
- `.seeds/issues.jsonl` was inspected read-only. Existing work includes `audio-graph-e9bd` (Nemotron spike), `faaa` (exclusive embeddings/snapshots), `4379` (offline refinement), `437e` (encrypted voiceprints), `c44d` (identity fixtures/metrics), `918c` (platform roster), `dd19`/`c237` (separation). **Do not duplicate those.** The final handoff proposes only narrow additions: constrained assignment/centroid-quality comparisons and explicit cross-session open-set scoring calibration; offline pipeline/clusterer and separation findings should instead extend their existing Seeds.

## Primary-source index

### pyannote, PixIT, VBx, and end-to-end foundations

[P1]: https://huggingface.co/pyannote/speaker-diarization-3.1
[P2]: https://huggingface.co/pyannote/speaker-diarization-community-1
[P3]: https://docs.pyannote.ai/models
[P4]: https://arxiv.org/html/2403.02288v2
[P5]: https://github.com/pyannote/pyannote-audio/blob/main/src/pyannote/audio/pipelines/clustering.py
[P6]: https://github.com/pyannote/pyannote-audio/blob/main/src/pyannote/audio/pipelines/speech_separation.py
[P7]: https://huggingface.co/pyannote/speech-separation-ami-1.0
[P8]: https://huggingface.co/pyannote/separation-ami-1.0
[V1]: https://arxiv.org/abs/2012.14952
[V2]: https://arxiv.org/html/2510.19572
[V3]: https://github.com/pyannote/pyannote-audio/blob/main/src/pyannote/audio/utils/vbx.py
[V4]: https://arxiv.org/html/2310.02732
[V5]: https://arxiv.org/html/2305.13580v1
[Q1]: https://arxiv.org/abs/2106.10654
[C1]: https://arxiv.org/html/2109.06483
[C2]: https://github.com/juanmc2005/diart

- [P1] Speaker diarization 3.1 model card: runtime, inputs, count control, gating/license.
- [P2] Community-1 model card: gate, offline use, ordinary/exclusive outputs, benchmark and component references.
- [P3] pyannoteAI model/capability documentation, including Precision-2/3, voiceprints and hosting.
- [P4] Kalda et al., *PixIT*, 2024: architecture/training, original-audio embeddings, AHC stitching, leakage ablation and meeting ASR results.
- [P5–P6] First-party pipeline code, including clean-embedding filtering, centroid/AHC/VBx, assignment, source reconstruction and leakage removal.
- [P7–P8] Distinct pipeline and local joint-network cards; both gated MIT listings.
- [V1] Landini et al., Bayesian HMM clustering of x-vector sequences; [V4] Klement et al., discriminative VBx, 2023.
- [V2] Pálka et al., *VBx for end-to-end neural and clustering-based diarization*, 2025: GMM simplification, filtering, constraints and evaluation.
- [V3] First-party GMM-VBx utility; header explicitly records dropped HMM support.
- [V5] Delcroix et al., *Multi-Stream Extension ... (MS-VBx)*, 2023.
- [Q1] Horiguchi et al., encoder-decoder attractors; [C1] Coria et al., 2021 incremental constrained diarization; [C2] maintained MIT diart implementation.

### Clustering mechanisms and implementations

[A1]: https://docs.scipy.org/doc/scipy/reference/generated/scipy.cluster.hierarchy.linkage.html
[A2]: https://arxiv.org/html/2003.02405v1
[H1]: https://hdbscan.readthedocs.io/en/latest/performance_and_scalability.html
[U1]: https://umap-learn.readthedocs.io/en/latest/clustering.html
[E1]: https://arxiv.org/abs/2611.09501
[E2]: https://e-means.github.io/
[E3]: https://github.com/facebookresearch/faiss/blob/v1.16.0/contrib/e_means/README.md
[E4]: https://github.com/facebookresearch/faiss/releases/tag/v1.16.0
[E5]: https://www.lkuffo.com/superemeans/
[E6]: https://github.com/cwida/SuperKMeans
[E7]: https://github.com/facebookresearch/faiss/blob/v1.16.0/LICENSE
[G1]: https://genieclust.gagolewski.com/rapi/gclust.html
[G2]: https://genieclust.gagolewski.com/pythonapi_root.html
[G3]: https://github.com/gagolews/genieclust
[G4]: https://github.com/gagolews/genieclust/blob/master/LICENSE
[G5]: https://doi.org/10.1016/j.ins.2016.05.003

- [A1] SciPy definitions/complexities and Euclidean caveat; [A2] Park et al., NME-SC original paper.
- [H1] HDBSCAN implementation/performance discussion; [U1] UMAP authors' clustering guide and density-distortion warning.
- [E1–E4] e-means paper, author project explanation, versioned API/algorithm guide and actual Faiss release.
- [E5–E7] SuperEMeans maintainer's benchmark, MIT implementation, and Faiss MIT license.
- [G1–G5] Genie author's manuals, implementation and explicit AGPL-3.0 license; Gagolewski, Bartoszuk and Cena's original algorithm paper.

### Alternative toolkits and native paths

[N1]: https://docs.nvidia.com/nemo-framework/user-guide/26.02/nemotoolkit/asr/speaker_diarization/models.html
[N2]: https://docs.nvidia.com/nemo/speech/latest/asr/speaker_diarization/configs.html
[N3]: https://huggingface.co/nvidia/diar_sortformer_4spk-v1
[N4]: https://huggingface.co/nvidia/diar_streaming_sortformer_4spk-v2
[N5]: https://huggingface.co/nvidia/diar_streaming_sortformer_4spk-v2.1
[N6]: https://huggingface.co/nvidia/Nemotron-3-Diarization
[N7]: https://github.com/NVIDIA/NeMo-Speech.cpp
[N8]: https://github.com/NVIDIA-NeMo/NeMo
[Z1]: https://github.com/BUTSpeechFIT/DiariZen
[D1]: https://github.com/modelscope/3D-Speaker/blob/main/egs/3dspeaker/speaker-diarization/README.md
[D2]: https://github.com/modelscope/3D-Speaker/blob/main/speakerlab/process/cluster.py
[D3]: https://github.com/modelscope/3D-Speaker/blob/main/egs/3dspeaker/speaker-diarization/conf/diar.yaml
[D4]: https://github.com/modelscope/3D-Speaker
[W1]: https://github.com/wenet-e2e/wespeaker
[W2]: https://github.com/wenet-e2e/wespeaker/blob/master/docs/voxconverse_diar.md
[W3]: https://github.com/wenet-e2e/wespeaker/blob/master/wespeaker/diar/umap_clusterer.py
[X1]: https://github.com/m-bain/whisperX
[B1]: https://github.com/speechbrain/speechbrain/tree/develop/recipes/AMI/Diarization
[B2]: https://github.com/speechbrain/speechbrain
[F1]: https://picovoice.ai/docs/falcon/
[F2]: https://picovoice.ai/docs/eagle/
[F3]: https://github.com/Picovoice/falcon
[F4]: https://github.com/Picovoice/eagle
[S1]: https://k2-fsa.github.io/sherpa/onnx/speaker-diarization/index.html
[S2]: https://github.com/k2-fsa/sherpa-onnx

- [N1–N2] NVIDIA architecture/configuration docs distinguish direct Sortformer output from clustering-initialized MSDD and expose practical count/search controls.
- [N3–N6] Version-specific model cards; [N7–N8] native runtime and toolkit licensing/interfaces.
- [Z1] DiariZen authors distinguish MIT code from non-commercial weights.
- [D1–D4], [W1–W3] Official recipes, clustering implementations/configuration, licenses and model/runtime inventories.
- [X1], [B1–B2] WhisperX and SpeechBrain authors' pipeline/recipe documentation and code terms.
- [F1–F4] Picovoice product docs/repos distinguish local inference from AccessKey/license-server validation.
- [S1–S2] sherpa-onnx official diarization/native API and toolkit documentation.

### Identification and non-acoustic evidence

[I1]: https://arxiv.org/html/2211.00815
[I2]: https://arxiv.org/html/2208.13085v3
[I3]: https://support.zoom.com/hc/en/article?id=zm_kb&sysparm_article=KB0077409
[I4]: https://learn.microsoft.com/en-us/microsoftteams/rooms/voice-recognition
[I5]: https://docs.pyannote.ai/tutorials/identification-with-voiceprints
[I6]: https://docs.speechmatics.com/speech-to-text/features/speaker-identification
[I7]: https://arxiv.org/html/2305.19581v1
[I8]: https://www.assemblyai.com/docs/speech-understanding/speaker-identification
[I9]: https://developers.zoom.us/docs/rtms/meetings/media
[I10]: https://docs.granola.ai/help-center/taking-notes/speaker-attribution

- [I1] Chen et al., 2023 revision: cosine/PLDA comparison, AS-norm, quality-aware calibration and metric-specific tradeoffs.
- [I2] Transformer TS-VAD and EEND-EDA integration; [I7] 2023 personal/SV-sensitive VAD.
- [I3–I6], [I8–I10] Official sources also documented in the preceding research; used here for bounded-candidate/open-set, model-pin, text-evidence and metadata-fusion implications, not a repeated competitor survey.
