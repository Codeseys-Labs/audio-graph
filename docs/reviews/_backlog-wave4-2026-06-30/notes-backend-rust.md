# Backlog Wave-4 — lane `backend-rust`

Worktree: `.claude/worktrees/wf_c2a2bd59-c6f-2`
Base: `8962dab` (verified via STEP ZERO reset; FieldRow.tsx + bedrock.rs present, content-egress guard present).

## Item 7e0b (MED task) — Extend the content-egress guard to remaining provider senders

### What I found first (read bedrock.rs + the whole egress surface)

The `ProviderContentEgressPolicy` defense-in-depth pattern (landed for Bedrock
in 3b9f at `src-tauri/src/llm/bedrock.rs`) is already applied broadly across the
codebase. Before writing any code I audited every sender named in the item:

| Sender | Guard on the SEND path? | Direct-client block test? |
|---|---|---|
| ASR Deepgram (audio send) | YES — `send_audio` `check_audio` + `AsrWsWriteGuard` on `run_io` write | YES (`blocked_policy_*`, `run_io_blocked_policy_sends_no_audio_content_frame`) |
| ASR AssemblyAI (audio send) | YES — same shape | YES (`run_io_blocked_policy_writes_no_audio_frame`) |
| ASR OpenAI Realtime (audio send) | YES — `AsrWsWriteGuard` on session-update + audio-append | YES (`open_ws_blocked_policy_*`, `run_io_blocked_policy_writes_no_audio_append_frame`) |
| ASR Soniox (audio send) | YES — `AsrWsWriteGuard` | YES (`open_ws_*`, `run_io_blocked_policy_writes_no_audio_frame`) |
| Deepgram Aura TTS (`speak`) | YES — `speak()` `check_text` | YES (`speak_rejects_blocked_policy_*`, `default_session_policy_rejects_speak_without_queueing_text`) |
| Gemini Live `send_audio` (public) | YES — `send_audio` `check_audio("gemini.live")` | YES (`blocked_policy_rejects_non_empty_audio_before_channel_initialization`) |
| OpenRouter request builder | YES — `chat_completion` `check_prompt("llm.openrouter")` | YES (`blocked_policy_rejects_chat_completion_before_http_request`) |
| Generic LLM API request builder | YES — `chat_completion_inner` `check_prompt("llm.api")` | YES (`blocked_policy_rejects_chat_completion_before_http_request`) |
| Bedrock LLM request | YES — adapter `run()` re-checks (3b9f) | YES (3b9f tests) |
| Streaming HTTP router | YES — `check_streaming_http_content_egress` | YES |
| Projection / extraction request paths | YES (transitive) — route through `ApiClient`/`OpenRouterClient`/Bedrock, all guarded above; `extraction.rs` makes no direct HTTP call | covered by the client-level tests |

So the high-level public senders were already covered. The genuine remaining
gap was at the **inner WS write-loop layer** for the two providers that own a
bespoke `run_io` write loop (rather than routing through the shared
`AsrWsWriteGuard`):

1. **Gemini `run_io`** wrote `realtimeInput.audio` Chunk frames to the socket
   with NO defense-in-depth re-check. The check lived only in `send_audio`
   (which enqueues into the channel). A direct caller that drives `run_io` or
   feeds `audio_rx` bypassing `send_audio` could ship audio bytes.
2. **Deepgram Aura `run_io`** wrote `Speak` text frames to the socket with NO
   re-check. The check lived only in `speak()` before enqueue. A direct caller
   that pushes a `SessionCmd::Speak` bypassing `speak()` could ship synthesis
   text.

This is exactly the second-layer gap the bedrock work targets (router checks,
adapter re-checks). The four ASR providers already close this gap because their
write loops go through `AsrWsWriteGuard::send_text`/`send_binary`, which
re-checks the policy at the write primitive.

### What I changed

- `src-tauri/src/gemini/mod.rs`
  - Threaded `content_egress_policy` from the session `config` → `session_task`
    → `run_io`.
  - Added a defense-in-depth `check_audio("gemini.live")` gate before the audio
    `Chunk` WS write. On a blocked policy the frame is dropped (`continue`)
    WITHOUT tearing down the socket — a blocked policy is a steady-state
    condition, not a transport failure to reconnect around. The terminal
    `audioStreamEnd` control frame (sent on Stop) carries no audio content and
    stays allowed.
  - New test `run_io_blocked_policy_writes_no_audio_frame`: drives `run_io`
    directly with a blocked policy + a payload-bearing Chunk, asserts the
    server socket never receives an audio DATA frame nor the secret bytes (only
    the terminal `audioStreamEnd` control frame is permitted).

