# audio-graph-4dbb implementation report

Date: 2026-08-14

Seed: `audio-graph-4dbb`

Parent: `audio-graph-ada2`

Branch: `work/4dbb-speech-span-contract-wave4`

Base: `f82052d30b8d34e451b078aec2fe371e99d5bebd`

## Outcome

Implemented the Rust-owned Speech Span Revision v2 contract as a deep module.
Its caller and test interface is:

```rust
SpeechSpanRevisionNormalizer::admit(
    SpanObservation,
) -> Result<SpeechSpanRevision, SpeechSpanRevisionError>
```

The implementation behind that interface owns source-local ordinal allocation,
app span identity, provider-item idempotency hints, exact revision
supersession, correlation classification, fidelity validation, and content-free
errors. The normalizer and its private raw constructor live with the Rust wire
contract in `audio-graph-ipc-contract`; ordinary Rust consumers cannot
construct an arbitrary v2 revision and can only deserialize a value that passes
the same wire validation. No ASR adapter, production transcript writer, provider registry,
readiness command, selector, store, UI, or i18n path was activated or changed.

## Interface and invariants

- `contract_version` is exactly `2` on admitted writes and during v2 decode.
- Timing is one tagged value: `unavailable`, `app_estimated`, or `provider`
  with `coarse`/`exact` precision. Timing must be finite, nonnegative, and
  ordered.
- Confidence is `unavailable`, `provider`, or `app`; present values must be
  finite and in `[0, 1]`.
- Turn, speaker, and channel each carry exactly one typed origin:
  `unavailable`, `provider`, or `app`. Every tagged variant and nested value
  rejects unknown fields, including contradictory `unavailable` objects that
  also carry a value.
- A source order is `{source_stream_id, ordinal}`. Ordinals begin at one and
  advance independently per source only after successful admission.
- The app span id is SHA-256-derived from only the stable source stream id and
  first-admission ordinal. Text, timing, confidence, speaker, provider item id,
  and provider event data never participate.
- Revisions are one-based. Revision `n` must supersede the same app span at
  exactly `n - 1`.
- Provider item ids index idempotency hints within a provider/source pair; they
  never become durable span identity.
- Duplicate, conflicting, stale, future, unknown, wrong-source, wrong-provider,
  and provider-item collision outcomes are typed. Error and `Debug` output do
  not contain transcript text, speaker labels, provider payloads, or
  credentials.
- V2 serialized rows contain nested fidelity only. They do not also write
  top-level `start_time`, `end_time`, `speaker_id`, `speaker_label`, `turn_id`,
  or `end_of_turn` scalars.
- `SpeechSpanRevisionParts` and `try_from_parts` are private. A compile-fail
  doctest proves the raw constructor is absent from the consumer API. Public
  JSON decode recomputes the app span id from source order before admitting a
  read-only wire value.

Content-redacted serialized examples:

```json
{
  "contract_version": 2,
  "span_id": "ssp_<source-order-digest>",
  "source_order": {"source_stream_id": "source-stream-a", "ordinal": 1},
  "provider": "fixture-provider",
  "text": "<redacted>",
  "stability": "final",
  "revision_number": 1,
  "timing": {"origin": "unavailable"},
  "confidence": {"origin": "unavailable"},
  "turn": {"origin": "unavailable"},
  "speaker": {"origin": "unavailable"},
  "channel": {"origin": "unavailable"},
  "received_at_ms": 1700000000000
}
```

```json
{
  "timing": {
    "origin": "provider",
    "precision": "exact",
    "start_time": 1.25,
    "end_time": 2.75
  },
  "confidence": {"origin": "provider", "value": 0.875},
  "turn": {
    "origin": "provider",
    "value": {"turn_id": "turn-opaque", "end_of_turn": true}
  },
  "speaker": {
    "origin": "app",
    "value": {"speaker_id": "speaker-opaque", "speaker_label": null}
  },
  "channel": {"origin": "provider", "value": "channel-opaque"}
}
```

## V1 compatibility, framing, and hash proof

- A missing `contract_version` selects the v1 compatibility decoder.
- Present legacy scalar values are retained internally as
  `LegacyUnspecified`; absent confidence/speaker/channel/turn/timing attributes
  become `Unavailable`. The decoder never infers provider or app ownership.
