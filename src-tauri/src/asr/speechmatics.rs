//! Speechmatics Realtime STT parser.
//!
//! This module is intentionally parser-only. It models the Realtime
//! WebSocket configuration and maps server JSON messages into AudioGraph's
//! normalized ASR span-revision contract without opening a socket or touching
//! the local credentials store.

use serde::Deserialize;
use serde_json::json;

use crate::events::{AsrSpanRevisionPayload, AsrSpanStability};

const PROVIDER: &str = "speechmatics";
pub const SPEECHMATICS_EU1_REALTIME_ENDPOINT: &str = "wss://eu.rt.speechmatics.com/v2";
pub const SPEECHMATICS_US1_REALTIME_ENDPOINT: &str = "wss://us.rt.speechmatics.com/v2";
pub const SPEECHMATICS_PCM16_ENCODING: &str = "pcm_s16le";
pub const SPEECHMATICS_DEFAULT_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechmaticsRealtimeRegion {
    Eu1,
    Us1,
}

impl SpeechmaticsRealtimeRegion {
    pub fn endpoint(self) -> &'static str {
        match self {
            Self::Eu1 => SPEECHMATICS_EU1_REALTIME_ENDPOINT,
            Self::Us1 => SPEECHMATICS_US1_REALTIME_ENDPOINT,
        }
    }
}

#[derive(Clone)]
pub enum SpeechmaticsRealtimeAuth {
    BearerToken(String),
    TemporaryJwt(String),
}

impl std::fmt::Debug for SpeechmaticsRealtimeAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BearerToken(token) => f
                .debug_tuple("BearerToken")
                .field(&crate::credentials::redacted_secret_presence(Some(token)))
                .finish(),
            Self::TemporaryJwt(jwt) => f
                .debug_tuple("TemporaryJwt")
                .field(&crate::credentials::redacted_secret_presence(Some(jwt)))
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechmaticsDiarizationMode {
    None,
    Speaker,
}

#[derive(Clone)]
pub struct SpeechmaticsRealtimeConfig {
    pub auth: SpeechmaticsRealtimeAuth,
    pub region: SpeechmaticsRealtimeRegion,
    pub language: String,
    pub model: String,
    pub sample_rate: u32,
    pub enable_partials: bool,
    pub max_delay_seconds: f32,
    pub max_delay_mode: String,
    pub diarization: SpeechmaticsDiarizationMode,
    pub speaker_sensitivity: Option<f32>,
    pub prefer_current_speaker: Option<bool>,
    pub max_speakers: Option<u32>,
    pub end_of_utterance_silence_trigger_seconds: Option<f32>,
}

impl std::fmt::Debug for SpeechmaticsRealtimeConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpeechmaticsRealtimeConfig")
            .field("auth", &self.auth)
            .field("region", &self.region)
            .field("language", &self.language)
            .field("model", &self.model)
            .field("sample_rate", &self.sample_rate)
            .field("enable_partials", &self.enable_partials)
            .field("max_delay_seconds", &self.max_delay_seconds)
            .field("max_delay_mode", &self.max_delay_mode)
            .field("diarization", &self.diarization)
            .field("speaker_sensitivity", &self.speaker_sensitivity)
            .field("prefer_current_speaker", &self.prefer_current_speaker)
            .field("max_speakers", &self.max_speakers)
            .field(
                "end_of_utterance_silence_trigger_seconds",
                &self.end_of_utterance_silence_trigger_seconds,
            )
            .finish()
    }
}

impl SpeechmaticsRealtimeConfig {
    pub fn enhanced_pcm16_16k_bearer(api_key: impl Into<String>) -> Self {
        Self {
            auth: SpeechmaticsRealtimeAuth::BearerToken(api_key.into()),
            region: SpeechmaticsRealtimeRegion::Eu1,
            language: "en".to_string(),
            model: "enhanced".to_string(),
            sample_rate: SPEECHMATICS_DEFAULT_SAMPLE_RATE,
            enable_partials: true,
            max_delay_seconds: 0.7,
            max_delay_mode: "flexible".to_string(),
            diarization: SpeechmaticsDiarizationMode::Speaker,
            speaker_sensitivity: None,
            prefer_current_speaker: None,
            max_speakers: None,
            end_of_utterance_silence_trigger_seconds: Some(0.5),
        }
    }

