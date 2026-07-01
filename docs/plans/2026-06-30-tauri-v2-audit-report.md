# Tauri v2 Audit — AudioGraph (Decision-Grade Report)

**Repo:** `/mnt/e/CS/github/audio-graph`
**App:** AudioGraph — Tauri v2 desktop meeting/transcription tool: cloud + local ASR/LLM/TTS providers, event-sourced projections, real-time audio capture, privacy-first opt-in Sentry analytics.
**Stack:** `tauri = 2.11.0`, `tauri-build 2.5.6`, `@tauri-apps/api ^2.11.0`, `@tauri-apps/cli ^2.11.2`; React 19 + Vite 6 + Bun; version `0.1.0-rc.1`.
**Date:** 2026-06-30
**Method:** Synthesis of six research passes (v2 docs) + six read-only code audits, with targeted source verification of load-bearing claims. No files were modified.

---

## Executive Summary — Top 5 Findings (ranked by value)

1. **`ipc::Channel` is used nowhere, and three streaming hot paths (LLM token deltas, partial transcripts, S2S transcription) push one `emit` per item.** This is the single highest-value technical finding. Tauri v2's own docs say *"the event system is not designed for low latency or high throughput situations"* and point to `Channel<T>` for exactly this. `chat-token-delta` (20–100+ emits/sec/completion, "bursts of 50+ deltas per frame") is the textbook `Channel` case — the code even threads a `request_id` through every payload to reconstruct the per-request scoping a `Channel` gives for free. The team clearly understands the problem (frontend 33ms/100ms coalescers, backend 1s download throttle) but applied rate-control to downloads instead of the token/partial streams, and coalescing only reduces React re-renders — it does **not** remove the per-item serialize + event-router + JS-bridge cost. **Verdict: GAP-TO-FILL (M).** Note: audio PCM is correctly kept *off* IPC via internal crossbeam channels — that part is exactly right.

2. **No auto-updater, and the release pipeline carries misleading dead updater-signing config.** There is no `tauri-plugin-updater`, no `plugins.updater` block, no `pubkey`/`endpoints`, no `createUpdaterArtifacts`. Worse, `.github/workflows/release.yml:264-266` passes `TAURI_SIGNING_PRIVATE_KEY` under a comment implying updater support — but with no plugin/config, tauri-action emits **no** `latest.json`/`.sig`, so setting that secret silently does nothing. For an RC-stage app shipping four bundle targets (`app/dmg/nsis/appimage/deb`), users get stranded on old builds. **Verdict: GAP-TO-FILL (M).**

3. **Answer to the logging question: we are NOT using `tauri-plugin-log`, and the roll-your-own is justified — with one small integration gap.** See the dedicated Logging Verdict section below. The bespoke `logging/mod.rs` (425 LOC) does per-target audio-backend noise capping, runtime file-mode/purge UI, and process-wide dep capture that `plugin-log` does not cleanly provide. **Verdict: JUSTIFIED-ROLL-OWN**, but the *frontend* log path is the weak seam (see finding 4).

4. **Privacy-first frontend Sentry egress is silently blocked by CSP `connect-src`.** `@sentry/browser` initializes with the default fetch transport and POSTs to `https://o4511644093448192.ingest.us.sentry.io`, but `tauri.conf.json:23` `connect-src` is `ipc: http://ipc.localhost` only — the host is not allowlisted, so the WebView blocks every frontend envelope as a CSP violation. Backend Rust Sentry (OS sockets, not CSP-governed) still works, which makes the asymmetry easy to miss. All the frontend `scrubEvent` privacy machinery is moot because events never leave the renderer. **Verdict: GAP-TO-FILL (S) — a real correctness bug in the rich-Sentry work just landed.**

5. **No system tray for a long-lived background-capture app + `devtools` ships in release.** (a) The tray is the strongest *product* gap: the backend already broadcasts a continuous `PIPELINE_STATUS_EVENT` stream and exposes clean `start/stop_capture`/`transcribe` IPC — a tray recording-indicator + quick stop + hide-to-tray would be a thin consumer of state that already exists, and for a privacy tool an always-visible "recording now" glyph is a consent affordance. Today capture can only be seen/stopped with the main window focused. (b) `tauri = { features = ["devtools"] }` is unconditional, so release bundles ship the WebView inspector on a renderer that aggregates live transcript text. **Verdict: tray GAP-TO-FILL (M); devtools hardening GAP-TO-FILL (S).**

