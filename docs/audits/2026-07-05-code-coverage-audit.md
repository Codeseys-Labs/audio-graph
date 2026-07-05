# Code Coverage Audit + Improvement Plan

**Date:** 2026-07-05
**Scope:** Frontend (React/TS, `src/`) measured; Rust backend (`src-tauri/src/`) statically inventoried.
**Baseline commit:** `origin/master` @ `67c2701`.

---

## 1. Executive summary

- **Frontend line coverage is healthy at ~84.9%** (functions ~85.6%, statements ~83.7%, branches ~78.7%), comfortably above the thresholds currently declared in `vitest.config.ts` (lines/statements 60, functions 55, branches 50). The declared thresholds are stale — they sit ~20 points below reality and gate nothing.
- **Rust has substantial test volume — 1,286 `#[test]`/`#[tokio::test]` functions across 74 of 88 files — but no measured line/branch coverage exists.** Test *density* is highly uneven: the largest and most bug-prone modules (`commands.rs`, `speech/mod.rs`, `state.rs`, the projection pipeline) carry the thinnest tests-per-LOC.
- **Highest-value gaps cluster where past bugs already clustered:** credential/config persistence and provider dispatch. `credentials/mod.rs` has 14 commits (mostly `fix:`), `commands.rs` 69, `settings/mod.rs` 26, `store/index.ts` 40 — and the frontend provider-settings components + central store are exactly the least-covered logic files.
- **Recommendation: adopt a coverage *ratchet* (never-decrease gate), not a fixed bar.** Set the frontend floor just under today's numbers and forbid regressions; stand up `cargo-llvm-cov` as a **non-gating** nightly report first, then ratchet once a real baseline exists.

---

## 2. Frontend coverage (measured)

### 2.1 How it was measured

- Tool: `@vitest/coverage-v8` (already a permanent `devDependency` — no dependency change was needed for this audit).
- Node v22.17.0 (Node 26 breaks jsdom in this repo), `bun install`, vitest 4.1.7.
- **Environment caveat (WSL / `/mnt/e`):** under the default parallel fork pool, 26 of 62 test suites failed to *spawn their worker* (`[vitest-pool]: Timeout waiting for worker to respond`) — a disk-I/O thundering-herd on worker startup, **not** test failures. Those suites' files then reported 0% and understated the total.
- To get an honest number the suite was run twice and merged per-file (max covered lines): a default-pool run (36 files / 660 tests green) and a **sequential** run (`fileParallelism: false`, 59 files / 761 tests green, zero worker-spawn timeouts). The union covers all 62 suites. Between the two runs, **every** suite passed in at least one — there are **no genuinely failing tests**, only environment-induced worker-spawn flake.

### 2.2 Overall (union of both runs — all 62 suites represented)

| Metric | Covered / Total | % | vitest.config threshold | Margin |
|---|---|---|---|---|
| Lines | 4665 / 5496 | **84.9%** | 60 | +24.9 |
| Statements | — | **83.7%** | 60 | +23.7 |
| Functions | 1425 / 1664 | **85.6%** | 55 | +30.6 |
| Branches | — | **78.7%** | 50 | +28.7 |

Single-command reproduction of the point-in-time number (accept the parallel-run understatement, or use the sequential config below):

```bash
export PATH="$HOME/.nvm/versions/node/v22.17.0/bin:$PATH"
bun install
bunx vitest run --coverage --pool=forks --hookTimeout=120000
```

Sequential (all suites, slow ~35 min on WSL but zero spawn-flake) — add to a throwaway config with `pool: "forks"`, `fileParallelism: false`.

### 2.3 Per-directory breakdown (union)

| Directory | Line % | Covered/Total | Notes |
|---|---|---|---|
| `constants/` | 100.0% | 1/1 | trivial |
| `i18n/` | 100.0% | 2/2 | trivial |
| `types/` | 100.0% | 1/1 | trivial (type-only) |
| `utils/` | 96.5% | 220/228 | strong |
| `hooks/` | 89.1% | 303/340 | strong |
| `components/settings/` | 86.5% | 1194/1381 | good |
| `components/` | 86.1% | 2265/2630 | good |
| `(root: App.tsx, theme, modelConstants…)` | 81.6% | 155/190 | App.tsx is the soft spot |
| `store/` | 73.9% | 522/706 | **lowest logic-heavy dir** |
| `generated/` | 16.7% | 1/6 | codegen'd; low test value |
| `analytics/` | 9.1% | 1/11 | `ErrorBoundary.tsx` + `safeInvoke.ts` genuinely untested |

### 2.4 The 15 least-covered non-trivial source files (union; `lines_total ≥ 20`, generated/config excluded)

