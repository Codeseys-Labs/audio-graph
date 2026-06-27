//! Audio playback subsystem (Wave B / audio-graph-8d75 / ADR-0004 consumer).
//!
//! Plays PCM from a [`crate::tts::TtsEvent::AudioChunk`] stream out the
//! user's selected output device. Supports cancel (barge-in) within ~50 ms.
//!
//! ## Architecture
//!
//! ```text
//!  TtsProvider events → producer task → ringbuf::Producer → cpal callback → device
//!                                          ▲                  │
//!                                          │                  ▼ AtomicBool cancel
//!                                          └── push samples
//! ```
//!
//! - **`cpal::Stream`** is `!Send` on Windows (WASAPI/COM affinity). It
//!   lives on a **dedicated `std::thread`** — never in `tauri::State`,
//!   never inside a `tokio::spawn` task.
//! - **Communication** with the audio thread is via `crossbeam_channel`
//!   commands (`OpenStream`, `StopStream`, `Shutdown`).
//! - **Sample plumbing** producer→callback uses a per-stream
//!   [`ringbuf::HeapRb<i16>`] (SPSC, lock-free). The producer side lives
//!   on `AudioPlayer` behind an `Arc<Mutex<...>>`; the consumer side moves
//!   into the cpal callback closure on the audio thread.
//! - **Resampling** happens producer-side, before samples enter the ring
//!   buffer. The cpal callback only drains device-rate mono samples, handles
//!   cancel/silence, and converts to the host sample format.
//! - **Cancel/barge-in** is an `Arc<AtomicBool>`. When set, the callback
//!   drains the ring buffer and emits silence. Audible cut-off is bounded
//!   by one callback period (~10–20 ms typical).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use audioadapter_buffers::direct::SequentialSlice;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, StreamConfig, StreamError};
use crossbeam_channel::{Receiver, Sender, unbounded};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};
use rubato::{
    Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

#[cfg(test)]
mod tests;

/// Public shape of an audio output device, exposed to the frontend so users
/// can pick one in settings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OutputDevice {
    /// Stable name returned by cpal. Use as the lookup key when the user
    /// picks a device.
    pub name: String,
    /// `true` if this device is the default for the host. Exactly one
    /// device is marked default so the UI can surface it as such.
    pub is_default: bool,
}

/// PCM stream parameters the playback subsystem expects from a producer.
#[derive(Debug, Clone, Copy)]
pub struct PlaybackConfig {
    /// Source sample rate of the i16 LE samples being pushed (e.g. 24_000
    /// for Deepgram Aura linear16 at default settings).
    pub source_sample_rate: u32,
    /// Channel count of the source. The MVP requires 1 (mono); the device
    /// callback duplicates to stereo on N-channel devices.
    pub source_channels: u16,
}

impl Default for PlaybackConfig {
    fn default() -> Self {
        Self {
            source_sample_rate: 24_000,
            source_channels: 1,
        }
    }
}

/// Errors returned from playback APIs. These bubble through to Tauri command
/// results as strings.
#[derive(Debug, thiserror::Error)]
pub enum PlaybackError {
    #[error("no default output device available on this host")]
    NoDefaultDevice,
    #[error("requested device '{0}' not found")]
    DeviceNotFound(String),
    #[error("device does not support any streamable format")]
    NoSupportedFormat,
    #[error("cpal stream build failed: {0}")]
    BuildStream(String),
    #[error("cpal stream play failed: {0}")]
    PlayStream(String),
    #[error("audio thread is not running")]
    ThreadDead,
}

impl From<PlaybackError> for String {
    fn from(value: PlaybackError) -> Self {
        value.to_string()
    }
}

/// Default ring-buffer capacity in samples (mono i16). 192 000 ≈ 4 s of
/// 48 kHz, enough headroom for a slightly bursty producer without making
/// cancel-latency worse than ~20 ms (drain-on-cancel happens within one
/// callback period).
const DEFAULT_CAPACITY: usize = 192_000;