**Overall posture:** the app is deliberately, correctly plugin-minimalist. It uses the one plugin it needs (`single-instance`) idiomatically and first-registered, and uses native v2 `invoke` + `emit` + `.manage`/`State` + Vite throughout. The suspicion that it reinvents shell/server is **not borne out** — there is no reinvented dev server, no JS `fetch` to a local server, and shell is a dead dependency (not a reimplementation). The real opportunities are the streaming Channel migration, the updater, the Sentry CSP fix, and the tray.

---

## Current Tauri-Surface Usage Table

| Concern | v2 mechanism available | What AudioGraph does | Verdict |
|---|---|---|---|
| FE→BE calls | `invoke` + `#[tauri::command]` | `invoke` from `@tauri-apps/api/core`, ~94 commands, ~33 non-test call sites (23 in the Zustand store) | ADOPT (in use, idiomatic) |
| BE→FE push | `emit` events / `Channel<T>` | `AppHandle::emit` only (54 raw `.emit` + 41 `emit_or_log` call sites, 31 typed event consts, `emit_or_log` wrapper); **`Channel` = 0 uses** | Events idiomatic; **Channel = GAP** |
| Shared state | `.manage()` + `State<T>` | one `.manage(app_state)`, 48 `State<>` sites, `Arc`-internal | ADOPT (in use, idiomatic) |
| Streaming audio PCM | `Channel` / internal | internal `crossbeam` bounded channels; never emitted | JUSTIFIED-ROLL-OWN (correct) |
| HTTP to providers | `plugin-http` (webview proxy) | `reqwest` in Rust backend | JUSTIFIED-ROLL-OWN |
| WebSocket ASR/S2S | `plugin-websocket` (webview) | `tokio-tungstenite` in Rust backend | JUSTIFIED-ROLL-OWN |
| Logging | `plugin-log` | bespoke `logging/mod.rs` (425 LOC) | JUSTIFIED-ROLL-OWN (see §Logging) |
| Settings/persistence | `plugin-store` / `plugin-fs` | bespoke YAML + atomic-write + redaction + load-status gating | JUSTIFIED-ROLL-OWN |
| Secrets | `plugin-stronghold` | `keyring` (OS keychain) | JUSTIFIED-ROLL-OWN |
| Crash capture | (none official) | `std::panic::set_hook`, id-only Sentry marker | JUSTIFIED-ROLL-OWN |
| Analytics | (none official) | Sentry Rust SDK + scrubbing `before_send`; FE `@sentry/browser` | JUSTIFIED-ROLL-OWN (FE has CSP bug) |
| Open logs folder | `plugin-opener` | `std::process::Command` (explorer/open/xdg-open), fixed path | JUSTIFIED-ROLL-OWN (borderline) |
| Single instance | `plugin-single-instance` | registered first; re-focus main window | ADOPT (in use, idiomatic) |
| Vite integration | v2 Vite template | near-verbatim canonical template | ADOPT (in use, idiomatic) |
| Updater | `plugin-updater` | **absent** (+ dead signing env in CI) | GAP-TO-FILL |
| Tray / menu | `TrayIconBuilder` / `Menu` | **absent** | tray GAP; menu low-priority |
| Window state | `plugin-window-state` | **absent** (static config) | GAP-TO-FILL (low) |
| `convertFileSrc`/asset proto | asset protocol | **not used / not enabled** | NOT-APPLICABLE today (latent) |
| Dialog / clipboard / notification / global-shortcut / autostart / deep-link | respective plugins | **absent** | mostly NOT-APPLICABLE (see §Plugins) |
| `plugin-shell` | `plugin-shell` | declared in `package.json`, **0 imports, no Rust crate** | dead dep — remove |

---

## Domain 1 — Plugins & Core Built-ins

**What v2 offers:** 31 official plugins + 9 `core:*` sub-plugins. **What we do:** exactly one plugin registered (`single-instance`); everything else is either native `core:*` (event/window/path/app via `core:default`) or a deliberate roll-your-own.

