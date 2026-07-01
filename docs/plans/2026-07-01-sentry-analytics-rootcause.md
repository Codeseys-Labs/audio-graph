# Root-Cause Verdict: analytics toggle "isn't holding" + Sentry receives nothing

**Date:** 2026-07-01
**Scope:** Tauri app `/mnt/e/CS/github/audio-graph`. Two linked symptoms:
(1) the "Send anonymous analytics" toggle does not persist, and (2) Sentry receives no events.
**Method:** Synthesis of two evidence lanes (logs: `/tmp/rootcause-logs.md`, code: `/tmp/rootcause-code.md`)
plus independent spot-check of every load-bearing line. No code edited.

---

## ROOT CAUSE (one sentence)

The main Settings footer Save re-serializes the **entire** `AppSettings` struct from a store object
that never carries `analytics_enabled`, so the field arrives as `None` and
`#[serde(default, skip_serializing_if = "Option::is_none")]` at
**`src-tauri/src/settings/mod.rs:1233-1234`** silently **drops the key** from `config.yaml`,
clobbering the `analytics_enabled: true` that `set_analytics_enabled` had written — the clobbering
whole-struct write is **`src-tauri/src/commands.rs:3221`** (`save_settings_cmd` →
`save_settings(&app, &settings)`), fed the analytics-less struct by
**`src/store/index.ts:2425-2427`** (`saveSettings` → `save_settings_cmd`).

**Broken layer:** frontend persistence model / cross-path clobber (two uncoordinated whole-file writers
racing the same `config.yaml`), the silent-drop being enabled by the serde `skip_serializing_if` on the
backend struct. It is NOT the network/DSN, NOT the frontend toggle wiring, and NOT the
`set_analytics_enabled` command itself.

---

## Causal chain (hop by hop, with confirmed file:line)

1. **Boot.** Store `fetchSettings()` (`src/store/index.ts:2413-2417`) calls `load_settings_cmd`.
   Disk `config.yaml` has no `analytics_enabled` key, so serde `#[serde(default)]` deserializes it
   to `Option::default()` = **`None`** (NOT `Some(false)`), which serializes over IPC to
   `undefined` in the store's `settings` object.
2. **User flips the toggle ON.** `LoggingSettings.tsx:145-153` `applyAnalytics(true)` invokes
   `set_analytics_enabled({ enabled: true })` directly, storing the result only in a LOCAL
   `useState` (`analyticsInfo`) — it never writes back to the zustand store.
3. **Command persists correctly (in isolation).** `commands.rs:3411-3441` `set_analytics_enabled`:
   inits Sentry at runtime (`init_if_enabled(true)`, line 3419), sets the in-memory cache to
   `Some(true)` (line 3429), and does a load→patch→save writing `analytics_enabled: true` to
   `config.yaml` (lines 3434-3436). The serialize path preserves it — `redacted_settings`
   (`settings/mod.rs:1578-1626`) only clears secrets, leaving `analytics_enabled` intact, and
   `skip_serializing_if` does NOT skip a `Some` value.
4. **Main Save clobbers it.** The user changes any other setting and clicks the footer Save →
   `saveSettings(store.settings)` (`src/store/index.ts:2425-2427`) sends the whole struct with
   `analytics_enabled === undefined` → `save_settings_cmd` (`commands.rs:3216-3232`) →
   `save_settings(&app, &settings)` at **`commands.rs:3221`** re-serializes the FULL struct.
   `analytics_enabled` is `None`, and `skip_serializing_if = "Option::is_none"`
   (`settings/mod.rs:1233-1234`) **omits the key entirely** (writes nothing, not even `false`),
   erasing the `true` from step 3. (`useSettingsController.tsx` has ZERO analytics references —
   verified by grep — so the store never learns the value and never writes it back.)
5. **`config.yaml` now has no `analytics_enabled` key.** Confirmed by the orchestrator/logs lane:
   the freshly-written config (mtime `13:47:54.337`, matching the final `Settings saved` log line)
   contains every other setting but no `analytics_enabled`.
6. **Next launch reads it as false.** `lib.rs:360` `let enabled = settings.analytics_enabled.unwrap_or(false)`
   → `enabled = false`.
7. **`init_if_enabled(false)` no-ops.** `analytics/mod.rs:147-150` returns immediately when
   `!enabled` — no client, no guard, no transport, and the `app.startup` ping (`lib.rs:367-368`)
   is skipped.
