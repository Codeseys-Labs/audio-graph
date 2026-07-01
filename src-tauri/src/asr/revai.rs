//! Rev AI realtime STT parser.
//!
//! This module is intentionally parser-only. It maps Rev AI streaming
//! WebSocket JSON messages into AudioGraph's normalized ASR span-revision
//! contract without opening a socket or touching credentials.

use serde::Deserialize;

use crate::events::{AsrSpanRevisionPayload, AsrSpanStability};

const PROVIDER: &str = "revai";
pub const REVAI_STREAMING_ENDPOINT: &str = "wss://api.rev.ai/speechtotext/v1/stream";
pub const REVAI_PCM16_16K_MONO_CONTENT_TYPE: &str =
    "audio/x-raw;layout=interleaved;rate=16000;format=S16LE;channels=1";

#[derive(Clone)]
pub struct RevAiStreamingConfig {
    pub access_token: String,
    pub content_type: String,
    pub transcriber: String,
    pub language: Option<String>,
    pub detailed_partials: bool,
    pub enable_speaker_switch: bool,
    pub max_segment_duration_seconds: Option<u64>,
    pub priority: Option<String>,
}

impl std::fmt::Debug for RevAiStreamingConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RevAiStreamingConfig")
            .field(
                "access_token",
                &crate::credentials::redacted_secret_presence(Some(&self.access_token)),
            )
            .field("content_type", &self.content_type)
            .field("transcriber", &self.transcriber)
            .field("language", &self.language)
            .field("detailed_partials", &self.detailed_partials)
            .field("enable_speaker_switch", &self.enable_speaker_switch)
            .field(
                "max_segment_duration_seconds",
                &self.max_segment_duration_seconds,
            )
            .field("priority", &self.priority)
            .finish()
    }
}

impl RevAiStreamingConfig {
    pub fn machine_v2_pcm16_16k(access_token: impl Into<String>) -> Self {
        Self {
            access_token: access_token.into(),
            content_type: REVAI_PCM16_16K_MONO_CONTENT_TYPE.to_string(),
            transcriber: "machine_v2".to_string(),
            language: None,
            detailed_partials: true,
            enable_speaker_switch: true,
            max_segment_duration_seconds: None,
            priority: None,
        }
    }

    pub fn streaming_url(&self) -> Result<url::Url, url::ParseError> {
        let mut url = url::Url::parse(REVAI_STREAMING_ENDPOINT)?;
        {
            let mut pairs = url.query_pairs_mut();
            pairs.append_pair("access_token", self.access_token.trim());
            pairs.append_pair("content_type", &self.content_type);
            pairs.append_pair("transcriber", &self.transcriber);
            pairs.append_pair("detailed_partials", bool_query(self.detailed_partials));
            pairs.append_pair(
                "enable_speaker_switch",
                bool_query(self.enable_speaker_switch),
            );
            if let Some(language) = self.language.as_deref().map(str::trim)
                && !language.is_empty()
            {
                pairs.append_pair("language", language);
            }
            if let Some(max_segment_duration_seconds) = self.max_segment_duration_seconds {
                pairs.append_pair(
                    "max_segment_duration_seconds",
                    &max_segment_duration_seconds.to_string(),
                );
            }
            if let Some(priority) = self.priority.as_deref().map(str::trim)
                && !priority.is_empty()
            {
                pairs.append_pair("priority", priority);
            }
        }
        Ok(url)
    }
}

#[derive(Debug, Clone)]
pub struct RevAiParsedMessage {
    pub connected_id: Option<String>,
    pub revisions: Vec<RevAiParsedRevision>,
}

