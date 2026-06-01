//! Audio capture manager — wraps rsac for multi-source audio capture.
//!
//! Responsibilities:
//! - Enumerate audio devices and applications via rsac
//! - Start/stop capture sessions
//! - Tag audio buffers with source ID and wall-clock time
//! - Forward tagged buffers to the processing pipeline via crossbeam channel

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{SendTimeoutError, Sender};
use rsac::{
    AudioCaptureBuilder, AudioDevice, AudioFormat, CaptureTarget, SampleFormat,
    get_device_enumerator,
};
use tauri::AppHandle;

use crate::events::{
    CAPTURE_BACKPRESSURE, CAPTURE_ERROR, CaptureBackpressurePayload, CaptureErrorPayload,
    emit_or_log,
};
use crate::state::{AudioSourceInfo, AudioSourceType};

// ---------------------------------------------------------------------------
// AudioChunk — tagged audio data flowing through the pipeline
// ---------------------------------------------------------------------------

/// A chunk of captured audio data tagged with its source and timestamp.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    /// Identifier of the capture source that produced this chunk.
    ///
    /// `Arc<str>` (not `String`): on the realtime audio path this id is cloned
    /// once per chunk into the [`ProcessedAudioChunk`] and again per emitted
    /// chunk, so a shared refcount bump is far cheaper than a heap alloc + copy
    /// (FA-4b). It is the same logical id for every chunk from one source.
    pub source_id: Arc<str>,
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
    /// Set by the capture thread on *any* exit — clean stop, fatal stream
    /// error, or a panic (caught via `catch_unwind`). A handle whose thread has
    /// finished is "dead": [`AudioCaptureManager::active_captures`] hides it and
    /// [`AudioCaptureManager::start_capture`] reaps it so the same source can be
    /// restarted instead of being wedged active forever (Finding #53b).
    finished: Arc<AtomicBool>,
    source_info: AudioSourceInfo,
}

// ---------------------------------------------------------------------------
// Capture-format negotiation
// ---------------------------------------------------------------------------

/// Pure selection logic: given a device's `supported_formats()` and the
/// caller's *requested* `(sample_rate, channels)`, pick a format the device
/// actually supports.
///
/// rsac's `AudioCaptureBuilder::build()` does an **exact** match of the
/// requested `AudioFormat` against `AudioDevice::supported_formats()` and
/// hard-errors on a miss (`UnsupportedFormat`). Real Windows devices expose
/// wildly different native formats (e.g. a virtual surround render endpoint
/// only advertises `8ch / 96000Hz`, a USB mic only `1ch / 48000Hz`), so a
/// fixed `48000 / 1 / F32` request fails on most of them. The downstream
/// pipeline resamples + downmixes to 16 kHz mono regardless, so we just need
/// *any* format the device supports — preferring something close to what was
/// requested and an `f32` sample type (rsac delivers f32 buffers anyway).
///
/// Preference order:
/// 1. Exact requested format (`req_sr`, `req_ch`, F32).
/// 2. F32 at the requested sample rate (any channel count).
/// 3. F32 at the requested channel count (any sample rate).
/// 4. Any F32 format (lowest channel count first — cheaper to downmix).
/// 5. The device's first advertised format (last resort).
///
/// Returns `None` only when `formats` is empty (caller then falls back to the
/// requested values and lets `build()` decide).
fn choose_capture_format(formats: &[AudioFormat], req_sr: u32, req_ch: u16) -> Option<AudioFormat> {
    if formats.is_empty() {
        return None;
    }

    let exact = AudioFormat {
        sample_rate: req_sr,
        channels: req_ch,
        sample_format: SampleFormat::F32,
    };
    if formats.contains(&exact) {
        return Some(exact);
    }

    let is_f32 = |f: &&AudioFormat| f.sample_format == SampleFormat::F32;

    if let Some(f) = formats
        .iter()
        .filter(is_f32)
        .find(|f| f.sample_rate == req_sr)
    {
        return Some(f.clone());
    }
    if let Some(f) = formats.iter().filter(is_f32).find(|f| f.channels == req_ch) {
        return Some(f.clone());
    }
    if let Some(f) = formats.iter().filter(is_f32).min_by_key(|f| f.channels) {
        return Some(f.clone());
    }

    formats.first().cloned()
}

