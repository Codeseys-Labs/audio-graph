import { invoke } from "@tauri-apps/api/core";
import { act, renderHook, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useAudioGraphStore } from "../../store";
import type { AppSettings, OpenRouterModelEndpoints } from "../../types";
import { setField } from "../settingsTypes";
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

// ── Interactive mode selection (settings redesign WS1 / FINAL DECISION 1) ───
// `handleSelectProductMode` is the fix for the stuck-on-native bug: picking a
// product-mode card must drive the store + reducer so `selectedModeId()`
// re-classifies to that card. Native is a two-flag store toggle; the three
// durable cards additionally swap the reducer's ASR/LLM provider selection to
// the providers the card is DERIVED from (local/cloud/hybrid classify from
// provider locality, so a bare flag flip cannot move between them).

/**
 * Baseline settings the mode-selection tests hydrate from: pipelined-notes
 * with a hybrid provider pair (local ASR + cloud LLM) so the initial
 * classification is `hybrid` and every target transition is observable.
 */
function modeSelectionSettings(): AppSettings {
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

function selectedCardId(view: {
  result: { current: ReturnType<typeof useSettingsController> };
}): string | undefined {
  return view.result.current.providerSetupModeCards.find(
    (card) => card.selected,
  )?.id;
}

describe("useSettingsController — handleSelectProductMode", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    // Start every case from pipelined-notes so the native transition is a real
    // change, not a no-op against a pre-native store.
    useAudioGraphStore.setState({
      settings: modeSelectionSettings(),
      conversationMode: "notes",
      converseEngine: "pipelined",
      saveSettings: vi.fn(async () => {}),
      notify: vi.fn(() => "ntf-test"),
    } as never);
    mockedInvoke.mockImplementation(async (cmd: string) => {
      switch (cmd) {
        case "load_credential_presence_cmd":
          return [];
        case "get_provider_readiness_cmd":
          return [];
        default:
          return undefined;
      }
    });
  });

  afterEach(() => {
    vi.clearAllMocks();
    useAudioGraphStore.setState({
      settings: null,
      conversationMode: "notes",
      converseEngine: "pipelined",
    } as never);
  });

  async function mountForModeSelection() {
    const view = renderHook(() => useSettingsController());
    // Wait for the hydrate effect to map the store settings into the reducer so
    // asrType/llmType reflect the baseline before we start toggling.
    await waitFor(() => {
      expect(view.result.current.asrType).toBe("local_whisper");
      expect(view.result.current.llmType).toBe("openrouter");
    });
    return view;
  }

  it("native card sets converse + native (two-flag store toggle)", async () => {
    const view = await mountForModeSelection();
    const nativeCard = view.result.current.providerSetupModeCards.find(
      (card) => card.id === "native_realtime",
    );
    expect(nativeCard).toBeDefined();

    act(() => {
      if (nativeCard) view.result.current.handleSelectProductMode(nativeCard);
    });

    const store = useAudioGraphStore.getState();
    expect(store.conversationMode).toBe("converse");
    expect(store.converseEngine).toBe("native");
    // Legacy native-S2S flag stays in sync via setConverseEngine.
    expect(store.nativeS2sEnabled).toBe(true);

    await waitFor(() => {
      expect(selectedCardId(view)).toBe("native_realtime");
    });
  });

  it("local_private card sets notes + pipelined and swaps to local ASR + local LLM", async () => {
    const view = await mountForModeSelection();
    const card = view.result.current.providerSetupModeCards.find(
      (c) => c.id === "local_private",
    );
    expect(card).toBeDefined();

    act(() => {
      if (card) view.result.current.handleSelectProductMode(card);
    });

    const store = useAudioGraphStore.getState();
    expect(store.conversationMode).toBe("notes");
    expect(store.converseEngine).toBe("pipelined");

    await waitFor(() => {
      expect(view.result.current.asrType).toBe("local_whisper");
      expect(view.result.current.llmType).toBe("local_llama");
      // selectedModeId re-classifies from provider locality.
      expect(selectedCardId(view)).toBe("local_private");
    });
  });

  it("cloud_fast card sets notes + pipelined and swaps to cloud ASR + cloud LLM", async () => {
    const view = await mountForModeSelection();
    const card = view.result.current.providerSetupModeCards.find(
      (c) => c.id === "cloud_fast",
    );
    expect(card).toBeDefined();

    act(() => {
      if (card) view.result.current.handleSelectProductMode(card);
    });

    const store = useAudioGraphStore.getState();
    expect(store.conversationMode).toBe("notes");
    expect(store.converseEngine).toBe("pipelined");

    await waitFor(() => {
      expect(view.result.current.asrType).toBe("deepgram");
      expect(view.result.current.llmType).toBe("openrouter");
      expect(selectedCardId(view)).toBe("cloud_fast");
    });
  });

  it("hybrid card sets notes + pipelined and swaps to local ASR + cloud LLM", async () => {
    const view = await mountForModeSelection();
    // Move away from the hybrid baseline first (to cloud_fast) so selecting
    // hybrid is an observable transition rather than a no-op.
    const cloudCard = view.result.current.providerSetupModeCards.find(
      (c) => c.id === "cloud_fast",
    );
    act(() => {
      if (cloudCard) view.result.current.handleSelectProductMode(cloudCard);
    });
    await waitFor(() => {
      expect(view.result.current.asrType).toBe("deepgram");
    });

    const hybridCard = view.result.current.providerSetupModeCards.find(
      (c) => c.id === "hybrid",
    );
    expect(hybridCard).toBeDefined();

    act(() => {
      if (hybridCard) view.result.current.handleSelectProductMode(hybridCard);
    });

    const store = useAudioGraphStore.getState();
    expect(store.conversationMode).toBe("notes");
    expect(store.converseEngine).toBe("pipelined");

    await waitFor(() => {
      // Hybrid = local ASR + cloud LLM (pickHybridAsrProviderId /
      // pickHybridLlmProviderId derive one local + one cloud provider).
      expect(view.result.current.asrType).toBe("local_whisper");
      expect(view.result.current.llmType).toBe("openrouter");
      expect(selectedCardId(view)).toBe("hybrid");
    });
  });
});

