//! Audio capture manager — wraps rsac for multi-source audio capture.
//!
//! Responsibilities:
//! - Enumerate audio devices and applications via rsac
//! - Start/stop capture sessions
//! - Tag audio buffers with source ID, source position, and capture loss
//! - Forward tagged buffers to the processing pipeline via crossbeam channel

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{SendTimeoutError, Sender, TryRecvError};
use rsac::{
    AudioBuffer, AudioCapture, AudioCaptureBuilder, AudioDevice, AudioError, AudioFormat,
    AudioResult, CaptureTarget, SampleFormat, get_device_enumerator,
};
// rsac 0.5.0 `hotplug-recovery`: Windows/Linux only — the recovery symbols
// (and every call site below) are compiled out on macOS, where rsac withholds
// the feature (rsac ADR-0021) and `with_auto_recovery` would fail preflight.
#[cfg(any(target_os = "windows", target_os = "linux"))]
use rsac::{AutoRecoveryConfig, RecoveryEvent};
use tauri::AppHandle;

use crate::audio::pipeline::AudioPipelineInput;
use crate::events::{
    CAPTURE_BACKPRESSURE, CAPTURE_ERROR, CaptureBackpressurePayload, CaptureErrorPayload,
    emit_or_log,
};
use crate::state::{
    AudioChannelProvenanceKind, AudioDeviceKind, AudioFormatInfo, AudioPermissionKind,
    AudioPermissionRecoveryAction, AudioPermissionRecoveryActionKind, AudioPermissionRecoveryHint,
    AudioPermissionRecoveryPlatform, AudioPermissionStatus, AudioSampleFormat,
    AudioSourceCapabilities, AudioSourceChannelProvenance, AudioSourceInfo, AudioSourceType,
};

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
    /// rsac's first-frame timestamp, or capture-thread elapsed time as a fallback.
    pub timestamp: Option<Duration>,
    pub metadata: CaptureAudioMetadata,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CaptureTimestampOrigin {
    #[default]
    Rsac,
    ElapsedFallback,
}

/// Newly observed loss at each capture delivery layer. The rsac counters
/// identify counts, but cannot identify the lost buffers' timestamps or sizes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureBufferDrops {
    pub ring_buffers: u64,
    pub subscriber_buffers: u64,
    pub pipeline_buffers: u64,
    pub pipeline_input_frames: u64,
}

impl CaptureBufferDrops {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn saturating_accumulate(&mut self, other: Self) {
        self.ring_buffers = self.ring_buffers.saturating_add(other.ring_buffers);
        self.subscriber_buffers = self
            .subscriber_buffers
            .saturating_add(other.subscriber_buffers);
        self.pipeline_buffers = self.pipeline_buffers.saturating_add(other.pipeline_buffers);
        self.pipeline_input_frames = self
            .pipeline_input_frames
            .saturating_add(other.pipeline_input_frames);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CaptureAudioMetadata {
    pub timestamp_origin: CaptureTimestampOrigin,
    pub stream_generation: u64,
    pub drops: CaptureBufferDrops,
}

impl AudioChunk {
    fn from_rsac_buffer(
        source_id: Arc<str>,
        buffer: AudioBuffer,
        elapsed: Duration,
        drops: CaptureBufferDrops,
    ) -> Self {
        let source_timestamp = buffer.timestamp();
        let timestamp_origin = if source_timestamp.is_some() {
            CaptureTimestampOrigin::Rsac
        } else {
            CaptureTimestampOrigin::ElapsedFallback
        };
        Self {
            source_id,
            sample_rate: buffer.sample_rate(),
            channels: buffer.channels(),
            num_frames: buffer.num_frames(),
            timestamp: Some(source_timestamp.unwrap_or(elapsed)),
            metadata: CaptureAudioMetadata {
                timestamp_origin,
                stream_generation: buffer.stream_generation(),
                drops,
            },
            data: buffer.into_data(),
        }
    }
}

// ---------------------------------------------------------------------------
// CaptureHandle — per-source bookkeeping
// ---------------------------------------------------------------------------

/// Handle to a committed audio capture thread.
struct CaptureHandle {
    thread: Option<JoinHandle<()>>,
    stop_signal: Arc<AtomicBool>,
    startup_acknowledged: Arc<AtomicBool>,
    /// Set as soon as the worker's stream is lost, and again when its thread
    /// exits: clean stop, fatal stream error, or a panic (caught via
    /// `catch_unwind`). A dead handle is hidden by
    /// [`AudioCaptureManager::active_captures`], while fatal-exit
    /// reconciliation retains ownership until the normal Stop path has closed
    /// the session lifecycle (Finding #53b).
    finished: Arc<AtomicBool>,
    source_info: AudioSourceInfo,
}

/// Closed, content-free reason a capture start failed or a stream was lost.
/// rsac errors are mapped here structurally; their text (device and
/// application ids, driver and OS messages) never leaves
/// [`capture_failure_reason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureFailureReason {
    InvalidSource,
    PermissionDenied,
    DeviceUnavailable,
    Unsupported,
    BackendUnavailable,
    StartupTimeout,
    WorkerExited,
    Cancelled,
    AlreadyRunning,
    PreviousCaptureStopping,
    PreviousCaptureReconciling,
}

impl CaptureFailureReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidSource => "invalid_source",
            Self::PermissionDenied => "permission_denied",
            Self::DeviceUnavailable => "device_unavailable",
            Self::Unsupported => "unsupported",
            Self::BackendUnavailable => "backend_unavailable",
            Self::StartupTimeout => "startup_timeout",
            Self::WorkerExited => "worker_exited",
            Self::Cancelled => "cancelled",
            Self::AlreadyRunning => "already_running",
            Self::PreviousCaptureStopping => "previous_capture_stopping",
            Self::PreviousCaptureReconciling => "previous_capture_reconciling",
        }
    }

    /// Fixed English text for the `capture-error` event; never device data.
    const fn event_text(self) -> &'static str {
        match self {
            Self::PermissionDenied => "audio capture permission was denied",
            Self::DeviceUnavailable => "the audio device is no longer available",
            Self::Unsupported => "the audio source does not support this capture",
            Self::StartupTimeout => "the audio device stopped responding",
            _ => "the audio stream failed",
        }
    }
}

impl std::fmt::Display for CaptureFailureReason {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Map an rsac error by variant only. `AudioError` is `#[non_exhaustive]`;
/// an unknown variant is a backend failure.
pub(crate) fn capture_failure_reason(error: &AudioError) -> CaptureFailureReason {
    match error {
        AudioError::PermissionDenied { .. } | AudioError::UserConsentRequired { .. } => {
            CaptureFailureReason::PermissionDenied
        }
        AudioError::DeviceNotFound { .. }
        | AudioError::DeviceNotAvailable { .. }
        | AudioError::ApplicationNotFound { .. }
        | AudioError::StreamEnded { .. } => CaptureFailureReason::DeviceUnavailable,
        AudioError::UnsupportedFormat { .. }
        | AudioError::InvalidParameter { .. }
        | AudioError::ConfigurationError { .. }
        | AudioError::PlatformNotSupported { .. } => CaptureFailureReason::Unsupported,
        AudioError::Timeout { .. } => CaptureFailureReason::StartupTimeout,
        _ => CaptureFailureReason::BackendUnavailable,
    }
}

/// What became of one requested source when a start failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CaptureSourceOutcome {
    /// It was ready, and was stopped because another source failed.
    RolledBack,
    Failed {
        reason: CaptureFailureReason,
    },
    NotAttempted,
}

/// A refused or failed capture start. Sources are identified by their
/// position in the request, never by id: ids can carry application names.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CaptureStartFailure {
    pub reason: CaptureFailureReason,
    /// The source that failed; `None` when the refusal concerns the start.
    pub source_index: Option<usize>,
    pub sources: Vec<CaptureSourceOutcome>,
}

impl CaptureStartFailure {
    /// A refusal before any source was attempted.
    pub fn refused(reason: CaptureFailureReason, source_count: usize) -> Self {
        Self {
            reason,
            source_index: None,
            sources: vec![CaptureSourceOutcome::NotAttempted; source_count],
        }
    }

    /// One request entry was rejected before any worker was spawned.
    pub fn invalid_source(index: usize, source_count: usize) -> Self {
        let mut failure = Self::refused(CaptureFailureReason::InvalidSource, source_count);
        failure.source_index = Some(index);
        failure.sources[index] = CaptureSourceOutcome::Failed {
            reason: CaptureFailureReason::InvalidSource,
        };
        failure
    }

    /// Source `index` failed after every earlier source became ready; those
    /// were rolled back and the later ones never attempted.
    fn during_prepare(index: usize, reason: CaptureFailureReason, source_count: usize) -> Self {
        let sources = (0..source_count)
            .map(|position| match position.cmp(&index) {
                std::cmp::Ordering::Less => CaptureSourceOutcome::RolledBack,
                std::cmp::Ordering::Equal => CaptureSourceOutcome::Failed { reason },
                std::cmp::Ordering::Greater => CaptureSourceOutcome::NotAttempted,
            })
            .collect();
        Self {
            reason,
            source_index: (reason != CaptureFailureReason::Cancelled).then_some(index),
            sources,
        }
    }

    /// Every source was ready; the commit failed at `index` (or was cancelled).
    fn during_commit(index: Option<usize>, reason: CaptureFailureReason, count: usize) -> Self {
        let mut sources = vec![CaptureSourceOutcome::RolledBack; count];
        if let Some(index) = index {
            sources[index] = CaptureSourceOutcome::Failed { reason };
        }
        Self {
            reason,
            source_index: index,
            sources,
        }
    }
}

/// A worker's startup report: Ready with its negotiated format, or why not.
type StartupReport = Result<Option<AudioFormatInfo>, CaptureFailureReason>;

type CaptureWorkerBody = dyn Fn(CaptureThreadContext) -> bool + Send + Sync;
type CaptureExitReconciler = dyn Fn(AppHandle, String, Arc<AtomicBool>) + Send + Sync;

/// The replaceable parts of a capture worker. Production uses the rsac body,
/// the command-layer reconciliation and fixed bounds; tests swap in a
/// scripted body so the manager, rollback, ledger and gates stay real.
#[derive(Clone)]
pub(crate) struct CaptureWorkerHooks {
    body: Arc<CaptureWorkerBody>,
    reconcile_exit: Arc<CaptureExitReconciler>,
    startup_timeout: Duration,
    join_timeout: Duration,
}

impl Default for CaptureWorkerHooks {
    fn default() -> Self {
        Self {
            body: Arc::new(AudioCaptureManager::capture_thread_fn),
            reconcile_exit: Arc::new(crate::commands::schedule_capture_exit_reconciliation),
            startup_timeout: CAPTURE_STARTUP_TIMEOUT,
            join_timeout: CAPTURE_JOIN_TIMEOUT,
        }
    }
}

/// Marks a worker's stream dead and requests lifecycle reconciliation at most
/// once. A fatal stream reports the loss before tearing rsac down, so a
/// teardown wedged in an OS call still leaves the source inactive and
/// reconciled.
#[derive(Clone)]
struct StreamLoss {
    finished: Arc<AtomicBool>,
    startup_acknowledged: Arc<AtomicBool>,
    reconciliation_requested: Arc<AtomicBool>,
    reconcile: Arc<dyn Fn(Arc<AtomicBool>) + Send + Sync>,
}

impl StreamLoss {
    fn report(&self) {
        self.finished.store(true, Ordering::Release);
        // Before commit the command still owns the outcome and reports it.
        if self.startup_acknowledged.load(Ordering::Acquire)
            && !self.reconciliation_requested.swap(true, Ordering::AcqRel)
        {
            (self.reconcile)(Arc::clone(&self.finished));
        }
    }
}

/// Values owned by one capture worker for its entire rsac stream lifetime.
/// Keeping the handoff explicit makes the thread boundary auditable and avoids
/// a long positional argument list as lifecycle metadata grows.
struct CaptureThreadContext {
    /// Tags chunks; never logged (ids can carry application names).
    source_id: String,
    target: CaptureTarget,
    stop_signal: Arc<AtomicBool>,
    startup_acknowledged: Arc<AtomicBool>,
    startup_tx: Sender<StartupReport>,
    startup_commit_rx: Receiver<()>,
    commit_timeout: Duration,
    pipeline_tx: Sender<AudioPipelineInput>,
    app_handle: AppHandle,
    sample_rate: u32,
    channels: u16,
    stream_loss: StreamLoss,
}

/// Maximum time the command path waits for one capture worker to finish the
/// real rsac `build -> start -> subscribe` sequence. Spawning a thread is not a
/// successful capture start: the caller may advertise Running only after this
/// acknowledgement arrives.
const CAPTURE_STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// Bound on joining a stop-signalled worker before it is retired (and fences
/// the next capture or session boundary until it exits).
const CAPTURE_JOIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Wait for one worker's startup report, a cancel, or the deadline. Cancel is
/// a dropped (or signalled) sender, so it is sticky once requested.
fn await_capture_startup(
    startup_rx: &crossbeam_channel::Receiver<StartupReport>,
    cancel_rx: &crossbeam_channel::Receiver<()>,
    timeout: Duration,
) -> StartupReport {
    crossbeam_channel::select! {
        recv(startup_rx) -> report => report.unwrap_or(Err(CaptureFailureReason::WorkerExited)),
        recv(cancel_rx) -> _ => Err(CaptureFailureReason::Cancelled),
        default(timeout) => Err(CaptureFailureReason::StartupTimeout),
    }
}