| # | File | Line % | Lines (n) | Func % | Load-bearing? |
|---|---|---|---|---|---|
| 1 | `analytics/safeInvoke.ts` | 0.0% | 4 | 0% | **YES** — the Tauri `invoke` wrapper every IPC call flows through |
| 2 | `analytics/ErrorBoundary.tsx` | 0.0% | 6 | 0% | Yes — top-level React error boundary |
| 3 | `components/AsrProviderSettings.tsx` | 56.4% | 55 | 53.2% | **YES** — ASR provider dispatch/config UI |
| 4 | `components/settings/downloadProgress.ts` | 59.1% | 22 | 100% | Moderate — model-download progress logic |
| 5 | `components/LlmProviderSettings.tsx` | 61.6% | 86 | 57.1% | **YES** — LLM provider dispatch/config UI |
| 6 | `components/LoggingSettings.tsx` | 65.7% | 70 | 57.9% | Moderate — logging config |
| 7 | `store/index.ts` | 73.9% | 706 | 76.8% | **YES** — central Zustand store (40 commits of churn) |
| 8 | `components/KnowledgeGraphViewer.tsx` | 75.4% | 211 | 83.7% | Presentational-heavy (canvas force-graph) |
| 9 | `utils/errorToMessage.ts` | 76.2% | 21 | 100% | Moderate — error-string mapping |
| 10 | `components/DemoModeBanner.tsx` | 77.3% | 22 | 90% | Presentational |
| 11 | `components/LiveTranscript.tsx` | 79.5% | 78 | 91.7% | Moderate — live transcript rendering logic |
| 12 | `App.tsx` | 80.3% | 173 | 75.0% | **YES** — application root/wiring (27 commits) |
| 13 | `hooks/useTauriEvents.ts` | 82.4% | 176 | 88.3% | **YES** — speech/pipeline IPC event glue |
| 14 | `components/ExpressSetup.tsx` | 84.5% | 239 | 84.6% | **YES** — first-run onboarding + credential entry |
| 15 | `components/settings/useSettingsController.tsx` | 85.5% | 1152 | 84.2% | **YES** — largest logic file; the settings/provider/credential controller |

**Load-bearing vs presentational read:** the genuine risk sits in items 1, 3, 5, 7, 12, 13, 14, 15 — the IPC glue (`safeInvoke`, `useTauriEvents`), the provider-dispatch settings UIs, the central store, App root, and the monster settings controller. `KnowledgeGraphViewer` and `DemoModeBanner` are largely presentational and are lower priority despite the raw percentage.

---

## 3. Rust backend — static test-density inventory (NOT measured)

> Per mission constraint (both cargo slots taken), `cargo llvm-cov`/`tarpaulin` were **not** run. Numbers below are static counts: file LOC vs in-file `#[test]`/`#[tokio::test]` functions. Test *presence* is not test *coverage* — treat this as a targeting heat-map, not a coverage figure.

**Totals:** 88 `.rs` files · 116,697 LOC · 1,286 `#[test]`/`#[tokio::test]` · 74 files carry a `#[cfg(test)]` module.

### 3.1 Largest / most load-bearing modules by test density (LOC per in-file test — higher = thinner)

| Module | LOC | in-file tests | LOC/test | Role |
|---|---|---|---|---|
| `speech/mod.rs` | 8,641 | 27 | **320** | Speech orchestration hub, 189 fns (+45 tests across `speech/` incl. `tests_integration.rs`, `tests_audio_accumulator.rs`) |
| `projection_eval.rs` | 1,578 | 7 | **225** | Projection scoring/eval logic — **worst ratio of any core module** |
| `state.rs` | 2,029 | 12 | **169** | Central app state, 35 fns |
| `analytics/mod.rs` | 1,447 | 10 | 144 | Analytics/Sentry config (bug history: analytics_enabled persistence) |
| `projection_scheduler.rs` | 1,252 | 9 | 139 | Projection scheduler/queue |
| `persistence/mod.rs` | 5,869 | 43 | 136 | Session/data persistence |
| `projections.rs` | 4,220 | 36 | 117 | Projection engine core |
| `commands.rs` | 14,716 | 135 | 109 | **Tauri command hub — 97 `#[tauri::command]`, 438 fns** (69 commits) |
| `promotion.rs` | 1,818 | 22 | 82 | Entity promotion |
| `settings/mod.rs` | 4,469 | 61 | 73 | Settings (26 commits) — comparatively well-tested |
| `credentials/mod.rs` | 2,577 | 38 | 67 | Credential store (14 commits) — comparatively well-tested |
| `provider_registry.rs` | 356 | 9 | 39 | Provider registry — well-tested for size |

### 3.2 Zero in-file-test files (and whether that's a real gap)

