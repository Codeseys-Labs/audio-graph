SUCCESS: The process with PID 213752 (child process of PID 213476) has been terminated.
SUCCESS: The process with PID 213476 (child process of PID 211976) has been terminated.
SUCCESS: The process with PID 214396 (child process of PID 214216) has been terminated.
SUCCESS: The process with PID 214216 (child process of PID 214092) has been terminated.
SUCCESS: The process with PID 214092 (child process of PID 210368) has been terminated.
SUCCESS: The process with PID 210368 (child process of PID 211232) has been terminated.
SUCCESS: The process with PID 211232 (child process of PID 211976) has been terminated.
SUCCESS: The process with PID 214096 (child process of PID 211092) has been terminated.
SUCCESS: The process with PID 211092 (child process of PID 211976) has been terminated.
SUCCESS: The process with PID 216608 (child process of PID 216284) has been terminated.
SUCCESS: The process with PID 216284 (child process of PID 213808) has been terminated.
SUCCESS: The process with PID 213808 (child process of PID 213488) has been terminated.
SUCCESS: The process with PID 213488 (child process of PID 211976) has been terminated.
SUCCESS: The process with PID 217016 (child process of PID 214472) has been terminated.
SUCCESS: The process with PID 214472 (child process of PID 214592) has been terminated.
SUCCESS: The process with PID 214592 (child process of PID 211376) has been terminated.
SUCCESS: The process with PID 211376 (child process of PID 211976) has been terminated.
SUCCESS: The process with PID 213828 (child process of PID 213648) has been terminated.
SUCCESS: The process with PID 213648 (child process of PID 213320) has been terminated.
SUCCESS: The process with PID 213320 (child process of PID 213724) has been terminated.
SUCCESS: The process with PID 213724 (child process of PID 213588) has been terminated.
SUCCESS: The process with PID 213588 (child process of PID 211976) has been terminated.
SUCCESS: The process with PID 212540 (child process of PID 213060) has been terminated.
SUCCESS: The process with PID 213060 (child process of PID 213264) has been terminated.
SUCCESS: The process with PID 213264 (child process of PID 213508) has been terminated.
SUCCESS: The process with PID 213508 (child process of PID 213468) has been terminated.
SUCCESS: The process with PID 213468 (child process of PID 211976) has been terminated.
SUCCESS: The process with PID 216452 (child process of PID 210716) has been terminated.
SUCCESS: The process with PID 210716 (child process of PID 214672) has been terminated.
SUCCESS: The process with PID 214672 (child process of PID 212984) has been terminated.
SUCCESS: The process with PID 212984 (child process of PID 211976) has been terminated.
SUCCESS: The process with PID 212760 (child process of PID 213928) has been terminated.
SUCCESS: The process with PID 215212 (child process of PID 215096) has been terminated.
SUCCESS: The process with PID 213928 (child process of PID 27932) has been terminated.
SUCCESS: The process with PID 215096 (child process of PID 213444) has been terminated.
SUCCESS: The process with PID 27932 (child process of PID 214320) has been terminated.
SUCCESS: The process with PID 213444 (child process of PID 213456) has been terminated.
SUCCESS: The process with PID 213456 (child process of PID 211976) has been terminated.
SUCCESS: The process with PID 214320 (child process of PID 203420) has been terminated.
SUCCESS: The process with PID 203420 (child process of PID 211976) has been terminated.
CONFIRMED GOOD

- Gemini API-key WebSocket auth uses `x-goog-api-key` header, not `?key=` URL, and comments explicitly prohibit query-string keys (`src-tauri/src/gemini/mod.rs:726-737`). No Gemini URL logging found; only close/read errors are logged (`src-tauri/src/gemini/mod.rs:1163-1211`).
- Settings writes redact inline secrets before `settings.json` (`src-tauri/src/settings/mod.rs:682-719`, saved at `:912-935`).
- Tauri capability surface is minimal: only `"core:default"`; no shell/fs/http broad allowlist (`src-tauri/capabilities/default.json:8-10`). CSP only allows self + IPC (`src-tauri/tauri.conf.json:22-24`).
- No disabled TLS verification found; provider WebSockets are `wss://` except test-only `ws://` references.
- Session IDs are filename-safe (`src-tauri/src/sessions/mod.rs:37-50`).

ISSUES

- MED: Windows credential ACLs are not actually restricted. `set_owner_only` documents ΓÇ£owner-only ACL on WindowsΓÇ¥ but only clears readonly and relies on parent ACLs (`src-tauri/src/fs_util/mod.rs:16-26`). This affects credentials/settings/session artifacts written via `set_owner_only`.
- MED: Secret temp files can be briefly created with default permissions before chmod. `fs::write` creates/writes `credentials.yaml.tmp`, then chmods it (`src-tauri/src/credentials/mod.rs:156-163`); same pattern for settings (`src-tauri/src/settings/mod.rs:924-933`). On Unix with permissive umask this is a short exposure window; Windows lacks ACL hardening entirely.
- MED: User-configurable LLM endpoints receive transcript/graph context. `prepare_chat_request` includes recent transcript and graph entities (`src-tauri/src/commands.rs:1105-1137`), and API clients POST it to the configured endpoint (`src-tauri/src/llm/api_client.rs:177-192`). The UI exposes a raw endpoint field without a privacy warning (`src/components/LlmProviderSettings.tsx:185-195`), so a malicious ΓÇ£local-compatibleΓÇ¥ endpoint can exfiltrate sensitive session data.
- LOW: Debug/file logs can contain PII transcripts. File logging defaults enabled (`src-tauri/src/settings/mod.rs:483-520`), and debug logs print transcript text across ASR paths (`src-tauri/src/speech/mod.rs:1373-1378`, `:2024-2027`, `:2414-2434`). Active log files are opened without `set_owner_only` (`src-tauri/src/logging/mod.rs:232-254`).
- LOW: Session index paths are trusted. If `sessions.json` is tampered, metadata paths are used for load/delete without containment checks (`src-tauri/src/commands.rs:2584-2589`, `src-tauri/src/sessions/mod.rs:323-334`, delete at `src-tauri/src/commands.rs:2693-2707`).

QUESTIONS

- Should `app:<pid>` be numeric-validated like `process-tree:<pid>`? Currently it passes arbitrary strings to rsac (`src-tauri/src/commands.rs:59-63`).
- Is returning full credentials to the frontend via `load_all_credentials_cmd` intentional (`src-tauri/src/commands.rs:2939-2942`), or should it return presence/redacted values only?
