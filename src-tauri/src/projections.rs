//! Event-sourced transcript/notes/graph projection contracts.
//!
//! This module defines the durable data model for the dynamic synthesis queue:
//! transcript span revisions are the source events, projection jobs record the
//! exact basis they were built from, and projection patches carry replayable
//! operations for notes and graph state. Wiring these types into persistence and
//! the LLM queue is tracked separately by `audio-graph-ad44`.

use std::collections::BTreeMap;
use std::fmt;

use crate::events::{
    AsrSpanRevisionPayload, AsrSpanStability, DiarizationSpanRevisionPayload,
    DiarizationSpanStability,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptEventStability {
    Partial,
    Final,
}

impl From<AsrSpanStability> for TranscriptEventStability {
    fn from(value: AsrSpanStability) -> Self {
        match value {
            AsrSpanStability::Partial => Self::Partial,
            AsrSpanStability::Final => Self::Final,
        }
    }
}

/// Immutable transcript-span revision event, suitable for JSONL persistence.
#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct TranscriptEvent {
    pub span_id: String,
    pub provider: String,
    pub source_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_item_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_segment_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    pub text: String,
    pub start_time: f64,
    pub end_time: f64,
    pub confidence: f32,
    pub is_final: bool,
    pub stability: TranscriptEventStability,
    pub revision_number: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    pub end_of_turn: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_event_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_latency_ms: Option<u64>,
    pub received_at_ms: u64,
}

const REDACTED_DEBUG_VALUE: &str = "<redacted>";

impl fmt::Debug for TranscriptEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TranscriptEvent")
            .field("span_id", &self.span_id)
            .field("provider", &self.provider)
            .field("source_id", &self.source_id)
            .field("provider_item_id", &self.provider_item_id)
            .field("transcript_segment_id", &self.transcript_segment_id)
            .field("speaker_id", &self.speaker_id)
            .field("speaker_label", &self.speaker_label)
            .field("channel", &self.channel)
            .field("text", &REDACTED_DEBUG_VALUE)
            .field("start_time", &self.start_time)
            .field("end_time", &self.end_time)
            .field("confidence", &self.confidence)
            .field("is_final", &self.is_final)
            .field("stability", &self.stability)
            .field("revision_number", &self.revision_number)
            .field("supersedes", &self.supersedes)
            .field("turn_id", &self.turn_id)
            .field("end_of_turn", &self.end_of_turn)
            .field("raw_event_ref", &self.raw_event_ref)
            .field("capture_latency_ms", &self.capture_latency_ms)
            .field("asr_latency_ms", &self.asr_latency_ms)
            .field("received_at_ms", &self.received_at_ms)
            .finish()
    }
}

impl From<AsrSpanRevisionPayload> for TranscriptEvent {
    fn from(payload: AsrSpanRevisionPayload) -> Self {
        Self {
            span_id: payload.span_id,
            provider: payload.provider,
            source_id: payload.source_id,
            provider_item_id: payload.provider_item_id,
            transcript_segment_id: payload.transcript_segment_id,
            speaker_id: payload.speaker_id,
            speaker_label: payload.speaker_label,
            channel: payload.channel,
            text: payload.text,
            start_time: payload.start_time,
            end_time: payload.end_time,
            confidence: payload.confidence,
            is_final: payload.is_final,
            stability: payload.stability.into(),
            revision_number: payload.revision_number,
            supersedes: payload.supersedes,
            turn_id: payload.turn_id,
            end_of_turn: payload.end_of_turn,
            raw_event_ref: payload.raw_event_ref,
            capture_latency_ms: payload.capture_latency_ms,
            asr_latency_ms: payload.asr_latency_ms,
            received_at_ms: payload.received_at_ms,
        }
    }
}

fn event_is_newer_or_tie_winner(candidate: &TranscriptEvent, current: &TranscriptEvent) -> bool {
    candidate.revision_number > current.revision_number
        || (candidate.revision_number == current.revision_number
            && candidate.received_at_ms > current.received_at_ms)
}

fn latest_transcript_events(events: &[TranscriptEvent]) -> Vec<TranscriptEvent> {
    let mut latest_by_span: BTreeMap<String, TranscriptEvent> = BTreeMap::new();
    for event in events {
        latest_by_span
            .entry(event.span_id.clone())
            .and_modify(|current| {
                if event_is_newer_or_tie_winner(event, current) {
                    *current = event.clone();
                }
            })
            .or_insert_with(|| event.clone());
    }
    latest_by_span.into_values().collect()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectionBasisSpan {
    pub span_id: String,
    pub revision_number: u64,
}

/// Exact transcript/diarization basis for a queued or completed projection.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectionBasis {
    pub span_revisions: Vec<ProjectionBasisSpan>,
    pub diarization_span_revisions: Vec<ProjectionBasisSpan>,
    pub transcript_hash: String,
}

impl ProjectionBasis {
    pub fn from_transcript_events(events: &[TranscriptEvent]) -> Self {
        Self::from_transcript_events_and_speaker_spans(events, &[])
    }

    /// Build a basis from the canonical transcript revisions plus the current
    /// speaker-timeline span revisions. The speaker spans are provider-neutral
    /// [`ProjectionBasisSpan`]s (typically [`SpeakerTimeline::current_basis_spans`]);
    /// passing an empty slice yields a transcript-only basis identical to
    /// [`Self::from_transcript_events`].
    pub fn from_transcript_events_and_speaker_spans(
        events: &[TranscriptEvent],
        speaker_spans: &[ProjectionBasisSpan],
    ) -> Self {
        let latest_events = latest_transcript_events(events);

        Self {
            span_revisions: latest_events
                .iter()
                .map(|event| ProjectionBasisSpan {
                    span_id: event.span_id.clone(),
                    revision_number: event.revision_number,
                })
                .collect(),
            diarization_span_revisions: speaker_spans.to_vec(),
            transcript_hash: transcript_events_hash(&latest_events),
        }
    }
}

/// Stability/finality state for a durable diarization span revision.
///
/// Stored as an independent copy of [`DiarizationSpanStability`] so the durable
/// projection layer does not depend on the live-event enum's representation,
/// mirroring the [`TranscriptEventStability`]/[`AsrSpanStability`] split.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DiarizationEventStability {
    Provisional,
    Stable,
    Final,
}

impl From<DiarizationSpanStability> for DiarizationEventStability {
    fn from(value: DiarizationSpanStability) -> Self {
        match value {
            DiarizationSpanStability::Provisional => Self::Provisional,
            DiarizationSpanStability::Stable => Self::Stable,
            DiarizationSpanStability::Final => Self::Final,
        }
    }
}

/// Immutable diarization (speaker-timeline) span revision, suitable for JSONL
/// persistence. Mirrors [`crate::events::DiarizationSpanRevisionPayload`] while
/// preserving the provider/local separation: `provider` records the engine that
/// produced the attribution and `provider_speaker_id` keeps the raw provider
/// label, but the durable identity is the provider-neutral `span_id` — the
/// provider speaker id is never treated as a stable identity.
#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct DiarizationSpanRevision {
    /// Stable, provider-neutral id for the logical speaker span being revised.
    pub span_id: String,
    /// Engine that produced the attribution (e.g. `deepgram`, `aws_transcribe`,
    /// `soniox`, `local_clustering`). Never used as durable identity.
    pub provider: String,
    /// Logical timeline being revised. Provider diarization may use a source id;
    /// session-level local diarization can use `session`.
    pub timeline_id: String,
    /// Capture source when the attribution is source-local.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_id: Option<String>,
    /// Resolved local/canonical speaker id, distinct from any provider label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_id: Option<String>,
    /// Human-facing label for the resolved speaker.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speaker_label: Option<String>,
    /// Raw provider-supplied speaker identifier. Retained for provenance only;
    /// it is never the durable span identity and may be remapped across
    /// revisions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_speaker_id: Option<String>,
    /// Channel label, only meaningful when source/channel provenance exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    pub start_time: f64,
    pub end_time: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    pub is_final: bool,
    pub stability: DiarizationEventStability,
    pub revision_number: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes: Option<String>,
    pub basis_asr_span_ids: Vec<String>,
    pub basis_transcript_segment_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_event_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub asr_latency_ms: Option<u64>,
    pub received_at_ms: u64,
}

impl fmt::Debug for DiarizationSpanRevision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Speaker labels can carry PII; redact the human-facing label while
        // keeping non-content identity fields for debugging, matching the
        // `TranscriptEvent` Debug redaction policy.
        f.debug_struct("DiarizationSpanRevision")
            .field("span_id", &self.span_id)
            .field("provider", &self.provider)
            .field("timeline_id", &self.timeline_id)
            .field("source_id", &self.source_id)
            .field("speaker_id", &self.speaker_id)
            .field(
                "speaker_label",
                &self.speaker_label.as_ref().map(|_| REDACTED_DEBUG_VALUE),
            )
            .field("provider_speaker_id", &self.provider_speaker_id)
            .field("channel", &self.channel)
            .field("start_time", &self.start_time)
            .field("end_time", &self.end_time)
            .field("confidence", &self.confidence)
            .field("is_final", &self.is_final)
            .field("stability", &self.stability)
            .field("revision_number", &self.revision_number)
            .field("supersedes", &self.supersedes)
            .field("basis_asr_span_ids", &self.basis_asr_span_ids)
            .field(
                "basis_transcript_segment_ids",
                &self.basis_transcript_segment_ids,
            )
            .field("raw_event_ref", &self.raw_event_ref)
            .field("capture_latency_ms", &self.capture_latency_ms)
            .field("asr_latency_ms", &self.asr_latency_ms)
            .field("received_at_ms", &self.received_at_ms)
            .finish()
    }
}

impl From<DiarizationSpanRevisionPayload> for DiarizationSpanRevision {
    fn from(payload: DiarizationSpanRevisionPayload) -> Self {
        Self {
            span_id: payload.span_id,
            provider: payload.provider,
            timeline_id: payload.timeline_id,
            source_id: payload.source_id,
            speaker_id: payload.speaker_id,
            speaker_label: payload.speaker_label,
            // The live payload's provider attribution is carried via the
            // provider/source fields; raw provider speaker ids are not part of
            // the live payload yet, so durable provenance starts unset.
            provider_speaker_id: None,
            channel: payload.channel,
            start_time: payload.start_time,
            end_time: payload.end_time,
            confidence: payload.confidence,
            is_final: payload.is_final,
            stability: payload.stability.into(),
            revision_number: payload.revision_number,
            supersedes: payload.supersedes,
            basis_asr_span_ids: payload.basis_asr_span_ids,
            basis_transcript_segment_ids: payload.basis_transcript_segment_ids,
            raw_event_ref: payload.raw_event_ref,
            capture_latency_ms: payload.capture_latency_ms,
            asr_latency_ms: payload.asr_latency_ms,
            received_at_ms: payload.received_at_ms,
        }
    }
}

/// Provider-neutral speaker-timeline ledger.
///
/// Mirrors [`TranscriptLedger`] revision semantics: a span is identified by its
/// provider-neutral `span_id`, later revisions replace earlier ones (so a
/// `Provisional` attribution is superseded by the `Stable`/`Final` remap of the
/// same `span_id`), stale revisions are rejected, and a same-revision payload
/// that disagrees with the accepted one is rejected as a conflict. The ledger
/// never derives identity from a provider speaker id.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct SpeakerTimeline {
    pub schema_version: u32,
    pub session_id: String,
    pub accepted_event_count: u64,
    pub latest_spans: Vec<DiarizationSpanRevision>,
}

