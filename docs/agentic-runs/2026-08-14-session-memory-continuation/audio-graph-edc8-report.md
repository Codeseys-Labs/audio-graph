# audio-graph-edc8 implementation report

Date: 2026-08-14

## Assignment

- Seed: `audio-graph-edc8` — Replay speaker history when validating persisted projection bases.
- Worktree: `/home/codeseys/DevBox/audio-graph/.worktrees/edc8-speaker-replay-wave3`
- Branch: `work/edc8-speaker-replay-wave3`
- Exact base: `7ae7f484ec7c8f2f420f6082f0c4382cc7a7bdf8`
- Production scope: `src-tauri/src/projections.rs`, `src-tauri/src/persistence/mod.rs`, and `src-tauri/src/commands.rs` only.
- Explicitly unchanged: projection scheduler, live state/apply implementation, speech runtime, projection provider/LLM/writer code, frontend, schemas, generated contracts, and the Seeds queue.

## Outcome

Persisted projection replay now validates each patch against both transcript and speaker revisions visible through that patch's `created_at_ms`. Patches still replay in canonical projection-stream order. A compatibility entry point preserves transcript-only callers, while the speaker-aware entry point distinguishes a missing speaker stream from a present-empty stream.

`FileMemoryRepository::replay_projection_state`, the projection replay report, and `load_session_impl` each load the strict speaker stream once and thread its presence-bearing payload into projection reconstruction. The combined legacy-prefix/framed-suffix transcript, speaker, and projection fixture reconstructs a speaker-bearing patch through `load_session_impl`.

Runtime scheduling and live apply remain transcript-only. This work does not generate or accept speaker-bearing runtime patches.

## Implementation

### Historical replay

- Added `MaterializedProjectionState::replay_accepted_patches_with_history` with optional speaker history.
- Kept `replay_accepted_patches_with_transcript_history` as the narrow transcript-only compatibility seam; it delegates with unavailable speaker history.
- Rebuilds transcript and speaker state through each patch timestamp instead of carrying a forward-only cursor. This prevents a later source revision from leaking into a canonically later patch whose timestamp regresses.
- Applies source events with `received_at_ms <= patch.created_at_ms`; equal-time revisions are visible.
- Continues to apply patches in their supplied canonical order; patch timestamps do not reorder the durable projection stream.
- Added typed `HistoricalProjectionValidationError::SpeakerReplay` diagnostics. They contain only the patch sequence and `SpeakerTimelineError` span/revision metadata, never speaker labels or full events.

### Strict persistence and command threading

- `FileMemoryRepository` overrides `replay_projection_state` to consume one strict transcript snapshot, one strict speaker snapshot, and one strict projection snapshot.
- `projection_replay_report_for_session` reads one strict speaker snapshot and passes it into historical replay.
- `load_session_impl` derives both returned `diarization_events` and projection replay history from the same strict speaker snapshot without re-reading the stream.
- Canonical read errors still propagate immediately. No legacy/cache fallback is attempted after canonical corruption, and replay remains read-only/non-mutating.

## Chosen semantics

| Case | Semantics |
| --- | --- |
| Speaker stream missing | Passed as `None`; a non-empty diarization basis is rejected as `DiarizationBasisUnavailable`. Transcript-only patches remain compatible. |
| Speaker stream present-empty | Passed as `Some(Vec::new())`; the stream is authoritative, so a cited speaker span is rejected as `UnknownDiarizationBasisSpan`, not unavailable. |
| Speaker event and patch have equal time | The event is visible because the replay boundary is inclusive (`<=`). |
| Speaker revisions share a receive time | Speaker history uses a stable `received_at_ms`-only sort, preserving canonical input order for ties even when a later revision moves its start/end boundaries earlier. |
| Later speaker append | Does not retroactively invalidate a patch created before the append. |
| Same-span speaker revision before a patch | A patch citing the older revision is rejected; a subsequent repair patch citing the current revision applies. |
| Out-of-order patch timestamps | Projection patches remain in canonical input order, but each validation ledger is rebuilt through that patch's own timestamp. No later source state leaks backward. |
| Invalid speaker history | Stale/conflicting events produce typed, content-free `SpeakerReplay` validation errors and the affected patch is skipped. |

