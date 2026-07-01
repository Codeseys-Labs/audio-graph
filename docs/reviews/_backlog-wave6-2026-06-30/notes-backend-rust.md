# Backlog Wave-6 — lane `backend-rust` notes (2026-06-30)

Base: `a642914` (verified via `git reset --hard` + `git log -1`). Isolated worktree.
Toolchain: `cargo 1.95.0`.

## Environment fix needed before the gate could run

The clean base failed to compile under the mandated gate because of a
pre-existing dependency skew: `time 0.3.52` changed `Parsable::parse`'s
signature, which broke `cookie 0.18.1` (`error[E0061] this method takes 2
arguments but 1 argument was supplied` in `cookie-0.18.1/src/parse.rs:226`).
This is unrelated to either seed and is present on `a642914` itself.

Resolution: pinned `time` down to the last compatible release in the local
lockfile — `cargo update -p time --precise 0.3.47` (0.3.47 is the floor allowed
by `plist 1.9.0`'s `time = "^0.3.47"`; 0.3.41 was rejected). `cookie` then
compiles and the full clippy gate passes.

`src-tauri/Cargo.lock` is **gitignored** (`.gitignore:11`), so this pin is a
local build workaround only and is NOT part of any commit. A fresh checkout that
resolves `time` to 0.3.52 will hit the same skew until the lockfile/dep is
pinned upstream — filed as a newSeed.

## Item 0966 (task) — wire a live producer for the temporal-graph retcon API

Commit: `feat(graph): wire live producer for temporal-graph invalidate_edge retcon (0966)`

Problem: `src-tauri/src/graph/temporal.rs` documented `invalidate_edge` + the
`valid_until` snapshot filter as a "Reserved" API with `#[allow(dead_code)]` and
**no live producer**, so `valid_until` was permanently `None` and a superseded
edge could never be hidden — the retcon engine was inert.

Producer chosen (most defensible per the item's guidance —
"superseded-by-retcon on speaker/entity merge"):
`TemporalKnowledgeGraph::supersede_entity(superseded_name, canonical_name,
timestamp, threshold)`. It:
- resolves both names via `resolve_entity` (which also gains a live caller, so
  its `#[allow(dead_code)]` is removed too);
- invalidates every LIVE edge incident to the superseded node via
  `invalidate_edge` (sets `valid_until`, so `snapshot()` and the delta
  `build_delta_edge` helper hide it) and surfaces each as a `removed_edge_id`;
- re-creates an equivalent LIVE edge between the canonical node and the original
  other endpoint, folding into an existing same-type live edge (weight sum) when
  one already exists rather than duplicating;
- folds the superseded node's mention bookkeeping into the canonical node.
- The superseded node + its invalidated edges are kept (auditable / persisted),
  hidden via `valid_until` — NOT deleted. This makes `valid_until` the
  load-bearing mechanism the item demands.

Real production caller: new `merge_graph_entities` Tauri command
(`src-tauri/src/commands.rs`, registered in `src-tauri/src/lib.rs`) — the
speaker/entity-resolution path that pairs with the speaker-timeline durable
layer + ProjectionBasis diarization work.

Tests (in `temporal.rs`), all passing:
- `supersede_entity_invalidates_old_edge_and_repoints_to_canonical` — proves the
  old edge stays in the graph with `valid_until == 100.0` set AND is excluded
  from `snapshot()` via that path, and the relation reappears re-pointed onto the
  canonical node; delta surfaces removed + added.
- `supersede_entity_is_a_noop_for_self_or_missing` — self-merge / unresolved
  names change nothing and emit no delta.
- `supersede_entity_folds_into_existing_canonical_edge` — duplicate relation
  folds weight into one live edge.

All 7 pre-existing temporal tests still pass (10 total).

## Item 20f2 (task) — provider speaker/channel diarization parser fixtures

Commit: `test(asr): expected_diarization_revisions fixture schema + deepgram
speaker/channel normalization (20f2)`

Extended the ASR event-fixture schema in `src-tauri/src/asr/event_fixtures.rs`:
- A fixture may now carry a `diarization` spec (`timeline_id`, `source_id`,
  `channel`, `channel_capable` capability gate, `speaker_labels` map) plus
  `expected_diarization_revisions`.
- New `normalize_deepgram_diarization` derives provider-neutral
  `DiarizationSpanRevisionPayload` span revisions from the replayed Deepgram
  transcript events (the one event-stream provider that carries word-level
  `speaker: Option<u32>`).

Normalization model (matches the item's constraints):
- Provider speaker id (`deepgram-{n}`) is kept SEPARATE from the display label
  (`speaker_label`, resolved from `speaker_labels`) — neither is conflated with a
  local stable speaker id.
- `channel` is provenance-only: emitted on the revision ONLY when the spec's
  capability gate `channel_capable` is true; suppressed otherwise.
- Contiguous same-speaker word runs become one span each (mixed-speaker spans
  split within a transcript).
- A word with no provider speaker → unknown/interim speaker (`speaker_id`/
  `speaker_label` = None, `Provisional` stability).
- A later transcript re-attributing the same span start emits a retcon revision
  with `supersedes` set + `revision_number = 2`.
- These produce speaker-timeline span revisions, NOT transcript-row mutation.

Fixture: `src-tauri/fixtures/asr/deepgram/diarization_revisions.json` covers all
listed cases (provider ids, labels, channel gate, mixed-speaker, unknown/interim,
retcon). Confidence values are exact-in-f32 (0.5/0.75/0.25/0.875) so the
`expected_events` round-trip is exact.

Test `deepgram_diarization_revision_fixture_normalizes_speaker_and_channel`
passes; the 3 pre-existing event-fixture tests still pass (4 total). A guard
asserts a fixture without a `diarization` spec must not declare
`expected_diarization_revisions`.

## Gate results

- Clippy (MANDATED): `cargo +1.95.0 clippy --lib --tests
  --no-default-features --features cloud -- -D warnings` → EXIT 0 (run after each
  item).
- Tests (`--test-threads=1`, cloud-only feature set to avoid the heavy local-ml
  native build): `graph::temporal::tests` 10/10 pass; `asr::event_fixtures`
  4/4 pass.
- `rustfmt --edition 2024 --check` clean on all touched files
  (`graph/temporal.rs`, `commands.rs`, `lib.rs`, `asr/event_fixtures.rs`).

Note on test feature set: the gate text suggests `cargo +1.95.0 test --features
cloud`, but bare `--features cloud` keeps the default `local-ml` feature on and
triggers a multi-minute whisper/llama native compile that timed out the sandbox.
Tests were validated under `--no-default-features --features cloud` — the same
feature set the mandated clippy gate uses — which is the cloud-only build the
gate's clippy line targets.
