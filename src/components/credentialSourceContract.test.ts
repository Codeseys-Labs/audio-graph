/**
 * Contract test — backend CredentialSnapshot source vocabulary ⇄ frontend
 * credential-source labels.
 *
 * The Rust credential backend stamps every credential with a `source` string
 * (`CredentialSnapshot::source_for(key)` → `CredentialPresence.source` over the
 * `load_credential_presence_cmd` IPC boundary). The frontend renders that
 * string via `credentialSourceLabel()` in `ProviderReadinessPanel.tsx`, which
 * only localizes the strings in its `LOCALIZED_CREDENTIAL_SOURCES` allow-list —
 * any other value falls through to the raw string (or the "unknown" label when
 * blank).
 *
 * This test reads the Rust source-of-truth
 * (`src-tauri/src/credentials/mod.rs`) and extracts every literal credential
 * `source` value the backend can emit, then asserts each one resolves to a
 * real, localized frontend label (not the raw passthrough, not blank). If the
 * backend gains a new `source` value with no frontend mapping the test fails
 * loudly — so a label gap can never ship silently.
 *
 * As of the ADR-0019 credential-source vocabulary work (Seeds a3d8 / 3ca3),
 * every backend source — including `file_override` (the BUG 7fc5
 * keychain-override path) — has a localized frontend label, so
 * `KNOWN_UNMAPPED_SOURCES` is intentionally empty. A NEW unmapped source
 * (anything the backend emits without a label) still fails the test loudly.
 */

import i18n from "i18next";
import { describe, expect, it } from "vitest";
// Vite `?raw` import pulls the Rust source-of-truth in as a string at build
// time — no node:fs / @types/node needed (the frontend tsconfig ships neither).
// The ambient module declaration below types the `.rs?raw` specifier.
import rustCredentialsSource from "../../src-tauri/src/credentials/mod.rs?raw";
import { credentialSourceLabel } from "./ProviderReadinessPanel";
import "../i18n";

/**
 * Backend source values that reach the frontend but are intentionally NOT yet
 * mapped to a localized label. Each entry MUST link to a filed finding/seed so
 * the gap is tracked rather than forgotten.
 *
 * Currently empty: every source the backend emits (including `file_override`,
 * the BUG 7fc5 keychain-override path) now resolves to a localized label via
 * `LOCALIZED_CREDENTIAL_SOURCES` (Seeds a3d8 / 3ca3). Add an entry here only as
 * a tracked, temporary gap — with a link to its filed finding — never as a way
 * to silence a real label that ought to exist.
 */
const KNOWN_UNMAPPED_SOURCES = new Set<string>([]);

/**
 * Extract the credential `source` string literals the backend can stamp onto a
 * `CredentialSnapshot` (and therefore onto `CredentialPresence.source`).
 *
 * Sources of truth in `mod.rs`:
 *   - `CredentialSnapshot::new(store, "X")`           — whole-store source
 *   - `CredentialSnapshot::with_key_sources(_, "X",)` — whole-store source
 *   - `key_sources.insert(key, "X")`                  — per-key override source
 *   - `source_for` early-return `"missing"` sentinel
 *   - the `source_label()` returns of the YAML/keychain backends that feed the
 *     constructors above (`credentials_yaml`, `os_keychain`)
 *
 * The `#[cfg(test)]` module is stripped first so test-only fixtures don't
 * pollute the vocabulary. The trait-level `DefaultCredentialBackend`
 * `source_label()` ("credential_backend") is excluded because it only ever
 * appears in a log line (`load_or_default`), never as a snapshot source.
 */
function extractBackendSources(rust: string): Set<string> {
  const testModIdx = rust.search(/#\[cfg\(test\)\]\s*\nmod tests/);
  const prod = testModIdx >= 0 ? rust.slice(0, testModIdx) : rust;

  const sources = new Set<string>();
  const collect = (re: RegExp) => {
    for (const m of prod.matchAll(re)) sources.add(m[1]);
  };

  collect(/CredentialSnapshot::new\([^,]+,\s*"([a-z_]+)"/g);
  collect(/with_key_sources\([^,]+,\s*"([a-z_]+)"/g);
  collect(/key_sources\.insert\([^,]+,\s*"([a-z_]+)"\)/g);
  collect(/source_for\([^)]*\)[^{]*\{[^}]*?return\s+"([a-z_]+)";/gs);

  // The constructor calls above pass `self.yaml.source_label()` /
  // `self.keychain.source_label()` rather than string literals, so pull those
  // two backend labels in explicitly from their `source_label` bodies.
  const labelBlocks = [
    /impl CredentialBackend for YamlCredentialBackend\b[\s\S]*?fn source_label\(&self\)[^{]*\{\s*"([a-z_]+)"/,
    /impl<S: KeychainStore> CredentialBackend for KeychainCredentialBackend<S>[\s\S]*?fn source_label\(&self\)[^{]*\{\s*"([a-z_]+)"/,
  ];
  for (const re of labelBlocks) {
    const m = prod.match(re);
    if (m) sources.add(m[1]);
  }

  return sources;
}

describe("credential source ⇄ label contract", () => {
  const t = i18n.getFixedT("en");
  const backendSources = extractBackendSources(rustCredentialsSource);

  it("extracts the known backend source vocabulary from the Rust source-of-truth", () => {
    // Guards against the regex silently matching nothing (which would make the
    // mapping assertions vacuously pass). These are the values present today.
    for (const expected of [
      "os_keychain",
      "credentials_yaml",
      "file_fallback",
      "imported_file",
      "file_override",
      "missing",
    ]) {
      expect(backendSources).toContain(expected);
    }
  });

  it("maps every mapped backend credential source to a localized frontend label", () => {
    const unmapped: string[] = [];
    for (const source of backendSources) {
      if (KNOWN_UNMAPPED_SOURCES.has(source)) continue;
      const label = credentialSourceLabel(source, t);
      // A real label must be non-blank AND must not be the raw passthrough of
      // the source key itself (which is what credentialSourceLabel returns for
      // any value outside LOCALIZED_CREDENTIAL_SOURCES).
      if (!label.trim() || label === source) unmapped.push(source);
    }
    expect(
      unmapped,
      `Backend credential sources with no frontend label mapping: ${unmapped.join(
        ", ",
      )}. Add a 'settings.providerReadiness.credentialSource.<source>' i18n key and list the source in LOCALIZED_CREDENTIAL_SOURCES (ProviderReadinessPanel.tsx), or — if the label is genuinely unknown — file a finding and add it to KNOWN_UNMAPPED_SOURCES with a tracking link.`,
    ).toEqual([]);
  });

  it("does not silently mask an unmapped source — known gaps are tracked, not labelled", () => {
    // Every entry in the known-gap allow-list must really be a backend source
    // (otherwise it's stale and should be removed), and must really lack a
    // frontend label (otherwise it should be removed from the allow-list and
    // treated as mapped).
    for (const source of KNOWN_UNMAPPED_SOURCES) {
      expect(
        backendSources,
        `KNOWN_UNMAPPED_SOURCES lists "${source}" but the backend no longer emits it — remove the stale entry.`,
      ).toContain(source);
      const label = credentialSourceLabel(source, t);
      expect(
        label === source,
        `KNOWN_UNMAPPED_SOURCES lists "${source}" but it now HAS a real frontend label ("${label}") — remove it from the allow-list so it is treated as mapped.`,
      ).toBe(true);
    }
  });
});
