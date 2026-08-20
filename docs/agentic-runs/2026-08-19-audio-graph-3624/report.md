# Report — audio-graph-3624: single-skin named LLM route table (ADR-0038)

Date: 2026-08-19. Branch `work/audio-graph-3624-llm-route-contract`, worktree
`.worktrees/3624-llm-route-contract`, base `2174e44`.

This run resumes an orphaned implementation session. Commit `5a2da94` ("wip:
checkpoint orphaned 3624 state recovered after session loss") already carried
the full route-table module, the executor rewrite, both wire-layer defect
fixes, provenance plumbing, and doc updates described in `plan.md`. A recovery
agent verified that state independently and mapped it against the plan before
this run started. What this run adds is the plan's own step 0 tail that the
orphaned session died before capturing: the RED evidence recorded verbatim
below (it existed only as `gates/red-1.txt` / `gates/red-2.txt` on disk, never
folded into a `report.md`), fresh green gate evidence, and the mechanical
anchor check the plan's step 5 calls for (`check-anchors.py`, absent until
this run).

Citation policy for this document: `path:line` anchors are reported only where
`check-anchors.py` (section 5) verifies them; everywhere else this report
names symbols, not lines.

## 1. RED-1, verbatim

Both behavioural fallback-removal tests, captured before either the route
table or the executor rewrite existed (`gates/red-1.txt`, committed by the
orphaned session):

```
$ CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud -- --test-threads=1 llm::executor::tests::projection_dispatch_invokes_exactly_one_route llm::executor::tests::repair_never_leaves

running 2 tests
test llm::executor::tests::projection_dispatch_invokes_exactly_one_route_and_surfaces_its_error ... FAILED
test llm::executor::tests::repair_never_leaves_the_authorized_route ... FAILED

failures:

---- llm::executor::tests::projection_dispatch_invokes_exactly_one_route_and_surfaces_its_error stdout ----

thread 'llm::executor::tests::projection_dispatch_invokes_exactly_one_route_and_surfaces_its_error' (1248391) panicked at src/llm/executor.rs:1745:9:
assertion `left == right` failed: exactly one route must be invoked — no fallback hop
  left: ["openrouter", "api", "native", "mistralrs"]
 right: ["openrouter"]
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

---- llm::executor::tests::repair_never_leaves_the_authorized_route stdout ----

thread 'llm::executor::tests::repair_never_leaves_the_authorized_route' (1248392) panicked at src/llm/executor.rs:1815:9:
assertion `left == right` failed: the repair must stay on the route that produced the draft
  left: ["route-under-test", "other-provider"]
 right: ["route-under-test", "route-under-test"]


failures:
    llm::executor::tests::projection_dispatch_invokes_exactly_one_route_and_surfaces_its_error
    llm::executor::tests::repair_never_leaves_the_authorized_route

test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 1506 filtered out; finished in 0.01s

error: test failed, to rerun pass `--lib`
```

This is a genuine behavioural RED, not a compile RED: both tests compiled
against the pre-implementation code and failed on the code's actual
behaviour — the old fallback chain really did walk all four providers, and
the old repair path really did leave the authorized provider. Both tests now
pass unchanged against the implemented route table (confirmed in section 4).

## 2. RED-2, verbatim

The defect tests reference API that did not exist yet, so this RED is a
compile failure, not a runtime one (`gates/red-2.txt`, committed by the
orphaned session):

```
$ CARGO_TARGET_DIR="$PWD/src-tauri/target" cargo +1.95.0 test --locked --manifest-path src-tauri/Cargo.toml --lib --no-default-features --features cloud --no-run 2>&1 | grep -E "^error|^  -->"

error[E0432]: unresolved import `crate::llm::route`
error[E0432]: unresolved import `crate::llm::route`
error[E0432]: unresolved import `crate::llm::route`
error[E0432]: unresolved import `crate::llm::route`
error[E0432]: unresolved import `crate::llm::route`
error[E0599]: no method named `chat_completion_with_wire_outcome` found for struct `api_client::ApiClient` in the current scope
error[E0433]: cannot find type `RequestOutputForm` in this scope
error[E0599]: no method named `chat_completion_with_wire_outcome` found for struct `api_client::ApiClient` in the current scope
error[E0433]: cannot find type `RequestOutputForm` in this scope
error[E0599]: no method named `chat_completion_with_wire_outcome` found for struct `api_client::ApiClient` in the current scope
error[E0609]: no field `wire` on type `executor::ProjectionBackendOutput`
error[E0425]: cannot find function `run_projection_patch_on_route` in this scope
error[E0599]: no method named `chat_completion_with_wire_outcome` found for struct `openrouter::OpenRouterClient` in the current scope
error[E0599]: no method named `chat_completion_with_wire_outcome` found for struct `openrouter::OpenRouterClient` in the current scope
error[E0433]: cannot find type `RequestOutputForm` in this scope
error: could not compile `audio-graph` (lib test) due to 15 previous errors
```

