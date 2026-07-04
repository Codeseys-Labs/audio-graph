# Ground: Diarization & End-of-Turn/Sentence state (audio-graph Tauri app)

READ-ONLY map as of branch `fix/gtk-test-harness-65f0`, 2026-07-04. All paths absolute.
Repo: `/mnt/e/CS/github/audio-graph`.

---

## TL;DR

Diarization in this app is a **three-source, revision-based speaker-timeline system**, not a single
provider checkbox:

1. **Local per-utterance worker** (`DiarizationWorker`, always compiled) with 3 backends: Simple
   (signal fingerprint), Sortformer (parakeet-rs, ≤4 spk, feature `diarization`), and Clustering
   (sherpa-onnx unbounded, feature `diarization-clustering`). Live-wired.
2. **Local streaming clustering worker** (`LiveDiarizationWorker`, feature-gated) that runs on a
   rolling window on its own thread and emits `DiarizationSpanRevision` events + `SPEAKER_DETECTED`.
3. **Provider-side diarization** normalized into the SAME provider-neutral
   `DiarizationSpanRevision` event contract — but only AssemblyAI v3 SpeakerRevision and the shared
   final-transcript tail are LIVE-wired; the richer Deepgram/AWS/Soniox/Speechmatics word-level
   normalizers are **written but only exercised by fixtures**, not the live receivers.

Speaker attribution is a **first-class revisioned timeline** (`SpeakerTimeline` ledger) that a
label remap on drives a **knowledge-graph entity retcon** (`supersede_entity`). End-of-turn is a
normalized `TurnEvent`/`end_of_turn` flag that gates when notes/graph projection jobs run. There is
**no sentence-boundary detection** — segmentation is turn/utterance-level, provider-driven.

---

## (1) What diarization exists today, and how are labels represented/attached?

### Backends (all local, provider-side normalized separately)

`src-tauri/src/diarization/mod.rs` — `DiarizationBackend` enum (mod.rs:96-118):
- **`Simple`** (mod.rs:98-100, default) — pure-Rust signal fingerprint: RMS energy, zero-crossing
  rate, mean-absolute-deviation (mod.rs:643-678); nearest-profile match with gap penalty
  (`find_or_create_speaker_simple`, mod.rs:684-730); IDs `speaker-{n}`, labels `Speaker {n}`.
- **`Sortformer`** (mod.rs:101-103, feature `diarization`) — NVIDIA Sortformer v2 ONNX via
  `parakeet-rs`, **max 4 speakers** (`SORTFORMER_MAX_SPEAKERS=4`, mod.rs:59); IDs `speaker-sf-{n}`,
  labels `Speaker A..D` (mod.rs:520-549). Falls back to Simple if model missing.
- **`Clustering`** (mod.rs:109-117, feature `diarization-clustering`) — sherpa-onnx pyannote
  segmentation + TitaNet embedding, **unbounded** (`max_speakers=usize::MAX`, mod.rs:173); IDs
  `speaker-c-{n}`, labels `Speaker {n+1}` (mod.rs:880-888). Clustering & Sortformer are mutually
  exclusive at build time (ORT link conflict, `compile_error!` in lib.rs; mod.rs:106-108).

Backend selection: `make_diarization_config()` at `src-tauri/src/speech/mod.rs:943-987` — prefers
Clustering (if feature + both ONNX models on disk) → Sortformer (if feature + model) → Simple.

### Two distinct live runtimes
- **Per-utterance `DiarizationWorker`** IS spawned in every live speech loop
  (`speech/mod.rs:3561, 3840, 4213`) and `process_input` is called inline with
  `DiarizationInput`s built from ASR segments (`speech/mod.rs:3643, 3923, 4279`). Its
  `DiarizedTranscript` output currently goes to a **dummy channel** (`dummy_diar_tx`,
  speech/mod.rs:3560, 3839, 4212) — the labeled segment is used inline, not consumed downstream.
