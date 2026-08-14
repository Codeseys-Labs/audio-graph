# AudioGraph

AudioGraph is a personal knowledge context for turning recorded conversations into durable session memory and, selectively, longer-lived knowledge about the user's world.

## Language

**Session**:
A bounded recording interval that begins when the user starts recording and ends when that recording stops and is finalized.
_Avoid_: Meeting, application run

### Memory

**Session Memory**:
The recording-scoped account of what was said, what it meant, and what should happen next, including its supporting provenance and temporal context.
_Avoid_: Meeting memory, session summary

**Provisional Session Memory**:
Session Memory for an active recording whose transcript, speaker assignments, notes, graph, and inferences may still be revised; it can guide live assistance but cannot enter the User World.
_Avoid_: Live facts, temporary session

**Finalized Session Memory**:
Session Memory whose recording has ended and whose admitted canonical evidence has reached a durable stabilization boundary; each derived artifact retains an explicit completeness or retry state.
_Avoid_: Finished notes, immutable session

**Projection Basis**:
The exact transcript and speaker revisions from which a notes or graph projection was derived, preserving the boundary between covered evidence and later revisions.
_Avoid_: Prompt context, input batch

**Projection Backlog**:
The durable frontier of accepted revisions not yet covered by a committed projection; it continues to advance while an LLM response is in flight and supplies the basis for follow-up work.
_Avoid_: Dynamic queue, transcript buffer

**Projection Draft**:
Incomplete or unvalidated LLM output tied to a Projection Basis; it may be displayed provisionally but is not part of Session Memory and cannot update canonical notes or graph state.
_Avoid_: Partial patch, streaming note

**Speech Span Revision**:
A provider-neutral version of a recognized speech span with stable identity, source ordering, explicit stability, and honest timing quality; optional provider features enrich it without changing the downstream contract.
_Avoid_: Provider transcript, Deepgram event

**Original Session Audio**:
The captured audio for a Session and the strongest available evidence of what was said.
_Avoid_: Raw audio, temporary audio

**Session Artifact**:
A durable source or derived record belonging to one Session, such as Original Session Audio, transcript revisions, notes, or graph state.
_Avoid_: File, output

**Evidence Annotation**:
A typed provenance link from temporal knowledge to the exact Session Artifact location that supports it.
_Avoid_: Artifact link, untyped citation

**Grounded Inference**:
A claim derived from grounded knowledge through an explicit, reviewable Inference Chain and permanently distinguished from directly asserted knowledge.
_Avoid_: Hidden fact, educated guess

**Inference Chain**:
The grounded premises, Evidence Annotations, and stated reasoning that support a Grounded Inference, including uncertainty and plausible alternatives.
_Avoid_: Model reasoning, confidence score

**High-Impact Inference**:
A Grounded Inference about identity, intent, commitments, sensitive traits, or legal, medical, or financial circumstances that remains a proposal until the user explicitly confirms it.
_Avoid_: Sensitive fact, assumed intent

**User World**:
The user's evolving cross-session knowledge about people, projects, commitments, preferences, relationships, and other durable context.
_Avoid_: Global memory, master graph

**World Promotion**:
An evidence-constrained reconciliation that proposes how Session Memory should change the User World while preserving provenance, contradictions, and temporal progression.
_Avoid_: Merge, memory copy

**Knowledge Gap**:
An explicitly unresolved part of the User World for which the available evidence is missing, insufficient, or conflicting, together with the reason it remains unresolved.
_Avoid_: Missing value, inferred fact

**Unavailable Evidence**:
Evidence known to have supported temporal knowledge whose Session Artifact was later deleted, expired, or became inaccessible, with the loss reason preserved.
_Avoid_: Knowledge Gap, broken link