    pub fn websocket_url(&self) -> Result<url::Url, url::ParseError> {
        let mut url = url::Url::parse(self.region.endpoint())?;
        if let SpeechmaticsRealtimeAuth::TemporaryJwt(jwt) = &self.auth {
            url.query_pairs_mut().append_pair("jwt", jwt.trim());
        }
        Ok(url)
    }

    pub fn authorization_header_value(&self) -> Option<String> {
        match &self.auth {
            SpeechmaticsRealtimeAuth::BearerToken(token) => {
                Some(format!("Bearer {}", token.trim()))
            }
            SpeechmaticsRealtimeAuth::TemporaryJwt(_) => None,
        }
    }

    pub fn start_recognition_message(&self) -> serde_json::Value {
        let mut transcription_config = serde_json::Map::from_iter([
            ("language".to_string(), json!(self.language)),
            ("model".to_string(), json!(self.model)),
            ("enable_partials".to_string(), json!(self.enable_partials)),
            ("max_delay".to_string(), json!(self.max_delay_seconds)),
            ("max_delay_mode".to_string(), json!(self.max_delay_mode)),
        ]);

        if self.diarization == SpeechmaticsDiarizationMode::Speaker {
            transcription_config.insert("diarization".to_string(), json!("speaker"));
            let mut speaker_config = serde_json::Map::new();
            if let Some(speaker_sensitivity) = self.speaker_sensitivity {
                speaker_config.insert(
                    "speaker_sensitivity".to_string(),
                    json!(speaker_sensitivity),
                );
            }
            if let Some(prefer_current_speaker) = self.prefer_current_speaker {
                speaker_config.insert(
                    "prefer_current_speaker".to_string(),
                    json!(prefer_current_speaker),
                );
            }
            if let Some(max_speakers) = self.max_speakers {
                speaker_config.insert("max_speakers".to_string(), json!(max_speakers));
            }
            if !speaker_config.is_empty() {
                transcription_config.insert(
                    "speaker_diarization_config".to_string(),
                    serde_json::Value::Object(speaker_config),
                );
            }
        }

        if let Some(trigger_seconds) = self.end_of_utterance_silence_trigger_seconds {
            transcription_config.insert(
                "conversation_config".to_string(),
                json!({ "end_of_utterance_silence_trigger": trigger_seconds }),
            );
        }

        json!({
            "message": "StartRecognition",
            "audio_format": {
                "type": "raw",
                "encoding": SPEECHMATICS_PCM16_ENCODING,
                "sample_rate": self.sample_rate,
            },
            "transcription_config": transcription_config,
        })
    }
}

#[derive(Debug, Clone)]
pub struct SpeechmaticsParsedMessage {
    pub recognition_id: Option<String>,
    pub revisions: Vec<SpeechmaticsParsedRevision>,
    pub end_of_utterance: Option<SpeechmaticsEndOfUtterance>,
    pub error: Option<SpeechmaticsProviderError>,
}