- **`LiveDiarizationWorker`** (feature `diarization-clustering`) — spawned by
  `maybe_spawn_clustering_diarization` (speech/mod.rs:1069-1165); rolling window + `stabilize.rs`
  cross-window registry; consumer thread `run_clustering_emit_loop` (speech/mod.rs:1179-1278) lifts
  window-local spans to session time, pushes into a bounded `spans` deque (`CLUSTERING_SPAN_HISTORY
  = 512`, speech/mod.rs:1014) and emits both a `DiarizationSpanRevision` (Provisional) and
  `SPEAKER_DETECTED`.

### Label representation & attachment
- `TranscriptSegment.speaker_id` / `.speaker_label` (`Option<String>`) — the segment-level display
  fields (events.rs:244-246; state.rs `SpeakerInfo`).
- Cross-window overlap labeling: `overlap_speaker_for_segment` (mod.rs:843-875) picks the global
  speaker with the greatest **aggregate** time-overlap for a transcript segment; the ASR loop reads
  the `spans` deque to label segments by overlap (speech/mod.rs:1045-1052). `UNKNOWN_SPEAKER`
  (`u32::MAX`, stabilize.rs:70) spans are skipped so a real speaker always wins.
- `SPEAKER_DETECTED` event (`"speaker-detected"`, events.rs:84) carries per-speaker `SpeakerInfo`
  (running total speaking time + segment count); frontend upserts into `store.speakers`
  (store/index.ts:1789 `addOrUpdateSpeaker`).
- **Provider-native** (Deepgram): kept SEPARATE — provider speaker id `deepgram-{n}` in
  `speaker_id`, human label resolved via `speaker_labels` map into `speaker_label`
  (deepgram.rs:1554-1592). Deepgram enables via URL `&diarize={bool}` (deepgram.rs:775-776).

### The provider-neutral event contract (the spine)
`src-tauri/src/events.rs`:
- `DiarizationSpanRevisionPayload` (events.rs:287-325): `span_id`, `provider`, `timeline_id`,
  `source_id?`, `speaker_id?`, `speaker_label?`, `channel?`, times, `confidence?`, `is_final`,
  `stability` (Provisional/Stable/Final — events.rs:275-283), `revision_number`, `supersedes?`,
  `basis_asr_span_ids`, `basis_transcript_segment_ids`, `raw_event_ref?`, latency fields.
- Emitted via `DIARIZATION_SPAN_REVISION` event; frontend `DiarizationSpanRevisionEvent`
  (types/index.ts:172) buffered in `store.diarizationSpanRevisions` (store/index.ts:1373-1381,
  capped 500).
- **Frontend SpeakerTimelineJoiner exists**: `joinSpeakerTimelineToTranscript` in
  `src/utils/speakerTimeline.ts` (used by `LiveTranscript.tsx:62`) replays the revision stream with
  the same `apply_event` semantics (newer rev replaces, stale dropped, conflict dropped) and joins
  spans onto transcript segments by overlap — the mono joiner from seed eebf, on the FE side.

---

## (2) Sentence-boundary or end-of-turn detection today?

**End-of-turn: YES (turn/utterance level). Sentence boundaries: NO.**

- Normalized turn contract: `TurnEventKind` (events.rs:330-338) = `SpeechStarted`, `SpeechFinal`,
  `UtteranceEnd`, `EagerEndOfTurn`, `EndOfTurn`, `TurnResumed`, `LocalWindow`; `TurnEventPayload`
  (events.rs:341-357). Emitted via `emit_turn_event` (speech/mod.rs:684); frontend
  `store.turnEvents` (store/index.ts:1383-1389, capped 100).
