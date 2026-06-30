//! Model management and downloading.
//!
//! Provides model listing, status checking, and HTTP-based downloading
//! with progress reporting via Tauri events. Replaces the old shell-script
//! based model setup with a cross-platform Rust implementation.

use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use crate::events::MODEL_DOWNLOAD_PROGRESS;

/// Minimum interval between `MODEL_DOWNLOAD_PROGRESS` events for an in-flight
/// download. A chunked HTTP read can fire tens of thousands of times per
/// second on a fast link; emitting at 1 Hz is plenty for a human-readable ETA
/// and keeps the IPC channel from being overwhelmed.
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(1000);

/// How long to wait for the TCP connect + TLS handshake before giving up. A
/// dead/unreachable host should fail fast rather than hang the download thread.
const DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Overall read timeout for the streaming body. Generous because models are
/// large (multi-GB Whisper variants on slow links), but bounded so a server
/// that accepts the connection and then stalls mid-stream cannot wedge the
/// download thread forever (P4).
const DOWNLOAD_READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Build the blocking HTTP client used for every model download.
///
/// `reqwest::blocking::Client::new()` has **no** timeouts, so a host that
/// accepts the TCP connection and then never sends bytes hangs the download
/// thread indefinitely (P4). We pin a connect timeout (fast-fail on dead
/// hosts) and an overall read timeout (bounded stall tolerance). Falls back to
/// the default client only if the builder somehow fails, which is unreachable
/// in practice but keeps the call sites infallible.
fn build_download_client() -> reqwest::blocking::Client {
    reqwest::blocking::Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_READ_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::blocking::Client::new())
}

// ---------------------------------------------------------------------------
// Model definitions
// ---------------------------------------------------------------------------

/// Internal model definition with expected sizes for verification.
struct ModelDef {
    name: &'static str,
    filename: &'static str,
    url: &'static str,
    expected_size: Option<u64>, // bytes, with 1% tolerance
    description: &'static str,
    /// When `Some`, this model ships as a `.tar.bz2` archive that extracts into a
    /// directory named `filename`; the slice lists the files that MUST exist
    /// inside it for the model to be considered valid. When `None`, the model is
    /// a single bare file downloaded directly to `filename` (size-verified).
    /// Generalizes the archive path so both the Zipformer ASR model and the
    /// pyannote diarization segmentation model (ADR-0017) share one downloader.
    archive_required_files: Option<&'static [&'static str]>,
    /// When `Some`, this model is a directory assembled from individually
    /// downloaded components under `url`. Moonshine streaming models use this:
    /// each required file lives at `{url}/{component}` and the directory is
    /// valid only when every component is present and non-empty.
    component_required_files: Option<&'static [&'static str]>,
}

pub const WHISPER_MODEL_TINY_EN: &str = "ggml-tiny.en.bin";
pub const WHISPER_MODEL_BASE_EN: &str = "ggml-base.en.bin";
pub const WHISPER_MODEL_SMALL_EN: &str = "ggml-small.en.bin";
pub const WHISPER_MODEL_MEDIUM_EN: &str = "ggml-medium.en.bin";
pub const WHISPER_MODEL_LARGE_V3: &str = "ggml-large-v3.bin";

/// Public so that commands can reference the canonical LLM model filename.
pub const LLM_MODEL_FILENAME: &str = "lfm2-350m-extract-q4_k_m.gguf";
// NOTE: HuggingFace paths are case-sensitive — the published asset is
// `LFM2-350M-Extract-Q4_K_M.gguf` (capitalized). The lowercase form 404s. The
// local on-disk filename above stays lowercase by convention; only the remote
// URL must match the published casing.
const LLM_MODEL_URL: &str = "https://huggingface.co/LiquidAI/LFM2-350M-Extract-GGUF/resolve/main/LFM2-350M-Extract-Q4_K_M.gguf";
const LLM_EXPECTED_SIZE: u64 = 229_000_000; // ~218MB Q4_K_M

const SORTFORMER_MODEL_URL: &str = "https://huggingface.co/altunenes/parakeet-rs/resolve/main/diar_streaming_sortformer_4spk-v2.onnx";
/// Public: canonical Sortformer ONNX model filename for diarization.
pub const SORTFORMER_MODEL_FILENAME: &str = "diar_streaming_sortformer_4spk-v2.onnx";
const SORTFORMER_EXPECTED_SIZE: u64 = 31_500_000; // ~30MB

/// Sherpa-onnx streaming Zipformer model directory name.
pub const SHERPA_ZIPFORMER_20M: &str = "streaming-zipformer-en-20M";
/// Sherpa-onnx Zipformer model archive URL (GitHub releases).
const SHERPA_ZIPFORMER_20M_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/sherpa-onnx-streaming-zipformer-en-20M-2023-02-17.tar.bz2";
/// Expected archive size (~20MB compressed, ~65MB extracted).
const SHERPA_ZIPFORMER_20M_EXPECTED_SIZE: u64 = 65_000_000;
/// Runtime files required by the Sherpa Zipformer streaming ASR worker.
///
/// Keep this as the single source of truth for model validation and capture
/// preflight; `asr::sherpa_streaming` opens these exact filenames.
pub const SHERPA_ZIPFORMER_REQUIRED_FILES: &[&str] = &[
    "encoder-epoch-99-avg-1.onnx",
    "decoder-epoch-99-avg-1.onnx",
    "joiner-epoch-99-avg-1.onnx",
    "tokens.txt",
];

pub const MOONSHINE_TINY_STREAMING_EN: &str = "moonshine-tiny-streaming-en";
pub const MOONSHINE_SMALL_STREAMING_EN: &str = "moonshine-small-streaming-en";
pub const MOONSHINE_MEDIUM_STREAMING_EN: &str = "moonshine-medium-streaming-en";
pub const MOONSHINE_STREAMING_REQUIRED_FILES: &[&str] = &[
    "adapter.ort",
    "cross_kv.ort",
    "decoder_kv.ort",
    "decoder_kv_with_attention.ort",
    "encoder.ort",
    "frontend.ort",
    "streaming_config.json",
    "tokenizer.bin",
];
const MOONSHINE_TINY_STREAMING_EN_URL: &str =
    "https://download.moonshine.ai/model/tiny-streaming-en/quantized";