fn cancel_requested(cancel_rx: &crossbeam_channel::Receiver<()>) -> bool {
    !matches!(cancel_rx.try_recv(), Err(TryRecvError::Empty))
}

fn await_capture_commit(commit_rx: &Receiver<()>, timeout: Duration) -> bool {
    commit_rx.recv_timeout(timeout).is_ok()
}

/// Report this worker's startup result, then hold a Ready stream behind the
/// commit gate. Returns true once the command committed this worker, the
/// only point from which captured audio may enter the pipeline.
fn startup_handshake(context: &CaptureThreadContext, report: StartupReport) -> bool {
    let ready = report.is_ok();
    // Zero capacity: this returns only once the command consumed the report,
    // and fails if the command stopped waiting (timeout or cancel).
    if context.startup_tx.send(report).is_err() || !ready {
        return false;
    }
    if !await_capture_commit(&context.startup_commit_rx, context.commit_timeout) {
        return false;
    }
    context.startup_acknowledged.store(true, Ordering::Release);
    true
}

/// Join a stop-signalled worker by `deadline`, returning it if it is still
/// running so the caller can retire it.
fn join_capture_worker(handle: JoinHandle<()>, deadline: Instant) -> Option<JoinHandle<()>> {
    // std has no timed join: poll `is_finished` until the deadline.
    loop {
        if handle.is_finished() {
            if handle.join().is_err() {
                log::error!("A capture worker panicked before it was joined");
            }
            return None;
        }
        if Instant::now() >= deadline {
            return Some(handle);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Stop every prepared worker: signal all of them and close every commit
/// gate before joining any, so no worker waits on another's join. Returns
/// the ones still running at `deadline`.
fn stop_prepared_workers(captures: Vec<PreparedCapture>, deadline: Instant) -> Vec<JoinHandle<()>> {
    let threads: Vec<JoinHandle<()>> = captures
        .into_iter()
        .map(|capture| {
            capture.stop_signal.store(true, Ordering::Release);
            // Dropping the rest of `capture` closes its commit gate.
            capture.thread
        })
        .collect();
    threads
        .into_iter()
        .filter_map(|thread| join_capture_worker(thread, deadline))
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn spawn_capture_worker(
    hooks: &CaptureWorkerHooks,
    index: usize,
    request: CaptureSourceStart,
    commit_timeout: Duration,
    pipeline_tx: Sender<AudioPipelineInput>,
    app_handle: &AppHandle,
    sample_rate: u32,
    channels: u16,
) -> std::io::Result<StartingCapture> {
    let stop_signal = Arc::new(AtomicBool::new(false));
    let finished = Arc::new(AtomicBool::new(false));
    let startup_acknowledged = Arc::new(AtomicBool::new(false));
    // Zero capacity makes startup a real hand-off: the worker cannot enter
    // its receive loop until this command consumed the result of
    // build -> start -> subscribe, and its report fails once we stop waiting.
    let (startup_tx, startup_rx) = crossbeam_channel::bounded(0);
    let (startup_commit_tx, startup_commit_rx) = std::sync::mpsc::sync_channel(0);
    let source_info = source_info_for_capture_start(
        &request.source_id,
        &request.target,
        request.descriptor,
        true,
    );

    let reconcile_exit = Arc::clone(&hooks.reconcile_exit);
    let reconcile_app = app_handle.clone();
    let reconcile_source = request.source_id.clone();
    let stream_loss = StreamLoss {
        finished: Arc::clone(&finished),
        startup_acknowledged: Arc::clone(&startup_acknowledged),
        reconciliation_requested: Arc::new(AtomicBool::new(false)),
        reconcile: Arc::new(move |finished| {
            reconcile_exit(reconcile_app.clone(), reconcile_source.clone(), finished)
        }),
    };
    let context = CaptureThreadContext {
        source_id: request.source_id.clone(),
        target: request.target,
        stop_signal: Arc::clone(&stop_signal),
        startup_acknowledged: Arc::clone(&startup_acknowledged),
        startup_tx,
        startup_commit_rx,
        commit_timeout,
        pipeline_tx,
        app_handle: app_handle.clone(),
        sample_rate,
        channels,
        stream_loss: stream_loss.clone(),
    };
    let body = Arc::clone(&hooks.body);
    let panic_app = app_handle.clone();
    let panic_source = request.source_id.clone();
    let panic_acknowledged = Arc::clone(&startup_acknowledged);
    let thread = std::thread::Builder::new()
        .name("audio-capture".to_string())
        .spawn(move || {
            // Catch a panic in the capture body so a wedged thread cannot
            // leave the source marked active forever (Finding #53b). The body
            // owns the `!Sync` AudioCapture; `AssertUnwindSafe` is sound here
            // because on unwind we drop everything and only emit an event —
            // we never observe partially-mutated shared state.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(context)));
            if result.is_err() {
                log::error!("A capture worker panicked");
                // Before commit the start command reports the failure.
                if panic_acknowledged.load(Ordering::Acquire) {
                    emit_or_log(
                        &panic_app,
                        CAPTURE_ERROR,
                        CaptureErrorPayload {
                            source_id: panic_source,
                            error: "capture thread panicked".to_string(),
                            recoverable: false,
                        },
                    );
                }
            }
            // Mark the handle dead on ANY exit (clean, error, or panic) so
            // active_captures() stops reporting it; an unexpected exit also
            // requests lifecycle reconciliation, at most once.
            if result.unwrap_or(true) {
                stream_loss.report();
            } else {
                stream_loss.finished.store(true, Ordering::Release);
            }
        })?;
    Ok(StartingCapture {
        capture: PreparedCapture {
            index,
            source_id: request.source_id,
            thread,
            stop_signal,
            startup_commit_tx: Some(startup_commit_tx),
            startup_acknowledged,
            finished,
            source_info,
        },
        startup_rx,
    })
}

/// Prepare every requested source in order: spawn its worker and wait for
/// the rsac build -> start -> subscribe acknowledgement. The quorum is every
/// source: on the first failure the rest are not attempted and every Ready
/// worker is stopped and joined (or returned for retirement). Blocking: run
/// it off the async runtime, without the manager lock.
pub(crate) fn prepare_capture_sources(
    ticket: &CaptureStartTicket,
    requests: Vec<CaptureSourceStart>,
    pipeline_tx: Sender<AudioPipelineInput>,
    app_handle: AppHandle,
    sample_rate: u32,
    channels: u16,
) -> Result<PreparedCaptures, CaptureStartRollback> {
    let hooks = &ticket.hooks;
    let count = requests.len();
    // A Ready worker waits behind its commit gate while later sources start.
    let commit_timeout = hooks
        .startup_timeout
        .saturating_mul(u32::try_from(count).unwrap_or(u32::MAX).saturating_add(1));
    let mut ready = Vec::with_capacity(count);
    for (index, request) in requests.into_iter().enumerate() {
        let label = capture_log_label(&request.target);
        let (reason, starting) = if cancel_requested(&ticket.cancel_rx) {
            (CaptureFailureReason::Cancelled, None)
        } else {
            match spawn_capture_worker(
                hooks,
                index,
                request,
                commit_timeout,
                pipeline_tx.clone(),
                &app_handle,
                sample_rate,
                channels,
            ) {
                Err(_) => (CaptureFailureReason::BackendUnavailable, None),
                Ok(starting) => match await_capture_startup(
                    &starting.startup_rx,
                    &ticket.cancel_rx,
                    hooks.startup_timeout,
                ) {
                    // A panic right after the report must not count as Ready.
                    Ok(negotiated) if !starting.capture.finished.load(Ordering::Acquire) => {
                        let mut capture = starting.capture;
                        apply_negotiated_channel_format(&mut capture.source_info, negotiated);
                        log::info!("[capture {label}] Ready; awaiting commit");
                        ready.push(capture);
                        continue;
                    }
                    Ok(_) => (CaptureFailureReason::WorkerExited, Some(starting)),
                    Err(reason) => (reason, Some(starting)),
                },
            }
        };
        log::warn!("[capture {label}] Start failed: {reason}");
        if let Some(StartingCapture {
            capture,
            startup_rx,
        }) = starting
        {
            // A worker still blocked in an OS call must not become an
            // untracked live capture if it eventually succeeds: with the
            // rendezvous dropped its report fails and it stops.
            drop(startup_rx);
            ready.push(capture);
        }
        let unjoined = stop_prepared_workers(ready, Instant::now() + hooks.join_timeout);
        return Err(CaptureStartRollback {
            failure: CaptureStartFailure::during_prepare(index, reason, count),
            unjoined,
        });
    }
    Ok(PreparedCaptures { captures: ready })
}

/// Log label for a capture target: its kind, never its id.
fn capture_log_label(target: &CaptureTarget) -> &'static str {
    match target {
        CaptureTarget::SystemDefault => "system-default",
        CaptureTarget::Device(_) => "device",
        CaptureTarget::Application(_) => "application",
        CaptureTarget::ApplicationByName(_) => "application-by-name",
        CaptureTarget::ProcessTree(_) => "process-tree",
        _ => "other",
    }
}

/// Return newly observed drops and advance the counter's cumulative baseline.
fn advance_drop_counter(previous: &mut u64, current: u64) -> u64 {
    let delta = current.saturating_sub(*previous);
    *previous = current;
    delta
}

// ---------------------------------------------------------------------------
// Hotplug recovery (rsac 0.5.0 `hotplug-recovery`) — Windows/Linux only
// ---------------------------------------------------------------------------
//
// rsac 0.5.0 can supervise `SystemDefault` capture: a default-device change or
// stream invalidation reopens the stream — delivered buffers keep one
// monotonic session timeline and carry the generation of the backend stream
// that produced them — instead of ending the session with a fatal
// `StreamEnded`. The feature is withheld on macOS (rsac ADR-0021:
// `with_auto_recovery` fails `preflight()`/`build()` there with
// `PlatformNotSupported`), so `Cargo.toml` links it for `feat_linux` /
// `feat_windows` builds only and every symbol below is compiled only where
// those features exist.

/// Whether this capture opts into rsac's auto-recovery: `SystemDefault`
/// targets only. rsac's supervisor follows the *default* device, so any other
/// target (`Device`, `Application`, …) stays unsupervised (and passing a
/// recovery config for one is a `preflight` error in rsac anyway).
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn system_default_auto_recovery_applies(target: &CaptureTarget) -> bool {
    matches!(target, CaptureTarget::SystemDefault)
}

/// Edge-triggered stream-generation tracking for a supervised read loop.
///
/// rsac stamps every delivered buffer with the generation of the backend
/// stream that produced it (restarting at 0 per session). Buffers arrive
/// continuously, so the read loop observes one generation per buffer and
/// surfaces only *changes* — a reopen is reported exactly once, not once per
/// 5 ms buffer.
#[cfg(any(target_os = "windows", target_os = "linux"))]
#[derive(Debug, Default)]
struct GenerationTracker {
    last: Option<u64>,
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
impl GenerationTracker {
    /// Records one observed generation.
    ///
    /// Returns `Some((previous, current))` on the first observation and on
    /// every change, `None` while the generation is unchanged.
    fn observe(&mut self, generation: u64) -> Option<(Option<u64>, u64)> {
        if self.last == Some(generation) {
            return None;
        }
        Some((self.last.replace(generation), generation))
    }
}

/// Flat, ownable copy of one rsac [`RecoveryEvent`].
///
/// rsac's event enum is `#[non_exhaustive]` on the enum *and* on every
/// struct-like variant, so out-of-crate code can neither construct events
/// (ruling them out of unit tests) nor match them exhaustively. This local
/// enum is the pure core of the mapping: [`classify_recovery_event`]
/// flattens an event into it (wildcard arm for future variants), and
/// [`recovery_signal_report`] — fully unit-testable — decides the log level,
/// telemetry kind, and message.
#[cfg(any(target_os = "windows", target_os = "linux"))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum RecoverySignal {
    /// The system default device changed; the supervisor may reopen.
    DefaultChanged,
    /// The backend stream died; a replacement is being arranged.
    DeviceInvalidated {
        /// Diagnostic description of the invalidation.
        reason: String,
    },
    /// Another reopen attempt was scheduled.
    RetryScheduled {
        /// One-based number of the scheduled attempt.
        attempt: u32,
    },
    /// A replacement stream was published and is delivering.
    Recovered,
    /// Recovery gave up; the session will end with a fatal error.
    Exhausted {
        /// Attempts spent before recovery ended.
        attempts: u32,
        /// Diagnostic description of the terminal failure.
        reason: String,
    },
    /// A future rsac event variant this build does not know.
    Unknown,
}

/// Flattens one rsac [`RecoveryEvent`] into a [`RecoverySignal`].
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn classify_recovery_event(event: &RecoveryEvent) -> RecoverySignal {
    match event {
        RecoveryEvent::DefaultChanged { .. } => RecoverySignal::DefaultChanged,
        RecoveryEvent::DeviceInvalidated { reason, .. } => RecoverySignal::DeviceInvalidated {
            reason: reason.clone(),
        },
        RecoveryEvent::RetryScheduled { attempt, .. } => {
            RecoverySignal::RetryScheduled { attempt: *attempt }
        }
        RecoveryEvent::Recovered { .. } => RecoverySignal::Recovered,
        RecoveryEvent::Exhausted {
            attempts, reason, ..
        } => RecoverySignal::Exhausted {
            attempts: *attempts,
            reason: reason.clone(),
        },
        // `RecoveryEvent` is `#[non_exhaustive]`: unknown future variants
        // degrade to a benign, logged unknown rather than a build break.
        _ => RecoverySignal::Unknown,
    }
}

/// Log level + telemetry classification of one recovery signal, with the
/// human-readable log line. Pure std — the unit tests assert the mapping.
#[cfg(any(target_os = "windows", target_os = "linux"))]
#[derive(Debug)]
struct RecoverySignalReport {
    level: sentry::Level,
    telemetry_kind: &'static str,
    message: String,
}

/// Whether a signal leaves the session running (i.e. is not terminal).
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn recovery_signal_recoverable(signal: &RecoverySignal) -> bool {
    !matches!(signal, RecoverySignal::Exhausted { .. })
}

/// Maps a [`RecoverySignal`] to its log level, telemetry kind, and message.
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn recovery_signal_report(signal: &RecoverySignal) -> RecoverySignalReport {
    let (level, telemetry_kind, message) = match signal {
        RecoverySignal::DefaultChanged => (
            sentry::Level::Info,
            "recovery_default_changed",
            "system default capture device changed; supervisor will reopen the stream".to_string(),
        ),
        RecoverySignal::DeviceInvalidated { reason } => (
            sentry::Level::Warning,
            "recovery_device_invalidated",
            format!("capture stream invalidated ({reason}); reopening on the default device"),
        ),
        RecoverySignal::RetryScheduled { attempt } => (
            sentry::Level::Warning,
            "recovery_retry_scheduled",
            format!("default device not recovered yet; reopen attempt {attempt} scheduled"),
        ),
        RecoverySignal::Recovered => (
            sentry::Level::Info,
            "recovery_recovered",
            "replacement stream opened; capture resumed on the current default device".to_string(),
        ),
        RecoverySignal::Exhausted { attempts, reason } => (
            sentry::Level::Error,
            "recovery_exhausted",
            format!("auto-recovery gave up after {attempts} attempt(s): {reason}"),
        ),
        RecoverySignal::Unknown => (
            sentry::Level::Warning,
            "recovery_unknown",
            "unrecognized rsac recovery event (newer variant); treated as a hiccup".to_string(),
        ),
    };
    RecoverySignalReport {
        level,
        telemetry_kind,
        message,
    }
}

/// Handles one supervisor event: log line + anonymous telemetry.
///
/// Never touches the `AudioCapture` handle. The rsac CHANGELOG warns that an
/// event handler must not call back into the capture (and must not hold a
/// lock across such a call); this only logs and emits a `DiagEvent`, which
/// cannot re-enter the capture.
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn handle_recovery_event(label: &str, event: RecoveryEvent) {
    let signal = classify_recovery_event(&event);
    let report = recovery_signal_report(&signal);
    match report.level {
        sentry::Level::Error => log::error!("[capture {label}] {}", report.message),
        sentry::Level::Warning => log::warn!("[capture {label}] {}", report.message),
        _ => log::info!("[capture {label}] {}", report.message),
    }
    crate::analytics::capture_diagnostic(crate::analytics::DiagEvent {
        name: "audio.capture.recovery_event",
        category: crate::analytics::Category::Audio,
        level: report.level,
        provider: None,
        kind: Some(report.telemetry_kind),
        http_status: None,
        recoverable: Some(recovery_signal_recoverable(&signal)),
    });
}

/// Drains every queued supervisor event without blocking.
///
/// Called from the read loop (buffer and no-data paths) — `try_recv` only, so
/// a quiet queue is one cheap check and the read path never stalls. rsac's
/// queue is bounded (32) and drops the *newest* events on overflow, so a slow
/// drain degrades telemetry, never capture.
#[cfg(any(target_os = "windows", target_os = "linux"))]
fn drain_recovery_events(recovery_rx: Option<&Receiver<RecoveryEvent>>, label: &str) {
    let Some(rx) = recovery_rx else {
        return;
    };
    while let Ok(event) = rx.try_recv() {
        handle_recovery_event(label, event);
    }
}

#[cfg(test)]
mod rsac_v041_contract_tests {
    use super::*;

    fn idle_cancel() -> (
        crossbeam_channel::Sender<()>,
        crossbeam_channel::Receiver<()>,
    ) {
        crossbeam_channel::bounded(1)
    }

    #[test]
    fn startup_acknowledgement_is_received_over_the_rendezvous_channel() {
        let (tx, rx) = crossbeam_channel::bounded(0);
        let (_cancel_tx, cancel_rx) = idle_cancel();
        let worker = std::thread::spawn(move || tx.send(Ok(None)).unwrap());

        assert_eq!(
            await_capture_startup(&rx, &cancel_rx, Duration::from_secs(1)),
            Ok(None)
        );
        worker.join().unwrap();
    }

    #[test]
    fn startup_worker_error_is_returned_to_the_command_caller() {
        let (tx, rx) = crossbeam_channel::bounded(0);
        let (_cancel_tx, cancel_rx) = idle_cancel();
        let worker = std::thread::spawn(move || {
            tx.send(Err(CaptureFailureReason::PermissionDenied))
                .unwrap()
        });

        assert_eq!(
            await_capture_startup(&rx, &cancel_rx, Duration::from_secs(1)),
            Err(CaptureFailureReason::PermissionDenied)
        );
        worker.join().unwrap();
    }

    #[test]
    fn worker_exit_before_acknowledgement_is_not_reported_as_started() {
        let (tx, rx) = crossbeam_channel::bounded::<StartupReport>(0);
        let (_cancel_tx, cancel_rx) = idle_cancel();
        drop(tx);

        assert_eq!(
            await_capture_startup(&rx, &cancel_rx, Duration::from_secs(1)),
            Err(CaptureFailureReason::WorkerExited)
        );
    }

    #[test]
    fn timed_out_startup_cannot_later_acknowledge_an_untracked_capture() {
        let (tx, rx) = crossbeam_channel::bounded::<StartupReport>(0);
        let (_cancel_tx, cancel_rx) = idle_cancel();

        assert_eq!(
            await_capture_startup(&rx, &cancel_rx, Duration::ZERO),
            Err(CaptureFailureReason::StartupTimeout)
        );
        drop(rx);

        assert!(tx.send(Ok(None)).is_err());
    }

    #[test]
    fn a_cancel_is_sticky_and_ends_the_wait() {
        let (_tx, rx) = crossbeam_channel::bounded::<StartupReport>(0);
        let (cancel_tx, cancel_rx) = idle_cancel();
        assert!(!cancel_requested(&cancel_rx));
        drop(cancel_tx);

        assert!(cancel_requested(&cancel_rx));
        assert_eq!(
            await_capture_startup(&rx, &cancel_rx, Duration::from_secs(5)),
            Err(CaptureFailureReason::Cancelled)
        );
        assert!(cancel_requested(&cancel_rx), "a cancel stays requested");
    }

    #[test]
    fn rsac_errors_map_to_closed_reasons_by_variant() {
        let cases = [
            (
                AudioError::PermissionDenied {
                    operation: "canary-op".into(),
                    details: Some("canary-detail".into()),
                },
                CaptureFailureReason::PermissionDenied,
            ),
            (
                AudioError::DeviceNotFound {
                    device_id: "canary-device".into(),
                },
                CaptureFailureReason::DeviceUnavailable,
            ),
            (
                AudioError::StreamEnded {
                    reason: "canary".into(),
                },
                CaptureFailureReason::DeviceUnavailable,
            ),
            (
                AudioError::UnsupportedFormat {
                    format: "canary".into(),
                    context: None,
                },
                CaptureFailureReason::Unsupported,
            ),
            (
                AudioError::StreamStartFailed {
                    reason: "canary".into(),
                },
                CaptureFailureReason::BackendUnavailable,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(capture_failure_reason(&error), expected, "{error:?}");
        }
    }

    #[test]
    fn prepared_capture_cannot_release_audio_before_commit() {
        let (commit_tx, commit_rx) = std::sync::mpsc::sync_channel(0);
        let (waiting_tx, waiting_rx) = std::sync::mpsc::channel();
        let (audio_tx, audio_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            waiting_tx.send(()).unwrap();
            if await_capture_commit(&commit_rx, Duration::from_secs(1)) {
                audio_tx.send("released").unwrap();
            }
        });

        waiting_rx.recv().unwrap();
        assert!(
            audio_rx.try_recv().is_err(),
            "a Ready worker must remain silent before the audit commit"
        );
        commit_tx.send(()).unwrap();
        assert_eq!(
            audio_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            "released"
        );
        worker.join().unwrap();
    }

    #[test]
    fn rsac_buffer_preserves_timestamp_generation_and_observed_loss() {
        let source_position = Duration::from_millis(125);
        let fallback = Duration::from_secs(9);
        let drop_delta = CaptureBufferDrops {
            ring_buffers: 2,
            subscriber_buffers: 3,
            pipeline_buffers: 1,
            pipeline_input_frames: 480,
        };
        let buffer =
            AudioBuffer::with_timestamp(vec![0.25, 0.75], AudioFormat::default(), source_position)
                .with_stream_generation(4);
        let chunk = AudioChunk::from_rsac_buffer("mic".into(), buffer, fallback, drop_delta);

        assert_eq!(chunk.timestamp, Some(source_position));
        assert_eq!(chunk.metadata.stream_generation, 4);
        assert_eq!(chunk.metadata.drops, drop_delta);
        assert_eq!(
            chunk.metadata.timestamp_origin,
            CaptureTimestampOrigin::Rsac
        );

        let untimed = AudioChunk::from_rsac_buffer(
            "mic".into(),
            AudioBuffer::new(vec![0.5], 1, 16_000),
            fallback,
            CaptureBufferDrops::default(),
        );
        assert_eq!(untimed.timestamp, Some(fallback));
        assert_eq!(
            untimed.metadata.timestamp_origin,
            CaptureTimestampOrigin::ElapsedFallback
        );
    }

    #[test]
    fn subscriber_drop_counter_reports_only_new_drops_and_rebases_safely() {
        let mut previous = 0;

        assert_eq!(advance_drop_counter(&mut previous, 3), 3);
        assert_eq!(advance_drop_counter(&mut previous, 5), 2);
        assert_eq!(advance_drop_counter(&mut previous, 5), 0);

        // A restarted/reset producer can report a lower cumulative value. It
        // must rebase without fabricating an underflow-sized loss spike.
        assert_eq!(advance_drop_counter(&mut previous, 1), 0);
        assert_eq!(advance_drop_counter(&mut previous, 4), 3);
    }

    #[test]
    fn only_an_acknowledged_stream_loss_requests_reconciliation_once() {
        let requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&requests);
        let loss = StreamLoss {
            finished: Arc::new(AtomicBool::new(false)),
            startup_acknowledged: Arc::new(AtomicBool::new(false)),
            reconciliation_requested: Arc::new(AtomicBool::new(false)),
            reconcile: Arc::new(move |_finished| {
                counted.fetch_add(1, Ordering::SeqCst);
            }),
        };

        loss.report();
        assert!(loss.finished.load(Ordering::SeqCst));
        assert_eq!(requests.load(Ordering::SeqCst), 0, "not yet committed");

        loss.startup_acknowledged.store(true, Ordering::SeqCst);
        loss.report();
        loss.report();
        assert_eq!(requests.load(Ordering::SeqCst), 1);
    }
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

fn audio_device_kind_from_rsac(kind: rsac::DeviceKind) -> AudioDeviceKind {
    match kind {
        rsac::DeviceKind::Input => AudioDeviceKind::Input,
        rsac::DeviceKind::Output => AudioDeviceKind::Output,
    }
}

fn audio_sample_format_from_rsac(format: SampleFormat) -> AudioSampleFormat {
    match format {
        SampleFormat::I16 => AudioSampleFormat::I16,
        SampleFormat::I24 => AudioSampleFormat::I24,
        SampleFormat::I32 => AudioSampleFormat::I32,
        SampleFormat::F32 => AudioSampleFormat::F32,
    }
}

fn audio_format_info_from_rsac(format: &AudioFormat) -> AudioFormatInfo {
    AudioFormatInfo {
        sample_rate: format.sample_rate,
        channels: format.channels,
        sample_format: audio_sample_format_from_rsac(format.sample_format),
    }
}

fn audio_format_infos_from_rsac(formats: &[AudioFormat]) -> Vec<AudioFormatInfo> {
    formats.iter().map(audio_format_info_from_rsac).collect()
}

fn default_format_info(formats: &[AudioFormatInfo]) -> Option<AudioFormatInfo> {
    formats.first().cloned()
}

fn source_channel_provenance_kind(
    source_type: &AudioSourceType,
    device_kind: Option<AudioDeviceKind>,
) -> AudioChannelProvenanceKind {
    match source_type {
        AudioSourceType::SystemDefault => AudioChannelProvenanceKind::Mixed,
        AudioSourceType::Device { .. } => match device_kind {
            Some(AudioDeviceKind::Input) => AudioChannelProvenanceKind::Physical,
            Some(AudioDeviceKind::Output) | None => AudioChannelProvenanceKind::Mixed,
        },
        AudioSourceType::Application { .. }
        | AudioSourceType::ApplicationName { .. }
        | AudioSourceType::ProcessTree { .. } => AudioChannelProvenanceKind::AppProcessDerived,
    }
}

fn source_channel_provenance(
    source_type: &AudioSourceType,
    device_kind: Option<AudioDeviceKind>,
    negotiated_format: Option<AudioFormatInfo>,
) -> AudioSourceChannelProvenance {
    AudioSourceChannelProvenance::fallback_for_format(
        source_channel_provenance_kind(source_type, device_kind),
        negotiated_format,
    )
}

fn ensure_channel_provenance(source_info: &mut AudioSourceInfo) {
    if source_info.channel_provenance.is_none() {
        source_info.channel_provenance = Some(source_channel_provenance(
            &source_info.source_type,
            source_info.device_kind,
            source_info.default_format.clone(),
        ));
    }
}

fn apply_negotiated_channel_format(
    source_info: &mut AudioSourceInfo,
    negotiated_format: Option<AudioFormatInfo>,
) {
    let Some(format) = negotiated_format else {
        ensure_channel_provenance(source_info);
        return;
    };

    match source_info.channel_provenance.as_mut() {
        Some(provenance) if provenance.is_source_native_admissible() => {
            provenance.negotiated_format = Some(format);
        }
        Some(provenance) => {
            *provenance = AudioSourceChannelProvenance::fallback_for_format(
                provenance.provenance,
                Some(format),
            );
        }
        None => {
            source_info.channel_provenance = Some(source_channel_provenance(
                &source_info.source_type,
                source_info.device_kind,
                Some(format),
            ));
        }
    }
}

#[derive(Default)]
struct SourceFormatSnapshot {
    system_default: Vec<AudioFormatInfo>,
    by_device_id: HashMap<String, Vec<AudioFormatInfo>>,
}

fn collect_source_format_snapshot() -> SourceFormatSnapshot {
    let mut snapshot = SourceFormatSnapshot::default();
    let enumerator = match get_device_enumerator() {
        Ok(enumerator) => enumerator,
        Err(e) => {
            log::debug!(
                "Device format enumeration unavailable: {}",
                capture_failure_reason(&e)
            );
            return snapshot;
        }
    };

    if let Ok(default_device) = enumerator.default_device() {
        snapshot.system_default = audio_format_infos_from_rsac(&default_device.supported_formats());
    }

    match enumerator.enumerate_devices() {
        Ok(devices) => {
            for device in devices {
                snapshot.by_device_id.insert(
                    device.id().to_string(),
                    audio_format_infos_from_rsac(&device.supported_formats()),
                );
            }
        }
        Err(e) => {
            log::debug!(
                "Device format list unavailable: {}",
                capture_failure_reason(&e)
            );
        }
    }

    snapshot
}

fn audio_permission_status_from_rsac(status: rsac::PermissionStatus) -> AudioPermissionStatus {
    match status {
        rsac::PermissionStatus::Granted => AudioPermissionStatus::Granted,
        rsac::PermissionStatus::NotDetermined => AudioPermissionStatus::NotDetermined,
        rsac::PermissionStatus::Denied => AudioPermissionStatus::Denied,
        rsac::PermissionStatus::NotRequired => AudioPermissionStatus::NotRequired,
        _ => AudioPermissionStatus::Unknown,
    }
}

fn source_permission_status(source_type: &AudioSourceType) -> AudioPermissionStatus {
    match source_type {
        AudioSourceType::Application { .. }
        | AudioSourceType::ApplicationName { .. }
        | AudioSourceType::ProcessTree { .. } => {
            audio_permission_status_from_rsac(rsac::check_audio_capture_permission())
        }
        AudioSourceType::SystemDefault | AudioSourceType::Device { .. } => {
            AudioPermissionStatus::NotRequired
        }
    }
}

fn permission_recovery_platform() -> AudioPermissionRecoveryPlatform {
    #[cfg(target_os = "macos")]
    {
        AudioPermissionRecoveryPlatform::Macos
    }
    #[cfg(target_os = "linux")]
    {
        AudioPermissionRecoveryPlatform::Linux
    }
    #[cfg(target_os = "windows")]
    {
        AudioPermissionRecoveryPlatform::Windows
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        AudioPermissionRecoveryPlatform::Unknown
    }
}

fn permission_kind_for_platform(platform: AudioPermissionRecoveryPlatform) -> AudioPermissionKind {
    match platform {
        AudioPermissionRecoveryPlatform::Macos => AudioPermissionKind::AudioCapture,
        AudioPermissionRecoveryPlatform::Linux => AudioPermissionKind::PipewireAccess,
        AudioPermissionRecoveryPlatform::Windows => AudioPermissionKind::WindowsAccess,
        AudioPermissionRecoveryPlatform::Unknown => AudioPermissionKind::Unknown,
    }
}

fn permission_needs_recovery(status: AudioPermissionStatus) -> bool {
    !matches!(
        status,
        AudioPermissionStatus::Granted | AudioPermissionStatus::NotRequired
    )
}

fn source_permission_recovery(
    source_type: &AudioSourceType,
    status: AudioPermissionStatus,
) -> Option<AudioPermissionRecoveryHint> {
    if !permission_needs_recovery(status) {
        return None;
    }

    if !matches!(
        source_type,
        AudioSourceType::Application { .. }
            | AudioSourceType::ApplicationName { .. }
            | AudioSourceType::ProcessTree { .. }
    ) {
        return None;
    }

    let platform = permission_recovery_platform();
    let permission_kind = permission_kind_for_platform(platform);
    let (summary, body) = match platform {
        AudioPermissionRecoveryPlatform::Macos => match status {
            AudioPermissionStatus::Denied => (
                "macOS Audio Capture permission is denied.",
                "Grant AudioGraph permission in macOS Privacy & Security, then relaunch AudioGraph and refresh sources.",
            ),
            AudioPermissionStatus::NotDetermined => (
                "macOS Audio Capture permission has not been granted.",
                "Grant AudioGraph permission in macOS Privacy & Security, then relaunch AudioGraph and refresh sources.",
            ),
            AudioPermissionStatus::Unknown => (
                "macOS Audio Capture permission could not be checked.",
                "Review AudioGraph permissions in macOS Privacy & Security, then relaunch AudioGraph and refresh sources.",
            ),
            AudioPermissionStatus::Granted | AudioPermissionStatus::NotRequired => return None,
        },
        AudioPermissionRecoveryPlatform::Linux => (
            "Linux audio capture permission is unavailable.",
            "Review the active audio backend permissions, then relaunch AudioGraph and refresh sources.",
        ),
        AudioPermissionRecoveryPlatform::Windows => (
            "Windows audio capture permission is unavailable.",
            "Review Windows app audio permissions, then relaunch AudioGraph and refresh sources.",
        ),
        AudioPermissionRecoveryPlatform::Unknown => (
            "Audio capture permission is unavailable.",
            "Review platform audio permissions, then relaunch AudioGraph and refresh sources.",
        ),
    };

    Some(AudioPermissionRecoveryHint {
        platform,
        permission_kind,
        summary: summary.to_string(),
        body: body.to_string(),
        actions: vec![
            AudioPermissionRecoveryAction {
                kind: AudioPermissionRecoveryActionKind::GrantPermissionManually,
                label: "Grant permission manually".to_string(),
            },
            AudioPermissionRecoveryAction {
                kind: AudioPermissionRecoveryActionKind::RelaunchApp,
                label: "Relaunch AudioGraph".to_string(),
            },
            AudioPermissionRecoveryAction {
                kind: AudioPermissionRecoveryActionKind::RefreshSources,
                label: "Refresh sources".to_string(),
            },
        ],
    })
}

fn unsupported_source_reason(source_type: &AudioSourceType, backend_name: &str) -> String {
    match source_type {
        AudioSourceType::SystemDefault => {
            format!("System capture is not supported by the {backend_name} backend")
        }
        AudioSourceType::Device { .. } => {
            format!("Device selection is not supported by the {backend_name} backend")
        }
        AudioSourceType::Application { .. } | AudioSourceType::ApplicationName { .. } => {
            format!("Application capture is not supported by the {backend_name} backend")
        }
        AudioSourceType::ProcessTree { .. } => {
            format!("Process-tree capture is not supported by the {backend_name} backend")
        }
    }
}

fn source_capabilities(
    source_type: &AudioSourceType,
    caps: &rsac::PlatformCapabilities,
) -> AudioSourceCapabilities {
    let capture_supported = match source_type {
        AudioSourceType::SystemDefault => caps.supports_system_capture,
        AudioSourceType::Device { .. } => caps.supports_device_selection,
        AudioSourceType::Application { .. } | AudioSourceType::ApplicationName { .. } => {
            caps.supports_application_capture
        }
        AudioSourceType::ProcessTree { .. } => caps.supports_process_tree_capture,
    };
    AudioSourceCapabilities {
        backend_name: caps.backend_name.to_string(),
        capture_supported,
        supports_system_capture: caps.supports_system_capture,
        supports_application_capture: caps.supports_application_capture,
        supports_process_tree_capture: caps.supports_process_tree_capture,
        supports_device_selection: caps.supports_device_selection,
        supports_device_change_notifications: caps.supports_device_change_notifications,
        unsupported_reason: (!capture_supported)
            .then(|| unsupported_source_reason(source_type, caps.backend_name)),
    }
}

fn source_descriptor_parts_from_rsac_kind(
    kind: &rsac::AudioSourceKind,
    format_snapshot: &SourceFormatSnapshot,
) -> (
    AudioSourceType,
    Option<AudioDeviceKind>,
    Option<bool>,
    Vec<AudioFormatInfo>,
    String,
) {
    match kind {
        rsac::AudioSourceKind::SystemDefault => (
            AudioSourceType::SystemDefault,
            None,
            Some(true),
            format_snapshot.system_default.clone(),
            "system".to_string(),
        ),
        rsac::AudioSourceKind::Device {
            device_id,
            is_default,
            kind,
        } => (
            AudioSourceType::Device {
                device_id: device_id.clone(),
            },
            kind.map(audio_device_kind_from_rsac),
            Some(*is_default),
            format_snapshot
                .by_device_id
                .get(device_id)
                .cloned()
                .unwrap_or_default(),
            format!("device:{device_id}"),
        ),
        rsac::AudioSourceKind::Application {
            pid,
            app_name,
            bundle_id,
        } => (
            AudioSourceType::Application {
                pid: *pid,
                app_name: app_name.clone(),
                bundle_id: bundle_id.clone(),
            },
            None,
            Some(false),
            Vec::new(),
            format!("app:{pid}"),
        ),
        // `rsac::AudioSourceKind` is `#[non_exhaustive]`, so out-of-crate
        // matches must carry a wildcard. Any future variant we don't yet map
        // degrades to SystemDefault for source-info display.
        _ => {
            log::warn!("Unknown rsac::AudioSourceKind variant; mapping to SystemDefault");
            (
                AudioSourceType::SystemDefault,
                None,
                None,
                Vec::new(),
                "system".to_string(),
            )
        }
    }
}

fn source_info_for_capture_target(
    source_id: &str,
    target: &CaptureTarget,
    is_active: bool,
) -> AudioSourceInfo {
    let caps = rsac::PlatformCapabilities::query();
    let source_type = match target {
        CaptureTarget::SystemDefault => AudioSourceType::SystemDefault,
        CaptureTarget::Device(dev_id) => AudioSourceType::Device {
            device_id: dev_id.0.clone(),
        },
        CaptureTarget::Application(app_id) => AudioSourceType::Application {
            pid: app_id.0.parse::<u32>().unwrap_or(0),
            app_name: source_id.to_string(),
            bundle_id: None,
        },
        CaptureTarget::ApplicationByName(name) => AudioSourceType::ApplicationName {
            app_name: name.clone(),
        },
        CaptureTarget::ProcessTree(proc_id) => AudioSourceType::ProcessTree { pid: proc_id.0 },
        // `CaptureTarget` is `#[non_exhaustive]`, so out-of-crate matches must
        // carry a wildcard. Any future target kind we don't yet map degrades to
        // SystemDefault for source-type display.
        _ => {
            log::warn!("Unknown CaptureTarget variant; mapping to SystemDefault");
            AudioSourceType::SystemDefault
        }
    };
    let capabilities = source_capabilities(&source_type, &caps);
    let permission_status = source_permission_status(&source_type);
    let permission_recovery = source_permission_recovery(&source_type, permission_status);
    let channel_provenance = source_channel_provenance(&source_type, None, None);

    AudioSourceInfo {
        id: source_id.to_string(),
        name: source_id.to_string(),
        source_type,
        capture_target: Some(source_id.to_string()),
        device_kind: None,
        is_default: Some(matches!(target, CaptureTarget::SystemDefault)),
        supported_formats: Vec::new(),
        default_format: None,
        channel_provenance: Some(channel_provenance),
        capabilities: Some(capabilities),
        permission_status: Some(permission_status),
        permission_recovery,
        is_active,
    }
}

fn source_info_for_capture_start(
    source_id: &str,
    target: &CaptureTarget,
    source_descriptor: Option<AudioSourceInfo>,
    is_active: bool,
) -> AudioSourceInfo {
    let Some(mut source_info) = source_descriptor else {
        return source_info_for_capture_target(source_id, target, is_active);
    };

    source_info.id = source_id.to_string();
    source_info.is_active = is_active;
    if source_info.capture_target.is_none() {
        source_info.capture_target = Some(source_id.to_string());
    }
    ensure_channel_provenance(&mut source_info);
    source_info
}

fn active_handle_is_live(handle: &CaptureHandle) -> bool {
    !handle.finished.load(Ordering::Acquire)
}

fn source_is_active(
    active_sources: &HashMap<String, CaptureHandle>,
    source_id: &str,
    capture_target: &str,
) -> bool {
    active_sources
        .get(capture_target)
        .or_else(|| active_sources.get(source_id))
        .is_some_and(active_handle_is_live)
}

/// Manages multiple concurrent audio capture sources.
///
/// Each active capture runs on its own dedicated thread (required because
/// `rsac::AudioCapture` is `!Sync`). Audio data is forwarded as [`AudioChunk`]
/// values over the supplied `crossbeam_channel::Sender`.
pub struct AudioCaptureManager {
    sources: HashMap<String, CaptureHandle>,
    /// Capture threads that did not observe Stop within the bounded join.
    /// Their handles stay owned here so a new capture/session boundary can be
    /// fenced until the old rsac generation has actually exited.
    retired_threads: Vec<JoinHandle<()>>,
    hooks: CaptureWorkerHooks,
    /// The start currently preparing sources. Its workers are not owned here
    /// (nor listed, nor seen by any gate) until
    /// [`Self::commit_prepared_captures`].
    pending_start: Option<PendingCaptureStart>,
    next_attempt: u64,
    /// Attempt id of the committed aggregate: the acknowledged generation.
    generation: u64,
    /// Set by app shutdown; no start may commit afterwards.
    shutting_down: bool,
}

struct PendingCaptureStart {
    attempt: u64,
    source_ids: Vec<String>,
    /// Dropped to cancel, so a cancel is sticky and wakes the waiting start.
    cancel_tx: Option<crossbeam_channel::Sender<()>>,
}

/// A registered start. Its owner must call
/// [`AudioCaptureManager::release_capture_start`] on every exit.
pub(crate) struct CaptureStartTicket {
    pub(crate) attempt: u64,
    cancel_rx: crossbeam_channel::Receiver<()>,
    hooks: CaptureWorkerHooks,
}

/// One requested source, resolved by the command layer.
pub(crate) struct CaptureSourceStart {
    pub(crate) source_id: String,
    pub(crate) target: CaptureTarget,
    pub(crate) descriptor: Option<AudioSourceInfo>,
}

/// A worker whose rsac stream is Ready and parked behind its commit gate.
/// Dropping it closes the gate, which stops the worker.
struct PreparedCapture {
    index: usize,
    source_id: String,
    thread: JoinHandle<()>,
    stop_signal: Arc<AtomicBool>,
    startup_commit_tx: Option<SyncSender<()>>,
    startup_acknowledged: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    source_info: AudioSourceInfo,
}

/// Every requested source, Ready, in request order.
pub(crate) struct PreparedCaptures {
    captures: Vec<PreparedCapture>,
}

/// A failed prepare: the failure to report, and the stop-signalled workers
/// that did not exit within the join bound (the caller retires them).
pub(crate) struct CaptureStartRollback {
    pub(crate) failure: CaptureStartFailure,
    pub(crate) unjoined: Vec<JoinHandle<()>>,
}

/// The committed aggregate a transcription or realtime start may attach to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AcknowledgedCapture {
    pub(crate) generation: u64,
    pub(crate) sources: Vec<String>,
}

struct StartingCapture {
    capture: PreparedCapture,
    startup_rx: crossbeam_channel::Receiver<StartupReport>,
}

/// An open rsac stream, not yet releasing audio.
struct OpenedCaptureStream {
    capture: AudioCapture,
    rx: Receiver<AudioResult<AudioBuffer>>,
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    recovery_rx: Option<Receiver<RecoveryEvent>>,
    negotiated: Option<AudioFormatInfo>,
}

impl AudioCaptureManager {
    /// Create a new capture manager.
    pub fn new() -> Self {
        Self {
            sources: HashMap::new(),
            retired_threads: Vec::new(),
            hooks: CaptureWorkerHooks::default(),
            pending_start: None,
            next_attempt: 0,
            generation: 0,
            shutting_down: false,
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
                log::warn!(
                    "Failed to list audio sources via rsac: {}",
                    capture_failure_reason(&e)
                );
                let caps = rsac::PlatformCapabilities::query();
                let source_type = AudioSourceType::SystemDefault;
                let capabilities = source_capabilities(&source_type, &caps);
                let permission_status = source_permission_status(&source_type);
                let permission_recovery =
                    source_permission_recovery(&source_type, permission_status);
                let channel_provenance = source_channel_provenance(&source_type, None, None);
                // Fallback: at least return the system default
                return vec![AudioSourceInfo {
                    id: "system-default".to_string(),
                    name: "System Default".to_string(),
                    source_type,
                    capture_target: Some("system".to_string()),
                    device_kind: None,
                    is_default: Some(true),
                    supported_formats: Vec::new(),
                    default_format: None,
                    channel_provenance: Some(channel_provenance),
                    capabilities: Some(capabilities),
                    permission_status: Some(permission_status),
                    permission_recovery,
                    is_active: source_is_active(&self.sources, "system-default", "system"),
                }];
            }
        };

        let caps = rsac::PlatformCapabilities::query();
        let format_snapshot = collect_source_format_snapshot();
        let sources: Vec<AudioSourceInfo> = rsac_sources
            .into_iter()
            .map(|src| {
                let (source_type, device_kind, is_default, supported_formats, capture_target) =
                    source_descriptor_parts_from_rsac_kind(&src.kind, &format_snapshot);
                // A finished-but-not-yet-reaped handle is dead, so report the
                // source as inactive (Finding #53b). Device rows are keyed by
                // the canonical capture target (`device:<id>`), but we also
                // check rsac's opaque row id for handles started by older UI
                // state before this mapping was tightened.
                let is_active = source_is_active(&self.sources, &src.id, &capture_target);
                let default_format = default_format_info(&supported_formats);
                let channel_provenance =
                    source_channel_provenance(&source_type, device_kind, default_format.clone());
                let capabilities = source_capabilities(&source_type, &caps);
                let permission_status = source_permission_status(&source_type);
                let permission_recovery =
                    source_permission_recovery(&source_type, permission_status);
                AudioSourceInfo {
                    id: src.id.clone(),
                    name: src.name,
                    source_type,
                    capture_target: Some(capture_target),
                    device_kind,
                    is_default,
                    supported_formats,
                    default_format,
                    channel_provenance: Some(channel_provenance),
                    capabilities: Some(capabilities),
                    permission_status: Some(permission_status),
                    permission_recovery,
                    is_active,
                }
            })
            .collect();

        log::info!("Total audio sources listed: {}", sources.len());
        sources
    }

    // ----- capture lifecycle -----------------------------------------------

    /// Register a capture start before it waits for the session lifecycle,
    /// so a Stop or shutdown issued from then on cancels it. Refuses a second
    /// concurrent start.
    pub(crate) fn register_capture_start(
        &mut self,
        source_ids: Vec<String>,
    ) -> Result<CaptureStartTicket, CaptureFailureReason> {
        if self.shutting_down {
            return Err(CaptureFailureReason::Cancelled);
        }
        if self.pending_start.is_some() {
            return Err(CaptureFailureReason::AlreadyRunning);
        }
        self.next_attempt += 1;
        let (cancel_tx, cancel_rx) = crossbeam_channel::bounded(1);
        self.pending_start = Some(PendingCaptureStart {
            attempt: self.next_attempt,
            source_ids,
            cancel_tx: Some(cancel_tx),
        });
        Ok(CaptureStartTicket {
            attempt: self.next_attempt,
            cancel_rx,
            hooks: self.hooks.clone(),
        })
    }

    /// Clear `attempt`'s registration. Idempotent; called on every exit.
    pub(crate) fn release_capture_start(&mut self, attempt: u64) {
        if self
            .pending_start
            .as_ref()
            .is_some_and(|pending| pending.attempt == attempt)
        {
            self.pending_start = None;
        }
    }

    /// Cancel a pending start that includes `source_id`. Returns whether it
    /// did; the start then rolls back every source it prepared.
    pub(crate) fn cancel_pending_start(&mut self, source_id: &str) -> bool {
        match self.pending_start.as_mut() {
            Some(pending) if pending.source_ids.iter().any(|id| id == source_id) => {
                pending.cancel_tx = None;
                true
            }
            _ => false,
        }
    }

    /// Refuse a start the current ownership cannot accept, before the caller
    /// opens writers, spawns process-lifetime workers, or performs other
    /// observable preflight. A start never adds sources to a running
    /// capture, and a dead aggregate must be reconciled first so the movement
    /// ledger never records Started twice.
    pub(crate) fn validate_capture_start(&mut self) -> Result<(), CaptureFailureReason> {
        self.reap_retired_capture_workers();
        if !self.retired_threads.is_empty() {
            Err(CaptureFailureReason::PreviousCaptureStopping)
        } else if self.first_source_reconciliation_pending() {
            Err(CaptureFailureReason::PreviousCaptureReconciling)
        } else if !self.sources.is_empty() {
            Err(CaptureFailureReason::AlreadyRunning)
        } else {
            Ok(())
        }
    }

    /// Retain workers a failed start could not join within the bound; they
    /// fence the next capture or session boundary until they exit.
    pub(crate) fn retire_capture_workers(&mut self, threads: Vec<JoinHandle<()>>) {
        if !threads.is_empty() {
            log::warn!(
                "Retaining {} stop-signalled capture worker(s) until they exit",
                threads.len()
            );
        }
        self.retired_threads.extend(threads);
    }

    /// The single commit transition for `attempt`: release every prepared
    /// worker into the pipeline and take ownership of all of them, unless the
    /// start was cancelled, the app is shutting down, or a worker died. On
    /// refusal every prepared worker is stopped and joined (or retired).
    /// The caller has already recorded CaptureStarted.
    pub(crate) fn commit_prepared_captures(
        &mut self,
        attempt: u64,
        prepared: PreparedCaptures,
    ) -> Result<u64, CaptureStartFailure> {
        let mut captures = prepared.captures;
        let count = captures.len();
        let cancelled = self.shutting_down
            || !self
                .pending_start
                .as_ref()
                .is_some_and(|pending| pending.attempt == attempt && pending.cancel_tx.is_some());
        self.release_capture_start(attempt);
        let failure = if cancelled {
            Some(CaptureStartFailure::during_commit(
                None,
                CaptureFailureReason::Cancelled,
                count,
            ))
        } else {
            captures.iter_mut().find_map(|capture| {
                let released = capture
                    .startup_commit_tx
                    .take()
                    .is_some_and(|commit| commit.send(()).is_ok());
                if released {
                    // Parent-side publication closes the post-rendezvous
                    // panic window: a worker that exits after accepting
                    // commit must reconcile normally.
                    capture.startup_acknowledged.store(true, Ordering::Release);
                }
                (!released || capture.finished.load(Ordering::Acquire)).then(|| {
                    CaptureStartFailure::during_commit(
                        Some(capture.index),
                        CaptureFailureReason::WorkerExited,
                        count,
                    )
                })
            })
        };
        if let Some(failure) = failure {
            let unjoined =
                stop_prepared_workers(captures, Instant::now() + self.hooks.join_timeout);
            self.retire_capture_workers(unjoined);
            return Err(failure);
        }
        for capture in captures {
            self.sources.insert(
                capture.source_id,
                CaptureHandle {
                    thread: Some(capture.thread),
                    stop_signal: capture.stop_signal,
                    startup_acknowledged: capture.startup_acknowledged,
                    finished: capture.finished,
                    source_info: capture.source_info,
                },
            );
        }
        self.generation = attempt;
        log::info!("Committed a capture of {count} source(s)");
        Ok(attempt)
    }

    /// Stop every prepared worker of a start that cannot commit (its
    /// CaptureStarted row could not be recorded). Nothing was released.
    pub(crate) fn abort_prepared_captures(&mut self, prepared: PreparedCaptures) {
        let unjoined =
            stop_prepared_workers(prepared.captures, Instant::now() + self.hooks.join_timeout);
        self.retire_capture_workers(unjoined);
    }

    /// Whether the manager owns a handle (live or dead) for `source_id`.
    pub(crate) fn owns_capture(&self, source_id: &str) -> bool {
        self.sources.contains_key(source_id)
    }

    /// Stop capturing audio from the specified source.
    ///
    /// Signals the capture thread to exit and joins it with a timeout.
    pub fn stop_capture(&mut self, source_id: &str) -> Result<(), String> {
        let handle = self
            .sources
            .remove(source_id)
            .ok_or_else(|| "No active capture for this source".to_string())?;
        handle.stop_signal.store(true, Ordering::Release);
        if let Some(thread) = handle.thread
            && let Some(still_running) =
                join_capture_worker(thread, Instant::now() + self.hooks.join_timeout)
        {
            log::warn!(
                "A capture worker did not exit within the stop bound; retaining it and fencing the next session boundary"
            );
            self.retired_threads.push(still_running);
        }
        log::info!("Stopped one capture source");
        Ok(())
    }

    /// Stop a capture only when the manager still owns the exact worker handle
    /// that reported a fatal exit.
    ///
    /// A source can be restarted before asynchronous fatal-exit reconciliation
    /// acquires the session lifecycle lock. Comparing the `Arc` identity keeps
    /// an old worker's delayed cleanup from stopping the replacement generation
    /// that happens to reuse the same source id.
    pub fn stop_capture_if_matches(
        &mut self,
        source_id: &str,
        expected_finished: &Arc<AtomicBool>,
    ) -> Result<bool, String> {
        let Some(current) = self.sources.get(source_id) else {
            return Ok(false);
        };
        if !expected_finished.load(Ordering::Acquire)
            || !Arc::ptr_eq(&current.finished, expected_finished)
        {
            return Ok(false);
        }
        self.stop_capture(source_id)?;
        Ok(true)
    }

    /// App shutdown: refuse every later start, cancel a pending one, and stop
    /// every owned source. Returns the source IDs that were stopped.
    pub fn stop_all(&mut self) -> Vec<String> {
        self.shutting_down = true;
        if let Some(pending) = self.pending_start.as_mut() {
            pending.cancel_tx = None;
        }
        let ids: Vec<String> = self.sources.keys().cloned().collect();
        let mut stopped = Vec::new();

        for id in &ids {
            match self.stop_capture(id) {
                Ok(()) => stopped.push(id.clone()),
                Err(e) => log::error!("Failed to stop a capture source: {}", e),
            }
        }

        log::info!("Stopped {} capture(s)", stopped.len());
        stopped
    }

    /// The committed aggregate, when at least one committed source is live.
    /// A dead handle awaiting reconciliation does not count.
    pub(crate) fn acknowledged_capture(&self) -> Option<AcknowledgedCapture> {
        let sources: Vec<String> = self
            .sources
            .iter()
            .filter(|(_, handle)| {
                handle.startup_acknowledged.load(Ordering::Acquire)
                    && !handle.finished.load(Ordering::Acquire)
            })
            .map(|(source_id, _)| source_id.clone())
            .collect();
        (!sources.is_empty()).then_some(AcknowledgedCapture {
            generation: self.generation,
            sources,
        })
    }

    /// Remove every acknowledged handle whose worker has already exited. This
    /// lets an explicit Stop or one fatal reconciliation sweep sibling fatal
    /// exits under the same session-lifecycle lock, producing one aggregate
    /// CaptureStopped transition rather than one per queued callback.
    pub fn stop_finished_captures(&mut self) -> Vec<String> {
        let finished_ids = self
            .sources
            .iter()
            .filter(|(_, handle)| handle.finished.load(Ordering::Acquire))
            .map(|(source_id, _)| source_id.clone())
            .collect::<Vec<_>>();
        let mut stopped = Vec::new();
        for source_id in finished_ids {
            if self.stop_capture(&source_id).is_ok() {
                stopped.push(source_id);
            }
        }
        stopped
    }

    /// Number of source handles still owned by the current aggregate,
    /// including fatal workers awaiting reconciliation.
    pub fn owned_capture_count(&self) -> usize {
        self.sources.len()
    }

    /// Live source provenance for ephemeral speaker suggestions; never infer input kind from an ID.
    pub(crate) fn speaker_channel_kinds(&self) -> HashMap<String, AudioChannelProvenanceKind> {
        self.sources
            .iter()
            .filter(|(_, h)| !h.finished.load(Ordering::Acquire))
            .map(|(id, h)| {
                let kind = source_channel_provenance_kind(
                    &h.source_info.source_type,
                    h.source_info.device_kind,
                );
                (id.clone(), kind)
            })
            .collect()
    }
    /// Returns the list of currently active source IDs.
    ///
    /// A handle whose capture thread has already exited (clean stop, fatal
    /// stream error, or a panic — see [`CaptureHandle::finished`]) is **not**
    /// reported: it is dead and waiting for lifecycle reconciliation, so
    /// surfacing it would show the UI a phantom "Running" source (Finding #53b).
    pub fn active_captures(&self) -> Vec<String> {
        self.sources
            .iter()
            .filter(|(_, h)| !h.finished.load(Ordering::Acquire))
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn first_source_reconciliation_pending(&self) -> bool {
        self.active_captures().is_empty() && !self.sources.is_empty()
    }

    fn reap_retired_capture_workers(&mut self) {
        let mut still_running = Vec::new();
        for handle in self.retired_threads.drain(..) {
            if handle.is_finished() {
                if handle.join().is_err() {
                    log::error!("A retired capture worker panicked before it was reaped");
                }
            } else {
                still_running.push(handle);
            }
        }
        self.retired_threads = still_running;
    }

    /// Return true while any stopped capture generation can still emit data.
    /// This is stronger than [`Self::active_captures`], which intentionally
    /// hides stopped/finished handles from user-facing source state.
    pub fn has_unquiesced_workers(&mut self) -> bool {
        self.reap_retired_capture_workers();
        !self.retired_threads.is_empty()
    }

    /// Test-only: insert a synthetic handle (no real rsac capture / thread) so
    /// the lifecycle bookkeeping — finished-flag filtering and dead-handle
    /// reconciliation (Finding #53b) — can be exercised without audio hardware.
    #[cfg(test)]
    pub(crate) fn insert_synthetic_handle(
        &mut self,
        source_id: &str,
        finished: bool,
    ) -> Arc<AtomicBool> {
        let finished_flag = Arc::new(AtomicBool::new(finished));
        self.sources.insert(
            source_id.to_string(),
            CaptureHandle {
                thread: None,
                stop_signal: Arc::new(AtomicBool::new(false)),
                startup_acknowledged: Arc::new(AtomicBool::new(true)),
                finished: Arc::clone(&finished_flag),
                source_info: AudioSourceInfo {
                    id: source_id.to_string(),
                    name: source_id.to_string(),
                    source_type: AudioSourceType::SystemDefault,
                    capture_target: Some(source_id.to_string()),
                    device_kind: None,
                    is_default: Some(source_id == "system" || source_id == "system-default"),
                    supported_formats: Vec::new(),
                    default_format: None,
                    channel_provenance: Some(AudioSourceChannelProvenance::unknown_for_format(
                        None,
                    )),
                    capabilities: None,
                    permission_status: None,
                    permission_recovery: None,
                    is_active: true,
                },
            },
        );
        finished_flag
    }

    /// Test-only: replace the worker body, exit reconciliation and bounds.
    #[cfg(test)]
    pub(crate) fn set_worker_hooks(&mut self, hooks: CaptureWorkerHooks) {
        self.hooks = hooks;
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
    fn capture_thread_fn(context: CaptureThreadContext) -> bool {
        let label = capture_log_label(&context.target);
        log::info!(
            "[capture {label}] Worker started (requested {} Hz, {} ch)",
            context.sample_rate,
            context.channels
        );
        let opened = Self::open_capture_stream(
            &context.target,
            context.sample_rate,
            context.channels,
            label,
        );
        let report = match &opened {
            Ok(stream) => Ok(stream.negotiated.clone()),
            Err(reason) => Err(*reason),
        };
        let committed = startup_handshake(&context, report);
        let Ok(OpenedCaptureStream {
            mut capture,
            rx,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            recovery_rx,
            ..
        }) = opened
        else {
            return false;
        };
        // Two-phase startup: build/start/subscribe is merely Ready. No audio
        // is received or forwarded unless the command recorded the
        // CaptureStarted movement boundary and committed this worker.
        if !committed {
            log::warn!("[capture {label}] Not committed; stopping the prepared stream");
            let _ = capture.stop();
            return false;
        }
        let CaptureThreadContext {
            source_id,
            stop_signal,
            pipeline_tx,
            app_handle,
            stream_loss,
            ..
        } = context;

        let start_time = Instant::now();
        log::info!("[capture {label}] Receiving audio buffers");

        // Build the chunk's source id once: every chunk from this thread carries
        // the same id, so we share one `Arc<str>` and refcount-bump it per chunk
        // instead of heap-allocating a fresh `String` each iteration (FA-4b).
        let source_id_arc: Arc<str> = Arc::from(source_id.as_str());

        // Edge-triggered delivery-loss tracking. We poll rsac's windowed
        // `backpressure_report()` (v0.4.0): it carries the legacy
        // consecutive-drop `is_under_backpressure` bool AND a `drop_rate` over a
        // ~10 s sliding window. We trip on EITHER the bool OR sustained partial
        // loss (`drop_rate >= DROP_RATE_TRIP`) — the latter catches steady
        // 1-in-N dropping that the all-or-nothing bool (which resets on any
        // successful push) misses entirely. rsac v0.4.1 also exposes the
        // separate bounded subscriber-channel drop counter; any newly observed
        // subscriber drop trips the same public signal and is logged with the
        // precise layer. Emitting only on transitions keeps the frontend's
        // enter/leave signal clean (no event storm).
        const DROP_RATE_TRIP: f64 = 0.05;
        let mut last_backpressured = false;
        let mut last_subscriber_dropped = 0;
        let mut last_ring_dropped = 0;
        let mut pending_drops = CaptureBufferDrops::default();
        let mut subscriber_drops_since_report = 0u64;
        // Throttle the public health report separately from per-chunk loss.
        let mut poll_counter: u32 = 0;

        // Stream-generation tracking for `hotplug-recovery` (Windows/Linux
        // only): rsac stamps each delivered buffer with the generation of the
        // backend stream that produced it. A supervised SystemDefault session
        // reopens on device changes, so the generation can bump mid-session;
        // an unsupervised capture never leaves generation 0.
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let mut generation_tracker = GenerationTracker::default();

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
        let mut unexpected_exit = false;
        while !stop_signal.load(Ordering::Relaxed) {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(buffer)) => {
                    // `hotplug-recovery` (Windows/Linux only): surface a replacement
                    // stream exactly once per reopen (the generation stamped on this
                    // buffer moved), then drain queued supervisor events. The session
                    // keeps running — this is the recovery working, not a fault.
                    #[cfg(any(target_os = "windows", target_os = "linux"))]
                    {
                        if let Some((Some(previous), current)) =
                            generation_tracker.observe(buffer.stream_generation())
                        {
                            log::warn!(
                                "[capture {label}] capture stream generation changed \
                         {previous} -> {current}: a reopened default-device stream \
                         (hotplug recovery) is delivering audio",
                            );
                            crate::analytics::capture_diagnostic(crate::analytics::DiagEvent {
                                name: "audio.capture.generation_change",
                                category: crate::analytics::Category::Audio,
                                level: sentry::Level::Warning,
                                provider: None,
                                kind: Some("capture_generation_change"),
                                http_status: None,
                                recoverable: Some(true),
                            });
                        }
                        drain_recovery_events(recovery_rx.as_ref(), label);
                    }
                    if stop_signal.load(Ordering::Relaxed) {
                        break;
                    }
                    if buffer.is_empty() {
                        continue;
                    }
                    let subscriber_drops = advance_drop_counter(
                        &mut last_subscriber_dropped,
                        capture.subscriber_dropped_count(),
                    );
                    subscriber_drops_since_report =
                        subscriber_drops_since_report.saturating_add(subscriber_drops);
                    pending_drops.subscriber_buffers = pending_drops
                        .subscriber_buffers
                        .saturating_add(subscriber_drops);
                    pending_drops.ring_buffers =
                        pending_drops
                            .ring_buffers
                            .saturating_add(advance_drop_counter(
                                &mut last_ring_dropped,
                                capture.overrun_count(),
                            ));
                    let chunk = AudioChunk::from_rsac_buffer(
                        Arc::clone(&source_id_arc),
                        buffer,
                        start_time.elapsed(),
                        pending_drops,
                    );
                    let input_frames = chunk.num_frames as u64;
                    match pipeline_tx.send_timeout(AudioPipelineInput::Chunk(chunk), SEND_TIMEOUT) {
                        Ok(()) => pending_drops = CaptureBufferDrops::default(),
                        Err(SendTimeoutError::Timeout(_)) => {
                            // Carry all unreported losses to the next successful send.
                            pending_drops.pipeline_buffers =
                                pending_drops.pipeline_buffers.saturating_add(1);
                            pending_drops.pipeline_input_frames = pending_drops
                                .pipeline_input_frames
                                .saturating_add(input_frames);
                            log::warn!(
                                "[capture {label}] Pipeline send timed out — \
                                 consumer stalled, dropping chunk"
                            );
                        }
                        Err(SendTimeoutError::Disconnected(_)) => {
                            log::warn!("[capture {label}] Pipeline channel closed, exiting");
                            stream_loss.report();
                            let _ = capture.stop();
                            emit_or_log(
                                &app_handle,
                                CAPTURE_ERROR,
                                CaptureErrorPayload {
                                    source_id: source_id.clone(),
                                    error: "audio pipeline stopped accepting capture data"
                                        .to_string(),
                                    recoverable: false,
                                },
                            );
                            return true;
                        }
                    }

                    poll_counter = poll_counter.wrapping_add(1);
                    if poll_counter.is_multiple_of(10) {
                        let report = capture.backpressure_report();
                        let subscriber_drop_delta =
                            std::mem::take(&mut subscriber_drops_since_report);
                        if subscriber_drop_delta > 0 {
                            log::warn!(
                                "[capture {label}] rsac subscriber channel dropped {} buffer(s) \
                                 since the last poll ({} total)",
                                subscriber_drop_delta,
                                last_subscriber_dropped
                            );
                        }
                        let now_backpressured = report.is_under_backpressure
                            || report.drop_rate >= DROP_RATE_TRIP
                            || subscriber_drop_delta > 0;
                        if now_backpressured != last_backpressured {
                            if now_backpressured {
                                log::warn!(
                                    "[capture {label}] Capture delivery loss detected — \
                                     ring or subscriber buffering is dropping chunks"
                                );
                            } else {
                                log::info!("[capture {label}] Backpressure cleared");
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
                    // The error-carrying channel surfaced an AudioError. Only
                    // its closed reason leaves this branch.
                    let reason = capture_failure_reason(&e);
                    if e.is_fatal() {
                        // Terminal device-death (StreamEnded on unplug/format
                        // change, etc.): mark the source dead and request
                        // reconciliation before the rsac teardown below can
                        // wedge, then emit CAPTURE_ERROR so the frontend can
                        // distinguish this from a clean stop (Finding #52).
                        // This is the final item the channel delivers.
                        log::error!("[capture {label}] Fatal stream error, exiting: {reason}");
                        stream_loss.report();
                        emit_or_log(
                            &app_handle,
                            CAPTURE_ERROR,
                            CaptureErrorPayload {
                                source_id: source_id.clone(),
                                error: reason.event_text().to_string(),
                                // `e.is_fatal()` already proved the stream is
                                // dead: a fatal error is never recoverable (#63).
                                recoverable: false,
                            },
                        );
                        unexpected_exit = true;
                        break;
                    } else {
                        // Recoverable hiccup (transient read error / over- or
                        // under-run): log and keep going — the subscription is
                        // still live and the next buffer will follow.
                        log::warn!(
                            "[capture {label}] Recoverable stream error (continuing): {reason}"
                        );
                        #[cfg(any(target_os = "windows", target_os = "linux"))]
                        drain_recovery_events(recovery_rx.as_ref(), label);
                        continue;
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // No data yet — loop back and check stop_signal.
                    // Recovery reopens can outlast several 100 ms poll ticks
                    // (backoff / device wait): drain here too so transitions
                    // are logged even while no audio flows.
                    #[cfg(any(target_os = "windows", target_os = "linux"))]
                    drain_recovery_events(recovery_rx.as_ref(), label);
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    // With subscribe_with_errors() a fatal terminal error is
                    // delivered as an Ok(Err(..)) item *before* this disconnect,
                    // so reaching here means a clean stop (stop_capture dropped
                    // the rsac reader) — no CAPTURE_ERROR needed (Finding #52).
                    log::info!("[capture {label}] Audio stream ended (disconnected)");
                    if !stop_signal.load(Ordering::Relaxed) {
                        stream_loss.report();
                        emit_or_log(
                            &app_handle,
                            CAPTURE_ERROR,
                            CaptureErrorPayload {
                                source_id: source_id.clone(),
                                error: "audio stream ended unexpectedly".to_string(),
                                recoverable: false,
                            },
                        );
                        unexpected_exit = true;
                    }
                    break;
                }
            }
        }

        // 5. Tear down.
        log::info!("[capture {label}] Stopping capture");
        let _ = capture.stop();
        log::info!("[capture {label}] Worker exiting");
        unexpected_exit
    }

    /// Negotiate a supported format, then rsac build -> start -> subscribe.
    /// Failures are reported by closed reason only.
    fn open_capture_stream(
        target: &CaptureTarget,
        sample_rate: u32,
        channels: u16,
        label: &str,
    ) -> Result<OpenedCaptureStream, CaptureFailureReason> {
        // 1. Negotiate a format the device actually supports. rsac does an
        //    exact-match on the requested format and hard-errors otherwise, so
        //    we query the device's supported_formats() up front and pick the
        //    closest supported one. The pipeline resamples to 16 kHz mono
        //    downstream, so the captured format only needs to be *capturable*.
        let negotiated = negotiate_capture_format(target, sample_rate, channels);
        let (cap_sr, cap_ch, cap_fmt) = match &negotiated {
            Some(f) => {
                if f.sample_rate != sample_rate || f.channels != channels {
                    log::info!(
                        "[capture {label}] Requested {} Hz / {} ch not supported by device; \
                         negotiated {} Hz / {} ch / {:?}",
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
                    "[capture {label}] Could not introspect device formats; using requested \
                     {} Hz / {} ch",
                    sample_rate,
                    channels
                );
                (sample_rate, channels, SampleFormat::F32)
            }
        };

        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let supervised = system_default_auto_recovery_applies(target);
        let builder = AudioCaptureBuilder::new()
            .with_target(target.clone())
            .sample_rate(cap_sr)
            .channels(cap_ch)
            .sample_format(cap_fmt);
        // rsac 0.5.0 `hotplug-recovery`: supervise SystemDefault capture so a
        // default-device change or stream invalidation reopens the stream
        // instead of ending the session. Windows/Linux only — rsac withholds
        // the feature on macOS (ADR-0021), where `with_auto_recovery` would
        // fail `preflight()` with `PlatformNotSupported`; this re-bind exists
        // only where Cargo.toml links the feature, so a macOS build never
        // calls it.
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let builder = if supervised {
            builder.with_auto_recovery(AutoRecoveryConfig::default())
        } else {
            builder
        };
        let mut capture = builder.build().map_err(|e| {
            let reason = capture_failure_reason(&e);
            log::error!("[capture {label}] Failed to build AudioCapture: {reason}");
            reason
        })?;

        // 2. Start capture.
        if let Err(e) = capture.start() {
            let reason = capture_failure_reason(&e);
            log::error!("[capture {label}] Failed to start capture: {reason}");
            // Anonymous, structured diagnostic (no-op unless analytics is
            // enabled). Only the category + recoverable flag ride along — never
            // the error string or source_id.
            crate::analytics::capture_diagnostic(crate::analytics::DiagEvent {
                name: "audio.capture.start_failed",
                category: crate::analytics::Category::Audio,
                level: sentry::Level::Error,
                provider: None,
                kind: Some("capture_start_failed"),
                http_status: None,
                recoverable: Some(!e.is_fatal()),
            });
            return Err(reason);
        }

        // 2b. `hotplug-recovery` (Windows/Linux only): subscribe to the
        //     supervisor's event feed. Exactly one subscriber per rsac
        //     handle; the read loop drains it (try_recv only — never a
        //     callback into `capture`, per the rsac CHANGELOG lock rule).
        #[cfg(any(target_os = "windows", target_os = "linux"))]
        let recovery_rx = if supervised {
            match capture.subscribe_recovery_events() {
                Ok(rx) => Some(rx),
                // Never fatal: the capture is supervised regardless, and the
                // generation tracking in the read loop still surfaces any
                // reopen that delivers audio.
                Err(e) => {
                    log::warn!(
                        "[capture {label}] Failed to subscribe to recovery events: {}",
                        capture_failure_reason(&e)
                    );
                    None
                }
            }
        } else {
            None
        };

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
            Ok(rx) => rx,
            Err(e) => {
                let reason = capture_failure_reason(&e);
                log::error!("[capture {label}] Failed to subscribe: {reason}");
                let _ = capture.stop();
                return Err(reason);
            }
        };
        Ok(OpenedCaptureStream {
            capture,
            rx,
            #[cfg(any(target_os = "windows", target_os = "linux"))]
            recovery_rx,
            negotiated: negotiated.map(|format| audio_format_info_from_rsac(&format)),
        })
    }
}

impl Default for AudioCaptureManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Test support: a scripted worker body that follows the production startup
/// handshake without opening rsac, so command-layer tests drive the real
/// manager, rollback, ledger and gate code.
#[cfg(test)]
pub(crate) mod scripted {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// How one scripted worker behaves, in spawn order.
    pub(crate) enum ScriptedStart {
        /// Report Ready, wait for commit, then run until stopped or lost.
        Ready,
        /// Fail build/start/subscribe with this rsac error.
        Fail(AudioError),
        /// Block as in a stuck OS call, ignoring Stop, until released; then
        /// try to report Ready.
        Hang,
        /// Report Ready, then die on `lose_stream()` without taking the
        /// commit: the worker gone between Ready and commit.
        ReadyThenExit,
    }

    /// Controls for one spawned scripted worker.
    pub(crate) struct ScriptedWorker {
        release: crossbeam_channel::Sender<()>,
        lose: crossbeam_channel::Sender<()>,
    }

    impl ScriptedWorker {
        /// Let a `Hang` worker return from its stuck call.
        pub(crate) fn release(&self) {
            let _ = self.release.try_send(());
        }

        /// The device disappears: a committed worker's stream ends.
        pub(crate) fn lose_stream(&self) {
            let _ = self.lose.try_send(());
        }
    }

    pub(crate) struct ScriptedControls {
        /// One entry per spawned worker, in spawn order.
        pub(crate) workers: crossbeam_channel::Receiver<ScriptedWorker>,
        /// Reconciliation requests: (source id, the dead handle's identity).
        pub(crate) exit_requests: crossbeam_channel::Receiver<(String, Arc<AtomicBool>)>,
    }

    pub(crate) fn scripted_hooks(
        scripts: Vec<ScriptedStart>,
        startup_timeout: Duration,
        join_timeout: Duration,
    ) -> (CaptureWorkerHooks, ScriptedControls) {
        let scripts = Arc::new(Mutex::new(VecDeque::from(scripts)));
        let (workers_tx, workers) = crossbeam_channel::unbounded();
        let (exits_tx, exit_requests) = crossbeam_channel::unbounded();
        let body = move |context: CaptureThreadContext| -> bool {
            let script = scripts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .pop_front()
                .expect("a capture worker was spawned beyond its script");
            let (release, release_rx) = crossbeam_channel::bounded(1);
            let (lose, lose_rx) = crossbeam_channel::bounded(1);
            let _ = workers_tx.send(ScriptedWorker { release, lose });
            match script {
                ScriptedStart::Fail(error) => {
                    startup_handshake(&context, Err(capture_failure_reason(&error)));
                    false
                }
                ScriptedStart::Hang => {
                    let _ = release_rx.recv();
                    startup_handshake(&context, Ok(None));
                    false
                }
                ScriptedStart::ReadyThenExit => {
                    if context.startup_tx.send(Ok(None)).is_ok() {
                        let _ = lose_rx.recv();
                    }
                    // Returning drops the commit gate's receiver, so the
                    // command's commit send fails.
                    false
                }
                ScriptedStart::Ready => {
                    if !startup_handshake(&context, Ok(None)) {
                        return false;
                    }
                    while !context.stop_signal.load(Ordering::Acquire) {
                        match lose_rx.recv_timeout(Duration::from_millis(5)) {
                            Ok(()) => {
                                context.stream_loss.report();
                                return true;
                            }
                            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                                std::thread::sleep(Duration::from_millis(5));
                            }
                        }
                    }
                    false
                }
            }
        };
        let hooks = CaptureWorkerHooks {
            body: Arc::new(body),
            reconcile_exit: Arc::new(move |_app, source_id, finished| {
                let _ = exits_tx.send((source_id, finished));
            }),
            startup_timeout,
            join_timeout,
        };
        (
            hooks,
            ScriptedControls {
                workers,
                exit_requests,
            },
        )
    }

    impl AudioCaptureManager {
        /// Block until every retired worker has exited (after the test
        /// released it), so the next boundary check is deterministic.
        pub(crate) fn join_retired_workers(&mut self) {
            for handle in self.retired_threads.drain(..) {
                let _ = handle.join();
            }
        }

        pub(crate) fn retired_worker_count(&self) -> usize {
            self.retired_threads.len()
        }
    }
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
    fn format_infos_preserve_rate_channels_and_sample_format() {
        let infos = audio_format_infos_from_rsac(&[
            fmt(44100, 1, SampleFormat::I16),
            fmt(48000, 2, SampleFormat::F32),
        ]);

        assert_eq!(
            infos,
            vec![
                AudioFormatInfo {
                    sample_rate: 44100,
                    channels: 1,
                    sample_format: AudioSampleFormat::I16,
                },
                AudioFormatInfo {
                    sample_rate: 48000,
                    channels: 2,
                    sample_format: AudioSampleFormat::F32,
                },
            ]
        );
        assert_eq!(
            default_format_info(&infos),
            Some(AudioFormatInfo {
                sample_rate: 44100,
                channels: 1,
                sample_format: AudioSampleFormat::I16,
            })
        );
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
    fn delayed_fatal_reconciliation_cannot_stop_restarted_source_generation() {
        let mut manager = AudioCaptureManager::new();
        let old_finished = manager.insert_synthetic_handle("src", true);

        // Models restart winning the session lifecycle lock before the old
        // worker's queued reconciliation task. The replacement reuses the same
        // public source id but owns a distinct handle identity.
        let replacement_finished = manager.insert_synthetic_handle("src", false);
        assert!(
            !manager
                .stop_capture_if_matches("src", &old_finished)
                .expect("stale reconciliation is a no-op"),
            "an old generation must not stop the replacement capture"
        );
        assert_eq!(manager.active_captures(), vec!["src".to_string()]);

        replacement_finished.store(true, Ordering::Release);
        assert!(
            manager
                .stop_capture_if_matches("src", &replacement_finished)
                .expect("matching finished generation is reaped")
        );
        assert!(manager.active_captures().is_empty());
    }

    #[test]
    fn dead_last_source_blocks_a_different_first_source_generation() {
        let mut manager = AudioCaptureManager::new();
        manager.insert_synthetic_handle("dead-source-a", true);

        assert!(manager.active_captures().is_empty());
        assert!(
            manager.first_source_reconciliation_pending(),
            "a different source must not begin a second aggregate lifecycle yet"
        );
    }

    #[test]
    fn one_cleanup_sweeps_sibling_finished_sources() {
        let mut manager = AudioCaptureManager::new();
        manager.insert_synthetic_handle("dead-a", true);
        manager.insert_synthetic_handle("dead-b", true);

        let stopped = manager.stop_finished_captures();
        assert_eq!(stopped.len(), 2);
        assert_eq!(manager.owned_capture_count(), 0);
    }

    #[test]
    fn retired_capture_worker_fences_boundaries_until_it_finishes() {
        let mut manager = AudioCaptureManager::new();
        let release = Arc::new(AtomicBool::new(false));
        let release_worker = Arc::clone(&release);
        manager.retired_threads.push(std::thread::spawn(move || {
            while !release_worker.load(Ordering::Acquire) {
                std::thread::yield_now();
            }
        }));

        assert!(manager.has_unquiesced_workers());
        release.store(true, Ordering::Release);

        let deadline = Instant::now() + Duration::from_secs(1);
        while manager.has_unquiesced_workers() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            !manager.has_unquiesced_workers(),
            "the boundary fence clears only after the retired worker exits"
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

#[cfg(test)]
mod source_descriptor_tests {
    use super::*;

    fn caps_with_support(
        supports_application_capture: bool,
        supports_process_tree_capture: bool,
        supports_device_selection: bool,
    ) -> rsac::PlatformCapabilities {
        let mut caps = rsac::PlatformCapabilities::query();
        caps.backend_name = "TestBackend";
        caps.supports_system_capture = true;
        caps.supports_application_capture = supports_application_capture;
        caps.supports_process_tree_capture = supports_process_tree_capture;
        caps.supports_device_selection = supports_device_selection;
        caps.supports_device_change_notifications = true;
        caps
    }

    #[test]
    fn source_capabilities_gate_process_tree_support() {
        let caps = caps_with_support(true, false, true);
        let source_type = AudioSourceType::ProcessTree { pid: 42 };
        let source_caps = source_capabilities(&source_type, &caps);

        assert!(!source_caps.capture_supported);
        assert_eq!(source_caps.backend_name, "TestBackend");
        assert!(
            source_caps
                .unsupported_reason
                .as_deref()
                .unwrap_or_default()
                .contains("Process-tree capture")
        );
    }

    #[test]
    fn source_capabilities_gate_device_selection_support() {
        let caps = caps_with_support(true, true, false);
        let source_type = AudioSourceType::Device {
            device_id: "dev-1".to_string(),
        };
        let source_caps = source_capabilities(&source_type, &caps);

        assert!(!source_caps.capture_supported);
        assert_eq!(
            source_caps.unsupported_reason.as_deref(),
            Some("Device selection is not supported by the TestBackend backend")
        );
    }

    #[test]
    fn source_info_preserves_process_tree_capture_mode() {
        let source = source_info_for_capture_target(
            "tree:42",
            &CaptureTarget::ProcessTree(rsac::ProcessId(42)),
            true,
        );

        assert!(matches!(
            source.source_type,
            AudioSourceType::ProcessTree { pid: 42 }
        ));
        assert_eq!(source.capture_target.as_deref(), Some("tree:42"));
        assert_eq!(source.is_default, Some(false));
        assert!(source.capabilities.is_some());
        assert!(source.permission_status.is_some());
    }

    #[test]
    fn source_info_preserves_application_name_capture_mode() {
        let source = source_info_for_capture_target(
            "name:Spotify",
            &CaptureTarget::ApplicationByName("Spotify".to_string()),
            true,
        );

        assert!(matches!(
            source.source_type,
            AudioSourceType::ApplicationName { ref app_name } if app_name == "Spotify"
        ));
        assert_eq!(source.capture_target.as_deref(), Some("name:Spotify"));
    }

    #[test]
    fn permission_recovery_is_only_emitted_for_blocked_process_capture_sources() {
        let app_source = AudioSourceType::Application {
            pid: 42,
            app_name: "Design Tool".to_string(),
            bundle_id: None,
        };
        let device_source = AudioSourceType::Device {
            device_id: "mic-1".to_string(),
        };

        let hint = source_permission_recovery(&app_source, AudioPermissionStatus::Denied)
            .expect("denied app capture should have recovery metadata");

        assert!(!hint.summary.is_empty());
        assert!(!hint.body.is_empty());
        assert_eq!(
            hint.permission_kind,
            permission_kind_for_platform(hint.platform)
        );
        assert!(hint.actions.iter().any(
            |action| action.kind == AudioPermissionRecoveryActionKind::GrantPermissionManually
        ));
        assert_eq!(
            source_permission_recovery(&app_source, AudioPermissionStatus::Granted),
            None
        );
        assert_eq!(
            source_permission_recovery(&device_source, AudioPermissionStatus::Denied),
            None
        );
    }

    #[test]
    fn rsac_application_source_mapping_preserves_bundle_id() {
        let (source_type, device_kind, is_default, supported_formats, capture_target) =
            source_descriptor_parts_from_rsac_kind(
                &rsac::AudioSourceKind::Application {
                    pid: 2024,
                    app_name: "Safari".to_string(),
                    bundle_id: Some("com.apple.Safari".to_string()),
                },
                &SourceFormatSnapshot::default(),
            );

        assert!(matches!(
            source_type,
            AudioSourceType::Application {
                pid: 2024,
                ref app_name,
                ref bundle_id,
            } if app_name == "Safari" && bundle_id.as_deref() == Some("com.apple.Safari")
        ));
        assert_eq!(device_kind, None);
        assert_eq!(is_default, Some(false));
        assert!(supported_formats.is_empty());
        assert_eq!(capture_target, "app:2024");
    }

    #[test]
    fn source_info_for_capture_start_preserves_descriptor_metadata() {
        let descriptor = AudioSourceInfo {
            id: "opaque-rsac-row".to_string(),
            name: "Safari".to_string(),
            source_type: AudioSourceType::Application {
                pid: 2024,
                app_name: "Safari".to_string(),
                bundle_id: Some("com.apple.Safari".to_string()),
            },
            capture_target: Some("app:2024".to_string()),
            device_kind: None,
            is_default: Some(false),
            supported_formats: Vec::new(),
            default_format: None,
            channel_provenance: None,
            capabilities: None,
            permission_status: Some(AudioPermissionStatus::Denied),
            permission_recovery: Some(AudioPermissionRecoveryHint {
                platform: AudioPermissionRecoveryPlatform::Macos,
                permission_kind: AudioPermissionKind::AudioCapture,
                summary: "macOS Audio Capture permission is denied.".to_string(),
                body: "Grant permission, relaunch, and refresh sources.".to_string(),
                actions: Vec::new(),
            }),
            is_active: false,
        };

        let source_info = source_info_for_capture_start(
            "app:2024",
            &CaptureTarget::Application(rsac::ApplicationId("2024".to_string())),
            Some(descriptor),
            true,
        );

        assert_eq!(source_info.id, "app:2024");
        assert_eq!(source_info.name, "Safari");
        assert_eq!(source_info.capture_target.as_deref(), Some("app:2024"));
        assert!(source_info.is_active);
        assert!(source_info.permission_recovery.is_some());
        assert!(source_info.channel_provenance.is_some());
        assert!(matches!(
            source_info.source_type,
            AudioSourceType::Application {
                pid: 2024,
                ref app_name,
                ref bundle_id,
            } if app_name == "Safari" && bundle_id.as_deref() == Some("com.apple.Safari")
        ));
    }

    #[test]
    fn misleading_stereo_source_requires_mono_fallback() {
        let source_type = AudioSourceType::Device {
            device_id: "opaque-device-id".to_string(),
        };
        let provenance = source_channel_provenance(
            &source_type,
            Some(AudioDeviceKind::Output),
            Some(AudioFormatInfo {
                sample_rate: 96_000,
                channels: 8,
                sample_format: AudioSampleFormat::F32,
            }),
        );

        assert_eq!(provenance.channel_count, 8);
        assert!(!provenance.source_native);
        assert!(provenance.requires_mono_fallback());
    }

    #[test]
    fn source_native_descriptor_survives_negotiated_format_update() {
        let mut source_info = AudioSourceInfo {
            id: "meeting-lanes".to_string(),
            name: "Meeting lanes".to_string(),
            source_type: AudioSourceType::ApplicationName {
                app_name: "Meeting".to_string(),
            },
            capture_target: Some("name:Meeting".to_string()),
            device_kind: None,
            is_default: Some(false),
            supported_formats: Vec::new(),
            default_format: None,
            channel_provenance: Some(AudioSourceChannelProvenance::source_native(
                AudioChannelProvenanceKind::VirtualMeetingLane,
                vec![
                    audio_graph_ipc_contract::AudioSourceChannelInfo {
                        index: 0,
                        id: "host".to_string(),
                        label: Some("Host".to_string()),
                        provenance: AudioChannelProvenanceKind::VirtualMeetingLane,
                    },
                    audio_graph_ipc_contract::AudioSourceChannelInfo {
                        index: 1,
                        id: "guest".to_string(),
                        label: Some("Guest".to_string()),
                        provenance: AudioChannelProvenanceKind::VirtualMeetingLane,
                    },
                ],
                None,
            )),
            capabilities: None,
            permission_status: None,
            permission_recovery: None,
            is_active: false,
        };

        apply_negotiated_channel_format(
            &mut source_info,
            Some(AudioFormatInfo {
                sample_rate: 48_000,
                channels: 2,
                sample_format: AudioSampleFormat::F32,
            }),
        );

        let provenance = source_info
            .channel_provenance
            .as_ref()
            .expect("channel provenance should remain present");
        assert!(provenance.is_source_native_admissible());
        assert_eq!(provenance.channels[0].id, "host");
        assert_eq!(provenance.channels[1].id, "guest");
        assert_eq!(
            provenance
                .negotiated_format
                .as_ref()
                .map(|format| format.channels),
            Some(2)
        );
    }

    #[test]
    fn source_is_active_checks_canonical_capture_target_before_rsac_row_id() {
        let mut manager = AudioCaptureManager::new();
        manager.insert_synthetic_handle("device:{0.0.1.00000000}.{mic-guid}", false);

        assert!(source_is_active(
            &manager.sources,
            "{0.0.1.00000000}.{mic-guid}",
            "device:{0.0.1.00000000}.{mic-guid}",
        ));
        assert!(!source_is_active(
            &manager.sources,
            "{0.0.1.00000000}.{other-guid}",
            "device:{0.0.1.00000000}.{other-guid}",
        ));

        manager.insert_synthetic_handle("{0.0.1.00000000}.{legacy-guid}", false);
        assert!(source_is_active(
            &manager.sources,
            "{0.0.1.00000000}.{legacy-guid}",
            "device:{0.0.1.00000000}.{legacy-guid}",
        ));
    }
}

#[cfg(test)]
#[cfg(any(target_os = "windows", target_os = "linux"))]
mod hotplug_recovery_tests {
    //! Deterministic, pure-std coverage for the rsac 0.5.0 `hotplug-recovery`
    //! wiring: the SystemDefault-only opt-in, the read-loop generation
    //! tracking, and the event→(level, telemetry kind) mapping. No hardware
    //! and no backend — rsac's `RecoveryEvent`/`RecoveryState` are
    //! `#[non_exhaustive]`, so the mapping is exercised through the local
    //! `RecoverySignal` copy rather than constructed rsac values.
    use super::*;
    use rsac::{ApplicationId, DeviceId};

    #[test]
    fn only_system_default_opts_into_auto_recovery() {
        assert!(system_default_auto_recovery_applies(
            &CaptureTarget::SystemDefault
        ));
        assert!(!system_default_auto_recovery_applies(
            &CaptureTarget::Device(DeviceId("hw:0,0".into()))
        ));
        assert!(!system_default_auto_recovery_applies(
            &CaptureTarget::Application(ApplicationId("4242".into()))
        ));
    }

    #[test]
    fn generation_tracker_reports_onset_then_only_changes() {
        let mut tracker = GenerationTracker::default();

        // The first observation establishes the baseline, not a change.
        assert_eq!(tracker.observe(0), Some((None, 0)));
        assert_eq!(tracker.observe(0), None);

        // A reopen bumps the generation: reported once, with the previous.
        assert_eq!(tracker.observe(1), Some((Some(0), 1)));
        assert_eq!(tracker.observe(1), None);

        // A further reopen, and a session restart that renumbers from 0.
        assert_eq!(tracker.observe(2), Some((Some(1), 2)));
        assert_eq!(tracker.observe(0), Some((Some(2), 0)));
    }

    #[test]
    fn recovery_signals_map_to_levels_and_telemetry_kinds() {
        let cases = [
            (
                RecoverySignal::DefaultChanged,
                sentry::Level::Info,
                "recovery_default_changed",
            ),
            (
                RecoverySignal::DeviceInvalidated {
                    reason: "device unplugged".into(),
                },
                sentry::Level::Warning,
                "recovery_device_invalidated",
            ),
            (
                RecoverySignal::RetryScheduled { attempt: 3 },
                sentry::Level::Warning,
                "recovery_retry_scheduled",
            ),
            (
                RecoverySignal::Recovered,
                sentry::Level::Info,
                "recovery_recovered",
            ),
            (
                RecoverySignal::Exhausted {
                    attempts: 5,
                    reason: "no device reappeared".into(),
                },
                sentry::Level::Error,
                "recovery_exhausted",
            ),
        ];
        for (signal, level, kind) in cases {
            let report = recovery_signal_report(&signal);
            assert_eq!(report.level, level, "log level for {signal:?}");
            assert_eq!(report.telemetry_kind, kind, "telemetry kind for {signal:?}");
            assert_eq!(
                recovery_signal_recoverable(&signal),
                !matches!(signal, RecoverySignal::Exhausted { .. }),
                "recoverability for {signal:?}"
            );
            assert!(!report.message.is_empty(), "log line for {signal:?}");
        }
    }

    #[test]
    fn recovery_log_lines_carry_attempt_and_reason_details() {
        let retry = recovery_signal_report(&RecoverySignal::RetryScheduled { attempt: 4 });
        assert!(retry.message.contains('4'), "attempt in {retry:?}");

        let exhausted = recovery_signal_report(&RecoverySignal::Exhausted {
            attempts: 5,
            reason: "device never reappeared".into(),
        });
        assert!(exhausted.message.contains('5'), "attempts in {exhausted:?}");
        assert!(
            exhausted.message.contains("device never reappeared"),
            "reason in {exhausted:?}"
        );
    }

    #[test]
    fn draining_without_a_subscription_is_a_no_op() {
        // A non-supervised target (or a failed subscribe) drains nothing.
        drain_recovery_events(None, "system-default");
    }
}