No broader canonical-writer change or follow-up Seed is required for these bounded semantics. The replay implementation deliberately favors historical correctness over a forward cursor. Its cost is proportional to patches multiplied by source-history length; this is acceptable for offline session replay and can be revisited only if measured session-scale evidence shows a problem.

## TDD evidence

All Rust commands below used the worktree-local target directory, Rust `1.95.0`, `--locked`, and the cloud-only feature set.

The first attempted Cargo invocation used the ambient Rust `1.88.0` and stopped before compiling the test because current AWS/sysinfo dependencies require Rust 1.94/1.95. It was not counted as a red result; all valid cycles used `cargo +1.95.0`.

1. Speaker-bearing historical replay seam:
   - Red: `materialized_projection_history_applies_patch_with_visible_speaker_basis` failed to compile because `replay_accepted_patches_with_history` did not exist (`E0599`).
   - Green: 1 passed, 0 failed after adding the compatibility and speaker-aware replay seam.
2. Per-patch historical causality / out-of-order probe:
   - Red: `materialized_projection_history_uses_each_patch_time_when_timestamps_regress` reported 1 invalid patch where 0 were expected, proving the forward cursor leaked revision 2 into the earlier timestamp.
   - Green: 1 passed, 0 failed after rebuilding source state through each patch time.
   - The append/revision/repair slice then passed: 1 passed, 0 failed; the stale patch alone was rejected and the repair applied.
3. Typed speaker replay failures:
   - Red: `materialized_projection_history_reports_content_free_speaker_replay_failures` failed to compile because `SpeakerReplay` did not exist (`E0599`).
   - Green: 1 passed, 0 failed after adding the content-free typed error and recording speaker replay failures.
4. Repository replay threading:
   - Red: `file_memory_repository_replays_speaker_bearing_projection_basis` returned 1 invalid patch where 0 were expected.
   - Green: 1 passed, 0 failed after the file repository threaded the strict speaker snapshot.
5. Command replay report threading:
   - Red: `projection_replay_report_reconstructs_speaker_bearing_patch` returned 1 invalid patch where 0 were expected.
   - Green: 1 passed, 0 failed after the report consumed its single strict speaker snapshot.
6. `load_session_impl` threading:
   - Red: `load_session_replays_speaker_bearing_projection_state` failed closed with `SessionInvalid` because canonical replay rejected 1 patch.
   - Green: 1 passed, 0 failed after `load_session_impl` reused one speaker snapshot for both its payload and replay.
7. Combined strict mixed-format reload:
   - `load_session_replays_mixed_transcript_speaker_and_projection_streams` passed on its first execution after the command-threading slice: 1 passed, 0 failed. The existing strict readers already decoded mixed streams; this acceptance test proved that the newly threaded histories reconstruct the speaker-bearing framed suffix.

Additional probes passed:

- equal-time speaker boundary: 1 passed;
- missing versus present-empty speaker authority: 1 passed;
- runtime transcript-only guard: 1 passed.

## Review-fix round

Review identified one blocking same-timestamp ordering defect against committed tip `ea37fe40b2f47b2befb5d10986da43e891a0db18`. Two canonical revisions for one speaker span can share `received_at_ms`, while revision 2 corrects its start/end boundaries earlier. The original secondary timeline-field sort then placed revision 2 before revision 1 and made the canonical revision 1 row appear stale.

TDD evidence:

- Red: `materialized_projection_history_preserves_canonical_speaker_order_for_equal_timestamps` failed with `invalid_patch_count` 1 where 0 was expected.
- Correction: speaker history now uses stable `sort_by_key(received_at_ms)` only. Equal timestamps retain canonical input order; the `<= patch.created_at_ms` boundary and canonical patch application order are unchanged.
- Green: the exact regression passed 1/1, and `materialized_projection_history_` passed 11/11, including equal-time-with-patch, append, speaker retcon/repair, conflicting/stale history, and out-of-order patch timestamps.

Review re-gates:

