# Credential Save/Load Mechanism — Adversarial Review (2026-07-05)

Reviewer: Fable-tier adversarial review (read-only). Scope: the full credential
lifecycle — first save → keychain write → presence probe → provider selection
reads key → rotation/overwrite → delete — across the Rust backend
(`src-tauri/src/credentials/mod.rs`, `src-tauri/src/commands.rs`,
`src-tauri/src/settings/mod.rs`, `src-tauri/src/aws_util/mod.rs`) and the
React frontend (`src/App.tsx`, `src/components/ExpressSetup.tsx`,
`src/components/settings/useSettingsController.tsx`, `src/store/index.ts`).
Design of record: ADR-0019 (`docs/adr/0019-credential-and-config-storage.md`).

## Verdict summary

| Severity | Count |
| --- | --- |
| Blocker | 0 |
| Major | 4 |
| Minor | 6 |
| Nit | 3 |

The three known-history items check out:

1. **PR #29 empty-string skip fix holds.** `handleSaveCredentialValue`
   (`src/components/settings/useSettingsController.tsx:2517-2537`) surfaces a
   visible `empty` notice instead of silently no-oping, and the backend
   empty-skip is logged at info level in all three backend `set` impls
   (`credentials/mod.rs:481,712,849`). But a sibling path retains a related
   (deliberate, documented) silent skip — see M2.
2. **Dual config-writer race for credential-adjacent fields is mostly closed,
   but not for `set_analytics_enabled`.** See M3 for the one writer that still
   does an unlocked read-patch-write.
3. **Privacy invariant broadly holds.** All secret-bearing settings fields are
   `#[serde(skip_serializing)]`; `Debug` impls redact via
   `redacted_secret_presence`; the presence IPC returns only
   `{key, present, source}` (`commands.rs:209-214, 6660-6689`); Sentry's
   `scrub_free_text` drops all free prose (`analytics/mod.rs:356`); the save
   log emits only a one-way sha256 fingerprint
   (`credentials::secret_fingerprint`, `credentials/mod.rs:415`). Two narrow
   leak-shaped edges remain — see M4 and N2.

---

## Lifecycle diagram (save → probe → use → rotate → delete)

```
                      ┌──────────────────────────────────────────────────────────┐
                      │ FRONTEND (never holds saved plaintext; replace-only UI)  │
                      │  ExpressSetup / SettingsPage / CredentialsPanel drafts   │
                      └──────┬──────────────────────────────────────┬────────────┘
                             │ invoke save_credential_cmd            │ invoke load_credential_presence_cmd
                             │  {key, value}                         │  → [{key, present, source}]  ← never value
                             ▼                                       ▼
   [I1 allowlist] commands.rs:6582 is_allowed_key      commands.rs:6660 try_load_credentials_with_source
                             │                                       │
                             ▼                                       │  (presence computed from
        credentials::set_credential → DefaultCredentialBackend::set  │   full snapshot LOAD — see M1)
                             │                                       │
              ┌──────────────┴─────────────┐                         │
              ▼                            ▼                         ▼
   [I2 empty-skip] value.trim().empty  keychain.set_key      DefaultCredentialBackend::load_with_source
   → skip (log, no delete)             (OS keychain)         precedence per key:
                                           │                   1. delete tombstone  → missing
                             mark_migrated in                  2. credentials.yaml file_override (migrated keys)
                             credentials-state.yaml            3. keychain value
                                           │                   4. YAML import (untracked keys → keychain write!)
                                           ▼                          [side effect inside a "read" — M1]
   [I3 epoch] bump_provider_credential_epoch (commands.rs:6592)
   [I4 cache] rehydrate_app_settings_cache (commands.rs:6541)
              redacts inline → refills from store
                             │
                             ▼
   USE: read_settings_for_session_content clones AppState.app_settings
        (hydrated at load_settings_cmd/save_settings_cmd/save/delete;
         AWS STS re-reads backend per SDK call — aws_util/mod.rs:239)
                             │
   ROTATE: same save path; keychain overwrite + epoch bump + rehydrate.
        [I5 file-override hazard] a stale non-empty credentials.yaml entry
        for a migrated key SHADOWS the rotated keychain value — see M?/Minor 5.
                             │
   DELETE: delete_credential_cmd → keychain delete + tombstone (mark_deleted)
        + epoch bump + rehydrate (cache cleared).
        [I6 tombstone] blocks YAML resurrection on later loads/gets.
```