- `AsrSpanRevisionPayload` carries `turn_id?` + `end_of_turn: bool` (events.rs:261-262).
- **Provider-driven**: Deepgram Flux/Nova emits `EagerEndOfTurn`/`EndOfTurn`/`UtteranceEnd`
  (deepgram.rs:100-107, 1346-1373) with tunable `eot_threshold`, `eager_eot_threshold`,
  `utterance_end_ms` (deepgram.rs:129-138); mapped to `TurnEventKind` at speech/mod.rs:4722-4728.
  AssemblyAI v3 turns set `end_of_turn` (speech/mod.rs:5056, 5333). Local windows emit
  `TurnEventKind::LocalWindow` (speech/mod.rs:3710, 3970).
- **Function of end_of_turn**: it (with `is_final`/`Final` stability) GATES projection scheduling —
  `observe_projection_schedulers_for_asr_revision` returns early unless
  `is_final || end_of_turn || Final` (speech/mod.rs:1646-1651). So end-of-turn is the trigger for
  running notes/graph LLM projection jobs.
- **No sentence segmentation**: no sentence/clause splitter anywhere; grep for `sentence` returns
  only unrelated hits. Segmentation granularity = provider turn/utterance + local rolling window.

---

## (3) What the open diarization seeds actually scope

- **audio-graph-3588 (epic, P1, `open`, BLOCKED by dbac + c237)** — "Local streaming diarization &
  speaker timeline architecture." The umbrella: define DiarizationSpan/SpeakerTimeline with stable
  IDs, rolling-window revisions, source/channel metadata, confidence, basis ranges, auto/max
  speaker policy, local/provider/hybrid modes, model readiness, persistence, UI health, deterministic
  replay. Two slices PARTIAL: the span-revision event contract + local-clustering provisional emit,
  and the final-transcript speaker-revision tail. Remaining: provider word/token normalization,
  **persist+replay revisions with projections**, local/provider/hybrid merge policy, source/channel
  metadata from a multichannel feed, Settings health controls. Blocks eebf.

- **audio-graph-5011 (feature, P1, `open`)** — "Local streaming diarization worker with flexible
  speaker counts." Scopes the local runtime: rolling windows, overlap-aware scheduling, embedding
  clustering, stable speaker registry, **auto/unbounded + optional max-speaker cap**, confidence/
  stability, ORT feature gating, graceful fallback when models missing. Review reframe: make it a
  first-class processed-audio CONSUMER off the audio bus, emit revisioned events, and **enrich ASR
  spans via a metadata join — never overwrite provider labels or synthesize speaker channels from
  mono**. Runtime library decision (sherpa-onnx unbounded main path + optional parakeet Sortformer
  max-4) recorded but PENDING; b05b tracks cross-platform CI evidence.

- **audio-graph-1fbd (feature, P1, `open`)** — "Normalize provider diarization into speaker-span
  revisions." Fold Deepgram word speakers / AWS labels / AssemblyAI v3 SpeakerRevision / Soniox
  token speakers / Speechmatics labels into stable speaker-span revisions (not one-off transcript
  labels). Extend the contract with `speaker_label_source`, provider speaker id, local stable id,
  confidence, channel/source provenance, supersedes/basis; add per-provider fixtures (split to child
  seed **audio-graph-20f2**). PARTIAL: the final-transcript tail emits final revisions with ASR span
  + segment id as basis. Remaining: parse provider-native WORD/TOKEN spans (not just final labels),
  keep provider ids separate from display labels, emit supersedes for retcons, persist+replay with
  basis checks, add fixtures. Follow-ups noted for Speechmatics word speakers, Gladia live
  utterance.speaker (no live diarization toggle in API), channel-vs-speaker provenance split.

- **audio-graph-dbac (feature, P2, `open`, BLOCKS 3588)** — "Diarization settings UX." Off/provider/
  local/hybrid mode selector, auto/unbounded/fixed speaker count, advanced max-speakers, provider
  caveats, model-readiness badges, persist to config.yaml, session-visible provenance. Two slices
  PARTIAL_VERIFIED: the persisted `DiarizationSettings` block + STT-tab UX, AND runtime enforcement
  wiring `start_transcribe` to apply the global policy before preflight (so stale provider booleans
  can't bypass off/local). Remaining: wire local/hybrid SpeakerTimeline JOIN behavior (blocked on
  the replay schema), surface provenance in transcript views, model-readiness badges.

