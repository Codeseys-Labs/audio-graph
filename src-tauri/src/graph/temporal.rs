//! Temporal knowledge graph implementation using petgraph.
//!
//! The graph uses `StableGraph` for stable node/edge indices across mutations.
//! Each edge carries temporal metadata (valid_from, valid_until) for
//! time-aware relationship tracking.

use petgraph::stable_graph::{NodeIndex, StableGraph};
use petgraph::visit::EdgeRef;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

use super::entities::{
    ExtractedEntity, ExtractedRelation, ExtractionResult, GraphDelta, GraphEdge, GraphEntity,
    GraphLink, GraphNode, GraphSnapshot, GraphStats, entity_type_color, relation_type_color,
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
}

/// Maximum number of nodes before eviction of oldest (lowest `last_seen`).
const MAX_NODES: usize = 1000;

/// Maximum number of edges before eviction of oldest (lowest `valid_from`).
const MAX_EDGES: usize = 5000;

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

    /// Get the current episode count.
    pub fn episode_count(&self) -> u64 {
        self.event_counter
    }

    /// Add or update an entity. If entity name exists (case-insensitive),
    /// update `last_seen` and increment `mention_count`. Returns the `NodeIndex`.
    pub fn add_entity(
        &mut self,
        entity: &ExtractedEntity,
        timestamp: f64,
        speaker: &str,
    ) -> NodeIndex {
        let key = entity.name.to_lowercase();

        if let Some(&idx) = self.name_index.get(&key) {
            // Update existing entity
            if let Some(node) = self.graph.node_weight_mut(idx) {
                node.last_seen = timestamp;
                node.mention_count += 1;
                if !node.speakers.contains(&speaker.to_string()) {
                    node.speakers.push(speaker.to_string());
                }
                // Update description if we have a better one
                if entity.description.is_some() && node.description.is_none() {
                    node.description = entity.description.clone();
                }
                // Track as updated (if not already tracked as newly added)
                if !self.delta_added_node_ids.contains(&node.id) {
                    let id = node.id.clone();
                    if !self.delta_updated_node_ids.contains(&id) {
                        self.delta_updated_node_ids.push(id);
                    }
                }
            }
            idx
        } else {
            // Create new entity
            let id = uuid::Uuid::new_v4().to_string();
            self.delta_added_node_ids.push(id.clone());
            let node = GraphEntity {
                id,
                name: entity.name.clone(),
                entity_type: entity.entity_type.clone(),
                mention_count: 1,
                first_seen: timestamp,
                last_seen: timestamp,
                aliases: vec![],
                description: entity.description.clone(),
                speakers: vec![speaker.to_string()],
            };
            let idx = self.graph.add_node(node);
            self.name_index.insert(key, idx);
            idx
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
        let source_key = source_name.to_lowercase();
        let target_key = target_name.to_lowercase();

        let source_idx = match self.name_index.get(&source_key) {
            Some(&idx) => idx,
            None => {
                log::warn!("Source entity '{}' not found in graph", source_name);
                return;
            }
        };
        let target_idx = match self.name_index.get(&target_key) {
            Some(&idx) => idx,
            None => {
                log::warn!("Target entity '{}' not found in graph", target_name);
                return;
            }
        };

        // Check if same relation already exists between these nodes
        let existing_edge = self
            .graph
            .edges_connecting(source_idx, target_idx)
            .find(|e| e.weight().relation_type == relation.relation_type);

        if let Some(edge_ref) = existing_edge {
            let edge_idx = edge_ref.id();
            if let Some(edge) = self.graph.edge_weight_mut(edge_idx) {
                edge.weight += 1.0;
                edge.valid_from = edge.valid_from.min(timestamp); // earliest mention
            }
            // Track as updated so the frontend refreshes edge strength between
            // full snapshots — unless it was newly added this same delta window
            // (in which case the added_edges entry already carries fresh weight).
            if !self.delta_added_edge_indices.contains(&edge_idx)
                && !self.delta_updated_edge_indices.contains(&edge_idx)
            {
                self.delta_updated_edge_indices.push(edge_idx);
            }
        } else {
            // Assign a monotonic, never-reused seq_id (NOT the recyclable
            // EdgeIndex) so the frontend-facing link id stays unique for life.
            let seq_id = self.edge_seq_counter;
            self.edge_seq_counter += 1;
            let edge = TemporalEdge {
                seq_id,
                relation_type: relation.relation_type.clone(),
                valid_from: timestamp,
                valid_until: None,
                confidence: 1.0,
                source_segment_id: segment_id.to_string(),
                detail: relation.detail.clone(),
                weight: 1.0,
            };
            let edge_idx = self.graph.add_edge(source_idx, target_idx, edge);
            self.delta_added_edge_indices.push(edge_idx);
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
    /// This is the sole producer of `valid_until`, which [`Self::snapshot`] and
    /// the delta `build_delta_edge` helper filter on to hide retracted
    /// relations. Its live caller is [`Self::supersede_entity`]: when a
    /// diarization / entity retcon merges a superseded speaker entity into a
    /// canonical one, every edge incident to the superseded node is invalidated
    /// here so it disappears from snapshots and deltas while remaining auditable
    /// in the persisted graph (the row is hidden, not deleted).
    pub fn invalidate_edge(&mut self, edge_idx: petgraph::graph::EdgeIndex, timestamp: f64) {
        if let Some(edge) = self.graph.edge_weight_mut(edge_idx) {
            edge.valid_until = Some(timestamp);
        }
    }

    /// Retcon a superseded entity into a canonical one (speaker / entity merge).
    ///
    /// This is the live producer for [`Self::invalidate_edge`]. When later
    /// context resolves a provisional speaker (e.g. the local diarizer's
    /// `"Speaker 2"`) to a stable identity (e.g. `"Alice"`), or two extracted
    /// entity names turn out to denote the same thing, every relation that was
    /// attached to the *superseded* entity must stop being shown under the old
    /// node and reappear under the canonical node.
    ///
    /// Rather than mutate transcript-derived edges in place (which would lose the
    /// pre-retcon provenance), each incident edge is **invalidated** via
    /// [`Self::invalidate_edge`] — setting `valid_until` so [`Self::snapshot`]
    /// and the delta `build_delta_edge` helper hide it — and an equivalent live
    /// edge is re-created between the *canonical* node and the original other
    /// endpoint. The superseded node itself is then evicted (its
    /// already-invalidated edges cascade out of the delta as removals).
    ///
    /// `timestamp` is the retcon time recorded as the invalidated edges'
    /// `valid_until` and the re-pointed edges' `valid_from`.
    ///
    /// `threshold` is the fuzzy-match cutoff used to resolve both names via
    /// [`Self::resolve_entity`]; pass `1.0` for exact-only resolution.
    ///
    /// The superseded node and its now-invalidated edges are deliberately
    /// **kept** in the graph (and in the persisted file) rather than deleted:
    /// hiding them via `valid_until` preserves the pre-retcon attribution for
    /// audit/replay while keeping them out of the live snapshot. The superseded
    /// node simply has no live edges afterward.
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
        let superseded_idx = match self.resolve_entity(superseded_name, threshold) {
            Some(idx) => idx,
            None => return 0,
        };
        let canonical_idx = match self.resolve_entity(canonical_name, threshold) {
            Some(idx) => idx,
            None => return 0,
        };
        if superseded_idx == canonical_idx {
            // A name that resolves to the same node as the canonical target is a
            // no-op merge — never invalidate an entity into itself.
            return 0;
        }

        // Snapshot the incident edges (BOTH directions, since the graph is
        // directed) before mutating, so we re-point each one onto the canonical
        // node while preserving its direction relative to the other endpoint.
        struct Repoint {
            edge_idx: petgraph::graph::EdgeIndex,
            other: NodeIndex,
            superseded_is_source: bool,
            edge: TemporalEdge,
        }
        let mut repoints: Vec<Repoint> = Vec::new();
        for edge_ref in self
            .graph
            .edges_directed(superseded_idx, petgraph::Direction::Outgoing)
            .chain(
                self.graph
                    .edges_directed(superseded_idx, petgraph::Direction::Incoming),
            )
        {
            let edge_idx = edge_ref.id();
            // Skip already-invalidated edges — they are not live, so there is
            // nothing to hide or re-point.
            if edge_ref.weight().valid_until.is_some() {
                continue;
            }
            let (src, tgt) = match self.graph.edge_endpoints(edge_idx) {
                Some(pair) => pair,
                None => continue,
            };
            let superseded_is_source = src == superseded_idx;
            let other = if superseded_is_source { tgt } else { src };
            // A self-loop on the superseded node collapses to a self-loop on the
            // canonical node; the "other" endpoint becomes the canonical node.
            let other = if other == superseded_idx {
                canonical_idx
            } else {
                other
            };
            repoints.push(Repoint {
                edge_idx,
                other,
                superseded_is_source,
                edge: edge_ref.weight().clone(),
            });
        }

        let invalidated = repoints.len();
        if invalidated == 0 {
            return 0;
        }

        for repoint in repoints {
            // 1) Invalidate the old edge (the producer call): sets valid_until so
            //    snapshot()/build_delta_edge hide it. Surface it as a removal so
            //    the frontend drops the stale link immediately.
            self.invalidate_edge(repoint.edge_idx, timestamp);
            self.delta_removed_edge_ids
                .push(edge_link_id(repoint.edge.seq_id));
            self.delta_added_edge_indices
                .retain(|&ei| ei != repoint.edge_idx);
            self.delta_updated_edge_indices
                .retain(|&ei| ei != repoint.edge_idx);

            // 2) Re-create an equivalent LIVE edge on the canonical node, unless
            //    one with the same relation_type already connects the same
            //    endpoints (in which case fold the weight in to avoid a dup).
            let (new_src, new_tgt) = if repoint.superseded_is_source {
                (canonical_idx, repoint.other)
            } else {
                (repoint.other, canonical_idx)
            };
            let existing = self
                .graph
                .edges_connecting(new_src, new_tgt)
                .find(|e| {
                    e.weight().relation_type == repoint.edge.relation_type
                        && e.weight().valid_until.is_none()
                })
                .map(|e| e.id());
            if let Some(existing_idx) = existing {
                if let Some(edge) = self.graph.edge_weight_mut(existing_idx) {
                    edge.weight += repoint.edge.weight;
                    edge.valid_from = edge.valid_from.min(repoint.edge.valid_from);
                }
                if !self.delta_added_edge_indices.contains(&existing_idx)
                    && !self.delta_updated_edge_indices.contains(&existing_idx)
                {
                    self.delta_updated_edge_indices.push(existing_idx);
                }
            } else {
                let seq_id = self.edge_seq_counter;
                self.edge_seq_counter += 1;
                let mut new_edge = repoint.edge.clone();
                new_edge.seq_id = seq_id;
                new_edge.valid_from = timestamp;
                new_edge.valid_until = None;
                let new_idx = self.graph.add_edge(new_src, new_tgt, new_edge);
                self.delta_added_edge_indices.push(new_idx);
            }
        }

        // Fold the superseded node's mention bookkeeping into the canonical node
        // so the merged identity reflects the combined activity. The superseded
        // node is intentionally NOT removed — it lingers with only invalidated
        // (hidden) edges so the pre-retcon attribution stays auditable.
        if let (Some(superseded), Some(canonical)) = (
            self.graph.node_weight(superseded_idx).cloned(),
            self.graph.node_weight_mut(canonical_idx),
        ) {
            canonical.mention_count = canonical
                .mention_count
                .saturating_add(superseded.mention_count);
            canonical.first_seen = canonical.first_seen.min(superseded.first_seen);
            canonical.last_seen = canonical.last_seen.max(superseded.last_seen);
            for spk in superseded.speakers {
                if !canonical.speakers.contains(&spk) {
                    canonical.speakers.push(spk);
                }
            }
            if canonical.description.is_none() && superseded.description.is_some() {
                canonical.description = superseded.description;
            }
            // The canonical node changed — surface it as updated.
            let canonical_id = canonical.id.clone();
            if !self.delta_added_node_ids.contains(&canonical_id)
                && !self.delta_updated_node_ids.contains(&canonical_id)
            {
                self.delta_updated_node_ids.push(canonical_id);
            }
        }

        invalidated
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
        // First, add/update all entities
        for entity in &result.entities {
            self.add_entity(entity, timestamp, speaker);
        }

        // Then, add all relations
        for relation in &result.relations {
            self.add_relation(
                &relation.source,
                &relation.target,
                relation,
                timestamp,
                segment_id,
            );
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
        // Remove from name_index before removing from graph.
        if let Some(entity) = self.graph.node_weight(idx) {
            let key = entity.name.to_lowercase();
            // Only drop the name_index entry if it still points at THIS node. A
            // later node inserted under the same lowercased key may have
            // overwritten the mapping; removing it then would strand the
            // surviving node (its key would resolve to nothing).
            if self.name_index.get(&key) == Some(&idx) {
                self.name_index.remove(&key);
            }
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

        // The delta surfaces the old edge as removed and the new edge as added.
        let delta = g.take_delta();
        assert!(
            delta.removed_edge_ids.contains(&old_edge_id),
            "invalidated edge id must be surfaced as removed; got {:?}",
            delta.removed_edge_ids
        );
        assert_eq!(delta.added_edges.len(), 1, "one re-pointed edge added");
        assert_eq!(delta.added_edges[0].source, alice_id);
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
        // Both "Speaker 2" and "Alice" already "know" "Bob".
        let ext = ExtractionResult {
            entities: vec![entity("Speaker 2"), entity("Alice"), entity("Bob")],
            relations: vec![
                relation("Speaker 2", "Bob", "knows"),
                relation("Alice", "Bob", "knows"),
            ],
        };
        g.process_extraction(&ext, 1.0, "spk", "seg-1");
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
