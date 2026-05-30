//! Entity and relation type definitions for the knowledge graph.
//!
//! These types are serialized to JSON and sent to the frontend.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A node in the knowledge graph representing a named entity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEntity {
    /// Stable node ID.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Entity type: Person, Organization, Location, Event, Topic, Product, etc.
    pub entity_type: String,
    /// Number of times this entity has been mentioned.
    pub mention_count: u32,
    /// Timestamp of first mention (seconds since capture start).
    pub first_seen: f64,
    /// Timestamp of most recent mention.
    pub last_seen: f64,
    /// Alternative names / spellings.
    pub aliases: Vec<String>,
    /// Optional description for the entity.
    pub description: Option<String>,
    /// Which speakers mentioned this entity.
    pub speakers: Vec<String>,
}

/// An edge in the knowledge graph representing a relationship between entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphRelation {
    /// Stable edge ID.
    pub id: String,
    /// Source entity ID.
    pub source_id: String,
    /// Target entity ID.
    pub target_id: String,
    /// Relationship type: WORKS_AT, LOCATED_IN, KNOWS, etc.
    pub relation_type: String,
    /// When this relationship became valid.
    pub valid_from: f64,
    /// When this relationship ceased to be valid (None = still valid).
    pub valid_until: Option<f64>,
    /// Extraction confidence score.
    pub confidence: f32,
    /// ID of the transcript segment that sourced this relation.
    pub source_segment_id: String,
}

// ---------------------------------------------------------------------------
// Frontend-friendly snapshot types (react-force-graph compatible)
// ---------------------------------------------------------------------------

/// A graph node ready for react-force-graph rendering.
#[derive(Debug, Clone, Serialize)]
pub struct GraphNode {
    pub id: String,
    pub name: String,
    pub entity_type: String,
    /// Node size (based on mention_count).
    pub val: f32,
    /// Color by entity_type.
    pub color: String,
    pub first_seen: f64,
    pub last_seen: f64,
    pub mention_count: u32,
    pub description: Option<String>,
}

/// A graph link ready for react-force-graph rendering.
#[derive(Debug, Clone, Serialize)]
pub struct GraphLink {
    /// Stable edge ID.
    pub id: String,
    /// Source node id.
    pub source: String,
    /// Target node id.
    pub target: String,
    pub relation_type: String,
    pub weight: f32,
    pub color: String,
    pub label: Option<String>,
}

/// Aggregate graph statistics.
#[derive(Debug, Clone, Serialize, Default)]
pub struct GraphStats {
    pub total_nodes: usize,
    pub total_edges: usize,
    pub total_episodes: u64,
}

/// A point-in-time snapshot of the knowledge graph for frontend rendering.
#[derive(Debug, Clone, Serialize, Default)]
pub struct GraphSnapshot {
    /// All nodes in react-force-graph format.
    pub nodes: Vec<GraphNode>,
    /// All links in react-force-graph format.
    pub links: Vec<GraphLink>,
    /// Aggregate statistics.
    pub stats: GraphStats,
}

/// Delta update for the knowledge graph (incremental changes since last delta).
///
/// Emitted via the `GRAPH_DELTA` event to avoid sending the full snapshot on
/// every extraction cycle. The frontend can apply these deltas to its local
/// graph state for efficient updates.
#[derive(Debug, Clone, Serialize, Default)]
pub struct GraphDelta {
    /// Nodes added since the last delta.
    pub added_nodes: Vec<GraphNode>,
    /// Nodes that were updated (e.g. mention_count changed) since the last delta.
    pub updated_nodes: Vec<GraphNode>,
    /// Edges added since the last delta.
    pub added_edges: Vec<GraphEdge>,
    /// Edges whose weight/label changed (but were not newly added) since the
    /// last delta. The frontend merges these onto existing links by `id` so
    /// edge strength stays current between full snapshots.
    pub updated_edges: Vec<GraphEdge>,
    /// IDs of nodes removed (evicted) since the last delta.
    pub removed_node_ids: Vec<String>,
    /// IDs of edges removed (evicted) since the last delta.
    pub removed_edge_ids: Vec<String>,
    /// Timestamp of this delta.
    pub timestamp: f64,
}