#[derive(Debug, Clone)]
pub struct RevAiParsedRevision {
    pub payload: AsrSpanRevisionPayload,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevAiParseError {
    InvalidJson(String),
    UnsupportedMessageType(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevAiCloseClassification {
    pub code: u16,
    pub reason: &'static str,
    pub retryable: bool,
}

#[derive(Debug)]
pub struct RevAiStreamingParser {
    source_id: String,
    segment_index: u64,
    response_sequence: u64,
    active_segment: Option<RevAiActiveSegment>,
}

#[derive(Debug, Clone)]
struct RevAiActiveSegment {
    span_id: String,
    provider_item_id: String,
    revision_number: u64,
}

#[derive(Debug, Deserialize)]
struct RevAiResponse {
    #[serde(rename = "type")]
    message_type: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    ts: Option<f64>,
    #[serde(default)]
    end_ts: Option<f64>,
    #[serde(default)]
    elements: Vec<RevAiElement>,
    #[serde(default)]
    speaker_id: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RevAiElement {
    #[serde(rename = "type")]
    element_type: String,
    value: String,
    #[serde(default)]
    ts: Option<f64>,
    #[serde(default)]
    end_ts: Option<f64>,
    #[serde(default)]
    confidence: Option<f32>,
}

impl RevAiStreamingParser {
    pub fn new(source_id: impl Into<String>) -> Self {
        Self {
            source_id: source_id.into(),
            segment_index: 0,
            response_sequence: 0,
            active_segment: None,
        }
    }

    pub fn parse_message(
        &mut self,
        text: &str,
        received_at_ms: u64,
    ) -> Result<RevAiParsedMessage, RevAiParseError> {
        let response: RevAiResponse = serde_json::from_str(text)
            .map_err(|error| RevAiParseError::InvalidJson(error.to_string()))?;
        self.response_sequence += 1;

        match response.message_type.as_str() {
            "connected" => Ok(RevAiParsedMessage {
                connected_id: response.id,
                revisions: Vec::new(),
            }),
            "partial" => {
                let revision = self.emit_revision(&response, received_at_ms, false);
                Ok(RevAiParsedMessage {
                    connected_id: None,
                    revisions: revision.into_iter().collect(),
                })
            }
            "final" => {
                let revision = self.emit_revision(&response, received_at_ms, true);
                if revision.is_some() {
                    self.active_segment = None;
                }
                Ok(RevAiParsedMessage {
                    connected_id: None,
                    revisions: revision.into_iter().collect(),
                })
            }
            other => Err(RevAiParseError::UnsupportedMessageType(other.to_string())),
        }
    }

    fn emit_revision(
        &mut self,
        response: &RevAiResponse,
        received_at_ms: u64,
        is_final: bool,
    ) -> Option<RevAiParsedRevision> {
        let text = joined_text(&response.elements);
        if text.trim().is_empty() {
            return None;
        }

        let source_id = self.source_id.clone();
        let active_segment = self.ensure_active_segment();
        active_segment.revision_number += 1;
        let revision_number = active_segment.revision_number;
        let supersedes = (revision_number > 1)
            .then(|| revision_ref(&active_segment.span_id, revision_number - 1));
        let speaker_id = response.speaker_id.as_ref().map(json_value_to_string);

        Some(RevAiParsedRevision {
            payload: AsrSpanRevisionPayload {
                span_id: active_segment.span_id.clone(),
                provider: PROVIDER.to_string(),
                source_id,
                provider_item_id: Some(active_segment.provider_item_id.clone()),
                transcript_segment_id: is_final
                    .then(|| format!("{}@final", active_segment.span_id.as_str())),
                speaker_id: speaker_id.clone(),
                speaker_label: speaker_id
                    .as_ref()
                    .map(|speaker| format!("Speaker {speaker}")),
                channel: None,
                text,
                start_time: response_start_time(response),
                end_time: response_end_time(response),
                confidence: average_confidence(&response.elements),
                is_final,
                stability: if is_final {
                    AsrSpanStability::Final
                } else {
                    AsrSpanStability::Partial
                },
                revision_number,
                supersedes,
                turn_id: Some(active_segment.provider_item_id.clone()),
                end_of_turn: is_final,
                raw_event_ref: Some(format!("revai.response.{}", self.response_sequence)),
                capture_latency_ms: None,
                asr_latency_ms: None,
                received_at_ms,
            },
        })
    }

    fn ensure_active_segment(&mut self) -> &mut RevAiActiveSegment {
        if self.active_segment.is_none() {
            self.segment_index += 1;
            let provider_item_id = format!("segment-{}", self.segment_index);
            self.active_segment = Some(RevAiActiveSegment {
                span_id: revai_span_id(&self.source_id, self.segment_index),
                provider_item_id,
                revision_number: 0,
            });
        }
        self.active_segment
            .as_mut()
            .expect("active segment initialized")
    }
}

pub fn classify_close_code(code: u16) -> RevAiCloseClassification {
    match code {
        4001 => RevAiCloseClassification {
            code,
            reason: "unauthorized",
            retryable: false,
        },
        4002 => RevAiCloseClassification {
            code,
            reason: "bad_request",
            retryable: false,
        },
        4003 => RevAiCloseClassification {
            code,
            reason: "insufficient_credits",
            retryable: false,
        },
        4010 => RevAiCloseClassification {
            code,
            reason: "server_shutting_down",
            retryable: true,
        },
        4013 => RevAiCloseClassification {
            code,
            reason: "no_instance_available",
            retryable: true,
        },
        4029 => RevAiCloseClassification {
            code,
            reason: "too_many_requests",
            retryable: false,
        },
        _ => RevAiCloseClassification {
            code,
            reason: "unknown",
            retryable: false,
        },
    }
}

fn joined_text(elements: &[RevAiElement]) -> String {
    elements
        .iter()
        .filter(|element| element.element_type == "text" || element.element_type == "punct")
        .map(|element| element.value.as_str())
        .collect::<String>()
        .trim()
        .to_string()
}

fn response_start_time(response: &RevAiResponse) -> f64 {
    sanitized_seconds(
        response
            .ts
            .or_else(|| {
                response
                    .elements
                    .iter()
                    .filter_map(|element| element.ts)
                    .reduce(f64::min)
            })
            .unwrap_or(0.0),
    )
}

fn response_end_time(response: &RevAiResponse) -> f64 {
    let start_time = response_start_time(response);
    sanitized_seconds(
        response
            .end_ts
            .or_else(|| {
                response
                    .elements
                    .iter()
                    .filter_map(|element| element.end_ts)
                    .reduce(f64::max)
            })
            .unwrap_or(start_time),
    )
    .max(start_time)
}

fn average_confidence(elements: &[RevAiElement]) -> f32 {
    let mut total = 0.0;
    let mut count = 0usize;
    for confidence in elements.iter().filter_map(|element| element.confidence) {
        if confidence.is_finite() {
            total += confidence.clamp(0.0, 1.0);
            count += 1;
        }
    }

    if count == 0 {
        0.0
    } else {
        total / count as f32
    }
}

fn sanitized_seconds(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

fn revision_ref(span_id: &str, revision_number: u64) -> String {
    format!("{span_id}@rev{revision_number}")
}

fn revai_span_id(source_id: &str, segment_index: u64) -> String {
    format!("{PROVIDER}:{source_id}:segment-{segment_index}")
}

fn json_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn bool_query(value: bool) -> &'static str {
    if value { "true" } else { "false" }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projections::{TranscriptEvent, TranscriptLedger};
    use std::collections::HashMap;

    #[test]
    fn streaming_url_includes_auth_audio_and_machine_v2_controls() {
        let mut config = RevAiStreamingConfig::machine_v2_pcm16_16k("  test-token  ");
        config.language = Some("en".to_string());
        config.max_segment_duration_seconds = Some(12);
        config.priority = Some("low".to_string());

        let url = config.streaming_url().expect("streaming url");
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();

        assert_eq!(url.scheme(), "wss");
        assert_eq!(url.host_str(), Some("api.rev.ai"));
        assert_eq!(url.path(), "/speechtotext/v1/stream");
        assert_eq!(
            query.get("access_token").map(String::as_str),
            Some("test-token")
        );
        assert_eq!(
            query.get("content_type").map(String::as_str),
            Some(REVAI_PCM16_16K_MONO_CONTENT_TYPE)
        );
        assert_eq!(
            query.get("transcriber").map(String::as_str),
            Some("machine_v2")
        );
        assert_eq!(
            query.get("detailed_partials").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            query.get("enable_speaker_switch").map(String::as_str),
            Some("true")
        );
        assert_eq!(query.get("language").map(String::as_str), Some("en"));
        assert_eq!(
            query
                .get("max_segment_duration_seconds")
                .map(String::as_str),
            Some("12")
        );
        assert_eq!(query.get("priority").map(String::as_str), Some("low"));
    }

    #[test]
    fn config_debug_redacts_access_token() {
        let config = RevAiStreamingConfig::machine_v2_pcm16_16k("revai-secret");

        let debug = format!("{config:?}");

        assert!(debug.contains("access_token"));
        assert!(debug.contains("<present>"));
        assert!(!debug.contains("revai-secret"));
    }

    #[test]
    fn connected_message_records_job_id_without_transcript_revision() {
        let mut parser = RevAiStreamingParser::new("mic-1");

        let parsed = parser
            .parse_message(r#"{ "type": "connected", "id": "job-123" }"#, 100)
            .unwrap();

        assert_eq!(parsed.connected_id.as_deref(), Some("job-123"));
        assert!(parsed.revisions.is_empty());
    }

    #[test]
    fn partial_then_final_revisions_share_segment_and_replay_without_duplicates() {
        let mut parser = RevAiStreamingParser::new("mic-1");

        let first_partial = parser
            .parse_message(
                r#"{
                    "type": "partial",
                    "ts": 0.0,
                    "end_ts": 1.2,
                    "elements": [
                        { "type": "text", "value": "hello" }
                    ]
                }"#,
                1_700_000_000_001,
            )
            .unwrap();
        let second_partial = parser
            .parse_message(
                r#"{
                    "type": "partial",
                    "ts": 0.0,
                    "end_ts": 1.45,
                    "elements": [
                        { "type": "text", "value": "hello" },
                        { "type": "punct", "value": " " },
                        { "type": "text", "value": "world" }
                    ]
                }"#,
                1_700_000_000_002,
            )
            .unwrap();
        let final_message = parser
            .parse_message(
                r#"{
                    "type": "final",
                    "ts": 0.0,
                    "end_ts": 1.48,
                    "elements": [
                        { "type": "text", "value": "hello", "ts": 0.02, "end_ts": 0.44, "confidence": 0.93 },
                        { "type": "punct", "value": " " },
                        { "type": "text", "value": "world", "ts": 0.58, "end_ts": 1.1, "confidence": 0.97 },
                        { "type": "punct", "value": "." }
                    ]
                }"#,
                1_700_000_000_003,
            )
            .unwrap();

        assert_eq!(first_partial.revisions.len(), 1);
        assert_eq!(second_partial.revisions.len(), 1);
        assert_eq!(final_message.revisions.len(), 1);

        let first = &first_partial.revisions[0].payload;
        let second = &second_partial.revisions[0].payload;
        let final_revision = &final_message.revisions[0].payload;
        let span_id = "revai:mic-1:segment-1";

        assert_eq!(first.span_id, span_id);
        assert_eq!(first.text, "hello");
        assert_eq!(first.revision_number, 1);
        assert!(!first.is_final);
        assert_eq!(second.span_id, span_id);
        assert_eq!(second.text, "hello world");
        assert_eq!(second.revision_number, 2);
        assert_eq!(
            second.supersedes.as_deref(),
            Some("revai:mic-1:segment-1@rev1")
        );
        assert_eq!(final_revision.span_id, span_id);
        assert_eq!(final_revision.text, "hello world.");
        assert_eq!(final_revision.revision_number, 3);
        assert!(final_revision.is_final);
        assert!(final_revision.end_of_turn);
        assert!((final_revision.confidence - 0.95).abs() < f32::EPSILON);
        assert_eq!(
            final_revision.transcript_segment_id.as_deref(),
            Some("revai:mic-1:segment-1@final")
        );

        let ledger = TranscriptLedger::replay(
            "session-revai",
            [
                TranscriptEvent::from(first.clone()),
                TranscriptEvent::from(second.clone()),
                TranscriptEvent::from(final_revision.clone()),
            ],
        )
        .unwrap();

        assert_eq!(ledger.accepted_event_count, 3);
        assert_eq!(ledger.latest_spans.len(), 1);
        assert_eq!(ledger.latest_spans[0].span_id, span_id);
        assert_eq!(ledger.latest_spans[0].revision_number, 3);
        assert!(ledger.latest_spans[0].is_final);
    }

    #[test]
    fn eos_final_hypothesis_closes_active_segment_and_next_partial_starts_new_one() {
        let mut parser = RevAiStreamingParser::new("desktop");
        parser
            .parse_message(
                r#"{
                    "type": "partial",
                    "ts": 3.0,
                    "end_ts": 3.6,
                    "elements": [{ "type": "text", "value": "done" }]
                }"#,
                1,
            )
            .unwrap();

        let final_after_eos = parser
            .parse_message(
                r#"{
                    "type": "final",
                    "ts": 3.0,
                    "end_ts": 3.8,
                    "elements": [
                        { "type": "text", "value": "done", "ts": 3.0, "end_ts": 3.4, "confidence": 0.91 },
                        { "type": "punct", "value": "." }
                    ]
                }"#,
                2,
            )
            .unwrap();
        let next_partial = parser
            .parse_message(
                r#"{
                    "type": "partial",
                    "ts": 4.0,
                    "end_ts": 4.4,
                    "elements": [{ "type": "text", "value": "next" }]
                }"#,
                3,
            )
            .unwrap();

