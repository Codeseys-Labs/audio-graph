//! Offline validator for the AEC/VAD playback-reference fixture harness
//! (seed audio-graph-098b).
//!
//! This `#[cfg(test)]` module mirrors `source_separation_fixtures.rs`: it parses
//! the checked-in manifest, hand-parses each WAV header (no `hound` dependency),
//! and asserts the harness invariants offline — no audio device, no ML feature,
//! no real AEC dependency. It builds and passes under `--features cloud`.
//!
//! Because the fixtures are *synthesized* (deterministic, secret-free), the
//! validator regenerates any missing WAV from
//! [`crate::aec_vad::synthesize_fixture`] before validating, so a clean checkout
//! is self-healing and the bytes are reproducible.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::aec_vad::{FIXTURE_SAMPLE_RATE_HZ, encode_wav_pcm_s16le, synthesize_fixture};

#[derive(Debug, Deserialize)]
struct AecVadManifest {
    schema_version: u32,
    id: String,
    format: AudioFormat,
    provenance: Provenance,
    seam: Seam,
    fixtures: Vec<Fixture>,
    metrics: Metrics,
}

#[derive(Debug, Deserialize)]
struct AudioFormat {
    container: String,
    encoding: String,
    sample_rate: u32,
    channels: u16,
    bit_depth: u16,
}

#[derive(Debug, Deserialize)]
struct Provenance {
    kind: String,
    secret_free: bool,
    deterministic: bool,
    generator: String,
}

#[derive(Debug, Deserialize)]
struct Seam {
    aec_runs_before_processed_bus: bool,
    processed_bus_sample_rate_hz: u32,
    alignment_primitive: String,
    must_not_mutate_asr_chunks: bool,
    real_candidate_owned_by_seed: String,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    id: String,
    class: FixtureClass,
    duration_ms: u64,
    assistant_rendering: bool,
    user_speaking: bool,
    capture_track: Track,
    render_reference_track: Track,
    speech_regions: Vec<Region>,
    overlap_regions: Vec<OverlapRegion>,
}

#[derive(Debug, Deserialize, PartialEq, Eq, Hash, Clone, Copy)]
#[serde(rename_all = "kebab-case")]
enum FixtureClass {
    EchoOnly,
    UserBargeInOverAssistant,
    KeyboardNoise,
    QuietRoom,
    OverlappedSpeech,
}

impl FixtureClass {
    /// The string key understood by [`synthesize_fixture`].
    fn synth_key(self) -> &'static str {
        match self {
            FixtureClass::EchoOnly => "echo-only",
            FixtureClass::UserBargeInOverAssistant => "user-barge-in-over-assistant",
            FixtureClass::KeyboardNoise => "keyboard-noise",
            FixtureClass::QuietRoom => "quiet-room",
            FixtureClass::OverlappedSpeech => "overlapped-speech",
        }
    }
}

#[derive(Debug, Deserialize)]
struct Track {
    audio_path: String,
    start_ms: u64,
    duration_ms: u64,
}

#[derive(Debug, Deserialize)]
struct Region {
    source: String,
    start_ms: u64,
    end_ms: u64,
}

#[derive(Debug, Deserialize)]
struct OverlapRegion {
    start_ms: u64,
    end_ms: u64,
    sources: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct Metrics {
    axes: Vec<String>,
    reporter: String,
    baseline_status: String,
    real_candidate_required_before_close: bool,
}

#[derive(Debug)]
struct WavInfo {
    audio_format: u16,
    channels: u16,
    sample_rate: u32,
    bit_depth: u16,
    data_bytes: u32,
}

#[test]
fn aec_vad_manifest_has_required_fixture_shapes() {
    let manifest = load_manifest();

    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.id, "aec-vad-playback-reference-mini-v1");
    assert_eq!(manifest.format.container, "wav");
    assert_eq!(manifest.format.encoding, "pcm_s16le");
    assert_eq!(manifest.format.sample_rate, 16_000);
    assert_eq!(manifest.format.channels, 1);
    assert_eq!(manifest.format.bit_depth, 16);

