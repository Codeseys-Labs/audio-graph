import { invoke } from "@tauri-apps/api/core";
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../../store";
import type { AppSettings, OpenRouterModelEndpoints } from "../../types";
import { inferOpenRouterRoutingPreset } from "./settingsControllerHelpers";
import { useSettingsController } from "./useSettingsController";

// The controller is the orchestration hook behind the Settings modal. Its
// OpenRouter accelerator handlers (discover + apply-preset) have historically
// had ZERO direct coverage — the presentational tests mock the callbacks. This
// suite exercises the two handlers against their REAL behavior over the mocked
// Tauri invoke boundary (globally stubbed in src/test/setup.ts):
//   1. handleDiscoverOpenRouterAccelerators — locks the saved-key catalog fetch
//      shape (no plaintext key readback; providers is best-effort with a
//      .catch fallback).
//   2. handleApplyAcceleratorPreset -> handleSave — a discovered accelerator
//      preset must persist as a `strict_accelerator` routing policy through the
//      existing buildOpenRouterRoutingPolicy path.
const mockedInvoke = vi.mocked(invoke);

/**
 * Minimal OpenRouter AppSettings so the controller's hydrate effect populates
 * the reducer with `openrouterModel` / `openrouterBaseUrl` and marks the LLM
 * provider as openrouter — the state the two handlers read from.
 */
function openrouterSettings(): AppSettings {
  return {
    asr_provider: { type: "local_whisper" },
    whisper_model: "base",
    llm_provider: {
      type: "openrouter",
      model: "meta-llama/llama-3.1-70b-instruct",
      base_url: "https://openrouter.ai/api/v1",
      include_usage_in_stream: true,
      provider_order: null,
    },
    llm_api_config: null,
    audio_settings: { sample_rate: 48000, channels: 2 },
    gemini: { auth: { type: "api_key", api_key: "" }, model: "" },
    tts_provider: { type: "none" },
    speak_aloud: false,
  };
}

const ENDPOINTS: OpenRouterModelEndpoints = {
  id: "meta-llama/llama-3.1-70b-instruct",
  name: "Llama 3.1 70B Instruct",
  endpoints: [
    {
      provider_name: "Cerebras",
      tag: "cerebras",
      quantization: "fp16",
      supported_parameters: [],
    },
    {
      provider_name: "Groq",
      tag: "groq",
      quantization: "fp8",
      supported_parameters: [],
    },
  ],
};

/**
 * Route the shared invoke stub. `providersReject` flips the providers-metadata
 * call to reject so the best-effort .catch fallback is observable. `savedKey`
 * controls whether the OpenRouter credential reads as present. Everything the
 * hydrate/readiness effects touch resolves to a safe empty value so the hook
 * mounts without noise.
 */
function stubInvoke(
  opts: { savedKey?: boolean; providersReject?: boolean } = {},
) {
  const { savedKey = true, providersReject = false } = opts;
  mockedInvoke.mockImplementation(async (cmd: string) => {
    switch (cmd) {
      case "load_credential_presence_cmd":
        return savedKey
          ? [
              {
                key: "openrouter_api_key",
                present: true,
                source: "credentials_yaml",
              },
            ]
          : [];
      case "list_openrouter_model_endpoints_cmd":
        return ENDPOINTS;
      case "list_openrouter_providers_cmd":
        if (providersReject) throw new Error("provider metadata unavailable");
        return [];
      case "get_provider_readiness_cmd":
        return [];
      default:
        return undefined;
    }
  });
}

/** Mount the controller with the OpenRouter settings hydrated + credential present. */
async function mountController() {
  const view = renderHook(() => useSettingsController());
  // The hydrate effect runs off `settings` and asynchronously loads credential
  // presence; wait until both the model is hydrated and the saved key is
  // recognized so `openrouterCredentialAvailable` gates open.
  await waitFor(() => {
    expect(view.result.current.openrouterModel).toBe(
      "meta-llama/llama-3.1-70b-instruct",
    );
    expect(view.result.current.openrouterCredentialAvailable).toBe(true);
  });
  return view;
}