/// Commands sent from any thread to the dedicated audio thread.
enum AudioCommand {
    /// Open / re-open the output stream against this device.
    /// `device_name = None` means "use the host default".
    OpenStream {
        device_name: Option<String>,
        config: PlaybackConfig,
        /// New consumer for the per-stream ring buffer. The corresponding
        /// producer was sent to `AudioPlayer` over the reply channel
        /// before this command was issued.
        consumer: HeapCons<i16>,
        /// One-shot reply so the caller knows whether the open succeeded and
        /// which device sample rate the stream actually requested.
        reply: crossbeam_channel::Sender<Result<PlaybackOpenInfo, String>>,
    },
    /// Stop the stream and discard the consumer side. Safe to call when no
    /// stream is open (no-op).
    StopStream,
    /// Terminate the audio thread.
    Shutdown,
}

struct PlaybackOpenInfo {
    device_sample_rate: u32,
}

/// Output-device list helper. Read-only; safe to call from any thread.
///
/// First entry (if any) is the host's default device.
//
// `DeviceTrait::name()` is deprecated in cpal 0.17 in favour of
// `description()`/`id()`, but device selection elsewhere (set_output_device)
// matches on this exact name string. Keep `name()` until the whole
// device-identity path migrates to the new `id()` API together.
#[allow(deprecated)]
pub fn list_output_devices() -> Vec<OutputDevice> {
    let host = cpal::default_host();
    let default_name = host
        .default_output_device()
        .and_then(|d| d.name().ok())
        .unwrap_or_default();
    match host.output_devices() {
        Ok(iter) => iter
            .filter_map(|d| d.name().ok())
            .map(|name| {
                let is_default = name == default_name;
                OutputDevice { name, is_default }
            })
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Handle exposed via `tauri::State`. Cheaply cloneable.
#[derive(Clone)]
pub struct AudioPlayer {
    cmd_tx: Sender<AudioCommand>,
    /// Producer side of the active ring buffer. Replaced on each
    /// `open_*` so each stream gets its own SPSC pair (mirrors what the
    /// audio thread is reading from).
    producer: Arc<std::sync::Mutex<Option<PlaybackProducer>>>,
    /// Set to `true` to cancel in-flight audio. Audio thread observes
    /// every callback (~10 ms).
    cancel: Arc<AtomicBool>,
    capacity: usize,
}

impl AudioPlayer {
    /// Spawn the audio thread but do NOT open a stream. Producers can call
    /// `push_samples` but the bytes are dropped (no producer registered)
    /// until `open_default`/`open_named` succeeds.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    fn with_capacity(capacity: usize) -> Self {
        let (cmd_tx, cmd_rx) = unbounded::<AudioCommand>();
        let cancel = Arc::new(AtomicBool::new(false));

        let cancel_for_thread = cancel.clone();
        std::thread::Builder::new()
            .name("audio-player".into())
            .spawn(move || {
                audio_thread_main(cmd_rx, cancel_for_thread);
            })
            .expect("audio thread spawn");

        Self {
            cmd_tx,
            producer: Arc::new(std::sync::Mutex::new(None)),
            cancel,
            capacity,
        }
    }

    /// Open the host's default output device with the given source config.
    pub fn open_default(&self, config: PlaybackConfig) -> Result<(), PlaybackError> {
        self.open_inner(None, config)
    }

    /// Open a specific device by name (as listed by [`list_output_devices`]).
    pub fn open_named(&self, name: String, config: PlaybackConfig) -> Result<(), PlaybackError> {
        self.open_inner(Some(name), config)
    }

    fn open_inner(
        &self,
        device_name: Option<String>,
        config: PlaybackConfig,
    ) -> Result<(), PlaybackError> {
        validate_playback_config(config)?;

        // Build a fresh per-stream ringbuf. Producer side replaces what
        // AudioPlayer has stored; consumer side ships to the audio thread.
        let rb = HeapRb::<i16>::new(self.capacity);
        let (prod, cons) = rb.split();
        // Reset cancel so a previous barge-in doesn't immediately mute the
        // new stream.
        self.cancel.store(false, Ordering::SeqCst);
        // Do not expose the producer until the audio thread has opened the
        // stream and returned the actual device sample rate.
        *self.producer.lock().unwrap_or_else(|p| p.into_inner()) = None;

        // Send the consumer to the audio thread. Reply channel waits for an
        // explicit success/failure so callers know if the device opened.
        let (reply_tx, reply_rx) = crossbeam_channel::bounded(1);
        self.cmd_tx
            .send(AudioCommand::OpenStream {
                device_name,
                config,
                consumer: cons,
                reply: reply_tx,
            })
            .map_err(|_| PlaybackError::ThreadDead)?;
        match reply_rx
            .recv_timeout(std::time::Duration::from_millis(2_000))
            .map_err(|_| PlaybackError::ThreadDead)?
        {
            Ok(info) => {
                let producer =
                    PlaybackProducer::new(prod, config.source_sample_rate, info.device_sample_rate)
                        .map_err(PlaybackError::BuildStream)?;
                *self.producer.lock().unwrap_or_else(|p| p.into_inner()) = Some(producer);
                Ok(())
            }
            Err(message) => Err(PlaybackError::BuildStream(message)),
        }
    }

    /// Stop the active stream. Stored producer is dropped; subsequent
    /// `push_samples` calls return 0 until a stream is reopened.
    pub fn stop(&self) -> Result<(), PlaybackError> {
        {
            let mut slot = self.producer.lock().unwrap_or_else(|p| p.into_inner());
            *slot = None;
        }
        self.cmd_tx
            .send(AudioCommand::StopStream)
            .map_err(|_| PlaybackError::ThreadDead)
    }

    /// Push source-rate mono samples into the active playback stream. Returns
    /// the number of device-rate mono samples written to the ring buffer.
    /// Returns 0 if no stream is open or cancel is set.
    pub fn push_samples(&self, samples: &[i16]) -> usize {
        if self.cancel.load(Ordering::SeqCst) {
            return 0;
        }
        let mut slot = self.producer.lock().unwrap_or_else(|p| p.into_inner());
        match slot.as_mut() {
            None => 0,
            Some(producer) => producer.push_source_samples(samples),
        }
    }

    /// Flush producer-side resampler state into the active ring buffer.
    /// Returns the number of device-rate mono samples queued.
    pub fn flush_samples(&self) -> usize {
        if self.cancel.load(Ordering::SeqCst) {
            let mut slot = self.producer.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(producer) = slot.as_mut() {
                producer.reset();
            }
            return 0;
        }
        let mut slot = self.producer.lock().unwrap_or_else(|p| p.into_inner());
        match slot.as_mut() {
            None => 0,
            Some(producer) => producer.flush(),
        }
    }

    /// Set the cancel flag. Audio thread will drain the buffer + emit
    /// silence until [`Self::resume`] is called.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(producer) = self
            .producer
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_mut()
        {
            producer.reset();
        }
    }

    /// Clear the cancel flag.
    pub fn resume(&self) {
        self.cancel.store(false, Ordering::SeqCst);
    }

    /// Whether the cancel flag is currently set (barge-in / flush in effect).
    /// Diagnostic accessor — lets the converse runtime + tests observe that a
    /// `StopPlayback` action actually tripped cancellation without reaching
    /// into the private flag.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Free samples available in the active ring buffer. Returns 0 if no
    /// stream is open.
    pub fn free_samples(&self) -> usize {
        let slot = self.producer.lock().unwrap_or_else(|p| p.into_inner());
        slot.as_ref().map(PlaybackProducer::vacant_len).unwrap_or(0)
    }
}

