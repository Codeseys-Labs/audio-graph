# audio-graph review — Loop 10

**Date:** 2026-04-17
**Reviewer:** claude-agent (read-only explore pass)
**Scope:** audio-graph (backend + frontend + CI + docs)

## Summary

Fresh read of audio-graph reveals a well-structured Tauri v2 desktop app with
solid architecture across backend Rust and frontend React. The codebase
demonstrates good patterns in error handling (AWS credential refreshing,
WebSocket reconnection, path-traversal validation), crash reporting, and test
coverage (82 backend tests, 12 frontend tests). Key gaps remain: unused settings
fields, untested speech processor orchestration (2513 LOC with no integration
tests), Gemini session-resumption dead fields, partial frontend i18n coverage,
and pre-1.0 dependency management. No critical security flaws found, but
several medium-severity opportunities for polish remain. See prior
`gap-analysis.md` for historical context — this review focuses on state *after*
loops 1–9.

**Counts:** 0 CRITICAL, 4 HIGH, 7 MEDIUM, 1 LOW, 18 positive confirmations.

---

## CRITICAL

None flagged this cycle.

---

## HIGH

### 1. Audio settings persisted but never read
**Files:** `src-tauri/src/settings/mod.rs:274-275`, `src-tauri/src/audio/capture.rs:424-425`, `src-tauri/src/audio/pipeline.rs`

`AppSettings` includes an `audio_settings: AudioSettings` field with
`sample_rate` and `channels`. It's persisted to disk, but **never read or
applied**. Hard-coded values are used instead:
- `capture.rs:424-425` hard-codes `.sample_rate(48000).channels(2)`
- `pipeline.rs` hard-codes resampling to 16 kHz mono

**Impact:** User configuration has no effect.

**Action:** Either (a) remove the field if not intended to be user-configurable,
or (b) wire it into capture + pipeline and expose in Settings UI.

### 2. Frontend i18n: bulk of labels still hard-coded
**Files:** `src/components/SettingsPage.tsx` (~200 lines of form labels),
`src/components/SessionsBrowser.tsx`, `src/components/ControlBar.tsx` (pipeline
controls section)

