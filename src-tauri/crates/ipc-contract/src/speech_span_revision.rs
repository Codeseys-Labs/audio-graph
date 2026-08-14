//! Rust-owned Speech Span Revision v2 wire contract.

use std::collections::HashMap;
use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

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
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpeechTiming {
    Unavailable {},
    AppEstimated {
        #[schemars(range(min = 0.0))]
        start_time: f64,
        #[schemars(range(min = 0.0))]
        end_time: f64,
    },
    Provider {
        precision: SpeechTimingPrecision,
        #[schemars(range(min = 0.0))]
        start_time: f64,
        #[schemars(range(min = 0.0))]
        end_time: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpeechConfidence {
    Unavailable {},
    Provider {
        #[schemars(range(min = 0.0, max = 1.0))]
        value: f32,
    },
    App {
        #[schemars(range(min = 0.0, max = 1.0))]
        value: f32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpeechTurnFidelity {
    Unavailable {},
    Provider { value: SpeechTurnValue },
    App { value: SpeechTurnValue },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpeechSpeakerFidelity {
    Unavailable {},
    Provider { value: SpeechSpeakerValue },
    App { value: SpeechSpeakerValue },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "origin", rename_all = "snake_case", deny_unknown_fields)]
pub enum SpeechChannelFidelity {
    Unavailable {},
    Provider {
        #[schemars(length(min = 1))]
        value: String,
    },
    App {
        #[schemars(length(min = 1))]
        value: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SpeechTurnValue {
    #[schemars(length(min = 1))]
    pub turn_id: String,
    pub end_of_turn: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[schemars(extend("anyOf" = [
    {"required": ["speaker_id"], "properties": {"speaker_id": {"type": "string", "minLength": 1}}},
    {"required": ["speaker_label"], "properties": {"speaker_label": {"type": "string", "minLength": 1}}}
]))]
pub struct SpeechSpeakerValue {
    #[schemars(extend("anyOf" = [
        {"type": "string", "minLength": 1},
        {"type": "null"}
    ]))]
    pub speaker_id: Option<String>,
    #[schemars(extend("anyOf" = [
        {"type": "string", "minLength": 1},
        {"type": "null"}
    ]))]
    pub speaker_label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SpeechSpanSourceOrder {
    #[schemars(length(min = 1))]
    pub source_stream_id: String,
    #[schemars(range(min = 1))]
    pub ordinal: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SpeechSpanRevisionRef {
    #[schemars(regex(pattern = r"^ssp_[0-9a-f]{32}$"))]
    pub span_id: String,
    #[schemars(range(min = 1))]
    pub revision_number: u64,
}

/// One provider/local recognition observation admitted by the normalizer.
#[derive(Clone, PartialEq)]
pub struct SpanObservation {
    pub source_stream_id: String,
    pub provider: String,
    pub provider_item_id: Option<String>,
    pub correlation: Option<SpeechSpanRevisionRef>,
    pub text: String,
    pub stability: SpeechSpanStability,
    pub timing: SpeechTiming,
    pub confidence: SpeechConfidence,
    pub turn: SpeechTurnFidelity,
    pub speaker: SpeechSpeakerFidelity,
    pub channel: SpeechChannelFidelity,
    pub provider_event_ref: Option<String>,
    pub capture_latency_ms: Option<u64>,
    pub asr_latency_ms: Option<u64>,
    pub received_at_ms: u64,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
struct SpeechSpanRevisionWire {
    #[schemars(range(min = 2, max = 2), extend("const" = 2))]
    contract_version: u32,
    #[schemars(regex(pattern = r"^ssp_[0-9a-f]{32}$"))]
    span_id: String,
    source_order: SpeechSpanSourceOrder,
    #[schemars(length(min = 1))]
    provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("anyOf" = [
        {"type": "string", "minLength": 1},
        {"type": "null"}
    ]))]
    provider_item_id: Option<String>,
    #[schemars(length(min = 1))]
    text: String,
    stability: SpeechSpanStability,
    #[schemars(range(min = 1))]
    revision_number: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    supersedes: Option<SpeechSpanRevisionRef>,
    timing: SpeechTiming,
    confidence: SpeechConfidence,
    turn: SpeechTurnFidelity,
    speaker: SpeechSpeakerFidelity,
    channel: SpeechChannelFidelity,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(extend("anyOf" = [
        {"type": "string", "minLength": 1},
        {"type": "null"}
    ]))]
    provider_event_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    capture_latency_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    asr_latency_ms: Option<u64>,
    received_at_ms: u64,
}

