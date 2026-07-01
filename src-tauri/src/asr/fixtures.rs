use std::fmt::Debug;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::events::{AsrSpanRevisionPayload, AsrSpanStability};
use crate::projections::{TranscriptEvent, TranscriptLedger, TranscriptLedgerError};

use super::assemblyai::{AssemblyAiV3ParseError, AssemblyAiV3ParsedMessage, AssemblyAiV3Parser};
use super::gladia::{
    GladiaLiveParser, GladiaParseError, GladiaParsedMessage, GladiaSpeechEventType,
};
use super::revai::{RevAiParseError, RevAiParsedMessage, RevAiStreamingParser};
use super::soniox::{SonioxParseError, SonioxParsedMessage, SonioxRealtimeParser};
use super::speechmatics::{
    SpeechmaticsParseError, SpeechmaticsParsedMessage, SpeechmaticsRealtimeParser,
};

#[derive(Debug, Deserialize)]
struct AsrParserFixture {
    schema_version: u32,
    id: String,
    provider: FixtureProvider,
    source_id: String,
    session_id: String,
    messages: Vec<FixtureMessage>,
    expected_revisions: Vec<AsrSpanRevisionPayload>,
    expected_ledger: ExpectedLedger,
    #[serde(default)]
    stale_replay: Option<ExpectedStaleReplay>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FixtureProvider {
    AssemblyaiV3,
    Gladia,
    Revai,
    Soniox,
    Speechmatics,
}

#[derive(Debug, Deserialize)]
struct FixtureMessage {
    received_at_ms: u64,
    raw: String,
    #[serde(default)]
    expected_revision_count: Option<usize>,
    #[serde(default)]
    expected_parse_error: Option<ExpectedParseError>,
    #[serde(default)]
    expected_connected_id: Option<String>,
    #[serde(default)]
    expected_finished: Option<bool>,
    #[serde(default)]
    expected_provider_error: Option<ExpectedProviderError>,
    #[serde(default)]
    expected_session_id: Option<String>,
    #[serde(default)]
    expected_speech_event: Option<ExpectedGladiaSpeechEvent>,
    #[serde(default)]
    expected_acknowledgment: Option<ExpectedGladiaAcknowledgment>,
    #[serde(default)]
    expected_lifecycle_event: Option<String>,
    #[serde(default)]
    expected_recognition_id: Option<String>,
    #[serde(default)]
    expected_end_of_utterance: Option<ExpectedEndOfUtterance>,
    #[serde(default)]
    expected_speaker_revisions: Vec<ExpectedSpeakerRevision>,
    #[serde(default)]
    expected_soniox_revision_sidebands: Vec<ExpectedSonioxRevisionSideband>,
}

#[derive(Debug, Deserialize)]
struct ExpectedParseError {
    kind: ExpectedParseErrorKind,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExpectedParseErrorKind {
    InvalidJson,
    UnsupportedMessageType,
}

#[derive(Debug, Deserialize)]
struct ExpectedProviderError {
    #[serde(default)]
    message_type: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    error_type: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    more_info: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    seq_no: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ExpectedGladiaSpeechEvent {
    event_type: String,
    time: f64,
    #[serde(default)]
    channel: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedGladiaAcknowledgment {
    message_type: String,
    acknowledged: bool,
    #[serde(default)]
    byte_range: Option<[u64; 2]>,
    #[serde(default)]
    time_range: Option<[f64; 2]>,
}

#[derive(Debug, Deserialize)]
struct ExpectedEndOfUtterance {
    start_time: f64,
    end_time: f64,
    #[serde(default)]
    channel: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedSpeakerRevision {
    turn_order: u64,
    span_id: String,
    provider_item_id: String,
    #[serde(default)]
    speaker_id: Option<String>,
    #[serde(default)]
    speaker_label: Option<String>,
    #[serde(default)]
    words: Vec<ExpectedSpeakerRevisionWord>,
}

#[derive(Debug, Deserialize)]
struct ExpectedSpeakerRevisionWord {
    text: String,
    #[serde(default)]
    speaker_id: Option<String>,
    #[serde(default)]
    start_time: Option<f64>,
    #[serde(default)]
    end_time: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct ExpectedSonioxRevisionSideband {
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    source_language: Option<String>,
    #[serde(default)]
    final_audio_proc_ms: Option<u64>,
    #[serde(default)]
    total_audio_proc_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ExpectedLedger {
    accepted_event_count: u64,
    latest_spans: Vec<ExpectedLedgerSpan>,
}

#[derive(Debug, Deserialize)]
struct ExpectedLedgerSpan {
    span_id: String,
    revision_number: u64,
    is_final: bool,
    text: String,
}

#[derive(Debug, Deserialize)]
struct ExpectedStaleReplay {
    event_index: usize,
    span_id: String,
    current_revision: u64,
    incoming_revision: u64,
}

#[test]
fn revai_partial_final_revision_fixture_replays_through_ledger() {
    run_fixture("revai/partial_final_revision.json");
}

#[test]
fn soniox_partial_final_revision_fixture_replays_through_ledger() {
    run_fixture("soniox/partial_final_revision.json");
}

#[test]
fn soniox_diarization_normalization_fixture_replays_through_ledger() {
    run_fixture("soniox/diarization_normalization.json");
}

#[test]
fn gladia_partial_final_revision_fixture_replays_through_ledger() {
    run_fixture("gladia/partial_final_revision.json");
}

#[test]
fn speechmatics_partial_final_revision_fixture_replays_through_ledger() {
    run_fixture("speechmatics/partial_final_revision.json");
}

#[test]
fn revai_sideband_and_error_fixture_replays_through_ledger() {
    run_fixture("revai/sideband_and_error.json");
}

#[test]
fn soniox_sideband_and_error_fixture_replays_through_ledger() {
    run_fixture("soniox/sideband_and_error.json");
}

#[test]
fn gladia_sideband_and_error_fixture_replays_through_ledger() {
    run_fixture("gladia/sideband_and_error.json");
}

#[test]
fn speechmatics_sideband_and_error_fixture_replays_through_ledger() {
    run_fixture("speechmatics/sideband_and_error.json");
}

#[test]
fn assemblyai_v3_partial_final_revision_fixture_replays_through_ledger() {
    run_fixture("assemblyai/v3_partial_final_revision.json");
}

#[test]
fn assemblyai_v3_speaker_revision_sideband_fixture_does_not_mutate_ledger() {
    run_fixture("assemblyai/v3_speaker_revision_sideband.json");
}

fn run_fixture(relative_path: &str) {
    let fixture = load_fixture(relative_path);
    assert_eq!(
        fixture.schema_version, 1,
        "{relative_path}: unsupported schema version for {}",
        fixture.id
    );

    let actual_revisions = parse_fixture_messages(&fixture, relative_path);

    assert_eq!(
        actual_revisions.len(),
        fixture.expected_revisions.len(),
        "{relative_path}: normalized revision count"
    );
    for (index, (actual, expected)) in actual_revisions
        .iter()
        .zip(fixture.expected_revisions.iter())
        .enumerate()
    {
        assert_asr_revision_eq(
            actual,
            expected,
            &format!("{relative_path}: expected_revisions[{index}]"),
        );
    }

    let events = actual_revisions
        .iter()
        .cloned()
        .map(TranscriptEvent::from)
        .collect::<Vec<_>>();
    let mut ledger = TranscriptLedger::replay(&fixture.session_id, events.clone())
        .unwrap_or_else(|error| panic!("{relative_path}: ledger replay failed: {error:?}"));

    assert_eq!(
        ledger.accepted_event_count, fixture.expected_ledger.accepted_event_count,
        "{relative_path}: accepted ledger event count"
    );
    assert_eq!(
        ledger.latest_spans.len(),
        fixture.expected_ledger.latest_spans.len(),
        "{relative_path}: latest ledger span count"
    );
    for expected in &fixture.expected_ledger.latest_spans {
        let actual = ledger
            .latest_spans
            .iter()
            .find(|span| span.span_id == expected.span_id)
            .unwrap_or_else(|| panic!("{relative_path}: missing ledger span {}", expected.span_id));
        assert_eq!(
            actual.revision_number, expected.revision_number,
            "{relative_path}: ledger revision for {}",
            expected.span_id
        );
        assert_eq!(
            actual.is_final, expected.is_final,
            "{relative_path}: ledger finality for {}",
            expected.span_id
        );
        assert_eq!(
            actual.text, expected.text,
            "{relative_path}: ledger text for {}",
            expected.span_id
        );
    }

    if let Some(expected_stale) = &fixture.stale_replay {
        let event = events
            .get(expected_stale.event_index)
            .unwrap_or_else(|| {
                panic!(
                    "{relative_path}: stale replay index {} is out of range",
                    expected_stale.event_index
                )
            })
            .clone();
        let error = ledger
            .apply_event(event)
            .expect_err("stale replay must be rejected");
        assert_eq!(
            error,
            TranscriptLedgerError::StaleTranscriptRevision {
                span_id: expected_stale.span_id.clone(),
                current_revision: expected_stale.current_revision,
                incoming_revision: expected_stale.incoming_revision,
            },
            "{relative_path}: stale replay rejection"
        );
    }
}

fn load_fixture(relative_path: &str) -> AsrParserFixture {
    let path = fixture_path(relative_path);
    let body = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "failed to read ASR parser fixture {}: {error}",
            path.display()
        )
    });
    serde_json::from_str(&body).unwrap_or_else(|error| {
        panic!(
            "failed to parse ASR parser fixture {}: {error}",
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

fn parse_fixture_messages(
    fixture: &AsrParserFixture,
    relative_path: &str,
) -> Vec<AsrSpanRevisionPayload> {
    match fixture.provider {
        FixtureProvider::AssemblyaiV3 => {
            let mut parser = AssemblyAiV3Parser::new(fixture.source_id.as_str());
            let mut revisions = Vec::new();
            for (index, message) in fixture.messages.iter().enumerate() {
                let parsed = match parser.parse_message(&message.raw, message.received_at_ms) {
                    Ok(parsed) => {
                        assert_unexpected_parse_error_expectation(message, relative_path, index);
                        parsed
                    }
                    Err(error) => {
                        assert_assemblyai_v3_parse_error(error, message, relative_path, index);
                        continue;
                    }
                };
                assert_expected_revision_count(
                    parsed.revisions.len(),
                    message,
                    relative_path,
                    index,
                );
                assert_assemblyai_v3_sideband(&parsed, message, relative_path, index);
                revisions.extend(
                    parsed
                        .revisions
                        .into_iter()
                        .map(|revision| revision.payload),
                );
            }
            revisions
        }
        FixtureProvider::Gladia => {
            let mut parser = GladiaLiveParser::new(fixture.source_id.as_str());
            let mut revisions = Vec::new();
            for (index, message) in fixture.messages.iter().enumerate() {
                let parsed = match parser.parse_message(&message.raw, message.received_at_ms) {
                    Ok(parsed) => {
                        assert_unexpected_parse_error_expectation(message, relative_path, index);
                        parsed
                    }
                    Err(error) => {
                        assert_gladia_parse_error(error, message, relative_path, index);
                        continue;
                    }
                };
                assert_expected_revision_count(
                    parsed.revisions.len(),
                    message,
                    relative_path,
                    index,
                );
                assert_gladia_sideband(&parsed, message, relative_path, index);
                revisions.extend(
                    parsed
                        .revisions
                        .into_iter()
                        .map(|revision| revision.payload),
                );
            }
            revisions
        }
        FixtureProvider::Revai => {
            let mut parser = RevAiStreamingParser::new(fixture.source_id.as_str());
            let mut revisions = Vec::new();
            for (index, message) in fixture.messages.iter().enumerate() {
                let parsed = match parser.parse_message(&message.raw, message.received_at_ms) {
                    Ok(parsed) => {
                        assert_unexpected_parse_error_expectation(message, relative_path, index);
                        parsed
                    }
                    Err(error) => {
                        assert_revai_parse_error(error, message, relative_path, index);
                        continue;
                    }
                };
                assert_expected_revision_count(
                    parsed.revisions.len(),
                    message,
                    relative_path,
                    index,
                );
                assert_revai_sideband(&parsed, message, relative_path, index);
                revisions.extend(
                    parsed
                        .revisions
                        .into_iter()
                        .map(|revision| revision.payload),
                );
            }
            revisions
        }
        FixtureProvider::Soniox => {
            let mut parser = SonioxRealtimeParser::new(fixture.source_id.as_str());
            let mut revisions = Vec::new();
            for (index, message) in fixture.messages.iter().enumerate() {
                let parsed = match parser.parse_message(&message.raw, message.received_at_ms) {
                    Ok(parsed) => {
                        assert_unexpected_parse_error_expectation(message, relative_path, index);
                        parsed
                    }
                    Err(error) => {
                        assert_soniox_parse_error(error, message, relative_path, index);
                        continue;
                    }
                };
                assert_expected_revision_count(
                    parsed.revisions.len(),
                    message,
                    relative_path,
                    index,
                );
                assert_soniox_sideband(&parsed, message, relative_path, index);
                revisions.extend(
                    parsed
                        .revisions
                        .into_iter()
                        .map(|revision| revision.payload),
                );
            }
            revisions
        }
        FixtureProvider::Speechmatics => {
            let mut parser = SpeechmaticsRealtimeParser::new(fixture.source_id.as_str());
            let mut revisions = Vec::new();
            for (index, message) in fixture.messages.iter().enumerate() {
                let parsed = match parser.parse_message(&message.raw, message.received_at_ms) {
                    Ok(parsed) => {
                        assert_unexpected_parse_error_expectation(message, relative_path, index);
                        parsed
                    }
                    Err(error) => {
                        assert_speechmatics_parse_error(error, message, relative_path, index);
                        continue;
                    }
                };
                assert_expected_revision_count(
                    parsed.revisions.len(),
                    message,
                    relative_path,
                    index,
                );
                assert_speechmatics_sideband(&parsed, message, relative_path, index);
                revisions.extend(
                    parsed
                        .revisions
                        .into_iter()
                        .map(|revision| revision.payload),
                );
            }
            revisions
        }
    }
}

fn assert_expected_revision_count(
    actual: usize,
    message: &FixtureMessage,
    relative_path: &str,
    index: usize,
) {
    if let Some(expected) = message.expected_revision_count {
        assert_eq!(
            actual, expected,
            "{relative_path}: message {index} normalized revision count"
        );
    }
}

fn assert_unexpected_parse_error_expectation(
    message: &FixtureMessage,
    relative_path: &str,
    index: usize,
) {
    assert!(
        message.expected_parse_error.is_none(),
        "{relative_path}: message {index} parsed successfully but expected parse error {:?}",
        message.expected_parse_error
    );
}

fn expected_parse_error<'a>(
    message: &'a FixtureMessage,
    relative_path: &str,
    index: usize,
    error: &impl Debug,
) -> &'a ExpectedParseError {
    let Some(expected) = &message.expected_parse_error else {
        panic!("{relative_path}: parser rejected message {index}: {error:?}");
    };
    expected
}

fn assert_revai_parse_error(
    error: RevAiParseError,
    message: &FixtureMessage,
    relative_path: &str,
    index: usize,
) {
    let expected = expected_parse_error(message, relative_path, index, &error);
    match (&expected.kind, error) {
        (ExpectedParseErrorKind::InvalidJson, RevAiParseError::InvalidJson(_)) => {}
        (
            ExpectedParseErrorKind::UnsupportedMessageType,
            RevAiParseError::UnsupportedMessageType(actual),
        ) => assert_expected_parse_error_value(&actual, expected, relative_path, index),
        (kind, actual) => {
            panic!("{relative_path}: message {index} expected parse error {kind:?}, got {actual:?}")
        }
    }
}

fn assert_assemblyai_v3_parse_error(
    error: AssemblyAiV3ParseError,
    message: &FixtureMessage,
    relative_path: &str,
    index: usize,
) {
    let expected = expected_parse_error(message, relative_path, index, &error);
    match (&expected.kind, error) {
        (ExpectedParseErrorKind::InvalidJson, AssemblyAiV3ParseError::InvalidJson(_)) => {}
        (
            ExpectedParseErrorKind::UnsupportedMessageType,
            AssemblyAiV3ParseError::UnsupportedMessageType(actual),
        ) => assert_expected_parse_error_value(&actual, expected, relative_path, index),
        (kind, actual) => {
            panic!("{relative_path}: message {index} expected parse error {kind:?}, got {actual:?}")
        }
    }
}

fn assert_soniox_parse_error(
    error: SonioxParseError,
    message: &FixtureMessage,
    relative_path: &str,
    index: usize,
) {
    let expected = expected_parse_error(message, relative_path, index, &error);
    match (&expected.kind, error) {
        (ExpectedParseErrorKind::InvalidJson, SonioxParseError::InvalidJson(_)) => {}
        (kind, actual) => {
            panic!("{relative_path}: message {index} expected parse error {kind:?}, got {actual:?}")
        }
    }
}

fn assert_gladia_parse_error(
    error: GladiaParseError,
    message: &FixtureMessage,
    relative_path: &str,
    index: usize,
) {
    let expected = expected_parse_error(message, relative_path, index, &error);
    match (&expected.kind, error) {
        (ExpectedParseErrorKind::InvalidJson, GladiaParseError::InvalidJson(_)) => {}
        (
            ExpectedParseErrorKind::UnsupportedMessageType,
            GladiaParseError::UnsupportedMessageType(actual),
        ) => assert_expected_parse_error_value(&actual, expected, relative_path, index),
        (kind, actual) => {
            panic!("{relative_path}: message {index} expected parse error {kind:?}, got {actual:?}")
        }
    }
}

fn assert_speechmatics_parse_error(
    error: SpeechmaticsParseError,
    message: &FixtureMessage,
    relative_path: &str,
    index: usize,
) {
    let expected = expected_parse_error(message, relative_path, index, &error);
    match (&expected.kind, error) {
        (ExpectedParseErrorKind::InvalidJson, SpeechmaticsParseError::InvalidJson(_)) => {}
        (
            ExpectedParseErrorKind::UnsupportedMessageType,
            SpeechmaticsParseError::UnsupportedMessageType(actual),
        ) => assert_expected_parse_error_value(&actual, expected, relative_path, index),
        (kind, actual) => {
            panic!("{relative_path}: message {index} expected parse error {kind:?}, got {actual:?}")
        }
    }
}

fn assert_expected_parse_error_value(
    actual: &str,
    expected: &ExpectedParseError,
    relative_path: &str,
    index: usize,
) {
    if let Some(expected_value) = expected.value.as_deref() {
        assert_eq!(
            actual, expected_value,
            "{relative_path}: message {index} parse error value"
        );
    }
}

fn assert_revai_sideband(
    parsed: &RevAiParsedMessage,
    message: &FixtureMessage,
    relative_path: &str,
    index: usize,
) {
    if let Some(expected) = message.expected_connected_id.as_deref() {
        assert_eq!(
            parsed.connected_id.as_deref(),
            Some(expected),
            "{relative_path}: message {index} RevAI connected id"
        );
    }
}

fn assert_assemblyai_v3_sideband(
    parsed: &AssemblyAiV3ParsedMessage,
    message: &FixtureMessage,
    relative_path: &str,
    index: usize,
) {
    if let Some(expected) = message.expected_session_id.as_deref() {
        assert_eq!(
            parsed.session_id.as_deref(),
            Some(expected),
            "{relative_path}: message {index} AssemblyAI v3 session id"
        );
    }
    if let Some(expected) = message.expected_finished {
        assert_eq!(
            parsed.terminated, expected,
            "{relative_path}: message {index} AssemblyAI v3 termination flag"
        );
    }
    if let Some(expected) = &message.expected_provider_error {
        let actual = parsed.error.as_ref().unwrap_or_else(|| {
            panic!("{relative_path}: message {index} missing AssemblyAI v3 provider error")
        });
        assert_eq!(
            Some(actual.message.as_str()),
            expected.message.as_deref(),
            "{relative_path}: message {index} AssemblyAI v3 error message"
        );
    }
    assert_eq!(
        parsed.speaker_revisions.len(),
        message.expected_speaker_revisions.len(),
        "{relative_path}: message {index} AssemblyAI v3 speaker revision count"
    );
    for (revision_index, (actual, expected)) in parsed
        .speaker_revisions
        .iter()
        .zip(message.expected_speaker_revisions.iter())
        .enumerate()
    {
        assert_eq!(
            actual.turn_order, expected.turn_order,
            "{relative_path}: message {index} speaker revision {revision_index} turn_order"
        );
        assert_eq!(
            actual.span_id, expected.span_id,
            "{relative_path}: message {index} speaker revision {revision_index} span_id"
        );
        assert_eq!(
            actual.provider_item_id, expected.provider_item_id,
            "{relative_path}: message {index} speaker revision {revision_index} provider_item_id"
        );
        assert_eq!(
            actual.speaker_id, expected.speaker_id,
            "{relative_path}: message {index} speaker revision {revision_index} speaker_id"
        );
        assert_eq!(
            actual.speaker_label, expected.speaker_label,
            "{relative_path}: message {index} speaker revision {revision_index} speaker_label"
        );
        assert_eq!(
            actual.words.len(),
            expected.words.len(),
            "{relative_path}: message {index} speaker revision {revision_index} word count"
        );
        for (word_index, (actual_word, expected_word)) in
            actual.words.iter().zip(expected.words.iter()).enumerate()
        {
            assert_eq!(
                actual_word.text, expected_word.text,
                "{relative_path}: message {index} speaker revision {revision_index} word {word_index} text"
            );
            assert_eq!(
                actual_word.speaker_id, expected_word.speaker_id,
                "{relative_path}: message {index} speaker revision {revision_index} word {word_index} speaker"
            );
            assert_optional_f64(
                actual_word.start_time,
                expected_word.start_time,
                &format!("{relative_path}: message {index}"),
                &format!("speaker revision {revision_index} word {word_index} start_time"),
            );
            assert_optional_f64(
                actual_word.end_time,
                expected_word.end_time,
                &format!("{relative_path}: message {index}"),
                &format!("speaker revision {revision_index} word {word_index} end_time"),
            );
        }
    }
}

fn assert_soniox_sideband(
    parsed: &SonioxParsedMessage,
    message: &FixtureMessage,
    relative_path: &str,
    index: usize,
) {
    if let Some(expected) = message.expected_finished {
        assert_eq!(
            parsed.finished, expected,
            "{relative_path}: message {index} Soniox finished flag"
        );
    }
    if let Some(expected) = &message.expected_provider_error {
        let actual = parsed.error.as_ref().unwrap_or_else(|| {
            panic!("{relative_path}: message {index} missing Soniox provider error")
        });
        assert_eq!(
            actual.code.as_deref(),
            expected.code.as_deref(),
            "{relative_path}: message {index} Soniox error code"
        );
        assert_eq!(
            actual.error_type.as_deref(),
            expected.error_type.as_deref(),
            "{relative_path}: message {index} Soniox error type"
        );
        assert_eq!(
            Some(actual.message.as_str()),
            expected.message.as_deref(),
            "{relative_path}: message {index} Soniox error message"
        );
        assert_eq!(
            actual.request_id.as_deref(),
            expected.request_id.as_deref(),
            "{relative_path}: message {index} Soniox request id"
        );
        assert_eq!(
            actual.more_info.as_deref(),
            expected.more_info.as_deref(),
            "{relative_path}: message {index} Soniox more_info"
        );
    }
    if !message.expected_soniox_revision_sidebands.is_empty() {
        assert_eq!(
            parsed.revisions.len(),
            message.expected_soniox_revision_sidebands.len(),
            "{relative_path}: message {index} Soniox revision sideband count"
        );
        for (revision_index, (actual, expected)) in parsed
            .revisions
            .iter()
            .zip(message.expected_soniox_revision_sidebands.iter())
            .enumerate()
        {
            assert_eq!(
                actual.language, expected.language,
                "{relative_path}: message {index} Soniox revision {revision_index} language"
            );
            assert_eq!(
                actual.source_language, expected.source_language,
                "{relative_path}: message {index} Soniox revision {revision_index} source_language"
            );
            assert_eq!(
                actual.final_audio_proc_ms, expected.final_audio_proc_ms,
                "{relative_path}: message {index} Soniox revision {revision_index} final_audio_proc_ms"
            );
            assert_eq!(
                actual.total_audio_proc_ms, expected.total_audio_proc_ms,
                "{relative_path}: message {index} Soniox revision {revision_index} total_audio_proc_ms"
            );
        }
    }
}

fn assert_gladia_sideband(
    parsed: &GladiaParsedMessage,
    message: &FixtureMessage,
    relative_path: &str,
    index: usize,
) {
    if let Some(expected) = message.expected_session_id.as_deref() {
        assert_eq!(
            parsed.session_id.as_deref(),
            Some(expected),
            "{relative_path}: message {index} Gladia session id"
        );
    }
    if let Some(expected) = &message.expected_speech_event {
        let actual = parsed.speech_event.as_ref().unwrap_or_else(|| {
            panic!("{relative_path}: message {index} missing Gladia speech event")
        });
        assert_eq!(
            gladia_event_type_name(&actual.event_type),
            expected.event_type,
            "{relative_path}: message {index} Gladia speech event type"
        );
        assert_close_f64(
            actual.time,
            expected.time,
            &format!("{relative_path}: message {index}"),
            "Gladia speech event time",
        );
        assert_eq!(
            actual.channel.as_deref(),
            expected.channel.as_deref(),
            "{relative_path}: message {index} Gladia speech event channel"
        );
    }
    if let Some(expected) = &message.expected_acknowledgment {
        let actual = parsed.acknowledgment.as_ref().unwrap_or_else(|| {
            panic!("{relative_path}: message {index} missing Gladia acknowledgment")
        });
        assert_eq!(
            actual.message_type, expected.message_type,
            "{relative_path}: message {index} Gladia acknowledgment type"
        );
        assert_eq!(
            actual.acknowledged, expected.acknowledged,
            "{relative_path}: message {index} Gladia acknowledgment flag"
        );
        assert_eq!(
            actual.byte_range,
            expected.byte_range.map(|range| (range[0], range[1])),
            "{relative_path}: message {index} Gladia acknowledgment byte range"
        );
        assert_optional_time_range(
            actual.time_range,
            expected.time_range,
            relative_path,
            index,
            "Gladia acknowledgment time range",
        );
    }
    if let Some(expected) = message.expected_lifecycle_event.as_deref() {
        assert_eq!(
            parsed.lifecycle_event.as_deref(),
            Some(expected),
            "{relative_path}: message {index} Gladia lifecycle event"
        );
    }
    if let Some(expected) = &message.expected_provider_error {
        let actual = parsed.error.as_ref().unwrap_or_else(|| {
            panic!("{relative_path}: message {index} missing Gladia provider error")
        });
        assert_eq!(
            Some(actual.message_type.as_str()),
            expected.message_type.as_deref(),
            "{relative_path}: message {index} Gladia error message type"
        );
        assert_eq!(
            actual.code.as_deref(),
            expected.code.as_deref(),
            "{relative_path}: message {index} Gladia error code"
        );
        assert_eq!(
            actual.message.as_deref(),
            expected.message.as_deref(),
            "{relative_path}: message {index} Gladia error message"
        );
    }
}

fn assert_speechmatics_sideband(
    parsed: &SpeechmaticsParsedMessage,
    message: &FixtureMessage,
    relative_path: &str,
    index: usize,
) {
    if let Some(expected) = message.expected_recognition_id.as_deref() {
        assert_eq!(
            parsed.recognition_id.as_deref(),
            Some(expected),
            "{relative_path}: message {index} Speechmatics recognition id"
        );
    }
    if let Some(expected) = &message.expected_end_of_utterance {
        let actual = parsed.end_of_utterance.as_ref().unwrap_or_else(|| {
            panic!("{relative_path}: message {index} missing Speechmatics end-of-utterance")
        });
        assert_close_f64(
            actual.start_time,
            expected.start_time,
            &format!("{relative_path}: message {index}"),
            "Speechmatics end-of-utterance start time",
        );
        assert_close_f64(
            actual.end_time,
            expected.end_time,
            &format!("{relative_path}: message {index}"),
            "Speechmatics end-of-utterance end time",
        );
        assert_eq!(
            actual.channel.as_deref(),
            expected.channel.as_deref(),
            "{relative_path}: message {index} Speechmatics end-of-utterance channel"
        );
    }
    if let Some(expected) = &message.expected_provider_error {
        let actual = parsed.error.as_ref().unwrap_or_else(|| {
            panic!("{relative_path}: message {index} missing Speechmatics provider error")
        });
        assert_eq!(
            actual.error_type.as_deref(),
            expected.error_type.as_deref(),
            "{relative_path}: message {index} Speechmatics error type"
        );
        assert_eq!(
            actual.reason.as_deref(),
            expected.reason.as_deref(),
            "{relative_path}: message {index} Speechmatics error reason"
        );
        assert_eq!(
            actual.code.map(|code| code.to_string()).as_deref(),
            expected.code.as_deref(),
            "{relative_path}: message {index} Speechmatics error code"
        );
        assert_eq!(
            actual.seq_no, expected.seq_no,
            "{relative_path}: message {index} Speechmatics error sequence"
        );
    }
}

fn gladia_event_type_name(event_type: &GladiaSpeechEventType) -> &'static str {
    match event_type {
        GladiaSpeechEventType::Start => "start",
        GladiaSpeechEventType::End => "end",
    }
}

fn assert_optional_time_range(
    actual: Option<(f64, f64)>,
    expected: Option<[f64; 2]>,
    relative_path: &str,
    index: usize,
    field: &str,
) {
    match (actual, expected) {
        (Some((actual_start, actual_end)), Some([expected_start, expected_end])) => {
            let context = format!("{relative_path}: message {index}");
            assert_close_f64(
                actual_start,
                expected_start,
                &context,
                &format!("{field} start"),
            );
            assert_close_f64(actual_end, expected_end, &context, &format!("{field} end"));
        }
        (None, None) => {}
        (actual, expected) => {
            panic!(
                "{relative_path}: message {index} {field}: expected {expected:?}, got {actual:?}"
            )
        }
    }
}

fn assert_optional_f64(actual: Option<f64>, expected: Option<f64>, context: &str, field: &str) {
    match (actual, expected) {
        (Some(actual), Some(expected)) => assert_close_f64(actual, expected, context, field),
        (None, None) => {}
        (actual, expected) => {
            panic!("{context}: {field}: expected {expected:?}, got {actual:?}")
        }
    }
}

fn assert_asr_revision_eq(
    actual: &AsrSpanRevisionPayload,
    expected: &AsrSpanRevisionPayload,
    context: &str,
) {
    assert_eq!(actual.span_id, expected.span_id, "{context}: span_id");
    assert_eq!(actual.provider, expected.provider, "{context}: provider");
    assert_eq!(actual.source_id, expected.source_id, "{context}: source_id");
    assert_eq!(
        actual.provider_item_id, expected.provider_item_id,
        "{context}: provider_item_id"
    );
    assert_eq!(
        actual.transcript_segment_id, expected.transcript_segment_id,
        "{context}: transcript_segment_id"
    );
    assert_eq!(
        actual.speaker_id, expected.speaker_id,
        "{context}: speaker_id"
    );
    assert_eq!(
        actual.speaker_label, expected.speaker_label,
        "{context}: speaker_label"
    );
    assert_eq!(actual.channel, expected.channel, "{context}: channel");
    assert_eq!(actual.text, expected.text, "{context}: text");
    assert_close_f64(
        actual.start_time,
        expected.start_time,
        context,
        "start_time",
    );
    assert_close_f64(actual.end_time, expected.end_time, context, "end_time");
    assert_close_f32(
        actual.confidence,
        expected.confidence,
        context,
        "confidence",
    );
    assert_eq!(actual.is_final, expected.is_final, "{context}: is_final");
    assert_stability_eq(&actual.stability, &expected.stability, context);
    assert_eq!(
        actual.revision_number, expected.revision_number,
        "{context}: revision_number"
    );
    assert_eq!(
        actual.supersedes, expected.supersedes,
        "{context}: supersedes"
    );
    assert_eq!(actual.turn_id, expected.turn_id, "{context}: turn_id");
    assert_eq!(
        actual.end_of_turn, expected.end_of_turn,
        "{context}: end_of_turn"
    );
    assert_eq!(
        actual.raw_event_ref, expected.raw_event_ref,
        "{context}: raw_event_ref"
    );
    assert_eq!(
        actual.capture_latency_ms, expected.capture_latency_ms,
        "{context}: capture_latency_ms"
    );
    assert_eq!(
        actual.asr_latency_ms, expected.asr_latency_ms,
        "{context}: asr_latency_ms"
    );
    assert_eq!(
        actual.received_at_ms, expected.received_at_ms,
        "{context}: received_at_ms"
    );
}

fn assert_stability_eq(actual: &AsrSpanStability, expected: &AsrSpanStability, context: &str) {
    assert_eq!(actual, expected, "{context}: stability");
}

fn assert_close_f64(actual: f64, expected: f64, context: &str, field: &str) {
    let delta = (actual - expected).abs();
    assert!(
        delta <= 0.000_001,
        "{context}: {field}: expected {expected}, got {actual}, delta {delta}"
    );
}

fn assert_close_f32(actual: f32, expected: f32, context: &str, field: &str) {
    let delta = (actual - expected).abs();
    assert!(
        delta <= 0.000_1,
        "{context}: {field}: expected {expected}, got {actual}, delta {delta}"
    );
}
