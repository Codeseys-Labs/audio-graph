use std::collections::HashMap;

use sha2::{Digest, Sha256};

use audio_graph_ipc_contract::speech_span_revision::SpeechSpanRevisionParts;
pub use audio_graph_ipc_contract::speech_span_revision::{
    SPEECH_SPAN_CONTRACT_VERSION, SpeechAttribute, SpeechConfidence, SpeechSpanRevision,
    SpeechSpanRevisionRef, SpeechSpanSourceOrder, SpeechSpanStability, SpeechSpeakerValue,
    SpeechTiming, SpeechTimingPrecision, SpeechTurnValue,
};

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
    pub turn: SpeechAttribute<SpeechTurnValue>,
    pub speaker: SpeechAttribute<SpeechSpeakerValue>,
    pub channel: SpeechAttribute<String>,
    pub provider_event_ref: Option<String>,
    pub capture_latency_ms: Option<u64>,
    pub asr_latency_ms: Option<u64>,
    pub received_at_ms: u64,
}

#[derive(Clone, PartialEq)]
pub enum LegacyEvidence<T> {
    Unavailable,
    LegacyUnspecified { value: T },
}

impl<T> std::fmt::Debug for LegacyEvidence<T> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable => formatter.write_str("Unavailable"),
            Self::LegacyUnspecified { .. } => formatter
                .debug_struct("LegacyUnspecified")
                .field("value", &"<redacted>")
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct LegacySpeechSpanRevision {
    span_id: String,
    provider: String,
    source_id: String,
    provider_item_id: Option<String>,
    transcript_segment_id: Option<String>,
    text: String,
    start_time: LegacyEvidence<f64>,
    end_time: LegacyEvidence<f64>,
    confidence: LegacyEvidence<f32>,
    speaker: LegacyEvidence<SpeechSpeakerValue>,
    channel: LegacyEvidence<String>,
    is_final: bool,
    stability: crate::projections::TranscriptEventStability,
    revision_number: u64,
    supersedes: Option<String>,
    turn: LegacyEvidence<LegacyTurnValue>,
    raw_event_ref: Option<String>,
    capture_latency_ms: Option<u64>,
    asr_latency_ms: Option<u64>,
    received_at_ms: u64,
}

impl LegacySpeechSpanRevision {
    pub fn start_time(&self) -> &LegacyEvidence<f64> {
        &self.start_time
    }

    pub fn confidence(&self) -> &LegacyEvidence<f32> {
        &self.confidence
    }

    pub fn speaker(&self) -> &LegacyEvidence<SpeechSpeakerValue> {
        &self.speaker
    }

    pub fn channel(&self) -> &LegacyEvidence<String> {
        &self.channel
    }

    fn into_transcript_event(
        self,
    ) -> Result<crate::projections::TranscriptEvent, SpeechSpanRevisionError> {
        let start_time = required_legacy_value(self.start_time)?;
        let end_time = required_legacy_value(self.end_time)?;
        let confidence = required_legacy_value(self.confidence)?;
        let (speaker_id, speaker_label) = match self.speaker {
            LegacyEvidence::Unavailable => (None, None),
            LegacyEvidence::LegacyUnspecified { value } => (value.speaker_id, value.speaker_label),
        };
        let channel = optional_legacy_value(self.channel);
        let (turn_id, end_of_turn) = match self.turn {
            LegacyEvidence::Unavailable => {
                return Err(SpeechSpanRevisionError::LegacyProjectionUnavailable);
            }
            LegacyEvidence::LegacyUnspecified { value } => {
                let end_of_turn = value
                    .end_of_turn
                    .ok_or(SpeechSpanRevisionError::LegacyProjectionUnavailable)?;
                (value.turn_id, end_of_turn)
            }
        };

        Ok(crate::projections::TranscriptEvent {
            span_id: self.span_id,
            provider: self.provider,
            source_id: self.source_id,
            provider_item_id: self.provider_item_id,
            transcript_segment_id: self.transcript_segment_id,
            speaker_id,
            speaker_label,
            channel,
            text: self.text,
            start_time,
            end_time,
            confidence,
            is_final: self.is_final,
            stability: self.stability,
            revision_number: self.revision_number,
            supersedes: self.supersedes,
            turn_id,
            end_of_turn,
            raw_event_ref: self.raw_event_ref,
            capture_latency_ms: self.capture_latency_ms,
            asr_latency_ms: self.asr_latency_ms,
            received_at_ms: self.received_at_ms,
        })
    }
}