Invariant enforcement points: **I1** boundary allowlist
(`commands.rs:6582,6621`; inner match `credentials/mod.rs:1320`), **I2**
empty-is-skip (all three `set` impls), **I3** readiness-epoch invalidation,
**I4** symmetric writer-side cache rehydrate (save + delete share
`rehydrate_app_settings_cache`, closing #39 / c4d0), **I5** documented
file-override precedence (BUG 7fc5), **I6** delete tombstones in
`credentials-state.yaml`.

---

## Major findings

### M1 — Presence probe is a WRITE: `load_credential_presence_cmd` mutates the keychain and state file on every call

- **Where:** `src-tauri/src/credentials/mod.rs:883-917`
  (`DefaultCredentialBackend::load_with_source`), reached from
  `load_credential_presence_cmd` (`commands.rs:6660`), `load_credentials()`
  (called by `save_credential_cmd`, `delete_credential_cmd`,
  `load_settings_cmd`, `save_settings_cmd`, AWS SDK config building, startup).
- **What:** the snapshot load path performs, on every invocation:
  (a) `state.mark_present_keys(&store)` — a read-modify-write of
  `credentials-state.yaml`; (b) `import_missing_from_yaml` — keychain
  **writes** for any untracked YAML key; (c) the file-override scan. The
  frontend calls the presence probe from at least five places (App mount
  probe, App retry, ExpressSetup mount, Settings hydrate, post-save refresh),
  several of which run concurrently on first launch (App mount +
  ExpressSetup mount fire within the same tick).
- **Failure scenario (race):** two concurrent probes both read
  `credentials-state.yaml`, both compute independent
  `CredentialMigrationState` copies, and both write the whole file back via
  temp-file + rename. There is **no lock** anywhere in `credentials/mod.rs`
  (verified: zero `Mutex`/`RwLock` in the module), and the settings-side
  `SETTINGS_IO_LOCK` does not cover credential files. Last-writer-wins can
  drop a `mark_deleted` tombstone recorded by a concurrent
  `delete_credential_cmd`: probe A loads state (tombstone absent), user
  delete writes tombstone, probe A then writes back its stale state → the
  tombstone is erased → the next load re-imports the deleted key from
  `credentials.yaml` (resurrection, exactly what the tombstone exists to
  prevent). Same shape for a save's `mark_migrated` being reverted, which
  re-arms YAML import over the freshly saved keychain value.
- **Why it's plausible in practice:** `handleClearCredential`
  (`useSettingsController.tsx:2484-2489`) awaits the delete then immediately
  calls `refreshCredentialPresence()` *and* fires
  `refreshProviderReadiness()` unawaited — the readiness command also loads
  credentials. Two overlapping loads + one mutation is the normal UI flow,
  not an exotic one. The window is small (ms), so it will manifest as a rare
  "deleted key came back" ghost bug.
- **Fix direction:** (1) add a process-wide mutex in `credentials/mod.rs`
  guarding every state-file read-modify-write plus the import wave (mirror
  `SETTINGS_IO_LOCK`); (2) make the presence read path genuinely read-only —
  run the YAML import once at startup and after explicit user action, not
  inside every load; presence should not need `mark_present_keys` at all
  (it is a cache of what the load already computed).

### M2 — Footer Save still silently skips a *whitespace-only* credential, and `save_credential_cmd` reports success for a skipped save

- **Where:** frontend `saveCredentialIfPresent`
  (`useSettingsController.tsx:304-310`) — deliberate, PR #29 documented; and
  backend `save_credential_cmd` (`commands.rs:6558-6611`).
- **What:** the PR #29 fix made the *Credentials tab* row save non-silent,
  but two adjacent gaps remain:
  1. The backend command returns `Ok(())` for an empty/whitespace value (the
     backend `set` skips, logs, returns `Ok`). Frontend callers other than
     `handleSaveCredentialValue` — the ExpressSetup save helpers guard on
     `.trim()` themselves, but any future caller that passes whitespace (e.g.
     a pasted `"  "`) gets a success result, a bumped epoch, and a
     "presence refreshed" flow that shows the OLD key still present. The
     command even bumps `PROVIDER_CREDENTIAL_EPOCH` and rehydrates the cache
     on the skip path (`commands.rs:6592-6603` run unconditionally), doing
     spurious work and invalidating readiness caches for a no-op.
  2. `ExpressSetup.handleSave` (`ExpressSetup.tsx:609-643`) guards with
     `!asrKey.trim()` and returns silently — correct when a *saved* key
     exists (`asrUsesSavedKey` chip covers messaging), but if the user picks
     a cloud provider with no saved key and pastes whitespace, the blocker
     computation (`missingCredentialBlockers`) is what protects them, and
     that derives from `credentialPresence` fetched **once at mount**
     (`ExpressSetup.tsx:368-401`). A key deleted in Settings while
     ExpressSetup is open (edge, but reachable via the Advanced link
     round-trip) leaves a stale "present" and lets Save proceed with neither
     a draft nor a real saved key.
- **Fix direction:** make `save_credential_cmd` return a typed
  `SkippedEmpty` outcome (or an error) instead of `Ok(())`, and only bump
  epoch/rehydrate when a write actually happened. Re-fetch presence in
  ExpressSetup when it regains focus or before Save.

### M3 — `set_analytics_enabled` still does an unlocked read-patch-write of config.yaml (symmetric-writer blind spot)

- **Where:** `src-tauri/src/commands.rs:3512-3518`.
- **What:** the enumeration of all config.yaml writers is:
  1. `save_settings_cmd` (`commands.rs:3301`) — takes `SETTINGS_IO_LOCK` via
     `save_settings`.
  2. `set_logging_config` (`commands.rs:3410-3425`) — correctly holds
     `lock_settings_io()` across its load→patch→save (the fix pattern).
  3. `set_analytics_enabled` (`commands.rs:3514-3516`) — does
     `load_settings(&app)` **outside** any lock, patches, then calls
     `save_settings` (which locks only the write). A concurrent
     `save_settings_cmd` (footer Save) interleaved between the load and the
     save is clobbered: whatever the footer just wrote (provider selection,
     model tier, deepgram model, privacy_mode — i.e. credential-adjacent
     config) is reverted to the stale pre-Save snapshot that
     `set_analytics_enabled` read.
  4. Startup writers in `lib.rs:390,404` (inline-credential migration,
     first-launch demo mode) — sequential at startup, low risk.
  5. Legacy-import writeback inside `load_settings_from_paths_with_status`
     (`settings/mod.rs:2067`) — only when config.yaml is absent.
- **Why this matters for this review's scope:** provider selection *is*
  credential-adjacent config. The scenario "user saves a new provider+model
  in Settings, then flips the analytics toggle that was already open in the
  Logging panel" reverts the provider pick on disk while the in-memory cache
  keeps the new one — a divergence that surfaces only on next launch, as a
  provider silently reading a rotated-out configuration. The
  `preserve_owned_fields_from_disk` guard (`settings/mod.rs:1942`) protects
  `analytics_enabled` itself from other writers, but nothing protects other
  fields from *this* writer.
- **Fix direction:** copy the `set_logging_config` pattern verbatim: hold
  `lock_settings_io()` across load→patch→`save_settings_locked`. Three-line
  change.

### M4 — YAML/state parse errors embed raw file content risk via serde error text, and `CredentialFileError.reason` flows verbatim to the frontend

- **Where:** `credentials/mod.rs:672-673` (`Failed to parse {path}: {e}` from
  `serde_yaml`), propagated as `AppError::CredentialFileError { reason }`
  (`commands.rs:6590-6591, 6662`), rendered by the frontend via
  `errorToMessage` into toasts/alerts and — critically — as a *readiness
  error string* stored in React state (`setProviderReadinessError`).
- **What:** `serde_yaml` error messages can include a snippet of the
  offending scalar (e.g. `invalid type: string "sk-live-abc…", expected a
  map at line 3 column 18` shapes). `credentials.yaml` holds plaintext keys
  by design (legacy/fallback), so a malformed edit — precisely the moment a
  user hand-edits a key (the file-override feature invites this) — can echo
  a key fragment into: the UI error banner, `console.error`
  (`useSettingsController.tsx:2491`), and the log file. This does not pass
  through `redacted_provider_diagnostic`; the Sentry scrubber would catch it
  on that channel only.
- **Failure scenario:** user hand-edits `credentials.yaml` per BUG 7fc5
  workflow, leaves a stray quote, opens Settings → presence probe fails →
  the raw parse error including the mistyped key value renders in the
  Settings readiness banner and lands in the app log (log files are exactly
  what users attach to bug reports).
- **Fix direction:** route every `CredentialFileError.reason` through
  `crate::error::redacted_provider_diagnostic` (or a stricter "parse failed
  at line N, content omitted" formatter) before it leaves the credentials
  module. Assert in a test that a deliberately malformed file containing a
  sentinel `sk-…` never appears in the surfaced reason.

---

## Minor findings

### m1 — File-override can silently defeat a rotation done through the app

`migrated_overrides_from_yaml` (`credentials/mod.rs:955-986`) makes a
non-empty `credentials.yaml` entry for a migrated key permanently override
the keychain. Rotation flow: user has a legacy YAML entry (kept by design —
first wave never deletes the file), then rotates the key **through the app
UI**. The keychain now holds the new key; the YAML still holds the old one;
the very next load flips the effective value back to the OLD key
(`file_override` wins, `credentials/mod.rs:909-912`). The UI shows presence
`source: "file_override"` — but nothing warns "your app-saved key is being
shadowed by a file edit you may not remember." The Deepgram-401 diagnosis
this repo already lived through will reappear wearing a different hat.
**Fix direction:** on `save_credential_cmd` success for a migrated key,
also clear (or comment out) that key in `credentials.yaml`, or at minimum
log at warn + surface a UI chip when a file override is actively shadowing a
keychain value that was saved more recently. (Timestamps in the state file
would make this decidable.)

### m2 — `KeychainCredentialBackend::save` deletes every absent key (whole-store save is destructive-by-default)

`save` (`credentials/mod.rs:828-836`) iterates ALL allowlisted keys and
issues `delete_key` for any that is empty in the passed store. The only
production caller of whole-store `save_credentials` today is none (verified:
no non-test callers), so this is latent — but any future caller that builds
a partial `CredentialStore` and calls `save_credentials` will wipe every
other provider's key from the OS keychain. The YAML path has the same
whole-file overwrite shape but at least starts from `load_or_default`.
**Fix direction:** either remove the public `save_credentials` (dead code)
or make it merge-on-save (load, overlay non-None fields, write).

### m3 — Delete flow has no empty-notice symmetry and no per-key result; multi-key clear is not atomic

`handleClearCredential` (`useSettingsController.tsx:2473-2496`) loops
`delete_credential_cmd` over multiple keys (e.g. AWS triple) sequentially;
a failure mid-loop leaves a partial clear (access key deleted, secret key
still present) with a single generic alert. Presence refresh then shows a
half-configured AWS credential set whose readiness state is confusing.
**Fix direction:** aggregate failures per key and report which keys
remain; or add a backend `delete_credentials_cmd(keys: Vec<String>)` that
reports per-key outcomes.

### m4 — `aws_secret_key` absence check in `build_aws_sdk_config` reads the store snapshot once for static creds

`aws_util/mod.rs:232-255`: for long-term IAM keys (no session token), the
secret is captured at session build time. A rotation mid-session keeps the
old secret until the session is rebuilt — inconsistent with the
session-token path, which re-reads per SDK call via
`BackendRefreshingCredentialsProvider`. Combined with the epoch bump on
save, the readiness chip will say "ready (new key)" while a live Transcribe
session still signs with the old one. Known trade-off (comment says "matches
prior behavior"), but the asymmetry is undocumented at the readiness layer.
**Fix direction:** either wrap static creds in the refreshing provider too
(cost: one store read per request) or document/surface "restart capture to
apply" after an AWS key save.

### m5 — Demo-mode key list has drifted from the durable-pair logic

`DEMO_CREDENTIAL_KEYS` (`settings/mod.rs:2127-2142`) omits
`together_api_key` and `fireworks_api_key`, both of which are in the
frontend's `DURABLE_CLOUD_LLM_CREDENTIAL_KEYS` (`App.tsx:93-102`) and in the
backend allowlist. A user whose only key is Together/Fireworks gets
auto-flipped into demo mode (local ASR + local LLM) on first launch even
though they have a runnable cloud LLM credential — the provider selection
they configured is silently overwritten by
`apply_first_launch_demo_mode` (`settings/mod.rs:2200-2203`). The comment
says "IMPORTANT: keep in sync with `FIRST_TIME_CREDENTIAL_KEYS` in
`src/App.tsx`" — a constant that no longer exists under that name (renamed
in the PR #70 refactor to the two `DURABLE_CLOUD_*` sets), so the sync
anchor is dangling.
**Fix direction:** add the two keys; regenerate or share the list via the
generated-types pipeline; fix the stale comment.

### m6 — `load_credential_presence_cmd` fails closed but the App-level catch treats *any* throw as "backend not ready"

`runCredentialProbe` (`App.tsx:291-305`) catches everything and shows the
Get-started fallback. A real, actionable failure (malformed
`credentials-state.yaml` — which makes `load_with_source` return `Err` via
`self.state.load()?` at `credentials/mod.rs:886`) is indistinguishable from
a fresh install; the user is told to "get started" while their saved keys
exist but are unreadable, and ExpressSetup will then re-prompt for keys and
overwrite. The probe error text is discarded (bare `catch`).
**Fix direction:** distinguish error kinds (structured `AppError` code is
already available in the payload) and surface "your saved credentials could
not be read" with a repair hint instead of the first-run card.

---

## Nits

### n1 — Redundant work in `save_credential_cmd` on the skip path

Epoch bump + full store reload + cache rehydrate run even when the save was
an empty-skip (`commands.rs:6592-6603` before the `value.trim().is_empty()`
check at 6605). Harmless but wasteful; folding the skip check earlier also
fixes half of M2.1.

### n2 — `secret_fingerprint` logs key length

`sha256:<8hex> len=<n>` (`credentials/mod.rs:421`) — the length of an API
key is weak metadata but nonzero (distinguishes provider key families).
Fine as a deliberate diagnostic trade-off; consider bucketing the length
(e.g. `len≈32-64`) if logs are shared externally.

### n3 — `CredentialSnapshot::source_for` returns `missing` for a present-but-whitespace key, while `present_count` agrees — but the smoke-test payload builder is the only place asserting the pair stays coherent

The coherence between `is_present` (trim-based) and `key_sources` (only
populated for present keys) is enforced by construction, not by a unit test
on `CredentialSnapshot` itself outside the ignored OS-keychain smoke test.
Cheap test to add.

---

## What was checked and found sound

- **No `load_credential_cmd` remnants**: only a regression *test* asserting
  it is NOT registered (`commands.rs:11478`). No plaintext-returning
  credential command exists.
- **IPC redaction**: `load_settings_cmd` returns `redacted_settings`
  (`commands.rs:3272`), never the hydrated copy; the hydrated copy lives
  only in `AppState.app_settings`. All secret fields are
  `skip_serializing`, so even a future accidental serialization of the
  hydrated struct drops them (verified for AsrProvider, LlmProvider,
  LlmApiConfig, GeminiAuthMode, OpenAiRealtimeAgentAuthMode,
  AwsCredentialSource; `settings/mod.rs:126-906`).
- **Frontend never hydrates plaintext**: Settings drafts are blanked on
  hydrate; "replace-only" inputs; presence drives saved-key chips
  (`useSettingsController.tsx:3106-3108`).
- **Delete tombstones** block YAML resurrection, including via the
  file-override path (`is_deleted` short-circuit,
  `credentials/mod.rs:967`), covered by tests
  (`migrated_yaml_override_does_not_resurrect_deleted_key`,
  `fallback_delete_tombstone_masks_recovered_keychain_value`).
- **Save/delete writer symmetry**: both go through
  `rehydrate_app_settings_cache` (the c4d0 fix) — the inverse-delete bug
  class is structurally closed.
- **Keychain-unavailable fallback is opt-in only**: plaintext YAML fallback
  requires the explicit `AUDIO_GRAPH_CREDENTIAL_BACKEND=keychain_with_file_fallback`
  env; the default errors out instead of silently degrading
  (`credential_backend_mode_from_env`, `credentials/mod.rs:1109-1116`,
  with tests).
- **Atomic writes**: temp file (0o600 on Unix / pre-write icacls on
  Windows) + rename-with-retry; malformed-file mutation refusal preserves
  recovery data (`yaml_backend_set_preserves_malformed_file`).
- **Sentry**: breadcrumbs dropped wholesale; free text reduced to redaction
  sentinels; the frontend diag relay accepts only id-shaped fields
  (`commands.rs:3523+`).

## Residual verification suggestions

1. A concurrency test: N parallel `load_with_source` + one `delete` against
   a temp state path, asserting the tombstone survives (targets M1).
2. A leak test: seed a malformed `credentials.yaml` containing
   `LEAKCANARY-sk-123`, assert the surfaced `CredentialFileError` reason and
   the log line never contain the sentinel (targets M4).
3. A clobber test for `set_analytics_enabled` racing `save_settings_cmd`
   (targets M3) — the existing
   `save_settings_preserves_on_disk_analytics_when_payload_omits_it` covers
   the inverse direction only.
