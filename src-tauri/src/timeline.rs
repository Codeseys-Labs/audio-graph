//! Session-timeline read-model fold (epic 0d72 P1, ADR-0026 §4.1).
//!
//! A read-only derived projection — **no new events, no persistence, no schema
//! migration**. [`build_session_timeline`] folds the three already-event-sourced
//! in-memory structures the app maintains per session into an ordered,
//! speaker-attributed, provenance-linked [`TimelineEntry`] list answering
//! "who said what, when, and in relation to what":
//!
//! - [`TranscriptLedger`] — the surviving spans (latest revision per `span_id`,
//!   the ledger already collapses partials), the `text`/timing/turn backbone.
//! - [`SpeakerTimeline`] — the diarization latest-revision-wins attribution that
//!   overrides the untrusted inline ASR label (the same join the frontend
//!   `speakerTimeline.ts` does, done backend per ADR-0026 F3 so a *loaded*
//!   session resolves trustworthy speakers without the store-side gap).
//! - live [`TemporalKnowledgeGraph`] — the per-utterance "relates to" links,
//!   grouped by [`TemporalEdge::source_segment_id`](crate::graph::temporal).
//!
//! **Why the LIVE graph, not the materialized graph (ADR-0026 §4.1, sev4
//! critique).** Only the live `TemporalEdge` carries `source_segment_id`; the
//! `MaterializedGraphEdge` carries only the whole-window `basis`. Folding the
//! per-utterance edge join over the materialized graph would leave every
//! `related_edge_ids` silently empty. The materialized graph is reserved for the
//! Analysis as-of scrubber (a later phase), which needs the bitemporal
//! `valid_from_ms`/`valid_until_ms` the live graph lacks — it is *not* an input
//! to this fold.
//!
//! **Media-time sort** (by `start_time`) is the backbone: transcript and
//! diarization persist to separate JSONL logs with no merged wall-clock stream,
//! so media time is the only axis both share. The build is `O(spans + edges)`.
//!
//! **Privacy (ADR-0026 F2).** [`TimelineEntry`] carries raw `text` +
//! `speaker_label`, so — like [`TranscriptEvent`](crate::projections) and
//! [`DiarizationSpanRevision`](crate::projections) — it hand-implements a
//! redacting [`fmt::Debug`] (`text`/`speaker_label` → `<redacted>`) rather than
//! `derive(Debug)`, so a who-said-what join never lands in logs/telemetry. The
//! companion data-movement ledger event the design also calls for is *not* added
//! here: no production read command emits data-movement events today (every
//! `DataMovementLedgerBuilder` use is `#[cfg(test)]`), so wiring the first
//! read-path emitter is deferred rather than introduced ad hoc for one command
//! (see the PR body).

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::graph::temporal::TemporalKnowledgeGraph;
use crate::projections::{
    DiarizationSpanRevision, REDACTED_DEBUG_VALUE, SpeakerTimeline, TranscriptLedger, millis,
};

/// One row of the session timeline: a surviving transcript span, resolved to its
/// latest-wins speaker and linked forward to the live graph edges it produced.
///
/// Carries raw `text` + `speaker_label`, so [`fmt::Debug`] is hand-implemented to
/// redact them (never `derive(Debug)`) — matching the enforced convention on
/// sibling content-bearing types.
#[derive(Clone, PartialEq, serde::Serialize)]
pub struct TimelineEntry {
    /// Stable, provider-neutral join key for the utterance.
    pub span_id: String,
    /// Media-clock start (milliseconds), the timeline axis.
    pub start_ms: i64,
    /// Media-clock end (milliseconds).
    pub end_ms: i64,
    /// Wall-clock arrival (milliseconds), for as-at ordering when needed.
    pub received_at_ms: u64,
    /// Turn grouping id, when the provider supplied one.
    pub turn_id: Option<String>,
    /// Resolved speaker id (diarization latest-wins override, else inline ASR).
    pub speaker_id: Option<String>,
    /// Resolved human-facing speaker label (redacted in `Debug`).
    pub speaker_label: Option<String>,
    /// Utterance text (redacted in `Debug`).
    pub text: String,
    /// Frontend-facing ids of the live graph edges this utterance produced
    /// (edges whose `source_segment_id` matches this span/segment id). Only live
    /// edges — a retconned/invalidated edge is excluded, matching the graph
    /// snapshot the UI renders.
    pub related_edge_ids: Vec<String>,
}

