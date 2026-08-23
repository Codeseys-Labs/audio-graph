# Model distribution, storage, and lifecycle for offline STT v2

## Verdict

Do **not** adopt `hf-hub` as the core download-manager dependency right now, and do not
bundle a model registry format from Ollama/LM Studio/Jan wholesale either — build a thin,
AudioGraph-owned download manager (resumable `reqwest` + an external, signed-adjacent JSON
manifest + SHA-256 verification) and keep the current one-file-per-model flat cache layout,
because that layout already avoids the single biggest concrete problem this research
surfaced: **`hf-hub`'s content-addressed blob/snapshot cache does not work as advertised on
Windows.** On Windows the crate unconditionally copies blobs into snapshot paths instead of
symlinking (real privilege requirement, not a bug), which (a) doubles on-disk size for every
model — a real cost at AudioGraph's 0.5–3GB-per-model scale — and (b) as the crate's own
source comments admit, disables the conditional-request (`If-None-Match`/304) cache-hit path
that the format exists to provide. Layered onto that: `hf-hub` just underwent a **complete,
breaking API rewrite** (0.5.0 in Feb 2026 → three release candidates → 1.0.0 on
2026-07-10, with `main` already ahead at 1.1.0-in-progress) that drops the `ureq`
zero-tokio blocking backend the ecosystem currently depends on, and none of the
highest-traffic reverse dependencies (`tokenizers`, `candle-examples`, at 28M+ combined
downloads) have moved off the pre-rewrite `^0.4`/`^0.5` line yet — this is a five-week-old
API surface with no field mileage, a bad time to make it AudioGraph's model-lifecycle spine.
AudioGraph's current `models/mod.rs` (plain `reqwest::blocking`, hand-written `ModelDef`
table) is closer to correct than it looks, but it is missing three cheap, high-value things
every serious precedent has: **checksum verification** (today it's size-within-1%, which is
not tamper- or corruption-proof), **revision pinning** (every HF URL is a floating
`resolve/main/...`, so an upstream repo push silently changes what a byte-identical-looking
download actually is), and **`%LOCALAPPDATA%` instead of `%APPDATA%`** for the models
directory (Tauri's `app_data_dir()` resolves to the *roaming* profile on Windows, and
roaming profiles sync over the network at logon/logoff in AD-joined enterprise
environments — multi-GB models do not belong there).

---

## 1. `hf-hub` crate: the 1.0 rewrite is a discontinuity, not an incremental bump

**Verified via crates.io + GitHub source as of 2026-08-23.**

`hf-hub` had a stable, narrow, `Cache`/`Api`/`Repo`-shaped API through **v0.4.0/0.4.1
(2024-12-30/31), v0.4.3 (2025-06-16), v0.5.0 (2026-02-19)**. That API — `Api::new()`,
`.model(repo_id)`, `.get(filename)` — is what `candle-transformers`, `tokenizers`, and
essentially every Rust ML tutorial reference. It supports:

