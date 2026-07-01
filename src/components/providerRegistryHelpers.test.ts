import { describe, expect, it } from "vitest";
import {
  generatedModelCatalogForProvider,
  implementedProviderOptionsForStage,
  modelCatalogForProvider,
  PROVIDER_DESCRIPTORS,
  providerCapabilityCredentialLabel,
  providerCredentialKeysLabel,
  providerDescriptorForSettingsVariant,
  providerIdForSettingsVariant,
  providerNotSelectableLabel,
  providerRoadmapAuthLabel,
  providerStatusLabel,
} from "./providerRegistryHelpers";

describe("providerRegistryHelpers", () => {
  it("derives implemented ASR settings options from the generated registry", () => {
    const options = implementedProviderOptionsForStage("asr", [
      "local_whisper",
      "api",
      "openai_realtime",
      "soniox",
      "moonshine",
    ] as const);

    expect(options.map((option) => option.value)).toEqual([
      "local_whisper",
      "api",
      "openai_realtime",
    ]);
    expect(options.map((option) => option.label)).toContain(
      "OpenAI Realtime transcription",
    );
    expect(options.map((option) => option.value)).not.toContain("soniox");
    expect(options.map((option) => option.value)).not.toContain("moonshine");
  });

  it("looks up descriptors by stage and settings variant", () => {
    expect(providerDescriptorForSettingsVariant("llm", "openrouter")?.id).toBe(
      "llm.openrouter",
    );
    expect(
      providerDescriptorForSettingsVariant("asr", "openrouter"),
    ).toBeNull();
    expect(providerIdForSettingsVariant("asr", "openai_realtime")).toBe(
      "asr.openai_realtime",
    );
  });

  it("preserves provider audio attribution metadata for readiness UI", () => {
    expect(
      PROVIDER_DESCRIPTORS.get("asr.deepgram")?.audio_input?.attribution,
    ).toMatchObject({
      mode: "speaker",
      max_channels: 1,
      requires_source_native_channels: false,
    });
  });

  it("uses generated defaults as a model catalog fallback", () => {
    expect(generatedModelCatalogForProvider("asr.openai_realtime")).toEqual([
      {
        id: "gpt-realtime-whisper",
        display_name: "gpt-realtime-whisper",
        is_default: true,
      },
    ]);
  });

  it("uses fixed provider catalogs from generated registry metadata", () => {
    const catalog = generatedModelCatalogForProvider("tts.deepgram_aura");

    expect(catalog).toHaveLength(12);
    expect(catalog[0]).toEqual({
      id: "aura-asteria-en",
      display_name: "Asteria (en, female)",
      is_default: true,
    });
    expect(catalog.map((item) => item.id)).toContain("aura-zeus-en");

    const cerebrasCatalog = generatedModelCatalogForProvider("llm.cerebras");
    expect(cerebrasCatalog).toEqual([
      {
        id: "gpt-oss-120b",
        display_name: "OpenAI GPT OSS 120B",
        is_default: true,
      },
      {
        id: "zai-glm-4.7",
        display_name: "Z.ai GLM 4.7 (preview)",
        is_default: false,
      },
    ]);
  });

  it("prefers backend readiness catalogs over generated fallback catalogs", () => {
    const catalog = modelCatalogForProvider(
      {
        "asr.openai_realtime": {
          provider_id: "asr.openai_realtime",
          status: "ready",
          message: "ready",
          stale: false,
          credential_epoch: 1,
          credentials: [],
          model_catalog: [
            {
              id: "custom-realtime",
              display_name: "Custom realtime",
              is_default: false,
            },
          ],
        },
      },
      "asr.openai_realtime",
    );

    expect(catalog).toEqual([
      {
        id: "custom-realtime",
        display_name: "Custom realtime",
        is_default: false,
      },
    ]);
  });

  it("prefers typed backend voice catalogs for TTS voice pickers", () => {
    const catalog = modelCatalogForProvider(
      {
        "tts.deepgram_aura": {
          provider_id: "tts.deepgram_aura",
          status: "ready",
          message: "ready",
          stale: false,
          credential_epoch: 1,
          credentials: [],
          model_catalog: [
            {
              id: "legacy-model-shaped-voice",
              display_name: "Legacy shaped voice",
              is_default: false,
            },
          ],
          voice_catalog: [
            {
              id: "aura-asteria-en",
              display_name: "Asteria",
              is_default: true,
            },
          ],
        },
      },
      "tts.deepgram_aura",
    );

    expect(catalog).toEqual([
      {
        id: "aura-asteria-en",
        display_name: "Asteria",
        is_default: true,
      },
    ]);
  });

  it("does not present required-not-wired watch auth as credentialless", () => {
    const xai = PROVIDER_DESCRIPTORS.get("asr.xai_grok_stt");
    if (!xai) throw new Error("xAI watch descriptor missing");
    expect(xai.credential_keys).toEqual([]);
    expect(xai.roadmap?.auth_schema).toBe("required_not_wired");

    expect(providerStatusLabel(xai.status)).toBe("Watch candidate");
    expect(providerCredentialKeysLabel(xai)).toBe(
      "Credential schema not wired",
    );
    expect(providerCapabilityCredentialLabel(xai, {})).toBe(
      "Auth required; credential schema not wired",
    );
    expect(providerRoadmapAuthLabel(xai)).toBe(
      "Auth required; credential schema not wired",
    );
    expect(providerNotSelectableLabel(xai)).toMatch(
      /watch candidates are not selectable/i,
    );
    expect(providerCapabilityCredentialLabel(xai, {})).not.toBe(
      "No credential required",
    );
  });

  it("keeps enterprise watch providers non-selectable without saved-key slots", () => {
    const nvidia = PROVIDER_DESCRIPTORS.get("asr.nvidia_nemotron_asr");
    if (!nvidia) throw new Error("NVIDIA enterprise watch descriptor missing");
    expect(nvidia.credential_keys).toEqual([]);
    expect(nvidia.roadmap?.auth_schema).toBe("required_not_wired");

    expect(providerStatusLabel(nvidia.status)).toBe("Enterprise watch");
    expect(providerCapabilityCredentialLabel(nvidia, {})).toBe(
      "Auth required; credential schema not wired",
    );
    expect(providerNotSelectableLabel(nvidia)).toMatch(
      /enterprise watch candidates are not selectable/i,
    );
  });
});
