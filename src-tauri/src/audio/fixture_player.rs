//! `fixture-player` — a small standalone binary that plays a fixture WAV out
//! a named output device on a loop, for the strict `live-audio-smoke` CI
//! pass (seed audio-graph-f166). See `src/bin/fixture_player.rs` for the
//! thin binary wrapper; all real logic lives here so it is testable without
//! an audio device (`cargo test -p audio-graph --lib`).
//!
//! ## CLI
//!
//! ```text
//! fixture-player --list-devices
//! fixture-player --device <NAME> <FIXTURE.wav>
//! ```
//!
//! `--device` is required (not defaulted to "whatever the host calls
//! default") so a CI job always knows exactly which sink it is feeding.
//! Once the device opens, the player prints a single `READY pid=<PID>` line
//! to stdout and flushes — a driving shell script waits for this line before
//! starting whatever it wants to capture, then either closes the player's
//! stdin or sends it `SIGTERM` (Unix) to stop it. The player loops the
//! fixture's samples until told to stop, then exits promptly.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::audio::wav_io::{self, WavPcm16};
use crate::playback::{AudioPlayer, PlaybackConfig, list_output_devices};

/// Parsed CLI intent. Kept separate from IO so `parse_args` is a pure
/// function unit-testable with no device, no filesystem, no process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlayerCommand {
    /// `--list-devices`: print every output device cpal enumerates and exit.
    ListDevices,
    /// `--device <NAME> <FIXTURE.wav>`: play `FIXTURE.wav` on a loop into
    /// the named device.
    Play {
        device: String,
        fixture_path: PathBuf,
    },
}

/// Typed argument-parsing failures. Every variant names exactly what was
/// wrong so a misinvoked CI step fails with an actionable message instead of
/// a generic usage dump.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ArgsError {
    #[error("--device requires a value")]
    DeviceValueMissing,
    #[error("unknown flag {0:?}")]
    UnknownFlag(String),
    #[error("unexpected extra positional argument {0:?} (fixture path already given)")]
    TooManyPositionalArgs(String),
    #[error("--device <NAME> is required (use --list-devices to see available names)")]
    MissingDevice,
    #[error("a fixture WAV path is required")]
    MissingFixturePath,
}

/// Parse a full `argv` (including `argv[0]`, matching `std::env::args()`) into
/// a [`PlayerCommand`]. Pure — touches no filesystem, no device, no process.
pub fn parse_args<I>(argv: I) -> Result<PlayerCommand, ArgsError>
where
    I: IntoIterator<Item = String>,
{
    let mut iter = argv.into_iter();
    let _program_name = iter.next();

    let mut list_devices = false;
    let mut device: Option<String> = None;
    let mut fixture_path: Option<PathBuf> = None;

    while let Some(arg) = iter.next() {
        if arg == "--list-devices" {
            list_devices = true;
        } else if arg == "--device" {
            let value = iter.next().ok_or(ArgsError::DeviceValueMissing)?;
            device = Some(value);
        } else if let Some(value) = arg.strip_prefix("--device=") {
            device = Some(value.to_string());
        } else if arg.starts_with("--") {
            return Err(ArgsError::UnknownFlag(arg));
        } else if fixture_path.is_some() {
            return Err(ArgsError::TooManyPositionalArgs(arg));
        } else {
            fixture_path = Some(PathBuf::from(arg));
        }
    }

    if list_devices {
        return Ok(PlayerCommand::ListDevices);
    }
    let device = device.ok_or(ArgsError::MissingDevice)?;
    let fixture_path = fixture_path.ok_or(ArgsError::MissingFixturePath)?;
    Ok(PlayerCommand::Play {
        device,
        fixture_path,
    })
}

/// Errors that can occur once we start acting on a parsed [`PlayerCommand`].
#[derive(Debug, thiserror::Error)]
pub enum PlayerError {
    #[error("failed to read fixture WAV {path}: {source}")]
    ReadFixture {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("failed to decode fixture WAV {path}: {source}")]
    DecodeFixture {
        path: PathBuf,
        #[source]
        source: wav_io::WavError,
    },
    #[error(
        "fixture WAV {path:?} has {channels} channel(s); the fixture-player only plays mono fixtures"
    )]
    NotMono { channels: u16, path: PathBuf },
    #[error("failed to open output device {device:?}: {source}")]
    OpenDevice {
        device: String,
        #[source]
        source: crate::playback::PlaybackError,
    },
}