// ── In-place credential save from the Credentials & readiness tab ───────────
// The credential-health rows were STATUS-ONLY (Replace navigated to the STT/LLM
// tab; the global footer Save silently sent "" when the field wasn't edited this
// session). `handleSaveCredentialValue` lets a row save its key in place through
// the SAME `save_credential_cmd` path, and an empty/whitespace draft must set a
// visible notice instead of silently no-op'ing.
describe("useSettingsController — in-place credential save", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    useAudioGraphStore.setState({
      settings: openrouterSettings(),
      saveSettings: vi.fn(async () => {}),
      notify: vi.fn(() => "ntf-test"),
    } as never);
    stubInvoke();
  });

  afterEach(() => {
    vi.clearAllMocks();
    useAudioGraphStore.setState({ settings: null } as never);
  });

  it("saves a non-empty draft via save_credential_cmd and clears the draft", async () => {
    const view = await mountController();

    act(() => {
      view.result.current.setCredentialDraft("deepgram_api_key", "dg-secret");
    });
    expect(view.result.current.credentialDrafts.deepgram_api_key).toBe(
      "dg-secret",
    );

    mockedInvoke.mockClear();
    await act(async () => {
      await view.result.current.handleSaveCredentialValue("deepgram_api_key");
    });

    // The reused global save path fires with the row's key + typed value.
    const saveCall = mockedInvoke.mock.calls.find(
      ([cmd]) => cmd === "save_credential_cmd",
    );
    expect(saveCall?.[1]).toEqual({
      key: "deepgram_api_key",
      value: "dg-secret",
    });

    // On success the local draft is dropped and the row reports "saved".
    expect(
      view.result.current.credentialDrafts.deepgram_api_key,
    ).toBeUndefined();
    expect(view.result.current.credentialSaveNotice.deepgram_api_key).toBe(
      "saved",
    );
  });

  it("does NOT invoke on an empty/whitespace draft and surfaces the empty notice", async () => {
    const view = await mountController();

    act(() => {
      view.result.current.setCredentialDraft("deepgram_api_key", "   ");
    });

    mockedInvoke.mockClear();
    await act(async () => {
      await view.result.current.handleSaveCredentialValue("deepgram_api_key");
    });

    // The silent no-op is replaced by a visible "empty" notice; no save fires.
    expect(mockedInvoke.mock.calls.map(([cmd]) => cmd)).not.toContain(
      "save_credential_cmd",
    );
    expect(view.result.current.credentialSaveNotice.deepgram_api_key).toBe(
      "empty",
    );
  });

  it("clears a stale notice when the draft is edited again", async () => {
    const view = await mountController();

    // Trip the empty notice first.
    await act(async () => {
      await view.result.current.handleSaveCredentialValue("deepgram_api_key");
    });
    expect(view.result.current.credentialSaveNotice.deepgram_api_key).toBe(
      "empty",
    );

    // Typing again clears the outcome so the row is not stuck showing "empty".
    act(() => {
      view.result.current.setCredentialDraft("deepgram_api_key", "dg-new");
    });
    expect(
      view.result.current.credentialSaveNotice.deepgram_api_key,
    ).toBeUndefined();
  });
});

