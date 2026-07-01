# ADR-0014: On-demand notes synthesis (narrative parallel to the graph)

> **Superseded by [ADR-0024](0024-event-sourced-notes-graph-projections.md)** on
> 2026-06-30. The live notes surface is now an event-sourced projection of an
> immutable transcript event log (basis-checked, incremental `UpsertNote`/
> `DeleteNote`/`ReorderNote` patches, replayed from the durable projection log),
> not an on-demand whole-conversation synthesis. The `synthesize_notes` command
> this ADR proposed survives as a manual, user-triggered prose escape hatch —
> see ADR-0024 for the migration.

## Status

Superseded by ADR-0024 (2026-06-30). Originally accepted 2026-05-29; implemented
the `synthesize_notes` command, which is retained as a manual escape hatch.

## Context

The Notes panel (`src/components/NotesPanel.tsx`) is currently a **categorized
dump of graph entity labels** — it filters typed nodes (Question/Task/Decision/
Topic/Person) and lists their bare `name` strings. It performs no synthesis, and
notably does not even render the `description` field the extractor already
produces. The result reads as "questions and blurbs that happened to be noted
down," not as notes (W3.6 in `docs/reviews/2026-05-29-uiux-deep-dive.md`).

The graph and notes are meant to be **parallel views of the same conversation**:
the graph is the structured/spatial projection; notes should be the **narrative**
projection.

A canvas (2026-05-29) found the synthesis infrastructure already exists and is
reusable: `prepare_chat_request` assembles graph context (`build_graph_chat_context`,
which falls back to most-central nodes on an empty query) + a transcript window,
and `executor.chat_with_history` runs LLM generation at Interactive priority with
a provider fallback chain. There is no `summarize` command yet.

## Decision Drivers

- Notes should be coherent narrative + key points + action items + decisions +
  open questions — grounded in the same data as the graph.
- Reuse the existing chat/LLM pipeline; avoid new model plumbing.
- Keep the always-on, zero-cost categorized view; don't spend tokens on every
  graph tick.
- Stay consistent with the graph (same `GraphSnapshot` + transcript source).

## Considered Options

- **Option A — Add a backend `synthesize_notes` command + an on-demand
  "Synthesize" action in NotesPanel.** Reuses `build_graph_chat_context`
  (whole-conversation / most-central context) + a summarization system prompt
  through `executor.chat_with_history`. NotesPanel keeps its cheap live
  categorized chips as the base layer and renders synthesized prose above them
  when the user (or an idle/interval trigger) requests it.
- **Option B — Auto-summarize continuously** as the graph updates. Always fresh,
  but burns tokens, races extraction back-pressure, and fights the 429 cooldown.
- **Option C — Client-only templated sentences** from node fields (no LLM).
  Cheap and offline, but not real synthesis; brittle template prose.

## Decision Outcome

Chosen: **Option A**. It produces real notes by reusing the proven chat pipeline
with minimal new code (one command + one prompt + one frontend action), keeps
the live categorized view as a free fallback, and controls cost by being
on-demand (with an optional debounced/idle auto-refresh later). Option B is
wasteful and back-pressure-prone; Option C isn't actually synthesis.

### Consequences

- **Positive:** Notes become a genuine narrative parallel to the graph; the
  `description` field and relations finally inform the notes view.
- **Positive:** Reuses graph grounding, so notes and graph stay consistent.
- **Negative:** On-demand means notes can lag the live graph until refreshed —
  acceptable, and surfaced via a "synthesized at <time>" / stale indicator.
- **Negative:** One new backend command (collision-aware: reuses existing
  helpers, adds no new provider logic).
- **Neutral:** Optional structured (JSON) output can drive sectioned rendering
  later; start with prose + the existing chips.

## Implementation (intended)

- Backend: `synthesize_notes` command modeled on `send_chat_message`, using a
  whole-conversation `build_graph_chat_context` + larger transcript window + a
  summarization system prompt (narrative summary, key points, action items,
  decisions, open questions), via `executor.chat_with_history` (Interactive).
- Frontend: NotesPanel gains a "Synthesize / Refresh notes" action that renders
  the returned prose above the existing chips, with a timestamp/stale hint;
  categorized chips remain the always-on base layer.

## References

- `docs/reviews/2026-05-29-uiux-deep-dive.md` (W3.6 notes)
- ADR-0008 (conversation ontology — the typed nodes notes build on), ADR-0005
  (OpenRouter LLM), ADR-0006 (streaming chat).
- Reused code: `build_graph_chat_context` (`graph/entities.rs`),
  `executor.chat_with_history` (`llm/executor.rs`), `prepare_chat_request`
  (`commands.rs`).