const MOONSHINE_SMALL_STREAMING_EN_URL: &str =
    "https://download.moonshine.ai/model/small-streaming-en/quantized";
const MOONSHINE_MEDIUM_STREAMING_EN_URL: &str =
    "https://download.moonshine.ai/model/medium-streaming-en/quantized";

// --- Clustering diarization models (ADR-0017 / B16, `diarization-clustering`) -
// Unbounded-speaker diarization needs a pyannote segmentation model + a speaker
// embedding model (URLs verified 200 on 2026-05-30, see
// docs/research/b16-diarization-live-rust-impl.md §4). The segmentation model
// ships as a .tar.bz2 (extracted via the generalized archive path); the
// embedding model is a bare .onnx (direct download, like Sortformer). Both are
// registered in MODELS below; the canonical references live here so the
// downloader + the `ClusteringDiarizer::new(seg, emb, threshold)` call agree on
// one source.

/// pyannote segmentation-3.0 (ONNX), extracted directory name.
pub const DIAR_SEG_PYANNOTE_DIR: &str = "sherpa-onnx-pyannote-segmentation-3-0";
/// pyannote segmentation-3.0 archive URL (k2-fsa GitHub releases). MIT licensed.
const DIAR_SEG_PYANNOTE_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-segmentation-models/sherpa-onnx-pyannote-segmentation-3-0.tar.bz2";
/// Preferred model file inside the archive (int8 — ~4x faster, marginal accuracy
/// cost; `model.onnx` fp32 is also present).
pub const DIAR_SEG_PYANNOTE_FILE: &str = "model.int8.onnx";
const DIAR_SEG_PYANNOTE_REQUIRED_FILES: &[&str] = &["model.onnx", "model.int8.onnx"];

/// Speaker embedding model filename (NeMo TitaNet-small, 16 kHz; fast, dim=192).
pub const DIAR_EMB_TITANET_FILENAME: &str = "nemo_en_titanet_small.onnx";
/// NeMo TitaNet-small embedding model URL (k2-fsa GitHub releases; the upstream
/// tag literally spells "recongition").
const DIAR_EMB_TITANET_URL: &str = "https://github.com/k2-fsa/sherpa-onnx/releases/download/speaker-recongition-models/nemo_en_titanet_small.onnx";
/// Minimum acceptable size for the TitaNet embedding `.onnx` (BUG 3f23).
///
/// The published `nemo_en_titanet_small.onnx` is ~38 MB; we don't pin an exact
/// `expected_size` because the upstream release carries no published SHA/size
/// and a tight tolerance would reject a legitimately re-published model. A
/// minimum-size floor is the least-surprising guard: a truncated/interrupted
/// download or an HTML error page is bytes-to-KB, far below this floor, while
/// any real model clears it comfortably. Readiness reports such a file as
/// invalid (not ready) instead of waving it through to a runtime ONNX load
/// failure. 8 MiB leaves a wide margin under the real ~38 MB size.
pub const DIAR_EMB_TITANET_MIN_BYTES: u64 = 8 * 1024 * 1024;

/// Minimum acceptable on-disk size, in bytes, for a bare-file local model,
/// keyed by its filename. `None` means "non-empty is sufficient" (no published
/// size to verify against).
///
/// This is the size floor the descriptor-readiness check consults so a
/// truncated file fails readiness with a clear reason rather than passing the
/// `len() > 0` check and deferring the failure to a runtime model load (BUG
/// 3f23). Only models with a meaningful, stable lower bound are listed.
pub fn min_model_size_bytes(filename: &str) -> Option<u64> {
    match filename {
        DIAR_EMB_TITANET_FILENAME => Some(DIAR_EMB_TITANET_MIN_BYTES),
        _ => None,
    }
}