/// A single edge in delta format, carrying source/target node IDs for the
/// frontend to create links.
#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    /// Unique edge identifier.
    pub id: String,
    /// Source node ID.
    pub source: String,
    /// Target node ID.
    pub target: String,
    /// Relationship type.
    pub relation_type: String,
    /// Edge weight (strength).
    pub weight: f32,
    /// Display color.
    pub color: String,
    /// Optional label.
    pub label: Option<String>,
}

/// Result of entity extraction from a transcript segment (from native LLM or rule-based).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExtractionResult {
    pub entities: Vec<ExtractedEntity>,
    pub relations: Vec<ExtractedRelation>,
}

/// A raw entity extracted from text (before resolution).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedEntity {
    pub name: String,
    /// Entity type: "Person", "Organization", "Location", "Event", "Topic", "Product".
    pub entity_type: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// A raw relation extracted from text (before graph insertion).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedRelation {
    pub source: String,
    pub target: String,
    pub relation_type: String,
    #[serde(default)]
    pub detail: Option<String>,
}

// ---------------------------------------------------------------------------
// Color helpers
// ---------------------------------------------------------------------------

/// Map an entity type to a hex color string.
///
/// Delegates to the shared [`crate::ontology`] table so node colors stay in
/// sync with the extraction vocabulary.
pub fn entity_type_color(entity_type: &str) -> &'static str {
    crate::ontology::entity_type_color(entity_type)
}

/// Map a relation type to a hex color string.
pub fn relation_type_color(relation_type: &str) -> &'static str {
    crate::ontology::relation_type_color(relation_type)
}

// ---------------------------------------------------------------------------
// Chat-context retrieval (top-k RAG)
// ---------------------------------------------------------------------------

/// Tokenize for relevance scoring: lowercase alphanumeric words >2 chars.
fn context_tokens(s: &str) -> std::collections::HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 2)
        .map(|t| t.to_string())
        .collect()
}

/// Build the knowledge-graph context string for a chat prompt, focused on the
/// nodes most relevant to `query` (top-k retrieval) rather than dumping the
/// entire graph.
///
/// Why: at the 1000-node cap a full dump is large, slow, token-expensive, and
/// ships maximal session data to a user-configurable endpoint. Here we rank
/// nodes by query-term overlap (with a mild `mention_count` centrality
/// tiebreak), keep the top `max_nodes`, and include only relationships whose
/// BOTH endpoints survived — so the context stays small, on-topic, and
/// coherent. For graphs at/under `max_nodes` the result is the whole graph
/// (same content as before), so small sessions are unaffected.
pub fn build_graph_chat_context(
    snapshot: &GraphSnapshot,
    query: &str,
    max_nodes: usize,
) -> String {
    let q_tokens = context_tokens(query);

    let mut scored: Vec<(f64, &GraphNode)> = snapshot
        .nodes
        .iter()
        .map(|n| {
            let mut text = n.name.clone();
            if let Some(d) = &n.description {
                text.push(' ');
                text.push_str(d);
            }
            let overlap = context_tokens(&text)
                .iter()
                .filter(|t| q_tokens.contains(*t))
                .count();
            // Relevance dominates; mention_count is a gentle centrality tiebreak
            // so an empty/irrelevant query still yields the most-central nodes.
            let score = (overlap as f64) * 100.0 + (n.mention_count as f64).ln_1p();
            (score, n)
        })
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let selected: Vec<&GraphNode> = scored.into_iter().take(max_nodes).map(|(_, n)| n).collect();
    let selected_ids: std::collections::HashSet<&str> =
        selected.iter().map(|n| n.id.as_str()).collect();
    let id_to_name: std::collections::HashMap<&str, &str> =
        selected.iter().map(|n| (n.id.as_str(), n.name.as_str())).collect();
    let truncated = snapshot.nodes.len() > selected.len();

    let mut ctx = String::new();
    if truncated {
        ctx.push_str(&format!(
            "Entities (top {} of {} most relevant to the question):\n",
            selected.len(),
            snapshot.nodes.len()
        ));
    } else {
        ctx.push_str(&format!("Entities ({}):\n", selected.len()));
    }
    for n in &selected {
        ctx.push_str(&format!("- {} ({})", n.name, n.entity_type));
        if let Some(d) = &n.description {
            ctx.push_str(&format!(": {}", d));
        }
        ctx.push('\n');
    }

    // Only relationships between two selected nodes, printed with display names.
    let edges: Vec<&GraphLink> = snapshot
        .links
        .iter()
        .filter(|l| {
            selected_ids.contains(l.source.as_str()) && selected_ids.contains(l.target.as_str())
        })
        .collect();
    ctx.push_str(&format!("\nRelationships ({}):\n", edges.len()));
    for l in &edges {
        let src = id_to_name.get(l.source.as_str()).copied().unwrap_or(&l.source);
        let tgt = id_to_name.get(l.target.as_str()).copied().unwrap_or(&l.target);
        ctx.push_str(&format!("- {} → {} ({})\n", src, tgt, l.relation_type));
    }
    ctx
}