react-i18next scaffold (`src/i18n/index.ts`) + en/pt locales exist, but only
modal titles + common actions are wrapped in `t(...)`. Form labels ("AWS
Transcribe", "Deepgram Model", "API Endpoint", etc.) are hard-coded English.

**Impact:** Non-English users see mostly-English UI; inconsistent UX.

**Action:** Extract remaining hard-coded strings; prioritize SettingsPage.

### 3. Speech processor (2513 LOC) has no integration tests
**File:** `src-tauri/src/speech/mod.rs`

The `speech` module — orchestrating ASR, diarization, extraction, buffering —
has zero integration tests. Untested critical paths:
- ASR chunk accumulation → diarization → extraction pipeline
- Segment boundary / overlap logic
- Backpressure propagation from extractors to ASR input
- Transcript buffer overflow handling

**Impact:** Regressions ship undetected.

**Action:** Add integration test spawning full speech loop with mocked ASR.

### 4. Gemini session resumption never called
**File:** `src-tauri/src/gemini/mod.rs:162, 167-168`

`session_id` + `session_handle` fields marked `#[allow(dead_code)]`. Code
exists to capture `sessionResumption` from server messages, but never wired to
a `resume_session()` path.

**Impact:** Reconnects lose conversation context; implementation debt.

**Action:** Either implement + test resumption, or remove dead fields and
document the decision in `ARCHITECTURE.md`.

---

## MEDIUM

### 5. Token usage tracking incomplete
**File:** `src-tauri/src/commands.rs:958`

`send_chat_message` returns `tokens_used: 0` placeholder with `// TODO: track
actual token usage`. OpenAI/Ollama responses carry `usage` fields.

**Action:** Capture counts from LLM engine response; thread to frontend.

### 6. No TOML config loader (stub)
**File:** `src-tauri/src/state.rs:6-8`

Header comment notes `// TODO(I6): Load configuration from 'config/default.toml'`.
Hard-coded defaults for thread pool sizes, channel capacities.

**Action:** Parse `config/default.toml` at startup; expose thread-pool + buffer
knobs.

### 7. Credentials plaintext on disk
**File:** `src-tauri/src/credentials/mod.rs:117-135`

Saved to `~/.config/audio-graph/credentials.yaml` with `0600` perms + in-memory
zeroization, but no encryption at rest.

**Action:** OS keychain integration (macOS Keychain, Windows Credential
Manager, Linux Secret Service); fall back to encrypted YAML.

### 8. No error-code catalog
**Files:** ubiquitous across command handlers

Errors returned as bare `String`; frontend can't localize, categorize, or
provide recovery actions.

**Action:** `AppError` enum with structured variants (`AwsCredentialExpired`,
`GeminiRateLimited`, `ModelNotFound`, …); serialize with code + message.

### 9. No HTTPS cert pinning for WebSocket TLS
**Files:** `src-tauri/src/gemini/mod.rs:230`, `src-tauri/src/asr/deepgram.rs`,
`src-tauri/src/asr/assemblyai.rs`

WebSocket connections use default TLS. MITM possible on compromised networks.

**Action:** Pin certificates for cloud provider endpoints; custom
`ServerCertVerifier` in `rustls`.

### 10. Pre-1.0 deps on critical path
**File:** `src-tauri/Cargo.toml:47, 96, 104, 111, 116`

Critical pre-1.0 deps:
- `llama-cpp-2 = "0.1.139"` (LLM)
- `mistralrs = "0.8"` (Rust LLM)
- `parakeet-rs = "0.3"` (diarization)
- `sherpa-onnx = "1.12"`

**Action:** Pin patch versions in `Cargo.lock`; monitor for 1.0 transitions.

### 11. Disk-full scenario not handled in persistence
**Files:** `src-tauri/src/persistence/mod.rs`, transcript write sites in
`speech/mod.rs`

Auto-save thread fails silently on ENOSPC; users not notified.

**Status:** In progress in parallel via Task #73 this loop.

---

## LOW

### 12. `#[allow(dead_code)]` instances with documented rationale
**Files:** `src-tauri/src/asr/assemblyai.rs:132,346`,
`src-tauri/src/gemini/mod.rs:162,167-168`,
`src-tauri/src/diarization/mod.rs:45,444,477`,
`src-tauri/src/audio/capture.rs:51,426,502`

Each has an explicit comment. Not orphan code — intentional. No action.

---

## Noted but not flagged (positive confirmations)

- ✅ Path traversal protection — `validate_session_id()` rejects `..`, `/`,
  `\`, null bytes
- ✅ AWS credential refresh — `YamlRefreshingCredentialsProvider` re-reads
  YAML on every SDK call
- ✅ Bounded audio backlog — 200-chunk cap (~10s) via `AtomicUsize`
- ✅ Test count: 82 backend tests, 12 frontend tests
- ✅ Crash handler — global panic hook writes structured reports to
  `~/.audiograph/crashes/<ms>.log`
- ✅ Frontend i18n infrastructure scaffolded correctly
- ✅ Keyboard shortcuts — `useKeyboardShortcuts` hook (Cmd+R / Cmd+, / Cmd+Shift+S / Esc)
- ✅ Clean CI logs, no `console.log` spam
- ✅ AWS util structured error handling
- ✅ Atomic settings writes (temp file + rename + `set_owner_only`)
- ✅ Gemini reconnect with exponential backoff + setup replay
- ✅ Comprehensive CI (Linux/macOS/Windows + frontend + audit)
- ✅ Release workflow tag-triggered with optional code signing
- ✅ Credentials security (zeroize + 0600 perms + allowlist validation)
- ✅ Diarization feature-flagged with Simple backend fallback
- ✅ Bounded extraction thread pool (rayon, 4 threads) avoids O(n) spawning
- ✅ `sessions.json` mutex-serialized across concurrent writers
- ✅ Frontend types mirror Rust structures

---

## Top 3 recommendations for next loop

1. **OS keychain for credential storage.** Replace plaintext YAML with
   encrypted at-rest creds via `security-framework` (macOS) / `windows-rs`
   Credential Manager / `secret-service` (Linux). Fall back to encrypted YAML
   for portability. Effort: medium (2–3 days).

2. **Structured error codes + frontend i18n expansion.** `AppError` enum,
   20–30 variants, serialize with code + localized message; update all command
   handlers; localize frontend error messages. Effort: high (3–5 days).

3. **Speech processor integration test.** Mock ASR input stream (50 segments /
   10s), run full speech → diarization → extraction, assert transcript
   segments + entity counts. Unblocks detection of regressions in the largest
   untested module. Effort: medium (2 days).