fn validate_playback_config(config: PlaybackConfig) -> Result<(), PlaybackError> {
    if config.source_sample_rate == 0 {
        return Err(PlaybackError::BuildStream(
            "playback source sample rate must be greater than zero".to_string(),
        ));
    }
    if config.source_channels != 1 {
        return Err(PlaybackError::BuildStream(format!(
            "playback currently accepts mono source audio only, got {} channels",
            config.source_channels
        )));
    }
    Ok(())
}

impl Default for AudioPlayer {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AudioPlayer {
    fn drop(&mut self) {
        // Best-effort shutdown; thread observes Shutdown and exits.
        let _ = self.cmd_tx.send(AudioCommand::Shutdown);
    }
}

/// The audio thread's main loop. Owns each cpal::Stream + its consumer.
fn audio_thread_main(cmd_rx: Receiver<AudioCommand>, cancel: Arc<AtomicBool>) {
    let mut active_stream: Option<cpal::Stream> = None;

    while let Ok(cmd) = cmd_rx.recv() {
        match cmd {
            AudioCommand::OpenStream {
                device_name,
                config,
                consumer,
                reply,
            } => {
                drop(active_stream.take());
                let result = build_stream(device_name, config, consumer, cancel.clone());
                match result {
                    Ok(built) => match built.stream.play() {
                        Ok(()) => {
                            let info = PlaybackOpenInfo {
                                device_sample_rate: built.device_sample_rate,
                            };
                            active_stream = Some(built.stream);
                            let _ = reply.send(Ok(info));
                        }
                        Err(e) => {
                            let _ = reply.send(Err(format!("stream.play failed: {e}")));
                        }
                    },
                    Err(e) => {
                        let _ = reply.send(Err(e.to_string()));
                    }
                }
            }
            AudioCommand::StopStream => {
                drop(active_stream.take());
            }
            AudioCommand::Shutdown => {
                drop(active_stream.take());
                break;
            }
        }
    }
}

struct BuiltPlaybackStream {
    stream: cpal::Stream,
    device_sample_rate: u32,
}

#[allow(deprecated)] // see list_output_devices: name()-based device matching
fn build_stream(
    device_name: Option<String>,
    config: PlaybackConfig,
    consumer: HeapCons<i16>,
    cancel: Arc<AtomicBool>,
) -> Result<BuiltPlaybackStream, PlaybackError> {
    let host = cpal::default_host();
    let device = match device_name {
        None => host
            .default_output_device()
            .ok_or(PlaybackError::NoDefaultDevice)?,
        Some(name) => host
            .output_devices()
            .map_err(|e| PlaybackError::BuildStream(e.to_string()))?
            .find(|d| d.name().map(|n| n == name).unwrap_or(false))
            .ok_or(PlaybackError::DeviceNotFound(name))?,
    };

    let supported = device
        .default_output_config()
        .map_err(|e| PlaybackError::BuildStream(e.to_string()))?;
    let sample_format = supported.sample_format();
    let device_channels = supported.channels();
    let device_sample_rate = supported.sample_rate();
    let stream_config = StreamConfig {
        channels: device_channels,
        sample_rate: supported.sample_rate(),
        buffer_size: BufferSize::Default,
    };
    let source_channels = config.source_channels.max(1);

    // The consumer is moved into one of the format-specific closures.
    let err_cb = |e: StreamError| {
        log::warn!("cpal stream error: {e}");
    };

    // Three branches because cpal's build_output_stream is sample-format-
    // generic — we have to materialise the right type. Each branch shares
    // the same `pull_pcm` helper that drains the ringbuf consumer.
    let stream = match sample_format {
        SampleFormat::I16 => {
            let mut state = CallbackState {
                consumer,
                cancel: cancel.clone(),
                source_channels,
                device_channels,
            };
            device
                .build_output_stream(
                    &stream_config,
                    move |data: &mut [i16], _info| {
                        state.fill_i16(data);
                    },
                    err_cb,
                    None,
                )
                .map_err(|e| PlaybackError::BuildStream(e.to_string()))?
        }
        SampleFormat::F32 => {
            let mut state = CallbackState {
                consumer,
                cancel: cancel.clone(),
                source_channels,
                device_channels,
            };
            device
                .build_output_stream(
                    &stream_config,
                    move |data: &mut [f32], _info| {
                        state.fill_f32(data);
                    },
                    err_cb,
                    None,
                )
                .map_err(|e| PlaybackError::BuildStream(e.to_string()))?
        }
        SampleFormat::U16 => {
            let mut state = CallbackState {
                consumer,
                cancel: cancel.clone(),
                source_channels,
                device_channels,
            };
            device
                .build_output_stream(
                    &stream_config,
                    move |data: &mut [u16], _info| {
                        state.fill_u16(data);
                    },
                    err_cb,
                    None,
                )
                .map_err(|e| PlaybackError::BuildStream(e.to_string()))?
        }
        other => {
            return Err(PlaybackError::BuildStream(format!(
                "unsupported sample format {other:?}"
            )));
        }
    };

    Ok(BuiltPlaybackStream {
        stream,
        device_sample_rate,
    })
}

struct PlaybackProducer {
    prod: HeapProd<i16>,
    resampler: Option<MonoI16OutputResampler>,
}

impl PlaybackProducer {
    fn new(prod: HeapProd<i16>, source_rate: u32, output_rate: u32) -> Result<Self, String> {
        let resampler = if source_rate == output_rate {
            None
        } else {
            Some(MonoI16OutputResampler::new(source_rate, output_rate)?)
        };
        Ok(Self { prod, resampler })
    }

