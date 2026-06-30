use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crossbeam_channel::Receiver;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::assemblyai::{self, AssemblyAIEvent};
use super::deepgram::{self, DeepgramEvent};
use super::openai_realtime::{self, OpenAiRealtimeEvent};
use crate::events::{DiarizationSpanRevisionPayload, DiarizationSpanStability};

#[derive(Debug, Deserialize)]
struct EventFixture {
    schema_version: u32,
    id: String,
    provider: EventFixtureProvider,
    messages: Vec<EventFixtureMessage>,
    expected_events: Vec<Value>,
    /// Optional speaker/channel diarization normalization assertion. When
    /// present, the replayed provider events are normalized into provider-neutral
    /// [`DiarizationSpanRevisionPayload`] speaker-timeline span revisions and
    /// compared against these — proving provider speaker+channel metadata maps
    /// into the durable speaker timeline (NOT transcript-row mutation). Absent on
    /// fixtures that only assert the serialized event stream.
    #[serde(default)]
    diarization: Option<DiarizationNormalizationSpec>,
    #[serde(default)]
    expected_diarization_revisions: Vec<ExpectedDiarizationRevision>,
}

/// Fixture-side expectation for one normalized diarization span revision.
///
/// A local mirror of [`DiarizationSpanRevisionPayload`] whose every optional
/// field carries `#[serde(default)]` so fixtures stay terse (a span with no
/// label simply omits `speaker_label`). The production payload deliberately does
/// NOT default those fields, so it cannot be deserialized partially — hence this
/// dedicated expectation type, matching the pattern used by `asr/fixtures.rs`.
#[derive(Debug, Deserialize)]
struct ExpectedDiarizationRevision {
    span_id: String,
    provider: String,
    timeline_id: String,
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    speaker_id: Option<String>,
    #[serde(default)]
    speaker_label: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    start_time: f64,
    end_time: f64,
    is_final: bool,
    stability: DiarizationSpanStability,
    revision_number: u64,
    #[serde(default)]
    supersedes: Option<String>,
    #[serde(default)]
    basis_asr_span_ids: Vec<String>,
    #[serde(default)]
    basis_transcript_segment_ids: Vec<String>,
    #[serde(default)]
    raw_event_ref: Option<String>,
}

