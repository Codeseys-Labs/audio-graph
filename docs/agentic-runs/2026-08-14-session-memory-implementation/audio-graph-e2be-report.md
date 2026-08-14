# `audio-graph-e2be` implementation report

Date: 2026-08-14

Branch: `work/e2be-node26-gate-wave1`

Base: `19ba1a1c1cf9ccbd85a973f8b0222abf1cb8fff8`

Tip: the final Seed-linked commit on `work/e2be-node26-gate-wave1`; the exact
commit is reported in the implementation handoff because this report is part of
that commit.

## Scope and acceptance

Make the exact documented `bun run test:local` command reliable on Node 26
without requiring callers to set `NODE_OPTIONS`, hiding assertion failures, or
adding retries. Preserve the serial local authority (`maxWorkers=1`), preserve
pre-existing `NODE_OPTIONS`, and use a cross-platform command path.

## Changes

- `scripts/run-vitest-local.mjs`: launches the installed Vitest CLI through the
  current Node executable, adds `--no-experimental-webstorage` only to the child
  process, preserves existing `NODE_OPTIONS`, fixes local concurrency at one
  worker, forwards caller arguments, and returns Vitest's exit status.
- `scripts/run-vitest-local.test.mjs`: behaviorally verifies a real Vitest child
  sees Node Web Storage disabled while retaining an existing Node option, and
  verifies a real assertion failure remains nonzero.
- `package.json`: routes `test:local` and `test:focused` through the launcher.
- `docs/CONTRIBUTING.md`: documents the serial, JSDOM-compatible local gate and
  its no-retry/exit-forwarding behavior.
- `vitest.config.ts`: intentionally unchanged so default/CI parallelism remains
  outside this local-only workstream.

## Red evidence

Environment:

```text
node --version
v26.6.0
bun --version
1.3.14
```

Frozen dependencies installed successfully:

```text
bun install --frozen-lockfile
309 packages installed [909.00ms]
```

Before the patch, the exact authority failed:

```text
bun run test:local
Test Files  6 failed | 64 passed (70)
Tests       123 failed | 839 passed (962)
Duration    99.70s
exit code   1
```

Node repeatedly reported `ExperimentalWarning: localStorage is not available
because --localstorage-file was not provided`, and JSDOM suites failed at calls
such as `localStorage.clear()` with `TypeError: Cannot read properties of
undefined`. The first launcher behavior test also failed red with
`MODULE_NOT_FOUND` before the launcher existed.

## Green evidence and gates

### Focused launcher behavior

```text
node --test scripts/run-vitest-local.test.mjs
tests 2
pass 2
fail 0
duration_ms 1198.399191
```

The second fixture intentionally contains a failing Vitest assertion; the Node
test passes only when the launcher returns that nested failure as nonzero.

### Focused package command

```text
bun run test:focused -- src/utils/format.test.ts
Test Files  1 passed (1)
Tests       4 passed (4)
Duration    976ms
```

### Exact full local authority

```text
bun run test:local
Test Files  70 passed (70)
Tests       962 passed (962)
Duration    103.04s
exit code   0
```

### Typecheck

```text
bun run typecheck
$ tsc --noEmit
exit code 0
```

### Biome

```text
bun run check
Checked 171 files in 330ms. No fixes applied.
exit code 0
```

### Production build

```text
bun run build
$ tsc && vite build
2940 modules transformed
built in 4.88s
exit code 0
```

The build emitted Node's `DEP0205` deprecation warning for `module.register()`;
it did not affect the gate and is outside this bounded launcher workstream.

### Diff hygiene

```text
git diff --check
exit code 0 (no output)
```

## Remaining limitations and findings

- The default `bun run test`, coverage, watch, UI, and CI commands are unchanged;
  only the documented serial local/focused authority uses the launcher.
- CI worker bounds remain explicitly out of scope pending runner evidence.
- The local gate remains intentionally serial. No retry or assertion suppression
  was introduced.
- No unrelated implementation finding required a new follow-up in this bounded
  workstream. Seed mutations remain conductor-owned per mission policy.

## Rollback

Revert the final Seed-linked commit reported in the implementation handoff. This
restores the two package commands and contributing text and removes the launcher,
its behavior tests, and this report; no dependency or lockfile change is needed.
