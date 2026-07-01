# Backlog Wave-6 — lane "docs" notes

Worktree: `.claude/worktrees/wf_fb55c841-02d-3`. Base: `a642914` (verified via
`git reset --hard a642914`; `temporal.rs`, `FieldRow.tsx`,
`event_fixtures.rs` all present).

## 0d1c — Supersede ADR-0014 with event-sourced notes+graph projections

Committed: `ffeb027`.

- Added `docs/adr/0024-event-sourced-notes-graph-projections.md` (the actual
  supersession; ADR-0021 had recorded it as "supersession-pending via 0d1c").
- ADR-0014 (`docs/adr/0014-notes-synthesis.md`) now carries a supersession
  banner + status `Superseded by ADR-0024`. Its `synthesize_notes` command is
  documented as a retained manual escape hatch, NOT removed.
- README index: 0014 marked superseded; 0024 row + link reference added.

Every claim in ADR-0024 is grounded in real code (verified by reading):
- Immutable events + replay: `projections.rs` `TranscriptEvent`,
  `TranscriptLedger`/`SpeakerTimeline` (stale/conflict rejection),
  `derive_legacy_transcript_segments`.
- Basis checks: `ProjectionBasis` + `transcript_events_hash` (FNV-1a),
  `validate_basis_with_speaker_timeline` → `ProjectionBasisStaleness`.
- Patch contract: `ProjectionPatch` / `ProjectionOperation` (Note + Graph ops),
  materializer kind guards (`UnsupportedOperation`), `schemars` JsonSchema,
  trusted-metadata stamping in `projection_llm::trusted_projection_patch_from_model_json`.
- Graph retcon: `MaterializedGraph` invalidate/merge/split + `valid_until_ms`,
  prompt's "prefer retcon operations" line, mirroring `graph/temporal.rs`
  `invalidate_edge`/`valid_from`/`valid_until`.
- TTFT scheduling: `projection_scheduler::ProjectionScheduler` coalescing
  reasons + stale-repair decisions + `record_generation_result`.
- Replay semantics: `apply_validated_patch*` (live) vs `apply_replayed_patch`
  (trusts accepted log), `replay_accepted_patches_with_transcript_history`,
  `projection_eval::run_offline_projection_replay`, persistence
  `append/load_projection_patches` + `replay_projection_state`.
- Migration: `commands::synthesize_notes` retained (registered in `lib.rs`);
  artifact migration tracked by open seed `9c89`.

Seed `ad44` confirmed CLOSED (the data model is defined). All ADR links resolve
(checked every relative `.md` target). No `.rs`/`.ts`/generated file touched in
this commit → only the ADR-doc gate (well-formed, links resolve, grounded)
applies, and it passes.

## fee1 — Source-backed provider policy URL + processor matrix

Committed: see fee1 commit below.

Enriched `ProviderPrivacyDescriptor` (in crate
`src-tauri/crates/provider-registry/src/lib.rs`) with two new fields:
- `policy_url_source_date: Option<&'static str>` — paired 1:1 with `policy_url`.
- `subprocessors_url: Option<&'static str>` — official subprocessors list.

Mirror TS type added in `src/types/index.ts`; generated TS regenerated via the
`export-provider-registry` bin (`src/generated/providerRegistry.ts`).

### Honesty rule enforcement

Every non-local provider either carries an OFFICIAL policy URL + verification
date OR stays explicitly `Unknown`. NO fabricated URLs or claims. Sourced
providers (URLs verified against official docs on 2026-06-30):

