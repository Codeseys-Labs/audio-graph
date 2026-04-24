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
}

pub const WHISPER_MODEL_TINY_EN: &str = "ggml-tiny.en.bin";
pub const WHISPER_MODEL_BASE_EN: &str = "ggml-base.en.bin";
pub const WHISPER_MODEL_SMALL_EN: &str = "ggml-small.en.bin";
pub const WHISPER_MODEL_MEDIUM_EN: &str = "ggml-medium.en.bin";
pub const WHISPER_MODEL_LARGE_V3: &str = "ggml-large-v3.bin";

const LLM_MODEL_URL: &str = "https://huggingface.co/LiquidAI/LFM2-350M-Extract-GGUF/resolve/main/lfm2-350m-extract-q4_k_m.gguf";
/// Public so that commands can reference the canonical LLM model filename.
pub const LLM_MODEL_FILENAME: &str = "lfm2-350m-extract-q4_k_m.gguf";
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

const MODELS: &[ModelDef] = &[
    ModelDef {
        name: "Whisper Tiny (English)",
        filename: WHISPER_MODEL_TINY_EN,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin",
        expected_size: Some(77_700_000),
        description:
            "Fastest model (~75MB). 5x faster than Small, lower accuracy. Good for weak hardware.",
    },
    ModelDef {
        name: "Whisper Base (English)",
        filename: WHISPER_MODEL_BASE_EN,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin",
        expected_size: Some(147_500_000),
        description: "Best real-time balance (~142MB). 2-3x faster than Small on Apple Silicon.",
    },
    ModelDef {
        name: "Whisper Small (English)",
        filename: WHISPER_MODEL_SMALL_EN,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en.bin",
        expected_size: Some(487_654_400),
        description: "Default model (~466MB). Good accuracy/speed balance.",
    },
    ModelDef {
        name: "Whisper Medium (English)",
        filename: WHISPER_MODEL_MEDIUM_EN,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.en.bin",
        expected_size: Some(1_533_800_000),
        description: "High accuracy (~1.5GB). Requires strong GPU for real-time.",
    },
    ModelDef {
        name: "Whisper Large v3 (Multilingual)",
        filename: WHISPER_MODEL_LARGE_V3,
        url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin",
        expected_size: Some(3_094_600_000),
        description: "Best accuracy (~3GB). Multilingual. Requires powerful GPU.",
    },
    ModelDef {
        name: "LFM2-350M Extract (Entity Extraction)",
        filename: LLM_MODEL_FILENAME,
        url: LLM_MODEL_URL,
        expected_size: Some(LLM_EXPECTED_SIZE),
        description: "Small language model for entity and relationship extraction",
    },
    ModelDef {
        name: "Sortformer v2 (Speaker Diarization)",
        filename: SORTFORMER_MODEL_FILENAME,
        url: SORTFORMER_MODEL_URL,
        expected_size: Some(SORTFORMER_EXPECTED_SIZE),
        description: "Streaming speaker diarization — up to 4 speakers (NVIDIA Sortformer ONNX)",
    },
    ModelDef {
        name: "Sherpa Zipformer 20M (Streaming ASR)",
        filename: SHERPA_ZIPFORMER_20M,
        url: SHERPA_ZIPFORMER_20M_URL,
        expected_size: Some(SHERPA_ZIPFORMER_20M_EXPECTED_SIZE),
        description:
            "Streaming ASR via Zipformer transducer — sub-200ms first-word latency (sherpa-onnx)",
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
            let exists = path.exists();
            let valid = verify_model_file(&path, def.expected_size);
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

    if target_path.exists() && verify_model_file(&target_path, def.expected_size) {
        return Ok(target_path.to_string_lossy().to_string());
    }

    if target_path.exists() {
        let _ = fs::remove_file(&target_path);
    }

    let client = reqwest::blocking::Client::new();
    let response = client
        .get(def.url)
        .send()
        .map_err(|e| format!("Download failed: {}", e))?;

    // `content_length()` is `None` when the server omits `Content-Length`.
    // We encode that as `0` on the wire so the payload type stays a plain
    // `u64` and the frontend can branch on `total_bytes === 0`.
    let total_size = response.content_length().unwrap_or(0);
    let mut downloaded: u64 = 0;

    let mut file =
        fs::File::create(&target_path).map_err(|e| format!("Failed to create file: {}", e))?;

    let mut reader = response;
    let mut buffer = vec![0u8; 8192];

    let start = Instant::now();
    let mut throttle = ProgressThrottle::new(PROGRESS_EMIT_INTERVAL);

    loop {
        let bytes_read = match std::io::Read::read(&mut reader, &mut buffer) {
            Ok(n) => n,
            Err(e) => {
                let err_msg = format!("Read error: {}", e);
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

    if !verify_model_file(&target_path, def.expected_size) {
        let actual_size = fs::metadata(&target_path).map(|m| m.len()).unwrap_or(0);
        let _ = fs::remove_file(&target_path);
        let err_msg = format!(
            "Download verification failed for '{}': got {} bytes, expected ~{:?} bytes",
            filename, actual_size, def.expected_size
        );
        let progress = build_progress(def, downloaded, total_size, start.elapsed(), "error");
        let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);
        return Err(err_msg);
    }

    let progress = build_progress(def, downloaded, total_size, start.elapsed(), "complete");
    // Force percent=100 on completion even if the server misreported total.
    let progress = DownloadProgress {
        percent: 100.0,
        ..progress
    };
    let _ = app.emit(MODEL_DOWNLOAD_PROGRESS, &progress);

    Ok(target_path.to_string_lossy().to_string())
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

    fs::remove_file(&model_path).map_err(|e| format!("Failed to delete model: {}", e))?;

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
}