    fn push_source_samples(&mut self, samples: &[i16]) -> usize {
        if samples.is_empty() {
            return 0;
        }
        match self.resampler.as_mut() {
            None => self.prod.push_slice(samples),
            Some(resampler) => {
                let mut output = Vec::new();
                if let Err(err) = resampler.process(samples, &mut output) {
                    log::warn!("playback resampling failed: {err}");
                    resampler.reset();
                    return 0;
                }
                self.prod.push_slice(&output)
            }
        }
    }

    fn flush(&mut self) -> usize {
        let Some(resampler) = self.resampler.as_mut() else {
            return 0;
        };
        let mut output = Vec::new();
        if let Err(err) = resampler.finish(&mut output) {
            log::warn!("playback resampler flush failed: {err}");
            resampler.reset();
            return 0;
        }
        self.prod.push_slice(&output)
    }

    fn reset(&mut self) {
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
    }

    fn vacant_len(&self) -> usize {
        self.prod.vacant_len()
    }
}

const PLAYBACK_RESAMPLER_CHUNK_SIZE: usize = 1024;
const PLAYBACK_RESAMPLER_FLUSH_LIMIT: usize = 64;

struct MonoI16OutputResampler {
    source_rate: u32,
    output_rate: u32,
    resampler: Async<f32>,
    input_buffer: Vec<f32>,
    scratch_input: Vec<f32>,
    output_delay_remaining: usize,
    source_frames_seen: usize,
    output_frames_emitted: usize,
}

impl MonoI16OutputResampler {
    fn new(source_rate: u32, output_rate: u32) -> Result<Self, String> {
        if source_rate == 0 || output_rate == 0 {
            return Err("playback resampler sample rates must be greater than zero".to_string());
        }

        let resampler = Self::build_resampler(source_rate, output_rate)?;
        let output_delay_remaining = resampler.output_delay();
        Ok(Self {
            source_rate,
            output_rate,
            resampler,
            input_buffer: Vec::with_capacity(PLAYBACK_RESAMPLER_CHUNK_SIZE * 2),
            scratch_input: Vec::with_capacity(PLAYBACK_RESAMPLER_CHUNK_SIZE),
            output_delay_remaining,
            source_frames_seen: 0,
            output_frames_emitted: 0,
        })
    }

