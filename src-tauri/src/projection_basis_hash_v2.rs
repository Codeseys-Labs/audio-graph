//! Dormant Projection Basis hash-v2 conformance kernel.

use std::collections::HashMap;

use sha2::{Digest, Sha256};

use crate::speech_span_revision::{
    ProjectionSemanticChannel, ProjectionSemanticConfidence, ProjectionSemanticPayloadKind,
    ProjectionSemanticRevision, ProjectionSemanticSpeaker, ProjectionSemanticStability,
    ProjectionSemanticSupersession, ProjectionSemanticTiming, ProjectionSemanticTurn,
};

const DOMAIN_SEPARATOR: &[u8] = b"audio-graph:projection-basis:v2";

/// One selected logical span and the sequence of its first canonical Accepted record.
///
/// The position selects order only; its numeric value is never encoded.
#[derive(Clone, PartialEq)]
pub struct PositionedProjectionSemanticRevision {
    first_accepted_sequence: Option<u64>,
    revision: ProjectionSemanticRevision,
}

impl PositionedProjectionSemanticRevision {
    pub fn new(first_accepted_sequence: Option<u64>, revision: ProjectionSemanticRevision) -> Self {
        Self {
            first_accepted_sequence,
            revision,
        }
    }

    pub fn first_accepted_sequence(&self) -> Option<u64> {
        self.first_accepted_sequence
    }

