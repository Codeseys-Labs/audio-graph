/**
 * Credential keys that can satisfy each cloud stage in the durable notes/graph
 * path. `App` suppresses Express Setup when saved presence can cover both cloud
 * ASR and cloud LLM. The only approved single-key shortcut is OpenAI-compatible
 * ASR + LLM via `openai_api_key`; other shared-mode keys are ambiguous because
 * they may represent realtime-agent-only setup.
 *
 * Source of truth for the frontend "runnable durable cloud pair" gate. Kept in
 * its own module (rather than inline in `App.tsx`) so the cross-language
 * contract test can import the ACTUAL constants without pulling in `App`'s heavy
 * mount graph.
 *
 * SYNC (cred-review m5): every key in these two sets MUST also appear in
 * `DEMO_CREDENTIAL_KEYS` (src-tauri/src/settings/mod.rs) — otherwise a user
 * whose only key is that provider is wrongly forced into demo mode on first
 * launch. This is enforced from BOTH sides: the Rust test
 * `demo_credential_keys_superset_of_durable_cloud_pair` and the frontend
 * contract test `credentialKeysDemoSuperset.test.ts`, which reads THIS module
 * and the real Rust `DEMO_CREDENTIAL_KEYS` literal so a frontend-side addition
 * here can no longer silently drift.
 */

export const DURABLE_CLOUD_ASR_CREDENTIAL_KEYS = new Set<string>([
  "openai_api_key",
  "gemini_api_key",
  "deepgram_api_key",
  "assemblyai_api_key",
  "soniox_api_key",
  "gladia_api_key",
  "speechmatics_api_key",
  "revai_api_key",
  "aws_access_key",
]);

export const DURABLE_CLOUD_LLM_CREDENTIAL_KEYS = new Set<string>([
  "openai_api_key",
  "cerebras_api_key",
  "openrouter_api_key",
  "groq_api_key",
  "together_api_key",
  "fireworks_api_key",
  "gemini_api_key",
  "aws_access_key",
]);