- **`--features ureq,rustls-tls`** — fully synchronous, zero-tokio blocking client (good
  fit for a binary that doesn't want to pull an async runtime in just to fetch a file), or
  **`--features tokio,rustls-tls`** for async (default TLS on the `tokio` feature is
  `native-tls`/OpenSSL unless `rustls-tls` is explicitly selected — a real Windows build
  footgun since OpenSSL needs vendoring/cross-toolchain setup that `rustls` avoids).
- **HTTP `Range`-header resume** on interrupted downloads (`api/sync.rs`:
  `download_from(url, current, ...)` retried with `bytes={current}-`), with jittered
  exponential backoff (`jitter()` + `exponential_backoff()`).
- **Per-download file locking** (`lock_file`) so two processes can't corrupt the same blob.
- **ETag-based, content-addressed cache**: `blobs/<etag>` + `snapshots/<revision>/<filename>`
  pointer, mirroring Python `huggingface_hub`'s cache format bit-for-bit so a shared cache
  dir works with both ecosystems.
- **Token auth** via `ApiBuilder::with_token`, **no env-var auto-detection in the newest
  line** (see below — this changed).
- **Revision pinning**: `api.model(repo_id)` defaults to `main`, but `Repo::with_revision`
  takes any branch/tag/commit SHA.

The Windows-specific gotcha, verified in `v0.5.0`'s `symlink_or_rename()`
(`hf-hub/src/api/sync.rs:435`): it *attempts* `std::os::windows::fs::symlink_file` first
(this only succeeds if the user has Developer Mode enabled or admin + the
`SeCreateSymbolicLinkPrivilege`, which is **not the default state of a consumer Windows
box**) and on failure falls back to `std::fs::rename(src, dst)` — i.e. it **moves** the blob
out of the content-addressed store into the snapshot location. That silently breaks future
blob-sharing for that file: a second revision/repo that would have deduped against that blob
now has to re-download it, because the blob file no longer exists at its content-addressed
path.

Then, starting with **`v1.0.0-rc.0/rc.1/rc.2` → `1.0.0` (published to crates.io
2026-07-10)**, the crate became a **full rewrite**: `HFClient`/`HFClientSync` mirroring the
entire Hub REST API (repo CRUD, commits, branches/tags, buckets, users/orgs, upload), not
just download. Confirmed from the current README and `hf-hub/src/repository/download.rs` on
`main` (which is already at `1.1.0` in `Cargo.toml`, ahead of the last crates.io release —
**UNVERIFIED which exact commit becomes 1.1.0**, and notably GitHub has **no `v1.0.0` or
`v1.1.0` tag** even though crates.io shows 1.0.0 shipped, only `v1.0.0-rc.0/1/2` and earlier
— a real mismatch between the tag history and the published crate, worth treating as a
signal this project's release process is still settling):

- `hf-hub` **no longer reads any environment variables** (no `HF_HOME`, no `HF_TOKEN`
  auto-pickup). `HFClient::new()` defaults to caching in `.cache/huggingface/hub`
  **relative to the process's current working directory** — for a GUI app launched from
  Explorer/dock, CWD is not guaranteed to be anything sane. `HFClient::builder().cache_dir(...)`
  must be set explicitly. This is a footgun the classic API mostly avoided (it defaulted to
  a real home-relative cache dir).
- `hf-xet` (HF's Rust-native chunked, content-defined-chunking CAS transfer client,
  `github.com/huggingface/xet-core`, 1.6.0 max on crates.io, 825k downloads) is now a
  **hard, non-optional dependency** — even repos that don't use Xet storage compile it in.
  Not a blocker, just added surface.
- The `blocking` feature adds `tokio/rt` — `HFClientSync` "manages a dedicated tokio runtime
  internally, so callers do not need their own" (from the current README). The pure-`ureq`,
  zero-tokio blocking path from 0.4.x/0.5.x **is gone**.
- The Windows cache-copy behavior was made **explicit and unconditional** rather than a
  privilege-detection fallback. From `hf-hub/src/cache/storage.rs` (`main`, verified by
  direct source read):

  ```rust
  /// On Windows, copies the blob instead of creating a symlink because symlinks
  /// require elevated privileges. This means `find_cached_etag` (which uses
  /// `read_link`) cannot determine the cached etag on Windows, effectively
  /// disabling conditional-request (If-None-Match) optimization.
  pub(crate) async fn create_pointer_symlink(...) {
      ...
      #[cfg(windows)]
      { std::fs::copy(&blob_path(...), &pointer)?; }
  }
  ```

  This is the single most load-bearing fact in this report for AudioGraph's angle: on the
  platform end users actually run, `hf-hub`'s flagship feature (dedup via content-addressed
  blobs) costs double the disk and buys nothing, because the cache-hit optimization it
  exists to enable can't read the pointer back.
- Structured `HFError` variants map cleanly to UX states: `AuthRequired` (401),
  `RepoNotFound`, `RevisionNotFound`, `Forbidden` (403 — this is what a gated-model access
  denial looks like), `RateLimited` (429), `Conflict` (409).
- Reverse-dependency check (crates.io, `hf-hub/reverse_dependencies`, 511 total dependents):
  the highest-download consumers still pin the pre-rewrite line — e.g. one dependent at
  28.1M downloads pins `^0.4.1` with `features=["ureq"]`, another major one at 2.8M pins
  `^0.5.0`. **Zero visible reverse dependents on `^1.0` at time of writing.** The rewrite is
  real but the ecosystem hasn't followed it yet.

**Net for AudioGraph**: if `hf-hub` is used at all, pin `0.5.x` with
`--no-default-features --features ureq,rustls-tls` for the resume/retry/HTTP primitive
*only*, and **bypass its cache/blob/symlink layer entirely** — call the lower-level download
primitive and write straight into AudioGraph's existing flat `models/<filename>` layout. Do
not adopt `1.x` yet; it is too new, drops the no-tokio blocking path, and its Windows cache
story is worse, not better, for a single-file-per-model app that gets no benefit from
cross-revision blob dedup in the first place (AudioGraph downloads a whisper model *once*
per install, not across shifting revisions of a fine-tune line the way ML researchers do).

## 2. Alternative: plain `reqwest` + manifest (what AudioGraph already does, mostly)

`src-tauri/src/models/mod.rs` (1,398 lines) is a hand-written `reqwest::blocking` downloader
with a `ModelDef` const table (`MODELS: &[ModelDef]`, no external file). Verified directly
from source:

- **Timeouts are already handled correctly**: `build_download_client()` sets a 10s connect
  timeout and 300s read timeout — `reqwest::blocking::Client::new()` alone has no timeouts at
  all, so this existing guard is real defense against a dead/stalling host (their own comment
  cites this as "(P4)").
- **No checksum verification anywhere in the file.** `verify_model_file()` only checks
  non-zero size and, if `expected_size` is `Some`, a **1% size tolerance**. Archive/component
  models (`verify_archive_dir`) check "file exists and is non-empty," nothing more. A
  same-sized-but-different file (e.g. a compromised mirror, or an upstream author silently
  replacing a file at the same path) would pass validation undetected.
- **No revision pinning.** Every HF-hosted URL in the table is a floating branch reference:
  `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-*.bin`, same pattern for
  the LLM (`LiquidAI/LFM2-350M-Extract-GGUF/resolve/main/...`) and Sortformer
  (`altunenes/parakeet-rs/resolve/main/...`). GitHub-release-hosted models (sherpa-onnx
  Zipformer, pyannote segmentation, TitaNet) are safer by construction because GitHub release
  tags are immutable, but the HF ones are not.
- **No resume support.** A dropped connection restarts the whole multi-GB download from
  byte zero; there's a read timeout to fail fast, but nothing that persists partial bytes or
  issues a `Range` request on retry.
- **Storage location is `app.path().app_data_dir()`** → on Windows this is
  `{FOLDERID_RoamingAppData}` (i.e. `%APPDATA%`), confirmed against the current Tauri v2
  `path` API docs (`localDataDir()` docs explicitly distinguish `{FOLDERID_LocalAppData}`
  from `appDataDir()`'s `{FOLDERID_RoamingAppData}`). Roaming profiles are the Windows
  Server/AD-joined-enterprise default sync unit; multi-GB model files sitting there get
  copied over the network at every logon/logoff in that environment. `app_local_data_dir()`
  (→ `%LOCALAPPDATA%`) is the correct call for large, machine-local, non-roaming payloads —
  this is a one-line fix with real operational payoff for any AudioGraph deployment inside a
  managed-Windows org.
- **No external manifest.** Every model definition (`ModelDef`) — URL, filename, expected
  size, required-files list — is a Rust `const` compiled into the binary. Fixing a wrong URL
  or a changed upstream file size (this already happened: see the LFM2 casing comment at
  `models/mod.rs:83-85`, "the lowercase form 404s") requires a full app rebuild and release.
  This is the concrete cost of not having a data-only manifest layer.

## 3. Registry precedents: Ollama, LM Studio, Jan

**Ollama** — genuinely content-addressed, OCI-registry-inspired, and it actually works on
Windows (verified via multiple current how-tos, 2026): `~/.ollama/models/{manifests,blobs}`
on macOS/Linux, `C:\Users\<user>\.ollama\models` on Windows (note: **not** `%APPDATA%` — a
plain home-directory dotfile convention, unusual for Windows but functional), overridable via
`OLLAMA_MODELS` env var. A manifest per `model:tag` is small JSON mapping named layers
(weights, license, template, params) to `sha256-...` digests; blobs live flat in `blobs/` by
digest. Real dedup: `llama3.2:latest` and `llama3.2:1b` share license/template blobs and skip
re-downloading them. This is the right *shape* (manifest layer ≠ payload layer, content
addressing for real sharing) but it's overkill for AudioGraph's use case — AudioGraph has ~10
fixed, non-variant models, not an open catalog of thousands of tag combinations that
benefit from cross-model blob sharing.

**LM Studio** — deliberately mirrors HF's own `publisher/model/model-file.gguf` directory
tree under `~/.lmstudio/models/` (verified against `lmstudio.ai/docs/app/advanced/import-model`,
current as of the fetch), with **no content-addressing or dedup at all** — flat filesystem
mirror, one real copy per file, which is simpler to reason about and to manually back up
(literally document-visible, no digest indirection). Notable field-reported failure mode
(GitHub `lmstudio-ai/lmstudio-bug-tracker#686`, filed against 0.3.16): LM Studio actually
maintains **two separate, inconsistently-rooted directory trees** for model files vs. small
per-model metadata, and only one of the two respects the user's "Model Directory" setting —
a concrete cautionary precedent for "don't split one logical model's files across two
independently-configured roots."

**Jan** — data dir resolves the same way Tauri's own `app_data_dir` does per-platform
(`%APPDATA%\Jan\data` on Windows per official docs; `~/Library/Application Support/Jan/data`
macOS; XDG on Linux), with per-model storage as
`models/<publisher>/<model>/{model.yml, model.gguf}` — a small YAML sidecar manifest
co-located with the actual weight file. This sidecar-manifest pattern (one small metadata
file next to each model payload, not one giant central manifest) is the most directly
applicable precedent for AudioGraph: it keeps per-model provenance (source URL, revision,
checksum, license-acceptance record) physically next to the file it describes, so deleting a
model directory cleanly removes its metadata too, and a corrupted/missing sidecar is an
unambiguous integrity signal.

**Cross-cutting observation**: none of Ollama, LM Studio, or Jan implement delta/diff updates
for model weights. Every one of them treats a model file as an atomic blob — new
quantization or fine-tune = new full download, old one deleted or kept side-by-side. Building
bespoke binary-diff model updates for AudioGraph would be solving a problem no incumbent in
this space has bothered to solve; not recommended.

## 4. Quantization/file formats and their loaders

- **GGUF** (used today for AudioGraph's Whisper via `whisper-rs`/whisper.cpp, and the LFM2
  extraction model via presumably `llama-cpp-2`): single self-contained file with a KV
  metadata header + tensor data, the simplest format to verify (one checksum, one file) and
  already what AudioGraph juggles least awkwardly. For models too large for one file,
  `llama.cpp`'s `gguf-split` produces `model-NNNNN-of-MMMMM.gguf` shards that
  `llama_model_loader` auto-detects and reassembles at load time (verified against
  `ggml-org/llama.cpp` discussion #6404, current) — irrelevant at AudioGraph's model sizes
  (largest today is Whisper large-v3 at ~3GB, well under any practical single-file
  ceiling) but worth knowing if a future large multilingual model needs it.
- **ONNX + external data** (used today for Sortformer, sherpa-onnx Zipformer, pyannote
  segmentation, Moonshine streaming components — all `.onnx`/`.ort` files): ONNX's
  serialization is protobuf, which has a **hard 2GB message-size ceiling**; anything larger
  ships as `model.onnx` (graph) + one or more `model.onnx.data` sidecar files (raw tensor
  bytes), verified against current `onnx.ai` docs and `onnxruntime.ai` large-model docs. This
  is exactly the multi-file pattern AudioGraph's own `component_required_files` /
  `archive_required_files` machinery in `models/mod.rs` already generalizes over (Moonshine's
  8-file `.ort` component set, the Zipformer 4-file archive) — the existing abstraction is
  the right one, it just needs a checksum-per-file addition, not a redesign.
- **safetensors**: single-file `model.safetensors`, or sharded
  `model-NNNNN-of-MMMMM.safetensors` + a `model.safetensors.index.json` mapping tensor names
  to shard files (verified against current HF docs/forum threads). AudioGraph doesn't consume
  safetensors directly today (no candle/transformers-style loader in the stack per the audit
  companion note); only relevant if a future embedding/reranker model ships safetensors-only.
- **CTranslate2 (CT2)**: directory-based (`model.bin` + `config.json` + vocab files), the
  format behind `faster-whisper`. Rust bindings exist (`ct2rs`,
  `github.com/jkawamoto/ctranslate2-rs`, max version **0.10.0**, ~49k crates.io downloads,
  single-maintainer) but this is a thin, still-0.x wrapper around the CTranslate2 C++
  library — materially less mature than the already-integrated `whisper-rs`/`sherpa-onnx`
  paths. Not worth adding as a third inference backend purely for a distribution-format
  reason.

## 5. Gated/licensed models: pyannote as the concrete case

`pyannote/speaker-diarization-3.1` and `pyannote/segmentation-3.0` on the HF Hub are
**gated**: MIT-licensed once accepted, free for commercial use, but access requires (a) a
logged-in HF account, (b) clicking "Agree" on the model card in a browser, and (c) an HF
token with read access presented on every download (verified against current pyannote
FAQ/vendor writeups and live HF forum threads on `GatedRepoError`/401 flows). There is **no
API-only path to accept a gate on a user's behalf** — a background download manager cannot
silently satisfy this; it is fundamentally a human-in-the-loop, browser-mediated consent
step, and unauthenticated or unaccepted requests come back as a clean 403 (`HFError::Forbidden`
in the new `hf-hub`, generic HTTP 403 from plain `reqwest`).

AudioGraph has already made the right structural choice here without necessarily framing it
this way: `DIAR_SEG_PYANNOTE_URL` points at
`k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/...` — a GitHub-release
re-hosting of an ONNX conversion of pyannote segmentation-3.0, **not** the gated
`huggingface.co/pyannote/...` repo directly. This sidesteps the entire gating/consent UX
problem for end users, at the cost of trusting the sherpa-onnx maintainers' redistribution
(itself MIT-compatible, and a widely-used, well-known project). **Recommendation: keep this
pattern** — never route an end user through HF's gate-and-token flow for a model that has an
ungated, license-compatible re-hosted ONNX export available; reserve any future
gated-repo-direct integration (e.g. if AudioGraph wanted the full pyannote 3.1
speaker-embedding pipeline verbatim rather than sherpa-onnx's conversion) for a "bring your
own HF token" opt-in settings field with an explicit one-time consent screen, never a silent
background fetch.

One fresh (2026) pyannote-specific caution, filed as `pyannote/pyannote-audio#2044`: in
`pyannote.audio` **4.x**, the Python `SpeakerDiarization` pipeline unconditionally loads a
*third*, undocumented gated repo (`pyannote/speaker-diarization-community-1`) for a PLDA
transform it doesn't even use unless VBx clustering is selected — users who accepted exactly
the two repos the model card lists get an unexplained 403 on a repo name that doesn't
resemble anything they were told about. This doesn't affect AudioGraph's ONNX-export path
directly, but it's a live example of gating surface silently expanding across versions of an
upstream project — a reason not to hand-roll a "the user accepted these N repos, we're done"
assumption if AudioGraph ever integrates pyannote's own Python/PyTorch pipeline rather than a
frozen ONNX export.

## 6. Disk budget UX, storage location, first-run

- **Windows storage location**: use `app.path().app_local_data_dir()`
  (`%LOCALAPPDATA%\<bundle-id>`), not `app_data_dir()` (`%APPDATA%`, roaming). Verified
  against current Tauri v2 JS/Rust path API docs — `appDataDir()` documents
  `{FOLDERID_RoamingAppData}`, `appLocalDataDir()` documents `{FOLDERID_LocalAppData}`. This
  is a one-line change (`get_models_dir()` in `models/mod.rs:369-379`) with a real payoff:
  it stops multi-GB payloads from being subject to roaming-profile sync in managed-Windows/AD
  environments, and it matches what Ollama/LM Studio/Jan implicitly do by using a plain
  home-relative dotfile dir on Windows rather than the roaming store.
- **Per-model size disclosure**: every serious precedent (Jan's model cards, LM Studio's
  catalog, AudioGraph's own `ModelDef.description` strings) shows approximate size before
  download starts — AudioGraph already does this ("~75MB", "~1.5GB" in the `description`
  field); keep it, and surface it in the UI as a number pulled from the same struct that
  drives the progress bar, not a hand-maintained string that can drift from `expected_size`.
- **Install/uninstall granularity**: per-model, not all-or-nothing — AudioGraph's `ModelDef`
  table already supports this structurally (each model has its own filename/status); the gap
  is exposing an uninstall/delete affordance in the same UI surface that exposes install,
  with a disk-space-freed confirmation.
- **Low-disk warning**: none of the three precedents surfaced a sophisticated low-disk UX
  beyond "the download fails and you see an OS-level disk-full error." A reasonable minimum
  bar: check available disk space against the model's known size before starting a
  download (not after), and surface a specific "need 3.1GB, have 1.2GB free" message rather
  than letting a mid-download disk-full error look like a network failure.

## 7. First-run experience

The product constraint ("app must work before any model downloads") is best satisfied by
**bundling the smallest genuinely-useful local model as a Tauri resource**, not by a
cloud-fallback tier (this product is explicitly offline-first/local-first per the STT v2
brief, so standing up a cloud fallback purely to cover the gap before first download adds a
second, rarely-exercised code path for no strategic benefit). Candidates already in
AudioGraph's own model table, ranked by fit:

- **Sherpa Zipformer 20M streaming** (`SHERPA_ZIPFORMER_20M`, ~65MB extracted,
  "sub-200ms first-word latency" per its own description) — smallest, and unlike Whisper
  tiny it is *streaming*, which matches the live-meeting/live-graph latency requirement this
  whole research track is organized around. This is the strongest bundle candidate.
- **Whisper tiny.en** (~75MB) — smaller than base/small, but batch-only per the companion
  audit note (`event_semantics: TranscriptFinalOnly`), a worse fit for the "streaming
  feeds a live graph" framing even though it's a very common "smallest Whisper" default
  elsewhere.

Either is small relative to Tauri's own installer-size deltas — the WebView2
`offlineInstaller` bootstrapper alone adds ~127MB to a Windows installer (current Tauri v2
distribution docs), so a 65–75MB bundled ASR model is the same order of magnitude as a cost
AudioGraph may already be paying for WebView2 offline support, not a new order of magnitude.
Ship it via `tauri.conf.json`'s `bundle.resources` (verified current mechanism for
Tauri v2 static asset bundling — distinct from `externalBin`, which is for sidecar
*executables*, not data files) into a read-only resource path, and on first run, **copy**
(not symlink — Windows privilege story again) it into the same `%LOCALAPPDATA%/models/`
tree the download manager writes into, so there is exactly one code path that resolves "is
model X ready," whether X arrived via bundle-copy or network download.

## 8. Integrity and supply chain

Minimum bar, none of which AudioGraph does today:

- **SHA-256 checksum per file**, not size-tolerance. Cheap to compute (`sha2` crate, already
  a transitive dependency via `hf-hub` if that's ever added, but trivial to add standalone —
  it's a ubiquitous, zero-controversy crate) and it's the only check that actually detects
  "same size, different bytes" tampering or corruption. Store the expected digest in the
  manifest (see §9), not just in code.
- **Revision pinning for every HF URL**: replace `resolve/main/<file>` with
  `resolve/<commit-sha>/<file>`. HF resolves any full commit SHA the same way it resolves
  `main`, so this is a drop-in URL change, not a new capability to build. This closes the
  "upstream author silently changes what main points to" gap directly.
- **HTTPS-only, no arbitrary-URL execution**: already true — every URL is a hardcoded HTTPS
  constant, none are user-suppliable, and downloaded bytes are only ever treated as model
  weights/config, never executed. Keep the manifest itself (if externalized per §9) hosted
  over HTTPS with the same pinned-checksum discipline applied to *it*, so a compromised
  manifest host can't silently redirect models to attacker-controlled URLs.
- **Rate-limit-aware backoff for HF Hub specifically**: HF's documented resolver quota
  (`huggingface.co/docs/hub/main/en/rate-limits`, current) is **3,000 requests / 5-minute
  window for anonymous traffic, shared per source IP** (5,000 for a free authenticated user,
  scoped per-account rather than per-IP). This matters at fleet scale: many AudioGraph
  installs behind one corporate NAT egress IP updating/downloading around the same time
  could collectively exhaust the anonymous IP bucket and all see 429s with no
  attribution to any single user. The 429 response carries a `RateLimit` header with an
  exact retry-after duration — honor it precisely (both hf-hub generations already do this
  internally); do not roll a separate fixed-backoff scheme for HF specifically. An optional
  "bring your own free HF token" settings field is a legitimate defensive measure once
  AudioGraph has more than a handful of seats behind shared egress, not needed today.

## 9. Recommended manifest design

A small, versioned, externally-updatable JSON manifest (bundled as a Tauri resource **and**
periodically re-fetched from a pinned, checksummed URL so it can be updated without a full
app release — the concrete fix for the LFM2-URL-casing-required-a-rebuild problem already
visible in the current source):

```json
{
  "manifest_version": 1,
  "models": [
    {
      "id": "whisper-small-en",
      "kind": "single_file",
      "filename": "ggml-small.en.bin",
      "url": "https://huggingface.co/ggerganov/whisper.cpp/resolve/<pinned-sha>/ggml-small.en.bin",
      "sha256": "<64-hex-digest>",
      "size_bytes": 487654400,
      "license": "MIT",
      "gated": false
    }
  ]
}
```

For archive/component models, reuse the `required_files` idea already in `ModelDef`, with a
per-file digest map instead of a presence-only check. Sidecar-style provenance (Jan's
`model.yml`-next-to-`model.gguf` pattern) is worth adopting for the *record of what was
verified*, not the manifest itself: write a `<filename>.meta.json` next to each installed
model containing the digest that was actually verified and the manifest version it came
from, so a later integrity re-check doesn't need network access to know what to compare
against — this is also the offline-airgap story (§10).

## 10. Offline / airgap story

None of Ollama/LM Studio/Jan have a first-class "fully airgapped install" flow beyond
"manually copy files into the expected directory structure" (LM Studio's `lms import`,
Jan's "drop a GGUF into the models folder," Ollama's documented-but-manual blob+manifest
layout). AudioGraph should match that bar, not exceed it: support a **manual side-load**
path where a model file (matching a known manifest entry's checksum) dropped into the
models directory is picked up on next status check, with the same SHA-256 verification
applied as a network download would get — this makes "airgapped" just "verification without
a download step," not a separate code path. This is also the natural fallback when the
manifest-refresh network call itself fails: fall back to the manifest bundled in the
installer (already on disk from §7's Tauri-resource mechanism), never hard-fail first run
because a manifest CDN is unreachable.

## 11. Tauri-specific patterns

- **`bundle.resources`** (static files copied verbatim into the installed app tree) is the
  correct mechanism for bundling a small first-run model — distinct from
  **`bundle.externalBin`** (sidecar *executables*, resolved per-target-triple). A live
  Tauri issue (`tauri-apps/tauri#15134`, NSIS-specific) documents a real footgun in the
  `externalBin` path: the bundler can silently reuse a stale cached sidecar binary across
  rebuilds unless the source file's checksum (not just timestamp) is compared — irrelevant
  to a static resource file but worth knowing if AudioGraph ever ships a native
  helper process alongside the model.
- **No documented hard installer-size ceiling** for NSIS or WiX from Tauri itself; the
  practical constraints are download/install UX, not a format limit. Tauri's own size
  deltas for optional features are a useful yardstick: `embedBootstrapper` for WebView2 is
  ~1.8MB, `offlineInstaller` is ~127MB, `fixedVersion` is ~180MB (current
  `v2.tauri.app/distribute/windows-installer` docs) — bundling a 65–75MB first-run ASR model
  sits comfortably inside the range Tauri already treats as a normal trade-off for other
  offline-capability features.
- **Tauri's own updater** (`tauri-plugin-updater`, already a dependency per
  `src-tauri/Cargo.toml:362` referencing the plugin's `reqwest` feature) updates the *app
  binary*, not model payloads — do not try to route model updates through it; models need
  their own resumable, checksum-verified path independent of app version, since a model can
  be added/updated without an app release once the manifest is externalized per §9.

---

## Implications for AudioGraph

1. **Do not add `hf-hub` as a dependency yet.** It is five weeks old at the `1.0` API
   surface, has no ecosystem field mileage at that version, drops the no-tokio blocking path
   the rest of the Rust ML ecosystem still uses, and its Windows cache behavior (unconditional
   blob-copy, broken conditional-request path) provides zero benefit and real cost (2x disk)
   for AudioGraph's one-file-per-model usage pattern. If HF-hub's HTTP primitives (resume,
   retry, ETag) are wanted without the cache/blob machinery, either vendor the relevant
   ~150-line download-with-resume function or pin `hf-hub = { version = "0.5", default-features
   = false, features = ["ureq", "rustls-tls"] }` and call only `Api::model(repo).download()`
   equivalents, writing the result straight into AudioGraph's own flat directory — never let
   its cache module manage the on-disk layout.
2. **Two concrete, low-effort fixes to ship regardless of the above**:
   `get_models_dir()` should call `app.path().app_local_data_dir()` instead of
   `app_data_dir()` (Windows: `%LOCALAPPDATA%` not `%APPDATA%`), and every HF `resolve/main/`
   URL in `models/mod.rs` should be pinned to a commit SHA. Both are small diffs against the
   existing `ModelDef` table, not architecture changes.
3. **Add SHA-256 verification** to `verify_model_file()`/`verify_archive_dir()` alongside
   (not instead of) the existing size check — `sha2` is a small, uncontroversial dependency
   already pulled transitively by half the ML ecosystem.
4. **Externalize the model table into a versioned JSON manifest** (bundled as a Tauri
   resource, periodically re-fetched from a pinned/checksummed URL) so a wrong URL or a
   changed upstream artifact (the LFM2-casing incident already lived in this codebase) is a
   data update, not a binary release.
5. **Keep re-hosting gated models via ungated redistribution channels** (the sherpa-onnx
   GitHub-release pattern already used for pyannote segmentation) rather than routing end
   users through HF's token+consent gate; reserve direct gated-repo access for an explicit,
   opt-in "bring your own HF token" settings path if a future model requires it.
6. **Bundle Sherpa Zipformer 20M (~65MB) as a Tauri `resources` first-run model** rather than
   building a cloud-fallback tier — it's both the smallest model already in the table and the
   only bundle-sized one that's actually streaming, which matches the product's live-meeting
   latency framing better than a bundled Whisper tiny would.
7. **Do not build delta/diff model updates or a bespoke content-addressed dedup store.**
   No incumbent in this space (Ollama excepted, and only because it serves an open catalog of
   thousands of tag variants) has needed either at AudioGraph's scale of ~10 fixed models;
   building either would be solving a problem nobody in this space has actually had.
8. **Respect HF's documented per-IP anonymous rate limit (3,000 resolver requests / 5 min)**
   in the retry/backoff logic — read the `RateLimit` response header rather than guessing a
   fixed cooldown, and treat "many seats behind one corporate egress IP" as the realistic
   trigger condition, not "one user downloading too fast."