/// Per-fixture configuration for normalizing provider speaker/channel metadata
/// into [`DiarizationSpanRevisionPayload`] span revisions.
///
/// The normalizer keeps the PROVIDER speaker id strictly separate from any local
/// stable speaker id and the display label: `speaker_id` carries the
/// provider-scoped raw id (e.g. `"deepgram-1"`), `speaker_label` carries the
/// human-facing label resolved from [`Self::speaker_labels`]. The `channel`
/// field is provenance-only and is populated solely when [`Self::channel_capable`]
/// is `true` (a capability gate); otherwise it stays `None` even if a source
/// channel is configured.
#[derive(Debug, Deserialize)]
struct DiarizationNormalizationSpec {
    /// Logical timeline being revised (e.g. `"session"` or a provider source id).
    timeline_id: String,
    /// Capture source, when the attribution is source-local. Provenance-only.
    #[serde(default)]
    source_id: Option<String>,
    /// Source channel label (e.g. `"mixed"`, `"left"`). Provenance-only — emitted
    /// on the revision ONLY when `channel_capable` is `true`.
    #[serde(default)]
    channel: Option<String>,
    /// Capability gate for source/generated channel attribution. When `false`
    /// (the default), the channel field is suppressed even if `channel` is set.
    #[serde(default)]
    channel_capable: bool,
    /// Provider-speaker-id -> display-label map. A provider id with no entry
    /// yields a `None` label (an unknown/interim speaker keeps its raw id but no
    /// friendly label).
    #[serde(default)]
    speaker_labels: HashMap<String, String>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum EventFixtureProvider {
    Assemblyai,
    Deepgram,
    OpenaiRealtime,
}

#[derive(Debug, Deserialize)]
struct EventFixtureMessage {
    raw: String,
    #[serde(default)]
    expected_session_ready: Option<bool>,
}

#[test]
fn deepgram_server_event_fixture_replays_ordered_events() {
    run_fixture("deepgram/server_events.json");
}

#[test]
fn assemblyai_server_event_fixture_replays_ordered_events() {
    run_fixture("assemblyai/server_events.json");
}

#[test]
fn openai_realtime_server_event_fixture_replays_ordered_events() {
    run_fixture("openai_realtime/server_events.json");
}

fn run_fixture(relative_path: &str) {
    let fixture = load_fixture(relative_path);
    assert_eq!(
        fixture.schema_version, 1,
        "{relative_path}: unsupported schema version for {}",
        fixture.id
    );

    let actual_events = match fixture.provider {
        EventFixtureProvider::Assemblyai => replay_assemblyai(&fixture, relative_path),
        EventFixtureProvider::Deepgram => replay_deepgram(&fixture, relative_path),
        EventFixtureProvider::OpenaiRealtime => replay_openai_realtime(&fixture, relative_path),
    };

    assert_eq!(
        actual_events, fixture.expected_events,
        "{relative_path}: serialized event stream"
    );

    assert_diarization_revisions(&fixture, relative_path);
}

/// Normalize the fixture's replayed provider events into provider-neutral
/// diarization span revisions and assert them against
/// `expected_diarization_revisions`.
///
/// Fixtures without a `diarization` spec must NOT declare expected revisions —
/// catching a fixture that forgot to opt into the normalization path.
fn assert_diarization_revisions(fixture: &EventFixture, relative_path: &str) {
    let Some(spec) = &fixture.diarization else {
        assert!(
            fixture.expected_diarization_revisions.is_empty(),
            "{relative_path}: expected_diarization_revisions requires a `diarization` spec"
        );
        return;
    };

    let actual = match fixture.provider {
        EventFixtureProvider::Deepgram => normalize_deepgram_diarization(fixture, spec),
        // Speaker/channel normalization for AssemblyAI v3 + OpenAI Realtime flows
        // through the richer parser fixtures (`asr/fixtures.rs`); the event-stream
        // harness only normalizes the providers that carry word-level speaker ids
        // in their serialized events.
        other => {
            panic!("{relative_path}: diarization normalization is not wired for provider {other:?}")
        }
    };

    assert_eq!(
        actual.len(),
        fixture.expected_diarization_revisions.len(),
        "{relative_path}: normalized diarization revision count"
    );
    for (index, (actual, expected)) in actual
        .iter()
        .zip(fixture.expected_diarization_revisions.iter())
        .enumerate()
    {
        assert_diarization_revision_eq(
            actual,
            expected,
            &format!("{relative_path}: expected_diarization_revisions[{index}]"),
        );
    }
}

/// Normalize Deepgram transcript events into provider-neutral speaker-timeline
/// span revisions.
///
/// Deepgram attaches a per-word `speaker: Option<u32>` index. Each transcript
/// becomes one or more revisions, splitting a transcript whose words switch
/// speaker into a separate span per contiguous same-speaker run (mixed-speaker
/// spans). A word with no speaker index is an unknown/interim speaker: it keeps a
/// `None` provider id and `None` label, with `Provisional` stability.
///
/// Provider speaker id (`deepgram-{n}`) is kept SEPARATE from the display label
/// (resolved from the spec's `speaker_labels`); the channel is provenance-only
/// and suppressed unless the spec's capability gate (`channel_capable`) is set.
/// Re-attributing a span (a later transcript at the same start time switching
/// speaker) emits a retcon revision that `supersedes` the earlier span_id.
fn normalize_deepgram_diarization(
    fixture: &EventFixture,
    spec: &DiarizationNormalizationSpec,
) -> Vec<DiarizationSpanRevisionPayload> {
    // Re-replay to recover the TYPED events (the serialized `Vec<Value>` path
    // above loses the word-level structure we need here).
    let (tx, rx) = crossbeam_channel::unbounded::<DeepgramEvent>();
    for message in &fixture.messages {
        deepgram::handle_server_message(&message.raw, &tx);
    }
    drop(tx);

    let channel = if spec.channel_capable {
        spec.channel.clone()
    } else {
        None
    };

    // span start_time -> the span_id we last emitted for it, so a later
    // re-attribution can supersede the prior revision rather than duplicate it.
    let mut span_history: HashMap<u64, String> = HashMap::new();
    let mut revisions = Vec::new();

    for event in rx.try_iter() {
        let DeepgramEvent::Transcript {
            is_final,
            start,
            duration,
            words,
            ..
        } = event
        else {
            continue;
        };
        if words.is_empty() {
            continue;
        }

        // Group contiguous same-speaker words into runs (mixed-speaker spans).
        let mut runs: Vec<(Option<u32>, f64, f64)> = Vec::new();
        for word in &words {
            match runs.last_mut() {
                Some((spk, _run_start, run_end)) if *spk == word.speaker => {
                    *run_end = word.end;
                }
                _ => runs.push((word.speaker, word.start, word.end)),
            }
        }

        for (run_index, (speaker, run_start, run_end)) in runs.into_iter().enumerate() {
            // Quantize the start to whole milliseconds for a stable span key
            // independent of float jitter across re-attributions.
            let start_key = (run_start * 1000.0).round() as u64;
            let provider_speaker_id = speaker.map(|n| format!("deepgram-{n}"));
            let speaker_label = provider_speaker_id
                .as_deref()
                .and_then(|id| spec.speaker_labels.get(id).cloned());

            let span_id = format!(
                "deepgram:{}:{}:{}",
                spec.timeline_id,
                start_key,
                provider_speaker_id.as_deref().unwrap_or("unknown")
            );

            // A retcon supersedes the prior revision recorded for this start.
            let supersedes = span_history.get(&start_key).cloned();
            let revision_number = if supersedes.is_some() { 2 } else { 1 };
            span_history.insert(start_key, span_id.clone());

            let stability = if is_final {
                DiarizationSpanStability::Stable
            } else {
                DiarizationSpanStability::Provisional
            };

            revisions.push(DiarizationSpanRevisionPayload {
                span_id,
                provider: "deepgram".to_string(),
                timeline_id: spec.timeline_id.clone(),
                source_id: spec.source_id.clone(),
                speaker_id: provider_speaker_id,
                speaker_label,
                channel: channel.clone(),
                start_time: run_start,
                end_time: run_end,
                confidence: None,
                is_final,
                stability,
                revision_number,
                supersedes,
                basis_asr_span_ids: vec![format!("deepgram:{}:{}", spec.timeline_id, start_key)],
                basis_transcript_segment_ids: Vec::new(),
                raw_event_ref: Some(format!("transcript:{start}:{duration}:{run_index}")),
                capture_latency_ms: None,
                asr_latency_ms: None,
                received_at_ms: 0,
            });
        }
    }

    revisions
}

fn load_fixture(relative_path: &str) -> EventFixture {
    let path = fixture_path(relative_path);
    let body = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "failed to read ASR event fixture {}: {error}",
            path.display()
        )
    });
    serde_json::from_str(&body).unwrap_or_else(|error| {
        panic!(
            "failed to parse ASR event fixture {}: {error}",
            path.display()
        )
    })
}