    // Provenance: synthesized, secret-free, deterministic — no real dataset.
    assert_eq!(manifest.provenance.kind, "synthesized");
    assert!(manifest.provenance.secret_free);
    assert!(manifest.provenance.deterministic);
    assert!(manifest.provenance.generator.contains("synthesize_fixture"));

    // Seam guardrails: the AEC stage sits before the 16 kHz bus and must not
    // mutate ASR chunks; the real candidate is owned by the parent bakeoff seed.
    //
    // The processed-audio bus is canonically 16 kHz mono (see
    // `audio::pipeline` — the bus rate the harness fixtures match so no resample
    // is needed before scoring). We anchor the manifest's declared bus rate to
    // the harness sample rate, which equals the canonical bus rate by design.
    assert!(manifest.seam.aec_runs_before_processed_bus);
    assert_eq!(manifest.seam.processed_bus_sample_rate_hz, 16_000);
    assert_eq!(
        manifest.seam.processed_bus_sample_rate_hz,
        crate::aec_vad::FIXTURE_SAMPLE_RATE_HZ
    );
    assert_eq!(
        manifest.seam.alignment_primitive,
        "ProcessedAudioChunk.timestamp"
    );
    assert!(manifest.seam.must_not_mutate_asr_chunks);
    assert_eq!(
        manifest.seam.real_candidate_owned_by_seed,
        "audio-graph-0bdc"
    );

    // All five fixture classes must be present exactly once.
    let classes: std::collections::HashSet<FixtureClass> =
        manifest.fixtures.iter().map(|f| f.class).collect();
    assert_eq!(
        classes.len(),
        5,
        "manifest must define all five distinct fixture classes"
    );
    for expected in [
        FixtureClass::EchoOnly,
        FixtureClass::UserBargeInOverAssistant,
        FixtureClass::KeyboardNoise,
        FixtureClass::QuietRoom,
        FixtureClass::OverlappedSpeech,
    ] {
        assert!(
            classes.contains(&expected),
            "missing fixture class {expected:?}"
        );
    }

    for fixture in &manifest.fixtures {
        assert!(!fixture.id.trim().is_empty());
        assert!(fixture.duration_ms > 0);

        // Capture and render-reference tracks must be time-aligned: same start
        // and same duration so the two streams line up sample-for-sample via
        // the ProcessedAudioChunk.timestamp primitive.
        assert_eq!(
            fixture.capture_track.start_ms, fixture.render_reference_track.start_ms,
            "{} capture/reference must share a start timestamp",
            fixture.id
        );
        assert_eq!(
            fixture.capture_track.duration_ms, fixture.render_reference_track.duration_ms,
            "{} capture/reference must share a duration",
            fixture.id
        );
        assert_eq!(
            fixture.capture_track.start_ms, 0,
            "{} fixtures start aligned at 0 ms",
            fixture.id
        );

        // Speech regions must be well-formed and inside the clip.
        for region in &fixture.speech_regions {
            assert!(!region.source.trim().is_empty());
            assert!(
                region.start_ms < region.end_ms,
                "{} region {} has non-positive span",
                fixture.id,
                region.source
            );
            assert!(region.end_ms <= fixture.duration_ms);
        }

        // Overlap regions must be consistent with the actual speech regions.
        for overlap in &fixture.overlap_regions {
            assert!(overlap.start_ms < overlap.end_ms);
            assert!(overlap.end_ms <= fixture.duration_ms);
            assert!(
                overlap.sources.len() >= 2,
                "{} overlap region must name >=2 sources",
                fixture.id
            );
            assert!(
                region_actually_overlaps(&fixture.speech_regions, overlap),
                "{} overlap region {}..{} must be backed by overlapping speech regions",
                fixture.id,
                overlap.start_ms,
                overlap.end_ms
            );
        }

        // Class-specific structural assertions.
        match fixture.class {
            FixtureClass::EchoOnly => {
                assert!(fixture.assistant_rendering);
                assert!(!fixture.user_speaking);
                assert!(
                    fixture.overlap_regions.is_empty(),
                    "echo-only must not overlap user speech"
                );
            }
            FixtureClass::UserBargeInOverAssistant => {
                assert!(fixture.assistant_rendering);
                assert!(fixture.user_speaking);
                assert!(
                    !fixture.overlap_regions.is_empty(),
                    "barge-in fixture MUST overlap (user over assistant)"
                );
            }
            FixtureClass::OverlappedSpeech => {
                assert!(fixture.user_speaking);
                assert!(
                    !fixture.overlap_regions.is_empty(),
                    "overlapped-speech fixture MUST overlap"
                );
            }
            FixtureClass::KeyboardNoise | FixtureClass::QuietRoom => {
                assert!(!fixture.assistant_rendering);
                assert!(!fixture.user_speaking);
                assert!(fixture.overlap_regions.is_empty());
                assert!(fixture.speech_regions.is_empty());
            }
        }
    }