Per-item verdicts (covering every meaningful plugin + core built-in):

| Plugin / built-in | Verdict | Reason tied to THIS app |
|---|---|---|
| `single-instance` | **ADOPT (done)** | Correct, first-registered. Fixes BUG-3: 2nd launch fails WebView2 with `0x800700AA` on the user-data-dir lock. |
| `core:event` | **ADOPT (done)** | Load-bearing — the entire real-time event bus. One ACL check per listener, not per event, so tight grant costs nothing per-emit. |
| `core:window` / `core:app` / `core:path` / `core:webview` | **ADOPT (done)** | Read-only introspection via `core:default`; no window-mutation granted from JS. Correct least-privilege. |
| `updater` | **GAP-TO-FILL** | RC app, 4 bundle targets, no update path; CI has dead signing env (finding 2). |
| `plugin-log` | **JUSTIFIED-ROLL-OWN** | See Logging Verdict. |
| `store` / `fs` | **JUSTIFIED-ROLL-OWN** | Bespoke needs schema migration, human-editable YAML, atomic writes, credential redaction, corruption-safe load-status gating, legacy import — `plugin-store` gives a JSON blob with none of that. FE-side `fs` is also unneeded (frontend is a pure view). |
| `http` | **JUSTIFIED-ROLL-OWN** | `plugin-http` is a *webview* fetch proxy; all provider I/O is Rust-side `reqwest` (SSE streaming, multipart audio, AWS/GCP SDKs). Wrong layer. Also keeps API keys out of the renderer. |
| `websocket` | **JUSTIFIED-ROLL-OWN** | Same — `tokio-tungstenite` in Rust for Gemini Live / streaming ASR. |
| `stronghold` | **JUSTIFIED-ROLL-OWN** | `keyring` (OS keychain) is the standard choice; no plugin equivalent needed. |
| `opener` | **JUSTIFIED-ROLL-OWN (borderline)** | `open_logs_dir` hand-rolls 3 platform `Command` arms for a fixed, program-derived path. Works, minimal surface, but `plugin-opener` would delete the `cfg` branches. Low-priority tidy. |
| `dialog` | **NOT-APPLICABLE (design)** | Exports use in-webview `<a download>`+Blob (`utils/download.ts`); no user-chosen save path today. Adopt only if "Save As…" is requested. |
| `notification` | **NOT-APPLICABLE** | In-app toast host (ADR-0011) suits an always-foreground app. |
| `global-shortcut` | **NOT-APPLICABLE** | In-window `useKeyboardShortcuts` (keydown) is the correct scope; global system hotkeys not needed. |
| `window-state` / `positioner` | **GAP-TO-FILL (low)** | Window geometry not persisted across launches; minor UX. |
| `clipboard-manager` | **NOT-APPLICABLE** | No copy feature exists yet. |
| `process` | **GAP-TO-FILL (low, paired)** | Needed only as the companion to `updater` for `relaunch()`. |
| `autostart` | **NOT-APPLICABLE** | Launch-at-login would be surprising for a manual meeting tool. |
| `deep-link` | **GAP-TO-FILL (low)** | Plausible near-term "join-link → open in AudioGraph"; not shipped. |
| `os` | **NOT-APPLICABLE** | Platform branching is Rust-side (`cfg`); frontend doesn't need it. |
| `sql` | **NOT-APPLICABLE** | Event-sourced append-only JSONL + derived snapshots is the chosen storage model; SQLite would be a re-architecture, not a drop-in. |
| `upload` / `persisted-scope` / `localhost` | **NOT-APPLICABLE** | No webview uploads; no runtime-granted scopes; localhost server explicitly discouraged by docs. |
| `stronghold`/mobile plugins (barcode/biometric/nfc/geo/haptics) | **NOT-APPLICABLE** | Desktop-only app. |
| `plugin-shell` | **REMOVE (dead dep)** | `package.json:36`, 0 imports, no Rust crate, no `shell:*` capability. Remove for supply-surface hygiene and to stop signalling "shell is wired up." |

---

## Domain 2 — IPC (the highest-value domain)