    pub fn revision(&self) -> &ProjectionSemanticRevision {
        &self.revision
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ProjectionBasisHashV2Error {
    #[error("projection basis hash v2 input is missing a first Accepted position")]
    MissingFirstAcceptedPosition,
    #[error("projection basis hash v2 input has a duplicate first Accepted position")]
    DuplicateFirstAcceptedPosition,
    #[error("projection basis hash v2 input has a duplicate source ordinal")]
    DuplicateSourceOrdinal,
    #[error("projection basis hash v2 input has a reversed source ordinal")]
    ReversedSourceOrdinal,
    #[error("projection basis hash v2 input has an invalid required string")]
    InvalidRequiredString,
    #[error("projection basis hash v2 input has an invalid revision number")]
    InvalidRevisionNumber,
    #[error("projection basis hash v2 input has an invalid source ordinal")]
    InvalidSourceOrdinal,
    #[error("projection basis hash v2 input has invalid supersession")]
    InvalidSupersession,
    #[error("projection basis hash v2 input has invalid finality")]
    InvalidFinality,
    #[error("projection basis hash v2 input has non-finite timing")]
    NonFiniteTiming,
    #[error("projection basis hash v2 input has invalid timing")]
    InvalidTiming,
    #[error("projection basis hash v2 input has non-finite confidence")]
    NonFiniteConfidence,
    #[error("projection basis hash v2 input has invalid confidence")]
    InvalidConfidence,
    #[error("projection basis hash v2 input has invalid turn evidence")]
    InvalidTurn,
    #[error("projection basis hash v2 input has invalid speaker evidence")]
    InvalidSpeaker,
    #[error("projection basis hash v2 input has invalid channel evidence")]
    InvalidChannel,
    #[error("projection basis hash v2 input exceeds the canonical length limit")]
    LengthOverflow,
}

/// Hash normalized projection semantics using ADR-0036's frozen v2 encoding.
pub fn projection_basis_hash_v2(
    records: &[PositionedProjectionSemanticRevision],
) -> Result<String, ProjectionBasisHashV2Error> {
    let mut ordered = records.iter().collect::<Vec<_>>();
    for record in &ordered {
        if record
            .first_accepted_sequence
            .is_none_or(|sequence| sequence == 0)
        {
            return Err(ProjectionBasisHashV2Error::MissingFirstAcceptedPosition);
        }
    }
    ordered.sort_unstable_by_key(|record| record.first_accepted_sequence);
    if ordered
        .windows(2)
        .any(|pair| pair[0].first_accepted_sequence == pair[1].first_accepted_sequence)
    {
        return Err(ProjectionBasisHashV2Error::DuplicateFirstAcceptedPosition);
    }

    let mut last_ordinal_by_source = HashMap::<&str, u64>::new();
    for record in &ordered {
        validate_revision(&record.revision)?;
        if record.revision.payload_kind() == ProjectionSemanticPayloadKind::SpeechSpanRevisionV2 {
            let ordinal = record
                .revision
                .source_ordinal()
                .ok_or(ProjectionBasisHashV2Error::InvalidSourceOrdinal)?;
            if let Some(previous) =
                last_ordinal_by_source.insert(record.revision.source_id(), ordinal)
            {
                if ordinal == previous {
                    return Err(ProjectionBasisHashV2Error::DuplicateSourceOrdinal);
                }
                if ordinal < previous {
                    return Err(ProjectionBasisHashV2Error::ReversedSourceOrdinal);
                }
            }
        }
    }

    let mut encoder = Encoder::default();
    encoder.bytes.extend_from_slice(DOMAIN_SEPARATOR);
    encoder.byte(0);
    encoder.unsigned(
        ordered
            .len()
            .try_into()
            .map_err(|_| ProjectionBasisHashV2Error::LengthOverflow)?,
    );
    for record in ordered {
        encode_revision(&mut encoder, &record.revision)?;
    }
    let digest = Sha256::digest(&encoder.bytes);
    Ok(format!("sha256:{}", hex::encode(digest)))
}

fn validate_revision(
    revision: &ProjectionSemanticRevision,
) -> Result<(), ProjectionBasisHashV2Error> {
    for value in [
        revision.span_id(),
        revision.source_id(),
        revision.provider(),
        revision.text(),
    ] {
        if value.trim().is_empty() {
            return Err(ProjectionBasisHashV2Error::InvalidRequiredString);
        }
    }
    if revision.revision_number() == 0 {
        return Err(ProjectionBasisHashV2Error::InvalidRevisionNumber);
    }
    match revision.payload_kind() {
        ProjectionSemanticPayloadKind::LegacyV1 => {
            if revision.source_ordinal().is_some()
                || matches!(
                    revision.supersession(),
                    ProjectionSemanticSupersession::V2Exact { .. }
                )
            {
                return Err(ProjectionBasisHashV2Error::InvalidSupersession);
            }
        }
        ProjectionSemanticPayloadKind::SpeechSpanRevisionV2 => {
            if revision.source_ordinal().is_none_or(|ordinal| ordinal == 0) {
                return Err(ProjectionBasisHashV2Error::InvalidSourceOrdinal);
            }
            if revision.is_final() != (revision.stability() == ProjectionSemanticStability::Final) {
                return Err(ProjectionBasisHashV2Error::InvalidFinality);
            }
            match (revision.revision_number(), revision.supersession()) {
                (1, ProjectionSemanticSupersession::Absent) => {}
                (
                    number,
                    ProjectionSemanticSupersession::V2Exact {
                        span_id,
                        revision_number,
                    },
                ) if number > 1
                    && span_id == revision.span_id()
                    && revision_number.checked_add(1) == Some(number) => {}
                _ => return Err(ProjectionBasisHashV2Error::InvalidSupersession),
            }
        }
    }
    validate_timing(revision.timing())?;
    validate_confidence(revision.confidence())?;
    validate_turn(revision.turn())?;
    validate_speaker(revision.speaker())?;
    validate_channel(revision.channel())
}

fn validate_timing(timing: &ProjectionSemanticTiming) -> Result<(), ProjectionBasisHashV2Error> {
    let validate_pair = |start: f64, end: f64| {
        if !start.is_finite() || !end.is_finite() {
            Err(ProjectionBasisHashV2Error::NonFiniteTiming)
        } else if start < 0.0 || end < 0.0 || start > end {
            Err(ProjectionBasisHashV2Error::InvalidTiming)
        } else {
            Ok(())
        }
    };
    match timing {
        ProjectionSemanticTiming::Unavailable => Ok(()),
        ProjectionSemanticTiming::LegacyUnspecified {
            start_time,
            end_time,
        } => {
            for value in [start_time, end_time].into_iter().flatten() {
                if !value.is_finite() {
                    return Err(ProjectionBasisHashV2Error::NonFiniteTiming);
                }
                if *value < 0.0 {
                    return Err(ProjectionBasisHashV2Error::InvalidTiming);
                }
            }
            if matches!((start_time, end_time), (Some(start), Some(end)) if start > end) {
                Err(ProjectionBasisHashV2Error::InvalidTiming)
            } else {
                Ok(())
            }
        }
        ProjectionSemanticTiming::AppEstimated {
            start_time,
            end_time,
        }
        | ProjectionSemanticTiming::ProviderCoarse {
            start_time,
            end_time,
        }
        | ProjectionSemanticTiming::ProviderExact {
            start_time,
            end_time,
        } => validate_pair(*start_time, *end_time),
    }
}

fn validate_confidence(
    confidence: &ProjectionSemanticConfidence,
) -> Result<(), ProjectionBasisHashV2Error> {
    let value = match confidence {
        ProjectionSemanticConfidence::Unavailable => return Ok(()),
        ProjectionSemanticConfidence::LegacyUnspecified { value }
        | ProjectionSemanticConfidence::App { value }
        | ProjectionSemanticConfidence::Provider { value } => *value,
    };
    if !value.is_finite() {
        Err(ProjectionBasisHashV2Error::NonFiniteConfidence)
    } else if !(0.0..=1.0).contains(&value) {
        Err(ProjectionBasisHashV2Error::InvalidConfidence)
    } else {
        Ok(())
    }
}

fn validate_turn(turn: &ProjectionSemanticTurn) -> Result<(), ProjectionBasisHashV2Error> {
    match turn {
        ProjectionSemanticTurn::Unavailable | ProjectionSemanticTurn::LegacyUnspecified { .. } => {
            Ok(())
        }
        ProjectionSemanticTurn::App { turn_id, .. }
        | ProjectionSemanticTurn::Provider { turn_id, .. }
            if !turn_id.trim().is_empty() =>
        {
            Ok(())
        }
        ProjectionSemanticTurn::App { .. } | ProjectionSemanticTurn::Provider { .. } => {
            Err(ProjectionBasisHashV2Error::InvalidTurn)
        }
    }
}

fn valid_optional_string(value: &Option<String>) -> bool {
    value
        .as_deref()
        .is_none_or(|value| !value.trim().is_empty())
}

fn validate_speaker(speaker: &ProjectionSemanticSpeaker) -> Result<(), ProjectionBasisHashV2Error> {
    match speaker {
        ProjectionSemanticSpeaker::Unavailable
        | ProjectionSemanticSpeaker::LegacyUnspecified { .. } => Ok(()),
        ProjectionSemanticSpeaker::App {
            speaker_id,
            speaker_label,
        }
        | ProjectionSemanticSpeaker::Provider {
            speaker_id,
            speaker_label,
        } if valid_optional_string(speaker_id)
            && valid_optional_string(speaker_label)
            && (speaker_id.is_some() || speaker_label.is_some()) =>
        {
            Ok(())
        }
        ProjectionSemanticSpeaker::App { .. } | ProjectionSemanticSpeaker::Provider { .. } => {
            Err(ProjectionBasisHashV2Error::InvalidSpeaker)
        }
    }
}

fn validate_channel(channel: &ProjectionSemanticChannel) -> Result<(), ProjectionBasisHashV2Error> {
    match channel {
        ProjectionSemanticChannel::Unavailable
        | ProjectionSemanticChannel::LegacyUnspecified { .. } => Ok(()),
        ProjectionSemanticChannel::App { value }
        | ProjectionSemanticChannel::Provider { value }
            if !value.trim().is_empty() =>
        {
            Ok(())
        }
        ProjectionSemanticChannel::App { .. } | ProjectionSemanticChannel::Provider { .. } => {
            Err(ProjectionBasisHashV2Error::InvalidChannel)
        }
    }
}

#[derive(Default)]
struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    fn byte(&mut self, value: u8) {
        self.bytes.push(value);
    }

    fn unsigned(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_be_bytes());
    }