/// Read and decode a fixture WAV from disk, rejecting anything that is not
/// mono. Separated from [`run`] so it is unit-testable without a device.
fn read_fixture_wav(path: &Path) -> Result<WavPcm16, PlayerError> {
    let bytes = std::fs::read(path).map_err(|source| PlayerError::ReadFixture {
        path: path.to_path_buf(),
        source,
    })?;
    let decoded = wav_io::decode(&bytes).map_err(|source| PlayerError::DecodeFixture {
        path: path.to_path_buf(),
        source,
    })?;
    if decoded.channels != 1 {
        return Err(PlayerError::NotMono {
            channels: decoded.channels,
            path: path.to_path_buf(),
        });
    }
    Ok(decoded)
}

/// Entry point called by `src/bin/fixture_player.rs`. Returns a process exit
/// code; never panics on a bad invocation.
pub fn run(argv: Vec<String>) -> i32 {
    let command = match parse_args(argv) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("fixture-player: {error}");
            return 2;
        }
    };

    match command {
        PlayerCommand::ListDevices => {
            for device in list_output_devices() {
                if device.is_default {
                    println!("{} (default)", device.name);
                } else {
                    println!("{}", device.name);
                }
            }
            0
        }
        PlayerCommand::Play {
            device,
            fixture_path,
        } => run_play(&device, &fixture_path),
    }
}

fn run_play(device: &str, fixture_path: &Path) -> i32 {
    let fixture = match read_fixture_wav(fixture_path) {
        Ok(fixture) => fixture,
        Err(error) => {
            eprintln!("fixture-player: {error}");
            return 3;
        }
    };

    let player = AudioPlayer::new();
    if let Err(source) = player.open_named(
        device.to_string(),
        PlaybackConfig {
            source_sample_rate: fixture.sample_rate,
            source_channels: 1,
        },
    ) {
        eprintln!(
            "fixture-player: {}",
            PlayerError::OpenDevice {
                device: device.to_string(),
                source,
            }
        );
        return 4;
    }

    // Ready line: exactly one line, flushed immediately, so a driving shell
    // script can reliably wait for it before proceeding.
    println!("READY pid={}", std::process::id());
    if io::stdout().flush().is_err() {
        // Nothing useful to do if stdout itself is broken; keep going —
        // playback can still be evidence-worthy even if the ready line was
        // lost, and the caller's own wait will simply time out.
    }

    let stop = Arc::new(AtomicBool::new(false));
    spawn_stdin_close_watcher(Arc::clone(&stop));
    spawn_sigterm_watcher(Arc::clone(&stop));

    loop_fixture_until_stopped(&player, &fixture.samples, fixture.sample_rate, &stop);

    let _ = player.stop();
    0
}

/// Blocks on stdin until it returns EOF (closed) or an error, then sets
/// `stop`. Runs on its own thread so the playback loop is free to poll.
fn spawn_stdin_close_watcher(stop: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        loop {
            match io::stdin().read(&mut byte) {
                Ok(0) => break,    // EOF: stdin closed.
                Ok(_) => continue, // Ignore any bytes; only EOF/close matters.
                Err(_) => break,
            }
        }
        stop.store(true, Ordering::SeqCst);
    });
}

/// On Unix, watches for `SIGTERM` and sets `stop` when received. A no-op on
/// non-Unix targets — the stdin-close path still works there.
fn spawn_sigterm_watcher(stop: Arc<AtomicBool>) {
    #[cfg(unix)]
    {
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(_) => return,
            };
            runtime.block_on(async {
                if let Ok(mut term) =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                {
                    term.recv().await;
                    stop.store(true, Ordering::SeqCst);
                }
            });
        });
    }
    #[cfg(not(unix))]
    {
        let _ = stop;
    }
}