- **audio-graph-eebf (feature, P2, `open`, BLOCKED by 3588)** — "Speaker timeline → channel-aware
  ASR projection." Reframed by review as **SpeakerTimelineJoiner FIRST, not a channel mux** —
  revise ASR spans by overlap with speaker timelines, carry provenance+confidence; physical channel
  projection is provider-opt-in, gated by declared max channels + source-native channel provenance;
  overflow/overlap speakers stay metadata; separated-speaker PCM lanes are experimental (seed
  audio-graph-dd19). First impl = mono joiner mapping ASR spans by source/time overlap without
  retranscribing or duplicating rows (fixture gaps recorded).

---

## (4) How diarization interacts with the retcon machinery (speaker-span revisions)

The pipeline: **emit revision → SpeakerTimeline ledger → label remap → graph entity supersede →
graph delta/snapshot re-emit**. All live-wired.

1. **Ledger** — `SpeakerTimeline` (`src-tauri/src/projections.rs:373-553`), a revision ledger
   mirroring `TranscriptLedger`. `apply_event` (projections.rs:403-446):
   - newer `revision_number` for a `span_id` REPLACES the prior winner;
   - `Provisional` → `Stable`/`Final` collapse is a replace;
   - lower revision → `StaleDiarizationRevision` (rejected);
   - equal revision but different payload → `ConflictingDiarizationRevision` (rejected).
   Returns `Option<SpeakerLabelRemap>` (projections.rs:350-353) when a span's `speaker_label`
   changes from one non-empty label to a DIFFERENT one (`detect_label_remap`, projections.rs:448-461).

2. **Dispatch → retcon** — `dispatch_diarization_span_revision` (`speech/mod.rs:339-376`): applies
   the event to the timeline; if a remap is returned it calls
   `graph.supersede_entity(superseded_label, canonical_label, ts, 1.0)` on the
   `TemporalKnowledgeGraph`. `supersede_entity` (`graph/temporal.rs:344+`) re-points every incident
   edge from the superseded speaker node onto the canonical node, **invalidates** the old edges via
   `valid_until` (kept for audit/replay, hidden from live snapshot), folds mention bookkeeping, and
   re-points the name index. No-op when names resolve to the same node or the superseded name is
   unknown (temporal.rs:328-344).

3. **Emit + re-project** — `emit_and_dispatch_diarization_span_revision` (`speech/mod.rs:378-427`):
   emits the `DIARIZATION_SPAN_REVISION` event, then dispatches into timeline+graph under lock; on
   `retcon_fired` it takes a graph delta + snapshot, updates the cached snapshot, and emits
   `GRAPH_DELTA` + `GRAPH_UPDATE` so the UI reflects the merged identity. Outcome is
   `DiarizationRevisionOutcome { accepted, retcon_fired, edges_retconned }` (speech/mod.rs:332-337).

4. **Live producers of revisions**:
   - Local clustering emit loop → Provisional session-level revision (`provider="local_clustering"`,
     `timeline_id="session"`, `source_id=None`; speech/mod.rs:1231-1261).
   - Final speaker-labeled transcripts → Final source-local revision via
     `diarization_span_revision_for_transcript` / `emit_diarization_span_revision_for_transcript`
     (speech/mod.rs:273-330) — the shared tail covering Deepgram/AWS/AssemblyAI/Sherpa/local paths.
   - AssemblyAI v3 SpeakerRevision → Final revision with monotonic `revision_number`+`supersedes`
     via `emit_assemblyai_speaker_revision_with_dispatch` (speech/mod.rs:2398-2452), live at
     speech/mod.rs:5046-5050. THIS is the only provider whose native speaker-revision message drives
     a live retcon today (test `assemblyai_speaker_revision_emission_retcons_graph_on_label_remap`,
     speech/mod.rs:6881).

