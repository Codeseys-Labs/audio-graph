import { describe, expect, it } from "vitest";
import type {
  CredentialPresence,
  ProviderReadiness,
  ProviderRuntimeReadiness,
} from "../types";
import {
  deriveProviderSetupModeCards,
  type ProviderSetupAudioSource,
  type ProviderSetupModeCard,
  type ProviderSetupModeId,
  type ProviderSetupProviderSelection,
  type ProviderSetupSourceState,
  providerSetupSourceRecoveryIssues,
} from "./providerSetupModes";
import { initialSettingsState, type SettingsState } from "./settingsTypes";

function settings(patch: Partial<SettingsState> = {}): SettingsState {
  return { ...initialSettingsState, ...patch };
}

function presence(...keys: string[]): CredentialPresence[] {
  return keys.map((key) => ({
    key,
    present: true,
    source: "credentials_yaml",
  }));
}

function readyProvider(
  providerId: string,
  credentials: readonly string[] = [],
  runtime?: ProviderRuntimeReadiness,
): ProviderReadiness {
  return {
    provider_id: providerId,
    status: "ready",
    message: "ready",
    stale: false,
    credential_epoch: 1,
    credentials: credentials.map((key) => ({ key, present: true })),
    model_catalog: [],
    runtime: runtime ?? null,
  };
}

function source(
  patch: Partial<ProviderSetupAudioSource> = {},
): ProviderSetupAudioSource {
  return {
    id: "system-default",
    name: "System audio",
    source_type: { type: "SystemDefault" },
    capture_target: "system-default",
    capabilities: {
      backend_name: "FixtureBackend",
      capture_supported: true,
      supports_system_capture: true,
      supports_application_capture: true,
      supports_process_tree_capture: true,
      supports_device_selection: true,
      supports_device_change_notifications: true,
      unsupported_reason: null,
    },
    permission_status: "NotRequired",
    ...patch,
  };
}

function sourceState(
  selectedSourceIds: readonly string[],
  sources: readonly ProviderSetupAudioSource[],
): ProviderSetupSourceState {
  return { selectedSourceIds, sources };
}

function byId(
  cards: readonly ProviderSetupModeCard[],
  id: ProviderSetupModeId,
): ProviderSetupModeCard {
  const card = cards.find((item) => item.id === id);
  expect(card).toBeDefined();
  return card as ProviderSetupModeCard;
}

function providerIds(card: ProviderSetupModeCard): string[] {
  return card.selectedProviders.map((provider) => provider.providerId);
}

function providerById(
  card: ProviderSetupModeCard,
  providerId: string,
): ProviderSetupProviderSelection {
  const provider = card.selectedProviders.find(
    (selection) => selection.providerId === providerId,
  );
  expect(provider).toBeDefined();
  return provider as ProviderSetupProviderSelection;
}