    fn string(&mut self, value: &str) -> Result<(), ProjectionBasisHashV2Error> {
        self.unsigned(
            value
                .len()
                .try_into()
                .map_err(|_| ProjectionBasisHashV2Error::LengthOverflow)?,
        );
        self.bytes.extend_from_slice(value.as_bytes());
        Ok(())
    }

    fn boolean(&mut self, value: bool) {
        self.byte(u8::from(value));
    }

    fn optional<T>(
        &mut self,
        value: Option<T>,
        write: impl FnOnce(&mut Self, T) -> Result<(), ProjectionBasisHashV2Error>,
    ) -> Result<(), ProjectionBasisHashV2Error> {
        match value {
            None => self.byte(0),
            Some(value) => {
                self.byte(1);
                write(self, value)?;
            }
        }
        Ok(())
    }

    fn float64(&mut self, value: f64) {
        let bits = if value == 0.0 { 0 } else { value.to_bits() };
        self.bytes.extend_from_slice(&bits.to_be_bytes());
    }

    fn float32(&mut self, value: f32) {
        let bits = if value == 0.0 { 0 } else { value.to_bits() };
        self.bytes.extend_from_slice(&bits.to_be_bytes());
    }
}

fn encode_revision(
    encoder: &mut Encoder,
    revision: &ProjectionSemanticRevision,
) -> Result<(), ProjectionBasisHashV2Error> {
    encoder.byte(0xa0);
    encoder.byte(0x01);
    encoder.byte(match revision.payload_kind() {
        ProjectionSemanticPayloadKind::LegacyV1 => 1,
        ProjectionSemanticPayloadKind::SpeechSpanRevisionV2 => 2,
    });
    encoder.byte(0x02);
    encoder.string(revision.span_id())?;
    encoder.byte(0x03);
    encoder.string(revision.source_id())?;
    encoder.byte(0x04);
    encoder.optional(revision.source_ordinal(), |encoder, value| {
        encoder.unsigned(value);
        Ok(())
    })?;
    encoder.byte(0x05);
    encoder.string(revision.provider())?;
    encoder.byte(0x06);
    encoder.string(revision.text())?;
    encoder.byte(0x07);
    encoder.byte(match revision.stability() {
        ProjectionSemanticStability::Partial => 1,
        ProjectionSemanticStability::Final => 2,
    });
    encoder.byte(0x08);
    encoder.boolean(revision.is_final());
    encoder.byte(0x09);
    encoder.unsigned(revision.revision_number());
    encode_supersession(encoder, revision.supersession())?;
    encode_timing(encoder, revision.timing());
    encode_confidence(encoder, revision.confidence());
    encode_turn(encoder, revision.turn())?;
    encode_speaker(encoder, revision.speaker())?;
    encode_channel(encoder, revision.channel())?;
    encoder.byte(0xaf);
    Ok(())
}

