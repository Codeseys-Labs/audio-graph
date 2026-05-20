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
//! - **Cancel/barge-in** is an `Arc<AtomicBool>`. When set, the callback
//!   drains the ring buffer and emits silence. Audible cut-off is bounded
//!   by one callback period (~10–20 ms typical).
//! - **Resampling**: the MVP plays at the source rate without resampling.
//!   If the device wants 48 kHz and the source is 24 kHz, the output is
//!   pitched down 1 octave + plays half-speed. Wave C wires the producer
//!   to feed pre-resampled samples for the production speak-aloud path
//!   (matches the device's preferred rate). For now the API exposes
//!   `open_default(_with_rate)` so a caller can drive the device at the
//!   source rate.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, SampleRate, StreamConfig, StreamError};
use crossbeam_channel::{unbounded, Receiver, Sender};
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapProd, HeapRb};

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
        /// One-shot reply so the caller knows whether the open succeeded.
        reply: crossbeam_channel::Sender<Result<(), String>>,
    },
    /// Stop the stream and discard the consumer side. Safe to call when no
    /// stream is open (no-op).
    StopStream,
    /// Terminate the audio thread.
    Shutdown,
}

/// Output-device list helper. Read-only; safe to call from any thread.
///
/// First entry (if any) is the host's default device.
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
    producer: Arc<std::sync::Mutex<Option<HeapProd<i16>>>>,
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
        // Build a fresh per-stream ringbuf. Producer side replaces what
        // AudioPlayer has stored; consumer side ships to the audio thread.
        let rb = HeapRb::<i16>::new(self.capacity);
        let (prod, cons) = rb.split();
        // Reset cancel so a previous barge-in doesn't immediately mute the
        // new stream.
        self.cancel.store(false, Ordering::SeqCst);
        // Replace stored producer.
        {
            let mut slot = self.producer.lock().unwrap_or_else(|p| p.into_inner());
            *slot = Some(prod);
        }
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
            Ok(()) => Ok(()),
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

    /// Push samples into the active ring buffer. Returns the count actually
    /// written (≤ samples.len()). Returns 0 if no stream is open or cancel
    /// is set.
    pub fn push_samples(&self, samples: &[i16]) -> usize {
        if self.cancel.load(Ordering::SeqCst) {
            return 0;
        }
        let mut slot = self.producer.lock().unwrap_or_else(|p| p.into_inner());
        match slot.as_mut() {
            None => 0,
            Some(prod) => prod.push_slice(samples),
        }
    }

    /// Set the cancel flag. Audio thread will drain the buffer + emit
    /// silence until [`Self::resume`] is called.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Clear the cancel flag.
    pub fn resume(&self) {
        self.cancel.store(false, Ordering::SeqCst);
    }

    /// Free samples available in the active ring buffer. Returns 0 if no
    /// stream is open.
    pub fn free_samples(&self) -> usize {
        let slot = self.producer.lock().unwrap_or_else(|p| p.into_inner());
        slot.as_ref().map(|p| p.vacant_len()).unwrap_or(0)
    }
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
                    Ok(stream) => match stream.play() {
                        Ok(()) => {
                            active_stream = Some(stream);
                            let _ = reply.send(Ok(()));
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

fn build_stream(
    device_name: Option<String>,
    config: PlaybackConfig,
    consumer: HeapCons<i16>,
    cancel: Arc<AtomicBool>,
) -> Result<cpal::Stream, PlaybackError> {
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

    // The MVP uses the source rate as the device rate. Producers requested
    // for production (Wave C) provide samples at the device-preferred rate;
    // a future improvement is automatic on-the-fly resampling. For now: ask
    // cpal to run at the source rate so playback isn't pitch-shifted, even
    // though some devices may reject and force the build to error.
    let supported = device
        .default_output_config()
        .map_err(|e| PlaybackError::BuildStream(e.to_string()))?;
    let sample_format = supported.sample_format();
    let device_channels = supported.channels();
    let stream_config = StreamConfig {
        channels: device_channels,
        sample_rate: SampleRate(config.source_sample_rate),
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

    Ok(stream)
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
