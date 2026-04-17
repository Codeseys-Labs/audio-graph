# audio-graph review — Loop 11

**Date:** 2026-04-17
**Reviewer:** claude-agent (read-only explore pass)
**Scope:** audio-graph (backend + frontend + CI + docs)
**Context:** Parallel landing during this review: audio_settings wire-through
(HIGH #1), AppError enum pilot (MEDIUM #8), Settings i18n expansion (HIGH #2).

## Summary

Fresh read after loops 9–10 landed (crash_handler, logging, aws_util,
persistence/io, release pipeline, docs). Solid architectural groundwork with
good test discipline for pure functions. Four Loop-10 HIGH findings remain
open (three in-flight as this review runs — rescored against the freshly-
landed code by the next loop). Three new MEDIUM findings surfaced from a
fresh angle: credential-key allowlist gap on the frontend→backend boundary,
SettingsPage local-state explosion (48 useState hooks), and missing URL
validation in `configure_api_endpoint`.

**Counts:** 0 CRITICAL, 4 HIGH (confirmed open from Loop 10), 6 MEDIUM (3 new,
3 from Loop 10 still open), 1 LOW.

---

## CRITICAL

None this cycle.

---

## HIGH

### 1. Audio settings persisted but never read — CONFIRMED OPEN FROM LOOP 10
See Loop-10 HIGH #1. Parallel agent A2 is landing the wire-through during
this review pass; re-score in Loop 12.

### 2. Frontend i18n bulk labels still hard-coded — CONFIRMED OPEN FROM LOOP 10
See Loop-10 HIGH #2. Parallel agent A4 is landing the SettingsPage sweep
during this review.

### 3. Speech processor (2513 LOC) integration-untested — CONFIRMED OPEN FROM LOOP 10
See Loop-10 HIGH #3. Not in-flight this loop.

### 4. Gemini session resumption never wired — CONFIRMED OPEN FROM LOOP 10
See Loop-10 HIGH #4. Not in-flight this loop.

---

## MEDIUM

### 5. NEW — Credential key validation missing at the frontend boundary
**Files:** `src-tauri/src/commands.rs:1654-1677`,
`src-tauri/src/credentials/mod.rs:137-177`

`save_credential_cmd`, `load_credential_cmd`, `delete_credential_cmd` all
accept arbitrary key strings from the frontend without pre-validation.
Example:

```rust
pub fn save_credential_cmd(key: String, value: String) -> Result<(), String> {
    crate::credentials::set_credential(&key, &value)
}
```

The backend `set_field` function has an allowlist (lines 137–155) — the
mismatch case returns `Err(format!("Unknown credential key: {}", key))`.
So the allowlist *is* enforced, just at the inner layer rather than the
boundary. YAML escaping prevents path traversal. Still, the pattern is
weaker than explicit boundary validation.

**Impact:** Low security risk in practice. Pattern inconsistency: everywhere
else in the codebase, path-like inputs get validated at the command layer
(see `validate_session_id` in commands.rs). Exporting an explicit allowlist
to the frontend types also makes the UI self-documenting.

**Action:** Extract `ALLOWED_CREDENTIAL_KEYS: &[&str]` in `credentials/mod.rs`.
Export the set to the frontend TypeScript types. Validate at the command
layer before calling `set_credential`.

### 6. NEW — SettingsPage local state explosion (48 useState hooks)
**File:** `src/components/SettingsPage.tsx:60-124`

48 separate `useState` declarations for form fields across ASR / LLM / Gemini
/ AWS Transcribe / AWS Bedrock / Deepgram / AssemblyAI / Sherpa-ONNX
sections. Each keystroke in any field schedules a re-render of the entire
SettingsPage. The `useEffect` that syncs local state from `settings` has
a dep list that grows with every new field.

**Impact:** Re-render cost grows O(n) with field count. Harder to maintain
— adding a new provider means threading state through 4–6 sites. Not a
correctness issue; polish.

**Action:** Consolidate into `useReducer` keyed by the `AppSettings` shape,
OR adopt `react-hook-form` / `formik` for complex forms. Incremental
migration is fine — start with the AWS access-keys block.

### 7. NEW — `configure_api_endpoint` accepts any string as URL
**File:** `src-tauri/src/commands.rs:735-767`

```rust
pub async fn configure_api_endpoint(
    endpoint: String, api_key: Option<String>, model: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let config = ApiConfig { endpoint, api_key, model, ... };
    let client = ApiClient::new(config);
    if !client.is_configured() {
        return Err("Invalid API configuration: endpoint and model must be non-empty".to_string());
    }
```

Only non-empty check. Accepts `endpoint = "htp://..."`, `file:///etc/passwd`,
or literally `"foo"`. The user discovers the problem when a chat request
hits the endpoint and fails opaquely.

**Action:** `url::Url::parse()` the endpoint; require scheme in `{http, https}`.
Reject others with an error message that names the offending scheme.

### 8. NEW — `aws_util` test coverage minimal
**File:** `src-tauri/src/aws_util/mod.rs`

One happy-path test (`yaml_credentials_provider_reads_latest_disk_value`).
Not covered:
- Malformed `credentials.yaml` (serde_yaml parse error)
- Missing file (file-not-found)
- Permission errors (EACCES)
- Empty YAML (all fields `None`)

The provider re-reads disk on every SDK credential request — any transient
error during the read translates directly to an SDK-side `CredentialsError`.

**Action:** Add 3–4 error-path tests. Consider a short cache (1–5 s TTL)
so transient read failures don't translate to transient auth failures.

### 9. NEW — `persistence/io` module has no unit tests
**File:** `src-tauri/src/persistence/io.rs`

`write_or_emit_storage_full()` and `handle_write_error()` added in Loop 10
have no unit tests. The `is_storage_full` classifier in `events.rs` *is*
tested, but the wrapper logic that emits events + logs + returns errors
isn't.

**Action:** Add tests for: successful write, ENOSPC path (fake IO error),
non-ENOSPC error path, event emission assertion (use a mocked AppHandle
or a trait abstraction).

### 10. CONFIRMED OPEN FROM LOOP 10 — Token usage tracking (MEDIUM #5)
### 11. CONFIRMED OPEN FROM LOOP 10 — No TOML config loader (MEDIUM #6)
### 12. CONFIRMED OPEN FROM LOOP 10 — Plaintext credentials at rest (MEDIUM #7)
### 13. Log level persistence pattern inconsistency
**File:** `src-tauri/src/commands.rs:1092-1113` vs `1066-1077`

`set_log_level` does load → mutate → save; `save_settings_cmd` saves the
full AppSettings including `log_level` directly. If the frontend races a
direct `save_settings_cmd` (full settings blob) against `set_log_level`,
the final state depends on timing. Low severity (single user, slow clicks),
but breaks the "one path per field" principle.

**Action:** Pick one canonical path. Either `set_log_level` is the only
mutator (and `save_settings_cmd` ignores the `log_level` field), or
`save_settings_cmd` is canonical (and `set_log_level` is just a thin
wrapper that calls it).

---

## LOW

### 14. `#[allow(dead_code)]` instances with documented rationale — UNCHANGED
Same files as Loop-10 LOW #12. All intentional, each carries a comment.

---

## Resolved since loop-10

- ✅ **Disk-full handling** — `CAPTURE_STORAGE_FULL` event, classifier,
  persistence/io wrapper, frontend subscription. Loop-10 MEDIUM #11 closed.
- ✅ **New module test discipline** — `crash_handler` (4 tests),
  `logging` (8 tests from this loop). Both pure-function focused, no
  flakiness.
- ✅ **Atomic settings / credentials writes** — temp-file + rename +
  `set_owner_only` pattern confirmed in both `credentials/mod.rs` and
  `settings/mod.rs`.
- ✅ **AWS credential refresh** — `YamlRefreshingCredentialsProvider`
  re-reads YAML on every SDK call (Loop-10 HIGH #7 → ✅).
- ✅ **Cross-platform permission handling** — `fs_util::set_owner_only`
  Unix 0600 / Windows best-effort.

---

## Noted but not flagged (positive confirmations)

- ✅ Backpressure detection — edge-triggered, no spam
- ✅ ControlBar useEffect deps correct, no stale closures
- ✅ Store derived state (canStart / canTranscribe / canGemini) computed correctly
- ✅ Credentials zeroize-on-drop preserved through the refreshing provider
- ✅ `parse_capture_target` strip-prefix prevents `../` traversal
- ✅ Dispatcher thread fan-out (speech + Gemini) shares one processed_rx stream

---

## Top 3 recommendations for Loop 12

1. **Credentials allowlist at the command boundary.** Extract constant,
   export to frontend types, validate in commands.rs before calling set_credential.
   Effort: small (0.5 day).

2. **Error-path tests for `persistence/io` and `aws_util`.** Covers the
   modules most likely to bite users silently. Add 6–8 tests. Effort:
   medium (1–2 days).

3. **Consolidate SettingsPage into useReducer.** Reduces re-render cost
   and simplifies adding new providers. Incremental migration is fine.
   Effort: medium (2–3 days). Pair with Agent A4's i18n sweep so the
   reducer action types also carry translation keys.