    fn process(&mut self, samples: &[i16], output: &mut Vec<i16>) -> Result<usize, String> {
        self.source_frames_seen = self.source_frames_seen.saturating_add(samples.len());
        self.input_buffer
            .extend(samples.iter().copied().map(i16_to_unit_f32));
        let before = output.len();
        while self.input_buffer.len() >= self.resampler.input_frames_next() {
            self.process_next_block(None, output)?;
        }
        Ok(output.len() - before)
    }

    fn finish(&mut self, output: &mut Vec<i16>) -> Result<usize, String> {
        let before = output.len();
        let target = self.expected_output_frames();
        if target == 0 {
            self.reset();
            return Ok(0);
        }

        if !self.input_buffer.is_empty() {
            let partial_len = self.input_buffer.len();
            self.process_next_block(Some(partial_len), output)?;
        }

        let mut flushes = 0usize;
        while self.output_frames_emitted < target {
            self.process_next_block(Some(0), output)?;
            flushes += 1;
            if flushes > PLAYBACK_RESAMPLER_FLUSH_LIMIT {
                self.reset();
                return Err("playback resampler did not flush to expected output length".into());
            }
        }

        self.reset();
        Ok(output.len() - before)
    }

    fn reset(&mut self) {
        self.resampler.reset();
        self.input_buffer.clear();
        self.scratch_input.clear();
        self.output_delay_remaining = self.resampler.output_delay();
        self.source_frames_seen = 0;
        self.output_frames_emitted = 0;
    }

