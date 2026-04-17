# Gap Analysis: What Prior Reviews Missed

**Date:** 2026-04-16

## Executive Summary

Prior reviews covered: rsac architecture, code quality, security, performance,
UX. This targeted review looked at observability, release engineering,
dependency health, test coverage, accessibility, i18n, error recovery,
telemetry, and dead code.

**Counts:** 5 CRITICAL, 14 HIGH, 15 MEDIUM, 5 LOW.

## Top Findings

### CRITICAL

1. **No macOS notarization / code signing in CI** — Builds fail Gatekeeper.
2. **No Windows code signing** — Defender flags builds as suspicious.
3. **No version bumping or release script** — Stuck at `0.1.0`, no semver discipline.
4. **No frontend tests** — Zero `.test.ts` / `.spec.tsx` / Vitest config.
5. **No release build artifacts in CI** — DMG, EXE, AppImage, deb all missing.

### HIGH

6. No auto-reconnect for Gemini / Deepgram / AssemblyAI WebSockets.
7. AWS credential expiry not handled (session tokens have 1hr TTL).
8. No keyboard navigation.
9. Minimal ARIA labels (WCAG 2.1 Level A violations).
10. UI text hardcoded in English (no i18n framework).
11. No CONTRIBUTING.md for audio-graph (only rsac).
12. No crash handler or panic dump.
13. No error reporting mechanism (no "Send Report" button).
14. Errors are free-form strings (no error code catalog).
15. Changelog not automated.
16. Credential loading silently swallows errors (`unwrap_or_default()`).
17. Speech processor orchestration untested (2000+ LOC).
18. Gemini reconnection logic not tested.
19. Test coverage unknown (no tarpaulin/llvm-cov in CI).

### MEDIUM

- No Prometheus / OpenTelemetry metrics.
- Log verbosity not runtime-configurable.
- UI lacks detailed pipeline diagnostics (p99, buffer fill %).
- No `cargo audit` in CI.
- Many deps are pre-1.0 (`llama-cpp-2 = "0.1.139"`, `mistralrs = "0.8"`).
- Color contrast not validated.
- Gemini session resumption code never called (`#[allow(dead_code)]`).
- Token usage tracking incomplete (TODO).
- `config/default.toml` loader stub (TODO I6).
- Credentials plaintext on disk (zeroize is in-memory only).
- No HTTPS cert pinning for WebSocket TLS.
- ASR language picker UI missing.
- Gemini not documented for multi-language.
- Disk full during transcript persistence not handled.
- 9 `#[allow(dead_code)]` instances suggesting incomplete features.

### LOW

- No property-based tests (`proptest`, `quickcheck`).
- Inline panics in tests could cover production path bugs.

## Recommendations by Phase

### Phase 1: Critical (1-2 weeks)
1. Add macOS notarization + Windows code signing to CI
2. Create release script with semver bumping + changelog automation
3. Emit downloadable artifacts (DMG, MSI, AppImage, deb)
4. Scaffold frontend tests (Vitest + React Testing Library)

### Phase 2: High (2-4 weeks)
5. Auto-reconnect logic for cloud ASR providers
6. AWS credential refresh mid-stream
7. Structured error codes (enum-based rather than strings)
8. Accessibility: ARIA labels + keyboard nav
9. i18n framework (react-i18next)

### Phase 3: Medium (ongoing)
10. Crash reporting (Sentry or Tauri-compatible alternative)
11. `cargo audit` in CI
12. Document credential expiry + recovery flows
13. Resolve dead code (`session_id`, config stub, token tracking)
14. Encrypted credential storage (OS keychain integration)
