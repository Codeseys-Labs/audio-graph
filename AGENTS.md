# AudioGraph Agent Workflow

## GitHub Issues Are The Work Queue

- Use [GitHub Issues](https://github.com/Codeseys-Labs/audio-graph/issues) for all durable task tracking (`gh issue list`, `gh issue view`, `gh issue create`, `gh issue edit`, `gh issue close`, or the T3 issue tools when mounted). Create or update an issue before starting non-trivial code, research, CI, or UX work.
- Prefer focused issues with clear acceptance criteria over broad notes. Use `epic` issues for product work that spans backend, frontend, docs, and CI, and link children/blockers explicitly (`Parent epic: #N`, `Blocked by: #M`, or GitHub sub-issues).
- Apply the relevant type (`bug`, `enhancement`, `task`, `epic`), priority (`priority:p0`..`priority:p4`), and area labels.
- When research changes the direction, update or create GitHub Issues in the same turn so the queue remains the source of truth.
- Every meaningful finding should end in one of three actions: comment/update an existing issue, create a new issue under the nearest epic, or close an issue whose acceptance criteria are actually met (recording the commit SHA / PR and verification evidence).
- If the right epic is unclear, attach the work to the closest existing epic and leave a short `remaining` list on the issue rather than keeping follow-ups only in chat.
- Do not rely on chat-only TODOs, scratch markdown checklists, or hidden agent state for project work.
- **Historical Seeds archive:** Seeds (`@os-eco/seeds-cli` / `sd`) was retired on 2026-11-26. Look up historical `audio-graph-xxxx` IDs in [`docs/history/seeds/README.md`](docs/history/seeds/README.md) (`docs/history/seeds/issues.jsonl`) and map active migrated seeds to GitHub issues via [`docs/history/seeds/SEEDS_TO_ISSUES.md`](docs/history/seeds/SEEDS_TO_ISSUES.md). Do not add new rows to `docs/history/seeds/`.

## Issue Hygiene Loop

- Start broad work by querying open GitHub Issues (`gh issue list --state open --limit 500`, or filtering by `--label priority:p0` / `--label priority:p1` / area label with `--limit 200`) and choosing the highest-priority unblocked item that can be advanced without sweeping unrelated changes.
- During implementation, post partial progress on the relevant GitHub Issue (or link the open PR) instead of pretending the whole epic is complete.
- Close duplicate or narrow issues only when their stated acceptance criteria are actually met and the verification evidence is known; when closing a duplicate, name the canonical issue `#N`.
- If a patch resolves part of an epic, keep the epic open and check off or record remaining work on the epic issue.
- Keep CI/workflow changes approval-gated when this checkout has broad staged or unrelated work.

## Subagent Fan-Out

- Use subagents for independent research or implementation tracks that can run in parallel.
- Keep delegated work scoped by ownership: provider research, CI, settings UX, credentials, source/capture contracts, graph/notes synthesis, or a specific module.
- Subagents should return concrete GitHub Issue proposals or changed file paths, not vague summaries.
- The main agent owns integration, conflict resolution, and final queue hygiene.
- Close completed or superseded subagents promptly so the active worker set stays intentional.

## Deep Work Loop

- Begin large pushes by documenting the current commit/worktree state in `docs/commit-state-*.md`: HEAD, branch, dirty-tree caveats, active issues, known verification, and why any broad changes are not being committed or synced yet.
- Audit the full open backlog with `gh issue list --state open --limit 500` (or paginated `gh api` when open issues exceed 500); treat the result as a dynamic priority queue, not a static plan. If a new dependency or risk appears, create or update a GitHub Issue immediately.
- Research before acting when the task depends on provider APIs, CI behavior, cross-platform packaging, UX conventions, or architecture tradeoffs. Prefer primary sources and record source/date in docs or issue comments.
- Plan work in waves. Run independent research, implementation, and review tracks in parallel only when their file ownership is clear and the main agent can integrate the results without collisions.
- Keep a concurrent review mindset: every implementation wave should have either a subagent review, focused tests, or a main-thread critique pass that looks for regressions, edge cases, and missing issues.
- Iterate in bounded loops: implement the next highest-value unblocked issue, verify it, reconcile review findings, close only the issues whose acceptance criteria are met, then re-read the queue before choosing the next item.
- Do not hide unfinished work in prose. If something is too large, blocked by CI access, or needs a clean branch/worktree, record the blocker and the exact next command or workflow shape on the relevant GitHub Issue.
- Prefer clean branches/worktrees for CI/workflow edits and broad generated-file changes. In a broadly dirty checkout, make only tightly scoped edits and record workflow plans as GitHub Issues unless the user explicitly authorizes the workflow change.

## Architecture Guardrails

- Keep long-lived provider sockets, credentials, `rsac` PCM, graph updates, and source timing in the Rust backend.
- Treat React as configuration, control, and display unless an explicit browser-origin mode is designed.
- Model provider additions through capability contracts and health checks, not one-off UI branches.
- Before making any content-bearing provider or transport selectable, satisfy the Provider Addition Content-Egress Checklist in `docs/designs/provider-architecture.md`; parser fixtures and readiness probes alone do not prove blocked-policy runtime behavior.
- Preserve the product split: durable speech-to-notes/temporal-graph pipeline vs. realtime speech-to-speech agent.
- For cross-platform work, verify Windows, macOS, and Linux paths through CI where practical, including Blacksmith runners for audio-capable jobs.
- Do not ship Windows-only assumptions as the default path. OS-specific behavior must either be behind explicit `cfg` / platform capability checks or have matching macOS and Linux behavior documented and tested.
- Prefer Tauri path APIs, Rust path handling, and backend-owned platform probes over hardcoded shell commands, path strings, or device-id heuristics.

## Credentials And Configuration

- Persist non-secret settings separately from secrets. Secrets belong in the local credential backend: OS keychain by default on desktop, with `credentials.yaml` reserved for legacy import or explicit file fallback.
- UI should show saved credential presence and use saved keys for model discovery and health checks without requiring re-entry.
- Do not write plaintext secrets to `config.yaml`, legacy `settings.json`, logs, screenshots, docs, or GitHub Issues.

## Session Close

- Check open issues / blockers before closing out large work, and link every PR to its issue(s).
- Run relevant tests or explicitly record why they were not run.
- Leave the checkout clean (no unrelated staged changes swept into commits).
