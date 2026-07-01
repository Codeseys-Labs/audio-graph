# Backlog Wave-6 — lane `frontend-ts` notes (2026-06-30)

Base: `a642914` (verified; `temporal.rs`, `event_fixtures.rs`, `FieldRow.tsx`
present). Isolated worktree. JS gate baseline at the start of this lane: 52 test
files / 678 tests passing. After the lane: 54 files / 707 tests passing
(`bun run typecheck`, `bun run check`, `bun run test` all green).

Three seeds resolved, one commit each, gate run after each.

## 8145 (task) — Speaker-timeline JOIN materialized into the rendered transcript

Commit: `feat(transcript): materialize speaker-timeline JOIN onto rendered view (8145)`

The store already buffered the diarization span-revision event stream
(`diarizationSpanRevisions`, fed by `addDiarizationSpanRevision`) but never
materialized it onto the rendered transcript, so UI speaker attribution could
lag or contradict the backend ledger.

Added `src/utils/speakerTimeline.ts` — a pure module mirroring the backend
provider-neutral `SpeakerTimeline` ledger (`src-tauri/src/projections.rs`):

- `materializeSpeakerTimeline(revisions)` — latest-revision-wins by `span_id`,
  stale (lower `revision_number`) dropped, same-rev conflict dropped, sorted by
  `(start_time, end_time, span_id)` using millisecond-rounded keys to match
  the backend `millis()` sort exactly.
- `speakerAttributionIndex(timeline)` — maps each transcript span/segment id
  (`basis_asr_span_ids` ∪ `basis_transcript_segment_ids`) to its winning
  attribution; higher revision wins on ties.
- `joinSpeakerTimelineToTranscript(segments, revisions)` — overrides each
  segment's inline speaker fields with the materialized attribution; segments
  the timeline does not attribute keep object identity (React can bail out of
  re-render); empty revisions ⇒ input returned unchanged.

Wired into `LiveTranscript.tsx`: the rendered `segments` are now the JOIN of
`transcriptSegments` × `diarizationSpanRevisions`.

Tests (`src/utils/speakerTimeline.test.ts`, 12) prove parity with the backend
ledger's own unit tests: provisional→stable collapse
(`speaker_timeline_collapses_provisional_to_stable_supersede`), stale + conflict
rejection (`speaker_timeline_rejects_stale_and_conflicting_revisions`), basis-id
matching, and out-of-order safety (no duplicate UI artifact). The test helper
copies the backend `diarization_payload` shape (`{span_id}-asr` /
`{span_id}-segment` basis ids) so the assertions track the ledger 1:1.

## 9d93 (feature) — Frontend RETCON reducers, remaining acceptance branches

Commit: `test(store): cover remaining RETCON reducer acceptance branches (9d93)`

The event-sourced reducers (`applyAsrRevisionToTranscriptSegments`,
`applyProjectionNotesPatch`, `applyProjectionGraphPatch`, plus the
`addAsrSpanRevision` / `addProjectionPatch` / `addDiarizationSpanRevision`
actions) were already implemented at base with extensive tests: canonical span
map (partial revision replaces the same span), in-place note corrections,
temporal-graph merge/split/invalidate, stale-sequence drop, and
persisted-session restore (`loadSession`). The render layer
(`KnowledgeGraphViewer.materializedGraphToSnapshot`) already hides records with
`valid_until_ms != null`, and that filter is already tested.

Closed the remaining stated-acceptance gaps with three focused store tests in
`src/store/index.test.ts`:

- `invalidate_graph_edge` retcon stamps `valid_until_ms` and leaves zero active
  edges, so the render view hides the edge ("invalidate hides the edge"); both
  endpoints stay active.
- `strengthen_graph_edge` / `weaken_graph_edge` weight deltas clamp into
  `[0, 1]` (no prior coverage for these two operation types).
- Out-of-order ASR span revisions: a stale (lower-revision) event for a span
  neither replaces the canonical text nor appends a duplicate segment, while
  the append-only event ledger retains both revisions.

No reducer code changed — the implementation already satisfied the criteria;
this commit makes the remaining acceptance assertions explicit so a regression
in those branches fails the suite.

## 61db (task) — OpenRouter accelerator catalog VIEW MODEL

Commit: `feat(openrouter): non-secret accelerator catalog view model (61db)`

Settings hardcodes the accelerator routing order as `"cerebras, groq"`
(`LlmProviderSettings.tsx`, the `strict_accelerator` preset default). Built a
non-secret view model so Settings can discover accelerator endpoints
dynamically instead.

Added TS types mirroring the existing Rust catalog structs
(`src-tauri/src/llm/openrouter.rs`): `OpenRouterProvider`, `OpenRouterEndpoint`,
`OpenRouterEndpointPricing`, `OpenRouterPercentileStats`,
`OpenRouterModelEndpoints`, `OpenRouterEndpointArchitecture` in
`src/types/index.ts`.

Added `src/utils/openrouterCatalog.ts`:

- `buildAcceleratorCatalog(endpoints, providers)` — normalizes endpoint rows
  from the SAVED-KEY commands `list_openrouter_model_endpoints_cmd` +
  `list_openrouter_providers_cmd`: provider name/slug, tag, quantization,
  context length, p50 latency, p50 throughput, prompt/completion price (parsed
  from OpenRouter's scientific-notation strings), `isFree`, uptime, supported
  params, and normalized data/privacy fields (policy/ToS/status URLs,
  headquarters, datacenters).
- `rankAccelerators(views, preset)` — `low_latency` (fastest p50 first),
  `high_throughput` (Nitro intent: highest p50 first), `privacy_zdr` (filters to
  endpoints with verifiable privacy/ToS provenance, never inferring ZDR from
  absence). Nulls sort last; input never mutated.
- `acceleratorProviderOrder(views)` — deduped routing slug order, the dynamic
  replacement for the hardcoded default.

Constraints honored:

- PURE module — never calls `invoke`, never reads a plaintext key, never holds
  credentials. The caller passes in the saved-key command results. Satisfies
  "consume ONLY saved-key catalog commands, NO plaintext key readback".
- NO hardcoded provider is the source of truth — the accelerator list is
  whatever the catalog exposes. Slug drift is handled by joining endpoints to
  providers on slug OR name (case-folded), with a name→slug fallback when a
  provider is absent from `/providers`.
- Provenance honesty — policy URLs are surfaced verbatim or `null` (unknown),
  never fabricated. `zeroDataRetention` stays `null` because the public endpoint
  payload exposes no per-endpoint ZDR flag.

Tests (`src/utils/openrouterCatalog.test.ts`, 14) cover dynamic discovery,
slug-drift join-by-name, brand-new-provider slug fallback, missing-metadata /
null / empty endpoint payloads (error/uninit states), unparseable pricing, free
detection, the three ranking presets, no-mutation, and the deduped provider
order.

Follow-up (filed as a newSeed): wire this view model into the Settings
`LlmProviderSettings` UI — fetch the catalog with the saved-key commands, render
the ranked accelerator table, and let the user apply a preset's
`acceleratorProviderOrder` into the OpenRouter routing policy, replacing the
hardcoded `"cerebras, groq"` default. The item explicitly scoped UI wiring as a
separate follow-up if large; the view-model + tests are the bounded core
delivered here.

## Guardrails observed

No `sd` commands, no push, no `.github/**` edits. All commits in the worktree
only. No coverage deleted. No fabricated provider-policy URLs (`fee1`): every
policy field is verbatim-from-catalog or `null`.