    fn process_next_block(
        &mut self,
        partial_len: Option<usize>,
        output: &mut Vec<i16>,
    ) -> Result<(), String> {
        let needed = self.resampler.input_frames_next();
        self.scratch_input.clear();
        match partial_len {
            Some(valid) => {
                self.scratch_input.append(&mut self.input_buffer);
                self.scratch_input.resize(needed, 0.0);
                debug_assert!(valid <= needed);
            }
            None => {
                self.scratch_input.extend(self.input_buffer.drain(..needed));
            }
        }

        let input_adapter = SequentialSlice::new(&self.scratch_input, 1, needed)
            .map_err(|e| format!("failed to wrap playback resampler input: {e}"))?;
        let mut block = vec![0.0_f32; self.resampler.output_frames_next()];
        let output_frames = block.len();
        let mut output_adapter = SequentialSlice::new_mut(&mut block, 1, output_frames)
            .map_err(|e| format!("failed to wrap playback resampler output: {e}"))?;
        let indexing = partial_len.map(|valid| Indexing {
            input_offset: 0,
            output_offset: 0,
            partial_len: Some(valid),
            active_channels_mask: None,
        });
        self.resampler
            .process_into_buffer(&input_adapter, &mut output_adapter, indexing.as_ref())
            .map_err(|e| format!("playback resampler processing failed: {e}"))?;

        self.append_output_block(&block, output);
        Ok(())
    }

    fn append_output_block(&mut self, block: &[f32], output: &mut Vec<i16>) {
        let skip = self.output_delay_remaining.min(block.len());
        self.output_delay_remaining -= skip;
        let target = self.expected_output_frames();
        let remaining = target.saturating_sub(self.output_frames_emitted);
        let take = remaining.min(block.len().saturating_sub(skip));
        output.extend(
            block[skip..skip + take]
                .iter()
                .copied()
                .map(unit_f32_to_i16),
        );
        self.output_frames_emitted += take;
    }

    fn expected_output_frames(&self) -> usize {
        expected_resampled_frame_count(self.source_frames_seen, self.source_rate, self.output_rate)
    }

    fn build_resampler(source_rate: u32, output_rate: u32) -> Result<Async<f32>, String> {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            interpolation: SincInterpolationType::Linear,
            oversampling_factor: 256,
            window: WindowFunction::BlackmanHarris2,
        };
        Async::<f32>::new_sinc(
            output_rate as f64 / source_rate as f64,
            2.0,
            &params,
            PLAYBACK_RESAMPLER_CHUNK_SIZE,
            1,
            FixedAsync::Input,
        )
        .map_err(|e| format!("failed to create playback resampler: {e}"))
    }
}

