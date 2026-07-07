/**
 * Contract test — generated endpoint credential router ⇄ its Rust source of
 * truth (provider-selection accuracy audit, 2026-07-05; single-source codegen
 * seed audio-graph-ed48).
 *
 * The endpoint→credential-slot routing used to be hand-maintained TWICE (Rust
 * `credential_key_for_endpoint` + a TS `endpointCredentialKey`) and only this
 * contract test kept them lockstep. It is now generated from ONE table —
 * `ENDPOINT_CREDENTIAL_ROUTING` in
 * `src-tauri/crates/ipc-contract/src/endpoint_credential_routing.rs` — into
 * `src/generated/endpointCredentialRouting.ts`, with a Rust drift test
 * (`generated_endpoint_credential_routing_ts_is_current`) that fails CI if the
 * committed TS diverges from the table. Drift is now structurally impossible
 * rather than merely tested.
 *
 * This test remains a second, cross-language layer of defense on the generated
 * artifact:
 *  1. Behavior vectors: the generated `endpointCredentialKey` maps each known
 *     endpoint to the expected slot.
 *  2. Vocabulary sync: the slot set reachable from the Rust source-of-truth
 *     table (extracted via `?raw`) equals the slot set the shared vectors know.
 *     A slot added to the Rust table without a vector here fails loudly.
 */

import { describe, expect, it } from "vitest";
// Vite `?raw` import of the Rust source-of-truth table (same pattern as
// credentialSourceContract.test.ts; typed by src/rust-raw.d.ts).
import rustRoutingSource from "../../../src-tauri/crates/ipc-contract/src/endpoint_credential_routing.rs?raw";
import { endpointCredentialKey } from "../settingsTypes";

/**
 * Endpoint → slot vectors the generated router must satisfy. Mirrors (and
 * extends with case/trailing-slash variants) the Rust test
 * `routing_covers_known_openai_compatible_hosts`.
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

/**
 * The union of every credential slot the Rust source-of-truth reaches: the
 * `credential_key:` fields of `ENDPOINT_CREDENTIAL_ROUTING` plus the
 * `DEFAULT_ENDPOINT_CREDENTIAL_KEY` fallback.
 */
function rustReachableSlots(): Set<string> {
  const slots = new Set(
    Array.from(
      rustRoutingSource.matchAll(/credential_key:\s*"(\w+_api_key)"/g),
      (match) => match[1],
    ),
  );
  const fallback = rustRoutingSource.match(
    /DEFAULT_ENDPOINT_CREDENTIAL_KEY:\s*&str\s*=\s*"(\w+_api_key)"/,
  );
  expect(
    fallback,
    "Rust DEFAULT_ENDPOINT_CREDENTIAL_KEY not found",
  ).not.toBeNull();
  if (fallback) slots.add(fallback[1]);
  return slots;
}

describe("endpoint credential routing contract — generated router ⇄ Rust table", () => {
  it("the generated router maps every shared endpoint vector to the same slot", () => {
    for (const [endpoint, slot] of SHARED_VECTORS) {
      expect(
        endpointCredentialKey(endpoint),
        `endpointCredentialKey(${endpoint})`,
      ).toBe(slot);
    }
  });

  it("the Rust source table reaches exactly the slot vocabulary the vectors know", () => {
    const rustSlots = rustReachableSlots();
    const tsSlots = new Set(SHARED_VECTORS.map(([, slot]) => slot));
    // A slot added to the Rust table but absent from the vectors means an
    // untested routing rule; a slot here but not in Rust means the frontend
    // expects a route the backend never emits.
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
