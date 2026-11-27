//! Temporal knowledge graph implementation using petgraph.
//!
//! The graph uses `StableGraph` for stable node/edge indices across mutations.
//! Each edge carries temporal metadata (valid_from, valid_until) for
//! time-aware relationship tracking.

use petgraph::stable_graph::{EdgeIndex, NodeIndex, StableGraph};
use petgraph::visit::EdgeRef;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::Path;

use super::entities::{
    ExtractedEntity, ExtractedRelation, ExtractionResult, GraphDelta, GraphEdge, GraphEntity,
    GraphLink, GraphNode, GraphSnapshot, GraphStats, SpeakerEndpointMask, SpeakerMention,
    entity_type_color, relation_type_color,
};

/// Edge data in the temporal graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalEdge {
    /// Monotonic, never-reused sequence id assigned when this edge is first
    /// created. The frontend-facing link id derives from THIS (see
    /// [`edge_link_id`]) — never from the petgraph `EdgeIndex`, which
    /// `StableGraph` recycles after an edge is removed. Without this, an
    /// evicted edge's freed index could be reused by a later, unrelated edge
    /// and re-emit the same `edge-N` id, silently merging two relations in the
    /// frontend across delta windows.
    ///
    /// `#[serde(default)]` so graphs saved before this field existed still
    /// load; `load_from_file` re-derives a fresh, collision-free seq for every
    /// edge on load regardless of the persisted value.
    #[serde(default)]
    pub seq_id: u64,
    pub relation_type: String,
    pub valid_from: f64,
    pub valid_until: Option<f64>,
    pub confidence: f32,
    pub source_segment_id: String,
    pub detail: Option<String>,
    /// Strength — incremented on repeated mentions of the same relation.
    pub weight: f32,
    /// Every observation represented by the folded edge. Legacy graphs have only the seek hint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contributions: Option<Vec<EdgeContribution>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeContribution {
    pub source_segment_id: Option<String>,
    pub speaker_endpoint_mask: SpeakerEndpointMask,
    pub weight: f32,
    pub observed_at: f64,
    pub confidence: f32,
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_speaker_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_speaker_label: Option<String>,
}

fn attributed_node(
    mask: SpeakerEndpointMask,
    source: NodeIndex,
    target: NodeIndex,
) -> Option<NodeIndex> {
    match mask {
        SpeakerEndpointMask::Source => Some(source),
        SpeakerEndpointMask::Target => Some(target),
        SpeakerEndpointMask::Both if source == target => Some(source),
        SpeakerEndpointMask::Both | SpeakerEndpointMask::Neither => None,
    }
}

impl TemporalEdge {
    /// Segment keys for every observation (or the sole legacy seek hint).
    pub fn all_source_segment_ids(&self) -> impl Iterator<Item = &str> {
        self.contributions
            .iter()
            .flatten()
            .filter_map(|observation| observation.source_segment_id.as_deref())
            .chain(
                (self.contributions.is_none() && !self.source_segment_id.is_empty())
                    .then_some(self.source_segment_id.as_str()),
            )
    }

    fn legacy_contribution(&self) -> EdgeContribution {
        EdgeContribution {
            source_segment_id: (!self.source_segment_id.is_empty())
                .then(|| self.source_segment_id.clone()),
            // No roles were recorded. Preserve the legacy seek-hint fallback
            // when this opaque aggregate is later folded into a modern edge.
            speaker_endpoint_mask: SpeakerEndpointMask::Both,
            weight: self.weight,
            observed_at: self.valid_from,
            confidence: self.confidence,
            detail: self.detail.clone(),
            origin_speaker_label: None,
            current_speaker_label: None,
        }
    }