impl fmt::Debug for TimelineEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `text` is raw transcript content and `speaker_label` is a human-facing
        // (PII-bearing) name; both are redacted while stable, non-content
        // identity fields surface for debugging — mirroring `TranscriptEvent` /
        // `DiarizationSpanRevision`.
        f.debug_struct("TimelineEntry")
            .field("span_id", &self.span_id)
            .field("start_ms", &self.start_ms)
            .field("end_ms", &self.end_ms)
            .field("received_at_ms", &self.received_at_ms)
            .field("turn_id", &self.turn_id)
            .field("speaker_id", &self.speaker_id)
            .field(
                "speaker_label",
                &self.speaker_label.as_ref().map(|_| REDACTED_DEBUG_VALUE),
            )
            .field("text", &REDACTED_DEBUG_VALUE)
            .field("related_edge_ids", &self.related_edge_ids)
            .finish()
    }
}

/// Build the ordered, duplicate-free session timeline from the three
/// event-sourced structures. See the module docs for the design rationale
/// (ADR-0026 §4.1); in particular `live_graph` MUST be the live
/// [`TemporalKnowledgeGraph`] (the only structure carrying per-utterance
/// `source_segment_id`), never a materialized graph.
///
/// - One entry per surviving span (the ledger already collapses each span to its
///   latest accepted revision), ordered by media-clock `start_ms`
///   (ties: `end_ms`, then `span_id`), so the output is deterministic and
///   duplicate-free.
/// - `speaker_id`/`speaker_label` are resolved via the [`SpeakerTimeline`]
///   latest-revision-wins attribution index; a span the timeline does not
///   attribute keeps its inline ASR label.
/// - `related_edge_ids` are the live edges whose `source_segment_id` matches the
///   span's segment/span id.
pub fn build_session_timeline(
    ledger: &TranscriptLedger,
    speakers: &SpeakerTimeline,
    live_graph: &TemporalKnowledgeGraph,
) -> Vec<TimelineEntry> {
    let attribution = speaker_attribution_index(speakers);
    let edges_by_segment = live_graph.live_edges_by_source_segment();

    let mut entries: Vec<TimelineEntry> = ledger
        .latest_spans
        .iter()
        .map(|event| {
            // Candidate join keys, mirroring the frontend `resolveSegmentAttribution`
            // (segment id = `transcript_segment_id` when present, else `span_id`)
            // plus the raw `span_id` as a fallback — diarization/graph provenance
            // may cite either the transcript-segment id or the ASR span id.
            let keys = candidate_keys(
                event.transcript_segment_id.as_deref(),
                event.span_id.as_str(),
            );

            // Speaker: latest-wins diarization attribution overrides the inline
            // ASR label; an unattributed span keeps its inline speaker.
            let (speaker_id, speaker_label) = keys
                .iter()
                .find_map(|key| attribution.get(key.as_str()))
                .map(|span| (span.speaker_id.clone(), span.speaker_label.clone()))
                .unwrap_or_else(|| (event.speaker_id.clone(), event.speaker_label.clone()));

            // Forward "relates to" links: live edges grouped by source segment.
            let mut related_edge_ids: Vec<String> = Vec::new();
            let mut seen: HashSet<&str> = HashSet::new();
            for key in &keys {
                if let Some(ids) = edges_by_segment.get(key.as_str()) {
                    for id in ids {
                        if seen.insert(id.as_str()) {
                            related_edge_ids.push(id.clone());
                        }
                    }
                }
            }

            TimelineEntry {
                span_id: event.span_id.clone(),
                start_ms: millis(event.start_time),
                end_ms: millis(event.end_time),
                received_at_ms: event.received_at_ms,
                turn_id: event.turn_id.clone(),
                speaker_id,
                speaker_label,
                text: event.text.clone(),
                related_edge_ids,
            }
        })
        .collect();

    // Media-time backbone. The ledger already keeps `latest_spans` in this order,
    // but sort explicitly so the read-model contract does not depend on the
    // ledger's internal invariant.
    entries.sort_by(|a, b| {
        a.start_ms
            .cmp(&b.start_ms)
            .then(a.end_ms.cmp(&b.end_ms))
            .then(a.span_id.cmp(&b.span_id))
    });

    entries
}