const MODELS: &[ModelDef] = &[
    ModelDef {
        name: "Whisper Tiny (English)",
        filename: WHISPER_MODEL_TINY_EN,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin",
        expected_size: Some(77_700_000),
        description: "Fastest model (~75MB). 5x faster than Small, lower accuracy. Good for weak hardware.",
        archive_required_files: None,
        component_required_files: None,
    },
    ModelDef {
        name: "Whisper Base (English)",
        filename: WHISPER_MODEL_BASE_EN,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
        expected_size: Some(147_500_000),
        description: "Best real-time balance (~142MB). 2-3x faster than Small on Apple Silicon.",
        archive_required_files: None,
        component_required_files: None,
    },
    ModelDef {
        name: "Whisper Small (English)",
        filename: WHISPER_MODEL_SMALL_EN,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin",
        expected_size: Some(487_654_400),
        description: "Default model (~466MB). Good accuracy/speed balance.",
        archive_required_files: None,
        component_required_files: None,
    },
    ModelDef {
        name: "Whisper Medium (English)",
        filename: WHISPER_MODEL_MEDIUM_EN,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.en.bin",
        expected_size: Some(1_533_800_000),
        description: "High accuracy (~1.5GB). Requires strong GPU for real-time.",
        archive_required_files: None,
        component_required_files: None,
    },
    ModelDef {
        name: "Whisper Large v3 (Multilingual)",
        filename: WHISPER_MODEL_LARGE_V3,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
        expected_size: Some(3_094_600_000),
        description: "Best accuracy (~3GB). Multilingual. Requires powerful GPU.",
        archive_required_files: None,
        component_required_files: None,
    },
    ModelDef {
        name: "LFM2-350M Extract (Entity Extraction)",
        filename: LLM_MODEL_FILENAME,
        url: LLM_MODEL_URL,
        expected_size: Some(LLM_EXPECTED_SIZE),
        description: "Small language model for entity and relationship extraction",
        archive_required_files: None,
        component_required_files: None,
    },
    ModelDef {
        name: "Sortformer v2 (Speaker Diarization)",
        filename: SORTFORMER_MODEL_FILENAME,
        url: SORTFORMER_MODEL_URL,
        expected_size: Some(SORTFORMER_EXPECTED_SIZE),
        description: "Streaming speaker diarization — up to 4 speakers (NVIDIA Sortformer ONNX)",
        archive_required_files: None,
        component_required_files: None,
    },
    ModelDef {
        name: "Sherpa Zipformer 20M (Streaming ASR)",
        filename: SHERPA_ZIPFORMER_20M,
        url: SHERPA_ZIPFORMER_20M_URL,
        expected_size: Some(SHERPA_ZIPFORMER_20M_EXPECTED_SIZE),
        description: "Streaming ASR via Zipformer transducer — sub-200ms first-word latency (sherpa-onnx)",
        archive_required_files: Some(SHERPA_ZIPFORMER_REQUIRED_FILES),
        component_required_files: None,
    },
    ModelDef {
        name: "Moonshine Tiny Streaming (English)",
        filename: MOONSHINE_TINY_STREAMING_EN,
        url: MOONSHINE_TINY_STREAMING_EN_URL,
        expected_size: None,
        description: "Low-resource Moonshine Voice streaming ASR model assembled from native C API component files",
        archive_required_files: None,
        component_required_files: Some(MOONSHINE_STREAMING_REQUIRED_FILES),
    },
    ModelDef {
        name: "Moonshine Small Streaming (English)",
        filename: MOONSHINE_SMALL_STREAMING_EN,
        url: MOONSHINE_SMALL_STREAMING_EN_URL,
        expected_size: None,
        description: "Default Moonshine Voice streaming ASR model assembled from native C API component files",
        archive_required_files: None,
        component_required_files: Some(MOONSHINE_STREAMING_REQUIRED_FILES),
    },
    ModelDef {
        name: "Moonshine Medium Streaming (English)",
        filename: MOONSHINE_MEDIUM_STREAMING_EN,
        url: MOONSHINE_MEDIUM_STREAMING_EN_URL,
        expected_size: None,
        description: "Higher-accuracy Moonshine Voice streaming ASR model assembled from native C API component files",
        archive_required_files: None,
        component_required_files: Some(MOONSHINE_STREAMING_REQUIRED_FILES),
    },
    // --- Clustering diarization (ADR-0017 / B16, `diarization-clustering`) ---
    ModelDef {
        name: "Pyannote Segmentation 3.0 (Clustering Diarization)",
        filename: DIAR_SEG_PYANNOTE_DIR,
        url: DIAR_SEG_PYANNOTE_URL,
        // No published SHA/size; the archive verifier only checks the required
        // files exist, so size verification is irrelevant for the archive itself.
        expected_size: None,
        description: "Speaker-segmentation model for unbounded clustering diarization (pyannote-3.0, MIT)",
        archive_required_files: Some(DIAR_SEG_PYANNOTE_REQUIRED_FILES),
        component_required_files: None,
    },
    ModelDef {
        name: "NeMo TitaNet-small (Speaker Embedding)",
        filename: DIAR_EMB_TITANET_FILENAME,
        url: DIAR_EMB_TITANET_URL,
        // No published size — verifier falls back to a non-empty check.
        expected_size: None,
        description: "Speaker-embedding model for unbounded clustering diarization (NeMo TitaNet-small, 16 kHz, dim 192)",
        archive_required_files: None,
        component_required_files: None,
    },
];

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Information about a downloadable model.
#[derive(Debug, Clone, Serialize)]
pub struct ModelInfo {
    pub name: String,
    pub filename: String,
    pub url: String,
    pub size_bytes: Option<u64>,
    pub is_downloaded: bool,
    pub is_valid: bool,
    pub local_path: Option<String>,
    pub description: String,
}

/// Progress event payload emitted during model downloads.
///
/// `total_bytes` is `0` when the server didn't send a `Content-Length` header
/// — the frontend must treat that as "unknown" and skip ETA computation.
/// `elapsed_ms` measures wall time from the start of the download so the
/// frontend can compute a running ETA.
#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    /// Stable identifier — the model filename (e.g. `ggml-small.en.bin`).
    pub model_id: String,
    /// Human-readable name kept for display-side consumers that already key
    /// off the friendly name (legacy compatibility).
    pub model_name: String,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
    pub elapsed_ms: u64,
    pub percent: f32,
    /// One of: "downloading", "complete", "error"
    pub status: String,
}

/// Readiness state for a single model.
#[derive(Debug, Clone, Serialize)]
pub enum ModelReadiness {
    Ready,
    NotDownloaded,
    /// File exists but wrong size (possibly corrupt or incomplete).
    Invalid,
}

/// Aggregated status of all required models.
#[derive(Debug, Clone, Serialize)]
pub struct ModelStatus {
    pub whisper: ModelReadiness,
    pub llm: ModelReadiness,
    pub sortformer: ModelReadiness,
}

// ---------------------------------------------------------------------------
// Directory resolution (G6)
// ---------------------------------------------------------------------------

/// Return the directory where models are stored.
///
/// Resolves relative to Tauri's app data directory for a stable,
/// platform-appropriate location. Creates the directory if it doesn't exist.
pub fn get_models_dir(app: &AppHandle) -> PathBuf {
    let base = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| PathBuf::from("."));
    let dir = base.join("models");
    if !dir.exists() {
        let _ = fs::create_dir_all(&dir);
    }
    dir
}

// ---------------------------------------------------------------------------
// Verification (G5)
// ---------------------------------------------------------------------------

/// Verify a model file exists and has approximately the expected size.
///
/// Returns `true` if the file exists, is non-empty, and (if an expected size
/// is given) is within 1% of the expected size.
fn verify_model_file(path: &Path, expected_size: Option<u64>) -> bool {
    if let Ok(metadata) = fs::metadata(path) {
        let size = metadata.len();
        if size == 0 {
            return false;
        }
        if let Some(expected) = expected_size {
            let tolerance = expected / 100; // 1%
            size >= expected.saturating_sub(tolerance) && size <= expected + tolerance
        } else {
            true // No expected size, just check non-empty
        }
    } else {
        false
    }
}

