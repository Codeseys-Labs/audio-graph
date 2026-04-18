# Gemini Reconnect Runbook

Operational guide for diagnosing Gemini Live WebSocket disconnect / reconnect
behaviour in the `audio-graph` Tauri app. Companion to the design doc
[`../designs/session-management.md`](../designs/session-management.md) and the
implementation in `src-tauri/src/gemini/mod.rs`.

## Overview

The Gemini Live client is a synchronous wrapper around a WebSocket session run
on a dedicated tokio runtime. A background `session_task` drives the
reader/writer via `tokio::select!` and, on any network-layer drop or
server-initiated close (`goAway`, Close frame, TLS/TCP error, protocol
violation), automatically reconnects with exponential backoff. The backoff
schedule is `1s → 2s → 5s → 10s`, and after 4 failed attempts the task gives
up and emits a fatal `Error` event (`"Gemini reconnect attempts exhausted"`).

Every reconnect replays the full setup handshake — `BidiGenerateContentSetup`
is re-sent and the client waits for a fresh `setupComplete` before resuming
I/O. The Gemini-specific wrinkle is **session resumption**: the server pushes
`sessionResumptionUpdate` frames whose `newHandle` the client caches (only
when `resumable == true`). On reconnect that handle is threaded back into
`setup.sessionResumption.handle` so the server can restore prior conversation
state. When no handle is available yet or the server rejects it, the next
session starts fresh — the client falls back transparently.

User-initiated teardown (`disconnect()` or `Drop`) sets `user_disconnected`
and short-circuits the reconnect loop so the session exits cleanly instead of
trying to recover from its own shutdown. In-flight audio commands are
preserved across reconnects because `audio_rx` is owned by the session task
and not torn down between socket instances — the caller never sees a spurious
"Not connected" error for a transient hiccup. However, any in-flight model
**turn** on the dead socket is lost: the fresh socket starts from a blank
`turnComplete` state.

## How to Tell a Reconnect Happened

All Gemini log lines are tagged with prefixes like `Gemini session:`,
`Gemini Live:`, or `Gemini:`. Grep the app log for these markers:

```sh
# Any disconnect/reconnect activity
rg 'Gemini session:' logs/

# Reconnect cycle explicitly
rg 'Gemini session: reconnecting' logs/

# Successful recovery
rg 'Gemini session: reconnected on attempt' logs/

# Fatal — budget exhausted
rg 'reconnect budget exhausted' logs/

# Did resumption kick in?
rg 'reconnecting with resumption handle' logs/

# …or did we fall back to a fresh session?
rg 'reconnecting without resumption handle' logs/
```

A complete reconnect cycle produces (at minimum) these log lines in order:

1. `Gemini session: disconnected — <DisconnectKind>` (warn)
2. `Gemini session: reconnecting (attempt N, backoff Ns)` (info)
3. `Gemini session: reconnecting with resumption handle` **or**
   `Gemini session: reconnecting without resumption handle (new session)` (info)
4. Either `Gemini Live: setup complete` + `Gemini session: reconnected on
   attempt N` (success) or `Gemini session: reconnect attempt N failed: <err>`
   (failure — loop re-enters at step 2).

On the event bus, consumers see `Disconnected → Reconnecting { attempt,
backoff_secs } → Reconnected` (or → `Error` if the budget is blown).

## Fresh vs. Resumed Session

At the code level the distinction is logged but **not yet signalled on the
event bus** — `GeminiEvent::Reconnected` is currently a unit variant with no
payload. To tell whether the recovered session kept its prior context, grep
the log immediately above the `reconnected on attempt N` line:

| Log line                                                               | Meaning                                                                 |
| ---------------------------------------------------------------------- | ----------------------------------------------------------------------- |
| `Gemini session: reconnecting with resumption handle`                  | Client had a cached handle; server will attempt to restore session state. |
| `Gemini session: reconnecting without resumption handle (new session)` | No handle cached (first outage happened before any resumable update); session starts fresh. |

Note that "with handle" is a best-effort signal from the client side — the
server may still reject an expired or invalid handle and silently start a
fresh session (the handshake still returns `setupComplete`). If you need to
confirm continuity, check whether the model's next response references prior
turns.

> **NOTE:** the `resumed vs. fresh` distinction on the wire (a struct variant
> `Reconnected { resumed: bool }`) is being added in loop-16 task A1. Once
> A1 lands, the frontend can surface this directly; until then, log
> inspection is the only way to tell. After A1 ships, expect a new log line
> of the form `Gemini session: reconnected on attempt N (resumed=true)` —
> update this runbook when the code lands.