8. **No client → no Sentry sends.** Zero analytics/sentry/dsn/capture lines in the live log
   (orchestrator + logs lane confirm 0 hits). AND the toggle appears to "reset": on remount,
   `get_analytics_info` reads the backend cache/persisted value, which is now false, so the
   checkbox (`checked={analyticsInfo?.enabled ?? false}`, `LoggingSettings.tsx:296-297`) flips
   back off.

---

## Do both symptoms share ONE root cause? — YES

Both symptoms are the same defect observed at two moments:

- **"Toggle isn't holding"** = the persisted `true` is erased by the next whole-struct Save
  (steps 4-5), and the checkbox re-reads the erased/false value on remount (step 8).
- **"Sentry receives nothing"** = startup reads the erased field as `false`, so
  `init_if_enabled(false)` never creates a client (steps 6-8).

The single erasure of `analytics_enabled` from `config.yaml` (the cross-path clobber, steps 4-5)
causes both. They are NOT separate bugs.

---

## Innocent layers ruled out

- **Network / Sentry DSN:** FINE. The DSN is embedded by default (`analytics/mod.rs:78`), so
  Sentry is gated purely on the `enabled` flag, not a missing DSN. The problem is strictly
  upstream — the flag never reaches startup as `true`.
- **Frontend toggle wiring:** FINE. `LoggingSettings.tsx:150` genuinely invokes
  `set_analytics_enabled({ enabled: true })`.
- **`set_analytics_enabled` command / its disk write:** FINE in isolation
  (`commands.rs:3434-3436` writes `Some(true)`); it is only later overwritten by the main Save.
- **Serialize path (`redacted_settings`):** FINE — preserves `analytics_enabled` when `Some`.
- **Save-vs-load path mismatch:** REFUTED by both lanes. Both the analytics command and startup
  resolve to `app_config_dir()/config.yaml` = `.../com.rsac.audiograph/config.yaml`
  (`settings/mod.rs:1740-1746`). The `audio-graph/logs/` dir is the separate logging resolver,
  not the settings path. Logs confirm identical read/write paths and 8 successful saves with no
  write error, no panic (only benign keyring races and an unrelated Deepgram 401).

---

## Minimal fix (do NOT implement)

Make the main Save preserve `analytics_enabled` instead of clobbering it. Pick the single change
that best fits the codebase's persistence model; any one of these fixes the root cause:

- **Preferred (surgical, backend-only):** in `save_settings_cmd`
  (`src-tauri/src/commands.rs:3216-3232`), do a load→patch→save that preserves the on-disk
  `analytics_enabled` rather than a whole-struct rewrite — i.e., before saving, if the incoming
  `settings.analytics_enabled.is_none()`, carry over the existing on-disk value (or the in-memory
  cache value, which `set_analytics_enabled` keeps authoritative at `commands.rs:3429`). This makes
  the analytics field owned by the analytics path and immune to the form's whole-struct rewrite.
- **Alternative (frontend model):** have the store carry `analytics_enabled` and update it when the
  toggle fires (thread it through `useSettingsController` / the store `settings`) so the Save
  payload always sends the true value.

**Note on the serde attribute:** `skip_serializing_if = "Option::is_none"` at
`settings/mod.rs:1233-1234` is the silent-drop mechanism (writes nothing instead of `false`), but
removing it alone is NOT a complete fix — it would persist `false` and still overwrite `true` from
the whole-struct Save. The real fix is preventing the analytics-less struct from rewriting the
field; the serde attribute just makes the loss silent.

### The ONE test that proves the fix (round-trip)

Round-trip persistence test:
`set_analytics_enabled(true)` → then invoke `save_settings_cmd(settings)` with an `AppSettings`
whose `analytics_enabled` is `None`/absent (simulating the store's stale payload) → read the
serialized `config.yaml` → assert it contains `analytics_enabled: true` → then run the startup
read (`settings.analytics_enabled.unwrap_or(false)`) and assert it returns `true`. Before the fix
this fails (key dropped, startup reads `false`); after the fix it passes.

---

## Secondary issue

**Runtime enable within a live session may still send nothing if the session never gracefully
shuts down.** Sentry buffers events and flushes on a guard held for process lifetime; the analytics
module only emits its `flush_on_exit` log line at graceful shutdown (proven by the archived
`134147` session, which had Sentry ACTIVE yet logged analytics only at shutdown). The live `13:47`
session was hard-killed (no `Graceful shutdown` line), so even had analytics been enabled at
runtime, buffered events may not have flushed. This is a SECONDARY reliability concern (flush
timing on non-graceful exit / `capture_anonymous_event` buffering), independent of and behind the
primary persistence clobber — worth verifying after the primary fix, but it does not explain the
persisted `config.yaml` missing the key.
