//! Gladia live STT parser.
//!
//! This module is parser/config only. Gladia live sessions are initiated over
//! REST with a backend API key and then streamed over the returned WebSocket
//! URL; that runtime lifecycle is intentionally out of scope here.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::json;

use crate::events::{AsrSpanRevisionPayload, AsrSpanStability};

const PROVIDER: &str = "gladia";
pub const GLADIA_LIVE_INIT_ENDPOINT: &str = "https://api.gladia.io/v2/live";
pub const GLADIA_DEFAULT_MODEL: &str = "solaria-1";
pub const GLADIA_PCM_ENCODING: &str = "wav/pcm";
pub const GLADIA_DEFAULT_SAMPLE_RATE: u32 = 16_000;
pub const GLADIA_DEFAULT_BIT_DEPTH: u8 = 16;
pub const GLADIA_DEFAULT_CHANNELS: u8 = 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GladiaAudioFrame {
    Binary(Vec<u8>),
    JsonBase64(serde_json::Value),
}

impl GladiaAudioFrame {
    pub fn binary(pcm_bytes: impl Into<Vec<u8>>) -> Self {
        Self::Binary(pcm_bytes.into())
    }

    pub fn json_base64(pcm_bytes: &[u8]) -> Self {
        Self::JsonBase64(json!({
            "type": "audio_chunk",
            "data": {
                "chunk": base64_encode(pcm_bytes),
            },
        }))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GladiaRegion {
    UsWest,
    EuWest,
}

impl GladiaRegion {
    pub fn query_value(self) -> &'static str {
        match self {
            Self::UsWest => "us-west",
            Self::EuWest => "eu-west",
        }
    }
}

#[derive(Clone)]
pub struct GladiaLiveConfig {
    pub api_key: String,
    pub region: Option<GladiaRegion>,
    pub model: String,
    pub encoding: String,
    pub bit_depth: u8,
    pub sample_rate: u32,
    pub channels: u8,
    pub endpointing_seconds: f32,
    pub maximum_duration_without_endpointing_seconds: f32,
    pub languages: Vec<String>,
    pub code_switching: bool,
    pub receive_partial_transcripts: bool,
    pub receive_final_transcripts: bool,
    pub receive_speech_events: bool,
    pub receive_acknowledgments: bool,
    pub receive_errors: bool,
    pub receive_lifecycle_events: bool,
}

impl std::fmt::Debug for GladiaLiveConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GladiaLiveConfig")
            .field(
                "api_key",
                &crate::credentials::redacted_secret_presence(Some(&self.api_key)),
            )
            .field("region", &self.region)
            .field("model", &self.model)
            .field("encoding", &self.encoding)
            .field("bit_depth", &self.bit_depth)
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .field("endpointing_seconds", &self.endpointing_seconds)
            .field(
                "maximum_duration_without_endpointing_seconds",
                &self.maximum_duration_without_endpointing_seconds,
            )
            .field("languages", &self.languages)
            .field("code_switching", &self.code_switching)
            .field(
                "receive_partial_transcripts",
                &self.receive_partial_transcripts,
            )
            .field("receive_final_transcripts", &self.receive_final_transcripts)
            .field("receive_speech_events", &self.receive_speech_events)
            .field("receive_acknowledgments", &self.receive_acknowledgments)
            .field("receive_errors", &self.receive_errors)
            .field("receive_lifecycle_events", &self.receive_lifecycle_events)
            .finish()
    }
}