describe("deriveProviderSetupModeCards", () => {
  it("reports credential blockers for a no-key cloud setup", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "deepgram",
        deepgramModel: "nova-3",
        llmType: "openrouter",
        openrouterModel: "openai/gpt-4o-mini",
      }),
    });

    const cloud = byId(cards, "cloud_fast");
    const native = byId(cards, "native_realtime");

    expect(cloud.selected).toBe(true);
    expect(cloud.readinessStatus).toBe("missing_credentials");
    expect(providerIds(cloud)).toEqual([
      "asr.deepgram",
      "llm.openrouter",
      "tts.none",
    ]);
    expect(cloud.missingBlockers).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: "missing_credential",
          providerId: "asr.deepgram",
          key: "deepgram_api_key",
        }),
        expect.objectContaining({
          kind: "missing_credential",
          providerId: "llm.openrouter",
          key: "openrouter_api_key",
        }),
      ]),
    );
    expect(native.missingBlockers).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          providerId: "realtime_agent.gemini_live",
          key: "gemini_api_key",
        }),
      ]),
    );
  });

  it("keeps readiness-only present credentials present with an unknown source", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "deepgram",
        deepgramModel: "nova-3",
        llmType: "openrouter",
        openrouterModel: "openai/gpt-4o-mini",
      }),
      providerReadiness: [
        readyProvider("asr.deepgram", ["deepgram_api_key"]),
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
    });

    const deepgram = providerById(byId(cards, "cloud_fast"), "asr.deepgram");

    expect(deepgram.credentials).toEqual([
      { key: "deepgram_api_key", present: true, source: "" },
    ]);
    expect(deepgram.credentials[0]?.source).not.toBe("credentials_yaml");
  });

  it("keeps readiness-only missing credentials missing", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "deepgram",
        deepgramModel: "nova-3",
        llmType: "openrouter",
        openrouterModel: "openai/gpt-4o-mini",
      }),
      providerReadiness: [
        {
          provider_id: "asr.deepgram",
          status: "missing_credentials",
          message: "Missing saved credential(s): deepgram_api_key",
          stale: false,
          credential_epoch: 1,
          credentials: [{ key: "deepgram_api_key", present: false }],
          model_catalog: [],
          runtime: null,
        },
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
    });

    const deepgram = providerById(byId(cards, "cloud_fast"), "asr.deepgram");

    expect(deepgram.credentials).toEqual([
      { key: "deepgram_api_key", present: false, source: "missing" },
    ]);
  });

  it("keeps boolean presence-only credentials present with an unknown source", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "deepgram",
        deepgramModel: "nova-3",
        llmType: "openrouter",
        openrouterModel: "openai/gpt-4o-mini",
      }),
      credentialPresence: {
        deepgram_api_key: true,
        openrouter_api_key: true,
      },
      providerReadiness: [
        readyProvider("asr.deepgram", ["deepgram_api_key"]),
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
    });

    const deepgram = providerById(byId(cards, "cloud_fast"), "asr.deepgram");

    expect(deepgram.credentials).toEqual([
      { key: "deepgram_api_key", present: true, source: "" },
    ]);
    expect(deepgram.credentials[0]?.source).not.toBe("credentials_yaml");
  });

  it("prefers explicit credentialPresence sources over readiness fallbacks", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "deepgram",
        deepgramModel: "nova-3",
        llmType: "openrouter",
        openrouterModel: "openai/gpt-4o-mini",
      }),
      credentialPresence: [
        {
          key: "deepgram_api_key",
          present: true,
          source: "os_keychain",
        },
      ],
      providerReadiness: [
        readyProvider("asr.deepgram", ["deepgram_api_key"]),
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
    });

    const deepgram = providerById(byId(cards, "cloud_fast"), "asr.deepgram");

    expect(deepgram.credentials).toEqual([
      { key: "deepgram_api_key", present: true, source: "os_keychain" },
    ]);
  });

  it("marks local-only durable pipeline mode ready when local providers are ready", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "local_whisper",
        llmType: "local_llama",
        llmModel: "lfm2-350m-extract-q4_k_m.gguf",
      }),
      providerReadiness: [
        readyProvider("asr.local_whisper", [], {
          status: "healthy",
          message: "whisper model ready",
          model_id: "ggml-small.en.bin",
        }),
        readyProvider("llm.local_llama", [], {
          status: "healthy",
          message: "llm model ready",
          model_id: "lfm2-350m-extract-q4_k_m.gguf",
        }),
      ],
    });

    const local = byId(cards, "local_private");

    expect(local.selected).toBe(true);
    expect(local.dataBoundary).toBe("local_only");
    expect(local.dataLeavesDevice).toBe(false);
    expect(local.readinessStatus).toBe("ready");
    expect(local.missingBlockers).toEqual([]);
    expect(local.stageCoverage).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          role: "durable_transcription",
          providerId: "asr.local_whisper",
          model: "ggml-small.en.bin",
        }),
        expect.objectContaining({
          role: "durable_notes_graph",
          providerId: "llm.local_llama",
          model: "lfm2-350m-extract-q4_k_m.gguf",
        }),
      ]),
    );
  });

  it("classifies local ASR plus OpenRouter as hybrid without requiring ASR cloud credentials", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "local_whisper",
        llmType: "openrouter",
        openrouterModel: "anthropic/claude-3.5-haiku",
      }),
      credentialPresence: presence("openrouter_api_key"),
      providerReadiness: [
        readyProvider("asr.local_whisper", [], {
          status: "healthy",
          message: "whisper ready",
        }),
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
    });

    const hybrid = byId(cards, "hybrid");
    const cloud = byId(cards, "cloud_fast");

    expect(hybrid.selected).toBe(true);
    expect(hybrid.dataBoundary).toBe("mixed_local_cloud");
    expect(hybrid.readinessStatus).toBe("ready");
    expect(providerIds(hybrid)).toEqual([
      "asr.local_whisper",
      "llm.openrouter",
      "tts.none",
    ]);
    expect(cloud.readinessStatus).toBe("missing_credentials");
    expect(cloud.missingBlockers).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          providerId: "asr.deepgram",
          key: "deepgram_api_key",
        }),
      ]),
    );
  });

  it("marks Deepgram plus OpenRouter as cloud-fast with durable pipeline coverage", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "deepgram",
        deepgramModel: "nova-3",
        llmType: "openrouter",
        openrouterModel: "openai/gpt-4o-mini",
      }),
      credentialPresence: presence("deepgram_api_key", "openrouter_api_key"),
      providerReadiness: [
        readyProvider("asr.deepgram", ["deepgram_api_key"]),
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
    });

    const cloud = byId(cards, "cloud_fast");

    expect(cloud.selected).toBe(true);
    expect(cloud.dataBoundary).toBe("mixed_cloud");
    expect(cloud.dataLeavesDevice).toBe(true);
    expect(cloud.readinessStatus).toBe("ready");
    expect(providerIds(cloud)).toEqual([
      "asr.deepgram",
      "llm.openrouter",
      "tts.none",
    ]);
    expect(cloud.stageCoverage).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          path: "durable_pipeline",
          role: "durable_transcription",
          providerId: "asr.deepgram",
          model: "nova-3",
        }),
        expect.objectContaining({
          path: "durable_pipeline",
          role: "durable_notes_graph",
          providerId: "llm.openrouter",
          model: "openai/gpt-4o-mini",
        }),
        expect.objectContaining({
          path: "speech_output",
          role: "speech_output",
          providerId: "tts.none",
          covered: false,
        }),
      ]),
    );
  });

  it("preserves the selected Cerebras model in cloud-fast durable pipeline coverage", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "deepgram",
        deepgramModel: "nova-3",
        llmType: "cerebras",
        llmModel: "zai-glm-4.7",
      }),
      credentialPresence: presence("deepgram_api_key", "cerebras_api_key"),
      providerReadiness: [
        readyProvider("asr.deepgram", ["deepgram_api_key"]),
        readyProvider("llm.cerebras", ["cerebras_api_key"]),
      ],
    });

    const cloud = byId(cards, "cloud_fast");

    expect(providerIds(cloud)).toEqual([
      "asr.deepgram",
      "llm.cerebras",
      "tts.none",
    ]);
    expect(cloud.stageCoverage).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          path: "durable_pipeline",
          role: "durable_notes_graph",
          providerId: "llm.cerebras",
          model: "zai-glm-4.7",
        }),
      ]),
    );
  });

  it("keeps Gemini Live in the native realtime path and does not echo draft secrets", () => {
    const draftGeminiSecret = "AIza-draft-secret";
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        geminiApiKey: draftGeminiSecret,
        geminiModel: "gemini-2.0-flash-live-001",
      }),
      credentialPresence: presence("gemini_api_key"),
      providerReadiness: [
        readyProvider("realtime_agent.gemini_live", ["gemini_api_key"]),
      ],
      conversationMode: "converse",
      converseEngine: "native",
      nativeRealtimeEnabled: false,
    });

    const native = byId(cards, "native_realtime");

    expect(native.selected).toBe(true);
    expect(native.productPath).toBe("native_realtime_agent");
    expect(native.readinessStatus).toBe("ready");
    expect(providerIds(native)).toEqual(["realtime_agent.gemini_live"]);
    expect(native.stageCoverage).toEqual([
      expect.objectContaining({
        stage: "realtime_agent",
        path: "native_realtime_agent",
        role: "native_realtime_agent",
        model: "gemini-2.0-flash-live-001",
      }),
    ]);
    expect(byId(cards, "local_private").selected).toBe(false);
    expect(JSON.stringify(cards)).not.toContain(draftGeminiSecret);
  });

  it("does not select native realtime from the legacy flag when runtime mode is notes", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "local_whisper",
        llmType: "local_llama",
        llmModel: "lfm2-350m-extract-q4_k_m.gguf",
        geminiModel: "gemini-2.0-flash-live-001",
      }),
      credentialPresence: presence("gemini_api_key"),
      providerReadiness: [
        readyProvider("realtime_agent.gemini_live", ["gemini_api_key"]),
      ],
      conversationMode: "notes",
      converseEngine: "native",
      nativeRealtimeEnabled: true,
    });

    expect(byId(cards, "local_private").selected).toBe(true);
    expect(byId(cards, "native_realtime").selected).toBe(false);
  });

  it("adds no-source blockers only to audio-input provider stages", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "deepgram",
        deepgramModel: "nova-3",
        llmType: "openrouter",
        openrouterModel: "openai/gpt-4o-mini",
      }),
      credentialPresence: presence("deepgram_api_key", "openrouter_api_key"),
      providerReadiness: [
        readyProvider("asr.deepgram", ["deepgram_api_key"]),
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
      sourceState: sourceState([], [source()]),
    });

    const cloud = byId(cards, "cloud_fast");
    const cloudSourceBlockers = cloud.missingBlockers.filter((blocker) =>
      blocker.kind.startsWith("source_"),
    );
    expect(cloud.readinessStatus).toBe("blocked");
    expect(cloudSourceBlockers).toEqual([
      expect.objectContaining({
        kind: "source_unselected",
        providerId: "asr.deepgram",
        stage: "asr",
      }),
    ]);
    expect(cloudSourceBlockers).not.toEqual(
      expect.arrayContaining([
        expect.objectContaining({ providerId: "llm.openrouter" }),
        expect.objectContaining({ providerId: "tts.none" }),
      ]),
    );

    const native = byId(cards, "native_realtime");
    expect(native.missingBlockers).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: "source_unselected",
          providerId: "realtime_agent.gemini_live",
          stage: "realtime_agent",
        }),
      ]),
    );
  });

  it("reports unsupported and denied application source blockers", () => {
    const appSource = source({
      id: "app:42",
      name: "Design Tool",
      source_type: { type: "Application", pid: 42, app_name: "Design Tool" },
      capture_target: "app:42",
      capabilities: {
        backend_name: "FixtureBackend",
        capture_supported: false,
        supports_system_capture: true,
        supports_application_capture: false,
        supports_process_tree_capture: true,
        supports_device_selection: true,
        supports_device_change_notifications: true,
        unsupported_reason:
          "Application capture is not supported by FixtureBackend",
      },
      permission_status: "Denied",
      permission_recovery: {
        platform: "Macos",
        permission_kind: "AudioCapture",
        summary: "macOS Audio Capture permission is denied.",
        body: "Grant AudioGraph permission in macOS Privacy & Security, then relaunch AudioGraph and refresh sources.",
        actions: [
          {
            kind: "GrantPermissionManually",
            label: "Grant permission manually",
          },
        ],
      },
    });
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "deepgram",
        deepgramModel: "nova-3",
        llmType: "openrouter",
        openrouterModel: "openai/gpt-4o-mini",
      }),
      credentialPresence: presence("deepgram_api_key", "openrouter_api_key"),
      providerReadiness: [
        readyProvider("asr.deepgram", ["deepgram_api_key"]),
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
      sourceState: sourceState(["app:42"], [appSource]),
    });

    const cloud = byId(cards, "cloud_fast");
    expect(cloud.readinessStatus).toBe("blocked");
    expect(cloud.missingBlockers).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: "source_unsupported",
          providerId: "asr.deepgram",
          sourceId: "app:42",
          sourceName: "Design Tool",
        }),
        expect.objectContaining({
          kind: "source_permission_unavailable",
          providerId: "asr.deepgram",
          sourceId: "app:42",
          permissionStatus: "Denied",
          message: expect.stringContaining("macOS Audio Capture permission"),
          permissionRecovery: expect.objectContaining({
            permission_kind: "AudioCapture",
          }),
        }),
      ]),
    );
    expect(providerSetupSourceRecoveryIssues(cloud)).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: "unsupported",
          sourceId: "app:42",
          sourceName: "Design Tool",
        }),
        expect.objectContaining({
          kind: "permission",
          sourceId: "app:42",
          permissionStatus: "Denied",
          message: expect.stringContaining("macOS Audio Capture permission"),
          permissionRecovery: expect.objectContaining({
            platform: "Macos",
          }),
        }),
      ]),
    );
  });

  it("uses capture-target capability flags for process-tree support", () => {
    const treeSource = source({
      id: "opaque-row-7",
      name: "Build Tool Tree",
      source_type: { type: "ProcessTree", pid: 700 },
      capture_target: "process-tree:700",
      capabilities: {
        backend_name: "FixtureBackend",
        capture_supported: true,
        supports_system_capture: true,
        supports_application_capture: true,
        supports_process_tree_capture: false,
        supports_device_selection: true,
        supports_device_change_notifications: true,
        unsupported_reason: null,
      },
      permission_status: "Granted",
    });
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "deepgram",
        deepgramModel: "nova-3",
        llmType: "openrouter",
        openrouterModel: "openai/gpt-4o-mini",
      }),
      credentialPresence: presence("deepgram_api_key", "openrouter_api_key"),
      providerReadiness: [
        readyProvider("asr.deepgram", ["deepgram_api_key"]),
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
      sourceState: sourceState(["process-tree:700"], [treeSource]),
    });

    const cloud = byId(cards, "cloud_fast");
    expect(cloud.missingBlockers).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: "source_unsupported",
          providerId: "asr.deepgram",
          sourceId: "process-tree:700",
          message: expect.stringContaining(
            "Process-tree capture is not supported",
          ),
        }),
      ]),
    );
  });

  it("reports selected source ids that are no longer available", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "deepgram",
        deepgramModel: "nova-3",
        llmType: "openrouter",
        openrouterModel: "openai/gpt-4o-mini",
      }),
      credentialPresence: presence("deepgram_api_key", "openrouter_api_key"),
      providerReadiness: [
        readyProvider("asr.deepgram", ["deepgram_api_key"]),
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
      sourceState: sourceState(["device:stale"], [source()]),
    });

    const cloud = byId(cards, "cloud_fast");
    expect(cloud.readinessStatus).toBe("blocked");
    expect(cloud.missingBlockers).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: "source_unavailable",
          providerId: "asr.deepgram",
          sourceId: "device:stale",
        }),
      ]),
    );
  });

  it("blocks single-session providers when multiple sources are selected", () => {
    const cards = deriveProviderSetupModeCards({
      settings: settings({
        asrType: "assemblyai",
        llmType: "openrouter",
        openrouterModel: "openai/gpt-4o-mini",
      }),
      credentialPresence: presence("assemblyai_api_key", "openrouter_api_key"),
      providerReadiness: [
        readyProvider("asr.assemblyai", ["assemblyai_api_key"]),
        readyProvider("llm.openrouter", ["openrouter_api_key"]),
      ],
      sourceState: sourceState(
        ["system-default", "device:loopback"],
        [
          source(),
          source({
            id: "device:loopback",
            name: "Loopback Device",
            source_type: { type: "Device", device_id: "loopback" },
            capture_target: "device:loopback",
            permission_status: "NotRequired",
          }),
        ],
      ),
    });

    const cloud = byId(cards, "cloud_fast");
    expect(cloud.readinessStatus).toBe("blocked");
    expect(cloud.missingBlockers).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          kind: "source_policy_conflict",
          providerId: "asr.assemblyai",
          message: expect.stringContaining(
            "supports one selected audio source at a time",
          ),
        }),
      ]),
    );
  });
});
