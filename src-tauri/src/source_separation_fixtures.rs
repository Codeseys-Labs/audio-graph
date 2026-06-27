use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct SourceSeparationFixtureManifest {
    schema_version: u32,
    id: String,
    format: AudioFormat,
    source_dataset: SourceDataset,
    speakers: Vec<Speaker>,
    source_components: Vec<SourceComponent>,
    fixtures: Vec<Fixture>,
    candidate_quality_thresholds: CandidateQualityThresholds,
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
struct SourceDataset {
    license: String,
    license_url: String,
    homepage_url: String,
    dataset_card_url: String,
    derived_from_url: String,
}

#[derive(Debug, Deserialize)]
struct Speaker {
    id: String,
    source_speaker_id: String,
}

#[derive(Debug, Deserialize)]
struct SourceComponent {
    id: String,
    speaker_id: String,
    audio_path: String,
    duration_ms: u64,
    text: String,
}

#[derive(Debug, Deserialize)]
struct Fixture {
    id: String,
    kind: FixtureKind,
    audio_path: String,
    duration_ms: u64,
    expected_speaker_count: u32,
    segments: Vec<Segment>,
    baseline: Baseline,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FixtureKind {
    Overlap,
    TurnTaking,
}

#[derive(Debug, Deserialize)]
struct Segment {
    speaker_id: String,
    source_component_id: String,
    start_ms: u64,
    end_ms: u64,
    text: String,
}

#[derive(Debug, Deserialize)]
struct Baseline {
    mono_asr: BaselineStatus,
    diarization: DiarizationBaselineStatus,
}

#[derive(Debug, Deserialize)]
struct BaselineStatus {
    status: String,
    reference_transcript: String,
    required_before_close: bool,
}

#[derive(Debug, Deserialize)]
struct DiarizationBaselineStatus {
    status: String,
    expected_speaker_count: u32,
    expected_overlap_regions: Vec<ExpectedOverlapRegion>,
    required_before_close: bool,
}

#[derive(Debug, Deserialize)]
struct ExpectedOverlapRegion {
    start_ms: u64,
    end_ms: u64,
    speaker_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CandidateQualityThresholds {
    overlap_region_wer_must_improve_over_mono: bool,
    artifact_flag_requires_mono_fallback: bool,
    source_native_channel_claim_allowed: bool,
    generated_speaker_lane_selectable_without_baseline: bool,
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
fn source_separation_manifest_has_required_fixture_shapes() {
    let manifest = load_manifest();

    assert_eq!(manifest.schema_version, 1);
    assert_eq!(manifest.id, "librispeech-two-speaker-mini-v1");
    assert_eq!(manifest.format.container, "wav");
    assert_eq!(manifest.format.encoding, "pcm_s16le");
    assert_eq!(manifest.format.sample_rate, 16_000);
    assert_eq!(manifest.format.channels, 1);
    assert_eq!(manifest.format.bit_depth, 16);

    assert_eq!(manifest.source_dataset.license, "CC BY 4.0");
    assert!(
        manifest
            .source_dataset
            .license_url
            .starts_with("https://creativecommons.org/licenses/by/4.0/")
    );
    assert!(manifest.source_dataset.homepage_url.contains("openslr.org"));
    assert!(
        manifest
            .source_dataset
            .dataset_card_url
            .contains("librispeech_asr")
    );
    assert!(
        manifest
            .source_dataset
            .derived_from_url
            .contains("librivox")
    );

    let speaker_ids = manifest
        .speakers
        .iter()
        .map(|speaker| speaker.id.as_str())
        .collect::<HashSet<_>>();
    assert!(speaker_ids.contains("speaker_a"));
    assert!(speaker_ids.contains("speaker_b"));
    assert_eq!(speaker_ids.len(), 2);
    assert!(
        manifest
            .speakers
            .iter()
            .all(|speaker| !speaker.source_speaker_id.is_empty())
    );

    let component_ids = manifest
        .source_components
        .iter()
        .map(|component| component.id.as_str())
        .collect::<HashSet<_>>();
    assert!(component_ids.contains("source_a"));
    assert!(component_ids.contains("source_b"));
    for component in &manifest.source_components {
        assert!(
            speaker_ids.contains(component.speaker_id.as_str()),
            "{} references unknown speaker {}",
            component.id,
            component.speaker_id
        );
        assert!(!component.text.trim().is_empty());
        assert_eq!(component.duration_ms, 3000);
    }

    let overlap = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.kind == FixtureKind::Overlap)
        .expect("fixture set must include an overlap clip");
    assert!(
        has_cross_speaker_overlap(overlap),
        "overlap fixture must include an overlapping region"
    );

    let turn_taking = manifest
        .fixtures
        .iter()
        .find(|fixture| fixture.kind == FixtureKind::TurnTaking)
        .expect("fixture set must include a turn-taking clip");
    assert!(
        !has_cross_speaker_overlap(turn_taking),
        "turn-taking fixture should not overlap speakers"
    );

    for fixture in &manifest.fixtures {
        assert!(!fixture.id.trim().is_empty());
        assert_eq!(fixture.expected_speaker_count, 2);
        assert_eq!(fixture.baseline.mono_asr.status, "pending_real_run");
        assert_eq!(fixture.baseline.diarization.status, "pending_real_run");
        assert!(fixture.baseline.mono_asr.required_before_close);
        assert!(fixture.baseline.diarization.required_before_close);
        assert!(!fixture.baseline.mono_asr.reference_transcript.is_empty());
        assert_eq!(
            fixture.baseline.diarization.expected_speaker_count,
            fixture.expected_speaker_count
        );
        for region in &fixture.baseline.diarization.expected_overlap_regions {
            assert!(region.start_ms < region.end_ms);
            assert_eq!(region.speaker_ids.len(), 2);
        }
        for segment in &fixture.segments {
            assert!(speaker_ids.contains(segment.speaker_id.as_str()));
            assert!(component_ids.contains(segment.source_component_id.as_str()));
            assert!(segment.start_ms < segment.end_ms);
            assert!(segment.end_ms <= fixture.duration_ms);
            assert!(!segment.text.trim().is_empty());
        }
    }

    assert!(
        manifest
            .candidate_quality_thresholds
            .overlap_region_wer_must_improve_over_mono
    );
    assert!(
        manifest
            .candidate_quality_thresholds
            .artifact_flag_requires_mono_fallback
    );
    assert!(
        !manifest
            .candidate_quality_thresholds
            .source_native_channel_claim_allowed
    );
    assert!(
        !manifest
            .candidate_quality_thresholds
            .generated_speaker_lane_selectable_without_baseline
    );
}

#[test]
fn source_separation_wavs_match_manifest_format() {
    let manifest = load_manifest();
    let mut audio_paths = manifest
        .source_components
        .iter()
        .map(|component| (component.audio_path.as_str(), component.duration_ms))
        .collect::<Vec<_>>();
    audio_paths.extend(
        manifest
            .fixtures
            .iter()
            .map(|fixture| (fixture.audio_path.as_str(), fixture.duration_ms)),
    );

    for (relative_path, expected_duration_ms) in audio_paths {
        let path = fixture_root().join(relative_path);
        let info = parse_wav_info(&path);
        assert_eq!(info.audio_format, 1, "{} must be PCM", path.display());
        assert_eq!(
            info.sample_rate,
            manifest.format.sample_rate,
            "{} sample rate",
            path.display()
        );
        assert_eq!(
            info.channels,
            manifest.format.channels,
            "{} channels",
            path.display()
        );
        assert_eq!(
            info.bit_depth,
            manifest.format.bit_depth,
            "{} bit depth",
            path.display()
        );
        assert_eq!(
            wav_duration_ms(&info),
            expected_duration_ms,
            "{} duration",
            path.display()
        );
    }
}

fn load_manifest() -> SourceSeparationFixtureManifest {
    let path = fixture_root().join("manifest.json");
    let body = fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_json::from_str(&body)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn fixture_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("source_separation")
}

fn has_cross_speaker_overlap(fixture: &Fixture) -> bool {
    fixture.segments.iter().enumerate().any(|(index, left)| {
        fixture.segments.iter().skip(index + 1).any(|right| {
            left.speaker_id != right.speaker_id
                && left.start_ms < right.end_ms
                && right.start_ms < left.end_ms
        })
    })
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

fn wav_duration_ms(info: &WavInfo) -> u64 {
    let bytes_per_frame = u64::from(info.channels) * u64::from(info.bit_depth / 8);
    let frame_count = u64::from(info.data_bytes) / bytes_per_frame;
    (frame_count * 1000) / u64::from(info.sample_rate)
}