impl GladiaLiveConfig {
    pub fn solaria_pcm16_16k(api_key: impl Into<String>) -> Self {
        Self {
            api_key: api_key.into(),
            region: None,
            model: GLADIA_DEFAULT_MODEL.to_string(),
            encoding: GLADIA_PCM_ENCODING.to_string(),
            bit_depth: GLADIA_DEFAULT_BIT_DEPTH,
            sample_rate: GLADIA_DEFAULT_SAMPLE_RATE,
            channels: GLADIA_DEFAULT_CHANNELS,
            endpointing_seconds: 0.05,
            maximum_duration_without_endpointing_seconds: 5.0,
            languages: Vec::new(),
            code_switching: false,
            receive_partial_transcripts: true,
            receive_final_transcripts: true,
            receive_speech_events: true,
            receive_acknowledgments: true,
            receive_errors: true,
            receive_lifecycle_events: true,
        }
    }

    pub fn init_url(&self) -> Result<url::Url, url::ParseError> {
        let mut url = url::Url::parse(GLADIA_LIVE_INIT_ENDPOINT)?;
        if let Some(region) = self.region {
            url.query_pairs_mut()
                .append_pair("region", region.query_value());
        }
        Ok(url)
    }

    pub fn api_key_header_value(&self) -> String {
        self.api_key.trim().to_string()
    }

    pub fn init_body(&self) -> serde_json::Value {
        json!({
            "encoding": self.encoding,
            "bit_depth": self.bit_depth,
            "sample_rate": self.sample_rate,
            "channels": self.channels,
            "model": self.model,
            "endpointing": self.endpointing_seconds,
            "maximum_duration_without_endpointing": self.maximum_duration_without_endpointing_seconds,
            "language_config": {
                "languages": self.languages,
                "code_switching": self.code_switching,
            },
            "messages_config": {
                "receive_partial_transcripts": self.receive_partial_transcripts,
                "receive_final_transcripts": self.receive_final_transcripts,
                "receive_speech_events": self.receive_speech_events,
                "receive_acknowledgments": self.receive_acknowledgments,
                "receive_errors": self.receive_errors,
                "receive_lifecycle_events": self.receive_lifecycle_events,
            },
            "callback": false,
        })
    }
}

#[derive(Debug, Clone)]
pub struct GladiaParsedMessage {
    pub session_id: Option<String>,
    pub revisions: Vec<GladiaParsedRevision>,
    pub speech_event: Option<GladiaSpeechEvent>,
    pub acknowledgment: Option<GladiaAcknowledgment>,
    pub lifecycle_event: Option<String>,
    pub error: Option<GladiaProviderError>,
}

