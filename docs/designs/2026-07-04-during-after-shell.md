# During/After Workspace Shell

Status: in progress

Seed: `audio-graph-d633`

## Context

The previous Tauri workspace used a permanent cockpit layout: source controls, graph, notes, transcript, chat, agent proposals, token usage, projection runtime, data route, and pipeline status were all visually available at once. That is powerful for debugging but weak for a first successful note.

The competitor research recorded on `audio-graph-d633` points to a simpler product model:

- During the meeting: capture state, running notes, transcript, and live assistance.
- After the meeting: summary, decisions, actions, transcript review, memory, graph proof, and export.
- Diagnostics: runtime, token, provider, data-route, and backpressure details only when blocking or intentionally opened.

## Decision

Add a top-level workspace shell with three phases:

- `During`: default phase. Shows source controls, live notes, live transcript, and inline live assist when there is activity. It intentionally does not render the graph or projection diagnostics by default.
- `After`: review phase. Used for sample preview and loaded sessions. Shows notes plus transcript review so users see the finished artifact before graph internals.
- `Analysis`: technical/provenance phase. Shows the graph, notes, transcript/chat context, projection runtime, and historical data-route diagnostics.

The first implementation keeps existing panel components and backend contracts. It changes composition and visual priority only.

## Non-Goals

- Do not redesign Settings.
- Do not implement new live-assist card capabilities.
- Do not implement the full cross-session memory workspace.
- Do not move credential or provider health authority into React.
- Do not add new capture modes.

## UX Rules

- A fresh launch should land on `During`.
- Starting capture should return the shell to `During`.
- Sample preview and loaded historical sessions should land on `After`.
- Graph and projection runtime diagnostics should be absent from the default `During` surface.
- `Analysis` remains reachable for graph/provenance/debug work.
- Narrow windows should stack panels without hiding the primary workspace action.

## Follow-Up Candidates

- Promote live assist from conditional inline section to a dedicated During-side rail once `audio-graph-392b` owns card behaviors.
- Replace the After graph split with memory-object navigation once `audio-graph-ceda` designs people/topics/decisions/commitments views.
- Move runtime/data-route details into a named Health or Privacy drawer after screenshot validation proves the basic shell.

## Validation

Completed in this wave:

- `bunx vitest run src/App.test.tsx --pool=vmThreads --maxWorkers=1 --no-file-parallelism --reporter=dot --hookTimeout=120000 --testTimeout=120000`
- `bunx vitest run src/App.test.tsx -t "capture starts" --pool=vmThreads --maxWorkers=1 --no-file-parallelism --reporter=verbose --hookTimeout=120000 --testTimeout=120000`
- `bunx vitest run src/i18n/locale-parity.test.ts --environment=node --pool=vmThreads --maxWorkers=1 --no-file-parallelism --reporter=verbose`
- `bun run typecheck`
- `bunx @biomejs/biome@2.5.1 check src/App.tsx src/App.test.tsx src/styles/layout.css src/i18n/locales/en.json src/i18n/locales/pt.json docs/backlog/commit-state-2026-07-04-during-after-shell-wave.md docs/designs/2026-07-04-during-after-shell.md`

Blocked in this environment:

- Screenshot validation at desktop and narrow widths. `bun run dev --host 127.0.0.1 --port 5173`, a fresh forced server on `5174`, and direct `curl` probes all reached an accepting TCP socket but returned no bytes for `/` or `/@vite/client` within 15-20 seconds.
- `bun run build` reached `vite v6.4.2 building for production... transforming...` and did not complete within several minutes; it was interrupted to avoid leaving a hung process.

Next validation shape:

- Re-run a Tauri or Vite preview on a clean machine/session.
- Capture `During`, `After`, and `Analysis` screenshots at 1280 px, 1024 px, and a narrow side-window width.
- Verify no graph/runtime diagnostics appear in `During` or `After`, and that starting capture returns focus to `During`.