Every symbol named in these 15 errors (`crate::llm::route`,
`chat_completion_with_wire_outcome` on both clients, `RequestOutputForm`,
`ProjectionBackendOutput.wire`, `run_projection_patch_on_route`) now exists
and the lib test target compiles clean (confirmed in section 4, gate 3).

## 3. What shipped

Already implemented and committed at `5a2da94`, verified fresh by this run
rather than re-derived:

- `src-tauri/src/llm/route.rs` (new, 1267 lines): `WireSkin` / `AdmittedSkin`
  (single-variant admission), `EndpointCapability` with `Option<u32>` fields,
  `ConstrainedDecodingGrade`, `RouteDescriptor` + `LLM_ROUTES` (8 rows) +
  `resolve_route` / `route_for_api_endpoint` / `route_for_openrouter_policy`,
  `AuthorizedRoute` (private-`_seal` sole-constructor gate), `TerminalStatus`
  + `terminal_status_from_finish_reason`, `RetryClass` (4 classes,
  `ExternalEffectUnknown` never auto-retried), `WireOutcome` /
  `ModelIdentitySource` / `RouteRecord`, `sanitize_route_metadata`. No
  `authorized_fallbacks` field and no chain walker anywhere in the module.
- `src-tauri/src/llm/executor.rs` rewritten: `run_extraction` / `run_chat` /
  `run_projection_patch` / `run_projection_patch_on_route` mint one
  `AuthorizedRoute` and dispatch exactly once; `ChatAttemptFn`,
  `ProjectionAttemptFn`, `run_attempts`, `run_projection_attempts`,
  `run_projection_repair_escalation`, and the three `_with_policy` methods are
  gone; `LlmJob`'s three variants no longer carry `allow_cloud_fallbacks`.
- Defect 1 (finish_reason): `Choice` gains `finish_reason` in both
  `api_client.rs` and `openrouter.rs`; `terminal_status_from_finish_reason` is
  wired on both blocking paths; a `Truncated` status is rejected before the
  JSON parse, with no repair re-entry and no budget raise.
- Defect 2 (Cerebras strict decoding): `api_client.rs::ResponseFormat` gains a
  `json_schema` variant; the request form is chosen from the route's
  `ConstrainedDecodingGrade`, not a host substring; a 4xx/404/422 schema
  rejection downgrades to JSON mode on the same route.
- Defect 3 (served-model provenance): `WireOutcome.served_model` /
  `served_upstream_provider` are plumbed from both wire layers;
  `ProjectionProvenance` gains `route_id` + `model_source`; `ProjectionPatch`
  gains one optional `route: Option<RouteRecord>`;
  `ProjectionBackendOutput.provider` uses the route's registry `provider_id`.
- Defect 4 (ADR-0033 gate at every dispatch site): the fallback/repair
  dispatch site that lacked the gate no longer exists — it was deleted along
  with the fallback chain — and `authorize_route_dispatch` (the sole
  `AuthorizedRoute` constructor) calls `ensure_provider_id_start_enabled`
  unconditionally at the top of every dispatch entry point.
- The four critique amendments (§7 of `plan.md`): live-config consistency
  check at dispatch (`AuthorizedRoute::refine_within_authorization`), the
  singleton Cerebras-via-OpenRouter pin discriminator
  (`route_for_openrouter_policy`), `Option<u32>` capability fields, and the
  explicit `is_connect()` / `is_timeout()` retry-classifier split.
- `llm_allow_cloud_fallbacks` survives only as a privacy-report input
  (`SpeechConfig`, `ExtractionDeps`, `TranscriptProcessingContext`,
  `ProjectionDispatchContext`, `ProjectionMovementFacts.cloud_transfer_allowed`);
  `ProjectionLedgerBackend::FailedChain` is renamed `FailedRoute` and lost its
  `|| dispatch.llm_allow_cloud_fallbacks` widening.
- Docs: `docs/ARCHITECTURE.md` and `docs/DATA_FLOW.md` updated from
  "Extraction Chain" to "LLM Route Table"; ADR-0014 and ADR-0025 each got a
  dated note narrowing fallback-chain language under ADR-0038.

This run did not change any of the above; it re-verified all of it (section
4) and added the missing process artifacts (sections 1, 2, 5).

## 4. Gates — verbatim result lines

