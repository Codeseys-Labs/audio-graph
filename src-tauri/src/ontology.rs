//! Conversation knowledge-graph **ontology** — the single source of truth for
//! the entity and relation types AudioGraph extracts from speech.
//!
//! Both the LLM extraction prompts (OpenRouter / OpenAI-compatible / native)
//! and the graph's color mapping derive from the tables here, so the model is
//! steered toward a consistent, typed vocabulary instead of inventing ad-hoc
//! categories. The vocabulary is tuned for **spoken conversations / meetings /
//! lectures** (not generic web text): alongside the classic NER types it adds
//! `Question`, `Task`, and `Decision`, which are the actionable artifacts the
//! ReAct agent surfaces.
//!
//! Keep this list small and stable — a tight closed set yields far cleaner
//! graphs than an open-ended one. A future ADR may make the ontology
//! user-selectable; until then this is the built-in default.

/// One entity type in the ontology.
pub struct EntityType {
    /// Canonical PascalCase name emitted in the `entity_type` field.
    pub name: &'static str,
    /// One-line guidance the LLM sees, describing what belongs in this type.
    pub guidance: &'static str,
    /// Node color (hex) used by the graph renderer.
    pub color: &'static str,
}

/// One relation type in the ontology.
pub struct RelationType {
    pub name: &'static str,
    pub guidance: &'static str,
    pub color: &'static str,
}

/// Closed set of entity types. Order is the order shown to the model.
pub const ENTITY_TYPES: &[EntityType] = &[
    EntityType { name: "Person", guidance: "a named or referenced individual (incl. speakers)", color: "#4CAF50" },
    EntityType { name: "Organization", guidance: "a company, team, school, group, or institution", color: "#2196F3" },
    EntityType { name: "Location", guidance: "a physical or virtual place", color: "#FF9800" },
    EntityType { name: "Event", guidance: "a meeting, class, deadline, or scheduled happening", color: "#9C27B0" },
    EntityType { name: "Topic", guidance: "a subject, concept, or theme being discussed", color: "#00BCD4" },
    EntityType { name: "Product", guidance: "a tool, app, document, or concrete artifact", color: "#F44336" },
    EntityType { name: "Task", guidance: "an action item or to-do someone should do", color: "#FFC107" },
    EntityType { name: "Question", guidance: "an open question raised that wants an answer", color: "#E91E63" },
    EntityType { name: "Decision", guidance: "a choice or conclusion the participants reached", color: "#8BC34A" },
    EntityType { name: "Date", guidance: "a date, time, or temporal reference", color: "#795548" },
];

/// Closed set of relation types. `relation_type` SHOULD be one of these, but
/// the model may emit another lowercase verb phrase when none fit.
pub const RELATION_TYPES: &[RelationType] = &[
    RelationType { name: "mentions", guidance: "X refers to / brings up Y", color: "#2196F3" },
    RelationType { name: "works_at", guidance: "person is affiliated with an organization", color: "#4CAF50" },
    RelationType { name: "located_in", guidance: "X is situated in place Y", color: "#FF9800" },
    RelationType { name: "related_to", guidance: "generic association between X and Y", color: "#9E9E9E" },
    RelationType { name: "asks", guidance: "person raises a Question", color: "#E91E63" },
    RelationType { name: "assigned_to", guidance: "a Task is owned by a Person", color: "#FFC107" },
    RelationType { name: "decided", guidance: "a Person/group reached a Decision", color: "#8BC34A" },
    RelationType { name: "part_of", guidance: "X is a component/member of Y", color: "#673AB7" },
    RelationType { name: "scheduled_for", guidance: "an Event/Task is tied to a Date", color: "#795548" },
];

/// The `entity_type` enum string for the JSON schema, e.g.
/// `"Person|Organization|...|Date"`.
pub fn entity_type_enum() -> String {
    ENTITY_TYPES
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join("|")
}

/// Build the system prompt used for entity/relation extraction. Shared by all
/// LLM backends so extraction is consistent regardless of provider.
pub fn extraction_system_prompt() -> String {
    let entity_lines = ENTITY_TYPES
        .iter()
        .map(|t| format!("  - {}: {}", t.name, t.guidance))
        .collect::<Vec<_>>()
        .join("\n");
    let relation_lines = RELATION_TYPES
        .iter()
        .map(|t| format!("  - {}: {}", t.name, t.guidance))
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        "You extract a structured knowledge graph from one segment of a live spoken \
conversation. Use ONLY these entity types:\n{entity_lines}\n\nPrefer these relation types \
(use a short lowercase verb phrase only if none fit):\n{relation_lines}\n\n\
Rules:\n\
- Only extract entities explicitly present or clearly referenced in THIS segment. Do not invent.\n\
- Normalize names (trim filler, no leading articles). Merge obvious co-references.\n\
- Capture action items as Task, open questions as Question, and conclusions as Decision.\n\
- Be conservative: an empty result is better than noise.\n\n\
Output ONLY valid JSON with this exact shape:\n\
{{\"entities\": [{{\"name\": \"...\", \"entity_type\": \"{enum_}\", \"description\": \"...\"}}], \
\"relations\": [{{\"source\": \"...\", \"target\": \"...\", \"relation_type\": \"...\", \"detail\": \"...\"}}]}}\n\
If nothing is found, return {{\"entities\": [], \"relations\": []}}.",
        entity_lines = entity_lines,
        relation_lines = relation_lines,
        enum_ = entity_type_enum(),
    )
}

/// Color for an entity type (case-insensitive); falls back to a neutral gray.
pub fn entity_type_color(entity_type: &str) -> &'static str {
    ENTITY_TYPES
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(entity_type))
        .map(|t| t.color)
        .unwrap_or("#607D8B")
}

/// Color for a relation type (case-insensitive); falls back to a neutral gray.
pub fn relation_type_color(relation_type: &str) -> &'static str {
    RELATION_TYPES
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(relation_type))
        .map(|t| t.color)
        .unwrap_or("#757575")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_contains_all_types() {
        let e = entity_type_enum();
        assert!(e.starts_with("Person|"));
        assert!(e.contains("Question"));
        assert!(e.contains("Task"));
        assert!(e.ends_with("Date"));
    }

    #[test]
    fn prompt_lists_types_and_json_shape() {
        let p = extraction_system_prompt();
        assert!(p.contains("Person"));
        assert!(p.contains("Question"));
        assert!(p.contains("\"entities\""));
        assert!(p.contains("\"relations\""));
    }

    #[test]
    fn colors_resolve_case_insensitively() {
        assert_eq!(entity_type_color("person"), "#4CAF50");
        assert_eq!(entity_type_color("Question"), "#E91E63");
        assert_eq!(entity_type_color("unknown"), "#607D8B");
        assert_eq!(relation_type_color("MENTIONS"), "#2196F3");
        assert_eq!(relation_type_color("nope"), "#757575");
    }
}
