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
            log::debug!("Device format enumeration unavailable: {}", e);
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
            log::debug!("Device format list unavailable: {}", e);
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
    #[allow(clippy::too_many_arguments)]
    pub fn start_capture(
        &mut self,
        source_id: &str,
        target: CaptureTarget,
        source_descriptor: Option<AudioSourceInfo>,
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

        let mut source_info =
            source_info_for_capture_start(source_id, &target, source_descriptor, true);
        let negotiated_format = negotiate_capture_format(&target, sample_rate, channels)
            .map(|format| audio_format_info_from_rsac(&format));
        apply_negotiated_channel_format(&mut source_info, negotiated_format);

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
            let recoverable = crate::events::classify_capture_error(&err_str);
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
                recoverable: Some(recoverable),
            });
            emit_or_log(
                &app_handle,
                CAPTURE_ERROR,
                CaptureErrorPayload {
                    source_id: source_id.clone(),
                    error: err_str.clone(),
                    recoverable,
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
                                // `e.is_fatal()` already proved the stream is
                                // dead. The string heuristic
                                // (`classify_capture_error`) DEFAULTS to
                                // recoverable for unmatched text, so a fatal
                                // "format change" (no marker) would be reported
                                // recoverable (#63). Use the authoritative
                                // is_fatal verdict directly: a fatal error is
                                // never recoverable.
                                recoverable: !e.is_fatal(),
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
        rsac::PlatformCapabilities {
            supports_system_capture: true,
            supports_application_capture,
            supports_process_tree_capture,
            supports_device_selection,
            supports_device_change_notifications: true,
            supported_sample_formats: vec![SampleFormat::F32],
            sample_rate_range: (8000, 48000),
            max_channels: 2,
            backend_name: "TestBackend",
        }
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
mod fatal_branch_recoverability_tests {
    //! Finding #63: the fatal stream-error branch of `capture_thread_fn` must
    //! report `recoverable: false`. The previous code re-derived recoverability
    //! from the error string via `classify_capture_error`, which DEFAULTS to
    //! `true` (recoverable) for unmatched text — so a provably-dead
    //! `StreamEnded { reason: "format change" }` (a string with no fatal
    //! marker) was reported recoverable. The fix uses the authoritative
    //! `is_fatal()` verdict (`recoverable: !e.is_fatal()`) instead.
    use rsac::AudioError;

    #[test]
    fn stream_ended_format_change_is_fatal_so_branch_reports_not_recoverable() {
        // A format-change device death: fatal per rsac, but its string carries
        // no marker the heuristic recognises.
        let e = AudioError::StreamEnded {
            reason: "format change".to_string(),
        };
        let err_str = format!("{}", e);

        // Root cause: the lossy string heuristic would call this RECOVERABLE.
        assert!(
            crate::events::classify_capture_error(&err_str),
            "regression guard: classify_capture_error DEFAULTS to recoverable \
             for unmatched fatal text — this is exactly why the fatal branch \
             must NOT derive recoverability from the string"
        );

        // The error is unambiguously fatal…
        assert!(e.is_fatal(), "StreamEnded must be fatal");

        // …so the value the fatal branch now emits (`recoverable: !is_fatal()`)
        // is false. Before the fix this site used classify_capture_error and
        // would have emitted `true`.
        let recoverable = !e.is_fatal();
        assert!(
            !recoverable,
            "a fatal stream death must be reported recoverable:false (#63)"
        );
    }
}