/// Resolve the target device via rsac and negotiate a supported capture
/// format. Returns `None` when the device can't be introspected (caller then
/// uses the requested values verbatim and lets `build()` surface any error).
///
/// For `Device` targets the specific device is looked up by id; for
/// system/application targets the default (output/loopback) device is used,
/// mirroring what `AudioCaptureBuilder::build()` does internally.
fn negotiate_capture_format(
    target: &CaptureTarget,
    req_sr: u32,
    req_ch: u16,
) -> Option<AudioFormat> {
    let enumerator = get_device_enumerator().ok()?;
    // rsac v0.4.0 (CI-pinned at the `v0.4.0` tag) renamed the enumerator's default
    // accessor to `default_device()`; the old `get_default_device()` is now a
    // `#[deprecated]` alias. We call the current name directly.
    let device: Box<dyn AudioDevice> = match target {
        CaptureTarget::Device(id) => {
            let devices = enumerator.enumerate_devices().ok()?;
            devices.into_iter().find(|d| d.id() == *id)?
        }
        _ => enumerator.default_device().ok()?,
    };
    choose_capture_format(&device.supported_formats(), req_sr, req_ch)
}

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
                // A finished-but-not-yet-reaped handle is dead, so report the
                // source as inactive (Finding #53b) — consistent with
                // active_captures().
                let is_active = self
                    .sources
                    .get(&src.id)
                    .is_some_and(|h| !h.finished.load(Ordering::Acquire));
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
                    // `rsac::AudioSourceKind` is `#[non_exhaustive]`, so out-of-crate
                    // matches must carry a wildcard. Any future variant we don't yet
                    // map degrades to SystemDefault for source-info display.
                    _ => {
                        log::warn!(
                            "Unknown rsac::AudioSourceKind variant; mapping to SystemDefault"
                        );
                        AudioSourceType::SystemDefault
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
        // Reap a dead handle for this source first: if a previous capture
        // thread exited (clean stop we never observed, fatal stream error, or a
        // panic), its handle lingers in the map with `finished` set. Without
        // this, the source is wedged "already being captured" forever and can
        // never be restarted (Finding #53b).
        if let Some(existing) = self.sources.get(source_id) {
            if existing.finished.load(Ordering::Acquire) {
                self.sources.remove(source_id);
            } else {
                return Err(format!("Source '{}' is already being captured", source_id));
            }
        }

        let stop_signal = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop_signal);
        let finished = Arc::new(AtomicBool::new(false));
        let finished_clone = Arc::clone(&finished);
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
            // `CaptureTarget` is `#[non_exhaustive]`, so out-of-crate matches must
            // carry a wildcard. Any future target kind we don't yet map degrades to
            // SystemDefault for source-type display.
            _ => {
                log::warn!("Unknown CaptureTarget variant; mapping to SystemDefault");
                AudioSourceType::SystemDefault
            }
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
                let panic_sid = sid.clone();
                let panic_app = app_handle.clone();
                // Catch a panic in the capture body so a wedged thread cannot
                // leave the source marked active forever (Finding #53b). The
                // body owns the `!Sync` AudioCapture; `AssertUnwindSafe` is
                // sound here because on unwind we drop everything and only emit
                // an event — we never observe partially-mutated shared state.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    Self::capture_thread_fn(
                        sid,
                        target,
                        stop_clone,
                        pipeline_tx,
                        app_handle,
                        sample_rate,
                        channels,
                    );
                }));
                if result.is_err() {
                    log::error!("[capture-{}] Capture thread panicked", panic_sid);
                    emit_or_log(
                        &panic_app,
                        CAPTURE_ERROR,
                        CaptureErrorPayload {
                            source_id: panic_sid,
                            error: "capture thread panicked".to_string(),
                            // Not recoverable in-thread; the source is now dead
                            // and must be restarted by the user.
                            recoverable: false,
                        },
                    );
                }
                // Mark the handle dead on ANY exit (clean, error, or panic) so
                // active_captures() stops reporting it and start_capture() can
                // reap it for a restart.
                finished_clone.store(true, Ordering::Release);
            })
            .map_err(|e| format!("Failed to spawn capture thread: {}", e))?;

        self.sources.insert(
            source_id.to_string(),
            CaptureHandle {
                thread: Some(thread),
                stop_signal,
                finished,
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
    ///
    /// A handle whose capture thread has already exited (clean stop, fatal
    /// stream error, or a panic — see [`CaptureHandle::finished`]) is **not**
    /// reported: it is dead and waiting to be reaped, so surfacing it would show
    /// the UI a phantom "Running" source (Finding #53b).
    pub fn active_captures(&self) -> Vec<String> {
        self.sources
            .iter()
            .filter(|(_, h)| !h.finished.load(Ordering::Acquire))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Test-only: insert a synthetic handle (no real rsac capture / thread) so
    /// the lifecycle bookkeeping — finished-flag filtering and dead-handle
    /// reaping (Finding #53b) — can be exercised without audio hardware.
    #[cfg(test)]
    fn insert_synthetic_handle(&mut self, source_id: &str, finished: bool) -> Arc<AtomicBool> {
        let finished_flag = Arc::new(AtomicBool::new(finished));
        self.sources.insert(
            source_id.to_string(),
            CaptureHandle {
                thread: None,
                stop_signal: Arc::new(AtomicBool::new(false)),
                finished: Arc::clone(&finished_flag),
                source_info: AudioSourceInfo {
                    id: source_id.to_string(),
                    name: source_id.to_string(),
                    source_type: AudioSourceType::SystemDefault,
                    is_active: true,
                },
            },
        );
        finished_flag
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

        // 1. Negotiate a format the device actually supports. rsac does an
        //    exact-match on the requested format and hard-errors otherwise, so
        //    we query the device's supported_formats() up front and pick the
        //    closest supported one. The pipeline resamples to 16 kHz mono
        //    downstream, so the captured format only needs to be *capturable*.
        let (cap_sr, cap_ch, cap_fmt) =
            match negotiate_capture_format(&target, sample_rate, channels) {
                Some(f) => {
                    if f.sample_rate != sample_rate || f.channels != channels {
                        log::info!(
                            "[capture-{}] Requested {} Hz / {} ch not supported by device; \
                             negotiated {} Hz / {} ch / {:?}",
                            source_id,
                            sample_rate,
                            channels,
                            f.sample_rate,
                            f.channels,
                            f.sample_format
                        );
                    }
                    (f.sample_rate, f.channels, f.sample_format)
                }
                None => {
                    log::warn!(
                        "[capture-{}] Could not introspect device formats; using requested \
                         {} Hz / {} ch",
                        source_id,
                        sample_rate,
                        channels
                    );
                    (sample_rate, channels, SampleFormat::F32)
                }
            };

        let mut capture = match AudioCaptureBuilder::new()
            .with_target(target)
            .sample_rate(cap_sr)
            .channels(cap_ch)
            .sample_format(cap_fmt)
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

        // 3. Subscribe to push-based audio delivery via the *error-carrying*
        //    channel. `subscribe()` forwards only `Ok(buffer)` and drops the
        //    terminal `AudioError` on the floor — a fatal device-death
        //    (`StreamEnded`: unplug/format change) is then indistinguishable
        //    from a clean stop, so the UI stays "Running" with no audio and no
        //    toast (Finding #52). `subscribe_with_errors()` delivers each item
        //    as `AudioResult<AudioBuffer>`: a fatal terminal error arrives as
        //    the final `Err` *before* the channel disconnects, and recoverable
        //    hiccups arrive as non-terminal `Err`s without ending the stream.
        let rx = match capture.subscribe_with_errors() {
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

        // Build the chunk's source id once: every chunk from this thread carries
        // the same id, so we share one `Arc<str>` and refcount-bump it per chunk
        // instead of heap-allocating a fresh `String` each iteration (FA-4b).
        let source_id_arc: Arc<str> = Arc::from(source_id.as_str());

        // Edge-triggered backpressure tracking. We poll rsac's windowed
        // `backpressure_report()` (v0.4.0): it carries the legacy
        // consecutive-drop `is_under_backpressure` bool AND a `drop_rate` over a
        // ~10 s sliding window. We trip on EITHER the bool OR sustained partial
        // loss (`drop_rate >= DROP_RATE_TRIP`) — the latter catches steady
        // 1-in-N dropping that the all-or-nothing bool (which resets on any
        // successful push) misses entirely. Emitting only on transitions keeps
        // the frontend's enter/leave signal clean (no event storm). The trip is
        // a strict superset of the old bool, so it never regresses.
        const DROP_RATE_TRIP: f64 = 0.05;
        let mut last_backpressured = false;
        // Cheap rate limiter: poll every 10 iterations (~50ms at 48kHz / 5ms
        // buffers). `backpressure_report()` is a lock-free, alloc-free
        // consumer-side read (a Relaxed pass over a few atomics), so it's safe
        // at this cadence; this just keeps the hot path tight.
        let mut poll_counter: u32 = 0;

        // Short send timeout: if the pipeline consumer stalls (e.g. a panicked
        // downstream stage that hasn't dropped the receiver yet), a *blocking*
        // `pipeline_tx.send` on the bounded(64) channel would wedge this thread
        // forever — `stop_capture` then spins its 3 s deadline, gives up, and
        // LEAKS the thread + the rsac stream. With a timeout we re-check
        // `stop_signal` on every stall, so the thread is always reclaimable
        // (Finding #53a). The dropped chunk is acceptable: the pipeline is
        // already behind and resampling is lossy downstream anyway.
        const SEND_TIMEOUT: Duration = Duration::from_millis(100);

        // 4. Read loop — exit when stop_signal is set or channel closes.
        while !stop_signal.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(buffer)) => {
                    let chunk = AudioChunk {
                        source_id: Arc::clone(&source_id_arc),
                        data: buffer.data().to_vec(),
                        sample_rate: buffer.sample_rate(),
                        channels: buffer.channels(),
                        num_frames: buffer.num_frames(),
                        timestamp: Some(start_time.elapsed()),
                    };
                    // Non-blocking-ish send: retry on timeout (re-checking the
                    // stop signal each time) so a stalled consumer can never
                    // pin this thread.
                    let mut pending = Some(chunk);
                    while let Some(c) = pending.take() {
                        if stop_signal.load(Ordering::Relaxed) {
                            break;
                        }
                        match pipeline_tx.send_timeout(c, SEND_TIMEOUT) {
                            Ok(()) => {}
                            Err(SendTimeoutError::Timeout(_)) => {
                                // Consumer is stalled; drop this chunk and move
                                // on rather than block. (Do not stash it back —
                                // holding a chunk indefinitely just relocates
                                // the wedge.)
                                log::warn!(
                                    "[capture-{}] Pipeline send timed out — \
                                     consumer stalled, dropping chunk",
                                    source_id
                                );
                            }
                            Err(SendTimeoutError::Disconnected(_)) => {
                                log::warn!(
                                    "[capture-{}] Pipeline channel closed, exiting",
                                    source_id
                                );
                                // Mirror the Ok-path teardown below.
                                log::info!("[capture-{}] Stopping capture", source_id);
                                let _ = capture.stop();
                                log::info!("[capture-{}] Thread exiting", source_id);
                                return;
                            }
                        }
                    }

                    poll_counter = poll_counter.wrapping_add(1);
                    if poll_counter.is_multiple_of(10) {
                        let report = capture.backpressure_report();
                        let now_backpressured =
                            report.is_under_backpressure || report.drop_rate >= DROP_RATE_TRIP;
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
                Ok(Err(e)) => {
                    // The error-carrying channel surfaced an AudioError.
                    if e.is_fatal() {
                        // Terminal device-death (StreamEnded on unplug/format
                        // change, etc.): emit CAPTURE_ERROR so the frontend can
                        // distinguish this from a clean stop and tear down the
                        // "Running" UI (Finding #52). This is the final item the
                        // channel delivers before it disconnects.
                        let err_str = format!("{}", e);
                        log::error!(
                            "[capture-{}] Fatal stream error, exiting: {}",
                            source_id,
                            err_str
                        );
                        emit_or_log(
                            &app_handle,
                            CAPTURE_ERROR,
                            CaptureErrorPayload {
                                source_id: source_id.clone(),
                                error: err_str.clone(),
                                recoverable: crate::events::classify_capture_error(&err_str),
                            },
                        );
                        break;
                    } else {
                        // Recoverable hiccup (transient read error / over- or
                        // under-run): log and keep going — the subscription is
                        // still live and the next buffer will follow.
                        log::warn!(
                            "[capture-{}] Recoverable stream error (continuing): {}",
                            source_id,
                            e
                        );
                        continue;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // No data yet — loop back and check stop_signal.
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    // With subscribe_with_errors() a fatal terminal error is
                    // delivered as an Ok(Err(..)) item *before* this disconnect,
                    // so reaching here means a clean stop (stop_capture dropped
                    // the rsac reader) — no CAPTURE_ERROR needed (Finding #52).
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

#[cfg(test)]
mod format_negotiation_tests {
    use super::*;
    use rsac::{AudioFormat, SampleFormat};

    fn fmt(sample_rate: u32, channels: u16, sample_format: SampleFormat) -> AudioFormat {
        AudioFormat {
            sample_rate,
            channels,
            sample_format,
        }
    }

    #[test]
    fn empty_formats_returns_none() {
        assert!(choose_capture_format(&[], 48000, 1).is_none());
    }

    #[test]
    fn exact_match_is_preferred() {
        // A USB mic that natively supports 1ch/48000 (e.g. "fifine Microphone").
        let formats = [
            fmt(48000, 1, SampleFormat::F32),
            fmt(48000, 1, SampleFormat::I16),
            fmt(48000, 1, SampleFormat::I24),
        ];
        let chosen = choose_capture_format(&formats, 48000, 1).unwrap();
        assert_eq!(chosen, fmt(48000, 1, SampleFormat::F32));
    }

    #[test]
    fn falls_back_to_f32_at_requested_rate_when_channels_differ() {
        // A typical stereo device: request mono, get stereo F32 at same rate.
        let formats = [
            fmt(48000, 2, SampleFormat::F32),
            fmt(48000, 2, SampleFormat::I16),
        ];
        let chosen = choose_capture_format(&formats, 48000, 1).unwrap();
        assert_eq!(chosen, fmt(48000, 2, SampleFormat::F32));
    }

    #[test]
    fn surround_only_device_negotiates_to_its_native_format() {
        // The exact failure case: "SteelSeries Sonar - Gaming" advertises only
        // 8ch/96000. Requesting 48000/1/F32 must not error — it picks 8ch/96000 F32.
        let formats = [
            fmt(96000, 8, SampleFormat::F32),
            fmt(96000, 8, SampleFormat::I16),
            fmt(96000, 8, SampleFormat::I24),
        ];
        let chosen = choose_capture_format(&formats, 48000, 1).unwrap();
        assert_eq!(chosen, fmt(96000, 8, SampleFormat::F32));
    }

    #[test]
    fn prefers_f32_over_int_formats() {
        // Device exposes the requested rate only in non-F32; an F32 at another
        // rate should still win (rsac delivers f32 buffers).
        let formats = [
            fmt(44100, 2, SampleFormat::I16),
            fmt(96000, 2, SampleFormat::F32),
        ];
        let chosen = choose_capture_format(&formats, 44100, 2).unwrap();
        assert_eq!(chosen.sample_format, SampleFormat::F32);
        assert_eq!(chosen, fmt(96000, 2, SampleFormat::F32));
    }

    #[test]
    fn last_resort_first_format_when_no_f32() {
        let formats = [
            fmt(44100, 2, SampleFormat::I24),
            fmt(48000, 2, SampleFormat::I16),
        ];
        let chosen = choose_capture_format(&formats, 16000, 1).unwrap();
        assert_eq!(chosen, fmt(44100, 2, SampleFormat::I24));
    }

    #[test]
    fn prefers_lowest_channel_f32_when_rate_and_channels_miss() {
        // request 16000/1; device has F32 at 2ch and 6ch (different rates).
        let formats = [
            fmt(48000, 6, SampleFormat::F32),
            fmt(44100, 2, SampleFormat::F32),
        ];
        let chosen = choose_capture_format(&formats, 16000, 1).unwrap();
        assert_eq!(chosen.channels, 2);
    }
}

#[cfg(test)]
mod handle_lifecycle_tests {
    //! Lifecycle bookkeeping for capture handles (Finding #53b): a thread that
    //! has exited (clean stop, fatal stream error, or a panic) must not leave
    //! the source wedged "active" forever — it should drop out of
    //! `active_captures()` / `list_sources()` and be reapable for a restart.
    use super::*;

    #[test]
    fn active_captures_excludes_finished_handle() {
        let mut mgr = AudioCaptureManager::new();
        mgr.insert_synthetic_handle("live", false);
        mgr.insert_synthetic_handle("dead", true);

        let active = mgr.active_captures();
        assert!(
            active.contains(&"live".to_string()),
            "a live source must be reported active"
        );
        assert!(
            !active.contains(&"dead".to_string()),
            "a finished (dead) source must NOT be reported active — it is a \
             phantom 'Running' otherwise (Finding #53b)"
        );
    }

    #[test]
    fn active_captures_flips_when_thread_marks_finished() {
        // Models the thread setting `finished` on any exit (incl. catch_unwind
        // on panic): active_captures() must reflect the transition.
        let mut mgr = AudioCaptureManager::new();
        let flag = mgr.insert_synthetic_handle("src", false);
        assert_eq!(mgr.active_captures(), vec!["src".to_string()]);

        flag.store(true, Ordering::Release);
        assert!(
            mgr.active_captures().is_empty(),
            "once the thread marks the handle finished, the source is no longer active"
        );
    }

    #[test]
    fn list_sources_marks_finished_handle_inactive() {
        // list_sources() overlays active state from self.sources; a finished
        // handle must read as inactive even though the key is still present.
        let mut mgr = AudioCaptureManager::new();
        let flag = mgr.insert_synthetic_handle("system-default", false);

        let before = mgr.list_sources();
        let entry = before.iter().find(|s| s.id == "system-default");
        if let Some(e) = entry {
            assert!(e.is_active, "a live capture reads as active");
        }

        flag.store(true, Ordering::Release);
        let after = mgr.list_sources();
        if let Some(e) = after.iter().find(|s| s.id == "system-default") {
            assert!(
                !e.is_active,
                "a finished handle must read as inactive in list_sources (Finding #53b)"
            );
        }
        // (If rsac enumeration returns no 'system-default' on the CI box the
        // overlay path is still exercised via active_captures tests above.)
    }
}