fn encode_supersession(
    encoder: &mut Encoder,
    supersession: &ProjectionSemanticSupersession,
) -> Result<(), ProjectionBasisHashV2Error> {
    encoder.byte(0x0a);
    match supersession {
        ProjectionSemanticSupersession::Absent => encoder.byte(0),
        ProjectionSemanticSupersession::LegacyReference { reference } => {
            encoder.byte(1);
            encoder.string(reference)?;
        }
        ProjectionSemanticSupersession::V2Exact {
            span_id,
            revision_number,
        } => {
            encoder.byte(2);
            encoder.string(span_id)?;
            encoder.unsigned(*revision_number);
        }
    }
    Ok(())
}

fn encode_timing(encoder: &mut Encoder, timing: &ProjectionSemanticTiming) {
    encoder.byte(0x0b);
    match timing {
        ProjectionSemanticTiming::Unavailable => encoder.byte(0),
        ProjectionSemanticTiming::LegacyUnspecified {
            start_time,
            end_time,
        } => {
            encoder.byte(1);
            encoder
                .optional(*start_time, |encoder, value| {
                    encoder.float64(value);
                    Ok(())
                })
                .expect("float encoding cannot fail");
            encoder
                .optional(*end_time, |encoder, value| {
                    encoder.float64(value);
                    Ok(())
                })
                .expect("float encoding cannot fail");
        }
        ProjectionSemanticTiming::AppEstimated {
            start_time,
            end_time,
        }
        | ProjectionSemanticTiming::ProviderCoarse {
            start_time,
            end_time,
        }
        | ProjectionSemanticTiming::ProviderExact {
            start_time,
            end_time,
        } => {
            encoder.byte(match timing {
                ProjectionSemanticTiming::AppEstimated { .. } => 2,
                ProjectionSemanticTiming::ProviderCoarse { .. } => 3,
                ProjectionSemanticTiming::ProviderExact { .. } => 4,
                _ => unreachable!(),
            });
            encoder.float64(*start_time);
            encoder.float64(*end_time);
        }
    }
}

