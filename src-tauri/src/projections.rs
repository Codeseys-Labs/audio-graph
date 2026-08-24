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
use std::sync::Arc;

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

/// Compact pointer to the covered-but-summarized prefix of a
/// [`ProjectionBasis`] (audio-graph-cfa1): everything at or before
/// [`ProjectionBasis::summarized_through_revision`], folded into the rolling
/// summary window (ADR-0025 §2c) and therefore never re-sent to the
/// projection LLM verbatim. `span_revisions` used to carry every one of
/// these spans individually (`(span_id, revision_number)` per entry), which
/// grows without bound across a long session — field evidence showed one
/// basis carrying 933 entries despite a `summarized_through_revision` of 12.
/// This digest replaces that per-span identity list with a count and a
/// content hash, computed the same way
/// [`ProjectionBasis::from_transcript_events_and_speaker_spans`] always has
/// (`transcript_events_hash_v1` over the exact prefix events in canonical
/// order) so any revision, reorder, or deletion inside the prefix still
/// changes the digest and is still caught as [`BasisCurrency::Revised`] —
/// see `classify_basis_currency`'s prefix-reconstruction branch.
///
/// Never carries span ids or text: this is deliberately opaque provenance,
/// not a smaller identity list.
///
/// `content_hash` has no `hash_version` tag of its own, unlike
/// [`ProjectionBasis::transcript_hash`]. This is a deliberate, disclosed gap
/// rather than an oversight: `content_hash` is always produced by the exact
/// same frozen `transcript_events_hash_v1` call, in the same constructor
/// invocation, as `transcript_hash` itself — there is only ever one hash
/// algorithm active for a given `ProjectionBasis` instance, so the digest's
/// algorithm is definitionally the instance's own `hash_version` today.
/// `covered_prefix` is an audio-graph-cfa1 addition with exactly one
/// algorithm that has ever existed for it, so adding a version field now
/// would tag a value with no second variant to dispatch on. When
/// [`TranscriptHashVersion`] grows a v2 (ADR-0042), `ProjectionBasis`'s own
/// exhaustive `Deserialize` match on `hash_version` will already force every
/// call site that constructs or verifies a `covered_prefix` to be revisited
/// in lockstep — that is the natural point to decide whether the digest
/// needs its own explicit version tag, not before v2 exists.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct CoveredPrefixDigest {
    pub span_count: usize,
    pub content_hash: String,
}

/// Compaction is enforced by construction, not by field privacy: fields stay
/// `pub` (matching this module's convention for plain value types such as
/// [`TranscriptEvent`]) and the manual [`Deserialize`](serde::Deserialize)
/// impl below copies `span_revisions` through byte-verbatim with no
/// re-truncation, because a historical basis's untruncated list is exactly
/// what ADR-0042 byte-stability requires it to keep. What IS guaranteed:
/// every basis a *production* caller ever constructs goes through
/// [`Self::from_transcript_events_and_speaker_spans`] (the sole constructor
/// `TranscriptLedger::current_basis`/`current_projection_basis` call), so
/// compaction lands there unconditionally. A hand-built struct literal with a
/// long `span_revisions` and `covered_prefix: None` is still constructible —
/// audited (not type-enforced) to be confined to `#[cfg(test)]` fixture code
/// today. Deserialize cannot add that enforcement either: legacy on-disk
/// bases must round-trip with their original (possibly long) `span_revisions`
/// intact, so a blanket length assertion at deserialize time would itself
/// violate ADR-0042.
#[derive(Clone, PartialEq, Eq)]
pub struct ProjectionBasis {
    /// Spans NOT covered by `covered_prefix` — i.e. the whole covered set
    /// for a basis that predates compaction or hasn't grown past the
    /// rolling-summary hot window yet, or just the verbatim hot-window tail
    /// once `covered_prefix` is `Some`. `covered_prefix.is_none()` is this
    /// struct's own "legacy/uncompacted" encoding marker — deliberately NOT
    /// layered onto `hash_version`/[`TranscriptHashVersion`], which
    /// ADR-0042 reserves exclusively for the `session_semantics_version`-
    /// floor-gated transcript-hash algorithm; compaction is an orthogonal
    /// storage concern and must not ride the same dispatch rule.
    pub span_revisions: Vec<ProjectionBasisSpan>,
    /// `Some` once the covered set has grown past the rolling-summary hot
    /// window ([`ROLLING_SUMMARY_HOT_WINDOW_TURNS`]); see
    /// [`CoveredPrefixDigest`]. `None` for every basis this crate ever wrote
    /// before audio-graph-cfa1 (and for any basis, old or new, whose covered
    /// set still fits inside the hot window) — those keep the exact
    /// pre-compaction `span_revisions` shape.
    pub covered_prefix: Option<CoveredPrefixDigest>,
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

/// Compact, bounded Debug representation (audio-graph-cfa1): counts and
/// boundary revisions only, never the `span_revisions`/`covered_prefix`
/// identity content. `ProjectionBasis` is embedded in `ProjectionJob` and
/// `ProjectionSchedulerDecision`, both of which derive `Debug` and both of
/// which reach production `log::debug!` sites (`speech/mod.rs`'s
/// `observe_asr_revision` and scheduler-completion logs) — a derived Debug
/// here would dump every covered span id on every tick, which is exactly the
/// field-observed 80KB-single-line log bloat this ticket exists to fix.
/// Mirrors the existing redaction precedent on `TranscriptEvent`/
/// `MaterializedNote`/etc. in this same module.
impl fmt::Debug for ProjectionBasis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProjectionBasis")
            .field("span_revisions_count", &self.span_revisions.len())
            .field("covered_prefix", &self.covered_prefix)
            .field(
                "diarization_span_revisions_count",
                &self.diarization_span_revisions.len(),
            )
            .field("transcript_hash", &self.transcript_hash)
            .field(
                "summarized_through_revision",
                &self.summarized_through_revision,
            )
            .finish()
    }
}

impl serde::Serialize for ProjectionBasis {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;

        let field_count = 4
            + usize::from(self.covered_prefix.is_some())
            + usize::from(self.summarized_through_revision.is_some());
        let mut state = serializer.serialize_struct("ProjectionBasis", field_count)?;
        state.serialize_field("span_revisions", &self.span_revisions)?;
        if let Some(covered_prefix) = &self.covered_prefix {
            state.serialize_field("covered_prefix", covered_prefix)?;
        }
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
            #[serde(default)]
            covered_prefix: Option<CoveredPrefixDigest>,
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
                covered_prefix: wire.covered_prefix,
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

    /// Total number of transcript spans this basis covers, counting both the
    /// verbatim tail in `span_revisions` and any compacted prefix recorded in
    /// `covered_prefix` (audio-graph-cfa1). Equal to `span_revisions.len()`
    /// for every basis that predates compaction, or whose covered set never
    /// grew past the rolling-summary hot window. Callers that use
    /// `span_revisions.len()` as a stand-in for "how much this basis covers"
    /// (coalescing thresholds, telemetry) must use this instead so they keep
    /// seeing the true covered size rather than a value permanently capped at
    /// [`ROLLING_SUMMARY_HOT_WINDOW_TURNS`].
    pub fn covered_span_count(&self) -> usize {
        self.span_revisions.len()
            + self
                .covered_prefix
                .as_ref()
                .map_or(0, |prefix| prefix.span_count)
    }

    /// Reconstruct every transcript event this basis covers by resolving
    /// against `ledger_events` (typically [`TranscriptLedger::latest_spans`]).
    ///
    /// For an uncompacted basis this is exactly the existing exact-identity
    /// lookup every caller already did directly against `span_revisions`. For
    /// a compacted basis, the prefix is reconstructed by chronological
    /// position (the same [`ordered_for_window`] order used to build it) and
    /// verified against [`CoveredPrefixDigest::content_hash`] before being
    /// trusted — see [`reconstruct_verified_covered_prefix`]. A basis built
    /// with partials included (`current_basis` — rare on the automatic
    /// projection path, but a real production caller:
    /// `commands.rs`'s `approved_agent_projection_patch`, behind the
    /// `approve_agent_proposal` Tauri command, builds its patch basis this
    /// way) and one built eligible-only (`current_projection_basis`, the
    /// common case) both resolve correctly: this tries the eligible-only
    /// candidate universe first and falls back to every ledger event —
    /// pinned by
    /// `resolve_covered_events_falls_back_to_the_full_ledger_when_a_partial_lives_in_the_summarized_prefix`.
    ///
    /// Returns fewer events than [`Self::covered_span_count`] only when
    /// `ledger_events` cannot reproduce what this basis recorded (a caller
    /// resolving against a stale or foreign ledger) — it never fabricates or
    /// returns unverified content, matching the tolerant
    /// filter-out-what's-missing behavior every caller already had for a
    /// missing tail entry.
    pub fn resolve_covered_events(
        &self,
        ledger_events: &[TranscriptEvent],
    ) -> Vec<TranscriptEvent> {
        let tail_ids: std::collections::BTreeSet<&str> = self
            .span_revisions
            .iter()
            .map(|span| span.span_id.as_str())
            .collect();

        let mut resolved: Vec<TranscriptEvent> = match &self.covered_prefix {
            Some(prefix) => {
                let eligible: Vec<TranscriptEvent> = ledger_events
                    .iter()
                    .filter(|event| projection_event_is_eligible(event))
                    .cloned()
                    .collect();
                reconstruct_verified_covered_prefix(&eligible, &tail_ids, prefix)
                    .or_else(|| {
                        reconstruct_verified_covered_prefix(ledger_events, &tail_ids, prefix)
                    })
                    .unwrap_or_default()
            }
            None => Vec::new(),
        };

        let latest_by_identity: BTreeMap<(&str, u64), &TranscriptEvent> = ledger_events
            .iter()
            .map(|event| ((event.span_id.as_str(), event.revision_number), event))
            .collect();
        resolved.extend(self.span_revisions.iter().filter_map(|span| {
            latest_by_identity
                .get(&(span.span_id.as_str(), span.revision_number))
                .map(|event| (*event).clone())
        }));
        resolved
    }

    /// Build a basis from the canonical transcript revisions plus the current
    /// speaker-timeline span revisions. The speaker spans are provider-neutral
    /// [`ProjectionBasisSpan`]s (typically [`SpeakerTimeline::current_basis_spans`]);
    /// passing an empty slice yields a transcript-only basis identical to
    /// [`Self::from_transcript_events`].
    ///
    /// Compaction (audio-graph-cfa1): everything at or before the rolling
    /// summary boundary ([`summarized_through_revision`]) is folded into
    /// `covered_prefix` instead of being listed in `span_revisions`, exactly
    /// mirroring [`ROLLING_SUMMARY_HOT_WINDOW_TURNS`]'s own split. This is the
    /// ONLY basis constructor every production caller uses
    /// (`TranscriptLedger::current_basis` / `current_projection_basis`), so
    /// compaction lands for free at every embedding site without a scattered
    /// per-caller truncation call. `transcript_hash` still covers the whole
    /// set (prefix and tail) exactly as before — compaction changes what
    /// `span_revisions` stores, never the hash-v1 algorithm or its value.
    pub fn from_transcript_events_and_speaker_spans(
        events: &[TranscriptEvent],
        speaker_spans: &[ProjectionBasisSpan],
    ) -> Self {
        let latest_events = latest_transcript_events(events);
        let ordered = ordered_for_window(&latest_events);
        let hot_window_len = ordered.len().min(ROLLING_SUMMARY_HOT_WINDOW_TURNS);
        let prefix_len = ordered.len() - hot_window_len;

        let covered_prefix = if prefix_len == 0 {
            None
        } else {
            let prefix_events: Vec<TranscriptEvent> = ordered[..prefix_len]
                .iter()
                .map(|event| (*event).clone())
                .collect();
            Some(CoveredPrefixDigest {
                span_count: prefix_events.len(),
                content_hash: transcript_events_hash_v1(&prefix_events),
            })
        };
        let tail_ids: std::collections::BTreeSet<&str> = ordered[prefix_len..]
            .iter()
            .map(|event| event.span_id.as_str())
            .collect();

        Self {
            // Preserve the pre-compaction span_id-lexicographic order
            // (`latest_events` is a `BTreeMap`-derived Vec) rather than the
            // chronological `ordered` order used only to find the split —
            // `classify_basis_currency`'s per-index tail comparison and every
            // existing snapshot/test assume this ordering.
            span_revisions: latest_events
                .iter()
                .filter(|event| tail_ids.contains(event.span_id.as_str()))
                .map(|event| ProjectionBasisSpan {
                    span_id: event.span_id.clone(),
                    revision_number: event.revision_number,
                })
                .collect(),
            covered_prefix,
            diarization_span_revisions: speaker_spans.to_vec(),
            transcript_hash: transcript_events_hash_v1(&latest_events),
            summarized_through_revision: summarized_through_revision(&latest_events),
        }
    }
}

