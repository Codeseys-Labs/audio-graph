pub use audio_graph_ipc_contract::speech_span_revision::{
    SPEECH_SPAN_CONTRACT_VERSION, SpanObservation, SpeechChannelFidelity, SpeechConfidence,
    SpeechSpanRevision, SpeechSpanRevisionDecodeError, SpeechSpanRevisionError,
    SpeechSpanRevisionNormalizer, SpeechSpanRevisionRef, SpeechSpanSourceOrder,
    SpeechSpanStability, SpeechSpeakerFidelity, SpeechSpeakerValue, SpeechTiming,
    SpeechTimingPrecision, SpeechTurnFidelity, SpeechTurnValue, expected_speech_span_id,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionSemanticPayloadKind {
    LegacyV1,
    SpeechSpanRevisionV2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectionSemanticStability {
    Partial,
    Final,
}

#[derive(Clone, PartialEq, Eq)]
pub enum ProjectionSemanticSupersession {
    Absent,
    LegacyReference {
        reference: String,
    },
    V2Exact {
        span_id: String,
        revision_number: u64,
    },
}

#[derive(Clone, PartialEq)]
pub enum ProjectionSemanticTiming {
    Unavailable,
    LegacyUnspecified {
        start_time: Option<f64>,
        end_time: Option<f64>,
    },
    AppEstimated {
        start_time: f64,
        end_time: f64,
    },
    ProviderCoarse {
        start_time: f64,
        end_time: f64,
    },
    ProviderExact {
        start_time: f64,
        end_time: f64,
    },
}

#[derive(Clone, PartialEq)]
pub enum ProjectionSemanticConfidence {
    Unavailable,
    LegacyUnspecified { value: f32 },
    App { value: f32 },
    Provider { value: f32 },
}

#[derive(Clone, PartialEq, Eq)]
pub enum ProjectionSemanticTurn {
    Unavailable,
    LegacyUnspecified {
        turn_id: Option<String>,
        end_of_turn: Option<bool>,
    },
    App {
        turn_id: String,
        end_of_turn: bool,
    },
    Provider {
        turn_id: String,
        end_of_turn: bool,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub enum ProjectionSemanticSpeaker {
    Unavailable,
    LegacyUnspecified {
        speaker_id: Option<String>,
        speaker_label: Option<String>,
    },
    App {
        speaker_id: Option<String>,
        speaker_label: Option<String>,
    },
    Provider {
        speaker_id: Option<String>,
        speaker_label: Option<String>,
    },
}

#[derive(Clone, PartialEq, Eq)]
pub enum ProjectionSemanticChannel {
    Unavailable,
    LegacyUnspecified { value: String },
    App { value: String },
    Provider { value: String },
}

/// The projection-relevant meaning of one validated legacy or v2 revision.
///
/// Operational and storage metadata have no representation in this type, so
/// neither the hash encoder nor a future projection prompt can accidentally
/// consume them through this seam.
///
/// ```compile_fail
/// # fn inspect(revision: audio_graph::speech_span_revision::ProjectionSemanticRevision) {
/// revision.payload_kind = audio_graph::speech_span_revision::ProjectionSemanticPayloadKind::LegacyV1;
/// # }
/// ```
#[derive(Clone, PartialEq)]
pub struct ProjectionSemanticRevision {
    payload_kind: ProjectionSemanticPayloadKind,
    span_id: String,
    source_id: String,
    source_ordinal: Option<u64>,
    provider: String,
    text: String,
    stability: ProjectionSemanticStability,
    is_final: bool,
    revision_number: u64,
    supersession: ProjectionSemanticSupersession,
    timing: ProjectionSemanticTiming,
    confidence: ProjectionSemanticConfidence,
    turn: ProjectionSemanticTurn,
    speaker: ProjectionSemanticSpeaker,
    channel: ProjectionSemanticChannel,
}

impl ProjectionSemanticRevision {
    pub fn payload_kind(&self) -> ProjectionSemanticPayloadKind {
        self.payload_kind
    }

    pub fn span_id(&self) -> &str {
        &self.span_id
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn source_ordinal(&self) -> Option<u64> {
        self.source_ordinal
    }

    pub fn provider(&self) -> &str {
        &self.provider
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn stability(&self) -> ProjectionSemanticStability {
        self.stability
    }

    pub fn is_final(&self) -> bool {
        self.is_final
    }

    pub fn revision_number(&self) -> u64 {
        self.revision_number
    }

    pub fn supersession(&self) -> &ProjectionSemanticSupersession {
        &self.supersession
    }

    pub fn timing(&self) -> &ProjectionSemanticTiming {
        &self.timing
    }

    pub fn confidence(&self) -> &ProjectionSemanticConfidence {
        &self.confidence
    }

    pub fn turn(&self) -> &ProjectionSemanticTurn {
        &self.turn
    }

    pub fn speaker(&self) -> &ProjectionSemanticSpeaker {
        &self.speaker
    }

    pub fn channel(&self) -> &ProjectionSemanticChannel {
        &self.channel
    }

    pub(crate) fn validate_for_hash(&self) -> Result<(), ProjectionSemanticError> {
        match self.payload_kind {
            ProjectionSemanticPayloadKind::LegacyV1 => {
                if self.source_ordinal.is_some()
                    || matches!(
                        self.supersession,
                        ProjectionSemanticSupersession::V2Exact { .. }
                    )
                    || !matches!(
                        self.timing,
                        ProjectionSemanticTiming::Unavailable
                            | ProjectionSemanticTiming::LegacyUnspecified { .. }
                    )
                    || !matches!(
                        self.confidence,
                        ProjectionSemanticConfidence::Unavailable
                            | ProjectionSemanticConfidence::LegacyUnspecified { .. }
                    )
                    || !matches!(
                        self.turn,
                        ProjectionSemanticTurn::Unavailable
                            | ProjectionSemanticTurn::LegacyUnspecified { .. }
                    )
                    || !matches!(
                        self.speaker,
                        ProjectionSemanticSpeaker::Unavailable
                            | ProjectionSemanticSpeaker::LegacyUnspecified { .. }
                    )
                    || !matches!(
                        self.channel,
                        ProjectionSemanticChannel::Unavailable
                            | ProjectionSemanticChannel::LegacyUnspecified { .. }
                    )
                {
                    return Err(ProjectionSemanticError::UnsupportedSemanticCombination);
                }
                if matches!(
                    &self.supersession,
                    ProjectionSemanticSupersession::LegacyReference { reference }
                        if reference.trim().is_empty()
                ) {
                    return Err(ProjectionSemanticError::InvalidSupersession);
                }
            }
            ProjectionSemanticPayloadKind::SpeechSpanRevisionV2 => {
                let ordinal = self
                    .source_ordinal
                    .filter(|ordinal| *ordinal > 0)
                    .ok_or(ProjectionSemanticError::IdentityMismatch)?;
                if self.span_id != expected_speech_span_id(&self.source_id, ordinal) {
                    return Err(ProjectionSemanticError::IdentityMismatch);
                }
                if matches!(
                    self.supersession,
                    ProjectionSemanticSupersession::LegacyReference { .. }
                ) || matches!(
                    self.timing,
                    ProjectionSemanticTiming::LegacyUnspecified { .. }
                ) || matches!(
                    self.confidence,
                    ProjectionSemanticConfidence::LegacyUnspecified { .. }
                ) || matches!(self.turn, ProjectionSemanticTurn::LegacyUnspecified { .. })
                    || matches!(
                        self.speaker,
                        ProjectionSemanticSpeaker::LegacyUnspecified { .. }
                    )
                    || matches!(
                        self.channel,
                        ProjectionSemanticChannel::LegacyUnspecified { .. }
                    )
                {
                    return Err(ProjectionSemanticError::UnsupportedSemanticCombination);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProjectionSemanticError {
    #[error("unsupported projection semantic contract version")]
    UnsupportedContractVersion,
    #[error("unsupported projection semantic enum tag")]
    UnsupportedEnumTag,
    #[error("unsupported projection semantic option tag")]
    UnsupportedOptionTag,
    #[error("unsupported projection semantic boolean tag")]
    UnsupportedBooleanTag,
    #[error("malformed projection semantic value")]
    MalformedValue,
    #[error("projection semantic identity mismatch")]
    IdentityMismatch,
    #[error("unsupported projection semantic payload/evidence combination")]
    UnsupportedSemanticCombination,
    #[error("projection semantic revision has an invalid required string")]
    InvalidRequiredString,
    #[error("projection semantic revision has an invalid revision number")]
    InvalidRevisionNumber,
    #[error("projection semantic revision has invalid supersession")]
    InvalidSupersession,
    #[error("projection semantic revision has non-finite timing")]
    NonFiniteTiming,
    #[error("projection semantic revision has non-finite confidence")]
    NonFiniteConfidence,
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

    pub fn projection_semantics(
        &self,
    ) -> Result<ProjectionSemanticRevision, ProjectionSemanticError> {
        match self {
            Self::LegacyV1(revision) => revision.projection_semantics(),
            Self::V2(revision) => Ok(v2_projection_semantics(revision)),
        }
    }

    pub fn decode_json_value(value: serde_json::Value) -> Result<Self, ProjectionSemanticError> {
        match value.get("contract_version") {
            None => decode_legacy_json_value(value).map(Self::LegacyV1),
            Some(serde_json::Value::Number(version)) if version.as_u64() == Some(2) => {
                SpeechSpanRevision::decode_json_value(value)
                    .map(Self::V2)
                    .map_err(ProjectionSemanticError::from)
            }
            Some(_) => Err(ProjectionSemanticError::UnsupportedContractVersion),
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

impl From<SpeechSpanRevisionDecodeError> for ProjectionSemanticError {
    fn from(error: SpeechSpanRevisionDecodeError) -> Self {
        match error {
            SpeechSpanRevisionDecodeError::UnsupportedContractVersion => {
                Self::UnsupportedContractVersion
            }
            SpeechSpanRevisionDecodeError::UnsupportedEnumTag => Self::UnsupportedEnumTag,
            SpeechSpanRevisionDecodeError::UnsupportedOptionTag => Self::UnsupportedOptionTag,
            SpeechSpanRevisionDecodeError::UnsupportedBooleanTag => Self::UnsupportedBooleanTag,
            SpeechSpanRevisionDecodeError::MalformedValue => Self::MalformedValue,
            SpeechSpanRevisionDecodeError::IdentityMismatch => Self::IdentityMismatch,
            SpeechSpanRevisionDecodeError::InvalidSupersession => Self::InvalidSupersession,
        }
    }
}

impl LegacySpeechSpanRevision {
    fn projection_semantics(&self) -> Result<ProjectionSemanticRevision, ProjectionSemanticError> {
        for value in [&self.span_id, &self.source_id, &self.provider, &self.text] {
            if value.trim().is_empty() {
                return Err(ProjectionSemanticError::InvalidRequiredString);
            }
        }
        if self.revision_number == 0 {
            return Err(ProjectionSemanticError::InvalidRevisionNumber);
        }

        let supersession = match &self.supersedes {
            None => ProjectionSemanticSupersession::Absent,
            Some(reference) if !reference.trim().is_empty() => {
                ProjectionSemanticSupersession::LegacyReference {
                    reference: reference.clone(),
                }
            }
            Some(_) => return Err(ProjectionSemanticError::InvalidSupersession),
        };
        let start_time = legacy_option(&self.start_time);
        let end_time = legacy_option(&self.end_time);
        validate_semantic_f64(start_time)?;
        validate_semantic_f64(end_time)?;
        let timing = if start_time.is_none() && end_time.is_none() {
            ProjectionSemanticTiming::Unavailable
        } else {
            ProjectionSemanticTiming::LegacyUnspecified {
                start_time,
                end_time,
            }
        };
        let confidence = match &self.confidence {
            LegacyEvidence::Unavailable => ProjectionSemanticConfidence::Unavailable,
            LegacyEvidence::LegacyUnspecified { value } if value.is_finite() => {
                ProjectionSemanticConfidence::LegacyUnspecified { value: *value }
            }
            LegacyEvidence::LegacyUnspecified { .. } => {
                return Err(ProjectionSemanticError::NonFiniteConfidence);
            }
        };
        let turn = match &self.turn {
            LegacyEvidence::Unavailable => ProjectionSemanticTurn::Unavailable,
            LegacyEvidence::LegacyUnspecified { value } => {
                ProjectionSemanticTurn::LegacyUnspecified {
                    turn_id: value.turn_id.clone(),
                    end_of_turn: value.end_of_turn,
                }
            }
        };
        let speaker = match &self.speaker {
            LegacyEvidence::Unavailable => ProjectionSemanticSpeaker::Unavailable,
            LegacyEvidence::LegacyUnspecified { value } => {
                ProjectionSemanticSpeaker::LegacyUnspecified {
                    speaker_id: value.speaker_id.clone(),
                    speaker_label: value.speaker_label.clone(),
                }
            }
        };
        let channel = match &self.channel {
            LegacyEvidence::Unavailable => ProjectionSemanticChannel::Unavailable,
            LegacyEvidence::LegacyUnspecified { value } => {
                ProjectionSemanticChannel::LegacyUnspecified {
                    value: value.clone(),
                }
            }
        };

        Ok(ProjectionSemanticRevision {
            payload_kind: ProjectionSemanticPayloadKind::LegacyV1,
            span_id: self.span_id.clone(),
            source_id: self.source_id.clone(),
            source_ordinal: None,
            provider: self.provider.clone(),
            text: self.text.clone(),
            stability: match self.stability {
                crate::projections::TranscriptEventStability::Partial => {
                    ProjectionSemanticStability::Partial
                }
                crate::projections::TranscriptEventStability::Final => {
                    ProjectionSemanticStability::Final
                }
            },
            is_final: self.is_final,
            revision_number: self.revision_number,
            supersession,
            timing,
            confidence,
            turn,
            speaker,
            channel,
        })
    }
}

fn v2_projection_semantics(revision: &SpeechSpanRevision) -> ProjectionSemanticRevision {
    let timing = match revision.timing() {
        SpeechTiming::Unavailable {} => ProjectionSemanticTiming::Unavailable,
        SpeechTiming::AppEstimated {
            start_time,
            end_time,
        } => ProjectionSemanticTiming::AppEstimated {
            start_time: *start_time,
            end_time: *end_time,
        },
        SpeechTiming::Provider {
            precision: SpeechTimingPrecision::Coarse,
            start_time,
            end_time,
        } => ProjectionSemanticTiming::ProviderCoarse {
            start_time: *start_time,
            end_time: *end_time,
        },
        SpeechTiming::Provider {
            precision: SpeechTimingPrecision::Exact,
            start_time,
            end_time,
        } => ProjectionSemanticTiming::ProviderExact {
            start_time: *start_time,
            end_time: *end_time,
        },
    };
    let confidence = match revision.confidence() {
        SpeechConfidence::Unavailable {} => ProjectionSemanticConfidence::Unavailable,
        SpeechConfidence::App { value } => ProjectionSemanticConfidence::App { value: *value },
        SpeechConfidence::Provider { value } => {
            ProjectionSemanticConfidence::Provider { value: *value }
        }
    };
    let turn = match revision.turn() {
        SpeechTurnFidelity::Unavailable {} => ProjectionSemanticTurn::Unavailable,
        SpeechTurnFidelity::App { value } => ProjectionSemanticTurn::App {
            turn_id: value.turn_id.clone(),
            end_of_turn: value.end_of_turn,
        },
        SpeechTurnFidelity::Provider { value } => ProjectionSemanticTurn::Provider {
            turn_id: value.turn_id.clone(),
            end_of_turn: value.end_of_turn,
        },
    };
    let speaker = match revision.speaker() {
        SpeechSpeakerFidelity::Unavailable {} => ProjectionSemanticSpeaker::Unavailable,
        SpeechSpeakerFidelity::App { value } => ProjectionSemanticSpeaker::App {
            speaker_id: value.speaker_id.clone(),
            speaker_label: value.speaker_label.clone(),
        },
        SpeechSpeakerFidelity::Provider { value } => ProjectionSemanticSpeaker::Provider {
            speaker_id: value.speaker_id.clone(),
            speaker_label: value.speaker_label.clone(),
        },
    };
    let channel = match revision.channel() {
        SpeechChannelFidelity::Unavailable {} => ProjectionSemanticChannel::Unavailable,
        SpeechChannelFidelity::App { value } => ProjectionSemanticChannel::App {
            value: value.clone(),
        },
        SpeechChannelFidelity::Provider { value } => ProjectionSemanticChannel::Provider {
            value: value.clone(),
        },
    };

    ProjectionSemanticRevision {
        payload_kind: ProjectionSemanticPayloadKind::SpeechSpanRevisionV2,
        span_id: revision.span_id().to_string(),
        source_id: revision.source_order().source_stream_id.clone(),
        source_ordinal: Some(revision.source_order().ordinal),
        provider: revision.provider().to_string(),
        text: revision.text().to_string(),
        stability: match revision.stability() {
            SpeechSpanStability::Partial => ProjectionSemanticStability::Partial,
            SpeechSpanStability::Final => ProjectionSemanticStability::Final,
        },
        is_final: revision.is_final(),
        revision_number: revision.revision_number(),
        supersession: match revision.supersedes() {
            None => ProjectionSemanticSupersession::Absent,
            Some(reference) => ProjectionSemanticSupersession::V2Exact {
                span_id: reference.span_id.clone(),
                revision_number: reference.revision_number,
            },
        },
        timing,
        confidence,
        turn,
        speaker,
        channel,
    }
}

fn legacy_option<T: Copy>(evidence: &LegacyEvidence<T>) -> Option<T> {
    match evidence {
        LegacyEvidence::Unavailable => None,
        LegacyEvidence::LegacyUnspecified { value } => Some(*value),
    }
}

fn validate_semantic_f64(value: Option<f64>) -> Result<(), ProjectionSemanticError> {
    if value.is_some_and(|value| !value.is_finite()) {
        Err(ProjectionSemanticError::NonFiniteTiming)
    } else {
        Ok(())
    }
}

fn decode_legacy_json_value(
    value: serde_json::Value,
) -> Result<LegacySpeechSpanRevision, ProjectionSemanticError> {
    validate_legacy_json_tags(&value)?;
    let wire: LegacyTranscriptEventWire =
        serde_json::from_value(value).map_err(|_| ProjectionSemanticError::MalformedValue)?;
    if wire.span_id.trim().is_empty() || wire.source_id.trim().is_empty() {
        return Err(ProjectionSemanticError::IdentityMismatch);
    }
    if wire
        .supersedes
        .as_deref()
        .is_some_and(|reference| reference.trim().is_empty())
    {
        return Err(ProjectionSemanticError::InvalidSupersession);
    }
    wire.try_into().map_err(|error| match error {
        SpeechSpanRevisionError::InvalidCorrelation => {
            ProjectionSemanticError::InvalidRevisionNumber
        }
        SpeechSpanRevisionError::InvalidTiming | SpeechSpanRevisionError::InvalidConfidence => {
            ProjectionSemanticError::MalformedValue
        }
        _ => ProjectionSemanticError::MalformedValue,
    })
}

fn validate_legacy_json_tags(value: &serde_json::Value) -> Result<(), ProjectionSemanticError> {
    let object = value
        .as_object()
        .ok_or(ProjectionSemanticError::MalformedValue)?;
    if let Some(stability) = object.get("stability")
        && !matches!(stability.as_str(), Some("partial" | "final"))
    {
        return Err(ProjectionSemanticError::UnsupportedEnumTag);
    }
    for field in [
        "provider_item_id",
        "transcript_segment_id",
        "speaker_id",
        "speaker_label",
        "channel",
        "supersedes",
        "turn_id",
        "raw_event_ref",
    ] {
        validate_legacy_option(object.get(field), serde_json::Value::is_string)?;
    }
    for field in ["start_time", "end_time", "confidence"] {
        validate_legacy_option(object.get(field), serde_json::Value::is_number)?;
    }
    for field in ["capture_latency_ms", "asr_latency_ms"] {
        validate_legacy_option(object.get(field), |value| value.as_u64().is_some())?;
    }
    if let Some(value) = object.get("is_final")
        && !value.is_boolean()
    {
        return Err(ProjectionSemanticError::UnsupportedBooleanTag);
    }
    if let Some(value) = object.get("end_of_turn")
        && !value.is_null()
        && !value.is_boolean()
    {
        return Err(ProjectionSemanticError::UnsupportedBooleanTag);
    }
    Ok(())
}

fn validate_legacy_option(
    value: Option<&serde_json::Value>,
    valid_present: impl FnOnce(&serde_json::Value) -> bool,
) -> Result<(), ProjectionSemanticError> {
    match value {
        None | Some(serde_json::Value::Null) => Ok(()),
        Some(value) if valid_present(value) => Ok(()),
        Some(_) => Err(ProjectionSemanticError::UnsupportedOptionTag),
    }
}

impl<'de> serde::Deserialize<'de> for CompatibleSpeechSpanRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let value = <serde_json::Value as serde::Deserialize>::deserialize(deserializer)?;
        Self::decode_json_value(value)
            .map_err(|_| D::Error::custom("invalid compatible speech span revision"))
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

#[cfg(test)]
mod tests {
    use super::{
        CompatibleSpeechSpanRevision, LegacyEvidence, ProjectionSemanticConfidence,
        ProjectionSemanticSupersession, ProjectionSemanticTiming, SpanObservation,
        SpeechChannelFidelity, SpeechConfidence, SpeechSpanRevisionError,
        SpeechSpanRevisionNormalizer, SpeechSpanStability, SpeechSpeakerFidelity,
        SpeechSpeakerValue, SpeechTiming, SpeechTimingPrecision, SpeechTurnFidelity,
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
            timing: SpeechTiming::Unavailable {},
            confidence: SpeechConfidence::Unavailable {},
            turn: SpeechTurnFidelity::Unavailable {},
            speaker: SpeechSpeakerFidelity::Unavailable {},
            channel: SpeechChannelFidelity::Unavailable {},
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
        observation.turn = SpeechTurnFidelity::Provider {
            value: SpeechTurnValue {
                turn_id: "provider-turn-7".into(),
                end_of_turn: true,
            },
        };
        observation.speaker = SpeechSpeakerFidelity::App {
            value: SpeechSpeakerValue {
                speaker_id: Some("speaker-7".into()),
                speaker_label: Some("redacted in debug".into()),
            },
        };
        observation.channel = SpeechChannelFidelity::Provider {
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
                SpeechConfidence::Unavailable {},
                SpeechTurnFidelity::Unavailable {},
                SpeechSpeakerFidelity::Unavailable {},
                SpeechChannelFidelity::Unavailable {},
                SpeechSpanRevisionError::InvalidTiming,
            ),
            (
                SpeechTiming::AppEstimated {
                    start_time: -0.1,
                    end_time: 1.0,
                },
                SpeechConfidence::Unavailable {},
                SpeechTurnFidelity::Unavailable {},
                SpeechSpeakerFidelity::Unavailable {},
                SpeechChannelFidelity::Unavailable {},
                SpeechSpanRevisionError::InvalidTiming,
            ),
            (
                SpeechTiming::Provider {
                    precision: SpeechTimingPrecision::Coarse,
                    start_time: 2.0,
                    end_time: 1.0,
                },
                SpeechConfidence::Unavailable {},
                SpeechTurnFidelity::Unavailable {},
                SpeechSpeakerFidelity::Unavailable {},
                SpeechChannelFidelity::Unavailable {},
                SpeechSpanRevisionError::InvalidTiming,
            ),
            (
                SpeechTiming::Unavailable {},
                SpeechConfidence::App { value: 1.1 },
                SpeechTurnFidelity::Unavailable {},
                SpeechSpeakerFidelity::Unavailable {},
                SpeechChannelFidelity::Unavailable {},
                SpeechSpanRevisionError::InvalidConfidence,
            ),
            (
                SpeechTiming::Unavailable {},
                SpeechConfidence::Provider { value: f32::NAN },
                SpeechTurnFidelity::Unavailable {},
                SpeechSpeakerFidelity::Unavailable {},
                SpeechChannelFidelity::Unavailable {},
                SpeechSpanRevisionError::InvalidConfidence,
            ),
            (
                SpeechTiming::Unavailable {},
                SpeechConfidence::Unavailable {},
                SpeechTurnFidelity::Provider {
                    value: SpeechTurnValue {
                        turn_id: " ".into(),
                        end_of_turn: false,
                    },
                },
                SpeechSpeakerFidelity::Unavailable {},
                SpeechChannelFidelity::Unavailable {},
                SpeechSpanRevisionError::InvalidTurn,
            ),
            (
                SpeechTiming::Unavailable {},
                SpeechConfidence::Unavailable {},
                SpeechTurnFidelity::Unavailable {},
                SpeechSpeakerFidelity::App {
                    value: SpeechSpeakerValue {
                        speaker_id: None,
                        speaker_label: None,
                    },
                },
                SpeechChannelFidelity::Unavailable {},
                SpeechSpanRevisionError::InvalidSpeaker,
            ),
            (
                SpeechTiming::Unavailable {},
                SpeechConfidence::Unavailable {},
                SpeechTurnFidelity::Unavailable {},
                SpeechSpeakerFidelity::Unavailable {},
                SpeechChannelFidelity::Provider { value: "".into() },
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

    #[test]
    fn compatible_revision_normalizes_only_projection_semantics() {
        let legacy_json = serde_json::json!({
            "span_id": "legacy-span-1",
            "provider": "legacy-provider",
            "source_id": "legacy-source",
            "provider_item_id": "excluded-item-a",
            "transcript_segment_id": "excluded-segment-a",
            "speaker_id": "speaker-legacy",
            "text": "legacy hello",
            "start_time": 0.0,
            "end_time": 1.0,
            "confidence": 0.9,
            "is_final": true,
            "stability": "final",
            "revision_number": 1,
            "turn_id": "legacy-turn",
            "end_of_turn": true,
            "raw_event_ref": "excluded.ref.a",
            "capture_latency_ms": 10,
            "asr_latency_ms": 20,
            "received_at_ms": 1_u64
        });
        let mut operational_mutation = legacy_json.clone();
        operational_mutation["provider_item_id"] = serde_json::json!("excluded-item-b");
        operational_mutation["transcript_segment_id"] = serde_json::json!("excluded-segment-b");
        operational_mutation["raw_event_ref"] = serde_json::json!("excluded.ref.b");
        operational_mutation["capture_latency_ms"] = serde_json::json!(999);
        operational_mutation["asr_latency_ms"] = serde_json::json!(998);
        operational_mutation["received_at_ms"] = serde_json::json!(999_u64);

        let original: CompatibleSpeechSpanRevision =
            serde_json::from_value(legacy_json).expect("decode original legacy row");
        let mutated: CompatibleSpeechSpanRevision = serde_json::from_value(operational_mutation)
            .expect("decode operationally mutated legacy row");
        let original = original
            .projection_semantics()
            .expect("normalize original legacy row");
        let mutated = mutated
            .projection_semantics()
            .expect("normalize mutated legacy row");

        assert!(original == mutated);
        assert_eq!(original.source_ordinal(), None);
        assert_eq!(original.span_id(), "legacy-span-1");
        assert_eq!(original.source_id(), "legacy-source");
        assert_eq!(original.text(), "legacy hello");
        assert_eq!(original.revision_number(), 1);
        assert!(original.is_final());
        assert!(matches!(
            original.timing(),
            super::ProjectionSemanticTiming::LegacyUnspecified {
                start_time: Some(0.0),
                end_time: Some(1.0)
            }
        ));
        assert!(matches!(
            original.confidence(),
            super::ProjectionSemanticConfidence::LegacyUnspecified { value } if *value == 0.9
        ));
    }

    #[test]
    fn v2_operational_mutations_are_absent_from_projection_semantics() {
        let original = SpeechSpanRevisionNormalizer::new()
            .admit(unavailable_observation())
            .expect("admit original v2 row");
        let mut operational_mutation = unavailable_observation();
        operational_mutation.provider_item_id = Some("excluded-provider-item".into());
        operational_mutation.provider_event_ref = Some("excluded.provider.ref".into());
        operational_mutation.capture_latency_ms = Some(999);
        operational_mutation.asr_latency_ms = Some(998);
        operational_mutation.received_at_ms += 999;
        let mutated = SpeechSpanRevisionNormalizer::new()
            .admit(operational_mutation)
            .expect("admit operationally mutated v2 row");

        let original = CompatibleSpeechSpanRevision::V2(original)
            .projection_semantics()
            .expect("normalize original v2 row");
        let mutated = CompatibleSpeechSpanRevision::V2(mutated)
            .projection_semantics()
            .expect("normalize mutated v2 row");
        assert!(original == mutated);
    }

    #[test]
    fn typed_compatible_decode_preserves_unsupported_and_semantic_error_classes() {
        let base = serde_json::json!({
            "span_id": "legacy-span-1",
            "provider": "legacy-provider",
            "source_id": "legacy-source",
            "text": "legacy hello",
            "start_time": 0.0,
            "end_time": 1.0,
            "confidence": 0.9,
            "is_final": true,
            "stability": "final",
            "revision_number": 1,
            "end_of_turn": true,
            "received_at_ms": 1_u64
        });
        let cases = [
            (
                {
                    let mut value = base.clone();
                    value["contract_version"] = serde_json::json!(99);
                    value
                },
                super::ProjectionSemanticError::UnsupportedContractVersion,
            ),
            (
                {
                    let mut value = base.clone();
                    value["stability"] = serde_json::json!("settled");
                    value
                },
                super::ProjectionSemanticError::UnsupportedEnumTag,
            ),
            (
                {
                    let mut value = base.clone();
                    value["speaker_id"] = serde_json::json!([]);
                    value
                },
                super::ProjectionSemanticError::UnsupportedOptionTag,
            ),
            (
                {
                    let mut value = base.clone();
                    value["end_of_turn"] = serde_json::json!("yes");
                    value
                },
                super::ProjectionSemanticError::UnsupportedBooleanTag,
            ),
            (
                {
                    let mut value = base.clone();
                    value["text"] = serde_json::Value::Null;
                    value
                },
                super::ProjectionSemanticError::MalformedValue,
            ),
            (
                {
                    let mut value = base.clone();
                    value["span_id"] = serde_json::json!("");
                    value
                },
                super::ProjectionSemanticError::IdentityMismatch,
            ),
            (
                {
                    let mut value = base.clone();
                    value["revision_number"] = serde_json::json!(2);
                    value["supersedes"] = serde_json::json!("");
                    value
                },
                super::ProjectionSemanticError::InvalidSupersession,
            ),
        ];

        for (value, expected) in cases {
            let error = match CompatibleSpeechSpanRevision::decode_json_value(value) {
                Ok(_) => panic!("invalid compatible input must fail closed"),
                Err(error) => error,
            };
            assert_eq!(error, expected);
        }
    }

    #[test]
    fn hash_rejects_impossible_cross_payload_semantics_before_encoding() {
        use crate::projection_basis_hash_v2::{
            PositionedProjectionSemanticRevision, ProjectionBasisHashV2Error,
            projection_basis_hash_v2,
        };

        let v2 = SpeechSpanRevisionNormalizer::new()
            .admit(unavailable_observation())
            .expect("valid v2");
        let v2 = CompatibleSpeechSpanRevision::V2(v2)
            .projection_semantics()
            .expect("valid v2 semantics");
        let legacy = CompatibleSpeechSpanRevision::decode_json_value(serde_json::json!({
            "span_id": "legacy-span-1",
            "provider": "legacy-provider",
            "source_id": "legacy-source",
            "text": "legacy hello",
            "is_final": true,
            "stability": "final",
            "revision_number": 1,
            "received_at_ms": 1_u64
        }))
        .expect("valid legacy")
        .projection_semantics()
        .expect("valid legacy semantics");

        let mut v2_with_legacy_evidence = v2.clone();
        v2_with_legacy_evidence.timing = ProjectionSemanticTiming::LegacyUnspecified {
            start_time: Some(0.0),
            end_time: Some(1.0),
        };
        let mut legacy_with_provider_evidence = legacy.clone();
        legacy_with_provider_evidence.confidence =
            ProjectionSemanticConfidence::Provider { value: 0.9 };
        let mut empty_legacy_reference = legacy;
        empty_legacy_reference.supersession = ProjectionSemanticSupersession::LegacyReference {
            reference: String::new(),
        };
        let mut identity_mismatch = v2;
        identity_mismatch.source_ordinal = Some(2);

        let cases = [
            (
                v2_with_legacy_evidence,
                ProjectionBasisHashV2Error::UnsupportedSemanticCombination,
            ),
            (
                legacy_with_provider_evidence,
                ProjectionBasisHashV2Error::UnsupportedSemanticCombination,
            ),
            (
                empty_legacy_reference,
                ProjectionBasisHashV2Error::InvalidSupersession,
            ),
            (
                identity_mismatch,
                ProjectionBasisHashV2Error::InvalidIdentity,
            ),
        ];
        for (revision, expected) in cases {
            let positioned = [PositionedProjectionSemanticRevision::new(Some(1), revision)];
            assert_eq!(projection_basis_hash_v2(&positioned), Err(expected));
        }
    }
}