/// Verify an extracted archive directory contains all of `required_files`,
/// each present as a **non-empty** regular file.
///
/// A zero-byte `model.onnx` / `tokens.txt` is a corrupt extraction (e.g. a
/// truncated/interrupted unpack), not a ready model — `is_file()` alone would
/// wave it through and defer the failure to runtime model load. We require a
/// positive byte length so `list_models` never reports such a directory ready.
fn verify_archive_dir(path: &Path, required_files: &[&str]) -> bool {
    path.is_dir()
        && required_files.iter().all(|file| {
            fs::metadata(path.join(file))
                .map(|m| m.is_file() && m.len() > 0)
                .unwrap_or(false)
        })
}

fn model_exists_and_is_valid(path: &Path, def: &ModelDef) -> (bool, bool) {
    if let Some(required) = def.archive_required_files {
        let exists = path.exists();
        return (exists, exists && verify_archive_dir(path, required));
    }
    if let Some(required) = def.component_required_files {
        let exists = path.exists();
        return (exists, exists && verify_archive_dir(path, required));
    }

    let exists = path.exists();
    (exists, exists && verify_model_file(path, def.expected_size))
}

/// Check readiness of a single model file.
fn check_model_readiness(
    models_dir: &Path,
    filename: &str,
    expected_size: Option<u64>,
) -> ModelReadiness {
    let path = models_dir.join(filename);
    if !path.exists() {
        ModelReadiness::NotDownloaded
    } else if verify_model_file(&path, expected_size) {
        ModelReadiness::Ready
    } else {
        ModelReadiness::Invalid
    }
}

// ---------------------------------------------------------------------------
// Status (G1)
// ---------------------------------------------------------------------------

/// Get the readiness status of all known models.
pub fn get_model_status(app: &AppHandle) -> ModelStatus {
    let dir = get_models_dir(app);
    ModelStatus {
        whisper: check_model_readiness(&dir, WHISPER_MODEL_SMALL_EN, Some(487_654_400)),
        llm: check_model_readiness(&dir, LLM_MODEL_FILENAME, Some(LLM_EXPECTED_SIZE)),
        sortformer: check_model_readiness(
            &dir,
            SORTFORMER_MODEL_FILENAME,
            Some(SORTFORMER_EXPECTED_SIZE),
        ),
    }
}

// ---------------------------------------------------------------------------
// Listing
// ---------------------------------------------------------------------------

/// List all known models and their download/validation status.
pub fn list_models(app: &AppHandle) -> Vec<ModelInfo> {
    let models_dir = get_models_dir(app);

    MODELS
        .iter()
        .map(|def| {
            let path = models_dir.join(def.filename);
            let (exists, valid) = model_exists_and_is_valid(&path, def);
            ModelInfo {
                name: def.name.to_string(),
                filename: def.filename.to_string(),
                url: def.url.to_string(),
                size_bytes: def.expected_size,
                is_downloaded: exists,
                is_valid: valid,
                local_path: if exists {
                    Some(path.to_string_lossy().to_string())
                } else {
                    None
                },
                description: def.description.to_string(),
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Download
// ---------------------------------------------------------------------------

/// Time-based gate that limits how often progress events are emitted.
///
/// A byte-count heuristic ("emit every 1 MB") is unreliable: fast links can
/// exceed 1 MB/tick but slow links may crawl for many seconds without ever
/// hitting the threshold, so the UI goes silent. Gating on wall time gives
/// the frontend a steady cadence regardless of throughput.
struct ProgressThrottle {
    interval: Duration,
    last_emit: Option<Instant>,
}

impl ProgressThrottle {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_emit: None,
        }
    }

    /// Returns true if enough time has elapsed since the previous emit (or if
    /// no emit has happened yet). Records `now` as the most recent emit when
    /// it returns true.
    fn should_emit(&mut self, now: Instant) -> bool {
        let emit = match self.last_emit {
            None => true,
            Some(last) => now.duration_since(last) >= self.interval,
        };
        if emit {
            self.last_emit = Some(now);
        }
        emit
    }
}

/// Compute the progress percent from byte counters. Returns `0.0` when
/// `total` is `0` (unknown size) — the frontend renders "N MB" instead of a
/// percentage in that case.
fn compute_percent(downloaded: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (downloaded as f32 / total as f32) * 100.0
    }
}

/// Build a `DownloadProgress` snapshot. Extracted so the payload shape can be
/// unit-tested without touching the HTTP or Tauri layers.
fn build_progress(
    def: &ModelDef,
    downloaded: u64,
    total: u64,
    elapsed: Duration,
    status: &str,
) -> DownloadProgress {
    DownloadProgress {
        model_id: def.filename.to_string(),
        model_name: def.name.to_string(),
        bytes_downloaded: downloaded,
        total_bytes: total,
        elapsed_ms: elapsed.as_millis() as u64,
        percent: compute_percent(downloaded, total),
        status: status.to_string(),
    }
}

/// Download a model file by filename with progress reporting via Tauri events.
///
/// Looks up the model definition by filename. If the file already exists and
/// is valid, returns its path immediately. Otherwise performs a blocking HTTP
/// download, emitting `model-download-progress` events at most once per
/// second (plus a final event on completion or error). Each event includes
/// `elapsed_ms` so the frontend can compute `(total - downloaded) * elapsed /
/// downloaded` as an ETA.
pub fn download_model(app: &AppHandle, filename: &str) -> Result<String, String> {
    use tauri::Emitter;

    let def = MODELS
        .iter()
        .find(|m| m.filename == filename)
        .ok_or_else(|| format!("Unknown model filename: {}", filename))?;

    let models_dir = get_models_dir(app);
    let target_path = models_dir.join(filename);

    if let Some(required) = def.component_required_files {
        return download_component_directory_model(app, def, required, &models_dir, &target_path);
    }

    if let Some(required) = def.archive_required_files {
        return download_archive_model(app, def, required, &models_dir, &target_path);
    }

    if target_path.exists() && verify_model_file(&target_path, def.expected_size) {
        return Ok(target_path.to_string_lossy().to_string());
    }

    if target_path.exists() {
        let _ = fs::remove_file(&target_path);
    }

    // Download to a sibling `.download` temp file and rename onto `target_path`
    // only AFTER verification (P3). Writing straight to `target_path` means a
    // kill mid-download leaves a truncated file that, for `expected_size:None`
    // models, passes `verify_model_file` and is reported ready. The archive
    // path already uses this temp+rename idiom; mirror it here.
    let download_path = models_dir.join(format!("{}.download", filename));
    if download_path.exists() {
        let _ = fs::remove_file(&download_path);
    }

    let client = build_download_client();
    let response = client
        .get(def.url)
        .send()
        .map_err(|e| format!("Download failed: {}", e))?
        // A 404/403/HTML error page would otherwise stream into the file and
        // (for `expected_size:None` models) pass verification as a ready model
        // (P1). Reject any non-2xx status immediately.
        .error_for_status()
        .map_err(|e| format!("Download failed: {}", e))?;

    // `content_length()` is `None` when the server omits `Content-Length`.
    // We encode that as `0` on the wire so the payload type stays a plain
    // `u64` and the frontend can branch on `total_bytes === 0`.
    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;

    let mut file =
        fs::File::create(&download_path).map_err(|e| format!("Failed to create file: {}", e))?;

    let mut reader = response;
    let mut buffer = vec![0u8; 8192];

    let start = Instant::now();
    let mut throttle = ProgressThrottle::new(PROGRESS_EMIT_INTERVAL);

    loop {
        let bytes_read = match std::io::Read::read(&mut reader, &mut buffer) {
            Ok(n) => n,
            Err(e) => {
                let err_msg = format!("Read error: {}", e);
                let _ = fs::remove_file(&download_path);
                let progress =
                    build_progress(def, downloaded, total_size, start.elapsed(), "error");
                let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);
                return Err(err_msg);
            }
        };
        if bytes_read == 0 {
            break;
        }

        file.write_all(&buffer[..bytes_read])
            .map_err(|e| format!("Write error: {}", e))?;

        downloaded += bytes_read as u64;

        if throttle.should_emit(Instant::now()) {
            let progress =
                build_progress(def, downloaded, total_size, start.elapsed(), "downloading");
            let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);
        }
    }
    // Ensure all buffered bytes hit disk before we verify size.
    drop(file);

    if !verify_model_file(&download_path, def.expected_size) {
        let actual_size = fs::metadata(&download_path).map(|m| m.len()).unwrap_or(0);
        let _ = fs::remove_file(&download_path);
        let err_msg = format!(
            "Download verification failed for '{}': got {} bytes, expected ~{:?} bytes",
            filename, actual_size, def.expected_size
        );
        let progress = build_progress(def, downloaded, total_size, start.elapsed(), "error");
        let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);
        return Err(err_msg);
    }

    // Atomic install: rename the verified temp file onto the canonical path so
    // a concurrent reader never observes a partial file under `target_path`.
    fs::rename(&download_path, &target_path).map_err(|e| {
        let _ = fs::remove_file(&download_path);
        format!("Failed to install downloaded model: {}", e)
    })?;

    let progress = build_progress(def, downloaded, total_size, start.elapsed(), "complete");
    // Force percent=100 on completion even if the server misreported total.
    let progress = DownloadProgress {
        percent: 100.0,
        ..progress
    };
    let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);

    Ok(target_path.to_string_lossy().to_string())
}

