# Deepgram cloud-fast 401 — Root-Cause Verdict

Date: 2026-07-01
Branch: fix/gtk-test-harness-65f0
Inputs synthesized: `/tmp/dg401-code.md` (credential→auth code-path trace) and
`/tmp/dg401-logs.md` (Windows runtime log forensics + live keyring probe).
All cited files spot-checked against the working tree and git history below.

---

## VERDICT (one sentence)

The stored Deepgram key is resolved correctly and transmitted correctly, and Deepgram
rejects it with a WebSocket 401 — this is a **stale / revoked key at the provider**, not a
code regression that strips or mangles the key. No code root cause. (If the live curl in
"Decisive test" comes back 401, close as stale key; if it comes back 200, re-open as an
on-the-wire header regression — but the code evidence makes that unlikely.)

## Classification: **B — STALE / INVALID KEY**

(with C_inconclusive as the honest fallback: the two lanes agree A is falsified and the
header path is clean, and the only thing not directly observed is Deepgram's own accept/reject
of the exact key — one curl closes that gap.)

---

## Why it is NOT a code regression (Fork A falsified twice, independently)

1. **Empty-key guard short-circuits before any network call.**
   `src-tauri/src/asr/deepgram.rs:260-262` — `connect()` returns the string
   `"Deepgram API key is not configured"` when `self.config.api_key.is_empty()`. The logs show
   a *WebSocket 401*, not that string, so a **non-empty** key reached `connect_async`.

2. **Live keyring probe proves the correct key is present.** `/tmp/dg401-logs.md` §6:
   a read-only `CredRead` of `provider:deepgram_api_key.audio-graph` returned
   `LEN=40 / f22395…cda8`, byte-for-byte equal to the plaintext
   `<DEEPGRAM_KEY_REDACTED>` on disk in `credentials.yaml`
   (mtime 2026-06-20, the last day it worked). Keyring is not empty, not truncated, not
   corrupted.