        assert_eq!(
            final_after_eos.revisions[0].payload.span_id,
            "revai:desktop:segment-1"
        );
        assert!(final_after_eos.revisions[0].payload.is_final);
        assert_eq!(
            next_partial.revisions[0].payload.span_id,
            "revai:desktop:segment-2"
        );
        assert_eq!(next_partial.revisions[0].payload.revision_number, 1);
    }

    #[test]
    fn final_speaker_switch_id_is_preserved_as_provider_speaker_metadata() {
        let mut parser = RevAiStreamingParser::new("meeting");

        let parsed = parser
            .parse_message(
                r#"{
                    "type": "final",
                    "ts": 1.01,
                    "end_ts": 3.2,
                    "speaker_id": 1000,
                    "elements": [
                        { "type": "text", "value": "One", "ts": 1.04, "end_ts": 1.55, "confidence": 1.0 },
                        { "type": "punct", "value": " " },
                        { "type": "text", "value": "two", "ts": 1.84, "end_ts": 2.15, "confidence": 1.0 },
                        { "type": "punct", "value": "." }
                    ]
                }"#,
                42,
            )
            .unwrap();

        let payload = &parsed.revisions[0].payload;
        assert_eq!(payload.speaker_id.as_deref(), Some("1000"));
        assert_eq!(payload.speaker_label.as_deref(), Some("Speaker 1000"));
        assert_eq!(payload.start_time, 1.01);
        assert_eq!(payload.end_time, 3.2);
    }

    #[test]
    fn empty_partial_is_ignored_without_advancing_segment() {
        let mut parser = RevAiStreamingParser::new("mic");

        let empty = parser
            .parse_message(
                r#"{ "type": "partial", "ts": 0.0, "end_ts": 0.1, "elements": [] }"#,
                1,
            )
            .unwrap();
        let final_message = parser
            .parse_message(
                r#"{
                    "type": "final",
                    "ts": 0.0,
                    "end_ts": 0.2,
                    "elements": [{ "type": "text", "value": "ok" }]
                }"#,
                2,
            )
            .unwrap();

        assert!(empty.revisions.is_empty());
        assert_eq!(
            final_message.revisions[0].payload.span_id,
            "revai:mic:segment-1"
        );
        assert_eq!(final_message.revisions[0].payload.revision_number, 1);
    }

    #[test]
    fn close_codes_classify_retryable_and_terminal_failures() {
        assert_eq!(
            classify_close_code(4001),
            RevAiCloseClassification {
                code: 4001,
                reason: "unauthorized",
                retryable: false,
            }
        );
        assert_eq!(
            classify_close_code(4002),
            RevAiCloseClassification {
                code: 4002,
                reason: "bad_request",
                retryable: false,
            }
        );
        assert_eq!(
            classify_close_code(4010),
            RevAiCloseClassification {
                code: 4010,
                reason: "server_shutting_down",
                retryable: true,
            }
        );
        assert_eq!(
            classify_close_code(4013),
            RevAiCloseClassification {
                code: 4013,
                reason: "no_instance_available",
                retryable: true,
            }
        );
        assert_eq!(
            classify_close_code(4029),
            RevAiCloseClassification {
                code: 4029,
                reason: "too_many_requests",
                retryable: false,
            }
        );
    }

    #[test]
    fn invalid_json_and_unknown_message_types_are_errors() {
        let mut parser = RevAiStreamingParser::new("mic");
        assert!(matches!(
            parser.parse_message("{", 1).unwrap_err(),
            RevAiParseError::InvalidJson(_)
        ));
        assert_eq!(
            parser
                .parse_message(r#"{ "type": "unknown" }"#, 2)
                .unwrap_err(),
            RevAiParseError::UnsupportedMessageType("unknown".to_string())
        );
    }
}