fn fixture_path(relative_path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("asr")
        .join(relative_path)
}

fn replay_assemblyai(fixture: &EventFixture, relative_path: &str) -> Vec<Value> {
    let (tx, rx) = crossbeam_channel::unbounded::<AssemblyAIEvent>();
    let mut events = Vec::new();
    for (index, message) in fixture.messages.iter().enumerate() {
        assert_no_session_ready_expectation(relative_path, index, message);
        assemblyai::handle_server_message(&message.raw, &tx);
        events.extend(drain_serialized_events(&rx, relative_path));
    }
    events
}

fn replay_deepgram(fixture: &EventFixture, relative_path: &str) -> Vec<Value> {
    let (tx, rx) = crossbeam_channel::unbounded::<DeepgramEvent>();
    let mut events = Vec::new();
    for (index, message) in fixture.messages.iter().enumerate() {
        assert_no_session_ready_expectation(relative_path, index, message);
        deepgram::handle_server_message(&message.raw, &tx);
        events.extend(drain_serialized_events(&rx, relative_path));
    }
    events
}

fn replay_openai_realtime(fixture: &EventFixture, relative_path: &str) -> Vec<Value> {
    let (tx, rx) = crossbeam_channel::unbounded::<OpenAiRealtimeEvent>();
    let mut accumulator = HashMap::new();
    let mut events = Vec::new();
    for (index, message) in fixture.messages.iter().enumerate() {
        let session_ready =
            openai_realtime::handle_server_message(&message.raw, &tx, &mut accumulator);
        if let Some(expected) = message.expected_session_ready {
            assert_eq!(
                session_ready, expected,
                "{relative_path}: message {index} OpenAI session readiness"
            );
        }
        events.extend(drain_serialized_events(&rx, relative_path));
    }
    events
}