| Provider id(s) | Source URL | Key finding |
| --- | --- | --- |
| `asr.openai_realtime`, `realtime_agent.openai_realtime` | developers.openai.com/api/docs/guides/your-data | API data NOT used for training by default (since 2023-03-01); 30-day abuse-log retention; ZDR available; deletion supported. retention/training/deletion = ProviderDocsLinked. |
| `asr.deepgram`, `tts.deepgram_aura` | developers.deepgram.com/docs/the-deepgram-model-improvement-partnership-program (+ subprocessors: deepgram.com/privacy/subprocessors) | Deepgram DOES retain a sample of customer audio for model training by default (Model Improvement Program; opt-out via `mip_opt_out=true`). training = ProviderDocsLinked (a sourced TRAINS-BY-DEFAULT fact, not a no-training claim). EU/AU regional endpoints exist. |
| `asr.aws_transcribe`, `llm.aws_bedrock` | docs.aws.amazon.com/organizations/latest/userguide/orgs_manage_policies_ai-opt-out.html | AWS AI services MAY use customer content for service improvement / model training unless you opt out via AWS Organizations. Region residency user-configured. retention/training/deletion = ProviderDocsLinked. |
| `asr.assemblyai` | assemblyai.com/legal/privacy-policy (+ subprocessors via Vanta Trust Center) | Retention + deletion rights sourced. Policy is SILENT on model training → training stays explicitly `Unknown` (deliberately NOT fabricated). |

Everything else (Soniox, all planned/roadmap candidates, OpenRouter,
Cerebras, user-endpoint LLM/ASR, Gemini, etc.) remains `Unknown` with no policy
URL — they kept the `CLOUD_POLICY_UNKNOWN` baseline.

### Tests (never deleted coverage; only added/strengthened)

Rust (`crates/provider-registry/src/lib.rs`):
- Strengthened `provider_privacy_metadata_is_unknown_aware_and_stage_specific`:
  policy_url ↔ source_date must be set together; cloud URLs must be https;
  no-URL providers MUST keep retention/training/deletion = Unknown and no
  subprocessors; local providers carry no remote links.
- New `sourced_provider_policies_are_dated_and_official`: pins each sourced
  provider's URL + date + claim, including the AssemblyAI training-Unknown and
  Deepgram trains-by-default facts, and Soniox staying fully Unknown.

TS (`src/generated/providerRegistry.test.ts`):
- Updated the "declares lifecycle and privacy metadata consistently" loop from
  "all cloud providers are unknown" to the sourced-vs-unknown rule.
- New "exposes sourced data-boundary policy links (or honest unknowns)" test:
  Settings can show dated official links; sourced fields are https; and policy
  URLs never contain secrets (no api_key/secret/token/password substrings).

### Gate results (fee1)

- `bun run check` (biome): PASS.
- `check:provider-registry` (generated TS is current): PASS.
- TS vitest (`providerRegistry.test.ts` + `promotionSchema.test.ts`): 18 PASS.
- Registry crate `cargo test -p audio-graph-provider-registry`: 17/18 pass.
- Registry crate `cargo +1.95.0 clippy -p audio-graph-provider-registry
  --all-targets -- -D warnings`: PASS (0 warnings; this crate holds ALL my Rust
  changes).

### Pre-existing failures (NOT caused by this lane — confirmed on clean base)

1. `cargo +1.95.0 clippy --lib --features cloud -- -D warnings` (full app lib)
   fails to compile the transitive `cookie` crate against `time-0.3.52`
   (E0061). Verified identical failure with `git stash` on base `a642914`
   (EXIT=101). It is an environmental dependency-skew issue (see
   `docs/ops/windows-rust-test-crt-skew.md`), unrelated to the registry change,
   which is why the scoped registry-crate clippy was used as the clean signal.
2. Registry test `future_content_egress_candidates_wait_for_blocked_policy_harnesses`
   fails because `realtime_agent.openai_realtime` is marked `Implemented`
   upstream while the test still expects Planned/Watch. Verified failing on the
   clean base too (`git stash`). Unrelated to privacy metadata. Filed as a
   newSeed.

### Notes for reviewers

- `VENDOR_CLOUD_TTS_PRIVACY` and `USER_REGION_LLM_PRIVACY` became unused when
  Deepgram Aura / AWS Bedrock moved to sourced descriptors. Kept them with
  `#[allow(dead_code)]` + a comment as the `Unknown` template for the next cloud
  TTS / user-region LLM provider (clippy `-D warnings`-clean). The unsourced
  realtime templates (`VENDOR_CLOUD_REALTIME[_NO_HEALTH]_PRIVACY`) had only one
  user (OpenAI realtime, now sourced) and were removed with a comment pointer.
- Two foreign stashes from other worktrees existed in the shared repo; my
  temporary `git stash`/`pop` for base verification restored my work cleanly
  (verified `git diff --name-only` after each pop).
