/**
 * Contract test — Settings provider variant lists ⇄ generated provider
 * registry (provider-selection accuracy audit, 2026-07-05).
 *
 * The Settings modal offers providers from hand-maintained variant lists
 * (`ASR_PROVIDER_SETTINGS_VARIANTS` etc. in `useSettingsController.tsx`),
 * intersected with the backend-generated registry
 * (`src/generated/providerRegistry.ts`, itself pinned to the Rust registry
 * crate by `generated_provider_registry_ts_is_current`). Two silent failure
 * modes live in that intersection:
 *
 *  1. A typo'd / stale UI variant string matches nothing in the registry —
 *     the provider silently disappears from Settings (missing option).
 *  2. A registry provider is promoted to `implemented` but nobody adds it to
 *     the UI list — the backend supports it, the UI never offers it.
 *
 * This test freezes both directions. The exported *_PROVIDER_OPTIONS arrays
 * are module-level constants, so asserting on them exercises exactly what the
 * radio groups render.
 */

import { describe, expect, it } from "vitest";
import { GENERATED_PROVIDER_REGISTRY } from "../../generated/providerRegistry";
import type { ProviderStage } from "../../types";
import { defaultModelForProvider } from "../providerRegistryHelpers";
import {
  ASR_PROVIDER_OPTIONS,
  LLM_PROVIDER_OPTIONS,
  TTS_PROVIDER_OPTIONS,
} from "./useSettingsController";

/**
 * The UI variant vocabulary per stage, copied from the `AsrType` / `LlmType`
 * unions in `settingsTypes.ts` (+ `TtsType`). Kept as an explicit list here so
 * the test fails loudly when a union member is added or removed without
 * revisiting the registry contract below.
 */
const UI_VARIANTS: Record<"asr" | "llm" | "tts", readonly string[]> = {
  asr: [
    "local_whisper",
    "api",
    "openai_realtime",
    "aws_transcribe",
    "deepgram",
    "assemblyai",
    "soniox",
    "sherpa_onnx",
    "moonshine",
  ],
  llm: [
    "local_llama",
    "api",
    "cerebras",
    "sambanova",
    "openrouter",
    "aws_bedrock",
    "mistralrs",
  ],
  tts: ["none", "deepgram_aura"],
};

function registryVariantsForStage(
  stage: ProviderStage,
  status?: string,
): string[] {
  return GENERATED_PROVIDER_REGISTRY.filter(
    (provider) =>
      provider.stage === stage && (!status || provider.status === status),
  ).map((provider) => provider.settings_variant);
}

function selectableVariantsForStage(stage: ProviderStage): string[] {
  return GENERATED_PROVIDER_REGISTRY.filter(
    (provider) => provider.stage === stage && provider.ui_selectable,
  ).map((provider) => provider.settings_variant);
}

describe("provider variant contract — UI lists ⇄ generated registry", () => {
  it.each([
    "asr",
    "llm",
    "tts",
  ] as const)("every %s UI variant exists in the generated registry (no phantom options)", (stage) => {
    const registryVariants = new Set(registryVariantsForStage(stage));
    for (const variant of UI_VARIANTS[stage]) {
      expect(
        registryVariants.has(variant),
        `UI ${stage} variant "${variant}" has no registry descriptor — ` +
          "it can never resolve a label/status and silently vanishes " +
          "from Settings",
      ).toBe(true);
    }
  });

  it.each([
    "asr",
    "llm",
    "tts",
  ] as const)("every implemented %s registry provider is listed by the UI vocabulary (no missing options)", (stage) => {
    // The UI variant vocabulary must still name every implemented provider even
    // when it is deferred (ui_selectable=false): the variant string still has
    // to resolve a descriptor/label for saved-settings load and the capability
    // panel, and re-enabling a provider must not also require re-adding the
    // string here.
    const uiVariants = new Set(UI_VARIANTS[stage]);
    for (const variant of registryVariantsForStage(stage, "implemented")) {
      expect(
        uiVariants.has(variant),
        `registry ${stage} provider "${variant}" is implemented but ` +
          "missing from the UI variant list — its label/route can never resolve",
      ).toBe(true);
    }
  });

  it("the rendered option lists are exactly the ui_selectable UI variants", () => {
    // The module-level *_PROVIDER_OPTIONS constants are what the radio groups
    // actually render; they must be the UI list filtered to the `ui_selectable`
    // axis (NOT `status`), in UI-list order. This is the front-line assertion
    // that a deferred-but-implemented provider (MVP scoping, audio-graph-ad56)
    // stays out of the picker.
    const expectSelectable = (stage: "asr" | "llm" | "tts") => {
      const selectable = new Set(selectableVariantsForStage(stage));
      return UI_VARIANTS[stage].filter((variant) => selectable.has(variant));
    };
    expect(ASR_PROVIDER_OPTIONS.map((option) => option.value)).toEqual(
      expectSelectable("asr"),
    );
    expect(LLM_PROVIDER_OPTIONS.map((option) => option.value)).toEqual(
      expectSelectable("llm"),
    );
    expect(TTS_PROVIDER_OPTIONS.map((option) => option.value)).toEqual(
      expectSelectable("tts"),
    );
    // Concretely: ASR collapses to Deepgram only, while LLM/TTS keep their full
    // implemented set (all ui_selectable under the current MVP decision).
    expect(ASR_PROVIDER_OPTIONS.map((option) => option.value)).toEqual([
      "deepgram",
    ]);
  });

  it("frontend initial model defaults come from the registry default_model", () => {
    // `initialSettingsState` seeds model fields from
    // `defaultModelForProvider(...)`; the registry values are the backend's
    // source of truth. Pin the ones with a wire-visible model id so a
    // registry default change (e.g. nova-3 → nova-4) propagates to fresh
    // forms — and, critically, that the Deepgram default is never the
    // legacy `general`.
    expect(defaultModelForProvider("asr.deepgram")).toBe("nova-3");
    expect(defaultModelForProvider("asr.soniox")).toBe("stt-rt-v5");
    expect(defaultModelForProvider("asr.openai_realtime")).not.toBe("");
    expect(defaultModelForProvider("tts.deepgram_aura")).not.toBe("");
  });
});
