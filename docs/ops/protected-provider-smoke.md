# Protected provider smoke — operations runbook (audio-graph-315d / audio-graph-8772 / audio-graph-f3e3)

**Status: workflow landed, awaiting the first dispatched run.** The Rust-side
tests, the offline redaction/threshold proofs, and `protected-provider-smoke.yml`
are implemented and CI-green on every ordinary PR/push run (the two live tests
stay `#[ignore]`d there). The first execution against the real providers is
the dispatched or scheduled run of `protected-provider-smoke.yml` on `master`
after this lands — that run has not happened yet as of this writing.

This is the Tier C, non-PR-gating counterpart to `docs/ops/b18-converse-live-smoke.md`:
a human-run checklist there, a scheduled/dispatch-only CI job here. Neither
replaces the other.

## What this proves

- `asr::deepgram::tests::live_deepgram_streaming_smoke` drives the real
  `DeepgramStreamingClient` — `connect()` -> `send_audio()` -> `event_rx()` ->
  `disconnect()` — against a checked-in speech fixture, not the raw WS
  handshake helper the older `live_deepgram_handshake_rejects_general_accepts_nova3`
  test uses. It requires at least one normalized transcript event, a
  speech-final/end-of-turn signal, a tolerant keyword-hit threshold against
  the fixture's known reference transcript, and non-decreasing event timing.
- `llm::openrouter::tests::live_openrouter_routed_smoke` now dispatches
  through `route_for_openrouter_policy` — the same named route table
  `executor.rs` uses for production traffic (audio-graph-3624) — before
  issuing one tiny synthetic completion, so the smoke proves the routing
  decision itself, not only the raw client call.
- Both tests print a sanitized, content-free report (`StreamingSmokeReport` /
  `RoutedSmokeReport`) and a stable "strict pass" marker string that the
  workflow's vacuous-pass guard greps for.

## Credential handling

- Each provider gets its own narrowly-scoped test credential, distinct from
  any production credential, held only as a repository secret inside the
  `real-key-testing` GitHub Environment.
- The `live-provider-smoke` job carries an explicit
  `if: github.ref == 'refs/heads/master'` guard, so master-only enforcement
  holds even before the `real-key-testing` Environment is configured in repo
  settings, and even though referencing a not-yet-configured Environment name
  auto-creates it with no protection rules on first use. The `real-key-testing`
  Environment should ALSO be configured with a deployment branch policy
  restricted to `master` (repo Settings -> Environments) as defense-in-depth
  on top of the job-level guard — `workflow_dispatch` and `schedule` are the
  only triggers `protected-provider-smoke.yml` declares; it never runs on
  `pull_request`, so it cannot become a required PR status check by
  construction, and a PR from a fork never sees these secrets.
- Neither test logs the credential value, the synthetic prompt, the
  synthetic speech fixture's transcript text, or raw audio — only counts,
  durations, model/route identifiers, and status strings ever leave the test
  process. `report_carries_no_content_fields` (OpenRouter) and
  `streaming_smoke_report_has_no_content_fields` (Deepgram) assert this
  structurally on every ordinary CI run, without needing either credential.

### Rotation, budget, and revocation

Neither provider account is documented here by identity or account details —
this file only records the *policy* every rotation must satisfy, not the
account itself:

1. **Scope.** Each test credential should carry the narrowest role/allowlist
   the provider supports (a dedicated test project/workspace where possible),
   a model/provider allowlist where the provider exposes one, and — for
   OpenRouter — a key-level spending limit and no input/output logging
   enabled on the key.
2. **Budget.** Set a low monthly spend limit and an alert threshold on each
   credential. The smoke sends one short synthetic prompt (OpenRouter, tiny
   `max_tokens`) and one short synthetic PCM fixture (Deepgram, ~6.4 seconds)
   per scheduled run — cost per run should stay a small fraction of the
   monthly limit.
3. **Expiration.** Give each credential a fixed expiration where the provider
   supports it, and calendar a rotation before that date rather than waiting
   for it to fail closed mid-schedule.
4. **Rotation.** Generate the replacement credential, update the
   `real-key-testing` Environment secret, dispatch this workflow once by hand
   to confirm the new credential authenticates, then revoke the previous
   credential at the provider.