| File | LOC | Real gap? |
|---|---|---|
| `playback/mod.rs` | 852 | **No** — covered by sibling `playback/tests.rs` (14 tests) |
| `lib.rs` | 560 | Low — app/plugin wiring, exercised by integration tests |
| `asr/ws_fixture.rs` | 322 | No — it *is* a test fixture, not production code |
| `asr/sherpa_streaming.rs` | 142 | Minor — feature-gated (`sherpa-streaming`) |
| `speech/context.rs` | 91 | Minor |
| `llm/mod.rs` (30), `graph/mod.rs` (14), `main.rs` (6) | — | No — module declarations / entrypoint |

**Verdict:** there is no large *zero-test* production module. The Rust risk is **thin tests on huge, high-churn modules** — `commands.rs`, `speech/mod.rs`, `state.rs`, and the `projection_*` group — where a 109–320 LOC/test ratio means large swaths of branch logic are unexercised even though a `#[cfg(test)]` module exists.

---

## 4. Ranked top-10 "highest value per test-hour" gap list

Weighting: **user-facing blast radius × past-bug density × testability** (pure logic with injectable deps scores far higher per hour than UI needing full render/mock harnesses). Credential/config persistence and provider dispatch carry documented bug history and are weighted up.

| Rank | Target | Layer | Why (blast radius / bug history) | Testability |
|---|---|---|---|---|
| 1 | `analytics/safeInvoke.ts` | FE | 0% and **every** IPC call funnels through it; a regression silences all frontend diagnostics. Tiny surface. | **Very high** — mock `invoke` + `captureFrontendError`, assert relay + rethrow. ~30 min. |
| 2 | `store/index.ts` (73.9%, n=706) | FE | Central Zustand store, 40 commits of churn; drives the whole UI. Lowest-covered logic dir. | **High** — pure reducers/actions, no render needed. |
| 3 | `commands.rs` provider/credential/config commands | RUST | The 97-command IPC hub, 69 commits; provider dispatch + credential save/delete have repeated `fix:` history (PRs #26/#29/#39/#46/#51). 109 LOC/test. | **High** for the pure dispatch/validation paths; mock provider clients. |
| 4 | `components/{Asr,Llm}ProviderSettings.tsx` (56–62%) | FE | Provider dispatch/config UI — the exact surface behind the SambaNova/OpenRouter/Deepgram routing bug wave. | Medium — needs render + mocked store, but branch-rich payoff. |
| 5 | `projection_eval.rs` (225 LOC/test) | RUST | Worst density of any core module; projection scoring feeds user-visible knowledge-graph output. | **High** — scoring is pure/deterministic; table-driven tests. |
| 6 | `hooks/useTauriEvents.ts` (82.4%) | FE | Speech/pipeline IPC event glue; dropped/misrouted events surface as "app frozen / no transcript". | Medium — emit mock events, assert store mutations. |
| 7 | `state.rs` (169 LOC/test) | RUST | Central backend state, 35 fns; concurrency/coherence bugs here are high-severity (cache-coherence bug history). | Medium — some async/lock paths. |
| 8 | `components/settings/useSettingsController.tsx` (85.5%, n=1152) | FE | Biggest logic file; the settings/provider/credential controller. Small % gain = many lines. | Medium — large, but pure-ish helpers already extracted. |
| 9 | `App.tsx` (80.3%) + `analytics/ErrorBoundary.tsx` (0%) | FE | App root wiring + the error boundary that catches renderer crashes (currently unverified). | Medium — render + throw-to-boundary test. |
| 10 | `projection_scheduler.rs` (139) / `projections.rs` (117) | RUST | Projection queue + engine; scheduling/queue-persistence has recent feature churn (#62/#617e). | Medium — async scheduler, but core transitions are testable. |

Items 1, 2, 5 are the cheapest points and should go first (pure logic, no harness). Items 3 and 4 have the highest blast-radius given the credential/provider bug history and deserve the most hours.

---

## 5. Rust coverage recipe (ready-to-run when a cargo slot frees)

`cargo-llvm-cov` (source-based LLVM instrumentation) is the right tool — it reuses the exact `cargo test` invocation already in CI, understands feature flags, and emits LCOV/HTML/JSON.

### 5.1 Local one-time setup

```bash
rustup component add llvm-tools-preview
cargo install cargo-llvm-cov --locked
```

### 5.2 Cloud-only surface (fast — matches the PR-gate build, skips the heavy local-ML native compile)

```bash
cd src-tauri
# Human-readable per-file table:
xvfb-run -a cargo llvm-cov \
  --no-default-features --features cloud \
  --summary-only -- --test-threads=1

# Machine-readable LCOV for CI upload / diff tooling:
xvfb-run -a cargo llvm-cov \
  --no-default-features --features cloud \
  --lcov --output-path target/llvm-cov/cloud.lcov.info -- --test-threads=1

# Local browsable report:
xvfb-run -a cargo llvm-cov \
  --no-default-features --features cloud \
  --html -- --test-threads=1   # opens target/llvm-cov/html/index.html
```

> Notes matching this repo's CI: `xvfb-run -a` is required (tao's Linux event loop touches X11 even under MockRuntime), `--test-threads=1` matches the serialized test config, and the `rsac` path dep must be present at `../../rsac`. On macOS drop `xvfb-run`.

### 5.3 Full default (local-ml) surface — heavy, nightly only

```bash
cd src-tauri
xvfb-run -a cargo llvm-cov --summary-only -- --test-threads=1     # default = ["local-ml"]
```

### 5.4 CI job sketch (add to `.github/workflows/ci.yml`, initially **non-gating**)

```yaml
  rust-coverage:
    name: Rust coverage (cloud-only, report-only)
    # Report-only to start: nightly + manual, never a PR gate until a baseline exists.
    if: github.event_name == 'workflow_dispatch' || github.event_name == 'schedule'
    runs-on: blacksmith-4vcpu-ubuntu-2404
    steps:
      - uses: actions/checkout@11bd71901bbe5b1630ceea73d27597364c9af683 # v5
        with: { path: audio-graph }
      - name: Fetch rsac parent (for path dep)
        run: |
          git clone "$RSAC_REPO_URL" "$GITHUB_WORKSPACE/rsac"
          git -C "$GITHUB_WORKSPACE/rsac" checkout --detach "$RSAC_REPO_SHA"
      - uses: dtolnay/rust-toolchain@e081816240890017053eacbb1bdf337761dc5582 # 1.95.0
        with: { components: llvm-tools-preview }
      - uses: Swatinem/rust-cache@42dc69e1aa15d09112580998cf2ef0119e2e91ae # v2
        with: { workspaces: audio-graph/src-tauri }
      - name: Install system dependencies
        run: |
          sudo add-apt-repository ppa:pipewire-debian/pipewire-upstream -y
          sudo apt-get update
          sudo apt-get install -y \
            libpipewire-0.3-dev libspa-0.2-dev libasound2-dev \
            libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
            librsvg2-dev cmake clang libclang-dev pkg-config xvfb
      - name: Install cargo-llvm-cov
        run: cargo install cargo-llvm-cov --locked
      - name: Coverage (cloud-only)
        working-directory: audio-graph/src-tauri
        run: |
          xvfb-run -a cargo llvm-cov --no-default-features --features cloud \
            --lcov --output-path lcov.info -- --test-threads=1
          xvfb-run -a cargo llvm-cov --no-default-features --features cloud \
            --summary-only -- --test-threads=1 | tee coverage-summary.txt
      - uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02 # v4
        if: always()
        with:
          name: rust-coverage-cloud
          path: |
            audio-graph/src-tauri/lcov.info
            audio-graph/src-tauri/coverage-summary.txt
          retention-days: 14
```

---

## 6. Should CI gate on coverage? — recommendation

**Yes, but as a ratchet (never-decrease), not a fixed absolute bar.** A fixed bar is either set so low it gates nothing (today's `vitest.config.ts` at 50–60 while reality is 79–86) or so high it blocks unrelated PRs and invites test-padding. A ratchet gates the one thing that matters: *a PR must not lower coverage*.

### 6.1 Frontend (act now — the tooling already exists)

1. **Raise the stale `vitest.config.ts` thresholds to just under today's measured floor** so they finally bite:
   - lines 80, statements 80, functions 82, branches 72 (each ~2–4 pts under current, absorbing measurement jitter).
2. Keep `bun run test:coverage` in the `frontend` CI job (it already runs) and let the thresholds fail the build on regression.
3. **Ratchet mechanism:** on a green run that *exceeds* the floor by a margin, bump the config numbers upward in a follow-up commit. Optionally add a bot/PR-comment coverage diff so reviewers see per-PR movement without a hard cross-PR baseline store.
   > Caveat: run the gate in the **sequential** pool config on CI, or the parallel worker-spawn flake seen locally could cause false threshold failures. If CI's runners don't exhibit the WSL I/O issue, the default pool is fine — verify on the first few runs.

### 6.2 Rust (report first, gate later)

1. Land the job in §5.4 as **report-only** (artifact upload, no threshold) for 1–2 weeks to establish a true baseline — nobody knows the real Rust line % today.
2. Once the baseline is known, add `--fail-under-lines <baseline-2>` to the cloud-only invocation and flip it to run on PRs. Gate the **cloud** surface only (the default local-ml surface is too slow/heavy for a PR gate; keep it nightly report-only).
3. Then ratchet the `--fail-under-lines` value upward as coverage climbs, prioritizing the §4 targets.

**Net:** a ratchet locks in the already-good frontend number and turns the currently-unmeasured Rust side into a measured, slowly-tightening floor — without ever blocking a legitimately-unrelated PR on an arbitrary absolute threshold.