// The realtime-agent readiness set surfaces the agent the user actually runs in
// native speech-to-speech. Before WS3 (ADR-0006 B1 decision 3) only the Gemini
// Live agent was appended to `activeReadinessProviderIds`, so a native + OpenAI
// Realtime setup silently dropped OpenAI Realtime agent readiness from the
// Credentials view. This suite locks the by-agent branch.
describe("useSettingsController — native realtime agent readiness", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    useAudioGraphStore.setState({
      settings: openrouterSettings(),
      saveSettings: vi.fn(async () => {}),
      notify: vi.fn(() => "ntf-test"),
    } as never);
    stubInvoke();
  });

  afterEach(() => {
    vi.clearAllMocks();
    useAudioGraphStore.setState({
      settings: null,
      conversationMode: "notes",
      converseEngine: "pipelined",
      converseRealtimeAgentProvider: "gemini",
    } as never);
  });

  it("appends the OpenAI Realtime agent id when native + OpenAI is selected", async () => {
    useAudioGraphStore.setState({
      conversationMode: "converse",
      converseEngine: "native",
      converseRealtimeAgentProvider: "openai",
    } as never);

    const view = await mountController();

    const ids = view.result.current.activeReadinessProviderIdSet;
    expect(ids.has("realtime_agent.openai_realtime")).toBe(true);
    expect(ids.has("realtime_agent.gemini_live")).toBe(false);
  });

  it("appends the Gemini Live agent id when native + Gemini is selected", async () => {
    useAudioGraphStore.setState({
      conversationMode: "converse",
      converseEngine: "native",
      converseRealtimeAgentProvider: "gemini",
    } as never);

    const view = await mountController();

    const ids = view.result.current.activeReadinessProviderIdSet;
    expect(ids.has("realtime_agent.gemini_live")).toBe(true);
    expect(ids.has("realtime_agent.openai_realtime")).toBe(false);
  });

  it("appends no realtime agent id when native is not selected", async () => {
    useAudioGraphStore.setState({
      conversationMode: "notes",
      converseEngine: "pipelined",
      converseRealtimeAgentProvider: "openai",
    } as never);

    const view = await mountController();

    const ids = view.result.current.activeReadinessProviderIdSet;
    expect(ids.has("realtime_agent.openai_realtime")).toBe(false);
    expect(ids.has("realtime_agent.gemini_live")).toBe(false);
  });

  // Data-integrity regression guard (split-brain): the NATIVE voice-agent
  // OpenAI credential (`realtime_agent.openai_realtime`) shares the OpenAI key
  // with the pipeline-STT provider (`asr.openai_realtime`) but is a DIFFERENT
  // provider. Now that WS3 surfaces the native-agent id in the active readiness
  // set, its "Add or replace credential" route must navigate to the Realtime-
  // agent tab's capability card WITHOUT rewriting the user's saved STT provider
  // (`asrType`).
  it("native OpenAI-agent credential route navigates without mutating asrType", async () => {
    useAudioGraphStore.setState({
      conversationMode: "converse",
      converseEngine: "native",
      converseRealtimeAgentProvider: "openai",
    } as never);

    const view = await mountController();
    const asrTypeBefore = view.result.current.asrType;

    const route = view.result.current.credentialRouteForProviderCredential(
      "realtime_agent.openai_realtime",
      "openai_api_key",
    );

    // It STILL navigates — but to the Realtime-agent tab's capability card, NOT
    // the STT tab's openai-realtime field (which only renders when
    // asrType === "openai_realtime").
    expect(route).not.toBeNull();
    expect(route?.tab).toBe("gemini");
    expect(route?.fieldId).toBe(
      "settings-provider-capability-realtime_agent.openai_realtime",
    );
    expect(route?.activate).toBe(true);

    // But it must NOT carry an `apply` that rewrites asrType — that would
    // corrupt asr_provider to "openai_realtime" on the next Save.
    expect(route?.apply).toBeUndefined();

    // Belt-and-braces: even if invoked, asrType is untouched.
    act(() => {
      route?.apply?.();
    });
    expect(view.result.current.asrType).toBe(asrTypeBefore);

    // Contrast: the pipeline-STT provider route (asr.openai_realtime) SHOULD
    // still set asrType — that is the real STT provider selector.
    const asrRoute = view.result.current.credentialRouteForProviderCredential(
      "asr.openai_realtime",
      "openai_api_key",
    );
    expect(asrRoute?.apply).toBeTypeOf("function");
    act(() => {
      asrRoute?.apply?.();
    });
    expect(view.result.current.asrType).toBe("openai_realtime");
  });
});