- Projection back to the old `TranscriptEvent` is isolated and fails if a
  mandatory old scalar is unavailable instead of fabricating `0`, `0.9`, or
  `1.0`.
- A reader-first canonical seam decodes a framed v1 payload while preserving
  outer stream id `transcript_revisions`, domain schema version `1`, and the
  exact file bytes. That decoded compatibility event then replays through
  `TranscriptLedger`, produces a defaulted v1 `ProjectionBasis`, validates
  against the ledger, and retains the frozen fixture hash
  `fnv1a64:1708ff3ca940aa59`. The production writer remains the existing v1
  compatibility path for this child.
- `transcript_events_hash_v1` now owns the unchanged FNV-1a byte sequence.
  `transcript_events_hash` remains an exact compatibility alias. The frozen
  golden is `fnv1a64:4eb27818db1f8b3d` for the existing fixture fields.
- `ProjectionBasis` defaults an absent `hash_version` to `v1`; newly serialized
  bases write `"hash_version":"v1"`. Fidelity fields do not enter the v1
  hash.
- A pre-wave accepted ProjectionPatch with no hash version decodes, materializes
  one note, and leaves its source bytes unchanged.

## Generated contract

The dependency-light `audio-graph-ipc-contract` crate is the Rust source of
truth. Its exporter derives the TypeScript declarations deterministically from
the same `schemars` JSON Schema and generates
`src/generated/speechSpanRevision.ts`, including the discriminated unions and
embedded schema. The generation test audits every schema definition, field,
required/optional marker, and tagged variant rather than sampling fixed
substrings. `SpeechSpeakerValue` therefore correctly exposes each nullable
Rust `Option` field as independently optional in TypeScript while intersecting
that base object with the schema's `anyOf` constraint: at least one of
`speaker_id` or `speaker_label` must be a non-null string. A TypeScript compile
guard accepts id-only, label-only, and both forms and rejects `{}` or two null
identifiers.
`src/types/index.ts` only re-exports the generated types; the existing
`AsrSpanRevisionEvent` remains the explicit legacy display/event compatibility
projection until adapter activation.

Package integration:

- `generate:speech-span-contract`
- `check:speech-span-contract`
- `verify:contracts` now includes the speech contract drift gate

Schema-expressible constraints include contract version `const 2`, one-based
ordinal/revision minima, app-span-id shape, non-empty identifiers/content,
nonnegative timing, confidence `[0, 1]`, strict object fields, and at least one
non-empty speaker identifier. Runtime validation necessarily retains the
cross-field/stateful invariants JSON Schema cannot express: ordered timing,
exact source-order-derived identity, exact same-span `n - 1` supersession,
provider-item/correlation history, stale/duplicate/conflict classification,
and whitespace-only rejection.

## TDD evidence

First RED:

```text
error[E0432]: unresolved imports SpanObservation, SpeechAttribute,
SpeechConfidence, SpeechSpanRevisionNormalizer, SpeechSpanStability,
SpeechTiming
error: could not compile audio-graph (lib test) due to 1 previous error
```

Subsequent red tracers failed on missing identity/revision methods and typed
duplicate/conflict/stale errors, missing fidelity validation errors, missing v1
compatibility types, missing hash-version symbols, and the absent framed-reader
function. Each tracer was made green before the next slice.

Review correction RED:

```text
compile-fail API tracer: SpeechSpanRevision::try_from_parts compiled successfully
strict serde tracer: unavailable fidelity with contradictory nested fields decoded successfully
```

The correction made raw construction private, made every nested fidelity shape
strict, moved normalizer ownership behind the IPC seam, added schema constraints
and schema-driven TS emission, and extended the framed compatibility tracer
through ledger replay and basis/hash validation.

Final P2 emitter RED:

```text
src/generated/speechSpanRevision.typecheck.ts: TS2578 Unused '@ts-expect-error'
```

The unused directive proved that generated `SpeechSpeakerValue` incorrectly
accepted `{}`. The schema emitter now composes object properties with `anyOf`
or `oneOf` branches instead of discarding those branches, and its audit visits
both the base properties and composed requirements. The same typecheck seam is
green for id-only, label-only, and both valid cases while `{}` remains a
compile-time error.