fn assert_no_session_ready_expectation(
    relative_path: &str,
    index: usize,
    message: &EventFixtureMessage,
) {
    assert!(
        message.expected_session_ready.is_none(),
        "{relative_path}: message {index} session readiness is only valid for OpenAI Realtime fixtures"
    );
}

fn drain_serialized_events<T>(rx: &Receiver<T>, relative_path: &str) -> Vec<Value>
where
    T: Serialize,
{
    rx.try_iter()
        .map(|event| {
            serde_json::to_value(event).unwrap_or_else(|error| {
                panic!("{relative_path}: failed to serialize event: {error}")
            })
        })
        .collect()
}

fn assert_diarization_revision_eq(
    actual: &DiarizationSpanRevisionPayload,
    expected: &ExpectedDiarizationRevision,
    context: &str,
) {
    assert_eq!(actual.span_id, expected.span_id, "{context}: span_id");
    assert_eq!(actual.provider, expected.provider, "{context}: provider");
    assert_eq!(
        actual.timeline_id, expected.timeline_id,
        "{context}: timeline_id"
    );
    assert_eq!(actual.source_id, expected.source_id, "{context}: source_id");
    assert_eq!(
        actual.speaker_id, expected.speaker_id,
        "{context}: speaker_id (provider-scoped id, separate from local stable id)"
    );
    assert_eq!(
        actual.speaker_label, expected.speaker_label,
        "{context}: speaker_label"
    );
    assert_eq!(
        actual.channel, expected.channel,
        "{context}: channel (provenance-only; gated by channel_capable)"
    );
    assert_close_f64(
        actual.start_time,
        expected.start_time,
        context,
        "start_time",
    );
    assert_close_f64(actual.end_time, expected.end_time, context, "end_time");
    assert_eq!(actual.is_final, expected.is_final, "{context}: is_final");
    assert_eq!(actual.stability, expected.stability, "{context}: stability");
    assert_eq!(
        actual.revision_number, expected.revision_number,
        "{context}: revision_number"
    );
    assert_eq!(
        actual.supersedes, expected.supersedes,
        "{context}: supersedes (retcon link)"
    );
    assert_eq!(
        actual.basis_asr_span_ids, expected.basis_asr_span_ids,
        "{context}: basis_asr_span_ids"
    );
    assert_eq!(
        actual.basis_transcript_segment_ids, expected.basis_transcript_segment_ids,
        "{context}: basis_transcript_segment_ids"
    );
    assert_eq!(
        actual.raw_event_ref, expected.raw_event_ref,
        "{context}: raw_event_ref"
    );
}

fn assert_close_f64(actual: f64, expected: f64, context: &str, field: &str) {
    let delta = (actual - expected).abs();
    assert!(
        delta <= 0.000_001,
        "{context}: {field}: expected {expected}, got {actual}, delta {delta}"
    );
}

/// Seed 20f2: provider speaker/channel diarization normalizes into
/// provider-neutral speaker-timeline span revisions (NOT transcript-row
/// mutation). Covers provider speaker ids, display labels, the channel
/// provenance gate, mixed-speaker spans, unknown/interim speakers, and retcons.
#[test]
fn deepgram_diarization_revision_fixture_normalizes_speaker_and_channel() {
    run_fixture("deepgram/diarization_revisions.json");
}