#[derive(Debug, Clone)]
pub struct SpeechmaticsParsedRevision {
    pub payload: AsrSpanRevisionPayload,
    pub language: Option<String>,
    pub format: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeechmaticsEndOfUtterance {
    pub start_time: f64,
    pub end_time: f64,
    pub channel: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechmaticsProviderError {
    pub error_type: Option<String>,
    pub reason: Option<String>,
    pub code: Option<i64>,
    pub seq_no: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeechmaticsParseError {
    InvalidJson(String),
    UnsupportedMessageType(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechmaticsCloseClassification {
    pub code: u16,
    pub reason: &'static str,
    pub retryable: bool,
}

#[derive(Debug)]
pub struct SpeechmaticsRealtimeParser {
    source_id: String,
    segment_index: u64,
    response_sequence: u64,
    active_segment: Option<SpeechmaticsActiveSegment>,
}

#[derive(Debug, Clone)]
struct SpeechmaticsActiveSegment {
    span_id: String,
    provider_item_id: String,
    revision_number: u64,
}

#[derive(Debug, Deserialize)]
struct SpeechmaticsResponse {
    message: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    metadata: Option<SpeechmaticsMetadata>,
    #[serde(default)]
    results: Vec<SpeechmaticsResult>,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    seq_no: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
struct SpeechmaticsMetadata {
    #[serde(default)]
    start_time: Option<f64>,
    #[serde(default)]
    end_time: Option<f64>,
    #[serde(default)]
    transcript: Option<String>,
    #[serde(default)]
    channel: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct SpeechmaticsResult {
    #[serde(rename = "type")]
    result_type: String,
    #[serde(default)]
    start_time: Option<f64>,
    #[serde(default)]
    end_time: Option<f64>,
    #[serde(default)]
    alternatives: Vec<SpeechmaticsAlternative>,
}

#[derive(Debug, Clone, Deserialize)]
struct SpeechmaticsAlternative {
    content: String,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    speaker: Option<String>,
}

impl SpeechmaticsRealtimeParser {
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
    ) -> Result<SpeechmaticsParsedMessage, SpeechmaticsParseError> {
        let response: SpeechmaticsResponse = serde_json::from_str(text)
            .map_err(|error| SpeechmaticsParseError::InvalidJson(error.to_string()))?;
        self.response_sequence += 1;

        match response.message.as_str() {
            "RecognitionStarted" => Ok(SpeechmaticsParsedMessage {
                recognition_id: response.id,
                revisions: Vec::new(),
                end_of_utterance: None,
                error: None,
            }),
            "AddPartialTranscript" => Ok(SpeechmaticsParsedMessage {
                recognition_id: None,
                revisions: self
                    .emit_transcript_revision(&response, received_at_ms, false)
                    .into_iter()
                    .collect(),
                end_of_utterance: None,
                error: None,
            }),
            "AddTranscript" => {
                let revision = self.emit_transcript_revision(&response, received_at_ms, true);
                if revision.is_some() {
                    self.active_segment = None;
                }
                Ok(SpeechmaticsParsedMessage {
                    recognition_id: None,
                    revisions: revision.into_iter().collect(),
                    end_of_utterance: None,
                    error: None,
                })
            }
            "EndOfUtterance" => {
                let boundary = end_of_utterance_boundary(&response);
                Ok(SpeechmaticsParsedMessage {
                    recognition_id: None,
                    revisions: Vec::new(),
                    end_of_utterance: boundary,
                    error: None,
                })
            }
            "AudioAdded" | "ChannelAudioAdded" | "EndOfTranscript" | "Info" | "Warning"
            | "SpeakersResult" | "AudioEventStarted" | "AudioEventEnded" => {
                Ok(SpeechmaticsParsedMessage {
                    recognition_id: None,
                    revisions: Vec::new(),
                    end_of_utterance: None,
                    error: None,
                })
            }
            "Error" => Ok(SpeechmaticsParsedMessage {
                recognition_id: None,
                revisions: Vec::new(),
                end_of_utterance: None,
                error: Some(SpeechmaticsProviderError {
                    error_type: response.r#type,
                    reason: response.reason,
                    code: response.code,
                    seq_no: response.seq_no,
                }),
            }),
            other => Err(SpeechmaticsParseError::UnsupportedMessageType(
                other.to_string(),
            )),
        }
    }

    fn emit_transcript_revision(
        &mut self,
        response: &SpeechmaticsResponse,
        received_at_ms: u64,
        is_final: bool,
    ) -> Option<SpeechmaticsParsedRevision> {
        let text = transcript_text(response);
        if text.trim().is_empty() {
            return None;
        }

        let source_id = self.source_id.clone();
        let response_sequence = self.response_sequence;
        let metadata = response.metadata.as_ref();
        let results = transcript_results(&response.results);
        let language =
            consistent_result_field(&results, |alternative| alternative.language.as_deref());
        let speaker =
            consistent_result_field(&results, |alternative| alternative.speaker.as_deref());
        let active_segment = self.ensure_active_segment();
        active_segment.revision_number += 1;
        let revision_number = active_segment.revision_number;
        let supersedes = (revision_number > 1)
            .then(|| revision_ref(&active_segment.span_id, revision_number - 1));

        let payload = AsrSpanRevisionPayload {
            span_id: active_segment.span_id.clone(),
            provider: PROVIDER.to_string(),
            source_id,
            provider_item_id: Some(active_segment.provider_item_id.clone()),
            transcript_segment_id: is_final
                .then(|| format!("{}@final", active_segment.span_id.as_str())),
            speaker_id: speaker.clone(),
            speaker_label: speaker.as_ref().map(|speaker| format!("Speaker {speaker}")),
            channel: response
                .channel
                .clone()
                .or_else(|| metadata.and_then(|metadata| metadata.channel.clone())),
            text,
            start_time: response_start_time(metadata, &results),
            end_time: response_end_time(metadata, &results),
            confidence: average_confidence(&results),
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
            raw_event_ref: Some(format!("speechmatics.response.{response_sequence}")),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms,
        };

        Some(SpeechmaticsParsedRevision {
            payload,
            language,
            format: response.format.clone(),
        })
    }

    fn ensure_active_segment(&mut self) -> &mut SpeechmaticsActiveSegment {
        if self.active_segment.is_none() {
            self.segment_index += 1;
            let provider_item_id = format!("segment-{}", self.segment_index);
            self.active_segment = Some(SpeechmaticsActiveSegment {
                span_id: speechmatics_span_id(&self.source_id, self.segment_index),
                provider_item_id,
                revision_number: 0,
            });
        }
        self.active_segment
            .as_mut()
            .expect("active segment initialized")
    }
}

pub fn classify_close_code(code: u16) -> SpeechmaticsCloseClassification {
    match code {
        1000 => SpeechmaticsCloseClassification {
            code,
            reason: "normal",
            retryable: false,
        },
        1003 => SpeechmaticsCloseClassification {
            code,
            reason: "protocol_error",
            retryable: false,
        },
        1008 => SpeechmaticsCloseClassification {
            code,
            reason: "policy_violation",
            retryable: false,
        },
        1011 => SpeechmaticsCloseClassification {
            code,
            reason: "internal_error",
            retryable: true,
        },
        4001 => SpeechmaticsCloseClassification {
            code,
            reason: "not_authorised",
            retryable: false,
        },
        4003 => SpeechmaticsCloseClassification {
            code,
            reason: "not_allowed",
            retryable: false,
        },
        4004 => SpeechmaticsCloseClassification {
            code,
            reason: "invalid_model",
            retryable: false,
        },
        4005 => SpeechmaticsCloseClassification {
            code,
            reason: "quota_exceeded",
            retryable: false,
        },
        4006 => SpeechmaticsCloseClassification {
            code,
            reason: "timelimit_exceeded",
            retryable: false,
        },
        4013 => SpeechmaticsCloseClassification {
            code,
            reason: "job_error",
            retryable: true,
        },
        _ => SpeechmaticsCloseClassification {
            code,
            reason: "unknown",
            retryable: false,
        },
    }
}

fn transcript_text(response: &SpeechmaticsResponse) -> String {
    response
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.transcript.as_deref())
        .map(str::trim)
        .filter(|transcript| !transcript.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| joined_results_text(&response.results))
}

fn transcript_results(results: &[SpeechmaticsResult]) -> Vec<&SpeechmaticsResult> {
    results
        .iter()
        .filter(|result| {
            result.result_type == "word"
                || result.result_type == "punctuation"
                || result.result_type == "entity"
        })
        .collect()
}

fn joined_results_text(results: &[SpeechmaticsResult]) -> String {
    transcript_results(results)
        .iter()
        .filter_map(|result| result.alternatives.first())
        .map(|alternative| alternative.content.as_str())
        .collect::<String>()
        .trim()
        .to_string()
}

fn response_start_time(
    metadata: Option<&SpeechmaticsMetadata>,
    results: &[&SpeechmaticsResult],
) -> f64 {
    sanitized_seconds(
        metadata
            .and_then(|metadata| metadata.start_time)
            .or_else(|| {
                results
                    .iter()
                    .filter_map(|result| result.start_time)
                    .reduce(f64::min)
            })
            .unwrap_or(0.0),
    )
}

fn response_end_time(
    metadata: Option<&SpeechmaticsMetadata>,
    results: &[&SpeechmaticsResult],
) -> f64 {
    let start_time = response_start_time(metadata, results);
    sanitized_seconds(
        metadata
            .and_then(|metadata| metadata.end_time)
            .or_else(|| {
                results
                    .iter()
                    .filter_map(|result| result.end_time)
                    .reduce(f64::max)
            })
            .unwrap_or(start_time),
    )
    .max(start_time)
}

fn average_confidence(results: &[&SpeechmaticsResult]) -> f32 {
    let mut total = 0.0;
    let mut count = 0usize;
    for confidence in results
        .iter()
        .flat_map(|result| result.alternatives.iter())
        .filter_map(|alternative| alternative.confidence)
        .filter(|confidence| confidence.is_finite())
    {
        total += confidence.clamp(0.0, 1.0);
        count += 1;
    }

    if count == 0 {
        0.0
    } else {
        total / count as f32
    }
}

fn consistent_result_field(
    results: &[&SpeechmaticsResult],
    field: impl Fn(&SpeechmaticsAlternative) -> Option<&str>,
) -> Option<String> {
    let mut value = None::<&str>;
    for alternative in results
        .iter()
        .filter_map(|result| result.alternatives.first())
        .filter(|alternative| !alternative.content.trim().is_empty())
    {
        let alternative_value = field(alternative)?;
        match value {
            Some(current) if current != alternative_value => return None,
            Some(_) => {}
            None => value = Some(alternative_value),
        }
    }
    value.map(str::to_string)
}

fn end_of_utterance_boundary(
    response: &SpeechmaticsResponse,
) -> Option<SpeechmaticsEndOfUtterance> {
    let metadata = response.metadata.as_ref()?;
    let start_time = sanitized_seconds(metadata.start_time.unwrap_or(0.0));
    let end_time = sanitized_seconds(metadata.end_time.unwrap_or(start_time)).max(start_time);
    Some(SpeechmaticsEndOfUtterance {
        start_time,
        end_time,
        channel: response
            .channel
            .clone()
            .or_else(|| metadata.channel.clone()),
    })
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

fn speechmatics_span_id(source_id: &str, segment_index: u64) -> String {
    format!("{PROVIDER}:{source_id}:segment-{segment_index}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projections::{TranscriptEvent, TranscriptLedger};
    use std::collections::HashMap;

    #[test]
    fn config_builds_eu_bearer_url_start_message_and_redacted_debug() {
        let mut config = SpeechmaticsRealtimeConfig::enhanced_pcm16_16k_bearer("  sm-secret  ");
        config.speaker_sensitivity = Some(0.6);
        config.prefer_current_speaker = Some(true);
        config.max_speakers = Some(4);

        let url = config.websocket_url().expect("websocket url");
        assert_eq!(url.as_str(), SPEECHMATICS_EU1_REALTIME_ENDPOINT);
        assert_eq!(
            config.authorization_header_value().as_deref(),
            Some("Bearer sm-secret")
        );

        let start = config.start_recognition_message();
        assert_eq!(start["message"], "StartRecognition");
        assert_eq!(start["audio_format"]["type"], "raw");
        assert_eq!(
            start["audio_format"]["encoding"],
            SPEECHMATICS_PCM16_ENCODING
        );
        assert_eq!(
            start["audio_format"]["sample_rate"],
            SPEECHMATICS_DEFAULT_SAMPLE_RATE
        );
        assert_eq!(start["transcription_config"]["language"], "en");
        assert_eq!(start["transcription_config"]["model"], "enhanced");
        assert_eq!(start["transcription_config"]["enable_partials"], true);
        assert!(
            (start["transcription_config"]["max_delay"].as_f64().unwrap() - 0.7).abs() < 0.000_001
        );
        assert_eq!(start["transcription_config"]["max_delay_mode"], "flexible");
        assert_eq!(start["transcription_config"]["diarization"], "speaker");
        assert!(
            (start["transcription_config"]["speaker_diarization_config"]["speaker_sensitivity"]
                .as_f64()
                .unwrap()
                - 0.6)
                .abs()
                < 0.000_001
        );
        assert_eq!(
            start["transcription_config"]["speaker_diarization_config"]["prefer_current_speaker"],
            true
        );
        assert_eq!(
            start["transcription_config"]["speaker_diarization_config"]["max_speakers"],
            4
        );
        assert_eq!(
            start["transcription_config"]["conversation_config"]["end_of_utterance_silence_trigger"],
            0.5
        );

        let debug = format!("{config:?}");
        assert!(debug.contains("BearerToken"));
        assert!(debug.contains("<present>"));
        assert!(!debug.contains("sm-secret"));
    }

    #[test]
    fn temporary_jwt_uses_query_param_without_authorization_header() {
        let mut config = SpeechmaticsRealtimeConfig::enhanced_pcm16_16k_bearer("unused");
        config.region = SpeechmaticsRealtimeRegion::Us1;
        config.auth = SpeechmaticsRealtimeAuth::TemporaryJwt("  jwt-secret  ".to_string());

        let url = config.websocket_url().expect("websocket url");
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();

        assert_eq!(url.scheme(), "wss");
        assert_eq!(url.host_str(), Some("us.rt.speechmatics.com"));
        assert_eq!(url.path(), "/v2");
        assert_eq!(query.get("jwt").map(String::as_str), Some("jwt-secret"));
        assert_eq!(config.authorization_header_value(), None);
        assert!(!format!("{config:?}").contains("jwt-secret"));
    }

    #[test]
    fn recognition_started_records_session_id_without_transcript_revision() {
        let mut parser = SpeechmaticsRealtimeParser::new("mic-1");

        let parsed = parser
            .parse_message(
                r#"{ "message": "RecognitionStarted", "id": "session-123" }"#,
                100,
            )
            .unwrap();

        assert_eq!(parsed.recognition_id.as_deref(), Some("session-123"));
        assert!(parsed.revisions.is_empty());
        assert!(parsed.end_of_utterance.is_none());
    }

    #[test]
    fn partial_final_and_end_of_utterance_replay_without_duplicates() {
        let mut parser = SpeechmaticsRealtimeParser::new("mic-1");

        let partial = parser
            .parse_message(
                r#"{
                    "message": "AddPartialTranscript",
                    "format": "2.1",
                    "metadata": {
                        "start_time": 0.0,
                        "end_time": 0.62,
                        "transcript": "hello"
                    },
                    "results": [
                        {
                            "type": "word",
                            "start_time": 0.0,
                            "end_time": 0.5,
                            "alternatives": [
                                { "content": "hello", "confidence": 0.1, "language": "en", "speaker": "S1" }
                            ]
                        }
                    ]
                }"#,
                1_700_000_000_001,
            )
            .unwrap();
        let final_message = parser
            .parse_message(
                r#"{
                    "message": "AddTranscript",
                    "format": "2.1",
                    "metadata": {
                        "start_time": 0.0,
                        "end_time": 0.92,
                        "transcript": "Hello."
                    },
                    "results": [
                        {
                            "type": "word",
                            "start_time": 0.0,
                            "end_time": 0.5,
                            "alternatives": [
                                { "content": "Hello", "confidence": 0.94, "language": "en", "speaker": "S1" }
                            ]
                        },
                        {
                            "type": "punctuation",
                            "start_time": 0.5,
                            "end_time": 0.5,
                            "alternatives": [
                                { "content": ".", "confidence": 0.98, "language": "en", "speaker": "S1" }
                            ]
                        }
                    ]
                }"#,
                1_700_000_000_002,
            )
            .unwrap();
        let end_of_utterance = parser
            .parse_message(
                r#"{
                    "message": "EndOfUtterance",
                    "metadata": {
                        "start_time": 1.2,
                        "end_time": 1.2
                    }
                }"#,
                1_700_000_000_003,
            )
            .unwrap();

        assert_eq!(partial.revisions.len(), 1);
        assert_eq!(final_message.revisions.len(), 1);
        assert!(end_of_utterance.revisions.is_empty());

        let partial_revision = &partial.revisions[0].payload;
        let final_revision = &final_message.revisions[0].payload;
        let span_id = "speechmatics:mic-1:segment-1";

        assert_eq!(partial_revision.span_id, span_id);
        assert_eq!(partial_revision.text, "hello");
        assert_eq!(partial_revision.revision_number, 1);
        assert!(!partial_revision.is_final);
        assert!(!partial_revision.end_of_turn);
        assert_eq!(partial.revisions[0].language.as_deref(), Some("en"));
        assert_eq!(partial.revisions[0].format.as_deref(), Some("2.1"));
        assert_eq!(partial_revision.speaker_id.as_deref(), Some("S1"));

        assert_eq!(final_revision.span_id, span_id);
        assert_eq!(final_revision.text, "Hello.");
        assert_eq!(final_revision.revision_number, 2);
        assert!(final_revision.is_final);
        assert!(final_revision.end_of_turn);
        assert_eq!(
            final_revision.supersedes.as_deref(),
            Some("speechmatics:mic-1:segment-1@rev1")
        );
        assert!((final_revision.confidence - 0.96).abs() < f32::EPSILON);

        assert_eq!(
            end_of_utterance
                .end_of_utterance
                .as_ref()
                .map(|boundary| boundary.end_time),
            Some(1.2)
        );

        let ledger = TranscriptLedger::replay(
            "session-speechmatics",
            [
                TranscriptEvent::from(partial_revision.clone()),
                TranscriptEvent::from(final_revision.clone()),
            ],
        )
        .unwrap();

        assert_eq!(ledger.accepted_event_count, 2);
        assert_eq!(ledger.latest_spans.len(), 1);
        assert_eq!(ledger.latest_spans[0].span_id, span_id);
        assert_eq!(ledger.latest_spans[0].revision_number, 2);
        assert!(ledger.latest_spans[0].end_of_turn);
    }

    #[test]
    fn end_of_utterance_without_prior_final_records_boundary_without_empty_revision() {
        let mut parser = SpeechmaticsRealtimeParser::new("mic-1");

        let parsed = parser
            .parse_message(
                r#"{
                    "message": "EndOfUtterance",
                    "metadata": {
                        "start_time": 2.0,
                        "end_time": 2.0,
                        "channel": "agent"
                    }
                }"#,
                1_700_000_000_010,
            )
            .unwrap();

        assert!(parsed.revisions.is_empty());
        assert_eq!(
            parsed.end_of_utterance,
            Some(SpeechmaticsEndOfUtterance {
                start_time: 2.0,
                end_time: 2.0,
                channel: Some("agent".to_string()),
            })
        );
    }

    #[test]
    fn final_without_partial_starts_new_segment_and_next_partial_advances_segment() {
        let mut parser = SpeechmaticsRealtimeParser::new("system");

        let final_only = parser
            .parse_message(
                r#"{
                    "message": "AddTranscript",
                    "metadata": {
                        "start_time": 0.0,
                        "end_time": 0.4,
                        "transcript": "Done"
                    },
                    "results": [
                        {
                            "type": "word",
                            "start_time": 0.0,
                            "end_time": 0.4,
                            "alternatives": [
                                { "content": "Done", "confidence": 0.92, "language": "en" }
                            ]
                        }
                    ]
                }"#,
                1_700_000_000_020,
            )
            .unwrap();
        let next_partial = parser
            .parse_message(
                r#"{
                    "message": "AddPartialTranscript",
                    "metadata": {
                        "start_time": 1.0,
                        "end_time": 1.2,
                        "transcript": "Next"
                    },
                    "results": [
                        {
                            "type": "word",
                            "start_time": 1.0,
                            "end_time": 1.2,
                            "alternatives": [
                                { "content": "Next", "confidence": 0.2, "language": "en" }
                            ]
                        }
                    ]
                }"#,
                1_700_000_000_021,
            )
            .unwrap();

        assert_eq!(
            final_only.revisions[0].payload.span_id,
            "speechmatics:system:segment-1"
        );
        assert!(final_only.revisions[0].payload.is_final);
        assert_eq!(
            next_partial.revisions[0].payload.span_id,
            "speechmatics:system:segment-2"
        );
        assert!(!next_partial.revisions[0].payload.is_final);
    }

    #[test]
    fn mixed_speaker_results_leave_span_speaker_unset() {
        let mut parser = SpeechmaticsRealtimeParser::new("mic-1");
        let parsed = parser
            .parse_message(
                r#"{
                    "message": "AddPartialTranscript",
                    "metadata": {
                        "start_time": 0.0,
                        "end_time": 0.8,
                        "transcript": "hello there"
                    },
                    "results": [
                        {
                            "type": "word",
                            "start_time": 0.0,
                            "end_time": 0.3,
                            "alternatives": [
                                { "content": "hello", "confidence": 0.8, "speaker": "S1" }
                            ]
                        },
                        {
                            "type": "word",
                            "start_time": 0.4,
                            "end_time": 0.8,
                            "alternatives": [
                                { "content": "there", "confidence": 0.8, "speaker": "S2" }
                            ]
                        }
                    ]
                }"#,
                1_700_000_000_030,
            )
            .unwrap();

        assert_eq!(parsed.revisions.len(), 1);
        assert_eq!(parsed.revisions[0].payload.speaker_id, None);
        assert_eq!(parsed.revisions[0].payload.speaker_label, None);
    }