/// Download + extract a `.tar.bz2` model archive into a directory named
/// `def.filename`, verifying it contains all `required_files`. Generalizes the
/// original Zipformer-only path so the Zipformer ASR model and the pyannote
/// diarization segmentation model (ADR-0017) share one implementation.
fn download_archive_model(
    app: &AppHandle,
    def: &ModelDef,
    required_files: &[&str],
    models_dir: &Path,
    target_path: &Path,
) -> Result<String, String> {
    use tauri::Emitter;

    if verify_archive_dir(target_path, required_files) {
        return Ok(target_path.to_string_lossy().to_string());
    }

    if target_path.exists() {
        remove_path(target_path)?;
    }

    let archive_path = models_dir.join(format!("{}.tar.bz2.download", def.filename));
    if archive_path.exists() {
        let _ = fs::remove_file(&archive_path);
    }

    let client = build_download_client();
    let response = client
        .get(def.url)
        .send()
        .map_err(|e| format!("Download failed: {}", e))?
        // Reject 404/403/HTML error pages before they stream into the archive
        // file (P1): a non-2xx body would otherwise be handed to the bzip2/tar
        // decoder and fail with an opaque "Failed to extract archive" error
        // instead of a clear HTTP-status message.
        .error_for_status()
        .map_err(|e| format!("Download failed: {}", e))?;

    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;
    let mut file =
        fs::File::create(&archive_path).map_err(|e| format!("Failed to create archive: {}", e))?;
    let mut reader = response;
    let mut buffer = vec![0u8; 8192];

    let start = Instant::now();
    let mut throttle = ProgressThrottle::new(PROGRESS_EMIT_INTERVAL);

    loop {
        let bytes_read = match std::io::Read::read(&mut reader, &mut buffer) {
            Ok(n) => n,
            Err(e) => {
                let _ = fs::remove_file(&archive_path);
                let progress =
                    build_progress(def, downloaded, total_size, start.elapsed(), "error");
                let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);
                return Err(format!("Read error: {}", e));
            }
        };
        if bytes_read == 0 {
            break;
        }

        file.write_all(&buffer[..bytes_read])
            .map_err(|e| format!("Write error: {}", e))?;
        downloaded += bytes_read as u64;

        if throttle.should_emit(Instant::now()) {
            let progress =
                build_progress(def, downloaded, total_size, start.elapsed(), "downloading");
            let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);
        }
    }
    drop(file);

    extract_archive(
        def.filename,
        required_files,
        &archive_path,
        models_dir,
        target_path,
    )?;
    let _ = fs::remove_file(&archive_path);

    if !verify_archive_dir(target_path, required_files) {
        let progress = build_progress(def, downloaded, total_size, start.elapsed(), "error");
        let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);
        return Err(format!(
            "Model extraction did not produce required files in '{}'",
            target_path.display()
        ));
    }

    let progress = DownloadProgress {
        percent: 100.0,
        ..build_progress(def, downloaded, total_size, start.elapsed(), "complete")
    };
    let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);

    Ok(target_path.to_string_lossy().to_string())
}

