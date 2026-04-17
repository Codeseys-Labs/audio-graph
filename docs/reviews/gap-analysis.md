# Gap Analysis: What Prior Reviews Missed

**Originally authored:** 2026-04-16
**Last refreshed:** 2026-04-16 (after loops 1–6)

## Executive Summary

Prior reviews covered: rsac architecture, code quality, security, performance,
UX. This targeted review looked at observability, release engineering,
dependency health, test coverage, accessibility, i18n, error recovery,
telemetry, and dead code.

**Original counts:** 5 CRITICAL, 14 HIGH, 15 MEDIUM, 5 LOW.

**Status after shipped work:** many originally-listed items have been
resolved across the multi-phase loop sessions. See the annotated list
below — ✅ resolved, 🚧 partial, ⏳ open.

## Top Findings

### CRITICAL

1. 🚧 **No macOS notarization / code signing in CI** — `release.yml`
   workflow passes `APPLE_*` secrets to tauri-action; signing happens
   automatically if the repo secrets are populated. Still need to procure
   an Apple Developer ID + populate secrets for actual signing.
2. 🚧 **No Windows code signing** — same as above; `WINDOWS_CERTIFICATE`
   + `WINDOWS_CERTIFICATE_PASSWORD` wired, awaiting cert procurement.
3. ✅ **No version bumping or release script** — `scripts/bump-version.sh`
   bumps the three version locations atomically (package.json,
   src-tauri/Cargo.toml, src-tauri/tauri.conf.json) and rotates
   CHANGELOG.md. See `docs/RELEASE.md` for the full workflow.
4. ✅ **No frontend tests** — Vitest + React Testing Library scaffolded;
   12 passing tests across `download`, `format`, and `store` modules.
5. ✅ **No release build artifacts in CI** — `.github/workflows/release.yml`
   fires on `v*` tag push, uses tauri-action@v0.6.2 to build macOS DMG
   (universal), Windows MSI+NSIS, Linux AppImage+deb in parallel and
   drafts a GitHub Release with all artifacts attached.

### HIGH

6. ✅ **Auto-reconnect for Gemini / Deepgram / AssemblyAI WebSockets** —
   `session_task` + `open_ws` + exponential backoff + `DisconnectKind`
   + bounded audio-chunk backlog (200 chunks ≈ 10 s) across all three
   providers. Gemini replays `BidiGenerateContentSetup` on reconnect.
7. ✅ **AWS credential expiry handled** — STS `GetCallerIdentity` pre-flight
   + mid-stream refresh via `YamlRefreshingCredentialsProvider` in
   `src/aws_util/mod.rs`. When the user is in `access_keys` mode with a
   session token, the SDK re-reads `credentials.yaml` on each credential
   request so rotated STS creds take effect without restarting the session.
8. 🚧 **Keyboard navigation** — four global shortcuts wired via
   `useKeyboardShortcuts`: Cmd/Ctrl+R toggle capture, Cmd/Ctrl+, open
   settings, Cmd/Ctrl+Shift+S open sessions, Escape close modals. Broader
   navigation (focus-ring polish, arrow-key list navigation) not done.
9. 🚧 **ARIA labels** — dialog roles + `aria-modal` + `aria-labelledby`
   on SettingsPage and SessionsBrowser; `aria-pressed` on capture /
   transcribe / Gemini toggles; `aria-live="polite"` on the elapsed
   timer and backpressure pill; focus trap via `useFocusTrap` on both
   modals. Full WCAG 2.1 Level A audit not yet run against the whole
   surface.
10. 🚧 **i18n framework** — react-i18next + i18next-browser-languagedetector
    scaffolded. `src/i18n/locales/{en,pt}.json` hold ~13 keys under
    `controlBar.*`, `settings.*`, `sessions.*`, `common.*`. Initial
    component strings (ControlBar start/stop/sessions/settings, modal
    headers and close buttons) wrapped in `t(...)`. Bulk of form labels
    still hard-coded — future work.
11. ✅ **CONTRIBUTING.md for audio-graph** — `docs/CONTRIBUTING.md` covers
    prereqs, rsac path-dep mechanics, repo layout, dev-mode gate commands,
    CI overview, and a "How do I…" FAQ for common tasks. Paired with
    `docs/GEMINI_LANGUAGES.md` documenting multi-language Gemini Live.
12. ✅ **Crash handler / panic dump** — `crash_handler::install()` runs
    before anything else in `run()`. Chains via `take_hook()` so the
    default stderr prints still fire; writes a structured report
    (timestamp, version, OS, thread, location, payload, backtrace) to
    `~/.audiograph/crashes/<unix_ms>.log` best-effort. Hand-rolled ISO
    8601 formatter to avoid a chrono dependency.
13. ⏳ **No error reporting mechanism (no "Send Report" button).**
14. ⏳ **Errors are free-form strings (no error code catalog).** Structured
    error variants are in place for a few hotspots (WebSocket reconnect
    disconnect kinds) but user-facing error surface remains string-based.
15. ⏳ **Changelog not automated.**
16. ✅ **Credential loading silently swallows errors** — `try_load_credentials`
    returns `Result`; `diagnose_credentials` Tauri command exposes parse /
    IO errors to the UI.
17. ⏳ **Speech processor orchestration untested (2000+ LOC).** Still only
    module-scoped unit tests; no integration test spinning the full tail.
18. ⏳ **Gemini reconnection logic not tested.** Covered by manual smoke
    tests only; no programmatic reconnect scenario test yet.
19. ⏳ **Test coverage unknown (no tarpaulin/llvm-cov in CI).**

### MEDIUM