/// A validated, read-only Speech Span Revision v2 wire value.
///
/// Raw construction is intentionally not part of the public interface. New
/// revisions must cross [`SpeechSpanRevisionNormalizer::admit`].
///
/// ```compile_fail
/// use audio_graph_ipc_contract::speech_span_revision::SpeechSpanRevision;
///
/// let _raw_constructor = SpeechSpanRevision::try_from_parts;
/// ```
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

struct SpeechSpanRevisionParts {
    span_id: String,
    source_order: SpeechSpanSourceOrder,
    provider: String,
    provider_item_id: Option<String>,
    text: String,
    stability: SpeechSpanStability,
    revision_number: u64,
    supersedes: Option<SpeechSpanRevisionRef>,
    timing: SpeechTiming,
    confidence: SpeechConfidence,
    turn: SpeechTurnFidelity,
    speaker: SpeechSpeakerFidelity,
    channel: SpeechChannelFidelity,
    provider_event_ref: Option<String>,
    capture_latency_ms: Option<u64>,
    asr_latency_ms: Option<u64>,
    received_at_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechSpanContractError;

impl fmt::Display for SpeechSpanContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("invalid speech span revision v2")
    }
}

impl std::error::Error for SpeechSpanContractError {}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpeechSpanRevisionError {
    #[error("speech span observation has an invalid source stream identifier")]
    InvalidSourceStreamId,
    #[error("speech span observation has an invalid provider identifier")]
    InvalidProviderId,
    #[error("speech span observation has an invalid provider item identifier")]
    InvalidProviderItemId,
    #[error("speech span observation has invalid transcript content")]
    InvalidText,
    #[error("speech span observation has an invalid provider event reference")]
    InvalidProviderEventRef,
    #[error("speech span observation has invalid timing evidence")]
    InvalidTiming,
    #[error("speech span observation has invalid confidence evidence")]
    InvalidConfidence,
    #[error("speech span observation has invalid turn evidence")]
    InvalidTurn,
    #[error("speech span observation has invalid speaker evidence")]
    InvalidSpeaker,
    #[error("speech span observation has invalid channel evidence")]
    InvalidChannel,
    #[error("speech span observation has an invalid revision correlation")]
    InvalidCorrelation,
    #[error("legacy speech span cannot be projected without fabricating evidence")]
    LegacyProjectionUnavailable,
    #[error("speech span revision contract validation failed")]
    InvalidContract,
    #[error("speech span revision number is exhausted")]
    RevisionExhausted,
    #[error("speech span source order is exhausted")]
    SourceOrderExhausted,
    #[error("speech span observation duplicates an admitted revision")]
    DuplicateObservation,
    #[error("speech span observation conflicts with an admitted revision")]
    ConflictingObservation,
    #[error("speech span observation correlates to a stale revision")]
    StaleCorrelation,
    #[error("speech span observation correlates to an unknown span")]
    UnknownCorrelation,
    #[error("speech span observation correlates ahead of the admitted revision")]
    FutureCorrelation,
    #[error("speech span observation correlation belongs to another source stream")]
    CorrelationSourceMismatch,
    #[error("speech span observation correlation belongs to another provider")]
    CorrelationProviderMismatch,
    #[error("speech span provider item hint belongs to another span")]
    ProviderItemCollision,
}