    // Metrics block must enumerate every required reporting axis.
    for axis in [
        "echo_leak",
        "false_start_rate",
        "missed_barge_in_rate",
        "latency",
        "rtf",
        "cpu_fraction",
        "binary_footprint_bytes",
        "model_footprint_bytes",
    ] {
        assert!(
            manifest.metrics.axes.iter().any(|a| a == axis),
            "metrics axis {axis} missing from manifest"
        );
    }
    assert!(manifest.metrics.reporter.contains("AecMetricsReporter"));
    assert_eq!(manifest.metrics.baseline_status, "pending_real_candidate");
    assert!(manifest.metrics.real_candidate_required_before_close);
}

#[test]
fn aec_vad_wavs_match_manifest_format_and_are_aligned() {
    let manifest = load_manifest();
    ensure_fixtures_generated(&manifest);

    for fixture in &manifest.fixtures {
        let capture = parse_wav_info(&fixture_root().join(&fixture.capture_track.audio_path));
        let reference =
            parse_wav_info(&fixture_root().join(&fixture.render_reference_track.audio_path));

        for (label, info, expected_duration) in [
            ("capture", &capture, fixture.capture_track.duration_ms),
            (
                "reference",
                &reference,
                fixture.render_reference_track.duration_ms,
            ),
        ] {
            assert_eq!(info.audio_format, 1, "{} {label} must be PCM", fixture.id);
            assert_eq!(
                info.sample_rate, manifest.format.sample_rate,
                "{} {label} sample rate",
                fixture.id
            );
            assert_eq!(
                info.channels, manifest.format.channels,
                "{} {label} channels",
                fixture.id
            );
            assert_eq!(
                info.bit_depth, manifest.format.bit_depth,
                "{} {label} bit depth",
                fixture.id
            );
            assert_eq!(
                wav_duration_ms(info),
                expected_duration,
                "{} {label} duration",
                fixture.id
            );
        }

        // Sample-for-sample alignment: identical frame counts mean the two
        // tracks share a timebase and can be aligned via the timestamp anchor.
        assert_eq!(
            wav_frame_count(&capture),
            wav_frame_count(&reference),
            "{} capture and render reference must have identical frame counts",
            fixture.id
        );
    }
}

/// Regenerate any missing fixture WAV from the deterministic synthesizer so a
/// fresh checkout self-heals and the bytes stay reproducible.
fn ensure_fixtures_generated(manifest: &AecVadManifest) {
    for fixture in &manifest.fixtures {
        let synth = synthesize_fixture(fixture.class.synth_key(), FIXTURE_SAMPLE_RATE_HZ);
        write_if_missing(
            &fixture_root().join(&fixture.capture_track.audio_path),
            &synth.capture,
        );
        write_if_missing(
            &fixture_root().join(&fixture.render_reference_track.audio_path),
            &synth.render_reference,
        );
    }
}