impl SpeakerTimeline {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            session_id: session_id.into(),
            accepted_event_count: 0,
            latest_spans: Vec::new(),
        }
    }

    pub fn replay(
        session_id: impl Into<String>,
        events: impl IntoIterator<Item = DiarizationSpanRevision>,
    ) -> Result<Self, SpeakerTimelineError> {
        let mut timeline = Self::new(session_id);
        for event in events {
            timeline.apply_event(event)?;
        }
        Ok(timeline)
    }

    pub fn apply_event(
        &mut self,
        event: DiarizationSpanRevision,
    ) -> Result<(), SpeakerTimelineError> {
        match self
            .latest_spans
            .iter_mut()
            .find(|span| span.span_id == event.span_id)
        {
            Some(current) if event.revision_number < current.revision_number => {
                Err(SpeakerTimelineError::StaleDiarizationRevision {
                    span_id: event.span_id,
                    current_revision: current.revision_number,
                    incoming_revision: event.revision_number,
                })
            }
            Some(current)
                if event.revision_number == current.revision_number && event != *current =>
            {
                Err(SpeakerTimelineError::ConflictingDiarizationRevision {
                    span_id: event.span_id,
                    revision_number: event.revision_number,
                })
            }
            Some(current) => {
                // Newer (or identical) revision: the later attribution replaces
                // the earlier one, collapsing provisional -> stable remaps.
                *current = event;
                self.accepted_event_count += 1;
                self.sort_latest_spans();
                Ok(())
            }
            None => {
                self.latest_spans.push(event);
                self.accepted_event_count += 1;
                self.sort_latest_spans();
                Ok(())
            }
        }
    }

    /// Distinct resolved speaker ids currently attributed across the timeline.
    pub fn speaker_count(&self) -> usize {
        let mut speakers = std::collections::BTreeSet::new();
        for span in &self.latest_spans {
            if let Some(speaker_id) = &span.speaker_id
                && !speaker_id.trim().is_empty()
            {
                speakers.insert(speaker_id.as_str());
            }
        }
        speakers.len()
    }

    /// Provider-neutral basis spans for the current diarization timeline.
    pub fn current_basis_spans(&self) -> Vec<ProjectionBasisSpan> {
        self.latest_spans
            .iter()
            .map(|span| ProjectionBasisSpan {
                span_id: span.span_id.clone(),
                revision_number: span.revision_number,
            })
            .collect()
    }

    /// Validate the diarization portion of a [`ProjectionBasis`] against the
    /// current timeline, mirroring [`TranscriptLedger::validate_basis`]'s
    /// per-span revision checks.
    pub fn validate_diarization_basis(
        &self,
        basis: &ProjectionBasis,
    ) -> Result<(), ProjectionBasisStaleness> {
        let basis_spans: BTreeMap<&str, u64> = basis
            .diarization_span_revisions
            .iter()
            .map(|span| (span.span_id.as_str(), span.revision_number))
            .collect();

        // Diarization basis is opt-in per projection: a notes/graph patch that
        // did not consume the speaker timeline cites no diarization spans and is
        // not gated by it. Only a projection that explicitly cited speaker spans
        // is validated for full coverage and staleness against the timeline.
        if basis_spans.is_empty() {
            return Ok(());
        }

        let current_spans: BTreeMap<&str, u64> = self
            .latest_spans
            .iter()
            .map(|span| (span.span_id.as_str(), span.revision_number))
            .collect();

        for (span_id, current_revision) in &current_spans {
            match basis_spans.get(*span_id) {
                Some(basis_revision) if basis_revision == current_revision => {}
                Some(basis_revision) => {
                    return Err(ProjectionBasisStaleness::StaleDiarizationSpanRevision {
                        span_id: (*span_id).to_string(),
                        current_revision: *current_revision,
                        basis_revision: *basis_revision,
                    });
                }
                None => {
                    return Err(ProjectionBasisStaleness::MissingCurrentDiarizationSpan {
                        span_id: (*span_id).to_string(),
                        current_revision: *current_revision,
                    });
                }
            }
        }

        for (span_id, basis_revision) in &basis_spans {
            if !current_spans.contains_key(*span_id) {
                return Err(ProjectionBasisStaleness::UnknownDiarizationBasisSpan {
                    span_id: (*span_id).to_string(),
                    basis_revision: *basis_revision,
                });
            }
        }

        Ok(())
    }

    fn sort_latest_spans(&mut self) {
        self.latest_spans.sort_by(|a, b| {
            millis(a.start_time)
                .cmp(&millis(b.start_time))
                .then(millis(a.end_time).cmp(&millis(b.end_time)))
                .then(a.span_id.cmp(&b.span_id))
        });
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SpeakerTimelineError {
    StaleDiarizationRevision {
        span_id: String,
        current_revision: u64,
        incoming_revision: u64,
    },
    ConflictingDiarizationRevision {
        span_id: String,
        revision_number: u64,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct TranscriptLedger {
    pub schema_version: u32,
    pub session_id: String,
    pub accepted_event_count: u64,
    pub latest_spans: Vec<TranscriptEvent>,
}

impl TranscriptLedger {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            session_id: session_id.into(),
            accepted_event_count: 0,
            latest_spans: Vec::new(),
        }
    }

    pub fn replay(
        session_id: impl Into<String>,
        events: impl IntoIterator<Item = TranscriptEvent>,
    ) -> Result<Self, TranscriptLedgerError> {
        let mut ledger = Self::new(session_id);
        for event in events {
            ledger.apply_event(event)?;
        }
        Ok(ledger)
    }

    pub fn apply_event(&mut self, event: TranscriptEvent) -> Result<(), TranscriptLedgerError> {
        match self
            .latest_spans
            .iter_mut()
            .find(|span| span.span_id == event.span_id)
        {
            Some(current) if event.revision_number < current.revision_number => {
                Err(TranscriptLedgerError::StaleTranscriptRevision {
                    span_id: event.span_id,
                    current_revision: current.revision_number,
                    incoming_revision: event.revision_number,
                })
            }
            Some(current)
                if event.revision_number == current.revision_number && event != *current =>
            {
                Err(TranscriptLedgerError::ConflictingTranscriptRevision {
                    span_id: event.span_id,
                    revision_number: event.revision_number,
                })
            }
            Some(current) => {
                *current = event;
                self.accepted_event_count += 1;
                self.sort_latest_spans();
                Ok(())
            }
            None => {
                self.latest_spans.push(event);
                self.accepted_event_count += 1;
                self.sort_latest_spans();
                Ok(())
            }
        }
    }

    pub fn current_basis(&self) -> ProjectionBasis {
        ProjectionBasis::from_transcript_events(&self.latest_spans)
    }

    pub fn validate_basis(&self, basis: &ProjectionBasis) -> Result<(), ProjectionBasisStaleness> {
        self.validate_basis_with_speaker_timeline(basis, None)
    }

    /// Validate a projection basis against this transcript ledger and, when
    /// available, the session [`SpeakerTimeline`].
    ///
    /// Without a timeline, a non-empty diarization basis cannot be checked, so
    /// it is rejected as [`ProjectionBasisStaleness::DiarizationBasisUnavailable`].
    /// With a timeline, the diarization span revisions are validated the same
    /// way transcript spans are.
    pub fn validate_basis_with_speaker_timeline(
        &self,
        basis: &ProjectionBasis,
        speaker_timeline: Option<&SpeakerTimeline>,
    ) -> Result<(), ProjectionBasisStaleness> {
        match speaker_timeline {
            Some(timeline) => timeline.validate_diarization_basis(basis)?,
            None => {
                if !basis.diarization_span_revisions.is_empty() {
                    return Err(ProjectionBasisStaleness::DiarizationBasisUnavailable {
                        count: basis.diarization_span_revisions.len(),
                    });
                }
            }
        }

        let current_basis = self.current_basis();
        let current_spans: BTreeMap<&str, u64> = current_basis
            .span_revisions
            .iter()
            .map(|span| (span.span_id.as_str(), span.revision_number))
            .collect();
        let basis_spans: BTreeMap<&str, u64> = basis
            .span_revisions
            .iter()
            .map(|span| (span.span_id.as_str(), span.revision_number))
            .collect();

        for (span_id, current_revision) in &current_spans {
            match basis_spans.get(*span_id) {
                Some(basis_revision) if basis_revision == current_revision => {}
                Some(basis_revision) => {
                    return Err(ProjectionBasisStaleness::StaleSpanRevision {
                        span_id: (*span_id).to_string(),
                        current_revision: *current_revision,
                        basis_revision: *basis_revision,
                    });
                }
                None => {
                    return Err(ProjectionBasisStaleness::MissingCurrentSpan {
                        span_id: (*span_id).to_string(),
                        current_revision: *current_revision,
                    });
                }
            }
        }

        for (span_id, basis_revision) in &basis_spans {
            if !current_spans.contains_key(*span_id) {
                return Err(ProjectionBasisStaleness::UnknownBasisSpan {
                    span_id: (*span_id).to_string(),
                    basis_revision: *basis_revision,
                });
            }
        }

        if current_basis.transcript_hash != basis.transcript_hash {
            return Err(ProjectionBasisStaleness::TranscriptHashMismatch {
                current_hash: current_basis.transcript_hash,
                basis_hash: basis.transcript_hash.clone(),
            });
        }

        Ok(())
    }

    pub fn is_basis_current(&self, basis: &ProjectionBasis) -> bool {
        self.validate_basis(basis).is_ok()
    }

    fn sort_latest_spans(&mut self) {
        self.latest_spans.sort_by(|a, b| {
            millis(a.start_time)
                .cmp(&millis(b.start_time))
                .then(millis(a.end_time).cmp(&millis(b.end_time)))
                .then(a.span_id.cmp(&b.span_id))
        });
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptLedgerError {
    StaleTranscriptRevision {
        span_id: String,
        current_revision: u64,
        incoming_revision: u64,
    },
    ConflictingTranscriptRevision {
        span_id: String,
        revision_number: u64,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectionBasisStaleness {
    MissingCurrentSpan {
        span_id: String,
        current_revision: u64,
    },
    UnknownBasisSpan {
        span_id: String,
        basis_revision: u64,
    },
    StaleSpanRevision {
        span_id: String,
        current_revision: u64,
        basis_revision: u64,
    },
    TranscriptHashMismatch {
        current_hash: String,
        basis_hash: String,
    },
    DiarizationBasisUnavailable {
        count: usize,
    },
    MissingCurrentDiarizationSpan {
        span_id: String,
        current_revision: u64,
    },
    UnknownDiarizationBasisSpan {
        span_id: String,
        basis_revision: u64,
    },
    StaleDiarizationSpanRevision {
        span_id: String,
        current_revision: u64,
        basis_revision: u64,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionKind {
    Notes,
    Graph,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionPriority {
    Realtime,
    Background,
    Replay,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectionJob {
    pub id: String,
    pub session_id: String,
    pub kind: ProjectionKind,
    pub basis: ProjectionBasis,
    pub priority: ProjectionPriority,
    pub queued_at_ms: u64,
}

#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct ProjectionPatch {
    pub sequence: u64,
    pub kind: ProjectionKind,
    pub llm_request_id: String,
    pub basis: ProjectionBasis,
    pub operations: Vec<ProjectionOperation>,
    pub confidence: f32,
    pub provenance: ProjectionProvenance,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_at_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_latency_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apply_latency_ms: Option<u64>,
    pub created_at_ms: u64,
}

impl fmt::Debug for ProjectionPatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operations: Vec<DebugProjectionOperation<'_>> = self
            .operations
            .iter()
            .map(DebugProjectionOperation)
            .collect();

        f.debug_struct("ProjectionPatch")
            .field("sequence", &self.sequence)
            .field("kind", &self.kind)
            .field("llm_request_id", &self.llm_request_id)
            .field("basis", &self.basis)
            .field("operations", &operations)
            .field("confidence", &self.confidence)
            .field("provenance", &self.provenance)
            .field("queued_at_ms", &self.queued_at_ms)
            .field("generation_latency_ms", &self.generation_latency_ms)
            .field("apply_latency_ms", &self.apply_latency_ms)
            .field("created_at_ms", &self.created_at_ms)
            .finish()
    }
}

struct DebugProjectionOperation<'a>(&'a ProjectionOperation);

impl fmt::Debug for DebugProjectionOperation<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            ProjectionOperation::UpsertNote {
                id,
                title: _,
                body: _,
                tags,
            } => f
                .debug_struct("UpsertNote")
                .field("id", id)
                .field("title", &REDACTED_DEBUG_VALUE)
                .field("body", &REDACTED_DEBUG_VALUE)
                .field("tags", tags)
                .finish(),
            ProjectionOperation::DeleteNote { id } => {
                f.debug_struct("DeleteNote").field("id", id).finish()
            }
            ProjectionOperation::ReorderNote { id, after_id } => f
                .debug_struct("ReorderNote")
                .field("id", id)
                .field("after_id", after_id)
                .finish(),
            ProjectionOperation::UpsertGraphNode {
                id,
                name: _,
                entity_type: _,
                description,
            } => f
                .debug_struct("UpsertGraphNode")
                .field("id", id)
                .field("name", &REDACTED_DEBUG_VALUE)
                .field("entity_type", &REDACTED_DEBUG_VALUE)
                .field(
                    "description",
                    &description.as_ref().map(|_| REDACTED_DEBUG_VALUE),
                )
                .finish(),
            ProjectionOperation::RemoveGraphNode { id } => {
                f.debug_struct("RemoveGraphNode").field("id", id).finish()
            }
            ProjectionOperation::InvalidateGraphNode { id } => f
                .debug_struct("InvalidateGraphNode")
                .field("id", id)
                .finish(),
            ProjectionOperation::UpsertGraphEdge {
                id,
                source,
                target,
                relation_type: _,
                label,
                weight,
            } => f
                .debug_struct("UpsertGraphEdge")
                .field("id", id)
                .field("source", source)
                .field("target", target)
                .field("relation_type", &REDACTED_DEBUG_VALUE)
                .field("label", &label.as_ref().map(|_| REDACTED_DEBUG_VALUE))
                .field("weight", weight)
                .finish(),
            ProjectionOperation::RemoveGraphEdge { id } => {
                f.debug_struct("RemoveGraphEdge").field("id", id).finish()
            }
            ProjectionOperation::InvalidateGraphEdge { id } => f
                .debug_struct("InvalidateGraphEdge")
                .field("id", id)
                .finish(),
            ProjectionOperation::StrengthenGraphEdge { id, weight_delta } => f
                .debug_struct("StrengthenGraphEdge")
                .field("id", id)
                .field("weight_delta", weight_delta)
                .finish(),
            ProjectionOperation::WeakenGraphEdge { id, weight_delta } => f
                .debug_struct("WeakenGraphEdge")
                .field("id", id)
                .field("weight_delta", weight_delta)
                .finish(),
            ProjectionOperation::MergeGraphNodes {
                source_id,
                target_id,
            } => f
                .debug_struct("MergeGraphNodes")
                .field("source_id", source_id)
                .field("target_id", target_id)
                .finish(),
            ProjectionOperation::SplitGraphNode {
                id,
                replacement_nodes,
            } => {
                let nodes: Vec<DebugGraphNodeDraft<'_>> =
                    replacement_nodes.iter().map(DebugGraphNodeDraft).collect();
                f.debug_struct("SplitGraphNode")
                    .field("id", id)
                    .field("replacement_nodes", &nodes)
                    .finish()
            }
        }
    }
}

struct DebugGraphNodeDraft<'a>(&'a GraphNodeDraft);

impl fmt::Debug for DebugGraphNodeDraft<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GraphNodeDraft")
            .field("id", &self.0.id)
            .field("name", &REDACTED_DEBUG_VALUE)
            .field("entity_type", &REDACTED_DEBUG_VALUE)
            .field(
                "description",
                &self.0.description.as_ref().map(|_| REDACTED_DEBUG_VALUE),
            )
            .finish()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectionProvenance {
    pub provider: String,
    pub model: String,
    pub prompt_id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
pub struct GraphNodeDraft {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectionOperation {
    UpsertNote {
        id: String,
        title: String,
        body: String,
        tags: Vec<String>,
    },
    DeleteNote {
        id: String,
    },
    ReorderNote {
        id: String,
        after_id: Option<String>,
    },
    UpsertGraphNode {
        id: String,
        name: String,
        entity_type: String,
        description: Option<String>,
    },
    RemoveGraphNode {
        id: String,
    },
    InvalidateGraphNode {
        id: String,
    },
    UpsertGraphEdge {
        id: String,
        source: String,
        target: String,
        relation_type: String,
        label: Option<String>,
        weight: f32,
    },
    RemoveGraphEdge {
        id: String,
    },
    InvalidateGraphEdge {
        id: String,
    },
    StrengthenGraphEdge {
        id: String,
        weight_delta: f32,
    },
    WeakenGraphEdge {
        id: String,
        weight_delta: f32,
    },
    MergeGraphNodes {
        source_id: String,
        target_id: String,
    },
    SplitGraphNode {
        id: String,
        replacement_nodes: Vec<GraphNodeDraft>,
    },
}

#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct MaterializedNote {
    pub id: String,
    pub title: String,
    pub body: String,
    pub tags: Vec<String>,
    pub updated_by_sequence: u64,
    pub updated_at_ms: u64,
    pub basis: ProjectionBasis,
    pub provenance: ProjectionProvenance,
}

impl fmt::Debug for MaterializedNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaterializedNote")
            .field("id", &self.id)
            .field("title", &REDACTED_DEBUG_VALUE)
            .field("body", &REDACTED_DEBUG_VALUE)
            .field("tags", &self.tags)
            .field("updated_by_sequence", &self.updated_by_sequence)
            .field("updated_at_ms", &self.updated_at_ms)
            .field("basis", &self.basis)
            .field("provenance", &self.provenance)
            .finish()
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct MaterializedNotes {
    pub schema_version: u32,
    pub session_id: String,
    pub last_sequence: u64,
    pub notes: Vec<MaterializedNote>,
}

impl fmt::Debug for MaterializedNotes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let notes: Vec<&MaterializedNote> = self.notes.iter().collect();

        f.debug_struct("MaterializedNotes")
            .field("schema_version", &self.schema_version)
            .field("session_id", &self.session_id)
            .field("last_sequence", &self.last_sequence)
            .field("notes", &notes)
            .finish()
    }
}

