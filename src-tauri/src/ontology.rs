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
    EntityType {
        name: "Person",
        guidance: "a named or referenced individual (incl. speakers)",
        color: "#4CAF50",
    },
    EntityType {
        name: "Organization",
        guidance: "a company, team, school, group, or institution",
        color: "#2196F3",
    },
    EntityType {
        name: "Location",
        guidance: "a physical or virtual place",
        color: "#FF9800",
    },
    EntityType {
        name: "Event",
        guidance: "a meeting, class, deadline, or scheduled happening",
        color: "#9C27B0",
    },
    EntityType {
        name: "Topic",
        guidance: "a subject, concept, or theme being discussed",
        color: "#00BCD4",
    },
    EntityType {
        name: "Product",
        guidance: "a tool, app, document, or concrete artifact",
        color: "#F44336",
    },
    EntityType {
        name: "Task",
        guidance: "an action item or to-do someone should do",
        color: "#FFC107",
    },
    EntityType {
        name: "Question",
        guidance: "an open question raised that wants an answer",
        color: "#E91E63",
    },
    EntityType {
        name: "Decision",
        guidance: "a choice or conclusion the participants reached",
        color: "#8BC34A",
    },
    EntityType {
        name: "Date",
        guidance: "a date, time, or temporal reference",
        color: "#795548",
    },
];

/// Suggested/preferred relation types — NOT a closed set (the header used
/// to say "Closed set", which contradicted the very next sentence and this
/// const's own behavior: `relation_type` SHOULD be one of these, but the
/// model may emit another lowercase verb phrase when none fit, and nothing
/// downstream rejects that). Corrected during audio-graph-e700, whose
/// ticket explicitly asked to investigate this exact question ("does the
/// ontology define a CLOSED relation set?") before deciding whether to bind
/// `relation_type` to an enum on the wire the way `entity_type` now is; the
/// answer, confirmed against every actual caller, is no.
pub const RELATION_TYPES: &[RelationType] = &[
    RelationType {
        name: "mentions",
        guidance: "X refers to / brings up Y",
        color: "#2196F3",
    },
    RelationType {
        name: "works_at",
        guidance: "person is affiliated with an organization",
        color: "#4CAF50",
    },
    RelationType {
        name: "located_in",
        guidance: "X is situated in place Y",
        color: "#FF9800",
    },
    RelationType {
        name: "related_to",
        guidance: "generic association between X and Y",
        color: "#9E9E9E",
    },
    RelationType {
        name: "asks",
        guidance: "person raises a Question",
        color: "#E91E63",
    },
    RelationType {
        name: "assigned_to",
        guidance: "a Task is owned by a Person",
        color: "#FFC107",
    },
    RelationType {
        name: "decided",
        guidance: "a Person/group reached a Decision",
        color: "#8BC34A",
    },
    RelationType {
        name: "part_of",
        guidance: "X is a component/member of Y",
        color: "#673AB7",
    },
    RelationType {
        name: "scheduled_for",
        guidance: "an Event/Task is tied to a Date",
        color: "#795548",
    },
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

/// Deterministic ingest-side fallback for a model-supplied `entity_type`
/// string that did not survive schema-level enum binding (seed
/// audio-graph-e700 sub-fix 1): a non-strict route, a repair-prompt retry,
/// or a strict-mode provider that ignored the enum anyway. Always returns
/// one of [`ENTITY_TYPES`]'s ten canonical names — never a new invented
/// category — via three tiers:
///
/// 1. Case-insensitive exact match (`"PERSON"`, `"person"` -> `"Person"`).
/// 2. A small curated synonym table for common near-synonyms
///    ([`entity_type_synonym`]) — deliberately NOT fuzzy matching, because
///    fuzzy string similarity on a short closed vocabulary of category
///    *names* is unreliable for genuine synonyms: measured
///    `jaro_winkler("company", "location") = 0.607` and
///    `jaro_winkler("concept", "product") = 0.619` are indistinguishable
///    from noise, while a curated table gets both right deterministically.
/// 3. A near-misspelling of a canonical name only, caught by a
///    deliberately HIGH Jaro-Winkler threshold: measured
///    `jaro_winkler("persn", "person") = 0.967` and
///    `jaro_winkler("orgnization", "organization") = 0.938` clear it, while
///    the semantic-synonym false positives in tier 2's doc comment (0.607,
///    0.619) stay well below it.
///
/// Anything that clears none of the three tiers falls back to `"Topic"` —
/// [`ENTITY_TYPES`]'s existing broadest bucket ("a subject, concept, or
/// theme being discussed"), not a fabricated `"Other"` category (which would
/// be an ontology expansion, out of scope for this fallback).
///
/// Ingest-time only: called from
/// `projection_llm::normalize_projection_patch_draft_ontology` on a FRESH
/// model draft, before it becomes a trusted, persisted `ProjectionPatch`.
/// Never called from `MaterializedGraph::apply_patch` (replay) — a session
/// persisted before this fallback existed keeps its original free-string
/// `entity_type` forever; replay tolerates it exactly as it always has
/// (ADR-0029 / ADR-0045: materialization is a pure re-derivation from the
/// accepted patch log, which this fallback never rewrites).
pub fn normalize_entity_type(raw: &str) -> &'static str {
    let trimmed = raw.trim();
    if let Some(exact) = ENTITY_TYPES
        .iter()
        .find(|t| t.name.eq_ignore_ascii_case(trimmed))
    {
        return exact.name;
    }

    let lower = trimmed.to_lowercase();
    if let Some(synonym) = entity_type_synonym(&lower) {
        return synonym;
    }

    const TYPO_THRESHOLD: f64 = 0.93;
    let mut best: Option<(&'static str, f64)> = None;
    for t in ENTITY_TYPES {
        let score = strsim::jaro_winkler(&lower, &t.name.to_lowercase());
        if score >= TYPO_THRESHOLD && best.is_none_or(|(_, current)| score > current) {
            best = Some((t.name, score));
        }
    }
    if let Some((name, _)) = best {
        return name;
    }

    "Topic"
}

/// Curated near-synonym table backing tier 2 of [`normalize_entity_type`].
/// `lower` must already be trimmed and lowercased. Kept intentionally small
/// and explicit (no fuzzy scoring) — see [`normalize_entity_type`]'s doc
/// comment for why fuzzy matching a short category vocabulary for semantic
/// synonyms is unreliable. Not exhaustive: the STOP CONDITION in seed
/// audio-graph-e700 forbids reading the field session that produced the 31
/// invented types this ticket cites, so this table is built from generally
/// plausible near-synonyms, not that measured list.
fn entity_type_synonym(lower: &str) -> Option<&'static str> {
    Some(match lower {
        "company" | "team" | "group" | "institution" | "org" | "employer" => "Organization",
        "place" | "venue" | "site" | "region" | "city" | "country" => "Location",
        "meeting" | "appointment" | "deadline" | "milestone" => "Event",
        "concept" | "subject" | "theme" | "idea" | "issue" => "Topic",
        "tool" | "app" | "application" | "artifact" | "document" | "technology" | "provider"
        | "library" | "service" | "software" => "Product",
        "todo" | "action" | "action_item" => "Task",
        "conclusion" | "choice" | "resolution" | "verdict" => "Decision",
        "time" | "datetime" | "timestamp" => "Date",
        "speaker" | "individual" | "human" | "attendee" | "participant" => "Person",
        "inquiry" | "query" => "Question",
        _ => return None,
    })
}