**What v2 offers:** three primitives — Command (`invoke`, req/resp), Event (`emit`, low-throughput fan-out), Channel (`ipc::Channel<T>`, ordered high-throughput Rust→FE stream). **What we do:** Command + Event + managed State, all idiomatic; **Channel entirely absent**.

- **`invoke` / commands — ADOPT (done).** ~94 commands, well-organized, store-centric boundary.
- **`emit` events — ADOPT for lifecycle/status; GAP on hot paths.** 31 typed event consts, `emit_or_log` hygiene wrapper, single-window so global `emit` is correct. But it is the wrong tool on the streaming paths:
  - **`chat-token-delta` — GAP-TO-FILL (prime `Channel` candidate).** One emit per token, 20–100+/sec; `request_id` threaded through every payload = manual reimplementation of Channel scoping. FE 33ms coalescer cuts renders, not IPC cost.
  - **`asr-partial` + `asr-span-revision` — GAP-TO-FILL.** *Double-emit* per partial (2 serializations + 2 bridge crossings); `asr-partial` is throttled FE-side (100ms latest-wins) but `asr-span-revision` is **not** (cumulative, can't drop) so the unthrottled half fires at full provider rate (3–10/sec/source cloud).
  - **S2S (`gemini-transcription`, `openai-realtime-response`) — GAP-TO-FILL.** Emit-per-chunk, no FE throttle on `gemini-transcription`.
- **Audio PCM off IPC — JUSTIFIED-ROLL-OWN (correct).** ~31 chunks/sec/source stays on internal `crossbeam` bounded channels with drop-on-full; only low-frequency health/error events reach the FE. Exactly right.
- **Model-download throttle — the tell.** `ProgressThrottle` at 1s proves the team knows backend rate-control; it just wasn't applied to token/partial streams (where `Channel` is the canonical fix). No ACL change is needed to adopt `Channel` — channels are function-return handles, not permissioned event names.
- **State — ADOPT (done).** One `.manage`, 48 `State<>` sites, `Arc`-internal.

---

## Domain 3 — Windowing / Menu / Tray

**What v2 offers:** multi-window, `WindowBuilder`, native `Menu`, `TrayIconBuilder`, window-state plugin, effects, custom titlebar, `CloseRequested` intercept. **What we do:** one static window, one `get_webview_window("main")` (the single-instance re-focus), no builder, no menu, no tray.

- **Single window — JUSTIFIED-ROLL-OWN / idiomatic-minimal.** Correct for a single-surface SPA; multi-window would add IPC/state-sync cost for no payoff.
- **Tray — GAP-TO-FILL (strongest product gap).** Backend already emits continuous `PIPELINE_STATUS_EVENT` + `CAPTURE_ERROR`/`CAPTURE_STORAGE_FULL`/`AUDIO_CONSUMER_HEALTH`, and exposes clean `start_capture`/`stop_capture`/`start_transcribe`/`stop_transcribe`. A tray = thin consumer of an existing stream; needed because capture is invisible/uncontrollable when the window isn't focused, and a "recording now" glyph is a privacy/consent affordance. Pairs with a `WindowEvent::CloseRequested`→hide-to-tray intercept. Requires the `tray-icon` Cargo feature + `core:tray`/`core:menu` (already in `core:default`).
- **Native menu — GAP-TO-FILL (low, mostly macOS).** No About/Preferences/Quit/Edit menu; on macOS the native Edit menu makes clipboard accelerators reliable in the WebView. Lower priority than tray.
- **Window label — hygiene.** Config omits `"label"`; Tauri defaults to `"main"`, which the single-instance callback assumes. Pin `"label": "main"` so a future edit can't silently break re-focus.

---

## Domain 4 — Security / Capabilities / CSP

**What v2 offers:** capability→permission→scope ACL, CSP, isolation pattern, `freezePrototype`, asset-protocol scoping. **What we do:** one `default` capability = `core:default` → `main`; tight CSP; no shell/fs/http/dialog plugins to misconfigure.

- **Capability scoping — ADOPT (done, exemplary).** One capability, `main` window, `core:default`; unlabeled window resolves to `main` and matches. No `remote`, no `dangerousRemoteDomainIpcAccess`, no over-grant. `core:default` is slightly broad (read-only introspection) but benign and conventional — no fs/shell/http/dialog/clipboard/window-mutation. The prompt's premise that `default.json` includes "whatever shell needs" is **incorrect** — there is no shell permission and no shell plugin.
- **`connect-src` vs provider hosts — correct to omit.** All provider egress is Rust-side; the webview issues zero cross-origin requests. Adding provider hosts would *widen* webview egress for no reason.
- **`connect-src` vs Sentry — GAP-TO-FILL (HIGH, finding 4).** `@sentry/browser` default transport POSTs to `*.ingest.us.sentry.io`, absent from `connect-src` → frontend analytics silently blocked. Prior 2026-05-29 review praised the CSP but predates the `@sentry/browser` addition. Fix: add `https://*.ingest.us.sentry.io` to `connect-src`, OR (cleaner, keeps egress in Rust + reuses the Rust scrubber) relay frontend errors through an `invoke` to the Rust Sentry and drop the browser SDK. Add a test tying `connect-src` to the initialized DSN host.
- **`devtools` in release — GAP-TO-FILL (S, hardening, finding 5b).** `features = ["devtools"]` unconditional + `core:webview:allow-internal-toggle-devtools` ⇒ shipped builds can open the inspector on a renderer holding live transcript text. Gate behind `cfg(debug_assertions)`/dev profile.
- **`style-src 'unsafe-inline'` / `img-src data:` — INFO.** Appear unused by the shipped bundle (no inline style/script, no `data:` images). Over-broad in the safe direction; optional to tighten.
- **Isolation pattern — NOT-ADOPTED, defensible.** Docs recommend it when the frontend bundles third-party npm deps. Given the tight capability set (no fs/shell/http exposed to the webview) the marginal value is lower here; reasonable to skip, but it is the canonical place to enforce "analytics never carries transcript text" if that guarantee is wanted at the IPC boundary rather than only in `before_send`.
- **`open_logs_dir` raw spawn — JUSTIFIED-ROLL-OWN.** Fixed, program-derived path (no webview input) ⇒ no argument-injection surface. Document the invariant so a future edit doesn't turn it into an `open` sink.

---

## Domain 5 — Lifecycle / Setup / Updater / Process

**What v2 offers:** `setup` hook, `RunEvent` (Ready/ExitRequested/Exit/…), `on_window_event`, updater, process, autostart, deep-link, window-state, graceful `cleanup_before_exit`. **What we do:** idiomatic `setup`; a `RunEvent::Exit`-only closure; a coherent crash-recovery-forward scheme.

- **`setup` + single-instance — ADOPT (done, idiomatic).** Crash hook installed *before* the builder to catch startup panics; single-instance first-registered.
- **Crash-detection-on-next-launch — JUSTIFIED-ROLL-OWN.** `register_session` marks prior "active" sessions "crashed"; `finalize_session` marks "complete"; append-only JSONL + 30s autosave mean most state survives a hard kill. Good for a crash-heavy native-ML app.
- **Graceful shutdown — GAP-TO-FILL (M).** `AppState` holds 11 `JoinHandle` slots (24 `thread::spawn` + 64 `tokio::spawn` sites); `RunEvent::Exit` joins/stops **none**. The autosave daemon is an unsignalled `loop { sleep(30s) }` ⇒ up to ~30s of derived-graph loss on clean quit; `TranscriptWriter::shutdown_with_timeout` exists but is called only on rotation, never at quit, so a clean File→Quit gets no writer flush and no audio-device release. Primitives exist; the Exit hook wires up none. Handle in `RunEvent::Exit`/`ExitRequested`.
- **Updater — GAP-TO-FILL (M, finding 2).** Absent + dead `TAURI_SIGNING_PRIVATE_KEY` in `release.yml`. Idiomatic path: add `plugin-updater` + `plugin-process`, `plugins.updater { endpoints, pubkey }`, `updater:default` capability, `createUpdaterArtifacts: true`. OS code-signing is a separate, documented gap (`signingIdentity: null`). The M rating covers wiring the plugin; note the updater's own signing keypair (`TAURI_SIGNING_PRIVATE_KEY` / `pubkey`) is separate from OS code-signing. But OS code-signing/notarization is a **prerequisite** for a working macOS update path — `tauri.conf.json` has `signingIdentity: null` and no notarization, so on unsigned bundles simply "adding the plugin" does **not** yield working updates; the signing/notarization work must land first (or in tandem) for macOS.
- **Analytics flush at exit — GAP-TO-FILL (S).** Sentry guard in `static OnceLock` — no `Drop` at termination; no `Client::flush` in `Exit`. Add a bounded flush.
- **Autostart / deep-link — NOT-APPLICABLE / low.** As above.

---

## Domain 6 — Assets / Frontend Integration

**What v2 offers:** Vite template, `convertFileSrc` + asset protocol, bundled resources, `@tauri-apps/api` surface. **What we do:** near-verbatim canonical Vite config; minimal API surface (`core` + `event` only).

- **Vite + `tauri.conf.json` build block — ADOPT (done, idiomatic).** `clearScreen:false`, `strictPort`, `TAURI_DEV_HOST` host/HMR, `watch.ignored src-tauri`, `frontendDist ../dist`, `devUrl :1420` matching Vite. Justified extras: `react-vendor` manual chunk, opt-in `ANALYZE=1` visualizer. **Confirms the frame note: native Vite + `invoke` are correctly used; nothing reinvents the dev server.**
- **No JS `fetch` to a local server — clean.** Zero `fetch()` in non-test `src/`; localhost strings are LLM provider base URLs passed *as args* to Rust commands. **Confirms the frame note: no reinvented server.**
- **`convertFileSrc` / asset protocol — NOT-APPLICABLE (latent).** Not used, not enabled; frontend is a pure view over Rust-owned state, so nothing needs a file URL today. If session-audio playback or screenshot preview is ever added, `convertFileSrc` + an `assetProtocol` scope is the idiomatic path (not base64-over-`invoke`).
- **`utils/download.ts` `<a download>`+Blob — JUSTIFIED-ROLL-OWN.** Avoids `plugin-dialog`/`plugin-fs` + capabilities for simple exports; native save-as is the upgrade if UX demands it.
- **`safeInvoke` — DEAD CODE (verified).** Defined in `src/analytics/safeInvoke.ts` but **0 call sites** (grep confirms only the definition file references it; the plugins/ipc audits' higher counts included the definition). The per-command failure diagnostics it was built for are therefore not wired in — directly compounding finding 4. Either adopt it at call sites (and fix the CSP) or delete it.
- **`@tauri-apps/plugin-shell` — dead dep (remove).**

---

## The Sentry Question — Could v2 built-ins have simplified the rich-Sentry work?

**Short answer: partially on the frontend, no on the backend. The bespoke `capture_diagnostic` was the right call; `tauri-plugin-log` + a Sentry log bridge would NOT have replaced it, but it could have improved the frontend seam that is currently broken.**

Detail:

- **`capture_diagnostic` is a structured, typed capture path, not a logging shim.** `DiagEvent` carries only enums/controlled-ids/numbers (`name`, `Category`, `level`, `provider`, `kind`, `http_status`, `recoverable`) — there is *physically no free-text field*, and everything still passes an 8-key tag allowlist + id-shape gate (`^[a-z0-9._:-]{1,48}$`) + `scrub_event` before send. For a privacy-first transcription tool this "prose cannot ride in" property is the load-bearing design. A `tauri-plugin-log` → Sentry bridge routes **strings** (log messages), which is the *opposite* of what this app wants — it would reintroduce exactly the free-text leak surface the typed API eliminates. So on the diagnostic path, `capture_diagnostic` is **strictly better than a log bridge**, not a reinvention of one. **Verdict: JUSTIFIED-ROLL-OWN.**

- **Where a built-in *would* have helped: the frontend.** `tauri-plugin-log`'s `attachLogger(fn)` is the documented hook for funnelling JS-side log/error events into a custom sink (e.g. a Sentry breadcrumb/relay). AudioGraph instead runs a *second* Sentry client in the webview (`@sentry/browser`) with its own DSN and its own `scrubEvent` — which (a) duplicates the scrubber, and (b) is currently **dead on arrival** because its egress is CSP-blocked (finding 4). The v2-idiomatic, lower-surface design is: **no browser Sentry SDK at all** — forward frontend errors through `invoke` (or `plugin-log` + an `attachLogger` relay) to the *existing* Rust Sentry, which already has the transport, the scrubber, and is not CSP-constrained. That would have (i) avoided the CSP bug entirely, (ii) removed the duplicate DSN + duplicate scrubber, and (iii) made `safeInvoke`'s intended per-command instrumentation actually deliver. So: the backend rich-Sentry work is well-architected and *not* simplifiable by v2 built-ins; the frontend half is where a built-in (or a plain `invoke` relay) would have been simpler *and* correct.

- **`tauri-plugin-log` for the base logging tee?** It could cover stderr+file+level, but not the audio-backend per-target noise cap (`wasapi`/`cpal`/`reqwest` forced to WARN — ~99% of log volume was one `wasapi` line) or the runtime file-mode/purge-for-UI surface. So the logging module stays JUSTIFIED-ROLL-OWN; the Sentry angle doesn't change that.

---

## Prioritized Adopt / Fill Roadmap (with rough effort)

| # | Item | Verdict | Effort | Why now |
|---|---|---|---|---|
| 1 | Add `https://*.ingest.us.sentry.io` to CSP `connect-src` (or relay FE errors via `invoke` to Rust Sentry) + regression test | GAP-TO-FILL | **S** | The rich-Sentry work is silently non-functional on the frontend; small change, high correctness value. |
| 2 | Migrate `chat-token-delta` (then `asr-*`, S2S) from `emit` to `ipc::Channel<T>` returned by the streaming commands | GAP-TO-FILL | **M** | Highest technical value; removes per-item serialize + event-router cost on the hottest paths; no ACL change needed. |
| 3 | Add `tauri-plugin-updater` + `plugin-process`, `plugins.updater` config, `updater:default`, `createUpdaterArtifacts`; remove/activate the dead CI signing env | GAP-TO-FILL | **M** | RC app with no update path + misleading CI config; blocks safe shipping. |
| 4 | Wire real teardown into `RunEvent::Exit`/`ExitRequested`: signal autosave stop + final save, call `TranscriptWriter::shutdown_with_timeout`, flip capture/transcribe flags, `Sentry::flush` | GAP-TO-FILL | **M** | Clean quit currently == kill: ~30s graph loss, no writer flush, no device release. Primitives already exist. |
| 5 | Add `TrayIconBuilder` recording-indicator + start/stop/show menu + `CloseRequested`→hide-to-tray (`tray-icon` feature) | GAP-TO-FILL | **M** | Strongest product gap for a background-capture privacy tool; consumes existing `PIPELINE_STATUS_EVENT`/`CAPTURE_*` streams. |
| 6 | Gate `devtools` behind `cfg(debug_assertions)`/dev profile | GAP-TO-FILL | **S** | Stop shipping the inspector on a live-transcript renderer. |
| 7 | Remove dead `@tauri-apps/plugin-shell` dep; delete-or-adopt `safeInvoke` | cleanup | **S** | Supply-surface + honesty hygiene; `safeInvoke` decision is coupled to #1. |
| 8 | Pin `"label": "main"` in window config | hygiene | **S** | Prevents silent single-instance re-focus breakage on a future config edit. |
| 9 | Replace `open_logs_dir` raw spawn with `plugin-opener` (optional) | JUSTIFIED-ROLL-OWN today | **S** | Only if touching that code; deletes 3 platform `cfg` arms. |
| 10 | `plugin-window-state` for geometry persistence (optional) | GAP-TO-FILL (low) | **S** | Minor UX nicety. |

**Not on the roadmap (correctly skipped):** `plugin-http`/`websocket` (Rust owns provider I/O), `plugin-store`/`fs` (bespoke YAML is superior for redaction/corruption-safety), `plugin-sql` (event-sourcing is the storage model), `plugin-stronghold` (`keyring` suffices), `notification`/`global-shortcut`/`autostart`/`clipboard`/mobile plugins (not applicable), isolation pattern (low marginal value given the tight capability set), `convertFileSrc` (no file-URL feature yet).