impl SpeechSpanRevision {
    fn try_from_parts(parts: SpeechSpanRevisionParts) -> Result<Self, SpeechSpanContractError> {
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

#[derive(Clone)]
struct AdmittedSpan {
    revision: SpeechSpanRevision,
    observation: SpanObservation,
}

/// Stateful deep module that owns Speech Span Revision identity and ordering.
#[derive(Default)]
pub struct SpeechSpanRevisionNormalizer {
    next_ordinal_by_source: HashMap<String, u64>,
    spans_by_id: HashMap<String, AdmittedSpan>,
    span_by_provider_item: HashMap<(String, String, String), String>,
}

impl SpeechSpanRevisionNormalizer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn admit(
        &mut self,
        observation: SpanObservation,
    ) -> Result<SpeechSpanRevision, SpeechSpanRevisionError> {
        validate_observation(&observation)?;

        if let Some(correlation) = observation.correlation.clone() {
            return self.admit_correlated(observation, correlation);
        }

        if let Some(provider_item_id) = observation.provider_item_id.as_ref() {
            let key = provider_item_key(&observation, provider_item_id);
            if let Some(span_id) = self.span_by_provider_item.get(&key) {
                let admitted = self
                    .spans_by_id
                    .get(span_id)
                    .expect("provider-item index must reference an admitted span");
                return if same_observation_content(&observation, &admitted.observation) {
                    Err(SpeechSpanRevisionError::DuplicateObservation)
                } else {
                    Err(SpeechSpanRevisionError::ConflictingObservation)
                };
            }
        }

        let ordinal = self
            .next_ordinal_by_source
            .get(&observation.source_stream_id)
            .copied()
            .unwrap_or(0)
            .checked_add(1)
            .ok_or(SpeechSpanRevisionError::SourceOrderExhausted)?;
        let source_order = SpeechSpanSourceOrder {
            source_stream_id: observation.source_stream_id.clone(),
            ordinal,
        };
        let revision = SpeechSpanRevision::try_from_parts(SpeechSpanRevisionParts {
            span_id: app_span_id(&source_order.source_stream_id, source_order.ordinal),
            source_order,
            provider: observation.provider.clone(),
            provider_item_id: observation.provider_item_id.clone(),
            text: observation.text.clone(),
            stability: observation.stability,
            revision_number: 1,
            supersedes: None,
            timing: observation.timing.clone(),
            confidence: observation.confidence.clone(),
            turn: observation.turn.clone(),
            speaker: observation.speaker.clone(),
            channel: observation.channel.clone(),
            provider_event_ref: observation.provider_event_ref.clone(),
            capture_latency_ms: observation.capture_latency_ms,
            asr_latency_ms: observation.asr_latency_ms,
            received_at_ms: observation.received_at_ms,
        })
        .map_err(|_| SpeechSpanRevisionError::InvalidContract)?;
        self.index_provider_item(&observation, &revision)?;
        self.next_ordinal_by_source
            .insert(observation.source_stream_id.clone(), ordinal);
        self.spans_by_id.insert(
            revision.span_id().to_string(),
            AdmittedSpan {
                revision: revision.clone(),
                observation,
            },
        );
        Ok(revision)
    }

    fn admit_correlated(
        &mut self,
        observation: SpanObservation,
        correlation: SpeechSpanRevisionRef,
    ) -> Result<SpeechSpanRevision, SpeechSpanRevisionError> {
        let current = self
            .spans_by_id
            .get(&correlation.span_id)
            .ok_or(SpeechSpanRevisionError::UnknownCorrelation)?;
        if observation.source_stream_id != current.revision.source_order().source_stream_id {
            return Err(SpeechSpanRevisionError::CorrelationSourceMismatch);
        }
        if observation.provider != current.revision.provider() {
            return Err(SpeechSpanRevisionError::CorrelationProviderMismatch);
        }
        if correlation.revision_number > current.revision.revision_number() {
            return Err(SpeechSpanRevisionError::FutureCorrelation);
        }
        if correlation.revision_number < current.revision.revision_number() {
            if same_observation_content(&observation, &current.observation) {
                return Err(SpeechSpanRevisionError::DuplicateObservation);
            }
            return if correlation.revision_number + 1 == current.revision.revision_number() {
                Err(SpeechSpanRevisionError::ConflictingObservation)
            } else {
                Err(SpeechSpanRevisionError::StaleCorrelation)
            };
        }
        if same_observation_content(&observation, &current.observation) {
            return Err(SpeechSpanRevisionError::DuplicateObservation);
        }

        let supersedes = current.revision.revision_ref();
        let source_order = current.revision.source_order().clone();
        let span_id = current.revision.span_id().to_string();
        let revision_number = current
            .revision
            .revision_number()
            .checked_add(1)
            .ok_or(SpeechSpanRevisionError::RevisionExhausted)?;
        let revision = SpeechSpanRevision::try_from_parts(SpeechSpanRevisionParts {
            span_id,
            source_order,
            provider: observation.provider.clone(),
            provider_item_id: observation.provider_item_id.clone(),
            text: observation.text.clone(),
            stability: observation.stability,
            revision_number,
            supersedes: Some(supersedes),
            timing: observation.timing.clone(),
            confidence: observation.confidence.clone(),
            turn: observation.turn.clone(),
            speaker: observation.speaker.clone(),
            channel: observation.channel.clone(),
            provider_event_ref: observation.provider_event_ref.clone(),
            capture_latency_ms: observation.capture_latency_ms,
            asr_latency_ms: observation.asr_latency_ms,
            received_at_ms: observation.received_at_ms,
        })
        .map_err(|_| SpeechSpanRevisionError::InvalidContract)?;
        self.index_provider_item(&observation, &revision)?;
        self.spans_by_id.insert(
            revision.span_id().to_string(),
            AdmittedSpan {
                revision: revision.clone(),
                observation,
            },
        );
        Ok(revision)
    }

    fn index_provider_item(
        &mut self,
        observation: &SpanObservation,
        revision: &SpeechSpanRevision,
    ) -> Result<(), SpeechSpanRevisionError> {
        let Some(provider_item_id) = observation.provider_item_id.as_ref() else {
            return Ok(());
        };
        let key = provider_item_key(observation, provider_item_id);
        if self
            .span_by_provider_item
            .get(&key)
            .is_some_and(|span_id| span_id != revision.span_id())
        {
            return Err(SpeechSpanRevisionError::ProviderItemCollision);
        }
        self.span_by_provider_item
            .insert(key, revision.span_id().to_string());
        Ok(())
    }
}

fn provider_item_key(
    observation: &SpanObservation,
    provider_item_id: &str,
) -> (String, String, String) {
    (
        observation.source_stream_id.clone(),
        observation.provider.clone(),
        provider_item_id.to_string(),
    )
}

fn same_observation_content(left: &SpanObservation, right: &SpanObservation) -> bool {
    left.source_stream_id == right.source_stream_id
        && left.provider == right.provider
        && left.provider_item_id == right.provider_item_id
        && left.text == right.text
        && left.stability == right.stability
        && left.timing == right.timing
        && left.confidence == right.confidence
        && left.turn == right.turn
        && left.speaker == right.speaker
        && left.channel == right.channel
        && left.provider_event_ref == right.provider_event_ref
        && left.capture_latency_ms == right.capture_latency_ms
        && left.asr_latency_ms == right.asr_latency_ms
        && left.received_at_ms == right.received_at_ms
}

fn validate_observation(observation: &SpanObservation) -> Result<(), SpeechSpanRevisionError> {
    if observation.source_stream_id.trim().is_empty() {
        return Err(SpeechSpanRevisionError::InvalidSourceStreamId);
    }
    if observation.provider.trim().is_empty() {
        return Err(SpeechSpanRevisionError::InvalidProviderId);
    }
    if observation.text.trim().is_empty() {
        return Err(SpeechSpanRevisionError::InvalidText);
    }
    if observation
        .provider_item_id
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(SpeechSpanRevisionError::InvalidProviderItemId);
    }
    if observation
        .provider_event_ref
        .as_deref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(SpeechSpanRevisionError::InvalidProviderEventRef);
    }
    if observation.correlation.as_ref().is_some_and(|correlation| {
        correlation.span_id.trim().is_empty() || correlation.revision_number == 0
    }) {
        return Err(SpeechSpanRevisionError::InvalidCorrelation);
    }

    if !valid_timing(&observation.timing) {
        return Err(SpeechSpanRevisionError::InvalidTiming);
    }
    if !valid_confidence(&observation.confidence) {
        return Err(SpeechSpanRevisionError::InvalidConfidence);
    }
    if !valid_turn(&observation.turn) {
        return Err(SpeechSpanRevisionError::InvalidTurn);
    }
    if !valid_speaker(&observation.speaker) {
        return Err(SpeechSpanRevisionError::InvalidSpeaker);
    }
    if !valid_channel(&observation.channel) {
        return Err(SpeechSpanRevisionError::InvalidChannel);
    }
    Ok(())
}