fn download_component_directory_model(
    app: &AppHandle,
    def: &ModelDef,
    required_files: &[&str],
    models_dir: &Path,
    target_path: &Path,
) -> Result<String, String> {
    use tauri::Emitter;

    if verify_archive_dir(target_path, required_files) {
        return Ok(target_path.to_string_lossy().to_string());
    }

    if target_path.exists() {
        remove_path(target_path)?;
    }

    let install_dir = models_dir.join(format!("{}.downloading", def.filename));
    if install_dir.exists() {
        remove_path(&install_dir)?;
    }
    fs::create_dir_all(&install_dir)
        .map_err(|e| format!("Failed to create model component directory: {}", e))?;

    let client = build_download_client();
    let mut downloaded_total = 0_u64;
    let mut expected_total = 0_u64;
    let start = Instant::now();
    let mut throttle = ProgressThrottle::new(PROGRESS_EMIT_INTERVAL);
    let base_url = def.url.trim_end_matches('/');

    for component in required_files {
        let component_url = format!("{}/{}", base_url, component);
        let component_path = install_dir.join(component);
        let component_tmp_path = install_dir.join(format!("{}.download", component));
        if component_tmp_path.exists() {
            let _ = fs::remove_file(&component_tmp_path);
        }

        let response = match client
            .get(&component_url)
            .send()
            .and_then(|r| r.error_for_status())
        {
            Ok(response) => response,
            Err(error) => {
                let _ = remove_path(&install_dir);
                let progress = build_progress(
                    def,
                    downloaded_total,
                    expected_total,
                    start.elapsed(),
                    "error",
                );
                let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);
                return Err(format!(
                    "Download failed for Moonshine component '{}': {}",
                    component, error
                ));
            }
        };

        let content_length = response.content_length().unwrap_or(0);
        expected_total = expected_total.saturating_add(content_length);
        let mut file = fs::File::create(&component_tmp_path)
            .map_err(|e| format!("Failed to create component file: {}", e))?;
        let mut reader = response;
        let mut buffer = vec![0u8; 8192];

        loop {
            let bytes_read = match std::io::Read::read(&mut reader, &mut buffer) {
                Ok(n) => n,
                Err(error) => {
                    let _ = remove_path(&install_dir);
                    let progress = build_progress(
                        def,
                        downloaded_total,
                        expected_total,
                        start.elapsed(),
                        "error",
                    );
                    let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);
                    return Err(format!(
                        "Read error for Moonshine component '{}': {}",
                        component, error
                    ));
                }
            };
            if bytes_read == 0 {
                break;
            }

            file.write_all(&buffer[..bytes_read])
                .map_err(|e| format!("Write error for component '{}': {}", component, e))?;
            downloaded_total = downloaded_total.saturating_add(bytes_read as u64);

            if throttle.should_emit(Instant::now()) {
                let progress = build_progress(
                    def,
                    downloaded_total,
                    expected_total,
                    start.elapsed(),
                    "downloading",
                );
                let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);
            }
        }
        drop(file);

        if !verify_model_file(&component_tmp_path, None) {
            let _ = remove_path(&install_dir);
            let progress = build_progress(
                def,
                downloaded_total,
                expected_total,
                start.elapsed(),
                "error",
            );
            let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);
            return Err(format!(
                "Download verification failed for Moonshine component '{}'",
                component
            ));
        }

        fs::rename(&component_tmp_path, &component_path).map_err(|e| {
            let _ = remove_path(&install_dir);
            format!(
                "Failed to install Moonshine component '{}': {}",
                component, e
            )
        })?;
    }

    if !verify_archive_dir(&install_dir, required_files) {
        let _ = remove_path(&install_dir);
        let progress = build_progress(
            def,
            downloaded_total,
            expected_total,
            start.elapsed(),
            "error",
        );
        let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);
        return Err(format!(
            "Moonshine component download did not produce required files: {}",
            required_files.join(", ")
        ));
    }

    fs::rename(&install_dir, target_path)
        .map_err(|e| format!("Failed to install Moonshine model directory: {}", e))?;

    let progress = DownloadProgress {
        percent: 100.0,
        ..build_progress(
            def,
            downloaded_total,
            expected_total,
            start.elapsed(),
            "complete",
        )
    };
    let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);

    Ok(target_path.to_string_lossy().to_string())
}

fn extract_archive(
    model_dir_name: &str,
    required_files: &[&str],
    archive_path: &Path,
    models_dir: &Path,
    target_path: &Path,
) -> Result<(), String> {
    let extract_dir = models_dir.join(format!("{}.extracting", model_dir_name));
    if extract_dir.exists() {
        remove_path(&extract_dir)?;
    }
    fs::create_dir_all(&extract_dir).map_err(|e| format!("Failed to create extract dir: {}", e))?;

    let archive_file =
        fs::File::open(archive_path).map_err(|e| format!("Failed to open archive: {}", e))?;
    let decoder = bzip2::read::BzDecoder::new(archive_file);
    let mut archive = tar::Archive::new(decoder);
    archive
        .unpack(&extract_dir)
        .map_err(|e| format!("Failed to extract archive: {}", e))?;

    let model_root = find_archive_model_root(&extract_dir, required_files).ok_or_else(|| {
        format!(
            "Archive did not contain required files: {}",
            required_files.join(", ")
        )
    })?;

    if target_path.exists() {
        remove_path(target_path)?;
    }
    fs::rename(&model_root, target_path)
        .map_err(|e| format!("Failed to install extracted model: {}", e))?;

    if extract_dir.exists() {
        let _ = fs::remove_dir_all(&extract_dir);
    }
    Ok(())
}

/// Recursively locate the directory inside an extracted archive tree that holds
/// every `required_files` entry (archives often nest the model under a
/// release-named subdirectory).
fn find_archive_model_root(path: &Path, required_files: &[&str]) -> Option<PathBuf> {
    if verify_archive_dir(path, required_files) {
        return Some(path.to_path_buf());
    }

    let entries = fs::read_dir(path).ok()?;
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir()
            && let Some(found) = find_archive_model_root(&child, required_files)
        {
            return Some(found);
        }
    }
    None
}