#[derive(Clone, PartialEq)]
pub enum CompatibleSpeechSpanRevision {
    LegacyV1(LegacySpeechSpanRevision),
    V2(SpeechSpanRevision),
}

impl CompatibleSpeechSpanRevision {
    pub fn as_legacy_v1(&self) -> Option<&LegacySpeechSpanRevision> {
        match self {
            Self::LegacyV1(revision) => Some(revision),
            Self::V2(_) => None,
        }
    }

    pub fn into_legacy_transcript_event(
        self,
    ) -> Result<crate::projections::TranscriptEvent, SpeechSpanRevisionError> {
        match self {
            Self::LegacyV1(revision) => revision.into_transcript_event(),
            Self::V2(_) => Err(SpeechSpanRevisionError::LegacyProjectionUnavailable),
        }
    }
}

impl<'de> serde::Deserialize<'de> for CompatibleSpeechSpanRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
        match value.get("contract_version") {
            None => serde_json::from_value::<LegacyTranscriptEventWire>(value)
                .map_err(|_| D::Error::custom("invalid legacy speech span revision"))?
                .try_into()
                .map(Self::LegacyV1)
                .map_err(|_| D::Error::custom("invalid legacy speech span revision")),
            Some(serde_json::Value::Number(version)) if version.as_u64() == Some(2) => {
                serde_json::from_value(value)
                    .map(Self::V2)
                    .map_err(|_| D::Error::custom("invalid v2 speech span revision"))
            }
            Some(_) => Err(D::Error::custom(
                "unsupported speech span revision contract version",
            )),
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyTranscriptEventWire {
    span_id: String,
    provider: String,
    source_id: String,
    #[serde(default)]
    provider_item_id: Option<String>,
    #[serde(default)]
    transcript_segment_id: Option<String>,
    #[serde(default)]
    speaker_id: Option<String>,
    #[serde(default)]
    speaker_label: Option<String>,
    #[serde(default)]
    channel: Option<String>,
    text: String,
    #[serde(default)]
    start_time: Option<f64>,
    #[serde(default)]
    end_time: Option<f64>,
    #[serde(default)]
    confidence: Option<f32>,
    is_final: bool,
    stability: crate::projections::TranscriptEventStability,
    revision_number: u64,
    #[serde(default)]
    supersedes: Option<String>,
    #[serde(default)]
    turn_id: Option<String>,
    #[serde(default)]
    end_of_turn: Option<bool>,
    #[serde(default)]
    raw_event_ref: Option<String>,
    #[serde(default)]
    capture_latency_ms: Option<u64>,
    #[serde(default)]
    asr_latency_ms: Option<u64>,
    received_at_ms: u64,
}

#[derive(Clone, PartialEq, Eq)]
struct LegacyTurnValue {
    turn_id: Option<String>,
    end_of_turn: Option<bool>,
}

impl TryFrom<LegacyTranscriptEventWire> for LegacySpeechSpanRevision {
    type Error = SpeechSpanRevisionError;

    fn try_from(wire: LegacyTranscriptEventWire) -> Result<Self, Self::Error> {
        let start_time = legacy_evidence(wire.start_time);
        let end_time = legacy_evidence(wire.end_time);
        validate_legacy_timing(&start_time, &end_time)?;
        let confidence = legacy_evidence(wire.confidence);
        if let LegacyEvidence::LegacyUnspecified { value } = confidence
            && (!value.is_finite() || !(0.0..=1.0).contains(&value))
        {
            return Err(SpeechSpanRevisionError::InvalidConfidence);
        }
        if wire.revision_number == 0 {
            return Err(SpeechSpanRevisionError::InvalidCorrelation);
        }
        let speaker = if wire.speaker_id.is_none() && wire.speaker_label.is_none() {
            LegacyEvidence::Unavailable
        } else {
            LegacyEvidence::LegacyUnspecified {
                value: SpeechSpeakerValue {
                    speaker_id: wire.speaker_id,
                    speaker_label: wire.speaker_label,
                },
            }
        };
        let turn = if wire.turn_id.is_none() && wire.end_of_turn.is_none() {
            LegacyEvidence::Unavailable
        } else {
            LegacyEvidence::LegacyUnspecified {
                value: LegacyTurnValue {
                    turn_id: wire.turn_id,
                    end_of_turn: wire.end_of_turn,
                },
            }
        };

        Ok(Self {
            span_id: wire.span_id,
            provider: wire.provider,
            source_id: wire.source_id,
            provider_item_id: wire.provider_item_id,
            transcript_segment_id: wire.transcript_segment_id,
            text: wire.text,
            start_time,
            end_time,
            confidence,
            speaker,
            channel: legacy_evidence(wire.channel),
            is_final: wire.is_final,
            stability: wire.stability,
            revision_number: wire.revision_number,
            supersedes: wire.supersedes,
            turn,
            raw_event_ref: wire.raw_event_ref,
            capture_latency_ms: wire.capture_latency_ms,
            asr_latency_ms: wire.asr_latency_ms,
            received_at_ms: wire.received_at_ms,
        })
    }
}

fn legacy_evidence<T>(value: Option<T>) -> LegacyEvidence<T> {
    match value {
        Some(value) => LegacyEvidence::LegacyUnspecified { value },
        None => LegacyEvidence::Unavailable,
    }
}

fn required_legacy_value<T>(evidence: LegacyEvidence<T>) -> Result<T, SpeechSpanRevisionError> {
    match evidence {
        LegacyEvidence::Unavailable => Err(SpeechSpanRevisionError::LegacyProjectionUnavailable),
        LegacyEvidence::LegacyUnspecified { value } => Ok(value),
    }
}

fn optional_legacy_value<T>(evidence: LegacyEvidence<T>) -> Option<T> {
    match evidence {
        LegacyEvidence::Unavailable => None,
        LegacyEvidence::LegacyUnspecified { value } => Some(value),
    }
}

fn validate_legacy_timing(
    start_time: &LegacyEvidence<f64>,
    end_time: &LegacyEvidence<f64>,
) -> Result<(), SpeechSpanRevisionError> {
    let start = match start_time {
        LegacyEvidence::Unavailable => None,
        LegacyEvidence::LegacyUnspecified { value } => Some(*value),
    };
    let end = match end_time {
        LegacyEvidence::Unavailable => None,
        LegacyEvidence::LegacyUnspecified { value } => Some(*value),
    };
    if start.is_some_and(|value| !value.is_finite() || value < 0.0)
        || end.is_some_and(|value| !value.is_finite() || value < 0.0)
        || matches!((start, end), (Some(start), Some(end)) if start > end)
    {
        return Err(SpeechSpanRevisionError::InvalidTiming);
    }
    Ok(())
}

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

#[derive(Clone)]
struct AdmittedSpan {
    revision: SpeechSpanRevision,
    observation: SpanObservation,
}

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

    match &observation.timing {
        SpeechTiming::Unavailable => {}
        SpeechTiming::AppEstimated {
            start_time,
            end_time,
        }
        | SpeechTiming::Provider {
            start_time,
            end_time,
            ..
        } if !start_time.is_finite()
            || !end_time.is_finite()
            || *start_time < 0.0
            || *end_time < 0.0
            || start_time > end_time =>
        {
            return Err(SpeechSpanRevisionError::InvalidTiming);
        }
        SpeechTiming::AppEstimated { .. } | SpeechTiming::Provider { .. } => {}
    }

    match observation.confidence {
        SpeechConfidence::Unavailable => {}
        SpeechConfidence::Provider { value } | SpeechConfidence::App { value }
            if !value.is_finite() || !(0.0..=1.0).contains(&value) =>
        {
            return Err(SpeechSpanRevisionError::InvalidConfidence);
        }
        SpeechConfidence::Provider { .. } | SpeechConfidence::App { .. } => {}
    }

    match &observation.turn {
        SpeechAttribute::Unavailable => {}
        SpeechAttribute::Provider { value } | SpeechAttribute::App { value }
            if value.turn_id.trim().is_empty() =>
        {
            return Err(SpeechSpanRevisionError::InvalidTurn);
        }
        SpeechAttribute::Provider { .. } | SpeechAttribute::App { .. } => {}
    }

    match &observation.speaker {
        SpeechAttribute::Unavailable => {}
        SpeechAttribute::Provider { value } | SpeechAttribute::App { value }
            if invalid_speaker(value) =>
        {
            return Err(SpeechSpanRevisionError::InvalidSpeaker);
        }
        SpeechAttribute::Provider { .. } | SpeechAttribute::App { .. } => {}
    }

    match &observation.channel {
        SpeechAttribute::Unavailable => {}
        SpeechAttribute::Provider { value } | SpeechAttribute::App { value }
            if value.trim().is_empty() =>
        {
            return Err(SpeechSpanRevisionError::InvalidChannel);
        }
        SpeechAttribute::Provider { .. } | SpeechAttribute::App { .. } => {}
    }

    Ok(())
}

