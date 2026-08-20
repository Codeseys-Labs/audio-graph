//! Event-sourced transcript/notes/graph projection contracts.
//!
//! This module defines the durable data model for the dynamic synthesis queue:
//! transcript span revisions are the source events, projection jobs record the
//! exact basis they were built from, and projection patches carry replayable
//! operations for notes and graph state. These types are wired into persistence
//! (`persistence::append_projection_patch` / `load_projection_patches`) and the
//! LLM projection queue (`projection_scheduler`, `projection_llm`).

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

pub(crate) const REDACTED_DEBUG_VALUE: &str = "<redacted>";

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

fn projection_event_is_eligible(event: &TranscriptEvent) -> bool {
    event.is_final
        || event.end_of_turn
        || matches!(event.stability, TranscriptEventStability::Final)
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

/// Number of most-recent transcript turns fed to the projection LLM verbatim.
///
/// Older turns are folded into an incremental rolling summary rather than
/// re-serialized in full on every submission (ADR-0025 §2c, seed
/// audio-graph-18ee — kills the O(n²) full-transcript re-feed). This constant
/// is shared by [`ProjectionBasis`] (which records the summarized-through
/// revision boundary) and the projection prompt window so the two never
/// disagree about where the summary ends and the verbatim hot buffer begins.
pub const ROLLING_SUMMARY_HOT_WINDOW_TURNS: usize = 6;

/// Order transcript events into the canonical replay order used for windowing.
///
/// Matches [`transcript_events_hash`]'s sort so the "older vs. hot buffer" split
/// is deterministic and independent of the incoming slice order.
pub(crate) fn ordered_for_window(events: &[TranscriptEvent]) -> Vec<&TranscriptEvent> {
    let mut ordered: Vec<&TranscriptEvent> = events.iter().collect();
    ordered.sort_by(|a, b| {
        millis(a.start_time)
            .cmp(&millis(b.start_time))
            .then(millis(a.end_time).cmp(&millis(b.end_time)))
            .then(a.span_id.cmp(&b.span_id))
            .then(a.revision_number.cmp(&b.revision_number))
    });
    ordered
}

/// The highest revision number among the "older" turns that fall outside the
/// verbatim hot window and are therefore covered by the rolling summary.
///
/// Returns `None` when the whole transcript still fits in the hot window (no
/// summary yet). This is a pure, deterministic function of the deduped latest
/// events, so two bases built from the same transcript state always agree.
fn summarized_through_revision(latest_events: &[TranscriptEvent]) -> Option<u64> {
    if latest_events.len() <= ROLLING_SUMMARY_HOT_WINDOW_TURNS {
        return None;
    }
    let ordered = ordered_for_window(latest_events);
    let older_len = ordered.len() - ROLLING_SUMMARY_HOT_WINDOW_TURNS;
    ordered[..older_len]
        .iter()
        .map(|event| event.revision_number)
        .max()
}

/// Exact transcript/diarization basis for a queued or completed projection.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptHashVersion {
    #[default]
    V1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionBasis {
    pub span_revisions: Vec<ProjectionBasisSpan>,
    pub diarization_span_revisions: Vec<ProjectionBasisSpan>,
    pub transcript_hash: String,
    /// Highest transcript revision folded into the rolling summary of older
    /// turns (ADR-0025 §2c / seed audio-graph-18ee). `None` while the whole
    /// transcript still fits in the verbatim hot window. Recorded on the basis
    /// so `validate_basis` rejects a completion whose summary window no longer
    /// matches the current ledger — the staleness guarantee must survive the
    /// windowed feed. `skip_serializing_if` keeps older bases (and the frontend
    /// IPC shape) byte-identical when no summary exists yet.
    pub summarized_through_revision: Option<u64>,
}

impl serde::Serialize for ProjectionBasis {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;

        let field_count = if self.summarized_through_revision.is_some() {
            5
        } else {
            4
        };
        let mut state = serializer.serialize_struct("ProjectionBasis", field_count)?;
        state.serialize_field("span_revisions", &self.span_revisions)?;
        state.serialize_field(
            "diarization_span_revisions",
            &self.diarization_span_revisions,
        )?;
        state.serialize_field("transcript_hash", &self.transcript_hash)?;
        state.serialize_field("hash_version", &TranscriptHashVersion::V1)?;
        if let Some(revision) = self.summarized_through_revision {
            state.serialize_field("summarized_through_revision", &revision)?;
        }
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for ProjectionBasis {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Wire {
            span_revisions: Vec<ProjectionBasisSpan>,
            diarization_span_revisions: Vec<ProjectionBasisSpan>,
            transcript_hash: String,
            #[serde(default)]
            hash_version: TranscriptHashVersion,
            #[serde(default)]
            summarized_through_revision: Option<u64>,
        }

        let wire = Wire::deserialize(deserializer)?;
        match wire.hash_version {
            TranscriptHashVersion::V1 => Ok(Self {
                span_revisions: wire.span_revisions,
                diarization_span_revisions: wire.diarization_span_revisions,
                transcript_hash: wire.transcript_hash,
                summarized_through_revision: wire.summarized_through_revision,
            }),
        }
    }
}

impl ProjectionBasis {
    pub fn hash_version(&self) -> TranscriptHashVersion {
        TranscriptHashVersion::V1
    }

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
            transcript_hash: transcript_events_hash_v1(&latest_events),
            summarized_through_revision: summarized_through_revision(&latest_events),
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

/// A human-facing speaker-label change produced by
/// [`SpeakerTimeline::apply_event`] when a span's `speaker_label` is remapped
/// from one non-empty label to a different one.
///
/// This is the durable signal that drives the knowledge-graph entity retcon:
/// the `superseded_label` entity's relations are folded onto the
/// `canonical_label` entity via
/// [`crate::graph::TemporalKnowledgeGraph::supersede_entity`]. Labels can carry
/// PII, so the `Debug` impl redacts them.
#[derive(Clone, PartialEq, Eq)]
pub struct SpeakerLabelRemap {
    pub superseded_label: String,
    pub canonical_label: String,
}

impl fmt::Debug for SpeakerLabelRemap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpeakerLabelRemap")
            .field("superseded_label", &REDACTED_DEBUG_VALUE)
            .field("canonical_label", &REDACTED_DEBUG_VALUE)
            .finish()
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
            let _ = timeline.apply_event(event)?;
        }
        Ok(timeline)
    }

    pub fn apply_event(
        &mut self,
        event: DiarizationSpanRevision,
    ) -> Result<Option<SpeakerLabelRemap>, SpeakerTimelineError> {
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
                let remap = Self::detect_label_remap(
                    current.speaker_label.as_deref(),
                    event.speaker_label.as_deref(),
                );
                *current = event;
                self.accepted_event_count += 1;
                self.sort_latest_spans();
                Ok(remap)
            }
            None => {
                self.latest_spans.push(event);
                self.accepted_event_count += 1;
                self.sort_latest_spans();
                Ok(None)
            }
        }
    }

    fn detect_label_remap(
        old_label: Option<&str>,
        new_label: Option<&str>,
    ) -> Option<SpeakerLabelRemap> {
        let old = old_label.map(str::trim).filter(|s| !s.is_empty())?;
        let new = new_label.map(str::trim).filter(|s| !s.is_empty())?;
        if old == new {
            return None;
        }
        Some(SpeakerLabelRemap {
            superseded_label: old.to_string(),
            canonical_label: new.to_string(),
        })
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

/// Reason a diarization revision was refused by [`SpeakerTimeline::apply_event`].
///
/// Mirrors [`TranscriptLedgerError`]: an out-of-order revision for a span is
/// `StaleDiarizationRevision`, and a same-revision payload that disagrees with
/// the accepted one is `ConflictingDiarizationRevision`. Serialized
/// tag-and-snake-case so it can round-trip in replay diagnostics.
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

    /// Basis visible to automatic notes/graph projection work. Provisional ASR
    /// revisions stay durable in the transcript ledger but cannot enter an LLM
    /// prompt or create follow-up churn until a final/end-of-turn revision
    /// replaces them.
    pub fn current_projection_basis(&self) -> ProjectionBasis {
        let eligible: Vec<TranscriptEvent> = self
            .latest_spans
            .iter()
            .filter(|event| projection_event_is_eligible(event))
            .cloned()
            .collect();
        ProjectionBasis::from_transcript_events(&eligible)
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
        match self.classify_basis_currency(basis, speaker_timeline) {
            BasisCurrency::Current => Ok(()),
            BasisCurrency::AppendOnlyStale(staleness) | BasisCurrency::Revised(staleness) => {
                Err(staleness)
            }
        }
    }

    /// Classify whether a projection basis is current, stale only because the
    /// transcript grew, or invalid because content covered by the basis was
    /// revised. This is the single source of truth for both the live apply
    /// gate and the legacy two-way [`Self::validate_basis`] API.
    pub fn classify_basis_currency(
        &self,
        basis: &ProjectionBasis,
        speaker_timeline: Option<&SpeakerTimeline>,
    ) -> BasisCurrency {
        let diarization_staleness = match speaker_timeline {
            Some(timeline) => timeline.validate_diarization_basis(basis).err(),
            None if !basis.diarization_span_revisions.is_empty() => {
                Some(ProjectionBasisStaleness::DiarizationBasisUnavailable {
                    count: basis.diarization_span_revisions.len(),
                })
            }
            None => None,
        };
        if let Some(staleness) = diarization_staleness {
            return BasisCurrency::Revised(staleness);
        }

        let basis_spans: BTreeMap<&str, u64> = basis
            .span_revisions
            .iter()
            .map(|span| (span.span_id.as_str(), span.revision_number))
            .collect();
        // Projection jobs created by the current scheduler never include
        // provisional events. Historical bases may legitimately contain them,
        // so remember that distinction for the extra-span classification below
        // while validating every covered span against the full ledger.
        let basis_tracks_partial = self.latest_spans.iter().any(|event| {
            !projection_event_is_eligible(event)
                && basis_spans.get(event.span_id.as_str()) == Some(&event.revision_number)
        });
        let projection_events = self.latest_spans.clone();
        let current_basis = ProjectionBasis::from_transcript_events(&projection_events);
        let current_spans: BTreeMap<&str, u64> = current_basis
            .span_revisions
            .iter()
            .map(|span| (span.span_id.as_str(), span.revision_number))
            .collect();

        // First prove that every span the patch actually covered still exists
        // at the exact revision it saw. A revision or removal invalidates the
        // patch even when unrelated spans were also appended later.
        for (span_id, basis_revision) in &basis_spans {
            match current_spans.get(*span_id) {
                Some(current_revision) if current_revision == basis_revision => {}
                Some(current_revision) => {
                    return BasisCurrency::Revised(ProjectionBasisStaleness::StaleSpanRevision {
                        span_id: (*span_id).to_string(),
                        current_revision: *current_revision,
                        basis_revision: *basis_revision,
                    });
                }
                None => {
                    return BasisCurrency::Revised(ProjectionBasisStaleness::UnknownBasisSpan {
                        span_id: (*span_id).to_string(),
                        basis_revision: *basis_revision,
                    });
                }
            }
        }

        // Hash exactly the current-ledger subset covered by the basis before
        // inspecting later appends. Comparing against the full current hash
        // would misclassify every legitimate append, while skipping this check
        // would let a forged/corrupt hash through when ids and revisions match.
        let covered_events: Vec<TranscriptEvent> = projection_events
            .iter()
            .filter(|event| basis_spans.contains_key(event.span_id.as_str()))
            .cloned()
            .collect();
        let covered_basis = ProjectionBasis::from_transcript_events(&covered_events);

        if covered_basis.span_revisions.len() != basis.span_revisions.len() {
            return BasisCurrency::Revised(ProjectionBasisStaleness::CoveredSpanCountMismatch {
                current_count: covered_basis.span_revisions.len(),
                basis_count: basis.span_revisions.len(),
            });
        }

        for (index, (current_span, basis_span)) in covered_basis
            .span_revisions
            .iter()
            .zip(&basis.span_revisions)
            .enumerate()
        {
            if current_span != basis_span {
                return BasisCurrency::Revised(
                    ProjectionBasisStaleness::CoveredSpanOrderMismatch {
                        index,
                        current_span_id: current_span.span_id.clone(),
                        basis_span_id: basis_span.span_id.clone(),
                    },
                );
            }
        }

        if covered_basis.transcript_hash != basis.transcript_hash {
            return BasisCurrency::Revised(ProjectionBasisStaleness::TranscriptHashMismatch {
                current_hash: covered_basis.transcript_hash,
                basis_hash: basis.transcript_hash.clone(),
            });
        }

        // Preserve legacy bases whose summary boundary field was absent, but
        // reject an explicit boundary that disagrees with the exact covered
        // subset. This check also happens before append-only classification.
        if let (Some(basis_summarized), Some(covered_summarized)) = (
            basis.summarized_through_revision,
            covered_basis.summarized_through_revision,
        ) && basis_summarized != covered_summarized
        {
            return BasisCurrency::Revised(ProjectionBasisStaleness::SummaryWindowMismatch {
                current_summarized_through: covered_basis.summarized_through_revision,
                basis_summarized_through: basis.summarized_through_revision,
            });
        }

        if current_basis.span_revisions.len() == basis.span_revisions.len() {
            return BasisCurrency::Current;
        }

        // A final-only projection basis is still current when the only new
        // ledger entries are provisional. They remain durable transcript
        // revisions but are intentionally invisible to the projection queue.
        // Keep legacy partial-bearing and empty bases on the full-ledger rule.
        if !basis_tracks_partial
            && !basis_spans.is_empty()
            && projection_events.iter().all(|event| {
                basis_spans.contains_key(event.span_id.as_str())
                    || !projection_event_is_eligible(event)
            })
        {
            return BasisCurrency::Current;
        }

        // A valid append-only basis must cover the exact chronological prefix.
        // `ProjectionBasis::span_revisions` is deterministically keyed by span
        // id, so its vector position cannot prove audio chronology. Inspect the
        // current events in the same timestamp order used by transcript hashes
        // and rolling windows instead. An uncovered event inside the first N
        // events is a non-tail insertion, not a harmless append.
        let currency_events: Vec<TranscriptEvent> = projection_events
            .iter()
            .filter(|event| {
                basis_spans.is_empty()
                    || basis_tracks_partial
                    || basis_spans.contains_key(event.span_id.as_str())
                    || projection_event_is_eligible(event)
            })
            .cloned()
            .collect();
        let ordered_current = ordered_for_window(&currency_events);
        if let Some((_, inserted)) = ordered_current
            .iter()
            .take(basis_spans.len())
            .enumerate()
            .find(|(_, event)| !basis_spans.contains_key(event.span_id.as_str()))
        {
            return BasisCurrency::Revised(ProjectionBasisStaleness::MissingCurrentSpan {
                span_id: inserted.span_id.clone(),
                current_revision: inserted.revision_number,
            });
        }

        let extra = ordered_current[basis_spans.len()];
        BasisCurrency::AppendOnlyStale(ProjectionBasisStaleness::MissingCurrentSpan {
            span_id: extra.span_id.clone(),
            current_revision: extra.revision_number,
        })
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

/// Derive the legacy [`TranscriptSegment`](crate::state::TranscriptSegment)
/// view from an event-sourced [`TranscriptLedger`].
///
/// The ledger already collapses each stable span to its latest accepted
/// revision (partials superseded by their final span), so the derived view is
/// duplicate-free: exactly one segment per surviving span, in the ledger's
/// canonical start-time ordering. This is a read-only projection — it never
/// mutates the ledger or the underlying event log.
///
/// Segment ids prefer the span's `transcript_segment_id` (the provider's
/// stable segment identity) and fall back to the immutable `span_id` so the
/// derived view stays deterministic across replays.
pub fn derive_legacy_transcript_segments(
    ledger: &TranscriptLedger,
) -> Vec<crate::state::TranscriptSegment> {
    ledger
        .latest_spans
        .iter()
        .map(|event| crate::state::TranscriptSegment {
            id: event
                .transcript_segment_id
                .clone()
                .unwrap_or_else(|| event.span_id.clone()),
            source_id: event.source_id.clone(),
            speaker_id: event.speaker_id.clone(),
            speaker_label: event.speaker_label.clone(),
            text: event.text.clone(),
            start_time: event.start_time,
            end_time: event.end_time,
            confidence: event.confidence,
        })
        .collect()
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

/// Why a [`ProjectionBasis`] no longer matches the current ledgers.
///
/// Returned by [`TranscriptLedger::validate_basis_with_speaker_timeline`] and
/// [`SpeakerTimeline::validate_diarization_basis`]. The `*Span`/`TranscriptHash`
/// variants describe transcript-basis drift; the `*Diarization*` variants
/// describe speaker-timeline drift. `DiarizationBasisUnavailable` is the special
/// case where a patch cites diarization spans but no [`SpeakerTimeline`] was
/// supplied to check them against.
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
    CoveredSpanCountMismatch {
        current_count: usize,
        basis_count: usize,
    },
    CoveredSpanOrderMismatch {
        index: usize,
        current_span_id: String,
        basis_span_id: String,
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
    /// The rolling-summary window recorded on the basis no longer matches the
    /// current ledger (ADR-0025 §2c / seed audio-graph-18ee). A completion built
    /// against a summary that folded through a different revision boundary is
    /// stale even when every hot-buffer span still matches — the windowed feed
    /// must not weaken the ADR-0024 staleness guarantee.
    SummaryWindowMismatch {
        current_summarized_through: Option<u64>,
        basis_summarized_through: Option<u64>,
    },
}

/// Relationship between a projection basis and the current transcript state.
///
/// `AppendOnlyStale` carries the same error that the two-way `validate_basis`
/// API returns, while allowing the scheduler to retain a proven append-only
/// prefix and coalesce one follow-up. Durable materializer visibility remains
/// gated by ADR-0027's Accepted commit boundary. `Revised` always means content
/// covered by the patch can no longer be trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BasisCurrency {
    Current,
    AppendOnlyStale(ProjectionBasisStaleness),
    Revised(ProjectionBasisStaleness),
}

/// What the apply gate proved about a patch's basis at the moment it applied.
///
/// Both variants reach the materializer because [`BasisCurrency::Current`]
/// and [`BasisCurrency::AppendOnlyStale`] both prove every span the basis
/// pinned still resolves at its pinned revision — only [`BasisCurrency::Revised`]
/// breaks that proof, and it never reaches this type. Callers that need to
/// split applied-append-only telemetry from the ordinary current-basis path
/// (audio-graph-caad) read this instead of re-deriving [`BasisCurrency`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppliedBasisCurrency {
    Current,
    AppendedTail { staleness: ProjectionBasisStaleness },
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
    /// Content-free route evidence for this patch (ADR-0038): wire skin,
    /// normalized terminal status, retry class, constrained-decoding grade,
    /// completion-budget clamp, and the upstream provider that served the request.
    /// Patch-level — one per patch, deliberately NOT multiplied into every
    /// materialized item the way [`ProjectionProvenance`] is. `None` on records
    /// written before this contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<crate::llm::route::RouteRecord>,
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
            .field("route", &self.route)
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
                evidence,
            } => f
                .debug_struct("UpsertNote")
                .field("id", id)
                .field("title", &REDACTED_DEBUG_VALUE)
                .field("body", &REDACTED_DEBUG_VALUE)
                .field("tags", tags)
                .field("evidence", &DebugEvidenceAnchor(evidence))
                .finish(),
            ProjectionOperation::DeleteNote { id } => {
                f.debug_struct("DeleteNote").field("id", id).finish()
            }
            ProjectionOperation::InvalidateNote { id } => {
                f.debug_struct("InvalidateNote").field("id", id).finish()
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
                evidence,
            } => f
                .debug_struct("UpsertGraphNode")
                .field("id", id)
                .field("name", &REDACTED_DEBUG_VALUE)
                .field("entity_type", &REDACTED_DEBUG_VALUE)
                .field(
                    "description",
                    &description.as_ref().map(|_| REDACTED_DEBUG_VALUE),
                )
                .field("evidence", &DebugEvidenceAnchor(evidence))
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
                evidence,
            } => f
                .debug_struct("UpsertGraphEdge")
                .field("id", id)
                .field("source", source)
                .field("target", target)
                .field("relation_type", &REDACTED_DEBUG_VALUE)
                .field("label", &label.as_ref().map(|_| REDACTED_DEBUG_VALUE))
                .field("weight", weight)
                .field("evidence", &DebugEvidenceAnchor(evidence))
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