fn app_span_id(source_stream_id: &str, ordinal: u64) -> String {
    let mut digest = Sha256::new();
    digest.update(b"audio-graph:speech-span:v2\0");
    digest.update(source_stream_id.as_bytes());
    digest.update([0]);
    digest.update(ordinal.to_be_bytes());
    let hash = hex::encode(digest.finalize());
    format!("ssp_{}", &hash[..32])
}

fn validate_wire(wire: &SpeechSpanRevisionWire) -> Result<(), SpeechSpanContractError> {
    if wire.contract_version != SPEECH_SPAN_CONTRACT_VERSION
        || wire.span_id.trim().is_empty()
        || wire.span_id
            != app_span_id(
                &wire.source_order.source_stream_id,
                wire.source_order.ordinal,
            )
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
        SpeechTiming::Unavailable {} => true,
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
        SpeechConfidence::Unavailable {} => true,
        SpeechConfidence::Provider { value } | SpeechConfidence::App { value } => {
            value.is_finite() && (0.0..=1.0).contains(value)
        }
    }
}

fn valid_turn(turn: &SpeechTurnFidelity) -> bool {
    match turn {
        SpeechTurnFidelity::Unavailable {} => true,
        SpeechTurnFidelity::Provider { value } | SpeechTurnFidelity::App { value } => {
            !value.turn_id.trim().is_empty()
        }
    }
}