- `speaker_timeline`: 7 passed, 0 failed.
- `projection_replay`: 6 passed, 0 failed.
- `strict_reader_`: 22 passed, 0 failed.
- repository speaker replay, `load_session` speaker replay, and mixed-format reload: 1 passed each.
- locked cloud check: passed, exit 0, 7.84s.
- full direct locked cloud library suite: 1,555 passed, 0 failed, 8 ignored, 38.05s.
- strict cloud Clippy with `-D warnings`: passed, exit 0, 15.64s.

The non-blocking source-text runtime-guard concern remains intentionally unchanged and is tracked separately by `audio-graph-f451`.

## Files changed

- `src-tauri/src/projections.rs`
- `src-tauri/src/persistence/mod.rs`
- `src-tauri/src/commands.rs`
- `docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-edc8-report.md`

No change was needed in `src-tauri/src/persistence/canonical_reader.rs` because its existing strict mixed legacy/framed readers were reusable from the command fixture.

## Verification

### Focused behavior and regressions

- `materialized_projection_history_`: 11 passed, 0 failed after the review correction.
- runtime transcript-only guard: 1 passed, 0 failed.
- file repository speaker-bearing replay: 1 passed, 0 failed.
- projection report speaker-bearing replay: 1 passed, 0 failed.
- `load_session` speaker-bearing replay: 1 passed, 0 failed.
- mixed transcript/speaker/projection reload: 1 passed, 0 failed.
- `speaker_timeline`: 7 passed, 0 failed.
- `projection_replay`: 6 passed, 0 failed.
- `strict_reader_`: 22 passed, 0 failed.

### Required broad gates

- Locked cloud check:
  - Command: `cargo +1.95.0 check --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud --locked`
  - Result: passed, exit 0, 2m 05s.
- Full direct locked cloud library suite:
  - Command: `cargo +1.95.0 test --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud --locked -- --test-threads=1`
  - Result: 1,554 passed, 0 failed, 8 ignored, 38.83s.
- Strict cloud Clippy:
  - Command: `cargo +1.95.0 clippy --manifest-path src-tauri/Cargo.toml --lib --tests --no-default-features --features cloud --locked -- -D warnings`
  - Result: passed, exit 0, 40.65s.
- Rust formatting:
  - Command: `cargo +1.95.0 fmt --manifest-path src-tauri/Cargo.toml --all -- --check`
  - Result: passed, exit 0.
- Docs/Seeds secret hygiene:
  - Command: `bun scripts/check-docs-secret-hygiene.mjs`
  - Result: passed, 0 findings.
- Changed-file secret scan:
  - Command: `betterleaks dir --no-banner --redact src-tauri/src/projections.rs src-tauri/src/persistence/mod.rs src-tauri/src/commands.rs docs/agentic-runs/2026-08-14-session-memory-continuation/audio-graph-edc8-report.md`
  - Result: passed, no leaks found across approximately 1.12 MB.
- Diff hygiene:
  - Command: `git diff --check`
  - Result: passed, no output.

Generated contract checks and `bun run verify:fast` were not required because no command signature, frontend contract, schema, or generated artifact changed.

## Runtime guardrail proof

`runtime_projection_scheduler_and_apply_remain_transcript_only` statically guards both unchanged production seams:

- `projection_scheduler.rs` still obtains jobs from `ledger.current_projection_basis()` and does not consume `SpeakerTimeline::current_basis_spans`.
- `state.rs` still calls `apply_validated_patch(ledger, &patch)` and does not call `apply_validated_patch_with_speaker_timeline`.

The focused guard passed, and the full library suite also passed the existing runtime scheduling/apply tests.

## Risks and open questions

- Historical replay now rebuilds source ledgers for each patch. This is the smallest deterministic solution for regressing patch timestamps and avoids changing canonical writer semantics. No performance issue was observed or measured in this wave.
- Cross-stream causality remains timestamp-based because transcript, speaker, and projection streams do not share one global sequence. The inclusive boundary is now tested explicitly.
- Equal speaker timestamps preserve canonical stream order; timeline boundary fields are not causality tie-breakers.
- No open question or out-of-scope defect blocked `audio-graph-edc8`; no new Seed proposal is necessary.
