//! Rust-owned Speech Span Revision v2 wire contract.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const SPEECH_SPAN_CONTRACT_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpeechSpanStability {
    Partial,
    Final,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SpeechTimingPrecision {
    Coarse,
    Exact,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum SpeechTiming {
    Unavailable,
    AppEstimated {
        start_time: f64,
        end_time: f64,
    },
    Provider {
        precision: SpeechTimingPrecision,
        start_time: f64,
        end_time: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum SpeechConfidence {
    Unavailable,
    Provider { value: f32 },
    App { value: f32 },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "origin", rename_all = "snake_case")]
pub enum SpeechAttribute<T> {
    Unavailable,
    Provider { value: T },
    App { value: T },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SpeechTurnValue {
    pub turn_id: String,
    pub end_of_turn: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SpeechSpeakerValue {
    pub speaker_id: Option<String>,
    pub speaker_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SpeechSpanSourceOrder {
    pub source_stream_id: String,
    pub ordinal: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub struct SpeechSpanRevisionRef {
    pub span_id: String,
    pub revision_number: u64,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
struct SpeechSpanRevisionWire {
    contract_version: u32,
    span_id: String,
    source_order: SpeechSpanSourceOrder,
    provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_item_id: Option<String>,
    text: String,
    stability: SpeechSpanStability,
    revision_number: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    supersedes: Option<SpeechSpanRevisionRef>,
    timing: SpeechTiming,
    confidence: SpeechConfidence,
    turn: SpeechAttribute<SpeechTurnValue>,
    speaker: SpeechAttribute<SpeechSpeakerValue>,
    channel: SpeechAttribute<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provider_event_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asr_latency_ms: Option<u64>,
    received_at_ms: u64,
}

#[derive(Clone, Serialize, JsonSchema, PartialEq)]
#[serde(transparent)]
#[schemars(transparent)]
pub struct SpeechSpanRevision(SpeechSpanRevisionWire);

impl<'de> Deserialize<'de> for SpeechSpanRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let wire = SpeechSpanRevisionWire::deserialize(deserializer)?;
        validate_wire(&wire).map_err(|_| D::Error::custom("invalid speech span revision v2"))?;
        Ok(Self(wire))
    }
}

pub struct SpeechSpanRevisionParts {
    pub span_id: String,
    pub source_order: SpeechSpanSourceOrder,
    pub provider: String,
    pub provider_item_id: Option<String>,
    pub text: String,
    pub stability: SpeechSpanStability,
    pub revision_number: u64,
    pub supersedes: Option<SpeechSpanRevisionRef>,
    pub timing: SpeechTiming,
    pub confidence: SpeechConfidence,
    pub turn: SpeechAttribute<SpeechTurnValue>,
    pub speaker: SpeechAttribute<SpeechSpeakerValue>,
    pub channel: SpeechAttribute<String>,
    pub provider_event_ref: Option<String>,
    pub capture_latency_ms: Option<u64>,
    pub asr_latency_ms: Option<u64>,
    pub received_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechSpanContractError;

impl fmt::Display for SpeechSpanContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid speech span revision v2")
    }
}

impl std::error::Error for SpeechSpanContractError {}

impl SpeechSpanRevision {
    pub fn try_from_parts(parts: SpeechSpanRevisionParts) -> Result<Self, SpeechSpanContractError> {
        let wire = SpeechSpanRevisionWire {
            contract_version: SPEECH_SPAN_CONTRACT_VERSION,
            span_id: parts.span_id,
            source_order: parts.source_order,
            provider: parts.provider,
            provider_item_id: parts.provider_item_id,
            text: parts.text,
            stability: parts.stability,
            revision_number: parts.revision_number,
            supersedes: parts.supersedes,
            timing: parts.timing,
            confidence: parts.confidence,
            turn: parts.turn,
            speaker: parts.speaker,
            channel: parts.channel,
            provider_event_ref: parts.provider_event_ref,
            capture_latency_ms: parts.capture_latency_ms,
            asr_latency_ms: parts.asr_latency_ms,
            received_at_ms: parts.received_at_ms,
        };
        validate_wire(&wire)?;
        Ok(Self(wire))
    }

    pub fn span_id(&self) -> &str {
        &self.0.span_id
    }

    pub fn source_order(&self) -> &SpeechSpanSourceOrder {
        &self.0.source_order
    }

    pub fn provider(&self) -> &str {
        &self.0.provider
    }

    pub fn revision_number(&self) -> u64 {
        self.0.revision_number
    }

    pub fn supersedes(&self) -> Option<&SpeechSpanRevisionRef> {
        self.0.supersedes.as_ref()
    }

    pub fn revision_ref(&self) -> SpeechSpanRevisionRef {
        SpeechSpanRevisionRef {
            span_id: self.0.span_id.clone(),
            revision_number: self.0.revision_number,
        }
    }
}

impl fmt::Debug for SpeechSpanRevision {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SpeechSpanRevision")
            .field("contract_version", &self.0.contract_version)
            .field("span_id", &self.0.span_id)
            .field("source_order", &self.0.source_order)
            .field("provider", &self.0.provider)
            .field("text", &"<redacted>")
            .field("stability", &self.0.stability)
            .field("revision_number", &self.0.revision_number)
            .field("supersedes", &self.0.supersedes)
            .field("timing", &"<redacted>")
            .field("confidence", &"<redacted>")
            .field("turn", &"<redacted>")
            .field("speaker", &"<redacted>")
            .field("channel", &"<redacted>")
            .field("received_at_ms", &self.0.received_at_ms)
            .finish()
    }
}

fn validate_wire(wire: &SpeechSpanRevisionWire) -> Result<(), SpeechSpanContractError> {
    if wire.contract_version != SPEECH_SPAN_CONTRACT_VERSION
        || wire.span_id.trim().is_empty()
        || wire.source_order.source_stream_id.trim().is_empty()
        || wire.source_order.ordinal == 0
        || wire.provider.trim().is_empty()
        || wire.text.trim().is_empty()
        || wire.revision_number == 0
        || wire
            .provider_item_id
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        || wire
            .provider_event_ref
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        || !valid_timing(&wire.timing)
        || !valid_confidence(&wire.confidence)
        || !valid_turn(&wire.turn)
        || !valid_speaker(&wire.speaker)
        || !valid_channel(&wire.channel)
    {
        return Err(SpeechSpanContractError);
    }
    match (wire.revision_number, &wire.supersedes) {
        (1, None) => Ok(()),
        (revision, Some(previous))
            if revision > 1
                && previous.span_id == wire.span_id
                && previous.revision_number.checked_add(1) == Some(revision) =>
        {
            Ok(())
        }
        _ => Err(SpeechSpanContractError),
    }
}

fn valid_timing(timing: &SpeechTiming) -> bool {
    match timing {
        SpeechTiming::Unavailable => true,
        SpeechTiming::AppEstimated {
            start_time,
            end_time,
        }
        | SpeechTiming::Provider {
            start_time,
            end_time,
            ..
        } => {
            start_time.is_finite()
                && end_time.is_finite()
                && *start_time >= 0.0
                && *end_time >= 0.0
                && start_time <= end_time
        }
    }
}

fn valid_confidence(confidence: &SpeechConfidence) -> bool {
    match confidence {
        SpeechConfidence::Unavailable => true,
        SpeechConfidence::Provider { value } | SpeechConfidence::App { value } => {
            value.is_finite() && (0.0..=1.0).contains(value)
        }
    }
}

fn valid_turn(turn: &SpeechAttribute<SpeechTurnValue>) -> bool {
    match turn {
        SpeechAttribute::Unavailable => true,
        SpeechAttribute::Provider { value } | SpeechAttribute::App { value } => {
            !value.turn_id.trim().is_empty()
        }
    }
}

fn valid_speaker(speaker: &SpeechAttribute<SpeechSpeakerValue>) -> bool {
    match speaker {
        SpeechAttribute::Unavailable => true,
        SpeechAttribute::Provider { value } | SpeechAttribute::App { value } => {
            let id_valid = value
                .speaker_id
                .as_deref()
                .is_none_or(|speaker_id| !speaker_id.trim().is_empty());
            let label_valid = value
                .speaker_label
                .as_deref()
                .is_none_or(|speaker_label| !speaker_label.trim().is_empty());
            id_valid && label_valid && (value.speaker_id.is_some() || value.speaker_label.is_some())
        }
    }
}

fn valid_channel(channel: &SpeechAttribute<String>) -> bool {
    match channel {
        SpeechAttribute::Unavailable => true,
        SpeechAttribute::Provider { value } | SpeechAttribute::App { value } => {
            !value.trim().is_empty()
        }
    }
}

pub fn speech_span_revision_schema_json() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(SpeechSpanRevision))
        .expect("SpeechSpanRevision schema should serialize")
}

pub fn speech_span_revision_typescript_module() -> String {
    let schema = serde_json::to_string_pretty(&speech_span_revision_schema_json())
        .expect("SpeechSpanRevision schema should serialize");
    let schema_literal = crate::js_single_quoted_string_literal(&schema);
    format!(
        r#"// @generated by src-tauri/crates/ipc-contract/src/speech_span_revision.rs. Do not edit manually.

export const SPEECH_SPAN_CONTRACT_VERSION = 2 as const;

export type SpeechSpanStability = "partial" | "final";
export type SpeechTimingPrecision = "coarse" | "exact";

export type SpeechTiming =
  | {{ origin: "unavailable" }}
  | {{ origin: "app_estimated"; start_time: number; end_time: number }}
  | {{
      origin: "provider";
      precision: SpeechTimingPrecision;
      start_time: number;
      end_time: number;
    }};

export type SpeechConfidence =
  | {{ origin: "unavailable" }}
  | {{ origin: "provider"; value: number }}
  | {{ origin: "app"; value: number }};

export type SpeechAttribute<T> =
  | {{ origin: "unavailable" }}
  | {{ origin: "provider"; value: T }}
  | {{ origin: "app"; value: T }};

export interface SpeechTurnValue {{
  turn_id: string;
  end_of_turn: boolean;
}}

export interface SpeechSpeakerValue {{
  speaker_id: string | null;
  speaker_label: string | null;
}}

export interface SpeechSpanSourceOrder {{
  source_stream_id: string;
  ordinal: number;
}}

export interface SpeechSpanRevisionRef {{
  span_id: string;
  revision_number: number;
}}

export interface SpeechSpanRevision {{
  contract_version: typeof SPEECH_SPAN_CONTRACT_VERSION;
  span_id: string;
  source_order: SpeechSpanSourceOrder;
  provider: string;
  provider_item_id?: string | null;
  text: string;
  stability: SpeechSpanStability;
  revision_number: number;
  supersedes?: SpeechSpanRevisionRef | null;
  timing: SpeechTiming;
  confidence: SpeechConfidence;
  turn: SpeechAttribute<SpeechTurnValue>;
  speaker: SpeechAttribute<SpeechSpeakerValue>;
  channel: SpeechAttribute<string>;
  provider_event_ref?: string | null;
  capture_latency_ms?: number | null;
  asr_latency_ms?: number | null;
  received_at_ms: number;
}}

export const SPEECH_SPAN_REVISION_SCHEMA_JSON =
  {schema_literal};

export const SPEECH_SPAN_REVISION_SCHEMA = JSON.parse(
  SPEECH_SPAN_REVISION_SCHEMA_JSON,
) as Record<string, unknown>;
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_module_contains_version_and_nested_fidelity_contract() {
        let generated = speech_span_revision_typescript_module();
        for required in [
            "SPEECH_SPAN_CONTRACT_VERSION = 2",
            "origin: \"app_estimated\"",
            "SpeechAttribute<SpeechSpeakerValue>",
            "SPEECH_SPAN_REVISION_SCHEMA_JSON",
        ] {
            assert!(generated.contains(required));
        }
    }
}