## Known Failure Modes

### Handle expired or rejected

- **Symptom**: `reconnecting with resumption handle` is logged, the
  handshake succeeds, but the model has clearly lost prior context.
- **Cause**: Server rejected the handle (expired, wrong project, unknown
  format). No error is surfaced — the server silently falls back to a fresh
  session after returning `setupComplete`.
- **Remediation**: None needed; the session is functional, just not
  resumed. If this recurs, escalate (see below) and capture the failing
  handle for Google support.

### Network partition / exponential backoff exhaustion

- **Symptom**: Repeated `Gemini session: reconnect attempt N failed: <err>`
  followed after 4 attempts by `reconnect budget exhausted after 4
  attempts` and a `GeminiEvent::Error` with message
  `"Gemini reconnect attempts exhausted"`.
- **Cause**: Network is down longer than ~18 seconds total (1+2+5+10), or
  persistent TLS / DNS failure, or credentials expired mid-session (Vertex
  bearer token).
- **Remediation**: Check general connectivity. For Vertex, verify the
  service account key and `GOOGLE_APPLICATION_CREDENTIALS` are still
  valid. Then manually bounce (below).

### WebSocket 4xx/5xx on initial connect

- **Symptom**: `connect()` returns a synchronous `Err` with
  `"WebSocket connect failed: ..."` *before* any session task spawns. No
  reconnect loop runs because the session never started.
- **Cause**: Wrong API key, Vertex auth misconfigured (missing
  `project_id`/`location`), invalid model name, or firewall blocking the
  endpoint.
- **Remediation**: Verify the relevant settings and re-invoke `start_gemini`.

### `goAway` during active session

- **Symptom**: `Gemini Live: received goAway — server is shutting down`
  (warn) immediately followed by a `Disconnected → Reconnecting` cycle. The
  client emits a `GeminiEvent::Error { message: "Server sent goAway; ..." }`
  alongside the normal reconnect sequence.
- **Cause**: Server-side graceful shutdown — usually a backend rollout or
  node drain. Not a client fault.
- **Remediation**: None; the reconnect loop handles this transparently.
  If the reconnect lands on the same downshifting node it may cycle a
  second time, but the `1s → 2s → 5s → 10s` budget is generally enough.

### Protocol error

- **Symptom**: `Gemini session: disconnected — ProtocolError(...)` followed
  by a reconnect.
- **Cause**: Malformed frame from the server (rare) or a tungstenite version
  mismatch.
- **Remediation**: If it recurs on reconnect, capture the error string and
  escalate — this likely needs a code-level fix, not an ops action.

### Setup handshake timeout

- **Symptom**: `Timed out waiting for setupComplete` surfaced as a
  reconnect-attempt failure.
- **Cause**: Socket opens but the server never returns `setupComplete`
  within 15 s. Typically indicates server overload or a bad model path.
- **Remediation**: Verify the configured model is a Live-preview model. If
  the model is correct, wait one reconnect cycle; persistent failures
  warrant escalation.

## Log Lines Reference

Exact strings emitted by `src-tauri/src/gemini/mod.rs`. Grep targets.