// ── analytics_enabled persistence (Sentry clobber regression) ───────────────
// The footer "Save" builds its AppSettings payload field-by-field and used to
// OMIT `analytics_enabled`, so the payload carried `undefined`. The backend
// field is `skip_serializing_if = Option::is_none`, so a whole-struct write
// silently DROPPED the key from config.yaml — clobbering the `true` that the
// separate `set_analytics_enabled` toggle had written and leaving Sentry off.
// The footer Save must thread the store's loaded `analytics_enabled` through
// (like `demo_mode`) so a Save after enabling analytics never sends undefined.
describe("useSettingsController — analytics_enabled persistence", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    useAudioGraphStore.setState({
      settings: {
        ...openrouterSettings(),
        // The store loaded analytics as ON (the toggle wrote true to disk and
        // load_settings_cmd surfaced it).
        analytics_enabled: true,
      },
      saveSettings: vi.fn(async () => {}),
      notify: vi.fn(() => "ntf-test"),
    } as never);
    stubInvoke();
  });

  afterEach(() => {
    vi.clearAllMocks();
    useAudioGraphStore.setState({
      settings: null,
      analyticsEnabled: undefined,
    } as never);
  });

  it("footer Save preserves the loaded analytics_enabled instead of sending undefined", async () => {
    const view = await mountController();

    const saveSettings = useAudioGraphStore.getState()
      .saveSettings as unknown as ReturnType<typeof vi.fn>;

    await act(async () => {
      await view.result.current.handleSave();
    });

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0][0] as AppSettings;

    // The loaded ON value must ride through the footer Save payload — NOT be
    // dropped to undefined (which the backend would treat as None and clobber).
    expect(saved.analytics_enabled).toBe(true);
  });

  // The Logging-panel toggle writes its live value to the dedicated
  // `analyticsEnabled` store slice (NOT `settings.analytics_enabled`). The
  // footer Save must PREFER that slice so a toggle made this session is
  // authoritative and can't be reverse-clobbered by a now-stale
  // `settings.analytics_enabled`.
  it("footer Save prefers the live analyticsEnabled slice over stale settings.analytics_enabled", async () => {
    // Loaded value is OFF, but the user just toggled analytics ON this session
    // (the toggle wrote true via set_analytics_enabled and recorded it in the
    // dedicated slice, without mutating the settings object identity).
    useAudioGraphStore.setState({
      settings: {
        ...openrouterSettings(),
        analytics_enabled: false,
      },
      analyticsEnabled: true,
    } as never);

    const view = await mountController();
    const saveSettings = useAudioGraphStore.getState()
      .saveSettings as unknown as ReturnType<typeof vi.fn>;

    await act(async () => {
      await view.result.current.handleSave();
    });

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0][0] as AppSettings;
    // The live toggle value (true) wins over the stale loaded value (false):
    // sending false here would reverse-clobber the on-disk true.
    expect(saved.analytics_enabled).toBe(true);
  });

  // REGRESSION LOCK: toggling analytics must NOT wipe unsaved form edits.
  //
  // The Settings form re-hydrates from the `settings` store object whenever its
  // IDENTITY changes (the hydrate effect keys on `[settings, ...]`). An earlier
  // fix synced the toggle by patching `settings` — which replaced that identity
  // and silently reset the whole form. The toggle now writes a dedicated
  // `analyticsEnabled` slice instead, leaving `settings` identity untouched, so
  // an in-flight edit survives.
  it("toggling analytics does NOT wipe an unsaved form edit", async () => {
    useAudioGraphStore.setState({
      settings: {
        ...openrouterSettings(),
        analytics_enabled: false,
      },
      analyticsEnabled: undefined,
    } as never);

    const view = await mountController();

    // Unsaved edit: change the whisper model in the reducer-backed form.
    act(() => {
      view.result.current.dispatch(setField("whisperModel", "large-v3"));
    });
    await waitFor(() => {
      expect(view.result.current.whisperModel).toBe("large-v3");
    });

    // Simulate exactly what the Logging-panel toggle now does on success: write
    // the dedicated slice, WITHOUT touching the shared settings object.
    act(() => {
      useAudioGraphStore.setState({ analyticsEnabled: true } as never);
    });

    // The form edit must survive — the hydrate effect must not have re-fired.
    await waitFor(() => {
      expect(useAudioGraphStore.getState().analyticsEnabled).toBe(true);
    });
    expect(view.result.current.whisperModel).toBe("large-v3");
  });

  // Guard rail proving the regression test above has teeth: mutating the
  // `settings` object identity (the OLD, buggy toggle behavior) DOES re-hydrate
  // the form and wipe the unsaved edit. This documents WHY the toggle must stay
  // out of the settings-identity flow.
  it("mutating settings identity re-hydrates the form and wipes an unsaved edit (documents the trap)", async () => {
    useAudioGraphStore.setState({
      settings: {
        ...openrouterSettings(),
        analytics_enabled: false,
      },
      analyticsEnabled: undefined,
    } as never);

    const view = await mountController();

    act(() => {
      view.result.current.dispatch(setField("whisperModel", "large-v3"));
    });
    await waitFor(() => {
      expect(view.result.current.whisperModel).toBe("large-v3");
    });

    // Replace the settings object identity (what patching settings did).
    act(() => {
      useAudioGraphStore.setState({
        settings: {
          ...openrouterSettings(),
          analytics_enabled: true,
        },
      } as never);
    });

    // The hydrate effect re-fires and resets whisperModel back to the loaded
    // "base" — the unsaved "large-v3" edit is gone. This is the wipe the fix
    // avoids by using the dedicated slice.
    await waitFor(() => {
      expect(view.result.current.whisperModel).toBe("base");
    });
  });
});