describe("useSettingsController — OpenRouter accelerator discovery + apply", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    useAudioGraphStore.setState({
      settings: openrouterSettings(),
      // Capture the save payload without hitting the backend or triggering the
      // real store's redacted reload (which would re-hydrate the reducer).
      saveSettings: vi.fn(async () => {}),
      notify: vi.fn(() => "ntf-test"),
    } as never);
    stubInvoke();
  });

  afterEach(() => {
    vi.clearAllMocks();
    useAudioGraphStore.setState({ settings: null } as never);
  });

  it("discover uses ONLY the saved-key catalog commands (no plaintext key readback)", async () => {
    const view = await mountController();

    await act(async () => {
      await view.result.current.handleDiscoverOpenRouterAccelerators();
    });

    const endpointsCall = mockedInvoke.mock.calls.find(
      ([cmd]) => cmd === "list_openrouter_model_endpoints_cmd",
    );
    const providersCall = mockedInvoke.mock.calls.find(
      ([cmd]) => cmd === "list_openrouter_providers_cmd",
    );

    // The endpoint catalog is the load-bearing call: keyed by model id + base
    // URL, and CRITICALLY it never forwards a plaintext api key — discovery
    // rides the saved-key path (seed 7809 / ADR-0005).
    expect(endpointsCall?.[1]).toEqual({
      modelId: "meta-llama/llama-3.1-70b-instruct",
      baseUrl: "https://openrouter.ai/api/v1",
    });
    expect(endpointsCall?.[1]).not.toHaveProperty("apiKey");

    // Provider metadata is best-effort enrichment, keyed by base URL only.
    expect(providersCall?.[1]).toEqual({
      baseUrl: "https://openrouter.ai/api/v1",
    });

    // Never read the plaintext credential back to drive discovery.
    expect(mockedInvoke.mock.calls.map(([cmd]) => cmd)).not.toContain(
      "load_credential_cmd",
    );

    // Both raw payloads land in state on the happy path.
    expect(view.result.current.openrouterAcceleratorEndpoints).toEqual(
      ENDPOINTS,
    );
    expect(view.result.current.openrouterAcceleratorProviders).toEqual([]);
    expect(view.result.current.openrouterAcceleratorError).toBeNull();
    expect(view.result.current.openrouterAcceleratorLoading).toBe(false);
  });

  it("discover keeps the endpoint table when best-effort provider metadata fails", async () => {
    stubInvoke({ providersReject: true });
    const view = await mountController();

    await act(async () => {
      await view.result.current.handleDiscoverOpenRouterAccelerators();
    });

    // A provider-fetch rejection must NOT blank the endpoints or surface an
    // error — the Promise.all([..., ...catch(() => [])]) fallback keeps the
    // load-bearing endpoint payload and degrades providers to [].
    expect(view.result.current.openrouterAcceleratorEndpoints).toEqual(
      ENDPOINTS,
    );
    expect(view.result.current.openrouterAcceleratorProviders).toEqual([]);
    expect(view.result.current.openrouterAcceleratorError).toBeNull();
  });

  it("discover is a no-op without an available credential", async () => {
    stubInvoke({ savedKey: false });
    const view = renderHook(() => useSettingsController());
    await waitFor(() =>
      expect(view.result.current.openrouterModel).toBe(
        "meta-llama/llama-3.1-70b-instruct",
      ),
    );
    expect(view.result.current.openrouterCredentialAvailable).toBe(false);
    mockedInvoke.mockClear();

    await act(async () => {
      await view.result.current.handleDiscoverOpenRouterAccelerators();
    });

    expect(mockedInvoke.mock.calls.map(([cmd]) => cmd)).not.toContain(
      "list_openrouter_model_endpoints_cmd",
    );
  });

  it("apply-preset persists the ranked slugs as a strict_accelerator routing policy", async () => {
    const view = await mountController();

    // A discovered preset maps EVERY dynamic preset onto strict_accelerator so
    // the ranked slug order flows through buildOpenRouterRoutingPolicy.
    act(() => {
      view.result.current.handleApplyAcceleratorPreset("low_latency", [
        "cerebras",
        "groq",
      ]);
    });

    expect(view.result.current.openrouterRoutingPreset).toBe(
      "strict_accelerator",
    );
    expect(view.result.current.openrouterProviderOrderText).toBe(
      "cerebras, groq",
    );
    expect(view.result.current.openrouterAppliedAcceleratorPreset).toBe(
      "low_latency",
    );

    const saveSettings = useAudioGraphStore.getState()
      .saveSettings as unknown as ReturnType<typeof vi.fn>;

    await act(async () => {
      await view.result.current.handleSave();
    });

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0][0] as AppSettings;

    // The applied preset must persist as a strict_accelerator policy: the
    // ranked slugs pinned into BOTH provider.order and provider.only with
    // fallbacks disabled.
    expect(saved.openrouter_routing_policy).toEqual({
      order: ["cerebras", "groq"],
      only: ["cerebras", "groq"],
      ignore: [],
      quantizations: [],
      allow_fallbacks: false,
    });

    // Round-trip: the persisted policy infers back to strict_accelerator, so a
    // reload of these settings re-selects the same preset in the UI.
    expect(inferOpenRouterRoutingPreset(saved.openrouter_routing_policy)).toBe(
      "strict_accelerator",
    );
  });

  it("apply-preset ignores an empty slug list", async () => {
    const view = await mountController();
    const before = view.result.current.openrouterRoutingPreset;

    act(() => {
      view.result.current.handleApplyAcceleratorPreset("high_throughput", []);
    });

    expect(view.result.current.openrouterRoutingPreset).toBe(before);
    expect(view.result.current.openrouterAppliedAcceleratorPreset).toBeNull();
  });
});