/// Unique join keys for a span: its segment id (the provider's
/// `transcript_segment_id` when present, else the immutable `span_id`) followed
/// by the raw `span_id`. Order is significant (segment id preferred); duplicates
/// are dropped so an equal pair collapses to one key.
fn candidate_keys(transcript_segment_id: Option<&str>, span_id: &str) -> Vec<String> {
    let segment_key = transcript_segment_id.unwrap_or(span_id);
    let mut keys = vec![segment_key.to_string()];
    if span_id != segment_key {
        keys.push(span_id.to_string());
    }
    keys
}

/// Build the transcript-id → winning-speaker-span index from the speaker
/// timeline, mirroring the frontend `speakerAttributionIndex`.
///
/// A diarization span attributes every transcript id it lists in
/// `basis_transcript_segment_ids` / `basis_asr_span_ids`. When two spans claim
/// the same transcript id, the higher `revision_number` wins (ties broken by
/// later `received_at_ms`), so the most recent retconned attribution prevails —
/// matching the ledger's per-span latest-wins rule. `SpeakerTimeline::latest_spans`
/// is already the materialized (latest-per-`span_id`) set, so no prior
/// materialization pass is needed.
fn speaker_attribution_index(
    speakers: &SpeakerTimeline,
) -> HashMap<String, &DiarizationSpanRevision> {
    let mut winners: HashMap<String, &DiarizationSpanRevision> = HashMap::new();
    for span in &speakers.latest_spans {
        for key in span
            .basis_transcript_segment_ids
            .iter()
            .chain(span.basis_asr_span_ids.iter())
        {
            let wins = match winners.get(key.as_str()) {
                None => true,
                Some(current) => {
                    span.revision_number > current.revision_number
                        || (span.revision_number == current.revision_number
                            && span.received_at_ms > current.received_at_ms)
                }
            };
            if wins {
                winners.insert(key.clone(), span);
            }
        }
    }
    winners
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{
        AsrSpanRevisionPayload, AsrSpanStability, DiarizationSpanRevisionPayload,
        DiarizationSpanStability,
    };
    use crate::graph::entities::{ExtractedEntity, ExtractedRelation, ExtractionResult};
    use crate::projections::{TranscriptEvent, TranscriptLedger};

    /// A transcript span revision. `segment_id` is the provider's
    /// `transcript_segment_id` (also the id the graph edge's `source_segment_id`
    /// carries and the diarization basis cites) — the join key across all three
    /// structures.
    #[allow(clippy::too_many_arguments)]
    fn transcript_event(
        span_id: &str,
        segment_id: &str,
        revision_number: u64,
        speaker_id: &str,
        speaker_label: &str,
        text: &str,
        start_time: f64,
        turn_id: &str,
    ) -> TranscriptEvent {
        let is_final = revision_number > 1;
        TranscriptEvent::from(AsrSpanRevisionPayload {
            span_id: span_id.to_string(),
            provider: "openai_realtime".to_string(),
            source_id: "system-default".to_string(),
            provider_item_id: Some(format!("{span_id}-item")),
            transcript_segment_id: Some(segment_id.to_string()),
            speaker_id: Some(speaker_id.to_string()),
            speaker_label: Some(speaker_label.to_string()),
            channel: Some("mixed".to_string()),
            text: text.to_string(),
            start_time,
            end_time: start_time + 1.0,
            confidence: 0.9,
            is_final,
            stability: if is_final {
                AsrSpanStability::Final
            } else {
                AsrSpanStability::Partial
            },
            revision_number,
            supersedes: (revision_number > 1)
                .then(|| format!("{span_id}@rev{}", revision_number - 1)),
            turn_id: Some(turn_id.to_string()),
            end_of_turn: is_final,
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000 + revision_number,
        })
    }

    /// A diarization span revision that attributes the transcript span whose
    /// segment id is `basis_segment_id`.
    fn diarization_event(
        span_id: &str,
        basis_segment_id: &str,
        revision_number: u64,
        speaker_id: &str,
        speaker_label: &str,
        stability: DiarizationSpanStability,
    ) -> DiarizationSpanRevision {
        let is_final = matches!(stability, DiarizationSpanStability::Final);
        DiarizationSpanRevision::from(DiarizationSpanRevisionPayload {
            span_id: span_id.to_string(),
            provider: "local_clustering".to_string(),
            timeline_id: "session".to_string(),
            source_id: None,
            speaker_id: Some(speaker_id.to_string()),
            speaker_label: Some(speaker_label.to_string()),
            channel: None,
            start_time: 1.0,
            end_time: 2.0,
            confidence: Some(0.8),
            is_final,
            stability,
            revision_number,
            supersedes: (revision_number > 1)
                .then(|| format!("{span_id}@rev{}", revision_number - 1)),
            basis_asr_span_ids: Vec::new(),
            basis_transcript_segment_ids: vec![basis_segment_id.to_string()],
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_000_000 + revision_number,
        })
    }

    fn ledger_from(events: Vec<TranscriptEvent>) -> TranscriptLedger {
        TranscriptLedger::replay("session-timeline-test", events).expect("replay transcript ledger")
    }

    fn speakers_from(events: Vec<DiarizationSpanRevision>) -> SpeakerTimeline {
        SpeakerTimeline::replay("session-timeline-test", events).expect("replay speaker timeline")
    }

    fn extraction(source: &str, target: &str, relation: &str) -> ExtractionResult {
        ExtractionResult {
            entities: vec![
                ExtractedEntity {
                    name: source.to_string(),
                    entity_type: "Person".to_string(),
                    description: None,
                },
                ExtractedEntity {
                    name: target.to_string(),
                    entity_type: "Topic".to_string(),
                    description: None,
                },
            ],
            relations: vec![ExtractedRelation {
                source: source.to_string(),
                target: target.to_string(),
                relation_type: relation.to_string(),
                detail: None,
            }],
        }
    }

    #[test]
    fn folds_latest_revision_per_span_ordered_and_duplicate_free() {
        // span-b arrives first in the log but starts later in media time; span-a
        // has a partial superseded by its final revision.
        let ledger = ledger_from(vec![
            transcript_event(
                "span-b", "seg-b", 1, "spk-2", "Bob", "second", 10.0, "turn-2",
            ),
            transcript_event("span-a", "seg-a", 1, "spk-1", "Alice", "hi", 1.0, "turn-1"),
            transcript_event(
                "span-a",
                "seg-a",
                2,
                "spk-1",
                "Alice",
                "hello there",
                1.0,
                "turn-1",
            ),
        ]);
        let speakers = speakers_from(Vec::new());
        let graph = TemporalKnowledgeGraph::new();

        let timeline = build_session_timeline(&ledger, &speakers, &graph);

        // Duplicate-free: one entry per surviving span (span-a collapsed).
        assert_eq!(timeline.len(), 2);
        // Ordered by media-clock start_ms, not log arrival order.
        assert_eq!(timeline[0].span_id, "span-a");
        assert_eq!(timeline[1].span_id, "span-b");
        // Latest revision wins for the collapsed span.
        assert_eq!(timeline[0].text, "hello there");
        assert_eq!(timeline[0].start_ms, 1_000);
        assert_eq!(timeline[0].end_ms, 2_000);
        assert_eq!(timeline[0].turn_id.as_deref(), Some("turn-1"));
    }

    #[test]
    fn resolves_speaker_via_timeline_with_mid_session_relabel() {
        let ledger = ledger_from(vec![
            transcript_event(
                "span-a",
                "seg-a",
                2,
                "asr-x",
                "ASR Guess",
                "hi",
                1.0,
                "turn-1",
            ),
            transcript_event(
                "span-b",
                "seg-b",
                2,
                "asr-y",
                "ASR Guess",
                "bye",
                5.0,
                "turn-2",
            ),
        ]);
        // Diarization first attributes seg-a provisionally, then a corrected
        // (higher-revision) relabel wins latest-revision-wins.
        let speakers = speakers_from(vec![
            diarization_event(
                "dia-a",
                "seg-a",
                1,
                "spk-prov",
                "Speaker 2",
                DiarizationSpanStability::Provisional,
            ),
            diarization_event(
                "dia-a",
                "seg-a",
                2,
                "spk-alice",
                "Alice",
                DiarizationSpanStability::Final,
            ),
            diarization_event(
                "dia-b",
                "seg-b",
                1,
                "spk-bob",
                "Bob",
                DiarizationSpanStability::Final,
            ),
        ]);
        let graph = TemporalKnowledgeGraph::new();

        let timeline = build_session_timeline(&ledger, &speakers, &graph);

        // seg-a: the corrected (latest-revision) diarization attribution overrides
        // both the inline ASR label AND the earlier provisional attribution.
        assert_eq!(timeline[0].span_id, "span-a");
        assert_eq!(timeline[0].speaker_id.as_deref(), Some("spk-alice"));
        assert_eq!(timeline[0].speaker_label.as_deref(), Some("Alice"));
        // seg-b resolves to its (only) attribution.
        assert_eq!(timeline[1].speaker_id.as_deref(), Some("spk-bob"));
        assert_eq!(timeline[1].speaker_label.as_deref(), Some("Bob"));
    }

    #[test]
    fn unattributed_span_keeps_inline_speaker() {
        let ledger = ledger_from(vec![transcript_event(
            "span-a",
            "seg-a",
            2,
            "asr-inline",
            "Inline Label",
            "hi",
            1.0,
            "turn-1",
        )]);
        // Timeline attributes a different segment, so span-a is unresolved.
        let speakers = speakers_from(vec![diarization_event(
            "dia-z",
            "seg-other",
            1,
            "spk-z",
            "Zed",
            DiarizationSpanStability::Final,
        )]);
        let graph = TemporalKnowledgeGraph::new();

        let timeline = build_session_timeline(&ledger, &speakers, &graph);

        assert_eq!(timeline[0].speaker_id.as_deref(), Some("asr-inline"));
        assert_eq!(timeline[0].speaker_label.as_deref(), Some("Inline Label"));
    }

    #[test]
    fn groups_live_edges_by_source_segment_onto_the_right_entries() {
        let ledger = ledger_from(vec![
            transcript_event(
                "span-a",
                "seg-a",
                2,
                "spk-1",
                "Alice",
                "we ship friday",
                1.0,
                "t1",
            ),
            transcript_event(
                "span-b",
                "seg-b",
                2,
                "spk-2",
                "Bob",
                "sounds good",
                5.0,
                "t2",
            ),
        ]);
        let speakers = speakers_from(Vec::new());

        let mut graph = TemporalKnowledgeGraph::new();
        // Two edges cite seg-a; one cites seg-b.
        graph.process_extraction(
            &extraction("Alice", "Friday", "ships_on"),
            1.0,
            "Alice",
            "seg-a",
        );
        graph.process_extraction(
            &extraction("Alice", "Ship", "mentions"),
            1.0,
            "Alice",
            "seg-a",
        );
        graph.process_extraction(&extraction("Bob", "Plan", "approves"), 5.0, "Bob", "seg-b");

        let timeline = build_session_timeline(&ledger, &speakers, &graph);

        let entry_a = timeline.iter().find(|e| e.span_id == "span-a").unwrap();
        let entry_b = timeline.iter().find(|e| e.span_id == "span-b").unwrap();
        assert_eq!(
            entry_a.related_edge_ids.len(),
            2,
            "both seg-a edges land on span-a"
        );
        assert_eq!(
            entry_b.related_edge_ids.len(),
            1,
            "the seg-b edge lands on span-b"
        );
        // No cross-contamination.
        for id in &entry_b.related_edge_ids {
            assert!(!entry_a.related_edge_ids.contains(id));
        }
    }

    #[test]
    fn invalidated_edge_is_excluded_from_related_ids() {
        let ledger = ledger_from(vec![transcript_event(
            "span-a",
            "seg-a",
            2,
            "Speaker 2",
            "Speaker 2",
            "hello",
            1.0,
            "t1",
        )]);
        let speakers = speakers_from(Vec::new());

        let mut graph = TemporalKnowledgeGraph::new();
        // An edge attributed to the provisional "Speaker 2" entity on seg-a.
        graph.process_extraction(
            &extraction("Speaker 2", "Deadline", "mentions"),
            1.0,
            "Speaker 2",
            "seg-a",
        );
        let before = build_session_timeline(&ledger, &speakers, &graph);
        assert_eq!(
            before[0].related_edge_ids.len(),
            1,
            "live edge is linked pre-retcon"
        );

        // Retcon: merge "Speaker 2" into "Alice". The original seg-a edge is
        // invalidated (valid_until set) and re-pointed onto the canonical node
        // with a fresh source_segment_id-less identity carried from the repoint.
        graph.process_extraction(
            &extraction("Alice", "Deadline", "mentions"),
            2.0,
            "Alice",
            "seg-later",
        );
        let invalidated = graph.supersede_entity("Speaker 2", "Alice", 3.0, 1.0);
        assert!(invalidated > 0, "the retcon must invalidate the seg-a edge");

        let after = build_session_timeline(&ledger, &speakers, &graph);
        // The invalidated edge no longer surfaces on span-a; the re-pointed live
        // edge carries seg-later (not seg-a), so span-a ends up with no links.
        assert!(
            after[0].related_edge_ids.is_empty(),
            "an invalidated (retconned) edge must be excluded, got {:?}",
            after[0].related_edge_ids
        );
    }

    #[test]
    fn debug_redacts_text_and_speaker_label_but_keeps_identity() {
        let ledger = ledger_from(vec![transcript_event(
            "span-a",
            "seg-a",
            2,
            "spk-1",
            "SENSITIVE NAME",
            "SENSITIVE TRANSCRIPT CONTENT",
            1.0,
            "turn-1",
        )]);
        let speakers = speakers_from(Vec::new());
        let graph = TemporalKnowledgeGraph::new();

        let timeline = build_session_timeline(&ledger, &speakers, &graph);
        let debug = format!("{:?}", timeline[0]);

        // Content fields are redacted.
        assert!(!debug.contains("SENSITIVE TRANSCRIPT CONTENT"));
        assert!(!debug.contains("SENSITIVE NAME"));
        assert!(debug.contains("<redacted>"));
        // Stable, non-content identity fields still surface for debugging.
        assert!(debug.contains("span-a"));
        assert!(debug.contains("spk-1"));
    }
}