// ---------------------------------------------------------------------------
// Provider-selection → config accuracy (audit 2026-07-05)
//
// The chain under test: backend AppSettings → hydrate (load path) → reducer →
// handleSave (save path) → saveSettings payload. A selected provider + model +
// tuning params must survive the full round-trip byte-identical — the drift
// class of the historical Deepgram `model="general"` bug.
// ---------------------------------------------------------------------------

/** AppSettings with a fully non-default Deepgram ASR provider. */
function deepgramSettings(): AppSettings {
  return {
    ...openrouterSettings(),
    asr_provider: {
      type: "deepgram",
      api_key: "",
      model: "nova-3-medical",
      enable_diarization: true,
      endpointing_ms: 450,
      utterance_end_ms: 1500,
      vad_events: false,
      eot_threshold: 0.65,
      eager_eot_threshold: 0.4,
      eot_timeout_ms: 7000,
      max_speakers: 4,
    },
    diarization: {
      mode: "provider",
      speaker_count: "auto",
      max_speakers: null,
    },
  };
}

/** Mount the controller and wait until the Deepgram config has hydrated. */
async function mountDeepgramController(expectedModel: string) {
  const view = renderHook(() => useSettingsController());
  await waitFor(() => {
    expect(view.result.current.asrType).toBe("deepgram");
    expect(view.result.current.deepgramModel).toBe(expectedModel);
  });
  return view;
}