fn invalid_speaker(value: &SpeechSpeakerValue) -> bool {
    let invalid_id = value
        .speaker_id
        .as_deref()
        .is_some_and(|speaker_id| speaker_id.trim().is_empty());
    let invalid_label = value
        .speaker_label
        .as_deref()
        .is_some_and(|speaker_label| speaker_label.trim().is_empty());
    invalid_id || invalid_label || (value.speaker_id.is_none() && value.speaker_label.is_none())
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

#[cfg(test)]
mod tests {
    use super::{
        CompatibleSpeechSpanRevision, LegacyEvidence, SpanObservation, SpeechAttribute,
        SpeechConfidence, SpeechSpanRevisionError, SpeechSpanRevisionNormalizer,
        SpeechSpanStability, SpeechSpeakerValue, SpeechTiming, SpeechTimingPrecision,
        SpeechTurnValue,
    };

    fn unavailable_observation() -> SpanObservation {
        SpanObservation {
            source_stream_id: "source-stream-a".into(),
            provider: "fixture-provider".into(),
            provider_item_id: None,
            correlation: None,
            text: "content is intentionally omitted from assertions".into(),
            stability: SpeechSpanStability::Final,
            timing: SpeechTiming::Unavailable,
            confidence: SpeechConfidence::Unavailable,
            turn: SpeechAttribute::Unavailable,
            speaker: SpeechAttribute::Unavailable,
            channel: SpeechAttribute::Unavailable,
            provider_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000,
        }
    }

    #[test]
    fn admit_serializes_explicit_v2_unavailable_fidelity_without_legacy_scalars() {
        let mut normalizer = SpeechSpanRevisionNormalizer::new();
        let revision = normalizer
            .admit(unavailable_observation())
            .expect("valid unavailable observation");

        let json = serde_json::to_value(&revision).expect("serialize admitted revision");
        assert_eq!(json["contract_version"], 2);
        assert_eq!(json["timing"]["origin"], "unavailable");
        assert_eq!(json["confidence"]["origin"], "unavailable");
        assert_eq!(json["turn"]["origin"], "unavailable");
        assert_eq!(json["speaker"]["origin"], "unavailable");
        assert_eq!(json["channel"]["origin"], "unavailable");
        for legacy_scalar in [
            "start_time",
            "end_time",
            "speaker_id",
            "speaker_label",
            "turn_id",
            "end_of_turn",
        ] {
            assert!(
                !json
                    .as_object()
                    .expect("revision object")
                    .contains_key(legacy_scalar),
                "v2 must not serialize co-authoritative legacy scalar {legacy_scalar}"
            );
        }
    }

    #[test]
    fn admit_owns_per_source_order_identity_and_exact_revision_supersession() {
        let mut normalizer = SpeechSpanRevisionNormalizer::new();
        let first_a = normalizer
            .admit(unavailable_observation())
            .expect("first source-a span");
        let mut source_b = unavailable_observation();
        source_b.source_stream_id = "source-stream-b".into();
        source_b.text = "different content".into();
        let first_b = normalizer.admit(source_b).expect("first source-b span");

        assert_eq!(first_a.source_order().ordinal, 1);
        assert_eq!(first_b.source_order().ordinal, 1);
        assert_ne!(first_a.span_id(), first_b.span_id());

        let mut correction = unavailable_observation();
        correction.text = "corrected content".into();
        correction.received_at_ms += 1;
        correction.correlation = Some(first_a.revision_ref());
        let second_a = normalizer.admit(correction).expect("source-a correction");

        assert_eq!(second_a.span_id(), first_a.span_id());
        assert_eq!(second_a.source_order(), first_a.source_order());
        assert_eq!(second_a.revision_number(), 2);
        assert_eq!(second_a.supersedes(), Some(&first_a.revision_ref()));

        let mut second_span_a = unavailable_observation();
        second_span_a.text = "independent second span".into();
        let second_span_a = normalizer
            .admit(second_span_a)
            .expect("second independent source-a span");
        assert_eq!(second_span_a.source_order().ordinal, 2);
        assert_ne!(second_span_a.span_id(), first_a.span_id());
    }

    #[test]
    fn admit_rejects_duplicate_conflicting_and_stale_correlations() {
        let mut normalizer = SpeechSpanRevisionNormalizer::new();
        let first = normalizer
            .admit(unavailable_observation())
            .expect("first revision");
        let mut correction = unavailable_observation();
        correction.text = "correction one".into();
        correction.received_at_ms += 1;
        correction.correlation = Some(first.revision_ref());
        let second = normalizer
            .admit(correction.clone())
            .expect("second revision");

        assert_eq!(
            normalizer.admit(correction),
            Err(SpeechSpanRevisionError::DuplicateObservation)
        );

        let mut conflict = unavailable_observation();
        conflict.text = "divergent correction".into();
        conflict.received_at_ms += 1;
        conflict.correlation = Some(first.revision_ref());
        assert_eq!(
            normalizer.admit(conflict),
            Err(SpeechSpanRevisionError::ConflictingObservation)
        );

        let mut third = unavailable_observation();
        third.text = "correction two".into();
        third.received_at_ms += 2;
        third.correlation = Some(second.revision_ref());
        normalizer.admit(third).expect("third revision");

        let mut stale = unavailable_observation();
        stale.text = "stale candidate".into();
        stale.received_at_ms += 3;
        stale.correlation = Some(first.revision_ref());
        let error = normalizer.admit(stale).expect_err("stale correlation");
        assert_eq!(error, SpeechSpanRevisionError::StaleCorrelation);
        let debug = format!("{error:?}");
        for forbidden in [
            "stale candidate",
            "fixture-provider",
            "source-stream-a",
            "credential",
        ] {
            assert!(!debug.contains(forbidden), "errors must be content-free");
        }
    }

    #[test]
    fn admit_round_trips_provider_and_app_fidelity() {
        let mut observation = unavailable_observation();
        observation.timing = SpeechTiming::Provider {
            precision: SpeechTimingPrecision::Exact,
            start_time: 1.25,
            end_time: 2.75,
        };
        observation.confidence = SpeechConfidence::Provider { value: 0.875 };
        observation.turn = SpeechAttribute::Provider {
            value: SpeechTurnValue {
                turn_id: "provider-turn-7".into(),
                end_of_turn: true,
            },
        };
        observation.speaker = SpeechAttribute::App {
            value: SpeechSpeakerValue {
                speaker_id: Some("speaker-7".into()),
                speaker_label: Some("redacted in debug".into()),
            },
        };
        observation.channel = SpeechAttribute::Provider {
            value: "channel-1".into(),
        };

        let revision = SpeechSpanRevisionNormalizer::new()
            .admit(observation)
            .expect("valid mixed-origin observation");
        let json = serde_json::to_string(&revision).expect("serialize v2 revision");
        let decoded: super::SpeechSpanRevision =
            serde_json::from_str(&json).expect("deserialize v2 revision");

        assert_eq!(decoded, revision);
        assert!(!format!("{revision:?}").contains("redacted in debug"));
    }

    #[test]
    fn v2_decode_rejects_wrong_version_and_invalid_supersession() {
        let revision = SpeechSpanRevisionNormalizer::new()
            .admit(unavailable_observation())
            .expect("valid revision");
        let mut wrong_version = serde_json::to_value(&revision).expect("serialize revision");
        wrong_version["contract_version"] = serde_json::json!(1);
        assert!(
            serde_json::from_value::<super::SpeechSpanRevision>(wrong_version).is_err(),
            "v2 decoder must reject every contract version except 2"
        );

        let mut invalid_supersession = serde_json::to_value(&revision).expect("serialize revision");
        invalid_supersession["supersedes"] = serde_json::json!({
            "span_id": revision.span_id(),
            "revision_number": 1
        });
        assert!(
            serde_json::from_value::<super::SpeechSpanRevision>(invalid_supersession).is_err(),
            "revision one cannot supersede another revision"
        );
    }

    #[test]
    fn provider_item_id_is_an_idempotency_hint_not_span_identity() {
        let mut normalizer = SpeechSpanRevisionNormalizer::new();
        let mut first = unavailable_observation();
        first.provider_item_id = Some("provider-item-7".into());
        let admitted = normalizer.admit(first.clone()).expect("first observation");
        assert_eq!(
            normalizer.admit(first.clone()),
            Err(SpeechSpanRevisionError::DuplicateObservation)
        );

        first.text = "provider reused item id with different content".into();
        assert_eq!(
            normalizer.admit(first),
            Err(SpeechSpanRevisionError::ConflictingObservation)
        );
        assert!(admitted.span_id().starts_with("ssp_"));
        assert!(!admitted.span_id().contains("provider-item-7"));
    }

    #[test]
    fn admit_rejects_non_finite_out_of_range_and_empty_evidence() {
        let invalid_cases = [
            (
                SpeechTiming::Provider {
                    precision: SpeechTimingPrecision::Exact,
                    start_time: f64::NAN,
                    end_time: 1.0,
                },
                SpeechConfidence::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechSpanRevisionError::InvalidTiming,
            ),
            (
                SpeechTiming::AppEstimated {
                    start_time: -0.1,
                    end_time: 1.0,
                },
                SpeechConfidence::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechSpanRevisionError::InvalidTiming,
            ),
            (
                SpeechTiming::Provider {
                    precision: SpeechTimingPrecision::Coarse,
                    start_time: 2.0,
                    end_time: 1.0,
                },
                SpeechConfidence::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechSpanRevisionError::InvalidTiming,
            ),
            (
                SpeechTiming::Unavailable,
                SpeechConfidence::App { value: 1.1 },
                SpeechAttribute::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechSpanRevisionError::InvalidConfidence,
            ),
            (
                SpeechTiming::Unavailable,
                SpeechConfidence::Provider { value: f32::NAN },
                SpeechAttribute::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechSpanRevisionError::InvalidConfidence,
            ),
            (
                SpeechTiming::Unavailable,
                SpeechConfidence::Unavailable,
                SpeechAttribute::Provider {
                    value: SpeechTurnValue {
                        turn_id: " ".into(),
                        end_of_turn: false,
                    },
                },
                SpeechAttribute::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechSpanRevisionError::InvalidTurn,
            ),
            (
                SpeechTiming::Unavailable,
                SpeechConfidence::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechAttribute::App {
                    value: SpeechSpeakerValue {
                        speaker_id: None,
                        speaker_label: None,
                    },
                },
                SpeechAttribute::Unavailable,
                SpeechSpanRevisionError::InvalidSpeaker,
            ),
            (
                SpeechTiming::Unavailable,
                SpeechConfidence::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechAttribute::Unavailable,
                SpeechAttribute::Provider { value: "".into() },
                SpeechSpanRevisionError::InvalidChannel,
            ),
        ];

        for (timing, confidence, turn, speaker, channel, expected) in invalid_cases {
            let mut observation = unavailable_observation();
            observation.timing = timing;
            observation.confidence = confidence;
            observation.turn = turn;
            observation.speaker = speaker;
            observation.channel = channel;
            assert_eq!(
                SpeechSpanRevisionNormalizer::new().admit(observation),
                Err(expected)
            );
        }
    }

    #[test]
    fn absent_version_decodes_as_v1_with_explicit_legacy_unspecified_evidence() {
        let legacy_json = serde_json::json!({
            "span_id": "legacy-span-1",
            "provider": "legacy-provider",
            "source_id": "legacy-source",
            "provider_item_id": "provider-item-1",
            "transcript_segment_id": "segment-1",
            "speaker_id": "speaker-1",
            "speaker_label": "Legacy Speaker",
            "channel": "mixed",
            "text": "legacy transcript content",
            "start_time": 1.25,
            "end_time": 2.75,
            "confidence": 0.75,
            "is_final": true,
            "stability": "final",
            "revision_number": 2,
            "supersedes": "legacy-span-1@rev1",
            "turn_id": "turn-1",
            "end_of_turn": true,
            "raw_event_ref": "fixture.ref",
            "capture_latency_ms": 10,
            "asr_latency_ms": 20,
            "received_at_ms": 1700000000000_u64
        });

        let decoded: CompatibleSpeechSpanRevision =
            serde_json::from_value(legacy_json.clone()).expect("decode legacy v1");
        let legacy = decoded.as_legacy_v1().expect("absent version is v1");
        assert_eq!(
            legacy.start_time(),
            &LegacyEvidence::LegacyUnspecified { value: 1.25 }
        );
        assert_eq!(
            legacy.confidence(),
            &LegacyEvidence::LegacyUnspecified { value: 0.75 }
        );
        assert!(matches!(
            legacy.speaker(),
            LegacyEvidence::LegacyUnspecified { .. }
        ));

        let compatibility_event = decoded
            .into_legacy_transcript_event()
            .expect("fully populated v1 projects without fabrication");
        assert_eq!(
            serde_json::to_value(compatibility_event).expect("serialize compatibility event"),
            legacy_json
        );

        let mut missing = legacy_json;
        missing
            .as_object_mut()
            .expect("legacy object")
            .remove("confidence");
        missing
            .as_object_mut()
            .expect("legacy object")
            .remove("channel");
        let decoded_missing: CompatibleSpeechSpanRevision =
            serde_json::from_value(missing).expect("decode missing legacy attributes");
        let legacy_missing = decoded_missing.as_legacy_v1().expect("legacy v1");
        assert_eq!(legacy_missing.confidence(), &LegacyEvidence::Unavailable);
        assert_eq!(legacy_missing.channel(), &LegacyEvidence::Unavailable);
    }
}