#[cfg(test)]
mod chat_context_tests {
    use super::*;

    fn node(id: &str, name: &str, mentions: u32) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            name: name.to_string(),
            entity_type: "Topic".to_string(),
            val: 1.0,
            color: "#000".to_string(),
            first_seen: 0.0,
            last_seen: 0.0,
            mention_count: mentions,
            description: None,
        }
    }
    fn link(source: &str, target: &str) -> GraphLink {
        GraphLink {
            id: format!("{source}->{target}"),
            source: source.to_string(),
            target: target.to_string(),
            relation_type: "related_to".to_string(),
            weight: 1.0,
            color: "#000".to_string(),
            label: None,
        }
    }

    #[test]
    fn small_graph_includes_everything() {
        let snap = GraphSnapshot {
            nodes: vec![node("a", "Alpha", 1), node("b", "Beta", 1)],
            links: vec![link("a", "b")],
            stats: Default::default(),
        };
        let ctx = build_graph_chat_context(&snap, "anything", 40);
        assert!(ctx.contains("Alpha") && ctx.contains("Beta"));
        assert!(ctx.contains("Entities (2)")); // not truncated
        assert!(ctx.contains("Alpha → Beta")); // edge uses names
    }

    #[test]
    fn large_graph_keeps_query_relevant_nodes() {
        let mut nodes = vec![node("relevant", "Quantum Computing", 1)];
        for i in 0..60 {
            nodes.push(node(&format!("n{i}"), &format!("Filler{i}"), 1));
        }
        let snap = GraphSnapshot { nodes, links: vec![], stats: Default::default() };
        let ctx = build_graph_chat_context(&snap, "tell me about quantum computing", 10);
        assert!(ctx.contains("Quantum Computing"), "relevant node must survive top-k");
        assert!(ctx.contains("top 10 of 61")); // truncated header
    }

    #[test]
    fn empty_query_falls_back_to_most_mentioned() {
        let snap = GraphSnapshot {
            nodes: vec![node("a", "Rare", 1), node("b", "Central", 99)],
            links: vec![],
            stats: Default::default(),
        };
        let ctx = build_graph_chat_context(&snap, "", 1);
        assert!(ctx.contains("Central") && !ctx.contains("Rare"));
    }

    #[test]
    fn drops_edges_to_unselected_nodes() {
        let mut nodes = vec![node("keep", "KeepMe", 50)];
        for i in 0..30 {
            nodes.push(node(&format!("d{i}"), &format!("Drop{i}"), 1));
        }
        // Edge from the kept node to a low-ranked one that won't be selected.
        let snap = GraphSnapshot {
            nodes,
            links: vec![link("keep", "d29")],
            stats: Default::default(),
        };
        let ctx = build_graph_chat_context(&snap, "", 1);
        assert!(ctx.contains("Relationships (0)"), "edge to unselected node dropped");
    }
}
