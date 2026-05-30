# ADR-0008: Built-in conversation ontology for entity/relation extraction

## Status

Accepted; partially implemented (cloud only). Proposed 2026-05-28; accepted 2026-05-30.

> **Status note (2026-05-30):** promoted proposed → accepted. The ontology
> substrate (`ontology.rs`) shipped and the **cloud** extractors use it
> (`llm/openrouter.rs:399`, `llm/api_client.rs:224`). **Pending:** the native
> llama (`llm/engine.rs:283`) and mistral.rs (`llm/mistralrs_engine.rs:123`)
> extractors still hard-code their own type lists — Follow-up #1 below remains
> open. Status/scope clarified per backlog audit 2026-05-30 (B05; native/mistral
> adoption tracked as B04).

## Context

Entity/relation extraction prompts were duplicated as inline string literals in
each LLM backend (`openrouter.rs`, `api_client.rs`, and the native engines),
each hard-coding the type list `Person|Organization|Location|Event|Topic|
Product` and leaving `relation_type` completely free-form. The graph's color
mapping (`graph/entities.rs`) hard-coded a *different*, partially-overlapping
set of strings. Consequences:

- The model invented inconsistent categories and free-form relation verbs, so
  the live graph the user sees is noisy and weakly typed (many disconnected
  nodes, few meaningful edges — visible in the running app).
- The vocabulary missed the *actionable* artifacts of a spoken conversation —
  open **Questions**, **Tasks**/action items, and **Decisions** — even though
  the ReAct agent already surfaces "Question from Speaker 0" style proposals.
- Prompt drift: changing the type set meant editing several files, and the
  color map could (and did) fall out of sync with the prompt.

## Decision Drivers

- One source of truth for the extraction vocabulary, shared by every backend.
- Tuned for spoken conversations/meetings/lectures, not generic web NER.
- A *closed, small* type set — tight ontologies yield cleaner graphs than
  open-ended ones.
- Node/edge colors must derive from the same table as the prompt.
- Keep it provider-agnostic and cheap to extend; leave room for a future
  user-selectable ontology without committing to that complexity now.

## Considered Options

- **Option A — A shared `ontology` module** (Rust) defining `ENTITY_TYPES` and
  `RELATION_TYPES` tables (name + guidance + color), a generated
  `extraction_system_prompt()`, and color lookups. All backends and the graph
  renderer consume it.
- **Option B — Keep per-backend inline prompts**, just expand the type list in
  each. No central module.
- **Option C — Full user-configurable ontology** (load from YAML, editable in
  Settings, schema-validated) now.

## Decision Outcome

Chosen: **Option A**. It removes the duplication, keeps the prompt and colors
in lockstep, and adds the conversation-actionable types (`Task`, `Question`,
`Decision`, plus `Date`) with minimal surface area. Option B perpetuates drift;
Option C is the right *eventual* shape but over-builds for now — Option A is a
clean substrate a later ADR can make user-configurable.

### Consequences

- **Positive**: Single edit point for the vocabulary; all backends extract the
  same typed set; graph colors always match. Action items / questions /
  decisions become first-class nodes, which strengthens the ReAct loop and the
  notes view.
- **Positive**: Stronger prompt (explicit "be conservative", co-reference
  merging, "only extract what's in THIS segment") should cut graph noise.
- **Negative**: A closed set can miss a domain-specific type; mitigated by
  allowing a free lowercase relation verb when none fit, and a neutral color
  fallback for unknown types.
- **Neutral**: Native/mistral.rs extraction can adopt the shared prompt in a
  follow-up; OpenRouter + OpenAI-compatible (the cloud paths) use it now.

## Implementation

- `src-tauri/src/ontology.rs`: `ENTITY_TYPES`, `RELATION_TYPES`,
  `extraction_system_prompt()`, `entity_type_color()`, `relation_type_color()`.
- `llm/openrouter.rs` + `llm/api_client.rs`: `extract_entities()` now calls
  `ontology::extraction_system_prompt()`.
- `graph/entities.rs`: color fns delegate to `ontology`.

## Follow-ups

- Adopt the shared prompt in the native + mistral.rs extractors.
- User-selectable / YAML-defined ontology (future ADR).
- Feed the typed graph into a more structured notes view.

## References

- `src-tauri/src/ontology.rs`
- ADR-0001 (parallel pipeline), ADR-0005 (OpenRouter)
