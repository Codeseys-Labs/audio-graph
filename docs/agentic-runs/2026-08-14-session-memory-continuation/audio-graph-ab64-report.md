# audio-graph-ab64 implementation report

Date: 2026-08-14

## Assignment

- Seed: `audio-graph-ab64`
- Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/ab64-hash-v2-kernel-wave7a`
- Branch: `work/ab64-hash-v2-kernel-wave7a`
- Exact base: `72e23b506d6f4d2e465aeebfb452d20fbbc0bfe5`
- Acceptance: freeze a dormant normalized legacy/v2 projection-semantic view,
  first-Accepted positioned input, and fallible SHA-256 hash-v2 encoder; reproduce
  every active design golden in Rust and an independent Bun implementation;
  prove semantic include/exclude behavior, negative-zero canonicalization,
  non-finite refusal, exact supersession, position/source-order failures, and
  unchanged hash-v1 goldens; introduce no production caller.

## What changed

- Added immutable accessors for every projection-semantic Speech Span Revision
  v2 field. Raw construction remains private and deserialization still validates
  the complete wire contract before access.
- Added one normalized `ProjectionSemanticRevision` representation for legacy
  and v2 inputs. It preserves legacy values as `legacy_unspecified` or
  unavailable, leaves legacy source ordinal absent, and has no fields for
  provider item/segment ids, raw event references, latencies, received time, or
  storage metadata. Its fields are private, so callers can only obtain a valid
  record from the legacy/v2 normalization seam and inspect it read-only.
- Added the dormant `projection_basis_hash_v2` module. Its positioned input uses
  first canonical Accepted sequence only to sort; sequence numbers are not
  encoded. The encoder validates unique nonzero positions, strictly increasing
  same-source v2 ordinals, exact v2 supersession, finite evidence, and semantic
  invariants before emitting `sha256:<lowercase hex>`.
- Added typed public decode paths for v2 and compatible legacy/v2 JSON. Callers
  can distinguish unsupported contract, enum, option, and boolean tags from
  malformed values, identity mismatches, and invalid supersession without any
  input content appearing in the error.
- Added a defense-in-depth pre-hash validator that rejects v2 records carrying
  legacy-unspecified evidence, legacy records carrying provider/app evidence,
  empty legacy supersession references, and v2 source/ordinal/span-id identity
  mismatches. The included-field mutation matrix now creates only valid
  revisions through `SpeechSpanRevisionNormalizer`.
- Added a shared golden catalog with the four active ADR-0042 digests and an
  independently implemented Bun normalizer/encoder/verifier.
- Added focused public-seam and conformance tests for v2 access, legacy/v2
  normalization, excluded operational mutations, all goldens, included-field
  mutations, negative zero, non-finite timing/confidence, missing/duplicate
  positions, reversed/duplicate source ordinals, exact supersession, and input
  reorder stability.
- Production search finds the new encoder definition and tests only. No ledger,
  Projection Basis, prompt, scheduler, persistence, writer, state, command,
  provider, frontend/generated contract, workflow, or Seeds caller was changed.

## TDD evidence

1. RED — validated v2 semantic access seam:
   `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml -p audio-graph-ipc-contract validated_revision_exposes_every_projection_semantic_field_read_only -- --nocapture`
   failed with nine `E0599` missing-method errors for `contract_version`, `text`,
   `stability`, `is_final`, `timing`, `confidence`, `turn`, `speaker`, and
   `channel`.
2. GREEN — the same command passed `1 passed; 0 failed` after immutable
   accessors were added.
3. RED — the hash-v2 public seam:
   `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_basis_hash_v2_reproduces_every_design_golden -- --nocapture`
   failed with `E0432` because the positioned semantic input and encoder did not
   exist.
4. GREEN — focused `projection_basis_hash_v2` tests passed `5 passed; 0 failed`
   after implementing the encoder one vertical slice at a time.
5. Correction RED — typed v2 and compatible decoders failed to compile because
   `SpeechSpanRevisionDecodeError`, `decode_json_value`, and the unsupported-tag
   variants did not exist. GREEN added matchable content-free categories for
   unsupported contract/enum/option/boolean tags versus malformed, identity,
   and supersession failures.
6. Correction RED — positioned decoding failed with missing hash error variants
   and no typed JSON constructor. GREEN preserved every typed decode category
   through `PositionedProjectionSemanticRevision::decode_json_value`.
7. Correction RED — cross-payload conformance failed because the normalized
   record allowed crate-internal field mutation and the hash error lacked an
   unsupported-combination variant. GREEN made all fields private, replaced the
   invalid mutation matrix with normalizer-built revisions, and added pre-hash
   rejection for every reviewed impossible combination.
8. Formatting correction RED — the workspace-wide pinned check
   `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check`
   reported five mechanical diffs in the owned IPC Speech Span Revision file.
   GREEN applied those rustfmt changes only; the exact command then exited 0.

## Gates and real results

- IPC contract tests:
  `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml -p audio-graph-ipc-contract -- --nocapture`
  — PASS, `19 passed; 0 failed`; the focused Speech Span subset was
  `6 passed; 0 failed`.
- Application speech normalization tests:
  `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud speech_span_revision -- --nocapture --test-threads=1`
  — PASS, `12 passed; 0 failed`.
- Focused Rust hash-v2 conformance:
  `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_basis_hash_v2 -- --nocapture --test-threads=1`
  — PASS, `6 passed; 0 failed`.
- Independent verifier:
  `bun scripts/verify-projection-basis-hash-v2.mjs`
  — PASS, all four exact design digests reproduced:
  `b53cfde4...c9ffe`, `99ca8069...136d`, `b554723b...ac8a`, and
  `9aef73e4...e12`.
- Frozen projection hash-v1 golden:
  `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud projection_basis_defaults_and_serializes_explicit_transcript_hash_v1 -- --nocapture --test-threads=1`
  — PASS, including `fnv1a64:4eb27818db1f8b3d`.
- Frozen reader/ledger hash-v1 compatibility:
  `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud canonical_reader -- --nocapture --test-threads=1`
  — PASS, `8 passed; 0 failed`, including the frozen
  `fnv1a64:1708ff3ca940aa59` fixture.
- Locked cloud check:
  `cargo +1.95.0 check --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud`
  — PASS, exit 0.
- Full serialized cloud lib suite, rerun once after the correction:
  `cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud -- --test-threads=1`
  — PASS, `1580 passed; 0 failed; 8 ignored`.
- Strict Clippy:
  `cargo +1.95.0 clippy --locked --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud -- -D warnings`
  — PASS, exit 0.
- Rustfmt:
  `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check`
  — PASS after applying all five mechanical diffs in the IPC contract file;
  no semantic or out-of-scope file changed.
- Contract drift:
  `bun run verify:contracts`
  — PASS; audio source, provider registry, session data movement, endpoint
  credential routing, and Speech Span Revision generated contracts are current.
- Docs/Seeds secret hygiene:
  `bun scripts/check-docs-secret-hygiene.mjs`
  — PASS, `0 findings`.
- Betterleaks over the implementation footprint:
  `betterleaks dir --no-banner --redact src-tauri/crates/ipc-contract/src/speech_span_revision.rs src-tauri/src/speech_span_revision.rs src-tauri/src/projection_basis_hash_v2.rs src-tauri/src/lib.rs src-tauri/fixtures/projection_basis_hash_v2/goldens.json scripts/verify-projection-basis-hash-v2.mjs docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-ab64-report.md`
  — PASS, no leaks found.
- Diff hygiene: `git diff --check` — PASS.

## Findings

- No unrelated defect was found inside the assigned surfaces.
- The repository Biome configuration intentionally ignores `scripts/*.mjs` and
  `src-tauri/fixtures/**`; direct `bunx biome check` therefore processed zero
  files. The verifier was executed by Bun and the JSON catalog was parsed by
  both independent encoders instead.

## Open questions

- None for this dormant kernel. Activation, Session floor selection, ledger
  integration, prompt parity, persistence, and predecessor refusal remain with
  the later Seeds in the Wave 7 plan.