/// Push `samples` into `player` in ~100ms chunks, looping back to the start
/// whenever the fixture ends, until `stop` is set. Backpressure comes from
/// `AudioPlayer::free_samples` so the loop naturally paces to real time
/// instead of racing ahead of the device.
///
/// `AudioPlayer::free_samples` reports **device-rate** (post-resample) ring
/// vacancy, while `chunk` is a **source-rate** (fixture) sample count.
/// Whenever the opened device does not run at the fixture's own sample rate,
/// gating on `chunk.len()` directly under-reserves ring space: the resampled
/// output can be several times longer than the source chunk, and the excess
/// gets silently dropped by the ring buffer's `push_slice` (its return value
/// is unused by design — see `AudioPlayer::push_samples`). We convert the
/// gate to the device-rate sample count the chunk is expected to resample
/// to via [`device_samples_for_chunk`] before waiting, so the push below
/// always has room for the whole resampled chunk.
fn loop_fixture_until_stopped(
    player: &AudioPlayer,
    samples: &[i16],
    sample_rate: u32,
    stop: &AtomicBool,
) {
    if samples.is_empty() {
        return;
    }
    let chunk_len = ((sample_rate / 10).max(1) as usize).min(samples.len());
    let mut cursor = 0usize;

    while !stop.load(Ordering::SeqCst) {
        let end = (cursor + chunk_len).min(samples.len());
        let chunk = &samples[cursor..end];
        let needed = device_samples_for_chunk(player, chunk.len(), sample_rate);

        while player.free_samples() < needed {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        player.push_samples(chunk);

        cursor = end;
        if cursor >= samples.len() {
            cursor = 0;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Device-rate sample count that pushing `chunk_len` source-rate samples is
/// expected to produce once resampled, rounded up, plus a small fixed margin
/// to absorb the resampler's own block-based rounding (it buffers input into
/// fixed-size blocks internally, so any single `push_samples` call's actual
/// output can drift slightly from the steady-state ratio). The margin
/// (4096 device-rate samples, worst case ~85 ms at 48 kHz) is tiny relative
/// to the ring's default 192_000-sample capacity, so it does not meaningfully
/// change playback pacing.
///
/// Falls back to `chunk_len` unchanged (1:1) if the player has no open
/// stream yet — defensive only, since `run_play` always opens the device
/// before looping — or if the source and device rates match.
fn device_samples_for_chunk(
    player: &AudioPlayer,
    chunk_len: usize,
    fallback_source_rate: u32,
) -> usize {
    let (source_rate, device_rate) = player
        .sample_rates()
        .unwrap_or((fallback_source_rate, fallback_source_rate));
    device_samples_for_rates(chunk_len, source_rate, device_rate)
}

/// Pure rate-conversion math behind [`device_samples_for_chunk`], split out
/// so it is unit-testable without a real audio device.
fn device_samples_for_rates(chunk_len: usize, source_rate: u32, device_rate: u32) -> usize {
    if source_rate == 0 || device_rate == 0 || source_rate == device_rate {
        return chunk_len;
    }
    let needed = (chunk_len as u128 * device_rate as u128).div_ceil(source_rate as u128) as usize;
    needed.saturating_add(4096)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(parts: &[&str]) -> Vec<String> {
        std::iter::once("fixture-player".to_string())
            .chain(parts.iter().map(|s| s.to_string()))
            .collect()
    }

    #[test]
    fn parses_list_devices() {
        assert_eq!(
            parse_args(argv(&["--list-devices"])),
            Ok(PlayerCommand::ListDevices)
        );
    }

    #[test]
    fn list_devices_ignores_missing_device_and_fixture() {
        // --list-devices short-circuits before device/fixture are required.
        assert_eq!(
            parse_args(argv(&["--list-devices"])),
            Ok(PlayerCommand::ListDevices)
        );
    }

    #[test]
    fn parses_device_and_fixture_path() {
        assert_eq!(
            parse_args(argv(&["--device", "ag_sink", "fixture.wav"])),
            Ok(PlayerCommand::Play {
                device: "ag_sink".to_string(),
                fixture_path: PathBuf::from("fixture.wav"),
            })
        );
    }

    #[test]
    fn parses_device_equals_form() {
        assert_eq!(
            parse_args(argv(&["--device=ag_sink", "fixture.wav"])),
            Ok(PlayerCommand::Play {
                device: "ag_sink".to_string(),
                fixture_path: PathBuf::from("fixture.wav"),
            })
        );
    }

    #[test]
    fn accepts_flags_in_either_order() {
        assert_eq!(
            parse_args(argv(&["fixture.wav", "--device", "ag_sink"])),
            Ok(PlayerCommand::Play {
                device: "ag_sink".to_string(),
                fixture_path: PathBuf::from("fixture.wav"),
            })
        );
    }

    #[test]
    fn rejects_missing_device() {
        assert_eq!(
            parse_args(argv(&["fixture.wav"])),
            Err(ArgsError::MissingDevice)
        );
    }

    #[test]
    fn rejects_missing_fixture_path() {
        assert_eq!(
            parse_args(argv(&["--device", "ag_sink"])),
            Err(ArgsError::MissingFixturePath)
        );
    }

    #[test]
    fn rejects_device_flag_with_no_value() {
        assert_eq!(
            parse_args(argv(&["--device"])),
            Err(ArgsError::DeviceValueMissing)
        );
    }

    #[test]
    fn rejects_unknown_flag() {
        assert_eq!(
            parse_args(argv(&["--bogus"])),
            Err(ArgsError::UnknownFlag("--bogus".to_string()))
        );
    }

    #[test]
    fn rejects_a_second_positional_argument() {
        assert_eq!(
            parse_args(argv(&["--device", "ag_sink", "a.wav", "b.wav"])),
            Err(ArgsError::TooManyPositionalArgs("b.wav".to_string()))
        );
    }

    #[test]
    fn no_arguments_is_missing_device() {
        assert_eq!(parse_args(argv(&[])), Err(ArgsError::MissingDevice));
    }

    fn write_temp_wav(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-fixture-player-test-{name}-{}.wav",
            std::process::id()
        ));
        std::fs::write(&path, bytes).expect("write temp wav");
        path
    }

    #[test]
    fn read_fixture_wav_decodes_a_valid_mono_file() {
        let bytes = wav_io::encode(&[0, 100, -100, 200], 16_000, 1);
        let path = write_temp_wav("valid-mono", &bytes);
        let decoded = read_fixture_wav(&path).expect("valid mono WAV must decode");
        assert_eq!(decoded.sample_rate, 16_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.samples, vec![0, 100, -100, 200]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_fixture_wav_rejects_stereo_fixtures() {
        let bytes = wav_io::encode(&[0, 0, 1, 1], 16_000, 2);
        let path = write_temp_wav("stereo", &bytes);
        let error = read_fixture_wav(&path).expect_err("stereo fixture must be rejected");
        assert!(matches!(error, PlayerError::NotMono { channels: 2, .. }));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_fixture_wav_reports_missing_file() {
        let path = PathBuf::from("/nonexistent/audio-graph-fixture-player-missing.wav");
        let error = read_fixture_wav(&path).expect_err("missing file must error");
        assert!(matches!(error, PlayerError::ReadFixture { .. }));
    }

    // ---- backpressure gate: device-rate vs source-rate sample counts ----

    #[test]
    fn device_samples_for_rates_is_unchanged_at_1_to_1() {
        assert_eq!(device_samples_for_rates(1600, 16_000, 16_000), 1600);
    }

    #[test]
    fn device_samples_for_rates_scales_up_when_device_runs_faster() {
        // The exact scenario from the finding: a 16 kHz fixture chunk (100ms
        // @ 1600 samples) against a 44.1 kHz device. Gating on the raw
        // source chunk length (1600) instead of the resampled device-rate
        // count (~4410) would under-reserve ring space by nearly 3x.
        let needed = device_samples_for_rates(1_600, 16_000, 44_100);
        assert!(
            needed >= 4_410,
            "expected at least the resampled device-rate count (4410), got {needed}"
        );
    }

    #[test]
    fn device_samples_for_rates_falls_back_to_chunk_len_on_degenerate_rates() {
        assert_eq!(device_samples_for_rates(1600, 0, 44_100), 1600);
        assert_eq!(device_samples_for_rates(1600, 16_000, 0), 1600);
    }

    #[test]
    fn device_samples_for_chunk_falls_back_when_no_stream_is_open() {
        // A freshly constructed AudioPlayer has no open stream, so
        // `sample_rates()` is `None` and the fallback (source rate == the
        // caller-supplied rate) must leave the chunk length unchanged.
        let player = AudioPlayer::new();
        assert_eq!(device_samples_for_chunk(&player, 1_600, 16_000), 1_600);
    }

    #[test]
    fn read_fixture_wav_reports_corrupt_file() {
        let path = write_temp_wav("corrupt", b"not a wav file at all");
        let error = read_fixture_wav(&path).expect_err("corrupt file must error");
        assert!(matches!(error, PlayerError::DecodeFixture { .. }));
        let _ = std::fs::remove_file(&path);
    }
}