fn expected_resampled_frame_count(
    source_frames: usize,
    source_rate: u32,
    output_rate: u32,
) -> usize {
    if source_frames == 0 || source_rate == 0 || output_rate == 0 {
        return 0;
    }
    ((source_frames as u128 * output_rate as u128).div_ceil(source_rate as u128)) as usize
}

fn i16_to_unit_f32(sample: i16) -> f32 {
    if sample == i16::MIN {
        -1.0
    } else {
        sample as f32 / i16::MAX as f32
    }
}

fn unit_f32_to_i16(sample: f32) -> i16 {
    let clamped = if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    if clamped >= 0.0 {
        (clamped * i16::MAX as f32) as i16
    } else {
        (clamped * -(i16::MIN as f32)) as i16
    }
}

/// Per-stream state living inside the cpal callback closure. Owns the
/// consumer side of the ring buffer + a clone of the cancel flag.
struct CallbackState {
    consumer: HeapCons<i16>,
    cancel: Arc<AtomicBool>,
    source_channels: u16,
    device_channels: u16,
}

impl CallbackState {
    /// Drain N source samples (mono i16) into a frame buffer. On cancel,
    /// drain everything pending and silence. The frame buffer is laid out
    /// per cpal convention: interleaved per-channel.
    fn pull_mono(&mut self, frames: usize) -> Vec<i16> {
        if self.cancel.load(Ordering::SeqCst) {
            // Drain whatever's queued so subsequent resume() doesn't play
            // stale samples.
            let mut bin = vec![0i16; self.consumer.occupied_len()];
            let n = self.consumer.pop_slice(&mut bin);
            let _ = (bin, n); // discard
            return vec![0i16; frames];
        }
        let mut buf = vec![0i16; frames];
        self.consumer.pop_slice(&mut buf);
        buf
    }

    fn fill_i16(&mut self, data: &mut [i16]) {
        let frames = data.len() / self.device_channels as usize;
        let mono = self.pull_mono(frames);
        write_interleaved_i16(data, &mono, self.source_channels, self.device_channels);
    }

    fn fill_f32(&mut self, data: &mut [f32]) {
        let frames = data.len() / self.device_channels as usize;
        let mono = self.pull_mono(frames);
        write_interleaved_f32(data, &mono, self.source_channels, self.device_channels);
    }

    fn fill_u16(&mut self, data: &mut [u16]) {
        let frames = data.len() / self.device_channels as usize;
        let mono = self.pull_mono(frames);
        write_interleaved_u16(data, &mono, self.source_channels, self.device_channels);
    }
}

/// Spread mono source samples across N device channels by duplication.
/// Source stereo (2-channel) is downmixed to mono averaging in the producer
/// path; this MVP doesn't handle source_channels > 1 specially — caller is
/// expected to feed mono.
fn write_interleaved_i16(out: &mut [i16], mono: &[i16], _src_ch: u16, dev_ch: u16) {
    for (frame_idx, &sample) in mono.iter().enumerate() {
        for ch in 0..dev_ch as usize {
            let idx = frame_idx * dev_ch as usize + ch;
            if idx < out.len() {
                out[idx] = sample;
            }
        }
    }
}

fn write_interleaved_f32(out: &mut [f32], mono: &[i16], _src_ch: u16, dev_ch: u16) {
    let scale = 1.0_f32 / i16::MAX as f32;
    for (frame_idx, &sample) in mono.iter().enumerate() {
        let f = sample as f32 * scale;
        for ch in 0..dev_ch as usize {
            let idx = frame_idx * dev_ch as usize + ch;
            if idx < out.len() {
                out[idx] = f;
            }
        }
    }
}

fn write_interleaved_u16(out: &mut [u16], mono: &[i16], _src_ch: u16, dev_ch: u16) {
    for (frame_idx, &sample) in mono.iter().enumerate() {
        let u = (sample as i32 + 32_768) as u16;
        for ch in 0..dev_ch as usize {
            let idx = frame_idx * dev_ch as usize + ch;
            if idx < out.len() {
                out[idx] = u;
            }
        }
    }
}
