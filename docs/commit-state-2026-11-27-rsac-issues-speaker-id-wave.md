# Commit state, 2026-11-27: `rsac 0.5.0`, GitHub Issues migration, and Lazy Speaker ID Phase 1

- **Base `master` HEAD:** `cf3f096` (after 8 squash-merged PRs from this wave).
- **Final wave PR:** `#490` (`feat/speaker-identity-panel`, closes `#340`), adding `SpeakerIdentityPanel` + `self_display_name` settings UI + this commit-state record.
- **Worktree state:** Clean main checkout at `E:/CS/github/audio-graph` (`master`), clean session worktree (`t3/checkout-remote-rsac-status`), and temporary worktree `wt-speaker-ui` (removed once `#490` merges). No stale branches or AO worktrees remain.

## Landed in this wave (2026-11-26 – 2026-11-27)

| PR | `master` SHA | Scope | Issues |
|---|---|---|---|
| `#113` | `2214e0f` | Competitor, speaker ID, clustering bench, System-1, and screen-snapshots research/designs | Refreshed `#134`, `#244`, `#132`, `#484` |
| `#360` | `6d4cebc` | Upgraded `rsac` `0.4.4 → 0.5.0` (`b50d4cb`, clears `wasapi` RUSTSEC-2026-0332) + `cookie` `0.18.3` (RUSTSEC-2026-0594) | Updated `#137` |
| `#485` | `4f3c902` | Retired `@os-eco/seeds-cli`, archived 930 rows to `docs/history/seeds/`, migrated all 365 active Seeds to GitHub Issues (`#117`–`#484`) with 39 labels and epic checklists | Backlog migration |
| `#387` | `90e08fd` | Normalized documented vLLM `yes`/`no` noul probability keys preserving raw `WireResponse` bytes | Closed `#334` |
| `#430` | `4b130bd` | Replaced greedy assignment in `stabilize.rs` with exact maximum-weight Hungarian one-to-one matching; fixed stale 3D-Speaker comment in `clustering.rs` | Progress on `#346`, `#344` |
| `#487` | `a337a87` | Added ADR-0050 (accepted) & ADR-0051 (proposed) via multi-model deliberation; implemented `TemporalKnowledgeGraph::reattribute_speaker` (rename-in-place, folded-edge `contributions`, `speaker_mentions`, `evict_node`/`load_from_file` alias fixes) + AssemblyAI turn-scoped retcons + `AGENTS.md` `--limit 500` fix | Closed `#336`, `#338`, `#339`; updated `#458` |
| `#488` | `2dac786` | Lazy Speaker ID Phase 1 backend: `src-tauri/src/identity/`, `SpeakerTimeline::apply_batch`, mixed legacy/v1/v2 `speaker_revisions` + atomic single-record batch persistence (`persistence/speaker_log.rs`), source-local timelines & source-qualified graph keys, `mic = self` prior, 5 Tauri naming commands | Closed `#337`, `#341`; progress on `#340` |
| `#489` | `cf3f096` | Opted into `rsac 0.5.0` `hotplug-recovery` for `SystemDefault` on Windows and Linux only (macOS untouched per `rsac` ADR-0021) | Closed `#486` |
| `#490` | *(pending)* | Lazy Speaker ID Phase 1 frontend (`SpeakerIdentityPanel.tsx`, `self_display_name` setting, `en`/`pt` locales, 13 tests) | Closes `#340` |

## Verification summary

- Every merged PR (`#113`, `#360`, `#485`, `#387`, `#430`, `#487`, `#488`, `#489`) passed full cross-platform GitHub Actions CI (`Rust` + `Cloud-only smoke` on Linux, macOS, Windows; `Lints (fmt + clippy)`; `Frontend (TypeScript)`; `cargo audit`; `GitGuardian`).
- Local verification on `#490`: `bun run check`, `bun run typecheck`, `bun scripts/check-docs-secret-hygiene.mjs`, and 11 Vitest test files / 308 tests passed.

## Recommended next wave (open GitHub Issues)
1. **Core P0/P1 spine:** CredentialService v2 (`#127`), durable session memory & projection basis v2 (`#117`), speaker-to-LLM/notes plumbing (`#317` `1586`, `#415` `ca06`, `#255` `4796` → `#257` `262a`).
2. **Diarization calibration & release neural diarizer:** Meeting-domain clustering calibration gate (`#345` `60ea` → `#344` `34cc`), Nemotron-3-Diarization spike (`#455` `e9bd` → `#235` `8848`).
3. **System-1 authority & automatic speaker suggestions:** Independent-holdout gate (`#327` `6345`) + corpus (`#321` `f284`) → live-assist authority (`#328` `05a5`) + speaker fixtures (`#343` `c44d`) + suggest-only roster/cues binder (`#342` `efb2`).
4. **Screen snapshots Phase 0 → 1:** Ratify ADR-0051 (`#458` `40ad`) and resolve the two source-verified storage blockers (`sessions/mod.rs:764` delete ordering, Windows `canonical_durability.rs`), then Phase 1 Mac/Windows shared-pane capture + Tesseract 5 OCR (`#459` `576f`).