Final focused evidence:

```text
speech_span_revision: 8 passed; 0 failed; 1566 filtered out
projection hash/version golden: 1 passed; 0 failed; 1573 filtered out
canonical_reader: 8 passed; 0 failed; 1566 filtered out
ipc-contract: 17 passed; 0 failed; compile-fail doctest 1 passed
```

## Files

- `src-tauri/src/speech_span_revision.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/projections.rs`
- `src-tauri/src/persistence/canonical_reader.rs`
- `src-tauri/Cargo.lock`
- `src-tauri/crates/ipc-contract/Cargo.toml`
- `src-tauri/crates/ipc-contract/src/speech_span_revision.rs`
- `src-tauri/crates/ipc-contract/src/lib.rs`
- `src-tauri/crates/ipc-contract/src/bin/export_speech_span_revision.rs`
- `scripts/generate-speech-span-contract.mjs`
- `src/generated/speechSpanRevision.ts`
- `src/generated/speechSpanRevision.typecheck.ts`
- `src/types/index.ts`
- `package.json`
- this report

## Gates and results

- `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud speech_span_revision -- --nocapture`
  - PASS: 8 passed, 0 failed.
- focused ProjectionBasis/hash v1 golden
  - PASS: 1 passed, 0 failed.
- focused canonical reader suite
  - PASS: 8 passed, 0 failed.
- `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml -p audio-graph-ipc-contract -- --nocapture`
  - PASS after review correction: 17 passed, 0 failed; exporter bins passed;
    compile-fail API doctest passed 1/1.
- `cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
  - PASS.
- `cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings`
  - PASS.
- `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check`
  - PASS.
- `bun run check:speech-span-contract`
  - PASS: generated contract current.
- `bun run verify:contracts`
  - PASS: all generated contract gates current.
- `bun run typecheck`
  - PASS, including the generated speaker-value compile guard.
- `bun run check` / Biome
  - PASS after the final correction: 173 files checked, no fixes.
- `bun scripts/check-docs-secret-hygiene.mjs`
  - PASS: 0 findings.
- `betterleaks dir --no-banner --redact <touched files and report>`
  - PASS after review correction: approximately 803 KB scanned, no leaks
    found.
- `git diff --check`
  - PASS.
- Full direct locked cloud library suite, run once:
  - Initial implementation run: `1574` total, 1565 passed, 1 failed, 8
    ignored. The unrelated parallel/global-state shutdown failure passed its
    exact focused rerun.
  - Review correction run: `1574` total, 1565 passed, 1 failed, 8 ignored.
    The sole failure was the unrelated parallel/global-state test
    `sessions::tests::diarization_log_is_a_default_session_artifact_path`.
    Its exact focused rerun immediately passed 1/1. No session cleanup/default
    artifact file was changed, so this report records the finding without an
    out-of-scope fix.
- `bun run verify:fast`
  - Biome, typecheck, and all contract gates passed.
  - The command then stopped because the machine-global Seeds CLI at
    `/home/codeseys/.bun/install/global/node_modules/@os-eco/seeds-cli/src/output.ts`
    lacks the pipe-safe stdout retry patch. The repo-pinned CLI patch was
    present and all three JSON commands parsed. The global install is outside
    this worktree and was not mutated.

## Rollback and old-binary limitation

Rollback is reader-first and safe while the existing production writer remains
v1. Removing this commit removes v2 admission/generation without rewriting any
session artifact. Once a later child appends v2 rows, an old binary cannot
decode those rows. No fabricated down-conversion is provided; rollback after
activation requires preserving the newer reader or restoring from a pre-v2
artifact boundary.

## Handoff

- `audio-graph-48de`: replace direct adapter payload construction with
  `SpanObservation` admission, activate v2 persistence deliberately, and keep
  the legacy frontend event as an explicit compatibility projection only.
- `audio-graph-98ef`: publish static/effective fidelity capability contracts
  from the generated types without changing provider selectability.
- `audio-graph-fcca`: consume effective degradation from generated readiness
  data after 98ef; do not infer fidelity from provider ids.

No Seed file was edited in this worktree. The conductor owns Seed closure after
integration evidence.