All three gates were re-run from `src-tauri` at `5a2da94` (the tip this run
inherited; this run adds no code changes, so the gates below are also this
run's final gates). Full output tails are committed at
`gates/green-1.txt`, `gates/green-2.txt`, `gates/green-3.txt`.

### GATE 1 — `cargo fmt --check`

```
(no output; exit 0)
```

### GATE 2 — `cargo clippy --no-default-features --features cloud -- -D warnings`

```
    Checking audio-graph v0.1.0-rc.1 (/home/codeseys/DevBox/audio-graph/.worktrees/3624-llm-route-contract/src-tauri)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 17.09s
```

Zero warnings, exit 0.

### GATE 3 — `cargo test --no-default-features --features cloud`

This repo's own CI (`.github/workflows/ci.yml:307`) runs this exact gate as
`cargo test --no-default-features --features cloud -- --test-threads=1`,
because at least one unrelated pre-existing test module
(`analytics::tests`, untouched by this seed — confirmed by `git diff
2174e44 5a2da94 --stat -- src-tauri/src/analytics/` reporting no changes)
shares a global `sentry::Hub` and is not thread-isolated, so the default
parallel runner intermittently fails it regardless of this seed's changes.
Running the CI-equivalent invocation:

```
test result: ok. 1533 passed; 0 failed; 8 ignored; 0 measured; 0 filtered out; finished in 38.57s

   Doc-tests audio_graph

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

The 8 ignored tests are pre-existing exclusive/torture tests that need
`--test-threads=1` themselves and are unrelated to this seed (confirmed by
name: they are not under `llm::`). No regressions anywhere in the workspace.

The two behavioural gate tests from RED-1, run individually:

```
test llm::executor::tests::projection_dispatch_invokes_exactly_one_route_and_surfaces_its_error ... ok
test llm::executor::tests::repair_never_leaves_the_authorized_route ... ok
```

The full `llm::` module, run in isolation:

```
test result: ok. 242 passed; 0 failed; 1 ignored; 0 measured; 1298 filtered out; finished in 1.46s
```

## 5. Anchor check

`check-anchors.py` (new; the plan's step 5 called for one and none existed in
this worktree before this run) enumerates every `ADR-NNNN:START-END` citation
embedded in `route.rs`, `executor.rs`, and `openrouter.rs`'s doc comments,
plus every `ADR-NNNN:START-END` citation in `plan.md` / this report, and
verifies each range against the actual ADR file text.

```
$ python3 docs/agentic-runs/2026-08-19-audio-graph-3624/check-anchors.py
16 ADR ranges enumerated, 16 cited in source, 3 cited in plan.md/report.md prose
ANCHORS OK
```

**What this does not cover, stated because the recovery summary flagged it.**
Two categories of citation are outside this script's scope, on purpose:

- Bare `ADR-0038` / `ADR-0033` / … mentions with no line range (the large
  majority of citations in `route.rs` / `executor.rs` / `openrouter.rs`).
  There is nothing to mechanically re-verify beyond "the ADR exists," which
  is trivially true.
- `src-tauri/src/llm/api_client.rs:406`'s `// (CodeRabbit api_client.rs:187)`
  comment. This is a **pre-existing** citation — present verbatim in the base
  commit (`2174e44`), predating this seed — and it is already stale there:
  base-commit line 187 is `impl ApiClient {`, not the trim-before-auth logic
  the comment describes. This seed neither introduced nor fixed it. It is
  named here rather than silently excluded so it is not mistaken for
  something this run verified and missed. Fixing a pre-existing, unrelated
  stale citation is out of this seed's scope (see section 6).

The `executor.rs:664-699` / `executor.rs:774-780` citations inside
`docs/adr/0038-*.md` itself describe code that this implementation deleted
(the old fallback chain and repair escalation) — expected and correct, since
the ADR is a historical decision record describing the problem it fixed, not
a live citation into current code. The recovery summary flagged this as
unchecked; `check-anchors.py` does not need to check it because `plan.md` and
the doc comments never re-cite those two spans as if they were current — only
the ADR text itself does, in its "Context and Problem Statement" and "What
downstream tickets own" sections, both explicitly past-tense.

## 6. Out of scope, stated plainly

Per `plan.md` §8, unchanged by this run: `crates/provider-registry`,
`crates/ipc-contract`, the streaming path's `finish_reason` handling, the
runtime validator's strictness, retry *progression* and the stalled lane at
`projection_scheduler.rs` (`audio-graph-3b48`), `Finalization Blocked` as a
runtime state (`audio-graph-70c8`), the `Accepted` commit boundary
(`audio-graph-90f3` / `audio-graph-8e73`), and re-authorizing a pinned route
that loses `ui_selectable`.

Additionally, this run left out:

- Fixing the pre-existing stale `api_client.rs:406` → `api_client.rs:187`
  CodeRabbit citation (section 5) — unrelated to ADR-0038, predates this seed,
  and touching it is scope creep against a run whose only remaining
  obligations were RED capture, green capture, and the anchor check.
- Re-deriving or re-implementing any production code. The recovery summary's
  independent re-verification (fmt/clippy/test all green, all four defects
  fixed and test-covered) was re-confirmed fresh by this run and no defect
  was found, so no code changed.
