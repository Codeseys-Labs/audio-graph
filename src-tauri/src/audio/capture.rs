//! Audio capture manager — wraps rsac for multi-source audio capture.
//!
//! Responsibilities:
//! - Enumerate audio devices and applications via rsac
//! - Start/stop capture sessions
//! - Tag audio buffers with source ID and wall-clock time
//! - Forward tagged buffers to the processing pipeline via crossbeam channel

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use rsac::{AudioCaptureBuilder, CaptureTarget};
use tauri::AppHandle;

use crate::events::{
    emit_or_log, CaptureBackpressurePayload, CaptureErrorPayload, CAPTURE_BACKPRESSURE,
    CAPTURE_ERROR,
};
use crate::state::{AudioSourceInfo, AudioSourceType};

// ---------------------------------------------------------------------------
// AudioChunk — tagged audio data flowing through the pipeline
// ---------------------------------------------------------------------------

/// A chunk of captured audio data tagged with its source and timestamp.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// Identifier of the capture source that produced this chunk.
    pub source_id: String,
    /// Interleaved f32 sample data.
    pub data: Vec<f32>,
    /// Sample rate in Hz (typically 48 000).
    pub sample_rate: u32,
    /// Number of channels (typically 2 for stereo).
    pub channels: u16,
    /// Number of audio frames in this chunk.
    pub num_frames: usize,
    /// Elapsed time since the capture session started.
    pub timestamp: Option<Duration>,
}

// ---------------------------------------------------------------------------
// CaptureHandle — per-source bookkeeping
// ---------------------------------------------------------------------------

/// Handle to a running audio capture thread.
#[allow(dead_code)] // M8: source_info is stored for future introspection (e.g., active-capture queries)
struct CaptureHandle {
    thread: Option<JoinHandle<()>>,
    stop_signal: Arc<AtomicBool>,
    source_info: AudioSourceInfo,
}

// ---------------------------------------------------------------------------
// AudioCaptureManager
// ---------------------------------------------------------------------------

/// Manages multiple concurrent audio capture sources.
///
/// Each active capture runs on its own dedicated thread (required because
/// `rsac::AudioCapture` is `!Sync`). Audio data is forwarded as [`AudioChunk`]
/// values over the supplied `crossbeam_channel::Sender`.
pub struct AudioCaptureManager {
    sources: HashMap<String, CaptureHandle>,
}

impl AudioCaptureManager {
    /// Create a new capture manager.
    pub fn new() -> Self {
        Self {
            sources: HashMap::new(),
        }
    }

    // ----- source listing --------------------------------------------------

    /// List available audio sources (devices + running applications).
    ///
    /// Uses rsac's cross-platform `list_audio_sources()` to discover all
    /// capturable sources (system default, devices, applications) without
    /// platform-specific `#[cfg]` blocks. Active-capture state is overlaid
    /// from `self.sources`.
    pub fn list_sources(&self) -> Vec<AudioSourceInfo> {
        // Use rsac's unified cross-platform introspection API.
        // This replaces ~120 lines of per-platform #[cfg] code.
        let rsac_sources = match rsac::list_audio_sources() {
            Ok(s) => s,
            Err(e) => {
                log::warn!("Failed to list audio sources via rsac: {}", e);
                // Fallback: at least return the system default
                return vec![AudioSourceInfo {
                    id: "system-default".to_string(),
                    name: "System Default".to_string(),
                    source_type: AudioSourceType::SystemDefault,
                    is_active: self.sources.contains_key("system-default"),
                }];
            }
        };

        let sources: Vec<AudioSourceInfo> = rsac_sources
            .into_iter()
            .map(|src| {
                let is_active = self.sources.contains_key(&src.id);
                let source_type = match &src.kind {
                    rsac::AudioSourceKind::SystemDefault => AudioSourceType::SystemDefault,
                    rsac::AudioSourceKind::Device { device_id, .. } => AudioSourceType::Device {
                        device_id: device_id.clone(),
                    },
                    rsac::AudioSourceKind::Application { pid, app_name, .. } => {
                        AudioSourceType::Application {
                            pid: *pid,
                            app_name: app_name.clone(),
                        }
                    }
                };
                AudioSourceInfo {
                    id: src.id,
                    name: src.name,
                    source_type,
                    is_active,
                }
            })
            .collect();

        log::info!("Total audio sources listed: {}", sources.len());
        sources
    }

