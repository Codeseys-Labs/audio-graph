/**
 * Contract test — frontend durable-cloud credential keys ⊆ backend
 * `DEMO_CREDENTIAL_KEYS` (cred-review m5).
 *
 * `App` treats the union of `DURABLE_CLOUD_ASR_CREDENTIAL_KEYS` and
 * `DURABLE_CLOUD_LLM_CREDENTIAL_KEYS` (in `./credentialKeys`) as the credentials
 * that can drive a real cloud pipeline. The Rust `DEMO_CREDENTIAL_KEYS`
 * (src-tauri/src/settings/mod.rs) is the set of keys whose presence keeps the
 * app OUT of forced demo mode on first launch. If a durable cloud key is missing
 * from `DEMO_CREDENTIAL_KEYS`, a user whose ONLY key is that provider gets
 * wrongly flipped into demo mode, silently overwriting their provider choice.
 *
 * The Rust side already pins this with
 * `demo_credential_keys_superset_of_durable_cloud_pair`, but that test
 * hand-MIRRORS the two frontend sets as local Rust consts — so a frontend-side
 * ADDITION of a new durable cloud key (the real source of truth) can drift
 * without the Rust mirror being updated, and the drift ships. This test closes
 * that loop from the frontend: it imports the ACTUAL frontend constants and
 * reads the REAL Rust `DEMO_CREDENTIAL_KEYS` literal (via a Vite `?raw` import,
 * the same mechanism `credentialSourceContract.test.ts` uses), so adding a key
 * to `./credentialKeys` without adding it to the Rust list fails here.
 */

import { describe, expect, it } from "vitest";
import rustSettingsSource from "../src-tauri/src/settings/mod.rs?raw";
import {
  DURABLE_CLOUD_ASR_CREDENTIAL_KEYS,
  DURABLE_CLOUD_LLM_CREDENTIAL_KEYS,
} from "./credentialKeys";

/**
 * Extract the string literals in the Rust `DEMO_CREDENTIAL_KEYS` slice:
 *
 *   pub const DEMO_CREDENTIAL_KEYS: &[&str] = &[
 *       "openai_api_key",
 *       ...
 *   ];
 *
 * We slice from the declaration to the closing `];` and pull every
 * double-quoted `snake_case` key inside, rather than matching all quoted
 * strings in the file.
 */
function extractDemoCredentialKeys(rust: string): Set<string> {
  const declIdx = rust.indexOf("DEMO_CREDENTIAL_KEYS");
  if (declIdx < 0) {
    throw new Error(
      "Could not find DEMO_CREDENTIAL_KEYS in src-tauri/src/settings/mod.rs — " +
        "the constant was renamed or moved; update this contract test.",
    );
  }
  const openIdx = rust.indexOf("[", declIdx);
  const closeIdx = rust.indexOf("];", openIdx);
  if (openIdx < 0 || closeIdx < 0) {
    throw new Error(
      "Could not parse the DEMO_CREDENTIAL_KEYS slice body — the literal shape " +
        "changed; update this contract test.",
    );
  }
  const body = rust.slice(openIdx + 1, closeIdx);
  const keys = new Set<string>();
  for (const m of body.matchAll(/"([a-z0-9_]+)"/g)) {
    keys.add(m[1]);
  }
  return keys;
}

describe("durable cloud credential keys ⊆ DEMO_CREDENTIAL_KEYS contract (m5)", () => {
  const demoKeys = extractDemoCredentialKeys(rustSettingsSource);
  const durableCloudKeys = new Set<string>([
    ...DURABLE_CLOUD_ASR_CREDENTIAL_KEYS,
    ...DURABLE_CLOUD_LLM_CREDENTIAL_KEYS,
  ]);

  it("parses a non-empty DEMO_CREDENTIAL_KEYS from the Rust source-of-truth", () => {
    // Guards against the extractor silently matching nothing (which would make
    // the superset assertion vacuously pass). These anchor keys must be present.
    expect(demoKeys.size).toBeGreaterThan(5);
    for (const anchor of [
      "openai_api_key",
      "together_api_key",
      "fireworks_api_key",
    ]) {
      expect(demoKeys).toContain(anchor);
    }
  });

  it("has every frontend durable cloud key present in the backend DEMO_CREDENTIAL_KEYS", () => {
    const missing = [...durableCloudKeys].filter((key) => !demoKeys.has(key));
    expect(
      missing,
      `Frontend durable cloud credential keys missing from the Rust ` +
        `DEMO_CREDENTIAL_KEYS (src-tauri/src/settings/mod.rs): ${missing.join(", ")}. ` +
        `Add each to DEMO_CREDENTIAL_KEYS (and its demo_credential_slot match arm), ` +
        `or a user whose only key is one of these is wrongly forced into demo mode.`,
    ).toEqual([]);
  });
});