#[derive(Debug, Clone)]
pub struct GladiaParsedRevision {
    pub payload: AsrSpanRevisionPayload,
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GladiaSpeechEvent {
    pub event_type: GladiaSpeechEventType,
    pub time: f64,
    pub channel: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GladiaSpeechEventType {
    Start,
    End,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GladiaAcknowledgment {
    pub message_type: String,
    pub acknowledged: bool,
    pub byte_range: Option<(u64, u64)>,
    pub time_range: Option<(f64, f64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GladiaProviderError {
    pub message_type: String,
    pub code: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GladiaParseError {
    InvalidJson(String),
    UnsupportedMessageType(String),
}

#[derive(Debug)]
pub struct GladiaLiveParser {
    source_id: String,
    response_sequence: u64,
    fallback_utterance_index: u64,
    active_utterances: HashMap<String, GladiaActiveUtterance>,
}

#[derive(Debug, Clone)]
struct GladiaActiveUtterance {
    span_id: String,
    provider_item_id: String,
    revision_number: u64,
}

#[derive(Debug, Deserialize)]
struct GladiaResponse {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(rename = "type")]
    message_type: String,
    #[serde(default)]
    acknowledged: Option<bool>,
    #[serde(default)]
    error: Option<GladiaErrorPayload>,
    #[serde(default)]
    data: Option<GladiaData>,
}

#[derive(Debug, Clone, Deserialize)]
struct GladiaErrorPayload {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GladiaData {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    is_final: Option<bool>,
    #[serde(default)]
    utterance: Option<GladiaUtterance>,
    #[serde(default)]
    time: Option<f64>,
    #[serde(default)]
    channel: Option<serde_json::Value>,
    #[serde(default)]
    byte_range: Option<[u64; 2]>,
    #[serde(default)]
    time_range: Option<[f64; 2]>,
}

#[derive(Debug, Clone, Deserialize)]
struct GladiaUtterance {
    #[serde(default)]
    start: Option<f64>,
    #[serde(default)]
    end: Option<f64>,
    #[serde(default)]
    confidence: Option<f32>,
    #[serde(default)]
    channel: Option<serde_json::Value>,
    #[serde(default)]
    words: Vec<GladiaWord>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    speaker: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
struct GladiaWord {
    word: String,
    #[serde(default)]
    start: Option<f64>,
    #[serde(default)]
    end: Option<f64>,
    #[serde(default)]
    confidence: Option<f32>,
}

impl GladiaLiveParser {
    pub fn new(source_id: impl Into<String>) -> Self {
        Self {
            source_id: source_id.into(),
            response_sequence: 0,
            fallback_utterance_index: 0,
            active_utterances: HashMap::new(),
        }
    }

    pub fn parse_message(
        &mut self,
        text: &str,
        received_at_ms: u64,
    ) -> Result<GladiaParsedMessage, GladiaParseError> {
        let response: GladiaResponse = serde_json::from_str(text)
            .map_err(|error| GladiaParseError::InvalidJson(error.to_string()))?;
        self.response_sequence += 1;

        let error = response.error.as_ref().map(|error| GladiaProviderError {
            message_type: response.message_type.clone(),
            code: error.code.clone(),
            message: error.message.clone(),
        });

        match response.message_type.as_str() {
            "transcript" => Ok(GladiaParsedMessage {
                session_id: response.session_id.clone(),
                revisions: self
                    .emit_transcript_revision(&response, received_at_ms)
                    .into_iter()
                    .collect(),
                speech_event: None,
                acknowledgment: None,
                lifecycle_event: None,
                error,
            }),
            "speech_start" | "speech_end" => Ok(GladiaParsedMessage {
                session_id: response.session_id.clone(),
                revisions: Vec::new(),
                speech_event: speech_event(&response),
                acknowledgment: None,
                lifecycle_event: None,
                error,
            }),
            "audio_chunk" | "stop_recording" => Ok(GladiaParsedMessage {
                session_id: response.session_id.clone(),
                revisions: Vec::new(),
                speech_event: None,
                acknowledgment: acknowledgment(&response),
                lifecycle_event: None,
                error,
            }),
            "start_session" | "start_recording" | "end_recording" | "end_session" => {
                Ok(GladiaParsedMessage {
                    session_id: response.session_id.clone(),
                    revisions: Vec::new(),
                    speech_event: None,
                    acknowledgment: None,
                    lifecycle_event: Some(response.message_type),
                    error,
                })
            }
            "translation"
            | "named_entity_recognition"
            | "sentiment_analysis"
            | "post_transcript"
            | "post_final_transcript"
            | "post_chapterization"
            | "post_summarization" => Ok(GladiaParsedMessage {
                session_id: response.session_id.clone(),
                revisions: Vec::new(),
                speech_event: None,
                acknowledgment: None,
                lifecycle_event: None,
                error,
            }),
            other => Err(GladiaParseError::UnsupportedMessageType(other.to_string())),
        }
    }

    fn emit_transcript_revision(
        &mut self,
        response: &GladiaResponse,
        received_at_ms: u64,
    ) -> Option<GladiaParsedRevision> {
        let data = response.data.as_ref()?;
        let utterance = data.utterance.as_ref()?;
        let text = transcript_text(utterance);
        if text.trim().is_empty() {
            return None;
        }

        let provider_item_id = data
            .id
            .clone()
            .filter(|id| !id.trim().is_empty())
            .unwrap_or_else(|| self.next_fallback_utterance_id());
        let is_final = data.is_final.unwrap_or(false);
        let source_id = self.source_id.clone();
        let response_sequence = self.response_sequence;
        let active = self.ensure_active_utterance(&provider_item_id);
        active.revision_number += 1;
        let revision_number = active.revision_number;
        let supersedes =
            (revision_number > 1).then(|| revision_ref(&active.span_id, revision_number - 1));
        let speaker = utterance.speaker.as_ref().map(json_value_to_string);

        let payload = AsrSpanRevisionPayload {
            span_id: active.span_id.clone(),
            provider: PROVIDER.to_string(),
            source_id,
            provider_item_id: Some(active.provider_item_id.clone()),
            transcript_segment_id: is_final.then(|| format!("{}@final", active.span_id.as_str())),
            speaker_id: speaker.clone(),
            speaker_label: speaker.as_ref().map(|speaker| format!("Speaker {speaker}")),
            channel: utterance
                .channel
                .as_ref()
                .or(data.channel.as_ref())
                .map(json_value_to_string),
            text,
            start_time: utterance_start_time(utterance),
            end_time: utterance_end_time(utterance),
            confidence: utterance_confidence(utterance),
            is_final,
            stability: if is_final {
                AsrSpanStability::Final
            } else {
                AsrSpanStability::Partial
            },
            revision_number,
            supersedes,
            turn_id: Some(active.provider_item_id.clone()),
            end_of_turn: is_final,
            raw_event_ref: Some(format!("gladia.response.{response_sequence}")),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms,
        };

        if is_final {
            self.active_utterances.remove(&provider_item_id);
        }

        Some(GladiaParsedRevision {
            payload,
            language: utterance.language.clone(),
        })
    }

    fn ensure_active_utterance(&mut self, provider_item_id: &str) -> &mut GladiaActiveUtterance {
        let source_id = self.source_id.clone();
        self.active_utterances
            .entry(provider_item_id.to_string())
            .or_insert_with(|| GladiaActiveUtterance {
                span_id: gladia_span_id(&source_id, provider_item_id),
                provider_item_id: provider_item_id.to_string(),
                revision_number: 0,
            })
    }

    fn next_fallback_utterance_id(&mut self) -> String {
        self.fallback_utterance_index += 1;
        format!("utterance-{}", self.fallback_utterance_index)
    }
}

fn transcript_text(utterance: &GladiaUtterance) -> String {
    utterance
        .text
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            utterance
                .words
                .iter()
                .map(|word| word.word.as_str())
                .collect::<String>()
                .trim()
                .to_string()
        })
}

fn utterance_start_time(utterance: &GladiaUtterance) -> f64 {
    sanitized_seconds(
        utterance
            .start
            .or_else(|| {
                utterance
                    .words
                    .iter()
                    .filter_map(|word| word.start)
                    .reduce(f64::min)
            })
            .unwrap_or(0.0),
    )
}

fn utterance_end_time(utterance: &GladiaUtterance) -> f64 {
    let start_time = utterance_start_time(utterance);
    sanitized_seconds(
        utterance
            .end
            .or_else(|| {
                utterance
                    .words
                    .iter()
                    .filter_map(|word| word.end)
                    .reduce(f64::max)
            })
            .unwrap_or(start_time),
    )
    .max(start_time)
}

fn utterance_confidence(utterance: &GladiaUtterance) -> f32 {
    if let Some(confidence) = utterance
        .confidence
        .filter(|confidence| confidence.is_finite())
    {
        return confidence.clamp(0.0, 1.0);
    }

    let mut total = 0.0;
    let mut count = 0usize;
    for confidence in utterance
        .words
        .iter()
        .filter_map(|word| word.confidence)
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

fn speech_event(response: &GladiaResponse) -> Option<GladiaSpeechEvent> {
    let data = response.data.as_ref()?;
    let event_type = match response.message_type.as_str() {
        "speech_start" => GladiaSpeechEventType::Start,
        "speech_end" => GladiaSpeechEventType::End,
        _ => return None,
    };
    Some(GladiaSpeechEvent {
        event_type,
        time: sanitized_seconds(data.time.unwrap_or(0.0)),
        channel: data.channel.as_ref().map(json_value_to_string),
    })
}

fn acknowledgment(response: &GladiaResponse) -> Option<GladiaAcknowledgment> {
    Some(GladiaAcknowledgment {
        message_type: response.message_type.clone(),
        acknowledged: response.acknowledged?,
        byte_range: response
            .data
            .as_ref()
            .and_then(|data| data.byte_range.map(|range| (range[0], range[1]))),
        time_range: response.data.as_ref().and_then(|data| {
            data.time_range
                .map(|range| (sanitized_seconds(range[0]), sanitized_seconds(range[1])))
        }),
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

fn gladia_span_id(source_id: &str, provider_item_id: &str) -> String {
    format!("{PROVIDER}:{source_id}:{provider_item_id}")
}

fn json_value_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(value) => value.clone(),
        _ => value.to_string(),
    }
}

fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);

    for chunk in bytes.chunks(3) {
        let first = chunk[0];
        let second = *chunk.get(1).unwrap_or(&0);
        let third = *chunk.get(2).unwrap_or(&0);
        let packed = ((first as u32) << 16) | ((second as u32) << 8) | third as u32;

        output.push(ALPHABET[((packed >> 18) & 0x3f) as usize] as char);
        output.push(ALPHABET[((packed >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            output.push(ALPHABET[((packed >> 6) & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(ALPHABET[(packed & 0x3f) as usize] as char);
        } else {
            output.push('=');
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projections::{TranscriptEvent, TranscriptLedger};
    use std::collections::HashMap;

    #[test]
    fn config_builds_init_request_with_secret_redacted() {
        let mut config = GladiaLiveConfig::solaria_pcm16_16k("  gladia-secret  ");
        config.region = Some(GladiaRegion::EuWest);
        config.languages = vec!["en".to_string()];

        let url = config.init_url().expect("init url");
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        let body = config.init_body();

        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("api.gladia.io"));
        assert_eq!(url.path(), "/v2/live");
        assert_eq!(query.get("region").map(String::as_str), Some("eu-west"));
        assert_eq!(config.api_key_header_value(), "gladia-secret");
        assert_eq!(body["model"], GLADIA_DEFAULT_MODEL);
        assert_eq!(body["encoding"], GLADIA_PCM_ENCODING);
        assert_eq!(body["bit_depth"], GLADIA_DEFAULT_BIT_DEPTH);
        assert_eq!(body["sample_rate"], GLADIA_DEFAULT_SAMPLE_RATE);
        assert_eq!(body["channels"], GLADIA_DEFAULT_CHANNELS);
        assert_eq!(body["language_config"]["languages"][0], "en");
        assert_eq!(body["messages_config"]["receive_partial_transcripts"], true);
        assert_eq!(body["messages_config"]["receive_final_transcripts"], true);
        assert_eq!(body["messages_config"]["receive_speech_events"], true);

        let debug = format!("{config:?}");
        assert!(debug.contains("<present>"));
        assert!(!debug.contains("gladia-secret"));
    }

    #[test]
    fn partial_and_final_share_provider_id_and_replay_without_duplicates() {
        let mut parser = GladiaLiveParser::new("mic-1");

        let partial = parser
            .parse_message(
                r#"{
                    "session_id": "session-123",
                    "created_at": "2025-09-19T12:34:10Z",
                    "type": "transcript",
                    "data": {
                        "id": "00-00000011",
                        "is_final": false,
                        "utterance": {
                            "start": 0,
                            "end": 0.35,
                            "confidence": 0.4,
                            "channel": 0,
                            "words": [
                                { "word": "Hello", "start": 0, "end": 0.35, "confidence": 0.4 }
                            ],
                            "text": "Hello",
                            "language": "en",
                            "speaker": 0
                        }
                    }
                }"#,
                1_700_000_000_001,
            )
            .unwrap();
        let final_message = parser
            .parse_message(
                r#"{
                    "session_id": "session-123",
                    "created_at": "2025-09-19T12:34:11Z",
                    "type": "transcript",
                    "data": {
                        "id": "00-00000011",
                        "is_final": true,
                        "utterance": {
                            "start": 0,
                            "end": 0.48,
                            "confidence": 0.91,
                            "channel": 0,
                            "words": [
                                { "word": "Hello", "start": 0, "end": 0.35, "confidence": 0.91 },
                                { "word": " world.", "start": 0.36, "end": 0.48, "confidence": 0.91 }
                            ],
                            "text": "Hello world.",
                            "language": "en",
                            "speaker": 0
                        }
                    }
                }"#,
                1_700_000_000_002,
            )
            .unwrap();

        assert_eq!(partial.session_id.as_deref(), Some("session-123"));
        assert_eq!(partial.revisions.len(), 1);
        assert_eq!(final_message.revisions.len(), 1);

        let partial_revision = &partial.revisions[0].payload;
        let final_revision = &final_message.revisions[0].payload;
        let span_id = "gladia:mic-1:00-00000011";

        assert_eq!(partial_revision.span_id, span_id);
        assert_eq!(
            partial_revision.provider_item_id.as_deref(),
            Some("00-00000011")
        );
        assert_eq!(partial_revision.text, "Hello");
        assert_eq!(partial_revision.revision_number, 1);
        assert!(!partial_revision.is_final);
        assert_eq!(partial_revision.channel.as_deref(), Some("0"));
        assert_eq!(partial_revision.speaker_id.as_deref(), Some("0"));
        assert_eq!(partial.revisions[0].language.as_deref(), Some("en"));

        assert_eq!(final_revision.span_id, span_id);
        assert_eq!(final_revision.text, "Hello world.");
        assert_eq!(final_revision.revision_number, 2);
        assert_eq!(
            final_revision.supersedes.as_deref(),
            Some("gladia:mic-1:00-00000011@rev1")
        );
        assert!(final_revision.is_final);
        assert!(final_revision.end_of_turn);
        assert_eq!(
            final_revision.transcript_segment_id.as_deref(),
            Some("gladia:mic-1:00-00000011@final")
        );

        let ledger = TranscriptLedger::replay(
            "session-gladia",
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
        assert!(ledger.latest_spans[0].is_final);
    }

    #[test]
    fn next_utterance_after_final_starts_new_span() {
        let mut parser = GladiaLiveParser::new("mic-1");
        let final_message = r#"{
            "type": "transcript",
            "data": {
                "id": "utt-1",
                "is_final": true,
                "utterance": {
                    "start": 0,
                    "end": 0.4,
                    "words": [],
                    "text": "Done"
                }
            }
        }"#;
        let next_partial = r#"{
            "type": "transcript",
            "data": {
                "id": "utt-2",
                "is_final": false,
                "utterance": {
                    "start": 1,
                    "end": 1.4,
                    "words": [],
                    "text": "Next"
                }
            }
        }"#;

        let first = parser.parse_message(final_message, 1).unwrap();
        let second = parser.parse_message(next_partial, 2).unwrap();

        assert_eq!(first.revisions[0].payload.revision_number, 1);
        assert!(first.revisions[0].payload.is_final);
        assert_eq!(second.revisions[0].payload.revision_number, 1);
        assert_eq!(second.revisions[0].payload.supersedes, None);
        assert_eq!(second.revisions[0].payload.span_id, "gladia:mic-1:utt-2");
    }

    #[test]
    fn audio_frame_helpers_support_binary_primary_and_json_base64_fallback() {
        assert_eq!(
            GladiaAudioFrame::binary([0x01, 0x02, 0x03]),
            GladiaAudioFrame::Binary(vec![0x01, 0x02, 0x03])
        );
        assert_eq!(
            GladiaAudioFrame::json_base64(&[0x01, 0x02, 0x03]),
            GladiaAudioFrame::JsonBase64(json!({
                "type": "audio_chunk",
                "data": { "chunk": "AQID" },
            }))
        );
        assert_eq!(
            GladiaAudioFrame::json_base64(&[0xff]),
            GladiaAudioFrame::JsonBase64(json!({
                "type": "audio_chunk",
                "data": { "chunk": "/w==" },
            }))
        );
    }

    #[test]
    fn speech_events_acknowledgments_and_lifecycle_do_not_emit_transcripts() {
        let mut parser = GladiaLiveParser::new("system");

        let speech_start = parser
            .parse_message(
                r#"{
                    "session_id": "session-1",
                    "type": "speech_start",
                    "data": { "time": 1.24, "channel": 0 }
                }"#,
                10,
            )
            .unwrap();
        let ack = parser
            .parse_message(
                r#"{
                    "session_id": "session-1",
                    "type": "audio_chunk",
                    "acknowledged": true,
                    "error": null,
                    "data": {
                        "byte_range": [0, 4095],
                        "time_range": [0, 0.256]
                    }
                }"#,
                11,
            )
            .unwrap();
        let lifecycle = parser
            .parse_message(
                r#"{ "session_id": "session-1", "type": "start_session" }"#,
                12,
            )
            .unwrap();

        assert!(speech_start.revisions.is_empty());
        assert_eq!(
            speech_start.speech_event,
            Some(GladiaSpeechEvent {
                event_type: GladiaSpeechEventType::Start,
                time: 1.24,
                channel: Some("0".to_string()),
            })
        );
        assert!(ack.revisions.is_empty());
        assert_eq!(
            ack.acknowledgment,
            Some(GladiaAcknowledgment {
                message_type: "audio_chunk".to_string(),
                acknowledged: true,
                byte_range: Some((0, 4095)),
                time_range: Some((0.0, 0.256)),
            })
        );
        assert!(lifecycle.revisions.is_empty());
        assert_eq!(lifecycle.lifecycle_event.as_deref(), Some("start_session"));
    }

    #[test]
    fn provider_error_is_preserved_without_transcript_revision() {
        let mut parser = GladiaLiveParser::new("mic-1");

        let parsed = parser
            .parse_message(
                r#"{
                    "session_id": "session-1",
                    "type": "audio_chunk",
                    "acknowledged": false,
                    "error": {
                        "code": "invalid_audio",
                        "message": "Audio chunk cannot be decoded"
                    },
                    "data": {}
                }"#,
                20,
            )
            .unwrap();

        assert!(parsed.revisions.is_empty());
        assert_eq!(
            parsed.error,
            Some(GladiaProviderError {
                message_type: "audio_chunk".to_string(),
                code: Some("invalid_audio".to_string()),
                message: Some("Audio chunk cannot be decoded".to_string()),
            })
        );
    }

    #[test]
    fn empty_transcript_and_unknown_message_do_not_create_spans() {
        let mut parser = GladiaLiveParser::new("mic-1");

        let empty = parser
            .parse_message(
                r#"{
                    "type": "transcript",
                    "data": {
                        "id": "empty-1",
                        "is_final": false,
                        "utterance": { "text": "   ", "words": [] }
                    }
                }"#,
                1,
            )
            .unwrap();

        assert!(empty.revisions.is_empty());
        assert!(matches!(
            parser.parse_message(r#"{ "type": "future_event" }"#, 2),
            Err(GladiaParseError::UnsupportedMessageType(message)) if message == "future_event"
        ));
        assert!(matches!(
            parser.parse_message("{", 3),
            Err(GladiaParseError::InvalidJson(_))
        ));
    }
}