/// Redacts an [`EvidenceAnchor`](crate::claim_evidence::EvidenceAnchor)'s
/// content-bearing fields for debug logs, mirroring the `title`/`body`/`name`
/// redaction above. `claim_class` and `span_id` are never redacted: they are
/// structural metadata (a class tag and an id), not transcript content, and
/// `judge_claim_evidence`'s span-resolution and basis-laundering checks are
/// exactly what a debugger needs to see when a patch is rejected.
struct DebugEvidenceAnchor<'a>(&'a crate::claim_evidence::EvidenceAnchor);

impl fmt::Debug for DebugEvidenceAnchor<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EvidenceAnchor")
            .field("claim_class", &self.0.claim_class)
            .field("span_id", &self.0.span_id)
            .field(
                "quote",
                &self.0.quote.as_ref().map(|_| REDACTED_DEBUG_VALUE),
            )
            .field("note", &self.0.note.as_ref().map(|_| REDACTED_DEBUG_VALUE))
            .finish()
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

/// Per-item LLM provenance, cloned into every materialized note / node / edge and
/// persisted, so per-item byte cost governs what may live here (ADR-0038).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProjectionProvenance {
    /// The REGISTRY provider id the route was authorized against (`llm.openrouter`,
    /// `llm.cerebras`, …). Before ADR-0038 this carried a fourth ad-hoc naming
    /// scheme (`"api"` / `"openrouter"` / `"local_llama"` / `"mistralrs"`).
    pub provider: String,
    /// The SERVED model when the response reported one; see
    /// [`model_source`](Self::model_source), which says which it is rather than
    /// leaving a reader to assume.
    pub model: String,
    pub prompt_id: String,
    /// The stamped route id (`route.cerebras_via_openrouter`, …). `None` on records
    /// written before this contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_id: Option<String>,
    /// Whether [`model`](Self::model) is the served id or the requested one.
    /// Defaults to `Requested` so pre-contract records are read honestly and never
    /// mistaken for served identity.
    #[serde(default)]
    pub model_source: crate::llm::route::ModelIdentitySource,
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
        /// Untrusted claim-class evidence anchor (ADR-0037 part 2). `#[serde(default)]`
        /// is the backward-compat fallback for a `ProjectionOperation` persisted
        /// before this contract (ADR-0027) — see `EvidenceAnchor`'s `Default` impl,
        /// which lands on the one class `judge_claim_evidence` always refuses, so a
        /// FRESH draft that omits this field is refused rather than silently
        /// admitted.
        #[serde(default)]
        evidence: crate::claim_evidence::EvidenceAnchor,
    },
    DeleteNote {
        id: String,
    },
    /// Parity with `InvalidateGraphNode` / `InvalidateGraphEdge` (ADR-0037 part
    /// 4) — notes previously had only a hard `DeleteNote`, with no
    /// history-preserving retraction. Materialization currently treats this the
    /// same as `DeleteNote` (`MaterializedNotes` carries no `valid_until_ms`
    /// the way the graph structs do); giving notes that same soft-invalidate
    /// temporal tracking is a separate, unscoped change (flagged, not decided
    /// here — see audio-graph-2cf9's open questions).
    ///
    /// ADR-0037 part 4: "corrections and retractions are derived, not
    /// model-authored" — ADR-0031's pinned-revision advance is the mechanical
    /// trigger, not free-form model judgement. Because this variant's
    /// materialization is a hard delete with no `valid_until_ms` trace (the
    /// unscoped gap above), a model-authored one would be an UNRECOVERABLE
    /// hallucination rather than the auditable, reversible soft-invalidate
    /// `InvalidateGraphNode`/`Edge` give. `projection_llm::validate_operation`
    /// therefore refuses this variant in every model-submitted draft
    /// (`ProjectionPatchDraftError::DerivedOnlyOperation`); the variant stays
    /// defined here for a future ADR-0031-derived (trusted-code-only) caller,
    /// not for the LLM-facing draft path.
    InvalidateNote {
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
        /// See `UpsertNote::evidence`.
        #[serde(default)]
        evidence: crate::claim_evidence::EvidenceAnchor,
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
        /// See `UpsertNote::evidence`.
        #[serde(default)]
        evidence: crate::claim_evidence::EvidenceAnchor,
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
    /// Admitted per-item claim evidence (ADR-0037), re-judged at apply time
    /// via [`resolve_admitted_claim_evidence`] against the SAME basis-covered
    /// `TranscriptEvent`s the source operation's `EvidenceAnchor` was already
    /// judged `Admitted` against once, at draft-admission time
    /// (`projection_llm::trusted_projection_patch_from_model_json`) — the
    /// ledger's basis-validation check just before materialization
    /// (`apply_validated_patch_with_speaker_timeline_opt`) proves those two
    /// resolutions read the identical, immutable transcript content, so this
    /// is a deterministic re-read, not a second judgement. `None` on notes
    /// materialized before this contract, or on the replay path (no ledger
    /// snapshot at this patch's exact basis) — never confused with a
    /// class-satisfying absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<crate::claim_evidence::AdmittedClaimEvidence>,
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
            .field("evidence", &self.evidence)
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

    /// `evidence_basis` is the basis-covered `TranscriptEvent` map to
    /// re-judge each `UpsertNote`'s `EvidenceAnchor` against (ADR-0037);
    /// `None` for callers with no ledger snapshot at this patch's exact basis
    /// (replay — see [`MaterializedProjectionState::apply_replayed_patch`]),
    /// in which case every note materializes with `evidence: None`.
    pub fn apply_patch(
        &mut self,
        patch: &ProjectionPatch,
        evidence_basis: Option<&BTreeMap<&str, &TranscriptEvent>>,
    ) -> Result<(), ProjectionApplyError> {
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
                    evidence,
                } => next.upsert_note(id, title, body, tags, evidence, evidence_basis, patch),
                // InvalidateNote is applied identically to DeleteNote today:
                // `MaterializedNote` carries no `valid_until_ms` the way
                // `MaterializedGraphNode`/`Edge` do, so there is no
                // history-preserving state to set. The variant exists for
                // typed-operation parity with `InvalidateGraphNode`/
                // `InvalidateGraphEdge` (ADR-0037 part 4); giving notes that
                // same soft-invalidate temporal tracking is a separate,
                // unscoped change (flagged, not decided here).
                ProjectionOperation::DeleteNote { id }
                | ProjectionOperation::InvalidateNote { id } => {
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

    #[allow(clippy::too_many_arguments)]
    fn upsert_note(
        &mut self,
        id: &str,
        title: &str,
        body: &str,
        tags: &[String],
        evidence: &crate::claim_evidence::EvidenceAnchor,
        evidence_basis: Option<&BTreeMap<&str, &TranscriptEvent>>,
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
            evidence: resolve_admitted_claim_evidence(evidence, evidence_basis),
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
    /// See [`MaterializedNote::evidence`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<crate::claim_evidence::AdmittedClaimEvidence>,
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
            .field("evidence", &self.evidence)
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
    /// See [`MaterializedNote::evidence`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<crate::claim_evidence::AdmittedClaimEvidence>,
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
            .field("evidence", &self.evidence)
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

    /// See [`MaterializedNotes::apply_patch`] for what `evidence_basis` means
    /// and when it is `None`.
    pub fn apply_patch(
        &mut self,
        patch: &ProjectionPatch,
        evidence_basis: Option<&BTreeMap<&str, &TranscriptEvent>>,
    ) -> Result<(), ProjectionApplyError> {
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
                    evidence,
                } => next.upsert_node(
                    id,
                    name,
                    entity_type,
                    description.clone(),
                    evidence,
                    evidence_basis,
                    patch,
                ),
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
                    evidence,
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
                        evidence,
                        evidence_basis,
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
                | ProjectionOperation::InvalidateNote { .. }
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

    #[allow(clippy::too_many_arguments)]
    fn upsert_node(
        &mut self,
        id: &str,
        name: &str,
        entity_type: &str,
        description: Option<String>,
        evidence: &crate::claim_evidence::EvidenceAnchor,
        evidence_basis: Option<&BTreeMap<&str, &TranscriptEvent>>,
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
            evidence: resolve_admitted_claim_evidence(evidence, evidence_basis),
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
        evidence: &crate::claim_evidence::EvidenceAnchor,
        evidence_basis: Option<&BTreeMap<&str, &TranscriptEvent>>,
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
            evidence: resolve_admitted_claim_evidence(evidence, evidence_basis),
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
        // `SplitGraphNode`'s synthesized replacement nodes carry no evidence
        // anchor of their own (`GraphNodeDraft` has no `evidence` field) —
        // per this ticket's scope, evidence is required only for the three
        // pure content-creating Upsert* operations, not the structural/
        // derived-adjacent graph ops (`Strengthen`/`Weaken`/`Merge`/`Split`).
        // `EvidenceAnchor::default()` + no basis is therefore correct here,
        // not a shortcut: there is nothing to resolve.
        for replacement in replacement_nodes {
            self.upsert_node(
                &replacement.id,
                &replacement.name,
                &replacement.entity_type,
                replacement.description.clone(),
                &crate::claim_evidence::EvidenceAnchor::default(),
                None,
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
    SpeakerReplay {
        sequence: u64,
        error: SpeakerTimelineError,
    },
}

/// Reconstruct the transcript ledger and (when the speaker stream is
/// present) the speaker timeline from the canonical replay streams, folding
/// in every event received at or before `bound_ms`.
///
/// Used by [`MaterializedProjectionState::replay_accepted_patches_with_history`]
/// to widen the basis-currency check past a patch's `created_at_ms` toward
/// the live apply gate's actual apply-time view (audio-graph-f3d4 review
/// fix). Unlike the `created_at_ms`-bounded evidence ledger built alongside
/// it, a failure to fold an event here degrades to whatever was already
/// folded rather than invalidating the patch: the authoritative validity
/// check for the accepted transcript/speaker streams already happened while
/// building that `created_at_ms`-bounded ledger, and every event considered
/// here was already folded there too (or arrived later in the same
/// monotonic, pre-sorted stream) — this reconstruction only ever extends
/// coverage, so it degrades safely, not lossily, for a currency
/// determination that solely needs to prove the basis-covered spans still
/// resolve.
fn replay_ledger_and_timeline_up_to(
    session_id: &str,
    transcript_events: &[TranscriptEvent],
    speaker_events: &[DiarizationSpanRevision],
    speaker_history_present: bool,
    bound_ms: u64,
) -> (TranscriptLedger, Option<SpeakerTimeline>) {
    let mut ledger = TranscriptLedger::new(session_id);
    for event in transcript_events
        .iter()
        .take_while(|event| event.received_at_ms <= bound_ms)
    {
        if ledger.apply_event(event.clone()).is_err() {
            break;
        }
    }
    let mut timeline = speaker_history_present.then(|| SpeakerTimeline::new(session_id));
    if let Some(timeline) = timeline.as_mut() {
        for event in speaker_events
            .iter()
            .take_while(|event| event.received_at_ms <= bound_ms)
        {
            if timeline.apply_event(event.clone()).is_err() {
                break;
            }
        }
    }
    (ledger, timeline)
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
    /// This method does no basis classification of its own — it trusts the
    /// accepted log and replays materializer operations deterministically.
    /// Live runtime calls go through
    /// [`Self::apply_validated_patch_reporting_currency`] (the sole caller is
    /// `state.rs`), which classifies a patch's basis three ways *before* the
    /// patch is accepted: `Current` and a proven append-only tail
    /// (`AppliedBasisCurrency::AppendedTail`) both apply, while `Revised` —
    /// content the patch covered was itself superseded — is rejected as
    /// `ProjectionApplyError::StaleBasis` and never reaches the accepted log.
    /// [`Self::replay_accepted_patches_with_history`] re-derives that same
    /// three-way classification against each patch's `created_at_ms`-bounded
    /// history immediately before calling this method, so a later
    /// append-only transcript span still applies here instead of being
    /// mistaken for staleness — only a genuinely `Revised` span is rejected
    /// before reaching this method. [`Self::replay_accepted_patches`] has no
    /// transcript history to classify against at all, so it calls this
    /// method unconditionally and trusts the log outright.
    ///
    /// `evidence_basis` is the SAME kind of basis-covered, speaker-corrected
    /// event map [`resolve_claim_evidence_basis_events`] builds for the live
    /// apply path (ADR-0037). [`Self::replay_accepted_patches_with_history`]
    /// reconstructs a per-patch `TranscriptLedger` (and, when available,
    /// `SpeakerTimeline`) at each patch's exact `created_at_ms` precisely so
    /// it can pass one here — this used to be an unconditional `None` on
    /// every replayed item, silently dropping ADR-0037's per-item evidence
    /// (and ADR-0031's per-item pinned revision) on every rebuild from the
    /// canonical log, and making "canonical history remains replayable"
    /// false for any fixture with real evidence (see `state.rs`'s
    /// `runtime_projection_patch_applies_append_only_basis_and_replays_identically`,
    /// whose fixture previously only passed because it used the one anchor
    /// class that always refuses). `None` stays legitimate for
    /// [`Self::replay_accepted_patches`], which has no transcript history at
    /// all to reconstruct a basis from — every item there still materializes
    /// with `evidence: None`, but that caller has no other option, not a
    /// deliberately-skipped one.
    pub fn apply_replayed_patch(
        &mut self,
        patch: &ProjectionPatch,
        evidence_basis: Option<&BTreeMap<&str, &TranscriptEvent>>,
    ) -> Result<MaterializedProjectionApplyOutcome, ProjectionApplyError> {
        match patch.kind {
            ProjectionKind::Notes => {
                self.notes.apply_patch(patch, evidence_basis)?;
                Ok(MaterializedProjectionApplyOutcome::Notes {
                    last_sequence: self.notes.last_sequence,
                    note_count: self.notes.notes.len(),
                })
            }
            ProjectionKind::Graph => {
                self.graph.apply_patch(patch, evidence_basis)?;
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
            state.apply_replayed_patch(&patch, None)?;
        }
        Ok(state)
    }

    pub fn replay_accepted_patches_with_transcript_history(
        session_id: impl Into<String>,
        transcript_events: impl IntoIterator<Item = TranscriptEvent>,
        patches: impl IntoIterator<Item = ProjectionPatch>,
    ) -> Result<HistoricalProjectionReplay, ProjectionApplyError> {
        Self::replay_accepted_patches_with_history(session_id, transcript_events, None, patches)
    }

    /// Replay accepted patches against the transcript and optional speaker
    /// histories that were visible when each patch was created.
    ///
    /// Claim evidence is resolved against exactly that `created_at_ms`
    /// snapshot (draft-time visibility). The basis-currency classification
    /// that gates acceptance instead widens to `created_at_ms +
    /// generation_latency_ms` so it agrees with the live apply gate, which
    /// re-checks against a fresh snapshot taken after generation completes,
    /// not the draft-time one (see `replay_ledger_and_timeline_up_to`).
    ///
    /// `None` means the canonical speaker stream is unavailable. `Some(vec![])`
    /// means that stream is present and authoritatively empty.
    pub fn replay_accepted_patches_with_history(
        session_id: impl Into<String>,
        transcript_events: impl IntoIterator<Item = TranscriptEvent>,
        speaker_events: Option<Vec<DiarizationSpanRevision>>,
        patches: impl IntoIterator<Item = ProjectionPatch>,
    ) -> Result<HistoricalProjectionReplay, ProjectionApplyError> {
        let session_id = session_id.into();
        let mut state = Self::new(&session_id);
        let speaker_history_present = speaker_events.is_some();
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
        let mut speaker_events = speaker_events.unwrap_or_default();
        // Preserve canonical stream order when two speaker revisions share a
        // receipt timestamp. Boundary corrections can move a later revision's
        // start/end earlier, so timeline fields must not break timestamp ties.
        speaker_events.sort_by_key(|event| event.received_at_ms);

        'patches: for patch in patches {
            validation.checked_patch_count += 1;
            let mut evidence_ledger = TranscriptLedger::new(&session_id);
            for event in transcript_events
                .iter()
                .take_while(|event| event.received_at_ms <= patch.created_at_ms)
            {
                if let Err(error) = evidence_ledger.apply_event(event.clone()) {
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

            let mut evidence_speaker_timeline =
                speaker_history_present.then(|| SpeakerTimeline::new(&session_id));
            if let Some(timeline) = evidence_speaker_timeline.as_mut() {
                for event in speaker_events
                    .iter()
                    .take_while(|event| event.received_at_ms <= patch.created_at_ms)
                {
                    if let Err(error) = timeline.apply_event(event.clone()) {
                        validation.invalid_patch_count += 1;
                        validation.errors.push(
                            HistoricalProjectionValidationError::SpeakerReplay {
                                sequence: patch.sequence,
                                error,
                            },
                        );
                        continue 'patches;
                    }
                }
            }

            // The live gate (`apply_validated_patch_with_speaker_timeline_opt`)
            // classifies basis currency against a snapshot taken at actual
            // apply time (`state.rs`'s `transcript_ledger_snapshot`), not at
            // `patch.created_at_ms` — LLM generation can take seconds, during
            // which a boundary-correcting revision can legitimately move a
            // span from inside the covered prefix (as of `created_at_ms`) to
            // a proven append-only tail (as of apply time). Reclassifying
            // here against only the `created_at_ms`-bounded
            // `evidence_ledger` above would derive `Revised` for a patch the
            // live gate correctly accepted as `AppendOnlyStale`, and
            // `commands.rs`'s `invalid_patch_count`-gated refusal would make
            // the session unopenable (audio-graph-f3d4 review finding).
            // `generation_latency_ms` is stamped from the same wall-clock
            // source as `created_at_ms` (`speech/mod.rs`'s
            // `run_projection_job`) and brackets the live apply's fresh
            // snapshot almost exactly — the only remaining gap is the
            // sub-millisecond scheduling window between generation finishing
            // and that snapshot, not the multi-second LLM call this guards.
            // Extend a *separate* ledger/timeline to that bound instead of
            // reusing `evidence_ledger` so `resolve_claim_evidence_basis_events`
            // below keeps reconstructing exactly what the LLM saw at draft
            // time (unaffected by revisions that arrived after generation
            // started). `None` degrades to the unextended `created_at_ms`
            // bound for patches persisted before this field existed.
            let classify_bound_ms = patch
                .created_at_ms
                .saturating_add(patch.generation_latency_ms.unwrap_or(0));
            let (ledger, speaker_timeline) = replay_ledger_and_timeline_up_to(
                &session_id,
                &transcript_events,
                &speaker_events,
                speaker_history_present,
                classify_bound_ms,
            );

            match ledger.classify_basis_currency(&patch.basis, speaker_timeline.as_ref()) {
                BasisCurrency::Current | BasisCurrency::AppendOnlyStale(_) => {}
                BasisCurrency::Revised(staleness) => {
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
            }

            // The classification just above proves every span this patch's
            // basis pinned still resolves at the pinned revision as of
            // `classify_bound_ms` — true for both `Current` and a proven
            // append-only tail. Only a `Revised` span breaks that proof, and
            // it is rejected above before reaching here. Because revisions
            // are monotonic (a span's revision only ever moves forward),
            // that same proof holds transitively at the earlier
            // `created_at_ms` bound: no basis-pinned span could have been
            // revised between `created_at_ms` and `classify_bound_ms`
            // without also tripping the check above. So re-deriving claim
            // evidence against the `created_at_ms`-bounded
            // `evidence_ledger`/`evidence_speaker_timeline` HERE reproduces
            // what `judge_claim_evidence` saw at draft-admission time,
            // instead of the `None` this path used to hardcode — and stays
            // pinned to draft-time visibility rather than picking up
            // revisions the LLM never saw.
            let evidence_events = resolve_claim_evidence_basis_events(
                &patch.basis,
                &evidence_ledger,
                evidence_speaker_timeline.as_ref(),
            );
            let evidence_basis: BTreeMap<&str, &TranscriptEvent> = evidence_events
                .iter()
                .map(|event| (event.span_id.as_str(), event))
                .collect();

            state.apply_replayed_patch(&patch, Some(&evidence_basis))?;
        }

        Ok(HistoricalProjectionReplay { state, validation })
    }

    pub fn apply_validated_patch(
        &mut self,
        ledger: &TranscriptLedger,
        patch: &ProjectionPatch,
    ) -> Result<MaterializedProjectionApplyOutcome, ProjectionApplyError> {
        self.apply_validated_patch_with_speaker_timeline_opt(ledger, None, patch)
            .map(|(outcome, _currency)| outcome)
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
            .map(|(outcome, _currency)| outcome)
    }

    /// Like [`Self::apply_validated_patch`] but also reports the
    /// [`AppliedBasisCurrency`] the gate classified the patch's basis as, so
    /// the one live runtime caller (`state.rs`) can split its
    /// applied-append-only telemetry from the ordinary current-basis path
    /// without re-deriving [`BasisCurrency`] itself (audio-graph-caad).
    pub fn apply_validated_patch_reporting_currency(
        &mut self,
        ledger: &TranscriptLedger,
        patch: &ProjectionPatch,
    ) -> Result<(MaterializedProjectionApplyOutcome, AppliedBasisCurrency), ProjectionApplyError>
    {
        self.apply_validated_patch_with_speaker_timeline_opt(ledger, None, patch)
    }

    fn apply_validated_patch_with_speaker_timeline_opt(
        &mut self,
        ledger: &TranscriptLedger,
        speaker_timeline: Option<&SpeakerTimeline>,
        patch: &ProjectionPatch,
    ) -> Result<(MaterializedProjectionApplyOutcome, AppliedBasisCurrency), ProjectionApplyError>
    {
        let currency = match ledger.classify_basis_currency(&patch.basis, speaker_timeline) {
            BasisCurrency::Current => AppliedBasisCurrency::Current,
            BasisCurrency::AppendOnlyStale(staleness) => {
                AppliedBasisCurrency::AppendedTail { staleness }
            }
            BasisCurrency::Revised(staleness) => {
                return Err(ProjectionApplyError::StaleBasis { staleness });
            }
        };

        // The classification just above proves `ledger` still holds every
        // span this patch's basis pinned, at the pinned revision, whether
        // the basis is `Current` or a proven append-only tail
        // (`AppendedTail`) — only a `Revised` span breaks that proof, and it
        // is rejected above before reaching here. So resolving claim
        // evidence against `ledger` HERE reproduces the same basis-covered
        // set `judge_claim_evidence` saw at draft-admission time
        // (ADR-0037/ADR-0031: never launder a `Revised` span into an
        // evidence proof).
        let evidence_events =
            resolve_claim_evidence_basis_events(&patch.basis, ledger, speaker_timeline);
        let evidence_basis: BTreeMap<&str, &TranscriptEvent> = evidence_events
            .iter()
            .map(|event| (event.span_id.as_str(), event))
            .collect();

        let outcome = match patch.kind {
            ProjectionKind::Notes => {
                self.notes.apply_patch(patch, Some(&evidence_basis))?;
                MaterializedProjectionApplyOutcome::Notes {
                    last_sequence: self.notes.last_sequence,
                    note_count: self.notes.notes.len(),
                }
            }
            ProjectionKind::Graph => {
                self.graph.apply_patch(patch, Some(&evidence_basis))?;
                MaterializedProjectionApplyOutcome::Graph {
                    last_sequence: self.graph.last_sequence,
                    node_count: self.graph.nodes.len(),
                    edge_count: self.graph.edges.len(),
                }
            }
        };
        Ok((outcome, currency))
    }
}

/// Resolve the basis-covered `TranscriptEvent`s for `patch_basis` against
/// `ledger`, for claim-evidence re-judging at apply time (ADR-0037). Mirrors
/// `projection_llm::basis_events`'s `(span_id, revision_number)` lookup
/// exactly, but is local to this module so `MaterializedNotes`/
/// `MaterializedGraph` can call it without a dependency on `projection_llm`
/// (which itself depends on `projections`). Spans the ledger no longer holds
/// at the pinned revision are simply absent from the returned set — callers
/// that need "this basis is still valid" as a hard precondition (e.g.
/// [`TranscriptLedger::validate_basis_with_speaker_timeline`]) check that
/// separately, before calling this.
///
/// Returns OWNED events, not borrows into `ledger`, because `speaker_timeline`
/// (when supplied) overrides each resolved event's `speaker_id`/`speaker_label`
/// with the canonical [`SpeakerTimeline`] latest-wins attribution — the same
/// join `timeline::build_session_timeline` uses — before the caller ever hands
/// the event to [`crate::claim_evidence::judge_claim_evidence`]. Without this,
/// `ResolvedSpanEvidence::speaker_ref` (claim_evidence.rs) would read straight
/// off the untrusted inline ASR label the diarization override exists
/// precisely to supersede (ADR-0026 §3/§4). `None` means no diarization
/// history is available for this call (e.g. a repository that does not
/// support durable diarization storage, or a replay with no speaker-revision
/// log); every event then keeps its inline speaker fields unchanged, exactly
/// as before this override existed.
pub(crate) fn resolve_claim_evidence_basis_events(
    patch_basis: &ProjectionBasis,
    ledger: &TranscriptLedger,
    speaker_timeline: Option<&SpeakerTimeline>,
) -> Vec<TranscriptEvent> {
    let latest_by_span: BTreeMap<(&str, u64), &TranscriptEvent> = ledger
        .latest_spans
        .iter()
        .map(|event| ((event.span_id.as_str(), event.revision_number), event))
        .collect();
    let attribution = speaker_timeline.map(crate::timeline::speaker_attribution_index);

    patch_basis
        .span_revisions
        .iter()
        .filter_map(|span| {
            latest_by_span
                .get(&(span.span_id.as_str(), span.revision_number))
                .map(|event| {
                    let mut event = (*event).clone();
                    if let Some(attribution) = attribution.as_ref() {
                        let keys = crate::timeline::candidate_keys(
                            event.transcript_segment_id.as_deref(),
                            event.span_id.as_str(),
                        );
                        if let Some(winner) =
                            keys.iter().find_map(|key| attribution.get(key.as_str()))
                        {
                            event.speaker_id = winner.speaker_id.clone();
                            event.speaker_label = winner.speaker_label.clone();
                        }
                    }
                    event
                })
        })
        .collect()
}

/// Re-judge one operation's untrusted [`EvidenceAnchor`](crate::claim_evidence::EvidenceAnchor)
/// at apply time, for the `Materialized*.evidence` field.
///
/// `basis` is `None` when the caller has no ledger snapshot for this patch's
/// exact basis (replay); the anchor was already judged once, correctly,
/// against the SAME basis at draft-admission time
/// (`projection_llm::trusted_projection_patch_from_model_json`), so a
/// `Refused` judgement here — which should be unreachable on the live path,
/// since `apply_validated_patch_with_speaker_timeline_opt` just proved this
/// basis still resolves — is logged and treated as `None` rather than
/// failing the whole apply. Materializing the note/node/edge itself is not
/// contingent on its evidence badge.
fn resolve_admitted_claim_evidence(
    anchor: &crate::claim_evidence::EvidenceAnchor,
    basis: Option<&BTreeMap<&str, &TranscriptEvent>>,
) -> Option<crate::claim_evidence::AdmittedClaimEvidence> {
    let basis = basis?;
    match crate::claim_evidence::judge_claim_evidence(anchor, basis) {
        crate::claim_evidence::ClaimAdmission::Admitted(evidence) => Some(evidence),
        // `CoverageMarkerUnavailable` is `KnowledgeGap`'s UNCONDITIONAL,
        // by-design refusal (ADR-0037) — locally-generated operations that
        // never went through `validate_projection_patch_draft` at all (e.g.
        // `commands::approved_agent_projection_patch`) deliberately anchor
        // with `EvidenceAnchor::default()`, which is this class, so refusing
        // here is expected on every application, not an anomaly worth a log
        // line. Every OTHER deficiency reaching this point is unexpected: the
        // operation's anchor was already judged `Admitted` once, against the
        // same basis, at validation time.
        crate::claim_evidence::ClaimAdmission::Refused(deficiency) => {
            if !matches!(
                deficiency,
                crate::claim_evidence::ClaimEvidenceDeficiency::CoverageMarkerUnavailable
            ) {
                log::warn!(
                    "claim evidence anchor refused at apply time after having been admitted at \
                     validation time (basis drift?): {deficiency}"
                );
            }
            None
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

pub(crate) fn millis(value: f64) -> i64 {
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

/// Deterministic per-event content fingerprint for claim-evidence resolution
/// (ADR-0037): the same FNV-1a64 family and `"fnv1a64:…"` format as
/// [`transcript_events_hash_v1`], scoped to ONE resolved span instead of a
/// whole ordered ledger. `claim_evidence::judge_claim_evidence` needs a
/// content hash for the single event a `span_id` resolved to, not a basis-wide
/// hash, so this is a distinct (smaller) hashed byte sequence, not a reuse of
/// the ledger-wide hash's exact bytes.
pub(crate) fn transcript_event_content_hash(event: &TranscriptEvent) -> String {
    let mut hash = 14_695_981_039_346_656_037u64;
    update_hash(&mut hash, &event.span_id);
    update_hash(&mut hash, &event.text);
    update_hash(&mut hash, &event.revision_number.to_string());
    format!("fnv1a64:{hash:016x}")
}

/// Frozen v1 deterministic FNV-1a hash over canonical transcript revision fields.
///
/// Fidelity metadata is intentionally absent. A future change to the hashed
/// byte sequence requires a new [`TranscriptHashVersion`].
pub fn transcript_events_hash_v1(events: &[TranscriptEvent]) -> String {
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

/// Compatibility name for callers written before transcript hash versions
/// became explicit. This is byte-for-byte [`transcript_events_hash_v1`].
pub fn transcript_events_hash(events: &[TranscriptEvent]) -> String {
    transcript_events_hash_v1(events)
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

    /// `transcript_event_content_hash` (ADR-0037's claim-evidence content
    /// fingerprint) is deterministic per (span_id, text, revision_number) and
    /// changes when any of those three change — the exact sensitivity
    /// `judge_claim_evidence`'s `ResolvedSpanEvidence.content_hash` needs.
    #[test]
    fn transcript_event_content_hash_is_deterministic_and_content_sensitive() {
        let event_a = TranscriptEvent::from(asr_payload("span-1", 2, "hello world"));
        let event_a_again = TranscriptEvent::from(asr_payload("span-1", 2, "hello world"));
        let event_different_text = TranscriptEvent::from(asr_payload("span-1", 2, "goodbye"));
        let event_different_revision =
            TranscriptEvent::from(asr_payload("span-1", 3, "hello world"));

        let hash_a = transcript_event_content_hash(&event_a);
        assert_eq!(hash_a, transcript_event_content_hash(&event_a_again));
        assert!(hash_a.starts_with("fnv1a64:"));
        assert_ne!(hash_a, transcript_event_content_hash(&event_different_text));
        assert_ne!(
            hash_a,
            transcript_event_content_hash(&event_different_revision)
        );
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
    fn projection_basis_defaults_and_serializes_explicit_transcript_hash_v1() {
        let event = TranscriptEvent::from(asr_payload("span-1", 1, "hash fixture"));
        let current = ProjectionBasis::from_transcript_events(std::slice::from_ref(&event));
        assert_eq!(current.hash_version(), TranscriptHashVersion::V1);
        let current_json = serde_json::to_value(&current).expect("serialize current basis");
        assert_eq!(current_json["hash_version"], "v1");

        let mut legacy_json = current_json;
        legacy_json
            .as_object_mut()
            .expect("basis object")
            .remove("hash_version");
        let legacy: ProjectionBasis =
            serde_json::from_value(legacy_json).expect("decode pre-version basis");
        assert_eq!(legacy.hash_version(), TranscriptHashVersion::V1);
        assert_eq!(legacy.transcript_hash, current.transcript_hash);
        assert_eq!(
            transcript_events_hash_v1(std::slice::from_ref(&event)),
            transcript_events_hash(std::slice::from_ref(&event)),
            "the legacy function remains an exact v1 compatibility alias"
        );
        assert_eq!(
            transcript_events_hash_v1(std::slice::from_ref(&event)),
            "fnv1a64:4eb27818db1f8b3d",
            "the accepted v1 field bytes are a frozen replay golden"
        );
    }

    #[test]
    fn classify_basis_currency_distinguishes_current_append_only_and_revised() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(TranscriptEvent::from(asr_payload("span-1", 1, "hello")))
            .expect("seed first span");
        let basis = ledger.current_basis();

        assert_eq!(
            ledger.classify_basis_currency(&basis, None),
            BasisCurrency::Current
        );
        assert_eq!(ledger.validate_basis(&basis), Ok(()));

        ledger
            .apply_event(TranscriptEvent::from(asr_payload("span-2", 1, "world")))
            .expect("append second span");
        assert_eq!(
            ledger.classify_basis_currency(&basis, None),
            BasisCurrency::AppendOnlyStale(ProjectionBasisStaleness::MissingCurrentSpan {
                span_id: "span-2".to_string(),
                current_revision: 1,
            })
        );
        assert_eq!(
            ledger.validate_basis(&basis),
            Err(ProjectionBasisStaleness::MissingCurrentSpan {
                span_id: "span-2".to_string(),
                current_revision: 1,
            }),
            "the legacy two-way API must delegate to the same classifier"
        );

        ledger
            .apply_event(TranscriptEvent::from(asr_payload(
                "span-1",
                2,
                "hello, corrected",
            )))
            .expect("revise covered span");
        let revised = ProjectionBasisStaleness::StaleSpanRevision {
            span_id: "span-1".to_string(),
            current_revision: 2,
            basis_revision: 1,
        };
        assert_eq!(
            ledger.classify_basis_currency(&basis, None),
            BasisCurrency::Revised(revised.clone())
        );
        assert_eq!(ledger.validate_basis(&basis), Err(revised));
    }

    #[test]
    fn same_span_revisions_with_wrong_hash_are_revised_and_match_validation() {
        let mut ledger = TranscriptLedger::replay(
            "session-1",
            [TranscriptEvent::from(asr_payload(
                "span-1",
                1,
                "basis text",
            ))],
        )
        .expect("ledger replay");
        let mut wrong_basis = ledger.current_basis();
        let covered_hash = wrong_basis.transcript_hash.clone();
        wrong_basis.transcript_hash = "fnv1a64:0000000000000000".to_string();
        ledger
            .apply_event(TranscriptEvent::from(asr_payload(
                "span-2",
                1,
                "later append",
            )))
            .expect("append after corrupt basis was captured");
        let expected = ProjectionBasisStaleness::TranscriptHashMismatch {
            current_hash: covered_hash,
            basis_hash: wrong_basis.transcript_hash.clone(),
        };

        assert_eq!(
            ledger.classify_basis_currency(&wrong_basis, None),
            BasisCurrency::Revised(expected.clone())
        );
        assert_eq!(ledger.validate_basis(&wrong_basis), Err(expected));
    }

    #[test]
    fn basis_currency_rejects_deletion_reorder_and_non_tail_insertion() {
        let mut first = TranscriptEvent::from(asr_payload("span-1", 1, "first"));
        first.start_time = 2.0;
        first.end_time = 3.0;
        let mut second = TranscriptEvent::from(asr_payload("span-2", 1, "second"));
        second.start_time = 3.0;
        second.end_time = 4.0;

        let full_ledger = TranscriptLedger::replay("session-1", [first.clone(), second.clone()])
            .expect("full ledger");
        let full_basis = full_ledger.current_basis();

        let deleted_ledger = TranscriptLedger::replay("session-1", [first.clone()])
            .expect("ledger missing one covered span");
        let deleted = ProjectionBasisStaleness::UnknownBasisSpan {
            span_id: "span-2".to_string(),
            basis_revision: 1,
        };
        assert_eq!(
            deleted_ledger.classify_basis_currency(&full_basis, None),
            BasisCurrency::Revised(deleted.clone())
        );
        assert_eq!(deleted_ledger.validate_basis(&full_basis), Err(deleted));

        let mut reordered_basis = full_basis.clone();
        reordered_basis.span_revisions.swap(0, 1);
        let reordered = ProjectionBasisStaleness::CoveredSpanOrderMismatch {
            index: 0,
            current_span_id: "span-1".to_string(),
            basis_span_id: "span-2".to_string(),
        };
        assert_eq!(
            full_ledger.classify_basis_currency(&reordered_basis, None),
            BasisCurrency::Revised(reordered.clone())
        );
        assert_eq!(full_ledger.validate_basis(&reordered_basis), Err(reordered));

        let prefix_basis = TranscriptLedger::replay("session-1", [first.clone()])
            .expect("prefix ledger")
            .current_basis();
        let mut inserted = TranscriptEvent::from(asr_payload("span-z", 1, "inserted earlier"));
        inserted.start_time = 1.0;
        inserted.end_time = 1.5;
        let inserted_ledger = TranscriptLedger::replay("session-1", [first, inserted])
            .expect("ledger with non-tail insertion");
        let non_tail = ProjectionBasisStaleness::MissingCurrentSpan {
            span_id: "span-z".to_string(),
            current_revision: 1,
        };
        assert_eq!(
            inserted_ledger.classify_basis_currency(&prefix_basis, None),
            BasisCurrency::Revised(non_tail.clone())
        );
        assert_eq!(inserted_ledger.validate_basis(&prefix_basis), Err(non_tail));
    }

    #[test]
    fn append_only_uses_audio_chronology_not_span_id_sort_order() {
        let mut first = TranscriptEvent::from(asr_payload("span-z", 1, "first"));
        first.start_time = 1.0;
        first.end_time = 2.0;
        let mut ledger = TranscriptLedger::replay("session-1", [first]).expect("prefix ledger");
        let prefix_basis = ledger.current_basis();

        let mut appended = TranscriptEvent::from(asr_payload("span-a", 1, "later"));
        appended.start_time = 3.0;
        appended.end_time = 4.0;
        ledger.apply_event(appended).expect("tail append");

        let append_only = ProjectionBasisStaleness::MissingCurrentSpan {
            span_id: "span-a".to_string(),
            current_revision: 1,
        };
        assert_eq!(
            ledger.classify_basis_currency(&prefix_basis, None),
            BasisCurrency::AppendOnlyStale(append_only.clone())
        );
        assert_eq!(ledger.validate_basis(&prefix_basis), Err(append_only));
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
    fn derive_legacy_segments_collapses_superseding_partials_to_one_per_final_span() {
        // Two stable spans, each with a partial (rev 1) then a final (rev 2)
        // revision. The ledger collapses them to the latest accepted revision,
        // so the derived legacy view must yield exactly one segment per span.
        let events = [
            provider_payload(
                "openai_realtime",
                "system",
                "openai_realtime:system:item-1",
                Some("item-1"),
                1,
                "partial one",
                false,
            ),
            provider_payload(
                "openai_realtime",
                "system",
                "openai_realtime:system:item-1",
                Some("item-1"),
                2,
                "final one",
                true,
            ),
            provider_payload(
                "deepgram",
                "system",
                "deepgram:system:start-2000",
                None,
                1,
                "partial two",
                false,
            ),
            provider_payload(
                "deepgram",
                "system",
                "deepgram:system:start-2000",
                None,
                2,
                "final two",
                true,
            ),
        ]
        .into_iter()
        .map(TranscriptEvent::from);

        let ledger = TranscriptLedger::replay("session-derive", events).expect("ledger replay");
        let segments = derive_legacy_transcript_segments(&ledger);

        assert_eq!(
            segments.len(),
            2,
            "superseding partials must collapse to one segment per final span"
        );
        // Both final revisions share start_time == 2.0, so the canonical view
        // orders them by span_id (`deepgram:...` < `openai_realtime:...`).
        assert_eq!(
            segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>(),
            vec!["final two", "final one"],
            "derived view must reflect the latest accepted (final) revisions"
        );
        // The provider-final fixtures expose a `transcript_segment_id`, which
        // becomes the stable legacy segment id; the view is duplicate-free.
        let ids: Vec<&str> = segments.iter().map(|s| s.id.as_str()).collect();
        let unique: std::collections::BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(
            ids.len(),
            unique.len(),
            "derived segment ids must be unique"
        );
        assert_eq!(
            ids,
            vec![
                "deepgram:system:start-2000-segment",
                "openai_realtime:system:item-1-segment",
            ]
        );
    }

    #[test]
    fn derive_legacy_segments_falls_back_to_span_id_without_segment_id() {
        // A partial-only span has no transcript_segment_id; the derived view
        // falls back to the immutable span_id so it stays deterministic.
        let event = TranscriptEvent::from(provider_payload(
            "deepgram",
            "system",
            "deepgram:system:start-1000",
            None,
            1,
            "interim hypothesis",
            false,
        ));
        let ledger = TranscriptLedger::replay("session-fallback", [event]).expect("ledger replay");

        let segments = derive_legacy_transcript_segments(&ledger);
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].id, "deepgram:system:start-1000");
        assert_eq!(segments[0].text, "interim hypothesis");
        assert_eq!(segments[0].source_id, "system");
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
            transcript_hash: ProjectionBasis::from_transcript_events(&[]).transcript_hash,
            summarized_through_revision: None,
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

    /// Legacy-basis replay compatibility (Codex P1 on PR #77): patches
    /// persisted before `summarized_through_revision` existed deserialize to
    /// `None`. Once a historical transcript exceeds the hot window the
    /// recomputed current basis is `Some(..)` — a legacy patch with matching
    /// spans + hash must still validate. Only Some-vs-Some disagreement fails.
    #[test]
    fn legacy_basis_without_summary_boundary_validates_against_windowed_ledger() {
        // Long session: more turns than the hot window, so the current basis
        // carries Some(summarized_through_revision).
        let events: Vec<TranscriptEvent> = (0..(ROLLING_SUMMARY_HOT_WINDOW_TURNS + 3))
            .map(|i| {
                let mut event =
                    TranscriptEvent::from(asr_payload(&format!("span-{i}"), 1, "legacy turn"));
                event.start_time = i as f64;
                event.end_time = i as f64 + 0.5;
                event
            })
            .collect();
        let ledger = TranscriptLedger::replay("session-1", events).expect("ledger replay");
        let current_basis = ledger.current_basis();
        assert!(
            current_basis.summarized_through_revision.is_some(),
            "long transcript must produce a summary boundary"
        );

        // A pre-18ee persisted patch: same spans + hash, but the field is
        // absent on disk → deserializes to None.
        let legacy_json = serde_json::json!({
            "span_revisions": current_basis.span_revisions,
            "diarization_span_revisions": [],
            "transcript_hash": current_basis.transcript_hash,
        });
        let legacy_basis: ProjectionBasis =
            serde_json::from_value(legacy_json).expect("legacy basis deserializes");
        assert_eq!(legacy_basis.summarized_through_revision, None);

        // Replay/reconstruction must accept the legacy patch.
        assert_eq!(ledger.validate_basis(&legacy_basis), Ok(()));

        // An explicit Some-vs-Some disagreement is still stale.
        let mut mismatched = current_basis.clone();
        mismatched.summarized_through_revision = current_basis
            .summarized_through_revision
            .map(|revision| revision + 7);
        assert_eq!(
            ledger.validate_basis(&mismatched),
            Err(ProjectionBasisStaleness::SummaryWindowMismatch {
                current_summarized_through: current_basis.summarized_through_revision,
                basis_summarized_through: mismatched.summarized_through_revision,
            })
        );
    }

    #[test]
    fn projection_patch_serializes_replayable_operations() {
        let event = TranscriptEvent::from(asr_payload("span-1", 2, "decision made"));
        let basis = ProjectionBasis::from_transcript_events(&[event]);
        let patch = ProjectionPatch {
            route: None,
            sequence: 7,
            kind: ProjectionKind::Notes,
            llm_request_id: "llm-req-1".to_string(),
            basis,
            operations: vec![ProjectionOperation::UpsertNote {
                id: "note-1".to_string(),
                title: "Decision".to_string(),
                body: "Ship the event-sourced projection model.".to_string(),
                tags: vec!["decision".to_string()],
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
            confidence: 0.86,
            provenance: ProjectionProvenance {
                provider: "openrouter".to_string(),
                model: "anthropic/claude-sonnet-4".to_string(),
                prompt_id: "notes-v1".to_string(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
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
            route: None,
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
                summarized_through_revision: None,
            },
            operations: vec![
                ProjectionOperation::UpsertNote {
                    id: "note-1".to_string(),
                    title: "Decision".to_string(),
                    body: "SENSITIVE NOTE BODY".to_string(),
                    tags: vec!["decision".to_string()],
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "node-1".to_string(),
                    name: "SECRET NAME".to_string(),
                    entity_type: "SECRET TYPE".to_string(),
                    description: Some("SECRET DESCRIPTION".to_string()),
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge-1".to_string(),
                    source: "node-1".to_string(),
                    target: "node-2".to_string(),
                    relation_type: "SECRET RELATION".to_string(),
                    label: Some("SECRET LABEL".to_string()),
                    weight: 0.9,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
            ],
            confidence: 0.8,
            provenance: ProjectionProvenance {
                provider: "openrouter".to_string(),
                model: "gpt-4.1".to_string(),
                prompt_id: "graph-v1".to_string(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
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
            .apply_patch(
                &notes_patch(1, "note-1", "Decision", "SENSITIVE NOTE BODY"),
                None,
            )
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
            .apply_patch(
                &graph_patch(
                    1,
                    vec![
                        ProjectionOperation::UpsertGraphNode {
                            id: "node-a".to_string(),
                            name: "Node A".to_string(),
                            entity_type: "PERSON".to_string(),
                            description: Some("SENSITIVE DESCRIPTION".to_string()),
                            evidence: crate::claim_evidence::EvidenceAnchor::default(),
                        },
                        ProjectionOperation::UpsertGraphNode {
                            id: "node-b".to_string(),
                            name: "Node B".to_string(),
                            entity_type: "TOPIC".to_string(),
                            description: None,
                            evidence: crate::claim_evidence::EvidenceAnchor::default(),
                        },
                        ProjectionOperation::UpsertGraphEdge {
                            id: "edge-ab".to_string(),
                            source: "node-a".to_string(),
                            target: "node-b".to_string(),
                            relation_type: "SENSITIVE RELATION".to_string(),
                            label: Some("SENSITIVE LABEL".to_string()),
                            weight: 0.4,
                            evidence: crate::claim_evidence::EvidenceAnchor::default(),
                        },
                    ],
                ),
                None,
            )
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
            .apply_replayed_patch(
                &notes_patch(1, "note-1", "Decision", "SENSITIVE NOTE IN STATE"),
                None,
            )
            .expect("apply state note patch");
        state
            .apply_replayed_patch(
                &graph_patch(
                    2,
                    vec![
                        ProjectionOperation::UpsertGraphNode {
                            id: "node-a".to_string(),
                            name: "Node A".to_string(),
                            entity_type: "PERSON".to_string(),
                            description: Some("SENSITIVE DESC".to_string()),
                            evidence: crate::claim_evidence::EvidenceAnchor::default(),
                        },
                        ProjectionOperation::UpsertGraphNode {
                            id: "node-b".to_string(),
                            name: "Node B".to_string(),
                            entity_type: "TOPIC".to_string(),
                            description: None,
                            evidence: crate::claim_evidence::EvidenceAnchor::default(),
                        },
                        ProjectionOperation::UpsertGraphEdge {
                            id: "edge-ab".to_string(),
                            source: "node-a".to_string(),
                            target: "node-b".to_string(),
                            relation_type: "SENSITIVE RELATION".to_string(),
                            label: Some("SENSITIVE LABEL".to_string()),
                            weight: 0.4,
                            evidence: crate::claim_evidence::EvidenceAnchor::default(),
                        },
                    ],
                ),
                None,
            )
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
            route: None,
            sequence,
            kind: ProjectionKind::Notes,
            llm_request_id: format!("llm-req-{sequence}"),
            basis: ProjectionBasis::from_transcript_events(basis_events),
            operations: vec![ProjectionOperation::UpsertNote {
                id: id.to_string(),
                title: title.to_string(),
                body: body.to_string(),
                tags: vec!["decision".to_string()],
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
            confidence: 0.86,
            provenance: ProjectionProvenance {
                provider: "openrouter".to_string(),
                model: "anthropic/claude-sonnet-4".to_string(),
                prompt_id: "notes-v1".to_string(),
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
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
            route: None,
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
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
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
        notes.apply_patch(&first, None).expect("insert patch");

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
        notes.apply_patch(&update, None).expect("update patch");

        assert_eq!(notes.last_sequence, 2);
        assert_eq!(notes.notes.len(), 1);
        assert_eq!(notes.notes[0].body, "Ship materialized notes.");
        assert_eq!(notes.notes[0].updated_by_sequence, 2);

        let second = notes_patch(3, "note-2", "Follow-up", "Keep stable note ids.");
        notes.apply_patch(&second, None).expect("second note patch");
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
            route: None,
        };
        notes.apply_patch(&reorder, None).expect("reorder patch");
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
            route: None,
        };
        notes.apply_patch(&delete, None).expect("delete patch");

        assert_eq!(notes.last_sequence, 5);
        assert_eq!(notes.notes.len(), 1);
        assert_eq!(notes.notes[0].id, "note-2");
    }

    #[test]
    fn materialized_notes_reject_stale_or_wrong_kind_patches() {
        let mut notes = MaterializedNotes::new("session-1");
        notes
            .apply_patch(&notes_patch(2, "note-1", "Decision", "Ship notes."), None)
            .expect("first patch");

        let stale = notes_patch(2, "note-2", "Duplicate", "Should be rejected.");
        assert_eq!(
            notes.apply_patch(&stale, None),
            Err(ProjectionApplyError::StaleSequence {
                current: 2,
                incoming: 2,
            })
        );

        let mut graph_patch = notes_patch(3, "note-3", "Graph", "Wrong kind.");
        graph_patch.kind = ProjectionKind::Graph;
        assert_eq!(
            notes.apply_patch(&graph_patch, None),
            Err(ProjectionApplyError::WrongKind {
                expected: ProjectionKind::Notes,
                actual: ProjectionKind::Graph,
            })
        );

        let missing_reorder = ProjectionPatch {
            route: None,
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
                route_id: None,
                model_source: crate::llm::route::ModelIdentitySource::Requested,
            },
            queued_at_ms: None,
            generation_latency_ms: None,
            apply_latency_ms: None,
            created_at_ms: 1_700_000_000_103,
        };
        assert_eq!(
            notes.apply_patch(&missing_reorder, None),
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
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "node-b".to_string(),
                    name: "Soniox".to_string(),
                    entity_type: "Provider".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge-1".to_string(),
                    source: "node-a".to_string(),
                    target: "node-b".to_string(),
                    relation_type: "evaluates".to_string(),
                    label: Some("evaluates as streaming STT".to_string()),
                    weight: 0.7,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
            ],
        );
        graph.apply_patch(&first, None).expect("graph insert patch");

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
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge-1".to_string(),
                    source: "node-a".to_string(),
                    target: "node-b".to_string(),
                    relation_type: "shortlists".to_string(),
                    label: Some("shortlisted provider".to_string()),
                    weight: 0.9,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
            ],
        );
        graph
            .apply_patch(&update, None)
            .expect("graph update patch");

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
        graph
            .apply_patch(&remove_edge, None)
            .expect("remove edge patch");
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
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        graph
            .apply_patch(&restore, None)
            .expect("restore edge patch");
        assert_eq!(graph.edges.len(), 1);

        let remove_node = graph_patch(
            5,
            vec![ProjectionOperation::RemoveGraphNode {
                id: "node-b".to_string(),
            }],
        );
        graph
            .apply_patch(&remove_node, None)
            .expect("remove node patch");
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
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "person:alicia".to_string(),
                    name: "Alicia".to_string(),
                    entity_type: "person".to_string(),
                    description: Some("Duplicate mention of Alice.".to_string()),
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "project:audio-graph".to_string(),
                    name: "AudioGraph".to_string(),
                    entity_type: "project".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "topic:provider-work".to_string(),
                    name: "Provider work".to_string(),
                    entity_type: "topic".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge:alicia:owns".to_string(),
                    source: "person:alicia".to_string(),
                    target: "project:audio-graph".to_string(),
                    relation_type: "owns".to_string(),
                    label: Some("owns".to_string()),
                    weight: 0.4,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge:alice:owns".to_string(),
                    source: "person:alice".to_string(),
                    target: "project:audio-graph".to_string(),
                    relation_type: "owns".to_string(),
                    label: Some("owns".to_string()),
                    weight: 0.8,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge:project:topic".to_string(),
                    source: "project:audio-graph".to_string(),
                    target: "topic:provider-work".to_string(),
                    relation_type: "tracks".to_string(),
                    label: None,
                    weight: 0.5,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
            ],
        );
        graph.apply_patch(&first, None).expect("seed graph");

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
        graph
            .apply_patch(&weights, None)
            .expect("adjust edge weights");
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
        graph
            .apply_patch(&merge, None)
            .expect("merge duplicate nodes");
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
        graph.apply_patch(&split, None).expect("split topic node");
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
            .apply_patch(&invalidate_edge, None)
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
            .apply_patch(&invalidate_node, None)
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
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        let mut graph = MaterializedGraph::new("session-1");
        graph.apply_patch(&graph_patch, None).expect("graph patch");
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
            .apply_patch(
                &graph_patch(
                    2,
                    vec![ProjectionOperation::UpsertGraphNode {
                        id: "node-a".to_string(),
                        name: "AudioGraph".to_string(),
                        entity_type: "Product".to_string(),
                        description: None,
                        evidence: crate::claim_evidence::EvidenceAnchor::default(),
                    }],
                ),
                None,
            )
            .expect("first patch");

        let stale = graph_patch(
            2,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "node-b".to_string(),
                name: "Duplicate".to_string(),
                entity_type: "Topic".to_string(),
                description: None,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        assert_eq!(
            graph.apply_patch(&stale, None),
            Err(ProjectionApplyError::StaleSequence {
                current: 2,
                incoming: 2,
            })
        );

        let wrong_kind = notes_patch(3, "note-1", "Decision", "Wrong kind.");
        assert_eq!(
            graph.apply_patch(&wrong_kind, None),
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
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        assert_eq!(
            graph.apply_patch(&note_op, None),
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
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge-missing".to_string(),
                    source: "node-c".to_string(),
                    target: "node-missing".to_string(),
                    relation_type: "mentions".to_string(),
                    label: None,
                    weight: 0.5,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
            ],
        );
        assert_eq!(
            graph.apply_patch(&dangling, None),
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
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::MergeGraphNodes {
                    source_id: "node-missing".to_string(),
                    target_id: "node-a".to_string(),
                },
            ],
        );
        assert_eq!(
            graph.apply_patch(&missing_retcon, None),
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
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
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
        // A REVISION of the accepted patch's covered span, not a mere
        // append — an append-only tail now applies live too (audio-graph-caad
        // / audio-graph-f3d4), so this test needs a genuinely `Revised` basis
        // to keep demonstrating that the final ledger is too strict for an
        // older accepted patch while blind replay is not.
        let revised = TranscriptEvent::from(asr_payload("span-1", 2, "Ship the corrected notes."));
        let mut final_ledger = TranscriptLedger::new("session-1");
        final_ledger.apply_event(first).expect("first event");
        let accepted_patch = notes_patch(1, "note-1", "Decision", "Ship notes.");
        final_ledger.apply_event(revised).expect("revised event");

        let mut live_state = MaterializedProjectionState::new("session-1");
        assert!(
            matches!(
                live_state.apply_validated_patch(&final_ledger, &accepted_patch),
                Err(ProjectionApplyError::StaleBasis { .. })
            ),
            "the final ledger should still reject an accepted patch whose covered span was revised"
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
    fn materialized_projection_history_applies_patch_with_visible_speaker_basis() {
        let mut transcript = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        transcript.received_at_ms = 1_700_000_010_000;
        let mut speaker = DiarizationSpanRevision::from(diarization_payload(
            "speaker-span-1",
            "deepgram",
            1,
            "speaker-1",
            DiarizationSpanStability::Stable,
        ));
        speaker.received_at_ms = 1_700_000_010_050;
        let mut accepted_patch = notes_patch_for_basis(
            1,
            std::slice::from_ref(&transcript),
            "note-1",
            "Decision",
            "Ship notes.",
        );
        accepted_patch.basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &[ProjectionBasisSpan {
                span_id: speaker.span_id.clone(),
                revision_number: speaker.revision_number,
            }],
        );
        accepted_patch.created_at_ms = 1_700_000_010_100;

        let replayed = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-1",
            [transcript],
            Some(vec![speaker]),
            [accepted_patch.clone()],
        )
        .expect("historically validated speaker-aware replay");

        assert_eq!(replayed.validation.checked_patch_count, 1);
        assert_eq!(replayed.validation.invalid_patch_count, 0);
        assert_eq!(replayed.state.notes.last_sequence, accepted_patch.sequence);
        assert_eq!(replayed.state.notes.notes[0].id, "note-1");
    }

    /// The exact invariant the finding pinned as broken: replay must
    /// re-derive per-item claim evidence (ADR-0037) from the reconstructed
    /// per-patch ledger, not hardcode `None`. Live apply and replay of the
    /// SAME accepted patch, with a REAL (non-default) evidence anchor, must
    /// produce byte-identical `Some(..)` evidence — not just agree by both
    /// being `None`, which is all `historical_replay_matches_live_
    /// materialized_state` (state.rs) pinned before this fix, because its
    /// fixture used `EvidenceAnchor::default()` (the one class
    /// `judge_claim_evidence` always refuses).
    #[test]
    fn replay_rederives_claim_evidence_identically_to_the_live_apply_path() {
        let transcript = TranscriptEvent::from(asr_payload("span-1", 1, "Alice chose Soniox."));
        let mut patch = notes_patch_for_basis(
            1,
            std::slice::from_ref(&transcript),
            "note-1",
            "Provider choice",
            "Alice chose Soniox.",
        );
        patch.operations = vec![ProjectionOperation::UpsertNote {
            id: "note-1".to_string(),
            title: "Provider choice".to_string(),
            body: "Alice chose Soniox.".to_string(),
            tags: vec!["decision".to_string()],
            evidence: crate::claim_evidence::EvidenceAnchor {
                claim_class: crate::claim_evidence::ClaimClass::GroundedInference,
                span_id: Some("span-1".to_string()),
                quote: None,
                note: None,
            },
        }];

        // Live path.
        let mut ledger = TranscriptLedger::new("session-1");
        ledger.apply_event(transcript.clone()).unwrap();
        let mut live_state = MaterializedProjectionState::new("session-1");
        live_state
            .apply_validated_patch(&ledger, &patch)
            .expect("live apply");
        let live_evidence = live_state.notes.notes[0]
            .evidence
            .clone()
            .expect("live apply must admit GroundedInference evidence");

        // Replay path — same accepted patch, same transcript history.
        let replayed =
            MaterializedProjectionState::replay_accepted_patches_with_transcript_history(
                "session-1",
                [transcript],
                [patch],
            )
            .expect("replay");
        assert_eq!(replayed.validation.invalid_patch_count, 0);
        let replayed_evidence = replayed.state.notes.notes[0]
            .evidence
            .clone()
            .expect("replay must re-derive the SAME evidence as the live path, not None");

        assert_eq!(live_evidence, replayed_evidence);
        assert_eq!(
            live_evidence.claim_class(),
            crate::claim_evidence::ClaimClass::GroundedInference
        );
        assert_eq!(
            live_evidence.span().map(|span| span.span_id.as_str()),
            Some("span-1")
        );
    }

    /// ADR-0026 §3/§4: a diarization span's latest-wins attribution overrides
    /// the transcript event's untrusted inline ASR speaker label. Before
    /// `resolve_claim_evidence_basis_events` existed, `judge_claim_evidence`
    /// stamped `speaker_ref` straight from `event.speaker_id`/`speaker_label`
    /// — the label diarization exists to supersede — even when the session's
    /// canonical `SpeakerTimeline` disagreed.
    #[test]
    fn apply_validated_patch_stamps_the_canonical_speaker_not_the_inline_asr_label() {
        // Inline ASR (untrusted) says "speaker-1"; the transcript segment id
        // is the fixed "segment-1" `asr_payload` always sets.
        let transcript = TranscriptEvent::from(asr_payload("span-1", 1, "Ana owns the migration."));
        assert_eq!(transcript.speaker_id.as_deref(), Some("speaker-1"));

        // Diarization attributes that SAME segment id to "speaker-2" — the
        // canonical, backend-derived override.
        let mut diarization_span = DiarizationSpanRevision::from(diarization_payload(
            "dia-1",
            "deepgram",
            1,
            "speaker-2",
            DiarizationSpanStability::Stable,
        ));
        diarization_span.basis_transcript_segment_ids = vec!["segment-1".to_string()];

        let mut ledger = TranscriptLedger::new("session-1");
        ledger.apply_event(transcript.clone()).unwrap();
        let speaker_timeline =
            SpeakerTimeline::replay("session-1", [diarization_span]).expect("speaker timeline");

        let mut patch = notes_patch_for_basis(
            1,
            std::slice::from_ref(&transcript),
            "note-1",
            "Ownership",
            "Ana owns the migration.",
        );
        patch.operations = vec![ProjectionOperation::UpsertNote {
            id: "note-1".to_string(),
            title: "Ownership".to_string(),
            body: "Ana owns the migration.".to_string(),
            tags: vec!["decision".to_string()],
            evidence: crate::claim_evidence::EvidenceAnchor {
                claim_class: crate::claim_evidence::ClaimClass::GroundedInference,
                span_id: Some("span-1".to_string()),
                quote: None,
                note: None,
            },
        }];

        let mut state = MaterializedProjectionState::new("session-1");
        state
            .apply_validated_patch_with_speaker_timeline(&ledger, &speaker_timeline, &patch)
            .expect("apply with speaker timeline");

        let evidence = state.notes.notes[0].evidence.as_ref().expect("admitted");
        assert_eq!(
            evidence.span().and_then(|span| span.speaker_ref.as_deref()),
            Some("speaker-2"),
            "speaker_ref must read the canonical diarization attribution, not the inline ASR label"
        );
    }

    #[test]
    fn materialized_projection_history_uses_each_patch_time_when_timestamps_regress() {
        let mut transcript = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        transcript.received_at_ms = 1_700_000_010_000;
        let mut speaker_revision_one = DiarizationSpanRevision::from(diarization_payload(
            "speaker-span-1",
            "deepgram",
            1,
            "speaker-1",
            DiarizationSpanStability::Provisional,
        ));
        speaker_revision_one.received_at_ms = 1_700_000_010_050;
        let mut speaker_revision_two = DiarizationSpanRevision::from(diarization_payload(
            "speaker-span-1",
            "deepgram",
            2,
            "speaker-2",
            DiarizationSpanStability::Stable,
        ));
        speaker_revision_two.received_at_ms = 1_700_000_010_150;

        let mut later_timestamp_patch = notes_patch_for_basis(
            1,
            std::slice::from_ref(&transcript),
            "note-revision-two",
            "Revision two",
            "Later source state.",
        );
        later_timestamp_patch.basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &[ProjectionBasisSpan {
                span_id: "speaker-span-1".to_string(),
                revision_number: 2,
            }],
        );
        later_timestamp_patch.created_at_ms = 1_700_000_010_200;

        let mut earlier_timestamp_patch = notes_patch_for_basis(
            2,
            std::slice::from_ref(&transcript),
            "note-revision-one",
            "Revision one",
            "Earlier source state in canonical patch order.",
        );
        earlier_timestamp_patch.basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &[ProjectionBasisSpan {
                span_id: "speaker-span-1".to_string(),
                revision_number: 1,
            }],
        );
        earlier_timestamp_patch.created_at_ms = 1_700_000_010_100;

        let replayed = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-1",
            [transcript],
            Some(vec![speaker_revision_one, speaker_revision_two]),
            [later_timestamp_patch, earlier_timestamp_patch],
        )
        .expect("canonical patch replay with regressing timestamps");

        assert_eq!(replayed.validation.checked_patch_count, 2);
        assert_eq!(replayed.validation.invalid_patch_count, 0);
        assert_eq!(replayed.state.notes.last_sequence, 2);
        assert_eq!(replayed.state.notes.notes.len(), 2);
    }

    #[test]
    fn materialized_projection_history_replays_speaker_append_revision_and_repair() {
        let mut transcript = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        transcript.received_at_ms = 1_700_000_010_000;

        let mut first_speaker = DiarizationSpanRevision::from(diarization_payload(
            "speaker-span-1",
            "deepgram",
            1,
            "speaker-1",
            DiarizationSpanStability::Provisional,
        ));
        first_speaker.received_at_ms = 1_700_000_010_010;
        let mut appended_speaker = DiarizationSpanRevision::from(diarization_payload(
            "speaker-span-2",
            "deepgram",
            1,
            "speaker-2",
            DiarizationSpanStability::Stable,
        ));
        appended_speaker.received_at_ms = 1_700_000_010_030;
        let mut revised_speaker = DiarizationSpanRevision::from(diarization_payload(
            "speaker-span-1",
            "deepgram",
            2,
            "speaker-3",
            DiarizationSpanStability::Stable,
        ));
        revised_speaker.received_at_ms = 1_700_000_010_040;

        let mut initial_patch = notes_patch_for_basis(
            1,
            std::slice::from_ref(&transcript),
            "note-initial",
            "Initial",
            "Initial speaker state.",
        );
        initial_patch.basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &[ProjectionBasisSpan {
                span_id: "speaker-span-1".to_string(),
                revision_number: 1,
            }],
        );
        initial_patch.created_at_ms = 1_700_000_010_020;

        let stale_speaker_basis = [
            ProjectionBasisSpan {
                span_id: "speaker-span-1".to_string(),
                revision_number: 1,
            },
            ProjectionBasisSpan {
                span_id: "speaker-span-2".to_string(),
                revision_number: 1,
            },
        ];
        let mut stale_patch = notes_patch_for_basis(
            2,
            std::slice::from_ref(&transcript),
            "note-stale",
            "Stale",
            "Stale speaker state.",
        );
        stale_patch.basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &stale_speaker_basis,
        );
        stale_patch.created_at_ms = 1_700_000_010_050;

        let repaired_speaker_basis = [
            ProjectionBasisSpan {
                span_id: "speaker-span-1".to_string(),
                revision_number: 2,
            },
            ProjectionBasisSpan {
                span_id: "speaker-span-2".to_string(),
                revision_number: 1,
            },
        ];
        let mut repair_patch = notes_patch_for_basis(
            3,
            std::slice::from_ref(&transcript),
            "note-repair",
            "Repair",
            "Current speaker state.",
        );
        repair_patch.basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &repaired_speaker_basis,
        );
        repair_patch.created_at_ms = 1_700_000_010_060;

        let replayed = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-1",
            [transcript],
            Some(vec![first_speaker, appended_speaker, revised_speaker]),
            [initial_patch, stale_patch, repair_patch],
        )
        .expect("speaker retcon replay");

        assert_eq!(replayed.validation.checked_patch_count, 3);
        assert_eq!(replayed.validation.invalid_patch_count, 1);
        assert!(matches!(
            replayed.validation.errors.first(),
            Some(HistoricalProjectionValidationError::StaleBasis {
                sequence: 2,
                staleness: ProjectionBasisStaleness::StaleDiarizationSpanRevision {
                    span_id,
                    current_revision: 2,
                    basis_revision: 1,
                },
                ..
            }) if span_id == "speaker-span-1"
        ));
        assert_eq!(replayed.state.notes.last_sequence, 3);
        assert_eq!(
            replayed
                .state
                .notes
                .notes
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            vec!["note-initial", "note-repair"]
        );
    }

    #[test]
    fn materialized_projection_history_includes_equal_time_speaker_revision() {
        let mut transcript = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        transcript.received_at_ms = 1_700_000_010_000;
        let mut speaker = DiarizationSpanRevision::from(diarization_payload(
            "speaker-span-1",
            "deepgram",
            1,
            "speaker-1",
            DiarizationSpanStability::Stable,
        ));
        speaker.received_at_ms = 1_700_000_010_100;
        let mut patch = notes_patch_for_basis(
            1,
            std::slice::from_ref(&transcript),
            "note-equal-time",
            "Equal time",
            "The speaker revision is visible at the patch boundary.",
        );
        patch.basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &[ProjectionBasisSpan {
                span_id: speaker.span_id.clone(),
                revision_number: speaker.revision_number,
            }],
        );
        patch.created_at_ms = speaker.received_at_ms;

        let replayed = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-1",
            [transcript],
            Some(vec![speaker]),
            [patch],
        )
        .expect("equal-time speaker replay");

        assert_eq!(replayed.validation.invalid_patch_count, 0);
        assert_eq!(replayed.state.notes.last_sequence, 1);
    }

    #[test]
    fn materialized_projection_history_preserves_canonical_speaker_order_for_equal_timestamps() {
        let mut transcript = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        transcript.received_at_ms = 1_700_000_010_000;

        let mut revision_one = DiarizationSpanRevision::from(diarization_payload(
            "speaker-span-1",
            "deepgram",
            1,
            "speaker-1",
            DiarizationSpanStability::Provisional,
        ));
        revision_one.start_time = 2.0;
        revision_one.end_time = 3.0;
        revision_one.received_at_ms = 1_700_000_010_100;

        let mut revision_two = DiarizationSpanRevision::from(diarization_payload(
            "speaker-span-1",
            "deepgram",
            2,
            "speaker-2",
            DiarizationSpanStability::Stable,
        ));
        revision_two.start_time = 1.0;
        revision_two.end_time = 2.0;
        revision_two.received_at_ms = revision_one.received_at_ms;

        let mut patch = notes_patch_for_basis(
            1,
            std::slice::from_ref(&transcript),
            "note-same-time-speaker-revision",
            "Same-time speaker revision",
            "The later canonical speaker revision remains current.",
        );
        patch.basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &[ProjectionBasisSpan {
                span_id: revision_two.span_id.clone(),
                revision_number: revision_two.revision_number,
            }],
        );
        patch.created_at_ms = 1_700_000_010_200;

        let replayed = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-1",
            [transcript],
            Some(vec![revision_one, revision_two]),
            [patch],
        )
        .expect("same-time canonical speaker history replay");

        assert_eq!(replayed.validation.checked_patch_count, 1);
        assert_eq!(replayed.validation.invalid_patch_count, 0);
        assert!(replayed.validation.errors.is_empty());
        assert_eq!(replayed.state.notes.last_sequence, 1);
    }

    #[test]
    fn materialized_projection_history_preserves_missing_and_present_empty_speaker_authority() {
        let mut transcript = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        transcript.received_at_ms = 1_700_000_010_000;
        let mut patch = notes_patch_for_basis(
            1,
            std::slice::from_ref(&transcript),
            "note-speaker-authority",
            "Speaker authority",
            "The speaker stream presence is authoritative.",
        );
        patch.basis = ProjectionBasis::from_transcript_events_and_speaker_spans(
            std::slice::from_ref(&transcript),
            &[ProjectionBasisSpan {
                span_id: "speaker-span-1".to_string(),
                revision_number: 1,
            }],
        );
        patch.created_at_ms = 1_700_000_010_100;

        let missing = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-1",
            [transcript.clone()],
            None,
            [patch.clone()],
        )
        .expect("missing speaker stream report");
        let present_empty = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-1",
            [transcript],
            Some(Vec::new()),
            [patch],
        )
        .expect("present-empty speaker stream report");

        assert!(matches!(
            missing.validation.errors.first(),
            Some(HistoricalProjectionValidationError::StaleBasis {
                staleness: ProjectionBasisStaleness::DiarizationBasisUnavailable { count: 1 },
                ..
            })
        ));
        assert!(matches!(
            present_empty.validation.errors.first(),
            Some(HistoricalProjectionValidationError::StaleBasis {
                staleness: ProjectionBasisStaleness::UnknownDiarizationBasisSpan {
                    span_id,
                    basis_revision: 1,
                },
                ..
            }) if span_id == "speaker-span-1"
        ));
    }

    #[test]
    fn materialized_projection_history_reports_content_free_speaker_replay_failures() {
        let mut transcript = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        transcript.received_at_ms = 1_700_000_010_000;
        let mut patch = notes_patch_for_basis(
            1,
            std::slice::from_ref(&transcript),
            "note-speaker-error",
            "Speaker error",
            "Invalid speaker history must fail closed.",
        );
        patch.created_at_ms = 1_700_000_010_100;

        let mut conflict_a = DiarizationSpanRevision::from(diarization_payload(
            "speaker-span-conflict",
            "deepgram",
            1,
            "speaker-1",
            DiarizationSpanStability::Stable,
        ));
        conflict_a.speaker_label = Some("PRIVATE-SPEAKER-LABEL-A".to_string());
        conflict_a.received_at_ms = 1_700_000_010_010;
        let mut conflict_b = conflict_a.clone();
        conflict_b.speaker_label = Some("PRIVATE-SPEAKER-LABEL-B".to_string());
        conflict_b.received_at_ms = 1_700_000_010_020;

        let conflict = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-1",
            [transcript.clone()],
            Some(vec![conflict_a, conflict_b]),
            [patch.clone()],
        )
        .expect("conflicting speaker replay report");
        assert!(matches!(
            conflict.validation.errors.first(),
            Some(HistoricalProjectionValidationError::SpeakerReplay {
                sequence: 1,
                error: SpeakerTimelineError::ConflictingDiarizationRevision {
                    span_id,
                    revision_number: 1,
                },
            }) if span_id == "speaker-span-conflict"
        ));

        let mut current = DiarizationSpanRevision::from(diarization_payload(
            "speaker-span-stale",
            "deepgram",
            2,
            "speaker-2",
            DiarizationSpanStability::Stable,
        ));
        current.speaker_label = Some("PRIVATE-SPEAKER-LABEL-C".to_string());
        current.received_at_ms = 1_700_000_010_010;
        let mut stale = DiarizationSpanRevision::from(diarization_payload(
            "speaker-span-stale",
            "deepgram",
            1,
            "speaker-1",
            DiarizationSpanStability::Provisional,
        ));
        stale.speaker_label = Some("PRIVATE-SPEAKER-LABEL-D".to_string());
        stale.received_at_ms = 1_700_000_010_020;

        let stale_replay = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-1",
            [transcript],
            Some(vec![current, stale]),
            [patch],
        )
        .expect("stale speaker replay report");
        assert!(matches!(
            stale_replay.validation.errors.first(),
            Some(HistoricalProjectionValidationError::SpeakerReplay {
                sequence: 1,
                error: SpeakerTimelineError::StaleDiarizationRevision {
                    span_id,
                    current_revision: 2,
                    incoming_revision: 1,
                },
            }) if span_id == "speaker-span-stale"
        ));

        let diagnostics = format!("{:?}{:?}", conflict.validation, stale_replay.validation);
        assert!(!diagnostics.contains("PRIVATE-SPEAKER-LABEL"));
    }

    #[test]
    fn runtime_projection_scheduler_and_apply_remain_transcript_only() {
        let scheduler_source = include_str!("projection_scheduler.rs");
        assert!(
            scheduler_source.contains("ledger.current_projection_basis()"),
            "runtime scheduler jobs must continue to use the transcript-only ledger basis"
        );
        assert!(
            !scheduler_source.contains("current_basis_spans"),
            "runtime scheduler must not start consuming speaker-timeline basis spans"
        );

        let state_source = include_str!("state.rs");
        assert!(
            state_source.contains(".apply_validated_patch_reporting_currency(ledger, &patch)"),
            "live runtime apply must retain the transcript-only validation seam"
        );
        assert!(
            !state_source.contains(".apply_validated_patch_with_speaker_timeline("),
            "live runtime apply must not enable speaker-bearing patches in this replay-only wave"
        );

        // REVERT-PIN (audio-graph-caad / audio-graph-f3d4): both apply gates
        // must classify the basis three ways and must never call the two-way
        // validator on a patch basis directly — a revert to the two-way call
        // at either site would silently reopen the caad discard bug. Every
        // needle below is assembled from two literals that never appear
        // contiguous in source, because `include_str!` here pulls in this
        // very test — a needle written as one contiguous literal would
        // match itself and never fail, no matter what the gates do.
        let this_source = include_str!("projections.rs");
        let live_gate_needle = format!(
            "{}{}",
            "ledger.classify_basis_currency(&patch.basis, speaker_timeline", ")"
        );
        let replay_gate_needle = format!(
            "{}{}",
            "ledger.classify_basis_currency(&patch.basis, speaker_timeline.as_ref(", ")"
        );
        let two_way_patch_basis_needle = format!(
            "{}{}",
            "validate_basis_with_speaker_timeline(&patch.", "basis"
        );
        assert!(
            this_source.contains(&live_gate_needle),
            "the live apply gate must classify the basis three ways, not two"
        );
        assert!(
            this_source.contains(&replay_gate_needle),
            "the replay gate must classify the basis three ways, not two"
        );
        assert!(
            !this_source.contains(&two_way_patch_basis_needle),
            "neither gate may call the two-way validator on a patch basis; \
             only classify_basis_currency may distinguish AppendOnlyStale from Revised"
        );

        // REVERT-PIN (audio-graph-caad / audio-graph-f3d4): splitting the
        // applied-append-only-tail telemetry from the ordinary current-basis
        // apply log is a named ticket deliverable, not incidental logging —
        // deleting the `AppendedTail` INFO arm in `speech/mod.rs`, or
        // widening its guard so it also matches `Current`, would silently
        // fold that signal back into the ordinary apply log line and leave
        // the rest of this suite green.
        let speech_source = include_str!("speech/mod.rs");
        assert!(
            speech_source.contains(
                "Projection job applied append-only tail job_id={} kind={:?} staleness={:?}"
            ),
            "the AppendedTail INFO arm's distinctive log line must stay in place"
        );
        assert!(
            speech_source.contains(
                "Projection job apply failed job_id={} kind={:?} stale_apply={} error={:?}"
            ),
            "the stale_apply warn telemetry must stay in place"
        );
    }

    /// The live apply gate and the replay gate must classify a given
    /// (ledger, basis) pair identically to `classify_basis_currency` — the
    /// two sites cannot silently diverge on what "safe to apply" means
    /// (grafted from Design 2's `gate_arms_agree_with_classify_basis_currency`).
    #[test]
    fn gate_arms_agree_with_classify_basis_currency() {
        // Current: nothing changed since the patch's basis was captured.
        let current_event = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        let current_ledger =
            TranscriptLedger::replay("session-1", [current_event.clone()]).expect("ledger");
        let current_patch = notes_patch(1, "note-current", "Decision", "Ship notes.");
        assert_eq!(
            current_ledger.classify_basis_currency(&current_patch.basis, None),
            BasisCurrency::Current
        );
        let (_, currency) = MaterializedProjectionState::new("session-1")
            .apply_validated_patch_reporting_currency(&current_ledger, &current_patch)
            .expect("current basis must apply");
        assert_eq!(currency, AppliedBasisCurrency::Current);
        let current_replay =
            MaterializedProjectionState::replay_accepted_patches_with_transcript_history(
                "session-1",
                [current_event],
                [current_patch],
            )
            .expect("current replay");
        assert_eq!(current_replay.validation.invalid_patch_count, 0);

        // AppendOnlyStale: a new span arrived after the basis was captured
        // but before the patch's `created_at_ms` — the live gate re-checks
        // against the final ledger; the replay gate reconstructs the ledger
        // at `created_at_ms`. Both must agree with `classify_basis_currency`
        // and both must apply, not discard, the patch (audio-graph-caad).
        let old_event = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        let appended_event = TranscriptEvent::from(asr_payload("span-2", 1, "New context."));
        let mut append_only_ledger = TranscriptLedger::new("session-1");
        append_only_ledger
            .apply_event(old_event.clone())
            .expect("seed old event");
        let append_only_patch = notes_patch(1, "note-append-only", "Decision", "Ship notes.");
        append_only_ledger
            .apply_event(appended_event.clone())
            .expect("seed appended event");
        assert!(matches!(
            append_only_ledger.classify_basis_currency(&append_only_patch.basis, None),
            BasisCurrency::AppendOnlyStale(_)
        ));
        let (_, currency) = MaterializedProjectionState::new("session-1")
            .apply_validated_patch_reporting_currency(&append_only_ledger, &append_only_patch)
            .expect("an append-only basis must apply, not be discarded as stale");
        assert!(matches!(
            currency,
            AppliedBasisCurrency::AppendedTail { .. }
        ));
        let append_only_replay =
            MaterializedProjectionState::replay_accepted_patches_with_transcript_history(
                "session-1",
                [old_event, appended_event],
                [append_only_patch],
            )
            .expect("append-only replay");
        assert_eq!(append_only_replay.validation.invalid_patch_count, 0);

        // Revised: the patch's covered span was itself revised — both gates
        // must still refuse to launder it into an evidence proof.
        let stale_event = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        let revised_event =
            TranscriptEvent::from(asr_payload("span-1", 2, "Ship the corrected notes."));
        let mut revised_ledger = TranscriptLedger::new("session-1");
        revised_ledger
            .apply_event(stale_event.clone())
            .expect("seed stale event");
        let revised_patch = notes_patch(1, "note-revised", "Decision", "Ship notes.");
        revised_ledger
            .apply_event(revised_event.clone())
            .expect("seed revised event");
        assert!(matches!(
            revised_ledger.classify_basis_currency(&revised_patch.basis, None),
            BasisCurrency::Revised(_)
        ));
        assert!(matches!(
            MaterializedProjectionState::new("session-1")
                .apply_validated_patch_reporting_currency(&revised_ledger, &revised_patch),
            Err(ProjectionApplyError::StaleBasis { .. })
        ));
        let revised_replay =
            MaterializedProjectionState::replay_accepted_patches_with_transcript_history(
                "session-1",
                [stale_event, revised_event],
                [revised_patch],
            )
            .expect("revised replay");
        assert_eq!(revised_replay.validation.invalid_patch_count, 1);
        assert!(matches!(
            revised_replay.validation.errors.first(),
            Some(HistoricalProjectionValidationError::StaleBasis { .. })
        ));
    }

    /// audio-graph-f3d4 review fix: a boundary-correcting revision that
    /// arrives *during* LLM generation (after `created_at_ms`, before the
    /// live apply gate's fresh snapshot) must not make the live gate and
    /// the replay gate disagree. The live gate accepts this patch as
    /// `AppendOnlyStale` against the ledger it actually sees at apply time
    /// (which already includes the corrected revision); replaying the
    /// session must reach the same verdict, or `load_session_impl` would
    /// refuse to open it (`invalid_patch_count > 0` -> `SessionInvalid`).
    #[test]
    fn replay_gate_agrees_with_live_gate_on_boundary_correction_during_generation() {
        // The basis pins only span-B, an audio span at 5-6s.
        let mut span_b = TranscriptEvent::from(asr_payload("span-b", 1, "Later context."));
        span_b.start_time = 5.0;
        span_b.end_time = 6.0;
        span_b.received_at_ms = 1_000;

        // span-C rev1 arrives between basis capture and `created_at_ms`,
        // audio-positioned *before* span-B (inside the covered prefix).
        let mut span_c_rev1 = TranscriptEvent::from(asr_payload("span-c", 1, "Early context."));
        span_c_rev1.start_time = 0.5;
        span_c_rev1.end_time = 1.5;
        span_c_rev1.received_at_ms = 1_500;

        // span-C rev2 is a boundary-corrected FINAL revision of the same
        // span, arriving *during generation* (after `created_at_ms`),
        // audio-repositioned to a proven tail after span-B.
        let mut span_c_rev2 = TranscriptEvent::from(asr_payload("span-c", 2, "Late context."));
        span_c_rev2.start_time = 7.0;
        span_c_rev2.end_time = 8.0;
        span_c_rev2.received_at_ms = 2_500;

        let mut patch = notes_patch_for_basis(
            1,
            &[span_b.clone()],
            "note-boundary",
            "Decision",
            "Ship it.",
        );
        patch.created_at_ms = 2_000;
        patch.generation_latency_ms = Some(600); // classify_bound_ms = 2_600, covers rev2 at 2_500.

        // Live apply gate: sees the fresh ledger with span-C already at
        // rev2 (the corrected, tail position) and must accept it as an
        // append-only tail, not discard it.
        let mut live_ledger = TranscriptLedger::new("session-1");
        live_ledger.apply_event(span_b.clone()).expect("span-b");
        live_ledger
            .apply_event(span_c_rev1.clone())
            .expect("span-c rev1");
        live_ledger
            .apply_event(span_c_rev2.clone())
            .expect("span-c rev2");
        assert!(matches!(
            live_ledger.classify_basis_currency(&patch.basis, None),
            BasisCurrency::AppendOnlyStale(_)
        ));
        let (_, currency) = MaterializedProjectionState::new("session-1")
            .apply_validated_patch_reporting_currency(&live_ledger, &patch)
            .expect("the live gate must apply the boundary-corrected append-only tail");
        assert!(matches!(
            currency,
            AppliedBasisCurrency::AppendedTail { .. }
        ));

        // Reopen replay must reach the same verdict, or the session
        // becomes permanently unopenable via commands.rs's
        // invalid_patch_count-gated SessionInvalid refusal.
        let replay = MaterializedProjectionState::replay_accepted_patches_with_transcript_history(
            "session-1",
            [span_b, span_c_rev1, span_c_rev2],
            [patch],
        )
        .expect("replay must not error");
        assert_eq!(
            replay.validation.invalid_patch_count, 0,
            "replay diverged from the live gate's AppendOnlyStale acceptance: {:?}",
            replay.validation.errors
        );
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
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "person:alicia".to_string(),
                    name: "Alicia".to_string(),
                    entity_type: "person".to_string(),
                    description: Some("Early duplicate mention.".to_string()),
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "project:audio-graph".to_string(),
                    name: "AudioGraph".to_string(),
                    entity_type: "project".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "topic:providers".to_string(),
                    name: "Providers".to_string(),
                    entity_type: "topic".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge:alice:owns".to_string(),
                    source: "person:alice".to_string(),
                    target: "project:audio-graph".to_string(),
                    relation_type: "owns".to_string(),
                    label: Some("owns".to_string()),
                    weight: 0.8,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge:alicia:owns".to_string(),
                    source: "person:alicia".to_string(),
                    target: "project:audio-graph".to_string(),
                    relation_type: "owns".to_string(),
                    label: Some("owns".to_string()),
                    weight: 0.5,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge:project:providers".to_string(),
                    source: "project:audio-graph".to_string(),
                    target: "topic:providers".to_string(),
                    relation_type: "tracks".to_string(),
                    label: None,
                    weight: 0.6,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
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
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
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

    /// audio-graph-caad / audio-graph-f3d4: an append-only tail must apply,
    /// not be silently discarded as stale, and the gate must report the
    /// currency it proved so callers can split applied-append-only telemetry
    /// from the ordinary current-basis path.
    #[test]
    fn materialized_projection_state_applies_append_only_basis_after_check() {
        let first = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        let second = TranscriptEvent::from(asr_payload("span-2", 1, "New context."));
        let mut ledger = TranscriptLedger::new("session-1");
        ledger.apply_event(first).expect("first event");
        let append_only_patch = notes_patch(1, "note-1", "Decision", "Ship notes.");
        ledger.apply_event(second).expect("second event");

        let mut state = MaterializedProjectionState::new("session-1");
        let (outcome, currency) = state
            .apply_validated_patch_reporting_currency(&ledger, &append_only_patch)
            .expect("an append-only basis must apply, not be discarded as stale");
        assert_eq!(
            outcome,
            MaterializedProjectionApplyOutcome::Notes {
                last_sequence: 1,
                note_count: 1,
            }
        );
        assert!(matches!(
            currency,
            AppliedBasisCurrency::AppendedTail { .. }
        ));
        assert_eq!(state.notes.notes[0].id, "note-1");
    }

    /// audio-graph-f3d4 review fix: `AppliedBasisCurrency` and
    /// `ProjectionBasisStaleness` both derive `#[serde(tag = "type")]`. A
    /// tuple variant wrapping the inner enum directly
    /// (`AppendedTail(ProjectionBasisStaleness)`) flattens the inner enum's
    /// tag into the SAME JSON object as the outer tag, producing two
    /// competing `"type"` keys on one object, which serde_json's internally
    /// tagged deserializer rejects with "duplicate field `type`" — this
    /// guards the named-field shape (`AppendedTail { staleness }`) that
    /// nests the inner enum under its own `staleness` key instead (so its
    /// `"type"` tag lives on a nested object, not the outer one), matching
    /// every other staleness-carrying enum in this module
    /// (`ProjectionApplyError::StaleBasis { staleness }`,
    /// `HistoricalProjectionValidationError::StaleBasis { .. }`).
    #[test]
    fn applied_basis_currency_appended_tail_serde_round_trips() {
        let currency = AppliedBasisCurrency::AppendedTail {
            staleness: ProjectionBasisStaleness::MissingCurrentSpan {
                span_id: "span-2".to_string(),
                current_revision: 1,
            },
        };
        let json = serde_json::to_string(&currency).expect("serialize");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let object = value.as_object().expect("top-level JSON object");
        assert_eq!(
            object.get("type").and_then(|v| v.as_str()),
            Some("appended_tail"),
            "outer tag must not be shadowed by the inner enum's tag: {json}"
        );
        let staleness = object
            .get("staleness")
            .and_then(|v| v.as_object())
            .expect("staleness must nest under its own key, not flatten into the outer object");
        assert_eq!(
            staleness.get("type").and_then(|v| v.as_str()),
            Some("missing_current_span")
        );
        let round_tripped: AppliedBasisCurrency =
            serde_json::from_str(&json).expect("deserialize back");
        assert_eq!(round_tripped, currency);
    }

    #[test]
    fn materialized_projection_state_rejects_revised_basis_before_mutation() {
        let first = TranscriptEvent::from(asr_payload("span-1", 1, "Ship notes."));
        let revised = TranscriptEvent::from(asr_payload("span-1", 2, "Ship the corrected notes."));
        let mut ledger = TranscriptLedger::new("session-1");
        ledger.apply_event(first).expect("first event");
        let old_patch = notes_patch(1, "note-1", "Decision", "Ship notes.");
        ledger.apply_event(revised).expect("revised event");

        let mut state = MaterializedProjectionState::new("session-1");
        assert_eq!(
            state.apply_validated_patch(&ledger, &old_patch),
            Err(ProjectionApplyError::StaleBasis {
                staleness: ProjectionBasisStaleness::StaleSpanRevision {
                    span_id: "span-1".to_string(),
                    current_revision: 2,
                    basis_revision: 1,
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
    fn speaker_timeline_reports_label_remap_on_provisional_to_stable() {
        let provisional = DiarizationSpanRevision::from(diarization_payload(
            "span-1",
            "local_clustering",
            1,
            "2",
            DiarizationSpanStability::Provisional,
        ));
        let stable = DiarizationSpanRevision::from(diarization_payload(
            "span-1",
            "assemblyai",
            2,
            "Alice",
            DiarizationSpanStability::Stable,
        ));

        let mut timeline = SpeakerTimeline::new("session-1");
        let first = timeline
            .apply_event(provisional)
            .expect("provisional accepted");
        assert!(
            first.is_none(),
            "first-seen labels have no prior graph identity to retcon"
        );

        let remap = timeline
            .apply_event(stable)
            .expect("stable accepted")
            .expect("changed human-facing speaker label should report remap");
        assert_eq!(remap.superseded_label, "Speaker 2");
        assert_eq!(remap.canonical_label, "Speaker Alice");
    }

    #[test]
    fn speaker_timeline_reports_no_remap_when_label_unchanged() {
        let mut timeline = SpeakerTimeline::new("session-1");
        let first = DiarizationSpanRevision::from(diarization_payload(
            "span-1",
            "local_clustering",
            1,
            "2",
            DiarizationSpanStability::Provisional,
        ));
        let stable_same_label = DiarizationSpanRevision::from(diarization_payload(
            "span-1",
            "assemblyai",
            2,
            "2",
            DiarizationSpanStability::Stable,
        ));

        assert!(
            timeline
                .apply_event(first)
                .expect("first accepted")
                .is_none()
        );
        assert!(
            timeline
                .apply_event(stable_same_label)
                .expect("stable accepted")
                .is_none(),
            "unchanged labels are span enrichment, not entity retcons"
        );
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
