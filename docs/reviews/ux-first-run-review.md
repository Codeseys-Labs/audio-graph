# First-Run UX Review

**Date:** 2026-04-16

## Summary

Walked through 6 first-time user scenarios. Found **6 CRITICAL issues** and
**several HIGH/MEDIUM gaps**. Most critical: AWS "Access Keys" form is
incomplete (missing Secret Key field = feature broken), no "Test Connection"
button exposes users to 10+ second delays discovering bad API keys, and
model/API errors fail silently in speech processor threads.

## Critical Issues (block or severely degrade UX)

| # | Issue | Location | Severity |
|---|-------|----------|----------|
| 1 | Model errors fail silently in speech processor thread | speech/mod.rs + frontend | CRITICAL |
| 2 | API errors (bad key) don't surface to UI | llm/api_client.rs | CRITICAL |
| 3 | No "Test Connection" button for any cloud provider | SettingsPage.tsx | CRITICAL |
| 4 | AWS Access Keys form missing Secret Key input | SettingsPage.tsx ~628 | CRITICAL |
| 5 | No model readiness check before "Start Transcribe" | ControlBar.tsx + store | CRITICAL |
| 6 | Empty audio source list gives no remediation hint | AudioSourceSelector.tsx | CRITICAL |

## High Priority

| # | Issue | Severity |
|---|-------|----------|
| 7 | No session history — restart = fresh start | HIGH |
| 8 | Cloud API save has no validation / feedback | HIGH |
| 9 | AWS profile dropdown doesn't auto-populate | HIGH |
| 10 | AWS credential errors cryptic ("Unable to refresh credentials") | HIGH |

## Medium Priority

| # | Issue | Severity |
|---|-------|----------|
| 11 | Gemini WebSocket errors not categorized (auth vs network vs rate) | MEDIUM |
| 12 | Model download has no time estimate (e.g. "466MB, ~2min") | MEDIUM |
| 13 | Model size picker lacks guidance on which to choose | MEDIUM |

## Scenario Walkthroughs

### Scenario 1: First launch, default config
- **Expected:** Audio sources populate, ready to start
- **Actual:** Works in happy path; permission denial or PipeWire failure produces
  "No audio sources detected" with no remediation hint
- **Fix:** Show OS-specific instructions (macOS: System Settings → Privacy;
  Linux: `systemctl --user start pipewire`)

### Scenario 2: Local Whisper transcription
- **Expected:** Download model, transcribe
- **Actual:** Settings has download buttons, but if model missing when
  user clicks Start Transcribe, the speech processor thread loads and
  fails silently; frontend never sees the error
- **Fix:** Pre-flight check in `start_transcribe` Tauri command;
  return error before spawning thread

### Scenario 3: Cloud API (Groq/OpenAI)
- **Expected:** Enter endpoint + key, test, transcribe
- **Actual:** No Test Connection. User saves, starts transcribe, waits 10s,
  sees nothing. 401 errors logged but not emitted.
- **Fix:** Add `test_asr_provider` Tauri command with `GET /models` or similar;
  add "Test Connection" button in Settings

### Scenario 4: AWS Transcribe
- **Expected:** Select credential mode, configure, work
- **Actual:** **"Access Keys" form has ONE field but AWS needs TWO**
  (Access Key ID + Secret Access Key). Secret is supposed to be in
  credentials.yaml but UI doesn't expose it. Feature BROKEN.
- **Fix:** Add Secret Access Key password field + optional Session Token field;
  populate AWS profile dropdown from `list_aws_profiles()` command

### Scenario 5: Gemini Live
- **Expected:** Enter API key, connect
- **Actual:** Works; WebSocket errors are generic
- **Fix:** Categorize errors (auth/network/rate-limit/server)

### Scenario 6: Power user — 2 hours in
- **Expected:** Can load prior session
- **Actual:** No UI. Each restart = new session. Old transcripts
  orphaned on disk.
- **Fix:** Session management (see session-management.md)

## Recommended Implementation Order

1. **Fix AWS Secret Key field** (15 min) — feature is broken
2. **Add pre-flight model check** in `start_transcribe` (30 min)
3. **Add "Test Connection" Tauri commands** + UI buttons (2-3 hrs)
4. **Disable Transcribe button when model not Ready** (1 hr)
5. **Emit API/ASR errors to frontend** (2 hrs) — use events::emit_or_log
6. **Permission hints in AudioSourceSelector empty state** (1 hr)
7. **Auto-populate AWS profile dropdown** (1 hr)
8. **Model download time estimates + size guidance** (2 hrs)
9. **Session management** (1 day, see session-management.md)
