# Subagent Integration Manifest - 2026-06-27

## Purpose

Use this manifest before merging or closing subagent work in a shared, dirty checkout. It gives the orchestrator one place to confirm file ownership, verification, conflict handling, secret hygiene, and whether a clean ref is required before any CI or workflow evidence is treated as current.

## Require A Clean Branch/Worktree When

- the worker touched `.github/workflows/*`, release automation, or other CI dispatch surfaces
- the worker needs remote GitHub or Blacksmith evidence tied to the current changeset
- the worker changed generated artifacts or broad shared surfaces that cannot be cleanly attributed in this checkout
- the worker cannot provide a narrow `owned_paths` to `changed_paths` match
- the worker’s verification depends on a pushed ref rather than local read-only or focused local tests

## Manifest Fields

| Field | Required content |
| --- | --- |
| `worker_id` / `nickname` | Orchestrator worker handle and human nickname used in wave notes |
| `seed_id` | Primary Seed advanced or closed by the worker |
| `owned_paths` | Paths the worker was allowed to edit |
| `changed_paths` | Actual touched paths to merge or review |
| `verification` | Focused commands/tests already run, with pass/fail status only |
| `conflicts` | Shared-surface overlap, stale diff risk, or reviewer objections |
| `secret_handling_note` | Confirm no plaintext keys, provider payloads, or raw secret-shaped outputs were written |
| `ci_workflow_clean_ref_required` | `yes` for workflow/remote-evidence lanes, otherwise `no` with reason |

## Current-Wave Manifest

This table records the first completed 2026-06-27 backlog-zero worker wave and
the active second-wave handoff. The wave started from
[docs/commit-state-2026-06-27-backlog-zero-continuation.md](/mnt/e/cs/github/audio-graph/docs/commit-state-2026-06-27-backlog-zero-continuation.md).

| Wave | worker_id / nickname | seed_id | owned_paths | changed_paths | verification | conflicts | secret_handling_note | ci_workflow_clean_ref_required |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| completed | `019f06c6-22a4-79b3-ae24-de31da526f59` / Godel the 2nd | backlog audit | `docs/reviews/backlog-audit-2026-06-27-dynamic-roadmap.md` | `docs/reviews/backlog-audit-2026-06-27-dynamic-roadmap.md` | `git diff --check` passed for the new doc | read-only against Seeds; no code conflicts | no provider keys, payloads, or raw responses copied | no - documentation inventory |
| completed | `019f06c6-5a42-7932-b96a-3452c058f83a` / Sartre the 2nd | `audio-graph-b841` | `src-tauri/src/asr/transport.rs`, `src-tauri/src/asr/mod.rs`, `src-tauri/src/asr/soniox.rs` | `src-tauri/src/asr/transport.rs`, `src-tauri/src/asr/mod.rs`, `src-tauri/src/asr/soniox.rs` | `rustfmt +1.95.0 --edition 2024 --check`; `cargo +1.95.0 test ... asr::transport`; `cargo +1.95.0 test ... asr::soniox`; `git diff --check` passed | ASR boundary remains shared; next provider migration must be sequenced | no plaintext keys; blocked-policy tests assert diagnostics are redacted | no - local backend slice |
| completed | `019f06c6-8056-73a2-a131-f5067d94aec9` / James the 2nd | `audio-graph-eb6c` scout | read-only | none | read-only analysis only | no write conflict; findings converted to Seed extension | no secrets inspected or printed | no - read-only scout |
| completed | `019f06c6-ac7e-7933-96f7-9a513cb4f930` / Dewey the 2nd | review lane | read-only | none | read-only critique only | findings converted to Seed updates/new Seeds | no secrets inspected or printed | no - read-only review |
| completed | `019f06d5-ee56-7bf2-918d-69efb9d364fe` / Meitner the 2nd | `audio-graph-c0cb` | `scripts/sd-issues.mjs` | `scripts/sd-issues.mjs`; orchestrator documentation update in `AGENTS.md` | `bun scripts/sd-issues.mjs ready-all` returned 60 open-ready rows; `bun scripts/sd-issues.mjs ready` returned 50 capped rows; `bun run check:seeds-json-output` passed; `git diff --check` passed | queue tooling only; no Seeds writes by worker | no secrets used or expected | no - local tooling |
| completed | `019f06d6-2292-7c71-a5eb-211f2e474c8b` / Bacon the 2nd | `audio-graph-5f5e` | `docs/reviews/subagent-integration-manifest-2026-06-27.md` | `docs/reviews/subagent-integration-manifest-2026-06-27.md` | `git diff --check` passed; orchestrator patched current-wave table | documentation only | no provider keys, payloads, or raw responses copied | no - documentation runbook |
| active | `019f06d6-50d3-7ae3-9c67-76afa5567e9f` / Carver the 2nd | `audio-graph-b841` | one ASR provider file, preferably `src-tauri/src/asr/deepgram.rs`; `src-tauri/src/asr/transport.rs` only for tiny helper changes | fill after return | fill after return | must not overlap Soniox slice or Settings/generated files | no live provider tests; no keys | no - local backend slice unless remote evidence is requested |
| completed | `019f06d6-7ffd-70d3-a6b2-aab904cb34a6` / Faraday the 2nd | `audio-graph-0f8e` scout | read-only | none | read-only scout | findings only; implementation must be dispatched separately | no secrets inspected or printed | no - read-only scout |

## Integration / Closeout Checklist

- Confirm `owned_paths` and `changed_paths` still match the worker contract before merging.
- Reject or re-scope any worker diff that spills into shared surfaces without prior ownership.
- Treat workflow, release, and remote-evidence lanes as clean-ref gated until a dedicated branch/worktree exists.
- Record follow-up gaps as Seed updates or new child Seeds; do not hide them in chat-only notes.
- Close the subagent only after verification is copied into Seeds and any conflict disposition is explicit.
