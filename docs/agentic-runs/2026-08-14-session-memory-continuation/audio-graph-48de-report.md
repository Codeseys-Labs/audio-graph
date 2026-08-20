# audio-graph-48de blocked implementation report

Date: 2026-08-14

Seed: `audio-graph-48de`

Parent: `audio-graph-ada2`

Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/48de-asr-observations-wave5`

Branch: `work/48de-asr-observations-wave5`

Exact base: `2dd2a02883df4b4e254913e3fe9eaf4473127dea`

## Outcome

Implementation is design-blocked. No ASR adapter, speech runtime, projection,
persistence, state, generated contract, provider registry, readiness, UI,
workflow, or Seed file was changed.

The accepted v2 normalizer can truthfully admit a final-only observation with
app-estimated timing and unavailable confidence/turn/speaker/channel evidence.
The production ledger cannot accept that admitted revision: its only public
write seam requires the legacy `TranscriptEvent`, whose `start_time`,
`end_time`, and `confidence` are mandatory scalars. Projecting an unavailable
v2 field into those scalars would fabricate evidence. The current Projection
Basis is also explicitly hash-v1-only, so bypassing the legacy event and
silently hashing v2 content as v1 would violate the Wave 4 compatibility
contract and ADR-0031.

The conductor recorded the architectural blocker in `audio-graph-4249` and
made `audio-graph-48de` dependency-blocked. This worktree intentionally does
not edit `.seeds/issues.jsonl`.

## Public-seam RED tracer

The agreed test seam was the production-facing vertical path:

```rust
SpeechSpanRevisionNormalizer::admit(SpanObservation)
    -> TranscriptLedger
    -> ProjectionBasis
    -> ProjectionSchedulers::{notes, graph}
```

A disposable test admitted this truthful final-only observation:

```rust
SpanObservation {
    source_stream_id: "fixture-source".to_string(),
    provider: "final-only-fixture".to_string(),
    provider_item_id: Some("result-1".to_string()),
    correlation: None,
    text: "honest degraded evidence".to_string(),
    stability: SpeechSpanStability::Final,
    timing: SpeechTiming::AppEstimated {
        start_time: 0.0,
        end_time: 1.0,
    },
    confidence: SpeechConfidence::Unavailable {},
    turn: SpeechTurnFidelity::Unavailable {},
    speaker: SpeechSpeakerFidelity::Unavailable {},
    channel: SpeechChannelFidelity::Unavailable {},
    provider_event_ref: Some("fixture.result[0]".to_string()),
    capture_latency_ms: None,
    asr_latency_ms: None,
    received_at_ms: 1_700_000_000_000,
}
```

It then attempted the missing public bridge:

```rust
ledger.apply_speech_span_revision(revision)?;
```

Command:

```text
cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml \
  --lib --no-default-features --features cloud \
  final_only_unavailable_observation_reaches_ledger_basis_and_both_projection_lanes \
  -- --nocapture
```

Exact RED:

```text
error[E0599]: no method named `apply_speech_span_revision` found for struct `projections::TranscriptLedger` in the current scope
   --> src/speech/tests_integration.rs:814:10
    |
813 | /     ledger
814 | |         .apply_speech_span_revision(revision)
    | |         -^^^^^^^^^^^^^^^^^^^^^^^^^^ method not found in `projections::TranscriptLedger`
    | |_________|
    |
    |
   ::: src/projections.rs:701:1
    |