fn valid_speaker(speaker: &SpeechSpeakerFidelity) -> bool {
    match speaker {
        SpeechSpeakerFidelity::Unavailable {} => true,
        SpeechSpeakerFidelity::Provider { value } | SpeechSpeakerFidelity::App { value } => {
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

fn valid_channel(channel: &SpeechChannelFidelity) -> bool {
    match channel {
        SpeechChannelFidelity::Unavailable {} => true,
        SpeechChannelFidelity::Provider { value } | SpeechChannelFidelity::App { value } => {
            !value.trim().is_empty()
        }
    }
}

pub fn speech_span_revision_schema_json() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(SpeechSpanRevision))
        .expect("SpeechSpanRevision schema should serialize")
}

pub fn speech_span_revision_typescript_module() -> String {
    let schema_value = speech_span_revision_schema_json();
    let root_ref = schema_value
        .get("$ref")
        .and_then(serde_json::Value::as_str)
        .expect("SpeechSpanRevision schema should have a root reference");
    let root_name = schema_reference_name(root_ref);
    let definitions = schema_value
        .get("$defs")
        .and_then(serde_json::Value::as_object)
        .expect("SpeechSpanRevision schema should have definitions");
    let root_schema = definitions
        .get(root_name)
        .expect("SpeechSpanRevision root definition should exist");
    let contract_version = root_schema
        .pointer("/properties/contract_version/const")
        .and_then(serde_json::Value::as_u64)
        .expect("SpeechSpanRevision contract version should be constant");
    let schema = serde_json::to_string_pretty(&schema_value)
        .expect("SpeechSpanRevision schema should serialize");
    let schema_literal = crate::js_single_quoted_string_literal(&schema);
    let mut generated = String::from(
        "// @generated by src-tauri/crates/ipc-contract/src/speech_span_revision.rs. Do not edit manually.\n\n",
    );
    generated.push_str(&format!(
        "export const SPEECH_SPAN_CONTRACT_VERSION = {contract_version} as const;\n\n"
    ));
    for (name, definition) in definitions {
        if name != root_name {
            generated.push_str(&typescript_declaration(name, definition));
            generated.push('\n');
        }
    }
    generated.push_str(&typescript_declaration("SpeechSpanRevision", root_schema));
    generated.push('\n');
    generated.push_str(&format!(
        "export const SPEECH_SPAN_REVISION_SCHEMA_JSON =\n  {schema_literal};\n\n"
    ));
    generated.push_str(
        "export const SPEECH_SPAN_REVISION_SCHEMA = JSON.parse(\n  SPEECH_SPAN_REVISION_SCHEMA_JSON,\n) as Record<string, unknown>;\n",
    );
    generated
}

fn schema_reference_name(reference: &str) -> &str {
    reference
        .rsplit('/')
        .next()
        .expect("JSON Schema references should end in a definition name")
}

fn typescript_declaration(name: &str, schema: &serde_json::Value) -> String {
    if schema.get("properties").is_some() {
        let properties = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .expect("object schema properties should be an object");
        let required = schema
            .get("required")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect::<std::collections::HashSet<_>>();
        let mut declaration = format!("export interface {name} {{\n");
        for (property, property_schema) in properties {
            let optional = if required.contains(property.as_str()) {
                ""
            } else {
                "?"
            };
            declaration.push_str(&format!(
                "  {property}{optional}: {};\n",
                typescript_type(property_schema)
            ));
        }
        declaration.push_str("}\n");
        return declaration;
    }

    let variants = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(serde_json::Value::as_array);
    if let Some(variants) = variants {
        let mut declaration = format!("export type {name} =\n");
        for (index, variant) in variants.iter().enumerate() {
            let terminator = if index + 1 == variants.len() { ";" } else { "" };
            declaration.push_str(&format!("  | {}{terminator}\n", typescript_type(variant)));
        }
        return declaration;
    }

    format!("export type {name} = {};\n", typescript_type(schema))
}

fn typescript_type(schema: &serde_json::Value) -> String {
    if let Some(reference) = schema.get("$ref").and_then(serde_json::Value::as_str) {
        return schema_reference_name(reference).to_string();
    }
    if let Some(value) = schema.get("const") {
        return serde_json::to_string(value).expect("schema constant should serialize");
    }
    if let Some(properties) = schema
        .get("properties")
        .and_then(serde_json::Value::as_object)
    {
        let required = schema
            .get("required")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .collect::<std::collections::HashSet<_>>();
        let fields = properties
            .iter()
            .map(|(property, property_schema)| {
                let optional = if required.contains(property.as_str()) {
                    ""
                } else {
                    "?"
                };
                format!("{property}{optional}: {}", typescript_type(property_schema))
            })
            .collect::<Vec<_>>();
        if fields.len() >= 4 {
            return format!("{{\n      {};\n    }}", fields.join(";\n      "));
        }
        return format!("{{ {} }}", fields.join("; "));
    }
    if let Some(variants) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(serde_json::Value::as_array)
    {
        return variants
            .iter()
            .map(typescript_type)
            .collect::<Vec<_>>()
            .join(" | ");
    }
    if let Some(values) = schema.get("enum").and_then(serde_json::Value::as_array) {
        return values
            .iter()
            .map(|value| serde_json::to_string(value).expect("schema enum should serialize"))
            .collect::<Vec<_>>()
            .join(" | ");
    }
    if let Some(types) = schema.get("type").and_then(serde_json::Value::as_array) {
        return types
            .iter()
            .map(|value| {
                value
                    .as_str()
                    .map(typescript_primitive)
                    .unwrap_or("unknown")
            })
            .collect::<Vec<_>>()
            .join(" | ");
    }
    match schema.get("type").and_then(serde_json::Value::as_str) {
        Some("array") => format!(
            "Array<{}>",
            schema
                .get("items")
                .map(typescript_type)
                .unwrap_or_else(|| "unknown".into())
        ),
        Some(schema_type) => typescript_primitive(schema_type).to_string(),
        None => "unknown".into(),
    }
}

fn typescript_primitive(schema_type: &str) -> &str {
    match schema_type {
        "integer" | "number" => "number",
        "string" => "string",
        "boolean" => "boolean",
        "null" => "null",
        "object" => "Record<string, unknown>",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_revision_json() -> serde_json::Value {
        let revision = SpeechSpanRevisionNormalizer::new()
            .admit(SpanObservation {
                source_stream_id: "source-stream-a".into(),
                provider: "fixture-provider".into(),
                provider_item_id: None,
                correlation: None,
                text: "fixture transcript".into(),
                stability: SpeechSpanStability::Final,
                timing: SpeechTiming::Unavailable {},
                confidence: SpeechConfidence::Unavailable {},
                turn: SpeechTurnFidelity::Unavailable {},
                speaker: SpeechSpeakerFidelity::Unavailable {},
                channel: SpeechChannelFidelity::Unavailable {},
                provider_event_ref: None,
                capture_latency_ms: None,
                asr_latency_ms: None,
                received_at_ms: 1_700_000_000_000,
            })
            .expect("fixture observation should be valid");
        serde_json::to_value(revision).expect("fixture revision should serialize")
    }

    #[test]
    fn generated_typescript_surface_tracks_every_schema_definition_field_and_variant() {
        let schema = speech_span_revision_schema_json();
        let generated = speech_span_revision_typescript_module();
        let surface = generated
            .split("export const SPEECH_SPAN_REVISION_SCHEMA_JSON")
            .next()
            .expect("generated module should contain its schema constant");
        let definitions = schema["$defs"]
            .as_object()
            .expect("schema definitions object");
        let root_name = schema_reference_name(schema["$ref"].as_str().expect("root reference"));

        for (schema_name, definition) in definitions {
            let generated_name = if schema_name == root_name {
                "SpeechSpanRevision"
            } else {
                schema_name
            };
            let declaration = generated_declaration(surface, generated_name);
            assert_schema_is_represented(definition, declaration);
        }

        let speaker = generated_declaration(surface, "SpeechSpeakerValue");
        assert!(speaker.contains("speaker_id?: string | null;"));
        assert!(speaker.contains("speaker_label?: string | null;"));
    }

    #[test]
    fn exported_schema_encodes_load_bearing_wire_constraints() {
        let schema = speech_span_revision_schema_json();
        let root_name = schema_reference_name(schema["$ref"].as_str().expect("root reference"));
        let root = &schema["$defs"][root_name];

        assert_eq!(root["additionalProperties"], false);
        assert_eq!(root["properties"]["contract_version"]["const"], 2);
        assert_eq!(root["properties"]["contract_version"]["minimum"], 2);
        assert_eq!(root["properties"]["contract_version"]["maximum"], 2);
        assert_eq!(root["properties"]["revision_number"]["minimum"], 1);
        assert_eq!(root["properties"]["provider"]["minLength"], 1);
        assert_eq!(root["properties"]["text"]["minLength"], 1);
        assert_eq!(
            root["properties"]["span_id"]["pattern"],
            r"^ssp_[0-9a-f]{32}$"
        );

        let source_order = &schema["$defs"]["SpeechSpanSourceOrder"];
        assert_eq!(source_order["additionalProperties"], false);
        assert_eq!(source_order["properties"]["ordinal"]["minimum"], 1);
        assert_eq!(
            source_order["properties"]["source_stream_id"]["minLength"],
            1
        );

        for variant in schema["$defs"]["SpeechConfidence"]["oneOf"]
            .as_array()
            .expect("confidence variants")
        {
            assert_eq!(variant["additionalProperties"], false);
            if variant["properties"]["value"].is_object() {
                assert_eq!(variant["properties"]["value"]["minimum"], 0.0);
                assert_eq!(variant["properties"]["value"]["maximum"], 1.0);
            }
        }
        for variant in schema["$defs"]["SpeechTiming"]["oneOf"]
            .as_array()
            .expect("timing variants")
        {
            assert_eq!(variant["additionalProperties"], false);
            if variant["properties"]["start_time"].is_object() {
                assert_eq!(variant["properties"]["start_time"]["minimum"], 0.0);
                assert_eq!(variant["properties"]["end_time"]["minimum"], 0.0);
            }
        }

        let speaker = &schema["$defs"]["SpeechSpeakerValue"];
        assert_eq!(speaker["additionalProperties"], false);
        assert_eq!(speaker["anyOf"].as_array().map(Vec::len), Some(2));
        assert!(
            speaker["properties"]["speaker_id"]["anyOf"]
                .as_array()
                .expect("speaker id nullable constraint")
                .iter()
                .any(|branch| branch["minLength"] == 1)
        );
    }

    #[test]
    fn public_v2_decode_rejects_unknown_nested_fidelity_fields() {
        let mut invalid_values = Vec::new();

        let mut timing = valid_revision_json();
        timing["timing"] = serde_json::json!({
            "origin": "unavailable",
            "start_time": 0.0,
            "end_time": 1.0
        });
        invalid_values.push(timing);

        let mut confidence = valid_revision_json();
        confidence["confidence"] = serde_json::json!({"origin": "unavailable", "value": 0.9});
        invalid_values.push(confidence);

        let mut turn = valid_revision_json();
        turn["turn"] = serde_json::json!({
            "origin": "provider",
            "value": {
                "turn_id": "turn-1",
                "end_of_turn": true,
                "unexpected": true
            }
        });
        invalid_values.push(turn);

        let mut unavailable_turn = valid_revision_json();
        unavailable_turn["turn"] = serde_json::json!({
            "origin": "unavailable",
            "value": {"turn_id": "turn-1", "end_of_turn": true}
        });
        invalid_values.push(unavailable_turn);

        let mut speaker = valid_revision_json();
        speaker["speaker"] = serde_json::json!({
            "origin": "app",
            "value": {
                "speaker_id": "speaker-1",
                "speaker_label": null,
                "unexpected": true
            }
        });
        invalid_values.push(speaker);

        let mut unavailable_speaker = valid_revision_json();
        unavailable_speaker["speaker"] = serde_json::json!({
            "origin": "unavailable",
            "value": {"speaker_id": "speaker-1"}
        });
        invalid_values.push(unavailable_speaker);

        let mut unavailable_channel = valid_revision_json();
        unavailable_channel["channel"] =
            serde_json::json!({"origin": "unavailable", "value": "mixed"});
        invalid_values.push(unavailable_channel);

        let mut source_order = valid_revision_json();
        source_order["source_order"]["unexpected"] = serde_json::json!(true);
        invalid_values.push(source_order);

        let mut supersedes = valid_revision_json();
        supersedes["revision_number"] = serde_json::json!(2);
        supersedes["supersedes"] = serde_json::json!({
            "span_id": "ssp_public-seam-fixture",
            "revision_number": 1,
            "unexpected": true
        });
        invalid_values.push(supersedes);

        for (case_index, invalid) in invalid_values.into_iter().enumerate() {
            assert!(
                serde_json::from_value::<SpeechSpanRevision>(invalid).is_err(),
                "unknown nested fidelity fields must fail closed (case {case_index})"
            );
        }
    }

    #[test]
    fn public_v2_decode_accepts_each_optional_speaker_identifier_independently() {
        for value in [
            serde_json::json!({"speaker_id": "speaker-1"}),
            serde_json::json!({"speaker_label": "Speaker One"}),
        ] {
            let mut revision = valid_revision_json();
            revision["speaker"] = serde_json::json!({"origin": "provider", "value": value});
            serde_json::from_value::<SpeechSpanRevision>(revision)
                .expect("one non-empty speaker identifier is sufficient");
        }
    }

    fn generated_declaration<'a>(surface: &'a str, name: &str) -> &'a str {
        let interface_marker = format!("export interface {name} {{");
        let type_marker = format!("export type {name} =");
        let start = surface
            .find(&interface_marker)
            .or_else(|| surface.find(&type_marker))
            .unwrap_or_else(|| panic!("missing generated declaration for {name}"));
        let remainder = &surface[start..];
        let end = remainder[1..]
            .find("\nexport ")
            .map_or(remainder.len(), |index| index + 1);
        &remainder[..end]
    }

    fn assert_schema_is_represented(schema: &serde_json::Value, declaration: &str) {
        if let Some(properties) = schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
        {
            let required = schema
                .get("required")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .collect::<std::collections::HashSet<_>>();
            for property in properties.keys() {
                let marker = if required.contains(property.as_str()) {
                    format!("{property}:")
                } else {
                    format!("{property}?:")
                };
                assert!(
                    declaration.contains(&marker),
                    "generated declaration omitted or misclassified {property}: {declaration}"
                );
            }
            return;
        }
        if let Some(values) = schema.get("enum").and_then(serde_json::Value::as_array) {
            for value in values {
                let literal = serde_json::to_string(value).expect("enum literal");
                assert!(declaration.contains(&literal));
            }
        }
        for variant in schema
            .get("oneOf")
            .or_else(|| schema.get("anyOf"))
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(properties) = variant
                .get("properties")
                .and_then(serde_json::Value::as_object)
            {
                for property in properties.keys() {
                    assert!(
                        declaration.contains(&format!("{property}:")),
                        "generated union omitted {property}: {declaration}"
                    );
                }
            }
            if let Some(value) = variant.pointer("/properties/origin/const") {
                let literal = serde_json::to_string(value).expect("origin literal");
                assert!(declaration.contains(&literal));
            }
        }
    }
}
