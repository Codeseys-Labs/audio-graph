# Sentry Observability Runbook — anonymous analytics

How to turn on the opt-in anonymous analytics channel and read what the app
reports, using `sentry-cli`. Pairs with [ADR-0023](../adr/0023-anonymous-analytics-sentry-integration.md).

## What ships and where events go

- **DSN** (client-side public ingest key, safe to embed):
  `https://1e39b03ea3018d02551500bf428306b9@o4511644093448192.ingest.us.sentry.io/4511644102885381`
  (org `o4511644093448192`, project `4511644102885381`). Override with the
  `SENTRY_DSN` env var; an explicitly-empty `SENTRY_DSN` is a kill switch.
- **Off by default.** `analytics_enabled` defaults to `false`. Enable it in
  Settings → "Privacy & Diagnostics" (independent of local file logging), or set
  it once in the settings file. On the next launch the app emits an anonymous
  `app.startup` event so you get immediate confirmation telemetry is flowing.
- **Anonymous.** `send_default_pii = false` + a `before_send` scrubber strips
  secrets, transcript text, user, IP, server name, tags, and stack-frame source.
  No transcript/audio/credential data leaves the machine.

## Verify the pipeline (no app run needed) — SEND side

`sentry-cli send-event` uses only the DSN (send-only), so you can prove the
transport/project/ingestion are live before the app even runs:

```bash
SENTRY_DSN="https://1e39b03ea3018d02551500bf428306b9@o4511644093448192.ingest.us.sentry.io/4511644102885381" \
  sentry-cli send-event \
  --message "audio-graph pipeline verification" \
  --level info --tag "verification:true" \
  --release "audio-graph@$(git rev-parse --short HEAD)"
# -> "Event dispatched. Event id: <uuid>"
```

Verified working 2026-06-28 (event id `f32c52bc-e4af-4cd3-9044-81250c564ed6`).

## Read events back — RECEIVE side (needs YOUR token)

Reading events/issues needs a Sentry auth token with `event:read` +
`project:read` scope. The token currently in `~/.sentryclirc` is `org:ci`-only
(upload scope) and gets a 403 on reads — create a personal token at
**sentry.io → Settings → Auth Tokens** with `event:read`/`project:read`, then:

```bash
export SENTRY_AUTH_TOKEN=<your-token-with-event:read>
export SENTRY_ORG=<your-org-slug>          # the human slug, not the o-number
export SENTRY_PROJECT=<your-project-slug>

sentry-cli organizations list                       # confirm the token sees the org
sentry-cli issues list                              # recent issues (errors/panics)
sentry-cli events list                              # recent individual events
sentry-cli send-event --message "ping" && sentry-cli events list | head
```

Or just open the project dashboard:
`https://<org>.sentry.io/issues/?project=4511644102885381`.

## What the app captures

- **`app.startup`** — one anonymous Info event per launch (when enabled). Your
  "is telemetry on?" smoke signal.
- **Panics** — the Sentry `panic` integration captures any thread panic
  (scrubbed). The local crash handler (`crash_handler`) still writes a local
  crash report independently — that one is never gated by this toggle.
- **Explicit diagnostics** — `analytics::capture_message` /
  `capture_anonymous_event` are the only intentional send paths; they are used
  sparingly and never carry transcript/audio/credential data.

Provider WS/connect + reconnect errors are routed through
`redacted_provider_diagnostic` before they reach logs OR Sentry, so an enabled
analytics channel never leaks an API key from a failed Deepgram/AssemblyAI/
Soniox/OpenAI-Realtime/Gemini connection.

## Runtime toggle semantics

OFF closes the shared client transport on the process hub (`Hub::main`) — a
thread-global kill — and drops the guard; a later ON re-inits a fresh client.
See `src-tauri/src/analytics/mod.rs` for the full contract.