701 |   pub struct TranscriptLedger {
    |   --------------------------- method `apply_speech_span_revision` not found for this struct

For more information about this error, try `rustc --explain E0599`.
error: could not compile `audio-graph` (lib test) due to 1 previous error
```

The disposable tracer was removed with `apply_patch`. The worktree returned to
the exact base before this report was added.

## Exact architectural backflow

### Normalized contract to ledger

- `src-tauri/crates/ipc-contract/src/speech_span_revision.rs` seals raw v2
  construction behind `SpeechSpanRevisionNormalizer::admit` and correctly
  represents unavailable evidence.
- `SpeechSpanRevision` intentionally exposes identity/order/provider/revision
  accessors, not an unchecked raw-parts escape hatch.
- `src-tauri/src/projections.rs` accepts only `TranscriptEvent` in
  `TranscriptLedger::apply_event` and `TranscriptLedger::replay`.
- `TranscriptEvent` requires scalar `f64` timing and `f32` confidence. It has
  no honest representation for unavailable timing/confidence or for evidence
  origin/precision.

Adding getters alone cannot solve this. The information could be read, but it
still could not be represented by the ledger without changing the ledger event
contract or fabricating mandatory scalars.

### Projection Basis and scheduler

- `TranscriptHashVersion` has only `V1`.
- `ProjectionBasis::serialize`, `deserialize`, and `hash_version` are hardcoded
  to v1.
- `ProjectionBasis::from_transcript_events*`, ledger currency classification,
  covered-subset validation, ordering, and append-only/revised classification
  all consume `TranscriptEvent` and `transcript_events_hash_v1`.
- `projection_scheduler.rs` correctly delegates basis creation and currency to
  the ledger, so it cannot independently accept a v2 span without splitting
  the source of truth.
- `projection_llm.rs::basis_events` resolves every basis span from
  `TranscriptLedger.latest_spans` as `TranscriptEvent` and serializes that
  legacy shape into the notes/graph prompt.

Therefore a parallel v2 ledger path also requires an explicit hash v2 and a
unified basis/prompt interpretation. Treating v2 as v1 would break the frozen
hash claim from `audio-graph-4dbb`.

### Canonical persistence, replay, and session consumers

- `TranscriptEventWriteMsg`, `TranscriptEventWriter::append`,
  `write_transcript_event`, and `LocalMemoryRepository::append_transcript_event`
  are typed only for `TranscriptEvent`.
- `canonical_reader::load_speech_span_revisions` can strictly decode the mixed
  compatibility payload, but production `load_transcript_revisions` and every
  repository/session caller still request `TranscriptEvent`.
- `state.rs` constructs, rotates, and snapshots a legacy-only
  `TranscriptLedger`.
- `commands.rs` replay reports, session load, export, transcript views, and
  session timeline all load `TranscriptEvent`, replay the legacy ledger, and
  derive legacy segments.
- `persistence/mod.rs` ledger replay and preferred transcript-segment loading
  do the same.

Writing v2 rows without changing these strict readers would make the next
reload fail. A mixed canonical stream is consequently a migration decision,
not an adapter-local edit.

### Provider adapters

Production direct legacy construction currently exists in:

- `src-tauri/src/speech/mod.rs` for common partial/final paths, local Whisper,
  local diarization, cloud batch, Deepgram, OpenAI Realtime, AWS Transcribe,
  and sherpa streaming;
- `src-tauri/src/asr/assemblyai.rs`;
- `src-tauri/src/asr/gladia.rs`;
- `src-tauri/src/asr/moonshine.rs`;
- `src-tauri/src/asr/revai.rs`;
- `src-tauri/src/asr/soniox.rs`;
- `src-tauri/src/asr/speechmatics.rs`.

Deepgram currently correlates partial/final transcript revisions from a
provider start timestamp, while provider turn lifecycle messages travel as a
separate `TurnEvent`. Moving it to v2 requires an explicit rule that correlates
provider-evidenced turn messages to the app-owned span without reintroducing a
provider branch downstream.

## Decisions required before implementation

The following are hard-to-reverse contracts and should be decided in an ADR or
the decision Seed `audio-graph-4249`, not implicitly inside the adapter child.

### 1. Unified v2 ledger and Projection Basis hash v2

Choose the canonical in-memory revision representation for legacy v1 plus v2,
define `TranscriptHashVersion::V2`, and freeze its exact canonical byte/hash
input. The decision must specify:

- ordering across legacy and v2 events, including timing-unavailable spans;
- whether source order is the primary v2 order and how mixed v1/v2 rows order;
- which content and fidelity fields enter hash v2;
- final/provisional eligibility;
- current/append-only/revised classification for v2 and mixed bases;
- how v1 accepted patches continue to validate with hash v1;
- how notes/graph prompts encode unavailable evidence without dropping or
  fabricating it.

This is the smallest honest semantic route to the Seed acceptance criteria,
but it necessarily changes `projections.rs`, `projection_llm.rs`, scheduler
fixtures, and replay validation.

### 2. Mixed canonical stream and reader/writer migration

Decide whether outer `transcript_revisions` stream schema v1 may carry both
nested v2 and legacy v1 payloads, or whether the outer domain schema/stream id
must advance. The decision must cover:

- writer message/repository trait payload type;
- strict mixed decoding and record-index error reporting;
- replay/hydration into the unified ledger;
- load, review, export, timeline, recovery, deletion, and snapshot behavior;
- old-binary behavior after the first v2 row;
- reader-first rollout and rollback boundary;
- ADR-0027 Accepted durability semantics.

The existing `load_speech_span_revisions` proves strict decoding is possible;
it does not make production replay mixed-version-safe.

### 3. Temporary deferral policy

Until 1 and 2 land, final-only observations with unavailable fields must not be
projected into `TranscriptEvent`. Safe behavior is an explicit typed refusal or
continued legacy-v1 production mode. That preserves honesty but does not meet
`audio-graph-48de`; it must remain blocked rather than being partially closed.

## Proposed Seed decomposition and dependencies

The conductor reports that `audio-graph-4249` now records this blocker and that
`audio-graph-48de` is dependency-blocked. Recommended durable decomposition:

1. `audio-graph-4249` — decide and implement the unified v2 ledger/Basis hash
   contract, or own an ADR child that makes the decision first. It blocks
   `audio-graph-48de` and `audio-graph-ada2`.
2. Persistence/migration child under `audio-graph-9c89` — activate strict mixed
   v1/v2 canonical writing, reading, replay, state hydration, load/export,
   timeline, recovery, rollback, and old-binary evidence. It depends on the
   hash/ledger decision and blocks `audio-graph-48de`.
3. Projection-consumer child under the ADR-0024 projection epic — make notes,
   graph, replay/eval, and scheduler basis lookup consume the unified
   provider-neutral revision representation. It depends on the hash/ledger
   decision and mixed replay contract and blocks `audio-graph-48de`.
4. Resume `audio-graph-48de` — map ASR adapters to `SpanObservation`, retain one
   app span id across Deepgram partial/final/turn observations, activate the
   decided writer, and add final-only/Deepgram/multi-source/error vertical
   fixtures. It depends on all three items above.

`audio-graph-98ef` remains independently scoped to static/effective readiness
capabilities. It must not become the per-span evidence authority; normalized
v2 evidence remains authoritative at runtime. No provider selectability change
is part of any item above.

## ADR constraints

- ADR-0024 requires one immutable transcript source, deterministic replay, and
  exact basis validation for both notes and graph. A shadow v2 scheduler input
  cannot diverge from the canonical ledger.
- ADR-0026 makes timeline/load behavior a fold over the canonical transcript
  log; a v2 activation that only works live is incomplete.
- ADR-0027 requires versioned file-canonical events and Accepted durability;
  enqueue or in-memory ledger admission alone is not durable acceptance.
- ADR-0031 requires hashing the exact covered subset before append-only
  classification. Hash v1 cannot be silently reused for new semantics.
- ADR-0033 forbids this work from promoting a provider or creating a one-off
  provider start exception.

## Cheapest decision-validating probes

Run these vertical probes before broad adapter conversion:

1. Admit one final-only v2 observation with app-estimated timing and unavailable
   confidence/turn/speaker/channel; durably append it; reload it; obtain one
   provider-neutral Basis with explicit hash v2; start both scheduler lanes.
2. Replay the frozen framed v1 fixture and pre-wave accepted patch byte-for-byte
   with hash v1 unchanged.
3. Strictly replay one mixed stream containing v1 then v2, restart state, and
   prove load/export/timeline parity without down-conversion fabrication.
4. Admit a Deepgram interim, final, and correlated provider turn; prove one app
   span id, revisions 1..n, exact supersession, provider timing/turn origin,
   and no downstream provider branch.
5. Interleave two sources and prove independent source ordinals plus stable
   mixed ordering and hash across input reorder.
6. Prove duplicate, conflict, stale/future/wrong-source/wrong-provider,
   provider-item collision, non-finite timing/confidence, and provider reorder
   return typed content-free errors.
7. Run reader/replay/load/export/timeline recovery fixtures before the full
   serialized Rust suite and cross-platform evidence.

## Verification evidence

Focused untouched-baseline gates after removing the RED tracer:

- `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud speech_span_revision -- --nocapture`
  - PASS: 8 passed, 0 failed.
- same command filtered to `deepgram`
  - PASS: 104 passed, 0 failed, 1 ignored.
- same command filtered to `projection_scheduler`
  - PASS: 18 passed, 0 failed.
- same command filtered to
  `transcript_ledger_replays_provider_partial_final_fixtures_without_duplicate_spans`
  - PASS: 1 passed, 0 failed.
- `git diff --check` before adding this report
  - PASS.

A combined locked cloud check/full serialized library test/strict Clippy/fmt
run was started after the focused gates. It was interrupted during dependency
checking when the conductor directed an immediate blocked handoff. No result is
claimed for that interrupted command. Because no production code exists in
this branch, the full implementation gate stack cannot establish the missing
vertical acceptance and is deferred to the dependency work.

## Scope and rollback

The committed footprint is this report only. There is no product behavior to
roll back. Removing the report commit restores the exact base tree.