impl MaterializedNotes {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            session_id: session_id.into(),
            last_sequence: 0,
            notes: Vec::new(),
        }
    }

    pub fn apply_patch(&mut self, patch: &ProjectionPatch) -> Result<(), ProjectionApplyError> {
        if patch.kind != ProjectionKind::Notes {
            return Err(ProjectionApplyError::WrongKind {
                expected: ProjectionKind::Notes,
                actual: patch.kind.clone(),
            });
        }
        if patch.sequence <= self.last_sequence {
            return Err(ProjectionApplyError::StaleSequence {
                current: self.last_sequence,
                incoming: patch.sequence,
            });
        }

        let mut next = self.clone();
        for operation in &patch.operations {
            match operation {
                ProjectionOperation::UpsertNote {
                    id,
                    title,
                    body,
                    tags,
                } => next.upsert_note(id, title, body, tags, patch),
                ProjectionOperation::DeleteNote { id } => {
                    next.notes.retain(|note| note.id != *id);
                }
                ProjectionOperation::ReorderNote { id, after_id } => {
                    next.reorder_note(id, after_id.as_deref())?;
                }
                ProjectionOperation::UpsertGraphNode { .. }
                | ProjectionOperation::RemoveGraphNode { .. }
                | ProjectionOperation::InvalidateGraphNode { .. }
                | ProjectionOperation::UpsertGraphEdge { .. }
                | ProjectionOperation::RemoveGraphEdge { .. }
                | ProjectionOperation::InvalidateGraphEdge { .. }
                | ProjectionOperation::StrengthenGraphEdge { .. }
                | ProjectionOperation::WeakenGraphEdge { .. }
                | ProjectionOperation::MergeGraphNodes { .. }
                | ProjectionOperation::SplitGraphNode { .. } => {
                    return Err(ProjectionApplyError::UnsupportedOperation {
                        kind: "graph_operation_in_notes_patch",
                    });
                }
            }
        }

        next.last_sequence = patch.sequence;
        *self = next;
        Ok(())
    }

    fn upsert_note(
        &mut self,
        id: &str,
        title: &str,
        body: &str,
        tags: &[String],
        patch: &ProjectionPatch,
    ) {
        let next = MaterializedNote {
            id: id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            tags: tags.to_vec(),
            updated_by_sequence: patch.sequence,
            updated_at_ms: patch.created_at_ms,
            basis: patch.basis.clone(),
            provenance: patch.provenance.clone(),
        };

        if let Some(existing) = self.notes.iter_mut().find(|note| note.id == id) {
            *existing = next;
        } else {
            self.notes.push(next);
        }
    }

    fn reorder_note(
        &mut self,
        id: &str,
        after_id: Option<&str>,
    ) -> Result<(), ProjectionApplyError> {
        let Some(from_index) = self.notes.iter().position(|note| note.id == id) else {
            return Err(ProjectionApplyError::MissingNoteForReorder { id: id.to_string() });
        };
        if after_id.is_some_and(|after_id| after_id == id) {
            return Ok(());
        }

        let note = self.notes.remove(from_index);
        let insert_index = match after_id {
            Some(after_id) => {
                let Some(after_index) = self.notes.iter().position(|note| note.id == after_id)
                else {
                    return Err(ProjectionApplyError::MissingNoteAfter {
                        id: id.to_string(),
                        after_id: after_id.to_string(),
                    });
                };
                after_index + 1
            }
            None => 0,
        };
        self.notes.insert(insert_index, note);
        Ok(())
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct MaterializedGraphNode {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    pub description: Option<String>,
    #[serde(default = "default_projection_confidence")]
    pub confidence: f32,
    #[serde(default)]
    pub valid_from_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until_ms: Option<u64>,
    pub updated_by_sequence: u64,
    pub updated_at_ms: u64,
    pub basis: ProjectionBasis,
    pub provenance: ProjectionProvenance,
}

impl fmt::Debug for MaterializedGraphNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaterializedGraphNode")
            .field("id", &self.id)
            .field("name", &REDACTED_DEBUG_VALUE)
            .field("entity_type", &REDACTED_DEBUG_VALUE)
            .field(
                "description",
                &self.description.as_ref().map(|_| REDACTED_DEBUG_VALUE),
            )
            .field("confidence", &self.confidence)
            .field("valid_from_ms", &self.valid_from_ms)
            .field("valid_until_ms", &self.valid_until_ms)
            .field("updated_by_sequence", &self.updated_by_sequence)
            .field("updated_at_ms", &self.updated_at_ms)
            .field("basis", &self.basis)
            .field("provenance", &self.provenance)
            .finish()
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct MaterializedGraphEdge {
    pub id: String,
    pub source: String,
    pub target: String,
    pub relation_type: String,
    pub label: Option<String>,
    pub weight: f32,
    #[serde(default = "default_projection_confidence")]
    pub confidence: f32,
    #[serde(default)]
    pub valid_from_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_until_ms: Option<u64>,
    pub updated_by_sequence: u64,
    pub updated_at_ms: u64,
    pub basis: ProjectionBasis,
    pub provenance: ProjectionProvenance,
}

impl fmt::Debug for MaterializedGraphEdge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaterializedGraphEdge")
            .field("id", &self.id)
            .field("source", &self.source)
            .field("target", &self.target)
            .field("relation_type", &REDACTED_DEBUG_VALUE)
            .field("label", &self.label.as_ref().map(|_| REDACTED_DEBUG_VALUE))
            .field("weight", &self.weight)
            .field("confidence", &self.confidence)
            .field("valid_from_ms", &self.valid_from_ms)
            .field("valid_until_ms", &self.valid_until_ms)
            .field("updated_by_sequence", &self.updated_by_sequence)
            .field("updated_at_ms", &self.updated_at_ms)
            .field("basis", &self.basis)
            .field("provenance", &self.provenance)
            .finish()
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct MaterializedGraph {
    pub schema_version: u32,
    pub session_id: String,
    pub last_sequence: u64,
    pub nodes: Vec<MaterializedGraphNode>,
    pub edges: Vec<MaterializedGraphEdge>,
}

impl fmt::Debug for MaterializedGraph {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nodes: Vec<&MaterializedGraphNode> = self.nodes.iter().collect();
        let edges: Vec<&MaterializedGraphEdge> = self.edges.iter().collect();

        f.debug_struct("MaterializedGraph")
            .field("schema_version", &self.schema_version)
            .field("session_id", &self.session_id)
            .field("last_sequence", &self.last_sequence)
            .field("nodes", &nodes)
            .field("edges", &edges)
            .finish()
    }
}