5. **Revocation on suspicion.** If usage, spend, or a provider security
   notice looks anomalous for either credential, revoke it immediately at the
   provider first, then update the Environment secret — do not wait for the
   next scheduled run to notice.
6. **Never reuse a developer's personal or production credential** for this
   workflow, and never place either credential in a workflow file, a script
   default, a doc, or `.seeds/`. The secret-hygiene scan below reliably
   catches an accidental paste of an OpenRouter key (its `sk-or-...` shape
   matches `scripts/check-docs-secret-hygiene.mjs`'s `openai-key` rule) — but
   its Deepgram rule only matches a `dg_`/`dg-`-prefixed shape, and a live
   Deepgram key is a bare, unprefixed 40-character hex string that rule does
   not match. Treat the scan as a real but incomplete backstop for a pasted
   Deepgram key specifically, and rely primarily on not pasting either
   credential anywhere outside the `real-key-testing` Environment secret in
   the first place.

## Secret-hygiene scanning (audio-graph-f3e3)

`scripts/check-docs-secret-hygiene.mjs` scans a fixed, narrow set of roots
(`docs/`, `.seeds/`, `README.md`, `AGENTS.md`) for provider-shaped key
patterns and credential-presence prose claims. It takes no path argument by
design — its allowlist is deliberately narrow so that extending it casually
does not quietly widen what is exempt from scanning.

That scanner cannot see `target/protected-smoke-logs/` (the CI evidence
directory the smoke tests write their sanitized reports into) because that
path is outside its fixed roots. Two separate, complementary gates cover the
two different surfaces instead of extending the scanner's roots to reach into
a build-output directory:

1. **Docs/Seeds narrative gate.** `protected-provider-smoke.yml` runs
   `bun scripts/check-docs-secret-hygiene.mjs` unmodified. This covers any
   status update written back into `docs/` or `.seeds/` about this smoke
   (including this file) — not the evidence artifact itself. **Coverage gap
   (Deepgram specifically):** the scanner's `deepgram-key` rule only matches
   a `dg_`/`dg-`-prefixed shape; a real Deepgram key is bare, unprefixed
   40-character hex, which no rule in the scanner matches (a blind
   40-hex-character rule was deliberately NOT added here — this repo's own
   `.seeds/issues.jsonl` and several `docs/` files already contain
   unrelated, legitimate 40-character git SHAs, so that rule would fail the
   gate on pre-existing, non-secret content). A pasted real Deepgram key
   would currently pass this gate undetected; do not rely on it for that
   specific case — see "Rotation, budget, and revocation" item 6 above.
2. **Evidence-specific gate.** The workflow's "Evidence redaction check" step
   greps the sanitized log files for the literal secret values the run's own
   `DEEPGRAM_API_KEY` / `OPENROUTER_API_KEY` secrets hold, and fails the job
   if either literal appears. This is defense-in-depth layered on top of —
   not a substitute for — the structural, credential-free guarantee the two
   report types already provide on every ordinary CI run (no key needed):
   `report_carries_no_content_fields` and
   `streaming_smoke_report_has_no_content_fields`.

This split is the explicit, reproducible choice for audio-graph-f3e3: keep
`check-docs-secret-hygiene.mjs` scoped to docs/Seeds text exactly as it is
today, and let the Rust-side structural guarantees plus a workflow-local
literal-value grep cover the CI evidence artifact. Extending the scanner's
`scanRoots`/CLI to point directly at a build-output directory remains an
option for later if the evidence path grows more complex than two small log
files, but is not needed for the current two-provider shape.

## Running it manually

Dispatch `protected-provider-smoke.yml` from the Actions tab (`workflow_dispatch`)
on `master` to run both smokes ahead of the nightly schedule, for example
right after rotating a credential. Sanitized evidence (the two log files,
containing only counts, timing, model/route identifiers, and status strings)
is uploaded as the `protected-provider-smoke-evidence` artifact regardless of
whether the smoke *tests themselves* passed or failed — but ONLY when the
"Evidence redaction check" step (which greps those logs for the run's own
raw `DEEPGRAM_API_KEY` / `OPENROUTER_API_KEY` values) itself succeeds. If
that check fails — i.e. it found a live key literal in a log — the upload
step is skipped, so a caught leak is never also published as a downloadable
artifact.
