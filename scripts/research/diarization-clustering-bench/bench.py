# /// script
# requires-python = ">=3.12,<3.13"
# dependencies = [
#   "numpy==2.2.6", "scipy==1.15.3", "scikit-learn==1.6.1",
#   "sherpa-onnx==1.13.4", "soundfile==0.13.1", "requests==2.32.3",
#   "threadpoolctl==3.6.0", "genieclust==1.3.0", "faiss-cpu==1.16.0",
# ]
# ///
"""Small, CPU-only clustering experiment; see README for scope and attribution."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import platform
import subprocess
import sys
import time

# Set before native imports, including spawned timing workers.
os.environ["OMP_NUM_THREADS"] = "1"
os.environ["OPENBLAS_NUM_THREADS"] = "1"
os.environ["MKL_NUM_THREADS"] = "1"
import numpy as np
import requests
import soundfile as sf
import sherpa_onnx
from scipy.cluster.hierarchy import fcluster, linkage
from scipy.linalg import eigh
from scipy.optimize import linear_sum_assignment
from scipy.sparse import csr_matrix, diags
from scipy.sparse.linalg import eigsh
from scipy.spatial.distance import pdist
from sklearn.cluster import HDBSCAN, KMeans
from sklearn.metrics import adjusted_rand_score, silhouette_score
from threadpoolctl import threadpool_limits
from faiss.contrib.e_means import NumpyEmeans
from genieclust import Genie

ROOT = Path(__file__).resolve().parent
DATA = ROOT / "data"
CACHE = ROOT / ".cache"
RELEASE = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/"
MODELS = {"titanet": "nemo_en_titanet_small.onnx", "campp": "3dspeaker_speech_campplus_sv_en_voxceleb_16k.onnx"}
AMI = "https://raw.githubusercontent.com/pyannote/AMI-diarization-setup/67c2d539286e89f68952d5dcf83912bd9f01dfae/"
COUNTS = (2, 3, 4, 6, 8, 10)
SEEDS = (11, 22, 33)
MAX_K = 12


def save(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2), encoding="utf-8")


def download(url, path):
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        response = requests.get(url, timeout=180)
        response.raise_for_status()
        path.write_bytes(response.content)
    return path


def normalize(x):
    return np.asarray(x, dtype=np.float32) / np.maximum(np.linalg.norm(x, axis=-1, keepdims=True), 1e-12)


def extractor(model):
    path = download(RELEASE + MODELS[model], DATA / MODELS[model])
    config = sherpa_onnx.SpeakerEmbeddingExtractorConfig(model=str(path), num_threads=1, provider="cpu")
    return sherpa_onnx.SpeakerEmbeddingExtractor(config)


def embed(ex, audio):
    stream = ex.create_stream()
    stream.accept_waveform(sample_rate=16000, waveform=np.ascontiguousarray(audio, dtype=np.float32))
    stream.input_finished()
    if not ex.is_ready(stream):
        raise RuntimeError("Extractor rejected a 1.5-second window")
    return normalize(np.asarray(ex.compute(stream), dtype=np.float32))


def librispeech_rows():
    manifest = DATA / "librispeech-rows.json"
    if manifest.exists():
        return json.loads(manifest.read_text())
    def page(offset):
        response = requests.get("https://datasets-server.huggingface.co/rows", params={
            "dataset": "openslr/librispeech_asr", "config": "clean", "split": "test",
            "offset": offset, "length": min(100, 2620-offset)}, timeout=120)
        response.raise_for_status()
        return [item["row"] for item in response.json()["rows"]]
    with ThreadPoolExecutor(max_workers=6) as pool:
        rows = [row for chunk in pool.map(page, range(0, 2620, 100)) for row in chunk]
    save(manifest, rows)
    return rows


def prepare(model):
    bank = CACHE / f"librispeech-{model}.npz"
    if bank.exists():
        with np.load(bank) as existing:
            if min(np.unique(existing["y"], return_counts=True)[1]) >= 100:
                return
    rows = librispeech_rows()
    speakers = sorted({row["speaker_id"] for row in rows})
    # A disjoint speaker split, stronger than merely disjoint session seeds.
    selected = speakers[:12] + speakers[-12:]
    chosen = [row for speaker in selected for row in [r for r in rows if r["speaker_id"] == speaker][:12]]
    def get_audio(row):
        return download(row["audio"][0]["src"], DATA / "librispeech" / (row["id"] + ".flac"))
    with ThreadPoolExecutor(max_workers=6) as pool:
        paths = list(pool.map(get_audio, chosen))
    # Some readers have short utterances. Extend their prefix, never duplicate a window.
    for speaker in selected:
        count = sum(max(0, (sf.info(path).frames-24000)//12000+1)
                    for row, path in zip(chosen, paths) if row["speaker_id"] == speaker)
        for row in [r for r in rows if r["speaker_id"] == speaker][12:]:
            if count >= 100:
                break
            path = get_audio(row)
            chosen.append(row)
            paths.append(path)
            count += max(0, (sf.info(path).frames-24000)//12000+1)
        if count < 100:
            raise ValueError(f"Insufficient source audio for speaker {speaker}: {count} windows")
    ex = extractor(model)
    vectors, labels, utterances, starts = [], [], [], []
    start = time.perf_counter()
    for index, (row, path) in enumerate(zip(chosen, paths)):
        audio, rate = sf.read(path, dtype="float32")
        if rate != 16000:
            raise ValueError(f"Unexpected sample rate {rate}")
        for offset in range(0, len(audio)-24000+1, 12000):
            vectors.append(embed(ex, audio[offset:offset+24000]))
            labels.append(row["speaker_id"])
            utterances.append(row["id"])
            starts.append(offset/16000)
        if index % 24 == 0:
            print(f"{model}: {index+1}/{len(chosen)} utterances", flush=True)
    CACHE.mkdir(parents=True, exist_ok=True)
    np.savez_compressed(bank, x=np.array(vectors), y=labels, utterance=utterances, start=starts,
                        dev=selected[:12], test=selected[12:])
    save(CACHE / f"extraction-{model}.json", {"seconds": time.perf_counter()-start, "n": len(vectors),
         "dimension": ex.dim, "model_sha256": hashlib.sha256((DATA/MODELS[model]).read_bytes()).hexdigest(),
         "utterances": len(chosen), "dev_speakers": selected[:12], "test_speakers": selected[12:]})


def session(bank, split, k, seed, n=160):
    rng = np.random.default_rng(seed + 100*k)
    speakers = rng.choice(bank[split], size=k, replace=False)
    dominant = int(n*.6) if k == 2 else n//2
    sizes = np.full(k, (n-dominant)//(k-1), dtype=int)
    sizes[0] = dominant
    sizes[-1] += n-int(sizes.sum())
    turns = []
    for speaker, count in zip(speakers, sizes):
        candidates = np.flatnonzero(bank["y"] == speaker)
        if len(candidates) < count:
            raise ValueError(f"Speaker {speaker}: only {len(candidates)} windows, need {count}")
        # Sample without replacement; retain within-speaker time order and interleave short turns.
        picks = np.sort(rng.choice(candidates, size=count, replace=False))
        turns.extend([picks[i:i+4] for i in range(0, len(picks), 4)])
    rng.shuffle(turns)
    ix = np.concatenate(turns)
    return bank["x"][ix], bank["y"][ix]


def ahc(x, threshold, method="average"):
    return fcluster(linkage(pdist(x, metric="cosine"), method=method), threshold, criterion="distance")


def registry(x, threshold=0.6, window=8, local_threshold=None):
    """stabilize.rs rules, including snapshot matches and count-weighted renormalization.

    With no local_threshold, each window contains one 1.5s local cluster (no oracle).
    Otherwise, AHC supplies locals in eight-embedding batches (not fixed-time AMI windows).
    """
    centroids, counts, output = [], [], np.empty(len(x), dtype=int)
    for start in range(0, len(x), window):
        chunk = x[start:start+window]
        labels = ahc(chunk, local_threshold, "complete") if local_threshold is not None and len(chunk)>1 else np.zeros(len(chunk), dtype=int)
        local_ids = np.unique(labels)
        locals_ = normalize(np.array([chunk[labels == label].mean(axis=0) for label in local_ids]))
        # Union of 1.5s intervals at 0.75s hops, including nonadjacent local members.
        durations = []
        for label in local_ids:
            positions = np.flatnonzero(labels == label)
            durations.append(1.5 + np.minimum(np.diff(positions)*.75, 1.5).sum())
        sim = locals_ @ np.array(centroids).T if centroids else np.empty((len(locals_), 0))
        triples = sorted((-float(sim[li, gi]), li, gi) for li in range(len(locals_))
                         for gi in range(len(centroids)) if sim[li, gi] >= threshold)
        matches, used = {}, set()
        for _, li, gi in triples:
            if li not in matches and gi not in used:
                matches[li] = gi
                used.add(gi)
        for li, label in enumerate(local_ids):
            if li in matches:
                gi = matches[li]
                if durations[li] >= 1.5:
                    centroids[gi] = normalize((centroids[gi]*counts[gi]+locals_[li])/(counts[gi]+1))
                    counts[gi] += 1
            else:
                gi = len(centroids)
                centroids.append(locals_[li].copy())
                counts.append(1)
            output[start+np.flatnonzero(labels == label)] = gi
    return output


def nme(x):
    """NME-SC style: binary p-NN, unnormalized Laplacian, normalized max eigengap.

    NumPy/SciPy reimplementation, not a claim of NeMo bit-for-bit parity.
    Search <=512 uniformly subsampled points, 30 p values up to 0.25*n.
    """
    sample = x[np.linspace(0, len(x)-1, min(512, len(x)), dtype=int)]
    similarity = sample @ sample.T
    n = len(sample)
    order = np.argsort(-similarity, axis=1)
    best = None
    for p in np.unique(np.linspace(2, max(2, int(n*0.25)), 30).astype(int)):
        a = np.zeros((n, n), dtype=np.float64)
        a[np.arange(n)[:, None], order[:, :p]] = 1
        a = (a+a.T)/2
        np.fill_diagonal(a, 0)
        lap = np.diag(a.sum(axis=1))-a
        values = eigh(lap, eigvals_only=True, check_finite=False)
        gaps = np.diff(values[:MAX_K+1])
        k = int(np.argmax(gaps)+1)
        objective = (p/n)/(gaps[k-1]/max(values[-1], 1e-12)+1e-12)
        if best is None or objective < best[0]:
            best = (objective, k, p/n)
    return best[1], best[2]


def spectral(x, k, ratio, seed):
    n = len(x)
    p = max(2, min(n, int(np.ceil(n*ratio))))
    # Block affinities avoid a second dense n*n matrix at 10k.
    rows, cols = [], []
    for start in range(0, n, 256):
        affinity = x[start:start+256] @ x.T
        nearest = np.argpartition(-affinity, p-1, axis=1)[:, :p]
        rows.append(np.repeat(np.arange(start, start+len(nearest)), p))
        cols.append(nearest.ravel())
    rr, cc = np.concatenate(rows), np.concatenate(cols)
    a = csr_matrix((np.ones(len(rr)), (rr, cc)), shape=(n, n))
    a = (a+a.T)*0.5
    a.setdiag(0)
    lap = diags(np.asarray(a.sum(axis=1)).ravel())-a
    if n <= 1000:
        _, vectors = eigh(lap.toarray(), subset_by_index=(0, k-1), check_finite=False)
    else:
        _, vectors = eigsh(lap, k=k, which="SM", v0=np.random.default_rng(seed).normal(size=n), tol=1e-4)
    return KMeans(n_clusters=k, n_init=10, random_state=seed, algorithm="lloyd").fit_predict(vectors)


def lloyd(x, k, seed):
    return KMeans(n_clusters=k, n_init=10, random_state=seed, algorithm="lloyd").fit_predict(x)


def emeans(x, k, seed):
    # Reassign to returned final centroids rather than mixing last-epoch labels.
    centroids, _ = NumpyEmeans(x.shape[1], k, nepochs=5, seed=seed).train(x)
    return ((x[:, None, :]-centroids[None, :, :])**2).sum(axis=2).argmin(axis=1)


def silhouette_k(x, seed):
    sample = x[np.random.default_rng(seed).choice(len(x), min(512, len(x)), replace=False)]
    scored = [(silhouette_score(sample, labels, metric="cosine"), k)
              for k in range(2, MAX_K+1) for labels in [lloyd(sample, k, seed)]]
    return max(scored)[1]


def genie(x, k, gini):
    return Genie(n_clusters=k, metric="cosine", gini_threshold=gini).fit_predict(x)


def genie_cut(x, threshold, gini):
    model = Genie(n_clusters=2, metric="cosine", gini_threshold=gini).fit(x)
    # Cosine distance is nonnegative; native dot roundoff on duplicate vectors can be <0.
    tree = np.column_stack((model.children_, np.maximum(0, model.distances_), model.counts_))
    return fcluster(tree, threshold, criterion="distance")


def hdbscan(x):
    # sklearn's brute/precomputed route supports cosine; tree backends do not.
    distances = np.maximum(0, 1-x.astype(np.float64) @ x.astype(np.float64).T)
    np.fill_diagonal(distances, 0)
    return HDBSCAN(min_cluster_size=5, min_samples=3, metric="precomputed", allow_single_cluster=True).fit_predict(distances)


def metrics(y, labels):
    _, y = np.unique(y, return_inverse=True)
    # Noise is always wrong for confusion/purity; coverage includes its dispersion.
    ids = sorted(set(labels)-{-1})
    table = np.zeros((len(set(y)), len(ids)), dtype=int)
    for j, label in enumerate(ids):
        table[:, j] = np.bincount(y[labels == label], minlength=table.shape[0])
    matched = table[linear_sum_assignment(-table)].sum() if table.size else 0
    return {"ari": float(adjusted_rand_score(y, labels)),
            "purity": float(table.max(axis=0).sum()/len(y)) if table.size else 0.,
            "coverage": float(table.max(axis=1).sum()/len(y)) if table.size else 0.,
            "count_error": len(ids)-len(set(y)), "confusion": float(1-matched/len(y)),
            "noise_fraction": float(np.mean(labels == -1))}


def timed(call):
    start = time.perf_counter()
    labels = call()
    return labels, time.perf_counter()-start


def tune(bank):
    sessions = [session(bank, "dev", k, seed) for k in COUNTS for seed in (101, 202)]
    def select(values, run):
        scored = [(float(np.mean([metrics(y, run(x, value))["confusion"] for x, y in sessions])), value)
                  for value in values]
        return min(scored)[1], scored
    threshold, ahc_scores = select(np.arange(.2, .851, .05), ahc)
    complete, complete_scores = select(np.arange(.2, 1.201, .05), lambda x, t: ahc(x, t, "complete"))
    similarity, online_scores = select(np.arange(.3, .851, .05), lambda x, v: registry(x, v, window=8, local_threshold=complete))
    gini_scores = []
    for g in (.1, .3, .5, .7, 1.):
        cut, scores = select(np.arange(.1, .701, .05), lambda x, t: genie_cut(x, t, g))
        gini_scores.append((min(s[0] for s in scores), g, cut))
    _, gini, cut = min(gini_scores)
    return {"ahc_threshold": float(threshold), "complete_threshold": float(complete), "online_threshold": float(similarity),
            "genie_gini": gini, "genie_cut": float(cut), "ahc_dev_grid": ahc_scores,
            "complete_dev_grid": complete_scores, "online_dev_grid": online_scores,
            "genie_dev_best_by_gini": gini_scores}


def methods(x, y, seed, settings):
    k = len(set(y))
    count_start = time.perf_counter()
    estimated, ratio = nme(x)
    nme_time = time.perf_counter()-count_start
    sil_start = time.perf_counter()
    sil = silhouette_k(x, seed)
    sil_time = time.perf_counter()-sil_start
    jobs = {
        "ahc_tuned": (lambda: ahc(x, settings["ahc_threshold"]), 0),
        "ahc_default_0.5": (lambda: ahc(x, .5), 0),
        "complete_tuned": (lambda: ahc(x, settings["complete_threshold"], "complete"), 0),
        "complete_default_0.5": (lambda: ahc(x, .5, "complete"), 0),
        "spectral_nme": (lambda: spectral(x, estimated, ratio, seed), nme_time),
        "kmeans_nme": (lambda: lloyd(x, estimated, seed), nme_time),
        "kmeans_silhouette": (lambda: lloyd(x, sil, seed), sil_time),
        "kmeans_oracle": (lambda: lloyd(x, k, seed), 0),
        "emeans_nme": (lambda: emeans(x, estimated, seed), nme_time),
        "emeans_silhouette": (lambda: emeans(x, sil, seed), sil_time),
        "emeans_oracle": (lambda: emeans(x, k, seed), 0),
        "genie_nme": (lambda: genie(x, estimated, settings["genie_gini"]), nme_time),
        "genie_cut_tuned": (lambda: genie_cut(x, settings["genie_cut"], settings["genie_gini"]), 0),
        "hdbscan": (lambda: hdbscan(x), 0),
        "registry_default": (lambda: registry(x, .6, 8, settings["complete_threshold"]), 0),
        "registry_tuned": (lambda: registry(x, settings["online_threshold"], 8, settings["complete_threshold"]), 0),
        "registry_single_default": (lambda: registry(x, .6, 1), 0),
    }
    for gini in (.1, .3, .5, .7, 1.):
        jobs[f"genie_oracle_g{gini}"] = (lambda g=gini: genie(x, k, g), 0)
    result = []
    for name, (call, count_time) in jobs.items():
        labels, seconds = timed(call)
        result.append({"method": name, **metrics(y, labels), "seconds": seconds+count_time,
                       "fit_seconds": seconds, "estimated_k_nme": estimated, "estimated_k_silhouette": sil})
    return result


def quality(model):
    bank = np.load(CACHE / f"librispeech-{model}.npz")
    settings = tune(bank)
    save(CACHE / f"settings-{model}.json", settings)
    results = []
    for k in COUNTS:
        for seed in SEEDS:
            x, y = session(bank, "test", k, seed)
            rows = methods(x, y, seed, settings)
            results.extend(dict(row, embedder=model, corpus="librispeech", true_k=k, seed=seed, n=len(x)) for row in rows)
            save(CACHE / f"quality-{model}.json", results)
            print(f"quality {model} k={k} seed={seed}", flush=True)


def ami(model):
    ex = extractor(model)
    settings = json.loads((CACHE / f"settings-{model}.json").read_text())
    results = []
    for meeting in ("ES2004a", "TS3003a"):
        bank = CACHE / f"ami-{meeting}-{model}.npz"
        if not bank.exists():
            rttm = download(AMI+f"only_words/rttms/test/{meeting}.rttm", DATA / "ami" / f"{meeting}.rttm")
            url = f"https://groups.inf.ed.ac.uk/ami/AMICorpusMirror/amicorpus/{meeting}/audio/{meeting}.Mix-Headset.wav"
            path = download(url, DATA / "ami" / f"{meeting}.wav")
            turns = []
            for line in rttm.read_text().splitlines():
                fields = line.split()
                turns.append((float(fields[3]), float(fields[3])+float(fields[4]), fields[7]))
            # Sweep reference boundaries; keep maximal contiguous exactly-one-speaker spans.
            events = sorted({v for a, b, _ in turns for v in (a, b)})
            spans = []
            for a, b in zip(events, events[1:]):
                active = {s for start, end, s in turns if start < b and end > a}
                if len(active) == 1:
                    speaker = next(iter(active))
                    if spans and spans[-1][1] == a and spans[-1][2] == speaker:
                        spans[-1] = (spans[-1][0], b, speaker)
                    else:
                        spans.append((a, b, speaker))
            vectors, labels, timestamps = [], [], []
            with sf.SoundFile(path) as audio:
                if audio.samplerate != 16000:
                    raise ValueError("AMI is not 16kHz")
                # At most 400 evenly distributed reference windows, retain chronology.
                windows = [(t, s) for a, b, s in spans for t in np.arange(a, b-1.5+1e-7, .75)]
                ix = np.linspace(0, len(windows)-1, min(400, len(windows)), dtype=int)
                for index in ix:
                    t, s = windows[index]
                    audio.seek(round(t*16000))
                    waveform = audio.read(24000, dtype="float32", always_2d=True).mean(axis=1)
                    vectors.append(embed(ex, waveform))
                    labels.append(s)
                    timestamps.append(t)
            np.savez_compressed(bank, x=np.array(vectors), y=labels, start=timestamps)
        data = np.load(bank)
        for seed in SEEDS:
            rows = methods(data["x"], data["y"], seed, settings)
            results.extend(dict(row, embedder=model, corpus="ami", meeting=meeting, seed=seed,
                                true_k=len(set(data["y"])), n=len(data["x"])) for row in rows)
        save(CACHE / f"ami-quality-{model}.json", results)
        print(f"AMI {model} {meeting}: {len(data['x'])} embeddings", flush=True)


def timing_worker(model, name, n, seed):
    bank = np.load(CACHE / f"librispeech-{model}.npz")
    rng = np.random.default_rng(seed)
    x0, y0 = session(bank, "test", 8, seed)
    # Bootstrap real embeddings for scaling only; quality uses no repeated windows.
    x = x0[rng.choice(len(x0), n, replace=True)]
    settings = json.loads((CACHE / f"settings-{model}.json").read_text())
    def run():
        if name == "ahc_tuned": return ahc(x, settings["ahc_threshold"])
        if name == "complete_tuned": return ahc(x, settings["complete_threshold"], "complete")
        if name == "registry_tuned": return registry(x, settings["online_threshold"], 8, settings["complete_threshold"])
        if name == "hdbscan": return hdbscan(x)
        if name == "genie_cut_tuned": return genie_cut(x, settings["genie_cut"], settings["genie_gini"])
        if name.endswith("silhouette"):
            k = silhouette_k(x, seed)
        elif name.endswith("oracle"):
            k = len(set(y0))
        else:
            k, ratio = nme(x)
        if name == "spectral_nme": return spectral(x, k, ratio, seed)
        if name.startswith("kmeans"): return lloyd(x, k, seed)
        if name.startswith("emeans"): return emeans(x, k, seed)
        if name == "genie_nme": return genie(x, k, settings["genie_gini"])
        raise ValueError(name)
    _, seconds = timed(run)
    print(json.dumps({"seconds": seconds}))


def timing(model, selected=None, fresh=False):
    names = ("ahc_tuned", "complete_tuned", "spectral_nme", "kmeans_nme", "kmeans_silhouette", "kmeans_oracle",
             "emeans_nme", "emeans_silhouette", "emeans_oracle", "genie_nme", "genie_cut_tuned",
             "hdbscan", "registry_tuned")
    names = tuple(selected) if selected else names
    output = CACHE / f"timing-{model}.json"
    results = json.loads(output.read_text()) if output.exists() else []
    for n in (100, 1000, 10000):
        for seed in SEEDS:
            for name in names:
                previous = [r for r in results if (r["n"], r["seed"], r["method"]) == (n, seed, name)]
                if previous and previous[0]["seconds"] is not None and not fresh:
                    continue
                results = [r for r in results if (r["n"], r["seed"], r["method"]) != (n, seed, name)]
                command = [sys.executable, str(Path(__file__)), "timing-worker", "--models", model,
                           "--method", name, "--n", str(n), "--seed", str(seed)]
                try:
                    result = subprocess.run(command, capture_output=True, text=True, timeout=180, check=True)
                    row = json.loads(result.stdout.strip().splitlines()[-1])
                except subprocess.TimeoutExpired:
                    row = {"seconds": None, "status": "timeout>180s"}
                except subprocess.CalledProcessError as error:
                    row = {"seconds": None, "status": "error", "detail": error.stderr[-2000:]}
                results.append(dict(row, embedder=model, method=name, n=n, seed=seed))
                save(CACHE / f"timing-{model}.json", results)
                print(f"timing {model} {name} n={n} seed={seed}: {row}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("all", "prepare", "quality", "ami", "timing", "timing-worker"))
    parser.add_argument("--models", nargs="+", choices=MODELS, default=list(MODELS))
    parser.add_argument("--method")
    parser.add_argument("--n", type=int)
    parser.add_argument("--seed", type=int)
    parser.add_argument("--methods", nargs="+", help="Timing method subset")
    parser.add_argument("--fresh", action="store_true", help="Replace selected successful timing rows")
    args = parser.parse_args()
    CACHE.mkdir(parents=True, exist_ok=True)
    versions = {name: importlib.metadata.version(name) for name in ("numpy", "scipy", "scikit-learn", "sherpa-onnx", "soundfile", "genieclust", "faiss-cpu", "requests", "threadpoolctl")}
    if args.command != "timing-worker":
        save(CACHE / "environment.json", {"python": sys.version, "platform": platform.platform(),
             "processor": platform.processor(), "versions": versions, "threads": 1})
    with threadpool_limits(limits=1):
        for model in args.models:
            if args.command == "timing-worker": timing_worker(model, args.method, args.n, args.seed)
            else:
                for name, fn in (("prepare", prepare), ("quality", quality), ("ami", ami), ("timing", timing)):
                    if args.command in (name, "all"):
                        if name == "timing":
                            fn(model, args.methods, args.fresh)
                        else:
                            fn(model)


if __name__ == "__main__":
    main()
