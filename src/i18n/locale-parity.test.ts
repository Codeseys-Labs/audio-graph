import { describe, expect, it } from "vitest";

import en from "./locales/en.json";
import pt from "./locales/pt.json";

// Automated key-parity guard for the shipped locales. Translations may differ
// in value, but the *shape* (every nested key path) must match exactly — a
// missing key silently falls back to "en" at runtime with no compile error,
// so this test is the only thing that catches drift when someone adds an "en"
// string and forgets the "pt" mirror (or vice versa).
//
// Add new locales to the `locales` table below and they get parity-checked
// against the canonical "en" key set for free.

type JsonValue =
  | string
  | number
  | boolean
  | null
  | JsonValue[]
  | { [key: string]: JsonValue };

/** Flatten a nested locale object into its set of dotted leaf key paths. */
function flattenKeys(value: JsonValue, prefix = ""): string[] {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    return [prefix];
  }
  const keys: string[] = [];
  for (const [key, child] of Object.entries(value)) {
    const path = prefix ? `${prefix}.${key}` : key;
    keys.push(...flattenKeys(child as JsonValue, path));
  }
  return keys;
}

const enKeys = new Set(flattenKeys(en as JsonValue));

const locales: Record<string, JsonValue> = { pt: pt as JsonValue };

describe("i18n locale key parity", () => {
  for (const [name, locale] of Object.entries(locales)) {
    it(`${name}.json has the same key set as en.json`, () => {
      const localeKeys = new Set(flattenKeys(locale));
      const missingInLocale = [...enKeys]
        .filter((k) => !localeKeys.has(k))
        .sort();
      const extraInLocale = [...localeKeys]
        .filter((k) => !enKeys.has(k))
        .sort();

      expect(
        missingInLocale,
        `${name}.json is missing keys present in en.json`,
      ).toEqual([]);
      expect(
        extraInLocale,
        `${name}.json has keys not present in en.json`,
      ).toEqual([]);
    });
  }
});
