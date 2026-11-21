# Commit state, 2026-11-21: competitor, speaker identification and System-1 research wave

- HEAD: `1516c8d` on `master`. Branch for this wave: `docs/research-2026-11-21`.
- Master CI and Tauri shell E2E were green after each of #111 (`e1699a9`), #109 (`b6b779e`) and #112 (`44fa668`). The Dependabot action bumps #103-#107 were rebased, passed PR CI, and were squash-merged; #103 (github-script 9) and #104 (tauri-action 1.0) change only `release.yml`, which PR CI does not exercise, so they are first proven by the next release run.
- Tree: clean apart from this wave's docs and Seeds rows.

## Cleanup completed before the wave

- Remote: every branch except `master` was deleted. 24 stale branches (`stack-*`, `deep-work-loop-2026-05-31`, `backup/*`, `archive/*`, `codex/canonical-log-salvage-20260819`, `fix/notes-apply-policy-caad`) went in this pass, after the 19 merged or superseded branches deleted earlier in the day; the five Dependabot branches closed with their merged PRs.
- Local: the four AO worktrees under `~/.ao/data/worktrees/audio-graph`, the 18 local `ao/*` branches, and the main checkout's stash were removed. Their content had already landed through #109, #110, #111 and #112.
- `E:/CS/github/rust-crossplat-audio-capture` was removed; it duplicated `E:/CS/github/rsac` at the same commit with no local work.
- Everything deleted is preserved under `E:/CS/github/.worktree-archives/audio-graph-20261121/`:
  - `audio-graph-refs.bundle` (verified with `git bundle verify`) holds all deleted remote and local branches under `refs/archive/{remote,local}/*`, the dropped stash and the AO worktree WIP under `refs/archive/{stash,wip}/*`. Restore one with `git fetch <bundle> refs/archive/remote/<name>:refs/heads/<name>`.
  - `audio-graph-archive-20260819.tar.gz` is the full old clone, including its 56 local branches, 2 stashes, uncommitted files and nested repos.

## Active Seeds

`audio-graph-b153` (competitive roadmap), `audio-graph-8181` (competitive benchmark), diarization epics `audio-graph-3588`, `audio-graph-5011`, `audio-graph-1fbd`, System-1 epic `audio-graph-6dfb` and its open follow-ups.

## Verification

Docs and Seeds only; no code changes in this wave.