fn encode_confidence(encoder: &mut Encoder, confidence: &ProjectionSemanticConfidence) {
    encoder.byte(0x0c);
    match confidence {
        ProjectionSemanticConfidence::Unavailable => encoder.byte(0),
        ProjectionSemanticConfidence::LegacyUnspecified { value } => {
            encoder.byte(1);
            encoder.float32(*value);
        }
        ProjectionSemanticConfidence::App { value } => {
            encoder.byte(2);
            encoder.float32(*value);
        }
        ProjectionSemanticConfidence::Provider { value } => {
            encoder.byte(3);
            encoder.float32(*value);
        }
    }
}

fn encode_turn(
    encoder: &mut Encoder,
    turn: &ProjectionSemanticTurn,
) -> Result<(), ProjectionBasisHashV2Error> {
    encoder.byte(0x0d);
    match turn {
        ProjectionSemanticTurn::Unavailable => encoder.byte(0),
        ProjectionSemanticTurn::LegacyUnspecified {
            turn_id,
            end_of_turn,
        } => {
            encoder.byte(1);
            encoder.optional(turn_id.as_deref(), Encoder::string)?;
            encoder.optional(*end_of_turn, |encoder, value| {
                encoder.boolean(value);
                Ok(())
            })?;
        }
        ProjectionSemanticTurn::App {
            turn_id,
            end_of_turn,
        }
        | ProjectionSemanticTurn::Provider {
            turn_id,
            end_of_turn,
        } => {
            encoder.byte(if matches!(turn, ProjectionSemanticTurn::App { .. }) {
                2
            } else {
                3
            });
            encoder.string(turn_id)?;
            encoder.boolean(*end_of_turn);
        }
    }
    Ok(())
}

fn encode_speaker(
    encoder: &mut Encoder,
    speaker: &ProjectionSemanticSpeaker,
) -> Result<(), ProjectionBasisHashV2Error> {
    encoder.byte(0x0e);
    match speaker {
        ProjectionSemanticSpeaker::Unavailable => encoder.byte(0),
        ProjectionSemanticSpeaker::LegacyUnspecified {
            speaker_id,
            speaker_label,
        }
        | ProjectionSemanticSpeaker::App {
            speaker_id,
            speaker_label,
        }
        | ProjectionSemanticSpeaker::Provider {
            speaker_id,
            speaker_label,
        } => {
            encoder.byte(match speaker {
                ProjectionSemanticSpeaker::LegacyUnspecified { .. } => 1,
                ProjectionSemanticSpeaker::App { .. } => 2,
                ProjectionSemanticSpeaker::Provider { .. } => 3,
                _ => unreachable!(),
            });
            encoder.optional(speaker_id.as_deref(), Encoder::string)?;
            encoder.optional(speaker_label.as_deref(), Encoder::string)?;
        }
    }
    Ok(())
}

