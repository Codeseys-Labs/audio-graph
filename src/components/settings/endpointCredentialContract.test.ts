/**
 * Contract test — frontend `endpointCredentialKey` ⇄ backend
 * `credential_key_for_endpoint` (provider-selection accuracy audit,
 * 2026-07-05).
 *
 * The endpoint→credential-slot routing exists TWICE: once in Rust
 * (`src-tauri/src/settings/mod.rs::credential_key_for_endpoint`, used to
 * hydrate runtime credentials for whatever endpoint is persisted) and once in
 * TypeScript (`settingsTypes.ts::endpointCredentialKey`, used to decide which
 * slot the Settings form saves a typed key into). If they disagree, the user
 * saves a key into slot A while the runtime reads slot B — the provider then
 * fails auth with a key that "is definitely saved" (the exact
 * credential-shape of the historical Deepgram config-drift incidents).
 *
 * Two layers of defense:
 *  1. Shared behavior vectors: both sides must map each known endpoint to the
 *     same slot. The TS side is exercised directly; the Rust side's expected
 *     values are the committed vectors below, which mirror the Rust unit test
 *     `endpoint_credential_routing_covers_known_openai_compatible_hosts`.
 *  2. Vocabulary sync: the set of credential slots reachable from the Rust
 *     function body (extracted from the Rust source via `?raw`) must equal
 *     the set reachable from the TS function. A slot added on one side only
 *     fails here loudly.
 */

import { describe, expect, it } from "vitest";
// Vite `?raw` import of the Rust source-of-truth (same pattern as
// credentialSourceContract.test.ts; typed by src/rust-raw.d.ts).
import rustSettingsSource from "../../../src-tauri/src/settings/mod.rs?raw";
import { endpointCredentialKey } from "../settingsTypes";

/**
 * Endpoint → slot vectors both routers must agree on. Mirrors (and extends
 * with case/trailing-slash variants) the Rust test
 * `endpoint_credential_routing_covers_known_openai_compatible_hosts`.
 */
const SHARED_VECTORS: ReadonlyArray<[endpoint: string, slot: string]> = [
  ["https://api.openai.com/v1", "openai_api_key"],
  ["https://api.cerebras.ai/v1", "cerebras_api_key"],
  ["https://api.cerebras.ai/v1/", "cerebras_api_key"],
  ["HTTPS://API.CEREBRAS.AI/V1", "cerebras_api_key"],
  ["https://api.sambanova.ai/v1", "sambanova_api_key"],
  ["https://api.sambanova.ai/v1/", "sambanova_api_key"],
  ["https://openrouter.ai/api/v1", "openrouter_api_key"],
  ["https://api.groq.com/openai/v1", "groq_api_key"],
  ["https://api.together.xyz/v1", "together_api_key"],
  ["https://api.fireworks.ai/inference/v1", "fireworks_api_key"],
  ["https://generativelanguage.googleapis.com/v1beta/openai", "gemini_api_key"],
  // Unknown OpenAI-compatible endpoints fall through to the generic slot.
  ["http://localhost:11434/v1", "openai_api_key"],
  ["https://my-vllm.internal:8000/v1", "openai_api_key"],
];

/** Extract the body of `credential_key_for_endpoint` from the Rust source. */
function rustRoutingFunctionBody(): string {
  const start = rustSettingsSource.indexOf(
    "pub fn credential_key_for_endpoint",
  );
  expect(start, "Rust credential_key_for_endpoint not found").toBeGreaterThan(
    -1,
  );
  const end = rustSettingsSource.indexOf("\n}", start);
  expect(end).toBeGreaterThan(start);
  return rustSettingsSource.slice(start, end);
}

describe("endpoint credential routing contract — TS ⇄ Rust", () => {
  it("both routers map every shared endpoint vector to the same slot", () => {
    for (const [endpoint, slot] of SHARED_VECTORS) {
      expect(
        endpointCredentialKey(endpoint),
        `TS endpointCredentialKey(${endpoint})`,
      ).toBe(slot);
    }
  });

  it("the Rust router reaches exactly the slot vocabulary TS knows", () => {
    const body = rustRoutingFunctionBody();
    const rustSlots = new Set(
      Array.from(body.matchAll(/"(\w+_api_key)"/g), (match) => match[1]),
    );
    const tsSlots = new Set(SHARED_VECTORS.map(([, slot]) => slot));
    // A slot added in Rust but absent here means the frontend can't save
    // into it (missing routing); a slot here but not in Rust means the
    // frontend saves a key the runtime never reads.
    expect([...rustSlots].sort()).toEqual([...tsSlots].sort());
  });

  it("cerebras/sambanova detection is exact-host, not substring", () => {
    // Both sides use exact normalized comparison for these two (unlike the
    // substring checks for groq/together/etc.). A lookalike host must fall
    // through to the generic slot, not capture the dedicated one.
    expect(endpointCredentialKey("https://api.cerebras.ai.evil.com/v1")).toBe(
      "openai_api_key",
    );
    expect(endpointCredentialKey("https://cerebras-proxy.internal/v1")).toBe(
      "openai_api_key",
    );
  });
});