    // ----- capture lifecycle -----------------------------------------------

    /// Start capturing audio from the specified source.
    ///
    /// Spawns a dedicated thread that creates an `rsac::AudioCapture`,
    /// subscribes to audio buffers, converts them to [`AudioChunk`], and
    /// forwards them through `pipeline_tx`.
    ///
    /// `sample_rate` and `channels` come from the caller (typically resolved
    /// from `AppSettings.audio_settings`) and are passed straight to the rsac
    /// builder. They're expected to have already passed validation via
    /// [`crate::settings::resolve_audio_settings`] so the rsac call gets a
    /// value it actually supports.
    pub fn start_capture(
        &mut self,
        source_id: &str,
        target: CaptureTarget,
        pipeline_tx: Sender<AudioChunk>,
        app_handle: AppHandle,
        sample_rate: u32,
        channels: u16,
    ) -> Result<(), String> {
        if self.sources.contains_key(source_id) {
            return Err(format!("Source '{}' is already being captured", source_id));
        }

        let stop_signal = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_signal);
        let sid = source_id.to_string();

        // M3: Derive the actual AudioSourceType from the CaptureTarget.
        let source_type = match &target {
            CaptureTarget::SystemDefault => AudioSourceType::SystemDefault,
            CaptureTarget::Device(dev_id) => AudioSourceType::Device {
                device_id: dev_id.0.clone(),
            },
            CaptureTarget::Application(app_id) => AudioSourceType::Application {
                pid: app_id.0.parse::<u32>().unwrap_or(0),
                app_name: source_id.to_string(),
            },
            CaptureTarget::ApplicationByName(name) => AudioSourceType::Application {
                pid: 0,
                app_name: name.clone(),
            },
            CaptureTarget::ProcessTree(proc_id) => AudioSourceType::Application {
                pid: proc_id.0,
                app_name: source_id.to_string(),
            },
        };

        let source_info = AudioSourceInfo {
            id: source_id.to_string(),
            name: source_id.to_string(),
            source_type,
            is_active: true,
        };

        let thread = std::thread::Builder::new()
            .name(format!("capture-{}", source_id))
            .spawn(move || {
                Self::capture_thread_fn(
                    sid,
                    target,
                    stop_clone,
                    pipeline_tx,
                    app_handle,
                    sample_rate,
                    channels,
                );
            })
            .map_err(|e| format!("Failed to spawn capture thread: {}", e))?;

        self.sources.insert(
            source_id.to_string(),
            CaptureHandle {
                thread: Some(thread),
                stop_signal,
                source_info,
            },
        );