5. **Projection basis-check** — `SpeakerTimeline::validate_diarization_basis`
   (projections.rs:490-543): notes/graph patches that cited speaker spans are validated for coverage
   + staleness against the timeline (`StaleDiarizationSpanRevision` /
   `MissingCurrentDiarizationSpan` / `UnknownDiarizationBasisSpan`). Diarization basis is OPT-IN —
   a projection that cited no speaker spans is not gated. `current_basis_spans` (projections.rs:477)
   exposes `(span_id, revision_number)` pairs as the basis.

6. **Persistence gap (confirmed)** — persistence trait has `append_diarization_span_revision` /
   `load_diarization_span_revisions` / `replay_speaker_timeline`
   (`persistence/mod.rs:516-533, 661-663`), a `DiarizationEvents` artifact kind, and export bundling
   (commands.rs export test). BUT the only `append_diarization_span_revision` caller outside tests
   is absent — the live emit path (`emit_and_dispatch_diarization_span_revision`) does NOT persist to
   the JSONL log; the only real caller found is a test in commands.rs:10312. This matches the
   seeds' universal "persist and replay diarization revisions" remaining item.

### Provider normalizer status (the sharpest gap vs. seed 1fbd)
- `normalize_deepgram_diarization` (deepgram.rs:1595-1692) DOES parse per-word `speaker` indices
  into contiguous same-speaker runs, mixed-speaker spans, provider-id/label separation, and
  supersede-on-reattribution — **but it is only called from `asr/event_fixtures.rs:160,217`**, i.e.
  the fixture/replay harness, NOT the live Deepgram receiver. The live Deepgram path emits only the
  shared final-transcript-level revision. AWS / Soniox / Speechmatics word-level normalizers are not
  implemented as live producers (fixtures/followups only). So seed 1fbd's "parse provider-native
  word/token spans" is genuinely still open for everyone except AssemblyAI v3.

---

## Settings model (dbac)
`src-tauri/src/settings/mod.rs`: `DiarizationMode` = Off/Provider(default)/Local/Hybrid (mod.rs:987);
`DiarizationSpeakerCount` = Auto(default)/Fixed/Unbounded (mod.rs:998); `DiarizationSettings { mode,
speaker_count, max_speakers? }` (mod.rs:1006-1013). `provider_diarization_enabled`
(mod.rs:1019-1025) gates provider flags on Provider|Hybrid; `provider_max_speakers`
(mod.rs:1030-1036) maps Auto→provider default, Unbounded→0, Fixed→cap. `apply_diarization_settings`
(mod.rs:1043+) rewrites per-provider `enable_diarization` at runtime startup so stale config booleans
can't bypass the global policy.

## Key files
- `src-tauri/src/diarization/{mod.rs,worker.rs,stabilize.rs,clustering.rs}` — local backends + live
  rolling-window worker + cross-window stabilization.
- `src-tauri/src/events.rs:275-357` — DiarizationSpanRevision + TurnEvent contracts.
- `src-tauri/src/projections.rs:226-573` — DiarizationSpanRevision persistence type, SpeakerTimeline
  ledger, SpeakerLabelRemap, basis validation.
- `src-tauri/src/speech/mod.rs` — all live emit/dispatch/retcon wiring + turn events.
- `src-tauri/src/graph/temporal.rs:344+` — `supersede_entity` graph retcon.
- `src-tauri/src/asr/deepgram.rs:1595` — Deepgram word-speaker normalizer (fixture-only).
- `src-tauri/src/asr/assemblyai.rs:377` `parse_speaker_revisions` — AssemblyAI v3 (live).
- `src/utils/speakerTimeline.ts` + `src/components/LiveTranscript.tsx` — frontend joiner.
- `src-tauri/src/settings/mod.rs:985-1090` — DiarizationSettings policy.
- `docs/adr/0017-unbounded-speaker-diarization.md`, `docs/research/b16-diarization-live-rust-impl.md`.