/// Deterministic soft-normalization for a model-supplied `relation_type`
/// string (seed audio-graph-e700 sub-fix "RELATION TYPES"). Unlike
/// `entity_type`, [`RELATION_TYPES`] is NOT a hard-closed set — its own doc
/// comment used to open with "Closed set", which contradicted the very
/// next sentence AND [`extraction_system_prompt`]'s guidance to the model
/// that it "may emit another lowercase verb phrase when none fit"; that
/// header has been corrected (see [`RELATION_TYPES`]'s doc comment) rather
/// than silently resolved in whichever direction happened to be cheaper. So
/// this never rejects or remaps onto a fixed vocabulary — it only collapses
/// trivial surface-form variance (case, whitespace, punctuation) so
/// `"Works At"`, `"works-at"`,
/// and `"works_at"` land on the SAME persisted string instead of forking
/// into distinct relation types that only differ by formatting. Idempotent:
/// normalizing an already-normalized string is a no-op. An empty or
/// all-punctuation input falls back to `"related_to"` — an existing
/// [`RELATION_TYPES`] entry ("generic association between X and Y"), not an
/// invented catch-all.
pub fn normalize_relation_type(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_was_separator = true; // suppress a leading underscore
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_separator = false;
        } else if !last_was_separator {
            out.push('_');
            last_was_separator = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    if out.is_empty() {
        "related_to".to_string()
    } else {
        out
    }
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

    #[test]
    fn normalize_entity_type_matches_case_insensitively() {
        assert_eq!(normalize_entity_type("person"), "Person");
        assert_eq!(normalize_entity_type("PERSON"), "Person");
        assert_eq!(normalize_entity_type("  Question  "), "Question");
    }

    #[test]
    fn normalize_entity_type_applies_curated_synonyms() {
        assert_eq!(normalize_entity_type("company"), "Organization");
        assert_eq!(normalize_entity_type("provider"), "Product");
        assert_eq!(normalize_entity_type("place"), "Location");
        assert_eq!(normalize_entity_type("meeting"), "Event");
        assert_eq!(normalize_entity_type("todo"), "Task");
        assert_eq!(normalize_entity_type("conclusion"), "Decision");
        assert_eq!(normalize_entity_type("datetime"), "Date");
    }

    #[test]
    fn normalize_entity_type_catches_near_misspellings() {
        assert_eq!(normalize_entity_type("persn"), "Person");
        assert_eq!(normalize_entity_type("orgnization"), "Organization");
    }

    #[test]
    fn normalize_entity_type_falls_back_to_topic_for_unrecognized_input() {
        // These score well below the typo threshold against every canonical
        // name (measured: best fuzzy hit for "SECRET TYPE" is "event" at
        // 0.624) and have no curated synonym, so they hit the final
        // catch-all rather than fabricating a new category.
        assert_eq!(normalize_entity_type("SECRET TYPE"), "Topic");
        assert_eq!(normalize_entity_type("Widget Category 47"), "Topic");
        assert_eq!(normalize_entity_type(""), "Topic");
    }

    #[test]
    fn normalize_relation_type_collapses_surface_form_variance() {
        assert_eq!(normalize_relation_type("Works At"), "works_at");
        assert_eq!(normalize_relation_type("works-at"), "works_at");
        assert_eq!(normalize_relation_type("works_at"), "works_at");
        assert_eq!(normalize_relation_type("  WORKS   AT  "), "works_at");
    }

    #[test]
    fn normalize_relation_type_is_idempotent() {
        let once = normalize_relation_type("Reports To!!");
        let twice = normalize_relation_type(&once);
        assert_eq!(once, twice);
    }

    #[test]
    fn normalize_relation_type_falls_back_to_related_to_for_empty_input() {
        assert_eq!(normalize_relation_type(""), "related_to");
        assert_eq!(normalize_relation_type("   ---   "), "related_to");
    }
}