fn encode_channel(
    encoder: &mut Encoder,
    channel: &ProjectionSemanticChannel,
) -> Result<(), ProjectionBasisHashV2Error> {
    encoder.byte(0x0f);
    match channel {
        ProjectionSemanticChannel::Unavailable => encoder.byte(0),
        ProjectionSemanticChannel::LegacyUnspecified { value }
        | ProjectionSemanticChannel::App { value }
        | ProjectionSemanticChannel::Provider { value } => {
            encoder.byte(match channel {
                ProjectionSemanticChannel::LegacyUnspecified { .. } => 1,
                ProjectionSemanticChannel::App { .. } => 2,
                ProjectionSemanticChannel::Provider { .. } => 3,
                _ => unreachable!(),
            });
            encoder.string(value)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        PositionedProjectionSemanticRevision, ProjectionBasisHashV2Error, projection_basis_hash_v2,
    };
    use crate::speech_span_revision::{
        CompatibleSpeechSpanRevision, ProjectionSemanticChannel, ProjectionSemanticConfidence,
        ProjectionSemanticSpeaker, ProjectionSemanticStability, ProjectionSemanticSupersession,
        ProjectionSemanticTiming, ProjectionSemanticTurn,
    };

    #[derive(serde::Deserialize)]
    struct GoldenCatalog {
        goldens: Vec<Golden>,
    }

    #[derive(serde::Deserialize)]
    struct Golden {
        name: String,
        expected_digest: String,
        records: Vec<GoldenRecord>,
    }

    #[derive(serde::Deserialize)]
    struct GoldenRecord {
        first_accepted_sequence: u64,
        revision: CompatibleSpeechSpanRevision,
    }

    #[test]
    fn projection_basis_hash_v2_reproduces_every_design_golden() {
        let catalog = catalog();

        for golden in catalog.goldens {
            let records = golden
                .records
                .iter()
                .map(|record| {
                    PositionedProjectionSemanticRevision::new(
                        Some(record.first_accepted_sequence),
                        record
                            .revision
                            .projection_semantics()
                            .expect("golden revision must normalize"),
                    )
                })
                .collect::<Vec<_>>();
            assert_eq!(
                projection_basis_hash_v2(&records).expect("golden must hash"),
                golden.expected_digest,
                "{}",
                golden.name
            );
        }
    }

    fn catalog() -> GoldenCatalog {
        serde_json::from_str(include_str!(
            "../fixtures/projection_basis_hash_v2/goldens.json"
        ))
        .expect("golden catalog must deserialize")
    }

    fn positioned(golden: &Golden) -> Vec<PositionedProjectionSemanticRevision> {
        golden
            .records
            .iter()
            .map(|record| {
                PositionedProjectionSemanticRevision::new(
                    Some(record.first_accepted_sequence),
                    record
                        .revision
                        .projection_semantics()
                        .expect("golden revision must normalize"),
                )
            })
            .collect()
    }

    #[test]
    fn canonicalizes_negative_zero_and_refuses_non_finite_values() {
        let catalog = catalog();
        let baseline = positioned(&catalog.goldens[0]);
        let expected = projection_basis_hash_v2(&baseline).expect("baseline must hash");

        let mut negative_zero = baseline.clone();
        negative_zero[0].revision.timing = ProjectionSemanticTiming::AppEstimated {
            start_time: -0.0,
            end_time: 1.5,
        };
        assert_eq!(
            projection_basis_hash_v2(&negative_zero).expect("negative zero must hash"),
            expected
        );

        let mut non_finite_timing = baseline.clone();
        non_finite_timing[0].revision.timing = ProjectionSemanticTiming::AppEstimated {
            start_time: f64::NAN,
            end_time: 1.5,
        };
        assert_eq!(
            projection_basis_hash_v2(&non_finite_timing),
            Err(ProjectionBasisHashV2Error::NonFiniteTiming)
        );

        let mut non_finite_confidence = baseline;
        non_finite_confidence[0].revision.confidence =
            ProjectionSemanticConfidence::Provider { value: f32::NAN };
        let error = projection_basis_hash_v2(&non_finite_confidence)
            .expect_err("non-finite confidence must not produce a digest");
        assert_eq!(error, ProjectionBasisHashV2Error::NonFiniteConfidence);
        let debug = format!("{error:?}");
        for forbidden in ["hello", "fixture-provider", "source-stream-a"] {
            assert!(!debug.contains(forbidden), "errors must be content-free");
        }
    }

    #[test]
    fn position_and_source_order_fail_closed_without_repair() {
        let catalog = catalog();
        let baseline = positioned(&catalog.goldens[3]);

        let missing = [PositionedProjectionSemanticRevision::new(
            None,
            baseline[0].revision.clone(),
        )];
        assert_eq!(
            projection_basis_hash_v2(&missing),
            Err(ProjectionBasisHashV2Error::MissingFirstAcceptedPosition)
        );

        let duplicate_position = [
            PositionedProjectionSemanticRevision::new(Some(1), baseline[0].revision.clone()),
            PositionedProjectionSemanticRevision::new(Some(1), baseline[1].revision.clone()),
        ];
        assert_eq!(
            projection_basis_hash_v2(&duplicate_position),
            Err(ProjectionBasisHashV2Error::DuplicateFirstAcceptedPosition)
        );

        let reversed_ordinal = [
            PositionedProjectionSemanticRevision::new(Some(1), baseline[1].revision.clone()),
            PositionedProjectionSemanticRevision::new(Some(2), baseline[0].revision.clone()),
        ];
        assert_eq!(
            projection_basis_hash_v2(&reversed_ordinal),
            Err(ProjectionBasisHashV2Error::ReversedSourceOrdinal)
        );

        let mut duplicate_ordinal = baseline;
        duplicate_ordinal[1].revision.source_ordinal = duplicate_ordinal[0].revision.source_ordinal;
        assert_eq!(
            projection_basis_hash_v2(&duplicate_ordinal),
            Err(ProjectionBasisHashV2Error::DuplicateSourceOrdinal)
        );
    }

    #[test]
    fn semantic_include_matrix_changes_digest_and_exact_supersession_is_enforced() {
        type Mutation = Box<dyn Fn(&mut PositionedProjectionSemanticRevision)>;

        let catalog = catalog();
        let baseline = positioned(&catalog.goldens[0]);
        let expected = projection_basis_hash_v2(&baseline).expect("baseline must hash");
        let mutations: Vec<(&str, Mutation)> = vec![
            (
                "span_id",
                Box::new(|record| record.revision.span_id.push('0')),
            ),
            (
                "source_id",
                Box::new(|record| record.revision.source_id.push('b')),
            ),
            (
                "source_ordinal",
                Box::new(|record| record.revision.source_ordinal = Some(2)),
            ),
            (
                "provider",
                Box::new(|record| record.revision.provider.push('b')),
            ),
            ("text", Box::new(|record| record.revision.text.push('!'))),
            (
                "stability/finality",
                Box::new(|record| {
                    record.revision.stability = ProjectionSemanticStability::Partial;
                    record.revision.is_final = false;
                }),
            ),
            (
                "revision/supersession",
                Box::new(|record| {
                    record.revision.revision_number = 2;
                    record.revision.supersession = ProjectionSemanticSupersession::V2Exact {
                        span_id: record.revision.span_id.clone(),
                        revision_number: 1,
                    };
                }),
            ),
            (
                "timing",
                Box::new(|record| {
                    record.revision.timing = ProjectionSemanticTiming::ProviderExact {
                        start_time: 0.0,
                        end_time: 1.5,
                    };
                }),
            ),
            (
                "confidence",
                Box::new(|record| {
                    record.revision.confidence =
                        ProjectionSemanticConfidence::Provider { value: 0.875 };
                }),
            ),
            (
                "turn",
                Box::new(|record| {
                    record.revision.turn = ProjectionSemanticTurn::Provider {
                        turn_id: "turn-a".into(),
                        end_of_turn: true,
                    };
                }),
            ),
            (
                "speaker",
                Box::new(|record| {
                    record.revision.speaker = ProjectionSemanticSpeaker::Provider {
                        speaker_id: Some("speaker-a".into()),
                        speaker_label: None,
                    };
                }),
            ),
            (
                "channel",
                Box::new(|record| {
                    record.revision.channel = ProjectionSemanticChannel::Provider {
                        value: "left".into(),
                    };
                }),
            ),
        ];

        for (name, mutate) in mutations {
            let mut changed = baseline.clone();
            mutate(&mut changed[0]);
            let actual = projection_basis_hash_v2(&changed).expect("valid semantic mutation");
            assert_ne!(actual, expected, "included {name} must change the digest");
        }

        let mut invalid_supersession = baseline;
        invalid_supersession[0].revision.revision_number = 2;
        invalid_supersession[0].revision.supersession = ProjectionSemanticSupersession::V2Exact {
            span_id: "ssp_00000000000000000000000000000000".into(),
            revision_number: 1,
        };
        assert_eq!(
            projection_basis_hash_v2(&invalid_supersession),
            Err(ProjectionBasisHashV2Error::InvalidSupersession)
        );
    }

    #[test]
    fn input_reordering_retains_first_accepted_order_and_digest() {
        let catalog = catalog();
        let mut records = positioned(&catalog.goldens[2]);
        let expected = projection_basis_hash_v2(&records).expect("baseline must hash");
        records.reverse();
        assert_eq!(
            projection_basis_hash_v2(&records).expect("reordered input must hash"),
            expected
        );
    }
}
