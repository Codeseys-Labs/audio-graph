# ADR-0050: Bind Speaker Identity Lazily Through `speaker_revisions` v2 Relabel Batches

- **Status:** accepted (deliberated 2026-11-26 via multi-model review panel: `architect-fable` + `deliberate-sol`)
- **Date:** 2026-11-26
- **Context / Issues:** `audio-graph-c32b` (epic `audio-graph-3588`); implements the persistence and retcon contracts of [`docs/designs/lazy-speaker-identification.md`](../designs/lazy-speaker-identification.md); gates `audio-graph-50e7`, `audio-graph-c321`, `audio-graph-94a1`, `audio-graph-5db5`, and `audio-graph-9ae7`.
- **Related ADRs:** ADR-0017 (unbounded diarization), ADR-0024 (event-sourced projections), ADR-0025 (context efficiency / retcons), ADR-0026 (session timeline), ADR-0027 (file-canonical durable store), ADR-0041 (versioned speech revisions), ADR-0042 (projection basis hash versioning), ADR-0043 (frozen canonical stream registry), ADR-0049 (decision route layer).

---

## Context

AudioGraph needs Zoom-style "lazy" speaker identification: diarization emits stable anonymous clusters immediately (`speaker-c-3`, "Speaker 4", `deepgram-1`), and real person names are bound later (via user rename, `mic = self`, closed roster + cues, System-1 decision routes, or opt-in local voiceprints) and remain revisable at any time.

Two persistence questions had to be settled before coding:

1. **Schema versioning (`ADR-0043`):** Is an atomic multi-span relabel batch + `SpeakerIdentityStamp` an additive field on `speaker_revisions` v1, or does it require `speaker_revisions` domain schema v2? What does that do to the Session compatibility floor (`ADR-0041` / `ADR-0042`)?
2. **Atomic batch wire format & `DecisionRecord` placement (`ADR-0049`):** How is a relabel batch persisted so a crash mid-write never leaves half a cluster renamed, and where do the `DecisionRecord`s justifying an identity binding live?

---

## Decision

### 1. Keep `SpeakerIdentityStamp` additive on `DiarizationSpanRevision`, and version atomic relabel batches as `speaker_revisions` v2

- Add `identity: Option<SpeakerIdentityStamp>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`, redacted `Debug`, content-free) to `DiarizationSpanRevision`.
- Persist authoritative relabel batches on the **existing `speaker_revisions` canonical stream** (`<session>.speaker.jsonl`, no fifth stream) using the existing `AGCL1` canonical envelope with **`domain_schema_version: 2`**.
- **Do not** disguise a relabel batch as an optional field on a v1 singleton record: an older v1 reader would silently apply the single carrier revision and drop the rest of the transaction. `ADR-0043` versions the payload and its replay contract, not merely field presence.
- **Mixed-version reader first:** Land a mixed-version canonical reader before activating v2 writes. The reader preserves historical bare JSONL and framed v1 prefixes under one continuous sequence and hash chain; framed v1 records decode with `identity = None`; once the first v2 frame is committed on a session, all subsequent speaker writes on that session (including ordinary single revisions) are emitted as v2 frames. An older v1 strict reader fails closed with `DomainSchemaVersionMismatch` at the first v2 frame rather than applying a partial prefix.
- **Compatibility floor isolation:** Writing a `speaker_revisions` v2 frame raises only the **speaker-stream reader floor** for that session. It does **not** advance `session_semantics_version`, `SpeakerTimeline::SCHEMA_VERSION`, or the transcript hash version (`ADR-0041` / `ADR-0042`). Golden v1 `ProjectionBasis` hashes and `TranscriptEvent` logs remain byte-for-byte untouched.

### 2. One framed JSONL record per batch; embed `DecisionRecord`s once at batch level

Physical line format: existing `AGCL1 <16-hex-byte-len> <compact JSON WireRecordV1 envelope>\n` with `format_version: 1`, `stream_id: "speaker_revisions"`, `domain_schema_version: 2`, and `event_id: "speaker-relabel:<batch_id>"`.

Two tagged v2 payload variants:

```json
{ "type": "span_revision", "revision": "<DiarizationSpanRevision>" }

{
  "type": "relabel_batch",
  "batch_id": "<uuid>",
  "kind": "bind | rename | unbind | merge | split",
  "scope": "entity | segments",
  "effective_at_ms": 123456,
  "revision_count": 4,
  "revisions": ["<DiarizationSpanRevision>", "..."],
  "decision_records": ["<DecisionRecord>", "..."]
}
```

Invariants:

- **All-or-none validation and replay:** Reject empty batches, duplicate `span_id`s, `revision_count != revisions.len()`, mismatched stamps, or any member whose revision does not equal `current + 1` with matching `supersedes`. Identity relabels preserve `span_id`, original `speaker_id`, timing, stability, and basis anchors. All members share `received_at_ms = effective_at_ms`. Historical projection replay folds each batch atomically at its cutoff (never half a batch).
- **Single serialized admission order:** Ordinary singleton revisions and relabel batches serialize through the same session writer in timeline acceptance order, validating under the timeline lock and persisting before publishing to live state. Identical retry of `(event_id, payload_hash)` is an idempotent no-op; same `event_id` with a different commitment is an error.
- **Torn-tail recovery:** The terminating LF is the frame commit delimiter. An unterminated trailing frame is never replayed (even if its JSON parses); managed recovery quarantines and truncates the torn tail before appending again. A newline-terminated malformed or conflicting record fails the stream closed.
- **Writer/reader size alignment:** Enforce the reader's speaker-artifact ceiling at write admission so a writer never acknowledges a batch reload would reject.
- **`DecisionRecord` placement:** Embed `DecisionRecord`s **once at batch level** (`decision_records`, empty `[]` for manual batches), referenced by `decision_record_id` from stamps. No separate `decision_records.jsonl` sidecar is introduced (avoiding two-file commit hazards and dangling references). Records store only digests and fixed option keys, never raw transcript text or person names.

### 3. Provenance-preserving graph retcon (`audio-graph-94a1` and `audio-graph-c321`)

- **Standalone provider speaker revisions (`94a1`):** Extend `SpeakerLabelRemap` to carry exact turn/segment anchors and old/new cluster identifiers. Route every standalone provider remap through `RelabelScope::Segments`, never whole-entity `supersede_entity`. Missing or ambiguous anchors must not widen a correction to the whole speaker entity.
- **Folded-edge and entity speaker provenance (`c321`):**
  - `TemporalEdge.contributions: Option<Vec<EdgeContribution>>` records per-contribution `(segment_key, origin_cluster, current_cluster, speaker_endpoint_mask, weight, observed_at_ms)` and is preserved across both `add_relation` folds and retcon refolds (`source_segment_id` remains a seek hint).
  - `GraphEntity.speaker_mentions: Option<Vec<SpeakerMention>>` records per-segment/per-cluster mention provenance so `GraphEntity.speakers` is deterministically rebuilt from surviving mentions rather than left stale.
  - `TemporalKnowledgeGraph::reattribute_speaker(from, to, scope, ts)`:
    - `Entity` scope: when `to` is absent, rename `from` in place (preserving node UUID/index and incident edges, adding old name as deduplicated alias, updating `name_index` and all `speaker_mentions`); when `to` exists, merge preserving all contributions and mentions even when 0 live edges exist.
    - `Segments` scope: partition live edge contributions by `(segment_key, old_cluster)`, retarget only speaker-role endpoints on selected contributions, retire the changed aggregate at `ts`, refold moved and residual contributions separately, and rebuild `speakers` on every affected entity.

---

## Consequences

- **Positive:** Atomic relabels with zero new canonical streams, zero transcript rewrites, zero impact on golden v1 projection basis hashes, and no two-file sidecar sync bugs for decision records.
- **Negative / Costs:** Requires a mixed-version `speaker_revisions` reader before v2 writer activation, and requires explicit per-contribution provenance on `TemporalEdge` and `GraphEntity` to make segment-scoped retcons accurate across edge folds.
- **Follow-up gates:** Predecessor canary proving v1 strict readers fail closed on v2 frames; AssemblyAI single-turn final/correction regression fixture (`94a1`); folded-edge segment-split regression fixture (`c321`).
