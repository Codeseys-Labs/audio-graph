# Research synthesis, 2026-11-21: competitors, lazy speaker identification, System-1

This page states the conclusions and the order of work. The evidence is in four reports:

- [Meeting-assistant competitors](../research/meeting-assistant-competitors-2026-11-21.md)
- [Speaker identification research](../research/speaker-identification-2026-11-21.md) and [lazy speaker identification design](../designs/lazy-speaker-identification.md)
- [System-1 model landscape](../research/system-one-model-landscape-2026-11-21.md)
- [LLM-to-System-1 migration audit](../research/llm-to-system-one-migration-2026-11-21.md)

All four are source research. No benchmark, build or model evaluation was run for them.

## Conclusions

1. **Being bot-free or local is no longer a differentiator.** Meetily Community (MIT, Tauri/Rust), Frigade's Yogurt (MIT, Electron) and Anarlog all transcribe locally. Fireflies, Fathom, Read, tl;dv and Zoom My Notes capture without a bot. AudioGraph has to win on trustworthy, correctable memory instead: speaker names that are right or honestly unknown, corrections that repair transcript, notes and graph together, cited cross-meeting answers, and live assist that is calibrated rather than noisy.
2. **Real names come from cheap evidence before acoustics.** Granola and Yogurt read participant names and active-speaker state from the meeting app's UI. Plaud enrolls only the user's own voice. Otter reuses voices the user has tagged, within a sharing scope. Zoom shows "Speaker N", matches enrolled voices only against the invitee list, and lets the host rename afterwards, with the latest edit syncing everywhere. "Lazy speaker identification" is not a Zoom feature name; it describes that combination of behaviors.
3. **AudioGraph already has the right spine for late binding.** A name binding becomes a relabel: a new `DiarizationSpanRevision` per cluster span, carrying the same span and speaker ids, a new label and an identity stamp. It flows through `SpeakerTimeline` → `SpeakerLabelRemap` → graph, notes (`262a`) and the timeline (`dcd3`). No new event stream, no transcript rewrite, and projection basis hashes stay valid.
4. **Three existing speaker defects block correct naming** (found in the design audit, to be confirmed with regression tests):
   - A single per-turn provider speaker revision merges the whole old speaker entity (`audio-graph-94a1`).
   - Binding a cluster to a name not yet in the graph is a silent no-op in `supersede_entity` (`audio-graph-c321`).
   - Manual speaker renames bypass the speaker timeline and are lost on reload (`audio-graph-5db5`).
5. **System-1 has become a real market with several typed APIs:**
   - TypeSafe Jev 1.14.
   - Cloudflare Clef, Clef-flash and Clef-omni.
   - Cloudflare Clairic and Clairic-flash. "claeric" is Clairic. It does typed extraction, not choice/noul/score.
   - Baseten's own Joey-1, Elephant-1, Mammoth-1, Groundhog-1 and Bison-1 on a System One API.
   - OpenAI `/v1/decisions` with `gpt-6-luna`.
   - Google `gemini-decision-1`.

   Native Laya, D3 and Clef-flash remain the local-first tier. Hosted services are evaluation comparators that need egress authorization, never fallbacks.
6. **Most early System-1 wins reduce downstream spend or raise quality; they do not delete whole LLM calls.** Extraction and projection prompts bundle generation with hidden decisions. The first candidates are the existing live-assist shadows, then claim support, same-referent checks, relation typing and speaker name cues. Text generation, arithmetic, authorization and replay stay out of System-1.

## Order of work

### Wave A: gates and speaker correctness (do first)

- `audio-graph-6345`: G3 independent holdout. `audio-graph-f284`: labeled corpus. Both gate any System-1 authority.
- `audio-graph-c32b`: ADR for identity-as-relabel.
- `audio-graph-94a1`, `audio-graph-c321`: the speaker retcon bugs.
- `audio-graph-c44d`: speaker identity fixtures and metrics harness.

### Wave B: lazy speaker identification phase 1 (smallest shippable slice)

- `audio-graph-50e7`: identity stamp, `apply_batch`, relabel dispatcher.
- `audio-graph-5db5`: naming commands and panel (rename, "this is me", same as, not this person).
- `audio-graph-9ae7`: per-source timelines and the mic-is-me prior.
- Existing prerequisites: `audio-graph-1586` (relabels reach the LLM), `audio-graph-ca06` (local-clustering spans join the timeline), `audio-graph-4796` then `audio-graph-262a` (notes consumer).

### Wave C: automatic suggestions

- `audio-graph-efb2`: roster, name-cue extractor and heuristic binder. Suggest-only by default.
- `audio-graph-636a`: System-1 identity questions Q1-Q4, shadow only.
- `audio-graph-05a5`: live-assist authority after the gates.
- `audio-graph-2f50`: vLLM noul key bug.

### Wave D: evaluation breadth

Evaluation-only transports, each behind the content-egress checklist:

- `audio-graph-a90a`: Baseten Joey-1 and siblings.
- `audio-graph-0ff0`: OpenAI Decisions.
- `audio-graph-d3c9`: Gemini Decision 1.
- `audio-graph-0683`: Laya 0.5 Vega.
- `audio-graph-cdb9`: Clairic extraction.

Projection decisions:

- `audio-graph-39e6`: claim support.
- `audio-graph-04ef`: same-referent.
- `audio-graph-6f83`: notes no-change gate.
- `audio-graph-4055`: graph-context rerank.

### Wave E: acoustic identity and refinement

- `audio-graph-437e`: encrypted self-voiceprint store.
- `audio-graph-faaa`: overlap-exclusive embeddings.
- `audio-graph-4379`: post-session re-diarization.
- `audio-graph-e9bd`: Nemotron-3-Diarization spike (permissive, 8 speakers, streaming). This is a candidate successor to the release-blocked neural diarizer (`audio-graph-8848`).
- `audio-graph-918c`: opt-in accessibility roster reading, per OS.

Competitive tracking stays on `audio-graph-b153` and `audio-graph-8181`; both carry the refreshed findings in their extensions. `audio-graph-9509` and `audio-graph-9c45` are now covered by the Seeds above and can be re-scoped when those land.

## Open decisions

- Whether the identity stamp is an additive member of speaker revisions v1 or needs a schema-version bump (ADR-0043), and where decision records live (ADR-0049). Owner: `audio-graph-c32b`.
- All fusion weights, thresholds and accuracy targets in the design are placeholders until `audio-graph-c44d` measures them.
- Whether Clairic-style typed extraction deserves a contract extension (`audio-graph-cdb9`).
- License and size for TitaNet-small, the embedding model currently in use, were not verified.