    fn complete_contributions(&mut self) -> &mut Vec<EdgeContribution> {
        if self.contributions.is_none() {
            self.contributions = Some(vec![self.legacy_contribution()]);
        }
        self.contributions
            .as_mut()
            .expect("contributions initialized")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelabelScope {
    Entity,
    Segments { segment_ids: BTreeSet<String> },
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SpeakerReattributionResult {
    pub edges_retconned: usize,
    pub entities_updated: usize,
    pub renamed_in_place: bool,
    pub merged: bool,
}

impl SpeakerReattributionResult {
    pub fn changed(&self) -> bool {
        self.renamed_in_place
            || self.merged
            || self.edges_retconned > 0
            || self.entities_updated > 0
    }
}

fn same_name(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
        || ((!a.is_ascii() || !b.is_ascii()) && a.to_lowercase() == b.to_lowercase())
}

fn record_speaker_mention(node: &mut GraphEntity, speaker: &str, segment_id: Option<&str>) {
    node.speaker_mentions
        .get_or_insert_with(|| {
            node.speakers
                .iter()
                .map(|speaker_label| SpeakerMention {
                    segment_id: None,
                    speaker_label: speaker_label.clone(),
                })
                .collect()
        })
        .push(SpeakerMention {
            segment_id: segment_id.map(str::to_owned),
            speaker_label: speaker.to_owned(),
        });
    if !node.speakers.iter().any(|label| label == speaker) {
        node.speakers.push(speaker.to_owned());
    }
}

fn rebuild_speakers(node: &mut GraphEntity) {
    node.speakers = node
        .speaker_mentions
        .iter()
        .flatten()
        .map(|mention| mention.speaker_label.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(str::to_owned)
        .collect();
}

/// Maximum number of nodes before eviction of oldest (lowest `last_seen`).
const MAX_NODES: usize = 1000;

/// Maximum number of edges before eviction of oldest (lowest `valid_from`).
///
/// `pub(crate)` (widened from private for audio-graph-9b11) so the live
/// diarization-dispatch eviction regression test in `speech/mod.rs` can drive
/// the real threshold instead of duplicating it as a second magic number that
/// could silently drift out of sync with this one.
pub(crate) const MAX_EDGES: usize = 5000;

/// A temporal knowledge graph backed by petgraph's StableGraph.
pub struct TemporalKnowledgeGraph {
    /// The underlying petgraph graph.
    graph: StableGraph<GraphEntity, TemporalEdge>,
    /// Index from entity name (lowercased) to node index.
    name_index: HashMap<String, NodeIndex>,
    /// Number of episodes (i.e. `process_extraction` calls) processed. This is
    /// surfaced to the UI as `total_episodes` and MUST count episodes only —
    /// it used to also be bumped per new edge, conflating episodes with edges.
    event_counter: u64,
    /// Monotonic counter for assigning [`TemporalEdge::seq_id`]. Never reused,
    /// so frontend-facing edge ids stay unique even after eviction recycles a
    /// petgraph `EdgeIndex`.
    edge_seq_counter: u64,

    // -- Delta tracking state --------------------------------------------------
    /// IDs of nodes added since the last `take_delta()` call.
    delta_added_node_ids: Vec<String>,
    /// IDs of nodes updated (but not newly added) since the last `take_delta()`.
    delta_updated_node_ids: Vec<String>,
    /// (source_idx, target_idx, edge_idx) of edges added since last delta.
    delta_added_edge_indices: Vec<petgraph::graph::EdgeIndex>,
    /// Indices of edges whose weight/label changed (but were not newly added)
    /// since the last delta.
    delta_updated_edge_indices: Vec<petgraph::graph::EdgeIndex>,
    /// IDs of removed (evicted) nodes since last delta.
    delta_removed_node_ids: Vec<String>,
    /// Synthetic IDs for removed (evicted) edges since last delta.
    delta_removed_edge_ids: Vec<String>,
}

/// Serializable representation of the graph for save/load.
#[derive(Serialize, Deserialize)]
struct SerializableGraph {
    nodes: Vec<GraphEntity>,
    edges: Vec<SerializableEdge>,
    event_counter: u64,
}

/// Serializable edge with source/target names.
#[derive(Serialize, Deserialize)]
struct SerializableEdge {
    source_name: String,
    target_name: String,
    edge: TemporalEdge,
}

/// Build the stable, frontend-facing link id for an edge from its monotonic
/// [`TemporalEdge::seq_id`].
///
/// This MUST be the single source of truth for edge ids: snapshot links,
/// delta `added_edges`/`updated_edges`, and delta `removed_edge_ids` all derive
/// from it so that incremental removals/updates match the ids the frontend
/// already holds.
///
/// The id derives from the edge's `seq_id`, NOT from the petgraph `EdgeIndex`.
/// `StableGraph` recycles a removed edge's index, so an `EdgeIndex`-derived id
/// could be re-emitted for a later, unrelated edge — silently merging two
/// relations under one id in the frontend across delta windows. The monotonic
/// `seq_id` is never reused, so each edge keeps a distinct, stable id for life.
fn edge_link_id(seq_id: u64) -> String {
    format!("edge-{seq_id}")
}

impl TemporalKnowledgeGraph {
    /// Create a new empty temporal knowledge graph.
    pub fn new() -> Self {
        Self {
            graph: StableGraph::new(),
            name_index: HashMap::new(),
            event_counter: 0,
            edge_seq_counter: 0,
            delta_added_node_ids: Vec::new(),
            delta_updated_node_ids: Vec::new(),
            delta_added_edge_indices: Vec::new(),
            delta_updated_edge_indices: Vec::new(),
            delta_removed_node_ids: Vec::new(),
            delta_removed_edge_ids: Vec::new(),
        }
    }

    /// Get the current number of nodes in the graph.
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Get the current number of edges in the graph.
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Test-only introspection: the `valid_from` of the live (`valid_until ==
    /// None`) edge of the given `relation_type` connecting `source_name` to
    /// `target_name`, by name (case-insensitive), if any. `GraphLink` (the
    /// public snapshot type) does not surface `valid_from`/`valid_until`, so
    /// callers outside this module have no other way to assert what
    /// timestamp a producer (e.g. `commands.rs::merge_graph_entities`)
    /// actually fed into `supersede_entity`/`add_relation` (audio-graph-4b52
    /// coverage-gap fix).
    #[cfg(test)]
    pub(crate) fn live_edge_valid_from_for_test(
        &self,
        source_name: &str,
        target_name: &str,
        relation_type: &str,
    ) -> Option<f64> {
        let source_idx = *self.name_index.get(&source_name.to_lowercase())?;
        let target_idx = *self.name_index.get(&target_name.to_lowercase())?;
        self.graph
            .edges_connecting(source_idx, target_idx)
            .find(|e| e.weight().relation_type == relation_type && e.weight().valid_until.is_none())
            .map(|e| e.weight().valid_from)
    }

    /// Get the current episode count.
    pub fn episode_count(&self) -> u64 {
        self.event_counter
    }

    /// Group the stable link ids of every live edge by all contributing segment
    /// ids. The live graph carries per-observation provenance, unlike the
    /// materialized graph's whole-window basis (ADR-0026 §4.1).
    ///
    /// Only live edges (`valid_until == None`) are included, matching
    /// [`Self::snapshot`]'s link filter — a retconned/invalidated edge is excluded
    /// so the timeline's forward links agree with the graph the UI renders. The
    /// ids within each group are sorted by the monotonic `seq_id` for
    /// deterministic output.
    pub fn live_edges_by_source_segment(&self) -> HashMap<String, Vec<String>> {
        let mut by_segment: HashMap<&str, Vec<u64>> = HashMap::new();
        for edge in self
            .graph
            .edge_weights()
            .filter(|edge| edge.valid_until.is_none())
        {
            for segment in edge.all_source_segment_ids() {
                let ids = by_segment.entry(segment).or_default();
                // Observations of the same edge can repeat a segment key.
                if ids.last() != Some(&edge.seq_id) {
                    ids.push(edge.seq_id);
                }
            }
        }
        by_segment
            .into_iter()
            .map(|(segment, mut ids)| {
                ids.sort_unstable();
                (
                    segment.to_owned(),
                    ids.into_iter().map(edge_link_id).collect(),
                )
            })
            .collect()
    }

    /// Add or update an entity. If entity name exists (case-insensitive),
    /// update `last_seen` and increment `mention_count`. Returns the `NodeIndex`.
    pub fn add_entity(
        &mut self,
        entity: &ExtractedEntity,
        timestamp: f64,
        speaker: &str,
    ) -> NodeIndex {
        self.add_entity_with_segment(entity, timestamp, speaker, None)
    }

    fn add_entity_with_segment(
        &mut self,
        entity: &ExtractedEntity,
        timestamp: f64,
        speaker: &str,
        segment_id: Option<&str>,
    ) -> NodeIndex {
        let key = entity.name.to_lowercase();
        let idx = match self.name_index.get(&key) {
            Some(&idx) => idx,
            None => self.insert_entity(entity, key, timestamp),
        };
        let node = &mut self.graph[idx];
        node.last_seen = timestamp;
        node.mention_count += 1;
        record_speaker_mention(node, speaker, segment_id);
        if node.description.is_none() {
            node.description.clone_from(&entity.description);
        }
        self.track_updated_node(idx);
        idx
    }

    fn insert_entity(
        &mut self,
        entity: &ExtractedEntity,
        key: String,
        timestamp: f64,
    ) -> NodeIndex {
        let id = uuid::Uuid::new_v4().to_string();
        self.delta_added_node_ids.push(id.clone());
        let idx = self.graph.add_node(GraphEntity {
            id,
            name: entity.name.clone(),
            entity_type: entity.entity_type.clone(),
            mention_count: 0,
            first_seen: timestamp,
            last_seen: timestamp,
            aliases: vec![],
            description: None,
            speakers: vec![],
            person_ref: None,
            speaker_mentions: Some(Vec::new()),
        });
        self.name_index.insert(key, idx);
        idx
    }

    fn track_updated_node(&mut self, idx: NodeIndex) {
        let id = &self.graph[idx].id;
        if !self.delta_added_node_ids.contains(id) && !self.delta_updated_node_ids.contains(id) {
            self.delta_updated_node_ids.push(id.clone());
        }
    }

    /// Add a relation between two entities. If the same relation type already
    /// exists between them, increment weight instead of creating a duplicate.
    pub fn add_relation(
        &mut self,
        source_name: &str,
        target_name: &str,
        relation: &ExtractedRelation,
        timestamp: f64,
        segment_id: &str,
    ) {
        if let Some((source, target)) =
            self.add_relation_observation(source_name, target_name, relation, timestamp, segment_id)
        {
            let Some(speaker) = attributed_node(relation.speaker_endpoint_mask, source, target)
            else {
                return;
            };
            let label = self.graph[speaker].name.clone();
            let segment = (!segment_id.is_empty()).then_some(segment_id);
            record_speaker_mention(&mut self.graph[source], &label, segment);
            self.track_updated_node(source);
            if target != source {
                record_speaker_mention(&mut self.graph[target], &label, segment);
                self.track_updated_node(target);
            }
        }
    }

    fn add_relation_observation(
        &mut self,
        source_name: &str,
        target_name: &str,
        relation: &ExtractedRelation,
        timestamp: f64,
        segment_id: &str,
    ) -> Option<(NodeIndex, NodeIndex)> {
        let Some(&source) = self.name_index.get(&source_name.to_lowercase()) else {
            log::warn!("Source entity '{}' not found in graph", source_name);
            return None;
        };
        let Some(&target) = self.name_index.get(&target_name.to_lowercase()) else {
            log::warn!("Target entity '{}' not found in graph", target_name);
            return None;
        };

        let mask = relation.speaker_endpoint_mask;
        let speaker = attributed_node(mask, source, target);
        let contribution = EdgeContribution {
            source_segment_id: (!segment_id.is_empty()).then(|| segment_id.to_owned()),
            speaker_endpoint_mask: mask,
            weight: 1.0,
            observed_at: timestamp,
            confidence: 1.0,
            detail: relation.detail.clone(),
            origin_speaker_label: speaker.map(|_| {
                if mask.source() {
                    source_name
                } else {
                    target_name
                }
                .to_owned()
            }),
            current_speaker_label: speaker.map(|idx| self.graph[idx].name.clone()),
        };
        if let Some(idx) = self.live_edge(source, target, &relation.relation_type) {
            let edge = &mut self.graph[idx];
            edge.complete_contributions().push(contribution);
            edge.weight += 1.0;
            edge.valid_from = edge.valid_from.min(timestamp);
            if edge.source_segment_id.is_empty() {
                segment_id.clone_into(&mut edge.source_segment_id);
            }
            self.track_updated_edge(idx);
        } else {
            self.insert_live_edge(
                source,
                target,
                TemporalEdge {
                    seq_id: 0,
                    relation_type: relation.relation_type.clone(),
                    valid_from: timestamp,
                    valid_until: None,
                    confidence: contribution.confidence,
                    source_segment_id: segment_id.to_owned(),
                    detail: contribution.detail.clone(),
                    weight: contribution.weight,
                    contributions: Some(vec![contribution]),
                },
            );
        }
        Some((source, target))
    }

    fn live_edge(
        &self,
        source: NodeIndex,
        target: NodeIndex,
        relation_type: &str,
    ) -> Option<EdgeIndex> {
        self.graph
            .edges_connecting(source, target)
            .find(|edge| {
                edge.weight().valid_until.is_none() && edge.weight().relation_type == relation_type
            })
            .map(|edge| edge.id())
    }

    fn insert_live_edge(&mut self, source: NodeIndex, target: NodeIndex, mut edge: TemporalEdge) {
        edge.seq_id = self.edge_seq_counter;
        self.edge_seq_counter += 1;
        let idx = self.graph.add_edge(source, target, edge);
        self.delta_added_edge_indices.push(idx);
    }

    fn track_updated_edge(&mut self, idx: EdgeIndex) {
        if !self.delta_added_edge_indices.contains(&idx)
            && !self.delta_updated_edge_indices.contains(&idx)
        {
            self.delta_updated_edge_indices.push(idx);
        }
    }

    /// Resolve an entity name using fuzzy matching (strsim).
    /// Returns `NodeIndex` if a close match is found above `threshold`.
    ///
    /// Part of the entity-resolution / edge-invalidation surface that
    /// [`Self::supersede_entity`] drives: when a diarization/entity retcon merges
    /// a provisional speaker entity into a canonical one, this is how the merge
    /// target is located by name even when the spelling drifted slightly.
    pub fn resolve_entity(&self, name: &str, threshold: f64) -> Option<NodeIndex> {
        let key = name.to_lowercase();

        // Exact match first
        if let Some(&idx) = self.name_index.get(&key) {
            return Some(idx);
        }

        // Fuzzy match
        let mut best_match: Option<(NodeIndex, f64)> = None;
        for (existing_name, &idx) in &self.name_index {
            let similarity = strsim::jaro_winkler(&key, existing_name);
            if similarity >= threshold
                && (best_match.is_none() || similarity > best_match.unwrap().1)
            {
                best_match = Some((idx, similarity));
            }
        }

        best_match.map(|(idx, _)| idx)
    }

    /// Invalidate an edge by setting its `valid_until` timestamp (Graphiti
    /// temporal concept).
    ///
    /// Snapshots and deltas hide the retired row, but persistence keeps it for audit.
    pub fn invalidate_edge(&mut self, edge_idx: petgraph::graph::EdgeIndex, timestamp: f64) {
        if let Some(edge) = self.graph.edge_weight_mut(edge_idx)
            && edge.valid_until.is_none()
        {
            edge.valid_until = Some(timestamp);
            self.delta_removed_edge_ids.push(edge_link_id(edge.seq_id));
            self.delta_added_edge_indices.retain(|&idx| idx != edge_idx);
            self.delta_updated_edge_indices
                .retain(|&idx| idx != edge_idx);
        }
    }

    /// Retcon a superseded entity into a canonical one (speaker / entity merge).
    ///
    /// Re-point live relations at `timestamp`, preserving their original
    /// observations. The retired node and its invalidated edges are kept for
    /// audit. `threshold` is the name-resolution cutoff; `1.0` is exact-only.
    ///
    /// Returns the number of edges that were invalidated (re-pointed). Returns
    /// `0` — and makes no change — when either name does not resolve, when both
    /// names resolve to the same node (nothing to merge), or when the superseded
    /// node has no live edges.
    pub fn supersede_entity(
        &mut self,
        superseded_name: &str,
        canonical_name: &str,
        timestamp: f64,
        threshold: f64,
    ) -> usize {
        let Some(from) = self.resolve_entity(superseded_name, threshold) else {
            return 0;
        };
        let Some(to) = self.resolve_entity(canonical_name, threshold) else {
            return 0;
        };
        if from == to {
            return 0;
        }
        let invalidated = self.repoint_entity_edges(from, to, timestamp);
        if invalidated > 0 {
            self.merge_entity_bookkeeping(from, to);
        }
        invalidated
    }

    /// Correct speaker attribution for an entire identity or for exact turns.
    pub fn reattribute_speaker(
        &mut self,
        from_name: &str,
        to_name: &str,
        scope: RelabelScope,
        timestamp: f64,
    ) -> SpeakerReattributionResult {
        let mut result = SpeakerReattributionResult::default();
        if from_name == to_name {
            return result;
        }
        let from = self.name_index.get(&from_name.to_lowercase()).copied();
        match scope {
            RelabelScope::Entity => {
                let Some(from) = from else {
                    return result;
                };
                let old_name = self.graph[from].name.clone();
                let to_key = to_name.to_lowercase();
                let to = self.name_index.get(&to_key).copied();
                let next_name = if let Some(to) = to.filter(|&to| to != from) {
                    result.edges_retconned = self.repoint_entity_edges(from, to, timestamp);
                    self.merge_entity_bookkeeping(from, to);
                    result.merged = true;
                    self.graph[to].name.clone()
                } else if old_name != to_name {
                    let node = &mut self.graph[from];
                    node.aliases.retain(|alias| !same_name(alias, to_name));
                    if !same_name(&old_name, to_name)
                        && !node.aliases.iter().any(|alias| same_name(alias, &old_name))
                    {
                        node.aliases.push(old_name.clone());
                    }
                    node.name = to_name.to_owned();
                    self.name_index.insert(to_key, from);
                    self.track_updated_node(from);
                    for edge in self
                        .graph
                        .edge_weights_mut()
                        .filter(|edge| edge.valid_until.is_none())
                    {
                        for contribution in edge.contributions.iter_mut().flatten() {
                            if contribution
                                .current_speaker_label
                                .as_deref()
                                .is_some_and(|label| same_name(label, &old_name))
                            {
                                contribution.current_speaker_label = Some(to_name.to_owned());
                            }
                        }
                    }
                    result.renamed_in_place = true;
                    to_name.to_owned()
                } else {
                    old_name.clone()
                };
                result.entities_updated =
                    self.reattribute_mentions(from_name, &old_name, &next_name, None);
            }
            RelabelScope::Segments { segment_ids } => {
                if same_name(from_name, to_name) || segment_ids.is_empty() {
                    return result;
                }
                if let Some(from) = from.filter(|&idx| same_name(&self.graph[idx].name, from_name))
                {
                    result.edges_retconned =
                        self.reattribute_segment_edges(from, to_name, &segment_ids, timestamp);
                }
                result.entities_updated =
                    self.reattribute_mentions(from_name, from_name, to_name, Some(&segment_ids));
                if result.edges_retconned == 0 && result.entities_updated > 0 {
                    self.get_or_create_split_target(to_name, timestamp);
                }
            }
        }
        result
    }

    fn live_incident_edges(
        &self,
        node: NodeIndex,
    ) -> impl Iterator<Item = (EdgeIndex, NodeIndex, NodeIndex, &TemporalEdge)> {
        self.graph
            .edges_directed(node, petgraph::Direction::Outgoing)
            .chain(
                self.graph
                    .edges_directed(node, petgraph::Direction::Incoming)
                    .filter(move |edge| edge.source() != node),
            )
            .filter(|edge| edge.weight().valid_until.is_none())
            .map(|edge| (edge.id(), edge.source(), edge.target(), edge.weight()))
    }

    fn repoint_entity_edges(&mut self, from: NodeIndex, to: NodeIndex, timestamp: f64) -> usize {
        let incident: Vec<_> = self
            .live_incident_edges(from)
            .map(|(idx, src, tgt, _)| (idx, src, tgt))
            .collect();
        for &(idx, src, tgt) in &incident {
            let mut edge = self.graph[idx].clone();
            self.invalidate_edge(idx, timestamp);
            let new_src = if src == from { to } else { src };
            let new_tgt = if tgt == from { to } else { tgt };
            for contribution in edge.complete_contributions() {
                let mask = contribution.speaker_endpoint_mask;
                if (src == from && mask.source()) || (tgt == from && mask.target()) {
                    contribution.current_speaker_label = attributed_node(mask, new_src, new_tgt)
                        .map(|node| self.graph[node].name.clone());
                }
            }
            self.refold_edge(new_src, new_tgt, edge, timestamp);
        }
        incident.len()
    }

    fn refold_edge(
        &mut self,
        src: NodeIndex,
        tgt: NodeIndex,
        mut incoming: TemporalEdge,
        timestamp: f64,
    ) {
        incoming.complete_contributions();
        if let Some(idx) = self.live_edge(src, tgt, &incoming.relation_type) {
            let edge = &mut self.graph[idx];
            if let Some(contributions) = incoming.contributions.take() {
                edge.complete_contributions().extend(contributions);
            }
            edge.weight += incoming.weight;
            edge.valid_from = edge.valid_from.min(incoming.valid_from);
            if edge.source_segment_id.is_empty() {
                edge.source_segment_id = incoming.source_segment_id;
            }
            self.track_updated_edge(idx);
        } else {
            // Retcon validity starts now; observations keep their original timestamps.
            incoming.valid_from = timestamp;
            incoming.valid_until = None;
            self.insert_live_edge(src, tgt, incoming);
        }
    }

    /// Fold source bookkeeping and preserve the redirect on disk via aliases.
    fn merge_entity_bookkeeping(&mut self, from: NodeIndex, to: NodeIndex) {
        let source = self.graph[from].clone();
        let canonical = &mut self.graph[to];
        canonical.mention_count = canonical.mention_count.saturating_add(source.mention_count);
        canonical.first_seen = canonical.first_seen.min(source.first_seen);
        canonical.last_seen = canonical.last_seen.max(source.last_seen);
        if canonical.description.is_none() {
            canonical.description = source.description;
        }
        if canonical.person_ref.is_none() {
            canonical.person_ref = source.person_ref;
        }
        if canonical.speaker_mentions.is_none() {
            canonical.speaker_mentions = Some(
                canonical
                    .speakers
                    .iter()
                    .map(|label| SpeakerMention {
                        segment_id: None,
                        speaker_label: label.clone(),
                    })
                    .collect(),
            );
        }
        let mentions = canonical
            .speaker_mentions
            .as_mut()
            .expect("mentions initialized");
        if let Some(observations) = source.speaker_mentions {
            mentions.extend(observations);
        } else {
            mentions.extend(
                source
                    .speakers
                    .into_iter()
                    .map(|speaker_label| SpeakerMention {
                        segment_id: None,
                        speaker_label,
                    }),
            );
        }
        rebuild_speakers(canonical);
        for alias in source
            .aliases
            .into_iter()
            .chain(std::iter::once(source.name))
        {
            if !same_name(&alias, &canonical.name)
                && !canonical
                    .aliases
                    .iter()
                    .any(|existing| same_name(existing, &alias))
            {
                canonical.aliases.push(alias);
            }
        }
        self.graph[from].aliases.clear();
        for target in self.name_index.values_mut() {
            if *target == from {
                *target = to;
            }
        }
        self.track_updated_node(to);
    }

    fn reattribute_segment_edges(
        &mut self,
        from: NodeIndex,
        to_name: &str,
        segments: &BTreeSet<String>,
        timestamp: f64,
    ) -> usize {
        // Index into one of four groups by which endpoint(s) move. Untouched
        // observations stay in group 0, including literal names on a mixed edge.
        let group = |contribution: &EdgeContribution, src, tgt| {
            if contribution
                .source_segment_id
                .as_ref()
                .is_none_or(|id| !segments.contains(id))
            {
                return 0;
            }
            let mask = contribution.speaker_endpoint_mask;
            usize::from(src == from && mask.source())
                | (usize::from(tgt == from && mask.target()) << 1)
        };
        let affected: Vec<_> = self
            .live_incident_edges(from)
            .filter_map(|(idx, src, tgt, edge)| {
                let selected = match &edge.contributions {
                    Some(contributions) => contributions.iter().any(|c| group(c, src, tgt) != 0),
                    None => {
                        !edge.source_segment_id.is_empty()
                            && segments.contains(&edge.source_segment_id)
                    }
                };
                selected.then_some((idx, src, tgt))
            })
            .collect();
        if affected.is_empty() {
            return 0;
        }
        let to = self.get_or_create_split_target(to_name, timestamp);
        // Retire all source rows before refolding: a mixed self-loop can
        // produce the same endpoints as another affected row.
        for &(idx, _, _) in &affected {
            self.invalidate_edge(idx, timestamp);
        }

        for &(idx, src, tgt) in &affected {
            let edge = &self.graph[idx];
            let legacy = edge
                .contributions
                .is_none()
                .then(|| edge.legacy_contribution());
            let mut groups: [Vec<EdgeContribution>; 4] = std::array::from_fn(|_| Vec::new());
            for mut observation in edge.contributions.iter().flatten().cloned().chain(legacy) {
                let bucket = group(&observation, src, tgt);
                if bucket != 0 {
                    observation.current_speaker_label = attributed_node(
                        observation.speaker_endpoint_mask,
                        if bucket & 1 != 0 { to } else { src },
                        if bucket & 2 != 0 { to } else { tgt },
                    )
                    .map(|node| self.graph[node].name.clone());
                }
                groups[bucket].push(observation);
            }
            let relation_type = edge.relation_type.clone();
            for (bucket, contributions) in groups.into_iter().enumerate() {
                let Some(first) = contributions.first() else {
                    continue;
                };
                let edge = TemporalEdge {
                    seq_id: 0,
                    relation_type: relation_type.clone(),
                    valid_from: timestamp,
                    valid_until: None,
                    confidence: first.confidence,
                    source_segment_id: contributions
                        .iter()
                        .find_map(|c| c.source_segment_id.clone())
                        .unwrap_or_default(),
                    detail: first.detail.clone(),
                    weight: contributions.iter().map(|c| c.weight).sum(),
                    contributions: Some(contributions),
                };
                self.refold_edge(
                    if bucket & 1 != 0 { to } else { src },
                    if bucket & 2 != 0 { to } else { tgt },
                    edge,
                    timestamp,
                );
            }
        }
        affected.len()
    }

    fn get_or_create_split_target(&mut self, name: &str, timestamp: f64) -> NodeIndex {
        let key = name.to_lowercase();
        let redirect = self.name_index.get(&key).copied();
        if let Some(idx) = redirect {
            if same_name(&self.graph[idx].name, name) {
                return idx;
            }
            // Splits/unbinds restore an exact display name, not its old redirect.
            self.graph[idx]
                .aliases
                .retain(|alias| !same_name(alias, name));
        }
        if let Some(idx) = self
            .graph
            .node_indices()
            .find(|&idx| same_name(&self.graph[idx].name, name))
        {
            self.name_index.insert(key, idx);
            idx
        } else {
            self.insert_entity(
                &ExtractedEntity {
                    name: name.to_owned(),
                    entity_type: "Person".to_owned(),
                    description: None,
                },
                key,
                timestamp,
            )
        }
    }

    fn reattribute_mentions(
        &mut self,
        from: &str,
        from_display: &str,
        to: &str,
        segments: Option<&BTreeSet<String>>,
    ) -> usize {
        let matches_from =
            |label: &str| label != to && (same_name(label, from) || same_name(label, from_display));
        let mut updated = 0;
        for idx in self.graph.node_indices().collect::<Vec<_>>() {
            let node = &mut self.graph[idx];
            let mut changed = false;
            if let Some(mentions) = &mut node.speaker_mentions {
                for mention in mentions {
                    if matches_from(&mention.speaker_label)
                        && segments.is_none_or(|ids| {
                            mention
                                .segment_id
                                .as_ref()
                                .is_some_and(|id| ids.contains(id))
                        })
                    {
                        mention.speaker_label = to.to_owned();
                        changed = true;
                    }
                }
                if changed {
                    rebuild_speakers(node);
                }
            } else if segments.is_none() {
                for label in &mut node.speakers {
                    if matches_from(label) {
                        *label = to.to_owned();
                        changed = true;
                    }
                }
                if changed {
                    node.speakers.sort_unstable();
                    node.speakers.dedup();
                }
            }
            if changed {
                updated += 1;
                self.track_updated_node(idx);
            }
        }
        updated
    }

    /// Process a full extraction result from a transcript segment.
    /// This is the main entry point for feeding data into the graph.
    ///
    /// After inserting entities and relations, enforces size limits by
    /// evicting the oldest nodes/edges when `MAX_NODES` or `MAX_EDGES`
    /// (see the consts in this module) are exceeded.
    pub fn process_extraction(
        &mut self,
        result: &ExtractionResult,
        timestamp: f64,
        speaker: &str,
        segment_id: &str,
    ) {
        let segment = (!segment_id.is_empty()).then_some(segment_id);
        let canonical_speaker = self
            .name_index
            .get(&speaker.to_lowercase())
            .map(|&idx| &self.graph[idx].name)
            .filter(|name| name.as_str() != speaker)
            .cloned();
        let speaker = canonical_speaker.as_deref().unwrap_or(speaker);
        let mut mentioned = HashSet::new();
        for entity in &result.entities {
            mentioned.insert(self.add_entity_with_segment(entity, timestamp, speaker, segment));
        }
        for relation in &result.relations {
            if let Some((source, target)) = self.add_relation_observation(
                &relation.source,
                &relation.target,
                relation,
                timestamp,
                segment_id,
            ) {
                for idx in [source, target] {
                    if mentioned.insert(idx) {
                        record_speaker_mention(&mut self.graph[idx], speaker, segment);
                        self.track_updated_node(idx);
                    }
                }
            }
        }

        self.event_counter += 1;

        // Evict oldest nodes if over limit
        self.evict_excess_nodes();

        // Evict oldest edges if over limit
        self.evict_excess_edges();
    }

    /// Remove the oldest nodes (by `last_seen`) until count ≤ [`MAX_NODES`].
    fn evict_excess_nodes(&mut self) {
        while self.graph.node_count() > MAX_NODES {
            // Find the node with the smallest `last_seen` timestamp
            let oldest = self.graph.node_indices().min_by(|&a, &b| {
                let a_ts = self
                    .graph
                    .node_weight(a)
                    .map(|n| n.last_seen)
                    .unwrap_or(f64::MAX);
                let b_ts = self
                    .graph
                    .node_weight(b)
                    .map(|n| n.last_seen)
                    .unwrap_or(f64::MAX);
                a_ts.partial_cmp(&b_ts).unwrap_or(std::cmp::Ordering::Equal)
            });

            match oldest {
                Some(idx) => self.evict_node(idx),
                None => break,
            }
        }
    }

    /// Evict a single node and all of its incident edges, recording every
    /// removal in the delta. Pulled out of [`Self::evict_excess_nodes`] so the
    /// edge-cascade bookkeeping lives in one place (and so tests can drive a
    /// single eviction without depending on the [`MAX_NODES`] threshold).
    fn evict_node(&mut self, idx: NodeIndex) {
        // All primary names and redirects into a recycled index must disappear.
        self.name_index.retain(|_, target| *target != idx);
        if let Some(entity) = self.graph.node_weight(idx) {
            // Track removal in delta
            self.delta_removed_node_ids.push(entity.id.clone());
            // Remove from added/updated lists if present
            self.delta_added_node_ids.retain(|id| id != &entity.id);
            self.delta_updated_node_ids.retain(|id| id != &entity.id);
            log::debug!(
                "Graph eviction: removed oldest node '{}' (last_seen={:.1})",
                entity.name,
                entity.last_seen,
            );
        }
        // petgraph's remove_node cascades into removing all incident edges, but
        // those cascaded edge removals are NOT surfaced anywhere else — so
        // enumerate them HERE (BOTH directions, since the graph is directed) and
        // push their (monotonic seq_id-derived) link ids into the removal delta,
        // scrubbing them from added/updated tracking. Otherwise the frontend
        // keeps dangling links to a node that no longer exists until the next
        // full snapshot.
        let mut incident: Vec<petgraph::graph::EdgeIndex> = self
            .graph
            .edges_directed(idx, petgraph::Direction::Outgoing)
            .chain(
                self.graph
                    .edges_directed(idx, petgraph::Direction::Incoming),
            )
            .map(|e| e.id())
            .collect();
        // A self-loop appears in both directions; de-dup so we don't emit its
        // removal id twice.
        incident.sort();
        incident.dedup();
        for edge_idx in incident {
            if let Some(edge) = self.graph.edge_weight(edge_idx) {
                self.delta_removed_edge_ids.push(edge_link_id(edge.seq_id));
            }
            self.delta_added_edge_indices.retain(|&ei| ei != edge_idx);
            self.delta_updated_edge_indices.retain(|&ei| ei != edge_idx);
        }
        self.graph.remove_node(idx);
    }

    /// Test-only: evict the single oldest node (by `last_seen`) regardless of
    /// the [`MAX_NODES`] threshold, exercising the same path the production
    /// evictor uses.
    #[cfg(test)]
    fn evict_oldest_node_for_test(&mut self) {
        let oldest = self.graph.node_indices().min_by(|&a, &b| {
            let a_ts = self
                .graph
                .node_weight(a)
                .map(|n| n.last_seen)
                .unwrap_or(f64::MAX);
            let b_ts = self
                .graph
                .node_weight(b)
                .map(|n| n.last_seen)
                .unwrap_or(f64::MAX);
            a_ts.partial_cmp(&b_ts).unwrap_or(std::cmp::Ordering::Equal)
        });
        if let Some(idx) = oldest {
            self.evict_node(idx);
        }
    }

    /// Remove the oldest edges (by `valid_from`) until count ≤ [`MAX_EDGES`].
    fn evict_excess_edges(&mut self) {
        while self.graph.edge_count() > MAX_EDGES {
            // Find the edge with the smallest `valid_from` timestamp
            let oldest = self.graph.edge_indices().min_by(|&a, &b| {
                let a_ts = self
                    .graph
                    .edge_weight(a)
                    .map(|e| e.valid_from)
                    .unwrap_or(f64::MAX);
                let b_ts = self
                    .graph
                    .edge_weight(b)
                    .map(|e| e.valid_from)
                    .unwrap_or(f64::MAX);
                a_ts.partial_cmp(&b_ts).unwrap_or(std::cmp::Ordering::Equal)
            });

            if let Some(idx) = oldest {
                // Track removal in delta using the SAME id scheme the frontend
                // holds (snapshot/added links derive from the edge's monotonic
                // seq_id), so the delta removal actually matches and the edge
                // disappears without waiting for a snapshot.
                if let Some(edge) = self.graph.edge_weight(idx) {
                    self.delta_removed_edge_ids.push(edge_link_id(edge.seq_id));
                }
                // Remove from added/updated lists if present
                self.delta_added_edge_indices.retain(|&ei| ei != idx);
                self.delta_updated_edge_indices.retain(|&ei| ei != idx);
                log::debug!("Graph eviction: removed oldest edge (idx={:?})", idx);
                self.graph.remove_edge(idx);
            } else {
                break;
            }
        }
    }

    /// Take a snapshot of the current graph state for frontend rendering.
    /// Produces a [`GraphSnapshot`] with `nodes`, `links`, and `stats` fields
    /// compatible with react-force-graph.
    pub fn snapshot(&self) -> GraphSnapshot {
        let nodes: Vec<GraphNode> = self
            .graph
            .node_indices()
            .filter_map(|idx| {
                self.graph.node_weight(idx).map(|entity| GraphNode {
                    id: entity.id.clone(),
                    name: entity.name.clone(),
                    entity_type: entity.entity_type.clone(),
                    val: (entity.mention_count as f32).sqrt() * 2.0 + 1.0,
                    color: entity_type_color(&entity.entity_type).to_string(),
                    first_seen: entity.first_seen,
                    last_seen: entity.last_seen,
                    mention_count: entity.mention_count,
                    description: entity.description.clone(),
                })
            })
            .collect();

        let links: Vec<GraphLink> = self
            .graph
            .edge_indices()
            .filter_map(|idx| {
                let (source_idx, target_idx) = self.graph.edge_endpoints(idx)?;
                let edge = self.graph.edge_weight(idx)?;
                let source_node = self.graph.node_weight(source_idx)?;
                let target_node = self.graph.node_weight(target_idx)?;

                // Only include valid (non-expired) edges
                if edge.valid_until.is_some() {
                    return None;
                }

                Some(GraphLink {
                    id: edge_link_id(edge.seq_id),
                    source: source_node.id.clone(),
                    target: target_node.id.clone(),
                    relation_type: edge.relation_type.clone(),
                    weight: edge.weight,
                    color: relation_type_color(&edge.relation_type).to_string(),
                    label: edge
                        .detail
                        .clone()
                        .or_else(|| Some(edge.relation_type.clone())),
                    source_segment_id: edge.source_segment_id.clone(),
                })
            })
            .collect();

        GraphSnapshot {
            stats: GraphStats {
                total_nodes: nodes.len(),
                total_edges: links.len(),
                total_episodes: self.event_counter,
            },
            nodes,
            links,
        }
    }

    // -----------------------------------------------------------------------
    // Delta tracking
    // -----------------------------------------------------------------------

    /// Returns `true` if there are any accumulated changes since the last
    /// `take_delta()` call.
    pub fn has_delta(&self) -> bool {
        !self.delta_added_node_ids.is_empty()
            || !self.delta_updated_node_ids.is_empty()
            || !self.delta_added_edge_indices.is_empty()
            || !self.delta_updated_edge_indices.is_empty()
            || !self.delta_removed_node_ids.is_empty()
            || !self.delta_removed_edge_ids.is_empty()
    }

    /// Take the accumulated delta since the last call, resetting the internal
    /// delta buffers. Returns a [`GraphDelta`] with the changes.
    pub fn take_delta(&mut self) -> GraphDelta {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();

        // Collect added nodes
        let added_nodes: Vec<GraphNode> = self
            .delta_added_node_ids
            .drain(..)
            .filter_map(|id| {
                self.graph.node_indices().find_map(|idx| {
                    let entity = self.graph.node_weight(idx)?;
                    if entity.id == id {
                        Some(GraphNode {
                            id: entity.id.clone(),
                            name: entity.name.clone(),
                            entity_type: entity.entity_type.clone(),
                            val: (entity.mention_count as f32).sqrt() * 2.0 + 1.0,
                            color: entity_type_color(&entity.entity_type).to_string(),
                            first_seen: entity.first_seen,
                            last_seen: entity.last_seen,
                            mention_count: entity.mention_count,
                            description: entity.description.clone(),
                        })
                    } else {
                        None
                    }
                })
            })
            .collect();

        // Collect updated nodes
        let updated_nodes: Vec<GraphNode> = self
            .delta_updated_node_ids
            .drain(..)
            .filter_map(|id| {
                self.graph.node_indices().find_map(|idx| {
                    let entity = self.graph.node_weight(idx)?;
                    if entity.id == id {
                        Some(GraphNode {
                            id: entity.id.clone(),
                            name: entity.name.clone(),
                            entity_type: entity.entity_type.clone(),
                            val: (entity.mention_count as f32).sqrt() * 2.0 + 1.0,
                            color: entity_type_color(&entity.entity_type).to_string(),
                            first_seen: entity.first_seen,
                            last_seen: entity.last_seen,
                            mention_count: entity.mention_count,
                            description: entity.description.clone(),
                        })
                    } else {
                        None
                    }
                })
            })
            .collect();

        // Build a delta edge from an index, skipping dangling/expired edges.
        // Defined as a fn (not a closure) so it can be reused for added/updated
        // without borrow-checker grief, taking the graph by reference.
        fn build_delta_edge(
            graph: &StableGraph<GraphEntity, TemporalEdge>,
            edge_idx: petgraph::graph::EdgeIndex,
        ) -> Option<GraphEdge> {
            let (source_idx, target_idx) = graph.edge_endpoints(edge_idx)?;
            let edge = graph.edge_weight(edge_idx)?;
            let source_node = graph.node_weight(source_idx)?;
            let target_node = graph.node_weight(target_idx)?;

            // Skip expired edges
            if edge.valid_until.is_some() {
                return None;
            }

            Some(GraphEdge {
                id: edge_link_id(edge.seq_id),
                source: source_node.id.clone(),
                target: target_node.id.clone(),
                relation_type: edge.relation_type.clone(),
                weight: edge.weight,
                color: relation_type_color(&edge.relation_type).to_string(),
                label: edge
                    .detail
                    .clone()
                    .or_else(|| Some(edge.relation_type.clone())),
                source_segment_id: edge.source_segment_id.clone(),
            })
        }

        // Collect added edges
        let added_edges: Vec<GraphEdge> = self
            .delta_added_edge_indices
            .drain(..)
            .filter_map(|edge_idx| build_delta_edge(&self.graph, edge_idx))
            .collect();

        // Collect updated edges (weight/label changes on existing edges)
        let updated_edges: Vec<GraphEdge> = self
            .delta_updated_edge_indices
            .drain(..)
            .filter_map(|edge_idx| build_delta_edge(&self.graph, edge_idx))
            .collect();

        let removed_node_ids: Vec<String> = self.delta_removed_node_ids.drain(..).collect();
        let removed_edge_ids: Vec<String> = self.delta_removed_edge_ids.drain(..).collect();

        GraphDelta {
            added_nodes,
            updated_nodes,
            added_edges,
            updated_edges,
            removed_node_ids,
            removed_edge_ids,
            timestamp,
        }
    }

    // -----------------------------------------------------------------------
    // Persistence (save / load)
    // -----------------------------------------------------------------------

    /// Serialize the graph to a JSON file.
    pub fn save_to_file(&self, path: &Path) -> Result<(), String> {
        let nodes: Vec<GraphEntity> = self
            .graph
            .node_indices()
            .filter_map(|idx| self.graph.node_weight(idx).cloned())
            .collect();

        let edges: Vec<SerializableEdge> = self
            .graph
            .edge_indices()
            .filter_map(|idx| {
                let (src, tgt) = self.graph.edge_endpoints(idx)?;
                let edge = self.graph.edge_weight(idx)?.clone();
                let source_name = self.graph.node_weight(src)?.name.clone();
                let target_name = self.graph.node_weight(tgt)?.name.clone();
                Some(SerializableEdge {
                    source_name,
                    target_name,
                    edge,
                })
            })
            .collect();

        let data = SerializableGraph {
            nodes,
            edges,
            event_counter: self.event_counter,
        };

        crate::persistence::save_json(&data, path)
    }

    /// Deserialize a graph from a JSON file.
    pub fn load_from_file(path: &Path) -> Result<Self, String> {
        let data: SerializableGraph = crate::persistence::load_json(path)?;

        let mut graph = StableGraph::new();
        let mut name_index = HashMap::new();

        // Re-create nodes
        for entity in &data.nodes {
            let idx = graph.add_node(entity.clone());
            name_index.insert(entity.name.to_lowercase(), idx);
        }

        // Re-create edges, re-deriving a fresh monotonic seq_id for each so ids
        // are collision-free regardless of the persisted value (older saves may
        // not carry seq_id at all and would otherwise all default to 0).
        let mut edge_seq_counter: u64 = 0;
        for se in &data.edges {
            let src_key = se.source_name.to_lowercase();
            let tgt_key = se.target_name.to_lowercase();
            if let (Some(&src_idx), Some(&tgt_idx)) =
                (name_index.get(&src_key), name_index.get(&tgt_key))
            {
                let mut edge = se.edge.clone();
                edge.seq_id = edge_seq_counter;
                edge_seq_counter += 1;
                graph.add_edge(src_idx, tgt_idx, edge);
            } else {
                log::warn!(
                    "Graph load: skipping edge '{}' → '{}' (missing node)",
                    se.source_name,
                    se.target_name
                );
            }
        }

        // Build edges from the stored primary names first so historical rows
        // still point to retired nodes. Restore redirects only for live lookup.
        for idx in graph.node_indices() {
            for alias in &graph[idx].aliases {
                name_index.insert(alias.to_lowercase(), idx);
            }
        }

        Ok(Self {
            graph,
            name_index,
            event_counter: data.event_counter,
            edge_seq_counter,
            delta_added_node_ids: Vec::new(),
            delta_updated_node_ids: Vec::new(),
            delta_added_edge_indices: Vec::new(),
            delta_updated_edge_indices: Vec::new(),
            delta_removed_node_ids: Vec::new(),
            delta_removed_edge_ids: Vec::new(),
        })
    }
}

impl Default for TemporalKnowledgeGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::entities::{ExtractedEntity, ExtractedRelation, ExtractionResult};

    fn entity(name: &str) -> ExtractedEntity {
        ExtractedEntity {
            name: name.to_string(),
            entity_type: "Person".to_string(),
            description: None,
        }
    }

    fn relation(src: &str, tgt: &str, rel: &str) -> ExtractedRelation {
        ExtractedRelation {
            source: src.to_string(),
            target: tgt.to_string(),
            relation_type: rel.to_string(),
            detail: None,
            speaker_endpoint_mask: SpeakerEndpointMask::Neither,
        }
    }

    fn segments(ids: &[&str]) -> RelabelScope {
        RelabelScope::Segments {
            segment_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        }
    }

    #[test]
    fn entity_relabel_renames_in_place_with_stable_ids_and_persistent_alias() {
        let mut graph = TemporalKnowledgeGraph::new();
        let mut edge = relation("Speaker A", "Acme", "mentions");
        edge.speaker_endpoint_mask = SpeakerEndpointMask::Source;
        graph.process_extraction(
            &ExtractionResult {
                entities: vec![entity("Speaker A"), entity("Acme")],
                relations: vec![edge],
            },
            1.0,
            "Speaker A",
            "turn-1",
        );
        let speaker = graph.resolve_entity("Speaker A", 1.0).unwrap();
        let acme = graph.resolve_entity("Acme", 1.0).unwrap();
        let node_id = graph.graph[speaker].id.clone();
        let edge_id = graph.snapshot().links[0].id.clone();
        let _ = graph.take_delta();

        let result = graph.reattribute_speaker("Speaker A", "Alice", RelabelScope::Entity, 2.0);
        assert!(result.renamed_in_place);
        assert!(result.changed());
        assert_eq!(result.edges_retconned, 0);
        assert_eq!(graph.resolve_entity("SPEAKER A", 1.0), Some(speaker));
        assert_eq!(graph.resolve_entity("alice", 1.0), Some(speaker));
        assert_eq!(graph.graph[speaker].id, node_id);
        assert_eq!(graph.graph[speaker].aliases, ["Speaker A"]);
        let delta = graph.take_delta();
        assert!(delta.added_edges.is_empty() && delta.removed_edge_ids.is_empty());
        assert!(
            delta
                .updated_nodes
                .iter()
                .any(|node| node.id == node_id && node.name == "Alice")
        );
        assert_eq!(graph.snapshot().links[0].id, edge_id);
        assert_eq!(graph.graph[acme].speakers, ["Alice"]);
        assert_eq!(
            graph.graph[acme].speaker_mentions.as_ref().unwrap()[0].speaker_label,
            "Alice"
        );

        let path =
            std::env::temp_dir().join(format!("audiograph-retcon-{}.json", uuid::Uuid::new_v4()));
        graph.save_to_file(&path).unwrap();
        let mut reloaded = TemporalKnowledgeGraph::load_from_file(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        let alice = reloaded.resolve_entity("Alice", 1.0).unwrap();
        assert_eq!(reloaded.resolve_entity("Speaker A", 1.0), Some(alice));
        assert_eq!(reloaded.graph[alice].id, node_id);
        reloaded.evict_node(alice);
        assert_eq!(reloaded.resolve_entity("Speaker A", 1.0), None);
        assert_eq!(reloaded.resolve_entity("Alice", 1.0), None);
    }

    #[test]
    fn segment_relabel_splits_folded_observations_and_rebuilds_speakers() {
        let mut graph = TemporalKnowledgeGraph::new();
        let mut edge = relation("Speaker A", "Acme", "mentions");
        edge.speaker_endpoint_mask = SpeakerEndpointMask::Source;
        let mut extraction = ExtractionResult {
            entities: vec![entity("Speaker A"), entity("Acme")],
            relations: vec![edge],
        };
        extraction.relations[0].detail = Some("first".into());
        graph.process_extraction(&extraction, 1.0, "Speaker A", "turn-1");
        extraction.relations[0].detail = Some("second".into());
        graph.process_extraction(&extraction, 2.0, "Speaker A", "turn-2");
        let original = graph.live_edges_by_source_segment();
        assert_eq!(original["turn-1"], original["turn-2"]);

        let result =
            graph.reattribute_speaker("Speaker A", "Speaker B", segments(&["turn-2"]), 3.0);
        assert_eq!(result.edges_retconned, 1);
        let a = graph.resolve_entity("Speaker A", 1.0).unwrap();
        let b = graph.resolve_entity("Speaker B", 1.0).unwrap();
        let acme = graph.resolve_entity("Acme", 1.0).unwrap();
        assert_ne!(a, b);
        assert_eq!(graph.graph[b].entity_type, "Person");
        let retired = graph
            .graph
            .edge_weights()
            .find(|e| e.valid_until == Some(3.0))
            .unwrap();
        assert_eq!(retired.weight, 2.0);
        assert_eq!(retired.contributions.as_ref().unwrap().len(), 2);
        for (source, label, turn, observed_at, detail) in [
            (a, "Speaker A", "turn-1", 1.0, "first"),
            (b, "Speaker B", "turn-2", 2.0, "second"),
        ] {
            let idx = graph.live_edge(source, acme, "mentions").unwrap();
            let live = &graph.graph[idx];
            assert_eq!(live.weight, 1.0);
            assert_eq!(live.source_segment_id, turn);
            assert_eq!(live.detail.as_deref(), Some(detail));
            let observations = live.contributions.as_ref().unwrap();
            assert_eq!(observations.len(), 1);
            assert_eq!(observations[0].observed_at, observed_at);
            assert_eq!(
                observations[0].origin_speaker_label.as_deref(),
                Some("Speaker A")
            );
            assert_eq!(
                observations[0].current_speaker_label.as_deref(),
                Some(label)
            );
        }
        let joined = graph.live_edges_by_source_segment();
        assert_ne!(joined["turn-1"], joined["turn-2"]);
        assert_eq!(graph.graph[acme].speakers, ["Speaker A", "Speaker B"]);
        assert_eq!(
            graph.graph[acme]
                .speaker_mentions
                .as_ref()
                .unwrap()
                .iter()
                .map(|m| { (m.segment_id.as_deref().unwrap(), m.speaker_label.as_str()) })
                .collect::<Vec<_>>(),
            [("turn-1", "Speaker A"), ("turn-2", "Speaker B")]
        );

        let result =
            graph.reattribute_speaker("Speaker A", "Speaker B", segments(&["turn-1"]), 4.0);
        assert_eq!(result.edges_retconned, 1);
        assert_eq!(graph.graph[acme].speakers, ["Speaker B"]);
        assert_eq!(graph.snapshot().links[0].weight, 2.0);
        let folded = graph.live_edges_by_source_segment();
        assert_eq!(folded["turn-1"], folded["turn-2"]);
    }

    #[test]
    fn scoped_relabel_only_moves_speaker_role_on_self_loop() {
        let mut graph = TemporalKnowledgeGraph::new();
        let literal = relation("Speaker A", "Speaker A", "mentions");
        let mut source_role = literal.clone();
        source_role.speaker_endpoint_mask = SpeakerEndpointMask::Source;
        let mut target_role = literal.clone();
        target_role.speaker_endpoint_mask = SpeakerEndpointMask::Target;
        let mut both = literal.clone();
        both.speaker_endpoint_mask = SpeakerEndpointMask::Both;
        graph.process_extraction(
            &ExtractionResult {
                entities: vec![entity("Speaker A")],
                relations: vec![literal, source_role, target_role, both],
            },
            1.0,
            "Speaker A",
            "turn-1",
        );
        let result =
            graph.reattribute_speaker("Speaker A", "Speaker B", segments(&["turn-1"]), 2.0);
        assert_eq!(
            result.edges_retconned, 1,
            "a self-loop is retired only once"
        );
        let a = graph.resolve_entity("Speaker A", 1.0).unwrap();
        let b = graph.resolve_entity("Speaker B", 1.0).unwrap();
        for (src, tgt) in [(a, a), (a, b), (b, a), (b, b)] {
            let idx = graph.live_edge(src, tgt, "mentions").unwrap();
            assert_eq!(graph.graph[idx].weight, 1.0);
        }
    }

    #[test]
    fn merge_without_edges_updates_mentions_and_split_reclaims_retired_name() {
        let mut graph = TemporalKnowledgeGraph::new();
        graph.process_extraction(
            &ExtractionResult {
                entities: vec![entity("Speaker A"), entity("Acme")],
                relations: vec![],
            },
            1.0,
            "Speaker A",
            "turn-1",
        );
        let retired = graph.resolve_entity("Speaker A", 1.0).unwrap();
        let acme = graph.resolve_entity("Acme", 1.0).unwrap();
        let alice = graph.add_entity(&entity("Alice"), 2.0, "Other");
        graph.graph[acme].speaker_mentions = None; // Legacy speaker list.
        let result = graph.reattribute_speaker("Speaker A", "Alice", RelabelScope::Entity, 3.0);
        assert!(result.changed() && result.merged);
        assert_eq!(result.edges_retconned, 0);
        assert_eq!(graph.graph[acme].speakers, ["Alice"]);
        assert_eq!(graph.graph[alice].speakers, ["Alice", "Other"]);
        assert_eq!(graph.resolve_entity("Speaker A", 1.0), Some(alice));

        let result = graph.reattribute_speaker("Alice", "Speaker A", segments(&["turn-1"]), 4.0);
        assert!(result.changed());
        assert_eq!(graph.resolve_entity("Speaker A", 1.0), Some(retired));
        assert_eq!(
            graph.graph[acme].speakers,
            ["Alice"],
            "cannot split unanchored legacy mentions"
        );
        assert_eq!(graph.graph[alice].speakers, ["Other", "Speaker A"]);
    }

    #[test]
    fn legacy_edge_uses_seek_hint_and_keeps_later_observations() {
        let mut graph = TemporalKnowledgeGraph::new();
        let source = graph.add_entity(&entity("Speaker A"), 1.0, "Speaker A");
        let target = graph.add_entity(&entity("Acme"), 1.0, "Speaker A");
        let legacy: TemporalEdge = serde_json::from_str(
            r#"{
            "relation_type":"mentions", "valid_from":1.0, "valid_until":null,
            "confidence":1.0, "source_segment_id":"old-turn", "detail":null, "weight":3.0
        }"#,
        )
        .unwrap();
        assert!(legacy.contributions.is_none());
        assert_eq!(
            legacy.all_source_segment_ids().collect::<Vec<_>>(),
            ["old-turn"]
        );
        graph.insert_live_edge(source, target, legacy);
        let mut later = relation("Speaker A", "Acme", "mentions");
        later.speaker_endpoint_mask = SpeakerEndpointMask::Source;
        graph.add_relation("Speaker A", "Acme", &later, 2.0, "new-turn");

        let result =
            graph.reattribute_speaker("Speaker A", "Speaker B", segments(&["old-turn"]), 3.0);
        assert_eq!(result.edges_retconned, 1);
        let b = graph.resolve_entity("Speaker B", 1.0).unwrap();
        for (node, segment, weight) in [(source, "new-turn", 1.0), (b, "old-turn", 3.0)] {
            let idx = graph.live_edge(node, target, "mentions").unwrap();
            let edge = &graph.graph[idx];
            assert_eq!(edge.weight, weight);
            assert_eq!(edge.source_segment_id, segment);
        }
    }

    /// Snapshot links, delta `added_edges`, and `edge_link_id` must all agree on
    /// the id for a given edge — otherwise frontend removals/updates never match.
    #[test]
    fn edge_ids_are_consistent_across_snapshot_and_delta() {
        let mut g = TemporalKnowledgeGraph::new();
        let extraction = ExtractionResult {
            entities: vec![entity("Alice"), entity("Bob")],
            relations: vec![relation("Alice", "Bob", "knows")],
        };
        g.process_extraction(&extraction, 1.0, "spk", "seg-1");

        let delta = g.take_delta();
        assert_eq!(delta.added_edges.len(), 1, "one edge added");
        let added_id = delta.added_edges[0].id.clone();

        // The id the helper produces for the underlying edge's seq_id matches.
        let idx = g.graph.edge_indices().next().expect("edge exists");
        let seq_id = g.graph.edge_weight(idx).expect("weight").seq_id;
        assert_eq!(edge_link_id(seq_id), added_id);
        // First edge gets seq_id 0 → id "edge-0".
        assert_eq!(added_id, "edge-0");

        // A full snapshot uses the SAME id scheme.
        let snap = g.snapshot();
        assert_eq!(snap.links.len(), 1);
        assert_eq!(snap.links[0].id, added_id);
    }

    /// b272 (epic 0d72 P2): the triggering `source_segment_id` stored on the
    /// backing `TemporalEdge` must be surfaced on BOTH emitted payloads — the
    /// snapshot `GraphLink` and the delta `GraphEdge` (added + updated) — so the
    /// graph view can resolve an edge back to the utterance that produced it.
    #[test]
    fn source_segment_id_is_surfaced_on_snapshot_and_delta() {
        let mut g = TemporalKnowledgeGraph::new();
        let first = ExtractionResult {
            entities: vec![entity("Alice"), entity("Bob")],
            relations: vec![relation("Alice", "Bob", "knows")],
        };
        g.process_extraction(&first, 1.0, "spk", "seg-42");

        // Added-edge delta carries the segment id.
        let d1 = g.take_delta();
        assert_eq!(d1.added_edges.len(), 1);
        assert_eq!(d1.added_edges[0].source_segment_id, "seg-42");

        // Snapshot link carries the same segment id.
        let snap = g.snapshot();
        assert_eq!(snap.links.len(), 1);
        assert_eq!(snap.links[0].source_segment_id, "seg-42");

        // Re-asserting the relation surfaces an updated edge that still carries
        // the ORIGINAL triggering segment id (the edge's provenance is set once,
        // at creation, and a weight bump does not overwrite it).
        let second = ExtractionResult {
            entities: vec![entity("Alice"), entity("Bob")],
            relations: vec![relation("Alice", "Bob", "knows")],
        };
        g.process_extraction(&second, 2.0, "spk", "seg-99");
        let d2 = g.take_delta();
        assert_eq!(d2.updated_edges.len(), 1);
        assert_eq!(
            d2.updated_edges[0].source_segment_id, "seg-42",
            "an updated (weight-bumped) edge keeps its original source segment"
        );
    }

    /// Re-asserting the same relation bumps its weight and surfaces it via
    /// `updated_edges` (so the UI refreshes edge strength between snapshots).
    #[test]
    fn repeated_relation_emits_updated_edge() {
        let mut g = TemporalKnowledgeGraph::new();
        let first = ExtractionResult {
            entities: vec![entity("Alice"), entity("Bob")],
            relations: vec![relation("Alice", "Bob", "knows")],
        };
        g.process_extraction(&first, 1.0, "spk", "seg-1");
        let d1 = g.take_delta();
        assert_eq!(d1.added_edges.len(), 1);
        assert!(d1.updated_edges.is_empty());
        assert_eq!(d1.added_edges[0].weight, 1.0);

        // Same relation again — should be an UPDATE, not a new edge.
        let second = ExtractionResult {
            entities: vec![entity("Alice"), entity("Bob")],
            relations: vec![relation("Alice", "Bob", "knows")],
        };
        g.process_extraction(&second, 2.0, "spk", "seg-2");
        let d2 = g.take_delta();
        assert!(d2.added_edges.is_empty(), "no new edge on repeat");
        assert_eq!(d2.updated_edges.len(), 1, "weight bump surfaced as update");
        assert_eq!(d2.updated_edges[0].weight, 2.0);
        // Same id as the originally added edge.
        assert_eq!(d2.updated_edges[0].id, d1.added_edges[0].id);
    }

    /// Regression: evicted-edge removal ids MUST match the `edge-{idx}` scheme
    /// the frontend holds — previously eviction emitted `edge-evicted-{idx}`,
    /// which never matched, so evicted edges lingered until a full snapshot.
    #[test]
    fn evicted_edge_ids_match_link_id_scheme() {
        let mut g = TemporalKnowledgeGraph::new();
        g.add_entity(&entity("A"), 0.0, "spk");
        g.add_entity(&entity("B"), 0.0, "spk");
        // Distinct relation_type per edge so each is a separate edge.
        for i in 0..(MAX_EDGES + 3) {
            let rel = relation("A", "B", &format!("rel{i}"));
            g.add_relation("A", "B", &rel, i as f64, "seg");
        }
        g.evict_excess_edges();

        let delta = g.take_delta();
        assert!(
            !delta.removed_edge_ids.is_empty(),
            "eviction should have removed some edges"
        );
        for id in &delta.removed_edge_ids {
            assert!(
                id.starts_with("edge-") && !id.starts_with("edge-evicted-"),
                "removed edge id `{id}` must use the `edge-{{idx}}` scheme"
            );
        }
    }

    /// Finding #54 (P1): when `StableGraph` recycles a freed `EdgeIndex`, the
    /// new, unrelated edge MUST receive a DISTINCT frontend id — otherwise the
    /// frontend silently merges two different relations across delta windows.
    /// Ids derive from a monotonic `seq_id`, not the recyclable index, so the
    /// reused slot gets a fresh id.
    #[test]
    fn reused_edge_index_gets_distinct_id() {
        let mut g = TemporalKnowledgeGraph::new();
        g.add_entity(&entity("A"), 0.0, "spk");
        g.add_entity(&entity("B"), 0.0, "spk");

        // First edge → seq_id 0 → "edge-0".
        g.add_relation("A", "B", &relation("A", "B", "rel0"), 0.0, "seg");
        let first_idx = g.graph.edge_indices().next().expect("edge exists");
        let first_id = edge_link_id(g.graph.edge_weight(first_idx).unwrap().seq_id);
        assert_eq!(first_id, "edge-0");

        // Remove it, freeing the index for recycling.
        g.graph.remove_edge(first_idx);

        // Adding a new edge reuses the SAME EdgeIndex...
        g.add_relation("A", "B", &relation("A", "B", "rel1"), 1.0, "seg");
        let second_idx = g.graph.edge_indices().next().expect("edge exists");
        assert_eq!(
            first_idx, second_idx,
            "petgraph should have recycled the freed EdgeIndex (precondition)"
        );

        // ...but the id MUST be different (derived from a new seq_id).
        let second_id = edge_link_id(g.graph.edge_weight(second_idx).unwrap().seq_id);
        assert_eq!(second_id, "edge-1");
        assert_ne!(
            first_id, second_id,
            "a recycled edge index must NOT re-emit the prior edge's id"
        );
    }

    /// Finding #54 (P2): evicting a node cascades into petgraph removing its
    /// incident edges. Those edge removals MUST be surfaced in
    /// `removed_edge_ids` (matching the link-id scheme) and scrubbed from
    /// added/updated tracking — otherwise the frontend keeps dangling links to
    /// a node that no longer exists.
    #[test]
    fn node_eviction_emits_incident_edges_in_removed_ids() {
        let mut g = TemporalKnowledgeGraph::new();
        // Hub connects to two leaves: incident edges in BOTH directions.
        g.add_entity(&entity("Hub"), 100.0, "spk");
        g.add_entity(&entity("Leaf1"), 100.0, "spk");
        g.add_entity(&entity("Leaf2"), 100.0, "spk");
        g.add_relation("Hub", "Leaf1", &relation("Hub", "Leaf1", "out"), 0.0, "seg");
        g.add_relation("Leaf2", "Hub", &relation("Leaf2", "Hub", "in"), 0.0, "seg");

        // The two incident-edge ids (seq 0 and 1).
        let incident_ids: Vec<String> = g
            .graph
            .edge_indices()
            .map(|i| edge_link_id(g.graph.edge_weight(i).unwrap().seq_id))
            .collect();
        assert_eq!(incident_ids.len(), 2);

        // Clear the "added" delta so we test eviction in isolation.
        let _ = g.take_delta();

        // Make Hub the oldest and drive a single eviction through the same path
        // the production evictor uses (without depending on MAX_NODES).
        let hub_idx = *g.name_index.get("hub").expect("hub exists");
        g.graph
            .node_weight_mut(hub_idx)
            .expect("hub weight")
            .last_seen = -1.0; // oldest
        g.evict_oldest_node_for_test();

        let delta = g.take_delta();
        // Hub removed.
        assert!(
            !delta.removed_node_ids.is_empty(),
            "the evicted node id should be present"
        );
        // BOTH incident edges surfaced as removed.
        for id in &incident_ids {
            assert!(
                delta.removed_edge_ids.contains(id),
                "incident edge `{id}` must appear in removed_edge_ids; got {:?}",
                delta.removed_edge_ids
            );
        }
        // No dangling references remain in added/updated edge tracking.
        assert!(delta.added_edges.is_empty());
        assert!(delta.updated_edges.is_empty());
    }

    /// audio-graph-4b52: a manual-proposal write (`approve_agent_proposal` /
    /// `add_question_to_graph` in `commands.rs`) must land on the SAME
    /// session-relative-seconds clock the live speech path uses, via
    /// `TranscriptLedger::session_relative_timestamp` — not raw Unix epoch
    /// seconds. This drives the EXACT conversion those call sites perform,
    /// then proves the resulting node is evicted on schedule like any other
    /// node instead of being immortal (epoch-scale `last_seen` values are
    /// always the graph's maximum, so `evict_excess_nodes`'s `min_by` never
    /// selects them — see `temporal.rs`'s eviction doc comment).
    #[test]
    fn manual_proposal_node_evicts_on_same_terms_as_live_path_node() {
        use crate::events::{AsrSpanRevisionPayload, AsrSpanStability};
        use crate::projections::{TranscriptEvent, TranscriptLedger};

        // A ledger with one recorded live-speech span, as the live path would
        // already have written before the corresponding proposal is
        // approved.
        let mut ledger = TranscriptLedger::new("session-1");
        let asr_span = AsrSpanRevisionPayload {
            span_id: "span-1".to_string(),
            provider: "system".to_string(),
            source_id: "mic".to_string(),
            provider_item_id: None,
            transcript_segment_id: Some("segment-1".to_string()),
            speaker_id: None,
            speaker_label: None,
            channel: None,
            text: "hello".to_string(),
            start_time: 5.0, // 5s into the session
            end_time: 6.0,
            confidence: 0.9,
            is_final: true,
            stability: AsrSpanStability::Final,
            revision_number: 1,
            supersedes: None,
            turn_id: None,
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_005_000, // wall clock at receipt
        };
        ledger
            .apply_event(TranscriptEvent::from(asr_span))
            .expect("seed the live span");

        // The proposal is approved ~2s of wall-clock time after the span was
        // recorded. `created_at_ms` is Unix epoch ms, exactly as
        // `AgentProposalPayload::created_at_ms` stores it.
        let proposal_created_at_ms: u64 = 1_700_000_007_000;
        let manual_timestamp =
            ledger.session_relative_timestamp("segment-1", proposal_created_at_ms);

        // The literal "would fail on master" pin: master's
        // `proposal.created_at_ms as f64 / 1000.0` would produce
        // ~1_700_000_007.0 here (epoch seconds), which is nowhere near a
        // plausible session-relative value.
        assert!(
            manual_timestamp < 1_000.0,
            "expected session-relative seconds, got {manual_timestamp} \
             (looks like raw epoch seconds leaked through)"
        );
        assert!(
            (manual_timestamp - 7.0).abs() < 1e-9,
            "expected exactly the matching span's start_time (5.0) + 2.0s \
             wall-clock offset = 7.0, got {manual_timestamp}"
        );

        let mut g = TemporalKnowledgeGraph::new();
        // The manual-proposal write: approving a proposal derived from
        // segment-1.
        g.process_extraction(
            &ExtractionResult {
                entities: vec![entity("ManualNode")],
                relations: vec![],
            },
            manual_timestamp,
            "spk",
            "segment-1",
        );

        // Fill the graph with MAX_NODES more live-path nodes, each newer
        // (larger session-relative timestamp) than the manual node, to push
        // the graph exactly one node over the eviction threshold.
        for i in 0..MAX_NODES {
            g.process_extraction(
                &ExtractionResult {
                    entities: vec![entity(&format!("Live{i}"))],
                    relations: vec![],
                },
                100.0 + i as f64,
                "spk",
                &format!("seg-live-{i}"),
            );
        }

        // `process_extraction` evicts down to MAX_NODES after every call, so
        // the graph never exceeds it — the manual node (the oldest by
        // `last_seen`) must have been the one evicted, not immortal.
        assert!(
            !g.name_index.contains_key("manualnode"),
            "the manual-proposal node must be evictable on the same terms as \
             a live-path node, not immortal under epoch-scale timestamps"
        );
        assert_eq!(g.graph.node_count(), MAX_NODES);
    }

    /// audio-graph-4b52 (undisclosed-sibling fix): `supersede_entity` is the
    /// SAME epoch-vs-session-relative bug class as `process_extraction`, just
    /// for edges. Its production callers are `commands.rs::
    /// merge_graph_entities` (a manual retcon-merge command) and
    /// `speech/mod.rs`'s live diarization-relabel path — both must now feed a
    /// ledger-derived session-relative timestamp, not raw epoch seconds. Left
    /// as epoch seconds, a retconned edge's `valid_from` would always be the
    /// graph's maximum, and `evict_excess_edges`'s `min_by` (this file, just
    /// above) could never select it — the exact immortality bug fixed for
    /// nodes, but for edges. This drives the EXACT conversion
    /// `merge_graph_entities` performs (fallback-to-any-span anchor, since a
    /// manual merge has no source segment id to prefer), then proves the
    /// resulting re-pointed edge is evicted on schedule like any other edge.
    #[test]
    fn supersede_entity_repointed_edge_evicts_on_same_terms_as_live_path_edge() {
        use crate::events::{AsrSpanRevisionPayload, AsrSpanStability};
        use crate::projections::{TranscriptEvent, TranscriptLedger};

        // A ledger with one recorded live-speech span, as the live path would
        // already have written before the corresponding merge is invoked.
        let mut ledger = TranscriptLedger::new("session-1");
        let asr_span = AsrSpanRevisionPayload {
            span_id: "span-1".to_string(),
            provider: "system".to_string(),
            source_id: "mic".to_string(),
            provider_item_id: None,
            transcript_segment_id: Some("segment-1".to_string()),
            speaker_id: None,
            speaker_label: None,
            channel: None,
            text: "hello".to_string(),
            start_time: 5.0, // 5s into the session
            end_time: 6.0,
            confidence: 0.9,
            is_final: true,
            stability: AsrSpanStability::Final,
            revision_number: 1,
            supersedes: None,
            turn_id: None,
            end_of_turn: true,
            raw_event_ref: None,
            capture_latency_ms: None,
            asr_latency_ms: None,
            received_at_ms: 1_700_000_005_000, // wall clock at receipt
        };
        ledger
            .apply_event(TranscriptEvent::from(asr_span))
            .expect("seed the live span");

        // `merge_graph_entities` has no source segment id to prefer, so it
        // always resolves through the fallback-to-any-span anchor — 2s of
        // wall-clock time after the recorded span.
        let merge_timestamp = ledger.session_relative_timestamp("", 1_700_000_007_000);
        assert!(
            merge_timestamp < 1_000.0,
            "expected session-relative seconds, got {merge_timestamp} \
             (looks like raw epoch seconds leaked through)"
        );

        let mut g = TemporalKnowledgeGraph::new();
        // Build: provisional speaker "Speaker 2" knows "Acme"; canonical
        // identity "Alice" already exists.
        let ext = ExtractionResult {
            entities: vec![entity("Speaker 2"), entity("Acme"), entity("Alice")],
            relations: vec![relation("Speaker 2", "Acme", "works_at")],
        };
        g.process_extraction(&ext, 1.0, "spk", "seg-1");

        // The manual-merge write: "Speaker 2" is actually "Alice".
        let invalidated = g.supersede_entity("Speaker 2", "Alice", merge_timestamp, 1.0);
        assert_eq!(invalidated, 1, "exactly one edge retconned");

        // Fill the graph with MAX_EDGES more live-path edges between the
        // SAME two surviving nodes (distinct relation types, so each is a
        // new edge rather than a weight-fold), each newer (larger
        // `valid_from`) than the merge timestamp, to push the graph past the
        // eviction threshold.
        for i in 0..MAX_EDGES {
            g.process_extraction(
                &ExtractionResult {
                    entities: vec![],
                    relations: vec![relation("Alice", "Acme", &format!("filler-{i}"))],
                },
                100.0 + i as f64,
                "spk",
                &format!("seg-live-{i}"),
            );
        }

        // `process_extraction` evicts down to MAX_EDGES after every call, so
        // the graph never exceeds it. Both the invalidated old edge
        // (valid_from 1.0) and the re-pointed live edge (valid_from
        // `merge_timestamp`, ~7.0) are session-relative and smaller than
        // every filler's valid_from (>= 100.0), so both must have been
        // evicted — the re-pointed edge is NOT immortal.
        let live_edge_survives = g
            .graph
            .edge_indices()
            .filter_map(|i| g.graph.edge_weight(i))
            .any(|e| e.relation_type == "works_at");
        assert!(
            !live_edge_survives,
            "the re-pointed edge must be evictable on the same terms as a \
             live-path edge, not immortal under epoch-scale timestamps"
        );
        assert_eq!(g.graph.edge_count(), MAX_EDGES);
    }

    /// Finding #55 (P3): on eviction, the name_index key must only be removed if
    /// it still points at the node being evicted. If a DIFFERENT surviving node
    /// was later inserted under the same lowercased key, the index must keep
    /// pointing at that survivor.
    #[test]
    fn name_index_eviction_keeps_survivor_under_same_key() {
        let mut g = TemporalKnowledgeGraph::new();
        // Insert "alice" (old), evict it, but first simulate the index being
        // re-pointed at a NEW node under the same key. We emulate that by
        // inserting the old node, then overwriting name_index to a fresh node
        // index, then evicting the OLD index.
        g.add_entity(&entity("Alice"), 0.0, "spk");
        let old_idx = *g.name_index.get("alice").unwrap();

        // A second, distinct node that the key now resolves to.
        let new_node = GraphEntity {
            id: "new-id".into(),
            name: "Alice".into(),
            entity_type: "Person".into(),
            mention_count: 1,
            first_seen: 10.0,
            last_seen: 10.0,
            aliases: vec![],
            description: None,
            speakers: vec![],
            person_ref: None,
            speaker_mentions: None,
        };
        let new_idx = g.graph.add_node(new_node);
        g.name_index.insert("alice".to_string(), new_idx); // re-point key

        // Make the OLD node the eviction target.
        if let Some(n) = g.graph.node_weight_mut(old_idx) {
            n.last_seen = -1.0;
        }
        g.evict_oldest_node_for_test();

        // The key must STILL resolve to the survivor, not be wrongly removed.
        assert_eq!(
            g.name_index.get("alice"),
            Some(&new_idx),
            "evicting the stale node must not strand the surviving node's key"
        );
    }

    /// Seed 0966: the retcon producer for `invalidate_edge`. After a
    /// speaker/entity supersede fires, `snapshot()` MUST exclude the superseded
    /// entity's old edge specifically via the `valid_until` filter (the edge is
    /// hidden, not deleted), and the relation MUST reappear re-pointed onto the
    /// canonical entity.
    #[test]
    fn supersede_entity_invalidates_old_edge_and_repoints_to_canonical() {
        let mut g = TemporalKnowledgeGraph::new();
        // Build: provisional speaker "Speaker 2" knows "Acme", and the canonical
        // identity "Alice" already exists.
        let ext = ExtractionResult {
            entities: vec![
                entity("Speaker 2"),
                ExtractedEntity {
                    name: "Acme".into(),
                    entity_type: "Organization".into(),
                    description: None,
                },
                entity("Alice"),
            ],
            relations: vec![relation("Speaker 2", "Acme", "works_at")],
        };
        g.process_extraction(&ext, 1.0, "spk", "seg-1");

        // Precondition: the live snapshot shows the Speaker 2 → Acme edge.
        let before = g.snapshot();
        assert_eq!(before.links.len(), 1, "one live edge before retcon");
        let old_edge_id = before.links[0].id.clone();
        let speaker2_id = before
            .nodes
            .iter()
            .find(|n| n.name == "Speaker 2")
            .expect("Speaker 2 node")
            .id
            .clone();
        let alice_id = before
            .nodes
            .iter()
            .find(|n| n.name == "Alice")
            .expect("Alice node")
            .id
            .clone();
        let acme_id = before
            .nodes
            .iter()
            .find(|n| n.name == "Acme")
            .expect("Acme node")
            .id
            .clone();

        // Clear the additive delta so we observe the retcon in isolation.
        let _ = g.take_delta();

        // FIRE THE PRODUCER: "Speaker 2" is actually "Alice".
        let invalidated = g.supersede_entity("Speaker 2", "Alice", 100.0, 1.0);
        assert_eq!(invalidated, 1, "exactly one edge retconned");

        // The underlying edge still EXISTS but now carries valid_until == 100.0,
        // proving the snapshot exclusion goes through the valid_until path (not
        // an outright edge removal).
        let invalidated_edge_present = g
            .graph
            .edge_indices()
            .filter_map(|i| g.graph.edge_weight(i))
            .any(|e| edge_link_id(e.seq_id) == old_edge_id && e.valid_until == Some(100.0));
        assert!(
            invalidated_edge_present,
            "the old edge must remain in the graph with valid_until set (hidden, not deleted)"
        );

        // snapshot() must now EXCLUDE the invalidated edge...
        let after = g.snapshot();
        assert!(
            after.links.iter().all(|l| l.id != old_edge_id),
            "the invalidated edge must be filtered out of the snapshot"
        );
        // ...and the relation must reappear re-pointed onto the canonical node.
        assert_eq!(after.links.len(), 1, "exactly one live edge after retcon");
        let live = &after.links[0];
        assert_eq!(live.relation_type, "works_at");
        assert_eq!(
            live.source, alice_id,
            "re-pointed source is the canonical node"
        );
        assert_eq!(live.target, acme_id, "target endpoint preserved");
        assert_ne!(
            live.source, speaker2_id,
            "the live edge must NOT still originate from the superseded node"
        );
        // b272 provenance policy — clone/new-edge path: the re-pointed live edge
        // PRESERVES the original triggering segment id (it is cloned wholesale
        // from the invalidated edge), so a repoint retcon does not thin the
        // moved relation's provenance.
        assert_eq!(
            live.source_segment_id, "seg-1",
            "re-pointed edge preserves the superseded edge's source segment"
        );

        // The delta surfaces the old edge as removed and the new edge as added.
        let delta = g.take_delta();
        assert!(
            delta.removed_edge_ids.contains(&old_edge_id),
            "invalidated edge id must be surfaced as removed; got {:?}",
            delta.removed_edge_ids
        );
        assert_eq!(delta.added_edges.len(), 1, "one re-pointed edge added");
        assert_eq!(delta.added_edges[0].source, alice_id);
        assert_eq!(
            delta.added_edges[0].source_segment_id, "seg-1",
            "the added re-pointed edge carries the preserved source segment"
        );
    }

    /// Seed 0966: a supersede whose two names resolve to the same node, or whose
    /// names don't resolve, is a no-op — it must NOT invalidate anything.
    #[test]
    fn supersede_entity_is_a_noop_for_self_or_missing() {
        let mut g = TemporalKnowledgeGraph::new();
        let ext = ExtractionResult {
            entities: vec![entity("Alice"), entity("Bob")],
            relations: vec![relation("Alice", "Bob", "knows")],
        };
        g.process_extraction(&ext, 1.0, "spk", "seg-1");
        let _ = g.take_delta();

        // Same node into itself.
        assert_eq!(g.supersede_entity("Alice", "Alice", 50.0, 1.0), 0);
        // Superseded name does not resolve.
        assert_eq!(g.supersede_entity("Nobody", "Alice", 50.0, 1.0), 0);
        // Canonical name does not resolve.
        assert_eq!(g.supersede_entity("Alice", "Nobody", 50.0, 1.0), 0);

        // The original edge is untouched and still live.
        let snap = g.snapshot();
        assert_eq!(snap.links.len(), 1);
        assert!(!g.has_delta(), "no-op merges must not produce a delta");
    }

    /// Seed 0966: when a relation of the SAME type already connects the canonical
    /// node to the other endpoint, re-pointing folds the weight in rather than
    /// creating a duplicate live edge.
    #[test]
    fn supersede_entity_folds_into_existing_canonical_edge() {
        let mut g = TemporalKnowledgeGraph::new();
        // Alice "knows" Bob (from seg-alice), and later the provisional
        // "Speaker 2" also "knows" Bob (from a DISTINCT seg-speaker2). Separate
        // extractions so each edge carries its own source segment id.
        g.process_extraction(
            &ExtractionResult {
                entities: vec![entity("Alice"), entity("Bob")],
                relations: vec![relation("Alice", "Bob", "knows")],
            },
            1.0,
            "spk",
            "seg-alice",
        );
        g.process_extraction(
            &ExtractionResult {
                entities: vec![entity("Speaker 2")],
                relations: vec![relation("Speaker 2", "Bob", "knows")],
            },
            2.0,
            "spk",
            "seg-speaker2",
        );
        let _ = g.take_delta();

        let invalidated = g.supersede_entity("Speaker 2", "Alice", 100.0, 1.0);
        assert_eq!(invalidated, 1);

        let after = g.snapshot();
        // Only ONE live Alice→Bob edge survives (the dup folded in).
        let live: Vec<_> = after
            .links
            .iter()
            .filter(|l| l.relation_type == "knows")
            .collect();
        assert_eq!(live.len(), 1, "duplicate relation folds into one live edge");
        // Its weight is the sum of the two original weights (1.0 + 1.0).
        assert_eq!(live[0].weight, 2.0, "folded edge accumulates weight");
        // Keep the surviving seek hint, but join both contributing turns.
        assert_eq!(
            live[0].source_segment_id, "seg-alice",
            "the fold-in survivor keeps its own seek hint",
        );
        let by_segment = g.live_edges_by_source_segment();
        assert_eq!(by_segment["seg-alice"], [live[0].id.clone()]);
        assert_eq!(by_segment["seg-speaker2"], [live[0].id.clone()]);
    }

    /// 78d0 (P2): after a merge, a LATER mention of the superseded name must
    /// attach to the canonical node — not resurrect the retired one.
    ///
    /// `supersede_entity` re-points every `name_index` key that still targeted
    /// the superseded node onto the canonical node. Without that re-point,
    /// `name_index["speaker 2"]` keeps mapping to the retired node, so a
    /// follow-up `add_relation("Speaker 2", ...)` re-attaches a fresh, live
    /// (`valid_until = None`) edge to the retired node — invisible bookkeeping
    /// that never appears under the merged identity. This test drives exactly
    /// that re-mention and proves the new edge lands on Alice (canonical) and
    /// that the retired Speaker-2 node carries no live edge.
    #[test]
    fn supersede_entity_redirects_later_same_name_mention_to_canonical() {
        let mut g = TemporalKnowledgeGraph::new();
        let ext = ExtractionResult {
            entities: vec![entity("Speaker 2"), entity("Alice"), entity("Bob")],
            relations: vec![relation("Speaker 2", "Bob", "knows")],
        };
        g.process_extraction(&ext, 1.0, "spk", "seg-1");
        let _ = g.take_delta();

        // Capture the node ids BEFORE the merge so we can tell canonical from
        // retired in the post-merge snapshot.
        let speaker2_idx = g
            .resolve_entity("Speaker 2", 1.0)
            .expect("Speaker 2 resolves before merge");
        let alice_idx = g
            .resolve_entity("Alice", 1.0)
            .expect("Alice resolves before merge");
        let retired_id = g
            .graph
            .node_weight(speaker2_idx)
            .expect("retired node exists")
            .id
            .clone();
        let canonical_id = g
            .graph
            .node_weight(alice_idx)
            .expect("canonical node exists")
            .id
            .clone();
        assert_ne!(retired_id, canonical_id, "two distinct nodes pre-merge");

        // Merge Speaker 2 -> Alice.
        assert_eq!(g.supersede_entity("Speaker 2", "Alice", 100.0, 1.0), 1);

        // The old name must now resolve to the CANONICAL node, not the retired
        // one — this is the direct contract that breaks without the fix.
        assert_eq!(
            g.resolve_entity("Speaker 2", 1.0),
            Some(alice_idx),
            "the superseded name must resolve to the canonical node after merge"
        );

        // A LATER mention of the OLD name introduces a brand-new relation.
        let later = ExtractedRelation {
            source: "Speaker 2".to_string(),
            target: "Bob".to_string(),
            relation_type: "greets".to_string(),
            detail: None,
            speaker_endpoint_mask: SpeakerEndpointMask::Neither,
        };
        g.add_relation("Speaker 2", "Bob", &later, 200.0, "seg-2");

        // The new live edge must originate from Alice (canonical), and NO live
        // edge may originate from the retired Speaker-2 node.
        let snap = g.snapshot();
        let greets: Vec<_> = snap
            .links
            .iter()
            .filter(|l| l.relation_type == "greets")
            .collect();
        assert_eq!(greets.len(), 1, "the later mention creates one live edge");
        assert_eq!(
            greets[0].source, canonical_id,
            "re-mention of the superseded name must attach to the canonical node"
        );
        assert!(
            snap.links
                .iter()
                .all(|l| l.source != retired_id && l.target != retired_id),
            "no live edge may touch the retired node after the merge + re-mention"
        );
    }

    /// Finding #55 (P4): `total_episodes` must count `process_extraction` calls
    /// only — NOT edges. Two episodes that add several edges each must report
    /// exactly two episodes.
    #[test]
    fn episode_count_is_not_inflated_by_edges() {
        let mut g = TemporalKnowledgeGraph::new();
        let ext = ExtractionResult {
            entities: vec![entity("A"), entity("B"), entity("C")],
            relations: vec![
                relation("A", "B", "r1"),
                relation("A", "C", "r2"),
                relation("B", "C", "r3"),
            ],
        };
        g.process_extraction(&ext, 1.0, "spk", "seg-1");
        g.process_extraction(&ext, 2.0, "spk", "seg-2");

        // 2 episodes, regardless of the (3 new) edges added in episode 1.
        assert_eq!(g.episode_count(), 2, "episodes must not count edges");
        assert_eq!(g.snapshot().stats.total_episodes, 2);
        // Sanity: there really are edges, so the old per-edge bump WOULD have
        // inflated the count.
        assert_eq!(g.edge_count(), 3);
    }
}