fn write_if_missing(path: &Path, samples: &[f32]) {
    if path.exists() {
        return;
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .unwrap_or_else(|error| panic!("failed to create {}: {error}", parent.display()));
    }
    let bytes = encode_wav_pcm_s16le(samples, FIXTURE_SAMPLE_RATE_HZ);
    fs::write(path, bytes)
        .unwrap_or_else(|error| panic!("failed to write {}: {error}", path.display()));
}

fn load_manifest() -> AecVadManifest {
    let path = fixture_root().join("manifest.json");
    let body = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("aec_vad")
}

/// `true` when at least two distinct speech regions actually cover the claimed
/// overlap window.
fn region_actually_overlaps(regions: &[Region], overlap: &OverlapRegion) -> bool {
    let covering = regions
        .iter()
        .filter(|r| r.start_ms < overlap.end_ms && overlap.start_ms < r.end_ms)
        .count();
    covering >= 2
}

fn parse_wav_info(path: &Path) -> WavInfo {
    let data =
        fs::read(path).unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    assert!(data.len() >= 44, "{} is too small for WAV", path.display());
    assert_eq!(&data[0..4], b"RIFF", "{} RIFF header", path.display());
    assert_eq!(&data[8..12], b"WAVE", "{} WAVE header", path.display());

    let mut offset = 12usize;
    let mut audio_format = None;
    let mut channels = None;
    let mut sample_rate = None;
    let mut bit_depth = None;
    let mut data_bytes = None;

    while offset + 8 <= data.len() {
        let chunk_id = &data[offset..offset + 4];
        let chunk_size = u32::from_le_bytes(
            data[offset + 4..offset + 8]
                .try_into()
                .expect("chunk size slice"),
        ) as usize;
        let chunk_start = offset + 8;
        let chunk_end = chunk_start + chunk_size;
        assert!(
            chunk_end <= data.len(),
            "{} has truncated WAV chunk",
            path.display()
        );

        if chunk_id == b"fmt " {
            assert!(chunk_size >= 16, "{} has short fmt chunk", path.display());
            audio_format = Some(u16::from_le_bytes(
                data[chunk_start..chunk_start + 2]
                    .try_into()
                    .expect("audio format slice"),
            ));
            channels = Some(u16::from_le_bytes(
                data[chunk_start + 2..chunk_start + 4]
                    .try_into()
                    .expect("channel slice"),
            ));
            sample_rate = Some(u32::from_le_bytes(
                data[chunk_start + 4..chunk_start + 8]
                    .try_into()
                    .expect("sample rate slice"),
            ));
            bit_depth = Some(u16::from_le_bytes(
                data[chunk_start + 14..chunk_start + 16]
                    .try_into()
                    .expect("bit depth slice"),
            ));
        } else if chunk_id == b"data" {
            data_bytes = Some(chunk_size as u32);
        }

        offset = chunk_end + (chunk_size % 2);
    }

    WavInfo {
        audio_format: audio_format.expect("WAV fmt chunk missing"),
        channels: channels.expect("WAV channels missing"),
        sample_rate: sample_rate.expect("WAV sample rate missing"),
        bit_depth: bit_depth.expect("WAV bit depth missing"),
        data_bytes: data_bytes.expect("WAV data chunk missing"),
    }
}

fn wav_frame_count(info: &WavInfo) -> u64 {
    let bytes_per_frame = u64::from(info.channels) * u64::from(info.bit_depth / 8);
    u64::from(info.data_bytes) / bytes_per_frame
}

fn wav_duration_ms(info: &WavInfo) -> u64 {
    (wav_frame_count(info) * 1000) / u64::from(info.sample_rate)
}
