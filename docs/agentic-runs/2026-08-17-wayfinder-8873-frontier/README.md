# Wayfinder 8873 — frontier decision briefs (2026-08-17)

Agent-prepared decision support for the three unblocked tickets on
[audio-graph-8873](../../../.seeds/issues.jsonl) — `21e9` (provider-neutral LLM
route contract), `70c8` (Session finalization state machine), and `a668`
(per-item evidence and inference admission).

**Nothing here resolves a ticket.** Those three are `wayfinder:grilling`, so the
decision is the maintainer's; these documents exist to make it cheap to make.
No maintainer agreement is recorded in them.

- `decision-packet.md` — the thing to read. Decision order, three cross-cutting
  §0 questions that are not ticket-local, then one section per ticket with
  constraints, options, and a recommendation. Questions carrying a defensible
  default are marked `DEFAULT:`; two are flagged expensive to reverse.
- `reconciliation.md` — the contradictions between the three briefs, named
  rather than averaged. The briefs were prepared in parallel and independently,
  so each made assumptions about the others; this is where those collide.

## Measured afterwards, not in either document

`reconciliation.md` C3 says the Cerebras strict-mode character budget is
measurable rather than arguable. It had never been measured, so it was, via a
throwaway probe over `projection_patch_strict_json_schema` (reverted after):

| kind | serialized chars | ceiling | headroom | variants | headroom/variant |
|---|---|---|---|---|---|
| Notes | 837 | 5000 | 4163 | 3 | 1387 |
| Graph | 2628 | 5000 | 2372 | 10 | 237 |

Because the strict subset forbids external `$ref`, an evidence-annotation shape
must be inlined at every variant, so Graph's real budget is ~237 characters per
variant. Independently, the subset forbids array `minItems`, so "at least one
Evidence Annotation" and "at least one plausible alternative" cannot be
expressed in the strict schema at all — they are validator-only, like the
numeric ranges already documented at `src-tauri/src/projection_llm.rs:281-287`.
That resolves C2 in `a668`'s favour by measurement rather than argument.