        log::info!("Started capture for source '{}'", source_id);
        Ok(())
    }

    /// Stop capturing audio from the specified source.
    ///
    /// Signals the capture thread to exit and joins it with a timeout.
    pub fn stop_capture(&mut self, source_id: &str) -> Result<(), String> {
        let handle = self
            .sources
            .remove(source_id)
            .ok_or_else(|| format!("No active capture for source '{}'", source_id))?;

        // Signal the thread to stop.
        handle.stop_signal.store(true, Ordering::Release);

        // Join the thread with a timeout strategy: park the current thread
        // briefly and check if the child is finished.
        if let Some(join_handle) = handle.thread {
            let deadline = Instant::now() + Duration::from_secs(3);
            let mut joined = false;

            // We can't do a timed join directly on std JoinHandle, so we
            // spin-sleep and check `is_finished()`.
            while Instant::now() < deadline {
                if join_handle.is_finished() {
                    let _ = join_handle.join();
                    joined = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }

            if !joined {
                log::warn!(
                    "Capture thread for '{}' did not exit within 3 s — detaching",
                    source_id
                );
                // Thread is leaked intentionally; the stop signal is already
                // set so it should eventually exit on its own.
            }
        }

        log::info!("Stopped capture for source '{}'", source_id);
        Ok(())
    }

    /// Stop all active captures. Returns the list of source IDs that were
    /// stopped.
    pub fn stop_all(&mut self) -> Vec<String> {
        let ids: Vec<String> = self.sources.keys().cloned().collect();
        let mut stopped = Vec::new();

        for id in &ids {
            match self.stop_capture(id) {
                Ok(()) => stopped.push(id.clone()),
                Err(e) => log::error!("Failed to stop capture '{}': {}", id, e),
            }
        }

        log::info!("Stopped {} capture(s)", stopped.len());
        stopped
    }

    /// Returns the list of currently active source IDs.
    pub fn active_captures(&self) -> Vec<String> {
        self.sources.keys().cloned().collect()
    }

    // ----- internal: capture thread ----------------------------------------

    /// Body of a capture thread.
    ///
    /// Owns the `AudioCapture` (which is `!Sync`) for its entire lifetime.
    ///
    /// `sample_rate` and `channels` are the user-configured capture format
    /// (resolved from `AppSettings.audio_settings` in the caller). The
    /// pipeline still downsamples to 16 kHz mono for ASR downstream — these
    /// values only control what the OS / driver captures in the first place.
    fn capture_thread_fn(
        source_id: String,
        target: CaptureTarget,
        stop_signal: Arc<AtomicBool>,
        pipeline_tx: Sender<AudioChunk>,
        app_handle: AppHandle,
        sample_rate: u32,
        channels: u16,
    ) {
        log::info!(
            "[capture-{}] Thread started (requested {} Hz, {} ch)",
            source_id,
            sample_rate,
            channels
        );

        // 1. Build the capture session using the resolved user-configured
        //    format. The pipeline resamples to 16 kHz mono downstream, so
        //    the only thing these values affect is how the OS delivers audio.
        let mut capture = match AudioCaptureBuilder::new()
            .with_target(target)
            .sample_rate(sample_rate)
            .channels(channels)
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                log::error!(
                    "[capture-{}] Failed to build AudioCapture: {}",
                    source_id,
                    e
                );
                emit_or_log(
                    &app_handle,
                    CAPTURE_ERROR,
                    CaptureErrorPayload {
                        source_id: source_id.clone(),
                        error: format!("{}", e),
                        recoverable: false,
                    },
                );
                return;
            }
        };

        // 2. Start capture.
        if let Err(e) = capture.start() {
            log::error!("[capture-{}] Failed to start capture: {}", source_id, e);
            let err_str = format!("{}", e);
            emit_or_log(
                &app_handle,
                CAPTURE_ERROR,
                CaptureErrorPayload {
                    source_id: source_id.clone(),
                    error: err_str.clone(),
                    recoverable: crate::events::classify_capture_error(&err_str),
                },
            );
            return;
        }

        // 3. Subscribe to push-based audio delivery.
        let rx = match capture.subscribe() {
            Ok(r) => r,
            Err(e) => {
                log::error!("[capture-{}] Failed to subscribe: {}", source_id, e);
                emit_or_log(
                    &app_handle,
                    CAPTURE_ERROR,
                    CaptureErrorPayload {
                        source_id: source_id.clone(),
                        error: format!("{}", e),
                        recoverable: false,
                    },
                );
                let _ = capture.stop();
                return;
            }
        };

        let start_time = Instant::now();
        log::info!("[capture-{}] Receiving audio buffers", source_id);

        // Edge-triggered backpressure tracking. rsac's ring buffer flips
        // `is_under_backpressure()` once it has dropped N consecutive chunks;
        // we only emit on transitions so the frontend gets clean enter/leave
        // signals rather than a storm of identical events.
        let mut last_backpressured = false;
        // Cheap rate limiter: poll every 10 iterations (~50ms at 48kHz / 5ms
        // buffers). is_under_backpressure() is an atomic load so it's already
        // dirt cheap, but this keeps the loop tight in the hot path.
        let mut poll_counter: u32 = 0;

        // 4. Read loop — exit when stop_signal is set or channel closes.
        while !stop_signal.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(buffer) => {
                    let chunk = AudioChunk {
                        source_id: source_id.clone(),
                        data: buffer.data().to_vec(),
                        sample_rate: buffer.sample_rate(),
                        channels: buffer.channels(),
                        num_frames: buffer.num_frames(),
                        timestamp: Some(start_time.elapsed()),
                    };
                    if let Err(e) = pipeline_tx.send(chunk) {
                        log::warn!(
                            "[capture-{}] Pipeline channel closed, exiting: {}",
                            source_id,
                            e
                        );
                        break;
                    }

                    poll_counter = poll_counter.wrapping_add(1);
                    if poll_counter.is_multiple_of(10) {
                        let now_backpressured = capture.is_under_backpressure();
                        if now_backpressured != last_backpressured {
                            if now_backpressured {
                                log::warn!(
                                    "[capture-{}] Backpressure detected — \
                                     pipeline consumer is too slow, ring buffer \
                                     is dropping chunks",
                                    source_id,
                                );
                            } else {
                                log::info!("[capture-{}] Backpressure cleared", source_id,);
                            }
                            emit_or_log(
                                &app_handle,
                                CAPTURE_BACKPRESSURE,
                                CaptureBackpressurePayload {
                                    source_id: source_id.clone(),
                                    is_backpressured: now_backpressured,
                                },
                            );
                            last_backpressured = now_backpressured;
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // No data yet — loop back and check stop_signal.
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    log::info!("[capture-{}] Audio stream ended (disconnected)", source_id);
                    break;
                }
            }
        }

        // 5. Tear down.
        log::info!("[capture-{}] Stopping capture", source_id);
        let _ = capture.stop();
        log::info!("[capture-{}] Thread exiting", source_id);
    }

    // ----- internal: PipeWire application discovery (Linux only) -----------
    // NOTE: This function is superseded by rsac::list_audio_sources() which
    // handles PipeWire discovery cross-platform. Kept for reference only.

    /// Discover PipeWire audio client applications by parsing `pw-dump` JSON.
    #[cfg(target_os = "linux")]
    #[allow(dead_code)]
    fn list_pipewire_applications() -> Result<Vec<PipeWireApp>, String> {
        let output = std::process::Command::new("pw-dump")
            .output()
            .map_err(|e| format!("Failed to run pw-dump: {}", e))?;

        if !output.status.success() {
            return Err(format!("pw-dump exited with status {}", output.status));
        }

        let json_str = String::from_utf8(output.stdout)
            .map_err(|e| format!("Invalid UTF-8 from pw-dump: {}", e))?;

        let nodes: serde_json::Value = serde_json::from_str(&json_str)
            .map_err(|e| format!("Failed to parse pw-dump JSON: {}", e))?;

        let mut apps: Vec<PipeWireApp> = Vec::new();
        let mut seen_pids = std::collections::HashSet::new();

        if let Some(arr) = nodes.as_array() {
            for node in arr {
                // We only care about PipeWire nodes that are audio output streams.
                let media_class = node
                    .pointer("/info/props/media.class")
                    .and_then(|v| v.as_str());

                if media_class != Some("Stream/Output/Audio") {
                    continue;
                }

                let app_name = node
                    .pointer("/info/props/application.name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown")
                    .to_string();

                let pid_str = node
                    .pointer("/info/props/application.process.id")
                    .and_then(|v| v.as_str());

                let pid: u32 = match pid_str {
                    Some(s) => match s.parse() {
                        Ok(p) => p,
                        Err(_) => continue,
                    },
                    None => continue,
                };

                // Deduplicate by PID (an app may open multiple streams).
                if seen_pids.insert(pid) {
                    apps.push(PipeWireApp {
                        id: pid.to_string(),
                        name: app_name,
                        pid,
                    });
                }
            }
        }

        Ok(apps)
    }
}

impl Default for AudioCaptureManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// PipeWire helpers (Linux)
// ---------------------------------------------------------------------------

/// Metadata for a PipeWire audio application discovered via `pw-dump`.
/// NOTE: Superseded by rsac::AudioSource. Kept for reference.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
struct PipeWireApp {
    id: String,
    name: String,
    pid: u32,
}
