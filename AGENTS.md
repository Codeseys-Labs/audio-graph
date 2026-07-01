# AudioGraph Agent Workflow

## Seeds Are The Work Queue

- Use `sd` for all durable task tracking. Create or update Seeds before starting non-trivial code, research, CI, or UX work.
- Prefer focused issues with clear acceptance criteria over broad notes. Use epics for product work that spans backend, frontend, docs, and CI.
- When research changes the direction, update or create Seeds in the same turn so the queue remains the source of truth.
- Every meaningful finding should end in one of three actions: update an existing Seed extension, create a new Seed under the nearest epic, or close a Seed whose acceptance criteria are actually met.
- If the right epic is unclear, attach the work to the closest existing epic and leave a short `remaining` list rather than keeping the follow-up only in chat.
- Before marking work complete, close resolved Seeds and record follow-up work as new Seeds.
- Do not rely on chat-only TODOs, scratch markdown checklists, or hidden agent state for project work.

## Seed Hygiene Loop

- Start broad work by reading `sd ready --format json` and choosing the highest-priority non-blocked item that can be advanced without sweeping unrelated changes. The JSON envelope is `{ success, command, issues, count }`; parse issue rows from `.issues`, not `.items`. For jq-heavy one-liners, prefer `bun run sd:issues -- ready` / `blocked` / `list --all`, which validates the envelope and emits only the issue array. If `sd ready` is capped or the backlog audit needs the complete open/unblocked queue, use `bun run sd:issues -- ready-all`; it reads `.seeds/issues.jsonl` directly and preserves JSONL order.
- If `sd --format json` output truncates or fails through a pipe, run `bun run check:seeds-json-output`; if it reports a missing Seeds CLI stdout patch, run `bun run prepare:seeds-json-output` and then re-run `sd doctor`. These scripts inspect and patch the repo-pinned `@os-eco/seeds-cli` dependency before falling back to a global install.
- During implementation, attach partial progress to the relevant Seed with `sd update --extensions` instead of pretending the whole epic is complete.
- Close duplicate or narrow Seeds only when their stated acceptance criteria are actually met and the verification evidence is known.
- If a patch resolves part of an epic, keep the epic open and record remaining work explicitly in the extension payload.
- Keep CI/workflow changes approval-gated when this checkout has broad staged or unrelated work.

## Subagent Fan-Out

- Use subagents for independent research or implementation tracks that can run in parallel.
- Keep delegated work scoped by ownership: provider research, CI, settings UX, credentials, source/capture contracts, graph/notes synthesis, or a specific module.
- Subagents should return concrete Seeds proposals or changed file paths, not vague summaries.
- The main agent owns integration, conflict resolution, and final queue hygiene.
- Close completed or superseded subagents promptly so the active worker set stays intentional.

## Deep Work Loop

- Begin large pushes by documenting the current commit/worktree state in `docs/commit-state-*.md`: HEAD, branch, dirty-tree caveats, active Seeds, known verification, and why any broad changes are not being committed or synced yet.
- Audit the backlog with `sd ready --format json` and `sd blocked --format json`; treat the result as a dynamic priority queue, not a static plan. If a new dependency or risk appears, create or update a Seed immediately.
- Research before acting when the task depends on provider APIs, CI behavior, cross-platform packaging, UX conventions, or architecture tradeoffs. Prefer primary sources and record source/date in docs or Seed extensions.
- Plan work in waves. Run independent research, implementation, and review tracks in parallel only when their file ownership is clear and the main agent can integrate the results without collisions.
- Keep a concurrent review mindset: every implementation wave should have either a subagent review, focused tests, or a main-thread critique pass that looks for regressions, edge cases, and missing Seeds.
- Iterate in bounded loops: implement the next highest-value non-blocked Seed, verify it, reconcile review findings, close only the Seeds whose acceptance criteria are met, then re-read the queue before choosing the next item.
- Do not hide unfinished work in prose. If something is too large, blocked by CI access, or needs a clean branch/worktree, record the blocker and the exact next command or workflow shape in the relevant Seed.
- Prefer clean branches/worktrees for CI/workflow edits and broad generated-file changes. In a broadly dirty checkout, make only tightly scoped edits and record workflow plans as Seeds unless the user explicitly authorizes the workflow change.

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
- Do not write plaintext secrets to `config.yaml`, legacy `settings.json`, logs, screenshots, docs, or Seeds.

## Session Close

- Run `sd ready` / `sd blocked` when planning large work.
- Run relevant tests or explicitly record why they were not run.
- Do not run `sd sync` if unrelated staged changes would be swept into a commit; note the reason instead.