describe("useSettingsController — provider selection/config round-trip", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
    useAudioGraphStore.setState({
      settings: deepgramSettings(),
      saveSettings: vi.fn(async () => {}),
      notify: vi.fn(() => "ntf-test"),
    } as never);
    stubInvoke();
  });

  afterEach(() => {
    vi.clearAllMocks();
    useAudioGraphStore.setState({ settings: null } as never);
  });

  it("a hydrated non-default Deepgram config survives Save byte-identical", async () => {
    const view = await mountDeepgramController("nova-3-medical");
    const saveSettings = useAudioGraphStore.getState()
      .saveSettings as unknown as ReturnType<typeof vi.fn>;

    await act(async () => {
      await view.result.current.handleSave();
    });

    expect(saveSettings).toHaveBeenCalledTimes(1);
    const saved = saveSettings.mock.calls[0][0] as AppSettings;
    // The entire provider config must round-trip unchanged: hydrate defaults
    // (`?? …`) and save clamps (`Math.max`/`Math.min`/`Math.round`) must all
    // be identity for an in-range persisted config.
    expect(saved.asr_provider).toEqual({
      type: "deepgram",
      api_key: "",
      model: "nova-3-medical",
      enable_diarization: true,
      endpointing_ms: 450,
      utterance_end_ms: 1500,
      vad_events: false,
      eot_threshold: 0.65,
      eager_eot_threshold: 0.4,
      eot_timeout_ms: 7000,
      max_speakers: 4,
    });
  });

  // REGRESSION (audit finding #1): settings persisted before the
  // `max_speakers` field existed hydrate WITHOUT the key. The hydrate
  // fallback used `?? 2` while the backend serde default (BUG-4) and
  // `initialSettingsState` are both 0 (= no cap) — so merely opening
  // Settings and hitting Save silently re-capped transcription to 2
  // speakers. The fallback must be 0.
  it("hydrates a missing max_speakers to 0 (no cap) and Save persists 0", async () => {
    const legacy = deepgramSettings();
    if (legacy.asr_provider.type === "deepgram") {
      delete (legacy.asr_provider as { max_speakers?: number }).max_speakers;
    }
    useAudioGraphStore.setState({ settings: legacy } as never);

    const view = await mountDeepgramController("nova-3-medical");
    expect(view.result.current.deepgramMaxSpeakers).toBe(0);

    const saveSettings = useAudioGraphStore.getState()
      .saveSettings as unknown as ReturnType<typeof vi.fn>;
    await act(async () => {
      await view.result.current.handleSave();
    });
    const saved = saveSettings.mock.calls[0][0] as AppSettings;
    expect(saved.asr_provider.type).toBe("deepgram");
    if (saved.asr_provider.type === "deepgram") {
      expect(saved.asr_provider.max_speakers).toBe(0);
    }
  });

  it("save clamps out-of-range Deepgram tuning values into the wire domain", async () => {
    const view = await mountDeepgramController("nova-3-medical");

    // Drive the reducer with hostile values the number inputs could produce.
    act(() => {
      view.result.current.dispatch(setField("deepgramEndpointingMs", -50));
      view.result.current.dispatch(setField("deepgramUtteranceEndMs", 999.6));
      view.result.current.dispatch(setField("deepgramEotThreshold", 1.4));
      view.result.current.dispatch(setField("deepgramEagerEotThreshold", 2.0));
      view.result.current.dispatch(setField("deepgramEotTimeoutMs", -1));
      view.result.current.dispatch(setField("deepgramMaxSpeakers", -3));
    });

    const saveSettings = useAudioGraphStore.getState()
      .saveSettings as unknown as ReturnType<typeof vi.fn>;
    await act(async () => {
      await view.result.current.handleSave();
    });
    const saved = saveSettings.mock.calls[0][0] as AppSettings;
    expect(saved.asr_provider.type).toBe("deepgram");
    if (saved.asr_provider.type === "deepgram") {
      const asr = saved.asr_provider;
      // ms fields: rounded, floored at 0 (0 = backend "not configured").
      expect(asr.endpointing_ms).toBe(0);
      expect(asr.utterance_end_ms).toBe(1000);
      expect(asr.eot_timeout_ms).toBe(0);
      // eot_threshold clamps to [0, 1].
      expect(asr.eot_threshold).toBe(1);
      // eager clamps to <= eot (the backend drops invalid pairs; the UI
      // must not persist one in the first place).
      expect(asr.eager_eot_threshold).toBeLessThanOrEqual(
        asr.eot_threshold ?? 0,
      );
      expect(asr.max_speakers).toBe(0);
    }
  });

  it("each implemented ASR selection saves the matching backend serde tag", async () => {
    // Selecting a provider variant in the UI must produce a payload whose
    // `type` tag is exactly the backend's serde rename for that variant —
    // the dispatch in speech/mod.rs keys off this tag.
    const view = await mountDeepgramController("nova-3-medical");
    const saveSettings = useAudioGraphStore.getState()
      .saveSettings as unknown as ReturnType<typeof vi.fn>;

    const expected: Array<[string, string]> = [
      ["local_whisper", "local_whisper"],
      ["api", "api"],
      ["openai_realtime", "openai_realtime"],
      ["aws_transcribe", "aws_transcribe"],
      ["deepgram", "deepgram"],
      ["assemblyai", "assemblyai"],
      ["sherpa_onnx", "sherpa_onnx"],
    ];
    for (const [variant, tag] of expected) {
      act(() => {
        view.result.current.dispatch(setField("asrType", variant as never));
      });
      await act(async () => {
        await view.result.current.handleSave();
      });
      const saved = saveSettings.mock.calls.at(-1)?.[0] as AppSettings;
      expect(saved.asr_provider.type).toBe(tag);
    }
  });
});