- `src-tauri/src/tts/deepgram_aura.rs`
  - Threaded `content_egress_policy` into `SessionCtx` → `session_task` →
    `run_io`.
  - Added a defense-in-depth `check_text(AURA_POLICY_PROVIDER)` gate before the
    `Speak` WS write. On a blocked policy the frame is dropped (`continue`)
    WITHOUT tearing down the socket. The payload text is never interpolated into
    any error.
  - New test `run_io_blocked_policy_writes_no_speak_frame`: drives `run_io`
    directly with a blocked policy + a payload-bearing `SessionCmd::Speak`,
    asserts the server socket never receives a `Speak` frame.

### Acceptance (met)

- Each covered sender refuses content egress under a blocked policy with a test.
- No-content readiness/model-catalog probes stay ALLOWED (untouched; the probe
  policy path in `commands.rs` uses `requires_cloud_content_transfer = false`).
- Errors stay redacted (no payload/secret interpolation — verified by the new
  tests asserting the secret bytes/text never reach the wire).

The highest-risk content paths named as the minimum scope (ASR-audio-send,
TTS-send, Gemini-send, LLM-request-builder) are ALL covered with tests. No
remainder seed was required — the only missing pieces were the two inner-loop
re-checks above; everything else was already guarded + tested in the base.

## Item 559d (MED feature) — Migrate user (non-secret) settings to config.yaml

**Already fully implemented and committed in the base (8962dab).** No code
change required. The settings persistence layer in `src-tauri/src/settings/mod.rs`
already implements the entire acceptance surface:

- `get_settings_path` → `app_config_dir()/config.yaml` (canonical);
  `get_legacy_settings_json_path` → `app_data_dir()/settings.json` (legacy).
- `load_settings_from_paths_with_status`: read config.yaml if present, else
  import settings.json ONCE (writing canonical config.yaml via `persist_import`),
  else defaults. Status enum (`CanonicalOk` / `LegacyImported` /
  `CanonicalErrorDefaulted` / `LegacyErrorDefaulted` / `DefaultsMissing` /
  `PathErrorDefaulted`) drives writeback gating.
- Atomic YAML writes via `.yaml.tmp` + `fs::rename` + owner-only perms
  (`save_settings_to_path`).
- Corrupt-YAML fallback: a non-parseable config.yaml yields
  `CanonicalErrorDefaulted` and is left untouched (no legacy import, no
  overwrite); `ensure_existing_config_is_parseable_for_write` refuses to clobber
  a corrupt file on save.
- Secrets stay ONLY in credentials.yaml: `serialize_config_yaml` runs settings
  through `redacted_settings` so no inline credential field is ever written to
  config.yaml. `persist_inline_credentials` routes inline secrets to the
  credential store.
- Bundled `src-tauri/config/default.toml` remains the build default
  (`AppSettings::default`).

Existing tests already cover the full acceptance matrix:
- `config_yaml_round_trips_redacted_settings` (save/load round-trip)
- `legacy_settings_json_import_writes_canonical_config_yaml` (import once)
- `corrupt_config_yaml_falls_back_without_importing_legacy_json` (corrupt-YAML
  fallback)
- `save_settings_to_path_writes_redacted_yaml` / `assert_yaml_has_no_inline_secrets`
  (secrets stay out of config.yaml)
- `normal_save_refuses_to_overwrite_corrupt_config_yaml`
- `legacy_settings_json_import` / `missing_settings_reports_defaults_missing_status`

I verified the implementation and tests are present and not stubbed. There is
nothing to commit for 559d and no half-migrated path to clean up.

## Gate

- `cargo +1.95.0 clippy --lib --tests --no-default-features --features cloud -- -D warnings` — see commit gate run.
- `cargo +1.95.0 test --lib --no-default-features --features cloud run_io_blocked_policy -- --test-threads=1` — 6 passed (incl. both new tests).
- `rustfmt --edition 2024 --check src/gemini/mod.rs src/tts/deepgram_aura.rs` — exit 0.

## New seeds filed

None required. Both items resolved within scope.