impl MaterializedGraph {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            schema_version: Self::SCHEMA_VERSION,
            session_id: session_id.into(),
            last_sequence: 0,
            nodes: Vec::new(),
            edges: Vec::new(),
        }
    }

    pub fn apply_patch(&mut self, patch: &ProjectionPatch) -> Result<(), ProjectionApplyError> {
        if patch.kind != ProjectionKind::Graph {
            return Err(ProjectionApplyError::WrongKind {
                expected: ProjectionKind::Graph,
                actual: patch.kind.clone(),
            });
        }
        if patch.sequence <= self.last_sequence {
            return Err(ProjectionApplyError::StaleSequence {
                current: self.last_sequence,
                incoming: patch.sequence,
            });
        }

        let mut next = self.clone();
        for operation in &patch.operations {
            match operation {
                ProjectionOperation::UpsertGraphNode {
                    id,
                    name,
                    entity_type,
                    description,
                } => next.upsert_node(id, name, entity_type, description.clone(), patch),
                ProjectionOperation::RemoveGraphNode { id } => {
                    next.nodes.retain(|node| node.id != *id);
                    next.edges
                        .retain(|edge| edge.source != *id && edge.target != *id);
                }
                ProjectionOperation::InvalidateGraphNode { id } => {
                    next.invalidate_node(id, patch)?;
                }
                ProjectionOperation::UpsertGraphEdge {
                    id,
                    source,
                    target,
                    relation_type,
                    label,
                    weight,
                } => {
                    if !next.has_active_node(source) {
                        return Err(ProjectionApplyError::MissingGraphNode {
                            edge_id: id.clone(),
                            node_id: source.clone(),
                        });
                    }
                    if !next.has_active_node(target) {
                        return Err(ProjectionApplyError::MissingGraphNode {
                            edge_id: id.clone(),
                            node_id: target.clone(),
                        });
                    }
                    next.upsert_edge(
                        id,
                        source,
                        target,
                        relation_type,
                        label.clone(),
                        *weight,
                        patch,
                    );
                }
                ProjectionOperation::RemoveGraphEdge { id } => {
                    next.edges.retain(|edge| edge.id != *id);
                }
                ProjectionOperation::InvalidateGraphEdge { id } => {
                    next.invalidate_edge(id, patch)?;
                }
                ProjectionOperation::StrengthenGraphEdge { id, weight_delta } => {
                    next.adjust_edge_weight("strengthen_graph_edge", id, *weight_delta, patch)?;
                }
                ProjectionOperation::WeakenGraphEdge { id, weight_delta } => {
                    next.adjust_edge_weight("weaken_graph_edge", id, -*weight_delta, patch)?;
                }
                ProjectionOperation::MergeGraphNodes {
                    source_id,
                    target_id,
                } => {
                    next.merge_nodes(source_id, target_id, patch)?;
                }
                ProjectionOperation::SplitGraphNode {
                    id,
                    replacement_nodes,
                } => {
                    next.split_node(id, replacement_nodes, patch)?;
                }
                ProjectionOperation::UpsertNote { .. }
                | ProjectionOperation::DeleteNote { .. }
                | ProjectionOperation::ReorderNote { .. } => {
                    return Err(ProjectionApplyError::UnsupportedOperation {
                        kind: "note_operation_in_graph_patch",
                    });
                }
            }
        }

        next.last_sequence = patch.sequence;
        *self = next;
        Ok(())
    }

    fn has_active_node(&self, id: &str) -> bool {
        self.nodes
            .iter()
            .any(|node| node.id == id && node.valid_until_ms.is_none())
    }

    fn upsert_node(
        &mut self,
        id: &str,
        name: &str,
        entity_type: &str,
        description: Option<String>,
        patch: &ProjectionPatch,
    ) {
        let next = MaterializedGraphNode {
            id: id.to_string(),
            name: name.to_string(),
            entity_type: entity_type.to_string(),
            description,
            confidence: patch.confidence,
            valid_from_ms: patch.created_at_ms,
            valid_until_ms: None,
            updated_by_sequence: patch.sequence,
            updated_at_ms: patch.created_at_ms,
            basis: patch.basis.clone(),
            provenance: patch.provenance.clone(),
        };

        if let Some(existing) = self.nodes.iter_mut().find(|node| node.id == id) {
            *existing = next;
        } else {
            self.nodes.push(next);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn upsert_edge(
        &mut self,
        id: &str,
        source: &str,
        target: &str,
        relation_type: &str,
        label: Option<String>,
        weight: f32,
        patch: &ProjectionPatch,
    ) {
        let next = MaterializedGraphEdge {
            id: id.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            relation_type: relation_type.to_string(),
            label,
            weight,
            confidence: patch.confidence,
            valid_from_ms: patch.created_at_ms,
            valid_until_ms: None,
            updated_by_sequence: patch.sequence,
            updated_at_ms: patch.created_at_ms,
            basis: patch.basis.clone(),
            provenance: patch.provenance.clone(),
        };

        if let Some(existing) = self.edges.iter_mut().find(|edge| edge.id == id) {
            *existing = next;
        } else {
            self.edges.push(next);
        }
    }

    fn invalidate_node(
        &mut self,
        id: &str,
        patch: &ProjectionPatch,
    ) -> Result<(), ProjectionApplyError> {
        let Some(index) = self.active_node_index(id) else {
            return Err(ProjectionApplyError::MissingGraphNodeForOperation {
                operation: "invalidate_graph_node",
                node_id: id.to_string(),
            });
        };

        self.invalidate_node_at(index, patch);
        for edge_index in 0..self.edges.len() {
            let edge = &self.edges[edge_index];
            if edge.valid_until_ms.is_none() && (edge.source == id || edge.target == id) {
                self.invalidate_edge_at(edge_index, patch);
            }
        }
        Ok(())
    }

    fn invalidate_edge(
        &mut self,
        id: &str,
        patch: &ProjectionPatch,
    ) -> Result<(), ProjectionApplyError> {
        let Some(index) = self.active_edge_index(id) else {
            return Err(ProjectionApplyError::MissingGraphEdgeForOperation {
                operation: "invalidate_graph_edge",
                edge_id: id.to_string(),
            });
        };

        self.invalidate_edge_at(index, patch);
        Ok(())
    }

    fn adjust_edge_weight(
        &mut self,
        operation: &'static str,
        id: &str,
        weight_delta: f32,
        patch: &ProjectionPatch,
    ) -> Result<(), ProjectionApplyError> {
        if !weight_delta.is_finite() || !(-1.0..=1.0).contains(&weight_delta) {
            return Err(ProjectionApplyError::InvalidGraphEdgeWeightDelta {
                operation,
                edge_id: id.to_string(),
                weight_delta,
            });
        }
        let Some(index) = self.active_edge_index(id) else {
            return Err(ProjectionApplyError::MissingGraphEdgeForOperation {
                operation,
                edge_id: id.to_string(),
            });
        };

        let edge = &mut self.edges[index];
        edge.weight = (edge.weight + weight_delta).clamp(0.0, 1.0);
        edge.confidence = patch.confidence;
        edge.updated_by_sequence = patch.sequence;
        edge.updated_at_ms = patch.created_at_ms;
        edge.basis = patch.basis.clone();
        edge.provenance = patch.provenance.clone();
        Ok(())
    }

    fn merge_nodes(
        &mut self,
        source_id: &str,
        target_id: &str,
        patch: &ProjectionPatch,
    ) -> Result<(), ProjectionApplyError> {
        if source_id == target_id {
            return Err(ProjectionApplyError::InvalidGraphMerge {
                source_id: source_id.to_string(),
                target_id: target_id.to_string(),
            });
        }
        let Some(source_index) = self.active_node_index(source_id) else {
            return Err(ProjectionApplyError::MissingGraphNodeForOperation {
                operation: "merge_graph_nodes",
                node_id: source_id.to_string(),
            });
        };
        if !self.has_active_node(target_id) {
            return Err(ProjectionApplyError::MissingGraphNodeForOperation {
                operation: "merge_graph_nodes",
                node_id: target_id.to_string(),
            });
        }

        self.invalidate_node_at(source_index, patch);
        for edge_index in 0..self.edges.len() {
            let edge = &mut self.edges[edge_index];
            if edge.valid_until_ms.is_some() {
                continue;
            }
            if edge.source == source_id {
                edge.source = target_id.to_string();
            }
            if edge.target == source_id {
                edge.target = target_id.to_string();
            }
            if edge.source == edge.target {
                self.invalidate_edge_at(edge_index, patch);
            } else if self.edges[edge_index].source == target_id
                || self.edges[edge_index].target == target_id
            {
                self.edges[edge_index].updated_by_sequence = patch.sequence;
                self.edges[edge_index].updated_at_ms = patch.created_at_ms;
                self.edges[edge_index].basis = patch.basis.clone();
                self.edges[edge_index].provenance = patch.provenance.clone();
            }
        }
        self.invalidate_duplicate_active_edges(patch);
        Ok(())
    }

    fn split_node(
        &mut self,
        id: &str,
        replacement_nodes: &[GraphNodeDraft],
        patch: &ProjectionPatch,
    ) -> Result<(), ProjectionApplyError> {
        if replacement_nodes.len() < 2 {
            return Err(ProjectionApplyError::InvalidGraphSplit {
                node_id: id.to_string(),
                reason: "split_requires_at_least_two_replacement_nodes",
            });
        }
        if replacement_nodes
            .iter()
            .any(|replacement| replacement.id == id)
        {
            return Err(ProjectionApplyError::InvalidGraphSplit {
                node_id: id.to_string(),
                reason: "replacement_node_reuses_split_node_id",
            });
        }
        let Some(index) = self.active_node_index(id) else {
            return Err(ProjectionApplyError::MissingGraphNodeForOperation {
                operation: "split_graph_node",
                node_id: id.to_string(),
            });
        };

        self.invalidate_node_at(index, patch);
        for edge_index in 0..self.edges.len() {
            let edge = &self.edges[edge_index];
            if edge.valid_until_ms.is_none() && (edge.source == id || edge.target == id) {
                self.invalidate_edge_at(edge_index, patch);
            }
        }
        for replacement in replacement_nodes {
            self.upsert_node(
                &replacement.id,
                &replacement.name,
                &replacement.entity_type,
                replacement.description.clone(),
                patch,
            );
        }
        Ok(())
    }

    fn active_node_index(&self, id: &str) -> Option<usize> {
        self.nodes
            .iter()
            .position(|node| node.id == id && node.valid_until_ms.is_none())
    }

    fn active_edge_index(&self, id: &str) -> Option<usize> {
        self.edges
            .iter()
            .position(|edge| edge.id == id && edge.valid_until_ms.is_none())
    }

    fn invalidate_node_at(&mut self, index: usize, patch: &ProjectionPatch) {
        let node = &mut self.nodes[index];
        node.valid_until_ms = Some(patch.created_at_ms);
        node.confidence = patch.confidence;
        node.updated_by_sequence = patch.sequence;
        node.updated_at_ms = patch.created_at_ms;
        node.basis = patch.basis.clone();
        node.provenance = patch.provenance.clone();
    }

    fn invalidate_edge_at(&mut self, index: usize, patch: &ProjectionPatch) {
        let edge = &mut self.edges[index];
        edge.valid_until_ms = Some(patch.created_at_ms);
        edge.confidence = patch.confidence;
        edge.updated_by_sequence = patch.sequence;
        edge.updated_at_ms = patch.created_at_ms;
        edge.basis = patch.basis.clone();
        edge.provenance = patch.provenance.clone();
    }

    fn invalidate_duplicate_active_edges(&mut self, patch: &ProjectionPatch) {
        let mut winners: BTreeMap<(String, String, String), usize> = BTreeMap::new();
        for edge_index in 0..self.edges.len() {
            if self.edges[edge_index].valid_until_ms.is_some() {
                continue;
            }
            let key = (
                self.edges[edge_index].source.clone(),
                self.edges[edge_index].target.clone(),
                self.edges[edge_index].relation_type.clone(),
            );
            if let Some(winner_index) = winners.get(&key).copied() {
                if self.edges[edge_index].weight > self.edges[winner_index].weight {
                    self.edges[winner_index].weight = self.edges[edge_index].weight;
                }
                if self.edges[winner_index].label.is_none() {
                    self.edges[winner_index].label = self.edges[edge_index].label.clone();
                }
                self.edges[winner_index].confidence = self.edges[winner_index]
                    .confidence
                    .max(self.edges[edge_index].confidence);
                self.edges[winner_index].updated_by_sequence = patch.sequence;
                self.edges[winner_index].updated_at_ms = patch.created_at_ms;
                self.edges[winner_index].basis = patch.basis.clone();
                self.edges[winner_index].provenance = patch.provenance.clone();
                self.invalidate_edge_at(edge_index, patch);
            } else {
                winners.insert(key, edge_index);
            }
        }
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct MaterializedProjectionState {
    pub session_id: String,
    pub notes: MaterializedNotes,
    pub graph: MaterializedGraph,
}

impl fmt::Debug for MaterializedProjectionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaterializedProjectionState")
            .field("session_id", &self.session_id)
            .field("notes", &self.notes)
            .field("graph", &self.graph)
            .finish()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct HistoricalProjectionReplay {
    pub state: MaterializedProjectionState,
    pub validation: HistoricalProjectionValidationReport,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq, Default)]
pub struct HistoricalProjectionValidationReport {
    pub checked_patch_count: usize,
    pub invalid_patch_count: usize,
    pub errors: Vec<HistoricalProjectionValidationError>,
}

impl HistoricalProjectionValidationReport {
    pub fn first_error_summary(&self) -> Option<String> {
        self.errors.first().map(|error| format!("{error:?}"))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HistoricalProjectionValidationError {
    StaleBasis {
        sequence: u64,
        kind: ProjectionKind,
        staleness: ProjectionBasisStaleness,
    },
    TranscriptReplay {
        sequence: u64,
        error: TranscriptLedgerError,
    },
}

impl MaterializedProjectionState {
    pub fn new(session_id: impl Into<String>) -> Self {
        let session_id = session_id.into();
        Self {
            notes: MaterializedNotes::new(session_id.clone()),
            graph: MaterializedGraph::new(session_id.clone()),
            session_id,
        }
    }

    /// Apply a projection patch that was already accepted into the durable
    /// projection event log.
    ///
    /// Live runtime calls should use [`Self::apply_validated_patch`] so stale
    /// LLM responses are rejected before they become accepted events. Replay
    /// cannot validate old accepted patches against the final transcript
    /// ledger: a later transcript span would make an earlier valid patch look
    /// stale. Historical basis validation needs the full transcript-event
    /// history and event ordering, so this path trusts the accepted log and
    /// replays materializer operations deterministically.
    pub fn apply_replayed_patch(
        &mut self,
        patch: &ProjectionPatch,
    ) -> Result<MaterializedProjectionApplyOutcome, ProjectionApplyError> {
        match patch.kind {
            ProjectionKind::Notes => {
                self.notes.apply_patch(patch)?;
                Ok(MaterializedProjectionApplyOutcome::Notes {
                    last_sequence: self.notes.last_sequence,
                    note_count: self.notes.notes.len(),
                })
            }
            ProjectionKind::Graph => {
                self.graph.apply_patch(patch)?;
                Ok(MaterializedProjectionApplyOutcome::Graph {
                    last_sequence: self.graph.last_sequence,
                    node_count: self.graph.nodes.len(),
                    edge_count: self.graph.edges.len(),
                })
            }
        }
    }

    pub fn replay_accepted_patches(
        session_id: impl Into<String>,
        patches: impl IntoIterator<Item = ProjectionPatch>,
    ) -> Result<Self, ProjectionApplyError> {
        let mut state = Self::new(session_id);
        for patch in patches {
            state.apply_replayed_patch(&patch)?;
        }
        Ok(state)
    }

    pub fn replay_accepted_patches_with_transcript_history(
        session_id: impl Into<String>,
        transcript_events: impl IntoIterator<Item = TranscriptEvent>,
        patches: impl IntoIterator<Item = ProjectionPatch>,
    ) -> Result<HistoricalProjectionReplay, ProjectionApplyError> {
        let session_id = session_id.into();
        let mut state = Self::new(&session_id);
        let mut ledger = TranscriptLedger::new(&session_id);
        let mut validation = HistoricalProjectionValidationReport::default();
        let mut transcript_events: Vec<TranscriptEvent> = transcript_events.into_iter().collect();
        transcript_events.sort_by(|a, b| {
            a.received_at_ms
                .cmp(&b.received_at_ms)
                .then(millis(a.start_time).cmp(&millis(b.start_time)))
                .then(millis(a.end_time).cmp(&millis(b.end_time)))
                .then(a.span_id.cmp(&b.span_id))
                .then(a.revision_number.cmp(&b.revision_number))
        });
        let mut transcript_cursor = 0;

        'patches: for patch in patches {
            validation.checked_patch_count += 1;
            while transcript_cursor < transcript_events.len()
                && transcript_events[transcript_cursor].received_at_ms <= patch.created_at_ms
            {
                let event = transcript_events[transcript_cursor].clone();
                transcript_cursor += 1;
                if let Err(error) = ledger.apply_event(event) {
                    validation.invalid_patch_count += 1;
                    validation
                        .errors
                        .push(HistoricalProjectionValidationError::TranscriptReplay {
                            sequence: patch.sequence,
                            error,
                        });
                    continue 'patches;
                }
            }

            if let Err(staleness) = ledger.validate_basis(&patch.basis) {
                validation.invalid_patch_count += 1;
                validation
                    .errors
                    .push(HistoricalProjectionValidationError::StaleBasis {
                        sequence: patch.sequence,
                        kind: patch.kind.clone(),
                        staleness,
                    });
                continue;
            }

            state.apply_replayed_patch(&patch)?;
        }

        Ok(HistoricalProjectionReplay { state, validation })
    }

    pub fn apply_validated_patch(
        &mut self,
        ledger: &TranscriptLedger,
        patch: &ProjectionPatch,
    ) -> Result<MaterializedProjectionApplyOutcome, ProjectionApplyError> {
        self.apply_validated_patch_with_speaker_timeline_opt(ledger, None, patch)
    }

    /// Like [`Self::apply_validated_patch`] but also validates the patch's
    /// diarization basis against the session [`SpeakerTimeline`].
    pub fn apply_validated_patch_with_speaker_timeline(
        &mut self,
        ledger: &TranscriptLedger,
        speaker_timeline: &SpeakerTimeline,
        patch: &ProjectionPatch,
    ) -> Result<MaterializedProjectionApplyOutcome, ProjectionApplyError> {
        self.apply_validated_patch_with_speaker_timeline_opt(ledger, Some(speaker_timeline), patch)
    }

    fn apply_validated_patch_with_speaker_timeline_opt(
        &mut self,
        ledger: &TranscriptLedger,
        speaker_timeline: Option<&SpeakerTimeline>,
        patch: &ProjectionPatch,
    ) -> Result<MaterializedProjectionApplyOutcome, ProjectionApplyError> {
        ledger
            .validate_basis_with_speaker_timeline(&patch.basis, speaker_timeline)
            .map_err(|staleness| ProjectionApplyError::StaleBasis { staleness })?;

        match patch.kind {
            ProjectionKind::Notes => {
                self.notes.apply_patch(patch)?;
                Ok(MaterializedProjectionApplyOutcome::Notes {
                    last_sequence: self.notes.last_sequence,
                    note_count: self.notes.notes.len(),
                })
            }
            ProjectionKind::Graph => {
                self.graph.apply_patch(patch)?;
                Ok(MaterializedProjectionApplyOutcome::Graph {
                    last_sequence: self.graph.last_sequence,
                    node_count: self.graph.nodes.len(),
                    edge_count: self.graph.edges.len(),
                })
            }
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MaterializedProjectionApplyOutcome {
    Notes {
        last_sequence: u64,
        note_count: usize,
    },
    Graph {
        last_sequence: u64,
        node_count: usize,
        edge_count: usize,
    },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProjectionApplyError {
    WrongKind {
        expected: ProjectionKind,
        actual: ProjectionKind,
    },
    StaleSequence {
        current: u64,
        incoming: u64,
    },
    UnsupportedOperation {
        kind: &'static str,
    },
    MissingGraphNode {
        edge_id: String,
        node_id: String,
    },
    MissingGraphNodeForOperation {
        operation: &'static str,
        node_id: String,
    },
    MissingGraphEdgeForOperation {
        operation: &'static str,
        edge_id: String,
    },
    InvalidGraphEdgeWeightDelta {
        operation: &'static str,
        edge_id: String,
        weight_delta: f32,
    },
    InvalidGraphMerge {
        source_id: String,
        target_id: String,
    },
    InvalidGraphSplit {
        node_id: String,
        reason: &'static str,
    },
    MissingNoteForReorder {
        id: String,
    },
    MissingNoteAfter {
        id: String,
        after_id: String,
    },
    StaleBasis {
        staleness: ProjectionBasisStaleness,
    },
}

fn millis(value: f64) -> i64 {
    if value.is_finite() {
        (value * 1000.0).round() as i64
    } else {
        0
    }
}

fn default_projection_confidence() -> f32 {
    1.0
}

fn update_hash(hash: &mut u64, value: &str) {
    for byte in value.as_bytes() {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(1_099_511_628_211);
    }
    *hash ^= 0x1f;
    *hash = hash.wrapping_mul(1_099_511_628_211);
}

/// Deterministic FNV-1a hash over canonical transcript revision fields.
pub fn transcript_events_hash(events: &[TranscriptEvent]) -> String {
    let mut ordered: Vec<&TranscriptEvent> = events.iter().collect();
    ordered.sort_by(|a, b| {
        millis(a.start_time)
            .cmp(&millis(b.start_time))
            .then(millis(a.end_time).cmp(&millis(b.end_time)))
            .then(a.span_id.cmp(&b.span_id))
            .then(a.revision_number.cmp(&b.revision_number))
    });

    let mut hash = 14_695_981_039_346_656_037u64;
    for event in ordered {
        update_hash(&mut hash, &event.span_id);
        update_hash(&mut hash, &event.provider);
        update_hash(&mut hash, &event.source_id);
        update_hash(&mut hash, event.speaker_id.as_deref().unwrap_or(""));
        update_hash(&mut hash, event.speaker_label.as_deref().unwrap_or(""));
        update_hash(&mut hash, &event.text);
        update_hash(&mut hash, &millis(event.start_time).to_string());
        update_hash(&mut hash, &millis(event.end_time).to_string());
        update_hash(&mut hash, &event.revision_number.to_string());
        update_hash(&mut hash, if event.is_final { "final" } else { "partial" });
    }
    format!("fnv1a64:{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asr_payload(span_id: &str, revision_number: u64, text: &str) -> AsrSpanRevisionPayload {
        AsrSpanRevisionPayload {
            span_id: span_id.to_string(),
            provider: "openai_realtime".to_string(),
            source_id: "system-default".to_string(),
            provider_item_id: Some("item-1".to_string()),
            transcript_segment_id: Some("segment-1".to_string()),
            speaker_id: Some("speaker-1".to_string()),
            speaker_label: Some("Speaker 1".to_string()),
            channel: Some("mixed".to_string()),
            text: text.to_string(),
            start_time: 1.0,
            end_time: 2.0,
            confidence: 0.92,
            is_final: revision_number > 1,
            stability: if revision_number > 1 {
                AsrSpanStability::Final
            } else {
                AsrSpanStability::Partial
            },
            revision_number,
            supersedes: (revision_number > 1).then(|| format!("{span_id}@rev1")),
            turn_id: Some("turn-1".to_string()),
            end_of_turn: revision_number > 1,
            raw_event_ref: Some("provider.events[0]".to_string()),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000 + revision_number,
        }
    }

    fn provider_payload(
        provider: &str,
        source_id: &str,
        span_id: &str,
        provider_item_id: Option<&str>,
        revision_number: u64,
        text: &str,
        is_final: bool,
    ) -> AsrSpanRevisionPayload {
        AsrSpanRevisionPayload {
            span_id: span_id.to_string(),
            provider: provider.to_string(),
            source_id: source_id.to_string(),
            provider_item_id: provider_item_id.map(str::to_string),
            transcript_segment_id: is_final.then(|| format!("{span_id}-segment")),
            speaker_id: None,
            speaker_label: None,
            channel: None,
            text: text.to_string(),
            start_time: revision_number as f64,
            end_time: revision_number as f64 + 0.5,
            confidence: 0.9,
            is_final,
            stability: if is_final {
                AsrSpanStability::Final
            } else {
                AsrSpanStability::Partial
            },
            revision_number,
            supersedes: (revision_number > 1).then(|| format!("{span_id}@rev1")),
            turn_id: Some(format!("{provider}:{source_id}:turn")),
            end_of_turn: is_final,
            raw_event_ref: Some(format!("{provider}.fixture")),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_001_000 + revision_number,
        }
    }

    #[test]
    fn transcript_event_preserves_asr_revision_identity() {
        let event = TranscriptEvent::from(asr_payload("span-1", 2, "hello world"));

        assert_eq!(event.span_id, "span-1");
        assert_eq!(event.provider_item_id.as_deref(), Some("item-1"));
        assert_eq!(event.transcript_segment_id.as_deref(), Some("segment-1"));
        assert_eq!(event.speaker_id.as_deref(), Some("speaker-1"));
        assert_eq!(event.channel.as_deref(), Some("mixed"));
        assert_eq!(event.stability, TranscriptEventStability::Final);
        assert_eq!(event.revision_number, 2);
        assert_eq!(event.supersedes.as_deref(), Some("span-1@rev1"));
    }

    #[test]
    fn projection_basis_tracks_latest_revision_and_hash_changes() {
        let first = TranscriptEvent::from(asr_payload("span-1", 1, "hello"));
        let second = TranscriptEvent::from(asr_payload("span-1", 2, "hello world"));

        let basis_first = ProjectionBasis::from_transcript_events(std::slice::from_ref(&first));
        let basis_second = ProjectionBasis::from_transcript_events(&[first, second]);

        assert_eq!(
            basis_second.span_revisions,
            vec![ProjectionBasisSpan {
                span_id: "span-1".to_string(),
                revision_number: 2,
            }]
        );
        assert_ne!(basis_first.transcript_hash, basis_second.transcript_hash);
    }

    #[test]
    fn transcript_ledger_replays_latest_revisions_and_validates_current_basis() {
        let first = TranscriptEvent::from(asr_payload("span-1", 1, "hello"));
        let second = TranscriptEvent::from(asr_payload("span-1", 2, "hello world"));
        let third = TranscriptEvent::from(asr_payload("span-2", 1, "next topic"));

        let mut ledger = TranscriptLedger::new("session-1");
        ledger.apply_event(first.clone()).expect("first revision");
        let old_basis = ledger.current_basis();
        assert!(ledger.is_basis_current(&old_basis));

        ledger.apply_event(second.clone()).expect("second revision");
        assert_eq!(ledger.accepted_event_count, 2);
        assert_eq!(ledger.latest_spans.len(), 1);
        assert_eq!(ledger.latest_spans[0].text, "hello world");
        assert_eq!(
            ledger.validate_basis(&old_basis),
            Err(ProjectionBasisStaleness::StaleSpanRevision {
                span_id: "span-1".to_string(),
                current_revision: 2,
                basis_revision: 1,
            })
        );

        ledger.apply_event(third).expect("third span");
        let current_basis = ledger.current_basis();
        assert!(ledger.validate_basis(&current_basis).is_ok());
        assert_eq!(
            current_basis.span_revisions,
            vec![
                ProjectionBasisSpan {
                    span_id: "span-1".to_string(),
                    revision_number: 2,
                },
                ProjectionBasisSpan {
                    span_id: "span-2".to_string(),
                    revision_number: 1,
                },
            ]
        );
    }

    #[test]
    fn transcript_ledger_replays_provider_partial_final_fixtures_without_duplicate_spans() {
        let fixtures = [
            (
                "openai_realtime",
                "system",
                "openai_realtime:system:item-1",
                Some("item-1"),
            ),
            ("deepgram", "system", "deepgram:system:start-1000", None),
            (
                "assemblyai",
                "system",
                "assemblyai:system:turn-1",
                Some("turn-1"),
            ),
            (
                "aws-transcribe",
                "system",
                "aws-transcribe:system:result-1",
                Some("result-1"),
            ),
            (
                "sherpa-onnx",
                "mic-1",
                "sherpa-onnx:mic-1:utterance-1",
                Some("utterance-1"),
            ),
            ("soniox", "system", "soniox:system:turn-1", Some("turn-1")),
            (
                "speechmatics",
                "system",
                "speechmatics:system:segment-1",
                Some("segment-1"),
            ),
            ("gladia", "system", "gladia:system:utt-1", Some("utt-1")),
        ];

        let events =
            fixtures
                .iter()
                .flat_map(|(provider, source_id, span_id, provider_item_id)| {
                    [
                        TranscriptEvent::from(provider_payload(
                            provider,
                            source_id,
                            span_id,
                            *provider_item_id,
                            1,
                            "partial hypothesis",
                            false,
                        )),
                        TranscriptEvent::from(provider_payload(
                            provider,
                            source_id,
                            span_id,
                            *provider_item_id,
                            2,
                            "final transcript",
                            true,
                        )),
                    ]
                });

        let ledger = TranscriptLedger::replay("session-1", events).expect("provider replay");

        assert_eq!(ledger.accepted_event_count, (fixtures.len() * 2) as u64);
        assert_eq!(
            ledger.latest_spans.len(),
            fixtures.len(),
            "partial and final revisions should collapse by stable span id"
        );
        assert!(
            ledger
                .latest_spans
                .iter()
                .all(|event| event.is_final && event.revision_number == 2)
        );
        assert_eq!(
            ledger
                .latest_spans
                .iter()
                .map(|event| event.text.as_str())
                .collect::<Vec<_>>(),
            vec!["final transcript"; fixtures.len()]
        );
        assert_eq!(ledger.current_basis().span_revisions.len(), fixtures.len());
    }

    #[test]
    fn transcript_ledger_rejects_stale_and_conflicting_revisions() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(TranscriptEvent::from(asr_payload(
                "span-1",
                2,
                "current text",
            )))
            .expect("current revision");

        assert_eq!(
            ledger.apply_event(TranscriptEvent::from(asr_payload(
                "span-1",
                1,
                "older text",
            ))),
            Err(TranscriptLedgerError::StaleTranscriptRevision {
                span_id: "span-1".to_string(),
                current_revision: 2,
                incoming_revision: 1,
            })
        );

        assert_eq!(
            ledger.apply_event(TranscriptEvent::from(asr_payload(
                "span-1",
                2,
                "conflicting text",
            ))),
            Err(TranscriptLedgerError::ConflictingTranscriptRevision {
                span_id: "span-1".to_string(),
                revision_number: 2,
            })
        );
    }

    #[test]
    fn transcript_ledger_reports_basis_mismatch_reasons() {
        let event = TranscriptEvent::from(asr_payload("span-1", 1, "basis text"));
        let ledger = TranscriptLedger::replay("session-1", [event]).expect("ledger replay");
        let current_basis = ledger.current_basis();

        let missing_current_span = ProjectionBasis {
            span_revisions: Vec::new(),
            diarization_span_revisions: Vec::new(),
            transcript_hash: current_basis.transcript_hash.clone(),
        };
        assert_eq!(
            ledger.validate_basis(&missing_current_span),
            Err(ProjectionBasisStaleness::MissingCurrentSpan {
                span_id: "span-1".to_string(),
                current_revision: 1,
            })
        );

        let empty_ledger = TranscriptLedger::new("session-1");
        assert_eq!(
            empty_ledger.validate_basis(&current_basis),
            Err(ProjectionBasisStaleness::UnknownBasisSpan {
                span_id: "span-1".to_string(),
                basis_revision: 1,
            })
        );

        let mut hash_mismatch = current_basis.clone();
        hash_mismatch.transcript_hash = "fnv1a64:0000000000000000".to_string();
        assert_eq!(
            ledger.validate_basis(&hash_mismatch),
            Err(ProjectionBasisStaleness::TranscriptHashMismatch {
                current_hash: current_basis.transcript_hash.clone(),
                basis_hash: "fnv1a64:0000000000000000".to_string(),
            })
        );

        let mut diarization_basis = current_basis;
        diarization_basis
            .diarization_span_revisions
            .push(ProjectionBasisSpan {
                span_id: "speaker-span-1".to_string(),
                revision_number: 1,
            });
        assert_eq!(
            ledger.validate_basis(&diarization_basis),
            Err(ProjectionBasisStaleness::DiarizationBasisUnavailable { count: 1 })
        );
    }

    #[test]
    fn projection_patch_serializes_replayable_operations() {
        let event = TranscriptEvent::from(asr_payload("span-1", 2, "decision made"));
        let basis = ProjectionBasis::from_transcript_events(&[event]);
        let patch = ProjectionPatch {
            sequence: 7,
            kind: ProjectionKind::Notes,
            llm_request_id: "llm-req-1".to_string(),
            basis,
            operations: vec![ProjectionOperation::UpsertNote {
                id: "note-1".to_string(),
                title: "Decision".to_string(),
                body: "Ship the event-sourced projection model.".to_string(),
                tags: vec!["decision".to_string()],
            }],
            confidence: 0.86,
            provenance: ProjectionProvenance {
                provider: "openrouter".to_string(),
                model: "anthropic/claude-sonnet-4".to_string(),
                prompt_id: "notes-v1".to_string(),
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_700_000_000_100,
        };

        let json = serde_json::to_value(&patch).expect("serialize patch");
        assert_eq!(json["kind"], "notes");
        assert_eq!(json["operations"][0]["type"], "upsert_note");
        assert_eq!(json["basis"]["span_revisions"][0]["revision_number"], 2);
        assert_eq!(json["provenance"]["prompt_id"], "notes-v1");
    }

    #[test]
    fn transcript_event_debug_redacts_text_but_preserves_non_content_fields() {
        let event = TranscriptEvent::from(asr_payload("span-1", 2, "SENSITIVE TRANSCRIPT TEXT"));
        let debug = format!("{event:?}");

        assert!(!debug.contains("SENSITIVE TRANSCRIPT TEXT"));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("span_id"));
        assert!(debug.contains("received_at_ms"));
    }

    #[test]
    fn projection_patch_debug_redacts_note_and_graph_sensitive_payloads() {
        let patch = ProjectionPatch {
            sequence: 42,
            kind: ProjectionKind::Graph,
            llm_request_id: "llm-req-sensitive".to_string(),
            basis: ProjectionBasis {
                span_revisions: vec![ProjectionBasisSpan {
                    span_id: "span-1".to_string(),
                    revision_number: 1,
                }],
                diarization_span_revisions: Vec::new(),
                transcript_hash: "fnv1a64:000000".to_string(),
            },
            operations: vec![
                ProjectionOperation::UpsertNote {
                    id: "note-1".to_string(),
                    title: "Decision".to_string(),
                    body: "SENSITIVE NOTE BODY".to_string(),
                    tags: vec!["decision".to_string()],
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "node-1".to_string(),
                    name: "SECRET NAME".to_string(),
                    entity_type: "SECRET TYPE".to_string(),
                    description: Some("SECRET DESCRIPTION".to_string()),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge-1".to_string(),
                    source: "node-1".to_string(),
                    target: "node-2".to_string(),
                    relation_type: "SECRET RELATION".to_string(),
                    label: Some("SECRET LABEL".to_string()),
                    weight: 0.9,
                },
            ],
            confidence: 0.8,
            provenance: ProjectionProvenance {
                provider: "openrouter".to_string(),
                model: "gpt-4.1".to_string(),
                prompt_id: "graph-v1".to_string(),
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_700_000_000_500,
        };

        let json = serde_json::to_value(&patch).expect("serialize patch");
        assert_eq!(
            json["operations"][0]["body"],
            serde_json::Value::String("SENSITIVE NOTE BODY".to_string())
        );
        assert_eq!(
            json["operations"][2]["relation_type"],
            serde_json::Value::String("SECRET RELATION".to_string())
        );

        let debug = format!("{patch:?}");
        assert!(!debug.contains("Decision"));
        assert!(!debug.contains("SENSITIVE NOTE BODY"));
        assert!(!debug.contains("SECRET NAME"));
        assert!(!debug.contains("SECRET TYPE"));
        assert!(!debug.contains("SECRET DESCRIPTION"));
        assert!(!debug.contains("SECRET RELATION"));
        assert!(!debug.contains("SECRET LABEL"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn materialized_notes_debug_redacts_note_body_but_serialization_keeps_it() {
        let mut notes = MaterializedNotes::new("session-1");
        notes
            .apply_patch(&notes_patch(1, "note-1", "Decision", "SENSITIVE NOTE BODY"))
            .expect("insert note patch");

        let json = serde_json::to_value(&notes).expect("serialize notes");
        assert_eq!(
            json["notes"][0]["body"],
            serde_json::Value::String("SENSITIVE NOTE BODY".to_string())
        );

        let debug = format!("{notes:?}");
        assert!(!debug.contains("Decision"));
        assert!(!debug.contains("SENSITIVE NOTE BODY"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn materialized_graph_debug_redacts_node_and_edge_attributes_but_serialization_keeps_them() {
        let mut graph = MaterializedGraph::new("session-1");
        graph
            .apply_patch(&graph_patch(
                1,
                vec![
                    ProjectionOperation::UpsertGraphNode {
                        id: "node-a".to_string(),
                        name: "Node A".to_string(),
                        entity_type: "PERSON".to_string(),
                        description: Some("SENSITIVE DESCRIPTION".to_string()),
                    },
                    ProjectionOperation::UpsertGraphNode {
                        id: "node-b".to_string(),
                        name: "Node B".to_string(),
                        entity_type: "TOPIC".to_string(),
                        description: None,
                    },
                    ProjectionOperation::UpsertGraphEdge {
                        id: "edge-ab".to_string(),
                        source: "node-a".to_string(),
                        target: "node-b".to_string(),
                        relation_type: "SENSITIVE RELATION".to_string(),
                        label: Some("SENSITIVE LABEL".to_string()),
                        weight: 0.4,
                    },
                ],
            ))
            .expect("insert graph patch");

        let json = serde_json::to_value(&graph).expect("serialize graph");
        assert_eq!(
            json["nodes"][0]["name"],
            serde_json::Value::String("Node A".to_string())
        );
        assert_eq!(
            json["edges"][0]["relation_type"],
            serde_json::Value::String("SENSITIVE RELATION".to_string())
        );

        let debug = format!("{graph:?}");
        assert!(!debug.contains("Node A"));
        assert!(!debug.contains("SENSITIVE DESCRIPTION"));
        assert!(!debug.contains("SENSITIVE RELATION"));
        assert!(!debug.contains("SENSITIVE LABEL"));
        assert!(!debug.contains("PERSON"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn materialized_projection_state_debug_redacts_nested_notes_and_graph_sensitive_content() {
        let mut state = MaterializedProjectionState::new("session-1");
        state
            .apply_replayed_patch(&notes_patch(
                1,
                "note-1",
                "Decision",
                "SENSITIVE NOTE IN STATE",
            ))
            .expect("apply state note patch");
        state
            .apply_replayed_patch(&graph_patch(
                2,
                vec![
                    ProjectionOperation::UpsertGraphNode {
                        id: "node-a".to_string(),
                        name: "Node A".to_string(),
                        entity_type: "PERSON".to_string(),
                        description: Some("SENSITIVE DESC".to_string()),
                    },
                    ProjectionOperation::UpsertGraphNode {
                        id: "node-b".to_string(),
                        name: "Node B".to_string(),
                        entity_type: "TOPIC".to_string(),
                        description: None,
                    },
                    ProjectionOperation::UpsertGraphEdge {
                        id: "edge-ab".to_string(),
                        source: "node-a".to_string(),
                        target: "node-b".to_string(),
                        relation_type: "SENSITIVE RELATION".to_string(),
                        label: Some("SENSITIVE LABEL".to_string()),
                        weight: 0.4,
                    },
                ],
            ))
            .expect("apply state graph patch");

        let debug = format!("{state:?}");
        assert!(!debug.contains("SENSITIVE NOTE IN STATE"));
        assert!(!debug.contains("SENSITIVE DESC"));
        assert!(!debug.contains("SENSITIVE RELATION"));
        assert!(!debug.contains("SENSITIVE LABEL"));
        assert!(!debug.contains("Node A"));
    }

    fn notes_patch(sequence: u64, id: &str, title: &str, body: &str) -> ProjectionPatch {
        let event = TranscriptEvent::from(asr_payload("span-1", sequence, body));
        notes_patch_for_basis(sequence, std::slice::from_ref(&event), id, title, body)
    }

    fn notes_patch_for_basis(
        sequence: u64,
        basis_events: &[TranscriptEvent],
        id: &str,
        title: &str,
        body: &str,
    ) -> ProjectionPatch {
        ProjectionPatch {
            sequence,
            kind: ProjectionKind::Notes,
            llm_request_id: format!("llm-req-{sequence}"),
            basis: ProjectionBasis::from_transcript_events(basis_events),
            operations: vec![ProjectionOperation::UpsertNote {
                id: id.to_string(),
                title: title.to_string(),
                body: body.to_string(),
                tags: vec!["decision".to_string()],
            }],
            confidence: 0.86,
            provenance: ProjectionProvenance {
                provider: "openrouter".to_string(),
                model: "anthropic/claude-sonnet-4".to_string(),
                prompt_id: "notes-v1".to_string(),
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_700_000_000_100 + sequence,
        }
    }

    fn graph_patch(sequence: u64, operations: Vec<ProjectionOperation>) -> ProjectionPatch {
        let event = TranscriptEvent::from(asr_payload("span-graph", sequence, "graph basis"));
        ProjectionPatch {
            sequence,
            kind: ProjectionKind::Graph,
            llm_request_id: format!("llm-graph-req-{sequence}"),
            basis: ProjectionBasis::from_transcript_events(&[event]),
            operations,
            confidence: 0.81,
            provenance: ProjectionProvenance {
                provider: "openrouter".to_string(),
                model: "anthropic/claude-sonnet-4".to_string(),
                prompt_id: "graph-v1".to_string(),
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_700_000_000_200 + sequence,
        }
    }

    #[test]
    fn materialized_notes_apply_insert_update_reorder_and_delete_patches() {
        let mut notes = MaterializedNotes::new("session-1");
        let first = notes_patch(1, "note-1", "Decision", "Ship projection events.");
        notes.apply_patch(&first).expect("insert patch");

        assert_eq!(notes.last_sequence, 1);
        assert_eq!(notes.notes.len(), 1);
        assert_eq!(notes.notes[0].id, "note-1");
        assert_eq!(notes.notes[0].body, "Ship projection events.");
        assert_eq!(notes.notes[0].updated_by_sequence, 1);
        assert_eq!(
            notes.notes[0].basis.transcript_hash,
            first.basis.transcript_hash
        );

        let update = notes_patch(2, "note-1", "Decision", "Ship materialized notes.");
        notes.apply_patch(&update).expect("update patch");

        assert_eq!(notes.last_sequence, 2);
        assert_eq!(notes.notes.len(), 1);
        assert_eq!(notes.notes[0].body, "Ship materialized notes.");
        assert_eq!(notes.notes[0].updated_by_sequence, 2);

        let second = notes_patch(3, "note-2", "Follow-up", "Keep stable note ids.");
        notes.apply_patch(&second).expect("second note patch");
        assert_eq!(
            notes
                .notes
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            vec!["note-1", "note-2"]
        );

        let reorder = ProjectionPatch {
            sequence: 4,
            kind: ProjectionKind::Notes,
            llm_request_id: "llm-req-4".to_string(),
            basis: second.basis.clone(),
            operations: vec![ProjectionOperation::ReorderNote {
                id: "note-2".to_string(),
                after_id: None,
            }],
            confidence: 0.9,
            provenance: second.provenance.clone(),
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_700_000_000_104,
        };
        notes.apply_patch(&reorder).expect("reorder patch");
        assert_eq!(notes.last_sequence, 4);
        assert_eq!(
            notes
                .notes
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            vec!["note-2", "note-1"]
        );

        let delete = ProjectionPatch {
            sequence: 5,
            kind: ProjectionKind::Notes,
            llm_request_id: "llm-req-5".to_string(),
            basis: reorder.basis,
            operations: vec![ProjectionOperation::DeleteNote {
                id: "note-1".to_string(),
            }],
            confidence: 0.9,
            provenance: reorder.provenance,
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_700_000_000_105,
        };
        notes.apply_patch(&delete).expect("delete patch");

        assert_eq!(notes.last_sequence, 5);
        assert_eq!(notes.notes.len(), 1);
        assert_eq!(notes.notes[0].id, "note-2");
    }

    #[test]
    fn materialized_notes_reject_stale_or_wrong_kind_patches() {
        let mut notes = MaterializedNotes::new("session-1");
        notes
            .apply_patch(&notes_patch(2, "note-1", "Decision", "Ship notes."))
            .expect("first patch");

        let stale = notes_patch(2, "note-2", "Duplicate", "Should be rejected.");
        assert_eq!(
            notes.apply_patch(&stale),
            Err(ProjectionApplyError::StaleSequence {
                current: 2,
                incoming: 2,
            })
        );

        let mut graph_patch = notes_patch(3, "note-3", "Graph", "Wrong kind.");
        graph_patch.kind = ProjectionKind::Graph;
        assert_eq!(
            notes.apply_patch(&graph_patch),
            Err(ProjectionApplyError::WrongKind {
                expected: ProjectionKind::Notes,
                actual: ProjectionKind::Graph,
            })
        );

        let missing_reorder = ProjectionPatch {
            sequence: 3,
            kind: ProjectionKind::Notes,
            llm_request_id: "llm-req-reorder".to_string(),
            basis: notes_patch(3, "note-1", "Decision", "basis").basis,
            operations: vec![ProjectionOperation::ReorderNote {
                id: "note-missing".to_string(),
                after_id: None,
            }],
            confidence: 0.9,
            provenance: ProjectionProvenance {
                provider: "test".to_string(),
                model: "test".to_string(),
                prompt_id: "notes-v1".to_string(),
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_700_000_000_103,
        };
        assert_eq!(
            notes.apply_patch(&missing_reorder),
            Err(ProjectionApplyError::MissingNoteForReorder {
                id: "note-missing".to_string(),
            })
        );
    }

    #[test]
    fn materialized_graph_apply_node_edge_update_and_removals() {
        let mut graph = MaterializedGraph::new("session-1");
        let first = graph_patch(
            1,
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "node-a".to_string(),
                    name: "AudioGraph".to_string(),
                    entity_type: "Product".to_string(),
                    description: Some("Speech knowledge graph app.".to_string()),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "node-b".to_string(),
                    name: "Soniox".to_string(),
                    entity_type: "Provider".to_string(),
                    description: None,
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge-1".to_string(),
                    source: "node-a".to_string(),
                    target: "node-b".to_string(),
                    relation_type: "evaluates".to_string(),
                    label: Some("evaluates as streaming STT".to_string()),
                    weight: 0.7,
                },
            ],
        );
        graph.apply_patch(&first).expect("graph insert patch");

        assert_eq!(graph.last_sequence, 1);
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.nodes[0].confidence, first.confidence);
        assert_eq!(graph.nodes[0].valid_from_ms, first.created_at_ms);
        assert_eq!(graph.nodes[0].valid_until_ms, None);
        assert_eq!(graph.edges[0].source, "node-a");
        assert_eq!(graph.edges[0].target, "node-b");
        assert_eq!(graph.edges[0].confidence, first.confidence);
        assert_eq!(graph.edges[0].valid_from_ms, first.created_at_ms);
        assert_eq!(graph.edges[0].valid_until_ms, None);
        assert_eq!(graph.edges[0].updated_by_sequence, 1);
        assert_eq!(
            graph.edges[0].basis.transcript_hash,
            first.basis.transcript_hash
        );

        let update = graph_patch(
            2,
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "node-b".to_string(),
                    name: "Soniox".to_string(),
                    entity_type: "Provider".to_string(),
                    description: Some("Realtime STT candidate.".to_string()),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge-1".to_string(),
                    source: "node-a".to_string(),
                    target: "node-b".to_string(),
                    relation_type: "shortlists".to_string(),
                    label: Some("shortlisted provider".to_string()),
                    weight: 0.9,
                },
            ],
        );
        graph.apply_patch(&update).expect("graph update patch");

        assert_eq!(graph.last_sequence, 2);
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(
            graph
                .nodes
                .iter()
                .find(|node| node.id == "node-b")
                .and_then(|node| node.description.as_deref()),
            Some("Realtime STT candidate.")
        );
        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.edges[0].relation_type, "shortlists");
        assert_eq!(graph.edges[0].weight, 0.9);
        assert_eq!(graph.edges[0].confidence, update.confidence);
        assert_eq!(graph.edges[0].valid_from_ms, update.created_at_ms);

        let remove_edge = graph_patch(
            3,
            vec![ProjectionOperation::RemoveGraphEdge {
                id: "edge-1".to_string(),
            }],
        );
        graph.apply_patch(&remove_edge).expect("remove edge patch");
        assert!(graph.edges.is_empty());

        let restore = graph_patch(
            4,
            vec![ProjectionOperation::UpsertGraphEdge {
                id: "edge-2".to_string(),
                source: "node-a".to_string(),
                target: "node-b".to_string(),
                relation_type: "tracks".to_string(),
                label: None,
                weight: 0.6,
            }],
        );
        graph.apply_patch(&restore).expect("restore edge patch");
        assert_eq!(graph.edges.len(), 1);

        let remove_node = graph_patch(
            5,
            vec![ProjectionOperation::RemoveGraphNode {
                id: "node-b".to_string(),
            }],
        );
        graph.apply_patch(&remove_node).expect("remove node patch");
        assert_eq!(graph.nodes.len(), 1);
        assert!(graph.nodes.iter().all(|node| node.id != "node-b"));
        assert!(
            graph.edges.is_empty(),
            "removing a node should remove incident edges"
        );
    }

    #[test]
    fn materialized_graph_applies_temporal_retcon_operations() {
        let mut graph = MaterializedGraph::new("session-1");
        let first = graph_patch(
            1,
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "person:alice".to_string(),
                    name: "Alice".to_string(),
                    entity_type: "person".to_string(),
                    description: None,
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "person:alicia".to_string(),
                    name: "Alicia".to_string(),
                    entity_type: "person".to_string(),
                    description: Some("Duplicate mention of Alice.".to_string()),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "project:audio-graph".to_string(),
                    name: "AudioGraph".to_string(),
                    entity_type: "project".to_string(),
                    description: None,
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "topic:provider-work".to_string(),
                    name: "Provider work".to_string(),
                    entity_type: "topic".to_string(),
                    description: None,
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge:alicia:owns".to_string(),
                    source: "person:alicia".to_string(),
                    target: "project:audio-graph".to_string(),
                    relation_type: "owns".to_string(),
                    label: Some("owns".to_string()),
                    weight: 0.4,
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge:alice:owns".to_string(),
                    source: "person:alice".to_string(),
                    target: "project:audio-graph".to_string(),
                    relation_type: "owns".to_string(),
                    label: Some("owns".to_string()),
                    weight: 0.8,
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge:project:topic".to_string(),
                    source: "project:audio-graph".to_string(),
                    target: "topic:provider-work".to_string(),
                    relation_type: "tracks".to_string(),
                    label: None,
                    weight: 0.5,
                },
            ],
        );
        graph.apply_patch(&first).expect("seed graph");

        let weights = graph_patch(
            2,
            vec![
                ProjectionOperation::StrengthenGraphEdge {
                    id: "edge:alicia:owns".to_string(),
                    weight_delta: 0.2,
                },
                ProjectionOperation::WeakenGraphEdge {
                    id: "edge:project:topic".to_string(),
                    weight_delta: 0.3,
                },
            ],
        );
        graph.apply_patch(&weights).expect("adjust edge weights");
        assert_eq!(
            graph
                .edges
                .iter()
                .find(|edge| edge.id == "edge:alicia:owns")
                .map(|edge| edge.weight),
            Some(0.6)
        );
        let topic_weight = graph
            .edges
            .iter()
            .find(|edge| edge.id == "edge:project:topic")
            .map(|edge| edge.weight)
            .expect("topic edge");
        assert!((topic_weight - 0.2).abs() < f32::EPSILON);

        let merge = graph_patch(
            3,
            vec![ProjectionOperation::MergeGraphNodes {
                source_id: "person:alicia".to_string(),
                target_id: "person:alice".to_string(),
            }],
        );
        graph.apply_patch(&merge).expect("merge duplicate nodes");
        assert_eq!(
            graph
                .nodes
                .iter()
                .find(|node| node.id == "person:alicia")
                .and_then(|node| node.valid_until_ms),
            Some(merge.created_at_ms)
        );
        let active_own_edges: Vec<&MaterializedGraphEdge> = graph
            .edges
            .iter()
            .filter(|edge| {
                edge.valid_until_ms.is_none()
                    && edge.source == "person:alice"
                    && edge.target == "project:audio-graph"
                    && edge.relation_type == "owns"
            })
            .collect();
        assert_eq!(active_own_edges.len(), 1);
        assert_eq!(active_own_edges[0].weight, 0.8);

        let split = graph_patch(
            4,
            vec![ProjectionOperation::SplitGraphNode {
                id: "topic:provider-work".to_string(),
                replacement_nodes: vec![
                    GraphNodeDraft {
                        id: "topic:provider-research".to_string(),
                        name: "Provider research".to_string(),
                        entity_type: "topic".to_string(),
                        description: None,
                    },
                    GraphNodeDraft {
                        id: "topic:provider-implementation".to_string(),
                        name: "Provider implementation".to_string(),
                        entity_type: "topic".to_string(),
                        description: None,
                    },
                ],
            }],
        );
        graph.apply_patch(&split).expect("split topic node");
        assert_eq!(
            graph
                .nodes
                .iter()
                .find(|node| node.id == "topic:provider-work")
                .and_then(|node| node.valid_until_ms),
            Some(split.created_at_ms)
        );
        assert!(
            graph.nodes.iter().any(|node| {
                node.id == "topic:provider-research" && node.valid_until_ms.is_none()
            })
        );
        assert!(graph.nodes.iter().any(|node| {
            node.id == "topic:provider-implementation" && node.valid_until_ms.is_none()
        }));
        assert_eq!(
            graph
                .edges
                .iter()
                .find(|edge| edge.id == "edge:project:topic")
                .and_then(|edge| edge.valid_until_ms),
            Some(split.created_at_ms)
        );

        let active_own_edge_id = graph
            .edges
            .iter()
            .find(|edge| {
                edge.valid_until_ms.is_none()
                    && edge.source == "person:alice"
                    && edge.target == "project:audio-graph"
                    && edge.relation_type == "owns"
            })
            .map(|edge| edge.id.clone())
            .expect("active merged edge");
        let invalidate_edge = graph_patch(
            5,
            vec![ProjectionOperation::InvalidateGraphEdge {
                id: active_own_edge_id.clone(),
            }],
        );
        graph
            .apply_patch(&invalidate_edge)
            .expect("invalidate merged edge");
        assert_eq!(
            graph
                .edges
                .iter()
                .find(|edge| edge.id == active_own_edge_id)
                .and_then(|edge| edge.valid_until_ms),
            Some(invalidate_edge.created_at_ms)
        );

        let invalidate_node = graph_patch(
            6,
            vec![ProjectionOperation::InvalidateGraphNode {
                id: "project:audio-graph".to_string(),
            }],
        );
        graph
            .apply_patch(&invalidate_node)
            .expect("invalidate project node");
        assert_eq!(
            graph
                .nodes
                .iter()
                .find(|node| node.id == "project:audio-graph")
                .and_then(|node| node.valid_until_ms),
            Some(invalidate_node.created_at_ms)
        );
    }

    #[test]
    fn materialized_graph_metadata_deserializes_from_older_artifacts() {
        let graph_patch = graph_patch(
            1,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "node-a".to_string(),
                name: "AudioGraph".to_string(),
                entity_type: "Product".to_string(),
                description: None,
            }],
        );
        let mut graph = MaterializedGraph::new("session-1");
        graph.apply_patch(&graph_patch).expect("graph patch");
        let mut value = serde_json::to_value(&graph.nodes[0]).expect("node value");
        let object = value.as_object_mut().expect("node object");
        object.remove("confidence");
        object.remove("valid_from_ms");
        object.remove("valid_until_ms");

        let node: MaterializedGraphNode =
            serde_json::from_value(value).expect("old materialized node");

        assert_eq!(node.confidence, 1.0);
        assert_eq!(node.valid_from_ms, 0);
        assert_eq!(node.valid_until_ms, None);
    }

    #[test]
    fn materialized_graph_rejects_stale_wrong_kind_note_ops_and_dangling_edges() {
        let mut graph = MaterializedGraph::new("session-1");
        graph
            .apply_patch(&graph_patch(
                2,
                vec![ProjectionOperation::UpsertGraphNode {
                    id: "node-a".to_string(),
                    name: "AudioGraph".to_string(),
                    entity_type: "Product".to_string(),
                    description: None,
                }],
            ))
            .expect("first patch");

        let stale = graph_patch(
            2,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "node-b".to_string(),
                name: "Duplicate".to_string(),
                entity_type: "Topic".to_string(),
                description: None,
            }],
        );
        assert_eq!(
            graph.apply_patch(&stale),
            Err(ProjectionApplyError::StaleSequence {
                current: 2,
                incoming: 2,
            })
        );

        let wrong_kind = notes_patch(3, "note-1", "Decision", "Wrong kind.");
        assert_eq!(
            graph.apply_patch(&wrong_kind),
            Err(ProjectionApplyError::WrongKind {
                expected: ProjectionKind::Graph,
                actual: ProjectionKind::Notes,
            })
        );

        let note_op = graph_patch(
            3,
            vec![ProjectionOperation::UpsertNote {
                id: "note-1".to_string(),
                title: "Decision".to_string(),
                body: "Wrong operation.".to_string(),
                tags: Vec::new(),
            }],
        );
        assert_eq!(
            graph.apply_patch(&note_op),
            Err(ProjectionApplyError::UnsupportedOperation {
                kind: "note_operation_in_graph_patch",
            })
        );

        let dangling = graph_patch(
            3,
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "node-c".to_string(),
                    name: "Half Applied".to_string(),
                    entity_type: "Topic".to_string(),
                    description: None,
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge-missing".to_string(),
                    source: "node-c".to_string(),
                    target: "node-missing".to_string(),
                    relation_type: "mentions".to_string(),
                    label: None,
                    weight: 0.5,
                },
            ],
        );
        assert_eq!(
            graph.apply_patch(&dangling),
            Err(ProjectionApplyError::MissingGraphNode {
                edge_id: "edge-missing".to_string(),
                node_id: "node-missing".to_string(),
            })
        );
        assert!(
            graph.nodes.iter().all(|node| node.id != "node-c"),
            "failed graph patches should not partially mutate materialized state"
        );

        let missing_retcon = graph_patch(
            3,
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "node-retcon-prefix".to_string(),
                    name: "Should not persist".to_string(),
                    entity_type: "Topic".to_string(),
                    description: None,
                },
                ProjectionOperation::MergeGraphNodes {
                    source_id: "node-missing".to_string(),
                    target_id: "node-a".to_string(),
                },
            ],
        );
        assert_eq!(
            graph.apply_patch(&missing_retcon),
            Err(ProjectionApplyError::MissingGraphNodeForOperation {
                operation: "merge_graph_nodes",
                node_id: "node-missing".to_string(),
            })
        );
        assert!(
            graph
                .nodes
                .iter()
                .all(|node| node.id != "node-retcon-prefix"),
            "failed retcon patches should not partially mutate materialized state"
        );
    }

    #[test]
    fn materialized_projection_state_applies_notes_and_graph_after_basis_check() {
        let notes_event = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        let notes_ledger =
            TranscriptLedger::replay("session-1", [notes_event]).expect("notes ledger replay");
        let mut state = MaterializedProjectionState::new("session-1");

        assert_eq!(
            state.apply_validated_patch(
                &notes_ledger,
                &notes_patch(1, "note-1", "Decision", "Ship notes."),
            ),
            Ok(MaterializedProjectionApplyOutcome::Notes {
                last_sequence: 1,
                note_count: 1,
            })
        );
        assert_eq!(state.notes.notes[0].id, "note-1");

        let graph_event = TranscriptEvent::from(asr_payload("span-graph", 2, "graph basis"));
        let graph_ledger =
            TranscriptLedger::replay("session-1", [graph_event]).expect("graph ledger replay");
        let graph_patch = graph_patch(
            2,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "node-a".to_string(),
                name: "AudioGraph".to_string(),
                entity_type: "Product".to_string(),
                description: None,
            }],
        );
        assert_eq!(
            state.apply_validated_patch(&graph_ledger, &graph_patch),
            Ok(MaterializedProjectionApplyOutcome::Graph {
                last_sequence: 2,
                node_count: 1,
                edge_count: 0,
            })
        );
        assert_eq!(state.graph.nodes[0].id, "node-a");
    }

    #[test]
    fn materialized_projection_state_replays_accepted_patches_without_final_ledger_staleness() {
        let first = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        let second = TranscriptEvent::from(asr_payload("span-2", 1, "Later context."));
        let mut final_ledger = TranscriptLedger::new("session-1");
        final_ledger.apply_event(first).expect("first event");
        let accepted_patch = notes_patch(1, "note-1", "Decision", "Ship notes.");
        final_ledger.apply_event(second).expect("second event");

        let mut live_state = MaterializedProjectionState::new("session-1");
        assert!(
            matches!(
                live_state.apply_validated_patch(&final_ledger, &accepted_patch),
                Err(ProjectionApplyError::StaleBasis { .. })
            ),
            "the final ledger should be too strict for an older accepted patch"
        );

        let replayed = MaterializedProjectionState::replay_accepted_patches(
            "session-1",
            [accepted_patch.clone()],
        )
        .expect("accepted projection event replay");
        assert_eq!(replayed.notes.last_sequence, accepted_patch.sequence);
        assert_eq!(replayed.notes.notes[0].id, "note-1");
        assert_eq!(replayed.notes.notes[0].body, "Ship notes.");
    }

    #[test]
    fn materialized_projection_history_validation_accepts_old_patch_before_later_transcript_growth()
    {
        let first = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        let mut second = TranscriptEvent::from(asr_payload("span-2", 1, "Later context."));
        second.received_at_ms = 1_700_000_010_000;
        let accepted_patch = notes_patch(1, "note-1", "Decision", "Ship notes.");

        let replayed =
            MaterializedProjectionState::replay_accepted_patches_with_transcript_history(
                "session-1",
                [first, second],
                [accepted_patch.clone()],
            )
            .expect("historically validated replay");

        assert_eq!(replayed.validation.checked_patch_count, 1);
        assert_eq!(replayed.validation.invalid_patch_count, 0);
        assert_eq!(replayed.state.notes.last_sequence, accepted_patch.sequence);
        assert_eq!(replayed.state.notes.notes[0].id, "note-1");
    }

    #[test]
    fn materialized_projection_history_validation_skips_impossible_patch_basis() {
        let current = TranscriptEvent::from(asr_payload("span-1", 2, "Current transcript."));
        let impossible_patch = notes_patch(1, "note-1", "Decision", "Stale basis.");

        let replayed =
            MaterializedProjectionState::replay_accepted_patches_with_transcript_history(
                "session-1",
                [current],
                [impossible_patch],
            )
            .expect("historically validated replay");

        assert_eq!(replayed.validation.checked_patch_count, 1);
        assert_eq!(replayed.validation.invalid_patch_count, 1);
        assert!(matches!(
            replayed.validation.errors.first(),
            Some(HistoricalProjectionValidationError::StaleBasis {
                sequence: 1,
                kind: ProjectionKind::Notes,
                staleness: ProjectionBasisStaleness::StaleSpanRevision {
                    span_id,
                    current_revision: 2,
                    basis_revision: 1,
                },
            }) if span_id == "span-1"
        ));
        assert!(replayed.state.notes.notes.is_empty());
        assert_eq!(replayed.state.notes.last_sequence, 0);
    }

    #[test]
    fn materialized_projection_history_rejects_stale_note_and_replays_retcon_repair() {
        let mut first_event =
            TranscriptEvent::from(asr_payload("span-1", 1, "Alice said ship AlphaGraph."));
        let mut corrected_event = TranscriptEvent::from(asr_payload(
            "span-1",
            2,
            "Alice corrected it: ship AudioGraph.",
        ));
        let mut initial_note = notes_patch_for_basis(
            1,
            std::slice::from_ref(&first_event),
            "note-decision",
            "Decision",
            "Ship AlphaGraph.",
        );
        let mut stale_after_retcon = notes_patch_for_basis(
            2,
            std::slice::from_ref(&first_event),
            "note-duplicate",
            "Decision",
            "Duplicate note from stale rev1 basis.",
        );
        let mut repair_note = notes_patch_for_basis(
            3,
            std::slice::from_ref(&corrected_event),
            "note-decision",
            "Decision",
            "Ship AudioGraph.",
        );

        first_event.received_at_ms = 1_700_000_010_000;
        initial_note.created_at_ms = 1_700_000_010_100;
        corrected_event.received_at_ms = 1_700_000_020_000;
        stale_after_retcon.created_at_ms = 1_700_000_020_100;
        repair_note.created_at_ms = 1_700_000_020_200;

        let replayed =
            MaterializedProjectionState::replay_accepted_patches_with_transcript_history(
                "session-1",
                [first_event, corrected_event],
                [initial_note, stale_after_retcon, repair_note],
            )
            .expect("historical note retcon replay");

        assert_eq!(replayed.validation.checked_patch_count, 3);
        assert_eq!(replayed.validation.invalid_patch_count, 1);
        assert!(matches!(
            replayed.validation.errors.first(),
            Some(HistoricalProjectionValidationError::StaleBasis {
                sequence: 2,
                kind: ProjectionKind::Notes,
                staleness: ProjectionBasisStaleness::StaleSpanRevision {
                    span_id,
                    current_revision: 2,
                    basis_revision: 1,
                },
            }) if span_id == "span-1"
        ));
        assert_eq!(replayed.state.notes.notes.len(), 1);
        assert_eq!(replayed.state.notes.notes[0].id, "note-decision");
        assert_eq!(replayed.state.notes.notes[0].body, "Ship AudioGraph.");
        assert_eq!(replayed.state.notes.notes[0].updated_by_sequence, 3);
        assert_eq!(
            replayed.state.notes.notes[0].basis.span_revisions,
            vec![ProjectionBasisSpan {
                span_id: "span-1".to_string(),
                revision_number: 2,
            }]
        );
    }

    #[test]
    fn materialized_projection_history_replays_graph_retcons_without_active_duplicates() {
        let mut first_event = TranscriptEvent::from(asr_payload("span-graph", 1, "graph basis"));
        let mut second_event = TranscriptEvent::from(asr_payload("span-graph", 2, "graph basis"));
        let mut first_patch = graph_patch(
            1,
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "person:alice".to_string(),
                    name: "Alice".to_string(),
                    entity_type: "person".to_string(),
                    description: None,
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "person:alicia".to_string(),
                    name: "Alicia".to_string(),
                    entity_type: "person".to_string(),
                    description: Some("Early duplicate mention.".to_string()),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "project:audio-graph".to_string(),
                    name: "AudioGraph".to_string(),
                    entity_type: "project".to_string(),
                    description: None,
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "topic:providers".to_string(),
                    name: "Providers".to_string(),
                    entity_type: "topic".to_string(),
                    description: None,
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge:alice:owns".to_string(),
                    source: "person:alice".to_string(),
                    target: "project:audio-graph".to_string(),
                    relation_type: "owns".to_string(),
                    label: Some("owns".to_string()),
                    weight: 0.8,
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge:alicia:owns".to_string(),
                    source: "person:alicia".to_string(),
                    target: "project:audio-graph".to_string(),
                    relation_type: "owns".to_string(),
                    label: Some("owns".to_string()),
                    weight: 0.5,
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge:project:providers".to_string(),
                    source: "project:audio-graph".to_string(),
                    target: "topic:providers".to_string(),
                    relation_type: "tracks".to_string(),
                    label: None,
                    weight: 0.6,
                },
            ],
        );
        first_patch.created_at_ms = 1_700_000_010_000;
        first_event.received_at_ms = first_patch.created_at_ms - 10;

        let mut retcon_patch = graph_patch(
            2,
            vec![
                ProjectionOperation::MergeGraphNodes {
                    source_id: "person:alicia".to_string(),
                    target_id: "person:alice".to_string(),
                },
                ProjectionOperation::SplitGraphNode {
                    id: "topic:providers".to_string(),
                    replacement_nodes: vec![
                        GraphNodeDraft {
                            id: "topic:provider-research".to_string(),
                            name: "Provider research".to_string(),
                            entity_type: "topic".to_string(),
                            description: None,
                        },
                        GraphNodeDraft {
                            id: "topic:provider-implementation".to_string(),
                            name: "Provider implementation".to_string(),
                            entity_type: "topic".to_string(),
                            description: None,
                        },
                    ],
                },
            ],
        );
        retcon_patch.created_at_ms = 1_700_000_020_000;
        second_event.received_at_ms = retcon_patch.created_at_ms - 10;

        let replayed =
            MaterializedProjectionState::replay_accepted_patches_with_transcript_history(
                "session-1",
                [first_event, second_event],
                [first_patch, retcon_patch.clone()],
            )
            .expect("historical graph retcon replay");

        assert_eq!(replayed.validation.checked_patch_count, 2);
        assert_eq!(replayed.validation.invalid_patch_count, 0);
        assert_eq!(replayed.state.graph.last_sequence, retcon_patch.sequence);

        let active_person_nodes: Vec<&MaterializedGraphNode> = replayed
            .state
            .graph
            .nodes
            .iter()
            .filter(|node| node.valid_until_ms.is_none() && node.entity_type == "person")
            .collect();
        assert_eq!(
            active_person_nodes
                .iter()
                .map(|node| node.id.as_str())
                .collect::<Vec<_>>(),
            vec!["person:alice"]
        );
        assert_eq!(
            replayed
                .state
                .graph
                .nodes
                .iter()
                .find(|node| node.id == "person:alicia")
                .and_then(|node| node.valid_until_ms),
            Some(retcon_patch.created_at_ms)
        );

        let active_own_edges: Vec<&MaterializedGraphEdge> = replayed
            .state
            .graph
            .edges
            .iter()
            .filter(|edge| {
                edge.valid_until_ms.is_none()
                    && edge.source == "person:alice"
                    && edge.target == "project:audio-graph"
                    && edge.relation_type == "owns"
            })
            .collect();
        assert_eq!(active_own_edges.len(), 1);
        assert_eq!(active_own_edges[0].weight, 0.8);
        assert_eq!(
            replayed
                .state
                .graph
                .edges
                .iter()
                .filter(|edge| {
                    edge.valid_until_ms.is_none()
                        && (edge.source == "topic:providers" || edge.target == "topic:providers")
                })
                .count(),
            0
        );
        assert!(
            replayed.state.graph.nodes.iter().any(|node| {
                node.id == "topic:provider-research" && node.valid_until_ms.is_none()
            })
        );
        assert!(replayed.state.graph.nodes.iter().any(|node| {
            node.id == "topic:provider-implementation" && node.valid_until_ms.is_none()
        }));
    }

    #[test]
    fn materialized_projection_state_replays_accepted_notes_and_graph_patch_log() {
        let note_patch = notes_patch(1, "note-1", "Decision", "Ship replay.");
        let graph_patch = graph_patch(
            1,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "node-a".to_string(),
                name: "AudioGraph".to_string(),
                entity_type: "Product".to_string(),
                description: None,
            }],
        );

        let replayed = MaterializedProjectionState::replay_accepted_patches(
            "session-1",
            [note_patch, graph_patch],
        )
        .expect("mixed accepted projection replay");

        assert_eq!(replayed.notes.last_sequence, 1);
        assert_eq!(replayed.notes.notes.len(), 1);
        assert_eq!(replayed.graph.last_sequence, 1);
        assert_eq!(replayed.graph.nodes[0].id, "node-a");
    }

    #[test]
    fn materialized_projection_state_rejects_stale_basis_before_mutation() {
        let first = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        let second = TranscriptEvent::from(asr_payload("span-2", 1, "New context."));
        let mut ledger = TranscriptLedger::new("session-1");
        ledger.apply_event(first).expect("first event");
        let old_patch = notes_patch(1, "note-1", "Decision", "Ship notes.");
        ledger.apply_event(second).expect("second event");

        let mut state = MaterializedProjectionState::new("session-1");
        assert_eq!(
            state.apply_validated_patch(&ledger, &old_patch),
            Err(ProjectionApplyError::StaleBasis {
                staleness: ProjectionBasisStaleness::MissingCurrentSpan {
                    span_id: "span-2".to_string(),
                    current_revision: 1,
                },
            })
        );
        assert!(state.notes.notes.is_empty());
        assert_eq!(state.notes.last_sequence, 0);
    }

    fn diarization_payload(
        span_id: &str,
        provider: &str,
        revision_number: u64,
        speaker_id: &str,
        stability: DiarizationSpanStability,
    ) -> DiarizationSpanRevisionPayload {
        let is_final = matches!(stability, DiarizationSpanStability::Final);
        DiarizationSpanRevisionPayload {
            span_id: span_id.to_string(),
            provider: provider.to_string(),
            timeline_id: "session".to_string(),
            source_id: None,
            speaker_id: Some(speaker_id.to_string()),
            speaker_label: Some(format!("Speaker {speaker_id}")),
            channel: None,
            start_time: 1.0,
            end_time: 2.0,
            confidence: Some(0.8),
            is_final,
            stability,
            revision_number,
            supersedes: (revision_number > 1)
                .then(|| format!("{span_id}@rev{}", revision_number - 1)),
            basis_asr_span_ids: vec![format!("{span_id}-asr")],
            basis_transcript_segment_ids: vec![format!("{span_id}-segment")],
            raw_event_ref: Some(format!("{provider}.diar")),
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000 + revision_number,
        }
    }

    #[test]
    fn diarization_span_revision_preserves_provider_local_separation() {
        let revision = DiarizationSpanRevision::from(diarization_payload(
            "span-1",
            "deepgram",
            2,
            "local-1",
            DiarizationSpanStability::Stable,
        ));

        assert_eq!(revision.span_id, "span-1");
        assert_eq!(revision.provider, "deepgram");
        assert_eq!(revision.speaker_id.as_deref(), Some("local-1"));
        // The provider speaker id is never folded into the durable identity.
        assert_eq!(revision.provider_speaker_id, None);
        assert_eq!(revision.stability, DiarizationEventStability::Stable);
        assert_eq!(revision.revision_number, 2);
        assert_eq!(revision.basis_asr_span_ids, vec!["span-1-asr".to_string()]);
    }

    #[test]
    fn diarization_span_revision_debug_redacts_speaker_label() {
        let revision = DiarizationSpanRevision::from(diarization_payload(
            "span-1",
            "deepgram",
            1,
            "SENSITIVE-PERSON",
            DiarizationSpanStability::Provisional,
        ));
        let debug = format!("{revision:?}");
        // span_id/speaker_id are stable identities and surface; the human label
        // is PII and must be redacted.
        assert!(debug.contains("span-1"));
        assert!(debug.contains("SENSITIVE-PERSON"));
        assert!(!debug.contains("Speaker SENSITIVE-PERSON"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn speaker_timeline_collapses_provisional_to_stable_supersede() {
        let provisional = DiarizationSpanRevision::from(diarization_payload(
            "span-1",
            "local_clustering",
            1,
            "spk-1",
            DiarizationSpanStability::Provisional,
        ));
        let stable = DiarizationSpanRevision::from(diarization_payload(
            "span-1",
            "deepgram",
            2,
            "spk-2",
            DiarizationSpanStability::Stable,
        ));

        let mut timeline = SpeakerTimeline::new("session-1");
        timeline.apply_event(provisional).expect("provisional");
        timeline.apply_event(stable).expect("stable supersede");

        assert_eq!(timeline.accepted_event_count, 2);
        assert_eq!(timeline.latest_spans.len(), 1, "remap collapses by span id");
        assert_eq!(
            timeline.latest_spans[0].speaker_id.as_deref(),
            Some("spk-2")
        );
        assert_eq!(
            timeline.latest_spans[0].stability,
            DiarizationEventStability::Stable
        );
        assert_eq!(timeline.latest_spans[0].revision_number, 2);
    }

    #[test]
    fn speaker_timeline_rejects_stale_and_conflicting_revisions() {
        let mut timeline = SpeakerTimeline::new("session-1");
        timeline
            .apply_event(DiarizationSpanRevision::from(diarization_payload(
                "span-1",
                "deepgram",
                2,
                "spk-1",
                DiarizationSpanStability::Stable,
            )))
            .expect("current revision");

        assert_eq!(
            timeline.apply_event(DiarizationSpanRevision::from(diarization_payload(
                "span-1",
                "deepgram",
                1,
                "spk-old",
                DiarizationSpanStability::Provisional,
            ))),
            Err(SpeakerTimelineError::StaleDiarizationRevision {
                span_id: "span-1".to_string(),
                current_revision: 2,
                incoming_revision: 1,
            })
        );

        assert_eq!(
            timeline.apply_event(DiarizationSpanRevision::from(diarization_payload(
                "span-1",
                "deepgram",
                2,
                "spk-conflict",
                DiarizationSpanStability::Final,
            ))),
            Err(SpeakerTimelineError::ConflictingDiarizationRevision {
                span_id: "span-1".to_string(),
                revision_number: 2,
            })
        );
    }

    #[test]
    fn projection_basis_populates_and_validates_speaker_timeline_revisions() {
        let transcript = TranscriptEvent::from(asr_payload("t-span-1", 1, "hello"));
        let timeline = SpeakerTimeline::replay(
            "session-1",
            [
                DiarizationSpanRevision::from(diarization_payload(
                    "d-span-1",
                    "deepgram",
                    2,
                    "spk-1",
                    DiarizationSpanStability::Stable,
                )),
                DiarizationSpanRevision::from(diarization_payload(
                    "d-span-2",
                    "deepgram",
                    1,
                    "spk-2",
                    DiarizationSpanStability::Provisional,
                )),
            ],
        )
        .expect("timeline replay");

        let speaker_spans = timeline.current_basis_spans();
        let basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &speaker_spans,
        );

        assert_eq!(
            basis.diarization_span_revisions,
            vec![
                ProjectionBasisSpan {
                    span_id: "d-span-1".to_string(),
                    revision_number: 2,
                },
                ProjectionBasisSpan {
                    span_id: "d-span-2".to_string(),
                    revision_number: 1,
                },
            ]
        );

        let ledger =
            TranscriptLedger::replay("session-1", [transcript]).expect("transcript ledger replay");
        assert!(
            ledger
                .validate_basis_with_speaker_timeline(&basis, Some(&timeline))
                .is_ok()
        );

        // Without a timeline the diarization basis cannot be checked.
        assert_eq!(
            ledger.validate_basis(&basis),
            Err(ProjectionBasisStaleness::DiarizationBasisUnavailable { count: 2 })
        );
    }

    #[test]
    fn speaker_timeline_validation_reports_diarization_mismatch_reasons() {
        let transcript = TranscriptEvent::from(asr_payload("t-span-1", 1, "hello"));
        let ledger = TranscriptLedger::replay("session-1", [transcript.clone()])
            .expect("transcript ledger replay");

        let mut timeline = SpeakerTimeline::new("session-1");
        timeline
            .apply_event(DiarizationSpanRevision::from(diarization_payload(
                "d-span-1",
                "deepgram",
                2,
                "spk-1",
                DiarizationSpanStability::Stable,
            )))
            .expect("seed diarization span");
        timeline
            .apply_event(DiarizationSpanRevision::from(diarization_payload(
                "d-span-2",
                "deepgram",
                1,
                "spk-2",
                DiarizationSpanStability::Provisional,
            )))
            .expect("seed second diarization span");

        // Basis still references the provisional rev-1 of d-span-1 (now rev-2):
        // stale diarization span. (Also cites d-span-2 at its current rev so the
        // stale check, not the missing-coverage check, fires first.)
        let stale_basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &[
                ProjectionBasisSpan {
                    span_id: "d-span-1".to_string(),
                    revision_number: 1,
                },
                ProjectionBasisSpan {
                    span_id: "d-span-2".to_string(),
                    revision_number: 1,
                },
            ],
        );
        assert_eq!(
            ledger.validate_basis_with_speaker_timeline(&stale_basis, Some(&timeline)),
            Err(ProjectionBasisStaleness::StaleDiarizationSpanRevision {
                span_id: "d-span-1".to_string(),
                current_revision: 2,
                basis_revision: 1,
            })
        );

        // A diarization-consuming basis that cites d-span-1 but omits the
        // equally-current d-span-2: missing current span. (An empty diarization
        // basis is opt-out and would instead validate Ok.)
        let missing_basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &[ProjectionBasisSpan {
                span_id: "d-span-1".to_string(),
                revision_number: 2,
            }],
        );
        assert_eq!(
            ledger.validate_basis_with_speaker_timeline(&missing_basis, Some(&timeline)),
            Err(ProjectionBasisStaleness::MissingCurrentDiarizationSpan {
                span_id: "d-span-2".to_string(),
                current_revision: 1,
            })
        );

        // An empty diarization basis is opt-out: the timeline does not gate a
        // projection that consumed no speaker spans.
        let opt_out_basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &[],
        );
        assert!(
            ledger
                .validate_basis_with_speaker_timeline(&opt_out_basis, Some(&timeline))
                .is_ok()
        );

        // Basis references a span the timeline never saw: unknown basis span.
        // Cites both current spans so the unknown-span check fires (not the
        // missing-coverage check).
        let unknown_basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &[
                ProjectionBasisSpan {
                    span_id: "d-span-1".to_string(),
                    revision_number: 2,
                },
                ProjectionBasisSpan {
                    span_id: "d-span-2".to_string(),
                    revision_number: 1,
                },
                ProjectionBasisSpan {
                    span_id: "d-span-ghost".to_string(),
                    revision_number: 1,
                },
            ],
        );
        assert_eq!(
            ledger.validate_basis_with_speaker_timeline(&unknown_basis, Some(&timeline)),
            Err(ProjectionBasisStaleness::UnknownDiarizationBasisSpan {
                span_id: "d-span-ghost".to_string(),
                basis_revision: 1,
            })
        );
    }
}