    #[test]
    fn error_message_preserves_type_reason_code_without_transcript_revision() {
        let mut parser = SpeechmaticsRealtimeParser::new("mic-1");

        let parsed = parser
            .parse_message(
                r#"{
                    "message": "Error",
                    "type": "invalid_model",
                    "reason": "Model is not available",
                    "code": 4004,
                    "seq_no": 7
                }"#,
                1_700_000_000_040,
            )
            .unwrap();

        assert!(parsed.revisions.is_empty());
        assert_eq!(
            parsed.error,
            Some(SpeechmaticsProviderError {
                error_type: Some("invalid_model".to_string()),
                reason: Some("Model is not available".to_string()),
                code: Some(4004),
                seq_no: Some(7),
            })
        );
    }

    #[test]
    fn known_control_messages_do_not_emit_transcript_revisions() {
        let mut parser = SpeechmaticsRealtimeParser::new("mic-1");

        for message in [
            r#"{ "message": "AudioAdded", "seq_no": 1 }"#,
            r#"{ "message": "Info", "type": "recognition_quality", "reason": "broadcast" }"#,
            r#"{ "message": "Warning", "type": "duration_limit_exceeded", "reason": "limit" }"#,
            r#"{ "message": "EndOfTranscript" }"#,
        ] {
            let parsed = parser.parse_message(message, 1_700_000_000_050).unwrap();
            assert!(parsed.revisions.is_empty());
            assert!(parsed.error.is_none());
        }
    }

    #[test]
    fn close_code_classifier_matches_speechmatics_realtime_codes() {
        assert_eq!(classify_close_code(1003).reason, "protocol_error");
        assert_eq!(classify_close_code(1008).reason, "policy_violation");
        assert!(classify_close_code(1011).retryable);
        assert_eq!(classify_close_code(4001).reason, "not_authorised");
        assert_eq!(classify_close_code(4003).reason, "not_allowed");
        assert_eq!(classify_close_code(4004).reason, "invalid_model");
        assert_eq!(classify_close_code(4005).reason, "quota_exceeded");
        assert_eq!(classify_close_code(4006).reason, "timelimit_exceeded");
        assert_eq!(classify_close_code(4013).reason, "job_error");
        assert_eq!(classify_close_code(4999).reason, "unknown");
    }

    #[test]
    fn invalid_json_and_unknown_message_are_errors() {
        let mut parser = SpeechmaticsRealtimeParser::new("mic-1");

        assert!(matches!(
            parser.parse_message("{", 1),
            Err(SpeechmaticsParseError::InvalidJson(_))
        ));
        assert!(matches!(
            parser.parse_message(r#"{ "message": "NewFutureMessage" }"#, 2),
            Err(SpeechmaticsParseError::UnsupportedMessageType(message))
                if message == "NewFutureMessage"
        ));
    }
}