/// Attempt to reconstruct a [`CoveredPrefixDigest`]'s covered events from
/// `candidates`, verifying the reconstruction against the recorded digest
/// before returning it (audio-graph-cfa1).
///
/// `candidates` should already be deduped to one (latest) event per
/// `span_id` — both call sites pass either `TranscriptLedger::latest_spans`
/// or an eligibility-filtered copy of it, which is already deduped by
/// construction. The prefix is exactly the events NOT in `tail_ids`, taken
/// in the same [`ordered_for_window`] chronological order used to build the
/// digest, up to `prefix.span_count`. Returns `None` (never partial,
/// unverified content) when `candidates` has fewer than `prefix.span_count`
/// eligible non-tail events, or when the reconstructed set's content hash
/// does not match — either means `candidates` is not the universe this
/// basis was built from (wrong ledger, or the caller needs to retry with a
/// different eligibility filter).
fn reconstruct_verified_covered_prefix(
    candidates: &[TranscriptEvent],
    tail_ids: &std::collections::BTreeSet<&str>,
    prefix: &CoveredPrefixDigest,
) -> Option<Vec<TranscriptEvent>> {
    let others: Vec<TranscriptEvent> = candidates
        .iter()
        .filter(|event| !tail_ids.contains(event.span_id.as_str()))
        .cloned()
        .collect();
    let ordered = ordered_for_window(&others);
    if ordered.len() < prefix.span_count {
        return None;
    }
    let reconstructed: Vec<TranscriptEvent> = ordered
        .into_iter()
        .take(prefix.span_count)
        .cloned()
        .collect();
    if transcript_events_hash_v1(&reconstructed) == prefix.content_hash {
        Some(reconstructed)
    } else {
        None
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

    /// Convert a wall-clock creation timestamp (Unix epoch ms) into the
    /// session-relative-seconds domain that
    /// `TemporalKnowledgeGraph::process_extraction` expects — the same
    /// "relative to stream start" clock as `AccumulatedSegment::start_time`
    /// (`speech/mod.rs`) and every `TranscriptEvent::start_time` in this
    /// ledger.
    ///
    /// There is no dedicated "session start" anchor stored anywhere in
    /// `AppState`: the live speech path derives its clock from a local
    /// `Instant` that is never persisted. A durable per-session wall clock
    /// DOES exist (`SessionMetadata::created_at`, `sessions/mod.rs`) — it was
    /// considered and rejected: it is manifest-creation time, not
    /// audio-stream-start time, and reading it here would add a disk read
    /// (plus a session-id lookup this function doesn't otherwise need) for an
    /// anchor no more accurate than the one already in hand. This ledger's
    /// own `(start_time, received_at_ms)` pairs are that cheaper,
    /// already-in-hand mapping between the two clocks, and using them is
    /// inherently rotation-safe — `rotate_session` replaces this ledger
    /// wholesale (see `AppState::rotate_session`), so a stale anchor from a
    /// prior session can never leak into a new one the way a cached
    /// wall-clock offset could.
    ///
    /// Anchor preference:
    /// 1. The exact span that produced `source_segment_id`, matched against
    ///    either `span_id` or `transcript_segment_id` — mirroring
    ///    `live_assist_evidence_anchor`'s OR-match in `commands.rs`, since a
    ///    partial-revision producer can leave `transcript_segment_id` unset
    ///    (e.g. `speech/mod.rs`'s diarization-only revision path), so a
    ///    caller's id may only resolve against the immutable `span_id`.
    /// 2. Any other span currently in the ledger, when no exact match exists.
    /// 3. `0.0` (session start) when the ledger has no spans at all.
    ///
    /// All three branches carry the SAME class of error: whatever
    /// capture/ASR latency separates the anchor span's own `start_time` from
    /// its own `received_at_ms` (the formula below reconstructs an implied
    /// session-start wall clock from the anchor alone —
    /// `received_at_ms - start_time * 1000` — then re-applies it to
    /// `created_at_ms`). Preferring the exact match is NOT about
    /// `created_at_ms` landing closer to that span's `received_at_ms`; every
    /// span's implied anchor is equally "fresh" regardless of which one is
    /// picked. It matters because it anchors on the span the caller is
    /// actually citing, so a reader can attribute the result's drift to that
    /// specific span's own capture latency instead of an arbitrary other
    /// span's.
    ///
    /// Branch 3 (`0.0`) is reachable in production, not just a defensive
    /// stub: `record_asr_span_revision_event` (`speech/mod.rs`) returns
    /// `false` without advancing this ledger whenever the transcript-event
    /// writer is unavailable or rejects the append, while the segment still
    /// enters `transcript_buffer` and can still source a manual proposal or
    /// auto-added question. When that happens, this returns `0.0` (session
    /// start) rather than panicking or guessing — the SAFE failure direction,
    /// since it makes the node the graph's EARLIEST eviction candidate, the
    /// opposite of the epoch-timestamp immortality bug this function exists
    /// to fix.
    pub fn session_relative_timestamp(&self, source_segment_id: &str, created_at_ms: u64) -> f64 {
        let anchor = self
            .latest_spans
            .iter()
            .find(|event| {
                event.span_id == source_segment_id
                    || event.transcript_segment_id.as_deref() == Some(source_segment_id)
            })
            .or_else(|| self.latest_spans.first());

        match anchor {
            Some(event) => {
                event.start_time + (created_at_ms as f64 - event.received_at_ms as f64) / 1000.0
            }
            None => 0.0,
        }
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
        let projection_events = self.latest_spans.clone();

        // audio-graph-cfa1: reconstruct the FULL set this basis covers (its
        // verbatim tail plus, when compacted, the summarized-away prefix)
        // before deciding anything else. `resolve_covered_events` verifies
        // any recorded `covered_prefix` digest against `projection_events`
        // internally and silently drops the prefix on a verification
        // failure (revised/reordered/deleted content, or a shrunk ledger) —
        // safe here because that always shows up as a covered-count
        // mismatch a few lines below, before `basis_tracks_partial` (also
        // derived from this reconstruction) is ever read. For a basis that
        // predates compaction (`covered_prefix: None`), this is exactly the
        // original tail-only-identity lookup with no behavior change.
        let covered_events = basis.resolve_covered_events(&projection_events);
        let covered_ids: std::collections::BTreeSet<&str> = covered_events
            .iter()
            .map(|event| event.span_id.as_str())
            .collect();

        // Projection jobs created by the current scheduler never include
        // provisional events. Historical bases may legitimately contain them,
        // so remember that distinction for the extra-span classification below
        // while validating every covered span against the full ledger. Reads
        // the reconstructed covered set (not just the tail) so a partial
        // hidden inside a compacted prefix is still detected.
        let basis_tracks_partial = covered_events
            .iter()
            .any(|event| !projection_event_is_eligible(event));

        let current_spans: BTreeMap<&str, u64> = projection_events
            .iter()
            .map(|event| (event.span_id.as_str(), event.revision_number))
            .collect();

        // First prove that every span the basis names in its verbatim tail
        // still exists at the exact revision it saw. A revision or removal
        // invalidates the basis even when unrelated spans were also appended
        // later. The summarized-away prefix (if any) is proven below by hash
        // reconstruction instead — compaction never exposes its identities
        // for a per-span check.
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

        // Prove the reconstructed covered set is exactly the size the basis
        // recorded before hashing it. A prefix whose verification failed
        // inside `resolve_covered_events` always shows up here first,
        // because `resolve_covered_events` drops the whole prefix rather
        // than return unverified content.
        if covered_events.len() != basis.covered_span_count() {
            return BasisCurrency::Revised(ProjectionBasisStaleness::CoveredSpanCountMismatch {
                current_count: covered_events.len(),
                basis_count: basis.covered_span_count(),
            });
        }

        // Prove the exposed per-span order of the basis's verbatim tail
        // (`span_revisions`) matches the current ledger's own deterministic
        // order (ADR-0031 step 3/4's "reordered" check). For a legacy
        // (uncompacted) basis, `span_revisions` names the WHOLE covered set,
        // so `tail_ids` below is every covered id and this proves the whole
        // set's order — exactly the original pre-cfa1 behavior. For a
        // compacted basis it proves only the exposed tail's order.
        //
        // audio-graph-cfa1 (post-adversarial-review fix): this check used to
        // be skipped ENTIRELY whenever `covered_prefix.is_some()`, silently
        // narrowing ADR-0031's "reordered ... covered span" detection to
        // legacy bases only — a hand-corrupted `span_revisions` permutation
        // on a compacted basis's tail passed undetected, because neither the
        // per-id `basis_spans` map (order-independent) nor the covered-hash
        // check below (which canonicalizes by chronological order, not
        // vector order) is sensitive to on-disk vector order. Filtering to
        // `tail_ids` (rather than gating the whole block on
        // `covered_prefix.is_none()`) restores the check uniformly. The
        // summarized-away prefix still has no exposed per-span order to
        // compare here: its content — and therefore its order — is proven by
        // the hash check below instead, and `transcript_events_hash_v1`
        // canonicalizes by chronological order internally, so a prefix built
        // from the same event set always hashes identically regardless of
        // input order. There is no separate "prefix order" fact for a
        // corrupted vector to misrepresent that the hash wouldn't already
        // catch as a content change.
        let tail_ids: std::collections::BTreeSet<&str> = basis_spans.keys().copied().collect();
        let covered_span_revisions: Vec<ProjectionBasisSpan> =
            latest_transcript_events(&covered_events)
                .iter()
                .filter(|event| tail_ids.contains(event.span_id.as_str()))
                .map(|event| ProjectionBasisSpan {
                    span_id: event.span_id.clone(),
                    revision_number: event.revision_number,
                })
                .collect();
        for (index, (current_span, basis_span)) in covered_span_revisions
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

        // Hash exactly the current-ledger subset covered by the basis before
        // inspecting later appends. Comparing against the full current hash
        // would misclassify every legitimate append, while skipping this
        // check would let a forged/corrupt hash through when ids and
        // revisions match. `transcript_hash` is always computed over the
        // WHOLE covered set regardless of how `span_revisions` represents it
        // (ADR-0042's hash-v1 algorithm never changed), so hashing the
        // reconstructed set directly is the correct comparison for both a
        // legacy and a compacted basis.
        let current_covered_hash = transcript_events_hash_v1(&covered_events);
        if current_covered_hash != basis.transcript_hash {
            return BasisCurrency::Revised(ProjectionBasisStaleness::TranscriptHashMismatch {
                current_hash: current_covered_hash,
                basis_hash: basis.transcript_hash.clone(),
            });
        }

        // Preserve legacy bases whose summary boundary field was absent, but
        // reject an explicit boundary that disagrees with the exact covered
        // subset. This check also happens before append-only classification.
        let covered_summarized_through = summarized_through_revision(&covered_events);
        if let (Some(basis_summarized), Some(covered_summarized)) = (
            basis.summarized_through_revision,
            covered_summarized_through,
        ) && basis_summarized != covered_summarized
        {
            return BasisCurrency::Revised(ProjectionBasisStaleness::SummaryWindowMismatch {
                current_summarized_through: covered_summarized_through,
                basis_summarized_through: basis.summarized_through_revision,
            });
        }

        if projection_events.len() == basis.covered_span_count() {
            return BasisCurrency::Current;
        }

        // A final-only projection basis is still current when the only new
        // ledger entries are provisional. They remain durable transcript
        // revisions but are intentionally invisible to the projection queue.
        // Keep legacy partial-bearing and empty bases on the full-ledger rule.
        if !basis_tracks_partial
            && !covered_ids.is_empty()
            && projection_events.iter().all(|event| {
                covered_ids.contains(event.span_id.as_str()) || !projection_event_is_eligible(event)
            })
        {
            return BasisCurrency::Current;
        }

        // A valid append-only basis must cover the exact chronological prefix.
        // `ProjectionBasis::span_revisions` is deterministically keyed by span
        // id, so its vector position cannot prove audio chronology. Inspect the
        // current events in the same timestamp order used by transcript hashes
        // and rolling windows instead. An uncovered event inside the first N
        // events is a non-tail insertion, not a harmless append. Membership is
        // checked against `covered_ids` (tail + reconstructed prefix), not
        // just `basis_spans` (tail only) — a compacted basis's covered set is
        // mostly the (chronologically earliest) prefix.
        let currency_events: Vec<TranscriptEvent> = projection_events
            .iter()
            .filter(|event| {
                covered_ids.is_empty()
                    || basis_tracks_partial
                    || covered_ids.contains(event.span_id.as_str())
                    || projection_event_is_eligible(event)
            })
            .cloned()
            .collect();
        let ordered_current = ordered_for_window(&currency_events);
        let covered_count = basis.covered_span_count();
        if let Some((_, inserted)) = ordered_current
            .iter()
            .take(covered_count)
            .enumerate()
            .find(|(_, event)| !covered_ids.contains(event.span_id.as_str()))
        {
            return BasisCurrency::Revised(ProjectionBasisStaleness::MissingCurrentSpan {
                span_id: inserted.span_id.clone(),
                current_revision: inserted.revision_number,
            });
        }

        let extra = ordered_current[covered_count];
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
    /// Additive, EVENT-PAYLOAD-ONLY mirror of
    /// [`crate::state::ProjectionRuntimeApplyResult::basis_currency_at_apply`]
    /// (ticket W3, audio-graph-a6b5). `ProjectionPatch` is one Rust type that
    /// serves BOTH the persisted canonical projection event log
    /// (`ProjectionEventWriter::append` -> `write_projection_event`,
    /// `persistence/mod.rs`) AND the frontend-bound `PROJECTION_PATCH` Tauri
    /// event (`TauriProjectionRuntimeEventSink::emit_projection_patch`,
    /// `speech/mod.rs`) — both take `&ProjectionPatch`. This field is set on
    /// a SEPARATE clone at the apply-success emit site
    /// (`emit_projection_runtime_events`'s caller in `speech/mod.rs`), never
    /// on the value handed to `apply_runtime_projection_patch` that gets
    /// persisted. Consequence: the canonical log's serialized bytes are
    /// completely unaffected by this field — `None`/absent is what every
    /// pre-W3 record deserializes to (`#[serde(default)]`) AND what every
    /// record this app ever writes to disk continues to deserialize to,
    /// forever (ADR-0045 replay: byte-identical, not merely tolerant). Only
    /// the frontend wire clone ever carries `Some`. Downstream: the strip's
    /// recency chip (`liveWorkspaceTone.ts`) maps this tagged enum's `.type`
    /// onto its `BasisCurrencyEvidence` — `Current` is the only value that
    /// can ever earn a "success" tone; `AppendedTail` stays honest-neutral.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub basis_currency_at_apply: Option<AppliedBasisCurrency>,
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
            .field("basis_currency_at_apply", &self.basis_currency_at_apply)
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
                heading_level,
            } => f
                .debug_struct("UpsertNote")
                .field("id", id)
                .field("title", &REDACTED_DEBUG_VALUE)
                .field("body", &REDACTED_DEBUG_VALUE)
                .field("tags", tags)
                .field("evidence", &DebugEvidenceAnchor(evidence))
                .field("heading_level", heading_level)
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
        /// Document heading depth for `title` (audio-graph-a6b5 W1), clamped
        /// to `2..=4` by [`normalize_projection_patch_draft_doc_structure`]
        /// at fresh ingest (2 = top-level section, 3 = subsection, 4 =
        /// sub-subsection). `#[serde(default)]` makes this ADDITIVE and
        /// OPTIONAL: `ProjectionOperation`/`ProjectionPatch` carry no
        /// `#[serde(deny_unknown_fields)]` (that attribute lives only on the
        /// model-facing `ProjectionPatchDraft`, and only gates ITS OWN
        /// top-level keys — it does not reach into a nested operation's
        /// fields), so an old build's strict canonical reader tolerates this
        /// key as an unrecognized field rather than failing the whole
        /// `projection_patches` stream (ADR-0045; see the strict-reader
        /// pinning test in `commands.rs`).
        ///
        /// `None` is NOT a class-satisfying absence and NOT "level 2" — it
        /// means this operation asserted no document structure at all: a
        /// pre-living-document session's card op, or a model that omitted
        /// the field. The frontend's legacy renderer handles `None`; nothing
        /// on this path ever fabricates a depth into the materialized
        /// record. Mirrors the posture of `MaterializedNote::evidence`'s
        /// `Option` doc comment above.
        ///
        /// W1 shipped this field DARK (no model-facing prompt or schema
        /// surface change on any route). audio-graph-a6b5 W2 is the flip to
        /// model-visible, landed as one commit across both routes together
        /// so neither ever tells the model something the other forbids: the
        /// hand-authored strict OpenRouter schema
        /// (`projection_patch_strict_json_schema`) now advertises
        /// `heading_level` as a required-but-nullable integer (matching
        /// `description`/`after_id`/`label`'s existing posture), and the
        /// `schemars`-derived draft schema (`projection_patch_draft_json_schema`)
        /// advertises it too — but with this doc comment's internal
        /// ticket/ADR prose swapped out for a short model-facing sentence by
        /// `shorten_heading_level_description_in_draft_schema`, since
        /// `schemars` would otherwise paste this whole comment verbatim into
        /// every projection prompt as the property's JSON Schema
        /// `description`. The Notes-kind operation guidance
        /// (`projection_patch_prompt_messages`) now names `heading_level` by
        /// name and explains its 2..=4 depth scale directly.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        heading_level: Option<u8>,
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
    /// Shared, not owned per-note (audio-graph-cfa1): one `ProjectionPatch`
    /// commonly touches many notes/nodes/edges in a single apply cycle, and
    /// every touched item used to deep-clone the WHOLE
    /// [`ProjectionBasis`] (span identities, prefix digest, hash) out of
    /// `patch.basis` independently. `apply_patch` now clones `patch.basis`
    /// into one `Arc` once per patch and every mutator clones the `Arc`
    /// (a refcount bump) instead. Serializes/deserializes byte-identically
    /// to an owned `ProjectionBasis` (`serde`'s `rc` feature).
    pub basis: Arc<ProjectionBasis>,
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
    /// Carried through unchanged from the source `UpsertNote` operation
    /// (audio-graph-a6b5 W1) — see that variant's doc comment for the full
    /// contract. `None` on notes materialized before this field existed, or
    /// from an operation that asserted no structure; never fabricated into
    /// a default depth by materialization. Full-replace semantics: a later
    /// `UpsertNote` for the same id overwrites this field exactly like every
    /// other field on the record (there is no incremental merge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heading_level: Option<u8>,
}

impl fmt::Debug for MaterializedNote {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MaterializedNote")
            .field("id", &self.id)
            .field("title", &REDACTED_DEBUG_VALUE)
            .field("body", &REDACTED_DEBUG_VALUE)
            .field("tags", &self.tags)
            .field("heading_level", &self.heading_level)
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
        // audio-graph-cfa1: clone `patch.basis` into an `Arc` exactly ONCE
        // per patch, not once per touched note. A single patch can carry
        // many `UpsertNote` operations, and every one used to deep-clone the
        // whole `ProjectionBasis` independently out of `patch.basis`.
        let basis = Arc::new(patch.basis.clone());
        for operation in &patch.operations {
            match operation {
                ProjectionOperation::UpsertNote {
                    id,
                    title,
                    body,
                    tags,
                    evidence,
                    heading_level,
                } => next.upsert_note(
                    id,
                    title,
                    body,
                    tags,
                    *heading_level,
                    evidence,
                    evidence_basis,
                    &basis,
                    patch,
                ),
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
        heading_level: Option<u8>,
        evidence: &crate::claim_evidence::EvidenceAnchor,
        evidence_basis: Option<&BTreeMap<&str, &TranscriptEvent>>,
        basis: &Arc<ProjectionBasis>,
        patch: &ProjectionPatch,
    ) {
        let next = MaterializedNote {
            id: id.to_string(),
            title: title.to_string(),
            body: body.to_string(),
            tags: tags.to_vec(),
            heading_level,
            updated_by_sequence: patch.sequence,
            updated_at_ms: patch.created_at_ms,
            basis: Arc::clone(basis),
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
    /// See [`MaterializedNote::basis`] — shared across every node/edge one
    /// apply cycle touches instead of deep-cloned per item.
    pub basis: Arc<ProjectionBasis>,
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
    /// See [`MaterializedNote::basis`] — shared across every node/edge one
    /// apply cycle touches instead of deep-cloned per item.
    pub basis: Arc<ProjectionBasis>,
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
    /// Raw model-supplied node id -> the id `upsert_node` ACTUALLY landed it
    /// on, for every redirection ever observed across THIS graph's whole
    /// history (audio-graph-e700 replay-compatibility fix). Unlike the
    /// per-`apply_patch` `id_overrides` local used while an operation list is
    /// being processed, this map is a field on the graph itself: it is
    /// carried forward by `apply_patch` (`next = self.clone()` copies it,
    /// `*self = next` commits it) so a LATER, SEPARATE patch — including one
    /// replayed from an empty graph on session reload — can still resolve a
    /// raw id that an EARLIER patch's upsert redirected elsewhere (e.g. a
    /// fuzzy cross-id name merge). See `resolve_graph_node_id`'s doc comment
    /// for the full resolution order and why a literal existing row always
    /// takes priority over an entry here. Never pruned; only ever grows,
    /// which is safe because every key is a short model-invented id string,
    /// not transcript content.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub id_aliases: BTreeMap<String, String>,
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
            .field("id_aliases", &self.id_aliases)
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
            id_aliases: BTreeMap::new(),
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
        // Raw model id -> the id it was ACTUALLY persisted under, for THIS
        // patch only (seed audio-graph-e700 sub-fixes 2/3). `upsert_node`
        // may land a fresh `UpsertGraphNode` on a DIFFERENT id than the one
        // the model supplied — either because it merged into an existing
        // near-duplicate entity found by name, or because the model's raw id
        // collided with an unrelated existing node and had to be
        // disambiguated (see `upsert_node`'s doc comment). Every OTHER
        // node-id-bearing operation in this SAME patch resolves through this
        // map FIRST — it always wins over `next.id_aliases` below, because it
        // reflects what THIS patch itself just did, which must take priority
        // over any older redirection history (audio-graph-e700 blocker-class
        // fix: without this, a same-patch reference right after a displaced
        // upsert would incorrectly fall through to a stale cross-patch
        // alias, or to a pre-existing unrelated node that happens to still
        // literally own the raw id). Local to one `apply_patch` call, never
        // stored on `self`.
        let mut id_overrides: BTreeMap<String, String> = BTreeMap::new();
        // audio-graph-cfa1: clone `patch.basis` into an `Arc` exactly ONCE
        // per patch, not once per touched node/edge. A single patch can
        // touch many nodes and edges, and every one used to deep-clone the
        // whole `ProjectionBasis` independently out of `patch.basis`.
        let basis = Arc::new(patch.basis.clone());

        for operation in &patch.operations {
            match operation {
                ProjectionOperation::UpsertGraphNode {
                    id,
                    name,
                    entity_type,
                    description,
                    evidence,
                } => {
                    let final_id = next.upsert_node(
                        id,
                        name,
                        entity_type,
                        description.clone(),
                        evidence,
                        evidence_basis,
                        &basis,
                        patch,
                    );
                    if final_id != *id {
                        id_overrides.insert(id.clone(), final_id.clone());
                        // Persisted across patches too (audio-graph-e700
                        // replay-compatibility fix), so a LATER, SEPARATE
                        // patch that references this same raw id — including
                        // one replayed from an empty graph on session reload
                        // — can still resolve it instead of hard-erroring.
                        // See `resolve_graph_node_id`'s doc comment for why a
                        // literal existing row always takes priority over
                        // this entry, so it never overrides a legitimate
                        // stable-id reuse.
                        next.id_aliases.insert(id.clone(), final_id);
                    }
                }
                ProjectionOperation::RemoveGraphNode { id } => {
                    let id = resolve_graph_node_id(&next, &id_overrides, id);
                    next.nodes.retain(|node| node.id != id);
                    next.edges
                        .retain(|edge| edge.source != id && edge.target != id);
                }
                ProjectionOperation::InvalidateGraphNode { id } => {
                    let id = resolve_graph_node_id(&next, &id_overrides, id);
                    next.invalidate_node(&id, &basis, patch)?;
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
                    let source = resolve_graph_node_id(&next, &id_overrides, source);
                    let target = resolve_graph_node_id(&next, &id_overrides, target);
                    if !next.has_active_node(&source) {
                        return Err(ProjectionApplyError::MissingGraphNode {
                            edge_id: id.clone(),
                            node_id: source,
                        });
                    }
                    if !next.has_active_node(&target) {
                        return Err(ProjectionApplyError::MissingGraphNode {
                            edge_id: id.clone(),
                            node_id: target,
                        });
                    }
                    next.upsert_edge(
                        id,
                        &source,
                        &target,
                        relation_type,
                        label.clone(),
                        *weight,
                        evidence,
                        evidence_basis,
                        &basis,
                        patch,
                    );
                }
                ProjectionOperation::RemoveGraphEdge { id } => {
                    next.edges.retain(|edge| edge.id != *id);
                }
                ProjectionOperation::InvalidateGraphEdge { id } => {
                    next.invalidate_edge(id, &basis, patch)?;
                }
                ProjectionOperation::StrengthenGraphEdge { id, weight_delta } => {
                    next.adjust_edge_weight(
                        "strengthen_graph_edge",
                        id,
                        *weight_delta,
                        &basis,
                        patch,
                    )?;
                }
                ProjectionOperation::WeakenGraphEdge { id, weight_delta } => {
                    next.adjust_edge_weight(
                        "weaken_graph_edge",
                        id,
                        -*weight_delta,
                        &basis,
                        patch,
                    )?;
                }
                ProjectionOperation::MergeGraphNodes {
                    source_id,
                    target_id,
                } => {
                    let source_id = resolve_graph_node_id(&next, &id_overrides, source_id);
                    let target_id = resolve_graph_node_id(&next, &id_overrides, target_id);
                    // A merge whose two ends resolve to the SAME node is a
                    // no-op, not an error (audio-graph-e700
                    // replay-compatibility fix): a model naturally emits
                    // `upsert n1; upsert n7 (fuzzy-absorbed into n1);
                    // merge_graph_nodes(source: n7, target: n1)` in one
                    // patch to explicitly reconcile the near-duplicate it
                    // just created — `upsert_node` already unified them, so
                    // there is nothing left to merge. Erroring here would
                    // reject the WHOLE patch (this operation runs inside the
                    // same all-or-nothing `next`) for a redundant-but-benign
                    // operation, exactly the automatic-blocker class this
                    // ticket forbids. Mirrors the TS
                    // `applyProjectionGraphPatch`'s existing
                    // `sourceId === targetId` no-op guard.
                    if source_id != target_id {
                        next.merge_nodes(&source_id, &target_id, &basis, patch)?;
                    }
                }
                ProjectionOperation::SplitGraphNode {
                    id,
                    replacement_nodes,
                } => {
                    let id = resolve_graph_node_id(&next, &id_overrides, id);
                    next.split_node(&id, replacement_nodes, &basis, patch)?;
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

    /// Upsert a graph node, returning the id it was ACTUALLY persisted under
    /// (seed audio-graph-e700 sub-fixes 2 and 3).
    ///
    /// Before this change, this matched purely on the model-supplied `id`
    /// (`self.nodes.iter_mut().find(|node| node.id == id)`), which is what
    /// let two unrelated projection ticks that both happened to invent id
    /// `"node1"` silently overwrite each other's node under one shared id —
    /// the field evidence behind this ticket measured 54 of 155 persisted
    /// node ids carrying more than one distinct name across one session.
    /// Identity now resolves by NAME (via [`fuzzy_entity_name_match`]), in
    /// three tiers:
    ///
    /// 1. A node — ACTIVE or already invalidated — already has this exact
    ///    `id`, and its name still matches `name`. Update it in place, keep
    ///    the id, and RESURRECT it (clear `valid_until_ms`) if it was
    ///    invalidated. Covers both the common "stable id, refined over
    ///    later ticks" case (see `projection_llm`'s
    ///    `later_graph_context_can_update_stable_node_id` test) and the
    ///    "invalidate, then re-upsert the same entity under the same id"
    ///    case a pre-e700 accepted log may depend on for replay (the old
    ///    code matched purely on id regardless of active status, so it
    ///    always resurrected in place; requiring ACTIVE here — as an
    ///    earlier version of this fix did — mints a needless disambiguated
    ///    id instead and can make a later patch's raw-id reference to this
    ///    same entity hard-error on replay; audio-graph-e700
    ///    replay-compatibility fix). The name check still means an
    ///    UNRELATED entity minted under this same (invalidated) id is never
    ///    mistaken for the original — falls through to tier 2/3 instead.
    /// 2. No id match (or the id match's name diverged too far to be the
    ///    same entity — the collision case) — search ALL active nodes of
    ///    the SAME `entity_type` for a name match anywhere. A hit means
    ///    either a near-duplicate under a DIFFERENT raw id (sub-fix 3, e.g.
    ///    "Postgres" / "PostgreSQL" minted under two different ids across
    ///    ticks) or the collision case landing on its true owner. Update
    ///    that node, keep ITS id. `apply_patch` records this redirection —
    ///    raw `id` -> the final id returned here — in `self.id_aliases` so
    ///    a LATER, separate patch that references the raw id by itself
    ///    (an edge endpoint, an invalidate, a merge...) can still resolve it
    ///    instead of hard-erroring (see `resolve_graph_node_id`).
    /// 3. No match anywhere — a genuinely new entity. Use `id` verbatim
    ///    UNLESS some other node (active or already invalidated) already
    ///    owns that exact literal id with a name that never matched it —
    ///    then mint a disambiguated id ([`Self::disambiguated_new_node_id`])
    ///    so two distinct entities never share one persisted row.
    ///
    /// `entity_type` is compared case-insensitively but otherwise exactly as
    /// given — never the ontology-canonicalized form — because this method
    /// also runs unmodified against pre-audio-graph-e700 replay data, which
    /// may carry inconsistent free-string types the fresh-ingest normalizer
    /// (`projection_llm::normalize_projection_patch_draft_ontology`) never
    /// touched.
    ///
    /// DISCLOSED TRADE-OFF: a same-id rename whose new name falls OUTSIDE
    /// [`fuzzy_entity_name_match`]'s window (e.g. `"Roadmap"` ->
    /// `"Q3 Roadmap"`, or `"Alice"` -> `"Alice Smith"`) fails tier 1 on the
    /// name check and, finding no other match, forks a visible duplicate
    /// under a disambiguated id at tier 3 instead of updating in place —
    /// this method does NOT preserve every pre-e700 same-id update pattern,
    /// only ones whose name still fuzzy-matches. Accepted against the
    /// collision bug this whole function exists to fix; pinned by
    /// `same_id_rename_beyond_the_fuzzy_window_forks_a_disambiguated_duplicate`.
    #[allow(clippy::too_many_arguments)]
    fn upsert_node(
        &mut self,
        id: &str,
        name: &str,
        entity_type: &str,
        description: Option<String>,
        evidence: &crate::claim_evidence::EvidenceAnchor,
        evidence_basis: Option<&BTreeMap<&str, &TranscriptEvent>>,
        basis: &Arc<ProjectionBasis>,
        patch: &ProjectionPatch,
    ) -> String {
        let same_id_match = self
            .nodes
            .iter()
            .position(|node| node.id == id && fuzzy_entity_name_match(&node.name, name));
        let target_index = same_id_match.or_else(|| {
            self.nodes.iter().position(|node| {
                node.valid_until_ms.is_none()
                    && node.entity_type.eq_ignore_ascii_case(entity_type)
                    && fuzzy_entity_name_match(&node.name, name)
            })
        });

        let final_id = match target_index {
            Some(index) => self.nodes[index].id.clone(),
            None => self.disambiguated_new_node_id(id),
        };

        let next = MaterializedGraphNode {
            id: final_id.clone(),
            name: name.to_string(),
            entity_type: entity_type.to_string(),
            description,
            confidence: patch.confidence,
            valid_from_ms: patch.created_at_ms,
            valid_until_ms: None,
            updated_by_sequence: patch.sequence,
            updated_at_ms: patch.created_at_ms,
            basis: Arc::clone(basis),
            provenance: patch.provenance.clone(),
            evidence: resolve_admitted_claim_evidence(evidence, evidence_basis),
        };

        match target_index {
            Some(index) => self.nodes[index] = next,
            None => self.nodes.push(next),
        }

        final_id
    }

    /// Mint a collision-free id for a genuinely new node when the model's
    /// raw `id` is already owned by a DIFFERENT entity — any node, active or
    /// already invalidated, ever recorded under that literal string (the
    /// audio-graph-e700 field bug: two ticks independently invented the same
    /// generic id, e.g. `"node1"`, for two unrelated entities). Deterministic
    /// within one call: the first free `"{id}~2"`, `"{id}~3"`, ... suffix, so
    /// replaying the same patch log always produces the same disambiguated
    /// id — it depends on nothing but the graph's own existing id set, never
    /// on wall-clock time or randomness.
    fn disambiguated_new_node_id(&self, id: &str) -> String {
        if !self.nodes.iter().any(|node| node.id == id) {
            return id.to_string();
        }
        let mut suffix: u64 = 2;
        loop {
            let candidate = format!("{id}~{suffix}");
            if !self.nodes.iter().any(|node| node.id == candidate) {
                return candidate;
            }
            suffix += 1;
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
        basis: &Arc<ProjectionBasis>,
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
            basis: Arc::clone(basis),
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
        basis: &Arc<ProjectionBasis>,
        patch: &ProjectionPatch,
    ) -> Result<(), ProjectionApplyError> {
        let Some(index) = self.active_node_index(id) else {
            return Err(ProjectionApplyError::MissingGraphNodeForOperation {
                operation: "invalidate_graph_node",
                node_id: id.to_string(),
            });
        };

        self.invalidate_node_at(index, basis, patch);
        for edge_index in 0..self.edges.len() {
            let edge = &self.edges[edge_index];
            if edge.valid_until_ms.is_none() && (edge.source == id || edge.target == id) {
                self.invalidate_edge_at(edge_index, basis, patch);
            }
        }
        Ok(())
    }

    fn invalidate_edge(
        &mut self,
        id: &str,
        basis: &Arc<ProjectionBasis>,
        patch: &ProjectionPatch,
    ) -> Result<(), ProjectionApplyError> {
        let Some(index) = self.active_edge_index(id) else {
            return Err(ProjectionApplyError::MissingGraphEdgeForOperation {
                operation: "invalidate_graph_edge",
                edge_id: id.to_string(),
            });
        };

        self.invalidate_edge_at(index, basis, patch);
        Ok(())
    }

    fn adjust_edge_weight(
        &mut self,
        operation: &'static str,
        id: &str,
        weight_delta: f32,
        basis: &Arc<ProjectionBasis>,
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
        edge.basis = Arc::clone(basis);
        edge.provenance = patch.provenance.clone();
        Ok(())
    }

    fn merge_nodes(
        &mut self,
        source_id: &str,
        target_id: &str,
        basis: &Arc<ProjectionBasis>,
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

        self.invalidate_node_at(source_index, basis, patch);
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
                self.invalidate_edge_at(edge_index, basis, patch);
            } else if self.edges[edge_index].source == target_id
                || self.edges[edge_index].target == target_id
            {
                self.edges[edge_index].updated_by_sequence = patch.sequence;
                self.edges[edge_index].updated_at_ms = patch.created_at_ms;
                self.edges[edge_index].basis = Arc::clone(basis);
                self.edges[edge_index].provenance = patch.provenance.clone();
            }
        }
        self.invalidate_duplicate_active_edges(basis, patch);
        Ok(())
    }

    fn split_node(
        &mut self,
        id: &str,
        replacement_nodes: &[GraphNodeDraft],
        basis: &Arc<ProjectionBasis>,
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

        self.invalidate_node_at(index, basis, patch);
        for edge_index in 0..self.edges.len() {
            let edge = &self.edges[edge_index];
            if edge.valid_until_ms.is_none() && (edge.source == id || edge.target == id) {
                self.invalidate_edge_at(edge_index, basis, patch);
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
                basis,
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

    fn invalidate_node_at(
        &mut self,
        index: usize,
        basis: &Arc<ProjectionBasis>,
        patch: &ProjectionPatch,
    ) {
        let node = &mut self.nodes[index];
        node.valid_until_ms = Some(patch.created_at_ms);
        node.confidence = patch.confidence;
        node.updated_by_sequence = patch.sequence;
        node.updated_at_ms = patch.created_at_ms;
        node.basis = Arc::clone(basis);
        node.provenance = patch.provenance.clone();
    }

    fn invalidate_edge_at(
        &mut self,
        index: usize,
        basis: &Arc<ProjectionBasis>,
        patch: &ProjectionPatch,
    ) {
        let edge = &mut self.edges[index];
        edge.valid_until_ms = Some(patch.created_at_ms);
        edge.confidence = patch.confidence;
        edge.updated_by_sequence = patch.sequence;
        edge.updated_at_ms = patch.created_at_ms;
        edge.basis = Arc::clone(basis);
        edge.provenance = patch.provenance.clone();
    }

    fn invalidate_duplicate_active_edges(
        &mut self,
        basis: &Arc<ProjectionBasis>,
        patch: &ProjectionPatch,
    ) {
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
                self.edges[winner_index].basis = Arc::clone(basis);
                self.edges[winner_index].provenance = patch.provenance.clone();
                self.invalidate_edge_at(edge_index, basis, patch);
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

/// Cheap, semantics-preserving equivalent of [`TranscriptLedger::apply_event`]
/// for [`LedgerHistory`]'s persistent forward-fold map (audio-graph-927a):
/// an O(log n) `BTreeMap` find/insert keyed by `span_id`, instead of
/// `apply_event`'s O(n) linear `Vec` find plus a full `Vec::sort_by`
/// re-sort after every event. Stale/conflict detection and their exact
/// error payloads are identical — only the storage/cost shape differs.
/// [`TranscriptLedger::apply_event`] itself is untouched, so the
/// live-capture path that shares it keeps its exact pre-existing cost
/// profile (see [`LedgerHistory`]'s doc comment for why that's safe to
/// leave alone).
///
/// Deliberately does NOT reproduce `apply_event`'s `sort_latest_spans` call:
/// callers materialize a `TranscriptLedger` from this map via
/// [`LedgerHistory::materialize_transcript_ledger`], whose `latest_spans`
/// vector order this module's only two consumers —
/// [`TranscriptLedger::classify_basis_currency`] and
/// [`resolve_claim_evidence_basis_events`] — never depend on: every place
/// either function needs a deterministic order (windowing, hashing,
/// order-mismatch detection) re-derives it explicitly via
/// [`ordered_for_window`] rather than trusting incoming slice order.
fn fast_apply_transcript_event(
    map: &mut BTreeMap<String, TranscriptEvent>,
    event: TranscriptEvent,
) -> Result<(), TranscriptLedgerError> {
    #[cfg(test)]
    LEDGER_HISTORY_FOLD_OPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match map.get(&event.span_id) {
        Some(current) if event.revision_number < current.revision_number => {
            Err(TranscriptLedgerError::StaleTranscriptRevision {
                span_id: event.span_id,
                current_revision: current.revision_number,
                incoming_revision: event.revision_number,
            })
        }
        Some(current) if event.revision_number == current.revision_number && event != *current => {
            Err(TranscriptLedgerError::ConflictingTranscriptRevision {
                span_id: event.span_id,
                revision_number: event.revision_number,
            })
        }
        _ => {
            map.insert(event.span_id.clone(), event);
            Ok(())
        }
    }
}

/// [`fast_apply_transcript_event`]'s [`SpeakerTimeline`] counterpart. Mirrors
/// [`SpeakerTimeline::apply_event`]'s stale/conflict semantics exactly;
/// deliberately drops the `Option<SpeakerLabelRemap>` return value because
/// neither replay caller of the original speaker fold loops
/// (`replay_accepted_patches_with_history`'s evidence fold nor the old
/// classify-bound reconstruction) ever read it — both only checked
/// `.is_err()`.
fn fast_apply_speaker_event(
    map: &mut BTreeMap<String, DiarizationSpanRevision>,
    event: DiarizationSpanRevision,
) -> Result<(), SpeakerTimelineError> {
    #[cfg(test)]
    LEDGER_HISTORY_FOLD_OPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    match map.get(&event.span_id) {
        Some(current) if event.revision_number < current.revision_number => {
            Err(SpeakerTimelineError::StaleDiarizationRevision {
                span_id: event.span_id,
                current_revision: current.revision_number,
                incoming_revision: event.revision_number,
            })
        }
        Some(current) if event.revision_number == current.revision_number && event != *current => {
            Err(SpeakerTimelineError::ConflictingDiarizationRevision {
                span_id: event.span_id,
                revision_number: event.revision_number,
            })
        }
        _ => {
            map.insert(event.span_id.clone(), event);
            Ok(())
        }
    }
}

/// Work-counter for the complexity pinning test
/// (`ledger_history_folds_each_event_a_bounded_number_of_times_not_once_per_patch`,
/// audio-graph-927a deliverable c). Counts every call into
/// [`fast_apply_transcript_event`] / [`fast_apply_speaker_event`] — i.e.
/// every time a raw event is actually folded, by either cursor, whether via
/// the persistent forward advance or an isolated regression/poison fresh
/// fold. `#[cfg(test)]`-only: zero footprint in the shipped binary, and
/// mutation-proof by construction — a revert to rebuilding a fresh
/// [`TranscriptLedger`]/[`SpeakerTimeline`] per patch via
/// [`TranscriptLedger::apply_event`]/[`SpeakerTimeline::apply_event`]
/// directly (bypassing this module) never touches this counter at all, so
/// the test's lower bound (every event folded at least once) catches that
/// silently, and the upper bound catches a revert that routes the old
/// per-patch-refold shape through these functions instead.
///
/// SERIAL-ONLY precondition (audio-graph-927a review finding): this is a
/// single process-global counter with no per-test isolation. Every
/// prescribed gate in this repo runs `cargo test -- --test-threads=1`
/// (grep `.github/workflows/ci.yml` and this ticket's own gate commands),
/// so no other test's fold operations can land between a reset and a read.
/// A parallel (default-threaded) `cargo test --lib` run can flake this
/// counter's exact-equality assertion if some OTHER test that also calls
/// `replay_accepted_patches_with_history` happens to interleave — this is
/// an accepted, documented constraint of the counter's design, not a bug in
/// the test itself.
#[cfg(test)]
static LEDGER_HISTORY_FOLD_OPS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
fn reset_ledger_history_fold_ops_counter() {
    LEDGER_HISTORY_FOLD_OPS.store(0, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
fn ledger_history_fold_ops_counter() -> usize {
    LEDGER_HISTORY_FOLD_OPS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Forward-cursor reconstruction of the transcript ledger / speaker timeline
/// used by [`MaterializedProjectionState::replay_accepted_patches_with_history`]
/// (audio-graph-927a; supersedes the old free function
/// `replay_ledger_and_timeline_up_to`, which rebuilt a fresh
/// [`TranscriptLedger`]/[`SpeakerTimeline`] from event zero on every call).
///
/// # The problem this replaces
///
/// The previous implementation rebuilt a fresh ledger/timeline from event
/// zero for EVERY patch, TWICE per patch (once for the draft-time evidence
/// bound, once for the classify-time currency bound) — O(patches × events)
/// — and each [`TranscriptLedger::apply_event`]/[`SpeakerTimeline::apply_event`]
/// call inside that rebuild is itself O(current ledger size) (linear
/// span-id find) plus a full `Vec` re-sort. On a field session with 1,215
/// patches and 4,697 transcript events that is ~5.7M redundant
/// `TranscriptEvent` clones (plus ~1.14M speaker-revision clones) per
/// session-open.
///
/// # The fix
///
/// `LedgerHistory` folds each raw event AT MOST ONCE across the whole
/// replay by keeping a persistent `BTreeMap<span_id, latest event>` (O(log
/// n) find/update via [`fast_apply_transcript_event`] /
/// [`fast_apply_speaker_event`] — no `Vec` re-sort at all) plus a monotonic
/// "how far has this track been folded" cursor (`*_applied_len`).
/// Snapshotting at a bound means: advance the cursor to that bound (folding
/// only events not yet seen) and materialize the currently-visible spans
/// into a [`TranscriptLedger`]/[`SpeakerTimeline`] for the existing
/// [`TranscriptLedger::classify_basis_currency`] /
/// [`resolve_claim_evidence_basis_events`] call sites — deliberately left
/// unmodified by this ticket. Their own O(current ledger size) per-call cost
/// (both start by cloning the full `latest_spans`) is NOT what this ticket
/// targets: that cost is bounded by the number of DISTINCT spans, is paid
/// once per patch either way, and shrinks strictly relative to the old
/// per-patch RAW EVENT re-fold (which reprocessed every revision, not just
/// the latest one per span). What this module eliminates is the redundant
/// *re-folding of the same raw events* across patches.
///
/// One sort DOES still happen, once per snapshot rather than once per raw
/// event: [`Self::materialize_transcript_ledger`] /
/// [`Self::materialize_speaker_timeline`] (and their fresh-fold
/// counterparts) re-sort the materialized `latest_spans` into
/// [`TranscriptLedger::sort_latest_spans`]/[`SpeakerTimeline::sort_latest_spans`]'s
/// chronological order before returning — the persistent `BTreeMap`'s
/// natural iteration order is span-id-lexicographic, which
/// `timeline::speaker_attribution_index`'s exact-tie-break reads directly
/// and must not diverge from what the live `apply_event` path would have
/// produced (audio-graph-927a review fix). This is O(m log m) over the
/// DISTINCT-span count `m`, batched at snapshot points exactly as this
/// module's own doc above invites ("amortize: sorted insert or batch-sort
/// at snapshot points") — it is not a regression back to a per-event
/// re-sort.
///
/// # Two independent cursors, not one
///
/// The evidence bound (`created_at_ms`) and the classify bound
/// (`created_at_ms + generation_latency_ms`) are drawn from the same event
/// stream but do not advance together. `created_at_ms` is non-decreasing
/// across sequence-ordered patches in real sessions, but `classify_bound_ms`
/// is NOT — a slow generation on patch N can push its classify bound past a
/// fast patch N+1's, so patch N+1's classify bound can regress relative to
/// patch N's even when nothing else is unusual. (`created_at_ms` itself can
/// also regress in a defensively-tested, if not realistic, input shape —
/// see `materialized_projection_history_uses_each_patch_time_when_timestamps_regress`,
/// which feeds patches with deliberately out-of-order `created_at_ms`.) A
/// single shared cursor fed both bound sequences interleaved would
/// manufacture spurious regressions (patch N+1's smaller evidence bound
/// arriving right after patch N's larger classify bound), so each bound
/// sequence gets its own `LedgerHistory` instance, each independently
/// monotonic.
///
/// # Regression handling
///
/// When a requested bound falls behind this cursor's own high-water mark
/// (`*_applied_len`), this falls back to an ISOLATED fresh fold over just
/// that smaller prefix — the same cost the old implementation always paid,
/// scoped to one bound — rather than corrupting the running cursor state.
/// Correctness over cleverness for the rare/adversarial case; the
/// monotonic-advance path above is what carries the complexity win for the
/// realistic (non-regressing) case this ticket's field evidence describes.
/// A genuine fold failure (a stale/conflicting revision actually present in
/// the raw event stream) "poisons" the cursor at the first index it occurs:
/// the cursor freezes there forever (matching the old implementation, which
/// hit the identical deterministic failure at the identical index on every
/// fresh rebuild reaching that far) — see
/// [`Self::advance_transcript`]/[`Self::advance_speaker`].
struct LedgerHistory<'a> {
    session_id: &'a str,
    transcript_events: &'a [TranscriptEvent],
    speaker_events: &'a [DiarizationSpanRevision],
    speaker_history_present: bool,

    transcript_applied_len: usize,
    transcript_map: BTreeMap<String, TranscriptEvent>,
    transcript_accepted: u64,
    /// `(index, error)` of the first transcript fold failure ever hit while
    /// advancing, if any. Once set, `transcript_applied_len == index`
    /// forever — the persistent map never advances past a genuine failure.
    transcript_poison: Option<(usize, TranscriptLedgerError)>,

    speaker_applied_len: usize,
    speaker_map: BTreeMap<String, DiarizationSpanRevision>,
    speaker_accepted: u64,
    speaker_poison: Option<(usize, SpeakerTimelineError)>,
}

impl<'a> LedgerHistory<'a> {
    fn new(
        session_id: &'a str,
        transcript_events: &'a [TranscriptEvent],
        speaker_events: &'a [DiarizationSpanRevision],
        speaker_history_present: bool,
    ) -> Self {
        Self {
            session_id,
            transcript_events,
            speaker_events,
            speaker_history_present,
            transcript_applied_len: 0,
            transcript_map: BTreeMap::new(),
            transcript_accepted: 0,
            transcript_poison: None,
            speaker_applied_len: 0,
            speaker_map: BTreeMap::new(),
            speaker_accepted: 0,
            speaker_poison: None,
        }
    }

    /// Number of `transcript_events` with `received_at_ms <= bound_ms` —
    /// exactly the prefix `take_while(|e| e.received_at_ms <= bound_ms)`
    /// would produce. `transcript_events` is sorted ascending by
    /// `received_at_ms` (the caller's pre-sort, unchanged by this module),
    /// so that prefix is a genuine, well-defined `Vec` prefix and this
    /// binary search over exactly that field reproduces its length in
    /// O(log n) instead of a linear scan.
    fn transcript_prefix_len(&self, bound_ms: u64) -> usize {
        self.transcript_events
            .partition_point(|event| event.received_at_ms <= bound_ms)
    }

    fn speaker_prefix_len(&self, bound_ms: u64) -> usize {
        self.speaker_events
            .partition_point(|event| event.received_at_ms <= bound_ms)
    }

    /// Advance the persistent transcript fold to `target_len`, folding only
    /// events not yet seen. No-ops immediately if already poisoned (nothing
    /// further can ever be folded past a genuine failure) or if
    /// `target_len` does not exceed `transcript_applied_len` (nothing new to
    /// fold — including every regression case, left to the caller to
    /// resolve via an isolated fresh fold).
    fn advance_transcript(&mut self, target_len: usize) {
        if self.transcript_poison.is_some() {
            return;
        }
        while self.transcript_applied_len < target_len {
            let event = self.transcript_events[self.transcript_applied_len].clone();
            match fast_apply_transcript_event(&mut self.transcript_map, event) {
                Ok(()) => {
                    self.transcript_applied_len += 1;
                    self.transcript_accepted += 1;
                }
                Err(error) => {
                    self.transcript_poison = Some((self.transcript_applied_len, error));
                    break;
                }
            }
        }
    }

    fn advance_speaker(&mut self, target_len: usize) {
        if self.speaker_poison.is_some() {
            return;
        }
        while self.speaker_applied_len < target_len {
            let event = self.speaker_events[self.speaker_applied_len].clone();
            match fast_apply_speaker_event(&mut self.speaker_map, event) {
                Ok(()) => {
                    self.speaker_applied_len += 1;
                    self.speaker_accepted += 1;
                }
                Err(error) => {
                    self.speaker_poison = Some((self.speaker_applied_len, error));
                    break;
                }
            }
        }
    }

    /// Materializes `self.transcript_map`'s values into a [`TranscriptLedger`],
    /// then re-sorts `latest_spans` into [`TranscriptLedger::sort_latest_spans`]'s
    /// chronological `(start_time, end_time, span_id)` order — the SAME order
    /// [`TranscriptLedger::apply_event`] (the live-capture path) always
    /// produces. Without this, `latest_spans` would carry the persistent
    /// `BTreeMap`'s span-id-lexicographic key order instead (audio-graph-927a
    /// review finding, blocker): harmless for every consumer that re-derives
    /// order explicitly via [`ordered_for_window`], but
    /// `timeline::speaker_attribution_index`'s exact-tie-break ("first
    /// iterated wins" when two spans share both `revision_number` AND
    /// `received_at_ms`) reads `SpeakerTimeline::latest_spans`' vector order
    /// directly and would silently pick a different winner than the live
    /// path on that tie. This sort is O(m log m) over the DISTINCT-span
    /// count `m` (not the raw event count) — it does not reintroduce the
    /// O(n) per-EVENT re-sort this ticket removed from the fold hot path;
    /// it runs once per snapshot, exactly like [`TranscriptLedger::apply_event`]'s
    /// own re-sort would have, just batched instead of per-event.
    fn materialize_transcript_ledger(&self) -> TranscriptLedger {
        let mut ledger = TranscriptLedger {
            schema_version: TranscriptLedger::SCHEMA_VERSION,
            session_id: self.session_id.to_string(),
            accepted_event_count: self.transcript_accepted,
            latest_spans: self.transcript_map.values().cloned().collect(),
        };
        ledger.sort_latest_spans();
        ledger
    }

    /// [`Self::materialize_transcript_ledger`]'s speaker counterpart — see
    /// that method's doc comment for why the chronological re-sort is
    /// required, not optional. This is the fix for the reported blocker:
    /// `speaker_attribution_index`'s tie-break is exact-order-sensitive, so
    /// this snapshot must reproduce [`SpeakerTimeline::apply_event`]'s
    /// `(start_time, end_time, span_id)` order exactly.
    fn materialize_speaker_timeline(&self) -> SpeakerTimeline {
        let mut timeline = SpeakerTimeline {
            schema_version: SpeakerTimeline::SCHEMA_VERSION,
            session_id: self.session_id.to_string(),
            accepted_event_count: self.speaker_accepted,
            latest_spans: self.speaker_map.values().cloned().collect(),
        };
        timeline.sort_latest_spans();
        timeline
    }

    fn fresh_fold_transcript_or_error(
        session_id: &str,
        events_prefix: &[TranscriptEvent],
    ) -> Result<TranscriptLedger, TranscriptLedgerError> {
        let mut map = BTreeMap::new();
        let mut accepted = 0u64;
        for event in events_prefix {
            fast_apply_transcript_event(&mut map, event.clone())?;
            accepted += 1;
        }
        let mut ledger = TranscriptLedger {
            schema_version: TranscriptLedger::SCHEMA_VERSION,
            session_id: session_id.to_string(),
            accepted_event_count: accepted,
            latest_spans: map.into_values().collect(),
        };
        ledger.sort_latest_spans();
        Ok(ledger)
    }

    fn fresh_fold_speaker_or_error(
        session_id: &str,
        events_prefix: &[DiarizationSpanRevision],
    ) -> Result<SpeakerTimeline, SpeakerTimelineError> {
        let mut map = BTreeMap::new();
        let mut accepted = 0u64;
        for event in events_prefix {
            fast_apply_speaker_event(&mut map, event.clone())?;
            accepted += 1;
        }
        let mut timeline = SpeakerTimeline {
            schema_version: SpeakerTimeline::SCHEMA_VERSION,
            session_id: session_id.to_string(),
            accepted_event_count: accepted,
            latest_spans: map.into_values().collect(),
        };
        timeline.sort_latest_spans();
        Ok(timeline)
    }

    fn fresh_fold_transcript_degrading(
        session_id: &str,
        events_prefix: &[TranscriptEvent],
    ) -> TranscriptLedger {
        let mut map = BTreeMap::new();
        let mut accepted = 0u64;
        for event in events_prefix {
            match fast_apply_transcript_event(&mut map, event.clone()) {
                Ok(()) => accepted += 1,
                Err(_) => break,
            }
        }
        let mut ledger = TranscriptLedger {
            schema_version: TranscriptLedger::SCHEMA_VERSION,
            session_id: session_id.to_string(),
            accepted_event_count: accepted,
            latest_spans: map.into_values().collect(),
        };
        ledger.sort_latest_spans();
        ledger
    }

    fn fresh_fold_speaker_degrading(
        session_id: &str,
        events_prefix: &[DiarizationSpanRevision],
    ) -> SpeakerTimeline {
        let mut map = BTreeMap::new();
        let mut accepted = 0u64;
        for event in events_prefix {
            match fast_apply_speaker_event(&mut map, event.clone()) {
                Ok(()) => accepted += 1,
                Err(_) => break,
            }
        }
        let mut timeline = SpeakerTimeline {
            schema_version: SpeakerTimeline::SCHEMA_VERSION,
            session_id: session_id.to_string(),
            accepted_event_count: accepted,
            latest_spans: map.into_values().collect(),
        };
        timeline.sort_latest_spans();
        timeline
    }

    /// Snapshot the transcript ledger at `bound_ms`, propagating the first
    /// fold failure at or before this bound as an error. Mirrors the
    /// draft-time evidence-ledger loop in
    /// [`MaterializedProjectionState::replay_accepted_patches_with_history`],
    /// which abandons the whole patch on a fold error rather than degrading.
    fn transcript_snapshot_or_error(
        &mut self,
        bound_ms: u64,
    ) -> Result<TranscriptLedger, TranscriptLedgerError> {
        let target_len = self.transcript_prefix_len(bound_ms);
        self.advance_transcript(target_len);
        if let Some((poison_index, error)) = &self.transcript_poison
            && target_len > *poison_index
        {
            return Err(error.clone());
        }
        if target_len == self.transcript_applied_len {
            Ok(self.materialize_transcript_ledger())
        } else {
            debug_assert!(target_len < self.transcript_applied_len);
            Self::fresh_fold_transcript_or_error(
                self.session_id,
                &self.transcript_events[..target_len],
            )
        }
    }

    /// [`Self::transcript_snapshot_or_error`]'s speaker counterpart.
    /// Returns `Ok(None)` unconditionally when the speaker stream is not
    /// present, matching the old `speaker_history_present.then(...)` gate.
    fn speaker_snapshot_or_error(
        &mut self,
        bound_ms: u64,
    ) -> Result<Option<SpeakerTimeline>, SpeakerTimelineError> {
        if !self.speaker_history_present {
            return Ok(None);
        }
        let target_len = self.speaker_prefix_len(bound_ms);
        self.advance_speaker(target_len);
        if let Some((poison_index, error)) = &self.speaker_poison
            && target_len > *poison_index
        {
            return Err(error.clone());
        }
        if target_len == self.speaker_applied_len {
            Ok(Some(self.materialize_speaker_timeline()))
        } else {
            debug_assert!(target_len < self.speaker_applied_len);
            Self::fresh_fold_speaker_or_error(self.session_id, &self.speaker_events[..target_len])
                .map(Some)
        }
    }

    /// Snapshot the transcript ledger at `bound_ms`, degrading to whatever
    /// was already folded on a fold failure rather than erroring. Mirrors
    /// the old `replay_ledger_and_timeline_up_to`'s classify-bound
    /// reconstruction (audio-graph-f3d4 review fix): the authoritative
    /// validity check already happened in the evidence-bound fold, so a
    /// failure widening PAST that already-proven prefix degrades safely
    /// rather than invalidating the patch a second time.
    ///
    /// Once poisoned at index `p`, ANY `bound_ms` whose prefix reaches at or
    /// past `p` degrades to the SAME frozen state (a fresh rebuild that
    /// always dies at the identical `p` can never produce anything past
    /// it, no matter how large the requested bound), so this returns the
    /// live cursor snapshot directly with no repeated re-fold — only a
    /// genuine regression (`target_len` behind ground already covered)
    /// pays for an isolated fresh fold.
    fn transcript_snapshot_degrading(&mut self, bound_ms: u64) -> TranscriptLedger {
        let target_len = self.transcript_prefix_len(bound_ms);
        self.advance_transcript(target_len);
        if target_len >= self.transcript_applied_len {
            self.materialize_transcript_ledger()
        } else {
            Self::fresh_fold_transcript_degrading(
                self.session_id,
                &self.transcript_events[..target_len],
            )
        }
    }

    /// [`Self::transcript_snapshot_degrading`]'s speaker counterpart.
    /// Returns `None` unconditionally when the speaker stream is not
    /// present.
    fn speaker_snapshot_degrading(&mut self, bound_ms: u64) -> Option<SpeakerTimeline> {
        if !self.speaker_history_present {
            return None;
        }
        let target_len = self.speaker_prefix_len(bound_ms);
        self.advance_speaker(target_len);
        if target_len >= self.speaker_applied_len {
            Some(self.materialize_speaker_timeline())
        } else {
            Some(Self::fresh_fold_speaker_degrading(
                self.session_id,
                &self.speaker_events[..target_len],
            ))
        }
    }
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
    /// not the draft-time one (see [`LedgerHistory::transcript_snapshot_degrading`]).
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

        // Two independent forward cursors (audio-graph-927a) — see
        // `LedgerHistory`'s doc comment for why one shared cursor cannot
        // serve both bound sequences. `evidence_history` tracks the
        // draft-time `created_at_ms` bound; `classify_history` tracks the
        // classify-time `created_at_ms + generation_latency_ms` bound. Each
        // folds every transcript/speaker event AT MOST ONCE across the
        // whole replay instead of the old per-patch full re-fold.
        let mut evidence_history = LedgerHistory::new(
            &session_id,
            &transcript_events,
            &speaker_events,
            speaker_history_present,
        );
        let mut classify_history = LedgerHistory::new(
            &session_id,
            &transcript_events,
            &speaker_events,
            speaker_history_present,
        );

        'patches: for patch in patches {
            validation.checked_patch_count += 1;
            let evidence_ledger =
                match evidence_history.transcript_snapshot_or_error(patch.created_at_ms) {
                    Ok(ledger) => ledger,
                    Err(error) => {
                        validation.invalid_patch_count += 1;
                        validation.errors.push(
                            HistoricalProjectionValidationError::TranscriptReplay {
                                sequence: patch.sequence,
                                error,
                            },
                        );
                        continue 'patches;
                    }
                };

            let evidence_speaker_timeline =
                match evidence_history.speaker_snapshot_or_error(patch.created_at_ms) {
                    Ok(timeline) => timeline,
                    Err(error) => {
                        validation.invalid_patch_count += 1;
                        validation.errors.push(
                            HistoricalProjectionValidationError::SpeakerReplay {
                                sequence: patch.sequence,
                                error,
                            },
                        );
                        continue 'patches;
                    }
                };

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
            let ledger = classify_history.transcript_snapshot_degrading(classify_bound_ms);
            let speaker_timeline = classify_history.speaker_snapshot_degrading(classify_bound_ms);

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
    let attribution = speaker_timeline.map(crate::timeline::speaker_attribution_index);

    // audio-graph-cfa1: resolve the basis's FULL covered set (verbatim tail
    // plus, when compacted, the reconstructed-and-verified summarized
    // prefix) rather than only the exact-identity tail lookup this used to
    // do directly against `patch_basis.span_revisions`. An evidence anchor
    // can legitimately point at a span the rolling summary folded away —
    // `resolve_covered_events` is the one place that reconstruction is
    // allowed to happen, so evidence for older covered content keeps
    // resolving instead of silently downgrading to unsatisfied once a
    // session outgrows the hot window.
    patch_basis
        .resolve_covered_events(&ledger.latest_spans)
        .into_iter()
        .map(|mut event| {
            if let Some(attribution) = attribution.as_ref() {
                let keys = crate::timeline::candidate_keys(
                    event.transcript_segment_id.as_deref(),
                    event.span_id.as_str(),
                );
                if let Some(winner) = keys.iter().find_map(|key| attribution.get(key.as_str())) {
                    event.speaker_id = winner.speaker_id.clone();
                    event.speaker_label = winner.speaker_label.clone();
                }
            }
            event
        })
        .collect()
}

/// Resolve a node id referenced by a graph operation (edge endpoint,
/// invalidate/merge/split target, ...) to the id it should ACTUALLY be
/// looked up under, in three tiers (audio-graph-e700 replay-compatibility
/// fix):
///
/// 1. THIS patch's same-patch remap table (`id_overrides`, see
///    `MaterializedGraph::apply_patch`'s doc comment) — always wins when
///    present, since it reflects what THIS patch's own upsert JUST did,
///    which must take priority over anything older.
/// 2. A literal row under `id` in `graph.nodes` — active OR already
///    invalidated. Once an id has its own row it never again means anything
///    else, even if it is ALSO, separately, the source of a stale entry in
///    `graph.id_aliases` recorded before that row existed. This is what
///    keeps a legitimate stable-id reuse across ticks (tier 1 of
///    `upsert_node`) and the disclosed "first owner wins" semantics after a
///    same-id-different-name collision working exactly as before.
/// 3. `graph.id_aliases`, followed to its end (bounded by the map's own
///    size, so a cycle can never loop forever): the persistent, cross-patch
///    record of every raw id `upsert_node` ever redirected to a DIFFERENT
///    final id. This is the tier that makes a pre-e700 accepted log (or a
///    fresh session) replay/apply without error when an EARLIER patch's
///    fuzzy name merge absorbed a raw id that has no row of its own, and a
///    LATER, separate patch references that exact raw id.
///
/// Falls back to `id` verbatim when none of the above apply — the common
/// case, and correct: an id that was never upserted at all should fail
/// `has_active_node`/`active_node_index` exactly as it always has.
fn resolve_graph_node_id(
    graph: &MaterializedGraph,
    id_overrides: &BTreeMap<String, String>,
    id: &str,
) -> String {
    if let Some(overridden) = id_overrides.get(id) {
        return overridden.clone();
    }
    if graph.nodes.iter().any(|node| node.id == id) {
        return id.to_string();
    }
    let mut current = id.to_string();
    for _ in 0..=graph.id_aliases.len() {
        let Some(aliased) = graph.id_aliases.get(&current) else {
            break;
        };
        if aliased == &current {
            break;
        }
        current = aliased.clone();
        if graph.nodes.iter().any(|node| node.id == current) {
            break;
        }
    }
    current
}

/// Minimum shared-prefix length ratio (shorter/longer, over the
/// alphanumeric-only "fuzzy core" — [`fuzzy_entity_name_core`]) for two
/// entity names to be treated as the same real-world entity purely from
/// their spelling (seed audio-graph-e700 sub-fix 3).
///
/// Deliberately NOT the generic Jaro-Winkler threshold
/// `graph::temporal::TemporalKnowledgeGraph::resolve_entity` uses — that
/// algorithm is reused elsewhere in this codebase ONLY for
/// human-supervised merges: every unsupervised production caller of
/// `supersede_entity` (diarization retcon, projection-adjacent speaker
/// merges) passes `threshold = 1.0` (exact-only); the one caller that
/// accepts a lower threshold takes it from an explicit user action
/// (`merge_graph_entities`), never runs it automatically. Plain Jaro-Winkler
/// on full names scores dangerously high for exactly the pattern this
/// ticket's field evidence describes — generic model-invented labels that
/// share a long common stem and differ only in a trailing enumerator:
/// measured `jaro_winkler("task 1", "task 2") = 0.933`,
/// `jaro_winkler("decision 1", "decision 2") = 0.960`,
/// `jaro_winkler("provider a", "provider b") = 0.960` — ALL at or above the
/// `jaro_winkler("postgres", "postgresql") = 0.960` pair this sub-fix exists
/// to catch. One global similarity threshold cannot separate those two
/// classes.
///
/// The prefix+ratio rule below can, because same-length differing-suffix
/// names (the dangerous class above) can never satisfy a literal prefix
/// relationship on their alphanumeric core, while a genuine
/// abbreviation/extension can: `"postgres"` (core len 8) is a true prefix of
/// `"postgresql"` (core len 10), ratio 0.8; `"gpt-4"` (core `"gpt4"`, len 4)
/// is a true prefix of `"gpt-4o"` (core `"gpt4o"`, len 5), ratio 0.8. 0.6 is
/// calibrated so `"react"` / `"react native"` (core lens 5 / 11, ratio
/// 0.45 — arguably different products) does NOT merge, while both measured
/// examples above (ratio 0.8) do. `"acme corp"` / `"acme corporation"`
/// (ratio 0.53) also stays unmerged at this threshold — an accepted false
/// negative (two names stay separate that a human might consider the same
/// org), which is the safe direction to be wrong in for an AUTOMATIC,
/// unsupervised merge.
const FUZZY_ENTITY_NAME_MIN_PREFIX_RATIO: f64 = 0.6;

/// Case/whitespace-normalized name for EXACT-match comparisons: trim,
/// collapse internal whitespace runs to a single space, lowercase.
/// Punctuation is preserved at this tier — see [`fuzzy_entity_name_core`]
/// for the looser tier that also strips it.
fn normalized_entity_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Alphanumeric-only "core" of a name, used ONLY by the prefix/ratio fuzzy
/// tier in [`fuzzy_entity_name_match`]. Stripping whitespace/punctuation
/// entirely (not just collapsing it) is what makes `"OpenAI"` and `"Open
/// AI"` resolve to the identical core `"openai"` — a legitimate
/// near-duplicate this ticket's sub-fix 3 targets — without needing the
/// prefix/ratio check at all.
fn fuzzy_entity_name_core(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// True when `a` and `b` name the SAME real-world entity closely enough to
/// merge automatically at projection-graph ingest (seed audio-graph-e700 sub-
/// fixes 2 and 3: both the "same id, name drifted" tier and the "different
/// id, near-duplicate name" tier in `MaterializedGraph::upsert_node` resolve
/// through this one function, so they can never disagree about what counts
/// as a match). See [`FUZZY_ENTITY_NAME_MIN_PREFIX_RATIO`]'s doc comment for
/// why this is a deliberately narrower rule than generic Jaro-Winkler
/// similarity.
fn fuzzy_entity_name_match(a: &str, b: &str) -> bool {
    if normalized_entity_name(a) == normalized_entity_name(b) {
        return true;
    }
    let (core_a, core_b) = (fuzzy_entity_name_core(a), fuzzy_entity_name_core(b));
    if core_a.is_empty() || core_b.is_empty() {
        return false;
    }
    if core_a == core_b {
        return true;
    }
    // Length in CHARS (Unicode scalar values), not bytes: the TS mirror
    // (`fuzzyEntityNameCore`/`fuzzyEntityNameMatch` in
    // `src/utils/materializedGraph.ts`) counts Unicode code points via
    // `Array.from`, which has no byte-length equivalent in JS. Using byte
    // length here would silently disagree with the frontend for any
    // non-ASCII name — e.g. multi-byte-per-character scripts (CJK, Cyrillic,
    // accented Latin) would get a DIFFERENT ratio in Rust than in TS for the
    // identical pair of names, so the live incremental view and a replayed
    // session could resolve node identity differently (audio-graph-e700).
    let (len_a, len_b) = (core_a.chars().count(), core_b.chars().count());
    let (shorter, longer, len_shorter, len_longer) = if len_a <= len_b {
        (&core_a, &core_b, len_a, len_b)
    } else {
        (&core_b, &core_a, len_b, len_a)
    };
    if !longer.starts_with(shorter.as_str()) {
        return false;
    }
    (len_shorter as f64 / len_longer as f64) >= FUZZY_ENTITY_NAME_MIN_PREFIX_RATIO
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

    // -----------------------------------------------------------------------
    // `session_relative_timestamp` (audio-graph-4b52): converts a manual
    // write's wall-clock `created_at_ms` into the session-relative-seconds
    // domain `TemporalKnowledgeGraph::process_extraction` expects, using this
    // ledger's own (start_time, received_at_ms) pairs as the anchor.
    // -----------------------------------------------------------------------

    #[test]
    fn session_relative_timestamp_uses_exact_segment_match_as_anchor() {
        let mut ledger = TranscriptLedger::new("session-1");
        // asr_payload's fixed fields: transcript_segment_id "segment-1",
        // start_time 1.0, received_at_ms 1_700_000_000_000 + revision_number.
        ledger
            .apply_event(TranscriptEvent::from(asr_payload("span-1", 2, "hello")))
            .expect("seed matching span");
        // A decoy span with a DIFFERENT segment id and a wildly different
        // anchor, sorted to be `latest_spans[0]` (smaller start_time — the
        // ledger sorts ascending by start_time; see `sort_latest_spans`), so
        // the test only passes if the exact match is preferred over
        // "whichever span happens to be first."
        let mut decoy = asr_payload("span-2", 1, "decoy");
        decoy.transcript_segment_id = Some("other-segment".to_string());
        decoy.start_time = -500.0;
        decoy.received_at_ms = 1_800_000_000_000;
        ledger
            .apply_event(TranscriptEvent::from(decoy))
            .expect("seed decoy span");
        assert_eq!(
            ledger
                .latest_spans
                .first()
                .and_then(|e| e.transcript_segment_id.as_deref()),
            Some("other-segment"),
            "sanity: the decoy must sort first so this test actually exercises \
             the exact-match preference, not agree with `.first()` by luck"
        );

        // created_at_ms 500ms after the matching span's received_at_ms
        // (1_700_000_000_002) should land at start_time (1.0) + 0.5s.
        let timestamp = ledger.session_relative_timestamp("segment-1", 1_700_000_000_002 + 500);
        assert!(
            (timestamp - 1.5).abs() < 1e-9,
            "expected 1.5s (matching span's start_time + 0.5s offset), got {timestamp}"
        );
    }

    #[test]
    fn session_relative_timestamp_falls_back_to_any_span_when_no_exact_match() {
        let mut ledger = TranscriptLedger::new("session-1");
        ledger
            .apply_event(TranscriptEvent::from(asr_payload("span-1", 2, "hello")))
            .expect("seed span");

        // "missing-segment" never appears in the ledger; fall back to the
        // one span present (start_time 1.0, received_at_ms 1_700_000_000_002).
        let timestamp =
            ledger.session_relative_timestamp("missing-segment", 1_700_000_000_002 + 250);
        assert!(
            (timestamp - 1.25).abs() < 1e-9,
            "expected fallback-anchored 1.25s, got {timestamp}"
        );
    }

    #[test]
    fn session_relative_timestamp_is_zero_when_ledger_has_no_spans() {
        let ledger = TranscriptLedger::new("session-1");
        assert_eq!(
            ledger.session_relative_timestamp("any-segment", 1_700_000_000_000),
            0.0,
            "an empty ledger has no anchor to convert against; must stay total, not panic"
        );
    }

    /// A partial-revision producer can leave `transcript_segment_id` unset
    /// (`AsrSpanRevisionPayload::transcript_segment_id: None`), so a caller's
    /// `source_segment_id` may only resolve against the span's immutable
    /// `span_id` — mirroring `live_assist_evidence_anchor`'s existing
    /// OR-match in `commands.rs`. Before this OR-match, this case fell all
    /// the way through to the `.first()` fallback even when the exact span
    /// WAS present in the ledger.
    #[test]
    fn session_relative_timestamp_matches_on_span_id_when_transcript_segment_id_is_none() {
        let mut ledger = TranscriptLedger::new("session-1");
        let mut span_id_only = asr_payload("span-1", 2, "hello");
        span_id_only.transcript_segment_id = None;
        ledger
            .apply_event(TranscriptEvent::from(span_id_only))
            .expect("seed span-id-only span");

        // A decoy sorted ahead of the real span, so this only passes if the
        // exact `span_id` match is preferred over `.first()`.
        let mut decoy = asr_payload("span-2", 1, "decoy");
        decoy.transcript_segment_id = None;
        decoy.start_time = -500.0;
        decoy.received_at_ms = 1_800_000_000_000;
        ledger
            .apply_event(TranscriptEvent::from(decoy))
            .expect("seed decoy span");
        assert_eq!(
            ledger.latest_spans.first().map(|e| e.span_id.as_str()),
            Some("span-2"),
            "sanity: the decoy must sort first so this test actually exercises \
             the span_id match, not agree with `.first()` by luck"
        );

        let timestamp = ledger.session_relative_timestamp("span-1", 1_700_000_000_002 + 500);
        assert!(
            (timestamp - 1.5).abs() < 1e-9,
            "expected span_id-matched anchor (start_time 1.0 + 0.5s offset), got {timestamp}"
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

    /// audio-graph-cfa1 (post-adversarial-review fix): before this fix,
    /// `classify_basis_currency`'s `CoveredSpanOrderMismatch` check was
    /// skipped ENTIRELY whenever `basis.covered_prefix.is_some()`, so a
    /// hand-corrupted permutation of a COMPACTED basis's tail
    /// `span_revisions` vector (same identities/revisions, different
    /// positions) passed classification undetected — neither the per-id
    /// `basis_spans` map (order-independent) nor the whole-covered-set
    /// `transcript_hash` check (which canonicalizes by chronological order,
    /// not vector order) is sensitive to on-disk vector order. This pins the
    /// fix: the order check now runs against the exposed tail
    /// unconditionally, mirroring
    /// `basis_currency_rejects_deletion_reorder_and_non_tail_insertion`'s
    /// legacy-basis reorder coverage above for the compacted case.
    #[test]
    fn basis_currency_rejects_a_reordered_tail_even_when_the_basis_is_compacted() {
        let events: Vec<TranscriptEvent> = (0..(ROLLING_SUMMARY_HOT_WINDOW_TURNS + 2))
            .map(|i| {
                let mut event = TranscriptEvent::from(asr_payload(&format!("span-{i}"), 1, "turn"));
                event.start_time = i as f64;
                event.end_time = i as f64 + 0.5;
                event
            })
            .collect();
        let ledger = TranscriptLedger::replay("session-1", events).expect("ledger replay");
        let basis = ledger.current_basis();
        assert!(
            basis.covered_prefix.is_some(),
            "session must exceed the hot window for this test to exercise the compacted path"
        );
        assert!(
            basis.span_revisions.len() >= 2,
            "the verbatim tail must have at least two entries to permute"
        );

        let mut reordered = basis.clone();
        reordered.span_revisions.swap(0, 1);
        match ledger.classify_basis_currency(&reordered, None) {
            BasisCurrency::Revised(ProjectionBasisStaleness::CoveredSpanOrderMismatch {
                ..
            }) => {}
            other => panic!(
                "expected CoveredSpanOrderMismatch for a reordered compacted-basis tail, got \
                 {other:?}"
            ),
        }
    }

    /// audio-graph-cfa1: a compacted basis exposes its summarized-away
    /// prefix only as a `(span_count, content_hash)` digest. This proves the
    /// fix above doesn't quietly weaken detection of a revision or deletion
    /// INSIDE that prefix (as opposed to the tail, covered by the test
    /// above): both must still classify `Revised`, whether via
    /// `reconstruct_verified_covered_prefix`'s digest check dropping the
    /// unverifiable prefix (surfacing as `CoveredSpanCountMismatch`) or, if
    /// that inner layer were ever bypassed, the outer whole-covered-set
    /// `transcript_hash` check (`TranscriptHashMismatch`) —
    /// `reconstruct_verified_covered_prefix_rejects_a_content_mismatch`
    /// below pins that inner layer directly. Never `Current` or
    /// `AppendOnlyStale`.
    #[test]
    fn basis_currency_rejects_revision_and_deletion_inside_a_compacted_prefix() {
        fn build_events() -> Vec<TranscriptEvent> {
            (0..(ROLLING_SUMMARY_HOT_WINDOW_TURNS + 2))
                .map(|i| {
                    let mut event =
                        TranscriptEvent::from(asr_payload(&format!("span-{i}"), 1, "turn"));
                    event.start_time = i as f64;
                    event.end_time = i as f64 + 0.5;
                    event
                })
                .collect()
        }
        let baseline_ledger =
            TranscriptLedger::replay("session-1", build_events()).expect("baseline ledger");
        let basis = baseline_ledger.current_basis();
        assert!(
            basis.covered_prefix.is_some(),
            "session must exceed the hot window for this test to exercise the compacted path"
        );

        // `span-0` sits chronologically first — well inside the summarized
        // prefix, since the tail is only the last `ROLLING_SUMMARY_HOT_WINDOW_TURNS`
        // spans.
        let mut revised_events = build_events();
        revised_events[0] = {
            let mut event = TranscriptEvent::from(asr_payload("span-0", 2, "turn, revised"));
            event.start_time = 0.0;
            event.end_time = 0.5;
            event
        };
        let revised_ledger =
            TranscriptLedger::replay("session-1", revised_events).expect("revised ledger");
        assert!(
            matches!(
                revised_ledger.classify_basis_currency(&basis, None),
                BasisCurrency::Revised(_)
            ),
            "revising a span folded into the summarized prefix must still classify Revised"
        );

        let mut deleted_events = build_events();
        deleted_events.remove(0);
        let deleted_ledger = TranscriptLedger::replay("session-1", deleted_events)
            .expect("ledger missing a prefix span");
        assert!(
            matches!(
                deleted_ledger.classify_basis_currency(&basis, None),
                BasisCurrency::Revised(_)
            ),
            "deleting a span folded into the summarized prefix must still classify Revised"
        );
    }

    /// audio-graph-cfa1 (post-adversarial-review fix): direct pin of the
    /// digest-verification layer itself, independent of the outer
    /// whole-covered-set `transcript_hash` check
    /// `classify_basis_currency` also runs. A mutation probe that replaced
    /// this comparison with an unconditional `Some(reconstructed)` passed
    /// the full test suite before this test existed, because the outer hash
    /// check alone still happens to catch a revision — this test pins the
    /// inner layer directly so that defense-in-depth stays real rather than
    /// just accidentally true.
    #[test]
    fn reconstruct_verified_covered_prefix_rejects_a_content_mismatch() {
        let mut span_a = TranscriptEvent::from(asr_payload("span-a", 1, "prefix text a"));
        span_a.start_time = 0.0;
        span_a.end_time = 0.5;
        let mut span_b = TranscriptEvent::from(asr_payload("span-b", 1, "prefix text b"));
        span_b.start_time = 1.0;
        span_b.end_time = 1.5;
        let candidates = vec![span_a.clone(), span_b.clone()];
        let tail_ids: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();

        let genuine_digest = CoveredPrefixDigest {
            span_count: 2,
            content_hash: transcript_events_hash_v1(&[span_a.clone(), span_b.clone()]),
        };
        assert_eq!(
            reconstruct_verified_covered_prefix(&candidates, &tail_ids, &genuine_digest),
            Some(vec![span_a.clone(), span_b.clone()])
        );

        let mut tampered_span_a = span_a.clone();
        tampered_span_a.text = "tampered".to_string();
        let tampered_candidates = vec![tampered_span_a, span_b.clone()];
        assert_eq!(
            reconstruct_verified_covered_prefix(&tampered_candidates, &tail_ids, &genuine_digest),
            None,
            "a content mismatch inside the prefix must be rejected, never returned as if verified"
        );

        let short_digest = CoveredPrefixDigest {
            span_count: 5,
            content_hash: genuine_digest.content_hash.clone(),
        };
        assert_eq!(
            reconstruct_verified_covered_prefix(&candidates, &tail_ids, &short_digest),
            None,
            "fewer eligible candidates than the digest's recorded span_count must be rejected, \
             never returned as a partial/unverified set"
        );
    }

    /// audio-graph-cfa1: `TranscriptLedger::current_basis` (used by the
    /// live-assist approval path, `commands.rs`'s
    /// `approved_agent_projection_patch`) includes partial/ineligible events,
    /// unlike `current_projection_basis`'s eligible-only view.
    /// `resolve_covered_events` tries the eligibility-filtered candidate
    /// universe FIRST and falls back to the raw ledger only when that
    /// fails — this pins the fallback branch against the one basis shape
    /// that actually needs it (contra a prior review pass's belief that
    /// `current_basis` had no production caller; `approve_agent_proposal`
    /// is a live `#[tauri::command]`).
    #[test]
    fn resolve_covered_events_falls_back_to_the_full_ledger_when_a_partial_lives_in_the_summarized_prefix()
     {
        let mut ledger = TranscriptLedger::new("session-1");
        let mut partial = TranscriptEvent::from(asr_payload("span-0", 1, "still forming"));
        partial.start_time = 0.0;
        partial.end_time = 0.5;
        ledger.apply_event(partial).expect("partial span accepted");

        for i in 1..=(ROLLING_SUMMARY_HOT_WINDOW_TURNS + 2) {
            let mut event =
                TranscriptEvent::from(asr_payload(&format!("span-{i}"), 2, "final turn"));
            event.start_time = i as f64;
            event.end_time = i as f64 + 0.5;
            ledger.apply_event(event).expect("final span accepted");
        }

        let basis = ledger.current_basis();
        assert!(
            basis.covered_prefix.is_some(),
            "session must exceed the hot window for this test to exercise the compacted path"
        );

        let eligible_only_count = ledger
            .latest_spans
            .iter()
            .filter(|event| projection_event_is_eligible(event))
            .count();
        assert!(
            eligible_only_count < basis.covered_span_count(),
            "the eligibility-filtered candidate universe must be short by exactly the partial \
             span for this test to genuinely exercise resolve_covered_events' all-events \
             fallback, not just have its first attempt succeed anyway"
        );

        let covered = basis.resolve_covered_events(&ledger.latest_spans);
        assert_eq!(
            covered.len(),
            basis.covered_span_count(),
            "resolve_covered_events must recover the partial-bearing prefix via its all-events \
             fallback rather than silently dropping it because the eligible-only attempt alone \
             cannot reproduce it"
        );
        assert!(
            covered.iter().any(|event| event.span_id == "span-0"),
            "the partial event folded into the summarized prefix must still resolve"
        );
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
        // audio-graph-cfa1: 8 covered spans is past `ROLLING_SUMMARY_HOT_WINDOW_TURNS`
        // (6), so `span_revisions` alone (the verbatim tail) no longer names
        // every covered span — `covered_span_count()` is the compaction-aware
        // total (tail + summarized prefix) that still equals `fixtures.len()`.
        let basis = ledger.current_basis();
        assert_eq!(basis.covered_span_count(), fixtures.len());

        // audio-graph-cfa1 (post-adversarial-review fix): pin the exact
        // tail/prefix SPLIT, not just the total. A `hot_window_len`/
        // `prefix_len` off-by-one in the constructor would leave
        // `covered_span_count()` unchanged (both sides still sum to the same
        // total) but would leave one extra or missing span exposed in the
        // verbatim tail versus opaque in the prefix — invisible to every
        // other assertion in this test.
        let prefix = basis
            .covered_prefix
            .as_ref()
            .expect("8 covered spans must exceed the hot window and produce a prefix");
        assert_eq!(
            basis.span_revisions.len(),
            ROLLING_SUMMARY_HOT_WINDOW_TURNS,
            "the verbatim tail must be exactly the hot window size once the covered set exceeds \
             it"
        );
        assert_eq!(
            prefix.span_count,
            fixtures.len() - ROLLING_SUMMARY_HOT_WINDOW_TURNS,
            "the summarized prefix must carry exactly the spans NOT in the verbatim tail"
        );
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
            covered_prefix: None,
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
        // absent on disk → deserializes to None. `span_revisions` here must
        // be the FULL covered set (audio-graph-cfa1 predates compaction too:
        // a genuine legacy record never had a `covered_prefix`, so it named
        // every covered span individually) — `current_basis.span_revisions`
        // alone is now only the compacted tail and would silently mismatch
        // `current_basis.transcript_hash`, which still covers everything.
        let legacy_json = serde_json::json!({
            "span_revisions": latest_transcript_events(&ledger.latest_spans)
                .iter()
                .map(|event| ProjectionBasisSpan {
                    span_id: event.span_id.clone(),
                    revision_number: event.revision_number,
                })
                .collect::<Vec<_>>(),
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
                heading_level: None,
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
            basis_currency_at_apply: None,
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
                covered_prefix: None,
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
                    heading_level: None,
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
            basis_currency_at_apply: None,
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

    /// audio-graph-cfa1 deliverable (c): the two production `log::debug!`
    /// sites that reach a `ProjectionBasis` (`speech/mod.rs`'s
    /// `observe_asr_revision`, via `ProjectionSchedulersObservation`, and the
    /// scheduler-completion log, via `ProjectionSchedulerDecision::StartJob`)
    /// must never dump the full span-revision vector or prefix digest
    /// content — that pattern was 95% of a 46MB field log, single lines up
    /// to 80KB. Neither log site has ANY bespoke redaction code; both derive
    /// `Debug` on `ProjectionJob`/`ProjectionSchedulerDecision` and recurse
    /// into `ProjectionBasis`'s own manual `Debug` impl for free.
    #[test]
    fn projection_basis_debug_stays_bounded_and_never_dumps_span_identities_or_prefix_digest() {
        let many_span_revisions: Vec<ProjectionBasisSpan> = (0..50)
            .map(|i| ProjectionBasisSpan {
                span_id: format!("span-id-that-should-never-appear-in-a-debug-log-{i}"),
                revision_number: i,
            })
            .collect();
        let basis = ProjectionBasis {
            span_revisions: many_span_revisions,
            covered_prefix: Some(CoveredPrefixDigest {
                span_count: 927,
                content_hash: "fnv1a64:deadbeefcafef00d".to_string(),
            }),
            diarization_span_revisions: Vec::new(),
            transcript_hash: "fnv1a64:0123456789abcdef".to_string(),
            summarized_through_revision: Some(12),
        };

        let debug = format!("{basis:?}");
        assert!(
            !debug.contains("span-id-that-should-never-appear-in-a-debug-log"),
            "Debug output must never contain a raw span identity: {debug}"
        );
        assert!(
            debug.len() < 400,
            "Debug output should stay small and constant-sized regardless of how many spans \
             this basis covers, got {} bytes: {debug}",
            debug.len()
        );
        assert!(debug.contains("span_revisions_count"));
        assert!(debug.contains("50"));
        assert!(debug.contains("927"));
        assert!(debug.contains("12"));

        let job = ProjectionJob {
            id: "projection:session-1:notes:1".to_string(),
            session_id: "session-1".to_string(),
            kind: ProjectionKind::Notes,
            basis: basis.clone(),
            priority: ProjectionPriority::Realtime,
            queued_at_ms: 10,
        };
        let job_debug = format!("{job:?}");
        assert!(!job_debug.contains("span-id-that-should-never-appear-in-a-debug-log"));

        let decision = crate::projection_scheduler::ProjectionSchedulerDecision::StartJob { job };
        let decision_debug = format!("{decision:?}");
        assert!(!decision_debug.contains("span-id-that-should-never-appear-in-a-debug-log"));
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
                heading_level: None,
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
            basis_currency_at_apply: None,
            created_at_ms: 1_700_000_000_100 + sequence,
        }
    }

    /// [`notes_patch`] with an explicit `heading_level` (audio-graph-a6b5 W1
    /// fixtures) — the un-suffixed helper stays `heading_level: None` so its
    /// ~30 existing call sites keep testing the pre-W1 shape unchanged.
    fn notes_patch_with_heading_level(
        sequence: u64,
        id: &str,
        title: &str,
        body: &str,
        heading_level: Option<u8>,
    ) -> ProjectionPatch {
        let mut patch = notes_patch(sequence, id, title, body);
        if let Some(ProjectionOperation::UpsertNote {
            heading_level: op_heading_level,
            ..
        }) = patch.operations.first_mut()
        {
            *op_heading_level = heading_level;
        }
        patch
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
            basis_currency_at_apply: None,
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
            basis_currency_at_apply: None,
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
            basis_currency_at_apply: None,
            created_at_ms: 1_700_000_000_105,
            route: None,
        };
        notes.apply_patch(&delete, None).expect("delete patch");

        assert_eq!(notes.last_sequence, 5);
        assert_eq!(notes.notes.len(), 1);
        assert_eq!(notes.notes[0].id, "note-2");
    }

    // ----- audio-graph-a6b5 W1: `heading_level` contract field ------------

    /// Field round-trip through the FULL patch -> materialize -> persist ->
    /// reload cycle: an `UpsertNote` operation carrying `heading_level`
    /// materializes onto `MaterializedNote`, survives a JSON persist+reload
    /// (simulating the on-disk materialized-notes snapshot) unchanged, and a
    /// later full-replace `UpsertNote` for the SAME id with `heading_level:
    /// None` wipes it — full-replace semantics, unchanged by this field.
    #[test]
    fn heading_level_round_trips_through_patch_materialize_persist_reload_cycle() {
        let mut notes = MaterializedNotes::new("session-1");
        notes
            .apply_patch(
                &notes_patch_with_heading_level(
                    1,
                    "note-1",
                    "Decision",
                    "Ship the event-sourced projection model.",
                    Some(3),
                ),
                None,
            )
            .expect("insert patch with heading_level");

        assert_eq!(
            notes.notes.first().and_then(|note| note.heading_level),
            Some(3)
        );

        // Persist: serialize the whole materialized-notes snapshot exactly
        // like the on-disk artifact would.
        let persisted_json = serde_json::to_string(&notes).expect("persist materialized notes");
        assert!(
            persisted_json.contains("\"heading_level\":3"),
            "a Some(heading_level) must actually reach the persisted JSON, got: {persisted_json}"
        );

        // Reload: deserialize the persisted artifact back.
        let reloaded: MaterializedNotes =
            serde_json::from_str(&persisted_json).expect("reload materialized notes");
        assert_eq!(
            reloaded.notes.first().and_then(|note| note.heading_level),
            Some(3),
            "heading_level must survive a full persist/reload cycle unchanged"
        );

        // Full-replace: a later UpsertNote for the SAME id with no
        // heading_level wipes the field exactly like it wipes every other
        // field — there is no incremental/partial merge.
        let mut notes = reloaded;
        notes
            .apply_patch(
                &notes_patch(2, "note-1", "Decision", "Ship materialized notes."),
                None,
            )
            .expect("full-replace upsert");
        assert_eq!(
            notes.notes.first().and_then(|note| note.heading_level),
            None,
            "a later full-replace UpsertNote with no heading_level must wipe the prior value"
        );
    }

    /// Replay fixture: a pre-W1 `projection_patches` log (no `heading_level`
    /// anywhere, arbitrary body text — no normalization assumed) replays to
    /// a BYTE-IDENTICAL `MaterializedNotes` before and after W1 — checked at
    /// the serde level (the persisted JSON shape), not just at the Rust
    /// value level, since `skip_serializing_if` is exactly what keeps a
    /// `None` field from resurrecting on the wire.
    #[test]
    fn pre_w1_notes_log_replays_byte_identical_materialized_notes() {
        let pre_w1_log = [
            notes_patch(
                1,
                "note-1",
                "Decision",
                "Ship the event-sourced projection model.",
            ),
            notes_patch(2, "note-2", "Follow-up", "Keep stable note ids."),
            notes_patch(
                3,
                "note-1",
                "Decision",
                "Ship the event-sourced projection model, v2.",
            ),
        ];

        let replayed =
            MaterializedProjectionState::replay_accepted_patches("session-pre-w1", pre_w1_log)
                .expect("pre-W1 notes log replays");

        assert_eq!(replayed.notes.notes.len(), 2);
        for note in &replayed.notes.notes {
            assert_eq!(
                note.heading_level, None,
                "a pre-W1 log must never materialize a fabricated heading_level"
            );
        }

        let persisted = serde_json::to_string(&replayed.notes).expect("serialize replayed notes");
        assert!(
            !persisted.contains("heading_level"),
            "a pre-W1 replay's persisted JSON must be byte-identical to what pre-W1 code \
             would have written — no heading_level key may appear anywhere: {persisted}"
        );
    }

    /// Ticket W3 (audio-graph-a6b5): `ProjectionPatch::basis_currency_at_apply`'s
    /// wire shape. Pins BOTH directions — same additive/`skip_serializing_if`
    /// contract W1's `heading_level` pins above
    /// (`pre_w1_notes_log_replays_byte_identical_materialized_notes`), applied
    /// to `ProjectionPatch` instead of `MaterializedNote`:
    /// - `Some(Current)` serializes as a REAL nested tagged object
    ///   (`{"type":"current"}`, snake_case) — `AppliedBasisCurrency` is
    ///   `#[serde(tag = "type", rename_all = "snake_case")]`, never a bare
    ///   string.
    /// - `None` (every patch this app ever WRITES to the canonical log — see
    ///   the field's doc comment) leaves the key ABSENT entirely, not `null`.
    /// - A pre-W3 JSON object (the key never appears at all, simulating a
    ///   record written before this field existed) still deserializes
    ///   cleanly to `None`.
    #[test]
    fn basis_currency_at_apply_wire_shape_is_additive_and_skips_when_absent() {
        let mut with_currency = notes_patch(1, "note-1", "Decision", "Ship it.");
        with_currency.basis_currency_at_apply = Some(AppliedBasisCurrency::Current);

        let json = serde_json::to_string(&with_currency).expect("serialize patch");
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            value["basis_currency_at_apply"],
            serde_json::json!({ "type": "current" }),
            "Some(Current) must serialize as a nested tagged object, not a bare string: {json}"
        );

        let without_currency = notes_patch(2, "note-2", "Decision", "Ship it too.");
        assert_eq!(without_currency.basis_currency_at_apply, None);
        let json_without = serde_json::to_string(&without_currency).expect("serialize patch");
        assert!(
            !json_without.contains("basis_currency_at_apply"),
            "None must leave the key ABSENT, not null (skip_serializing_if): {json_without}"
        );

        // A patch from before this field existed (no key at all) must still
        // deserialize cleanly, defaulting to `None` — the same
        // `#[serde(default)]` contract W1's `heading_level` relies on.
        let reloaded: ProjectionPatch =
            serde_json::from_str(&json_without).expect("pre-W3 patch shape deserializes");
        assert_eq!(reloaded.basis_currency_at_apply, None);
    }

    /// Replay fixture: a MIXED log — some `upsert_note` operations from
    /// before W1 (no `heading_level`), interleaved with post-W1 operations
    /// that DO carry `heading_level` — replays without error, and each
    /// note's final materialized `heading_level` matches whichever operation
    /// last touched it (full-replace, not merge).
    #[test]
    fn mixed_old_and_new_style_notes_log_replays_without_error() {
        let mixed_log = [
            // Pre-W1 style: no heading_level.
            notes_patch(
                1,
                "note-1",
                "Decision",
                "Ship the event-sourced projection model.",
            ),
            // Post-W1 style: a genuinely new section with heading_level.
            notes_patch_with_heading_level(
                2,
                "note-2",
                "Follow-up",
                "Keep stable note ids.",
                Some(2),
            ),
            // Post-W1 style: refines the PRE-W1 note, now asserting structure.
            notes_patch_with_heading_level(
                3,
                "note-1",
                "Decision",
                "Ship materialized notes.",
                Some(3),
            ),
        ];

        let replayed =
            MaterializedProjectionState::replay_accepted_patches("session-mixed", mixed_log)
                .expect("mixed old/new-style notes log replays without error");

        assert_eq!(replayed.notes.notes.len(), 2);
        let note_1 = replayed
            .notes
            .notes
            .iter()
            .find(|note| note.id == "note-1")
            .expect("note-1 present");
        let note_2 = replayed
            .notes
            .notes
            .iter()
            .find(|note| note.id == "note-2")
            .expect("note-2 present");
        assert_eq!(
            note_1.heading_level,
            Some(3),
            "note-1's LATEST operation set heading_level — full-replace, not merge"
        );
        assert_eq!(note_2.heading_level, Some(2));
    }

    /// THE load-bearing replay-vs-materialization seam, pinned at the ONLY
    /// layer that can actually catch a regression here (ADR-0045): unlike
    /// `hostile_body_in_an_old_accepted_log_replays_untouched_never_normalized`
    /// in `projection_llm.rs` (which only proves `serde_json::from_str`
    /// doesn't rewrite a body — vacuously true, since serde can never call a
    /// normalizer), this test drives the SAME hostile body plus an
    /// out-of-range `heading_level` through the REAL reachable replay path,
    /// `MaterializedProjectionState::replay_accepted_patches` ->
    /// `apply_replayed_patch` -> `MaterializedNotes::apply_patch` ->
    /// `upsert_note`, and asserts the MATERIALIZED note comes out
    /// byte-identical to the log: body untouched (no bullet-marker rewrite,
    /// no heading-marker strip) and `heading_level` unclamped. A mutation
    /// that calls `projection_llm::normalize_doc_body` or clamps
    /// `heading_level` inside `upsert_note` — i.e. re-derives materialized
    /// state using TODAY's normalization rules instead of the rules in force
    /// when the patch was accepted — fails this test even though every other
    /// W1 replay fixture (plain-text bodies, in-range heading levels) stays
    /// green under that exact mutation.
    #[test]
    fn hostile_body_and_out_of_range_heading_level_replay_materialized_untouched_never_normalized()
    {
        // Same fixture text `projection_llm.rs`'s
        // `hostile_body_in_an_old_accepted_log_replays_untouched_never_normalized`
        // already uses (fixture-text reuse constraint) — carries a `*`
        // bullet marker, a raw `<script>` tag, and a stray `#` heading
        // marker, all of which `normalize_doc_body` WOULD rewrite if it ever
        // ran on this path.
        let hostile_body = "* unnormalized bullet\n<script>alert(1)</script>\n#stray heading";
        let old_accepted_log = [notes_patch_with_heading_level(
            1,
            "note:old-hostile",
            "Old accepted note",
            hostile_body,
            Some(99), // out of the normalizer's 2..=4 clamp range
        )];

        let replayed = MaterializedProjectionState::replay_accepted_patches(
            "session-old-hostile",
            old_accepted_log,
        )
        .expect("old accepted hostile log replays");

        let note = replayed
            .notes
            .notes
            .first()
            .expect("note:old-hostile materialized");
        assert_eq!(
            note.body, hostile_body,
            "replay must never rewrite a historical body at materialization time — \
             the log content is the source of truth, got: {:?}",
            note.body
        );
        assert_eq!(
            note.heading_level,
            Some(99),
            "replay must never clamp a historical heading_level — an out-of-range \
             value in the log stays out-of-range when materialized, got: {:?}",
            note.heading_level
        );
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
            basis_currency_at_apply: None,
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

    /// seed audio-graph-e700 sub-fix 2 (UPSERT KEYING): two SEPARATE
    /// projection ticks (jobs) that both independently invent the model id
    /// `"node1"` for two UNRELATED entities must no longer silently collapse
    /// into one node under that shared id — the field evidence behind this
    /// ticket measured 54 of 155 persisted node ids carrying more than one
    /// distinct name. Before this fix, `upsert_node` matched purely on `id`,
    /// so the second tick's "Bob" would have overwritten the first tick's
    /// "Alice" in place, losing "Alice" entirely.
    #[test]
    fn upsert_collision_same_model_id_different_names_does_not_merge() {
        let mut graph = MaterializedGraph::new("session-1");
        let first_tick = graph_patch(
            1,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "node1".to_string(),
                name: "Alice".to_string(),
                entity_type: "Person".to_string(),
                description: None,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        graph
            .apply_patch(&first_tick, None)
            .expect("first tick seed");

        let second_tick = graph_patch(
            2,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "node1".to_string(),
                name: "Bob".to_string(),
                entity_type: "Person".to_string(),
                description: None,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        graph
            .apply_patch(&second_tick, None)
            .expect("second tick, colliding raw id");

        assert_eq!(
            graph.nodes.len(),
            2,
            "colliding raw ids for two different names must produce two nodes, got: {:?}",
            graph
                .nodes
                .iter()
                .map(|n| (&n.id, &n.name))
                .collect::<Vec<_>>()
        );
        let alice = graph
            .nodes
            .iter()
            .find(|node| node.name == "Alice")
            .expect("Alice survives the collision");
        let bob = graph
            .nodes
            .iter()
            .find(|node| node.name == "Bob")
            .expect("Bob is inserted under a disambiguated id");
        assert_ne!(
            alice.id, bob.id,
            "the two distinct entities must never share one persisted id"
        );
        assert_eq!(
            alice.id, "node1",
            "the first occupant keeps the raw model id"
        );
        assert_eq!(
            bob.id, "node1~2",
            "the colliding second entity gets a deterministic disambiguated id"
        );
        assert!(alice.valid_until_ms.is_none());
        assert!(bob.valid_until_ms.is_none());
    }

    /// seed audio-graph-e700 sub-fix 3 (FUZZY RESOLUTION): "Postgres" and
    /// "PostgreSQL" minted under two DIFFERENT model ids across two
    /// projection ticks must merge into ONE node instead of forking, bounded
    /// deterministically by [`fuzzy_entity_name_match`] (no LLM calls
    /// involved).
    #[test]
    fn fuzzy_resolution_merges_near_duplicate_entity_names_across_ids() {
        let mut graph = MaterializedGraph::new("session-1");
        let first_tick = graph_patch(
            1,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "product-1".to_string(),
                name: "Postgres".to_string(),
                entity_type: "Product".to_string(),
                description: Some("A relational database.".to_string()),
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        graph
            .apply_patch(&first_tick, None)
            .expect("first tick seed");

        let second_tick = graph_patch(
            2,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "product-7".to_string(),
                name: "PostgreSQL".to_string(),
                entity_type: "Product".to_string(),
                description: Some("Open-source object-relational database.".to_string()),
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        graph
            .apply_patch(&second_tick, None)
            .expect("second tick, near-duplicate name under a different id");

        let active_nodes: Vec<&MaterializedGraphNode> = graph
            .nodes
            .iter()
            .filter(|node| node.valid_until_ms.is_none())
            .collect();
        assert_eq!(
            active_nodes.len(),
            1,
            "near-duplicate names must merge into one active node, got: {:?}",
            active_nodes
                .iter()
                .map(|n| (&n.id, &n.name))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            active_nodes[0].id, "product-1",
            "the merge keeps the FIRST id"
        );
        assert_eq!(
            active_nodes[0].name, "PostgreSQL",
            "the merge takes the LATEST name"
        );
        assert_eq!(
            active_nodes[0].description.as_deref(),
            Some("Open-source object-relational database."),
            "the merge takes the LATEST description, matching ordinary update-in-place upserts"
        );
    }

    /// Direct assertions on the standalone [`fuzzy_entity_name_match`]
    /// matcher — NOT a `MaterializedGraph`/`upsert_node` wiring test (no
    /// graph is constructed, no id is ever shared between two calls here,
    /// despite this test's name). It pins the matcher's negative cases
    /// (dissimilar names must never match, even sharing a common prefix
    /// style) that back BOTH tiers 1 and 2 of `upsert_node`, since both
    /// route through this one function. The actual "same id, tier 1" wiring
    /// path is covered incidentally by
    /// `upsert_collision_same_model_id_different_names_does_not_merge`
    /// (across two ticks) and `same_id_rename_beyond_the_fuzzy_window_forks_a_disambiguated_duplicate`
    /// (single id, name drifts beyond the fuzzy window).
    #[test]
    fn fuzzy_resolution_does_not_cross_merge_dissimilar_names_sharing_an_id() {
        assert!(!fuzzy_entity_name_match("Alice", "Bob"));
        assert!(!fuzzy_entity_name_match("task 1", "task 2"));
        assert!(!fuzzy_entity_name_match("decision 1", "decision 2"));
        assert!(!fuzzy_entity_name_match("provider a", "provider b"));
        assert!(fuzzy_entity_name_match("Postgres", "PostgreSQL"));
        assert!(fuzzy_entity_name_match("OpenAI", "Open AI"));
    }

    /// Pins `FUZZY_ENTITY_NAME_MIN_PREFIX_RATIO`'s 0.6 floor itself, not just
    /// the prefix requirement: "React" is a genuine prefix of "React
    /// Native" (core `"react"` / `"reactnative"`, ratio 5/11 ≈ 0.4545), so
    /// this pair is caught ONLY by the ratio check, never by
    /// `longer.starts_with(shorter)` alone. Mutating the constant to `0.0`
    /// passes every OTHER test in this module (they all also satisfy the
    /// prefix requirement or are rejected by it outright) but flips this
    /// assertion — mirrors the TS
    /// `fuzzyEntityNameMatch`'s "does not merge names below the prefix
    /// ratio floor" test (`React`/`React Native`) exactly, closing the gap
    /// where only the frontend suite pinned this floor.
    #[test]
    fn fuzzy_resolution_does_not_merge_names_below_the_prefix_ratio_floor() {
        assert!(!fuzzy_entity_name_match("React", "React Native"));
    }

    /// The TS mirror counts core length in Unicode CODE POINTS
    /// (`Array.from(core).length`) because JS has no cheap equivalent of
    /// Rust's UTF-8 byte length; matching that basis in Rust (`.chars()`
    /// count instead of `.len()` bytes — audio-graph-e700 fix) is what
    /// makes the two languages agree on every non-ASCII case, not just the
    /// CJK example in `fuzzy_resolution_matches_non_ascii_names...` (whose
    /// three characters happen to all be 3-byte, so byte- and char-based
    /// ratios there are coincidentally identical). This pair straddles the
    /// 0.6 floor DIFFERENTLY depending on the length basis: `"café"` is 4
    /// chars / 5 bytes (the accented `é` is 2 bytes in UTF-8), and
    /// `"cafétea"` is 7 chars / 8 bytes — char ratio 4/7 ≈ 0.571 (below the
    /// floor) vs byte ratio 5/8 = 0.625 (above it). Reverting the length
    /// basis back to `.len()` flips this to a false MATCH.
    #[test]
    fn fuzzy_resolution_uses_char_count_not_byte_length_for_the_ratio() {
        assert!(!fuzzy_entity_name_match("café", "cafétea"));
    }

    /// Cross-language parity pin for the CJK counterexample the frontend
    /// fuzzy-core parity gap was measured against — see the mirrored TS
    /// test "resolves non-ASCII names identically to the Rust backend" in
    /// `src/utils/materializedGraph.test.ts`. Both languages must agree, or
    /// the live incremental view diverges from a replayed session.
    #[test]
    fn fuzzy_resolution_matches_non_ascii_names_identically_to_the_ts_mirror() {
        assert!(!fuzzy_entity_name_match("José", "Jose"));
        // CJK: "東京" (2 chars) is a genuine prefix of "東京都" (3 chars),
        // ratio 2/3 ≈ 0.667 >= the 0.6 floor.
        assert!(fuzzy_entity_name_match("東京", "東京都"));
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
                heading_level: None,
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
            heading_level: None,
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

    /// Pins the audio-graph-927a review blocker: `LedgerHistory`'s
    /// materialized speaker snapshot must reproduce
    /// [`SpeakerTimeline::apply_event`]'s chronological `(start_time,
    /// end_time, span_id)` `latest_spans` order, not the persistent
    /// `BTreeMap`'s span-id-lexicographic order.
    ///
    /// `timeline::speaker_attribution_index`'s winner rule is strict
    /// (`revision_number >`, tie-broken by `received_at_ms >`), so on an
    /// EXACT tie (equal `revision_number` AND equal `received_at_ms` — a
    /// diarization batch flushing multiple overlapping first-revision spans
    /// in the same wall-clock millisecond, each attributing the SAME
    /// transcript span) neither candidate satisfies `wins` and the
    /// FIRST-ITERATED span keeps the key. `"dz-b"` sorts AFTER `"dz-a"`
    /// lexicographically but starts EARLIER (`start_time` 1.0 vs 3.0), so a
    /// lexicographic snapshot flips the winner relative to the live path's
    /// chronological one — silently, because `judge_claim_evidence` still
    /// admits evidence either way, it just stamps the wrong `speaker_ref`.
    #[test]
    fn ledger_history_speaker_snapshot_breaks_exact_ties_in_chronological_not_lexicographic_order()
    {
        let mut transcript = TranscriptEvent::from(asr_payload("span-1", 1, "Shared span."));
        transcript.received_at_ms = 1_000;

        let mut dz_b = DiarizationSpanRevision::from(diarization_payload(
            "dz-b",
            "deepgram",
            1,
            "spk-B",
            DiarizationSpanStability::Stable,
        ));
        dz_b.start_time = 1.0;
        dz_b.end_time = 2.0;
        dz_b.received_at_ms = 5_000;
        dz_b.basis_asr_span_ids = vec!["span-1".to_string()];

        let mut dz_a = DiarizationSpanRevision::from(diarization_payload(
            "dz-a",
            "deepgram",
            1,
            "spk-A",
            DiarizationSpanStability::Stable,
        ));
        dz_a.start_time = 3.0;
        dz_a.end_time = 4.0;
        dz_a.received_at_ms = 5_000;
        dz_a.basis_asr_span_ids = vec!["span-1".to_string()];

        let mut patch = notes_patch_for_basis(
            1,
            std::slice::from_ref(&transcript),
            "note-1",
            "Tie-break",
            "Shared span.",
        );
        patch.operations = vec![ProjectionOperation::UpsertNote {
            id: "note-1".to_string(),
            title: "Tie-break".to_string(),
            body: "Shared span.".to_string(),
            tags: vec!["decision".to_string()],
            evidence: crate::claim_evidence::EvidenceAnchor {
                claim_class: crate::claim_evidence::ClaimClass::GroundedInference,
                span_id: Some("span-1".to_string()),
                quote: None,
                note: None,
            },
            heading_level: None,
        }];
        patch.created_at_ms = 10_000;

        // Live path: builds the `SpeakerTimeline` via `apply_event`
        // (chronological `sort_latest_spans` order) — the ground truth this
        // replay must match.
        let mut ledger = TranscriptLedger::new("session-tie");
        ledger.apply_event(transcript.clone()).unwrap();
        let live_speakers =
            SpeakerTimeline::replay("session-tie", [dz_b.clone(), dz_a.clone()]).unwrap();
        let mut live_state = MaterializedProjectionState::new("session-tie");
        live_state
            .apply_validated_patch_with_speaker_timeline(&ledger, &live_speakers, &patch)
            .expect("live apply");
        let live_speaker_ref = live_state.notes.notes[0]
            .evidence
            .clone()
            .expect("live apply admits GroundedInference evidence")
            .span()
            .expect("span resolved")
            .speaker_ref
            .clone();
        assert_eq!(
            live_speaker_ref.as_deref(),
            Some("spk-B"),
            "sanity check: the chronologically-first span (start_time=1.0) wins the tie"
        );

        // Replay path: must agree with the live path exactly, not flip to
        // "spk-A" because `LedgerHistory`'s internal storage happens to be
        // span_id-lexicographic.
        let replayed = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-tie",
            [transcript],
            Some(vec![dz_b, dz_a]),
            [patch],
        )
        .expect("replay");
        assert_eq!(replayed.validation.invalid_patch_count, 0);
        let replayed_speaker_ref = replayed.state.notes.notes[0]
            .evidence
            .clone()
            .expect("replay must admit GroundedInference evidence")
            .span()
            .expect("span resolved")
            .speaker_ref
            .clone();
        assert_eq!(
            replayed_speaker_ref, live_speaker_ref,
            "replay's speaker-attribution tie-break must match the live apply path exactly \
             (chronological order), not silently flip because the forward-cursor's persistent \
             storage is BTreeMap span_id order"
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
            heading_level: None,
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

    /// Direct-construction transcript event for the `LedgerHistory`
    /// regression/complexity fixtures below — bypasses `asr_payload`'s
    /// `is_final: revision_number > 1` coupling so every synthetic event is
    /// eligible for projection regardless of the revision number chosen.
    fn ledger_history_test_transcript_event(
        span_id: &str,
        revision_number: u64,
        received_at_ms: u64,
        text: &str,
    ) -> TranscriptEvent {
        TranscriptEvent {
            span_id: span_id.to_string(),
            provider: "test".to_string(),
            source_id: "test-source".to_string(),
            provider_item_id: None,
            transcript_segment_id: None,
            speaker_id: Some("speaker-1".to_string()),
            speaker_label: Some("Speaker 1".to_string()),
            channel: None,
            text: text.to_string(),
            start_time: (received_at_ms as f64) / 1000.0,
            end_time: (received_at_ms as f64) / 1000.0 + 1.0,
            confidence: 0.9,
            is_final: true,
            stability: TranscriptEventStability::Final,
            revision_number,
            supersedes: None,
            turn_id: None,
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms,
        }
    }

    /// Deliverable (d) — audio-graph-927a: a regressing `classify_bound_ms`
    /// (this patch's classify-time bound is EARLIER than the immediately
    /// preceding patch's) must still classify correctly via an isolated
    /// fresh fold for that one bound, not `LedgerHistory`'s forward
    /// cursor's later, frozen state.
    ///
    /// `generation_latency_ms` genuinely varies in production, so
    /// `classify_bound_ms` is not guaranteed monotone across
    /// sequence-ordered patches even when `created_at_ms` is (the ticket's
    /// CRITICAL SEMANTIC CONSTRAINT). This fixture regresses BOTH bounds at
    /// once (patch B's `created_at_ms` also regresses relative to patch
    /// A's), exercising both cursors' regression paths in one pass —
    /// mirroring
    /// `materialized_projection_history_uses_each_patch_time_when_timestamps_regress`'s
    /// pre-existing out-of-order `created_at_ms` shape above, but for a
    /// retcon whose visibility genuinely depends on getting the regressed
    /// bound right (a buggy "use the forward-frozen state" implementation
    /// would see the retcon a patch three timestamps earlier should never
    /// see, and misclassify it as `Revised`).
    #[test]
    fn ledger_history_handles_a_regressing_classify_bound_via_an_isolated_fresh_fold() {
        let span_1_v1 = ledger_history_test_transcript_event("span-1", 2, 1_000, "Original text.");
        let span_2_v1 = ledger_history_test_transcript_event("span-2", 2, 2_000, "Other span.");
        let span_1_v2 = ledger_history_test_transcript_event("span-1", 3, 3_000, "Corrected text.");

        // Ground truth at t=3000 (after the retcon): span-1 is at rev3,
        // span-2 at rev2.
        let mut ledger_at_3000 = TranscriptLedger::new("session-regress");
        ledger_at_3000
            .apply_event(span_1_v1.clone())
            .expect("span-1 v1");
        ledger_at_3000
            .apply_event(span_2_v1.clone())
            .expect("span-2 v1");
        ledger_at_3000
            .apply_event(span_1_v2.clone())
            .expect("span-1 v2 (retcon)");

        // Ground truth at t=2000 (before the retcon): span-1 is still at
        // rev2, span-2 at rev2.
        let mut ledger_at_2000 = TranscriptLedger::new("session-regress");
        ledger_at_2000
            .apply_event(span_1_v1.clone())
            .expect("span-1 v1");
        ledger_at_2000
            .apply_event(span_2_v1.clone())
            .expect("span-2 v1");

        // Patch A: created and classified at t=3000, AFTER the retcon.
        let mut patch_a = notes_patch_for_basis(
            1,
            std::slice::from_ref(&span_1_v1),
            "note-a",
            "After retcon",
            "Corrected text.",
        );
        patch_a.basis = ledger_at_3000.current_basis();
        patch_a.created_at_ms = 3_000;
        patch_a.generation_latency_ms = Some(0);

        // Patch B: created and classified at t=2000, BEFORE the retcon.
        // Both `created_at_ms` (2000 < 3000) and `classify_bound_ms`
        // (2000 < 3000) regress relative to patch A.
        let mut patch_b = notes_patch_for_basis(
            2,
            std::slice::from_ref(&span_1_v1),
            "note-b",
            "Before retcon",
            "Original text.",
        );
        patch_b.basis = ledger_at_2000.current_basis();
        patch_b.created_at_ms = 2_000;
        patch_b.generation_latency_ms = Some(0);

        // The correct snapshot at the regressed bound (t=2000) classifies
        // patch B's basis as `Current`.
        assert!(matches!(
            ledger_at_2000.classify_basis_currency(&patch_b.basis, None),
            BasisCurrency::Current
        ));
        // The WRONG (forward-frozen, t=3000) state disagrees — exactly the
        // divergence a regression-handling bug would produce.
        assert!(matches!(
            ledger_at_3000.classify_basis_currency(&patch_b.basis, None),
            BasisCurrency::Revised(ProjectionBasisStaleness::StaleSpanRevision { .. })
        ));

        let replayed = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-regress",
            [span_1_v1, span_2_v1, span_1_v2],
            None,
            [patch_a, patch_b],
        )
        .expect("regressing classify bound replays");

        assert_eq!(replayed.validation.checked_patch_count, 2);
        assert_eq!(
            replayed.validation.invalid_patch_count, 0,
            "patch B's regressed classify bound must be resolved via an isolated fresh fold, \
             not the forward cursor's later, frozen state: {:?}",
            replayed.validation.errors
        );
        assert_eq!(
            replayed
                .state
                .notes
                .notes
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            vec!["note-a", "note-b"]
        );
    }

    /// Major review finding: the EVIDENCE cursor's regression fallback
    /// (`LedgerHistory::transcript_snapshot_or_error`'s isolated-fresh-fold
    /// branch) was implemented correctly but had zero test coverage — a
    /// mutant that replaced it with `Ok(self.materialize_transcript_ledger())`
    /// (returning the LATER, forward-frozen cursor state instead of an
    /// isolated fold at the regressed bound) survived the full default
    /// suite, because [`ledger_history_handles_a_regressing_classify_bound_via_an_isolated_fresh_fold`]
    /// only exercises the CLASSIFY cursor's regression branch, and every
    /// existing regression fixture's evidence anchor is the default
    /// `EvidenceAnchor` (`KnowledgeGap`, unconditionally refused either way,
    /// so the evidence ledger's content never surfaces in the output).
    ///
    /// This test gives patch B a real, span-citing `GroundedInference`
    /// anchor pinned to the PRE-retcon revision, so the dual-bound semantic
    /// constraint ("claim evidence stays pinned to draft-time visibility")
    /// is actually observable: a forward-frozen mutant resolves the anchor
    /// against the POST-retcon ledger, where the pinned (revision 2) span
    /// no longer exists — `resolve_covered_events`'s exact-identity
    /// `(span_id, revision_number)` lookup misses, and the note's evidence
    /// silently degrades from `Some(..)` to `None` instead of resolving.
    #[test]
    fn ledger_history_evidence_cursor_regression_resolves_evidence_against_the_regressed_snapshot()
    {
        let span_1_v1 = ledger_history_test_transcript_event("span-1", 2, 1_000, "Original text.");
        let span_2_v1 = ledger_history_test_transcript_event("span-2", 2, 2_000, "Other span.");
        let span_1_v2 = ledger_history_test_transcript_event("span-1", 3, 3_000, "Corrected text.");

        let mut ledger_at_3000 = TranscriptLedger::new("session-regress-evidence");
        ledger_at_3000
            .apply_event(span_1_v1.clone())
            .expect("span-1 v1");
        ledger_at_3000
            .apply_event(span_2_v1.clone())
            .expect("span-2 v1");
        ledger_at_3000
            .apply_event(span_1_v2.clone())
            .expect("span-1 v2 (retcon)");

        let mut ledger_at_2000 = TranscriptLedger::new("session-regress-evidence");
        ledger_at_2000
            .apply_event(span_1_v1.clone())
            .expect("span-1 v1");
        ledger_at_2000
            .apply_event(span_2_v1.clone())
            .expect("span-2 v1");

        // Patch A: created (and evidence-bound) at t=3000, AFTER the
        // retcon — forward-advances the shared evidence cursor past
        // span-1's rev3.
        let mut patch_a = notes_patch_for_basis(
            1,
            std::slice::from_ref(&span_1_v1),
            "note-a",
            "After retcon",
            "Corrected text.",
        );
        patch_a.basis = ledger_at_3000.current_basis();
        patch_a.created_at_ms = 3_000;
        patch_a.generation_latency_ms = Some(0);

        // Patch B: created (and evidence-bound) at t=2000 — BEFORE the
        // retcon, regressed relative to patch A. Its `GroundedInference`
        // anchor cites span-1, which draft-time visibility (ADR-0037/
        // ADR-0031) demands resolve at the PRE-retcon revision (2), matching
        // what patch B's author actually saw — never the post-retcon
        // revision (3) patch A's later bound exposed.
        let mut patch_b = notes_patch_for_basis(
            2,
            std::slice::from_ref(&span_1_v1),
            "note-b",
            "Before retcon",
            "Original text.",
        );
        patch_b.basis = ledger_at_2000.current_basis();
        patch_b.created_at_ms = 2_000;
        patch_b.generation_latency_ms = Some(0);
        patch_b.operations = vec![ProjectionOperation::UpsertNote {
            id: "note-b".to_string(),
            title: "Before retcon".to_string(),
            body: "Original text.".to_string(),
            tags: vec!["decision".to_string()],
            evidence: crate::claim_evidence::EvidenceAnchor {
                claim_class: crate::claim_evidence::ClaimClass::GroundedInference,
                span_id: Some("span-1".to_string()),
                quote: None,
                note: None,
            },
            heading_level: None,
        }];

        let replayed = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-regress-evidence",
            [span_1_v1, span_2_v1, span_1_v2],
            None,
            [patch_a, patch_b],
        )
        .expect("regressing evidence bound replays");

        assert_eq!(replayed.validation.checked_patch_count, 2);
        assert_eq!(
            replayed.validation.invalid_patch_count, 0,
            "both patches' bases must classify as current: {:?}",
            replayed.validation.errors
        );

        let note_b = replayed
            .state
            .notes
            .notes
            .iter()
            .find(|note| note.id == "note-b")
            .expect("note-b materialized");
        let evidence = note_b.evidence.clone().expect(
            "patch B's GroundedInference anchor must resolve against the regressed (t=2000) \
             evidence snapshot, not the evidence cursor's later, forward-frozen state — a \
             forward-frozen mutant resolves nothing here (revision mismatch) instead",
        );
        let span = evidence.span().expect("GroundedInference resolves a span");
        assert_eq!(span.span_id, "span-1");
        assert_eq!(
            span.revision_number, 2,
            "must pin span-1 at its pre-retcon revision (2), matching what patch B's author saw \
             at draft time — a forward-frozen mutant would see rev3 (post-retcon) and therefore \
             fail to resolve at all"
        );
    }

    /// Minor review finding: the dedicated regression test above regresses
    /// BOTH `created_at_ms` and `classify_bound_ms` together. The ticket's
    /// own named non-monotone shape — `created_at_ms` staying
    /// NON-DECREASING across sequence-ordered patches while
    /// `classify_bound_ms` regresses purely because `generation_latency_ms`
    /// varies (a slow patch followed by a fast one) — was previously only a
    /// prose claim plus an observation about the synth fixture's own
    /// latency arithmetic, never its own pinned test. This builds that
    /// exact shape: patch A is slow (5000ms latency, classify bound 6000),
    /// patch B is fast (0ms latency) and created LATER (2000 >= 1000) but
    /// classified EARLIER (2000 < 6000) — a genuine classify-only
    /// regression with the evidence cursor advancing normally throughout.
    #[test]
    fn ledger_history_classify_bound_regresses_while_created_at_ms_advances_via_varying_generation_latency()
     {
        let span_0 = ledger_history_test_transcript_event("span-0", 1, 100, "Baseline.");
        let span_1_v1 =
            ledger_history_test_transcript_event("span-1", 1, 500, "Original attribution.");
        let span_1_v2 =
            ledger_history_test_transcript_event("span-1", 2, 4_000, "Corrected attribution.");

        // Patch A: created at t=1000, classified at t=1000+5000=6000 — its
        // basis cites only span-0 (chronologically first, never revised),
        // so the later span-1 retcon is a harmless append relative to it.
        // Processing it forward-advances the shared classify cursor all the
        // way to t=6000.
        let mut patch_a = notes_patch_for_basis(
            1,
            std::slice::from_ref(&span_0),
            "note-a",
            "Baseline",
            "Baseline.",
        );
        patch_a.created_at_ms = 1_000;
        patch_a.generation_latency_ms = Some(5_000);

        // Patch B: created at t=2000 (>= patch A's 1000 — `created_at_ms`
        // is non-decreasing) but classified at t=2000+0=2000 — ITS classify
        // bound (2000) regresses relative to patch A's (6000) purely
        // because `generation_latency_ms` varies, not because `created_at_ms`
        // itself went backward. Its basis is the full ledger snapshot at
        // t=2000 (span-0 + span-1 at its pre-retcon revision 1), matching
        // what was visible before the retcon lands at t=4000.
        let mut patch_b = notes_patch_for_basis(
            2,
            &[span_0.clone(), span_1_v1.clone()],
            "note-b",
            "Attribution",
            "Original attribution.",
        );
        patch_b.created_at_ms = 2_000;
        patch_b.generation_latency_ms = Some(0);

        let replayed = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-classify-regress-only",
            [span_0, span_1_v1, span_1_v2],
            None,
            [patch_a, patch_b],
        )
        .expect("classify-bound-only regression replays");

        assert_eq!(replayed.validation.checked_patch_count, 2);
        assert_eq!(
            replayed.validation.invalid_patch_count, 0,
            "patch B's regressed classify bound (2000, behind patch A's 6000) must resolve via \
             an isolated fresh fold even though patch B's OWN created_at_ms (2000) never \
             regressed relative to patch A's (1000): {:?}",
            replayed.validation.errors
        );
        assert_eq!(
            replayed
                .state
                .notes
                .notes
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            vec!["note-a", "note-b"]
        );
    }

    /// Synthetic-fixture sizes for
    /// `ledger_history_folds_each_event_a_bounded_number_of_times_not_once_per_patch`
    /// (audio-graph-927a deliverable c): large enough that the pre-927a
    /// per-patch full-refold algorithm's fold-operation count (~patches x
    /// events / 2, for bounds spread evenly across the stream) is clearly
    /// superlinear relative to `events + patches`, while staying fast
    /// enough for the default `cargo test` run — this is a deterministic
    /// work-counter assertion, not a wall-clock benchmark, so it needs no
    /// `#[ignore]` gate.
    const LEDGER_HISTORY_COMPLEXITY_FIXTURE_EVENTS: usize = 900;
    const LEDGER_HISTORY_COMPLEXITY_FIXTURE_PATCHES: usize = 300;

    /// Builds `num_events` transcript events, `num_events` speaker
    /// (diarization) revisions, and `num_patches` notes patches spread
    /// evenly across them: `created_at_ms` and `classify_bound_ms` both
    /// strictly increasing across patches (the realistic, non-regressing
    /// shape this ticket's field evidence describes — the regression path
    /// has its own dedicated test above). Every patch's basis is the
    /// ledger's own running current basis at that point (transcript-only —
    /// the speaker revisions are NOT cited by any basis, so they cannot
    /// affect currency classification; they exist purely to exercise the
    /// SPEAKER cursor's fold-op counting, addressing the review finding
    /// that the original fixture built transcript events only and so could
    /// never detect a reverted-to-per-patch-refold speaker cursor), and
    /// every patch upserts a distinct note id, so the whole log applies
    /// without a single invalid patch.
    fn ledger_history_complexity_fixture(
        num_events: usize,
        num_patches: usize,
    ) -> (
        Vec<TranscriptEvent>,
        Vec<DiarizationSpanRevision>,
        Vec<ProjectionPatch>,
    ) {
        assert!(
            num_events.is_multiple_of(num_patches),
            "fixture size must be a multiple of the patch count"
        );
        let events_per_patch = num_events / num_patches;
        let mut ledger = TranscriptLedger::new("complexity-fixture");
        let mut events = Vec::with_capacity(num_events);
        let mut speaker_events = Vec::with_capacity(num_events);
        let mut patches = Vec::with_capacity(num_patches);

        for i in 0..num_events {
            let event = ledger_history_test_transcript_event(
                &format!("complexity-span-{i}"),
                1,
                1_700_000_000_000 + (i as u64) * 10,
                &format!("Complexity fixture line {i}."),
            );
            ledger
                .apply_event(event.clone())
                .expect("complexity fixture events apply cleanly");
            events.push(event);

            // Independent speaker stream, strictly increasing `received_at_ms`
            // in lockstep with the transcript stream, never cited by any
            // patch basis — see this fixture's doc comment above.
            let mut speaker_event = DiarizationSpanRevision::from(diarization_payload(
                &format!("complexity-speaker-{i}"),
                "test",
                1,
                "speaker-1",
                DiarizationSpanStability::Stable,
            ));
            speaker_event.received_at_ms = 1_700_000_000_000 + (i as u64) * 10;
            speaker_events.push(speaker_event);

            if !(i + 1).is_multiple_of(events_per_patch) {
                continue;
            }
            let patch_index = (i + 1) / events_per_patch - 1;
            let created_at_ms = 1_700_000_000_000 + (i as u64) * 10;
            patches.push(ProjectionPatch {
                route: None,
                sequence: patch_index as u64 + 1,
                kind: ProjectionKind::Notes,
                llm_request_id: format!("complexity-req-{patch_index}"),
                basis: ledger.current_projection_basis(),
                operations: vec![ProjectionOperation::UpsertNote {
                    id: format!("complexity-note-{patch_index}"),
                    title: "Complexity fixture".to_string(),
                    body: format!("Complexity fixture line {i}."),
                    tags: vec!["complexity".to_string()],
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                    heading_level: None,
                }],
                confidence: 0.9,
                provenance: ProjectionProvenance {
                    provider: "synth-bench".to_string(),
                    model: "complexity-fixture".to_string(),
                    prompt_id: "complexity_v1".to_string(),
                    route_id: None,
                    model_source: crate::llm::route::ModelIdentitySource::Requested,
                },
                queued_at_ms: Some(created_at_ms),
                generation_latency_ms: Some(5),
                apply_latency_ms: Some(5),
                basis_currency_at_apply: None,
                created_at_ms,
            });
        }

        (events, speaker_events, patches)
    }

    /// Deliverable (c) — audio-graph-927a: proves `LedgerHistory` folds
    /// each raw transcript AND speaker event a BOUNDED number of times
    /// across the whole replay (at most once per cursor — evidence and
    /// classify — i.e. exactly `2 x (events + speaker_events)` on this
    /// non-regressing fixture), not once per patch. The speaker stream is
    /// included specifically because an earlier version of this fixture
    /// built transcript events only, so it could never detect a revert of
    /// the SPEAKER cursor alone back to a per-patch full re-fold (review
    /// finding).
    ///
    /// The pre-927a implementation rebuilt a fresh `TranscriptLedger`/
    /// `SpeakerTimeline` pair from event zero for every patch, twice per
    /// patch, so its fold-operation count on this fixture would be on the
    /// order of `patches x (events + speaker_events)` (bounds spread evenly
    /// average out to roughly half the full prefix per patch) — orders of
    /// magnitude more than this test allows.
    ///
    /// No wall-clock timing: this counts actual fold operations via a
    /// `#[cfg(test)]` atomic counter incremented only inside
    /// `fast_apply_transcript_event`/`fast_apply_speaker_event`.
    /// Mutation-proof in both directions — a revert to a per-patch full
    /// re-fold (even if routed through this counter) blows past the
    /// exact-equality upper bound, and bypassing `LedgerHistory` entirely
    /// leaves the counter at zero, tripping the same exact-equality
    /// assertion from below.
    #[test]
    fn ledger_history_folds_each_event_a_bounded_number_of_times_not_once_per_patch() {
        let (events, speaker_events, patches) = ledger_history_complexity_fixture(
            LEDGER_HISTORY_COMPLEXITY_FIXTURE_EVENTS,
            LEDGER_HISTORY_COMPLEXITY_FIXTURE_PATCHES,
        );
        let events_len = events.len();
        let speaker_events_len = speaker_events.len();
        let patches_len = patches.len();

        reset_ledger_history_fold_ops_counter();
        let replay = MaterializedProjectionState::replay_accepted_patches_with_history(
            "complexity-fixture",
            events,
            Some(speaker_events),
            patches,
        )
        .expect("complexity fixture replays cleanly");
        let fold_ops = ledger_history_fold_ops_counter();

        assert_eq!(
            replay.validation.invalid_patch_count, 0,
            "complexity fixture must apply cleanly (every basis stays current): {:?}",
            replay.validation.errors
        );
        assert_eq!(replay.validation.checked_patch_count, patches_len);

        // Exact bound: the evidence cursor and the classify cursor each
        // fold every one of `events_len` transcript events AND every one of
        // `speaker_events_len` speaker events AT MOST ONCE across the WHOLE
        // replay (both bound sequences are strictly non-decreasing by
        // construction here), so the total fold-op count is exactly
        // `2 * (events_len + speaker_events_len)` — never a function of
        // `patches_len`.
        let total_raw_events = events_len + speaker_events_len;
        assert_eq!(
            fold_ops,
            2 * total_raw_events,
            "LedgerHistory must fold each transcript/speaker event at most once per cursor \
             across the whole replay, not once per patch (got {fold_ops} fold ops for \
             {events_len} transcript events + {speaker_events_len} speaker events / \
             {patches_len} patches)"
        );

        // Make the "the old algorithm would be superlinear here" claim
        // explicit and self-checking, not just asserted in prose: a
        // per-patch full re-fold rebuilding from event zero for every
        // patch, with bounds spread evenly across the stream, pays
        // roughly half the full prefix per patch on average.
        let old_algorithm_estimated_fold_ops = patches_len * total_raw_events / 2;
        assert!(
            old_algorithm_estimated_fold_ops >= 50 * fold_ops,
            "fixture is not large enough to demonstrate superlinearity: old-shape estimate \
             {old_algorithm_estimated_fold_ops} is not >= 50x the new fold-op count {fold_ops}"
        );
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

    /// Major review finding: the TRANSCRIPT side of `LedgerHistory`'s
    /// fold-failure/poison machinery had zero test coverage — nothing in
    /// the crate ever produced `HistoricalProjectionValidationError::TranscriptReplay`
    /// before this test, unlike the speaker side (mirrored/twinned here,
    /// see [`materialized_projection_history_reports_content_free_speaker_replay_failures`]
    /// just above). A mutant that silently accepted a stale transcript
    /// revision (`fast_apply_transcript_event`'s stale arm returning
    /// `Ok(())` instead of erroring) survived the full default suite before
    /// this test existed.
    #[test]
    fn materialized_projection_history_reports_content_free_transcript_replay_failures() {
        let mut base = TranscriptEvent::from(asr_payload("span-base", 1, "Ship notes."));
        base.received_at_ms = 1_700_000_010_000;
        let mut patch = notes_patch_for_basis(
            1,
            std::slice::from_ref(&base),
            "note-transcript-error",
            "Transcript error",
            "Invalid transcript history must fail closed.",
        );
        patch.created_at_ms = 1_700_000_010_100;

        let mut conflict_a =
            TranscriptEvent::from(asr_payload("span-conflict", 1, "PRIVATE-TRANSCRIPT-TEXT-A"));
        conflict_a.received_at_ms = 1_700_000_010_010;
        let mut conflict_b = conflict_a.clone();
        conflict_b.text = "PRIVATE-TRANSCRIPT-TEXT-B".to_string();
        conflict_b.received_at_ms = 1_700_000_010_020;

        let conflict = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-1",
            [conflict_a, conflict_b],
            None,
            [patch.clone()],
        )
        .expect("conflicting transcript replay report");
        assert!(matches!(
            conflict.validation.errors.first(),
            Some(HistoricalProjectionValidationError::TranscriptReplay {
                sequence: 1,
                error: TranscriptLedgerError::ConflictingTranscriptRevision {
                    span_id,
                    revision_number: 1,
                },
            }) if span_id == "span-conflict"
        ));

        let mut current =
            TranscriptEvent::from(asr_payload("span-stale", 2, "PRIVATE-TRANSCRIPT-TEXT-C"));
        current.received_at_ms = 1_700_000_010_010;
        let mut stale =
            TranscriptEvent::from(asr_payload("span-stale", 1, "PRIVATE-TRANSCRIPT-TEXT-D"));
        stale.received_at_ms = 1_700_000_010_020;

        let stale_replay = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-1",
            [current, stale],
            None,
            [patch],
        )
        .expect("stale transcript replay report");
        assert!(matches!(
            stale_replay.validation.errors.first(),
            Some(HistoricalProjectionValidationError::TranscriptReplay {
                sequence: 1,
                error: TranscriptLedgerError::StaleTranscriptRevision {
                    span_id,
                    current_revision: 2,
                    incoming_revision: 1,
                },
            }) if span_id == "span-stale"
        ));

        let diagnostics = format!("{:?}{:?}", conflict.validation, stale_replay.validation);
        assert!(!diagnostics.contains("PRIVATE-TRANSCRIPT-TEXT"));
    }

    /// Major review finding (boundary predicate): pins
    /// `LedgerHistory::transcript_snapshot_or_error`'s poison-propagation
    /// check (`target_len > poison_index`) at its exact edge. A patch whose
    /// bound's prefix length lands EXACTLY AT the poisoning event's index
    /// (i.e. its window ends strictly BEFORE that event, since `target_len`
    /// is a prefix LENGTH) must NOT see the failure; a patch whose prefix
    /// reaches past it must. Both branches of the `>` comparison are
    /// exercised by a single poisoned event stream shared across two
    /// sequence-ordered patches, matching how a real corrupted event log
    /// would be discovered mid-replay.
    #[test]
    fn ledger_history_transcript_poison_boundary_predicate_pins_patches_before_and_at_the_poison_index()
     {
        let span_a_v1 = ledger_history_test_transcript_event("span-a", 1, 1_000, "Original.");
        let span_a_v2 = ledger_history_test_transcript_event("span-a", 2, 2_000, "Retconned.");
        // Stale: revision_number=1, lower than the already-applied rev2 —
        // folding this poisons the cursor at index 2 (the event's own
        // index, since two events before it applied cleanly).
        let span_a_stale =
            ledger_history_test_transcript_event("span-a", 1, 3_000, "Stale replay.");

        let ledger_at_2000 = {
            let mut ledger = TranscriptLedger::new("session-poison-boundary");
            ledger.apply_event(span_a_v1.clone()).expect("v1");
            ledger.apply_event(span_a_v2.clone()).expect("v2");
            ledger
        };

        // Patch 1: bound=2000 — prefix length 2, EXACTLY EQUAL to the
        // poison index (2). Must NOT see the poison (its window ends
        // strictly before the poisoning event).
        let mut patch_1 = notes_patch_for_basis(
            1,
            std::slice::from_ref(&span_a_v1),
            "note-before-poison",
            "Before poison",
            "Retconned.",
        );
        patch_1.basis = ledger_at_2000.current_basis();
        patch_1.created_at_ms = 2_000;
        patch_1.generation_latency_ms = Some(0);

        // Patch 2: bound=3000 — prefix length 3, STRICTLY GREATER than the
        // poison index (2). Must surface the poisoning error.
        let mut patch_2 = notes_patch_for_basis(
            2,
            std::slice::from_ref(&span_a_v1),
            "note-at-poison",
            "At poison",
            "Should never materialize.",
        );
        patch_2.created_at_ms = 3_000;
        patch_2.generation_latency_ms = Some(0);

        let replayed = MaterializedProjectionState::replay_accepted_patches_with_history(
            "session-poison-boundary",
            [span_a_v1, span_a_v2, span_a_stale],
            None,
            [patch_1, patch_2],
        )
        .expect("poison-boundary replay report");

        assert_eq!(replayed.validation.checked_patch_count, 2);
        assert_eq!(
            replayed.validation.invalid_patch_count, 1,
            "exactly patch 2 (whose prefix reaches the poison index) must fail: {:?}",
            replayed.validation.errors
        );
        assert!(matches!(
            replayed.validation.errors.first(),
            Some(HistoricalProjectionValidationError::TranscriptReplay {
                sequence: 2,
                error: TranscriptLedgerError::StaleTranscriptRevision {
                    span_id,
                    current_revision: 2,
                    incoming_revision: 1,
                },
            }) if span_id == "span-a"
        ));
        assert_eq!(
            replayed
                .state
                .notes
                .notes
                .iter()
                .map(|note| note.id.as_str())
                .collect::<Vec<_>>(),
            vec!["note-before-poison"],
            "patch 1 (before the poison index) must materialize; patch 2 (at/past it) must not"
        );
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

    /// seed audio-graph-e700 REPLAY COMPATIBILITY (blocker-class per the
    /// ticket): a fixture shaped like a PRE-e700 session — free-string
    /// `entity_type`/`relation_type` values that are neither
    /// case-normalized nor members of the closed ontology, AND two separate
    /// patches that collide on a generic model-invented id (`"node1"`) for
    /// two unrelated names, exactly the field bug this ticket fixes.
    /// Replaying it through TODAY's `apply_patch` must not error and must
    /// not drop any entity (ADR-0045: "no accepted patch may be silently
    /// discarded" — materialization is a pure re-derivation from the
    /// accepted patch log; a validation that rejects old events is an
    /// automatic blocker). Built directly from this test
    /// module's existing fixture style, NOT from field-session content (this
    /// ticket's STOP CONDITION forbids reading the session that produced the
    /// measured 31-invented-type / 54-colliding-id evidence).
    #[test]
    fn replay_tolerates_pre_e700_free_string_types_and_colliding_ids() {
        let old_style_patch_one = graph_patch(
            1,
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "node1".to_string(),
                    name: "Alice".to_string(),
                    // Pre-e700 free string: not a canonical ontology name at
                    // all, and never normalized by replay (only fresh
                    // ingest normalizes).
                    entity_type: "SPEAKER".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "node2".to_string(),
                    name: "Q3 Roadmap".to_string(),
                    entity_type: "meeting_topic".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge1".to_string(),
                    source: "node1".to_string(),
                    target: "node2".to_string(),
                    relation_type: "DISCUSSED".to_string(),
                    label: None,
                    weight: 0.6,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
            ],
        );
        // A later, SEPARATE tick that independently re-invented the SAME
        // generic id ("node1") for an unrelated entity — the measured field
        // bug (54 of 155 colliding node ids in one session).
        let old_style_patch_two = graph_patch(
            2,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "node1".to_string(),
                name: "Carol".to_string(),
                entity_type: "SPEAKER".to_string(),
                description: None,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );

        let replayed = MaterializedProjectionState::replay_accepted_patches(
            "session-old",
            [old_style_patch_one, old_style_patch_two],
        )
        .expect("pre-e700 free-string types and colliding ids must not error on replay");

        assert_eq!(
            replayed.graph.nodes.len(),
            3,
            "no node may be silently dropped on replay, got: {:?}",
            replayed
                .graph
                .nodes
                .iter()
                .map(|n| (&n.id, &n.name))
                .collect::<Vec<_>>()
        );
        assert!(replayed.graph.nodes.iter().any(|n| n.name == "Alice"));
        assert!(replayed.graph.nodes.iter().any(|n| n.name == "Q3 Roadmap"));
        assert!(replayed.graph.nodes.iter().any(|n| n.name == "Carol"));
        // The collision-disambiguated Carol must land under a DIFFERENT
        // persisted id than Alice — three distinct names sharing a
        // duplicated id would satisfy the bare node-count check above while
        // corrupting the graph (two rows the frontend's `id`-keyed
        // `findIndex` lookups could never distinguish).
        let ids: std::collections::BTreeSet<&str> =
            replayed.graph.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids.len(),
            3,
            "every persisted node must have a UNIQUE id, got ids: {:?}",
            replayed
                .graph
                .nodes
                .iter()
                .map(|n| &n.id)
                .collect::<Vec<_>>()
        );
        // The pre-e700 free-string entity_type/relation_type values are
        // NEVER rewritten by replay — normalization is ingest-only
        // (`projection_llm::normalize_projection_patch_draft_ontology`),
        // never applied to historical persisted operations.
        assert!(
            replayed
                .graph
                .nodes
                .iter()
                .any(|n| n.entity_type == "SPEAKER")
        );
        assert!(
            replayed
                .graph
                .nodes
                .iter()
                .any(|n| n.entity_type == "meeting_topic")
        );
        assert_eq!(replayed.graph.edges[0].relation_type, "DISCUSSED");
    }

    /// seed audio-graph-e700 REPLAY COMPATIBILITY (blocker per reviewer
    /// finding 1): a pre-e700 accepted log commonly contains
    /// upsert-then-invalidate-then-re-upsert-under-the-same-id sequences
    /// (the old code always resurrected in place, since it matched purely
    /// on id regardless of active status). Requiring the tier-1 same-id
    /// match to be ACTIVE broke this: the re-upsert would miss all three
    /// tiers and mint a disambiguated id instead, so a LATER patch's raw-id
    /// reference (an edge, here) would hard-error with `MissingGraphNode`
    /// even though the exact same log replayed cleanly under the old gate.
    #[test]
    fn resurrection_after_invalidate_keeps_the_same_id_for_later_cross_patch_reference() {
        let seed = graph_patch(
            1,
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "node1".to_string(),
                    name: "Alice".to_string(),
                    entity_type: "Person".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "node2".to_string(),
                    name: "Roadmap".to_string(),
                    entity_type: "Topic".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
            ],
        );
        let invalidate = graph_patch(
            2,
            vec![ProjectionOperation::InvalidateGraphNode {
                id: "node1".to_string(),
            }],
        );
        let resurrect = graph_patch(
            3,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "node1".to_string(),
                name: "Alice".to_string(),
                entity_type: "Person".to_string(),
                description: None,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        // A LATER, SEPARATE patch referencing the resurrected raw id — the
        // per-patch `id_overrides` map is empty here, so this can only
        // succeed if `node1` is actually active again.
        let later_edge = graph_patch(
            4,
            vec![ProjectionOperation::UpsertGraphEdge {
                id: "edge1".to_string(),
                source: "node1".to_string(),
                target: "node2".to_string(),
                relation_type: "discussed".to_string(),
                label: None,
                weight: 0.5,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );

        let replayed = MaterializedProjectionState::replay_accepted_patches(
            "session-resurrect",
            [seed, invalidate, resurrect, later_edge],
        )
        .expect("resurrection followed by a later cross-patch edge reference must not error");

        assert!(
            replayed
                .graph
                .nodes
                .iter()
                .any(|n| n.id == "node1" && n.valid_until_ms.is_none()),
            "node1 must be resurrected active under its ORIGINAL id, not forked: {:?}",
            replayed
                .graph
                .nodes
                .iter()
                .map(|n| (&n.id, &n.name, n.valid_until_ms))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            replayed.graph.nodes.len(),
            2,
            "resurrection must update the original row, never fork a duplicate"
        );
        let edge = replayed
            .graph
            .edges
            .iter()
            .find(|e| e.id == "edge1")
            .expect("edge from the later patch must have been applied");
        assert_eq!(edge.source, "node1");
    }

    /// seed audio-graph-e700 REPLAY COMPATIBILITY (blocker per reviewer
    /// finding 2): a raw id fuzzy-absorbed by `upsert_node` (tier 2, cross-id
    /// near-duplicate merge) never gets a row of its own — the model's raw
    /// id `"n7"` never appears as a literal id anywhere in `graph.nodes`.
    /// Before this fix, `resolve_graph_node_id` had no memory of that
    /// redirection once its ORIGINATING patch finished (`id_overrides` was
    /// local to one `apply_patch` call), so a LATER, separate patch
    /// referencing `"n7"` directly (an edge endpoint here) would fail
    /// `has_active_node` and return `Err(MissingGraphNode)` — on a log every
    /// patch of which was individually accepted by the old gate.
    #[test]
    fn cross_patch_reference_to_a_fuzzy_absorbed_raw_id_resolves_via_persistent_alias() {
        let mut graph = MaterializedGraph::new("session-alias");
        let first_tick = graph_patch(
            1,
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "n1".to_string(),
                    name: "Postgres".to_string(),
                    entity_type: "Product".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "n20".to_string(),
                    name: "Deployment".to_string(),
                    entity_type: "Topic".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
            ],
        );
        graph.apply_patch(&first_tick, None).expect("seed tick");

        // "n7" never had its own row: `upsert_node` finds it fuzzy-matches
        // the ALREADY-active "n1" (Postgres) by name and absorbs into it.
        let second_tick = graph_patch(
            2,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "n7".to_string(),
                name: "PostgreSQL".to_string(),
                entity_type: "Product".to_string(),
                description: None,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        graph
            .apply_patch(&second_tick, None)
            .expect("absorption tick");
        assert!(
            !graph.nodes.iter().any(|n| n.id == "n7"),
            "the raw id must never get a row of its own once absorbed"
        );
        assert_eq!(
            graph.id_aliases.get("n7").map(String::as_str),
            Some("n1"),
            "the redirection must be recorded persistently, not just per-patch"
        );

        // A THIRD, separate patch referencing the raw id "n7" directly.
        let third_tick = graph_patch(
            3,
            vec![ProjectionOperation::UpsertGraphEdge {
                id: "edge1".to_string(),
                source: "n7".to_string(),
                target: "n20".to_string(),
                relation_type: "used_for".to_string(),
                label: None,
                weight: 0.5,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        graph
            .apply_patch(&third_tick, None)
            .expect("a later, separate patch referencing the absorbed raw id must not hard-error");

        let edge = graph
            .edges
            .iter()
            .find(|e| e.id == "edge1")
            .expect("edge must have been applied");
        assert_eq!(
            edge.source, "n1",
            "the edge must land on the id the content actually lives under"
        );

        // The SAME cross-patch trap also applies to `invalidate_graph_node`
        // referencing the absorbed raw id directly.
        let fourth_tick = graph_patch(
            4,
            vec![ProjectionOperation::InvalidateGraphNode {
                id: "n7".to_string(),
            }],
        );
        graph.apply_patch(&fourth_tick, None).expect(
            "invalidate_graph_node on the absorbed raw id must resolve to the real node, not error",
        );
        assert!(
            graph
                .nodes
                .iter()
                .any(|n| n.id == "n1" && n.valid_until_ms.is_some()),
            "invalidating via the absorbed raw id must invalidate the node it actually landed on"
        );
    }

    /// seed audio-graph-e700 REPLAY COMPATIBILITY: a model naturally emits
    /// `upsert n1; upsert n7 (fuzzy-absorbed into n1); merge_graph_nodes
    /// (source: n7, target: n1)` in ONE patch to explicitly reconcile the
    /// near-duplicate it just created in the SAME breath. Before this fix
    /// this resolved both ids to `"n1"` via `id_overrides` and then
    /// `merge_nodes` rejected the now-identical pair with
    /// `Err(InvalidGraphMerge)`, failing the WHOLE patch — on an operation
    /// that is semantically a no-op, not an error.
    #[test]
    fn same_patch_merge_into_its_own_fuzzy_absorption_target_is_a_no_op() {
        let mut graph = MaterializedGraph::new("session-merge-noop");
        let patch = graph_patch(
            1,
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "n1".to_string(),
                    name: "Postgres".to_string(),
                    entity_type: "Product".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "n7".to_string(),
                    name: "PostgreSQL".to_string(),
                    entity_type: "Product".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::MergeGraphNodes {
                    source_id: "n7".to_string(),
                    target_id: "n1".to_string(),
                },
            ],
        );

        graph
            .apply_patch(&patch, None)
            .expect("a merge that resolves to a no-op must not reject the whole patch");

        let active: Vec<&MaterializedGraphNode> = graph
            .nodes
            .iter()
            .filter(|n| n.valid_until_ms.is_none())
            .collect();
        assert_eq!(
            active.len(),
            1,
            "the absorption already unified them; the redundant merge must not invalidate n1"
        );
        assert_eq!(active[0].id, "n1");
    }

    /// seed audio-graph-e700 (reviewer finding 3): the SAME-patch remap
    /// table must keep working for a displaced upsert even when the
    /// displacement is a tier-3 DISAMBIGUATION (collision with an unrelated
    /// PRE-EXISTING node), not just a tier-2 fuzzy absorption — this is the
    /// one cross-reference axis no test in either language exercised before
    /// (mutating `resolve_graph_node_id` to the identity function passed the
    /// whole suite).
    #[test]
    fn same_patch_displaced_upsert_cross_reference_lands_on_the_disambiguated_id() {
        let mut graph = MaterializedGraph::new("session-displaced-xref");
        let seed = graph_patch(
            1,
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "node1".to_string(),
                    name: "Alice".to_string(),
                    entity_type: "Person".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "topic:standup".to_string(),
                    name: "Standup".to_string(),
                    entity_type: "Topic".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
            ],
        );
        graph.apply_patch(&seed, None).expect("seed collision node");

        // In ONE later patch: an unrelated "Bob" upsert collides on the
        // literal id "node1" (gets displaced to "node1~2"), and an edge in
        // the SAME patch references the raw id "node1" — per the
        // Graph-kind prompt's own model-facing convention, this means "the
        // node I just upserted", i.e. Bob/"node1~2", not the pre-existing
        // Alice.
        let displaced = graph_patch(
            2,
            vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "node1".to_string(),
                    name: "Bob".to_string(),
                    entity_type: "Person".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphEdge {
                    id: "edge1".to_string(),
                    source: "node1".to_string(),
                    target: "topic:standup".to_string(),
                    relation_type: "discussed".to_string(),
                    label: None,
                    weight: 0.5,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
            ],
        );
        graph
            .apply_patch(&displaced, None)
            .expect("displaced upsert plus same-patch cross-reference");

        let bob = graph
            .nodes
            .iter()
            .find(|n| n.name == "Bob")
            .expect("Bob must be inserted under a disambiguated id");
        assert_eq!(bob.id, "node1~2");
        let edge = graph
            .edges
            .iter()
            .find(|e| e.id == "edge1")
            .expect("edge must have been applied");
        assert_eq!(
            edge.source, "node1~2",
            "a same-patch reference to the raw id must follow THIS patch's own displacement, \
             not the pre-existing node that still literally owns that id"
        );
        assert_eq!(edge.target, "topic:standup");
    }

    /// Reviewer finding (minor, disclosed trade-off): a legitimate same-id
    /// rename whose new name falls OUTSIDE `fuzzy_entity_name_match`'s
    /// window (no shared prefix core, or below the 0.6 ratio floor) no
    /// longer updates in place — it forks a visible duplicate under a
    /// disambiguated id instead, because tier 1 requires the name to still
    /// fuzzy-match. This is an accepted trade-off against the collision bug
    /// (see `upsert_collision_same_model_id_different_names_does_not_merge`),
    /// not a defect; this test PINS the current, intentional behavior so a
    /// future change to it is deliberate rather than an accidental
    /// regression.
    #[test]
    fn same_id_rename_beyond_the_fuzzy_window_forks_a_disambiguated_duplicate() {
        let mut graph = MaterializedGraph::new("session-rename-fork");
        let first = graph_patch(
            1,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "topic:roadmap".to_string(),
                name: "Roadmap".to_string(),
                entity_type: "Topic".to_string(),
                description: None,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        graph.apply_patch(&first, None).expect("seed topic node");

        let renamed = graph_patch(
            2,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "topic:roadmap".to_string(),
                name: "Q3 Roadmap".to_string(),
                entity_type: "Topic".to_string(),
                description: None,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        graph
            .apply_patch(&renamed, None)
            .expect("rename beyond the fuzzy window");

        let active: Vec<&MaterializedGraphNode> = graph
            .nodes
            .iter()
            .filter(|n| n.valid_until_ms.is_none())
            .collect();
        assert_eq!(
            active.len(),
            2,
            "documented current behavior: a rename beyond the fuzzy window forks, \
             it does not update in place, got: {:?}",
            active.iter().map(|n| (&n.id, &n.name)).collect::<Vec<_>>()
        );
        assert!(
            active
                .iter()
                .any(|n| n.name == "Roadmap" && n.id == "topic:roadmap")
        );
        assert!(
            active
                .iter()
                .any(|n| n.name == "Q3 Roadmap" && n.id == "topic:roadmap~2")
        );
    }

    /// Reviewer finding (major, disclosed trade-off): after a same-id
    /// collision displaces a later upsert to a disambiguated id (tier 3), a
    /// LATER, SEPARATE patch's raw-id reference resolves to the FIRST
    /// occupant of that literal id (the one that still literally owns it),
    /// not the most-recently-written entity the model probably meant by
    /// reusing that raw id. Pre-e700, the model's LATEST content under a
    /// shared id always won (destructive overwrite in place); this fix
    /// deliberately does not persist tier-3 displacements into
    /// `id_aliases` in a way that would override a literal row (see
    /// `resolve_graph_node_id`'s doc comment, tier 2) — it is less
    /// destructive than the old overwrite, but not equivalent to "latest
    /// wins" either. This test pins the CHOSEN semantics so a future change
    /// to it (e.g. binding cross-patch raw-id references to the
    /// most-recently-written owner instead) is deliberate.
    #[test]
    fn cross_patch_reference_after_a_collision_binds_to_the_first_occupant_not_the_latest() {
        let mut graph = MaterializedGraph::new("session-first-owner-wins");
        let seed = graph_patch(
            1,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "node1".to_string(),
                name: "Alice".to_string(),
                entity_type: "Person".to_string(),
                description: None,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        graph
            .apply_patch(&seed, None)
            .expect("seed Alice under node1");

        // A SEPARATE, later patch mints "Bob" under the same raw id — no
        // fuzzy match, so tier 3 displaces Bob to "node1~2"; Alice's row
        // (literal id "node1") is untouched.
        let collision = graph_patch(
            2,
            vec![ProjectionOperation::UpsertGraphNode {
                id: "node1".to_string(),
                name: "Bob".to_string(),
                entity_type: "Person".to_string(),
                description: None,
                evidence: crate::claim_evidence::EvidenceAnchor::default(),
            }],
        );
        graph.apply_patch(&collision, None).expect("Bob displaced");

        // A THIRD, separate patch references the raw id "node1" directly —
        // `id_overrides` is empty (fresh patch), so this resolves via the
        // literal-row check, landing on Alice, not Bob.
        let reference = graph_patch(
            3,
            vec![ProjectionOperation::InvalidateGraphNode {
                id: "node1".to_string(),
            }],
        );
        graph
            .apply_patch(&reference, None)
            .expect("cross-patch reference to the collided raw id must not error");

        let alice = graph
            .nodes
            .iter()
            .find(|n| n.name == "Alice")
            .expect("Alice still exists under node1");
        assert!(
            alice.valid_until_ms.is_some(),
            "the cross-patch invalidate_graph_node(\"node1\") must have bound to Alice \
             (the first occupant of the literal id), not Bob"
        );
        let bob = graph
            .nodes
            .iter()
            .find(|n| n.name == "Bob")
            .expect("Bob still exists under node1~2");
        assert!(
            bob.valid_until_ms.is_none(),
            "Bob (node1~2) must be UNAFFECTED by a cross-patch reference to the raw id \"node1\""
        );
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

    /// audio-graph-cfa1 (P1 FATAL, field session d97bfcc3): before this
    /// ticket, `ProjectionBasis::span_revisions` carried every covered span
    /// individually forever — one field node accumulated 933 entries despite
    /// `summarized_through_revision: 12`, and growth was
    /// O(items touched × live spans), superlinear per session hour. This
    /// drives a synthetic session well past `ROLLING_SUMMARY_HOT_WINDOW_TURNS`,
    /// repeatedly re-touching the SAME small set of facts (mirroring the
    /// field pattern of one entity revised on nearly every tick), and proves
    /// the persisted per-item basis size stays bounded regardless of how
    /// long the session has run.
    ///
    /// Mutation-proof: if `ProjectionBasis`'s constructor stops folding the
    /// covered-but-summarized prefix into `covered_prefix` (compaction
    /// disabled — every covered span once again listed individually in
    /// `span_revisions`), the bounded-size assertion below fails, and the
    /// closing cross-check against a manually-constructed uncompacted
    /// equivalent basis fails too.
    ///
    /// Deliberately measures the touched item's `basis` field alone, not
    /// `serde_json::to_vec(&graph)`: the field evidence and this ticket's
    /// deliverables are about `ProjectionBasis`'s own unbounded growth
    /// (78% of the field artifact's bytes), and `TOUCHED_FACTS` fixes the
    /// node count here, so a whole-graph byte bound would pass trivially
    /// regardless of whether per-item compaction works. This does NOT pin
    /// `Arc<ProjectionBasis>` sharing across items one patch touches
    /// (deliverable (d)): serde's `rc` feature serializes the pointee per
    /// item, so sharing vs. independent deep clones is byte-identical on
    /// the wire — see
    /// `apply_patch_shares_one_basis_arc_across_every_touched_note_and_graph_item`
    /// for that separate, `Arc::ptr_eq`-based pin.
    #[test]
    fn materialized_artifact_basis_size_stays_bounded_as_a_long_session_grows_past_the_hot_window()
    {
        let mut ledger = TranscriptLedger::new("session-cfa1-size-regression");
        let mut graph = MaterializedGraph::new("session-cfa1-size-regression");
        const TOUCHED_FACTS: u64 = 5;
        const TICKS: u64 = 80;
        let mut per_tick_basis_bytes: Vec<usize> = Vec::new();

        for tick in 0..TICKS {
            // A new final transcript span arrives every tick —
            // `summarized_through_revision` keeps advancing once the ledger
            // outgrows the hot window.
            let span_id = format!("span-{tick}");
            ledger
                .apply_event(TranscriptEvent::from(provider_payload(
                    "openrouter",
                    "system-default",
                    &span_id,
                    None,
                    tick + 1,
                    "hello from the field session",
                    true,
                )))
                .expect("final span accepted");

            let basis = ledger.current_projection_basis();
            // Touch the SAME small set of facts over and over — the exact
            // field pattern (one node revised on nearly every tick).
            let fact_id = format!("fact-{}", tick % TOUCHED_FACTS);
            let patch = ProjectionPatch {
                route: None,
                sequence: tick + 1,
                kind: ProjectionKind::Graph,
                llm_request_id: format!("llm-graph-req-{tick}"),
                basis,
                // Distinct name per fact (not a shared "Repeatedly touched
                // fact" name) so `MaterializedGraph::upsert_node`'s
                // fuzzy-name/entity_type tier-2 matching cannot merge these
                // 5 distinct facts into one node — each id keeps its own row.
                operations: vec![ProjectionOperation::UpsertGraphNode {
                    id: fact_id.clone(),
                    name: format!("Repeatedly touched fact {}", tick % TOUCHED_FACTS),
                    entity_type: "fact".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                }],
                confidence: 0.9,
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
                basis_currency_at_apply: None,
                created_at_ms: 1_700_000_000_000 + tick,
            };
            graph
                .apply_patch(&patch, None)
                .expect("graph patch applies");

            let touched = graph
                .nodes
                .iter()
                .find(|node| node.id == fact_id)
                .expect("touched node exists");
            let bytes = serde_json::to_vec(&touched.basis)
                .expect("basis serializes")
                .len();
            per_tick_basis_bytes.push(bytes);
        }

        // Deliverable (a): once the ledger has outgrown the hot window,
        // every subsequent per-item basis stays SMALL and CONSTANT — not
        // proportional to how many ticks have elapsed. Compare the last ten
        // ticks (deep past the hot window) against a small fixed ceiling.
        let late_window = &per_tick_basis_bytes[per_tick_basis_bytes.len() - 10..];
        let max_late = *late_window.iter().max().unwrap();
        let min_late = *late_window.iter().min().unwrap();
        assert!(
            max_late < 700,
            "compacted per-item basis size should stay small and bounded regardless of \
             session length, got {max_late} bytes for a basis built {TICKS} ticks into the \
             session — span_revisions is no longer being truncated at \
             summarized_through_revision"
        );
        assert_eq!(
            max_late, min_late,
            "once past the hot window every per-item basis should serialize to the SAME \
             bounded size (fixed tail length + fixed-shape digest), not grow with tick count"
        );

        // Mutation-proof cross-check: reconstruct what the FINAL basis would
        // have serialized to WITHOUT compaction (every covered span named
        // individually in `span_revisions`, `covered_prefix: None`) and
        // prove the actual, compacted artifact is dramatically smaller. If
        // compaction is ever disabled, the actual basis converges to this
        // uncompacted shape and this assertion fails.
        let uncompacted_equivalent = ProjectionBasis {
            span_revisions: (0..TICKS)
                .map(|tick| ProjectionBasisSpan {
                    span_id: format!("span-{tick}"),
                    revision_number: tick + 1,
                })
                .collect(),
            covered_prefix: None,
            diarization_span_revisions: Vec::new(),
            transcript_hash: "irrelevant-for-size-comparison".to_string(),
            summarized_through_revision: None,
        };
        let uncompacted_bytes = serde_json::to_vec(&uncompacted_equivalent)
            .expect("uncompacted comparison basis serializes")
            .len();
        assert!(
            max_late.saturating_mul(4) < uncompacted_bytes,
            "compacted basis ({max_late} bytes) should be far smaller than the pre-fix \
             embed-every-covered-span shape ({uncompacted_bytes} bytes for the same \
             {TICKS}-span session)"
        );
    }

    /// audio-graph-cfa1 deliverable (d): `MaterializedNotes::apply_patch`
    /// and `MaterializedGraph::apply_patch` each build exactly ONE
    /// `Arc<ProjectionBasis>` per patch and share it (a refcount bump) across
    /// every note/node the patch touches, instead of each one independently
    /// deep-cloning `patch.basis`. Nothing else in this suite observes that
    /// directly: `serde`'s `rc` feature serializes the pointee per item
    /// (wire bytes identical to independent deep clones) and
    /// `ProjectionBasis`'s derived `PartialEq` compares values, not
    /// pointers, so a regression back to per-item cloning would pass every
    /// other test in this file. This pins the sharing itself via
    /// `Arc::ptr_eq`.
    #[test]
    fn apply_patch_shares_one_basis_arc_across_every_touched_note_and_graph_item() {
        let basis = ProjectionBasis::from_transcript_events(&[TranscriptEvent::from(asr_payload(
            "span-1",
            1,
            "shared basis source",
        ))]);

        let mut notes = MaterializedNotes::new("session-1");
        let notes_patch = ProjectionPatch {
            route: None,
            sequence: 1,
            kind: ProjectionKind::Notes,
            llm_request_id: "llm-req-shared-notes".to_string(),
            basis: basis.clone(),
            operations: vec![
                ProjectionOperation::UpsertNote {
                    id: "note-1".to_string(),
                    title: "First".to_string(),
                    body: "First body".to_string(),
                    tags: vec![],
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                    heading_level: None,
                },
                ProjectionOperation::UpsertNote {
                    id: "note-2".to_string(),
                    title: "Second".to_string(),
                    body: "Second body".to_string(),
                    tags: vec![],
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                    heading_level: None,
                },
            ],
            confidence: 0.9,
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
            basis_currency_at_apply: None,
            created_at_ms: 1_700_000_000_000,
        };
        notes
            .apply_patch(&notes_patch, None)
            .expect("notes patch applies");
        let note_1 = notes
            .notes
            .iter()
            .find(|note| note.id == "note-1")
            .expect("note-1 exists");
        let note_2 = notes
            .notes
            .iter()
            .find(|note| note.id == "note-2")
            .expect("note-2 exists");
        assert!(
            Arc::ptr_eq(&note_1.basis, &note_2.basis),
            "two notes touched by the SAME patch must share one Arc<ProjectionBasis>, not each \
             hold an independently cloned copy"
        );

        let mut graph = MaterializedGraph::new("session-1");
        let graph_patch = ProjectionPatch {
            route: None,
            sequence: 1,
            kind: ProjectionKind::Graph,
            llm_request_id: "llm-req-shared-graph".to_string(),
            basis,
            operations: vec![
                ProjectionOperation::UpsertGraphNode {
                    id: "node-1".to_string(),
                    name: "Node One".to_string(),
                    entity_type: "fact".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
                ProjectionOperation::UpsertGraphNode {
                    id: "node-2".to_string(),
                    name: "Node Two".to_string(),
                    entity_type: "fact".to_string(),
                    description: None,
                    evidence: crate::claim_evidence::EvidenceAnchor::default(),
                },
            ],
            confidence: 0.9,
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
            basis_currency_at_apply: None,
            created_at_ms: 1_700_000_000_000,
        };
        graph
            .apply_patch(&graph_patch, None)
            .expect("graph patch applies");
        let node_1 = graph
            .nodes
            .iter()
            .find(|node| node.id == "node-1")
            .expect("node-1 exists");
        let node_2 = graph
            .nodes
            .iter()
            .find(|node| node.id == "node-2")
            .expect("node-2 exists");
        assert!(
            Arc::ptr_eq(&node_1.basis, &node_2.basis),
            "two graph nodes touched by the SAME patch must share one Arc<ProjectionBasis>, not \
             each hold an independently cloned copy"
        );
    }
}