- ⏳ No Prometheus / OpenTelemetry metrics.
- ✅ **Log verbosity runtime-configurable** — `src-tauri/src/logging/`
  module + `set_log_level` Tauri command + "Diagnostics" section in
  SettingsPage. Persisted to settings.json, applied on startup after
  `env_logger::init()` (so `RUST_LOG` still wins when explicitly set).
- ⏳ UI lacks detailed pipeline diagnostics (p99, buffer fill %).
- ✅ **`cargo audit` in CI** — hard gate on audio-graph with a justified
  ignore list in `src-tauri/.cargo/audit.toml`. Re-assess whenever the AWS
  SDK rustls chain moves.
- ⏳ Many deps are pre-1.0 (`llama-cpp-2 = "0.1.139"`, `mistralrs = "0.8"`).
- ⏳ Color contrast not validated.
- ⏳ Gemini session resumption code never called (`#[allow(dead_code)]`).
- ⏳ Token usage tracking incomplete (TODO).
- ⏳ `config/default.toml` loader stub (TODO I6).
- ⏳ Credentials plaintext on disk (zeroize is in-memory only). File perms
  are 0600 via `fs_util::set_owner_only` but the file is not encrypted.
- ⏳ No HTTPS cert pinning for WebSocket TLS.
- ⏳ ASR language picker UI missing.
- ⏳ Gemini not documented for multi-language.
- ✅ **Disk-full handled during transcript + graph persistence** — new
  `CAPTURE_STORAGE_FULL` Tauri event + `is_storage_full()` classifier
  (checks `ErrorKind::StorageFull` plus ENOSPC=28 / ERROR_DISK_FULL=112
  raw OS codes defensively). `persistence::io::write_or_emit_storage_full`
  wraps the transcript writer thread and graph autosave `save_json` path;
  emits at most once per writer instance (latched) to avoid flooding the
  frontend. useTauriEvents subscribes and surfaces a user-facing error
  with bytes-lost count.
- 🚧 `#[allow(dead_code)]` instances — reduced: `is_under_backpressure`
  now has a documented reason (public observability API) and
  `credential_store` gained real callers via the new Settings UI delete
  button. A handful still remain.

### LOW

- ⏳ No property-based tests (`proptest`, `quickcheck`).
- ⏳ Inline panics in tests could cover production path bugs.

## Items Discovered and Resolved Outside the Original Scan

These weren't in the original review but came up during the loops:

- ✅ **Path traversal in `load_session_transcript` / `delete_session`** —
  allowlist validation via `validate_session_id` (alphanumeric + `-_`).
- ✅ **`sessions.json` concurrent-write race** — process-local `Mutex<()>`
  serializes register / update_stats / finalize / delete.
- ✅ **Audio backlog memory growth during WebSocket reconnect** — bounded
  at 200 chunks (~10 s) via `AtomicUsize` depth counter. See the
  `bounded-backlog-unbounded-channel` skill for the pattern.
- ✅ **`set_credential` silently overwrote saved secrets with empty form
  fields** — now treats empty as no-op; `delete_credential` is the
  explicit clear path, surfaced via a "Clear Saved AWS Keys" button.
- ✅ **Test Connection buttons could hang indefinitely** — all 5 now
  debounce and time out after 10 s via `Promise.race`.
- ✅ **AWS DefaultChain / Profile creds had no pre-flight** — STS
  `GetCallerIdentity` probe with 5 s timeout before start_transcribe.
- ✅ **AudioGraph CI failed on every commit standalone** — `rsac` was a
  relative path dep unresolvable outside the dev layout. CI now stages
  the parent repo around the audio-graph checkout.
- ✅ **Parent CI never built audio-graph** — new `Downstream (audio-graph)`
  job in parent CI catches rsac API breakage at PR time rather than
  submodule-bump time.
- ✅ **`rust-toolchain@stable` silently broke CI on clippy 1.95 lint
  bumps** — pinned to 1.95.0 in `rust-toolchain.toml` (both crates)
  plus `dtolnay/rust-toolchain@1.95.0` in all workflows. Bumps are now
  deliberate PRs.
- ✅ **Rustls-webpki advisories (RUSTSEC-2026-0098/0099)** — triaged as
  transitive via AWS SDK's rustls 0.21 pin, exposure nil; documented
  acceptance in `.cargo/audit.toml` with upgrade trigger noted.

## Recommendations by Phase

### Phase 1: Critical (remaining work)
1. ⏳ Procure Apple Developer ID + Windows Authenticode cert; populate
   the `APPLE_*` / `WINDOWS_*` GitHub secrets documented in
   `docs/RELEASE.md`. The signing plumbing in `release.yml` is in place
   and waiting.

### Phase 2: High (remaining work)
4. ⏳ AWS credential refresh mid-stream (not just pre-flight).
5. ⏳ Structured error codes (enum-based) across the user-facing surface.
6. ⏳ Accessibility: ARIA labels + keyboard nav.
7. ⏳ i18n framework (react-i18next).
8. ⏳ Integration tests for speech processor orchestration.
9. ⏳ Gemini reconnect scenario test (test double for WebSocket).
10. ⏳ Test coverage reporter in CI (tarpaulin / llvm-cov).

### Phase 3: Medium (ongoing)
11. ⏳ Crash reporting (Sentry or Tauri-compatible alternative).
12. ⏳ Encrypted credential storage (OS keychain integration for at-rest
    protection, keeping credentials.yaml for cross-machine export).
13. ⏳ Resolve remaining `#[allow(dead_code)]` instances.
14. ⏳ Wire `rsac::BridgeStream::is_under_backpressure` into audio-graph's
    per-chunk speech processor loop for adaptive throttling.