3. **The transmitted header is clean.** `src-tauri/src/asr/deepgram.rs:504-516` builds
   `Authorization: Token {api_key}` (line 506), correct scheme, correct `Host: api.deepgram.com`,
   no whitespace injection. `deepgram_listen_url` (`deepgram.rs:549-586`) puts **no token in the
   query string** — auth is header-only. There is no code path that mangles *how* the key is sent
   (this kills the logs lane's "B-header" speculation).

4. **Both stored copies are the SAME 40-char value**, so it is moot which source the resolver
   picks — the identical correct key is sent either way.

---

## Why the "new credential code co-emergence" is a timing artifact, not causation

The logs lane fingered commit `2ec816f "fix(security): real Windows credential ACLs"` as
co-emerging with the first 401. Spot-check of the git history disproves the timing:

- `2ec816f` is dated **2026-05-29**, a full month BEFORE the last successful session
  (2026-06-20). It cannot be the 06-29 regression, and it touched only the SAVE/temp-write path
  (`credentials/mod.rs`, `fs_util/mod.rs`), never a READ path.
- The credential-migration WARN chain that first appears **2026-06-29** is the write-path
  `?`-propagation of `try_set_owner_only` carried in the Jul-1 `8a603e4` "Backlog-zero wave"
  (PR #22) plus `de53a41` (2026-06-29, save-path parent-dir/rename). These abort the credential
  **save/finalize** on Windows `icacls` non-zero and hit `os error 80` on a leftover `.tmp`.
- **A failed state-file WRITE during load is swallowed as a WARN and the keychain READ already
  succeeded** (`credentials/mod.rs:825-827` — `mark_present_keys` failure only logs). So the WARN
  cluster spams logs but does **not** clear, block, or corrupt the key read. It is a real but
  SEPARATE latent bug, not the 401 cause.

## The one 06-29 READ-path change, checked and cleared

`3d453ef "honor edited credentials.yaml for migrated keychain keys"` (**2026-06-29**, exactly at
the failure boundary) adds `migrated_overrides_from_yaml` (`credentials/mod.rs:891-922`) letting a
hand-edited `credentials.yaml` override the migrated keychain value. Spot-checked in the working
tree — it CANNOT produce an empty/wrong key:

- Empty yaml value is skipped: `if file_value.is_empty() { continue; }` (line 910-912).
- Identical value is a no-op: `if keychain_store.get(key).ok().flatten() == Some(file_value) { continue; }` (line 916).
- Since the on-disk yaml value equals the keychain value, this override is a no-op here.
- Delete tombstones still win (`is_deleted` guard, line 903), so it can't resurrect a deleted key.

It is covered by passing tests `edited_credentials_yaml_overrides_migrated_keychain_value` and
`migrated_yaml_override_does_not_resurrect_deleted_key`.

## Model-name red herring

`nova-3` → `nova-3-general` is not the cause: the 07-01 logs show plain `nova-3` ALSO 401ing now,
and `nova-3` connected fine for a month prior. `nova-3-general` is not even present in `src-tauri/`.

---

## Root cause (file:line)

No code defect. The stored key `<DEEPGRAM_KEY_REDACTED>` is resolved correctly
(keychain via `credentials/mod.rs:810-862` `load_with_source` → hydrated into the runtime enum at
`settings/mod.rs:1648-1652`) and transmitted correctly (`asr/deepgram.rs:506`), and Deepgram
returns 401 — meaning the key is **stale / revoked / expired server-side**. Root cause location =
Deepgram provider account (the credential value), not this repo.

## Suspect commit

None for the 401. (Secondary, unrelated: the write-path ACL abort regression in `8a603e4` forward
from best-effort `set_owner_only`, plus save-path churn `de53a41` — these cause the WARN spam only.)

---

## Minimal next step (do NOT implement here)

1. **Decisive live test (run first, one command):** curl Deepgram with the exact stored key.
   ```
   curl -sS -o /dev/null -w '%{http_code}\n' \
     -H 'Authorization: Token <DEEPGRAM_KEY_REDACTED>' \
     'https://api.deepgram.com/v1/listen?model=nova-3' \
     --data-binary @/dev/null
   ```
   - **401** → key is genuinely revoked/expired → **Classification B confirmed**: user re-enters /
     rotates the Deepgram key in Settings (rewrites the keychain via `DefaultCredentialBackend::set`).
   - **200/400-not-401** → key is valid → re-open as an on-the-wire regression (unlikely given the
     clean header above) and diff the WS handshake.

2. **Ship a small diagnostic regardless (makes this debuggable next time, ~1 line):** at
   `connect()` entry in `asr/deepgram.rs` (right after the empty-key guard, ~line 262) log
   `log::debug!("Deepgram connect: key len={}", self.config.api_key.len())` and, on the
   `connect_async` error, if the tungstenite error is HTTP 401 surface a typed message
   *"Deepgram auth failed (401): key rejected — re-enter the key in Settings"* instead of the raw
   `WebSocket connect failed: …` string (redacted diagnostic currently at `deepgram.rs:520-525`).
   This disambiguates "empty key" vs "revoked key" from logs alone in the future (the current
   diagnostic gap, `/tmp/dg401-code.md` Hop 5 / `/tmp/dg401-logs.md` §2).

3. **Separately (not the 401):** make the Windows credential-state SAVE ACL hardening best-effort
   again — stop propagating `try_set_owner_only(...)?` in `credentials/mod.rs:537,543,638,643,1199`
   and remove a leftover `credentials-state.yaml.tmp` before `create_new`, to end the WARN spam.

## The ONE failing test / repro that would prove the fix

- **Primary proof (fix = user re-keys):** the live curl above returning **200** after the user
  rotates the key at the Deepgram dashboard and re-enters it in Settings. That is the only test
  that distinguishes "revoked" from "valid".
- **Regression guard for the credential resolver** (already green, cited to prove the read path is
  not the culprit): `edited_credentials_yaml_overrides_migrated_keychain_value` in
  `credentials/mod.rs` — a configured/migrated key resolves to the correct non-empty value with
  source `file_override`/keychain, never empty.
- **New guard to add with the diagnostic:** an integration/unit test asserting that a configured
  `DeepgramConfig` produces a non-empty `Authorization: Token <key>` header (guarding
  `open_ws`/header construction so a future refactor can't silently empty it).