| Log string (prefix match)                                              | Level | Meaning                                                                                   |
| ---------------------------------------------------------------------- | ----- | ----------------------------------------------------------------------------------------- |
| `Gemini Live: setup complete`                                          | info  | Initial connect finished handshake; session is live.                                      |
| `Gemini Live: pre-setup message: <text>`                               | debug | Non-`setupComplete` frame seen during handshake (unusual but harmless).                   |
| `GeminiLiveClient: disconnecting (user-initiated)`                     | info  | `disconnect()` called; session task will exit without reconnecting.                       |
| `GeminiLiveClient: dropped`                                            | info  | Client dropped; runtime shutdown under way.                                               |
| `Gemini session: ending (UserRequested)`                               | info  | Clean end after `disconnect()`.                                                           |
| `Gemini session: ending (WriterEnded)`                                 | info  | Audio sender closed (client dropped); session ended normally, no reconnect.               |
| `Gemini session: disconnected — ServerClose(...)`                      | warn  | Server sent a Close frame; reconnect will follow.                                         |
| `Gemini session: disconnected — NetworkError(...)`                     | warn  | TLS/TCP/tungstenite I/O error; reconnect will follow.                                     |
| `Gemini session: disconnected — ProtocolError(...)`                    | warn  | Malformed frame; reconnect will follow.                                                   |
| `Gemini session: reconnecting (attempt N, backoff Ns)`                 | info  | Starting backoff before reconnect attempt N.                                              |
| `Gemini session: user cancelled during backoff`                        | info  | `disconnect()` called mid-backoff; session exits without attempting reconnect.            |
| `Gemini session: reconnecting with resumption handle`                  | info  | Reconnect will send a cached handle so server can restore state.                          |
| `Gemini session: reconnecting without resumption handle (new session)` | info  | No cached handle; next session starts fresh.                                              |
| `Gemini session: reconnected on attempt N`                             | info  | Reconnect succeeded (new socket + setup-complete); resuming I/O.                          |
| `Gemini session: reconnect attempt N failed: <err>`                    | warn  | Reconnect attempt failed; will try next backoff step.                                     |
| `Gemini session: reconnect budget exhausted after N attempts`          | error | Gave up after 4 failed attempts; fatal `Error` event emitted, session task exits.         |
| `Gemini: session task exited`                                          | info  | Session task has fully unwound.                                                           |
| `Gemini: server closed connection: <frame>`                            | info  | Close frame observed in the read loop. Followed by classification into `ServerClose`.     |
| `Gemini: failed to send audio: <err>`                                  | error | Write failed; classified as `NetworkError` and triggers reconnect.                        |
| `Gemini: WebSocket read error: <err>`                                  | error | Read failed with a non-Close error; classified as `NetworkError` and triggers reconnect.  |
| `Gemini: unexpected binary message`                                    | warn  | Non-text frame received (should not happen with TEXT-only modality); ignored.             |
| `Gemini Live: received goAway — server is shutting down`               | warn  | Server is draining; client will reconnect.                                                |
| `Gemini Live: session resumption handle refreshed`                     | debug | New resumable handle cached from a `sessionResumptionUpdate`.                             |
| `Gemini Live: sessionResumptionUpdate with resumable=false`            | debug | Update arrived but resumption is temporarily unavailable; cached handle preserved.        |
| `Gemini Live: invalid JSON: <err>`                                     | warn  | Server sent non-JSON text; emitted as an `Error` event but does not end the session.      |
| `Gemini Live: turn complete with usage (...)`                          | debug | `turnComplete` frame carried token accounting.                                            |
| `Gemini Live: standalone usage frame (total=...)`                      | debug | `usageMetadata` arrived without `serverContent` (billing roll-up).                        |
| `Gemini Live: unhandled message: <text>`                               | debug | Server frame the client does not recognise; safe to ignore.                               |

## Escalation

**Bouncing the Gemini session** means forcing a fresh connect cycle. In order
of least-to-most disruptive:

1. **Restart the speech processor from the UI** — toggle the Gemini provider
   off and on in Settings. This invokes `stop_gemini` → `start_gemini`,
   which calls `GeminiLiveClient::disconnect()` (clean teardown, no
   reconnect loop) followed by a fresh `connect()` with a new runtime and
   an empty resumption handle cache. Use this when the session is wedged
   (budget exhausted, stale handle being repeatedly rejected, or auth
   change).

2. **Restart the audio-graph app** — full process restart. Use when the
   runtime itself looks unhealthy (tokio panic in logs, runtime shutdown
   timeout messages, or `GeminiLiveClient: dropped` with no subsequent
   reconnect). This also clears any in-process auth token caches.

3. **Check Vertex credentials** — if auth mode is Vertex AI, inspect
   `GOOGLE_APPLICATION_CREDENTIALS` and the service account key file
   pointed to by the settings. Renewing a rotated key typically requires
   scenario 1 or 2 above to pick up the new value.

Escalate to engineering when:

- Reconnect budget is exhausted **repeatedly** within a short window (the
  network is not to blame — investigate server side).
- `ProtocolError` appears in logs (not caused by ops; needs a code fix).
- A resumption handle is repeatedly cached but the model clearly loses
  context across every reconnect (possible server-side regression).

## Related

- Design: [`../designs/session-management.md`](../designs/session-management.md)
- Design: [`../designs/provider-architecture.md`](../designs/provider-architecture.md)
- Code: [`../../src-tauri/src/gemini/mod.rs`](../../src-tauri/src/gemini/mod.rs)
  — in particular `session_task`, `open_ws`, `DisconnectKind`,
  `build_setup_message`, and `handle_server_message`.
- Upstream: <https://ai.google.dev/api/live#session-management>