fn remove_path(path: &Path) -> Result<(), String> {
    if path.is_dir() {
        fs::remove_dir_all(path)
            .map_err(|e| format!("Failed to remove directory '{}': {}", path.display(), e))
    } else {
        fs::remove_file(path)
            .map_err(|e| format!("Failed to remove file '{}': {}", path.display(), e))
    }
}

// ---------------------------------------------------------------------------
// Deletion
// ---------------------------------------------------------------------------

/// Delete a downloaded model file
pub fn delete_model(app: &AppHandle, filename: &str) -> Result<String, String> {
    // Validate filename - prevent path traversal
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err("Invalid filename".to_string());
    }

    let models_dir = get_models_dir(app);
    let model_path = models_dir.join(filename);

    // Verify the file is actually in the models directory
    if !model_path.starts_with(&models_dir) {
        return Err("Invalid model path".to_string());
    }

    if !model_path.exists() {
        return Err(format!("Model file not found: {}", filename));
    }

    remove_path(&model_path).map_err(|e| format!("Failed to delete model: {}", e))?;

    log::info!("Deleted model: {}", filename);
    Ok(format!("Model '{}' deleted successfully", filename))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn throttle_emits_first_call_then_gates_until_interval_elapses() {
        // A download loop may iterate thousands of times before 1 s has
        // passed. The throttle must let the first tick through (so the UI
        // shows immediate feedback) and then suppress every follow-up read
        // that arrives inside the same interval.
        let mut throttle = ProgressThrottle::new(Duration::from_millis(1000));
        let start = Instant::now();

        assert!(
            throttle.should_emit(start),
            "first emit must fire so the UI sees progress immediately",
        );

        // A burst of rapid reads inside the interval must all be suppressed.
        for offset_ms in &[1_u64, 10, 100, 500, 999] {
            let t = start + Duration::from_millis(*offset_ms);
            assert!(
                !throttle.should_emit(t),
                "emit at +{}ms should be throttled",
                offset_ms,
            );
        }

        // Once the interval has elapsed, the next tick should fire again.
        let t_after = start + Duration::from_millis(1000);
        assert!(
            throttle.should_emit(t_after),
            "emit at +1000ms must fire — interval boundary is inclusive",
        );

        // And immediately re-gates.
        let t_just_after = start + Duration::from_millis(1001);
        assert!(
            !throttle.should_emit(t_just_after),
            "post-emit tick must be throttled again",
        );
    }

    #[test]
    fn compute_percent_handles_zero_total_as_unknown() {
        // When the server omits Content-Length we serialize total_bytes as
        // `0`. The percent must stay at 0 so the frontend can detect the
        // "unknown size" case and skip ETA rendering instead of showing a
        // garbage division-by-zero bar.
        assert_eq!(compute_percent(0, 0), 0.0);
        assert_eq!(compute_percent(12_345, 0), 0.0);

        // Normal case: halfway through a known-size download.
        assert!((compute_percent(50, 100) - 50.0).abs() < f32::EPSILON);
        assert!((compute_percent(100, 100) - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn build_progress_includes_model_id_and_elapsed_ms() {
        // Guards the on-wire shape the frontend depends on: filename as
        // `model_id`, monotonic elapsed_ms, and `total_bytes=0` for unknown.
        let def = &MODELS[0];
        let p = build_progress(def, 1024, 0, Duration::from_millis(250), "downloading");

        assert_eq!(p.model_id, def.filename);
        assert_eq!(p.bytes_downloaded, 1024);
        assert_eq!(p.total_bytes, 0);
        assert_eq!(p.elapsed_ms, 250);
        assert_eq!(p.percent, 0.0);
        assert_eq!(p.status, "downloading");
    }

    #[test]
    fn sherpa_zipformer_validation_requires_runtime_files() {
        let root =
            std::env::temp_dir().join(format!("audiograph-sherpa-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();

        for file in SHERPA_ZIPFORMER_REQUIRED_FILES {
            fs::write(root.join(file), b"test").unwrap();
        }

        assert!(verify_archive_dir(&root, SHERPA_ZIPFORMER_REQUIRED_FILES));
        fs::remove_file(root.join("tokens.txt")).unwrap();
        assert!(!verify_archive_dir(&root, SHERPA_ZIPFORMER_REQUIRED_FILES));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn moonshine_streaming_validation_requires_runtime_components() {
        let root = std::env::temp_dir().join(format!(
            "audiograph-moonshine-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();

        for file in MOONSHINE_STREAMING_REQUIRED_FILES {
            fs::write(root.join(file), b"component").unwrap();
        }

        assert!(verify_archive_dir(
            &root,
            MOONSHINE_STREAMING_REQUIRED_FILES
        ));
        fs::remove_file(root.join("streaming_config.json")).unwrap();
        assert!(!verify_archive_dir(
            &root,
            MOONSHINE_STREAMING_REQUIRED_FILES
        ));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn archive_dir_rejects_zero_byte_required_files() {
        // A truncated/interrupted extraction can leave a required file present
        // but zero-length. `is_file()` alone would call that valid and defer
        // the failure to runtime model load; verify_archive_dir must reject it.
        let root = std::env::temp_dir().join(format!(
            "audiograph-archive-empty-test-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();

        // All required files present and non-empty → valid.
        for file in SHERPA_ZIPFORMER_REQUIRED_FILES {
            fs::write(root.join(file), b"x").unwrap();
        }
        assert!(verify_archive_dir(&root, SHERPA_ZIPFORMER_REQUIRED_FILES));

        // Truncate one required file to zero bytes → must be rejected.
        fs::write(root.join("tokens.txt"), b"").unwrap();
        assert_eq!(fs::metadata(root.join("tokens.txt")).unwrap().len(), 0);
        assert!(
            !verify_archive_dir(&root, SHERPA_ZIPFORMER_REQUIRED_FILES),
            "a zero-byte required file is a corrupt extraction, not ready"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn finds_sherpa_model_root_inside_extracted_archive_tree() {
        let root = std::env::temp_dir().join(format!(
            "audiograph-sherpa-find-test-{}",
            uuid::Uuid::new_v4()
        ));
        let nested = root.join("sherpa-onnx-streaming-zipformer-en-20M-2023-02-17");
        fs::create_dir_all(&nested).unwrap();

        for file in SHERPA_ZIPFORMER_REQUIRED_FILES {
            fs::write(nested.join(file), b"test").unwrap();
        }

        assert_eq!(
            find_archive_model_root(&root, SHERPA_ZIPFORMER_REQUIRED_FILES),
            Some(nested)
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn pyannote_segmentation_archive_validation_requires_model_files() {
        // ADR-0017 / B16: the pyannote segmentation archive verifier uses the
        // same archive-dir path as Zipformer but with its own required files.
        let root =
            std::env::temp_dir().join(format!("audiograph-pyannote-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();

        for file in DIAR_SEG_PYANNOTE_REQUIRED_FILES {
            fs::write(root.join(file), b"test").unwrap();
        }
        assert!(verify_archive_dir(&root, DIAR_SEG_PYANNOTE_REQUIRED_FILES));

        // The int8 file we actually load must be among the required files.
        assert!(DIAR_SEG_PYANNOTE_REQUIRED_FILES.contains(&DIAR_SEG_PYANNOTE_FILE));

        fs::remove_file(root.join(DIAR_SEG_PYANNOTE_FILE)).unwrap();
        assert!(!verify_archive_dir(&root, DIAR_SEG_PYANNOTE_REQUIRED_FILES));

        let _ = fs::remove_dir_all(root);
    }

    /// Spawn a one-shot blocking HTTP/1.1 server on an ephemeral port that
    /// answers the first request with `status`/`status_text` and `body`, then
    /// closes. Returns the base URL. Used to prove the download path rejects a
    /// non-2xx error page instead of streaming it into the target file (P1).
    fn spawn_oneshot_http(
        status: u16,
        status_text: &'static str,
        content_type: &'static str,
        body: &'static str,
    ) -> String {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind test server");
        let addr = listener.local_addr().expect("local addr");
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                // Drain the request headers so the client's write side doesn't
                // get a RST before it reads our response.
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let response = format!(
                    "HTTP/1.1 {status} {status_text}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{addr}")
    }

    #[test]
    fn error_for_status_rejects_non_2xx_html_body() {
        // P1 regression guard: a 404 that returns an HTML error page must be
        // rejected at the HTTP layer via `.error_for_status()`, NOT streamed
        // into the target file. Before the fix, `download_model`/`
        // download_archive_model` called `.send()` without `.error_for_status()`,
        // so a 404 HTML body would write to disk and (for `expected_size:None`
        // models) pass `verify_model_file` as a ready model. This exercises the
        // exact builder + send + error_for_status chain the downloaders use.
        let url = spawn_oneshot_http(
            404,
            "Not Found",
            "text/html",
            "<!DOCTYPE html><html><body>404: model not found</body></html>",
        );

        let client = build_download_client();
        let result = client.get(&url).send().and_then(|r| r.error_for_status());

        let err = result.expect_err("a 404 HTML page must be rejected, not accepted as a model");
        assert!(
            err.status() == Some(reqwest::StatusCode::NOT_FOUND),
            "the mapped error must carry the 404 status, got {err:?}"
        );
    }

    #[test]
    fn error_for_status_passes_2xx() {
        // Complement: a normal 200 must NOT be turned into an error so real
        // downloads still proceed.
        let url = spawn_oneshot_http(200, "OK", "application/octet-stream", "model-bytes");
        let client = build_download_client();
        let resp = client
            .get(&url)
            .send()
            .and_then(|r| r.error_for_status())
            .expect("a 200 response must pass error_for_status");
        assert!(resp.status().is_success());
    }

    #[test]
    fn download_client_has_timeouts_configured() {
        // P4 guard: the download client must be the timeout-configured builder
        // output, not the no-timeout `Client::new()`. We can't read the timeout
        // back off a built Client, so this is a construction smoke test that the
        // builder path is wired and doesn't panic; the constants are asserted
        // directly to document the chosen values.
        let _client = build_download_client();
        assert_eq!(DOWNLOAD_CONNECT_TIMEOUT, Duration::from_secs(10));
        assert_eq!(DOWNLOAD_READ_TIMEOUT, Duration::from_secs(300));
    }

    #[test]
    fn clustering_diarization_models_are_registered() {
        // The two ADR-0017 models must appear in MODELS so the downloader + UI
        // can see them: segmentation as an archive, embedding as a bare file.
        let seg = MODELS
            .iter()
            .find(|m| m.filename == DIAR_SEG_PYANNOTE_DIR)
            .expect("pyannote segmentation registered");
        assert!(
            seg.archive_required_files.is_some(),
            "segmentation is an extracted archive"
        );
        assert_eq!(seg.expected_size, None, "no published size for the archive");

        let emb = MODELS
            .iter()
            .find(|m| m.filename == DIAR_EMB_TITANET_FILENAME)
            .expect("TitaNet embedding registered");
        assert!(
            emb.archive_required_files.is_none(),
            "embedding is a bare .onnx download"
        );
        assert_eq!(emb.expected_size, None, "non-empty check only");
    }

    #[test]
    fn moonshine_streaming_models_are_registered_as_component_directories() {
        let moonshine: Vec<_> = MODELS
            .iter()
            .filter(|model| model.filename.starts_with("moonshine-"))
            .collect();

        assert_eq!(moonshine.len(), 3);
        assert!(moonshine.iter().any(|model| {
            model.filename == MOONSHINE_TINY_STREAMING_EN
                && model.url == MOONSHINE_TINY_STREAMING_EN_URL
        }));
        assert!(moonshine.iter().any(|model| {
            model.filename == MOONSHINE_SMALL_STREAMING_EN
                && model.url == MOONSHINE_SMALL_STREAMING_EN_URL
        }));
        assert!(moonshine.iter().any(|model| {
            model.filename == MOONSHINE_MEDIUM_STREAMING_EN
                && model.url == MOONSHINE_MEDIUM_STREAMING_EN_URL
        }));

        for model in moonshine {
            assert_eq!(model.archive_required_files, None);
            assert_eq!(
                model.component_required_files,
                Some(MOONSHINE_STREAMING_REQUIRED_FILES)
            );
            assert_eq!(model.expected_size, None);
        }
    }
}
